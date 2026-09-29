# Rencana pengembangan: performa, desain, dan fitur untuk eksekusi orkestrator

- **Status:** rencana eksekusi, 29 Sep 2026. Belum ada yang dikerjakan.
- **Pasangan:** `docs/architecture/prd-performance-and-parity.md` (PRD: FR, NFR, dan log keputusan O-* dan P-*). Isi teknis setiap fase performa ada di `docs/architecture/performance-plan.md`. Dokumen ini tidak mengulanginya dan hanya menyebut tugas, berkas, gate, dan urutan.
- **Penomoran fase** mengikuti `performance-plan.md` versi final:
  - 0 pengukuran;
  - 1 higiene engine;
  - 2 sesi persisten dan TTFR;
  - 3 Batch 7;
  - 4 editor (4A, 4B);
  - 5 grid;
  - 6 data plane;
  - 7 plafon ingest;
  - 8 eskalasi.
- **Pembaca:** orkestrator yang men-dispatch subagen. Pemilik baru membaca hasilnya di laporan akhir (§9).

## 0. Cara kerja orkestrator

### 0.1 Git

1. **W0-T1, sebelum apa pun:**
   - Jalankan `git status --porcelain`.
   - Commit WIP pemilik di `main` sebagai commit sendiri. Isinya semua berkas yang berubah atau belum dilacak, **kecuali** `docs/architecture/prd-performance-and-parity.md` dan `docs/architecture/development-plan.md`. Subjek: `docs: the performance plan, the TablePro feature map and the design audit`.
   - Periksa bahwa `Views/ResultGrid.swift`, `Views/SQLEditor.swift`, `Tests/.../HighlightBandTests.swift`, dan `DataPreferencesTests.swift` sudah terlacak (P-25). Bila ada berkas Swift yang belum di-commit, berkas itu ikut commit WIP. Setelah commit ini, hash-nya adalah **commit P**.
2. **Branch kerja.** `git switch -c work/perf-parity`. Commit pertama di branch ini: kedua dokumen rencana, dengan subjek `docs: a PRD and a development plan for performance and parity`.
3. **Satu commit per tugas** (atau per sub-fase yang punya gate sendiri) yang lulus gate.
   - Hanya orkestrator yang meng-commit.
   - Identitas: `Faisal Nugraha Cayunda`, tanpa co-author dan tanpa footer sesi.
   - Subjek memakai Conventional Commits dalam bahasa Inggris, dengan gaya repo (huruf kecil, kalimat), misalnya `perf(engine): one runtime per process, and Stop reaches the server`.
4. **Subagen tidak pernah menjalankan** `git commit`, `push`, `stash`, `reset`, `checkout` terhadap berkas orang lain, atau `rebase`.
5. **Akhir run.** `git switch main && git merge --no-ff work/perf-parity`. Tidak ada push atau PR.
6. **Revert.** Orkestrator boleh me-`git revert` commit tugasnya sendiri di branch kerja bila gate gelombang gagal (§8).

### 0.2 Brief subagen

Setiap brief memuat:

- tujuan dan ID FR/NFR;
- berkas yang **dimiliki** dan berkas yang **tidak boleh disentuh**;
- tugas yang sudah mendarat dan menjadi dasarnya;
- path blueprint;
- perintah verifikasi (§1).

Aturan yang selalu ditulis di brief:

- TablePro (`/Users/isal/Workspaces/Lab/Experiments/TablePro`) hanya dibaca untuk ide. Kode, aset, dan string tidak boleh disalin.
- Kode, komentar, dan pesan log dalam bahasa Inggris. Dokumen di `docs/` dan ADR dalam bahasa Indonesia, mengikuti konvensi direktori itu (`tablepro-adoption-plan.md` §13, catatan bahasa).
- Tanpa git write.
- Jaringan hanya untuk `cargo fetch` dependensi yang disetujui, image container, dan alat yang disebut §2.
- Tes yang menyentuh data pengguna memakai jalur terisolasi (`TestIsolation`, `DB_PATH` sementara).
- Brief selalu diakhiri permintaan: kembalikan **kesimpulan dan bukti** (perintah yang dijalankan beserta hasil lulus/gagal dan hitungan, dan daftar berkas yang berubah), bukan isi berkas.
- Agen `Explore` dan `Plan` tidak memuat `~/.claude/CLAUDE.md`, jadi aturan di atas ditulis di brief mereka.

### 0.3 Konkurensi dan worktree

- **Maksimal 3 implementer bersamaan.** Architect, reviewer, dan Explore tidak dihitung, tetapi tidak boleh berjalan saat sesi benchmark (§0.4).
- **Worktree.** Bila ada ≥ 2 implementer yang menyentuh Swift atau Rust bersamaan, masing-masing memakai `isolation: worktree`.
  - Worktree Swift perlu `./app/build-ffi.sh debug` sekali, karena `app/Package.swift` mencari arsip di `<root worktree>/target/ffi/static/`.
  - Worktree Rust memakai `target/` miliknya sendiri.
  - Selesai tugas: orkestrator meng-commit di worktree, `git cherry-pick` ke `work/perf-parity`, lalu menghapus worktree **beserta** `target/`-nya (pagar disk).
- **Satu lane FFI.** Hanya satu tugas yang sedang berjalan boleh memiliki `app/Generated/`, `crates/qh-ffi/src/uniffi_api.rs`, empat daftar invariant #11, dan `Support/RustEngine.swift`. Bila cherry-pick berkonflik di `app/Generated/`, orkestrator menjalankan ulang `./app/build-ffi.sh` di kepala branch dan meng-commit hasilnya bersama tugas itu.
- **Baseline PNG** hanya direkam ulang di langkah merge orkestrator, satu per satu (§0.5).

### 0.4 Disiplin benchmark

- **Sesi benchmark eksklusif.** Tidak ada implementer, build, atau reviewer lain yang berjalan. Beban mesin, `hw.model`, dan jumlah P-core/E-core dicatat di setiap rekaman.
- **Jumlah ulangan dan label:**
  - n ≥ 5 untuk angka QueryHive saja;
  - n ≥ 10 untuk head-to-head, dan n ≥ 20 untuk TTFR head-to-head;
  - run bergantian;
  - label `<fase>-<yyyymmdd>`, misalnya `f2-20260930`.
- **Container.** Trino hanya hidup untuk skenario Trino. PostgreSQL dan MySQL hidup terus.
- **Sumber angka.** `docs/benchmarks.md` hanya ditulis oleh `bench_fetch.py --report-only`.

### 0.5 Baseline visual

- **Perekaman awal.** Baseline direkam di W1-T3 dari commit P dengan `QH_RECORD_BASELINES=1`.
- **Rekam ulang** hanya untuk scene yang terdaftar sebagai perubahan V-1…V-11 di PRD §6.5, dan hanya oleh tugas pemilik perubahan itu. Syaratnya:
  - dilakukan dalam commit sendiri, dengan subjek `test(visual): re-record <scene> for <V-n>`;
  - ui-ux-designer dan a11y-architect memberi verdict approved.
- **Scene di luar daftar** yang berubah dianggap gagal gate, bukan alasan untuk merekam ulang.

### 0.6 Ledger

Orkestrator menyimpan ledger di scratchpad sesi (bukan di repo). Isinya:

- status per tugas (antre, jalan, lulus, terblokir), hash commit, dan bukti gate;
- angka bench;
- keputusan yang diambil di tengah run.

`PROGRESS.md` diperbarui per gelombang (tugas `Wx-D`), supaya run yang terputus bisa dilanjutkan dari repo.

## 1. Set verifikasi bernama

| Nama | Perintah | Kapan dijalankan |
|---|---|---|
| G-RUST | `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` | setiap tugas Rust |
| G-DENY | `cargo deny check licenses` | dependensi berubah, dan setiap gate gelombang |
| G-FFI | `./app/build-ffi.sh`, lalu `app/Generated/` ikut di-commit, lalu `cd app && swift test --filter RustEngineTests` | permukaan UniFFI atau daftar perintah berubah |
| G-SWIFT | `cd app && swift build && swift test` | setiap tugas Swift |
| G-VIS | `cd app && swift test --filter VisualParityTests` | setiap tugas yang menyentuh view |
| G-GOLDEN | `cargo build --bin queryhive-engine && /usr/bin/python3 tools/golden/live_cases.py` | container hidup. Setiap selisih harus terklasifikasi di `docs/golden-deltas.md`, dan selisih baru dianggap regresi. |
| G-LIVE | tes live di crate yang disentuh, dengan penjaga `QH_TEST_*` yang ada di crate itu: `QH_TEST_POSTGRES=1 cargo test -p qh-ffi --test real_server`, `QH_TEST_TRINO=1 cargo test -p qh-driver-trino`, `deploy/dev/qh-sshd-run.sh && QH_TEST_SSH=1 cargo test -p qh-tunnel`, `QH_TEST_KEYCHAIN=1 cargo test -p qh-credentials` | tugas yang menyentuh driver, tunnel, atau Keychain |
| G-APP | `./app/build.sh && app/dist/QueryHive.app/Contents/MacOS/QueryHive --snapshot "$SCRATCH/smoke.png" --scene done` | gate gelombang, dan tugas yang menyentuh `app/build.sh`, `Package.swift`, atau resource |
| G-BENCH(a) | `python3 deploy/dev/bench_app.py --axis <a> --label <fase>-<yyyymmdd> --repeat <n>`, lalu `python3 deploy/dev/bench_fetch.py --report-only` | tugas bench (eksklusif) |
| G-BENCHQ | subset cepat `--bench ttfr-pg,scroll-1m,type-10k` dengan n = 5, dibanding rekaman gelombang sebelumnya | gate W9–W13 (NFR-P9) |
| G-LEAK | `MallocStackLogging=1 leaks --atExit -- app/.build/debug/QueryHive --bench tabs-100`, lalu direktori `~/Library/Caches/QueryHive/spill` harus kosong | Fase 6, dan final |
| **G-HEAVY** | G-RUST + G-DENY + G-FFI + G-SWIFT + G-VIS + G-GOLDEN + G-APP | penutupan setiap gelombang |

## 2. Lingkungan (W0-T2)

Fakta yang sudah diverifikasi orkestrator:

- Apple M4, 10 core;
- `cargo`, Swift 6.4, dan `xcodebuild` tersedia;
- `xcodegen` dan `samply` tidak terpasang;
- mesin podman hidup dengan 2 CPU / 1,86 GiB dan belum punya container.

1. **Ukur host.** `sysctl -n hw.memsize hw.model hw.perflevel0.physicalcpu hw.perflevel1.physicalcpu` dan `df -h`.
   - Syarat ruang disk: ≥ 60 GB bebas.
   - Bila kurang: worktree dibatasi 1 dan build TablePro dilewati (dicatat).
