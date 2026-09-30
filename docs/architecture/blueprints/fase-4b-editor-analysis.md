# Blueprint Fase 4B: analisis editor dengan tree-sitter

- **Status:** revisi 30 Sep 2026 untuk keputusan pemilik O-14 (tree-sitter dipakai untuk analisis editor; paritas visual dengan pewarnaan regex tidak lagi dituntut). Menggantikan blueprint lexer regex di `dc4186f`, yang verdict AR-nya disimpan di bagian terakhir sebagai riwayat. Belum ada kode. **Diperiksa `architect-reviewer` 30 Sep 2026: disetujui dengan koreksi** (verdict di bagian "Verdict architect-reviewer (tree-sitter)"; koreksinya ditandai "(koreksi AR-TS)"). Langkah 2 W3-T2 (refaktor `scan.rs`) menunggu W3-T0 (§15).
- **Untuk:** W3-T2 (inti Rust), W4-T2 (FFI dan integrasi Swift), W4-T2b (Run memakai pemecah yang sama). Titik kait untuk W10-T6 (diagnostik), W10-T7 (pasangan kurung), W11-T6 (alias dan tabel), W12-T1 (formatter).
- **Sumber:** `performance-plan.md` §8 dan §13; PRD FR-PERF-04, FR-ED-01, FR-ED-05, FR-ED-06, FR-ED-09, NFR-A1, NFR-L, NFR-P5, NFR-V, P-10; `development-plan.md` W3-T2, W4-T2, W4-T2b, W10-T6, W10-T7, W11-T6, W12-T1; kode di `work/perf-parity` sesudah 4A (`93c864b`, dengan `2e6f7ef` di atasnya); masukan profil reviewer 4A lewat orkestrator (73% waktu `insertText` adalah pembetulan atribut TextKit 1, berbanding lurus dengan jumlah run atribut).
- **Cara membaca klaim.** Setiap angka di §1 diukur di mesin ini (Mac16,12, M4, 10 core) dengan crate buangan di `target/run/ts-bench/` (gitignored) dan spike Swift di `target/run/ts-bench/spike/`. Perintah ulangnya ada di §1.1. Beban mesin dicatat karena agen lain sedang membangun di mesin yang sama; angka yang diambil di bawah beban tinggi ditandai. Yang belum diukur ditulis **[belum diukur]**.
- **Kontrak.** Pilihan grammar (§3), unit parse per statement (§4), pemetaan kelas ke warna (§5), API FFI (§6), dan penerapan lewat atribut sementara (§7) mengikat W3-T2 dan W4-T2. Angka ambang (batas statement besar, anggaran cache pohon) boleh disetel ulang oleh W3-T2 dari bench-nya sendiri, dengan angka dicatat di laporan.

## Ringkasan

Tree-sitter dipakai di Rust, bukan di Swift, di crate baru `qh-editor`, dengan grammar `tree-sitter-sql` milik DerekStride (crate `tree-sitter-sequel` 0.3.11, MIT, diverifikasi dari repo upstream). Parser C-nya di-vendor ke crate kecil `qh-sql-grammar` bersama satu perbaikan scanner dari upstream, karena versi crates.io bocor sekitar 74 byte per edit di dalam badan `$tag$`.

Keputusan yang paling menentukan: **satu pohon per statement, bukan satu pohon per dokumen.** Batas statement tetap diambil dari `qh_sql::scan`, pemecah yang sama dengan engine dan Safe Mode. Dengan pohon per statement, reparse satu ketikan turun dari 0,85–5,3 ms p50 (sampai 7,5 ms p99) pada pohon dokumen 0,4–2 MB ke 0,02–0,03 ms p50 (≤ 0,09 ms p99), dan warna pertama dokumen 2M hanya butuh parse statement yang terlihat (3–4 ms), bukan seluruh dokumen (130–770 ms).

Warna diterapkan sebagai **atribut sementara `NSLayoutManager`** (`.foregroundColor`), bukan atribut storage. Storage hanya membawa atribut dasar dan font italic komentar. Di spike 195k karakter, `insertText` di tengah dokumen turun dari 3,7 ms ke 0,5 ms, layout rentang terlihat dari 3,5 ms ke 0,04 ms, dan piksel yang digambar identik (0 dari 2,8 juta piksel berbeda, gelap dan terang).

Tiga butir di daftar pemilik tidak diikuti apa adanya, dengan bukti di §0.2: batas statement dan run mark tetap dari `scan.rs` (pohon hanya mengisi isi statement); galat sintaks dari pohon tidak ditampilkan secara default karena 14–47% statement valid per dialek ditandai ERROR; dan formatter tidak mengambil token dari pohon.

## 0. Keputusan pemilik dan posisi blueprint ini

### 0.1 Yang berubah dari blueprint regex

| Topik | Blueprint regex (`dc4186f`) | Blueprint ini |
|---|---|---|
| Lexer warna | Port regex byte demi byte, termasuk keanehannya, dibuktikan dengan paritas | Pohon tree-sitter per statement, ditambah lexer cadangan untuk celah ERROR. Tidak ada paritas; perubahan tampilan didaftar sebagai V-12 |
| Tabel Unicode ICU, eksportir fixture Swift, L4/L5 paritas | Wajib | Dihapus seluruhnya |
| Batas statement | `scan.rs` | `scan.rs` (tidak berubah) |
| Unit analisis | Blok checkpoint 64 baris | Satu statement `scan.rs` = satu pohon |
| Penerapan warna | Atribut storage di rentang kotor | Atribut sementara layout manager, storage nyaris seragam |
| Lipatan | Port `SQLFolding` | Dari node pohon: statement, CTE, subquery, badan `$tag$` |
| Diagnostik | `unclosed` dari `scan.rs` | Sama, ditambah data ERROR/MISSING yang tidak ditampilkan secara default |
| Dependensi baru | Tidak ada | `tree-sitter` 0.26.13, `tree-sitter-language` 0.1.7, grammar yang di-vendor, `streaming-iterator` (transitif) |

### 0.2 Tempat blueprint ini tidak mengikuti daftar pemilik apa adanya

1. **Batas statement dan run mark tetap dari `scan.rs`, bukan dari pohon.** Pohon mengisi isi statement (kelas token, lipatan, alias), tidak menentukan di mana statement berakhir. Buktinya:
   - `ANALYZE wide_500k;` di `deploy/dev/seed-postgres.sql:72` diparse menjadi satu node ERROR yang **menelan `;`**. Pohon menghitung 8 statement di seed itu, `scan()` menghitung 9. `ANALYZE TABLE wide_500k;` di `seed-mysql.sql:79` sama.
   - Grammar membungkus `BEGIN; …; COMMIT` menjadi satu node `transaction`, jadi anak teratas pohon bukan statement.
   - Di korpus bench (`lines-10k`, `chars-2m`, `bench-10k`, `dump-2m`), pemisah `scan()` dan `;` teratas pohon sama persis (0 selisih di 10.000, 2.457, 11.891, dan 1 pemisah). Jadi pemilihan ini tidak mengorbankan apa pun di teks yang parse bersih, dan menutup kasus yang tidak parse.
   - Run, Safe Mode, dan `script` (W12-T3) memecah dengan `scan.rs`. Band yang memakai sumber lain akan kembali ke bug yang ditutup W4-T2b (`select "a;b"` tampil satu statement, Run mengirim setengahnya).
2. **ERROR dan MISSING dari pohon tidak digambar sebagai garis bawah secara default (W10-T6).** Dari korpus uji dialek buatan sendiri (§1.2), statement **valid** yang pohonnya memuat ERROR atau MISSING: PostgreSQL 3 dari 21 (14%), MySQL 9 dari 19 (47%), Trino 8 dari 17 (47%). Contohnya `SHOW CATALOGS`, `USE tpch.sf1`, `DESCRIBE t`, `FETCH FIRST 5 ROWS ONLY`, `TABLESAMPLE BERNOULLI (10)`, `GROUPING SETS`, `LIMIT 10, 20`, dan `UPDATE … ORDER BY … LIMIT`. Garis bawah di bawah SQL yang benar lebih merugikan daripada tidak ada garis bawah. Datanya tetap dihitung dan diekspor (§4.8); yang ditampilkan W10-T6 adalah masalah leksikal dari `scan.rs` dan posisi galat dari server.
3. **Formatter (W12-T1) mengambil token dari mesin `scan.rs` dan lexer kode `qh-sql`, bukan dari pohon** (§8.4). Formatter yang menolak bekerja di setiap statement ber-ERROR akan menolak separuh SQL MySQL dan Trino yang valid, dan formatter yang tetap bekerja di atas pohon ber-ERROR berisiko memindahkan spasi di tempat yang pohonnya salah baca.

Lipatan per node sintaks, alias untuk autocomplete, dan kelas warna diambil dari pohon sesuai keputusan pemilik.

## 1. Bukti

### 1.1 Lingkungan dan cara mengulang

