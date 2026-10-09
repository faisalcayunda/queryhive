# Serah terima: melanjutkan run perf-parity

Dokumen ini untuk AI atau orang yang melanjutkan program perf-parity. Isinya metode kerja, bukan status. Status terbaru ada di `target/run/ledger.md`, bagian "PAUSE POINT 6 (2026-10-08)" dan "LANJUTAN (2026-10-09)". Tidak ada alat tertentu yang diasumsikan: semua langkah bisa dikerjakan dengan shell, git, dan editor biasa.

Bacaan wajib sebelum mulai, berurutan:

1. `AGENTS.md`: aturan kerja, gate, tingkat review, jebakan mesin.
2. `target/run/ledger.md`: PAUSE POINT 5, daftar keputusan pemilik O-14 sampai O-31b, backlog B-*, insiden I-*, "MENUNGGU PEMILIK", dan "DEFINITION OF DONE FOR THE WHOLE RUN".
3. `docs/architecture/development-plan.md`: §1 gate, §5 tugas per gelombang, §7 kepemilikan berkas dan urutan rantai, §9 gelombang akhir dan definisi selesai.
4. `target/run/plan-b1-onward.md` (rencana batch B1 sampai B20), `target/run/w8t2-decision.md` (keputusan performa W8), `target/run/debt-register.md` dan `target/run/dbx-adoption-backlog.md` (daftar utang dan pemilik tugasnya).

## 1. Status singkat

- `main` dan `work/perf-parity` di `origin` ada di `1708700`, dengan gate penuh hijau. Branch lokal `main` masih tertinggal di `a8c60d1`; fast-forward dulu.
- Delapan branch `lane/*` berisi kerja yang sudah dikomit tetapi belum diintegrasikan (plus `lane/w7-t1`, dipertahankan karena sudah di-merge ke `lane/w8-f2`). Tidak ada worktree lane yang tersisa.
- Ledger tidak punya baris tabel untuk B4 sampai B8. Untuk tahu apa yang sudah mendarat, cocokkan ID tugas di §5 dengan `git log` (misalnya `git log --format='%h %s' ae3b278..1708700`). Jangan menebak status.

## 2. Urutan lanjut

Langkah awal setiap sesi:

1. `git status`, `git branch -a`, `git worktree list`, `df -h /System/Volumes/Data`.
2. Nyalakan database dev (bagian 4) bila akan menjalankan tes live atau golden.
3. Fast-forward `main` lokal ke `origin/main`.

**Integrasi delapan branch: SUDAH SELESAI (per 2026-10-09).** Kedelapan lane (`w13-t14`, `w12-t11`, `w13-t16`, `w10-t7b`, `w10-t3`, `w12-t7a`, `w12-t9`, `w8-f2`) kini ada di `main` lewat commit B9/B10/B11 dan seterusnya, masing-masing dengan gate-nya (lihat `target/run/keep/ledger.md`, PAUSE POINT 6 dan LANJUTAN). Branch `lane/*` lokal sudah dihapus setelah isinya diverifikasi ada di main; salinannya tetap ada di `origin/lane/*`. Jangan ulangi integrasi ini.

Yang masih terbuka (lihat §5 `development-plan.md` dan rantai §7):
- Rantai FFI (satu pemilik `app/Generated/` dan `uniffi_api.rs` pada satu waktu): W11-T6b, W12-T3, W13-T4, W13-T8b, lalu W13-T9 dan W13-T13.
- Sisa W12/W13 yang belum punya commit: W12-T7b/c/e, W13-T5/T6/T7/T12/T15/T17/T18, W13-D, W13-C.
- **Seluruh W14 belum dimulai** (lihat §9).
- Keputusan bersyarat (W12-T7d, W13-T10, W13-T11, W13-T17, W13-T18, DBX-8, DBX-10, DBX-25) dicatat sebagai O-* di ledger, paling lambat seperti tertulis di W14-T12.

## 3. Metode lajur (keputusan O-22)

Tiga lajur implementer paling banyak berjalan bersamaan. Aturannya:

