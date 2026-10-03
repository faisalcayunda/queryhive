# 0033 — Analisis editor dengan tree-sitter per statement, di Rust

- **Status:** Diterima. Inti Rust (`qh-sql` `walk` dan `lex`, `qh-sql-grammar`, `qh-editor`), permukaan FFI,
  dan penerapan di editor sudah mendarat. Yang masih menunggu pekerjaan lain dicatat di bagian
  Konsekuensi, bukan disembunyikan.
- **Tanggal:** 2 Okt 2026 (W4-D, perf-parity Fase 4B)
- **Konteks instruksi:** `docs/architecture/development-plan.md` §4 (jadwal ADR, baris 0033) dan W4-D;
  `docs/architecture/blueprints/fase-4b-editor-analysis.md` beserta verdict architect-reviewer;
  `docs/architecture/performance-plan.md` §8 dan §13; keputusan pemilik O-14 di
  `docs/architecture/prd-performance-and-parity.md` (§11.1), V-12 (§6.5), NFR-L, NFR-A1, NFR-P5, P-10.
- **Menggantikan:** penolakan tree-sitter di `performance-plan.md` §13 (baris "tree-sitter" kini "Ya (O-14)").
  Tidak menggantikan ADR lain. Pendamping: ADR-0017 dan ADR-0026 (Safe Mode memakai pemecah yang sama),
  ADR-0009 (`panic = "unwind"`, relevan untuk risiko C di bawah).
- **Catatan bahasa dan penomoran:** ADR ini memakai nomor D-1 sampai D-21 dari blueprint 4B (revisi 30 Sep 2026).
  Nomor D yang sama di blueprint lain (Fase 6) adalah keputusan yang berbeda.

## Konteks

Editor SQL berwarna lewat regex di Swift (`SQLSyntax`), melipat lewat `SQLFolding`, dan memecah statement lewat
`SQLScanner`. Pada Fase 4A pengukuran menunjukkan bahwa 73% waktu `insertText` adalah pembetulan atribut TextKit 1
yang berbanding lurus dengan jumlah run atribut di storage (blueprint 4B, bagian Sumber). Pemilik memutuskan
(O-14, 30 Sep 2026) memakai tree-sitter sebagai dasar analisis editor: kelas warna, lipatan, alias, dan
fondasi untuk diagnostik, autocomplete, dan formatter. Perubahan tampilan dibolehkan asal didaftarkan (V-12),
sehingga paritas piksel dengan pewarnaan regex tidak lagi dituntut.

Gaya hambatan yang berlaku:

1. **Satu pemecah statement.** Engine, Safe Mode (ADR-0017, ADR-0026), `script` (W12-T3), dan Run memecah dengan
   `crates/qh-sql/src/scan.rs`. Band dan run mark di editor yang memakai sumber lain mengulang bug yang ditutup
   W4-T2b (`select "a;b"` tampil sebagai satu statement sementara Run mengirim setengahnya).
2. **NFR-P5.** Ketikan harus tetap di bawah 4 ms p99 di dokumen 10k baris, termasuk giliran `apply` yang menggambar
   ulang baris dengan warna baru.
3. **NFR-L.** Tidak ada paket SwiftPM baru yang direncanakan; dependensi Rust harus lolos `cargo deny check licenses`
   dan yang di luar jangkauannya (C yang di-vendor) harus tercatat.
4. **MSRV workspace 1.85** (`Cargo.toml`, `[workspace.package] rust-version`).
5. **Grammar tidak mengenal sintaks yang dipakai app.** `:name` (parameter), dan sebagian besar sintaks MySQL dan
   Trino berujung ERROR pada pengujian dialek di blueprint §1.2.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Letak parser | **Rust, di crate `qh-editor`** | Konsumen pohon ada di Rust (pemecah, formatter, alias). Satu parse melayani semuanya. Tes properti tanpa AppKit | Satu cermin teks di Rust dan hasil yang menyeberang FFI (kecil, karena hanya selisih) |