2. **Resize VM podman (P-02, otonom dengan pagar):**
   - Hanya bila `podman ps -a` kosong atau hanya berisi container `qh-*`. Bila ada container lain, VM tidak diubah; skenario Trino ditulis `tidak diukur (VM)`.
   - Catat nilai lama lewat `podman machine inspect`.
   - Jalankan `podman machine stop && podman machine set --cpus 4 --memory 6144 && podman machine start`. Pakai `8192` bila `hw.memsize` ≥ 32 GiB.
   - Named volume tetap utuh, dan perubahan ini bisa dikembalikan. Nilai sebelum dan sesudah masuk laporan akhir.
3. **Container:**
   - `deploy/dev/up.sh all` untuk PostgreSQL 55432 dan MySQL 53306.
   - Trino di 58080 dinyalakan dengan batas `--memory 3g` (W1-T7 memindahkan batas ini ke `up.sh`).
   - `deploy/dev/qh-sshd-run.sh` untuk 52222.
   - Verifikasi: `pg_isready`, `mysqladmin ping`, perintah engine `test` ke Trino, dan `SHOW CATALOGS` yang memuat `tpch`. Digest image dicatat untuk rekaman bench.
4. **Alat:**
   - `cargo install --locked samply` (MIT OR Apache-2.0, tidak dikirim). Bila gagal, W1-T8 memakai `xcrun xctrace record --template "Time Profiler"`.
   - XcodeGen hanya untuk W1-T9: `brew install xcodegen` bila brew ada. Bila tidak, zip rilis resmi diunduh ke scratchpad, tidak pernah ke repo.
5. **Batas otonomi:**
   - Izin TCC (Screen Recording, Accessibility) tidak disentuh. `qhbench --check-permissions` hanya melaporkan statusnya.
   - `sudo` tidak pernah dipakai. Start dingin (`purge`) diserahkan ke pemilik.
   - Koneksi TablePro tidak disetel oleh agen.

## 3. Peta gelombang

| Gelombang | Isi | Bergantung pada | Tugas (perkiraan) | Gate yang memblokir gelombang berikutnya |
|---|---|---|---|---|
| W0 | Git, lingkungan, gate awal dan golden live | — | 3 | gate berat hijau di commit P, container sehat |
| W1 | **Fase 0**: pengukuran, harness, baseline visual | W0 | 10 | setiap sumbu punya angka QueryHive atau alasan tertulis; baseline ter-commit; profil 0.1 tercatat |
| W2 | **Fase 1** dan **Fase 4A**, plus blueprint Fase 2, 4B, dan 6 | W1 | 9 | gate standar Fase 1 dan 4A (fungsional); angka dicatat |
| W3 | **Fase 2** dan **Fase 4B-core**, plus blueprint Fase 5 | W2 | 6 | tes kebocoran state, golden tidak berubah, verdict architect-reviewer |
| W4 | **Fase 3** (Batch 7), **Fase 4B-integrasi**, **Fase 6-core** | W3 | 7 | paritas lexer 100%, tes perilaku Batch 7, tes enkripsi spill |
| W5 | **Fase 5** (grid), dan **Fase 6-engine** di Rust | W4 | 5 | paritas visual grid, daftar paritas fitur, tes AX |
| W6 | **Fase 6-Swift** (integrasi data plane) | W5 | 5 | tes diferensial, G-LEAK, paritas terhadap baseline Fase 5 |
| W7 | **Fase 7** (plafon ingest, opsi bersyarat) | W6 | 7 | golden dan `type_zoo` live identik |
| W8 | Penutupan performa, keputusan **Fase 8** | W7 | 4 (+0–4 kontingen) | laporan performa lengkap |
| W9 | Fondasi desain: P0 chrome, keyboard, pohon, shell. Plus pemecahan `AppModel` | W8 | 12 | G-HEAVY, G-BENCHQ, lantai a11y |
| W10 | Desain grid dan editor di atas komponen baru | W9 | 10 | G-HEAVY, G-BENCHQ |
| W11 | Metadata, SSH, JWT dan CA, MCP, autocomplete | W10 | 9 | G-HEAVY, G-LIVE (SSH, TLS) |
| W12 | Formatter, berkas `.sql`, script runner, impor, notifikasi | W11 | 9 | G-HEAVY |
| W13 | Paginasi, rencana query, aktivitas server, audit, lokalisasi | W12 | 10 | G-HEAVY |
| W14 | Final: gate penuh, benchmark, paritas, dokumen, merge, laporan | W13 | 6 | kriteria rilis PRD §9 |

**Total: sekitar 112 tugas**, sekitar 59 di antaranya implementasi. Rinciannya di §10.

## 4. Jalur arsitektur dan ADR

**Blueprint.** `code-architect` (opus) menulis blueprint tingkat berkas ke `docs/architecture/blueprints/<nama>.md` **sebelum** tugas mulai. `architect-reviewer` (opus) memeriksa blueprint itu, lalu memberi verdict atas implementasinya **sesudah** tugas selesai. Blueprint di-commit bersama commit dokumen gelombangnya.

| Blueprint | Ditulis di | Untuk | Isi wajib |
|---|---|---|---|
| `fase-2-engine-host.md` | W2-A1 | W3-T1 | Bentuk UniFFI `EngineHost`. Pool 2+1 (O-7). Semantik reset. Operasi panjang di luar pool (P-05). Tunnel per kunci. Handle SQLite. Warm-up. Hemat RTT. Tes reset. |
| `fase-4b-editor-analysis.md` | W2-A2 | W3-T2, W4-T2 | API `EditorDocument`. Checkpoint lexer. Aturan EOF. Pemetaan UTF-16. Penjaga IME. Hook rotor (FR-ED-09). Daftar yang dihapus. |
| `fase-6-data-plane.md` | W2-A3, disegarkan di W6-A1 | W4-T3, W5-T2, W6-T1 | Codec per chunk. Spill terenkripsi (NFR-S3). `ResultHandle` dan generation. `set_view`. `StoreRows`. "Off" Batch 7 dengan dua store per tab. |
| `fase-5-grid.md` | W3-A1 | W5-T1 | Seam `ResultRows`. Tabel dan row view. Header. Cache. Model kursor sel dan elemen AX (P-24). Daftar paritas. |
| `w9-shell-and-a11y.md` | W9-A1 | W9 | Rencana pemecahan `AppModel`. Peta menu dan pintasan kedua skema, tanpa konflik. Outline. Shell. Pemetaan 11 pt. Spesifikasi dari ui-ux-designer dan a11y-architect. |
| `w10-grid-editor-design.md` | W10-A1 | W10 | Kunci kursor. Peek. Tata bahasa staged. Tombol tinjau. Record. Diagnostik. Rotor. |
| `w11-metadata-and-connections.md` | W11-A1 | W11 | SQL metadata per driver. Nama setelan baru. Protokol TOFU (juga ditinjau security-reviewer). Item Keychain. Pemetaan MCP. API alias. |
| `w12-editor-and-run.md` | W12-A1 | W12 | Formatter. Event protokol `script`. Model result set. Pembaca JSON. Notifier. |
| `w13-plan-activity.md` | W13-A1 | W13 | Paginasi. EXPLAIN JSON dan ANALYZE. SQL aktivitas dan guard. Penampil audit. Strategi string catalog di bundel. |

**Jadwal ADR.** Semua ditulis `adr-generator` (sonnet) dalam bahasa Indonesia, dengan format `docs/decisions/0001…`.

| ADR | Isi | Tugas |
|---|---|---|
| 0031 bagian 1, addendum 0010 | Runtime per proses dan cancel sampai server | W2-D |
| 0031 lengkap, addendum 0016 | `EngineHost`, pool 2+1, reset, operasi panjang di luar pool | W3-D |
| 0033 | Analisis editor di Rust, tanpa tree-sitter | W4-D |
| 0037 | Spill terenkripsi dengan kunci efemeral per proses | W4-D |
| 0032 | Grid dengan sel yang digambar (mengamandemen 0003) | W5-D |
| 0030, amandemen 0004 dan 0008 | Data plane app. Menggantikan 0013. | W6-D |
| 0034 | View in-memory di Rust, kolasi, divergensi | W6-D |
| 0035, 0036, amandemen 0028 | Bersyarat, hanya bila diadopsi | W7-T6 |
| 0038 | Perintah baca baru dan keluaran yang digerbangi setelan (P-06) | W11-D |
| 0039 | SSH di app, TOFU dengan fingerprint dipatok | W11-D |
| 0040 | JWT Trino dan CA per koneksi, termasuk batas MySQL | W11-D |
| 0041 | Formatter | W12-D |
| 0042 | Script runner dan result set berganda | W12-D |
| 0043 | Aktivitas server dan guard cancel | W13-D |
| 0044 | Paginasi dan cap tunggal | W13-D |

**Pembersihan di akhir setiap gelombang (`Wx-C`).**

- `refactor-cleaner` (sonnet) membuang kode mati, dibatasi ke berkas yang disentuh gelombang itu.
- `code-simplifier` (sonnet) menyederhanakan area yang sama tanpa mengubah perilaku.
- Gate: `code-reviewer` (opus) dan G-HEAVY.
- Commit `refactor(<area>): …`, atau tidak ada commit bila tidak ada perubahan.

## 5. Tugas per gelombang

**Singkatan reviewer (semuanya opus):**

| Singkatan | Agen |
|---|---|
| RR | rust-reviewer |
| SR | swift-reviewer |
| CR | code-reviewer (wajib di setiap fase performa, `performance-plan.md` §0) |
| AR | architect-reviewer |
| UX | ui-ux-designer |
| AX | a11y-architect |
| SEC | security-reviewer |
| DB | database-reviewer |
| SF | silent-failure-hunter |
| TD | type-design-analyzer |
| PO | performance-optimizer (verdict atas angka) |

**Implementer:** `general-purpose` sonnet (GP-s), kecuali yang ditandai **GP-o** (opus, dengan alasannya).

### W0: persiapan

| ID | Tugas | Pelaksana | Gate | Verifikasi | Ukuran | Commit |
|---|---|---|---|---|---|---|
| W0-T1 | Commit WIP pemilik, buat branch, commit dokumen rencana, catat commit P (§0.1) | orkestrator, solo (langkah git mekanis) | — | `git log -2`, `git status` bersih | S | dua commit di §0.1 |
| W0-T2 | Lingkungan (§2) | GP-s | orkestrator memeriksa bukti | pemeriksaan container di §2.3 | S | — |
| W0-T3 | Gate awal dan golden live pertama untuk MySQL dan Trino. Selisih baru diklasifikasi (tidak diperbaiki). | test-engineer · sonnet | CR | G-HEAVY, G-GOLDEN (22 kasus) | S | `test(golden): the MySQL and Trino live cases run on this machine, and their deltas are classified` (hanya bila `docs/golden-deltas.md` berubah) |

