# PROGRESS — Migrasi Engine Python → Rust

> Dokumen kerja berjalan (§4.2). Diperbarui setiap selesai satu tugas.
> **Baca ini lebih dulu di awal sesi, lalu lanjutkan dari titik terakhir.**

- **Branch aktif:** `feat/rust-engine` (dibuat dari `main` @ `602bfb7`)
- **Fase aktif:** Tahap C **selesai** → pindah ke **Fase 0** (audit, golden snapshot & kontrak)
- **Terakhir diperbarui:** sesi ini
- **Mesin:** macOS arm64, `rustc 1.98.1`, `cargo 1.98.1`, Swift 6.2.3, podman (VM `podman-machine-default`, sudah start)

---

## Status per tahap

| Tahap | Status | Catatan |
|---|---|---|
| A — Audit codebase | **Selesai** | §1 blueprint. Struktur, protokol NDJSON, 11 perintah, dependency, diagnosis 5 penyebab "fetch lambat" (P1–P5) semuanya merujuk path file. |
| A — Docker Compose + dataset uji | **Belum** | podman tersedia dan sudah start; compose belum ditulis. |
| A — Baseline benchmark Python | **Belum** | Butuh compose di atas. |
| B — Riset web | **Gagal sebagian** | `web_search` mengembalikan HTTP 402: kuota paket pengguna habis. Lihat [BUTUH TINDAKAN MANUAL] #1. `web_fetch` **berfungsi** dan sudah dipakai memverifikasi `sqlx` (0.9.0, MIT OR Apache-2.0, dirilis 2026-05-21) langsung dari crates.io API. |
| C — Blueprint + ADR | **Selesai** | `docs/architecture/rust-engine-blueprint.md` §1–§8 + Architecture Decision Summary; ADR 0001–0010. |
| D — Fase 0 | **Sedang dikerjakan** | Lihat di bawah. |
| D — Fase 1 | Belum | |
| D — Fase 2, 3, 4 | Belum | |

---

## Tugas selesai

- [x] Membuat branch `feat/rust-engine`
- [x] Memverifikasi ulang Temuan §1 terhadap kode (bukan asumsi):
  - `app/engine/queryhive_engine.py:240` `PREVIEW_BATCH = 200` ✔
  - `app/engine/queryhive_engine.py:744` `to_text(value)` per sel ✔
  - `app/Sources/TrinoExporter/Support/Engine.swift:95` `JSONDecoder` per baris ✔
  - `app/Sources/TrinoExporter/Models/Connections.swift:364` service Keychain `id.data-ecosystem.queryhive` ✔
  - `app/Package.swift` `platforms: [.macOS(.v14)]` ✔ (sesuai target macOS 14)
  - `LICENSE:1` = MIT ✔
- [x] Menulis blueprint §2 (konkurensi/QoS/result store/grid/FFI data plane/cancel) — `docs/architecture/rust-engine-blueprint.md`
- [x] Menulis blueprint §3 (rekomendasi crate + workspace + trait driver + `Value` + skema SQLite siap-sinkron)
- [x] Menulis blueprint §4 (pemisahan control/data plane, aturan keamanan FFI, build & distribusi, daftar yang dihapus)
- [x] Menulis blueprint §5–§8 + Architecture Decision Summary, termasuk tabel konflik instruksi
- [x] ADR 0001–0010 di `docs/decisions/`

## Tugas berikutnya (urutan yang dikerjakan)

1. `tools/golden/record.py` + `compare.py`, lalu rekam snapshot dari engine Python (tanpa DB, memakai harness in-process `tests/test_engine_events.py`).
2. Protocol `DatabaseEngine` di Swift + `MockEngine`; arahkan UI melewati protocol.
3. `docs/benchmarks.md` baseline Python (butuh langkah 4).
4. `deploy/dev/compose.yaml` (podman) PostgreSQL + MySQL + Trino + dataset 500k×30 dan tabel zoo tipe.
5. Inisialisasi workspace Cargo + `qh-core`, `qh-sql`, `qh-result-store` (implementasi nyata + test).
6. Tag `python-engine-final` pada commit terakhir yang masih memuat engine Python.

## Hasil pengukuran terakhir

Belum ada. Tidak ada angka yang ditulis ke `docs/benchmarks.md` sampai diukur (§4.4 melarang
mengarang angka). Perkiraan analitis yang ada di blueprint ditandai sebagai perkiraan, bukan hasil.

## Known issues

| # | Isu | Dampak | Rencana |
|---|---|---|---|
| K1 | `web_search` tidak tersedia (HTTP 402, kuota paket habis) | §8.1 (pain point pesaing) tidak bisa diselesaikan dengan sumber; verifikasi versi crate harus lewat `web_fetch` ke crates.io/docs.rs atau lewat resolusi Cargo | Lihat [BUTUH TINDAKAN MANUAL] #1 |
| K2 | Baseline benchmark Python belum ada | Target throughput 5× (§6) belum punya pembanding | Butuh compose podman + dataset |

## [BUTUH TINDAKAN MANUAL]

1. **Kuota paket untuk `web_search` habis.** Top up (isi ulang) kredit paket, lalu jalankan riset
   Tahap B: pain point Navicat/DBeaver/TablePlus/DataGrip/Beekeeper (sumber primer: issue tracker
   resmi masing-masing proyek), serta status dukungan upstream versi PostgreSQL/MySQL/Trino untuk
   `docs/compatibility.md`. Sampai itu selesai, setiap klaim di §8.1 tetap ditandai
   `[perlu verifikasi]` dan **tidak** dipakai sebagai dasar prioritas produk.
2. **Notarisasi & code signing** butuh Apple Developer ID + sertifikat. Seluruh skrip dan
   konfigurasi disiapkan pada Fase 4; sampai sertifikat tersedia, build memakai ad-hoc signing
   (`xattr -dr com.apple.quarantine`) dan langkah notarisasi ditandai gagal-opsional di CI.
3. **Kunci EdDSA Sparkle** untuk update yang ditandatangani belum ada dan tidak boleh masuk repo.
   Dibuat pada Fase 4 (`generate_keys`), lalu kunci privat disimpan di secret CI.
