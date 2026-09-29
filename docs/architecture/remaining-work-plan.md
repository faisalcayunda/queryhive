# Rencana sisa pekerjaan, setelah Gelombang 3

- **Status:** rencana kerja, 29 Sep 2026
- **Konteks:** hasil pemeriksaan ulang seluruh item yang belum dikerjakan, dengan koreksi terhadap
  beberapa klaim yang beredar di dokumen lama. Sumber tiap koreksi adalah kode di pohon ini.

## Yang sudah mendarat, supaya tidak diusulkan lagi

Gelombang 3 di-merge: `table_op` (truncate/drop di bawah Safe Mode), parameter binding di ketiga
driver, konfirmasi app yang mengirim `SAFE_MODE_CONFIRMED`, peta kolom impor, truncate/drop dari
pohon, dan sort server untuk hasil yang terpotong. Menyusul: pane General dengan preferensi startup
dan operasi tutup tab, fondasi identitas aplikasi dengan ADR-0029 (tabel `app_account` dan `profile`,
migrasi `0008`), dan rename akar sumber ke `Sources/QueryHive`. Gate terakhir: `cargo test` 768/0,
`swift test` 392/0.

## Empat temuan yang mengubah daftar

1. **Lubang ADR-0027 hari ini hanya bisa dicapai lewat CLI.** `SAFE_MODE_FLOOR` dibaca di satu tempat
   saja (`crates/qh-ffi/src/commands.rs:143`) dan di tes; app hanya mengirim `SAFE_MODE`
   (`app/Sources/QueryHive/Models/AppModel.swift:2850`), dan MCP memaksa `read_only`. Tetap harus
   ditutup lebih dulu, karena komentar di `commands.rs:109-111` menyebut floor itu sebagai "the one
   input a configuration profile would set": begitu profiles berjalan, lubangnya jadi nyata.
2. **Perbaikan yang benar bukan resolusi floor per jenis statement.** Guard biasa sudah benar di
   urutan strictness total (`crates/qh-sql/src/classify.rs:173-188`). Yang merusak monotonisitas
   hanya `guard_destructive` (`commands.rs:291`), karena ia menerima mode yang **sudah di-resolve**
   sehingga tidak bisa melihat floor. Perbaikannya sekitar dua puluh baris di fungsi itu, bukan
   perubahan di `qh-sql`.
3. **Penolakan versi protokol MCP sudah ada.** `crates/qh-ffi/src/mcp.rs:770-796` mengembalikan
   `-32022` beserta `error.data.supported` (commit `310c03d`), dan `docs/mcp-stability.md` sudah
   mencatatnya. Yang tersisa hanya teks basi: komentar `mcp.rs:79-83` yang masih menulis "falling
   back ... is the honest answer", dan kalimat terakhir plan §12.3.
4. **Studi settings melebih-lebihkan kategori "setting murah".** Nomor baris dan folding selalu
   menyala (`app/Sources/QueryHive/Views/SQLEditor.swift:76-79`), wrap di-hardcode (`:61`), dan
   keyword case, tab width, invisibles belum ada fiturnya sama sekali. Yang benar-benar murah dan
   berguna hanya default row limit untuk tab baru (`Models/QueryTab.swift:463`, sekarang `1000`).

Dua item lain ditutup tanpa dikerjakan: batas format kolom sudah terdokumentasi di
`Models/ColumnFormat.swift:3-12` ("rendering-only ... what an export writes"), dan item PROGRESS.md
tentang rotasi key, Apple Developer ID, serta riset web bukan pekerjaan agen.

## Rencana