### W1: Fase 0 (pengukuran dan baseline)

Urutan batch:

1. T1, T2, T3
2. T4, T5, T7
3. T6, T8, T9
4. T10 (eksklusif)

**W1-T1. Skema rekaman dan laporan per sumbu (0.6).** Ukuran S.
- Cakupan: FR-PERF-07, NFR-Q.
- Berkas: `deploy/dev/bench_app.py` (baru), `deploy/dev/bench_fetch.py` (kunci `bench`, `axis`, `scenario`, `app`, `app_rev`, `competitor_rev`, `hw_model`, `p_cores`, `e_cores`, `display_hz`, `server_image`, dan `write_report` per sumbu), `docs/benchmarks.md` (dihasilkan).
- Pelaksana: performance-engineer · sonnet. Gate: CR.
- Verifikasi: `--report-only` merender rekaman lama tanpa perubahan dan menambah bagian per sumbu berisi "[belum diukur]"; ditambah tes kecil `deploy/dev/test_bench_report.py`.
- Commit: `chore(bench): one record schema for every axis, and a report section per axis`.

**W1-T2. `bench_ffi` (0.2).** Ukuran S.
- Berkas: `crates/qh-ffi/examples/bench_ffi.rs` (baru). `crates/qh-ffi/Cargo.toml` hanya bila perlu.
- Pelaksana: GP-s. Gate: RR, CR.
- Verifikasi: G-RUST, lalu `cargo run --release -p qh-ffi --example bench_ffi -- local-loop|emit-only|preview-wide`.
- Commit: `perf(bench): an in-process FFI bench for per-call cost, preview and emit`.

**W1-T3. Gate paritas visual dan baseline (0.8).** Ukuran M. Dijalankan pertama, supaya baseline benar-benar berasal dari commit P.
- Cakupan: NFR-V.
- Berkas: `app/Tests/QueryHiveTests/VisualParityTests.swift` (baru), `app/Tests/QueryHiveTests/__Baselines__/` (baru), `app/Sources/QueryHive/Support/Snapshot.swift` (scene dan ekspor metrik layout). Accessor khusus tes di `Views/ResultGrid.swift` dan `Views/SQLEditor.swift` hanya bila tidak ada jalan lain.
- Pelaksana: test-engineer · sonnet. Gate: SR, UX.
- Verifikasi:
  - `QH_RECORD_BASELINES=1` sekali;
  - G-VIS lulus dua kali berturut-turut (determinisme);
  - komparator diuji dengan selisih sintetis 1 piksel.
- Commit: `test(visual): a parity gate with baselines recorded from the committed UI`.

**W1-T4. Signpost dan `--bench` (0.4).** Ukuran M. Dijalankan setelah T3.
- Berkas: `Support/PerfSignposts.swift` dan `Support/BenchMode.swift` (baru); `App.swift`, `Models/AppModel.swift`, `Views/SQLEditor.swift` (termasuk log mode TextKit untuk 0.9), `Views/ResultGrid.swift`, `Support/RustEngine.swift`.
- Skenario:
  - sintetis: `scroll-1m`, `scroll-500c`, `open-500c`, `type-10k`, `type-2m`, `tabs-100`, `launch`;
  - dengan DB: `ttfr-*`, `throughput-*`, `memory-*`, `cancel-*` lewat `QH_BENCH_*` di direktori support terisolasi.
- Pelaksana: GP-s. Gate: SR, PO.
- Verifikasi: G-SWIFT, G-VIS (tidak ada piksel berubah), G-APP, dan setiap skenario dijalankan sekali.
- Commit: `perf(app): signposts and a --bench mode that drive the grid and the editor without a person`.

**W1-T5. Microbench Swift (0.3).** Ukuran S.
- Berkas: `app/Tests/QueryHiveTests/Bench/` (baru).
- Pelaksana: test-engineer · sonnet. Gate: SR.
- Verifikasi: `cd app && QH_BENCH=1 swift test -c release --filter Bench`, dan tanpa `QH_BENCH` tes ini dilewati.
- Commit: `test(bench): Swift microbenchmarks for decode, paint, sort and one keystroke`.

**W1-T6. `qhbench` (0.5).** Ukuran M.
- Berkas: `tools/bench/qhbench/**` (paket baru, termasuk `profiles/queryhive.json` dan `profiles/tablepro.json`).
- Tanpa izin OS, alat ini menulis rekaman berstatus `tidak diukur (izin OS)` dan tidak gagal.
- Pelaksana: performance-engineer · sonnet. Gate: SR.
- Verifikasi: `swift build -c release --package-path tools/bench/qhbench`, lalu `qhbench --check-permissions`.
- Commit: `perf(bench): a black-box harness that drives either app the same way`.

**W1-T7. Fixture (0.7).** Ukuran S.
- Berkas: `deploy/dev/up.sh` (toxiproxy, `--memory` untuk Trino), `deploy/dev/compose.yaml`, `deploy/dev/make_sql_corpus.py` (baru), `deploy/dev/make_many_tables.py` (baru, 5.000 tabel).
- Pelaksana: GP-s. Gate: CR.
- Verifikasi: `bash -n deploy/dev/up.sh`, toxiproxy hidup, `tpch.sf1.lineitem` bisa dibaca, dan ukuran korpus sesuai.
- Commit: `chore(dev): fixtures for RTT, 5,000 tables and large SQL files`.

**W1-T8. Profil ADR-0013 (0.1) dan dua verifikasi (0.9).** Ukuran S.
- Isinya:
  - `samply` (atau xctrace) pada `queryhive-engine preview > /dev/null` dan lewat harness;
  - mode TextKit dari log W1-T4;
  - `rustc --print cfg --target aarch64-apple-darwin` dibanding `-C target-cpu=apple-m1`;
  - sapuan spill yatim di `store.rs:107-110`.
- Berkas: hanya rekaman `bench: cli-profile` lewat `bench_app.py`.
- Pelaksana: performance-engineer · **opus** (interpretasi profil ini menjadi dasar ADR-0030). Gate: PO.
- Commit: `perf(bench): the ADR-0013 claim profiled, and two assumptions checked`.

**W1-T9. Build TablePro.** Ukuran S, dengan batas waktu 60 menit dan maksimal 3 percobaan. Di luar repo.
- Langkah:
  - `git -C …/TablePro rev-parse HEAD` dicatat sebagai `competitor_rev`;
  - XcodeGen dan `scripts/download-libs.sh`;
  - `xcodebuild -scheme TablePro -configuration Release -derivedDataPath "$SCRATCH/tp-dd" CODE_SIGN_IDENTITY=- CODE_SIGNING_REQUIRED=NO build`.
- Bila gagal, alasannya dicatat. DMG resmi hanya bisa dipakai setelah peluncuran pertama oleh pemilik (Gatekeeper), jadi masuk laporan akhir.
- Pelaksana: performance-engineer · sonnet. Tanpa commit.

**W1-T10. Sesi angka Fase 0.** Ukuran M, eksklusif.
- Isinya: semua sumbu QueryHive dan sekunder; TablePro lewat `qhbench` bila izinnya ada.
- Pelaksana: performance-engineer · sonnet. Gate: PO.
- Verifikasi: G-BENCH untuk semua sumbu.
- Commit: `perf(bench): Fase 0 numbers for every axis`.

**Gate W1** (`performance-plan.md` §4): setiap sumbu punya angka QueryHive atau alasan tertulis; TablePro punya angka atau alasan; baseline ter-commit; profil tercatat; laporan diregenerasi; G-HEAVY.

### W2: Fase 1, Fase 4A, dan tiga blueprint

T1, T2, dan T3 berjalan paralel di worktree. A1–A3 berjalan paralel karena hanya membaca.

**W2-T1. Fase 1.1–1.2: runtime persisten dan cancel preemptif.** Ukuran S.
- Cakupan: FR-PERF-01, NFR-P6, NFR-P8.
- Berkas: `crates/qh-ffi/src/{uniffi_api.rs,lib.rs,commands.rs,main.rs}`, `crates/qh-ffi/tests/{golden.rs,real_server.rs}`, dan `app/Generated/` bila permukaannya berubah.
- Urutan kerja: `tdd-guide` (sonnet) menulis tes gagal lebih dulu (cancel sebelum batch pertama; `pg_sleep(30)` yang dikonfirmasi server). Setelah itu GP-s mengimplementasikan.
- Gate: RR, SF, CR.
- Verifikasi: G-RUST, G-LIVE (PostgreSQL, Trino), G-GOLDEN (`cancel_*` tetap lulus).
- Commit: `perf(engine): one runtime per process, and Stop reaches the server`.

**W2-T2. Fase 1.3–1.4: QoS dan clamp.** Ukuran S.
- Cakupan: FR-PERF-02 (clamp 200.000, O-12).
- Berkas: `Support/RustEngine.swift`, `Models/AppModel.swift` (`:2832`), `Views/SettingsView.swift`, `app/Tests/QueryHiveTests/RowLimitSettingTests.swift`.
- Pelaksana: GP-s. Gate: SR, CR.
- Verifikasi: G-SWIFT.
- Commit: `perf(app): engine work on a user-initiated queue, and a 200,000-row ceiling for now`.

**W2-T3. Fase 4A: jalur panas editor.** Ukuran M.
- Cakupan: FR-PERF-04, NFR-P5.
- Berkas: `Views/SQLEditor.swift`, `Views/Workspace.swift` (`lineCount`), `Support/SQLFolding.swift`, `Views/SQLSyntax.swift`.
- Pelaksana: GP-s. Gate: SR, CR, PO.
- Verifikasi:
  - G-SWIFT, termasuk `EditorFindAndFoldingTests`, `HighlightBandTests`, dan `LineNumberRulerTests`;
  - G-VIS (scene editor);
  - `--bench type-10k`.
- Commit: `perf(editor): TextKit 1 on purpose, folding in the layout manager, and no whole-document work per keystroke`.

**Blueprint (§4):**

| ID | Blueprint | Agen | Pemeriksa |
|---|---|---|---|
| W2-A1 | Fase 2 | code-architect · opus | AR |
| W2-A2 | Fase 4B | code-architect · opus | AR |
| W2-A3 | Fase 6 | code-architect · opus | AR |

**W2-T4. Sesi bench Fase 1 dan 4A.** Eksklusif. Pelaksana performance-engineer · sonnet, gate PO.
- Sumbu: 6, perintah lokal (`bench_ffi local-loop`), dan 5 (10k).
- Commit: `perf(bench): Fase 1 and 4A numbers`.

**W2-D.** ADR 0031 bagian 1, addendum 0010, commit blueprint, dan `PROGRESS.md`. Pelaksana adr-generator dan doc-updater · sonnet.
- Commit: `docs: ADR-0031 part one, and the blueprints for the engine host, editor analysis and data plane`.

