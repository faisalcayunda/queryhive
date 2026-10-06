# 0030 — Data plane app: store hasil Arrow per chunk, `window()` terender, Explain di store (menggantikan 0013 dan 0008, mengamandemen 0004)

- **Status:** Diterima sebagai keputusan arsitektur. Sisi Rust sudah mendarat (W4-T3 `e595877`, W4-T4 `b1a1838`, W5-T2
  `df445fd` dan `bfb9680`, penutupan W5-C `1b59154`). **Sisi Swift (W6-T1) belum dibangun**: `StoreRows`, halaman
  jendela, `swiftRenderedFormats`, penjaga edit, display link, dan `RLIMIT_NOFILE` ditulis di bawah sebagai
  **direncanakan**, bukan selesai. Blueprint Fase 6 sudah disegarkan di W6-A1 (`c19296e`), tetapi verdict
  architect-reviewer atas penyegaran itu belum ada, jadi D-22 sampai D-29 bisa berubah sebelum W6-T1 selesai.
- **Tanggal:** 6 Okt 2026 (W6-D, perf-parity Fase 6)
- **Konteks instruksi:** `docs/architecture/development-plan.md` §4 (jadwal ADR, baris 0030) dan W6-D;
  `docs/architecture/blueprints/fase-6-data-plane.md` §3 (D-1 sampai D-15, D-22 sampai D-29), §23, §26;
  `docs/architecture/performance-plan.md` §10; PRD `docs/architecture/prd-performance-and-parity.md` FR-PERF-05,
  FR-GRID-03, FR-GRID-04, NFR-P1 (S2), NFR-P2, NFR-P3, NFR-P8, O-12, O-15, O-18.
- **Menggantikan:** ADR-0008 (store kolumnar kustom) dan ADR-0013 (target throughput dibatasi JSON). Keduanya diberi
  catatan bertanggal di bagian atas; badannya tidak ditulis ulang.
- **Mengamandemen:** ADR-0004 (UniFFI untuk control plane). Pemisahan control plane dan data plane bertahan; yang berubah
  adalah isi data plane (objek `ResultHandle` dan buffer `QHW1`, bukan sekadar "handle + offset").
- **Berhubungan dengan:** ADR-0032 (grid sel digambar; seam `ResultRows` yang diisi `StoreRows` di sini), ADR-0034
  (view in-memory di Rust, kolasi, divergensi), ADR-0037 (spill terenkripsi; format rekaman dan aturan kunci),
  ADR-0045 (helper analitik DataFusion, W13-D, belum ditulis).
- **Catatan penomoran:** D-1 sampai D-29 di ADR ini adalah nomor dari blueprint Fase 6. Nomor D yang sama di blueprint
  Fase 2, 4B, dan 5 adalah keputusan yang berbeda.

## Konteks

Hari ini, di app, hasil query adalah `PreviewResult.rows: [[String?]]` yang ditumpuk di Swift dari event NDJSON, lalu
disaring, diurutkan, dan dicari di Swift. Profil W1-T8 (jalur FFI in-process, `wide_500k`) menunjukkan ke mana waktu
pergi: decode driver 38,4%, transpose/clone 26,8%, `to_text` 13,2%, serde_json 19,6%, dan satu sel teks disalin sekitar
empat kali. Plafon `COPY` sekitar 990k baris/s. Klaim ADR-0013 bahwa JSON adalah 99,6% waktu tidak bertahan: angka itu
hanya `(elapsed − TTFR)/elapsed`. Profil yang sebenarnya menempatkan JSON di 11% (CLI) dan 20% (FFI), penulisan stdout
36% (CLI), dan decode driver 25 sampai 38%, jadi batasnya CPU satu thread engine, bukan protokol.

Tiga keadaan baru membuat ADR-0008 dan ADR-0013 tidak lagi memandu pekerjaan:

1. **Keputusan pemilik O-15.** Arrow dan Apache DataFusion diadopsi. ADR-0008 menolak Arrow sebagai fondasi karena
   dependensi (~60 crate transitif) dan karena Arrow tidak melayani window acak serta spill. Probe W2-A3 (`target/run/df-probe`)
   mengukur ulang: gugus arrow untuk store menambah **18** crate (bukan ~60), versinya sama dengan `parquet` 59.3 yang
   sudah ada di `Cargo.lock`, dan `qh-result-store` memakai kembali indeks chunk + `partition_point` untuk window acak.