> **Status 29 Sep 2026.** Batch 1 selesai. Lane A mendarat sebagai `30aef13` (floor ADR-0027
> ditutup, `cargo test` 770/0), Lane B sebagai `d502d0c` (default row limit dan Appearance
> master-detail, `swift test` 395/0), dan keempat keputusan Batch 2 dicatat di `2dbe952`.
> Batch 3 selesai: command akun dan profil sebagai `581ece6` (`cargo test` 773/0), dan Open Quickly
> sebagai `551b13a` (`swift test` 401/0). Batch 4, sign-in Google dan pane Account, mendarat sebagai
> `0ad767e` (`swift test` 411/0); addendum ADR-0029 mencatat bentuknya. Batch 5, pane Editor,
> mendarat sebagai `84cbe71` (`swift test` 424/0): tujuh setelan editor dengan store sendiri, dan
> pane Account disembunyikan dari bar sebagai `6c546dd` sampai client ID ada.
>
> **Yang tersisa bukan pekerjaan, melainkan konfigurasi.** Sign-in butuh OAuth client ID tipe
> *Desktop app* dari Google Cloud Console milik pemilik proyek, diberikan saat build sebagai
> `QH_GOOGLE_CLIENT_ID`. Tanpa itu pane-nya mengatakan apa yang kurang alih-alih menawarkan tombol
> yang tidak bisa bekerja. Rotasi key kenari tetap tugas pengguna.

### Batch 1, Lane A: floor Safe Mode (engine, serial)

Aksi pertama: dua tes gagal di `crates/qh-ffi/tests/safe_mode.rs`.

- `SAFE_MODE=confirm` dengan `SAFE_MODE_FLOOR=no_ddl` dan `SAFE_MODE_CONFIRMED=1`, `TABLE_OP=drop`:
  ditolak sebelum connect, memakai kalimat `no_ddl`.
- `SAFE_MODE=no_ddl` dengan `SAFE_MODE_FLOOR=confirm`: ditolak.

Lalu `guard_destructive` membaca entri floor (`SafeModeFloor::iter()`), bukan mode yang sudah
di-resolve: ada entri `no_ddl` atau `read_only` berarti refuse dengan kalimat mode itu; kalau tidak
ada tetapi ada `confirm` berarti tanya; selain itu allow. Di lane yang sama, perbaiki komentar basi
`mcp.rs:79-83` dan kalimat plan §12.3, lalu tulis addendum ADR-0027 yang mengoreksi kalimat "tempat
memperbaikinya adalah resolusi floor". Tidak ada command baru, jadi `lib.rs`, `COMMANDS`, dan
`tests/golden.rs` tidak tersentuh.

Gate: `cargo test`, target 770/0.

### Batch 1, Lane B: pane app (jalan selagi gate Lane A)

Satu berkas: `app/Sources/QueryHive/Views/SettingsView.swift`, plus `Models/QueryTab.swift` untuk
default-nya. Tidak ada irisan dengan Lane A.

- Default row limit untuk tab baru, di pane Data.
- Appearance master-detail: daftar dikelompokkan Dark/Light yang mengisi slot mode, detail berisi
  preview dan baris global yang sudah ada.

Gate: `swift build && swift test`, target 392/0. Kedua lane di-merge `--no-ff` setelah hijau.

### Batch 2: checkpoint keputusan, sudah dijawab 29 Sep 2026

1. **Profiles: kind pertama adalah `preference`.** Setelan aplikasi per akun. Konsumennya paling
   jelas dan tidak menyentuh koneksi, jadi ia dibangun lebih dulu; `connection` dan `saved_query`
   menyusul bila ada kebutuhan.
2. **Notifications: tidak dibangun.** App belum punya permukaan notifikasi dan belum ada pelacakan
   operasi panjang.
3. **Gelombang 4: hanya Open Quickly.** Structure editor, routines/UDT, backup & restore, copy
   object, dan external API tidak dikerjakan.
4. **Pane MCP: tetap ditunda** sampai MCP dipakai lebih dari satu orang.

Command profil di `lib.rs` dikerjakan sebagai lane tersendiri karena menyentuh `lib.rs`,
`tests/golden.rs`, dan `EngineCommand` di app. Sebuah profil **boleh** menetapkan
`SAFE_MODE_FLOOR`, dan itulah alasan floor ditutup lebih dulu di Lane A: sebelum addendum ADR-0027,
sebuah profil yang memaksa `no_ddl` masih bisa dilewati `SAFE_MODE_CONFIRMED`.

