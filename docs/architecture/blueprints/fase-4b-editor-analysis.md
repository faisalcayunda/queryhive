# Blueprint Fase 4B: analisis editor di Rust

- **Status:** blueprint W2-A2, 30 Sep 2026. Belum ada kode. Diperiksa `architect-reviewer` 30 Sep 2026: **disetujui dengan koreksi**, yang sudah diterapkan di dokumen ini dan ditandai "(koreksi AR)". Verdict ada di bagian terakhir.
- **Untuk:** W3-T2 (inti Rust, fixture yang diekspor dari Swift) dan W4-T2 (FFI dan integrasi Swift).
- **Sumber:** `performance-plan.md` §8 (4A dan 4B); PRD FR-PERF-04, FR-ED-09, FR-ED-06 (hanya titik kait), NFR-V, NFR-P5, P-10; kebutuhan W11-T6 di `development-plan.md` §5; dan kode di pohon ini pada `9892ddb`.
- **Cara membaca klaim.** Setiap klaim tentang kode menyebut berkas dan barisnya. Perilaku regex di §3 sudah dijalankan ulang terhadap pola aslinya dengan skrip Swift kecil di scratchpad, dan hasilnya dikutip apa adanya. Yang belum terverifikasi ditandai `[perlu verifikasi]`.
- **Kontrak.** Aturan token (§3), aturan statement dan lipatan (§4), desain inkremental (§5), dan API FFI (§6) mengikat W3-T2 dan W4-T2. Struktur Swift di §7 mengikat sejauh 4A (W2-T3) tidak mengubah titik kaitnya. Bila 4A mendarat dengan bentuk lain, bentuk 4A yang berlaku untuk Swift, sedangkan kontrak Rust dan FFI di sini tetap.

## Ringkasan

Hari ini setiap ketikan menjalankan satu regex atas seluruh dokumen (`Views/SQLSyntax.swift:79-101`), memindai ulang statement dan lipatan (`Views/SQLEditor.swift:666-691`, `Support/SQLFolding.swift:89-134`), dan mematikan pewarnaan di atas 200.000 unit UTF-16 (`SQLSyntax.swift:36`, `SQLFolding.swift:47`). 4B memindahkan analisis itu ke `crates/qh-sql` sebagai dokumen inkremental:

- Rust memegang cermin teks dalam UTF-8, beserta indeks baris dan indeks potongan untuk pemetaan UTF-16.
- Lexer warna adalah port regex apa adanya, termasuk keanehannya. Checkpoint dipasang per 64 baris atau 4 KB, dan lex ulang berhenti begitu state konvergen.
- Statement diambil dari mesin `scan.rs` yang sudah dipakai engine. Lipatan adalah port `SQLFolding`.
- Swift hanya mengirim edit dan menerapkan hasilnya: rentang kotor yang beririsan dengan layar (±2 layar) lebih dulu, sisanya dalam potongan saat idle, dan hanya bila revisinya masih terkini.

Plafon naik ke 2.000.000 unit UTF-16. Tidak ada tree-sitter dan tidak ada dependensi baru. Regex Swift baru dihapus setelah tes paritas lewat FFI lulus.

## 1. Keputusan desain