| | SwiftTreeSitter | Dekat dengan `NSTextView` | Paket SwiftPM pertama (NFR-L), grammar C dibangun dua kali, parse kedua untuk formatter |
| | Modul `qh-sql/src/editor/` | Tanpa crate baru | `qh-sql` dipakai tiga driver dan `qh-import`; mereka tidak boleh ikut mengompilasi C 17 MB. Feature flag di crate yang dipakai lima crate lain rawan penyatuan fitur |
| Unit parse | **Satu pohon per statement `scan.rs`** | Reparse 0,02 sampai 0,09 ms per ketikan berapa pun ukuran dokumen. ERROR terkurung. Cache pohon bisa dibatasi | Statement raksasa butuh jalan lain (D-9) |
| | Satu pohon per dokumen | Struktur global | Reparse 0,85 sampai 5,3 ms p50 pada dokumen 0,4 sampai 2 MB, warna pertama dokumen 2M butuh 130 sampai 770 ms (blueprint §1.3) |
| Sumber grammar | **`tree-sitter-sequel` 0.3.11 di-vendor, scanner ditambal** | Menghentikan bocor 74 B per edit. Disematkan persis seperti yang diukur | Berkas C 17 MB di repo (sekitar 818 KB terkompresi); pembaruan jadi tugas dengan gate |
| | Crate crates.io apa adanya | Tanpa vendoring | Scanner bocor; `main` upstream tiga kali lebih besar dengan dua uji tambahan |
| | `gmr/tree-sitter-postgres` | Akurat untuk PostgreSQL | Hanya PostgreSQL (ditolak, blueprint §1.2) |
| Penerapan warna | **Atribut sementara `NSLayoutManager` (`.foregroundColor`)** | `insertText` 3,7 ke 0,5 ms, layout terlihat 3,5 ke 0,04 ms di 195k, piksel identik (blueprint §1.6) | Pemilik kunci harus diatur (D-17); `apply` harus menghindari invalidasi yang tidak perlu |
| | Atribut storage per run | Sederhana | Jalan yang membuat `insertText` lambat |
| | CodeEditTextView | Editor siap pakai | Paket SwiftPM pertama dengan dependensi transitif (TextStory BSD-3-Clause, swift-collections Apache-2.0, plugin SwiftLint), dan belum terbukti lebih murah. Hanya lewat spike terukur dan keputusan pemilik (blueprint §7.7) |
| Batas statement | **`scan.rs`, bukan pohon** | Sama dengan engine dan Safe Mode. Menutup kasus yang tidak parse | Pohon hanya mengisi isi statement |
| | Anak teratas pohon | Satu sumber | `ANALYZE t;` menjadi ERROR yang menelan `;` (pohon 8 statement, `scan()` 9 di seed PostgreSQL); `BEGIN; …; COMMIT` menjadi satu node `transaction` |

## Keputusan

**Analisis editor dilakukan di Rust, di crate `qh-editor`, dengan tree-sitter yang menguraikan satu pohon per statement
yang dipecah `qh_sql::walk`. Grammar SQL di-vendor ke `crates/qh-sql-grammar` dengan satu scanner yang ditambal. Warna
tiba di Swift sebagai paket per-revisi dan diterapkan sebagai atribut sementara `.foregroundColor`.**

Dua puluh satu keputusan rancangan (nomor sama dengan blueprint 4B §2, dengan status sesudah dikirim):