### Batch 5: pane Editor

**Setelan yang dikerjakan** (perilaku yang sudah ada, tinggal disakelar): line numbers, highlight
current line, highlight current statement, word wrap, code folding, show invisible characters, dan
tab width. Ketujuhnya mendarat sebagai store preferensi editor tersendiri.

**Status 29 Sep 2026.** Ketujuh setelan mendarat, dan begitu pula empat dari lima tugas di bawah:
run button per statement (`203c433`), auto-uppercase keywords (`b559925`), alur `:name` (`7189bc3`),
dan pemeriksaan `UpdateStatements.literal` (`eb69841`, yang menemukan cacat nyata: teks ber-tipe
numerik ditulis ke SQL tanpa diperiksa di jalur inline Trino). Yang tersisa hanya Vim mode, ditunda
atas permintaan pengguna. Daftar di bawah disimpan sebagai catatan apa yang masing-masing butuhkan
dan apa yang sengaja tidak dibangun.

1. **Run button beside each statement.** App sudah punya "Run Current Statement"; yang belum adalah
   tombolnya di gutter. Butuh satu glyph per rentang statement di `LineNumberRulerView`, memakai
   `SQLFolding.statementRanges` yang sudah ada, plus aksi ke jalur run yang sudah ada. Ukuran:
   sedang. Risiko: ruler sudah menggambar fold mark, dan dua glyph di satu kolom harus tidak saling
   menimpa.
2. **Auto-uppercase keywords.** Butuh jalur edit yang sadar kata kunci pada commit: setelah spasi,
   newline, atau tanda buka, kata sebelumnya diganti bentuk kanoniknya bila ia kata kunci. Ukuran:
   sedang. Risiko: menulis ulang teks saat mengetik harus tidak merusak undo stack dan pewarnaan;
   `SQLSyntax` sudah memegang daftar kata kuncinya.
3. **Query parameters (`:name`).** **Desainnya sudah diputuskan lewat advisor, 29 Sep 2026; belum
   dikerjakan.** Intinya: ini **substitusi, bukan binding**, dan itu bukan jalan pintas melainkan yang
   memang diminta ADR-0028 (keputusan 5: driver yang tidak bisa mengikat ditolak, pemanggil
   meng-inline). Mengikat untuk sebuah `SELECT` tidak bisa dicapai hari ini di driver mana pun:
   PostgreSQL menolaknya secara desain (keputusan 7, extended protocol mengembalikan biner), Trino
   tidak punya placeholder, dan jalur `preview` tidak memanggil `execute_bound`. Mengikat hanya di
   MySQL akan membuat semantik berbeda per koneksi, dan teks di history berhenti menjadi teks yang
   benar-benar dijalankan. Jadi nilainya ditulis sebagai literal yang diperiksa ke dalam teks
   statement, lalu teks itu lewat jalur `preview` biasa, sehingga `guard_for`, `RunConfirmation`, dan
   history semuanya melihat teks final yang sama.

   Keputusan yang mengikat:

   - **Nilai hidup per tab, hanya di memori.** Prompt setiap kali Run, terisi nilai terakhir tab itu.
     Tidak ikut `restoreSession`: tab dipulihkan saat launch, jadi nilai yang tersimpan berarti nilai
     basi plus menjalankan tanpa prompt. Set per koneksi ditolak karena nama yang sama berarti hal
     berbeda di statement berbeda, dan hasilnya jawaban salah tanpa suara.
   - **Sintaks `:name` saja.** `$name` menabrak `$1` dan dollar quoting PostgreSQL, `@name` adalah
     user variable MySQL, dan `{{name}}` akan menyala pada SQL dbt yang ditempel. Lexer-nya di Swift,
     state machine yang menyalin state `crates/qh-sql/src/scan.rs:40-56`, dan hanya mengenali `:` +
     `[A-Za-z_][A-Za-z0-9_]*` dalam state Normal. Yang dikecualikan: string `'...'`, identifier `"..."`
     dan backtick, komentar `--` dan `/* */`, body `$tag$...$tag$`, `::` cast, isi `[...]` sehingga
     slice `arr[lo:hi]` tidak menghasilkan `:hi`, `:=`, `:1`, dan `: name`. Batas yang diketahui: tidak
     ada scanner di pohon ini yang memahami backslash escape `E'...'`, jadi itu dinyatakan, bukan
     disembunyikan.
   - **Keamanan lewat satu fungsi.** Nilai mencapai SQL hanya lewat `render(value:kind:) -> String?`,
     dan ia mengembalikan nil (run ditolak dengan pesan) untuk yang tidak bisa direpresentasikan.
     Tidak ada parameter mentah atau fragmen SQL. `UpdateStatements.literal` **tidak** dipakai apa
     adanya: ia mengambil tipe dari deskripsi kolom server yang tidak dimiliki sebuah parameter. Per
     tipe: angka dengan grammar ketat dan negatif dalam tanda kurung (`x-:n` dengan `n = -5` menjadi
     `x--5`, yaitu komentar baris); teks dengan penggandaan `''`, dan `E'...'` bila mengandung
     backslash di PostgreSQL, sedangkan MySQL menolak backslash karena artinya bergantung `sql_mode`;
     `DATE '...'` dan `TIMESTAMP '...'` karena klien ini Trino-first dan Trino ketat soal varchar vs
     date; boolean `TRUE`/`FALSE`; `NULL` dengan peringatan bila mengikuti `=`.
   - **Yang tidak dibangun:** binding di jalur Run (termasuk yang hanya-MySQL), parameter mentah atau
     identifier, ekspansi daftar untuk `IN (:ids)`, substitusi di dalam string literal, menjalankan
     dengan nilai yang diingat tanpa prompt, set parameter per koneksi atau yang tersinkron, dan
     kalimat setelan yang memakai kata "bind". Barisnya menyebut substitusi.