2. **Keputusan pemilik O-18.** DataFusion hidup di executable terpisah, `queryhive-analytics` (W13-T8a), supaya app tetap
   sekitar 30 MB (DataFusion menambah +64,79 MB stripped, §2.2 blueprint). Store Arrow tetap di app, dan grid tidak
   boleh bergantung pada helper.
3. **Seam Fase 5 sudah ada.** ADR-0032 menetapkan `ResultRows` dengan `cell(row:column:format:)`, `fullValue`, `rows(in:columns:)`,
   `naturalCharCounts()`, dan `distinctValues(column:)` (`app/Sources/QueryHive/Models/ResultRows.swift`). Data plane
   tinggal mengisinya.

Bentuk masalahnya tetap sama dengan ADR-0004: 500.000 baris × 30 kolom harus sampai ke grid tanpa menyalin ke Swift,
dengan window akses acak per viewport, spill di atas anggaran memori, dan time-to-first-row S2 yang tidak menunggu decode JSON.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Format store | **`RecordBatch` Arrow per chunk, encoding dipilih per (chunk, kolom), kolom `Binary` bertag untuk yang tak seragam** | Satu representasi untuk memori, spill (IPC), window, view, dan DataFusion di helper; round trip `Value` tanpa kehilangan, termasuk `Unknown` bentuk `raw` | Dua puluhan encoding dan codec bertag yang harus dijaga; skema fisik berbeda antar-chunk |
| | Store kolumnar kustom (ADR-0008) | Tanpa dependensi arrow | Konversi ke Arrow kelak untuk DataFusion; layout `QHC1` yang sudah disegel W2-A3 tidak pernah dibangun |
| | `Vec<Vec<Option<String>>>` lewat UniFFI | Sederhana | Persis masalah P1: salinan penuh dan jutaan alokasi |
| Jalur ke Swift | **`window()` mengembalikan satu buffer `QHW1` berisi teks terender, offset, dan flag** | Swift tidak tahu apa-apa soal Arrow; renderer `qh_core::render` tetap satu; satu salinan `RustBuffer → Data` | Teks dirender di Rust, jadi format kolom yang belum teruji paritasnya perlu pintu darurat (D-24) |
| | Arrow C Data Interface ke Swift | Tanpa render di Rust | Swift harus merender tiap tipe sendiri (dua renderer yang bisa berselisih); ditolak `performance-plan.md` §13 |
| Batas FFI | **UniFFI untuk objek `ResultHandle` (semua metode throwing); buffer data lewat `Vec<u8>`** | Pemetaan error dan `Sendable` tetap dari UniFFI | Satu penyeberangan UniFFI per halaman; `StoreWindowBench` yang mengukurnya; eskalasi C ABI Fase 8 bila p99 > 0,5 ms |
| | C ABI manual sekarang | Tanpa generator | Biaya dan risiko bug besar, belum terbukti perlu |
| Explain | **Explain menulis ke store seperti Run** | Grid satu jalur; `RESULT_SINK=store` untuk `preview` dan `explain` | Menyimpang dari `performance-plan.md` §10 butir 5 (dicatat R-13); golden `explain` dijaga G-GOLDEN |
| | Explain tetap NDJSON ke `ArrayRows` | Tanpa perubahan | Mempertahankan penumpukan baris di Swift hanya untuk satu perintah, dan `ArrayRows` produksi tidak jadi habis |
| Tempat view | **Rust (ADR-0034)** | 500k baris tidak pernah disalin ke Swift | Semantik Swift harus di-port dan diuji diferensial |
| | Swift (status quo) | Tidak ada port | Setiap baris hidup di Swift sebagai `[[String?]]`, dan tiap revisi filter, search, atau sort membangun ulang array tersaring dan terurut di thread utama (`displayedRows` di-cache per `gridRevision`, `QueryTab.swift:869-892`, jadi biayanya per perubahan, bukan per `body`); tidak tahan 500k × 30 |

## Keputusan

**Hasil query di app disimpan di Rust sebagai deretan `RecordBatch` Arrow (arrow-rs 59.3), satu batch per chunk. Swift
membacanya hanya lewat `ResultHandle`: halaman teks terender `QHW1` untuk sel yang digambar, dan teks penuh lewat
`cell_text` dan `rows_text`. Explain menulis ke store yang sama. ADR ini menggantikan ADR-0008 dan ADR-0013 dan mengamandemen ADR-0004.**