| # | Keputusan | Status sebagai dikirim |
|---|---|---|
| D-1 | Grammar `tree-sitter-sequel` 0.3.11 di-vendor ke `crates/qh-sql-grammar`; `parser.c` apa adanya, `scanner.c` ditambal | Dikirim (commit `c96c6d6`). Lihat bagian grammar |
| D-2 | `tree-sitter = "=0.26.13"` dan `tree-sitter-language = "=0.1.7"`, dipatok di `[workspace.dependencies]` | Dikirim. Lihat bagian MSRV |
| D-3 | Di Rust, bukan Swift | Dikirim. Tidak ada paket SwiftPM baru |
| D-4 | Crate baru `crates/qh-editor`, bukan modul `qh-sql` | Dikirim (commit `551de40`). `qh-editor` memakai `#![forbid(unsafe_code)]`; hanya `qh-ffi` yang bergantung padanya (`crates/qh-ffi/Cargo.toml`) |
| D-5 | Satu pohon per statement `scan.rs` | Dikirim (`crates/qh-editor/src/statements.rs`, `syntax.rs`) |
| D-6 | Klasifikasi lewat jalan pohon dan tabel jenis node, bukan `highlights.scm` | Dikirim (`crates/qh-editor/src/classify.rs`) |
| D-7 | Celah ERROR dan statement besar diwarnai `qh_sql::lex` | Dikirim. Warna adalah fungsi teks statement saja (doc modul `lib.rs`) |
| D-8 | View parse `:name`: `:` awal parameter diganti `_` dengan panjang sama; kelas Parameter dari lapis leksikal | Dikirim (`crates/qh-editor/src/view.rs`: `:` yang didahului `[A-Za-z0-9_:]` tidak diganti, sehingga `a::text`, `x:a`, `arr[lo:hi]`, `:=` utuh). Edit diperlebar satu byte tiap sisi (`syntax.rs::input_edit`) |
| D-9 | Statement > 256 KiB tanpa pohon dan tanpa cache token; cache pohon sekitar 640 KiB sumber per dokumen, LRU | Dikirim: `GIANT_STATEMENT_BYTES = 256 * 1024` dan `TREE_CACHE_BYTES = 640 * 1024` di `crates/qh-editor/src/lib.rs`, dipertahankan oleh bench W3-T2. Plafon dokumen `CEILING_UTF16 = 2_000_000` |
| D-10 | Warna lewat atribut sementara; storage hanya atribut dasar dan italic komentar | Dikirim (commit `3f3fcd3`; `EditorAnalysis.apply`, `SQLEditor.swift`) |
| D-11 | Himpunan kotor dan selisih token per statement menggantikan `changed_ranges` | Dikirim (`paint.rs`) |
| D-12 | Diagnostik: leksikal tersedia, ERROR/MISSING dihitung tetapi tidak ditampilkan secara default | Sebagian: data ada, penggambaran garis bawah belum (W10-T6). Lihat bagian kebijakan diagnostik |
| D-13 | Formatter W12-T1 mengambil token dari `scan.rs` dan `qh_sql::lex`, bukan pohon | Belum dibangun (W12-T1). Kontraknya tetap |
| D-14 | Alias W11-T6 dari pohon statement, cadangan leksikal di statement ber-ERROR | Belum dibangun (W11-T6) |
| D-15 | Lipatan dari node: statement, `cte`, `subquery`, badan `$tag$`, hanya yang lebih dari satu baris | Dikirim (`crates/qh-editor/src/folds.rs`; `FoldKind { Statement, Cte, Subquery, Body }` di FFI) |
| D-16 | Penjaga IME di sisi penerapan | Dikirim: edit diteruskan ke analisis selama komposisi, `apply` dan paint sinkron menunggu `hasMarkedText()` (`SQLEditor.swift`, `EditorAnalysis.shouldApply`). Klaim blueprint bahwa "kuncinya berbeda" untuk komposisi tetap **belum diuji** |
| D-17 | Dua kunci, main tidak pernah memegang kunci analisis; QoS dari antrean Swift | Dikirim, dengan satu penyimpangan: lihat Konsekuensi |
| D-18 | Hasil berupa `uniffi::Record` dengan larik `u32` datar | Dikirim (`crates/qh-ffi/src/editor.rs`: `EditorPaint`, `EditorOutline`, `EditorFold`, `EditorIssue`) |
| D-19 | W4-T2 dua commit (A: FFI tanpa perubahan perilaku, B: jalur baru, regex dihapus, V-12 direkam ulang) | Dikirim sebagai `4cdd377` (A) dan `3f3fcd3` (B); W4-T2b menyusul sebagai `c5e0213`. Lihat penyimpangan urutan di bawah |
| D-20 | API punya parameter dialek; W4-T2 mengirim `.generic` | Dikirim: `EditorDialect { Generic, Postgres, Mysql, Trino }` dan `sql_statement_ranges(sql, dialect)`; app selalu `.generic` (`EditorAnalysis.swift`, `QueryTab.swift`) |
| D-21 | `qh-sql` tidak tahu daftar kata kunci editor | Dikirim: `qh_sql::lex` mengeluarkan `Word` tanpa kelas, himpunan kata kunci ada di `crates/qh-editor/src/keywords.rs` (`EXTRA_KEYWORDS`) |

### Grammar yang di-vendor dan patch-nya

Sumber lengkap ada di `crates/qh-sql-grammar/PROVENANCE.md`; ringkasannya:

- **Asal:** DerekStride/tree-sitter-sql, terbit sebagai crate `tree-sitter-sequel` 0.3.11 (1 Okt 2025), lisensi
  MIT ("Copyright (c) 2021 Derek Stride"). Checksum `.crate` sha256 `9d198ad3…ec7d17`. Berkas `LICENSE` diambil dari
  repositori upstream karena `.crate` tidak membawanya.
- **Isi crate:** `vendor/parser.c`, `vendor/scanner.c`, `vendor/tree_sitter/{alloc,array,parser}.h`, `grammar.js`,
  `node-types.json`, `LICENSE`. Semua disalin byte demi byte kecuali `scanner.c`. Build memakai `parser.c`, tidak
  menjalankan `grammar.js` (`build.rs`: `cc::Build` dengan `std("c11")`).
