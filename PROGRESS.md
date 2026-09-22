# PROGRESS — Migrasi Engine Python → Rust

> Dokumen kerja berjalan (§4.2). Diperbarui setiap selesai satu tugas.
> **Baca ini lebih dulu di awal sesi, lalu lanjutkan dari titik terakhir.**

- **Branch aktif:** `feat/rust-engine` (dibuat dari `main` @ `602bfb7`)
- **Fase aktif:** **Fase 0 hampir selesai** → dilanjutkan ke **Fase 1** (core engine Rust)
- **Mesin:** macOS arm64, `rustc 1.98.1`, `cargo 1.98.1`, Swift 6.2.3, podman (VM
  `podman-machine-default` sudah start, `podman ps` bersih tanpa container)

---

## Status per tahap

| Tahap | Status | Catatan |
|---|---|---|
| A — Audit codebase | **Selesai** | §1 blueprint. Diagnosis "fetch lambat" terbagi lima penyebab (P1–P5), semuanya merujuk path file dan sudah diverifikasi ulang terhadap kode. |
| B — Riset web | **Berjalan sebagian** | `tools/kenari_search.py` — alat lokal yang **tidak masuk repo** (lihat §Perkakas lokal) — membuka `web_search` dan `web_fetch` lewat akun kenari pengguna, yang **terbukti bersaldo**: pencarian nyata mengembalikan hasil. Ini akun yang berbeda dari yang menghasilkan 402 di atas. `x_search` belum: jawabannya `plan_limit_reached` karena ditagih dari saldo terpisah, bukan kuota paket. §8.1 kini **terisi sebagian**: empat issue DBeaver dibuka langsung dan dikutip; Navicat dan DataGrip masih terbuka karena sumber yang ditemukan ditolak (bukan sumber primer). Satu klaim lama **dikoreksi**: TablePlus ternyata sekali beli, bukan langganan. `sqlx` terverifikasi langsung dari crates.io API (0.9.0, `MIT OR Apache-2.0`, 2026-05-21). Versi dependency lain diverifikasi lewat resolusi Cargo → `docs/dependencies.md`. |
| C — Blueprint + ADR | **Selesai** | `docs/architecture/rust-engine-blueprint.md` §1–§8 + Architecture Decision Summary; ADR 0001–0010 di `docs/decisions/`. |
| D — Fase 0 | **Selesai kecuali protocol Swift** | Golden snapshot ✅, baseline benchmark ✅, tag `python-engine-final` ✅, protocol `DatabaseEngine` + `MockEngine` ❌ (lihat catatan di bawah) |
| D — Fase 1 | **Sedang dikerjakan** | `qh-core`, `qh-sql`, `qh-result-store`, `qh-driver`, `qh-driver-postgres`, dan `qh-driver-mysql` selesai dan hijau; Trino, export, credentials, storage, tunnel, FFI belum ada |
| D — Fase 2, 3, 4 | Belum | |

---

## Tugas selesai

### Tahap A–C

- [x] Branch `feat/rust-engine`
- [x] Verifikasi ulang setiap klaim §1 terhadap kode (bukan asumsi):
  `queryhive_engine.py:240` `PREVIEW_BATCH = 200`; `queryhive_engine.py:744` `to_text` per sel;
  `Support/Engine.swift:95` `JSONDecoder` per baris; `Models/Connections.swift:364` service
  Keychain `id.data-ecosystem.queryhive`; `app/Package.swift` `macOS(.v14)`; `LICENSE:1` MIT
- [x] Blueprint §2–§8 + Architecture Decision Summary, termasuk tabel konflik instruksi
- [x] ADR 0001–0010
- [x] `docs/golden-deltas.md` dengan dua perbedaan teridentifikasi (D-1 notasi ilmiah DECIMAL,
  D-2 kutip JSON pada INTERVAL)

### Fase 0 — golden snapshot

- [x] `tools/golden/record.py` — menjalankan engine Python **in-process** memakai harness
  `tests/test_engine_events.py` (diimpor, bukan diduplikasi), menormalkan `elapsed_ms`,
  `query_id`, dan path tmp, lalu menulis stdout verbatim satu baris JSON per event
- [x] `tools/golden/compare.py` — menjalankan ulang kasus yang sama dan membandingkan;
  **21/21 cocok**, dan dua kali dijalankan hasilnya identik (idempoten)
- [x] `tests/golden/` — 21 kasus: 11 perintah, tiga jalur error, dan zoo tipe §1.8
  (`decimal(38,10)` presisi penuh, timestamptz `+07:00`, timestamp naive, date/time,
  interval, bytea berisi NUL dan byte non-UTF8, `''` vs NULL, NUL di dalam teks,
  dan empat karakter `NULL`)

### Fase 1 — crate inti

- [x] Workspace Cargo (`Cargo.toml`) dengan profil release sesuai ADR-0009
  (`panic = "unwind"` + LTO + strip)