**W2-C.** Pembersihan (§4).

**Gate W2:** gate standar Fase 1 dan 4A. Angka yang meleset diperiksa PO satu putaran (P-21).

### W3: Fase 2 dan Fase 4B-core

**W3-T1. Fase 2: `EngineHost`, pool, dan TTFR.** Ukuran L. Implementer **GP-o**, karena kebenaran reset, konkurensi pool, dan tunnel bersama adalah bug yang diam.
- Cakupan: FR-PERF-03, NFR-P1 (S1, S3, S4), NFR-P8.
- Berkas:
  - `crates/qh-ffi/src/{host.rs (baru),lib.rs,commands.rs,uniffi_api.rs,tunnel.rs,local.rs}`;
  - `crates/qh-driver/src/lib.rs` (`Session::reset`);
  - `crates/qh-driver-{postgres,mysql,trino}/src/lib.rs`;
  - `crates/qh-tunnel/src/*`;
  - `Support/RustEngine.swift`, `Support/DatabaseEngine.swift`;
  - `Models/AppModel.swift` (warm-up, pemuatan pohon lewat host);
  - `app/Generated/`.
- Urutan kerja: `tdd-guide` (sonnet) menulis lebih dulu tes berikut: `search_path` tidak bocor, transaksi gagal pulih, timeout antar-Run, Safe Mode per Run, tunnel dipakai ulang.
- Gate: RR, DB (`ROLLBACK`/`DISCARD ALL`, `COM_RESET_CONNECTION`, `SQL_SELECT_LIMIT`, pipelining), SEC (kunci pool, tunnel, versi kredensial), SF, TD, AR, CR.
- Verifikasi:
  - G-RUST, G-FFI, G-SWIFT, G-GOLDEN (tidak berubah);
  - G-LIVE (PostgreSQL, Trino, SSH);
  - G-BENCH(1), pohon, 5.000 tabel.
- Commit: boleh dua, masing-masing dengan gate:
  - `perf(engine): an engine host with a session pool that resets every run`;
  - `perf(engine): capped previews keep their session, warm-up on select, one round trip fewer`.

**W3-T2. Fase 4B-core.** Ukuran M. Implementer **GP-o**, karena semantik regex dan aturan EOF harus direproduksi persis, di atas pemetaan UTF-8 ke UTF-16.
- Cakupan: FR-PERF-04.
- Berkas:
  - `crates/qh-sql/src/editor.rs` (baru), `crates/qh-sql/src/lib.rs`, `crates/qh-sql/tests/editor_*.rs`, `crates/qh-sql/tests/fixtures/`;
  - `app/Tests/QueryHiveTests/LexerFixtureExport.swift` (baru; mengekspor span `SQLSyntax` saat `QH_EXPORT_FIXTURES=1`).
- Uji acak berbenih tanpa dependensi baru. `proptest` hanya bila `cargo deny` lulus.
- Gate: RR, TD, CR.
- Verifikasi: G-RUST (paritas terhadap fixture Swift), G-SWIFT.
- Commit: `feat(sql): an incremental editor lexer with statement and fold analysis in qh-sql`.

**W3-A1.** Blueprint Fase 5. code-architect · opus, dengan spesifikasi kursor dan AX dari UX dan AX. Pemeriksa AR.

**W3-T3.** Sesi bench Fase 2 (eksklusif) dan verdict PO.

**W3-D.** ADR-0031 lengkap, addendum 0016, dan blueprint Fase 5.

**W3-C.** Pembersihan.

### W4: Fase 3, Fase 4B-integrasi, dan Fase 6-core

T1, T2, dan T3 berjalan paralel dengan berkas yang terpisah. T4 dijalankan setelah T1 dan T3.

**W4-T1. Fase 3: Batch 7 server-first.** Ukuran S–M.
- Cakupan: FR-GRID-03, FR-GRID-04 (O-8), V-1.
- Berkas: `Models/{QueryTab,AppModel,ServerSort,GridSort,GridSearch}.swift`, `Views/ResultGrid.swift`, tes baru `Batch7Tests.swift`, dan baseline scene banner.
- Pelaksana: GP-s. Gate: SR, DB (SQL pembungkus), UX, CR.
- Tes perilaku sesuai `performance-plan.md` §7 Gate:
  - siklus klik;
  - sumber indikator tunggal;
  - tiga kasus fallback;
  - galat server yang tidak disamarkan;
  - debounce 250 ms dan minimal 3 karakter;
  - penjaga edit;
  - "off" ≤ 10k disimpan dan > 10k dijalankan ulang.
- Verifikasi: G-SWIFT, G-VIS (rekam ulang V-1 saja), G-GOLDEN.
- Commit: `feat(grid): sort and search go to the server first, with one source for the header's sort state`.

**W4-T2. Fase 4B-integrasi.** Ukuran M.
- Cakupan: FR-PERF-04, FR-ED-09, NFR-P5.
- Berkas:
  - `crates/qh-ffi/src/editor.rs` (baru), `crates/qh-ffi/src/lib.rs` (satu baris `mod`), `app/Generated/`;
  - `Views/SQLEditor.swift`, `Views/SQLSyntax.swift`, `Support/SQLFolding.swift`, `Models/SQLScanner.swift` (bagian statement dihapus);
  - `Support/EditorAnalysis.swift` (baru).
- Pelaksana: GP-s. Gate: SR, RR, SEC (buffer FFI, semua ekspor throwing), AX (hook rotor), AR, CR.
- Verifikasi:
  - G-RUST, G-FFI, G-SWIFT;
  - tes Swift yang membandingkan span lewat FFI dan span regex atas korpus. Tes ini harus lulus **sebelum** regex dihapus.
  - G-VIS (editor);
  - `--bench type-10k` ≤ 4 ms dan `type-2m` ≤ 8 ms.
- Commit: `perf(editor): analysis moves to Rust, colouring and folding reach 2M characters`.

**W4-T3. Fase 6-core: codec, view, dan spill terenkripsi.** Ukuran L.
- Cakupan: FR-PERF-05, NFR-S3.
- Berkas:
  - `crates/qh-result-store/src/{codec.rs,store.rs,view.rs (baru),spill.rs (baru),lib.rs}`, `crates/qh-result-store/Cargo.toml` (`ring`, `rayon`, `unicode-segmentation`);
  - `crates/qh-rt/src/lib.rs` (pool rayon ber-QoS);
  - `Cargo.toml`, `Cargo.lock`.
- Pelaksana: GP-s. Gate: RR, SEC (nonce, AAD, umur kunci, `0600`, sapuan), TD, SF (disk penuh), CR.
- Verifikasi:
  - G-RUST, G-DENY;
  - tes spill: teks biasa tidak ada di berkas, kunci salah gagal, manipulasi gagal, mode berkas `0600`, sapuan yatim;
  - tes view dan window.
- Commit: `feat(store): a typed columnar codec, a rayon view, and spill encrypted with a per-process key`.

**W4-T4. Fixture diferensial sort, filter, dan search.** Ukuran S.
- Berkas: `app/Tests/QueryHiveTests/SortFixtureExport.swift` (baru), `crates/qh-result-store/tests/{differential.rs,fixtures/}`.
- Korpus: ASCII, Latin beraksen, nama Indonesia, CJK, emoji, dan campuran digit. Divergensi didaftar secara eksplisit di tes (O-9).
- Pelaksana: test-engineer · sonnet. Gate: CR.
- Commit: `test(store): differential sort, filter and search fixtures exported from the Swift implementation`.

**W4-T5.** Sesi bench: sumbu 5, sort sekunder, dan verdict PO.

**W4-D.** ADR 0033 dan 0037.

**W4-C.** Pembersihan.

### W5: Fase 5 dan Fase 6-engine

T1 (Swift) dan T2 (Rust) berjalan paralel di worktree. T2 memiliki `app/Generated/`, sedangkan T1 tidak menyentuhnya.

**W5-T1. Fase 5: grid `NSTableView`.** Ukuran L. Implementer **GP-o**, karena permukaan paritasnya paling luas, dan fokus SwiftUI ke AppKit, IME, serta AX semuanya bertemu di sini.
- Cakupan: FR-GRID-01, FR-GRID-07 (dasar), NFR-P4.
- Berkas:
  - baru: `Views/{ResultGridTable,GridRowView,GridHeaderView,GridAccessibility}.swift`, `Models/ResultRows.swift`;
  - diubah: `Views/ResultGrid.swift` (body diganti), `Models/{QueryTab,GridValue,ColumnFormat,WritePlan,CellSelection}.swift`, `Support/Snapshot.swift`;
  - tes: `ResultGridTests`, `GridColumnsTests`, `GridParityTests` baru, dan tes pohon AX.
- Gate: SR, TD (`ResultRows`, `CellText`), AX, UX, AR, CR, PO.
- Verifikasi:
  - G-SWIFT, G-VIS (grid, tanpa rekam ulang);
  - daftar paritas `performance-plan.md` §9;
  - `--bench scroll-1m`, `scroll-500c`, `open-500c` ≤ 30 ms; TTFR S1 tidak mundur.
- Commit: dua, masing-masing dengan gate:
  - 5a `refactor(grid): every reader of rows goes through a ResultRows seam`;
  - 5b `perf(grid): an NSTableView that draws its cells, and the SwiftUI grid is gone`.

**W5-T2. Fase 6-engine.** Ukuran L. Implementer **GP-o**, karena menyangkut umur handle, generation, dan panic lintas FFI.
- Cakupan: FR-PERF-05.
- Berkas: `crates/qh-ffi/src/{host.rs,store_api.rs (baru),commands.rs (`RESULT_SINK=store`),uniffi_api.rs,lib.rs}`, `app/Generated/`, `app/Tests/QueryHiveTests/ResultHandleSmokeTests.swift` (baru).
- Gate: RR, SEC, TD, SF, AR, CR.
- Verifikasi: G-RUST, G-FFI, G-SWIFT, G-GOLDEN (CLI tidak berubah), dan `bench_ffi window`.
- Commit: `feat(engine): a store sink and ResultHandle windows over UniFFI`.

**W5-T3.** Sesi bench: sumbu 4, lalu `window` p99, dengan verdict PO. Bila p99 > 0,5 ms, kasus ini dicatat untuk W8-T2.

**W5-D.** ADR 0032.

**W5-C.** Pembersihan.

### W6: Fase 6-Swift

**W6-A1.** Blueprint Fase 6 disegarkan terhadap seam Fase 5 (code-architect · opus, pemeriksa AR).