- **Patch (`vendor/scanner.patch`):** bagian `src/scanner.c` dari PR upstream #361 (head `77e33d3d…`, digabung
  `97614d05…` pada 19 Sep 2026, belum dirilis): (1) `serialize` tidak lagi membebaskan `start_tag`; (2) `deserialize`
  membebaskan `start_tag` lama sebelum menimpa, sehingga tidak ada lagi bocor sekitar 74 B per edit pada badan
  `$body$ … $body$`; (3) `deserialize` keluar bila `malloc` gagal.
- **Hunk keempat, lokal dan bukan dari upstream:** pada cabang `DOLLAR_QUOTED_STRING` di `scan`, `return false` dini
  ketika tag baru sama dengan `state->start_tag` tidak membebaskan buffer 1024 byte. Kini `free(start_tag);` mendahuluinya.
  Ditemukan oleh tinjauan keamanan (commit `c96c6d6`).
- **Penjaga:** `crates/qh-sql-grammar/tests/scanner_leak.rs` (G6): 10.000 edit inkremental di dalam badan `$body$` tidak
  boleh menumbuhkan heap lebih dari 128 KiB, dan dua masukan hunk lokal diparse ulang 1.000 kali. PROVENANCE mencatat
  bahwa scanner tanpa patch menumbuhkan heap 750.080 byte dan gagal. Pengukuran memakai penghitung zona malloc sistem
  karena scanner memanggil `malloc` libc dan `tree_sitter::set_allocator` tidak melihatnya.
- **Tidak ada fork grammar** untuk sintaks yang tidak dikenal (`:name`, Trino): ditangani `qh_sql::lex` dan view `:name`.
- **Pembaruan** adalah tugas sendiri dengan gate: probe dialek (blueprint §1.2), diferensial inkremental (§1.5), bench
  (§1.3), ukuran biner (§1.7); lalu `cargo test -p qh-sql-grammar` dan tes golden `qh-editor`.
- **Lisensi dan pemberitahuan:** C yang di-vendor di luar jangkauan `cargo deny`, karena itu dicatat di PROVENANCE
  dan `THIRD-PARTY-NOTICES.md` (runtime tree-sitter MIT, header UTF yang dibundel runtime di bawah lisensi Unicode,
  dan grammar DerekStride MIT). Pemberitahuan seluruh crate lain dan pemasangannya ke bundle adalah tugas W14.
- **Biaya build:** `[profile.dev.package.qh-sql-grammar]` dan `[profile.dev.package.tree-sitter]` diberi
  `opt-level = 2`, karena `cc` membaca `OPT_LEVEL` per paket dan tanpa itu tes debug menjalankan parser -O0
  (`Cargo.toml`).

### Kebijakan diagnostik (D-12, FR-ED-06)

Yang dipakai dan yang tidak:

1. **Masalah leksikal ditampilkan** (data `IssueKind::UnclosedQuote`, `UnclosedIdentifier`, `UnclosedComment`,
   `UnclosedDollar`, `UnbalancedParen`): berasal dari pembacaan `scan.rs` sendiri, jadi benar apa pun yang grammar tahu
   (`crates/qh-editor/src/issues.rs`).
2. **ERROR dan MISSING dari pohon (`SyntaxError`, `MissingToken`) dihitung dan diekspor, tetapi tidak digambar secara
   default.** Alasannya terukur pada korpus uji dialek di blueprint §0.2 butir 2: statement **valid** yang pohonnya
   memuat ERROR atau MISSING sebanyak 3 dari 21 (14%) untuk PostgreSQL, 9 dari 19 (47%) untuk MySQL, dan 8 dari 17 (47%)
   untuk Trino. Contoh: `SHOW CATALOGS`, `USE tpch.sf1`, `DESCRIBE t`, `FETCH FIRST 5 ROWS ONLY`,
   `TABLESAMPLE BERNOULLI (10)`, `GROUPING SETS`, `LIMIT 10, 20`, `UPDATE … ORDER BY … LIMIT`. Garis bawah di bawah SQL
   yang benar lebih merugikan daripada tidak ada garis bawah.
3. **Posisi galat dari server** adalah sumber garis bawah yang dapat dipercaya; penggambarannya, dan sambungan dialek
   per tab ke `EditorDocument::new`, milik W10-T6 dan tidak ada di commit yang menutup ADR ini.
