# Rencana performa: melampaui TablePro di setiap sumbu

- **Status:** rencana kerja, 29 Sep 2026. Belum ada yang dikerjakan.
- **Konteks:** hasil audit statis QueryHive dan TablePro. Setiap klaim yang dipakai di sini sudah diperiksa ulang terhadap kode di pohon ini, dan path serta nomor barisnya disebut. Pemilik melonggarkan batasnya. ADR lama diperlakukan sebagai catatan, dan rencana ini boleh membatalkannya asal menyebut yang mana (§14). Penggantian data plane masuk lingkup sekarang.
- **Pasangan:** `docs/benchmarks.md` (angka), `docs/architecture/remaining-work-plan.md` Batch 7 (sort dan search ke server), dan `docs/architecture/tablepro-adoption-plan.md` §12.2 dan §13 (dicatat di §15, mana yang diserap dan mana yang digantikan).

## Ringkasan

QueryHive sudah menang di engine. Lewat CLI, baris pertama tiba 11 ms setelah connect dan RSS 9 MB untuk 500k × 30. Kekalahannya ada di jalur app, di lima tempat:

- setiap perintah membangun runtime tokio dan koneksi baru;
- setiap 200 baris menjadi satu baris JSON yang di-decode Swift di thread engine;
- seluruh hasil ditahan sebagai `[[String?]]`;
- grid adalah SwiftUI `LazyVStack` yang membangun view untuk setiap sel;
- editor memindai seluruh dokumen pada setiap ketikan.

TablePro menang di keempat titik itu karena ia in-process, grid-nya menggambar sel dengan CoreText, dan editornya lazy. Ia kalah di tempat lain: ia tidak menampilkan hasil sambil mengalir, tidak spill ke disk, memakai memori O(total baris) setelah fetch, dan tidak punya benchmark sama sekali.

Urutannya menurut hasil yang terasa per ongkos. Pertama ukur (Fase 0). Lalu buang ongkos per panggilan dan perbaiki Stop (Fase 1), pakai ulang koneksi (Fase 2), dan daratkan Batch 7 di atasnya (Fase 3). Sesudah itu editor (Fase 4), grid (Fase 5), data plane (Fase 6), dan plafon ingest (Fase 7). Setiap fase bisa di-commit sendiri, punya gate benchmark, dan mengurangi kode lebih banyak daripada menambah bila memungkinkan.

Yang tidak berubah: NDJSON tetap menjadi protokol CLI, MCP, dan korpus golden. Renderer nilai tetap satu, yaitu `qh_core::render::to_text`. Tidak ada kode TablePro yang masuk pohon ini.

## 0. Prinsip, prasyarat, dan aturan gate

**Lisensi.** TablePro berlisensi AGPL-3.0, QueryHive MIT (`LICENSE`, ADR-0002, `deny.toml`). TablePro hanya dipelajari sebagai ide dan angka, dan kodenya ditulis ulang di sini. Editor pihak ketiga, bila suatu hari dipakai, harus proyek upstream (CodeEditApp) yang lisensinya sudah diverifikasi dari repo miliknya sendiri. Fork di `TablePro/Packages/TableProEditor` tidak boleh dipakai. Setiap dependensi baru tercantum di §16 dan harus lulus `cargo deny check licenses`.

**Prasyarat P: pemilik meng-commit pekerjaan UI yang belum di-commit.** Pohon kerja memuat perubahan di `Views/ResultGrid.swift`, `Views/SQLEditor.swift`, `Views/CellValueViewer.swift`, `Views/SettingsView.swift`, `Support/Theme.swift`, `Support/Snapshot.swift`, dan `Support/DataPreferences.swift`, ditambah `Tests/.../DataPreferencesTests.swift` dan `HighlightBandTests.swift` yang belum dilacak. Rencana ini tidak menganggap perubahan itu dibuang. Baseline visual Fase 0 direkam dari commit P. Fase 3, 4, dan 5 dimulai dari commit itu.

**Angka.** Kebijakan `docs/benchmarks.md` berlaku: angka yang belum diukur ditulis **[belum diukur]**. Semua rekaman masuk `deploy/dev/bench-results.jsonl`, dan `docs/benchmarks.md` hanya dihasilkan oleh generator.