- [x] `crates/qh-core` — `Value` (DECIMAL `i128 + scale`, `Unknown` untuk tipe tak dikenal),
  `render_text` sebagai penerus `to_text`, taksonomi `EngineError` + `FailureKind`
  (klasifikasi retry yang sama dengan `source.py:57`), `unsupported`
- [x] `crates/qh-sql` — scanner SQL satu lintasan (literal, komentar, quoted identifier,
  dollar-quoted), `statement_count`, `strip_terminator`, `count_statement`, `quote_ident`
- [x] **48 test hijau** (23 `qh-core` + 25 `qh-sql`) setelah crate inti pertama,
  `cargo fmt --all --check` bersih, `cargo clippy --workspace --all-targets -- -D warnings` bersih
- [x] `crates/qh-result-store` — store kolumnar: batch kolumnar dengan tabel offset `u32` per
  kolom (akses sel O(1)), satu encoder untuk bentuk in-memory dan spilled, spill ke disk lewat
  `read_exact_at`/`write_all_at` (`std`, tanpa dependency tambahan), file spill dihapus saat
  `Drop`
- [x] **74 test hijau** (29 `qh-core` + 25 `qh-sql` + 20 `qh-result-store`),
  `cargo fmt --all --check` bersih, `cargo clippy --workspace --all-targets -- -D warnings` bersih
- [x] `docs/dependencies.md` dihasilkan dari `cargo metadata` (semuanya MIT/Apache-2.0 →
  konsisten dengan ADR-0002)

### Tahap A — lingkungan uji & baseline

- [x] `deploy/dev/make_seed.py` — generator fixture SQL (deterministik, dijalankan ulang
  menghasilkan berkas identik), bukan SQL yang diketik tangan: 30 kolom × 500.000 baris sulit
  dirawat manual
- [x] `deploy/dev/up.sh` + `deploy/dev/compose.yaml` — PostgreSQL 17 dan MySQL 8.4 berjalan di
  podman, `wide_500k` (500.000 baris × 30 kolom) dan `type_zoo` (zoo tipe §1.8) **terverifikasi
  termuat di kedua engine**
- [x] `deploy/dev/bench_fetch.py` — harness baseline yang dapat diulang; mencatat time-to-first-row
  dari dua anchor, throughput, dan peak RSS lewat `/usr/bin/time -l`
- [x] `docs/benchmarks.md` + `deploy/dev/bench-results.jsonl` — baseline Python terukur untuk
  PostgreSQL dan MySQL; tabel laporan dihasilkan dari JSONL, tidak ada angka yang ditulis tangan
- [x] Tag `python-engine-final` (lihat catatan di bagian tag)

- [x] `crates/qh-driver` — kontrak `Driver`/`Session`/`Cursor`, `Capabilities`, `DriverRegistry`,
  dan `ConnectionConfig` dengan `Debug` tulisan tangan yang tidak pernah mencetak password
- [x] `crates/qh-driver-postgres` — driver PostgreSQL nyata: streaming batch, normalisasi tipe
  yang menjaga DECIMAL presisi penuh, metadata tipe dari `prepare`, dan **cancel yang sampai ke
  server** lewat `CancelRequest` pada koneksi kedua. **26 uji unit + 12 uji integrasi terhadap
  container**, semuanya lulus
- [x] `crates/qh-driver-mysql` — driver MySQL: task produsen + channel berbatas, `KILL QUERY`
  untuk cancel, pembedaan `TEXT`/`BLOB` lewat character set. **28 uji unit + 15 uji integrasi
  terhadap container**, semuanya lulus
- [x] `ColumnBatch::slice_rows` di `qh-core`, supaya batas baris yang diminta pemanggil dihormati
  meskipun produsen membaca dalam batch-nya sendiri
- [x] `docs/compatibility.md` — kebijakan versi hulu dari sumber primer untuk PostgreSQL,
  MySQL, dan Trino, plus versi yang benar-benar teruji (PostgreSQL 17.11, MySQL 8.4.11).
  Catatan penting yang muncul dari riset ini: Trino **tidak punya jaminan antarversi**, jadi
  driver Trino nanti harus diuji terhadap rilis bernomor dan proyek ini tidak boleh mengklaim
  "bekerja dengan Trino" secara umum