4. **Sebagai dikirim:** `EditorIssueKind` di FFI memuat ketujuh jenis, `EditorAnalysis.swift` meneruskan semuanya, dan
   `SQLEditor.swift` hanya memakai daftar masalah untuk rotor ("Query issues"). Tidak ada kode yang menggambar garis
   bawah dari jenis mana pun. Pernyataan "tidak ditampilkan secara default" karena itu benar sekarang.
5. **Warna tidak bergantung pada ERROR.** Anak ERROR diwarnai pohon sedangkan celahnya diwarnai lexer; konvergensi
   (`Analyzer::converge`, dipanggil dari `EditorDocument::converge` dan dijadwalkan sesudah jeda 0,5 detik di antrean
   idle) mengecat ulang selisih token sehingga warna akhir selalu fungsi dari teks.

### Pemilik kunci atribut sementara (D-17, blueprint §7.4)

Satu kunci, satu pemilik; tidak ada fitur yang menghapus kunci milik fitur lain.

| Kunci sementara | Pemilik | Aturan |
|---|---|---|
| `.foregroundColor` | warna sintaks (4B) | Hanya lewat `addTemporaryAttribute` dan `removeTemporaryAttribute`. `setTemporaryAttributes` dilarang karena mengganti semua kunci, termasuk sorotan find (komentar di `EditorAnalysis.apply`) |
| `.backgroundColor` | find bar | `SQLEditor.swift` (`updateFindHighlight`, `clearFindHighlight`). Sorotan find tergambar di bawah glyph berwarna sintaks, tanpa urutan menang-kalah karena kuncinya berbeda |
| `.underlineStyle`, `.underlineColor` | W10-T6 (garis bawah diagnostik) | Belum dipakai. Kunci yang tidak memengaruhi layout, jadi diizinkan sebagai atribut sementara |
| (bukan atribut sementara) | W10-T7 (pasangan kurung) | Digambar di `SQLTextView.drawBackground(in:)` seperti band sorotan, supaya tidak berebut `.backgroundColor` dengan find |

Aturan penerapan yang menyertainya: storage hanya diubah saat dokumen dibuat, saat font atau lebar tab berganti, dan
untuk font italic komentar (temporary attribute tidak bisa membawa font); `apply` membandingkan dahulu dengan
`temporaryAttribute(.foregroundColor, atCharacterIndex:…)` dan melewati run yang sudah benar
(`rangeNeedsPaint` di `SQLEditor.swift`), karena setiap `add`/`remove` meng-invalidate tampilan dan satu baris berwarna
berharga 4 sampai 5 ms untuk digambar ulang (blueprint §1.6). Karakter yang baru diketik mewarisi warna tetangga untuk satu
giliran (`inheritNeighborColor`), supaya komentar atau kata tidak berkedip tanpa warna.

### V-12: perubahan tampilan yang didaftarkan

V-12 terdaftar di PRD §6.5 sebagai "Warna sintaks dan lipatan editor dari tree-sitter, dengan warna sebagai atribut
sementara", dikirim di W4-T2 commit B. Sebelas butir tercantum di blueprint §5.3; ringkasannya:

1. Kata kunci mengikuti grammar dan konteks (tipe data ungu; kata yang dilex grammar sebagai identifier berwarna dasar;
   kata di luar grammar seperti `describe`, `grant`, `fetch` tetap ungu di celah ERROR lewat daftar cadangan).
2. Deteksi fungsi mengikuti `invocation` (`INSERT INTO t (a)` tidak lagi mewarnai `t` sebagai fungsi).
3. Dollar-quote biasa menjadi string; badan fungsi diwarnai sebagai SQL dan tagnya sebagai string.
4. Komentar sesudah tanda baca (`1;-- c`, `count(*)--c`, `)/* x */`) menjadi komentar.
5. Parameter `:name`, `?`, dan `$1` memakai warna literal; `:n` di dalam `[…]` tidak berwarna parameter, `x:a` ya,
   mengikuti `SQLScanner` karena Run mengikatnya.
6. `::`, `->>`, `#>`, `@>`, dan `:=` abu-abu tanda baca.
7. Kutip atau komentar yang belum ditutup mewarnai sisa statement menurut `scan.rs`.
8. Penanda lipatan bertambah: CTE, subquery, badan `$tag$` lebih dari satu baris.
9. Warna pindah dari run storage ke run `temporary: true`.
10. `keyword` dan `punctuation` gelap dinaikkan sampai ≥ 4,5:1 di Nord (P-30). Terkunci oleh
    `app/Tests/QueryHiveTests/SyntaxPaletteTests.swift` (G8), yang menguji setiap token terhadap setiap kanvas
    dengan satu pengecualian yang disebut di berkasnya: `comment` di mode gelap sengaja tidak mencolok.