**Gate standar setiap fase.** Sebuah fase ditutup hanya bila semua perintah ini hijau dan hasilnya dicatat:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check licenses
./app/build-ffi.sh        # bila permukaan UniFFI berubah; app/Generated ikut di-commit (invariant #1, #11)
cd app && swift build && swift test && cd ..
./app/build.sh
python3 deploy/dev/bench_app.py --axis <sumbu fase> --label <fase>-<tanggal> --repeat <n>
python3 deploy/dev/bench_fetch.py --report-only
/usr/bin/python3 tools/golden/live_cases.py   # bila container hidup; bila tidak, dicatat "tidak dijalankan"
```

Fase UI (3, 4, dan 5) juga harus lulus **gate paritas visual** (Fase 0.8).

**Commit.** Satu commit lokal per fase, atau per sub-fase bila sub-fasenya punya gate sendiri, di branch kerja, tanpa push. Setiap commit meninggalkan pohon dalam keadaan hijau. Otorisasi git mengikuti aturan pemilik.

**Arsitektur dan review.** Pekerjaannya dibagi ke tiga peran:

- **Implementasi:** agen implementasi (`sonnet`).
- **Review setiap fase:** `code-reviewer` (`opus`), dengan verdict "approved" yang menyebut berkasnya.
- **Rewrite besar (Fase 2, 4B, 5, 6):** `code-architect` (`opus`) menulis blueprint tingkat berkas **sebelum** mulai, dan `architect-reviewer` (`opus`) memberi verdict **sesudahnya**.

**Freeze.** Selama Fase 4 dan 5 berjalan, tidak ada pekerjaan fitur paralel di `SQLEditor.swift` atau `ResultGrid.swift`. Fitur yang menyentuh berkas itu mendarat sebelum baseline diambil, atau sesudah fase itu ditutup.

**Satu ADR per keputusan yang mengubah kontrak.** Daftarnya di §14.

## 1. Temuan yang mengubah rencana

1. **Runtime dibangun per panggilan.** `crates/qh-ffi/src/uniffi_api.rs:309-327` membangun runtime multi-thread di setiap `run`. `qh_rt::build_main` (worker P-core, `USER_INITIATED`, ADR-0010) hanya dipakai CLI dan MCP (`crates/qh-ffi/src/main.rs:43`, `src/bin/mcp.rs:97`). ADR-0010 belum pernah berlaku di app.
2. **Setiap halaman menjadi JSON, dan decode Swift ada di jalur kritis fetch.** Alurnya begini:
   - `emit_batches` merender setiap sel ke `Json::String` (`crates/qh-ffi/src/commands.rs:1581-1589`), lalu `SinkEmitter` men-serialize per event (`uniffi_api.rs:234-235`).
   - `Sink.onEvent` men-decode dengan `JSONDecoder` baru per baris (`Support/EngineWire.swift:13-33`, `Support/RustEngine.swift:229-234`) **sebelum** melompat ke main.
   - `block_on` menjalankan future di thread pemanggil, jadi decode Swift menahan fetch berikutnya.
   - Batch 200 dan limit 1.000 ada di `commands.rs:51,55`.
3. **Klaim ADR-0013 belum diprofil.** "JSON 99,6% dari elapsed" adalah hasil pengurangan elapsed − TTFR. Rentang itu juga memuat pipe stdout dan harness Python yang mem-`json.loads` setiap baris (`deploy/dev/bench_fetch.py:242-272`). Fase 0.1 memprofilnya sebelum angka itu dikutip lagi.
4. **Stop pada preview tidak mencapai server.** `CancelFlag` hanya `AtomicBool` (`crates/qh-ffi/src/lib.rs:242-258`). `emit_batches` memeriksanya hanya sebelum setiap fetch (`commands.rs:1554-1559`), dan batch pertama di-await tanpa pemeriksaan (`:1480`). Preview tidak pernah memanggil `session.cancel()`; hanya export dan `to_table` yang melakukannya (`:1100`, `:1278`). `close()` PostgreSQL hanya melepas koneksi (`crates/qh-driver-postgres/src/lib.rs:643-647`). Akibatnya `SELECT pg_sleep(30)` atau agregasi berat tetap berjalan di server sampai selesai atau terkena timeout app 60 detik.
5. **Satu koneksi per perintah, termasuk tunnel SSH.** Lihat `lib.rs:332-372` dan `commands.rs:638-646`. PostgreSQL membayar `prepare` lalu `simple_query_raw`, yaitu dua round trip. MySQL membayar `prep` lalu `query_iter`. Pohon objek dan autocomplete (`Models/AppModel.swift:1714-1766`) juga membuka koneksi baru untuk setiap level yang dimuat.
6. **`qh-result-store` ada, tetapi bukan kolumnar bertipe.** Crate ini tidak dipakai crate mana pun; ia hanya terdaftar di workspace `Cargo.toml`. Codec-nya menyimpan satu nilai bertag per sel di blob per kolom dengan offset `u32` (`codec.rs`). `window()` mengembalikan `Vec<Vec<Value>>` yang sudah di-decode (`store.rs:201-251`). Spill memakai `read_exact_at`, dan `unsafe_code` di-forbid. Kerangkanya layak dipakai ulang: indeks kumulatif, spill, jendela yang di-clamp, dan tesnya. Codec-nya diganti.
7. **Memori Swift tumbuh bersama hasil.** Baris ditumpuk di `[[String?]]` (`Models/AppModel.swift:2304-2336`). Setiap paint (200 ms, `:1887`) berbagi array dengan `tab.preview`, jadi append berikutnya menyalin array luar. `didSet` pada `preview` menghapus sort, menaikkan revisi, dan membersihkan undo edit **di setiap paint** (`Models/QueryTab.swift:482-495`). `displayedCache` (`:817-848`) menambah salinan saat filter atau sort aktif.
8. **Grid tidak memakai NSTableView seperti yang diputuskan ADR-0003.** Isinya `ScrollView` + `LazyVStack` + `ForEach(Array(displayedRows.enumerated()), id: \.offset)` (`Views/ResultGrid.swift:286-291`). Setiap sel membayar sendiri:
   - pembacaan `UserDefaults` lewat `ColumnFormatStore` (`:752-756`, `Models/ColumnFormat.swift:117-120`);
   - parse JSON lewat `GridValue.isOpenable` (`Models/GridValue.swift:85-97`);
   - modifier popover;
   - `.help(value)` (`ResultGrid.swift:1033`).

   `naturalWidths` memindai 200 baris pertama setiap kali diakses (`:82-96`).
9. **Sort sekarang, dan Batch 7.** Klik header menjalankan sort di Swift pada main thread (`Models/GridSort.swift:51-115`, dengan `Decimal(string:locale:)` di setiap perbandingan). Sort server sudah ada, tetapi hanya lewat menu dan banner bila hasil terpotong (`ResultGrid.swift:663-671`, `AppModel.swift:2224-2253`), dan search server ada di `AppModel.swift:2171`. Batch 7 menjadikan server sebagai default. Rencana ini tidak mengusulkan ulang sort server; ia menyejajarkan diri dengan Batch 7 (Fase 3) dan menempatkan sort in-memory sebagai fallback yang cepat (Fase 6).
10. **Editor memindai seluruh dokumen di setiap ketikan.** `SQLTextView()` dibuat tanpa memilih TextKit (`Views/SQLEditor.swift:39`). Di macOS 12+ itu berarti TextKit 2 yang jatuh ke TextKit 1 begitu `layoutManager` disentuh, dan ia memang disentuh (`:249`, `:605-620`, ruler); Fase 0.9 memverifikasinya. `allowsNonContiguousLayout` tidak dipakai di mana pun. Setiap ketikan menjalankan:
    - penyalinan string ke binding (`:349`), perbandingan string penuh di `updateNSView` (`:148`), dan `split` di `Workspace.swift:325-327`;
    - `statementRanges`, `shift`, dan `regions` (`:666-691`);
    - `setAttributes` dan regex atas seluruh dokumen (`Views/SQLSyntax.swift:79-101`);
    - reset paragraph style dan `invalidateLayout` atas seluruh dokumen (`SQLEditor.swift:920-941`);
    - hitungan karakter penuh untuk ruler (`:1164-1170`).

    Pewarnaan dan folding mati di atas 200.000 unit UTF-16 (`SQLSyntax.swift:36`, `Support/SQLFolding.swift:47`). `Models/SQLScanner.swift:5` adalah salinan state machine `crates/qh-sql/src/scan.rs`.
11. **Korpus golden lewat `run()` yang sama** (`uniffi_api.rs:58-63`). Data plane app harus menjadi mode sink tersendiri. NDJSON `rows` tetap untuk CLI, MCP, dan golden.
12. **Build.** Hanya arm64, tanpa slice universal. `[profile.release]` memakai `lto = "fat"`, `codegen-units = 1`, `panic = "unwind"`. Tidak ada global allocator dan tidak ada PGO.
13. **Alat benchmark yang dijanjikan belum ada.** `tests/bench_grid.swift` (ADR-0003) dan `tools/bench/ffi_window_bench.rs` (ADR-0008) tidak ada. Yang ada: `--snapshot --scene` (`Support/Snapshot.swift`), dan tes render yang menulis PNG lewat `QH_RENDER_DIR` (`Tests/QueryHiveTests/ResultGridTests.swift:265-302`) tetapi tidak membandingkan apa pun.
14. **Timeout sudah aman untuk sesi yang dipakai ulang.** PostgreSQL mencatat `statement_timeout` terakhir per sesi (`qh-driver-postgres/src/lib.rs:509-515`), jadi ADR-0016 cocok dengan sesi yang dipakai ulang.

## 2. Definisi "mengalahkan TablePro"

Setiap sumbu punya skenario, cara ukur, angka hari ini, target absolut, dan target relatif terhadap TablePro. Pengukuran head-to-head memakai harness black-box yang sama untuk kedua app (Fase 0.5), sehingga tidak ada angka yang bergantung pada instrumentasi internal salah satunya.

| # | Sumbu | Skenario | Cara ukur | Hari ini | Target absolut | vs TablePro |
|---|---|---|---|---|---|---|
| 1 | TTFR sampai baris pertama tergambar | S1: hangat, PG lokal, `SELECT * FROM wide_500k`, cap 1.000 dan 10.000 | `qhbench ttfr`: waktu event Run sampai frame ScreenCaptureKit 120 fps pertama yang berubah di area grid; signpost `run→firstPaint` | CLI 11 ms dari connect; app [belum diukur] | p50 ≤ 25 ms, p95 ≤ 40 ms | ≤ 0,5× |
| | | S2: cap 500.000 | sama | [belum diukur] | p95 ≤ 50 ms (progresif) | ≤ 0,1× (TablePro melukis setelah fetch selesai) |
| | | S3: RTT 30 ms lewat toxiproxy | sama | [belum diukur] | hangat ≤ 1 RTT + 20 ms | ≤ 1,0× |
| | | S4: Run pertama setelah app dibuka | sama | [belum diukur] | — | ≤ 1,0× |
| 2 | Baris/s ke grid | `wide_500k` tanpa cap; Trino `tpch.sf1.lineitem` cap 1M | Baris pertama sampai baris terakhir bisa di-scroll. Plafon hari yang sama: `psql -c "COPY (SELECT * FROM wide_500k) TO STDOUT" > /dev/null` | 191.251 baris/s (CLI) | ≥ 575.000 baris/s (3×), atau ≥ 80% plafon bila plafonnya lebih rendah | ≥ 1,5× |
| 3 | Memori puncak | 500k × 30; 5M baris (`generate_series` / lineitem) | `phys_footprint` puncak dikurangi idle, dari `proc_pid_rusage` tiap 20 ms | CLI 9 MB; app [belum diukur], estimasi 250–500 MB | ≤ anggaran store + 64 MB, **berapa pun jumlah barisnya** | ≤ 0,5× |
| 4 | Frame saat scroll | (a) 30 kol × 1M baris, fling vertikal; (b) 500 kol × 10k baris, horizontal + vertikal | `xctrace record --template 'Animation Hitches' --attach <pid>` sambil harness mengirim scroll CGEvent | [belum diukur] | hitch ≤ 1 ms/s; p99 frame ≤ 8,3 ms (120 Hz); hasil 500 kolom tergambar ≤ 30 ms | hitch ≤ 1,0× |
| 5 | Latensi ketikan | berkas 10k baris (~400k karakter) dan berkas 2M karakter, mengetik di tengah | signpost `keyDown→didChange→display`; input-to-photon lewat `qhbench type` | [belum diukur]; pewarnaan mati di atas 200k | main thread p99 ≤ 4 ms (10k baris), ≤ 8 ms (2M) dengan pewarnaan menyala | input-to-photon p95 ≤ 1,0× |
| 6 | Latensi cancel | `pg_sleep(30)`, `SLEEP(30)`, stream `wide_500k` di tengah, agregasi berat Trino | Stop sampai server mengonfirmasi (`pg_stat_activity` / processlist / state query Trino, di-poll tiap 5 ms) | pg_sleep: sampai statement selesai atau timeout | PG/MySQL p95 ≤ 100 ms; Trino ≤ 300 ms; UI berhenti ≤ 1 frame | ≤ 1,0× |
| 7 | Cold start | launch sampai frame pertama yang interaktif (editor menerima ketikan) | `xctrace --template 'App Launch'` + harness | [belum diukur] | hangat ≤ 400 ms; dingin (setelah `purge`) ≤ 1 dtk | ≤ 1,0× |

Sumbu sekunder, tanpa angka TablePro:

- **Sort dan search.** Jalur server (Batch 7) = TTFR ulang hangat. Fallback in-memory pada 500k: numerik ≤ 100 ms, teks ≤ 300 ms, off-main.
- **Baris §6 yang masih [belum diukur].** Introspeksi 5.000 tabel < 1 dtk, dan nol leak lintas FFI (100× buka/tutup tab, `leaks`).

**Aturan keadilan head-to-head.**

- Mesin, container, jendela, dan cap harus sama. Default TablePro 10k dan QueryHive 1k, jadi keduanya diukur di 1k, 10k, dan 500k.
- Kedua app dibangun Release. TablePro dibangun lokal pada commit yang dipatok, dan commit itu dicatat sebagai `competitor_rev`. Membangunnya untuk pengukuran pribadi tidak menyalin kode.
- Run dijalankan bergantian. n ≥ 10, dan ≥ 20 untuk TTFR. Yang dilaporkan median dan p95. Beban mesin dan `hw.model` dicatat.
- Resolusi black-box adalah satu frame (8,3 ms), dan signpost memberi angka presisi untuk QueryHive.

## 3. Ringkasan fase

| Fase | Isi | Ukuran | Risiko | Bergantung pada | ADR |
|---|---|---|---|---|---|
| 0 | Pengukuran, harness head-to-head, baseline visual | M | rendah | P | — |
| 1 | Runtime persisten, cancel preemptif + server-side, QoS, clamp | S | rendah | 0 | 0031 (bagian 1) |
| 2 | Sesi persisten dan TTFR | M | sedang | 1 | 0031 |
| 3 | Batch 7: sort dan search server-first | S–M | sedang | 2 (untuk kecepatan; secara fungsi tidak wajib) | — (keputusannya sudah di Batch 7) |
| 4 | Editor: 4A jalur panas Swift, 4B analisis di Rust | S–M + M | sedang | 0, P | 0033 |
| 5 | Grid NSTableView dengan sel yang digambar | L | tinggi | 0, P, 3 | 0032 (amandemen 0003) |
| 6 | Data plane: result store, jendela, fallback in-memory di Rust | L | tinggi | 1, 2, 5 | 0030, 0034 |
| 7 | Plafon ingest dan tingkat build | M | sedang | 6 | 0035, 0036 (bersyarat) |
| 8 | Eskalasi, hanya bila gate gagal | — | — | gate gagal | per kasus |

Fase 4 tidak bergantung pada 1–3 dan boleh dikerjakan kapan pun setelah Fase 0. Fase 3 harus mendarat sebelum Fase 5, supaya grid baru memindahkan semantik sort yang final.

## 4. Fase 0: pengukuran dan baseline (M)

**Tujuan.** Setiap sumbu di §2 punya angka QueryHive dan, bila harness bisa mencapainya, angka TablePro. Baseline visual direkam dari commit P. Semua ini ada sebelum satu baris optimasi pun ditulis.

**0.1 Memprofil klaim ADR-0013.** `samply record` (tool, tidak dikirim) dijalankan pada `queryhive-engine preview` dengan stdout ke `/dev/null`, lalu lewat harness. Rinciannya dicatat: decode driver, `to_text`, `serde_json`, sink, pipe, dan pembaca. Hasilnya menjadi dasar ADR-0030.

**0.2 `crates/qh-ffi/examples/bench_ffi.rs`.** In-process lewat `uniffi_api::run` dengan sink penghitung. Skenario:

- `local-loop`: 100× `connections` pada DB sementara, untuk ongkos per panggilan termasuk runtime;
- `preview-wide`: PG hidup, waktu di sink dibanding total;
- `emit-only`: batch sintetis tanpa DB.

Ditaruh sebagai `examples/`, bukan crate baru. Di Fase 6 ia diperluas dengan `window`, dan dengan itu ia menjadi `ffi_window_bench` yang dijanjikan ADR-0008.

**0.3 Microbench Swift.** `app/Tests/QueryHiveTests/Bench/`, digerbangi `QH_BENCH=1` dan dijalankan dengan `swift test -c release --filter Bench`. Yang diukur:

- decode `EngineWire` per halaman 200 baris;
- tumpuk + paint pada 500k;
- `displayedRows` (filter + sort) pada 500k;
- `SQLSyntax.apply` pada berkas 10k baris;
- satu ketikan lewat `Coordinator` pada `SQLTextView` headless, dengan pola dari `EditorFindAndFoldingTests`.

**0.4 Signpost dan mode `--bench`.**

- `Support/PerfSignposts.swift` (baru, `OSSignposter` kategori `perf`) mencatat interval `run→firstRowsEvent→firstPaint`, `keystroke`, `cancel`, dan `launch→firstFrame`.
- `Support/BenchMode.swift` menambah `--bench <skenario>` di samping `--snapshot`: `ArrayRows` sintetis 1M × 30 dan 500 × 10k, fixture editor 10k baris dan 2M karakter, serta scroll terprogram dengan kecepatan tetap. Hasilnya JSON di stdout. Grid dan editor jadi bisa diukur tanpa database.

**0.5 Harness black-box `tools/bench/qhbench/`.** Paket Swift terpisah yang tidak ditautkan ke app. Subperintahnya:

- `ttfr` dan `type`: ScreenCaptureKit 120 fps;
- `memory`: `proc_pid_rusage`;
- `scroll`: CGEvent scroll + `xctrace` Animation Hitches;
- `cancel`: polling `pg_stat_activity` / processlist / REST Trino;
- `launch`: `xctrace` App Launch.

App dikendalikan lewat AX + CGEvent, dengan profil per app (pintasan Run dan Stop, lokasi editor) di `profiles/*.json`. Harness butuh izin Accessibility dan Screen Recording, diberikan sekali oleh pemilik.

**0.6 `deploy/dev/bench_app.py`.** Menjalankan ketiga sumber di atas dan menambah rekaman ke `bench-results.jsonl` dengan kunci `bench` (sumbu), `app`, `competitor_rev`, dan `hw_model`. Rekaman lama tanpa `bench` dibaca sebagai `fetch`. `write_report` di `bench_fetch.py` diperluas: satu tabel per sumbu, dan baris "[belum diukur]" di "Perbandingan dengan target §6" terisi.

**0.7 Fixture.**

- 1M baris dan 500 kolom tidak butuh seed baru: `generate_series` dengan SQL yang dibangun harness.
- Trino: `tpch.sf1.lineitem`. Katalog `tpch` sudah dipakai di §4 adoption plan, tetapi Fase 0 memverifikasi keberadaannya.
- toxiproxy (MIT) sebagai container dev untuk RTT (127.0.0.1:55435; 55433 terpakai di host ini dan 55434 milik `qh-pg-old.sh`).
- Skrip 5.000 tabel di `deploy/dev/`.

**0.8 Gate paritas visual.** `app/Tests/QueryHiveTests/VisualParityTests.swift` dengan baseline PNG di `__Baselines__/`, direkam dari commit P lewat `QH_RECORD_BASELINES=1`. Merekam ulang butuh persetujuan pemilik.

Matriks scene:

- **Grid:** gelap/terang; alternate rows; row numbers; seleksi blok; titik staged; NULL, kosong, panjang, JSON, dan numerik; header dengan chevron, funnel, dan tipe; footer; inspector; banner sort; placeholder.
- **Editor:** gelap/terang; lipatan terbuka dan terlipat; tombol run; band current statement; find bar dengan match; invisibles; wrap; dokumen panjang yang di-scroll ke tengah.
- **`--snapshot` yang sudah ada** (`grid-sorted`, `grid-json`) ikut.

Kriterianya tiga lapis:

1. Geometri **identik**, diperiksa secara numerik: tinggi baris, x kolom, padding, tinggi header, lebar gutter.
2. Warna identik di titik sampel: latar sel, seleksi, stripe, dan staged.
3. Teks dalam toleransi: ≤ 0,1% piksel dengan delta kanal > 16/255 per scene. Ditambah tinjauan berdampingan oleh pemilik.

Kondisi bit-identik tidak bisa dijanjikan ketika renderer berganti dari SwiftUI `Text` ke `CTLineDraw`, dan itu dikatakan di sini alih-alih ditemukan belakangan.

Tooltip dan popover adalah jendela terpisah yang tidak bisa ditangkap `cacheDisplay`. Paritasnya diuji lewat perilaku (isi tooltip = nilai penuh; double-click membuka reader) dan render konten popover secara terpisah, seperti yang sudah dilakukan untuk `CellValueViewer`.

**0.9 Verifikasi dua asumsi.** Pertama, mode TextKit editor saat runtime (log fallback). Kedua, CPU default `aarch64-apple-darwin`, dengan membandingkan `rustc --print cfg --target aarch64-apple-darwin` terhadap `-C target-cpu=apple-m1`.

**Gate.**

- Setiap sumbu punya angka QueryHive, atau alasan tertulis kenapa belum.
- Angka TablePro ada untuk sumbu 1–7, atau alasan tertulis.
- Baseline visual terekam dari P.
- Profil 0.1 tercatat.
- Laporan diregenerasi.

**Risiko.** Harness black-box bisa rapuh; profil per app dan n besar meredamnya. Izin OS harus diberikan pemilik. Build TablePro (Xcode, unsigned lokal) bisa gagal; bila begitu, sumbu yang terdampak ditulis "TablePro [belum diukur]" dengan alasannya dan target absolut tetap berlaku.

## 5. Fase 1: higiene engine (S)

**Tujuan.** Membuang ongkos per panggilan dan membuat Stop benar-benar menghentikan query, tanpa mengubah protokol.

1. **Runtime persisten.** `OnceLock<Runtime>` dibangun dengan `qh_rt::build_main()` di `crates/qh-ffi`, dan `run()` memanggil `block_on` padanya. `block_on` aman dipanggil bersamaan dari beberapa thread yang bukan worker. Builder per panggilan di `uniffi_api.rs:309-327` dihapus. Komentar di situ dan di `RustEngine.swift:133-137` diperbarui. Dengan ini ADR-0010 akhirnya berlaku di app.
2. **Cancel preemptif dan server-side.** `CancelFlag` mendapat `tokio::sync::Notify` di samping `AtomicBool`. Ini aman karena sinyal CLI datang lewat `tokio::signal` (`main.rs:122-148`), bukan handler mentah.
   - `stream_rows`, `emit_batches`, dan `explain` membungkus **setiap** await dengan `select!` terhadap `cancelled()`, termasuk batch pertama.
   - Saat cancel, panggil `session.cancel()` dengan batas 250 ms, lalu tutup sesi.
   - Bentuk event tidak berubah (`done` dengan `cancelled: true`), jadi kasus `cancel_*` di `crates/qh-ffi/tests/golden.rs` tetap lulus.
3. **QoS di sisi Swift.** Ganti `DispatchQueue.global()` (`RustEngine.swift:132`) dengan antrean konkuren ber-QoS `.userInitiated`.
4. **Clamp `rowLimit`.** Sekarang tidak ada batas atas (`AppModel.swift:2832`). Dipasang 100.000 sampai Fase 6, dinaikkan ke 5.000.000 sesudahnya. Batas ini terlihat oleh pengguna (§13).

**Gate.**

- `bench_ffi local-loop` p50 ≤ 2 ms per panggilan.
- Sumbu 6: PG/MySQL p95 ≤ 100 ms pada `pg_sleep(30)`/`SLEEP(30)`; Trino ≤ 300 ms.
- `real_server.rs` dengan `QH_TEST_POSTGRES=1`.
- Gate standar.

**Risiko.** Runtime tidak pernah di-shutdown. Itu disengaja karena umurnya sama dengan proses, dan ditulis di doc modul. Panic dalam task menjadi `JoinError` yang dipetakan ke event `error`.

## 6. Fase 2: sesi persisten dan TTFR (M)

Blueprint `code-architect` dibuat lebih dulu. **Tujuan.** Setiap Run, level pohon objek, dan perintah lokal berhenti membayar connect dan round trip yang tidak perlu. Sumbu 1 (S1, S3, S4) dan introspeksi.

1. **`EngineHost`** (objek UniFFI, `crates/qh-ffi/src/host.rs`). Ia memiliki runtime dari Fase 1, `SessionPool`, dan satu handle SQLite lokal: dibuka sekali dan dimigrasi sekali, lalu dipakai `history`, `saved_queries`, `session`, `account`, dan `profiles`. Di Fase 6 ia juga memegang registry store. `RustEngine` memanggil `host.run(...)`. Fungsi bebas `run()` tetap ada untuk CLI dan tes.
2. **Pool.**
   - Kunci: hash konfigurasi koneksi (kind, host, port, user, database, TLS, tunnel, versi kredensial). Setelan per-run (SQL, LIMIT, timeout, Safe Mode) tidak termasuk.
   - Maksimum 4 sesi per kunci, idle 5 menit.
   - Tidak ada ping saat checkout. Galat transport pada statement baca dijawab dengan satu reconnect lewat `RetryPolicy` yang ada; write tidak di-retry, sesuai aturan `retry.rs`.
3. **Semantik reset (rekomendasi; pertanyaan terbuka 1).** Setiap Run tetap mulai dari sesi bersih, seperti hari ini.
   - PostgreSQL: `ROLLBACK` bila status `ReadyForQuery` bukan idle, lalu `DISCARD ALL`, keduanya saat checkin, di luar jalur kritis.
   - MySQL: `COM_RESET_CONNECTION` saat checkin.
   - Trino tidak punya state koneksi. Yang dipakai ulang adalah `reqwest::Client` (keep-alive dan TLS).
   - Timeout per run sudah dilacak di PostgreSQL. Untuk MySQL (`max_execution_time`), Fase 2 memverifikasinya.
4. **Preview yang terpotong pada sesi pool.**
   - PostgreSQL: sesi dibuang, sama seperti hari ini, dan pool mengisi ulang di latar.
   - MySQL: `SQL_SELECT_LIMIT = cap+1` per run. Server berhenti di cap dan sesi tetap bisa dipakai; nilainya di-reset sesudahnya. Idenya dilihat di TablePro, kodenya ditulis sendiri.
   - Trino: pastikan `close` mengirim `DELETE` ke `nextUri`.
5. **Tunnel dibagi per kunci pool** (`Arc<Tunnel>`), ditutup setelah sesi terakhir kunci itu idle.
6. **Warm-up.** Saat sebuah koneksi dipilih atau dibuka di sidebar, host membuka satu sesi di latar: hanya connect, tanpa query. Prefetch data spekulatif ditolak (§12).
7. **Hemat round trip.**
   - PostgreSQL: `prepare` dan `simple_query_raw` dikirim bersamaan. tokio-postgres mem-pipeline request pada satu koneksi, jadi satu RTT hilang tanpa fork.
   - MySQL: `prep` dilepas bila definisi kolom result set teks sudah membawa tipe yang sama. Paritasnya lewat golden `type_zoo`.

**Gate.**

- TTFR S1 p50 ≤ 25 ms; S3 hangat ≤ 1 RTT + 20 ms; buka level pohon hangat ≤ 1 RTT + 10 ms; cold start tidak mundur.
- Tes kebenaran:
  - `SET search_path` di run A tidak terlihat di run B;
  - transaksi yang gagal dipulihkan;
  - perubahan timeout di antara run dihormati;
  - Safe Mode per run;
  - tunnel dipakai ulang.
- Golden tidak berubah. Verdict `architect-reviewer`. Gate standar.

**Risiko.**

- Koneksi basi karena idle NAT: galat pada pemakaian pertama, dan untuk baca dijawab retry.
- Batas koneksi server: maksimum 4 per kunci.
- Kebocoran pool saat tab ditutup.
- Rotasi kredensial: versi kredensial masuk kunci.

## 7. Fase 3: Batch 7, sort dan search server-first (S–M)

**Tujuan.** Mendaratkan keputusan pemilik di `remaining-work-plan.md` Batch 7 dengan mekanisme yang sudah ada: `ServerSort.order(sql:)`, `SearchStatement.crossColumn(sql:)`, dan jalur `preview`. Fase 2 membuat setiap sort atau search ke server semurah TTFR hangat, dan itulah alasan fase ini datang sesudahnya.

**Jawaban atas tiga pertanyaan Batch 7 (rekomendasi):**

1. **Arti "off".** Sebelum Fase 6, hasil dasar disimpan bila ≤ 10.000 baris, dan "off" mengembalikannya tanpa query. Bila lebih besar, SQL dasar dijalankan ulang, karena menahan dua `[[String?]]` besar menggandakan memori. Sesudah Fase 6, hasil dasar selalu disimpan sebagai handle store yang bisa spill (maksimal dua store per tab: dasar dan aktif), jadi "off" tidak pernah menjadi query ketiga. SQL dasar hanya dijalankan ulang bila store dasar sudah dilepas.
2. **Sumber indikator header.** Hanya satu: `tab.activeSort: ActiveSort?` berisi `{column, direction, origin: .server | .memory}`. `gridSort` dan `serverSort` menjadi turunan lalu dihapus. Menerapkan satu origin menghapus yang lain, jadi dua indikator tidak bisa berselisih. Gaya chevron tidak berubah.
3. **Kapan fallback, dan apa yang dikatakan.** In-memory dipakai hanya bila:
   - `ServerSort` atau `SearchStatement` menolak (multi-statement, atau tidak ada kolom teks);
   - hasil yang tampil adalah plan (`showingPlan`);
   - hasil yang tampil adalah preview inspector objek.

   Bila server **gagal**, galatnya ditampilkan tanpa fallback diam-diam, sesuai larangan fallback senyap proyek ini. Pesannya:
   - in-memory atas hasil terpotong: satu baris tipis, "urutan parsial atas N baris yang sudah diambil";
   - in-memory atas hasil lengkap: tanpa banner;
   - server: tanpa banner.

**Search.** Debounce 250 ms, sama dengan panel History, dan panjang minimum 3 karakter. Di bawah minimum, search dikosongkan.

**Penjaga edit.** Sort atau search ke server menjalankan ulang query dan akan membuang edit yang di-staged. Bila `cellEdits` tidak kosong, aksinya ditolak dengan pesan "simpan atau buang perubahan dulu".

**Gate.**

- Tes perilaku:
  - siklus kedua arah klik pertama;
  - sumber indikator tunggal;
  - ketiga kasus fallback;
  - debounce dan panjang minimum;
  - penjaga edit.
- Baseline visual direkam ulang dengan persetujuan pemilik, karena banner berubah.
- Sumbu sort sekunder tercatat. Gate standar.

**Catatan.** Pintasan "hasil lengkap disortir in-memory tanpa query ulang" **tidak** dinyalakan secara default (pertanyaan terbuka 2). Di hasil lengkap urutan in-memory adalah jawaban utuh, tetapi kolasinya bisa berbeda dari server, dan klik yang sama bisa memberi urutan berbeda tergantung apakah hasilnya terpotong.

## 8. Fase 4: editor (4A: S–M, 4B: M)

Prasyarat P (`SQLEditor.swift` dan `HighlightBandTests.swift` ada di pekerjaan yang belum di-commit). Blueprint `code-architect` dibuat sebelum 4B.

**4A: jalur panas Swift, kebanyakan penghapusan.**

1. **Tumpukan TextKit 1 eksplisit.** `NSTextView(frame:textContainer:)` dengan `NSLayoutManager` sendiri, atau `usingTextLayoutManager: false`, plus `allowsNonContiguousLayout = true`. Mode editor tidak lagi bergantung pada kapan `layoutManager` pertama disentuh.
2. **Folding lewat delegate layout manager.** Glyph yang dilipat dijadikan null, dan line fragment-nya bertinggi nol. Lipat dan buka hanya meng-invalidate rentangnya sendiri. Reset paragraph style dan `invalidateLayout` atas seluruh dokumen di `SQLFoldStyler` (`:920-941`) dihapus.
3. **Atribut hanya di rentang kotor dan rentang terlihat.** Tidak ada lagi `setAttributes` atas seluruh dokumen. Sementara 4B belum ada, rentang kotor adalah statement yang diedit, diambil dari batas statement yang di-cache.
4. **Sinkronisasi model di-debounce.** `parent.text` ditulis paling cepat tiap 150 ms, dan dipaksa sebelum Run, Save, pindah tab, dan simpan sesi. `updateNSView` membandingkan revisi editor, bukan string penuh (`:148`). `lineCount` di `Workspace.swift:325-327` membaca indeks baris ruler.
5. **Indeks baris ruler inkremental** dari `textStorage(_:didProcessEditing:range:changeInLength:)`, menggantikan hitungan karakter penuh (`:1164-1170`) dan `lineStarts` untuk run marks (`:266`).
6. `statementRanges`, region lipatan, dan run marks dihitung off-main setelah debounce, sampai 4B menggantinya.

**4B: analisis editor di Rust (ADR-0033).**

- **Modul baru `crates/qh-sql/src/editor.rs`.**
  - Dokumen inkremental: UTF-8 dengan indeks baris UTF-8 ↔ UTF-16.
  - Lexer: port aturan `SQLSyntax` apa adanya. Enam kelas token; daftar kata kunci yang sama; deteksi fungsi dengan lookahead hanya melewati spasi. Aturan EOF untuk kutip atau komentar yang tidak tertutup meniru perilaku regex hari ini: pasangkan dengan penutup berikutnya, dan bila tidak ada, karakter itu dilewati.
  - Checkpoint state per N baris, lalu lex ulang dari checkpoint sampai state konvergen.
  - Batas statement dari `scan.rs` yang sudah dipakai engine.
  - Region lipatan: port `SQLFolding.regions`.
- **Permukaan FFI.** Objek UniFFI `EditorDocument` dengan `replace(start_utf16, len_utf16, text) -> revision` dan `analysis(revision, visible) -> bytes` (span dalam UTF-16, statement, lipatan). Rentang terlihat dikerjakan di pool `USER_INITIATED`, sisanya di `UTILITY` (ADR-0010). Swift menerapkan hasilnya di main bila revisinya masih terkini, Penjaga IME ada di sisi penerapan, bukan di sisi kirim: edit tetap dikirim selama `hasMarkedText`, dan `apply` serta penjadwal idle menunggu komposisi selesai (ADR-0033, D-9).
- **Yang dihapus.** Pemindaian regex di `SQLSyntax` (palet tetap), bagian statement di `SQLScanner.swift` (duplikat `scan.rs`), dan pemindaian di `SQLFolding`.
- **Batas naik** dari 200.000 ke 2.000.000 karakter, sama dengan plafon TablePro. Rentang terlihat diwarnai lebih dulu, sisanya bertahap di latar.

**Gate.**

- Sumbu 5 terpenuhi.
- Paritas lexer 100%: span Rust == span `SQLSyntax.attributes`, dari tes Swift yang memanggil FFI atas korpus nyata, korpus sintetis, dan proptest di Rust. Paritas ini harus lulus **sebelum** regex Swift dihapus.
- Tes yang ada tetap hijau: `EditorFindAndFoldingTests`, `HighlightBandTests`, run button, auto-uppercase, `:name`.
- Paritas visual scene editor. Verdict `architect-reviewer` untuk 4B. Gate standar.

**Risiko.** Warna tertinggal satu frame. Bug pemetaan UTF-16 (surrogate pair, CRLF) ditutup dengan proptest. Undo dan IME. Highlight find bar tetap memakai temporary attribute, jadi tidak tersentuh.

## 9. Fase 5: grid NSTableView dengan sel yang digambar (L)

Prasyarat P dan Fase 3. Blueprint `code-architect` dibuat lebih dulu. Pendekatan TablePro dipelajari sebagai ide saja. **Tujuan:** sumbu 4, serta bagian paint dari sumbu 1.

1. **Seam `ResultRows`** (protokol Swift): `count`, `columns`, `cell(row, col) -> CellText` (prefix tampilan + flag null/kosong/openable/numerik/terpotong), `fullValue`, dan `rows(in:)` untuk copy dan `WritePlan`.
   - `ArrayRows` membungkus `[[String?]]` dan `QueryTab.displayedRows` yang ada.
   - 28 pemakai `preview.rows`/`displayedRows` di `ResultGrid.swift`, `QueryTab.swift`, dan `Snapshot.swift` dipindah ke seam ini.
   - `WritePlan.build` menerima accessor, bukan array penuh.
2. **`ResultGridTable: NSViewRepresentable`** membungkus `NSScrollView` + subclass `NSTableView`.
   - Tinggi baris tetap (setelan Data) dengan `usesAutomaticRowHeights = false`, `intercellSpacing = .zero`, dan `gridStyleMask = []`. Garis vertikal bawaan AppKit membuat subview per kolom; TablePro mengukurnya kuadratik, dan kita ukur ulang sendiri.
   - `allowsColumnReordering = false`, karena memindah kolom tetap lewat menu (paritas).
   - `viewFor` mengembalikan `nil` untuk kolom data. `GridRowView` menggambar stripe, seleksi, tint dan titik staged, garis pemisah, gaya NULL/∅, dan teks lewat `CTLine`, hanya untuk kolom yang memotong rect kotor (`columnIndexes(in:)`).
   - Gutter nomor baris digambar di row view dan ikut tergeser horizontal seperti hari ini (tidak di-pin), demi paritas.
3. **Header.** `NSTableHeaderView` kustom menggambar label, chevron dari `activeSort`, dan funnel. Funnel membuka `NSPopover` yang menampung konten filter SwiftUI yang ada (`NSHostingController`). Menu konteks header adalah `NSMenu` dengan item yang sama, dan teks tooltip-nya sama.
4. **Interaksi.**
   - Drag seleksi, yang hanya menggambar ulang baris yang berubah.
   - ⌘C, menu, dan tombol footer lewat `GridClipboard` dengan accessor. Teks dibangun off-main bila lebih dari 10k sel.
   - Edit lewat `NSTextField` overlay (mono12, plain; Return menyimpan, Esc membatalkan). `typeCellEdit` tetap, jadi penggabungan undo sama.
   - Sel openable membuka `NSPopover` berisi `CellValueViewer` di tepi bawah sel. Inspector dan auto-show tetap ada.
   - Tooltip lewat `addToolTip(rect:owner:userData:)` untuk sel yang terlihat, dengan nilai penuh yang dibaca lazy.
5. **Cache terbatas.**
   - Cache tampilan per baris: viewport ± 2 halaman, maksimal 16 MB.
   - Cache `CTLine` dengan batas yang ditetapkan dari angka Fase 0.
   - `CTLine` untuk jendela prefetch dibangun off-main (`.userInitiated`) dengan `CGColor` dan `CTFont` yang sudah di-resolve untuk appearance saat ini, dan dibangun ulang saat appearance atau font berubah.
   - Format kolom dibaca **sekali per kolom**, dan di-invalidate lewat notifikasi perubahan.
6. **Lebar kolom.** Rumusnya tidak berubah: hitungan × 7,2 + 20, di-clamp 84–320, + 22, dengan pembagian slack yang sama. Bedanya, ia dihitung sekali per hasil atau per perubahan layout kolom, bukan per `body`.
7. **Aksesibilitas (ADR-0003 §7.7).** Sel yang digambar tidak terlihat VoiceOver. Jawabannya view aksesibilitas ringan yang tidak menggambar, dipasang hanya untuk baris terlihat dan hanya setelah aksesibilitas aktif. Implementasinya ditulis sendiri.
8. **Dihapus:** body SwiftUI grid (`LazyVStack`, `rowView`, `cellView`) dan modifier per sel.

**Daftar paritas fitur**, diuji satu per satu:

- seleksi blok dan copy dalam semua formatnya;
- edit, titik staged, undo per sesi edit;
- sembunyikan, pindah, ganti nama, dan reset kolom;
- sort in-memory dan server (Batch 7), filter per kolom, preset, search lintas kolom dan eskalasinya;
- reader JSON/Tree/Hex dan inspector;
- setelan Batch 6: tinggi baris, tampilan NULL, alternate rows, nomor baris, arah klik pertama, default view JSON;
- format kolom (Raw/Text/UUID/Unix/JSON);
- placeholder, footer, dan kontrol limit;
- scene snapshot.

**Gate.**

- Sumbu 4 dengan `--bench` sintetis (mengisolasi grid) dan head-to-head.
- Hasil 500 kolom tergambar ≤ 30 ms. TTFR S1 tidak mundur.
- Paritas visual scene grid. Semua daftar paritas fitur hijau, termasuk `ResultGridTests` dan `GridColumnsTests` yang ada.
- Smoke test VoiceOver. Verdict `architect-reviewer`. Gate standar.

**Risiko.**

- Paritas piksel ketika renderer berganti: geometri identik, teks dalam toleransi.
- Fokus dan undo field editor. Anchor popover. Campuran header AppKit dan konten SwiftUI.
- Aksesibilitas.

## 10. Fase 6: data plane lewat result store (L)

Blueprint `code-architect` dibuat lebih dulu. **Tujuan:** sumbu 1 S2, sumbu 2 (interim), dan sumbu 3. Sort, filter, dan search in-memory berpindah ke Rust sebagai fallback Batch 7.

1. **Codec baru di `qh-result-store`** (ADR-0008 diamandemen). API store, spill, indeks kumulatif, dan tesnya dipertahankan.
   - **Kolom bertipe per chunk**, satu chunk sampai 64k baris hasil penggabungan batch driver.
     - Fixed-width: i64/u64/f64/i128+scale/date/time/timestamp+flag offset, dengan bitmap validitas.
     - Teks/JSON/bytes: offset `u32` + byte.
     - Kolom komposit atau tak dikenal tetap memakai encoding bertag yang sekarang.
   - **Flag openable per sel** di bitmap. Dihitung saat ingest hanya untuk sel yang karakter non-spasi pertamanya `{` atau `[`, dengan parse ≤ `parseLimit`. Semantiknya sama dengan `isOpenable` hari ini, karena parse tetap dilakukan.
   - **Jendela merender teks tampilan** lewat `qh_core::render::to_text`, renderer yang satu itu. Renderer `ColumnFormat` (Raw/Text/UUID/Unix/JSON) di-port ke Rust dengan tes paritas. Teksnya dipotong ke 256 unit UTF-16 untuk layout, dan nilai penuh dibaca sesuai permintaan.
   - **Spill tetap `pread`**, dengan LRU 8 chunk yang sudah di-decode.
2. **Engine.** App menjalankan `preview` dengan `RESULT_SINK=store` (hanya app yang memasangnya). Baris masuk store yang terdaftar di `EngineHost`. Event yang dikirim hanya `columns`, `progress {rows}` (≤ 1 per 16 ms), dan `done`. CLI, MCP, dan golden tidak berubah.
3. **FFI.** Objek UniFFI `ResultHandle`, sebuah `Arc`. Saat di-drop, store dilepas dan spill-nya dihapus. Handle memakai generation, dan handle basi menjadi galat `StaleHandle`. Metodenya:
   - `row_count()`: load atomik;
   - `window(view, rows, cols, formats) -> bytes`: satu buffer terkemas;
   - `cell_text(...)`;
   - `column_widths()`: hitungan grapheme atas 200 baris pertama lewat `unicode-segmentation`, supaya sama dengan `String.count` di Swift;
   - `set_view(sort, filters, search) -> ViewInfo`.

   Semua metode **throwing**, sehingga panic menjadi galat Swift dan bukan crash (ADR-0009).
4. **View in-memory di Rust (ADR-0034), dengan peran sesuai Batch 7:** fallback, plus pintasan hasil lengkap bila pemilik memilihnya.
   - Pool rayon: jumlah worker = P-core, QoS `USER_INITIATED` lewat start handler `qh-rt`.
   - Permutasi `Vec<u32>`.
   - Semantik di-port dari `GridSort` (NULL terakhir saat ascending, numerik bila kedua sel angka murni, tie stabil), `ColumnFilter`, dan `GridSearch`.
   - Kolom bertipe diurutkan menurut nilainya. Teks memakai kunci natural yang mendekati `localizedStandardCompare`.
   - Tes diferensial terhadap implementasi Swift atas korpus ASCII, Latin beraksen, nama Indonesia, CJK, emoji, dan campuran digit. Setiap divergensi dicatat.
   - Implementasi Swift dihapus hanya setelah paritasnya lulus.
5. **Swift.**
   - `StoreRows: ResultRows`.
   - Selama streaming, grid mem-poll `row_count` per frame (`NSView.displayLink`, macOS 14) lalu `noteNumberOfRowsChanged`.
   - Jendela diambil sinkron saat cache miss (p99 ≤ 0,5 ms), dan prefetch di latar mengikuti arah scroll.
   - Dihapus: penumpukan baris dan loop `previewPaintInterval` di `AppModel`, `displayedCache`, `GridSort.order` dan jalur filter di hot path, `naturalWidths` per baris, dan `isOpenable` per render.
   - Explain dan inspector objek tetap `ArrayRows` lewat NDJSON, karena hasilnya kecil.
6. **"Off" di Batch 7 memakai store dasar yang disimpan** (jawaban 1, Fase 3).
7. **Batas dan anggaran.** Clamp `rowLimit` dinaikkan ke 5.000.000, sama dengan plafon TablePro, dan memori dibatasi oleh spill. Anggaran store default 256 MB (`StoreConfig`). Spill di `~/Library/Caches/QueryHive/spill`. Sapuan berkas spill yatim saat startup diverifikasi ada, dan dibangun bila belum.
8. **Sort header dinonaktifkan selama streaming.** Hari ini setiap paint sudah menghapus sort, jadi ini membuat aturan itu terlihat. Dicatat di §13.

**Gate.**

- TTFR S2 p95 ≤ 50 ms.
- Baris/s ≥ 380.000 (2× hari ini; target penuh di Fase 7).
- Memori ≤ anggaran + 64 MB pada 500k dan 5M baris.
- `window` p99 ≤ 0,5 ms; bila gagal, eskalasi C ABI di Fase 8.
- Angka fallback in-memory tercatat.
- 100× buka/tutup tab: `leaks` nol dan tidak ada spill yang tersisa.
- Paritas visual terhadap baseline Fase 5. Tes diferensial sort dan filter.
- Golden tidak berubah. Verdict `architect-reviewer`. Gate standar.

**Risiko.**

- Kontensi pembaca (main) dengan penulis (ingest): chunk append-only, dan kunci hanya dipegang untuk publikasi.
- Handle basi setelah tab ditutup.
- Disk penuh saat spill: menjadi event galat, bukan crash.
- Akuntansi memori.

## 11. Fase 7: plafon ingest dan tingkat build (M)

Setiap item diuji A/B dan hanya dipertahankan bila memberi ≥ 10% pada sumbunya tanpa regresi di sumbu lain. Satu commit per item.

1. **Builder tanpa alokasi.** `qh_core::ColumnarBuilder` menulis langsung ke layout chunk store. `Cursor::next_into(&mut builder, max_rows)` punya implementasi default lewat `next_batch`, jadi penulis ekspor tidak tersentuh.
   - PostgreSQL mem-parse teks langsung ke builder, dan `Vec<Vec<Value>>` + transpose (`qh-driver-postgres/src/lib.rs:703-748`) dihapus.
   - MySQL serupa.
   - Trino: `DeserializeSeed` streaming untuk `data`, tanpa pohon `serde_json::Value`.
2. **PostgreSQL `COPY (<sql>) TO STDOUT` format teks**, lewat `copy_out`. Untuk ekspor dan untuk ingest store tanpa cap atau dengan cap besar, hanya untuk satu `SELECT` (diperiksa `qh-sql`). Parsing memakai `memchr`. Teksnya berasal dari fungsi output server yang sama, jadi paritasnya lewat golden. Dipakai bila ≥ 1,3× pada `wide_500k` atau pada tabel bertipe.
3. **Hasil biner, bersyarat.** Hanya bila **semua** OID kolom ada di himpunan decoder (diketahui setelah Describe), lewat extended protocol biner. Teks tampilan harus sama dengan hari ini: float shortest, skala numeric, offset timestamptz. Gate-nya golden `type_zoo` live. Dipakai hanya bila profil tabel bertipe menunjukkan output teks server dominan. Ini akan mengamandemen keputusan 7 ADR-0028.
4. **Protokol spooling Trino** (Trino 483 di dev) dan JSON SIMD (`sonic-rs`) hanya dievaluasi bila setelah 7.1 parsing JSON Trino masih > 50% profil.
5. **Tingkat build.**
   - **mimalloc** (`#[global_allocator]` di `qh-ffi`, hanya alokasi Rust): dipertahankan bila ≥ 10% throughput dengan kenaikan `phys_footprint` ≤ 5%. mimalloc bisa menahan halaman yang sudah dibebaskan, jadi sumbu 3 ikut diukur.
   - **PGO** lewat `cargo-pgo` + `tools/pgo.sh`, dilatih pada `bench_ffi` dan `bench_fetch`: dipertahankan bila ≥ 10% dan build-nya reproducible.
   - **`target-cpu`: ditolak.** Default `aarch64-apple-darwin` sudah `apple-m1` (diverifikasi di Fase 0.9), dan CPU yang lebih tinggi akan memutus pengguna M1.

**Gate.**

- Sumbu 2 penuh: ≥ 575.000 baris/s atau ≥ 80% plafon COPY, dan ≥ 1,5× TablePro. Trino lineitem ≥ 1,5× TablePro.
- Golden dan `type_zoo` live identik.
- Gate standar.

## 12. Fase 8: eskalasi, hanya bila gate gagal

- **C ABI untuk jalur `window`** bila p99 UniFFI > 0,5 ms: `extern "C"` tulisan tangan, `catch_unwind` per panggilan (ADR-0009), dan buffer dari pemanggil. Ongkosnya permukaan `unsafe`, firewall panic manual, dan mekanisme binding kedua.
- **TextKit 2, atau CodeEditTextView/CodeEditSourceEditor upstream** (lisensi diverifikasi dari repo CodeEditApp) bila Fase 4 gagal di 2M karakter. Ongkosnya menulis ulang find, fold, ruler, run marks, completion, auto-uppercase, dan jalur `interceptKey` yang dibutuhkan Vim.
- **Grid Metal** bila Fase 5 gagal di 120 Hz pada 500 kolom. Aksesibilitas, IME, kualitas teks, dan seleksi semuanya ditulis sendiri.
- **Cap PostgreSQL berbasis portal** (`DECLARE CURSOR` atau `Execute max_rows`) bila Run ulang setelah preview terpotong terasa membayar reconnect.

## 13. Evaluasi opsi yang diminta, satu rekomendasi per opsi

| Opsi | Putusan | Di mana | Alasan |
|---|---|---|---|
| Sort/filter/search in-memory di Rust dengan **rayon** | **Ya** | Fase 6 | Kolom bertipe, off-main, permutasi murah. Perannya fallback dan pelengkap Batch 7, bukan default. |
| **DataFusion** | Tidak | — | Menyeret arrow dan ratusan crate, butuh konversi ke `RecordBatch`, kolasinya byte-order (tidak cocok dengan `localizedStandardCompare`), dan SQL atas hasil bukan fitur yang diminta. |
| Group-by atas hasil | Tidak | — | Bukan fitur yang ada. View permutasi Fase 6 adalah fondasinya bila suatu hari diminta. |
| Data tampilan dihitung di Rust (teks terpotong, lebar, flag openable) | **Ya** | Fase 6 (Fase 5 memakai cache Swift sementara) | Swift menggambar tanpa memformat, dan parse JSON per render hilang. |
| Analisis editor di Rust | **Ya**, sebagai port aturan lexer ke `qh-sql` | Fase 4B | Satu scanner untuk engine dan editor, off-main, dan duplikat `SQLScanner.swift` terhapus. |
| **tree-sitter** | Tidak | — | Klasifikasi token akan berbeda, jadi highlight berubah di layar dan melanggar paritas visual. Ditambah build C dan pemeliharaan grammar. |
| PostgreSQL `COPY ... TO STDOUT` teks | Ya, bersyarat angka | Fase 7.2 | Jalur massal tercepat yang tetap teks. |
| `COPY BINARY` / hasil biner | Bersyarat | Fase 7.3 | Hanya bila semua OID bisa di-decode dan output teks server terbukti dominan. |
| mimalloc, PGO | Bersyarat | Fase 7.5 | Diukur dulu, termasuk dampaknya ke memori. |
| `target-cpu` per arsitektur | Tidak | — | Default sudah `apple-m1`, dan hanya ada slice arm64. |
| Prefetch data spekulatif saat pohon dipilih | Tidak | — | Query yang tidak diminta siapa pun. Di Trino itu slot gudang dan biaya. Sikap ini sudah dipegang keputusan session restore. |
| Warm-up sesi saat koneksi dipilih | **Ya** | Fase 2.6 | Hanya connect, tanpa query. |
| Cache metadata di Rust untuk autocomplete | Tidak | — | `TreeNode` sudah menjadi cache-nya. Cache kedua berarti dua sumber kebenaran. Yang membuatnya cepat adalah pool di Fase 2. |
| Arrow C Data Interface / buffer kolumnar zero-copy dibaca Swift | Tidak | — | Swift harus merender tipe sendiri, sehingga ada renderer kedua. Jendela terender yang dikemas sudah beberapa µs. |
| C ABI tulisan tangan | Tidak secara default | Fase 8 | UniFFI yang mengembalikan satu buffer per jendela ≪ 1% anggaran frame. Diputuskan oleh angka Fase 6. |
| Spill lewat mmap | Tidak | — | `unsafe_code` di-forbid di crate ini, dan `pread` dari page cache hanya puluhan µs per chunk. |
| Grid Metal | Tidak secara default | Fase 8 | NSTableView dengan sel yang digambar mencapai anggaran bila tidak ada view per sel. |
| CodeEditTextView / TextKit 2 | Tidak secara default | Fase 8 | Menulis ulang semua fitur editor, dan menutup jalan Vim yang lewat `interceptKey`. |
| Protokol biner MySQL | Bersyarat | Fase 7 | Hanya bila profil menunjukkan parse teks dominan. |
| Parsing SIMD | `memchr` ya; JSON SIMD bersyarat | Fase 7.2, 7.4 | |
| Paging di server secara default | Sort/search: Batch 7. Paging via portal: bersyarat. | Fase 3, 8 | |

## 14. Perubahan perilaku yang terlihat meski piksel sama

1. **Baris datang terus-menerus.** Baris muncul per frame, bukan per 200 ms. Hitungan di footer naik, dan area scroll tumbuh saat di-scroll.
2. **Sort header dinonaktifkan selama streaming.** Hari ini sort terhapus setiap kali paint, jadi aturan lama ini sekarang terlihat.
3. **Sort server menjadi default (Batch 7).** Cakupannya seluruh hasil, bukan hanya baris yang sudah diambil. Kolasinya milik server: kolasi database PostgreSQL, `utf8mb4_0900_ai_ci` di MySQL, urutan byte di Trino. Satu klik berarti satu query. Banner hilang di jalur server.
4. **Sort in-memory di Rust.** Kolom bertipe diurutkan menurut nilai: timestamp dengan offset campuran sekarang kronologis, bukan leksikal. Teks memakai kunci natural, dan divergensinya terhadap `localizedStandardCompare` dicatat.
5. **Stop membatalkan query di server.** Server mencatat "canceling statement due to user request", dan app memetakannya ke `cancelled`, bukan galat (ADR-0016 sudah membedakan cancel dari timeout).
6. **Sesi dipakai ulang tetapi di-reset per Run.** Rekomendasinya tidak mengubah semantik. Bila pemilik memilih sesi yang lengket, `SET`, `USE`, dan tabel temp akan bertahan di antara Run.
7. **Batas `rowLimit`.** 100.000 dulu, lalu 5.000.000 setelah Fase 6.
8. **Editor.** Pewarnaan dan folding sekarang bekerja di atas 200k karakter. Karakter yang baru diketik bisa berwarna dasar selama ≤ 1 frame.
9. **Grid.** Tooltip AppKit (sistem yang sama dengan `.help` SwiftUI), dan nilainya penuh. Field editor AppKit menggantikan `TextField` SwiftUI, dengan perilaku Return, Esc, dan undo yang dikunci tes. Kolom tidak bisa di-drag untuk dipindah, sama seperti hari ini.
10. **Sort dan search dengan edit tertunda ditolak** dengan pesan (Fase 3).
11. **Pemecahan statement di editor mengikuti `scan.rs`** (4B, dan W4-T2b untuk Run), jadi editor sepakat dengan engine. Perbedaan terhadap `sqlStatements` Swift hari ini:
    - `;` di dalam `"a;b"` atau `` `a;b` `` tidak lagi memecah;
    - `;` di dalam `$tag$ … $tag$` tidak lagi memecah;
    - potongan yang hanya komentar (`SELECT 1; -- akhir`) bukan statement lagi, jadi tanpa band dan run mark;
    - teks CRLF yang memuat `--`: komentar baris berakhir di LF, bukan tidak pernah berakhir;
    - potongan yang hanya NBSP, U+2028, atau VT dianggap signifikan (`is_ascii_whitespace`), tidak lagi dibuang.
12. **Karakter yang baru diketik mewarisi warna tetangga** selama ≤ 1 frame sampai pewarnaan asinkron tiba (typing attributes, bukan warna dasar). Mengetik di dalam komentar atau string tidak berkedip.

## 15. ADR yang dibatalkan, diamandemen, atau dibuat

Nomor baru adalah nomor kosong berikutnya saat ditulis.

| ADR | Nasib | Isi |
|---|---|---|
| 0003 grid NSTableView | Diamandemen oleh 0032 | Kembali ke NSTableView, tetapi sel digambar CoreText (`viewFor` nil), view aksesibilitas yang lazy, edit lewat field editor overlay, syarat eskalasi Metal. Konsekuensi "sort/filter di Rust" tetap, sebagai fallback Batch 7. |
| 0004 UniFFI control plane | Diamandemen oleh 0030 | Data plane = objek UniFFI `ResultHandle` dengan satu buffer terender per jendela. Baris app tidak lagi lewat event. C ABI hanya eskalasi. Event tetap berupa callback. |
| 0008 store kolumnar kustom | Diamandemen | Codec bertipe per chunk. Jendela mengembalikan teks terender, bukan `Vec<Vec<Value>>`. mmap ditolak. Alasan menolak Arrow diganti menjadi "renderer tunggal di Rust". |
| 0009 panic unwind | Tetap | Semua ekspor baru throwing. Relevan untuk eskalasi C ABI. |
| 0010 QoS | Diterapkan + addendum | Runtime app memakai `build_main`. Pool rayon P-core `USER_INITIATED`. Antrean Swift `.userInitiated`. |
| 0013 throughput terikat JSON | **Digantikan** oleh 0030 | Protokol baris app diganti sekarang. NDJSON tetap untuk CLI, MCP, dan golden. Klaim 99,6% diganti hasil profil Fase 0.1. |
| 0016 timeout statement | Tetap + addendum | Sesi pool menerapkan timeout per run (PostgreSQL sudah melacaknya). |
| 0028 binding parameter | Mungkin diamandemen oleh 0035 | Hanya bila hasil biner diadopsi di Fase 7.3. |
| baru 0030 | Dibuat | Data plane app lewat result store. |
| baru 0031 | Dibuat | `EngineHost`: runtime persisten, pool sesi dan semantik reset, cancel preemptif + server-side. |
| baru 0032 | Dibuat | Grid dengan sel yang digambar. |
| baru 0033 | Dibuat | Analisis editor di Rust (`qh-sql`), tanpa tree-sitter. |
| baru 0034 | Dibuat | View in-memory di Rust (rayon, kunci kolasi). DataFusion ditolak. |
| baru 0035, 0036 | Bersyarat | Protokol per driver (COPY/biner); allocator dan PGO. |

## 16. Hubungan dengan rencana lain

- **`tablepro-adoption-plan.md` §12.2:**
  - 12.2.1 (cache `displayedRows`) sudah mendarat, dan Fase 6 menghapusnya.
  - 12.2.2 digantikan Fase 4.
  - 12.2.3 sudah mendarat. Di Fase 4B batas statement pindah ke `qh-sql`, persis seperti saran dokumen itu sendiri.
  - 12.2.4 (runtime per `run`) diserap Fase 1.
  - 12.2.5 bukan butir performa.
  - 12.2.6 di luar lingkup.
  - 12.1.3 dan 12.1.4: batas folding naik ke 2M di 4B, dan `parseLimit` dipakai deteksi openable di Rust.
  - 12.1.5 sudah mendarat sebagai sort server untuk hasil terpotong, dan diperluas oleh Batch 7.
- **§13 Gelombang 2 (kedalaman grid) dan §4.3/§4.4:** masuk daftar paritas Fase 5. Kalimat banner §4.3 diubah oleh Batch 7.
- **`remaining-work-plan.md`:** Batch 6 (setelan Data) harus hidup di grid baru. Batch 7 = Fase 3. Vim yang ditunda tetap mungkin, karena NSTextView dan `interceptKey` dipertahankan.
- **Blueprint:** §2.4 (halaman, cache 3 halaman, prefetch), §2.6 (handle + generation), dan §2.7 (cancel per driver) dilaksanakan di Fase 6, 6, dan 1–2. Angka offset-buffer di §4.2 diganti angka jendela terender yang diukur.
- **`docs/benchmarks.md`:** baris §6 yang masih "[belum diukur]" terisi mulai Fase 0.

## 17. Dependensi pihak ketiga dan lisensinya

| Dependensi | Lisensi | Fase | Keterangan |
|---|---|---|---|
| `rayon` | MIT OR Apache-2.0 | 6 | pool dikonfigurasi `qh-rt` |
| `unicode-segmentation` | MIT OR Apache-2.0 | 6 | hitungan grapheme untuk lebar kolom |
| `memchr` | Unlicense OR MIT (MIT dipakai) | 7 | kemungkinan sudah transitif |
| `mimalloc` (crate) | MIT | 7, bersyarat | |
| `sonic-rs` | Apache-2.0 | 7, bersyarat | |
| `cargo-pgo`, `samply` | MIT OR Apache-2.0 | 0, 7 | alat, tidak dikirim |
| toxiproxy | MIT | 0 | container dev saja |
| CodeEditTextView, CodeEditSourceEditor (upstream CodeEditApp) | MIT menurut salinan lisensi di pohon TablePro; **wajib diverifikasi dari repo aslinya** | 8, eskalasi | fork TablePro dilarang |
| Ditolak: DataFusion, `arrow` (Apache-2.0), tree-sitter + `tree-sitter-sequel` (MIT) | — | — | ditolak karena alasan teknis, bukan lisensi |

Semua lisensi di atas masuk allow-list `deny.toml` (MIT, Apache-2.0, BSD-3-Clause, ISC, Unicode-3.0, Zlib). Framework Apple (CoreText, ScreenCaptureKit, `xctrace`) adalah bagian sistem.

## 18. Risiko lintas fase

- **Paritas visual vs pergantian renderer.** Geometri dikunci secara numerik, teks dengan toleransi, dan pemilik meninjau PNG berdampingan.
- **Pekerjaan UI yang belum di-commit.** Prasyarat P dan aturan freeze.
- **Noise pengukuran.** Median dan p95, run bergantian, beban mesin dicatat, dan semua gate dijalankan di mesin yang sama.
- **Permukaan UniFFI tumbuh.** Invariant #1 dan #11: `app/Generated` diregenerasi dan di-commit, dan `RustEngineTests` menjaga daftar perintah.
- **Semantik sesi yang dipakai ulang.** Tes reset di Fase 2.
- **Lingkup merayap.** Tidak ada fitur baru di dalam fase performa. Yang ditolak di §13 tetap ditolak.

## 19. Pertanyaan terbuka untuk pemilik

1. **Sesi:** reset per Run (direkomendasikan, semantik sama dengan hari ini), atau sesi lengket per tab di mana `SET`/`USE`/tabel temp bertahan?
2. **Batch 7:** apakah hasil yang **lengkap** boleh disortir in-memory tanpa query ulang (lebih cepat, tetapi kolasinya bisa berbeda dari server)? Rekomendasi: tidak, demi konsistensi.
3. **Kolasi fallback:** apakah kunci natural dengan divergensi yang dicatat bisa diterima, atau harus identik dengan `localizedStandardCompare`? Pilihan kedua berarti memanggil CoreFoundation dari Rust, dan sort teks 500k menjadi sekitar 5× lebih lambat.
4. **Paritas visual:** apakah "geometri identik + teks dalam toleransi + tinjauan berdampingan" diterima? Bit-identik tidak bisa dicapai ketika renderer berganti.
5. **Baseline:** PNG di-commit ke `app/Tests/.../__Baselines__/` (direkomendasikan), atau dihasilkan ulang dari commit yang dipatok?
6. **Batas:** clamp `rowLimit` 100.000 sampai Fase 6 lalu 5.000.000, dan anggaran memori store 256 MB. Setuju?
7. **Harness:** izin Accessibility dan Screen Recording untuk `qhbench`, dan izin membangun TablePro lokal pada commit yang dipatok hanya untuk pengukuran.