**W6-T1. Fase 6-Swift: integrasi data plane.** Ukuran L. Implementer **GP-o**, karena kode lama dihapus besar-besaran dan umur handle berinteraksi dengan tab.
- Cakupan: FR-PERF-05, FR-GRID-03 dan FR-GRID-04 (fallback Rust, "off" lewat store dasar), NFR-P1 (S2), P2, P3, P8.
- Berkas:
  - `Models/StoreRows.swift` (baru), `Models/{QueryTab,AppModel,GridSort,GridSearch,WritePlan}.swift`;
  - `Support/RustEngine.swift`, `Views/ResultGridTable.swift` (polling `displayLink`), `Views/CellValueViewer.swift`;
  - `App.swift` (sapuan spill saat startup).
- Dihapus: penumpukan baris, `previewPaintInterval`, `displayedCache`, dan `GridSort.order` di jalur panas.
- Clamp naik ke 5.000.000.
- Gate: SR, SF, AR, CR, PO.
- Verifikasi:
  - G-SWIFT, G-VIS (terhadap baseline Fase 5), tes diferensial W4-T4;
  - G-LEAK;
  - G-BENCH(1 S2, 2, 3) dan window.
- Commit: `perf(app): results live in the Rust store, and the grid reads windows`.

**W6-T2.** Sesi bench lengkap Fase 6 dan G-LEAK (eksklusif), dengan verdict PO.

**W6-D.** ADR 0030 (menggantikan 0013; mengamandemen 0004 dan 0008) dan ADR 0034 (termasuk daftar divergensi).

**W6-C.** Pembersihan.

### W7: Fase 7 (plafon ingest)

Setiap item diuji A/B dan hanya dipertahankan bila memberi ≥ 10% pada sumbunya tanpa regresi di sumbu lain. Satu commit per item.

Urutan:
1. T1
2. T2 dan T4 paralel (PostgreSQL dan Trino, berkas terpisah)
3. T3 setelah T2
4. T5 terakhir

| ID | Isi | Berkas | Pelaksana | Gate | Verifikasi | Commit |
|---|---|---|---|---|---|---|
| W7-T1 | 7.1 builder tanpa alokasi | `crates/qh-core` (`ColumnarBuilder`), `crates/qh-driver` (`Cursor::next_into`), tiga driver, ingest `qh-result-store` | GP-s | RR, CR, PO | G-RUST, G-GOLDEN (`type_zoo` live identik), G-BENCH(2) A/B | `perf(drivers): rows go straight into the store's columns` |
| W7-T2 | 7.2 PostgreSQL `COPY … TO STDOUT` | `crates/qh-driver-postgres`, pemeriksaan satu SELECT di `crates/qh-sql`, jalur ekspor, `memchr` langsung | GP-s | RR, DB, CR | sama; dipakai bila ≥ 1,3× | `perf(postgres): COPY TO STDOUT for exports and large previews` |
| W7-T3 | 7.3 hasil biner (**bersyarat**: profil menunjukkan output teks server dominan) | driver PostgreSQL | GP-s | RR, DB | golden `type_zoo` live | `perf(postgres): binary results where every column has a decoder` |
| W7-T4 | 7.4 spooling Trino dan JSON SIMD (**bersyarat**: parse JSON > 50% profil setelah 7.1) | `crates/qh-driver-trino` | GP-s | RR, CR | G-LIVE Trino, G-BENCH(2) | `perf(trino): …` |
| W7-T5 | 7.5 mimalloc dan PGO (**bersyarat**, A/B termasuk sumbu 3) | `crates/qh-ffi` (`#[global_allocator]`), `tools/pgo.sh` | GP-s | RR, PO | G-BENCH(2,3), build reproducible | `perf(build): …` |
| W7-T6 | Sesi A/B, lalu ADR 0035/0036 dan amandemen 0028 bila diadopsi | — | performance-engineer, adr-generator · sonnet | PO | G-BENCH | `docs: …` |

**W7-C.** Pembersihan.

**Gate W7:** sumbu 2 penuh (≥ 575k atau ≥ 80% plafon COPY, dan ≥ 1,5× TablePro bila terukur), golden identik, G-HEAVY.

### W8: penutupan performa

- **W8-T1.** Sesi bench penuh untuk semua sumbu (eksklusif). Pelaksana performance-engineer · sonnet, verdict PO.
- **W8-T2. Keputusan Fase 8.** PO dan `architect` (keduanya opus) memeriksa setiap gate angka yang meleset, lalu memilih eskalasi dari `performance-plan.md` §12. Hasilnya adalah tugas kontingen, masing-masing L dengan blueprint dan gate sendiri, disusun `planner` (opus):
  - W8-E1 C ABI `window`;
  - W8-E2 TextKit 2 atau CodeEditTextView upstream;
  - W8-E3 grid Metal;
  - W8-E4 cap berbasis portal PostgreSQL.
- **W8-D.** doc-updater memperbarui:
  - status `performance-plan.md`;
  - `PROGRESS.md`;
  - bagian `app/DESIGN.md` yang kini salah (batas Stop, batas pewarnaan 200k, deskripsi grid);
  - `docs/invariants.md`, hanya untuk insiden nyata selama run.
- **W8-C.** Pass perbaikan kode atas seluruh area performa.

### W9: fondasi desain

**Urutan batch:**
1. T0 sendirian.
2. T1, T2, T3.
3. T4 (setelah T1), T5, T6.
4. T7 (setelah T1) dan T8 (setelah T1, T2, T4).
5. T9 sendirian.

T3 boleh masih berjalan di batch 3 dan 4, selama jumlah implementer tetap ≤ 3.

**W9-A1.** Blueprint `w9-shell-and-a11y.md` (code-architect · opus) dengan spesifikasi UX dan AX. Pemeriksa AR.

| ID | Isi | Cakupan | Berkas | Pelaksana | Gate | Verifikasi | Ukuran | Commit |
|---|---|---|---|---|---|---|---|---|
| W9-T0 | Pecah `AppModel` (P-22), perpindahan murni | — | `Models/AppModel.swift` menjadi `AppModel.swift` plus `AppModel+{Run,Tree,Connections,Edit,Export,History,Session,Completion,Focus}.swift` | GP-s | SR | G-SWIFT, G-VIS tidak berubah | M | `refactor(app): AppModel split into one extension per domain, with no change in behaviour` |
| W9-T1 | Lantai a11y chrome dan salinan basi | FR-UI-01, 04, 05, 07; V-2 | `Support/Theme.swift` (label `IconButton`/`Chip`, modifier permukaan, Reduce Motion), `Views/Workspace.swift` (chip tab, `EmptyWorkspace`, placeholder) | GP-s | SR, AX, UX | G-SWIFT (tes semua ikon berlabel), G-VIS (V-2), tes kontras | M | `feat(a11y): tabs are buttons, icons have names, and motion and glass follow the system settings` |
| W9-T2 | Menu View, fokus, kunci tab, tes konflik, perbaikan ⌘E | FR-UI-02, 03 | `App.swift`, `Models/Shortcuts.swift`, `Models/AppModel+Focus.swift` | GP-s | SR, AX, UX | G-SWIFT (`ShortcutConflictTests` baru) | S | `feat(app): a View menu, focus moves and tab keys, and no key bound twice` |
| W9-T3 | Pohon `NSOutlineView`, dialog Truncate/Drop, drag ke editor | FR-TREE-04, 05, FR-RUN-07 (sebagian), FR-UI-07; V-5 | `Views/SchemaOutline.swift` (baru), `Views/SidebarTree.swift`, `Models/SchemaTree.swift`, `Models/AppModel+Tree.swift` | GP-s | SR, AX, UX, AR, CR | G-SWIFT (`SidebarRenderTests`, `TreeNodeStalenessTests`, tes outline AX), G-VIS | L | `feat(tree): the object tree becomes an outline view that the keyboard and VoiceOver can walk` |
| W9-T4 | Tag lingkungan dan chrome Safe Mode | FR-CON-06, FR-SAFE-01; V-3 | `Models/Connections.swift`, `Views/ConnectionsViews.swift` (pemilih), `Views/ContextCascade.swift`, `Views/Workspace.swift` (lencana tab), `Views/RootView.swift` (status bar) | GP-s | SR, UX, AX, CR | G-SWIFT (dekode lama tetap tanpa tag), G-VIS (V-3) | M | `feat(safety): the connection's environment and Safe Mode are visible wherever a query is written` |
| W9-T5 | Banner galat inline dan lembar konfirmasi | FR-RUN-06, 07; V-4 | `Views/ResultGrid.swift` (badan galat), `Views/RunConfirmationSheet.swift`, `Models/RunConfirmation.swift` | GP-s | SR, UX, AX | G-SWIFT (`RunConfirmationTests`: Esc membatalkan, tanpa Return default), G-VIS | S | `feat(app): errors you can select and copy, and a write confirmation that Return cannot approve by accident` |
| W9-T6 | Open Quickly | FR-UI-09; V-6 | `Views/OpenQuickly.swift`, `Models/QuickSearch.swift` | GP-s | SR, AX | G-SWIFT (`QuickSearchTests`) | S | `feat(app): Open Quickly keeps the highlight in view and filters by scope` |
| W9-T7 | Settings: judul pane, `HelpHint` yang terjangkau, ukuran font editor dan grid | FR-UI-10, FR-ED-07 (setelan), FR-UI-06 (font grid); V-6 | `Views/SettingsView.swift`, `Support/Theme.swift` (`HelpHint`), `Support/EditorPreferences.swift`, `Support/DataPreferences.swift` | GP-s | SR, UX, AX | G-SWIFT (`EditorPreferencesTests`, `DataPreferencesTests`), G-VIS | M | `feat(settings): panes title the window, help is reachable by keyboard, and fonts have a size` |
| W9-T8 | Shell native. Implementer **GP-o**, karena struktur jendela berubah dan harness snapshot harus tetap menangkapnya. | FR-UI-08; V-7 | `Views/RootView.swift`, `Views/Workspace.swift` (toolbar), `App.swift` (jendela, ukuran minimum), `Views/ContextCascade.swift`, `Support/Snapshot.swift` | GP-o | SR, UX, AX, AR, CR | G-SWIFT, G-VIS (V-7; scene lain tetap), semua scene `--snapshot` merender, ukuran minimum diukur dan dicatat | L | `feat(app): a native split view and toolbar around the same surfaces` |
| W9-T9 | Lantai 11 pt, serial dan terakhir | FR-UI-06; V-8 | semua view yang punya pemanggilan `.ui`/`.code` di bawah 11 | GP-s | UX, AX, SR | G-VIS (V-8), tes kontras | M | `fix(a11y): nothing a person must read is smaller than 11pt` |

- **W9-D.** Bagian shell, pintasan, dan Appearance di `app/DESIGN.md`, plus `PROGRESS.md`.
- **W9-C.** Pembersihan.
- **Gate W9:** G-HEAVY, G-BENCHQ, dan tes lantai a11y.

