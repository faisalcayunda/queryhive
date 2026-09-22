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
| D — Fase 1 | **Hampir selesai** | `qh-core`, `qh-sql`, `qh-result-store`, `qh-driver`, ketiga driver, `qh-export` (sembilan format + `plan`), `qh-rt`, dan `qh-ffi` (11 perintah + CLI) selesai dan hijau; yang belum: `qh-credentials`, `qh-storage`, `qh-tunnel`, dan permukaan UniFFI di atas `qh-ffi` |
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

### Fase 1 — driver, ekspor, dan entry point

- [x] `crates/qh-driver` — trait `Driver`/`Session`/`Cursor`, `Capabilities`, `ConnectionConfig`
  (yang `Debug`-nya tidak pernah mencetak password), `DriverRegistry`
- [x] `crates/qh-driver-trino` — protokol klien ditulis tangan di atas `reqwest`: `POST
  /v1/statement`, polling `nextUri`, `DELETE` untuk cancel. Tanpa pool (ADR-0006). Kolom boleh
  kosong sampai batch pertama tiba, dan itu didokumentasikan sebagai batas kontrak, bukan bug
- [x] `crates/qh-driver-postgres` dan `crates/qh-driver-mysql` — normalisasi tipe per server,
  cancel sungguhan (`CancelRequest` dan `KILL QUERY`)
- [x] `crates/qh-export` — sembilan writer streaming. Kekuatan klaimnya berbeda per format dan
  ditulis apa adanya: `dbf` **byte-per-byte** terhadap `exporter/writers.py` (keduanya menulis
  byte dengan tangan), `xlsx`/`xls` **terbaca kembali** dengan nilai sel yang sama (`openpyxl`,
  `xlrd`) karena byte-nya milik `openpyxl`/`xlwt` di sisi Python dan tidak mungkin disamakan
- [x] `crates/qh-export/src/plan.rs` — pemecahan part `export_rows`, **byte-per-byte** terhadap
  `exporter/export.py` termasuk penamaan ulang `_part01`
- [x] `crates/qh-export/src/zip.rs` — satu penulis ZIP untuk dua pemakai: part `xlsx` yang
  di-*store* dan `bundle` yang di-*deflate*
- [x] `crates/qh-ffi` — **11 perintah** (`db_drivers|objects|test|catalogs|schemas|tables|export|
  to_table|preview|count|explain`) sebagai library + binary `queryhive-engine`, plus CLI debug
  yang dijalankan harness golden. Panic ditangkap di `main` dan menjadi satu event `error`,
  mengikuti aturan `queryhive_engine.py` bahwa jalur pelaporan error tidak boleh ikut gagal
- [x] **Uji paritas golden di Rust** (`crates/qh-ffi/tests/golden.rs`): sesi palsu menjawab
  `execute`/`browse`/`objects`, persis seperti `record.py` men-drive engine Python in-process.
  **16 kasus identik**, 5 kasus lain terklasifikasi dengan alasannya (lihat
  `docs/golden-deltas.md`), dan sebuah penjaga menolak snapshot baru yang belum diklasifikasi
- [x] **355 uji hijau** di seluruh workspace, termasuk uji integrasi terhadap server nyata
  (Trino 483: 15, PostgreSQL: 13, MySQL: 16), `cargo fmt --all --check` bersih, dan
  `cargo clippy --workspace --all-targets -- -D warnings` bersih

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
- [x] `qh-core::render` — rendering nilai ke teks dan ke JSON (paritas `writers.py:38` dan `:58`),
  **12 uji unit** baru (43 di `qh-core`). Aturannya diambil dari sumber Python dan dari Python itu
  sendiri, bukan dari tebakan: mikrodetik **dihilangkan bila nol** (`12:00:00`, bukan
  `12:00:00.000000`), offset ditulis `±HH:MM`, dan `bytes` menjadi **hex** — bukan base64. Yang
  terakhir itu jebakan yang nyata: Trino mengirim `varbinary` sebagai base64 *di kawat*, sedangkan
  bentuk kanonik yang dilihat pengguna di berkas adalah hex. Dua hal berbeda, dan driver Trino yang
  tadinya punya salinan formatter sendiri sekarang memakai yang ini.
  Tiga hal **sengaja tidak** direproduksi, dan dicatat di modulnya alih-alih dibiarkan ditemukan:
  teks float (Python menulis `1e+30`, Rust menulis angka penuh — keduanya round-trip, tapi berbeda),
  `INTERVAL` (klien `trino` tidak terpasang di sini sehingga tidak ada yang bisa ditanya: **belum
  terverifikasi**, bukan diklaim setara), dan `bytes` di dalam array (Python menulis repr
  `b'\x00\xff'`, di sini hex).
  Satu hal **dipertahankan justru karena lossy**: `to_json_value` mengubah `Decimal` menjadi float,
  persis seperti `writers.py:58` dan "Sama" di blueprint §1.7. Ekspor JSON karena itu kehilangan
  presisi desimal. Saya tidak memperbaikinya diam-diam — mengubahnya akan membuat dua engine
  menghasilkan bentuk yang berbeda, dan bentuk teks yang eksak masih tersedia di writer lain.