- **MAIN** (checkout utama): memegang rantai FFI dan menjalankan gate integrasi: `build-ffi`, G-FFI, G-GOLDEN, G-APP, G-LEAK, `cargo test --workspace`, dan slot eksklusif benchmark.
- **Lajur Swift:** git worktree `../query_hive-wtsN`. Menyalin `target/ffi` dari MAIN (ulangi setiap `app/Generated/` berubah) dan tidak pernah menjalankan `cargo` atau `build-ffi.sh`, karena skrip itu dan `app/Package.swift` mengharapkan `<root>/target`. Menjalankan `swift build` dan `swift test` saja.
- **Lajur Rust:** git worktree `../query_hive-wtrN`, masing-masing dengan `CARGO_TARGET_DIR` sendiri di SSD eksternal: `/Volumes/Faisal SSD/qh-cargo-targets/t1`, `t2`, `t3` (path memuat spasi, beri kutip). SSD itu kini **APFS** (diformat ulang 2026-10-09; sebelumnya exFAT, yang lambat untuk ribuan berkas kecil dan pernah menahan lajur cargo dengan I/O tak terputus). APFS kini juga cocok untuk SwiftPM `.build`, tetapi tetap taruh `.build` di disk internal kecuali disk internal sempit. Jangan menyentuh apa pun lain di SSD itu: isinya berkas pribadi pemilik. Lajur Rust menjalankan `cargo test -p <crate yang disentuh>`, tidak pernah `--workspace` (tiap path worktree membangun ulang semua binary tes, sekitar 5 GB per lajur).
- Jangan menjalankan lebih dari dua lajur cargo di SSD sekaligus (lihat insiden di bagian 7).
- **Integrasi:** komit di lajur, cherry-pick ke MAIN, jalankan gate tugas itu di MAIN, lalu push fast-forward ke `main` dan `work/perf-parity` (izin tetap O-21 dan O-31b, hanya untuk program ini). Satu komit per tugas yang lulus gate. Sebelum push, baca `gh run list --branch main -L2`; main yang merah dicatat sebagai insiden dan memblokir push berikutnya (DBX-43). Bila `cherry-pick` bentrok di `app/Generated/`, jalankan ulang `./app/build-ffi.sh` di kepala branch dan komit hasilnya bersama tugas itu.
- Baseline PNG hanya direkam ulang untuk perubahan V-n yang terdaftar, satu per satu, dalam komit sendiri bersubjek `test(visual): re-record <scene> for <V-n>`.
- Nama branch lane tidak dipakai ulang. Worktree dihapus hanya setelah komit dan `git status` bersih, tidak pernah dengan `--force` (insiden I-2: worktree yang dihapus paksa menghilangkan kerja yang belum dikomit).
- Benchmark dan slot eksklusif (X/Q): tidak ada lajur, build, atau reviewer lain yang berjalan.

## 4. Database dev dan kunci live

- Container podman, loopback saja: PostgreSQL 55432, MySQL 53306, Trino 58080, SSH 52222, toxiproxy (+30 ms) 55435.
- Setelah reboot: `podman start qh-postgres qh-mysql qh-trino`, lalu `deploy/dev/up.sh toxiproxy`, lalu `deploy/dev/qh-sshd-run.sh`. Konfigurasi toxiproxy hidup di memori, jadi `up.sh toxiproxy` harus diulang setiap restart.
- Database dev dipakai bersama, jadi hanya satu lajur sekaligus yang boleh menjalankan tes live atau golden. Kuncinya direktori: `mkdir target/run/live.lock` sebelum tes live apa pun (gagal bila sudah ada, artinya lajur lain memegangnya), `rmdir target/run/live.lock` sesudahnya, termasuk saat tes gagal.
- Lajur tidak boleh menjalankan `deploy/dev/qh-sshd-run.sh`; itu urusan MAIN.
- Jangan menyentuh volume podman milik pemilik (`ask-ai`, `chat-ai`) atau container lain di luar `qh-*`.

## 5. Gate

Perintah lengkap dan kapan dipakai ada di `development-plan.md` §1 dan `AGENTS.md`. Ringkasnya:

| Gate | Perintah |
|---|---|
| G-RUST | `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` |
| G-DENY | `cargo deny check licenses` (sejak W13-T8a juga untuk `helpers/analytics`) |
| G-FFI | `./app/build-ffi.sh`, komit `app/Generated/`, lalu `cd app && swift test --filter RustEngineTests` |
| G-SWIFT | `cd app && swift build && swift test` |
| G-VIS | `cd app && swift test --filter VisualParityTests` |
| G-GOLDEN | `cargo build --bin queryhive-engine && python3 tools/golden/live_cases.py` |
| G-LIVE | tes live crate yang disentuh, dengan penjaga `QH_TEST_*` (daftar di §1 plan; sertakan `-p qh-driver-postgres`, bukan hanya `real_server`) |
| G-APP | `./app/build.sh`, lalu `app/dist/QueryHive.app/Contents/MacOS/QueryHive --snapshot <png> --scene done` |
| G-BENCHQ | subset cepat `ttfr-pg,scroll-1m,type-10k`, n = 5, di slot eksklusif (gate W9 sampai W13) |
| G-LEAK, G-ANALYTICS | lihat §1 plan; G-ANALYTICS hanya untuk W13 dan W14 |