Rincian yang mengikat. Bagian "Sudah dibangun" diperiksa terhadap kode di `465a0c6` dan sesudahnya; bagian
"Direncanakan (W6-T1)" belum ada di repo.

### Sudah dibangun (sisi Rust)

1. **Store = `RecordBatch` Arrow per chunk (D-1).** Crate `qh-columnar` (D-15) memegang pemetaan `Value` ↔ Arrow,
   `ChunkBuilder`, codec bertag, dan `value_at` (`crates/qh-columnar/src/lib.rs`, `#![forbid(unsafe_code)]`).
   `qh-result-store` bergantung padanya, bukan sebaliknya, supaya driver kelak (W7-T1) menulis array Arrow tanpa
   menarik spill, kripto, dan rayon. Chunk disegel pada 65.536 baris atau perkiraan 2 MiB (`builder.rs:31` dan `:34`).
   Tipe teks adalah `Utf8`, bukan `Utf8View`, karena offset 4 byte lebih kecil dari view 16 byte dan selisihnya keluar
   dari anggaran 256 MiB (`qh-columnar/src/lib.rs`). Layout `QHC1` buatan sendiri tidak dibangun.
2. **Encoding per chunk-kolom (D-2).** Varian `Value` yang benar-benar datang menentukan encoding. Chunk-kolom yang
   tidak seragam menjadi `Binary` bertag (`qh.enc = tagged`), sehingga `Value` apa pun kembali utuh. Itu menutup
   cacat lama: `decode_value` di codec lama mengubah `Unknown` bentuk `raw` menjadi teks lossy (B-5). Skema logis
   untuk SQL disatukan terpisah di `crates/qh-result-store/src/logical.rs` (blueprint §5.4).
3. **Publikasi hanya lewat chunk yang disegel (D-3)**, dan **satu anggaran global 256 MiB di `StoreRegistry` (D-4)**
   (`registry.rs`). Sewa memori untuk helper (bagian D-4) belum ada: ia dibangun di W13-T8b.
4. **Spill (D-5)** mengikuti ADR-0037 apa adanya: rekaman `QHP1` (stream IPC Arrow + flag) disegel AES-256-GCM dengan
   kunci acak per proses, berkas di-unlink sebelum byte pertama, `pread`/`pwrite`, tanpa mmap.
5. **`window()` terender (D-6).** `ResultHandle::window(view_id, first_row, row_count, columns, formats)` mengembalikan
   satu `Vec<u8>` `QHW1` (`crates/qh-ffi/src/store_api.rs:610`): magic, versi, jumlah baris dan kolom, `source_rows`,
   offset teks, flag per sel (NULL 1, EMPTY 2, OPENABLE 4, NUMERIC 8, TRUNCATED 16), dan heap UTF-8. Teks dipotong di
   256 unit UTF-16 pada batas grapheme dan `TRUNCATED` menyala bila terpotong; `rows_text` dan `cell_text` tidak
   dipotong. Teks dirender lewat `qh_core::render::write_text` (`crates/qh-core/src/render.rs:79`), implementasi
   tunggal yang dibungkus `to_text`. Array Arrow tidak pernah menyeberang FFI. Batas: R ≤ 4.096, C ≤ 1.024, R × C ≤ 262.144.
6. **Explain di store (D-8, R-13).** `explain` memanggil `row_target(settings, out)` seperti `preview`
   (`crates/qh-ffi/src/commands.rs:1593`), dan `RESULT_SINK=store` hanya diterima bila emitter membawa store dari engine
   host (`commands.rs:2025`). Dari CLI, MCP, dan golden, nilai itu adalah galat usage, jadi mereka tetap NDJSON (D-11, D-14).
7. **Handle dibuat pemanggil sebelum run (D-12)**, dan metode `ResultHandle` semuanya throwing (amandemen D-1 Fase 2):
   `row_count`, `window`, `rows_text`, `cell_text`, `set_view`, `distinct_values`, `column_widths`, `release`
   (`store_api.rs`). Tambahan yang dibutuhkan fitur yang ada (D-13): `distinct_values`, `rows_text`, `store_from_rows`,
   `store_stats` (metode `EngineHost` di `crates/qh-ffi/src/host.rs`); `store_synthetic` hanya konstruktor Rust.
8. **Tidak ada perintah engine baru (D-14).** Empat daftar invariant #11 tidak tersentuh.

### Direncanakan (W6-T1, belum ada di repo)