- [x] `crates/qh-export` — **6 dari 9 writer** streaming (paritas `writers.py`): `text`, `csv`,
  `json`, `xml`, `html`, `sql`, dengan **24 uji unit**. Tiga format biner (`xlsx`, `xls`, `dbf`)
  **belum ditulis**, dan itu dinyatakan: `Format::implemented()` menjawabnya, `open()`
  mengembalikan galat yang menyebutkan apa yang ada, dan batas baris tipe itu tetap dicatat
  karena angkanya diketahui dari sumber Python (`XLS_MAX_ROWS = 65_536`,
  `XLSX_MAX_ROWS = 1_048_576`). Jadi antarmuka bisa mematikan tiga menu, bukan menawarkannya
  lalu gagal.
  Aturan yang diambil dari sumber, bukan ditebak:
  - **`html.escape`, bukan `xml.sax.saxutils.escape`** — modul Python mengimpor yang pertama,
    dan writer XML memakai fungsi yang sama. Jadi elemen XML menulis `&quot;` dan `&#x27;`,
    yang tidak lazim untuk XML. Dipertahankan: mengubahnya akan membuat dua engine menghasilkan
    bentuk berbeda.
  - `QUOTE_MINIMAL` (kutip hanya bila ada delimiter, quotechar, atau line terminator), kutip di
    dalam digandakan, `\r\n` sebagai line terminator, BOM bila diminta.
  - JSON memakai separator `", "` dan `": "` bahkan tanpa indentasi, dan bentuk array memberi
    awalan dua spasi pada **setiap** baris.
  - `render_rows` hanya untuk format yang barisnya mandiri — `text`, `csv`, `xml`, `html`. Itu
    tepat empat format yang Python tandai `renderable = True`; `json` dan `sql` membawa status
    per baris. Sebuah uji menegaskan bahwa merender per potongan menghasilkan byte yang sama
    dengan merender semuanya sekaligus.
  Dua bug nyata, keduanya ditangkap uji:
  - **`serde_json::Map` mengurutkan kunci.** Kolom ekspor keluar **urut abjad**, bukan urutan
    query — ekspor yang salah dengan cara yang terbaca sebagai benar. `serde_json` kini memakai
    fitur `preserve_order`.
  - Uji paralel menulis berkas temp yang sama karena namanya hanya dari format, sehingga uji
    membandingkan hasil uji lain.
  Yang **belum**: opsi `encoding` (crate ini menulis UTF-8 saja — writer yang diam-diam
  mengonversi lebih buruk daripada yang mengaku tidak bisa) dan `qh-export::plan` dari
  `export.py:60-138`, yang belum dibaca sehingga belum ditulis.
- [x] `qh-export`: format **`dbf`** ditulis tangan, dengan **paritas byte** terhadap engine Python —
  **261 uji hijau** seluruh workspace. Ini paritas yang bisa dibuktikan, bukan diklaim: `writers.py`
  hanya memakai stdlib, jadi saya menjalankannya langsung (dimuat lewat `importlib` karena
  `exporter/__init__.py` menarik `trino` yang tidak terpasang) dan membandingkan keluarannya
  byte per byte. Tiga byte tanggal di header di-mask, karena keduanya membaca jam.
  Bentuk berkas yang dipaksakan formatnya: record dibatasi **4000 byte**, jadi lebar kolom teks
  direncanakan lebih dulu — kolom tetap dijumlahkan, sisanya dibagi rata. Query dengan seratus
  kolom teks menghasilkan kolom sempit, bukan berkas yang tidak bisa dibuka.
  Dua aturan yang mengejutkan dan sudah dikunci uji:
  - `decimal(10,2)` ditulis dengan **enam** desimal (`1.500000`). Aturan Python memberi 6 desimal
    untuk semua tipe non-integer dan **tidak** melihat skala yang dideklarasikan.
  - **cp1252**: `U+0000-U+007F` dan `U+00A0-U+00FF` memetakan ke dirinya sendiri, dua puluh tujuh
    karakter pungtuasi menempati `0x80-0x9F` — sehingga **`U+0080-U+009F` justru tidak bisa
    dikodekan** dan Python menjawab `?`. Saya sempat mengira rentang itu identitas; cek ke Python
    membuktikan sebaliknya.
  Satu **penyimpangan yang disengaja**: Python hanya memeriksa batas record ketika kolom teks
  memaksa perhitungan anggaran, jadi 255 kolom numerik (255 x 20 = 5100 byte) akan menulis berkas
  yang ditolak dBase. Di sini ditolak dengan alasan dan saran. Ujinya ada supaya keputusan itu
  tetap disengaja.
  Dua bug di uji saya sendiri: terminator header ada di `32 + 32 x jumlah field` sementara flag
  hapus ada **di dalam** record (bukan sebelumnya), dan uji NULL menegaskan kosong di field yang
  sebenarnya berisi `-0.25`. Yang ketiga adalah pola yang sama seperti sebelumnya — uji paralel
  menulis berkas temp yang sama.
  **Saat itu `xlsx` dan `xls` masih belum ditulis**, dan alasannya beda jenis: byte keduanya ditentukan
  `openpyxl` dan `xlwt`, jadi menyamakannya tidak mungkin dan tidak bermakna. Yang bisa
  dijanjikan di sana adalah berkas yang sah dengan nilai sel yang sama — klaim yang lebih lemah
  dan berbeda, dan itu akan dinyatakan begitu.