### W10: desain grid dan editor

Lane:

- **Grid:** T1 → T2 → T3 → T4.
- **Record:** T5, setelah T1.
- **Editor:** T6 → T7.

Maksimal 3 lane berjalan bersamaan.

**W10-A1.** Blueprint `w10-grid-editor-design.md` (code-architect · opus) dengan spesifikasi UX dan AX. Pemeriksa AR.

| ID | Isi | Cakupan | Berkas | Pelaksana | Gate | Verifikasi | Ukuran | Commit |
|---|---|---|---|---|---|---|---|---|
| W10-T1 | Kursor sel keyboard, panel peek, AX per sel, pengumuman | FR-GRID-06, 07; V-9 | `Views/ResultGridTable.swift`, `Views/GridAccessibility.swift`, `Views/GridKeyboard.swift` (baru), `Views/CellPeekPanel.swift` (baru) | GP-s | SR, AX, UX, CR | G-SWIFT (tes kunci dan format label AX), G-VIS, G-BENCHQ (scroll) | L | `feat(grid): a cell cursor for the keyboard, a Space preview, and VoiceOver that reads each cell` |
| W10-T2 | Lantai kontras, Increase Contrast, tata bahasa staged, funnel | FR-GRID-08, 09, 14; V-9 | `Views/GridRowView.swift`, `Views/GridHeaderView.swift`, token grid di `Support/Theme.swift`, `Models/CellEdits.swift` (status tampilan) | GP-s | AX, UX, SR | tes kontras, G-VIS | M | `fix(grid): row numbers, NULL and the funnel meet the contrast floor, and staged rows say what they are` |
| W10-T3 | Insert/delete baris, tombol tinjau ⌘S (responder chain, P-16), undo | FR-GRID-10, 11; V-9 | `Views/ResultGrid.swift` (footer), `Views/ResultGridTable.swift` (menu), `Models/{CellEdits,WritePlan}.swift`, `Models/AppModel+Edit.swift`, `App.swift` (Save) | GP-s | SR, DB, UX, AX | G-SWIFT (`WritePlanTests`, `CellEditUndoTests`), G-LIVE PostgreSQL untuk `apply_changes` | M | `feat(grid): add and delete rows, review them with ⌘S, and undo with ⌘Z` |
| W10-T4 | Salin sebagai INSERT, drag keluar sebagai CSV | FR-GRID-12, 15 | `Models/CellSelection.swift` (`GridClipboard`), `Models/InsertStatements.swift`, `Views/ResultGridTable.swift` (sumber drag) | GP-s | DB, SR | G-SWIFT (`InsertStatementsTests`, uji pasteboard) | M | `feat(grid): copy a selection as INSERT statements, or drag it out as a CSV file` |
| W10-T5 | Mode Record | FR-GRID-13; V-11 | `Views/RecordPanel.swift` (baru), `Views/CellValueViewer.swift` (sakelar mode) | GP-s | SR, UX, AX | G-SWIFT, scene baru | M | `feat(grid): a record view that reads a whole row as fields` |
| W10-T6 | Diagnostik editor, rotor, dan readout. Posisi galat hanya dikirim bila app memasang setelan (P-06). | FR-ED-06; V-10 | `crates/qh-core/src/error.rs`, pemetaan galat di tiga driver, `crates/qh-ffi/src/events.rs`, `Views/SQLEditor.swift`, `Support/EditorDiagnostics.swift` (baru) | GP-s | RR, SR, AX, UX, CR | G-RUST, G-SWIFT, G-GOLDEN (tidak berubah) | M | `feat(editor): server errors and unclosed quotes are underlined, listed in a rotor, and read out` |
| W10-T7 | Ukuran font editor (⌘+/⌘−/⌘0), pasangan kurung, gutter recess | FR-ED-07, 08, 10; V-10 | `Views/SQLEditor.swift`, `Support/EditorPreferences.swift` | GP-s | SR, UX, AX | G-VIS, G-BENCHQ (ketikan) | M | `feat(editor): a font size you can change, matching brackets, and a gutter that recesses on light canvases` |

**W10-D** dan **W10-C**, lalu gate W10: G-HEAVY dan G-BENCHQ.

### W11: metadata, koneksi, dan MCP

**Urutan batch:**
1. T1 sendirian (lane FFI).
2. T2 (Rust) dan T4 (Swift).
3. T3 (Swift dan `mcp.rs`) dan T6 (lane FFI).
4. T5.

**W11-A1.** Blueprint `w11-metadata-and-connections.md` (code-architect · opus). Protokol TOFU di dalamnya ditinjau SEC sebelum W11-T2 dimulai.

| ID | Isi | Cakupan | Berkas | Pelaksana | Gate | Verifikasi | Ukuran | Commit |
|---|---|---|---|---|---|---|---|---|
| W11-T1 | Perintah `columns`, `ddl`, `execution_log`, dan jenis objek di `tables` (lewat setelan) | FR-TREE-01…03, FR-SAFE-02 (engine) | `crates/qh-ffi/src/metadata.rs` (baru), empat daftar invariant #11 (`lib.rs`, `uniffi_api.rs`, `Support/RustEngine.swift`), `commands.rs`, `crates/qh-driver/src/lib.rs`, tiga driver, `app/Generated/`, `tests/golden.rs`, `tests/safe_mode.rs` | GP-s | RR, DB, SF, CR | G-RUST, G-FFI, G-SWIFT, G-GOLDEN (kasus lama tidak berubah; kasus `_live` baru direkam dengan `--record`) | L | `feat(engine): read-only columns, DDL and object kinds, plus a reader for the execution log` |
| W11-T2 | SSH (alias `~/.ssh/config`, TOFU dengan fingerprint dipatok, known_hosts app), JWT Trino, CA untuk PostgreSQL dan Trino. Implementer **GP-o**, karena ini keputusan kepercayaan. | FR-CON-01…03, 07, 08 (engine) | `crates/qh-tunnel/src/{ssh_config.rs (baru),known_hosts.rs,lib.rs}`, `crates/qh-ffi/src/{tunnel.rs,config.rs,events.rs}`, `crates/qh-driver/src/lib.rs` (`ConnectionConfig`, `Debug` tersensor), `crates/qh-driver-trino/src/lib.rs`, `crates/qh-driver-postgres/src/{tls.rs,lib.rs}` | GP-o | SEC, RR, SF, AR, CR | G-RUST, G-LIVE (SSH terhadap `qh-sshd-dev`: host asing, terima dengan fingerprint benar dan salah, kunci berubah), tes TLS dengan CA uji | L | `feat(engine): SSH trust on first use pinned to a fingerprint, ~/.ssh/config aliases, Trino JWT and a per-connection CA` |
| W11-T3 | Form koneksi di app, item Keychain, prompt host key, validasi bernama, ⌘↩, Navicat SSH, pemetaan MCP | FR-CON-01…05, 07…09; V-11 | `Views/ConnectionsViews.swift`, `Views/HostKeySheet.swift` (baru), `Models/Connections.swift`, `Support/NavicatImport.swift`, `Models/AppModel+Connections.swift`, `crates/qh-ffi/src/mcp.rs` (pemetaan), `crates/qh-ffi/tests/mcp.rs` | GP-s | SEC, SR, RR, UX, AX | G-SWIFT, G-RUST, G-LIVE (Keychain), CLI end-to-end lewat tunnel: `SSH_HOST=127.0.0.1 SSH_PORT=52222 … queryhive-engine test` | L | `feat(connections): an SSH section with Keychain secrets and a host-key prompt, JWT and CA fields` |
| W11-T4 | Label jenis objek, anak kolom, dan tab DDL read-only | FR-TREE-01…03; V-11 | `Views/SchemaOutline.swift`, `Models/SchemaTree.swift`, `Models/AppModel+Tree.swift`, `Models/QueryTab.swift` (jenis tab DDL) | GP-s | SR, UX, AX | G-SWIFT, scene baru | M | `feat(tree): views are labelled, tables show their columns, and DDL opens read-only` |
| W11-T5 | Tool MCP `describe_table` dan `table_ddl` | FR-MCP-01 | `crates/qh-ffi/src/mcp.rs`, `docs/mcp-stability.md`, `crates/qh-ffi/tests/{mcp.rs,mcp_stdio.rs}` | GP-s | SEC, RR | G-RUST (allowlist kosong menolak; `read_only` tetap) | S | `feat(mcp): describe_table and table_ddl, inside the same scope and allowlist` |
| W11-T6 | Autocomplete dari katalog dengan resolusi alias (tanpa cache kedua) | FR-ED-05 | `crates/qh-sql/src/editor.rs` (API referensi tabel dan alias), `crates/qh-ffi/src/editor.rs`, `app/Generated/`, `Models/SQLSuggestions.swift`, `Models/AppModel+Completion.swift` | GP-s | RR, SR, CR | G-RUST, G-FFI, G-SWIFT (`SuggestionScopeTests`), G-BENCHQ (ketikan) | M | `feat(editor): column suggestions from the catalog, with aliases resolved` |

**W11-D** (ADR 0038, 0039, 0040) dan **W11-C**, lalu gate W11.

### W12: editor dan eksekusi

**Urutan batch:**
1. T1 (lane FFI) dan T5.
2. T3 (lane FFI, setelah T1 mendarat) dan T2 (setelah T1).
3. T4 (setelah T2 dan T3).
4. T6.

**W12-A1.** Blueprint `w12-editor-and-run.md` (code-architect · opus).