9. **`StoreRows: ResultRows` di `Models/StoreRows.swift` (baru)**, dengan `poll()`, `prepare(formats:)`, `apply(ViewSpec)`
   asinkron, `distinctValues(column:) async`, `rowsOrThrow`, `dropPages()`, dan `release()` idempoten. Tipe FFI hanya
   boleh muncul di `StoreRows.swift` dan `Support/RustEngine.swift`; `DatabaseEngine.swift` tetap bebas FFI. Produksi
   memakai `EmptyRows` untuk "belum ada hasil".
10. **Dua store per tab untuk "off" (Batch 7).** `QueryTab` memegang `baseResult` dan `activeResult`, paling banyak dua
    store per tab; `BaseResultCache` (aturan 10.000 baris) dihapus karena anggaran global dan spill menggantikannya.
11. **Umur handle dan penutupan tab.** `closeTab` menghentikan `process` **dan** `previewProcess`, lalu melepas kedua
    store. Hari ini hanya `process?.terminate()` (`AppModel.swift:710`), padahal `preview` dan `explain` disimpan di
    `previewProcess` (TM-4); itu cacat yang ditutup W6-T1.
12. **Startup.** `ensureStoresConfigured(spillDir:budgetBytes:)` dipanggil sekali, sinkron, sebelum `QueryHiveApp.main()`,
    dengan `~/Library/Caches/QueryHive/spill` dan anggaran 256 MiB.
13. **Tiga commit** untuk W6-T1 (6a refaktor kolom terlihat dan kontrak `distinctValues`, 6b store masuk, 6c plafon),
    pelaksana sonnet (O-20), tinjau satu putaran oleh model terkuat ditambah reviewer database.

### D-22 sampai D-29 (disegarkan W6-A1, 2026-10-06; semuanya direncanakan)