- [x] `qh-export`: format **`xlsx`** ditulis tangan — kontainer ZIP dan bagian OOXML-nya.
  Saat itu **8 dari 9 format** dan **273 uji hijau**; `xls` menyusul di entri bawah.
  ZIP-nya tanpa dependency: entri *stored*, dan yang penting — **lembar kerjanya di-stream
  dengan data descriptor** (flag bit 3), karena CRC dan ukuran sebuah entri stored baru diketahui
  setelah byte terakhirnya ditulis. Itulah yang menjaga memori tetap datar.
  Dua keputusan yang dinyatakan, bukan disembunyikan:
  - **Timestamp berzona menjadi teks.** Sel tidak bisa membawa offset, dan menulis instannya
    tanpa zona adalah konversi diam-diam.
  - **Timestamp ditulis sebagai serial Excel dengan number format**, bukan sebagai teks. Serial
    tanpa format tampil sebagai `45922` — lebih buruk daripada teks — jadi `styles.xml` ada
    justru untuk itu. Serial `46053 + 0.5` saya verifikasi memang jatuh di 2026-01-31 12:00.
  Verifikasi berlapis, dan yang terakhir bukan kode saya sendiri:
  1. Uji Rust membaca kembali ZIP-nya lewat central directory dan memeriksa **CRC** setiap entri
     terhadap datanya.
  2. Python `zipfile.testzip()` + `ElementTree.parse()` atas **19 berkas yang dihasilkan**:
     semua bagian bisa diparse, setiap bagian dideklarasikan di `[Content_Types].xml`, setiap
     `Override` benar-benar ada, relasi menunjuk berkas yang ada, dan setiap indeks gaya sel
     ada di `cellXfs`. 19 valid, 0 rusak.
  3. **openpyxl 3.1.5 — pustaka yang sama yang dipakai engine Python — membaca berkasnya** dan
     mengembalikan nilai yang benar: timestamp kembali sebagai `datetime(2026,1,31,12,0)`
     (bukan angka), `1.5` sebagai angka, spasi tepi **tetap utuh** (itu yang dibuktikan
     `xml:space="preserve"`), karakter kontrol hilang, dan NULL menjadi `None`.
  Satu bug nyata ditangkap uji: referensi sel di baris header ditulis `r="A"` **tanpa nomor
  baris** — berkas yang Excel tolak. Penyebabnya saya sendiri, saat menghapus parameter yang
  tampak tidak terpakai.
  Cap baris 1.048.575 (batas 1.048.576 termasuk header) ditegakkan dengan galat, bukan ditulis
  lalu menghasilkan lembar yang ditolak Excel. Stempel waktu DOS di ZIP dipatok 1980-01-01 supaya
  keluarannya reproducible dan uji bisa membandingkan berkas utuh.