11. Aturan parameter butir 5.

Delapan scene editor direkam ulang gelap dan terang: `editor-syntax`, `-folded`, `-plain`, `-find`, `-invisibles`,
`-wrap-on`, `-wrap-off`, `-long-middle` (16 pasang; commit `3f3fcd3` mengubah 16 PNG di
`app/Tests/QueryHiveTests/__Baselines__/`). Pasangan lama dan baru ditinjau berdampingan oleh pemilik di laporan akhir (P-01).

### Risiko C di dalam proses

Parser dan scanner tree-sitter adalah C yang berjalan di proses app. **`panic = "unwind"` (ADR-0009, `Cargo.toml`)
menangkap panic Rust, bukan crash atau baca di luar batas di C.** Crash di `parser.c`, `scanner.c`, atau runtime
menjatuhkan seluruh proses app. Risiko ini **diterima secara sadar** dan dicatat di sini (PRD §13 dan
`performance-plan.md` §18 merujuk ADR ini). Penahannya:

- Runtime tree-sitter dan grammar DerekStride dipakai luas; `scanner.c` hanya sekitar 190 baris dan sudah ditinjau
  keamanan satu putaran (disetujui) bersama patch-nya, dan semua berkas vendor dicocokkan dengan upstream.
- Pengujian inkremental acak (`crates/qh-editor/tests/incremental.rs`, `golden.rs`) mencakup byte non-ASCII.
- Pohon tidak dibuat untuk statement > 256 KiB, sehingga masukan raksasa tidak pernah menyentuh parser.
- `qh-editor` memakai `#![forbid(unsafe_code)]`; satu-satunya `unsafe` di sisi Rust ada di
  `crates/qh-sql-grammar/src/lib.rs` (deklarasi `extern "C"` dan `LanguageFn::from_raw`, keduanya berbentuk seperti
  yang didokumentasikan `tree-sitter-language`).
- Backlog terbuka B-19 (tidak ada pemeriksaan `malloc` NULL di `scanner.c`; `add_char` memotong karakter non-ASCII pada
  tag, salah parse hanya di editor) dan B-20 (`build.rs` memakai `warnings(false)` sehingga peringatan `scanner.c` ikut
  tersembunyi). Keduanya dicatat di ledger run, tanpa perubahan kode oleh ADR ini.

### MSRV

- Workspace menyatakan `rust-version = "1.85"` (`Cargo.toml`). tree-sitter 0.27 dan `tree-sitter-language` 0.1.8
  membutuhkan Rust 1.90, dan resolver yang tidak menghormati MSRV akan memilihnya pada `cargo update` berikutnya.
  Karena itu keduanya dipatok dengan `=` ke 0.26.13 dan 0.1.7 (`rust-version` yang dinyatakan crate: 1.77).
- Diperiksa lewat `cargo metadata`: `arrow-array` dan `arrow-ipc` 59.3.0 menyatakan 1.85, jadi MSRV efektif workspace
  tetap 1.85 dan tidak naik karena editor. Toolchain lokal `rustc 1.98.1`; tidak ada `rust-toolchain.toml`, dan CI
  (`.github/workflows/verify.yml`) memasang `stable`. **MSRV 1.85 karena itu dijaga oleh pematokan versi, bukan oleh
  job CI**: tidak ada job yang membangun dengan 1.85.
- Helper analitik (DataFusion) punya workspace sendiri dengan rust 1.94 (ADR-0045 nanti); itu tidak menaikkan MSRV app.

## Alasan

1. **Pohon per statement adalah keputusan yang paling menentukan.** Reparse satu ketikan turun dari 0,85 sampai
   5,3 ms p50 (p99 7,5 ms) pada pohon dokumen ke 0,02 sampai 0,03 ms p50 (≤ 0,09 ms p99), dan warna pertama
   dokumen 2M hanya butuh parse statement yang terlihat (3 sampai 4 ms), bukan seluruh dokumen (130 sampai 770 ms).
   Semua angka dari blueprint §1.3, diukur di Mac16,12 (M4, 10 core) dengan crate buangan di `target/run/ts-bench/`.
2. **Batas statement dari `scan.rs`** karena pohon memberi hasil yang salah persis di tempat yang paling berbahaya
   (Safe Mode, Run), dan di korpus bench yang parse bersih pemisah `scan()` dan `;` teratas pohon sama persis
   (0 selisih di 10.000, 2.457, dan 11.891 pemisah, dan 1 pemisah), jadi pilihan ini tidak mengorbankan apa pun.
