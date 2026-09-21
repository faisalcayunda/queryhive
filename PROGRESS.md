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
| D — Fase 0 | **Hampir selesai** | Golden snapshot ✅, protocol `DatabaseEngine` + `MockEngine` ❌, baseline benchmark ❌, tag `python-engine-final` ❌ |
| D — Fase 1 | **Dimulai** | Workspace Cargo + `qh-core` + `qh-sql` selesai dan hijau; sisa crate belum ada |
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
- [x] **48 test hijau** (23 `qh-core` + 25 `qh-sql`), `cargo fmt --all --check` bersih,
  `cargo clippy --workspace --all-targets -- -D warnings` bersih
- [x] `docs/dependencies.md` dihasilkan dari `cargo metadata` (6 paket transitif, semuanya
  MIT/Apache-2.0 → konsisten dengan ADR-0002)

Dua temuan nyata dari proses ini, keduanya diperbaiki di kode dan bukan disesuaikan di test:

1. **`statement_count` salah.** Versi pertama menghitung `separators.len()`, yang membuat
   `SELECT 1; SELECT 2` terbaca satu statement. Aturannya sekarang: separator di akhir teks
   adalah *terminator*, separator yang masih punya lanjutan adalah *pemisah*.
2. **`format_interval` salah.** Python tidak mem-pad jam pada `str(timedelta)`
   (`3 days, 4:05:06`, bukan `04:05:06`), dan snapshot memang mencatat bentuk itu.

## Tugas berikutnya (urutan yang dikerjakan)

1. Protocol `DatabaseEngine` di Swift + `MockEngine`, dan mengarahkan UI lewat protocol itu
   (Fase 0, sisa).
2. Tag `python-engine-final` pada commit terakhir yang masih memuat engine Python.
3. `deploy/dev/compose.yaml` (podman) PostgreSQL + MySQL + Trino + dataset 500k×30 dan tabel
   zoo tipe.
4. `docs/benchmarks.md` baseline engine Python (butuh langkah 3).
5. `qh-result-store`, lalu `qh-rt`, `qh-driver` + tiga driver, `qh-export`, `qh-credentials`,
   `qh-storage`, `qh-tunnel`.

## Hasil pengukuran terakhir

**Belum ada.** Tidak ada satu pun angka kinerja yang ditulis ke `docs/benchmarks.md`, karena
§4.4 melarang mengarang angka dan baseline belum diukur. Perkiraan analitis di blueprint §4.2
(~61 KB per page window) ditandai sebagai perkiraan, bukan hasil.

## Known issues

| # | Isu | Dampak | Rencana |
|---|---|---|---|
| K1 | `web_search` tidak tersedia (HTTP 402, kuota paket habis) | §8.1 (pain point pesaing) tidak punya sumber; riset Tahap B tidak lengkap | [BUTUH TINDAKAN MANUAL] #1 |
| K2 | Baseline benchmark Python belum ada | Target throughput 5× (§6) belum punya pembanding | Butuh compose podman + dataset (langkah 3) |
| K3 | Zoo tipe belum diuji terhadap server nyata | Normalisasi tipe PG/MySQL/Trino belum tervalidasi di luar objek Python | Snapshot server nyata masuk langkah 3–4 |
| K4 | `tools/deps.py` (referensi di `docs/dependencies.md`) belum ada | Tabel dependency masih dibuat manual | Dibuat bersama job CI `cargo deny` |

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