- [x] `qh-export`: format **`xls`** — kontainer OLE2 dan record BIFF8, ditulis tangan.
  **9 dari 9 format kini bisa ditulis.** **283 uji hijau.**
  Verifikasi di sini berbeda jenisnya dari `dbf`. Byte `xls` ditentukan `xlwt` di engine
  Python, jadi menyamakannya tidak mungkin dan tidak bermakna; yang saya klaim adalah
  berkas yang **dibaca `xlrd` dengan nilai sel yang sama**, dan saya benar-benar
  melakukannya: `xlwt` 1.3.0 + `xlrd` 2.0.2 di venv terpisah
  (`python3 -m venv /tmp/qh-xls-venv && /tmp/qh-xls-venv/bin/pip install xlwt xlrd`).
  Yang terbukti lewat pembaca luar:
  - Tanggal kembali sebagai **ctype 3 — tanggal sungguhan**, bukan `46053`. Ini hasil
    keputusan menulisnya sebagai sel NUMBER dengan XF ber-format tanggal, bukan record
    FORMULA, dan itulah alasan record FORMULA tidak diperlukan sama sekali.
  - String 9000 karakter utuh di selnya, melintasi batas record. Jadi CONTINUE bekerja,
    termasuk aturan yang paling mudah salah: **bila batas itu jatuh di tengah karakter,
    record lanjutan harus mengulang byte `options`** string tersebut. Aturan itu bukan
    kesimpulan saya — saya bacanya di `unpack_SST_table` xlrd baris 1448, lalu diuji.
  - Offset zona waktu **selamat** (`2026-01-31 12:00:00+07:00`): sel tidak bisa membawa
    offset, jadi instannya tetap teks.
  - Nama sheet dipotong ke 31 karakter seperti yang dilakukan engine Python.
  **Memori rata tanpa trik**: `xlwt` memadatkan stream Workbook ke tepat 4096 byte —
  ambang mini-stream — sehingga tabel alokasi mini tidak pernah diperlukan. Ini bagian
  paling berbelit dari OLE2, dan bisa dihindari seluruhnya.
  **Penyangga seluruh sheet itu bawaan format, bukan jalan pintas.** BIFF8 tidak punya
  mode streaming: sel merujuk tabel string bersama lewat indeks, jadi semua string harus
  diketahui sebelum baris pertama ditulis. Engine Python menyangga karena alasan yang
  sama, dan itulah sebabnya `max_rows` ada dan ekspor dipecah jadi bagian.
  Tiga bug, dan **dua di antaranya hanya bisa ditangkap pembaca luar** — keduanya
  menghasilkan berkas yang tampak baik-baik saja sampai ada yang mencoba membukanya:
  1. Record STYLE harus bentuk **built-in** (bit 15 field pertama diset). Tanpa itu xlrd
     mengiranya style buatan pengguna dan mencari nama string yang tidak ada.
  2. BOUNDSHEET butuh **byte flag nama** setelah panjangnya. Tanpa itu xlrd membaca huruf
     pertama nama sebagai flag dan mencoba mendekode sisanya sebagai UTF-16.
  3. Entri root direktori saya memberi **`left=0, right=0`** — artinya "entri nol", yang
     bagi root adalah dirinya sendiri, sehingga xlrd rekursi tanpa henti. Nilai yang benar
     untuk "tidak ada saudara" adalah `0xFFFFFFFF`. Bug ini muncul sebagai
     `RecursionError` di 31 berkas sekaligus, dan tidak ada uji saya sendiri yang
     menangkapnya.
  Ditambah dua bug di uji saya sendiri: kolom ada di byte 2–4 record NUMBER dan XF di
  4–6 (saya mencampurnya), dan helper `records()` berhenti di EOF pertama — yang
  merupakan akhir **globals** — sehingga record worksheet tidak pernah terbaca.
  Satu keputusan untuk mengurangi risiko: **RK tidak dipakai** meski `xlwt` memakainya.
  Saya dua kali salah menurunkan pengkodeannya dari byte; NUMBER menulis f64 apa adanya,
  empat byte lebih besar per sel, tanpa tebakan.