- [x] `crates/qh-driver-trino` — protokol klien Trino ditulis sendiri di atas `reqwest`
  (ADR-0006), **24 uji unit + 15 uji integrasi** terhadap Trino **483** nyata. Yang ditemukan
  dengan mengukur, bukan membaca, dan semuanya mengubah kode:
  - **`decimal` datang sebagai string JSON**, sehingga 38 digit bertahan tanpa lewat `f64`.
  - **`varbinary` datang sebagai base64**; meneruskan teksnya sebagai byte akan salah dan baru
    terlihat saat ekspor.
  - **`timestamp`/`time` kehilangan mikrodetik di protokolnya sendiri.** Server melaporkan
    `timestamp(6)` lewat `typeof`, tetapi JSON membawa `.123` dari nilai `.123456`. Itu batas
    hulu yang tidak bisa dipulihkan decoder, dan dicatat karena janji mesin ini adalah tidak
    membulatkan apa pun.
  - **Galat datang di halaman poll, bukan di `POST`** — `POST` menjawab 200 dengan `nextUri`.
  - **`USER_CANCELED` membawa `errorType = USER_ERROR`.** Memetakannya apa adanya akan
    menampilkan "gagal" kepada pengguna yang menekan stop, jadi namanya diperiksa lebih dulu
    dan hasilnya `FailureKind::Cancelled`.
  - **`columns` tidak ada selama `QUEUED`**, jadi `execute` mengembalikan cursor lebih dulu dan
    kolom menyusul di batch pertama. Ini sengaja menyimpang dari kontrak `Cursor`, dan
    alasannya persis pelajaran K11: menunggu di sini berarti menunggu seluruh query, dan
    pemanggil yang belum punya cursor tidak punya apa pun untuk dibatalkan.
- [x] `crates/qh-rt` — runtime dan pemetaan QoS P-core/E-core (ADR-0010), **7 uji unit**.
  Jumlah core dibaca dari `hw.perflevel0.physicalcpu` dan `hw.perflevel1.physicalcpu` lewat
  `sysctlbyname`, bukan dari `available_parallelism()` — yang terakhir itu mengembalikan
  **total** dan menyebut P-core setara E-core, dan itulah kesalahan yang ADR-0010 ada untuk
  mencegah. Runtime utama berukuran jumlah P-core dengan QoS `USER_INITIATED`; pool
  `UTILITY` dan `BACKGROUND` untuk indeks metadata dan spill.
  Yang penting: kelasnya **dibaca kembali** (`pthread_get_qos_class_np`) dari dalam worker
  thread, bukan sekadar diminta. Meminta kelas dan mendapatkannya itu dua hal berbeda, dan
  hanya membaca kembali yang membedakannya — ujinya membuktikan yang kedua.
  Tiga blok `unsafe`, masing-masing dengan `SAFETY:`, dan `unsafe_code` di-`deny` di tingkat
  lint sehingga blok keempat tidak bisa ditambahkan tanpa sengaja. Di mesin non-Apple-Silicon
  crate ini tetap berjalan: pool lambat dapat **1** worker, bukan 0, karena pool tanpa worker
  berarti indeks metadata yang diam-diam tidak pernah berjalan.
- [x] **217 uji hijau** seluruh workspace (dengan PostgreSQL, MySQL, dan Trino nyata), `cargo fmt --all --check` bersih,
  `cargo clippy --workspace --all-targets -- -D warnings` bersih

Yang dibuktikan uji integrasi terhadap server nyata, bukan diasumsikan:

| Bukti | Hasil |
|---|---|
| Cancel sampai ke server | `SELECT pg_sleep(30)` dibatalkan, selesai **di bawah 500 ms** (target §6), error membawa SQLSTATE **57014** (`query_canceled`). Proses Python yang dibunuh tidak bisa menghasilkan ini. |
| Koneksi tetap sehat sesudah cancel | `SELECT 1` sesudahnya berhasil |
| DECIMAL presisi penuh | `1234567890123456789012345678.1234567890` kembali utuh — 38 digit, tanpa `f64` |
| Metadata tipe | `int4`, `text`, `numeric` terbaca dari `prepare` |
| `bytea` berisi NUL dan non-UTF8 | `Value::Bytes([0x00, 0x01, 0xff])` |
| NULL vs string kosong vs kata "NULL" | Tiga nilai berbeda, ketiganya benar |
| Zona waktu | Instant dipertahankan; setelah `SET TIME ZONE`, dirender ulang sesuai zona sesi (D-3) |
| TLS | Tiga mode selain `Disable` **ditolak** dengan pesan yang menyebut TLS, bukan turun ke plaintext |
| Password salah | `FailureKind::Permanent`, dan pesannya tidak memuat password itu |

Yang dibuktikan uji integrasi MySQL terhadap server nyata:

