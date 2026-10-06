# Rencana pengembangan: performa, desain, dan fitur untuk eksekusi orkestrator

- **Status:** rencana eksekusi, 29 Sep 2026. W0 sampai W2 sudah mendarat di branch `work/perf-parity` (`3ba01ae..2e6f7ef`, 30 Sep 2026); W3-T0 (celah Safe Mode MySQL) sedang dikerjakan, lalu W3. Rincian di `PROGRESS.md` §"Run perf-parity".
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

### 0.3 Lane model dan konkurensi

**Lane types** (O-22):
- **MAIN:** the FFI lane and integration. It runs build-ffi, G-FFI, G-GOLDEN, G-APP, G-LEAK, full-workspace G-RUST and the X/Q exclusive slots.
- **R1–R3:** Rust worktrees sharing `CARGO_TARGET_DIR`, `cargo test -p <touched>` only (reduce artifact bloat from multiple worktrees' binaries).
- **S1–S3:** Swift worktrees, about 1 GB each. They copy `target/ffi/static/debug` from MAIN, re-copy after every Generated change, and never run build-ffi themselves.
- **D1–D2:** docs agents on haiku (O-23).
- At most 8 concurrent.

**Live slot:** only one lane at a time runs G-LIVE or G-GOLDEN (shared dev DBs). Tests drop their own tables.

**Exclusive slots (X/Q)** stop every lane and reviewers.

**Integration:** lane commit → cherry-pick into MAIN → the task's MAIN-only gates → push (O-21). Remove the worktree only after the commit and a clean status, never with `--force` (I-2).

- **Maksimal 3 implementer bersamaan.** Architect, reviewer, dan Explore tidak dihitung, tetapi tidak boleh berjalan saat sesi benchmark atau X-slot (§0.4).
- **Worktree (dimatikan di run ini).** Pada 30 Sep 2026 disk bebas host 30 GiB (< 60, §2.1), jadi implementer berjalan serial di checkout utama. Aturan di bawah berlaku bila disk cukup.
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
- **Rekam ulang** hanya untuk scene yang terdaftar sebagai perubahan V-1…V-12 di PRD §6.5, dan hanya oleh tugas pemilik perubahan itu. Syaratnya:
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
| G-DENY | `cargo deny check licenses`. Sejak W13-T8a ditambah `cargo deny --manifest-path helpers/analytics/Cargo.toml check licenses --config deny.toml`. | dependensi berubah, dan setiap gate gelombang |
| G-FFI | `./app/build-ffi.sh`, lalu `app/Generated/` ikut di-commit, lalu `cd app && swift test --filter RustEngineTests` | permukaan UniFFI atau daftar perintah berubah |
| G-SWIFT | `cd app && swift build && swift test` | setiap tugas Swift |
| G-VIS | `cd app && swift test --filter VisualParityTests` | setiap tugas yang menyentuh view |
| G-GOLDEN | `cargo build --bin queryhive-engine && /usr/bin/python3 tools/golden/live_cases.py` | container hidup. Setiap selisih harus terklasifikasi di `docs/golden-deltas.md`, dan selisih baru dianggap regresi. |
| G-LIVE | tes live di crate yang disentuh, dengan penjaga `QH_TEST_*` yang ada di crate itu: `QH_TEST_POSTGRES=1 cargo test -p qh-ffi --test real_server` dan `-p qh-driver-postgres`, `QH_TEST_TRINO=1 cargo test -p qh-driver-trino`, `deploy/dev/qh-sshd-run.sh && QH_TEST_SSH=1 cargo test -p qh-tunnel`, `QH_TEST_KEYCHAIN=1 cargo test -p qh-credentials` | tugas yang menyentuh driver, tunnel, atau Keychain |
| G-APP | `./app/build.sh && app/dist/QueryHive.app/Contents/MacOS/QueryHive --snapshot "$SCRATCH/smoke.png" --scene done` | gate gelombang, dan tugas yang menyentuh `app/build.sh`, `Package.swift`, atau resource |
| G-BENCH(a) | `python3 deploy/dev/bench_app.py --axis <a> --label <fase>-<yyyymmdd> --repeat <n>`, lalu `python3 deploy/dev/bench_fetch.py --report-only`. Untuk sumbu 3, rekaman mencatat `spilled_bytes` di samping memori (R-29), supaya hasil yang tumpah atau tidak tidak dibaca sebagai regresi. | tugas bench (eksklusif) |
| G-BENCHQ | subset cepat `--bench ttfr-pg,scroll-1m,type-10k` dengan n = 5, dibanding rekaman gelombang sebelumnya | gate W9–W13 (NFR-P9) |
| G-LEAK | `MallocStackLogging=1 leaks --atExit -- app/.build/debug/QueryHive --bench tabs-100`, lalu kriterianya `stores == 0`, `spilled_bytes == 0`, `open_fds` sesudah == sebelum, dan `leaks --atExit` bersih setelah frame sistem (`com.apple.linkd.autoShortcut`, `NSXPCConnection`) disaring, dibaca dari `store_stats()` di akhir `tabs-100` (berkas spill di-unlink saat dibuat, jadi pemeriksaan direktori spill hampa; `spilled_bytes` nyata sejak `1b59154` dan dihitung atas store yang masih hidup, jadi nol di akhir mengikuti `stores == 0`). Ditambah skenario `tabs-100-held` (100 tab terbuka bersamaan, masing-masing dengan store yang tumpah): `open_fds_peak` (dibaca lewat `proc_pidinfo(PROC_PIDLISTFDS)`) dicatat sebelum, puncak, dan sesudah semua tab ditutup, dan hitungan fd akhir harus kembali ke nilai awal (ukuran R-19, blueprint Fase 6 §17.6 dan §19) | Fase 6, dan final |
| G-ANALYTICS | `cargo test --manifest-path helpers/analytics/Cargo.toml && cargo build --release --manifest-path helpers/analytics/Cargo.toml && cargo deny --manifest-path helpers/analytics/Cargo.toml check licenses --config deny.toml`, ditambah `cargo tree --manifest-path helpers/analytics/Cargo.toml -e normal` yang tidak boleh memuat `reqwest`, `hyper`, `h2`, atau `rustls`. Ukuran helper stripped dan terkompresi dicatat. | W13-T8a, W13-T8b, gate W13, dan W14 |
| **G-HEAVY** | G-RUST + G-DENY + G-FFI + G-SWIFT + G-VIS + G-GOLDEN + G-APP. Untuk W13 dan W14 saja, ditambah G-ANALYTICS. | penutupan setiap gelombang |

## 2. Lingkungan (W0-T2)

Fakta yang sudah diverifikasi orkestrator:

- Apple M4, 10 core;
- `cargo`, Swift 6.4, dan `xcodebuild` tersedia;
- `xcodegen` dan `samply` tidak terpasang;
- mesin podman hidup dengan 2 CPU / 1,86 GiB dan belum punya container.

1. **Ukur host.** `sysctl -n hw.memsize hw.model hw.perflevel0.physicalcpu hw.perflevel1.physicalcpu` dan `df -h`.
   - Syarat ruang disk: ≥ 60 GB bebas.
   - Bila kurang: worktree dibatasi 1 dan build TablePro dilewati (dicatat).
   - **Tercatat 30 Sep 2026:** disk bebas host 30 GiB (< 60). Worktree dimatikan (implementer serial di checkout utama) dan W1-T9 (build TablePro) dilewati. Semua sumbu TablePro ditulis `tidak diukur (izin OS)`.
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
| W3 | **W3-T0** (Safe Mode MySQL), **Fase 2**, dan **Fase 4B-core**, plus blueprint Fase 5 | W2 | 7 | tes kebocoran state, golden tidak berubah, tes Safe Mode MySQL, G4 (refaktor `scan.rs`), verdict architect-reviewer |
| W4 | **Fase 3** (Batch 7), **Fase 4B-integrasi**, **Fase 6-core** | W3 | 8 | G7 dan G8 (penerapan dan palet editor), V-12, tes perilaku Batch 7, tes enkripsi spill |
| W5 | **Fase 5** (grid), dan **Fase 6-engine** di Rust | W4 | 5 | paritas visual grid, daftar paritas fitur, tes AX |
| W6 | **Fase 6-Swift** (integrasi data plane) | W5 | 5 | tes diferensial, G-LEAK, paritas terhadap baseline Fase 5 |
| W7 | **Fase 7** (plafon ingest, opsi bersyarat) | W6 | 7 | golden dan `type_zoo` live identik |
| W8 | Penutupan performa, keputusan **Fase 8**, perbaikan O-31, dan penutupan utang | W7 | 13 (+ E2s, E4, dan 0–4 kontingen) | laporan performa lengkap |
| W9 | Fondasi desain: P0 chrome, keyboard, pohon, shell. Plus pemecahan `AppModel` | W8 | 12 | G-HEAVY, G-BENCHQ, lantai a11y |
| W10 | Desain grid dan editor di atas komponen baru | W9 | 12 | G-HEAVY, G-BENCHQ |
| W11 | Metadata, SSH, JWT dan CA, MCP, autocomplete | W10 | 11 | G-HEAVY, G-LIVE (SSH, TLS) |
| W12 | Formatter, berkas `.sql`, script runner, impor, notifikasi | W11 | 19 | G-HEAVY |
| W13 | Paginasi, rencana query, aktivitas server, audit, helper analitik (mesin), lokalisasi | W12 | 23 | G-HEAVY, G-ANALYTICS |
| W14 | Final: gate penuh, benchmark, paritas, dokumen, panduan agen, merge, laporan | W13 | 12 | kriteria rilis PRD §9 |

**Total: sekitar 155 tugas**, sekitar 93 di antaranya implementasi (sebelum adopsi dbx, tugas perbaikan W8 dari O-31, dan register utang: 117 dan 63). Rinciannya di §10.

## 4. Jalur arsitektur dan ADR

**Blueprint.** `code-architect` (opus) menulis blueprint tingkat berkas ke `docs/architecture/blueprints/<nama>.md` **sebelum** tugas mulai. `architect-reviewer` (opus) memeriksa blueprint itu, lalu memberi verdict atas implementasinya **sesudah** tugas selesai. Blueprint di-commit bersama commit dokumen gelombangnya.

| Blueprint | Ditulis di | Untuk | Isi wajib |
|---|---|---|---|
| `fase-2-engine-host.md` | W2-A1 | W3-T1 | Bentuk UniFFI `EngineHost`. Pool 2+1 (O-7). Semantik reset. Operasi panjang di luar pool (P-05). Tunnel per kunci. Handle SQLite. Warm-up. Hemat RTT. Tes reset. |
| `fase-4b-editor-analysis.md` | W2-A2, direvisi 30 Sep 2026 (O-14) | W3-T0, W3-T2, W4-T2 | Tree-sitter per statement di `qh-editor` dan `qh-sql-grammar` (vendor). `walk` dan `lex` di `qh-sql`. Atribut sementara dan pemilik kunci sementara. API `EditorDocument`. Penjaga IME. Hook rotor (FR-ED-09). V-12. W3-T0 (Safe Mode MySQL). Daftar yang dihapus. |
| `fase-6-data-plane.md` | W2-A3, direvisi 30 Sep 2026 (O-15, O-18), disegarkan di W6-A1 | W4-T3, W5-T2, W6-T1, W7-T1, W13-T8a–c | Store Arrow per chunk (`qh-columnar`). Spill terenkripsi (NFR-S3). `ResultHandle` dan `StoreId`. `set_view`. `StoreRows`. "Off" Batch 7 dengan dua store per tab. Helper analitik (§14). |
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
| 0033 | Analisis editor dengan tree-sitter per statement di Rust. Isi: D-1 sampai D-21, grammar yang di-vendor dan patch-nya, kebijakan diagnostik, pemilik kunci atribut sementara, V-12, risiko C di dalam proses, dan MSRV. | W4-D |
| 0037 | Spill terenkripsi (IPC Arrow + AES-256-GCM) dengan kunci efemeral. Satu kunci per proses yang menulis spill, dan kunci tidak pernah menyeberang proses (blueprint Fase 6 §23). | W4-D |
| 0032 | Grid dengan sel yang digambar (mengamandemen 0003) | W5-D |
| 0030, amandemen 0004, menggantikan 0008 dan 0013 | Data plane app: store Arrow per chunk (`qh-columnar`), `window()` terender, Explain di store (blueprint Fase 6 §23). | W6-D |
| 0034 | View in-memory di Rust, kolasi, divergensi. DataFusion untuk SQL di helper opsional, bukan untuk view grid. | W6-D |
| 0035, 0036, amandemen 0028 | Bersyarat, hanya bila diadopsi | W7-T6 |
| 0038 | Perintah baca baru dan keluaran yang digerbangi setelan (P-06) | W11-D |
| 0039 | SSH di app, TOFU dengan fingerprint dipatok | W11-D |
| 0040 | JWT Trino dan CA per koneksi, termasuk batas MySQL | W11-D |
| 0041 | Formatter | W12-D |
| 0042 | Script runner dan result set berganda | W12-D |
| 0043 | Aktivitas server dan guard cancel | W13-D |
| 0044 | Paginasi dan cap tunggal | W13-D |
| 0045 | DataFusion sebagai komponen analitik terpisah yang diunduh saat pertama dipakai (O-15, O-18) | W13-D |
| Adendum 0007, 0010, 0014 | App tetap tanpa sandbox dan helper dikurung `sandbox-exec` (0007). Runtime tokio helper ber-QoS (0010). Helper tanpa entitlement (0014). | W13-D |

**Pembersihan di akhir setiap gelombang (`Wx-C`).**

- `refactor-cleaner` (sonnet) membuang kode mati, dibatasi ke berkas yang disentuh gelombang itu.
- `code-simplifier` (sonnet) menyederhanakan area yang sama tanpa mengubah perilaku.
- Gate: G-HEAVY, dan satu reviewer sonnet (risiko rendah, O-17).
- Commit `refactor(<area>): …`, atau tidak ada commit bila tidak ada perubahan.

## 5. Tugas per gelombang

**Singkatan reviewer (opus, kecuali tugas berisiko rendah yang memakai satu reviewer sonnet, O-17):**

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

### Adopsi dbx (2026-10-06)

- **Sumber.** `target/run/dbx-adoption-backlog.md` (DBX-1 sampai DBX-78 dan PF-1 sampai PF-27, dibaca di `da36406`), lanjutan `target/run/debt-register.md` §5 dan §8. Baris bukti setiap item ada di backlog dan dikutip di brief tugas, tidak diulang di sini.
- **dbx hanya bahan studi** (`/Users/isal/Workspaces/Lab/Experiments/dbx`, Apache-2.0), sama seperti TablePro: yang diambil hanya ide, angka, dan nama kasus uji. Kode, aset, dan string dbx tidak disalin, dan korpus uji ditulis ulang dengan kata-kata sendiri. Aturan ini masuk setiap brief di bawah, di samping aturan TablePro di §0.2.
- **Urutan.** Item Tier 0 (keamanan dan kehilangan data) didahulukan begitu rantai §7-nya bebas: W8-T3, W10-T3, W10-T8, W11-T2 (DBX-1), W11-T3s (DBX-29, PF-5, PF-6), W11-T7, W12-T7a, W12-T8, W12-T9, dan W13-T2 (DBX-3, DBX-4).
- **Angka.** Item performa (DBX-5, DBX-6, DBX-39, DBX-40, DBX-41) dipertahankan hanya bila A/B QueryHive memenuhi aturan W7 (≥ 10% pada sumbunya tanpa regresi di sumbu lain). Angka dbx bukan bukti.
- **Bersyarat keputusan:** W12-T7d (DBX-8), W13-T10 (DBX-36), W13-T11 (DBX-37), W13-T17 (DBX-38), W13-T18 (DBX-75), langkah 2 DBX-30 (W12-T7b), replay SAVEPOINT DBX-39 (W12-T7c), lebar kolom yang bertahan antar-launch DBX-64 (W10-T9), sikap baris pendek PF-4 (W12-T7a), dan perluasan lingkup W14-T5 (DBX-78). Keputusan dicatat sebagai O-* baru di ledger (O-31b); yang menambah FR juga dicatat di PRD §11. Item yang tidak diadopsi ditutup dengan alasan tertulis.
- **Berlaku sekarang (DBX-43).** Sebelum setiap push integrasi, orkestrator membaca `gh run list --branch main -L2`. Main yang merah dicatat sebagai insiden I-* (job, SHA, galat pertama) dan memblokir push berikutnya, tanpa rerun untuk menyembunyikannya. Menurut backlog DBX-42 main sudah merah sejak 2026-10-05T22:34Z, jadi push berikutnya harus membawa W8-T3.
- **String UI.** Tugas baru W13 yang menambah string UI mendarat sebelum W13-T7.
- **Bundel debt register ke ID:** CI-FIX = W8-T3; ENG-HYGIENE = W11-T7; IMPORT-2 = W12-T7a–e; EXPORT-2 = W12-T8; TO-TABLE-SAFE = W12-T9; PARAMS = W12-T11; HOST-HARDEN = W13-T9; SAFE-SCOPE = W13-T10; STORAGE-2 = W13-T12; GRID-UX-2 = W10-T9 dan W13-T13; CONN-2 = W13-T15, W13-T16, W13-T17.

  Bundel register utang (`target/run/debt-register.md` §8): DOC-SYNC-2 = W8-T5; VERIFY-V14 + PUSH = W8-T6; REVIEW-CLOSE = W14-T9; FLAKE-SOAK = W14-T10; REPO-HYGIENE = W14-T11; LEDGER-CLOSE = W14-T12.

| Tujuan | Item |
|---|---|
| W8-T3 (baru) | DBX-42 |
| W8-T4 (baru) | DBX-41 |
| W10-T3 (perluasan) | DBX-26, DBX-27, DBX-63 (klik gutter), PF-2, PF-12 |
| W10-T4 (perluasan) | DBX-40, DBX-66, PF-8, PF-9, PF-10 |
| W10-T8 (baru) | DBX-28, DBX-54 fase 1, PF-7 |
| W10-T9 (baru) | DBX-63 (autoscroll, shift-klik), DBX-64, DBX-65, PF-11 |
| W11-T2 (perluasan) | DBX-1 (T2b1d); DBX-20, PF-16 (T2b2) |
| W11-T3 (perluasan) | DBX-14, DBX-15, DBX-57 sisi MCP, DBX-60 (T3r); DBX-29, DBX-57 sisi app, PF-5, PF-6 (T3s) |
| W11-T4 (perluasan) | DBX-49 (refresh pohon) |
| W11-T6 (perluasan) | DBX-16, DBX-17, kait invalidasi DBX-49 (T6b) |
| W11-T7 (baru) | DBX-2, DBX-11, DBX-19, DBX-46, DBX-59 (stall, ack COMMIT), PF-17, PF-22, PF-23, B-8 |
| W11-T8 (baru) | DBX-53, DBX-54 fase 2 (engine) |
| W12-T2 (perluasan) | PF-13 (aturan), PF-14, satu langkah DBX-57 |
| W12-T3 (perluasan) | DBX-12, DBX-71 (engine) |
| W12-T4 (perluasan) | DBX-13, DBX-49 (pemicu), DBX-69, DBX-71 (label) |
| W12-T7a (baru) | DBX-7, DBX-31, DBX-32, DBX-59 (`reset_peer`), DBX-72, PF-3, PF-4 |
| W12-T7b (baru) | DBX-30, DBX-56, B-13a, B-14a |
| W12-T7c (baru) | DBX-5, DBX-39, DBX-55 |
| W12-T7d (baru, bersyarat) | DBX-8 |
| W12-T7e (baru) | DBX-6 |
| W12-T8 (baru) | DBX-9, DBX-33, DBX-34, DBX-35, PF-21; DBX-10 bila diadopsi |
| W12-T9 (baru) | PF-1 |
| W12-T10 (baru) | DBX-68 |
| W12-T11 (baru) | DBX-52, DBX-70 |
| W12-T12 (baru) | DBX-50, DBX-51 |
| W12-C (perluasan) | DBX-18 |
| W13-T2 (perluasan) | DBX-3, DBX-4 |
| W13-T4 (perluasan) | DBX-23 |
| W13-T5 (perluasan) | DBX-24 |
| W13-T7 (perluasan) | PF-27 |
| W13-T8a (perluasan) | PF-26 |
| W13-T9 (baru) | DBX-47, DBX-48, DBX-59 (probe idle) |
| W13-T10 (baru, bersyarat) | DBX-36 |
| W13-T11 (baru, bersyarat) | DBX-37 |
| W13-T12 (baru) | DBX-58 |
| W13-T13 (baru) | DBX-62, DBX-54 fase 2 (pemilih di grid) |
| W13-T14 (baru) | DBX-76, DBX-77 |
| W13-T15 (baru) | DBX-67 |
| W13-T16 (baru) | DBX-73, DBX-74 |
| W13-T17 (baru, bersyarat) | DBX-38 |
| W13-T18 (baru, bersyarat) | DBX-75 |
| W14-T1 (perluasan, §9) | DBX-44 |
| W14-T4 (perluasan, §9) | PF-25 |
| W14-T5 (perluasan, §9) | DBX-78 (bersyarat), PF-24 |
| W14-T7 (perluasan, §9) | DBX-43, DBX-61 |
| W14-T8 (baru, §9) | DBX-45 |
| Keputusan ledger (LEDGER-CLOSE), bukan tugas | DBX-8 (lalu W12-T7d), DBX-10 (lalu W12-T8), DBX-25 (bawaan: tidak diadopsi, masuk PRD §10) |
| Sudah tertutup | DBX-21, DBX-22 (`487a18b`) |
| Catatan desain tanpa tugas | PF-15 (asisten AI tidak pernah menyimpulkan izin dari teks chat), PF-18 (ProxyJump tetap ditolak), PF-19 (skema URL tetap dilewati, `prd:381`), PF-20 (buat tabel dari berkas) |
| Temuan sampingan | komentar basi `crates/qh-driver-postgres/tests/integration.rs:10` (W14-T1) dan `crates/qh-ffi/src/host/pool.rs:47` (W11-T7); doc comment `crates/qh-export/src/xls.rs:170-173` (W12-T8) dan `crates/qh-sql/src/classify.rs:784` (W12-T7b); `quotedIdent` tanpa penggandaan kutip (W12-T12); guard `app/release.sh:136-137` (W13-T14); 5,5 menjadi 6 di kolom int (W12-T7a); `with_root_certs` mysql_async vs PRD §12.1 (W13-T15) |

### Register utang (2026-10-06)

- **Sumber.** `target/run/debt-register.md` (audit opus atas `da36406`, 130 butir, usulan bundel di §8) dan definisi selesai di `target/run/ledger.md` ("DEFINITION OF DONE FOR THE WHOLE RUN"). Setiap butir yang belum punya pemilik kini punya satu tugas atau satu perluasan brief di bawah; yang fisik butuh pemilik tetap daftar tunggu: L-23, L-34 (W6-T1 6c, menunggu P-1), L-48, L-51, C-5, C-9, W8D-12, dan start dingin L-15.
- **Pemilik butir, selain bundel yang sudah bertugas sendiri.** Isi setiap perluasan ada di bagian "Perluasan register utang" pada gelombang masing-masing.

| Tujuan | Butir |
|---|---|
| W8-T5 (baru, DOC-SYNC-2) | L-50 |
| W8-T6 (baru, VERIFY-V14 + PUSH) | L-40, L-46 |
| W8-F2 (tercatat) | W8D-4, W8D-5, L-35, L-37 |
| W8-F5 (tercatat) | L-3, L-13, L-14, L-15, C-10, C-11, W8D-3, W8D-6 sampai W8D-10 |
| W9-T9 | L-39 |
| W9-C (perluasan) | C-1, C-2, C-3, C-4, L-29, TD-3 |
| W10-T2 (perluasan) | TD-1 |
| W10-T6b (perluasan) | B-1, L-18, L-12 (dialek) |
| W10-T7b (perluasan) | B-2, B-9 |
| W10-C (perluasan) | L-12 (sisa), L-22 |
| W11-T3s (perluasan) | B-6, C-6 |
| W11-T4 (perluasan) | TD-2 |
| W11-T6b (perluasan) | TD-4 |
| W11-T7 | B-8, DBX-2, DBX-11, DBX-19 (ENG-HYGIENE) |
| W12-T4 (perluasan) | W8D-1 |
| W12-T6b | C-12 |
| W12-C (perluasan) | DBX-9 (pemeriksaan), DBX-18 |
| W13-T2 (perluasan) | DBX-3 (kode), B-16, B-17, B-18 |
| W13-D (perluasan) | L-6 |
| W14-T4 (perluasan) | L-7, L-9, L-16, L-17, L-27 |
| W14-T5 (perluasan) | B-12, L-44, C-8 |
| W14-T6 (laporan) | C-13 |
| W14-T7 (perluasan) | L-45 |
| W14-T9 (baru, REVIEW-CLOSE) | L-5, L-8, L-31 |
| W14-T10 (baru, FLAKE-SOAK) | B-10, L-10, C-7 |
| W14-T11 (baru, REPO-HYGIENE) | L-47, L-49 |
| W14-T12 (baru, LEDGER-CLOSE) | B-15, L-21, TD-5, DBX-8, DBX-10, DBX-25 |

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
- Berkas: `deploy/dev/up.sh` (toxiproxy di 127.0.0.1:55435, karena 55433 terpakai di host ini dan 55434 milik `qh-pg-old.sh`; `--memory` untuk Trino), `deploy/dev/compose.yaml`, `deploy/dev/make_sql_corpus.py` (baru), `deploy/dev/make_many_tables.py` (baru, 5.000 tabel).
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
- **Dilewati di run ini (30 Sep 2026):** disk bebas host 30 GiB (< 60, §2.1). Head-to-head TablePro ditulis `tidak diukur (izin OS)`, dan `competitor_rev` tetap dicatat bila repo TablePro terbaca.
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

### W3: Safe Mode MySQL, Fase 2, dan Fase 4B-core

Urutan: W3-T0 dan W3-T1 berjalan paralel (berkas terpisah). W3-T2 langkah 1 (`qh-sql-grammar`) boleh bersamaan dengan W3-T0. Langkah 2 W3-T2 menunggu W3-T0 mendarat, dan W3-T2 menunggu W3-T1 untuk `Cargo.toml` dan `Cargo.lock` (§7).

**W3-T0. Celah Safe Mode MySQL.** Ukuran S–M. Risiko tinggi. Implementer **GP-o**, karena `scan()` menentukan apa yang dikirim ke server dan MCP bersandar penuh padanya untuk MySQL. **Sedang dikerjakan (30 Sep 2026).** Lahir dari verdict AR 4B, dan mendahului W3-T2 langkah 2.
- Cakupan: NFR-S1, NFR-S6.
- Masalah (diturunkan dari kode, belum dijalankan): `scan.rs` tidak mengenal escape backslash dan komentar `#` MySQL, `mysql_async` 0.36.2 selalu menyalakan `CLIENT_MULTI_STATEMENTS`, dan jalur teks driver MySQL tidak punya `prepare` yang menolak multi-statement seperti PostgreSQL.
  - `SELECT '\''; DELETE FROM t; -- '` dibaca `scan()` sebagai satu `SELECT` (`\` lalu `''` dianggap kutip ganda), `classify` memberi `ReadOnly`, dan server menjalankan `DELETE`.
  - Bentuk kedua: `SELECT 1 # '` + LF + `; DELETE FROM t; -- '`, yaitu komentar `#`.
  - Komentar eksekusi `/*! … */` berisi kode yang dijalankan MySQL.
  - MCP memaksa `SafeMode::ReadOnly` dan driver MySQL tidak punya sesi read-only di server, jadi ini melanggar NFR-S6.
- Arah perbaikan (diputuskan SEC): mode MySQL di pemindai (`\` meng-escape di `'…'` dan `"…"`, `#` komentar baris, isi `/*! … */` dibaca sebagai kode), dan Safe Mode di koneksi MySQL menolak bila **salah satu** pembacaan (standar atau MySQL) tidak `ReadOnly` atau jumlah statement-nya berbeda.
- Urutan kerja: tes gagal lebih dulu, yaitu kedua contoh di atas (SEC menulis atau meninjaunya) di `crates/qh-sql` dan di `crates/qh-ffi/tests/safe_mode.rs`. Baru sesudah itu perbaikannya.
- Berkas: `crates/qh-sql/src/{scan.rs,classify.rs}`, tes `crates/qh-sql/tests/`, `crates/qh-ffi/tests/safe_mode.rs`.
- Gate: SEC, DB, CR.
- Verifikasi: G-RUST, G-LIVE MySQL (`QH_TEST_MYSQL=1`, di crate yang punya tes itu).
- Commit: `fix(sql): MySQL backslash escapes and # comments can no longer hide a write from Safe Mode`.

**W3-T1. Fase 2: `EngineHost`, pool, dan TTFR.** Ukuran L. Implementer **GP-o**, karena kebenaran reset, konkurensi pool, dan tunnel bersama adalah bug yang diam.
- Cakupan: FR-PERF-03, NFR-P1 (S1, S3, S4), NFR-P8.
- Berkas:
  - `crates/qh-ffi/src/{host.rs (baru),lib.rs,commands.rs,uniffi_api.rs,tunnel.rs,local.rs,retry.rs}`, `crates/qh-ffi/tests/host.rs` (baru);
  - `crates/qh-ffi/examples/bench_ffi.rs` (pindah ke `EngineHost` di commit 1, karena `run` bebas UniFFI dihapus);
  - `crates/qh-driver/src/lib.rs` (`Session::reset`);
  - `crates/qh-driver-{postgres,mysql,trino}/src/lib.rs`, `crates/qh-driver-mysql/tests/integration.rs`;
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
- **Reconnect** (verdict AR, `fase-2-engine-host.md`): statement hanya dijalankan ulang di koneksi pengganti bila `ReadOnly`, atau bila teksnya persis `BEGIN`/`START TRANSACTION`. `Connect` tidak dianggap "belum terkirim", dan `retry::execute` tidak diberi koneksi segar untuk write. Tes T5 membuktikannya.
- Commit: sampai tiga, masing-masing dengan gate (DB dan SF ikut menjaga F dan commit 1):
  - F `fix(mysql): one session keeps one connection, so a transaction is one`. Mendarat **sebelum** commit 1, bisa diuji sendiri lewat L5 tanpa pool (`crates/qh-driver-mysql/tests/integration.rs`), dan bisa di-revert terpisah;
  - 1 `perf(engine): an engine host with a session pool that resets every run`;
  - 2 `perf(engine): capped previews keep their session, warm-up on select, one round trip fewer`.

**W3-T2. Fase 4B-core: `qh-sql-grammar`, `walk`/`lex`, dan `qh-editor`.** Ukuran L (sebelumnya M). Implementer GP-s (O-17). Risikonya ada di batas Safe Mode (`scan.rs`), C di dalam proses, dan konvergensi warna, bukan di semantik regex, dan gate tinggi menjaganya.
- Cakupan: FR-PERF-04, O-14.
- Bergantung pada W3-T0. Langkah 1 (`qh-sql-grammar`) boleh berjalan bersamaan dengan W3-T0. Langkah 2 (refaktor `scan.rs`) menunggu W3-T0 mendarat, supaya salinan beku G4 membekukan pemindai yang sudah diperbaiki dan `walk` lahir dengan mode MySQL. `Cargo.toml` dan `Cargo.lock` berurutan dengan W3-T1, tidak paralel (§7).
- Berkas (blueprint `fase-4b-editor-analysis.md` §11):
  - `Cargo.toml`, `Cargo.lock`: anggota `crates/qh-sql-grammar` dan `crates/qh-editor`; `tree-sitter = "=0.26.13"`, `tree-sitter-language = "=0.1.7"`, dan `cc = "1.4"` di `[workspace.dependencies]`; `[profile.dev.package.*]` dengan `opt-level = 2` untuk grammar dan runtime;
  - `crates/qh-sql-grammar/**` (baru): C yang di-vendor, patch scanner PR #361, `PROVENANCE.md`, dan tes kebocoran scanner;
  - `crates/qh-sql/src/scan.rs` (`walk`, `Visitor`, `EndState`; `scan()` identik), `crates/qh-sql/src/lex.rs` (baru), `crates/qh-sql/src/lib.rs`, dan tes `crates/qh-sql/tests/{scan_refactor.rs,lex.rs}` (baru);
  - `crates/qh-editor/**` (baru): `text`, `statements`, `view`, `syntax`, `classify`, `keywords`, `paint`, `folds`, `issues`, tes `tests/{golden,incremental,statements,folds}.rs`, `tests/fixtures/**`, dan `examples/editor_bench.rs`.
  - Tidak ada berkas Swift. `LexerFixtureExport.swift` dan `UnicodeTableExport.swift` tidak dibuat, karena paritas terhadap regex tidak lagi dituntut.
- Uji acak berbenih (SplitMix64) tanpa dependensi baru.
- Gate: RR, TD, CR, dan **SEC** di commit 1 (diff `scan.rs` dan seluruh `qh-sql-grammar`: C yang di-vendor, patch scanner, dan `unsafe`). **G-DENY**.
- Verifikasi: G-RUST (G1 sampai G6), G-DENY, dan G-SWIFT (tidak ada berkas Swift yang berubah, jadi hanya bukti tidak mundur). Angka bench §11 blueprint dicatat di laporan.
- Commit: dua.
  - 1 `refactor(sql): a walk engine and a code lexer over scan.rs, and a vendored tree-sitter grammar`. Berisi `qh-sql` (`walk`, `lex`, G4, G5) dan `qh-sql-grammar` (G6), dengan SEC.
  - 2 `feat(editor): a qh-editor crate with one tree-sitter tree per statement`. Berisi `qh-editor` dan sisanya.

**W3-A1.** Blueprint Fase 5. code-architect · opus, dengan spesifikasi kursor dan AX dari UX dan AX. Pemeriksa AR.

**W3-T3.** Sesi bench Fase 2 (eksklusif) dan verdict PO.

**W3-D.** ADR-0031 lengkap, addendum 0016, dan blueprint Fase 5.

**W3-C.** Pembersihan.

### W4: Fase 3, Fase 4B-integrasi, dan Fase 6-core

T1, T2, dan T3 berjalan paralel dengan berkas yang terpisah. T4 dijalankan setelah T1 dan T3.

Urutan T2: commit A → W4-T1 → W4-T2b → commit B. Tidak ada tugas lain di antara A dan B selain W4-T2b, dan W4 tidak boleh ditutup sebelum W4-T2b mendarat.

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

**W4-T2. Fase 4B-integrasi.** Ukuran M. Dua commit (D-19, blueprint `fase-4b-editor-analysis.md` §12): A menambah FFI dan tes tanpa mengubah perilaku, dan `EditorAnalysis` belum tersambung ke editor; B mengganti jalur dan menghapus regex.
- Cakupan: FR-PERF-04, FR-ED-09, NFR-P5, V-12.
- Berkas:
  - A: `crates/qh-ffi/Cargo.toml` (`qh-editor`), `crates/qh-ffi/src/editor.rs` (baru), `crates/qh-ffi/src/lib.rs` (satu baris `mod`), `app/Generated/`, `Support/EditorAnalysis.swift` (baru), tes baru `EditorAnalysisTests.swift` (G7, pada `SQLTextView` headless), dan `THIRD-PARTY-NOTICES.md` (baru: bagian runtime tree-sitter, header UTF ICU, dan grammar DerekStride, karena commit A adalah yang pertama menautkan kode itu ke app; berkas lengkap untuk semua crate dan pemasangannya ke bundel adalah kriteria rilis W14);
  - B: `Views/SQLEditor.swift`, `Views/SQLSyntax.swift`, `Support/SQLFolding.swift`, `Support/PerfSignposts.swift` (interval `apply`), `Support/BenchMode.swift` (`type-*` menunggu `apply`), tes `EditorFindAndFoldingTests.swift`, `EditorIncrementalTests.swift`, `Bench/EditorBenchTests.swift`, `EditorRotorTests.swift` (baru), `SyntaxPaletteTests.swift` (baru, G8), `__Baselines__/editor-*` (rekam ulang V-12, 16 pasang), dan `app/DESIGN.md` §"Colouring the query";
  - `Models/SQLScanner.swift` **tidak disentuh** (tidak punya bagian statement).
- Pelaksana: GP-s. Gate: SR, RR, SEC (buffer FFI, validasi hasil, semua ekspor throwing), AX (hook rotor), UX (V-12 dan perbaikan kontras Nord), AR, CR.
- Verifikasi:
  - A: G-RUST, G-DENY, G-FFI, G-SWIFT (termasuk G7), G-VIS (tidak berubah). G7 harus hijau di commit A, sebelum jalur regex dihapus;
  - B: G-RUST, G-FFI, G-SWIFT, G-VIS (hanya scene editor yang berubah, sebagai V-12);
  - `--bench type-10k` ≤ 4 ms dan `type-2m` ≤ 8 ms p99 main untuk interval `keystroke` **dan** interval `apply`. `type-coloured-195k` dan bench auto-uppercase di `type-2m` dicatat.
- Commit: dua, masing-masing dengan gate:
  - A `feat(editor): tree-sitter analysis behind the FFI, not wired to the editor yet`;
  - B `perf(editor): tree-sitter colours and folds, applied as temporary attributes, reach 2M characters`.

**W4-T2b. Satu pemecah statement untuk editor dan Run.** Ukuran S. Wajib (verdict AR `fase-4b-editor-analysis.md` §15.1): tanpa tugas ini, sesudah commit B band dan run mark memakai `scan.rs` sementara Run masih memakai pemindai Swift, sehingga `select "a;b"` tampil satu statement tetapi Run mengirim `select "a`.
- Berkas: `Models/QueryTab.swift` (hanya isi dan komentar doc `sqlStatements(in:)`, dengan signature tetap dan `sqlStatement(in:atUTF16Offset:)` tidak diubah), `app/Tests/QueryHiveTests/StatementSplitTests.swift` (baru).
- Isi: badan `sqlStatements(in:)` diganti pemecahan berbasis `scan.rs` lewat FFI commit A, `sql_statement_ranges(sql, dialect)` (D-20). Ia meneruskan dialek koneksi tab bila diketahui, selain itu `.generic`. Tes invarian `statementRanges == sqlStatements`, dan `StatementSplitTests` memuat satu kasus MySQL dari W3-T0.
- Urutan: sesudah commit A dan W4-T1, sebelum commit B W4-T2.
- Pelaksana: GP-s. Gate: SR, DB, CR.
- Verifikasi: G-SWIFT, G-VIS (tidak ada piksel berubah).
- Commit: `refactor(editor): one statement splitter for the editor and Run`.

**W4-T3. Fase 6-core: store Arrow, view, dan spill terenkripsi.** Ukuran L.
- Cakupan: FR-PERF-05, NFR-S3, O-15.
- Berkas (blueprint `fase-6-data-plane.md` §21.1):
  - `crates/qh-columnar/**` (baru): `encoding`, `builder` (`ChunkBuilder`), `read`, `tagged` (codec bertag dipindah dari `codec.rs`), tes `roundtrip.rs`;
  - `crates/qh-result-store/src/{chunk.rs,logical.rs,store.rs,view.rs,spill.rs,registry.rs,collate.rs,render.rs,lib.rs}`, `crates/qh-result-store/Cargo.toml` (`qh-columnar`, `qh-rt`, `arrow-*`, `ring`, `rayon`, `unicode-segmentation`, dan lainnya), tes `crates/qh-result-store/tests/{spill.rs,window.rs,view.rs,logical.rs}`;
  - `crates/qh-result-store/src/codec.rs` **dihapus**;
  - `crates/qh-core/src/render.rs` (`write_text` dan hex tanpa `format!` per byte; keluaran tidak berubah);
  - `crates/qh-rt/src/lib.rs` (pool rayon ber-QoS), `crates/qh-rt/Cargo.toml`;
  - `Cargo.toml`, `Cargo.lock`.
- Pelaksana: GP-s. Gate: RR, SEC (nonce, AAD, umur kunci, `0600`, sapuan), TD, SF (disk penuh, rekaman rusak), AR (verdict baru), CR.
- Verifikasi:
  - G-RUST, G-DENY, **G-GOLDEN** (keluaran `render.rs` tidak berubah);
  - tes spill: teks biasa tidak ada di berkas, kunci salah gagal, manipulasi gagal, mode berkas `0600`, sapuan yatim;
  - tes view, window, round trip, dan skema logis (`tests/logical.rs`);
  - `cargo test -p qh-result-store -- --ignored bench_*` untuk angka lokal (dicatat, bukan gate). Target lokal: jendela 128 × 30 bertipe p99 ≤ 250 µs di sisi Rust.
- **Perbaikan wajib** (R-15, verdict AR `fase-6-data-plane.md`):
  - `Unknown` bentuk 2 (`raw`) lolos round-trip (B-5): hari ini `codec.rs` men-decode-nya menjadi `text` lossy dengan `raw: None`, sehingga teks sel berubah senyap dari hex. Perbaikannya di `qh-columnar/tagged.rs`;
  - sapuan spill (direktori `0700`, tanpa mengikuti symlink) yang hari ini tidak ada;
  - penimpaan berkas spill: `create(true).truncate(true)` dengan penghitung per store membuat dua store dalam satu proses menimpa berkas yang sama, jadi berkas dibuat `create_new` (`0600`) lalu di-unlink sebelum byte pertama ditulis.
- Commit: `feat(store): Arrow chunks with a lossless Value mapping, a rayon view, and spill encrypted with a per-process key`.

**W4-T4. Fixture diferensial sort, filter, dan search.** Ukuran S.
- Berkas: `app/Tests/QueryHiveTests/SortFixtureExport.swift` (baru), `crates/qh-result-store/tests/{differential.rs,fixtures/}`.
- Korpus: ASCII, Latin beraksen, nama Indonesia, CJK, emoji, dan campuran digit. Divergensi didaftar secara eksplisit di tes (O-9).
- Pelaksana: test-engineer · sonnet. Gate: CR.
- Commit: `test(store): differential sort, filter and search fixtures exported from the Swift implementation`.

**W4-T5.** Sesi bench: sumbu 5, sort sekunder, dan verdict PO.

**W4-D.** ADR 0033 (isi di §4) dan 0037.

**W4-C.** Pembersihan.

### W5: Fase 5 dan Fase 6-engine

T1 (Swift) dan T2 (Rust) berjalan paralel di worktree. T2 memiliki `app/Generated/`, sedangkan T1 tidak menyentuhnya.

**W5-T1. Fase 5: grid `NSTableView`.** Ukuran L. Implementer **GP-o**, karena permukaan paritasnya paling luas, dan fokus SwiftUI ke AppKit, IME, serta AX semuanya bertemu di sini.
- Cakupan: FR-GRID-01, FR-GRID-07 (dasar), NFR-P4.
- Berkas:
  - baru: `Views/{ResultGridTable,GridRowView,GridHeaderView,GridAccessibility}.swift`, `Models/{ResultRows,GridMetrics}.swift`;
  - diubah: `Views/ResultGrid.swift` (body diganti), `Views/Panels.swift`, `Models/{QueryTab,ColumnFormat,WritePlan,CellSelection,UpdateStatements,CellEdits}.swift`, `Support/BenchMode.swift`;
  - tes: `ResultGridTests`, `GridColumnsTests`, `GridParityTests` baru, `ResultRowsTests`, `GridMetricsTests`, `GridAccessibilityTests` (tes pohon AX), dan `CellSelectionTests`;
  - urutan: W4-T1 → W4-T2b → 5a → probe P-2 sampai P-6 → 5b (P-2 dijalankan terhadap baseline pasca-V-1);
  - 5a juga merekam scene baseline tambahan dari renderer SwiftUI yang masih ada: `explain-*` dan satu scene grid berisi baris baru, tab, spasi awal, karakter kontrol, dan chip yang dibungkus. Disetujui pemilik sebagai scene baru saja; baseline yang ada tidak disentuh.
- Gate: SR, TD (`ResultRows`, `CellText`), AX, UX, AR, CR, PO.
- Verifikasi:
  - G-SWIFT, G-VIS (grid, tanpa rekam ulang);
  - daftar paritas `performance-plan.md` §9;
  - `--bench scroll-30x1m`, `scroll-500x10k`, `open-500x10k` ≤ 30 ms; TTFR S1 tidak mundur.
- Commit: dua, masing-masing dengan gate:
  - 5a `refactor(grid): every reader of rows goes through a ResultRows seam`;
  - 5b `perf(grid): an NSTableView that draws its cells, and the SwiftUI grid is gone`.

**W5-T2. Fase 6-engine.** Ukuran L. Implementer GP-s (O-17). Umur handle, `StoreId`, dan panic lintas FFI dijaga gate RR, SEC, TD, SF, dan AR.
- Cakupan: FR-PERF-05.
- Berkas: `crates/qh-ffi/src/{host.rs,store_api.rs (baru),commands.rs (`RESULT_SINK=store`),events.rs,uniffi_api.rs,lib.rs}`, `crates/qh-ffi/Cargo.toml`, `crates/qh-ffi/examples/bench_ffi.rs` (kasus `window`), `crates/qh-ffi/tests/store_sink.rs` (baru), `Cargo.lock` (hanya tepi `qh-ffi → qh-result-store`), `app/Generated/`, `app/Tests/QueryHiveTests/ResultHandleSmokeTests.swift` (baru).
- Gate: RR, SEC, TD, SF, AR, CR.
- Verifikasi: G-RUST, G-FFI, G-SWIFT, G-GOLDEN (CLI tidak berubah), dan `bench_ffi window`.
- Commit: `feat(engine): a store sink and ResultHandle windows over UniFFI`.

**W5-T3.** Sesi bench: sumbu 4, lalu `window` p99, dengan verdict PO. Bila p99 > 0,5 ms, kasus ini dicatat untuk W8-T2.

**W5-D.** ADR 0032.

**W5-C.** Pembersihan.

### W6: Fase 6-Swift

**W6-A1.** Blueprint Fase 6 disegarkan terhadap seam Fase 5 (code-architect · opus, pemeriksa AR).

**W6-T1. Fase 6-Swift: integrasi data plane.** Ukuran L. Implementer **sonnet** (O-20; sebelumnya GP-o). Kode lama dihapus besar-besaran dan umur handle berinteraksi dengan tab, jadi pengamannya adalah tiga commit, gate di tiap commit, dan satu putaran tinjau model terkuat. Rencana kerja presisinya ada di blueprint Fase 6 §21.4. **Tiga commit** (6a, 6b, 6c): 6a membangun hanya kolom yang tergambar dan memberi `distinctValues` kontrak sebenarnya (di atas `ArrayRows`, engine tidak disentuh); 6b memasukkan store; 6c menaikkan plafon, bersyarat P-1.
- Cakupan: FR-PERF-05, FR-GRID-03 dan FR-GRID-04 (fallback Rust, "off" lewat store dasar), NFR-P1 (S2), P2, P3, P8.
- Berkas:
  - `Models/StoreRows.swift` (baru), `Models/{QueryTab,AppModel,GridSort,GridSearch,WritePlan}.swift`;
  - `Support/RustEngine.swift`, `Support/DatabaseEngine.swift`, `Views/ResultGridTable.swift` (polling `displayLink`), `Views/ResultGrid.swift`, `Views/CellValueViewer.swift`, `Views/SettingsView.swift`;
  - `Views/GridTableView.swift`, `Views/GridRowView.swift` (rentang kolom yang tergambar dan display link), `Models/ResultRows.swift`, `Models/CellSelection.swift`;
  - `Support/Snapshot.swift`, `Support/BenchMode.swift` (skenario memakai `store_from_rows` di luar interval ukur; `tabs-100-held` dan hitungan fd);
  - `App.swift` (sapuan spill saat startup, `RLIMIT_NOFILE`);
  - tes di `app/Tests/QueryHiveTests/`: `StoreRowsTests.swift` (baru), `Bench/StoreWindowBench.swift` (baru), `TestStores.swift` (baru), `ArrayRowsReference.swift` dan `SwiftGridReference.swift` (baru, pindahan), `ResultGridTests.swift`, `GridColumnsTests.swift`, `GridColumnRangeTests.swift`, `Batch7Tests.swift`, `TabCloseTests.swift`, `MockEngine.swift`, `RowLimitSettingTests.swift`, `VisualParityTests.swift`, `ResultRowsTests.swift`, `SortFixtureExport.swift`, `GridTestSupport.swift`, `StoppedRunTests.swift`, `CellEditUndoTests.swift`, `FilterPresetTests.swift`, `PanelDefaultTests.swift`, `EngineContract.swift`.
- Dihapus: penumpukan baris, `previewPaintInterval`, `displayedCache`, dan `GridSort.order` di jalur panas.
- Clamp naik ke 5.000.000.
- Gate: SR, SF, AR, CR, PO. Tinjau: model terkuat, satu putaran (O-20), ditambah reviewer database untuk penjaga edit dan jalur `WritePlan` (D-27, blueprint Fase 6 §17.4).
- Verifikasi:
  - G-SWIFT, G-VIS (terhadap baseline Fase 5), tes diferensial W4-T4;
  - P-1 lewat tangkapan compositor harus lulus sebelum `productRowLimitCeiling` naik ke 5.000.000. Pemilik yang menjalankannya (butuh Screen Recording), atau sesi eksklusif. Bila gagal, `WindowedRows` masuk W6-T1;
  - G-LEAK;
  - G-BENCH(1 S2, 2, 3) dan window.
- Commit: tiga, yaitu 6a `refactor(grid): build only the columns on screen, and give the distinct list its real contract`, 6b `perf(app): results live in the Rust store, and the grid reads windows`, dan 6c `feat(app): raise the row limit ceiling to 5,000,000` (bersyarat P-1).

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
| W7-T1 | 7.1 builder tanpa alokasi: driver menulis array Arrow langsung lewat `qh_columnar::ChunkBuilder` | `crates/qh-driver/src/lib.rs` dan `Cargo.toml` (`Cursor::next_chunk`), `crates/qh-driver-postgres/src/{normalize.rs,lib.rs}`, `crates/qh-driver-mysql/src/lib.rs` (dan modul decode-nya), `crates/qh-driver-trino/src/{lib.rs,decode.rs}`, `crates/qh-columnar/src/builder.rs` (append yang dibutuhkan parser driver), `crates/qh-ffi/src/commands.rs` (`StoreTarget`), `crates/qh-driver-{postgres,mysql,trino}/tests/chunk_parity.rs` (baru). `crates/qh-core` (`ColumnarBuilder`) tidak dibangun. | GP-s | RR, CR, PO | G-RUST, G-GOLDEN (`type_zoo` live identik), G-LIVE, G-BENCH(2) A/B | `perf(drivers): rows go straight into the store's columns` |
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
- **W8-T3. CI-FIX (DBX-42, adopsi dbx), segera.** Main merah karena toolchain `stable` yang mengambang: rustc 1.99 men-deprecate `fetch_update` (`crates/qh-result-store/src/registry.rs:297`, `spill.rs:112`), sedangkan lokal 1.98.1; job hygiene gagal karena shellcheck (SC1010, SC2043, SC2034). Berkas: `rust-toolchain.toml` (baru, dipatok 1.98.1), `.github/workflows/verify.yml` (cargo-deny prebuilt yang dipatok, `timeout-minutes`, `paths-ignore` untuk `docs/**`; tanpa concurrency group di main, karena O-21 butuh catatan per commit), `.github/workflows/repo-hygiene.yml`, dan skrip yang ditandai shellcheck. Pindah ke `try_update` bukan bagian tugas ini; itu kenaikan toolchain yang disengaja nanti. Pelaksana GP-s. Tingkat rendah (gate plus satu reviewer sonnet). Verifikasi: G-RUST lokal tidak berubah, lalu run `verify` dan `hygiene` berikutnya di main hijau (`gh run list --branch main -L2`). bp: — (backlog DBX-42). Commit `ci: pin the toolchain, and give the jobs a prebuilt cargo-deny and a time limit`.
- **W8-T4. Debuginfo `line-tables-only` untuk dependensi di build dev (DBX-41, adopsi dbx).** Berkas: `Cargo.toml` saja (`[profile.dev.package."*"]`), setelah W11-T2b1d dan sebelum W13-T8a di rantai §7. RUSTFLAGS CI dbx tidak disalin. Pelaksana GP-s. Tingkat rendah. Verifikasi: `du -sh target/debug` dan `cargo test --workspace` dingin sebelum dan sesudah, keduanya dicatat (backlog: `target/debug` 15G dengan 10Gi bebas); G-RUST hijau. Dipertahankan hanya bila `target/debug` turun ≥ 10% tanpa build dingin yang lebih lambat. bp: — (`fase-6-data-plane.md:932` hanya mengatur helper). Commit `build: dependencies keep only line tables in dev builds`.
- **W8-F0 (O-31, selesai `7d1f5b4`).** Skenario bench sesuai rencana: enam skenario S baru, enam baris Axis baru, fixture 5M, 1M, cancel, dan scroll, stage stamp, proxy `frame_cost`, dan rebinding `AXIS_ROWS`. Berkas: `BenchMode.swift`, `PerfSignposts.swift`, `deploy/dev/bench*.py`. Ukuran M, GP-s.
- **W8-F1 (O-31, mendarat `854e3cc`).** Diagnosis TTFR S1 (`w8t2-decision.md` §3): stage stamp dibersihkan per pengulangan, 20 kali eksklusif, profil, hipotesis H1 sampai H5, lalu A/B (mount grid, `displayIfNeeded`, coalesce kolom dan progres, flush terpisah). Ukuran M, GP-s, SR. A/B aturan simpan (≥ 10%) masih terutang dan diputuskan di W8-F5 (C-11); bila meleset, commit ini di-revert.
- **W8-F2 (O-31).** Ulang A/B ingest: plafon Trino, profil baru, rebase `lane/w7-t1` (A/B MySQL, Trino, memori) di HEAD, puncak memori MySQL (staged, mimalloc). Berkas: tiga driver, `qh-driver/lib`, `qh-columnar`, `commands.rs`, `tests/chunk_parity`. Ukuran M, GP-s, RR dan DB. Perluasan register utang: W8D-4 (puncak dingin MySQL 369 MiB melawan 320), W8D-5 (Axis 2 MySQL dan Trino meleset, plafon Trino belum diukur, W7-T4 diperiksa lagi terhadap profil baru), L-35 (outlier mem-500k 280 MB pada run pertama), dan L-37 bila W7-T1 mendarat ulang (`feed_head_widths` pada `push_chunk`, `truncate` yang meninggalkan `estimated_bytes` basi, `next_chunk` MySQL, `ChunkBuilder` bertipe).
- **W8-F3 (O-31, mendarat `77d5fd0`).** Ekor ketikan editor: atribusi sub-interval dan tuas A/B L1 sampai L4 (paint sinkron, cache CGColor dan atribut, outline lepas dari ketikan, invalidasi line fragment). Lingkup: `type-10k-plan`, `type-2m` berwarna, tes. Ukuran M, GP-s, SR. A/B `type-10k` eksklusif masih terutang (C-10), diputuskan di W8-F5.
- **W8-F5 (O-31).** Bench ulang X8 (semua sumbu dan skenario baru setelah F1 sampai F3), dan penilaian ulang batch pertama COPY S2t melawan S1t. Ukuran S. Perluasan register utang, masing-masing dengan verdict tertulis: L-3 (baris berwarna menggambar ulang 4 sampai 5 ms), L-13 (jendela pertama `qh-editor` 3,9 sampai 9,2 ms melawan target 3 sampai 4 ms, kini diberi baris verdict), L-14 (S1 TTFR), L-15 (`ttfr-s3-rtt30`, `ttfr-s4-first-run`, `introspect-5000`; start dingin butuh `sudo purge`, tugas pemilik), C-10 dan C-11 (A/B F3 dan F1, revert bila aturan simpan gagal), W8D-3 (catat konfigurasi dan arsip yang ditautkan StoreWindowBench), W8D-6 (Axis 2 PG tanpa rekaman `rows-pg-table-500k`), W8D-7 (verdict S2 `ttfr-s2t-500k`), W8D-8 (`type-2m` berwarna dan `type-10k-plan`), W8D-9 (miss spill p99 ≤ 2 ms; prefetch R-3 hanya bila `scroll-30x1m-spilled` menunjukkan > 1 ms/s), dan W8D-10 (pemicu E4 `rerun-capped`).
- **W8-E2s (O-31, kontingen).** Spike CodeEditTextView bila `type-10k-plan` p99 > 4 ms atau `type-2m` berwarna p99 > 8 ms setelah F3: tiruan pola 200 karakter per ketikan di `target/run/e2-spike`, ambang biaya gambar ulang < 50% sebelum E2 diajukan ke pemilik. **W8-E4** bersyarat: hanya bila `rerun-capped` menunjukkan penalti ≥ 1 RTT (W8D-10).
- **W8-T5. DOC-SYNC-2 (register utang L-50).** Rencana ini disinkronkan: baris W8-F0 sampai F5 dan W8-E2s, seluruh adopsi dbx, register utang, §3, §6, §9, §10. Pelaksana doc-updater · sonnet (skill `writing-for-agents`). Tingkat rendah, satu reviewer sonnet. Berkas: `docs/architecture/development-plan.md` saja. Verifikasi: `grep -c "DBX-" docs/architecture/development-plan.md` tidak nol, dan setiap ID tugas di tabel ringkasan ada di bagian gelombangnya. Mendarat bersama commit rencana ini. Commit `docs(plan): fold the dbx adoption, the debt register and the W8 fix tasks into the plan`.
- **W8-T6. VERIFY-V14 + PUSH (register utang L-40, L-46).** Jalankan G-VIS dan ChromeParity di harness 2x tetap (`2864674`) untuk `c0daa99` dan `da36406`, lalu G-HEAVY, lalu dorong delapan commit (`e2af443..da36406`) ke `work/perf-parity` dan `main` secara fast-forward (O-21, O-31b). Sebelum push, baca `gh run list --branch main -L2` (DBX-43); main merah memblokir push, dan push berikutnya harus membawa W8-T3. Berkas: tidak ada (verifikasi), hasilnya masuk `target/run/ledger.md`. Pelaksana orkestrator solo, gate dijalankan test-engineer · sonnet. Tingkat rendah. Verifikasi: kedua gate hijau dan `git rev-list --left-right --count origin/main...HEAD` menunjukkan `0 0`. Hanya bila layar menentukan hasil V-14, pemilik diminta memeriksa.

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
| W9-T8 | Shell native. Implementer GP-s (O-17); struktur jendela berubah dan harness snapshot harus tetap menangkapnya, dijaga gate AR dan CR. | FR-UI-08; V-7 | `Views/RootView.swift`, `Views/Workspace.swift` (toolbar), `App.swift` (jendela, ukuran minimum), `Views/ContextCascade.swift`, `Support/Snapshot.swift` | GP-s | SR, UX, AX, AR, CR | G-SWIFT, G-VIS (V-7; scene lain tetap), semua scene `--snapshot` merender, ukuran minimum diukur dan dicatat | L | `feat(app): a native split view and toolbar around the same surfaces` |
| W9-T9 | Lantai 11 pt, serial dan terakhir | FR-UI-06; V-8 | semua view yang punya pemanggilan `.ui`/`.code` di bawah 11 | GP-s | UX, AX, SR | G-VIS (V-8), tes kontras | M | `fix(a11y): nothing a person must read is smaller than 11pt` |

- **W9-D.** Bagian shell, pintasan, dan Appearance di `app/DESIGN.md`, plus `PROGRESS.md`.
- **W9-C.** Pembersihan.
- **Perluasan register utang di W9-C** (C-1, C-2, C-3, C-4, L-29, TD-3; tingkat tetap rendah): lencana bilah status mengikuti tab aktif, bukan seleksi pohon (`Models/AppModel+Connections.swift:32-39`, `Views/RootView.swift`); scene V-3 dan scene V-4 `error-banner` masuk `VisualParityTests`, `BadgeSpec.id` menjadi enum, dan `Request.isDestructive` (D-14) dibuat; Snapshot `--grid-font` (P-7b), tabel Keyboard diturunkan dari `Models/AppMenu.swift`, ukuran font dipatok di Snapshot, dan label `HelpHint` menyebut bagiannya (`Views/SettingsView.swift`); tes untuk `SilentEngine` dan penjaga bidang rekaman di tes kontrak `EngineContract`; filter lingkup Open Quickly pindah ke `Models/AppModel+Focus.swift` dan marker `ponytail:` di `Views/OpenQuickly.swift:15` dihapus. Verifikasi: G-SWIFT, dan G-VIS bila scene baru direkam (commit sendiri).
- **Gate W9:** G-HEAVY, G-BENCHQ, dan tes lantai a11y.

### W10: desain grid dan editor

Lane:

- **Grid:** T1 → T2 → T3 → T8 → T4 → T9 (T8 dan T9 dari adopsi dbx).
- **Record:** T5, setelah T1.
- **Editor:** T6 → T7.

Maksimal 3 lane berjalan bersamaan.

**W10-A1.** Blueprint `w10-grid-editor-design.md` (code-architect · opus) dengan spesifikasi UX dan AX. Pemeriksa AR.

| ID | Isi | Cakupan | Berkas | Pelaksana | Gate | Verifikasi | Ukuran | Commit |
|---|---|---|---|---|---|---|---|---|
| W10-T1 | Kursor sel keyboard, panel peek, AX per sel, pengumuman | FR-GRID-06, 07; V-9 | `Views/ResultGridTable.swift`, `Views/GridAccessibility.swift`, `Views/GridKeyboard.swift` (baru), `Views/CellPeekPanel.swift` (baru) | GP-s | SR, AX, UX, CR | G-SWIFT (tes kunci dan format label AX), G-VIS, G-BENCHQ (scroll) | L | `feat(grid): a cell cursor for the keyboard, a Space preview, and VoiceOver that reads each cell` |
| W10-T2 | Lantai kontras, Increase Contrast, tata bahasa staged, funnel | FR-GRID-08, 09, 14; V-9 | `Views/GridRowView.swift`, `Views/GridHeaderView.swift`, token grid di `Support/Theme.swift`, `Models/CellEdits.swift` (status tampilan) | GP-s | AX, UX, SR | tes kontras, G-VIS | M | `fix(grid): row numbers, NULL and the funnel meet the contrast floor, and staged rows say what they are` |
| W10-T3 | Insert/delete baris, tombol tinjau ⌘S (responder chain, P-16), undo | FR-GRID-10, 11; V-9 | `Views/ResultGrid.swift` (footer), `Views/ResultGridTable.swift` (menu), `Models/{CellEdits,WritePlan}.swift`, `Models/AppModel+Edit.swift`, `App.swift` (Save) | GP-s | SR, DB, UX, AX | G-SWIFT (`WritePlanTests`, `CellEditUndoTests`), G-LIVE PostgreSQL untuk `apply_changes` | L | `feat(grid): add and delete rows, review them with ⌘S, and undo with ⌘Z` |
| W10-T4 | Salin sebagai INSERT, drag keluar sebagai CSV | FR-GRID-12, 15 | `Models/CellSelection.swift` (`GridClipboard`), `Models/InsertStatements.swift`, `Views/ResultGridTable.swift` (sumber drag) | GP-s | DB, SR | G-SWIFT (`InsertStatementsTests`, uji pasteboard) | M | `feat(grid): copy a selection as INSERT statements, or drag it out as a CSV file` |
| W10-T5 | Mode Record | FR-GRID-13; V-11 | `Views/RecordPanel.swift` (baru), `Views/CellValueViewer.swift` (sakelar mode) | GP-s | SR, UX, AX | G-SWIFT, scene baru | M | `feat(grid): a record view that reads a whole row as fields` |
| W10-T6 | Diagnostik editor, rotor, dan readout: masalah leksikal dari `scan.rs` dan posisi galat dari server. Garis bawah lewat atribut sementara `.underlineStyle`/`.underlineColor`. Dialek koneksi tab diteruskan ke `EditorDocument::new`. Galat sintaks dari pohon tree-sitter mati secara default (P-28). Posisi galat server hanya dikirim bila app memasang setelan (P-06). | FR-ED-06; V-10 | `crates/qh-core/src/error.rs`, pemetaan galat di tiga driver, `crates/qh-ffi/src/events.rs`, `Support/EditorAnalysis.swift`, `Views/SQLEditor.swift`, `Support/EditorDiagnostics.swift` (baru) | GP-s | RR, SR, AX, UX, CR | G-RUST, G-SWIFT, G-GOLDEN (tidak berubah) | M | `feat(editor): server errors and unclosed quotes are underlined, listed in a rotor, and read out` |
| W10-T7 | Ukuran font editor (⌘+/⌘−/⌘0), pasangan kurung (`bracket_pair` di Rust, digambar di `drawBackground`), gutter recess | FR-ED-07, 08, 10; V-10 | `crates/qh-editor/src/brackets.rs` (baru), `crates/qh-ffi/src/editor.rs`, `app/Generated/`, `Support/EditorAnalysis.swift`, `Views/SQLEditor.swift`, `Support/EditorPreferences.swift` | GP-s | RR, SR, UX, AX | G-RUST, G-FFI, G-VIS, G-BENCHQ (ketikan) | M | `feat(editor): a font size you can change, matching brackets, and a gutter that recesses on light canvases` |
| W10-T8 | NULL yang bisa di-stage, dan metadata kolom yang benar-benar dipakai (adopsi dbx). Status NULL bertipe di `CellEdits`, bukan teks sentinel `'NULL'`; edit kosong pada sel yang tadinya NULL tidak mengubah apa pun; Restore Original per sel; `.null` di-bind di `UpdateStatements`, `InsertStatements`, dan `WritePlan`, dan grid menggambar NULL yang di-stage. Fase 1 metadata kolom (`EngineEvent.swift:154-164` sudah didekode, belum dibaca): kolom generated read-only, peringatan (bukan blokir) bila Add Row membiarkan kolom NOT NULL tanpa default kosong, ekspresi default yang asli, siklus boolean NULL/true/false. Predikat MySQL tanpa kunci membandingkan teks secara biner dan JSON lewat `CAST(? AS JSON)` (PF-7). Return multi-sel yang mengisi blok tetap, karena itu desain (`CellEdits.swift:76-90`). Tingkat **Tinggi** (jalur tulis). bp: w10 §5, ditambah aturan NULL dan metadata di commit tugas. | DBX-28, DBX-54 (fase 1), PF-7; FR-GRID-10, 11 | `Models/{CellEdits,QueryTab,UpdateStatements,InsertStatements,BoundSQL,MatchPolicy,WritePlan}.swift`, `Views/{ResultGridTable,GridRowView}.swift`, tes `CellEditsNullTests.swift` (baru), `UpdateStatementsTests`, `InsertStatementsTests`, `WritePlanTests` | GP-s | DB, SR | G-SWIFT; G-LIVE PostgreSQL dan MySQL untuk `apply_changes` (NULL ke kolom angka dan tanggal, NULL eksplisit di baris baru, baris 'Jose', 'JOSE', dan 'Jose ' masing-masing bisa diedit); G-VIS hanya scene `grid-edits` V-9, direkam ulang di commit sendiri | M | `fix(grid): stage a real NULL, respect generated and NOT NULL columns, and match MySQL rows exactly` |
| W10-T9 | Tata letak dan pointer grid (GRID-UX-2 bagian 1, adopsi dbx). Ubah lebar kolom dengan tangan, klik ganda untuk auto-fit, dan Fit All; lebar hanya untuk sesi (menyimpannya antar-launch bersyarat keputusan). Autoscroll di tepi saat drag dan shift-klik untuk memperluas seleksi; ⌘-toggle tidak diadopsi karena model seleksi satu persegi panjang, dan ambang drag memakai 4 pt D-10 sebagai perubahan perilaku yang dinyatakan. Posisi gulir bertahan saat panel berganti, dengan kunci id tab dan `gridRevision`, disimpan di model baru supaya `Workspace.swift` tidak disentuh (BottomPanel di `Workspace.swift:27-43` tidak diberi `.id(tab.id)`). Tata letak kolom di-reset atau dipetakan ulang bila nama atau tipe kolom berubah, bukan hanya jumlahnya (PF-11, `QueryTab.swift:435-448`). Zona klik funnel FR-GRID-14 tetap. Tingkat Sedang. bp: fase-5-grid §8, w10 §3; addendum di commit. | DBX-63 (autoscroll, shift-klik), DBX-64, DBX-65, PF-11 | `Views/{GridTableView,ResultGridTable,GridHeaderView}.swift`, `Models/{GridMetrics,CellSelection,QueryTab}.swift`, `Models/GridScrollMemory.swift` (baru), `AppMenu.swift` (R-ADD, Fit All), tes `GridColumnResizeTests.swift` dan `GridScrollMemoryTests.swift` (baru), `VisualParityTests.swift` | GP-s | SR, UX, AX | G-SWIFT (dua hasil 4 kolom yang berbeda tidak berbagi tata letak; gulir kembali ke posisi semula setelah pindah panel dan pindah tab), G-VIS (V-n baru dinyatakan di PRD §6.5 sebelum rekam ulang, commit sendiri), G-BENCHQ (scroll) | M | `feat(grid): resize and fit columns, keep the scroll position, and extend a selection with the pointer` |

**W10-D** dan **W10-C**, lalu gate W10: G-HEAVY dan G-BENCHQ.

**Perluasan adopsi dbx di W10** (berkas tambahan masuk rantai §7):

- **W10-T3** (+ DBX-26, DBX-27, DBX-63 sebagian, PF-2, PF-12; ukuran naik ke L):
  - DBX-26: satu konfirmasi di `applicationShouldTerminate` dan di `closeTab` bila ada edit yang di-stage, atau impor, ekspor, atau apply yang sedang berjalan; relaunch Sparkle lewat jalur yang sama. Berkas tambahan: `Models/AppModel+Focus.swift` (setelah W10-T7b) dan `Support/Updater.swift`. Penyimpanan sesi sudah sinkron (`DatabaseEngine.swift:41-47`), jadi tanpa ack barrier.
  - DBX-27: setelah Apply berhasil grid tidak lagi menampilkan nilai sebelum edit. Base query dijalankan ulang lewat `rerunBaseSQL` (D-27), atau nilai yang di-commit ditimpakan, dan log menyebut yang mana. Edit kedua pada baris yang sama tidak boleh rollback karena predikat seluruh baris (`WritePlan.swift:29-37`). Aturan ini ditulis di bp w10 §5. Refresh lewat kunci primer menyusul setelah W11-T8.
  - DBX-63: klik gutter memilih satu baris (dibutuhkan Delete Row).
  - PF-2: satu apply per tab. Flag di tab memblokir buka ulang, ⌘S, dan edit; saat berhasil, hanya sel yang masih sama dengan snapshot yang dibersihkan. Tes: Apply kedua ditolak, dan edit yang diketik selama apply tetap ada.
  - PF-12: `levelsOfUndo` sekitar 100, dan plafon sel untuk fill, paste, dan delete.
  - Verifikasi tambahan: Quit dan tutup tab dengan edit yang di-stage memunculkan konfirmasi, dan Batal menyimpan edit; G-LIVE PostgreSQL untuk nilai sesudah Apply.
- **W10-T4** (+ DBX-40, DBX-66, PF-8, PF-9, PF-10):
  - DBX-40: satu anggaran salin, dipakai bersama Copy as INSERT. Di atas 10k sel teks dibangun di luar main thread, dan di atas anggaran ditolak dengan "Export the selection instead". Cabang off-main yang mati di `ResultGridTable.swift:780-788` dibuang, dan penyimpangan dari fase-5-grid §8.7 dicatat. Hang main thread pada 200k×50 diukur dengan signpost (`Support/PerfSignposts.swift`).
  - DBX-66: format salin daftar IN, klausa WHERE, dan nama kolom. Daftar IN dengan NULL menjadi `(col IS NULL OR col IN (…))`. JSON dan Markdown ke backlog.
  - PF-8 (dipindah dari W10-C supaya W10-C tetap tingkat rendah): sort dan search server menolak hasil dengan nama kolom keluaran ganda, dengan pesan bernama, sebelum membungkusnya (`Models/ServerSort.swift`, `Models/GridSearch.swift`). Tes `SELECT *` atas JOIN. Mendarat sebelum W13-T1 menambah Show SQL.
  - PF-9: setiap literal yang dibangun Swift sadar dialek: jarum search, teks tinjau, dan Copy SQL. Tes `C:\temp` dan `\` di akhir pada MySQL.
  - PF-10: Copy as INSERT, Duplicate Row, dan paste ke baris baru membuang kolom identity, generated, dan `nextval` (metadata dari W10-T8).
  - Berkas tambahan: `Models/{ServerSort,GridSearch,BoundSQL}.swift`, `Support/PerfSignposts.swift`.

**Perluasan register utang di W10:**

- **W10-T2** (+ TD-1): token `Tone.focusRing` dibuat di `Support/Theme.swift`, dan `Views/GridRowView.swift:30` memakainya, tanpa salinan aturan dan tanpa marker `ponytail:`.
- **W10-T6b** (+ B-1, L-18, bagian dialek L-12): tombol clear `EditorPane` tidak menimpa tombol tutup find-bar dan peringatan constraint find-bar hilang (B-1); `sqlStatements` memakai dialek per tab, bukan `.generic` (L-18); dialek `qh-editor` tidak lagi tetap sejak konstruksi (L-12).
- **W10-T7b** (+ B-2, B-9): tanda run gutter untuk statement kedua saat wrap menyala, dengan tes (B-2); baseline wrap mati tidak menyandikan teks di bawah gutter (B-9).
- **W10-C** (+ sisa L-12, L-22; tingkat tetap rendah): redaksi doc `CEILING_UTF16`, `saturating_add` di `paint.rs`, kursor per statement; celah tes W5-T1 §13 (seam mouseDown header tanpa NSEvent, build menu, signpost, responder chain).

### W11: metadata, koneksi, dan MCP

**Urutan batch:**
1. T1 sendirian (lane FFI).
2. T2 (Rust) dan T4 (Swift).
3. T3 (Swift dan `mcp.rs`) dan T6 (lane FFI).
4. T5.

Tugas adopsi dbx: T7 setelah W13-T2 (rantai driver dan `commands.rs`) dan W11-T2b2 (`host/pool.rs`), sebelum W12-T3. T8 setelah T7, W10-T8, dan W12-T3 (golden), sebelum W13-T4; bila belum mendarat saat gate W11, T8 diverifikasi di gate W12.

**W11-A1.** Blueprint `w11-metadata-and-connections.md` (code-architect · opus). Protokol TOFU di dalamnya ditinjau SEC sebelum W11-T2 dimulai.

| ID | Isi | Cakupan | Berkas | Pelaksana | Gate | Verifikasi | Ukuran | Commit |
|---|---|---|---|---|---|---|---|---|
| W11-T1 | Perintah `columns`, `ddl`, `execution_log`, dan jenis objek di `tables` (lewat setelan) | FR-TREE-01…03, FR-SAFE-02 (engine) | `crates/qh-ffi/src/metadata.rs` (baru), empat daftar invariant #11 (`lib.rs`, `uniffi_api.rs`, `Support/RustEngine.swift`), `commands.rs`, `crates/qh-driver/src/lib.rs`, tiga driver, `app/Generated/`, `tests/golden.rs`, `tests/safe_mode.rs` | GP-s | RR, DB, SF, CR | G-RUST, G-FFI, G-SWIFT, G-GOLDEN (kasus lama tidak berubah; kasus `_live` baru direkam dengan `--record`) | L | `feat(engine): read-only columns, DDL and object kinds, plus a reader for the execution log` |
| W11-T2 | SSH (alias `~/.ssh/config`, TOFU dengan fingerprint dipatok, known_hosts app), JWT Trino, CA untuk PostgreSQL dan Trino. Implementer GP-s (O-17); ini keputusan kepercayaan, dijaga gate SEC, RR, SF, AR, dan CR. | FR-CON-01…03, 07, 08 (engine) | `crates/qh-tunnel/src/{ssh_config.rs (baru),known_hosts.rs,lib.rs}`, `crates/qh-ffi/src/{tunnel.rs,config.rs,events.rs}`, `crates/qh-driver/src/lib.rs` (`ConnectionConfig`, `Debug` tersensor), `crates/qh-driver-trino/src/lib.rs`, `crates/qh-driver-postgres/src/{tls.rs,lib.rs}` | GP-s | SEC, RR, SF, AR, CR | G-RUST, G-LIVE (SSH terhadap `qh-sshd-dev`: host asing, terima dengan fingerprint benar dan salah, kunci berubah), tes TLS dengan CA uji | L | `feat(engine): SSH trust on first use pinned to a fingerprint, ~/.ssh/config aliases, Trino JWT and a per-connection CA` |
| W11-T3 | Form koneksi di app, item Keychain, prompt host key, validasi bernama, ⌘↩, Navicat SSH, pemetaan MCP | FR-CON-01…05, 07…09; V-11 | `Views/ConnectionsViews.swift`, `Views/HostKeySheet.swift` (baru), `Models/Connections.swift`, `Support/NavicatImport.swift`, `Models/AppModel+Connections.swift`, `crates/qh-ffi/src/mcp.rs` (pemetaan), `crates/qh-ffi/tests/mcp.rs` | GP-s | SEC, SR, RR, UX, AX | G-SWIFT, G-RUST, G-LIVE (Keychain), CLI end-to-end lewat tunnel: `SSH_HOST=127.0.0.1 SSH_PORT=52222 … queryhive-engine test` | L | `feat(connections): an SSH section with Keychain secrets and a host-key prompt, JWT and CA fields` |
| W11-T4 | Label jenis objek, anak kolom, dan tab DDL read-only | FR-TREE-01…03; V-11 | `Views/SchemaOutline.swift`, `Models/SchemaTree.swift`, `Models/AppModel+Tree.swift`, `Models/QueryTab.swift` (jenis tab DDL) | GP-s | SR, UX, AX | G-SWIFT, scene baru | M | `feat(tree): views are labelled, tables show their columns, and DDL opens read-only` |
| W11-T5 | Tool MCP `describe_table` dan `table_ddl` | FR-MCP-01 | `crates/qh-ffi/src/mcp.rs`, `docs/mcp-stability.md`, `crates/qh-ffi/tests/{mcp.rs,mcp_stdio.rs}` | GP-s | SEC, RR | G-RUST (allowlist kosong menolak; `read_only` tetap) | S | `feat(mcp): describe_table and table_ddl, inside the same scope and allowlist` |
| W11-T6 | Autocomplete dari katalog dengan resolusi alias (tanpa cache kedua) | FR-ED-05 | `crates/qh-editor/src/refs.rs` (API referensi tabel dan alias; menggantikan `crates/qh-sql/src/editor/`), `crates/qh-ffi/src/editor.rs`, `app/Generated/`, `Models/SQLSuggestions.swift`, `Models/AppModel+Completion.swift` | GP-s | RR, SR, CR | G-RUST, G-FFI, G-SWIFT (`SuggestionScopeTests`), G-BENCHQ (ketikan) | M | `feat(editor): column suggestions from the catalog, with aliases resolved` |
| W11-T7 | Higiene engine (ENG-HYGIENE, adopsi dbx). Cancel dibatasi waktu di `commands.rs:1160,1338` dan `host/lease.rs:515` (separuh kedua S-1, bersama DBX-1). Seluruh connect driver PostgreSQL dan MySQL, termasuk TLS dan auth, dibungkus `tokio::time::timeout` dengan satu konstanta bernama yang lepas dari `STATEMENT_TIMEOUT_MS`, dan mengembalikan `EngineError::Connect` Transient yang menyebut tahapnya; jalur connect tanpa pool (`crates/qh-ffi/src/lib.rs:421-438`, dipakai CLI, MCP, dan ekspor) ikut. Sekarang peer yang diam atau TLS yang macet menggantung selamanya. Keepalive PostgreSQL dengan interval dan retries. Statement infra (`reset`, `SET statement_timeout`) tidak membawa posisi galat. Kegagalan stop di jalur capped masuk execution log, bukan `eprintln!` (B-8, `commands.rs:1494,1519`). Galat bernama untuk "no common algorithm" SSH (tanpa opt-in algoritma lama) dan "packets out of sync" MySQL di belakang proxy; `prefer_socket(false)` tetap. PF-23 (`statement_timeout` per perintah yang memotong hasil streaming ke konsumen lambat) diverifikasi dulu dengan tes live; bila terbukti, perbaikannya menjadi tugas lanjutan. Memegang `crates/qh-ffi/src/lib.rs` di antara W11-T6b dan W12-T3 tanpa meregenerasi `app/Generated/`. Tingkat **Tinggi** (engine host, tiga driver). bp: `fase-2-engine-host.md`, w11 §6.2, ADR-0016, ADR-0031. | DBX-2, DBX-11, DBX-19, DBX-46, DBX-59 (sebagian), B-8, PF-17, PF-22, PF-23 | `crates/qh-ffi/src/{commands.rs,lib.rs,retry.rs}`, `crates/qh-ffi/src/host/{pool.rs,lease.rs}`, `crates/qh-driver-postgres/src/lib.rs`, `crates/qh-driver-mysql/src/lib.rs`, `crates/qh-core/src/error.rs`, `crates/qh-tunnel/src/tunnel.rs`, `crates/qh-ffi/tests/fault_injection.rs` (baru: proxy toxiproxy milik tes lewat `:8474`, karena proxy PostgreSQL bersama di 55435 membawa toxic permanen; proxy MySQL di dalam tes yang membuang ack COMMIT) | GP-s | SEC, RR, SF | G-RUST, G-LIVE (PostgreSQL, MySQL, SSH): peer diam dan TLS yang macet berhenti di batas waktu dengan galat yang menyebut tahap; cancel ke server yang tidak menjawab kembali dalam batas; perilaku `apply.rs:245-260` saat ack COMMIT hilang dikunci tes | M | `fix(engine): connect, TLS and cancel have a time limit, and infra statements never point at the user's SQL` |
| W11-T8 | Kunci primer untuk predikat tulis (adopsi dbx). Metadata kolom membawa kunci primer; `apply_changes` memakai PK ditambah nilai asli setiap kolom yang diedit, dengan aturan dua arah, karena `keyed:true` saja membuat perubahan 0 baris yang bersamaan terbaca sukses. MySQL menyalakan `CLIENT_FOUND_ROWS` (sekarang tidak disetel). Sisi engine fase 2 DBX-54: label enum PostgreSQL dari `pg_enum` di metadata kolom (pemilihnya di grid ada di W13-T13). Refresh baris lewat PK untuk DBX-27 dan mode Record memakai data ini. Ini perluasan output perintah `columns`, bukan perintah baru: daftar invariant #11 dan `app/Generated/` tidak berubah; bila ternyata berubah, tugas ini antre di lane FFI dan penyimpangannya dicatat. Amandemen ADR-0020 (`:49-51`). Tingkat **Tinggi** (jalur tulis). bp: w11 §3, ADR-0020. | DBX-53, DBX-54 (fase 2, engine); FR-GRID-10, 11 | `crates/qh-driver/src/metadata.rs`, `crates/qh-driver-{postgres,mysql,trino}/src/{metadata.rs,lib.rs}`, `crates/qh-ffi/src/{metadata.rs,apply.rs}`, `Support/EngineEvent.swift` (R-ADD), `Models/{WritePlan,MatchPolicy}.swift`, `docs/decisions/0020-apply-changes-in-engine.md`, kasus golden `_live` metadata bila berubah, tes `crates/qh-ffi/tests/apply_keyed.rs` (baru) | GP-s | DB, RR, SF, AR | G-RUST, G-SWIFT, G-GOLDEN (kasus lama tidak berubah), G-LIVE PostgreSQL dan MySQL: perubahan bersamaan yang membuat UPDATE ber-PK mengenai 0 baris ditolak, bukan sukses | L | `feat(engine): write predicates use the primary key and the edited columns' original values` |

**W11-D** (ADR 0038, 0039, 0040) dan **W11-C**, lalu gate W11.

**Perluasan adopsi dbx di W11:**

- **W11-T2, bagian W11-T2b1d** (+ DBX-1): cancel PostgreSQL pada sesi TLS memakai TLS, bukan `NoTls` (`crates/qh-driver-postgres/src/lib.rs:682`; perbaikan `CancelTls` sudah ada di worktree `lane/w11-t2b1d` tetapi belum di-commit). Gate SEC wajib; tes live cancel lewat TLS.
- **W11-T2, bagian W11-T2b2** (+ DBX-20, PF-16): CLI dan MCP menolak host key yang belum dikenal dan menunjuk ke app (periksa dulu perilaku sekarang). Menunggu SSH agent atau Touch ID tidak dihitung ke anggaran jaringan (`qh-tunnel/src/tunnel.rs:31-35` vs `host/pool.rs:49,622`), dan galatnya menyebut tahap. Bila DBX-29 butuh perubahan di `crates/qh-ffi/src/config.rs` (kata `require` di `:290-300`), perubahan itu ikut T2b2 karena T2b2 memegang `config.rs`.
- **W11-T3, bagian W11-T3r** (+ DBX-14, DBX-15, DBX-57 sisi MCP, DBX-60): batas default 1000 di `mcp.rs:405` diberi clamp maksimum dan cap karakter per sel; scope MCP diperiksa pada argumen schema dan database `describe_table` dan `table_ddl` (diperiksa ulang setelah `4d575a7`); MCP mendapat statement timeout untuk pertama kali (`crates/qh-storage/src/import.rs:421-446`); permukaan tool MCP dibekukan sebagai baseline JSON yang hanya boleh bertambah (nama, required, properties, prompts, resources; tanpa insta) di `crates/qh-ffi/tests/mcp_surface.json` (baru), dibaca `crates/qh-ffi/tests/mcp.rs`.
- **W11-T3, bagian W11-T3s** (+ DBX-29, DBX-57 sisi app, PF-5, PF-6):
  - DBX-29: impor URL menyimpan kata libpq PostgreSQL apa adanya dan tidak pernah memetakan `verify-full` ke `require`; `ssl-mode` dan `useSSL` MySQL dipetakan ke disable/prefer/require, VERIFY_* ditolak dengan namanya; nilai yang tidak dikenal memberi galat bernama; parameter `host`, `hostaddr`, dan `port` diabaikan. Korpus tes murni `ConnectionURLTests.swift` (baru).
  - DBX-57: statement timeout per koneksi (nil berarti mewarisi) di `Models/Connections.swift`, dan nilai efektifnya di `Models/AppModel+Connections.swift`. Pemanggilnya di `AppModel+Focus.swift:110,121` diganti oleh W12-T2, supaya T3s tidak menunggu W10-T3.
  - PF-5: sebelum T3s mulai, bp w11 §8 butir 1 ("field kosong menghapus item", baris 676) diamandemen: saat simpan, field kosong berarti tetap; penghapusan hanya lewat "Remove saved…" yang eksplisit atau pergantian metode auth. Tes: Edit lalu Save dengan field kosong membiarkan keempat slot Keychain utuh.
  - PF-6: impor Navicat tidak pernah menurunkan TLS dan tidak menimpa kredensial yang ada; untuk koneksi yang sudah ada ia melewati atau bertanya. Tes: `.ncx` dengan SSL menyala memberi PostgreSQL minimal `require`, dan impor ulang berkas lama membiarkan password tidak berubah. Atribut SSL `.ncx` diverifikasi dulu.
- **W11-T4** (+ DBX-49 sisi pohon): refresh yang menjaga anak lama sampai balasan tiba (`AppModel+Tree.swift:378-383` sekarang mengosongkannya), dipanggil W12-T4 setelah DDL.
- **W11-T6, bagian W11-T6b** (+ DBX-16, DBX-17, kait DBX-49): saran join dari foreign key; tanpa alias otomatis pada target UPDATE/DELETE, kata reserved tidak pernah jadi alias, alias HAVING per dialek, ekspansi select-star; fungsi invalidasi katalog per objek di `AppModel+Completion.swift` yang dipanggil W12-T4 setelah DDL.

**Perluasan register utang di W11:**

- **W11-T3, bagian W11-T3s** (+ B-6, C-6): duplikasi koneksi tidak menulis Keychain asli di bawah bench (`Models/AppModel+Connections.swift:209-226`); item Keychain berACL lama yang masih memunculkan prompt ikut dimigrasi bersama item lain.
- **W11-T4** (+ TD-2): outline `Views/SchemaOutline.swift:559` memakai `Tone.focusRing` (dari W10-T2), marker `ponytail:` dihapus.
- **W11-T6, bagian W11-T6b** (+ TD-4): `crates/qh-editor/src/refs.rs:350` membaca subquery dan CTE (`extract(year from d)` bukan tabel), atau plafon itu diterima tertulis di review.
- **W11-T7** (ENG-HYGIENE) sudah memegang B-8, DBX-2, DBX-11, dan DBX-19; tidak ada tugas ENG-HYGIENE terpisah.

### W12: editor dan eksekusi

**Urutan batch:**
1. T1 (lane FFI) dan T5.
2. T3 (lane FFI, setelah T1 mendarat) dan T2 (setelah T1).
3. T4 (setelah T2 dan T3).
4. T6.

Tugas adopsi dbx (urutan berkasnya di §7):
- T7a bisa mulai sekarang (`import.rs`, `qh-import`, sheet impor; `ImportSheet.swift` setelah W9-T9).
- T9 setelah W11-T7 dan sebelum T3 di rantai `commands.rs`.
- T7b setelah T3, W13-T2 (`classify.rs`), dan T7a; lalu T7c (juga setelah T8 dan T10), T7e (juga setelah T8), dan terakhir T7d yang bersyarat.
- T8 setelah T3 (`commands.rs`) dan T4 (`Workspace.swift`).
- T10 setelah W13-T4 (rantai driver) dan T4 (`Panels.swift`).
- T11 setelah W11-T6b. T12 setelah T2, W11-T4, dan W11-T6b.

**W12-A1.** Blueprint `w12-editor-and-run.md` (code-architect · opus).

| ID | Isi | Cakupan | Berkas | Pelaksana | Gate | Verifikasi | Ukuran | Commit |
|---|---|---|---|---|---|---|---|---|
| W12-T1 | Formatter. Token dari `walk` + `qh_sql::lex` dengan daftar kata klausa sendiri, tanpa pohon tree-sitter (P-10, D-21). | FR-ED-01 | `crates/qh-sql/src/format.rs` (baru), `crates/qh-sql/src/lib.rs`, ekspor fungsi di `crates/qh-ffi/src/editor.rs`, `app/Generated/` | GP-s | RR, DB, CR | G-RUST (deretan token non-spasi identik, idempoten, dollar-quote, `:name`, per dialek), G-FFI | M | `feat(sql): a formatter that moves whitespace and nothing else` |
| W12-T2 | Format, toggle comment, Save dan Save As, tab terikat berkas, deteksi perubahan dari luar, drop `.sql`, restore | FR-ED-01…04, PR-16 | `Models/QueryTab.swift`, `Models/Session.swift`, `Models/AppModel+Files.swift` (baru), `App.swift`, `Views/SQLEditor.swift`, `Views/Workspace.swift` | GP-s | SR, UX, AX | G-SWIFT (`SessionTests`: restore tanpa menjalankan) | M | `feat(editor): format, toggle comment, and .sql tabs that save back to their file` |
| W12-T3 | Perintah engine `script` | FR-RUN-01 | `crates/qh-ffi/src/script.rs` (baru), empat daftar invariant #11, `app/Generated/`, `Support/EngineWire.swift`, `tests/golden.rs` (cursor palsu), `tests/safe_mode.rs` | GP-s | RR, DB, SF, SEC, CR | G-RUST, G-FFI, G-SWIFT, G-LIVE PostgreSQL (stop, continue, cancel di dalam statement) | L | `feat(engine): a script command that runs statements one by one with a stop or continue policy` |
| W12-T4 | UI Run Script, result set per statement, pin, satu entri history | FR-RUN-01…03; V-11 | `Models/QueryTab.swift` (result set berisi handle store), `Views/ResultGrid.swift` (strip tab hasil), `Models/AppModel+Run.swift`, `Views/Panels.swift` | GP-s | SR, UX, AX, SF, AR | G-SWIFT, scene baru, memori tetap di dalam anggaran global | L | `feat(app): Run Script shows each statement's outcome and keeps a result per statement` |
| W12-T5 | Impor `.sql` dan JSON/JSONL | FR-IMP-01, 02 | `crates/qh-import/src/json_source.rs` (baru), `crates/qh-import/src/lib.rs`, `crates/qh-ffi/src/import.rs`, `Views/ImportSheet.swift`, `Models/ImportMapping.swift`, `crates/qh-ffi/tests/import_json.rs` (baru) | GP-s | RR, DB, SR, SF | G-RUST, G-SWIFT (`ImportMappingTests`), G-LIVE PostgreSQL (ketiga kebijakan) | M | `feat(import): .sql and JSON files from the app, under the same transaction policy` |
| W12-T6 | Notifikasi, pengumuman VoiceOver, progres Finder dan Dock | FR-RUN-04, 05 | `Support/LongRunNotifier.swift` (baru, di belakang protokol), `Support/FileProgress.swift` (baru), `Models/AppModel+Export.swift`, hook di `Models/AppModel+Run.swift` | GP-s | SR, AX, UX | G-SWIFT (notifier palsu). Pengiriman nyata diperiksa pemilik. | M | `feat(app): long exports report progress in Finder and the Dock, and say when they finish` |
| W12-T7a | Impor yang tidak menulis nilai salah diam-diam (IMPORT-2 bagian 1, adopsi dbx). Angka non-finite dikutip; `Z` pada MySQL menjadi `+00:00`. `DATE_FORMAT` opsional per impor, di-parse di engine dan dikirim sebagai ISO (PostgreSQL `ISO, MDY` membaca '03/04/2024' sebagai 4 Maret). Pengelompokan angka hanya opt-in, berpasangan dengan pemisah desimal (`1.500,00`). '5.0'→'5' dbx tidak diadopsi; 5,5 ke kolom int yang menjadi 6 di PostgreSQL ditolak atau diperingatkan. XLSX: batas ukuran berkas dan baris×kolom dengan galat pemakaian (langkah pertama, boleh mendarat lebih dulu sebagai commit sendiri), lalu pembaca sel streaming `worksheet_cells_reader` calamine yang sudah dipatok; angka yang mendarat di kolom teks diformat atau ditolak, tanpa crate baru. CSV: encoding dipindai sebelum tulis pertama (hanya penting tanpa transaksi: mode skip, Trino), cp1252 diterima, pemeriksaan prefiks di sheet diperbaiki (tabel `crates/qh-export/src/encoding.rs` dipakai ulang). Progres impor menurut byte, dan event progres yang sekarang dibuang app (`AppModel+Export.swift:142-150`) disambungkan. PF-3: kegagalan Transient atau `EngineError::Connect` di mode skip menghentikan impor dan melaporkan "rows after line N were not attempted"; INSERT tidak pernah diulang otomatis. PF-4: jumlah field per baris sama dengan lebar header, hanya ekstra kosong di akhir yang boleh; selisih menjadi baris ditolak di bawah ON_ERROR ("row has N fields, header has M"); baris pendek hanya lewat opt-in eksplisit (bersyarat keputusan). Tingkat **Tinggi** (kehilangan data). bp: w12 §7, ADR-0019, ADR-0022. | DBX-7, DBX-31, DBX-32, DBX-59 (sebagian), DBX-72, PF-3, PF-4; FR-IMP-01, 02 | `crates/qh-ffi/src/{import.rs,progress.rs}`, `crates/qh-import/src/{csv_source.rs,xlsx_source.rs,json_source.rs,lib.rs}`, `Models/{ImportMapping,AppModel+Export}.swift`, `Views/ImportSheet.swift`, tes `crates/qh-ffi/tests/import_values.rs` (baru), tes CSV ragged dan XLSX besar di `crates/qh-import/tests/`, `ImportMappingTests` | GP-s | DB, SF, RR, SR | G-RUST, G-SWIFT, G-LIVE PostgreSQL dan MySQL dengan ketiga kebijakan (tanggal dd/mm, NaN, `Z`, koma liar di baris 3 di bawah stop dan skip); toxiproxy `reset_peer` di mode skip memberi event galat, bukan done | L | `fix(import): dates, NaN, ragged rows and dead connections no longer write wrong data quietly` |
| W12-T7b | Satu splitter untuk `.sql` (IMPORT-2 bagian 2, adopsi dbx). Langkah 1, di dalam ADR-0022: badan `BEGIN … END` MySQL dikenali sebagai satu statement menurut tata bahasa server, sehingga `DELETE FROM b` di dalam prosedur tidak pernah berjalan sendiri di bawah ON_ERROR=skip atau SCRIPT_POLICY=continue; baris `DELIMITER`, baris backslash psql, dan `COPY … FROM stdin` ditolak dengan namanya sebelum connect; baris `\restrict` pg_dump tidak menempel ke statement pertama. Langkah 2, **bersyarat** keputusan dan addendum ADR-0022: `DELIMITER` dihormati dan baris kontrol psql dikosongkan di tempat supaya nomor baris tetap benar. Dekode byte: BOM UTF-8 dan UTF-16 (pola `json_source.rs:287-292`; fallback GBK dbx tidak diadopsi), lalu literal `_binary` mysqldump. B-13a dan B-14a (`/*!40101 SET … */`) ditutup di sini, diperbaiki atau ditolak dengan alasan tertulis. Semua di `qh-sql`, supaya guard dan pengiriman melihat statement yang sama; tanpa tokenisasi ulang O(N²). Tanda tangan API yang dipakai `crates/qh-editor/src/lib.rs:6` tetap, jadi `qh-editor` tidak disentuh (tesnya tetap dijalankan). Doc comment `classify.rs:784` disesuaikan dengan kode. Tingkat **Tinggi** (Safe Mode di `qh-sql`). bp: w12 §4 (API scanner) dan §7, ADR-0022. | DBX-30, DBX-56, B-13a, B-14a; FR-IMP-01 | `crates/qh-sql/src/{scan.rs,classify.rs,lib.rs}`, `crates/qh-ffi/src/import.rs` (pemanggil, `:715-767`), `docs/decisions/0022-import-statement-family.md` (addendum), tes `crates/qh-sql/tests/split_routines.rs` dan `crates/qh-ffi/tests/import_sql_split.rs` (baru) | GP-s | SEC, DB, RR, SF | G-RUST (termasuk `-p qh-editor`), G-GOLDEN (tidak berubah), G-LIVE MySQL (dump mysqldump dengan rutin dipulihkan lewat sheet impor; `CREATE PROCEDURE … BEGIN DELETE FROM a; DELETE FROM b; END` menjadi satu statement) dan PostgreSQL (dump pg_dump dengan `\restrict`) | L | `fix(sql): routine bodies stay one statement, and dump directives are refused by name before connecting` |
| W12-T7c | Throughput impor (IMPORT-2 bagian 3, adopsi dbx, A/B). COPY untuk PostgreSQL dan batch INSERT yang dibatasi byte dan batas server (`max_allowed_packet` MySQL, panjang query Trino). Mode skip mengirim batch dan baru mengisolasi baris per baris bila server menolak; tidak pernah mengulang saat jumlah baris tidak cocok (itu tulis sebagian), dan tidak di engine non-atomik (MyISAM, sebagian konektor Trino). Replay SAVEPOINT untuk stop/commit **bersyarat** amandemen ADR-0019 dan tidak termasuk di sini. ROLLBACK pada sesi yang mati tidak boleh menutupi "stopped at line N" (`import.rs:640-643`). Angka dbx tidak dipakai sebagai bukti. Tingkat Sedang (DB). bp: w12 §7, ADR-0019. | DBX-5, DBX-39, DBX-55; FR-IMP-01, 02 | `crates/qh-ffi/src/import.rs`, `crates/qh-driver/src/lib.rs` (copy-in, bila perlu), `crates/qh-driver-postgres/src/lib.rs`, `crates/qh-driver-mysql/src/lib.rs` (baca `max_allowed_packet`), `crates/qh-export/src/writers.rs` (bila builder batch dipakai bersama), tes `crates/qh-ffi/tests/import_batch.rs` (baru) | GP-s | DB, SF, RR, PO | G-RUST, G-GOLDEN (tidak berubah), G-LIVE PostgreSQL dan MySQL (ketiga kebijakan), A/B rows/s lewat toxiproxy 55435 (+30 ms) di mode skip dan default dengan aturan W7 | L | `perf(import): COPY and byte-bounded batches, with row-by-row isolation only after a server error` |
| W12-T7d | Kebijakan konflik dan mode replace untuk impor (IMPORT-2 bagian 4, **bersyarat** keputusan LEDGER-CLOSE untuk DBX-8). Validasi sebelum mengosongkan tabel; MySQL memakai `DELETE FROM` karena TRUNCATE melakukan commit implisit (`classify.rs:580` menganggap TRUNCATE DDL); ON DUPLICATE KEY UPDATE MySQL melaporkan 2 per baris yang diperbarui, jadi hitungan disesuaikan. Butuh PK dari W11-T8. Tingkat **Tinggi**. bp: w12 §7, ADR-0019, ADR-0027. | DBX-8 | `crates/qh-ffi/src/import.rs`, `crates/qh-core/src/batch.rs`, `Models/ImportMapping.swift`, `Views/ImportSheet.swift`, tes `crates/qh-ffi/tests/import_conflict.rs` (baru) | GP-s | DB, SF, SR | G-RUST, G-SWIFT, G-LIVE PostgreSQL dan MySQL (replace yang gagal validasi tidak menyentuh tabel) | M | `feat(import): conflict policies and a replace mode that validates before it clears the table` |
| W12-T7e | Impor `.sql` streaming dua lintasan (IMPORT-2 bagian 5, adopsi dbx). Lintasan 1 mengklasifikasi dengan satu scanner yang bisa dilanjutkan per pembacaan; lintasan 2 mengeksekusi dan menjaga setiap statement lagi; keduanya memberi nomor baris yang sama, jadi penolakan sebelum connect tetap berlaku. Berkas dipatok di antara lintasan (ukuran dan mtime, atau hash). Volume baris `execution_log` dibatasi (`commands.rs:249-285` menulis satu baris per keputusan). Menggantikan pembacaan utuh ADR-0022 untuk berkas besar. Tingkat **Tinggi**. bp: w12 §4 dan §7 (`:24`, `:540`), ADR-0022. | DBX-6 | `crates/qh-sql/src/scan.rs`, `crates/qh-ffi/src/{import.rs,commands.rs}`, tes `crates/qh-ffi/tests/import_sql_stream.rs` (baru) | GP-s | SEC, DB, SF, RR | G-RUST, G-GOLDEN (tidak berubah), G-LIVE PostgreSQL (berkas yang berubah di antara lintasan ditolak; memori puncak dump besar dicatat) | L | `perf(import): stream large .sql files in two passes without losing refuse-before-connect` |
| W12-T8 | Ekspor yang jujur (EXPORT-2, adopsi dbx). Staging: tulis ke berkas sementara di direktori tujuan, rename saat sukses dan saat Stop, hapus saat galat; part `roll_over` memakai `create_new` atau memeriksa nama part sebelum rename pertama (`plan.rs:294-311` sekarang menimpa `_part01`); URL staging diteruskan ke `Support/FileProgress.swift` (sentuhan lintas lane terhadap W12-T6a). XLSX dan XLS: nilai dengan lebih dari 15 digit signifikan (BIGINT, NIK 16 digit, NUMERIC(38,x)) menjadi sel string inline, dan tes `xlsx.rs:680-698` yang mengunci kehilangan itu ditulis ulang. Sel di atas 32.767 karakter dan karakter kontrol yang dibuang dihitung (`truncated()`, unit UTF-16 tanpa memecah surrogate pair) dan dilaporkan dengan teks per format (teks "dbf field width" yang dipatok di `plan.rs:287` diganti). Ekspor SQL mengikuti dialek koneksi dengan override dialek tujuan: karakter kutip, nama berkualifikasi dipecah sebelum dikutip (pakai `crates/qh-sql/src/ident.rs`), backslash MySQL digandakan, bytea PostgreSQL lewat `decode('..','hex')`, float non-finite dikutip atau ditulis NULL dengan peringatan. Tanpa pembungkus CSV `="…"` dan tanpa galat yang ditulis sebagai komentar di dalam berkas (PF-21). DBX-10 ikut hanya bila diadopsi. Doc comment `xls.rs:170-173` disesuaikan. Setiap perubahan output dicatat di `docs/golden-deltas.md` sebagai "Perbaikan disengaja". Tingkat **Tinggi** (DBX-9 kehilangan data). bp: — (tanpa blueprint baru; desain dari backlog DBX-9 dan DBX-33 sampai DBX-35, dicatat di commit). | DBX-9, DBX-33, DBX-34, DBX-35, PF-21, DBX-10 (bersyarat) | `crates/qh-export/src/{plan.rs,writers.rs,lib.rs,xlsx.rs,xls.rs,dbf.rs,parquet.rs,zip.rs}`, `crates/qh-core/src/render.rs`, `crates/qh-ffi/src/commands.rs` (`:566-607`, argumen dialek), `Support/FileProgress.swift`, `Views/Workspace.swift` (override dialek, `:744`), `docs/golden-deltas.md`, tes di `crates/qh-export/tests/` | GP-s | DB, RR, SF, SR | G-RUST, G-SWIFT, G-GOLDEN (selisih baru terklasifikasi); tes: Stop di tengah ekspor tidak meninggalkan berkas setengah jadi dan tidak menimpa berkas lama, `_part01` yang sudah ada tidak tertimpa, NIK 16 digit kembali utuh dari XLSX | L | `fix(export): staged files, exact long numbers, reported truncation, and SQL in the connection's dialect` |
| W12-T9 | Replace `to_table` yang tidak pernah meninggalkan pengguna tanpa tabel (TO-TABLE-SAFE, PF-1, adopsi dbx). Sekarang tabel lama di-drop sebelum CTAS, dan peringatan baru muncul sesudahnya (`commands.rs:1117-1121`, `:1153-1157`). Tabel baru dibangun dengan nama sementara lalu ditukar; di PostgreSQL dengan DDL transaksional; tabrakan nama adalah galat; setiap pesan gagal menyebut backup yang disimpan. Tingkat **Tinggi** (kehilangan data). bp: — (invarian PF-1 di brief), ADR-0027. | PF-1 | `crates/qh-ffi/src/commands.rs` (`to_table`), tes `crates/qh-ffi/tests/to_table_replace.rs` (baru, live) | GP-s | DB, SF, RR | G-RUST, G-LIVE PostgreSQL dan MySQL: SELECT yang gagal dan Stop di tengah CTAS membiarkan baris asli tidak berubah | M | `fix(engine): replacing a table builds the new one first, so a failed export never drops your data` |
| W12-T10 | RAISE NOTICE dan peringatan server, per sesi (adopsi dbx). Pesan yang sekarang dibuang (`crates/qh-driver-postgres/src/lib.rs:277-287`) dikumpulkan per sesi, bukan di peta global berkunci pid seperti dbx; WARNING dari ROLLBACK reset (`:716-749`) dikuras dan dibuang; pesan tampil di panel log per statement (`Views/Panels.swift:148-152`). Kunci event baru bersifat opsional, jadi kasus golden tanpa NOTICE tidak berubah. Tingkat **Tinggi** (driver, event). bp: w12 §4 (bentuk event `script`). | DBX-68; FR-RUN-01 | `crates/qh-driver/src/lib.rs`, `crates/qh-driver-postgres/src/lib.rs`, `crates/qh-driver-mysql/src/lib.rs` (SHOW WARNINGS, bila diadopsi untuk MySQL), `crates/qh-ffi/src/events.rs`, `Support/EngineEvent.swift` (R-ADD), `Views/Panels.swift`, tes `crates/qh-driver-postgres/tests/notices.rs` (baru) | GP-s | RR, DB, SR | G-RUST, G-SWIFT, G-GOLDEN (tidak berubah), G-LIVE PostgreSQL (`DO $$ BEGIN RAISE NOTICE 'x'; END $$` tampil sekali, di sesi yang benar) | M | `feat(engine): server notices and warnings reach the log, per session` |
| W12-T11 | Parameter: `:name` di dalam `[...]` dan jenis daftar (PARAMS, adopsi dbx). Aturan kurung khusus PostgreSQL: `[` konstruktor tetap sadar parameter, `:` slice pada kedalaman kurung yang sama bukan parameter. Aturan itu ada di dua salinan (`Models/SQLScanner.swift`, `crates/qh-editor/src/classify.rs`); keduanya diperbaiki dengan tabel kasus yang sama. Menjadikan Rust satu-satunya pemilik masuk backlog (M, tingkat Tinggi). Jenis parameter daftar untuk `id IN (:ids)`: daftar membawa jenis elemen, setiap elemen lewat `literal()`, dan daftar kosong, rusak, atau elemen yang gagal memberi `ParameterError` (bukan `(NULL)` seperti dbx). Tingkat Sedang. bp: ADR-0028. | DBX-52, DBX-70 | `Models/{SQLScanner,QueryParameters}.swift`, `Views/ParameterSheet.swift`, `crates/qh-editor/src/classify.rs`, tes `QueryParametersTests` dan `crates/qh-editor/tests/params_cases.rs` (baru) | GP-s | SR, RR, DB | G-RUST, G-SWIFT | S | `fix(params): slices inside brackets are not parameters, and a list parameter fills an IN list` |
| W12-T12 | Penyelesaian kode yang mengutip identifier dan sadar komentar (adopsi dbx). Saran yang diterima mengutip identifier bila perlu, lewat `needsQuoting` Swift per dialek, dan menangani kutip yang sudah terbuka; `quotedIdent` (`SchemaTree.swift:235-240`) menggandakan kutip yang tertanam. Penyelesaian tidak lagi tertahan setelah `-- don't`: span cat untuk komentar dan string dibaca ulang dari `Support/EditorAnalysis.swift` tanpa mengubahnya, dan pemindaian dibatasi ke statement saat ini, bukan seluruh dokumen setiap jeda. Tingkat Sedang. bp: w11 §12. | DBX-50, DBX-51; FR-ED-05 | `Views/SQLEditor.swift`, `Models/{AppModel+Completion,SchemaTree}.swift`, tes `CompletionQuotingTests.swift` dan `CompletionCommentTests.swift` (baru: `-- don't`, `/* it's */`, `"it's"`, `$$don't$$`, MySQL `'it\'s'`) | GP-s | SR | G-SWIFT, G-BENCHQ (ketikan) | M | `fix(editor): completions quote identifiers that need it, and an apostrophe in a comment no longer silences them` |

**W12-D** (ADR 0041, 0042) dan **W12-C**, lalu gate W12.

**Perluasan adopsi dbx di W12:**

- **W12-T2** (+ PF-14, aturan PF-13, satu langkah DBX-57): Run, Format, dan preview diabaikan selama `hasMarkedText()` (penjaga di `Views/SQLEditor.swift`); perintah ubah huruf apa pun mengikuti aturan hanya kata kunci (w12 D-4); pembacaan timeout di `AppModel+Focus.swift:110,121` memakai nilai efektif dari W11-T3s.
- **W12-T3** (+ DBX-12, DBX-71 sisi engine): event per statement membawa offset awal statement; komentar pembuka `-- name:` (hanya prefiks `name:`, tanpa fallback ke nama tabel) diteruskan sebagai nama statement.
- **W12-T4** (+ DBX-13, DBX-49 sisi run, DBX-69, DBX-71 sisi UI):
  - DBX-13: hasil yang di-pin dikeluarkan menurut byte dan jumlah, dalam urutan LRU.
  - DBX-49: saat onExit, sukses atau gagal, app mengklasifikasi statement. Hanya CREATE, ALTER, DROP, dan RENAME (bukan TEMP) yang memicu refresh pohon (W11-T4) dan invalidasi penyelesaian (W11-T6b), dengan nama 1 sampai 3 bagian, tanpa perubahan FFI. Split naif dan regex dua bagian dbx tidak diadopsi.
  - DBX-69: pemulihan Run Script (retry, skip, atau skip all setelah kegagalan) tanpa mengulang yang sukses. Hanya untuk outcome `failed`, tidak pernah untuk unknown atau cancelled, dan tidak pernah saat transaksi terbuka. Sisanya berjalan sebagai `script` baru dengan digest sendiri, dengan peringatan bila bagian sebelumnya memakai SET, TEMP, atau BEGIN. Satu baris tambahan di ADR-0042 (W12-D).
  - DBX-71: label result set dari nama `-- name:`.
- **W12-C** (+ DBX-18): korpus tes formatter di `crates/qh-sql/tests/format.rs` (baris kosong, tanda baca lebar penuh, teks mirip XML, AND/OR di komentar, `/*! */`, dollar quote, batas ukuran), ditulis sendiri dari nama kasus dbx.

**Perluasan register utang di W12:**

- **W12-C** (+ DBX-9 sebagai pemeriksaan): tidak ada `File::create` langsung tersisa di `crates/qh-export/src/` (pekerjaannya di W12-T8); sisa ditulis ulang atau dicatat. DBX-18 sudah di atas.
- **W12-T4** (+ W8D-1): `closeTab` melepas semua rujukan ke `QueryTab` sehingga `tabs-100` mencatat `leak_count` 0. Berkas: `Models/AppModel+Focus.swift`, `Models/QueryTab.swift`. Verifikasi: bench `tabs-100` dan G-LEAK.
- **W12-T6b** menunggu W12-T4 untuk hook `runScript` (C-12); tidak ada perubahan lingkup.

### W13: paginasi, rencana query, aktivitas, audit, lokalisasi

**Urutan batch:**
1. T1, T2, dan T6.
2. T3 (setelah T1 dan T2) dan T4 (lane FFI, setelah T2, karena keduanya menyentuh berkas driver).
3. T5 dan T8a (setelah T4, di lajur Rust sendiri).
4. T8b (lane FFI, setelah T4 dan T8a).
5. T8c (setelah T8b).
6. T7 sendirian, sesudah T8c, supaya string panel Analitik lewat katalog.

Tugas adopsi dbx (urutan berkasnya di §7): T9 setelah T8b (lane FFI, `App.swift`) dan W11-T7; T12 setelah T4b dan T3b; T13 setelah T9, T3b, dan W11-T8; T14 setelah T8b; T15 setelah T4b, W12-T7c (rantai driver), dan T9 (`commands.rs`); T16 setelah T15; T11, T17, T10, dan T18 bersyarat dan berurutan di rantai `Connections.swift`. Semuanya mendarat sebelum T7, kecuali T14 (skrip rilis, tanpa string UI).

**W13-A1.** Blueprint `w13-plan-activity.md` (code-architect · opus). Termasuk verifikasi dukungan `EXPLAIN ANALYZE (FORMAT JSON)` di Trino 483 dalam container. Bila tidak didukung, ANALYZE untuk Trino tetap berupa teks.

| ID | Isi | Cakupan | Berkas | Pelaksana | Gate | Verifikasi | Ukuran | Commit |
|---|---|---|---|---|---|---|---|---|
| W13-T1 | Paginasi (P-07) dan "Tampilkan SQL" | FR-GRID-02, 05; V-11 | `Views/ResultGrid.swift` (footer), `Models/QueryTab.swift`, `Models/AppModel+Run.swift` | GP-s | SR, UX, AX | G-SWIFT, G-VIS | M | `feat(grid): fetch more, fetch all and go to row, with the row limit as the one cap` |
| W13-T2 | EXPLAIN JSON dan ANALYZE di engine (lewat setelan, dengan guard statement di dalamnya) | FR-PLAN-01 | `crates/qh-ffi/src/commands.rs`, ejaan explain di driver, `crates/qh-sql/src/classify.rs`, `tests/safe_mode.rs` | GP-s | RR, DB, SEC | G-RUST, G-GOLDEN (tidak berubah), G-LIVE (PostgreSQL, Trino) | M | `feat(engine): JSON plans and EXPLAIN ANALYZE, guarded like the statement they run` |
| W13-T3 | Pohon rencana | FR-PLAN-02; V-11 | `Models/QueryPlan.swift` (baru), `Views/PlanTreeView.swift` (baru), `Views/ResultGrid.swift` (sakelar Tree/Raw) | GP-s | SR, UX, AX | G-SWIFT (parse fixture JSON PostgreSQL dan Trino), scene baru | M | `feat(app): plans read as a tree with the hottest node marked` |
| W13-T4 | Perintah `sessions` dan `session_cancel` dengan guard sendiri (P-12) | FR-SAFE-03 (engine) | `crates/qh-ffi/src/activity.rs` (baru), empat daftar invariant #11, `app/Generated/`, SQL aktivitas di driver, `tests/safe_mode.rs` | GP-s | DB, SEC, RR, SF | G-RUST, G-FFI, G-SWIFT, G-LIVE (cancel pada `pg_sleep` di sesi lain) | M | `feat(engine): list server sessions and cancel one, under Safe Mode` |
| W13-T5 | Tampilan aktivitas server | FR-SAFE-03; V-11 | `Views/ServerActivity.swift` (baru), `Models/AppModel+Activity.swift` (baru), `Views/SchemaOutline.swift` (menu) | GP-s | SR, UX, AX | G-SWIFT, scene baru | M | `feat(app): a server activity view with cancel` |
| W13-T6 | Penampil execution log | FR-SAFE-02; V-11 | `Views/ExecutionLogView.swift` (baru), `Views/Panels.swift` (tab Audit), `Models/AppModel+Audit.swift` (baru) | GP-s | SR, UX, AX, SEC | G-SWIFT | S | `feat(safety): read the execution log and whether its chain holds` |
| W13-T8a | Helper analitik `queryhive-analytics`: DataFusion di workspace dan proses terpisah (O-15, O-18). Implementer **GP-o**, karena kripto spill helper, pool anggaran, dan protokol. | FR-ANL-01…04; NFR-S3, S5 | `crates/qh-analytics-proto/**` (baru, anggota workspace utama); `helpers/analytics/**` (baru: `Cargo.toml` dengan `[workspace]` sendiri dan `rust-version = "1.94"`, `Cargo.lock`, `src/{main,server,session,pool,spill,remote_table,files,udf,output}.rs`, `tests/`); `crates/qh-columnar/src/from_arrow.rs` dan tesnya (fitur `from-arrow`); `crates/qh-result-store/src/spill.rs` (`SpillCipher` dan `SpillFile` dibuka `pub`, tag domain `QHD1`); `crates/qh-rt/src/lib.rs` (runtime tokio ber-QoS); `Cargo.toml` (anggota proto, `exclude = ["helpers/analytics"]`), `Cargo.lock` | GP-o | SEC (wajib: §14.7, §14.8, §14.9), RR, SF, AR | G-RUST (workspace utama, tanpa DataFusion), G-DENY (dua workspace), G-ANALYTICS. Ukuran helper stripped dan terkompresi dicatat terhadap blueprint §2.2. | L | `feat(analytics): a separate DataFusion helper that runs SQL over results inside a leased budget and encrypted spill` |
| W13-T8b | Klien helper di app: spawn di dalam kurungan `sandbox-exec`, sewa anggaran, unduh dan verifikasi, rilis. Implementer **GP-o**, karena umur proses, sewa, dan verifikasi unduhan. | FR-ANL-01, 05; NFR-S5, S7 | `crates/qh-ffi/src/analytics/{mod,client,serve,lease}.rs` dan `profile.sb` (baru), `crates/qh-ffi/src/{analytics_api.rs (baru),host.rs,lib.rs,uniffi_api.rs}`, `crates/qh-ffi/Cargo.toml`, `crates/qh-ffi/tests/analytics_api.rs` (baru), `crates/qh-ffi/examples/bench_ffi.rs` (`sql-*`), `crates/qh-result-store/src/{registry.rs,logical.rs}` (sewa, chunk logis), `Support/AnalyticsComponent.swift` (baru), `App.swift` (`SIGPIPE` diabaikan), `Support/RustEngine.swift`, `Tests/.../{AnalyticsComponentTests,AnalyticsSmokeTests}.swift` (baru), `app/build.sh`, `app/build-dmg.sh`, `app/release.sh`, `app/Generated/` | GP-o | SEC (wajib: §14.2, §14.4, §14.9), RR, SF, SR, AR, CR | G-RUST, G-FFI, G-SWIFT, G-APP, G-ANALYTICS, `./app/release.sh --dry-run`. Throughput pipa (target ≥ 1 GB/s) dan `sql-*` dicatat. | L | `feat(app): fetch, verify and confine the analytics helper, and hand it results chunk by chunk` |
| W13-T8c | Panel Analitik di Settings: status, unduh, pasang dari berkas, hapus. Sebelum W13-T7. | FR-ANL-05; V-11 | `Views/SettingsView.swift`, `Models/AppModel+Analytics.swift` (baru), `Tests/.../AnalyticsSettingsTests.swift` (baru), scene snapshot baru | GP-s | SR, UX, AX | G-SWIFT, G-VIS (scene baru) | M | `feat(app): an Analytics pane that installs, repairs and removes the optional helper` |
| W13-T7 | Fondasi lokalisasi (P-20), serial dan terakhir, sesudah W13-T8c | FR-UI-11 | `app/Package.swift` (`defaultLocalization`), `app/Resources/` (katalog, baru), `app/build.sh` (menyalin atau mengompilasi string ke `Contents/Resources`), `Support/L10n.swift` (baru), string di berkas yang disentuh W9–W13 | GP-s | SR, UX, AX | G-SWIFT, G-VIS tidak berubah, G-APP (bundel memuat string dari katalog) | M | `feat(app): a string catalog, and the strings in the new surfaces go through it` |
| W13-T9 | Pengerasan engine host (HOST-HARDEN, adopsi dbx). Sesi idle diprobe saat checkout bila sudah idle lebih dari sekitar 30 s, dibatasi sekitar 1,5 s; probe tidak dihitung sebagai execute maupun retry, dan pool yang sibuk tidak diprobe. `EngineHost::drop_idle` lewat UniFFI, dipanggil saat `didWake` dan saat jalur jaringan berubah (`Instant` macOS tidak berjalan selama tidur; separuh wake ini ide QueryHive sendiri). ⌘Q benar-benar menghentikan query server: `EngineHost::shutdown(deadline)` menunggu run melihat `RunCancel` dalam batas waktu, tugas stop dihitung sebagai Background, lalu `settle` (sudah ada, belum dipakai); app memakai `.terminateLater`. Bergantung pada DBX-26 di W10-T3 (metode delegate yang sama). Penyimpangan dari `performance-plan.md:244` dicatat di commit. Tingkat **Tinggi** (FFI, umur proses). bp: `fase-2-engine-host.md` (`:118`), ADR-0031. | DBX-47, DBX-48, DBX-59 (sebagian) | `crates/qh-ffi/src/{host.rs,uniffi_api.rs,commands.rs,lib.rs}`, `crates/qh-ffi/src/host/{pool.rs,lease.rs}`, `app/Generated/`, `Support/RustEngine.swift`, `App.swift`, `Support/WakeObserver.swift` (baru), tes `crates/qh-ffi/tests/{host.rs,fault_injection.rs}`, `RustEngineTests` | GP-s | RR, SF, SR, AR | G-RUST, G-FFI, G-SWIFT, G-LIVE PostgreSQL: `pg_sleep(600)` lalu `shutdown(deadline)` (jalur ⌘Q) membuat sesi itu hilang dari `pg_stat_activity`; koneksi yang mati saat tidur diganti tanpa galat di run pertama sesudah bangun | M | `fix(engine): quitting stops server queries, and idle sessions are checked after sleep` |
| W13-T10 | Lingkup produksi per database dan katalog (SAFE-SCOPE, **bersyarat**: ADR baru dengan nomor bebas berikutnya, dan FR baru di PRD). Database atau katalog tertentu bisa ditandai produksi; target setiap statement di-resolve di `qh-sql` dan menjadi sumber lantai baru "target" (sekarang lantai dipasang sekali per run sebelum connect, `commands.rs:122-155`); mutasi yang targetnya tidak bisa di-resolve gagal tertutup. MySQL melacak `USE` (`crates/qh-driver-mysql/src/lib.rs:475-494`). ADR menentukan perilaku `apply_changes` dan `import_data` (di bawah ADR-0026 `confirm` menolaknya). Yang diuntungkan hanya MySQL dan Trino; MCP sudah read-only (NFR-S6). Korpus uji ditulis sendiri dari nama kasus dbx. Tingkat **Tinggi** (Safe Mode). bp: ADR baru, ADR-0017, ADR-0026. | DBX-36; FR baru | `crates/qh-sql/src/{classify.rs,scan.rs}`, `crates/qh-ffi/src/commands.rs`, `crates/qh-driver-mysql/src/lib.rs`, `Models/Connections.swift`, `Views/ConnectionsViews.swift`, `crates/qh-ffi/tests/safe_mode.rs`, ADR baru | GP-s | SEC, DB, RR, SR, AR | G-RUST, G-SWIFT, G-GOLDEN (tidak berubah untuk koneksi tanpa tanda), G-LIVE MySQL (`USE prod; DELETE …` ditolak, `USE dev; DELETE …` lolos) dan Trino | L | `feat(safety): mark single databases as production, and refuse writes whose target cannot be resolved` |
| W13-T11 | Konfirmasi sebelum UPDATE/DELETE tanpa batas di mode `full` dan `no_ddl` (**bersyarat**: mengubah janji `full`, "Anything runs" di `Connections.swift:168`; ADR baru dengan gaya D-11). Subset leksikal saja: tanpa WHERE di tingkat atas, atau WHERE tanpa referensi kolom (`1=1`, `TRUE`). Advisory di app; bila dijadikan guard engine, tingkatnya naik ke Tinggi dan `commands.rs` masuk rantai. Tingkat Sedang. bp: ADR baru, ADR-0026. | DBX-37; FR-RUN-07 | `crates/qh-sql/src/classify.rs`, `Models/{RunConfirmation,Connections}.swift`, `Views/RunConfirmationSheet.swift`, `crates/qh-ffi/tests/safe_mode.rs` (kasus klasifikasi), `RunConfirmationTests` | GP-s | SEC, SR, DB | G-RUST, G-SWIFT, G-VIS bila lembar konfirmasi berubah | S | `feat(safety): ask before an UPDATE or DELETE that touches every row` |
| W13-T12 | History dan query tersimpan mencatat catalog dan schema tempat ia dijalankan (STORAGE-2, adopsi dbx; setelah W12 karena D-16 melarangnya di W12). Migrasi `0010` setelah `0009` milik W13-T4b; schema PostgreSQL hanya informatif (`crates/qh-driver-postgres/src/lib.rs:630-649`). Tingkat **Tinggi** (migrasi DB lokal). bp: w12 (D-16, `:83`, `:482`). | DBX-58 | `crates/qh-storage/migrations/0010_history_context.sql` (baru), `crates/qh-storage/src/{history.rs,lib.rs}`, `Models/{AppModel+History,AppModel+Run}.swift`, tes migrasi di `crates/qh-storage/tests/`, `HistoryTests` | GP-s | DB, SF, SR | G-RUST (migrasi naik dari DB versi 0009 yang berisi data), G-SWIFT | M | `feat(history): history and saved queries remember the catalog and schema they ran under` |
| W13-T13 | Ringkasan seleksi di footer: jumlah, SUM, dan AVG dengan desimal persis yang dihitung di Rust (GRID-UX-2 bagian 2, adopsi dbx; bukan FR-ANL-02). Baris tampilan dipetakan lewat permutasi sort dan filter; "≈" tampil bila ada float. Penjumlahan dengan Number JS seperti dbx tidak diadopsi. Ditambah pemilih label enum PostgreSQL di editor sel, dari metadata W11-T8 (DBX-54 fase 2). Tingkat **Tinggi** (FFI). bp: fase-6 (`StoreRows`), w10 §3. | DBX-62, DBX-54 (fase 2, UI) | `crates/qh-ffi/src/{store_api.rs,uniffi_api.rs}`, `crates/qh-result-store/src/` (agregasi, bila tidak cukup lewat `store_api.rs`), `app/Generated/`, `Support/RustEngine.swift`, `Views/{ResultGrid,ResultGridTable}.swift`, tes `crates/qh-ffi/tests/selection_summary.rs` dan `SelectionSummaryTests.swift` (baru), `VisualParityTests.swift` | GP-s | RR, SR, UX, AX | G-RUST, G-FFI, G-SWIFT, G-VIS (scene footer baru, V-n dinyatakan), G-BENCHQ (scroll) | M | `feat(grid): the footer sums and averages the selection exactly` |
| W13-T14 | Alat rilis (adopsi dbx). Mode `release.sh --rollback <tag>`; v0.0.1 belum punya appcast, jadi belum ada target rollback yang sah, dan menjalankan rollback adalah tindakan keluar yang butuh "ya" pemilik. Guard `release.sh` diuji dengan `gh`, `git`, dan `curl` palsu, dan bug di `:136-137` (nomor build live yang bukan angka membuat guard melewati dirinya diam-diam di bawah `set -e`) diperbaiki. Tingkat rendah. bp: — (`docs/invariants.md:132-146`). | DBX-76, DBX-77 | `app/release.sh`, `tools/release/test_release_guards.sh` (baru), `.github/workflows/repo-hygiene.yml` (menjalankan tes itu) | GP-s | satu reviewer sonnet | `./app/release.sh --dry-run`, tes guard (termasuk nomor build bukan angka) | S | `fix(release): a rollback mode, and the build-number guard can no longer skip itself` |
| W13-T15 | Status TLS hasil negosiasi dan versi server setelah Test Connection (CONN-2 bagian 1, adopsi dbx; dari dbx hanya fakta server). Bila probe `pg_stat_ssl` gagal, tampil "unknown". `securityLabel` (`Connections.swift:405-420`, belum dipakai) dipakai atau dihapus. Termasuk memeriksa temuan sampingan: mysql_async 0.36.2 punya `with_root_certs` (`src/opts/mod.rs:237`), yang bertentangan dengan `docs/tls-modes.md:59,123-129` dan PRD §12.1; hasilnya dicatat sebagai keputusan, lalu dokumen dikoreksi. Tingkat **Tinggi** (output engine, golden). bp: w11 §7, ADR-0040. | DBX-67; FR-CON | `crates/qh-ffi/src/{commands.rs,events.rs}` (output `test`), `crates/qh-driver-{postgres,mysql,trino}/src/lib.rs` (versi dan probe), `Views/ConnectionsViews.swift`, `Models/Connections.swift`, `docs/tls-modes.md`, kasus golden dan `docs/golden-deltas.md` bila output `test` berubah | GP-s | SEC, RR, SR | G-RUST, G-SWIFT, G-GOLDEN (selisih terklasifikasi), G-LIVE TLS PostgreSQL dan MySQL (status yang dilaporkan sama dengan probe di tes `tls.rs`) | M | `feat(connections): Test Connection shows the negotiated TLS and the server version` |
| W13-T16 | Impor koneksi dari DBeaver (lalu DataGrip), dan ekspor/impor daftar koneksi QueryHive sendiri tanpa rahasia (CONN-2 bagian 2, adopsi dbx). Keychain dibaca lewat `SecItemCopyMatching`, dengan batal dan tidak-ditemukan dilaporkan terpisah; jalur folder menjadi satu grup datar "A / B"; `ssl:false` DBeaver tidak menurunkan TLS (invarian PF-6). Ekspor daftar memakai `import_connections` yang sudah ada (`crates/qh-storage/src/import.rs:249-300`); lapisan pemiliknya diputuskan dulu (`rust-engine-blueprint.md:296`). Tingkat **Tinggi** (Keychain). bp: w11 §8–§9. | DBX-73, DBX-74 | `Support/{DBeaverImport,DataGripImport}.swift` (baru), `Models/{Connections,AppModel+Connections}.swift`, `Views/ConnectionsViews.swift` (menu impor), `crates/qh-storage/src/import.rs` (bila ekspor lewat engine), tes `DBeaverImportTests.swift` dan `ConnectionListTransferTests.swift` (baru, fixture ditulis sendiri) | GP-s | SEC, SR | G-SWIFT, G-LIVE (Keychain), G-RUST bila `qh-storage` berubah | M | `feat(connections): import from DBeaver and DataGrip, and move your connection list without secrets` |
| W13-T17 | Mutual TLS (sertifikat klien) untuk PostgreSQL, MySQL, dan Trino (CONN-2 bagian 3, **bersyarat**: FR baru, dan membaca kunci bertentangan dengan D-15 w11 §7.2). Kunci terenkripsi ditolak di v1; `Debug` disensor, kunci di-zeroize, hanya fingerprint di kunci pool. mysql_async 0.36.2 punya `with_client_identity`. Tingkat **Tinggi** (kepercayaan). bp: w11 §7 (`:634`), amandemen ADR-0040. | DBX-38; FR baru | `crates/qh-driver/src/lib.rs` (`ConnectionConfig`), `crates/qh-driver-postgres/src/tls.rs`, `crates/qh-driver-mysql/src/tls.rs`, `crates/qh-driver-trino/src/lib.rs`, `crates/qh-ffi/src/config.rs`, `crates/qh-ffi/src/host/pool.rs`, `Models/Connections.swift`, `Views/ConnectionsViews.swift`, `Cargo.lock` bila ada crate baru, tes TLS dengan CA dan sertifikat klien uji | GP-s | SEC, RR, SF, SR | G-RUST, G-DENY, G-SWIFT, G-LIVE (PostgreSQL dan MySQL dengan `clientcert=verify-full`) | L | `feat(connections): client certificates for PostgreSQL, MySQL and Trino` |
| W13-T18 | Definisi tunnel SSH bersama (**bersyarat**: PRD §10 mencatatnya Later, `prd:370`; tanpa keputusan, tetap Later dan tidak dijadwalkan). Nama "profile" sudah dipakai (`crates/qh-storage/src/identity.rs:206`, `crates/qh-credentials/src/lib.rs:69`), jadi pakai istilah lain. Tingkat **Tinggi**. bp: w11 §5–§6, ADR-0039. | DBX-75 | `crates/qh-ffi/src/host/pool.rs` (kunci tunnel, `:142-174`), `crates/qh-ffi/src/mcp.rs` (`:1310-1319`), `crates/qh-storage/` (tabel baru), `Models/Connections.swift`, `Views/ConnectionsViews.swift` | GP-s | SEC, RR, SR | G-RUST, G-SWIFT, G-LIVE (SSH) | L | `feat(connections): SSH tunnels defined once and shared by connections` |

**W13-D** (ADR 0043, 0044, 0045, adendum 0007, 0010, 0014) dan **W13-C**, lalu gate W13: G-HEAVY, G-ANALYTICS, dan ambang NFR-P3 untuk SQL yang ditetapkan dari angka `bench_ffi sql-*`.

**Perluasan adopsi dbx di W13:**

- **W13-T2** (+ DBX-3, DBX-4): daftar tolak fungsi berefek samping masuk kode `crates/qh-sql/src/classify.rs`, bukan hanya tes seperti di plan-b1. `pg_terminate_backend`, `pg_cancel_backend`, `nextval`, `setval`, `lo_import`, dan advisory lock tidak lagi terklasifikasi ReadOnly, dan ini dibuktikan dulu dengan tes yang gagal. EXPLAIN ANALYZE berjalan di dalam transaksi READ ONLY yang di-rollback. Gate SEC wajib (S-2, DoD #6).
- **W13-T4** (+ DBX-23): SQL aktivitas bekerja di hot standby dan di PostgreSQL 17 (`pg_stat_bgwriter` pindah).
- **W13-T5** (+ DBX-24): batch cancel di tampilan aktivitas, atau catatan "tidak diadopsi" (guard `inFlight` sudah ada di `d1b5652`).
- **W13-T7** (+ PF-27): pemeriksaan kelengkapan lokalisasi tetap offline: setiap kunci ada di setiap locale, placeholder sama, tanpa terjemahan otomatis di CI.
- **W13-T8a** (+ PF-26): setiap manifest mandiri masuk CI dengan `cargo metadata --locked` dan pemeriksaan tanpa jaringan `cargo tree -e normal`, di commit yang membuatnya; `.github/workflows/verify.yml` masuk daftar berkas T8a (setelah W8-T3).

**Perluasan register utang di W13:**

- **W13-T2** (+ B-16, B-17, B-18): `FOR` key/no/share tidak lagi positif palsu (B-16); teks `client_encoding` tidak lagi over-refusal (B-17); tes bahwa lease memasang ulang read-only setelah reconnect di tengah run (B-18). Berkas: `crates/qh-sql/src/classify.rs`, `crates/qh-ffi/tests/`. Gate SEC yang sama.
- **W13-D** (+ L-6): satu baris di `docs/invariants.md` atau ADR Safe Mode: MariaDB (`/*M!`) di luar lingkup, dibaca sebagai komentar biasa (`scan.rs:48,1186`, `classify.rs:1049`).

### W14: final

Lihat §9.

**Perluasan adopsi dbx di W14** (baris tugas di §9):

- **W14-T1** (+ DBX-44): setiap tes live diberi `#[ignore = "requires QH_TEST_X=1"]` per tes, bukan per berkas (`local.rs` dan `host.rs` mencampur tes offline dan live; `scan_refactor.rs:913` dan `seal_bench.rs:29` tidak disentuh), G-LIVE di §1 memakai `-- --include-ignored`, dan komentar basi yang mengklaim CI menyetel variabel itu (`crates/qh-driver-postgres/tests/integration.rs:8-10`) dibetulkan. Dikerjakan sebelum T1 mencatat hitungan, supaya hitungan G-LIVE menyebut tes yang benar-benar jalan.
- **W14-T4** (+ PF-25): arah dependensi crate diperiksa lewat `cargo metadata` atau `[bans]` cargo-deny: driver tidak bergantung pada `qh-sql`, `qh-ffi`, `qh-export`, atau `qh-import`, dan crate store bebas driver (invarian #3 dan #8). Tanpa pemeriksaan string sumber.
- **W14-T5** (+ DBX-78 bersyarat, PF-24): daftar uji penerimaan update tertulis yang memakai override `FEED_URL` (`app/build.sh:22-29`), tanpa kasus arsitektur salah karena app hanya arm64; catatan bahwa jaminan sesi pool butuh session pooling, bukan transaction pooling PgBouncer.
- **W14-T7** (+ DBX-43, DBX-61): hasil CI remote menjadi bagian dari "done" dan dilaporkan terpisah dari gate lokal; lima aturan kualitas tes masuk `AGENTS.md` (tes bisa gagal karena alasan yang ia sebut; banding dengan sumber yang ditulis independen; perilaku lewat entry point produksi, mock hanya batas eksternal; teks sumber hanya untuk properti call-site dan kontrak; guard lama dipertahankan sampai cakupan perilaku menggantikannya).

- **W14-T4** (+ register utang L-7, L-9, L-16, L-17, L-27): catatan non-blokir W3-T0 r2 N1 sampai N3 diperiksa; catatan Trino tanpa retry tulis di ADR-0031 dan butir c1 W3-T1 yang tersisa (N2 Notify tunggal, N6 tes PG butuh superuser, N9 `connection_id`); celah tes undo AppKit (L-16) dan deviasi `regions()` serta antrean `qh.editor.visible` (L-17) diterima atau dites; tes yang gagal tanpa pemeriksaan awal 64 MiB sebelum render (`qh-ffi/tests/store_sink.rs`, L-27).
- **W14-T5** (+ B-12, L-44, C-8): `THIRD-PARTY-NOTICES.md` memuat inventaris crate penuh, pemasangan bundel, dan helper (B-12); dokumen W8-D disegarkan terhadap hasil F1 dan F3 (L-44); deviasi w9 D-17 (tab di baris atas, tanpa breadcrumb toolbar) tercermin di blueprint dan `app/DESIGN.md` (C-8).
- **W14-T6** (laporan, C-13): batas direktif `ssh_config` yang ditolak dengan nama (Match, ProxyJump, ProxyCommand, HostKeyAlias, dan sebagainya) masuk daftar "batas yang dinyatakan" (§9 butir 8).
- **W14-T7** (+ L-45): bagian "Resume" di `AGENTS.md` diperbarui di setiap jeda, bukan hanya di akhir.
- **W14-T9 sampai W14-T12** (baru, register utang): lihat §9.
- **W14-T8** (baru, DBX-45): lihat §9.

## 6. Pemetaan subagen

| Peran | Agen | Model | Dipakai di |
|---|---|---|---|
| Implementasi Rust, Swift, dan Python | general-purpose | sonnet secara default. **opus** hanya untuk W3-T0, W3-T1, W5-T1, W13-T8a, dan W13-T8b (O-17; alasannya di masing-masing tugas). Semua tugas adopsi dbx dan register utang berimplementer sonnet | semua `Tx` implementasi |
| Tes lebih dulu | tdd-guide | sonnet | W2-T1, W3-T0, W3-T1 |
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
| Keamanan | security-reviewer | opus | FFI, spill, SSH, Keychain, JWT, CA, MCP, guard aktivitas, Safe Mode (W3-T0, `scan.rs` di W3-T2), helper analitik (W13-T8a dan W13-T8b wajib: unduhan, kurungan, spill helper). Adopsi dbx: W11-T2b1d (cancel TLS), W11-T3s dan W13-T16 (Keychain, TOFU), W11-T7, W12-T7b dan W12-T7e (`qh-sql`), W13-T2 (daftar tolak S-2), W13-T9, W13-T10, W13-T11, W13-T15, W13-T17, W13-T18; W14-T9 (review penutup) |
| SQL | database-reviewer | opus | reset pool, COPY, metadata, `script`, EXPLAIN, aktivitas, INSERT, formatter, SQL pembungkus Batch 7. Adopsi dbx: W10-T8, W11-T8, W12-T7a sampai T7e, W12-T8, W12-T9, W12-T10, W13-T10, W13-T12 |
| Jalur galat | silent-failure-hunter | opus | cancel, pool, spill, `script`, impor, helper analitik. Adopsi dbx: W11-T7, W11-T8, W12-T7a, W12-T7b, W12-T7c, W12-T7e, W12-T9, W13-T9, W13-T17 |
| Desain tipe | type-design-analyzer | opus | `EngineHost`, `ResultRows`, `EditorDocument`, `ResultHandle` |
| Galat build | swift-build-resolver, rust-build-resolver | sonnet | §8 |
| Bug keras dan gate merah | debugger | opus | §8 |
| ADR | adr-generator | sonnet | `Wx-D` |
| Dokumen | doc-updater | sonnet | `Wx-D`, W14-T5, W14-T7 (skill `writing-for-agents`) |
| Kode mati | refactor-cleaner | sonnet | `Wx-C` |
| Penyederhanaan | code-simplifier | sonnet | `Wx-C` |
| Pencarian untuk menyusun brief | Explore | haiku | orkestrator, sebelum setiap tugas |

### Kebijakan review berjenjang menurut risiko (O-17)

Kolom **Gate** di §5 adalah daftar spesialis maksimum untuk tugas berisiko tinggi. Tingkatnya menentukan siapa yang benar-benar dipanggil dan berapa putaran.

| Tingkat | Yang termasuk | Reviewer | Putaran maksimal |
|---|---|---|---|
| Tinggi | Tugas berimplementer opus, dan yang menyentuh Safe Mode, kripto dan spill, kepercayaan (SSH, JWT, CA), umur proses atau handle, pool sesi, atau batas FFI | opus, hanya spesialis yang relevan dari kolom Gate (bukan seluruh daftar) | 2 (O-19) |
| Sedang | Tugas implementasi lain | satu reviewer, yang paling relevan dari kolom Gate | 2 |
| Rendah | Dokumen, ADR, tooling, hanya tes, hanya pemindahan kode, dan pembersihan (`Wx-C`) | satu reviewer sonnet, atau cukup gate | 1 |

- Hanya temuan yang **memblokir** memicu satu putaran. Saran dan nit dicatat di laporan tugas, bukan putaran.
- Implementer sonnet secara default. Opus hanya untuk W3-T0, W3-T1, W5-T1, W13-T8a, dan W13-T8b; tugas adopsi dbx dan register utang semuanya sonnet. Opus hanya sebagai reviewer, dan untuk W14-T9 (REVIEW-CLOSE) serta debugger bila W14-T10 buntu.
- **O-20 (kebijakan sesi berjalan):** semua pekerjaan mahal (implementasi, blueprint, dokumen, bench) berjalan di sonnet atau haiku. Reviewer boleh opus tetapi hanya satu putaran. Setelah temuan yang memblokir, sonnet memperbaiki, orkestrator memverifikasi dengan tes dan gate, meng-commit, lalu mencatat "pending review" di ledger. Ini mengesampingkan tabel model dan batas putaran di atas selama sesi ini.
- Verdict tetap "approved" dengan daftar berkas (§8). Yang berubah adalah jumlah reviewer dan putarannya, bukan bukti verifikasinya.

## 7. Paralelisme dan kepemilikan berkas

**Aturan dasar.** Satu berkas bersama hanya boleh dimiliki satu tugas yang sedang berjalan. Urutan di tabel ini mengikat.

**Status per 2026-10-06.** Mata rantai W9 dan W10 di tabel disalin dari blueprint (`w9-shell-and-a11y.md` §16.2 dan `w10-grid-editor-design.md` §12.4), lalu disesuaikan dengan berkas yang benar-benar disentuh commit W9 (`git log --stat work/perf-parity`). Tanda "(selesai)" berarti tugasnya sudah mendarat di `work/perf-parity`: W9-T0 `e708830`, W9-T1 dan W9-T6 satu commit `03e1495`, W9-T2 `2cf315c`, W9-T3 `7cf63f4`, W9-T4 `9890bd7`, W9-T5 `a6f866b`, W9-T7 `efdddab`. Dari W9 masih tersisa T8 dan T9. W9-T0b (`ChromeParityTests` dan baseline `chrome-*`, blueprint W9 §15.1) belum dibangun: ia harus mendarat sebelum W9-T8, atau dicoret secara eksplisit. Dari W10 baru inti Rust W10-T7a (`crates/qh-editor/src/brackets.rs`, `2515a8b`) yang mendarat. Sisanya menunggu, dan lane W10 dijadwalkan dari rantai di bawah.

| Berkas atau kelompok | Pemilik, berurutan |
|---|---|
| `app/Generated/`, `crates/qh-ffi/src/uniffi_api.rs`, empat daftar invariant #11, `Support/RustEngine.swift` (lane FFI) | W11-T1 → W12-T1b → W11-T2b2 → W10-T7b → W11-T6b → W12-T3 → W13-T4 → W13-T8b → W13-T9 → W13-T13. W10-T6a takes `uniffi_api.rs`/`lib.rs` between W11-T1 and T2b2 without regenerating. W11-T7 memegang `lib.rs` di antara W11-T6b dan W12-T3 tanpa meregenerasi. W11-T8, W12-T10, dan W13-T15 tidak mengubah permukaan UniFFI; bila ternyata mengubahnya, mereka antre sesudah W13-T13 dan penyimpangannya dicatat. |
| `crates/qh-core/src/error.rs` | W10-T6a → W11-T2b1d → W11-T7 |
| `crates/qh-ffi/src/editor.rs` | W12-T1b → W10-T7b → W11-T6b |
| `crates/qh-ffi/Cargo.toml` | W4-T2 → W5-T2 → W13-T8b |
| `crates/qh-ffi/src/commands.rs` | W11-T1 → W13-T2 → W11-T7 → W12-T9 → W12-T3 → W12-T8 → W12-T7e → W13-T9 → W13-T15 → W13-T10 (bersyarat) |
| `crates/qh-ffi/src/lib.rs` (baris `mod` dianggap boleh digabung tangan) | ikut lane FFI |
| Tiga driver | W7-C → W11-T1 → W10-T6a → W11-T2b1d → W13-T2 → W11-T7 → W11-T8 → W13-T4 → W12-T10 → W12-T7c → W13-T15 → W13-T17 (bersyarat) → W13-T10 (bersyarat) |
| `crates/qh-ffi/src/mcp.rs` | W11-T5 → W11-T3r → W13-T18 (bersyarat) |
| `Cargo.toml`, `Cargo.lock` | X4 reverts → W11-T2b1d → W8-T4 (`Cargo.toml` saja) → W13-T8a → W13-T17 (bersyarat, bila menambah crate) |
| `crates/qh-sql/src/{scan.rs,classify.rs}` | W3-T0 → W3-T2 → W7-T2 (pemeriksaan satu SELECT, bila menyentuh) → W13-T2 (`classify.rs`, kini termasuk kode daftar tolak DBX-3) → W12-T7b → W12-T7e (`scan.rs`) → W13-T11 (bersyarat, `classify.rs`) → W13-T10 (bersyarat) |
| `crates/qh-sql/src/lib.rs` | W3-T2 → W12-T1a′ → W12-T7b (bila API publik bertambah) |
| `crates/qh-editor/**` | W3-T2 → W10-T7b (`src/brackets.rs`) → W11-T6b (`src/refs.rs`) → W12-T11 (`src/classify.rs`) |
| `crates/qh-rt/src/lib.rs` | W2-T1 → W4-T3 → W13-T8a |
| `crates/qh-columnar/**` | W4-T3 → W7-T1 → W13-T8a |
| `crates/qh-result-store/src/spill.rs` | W4-T3 → W13-T8a |
| `crates/qh-result-store/src/{registry.rs,logical.rs}` | W4-T3 → W13-T8b → W13-T13 (bila agregasi seleksi butuh store) |
| `helpers/analytics/**`, `crates/qh-analytics-proto/**` | W13-T8a (baru) |
| `crates/qh-ffi/examples/bench_ffi.rs` | W1-T2 → W3-T1 → W5-T2 |
| `tests/golden/`, `crates/qh-ffi/tests/golden.rs` | W2-T1 → W11-T1 → W12-T3 → W11-T8 (bila kasus `_live` metadata berubah) → W12-T8 → W13-T15. Entri `docs/golden-deltas.md` mengikuti urutan yang sama. |
| `Models/AppModel.swift` | W1-T4 → W2-T2 → W3-T1 → W4-T1 → W6-T1 → W9-T0 (selesai) → W9-T2 (selesai; satu baris, `var navigation`). Sesudahnya, satu pemilik per extension. |
| `Models/AppModel+Focus.swift` | W10-T7b → W10-T3 (`closeTab`, DBX-26) → W12-T2 (juga pembacaan timeout DBX-57) → W12-T4 |
| `Models/QueryTab.swift` | W10-T6b → W10-T5 → W10-T3 → W10-T8 → W10-T9 → W11-T4 → W12-T2 → W12-T4 → W13-T1 → W13-T3b → W13-T6. W10-T8 dan W10-T9 menambah dua mata rantai di jalur kritis. |
| `Views/ResultGrid.swift` | W9-C → W10-T5 → W10-T3 → W10-T4 → W12-T4 → W13-T1 → W13-T3b → W13-T13 |
| `Models/CellSelection.swift` | W5-T1 → W6-T1 → W10-T1 (`GridCursorMath`) → W10-T3 (`GridClipboard.text` memilah blok, W10 §5.1) → W10-T4 (`GridClipboard`; anggaran salin DBX-40) → W10-T9 (shift-klik, autoscroll) |
| `Views/ResultGridTable.swift`, `Views/GridTableView.swift`, `GridRowView`, `GridHeaderView`, `Models/GridMetrics.swift`, `Views/GridAccessibility.swift`, dan `VisualParityTests.swift` | W5-T1 → W6-T1 → W9-T1 (selesai; hanya `VisualParityTests.swift`) → W9-T7 (selesai; semua berkas kelompok ini kecuali `GridAccessibility.swift`) → W10-T1 → W10-T2 → W10-T3 → W10-T8 → W10-T4 → W10-T9 → W13-T13 |
| `Views/SQLEditor.swift` | W7-C → W10-T6b → W10-T7b → W11-T4 → W12-T2 → W12-T12 |
| `Views/Workspace.swift` | W9-T8 → W9-T9 → W10-T6b → W10-T7b → W11-T4 → W12-T2 → W12-T4 → W12-T8 |
| `Views/RootView.swift` | W9-T8 |
| `App.swift` | PREP-EV → W9-T8 → W10-T3 (juga `applicationShouldTerminate`, DBX-26) → W12-T2 → W12-T4 → W13-T5 → W13-T8b → W13-T9 |
| `Models/AppMenu.swift`, `Models/Shortcuts.swift` (R-ADD) | W10-T1 → W10-T3 → W10-T5 → W10-T7b. `Shortcuts.swift` lanjut ke W12-T2 (w12 §10). |
| `Views/SettingsView.swift` | W9-T9 → W13-T8c → W13-T7 |
| `app/build.sh` | W13-T8b → W13-T7 → W14-T5 |
| `app/release.sh`, `app/build-dmg.sh` | W13-T8b → W13-T14 (`release.sh` saja) |
| `THIRD-PARTY-NOTICES.md` | W4-T2 (commit A) → W14-T5 |
| `AGENTS.md`, `CLAUDE.md` | W14-T7 (baru) |
| `Support/Theme.swift` | W9-T9 → W10-T2 |
| `Support/ThemeStore.swift` | W9-T1 (selesai) |
| `Views/ConnectionsViews.swift`, `Models/Connections.swift` | W11-T3s → W13-T4b → W13-T15 → W13-T16 → W13-T11 (bersyarat) → W13-T17 (bersyarat) → W13-T10 (bersyarat) → W13-T18 (bersyarat) |
| `Support/Snapshot.swift` (R-ADD) | W9-T8 → W10-T5 → W10-T6b → W11-T3s → W11-T4 → W12-T4 → W13-T5 → W13-T6 |
| `Support/EditorAnalysis.swift` | W10-T6b → W10-T7b |
| `Support/EditorPreferences.swift` | W10-T7b |
| `Models/RunConfirmation.swift`, `Views/RunConfirmationSheet.swift` | W9-T5 (selesai) → W9-C (`Request.isDestructive`) → W13-T11 (bersyarat) |
| `Support/PerfSignposts.swift` | W1-T4 → W4-T2 → W10-T4 (signpost salin, DBX-40) |
| `Support/BenchMode.swift` | W1-T4 → W4-T2 → W6-T1 |
| `crates/qh-ffi/src/host.rs` | W11-T1 → W12-T3 → W13-T4 → W13-T4b → W13-T8b → W13-T9 |
| `crates/qh-ffi/src/host/pool.rs`, `crates/qh-ffi/src/retry.rs` | W11-T2b2 → W11-T7 → W13-T9 → W13-T17 (bersyarat) → W13-T18 (bersyarat) |
| `crates/qh-ffi/src/host/lease.rs` | W13-T2 → W11-T7 → W13-T4 → W13-T9 |
| `crates/qh-ffi/src/events.rs` | W10-T6a → W11-T2b2 → W12-T3 → W12-T10 → W13-T15 |
| `crates/qh-ffi/src/config.rs` | W11-T2b2 (termasuk bagian Rust DBX-29 bila ada) → W13-T17 (bersyarat) |
| `crates/qh-ffi/src/import.rs` | W12-T5b-R (selesai) → W12-T7a → W12-T7b → W12-T7c → W12-T7e → W12-T7d (bersyarat) |
| `crates/qh-ffi/tests/safe_mode.rs` | W11-T1 → W13-T2 → W12-T3 → W13-T4 → W13-T4b → W13-T11 (bersyarat) → W13-T10 (bersyarat) |
| `crates/qh-export/src/**` | W12-T8 → W12-T7c (bila `writers.rs` dipakai bersama) |
| `crates/qh-import/src/**`, `Models/ImportMapping.swift`, `Views/ImportSheet.swift`, `Models/AppModel+Export.swift` | W9-T9 (`ImportSheet.swift`) → W12-T7a → W12-T7d (bersyarat) |
| `crates/qh-storage/migrations/`, `crates/qh-storage/src/lib.rs` | W13-T4b (`0009`) → W13-T12 (`0010`) → W13-T18 (bersyarat) |
| `crates/qh-storage/src/import.rs` | W11-T3r → W13-T16 (bila ekspor daftar lewat engine) |
| `Models/CellEdits.swift` | W10-T2 → W10-T3 → W10-T8 |
| `Models/{WritePlan,MatchPolicy}.swift` | W10-T3 → W10-T8 → W11-T8 |
| `Models/{InsertStatements,UpdateStatements,BoundSQL}.swift` | W10-T8 → W10-T4 |
| `Models/AppModel+Run.swift` | W10-T6b → W12-T4 → W12-T6b → W13-T1 → W13-T3b → W13-T12 |
| `Models/AppModel+Connections.swift` | W11-T3s → W13-T4b → W13-T16 |
| `Models/AppModel+Completion.swift` | W11-T6b → W12-T12 |
| `Models/SchemaTree.swift` | W9-T3 (selesai) → W11-T4 → W12-T12 |
| `Views/Panels.swift` | W9-T9 → W12-T4 → W12-T10 → W13-T6 |
| `.github/workflows/verify.yml` | W8-T3 → W13-T8a → W14-T8 |
| `.github/workflows/repo-hygiene.yml` | W8-T3 → W13-T14 |
| `__Baselines__/` | hanya lewat langkah merge orkestrator (§0.5) |

**Adopsi dbx (2026-10-06).** Mata rantai untuk W8-T4, W10-T3 (`AppModel+Focus.swift`), W10-T8, W10-T9, W11-T7, W11-T8, W12-T7a–e, W12-T8 sampai T12, W13-T9 sampai T18, dan W14-T8 ditambahkan tanpa mengubah urutan relatif tugas yang sudah ada. Baris baru untuk `host.rs`, `events.rs`, `safe_mode.rs`, `AppModel+Run.swift`, `AppModel+Connections.swift`, dan `Panels.swift` juga memuat urutan O-25 (plan-b1) yang belum tertulis di tabel. "(bersyarat)" berarti tugas itu dilewati bila keputusannya tidak diadopsi, dan urutan sisanya tetap.

**Freeze** (`performance-plan.md` §0). Selama Fase 4 dan 5 tidak ada fitur yang menyentuh `SQLEditor.swift` atau `ResultGrid.swift`. Urutan gelombang di atas sudah memenuhinya: fitur baru dimulai di W9.

**Worktree dipakai** di W2 (T1, T2, T3), W4 (T1, T2, T3), W5 (T1, T2), dan W7 (T2, T4). Di W9 sampai W13, worktree dipakai setiap kali dua lane berjalan bersamaan. Tugas serial dikerjakan di checkout utama.

**Verifikasi ronde 2 (2026-10-06): 1 blocking, dikoreksi; pending review (O-20).** Temuannya: rantai kepemilikan W9 dan W10 hanya ada di blueprint (W9 §16.2, W10 §12.4), belum di plan. Tabel di atas sekarang memuat rantai itu, disesuaikan dengan apa yang benar-benar disentuh commit W9. Klaim kode diperiksa terhadap `work/perf-parity` (`4ec7480`): `var navigation` ada di `AppModel.swift:125`, `closeTab` ada di `AppModel+Focus.swift:154`, `adjustFontSize` belum ada, dan `Snapshot.swift` hanya memuat `store.pin(...)` dari W9-T1 (tanpa `--reduce-*` dan `--grid-font`). Daftar berkas per tugas W9 diambil dari `git show --name-status` tiap commit. Sisi §5 dari temuan yang sama (daftar berkas W9-T1, T2, T5, T7 dan W10-T1 sampai T7) tidak dikerjakan di sini. Penyimpangan W9 yang tidak punya baris sendiri karena tak ada pemilik berikutnya: `Support/Accessibility.swift` dibuat W9-T2 (bukan W9-T1), dan W9-T7 menyentuh `Views/SQLSyntax.swift`.

## 8. Gate dan penanganan kegagalan

**Gate tugas.**

- Verifikasi tugas lulus, dan setiap reviewer yang dijadwalkan menurut tingkat risikonya (§6, O-17) memberi verdict **approved** dengan daftar berkasnya (aturan "done means verified").
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
3. **Reviewer meminta perubahan (O-17; sesi berjalan mengikuti O-20 di §6):** hanya temuan yang memblokir memicu putaran; saran dan nit dicatat saja. Implementer memperbaiki lalu review ulang, dengan batas global maksimal 2 putaran (O-19; rendah 1). Setelah putaran terakhir, perbaikan yang tersisa diverifikasi lewat tes dan gate lalu dicatat sebagai pending review, tanpa putaran ketiga.
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
| W14-T1 | G-HEAVY penuh, termasuk G-GOLDEN dengan Trino, semua G-LIVE, dan G-ANALYTICS. `./app/build.sh` dan semua scene `--snapshot`. | test-engineer · sonnet | semua hijau, hitungan tes dicatat |
| W14-T2 | Sesi bench final untuk semua sumbu (eksklusif), TablePro best-effort, laporan diregenerasi | performance-engineer · sonnet; PO | `docs/benchmarks.md` hasil generator |
| W14-T3 | Laporan paritas visual: baseline P, lalu setelah Fase 5 dan 6, lalu final. Pasangan berdampingan untuk setiap V ditulis ke `app/.build/parity-report/` (tidak di-commit), dengan indeks. | test-engineer · sonnet; UX | semua scene non-V lulus |
| W14-T4 | Pass perbaikan akhir dan review per area atas diff branch (engine, store, grid, editor, shell, koneksi), plus review keamanan atas semua diff yang relevan | refactor-cleaner, code-simplifier · sonnet; CR, SEC · opus | approved per area |
| W14-T5 | Dokumen: `PROGRESS.md`, `app/DESIGN.md` (Scope, grid, editor, shell, CLI, Stop, formatter, skrip), `docs/invariants.md` (hanya insiden nyata), status di `performance-plan.md`, `remaining-work-plan.md`, `tablepro-adoption-plan.md`, `tablepro-feature-map.md`, dan `tablepro-design-audit.md`. Ditambah `THIRD-PARTY-NOTICES.md` lengkap (semua crate dan grammar yang dikirim, termasuk helper) dan pemasangannya ke bundel lewat `app/build.sh` (kriteria rilis PRD §9). | doc-updater · sonnet | satu reviewer sonnet (O-17); `THIRD-PARTY-NOTICES.md` ada di `Contents/Resources` setelah `./app/build.sh` |
| W14-T6 | Merge: under O-21 every commit is already fast-forwarded to main, so `merge --no-ff` does nothing. Run the final G-HEAVY (+G-ANALYTICS) on main, which equals `work/perf-parity`, then write the §9 report. | orkestrator | hijau |
| W14-T7 | Panduan agen repo (O-16): `AGENTS.md` sebagai sumber dan `CLAUDE.md` yang menunjuk ke sana, supaya AI mana pun yang bekerja di codebase ini menjaga kualitasnya. Isinya gate (§1), invariant, anggaran performa, gate paritas, batas lisensi (TablePro AGPL-3.0, QueryHive MIT), dan pelajaran yang didapat selama run. Ditulis dengan skill `writing-for-agents`. Nomor terakhir, tetapi dijalankan sebelum W14-T6 dan ditinjau sebelum merge, supaya ikut masuk `main`. | doc-updater · sonnet (skill `writing-for-agents`) | satu reviewer sonnet (O-17); setiap path dan perintah yang disebut ada dan dijalankan, dan `CLAUDE.md` hanya menunjuk ke `AGENTS.md` |
| W14-T8 | CI live di Linux (adopsi dbx DBX-45). Pertama `cargo check` di Ubuntu; lalu service container postgres:17 dan mysql:8.4 (trust auth untuk `import_live`) menjalankan driver PostgreSQL dan MySQL, `real_server`, dan `import_live`; Trino di job opsional yang tidak memblokir. Lantai versi diukur ulang tiap minggu (`docs/compatibility.md`). Berkas: `.github/workflows/verify.yml` (setelah W13-T8a), `.github/workflows/live-linux.yml` (baru), `docs/compatibility.md`. Butuh W14-T1 (`#[ignore]` per tes). Tingkat rendah. bp: `tablepro-adoption-plan.md` §0.2. Sebelum W14-T6. | GP-s · sonnet; satu reviewer sonnet | job baru hijau di main (`gh run list`); job Trino boleh merah tanpa memblokir |
| W14-T9 | REVIEW-CLOSE (L-5, L-8, L-31): satu review opus atas semua yang masih "pending review": W3-T0 `1f3182d` (scan MySQL, SEC), W3-T1 F `a8174f8` (SEC, RR), W6-A1 `c19296e`, W6-D `f2cbf46`, blueprint W9, W10, W12, W13 `8d700ba`, dan koreksi putaran 2 `a6bd100` (fase-6 §17.2, ADR 0030 dan 0034 `displayedRows`, rantai §7, w12 §5.1). Sebelum W14-T4. Satu putaran (O-20): temuan memblokir diperbaiki, diverifikasi gate, dan dicatat; koreksi dokumen oleh doc-updater. Tingkat sedang. | code-reviewer, security-reviewer · opus; doc-updater · sonnet | setiap baris "PENDING REVIEW" di `target/run/ledger.md` menjadi "approved" dengan daftar berkas, atau backlog bernama |
| W14-T10 | FLAKE-SOAK (B-10, L-10, C-7): soak ×20 untuk `EngineContractTests ...AnswersOffTheMainQueue` (hang sekitar 1 dari 6), probe kill tingkat pool yang `written`-nya kadang 0 (`crates/qh-ffi/tests/host.rs:2320-2339`), dan `import_live` yang gagal connect tanpa password (`crates/qh-ffi/tests/import_live.rs`). Setiap tes diperbaiki sampai ×20 hijau, atau dikarantina dengan alasan tertulis di ledger. Berkas: `app/Tests/QueryHiveTests/EngineContractTests.swift`, `crates/qh-ffi/tests/{host,import_live}.rs`. Sebelum W14-T1, supaya hitungan gate jujur. Tingkat sedang. | GP-s · sonnet; debugger · opus bila akarnya belum ketemu; satu reviewer | ×20 hijau per tes atau alasan karantina di ledger; G-RUST dan G-SWIFT hijau |
| W14-T11 | REPO-HYGIENE (L-47, L-49): hapus atau abaikan `target-logs/` di akar repo dan tiga worktree; hapus cabang lane sisa (`lane/w7-t1`, `lane/w11-t2b1d`, `lane/w13-t5`, `lane/w13-t8a`, `lane/w9-t9`) dan worktree-nya setelah commit dan `git status` bersih, tanpa `--force`; bersihkan artefak cargo basi dan periksa `df`. Worktree asing opencode (L-48) bukan buatan run ini, jadi dilaporkan, bukan dihapus. Sebelum W14-T6. Tingkat rendah. | orkestrator solo (langkah git mekanis) | `git worktree list`, `git branch`, dan `git status` bersih; `df -h` dicatat |
| W14-T12 | LEDGER-CLOSE (B-15, L-21, TD-5, DBX-8, DBX-10, DBX-25): keputusan tertulis di ledger, satu alasan per butir: gerbang versi `/*!` sengaja tidak dievaluasi (over-refusal aman, B-15); I-1 `7b44e29` tidak compile sendiri dan tidak bisa diperbaiki tanpa menulis ulang riwayat (L-21); plafon 2048 atom pemindai koma formatter diterima (TD-5, `crates/qh-sql/src/format.rs:288`); DBX-8 diadopsi (membuka W12-T7d) atau masuk PRD §10; DBX-10 diadopsi (ikut W12-T8) atau masuk PRD §10; DBX-25 tidak diadopsi dan masuk PRD §10. Keputusan DBX-8, DBX-10, dan butir bersyarat W13-T10, T11, T17, dan T18 diambil sebelum W12 dimulai (O-31b); sisanya paling lambat sebelum W14-T6. Tingkat rendah. | orkestrator solo | `target/run/ledger.md` memuat satu baris keputusan per butir; PRD §10 diperbarui bila ada yang ditolak |

**Isi laporan akhir untuk pemilik:**

1. **Status:**
   - tugas yang selesai (ID dan hash commit);
   - tugas yang terblokir (dengan bukti);
   - tugas kontingen Fase 8 yang dijalankan atau tidak.
2. **Angka per sumbu:** target, QueryHive, TablePro (atau `tidak diukur (izin OS)` dan `tidak diukur (butuh sudo)`), verdict, dan rujukan ke `docs/benchmarks.md`.
3. **Paritas visual:** daftar scene yang lulus, dan pasangan berdampingan untuk V-1…V-12 yang perlu ditinjau. Setiap rekam ulang punya commit sendiri, jadi bisa di-revert.
4. **Status UC-01…UC-18** dan bukti otomatisnya.
5. **Keputusan perencana P-01…P-31** yang bisa dibatalkan, dan keputusan blueprint yang diserahkan ke pemilik (blueprint Fase 6 §26: D-8, D-16 dan O-18, D-19, D-21, anggaran 256 MiB untuk `wide_500k`).
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
9. Push: sejak O-21 dan O-31b (2026-10-06) setiap commit yang gate-nya hijau di-push langsung ke `main` dan `work/perf-parity` (fast-forward, tanpa PR). Rilis (DMG, notarisasi, Sparkle) tetap tidak dilakukan dalam run ini.
10. **Definisi selesai tanpa utang.** Run tidak dinyatakan selesai sebelum semua syarat di `target/run/ledger.md` ("DEFINITION OF DONE FOR THE WHOLE RUN") terpenuhi dan setiap butir di `target/run/debt-register.md` berstatus tertutup dengan bukti, atau tercatat sebagai milik pemilik (daftar tunggu butir 6) dengan alasan fisiknya.

## 10. Ukuran dan titik rawan

**Jumlah tugas.** Sekitar 155: 93 implementasi, 10 blueprint, 15 ADR/dokumen, 13 pembersihan, 12 bench dan verifikasi, dan 12 final. Dari 117 semula, bertambah 27 tugas adopsi dbx, 6 tugas register utang (W8-T5, W8-T6, W14-T9 sampai W14-T12), dan 5 tugas perbaikan W8 (F0, F1, F2, F3, F5). Bila Fase 8 terpicu, ada tambahan 0–4 tugas kontingen berukuran L, ditambah W8-E2s dan W8-E4. Tugas berukuran L ada 24, dan 3 lagi bersyarat:

- W3-T1, W3-T2, W4-T3, W5-T1, W5-T2, W6-T1;
- W9-T3, W9-T8;
- W10-T1;
- W11-T1, W11-T2, W11-T3;
- W12-T3, W12-T4;
- W13-T8a, W13-T8b;
- W1-T10, dihitung L karena lama berjalan.- W1-T10, dihitung L karena lama berjalan;
- dari adopsi dbx: W10-T3 (naik dari M), W11-T8, W12-T7a, W12-T7b, W12-T7c, W12-T7e, W12-T8;
- bersyarat, L bila diadopsi: W13-T10, W13-T17, W13-T18.

**Paling mungkin meledak, berurutan:**

1. **W5-T1, grid.** Daftar paritasnya paling luas, dan fokus, IME, serta AX bertemu di sini. Karena itu dipecah menjadi dua commit (5a seam, 5b tabel).
2. **W6-T1, integrasi data plane.** Penghapusan besar di Swift dan umur handle per tab. Tes diferensial harus lulus sebelum kode Swift dihapus.
3. **W3-T1, `EngineHost`.** Kebocoran state yang diam, pool bersama tunnel, dan reconnect.
4. **W3-T0, W3-T2, dan W4-T2, editor 4B.** Batas Safe Mode (`scan.rs`), C di dalam proses, pemetaan UTF-16, dan biaya gambar baris berwarna.
5. **W9-T8, shell native.** Mengubah struktur jendela, dan harness snapshot harus tetap bisa menangkapnya.
6. **W12-T3 dan W12-T4, skrip dan result set.** Protokol event baru ditambah UX baru.
7. **W11-T2 dan W11-T3, SSH.** Kepercayaan dan Keychain. Pengujian ujung ke ujung bergantung pada `qh-sshd-dev`.
8. **W13-T7, lokalisasi.** Resource bundel SwiftPM di app yang dirakit `build.sh` belum pernah dicoba di repo ini.
9. **W1-T9, build TablePro.** Butuh jaringan dan XcodeGen. Dilewati di run ini (disk 30 GiB).
10. **W1-T6 dan W1-T10, harness black-box.** Terhalang izin OS. Hasilnya tercatat dan tidak menggagalkan gate.
11. **W13-T8a, build DataFusion.** Sekitar 2,3 GB `target` rilis, dan debug dikurangi dengan `line-tables-only`. Periksa ruang disk sebelum mulai.
12. **W12-T7a sampai T7e dan W12-T8, impor dan ekspor.** Kehilangan data diam-diam (tanggal, NaN, baris pendek, angka 16 digit); tingkat Tinggi dan A/B di T7c. Karena itu dipecah lima commit dan dijaga tes live ketiga kebijakan.

## 11. Yang masih terbuka

1. **Tindakan pemilik setelah run, tidak memblokir:** izin OS dan koneksi TablePro untuk head-to-head, `sudo purge` untuk start dingin, smoke manual (VoiceOver, IME, notifikasi, drag and drop), dan tinjauan paritas V-1…V-12.
2. **Dukungan `EXPLAIN ANALYZE (FORMAT JSON)` di Trino 483.** Diverifikasi di W13-A1, dan fallback-nya (teks) sudah ditetapkan. Ini bukan pertanyaan untuk pemilik.
3. **Plafon kawat VM (gvproxy).** Menentukan apakah sumbu 2 dinilai dengan 575k atau dengan 80% plafon. Aturannya sudah ada di `performance-plan.md` §2. Yang belum ada hanya angkanya, yang baru diketahui di W1-T10.

4. **Ukuran unduhan helper terkompresi** (W13-T8a), throughput pipa (W13-T8b, target ≥ 1 GB/s), dan ambang NFR-P3 untuk SQL (gate W13).
5. **Sort natural rayon dengan `view_pool` 4 thread** (W5-T2, `bench_ffi view-*`).
6. **Perilaku profil `sandbox-exec` di macOS 26**, dibuktikan `confinement.rs` (W13-T8a). Jalur App Sandbox menunggu sertifikat Developer ID.
7. ~~**Dari W2-A3, untuk W6-A1:** bentuk pintu darurat `Json`, nasib `ArrayRows`, `PAGE_ROWS`/`COL_BLOCK` dari angka W5-T3, dan mitigasi R-19 bila batas fd terukur.~~ **Terjawab di W6-A1 (2026-10-06), blueprint Fase 6 §26 dan ADR-0030:** pintu darurat `Json` = `swiftRenderedFormats`, awalnya `{.json}` (D-24); `ArrayRows` dan implementasi sort, filter, search Swift pindah ke target tes sebagai kembaran acuan (D-25); `PAGE_ROWS` 64 dan `COL_BLOCK` 32 sementara, dan W5-T3 mengonfirmasi atau menggantinya (D-22); mitigasi R-19 = `RLIMIT_NOFILE` lunak naik ke min(batas keras, 4.096) (D-26). Yang masih menunggu hanya angka halaman dari W5-T3.
8. **W13-T8a–b tetap di lingkup program ini** (O-15). Bila tugas itu dipotong, orkestrator melaporkannya ke pemilik sebagai penyimpangan dari O-15, bukan diam-diam.

Tidak ada pertanyaan produk yang tersisa. Semua keputusan yang diperlukan tercatat sebagai O-* dan P-* di PRD §11.
