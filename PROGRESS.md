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
| D — Fase 1 | **Sedang dikerjakan** | `qh-core`, `qh-sql`, `qh-result-store` selesai dan hijau; driver, export, credentials, storage, tunnel, FFI belum ada |
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

### Fase 1 — awal

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

### Fase 0 — lingkungan uji & baseline

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

## Tugas berikutnya (urutan yang dikerjakan)

1. **`qh-driver-postgres`, di atas jalur extended protocol.** Rancangan dan alasan mengapa simple
   query tidak bisa dipakai ada di K7 di bawah — baca itu lebih dulu, ia menentukan bentuk
   driver-nya. Modul normalisasi teks→`Value` yang sudah ditulis (belum terkompilasi) dipakai
   bersama driver ini, bukan sebelumya, supaya tidak ada kode mati di antara keduanya.
2. **Protocol `DatabaseEngine` di Swift + `MockEngine`.** Dijadwalkan bersama Fase 2, bukan
   sekarang, dan alasannya dicatat supaya tidak terlihat seperti kelalaian: `AppModel` memakai
   `tab.process?.terminate()` sebagai cancel di 10 titik (`Models/AppModel.swift:243, 758, 809,
   828, 1024, 1098, 1155, 1243`), dan `Engine.terminate` di `App.swift:113`. Protocol yang benar
   menyatakan cancel sebagai kemampuan driver dengan semantik server-side (blueprint §2.7), dan
   itu baru jujur diimplementasikan di atas engine Rust. Menuliskannya sekarang berarti membuat
   adapter yang berpura-pura membatalkan di server padahal hanya membunuh proses — persis P5 yang
   sedang diperbaiki, hanya dipindahkan ke lapisan lain. Protocol ditulis begitu `qh-ffi` ada,
   lalu `AppModel` dipindah dalam satu langkah.
3. `qh-rt` (pemetaan QoS), lalu driver MySQL dan Trino mengikuti bentuk yang sama.
4. `qh-export`, `qh-credentials`, `qh-storage`, `qh-tunnel`.
5. Snapshot golden dari **server nyata** (container sudah menyala; zoo tipe PG dan MySQL sudah
   ada di `deploy/dev/seed-*.sql`) untuk menutup K3.
6. `qh-ffi` + CLI `qh-ffi` setara `preview`, supaya sisi "Rust" di `docs/benchmarks.md` bisa diisi.

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
| K3 | Zoo tipe belum diuji terhadap server nyata | Normalisasi tipe PG/MySQL belum tervalidasi di luar objek Python | Tabel `type_zoo` sudah dimuat di kedua container; snapshot server nyata masuk tugas berikutnya #4 |
| K4 | `tools/deps.py` (referensi di `docs/dependencies.md`) belum ada | Tabel dependency masih dibuat manual | Dibuat bersama job CI `cargo deny` |
| K5 | Trino belum pernah dijalankan | Driver Trino (ADR-0006) belum punya validasi terhadap protokol nyata | [BUTUH TINDAKAN MANUAL] #4 |
| K6 | Ukuran XCFramework belum diukur | `panic = "unwind"` (ADR-0009) menambah unwinding table; konsekuensinya dijanjikan dicatat sebagai angka | Diukur begitu `qh-ffi` menghasilkan artefak |
| K7 | **Driver PostgreSQL belum ada**, dan bentuknya ditentukan oleh temuan API di bawah | Tidak ada driver yang bisa dipakai; `qh-driver` sudah ada tapi belum ada implementasinya | Dikerjakan berikutnya dengan jalur extended protocol |

### K7 — kenapa driver PostgreSQL belum selesai (temuan API yang mengikat desain)

Dibaca langsung dari sumber crate, bukan dari ingatan:
`~/.cargo/registry/src/*/tokio-postgres-0.7.18/src/simple_query.rs`.

| Fakta | Konsekuensi |
|---|---|
| `Client::simple_query_raw(&self, query: &str) -> Result<SimpleQueryStream, Error>` — `SimpleQueryStream` **tidak punya parameter lifetime** | Bagus: cursor bisa memilikinya langsung, tanpa task perantara + channel. Streaming murni tetap mungkin. |
| `SimpleColumn` hanya mengekspos **`name()`**. Tidak ada akses ke tipe kolom (`src/simple_query.rs:23-32`). | **Ini yang memblokir.** Jalur simple query tidak memberi nama tipe, padahal: (a) grid menampilkan type chip dari nama tipe, dan (b) normalisasi tipe→`Value` butuh nama tipe sebagai kunci. |
| `SimpleQueryMessage::RowDescription(Arc<[SimpleColumn]>)` (`src/lib.rs:260`) | Tetap tidak menolong: `SimpleColumn`-nya sama, hanya nama. |
| `Client` adalah `Clone` dan punya `cancel_token()` (`src/client.rs:721`) | Cancel side-server tetap bisa diimplementasikan seperti direncanakan (ADR-0005). |

Alternatif yang dievaluasi, dan kenapa belum diambil: jalur extended protocol (`query_raw`) memang memberi `Row::columns()` → `Column::type_().name()`, jadi ia menyelesaikan masalah nama tipe — tetapi hasilnya dikirim dalam format biner, sehingga setiap nilai harus didekode per tipe konkret. Decoding `numeric` yang presisi penuh lewat jalur itu butuh penanganan `i128 + scale` sendiri, dan itu pekerjaan yang harus dirancang, bukan ditambal.

**Keputusan yang diambil:** crate driver yang setengah jadi **dihapus**, bukan dibiarkan di workspace. Alasannya §4.4: stub dan kode mati di jalur pengguna dilarang. Yang paling penting, driver yang berjalan di atas simple query akan kehilangan metadata tipe — itu **regresi diam-diam** dari engine Python, yang melaporkan tipe kolom, dan regresi diam-diam adalah hal yang justru dilarang dokumen ini. Menghapus lebih jujur daripada mengirim driver yang terlihat bekerja.

Rancangan yang harus dipakai saat melanjutkan: `query_raw` untuk nama tipe, dengan teks sebagai format nilai. Modul normalisasi teks→`Value` sudah ditulis untuk seluruh zoo tipe (`numeric(38,10)` presisi penuh, `timestamptz` ber-offset, `bytea` berisi NUL, interval, json/jsonb) dan berisi 20 uji, **tetapi uji itu belum pernah dijalankan**: crate-nya dihapus sebelum sempat dikompilasi, jadi statusnya belum terverifikasi. Angka "20" adalah jumlah fungsi uji yang ditulis, bukan hasil yang lulus. Ia disimpan untuk sesi berikutnya bersama driver-nya, supaya tidak ada kode mati di antara keduanya.

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