| # | Keputusan | Status | Alasan singkat |
|---|---|---|---|
| D-22 | Halaman jendela **64 baris × blok 32 kolom sumber**, miss sinkron di main, tanpa prefetch. Cache paling banyak 24 halaman dan 8 MB, tidak pernah di bawah 8 halaman. `PAGE_ROWS`, `COL_BLOCK`, dan batas cache adalah konstanta **sementara**; W5-T3 dan `StoreWindowBench` mengonfirmasi atau menggantinya (128 baris bila p99 128 × 32 ≤ 0,4 ms termasuk UniFFI, 32 baris bila p99 64 × 32 > 0,4 ms). | Direncanakan. Angka dasarnya terukur (§2.9). | 128 × 30 bertipe 0,396 sampai 0,449 ms p99 di sisi Rust, jadi 128 × 32 tanpa margin terhadap 0,5 ms. 64 × 32 diperkirakan 0,21 sampai 0,24 ms (ekstrapolasi linear, **belum diukur**). |
| D-23 | Tabel membangun dan mengecat **hanya kolom yang tergambar** (rentang dari `GridColumnGeometry.columns(in:)` ± satu lebar viewport), commit 6a di atas `ArrayRows`. | Direncanakan (commit 6a). Cacatnya tercatat di blueprint TM-2 terhadap `465a0c6`: `GridTableView.draw` membuang rentang (`_ = columns`) dan `rowText` membaca semua kolom. Saat ADR ini ditulis pohon kerja punya perubahan 6a yang belum di-commit di `GridTableView.swift`, `GridRowView.swift`, dan `ResultGridTable.swift`, jadi nomor baris lama tidak lagi berlaku dan ADR tidak mengutipnya. | Hasil 500 kolom membayar 500 pembacaan sel per baris. 6a diverifikasi G-VIS tanpa rekam ulang. |
| D-24 | Pintu darurat format = `StoreRows.swiftRenderedFormats`, awalnya `{.json}`: kolom di himpunan itu dibaca lewat `rows_text` dan `ColumnFormat.render` di Swift. Tanpa parameter FFI baru. Format lain pindah ke himpunan itu bila tes kembaran menemukan selisih yang bukan divergensi tercatat. **`Raw` tidak pernah pindah**: selisih `Raw` di luar divergensi tercatat adalah bug dan menghentikan W6-T1. Divergensi tercatat: potongan 256 UTF-16 dan `TRUNCATED` milik `window` (lawan 1.024 `Character` milik `ArrayRows`), dan `openable` kembaran yang lebih longgar. | Direncanakan. | Paritas format di Rust belum teruji: fixture W4-T4 hanya mencakup sort, filter, dan search (lihat ADR-0034). Printer `Json` tidak punya satu pun tes paritas; `Json` kembali ke Rust hanya setelah fixture `format.json` dua sisi (B-21). |
| D-25 | `ArrayRows` dan implementasi sort, filter, search Swift **pindah ke target tes** (`ArrayRowsReference.swift`, `SwiftGridReference.swift`) sebagai kembaran acuan, tidak dihapus. | Direncanakan. | Menjaga sepuluh tes `ResultRowsTests`, jaring paritas format dan flag, dan `SortFixtureExport`, yang memanggil implementasi Swift (TM-13) dan tidak akan terkompilasi bila dihapus. |
| D-26 | Mitigasi R-19: `RLIMIT_NOFILE` lunak dinaikkan ke min(batas keras, 4.096) saat peluncuran. Menutup fd store yang menganggur **tidak mungkin**. | Direncanakan; `setrlimit` belum ada di `app/Sources`. Batas lunak 256 diukur di mesin ini (TM-12). | Berkas spill di-unlink sebelum byte pertama (D-5), jadi fd satu-satunya pegangan ke datanya. 4.096 adalah 16 kali lipat, jauh di atas 100 tab × 2 store. `EMFILE` tetap diperlakukan seperti disk penuh (chunk resident, store lain utuh); ADR-0037 NEG-005 mencatat penanganannya belum diverifikasi tes. |
| D-27 | `apply(ViewSpec)` **asinkron, dengan penjaga edit**: selama `viewBusy`, mengisi, menempel, dan menyimpan perubahan sel ditolak; saat view terpasang, seleksi dan antrean edit dibuang. | Direncanakan. | Edit dikunci menurut indeks baris tampilan (`CellKey`); antara `set_view` selesai di Rust dan hop ke main, indeks itu menunjuk baris lain, dan `WritePlan` membaca nilai asli lewat `row(at:)` untuk klausa `WHERE` pada `UPDATE` dan `DELETE`. Tanpa penjaga, satu edit bisa menulis atau menghapus baris yang salah (R-35). |
| D-28 | Hitungan footer dari `progress` disaring 5 Hz (`footerCountInterval` 0,2 detik); grid dipoll display link paling tinggi 60 Hz; `rowsDidGrow(from:to:)` hanya membatalkan baris baru. | Direncanakan. | Mempertahankan kadens 200 ms hari ini untuk SwiftUI tanpa mengganti seluruh hasil. Baris yang sudah ada tidak berubah saat hasil bertambah, jadi `GridRowTextCache` tidak dikosongkan. |
| D-29 | Plafon 5.000.000 dinaikkan di commit tersendiri (6c) sesudah P-1. Lulus: naik. Gagal: `WindowedRows` masuk lebih dulu. Tidak terukur (izin OS): tetap 200.000. | Direncanakan. `productRowLimitCeiling` masih 200.000 (`AppModel.swift:2692`). | Aturan `development-plan.md` W6-T1 dan blueprint Fase 5 §5.4 dan §15. Dipisah supaya perpindahan data plane tidak tertahan izin Screen Recording. **Risiko** (ADR-0034 NEG-002): sort di plafon membangun satu kunci natural `Vec<u8>` per baris di luar anggaran 256 MiB (sedikitnya sekitar 325 MB isi kunci untuk 5.000.000 baris, hitungan tangan, belum diukur), jadi P-1 harus mengukur memori puncak sort. |

### Yang tidak berubah

CLI `queryhive-engine`, MCP, korpus golden, `export`, `to_table`, `import_data`, `apply_changes`, `table_op`, `count`,
dan perintah lokal tetap NDJSON (blueprint §20). Tes kesetaraan W5-T2 membandingkan `rows_text` store dengan array
`data` NDJSON sel demi sel, sebagai bukti bahwa renderer tetap satu.

## Alasan

1. **Satu representasi.** Memori, spill, window, view, dan (kelak) DataFusion membaca `RecordBatch` yang sama. Dengan
   store kustom, DataFusion berarti konversi per window atau per query. Keputusan pemilik O-15 menutup opsi itu.