- [x] `qh-export`: **`plan`** — orkestrasi pemecahan bagian, dari `export.py:60-138` yang
  sebelumnya belum pernah dibaca.
  Aturannya: batas baris per berkas adalah **yang terkecil** antara batas format itu sendiri
  (`xls` 65 535, `xlsx` 1 048 575) dan `rows_per_file` milik pemanggil. Bagian pertama bernama
  apa yang diketik pengguna (`report.csv`); begitu bagian kedua dibuka, yang pertama **diganti
  nama** jadi `report_part01.csv` dan yang baru jadi `part02`. Pengguna yang dapat satu berkas
  dapat persis nama yang ia tulis, dan yang dapat empat dapat himpunan bernomor tanpa nomor yang
  hilang — alternatifnya, `report.csv` lalu `part02`, terbaca seolah `part01` lenyap. Cancel
  ditanyakan **sebelum** baris berikutnya diambil dan bagian yang sedang ditulis tetap ditutup,
  jadi ekspor yang dibatalkan adalah berkas sah berisi baris yang sempat datang — bukan CSV
  dengan baris terakhir yang robek.
  Verifikasinya tidak memakai aturan di atas sebagai patokan. Uji baru `plan_parity` memuat
  `exporter/export.py` lewat `importlib` (dua impor yang tidak dibutuhkan `export_rows`
  di-stub), menjalankan `export_rows` milik engine itu sendiri atas baris yang sama, lalu
  membandingkan direktorinya dengan milik crate ini **nama per nama dan byte per byte**. Lima
  kasus — pecah jadi 6 bagian, satu berkas, hasil kosong, format lain, satu baris per berkas —
  semuanya sama persis.
  Membaca sumber kebenaran itu menemukan **tiga tempat kosakata crate ini sudah menyimpang**,
  dan ketiganya sudah diperbaiki:
  1. Batas `xlsx` adalah `XLSX_MAX_ROWS - 1` = **1 048 575**, bukan 1 048 576. Satu baris terlalu
     longgar, dan baris itu persis bedanya antara pecah dengan rapi dan ekspor yang ditolak
     writer.
  2. Kunci format di engine adalah **`txt`**, bukan `text` — dan kunci itu yang dibawa preferensi
     tersimpan, flag, atau parameter URL. `name()` kini menjawab `txt`, dan `parse` menerima
     `text` juga karena itu yang diketik orang.
  3. `WRITERS` menaruh **`xls` sebelum `xlsx`**. Urutan format adalah urutan yang dilihat di menu.
  Satu hal yang **tidak** direproduksi: jalur paralel `export.py:96` yang me-render baris di
  thread. Jalur itu ada untuk mengakali GIL; `render_workers()` mengembalikan 1 pada build CPython
  biasa, jadi jalur bawaan engine pun sekuensial — dan di sini pekerjaan yang sama sudah berjalan
  dalam puluhan nanodetik per baris, jadi tidak ada yang perlu diakali. `render_rows` tetap publik
  kalau suatu hari profiling berkata lain.
- [x] **301 uji hijau** seluruh workspace (dengan PostgreSQL, MySQL, dan Trino nyata), `cargo fmt --all --check` bersih,
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

### Verifikasi independen `qh-credentials` + `qh-storage`, dan tiga cacat yang ditemukannya (22 Sep 2026)

Langkah ini dikerjakan agen verifier terpisah, yang tidak menulis kode dan tidak boleh mengubah apa
pun — tugasnya mencoba **membuktikan klaimnya salah**, bukan membacanya. Hasilnya bukan sertifikat
kosong: tiga cacat nyata, semuanya di jalur yang tidak disentuh uji yang ada.

Metodenya patut dicatat karena itu yang membuat temuannya berarti: ia membangun harness SQLite
sementara sendiri di luar repo dan mereproduksi tiap klaim dari nol, bukan menjalankan uji kami lalu
menyimpulkan "lulus berarti benar". Termasuk membuat migrasi kedua gagal dengan menyiapkan sebuah
*view* bernama `legacy_import` — bukti langsung bahwa satu transaksi per versi, karena `user_version`
dan riwayatnya tetap di 1 sementara tabel dari migrasi pertama masih utuh.

| Temuan | Akibat nyatanya | Perbaikan |
|---|---|---|
| Database dengan `user_version` tapi **tanpa tabel `schema_migration`** hanya menghasilkan error mentah SQLite `no such table` | Pesan yang tidak menjelaskan apa pun kepada pengguna | Sekarang penolakan yang sama dengan ketidakcocokan penanda lain, beserta alasannya |
| Berkas yang isinya **hanya sebuah view** (tanpa tabel) tetap dimigrasi di atasnya | Pengambilalihan diam-diam; kebetulan tanpa kehilangan data | Pemeriksaan diperluas dari `type='table'` menjadi objek apa pun — indeks atau trigger pun tidak mungkin ada di berkas buatan kami sebelum migrasi pertama |
| Dua baris dalam satu `connections.json` dengan **UUID yang sama** membuat impor tak pernah selesai | Keduanya ditulis di bawah satu id, verifikasi gagal terhadap salah satunya, penanda tidak pernah ditulis, jadi tiap peluncuran menyalin berkas lagi dan menulis ulang baris pertama — lingkaran tanpa ujung | Baris kedua ditolak **di dalam plan**, dengan menyebut indeks baris pertama yang telah memakainya, sehingga pengguna melihatnya dan impor selesai |

Verifier juga menunjukkan klaim yang **dinyatakan tapi tidak diuji**, yang sama berbahayanya dengan
cacat: kegagalan cadangan, cabang verifikasi yang gagal, kegagalan di tengah migrasi, dan referensi
`group_id` yang menggantung. Keempatnya sekarang punya uji — cabang verifikasi yang gagal bahkan
lewat fungsi terpisah (`verify_written`), justru karena cabang itu tidak bisa dicapai lewat pintu
depan, dan itulah alasan ia harus punya uji. Uji `qh-storage` naik dari 28 ke 36.