| ID | Isi | Cakupan | Berkas | Pelaksana | Gate | Verifikasi | Ukuran | Commit |
|---|---|---|---|---|---|---|---|---|
| W12-T1 | Formatter | FR-ED-01 | `crates/qh-sql/src/format.rs` (baru), `crates/qh-sql/src/lib.rs`, ekspor fungsi di `crates/qh-ffi/src/editor.rs`, `app/Generated/` | GP-s | RR, DB, CR | G-RUST (deretan token non-spasi identik, idempoten, dollar-quote, `:name`, per dialek), G-FFI | M | `feat(sql): a formatter that moves whitespace and nothing else` |
| W12-T2 | Format, toggle comment, Save dan Save As, tab terikat berkas, deteksi perubahan dari luar, drop `.sql`, restore | FR-ED-01…04, PR-16 | `Models/QueryTab.swift`, `Models/Session.swift`, `Models/AppModel+Files.swift` (baru), `App.swift`, `Views/SQLEditor.swift`, `Views/Workspace.swift` | GP-s | SR, UX, AX | G-SWIFT (`SessionTests`: restore tanpa menjalankan) | M | `feat(editor): format, toggle comment, and .sql tabs that save back to their file` |
| W12-T3 | Perintah engine `script` | FR-RUN-01 | `crates/qh-ffi/src/script.rs` (baru), empat daftar invariant #11, `app/Generated/`, `Support/EngineWire.swift`, `tests/golden.rs` (cursor palsu), `tests/safe_mode.rs` | GP-s | RR, DB, SF, SEC, CR | G-RUST, G-FFI, G-SWIFT, G-LIVE PostgreSQL (stop, continue, cancel di dalam statement) | L | `feat(engine): a script command that runs statements one by one with a stop or continue policy` |
| W12-T4 | UI Run Script, result set per statement, pin, satu entri history | FR-RUN-01…03; V-11 | `Models/QueryTab.swift` (result set berisi handle store), `Views/ResultGrid.swift` (strip tab hasil), `Models/AppModel+Run.swift`, `Views/Panels.swift` | GP-s | SR, UX, AX, SF, AR | G-SWIFT, scene baru, memori tetap di dalam anggaran global | L | `feat(app): Run Script shows each statement's outcome and keeps a result per statement` |
| W12-T5 | Impor `.sql` dan JSON/JSONL | FR-IMP-01, 02 | `crates/qh-import/src/json_source.rs` (baru), `crates/qh-import/src/lib.rs`, `crates/qh-ffi/src/import.rs`, `Views/ImportSheet.swift`, `Models/ImportMapping.swift`, `crates/qh-ffi/tests/import_json.rs` (baru) | GP-s | RR, DB, SR, SF | G-RUST, G-SWIFT (`ImportMappingTests`), G-LIVE PostgreSQL (ketiga kebijakan) | M | `feat(import): .sql and JSON files from the app, under the same transaction policy` |
| W12-T6 | Notifikasi, pengumuman VoiceOver, progres Finder dan Dock | FR-RUN-04, 05 | `Support/LongRunNotifier.swift` (baru, di belakang protokol), `Support/FileProgress.swift` (baru), `Models/AppModel+Export.swift`, hook di `Models/AppModel+Run.swift` | GP-s | SR, AX, UX | G-SWIFT (notifier palsu). Pengiriman nyata diperiksa pemilik. | M | `feat(app): long exports report progress in Finder and the Dock, and say when they finish` |

**W12-D** (ADR 0041, 0042) dan **W12-C**, lalu gate W12.

### W13: paginasi, rencana query, aktivitas, audit, lokalisasi

**Urutan batch:**
1. T1, T2, dan T6.
2. T3 (setelah T1 dan T2) dan T4 (lane FFI, setelah T2, karena keduanya menyentuh berkas driver).
3. T5.
4. T7 sendirian.

**W13-A1.** Blueprint `w13-plan-activity.md` (code-architect · opus). Termasuk verifikasi dukungan `EXPLAIN ANALYZE (FORMAT JSON)` di Trino 483 dalam container. Bila tidak didukung, ANALYZE untuk Trino tetap berupa teks.

| ID | Isi | Cakupan | Berkas | Pelaksana | Gate | Verifikasi | Ukuran | Commit |
|---|---|---|---|---|---|---|---|---|
| W13-T1 | Paginasi (P-07) dan "Tampilkan SQL" | FR-GRID-02, 05; V-11 | `Views/ResultGrid.swift` (footer), `Models/QueryTab.swift`, `Models/AppModel+Run.swift` | GP-s | SR, UX, AX | G-SWIFT, G-VIS | M | `feat(grid): fetch more, fetch all and go to row, with the row limit as the one cap` |
| W13-T2 | EXPLAIN JSON dan ANALYZE di engine (lewat setelan, dengan guard statement di dalamnya) | FR-PLAN-01 | `crates/qh-ffi/src/commands.rs`, ejaan explain di driver, `crates/qh-sql/src/classify.rs`, `tests/safe_mode.rs` | GP-s | RR, DB, SEC | G-RUST, G-GOLDEN (tidak berubah), G-LIVE (PostgreSQL, Trino) | M | `feat(engine): JSON plans and EXPLAIN ANALYZE, guarded like the statement they run` |
| W13-T3 | Pohon rencana | FR-PLAN-02; V-11 | `Models/QueryPlan.swift` (baru), `Views/PlanTreeView.swift` (baru), `Views/ResultGrid.swift` (sakelar Tree/Raw) | GP-s | SR, UX, AX | G-SWIFT (parse fixture JSON PostgreSQL dan Trino), scene baru | M | `feat(app): plans read as a tree with the hottest node marked` |
| W13-T4 | Perintah `sessions` dan `session_cancel` dengan guard sendiri (P-12) | FR-SAFE-03 (engine) | `crates/qh-ffi/src/activity.rs` (baru), empat daftar invariant #11, `app/Generated/`, SQL aktivitas di driver, `tests/safe_mode.rs` | GP-s | DB, SEC, RR, SF | G-RUST, G-FFI, G-SWIFT, G-LIVE (cancel pada `pg_sleep` di sesi lain) | M | `feat(engine): list server sessions and cancel one, under Safe Mode` |
| W13-T5 | Tampilan aktivitas server | FR-SAFE-03; V-11 | `Views/ServerActivity.swift` (baru), `Models/AppModel+Activity.swift` (baru), `Views/SchemaOutline.swift` (menu) | GP-s | SR, UX, AX | G-SWIFT, scene baru | M | `feat(app): a server activity view with cancel` |
| W13-T6 | Penampil execution log | FR-SAFE-02; V-11 | `Views/ExecutionLogView.swift` (baru), `Views/Panels.swift` (tab Audit), `Models/AppModel+Audit.swift` (baru) | GP-s | SR, UX, AX, SEC | G-SWIFT | S | `feat(safety): read the execution log and whether its chain holds` |
| W13-T7 | Fondasi lokalisasi (P-20), serial dan terakhir | FR-UI-11 | `app/Package.swift` (`defaultLocalization`), `app/Resources/` (katalog, baru), `app/build.sh` (menyalin atau mengompilasi string ke `Contents/Resources`), `Support/L10n.swift` (baru), string di berkas yang disentuh W9–W13 | GP-s | SR, UX, AX | G-SWIFT, G-VIS tidak berubah, G-APP (bundel memuat string dari katalog) | M | `feat(app): a string catalog, and the strings in the new surfaces go through it` |

**W13-D** (ADR 0043, 0044) dan **W13-C**, lalu gate W13.

### W14: final

Lihat §9.

## 6. Pemetaan subagen

| Peran | Agen | Model | Dipakai di |
|---|---|---|---|
| Implementasi Rust, Swift, dan Python | general-purpose | sonnet. **opus** untuk W3-T1, W3-T2, W5-T1, W5-T2, W6-T1, W9-T8, dan W11-T2 (alasannya di masing-masing tugas) | semua `Tx` implementasi |
| Tes lebih dulu | tdd-guide | sonnet | W2-T1, W3-T1 |
| Rangkaian tes dan fixture | test-engineer | sonnet | W0-T3, W1-T3, W1-T5, W4-T4, W14-T3 |
| Harness dan sesi bench | performance-engineer | sonnet. **opus** untuk W1-T8 (interpretasi profil) | W1, `Wx` bench |
| Verdict angka, keputusan Fase 7 dan 8 | performance-optimizer | opus | semua gate bench, W8-T2 |
| Blueprint | code-architect | opus | `Wx-A*` |
| Verdict arsitektur | architect-reviewer | opus | blueprint dan tugas L |
| Arbitrase, eskalasi Fase 8 | architect | opus | W8-T2, kebuntuan reviewer |
| Re-plan setelah blokir L atau kontingen | planner | opus | W8-T2, §8 |
| Review Rust | rust-reviewer | opus | setiap tugas Rust |
| Review Swift | swift-reviewer | opus | setiap tugas Swift |
| Review umum per fase | code-reviewer | opus | setiap fase performa, pembersihan, W14-T4 |
| UI | ui-ux-designer | opus | spesifikasi W9–W13, gate UI, rekam ulang V |
| Aksesibilitas | a11y-architect | opus | spesifikasi W9–W10, gate UI |
| Keamanan | security-reviewer | opus | FFI, spill, SSH, Keychain, JWT, CA, MCP, guard aktivitas, Safe Mode |
| SQL | database-reviewer | opus | reset pool, COPY, metadata, `script`, EXPLAIN, aktivitas, INSERT, formatter, SQL pembungkus Batch 7 |
| Jalur galat | silent-failure-hunter | opus | cancel, pool, spill, `script`, impor |
| Desain tipe | type-design-analyzer | opus | `EngineHost`, `ResultRows`, `EditorDocument`, `ResultHandle` |
| Galat build | swift-build-resolver, rust-build-resolver | sonnet | §8 |
| Bug keras dan gate merah | debugger | opus | §8 |
| ADR | adr-generator | sonnet | `Wx-D` |
| Dokumen | doc-updater | sonnet | `Wx-D`, W14-T5 |
| Kode mati | refactor-cleaner | sonnet | `Wx-C` |
| Penyederhanaan | code-simplifier | sonnet | `Wx-C` |
| Pencarian untuk menyusun brief | Explore | haiku | orkestrator, sebelum setiap tugas |

## 7. Paralelisme dan kepemilikan berkas

**Aturan dasar.** Satu berkas bersama hanya boleh dimiliki satu tugas yang sedang berjalan. Urutan di tabel ini mengikat.

| Berkas atau kelompok | Pemilik, berurutan |
|---|---|
| `app/Generated/`, `crates/qh-ffi/src/uniffi_api.rs`, empat daftar invariant #11, `Support/RustEngine.swift` (lane FFI) | W2-T1 (bila berubah) → W3-T1 → W4-T2 → W5-T2 → W6-T1 → W11-T1 → W11-T6 → W12-T1 → W12-T3 → W13-T4 |
| `crates/qh-ffi/src/commands.rs` | W2-T1 → W3-T1 → W5-T2 → W11-T1 → W13-T2 |
| `crates/qh-ffi/src/lib.rs` (baris `mod` dianggap boleh digabung tangan) | ikut lane FFI |
| Tiga driver | W3-T1 → W7-T1 → W7-T2 dan W7-T4 (PostgreSQL dan Trino, berkas terpisah) → W7-T3 → W10-T6 → W11-T1 → W11-T2 → W13-T2 → W13-T4 |
| `crates/qh-ffi/src/mcp.rs` | W11-T3 → W11-T5 |
| `Cargo.toml`, `Cargo.lock` | W4-T3 → W7-T2 → W7-T5 |
| `tests/golden/`, `crates/qh-ffi/tests/golden.rs` | W2-T1 → W11-T1 → W12-T3 |
| `Models/AppModel.swift` | W1-T4 → W2-T2 → W3-T1 → W4-T1 → W6-T1 → W9-T0. Sesudahnya, satu pemilik per extension. |
| `Models/QueryTab.swift` | W2-T2 (bila perlu) → W4-T1 → W5-T1 → W6-T1 → W11-T4 → W12-T2 → W12-T4 → W13-T1 |
| `Views/ResultGrid.swift` | W1-T4 → W4-T1 → W5-T1 → W9-T5 → W10-T3 → W12-T4 → W13-T1 → W13-T3 |
| `Views/ResultGridTable.swift`, `GridRowView`, `GridHeaderView` | W5-T1 → W6-T1 → W10-T1 → W10-T2 → W10-T3 → W10-T4 |
| `Views/SQLEditor.swift` | W1-T4 → W2-T3 → W4-T2 → W10-T6 → W10-T7 → W12-T2 |
| `Views/Workspace.swift` | W2-T3 → W9-T1 → W9-T4 → W9-T8 → W12-T2 |
| `App.swift` | W1-T4 → W6-T1 → W9-T2 → W9-T8 → W10-T3 → W12-T2 |
| `Support/Theme.swift` | W9-T1 → W9-T7 → W10-T2 |
| `Views/ConnectionsViews.swift`, `Models/Connections.swift` | W9-T4 → W11-T3 |
| `Support/Snapshot.swift` | W1-T3 → W5-T1 → W9-T8 |
| `__Baselines__/` | hanya lewat langkah merge orkestrator (§0.5) |