2. **Jawaban atas empat alasan ADR-0008, satu per satu.** (a) Window acak dilayani indeks chunk + `partition_point`,
   bukan scan kolumnar; terukur 1,63 µs p99 untuk satu baris × 30 kolom `wide` dan 38 µs p99 untuk 128 × 30 `wide` (§2.6).
   (b) Arrow memang tidak punya spill, jadi spill tetap kita tulis dan enkripsi (ADR-0037); IPC hanya menggantikan layout
   buatan sendiri. (c) `Decimal128` memetakan `i128 + scale` tanpa konversi per halaman untuk skala seragam; nilai di
   luar presisi 38 jatuh ke `tagged`, tanpa kehilangan. (d) Dependensi terukur: +18 crate dan ≤ +7,35 MB stripped
   (batas atas, §2.2), bukan ~60 crate.
3. **Teks terender di Rust.** Swift tidak perlu tahu tipe Arrow, dan hanya ada satu renderer. Ongkosnya: `to_text` lama
   mengalokasikan sampai empat `String` per timestamp (288 ns per sel), sehingga `write_text` tanpa alokasi (D-6) wajib.
4. **ADR-0013 digantikan oleh profil, bukan oleh rasa.** Klaim "JSON 99,6%" salah; throughput yang dibatasi satu thread
   engine dijawab dengan menghapus transpose, clone, dan penumpukan baris di Swift, bukan dengan mengganti serializer.
5. **Halaman 64 × 32 (D-22).** Satu halaman menutupi 1.344 sampai 1.920 pt, jadi fling 3.000 pt/s hanya memicu sekitar
   dua miss sinkron per detik, sekitar 0,05% waktu main thread. Prefetch menambah jalur asinkron (basi menurut
   `viewID`, balapan dengan `apply`) untuk menghemat jumlah itu.
6. **Pintu darurat sempit (D-24).** Memindahkan hanya format yang terbukti selisih menjaga perilaku Swift persis, dan
   menolak memindahkan `Raw` menjaga seluruh alasan jalur jendela.

## Konsekuensi

### Positif

- **POS-001**: Jalur panas tidak lagi menyalin sel ke Swift. Tab yang menganggur tumpah lebih dulu, jadi NFR-P3 tidak
  dimakan base store yang tidak dipakai.
- **POS-002**: Satu format untuk app dan helper. Data plane siap DataFusion tanpa konversi, dan grid bekerja tanpa
  helper (O-18).
- **POS-003**: Round trip `Value` lossless, termasuk `Unknown` `raw` (B-5 tertutup di `qh-columnar/tagged.rs`).
- **POS-004**: Penyeberangan FFI terbatas pada satu `Vec<u8>` per halaman. Swift memeriksa magic, versi, panjang, dan
  offset monoton sebelum membaca (O(RC)).
- **POS-005**: `closeTab` yang benar (TM-4) menghentikan query yang masih berjalan di server, yang hari ini bocor.

### Negatif

- **NEG-001**: Kita memiliki bug store sendiri: codec bertag, flag chunk, penghitung nonce, dan dekoder QHW1. Dijaga
  tes properti, tes spill, dan tes kesetaraan golden, tetapi permukaannya lebih besar daripada sekadar memakai Arrow.
- **NEG-002**: Paritas format di Rust belum teruji (TM-10). Printer `Json` dan format `Text`, `Uuid`, `UnixTimestamp`
  di `render.rs` hanya punya empat tes unit. D-24 menanganinya dengan pintu darurat, bukan dengan bukti.
- **NEG-003**: Anggaran 256 MiB berada di tepi untuk `wide_500k` (kira-kira 250 MiB setelah `shrink_to_fit`, R-29). Hasil
  500k bisa tumpah beberapa puluh MiB atau tidak sama sekali, jadi G-BENCH(3) mencatat `spilled_bytes` di samping memori.
- **NEG-004**: Target lokal window 128 × 30 p99 ≤ 0,25 ms **meleset** (0,396 sampai 0,449 ms, §2.9). Halaman 64 baris
  adalah mitigasi sementara; bila p99 64 × 32 lewat UniFFI masih > 0,5 ms, jalurnya eskalasi C ABI Fase 8 (W8-T2).
- **NEG-005**: Explain lewat store menyimpang dari `performance-plan.md` §10 butir 5, dan `ArrayRows` hanya hidup
  sebagai kembaran tes, sehingga dua implementasi Swift lama harus dijaga agar tetap terkompilasi.
- **NEG-006**: Satu fd per store yang tumpah. Batas lunak 256 fd di app GUI terukur di mesin ini, dan mitigasinya
  (D-26) belum ada; tidak ada bench fd per store yang tumpah (TM-12).