Satu klaim yang **jujur belum bisa ditutup**, dan verifier menyebutnya apa adanya: bahwa aplikasi
sungguhan membaca item yang ditulis engine ini. Kesetaraan atributnya sudah dibuktikan dari sumber
kedua program, tapi pembacaan lintas-program memunculkan dialog izin Keychain — batas yang memang
tidak bisa dilewati tanpa manusia. Itu tetap tugas manual.

### Seam `DatabaseEngine` di aplikasi, dan di mana ia akan terasa canggung bagi `RustEngine` (22 Sep 2026)

Protokolnya sekarang ada (`app/Sources/TrinoExporter/Support/DatabaseEngine.swift`) dengan satu
konformer, `PythonEngine`, dan **kesembilan call site engine plus jalur terminasi aplikasi** sudah
melewatinya. Bentuknya sengaja bentuk yang sudah ada hari ini — perintah, environment, callback
event, callback exit — bukan permukaan bertipe `descriptors()/browse()/run()` di §1.6, karena
menjanjikan yang terakhir berarti menjanjikan operasi yang belum bisa dilakukan implementasi mana pun
di pohon ini. `swift build` dan `swift build -c release` lolos; `swift test` tidak ada targetnya,
jadi tidak ada uji yang ditambahkan alih-alih membuat target uji untuk satu refactor.

Yang **sengaja tidak** dikerjakan: error type §4.4. Kegagalan berjalan sebagai exit status plus string
stderr, dan tujuh call site mengambil pesannya dari baris terakhir log itu. Error type dengan medan
`code` dan `position` yang tidak bisa diisi siapa pun adalah placeholder yang §4.4 larang. Itu
keputusan yang benar, dan yang membuatnya benar adalah pengukuran: mesin Python hanya memancarkan
`{"event":"error","message":…}` tanpa code maupun posisi SQL sama sekali.

Enam hal yang akan terasa canggung saat `RustEngine` menggantikannya — dicatat sekarang supaya Fase 2
tidak menemukannya satu per satu:

1. **`onExit(status, stderr)` adalah tepi paling kasar.** Sukses berarti status 0, gagal berarti kode
   keluar proses plus log teks, dan tujuh call site mem-parse baris terakhir log itu. Rust/UniFFI
   tidak punya exit status maupun stderr per operasi — ia punya `Result` bertipe. Callback inilah yang
   harus diganti lebih dulu, dan itulah sebabnya error type §4.4 tidak bisa dipasang belakangan tanpa
   menyentuh setiap jalur kegagalan di `AppModel`.
2. **`command: String` + `env: [String: String]` adalah CLI NDJSON, bukan API domain.** Bentuk
   environment itu ada semata-mata karena setting (termasuk password) harus lewat environment agar
   tidak muncul di `ps` (§1.2). FFI menghapus kendala itu.
3. **`EngineRun` menyatukan cancel dan release.** Bagi Python keduanya `Process.terminate()`;
   §1.6 memisahkan `cancel` dari `release`, jadi protokol ini wajar tumbuh.
4. **`terminateAll()` berbentuk siklus hidup proses.** Engine Rust memegang sesi, bukan anak proses.
5. **Data plane belum ada, dan itu benar.** Mesin sekarang menyerahkan seluruh hasil sebagai
   `[[String?]]` di dalam event, jadi menjanjikan window sekarang berarti menjanjikan yang tidak bisa
   ditepati.
6. Protokolnya masih terikat pada `Event`, yaitu tipe kawat NDJSON.

### `qh-tunnel`: tunnel SSH, `known_hosts`, dan cacat di dependensinya sendiri (22 Sep 2026)

Crate-nya ada dan hijau: 21 uji unit + 8 uji terhadap `sshd` sungguhan di container
(`deploy/qh-sshd-run.sh`, port 52222). Yang dibangun adalah `known_hosts` sendiri — parsing,
pencocokan nama host, entri ter-hash, wildcard dan negasi, `@revoked`, `@cert-authority` —
lalu `russh` untuk transportnya, dengan verifikasi host key **sebelum autentikasi** dan TOFU
sebagai **dua panggilan** (laporkan "host tidak dikenal beserta fingerprint-nya", lalu terima
kalau manusia bilang ya), bukan prompt yang memblokir di dalam library.

Bukti yang membuatnya layak dipercaya, bukan sekadar "lulus":

- **Pencocokan entri ter-hash diperiksa terhadap OpenSSH, bukan terhadap bacaan kami sendiri.**
  Ujinya menulis berkas dengan `ssh-keygen -H`, lalu membandingkan tiap putusan dengan exit status
  `ssh-keygen -F`. Ini penting karena `HashKnownHosts` lazim menyala, dan pemeriksa yang diam-diam
  mengabaikan entri ter-hash akan memunculkan prompt "host tidak dikenal" untuk host yang sudah
  pernah diterima pengguna — yaitu melatih orang mengklik tembus satu-satunya prompt yang
  melindungi mereka.