4. **Vim mode.** **Ditunda atas permintaan pengguna**, 29 Sep 2026. Lapisan modal editing: mode
   normal/insert/visual, operator dan motion, register, dan peta tombolnya. TablePro punya; pohon ini
   tidak punya satu pun bagiannya. Ukuran: proyek tersendiri, dan ia bersinggungan dengan
   `interceptKey` yang sekarang dipakai completion dan shortcut.

5. **Memeriksa `UpdateStatements.literal` pada jalur inline Trino.** Temuan sampingan advisor, belum
   ditelusuri ujung ke ujung: untuk penulisan inline Trino, ia menaruh teks ber-tipe numerik ke SQL
   yang dijalankan tanpa memeriksanya. Perlu dicek apakah validasi nilainya sudah terjadi di hulu
   (grid edit) atau belum.

### Batch 6: pane Data

**Setelan yang dikerjakan** (perilaku yang sudah ada, tinggal disakelar): row height, NULL display,
show alternate row backgrounds, show row numbers, arah klik pertama pada header kolom, default view
JSON viewer, dan clear all query history.

**Status 29 Sep 2026.** Ketujuhnya mendarat sebagai store preferensi data tersendiri
(`app/Sources/QueryHive/Support/DataPreferences.swift`), plus tombol Clear History yang sebelumnya
tidak pernah ada — `history_clear` sudah hidup di engine sejak lama dan tidak dipanggil siapa pun.
Dua baris menu TablePro yang lain sudah ada lebih dulu dan tidak digandakan: "Default page size"
adalah Result rows (`defaultRowLimit`), dan "Maximum entries" adalah Rows the panel reads
(`historyLimit`).

Satu cacat ikut tertangkap tes saat arah klik pertama dibuat dapat disetel: siklus lama
`ascending → descending → off` membuat arah `descending` sebagai klik pertama **tidak pernah bisa
mencapai ascending**, karena klik keduanya langsung menghapus sort. Sekarang siklusnya
"arah pertama → arah lain → off", jadi kedua arah tetap terjangkau dari titik mana pun.