| Bukti | Hasil |
|---|---|
| Pembedaan `TEXT` dari `BLOB` | Keduanya tiba sebagai `MYSQL_TYPE_BLOB`; hanya character set (63) yang membedakan. Tanpa itu string kosong menjadi `Bytes([])` — nilai yang berbeda di grid dan di setiap export |
| DECIMAL presisi penuh | 38 digit utuh, sama seperti PostgreSQL |
| `DATETIME` tidak dikonversi | `2026-01-31 12:00:00.123456` kembali persis; `TIMESTAMP` dikonversi server ke zona sesi |
| Batas baris pemanggil | Produsen membaca 1024 baris per batch, pemanggil minta 250, dan menerima tepat `[250, 250, 250, 250]` |
| Urutan lintas batas batch | id terbaca 1, 2, 3 … lurus menembus beberapa batch produsen |
| `binary(16)` | 16 byte apa adanya, bukan teks hasil decode yang rusak |
| ENUM | Label-nya (`ok`), bukan ordinalnya |
| Cancel | `execute` kembali < 500 ms sehingga cursor sudah ada saat query berjalan, lalu join panjang benar-benar diinterupsi dengan kode **1317**; `SLEEP` mengembalikan **1** sebagai sinyal interupsi |
| Sesudah cancel | `SELECT 1` tetap berhasil — `KILL QUERY`, bukan `KILL CONNECTION` |
| Deskripsi tanpa eksekusi | `prep` mengembalikan 1 kolom dalam 915 µs untuk join yang butuh >90 detik bila dijalankan |
| Durasi suite | **0,33 dtk**, turun dari 30,28 dtk — sekaligus membuktikan query-nya tidak lagi berjalan tuntas di dalam `execute` |
| Database sistem | Disembunyikan kecuali diminta, dan yang disembunyikan tepat empat nama, bukan pola `LIKE` yang bisa menelan database pengguna |
| Level schema | Ditolak dengan petunjuk yang menyebut apa yang harus dipakai |

Temuan nyata dari proses ini, semuanya diperbaiki di kode dan bukan disesuaikan di test:

1. **`statement_count` salah.** Versi pertama menghitung `separators.len()`, yang membuat
   `SELECT 1; SELECT 2` terbaca satu statement. Aturannya sekarang: separator di akhir teks
   adalah *terminator*, separator yang masih punya lanjutan adalah *pemisah*.
2. **`format_interval` salah.** Python tidak mem-pad jam pada `str(timedelta)`
   (`3 days, 4:05:06`, bukan `04:05:06`), dan snapshot memang mencatat bentuk itu.
3. **Parser peak RSS salah.** macOS menulis angka lebih dulu dan label sesudahnya
   (`1234567890  maximum resident set size`); versi pertama mencocokkan label di awal baris,
   sehingga seluruh angka memori keluar `null`.
4. **Anchor time-to-first-row salah.** Menganchor pada event `columns` menghasilkan 1,7 ms —
   menyesatkan, karena event itu baru muncul setelah halaman pertama sudah ada. Anchor yang benar
   adalah `step connect`, yang dikirim engine sebelum menyentuh jaringan.
5. **`split_offset` terlalu rakus.** Versi pertama memindai mundur selama karakter masih
   `[0-9:+-]`, sehingga pada `12:00:00+07` ia menelan seluruh bagian jam dan menyisakan kepala
   kosong — tiga uji timestamp gagal. Sekarang ia mulai dari tanda terakhir dan membatasi offset
   ke rentang nyata (−12:00…+14:00), sehingga `-31` di `2026-01-31` tidak pernah dibaca sebagai
   offset tiga puluh satu jam.
6. **Asumsi tipe kolom MySQL salah.** Versi pertama memetakan `MYSQL_TYPE_BLOB` ke byte. Ternyata
   MySQL melaporkan kolom `TEXT` sebagai `MYSQL_TYPE_BLOB` juga, sehingga kolom teks kosong menjadi
   `Bytes([])` alih-alih string kosong. Diperbaiki dengan membaca character set (63 = binary) —
   nilainya diambil dari `information_schema.COLLATIONS` server, bukan dari ingatan.
7. **`execute` menunggu seluruh query, dan itu membuat cancel mustahil — bukan cancel-nya yang
   rusak.** Versi pertama `execute` menunggu produsen mengirim deskripsi kolom, dan MySQL mengirim
   deskripsi itu saat result set *mulai*; untuk query blocking, artinya saat query *selesai*.
   Terukur: `execute(SELECT SLEEP(2))` kembali setelah **2,002 detik**, dan join panjang belum
   kembali setelah **5 detik** — jadi pemanggil belum memegang cursor, dan tidak ada yang bisa
   dibatalkan. Diperbaiki dengan mendeskripsikan lebih dulu lewat `prep` (COM_STMT_PREPARE):
   query yang sama dideskripsikan dalam **915 µs** dengan 1 kolom.
8. **Klaim saya sendiri yang salah, dicabut.** Saya pernah menulis bahwa cancel MySQL "terbukti
   untuk `SLEEP` (latensi < 500 ms, `SLEEP` mengembalikan 0)". Itu keliru dua kali. Ujinya lulus
   karena `SLEEP(30)` sudah selesai di dalam `execute`, jadi yang diukur adalah pembatalan atas
   sesuatu yang tidak ada. Dan `SLEEP` mengembalikan **1** saat diinterupsi, bukan 0 — **0 berarti
   ia tidur penuh dan kembali normal**. Nilai 0 itulah bukti bahwa tidak terjadi interupsi, dan uji
   lama justru menegaskannya sebagai keberhasilan.
8. **Ekspektasi `timestamptz` salah, kodenya benar.** Uji saya mengasumsikan server
   mengembalikan `+07:00` seperti saat ditulis. PostgreSQL merender di zona waktu **sesi**, jadi
   yang kembali `+00:00` dengan instant yang sama. Diperbaiki dengan membuktikan dua arah:
   instant-nya cocok, dan setelah `SET TIME ZONE 'Asia/Jakarta'` offset-nya kembali `+07:00`.