3. **Atribut sementara** menghapus penyebab 73% waktu `insertText` tanpa mengubah piksel yang digambar (0 dari 2,8 juta
   piksel berbeda di spike, gelap dan terang).
4. **Di Rust** agar satu parse melayani pemecah, formatter, dan alias, dan agar tes diferensial dan properti tidak
   butuh AppKit.
5. **Vendoring** menyematkan tepat apa yang diukur dan menutup bocor memori yang ditemukan, sementara crate terpisah
   mengurung C dan satu-satunya `unsafe` di tempat kecil yang bisa diperiksa.

## Konsekuensi

### Hasil terukur (bench W3-T2 commit 2, `cargo run --release -p qh-editor --example editor_bench`)

Mesin yang sama (Apple M4) dipakai agen lain secara bersamaan (load average 4,5 sampai 9,2), jadi semua angka adalah
batas atas. Sumber: `target/run/w3t2-bench.md` (gitignored, scratch).

| Ukuran | Target blueprint | Terukur |
|---|---|---|
| `replace` (thread main) p99 di dokumen 2M | < 0,5 ms (§4.1) | 0,036 sampai 0,16 ms |
| Analisis per ketikan (reparse + klasifikasi + selisih + run) p50 / p99 | 0,02 sampai 0,03 / 0,09 ms (hanya reparse) | 0,04 sampai 0,12 / 0,06 sampai 0,31 ms |
| Memori analisis dokumen 2M | ≤ 48 MB | RSS +27 sampai +44 MB; cache pohon tertahan di 639 KiB sumber |
| Jendela pertama berwarna | 3 sampai 4 ms (hanya parse) | 4,6 sampai 13 ms (dengan klasifikasi dan run untuk jendela 48.000 unit) |
| Typing paint p99 di `dump-2m` (satu `INSERT` 2 MB) | ≤ 8 ms | 8,3 sampai 10,2 ms di file bench; baris ledger W3-T2 c2 mencatat 5,169 ms pada satu run lain. Di bawah beban, target ini **belum terpenuhi dengan pasti** |
| Build dingin `qh-sql-grammar` | (throwaway: 15,7 s) | 3,1 s release |

### Positif

- Ketikan di dokumen 2M tetap di bawah 0,2 ms p99 di sisi thread main (`replace`), dan analisis berjalan di latar.
- Satu pemecah statement untuk engine, Safe Mode, Run, band, dan run mark; `sql_statement_ranges(sql, dialect)` mengekspos
  pemecah itu, sehingga Run di tab MySQL memecah seperti engine begitu dialek per tab tersambung.
- Kelas warna, lipatan (CTE, subquery, badan `$tag$`), dan fondasi alias dan formatter datang dari satu parse.
- Warna adalah fungsi teks statement saja, sehingga tidak bergantung pada riwayat edit.
- `qh-sql` dan ketiga driver tidak mengompilasi C 17 MB: arah dependensi `qh-editor → qh-sql`, tidak sebaliknya.

### Negatif

- **C di dalam proses** (bagian risiko di atas): crash di parser menjatuhkan app dan tidak tertangkap `catch_unwind`.
- **Ukuran dan build:** biner app naik sekitar 2,63 MB (11%) dan build dingin sekitar 15 detik pada pengukuran blueprint
  §1.7. Angka itu tidak diukur ulang sesudah semua commit mendarat. Tidak ada target ukuran di PRD.
- **Berkas vendor 17 MB** di repo (sekitar 818 KB terkompresi), dan setiap pembaruan grammar adalah tugas berbobot dengan
  empat gate.
- **Pohon inkremental bisa berbeda dari pohon baru** pada statement ber-ERROR (14 sampai 16% edit acak di bench blueprint);
  lipatan dan alias bisa bergantung pada riwayat sampai konvergensi (500 ms). Warna tidak terpengaruh.
- **Memori pohon** 36 sampai 54 B per byte sumber; dibatasi cache LRU, tetapi batas itu adalah angka setelan
  (`TREE_CACHE_BYTES`, `GIANT_STATEMENT_BYTES`), bukan jaminan struktur.
- **Pemilik kunci atribut sementara** adalah konvensi, bukan jaminan tipe: fitur baru yang memakai `setTemporaryAttributes`
  akan menghapus sorotan find. Blueprint §9 menuntut tes yang memastikan sorotan find bertahan sesudah `apply`; ADR ini tidak memeriksa keberadaannya satu per satu.