- **NEG-007**: Search atas 1 juta × 30 memakan 768 sampai 796 ms (ekstrapolasi 500k: sekitar 384 sampai 398 ms), di atas
  sasaran 300 ms; itu sebabnya `apply` asinkron dan grid menampilkan view lama sampai yang baru siap.

### Belum ada

- Seluruh sisi Swift di bagian "Direncanakan". Angka yang bergantung padanya tidak ada: penyeberangan UniFFI
  (`StoreWindowBench`), miss chunk yang tumpah di main thread (perkiraan 0,4 ms tambahan, §2.5), 500k nyata (W5-T3, W6-T2),
  dan P-1.
- Verdict architect-reviewer atas penyegaran blueprint (lima koreksi blocking B1 sampai B5 sudah diterapkan di dokumen).
- `TM-8`: komentar `column_widths()` di `store_api.rs:685` masih salah (menulis "UTF-16 units" padahal hitungan grapheme);
  backlog ledger tanpa gelombang pemilik.
- Helper analitik dan `AnalyticsSession` (W13-T8a sampai c), ADR-0045.

## Bukti

- Kode: `crates/qh-columnar/`, `crates/qh-result-store/`, `crates/qh-ffi/src/store_api.rs`, `crates/qh-ffi/src/commands.rs`
  (`row_target`), `crates/qh-rt/src/lib.rs`, `app/Sources/QueryHive/Models/ResultRows.swift`.
- Tes: `crates/qh-ffi/tests/store_sink.rs` (`RESULT_SINK=store`, `closing_the_tab_mid_stream_ends_the_run_cancelled_and_not_failed`);
  `crates/qh-result-store/tests/window.rs`, `spill.rs`, `view.rs`, `logical.rs`, `differential.rs`.
- Pengukuran: blueprint Fase 6 §2.2 (ukuran), §2.4 (memori), §2.5 (spill), §2.6 (window Rust), §2.9 (angka W5-T2:
  `target/run/w5t2-bench-window.json`, `target/run/w5t2-bench.json`, commit `bfb9680`, scratch gitignored).
  Profil W1-T8: `target/run/w1-t8-profile.md`.
- ADR ini tidak menjalankan ulang gate atau bench. Pemeriksaan keberadaan kode dilakukan terhadap pohon kerja pada 6 Okt 2026.

## Verdict architect-reviewer

**Verdict: satu koreksi blocking atas ADR ini, sudah diterapkan (6 Okt 2026); menunggu tinjau ulang (pending review, O-20).**
Temuan architect-reviewer atas ADR-0030 dan ADR-0034 diteruskan orkestrator. Menurut O-20 review hanya satu putaran:
koreksi diperiksa terhadap kode oleh penulis (pembacaan kode, tanpa menjalankan gate, tes, atau bench) dan dicatat sebagai
pending review, bukan putaran ketiga. Temuan blocking atas ADR-0034 (kunci natural) ada di bagian verdict ADR itu.

**Blocking, diterapkan**

1. Baris "Tempat view / Swift (status quo)" pada tabel opsi mengulang bahwa `displayedRows` mematerialisasi koleksi di tiap
   `body`. Kode itu sudah tidak ada: `displayedRows` ada di `QueryTab.swift:869-892` dan di-cache per `gridRevision`
   (`displayedCache`, `:862-863` dan `:889-890`); `result` membungkusnya dalam `ArrayRows` yang di-cache per revisi
   (`:900-912`). Diperbaiki ke biaya yang nyata: semua baris hidup di Swift sebagai `[[String?]]`, dan tiap revisi filter,
   search, atau sort membangun ulang salinan tersaring dan terurut di thread utama. Keputusan (view di Rust) tidak berubah.
   Selain itu baris D-29 diberi catatan risiko memori kunci sort yang tidak ditagih (ADR-0034 NEG-002).

**Non-blocking, dicatat dan belum diterapkan**

- **Routing model W6-T1 saling bertentangan.** `development-plan.md` §5 (`:553`) menulis sonnet (O-20), tetapi §6 (`:747`
  dan `:784`) masih mendaftar W6-T1 di antara implementer opus, begitu pula PRD O-17 (`prd-performance-and-parity.md:424`).
  Pemeriksaan penulis: O-20 tercatat di `development-plan.md:785` (pohon kerja) dan `AGENTS.md:15`, selain ledger
  (`target/run/ledger.md:82`, gitignored); ia tidak ada di PRD. Bagian "Referensi" ADR ini menyebut "PRD ... O-20", yang tidak
  ada; penunjuknya harus ke `development-plan.md` §6 dan `AGENTS.md`. Usul: perbarui §6 `:747` dan `:784`, atau catat O-20 di
  PRD (keputusan pemilik).