## Tugas berikutnya (urutan yang dikerjakan)

1. **Menyelidiki K11** — `KILL QUERY` yang tidak menghentikan join panjang. Ini yang paling
   penting dari daftar ini: cancel yang bekerja untuk sebagian query lebih berbahaya daripada
   cancel yang tidak ada, karena UI akan mengklaim sudah berhenti padahal belum. Sekalian
   menjelaskan K12 (30 detik yang tidak dijelaskan di suite MySQL).
2. **Driver Trino** — protokol HTTP, tanpa pool (ADR-0006). Butuh VM podman dinaikkan dulu.
3. **TLS PostgreSQL** (K8) — connector `rustls`, lalu `TlsMode::Prefer`/`Require` benar-benar
   berfungsi alih-alih ditolak. Dijadwalkan bersama `qh-credentials`.
4. `qh-rt` (pemetaan QoS), lalu `qh-export`, `qh-credentials`, `qh-storage`, `qh-tunnel`.
5. **Protocol `DatabaseEngine` di Swift + `MockEngine`.** Dijadwalkan bersama Fase 2, dan
   alasannya dicatat supaya tidak terlihat seperti kelalaian: `AppModel` memakai
   `tab.process?.terminate()` sebagai cancel di 10 titik (`Models/AppModel.swift:243, 758, 809,
   828, 1024, 1098, 1155, 1243`), dan `Engine.terminate` di `App.swift:113`. Protocol yang benar
   menyatakan cancel sebagai kemampuan driver dengan semantik server-side (blueprint §2.7), dan
   itu baru jujur diimplementasikan di atas engine Rust. Protocol ditulis begitu `qh-ffi` ada,
   lalu `AppModel` dipindah dalam satu langkah.
6. **Snapshot golden dari server nyata** (container sudah menyala; tabel `type_zoo` sudah ada di
   kedua engine) untuk menutup K3. Perhatikan D-3: setel zona waktu sesi sebelum merekam.
7. `qh-ffi` + CLI `qh-ffi` setara `preview`, supaya sisi "Rust" di `docs/benchmarks.md` bisa diisi.

## Hasil pengukuran terakhir

Baseline engine Python sudah diukur, dengan `deploy/dev/bench_fetch.py`, pada
`SELECT * FROM wide_500k` (30 kolom, 500.000 baris) terhadap container lokal:

| Kind | Baris pertama (dari connect) | Total | Throughput | Peak RSS |
|---|---|---|---|---|
| postgres | **722 ms** | 4.234 ms | 149.687 baris/s | 546 MB |
| mysql | **3.193 ms** | 6.687 ms | 146.423 baris/s | **1.080 MB** |

Tiga temuan yang mengubah prioritas:

1. **Target memori §6 (800 MB) sudah dilanggar engine Python pada MySQL** — 1.080 MB untuk satu
   hasil 500k × 30. Ini memperkuat keputusan store kolumnar + spill (ADR-0008), bukan sekadar
   optimasi yang bagus dimiliki.
2. **Time-to-first-row 722 ms (PG) dan 3.193 ms (MySQL)**, jauh di atas target < 200 ms. Penyebab
   terukurnya: engine menunggu halaman pertama utuh sebelum mengirim event apa pun. Streaming
   per halaman adalah syarat, bukan penyempurnaan.
3. **Target 5× berarti sekitar 748.000 baris/s** untuk PostgreSQL. Angka absolut ini sekarang
   tercatat supaya tidak ada target yang bisa ditafsirkan mundur setelah implementasi.

Angka mentah ada di `deploy/dev/bench-results.jsonl`; tabel di `docs/benchmarks.md` dihasilkan
dari berkas itu oleh `--report-only`, jadi tidak ada angka yang ditulis tangan.

Engine Rust: **[belum diukur]** — belum punya CLI setara `preview`.

## Known issues