Deskripsi panjang di semua pane settings juga pindah ke belakang glyph `?` (`HelpHint`, di
`Support/Theme.swift`): paragraf di bawah tiap judul membuat tiap kartu dua paragraf dalam dan
mendorong kontrol pertama turun, sehingga jendela setelan terbaca sebagai prosa berisi kontrol.
Teksnya tetap ada, satu hover saja.

**Empat dari delapan tugas TablePro belum dibangun.** Daftar di bawah menyimpan apa yang
masing-masing butuhkan, supaya tidak diusulkan ulang tanpa alasan.

1. **Date format.** Transformasi tampilan untuk kolom tanggal dan timestamp. `ColumnFormatStore`
   sudah memegang format **per kolom**; yang belum adalah setelan global yang dipakai kolom tanggal
   yang belum punya format sendiri. Ukuran: sedang. Risiko: dua sumber format harus punya urutan
   menang yang jelas, dan menambah setelan harus tidak membuat sel yang sudah diatur berubah sendiri.
2. **Auto-show inspector saat baris dipilih.** Buka `CellValueViewer` begitu seleksi berubah.
   Ukuran: kecil. Risiko: seleksi grid hari ini dipakai untuk salin dan untuk edit sel, jadi
   membuka pembaca di setiap perubahan seleksi harus tidak menghalangi keduanya — karena itu ia
   setelan, bukan perilaku tetap.
3. **Smart value detection.** Mengenali bentuk nilai **per sel** (JSON, UUID, URL) alih-alih per
   kolom seperti `ColumnFormat` sekarang. Ukuran: sedang. Risiko: tiap sel jadi perlu penafsiran
   sendiri, dan itu biaya pada grid yang sudah menggambar ribuan baris.
4. **Default row sort.** TablePro menerapkan urutan saat tabel pertama dibuka. Butuh keputusan lebih
   dulu: sebuah setelan default tidak bisa menyebut nama kolom tabel yang belum dibuka, jadi
   pilihannya harus struktural (tanpa urutan, primary key, kolom pertama) — dan itu keputusan produk,
   bukan keputusan teknis.

**Yang sengaja tidak dibangun:**

- **Count rows if estimate less than.** Butuh estimasi jumlah baris yang belum dimiliki engine
  (`count` yang ada menghitung persis). Membangun estimasi hanya untuk memutuskan kapan menghitung
  persis adalah biaya yang tidak dibayar oleh manfaatnya.
- **Query result row cap (Truncate + Row cap + Fetch All + Execute Without Limit).** Tumpang tindih
  dengan `rowLimit` per tab yang sudah ada dan sudah dikirim sebagai `LIMIT`. Bedanya adalah cap
  yang **tidak** mengubah teks statement; menambah jalur kedua untuk hal yang sama akan membuat dua
  arti untuk satu angka. Perlu keputusan dulu apakah `rowLimit` yang ada cukup.
- **Query history retention (Keep entries for + Auto cleanup).** Butuh perintah engine baru: hapus
  baris lebih tua dari N hari. `history_clear` yang ada hanya bisa menghapus semua atau satu
  koneksi, jadi ini pekerjaan engine (Rust) plus UI, bukan setelan yang tinggal disakelar.
- **Data Rewind.** Memulihkan hasil save yang sudah commit. Ukuran: besar, dan TablePro sendiri
  menggantungkannya pada lisensi — sedangkan §9 menolak lisensi. Undo sebelum commit sudah ada
  (`CellEdits` dan penggabungan undo); yang belum adalah undo setelah commit.

### Batch 7: sort dan filter ke server sebagai default

**Diputuskan 29 Sep 2026.** Sort dan search yang sekarang in-memory atas halaman yang sudah
difetch menjadi **server-first**, dengan jalur in-memory sebagai fallback. Alasan: urutan dan
pencarian yang hanya melihat baris yang sudah difetch tidak bisa menjawab pertanyaan yang
sesungguhnya ("baris mana di seluruh hasil"), dan `rowLimit` membuat batas itu sering tercapai.