- Fingerprint-nya cocok dengan vektor `ssh-keygen` (`SHA256:ldyiXa1J…`).
- Forward-nya membawa bytes **dua arah** melalui bastion: banner OpenSSH dibaca kembali lewat
  tunnel, lalu baris identifikasi kami dituliskan lewat tunnel yang sama.
- Sebuah baris `@revoked` **tidak bisa** ditembus TOFU, dan urutan baris tidak menentukan: baris
  `Matched` di atas baris `@revoked` untuk blob yang sama tetap `Revoked`.

**Temuan yang memaksa kami menulis sendiri, bukan memilih:** modul `known_hosts` milik `russh`
0.63 — yang sengaja tidak kami pakai — mengabaikan baris `@revoked` sepenuhnya. Saya verifikasi
sendiri di sumber crate-nya: kata `revoked` tidak muncul sekali pun di `src/keys/known_hosts.rs`.
Akibatnya sebuah kunci yang hanya ada sebagai `@revoked` terbaca "tidak dikenal", dan alur TOFU
akan menerimanya kembali — persis kebalikan dari maksud baris itu. Dua cacat lain di modul yang
sama: `learn_known_hosts_path` membuat berkasnya tanpa mode 0600, dan satu baris base64 yang
rusak membatalkan seluruh pemeriksaan. Ketiganya ditangani implementasi kami.

Dua penyimpangan yang disengaja, keduanya dicatat di doc crate-nya: `russh-keys` tidak dipakai
sebagai dependensi terpisah karena di `russh` 0.63 kode kunci ada di `russh::keys`, dan menambahkan
`russh-keys` 0.49 akan menaruh dua versi `ssh-key` (0.6 dan 0.7-rc) dalam satu pohon dengan tipe
yang tidak sama; dan sertifikat host **ditolak dengan jelas**, bukan diabaikan.

Yang **belum** teruji dan disebut apa adanya: jalur "kunci agen diterima" — jalur
connect/identities/sign-nya berjalan, tapi container ini tidak menerima kunci agen mesin ini.

### TLS: satu keputusan untuk tiga driver, dan satu driver yang tidak bisa ikut (22 Sep 2026)