| # | Isu | Dampak | Rencana |
|---|---|---|---|
| K1 | ~~`web_search` tidak tersedia~~ **Selesai** | — | Alat lokal `tools/kenari_search.py` memberi pencarian dan pengambilan halaman. Riset Tahap B kini bisa dikerjakan; §8.1 tinggal diisi, bukan lagi terhalang tooling |
| K2 | ~~Baseline benchmark Python belum ada~~ **Selesai** | — | Terukur untuk PG dan MySQL; Trino tertunda karena memori VM (lihat #4) |
| K3 | ~~Zoo tipe belum diuji terhadap server nyata~~ **Sebagian selesai** | PostgreSQL sudah tervalidasi uji integrasi; MySQL belum | Snapshot server nyata untuk MySQL masuk tugas berikutnya #1 |
| K4 | `tools/deps.py` (referensi di `docs/dependencies.md`) belum ada | Tabel dependency masih dibuat manual | Dibuat bersama job CI `cargo deny` |
| K5 | Trino belum pernah dijalankan | Driver Trino (ADR-0006) belum punya validasi terhadap protokol nyata | [BUTUH TINDAKAN MANUAL] #4 |
| K6 | Ukuran XCFramework belum diukur | `panic = "unwind"` (ADR-0009) menambah unwinding table; konsekuensinya dijanjikan dicatat sebagai angka | Diukur begitu `qh-ffi` menghasilkan artefak |
| K7 | ~~Driver PostgreSQL belum ada~~ **Selesai** | — | Bentuknya ditentukan temuan API di bawah; celah yang tersisa ada di K8 dan K9 |
| K8 | **TLS PostgreSQL belum diimplementasikan** | Driver **menolak** `Prefer`/`Require`/`RequireNoVerify` dengan error yang jelas, jadi tidak ada penurunan senyap ke plaintext — tetapi koneksi yang butuh TLS belum bisa dipakai | Butuh connector `rustls` + root store sistem; dijadwalkan bersama `qh-credentials` |
| K9 | SQL multi-statement ditolak driver | `prepare` mendeskripsikan satu statement; skrip banyak statement gagal dengan pesan yang menyebutkan penyebabnya | Pemanggil memecah dengan `qh-sql::scan`/`strip_terminator`, yang sudah ada dan teruji |
| K10 | ~~Driver MySQL belum ada~~ **Selesai** | — | Rancangannya ternyata bukan extended protocol melainkan task produsen + channel; alasannya di bawah |
| K11 | ~~`KILL QUERY` tidak menghentikan join panjang~~ **Selesai, akar masalahnya bukan cancel** | `execute` menunggu deskripsi kolom, dan MySQL mengirimnya saat result set **mulai** — untuk query blocking, itu berarti saat query **selesai**. Jadi `execute` menunggu seluruh query, pemanggil belum memegang cursor apa pun, dan cancel tidak punya sasaran. Cancel-nya sendiri selalu sehat | Diperbaiki dengan mendeskripsikan lebih dulu lewat `prep` (COM_STMT_PREPARE, tanpa eksekusi). Terukur: `execute(SLEEP(2))` 2,002 dtk → `prep` untuk join yang sama **915 µs** |
| K12 | ~~Suite uji MySQL memakan 30 detik~~ **Selesai** | 30,28 dtk itu adalah `SELECT SLEEP(30)` yang berjalan **tuntas di dalam `execute`** sebelum cancel sempat dipanggil. Penjelasan yang sama dengan K11, dan bukti bahwa uji cancel-nya tidak membuktikan apa pun | Suite kini **0,33 dtk** |

### K7 — temuan API yang menentukan bentuk driver PostgreSQL (kini terjawab)

Dibaca langsung dari sumber crate, bukan dari ingatan:
`~/.cargo/registry/src/*/tokio-postgres-0.7.18/src/simple_query.rs`.

| Fakta | Konsekuensi |
|---|---|
| `Client::simple_query_raw(&self, query: &str) -> Result<SimpleQueryStream, Error>` — `SimpleQueryStream` **tidak punya parameter lifetime** | Bagus, tapi tidak cukup: lihat baris berikutnya. |
| `SimpleColumn` hanya mengekspos **`name()`**; tidak ada aksesor tipe kolom (`src/simple_query.rs:23-32`) | Jalur simple query saja **tidak memberi metadata tipe**. Ini yang memblokir versi pertama. |
| `SimpleQueryMessage::RowDescription(Arc<[SimpleColumn]>)` (`src/lib.rs:260`) | Tetap tidak menolong: `SimpleColumn`-nya sama. |
| `Client::prepare(&self, query) -> Statement`, `Statement::columns() -> &[Column]`, `Column::type_() -> &Type`, `Type::name() -> &str` | **Jalan keluarnya:** satu `prepare` (Parse/Describe, tanpa eksekusi) memberi nama tipe, lalu `simple_query_raw` menjalankan statement dan mengalirkan nilai teks. Satu putaran tambahan + satu parse tambahan di server, tanpa eksekusi ganda. |
| `SimpleQueryStream` **bukan `Unpin`** | Harus di-`Box::pin` sebelum bisa di-poll dari balik `&mut`. |
| `Client` adalah `Clone` dan punya `cancel_token()` (`src/client.rs:721`) | Cancel server-side terpasang seperti direncanakan (ADR-0005). |

Biaya yang diterima secara sadar: karena `prepare` hanya mendeskripsikan **satu** statement, SQL
multi-statement ditolak driver — lihat K9. Alternatifnya (menebak metadata, atau menjalankan
tanpa tipe) sudah dievaluasi dan ditolak: yang pertama menghasilkan grid tanpa type chip, yang
kedua adalah regresi diam-diam dari engine Python.

Versi pertama driver ini **dihapus, bukan dikirim**, karena berjalan di atas simple query saja —
artinya kehilangan tipe kolom yang dilaporkan engine Python. Alasan pencatatan itu ada di commit
`dafd071`.

### K10 — desain driver MySQL, dan bagaimana ia akhirnya dibangun

Dibaca langsung dari `~/.cargo/registry/src/*/mysql_async-0.36.2/`, bukan dari ingatan. Crate
`mysql_async` ter-resolve ke **0.36.2**, jadi versi itu terverifikasi, bukan tebakan.

| Fakta | Konsekuensi |
|---|---|
| `QueryResult<'a, 't: 'a, P>` menyimpan `conn: Connection<'a, 't>` (`src/queryable/query_result/mod.rs:71`) | **Ini yang menentukan desain.** Berbeda dari `SimpleQueryStream` PostgreSQL yang owned, `QueryResult` **meminjam** koneksinya, sehingga `Box<dyn Cursor>` tidak bisa memilikinya selama session masih dipinjam. |
| `QueryResult::next(&mut self) -> Result<Option<Row>>` (`:194`) | Streaming per baris tersedia, tetapi hanya di dalam scope yang meminjam koneksi. |
| `QueryResult::columns() -> Option<Arc<[Column]>>` (`:395`), `columns_ref() -> &[Column]` (`:382`) | **Kabar baiknya: MySQL tidak punya masalah metadata PostgreSQL.** Tipe kolom tersedia langsung dari hasil query, jadi tidak perlu langkah describe terpisah. |
| `Column::name_str() -> Cow<str>` (`mysql_common-0.35.5/src/packets/mod.rs:406`), `Column::column_type() -> ColumnType` (`:335`) | Nama dan tipe kolom bisa dibaca tanpa menebak. |
| `Conn::id() -> u32` (`src/conn/mod.rs:195`) | ID koneksi untuk `KILL QUERY` — mekanisme cancel MySQL (blueprint §2.7). |
| `OptsBuilder`: `ip_or_hostname`, `tcp_port`, `user`, `pass`, `db_name`, `init(Vec<String>)`, `secure_auth`, `stmt_cache_size` | `init` adalah tempat `SET time_zone = ...` bila nanti diinginkan; untuk sekarang zona dibiarkan seperti server memutuskan (lihat D-3). |
| `Conn::new<T: Into<Opts>>(opts) -> BoxFuture<'static, Conn>` (`src/conn/mod.rs:947`) | `BoxFuture<'a, T>` di crate ini berarti `Future<Output = Result<T>>`, jadi pemanggilnya menulis `.await?`. Terverifikasi saat implementasi. |

**Yang akhirnya dibangun:** karena `QueryResult` meminjam koneksi, cursor tidak bisa memilikinya,
jadi `Conn` dipindahkan ke **task produsen** yang mengalirkan `ColumnBatch` lewat channel berbatas
(4 batch). Ini sekaligus memberi backpressure — persis bentuk yang direncanakan di blueprint §2.3
("channel dengan batas, bukan `queue.Queue` + thread manual").

Satu konsekuensi yang tidak langsung terlihat dan harus diingat: `Cursor::columns()` harus sah
segera setelah `execute` kembali, sedangkan kolom hanya diketahui di dalam task. Versi pertama
menjawabnya dengan menunggu pesan pertama dari produsen, dan itu **salah** — catatan aslinya
mengklaim "deskripsi kolom selalu datang sebelum baris apa pun", padahal untuk query blocking
MySQL mengirim deskripsi itu saat query *selesai*. Akibatnya `execute` menunggu seluruh query dan
cancel tidak punya sasaran; terukur 2,002 dtk untuk `SELECT SLEEP(2)`. Perbaikannya (K11):
deskripsi diambil **lebih dulu** dengan `prep` (COM_STMT_PREPARE, tanpa eksekusi, 915 µs untuk join
yang butuh >90 dtk bila dijalankan), di luar task produsen. Produsen hanya mengumumkan kolom
sebagai fallback, untuk statement yang server menolak mendeskripsikannya.

Cancel memakai koneksi kedua yang terpisah, jadi tidak terpengaruh koneksi yang sedang dipinjam
task. **Ini sempat keliru**: lihat K11 di daftar Known issues — yang rusak ternyata bukan cancel,
melainkan `execute` yang menunggu seluruh query karena menunggu deskripsi kolom. Kerangkanya tetap
seperti di atas, tetapi deskripsinya sekarang diambil lebih dulu lewat `prep`, di luar task
produsen.

Crate kerangka MySQL pernah **dihapus dari workspace** alih-alih dibiarkan berisi `// placeholder`
(commit `dafd071`). Itu alasan sesi ini menelusuri API sampai ke sumbernya sebelum menulis satu
baris pun: kerangka kosong yang terlihat seperti pekerjaan belum selesai lebih membingungkan
daripada tidak ada apa-apa.

## Perkakas lokal (sengaja tidak masuk repo)

`tools/kenari_search.py` adalah alat bantu riset saat membangun aplikasi, bukan bagian dari yang
dikirim produk. Karena itu ia **di-gitignore** dan tidak ada di repo — alasannya sama seperti skrip
sekali pakai tidak di-commit: pohon repo seharusnya menggambarkan produknya.

Konsekuensi yang harus diingat: **clone baru tidak punya file ini.** Yang perlu diketahui untuk
membuatnya lagi:

- Memanggil endpoint kenari secara langsung, karena `kenari:web_search` adalah server tool yang
  normalnya dipakai lewat array `tools` pada request chat — tidak berguna bagi skrip atau agent
  yang ingin hasilnya sebagai data.
- Bentuk yang sudah diverifikasi terhadap API hidup, bukan dari dokumentasi saja:
  `POST /v1/web/search` → `{"results":[{"title","url","content"}]}`;
  `POST /v1/web/fetch` → `{"title","content"}`;
  `POST /v1/x/search` → `{"answer","citations"}`, dan saat saldo kurang →
  `{"error":{"code":"plan_limit_reached"}}` dengan **HTTP 429**.
- 429 itu penting: kode terstruktur di body lebih tepat daripada status HTTP. Mengklasifikasikan
  dari status saja membuat batas penagihan terlihat seperti rate limit, dan pemanggil akan
  mengulanginya selamanya. Klien mengecek kode body lebih dulu, dan `rate_limited` asli tetap
  dianggap bisa diulang.
- Key dibaca dari `KENARI_API_KEY`, fallback ke `.env` di akar repo. Keduanya di luar version
  control. `.env` **tidak** di-gitignore sebelum ini — itu diperbaiki bersamaan.

`x_search` belum bisa dipakai: ditagih dari saldo, bukan dari kuota paket, dan saldonya kurang.

## [BUTUH TINDAKAN MANUAL]

1. ~~Kuota paket untuk `web_search` habis.~~ **Teratasi**, dan risetnya sudah mulai dikerjakan.
   §8.1 kini memuat empat issue DBeaver sebagai sumber primer, dengan kutipan dan URL. Yang
   **masih terbuka** dan butuh dilanjutkan:
   - **Navicat**: cari di forum resmi + catatan rilis versinya; produk ini tidak punya issue
     tracker publik di GitHub.
   - **DataGrip**: cari di YouTrack JetBrains (proyek `DBE`); halaman bantuan JetBrains tentang
     startup lambat **tidak** dipakai, karena dokumentasi troubleshooting bukan bukti produknya
     lebih lambat daripada pembanding.
   - ~~**`docs/compatibility.md`**~~ **Selesai**: dokumennya sudah ada, memuat kebijakan versi
     hulu dari sumber primer (PostgreSQL 5 tahun/mayor, MySQL LTS vs Innovation, Trino tanpa
     jaminan antarversi) plus versi yang benar-benar teruji. Yang **masih terbuka di dalamnya**:
     versi *minimum* per mesin — yang tercatat baru versi *teruji*, dan menuliskan "PostgreSQL
     15+" tanpa mengukurnya akan jadi klaim tanpa sumber.
   Sampai selesai, baris yang belum tertutup **tidak** dipakai sebagai dasar prioritas produk,
   dan §8.2 mewarisi batasan yang sama.
2. **`x_search` belum bisa dipakai**: akun kenari menjawab `plan_limit_reached`, karena pencarian X
   ditagih dari saldo dan bukan dari kuota paket. Top up saldo kalau pencarian X memang dibutuhkan;
   `web_search` dan `web_fetch` sudah cukup untuk riset Tahap B.
3. **Rotasi key kenari.** Key sempat ditempelkan ke dalam percakapan, jadi harus dianggap bocor.
   Buat key baru di kenari.id, taruh di `.env` (sudah di-gitignore), lalu hapus yang lama.
4. **Notarisasi & code signing** butuh Apple Developer ID + sertifikat. Skrip dan konfigurasi
   disiapkan pada Fase 4; sampai tersedia, build memakai ad-hoc signing.
5. **Kunci EdDSA Sparkle** untuk update bertanda tangan belum ada dan tidak boleh masuk repo;
   dibuat pada Fase 4 dan disimpan sebagai secret CI.
6. ~~**VM podman hanya punya 2 GiB RAM**~~ **Selesai.** VM dinaikkan ke 4 GiB, dan Trino
   **483** berjalan di `127.0.0.1:58080` — melayani sekitar **sepuluh detik** setelah container
   dinyalakan. Protokol kliennya sudah diverifikasi manual dan empat langkahnya tercatat di
   `docs/compatibility.md`, termasuk satu jebakan yang lebih baik diketahui sekarang:
   **`columns` belum ada selama state masih `QUEUED`**, jadi skema tidak boleh diasumsikan
   datang di respons `POST` pertama.
   **Lanjutannya selesai:** `crates/qh-driver-trino` kini ada dan diuji — 24 uji unit + 15 uji
   integrasi terhadap rilis 483 yang sama. Yang tetap berlaku: Trino tidak memberi jaminan
   antarversi, jadi setiap klaim tentangnya harus menyebut nomor rilis.