Yang sudah ada dan dipakai, bukan dibangun baru: `ServerSort.order(sql:)` membungkus SQL pengguna
dalam derived table dengan `ORDER BY`, `SearchStatement.crossColumn(sql:)` membungkusnya dengan
`WHERE` lintas kolom, dan keduanya berjalan lewat jalur `preview` biasa — jadi Safe Mode, timeout,
retry dan streaming tetap sama, dan teks di editor tidak pernah berubah (`previewBaseSQL`).
Perubahannya adalah siapa yang **default**, bukan mekanismenya.

Tiga hal yang harus dijawab sebelum dikodekan, karena masing-masing mengubah perilaku:

1. **Apa arti "sort dimatikan".** Siklus header adalah ascending → descending → off. Dengan sort di
   server, "off" berarti menjalankan ulang SQL dasar — query ketiga untuk satu siklus klik. Pilihan
   yang perlu diputuskan: jalankan ulang SQL dasar, atau simpan hasil dasar dan kembalikan tanpa
   query. Yang kedua lebih cepat tetapi menahan satu hasil penuh di memori per tab.
2. **Apa yang ditunjuk indikator header.** Chevron di header harus mengikuti sort yang **aktif**,
   entah itu dari server atau in-memory, dan `tab.gridSort` tidak lagi menjadi satu-satunya sumber.
   Kalau keduanya bisa aktif bersamaan, dua indikator akan berbeda pendapat.
3. **Kapan fallback dipakai, dan apa yang dikatakan.** `ServerSort` dan `SearchStatement` menolak
   teks yang memuat lebih dari satu statement, dan search menolak bila tidak ada kolom yang bisa
   dibaca sebagai teks. Di situlah in-memory dipakai — dan di situ pula caveat "urutan ini parsial"
   masih benar, jadi satu baris tipis tetap ada. Di jalur server bannernya hilang seluruhnya, karena
   urutannya sudah utuh.

Search di server butuh **debounce dan panjang minimum** (2–3 karakter): satu query per ketukan tidak
mungkin, dan satu query untuk satu huruf hanya membuang waktu server.

### Di luar agen

Rotasi key kenari, karena PROGRESS.md sudah menyatakannya bocor. Notarisasi dan kunci EdDSA Sparkle
diblokir Apple Developer ID, jadi dikerjakan saat mau rilis.

## Yang di-drop atau ditunda

| Item | Alasan |
|---|---|
| Format kolom diterapkan di ekspor | Batasnya sudah ditulis di kode; spec yang dijalankan engine adalah fitur yang belum diminta siapa pun |
| Sisa §12 (12.2.2, MCP §12.3) | 12.2.2 dinilai rendah oleh plan sendiri; MCP tidak menyisakan apa pun untuk dibangun |
| Toggle editor untuk nomor baris dan folding | Fiturnya selalu menyala; toggle untuk yang selalu menyala tidak berguna, dan sisanya fitur baru |
| Notifications | Belum ada keputusan, dan belum ada permukaan notifikasi di app |
| Command dan UI profiles | Sampai checkpoint Batch 2 dijawab; CRUD tanpa konsumen itu spekulatif |
| Gelombang 4 selain Open Quickly | Structure editor menambah permukaan DDL besar yang bentrok dengan Safe Mode; backup PostgreSQL berarti urusan versi `pg_dump`; external API sudah dicakup MCP |
| Riset Navicat/DataGrip dan versi minimum | Mencatat "versi teruji" adalah sikap yang jujur dan sudah terdokumentasi |

## Verifikasi

`cargo test` minimal 770/0 dengan dua tes baru; `swift build && swift test` minimal 392/0; dan satu
jalan manual `table_op` dengan `SAFE_MODE=confirm SAFE_MODE_FLOOR=no_ddl SAFE_MODE_CONFIRMED=1` yang
menunjukkan penolakan `no_ddl`.
