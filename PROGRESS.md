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
| B — Riset web | **Gagal sebagian** | `web_search` mengembalikan HTTP 402 (kuota paket pengguna habis) → [BUTUH TINDAKAN MANUAL] #1. `web_fetch` **berfungsi**: `sqlx` terverifikasi langsung dari crates.io API (0.9.0, `MIT OR Apache-2.0`, 2026-05-21). Versi dependency lain diverifikasi lewat resolusi Cargo → `docs/dependencies.md`. |
| C — Blueprint + ADR | **Selesai** | `docs/architecture/rust-engine-blueprint.md` §1–§8 + Architecture Decision Summary; ADR 0001–0010 di `docs/decisions/`. |
| D — Fase 0 | **Selesai kecuali protocol Swift** | Golden snapshot ✅, baseline benchmark ✅, tag `python-engine-final` ✅, protocol `DatabaseEngine` + `MockEngine` ❌ (lihat catatan di bawah) |
| D — Fase 1 | **Sedang dikerjakan** | `qh-core`, `qh-sql`, `qh-result-store`, `qh-driver`, dan `qh-driver-postgres` selesai dan hijau; MySQL, Trino, export, credentials, storage, tunnel, FFI belum ada |
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
- [x] **122 uji hijau** seluruh workspace (29 `qh-core` + 10 `qh-driver` + 26 `qh-driver-postgres`
  + 12 integrasi + 20 `qh-result-store` + 25 `qh-sql`), `cargo fmt --all --check` bersih,
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
6. **Ekspektasi `timestamptz` salah, kodenya benar.** Uji saya mengasumsikan server
   mengembalikan `+07:00` seperti saat ditulis. PostgreSQL merender di zona waktu **sesi**, jadi
   yang kembali `+00:00` dengan instant yang sama. Diperbaiki dengan membuktikan dua arah:
   instant-nya cocok, dan setelah `SET TIME ZONE 'Asia/Jakarta'` offset-nya kembali `+07:00`.

## Tugas berikutnya (urutan yang dikerjakan)

1. **Driver MySQL**, mengikuti bentuk `qh-driver-postgres`. Bedanya nyata dan harus dijaga:
   `KILL QUERY` pada koneksi kedua untuk cancel, `caching_sha2_password`, dan `TIMESTAMP` yang
   dirender di zona sesi (lihat D-3).
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
| K1 | `web_search` tidak tersedia (HTTP 402, kuota paket habis) | §8.1 (pain point pesaing) tidak punya sumber; riset Tahap B tidak lengkap | [BUTUH TINDAKAN MANUAL] #1 |
| K2 | ~~Baseline benchmark Python belum ada~~ **Selesai** | — | Terukur untuk PG dan MySQL; Trino tertunda karena memori VM (lihat #4) |
| K3 | ~~Zoo tipe belum diuji terhadap server nyata~~ **Sebagian selesai** | PostgreSQL sudah tervalidasi uji integrasi; MySQL belum | Snapshot server nyata untuk MySQL masuk tugas berikutnya #1 |
| K4 | `tools/deps.py` (referensi di `docs/dependencies.md`) belum ada | Tabel dependency masih dibuat manual | Dibuat bersama job CI `cargo deny` |
| K5 | Trino belum pernah dijalankan | Driver Trino (ADR-0006) belum punya validasi terhadap protokol nyata | [BUTUH TINDAKAN MANUAL] #4 |
| K6 | Ukuran XCFramework belum diukur | `panic = "unwind"` (ADR-0009) menambah unwinding table; konsekuensinya dijanjikan dicatat sebagai angka | Diukur begitu `qh-ffi` menghasilkan artefak |
| K7 | ~~Driver PostgreSQL belum ada~~ **Selesai** | — | Bentuknya ditentukan temuan API di bawah; celah yang tersisa ada di K8 dan K9 |
| K8 | **TLS PostgreSQL belum diimplementasikan** | Driver **menolak** `Prefer`/`Require`/`RequireNoVerify` dengan error yang jelas, jadi tidak ada penurunan senyap ke plaintext — tetapi koneksi yang butuh TLS belum bisa dipakai | Butuh connector `rustls` + root store sistem; dijadwalkan bersama `qh-credentials` |
| K9 | SQL multi-statement ditolak driver | `prepare` mendeskripsikan satu statement; skrip banyak statement gagal dengan pesan yang menyebutkan penyebabnya | Pemanggil memecah dengan `qh-sql::scan`/`strip_terminator`, yang sudah ada dan teruji |

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

## [BUTUH TINDAKAN MANUAL]

1. **Kuota paket untuk `web_search` habis.** Top up kredit paket, lalu jalankan riset Tahap B:
   pain point Navicat/DBeaver/TablePlus/DataGrip/Beekeeper (sumber primer: issue tracker resmi
   masing-masing proyek) dan status dukungan upstream versi PostgreSQL/MySQL/Trino untuk
   `docs/compatibility.md`. Sampai selesai, klaim di §8.1 tetap `[perlu verifikasi]` dan
   **tidak** dipakai sebagai dasar prioritas produk.
2. **Notarisasi & code signing** butuh Apple Developer ID + sertifikat. Skrip dan konfigurasi
   disiapkan pada Fase 4; sampai tersedia, build memakai ad-hoc signing.
3. **Kunci EdDSA Sparkle** untuk update bertanda tangan belum ada dan tidak boleh masuk repo;
   dibuat pada Fase 4 dan disimpan sebagai secret CI.
4. **VM podman hanya punya 2 GiB RAM**, sedangkan Trino single-node butuh sekitar 2 GiB untuk
   dirinya sendiri. Naikkan dulu, lalu jalankan `deploy/dev/up.sh trino`:
   ```bash
   podman machine stop
   podman machine set --memory 8192
   podman machine start
   ```
   Sampai itu dilakukan, driver Trino (ADR-0006) belum punya validasi terhadap server nyata.