- **Dialek editor selalu `.generic`** sampai W10-T6 menyambungkan dialek per tab, sehingga warna dan pemecahan di tab
  MySQL belum memakai aturan MySQL walau engine sudah.
- **Satu jalan `walk` per ketikan di statement raksasa**: `walk` belum bisa dilanjutkan dari titik tengah, jadi
  `dump-2m` menjalankan satu `walk` seluruh statement tiap ketikan. Menambahkan `walk` yang bisa dilanjutkan butuh
  perubahan API `qh-sql` dan masih menjadi keputusan terbuka (ledger).

### Penyimpangan dari blueprint 4B, sebagai dikirim

| Blueprint | Dikirim | Sumber |
|---|---|---|
| Dua antrean: `qh.editor.visible` (`.userInitiated`) dan `qh.editor.idle` (`.utility`) | Satu antrean paint `queryhive.editor.analysis` tanpa QoS eksplisit di `EditorAnalysis`, ditambah antrean idle `qh.editor.idle` (`.utility`) di `SQLEditor`. Tidak ada antrean `qh.editor.visible` | `EditorAnalysis.swift`, `SQLEditor.swift:308` |
| Struktur FFI menyimpan `latest: AtomicU64` | Dihilangkan; ditambah `converge()` | Ledger W4-T2A |
| `regions()` memakai rentang yang diteruskan | Mengabaikan rentang yang diteruskan | Ledger W4-T2B |
| Tes undo lewat AppKit | Tes undo lewat `replace` | Ledger W4-T2A |
| W4-T2 A dan B dengan W4-T2b di antaranya, tanpa tugas lain di antaranya | A (`4cdd377`), lalu dua commit bukan editor (`8583dfa` tes grid, `cc577ce` pembersihan), lalu B (`3f3fcd3`), lalu W4-T2b (`c5e0213`, hanya `StatementSplitTests`); isi `sqlStatements(in:)` memakai FFI sudah ikut `3f3fcd3`. Syarat verdict AR (tidak ada tugas lain di antara A dan B) tidak dipenuhi secara harfiah | `git log` |

## Bukti

- Kode: `crates/qh-sql/src/scan.rs` (`walk`), `crates/qh-sql/src/lex.rs`, `crates/qh-sql-grammar/` (`PROVENANCE.md`,
  `tests/scanner_leak.rs`), `crates/qh-editor/` (`src/{statements,syntax,classify,paint,folds,issues,keywords,view,text}.rs`;
  `tests/{golden,incremental,folds,statements}.rs`; `examples/editor_bench.rs`), `crates/qh-ffi/src/editor.rs`,
  `app/Sources/QueryHive/Support/EditorAnalysis.swift`, `app/Sources/QueryHive/Views/SQLEditor.swift`.
- Commit: `c96c6d6` (W3-T2 commit 1: `walk`, `lex`, grammar; tinjauan keamanan satu putaran disetujui, perbandingan
  sekitar 100 juta kasus dengan scanner `a25fa50` tanpa selisih), `551de40` (`qh-editor`), `4cdd377` (FFI dan
  `EditorAnalysis`), `3f3fcd3` (penerapan, V-12), `c5e0213` (tes pemecah statement).
- Gate yang tercatat di ledger run pada saat penulisan: `cargo test --workspace` 975 lulus pada W4-T2A dan W4-T2B,
  `cargo deny check licenses` baik, VIS 17/0 sesudah V-12 direkam ulang. ADR ini tidak menjalankan ulang gate.
- Blueprint: `docs/architecture/blueprints/fase-4b-editor-analysis.md` (verdict architect-reviewer: disetujui dengan
  koreksi, 27 koreksi diterapkan, termasuk D-21).

## Referensi

- ADR-0009 (panic unwind di FFI), ADR-0017 dan ADR-0026 (Safe Mode memakai pemecah yang sama), ADR-0031 (EngineHost).
- `docs/architecture/blueprints/fase-4b-editor-analysis.md`, `docs/architecture/performance-plan.md` §8, §13, §14,
  `docs/architecture/prd-performance-and-parity.md` (O-14, V-12, P-30, NFR-A1, NFR-L, NFR-P5, FR-ED-06, P-10).
- `crates/qh-sql-grammar/PROVENANCE.md`, `THIRD-PARTY-NOTICES.md`.
- Tugas penerus: W10-T6 (garis bawah diagnostik dan dialek per tab), W10-T7 (pasangan kurung), W11-T6 (alias), W12-T1
  (formatter), W14 (pemberitahuan lengkap di bundle).