- **Celah kepemilikan berkas.** `app/Tests/QueryHiveTests/GridColumnRangeTests.swift` (belum dilacak, milik lajur 6a) tidak ada
  di daftar berkas W6-T1 (`development-plan.md:558-561`) maupun blueprint §21.4. Menurut definisi selesai di `AGENTS.md`
  ("berkas cocok dengan daftar kepemilikan") commit 6a akan gagal. Tambahkan ke daftar, atau lipat ke `GridColumnsTests.swift`.
- **Butir 12 "Direncanakan"** menaruh `signal(SIGPIPE, SIG_IGN)` di W6-T1. Blueprint memberikannya ke W13-T8b
  (`fase-6-data-plane.md:1610`, R-33), begitu pula `development-plan.md` W13-T8b (`:733`, `App.swift` "SIGPIPE diabaikan").
  Keluarkan dari daftar W6-T1 atau tandai W13-T8b.
- **Butir 7 "Sudah dibangun"** menyebut `store_from_rows` dan `store_stats` seolah ada di `store_api.rs`. Keduanya metode
  `EngineHost` di `crates/qh-ffi/src/host.rs:377` dan `:390`; `store_synthetic` (`host.rs:434`) memang hanya Rust.
- **Butir 12** merencanakan `ensureStoresConfigured` sinkron di main thread sebelum `QueryHiveApp.main()`, sedangkan komentar
  Rust di `host.rs:300-301` menulis "Call it off the main thread". Catat bahwa blueprint sengaja menimpanya (§17.6: di bawah
  1 ms, direktori biasanya kosong), atau minta W6-T1 memperbaiki komentar Rust.
- **Baris status ADR-0008 dan ADR-0013** masih "Diterima" di bawah catatan "digantikan". Itu sama dengan pola catatan ADR-0003,
  tetapi pertimbangkan "Digantikan oleh ADR-0030" di baris status supaya pembaca yang hanya melihat status tidak tersesat.
  Kata "akan datang" pada catatan 2026-09-30 di ADR-0013 dan "nanti digantikan ADR-0030" di ADR-0037 (`:24`, `:186`) masih
  basi dan diserahkan ke orkestrator.

**Terverifikasi benar oleh reviewer, tanpa tindakan:** konstanta chunk `builder.rs:31` dan `:34`; `qh-columnar`
`forbid(unsafe_code)` dan `Utf8` (bukan `Utf8View`); `store_api.rs:610` (`window`), `:685` (komentar `column_widths`, TM-8),
`:691` (`set_view`), `:738` (`distinct_values`); flag sel `QHW1` 1/2/4/8/16 dan potongan 256 unit UTF-16 (`render.rs:23-35`);
batas jendela 4.096 / 1.024 / 262.144 (`store_api.rs:37-41`); `render.rs:79` (`write_text`); `commands.rs:1593` dan `:2025`
(`row_target`, `RESULT_SINK=store` ditolak tanpa store host); `AppModel.swift:710` (`closeTab` hanya menghentikan
`process`; `previewProcess` di `:2475` dan `:2973` tidak dihentikan); `AppModel.swift:2692` (plafon 200_000); tidak ada
`setrlimit` di `app/Sources`; ketujuh hash commit. Dari edit `development-plan.md`: G-LEAK, daftar berkas §5 W6-T1 (selain
celah di atas), §7, dan §11 butir 7 cocok dengan blueprint §26.

## Referensi

- ADR-0004 (diamandemen), ADR-0008 dan ADR-0013 (digantikan), ADR-0032, ADR-0034, ADR-0037, ADR-0009 (panic unwind
  lintas FFI), ADR-0010 (QoS).
- `docs/architecture/blueprints/fase-6-data-plane.md` §3, §11, §12, §17 sampai §20, §23, §25, §26.
- `docs/architecture/prd-performance-and-parity.md` O-12, O-15, O-18.
- `docs/architecture/development-plan.md` §6 (W6-T1 implementer per O-20).
- Tugas penerus: W6-T1 (Swift), W6-T2 (bench eksklusif, G-LEAK), W7-T1 (driver menulis array Arrow), W13-T8a sampai c dan
  ADR-0045 (helper DataFusion).