Tiga agen mengerjakan TLS di tiga crate terpisah. Yang mereka **tidak** boleh putuskan sendiri
adalah semantik `Prefer`, dan itu terbukti benar: agen PostgreSQL memilih `Prefer` memverifikasi
sertifikat, dengan alasan yang masuk akal (verifikasi memberi gigi pada aturan "handshake gagal
bukan alasan untuk jatuh ke plaintext"). Saya **membalikkannya**, karena alasan yang lebih
menentukan: psycopg dan pymysql tidak memverifikasi di `prefer`, jadi memverifikasi di situ akan
memutus setiap koneksi yang hari ini bekerja ke server internal bersertifikat sendiri — dengan
error yang hanya menyebut "TLS handshake". Aturan yang penting tetap utuh: hanya
`NoClientSslFlagFromServer` yang boleh memicu percobaan ulang tanpa enkripsi, dan handshake yang
rusak tetap error. Keputusan itu dikirim ke dua agen yang masih berjalan sebagai instruksi, bukan
diserahkan pada tebakan masing-masing, sehingga tiga driver berperilaku sama.

Konsekuensi tak terhindarkan yang harus dicatat: kosakata `sslmode` milik aplikasi adalah milik
libpq, yang artinya **tidak** sama dengan nama enum kami. libpq `require` mengenkripsi **tanpa**
memverifikasi; hanya `verify-ca`/`verify-full` yang meminta sertifikat diperiksa. Karena itu
pemetaannya sekarang: `disable`→`Disable`, `prefer`/kosong→`Prefer`, `require`→`RequireNoVerify`,
`verify-ca`/`verify-full`→`Require`. Sebelumnya `require` memetakan ke mode yang memverifikasi —
artinya upgrade akan menolak server internal bersertifikat sendiri bagi semua orang yang memakai
`require`. Ejaan yang tidak dikenal sekarang **ditolak**, bukan diam-diam menjadi default: salah
ketik pada `sslmode` tidak layak menentukan apakah koneksi dienkripsi tanpa memberi tahu siapa pun.

**Perbedaan antar-driver yang tidak bisa dihilangkan hari ini.** PostgreSQL dan (kemungkinan)
Trino memakai `rustls-platform-verifier`, sehingga CA korporat yang dipasang pengguna di Keychain
langsung dipercaya. **MySQL tidak bisa**: `mysql_async` 0.36 (dan 0.37.1) menyimpan
`build_tls_connector` serta cached connector sebagai `pub(crate)`, dan satu-satunya penyetel root
store mengambil tipe yang tidak di-re-export — probe-nya gagal dengan `E0603`. Jadi untuk MySQL
`Require` memverifikasi terhadap root bawaan saja, dan pengguna dengan CA korporat mendapat
**penolakan**, bukan koneksi terpercaya. Agennya menolak mengakalinya dengan verifikasi di muka,
dan itu keputusan yang benar: memverifikasi lebih dulu lalu menyambung dengan klien yang tidak
memeriksa adalah lubang TOCTOU. Menutupnya butuh hook dari upstream (atau fork) **plus** medan
`ssl_ca` di `ConnectionConfig` — pekerjaan tersendiri, bukan tambalan.

Satu celah di sisi aplikasi yang ditemukan dari sini: `Connections.swift:97-105` hanya memberi
MySQL pilihan `disable`/`require` dengan bawaan `disable`, sedangkan driver-nya berdefault
`Prefer`. Jadi mode bawaan driver tidak bisa dipilih dari picker — bukan peta yang hilang,
melainkan kosakata UI yang belum punya kata untuk itu.

## Tugas berikutnya (urutan yang dikerjakan)

Daftar ini diperbarui 23 Sep 2026. Sembilan item sebelumnya sudah selesai -- termasuk K11 dan K12
yang justru bukan soal cancel sama sekali, melainkan `execute` yang menunggu deskripsi kolom
sehingga query blocking selesai sebelum pemanggil memegang cursor apa pun. Yang tersisa di bawah
ini adalah yang benar-benar belum.

1. **Empat workstream paritas, satu agen per crate** (berkas terpisah, jadi paralel):
   - `crates/qh-driver-trino` — header `X-Trino-Client-Capabilities` yang tidak pernah dikirim,
     sehingga server menurunkan presisi timestamp menjadi milidetik. Yang membuat ini mendesak
     bukan presisinya, melainkan **uji integrasi di crate itu yang mengabadikan pemotongan
     tersebut sebagai kebenaran protokol**: uji yang salah lebih berbahaya daripada tidak ada uji,
     karena ia membuat perbaikan terlihat seperti regresi. Sekaligus `explain` yang tidak membuang
     `;` milik pemanggil.
   - `crates/qh-driver-postgres` — `catalogs` ditolak padahal mesin lama menjawabnya; itu
     satu-satunya cacat paritas nyata di crate itu. Tiga selisih lain (interval, array, uuid)
     arahnya **kebalikan** dari dugaan pertama: sisi Rust yang lebih setia (psycopg melipat
     interval menjadi hari dan menambahkan tanda kutip yang tidak diminta), jadi yang dibutuhkan
     keputusan tertulis, bukan perubahan.
   - `crates/qh-driver-mysql` — kolom `ENUM` terbaca `char` (keputusan parent, karena `254` berarti
     `CHAR` **dan** `ENUM`), dan pesan penolakan level yang kehilangan petunjuk yang dulu ada.
   - `crates/qh-ffi/src/config.rs` — `DB_SSLMODE` diabaikan untuk Trino, dan `Prefer` tidak punya
     sumber sama sekali dari setelan Trino.
2. **`RETRIES` dibaca lalu diabaikan.** Retry adalah milik lapisan session (blueprint §1.7), yang
   belum ada. Ini perbedaan perilaku nyata pada koneksi yang putus di tengah ekspor, jadi dicatat
   dan bukan disembunyikan.
3. **`ENCODING` dan `DBF_ENCODING`** juga dibaca lalu diabaikan: semua writer engine ini UTF-8 dan
   code page `dbf` tetap cp1252.
4. **Protocol bertipe + `MockEngine` di aplikasi (Fase 2).** Seam-nya sudah ada
   (`app/Sources/TrinoExporter/Support/DatabaseEngine.swift`, satu konformer `PythonEngine`,
   sembilan call site melewatinya); yang belum adalah permukaan `descriptors()/browse()/run()`
   bertipe §1.6 beserta `MockEngine`-nya, dan data plane handle + offset buffer (ADR-0004). Enam
   tepi yang akan terasa canggung sudah dicatat di riwayat di atas, dimulai dari
   `onExit(status, stderr)`.
5. **Celah cakupan golden: `export` dan `to_table` live di PostgreSQL dan MySQL.** Boleh
   ditinggalkan, tapi harus sebagai pilihan: stdout keduanya memuat path dan ukuran, bukan nilai,
   jadi yang dibekukan sedikit.
6. **Yang butuh tangan manusia, bukan agen:** rotasi kunci kenari, dan pembacaan lintas-program
   item Keychain yang ditulis engine ini -- pembacaan dari aplikasi memunculkan dialog izin, dan
   itu batas yang tidak bisa dilewati tanpa orang.

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