- Crate buangan: `target/run/ts-bench/` (grammar crates.io 0.3.11), `ts-bench-fixed/` (0.3.11 + perbaikan scanner upstream PR #361), `ts-bench-abi15/` (grammar 0.3.11 dibangkitkan ulang dengan ABI 15 memakai `tree-sitter-cli` 0.26.13 yang dipasang lokal di `target/run/tools/`), `ts-bench-main/` (snapshot `gh-pages` upstream `39fdb00640`, yaitu build dari `main@97614d05`). Semuanya `[workspace]` sendiri, profil rilis sama dengan workspace (`lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`, `panic = "unwind"`).
- Korpus: `deploy/dev/make_sql_corpus.py` (`lines-10k.sql` 411.038 B, `chars-2m.sql` 2.000.000 B, keduanya penuh `:name`, komentar, dan `$tag$`), salinan persis dokumen `BenchMode.sqlDocument` (`bench-10k.sql` 957.780 B, `bench-2m.sql` 2.000.010 B), dan `dump-2m.sql` (satu `INSERT` 1.999.878 B dalam satu baris). Awalan `sub-` berarti `:name` diganti `_name` dengan panjang sama (§4.3).
- Perintah: `target/release/bench parse1|type1|chunked|flip|patho|agree|probe|incr|leak|mirror`, dan `spike/spike bench|verify [light] [nocheck]`. Keluaran yang dipakai di dokumen ini tersimpan di `target/run/ts-bench/results.txt`, `probe-out*.txt`.
- Beban: pengukuran §1.3 diambil pada load average 3–5 (sepuluh core), kecuali yang ditandai.

### 1.2 Grammar, lisensi, dan cakupan dialek

| Kandidat | Lisensi (diperiksa dari repo) | Status | Putusan |
|---|---|---|---|
| **DerekStride/tree-sitter-sql**, crate `tree-sitter-sequel` | MIT, `LICENSE` upstream "Copyright (c) 2021 Derek Stride"; `license = "MIT"` di crate | Aktif (push 19 Sep 2026, 247 bintang). Rilis terakhir 0.3.11 tanggal 1 Okt 2025; `main` 64 commit lebih maju dan tidak meng-commit `parser.c` | **Dipakai, versi 0.3.11, di-vendor** |
| m-novikov/tree-sitter-sql | MIT | Push terakhir 6 Mar 2024 | Ditolak: tidak dipelihara |
| gmr/tree-sitter-postgres | BSD-3-Clause | Aktif, versi beta | Ditolak: hanya PostgreSQL |
| takegue/tree-sitter-sql-bigquery | MIT | Mei 2024 | Ditolak: BigQuery |
| tree-sitter (runtime) | MIT (repo dan crate) | 0.27.0 butuh Rust 1.90, `tree-sitter-language` 0.1.8 juga | **0.26.13 dan 0.1.7 dipatok**, karena `rust-version = "1.85"` di workspace |
| SwiftTreeSitter (tree-sitter/swift-tree-sitter) | BSD-3-Clause | Aktif | Tidak dipakai (§2, D-3) |

**Isi grammar 0.3.11 yang relevan** (`grammar.js` di crate):

- Dollar-quote lewat scanner eksternal (`src/scanner.c`), termasuk tag bernama. Badan `CREATE FUNCTION … AS $body$ … $body$` diparse sebagai SQL; tag menjadi node `dollar_quote`.
- Backtick hanya untuk identifier sederhana: `` seq("`", $._identifier, "`") ``. `` `order details` `` menjadi ERROR (diperbaiki di `main`, belum dirilis).
- `"…"` berupa regex `/"[^"]*"/` tanpa escape `""`, dan bisa menjadi `identifier` atau `literal` tergantung konteks.
- Parameter hanya `?` dan `$1` (`parameter: /\?|(\$[0-9]+)/`). **`:name` tidak dikenal**: setiap `:` menjadi ERROR satu karakter.
- Identifier: ASCII ditambah U+00C0–U+017F. `имя` menjadi ERROR.
- String `'…'` dengan `''`; `E'…'` dengan `\'`. Escape backslash MySQL di string biasa tidak dikenal (`'it\'s'` menjadi ERROR), sama dengan `scan.rs` hari ini.
- Kata kunci: 340 simbol `keyword_*`. Dari 131 kata kunci `SQLSyntax.keywords`, yang tidak ada di grammar: `apply`, `at`, `catalog`, `charset`, `describe`, `fetch`, `grant`, `identity`, `regexp`, `revoke`, `rlike`, `straight_join`, `try_cast`, `unnest`.
- Kueri `queries/highlights.scm` bawaan ditulis untuk nvim-treesitter: `#match?` dengan pola Lua (`%d`) yang tidak dikenal mesin regex Rust, capture `@spell`, dan semua nama tabel ditandai `@type`. Tidak bisa dipakai apa adanya (§2, D-6).

**Uji dialek** (`target/run/ts-bench/probes.sql`, SQL valid satu statement per baris, ditulis sendiri):

| Kelompok | Bersih | Ber-ERROR/MISSING | Contoh yang gagal |
|---|---|---|---|
| PostgreSQL | 18 | 3 | `ON CONFLICT (a) DO UPDATE`, `DO $$ … RAISE …$$`, `имя` |
| MySQL | 10 | 9 | `` `order details` ``, `'it\'s'`, `LIMIT 10, 20`, `@x := …`, `REGEXP`, `UPDATE … ORDER BY … LIMIT`, `WITH ROLLUP`, `STRAIGHT_JOIN`, `DESCRIBE` |
| Trino | 9 | 8 | `WITH ORDINALITY`, `TABLESAMPLE`, `OFFSET … FETCH FIRST`, `SHOW CATALOGS`, `SHOW SCHEMAS FROM`, `USE`, `GROUPING SETS`, `EXPLAIN (TYPE …)` |
| Parameter QueryHive | 0 | 2 | setiap `:name` |

Yang lulus termasuk `$$a;b$$`, fungsi `$body$`, `a::text`, `E'…'`, `$1`, `->>`/`#>`/`@>`, `WITH RECURSIVE`, `FILTER … OVER (… ROWS BETWEEN …)`, `ARRAY[…]` dan slice, `DISTINCT ON`, `LATERAL`, lambda Trino `x -> x * 2`, nama tiga bagian `tpch.sf1.lineitem`, `TRY_CAST`, literal bertipe `date '…'`, dan `VALUES` sebagai tabel.

**`main` upstream tidak layak dipakai sekarang.** Snapshot `39fdb00640` (build dari `main@97614d05`) menghasilkan `parser.c` 41,6 MB (0.3.11: 17,4 MB) dan pustaka statis grammar 11,09 MB (0.3.11: 2,47 MB), dengan hanya dua uji tambahan yang lulus (backtick berspasi dan `REGEXP`).

### 1.3 Parse, reparse, dan memori

**Parse penuh satu pohon per dokumen** (median n = 5):

| Dokumen | Byte | ERROR | Parse | Parse dengan view `:name` | Heap pohon |
|---|---|---|---|---|---|
| `lines-10k` | 411.038 | 2.327 | 133–152 ms | 29 ms (0 ERROR) | 14,9 MB |
| `bench-10k` | 957.780 | 0 | 78–90 ms | — | 51,6 MB |
| `chars-2m` | 2.000.000 | 11.414 | 648–768 ms | 133 ms (0 ERROR) | 72,8 MB |
| `bench-2m` | 2.000.010 | 0 | 162–216 ms | — | 106,6 MB |
| `dump-2m` | 1.999.878 | 0 | 126–153 ms | — | 101,9 MB |

Memori pohon 36–54 byte per byte sumber. Semua ERROR di korpus berasal dari `:name` (dicek per templat); pemulihan galat membuat parse 5–6 kali lebih lambat.

**Ketikan satu karakter, satu pohon per dokumen** (200 ketikan di awal baris tengah; p50/p99 ms; load ≈ 5):

| Dokumen | Reparse | `changed_ranges` |
|---|---|---|
| `sub-lines-10k` | 0,85 / 0,92 | 0,73 / 0,76 |
| `bench-10k` | 2,49 / 3,05 | 2,35 / 3,04 |
| `sub-chars-2m` | 4,71 / 5,63 | 4,01 / 4,60 |
| `bench-2m` | 5,25 / 7,52 | 5,01 / 6,69 |
| `dump-2m` (mengetik di dalam string) | 7,57 / 12,50 | 5,61 / 8,04 |

Biaya ini naik bersama jumlah anak node akar (`program: repeat(seq(statement, ';'))`, 2,5–20,6 ribu statement di korpus ini) atau panjang daftar di dalam satu node (`dump-2m`), bukan bersama ukuran edit.

**Satu pohon per statement `scan.rs`** (load ≈ 3,7):

| Dokumen | Statement (terbesar) | Parse semua | Parse jendela ±24 KB | Ketikan: edit + reparse p50 / p99 |
|---|---|---|---|---|
| `sub-lines-10k` | 2.458 (1.277 B) | 45 ms | 3,7 ms (308 statement) | 0,021 / 0,026 ms |
| `bench-10k` | 10.001 (96 B) | 79 ms | 3,9 ms | 0,018 / 0,024 ms |
| `sub-chars-2m` | 11.892 (1.277 B) | 108 ms | 2,6 ms | 0,028 / 0,033 ms |
| `bench-2m` | 20.636 (98 B) | 163 ms | 3,9 ms | 0,026 / 0,032 ms |
| `chars-2m` (tanpa view) | 11.892 | 641 ms | 16,2 ms | 0,030 / 0,087 ms |
| `dump-2m` | 2 (1.999.877 B) | 134 ms | 128 ms | 6,8 / 17,3 ms |

Angka ketikan sudah termasuk menggeser offset semua statement sesudahnya. Heap total sama dengan pohon tunggal (14,4 / 52,4 / 70,5 / 108,1 MB). Kasus `dump-2m` (satu statement 2 MB) tidak tertolong pohon per statement, dan karena itulah ada batas statement besar (§4.4).

**Sisi main.** Cermin teks Rust (sisip ke `String` 2 MB, geser 48.629 awal baris dan 11.891 awal statement): p50 0,018 ms, p99 0,020 ms; di bawah load 11: p99 0,22 ms. Biaya satu panggilan UniFFI ke objek **[belum diukur]**; diukur W4-T2 di bench FFI.

### 1.4 Edit yang mengubah makna teks sesudahnya

Satu sisipan di awal baris tengah (`flip`, model per statement; "rescan" di sini adalah `scan()` seluruh dokumen, batas atas dari resinkronisasi §4.2):

| Dokumen | Sisipan | Rescan | Statement | Yang berubah | Reparse yang berubah ∩ jendela |
|---|---|---|---|---|---|
| `sub-lines-10k` | `'` | 4,4 ms | 2.458 → 2.446 | 2 (2.026 B) | 1,8 ms |
| `sub-lines-10k` | `$$` | 1,4 ms | 2.458 → 1.585 | 356 (205 KB) | 4,3 ms |
| `sub-chars-2m` | `"` | 5,0 ms | 11.892 → 5.966 | 1 (1,0 MB) | 0,17 ms |
| `sub-chars-2m` | `$$` | 4,0 ms | 11.892 → 7.625 | 1.660 (1,0 MB) | 1,9 ms |
| `bench-2m` | `'` | 4,5 ms | 20.636 → 10.431 | 1 (1,0 MB) | 0,24 ms |
| `bench-2m` | `;` | 6,8 ms | 20.636 → 20.637 | 2 (99 B) | 0,02 ms |

Statement 1 MB yang muncul saat kutip belum ditutup diwarnai dengan lexer saja (§4.4), sesuai dengan cara engine akan membaca teks itu.

Sebagai pembanding, pada pohon tunggal sisipan `$$` di `sub-chars-2m` butuh reparse 153 ms dan menghasilkan 7.610 ERROR, dan pembatalannya 67 ms.

### 1.5 Stabilitas inkremental dan lexer cadangan

Uji diferensial: 2.000 edit acak (sisip dan hapus `'`, `"`, `$`, `$body$`, `/*`, `*/`, `--`, LF, `;`, huruf, `é`, 😀) di potongan 12.000 karakter yang memuat badan fungsi `$body$`. Setelah setiap edit, pohon inkremental dibandingkan dengan parse dari nol.

| Grammar | S-expression berbeda | Semuanya pohon ber-ERROR? | Daftar token warna berbeda, tanpa lexer celah | Dengan lexer celah (§4.5) |
|---|---|---|---|---|
| 0.3.11, edit dibias ke badan `$tag$` | 282 | ya | 282 | **0** |
| 0.3.11, edit merata | 329 | ya | 329 | **0** |
| 0.3.11 dibangkitkan ulang dengan ABI 15 | 282 / 329 | ya | — | — |
| `main@97614d05` | 22 / 4 | ya | 22 / 4 | tidak diuji |

Artinya:

- Pohon inkremental 0.3.11 berbeda dari parse baru di 14–16% edit, **hanya di pohon ber-ERROR**, dan penyebabnya ada di isi grammar, bukan versi ABI.
- Perbedaannya hampir selalu soal token mana yang dilewati pemulihan galat. Contoh: `FROM` muncul sebagai `keyword_from` di dalam ERROR pada pohon inkremental, dan menjadi celah tanpa anak di pohon baru. Node ERROR punya byte yang tidak dimiliki anak mana pun (`SELEC 1;` adalah satu ERROR tanpa anak kecuali `;`).
- Dengan celah ERROR diwarnai oleh lexer leksikal, warna menjadi fungsi dari teks saja: **0 perbedaan dari 4.000 edit**. Diagnostik dan lipatan tetap bergantung pada pohon, jadi statement ber-ERROR diparse ulang dari nol saat idle (§4.4).
- **Batas bukti ini (koreksi AR-TS).** `cmd_incr` di `target/run/ts-bench/src/main.rs` memakai **satu pohon untuk potongan 12.000 karakter**, bukan pohon per statement, **tanpa** view `:name`, dan sesudah setiap selisih pohon inkremental **diganti pohon baru** (`tree = fresh`). Jadi yang terbukti adalah "0 dari 611 pohon yang menyimpang satu langkah", bukan bahwa penyimpangan yang menumpuk selama banyak edit tidak pernah mengubah warna. Klaim itu juga tidak dijamin secara struktur: anak ERROR di pohon inkremental diwarnai pohon, sedangkan byte yang sama di pohon baru bisa menjadi celah yang diwarnai lexer. Karena itu: (1) G2 (§9) menjalankan model yang sungguhan (pohon per statement, view, lexer celah) **tanpa** resinkronisasi, sehingga penyimpangan boleh menumpuk; (2) konvergensi §4.4 menghitung ulang token dan menambahkan selisihnya ke `dirty`, sehingga warna akhirnya selalu sama dengan parse baru walaupun klaim per edit gagal; (3) bila G2 menemukan selisih warna sebelum konvergensi, W3-T2 mengganti aturan ERROR menjadi "seluruh rentang ERROR dilex, anaknya diabaikan", lalu mengulang G2.

**Kebocoran scanner.** 100.000 edit di dalam badan `$body$`, lalu pohon dan parser dibuang: heap naik 7,35 MB dengan scanner 0.3.11, dan 0,20 MB dengan perbaikan PR #361 (0,17 MB pada 20.000 edit, jadi tidak tumbuh). Penyebabnya `tree_sitter_sql_external_scanner_deserialize` menimpa `start_tag` tanpa `free` (`src/scanner.c:180-187` di crate), sementara `serialize` (`:161-178`) membebaskannya di tempat yang salah.

### 1.6 Atribut sementara: spike AppKit

Spike `target/run/ts-bench/spike/TempAttrSpike.swift`: tumpukan TextKit 1 yang sama dengan `SQLEditor` (`NSLayoutManager` sendiri, `allowsNonContiguousLayout`, `isRichText = false`), dokumen 195k karakter berpola `type-coloured-195k`, 200 ketikan `"select id, name from t where x = 1 and y in (2) "`, lalu layout rentang terlihat dan gambar ulang baris caret lewat `cacheDisplay`. Rilis, load ≈ 3. p50/p99 ms.

| Mode | `insertText` tengah | Layout terlihat tengah | Gambar baris caret | `insertText` di akhir | Run storage |
|---|---|---|---|---|---|
| Seragam (tanpa warna) | 0,90 / 1,26 | 0,00 | 0,26 / 0,34 | 0,28 / 0,32 | 1 |
| Warna di storage (hari ini) | 3,72 / 4,09 | 3,53 / 3,93 | 4,19 / 4,64 | 1,84 / 2,17 | 65.908 |
| **Warna sementara, italic komentar di storage** | **0,53 / 0,80** | **0,04 / 0,06** | 5,28 / 5,67 | **0,40 / 0,65** | 3.623 |
| Warna sementara, tanpa italic | 0,49 / 0,74 | 0,00 | 5,37 / 5,80 | 0,31 / 0,55 | 1 |

Pemeriksaan perilaku (`spike verify`, `spike verify light`):

- **Piksel identik.** Warna storage dan warna sementara menghasilkan 0 dari 2.800.000 piksel berbeda, di gelap dan terang, dengan `NSColor` dinamis. `cacheDisplay` menggambar atribut sementara (storage dibanding tanpa warna: 327.096 piksel berbeda di gelap, 297.556 di terang), jadi G-VIS tetap menangkapnya.
- **`.font` sebagai atribut sementara diabaikan**: 0 piksel berubah. Italic komentar harus tetap di storage, atau dihapus.
- **Karakter yang diketik tidak mewarisi warna sementara**: di dalam kata kunci, sesudahnya, di dalam komentar, dan di posisi 0, semuanya tanpa warna sementara. Di `NSTextView` dengan `isRichText = false`, karakter baru juga mendapat `typingAttributes` apa adanya, bukan atribut storage tetangga: di dalam komentar ia tegak, bukan italic. Ini menjawab `[perlu verifikasi]` blueprint lama.
- **Undo tidak memulihkan atribut sementara**: `select` dihapus lalu di-undo, dan warna sementaranya hilang. Undo selalu masuk jalur edit, jadi rentangnya dicat ulang (§7.5).

**Sisa biaya: gambar ulang baris yang diedit.** Sesudah edit, menggambar satu baris berwarna di jalur `cacheDisplay` makan 4–5 ms baik warna di storage maupun sementara, sedangkan baris seragam 0,26 ms. Menggambar ulang baris yang sama tanpa edit di antaranya hanya 0,25 ms, kemungkinan karena isi layer diambil dari cache tanpa `drawRect`. Profil `sample`: `NSColor` → `CGColor` dibuat per run (`create_color`, `evaluate_headroom`), state display list CoreGraphics per run, dan `NSTextCheckingController considerTextCheckingForRange:` (sekitar seperempat waktu gambar). Mematikan deteksi tautan, deteksi data, penyelesaian teks, dan prediksi inline menurunkan gambar dari 5,49 ke 4,20 ms. `NSColor` statis dan `NSColor(cgColor:)` tidak menurunkannya secara berarti. **Apakah jalur di layar semahal ini [belum diukur]**: W4-T2 mengukurnya dengan signpost di app, dan itu gate NFR-P5 (§14).

### 1.7 Ukuran biner dan lisensi

- **Delta biner** (rilis, stripped, LTO fat, binari dengan satu parse, satu edit, dan satu query):
  - di atas baseline yang sudah menautkan `regex` (app sudah menautkannya lewat `mysql_common`): **+2,63 MB** untuk 0.3.11; +11,31 MB untuk snapshot `main`;
  - di atas baseline polos: +3,75 MB.
  - Binari app hari ini 23,67 MB (`app/dist/QueryHive.app/Contents/MacOS/QueryHive`), jadi 0.3.11 menambah sekitar 11%.
- **Waktu build**: `parser.c` 17,4 MB dikompilasi sekali (build dingin crate buangan 15,7 dtk total), lalu di-cache.
- **`cargo deny check licenses`** dengan `deny.toml` repo: `licenses ok`. Crate baru: `tree-sitter` 0.26.13 (MIT), `tree-sitter-language` 0.1.7 (MIT), `tree-sitter-sequel` 0.3.11 (MIT), `streaming-iterator` 0.1.9 (MIT OR Apache-2.0), dan `cc` 1.2.67 (build, MIT OR Apache-2.0; versi kedua karena crate grammar mematok `~1.2.1`, dan hilang bila grammar di-vendor dengan `build.rs` sendiri). `regex`, `serde_json`, `shlex`, dan `find-msvc-tools` sudah ada di `Cargo.lock`.
- **Di luar jangkauan `cargo deny`**: runtime C tree-sitter membawa header UTF ICU di bawah lisensi Unicode (`lib/src/unicode/LICENSE` di crate `tree-sitter`), dan `parser.c`/`scanner.c` yang di-vendor membawa MIT Derek Stride. Keduanya dicatat di `PROVENANCE.md` crate grammar. Repo belum punya berkas pemberitahuan pihak ketiga untuk app; itu celah yang sudah ada sebelum 4B (§15).

## 2. Keputusan desain

| # | Keputusan | Alasan |
|---|---|---|
| D-1 | **Grammar `tree-sitter-sequel` 0.3.11, di-vendor ke `crates/qh-sql-grammar`**, dengan `parser.c` apa adanya dari crate (checksum `9d198ad3…ec7d17`) dan `scanner.c` yang diberi patch PR #361 upstream. | MIT diverifikasi dari upstream. Perbaikan scanner menghentikan bocor 74 B per edit (§1.5). Rilis crates.io belum membawanya, dan `main` tiga kali lebih besar untuk dua uji tambahan (§1.2). `parser.c` 17,4 MB menjadi sekitar 818 KB terkompresi di git. Crate terpisah mengurung C dan satu-satunya `unsafe` (deklarasi `extern "C"` dan `LanguageFn::from_raw`) di satu tempat kecil yang diperiksa SEC. |
| D-2 | **Runtime `tree-sitter = "=0.26.13"` dan `tree-sitter-language = "=0.1.7"`**, dipatok di `[workspace.dependencies]`. | 0.27.0 dan 0.1.8 butuh Rust 1.90; workspace menyatakan `rust-version = "1.85"`. ABI 15 didukung runtime 0.26. |
| D-3 | **Di Rust, bukan Swift (SwiftTreeSitter).** | Konsumen pohon ada di Rust: pemecah statement (`scan.rs`), formatter W12-T1, dan alias W11-T6. Satu parse melayani semuanya. Tes diferensial dan properti di Rust tidak butuh AppKit. SwiftTreeSitter berarti paket SwiftPM baru (NFR-L: "tidak ada yang direncanakan"), grammar C dibangun dua kali, dan parse kedua untuk formatter. Harga Rust: satu cermin teks (§1.3, ≤ 0,02 ms per ketikan) dan hasil yang menyeberang FFI (kecil, karena hanya selisih). |
| D-4 | **Crate baru `crates/qh-editor`, bukan modul `qh-sql/src/editor/`.** | `qh-sql` dipakai ketiga driver dan `qh-import`. Mereka tidak boleh ikut mengompilasi 17 MB C atau menautkan tree-sitter. Kontrak `qh-sql` ("murni, tanpa global", `forbid(unsafe_code)`) tetap berlaku. Feature flag di crate yang dipakai lima crate lain rawan penyatuan fitur. `qh-editor` juga `forbid(unsafe_code)`. |
| D-5 | **Satu pohon per statement `scan.rs`.** | Reparse 0,02–0,09 ms per ketikan dan parse jendela 3–4 ms, berapa pun ukuran dokumen (§1.3). Statement sama dengan engine dan Safe Mode. ERROR terkurung di satu statement. Cache pohon bisa dibatasi (D-9). |
| D-6 | **Klasifikasi lewat jalan pohon (`TreeCursor`) dan tabel jenis node, bukan `highlights.scm`.** | Kueri upstream khusus nvim (§1.2). Celah ERROR tetap butuh kode sendiri. Konteks induk (`invocation > object_reference > identifier`) terbaca langsung tanpa mesin predikat. |
| D-7 | **Celah ERROR dan statement besar diwarnai lexer leksikal** (`qh_sql::lex`, di atas mesin `scan.rs`). | Warna tidak bergantung pada riwayat edit (§1.5). SQL yang belum selesai diketik tetap berwarna. Kutip yang belum ditutup mewarnai sisa statement sebagai string, seperti yang engine baca. |
| D-8 | **View parse `:name`**: sebelum diparse, `:` yang didahului byte di luar `[A-Za-z0-9_:]` (atau awal statement) dan diikuti `[A-Za-z_]` diganti `_`, dengan panjang sama. **Kelas Parameter tidak diambil dari pohon**, tetapi dari lapis leksikal yang meniru aturan `SQLScanner` (koreksi AR-TS, §4.3). | Parse 5–6 kali lebih cepat di SQL berparameter dan tidak ada ERROR palsu (§1.3). Aturannya lokal (tiga byte), jadi edit hanya perlu diperlebar satu byte di kiri dan kanan. Di dalam string, komentar, atau dollar-quote, penggantian tidak mengubah jenis token. Tanpa fork grammar. Warna parameter sama dengan yang akan diikat Run. |
| D-9 | **Statement > 256 KiB tidak diberi pohon dan tokennya tidak di-cache** (koreksi AR-TS, §4.4), dan cache pohon dibatasi kira-kira 640 KiB sumber per dokumen (≈ 32 MB pohon), LRU. | `dump-2m`: reparse 7–17 ms per ketikan pada satu statement 2 MB (di bawah load 11: p99 62 ms, `results.txt`), sedangkan lexer cukup untuk dump. Pohon 36–54 B per byte berarti 70–108 MB untuk 2M bila semua pohon disimpan. Token, lipatan, dan masalah per statement biasa tetap di-cache setelah pohonnya dibuang, dan parse ulang satu statement rata-rata 8–18 µs. Kedua angka boleh disetel W3-T2. |
| D-10 | **Warna lewat atribut sementara `NSLayoutManager` (`.foregroundColor`).** Storage hanya atribut dasar (warna dasar, font tegak 12,5 pt, paragraph style) ditambah font italic di rentang komentar. | `insertText` 3,7 → 0,5 ms dan layout terlihat 3,5 → 0,04 ms di 195k, piksel identik (§1.6). Run storage 65.908 → 3.623. |
| D-11 | **Himpunan kotor dan selisih token per statement** menggantikan `changed_ranges`. | Rentang edit selalu kotor (karakter baru tanpa warna sementara). Token lama dan baru dibandingkan dengan pemangkasan prefiks dan sufiks, seperti blueprint lama §5.6. Rentang yang dicat ulang saat mengetik: p50 1 unit, p99 54–84 unit (§1.3). `changed_ranges` berbiaya O(anak akar), 0,7–5,6 ms. |
| D-12 | **Diagnostik: leksikal ditampilkan, ERROR/MISSING dihitung tetapi tidak ditampilkan secara default.** | §0.2 butir 2. |
| D-13 | **Formatter W12-T1: token dari `scan.rs` + `qh_sql::lex`.** | §0.2 butir 3, §8.4. |
| D-14 | **Alias W11-T6 dari pohon statement, dengan cadangan leksikal di statement ber-ERROR.** | Pohon memberi `relation` beserta `alias`, `object_reference`, dan nama `cte`. |
| D-15 | **Lipatan dari node**: statement (dari `scan.rs`), `cte`, `subquery`, dan badan `$tag$`, hanya yang lebih dari satu baris. | Permintaan pemilik. Ini perubahan penanda gutter yang didaftar di V-12. |
| D-16 | **Penjaga IME di sisi penerapan** (blueprint lama D-9, diterima AR). | Tidak berubah. |
| D-17 | **Dua kunci; main tidak pernah memegang kunci analisis. QoS dari antrean Swift.** | Tidak berubah (blueprint lama D-7, D-8). |
| D-18 | **Hasil berupa `uniffi::Record` dengan larik `u32` datar.** | Tidak berubah (blueprint lama D-14). |
| D-19 | **W4-T2 tetap dua commit dengan W4-T2b di antaranya.** A: FFI, `EditorAnalysis`, dan tes, tanpa perubahan perilaku. B: jalur baru, regex dihapus, baseline V-12 direkam ulang. | W4-T2b butuh `sql_statement_ranges` dari A. Gate "paritas sebelum regex dihapus" diganti fixture capture emas dan tes diferensial (§9). |
| D-20 | **API punya parameter dialek, W4-T2 mengirim `.generic`.** `sql_statement_ranges` juga menerima dialek (koreksi AR-TS). | Satu-satunya aturan dialek di 4B adalah `"…"` (string di MySQL, identifier di tempat lain), dan `SQLEditor` belum tahu driver tab. Sambungan per tab ikut W10-T6, yang memang butuh dialek untuk posisi galat server. Pemecah Run (`sql_statement_ranges`, W4-T2b) mendapat parameternya sekarang, karena W3-T0 (§15) membuat pemecahan MySQL di engine sadar-dialek; tanpa parameter itu Run di tab MySQL akan memecah berbeda dari engine, dan menambahkannya belakangan berarti memecah permukaan FFI lagi. |
| D-21 | **`qh-sql` tidak tahu daftar kata kunci editor** (koreksi AR-TS). `qh_sql::lex` mengeluarkan `Word` tanpa kelas; himpunan kata kunci (grammar ∪ `EXTRA_KEYWORDS`) tinggal di `qh-editor`. | Arah dependensi: `qh-sql` adalah crate engine yang dipakai driver dan tidak boleh bergantung pada daftar yang diturunkan dari grammar tree-sitter. Formatter W12-T1 (di `qh-sql`, P-10) memakai daftar kata klausa miliknya sendiri. |

## 3. Grammar: vendoring dan pemeliharaan

**Isi `crates/qh-sql-grammar/`:**

- `Cargo.toml`: `license = "MIT"`, `links = "tree-sitter-sql"`, dependensi `tree-sitter-language` dan build-dependency `cc` dari workspace (1.4.7, sehingga tidak ada `cc` kedua).
- `build.rs`: `cc::Build::new().std("c11").include("vendor")`, dengan berkas `vendor/parser.c` dan `vendor/scanner.c`. `MACOSX_DEPLOYMENT_TARGET` dipaksa oleh `.cargo/config.toml` (invariant #4), jadi objek C mengikuti 14.0 seperti SQLite yang di-bundle.
- `src/lib.rs`: `pub const LANGUAGE: LanguageFn`. `#![deny(unsafe_code)]`, dengan `#[allow(unsafe_code)]` hanya di blok `extern "C"` dan `from_raw`.
- `vendor/parser.c`, `vendor/tree_sitter/{parser.h,alloc.h,array.h}`: salinan byte demi byte dari crate 0.3.11.
- `vendor/scanner.c`: salinan crate ditambah patch PR #361 (hapus `free` di `serialize`; `free` sebelum menimpa di `deserialize`; periksa `malloc`). Diff-nya disimpan sebagai `vendor/scanner.patch`.
- `grammar.js` dan `node-types.json`: sumber untuk membangkitkan ulang dan untuk tes nama node.
- `LICENSE` (MIT, Derek Stride) dan `PROVENANCE.md`: versi crate, checksum `.crate`, URL dan commit PR #361, versi runtime yang diuji, lisensi ICU di runtime, dan prosedur pembaruan.
- `tests/scanner_leak.rs`: `tree_sitter::set_allocator` dengan penghitung alokasi hidup. Sesudah 10.000 edit di badan `$body$` dan semua objek dibuang, hitungannya kembali ke awal. Ini satu-satunya berkas tes yang memakai `unsafe`.

**Di `Cargo.toml` root (koreksi AR-TS):**

- `cc = "1.4"` di `[workspace.dependencies]` (hari ini `cc` hanya transitif, 1.4.7 di `Cargo.lock`). Runtime `tree-sitter` 0.26.13 meminta `cc ^1.2.48`, jadi satu versi `cc` melayani keduanya.
- `[profile.dev.package.qh-sql-grammar]` dan `[profile.dev.package.tree-sitter]` dengan `opt-level = 2`. `cc` membaca `OPT_LEVEL` per paket; tanpa ini `parser.c` dikompilasi `-O0` di build debug, dan anggaran G2 "< 30 dtk di debug" diukur terhadap parser yang beberapa kali lebih lambat dari yang dikirim.
- MSRV: `tree-sitter` 0.26.13 dan `tree-sitter-language` 0.1.7 menyatakan `rust-version = "1.77"`, jadi cocok dengan 1.85; 0.1.8 menyatakan 1.90 (diperiksa di registry lokal). **`rust-version = "1.85"` tidak diverifikasi siapa pun**: tidak ada `rust-toolchain.toml`, CI memasang `stable`, dan mesin ini memakai rustc 1.98.1. Resolver `"2"` tidak sadar MSRV, jadi patok `=` adalah satu-satunya penahan. Menaikkan MSRV untuk tree-sitter 0.27 adalah keputusan ADR tersendiri, bukan bagian 4B.

**Pembaruan grammar** adalah tugas tersendiri dengan gate: jalankan ulang uji dialek §1.2, diferensial §1.5, bench §1.3, dan ukuran biner §1.7. Pembaruan ke `main` upstream hanya bila ukuran bisa ditekan, misalnya dengan membangkitkan ulang dari `grammar.js` dengan CLI yang dipatok. Fork grammar (misalnya untuk `:name` atau sintaks Trino) **tidak** direncanakan. View §4.3 dan lexer celah §4.5 menutup kebutuhan warna tanpa fork.

## 4. Arsitektur `qh-editor`

Semua tipe di bagian ini murni: tanpa thread dan tanpa global selain tabel `const`. Konkurensi dirakit di `qh-ffi` (§6, §7).

### 4.1 `TextBuffer`

Sama dengan blueprint lama §5.1, dengan tiga penyederhanaan:

- Cermin UTF-8 (`Arc<String>`, CoW), indeks baris LF (`starts_utf16`, `starts_byte`), indeks potongan UTF-16 ↔ byte setiap ≤ 1 KiB, revisi, dan log (`Edit`, `Touch`, `Applied`).
- `last_closer` dan `closers_inserted` dihapus. Pembalikan kutip ditangani resinkronisasi statement (§4.2), bukan oleh lexer.
- `replace` di main: validasi batas char, `replace_range`, geser indeks, dorong `LogEntry::Edit`. Anggaran < 0,5 ms di 2 MB; terukur ≤ 0,02 ms p99 tanpa beban (§1.3).
- **Biaya CoW di main (koreksi AR-TS).** Angka di atas diukur tanpa snapshot hidup. Selama `paint` di latar memegang `Arc<String>`, `replace` di main harus menyalin seluruh 2 MB (`Arc::make_mut`), dan itu terjadi di sebagian besar ketikan cepat. Bench W3-T2 mencatat `replace` p50/p99 dengan snapshot hidup di 2M. Bila p99 > 0,5 ms, W3-T2 mengganti cermin dengan buffer berpotongan (misalnya potongan 64 KiB dengan `Arc` per potongan) tanpa mengubah API, dan mencatatnya di laporan.

### 4.2 Statement dari mesin `scan.rs`

- **Refaktor `scan.rs` (dari blueprint lama §4.1, tetap dengan gate SEC).** Loop `scan()` diekstrak menjadi `pub fn walk(bytes, &mut impl Visitor) -> EndState`. Visitor menerima `separator(pos)`, `opaque(kind, start, end)` untuk kutip `'`, `"`, backtick, komentar, dan `$tag$`, serta `word(start, end)`. `scan()` tetap mengumpulkan hasil yang identik, dikunci oleh tes diferensial `scan_refactor.rs` dengan salinan fungsi lama.
- **Batas Safe Mode (koreksi AR-TS).** `scan()` adalah masukan tunggal `classify`, `statements_with_lines`, `decisions`, `check_confirmed`, `wrap.rs`, dan deteksi multi-statement driver PostgreSQL (`qh-driver-postgres/src/lib.rs:910`). W3-T2 **tidak boleh** mengubah perilaku engine sedikit pun. Syarat refaktornya:
  - salinan beku fungsi lama hanya ada di `tests/scan_refactor.rs`, tidak di `src/`;
  - yang dibandingkan adalah **seluruh** `Scan` (`separators`, `ends_with_terminator`, `leading_keyword`, `keywords`), ditambah `statement_count`, `statements_with_lines`, dan `decisions` untuk keempat `SafeMode`, bukan hanya pemisah;
  - korpus: seed `deploy/dev/*.sql`, `tools/golden/live_cases.py`, korpus bench, dan fixture `qh-sql` yang ada; acak: SplitMix64 berbenih dengan alfabet `'`, `''`, `"`, `""`, backtick, `--`, `/*`, `*/`, `$`, `$$`, `$tag$`, `$1`, `;`, LF, CR, `\`, `#`, `_`, huruf, angka, `é`, 😀; 10.000 kasus di debug dan ≥ 1.000.000 dengan `QH_SCAN_SOAK=1` di rilis, dijalankan sekali oleh W3-T2 dan dicatat;
  - tes unit `scan.rs` yang ada tidak diubah satu baris pun;
  - `walk` tanpa `unsafe` dan tanpa indeks yang bisa panic (setiap `bytes[i]` di belakang pemeriksaan batas, seperti hari ini);
  - `scan()` di `chars-2m` dan `bench-2m` tidak lebih lambat dari 10% (angka sebelum dan sesudah di laporan);
  - satu-satunya perubahan API `qh-sql` selain `walk` dan `lex` adalah `first_significant` menjadi `pub` (dibutuhkan `qh-editor` untuk aturan `statements_with_lines`).
  Perbaikan escape MySQL dan komentar `#` **bukan** bagian W3-T2; itu W3-T0 (§15), dan W3-T2 langkah 2 dimulai sesudah W3-T0 mendarat, supaya salinan beku di `scan_refactor.rs` membekukan pemindai yang sudah benar.
- **Daftar statement** `Vec<Stmt>` terurut. Statement ke-i adalah `[pemisah_(i-1) + 1, pemisah_i + 1)`: `;` ikut di dalam statement, supaya pohonnya melihat terminator. Potongan terakhir sampai EOF. Setiap `Stmt` menyimpan awal byte dan awal UTF-16 (digeser per edit, O(jumlah statement)), pohon opsional, token, lipatan, masalah, dan `giant: bool`.
- **Rentang untuk UI** (band, run mark, rotor, `sql_statement_ranges`) adalah `[pemisah_(i-1) + 1, pemisah_i)`, tanpa `;`, dengan aturan `statements_with_lines`: potongan dipertahankan hanya bila `first_significant` ada. Bentuknya sama dengan blueprint lama §4.1, termasuk tabel perbedaan terhadap `sqlStatements` Swift hari ini (`"a;b"`, `$tag$`, potongan komentar saja, CRLF, NBSP).
- **Resinkronisasi sesudah edit.** Jalankan `walk` dari awal statement yang memuat awal edit. Berhenti di pemisah pertama yang (1) terletak sesudah ujung edit dan (2) sama dengan batas lama yang sudah digeser. Statement lama dari titik itu dipakai ulang. Alasannya sama dengan konvergensi blueprint lama §5.5: pemisah hanya muncul di state Normal, dan teks sesudahnya tidak berubah. Kutip yang membalik pasangan bisa membuat jalan ini sampai EOF, dengan batas atas `scan()` penuh 2–10 ms di 2M, di latar.
- Statement yang teksnya berubah tetapi batasnya tetap diberi `tree.edit` dengan `InputEdit` relatif terhadap awal statement. Statement baru hasil pecah atau gabung diparse dari nol saat dibutuhkan.

### 4.3 View parse `:name`

`ParseView::bytes(stmt_text) -> Cow<[u8]>`: salin hanya bila ada `:` yang diganti. Aturannya:

- byte `:` di i, byte i+1 ∈ `[A-Za-z_]`, dan byte i−1 tidak ada atau ∉ `[A-Za-z0-9_:]` (koreksi AR-TS). Tanpa syarat "di dalam kode", karena penggantian di dalam string atau komentar tidak mengubah jenis token. Syarat byte i−1 mencegah view merusak slice yang parse bersih hari ini: dengan aturan lama `arr[lo:hi]` menjadi identifier `lo_hi` dan `arr[1:n]` menjadi `1_n`.
- Edit diperlebar satu byte di kiri dan kanan saat membentuk `InputEdit` (view byte i bergantung pada byte i−1, i, i+1, jadi ini tetap cukup).
- **Kelas `Parameter` datang dari lapis leksikal, bukan dari pohon (koreksi AR-TS).** Sesudah klasifikasi pohon dan celah, `classify` menimpa kelas di setiap `:name` yang lolos aturan `SQLScanner.swift:58-64,112-117`: di region kode `walk`, kedalaman `[`…`]` nol (dihitung dari awal statement), dan byte sebelumnya bukan `:`. Blueprint versi sebelumnya memberi kelas ke `identifier` yang byte aslinya `:`, dan itu keliru di dua arah: `arr[1:n]` akan diwarnai parameter padahal `SQLScanner` sengaja menganggapnya slice (`:58`), dan `x:a` tidak diwarnai padahal Run akan mengikatnya. Dengan lapis ini, warna parameter sama dengan yang diikat Run, dan tetap fungsi dari teks statement saja. Korpus `edge-params` mengunci `a::text`, `:=`, `arr[lo:hi]`, `arr[1:n]`, `arr[:n]`, `x:a`, `(:a)`, `':a'`, `-- :a`, `$$ :a $$`, dan `:a` di awal statement.

Dialek: tidak ada view kedua. Escape backslash dan komentar `#` MySQL mengikuti `walk`: sesudah W3-T0, dokumen berdialek `MySql` memakai mode MySQL `walk`, dan grammar tetap salah baca `'it\'s'` (ERROR, diwarnai lexer celah) (§14, §15).

### 4.4 Pohon per statement

- Satu `tree_sitter::Parser` per dokumen, di dalam `Analyzer`. Parse lewat `parse_with_options` dengan callback progres yang berhenti bila revisi terbaru (atomik yang ditulis `replace`) sudah melewati revisi yang sedang dianalisis.
- **Statement besar**: > 256 KiB → `giant = true`. Tidak ada pohon; token dari `qh_sql::lex`; tidak ada lipatan di dalamnya dan tidak ada masalah sintaks.
- **Token statement besar tidak di-cache (koreksi AR-TS).** `dump-2m` punya sekitar 400.000 token dalam satu statement; melex, menyimpan, dan membandingkan semuanya per ketikan berarti O(2 MB) kerja dan ~5 MB alokasi setiap giliran. Untuk statement `giant`, `paint` menjalankan `walk` dari awal statement sampai ujung jendela (terukur `scan()` 2,8 ms untuk 2 MB) dan hanya mengeluarkan token di dalam jendela. `dirty` untuk statement `giant` adalah rentang edit yang diperlebar ke batas token, ditambah sisa statement di dalam jendela bila `EndState` `walk` di ujung edit berubah (kutip atau komentar membalik). W3-T2 mencatat `paint` p50/p99 untuk ketikan di `dump-2m`; bila p99 > 8 ms, tambahkan checkpoint state `walk` setiap 64 KiB di dalam statement besar.
- **Cache**: pohon statement yang terlihat dan yang baru diedit dipertahankan. Sisanya dibuang dengan LRU bila jumlah byte sumber statement berpohon melewati ~640 KiB. Token, lipatan, dan masalah tetap disimpan. Memori di luar pohon (token sekitar 12 B, ditambah `Stmt` dan larik per statement) dicatat W3-T2 sesudah `paint` penuh `chars-2m` dan `bench-2m` (20.636 statement), dengan sasaran total analisis ≤ 48 MB untuk dokumen 2M.
- **Konvergensi**: statement yang pohon inkrementalnya `has_error()` diparse ulang dari nol 500 ms sesudah edit terakhirnya, di antrean idle. Token statement itu dihitung ulang dan selisihnya masuk `dirty` (koreksi AR-TS, §1.5), jadi warna akhir selalu sama dengan parse baru; lipatan, masalah, dan alias juga bisa berubah.
- **Warna sementara sebelum parse selesai.** Pewarnaan pertama sinkron (§7.6) dan prefiks jendela (butir berikut) boleh menghasilkan warna yang bukan warna akhir. Itu hanya sah bila parse lengkapnya dijadwalkan di idle dan hasilnya lewat jalur selisih yang sama. G2 membandingkan warna sesudah antrean idle dikuras (§9).
- **Jendela untuk statement yang sangat panjang dan baru**: bila statement yang harus diparse lebih besar dari jendela, jalur terlihat boleh memparse awalannya sampai ujung jendela saja, dan pohon lengkapnya diparse di idle.

### 4.5 Klasifikasi dan lexer cadangan

Jalan pohon menurun hanya ke node yang beririsan dengan rentang yang diminta. Node atomik (tidak dimasuki): `literal`, `identifier`, `comment`, `marginalia`, `parameter`, `op_other`, `op_unary_other`, `dollar_quote`, dan semua `keyword_*`. Tabel kelas ada di §5.1.

**Celah ERROR**: untuk setiap node ERROR, byte di antara anak-anaknya, sebelum anak pertama, dan sesudah anak terakhir diberikan ke `qh_sql::lex`. Lexer ini (modul baru di `qh-sql`, di atas `walk`) mengeluarkan:

- region opaque dari `walk` dengan batas **dipotong ke celah**: komentar, string, identifier berkutip, dan dollar-quote. Pembuka tanpa penutup menjadi token sampai ujung celah;
- di region kode: angka, `Word` tanpa kelas, `:name`, dan lari tanda baca.

`qh-editor` memberi kelas ke `Word`: kata kunci bila ada di himpunan kata kunci grammar ∪ 131 kata `SQLSyntax.keywords`; Literal untuk `null`/`true`/`false`; selain itu tanpa kelas (koreksi AR-TS, D-21). Lexer yang sama dipakai untuk statement besar, formatter W12-T1, dan cadangan alias W11-T6. Himpunan kata kunci diambil dari `Language::node_kind_for_id` (awalan `keyword_`) sekali saat start, digabung dengan daftar tetap `qh_editor::keywords::EXTRA_KEYWORDS`.

### 4.6 Himpunan kotor, selisih, dan hasil cat

`dirty: RangeSet<u32>` dalam UTF-16, dengan koordinat revisi analisis. Semantiknya sama dengan blueprint lama §5.6:

- `Edit`: geser, lalu **selalu** tambahkan `[s, s + new_len)`;
- statement yang diparse ulang: token lama (digeser) dan baru dibandingkan dengan pemangkasan prefiks dan sufiks, dan sisanya ditambahkan;
- statement yang batasnya berubah: seluruh rentangnya ditambahkan;
- `Touch` menambah, dan `Applied { revision, ranges }` mengurangi hanya bila revisinya cocok di titik log itu.

**Hasil `paint`** untuk `dirty ∩ jendela`, dibatasi anggaran:

- `ranges`: rentang yang harus dicat ulang Swift, di-snap ke batas token;
- `runs`: token berkelas di dalam rentang itu. Yang tidak disebut berarti tanpa warna sementara;
- `fonts`: rentang yang font storage-nya harus ditulis, dengan penanda italic. Isinya hanya rentang edit (font storage-nya tidak diketahui Rust, karena undo bisa memulihkan italic lama) dan tempat status komentar berubah. Saat mengetik biasa, `fonts` kosong atau berisi satu rentang satu karakter.

### 4.7 Lipatan

Dari pohon statement, hanya yang mencakup lebih dari satu baris:

- `Statement`: statement non-kosong (header = baris pertama yang signifikan, ringkasan = kata kunci pertama dalam huruf besar atau `"statement"`);
- `Cte`: badan berkurung node `cte` (ringkasan `"CTE"`);
- `Subquery`: node `subquery` (ringkasan `"subquery"`);
- `Body`: isi di antara dua node `dollar_quote` (ringkasan `"body"`).

Statement `giant` atau yang pohonnya dibuang dan belum diparse ulang memakai lipatan terakhir yang di-cache, digeser. Deduplikasi dan urutan `(header, lastLine)` sama dengan `SQLFolding` hari ini.

### 4.8 Masalah (data untuk W10-T6)

| Jenis | Sumber | Ditampilkan W10-T6 |
|---|---|---|
| `UnclosedQuote`, `UnclosedComment`, `UnclosedDollar`, `UnclosedIdentifier` | state akhir `walk` per statement (pembuka yang benar-benar diketik) | ya |
| `UnbalancedParen` | hitungan kurung di region kode `walk` per statement | ya |
| `SyntaxError` (ERROR), `MissingToken` (MISSING) | pohon statement, sesudah konvergensi §4.4 | **tidak** secara default (D-12) |

### 4.9 Plafon

`CEILING_UTF16 = 2_000_000`, inklusif, seperti blueprint lama §5.7. Di atas plafon tidak ada warna (Swift menghapus warna sementara bertahap), tidak ada pohon dan lipatan, tetapi statement tetap dihitung. `bench-2m.sql` berukuran 2.000.010 unit, jadi fixture bench `type-2m` harus dipangkas ≤ 2.000.000 sebelum NFR-P5 diukur.

## 5. Kelas, warna, dan perubahan tampilan (V-12)

### 5.1 Pemetaan node → kelas → warna

Warna adalah pasangan `SQLSyntax` yang sudah ada (`Views/SQLSyntax.swift:202-210`), dan tidak ada warna baru. Kode kelas adalah enum tertutup di dua sisi batas (invariant #3), dengan `default` yang memicu fault di Swift.

| Node grammar 0.3.11 | Kelas (kode) | Token warna (gelap / terang) | Hari ini (regex) |
|---|---|---|---|
| `comment` (`--`), `marginalia` (`/* */`) | Comment (1), italic di storage | `comment` 5A6072 / 5F6672 | sama, kecuali `;--`, `)--`, `+--`, `)/*` yang hari ini menjadi tanda baca |
| `literal` diawali `'`, `E'`, `N'`, `U&'`, `B'`, `X'`, `$` | String (2) | `string` 3EE6A8 / 0A7A52 | `'…'` sama; `$tag$…$tag$` hari ini dilex sebagai kode; `E` di `E'…'` hari ini tanpa warna |
| `dollar_quote` (tag badan fungsi) | String (2) | `string` | hari ini `tag$` kata biasa |
| Badan fungsi `$body$ … $body$` | kelas SQL biasa | — | hari ini kode juga, tetapi dengan keanehan regex |
| `literal` literal bertipe (`date '…'`, anak `identifier` + string) | identifier → Keyword (5), sisanya String (2) | `keyword`, `string` | `date` tanpa warna |
| `literal` diawali angka, `.`, atau tanda | Number (4) | `number` FFB547 / 9A5B00 | sama, kecuali `$1`, `1abc`, `1.5a` |
| `literal` diawali `"` | QuotedIdentifier (3); String (2) untuk dialek MySQL | `quotedIdentifier` E8C468 / 7A5C00 | selalu `quotedIdentifier` |
| `keyword_null`, `keyword_true`, `keyword_false` | Literal (6) | `literal` FF7A8A / BE2F45 | sama |
| `keyword_*` lain, termasuk tipe (`int`, `varchar`, `text`, `timestamp`, `jsonb`, …) | Keyword (5) | `keyword` 8B7BFF / 5B3FD6 | hanya 131 kata; tipe tanpa warna, atau `function` bila diikuti `(` |
| `identifier` sebagai `name` dari `object_reference` di dalam `invocation` | Function (7) | `function` 4FD8FF / 0B6E8F | kata bukan kata kunci yang diikuti spasi lalu `(` |
| `identifier` diawali `"` atau backtick | QuotedIdentifier (3) | `quotedIdentifier` | sama |
| `parameter` (`?`, `$1`) dan `:name` dari lapis leksikal §4.3 | Parameter (9) | **`literal`** | `?` dan `:` tanpa warna; `1` di `$1` menjadi angka |
| Token anonim operator dan tanda baca (`( ) , ; . [ ] = < > <= >= != <> + - * / % ^ :: \|\| :=`), `op_other`, `op_unary_other` (`->`, `->>`, `#>`, `@>`, …) | Punctuation (8) | `punctuation` 8A90A6 / 565C6B | lari `[-+*/%=<>!\|,;().\[\]]+`; `:`, `@`, `#` tanpa warna |
| `identifier` lain (kolom, tabel, alias, skema) | tanpa kelas | `base` | sama, kecuali kata di daftar 131 |
| Celah ERROR, statement besar | dari `qh_sql::lex` (§4.5) | sesuai kelas | — |

Parameter memakai warna `literal` karena parameter dibaca sebagai nilai, seperti `null`, dan karena kandidat warna baru dari kosakata `Tone` gagal di kanvas Nord: `Tone.magenta` FF4FA3 4,10:1, `Tone.blue` 4F8DFF 3,92:1. Kelasnya tetap terpisah di FFI supaya UX bisa memberinya warna sendiri tanpa perubahan Rust.

### 5.2 Kontras (dihitung WCAG 2.x dari nilai heksadesimal)

| Token | Midnight | Graphite | Nord | Ink | Daylight | Cloud | Paper |
|---|---|---|---|---|---|---|---|
| base | 16,19 | 15,18 | 10,40 | 16,78 | 15,25 | 15,15 | 15,55 |
| keyword | 5,91 | 5,54 | **3,79** | 6,12 | 6,21 | 6,17 | 6,33 |
| function | 11,67 | 10,94 | 7,49 | 12,09 | 5,33 | 5,30 | 5,44 |
| string | 12,12 | 11,37 | 7,78 | 12,56 | 4,96 | 4,92 | 5,05 |
| number | 11,07 | 10,38 | 7,11 | 11,47 | 5,02 | 4,98 | 5,12 |
| quotedIdentifier | 11,60 | 10,88 | 7,45 | 12,02 | 5,78 | 5,74 | 5,90 |
| literal (juga Parameter) | 7,78 | 7,30 | 5,00 | 8,06 | 5,27 | 5,24 | 5,38 |
| comment | 3,10 | 2,91 | 1,99 | 3,22 | 5,35 | 5,31 | 5,45 |
| punctuation | 6,13 | 5,75 | **3,94** | 6,35 | 6,19 | 6,14 | 6,31 |

Aturan palet (`app/DESIGN.md` §Appearance: setiap nilai terang ≥ 4,5:1 di Daylight, setiap nilai gelap ≥ 4,5:1 di Midnight, `comment` gelap sengaja di bawahnya) terpenuhi, dan 4B tidak menambah warna. **Temuan yang sudah ada sebelum 4B:** NFR-A1 menuntut ≥ 4,5:1 di ketujuh kanvas, sedangkan `keyword` (3,79) dan `punctuation` (3,94) gagal di Nord. Memperbaikinya berarti nilai gelap per kanvas atau menaikkan kecerahan untuk semua kanvas gelap. **Diputuskan orkestrator (30 Sep 2026): diperbaiki di dalam V-12**, karena baseline editor direkam ulang di commit yang sama dan NFR-A1 berlaku untuk ketujuh kanvas. Nilai barunya dipilih W4-T2 commit B dengan gate UX, dengan syarat: `keyword` dan `punctuation` ≥ 4,5:1 di keempat kanvas gelap, nilai terang tidak berubah, dan hue tetap dikenali sebagai ungu dan abu-abu. Bila palet hanya membedakan gelap dan terang (bukan per kanvas), nilai gelap yang dinaikkan berlaku untuk keempat kanvas gelap. `SyntaxPaletteTests` (baru, §9) mengunci ≥ 4,5:1 untuk semua token di ketujuh kanvas, kecuali `comment` gelap.

### 5.3 V-12: perubahan tampilan yang didaftarkan

Baris baru untuk PRD §6.5 (diisi orkestrator): **V-12 — Warna sintaks dan lipatan editor dari tree-sitter; warna sebagai atribut sementara — W4-T2 commit B.** Isinya:

1. Kata kunci mengikuti grammar dan konteks: tipe data menjadi ungu; kata yang dipakai sebagai nama kolom menjadi warna dasar bila grammar melex-nya sebagai identifier; kata di luar grammar (`describe`, `grant`, `fetch`, …) tetap ungu di celah ERROR lewat daftar cadangan.
2. Deteksi fungsi mengikuti `invocation`: `INSERT INTO t (a)` tidak lagi mewarnai `t` sebagai fungsi, dan `varchar(20)` menjadi kata kunci.
3. Dollar-quote biasa menjadi string. Badan fungsi diwarnai sebagai SQL, dan tagnya sebagai string.
4. Komentar sesudah tanda baca (`1;-- c`, `count(*)--c`, `)/* x */`) menjadi komentar.
5. Parameter `:name`, `?`, dan `$1` memakai warna literal.
6. `::`, `->>`, `#>`, `@>`, dan `:=` menjadi abu-abu tanda baca.
7. Kutip atau komentar yang belum ditutup mewarnai sisa statement (menurut `scan.rs`) sebagai string atau komentar. Hari ini karakter pembukanya dilewati dan sisanya diwarnai sebagai kode.
8. Penanda lipatan bertambah: CTE, subquery, dan badan `$tag$` yang lebih dari satu baris.
9. Lapis atribut G-VIS: warna pindah dari run storage ke run `temporary: true`. Run storage tinggal dasar dan italic komentar.
10. `keyword` dan `punctuation` gelap dinaikkan sampai ≥ 4,5:1 di Nord (§5.2, keputusan orkestrator).
11. Warna parameter (butir 5) mengikuti aturan `SQLScanner` persis: `:n` di dalam `[`…`]` (slice) tidak berwarna parameter, sedangkan `x:a` berwarna parameter karena Run mengikatnya (§4.3).

Scene editor yang direkam ulang: `editor-syntax`, `-folded`, `-plain`, `-find`, `-invisibles`, `-wrap-on`, `-wrap-off`, dan `-long-middle`, masing-masing gelap dan terang (16 pasang PNG dan JSON). Pasangan lama dan baru ditinjau berdampingan oleh pemilik di laporan akhir (P-01).

## 6. API UniFFI (`crates/qh-ffi/src/editor.rs`)

```rust
#[derive(uniffi::Enum)] pub enum EditorDialect { Generic, Postgres, MySql, Trino }

#[derive(uniffi::Object)]
pub struct EditorDocument {
    text: Mutex<TextBuffer>,       // main: replace, mark_*, len; latar: snapshot + kuras log. Kunci daun.
    analysis: Mutex<Analyzer>,     // hanya thread latar
    latest: AtomicU64,             // revisi terbaru, dibaca callback progres parse (§4.4)
}

#[uniffi::export]
impl EditorDocument {
    #[uniffi::constructor]
    pub fn new(text: String, dialect: EditorDialect) -> Result<Arc<Self>, EditorError>;
    pub fn revision(&self) -> Result<u64, EditorError>;
    pub fn len_utf16(&self) -> Result<u32, EditorError>;
    /// Main thread. O(edit) + geser indeks. Tidak pernah memparse.
    pub fn replace(&self, start_utf16: u32, len_utf16: u32, text: String) -> Result<u64, EditorError>;
    /// Thread latar. Resinkronisasi statement, reparse, dan selisih untuk dirty ∩ jendela, dibatasi anggaran.
    pub fn paint(&self, revision: u64, window_start: u32, window_len: u32,
                 budget_utf16: u32) -> Result<EditorPaint, EditorError>;
    /// Main thread. `ranges` = pasangan (start, len) dari `EditorPaint.ranges` yang sudah diterapkan.
    pub fn mark_applied(&self, revision: u64, ranges: Vec<u32>) -> Result<(), EditorError>;
    /// Main thread. Ganti font kode dan pemulihan.
    pub fn mark_dirty(&self, start_utf16: u32, len_utf16: u32) -> Result<(), EditorError>;
    /// Thread latar, sesudah debounce. Statement, lipatan, dan masalah.
    pub fn outline(&self, revision: u64) -> Result<EditorOutline, EditorError>;
    pub fn line_count(&self) -> Result<u32, EditorError>;   // deteksi drift
}
#[uniffi::export] pub fn editor_ceiling_utf16() -> Result<u32, EditorError>;
/// Tanpa state: rentang statement §4.2 untuk teks apa pun (W4-T2b). `dialect` diabaikan sampai
/// W3-T0 memberi `walk` mode MySQL; sesudah itu wajib sama dengan dialek yang dipakai engine (D-20).
#[uniffi::export] pub fn sql_statement_ranges(sql: String, dialect: EditorDialect) -> Result<Vec<u32>, EditorError>;

#[derive(uniffi::Record)] pub struct EditorPaint {
    pub revision: u64, pub doc_len_utf16: u32, pub window_start: u32, pub window_len: u32,
    pub inactive: bool,          // di atas plafon: tanpa runs, rentang dikembalikan ke dasar
    pub more_in_window: bool, pub dirty_elsewhere: bool,
    pub ranges: Vec<u32>,        // pasangan (start, len), terurut, tidak beririsan
    pub runs: Vec<u32>,          // tripel (start, len, class 1…9), masing-masing utuh di dalam satu rentang
    pub fonts: Vec<u32>,         // tripel (start, len, italic 0|1), di dalam rentang
}
#[derive(uniffi::Record)] pub struct EditorOutline {
    pub revision: u64,
    pub statements: Vec<u32>,    // pasangan (start, end), aturan statements_with_lines
    pub folds: Vec<EditorFold>,
    pub issues: Vec<EditorIssue>,
}
#[derive(uniffi::Record)] pub struct EditorFold { pub kind: EditorFoldKind, pub header_line: u32, pub last_line: u32,
    pub header: u32, pub body_start: u32, pub body_end: u32, pub summary: String }
#[derive(uniffi::Enum)] pub enum EditorFoldKind { Statement, Cte, Subquery, Body }
#[derive(uniffi::Record)] pub struct EditorIssue { pub kind: EditorIssueKind, pub start: u32, pub len: u32 }
#[derive(uniffi::Enum)] pub enum EditorIssueKind { UnclosedQuote, UnclosedIdentifier, UnclosedComment,
    UnclosedDollar, UnbalancedParen, SyntaxError, MissingToken }
```

**Semantik yang mengikat** (sisanya sama dengan blueprint lama §6.1–§6.4):

- `revision` mulai 1 dan naik satu per `replace`. `Stale` bila argumen revisi lebih kecil dari revisi saat panggilan masuk. Hasil selalu membawa revisi tempat ia dihitung.
- Urutan kunci: `analysis → text` saja. `text` adalah kunci daun. Main hanya mengambil `text`.
- **Invarian hasil** (dikunci tes Rust dan divalidasi Swift di latar): larik genap atau kelipatan tiga; terurut; di dalam `[0, doc_len_utf16]`; setiap run dan font utuh di dalam satu rentang; kelas 1…9.
- Galat: `EditorError { Stale, OutOfBounds, SplitsCharacter, TooLarge, Malformed }`. Semua ekspor mengembalikan `Result`. Tidak ada `unwrap` pada jalur input, dan mutex poisoned menjadi `Malformed`.
- **Contoh yang dikunci tes W3-T2.** `new("select :a -- c", Generic)`, lalu `paint(1, 0, 14, u32::MAX)` → `revision = 1`, `doc_len_utf16 = 14`, ketiga flag `false`, `ranges = [0, 14]`, `runs = [0,6,5, 7,2,9, 10,4,1]`, dan `fonts = [0,10,0, 10,4,1]`. Pada dokumen baru seluruh teks dianggap rentang edit, jadi `fonts` mencakup semuanya. Sesudah `mark_applied(1, [0, 14])` dan `replace(14, 0, "x")` (revisi 2, teks `select :a -- cx`), `paint(2, 0, 15, u32::MAX)` hanya mengembalikan token komentar yang memanjang: `ranges = [10, 5]`, `runs = [10, 5, 1]`, `fonts = [14, 1, 1]`.
- `EditorDocument` bukan perintah engine, jadi invariant #11 tidak tersentuh. Invariant #1 berlaku (`app/Generated/` diregenerasi bersama Rust).

## 7. Threading dan penerapan di Swift

### 7.1 Antrean

Sama dengan blueprint lama §7.1–§7.2:

- `qh.editor.visible` (`.userInitiated`, serial, dikoalesi): `paint` untuk jendela ±2 layar, anggaran 131.072;
- `qh.editor.idle` (`.utility`, serial): `paint` sisa (16.384 per giliran), `outline` sesudah debounce, dan parse ulang konvergensi.

### 7.2 Alur satu ketikan

```
main   textStorage(_:didProcessEditing:) (.editedCharacters)
         ├─ indeks ruler 4A, lalu doc.replace(start, oldLen, newText)     ≤ 0,02 ms (2M, §1.3)
         ├─ geser band, run mark, header lipatan (O(k))
         └─ catat rentang edit terakhir
main   textDidChange
         ├─ warisi warna tetangga untuk rentang edit (§7.5)                O(1)
         └─ requestVisible()
visible pkt = doc.paint(rev, jendela, 131_072)                              reparse 0,02–0,09 ms; sisanya [belum diukur]
         └─ validate(pkt) → main.async { apply(pkt) }
main   apply bila pkt.revision == revision && !hasMarkedText && docLen == storage.length
         ├─ lm.removeTemporaryAttribute(.foregroundColor, r) untuk setiap rentang
         ├─ lm.addTemporaryAttribute(.foregroundColor, SQLSyntax.colour(class), r) untuk setiap run
         ├─ bila fonts tidak kosong: storage.beginEditing; addAttribute(.font, italic|tegak); endEditing
         └─ doc.markApplied(rev, ranges); jadwalkan idle bila more_in_window || dirty_elsewhere
idle   (debounce) doc.outline(rev) → main.async { band, run mark, lipatan, rotor, masalah }
```

Hook `didProcessEditing` mengabaikan edit yang hanya mengubah atribut. Atribut sementara tidak lewat storage, jadi tidak memicu hook itu sama sekali.

### 7.3 Aturan penerapan

- **Storage** hanya diubah di tiga tempat: saat dokumen dibuat (`setAttributes(base)` seluruh dokumen, seperti `paintWhole` hari ini), saat font kode atau lebar tab berganti (tulis ulang seluruh dokumen, lalu `mark_dirty(0, len)`), dan di `fonts`. Tidak pernah saat mengetik, kecuali satu rentang edit.
- **Warna** hanya lewat `addTemporaryAttribute` dan `removeTemporaryAttribute` dengan kunci `.foregroundColor`. `setTemporaryAttributes` dilarang, karena ia mengganti semua kunci, termasuk sorotan find.
- **Jangan menyentuh yang sudah benar (koreksi AR-TS).** Setiap `add`/`remove` meng-invalidate tampilan rentangnya, dan menggambar ulang satu baris berwarna berharga 4–5 ms (§1.6). `apply` membandingkan dulu dengan `temporaryAttribute(.foregroundColor, atCharacterIndex:longestEffectiveRange:in:)` dan melewati run yang warnanya sudah sama. Karena pewarisan tetangga (§7.5) biasanya sudah benar untuk ketikan di dalam kata, string, atau komentar, sebagian besar giliran `apply` tidak menggambar ulang apa pun. G7 menghitung invalidasi pada ketikan di tengah identifier dan mengharapkan nol.
- **`typingAttributes`** = dasar (12,5 pt tegak), dipasang sekali saat dokumen dibuat dan saat font berganti. Spike menunjukkan editor `isRichText = false` tidak menyalin atribut tetangga, jadi font karakter baru selalu 12,5 pt tegak dan tinggi baris tidak melompat. Italic untuk karakter yang diketik di dalam komentar datang bersama `apply`. Font monospace tegak dan italic sama lebarnya, jadi tidak ada geometri yang bergeser.
- **Warna dinamis.** Objek `NSColor(name:dynamicProvider:)` yang sama dipakai sebagai nilai atribut sementara. Pergantian appearance tidak butuh cat ulang (terverifikasi: piksel identik di gelap dan terang).
- **Pemeriksaan teks dimatikan.** Selain yang sudah dimatikan di `SQLEditor.swift:99-105`, tambahkan `isAutomaticLinkDetectionEnabled`, `isAutomaticDataDetectionEnabled`, `isAutomaticTextCompletionEnabled` bernilai `false`, dan `inlinePredictionType = .no`. Di spike ini menghemat 1,3 ms per gambar ulang baris. Tidak ada perubahan piksel.

### 7.4 Pemilik kunci atribut sementara

Satu kunci, satu pemilik. Tidak ada fitur yang menghapus kunci milik fitur lain.

| Kunci sementara | Pemilik | Catatan |
|---|---|---|
| `.foregroundColor` | 4B (warna sintaks) | |
| `.backgroundColor` | find bar (`SQLEditor.swift:1237`, `:1247`, sudah ada) | Sorotan find tergambar di bawah glyph yang berwarna sintaks. Keduanya terlihat, tanpa urutan menang-kalah karena kuncinya berbeda. |
| `.underlineStyle`, `.underlineColor` | W10-T6 (garis bawah diagnostik) | Kunci yang tidak memengaruhi layout, jadi diizinkan sebagai atribut sementara |
| — | W10-T7 (pasangan kurung) | Digambar di `SQLTextView.drawBackground(in:)` seperti band sorotan, bukan atribut sementara, supaya tidak berebut `.backgroundColor` dengan find |

G-VIS (`VisualParityTests.attributeRuns`, baris 958-1009) sudah merekam run sementara per rentang. Sesudah 4B, satu run sementara bisa membawa `NSColor` dan `NSBackgroundColor` sekaligus di dalam kecocokan find. Deskripsinya deterministik.

### 7.5 IME, undo, surrogate, pewarisan

- **IME**: seperti blueprint lama §7.5 (D-16 di sini). Edit dikirim selama `hasMarkedText`, `apply` dan penjadwal idle menunggu, dan akhir komposisi dideteksi dari transisi di `insertText`, `setMarkedText`, dan `unmarkText`. **[perlu verifikasi] (koreksi AR-TS)**: klaim "kuncinya berbeda" belum diuji. `markedTextAttributes` bisa memuat `.foregroundColor`, dan cara AppKit membersihkan atribut teks bertanda sesudah komposisi tidak didokumentasikan. Penahannya: rentang komposisi adalah rentang edit, jadi selalu kotor dan dicat ulang sesudah transisi. G7 menambah kasus: sesudah komposisi di-commit (dan sesudah dibatalkan), warna di rentang itu dan di kedua tetangganya sama dengan `paint` dokumen baru, dan sorotan find tetap ada.
- **Aksesibilitas.** Warna di storage hari ini ikut ke `AXAttributedString`; warna sementara tidak. VoiceOver tidak membacakan warna secara default, jadi ini bukan kemunduran fungsional, tetapi gate AX W4-T2 memeriksanya dan mencatatnya di ADR-0033.
- **Undo**: undo memutar edit storage lewat hook yang sama. Atribut sementara tidak ikut undo (terverifikasi), sedangkan italic storage ikut. Rentang edit selalu kotor dan selalu membawa `fonts`, jadi keduanya dicat ulang benar. Penerapan warna tidak pernah masuk tumpukan undo.
- **Surrogate dan CRLF**: seperti blueprint lama (pelebaran di Swift, `SplitsCharacter`, indeks baris hanya LF).
- **Pewarisan warna tetangga** (menggantikan pewarisan `typingAttributes` di blueprint lama §7.4). Di `textDidChange`, sesudah layout manager memproses edit, rentang sisipan diberi warna sementara karakter sebelumnya bila (a) warna itu `string` atau `comment`, atau (b) karakter sebelumnya bukan spasi dan teks sisipan tidak memuat spasi. Selain itu rentang dibiarkan dasar. Aturan ini mencegah kedipan saat mengetik di dalam string, komentar, atau di tengah kata, tanpa mewarnai kata baru sesudah kata kunci. Kesalahan paling lama bertahan satu giliran analisis.

### 7.6 Membuka, scroll, font

- Membuka dokumen (`makeNSView`, teks dari luar): `EditorDocument` baru. Dokumen ≤ 256 KiB: jendela terlihat dianalisis dan diterapkan sinkron sebelum frame pertama (parse statement terlihat 3–4 ms, §1.3). Lebih besar: asinkron dengan jendela lebih dulu, sehingga paling lama satu frame tanpa warna. `drainForTesting()` membuat G-VIS deterministik.
- Scroll: `boundsDidChange` → `requestVisible()`, paling banyak sekali per frame.
- Font kode dan lebar tab: tulis ulang storage seluruh dokumen, lalu `mark_dirty(0, len)`.

### 7.7 Jalan mundur

- **Bila atribut sementara ternyata tidak tergambar sama di suatu jalur** (misalnya cetak; `NSLayoutManager` hanya menggambar atribut sementara saat menggambar ke layar), penerapan boleh kembali ke atribut storage dengan data `EditorPaint` yang sama. Hanya rentang yang berubah yang ditulis, sehingga biayanya mendekati baris "storage" di §1.6, bukan cat ulang penuh ala 4A. Pencetakan editor bukan fitur hari ini.
- **Bila gate gambar §14 gagal** sesudah pengungkit murah, pemicunya adalah biaya gambar run berwarna (§1.6). Eskalasi yang tersedia adalah Fase 8 (`performance-plan.md` §12, butir kedua), CodeEditTextView upstream. Fork TablePro tetap dilarang. TextKit 2 kemungkinan besar membawa biaya storage yang sama, dan tidak dikejar. Yang diperiksa ulang AR (30 Sep 2026):
  - lisensi: `LICENSE.md` repo CodeEditApp/CodeEditTextView memang MIT, "Copyright (c) 2023 CodeEdit". **Tetapi paketnya membawa dependensi**: `Package.swift` di `main` menarik ChimeHQ/TextStory (BSD-3-Clause, "Copyright (c) 2020, Chime"), apple/swift-collections (Apache-2.0), dan plugin build `SwiftLintPlugin`. Semuanya masuk allow-list yang lazim, tetapi NFR-L menuntut tinjauan manual per paket SwiftPM, dan ini paket SwiftPM pertama app;
  - platform `.macOS(.v13)`, cocok dengan minimum 14.0;
  - **belum terbukti lebih murah**: CodeEditTextView juga menggambar baris lewat CoreText dengan warna per run, jadi biaya `CGColor` dan display list per run (§1.6) bisa ikut pindah. Eskalasi hanya boleh diajukan ke pemilik sesudah spike buangan di `target/run/` mengukur gambar ulang satu baris berwarna (pola `type-coloured-195k`) di CodeEditTextView dan hasilnya < 50% angka `SQLEditor`;
  - ongkos penulisan ulang tetap seperti `performance-plan.md` §12: find, fold, ruler, run mark, completion, auto-uppercase, dan `interceptKey`.

## 8. Titik kait

### 8.1 Rotor (FR-ED-09), W4-T2

Seperti blueprint lama §8.1: `EditorRotorSource`, dan klien pertama rotor "Statements" dari `EditorOutline.statements`.

### 8.2 Diagnostik (FR-ED-06, W10-T6)

- W10-T6 membaca `EditorOutline.issues` berjenis leksikal (§4.8) dan posisi galat server, menggambar garis bawah dengan kunci `.underlineStyle`/`.underlineColor` (§7.4), dan menambah rotor "Query issues" sebagai `EditorRotorSource` kedua.
- `SyntaxError` dan `MissingToken` tidak digambar. Menampilkannya, misalnya hanya untuk dialek PostgreSQL atau di balik setelan, adalah keputusan pemilik (§15) dengan angka positif palsu dari korpus §1.2 sebagai dasarnya.
- W10-T6 juga yang menyambungkan dialek per tab ke `EditorDocument::new` (D-20).

### 8.3 Referensi tabel dan alias (FR-ED-05, W11-T6)

- `crates/qh-editor/src/refs.rs` (W11-T6): pada statement di bawah caret, kumpulkan `relation` (`object_reference` dengan skema dan katalog, `alias`), nama `cte`, dan alias `term`. Bila statement ber-ERROR, gunakan cadangan leksikal: pola `FROM|JOIN nama [AS] alias` di atas `qh_sql::lex`.
- Diekspor sebagai `EditorDocument::references(revision, offset)`. Tidak menambah kerja per ketikan; dihitung saat diminta, di antrean latar.

### 8.4 Formatter (FR-ED-01, P-10, W12-T1)

- Token: region opaque dari `walk` (kutip, komentar, `$tag$`) diambil byte demi byte, dan region kode dari `qh_sql::lex` (kata, angka, tanda baca, `:name`). Keduanya engine-konsisten dan tidak pernah ERROR.
- Tata letak: kata kunci klausa dan kedalaman kurung dari aliran token yang sama. Pohon tidak dipakai di W12-T1. Bila kelak dipakai untuk kerapian, hanya di statement tanpa ERROR, dan itu keputusan rancangan W12-T1 dan AR-nya.
- Formatter memeriksa sendiri hasilnya sebelum mengembalikannya: deretan karakter non-spasi di luar region opaque dan seluruh isi region opaque harus identik. Bila tidak, ia menolak. Ia juga menolak bila `walk` berakhir di luar state Normal (kutip terbuka).
- Catatan untuk PRD P-10: "di atas mesin `scan.rs` (`walk`) dan lexer kode `qh-sql`", bukan "di atas lexer 4B" atau "di atas pohon".

### 8.5 Pasangan kurung (FR-ED-08, W10-T7)

`EditorDocument::bracket_pair(revision, offset)` (W10-T7) boleh memakai pohon statement (anak `(`/`)` dari node yang sama) dengan cadangan hitungan kurung dari `walk`. Penggambaran lewat `drawBackground` (§7.4). Karena ini menambah ekspor FFI, W10-T7 masuk rantai lane FFI (`crates/qh-ffi/src/editor.rs`, `app/Generated/`) dan memegang `crates/qh-editor/src/brackets.rs` (koreksi AR-TS).

## 9. Strategi uji

Paritas terhadap regex hilang bersama regex. Penggantinya:

| Lapis | Di mana | Isi | Kapan |
|---|---|---|---|
| G1 fixture capture emas | `crates/qh-editor/tests/golden.rs`, `tests/fixtures/corpus/*.sql` → `*.classes` | Untuk setiap berkas korpus: baris `start len class` (UTF-16) hasil `paint` penuh, ditambah `*.outline` (statement, lipatan, masalah). `QH_BLESS=1` menulis ulang; diff-nya ditinjau di review sebagai perubahan tampilan. | W3-T2, selamanya |
| G2 diferensial inkremental | `tests/incremental.rs` | SplitMix64 berbenih, **tanpa resinkronisasi** ke pohon baru, jadi penyimpangan menumpuk (koreksi AR-TS, §1.5). Setiap edit acak: daftar statement == `scan()` seluruh teks; `dirty` ⊇ token yang berubah; indeks baris dan potongan == hitung naif; token inkremental sebelum konvergensi == `paint` dokumen baru (selisihnya dihitung dan dilaporkan; sasaran 0, dan bila tidak 0 berlaku §1.5 butir (3)). Sesudah antrean idle dikuras (termasuk konvergensi): token == `paint` dokumen baru, **wajib** sama. Seperempat edit dibias ke ±1 byte dari pemisah statement (mengetik dan menghapus `;`, kutip di batas). Default < 30 dtk di debug; `QH_EDITOR_SOAK=1` untuk soak. Alfabet dari blueprint lama §9 ditambah `$body$`, `::`, `:name`, `[`, `]`, dan `'it\'s'`. | W3-T2, selamanya |
| G3 konvergensi | `tests/incremental.rs` | Sesudah parse ulang konvergensi, lipatan dan masalah == dokumen baru | W3-T2 |
| G4 refaktor `scan.rs` | `crates/qh-sql/tests/scan_refactor.rs` | Seluruh `Scan`, `statement_count`, `statements_with_lines`, dan `decisions` untuk keempat `SafeMode`: lama == baru di korpus dan alfabet acak, dengan syarat lengkap di §4.2 "Batas Safe Mode" (koreksi AR-TS) | W3-T2, selamanya |
| G5 lexer | `crates/qh-sql/tests/lex.rs` | Region opaque == `walk`; pemotongan ke celah; kata kunci cadangan | W3-T2 |
| G6 kebocoran scanner | `crates/qh-sql-grammar/tests/scanner_leak.rs` | Hitungan alokasi hidup kembali ke awal (§3) | W3-T2, selamanya |
| G7 FFI dan penerapan | `app/Tests/QueryHiveTests/EditorAnalysisTests.swift` | Validasi hasil (setiap aturan tolak), revisi basi dibuang, penjaga IME (termasuk `setMarkedText` kosong), undo dan redo, surrogate, CRLF, drift, `drainForTesting`, pewarisan tetangga, `setTemporaryAttributes` tidak pernah dipanggil (sorotan find tetap ada sesudah `apply`), dan font 12,5 pt pada karakter baru | W4-T2 A |
| G8 palet | `SyntaxPaletteTests.swift` (baru) | Kontras §5.2 ≥ 4,5 untuk semua token di ketujuh kanvas, kecuali `comment` gelap (keputusan orkestrator, §5.2) | W4-T2 B |
| G9 visual | G-VIS | 16 baseline editor direkam ulang sebagai V-12; lapis atribut memuat run sementara | W4-T2 B |
| G10 angka | `--bench type-10k`, `type-2m` (dipangkas ≤ 2.000.000), `type-coloured-195k` | NFR-P5 untuk interval `keystroke` **dan** interval `apply` yang baru; gate gambar §14 | W4-T2 B |

**Korpus** (tanpa SQL dari TablePro): seed `deploy/dev/*.sql`; SQL dari `tools/golden/live_cases.py`; scene `Support/Snapshot.swift`; `probes.sql` §1.2 (dipindah dari scratch ke `tests/fixtures/corpus/dialect-*.sql`); dan tema tulisan tangan `edge-unclosed`, `edge-params`, `edge-dollar`, `edge-crlf`, `edge-surrogate`, `edge-unicode`, `edge-giant` (statement 300 KiB).

## 10. Yang dihapus, dan kapan

| Kapan | Yang dihapus | Di mana |
|---|---|---|
| W3-T2, W4-T2 A | tidak ada | — |
| W4-T2 B | `pattern`, `ceiling`, `attributes(for:baseFont:commentFont:)`, `apply(to:lexing:painting:)`, `storage(_:matches:in:)`, `isCalled`, `isLiteral`. Tetap ada: palet, `font(italic:)`, `keywords` (dipakai uppercase otomatis, `SQLEditor.swift:581`), ditambah `colour(for:)`. | `Views/SQLSyntax.swift` |
| W4-T2 B | `dirty`, `paintLimit`, `editedStatement`, `addDirty`, `paintDirty`, `widenedForQuotes`, `compute`, jalur `analyse` ke `SQLFolding` | `Views/SQLEditor.swift` |
| W4-T2 B | isi `regions`, `statementRanges`, `cteRegions`, `cteOpenParens`, `matchingParen`, helper lexing, `foldingSizeLimit`. `regions(in:)` dan `statementRanges(in:)` menjadi pembungkus FFI. | `Support/SQLFolding.swift` |
| W4-T2b | isi `sqlStatements(in:)` → pembungkus `sql_statement_ranges` | `Models/QueryTab.swift:1061` |
| tidak pernah | `SQLScanner` (`:name` dan opaque untuk parameter dan `KeywordCase`) | `Models/SQLScanner.swift` |

Yang tidak pernah dibuat, dibanding blueprint lama: `unicode_tables.rs`, `UnicodeTableExport.swift`, `LexerFixtureExport.swift`, `EditorLexerParityTests.swift`, dan `editor/lex.rs` versi port regex.

## 11. W3-T2: berkas per berkas

Pelaksana GP-o (opus). Ukuran **L** (sebelumnya M). Gate: RR, TD, CR, **SEC** (diff `scan.rs` dan seluruh `qh-sql-grammar`: C yang di-vendor, patch scanner, dan `unsafe`). Verifikasi: G-RUST, **G-DENY**, dan G-SWIFT (tidak ada berkas Swift yang berubah, jadi hanya bukti tidak mundur).

**Dua commit di dalam W3-T2 (koreksi AR-TS)**, supaya tinjauan SEC atas batas Safe Mode tidak tenggelam di diff ukuran L: commit 1 `qh-sql` (`walk`, `lex`, G4, G5) dan `qh-sql-grammar` (G6), dengan SEC; commit 2 `qh-editor` dan sisanya. Commit 1 dimulai sesudah W3-T0 mendarat (§15). `crates/qh-sql-grammar` boleh dikerjakan lebih dulu.

| Berkas | Tujuan | Antarmuka kunci |
|---|---|---|
| `Cargo.toml` | anggota `crates/qh-sql-grammar`, `crates/qh-editor`; `tree-sitter = "=0.26.13"`, `tree-sitter-language = "=0.1.7"`, `cc = "1.4"` di `[workspace.dependencies]`; `[profile.dev.package.*]` untuk grammar dan runtime (§3) | — |
| `Cargo.lock` | crate baru | — |
| `crates/qh-sql-grammar/**` (baru) | §3 | `LANGUAGE: LanguageFn` |
| `crates/qh-sql/src/scan.rs` | `walk` + `Visitor` + `EndState`; `scan()` identik | `pub fn walk`, `pub trait Visitor`, `pub enum OpaqueKind` |
| `crates/qh-sql/src/lex.rs` (baru) | lexer kode di atas `walk` (§4.5); kata tanpa kelas (D-21) | `pub fn lex(bytes, range, &mut impl FnMut(Token))`, `Token { start, end, kind }` |
| `crates/qh-sql/src/lib.rs` | ekspor `walk`, `lex`, `first_significant` | — |
| `crates/qh-sql/tests/{scan_refactor.rs,lex.rs}` (baru) | G4, G5 | — |
| `crates/qh-editor/Cargo.toml` (baru) | `qh-sql`, `qh-sql-grammar`, `tree-sitter`, `thiserror`; `forbid(unsafe_code)` | — |
| `crates/qh-editor/src/lib.rs` | doc crate, ekspor, konstanta (`CEILING_UTF16`, `GIANT_STATEMENT_BYTES`, `TREE_CACHE_BYTES`), `Class`, `Dialect` | — |
| `crates/qh-editor/src/text.rs` | `TextBuffer` (§4.1) | `new`, `replace`, `snapshot`, `drain_log`, `line_count`, `line_of` |
| `crates/qh-editor/src/statements.rs` | daftar statement dan resinkronisasi (§4.2) | `Statements::{new, apply_edit, ranges_for_ui}` |
| `crates/qh-editor/src/view.rs` | view `:name` (§4.3) | `parse_bytes(&str) -> Cow<[u8]>`, `widen(edit)` |
| `crates/qh-editor/src/syntax.rs` | parser, pohon per statement, cache LRU, konvergensi (§4.4) | `Syntax::{ensure, edit, evict, converge}` |
| `crates/qh-editor/src/classify.rs` | tabel §5.1, jalan pohon, celah ERROR (§4.5), lapis Parameter (§4.3) | `classify(stmt, tree, dialect) -> Vec<Tok>` |
| `crates/qh-editor/src/keywords.rs` | himpunan kata kunci grammar ∪ `EXTRA_KEYWORDS` (D-21) | `is_keyword(&[u8]) -> bool` |
| `crates/qh-editor/src/paint.rs` | himpunan kotor, selisih, `Paint` (§4.6) | `Analyzer::{sync, paint, outline}` (struct murni; `qh-ffi` memetakannya ke rekaman) |
| `crates/qh-editor/src/folds.rs` | §4.7 | — |
| `crates/qh-editor/src/issues.rs` | §4.8 | — |
| `crates/qh-editor/tests/{golden.rs,incremental.rs,statements.rs,folds.rs}` (baru) | G1–G3 | — |
| `crates/qh-editor/tests/fixtures/**` (baru) | korpus dan hasil emas, total ≤ 5 MB | — |
| `crates/qh-editor/benches/` atau `examples/editor_bench.rs` (baru) | mengulang tabel §1.3 dan §1.4 terhadap implementasi sungguhan; angka masuk laporan | — |

W3-T2 juga mencatat: parse penuh, parse jendela, dan ketikan p50/p99 pada keempat korpus; memori dengan cache terbatas; waktu build dingin `qh-sql-grammar`; dan, dari koreksi AR-TS, `replace` p99 dengan snapshot hidup (§4.1), `paint` p99 untuk ketikan di `dump-2m` (§4.4), memori analisis di luar pohon (§4.4), serta `scan()` sebelum dan sesudah refaktor (§4.2).

## 12. W4-T2 dan W4-T2b: berkas per berkas

Pelaksana GP-s. Gate SR, RR, SEC (FFI, validasi hasil), AX (rotor), UX (V-12, §5), AR, CR.

**Commit A** (`feat(editor): tree-sitter analysis behind the FFI, not wired to the editor yet`), tanpa perubahan perilaku:

| Berkas | Tujuan |
|---|---|
| `crates/qh-ffi/Cargo.toml` | `qh-editor` |
| `crates/qh-ffi/src/editor.rs` (baru) | §6 |
| `crates/qh-ffi/src/lib.rs` | `pub mod editor;` |
| `app/Generated/*` | `./app/build-ffi.sh` (invariant #1) |
| `app/Sources/QueryHive/Support/EditorAnalysis.swift` (baru) | pemilik `EditorDocument`; antrean; koalesi; `EditorPaintResult.validate`; `apply` lewat atribut sementara; `fonts`; penjaga IME; penjadwal idle; `outline`; pelebaran surrogate; drift; pewarisan tetangga; `StatementsRotorSource`; `drainForTesting` |
| `app/Tests/QueryHiveTests/EditorAnalysisTests.swift` (baru) | G7, pada `SQLTextView` headless yang belum disambungkan ke `SQLEditor` |
| `THIRD-PARTY-NOTICES.md` (baru, koreksi AR-TS) | Bagian pertama berkas pemberitahuan: runtime tree-sitter (MIT), header UTF ICU di runtime (lisensi Unicode), dan grammar DerekStride (MIT). Commit A adalah commit pertama yang menautkan kode ini ke `libqh_ffi.a` dan app. Pemberitahuan untuk seluruh crate lain dan pemasangannya ke bundle adalah tugas W14 (§15). |

Verifikasi A: G-RUST, G-DENY, G-FFI, G-SWIFT, G-VIS (tidak berubah).

**W4-T2b** (tidak berubah dari blueprint lama §15.1 dan `development-plan.md`): `Models/QueryTab.swift` (isi `sqlStatements(in:)` saja) dan `StatementSplitTests.swift`. Gate SR, DB, CR; G-SWIFT dan G-VIS tanpa perubahan piksel. Masih wajib, karena band dan run mark tetap dari `scan.rs` (§0.2). `sqlStatements(in:)` meneruskan dialek koneksi tab bila diketahui, selain itu `.generic` (D-20); `StatementSplitTests` memuat satu kasus MySQL dari W3-T0.

**Commit B** (`perf(editor): tree-sitter colours and folds, applied as temporary attributes, reach 2M characters`):

| Berkas | Tujuan |
|---|---|
| `app/Sources/QueryHive/Views/SQLEditor.swift` | hook ke `EditorAnalysis`, pengamat scroll, band/run mark/lipatan dari `outline`, rotor, flag pemeriksaan teks (§7.3), penghapusan §10 |
| `app/Sources/QueryHive/Views/SQLSyntax.swift` | §10; `colour(for class:)` |
| `app/Sources/QueryHive/Support/SQLFolding.swift` | §10 |
| `app/Tests/QueryHiveTests/EditorFindAndFoldingTests.swift` | batas lipatan dari `editorCeilingUtf16()`; lipatan CTE/subquery baru |
| `app/Tests/QueryHiveTests/EditorIncrementalTests.swift` | tes painter 4A (`testTypingInsideAStatementRepaintsItAndNothingElse`, `testAStatementWithADoubleQuotedSemicolon…`, `testAnEditAfterTheSemicolon…`, `testALoneQuoteInALargeDocumentDoesNotFreezeTyping`) ditulis ulang untuk jalur baru |
| `app/Tests/QueryHiveTests/Bench/EditorBenchTests.swift` | bench memakai jalur baru |
| `app/Sources/QueryHive/Support/PerfSignposts.swift`, `Support/BenchMode.swift` | interval `apply` (§14) dan pencatatannya di `--bench type-*` (koreksi AR-TS) |
| `app/Tests/QueryHiveTests/EditorRotorTests.swift` (baru) | rotor "Statements" |
| `app/Tests/QueryHiveTests/SyntaxPaletteTests.swift` (baru) | G8 |
| `app/Tests/QueryHiveTests/__Baselines__/editor-*.{png,json}` | rekam ulang V-12 (16 pasang) |
| `app/DESIGN.md` §"Colouring the query" | isi baru: tree-sitter per statement, atribut sementara, plafon 2M, pemilik kunci sementara. Kalimat "One scan, six ordered alternatives" dan "stops past 200,000 characters" menjadi salah di commit ini. |

Verifikasi B: G-RUST, G-FFI, G-SWIFT, G-VIS (hanya scene editor yang berubah, sebagai V-12), `--bench type-10k` ≤ 4 ms dan `type-2m` ≤ 8 ms p99 main untuk interval `keystroke` **dan** `apply`, `type-coloured-195k` dicatat, dan bench auto-uppercase di `type-2m` dicatat.

**Tidak disentuh:** `Models/SQLScanner.swift`, `Models/QueryParameters.swift`, `Models/KeywordCase.swift`, `Views/Workspace.swift`, `app/Package.swift` (tidak ada framework sistem baru; tree-sitter hanya butuh libc), `crates/qh-ffi/src/uniffi_api.rs`, dan keempat daftar invariant #11.

## 13. Urutan kerja

**W3-T0 lebih dulu** (§15): perbaikan Safe Mode MySQL di `scan.rs`/`classify.rs`, gate SEC. Langkah 1 W3-T2 boleh berjalan bersamaan; langkah 2 tidak.

**W3-T2:**

1. `qh-sql-grammar`, lalu G6. Jalankan `cargo deny check licenses`.
2. `walk`, lalu G4 hijau dengan tes lama `scan.rs` tanpa perubahan. Commit 1 (SEC) ditutup di sini sesudah langkah 3.
3. `lex`, lalu G5.
4. `text.rs` dan `statements.rs`, dengan tes statement == `scan()`.
5. `view.rs`, `syntax.rs`, `classify.rs`. Tulis korpus, lalu `QH_BLESS=1` pertama dan tinjau hasil emasnya baris demi baris untuk tabel §5.1.
6. `paint.rs`, sampai G2 hijau (termasuk contoh §6).
7. `folds.rs`, `issues.rs`, G3.
8. Bench §11, dan setel `GIANT_STATEMENT_BYTES` serta `TREE_CACHE_BYTES` dari angkanya.

**W4-T2 A → W4-T1 mendarat → W4-T2b → W4-T2 B**, sama dengan urutan yang sudah ada di `development-plan.md` §5 W4.

**W4-D.** ADR-0033 ditulis ulang: "Analisis editor dengan tree-sitter per statement di Rust". Isinya D-1 sampai D-20, grammar yang di-vendor dan patch-nya, kebijakan diagnostik, pemilik kunci atribut sementara, dan V-12.

## 14. Risiko

| Risiko | Dampak | Penahan |
|---|---|---|
| **Gambar ulang baris berwarna 4–5 ms** (§1.6, jalur `cacheDisplay`) | NFR-P5 (4 ms p99 di 10k) meleset walaupun pembetulan atribut hilang | **Gate gambar (koreksi AR-TS).** Interval `keystroke` yang ada sudah berakhir di observer sesudah commit Core Animation pada giliran yang menampilkan perubahan (`PerfSignposts.swift:130-141`), jadi gambar baris pada giliran edit sudah terukur. Yang belum terukur adalah giliran `apply`, yang menggambar ulang baris itu sekali lagi dengan warna baru. W4-T2 B menambah interval `apply` (mulai di `apply`, berakhir di observer yang sama), `--bench type-*` menunggu `apply` sebelum ketikan berikutnya dan mencatat p50/p99 keduanya, dan ambang NFR-P5 berlaku untuk masing-masing giliran. Urutan pengungkit: flag pemeriksaan teks (−1,3 ms terukur), `apply` yang melewati run yang sudah benar (§7.3), `CGColor` yang di-cache per appearance (belum terbukti). Bila sesudah itu salah satu p99 masih di atas ambang, angkanya dicatat sebagai kegagalan gate NFR-P5, tugas fungsional tetap jalan (P-21), dan eskalasi mengikuti syarat §7.7 (spike CodeEditTextView lebih dulu, keputusan pemilik). |
| Cakupan grammar untuk MySQL dan Trino | Struktur (lipatan, alias) hilang di statement ber-ERROR | Lexer celah menjaga warna; lipatan statement tetap ada; alias memakai cadangan leksikal. Diagnostik sintaks tidak ditampilkan. |
| Pohon inkremental ≠ pohon baru di statement ber-ERROR (14–16% edit acak) | Lipatan atau alias bergantung pada riwayat | Parse ulang konvergensi 500 ms sesudah edit (§4.4), dan warna terbukti tidak terpengaruh (§1.5) |
| Memori pohon 36–54 B per byte | 70–108 MB untuk 2M bila tanpa batas | Cache LRU sekitar 640 KiB sumber per dokumen (D-9), dan tes memori di W3-T2 |
| C di dalam proses (parser dan scanner) | Crash atau baca di luar batas pada input aneh | Runtime dan grammar dipakai luas; G2 menjalankan puluhan ribu edit acak termasuk byte non-ASCII; SEC memeriksa `scanner.c` (190 baris) dan patch-nya; `panic = "unwind"` tidak menangkap crash C, jadi ini risiko yang diterima dan dicatat di ADR-0033 |
| Kutip terbuka di dokumen besar | Statement 1 MB sampai kutip ditutup | Statement besar diwarnai lexer (§4.4); `walk` dari statement yang diedit, batas atas 2–10 ms di latar |
| Escape backslash MySQL (`'it\'s'`) dan komentar `#` | Warna dan batas statement salah, **di editor dan di engine**; di engine ini **celah Safe Mode** (koreksi AR-TS, §15) | W3-T0 sebelum W3-T2 langkah 2; editor mewarisi mode MySQL `walk` lewat D-20. |
| Pemilik kunci atribut sementara ganda | Satu fitur menghapus sorotan fitur lain | Tabel §7.4; tes G7 memastikan sorotan find bertahan sesudah `apply` |
| Karakter baru tanpa warna selama satu giliran | Kedip | Pewarisan tetangga (§7.5). Dicatat di `performance-plan.md` §14 butir 12 (kalimatnya berubah dari "typing attributes" ke "warna tetangga, dengan aturan §7.5"). |
| Grammar upstream bergerak | Perbaikan tidak masuk | Vendoring dengan provenance; pembaruan adalah tugas dengan gate (§3) |
| MSRV | `cargo update` menarik tree-sitter 0.27 | Versi dipatok `=` di workspace |
| Ukuran biner +2,63 MB, build dingin +15 dtk | App 11% lebih besar | Dicatat; tidak ada target ukuran di PRD |

## 15. Untuk orkestrator, dan pertanyaan terbuka

**Perubahan dokumen yang bukan milik blueprint ini:**

- **PRD:** baris O-14 di §11.1; V-12 di §6.5 (§5.3); NFR-L: `tree-sitter`, `tree-sitter-language`, grammar yang di-vendor; P-10 (§8.4); FR-ED-06: galat sintaks dari pohon tidak ditampilkan secara default (§0.2).
- **`performance-plan.md`:** §8 4B ditulis ulang ringkas sesuai blueprint ini (modul `qh-editor`, satu pohon per statement, atribut sementara, "paritas lexer 100%" diganti G1–G3 dan V-12); §13 baris tree-sitter menjadi "Ya (O-14)"; §14 butir 8 dan 12; §15 judul ADR-0033; §17 dependensi baru dan baris "Ditolak" diperbarui.
- **`development-plan.md`:** W3-T2 (berkas §11, ukuran L, SEC untuk `qh-sql-grammar`, G-DENY); W4-T2 (berkas §12, UX di gate, V-12); W11-T6 (`crates/qh-editor/src/refs.rs` menggantikan `crates/qh-sql/src/editor/`); W12-T1 (token dari `walk` dan `lex`); W10-T6 (masalah leksikal dan sambungan dialek); W10-T7 (pasangan kurung lewat `drawBackground`); tabel ADR (`0033` berganti judul); rantai kepemilikan `Cargo.toml` dan `Cargo.lock` untuk W3-T2.
- ~~Backlog~~ **Tugas baru (koreksi AR-TS), bukan backlog:**
  - **W3-T0. Celah Safe Mode MySQL (SEC, sebelum W3-T2 langkah 2).** Diturunkan dari membaca kode, belum dijalankan: `scan.rs` tidak mengenal escape backslash dan komentar `#` MySQL, `mysql_async` 0.36.2 selalu menyalakan `CLIENT_MULTI_STATEMENTS` (`opts/mod.rs:1096`), dan jalur teks driver MySQL tidak punya `prepare` yang menolak multi-statement seperti PostgreSQL (`qh-driver-postgres/src/lib.rs:19`). Akibatnya `SELECT '\''; DELETE FROM t; -- '` dibaca `scan()` sebagai satu `SELECT` (`\` lalu `''` dianggap kutip ganda), `classify` memberi `ReadOnly`, dan server MySQL menjalankan `DELETE`. Bentuk kedua: `SELECT 1 # '` + LF + `; DELETE FROM t; -- '`. MCP memaksa `SafeMode::ReadOnly` (`crates/qh-ffi/src/mcp.rs:1291-1299`) dan driver MySQL tidak punya sesi read-only di server (`read_only: false`), jadi ini melanggar NFR-S6 untuk koneksi MySQL. Arah perbaikan yang dianjurkan (diputuskan SEC): mode MySQL di pemindai (`\` meng-escape di `'…'` dan `"…"`, `#` komentar baris, isi `/*! … */` dibaca sebagai kode), dan Safe Mode di koneksi MySQL menolak bila **salah satu** pembacaan (standar atau MySQL) tidak `ReadOnly` atau jumlah statement-nya berbeda. Tes dulu: kedua contoh di atas di `crates/qh-sql` dan di `crates/qh-ffi/tests/safe_mode.rs`, plus G-LIVE MySQL. Gate SEC, DB, CR. Pemilik berkas: `crates/qh-sql/src/{scan.rs,classify.rs}` sebelum W3-T2.
  - **Pemberitahuan pihak ketiga.** Bagian tree-sitter/ICU/grammar ikut W4-T2 commit A (§12). Berkas lengkap untuk semua crate dan pemasangannya ke bundle (`app/build.sh`) masuk W14 sebagai kriteria rilis, karena celah ini sudah ada sebelum 4B dan tidak ada distribusi sebelum rilis.

**Keputusan orkestrator atas pertanyaan terbuka (30 Sep 2026), diterima AR:**

1. **Vendoring 17,4 MB C** (sekitar 818 KB di git) dengan perbaikan bocor PR #361, di `crates/qh-sql-grammar`. **Diputuskan: vendoring.**
2. **Garis bawah sintaks dari pohon:** **mati secara default.** W10-T6 menampilkan posisi galat server dan diagnostik leksikal (kutip, komentar, identifier, dan dollar-quote yang belum ditutup, kurung tidak seimbang).
3. **Warna parameter:** **`literal`.** Kelas 9 tetap terpisah di FFI.
4. **Kontras Nord:** **diperbaiki di V-12** (§5.2), karena baseline editor direkam ulang di commit yang sama (NFR-A1).

## Riwayat: blueprint lexer regex (digantikan)

Blueprint sebelumnya (port regex `SQLSyntax` ke `qh-sql`, dengan paritas byte demi byte) digantikan oleh keputusan pemilik O-14 pada 30 Sep 2026. Teks lengkapnya ada di `git show dc4186f:docs/architecture/blueprints/fase-4b-editor-analysis.md`. Verdict AR-nya disimpan di bawah apa adanya. Nomor bagian di dalamnya merujuk ke versi itu, bukan ke dokumen ini. Keputusan yang dibawa ke blueprint ini: D-9 (penjaga IME di sisi penerapan) menjadi D-16, D-13 (dua commit) menjadi D-19, D-14 (rekaman UniFFI) menjadi D-18, urutan kunci dan satu delegate storage, refaktor `scan.rs` dengan gate SEC, W4-T2b, dan koreksi P-10. Yang gugur: D-1, D-2, D-4, D-5, D-10, D-12, tabel Unicode, eksportir fixture Swift, dan uji paritas L4/L5.

### Verdict architect-reviewer atas blueprint regex

**Verdict: disetujui dengan koreksi.** Semua koreksi sudah diterapkan di dokumen ini dan ditandai "(koreksi AR)". W3-T2 boleh mulai sesudah orkestrator menyamakan daftar berkas di `development-plan.md` (lihat bagian terakhir di bawah). Dampak arsitektur: **sedang**. Batas barunya bersih: inti murni di `qh-sql`, konkurensi di `qh-ffi`, dan penerapan di Swift. Satu-satunya sentuhan ke batas keamanan adalah refaktor `scan.rs`, dan sekarang refaktor itu punya gate sendiri.

**Yang diperiksa.** `performance-plan.md` §8, PRD FR-PERF-04, FR-ED-09, NFR-V (§6.5), NFR-P5, dan P-10. Kode: `Views/SQLSyntax.swift` (pola, `ceiling`, `apply` dengan `setAttributes` atas seluruh dokumen), `Views/SQLEditor.swift` (`isRichText = false` di `:41`, font 13 pt di `:44`, find bar dengan temporary attribute di `:601-621`, `refreshFolds` di `:670-693`, `SQLFoldStyler`), `Support/SQLFolding.swift` (janji "tidak bisa berselisih" di `:138-140`), `Models/SQLScanner.swift` (tidak punya bagian statement, sesuai klaim §15.2), `Models/QueryTab.swift:1056-1109` beserta keempat pemanggilnya, `crates/qh-sql/src/scan.rs` dan `classify.rs:566-608`, `VisualParityTests.attributeRuns`, serta invariant #1, #3, dan #11.

**Kepatuhan pola**

| Pola | Status |
|---|---|
| Inti murni tanpa kunci atau thread di `qh-sql` (`lib.rs:16`) | patuh |
| Satu pemindai untuk statement (`scan.rs`) di engine, editor, dan formatter | patuh sesudah W4-T2b; sebelum itu ada jendela selisih (§15.1) |
| Dua mesin di satu modul (D-2) | dapat diterima; memang dituntut NFR-V selama keanehan §3.5 dibekukan |
| Main thread O(edit) | patuh sesudah `last_closer` dipindah ke latar (§5.1 langkah 6) |
| Invariant #1 dan #11 | patuh; `EditorDocument` bukan perintah engine |
| Invariant #3 (enum tertutup di dua sisi) | kode kelas 1 sampai 8 adalah enum kedua; cukup dicatat di ADR-0033 |
| Kepemilikan berkas §7 rencana | belum: `scan.rs` dan berkas tes belum ada di daftar tugas |

**Keputusan**

- **D-9, penjaga IME: di sisi penerapan.** Edit tetap dikirim selama `hasMarkedText`. Cara ini menjaga cermin Rust tetap tepat, sehingga detektor drift tidak menembak palsu saat pengguna men-scroll di tengah komposisi, dan kode penggabung edit tidak diperlukan. Ada dua tambahan: akhir komposisi dideteksi dari transisi `true → false` di `insertText`, `setMarkedText` (pembatalan dengan string kosong), dan `unmarkText`; penjadwal idle juga berhenti selama komposisi.
- **D-13, dua commit di W4-T2: diterima**, dengan syarat A hijau, `EditorAnalysis` belum tersambung ke editor di A, dan tidak ada tugas lain di antara A dan B selain W4-T2b.
- **W4-T2b: diterima dan wajib.** Tanpa tugas ini, commit B membuat band dan run mark memakai `scan.rs` sementara Run masih memakai pemindai Swift, sehingga `select "a;b"` tampil sebagai satu statement tetapi Run mengirim `select "a`. Cakupannya hanya isi `sqlStatements(in:)` ditambah satu berkas tes, dengan signature tetap. Gate SR, DB, CR. Urutan yang dianjurkan: A → W4-T1 → W4-T2b → B. W4 tidak boleh ditutup sebelum W4-T2b mendarat.
- **D-10, tabel `\b` dari ICU yang dibekukan: sehat.** Di ICU, `\b` dan `\w` memakai himpunan kata yang sama, dan aturan lompat `Grapheme_Extend`/`Cf` cocok dengan lima kasus yang diverifikasi. Tanpa crate baru, `std` Rust tidak punya `Nd`, `M`, `Pc`, `Grapheme_Extend`, atau `Cf`. Supaya tabel bisa dipelihara, kepala berkas mencatat asal-usulnya, dan eksportir tabel dipisah menjadi `UnicodeTableExport.swift` yang tetap hidup sesudah commit B. Selisih dengan macOS berikutnya diperlakukan sebagai perubahan V, bukan pembaruan otomatis.
- **P-10: koreksinya benar.** Formatter yang dibangun di atas lexer warna akan mewarisi badan `$tag$` yang dilex sebagai kode dan `+--` yang tidak dianggap komentar. Region opaque wajib diambil dari `scan.rs`. Tokenisasi region kode tidak diikat di sini dan diserahkan ke rancangan W12-T1.

**Koreksi yang diterapkan**

1. **Oracle paritas (§9).** L4 dan L5 membandingkan dengan `SQLSyntax.attributes(for:)` atas seluruh dokumen (semantik commit P), bukan dengan painter per statement milik 4A. L5 membandingkan seluruh dokumen sesudah `drainForTesting()` dan tidak mewarisi pembatasan G-VIS yang baru (rentang terlihat dikurangi badan yang dilipat).
2. **Paket byte diganti rekaman UniFFI** dengan larik `u32` datar (D-14, §6.2). Lifting terjadi di antrean latar, jadi NFR-P5 tidak terpengaruh. Magic, versi, dan padding tidak menjaga apa pun karena binding diregenerasi bersama Rust. Bila p99 lifting > 2 ms, `Vec<u8>` datar boleh dipakai tanpa AR baru.
3. **`frontier` lazy ditunda** (§5.5, §7.1). Lex ulang O(n) hanya terjadi saat kutip membalik pasangan dan hanya di latar. Pemicunya adalah `lex_all` 2 MB > 16 ms di bench W3-T2.
4. **`last_closer` tidak lagi dicari mundur di `replace`** (§5.1). Pencarian itu O(n) di main.
5. **Urutan kunci:** `text` adalah kunci daun (§6.1).
6. **Pewarnaan pertama sinkron memakai satu aturan:** jendela terlihat ±2 layar dengan anggaran 131.072 unit (§7.6). Draf awal mewarnai penuh sampai 262.144 unit, dan dengan anggaran §7.4 itu berarti hitch sekitar 30 ms setiap kali tab berpindah.
7. **Typing attributes** di editor `isRichText = false` ditandai `[perlu verifikasi]`, lengkap dengan tes font 12,5 pt dan jalan keluar O(1) (§7.4). Bila pewarisan tidak terjadi, karakter 13 pt membuat tinggi baris melompat, dan itu perubahan geometri.
8. **Satu delegate `NSTextStorage`** untuk indeks ruler 4A dan hook 4B, dengan urutan tetap (§7.3).
9. **Refaktor `scan.rs`** mendapat tes diferensial `scan_refactor.rs` dan SEC khusus untuk diffnya (§4.1, §11).
10. **YAGNI di permukaan FFI:** `line_of` dan `line_starts` hanya diekspor bila asumsi 4A no. 5 gagal (§6.1).

**Tidak diubah, dan dinilai tepat.** Granularitas checkpoint (64 baris atau 4 KB, dengan posisi layak di tengah baris untuk dump satu baris 2 MB) sederhana dan memang diminta rencana. Himpunan kotor di Rust, beserta `mark_applied`, diperlukan agar main hanya mengecat rentang yang berubah dalam anggaran 4 ms. Dua kunci dan dua antrean serial adalah bentuk paling kecil yang menjaga main tidak pernah menunggu lexer. Undo aman karena rentang edit selalu kotor. Find bar aman karena `addAttributes` di storage tidak menyentuh temporary attribute di layout manager.

**Untuk orkestrator (dokumen yang bukan milik AR)**

- `development-plan.md`: tambahkan `scan.rs`, `UnicodeTableExport.swift`, dan `scan_refactor.rs` ke W3-T2, dengan SEC untuk `scan.rs`; buang `Models/SQLScanner.swift` dari W4-T2 dan tambahkan berkas tesnya; ganti `editor.rs` dengan direktori `editor/`; tambahkan W4-T2b ke daftar tugas W4 dan ke rantai `QueryTab.swift` di antara W4-T1 dan W5-T1.
- `performance-plan.md` §8: kalimat IME diganti dengan rujukan ke ADR-0033 (D-9). `performance-plan.md` §14: daftarkan perubahan perilaku di tabel §4.1 dan warna tetangga pada karakter baru selama satu frame.
- PRD P-10: "di atas mesin `scan.rs` (region opaque) dan modul editor 4B".
- ADR-0033 (W4-D): D-1 sampai D-14, keanehan yang dibekukan, asal-usul tabel Unicode, dan enum kelas sebagai kasus invariant #3.

**Implikasi jangka panjang.** Sesudah W4-T2b, `scan.rs` menjadi satu-satunya jawaban untuk "di mana statement berakhir" di engine, editor, Run, sort dan search server, konfirmasi tulis, dan formatter. Jalur ini mempermudah W10-T6, W11-T6, W12-T1, dan W12-T3. Utang yang sengaja diambil adalah lexer warna kedua yang membawa keanehan regex. Utang itu hanya bisa dilunasi lewat perubahan V dengan rekam ulang baseline (§15.7), dan keputusan itu ada di tangan pemilik.

## Verdict architect-reviewer (tree-sitter)

**Verdict: disetujui dengan koreksi.** Semua koreksi sudah diterapkan di dokumen ini dan ditandai "(koreksi AR-TS)". Dampak arsitektur: **tinggi**. Ada crate C baru di dalam proses app, satu dependensi runtime baru, cara menerapkan warna yang baru, dan sentuhan ke batas Safe Mode. Bentuk dasarnya sehat: grammar dikurung di `qh-sql-grammar`, analisis murni di `qh-editor`, konkurensi di `qh-ffi`, dan penerapan di Swift. Batas statement tetap satu jawaban (`scan.rs`) untuk engine, Run, dan editor. W3-T2 boleh mulai dengan langkah 1 (grammar). Langkah 2 (refaktor `scan.rs`) menunggu W3-T0, tugas SEC baru yang lahir dari pemeriksaan ini (§15).

**Yang diperiksa.** Blueprint ini secara utuh; `target/run/ts-bench/results.txt`, `probe-out.txt`, `probe-out-gap.txt`, dan sumber bench `src/main.rs` (`cmd_incr`); `crates/qh-sql/src/scan.rs`, `classify.rs` (`classify`, `decisions`, `statements_with_lines`), `wrap.rs`, `lib.rs`; `Cargo.toml` root dan manifes kelima konsumen `qh-sql`; `crates/qh-ffi/Cargo.toml` dan `src/mcp.rs:1291-1299`; driver MySQL (jalur teks, `read_only: false`) dan `mysql_async` 0.36.2 `opts/mod.rs:1096`; driver PostgreSQL (`prepare` menolak multi-statement); manifes `tree-sitter` 0.26.13, `tree-sitter-language` 0.1.7 dan 0.1.8, `tree-sitter-sequel` 0.3.11 di registry lokal; `rustc` lokal 1.98.1, tidak ada `rust-toolchain.toml`, CI memasang `stable`; `SQLScanner.swift:58-64,112-117`; `SQLEditor.swift:99-105` (flag pemeriksaan teks), `:1228-1248` (sorotan find), `:1615` (lipatan lewat glyph null); `VisualParityTests.attributeRuns` (`:952-1009`); `PerfSignposts.swift:125-147` dan `BenchMode.swift`; PRD NFR-P5, NFR-A1, NFR-L, NFR-S6, V-1…V-11, FR-ED-06, P-10; `performance-plan.md` §8 dan §12; `development-plan.md` W3-T2, W4-T2, W4-T2b, W10-T6, W10-T7, W11-T6, W12-T1, §4, §7. Lisensi CodeEditTextView, `Package.swift`-nya, dan lisensi TextStory diambil dari GitHub hari ini. Tidak ada yang dibangun ulang.

### Kepatuhan pola

| Pola | Status |
|---|---|
| Driver dan `qh-import` tidak mengompilasi C grammar | patuh: `qh-editor` terpisah, hanya `qh-ffi` yang bergantung padanya (D-4) |
| Arah dependensi `qh-editor → qh-sql`, tidak sebaliknya | patuh sesudah koreksi D-21 (daftar kata kunci editor keluar dari `qh-sql`) |
| `qh-sql` murni, tanpa `unsafe` | patuh; `unsafe` hanya di `qh-sql-grammar` (FFI ke C) dan satu tes |
| Satu pemecah statement untuk engine, Run, dan editor | patuh, dan diperkuat: `sql_statement_ranges` menerima dialek (D-20) |
| Refaktor batas Safe Mode tanpa perubahan perilaku | patuh sesudah G4 diperluas ke seluruh `Scan` dan `decisions` (§4.2) |
| Main thread O(edit) | patuh dengan satu syarat terukur: CoW 2 MB saat snapshot hidup (§4.1) |
| Satu pemilik per kunci atribut sementara | patuh (§7.4); find memakai `removeTemporaryAttribute(.backgroundColor)`, tidak menyentuh `.foregroundColor` |
| Invariant #1, #3, #11 | patuh; kelas 1…9 enum tertutup kedua, dicatat di ADR-0033 |
| MSRV | patuh secara deklarasi (0.26.13 dan 0.1.7 menyatakan 1.77); 1.85 tidak diverifikasi CI |

### Jawaban atas titik yang diminta

**(a) Refaktor `scan.rs` dan Safe Mode.** Rencana awal (salinan lama + tes diferensial + SEC) arahnya benar, tetapi terlalu sempit: yang dikunci hanya "`scan()` lama == baru", padahal `classify` bergantung pada `keywords` dan `leading_keyword`, bukan hanya pemisah. Sekarang G4 membandingkan seluruh `Scan`, `statement_count`, `statements_with_lines`, dan `decisions` untuk keempat mode, dengan alfabet tertulis, soak satu juta kasus, pembanding kecepatan, dan larangan mengubah tes lama. W3-T2 tidak mengubah perilaku engine; satu-satunya perubahan API adalah `walk`, `lex`, dan `first_significant` menjadi `pub`. **Temuan yang lebih penting ada di luar 4B:** `scan.rs` hari ini sudah salah baca MySQL, dan untuk MCP itu celah keamanan (W3-T0, §15). Temuan ini diturunkan dari kode dan belum dijalankan. SEC harus menulis kedua contohnya sebagai tes yang gagal dulu.

**(b) Pohon per statement.** Resinkronisasi §4.2 benar untuk `;` dan kutip yang diketik atau dihapus. Pemisah hanya ada di state Normal, statement ke-i memuat `;`-nya sendiri, dan titik henti "pemisah sesudah ujung edit yang sama dengan batas lama yang digeser" menjamin teks dan state sesudahnya identik. Edit tepat di awal statement aman karena state sesudah `;` selalu Normal. G2 sekarang membias seperempat edit ke sekitar pemisah dan membandingkan daftar statement dengan `scan()` di setiap langkah. Untuk memori: pohon dibatasi LRU, dan sekarang memori di luar pohon juga dicatat dengan sasaran ≤ 48 MB di 2M. Batas 256 KiB masuk akal (statement terbesar di korpus biasa 1.277 B), tetapi statement besar semula tetap menyimpan dan membandingkan ~400.000 token per ketikan. Sekarang tokennya tidak di-cache, dan lex hanya berjalan sampai ujung jendela (§4.4).

**(c) Warna dan riwayat edit.** Klaim "0 dari 4.000" lebih lemah dari yang tertulis. Bench memakai satu pohon untuk seluruh potongan, tanpa view `:name`, dan pohon di-reset setiap kali menyimpang, jadi penyimpangan yang menumpuk tidak pernah diuji. Klaim itu juga tidak dijamin secara struktur, karena anak ERROR diwarnai pohon sedangkan celah diwarnai lexer. Penahannya sekarang ada tiga (§1.5): G2 tanpa reset, konvergensi yang mengecat ulang selisih token sehingga warna akhir selalu fungsi dari teks, dan aturan cadangan "lex seluruh ERROR" bila G2 menemukan selisih. View `:name` sendiri murni fungsi teks (tiga byte), tetapi aturannya merusak slice (`arr[lo:hi]` → `lo_hi`), dan cara memberi kelas Parameter tidak cocok dengan `SQLScanner` (`:58` mengecualikan isi `[…]`). Keduanya dikoreksi (D-8, §4.3).

**(d) Atribut sementara.** Karakter yang diketik dan undo sudah terverifikasi di spike, dan rentang edit selalu kotor, jadi keduanya dicat ulang. Find memakai kunci lain dan membersihkan hanya `.backgroundColor`. G-VIS memang membaca `temporaryAttributes` per rentang (`VisualParityTests.swift:986-996`) dan menggabungkan tetangga yang sama, jadi baseline V-12 deterministik. Italic komentar di storage benar, karena `.font` sementara tidak digambar. Yang kurang ada dua. Pertama, IME: "kuncinya berbeda" belum pernah diuji dan `markedTextAttributes` bisa memuat `.foregroundColor`, jadi klaim itu ditandai `[perlu verifikasi]` dengan tes G7. Kedua, `apply` yang menulis ulang warna yang sudah benar tetap meng-invalidate dan menggambar ulang baris seharga 4–5 ms; sekarang `apply` melewati run yang tidak berubah (§7.3).

**(e) Crate, MSRV, build.** Pemisahan `qh-editor`/`qh-sql-grammar` benar dan perlu: kelima konsumen `qh-sql` tidak menyentuh C. Bin `queryhive-engine` dan `queryhive-mcp` ikut menautkan `qh-editor` lewat `qh-ffi`, tetapi LTO fat dan penautan arsip membuang kode yang tidak dirujuk, jadi yang bertambah hanya waktu build. Patok `=0.26.13`/`=0.1.7` benar karena 0.1.8 menyatakan 1.90. Tambahan: `cc` masuk `[workspace.dependencies]`, dan `opt-level = 2` untuk grammar dan runtime di profil dev, supaya G2 di debug tidak memakai parser `-O0`.

**(f) Gambar ulang 4–5 ms.** Semula gate ini tidak terdefinisi. Interval `keystroke` yang ada ternyata sudah mencakup gambar pada giliran edit (berakhir sesudah commit Core Animation), tetapi giliran `apply`, yang menggambar ulang baris dengan warna baru, tidak terukur sama sekali. Sekarang ada interval `apply`, bench menunggu `apply`, dan ambang NFR-P5 berlaku untuk tiap giliran. Urutan pengungkit dan jalan eskalasinya juga tertulis (§14). Jalan mundur ke CodeEditTextView: lisensi MIT terverifikasi, tetapi paketnya menarik TextStory (BSD-3-Clause), swift-collections (Apache-2.0), dan plugin SwiftLint, sehingga butuh tinjauan NFR-L. Paket itu juga belum terbukti lebih murah karena ia pun menggambar lewat CoreText dengan warna per run. Eskalasi sekarang mensyaratkan spike terukur dan keputusan pemilik (§7.7).

**(g) Backlog.** Escape backslash MySQL **bukan backlog**. Bersama komentar `#`, ia membuat Safe Mode MySQL bisa dilewati (`SELECT '\''; DELETE FROM t; -- '`), dan MCP bersandar penuh pada Safe Mode untuk MySQL (NFR-S6). Karena itu ia menjadi **W3-T0**, sebelum W3-T2 langkah 2, supaya salinan beku G4 membekukan pemindai yang sudah diperbaiki dan `walk` lahir dengan mode MySQL. Berkas pemberitahuan pihak ketiga dipecah dua: bagian tree-sitter/ICU/grammar ikut W4-T2 commit A (commit pertama yang menautkan kode itu ke app), dan berkas lengkap beserta pemasangannya ke bundle menjadi kriteria rilis W14.

**(h) Pembagian tugas.** W3-T2 ukuran L diterima, dipecah menjadi dua commit: `qh-sql` + grammar dengan SEC, lalu `qh-editor`. W4-T2 A/B dengan W4-T2b di antaranya tetap. Commit A menambah `THIRD-PARTY-NOTICES.md`, commit B menambah `PerfSignposts.swift` dan `BenchMode.swift`, dan W4-T2b meneruskan dialek. W10-T6 hanya Swift: diagnostik leksikal dan posisi server, garis bawah lewat `.underlineStyle` sementara, sambungan dialek per tab ke `EditorDocument::new`, dan garis bawah sintaks mati. W10-T7 menambah `bracket_pair`, jadi masuk rantai lane FFI dan memegang `qh-editor/src/brackets.rs`. W11-T6 pindah ke `crates/qh-editor/src/refs.rs`. W12-T1 tetap di `qh-sql` di atas `walk` + `lex`, dengan daftar kata klausa sendiri, tanpa pohon.

### Keputusan

- Empat keputusan orkestrator dicatat dan diterima tanpa pembatalan: grammar di-vendor dengan PR #361 di `crates/qh-sql-grammar`; garis bawah sintaks dari pohon mati secara default; parameter memakai `literal`; kontras Nord diperbaiki di V-12. Keputusan keempat didukung bukti: G8 kini mengunci ketujuh kanvas.
- D-21 baru: `qh-sql` tidak tahu daftar kata kunci editor.
- W3-T0 baru, dengan gate SEC, mendahului refaktor `scan.rs`.
- `sql_statement_ranges` menerima dialek sekarang, bukan belakangan.

### Koreksi yang diterapkan

1. §1.5: batas bukti diferensial (satu pohon, tanpa view, reset per selisih), dan tiga penahan yang menggantikannya.
2. D-8, §4.3: syarat byte i−1 ∉ `[A-Za-z0-9_:]` di view, dan kelas Parameter dari lapis leksikal yang meniru `SQLScanner`, lengkap dengan korpus `edge-params`.
3. D-9, §4.4: token statement besar tidak di-cache dan hanya dilex sampai ujung jendela; sasaran memori di luar pohon; konvergensi mengecat ulang selisih; aturan warna sementara sebelum parse selesai.
4. D-20, §6, §12: `sql_statement_ranges(sql, dialect)`, dan W4-T2b meneruskan dialek tab.
5. D-21, §4.5, §11: `qh_sql::lex` mengeluarkan `Word` tanpa kelas, dan `keywords.rs` di `qh-editor`.
6. §3, §11: `cc` di workspace, `opt-level` dev untuk C, dan catatan MSRV yang tidak diverifikasi.
7. §4.1: biaya CoW di main diukur, dengan jalan keluar buffer berpotongan.
8. §4.2, G4: syarat refaktor batas Safe Mode (seluruh `Scan` dan `decisions`, soak, kecepatan, tes lama utuh, `first_significant` `pub`), dan W3-T2 langkah 2 sesudah W3-T0.
9. §7.3: `apply` melewati run yang sudah benar. §7.5: IME `[perlu verifikasi]` dengan tes, dan catatan AX.
10. §7.7, §14: gate gambar dengan interval `apply`, urutan pengungkit, serta syarat spike dan keputusan pemilik sebelum CodeEditTextView, termasuk dependensi transitifnya.
11. §5.2, §5.3, G8: perbaikan Nord di V-12 (butir 10), warna parameter sama dengan `SQLScanner` (butir 11), dan G8 untuk ketujuh kanvas.
12. §8.5: W10-T7 masuk lane FFI.
13. §9 G2: tanpa reset, bias ke pemisah, dua tingkat perbandingan (sebelum dan sesudah konvergensi).
14. §11, §13: W3-T2 dua commit, SEC di commit 1, dan urutan W3-T0.
15. §12: `THIRD-PARTY-NOTICES.md` di commit A; `PerfSignposts.swift` dan `BenchMode.swift` di commit B; verifikasi B untuk dua interval.
16. §14, §15: risiko MySQL naik menjadi celah Safe Mode, W3-T0 dirumuskan, pemberitahuan pihak ketiga dibagi ke W4-T2 A dan W14, dan pertanyaan pemilik dijawab.

### Untuk orkestrator (dokumen yang bukan milik AR)

**PRD `prd-performance-and-parity.md`:**
- §11.1: baris O-14 (30 Sep 2026: tree-sitter untuk analisis editor; perubahan tampilan boleh, dengan tampilan yang disetel ulang).
- §11.2: empat keputusan orkestrator (vendoring grammar, garis bawah sintaks mati, parameter `literal`, Nord di V-12) dan D-21.
- §11.2 P-10: "di `qh-sql` di atas mesin `scan.rs` (`walk`, region opaque) dan lexer kode `qh_sql::lex`; pohon tree-sitter tidak dipakai".
- §6.5: baris V-12 (butir 1–11 di §5.3, W4-T2 commit B), dan kalimat "Atribut editor harus identik per rentang" diberi pengecualian V-12.
- §6.3 NFR-L: `tree-sitter` =0.26.13, `tree-sitter-language` =0.1.7, `streaming-iterator`, dan grammar C yang di-vendor (MIT, di luar jangkauan `cargo deny`, dicatat di `PROVENANCE.md` dan `THIRD-PARTY-NOTICES.md`). Paket SwiftPM tetap "tidak ada yang direncanakan"; CodeEditTextView hanya lewat Fase 8.
- §5 FR-ED-06: galat sintaks dari pohon tidak ditampilkan secara default (positif palsu 14–47% di SQL valid).
- §6.1 NFR-P5: ambang berlaku untuk giliran ketikan dan giliran `apply` masing-masing.
- §6.4 NFR-S1/NFR-S6: tes `safe_mode.rs` untuk `\'` dan `#` di MySQL (W3-T0).
- §9 kriteria rilis: `THIRD-PARTY-NOTICES.md` lengkap dan terpasang di bundle (W14).
- §13 Risiko: C di dalam proses (grammar dan runtime), diterima dan dicatat di ADR-0033.

**`performance-plan.md`:**
- §8 4B ditulis ulang ringkas: `crates/qh-editor` + `crates/qh-sql-grammar`, satu pohon per statement `scan.rs`, atribut sementara, plafon 2M; gate "paritas lexer 100%" diganti G1–G4 dan V-12; interval `apply`; kalimat risiko find diganti rujukan ke tabel pemilik kunci (§7.4 blueprint).
- §12 butir kedua: CodeEditTextView hanya sesudah spike gambar-baris-berwarna < 50% dan keputusan pemilik; dependensi transitif TextStory (BSD-3-Clause), swift-collections (Apache-2.0), dan SwiftLintPlugin.
- §13: baris tree-sitter menjadi "Ya (O-14)".
- §14 butir 8 dan 12: warna tetangga dengan aturan §7.5; warna parameter sama dengan `SQLScanner`.
- §15: ADR-0033 "Analisis editor dengan tree-sitter per statement di Rust".
- §17: dependensi baru; baris "Ditolak" (tree-sitter tidak lagi ditolak; SwiftTreeSitter dan `main` upstream ditolak, dengan alasan §1.2 dan D-3).
- §18: risiko Safe Mode MySQL (W3-T0).

**`development-plan.md`:**
- §4 baris `fase-4b-editor-analysis.md`: isi baru (tree-sitter per statement, atribut sementara, `walk`/`lex`, pemilik kunci sementara, W3-T0). Tabel ADR: 0033 berganti judul.
- §5 W3: tugas baru **W3-T0** (Safe Mode MySQL: `crates/qh-sql/src/{scan.rs,classify.rs}`, tesnya, `crates/qh-ffi/tests/safe_mode.rs`; GP-o; gate SEC, DB, CR; G-RUST, G-LIVE MySQL; ukuran S–M; sebelum W3-T2 langkah 2).
- §5 W3-T2: ukuran L; berkas §11; dua commit (commit 1 dengan SEC); G-DENY; bergantung pada W3-T0; `LexerFixtureExport.swift` dan `UnicodeTableExport.swift` dihapus dari daftar; alasan GP-o diganti (batas Safe Mode, C, dan konvergensi, bukan semantik regex).
- §5 W4-T2: berkas §12 (A: `THIRD-PARTY-NOTICES.md`; B: `PerfSignposts.swift`, `BenchMode.swift`, `SyntaxPaletteTests.swift`, `EditorRotorTests.swift`); gate ditambah UX dan AX; "tes paritas" diganti G7; verifikasi B dengan dua interval.
- §5 W4-T2b: `sql_statement_ranges(sql, dialect)`.
- §5 W4-D: isi ADR-0033 (D-1…D-21, grammar dan patch, kebijakan diagnostik, pemilik kunci sementara, V-12, risiko C, MSRV).
- §5 W10-T6: `Support/EditorAnalysis.swift`; sambungan dialek per tab; garis bawah lewat kunci sementara `.underlineStyle`/`.underlineColor`; galat sintaks pohon mati.
- §5 W10-T7: tambah `crates/qh-editor/src/brackets.rs`, `crates/qh-ffi/src/editor.rs`, `app/Generated/`; penggambaran di `drawBackground`; G-FFI.
- §5 W11-T6: `crates/qh-editor/src/refs.rs` menggantikan `crates/qh-sql/src/editor/`.
- §5 W12-T1: token dari `walk` + `qh_sql::lex`, daftar kata klausa sendiri, tanpa pohon.
- §5 W14: pemberitahuan pihak ketiga lengkap dan terpasang di bundle.
- §6: W3-T0 memakai opus.
- §7 rantai kepemilikan: `crates/qh-sql/src/scan.rs` dan `classify.rs` W3-T0 → W3-T2; `Cargo.toml`/`Cargo.lock` W3-T1 dan W3-T2 berurutan, tidak paralel; lane FFI … W6-T1 → **W10-T7** → W11-T1 …; `Support/EditorAnalysis.swift` W4-T2 → W10-T6 → W10-T7; `Support/PerfSignposts.swift` dan `BenchMode.swift` ditambah W4-T2.
- §10 butir 4: "batas Safe Mode (`scan.rs`), C di dalam proses, pemetaan UTF-16, dan biaya gambar baris berwarna", menggantikan "paritas lexer".

**Implikasi jangka panjang.** Keputusan yang paling menentukan, batas statement tetap di `scan.rs`, adalah yang membuat pilihan tree-sitter aman. Pohon boleh salah membaca MySQL dan Trino tanpa pernah menentukan apa yang dikirim ke server, dan grammar bisa diganti tanpa menyentuh engine. Utang yang sengaja diambil ada tiga: 17 MB C yang di-vendor dan harus diperbarui lewat gate §3; cakupan grammar yang lemah untuk MySQL dan Trino, yang ditutup lexer untuk warna tetapi tidak untuk struktur; dan biaya gambar baris berwarna yang belum punya jawaban selain eskalasi mahal. Temuan terbesar pemeriksaan ini bukan milik 4B: pemindai yang menjadi satu-satunya jawaban "di mana statement berakhir" belum mengenal MySQL, dan karena `walk` akan dipakai bersama engine dan editor, perbaikan W3-T0 langsung berlaku untuk keduanya.