Harapan saat ini di `1708700`:

- `cargo test --workspace`: 1547 lulus, 0 gagal, 2 ignored. Run pertama pernah kehilangan 11 binary tes; run ulang hijau.
- G-SWIFT: 1210 tes, 0 gagal, 8 skip.
- G-VIS: 18/18. Gambar parity dirender pada skala 2x tetap, apa pun layar utamanya.
- G-GOLDEN: 20/31, dengan 11 selisih yang sudah terklasifikasi D-1 sampai D-11 di `docs/golden-deltas.md`. Skrip keluar dengan kode non-nol bila ada selisih, memang disengaja. Selisih baru berarti regresi, bukan alasan menambah klasifikasi.
- G-FFI: `app/Generated/` tanpa diff setelah `build-ffi.sh`.
- Belum pernah jalan: tes Keychain nyata (`QH_TEST_KEYCHAIN=1`).
- Kalau permukaan UniFFI berubah, jalankan `./app/build-ffi.sh` dan komit `app/Generated/` bersama perubahan Rust. Komit yang tidak menyertakan Swift tidak bisa dinyatakan hijau hanya dari gate cargo (insiden I-1).

## 6. Kebijakan review

- Keputusan O-20: implementasi, dokumen, bench, dan pembersihan dikerjakan oleh model yang lebih murah. Hanya review yang memakai model terkuat yang tersedia, satu putaran. Setelah temuan blocking: perbaiki, verifikasi dengan gate, komit, catat "pending review" di ledger.
- Tingkat risiko (`AGENTS.md`): tinggi (kredensial, kepercayaan SSH, enkripsi spill, klasifikasi Safe Mode di `crates/qh-sql`, FFI, jalur yang bisa menghilangkan teks atau data pengguna, host engine, penulisan ulang grid dan data plane) dapat satu review model terkuat ditambah reviewer keamanan atau database bila relevan. Sedang (fitur UI, SQL buatan app): satu reviewer. Rendah (dokumen, ADR, tooling bench, hanya tes, pemindahan kode, pembersihan): gate ditambah satu review ringan.
- Maksimal dua putaran per tugas, satu reviewer per putaran. Hanya temuan blocking yang membuka putaran kedua; sisanya masuk backlog. Setelah putaran kedua, verifikasi perbaikan lewat tes dan gate dan catat sebagai pending review, jangan mulai putaran ketiga.
- Selesai berarti: berkas sesuai daftar kepemilikan tugas, semua gate yang disebut tugas hijau, tingkat review terpenuhi, dan sudah dikomit di `work/perf-parity`. Laporkan yang tertunda terpisah dari yang selesai.
- Setiap brief untuk pembantu (manusia atau AI) diakhiri permintaan: kembalikan kesimpulan dan bukti (perintah, hasil, hitungan, daftar berkas), bukan isi berkas. Pembantu hanya-baca tidak bisa menulis hasil kerja; bila memakai pembantu seperti itu, minta ia mengembalikan teks dan tulis sendiri.

## 7. Jebakan yang sudah terjadi

- **Keluaran terpotong.** Di mesin pemilik ada hook shell yang menulis ulang `head`, `grep`, `find`, `wc` dan memotong keluarannya. Keluaran yang diawali `// ...` sudah terpotong. Untuk hitungan dan isi yang pasti, arahkan ke berkas log lalu baca dengan `python3` atau `/usr/bin/<perintah>`.
- **Sidik jari target bersama basi.** Target cargo yang dipakai beberapa path worktree menanam path lama ke binary tes. Hapus `.fingerprint/<crate>-*` yang bersangkutan. Itu juga penyebab dugaan tes qh-result-store yang gagal di lane w7-t5.
- **Checksum UniFFI tidak cocok** setelah komentar dokumentasi di sumber FFI berubah: jalankan ulang `./app/build-ffi.sh`.
- **Tabel tes live yang bocor merusak golden.** Pola yang pernah bocor: `qh_w7t2_ro_*`, `qh_json_*`, `qh_parent_*` dan `qh_child_*`. Hapus tabel sisa sebelum G-GOLDEN dan pastikan tes menjatuhkan tabelnya sendiri, juga saat gagal.
- **Jangan menulis ke `~/Library/Application Support/QueryHive`** dari tes atau bench. Pakai `TestIsolation` dan `DB_PATH` sementara. Jalur golden dan `bench_fetch.py` pernah menambah 20 baris ke `execution_log` milik pemilik (I-4); sudah diperbaiki.
- **Aplikasi dialamatkan lewat path, bukan bundle id atau nama.** `/Applications/QueryHive.app` dan `/Applications/TablePro.app` milik pemilik dan memakai bundle id yang sama dengan build dev. Ukur lewat `app/dist/QueryHive.app/Contents/MacOS/QueryHive --bench <skenario>` dengan data terisolasi. Jangan memakai `xctrace --launch` (I-2). `tools/bench/qhbench` dijalankan pemilik.
- **Disk sempit.** Saat jeda sekitar 9 GiB bebas di disk internal (diukur 29 GiB saat dokumen ini ditulis, tapi jangan mengandalkannya); jaga >= 7 GiB. Di antara batch, bersihkan penuh: `cargo clean --profile dev`, kosongkan target lajur di SSD, hapus `app/.build` di worktree Swift, dan hapus `target/debug/incremental` serta executable tes lama di `target/debug/deps` saat tidak ada build berjalan. Cek `df` setelah setiap tugas.
- **Lajur SSD macet.** Saat B8-extra beberapa proses cargo/rustc dengan target SSD tertahan di status I/O tak terputus sampai sekitar 59 menit. Cek `ps -axo stat` untuk huruf `U`, dan batasi dua lajur cargo di SSD.
- **Wiring Swift jangan dilewati.** Tugas Rust yang menambah opsi pengguna belum selesai sebelum UI dan tes Swift-nya ada.
- **Jangan ubah worktree milik orang lain**, misalnya worktree opencode `main-settings` yang terdaftar di `.git/worktrees`. Laporkan, jangan hapus (L-48).