| # | Keputusan | Alasan |
|---|---|---|
| D-1 | **Lexer warna meniru regex byte demi byte, termasuk keanehannya** (§3.5). | NFR-V menuntut atribut editor identik per rentang. Memperbaiki keanehan (misalnya `a+-- c` yang bukan komentar) berarti mengubah piksel, dan itu harus menjadi perubahan V tersendiri dengan rekam ulang baseline, bukan efek samping 4B. |
| D-2 | **Dua mesin status di satu modul.** Mesin warna (semantik regex) dan mesin statement (`scan.rs`, semantik engine). | Keduanya memang berbeda hari ini: regex tidak mengenal `$tag$`, dan memperlakukan kutip tak tertutup sebagai kode. Statement harus mengikuti engine (`performance-plan.md` §8: "Batas statement dari `scan.rs`"). Menyatukan keduanya akan melanggar D-1 atau salah memecah statement. |
| D-3 | **Cermin teks di Rust dalam UTF-8** (`Arc<String>`), dengan indeks baris LF dan indeks potongan (UTF-16, byte) setiap ≤ 1 KB. | Lexer bekerja atas byte. Swift mengirim edit dalam UTF-16. Indeks potongan menjaga pemetaan tetap O(1 KB) walaupun ada satu baris 2 MB (dump `INSERT` bergaya MySQL). |
| D-4 | **Lexer "selesaikan dulu, baru emit".** Token multi-baris (komentar blok, string, identifier berkutip) dicari penutupnya sebelum diemit. Tidak ada state tentatif yang disimpan. | Aturan EOF regex bergantung pada ada tidaknya penutup di sisa dokumen (§3.3). State tentatif yang kemudian dibatalkan adalah sumber bug inkremental yang paling licin. |
| D-5 | **Konvergensi hanya di checkpoint ber-mode Normal.** | Di checkpoint Normal, hasil lex sesudahnya hanya bergantung pada teks sesudahnya dan satu bit `prev_word`. Di dalam token, ujung token bisa berubah walaupun pembukanya sama. |
| D-6 | **Statement dan lipatan dihitung penuh per revisi, off-main, setelah debounce.** Tidak inkremental. | Satu lintasan `scan.rs` atas 2 MB hanya beberapa milidetik di latar. Kode inkremental untuk ini tidak sebanding dengan hematnya. |
| D-7 | **Dua kunci, dan main thread tidak pernah memegang kunci lexer.** | `replace` di main harus O(edit). Lex ulang besar (kutip yang membalik pasangan) boleh lama, tetapi hanya di latar. |
| D-8 | **QoS berasal dari antrean Swift, bukan dari pool Rust.** Panggilan UniFFI sinkron mewarisi QoS thread pemanggil. | Tidak perlu runtime `qh-rt`. `USER_INITIATED` untuk jendela terlihat dan `UTILITY` untuk sisanya terpenuhi oleh dua `DispatchQueue` serial. |
| D-9 | **Penjaga IME ada di sisi penerapan, bukan di sisi pengiriman.** Edit selalu dikirim, termasuk saat `hasMarkedText`, tetapi atribut tidak pernah diterapkan selama ada marked text. | **Menyimpang dari teks `performance-plan.md` §8** ("tidak mengirim edit selama `hasMarkedText`"). Alasannya ada di §7.5. **Diterima AR.** Alternatif rencana membuat cermin tertinggal selama komposisi, sehingga deteksi drift (§7.5) menembak palsu setiap kali ada scroll di tengah komposisi, dan butuh kode penggabung edit. Deviasi ini dicatat di ADR-0033. |
| D-10 | **Tabel Unicode diekspor dari ICU dan Foundation lewat Swift, lalu dibekukan** di `unicode_tables.rs`. | `\b` dan `\d` di ICU bergantung pada properti Unicode yang tidak diekspos `std` Rust dengan persis (§3.4). Setelah regex dihapus, tabel itu menjadi definisi. **Diterima AR**, dengan syarat asal-usul tercatat dan eksportir tabel tetap hidup (§3.4, koreksi AR). |
| D-11 | **Indeks baris Swift dari 4A dipertahankan untuk ruler.** Indeks Rust dipakai Rust sendiri dan diekspos untuk pemeriksaan drift. | Ruler butuh jawaban sinkron di main. Mengganti sumbernya ke FFI akan menyentuh `LineNumberRulerTests` di luar daftar berkas W4-T2. Dua indeks yang dibandingkan di tes adalah detektor drift yang murah. |
| D-12 | **Tanpa tree-sitter, tanpa crate baru.** Uji acak memakai SplitMix64 buatan sendiri. `proptest` tidak dipakai. | `performance-plan.md` §8 dan brief. `cargo deny` tidak perlu disentuh. |
| D-13 | **W4-T2 menjadi dua commit** (§13). Commit A menambah FFI dan tes paritas tanpa mengubah perilaku, commit B mengganti jalur dan menghapus regex. | Gate "paritas lulus **sebelum** regex dihapus" menjadi bukti di riwayat git, bukan sekadar klaim di laporan. **Diterima AR**, sah menurut `development-plan.md` §0 butir 3 (sub-fase dengan gate sendiri). Syaratnya: A meninggalkan pohon hijau (G-RUST, G-FFI, G-SWIFT, G-VIS), `EditorAnalysis` belum disambungkan ke `SQLEditor` di A, dan kedua commit masuk dalam satu tugas tanpa tugas lain di antaranya, kecuali W4-T2b (§15.1). |
| D-14 | **Hasil `analysis` adalah `uniffi::Record` dengan larik `u32` datar, bukan paket byte buatan tangan** (koreksi AR, §6.2). | Lifting UniFFI terjadi di thread pemanggil, yaitu antrean latar, sehingga biaya main thread sama saja. Binding diregenerasi bersama Rust (invariant #1), jadi magic, versi format, dan field cadangan tidak menjaga apa pun. `statements` dan `folds` sudah berupa record. Dengan begini ada satu gaya encoding, tanpa parser biner di Swift. |

## 2. Asumsi atas 4A (W2-T3)

4B dibangun di atas keadaan sesudah 4A. Yang diasumsikan, sesuai `performance-plan.md` §8 butir 4A:

1. TextKit 1 eksplisit dan `allowsNonContiguousLayout = true`. Temporary attribute find bar tetap ada di `NSLayoutManager`.
2. Lipatan lewat delegate layout manager, dengan tinggi terlipat yang sama, 0,1 pt (`SQLFoldStyler.collapsedLineHeight`, `SQLEditor.swift:918`). **Tidak ada atribut lipatan di text storage.**
3. Tidak ada lagi `setAttributes` atas seluruh dokumen. 4A menerapkan atribut hanya di rentang kotor dan terlihat, dengan statement yang diedit sebagai rentang kotornya.
4. Model sync di-debounce 150 ms. `updateNSView` membandingkan revisi, bukan string.
5. Indeks baris ruler inkremental, diperbarui dari `textStorage(_:didProcessEditing:range:changeInLength:)`.
6. `statementRanges`, region lipatan, dan run mark dihitung off-main setelah debounce.

**Bila salah satu tidak benar saat W4-T2 mulai:**

- (2) gagal: fungsi apply 4B (§7.4) harus menerapkan ulang atribut lipatan di rentang yang sama sesudah warna. Kalau tidak, ketikan pertama membuka lipatan secara visual.
- (3) gagal: himpunan kunci atribut yang disentuh 4B harus sama dengan milik 4A, supaya lapis atribut `VisualParityTests` tetap identik.
- (5) gagal: ruler memakai indeks Rust (`line_count`, `line_of`), dan D-11 gugur.

W4-T2 membaca kode 4A yang sudah mendarat sebelum menulis satu baris pun, lalu mencatat asumsi mana yang berlaku di laporannya.

## 3. Aturan token yang di-port

### 3.1 Pola, urutan, dan cara memindai

Pola di `SQLSyntax.swift:30-32` (`NSRegularExpression`, jadi ICU, dengan opsi kosong):

```
(--[^\n]*|/\*[\s\S]*?\*/)|('(?:[^']|'')*')|("(?:[^"]|"")*"|`[^`]*`)|(\b\d+(?:\.\d+)?\b)|([A-Za-z_][A-Za-z0-9_$]*)|([-+*/%=<>!|,;().\[\]]+)
```

Semantik pemindaian yang direproduksi:

1. **Kiri ke kanan, tanpa tumpang tindih.** Di posisi p, keenam alternatif dicoba berurutan. Yang **pertama** berhasil menang, bukan yang terpanjang. Percobaan berikutnya mulai di akhir match itu.
2. **Tidak ada yang cocok di p:** p maju satu code point, dan karakter itu tidak mendapat token. Di Rust itu satu `char`, yaitu 1 unit UTF-16, atau 2 untuk karakter di luar BMP.
3. **Kelas.** Alternatif 1 menjadi Comment, satu-satunya token yang membawa font italic (`SQLSyntax.swift:54-57`). Alternatif 2 String. Alternatif 3 QuotedIdentifier, untuk kedua jenis kutip. Alternatif 4 Number. Alternatif 5 kata, diklasifikasi lagi di §3.2. Alternatif 6 Punctuation.

| Alt | Pola | Aturan port |
|---|---|---|
| 1a | `--[^\n]*` | Dari `--` sampai sebelum LF (0x0A) atau EOF. CR ikut di dalam token. Selalu berhasil. |
| 1b | `/\*[\s\S]*?\*/` | Tertutup di `*/` pertama yang **mulai** di ≥ pembuka + 2, jadi `/*/` belum tertutup. Tidak bersarang. Tanpa penutup: §3.3. |
| 2 | `'(?:[^']\|'')*'` | Pasangan `''` adalah escape. Tertutup di `'` pertama yang tidak diikuti `'`. Tanpa penutup: §3.3. |
| 3a | `"(?:[^"]\|"")*"` | Sama dengan alternatif 2, untuk `"`. |
| 3b | `` `[^`]*` `` | **Tanpa** escape ganda. ``` `a``b` ``` menjadi dua token, `` `a` `` dan `` `b` `` (terverifikasi). |
| 4 | `\b\d+(?:\.\d+)?\b` | `\d` berarti Unicode Nd, bukan hanya ASCII, jadi `٣` adalah angka (terverifikasi). Backtracking: coba dengan pecahan lebih dulu. Bila `\b` di akhir gagal, coba tanpa pecahan. Bila masih gagal, alternatif ini gagal di p. `\b` dijelaskan di §3.4. |
| 5 | `[A-Za-z_][A-Za-z0-9_$]*` | ASCII saja. Tidak ada `\b` di depannya, jadi `1abc` menghasilkan `abc` sebagai kata. |
| 6 | `[-+*/%=<>!\|,;().\[\]]+` | Lari rakus atas karakter `- + * / % = < > ! \| , ; ( ) . [ ]`. Lari ini **tidak** berhenti di `--` atau `/*` (§3.5). |

Titik dua (`:`), `@`, `#`, `?`, `^`, `&`, `~`, `{`, `}`, `$` yang berdiri sendiri, spasi, tab, CR, LF, dan semua huruf non-ASCII tidak cocok alternatif mana pun. Karakter-karakter itu dilewati dan tetap berwarna dasar.

### 3.2 Klasifikasi kata

- **Keyword.** `lowercased` ASCII dari kata ada di daftar 131 kata (`SQLSyntax.swift:12-27`). Sumber kebenarannya pindah ke `crates/qh-sql/src/editor/mod.rs` (`KEYWORDS`, terurut) di W3-T2. Swift membacanya lewat `editorKeywords()` sejak W4-T2 commit B. W3-T2 menguji kesamaan kedua daftar terhadap `keywords.txt` hasil ekspor.
- **Literal.** `null`, `true`, `false` adalah keyword yang diwarnai sebagai Literal (`:113-115`).
- **Function.** Hanya untuk kata yang **bukan** keyword. Syaratnya: sesudah akhir kata ada nol atau lebih U+0020, lalu `(` (`isCalled`, `:105-109`). Tab, LF, dan komentar memutus rantai itu, jadi `foo\t(` bukan fungsi (terverifikasi). Karena keyword dicek lebih dulu, `replace(`, `left(`, dan `values (` tetap Keyword. Lookahead ini tidak pernah melewati LF.
- **Lainnya** tidak mendapat token. Di Rust ia menjadi kelas internal `Word`, yang dipakai W11-T6 dan formatter tetapi tidak pernah dikirim ke Swift.

### 3.3 Aturan EOF (terverifikasi dengan pola asli)

Pembuka yang tidak punya penutup **tidak** menjadi token sampai EOF. Hasilnya bergantung pada jenis pembuka, dan semuanya akibat backtracking ICU:

| Pembuka | Tanpa penutup | Input → token hari ini |
|---|---|---|
| `'`, tanpa pasangan `''` sesudahnya | `'` dilewati tanpa token. Lex lanjut di karakter berikutnya **sebagai kode**. | `x 'abc` → `x`, `abc` |
| `'`, dengan ≥ 1 pasangan `''` sebelum EOF | String ditutup di kutip **pertama** dari pasangan `''` **terakhir**. Kutip kedua pasangan itu gagal sebagai pembuka, lalu dilewati. | `select 'it''s` → `select`, `'it'`, `s` · `'''` → `''` |
| `"` | Sama dengan `'` | `"ab""c` → `"ab"`, `c` |
| `` ` `` | Dilewati. Lex lanjut di karakter berikutnya. | — |
| `/*` | Alternatif 1 gagal, dan alternatif 6 menang di posisi yang sama. `/*` beserta tanda baca yang menempel menjadi Punctuation, lalu sisanya dilex sebagai kode. | `x /* open` → `x`, `/*` (punct), `open` |
| `--` | Selalu tertutup, di LF atau EOF | — |

**Bentuk mesinnya.** Saat bertemu `'` atau `"`, lexer menelusuri kutip sejenis. Pasangan `qq` diteruskan, dan posisi pasangan terakhir dicatat. `q` yang tidak diikuti `q` menutup token. Bila penelusuran habis di EOF, aturan tabel di atas yang berlaku. Karena sesudah pembuka yang gagal tidak ada lagi kutip sejenis (setiap kutip sesudahnya pasti sudah menjadi bagian pasangan), paling banyak ada **satu** pembuka gagal per jenis kutip di seluruh dokumen. Untuk `/*` bisa lebih dari satu: semua `/*` sesudah `*/` terakhir gagal.

**Konsekuensi non-lokal.** Satu `'` yang diketik di baris 10 bisa berpasangan dengan `'` di baris 40.000, dan itu membalik pasangan semua string sesudahnya. Itulah perilaku hari ini, dan 4B mereproduksinya (§5.4).

### 3.4 Batas kata ICU (`\b`) dan kelas Unicode

`\b` di ICU, mode default (bukan `UREGEX_UWORD`), di posisi p:

1. Bila p = EOF, karakter sesudahnya dianggap bukan kata.
2. Bila karakter **di** p adalah `Grapheme_Extend` atau `Cf`, p **bukan** batas.
3. Karakter sebelumnya dicari dengan mundur melewati karakter `Grapheme_Extend` dan `Cf`. Yang pertama bukan keduanya dipakai. Awal teks dihitung bukan kata.
4. Batas = (sesudah ∈ W) XOR (sebelum ∈ W), dengan W = `[\p{Alphabetic}\p{M}\p{Nd}\p{Pc}‌‍]`.

Ini ditulis dari ingatan atas `RegexMatcher::isChunkWordBoundary` di ICU, lalu diperiksa dengan pola `\b\d+\b` di mesin ini:

| Input (code point) | Hasil |
|---|---|
| `a` U+0301 `1` | tidak ada angka. Tanda kombinasi dilewati mundur, lalu `a` adalah kata. |
| `x` U+200D `1` | tidak ada angka. ZWJ adalah `Cf`. |
| `٣` | angka `{0,1}` |
| `$1` | angka `{1,1}`. `$` bukan W. |
| `1` U+0301 | tidak ada angka. Karakter sesudahnya `Grapheme_Extend`. |

Mundur di langkah 3 tidak pernah melewati LF, karena LF bukan `Grapheme_Extend` dan bukan `Cf`. Jadi semua lookbehind dan lookahead lexer warna lokal di dalam baris. Hanya mode token multi-baris yang membawa state melintasi LF.

**Tabel yang diekspor** (D-10). `LexerFixtureExport.swift` menulis `crates/qh-sql/src/editor/unicode_tables.rs`: daftar rentang code point yang terurut dan tidak beririsan, beserta kepala "generated, do not edit". Isinya:

- `ICU_WORD`: dari `\w` ICU, yang di ICU memakai himpunan yang sama dengan `\b`;
- `ICU_DIGIT`: dari `\d`;
- `ICU_TRANSPARENT`: dari `[\p{Grapheme_Extend}\p{Cf}]`;
- `FND_LETTER` dan `FND_ALNUM`: `CharacterSet.letters` dan `.alphanumerics`, **BMP saja**, untuk port `SQLFolding` di §4.2.

Pengambilan untuk kelas ICU: satu string berisi semua scalar yang dipisah `\n`, lalu satu `matches(in:)` per kelas. `\n` bukan anggota kelas mana pun, jadi setiap match adalah satu scalar. Tiga lintasan atas sekitar 4,5 juta unit ini selesai dalam hitungan detik.

`CharacterSet.whitespacesAndNewlines` setara dengan `char::is_whitespace` (properti `White_Space`). Kesetaraan ini **diuji**, tidak ditabelkan. Jalur ASCII tidak pernah menyentuh tabel.

**Pemeliharaan tabel (koreksi AR).**

- Kepala berkas mencatat asal-usulnya: versi macOS dan build, versi ICU dan Unicode yang dilaporkan sistem, tanggal, dan perintah regenerasi. Tanpa catatan itu, tabel beku tidak bisa diaudit.
- Ekspor tabel dipisah ke `app/Tests/QueryHiveTests/UnicodeTableExport.swift` (di-skip kecuali `QH_EXPORT_UNICODE=1`). Ekspor ini hanya bergantung pada `NSRegularExpression` dan `CharacterSet`, bukan pada `SQLSyntax`, sehingga **tidak** ikut dihapus di commit B (§10). Dengan begitu tabel bisa dibandingkan dengan macOS berikutnya kapan saja.
- Selisih yang ditemukan di macOS baru **tidak** otomatis diterapkan. Mengubah tabel berarti mengubah warna, jadi itu perubahan V tersendiri (PRD §6.5). `editor_unicode.rs` menguji bahwa tabel terurut, tidak beririsan, dan bahwa `ICU_DIGIT ⊂ ICU_WORD`.

### 3.5 Keanehan yang sengaja dipertahankan (semuanya terverifikasi)

| Input | Token hari ini | Catatan |
|---|---|---|
| `a+-- c` | `a`, `+--` punct, `c` | `--` tertelan lari tanda baca. Bukan komentar. |
| `count(*)--c` | `count`, `(*)--` punct, `c` | Komentar tepat sesudah `)` tidak dikenali. |
| `1;-- c` | `1`, `;--` punct, `c` | Sama. Kasus yang umum di dunia nyata. |
| `)/* x */` | `)/*` punct, `x`, `*/` punct | `/*` tertelan. |
| `$tag$ select $tag$` | `tag$`, `select`, `tag$` | Badan dollar-quote dilex sebagai kode. |
| `1abc from` | `abc`, `from` | `\b` gagal di `1`, lalu `abc` menjadi kata. |
| `1.5a` | `1` number, `.` punct, `a` | Backtracking pecahan. |
| `:name`, `a::text` | `:` dilewati, `name` kata | Lexer `:name` milik `SQLScanner` tidak tersentuh. |
| `E'it\'s'` | escape backslash tidak dikenal | Sama dengan `SQLScanner` dan `scan.rs`. |

Memperbaiki satu pun dari ini adalah perubahan V baru (PRD §6.5). Itu di luar 4B.

### 3.6 Satuan posisi

- Semua posisi yang menyeberang FFI adalah offset UTF-16 absolut dalam `u32`. Rentang ditulis setengah terbuka, `[start, start + len)`.
- Karakter non-BMP selalu dilangkahi utuh. Tidak ada token yang mulai atau berakhir di tengah pasangan surrogate.
- **Surrogate yatim.** NSString bisa memuat surrogate yatim. Saat di-bridge ke Swift `String` untuk dikirim ke UniFFI, ia menjadi U+FFFD. Panjangnya tetap 1 unit UTF-16, sehingga offset tetap sejajar. Keduanya tidak cocok alternatif mana pun dan bukan anggota W, `Grapheme_Extend`, atau `Cf`, jadi hasil lex identik. Satu kasus korpus mengunci ini.

## 4. Statement, lipatan, dan baris

### 4.1 Statement: dari `scan.rs`

- **Pemisah.** Posisi `;` dari mesin `scan()` (`crates/qh-sql/src/scan.rs:62-187`). Mesin ini mengenal kutip `'`, `"`, backtick (semuanya dengan escape ganda), `--`, `/* */` tanpa sarang, dan `$tag$` (`read_dollar_tag`, `:195-209`). Pembuka yang tidak tertutup berlaku sampai EOF.
- **Rentang statement i.** `[pemisah_(i-1) + 1, pemisah_i)` dalam UTF-16, atau sampai akhir teks untuk potongan terakhir. Ini bentuk yang sama dengan `sqlStatements` hari ini (`Models/QueryTab.swift:1056-1098`), sehingga band dan run mark tidak bergeser untuk teks biasa.
- **Potongan dipertahankan** hanya bila `first_significant(potongan)` bukan `None` (`scan.rs:270-299`). Ini aturan `statements_with_lines` (`classify.rs:566-608`).
- **Refaktor `scan.rs` (W3-T2).** Loop di `scan()` diekstrak menjadi satu fungsi jalan internal yang memanggil callback pemisah dan kata. `scan()` tetap mengumpulkan keduanya dengan keluaran yang identik, dikunci oleh tesnya sendiri. Editor hanya memakai pemisah dan state akhir, sehingga tidak mengalokasikan satu `String` per kata.
- **Syarat refaktor (koreksi AR).** `scan()` adalah dasar Safe Mode (`classify.rs` membaca `keywords` dan `separators`), jadi refaktor ini menyentuh batas keamanan:
  - `scan.rs` belum ada di daftar berkas W3-T2 di `development-plan.md`. Orkestrator menambahkannya sebelum W3-T2 mulai.
  - Tes diferensial baru: `scan()` sebelum dan sesudah refaktor menghasilkan `Scan` yang identik atas seluruh korpus editor dan alfabet acak §9. Salinan fungsi lama disimpan di modul tes untuk keperluan ini saja.
  - Gate W3-T2 ditambah SEC, khusus untuk diff `scan.rs`.
  - Alasan refaktor yang mengikat adalah state akhir (`unclosed`, §8.2) dan region opaque untuk W12-T1, bukan alokasi. Outline dihitung di latar sesudah debounce, sehingga alokasi per kata tidak menyentuh NFR-P5.

**Perbedaan terhadap perilaku hari ini.** Semuanya membuat editor sepakat dengan engine:

| Kasus | `sqlStatements` Swift hari ini | Sesudah 4B (`scan.rs`) |
|---|---|---|
| `;` di dalam `"a;b"` atau `` `a;b` `` | memecah | tidak memecah |
| `;` di dalam `$tag$ … $tag$` | memecah (diakui di komentar `:1050-1051`) | tidak memecah |
| potongan yang hanya komentar (`SELECT 1; -- akhir`) | statement sendiri, dengan band dan run mark | bukan statement |
| teks CRLF yang memuat `--` | komentar baris tidak pernah berakhir, karena loop `Character` menganggap `"\r\n"` satu karakter (terverifikasi: 0 LF terlihat pada `"-- c\r\nSELECT 1; SELECT 2"`). Tidak ada pemecahan sesudah komentar pertama. | berakhir di LF |
| potongan yang hanya NBSP, U+2028, atau VT | dibuang (trim Unicode) | dianggap signifikan (`is_ascii_whitespace`) |

Scene `editor-syntax` dan `editor-long-middle` tidak memuat satu pun kasus ini. Scene pertama hanya satu statement; scene kedua memakai `;` biasa. Jadi tidak ada piksel yang diharapkan berubah. **Run Current Statement** masih memakai `sqlStatement(in:atUTF16Offset:)` versi Swift sampai tindak lanjut di §15.1. Selama itu, band atau run mark dan Run bisa berselisih untuk kasus di tabel ini.

### 4.2 Lipatan: port `SQLFolding` apa adanya

Port 1:1 dari `regions` (`SQLFolding.swift:89-134`), `cteRegions`, `cteOpenParens`, `matchingParen` (`:153-225`), dan mini-lexer `nextToken` dan `skipTrivia` (`:300-376`). Statement yang dipakai adalah daftar dari §4.1.

- **Trim header dan akhir** memakai `whitespacesAndNewlines`, per unit UTF-16 (`isSpace`, `:265-268`). Di Rust: `char::is_whitespace`.
- **Kata.** Dimulai oleh `letters` atau `_`, dilanjutkan `alphanumerics`, `_`, atau `$` (`:378-388`). Pengecekannya per unit UTF-16, jadi karakter non-BMP tidak pernah menjadi bagian kata. Rust memakai `FND_LETTER` dan `FND_ALNUM` untuk karakter BMP, dan `false` untuk non-BMP.
- **Kata dijadikan huruf besar** dengan pemetaan Unicode penuh (`uppercased()` Swift dan `str::to_uppercase` Rust). Kata itu dipakai untuk membandingkan `WITH`, `AS`, `NOT`, `MATERIALIZED`, dan untuk ringkasan (summary). Ringkasan jatuh ke `"statement"` bila token pertama bukan kata, dan bernilai `"CTE"` untuk region CTE.
- **Aturan EOF mini-lexer ini berbeda dengan lexer warna, dan tetap berbeda.** Kutip atau komentar yang tidak tertutup berlaku sampai akhir teks statement. `matchingParen` melompati literal dengan cara yang sama.
- **Dedup dan urutan.** Header ganda dibuang: yang pertama menang, statement lebih dulu dari CTE. Urutan akhir `(header, lastLine)`.
- **Plafon** 2.000.000, inklusif (§5.7).

Semantik ini hanya bisa berbeda dari Swift lewat statement-nya (tabel §4.1). Fixture lipatan untuk teks di tabel itu masuk daftar pengecualian yang dijelaskan (§9).

### 4.3 Baris

- Hanya LF (0x0A) yang memutus baris. `line_starts` = `[0] + [i + 1 untuk setiap LF di i]`. Dokumen kosong punya 1 baris. Ini sama dengan `SQLFolding.lineStarts` (`:55-63`).
- CR yang berdiri sendiri bukan pemutus baris di indeks, walaupun AppKit menggambarnya sebagai paragraf baru. Selisih ini sudah ada hari ini dan tidak diubah (§14).

## 5. Desain inkremental

Semua tipe di bagian ini hidup di `crates/qh-sql/src/editor/`. Semuanya murni: tanpa kunci, tanpa thread, dan tanpa global selain tabel `const`, sesuai kontrak `qh-sql/src/lib.rs:16`. Konkurensi dirakit di `qh-ffi` (§7.1).

### 5.1 `TextBuffer`

```rust
pub struct TextBuffer {
    text: Arc<String>,               // CoW: Arc::make_mut saat ada snapshot yang masih dipegang
    revision: u64,                   // 1 saat new(); +1 per replace
    lines: Arc<LineIndex>,           // starts_utf16: Vec<u32>, starts_byte: Vec<u32>
    chunks: Arc<ChunkIndex>,         // titik (utf16, byte) setiap ≤ 1024 byte, di batas char
    last_closer: [Closer; 4],        // ' " ` dan */: Absent | At(byte) | Unknown (langkah 6)
    log: Vec<LogEntry>,              // dikuras oleh Highlighter, berurutan
}
pub enum LogEntry {
    Edit(Edit),                               // setiap replace
    Touch { revision: u64, start: u32, len: u32 },    // mark_dirty
    Applied { revision: u64, ranges: Vec<(u32, u32)> }, // mark_applied
}
pub struct Edit {
    pub revision: u64,                         // revisi sesudah edit
    pub start_utf16: u32, pub old_len_utf16: u32, pub new_len_utf16: u32,
    pub start_byte: u32,  pub old_len_byte: u32,  pub new_len_byte: u32,
    pub closers_inserted: u8,                  // bit per jenis: ' " ` */
    pub joins_block_close: bool,               // penghapusan yang merapatkan * dan /
}
```

**`replace(start, len, text)`, langkah demi langkah:**

1. Validasi. `start + len` ≤ panjang dokumen. Kedua ujung harus jatuh di batas char. Kalau tidak: `SplitsCharacter`.
2. Petakan UTF-16 ke byte lewat `chunks`: pencarian biner, lalu pindai ≤ 1024 byte.
3. `String::replace_range`. Biayanya memmove sampai 2 MB, sekitar 0,1 ms.
4. Perbarui `lines`. Awal baris di dalam rentang lama dibuang, awal baris dari teks baru disisipkan, dan sisanya digeser (Δbyte, Δutf16). O(jumlah baris) penjumlahan.
5. Perbarui `chunks` dengan cara yang sama. Tambahkan titik di dalam teks baru sehingga tidak ada celah > 1024 byte.
6. Perbarui `last_closer`. Kemunculan di teks baru menang bila lebih akhir. Bila kemunculan terakhir yang lama berada di rentang yang dihapus, entri itu ditandai **tidak diketahui**, dan tidak dicari di sini. Pencarian mundur bisa O(n) di main (koreksi AR). Highlighter menghitung ulang entri yang tidak diketahui dari snapshot di latar, dengan satu `rfind` per jenis, sebelum melex.
7. `revision += 1`, dorong `LogEntry::Edit`, kembalikan revisi.

**Anggaran.** Pada 2 MB, `replace` di main < 0,5 ms, termasuk satu salinan CoW 2 MB bila analisis latar sedang memegang snapshot. `log` dibatasi 1.024 entri. Bila meluap, `log` dikosongkan dan Highlighter diberi tanda `needs_full_relex`.

### 5.2 Blok checkpoint

```rust
struct Block {
    start_byte: u32, start_utf16: u32,   // absolut, digeser per edit (O(jumlah blok))
    entry: Entry,
    tokens: Vec<Tok>,                    // token yang MULAI di blok ini, tanpa Word
}
enum Entry {
    Normal { prev_word: bool },          // bit W dari karakter non-transparan sebelumnya (§3.4)
    Inside { open_byte: u32, open_utf16: u32 }, // di dalam token multi-baris yang mulai di open
}
struct Tok { start: u32 /* UTF-16, relatif ke blok */, len: u32, class: Class /* u8 */ }
```

- **Batas blok baru** dipasang sesudah ≥ 64 baris atau ≥ 4.096 byte sejak awal blok, di posisi **layak** pertama. Posisi layak adalah:
  - awal baris, dengan `Entry` apa pun;
  - di tengah baris, bila ia **awal percobaan** (bukan di dalam token), mode Normal, dan karakter di posisi itu bukan U+0020. Aturan terakhir ini mencegah lookahead `isCalled` dari kata sebelumnya menyeberangi batas blok.
- **Token milik blok tempat ia mulai.** Token yang melintasi batas blok membuat blok-blok berikutnya ber-`Entry::Inside`.
- **Memori pada 2 MB.** Sekitar 800 blok. Token tanpa `Word` kira-kira 300–500 ribu × 12 byte, jadi 4–6 MB, ditambah teks 2 MB.

### 5.3 Lexer "selesaikan dulu, baru emit"

```rust
pub struct Lexer<'a> { /* text, pos_byte, pos_utf16, prev_word, closers: &[Option<u32>; 4] */ }
impl<'a> Lexer<'a> {
    pub fn new(text: &'a str, at_byte: u32, at_utf16: u32, prev_word: bool,
               closers: &'a [Option<u32>; 4]) -> Self;
    pub fn next_token(&mut self) -> Option<Token>;   // termasuk Class::Word
}
pub fn lex_all(text: &str) -> Vec<Token>;           // oracle: dari nol, tanpa blok
```

- Di pembuka `'`, `"`, backtick, atau `/*`, lexer mencari penutup lebih dulu. Pencarian dibatasi `last_closer`: bila `last_closer[k]` tidak ada atau < posisi pembuka, pembuka gagal seketika, tanpa memindai sampai EOF. Kemudian token diemit utuh, atau aturan EOF §3.3 diterapkan. Tidak ada state tentatif.
- `lex_all` adalah oracle untuk tes diferensial dan dipakai juga untuk `new()` atas dokumen kecil.

### 5.4 Titik mulai lex ulang

Untuk setiap `Edit` yang dikuras dari log, dengan s = `start_utf16` dalam koordinat baru:

1. **Blok awal** = blok terakhir yang mulai **sebelum** s (tegas, <), atau blok 0.
2. Selama `entry` blok itu `Inside { open }`, mundur ke blok yang memuat `open`. Prosesnya berhenti karena `open` selalu turun.
3. **Pembuka gagal.** Highlighter mencatat `failed_first[k]`: posisi pembuka gagal pertama untuk setiap jenis k. Bila edit menyisipkan penutup jenis k (`closers_inserted`, atau `joins_block_close` untuk `*/`) dan `failed_first[k]` < s, blok awal = min(blok awal, blok yang memuat `failed_first[k]`). Tanpa aturan ini, `'` yang diketik di baris 900 tidak akan berpasangan dengan `'` gagal di baris 5.
4. Beberapa edit dalam satu kurasan diproses berurutan. Rentang lex ulang digabung.

### 5.5 Konvergensi

- Lex ulang berjalan dari blok awal. Sesudah melewati ujung edit (s + `new_len`), setiap batas blok lama b (sudah digeser Δ) yang didarati lexer tepat sebagai awal percobaan diperiksa. Bila di sana mode baru `Normal { prev_word }` sama dengan `entry` lama `Normal { prev_word }`, lex ulang **berhenti**:
  - blok lama dari b dan seterusnya dipakai ulang;
  - `start_byte` dan `start_utf16` digeser Δ;
  - posisi `open` yang lebih besar dari ujung edit ikut digeser.
- **Kenapa aman.** Di checkpoint Normal, token sesudah b hanya bergantung pada teks sesudah b, yang tidak berubah, dan pada `prev_word`. Token sebelum b tidak bergantung pada teks sesudah b:
  - lookahead `isCalled` tidak bisa menyeberangi b, karena b bukan U+0020 atau b adalah awal baris;
  - `\b` di akhir angka hanya melihat karakter di b;
  - lari tanda baca berhenti karena karakter di b bukan tanda baca.
- **Batas `Inside` tidak pernah dipakai untuk konvergensi** (D-5).
- **Malas (lazy): ditunda (koreksi AR).** Lex ulang selalu berjalan sampai konvergen atau EOF di panggilan yang menguras log. Lex ulang O(n) hanya terjadi saat kutip membalik pasangan, dan hanya di latar, sehingga tidak menyentuh anggaran main NFR-P5; akibatnya paling-paling warna terlambat satu frame, risiko yang sudah diterima rencana. `frontier` dan bit `LEX_PENDING` baru ditambahkan bila bench W3-T2 menunjukkan `lex_all` atas 2 MB > 16 ms di mesin bench. Rancangannya bila dibutuhkan: panggilan jendela terlihat berhenti di checkpoint Normal pertama sesudah ujung jendela; token sesudahnya adalah token lama yang digeser dan tetap kotor; panggilan `UTILITY` melanjutkan dengan anggaran sekitar 256 KB per panggilan.

### 5.6 Himpunan kotor

`dirty: RangeSet<u32>` (vektor rentang UTF-16 yang terurut dan tidak beririsan) dalam koordinat revisi Highlighter. Isinya diperbarui saat log dikuras, berurutan:

- **`Edit`:** geser, lalu **selalu** tambahkan [s, s + `new_len`). Karakter yang baru masuk membawa atribut apa pun yang diberikan storage, entah typing attributes, atribut yang dipulihkan undo, atau hasil tempel. Jadi rentang edit wajib dicat ulang walaupun tokennya tidak berubah.
- **Lex ulang:** tambahkan rentang beda token. Prefiks dan sufiks token (start, len, class) yang sama antara daftar lama yang digeser dan daftar baru dibuang. Yang tersisa ditambahkan. Mengetik satu huruf di tengah biasanya mengotori satu atau dua token, bukan seluruh blok.
- **`Touch`:** tambahkan.
- **`Applied { revision, ranges }`:** kurangi, hanya bila `revision` sama dengan revisi Highlighter di titik log itu. Bila tidak, abaikan; rentangnya akan dikirim lagi.
- **Saat hasil disusun** (§6.2), rentang di-snap ke batas token sehingga tidak ada token yang terpotong.

### 5.7 Plafon

- `CEILING_UTF16 = 2_000_000`, inklusif: dokumen dengan ≤ 2.000.000 unit diwarnai. Diekspos lewat `editor_ceiling_utf16()` untuk Swift dan tes.
- **Di atas plafon:**
  - Highlighter tidak aktif. Saat melintasi plafon ke atas, seluruh dokumen ditandai kotor sekali tanpa token, dan hasil `analysis` membawa `inactive = true`. Swift mengembalikan semuanya ke warna dasar dalam potongan idle. Ini sama dengan hasil hari ini di atas 200 ribu (`SQLSyntax.swift:44`).
  - Lipatan kosong.
  - Statement **tetap** dihitung. Hari ini `statementBounds` juga tidak punya plafon (`SQLEditor.swift:673`).
- **Kembali ke bawah plafon:** lex penuh.
- **Fixture bench.** Fixture `type-2m` harus ≤ 2.000.000 unit UTF-16. Kalau lebih, pewarnaan mati dan angka NFR-P5 tidak berarti. W4-T2 memeriksanya sebelum mengukur.

## 6. API UniFFI `EditorDocument`

### 6.1 Permukaan (`crates/qh-ffi/src/editor.rs`)

```rust
#[derive(uniffi::Object)]
pub struct EditorDocument {
    text: Mutex<TextBuffer>,                       // main: replace, baris, log
    highlight: Mutex<Highlighter>,                 // hanya thread latar
    outline: Mutex<Option<(u64, Arc<Outline>)>>,   // cache statement+lipatan per revisi
}

#[uniffi::export]
impl EditorDocument {
    #[uniffi::constructor]
    pub fn new(text: String) -> Result<Arc<Self>, EditorError>;
    pub fn revision(&self) -> Result<u64, EditorError>;
    pub fn len_utf16(&self) -> Result<u32, EditorError>;
    /// Main thread. O(edit) + geser indeks. Tidak pernah melex.
    pub fn replace(&self, start_utf16: u32, len_utf16: u32, text: String) -> Result<u64, EditorError>;
    /// Thread latar. Rekaman §6.2 untuk dirty ∩ [window_start, window_start+window_len),
    /// dibatasi budget_utf16 (lunak: satu token yang lebih panjang tetap dikirim utuh).
    pub fn analysis(&self, revision: u64, window_start: u32, window_len: u32,
                    budget_utf16: u32) -> Result<EditorAnalysis, EditorError>;
    /// Main thread. `ranges` = pasangan datar (start, len) dari hasil `analysis` yang sudah diterapkan.
    pub fn mark_applied(&self, revision: u64, ranges: Vec<u32>) -> Result<(), EditorError>;
    /// Main thread. Untuk ganti font kode (seluruh dokumen) dan pemulihan.
    pub fn mark_dirty(&self, start_utf16: u32, len_utf16: u32) -> Result<(), EditorError>;
    pub fn statements(&self, revision: u64) -> Result<EditorStatements, EditorError>;
    pub fn folds(&self, revision: u64) -> Result<EditorFolds, EditorError>;
    pub fn line_count(&self) -> Result<u32, EditorError>;
    // line_of dan line_starts diekspor HANYA bila asumsi 4A no. 5 gagal (§2). Bila tidak,
    // tidak ada pemanggil di Swift, dan keduanya tetap internal (koreksi AR, YAGNI).
    pub fn line_of(&self, offset_utf16: u32) -> Result<u32, EditorError>;
    pub fn line_starts(&self, first_line: u32, max_count: u32) -> Result<Vec<u32>, EditorError>;
}

#[uniffi::export] pub fn editor_keywords() -> Result<Vec<String>, EditorError>;
#[uniffi::export] pub fn editor_ceiling_utf16() -> Result<u32, EditorError>;
/// Tanpa state: rentang statement §4.1 untuk teks apa pun. Dipakai wrapper
/// `SQLFolding.statementRanges(in:)` sekarang, dan `sqlStatements(in:)` pada tindak lanjut §15.1.
#[uniffi::export] pub fn sql_statement_ranges(sql: String) -> Result<Vec<u32>, EditorError>;
```

**Semantik yang mengikat:**

- **Revisi.** `revision` bermula 1 dan naik satu per `replace`. `mark_*` tidak mengubahnya.
- **`Stale`.** `analysis`, `statements`, dan `folds` mengembalikan `Stale` bila argumen `revision` < revisi teks saat panggilan masuk. Hasilnya selalu membawa revisi tempat ia dihitung. Revisi itu bisa lebih baru dari argumen bila edit masuk di antaranya; Swift yang memutuskan (§7.4).
- **`analysis` menguras log sebelum melex.** Ia:
  1. mengunci `highlight`, lalu `text` sebentar untuk snapshot `Arc` dan pengurasan log;
  2. melepas kunci `text` dan melex ulang sampai ujung jendela resolved (§5.5);
  3. menyusun `EditorAnalysis`, lalu melepas kunci.
- **Bukan perintah engine.** `EditorDocument` adalah objek UniFFI, bukan `EngineCommand`. Invariant #11 (empat daftar) **tidak** tersentuh. Invariant #1 berlaku: `app/Generated/` diregenerasi dan di-commit bersama perubahan Rust-nya.
- **Urutan kunci (koreksi AR).** `text` adalah kunci daun: selama memegangnya, tidak ada kunci lain yang diambil. Urutan yang sah hanya `highlight → text` dan `outline → text`. Main hanya mengambil `text`.

### 6.2 Rekaman `analysis` (koreksi AR, D-14)

Draf awal memakai paket byte little-endian (magic `QHEA`, versi, flags, padding) yang di-parse tangan di Swift. AR menggantinya dengan rekaman UniFFI. Isi dan invariannya tetap; hanya encoding-nya yang berubah.

```rust
#[derive(uniffi::Record)] pub struct EditorAnalysis {
    pub revision: u64,          // revisi tempat hasil dihitung
    pub doc_len_utf16: u32,     // panjang dokumen pada revisi itu
    pub window_start: u32,      // gema argumen setelah di-clamp
    pub window_len: u32,
    pub inactive: bool,         // di atas plafon: tanpa token, rentang dicat warna dasar
    pub more_in_window: bool,   // masih ada rentang kotor di jendela, di luar anggaran
    pub dirty_elsewhere: bool,  // ada rentang kotor di luar jendela
    pub ranges: Vec<u32>,       // pasangan datar (start, len), len ≥ 1
    pub tokens: Vec<u32>,       // tripel datar (start, len, class), len ≥ 1
}
```

**Invarian yang dijamin Rust dan dikunci L3:**

- `ranges`: terurut naik, tidak beririsan, di dalam [0, `doc_len_utf16`]. Inilah rentang yang dicat ulang Swift, lalu digemakan ke `mark_applied`.
- `tokens`: terurut menurut `start`, tidak beririsan, dan **setiap token berada utuh di dalam tepat satu rentang**.
- Panjang `ranges` genap dan panjang `tokens` kelipatan tiga.

**Kelas.** `1` Comment, `2` String, `3` QuotedIdentifier, `4` Number, `5` Keyword, `6` Literal, `7` Function, `8` Punctuation. Kodenya didefinisikan sekali di `Class` (`#[repr(u8)]`, `editor/mod.rs`). Swift memetakannya di satu `switch` dengan cabang `default` yang memicu fault. Ini enum tertutup di dua sisi batas (invariant #3), dan dicatat di ADR-0033. `Word` tidak pernah dikirim.

**Contoh yang dikunci tes W3-T2.** `EditorDocument::new("select 1")`, lalu `analysis(1, 0, 8, u32::MAX)`, menghasilkan `revision = 1`, `doc_len_utf16 = 8`, `window = (0, 8)`, ketiga flag `false`, `ranges = [0, 8]`, dan `tokens = [0, 6, 5, 7, 1, 4]`.

**Pemeriksaan di Swift** (`EditorAnalysisResult.validate`, fungsi murni, dijalankan di antrean latar):

- panjang larik, keterurutan, batas terhadap `doc_len_utf16`, token di dalam rentang, dan kelas 1–8;
- hasil yang tidak sah dibuang dengan `os_log(.fault)`, lalu dokumen dibuat ulang (§7.6). Tidak ada fallback diam. Pemeriksaan ini tetap perlu, karena `addAttributes` di luar batas melempar `NSRangeException`.

**Biaya dan jalan mundur.** Lifting `[UInt32]` terjadi di antrean latar. W4-T2 mengukurnya di bench FFI in-process (`b810592`) untuk jendela 131.072 unit. Bila p99 lifting > 2 ms, `tokens` dan `ranges` boleh diganti satu `Vec<u8>` berisi `u32` little-endian datar, tanpa magic, versi, atau padding, dan tanpa gate AR baru. Field lain tetap di rekaman.

### 6.3 Rekaman

```rust
#[derive(uniffi::Record)] pub struct EditorStatements {
    pub revision: u64,
    pub ranges: Vec<u32>,                  // pasangan datar (start, end) UTF-16, §4.1
    pub unclosed: Option<EditorUnclosed>,  // state akhir scan.rs; titik kait FR-ED-06 (§8.2)
}
#[derive(uniffi::Record)] pub struct EditorUnclosed { pub kind: EditorOpaqueKind, pub at_utf16: u32 }
#[derive(uniffi::Enum)]   pub enum EditorOpaqueKind { SingleQuote, DoubleQuote, Backtick, BlockComment, DollarQuote }
#[derive(uniffi::Record)] pub struct EditorFolds { pub revision: u64, pub folds: Vec<EditorFold> }
#[derive(uniffi::Record)] pub struct EditorFold {
    pub kind: EditorFoldKind, pub header_line: u32, pub last_line: u32,
    pub header: u32, pub body_start: u32, pub body_end: u32, pub summary: String,
}
#[derive(uniffi::Enum)]   pub enum EditorFoldKind { Statement, Cte }
```

`statements` dan `folds` dihitung dalam **satu** lintasan (lipatan butuh statement), dari snapshot teks tanpa kunci lexer. Hasilnya di-cache per revisi di `outline`.

### 6.4 Galat, dan aturan untuk gate SEC

```rust
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum EditorError {
    #[error("revision {asked} is not current ({current})")]              Stale { asked: u64, current: u64 },
    #[error("{start}+{len} is outside a document of {doc_len} UTF-16 units")] OutOfBounds { start: u32, len: u32, doc_len: u32 },
    #[error("offset {offset} splits a character")]                        SplitsCharacter { offset: u32 },
    #[error("a document of {len_utf16} UTF-16 units cannot be addressed")] TooLarge { len_utf16: u64 },
    #[error("{reason}")]                                                  Malformed { reason: String },
}
```

- **Semua ekspor mengembalikan `Result`**, termasuk getter dan fungsi bebas, sehingga pemanggil Swift wajib `try`.
- Tidak ada `unwrap`/`expect` pada jalur yang bergantung pada input. Mutex yang poisoned dipetakan ke `Malformed`.
- `TooLarge` hanya untuk panjang > `u32::MAX - 1`.
- `mark_applied` dengan jumlah elemen ganjil atau rentang di luar batas: `Malformed` atau `OutOfBounds`.
- Panic tetap ditangkap UniFFI (`panic = "unwind"`, `performance-plan.md` temuan 12). Di Swift ia diperlakukan sama dengan `Malformed`.
- Tidak ada buffer mentah yang menyeberang. Swift hanya mengirim `String` dan `Vec<u32>` ke Rust, dan hanya menerima rekaman UniFFI (D-14).

## 7. Threading dan penerapan di Swift

### 7.1 Kunci

| Kunci | Dipegang oleh | Lama |
|---|---|---|
| `text` | main (`replace`, `line_*`, `mark_*`, `len_utf16`); latar (snapshot dan pengurasan log) | mikrodetik |
| `highlight` | **hanya** antrean latar | biasanya < 1 ms. Lex ulang O(n) saat kutip membalik pasangan bisa belasan milidetik pada 2 MB, dan hanya di latar (§5.5) |
| `outline` | antrean latar | mikrodetik (cache) |

**Main thread tidak pernah menyentuh `highlight`.** `mark_applied` dan `mark_dirty` hanya menulis ke log di bawah kunci `text`. Inversi prioritas (panggilan terlihat yang menunggu potongan idle) dibatasi oleh anggaran pengemasan `UTILITY` (16.384 unit). Lex ulang besar hampir selalu dibayar oleh panggilan terlihat sesudah ketikan, karena panggilan itulah yang pertama menguras log. Harga terburuknya warna yang terlambat, bukan main yang tertahan.

### 7.2 Antrean (D-8)

- `DispatchQueue(label: "qh.editor.visible", qos: .userInitiated)`, serial: permintaan jendela terlihat, **dikoalesi** (paling banyak satu yang antre dan satu yang berjalan; yang antre membaca revisi terbaru saat mulai).
- `DispatchQueue(label: "qh.editor.idle", qos: .utility)`, serial: potongan idle, kelanjutan `frontier`, dan outline (statement dan lipatan) sesudah debounce 4A (150 ms bila 4A tidak menetapkan angka lain).

### 7.3 Alur satu ketikan

```
main   didProcessEditing(.editedCharacters, range, delta)
         ├─ lebarkan ke batas surrogate (§7.5)
         ├─ rev = doc.replace(start, oldLen, newText)          ≤ 0,5 ms pada 2 MB
         ├─ geser cache statement, run mark, header lipatan (O(k), per edit)
         └─ requestVisible()                                    → visible queue
visible  pkt = doc.analysis(rev, jendela ±2 layar, 131_072)    biasanya < 1 ms
         ├─ validate(pkt)                                       di latar (§6.2)
         └─ main.async { apply(pkt) }
main   apply: bila pkt.revision == revision && !hasMarkedText && docLen == storage.length
         ├─ cat rentang: dasar, lalu token
         ├─ doc.markApplied(pkt.revision, pkt.ranges)
         └─ bila more_in_window || dirty_elsewhere → jadwalkan potongan idle
idle   pkt = doc.analysis(rev, 0, len, 16_384)                  → main.async { apply }
idle   (debounce) doc.statements(rev), doc.folds(rev)           → main.async { band, run mark, lipatan, rotor }
```

- **Jendela ±2 layar.** Rentang karakter terlihat diambil dari `glyphRange(forBoundingRectWithoutAdditionalLayout:in:)` supaya tidak memaksa layout. Rentang itu lalu diperluas 2 × (jumlah baris terlihat) ke atas dan ke bawah lewat indeks baris.
- **Satu delegate storage (koreksi AR).** `NSTextStorage` hanya punya satu delegate. Indeks baris ruler 4A dan hook 4B sama-sama membaca `didProcessEditing`, jadi keduanya dipanggil dari satu metode di `Coordinator`, dengan urutan tetap: indeks ruler, lalu `EditorAnalysis`. Deteksi drift (§7.5) membandingkan keduanya sesudah itu.
- **Pemicu permintaan jendela terlihat:** edit; scroll (`boundsDidChange` pada clip view, paling banyak sekali per frame); resize atau ganti wrap; berakhirnya marked text; ganti font kode.
- **Ganti font kode** memanggil `mark_dirty(0, len)`. Warna tidak perlu diapa-apakan karena `NSColor` dinamis mengikuti appearance sendiri (`SQLSyntax.swift:138-143`).

### 7.4 Aturan penerapan

```swift
storage.beginEditing()
for r in pkt.ranges { storage.addAttributes(base, range: r) }   // [.foregroundColor: SQLSyntax.base, .font: upright]
for t in pkt.tokens { storage.addAttributes(attrs[t.class], range: t.range) } // Comment: warna + italic
storage.endEditing()
```

- **Hanya dua kunci atribut yang disentuh:** `.foregroundColor` dan `.font`. Objek warnanya tetap `static let` di `SQLSyntax` (palet tidak berubah), dan fontnya pasangan (tegak, italic) 12,5 pt dari `SQLSyntax.font(italic:)`. Font dasar 12,5 pt menimpa `textView.font` 13 pt, sama dengan hari ini (`SQLSyntax.swift:162` dan `SQLEditor.swift:44`).
- **Hook `didProcessEditing` mengabaikan edit atribut saja.** Hanya `editedMask.contains(.editedCharacters)` yang dikirim ke Rust.
- **Anggaran main per giliran runloop:** jendela terlihat ≤ 131.072 unit, potongan idle ≤ 16.384 unit (sekitar 2 ms `addAttributes`). Potongan berikutnya dijadwalkan sesudah yang sebelumnya diterapkan, sehingga event input tetap diproses di antaranya.
- **Typing attributes.** Hari ini `apply` memaksa `typingAttributes = base` di setiap lintasan (`SQLSyntax.swift:100`), dan itu aman karena pewarnaannya sinkron. Dengan pewarnaan asinkron, karakter yang diketik mewarisi atribut karakter sebelumnya (perilaku bawaan AppKit), sehingga mengetik di dalam komentar atau string tidak berkedip. `base` hanya dipasang saat dokumen kosong dan saat font berganti. Selama paling lama satu frame, karakter baru bisa memakai warna tetangganya (§14).
- **Pewarisan itu belum terbukti untuk editor ini (koreksi AR) `[perlu verifikasi]`.** `SQLEditor` memakai `isRichText = false` (`SQLEditor.swift:41`) dan `textView.font` 13 pt (`:44`). Bila `typingAttributes` tidak mengikuti karakter sebelumnya, karakter baru mendapat 13 pt sampai `apply` datang, dan tinggi baris bisa melompat selama satu frame. Itu perubahan geometri, bukan sekadar warna. `EditorAnalysisTests` wajib menguji bahwa karakter yang baru diketik, **sebelum** `apply` berjalan, ber-`.font` 12,5 pt, baik sesudah keyword, di dalam komentar, maupun di posisi 0. Bila tes gagal, jalan keluarnya: sesudah setiap edit karakter, `typingAttributes` diisi atribut `.foregroundColor` dan `.font` di `caret − 1` (atau `base` di posisi 0). Biayanya O(1) di main.

### 7.5 IME, undo, surrogate, find bar

- **IME (D-9).**
  - `replace` dikirim untuk setiap edit karakter, **termasuk** selama `hasMarkedText()`. `replace` tidak melex, jadi pengiriman ini murah, dan cermin teks serta indeks baris tetap tepat selama komposisi.
  - **Penjaga:** `apply` tidak pernah berjalan selama `hasMarkedText()`, dan permintaan jendela terlihat ditunda. `SQLTextView` meng-override `unmarkText()` dan `insertText(_:replacementRange:)` untuk memanggil `onMarkedTextEnded`, yang memicu permintaan jendela terlihat.
  - **Deteksi akhir komposisi (koreksi AR).** `onMarkedTextEnded` dipicu oleh **transisi** `hasMarkedText()` dari `true` ke `false`, bukan oleh setiap panggilan. Ketiga override (`insertText(_:replacementRange:)`, `setMarkedText(_:selectedRange:replacementRange:)`, `unmarkText()`) mencatat nilainya sebelum `super` dan membandingkannya sesudahnya. `setMarkedText` wajib ikut, karena membatalkan komposisi (Esc) bisa berakhir lewat `setMarkedText` dengan string kosong. `insertText` biasa tanpa marked text tidak memicu apa pun di luar jalur edit yang sudah ada.
  - **Penjadwal idle berhenti** selama marked text. Kalau tidak, hasil yang terus dibuang saat `apply` membuat penjadwal berputar. Rentang yang tidak diterapkan tetap kotor karena `mark_applied` tidak dipanggil, sehingga semuanya terkirim lagi sesudah komposisi.
  - *Alternatif yang setia pada teks rencana* (**ditolak AR**, dipertahankan sebagai catatan): selama `hasMarkedText`, edit ditahan dan digabung menjadi satu `(start, oldLen, newLen)` dengan aturan gabung `start' = min`, `oldLen' = (akhir_sebelum − start') − (pn − po)`, `newLen' = (akhir_sebelum − start') + (nl − ol)`, lalu dikirim sekali saat komposisi selesai. Harga alternatif ini: kode penggabung, dan cermin yang tertinggal selama komposisi.
- **Undo.** Undo memutar ulang edit storage. Edit itu masuk lewat hook yang sama sebagai `replace` biasa. Undo bisa memulihkan atribut lama bersama teksnya, dan itu tertangani karena rentang edit selalu kotor (§5.6). Penerapan atribut tidak lewat `shouldChangeText`, jadi tidak pernah masuk tumpukan undo.
- **Surrogate.** Sebelum `replace`, Swift melebarkan rentang edit berdasarkan teks **baru**:
  - bila unit di `start − 1` adalah high surrogate, `start −= 1`;
  - bila unit di akhir rentang baru adalah low surrogate, akhir lama dan akhir baru masing-masing `+= 1`.

  Teks di luar rentang edit tidak berubah, jadi pelebaran ini selalu mencakup karakter lama secara utuh. Bila Rust tetap menjawab `SplitsCharacter` atau `OutOfBounds`, dokumen dibuat ulang dari `storage.string` dan `os_log(.fault)` mencatatnya.
- **Deteksi drift.** Sesudah setiap `replace`, `len_utf16()` dibandingkan dengan `storage.length` (O(1)). Bila berbeda, dokumen dibuat ulang dan kejadiannya dicatat. Di DEBUG dan di tes, `line_count()` juga dibandingkan dengan indeks ruler 4A.
- **Find bar.** Sorotan find adalah temporary attribute `.backgroundColor` di layout manager (`SQLEditor.swift:601-621`). 4B tidak menyentuhnya, dan atribut storage tidak mengubahnya. Syaratnya TextKit 1 (asumsi 4A no. 1).

### 7.6 Membuka dokumen dan teks dari luar

- **`makeNSView` dan setiap set teks dari luar** (`updateNSView`, muat berkas, buka tabel) membuat **`EditorDocument` baru**. Hook `didProcessEditing` melewati edit yang sedang dibuat oleh set dari luar lewat sebuah flag. Hasil latar milik dokumen lama dibuang karena identitas objeknya berbeda. API `reset` tidak diperlukan.
- **Pewarnaan pertama sinkron,** supaya tidak ada frame tanpa warna saat membuka dan scene `VisualParityTests` tetap deterministik. **Satu aturan untuk semua ukuran (koreksi AR):** jendela terlihat ±2 layar dianalisis dan diterapkan di main sebelum frame pertama, dengan anggaran 131.072 unit yang sama dengan jalur ketikan. Sisanya dikerjakan potongan idle. Bila layout belum ada (misalnya di `makeNSView`), jendela diambil di sekitar caret. Draf awal mewarnai penuh secara sinkron sampai 262.144 unit. Dengan anggaran §7.4 (sekitar 2 ms per 16.384 unit), itu berarti hitch sekitar 30 ms setiap kali tab dibuka atau berpindah, dan tidak ada gate yang membutuhkannya. Lapis atribut G-VIS yang direvisi hanya membandingkan rentang terlihat, sedangkan L5 menguras antrean secara eksplisit (§9).

  Ini satu-satunya saat main memanggil `analysis`. Aman, karena dokumen baru belum punya kerja latar sehingga kunci `highlight` pasti bebas.
- **Pindah tab dengan dokumen 2 MB** membayar satu lex penuh di latar. Menyimpan satu dokumen per `QueryTab` bisa menghapus biaya itu, tetapi `QueryTab.swift` bukan milik W4-T2. Ini dicatat sebagai optimasi lanjutan, bukan bagian 4B.

## 8. Titik kait

### 8.1 Rotor (FR-ED-09), dipasang di W4-T2

- **Aksesibilitas yang utuh.** Editor tetap `NSTextView` dengan teks di storage. Tidak ada penggambaran teks kustom, jadi seluruh aksesibilitas teks bawaan AppKit tetap berlaku.
- **Hook.** `SQLTextView.rotorSources: [EditorRotorSource]`. Override `accessibilityCustomRotors()` membangun satu `NSAccessibilityCustomRotor(label:itemSearchDelegate:)` per sumber. Delegate-nya mencari rentang berikutnya atau sebelumnya dari `targetRange` item sekarang, lalu mengembalikan `ItemResult(targetElement: textView)` dengan `targetRange` dan `customLabel`.

  ```swift
  protocol EditorRotorSource: AnyObject {
      var rotorLabel: String { get }
      func rotorItems() -> [(range: NSRange, label: String)]
  }
  ```

- **Klien pertama (4B): rotor "Statements".** Itemnya rentang statement dari outline terakhir, dengan label baris pertama statement yang dipangkas ke 60 karakter. Klien ini membuat hook itu bisa diuji (`EditorRotorTests`) dan diperiksa AX. Bila AX lebih suka hook tanpa klien, rotor ini bisa dilepas tanpa mengubah hook.

### 8.2 Diagnostik (FR-ED-06, W10-T6)

- `EditorStatements.unclosed` sudah membawa pembuka yang tidak tertutup menurut `scan.rs`, yaitu pembuka yang benar-benar diketik pengguna: `'` pertama dari string yang tak berakhir, bukan kutip yang dilewati regex. W10-T6 cukup membacanya dan menambah `QueryIssuesRotor` sebagai `EditorRotorSource` kedua.
- Dengan begitu W10-T6 tidak perlu masuk lane FFI hanya untuk masalah leksikal. Posisi galat dari server tetap milik W10-T6.

### 8.3 Referensi tabel dan alias (W11-T6)

- `Lexer` bersifat publik di dalam crate. Ia bisa dijalankan atas potongan teks apa pun (mode Normal, `prev_word = false`) dan mengemit `Word`.
- W11-T6 menambah `editor/refs.rs`: resolusi `FROM`/`JOIN … [AS] alias` pada **satu statement** (rentang dari §4.1) yang dihitung sesuai permintaan. Hasilnya diekspor sebagai `EditorDocument::references(revision, offset)` di `qh-ffi/src/editor.rs`.
- Syaratnya tidak menambah kerja per ketikan. Blok token tidak menyimpan `Word`, dan itu disengaja.

### 8.4 Formatter (P-10, W12-T1)

- **P-10 menyebut formatter dibangun di atas lexer 4B.** Itu hanya separuh benar. Lexer warna membawa keanehan regex (§3.5): badan `$tag$` dilex sebagai kode, `+--` bukan komentar, dan kutip tak tertutup dilewati. Formatter yang mewarisinya akan mengubah isi string dan badan fungsi.
- **Aturan untuk W12-T1** (diterima AR):
  - region opaque (kutip, komentar, `$tag$`) **wajib** diambil dari mesin `scan.rs`, fungsi jalan yang diekstrak di §4.1. Ini sama dengan pemisah statement engine, jadi formatter tidak bisa memecah atau mengubah isi yang engine anggap teks;
  - formatter menolak bekerja bila `unclosed` terisi;
  - tokenisasi di region kode **tidak** diikat di sini (koreksi AR). `Lexer` boleh dipakai ulang, tetapi keanehannya (`1abc`, lari tanda baca, `:` yang dilewati) harus dinilai terhadap verifikasi W12-T1 ("deretan token non-spasi identik", `:name`). Keputusan itu milik rancangan W12-T1 dan AR-nya.
- **Catatan untuk PRD P-10.** Barisnya perlu berbunyi "di atas mesin `scan.rs` (region opaque) dan modul editor 4B", bukan "di atas lexer 4B". Yang mengubah PRD adalah orkestrator, bukan W4-T2.

## 9. Strategi uji paritas

Lapis demi lapis, dari yang paling murah:

| Lapis | Di mana | Isi | Kapan |
|---|---|---|---|
| L0 | `LexerFixtureExport.swift` (W3-T2) | Dengan `QH_EXPORT_FIXTURES=1`, menulis `keywords.txt`, tabel Unicode (§3.4), dan `corpus/*.spans`, `*.outline`, `scripts/*.edits` ke `crates/qh-sql/tests/fixtures/editor/`. Tanpa variabel itu, tes di-skip. | W3-T2, sekali; diulang di W4-T2 commit A dan hasilnya harus identik (`git diff` kosong) |
| L1 | `tests/editor_parity.rs` | `lex_all` == `.spans` untuk setiap berkas korpus | W3-T2 dan seterusnya |
| L2 | `tests/editor_parity.rs` | Setiap skrip edit diputar ulang lewat `TextBuffer` + `Highlighter`. Hash FNV-1a 64 dari daftar span penuh dibandingkan per langkah. | W3-T2 dan seterusnya |
| L3 | `tests/editor_incremental.rs` | Uji diferensial berbenih (SplitMix64): setelah setiap edit, span inkremental == `lex_all`; `dirty` ⊇ token yang berubah; invarian blok (entry sama dengan lex dari nol); indeks baris, indeks potongan, dan `last_closer` == hitung naif. | W3-T2 dan seterusnya |
| L4 | `EditorLexerParityTests.swift` (W4-T2 A) | **Lewat FFI, melawan regex yang masih hidup.** Korpus + 500 skrip × 40 edit (`QH_PARITY_SEEDS` untuk soak) + beberapa dokumen 1 MB. Span penuh FFI == span regex di setiap langkah. | **Harus lulus sebelum commit B** |
| L5 | `EditorLexerParityTests.swift` (W4-T2 A) | **Paritas atribut.** Dua `SQLTextView` headless (TextKit 1): satu dicat jalur lama, satu jalur baru, dengan edit yang sama. `enumerateAttributes` atas `.foregroundColor` dan `.font` dibandingkan per rentang, di appearance gelap dan terang, dengan cara yang sama seperti `VisualParityTests.attributeRuns`. | Harus lulus sebelum commit B |
| L6 | G-VIS | Delapan scene editor × dua appearance, termasuk lapis atribut | W4-T2 A dan B |

**Oracle L4 dan L5 (koreksi AR).**

- "Regex yang masih hidup" dan "jalur lama" berarti `SQLSyntax.attributes(for:)` atas **seluruh dokumen** sesudah setiap edit, dicat di atas `base` untuk seluruh storage. Itu semantik commit P, tempat baseline direkam. Yang **bukan** oracle adalah painter inkremental 4A: 4A mengecat ulang per statement yang diedit, sehingga hasilnya bisa bergantung pada riwayat edit (kutip yang berpasangan melintasi statement tidak dicat ulang di statement lain). Bila keduanya berbeda, yang salah adalah 4A, dan temuan itu dilaporkan, bukan ditiru.
- L5 membandingkan **seluruh dokumen**, sesudah antrean dikuras secara deterministik lewat kait tes (`EditorAnalysis.drainForTesting()`: jalankan analisis dan `apply` sampai `more_in_window` dan `dirty_elsewhere` sama-sama `false`). Tidak ada `sleep`.
- Lapis atribut G-VIS sedang direvisi agar hanya membandingkan run di rentang terlihat dikurangi badan yang dilipat. L5 **tidak** mewarisi pembatasan itu. L5 adalah bukti paritas atas seluruh dokumen, sedangkan G-VIS adalah bukti piksel dan tata letak.
| L7 | sesudah B | L1–L3 tetap di Rust. Di Swift, L4 berubah menjadi smoke FFI atas korpus melawan `.spans` yang dibekukan. | selamanya |

**Format fixture.** Teks biasa, tanpa serde:

- `.spans`: baris `start len class`, dalam UTF-16.
- `.outline`: baris `S start end`, `F kind headerLine lastLine header bodyStart bodyEnd summary`, dan `U kind at`.
- `.edits`: baris pertama `T <teks awal>`, lalu per langkah `E start len <teks>` dan `H <fnv64 hex>`.
- Teks di-escape per byte UTF-8 sebagai `%XX` untuk semua yang bukan ASCII cetak.
- Hash FNV-1a 64 dihitung atas baris `start,len,class\n`. Implementasinya sepuluh baris di tiap bahasa.

**Korpus nyata:**

- `deploy/dev/seed-postgres.sql`, `seed-mysql.sql`, dan `qh-mysql-old-seed-prefixed.sql` (sekitar 10 KB);
- SQL kasus di `tools/golden/live_cases.py`;
- SQL scene di `Support/Snapshot.swift` dan dokumen di `VisualParityTests`;
- string SQL dari tes editor yang ada.

**Korpus buatan tangan** (satu berkas per tema): `edge-eof`, `edge-punct`, `edge-numbers`, `edge-unicode`, `edge-crlf`, `edge-dollar`, `edge-functions`, `edge-surrogate`. Isinya semua baris di tabel §3.3–§3.5. Tidak ada SQL yang disalin dari TablePro.

**Alfabet generator acak.** Keyword dengan huruf acak, identifier, `_`, `$`, digit, `.`, `'`, `''`, `"`, `""`, backtick, `--`, `/*`, `*/`, `( ) ; ,`, operator, spasi, tab, `\n`, `\r\n`, `\r`, `é`, `ß`, `中`, 😀, U+0301, U+200D, U+200C, U+FEFF, `٣`, `²`, `Ⅻ`, U+00A0, U+2028, U+0085, `＿`, `‿`, `$1`, `:name`, dan `::`.

**Edit acak.** Sisip, hapus, dan ganti di posisi acak yang tidak membelah surrogate. Sesekali tempel 1–4 KB dan hapus lintas baris. Operasi terarah menyisipkan atau menghapus satu `'`, `"`, backtick, `/*`, atau `*/`.

**Anggaran waktu tes.** Default L3 selesai < 30 detik di `cargo test` debug. `QH_EDITOR_SOAK=1` memperbesar jumlah dan ukuran. Daftar pengecualian statement dan lipatan (tabel §4.1) ditulis di `fixtures/editor/DIVERGENT.md`, lengkap dengan keluaran Rust yang diharapkan, supaya perbedaan yang disengaja tidak terbaca sebagai regresi.

## 10. Yang dihapus, dan kapan

| Kapan | Yang dihapus | Di mana |
|---|---|---|
| W3-T2 | tidak ada | — |
| W4-T2 commit A | tidak ada; hanya tambahan | — |
| W4-T2 commit B | regex, `ceiling`, `attributes(for:baseFont:commentFont:)`, `apply(to:)`, `isCalled`, `isLiteral`, dan literal daftar keyword (`keywords` menjadi `Set(try editorKeywords())`). Palet dan `font(italic:)` tetap. | `Views/SQLSyntax.swift` |
| W4-T2 commit B | `foldingSizeLimit`, isi `regions`, isi `statementRanges`, `cteRegions`, `cteOpenParens`, `matchingParen`, dan semua helper lexing (`:262-388`). Yang tetap: `FoldRegion`, `lineStarts`, `line(containing:)`, dan `shift` bila 4A masih memakainya. `regions(in:)` dan `statementRanges(in:)` menjadi pembungkus tipis atas FFI, karena `VisualParityTests` dan tes lipatan memanggilnya. | `Support/SQLFolding.swift` |
| W4-T2 commit B | jalur `colour()` atas seluruh dokumen, perhitungan `statementBounds` di Swift, dan sisa `SQLFoldStyler` bila 4A belum menghapusnya | `Views/SQLEditor.swift` |
| W4-T2 commit B | eksportir fixture span dan outline (oracle-nya sudah hilang, dan fixture dibekukan), serta separuh L4 yang melawan regex. **Eksportir tabel Unicode tetap** (§3.4, koreksi AR). | `LexerFixtureExport.swift`, `EditorLexerParityTests.swift` |
| W4-T2b, sesudah W4-T1 dan sebelum W4 ditutup (§15.1) | isi `sqlStatements(in:)` diganti pembungkus `sqlStatementRanges` | `Models/QueryTab.swift:1056-1098` |
| **tidak pernah** | lexer `:name` dan opaque (dipakai `QueryParameters.swift:51,136` dan `KeywordCase.swift:39`) | `Models/SQLScanner.swift`, `Models/QueryParameters.swift` |

Perkiraan: Swift berkurang sekitar 450 baris, dan bertambah sekitar 350 (`EditorAnalysis.swift`). Rust bertambah sekitar 1.800–2.200 baris termasuk tes, ditambah tabel yang dihasilkan.

## 11. W3-T2: berkas per berkas (inti Rust dan fixture)

Pelaksana GP-o. Gate RR, TD, CR, ditambah SEC khusus untuk diff `scan.rs` (koreksi AR). Verifikasi G-RUST, lalu G-SWIFT (eksportir di-skip secara default). W3-T2 juga mencatat waktu `lex_all` atas 2 MB (keputusan `frontier`, §5.5).

| Berkas | Tujuan | Antarmuka kunci | Bergantung pada |
|---|---|---|---|
| `crates/qh-sql/src/editor/mod.rs` (baru) | Doc modul, re-ekspor, konstanta | `KEYWORDS: &[&str]` (131, terurut), `CEILING_UTF16`, `keywords()`, `Class`, `Token` | — |
| `crates/qh-sql/src/editor/lex.rs` (baru) | Lexer paritas regex (§3, §5.3) | `Lexer::new`, `next_token`, `lex_all` | `unicode_tables.rs` |
| `crates/qh-sql/src/editor/unicode_tables.rs` (baru, dihasilkan) | Lima tabel rentang (§3.4) | `ICU_WORD`, `ICU_DIGIT`, `ICU_TRANSPARENT`, `FND_LETTER`, `FND_ALNUM`, `fn contains(table, c)` | — |
| `crates/qh-sql/src/editor/text.rs` (baru) | Cermin teks, indeks baris, indeks potongan, `last_closer`, log (§5.1) | `TextBuffer::{new, replace, snapshot, drain_log, line_count, line_of, line_starts}`, `Edit`, `LogEntry`, `Snapshot` | — |
| `crates/qh-sql/src/editor/highlight.rs` (baru) | Blok, lex ulang, konvergensi, `dirty`, hasil `analysis` (§5.2–§5.7, §6.2) | `Highlighter::{new, sync(&Snapshot, Vec<LogEntry>), analysis(window, budget) -> Analysis}` (struct murni; `qh-ffi` memetakannya ke `EditorAnalysis`) | `lex.rs`, `text.rs` |
| `crates/qh-sql/src/editor/outline.rs` (baru) | Statement, `unclosed`, lipatan (§4) | `outline(&Snapshot) -> Outline { statements, unclosed, folds }`, `statement_ranges(&str)` | `scan.rs`, `unicode_tables.rs` |
| `crates/qh-sql/src/scan.rs` | Ekstrak loop menjadi fungsi jalan internal. Keluaran `scan()` identik. | `pub(crate) fn walk(bytes, on_separator, on_word) -> EndState` | — |
| `crates/qh-sql/src/lib.rs` | `pub mod editor;`, dan doc crate menyebut modul ini | — | — |
| `crates/qh-sql/tests/editor_parity.rs` (baru) | L1, L2 | — | fixture |
| `crates/qh-sql/tests/editor_incremental.rs` (baru) | L3, termasuk contoh `select 1` (§6.2) | SplitMix64 lokal | — |
| `crates/qh-sql/tests/editor_outline.rs` (baru) | Statement dan lipatan terhadap `.outline` plus `DIVERGENT.md`; baris | — | fixture |
| `crates/qh-sql/tests/editor_unicode.rs` (baru) | Tabel terurut dan tidak beririsan; `is_whitespace` == `whitespacesAndNewlines` yang diekspor | — | fixture |
| `crates/qh-sql/tests/fixtures/editor/**` (baru) | Keluaran eksportir dan korpus buatan tangan, total ≤ 5 MB | — | — |
| `app/Tests/QueryHiveTests/UnicodeTableExport.swift` (baru) | Ekspor `unicode_tables.rs` beserta kepala asal-usul (§3.4). Tetap hidup sesudah commit B. | skip bila `QH_EXPORT_UNICODE` ≠ `1` | `NSRegularExpression`, `CharacterSet` |
| `crates/qh-sql/tests/scan_refactor.rs` (baru) | Diferensial `scan()` lama vs baru (§4.1, koreksi AR) | salinan fungsi lama, hanya di tes | korpus §9 |
| `app/Tests/QueryHiveTests/LexerFixtureExport.swift` (baru) | L0. Memetakan atribut ke kelas lewat identitas objek warna `SQLSyntax.*` (`@testable`). Untuk teks > 200 ribu memakai salinan pola yang **diverifikasi** sama dengan `SQLSyntax.attributes(for:)` pada semua input ≤ 200 ribu. | skip bila `QH_EXPORT_FIXTURES` ≠ `1` | `SQLSyntax`, `SQLFolding` (dibaca, tidak diubah) |

Tidak ada perubahan pada `Cargo.toml` dan `Cargo.lock`.

## 12. W4-T2: berkas per berkas (FFI dan integrasi Swift)

Pelaksana GP-s. Gate SR, RR, SEC, AX, AR, CR. Verifikasi:

- G-RUST, G-FFI, G-SWIFT, L4, L5, G-VIS;
- `--bench type-10k` ≤ 4 ms dan `type-2m` ≤ 8 ms, p99 di main;
- bench tambahan `type-2m` dengan auto-uppercase menyala, dicatat saja (§14).

| Berkas | Commit | Tujuan | Antarmuka kunci |
|---|---|---|---|
| `crates/qh-ffi/src/editor.rs` (baru) | A | Objek UniFFI, galat, rekaman, fungsi bebas (§6) | §6.1–§6.4 |
| `crates/qh-ffi/src/lib.rs` | A | satu baris `pub mod editor;` (scaffolding sudah ada di `:111`) | — |
| `app/Generated/*` | A | diregenerasi `./app/build-ffi.sh` (invariant #1) | — |
| `app/Sources/QueryHive/Support/EditorAnalysis.swift` (baru) | A | Pemilik `EditorDocument` per editor. Revisi, dua antrean, koalesi, `EditorAnalysisResult.validate`, `apply`, penjaga marked text, penjadwal idle, fetch outline, pelebaran surrogate, deteksi drift, `StatementsRotorSource`. | `init(text:)`, `textStorageDidEdit(range:delta:)`, `requestVisible()`, `fontsChanged()`, `markedTextEnded()`, `onOutline`, `lineCount` (drift) |
| `app/Tests/QueryHiveTests/EditorLexerParityTests.swift` (baru) | A (dipangkas di B) | L4 dan L5 | — |
| `app/Tests/QueryHiveTests/EditorAnalysisTests.swift` (baru) | A | Validasi hasil (setiap aturan tolak), buang revisi basi, penjaga marked text (termasuk transisi lewat `setMarkedText` kosong dan penjadwal idle yang berhenti), font 12,5 pt pada karakter baru sebelum `apply` (§7.4), pelebaran surrogate, CRLF, undo dan redo memulihkan atribut, drift memicu pembuatan ulang, `drainForTesting` | — |
| `app/Sources/QueryHive/Views/SQLEditor.swift` | B | Hook `didProcessEditing` memanggil `EditorAnalysis`. Pengamat scroll. Band, run mark, dan lipatan dari outline Rust. `SQLTextView`: rotor, override `unmarkText`/`insertText`, typing attributes (§7.4). Jalur warna lama dihapus. | — |
| `app/Sources/QueryHive/Views/SQLSyntax.swift` | B | §10. Tambah `attributes(forClass:)` dan pasangan font. | `static let keywords` via FFI |
| `app/Sources/QueryHive/Support/SQLFolding.swift` | B | §10. Pembungkus `regions(in:)` dan `statementRanges(in:)` atas FFI. | — |
| `app/Tests/QueryHiveTests/EditorFindAndFoldingTests.swift` | B | `foldingSizeLimit` menjadi `editorCeilingUtf16()`. Tes lain tetap hijau tanpa diubah. | — |
| `app/Tests/QueryHiveTests/EditorRotorTests.swift` (baru) | B | Rotor "Statements" ada; pencarian maju dan mundur memberi rentang statement | — |
| `app/Tests/QueryHiveTests/LexerFixtureExport.swift` | B | dihapus | — |

**Tidak disentuh:**

- `Models/SQLScanner.swift`, `Models/QueryParameters.swift`, `Models/KeywordCase.swift` (§15.2), `Views/Workspace.swift`, `app/Package.swift`;
- `crates/qh-ffi/src/uniffi_api.rs` dan keempat daftar invariant #11;
- `Models/QueryTab.swift` (milik W4-T1 di gelombang yang sama; isinya diubah di W4-T2b, §15.1).

**Selisih dengan daftar berkas `development-plan.md` (koreksi AR, untuk orkestrator).** Daftar W4-T2 di sana menyebut `Models/SQLScanner.swift` (bagian statement dihapus), padahal berkas itu tidak disentuh (§15.2). Daftar itu juga belum memuat berkas tes di tabel ini. Daftar W3-T2 belum memuat `scan.rs`, `UnicodeTableExport.swift`, dan `scan_refactor.rs`, dan masih menulis `editor.rs` alih-alih direktori `editor/` (§15.5). Rencana perlu disamakan sebelum tugasnya mulai, supaya aturan kepemilikan berkas §7 tidak dilanggar.

## 13. Urutan kerja

**W3-T2**

1. Eksportir, korpus buatan tangan, lalu jalankan `QH_EXPORT_FIXTURES=1`. Fixture dan `unicode_tables.rs` di-commit lebih dulu, karena keduanya adalah kebenaran.
2. Refaktor `scan.rs`. Tes lamanya harus tetap hijau tanpa diubah.
3. `lex.rs` dan `lex_all`, sampai L1 hijau.
4. `text.rs` dan tes naifnya.
5. `highlight.rs`: blok, lex ulang, konvergensi, `dirty`, hasil `analysis`. Sampai L2 dan L3 hijau, termasuk contoh `select 1`.
6. `outline.rs`, sampai `editor_outline.rs` hijau dengan `DIVERGENT.md`.

**W4-T2 commit A** (tanpa perubahan perilaku; subjek usulan `feat(editor): rust analysis behind a parity test`):

1. `qh-ffi/src/editor.rs`, lalu `build-ffi.sh`.
2. Jalankan ulang eksportir: fixture harus identik.
3. `EditorAnalysis.swift`, sementara editor masih mengecat dengan regex.
4. L4, L5, dan `EditorAnalysisTests` hijau, lalu G-FFI, G-SWIFT, G-VIS. Hasil L4 dan L5 masuk laporan sebagai bukti gate.

**W4-T2b** (§15.1), dianjurkan di titik ini bila W4-T1 sudah mendarat.

**W4-T2 commit B** (subjek dari rencana: `perf(editor): analysis moves to Rust, colouring and folding reach 2M characters`):

1. Sambungkan `SQLEditor` ke `EditorAnalysis`.
2. Hapus sesuai §10.
3. Rotor.
4. L4 menjadi smoke terhadap fixture.
5. G-VIS, lalu bench `type-10k` dan `type-2m`.

**W4-D.** ADR-0033 mencatat D-1 sampai D-14, keanehan yang dibekukan (§3.5), tabel Unicode yang dibekukan, pergantian semantik statement (§4.1), dan penjaga IME.

## 14. Risiko

| Risiko | Dampak | Penahan |
|---|---|---|
| Aturan `\b` ICU di §3.4 ditulis dari ingatan | Warna angka berbeda di sebelah tanda kombinasi dan format | Lima kasus sudah diverifikasi. Tabel diekspor dari ICU sendiri. Korpus `edge-unicode` dan alfabet acak di L4. Fixture yang menang, bukan dokumen ini. |
| Surrogate | Cermin bergeser, dan posisi token meleset | Pelebaran di Swift (§7.5), `SplitsCharacter`, pembuatan ulang dengan log, kasus `edge-surrogate`, dan emoji di alfabet acak |
| CRLF | Baris, statement, dan komentar berbeda | Indeks hanya LF, sama dengan hari ini. CR ikut di token komentar (`[^\n]`). Statement sesudah `--` di berkas CRLF kini terpecah benar; ini perubahan yang disengaja (§4.1). CR tunggal tetap tidak dihitung sebagai baris walaupun AppKit menggambarnya sebagai baris. |
| Undo | Atribut basi kembali bersama teks | Rentang edit selalu kotor (§5.6). Ada tes. |
| IME | Komposisi terganggu oleh perubahan atribut | Tidak ada `apply` selama marked text, penjadwal idle berhenti, dan akhir komposisi dideteksi dari transisi di tiga override, termasuk pembatalan lewat `setMarkedText` kosong (§7.5). Diuji di `EditorAnalysisTests` dengan `setMarkedText` terprogram. Pemeriksaan manual dengan IME Jepang dan Tionghoa masuk laporan. |
| Temporary attribute find bar | Sorotan hilang | Tidak disentuh (§7.5). Scene `editor-find` di G-VIS. |
| Warna tertinggal satu frame | Kedip saat mengetik | Typing attributes diwarisi dari tetangga (§7.4). Pewarnaan pertama sinkron (§7.6). Risiko ini sudah diterima di `performance-plan.md` §8. |
| Karakter baru ber-font 13 pt sebelum `apply` (koreksi AR) | Tinggi baris melompat satu frame | Tes di `EditorAnalysisTests`, dan jalan keluar O(1) di §7.4 |
| Oracle paritas tertukar dengan painter 4A (koreksi AR) | L4 dan L5 lulus atau gagal terhadap perilaku yang salah | Oracle adalah `SQLSyntax.attributes(for:)` atas seluruh dokumen (§9) |
| Fragmentasi run atribut | Lapis atribut G-VIS gagal walaupun pikselnya sama | L5 membandingkan run dengan cara yang sama dengan `VisualParityTests` sebelum commit B |
| Kutip membalik pasangan di dokumen 2 MB | Lex ulang O(n) | Hanya di latar. Pencarian dibatasi `last_closer`. Jendela terlihat lebih dulu. Anggaran `UTILITY` per panggilan. |
| Perebutan kunci dan inversi prioritas | Hitch di main | Main tidak pernah memegang `highlight` (§7.1). Kunci `text` hanya dipegang mikrodetik. |
| Tabel Unicode terikat versi ICU macOS saat ekspor | Beda kecil dengan macOS berikutnya | Sesudah regex dihapus, Rust adalah definisinya. Hal ini dicatat di ADR-0033. |
| `KeywordCase` memindai seluruh dokumen per delimiter (`KeywordCase.swift:39`: `SQLScanner.scan` + `Array(sql.utf16)`) | Ketikan 2 MB lambat bila auto-uppercase menyala | Default-nya mati (`EditorPreferences.swift:55`). Diukur dan dicatat di W4-T2. Perbaikannya, `is_code` dari mesin `scan.rs` per statement, adalah tugas terpisah. |
| Fixture bench > 2.000.000 unit | NFR-P5 diukur dengan pewarnaan mati | Diperiksa di W4-T2 sebelum mengukur (§5.7) |
| Rencana menyebut hapus "bagian statement di `SQLScanner.swift`" | Implementer menghapus lexer `:name` | §15.2 |

## 15. Koreksi terhadap rencana dan pertanyaan terbuka

1. **Duplikat statement ada di `QueryTab.swift`, bukan di `SQLScanner.swift`.** `sqlStatements(in:)` (`Models/QueryTab.swift:1056-1098`) dipakai `SQLFolding`, `ServerSort.swift:37`, `GridSearch.swift:59`, `AppModel.swift:1917`, dan `sqlStatement(in:atUTF16Offset:)`. Berkas itu milik W4-T1 di gelombang yang sama.

   **W4-T2b diterima AR, wajib, dan cakupannya sebagai berikut (koreksi AR):**

   - **Kenapa wajib, bukan tindak lanjut bebas.** Komentar `SQLFolding.swift:138-140` menjanjikan bahwa lipatan dan Run "tidak bisa berselisih". Commit B mematahkan janji itu untuk kasus di tabel §4.1. Contohnya `select "a;b"`: band dan run mark menunjukkan satu statement, tetapi Run mengirim `select "a`. Itu persis "menjalankan setengah statement" yang ditolak komentar `QueryTab.swift:1049-1051`. `AppModel.swift:1914-1918` juga mengklaim memecah "the way the engine splits them", dan klaim itu baru benar sesudah W4-T2b.
   - **Urutan.** W4-T2b hanya butuh `sql_statement_ranges` dari commit A. Urutan yang dianjurkan: W4-T2 A → W4-T1 mendarat → W4-T2b → W4-T2 B. Dengan urutan ini band, lipatan, dan Run pindah semantik bersamaan (`SQLFolding.statementRanges` sudah memanggil `sqlStatements`), dan jendela selisih tidak pernah ada. Bila W4-T1 belum selesai saat B siap, B boleh mendarat lebih dulu, tetapi **W4 tidak ditutup sebelum W4-T2b mendarat**. Batas "sebelum W12-T3" di draf awal terlalu longgar.
   - **Berkas.** `Models/QueryTab.swift`: hanya isi dan komentar doc `sqlStatements(in:)`. Signature `[(range: Range<String.Index>, text: String)]` tetap, dan `sqlStatement(in:atUTF16Offset:)` tidak diubah, sehingga keempat pemanggil tidak tersentuh. Tes baru `app/Tests/QueryHiveTests/StatementSplitTests.swift`. Rantai kepemilikan `QueryTab.swift` di `development-plan.md` §7 ditambah W4-T2b di antara W4-T1 dan W5-T1.
   - **Semantik.** Rentang 1:1 dengan `sql_statement_ranges`, dipetakan dari UTF-16 ke `String.Index`. `text` = potongan yang di-trim dengan `.whitespacesAndNewlines`, yang setara dengan `White_Space` dan `str::trim` (§3.4). Tidak ada penyaringan tambahan di Swift, supaya lipatan, band, dan Run memakai rentang yang sama persis.
   - **Tes.** Setiap baris tabel §4.1, termasuk CRLF dan dollar-quote. Invarian `SQLFolding.statementRanges(in:) == sqlStatements(in:).map(range)` atas korpus editor, sehingga janji di `SQLFolding.swift:138-140` menjadi tes, bukan komentar. Ditambah `ServerSortTests`, `QuickSearchTests`, `RunConfirmationTests`, dan `HighlightBandTests` yang tetap hijau.
   - **Pelaksana dan gate.** GP-s · sonnet, ukuran S. Gate SR, DB (semantik pemecahan per dialek: `$tag$`, backtick, CRLF), dan CR. Verifikasi G-SWIFT dan G-VIS editor, yang tidak boleh berubah. Subjek commit usulan: `fix(editor): statements split the way the engine splits them`.
   - **Perubahan perilaku** di tabel §4.1 (Run, guard satu statement di sort dan search server, daftar statement di konfirmasi tulis) didaftarkan orkestrator di `performance-plan.md` §14.
2. **`SQLScanner.swift` tidak punya bagian statement.** Isinya region opaque dan parameter `:name`, dan berkas itu tetap utuh (§10). `KeywordCase` tetap memakainya. Mengganti `isCode` dengan token warna akan menjadi regresi, karena selama string belum ditutup regex menganggap sisanya kode, sehingga kata di dalam string yang sedang diketik ikut diubah ke huruf besar.
3. **Penjaga IME** di sisi penerapan (D-9). **Diputuskan AR: diterima**, dengan deteksi transisi dan penjadwal idle yang berhenti (§7.5). Kalimat `performance-plan.md` §8 ("tidak mengirim edit selama `hasMarkedText`") dicatat sebagai diganti oleh ADR-0033. Yang mengubah teks rencana adalah orkestrator.
4. **Dua commit di W4-T2** (D-13). **Diputuskan AR: dua commit**, dengan syarat di D-13. Satu commit tidak lagi dianjurkan, karena urutan W4-T2b di butir 1 membutuhkan titik A yang hijau di riwayat.
5. **Nama modul** `crates/qh-sql/src/editor/` (direktori), bukan `editor.rs`. W11-T6 dan W12-T1 yang menyebut `editor.rs` merujuk ke modul ini.
6. **P-10** perlu catatan: formatter memakai `scan.rs` untuk region opaque (§8.4). **Diterima AR**, dengan tokenisasi region kode diserahkan ke rancangan W12-T1.
7. **Untuk pemilik, tidak memblokir.** Apakah keanehan §3.5 (`;--`, `)--`, badan `$tag$`) kelak diperbaiki sebagai perubahan V dengan rekam ulang baseline? Bila ya, lexer warna bisa disatukan dengan `scan.rs`, dan D-2 gugur.

## Verdict architect-reviewer

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