**Freeze** (`performance-plan.md` §0). Selama Fase 4 dan 5 tidak ada fitur yang menyentuh `SQLEditor.swift` atau `ResultGrid.swift`. Urutan gelombang di atas sudah memenuhinya: fitur baru dimulai di W9.

**Worktree dipakai** di W2 (T1, T2, T3), W4 (T1, T2, T3), W5 (T1, T2), dan W7 (T2, T4). Di W9–W13, worktree dipakai setiap kali dua lane berjalan bersamaan. Tugas serial dikerjakan di checkout utama.

## 8. Gate dan penanganan kegagalan

**Gate tugas.**

- Verifikasi tugas lulus, dan setiap reviewer yang disebut memberi verdict **approved** dengan daftar berkasnya (aturan "done means verified").
- `./app/build.sh` dijalankan di gate gelombang, kecuali tugas itu menyentuh `app/build.sh`, `Package.swift`, atau resource.

**Gate gelombang.**

- Semua tugas sudah lulus atau tercatat terblokir.
- Kepala branch menjalankan G-HEAVY.
- Gate angka gelombang itu dan G-BENCHQ (mulai W9).
- Pembersihan sudah di-commit.
- `PROGRESS.md` sudah diperbarui.

**Yang memblokir gelombang berikutnya:**

- gate fungsional merah di kepala branch;
- tugas yang menjadi dependensi langsung terblokir (hanya tugas yang bergantung padanya yang dilewati; sisanya jalan terus).

Gate angka yang meleset tidak memblokir (P-21). Gate itu diperiksa PO satu putaran, lalu masuk W8-T2 bila ada eskalasi yang cocok. Bila tidak ada, ia dicatat sebagai "meleset" beserta angkanya.

**Loop kegagalan:**

1. **Galat build:** swift-build-resolver atau rust-build-resolver, maksimal 3 putaran.
2. **Tes atau gate merah:** debugger (opus) mendiagnosis, maksimal 2 putaran. Implementer memperbaiki sekali berdasarkan diagnosis itu.
3. **Reviewer meminta perubahan:** implementer memperbaiki lalu review ulang, maksimal 2 putaran. Bila masih buntu, `architect` memutuskan sekali.
4. **Bila semua putaran habis:**
   - buang perubahan tugas itu (worktree dihapus, atau `git restore` pada berkas milik tugas itu saja);
   - tandai **TERBLOKIR** di ledger dengan bukti, lalu lewati tugas yang bergantung padanya;
   - `planner` (opus) menata ulang sisa gelombang bila yang terblokir adalah tugas L;
   - lanjutkan pekerjaan yang independen.
5. **Gate gelombang merah akibat commit yang sudah masuk:** orkestrator me-`git revert` commit penyebabnya (bisect per commit tugas), lalu tugas itu kembali ke langkah 2.
6. **Lingkungan rusak** (container mati, disk penuh):
   - pulihkan dengan `deploy/dev/up.sh` dan coba ulang sekali;
   - bila disk penuh, hapus artefak sendiri saja: scratchpad, DerivedData TablePro, worktree dan `target/`-nya;
   - data pengguna tidak pernah disentuh.

## 9. Gelombang akhir (W14) dan laporan akhir

| ID | Isi | Pelaksana | Verifikasi |
|---|---|---|---|
| W14-T1 | G-HEAVY penuh, termasuk G-GOLDEN dengan Trino dan semua G-LIVE. `./app/build.sh` dan semua scene `--snapshot`. | test-engineer · sonnet | semua hijau, hitungan tes dicatat |
| W14-T2 | Sesi bench final untuk semua sumbu (eksklusif), TablePro best-effort, laporan diregenerasi | performance-engineer · sonnet; PO | `docs/benchmarks.md` hasil generator |
| W14-T3 | Laporan paritas visual: baseline P, lalu setelah Fase 5 dan 6, lalu final. Pasangan berdampingan untuk setiap V ditulis ke `app/.build/parity-report/` (tidak di-commit), dengan indeks. | test-engineer · sonnet; UX | semua scene non-V lulus |
| W14-T4 | Pass perbaikan akhir dan review per area atas diff branch (engine, store, grid, editor, shell, koneksi), plus review keamanan atas semua diff yang relevan | refactor-cleaner, code-simplifier · sonnet; CR, SEC · opus | approved per area |
| W14-T5 | Dokumen: `PROGRESS.md`, `app/DESIGN.md` (Scope, grid, editor, shell, CLI, Stop, formatter, skrip), `docs/invariants.md` (hanya insiden nyata), status di `performance-plan.md`, `remaining-work-plan.md`, `tablepro-adoption-plan.md`, `tablepro-feature-map.md`, dan `tablepro-design-audit.md` | doc-updater · sonnet | CR |
| W14-T6 | Merge: G-HEAVY di branch, `git switch main && git merge --no-ff work/perf-parity -m "merge: performance, design and parity work"`, G-HEAVY sekali lagi di `main` | orkestrator | hijau |

**Isi laporan akhir untuk pemilik:**

1. **Status:**
   - tugas yang selesai (ID dan hash commit);
   - tugas yang terblokir (dengan bukti);
   - tugas kontingen Fase 8 yang dijalankan atau tidak.
2. **Angka per sumbu:** target, QueryHive, TablePro (atau `tidak diukur (izin OS)` dan `tidak diukur (butuh sudo)`), verdict, dan rujukan ke `docs/benchmarks.md`.
3. **Paritas visual:** daftar scene yang lulus, dan pasangan berdampingan untuk V-1…V-11 yang perlu ditinjau. Setiap rekam ulang punya commit sendiri, jadi bisa di-revert.
4. **Status UC-01…UC-18** dan bukti otomatisnya.
5. **Keputusan perencana P-01…P-26** yang bisa dibatalkan.
6. **Tindakan pemilik:**
   - beri izin Screen Recording dan Accessibility, setel koneksi TablePro, lalu jalankan `qhbench` (perintahnya dicantumkan);
   - `sudo purge` untuk start dingin;
   - smoke VoiceOver dan IME;
   - izin dan pengiriman notifikasi;
   - drag and drop;
   - prompt host key terhadap bastion sungguhan;
   - meninjau baseline.
7. **Lingkungan:**
   - VM podman sebelum dan sesudah resize;
   - alat yang dipasang (`samply`, XcodeGen);
   - `competitor_rev`;
   - lokasi build TablePro, di luar repo.
8. **Batas yang dinyatakan:** CA MySQL, `ProxyJump`, terminate, dan daftar "di luar lingkup" PRD §10.
9. Tidak ada push, PR, maupun rilis.

## 10. Ukuran dan titik rawan

**Jumlah tugas.** Sekitar 112: 59 implementasi, 10 blueprint, 14 ADR/dokumen, 13 pembersihan, 10 bench, dan 6 final. Bila Fase 8 terpicu, ada tambahan 0–4 tugas kontingen berukuran L. Tugas berukuran L ada 14:

- W3-T1, W4-T3, W5-T1, W5-T2, W6-T1;
- W9-T3, W9-T8;
- W10-T1;
- W11-T1, W11-T2, W11-T3;
- W12-T3, W12-T4;
- W1-T10, dihitung L karena lama berjalan.

**Paling mungkin meledak, berurutan:**

1. **W5-T1, grid.** Daftar paritasnya paling luas, dan fokus, IME, serta AX bertemu di sini. Karena itu dipecah menjadi dua commit (5a seam, 5b tabel).
2. **W6-T1, integrasi data plane.** Penghapusan besar di Swift dan umur handle per tab. Tes diferensial harus lulus sebelum kode Swift dihapus.
3. **W3-T1, `EngineHost`.** Kebocoran state yang diam, pool bersama tunnel, dan reconnect.
4. **W3-T2 dan W4-T2, editor 4B.** Paritas lexer dan pemetaan UTF-16.
5. **W9-T8, shell native.** Mengubah struktur jendela, dan harness snapshot harus tetap bisa menangkapnya.
6. **W12-T3 dan W12-T4, skrip dan result set.** Protokol event baru ditambah UX baru.
7. **W11-T2 dan W11-T3, SSH.** Kepercayaan dan Keychain. Pengujian ujung ke ujung bergantung pada `qh-sshd-dev`.
8. **W13-T7, lokalisasi.** Resource bundel SwiftPM di app yang dirakit `build.sh` belum pernah dicoba di repo ini.
9. **W1-T9, build TablePro.** Butuh jaringan dan XcodeGen.
10. **W1-T6 dan W1-T10, harness black-box.** Terhalang izin OS. Hasilnya tercatat dan tidak menggagalkan gate.

## 11. Yang masih terbuka

1. **Tindakan pemilik setelah run, tidak memblokir:** izin OS dan koneksi TablePro untuk head-to-head, `sudo purge` untuk start dingin, smoke manual (VoiceOver, IME, notifikasi, drag and drop), dan tinjauan paritas V-1…V-11.
2. **Dukungan `EXPLAIN ANALYZE (FORMAT JSON)` di Trino 483.** Diverifikasi di W13-A1, dan fallback-nya (teks) sudah ditetapkan. Ini bukan pertanyaan untuk pemilik.
3. **Plafon kawat VM (gvproxy).** Menentukan apakah sumbu 2 dinilai dengan 575k atau dengan 80% plafon. Aturannya sudah ada di `performance-plan.md` §2. Yang belum ada hanya angkanya, yang baru diketahui di W1-T10.

Tidak ada pertanyaan produk yang tersisa. Semua keputusan yang diperlukan tercatat sebagai O-* dan P-* di PRD §11.