## 8. Definisi selesai

Run selesai hanya bila semua syarat ini terpenuhi (sumber: ledger "DEFINITION OF DONE FOR THE WHOLE RUN" dan `development-plan.md` §9 butir 10):

1. Semua tugas W0 sampai W14 di §5, termasuk tugas adopsi dbx (DBX-n), sudah dikomit dan ada di `origin/main`.
2. Backlog B-* kosong: tiap butir diperbaiki, atau ditutup dengan alasan tertulis.
3. Tidak ada butir "pending review", tidak ada komit WIP, tidak ada branch `lane/*` atau worktree tambahan, tidak ada perubahan yang belum dikomit.
4. G-HEAVY penuh (termasuk G-ANALYTICS) hijau di `main`, dan bench final W14 tercatat.
5. Dokumen sinkron dengan kode: ADR, `PROGRESS.md`, `app/DESIGN.md`, `AGENTS.md` (bagian resume), `docs/golden-deltas.md`.
6. Temuan keamanan S-1 (cancel PG lewat TLS dan batas waktu) dan S-2 (daftar tolak fungsi berefek samping di classifier) selesai dengan tes dan review keamanan.
7. Sisa yang boleh ada hanya yang secara fisik butuh pemilik, ditulis eksplisit: izin Screen Recording (P-1, W6-T1 6c), cek manual UI, pengiriman notifikasi nyata, bench 120 Hz, `qhbench` head-to-head melawan TablePro.

Aturan pemilik: jangan menyisakan utang. `target/run/debt-register.md` memuat 130 butir, tiap butir punya tugas pemilik (peta di `development-plan.md` §5, bagian "Register utang"); `target/run/dbx-adoption-backlog.md` memuat DBX-1 sampai DBX-78 dan PF-1 sampai PF-27. Register diaudit di `da36406`, jadi status butir sudah bergeser; cek terhadap `git log` sebelum menganggap sesuatu masih terbuka. Laporkan yang tertunda terpisah dari yang selesai, dengan hasil CI remote dipisah dari gate lokal.

## 9. Bahasa dan git

- Kode, komentar, pesan log, dan pesan komit dalam bahasa Inggris. Dokumen di `docs/` dan ADR dalam bahasa Indonesia, mengikuti berkas yang sudah ada.
- Komit memakai Conventional Commits berbahasa Inggris, gaya repo (huruf kecil, kalimat), mis. `perf(engine): one runtime per process, and Stop reaches the server`. Penulis: Faisal Nugraha Cayunda, tanpa trailer co-author dan tanpa footer sesi.
- Satu komit per tugas yang lulus gate. TablePro (AGPL-3.0) dan dbx (Apache-2.0) hanya bahan studi: ambil ide dan angka, jangan salin kode, aset, atau string. QueryHive (MIT) hanya berisi kode, aset, dan string miliknya sendiri.
- Berkas sementara ke `target/run/` (di-gitignore, bertahan lewat reboot).
- Izin komit dan push tanpa tanya berlaku hanya untuk program ini (O-21, O-31b). Di luar program ini, tanya pemilik dulu untuk setiap tindakan git yang menulis.
