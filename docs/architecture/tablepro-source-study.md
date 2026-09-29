# Studi sumber TablePro: tujuh area yang menopang Fase 2 sampai 5

> Pasangan kedua dari [`tablepro-feature-analysis.md`](tablepro-feature-analysis.md), setelah
> [`tablepro-adoption-plan.md`](tablepro-adoption-plan.md). Dokumen itu membaca dokumentasi TablePro;
> dokumen ini membaca **kodenya**. Tujuannya satu: memberi sesi berikutnya pola, urutan, invariant,
> dan jebakan yang sudah terverifikasi, bukan deskripsi fitur.
>
> **Akses:** 29 Sep 2026, TablePro pada HEAD `8fd32c80b` (`TablePro/`, 6.711 berkas Swift, 371 MB).
> **Batas lisensi:** TablePro AGPL-3.0, QueryHive MIT (`LICENSE`, ADR-0002). Tidak ada baris kode
> TablePro yang boleh masuk ke pohon ini, dan tidak ada satu pun yang masuk ke dokumen ini.

## 1. Batas lisensi dan metode

Sama seperti analisis sebelumnya: yang diambil hanya ide. Berkas ini menuliskan *bagaimana* sesuatu
bekerja, *urutan* langkahnya, dan *jebakan* yang sudah menggigit TablePro. Tidak ada badan fungsi
TablePro yang disalin, tidak ada potongan kode dalam dokumen ini. Beberapa pola praktis tidak bisa
dipisahkan dari kodenya, dan bagian itu ditandai **out of bounds**: polanya boleh, implementasinya
wajib ditulis ulang di sini, dan sesi berikutnya tidak boleh "mengambil sebagai referensi" lalu
menyalin.

Metode baca, karena 6.711 berkas tidak bisa dibaca dengan membaca:

- `TablePro/CLAUDE.md` dibaca penuh (403 baris, padat invariant dan jebakan performa) dan
  `.swift` dicari dengan `grep`/`glob`, bukan di-`ls -R`.
- Untuk tiap area, yang dibaca adalah model, generator, executor, dan gate-nya, bukan view-nya,
  kecuali view itu yang *adalah* mekanismenya (sheet review SQL, panel find, viewer JSON).
- Yang sengaja **tidak** dibaca dicantumkan di §10, supaya klaim di dokumen ini bisa ditelusuri.

Temuan tiap area disusun sama: berkas yang dibaca, cara TablePro, apa yang sudah ada di QueryHive,
apa yang layak diambil, dan rekomendasi untuk fase yang memakainya.

## 2. Change tracking: edit sel, insert, delete, dan tinjau SQL

Ini Fase 5.3, dan bagian TablePro yang paling matang. Nilainya bukan di UI-nya, melainkan di
pemisahan yang tegas antara **menyimpan perubahan**, **membangun rencana**, dan **menjalankannya**.

**Yang dibaca.** `TablePro/CLAUDE.md` §Change Tracking Flow; `TablePro/Core/ChangeTracking/`
(`DataChangeManager.swift`, `PendingChanges.swift`, `SQLStatementGenerator.swift`,
`AnyChangeManager.swift`, `SQLWriteBatchBudget.swift`, `RowMatchPolicy.swift`,
`BulkDeleteConfirmation.swift`); `TablePro/Core/DataWrite/`
(`DataWritePlan.swift`, `DataWriteExecutor.swift`, `RowWriteOperationBuilder.swift`,
`DataWriteError.swift`, `DataWriteRowCounts.swift`); `TablePro/Core/Services/Execution/`
(`WriteTransactionOwner.swift`, `BatchStatementRun.swift`); `TablePro/Core/Coordinators/`
(`RowEditingCoordinator+SaveChanges.swift`); `TablePro/Views/Main/Extensions/`
(`MainContentCoordinator+SQLPreview.swift`, `MainContentCoordinator+RowOperations.swift`);
`TablePro/Views/Components/SQLReviewSheet.swift`; dan `docs/features/change-tracking.mdx`.

**Cara TablePro.** Antreannya adalah nilai, bukan sekumpulan array yang dijaga terpisah.
`PendingChanges` adalah satu `struct` yang memiliki invariant lintas-lintas kumpulan sekaligus:
daftar `changes`, indeks `RowChangeKey → posisi`, himpunan `deletedRowIDs` dan `insertedRowIDs`,
peta `modifiedCells`, dan `insertedRowData`. Karena pembatalan menghapus dengan cara menukar elemen
terakhir ke slot yang dibuang, urutan array tidak bisa dipercaya; setiap perubahan karena itu
mendapat **cap `sequence`** saat dicatat, dan generator SQL membaca ulang dengan urutan itu. Ini
detail yang mudah terlewat: sebuah baris dihapus untuk membebaskan satu nilai unik, lalu baris baru
mengambil nilai itu, hanya benar kalau `DELETE`-nya jalan lebih dulu.

Edit yang sedang diketik di-coalesce. `DataChangeManager` menahan sel yang editornya masih terbuka
di sebuah buffer dan mendaftarkannya ke `NSUndoManager` sebagai satu langkah saat run berakhir, jadi
satu kata yang diketik adalah satu undo, bukan satu undo per karakter. Sel yang diketik balik ke
nilai semula tidak menjadi langkah undo sama sekali. Ini pola yang layak ditiru apa adanya untuk
QueryHive: undo per keystroke adalah keluhan yang pasti muncul.

Pembangkit SQL memakai parameter, bukan escaping. Bentuk placeholder dipilih per dialect
(`$1` atau `?`). Kolom yang dihitung server dibuang dari setiap `INSERT` dan `UPDATE`. Nilai
`DEFAULT` diangkut sebagai sentinel teks dan dirender sebagai kata kunci `DEFAULT` atau sebagai
ekspresi fungsi (`NOW()`, `CURRENT_DATE`) yang tidak di-quote. Baris yang seluruh kolomnya diisi
server punya spell tersendiri per engine (`INSERT … DEFAULT VALUES`, `INSERT … () VALUES ()`,
varian `DEFAULT`), dan itu bukan hiasan: generator yang mengembalikan `nil` untuk baris seperti itu
pernah membuat barisnya hilang dari batch sementara sisanya commit dan melaporkan sukses.

Pencocokan baris punya dua mode, dan mode keyless adalah sumber sebagian besar jebakannya. Kalau
tabel punya PK, `WHERE` dibangun dari PK, dan kalau tidak, dari **seluruh kolom asli** dengan
`IS NULL` untuk null. Tabel tanpa PK tidak bisa membedakan dua baris identik, jadi satu edit bisa
menulis dua baris. `RowMatchPolicy` menampung kolom yang tidak boleh ikut mencocokkan sama sekali
(`excludedColumns`) dan kolom yang harus dibandingkan lewat **render teks server**
(`textColumns`, di MySQL `CONCAT(col)`), karena teks yang dibaca grid tidak selalu sama dengan
nilai asalnya untuk `FLOAT`, `JSON`, dan sejenisnya.

Rencananya adalah objek tersendiri. `DataWritePlan` memuat daftar `DataWriteStep` berurutan, dan
setiap step membawa `expectedRowCount`, `tableName`, dan penanda `matchesRowsWithoutKey`. Rencana
juga memuat `prologue` dan `epilogue`: pernyataan `PRAGMA foreign_keys = OFF/ON` dan sejenisnya
harus jalan **di luar** transaksi, karena `PRAGMA` itu no-op di dalam transaksi SQLite. Save dan
Preview SQL membaca objek yang sama, jadi yang ditampilkan adalah yang dijalankan.

Review sebelum eksekusi adalah bagian dari kontrak, bukan tambahan. `Cmd+Shift+P` memanggil
`buildDataWritePlan` yang sama, lalu `displayStatements` menyalin nilai parameter ke dalam SQL
(`SQLParameterInliner`) supaya pengguna tidak menyetujui `WHERE id = ?`. `SQLReviewSheet` menampilkan
teks itu dengan syntax highlighting, batas tampilan 20.000 karakter, dan tree-sitter hanya sampai
8.000 karakter. Satu detail yang penting dan sengaja berbeda: sheet **konfirmasi** tidak boleh
memotong teks (`showsStatementsVerbatim`), karena `WHERE`-nya bisa duduk di balik potongan mana pun,
dan menyetujui yang tidak terlihat adalah hal yang justru dicegah dialog itu.

Transaksinya dikelola eksplisit. `WriteTransactionOwner` menjawab tiga kemungkinan: `app` (buka,
verifikasi, commit/rollback sendiri), `session` (sudah ada transaksi milik pengguna atau klien lain;
jangan kirim `BEGIN`/`COMMIT`/`ROLLBACK`), dan `none` (engine tanpa transaksi, tiap statement
langsung di server). Ini diputuskan dari `supportsTransactions` driver **dan** state transaksi sesi
yang sedang dipegang, dan itu jawaban yang benar: membuka transaksi di atas transaksi pengguna
diam-diam men-commit pekerjaan mereka di PostgreSQL, men-commit implisit di MySQL, dan menggagalkan
transaksi di DuckDB.

Verifikasi jumlah baris adalah lapisan yang tidak dimiliki QueryHive. Setelah tiap statement,
`DataWriteExecutor` membandingkan `rowsAffected` dengan `expectedRowCount`. Untuk write ber-PK
aturannya `actual > expected` (bukan `!=`, karena MySQL melaporkan nol baris untuk `UPDATE` yang
menulis nilai yang sudah ada, dan itu save yang normal). Untuk write tanpa PK aturannya dua arah:
`actual < expected` berarti barisnya sudah tidak ada. Kalau gagal, transaksi di-rollback. Jebakan
yang sudah nyata: pada MySQL 8.4.11 dan OceanBase, baris berisi `FLOAT` atau `JSON` tidak cocok
sama sekali, statement menulis nol baris, dan save lama melaporkan sukses sementara editnya hilang
saat reload.

Kegagalan sebagian punya kosakata sendiri, dan itu yang membedakannya dari patch biasa.
`DataWritePartialCommitError` melaporkan berapa statement yang sudah jalan, dari berapa, dan
**disposisinya**: `written` (tidak ada transaksi atau rollback-nya sendiri gagal, jadi menjalankan
save lagi berarti menulis dua kali) atau `pendingInSessionTransaction` (menunggu di transaksi
pengguna, hanya pengguna yang bisa memutuskan). Pesannya menyebut engine-nya, dan saran
pemulihannya menyuruh melihat isi tabel, bukan mencoba lagi. Ada satu lagi di `RowEditingCoordinator`:
save dikunci satu-per-window (`beginSaveInFlight`), rencana membawa scope tempat ia dibangun
supaya sheet dan prompt otorisasi yang lama tidak membuat statement mendarat di database lain, dan
tab yang memulai save yang memiliki hasilnya, bukan tab yang terpilih saat write mendarat.

DDL punya catatan tersendiri. MySQL, MariaDB, dan Oracle men-commit setiap `DROP` saat ia jalan, jadi
save yang gagal separuh bisa sudah menjatuhkan tabel yang rollback-nya tidak bisa mengembalikan.
TablePro menyegarkan katalog, menandai tabel yang benar-benar hilang, dan tidak mengantre ulang hal
yang tidak bisa dibatalkan; itu dicatat sebagai operasi yang sudah terjadi.

**Yang sudah ada di QueryHive.** `app/Sources/TrinoExporter/Models/CellEdits.swift` menyimpan nilai
sel yang berubah, dan `Models/UpdateStatements.swift` menghasilkan `UPDATE` beserta predikatnya.
Tidak ada insert, delete, rencana, review, transaksi, verifikasi jumlah baris, atau kosakata
partial-commit. Antreannya hidup di sisi Swift saja.

**Layak diambil sebagai pola.**

- Antrean sebagai **satu nilai** yang memiliki invariant lintas-kumpulan, dengan cap urutan
  terpisah. `PendingChanges` adalah contoh yang sudah punya alasan untuk tiap bagiannya.
- **Bangun rencana, tinjau rencana, jalankan rencana.** Satu objek `DataWritePlan` yang dipakai
  save dan preview; review menyalin parameter ke dalam SQL, bukan menampilkan bentuk ber-`?`.
- **Verifikasi jumlah baris di dalam transaksi.** `actual > expected` untuk ber-PK,
  `actual < expected` untuk tanpa PK, dan engine yang tidak melaporkan jumlah nyata jangan dipaksa
  (TablePro punya `reportsRowsAffected`; QueryHive sudah punya `Capabilities`).
- **Kosakata partial-commit** dengan saran pemulihan yang membedakan "sudah tertulis" dari
  "menunggu di transaksi pengguna".
- **Pencocokan keyless** yang mengucapkan hal yang tidak bisa: PK hilang jangan berarti "match semua
  kolom dan berharap". Kolom yang tidak bisa dibandingkan harus dikecualikan, dan kolom teks
  dibandingkan lewat render server.
- **Batching berbudget**: jumlah baris, byte, dan plafon bind-parameter engine sekaligus, dengan
  batch ditutup **sebelum** baris yang akan melewatinya.
- **Tombol save yang membawa scope** dan tidak bisa jalan dua kali bersamaan.

**Out of bounds.** Badan `SQLStatementGenerator`, `DataWriteExecutor`, `RowWriteOperationBuilder`,
dan `RowEditingCoordinator+SaveChanges`. Polanya boleh; SQL-nya harus ditulis ulang terhadap
`crates/qh-driver` dan `crates/qh-sql` yang sudah ada.

**Rekomendasi Fase 5.3.** Tulis model antrean sebagai nilai Rust di engine (`qh-sql` atau crate
baru), bukan sebagai tambahan di Swift, supaya CLI dan MCP dapat menginspeksi rencana yang sama.
Bentuk yang paling dekat dengan aturan proyek ini: satu perintah `plan_changes` yang mengembalikan
daftar statement (dengan parameter ter-inline untuk ditampilkan) dan satu perintah `apply_changes`
yang menjalankannya dalam satu transaksi dengan verifikasi jumlah baris. Insert dan delete baris
datang sebagai jenis perubahan baru pada antrean yang sama, bukan jalur kedua. Untuk `UPDATE` yang
sudah ada, tambahkan verifikasi jumlah baris sebelum menambah fitur baru: itu satu perubahan kecil
yang menutup lubang "UPDATE tanpa WHERE cocok nol baris tapi melaporkan sukses".

## 3. Impor CSV, JSON, SQL, dan XLSX

Ini Fase 5.1. TablePro membelah impor jadi dua keluarga, dan pembelahan itulah keputusan
desainnya yang paling berharga.

**Yang dibaca.** `Plugins/{CSVImportPlugin,JSONImportPlugin,SQLImportPlugin,XLSXImportPlugin}`
(empat berkas tiap plugin, kecuali SQL yang punya `SQLImportFailure.swift`); `TablePro/Core/Services/Export/`
(`ImportService.swift`, `ImportErrorReport.swift`, `ImportRouting.swift`); `TablePro/Core/Plugins/`
(`ImportDataSinkAdapter.swift`, `ImportTypeMapper.swift`, `PlainFileImportSource.swift`,
`SqlFileImportSource.swift`); `Plugins/TableProPluginKit/`
(`ImportFormatPlugin.swift`, `PluginImportSource.swift`, `PluginImportDataSink.swift`,
`PluginImportTypes.swift`, `RowImportRunner.swift`, `CSVStreamingParser.swift`,
`CSVTypeInferrer.swift`); `TablePro/Views/Import/RowImportSheet.swift`.

**Cara TablePro.** Plugin impor membawa flag `requiresTargetTable`. Yang `true` (CSV, XLSX, JSON)
adalah impor **baris**: plugin menghasilkan batch `(line, row)` lewat closure, dan runner bersama
yang mengurus sisanya. Yang `false` (SQL) adalah impor **statement**: plugin membaca stream
statement dari berkas dan mengirim satu per satu ke sink. Dua keluarga, dua pengendali transaksi,
satu `PluginImportDataSink` yang sama.

`RowImportRunner` adalah tempat seluruh keputusan sulit dikumpulkan: membuka transaksi,
menghapus baris lama bila diminta (`deleteExistingRows`), mengurus tiga mode kesalahan
(`stopAndRollback`, `stopAndCommit`, `skipAndContinue`), progres, batas 1.000 error yang dicatat,
dan nomor baris di setiap pesan error. Satu keputusan yang layak dikutip: transaksi **dimatikan**
di mode `skipAndContinue`, karena rollback tidak bisa "melewati" baris yang gagal. Dan error
pembatalan dibiarkan lewat sebagai tipe sendiri, supaya Stop di mode stop-and-commit tidak
me-commit prefix lalu melaporkan pembatalan sebagai kegagalan impor.

Mode `skipAndContinue` benar-benar per baris: ia memanggil `insertRow` satu per satu, bukan
`insertRows`, sehingga baris buruk tidak menjatuhkan tetangganya. Baris yang tidak punya satu pun
nilai yang mencapai kolom termapping **ditolak**, bukan dilewati sambil dihitung sebagai
"ter-insert"; doc string-nya menyebut alasan yang tepat: itu yang dulu membuat "Import completed"
melebih-lebihkan apa yang mendarat.

CSV-nya bukan streaming byte murni, dan ini penting untuk tidak salah menyalin klaimnya. Plugin
membuka berkas dengan `Data(contentsOf:options:.mappedIfSafe)` — mmap, bukan baca ke heap — lalu
`CSVStreamingParser.indexRows` mengembalikan **rentang baris**, bukan string. Parsing batch 500
baris mengambil potongan dari mapping itu. Baris kosong dilewati tapi tetap dihitung di progres.
Deteksi kolom hanya membaca prefiks 1 MiB lewat `FileHandle`. Jadi yang dijaga TablePro adalah "tidak
menahan isi berkas di heap", bukan "membaca satu baris lalu membuangnya"; QueryHive yang sudah
punya penulis streaming sebenarnya bisa lebih ketat dari ini.

XLSX dibaca utuh, dan TablePro mengatakan alasannya di kode: baris sebuah sheet terjalin dengan
`sharedStrings.xml` yang dirujuk indeksnya, jadi pembacaan streaming tetap harus menahan tabel
string itu, dan formatnya tidak memberi cara mengetahui ukurannya sebelum di-parse. `ZipArchive`
(pertama, bukan pustaka pihak ketiga) plus `XMLParser`; sel yang tipenya `s` adalah indeks ke
`sharedStrings`, dan baris **tidak menulis sel kosongnya**, jadi posisi kolom diambil dari referensi
`r` milik tiap sel (`C4`), bukan dari menghitung. Workbook rusak yang kehilangan indeks string tetap
mengembalikan teks mentahnya, supaya pengguna melihat ada yang salah.

Column mapping adalah langkah UI yang menghasilkan peta `[field: column]`. Untuk tabel yang sudah
ada, kolom dicocokkan **case-insensitive dengan nama field**, dengan sakelar `include` per field dan
pilihan kolom target; untuk tabel baru, tipe disimpulkan lalu dipetakan ke spell tiap engine
(`ImportTypeMapper`), dan pengguna menyunting nama, tipe, nullable, PK, dan default. Sink
menurunkan huruf field dan memetakan ke kolom lewat peta itu. Pengelompokan baris yang berbagi satu
set kolom terjadi di sink, lalu satu `SQLWriteBatchBudget` per kelompok membatasinya.

SQL import mengalirkan statement dari `SqlFileImportSource.statements()` dan bergantung pada
grammar engine untuk membelahnya. SQL Server punya jalur sendiri: `GO` bukan statement, jadi batch
dikirim utuh (T-SQL meng-scope variabel dan `TRY…CATCH` per batch), dan driver yang tidak bisa
mengirim batch menjalankannya satu per satu. FK checks dimatikan sebelum dan dinyalakan lagi di
kedua jalur keluar, commit maupun rollback.

**Yang sudah ada di QueryHive.** Tidak ada impor sama sekali. Jadi area ini murni celah.

**Layak diambil sebagai pola.**

- **Dua keluarga berdasarkan `requiresTargetTable`**, bukan satu jalur yang menangani keduanya.
- **Runner bersama yang memiliki transaksi dan matriks mode kesalahan**, termasuk aturan bahwa
  `skipAndContinue` tidak memakai transaksi.
- **Nomor baris di setiap error**, karena "baris 17.483 gagal" adalah satu-satunya hal yang membuat
  impor besar bisa diperbaiki.
- **Baris yang tidak termapping ditolak**, bukan dihitung sukses.
- **Mapping case-insensitive + sakelar include + pilihan kolom target**, dan untuk tabel baru
  pemetaan tipe per engine.
- **FK off/on di sekitar impor**, dinyalakan di kedua jalur keluar.
- **Batas error** (TablePro 1.000) supaya file yang benar-benar salah tidak menghabiskan memori
  untuk pesan.
- **mmap + indeks rentang baris** untuk CSV kalau QueryHive memang perlu membaca utuh; tetapi
  QueryHive sudah punya penulis streaming, jadi pembaca streaming yang simetris lebih baik dan
  sejalan dengan aturan `README.md`.

**Out of bounds.** Badan `CSVStreamingParser`, `RowImportRunner`, `XLSXSheetParser`, dan
`ImportDataSinkAdapter`. `CSVStreamingParser` khususnya: tokenizer RFC 4180 dengan deteksi dialect
dan penanganan encoding adalah pekerjaan yang harus ditulis QueryHive sendiri, atau diambil dari
crate MIT/Apache (mis. `csv`), bukan dari TablePro.

**Rekomendasi Fase 5.1.** Karena perintah `import_data` akan hidup di engine Rust, tulis dua jalur
yang jelas: `import_rows` (CSV/XLSX/JSON) dengan mapping eksplisit dan mode kesalahan yang sama
tiga itu, dan `import_sql` (statement). Untuk XLSX, jangan menjanjikan streaming yang tidak mungkin;
nyatakan bahwa ia membaca workbook utuh, pilih crate zip + XML yang lisensinya lolos `cargo deny`,
dan batasi ukuran yang mau dibuka. Untuk CSV, pakai pembaca streaming dan jangan menahan file.
Transaksi harus dimatikan di mode skip, persis alasan TablePro.

## 4. Ekspor, termasuk Parquet

Ini Fase 5.2. Jawaban singkat untuk pertanyaan yang paling penting: Parquet di TablePro **bukan
encoder sendiri**, melainkan DuckDB yang di-link sebagai static library.

**Yang dibaca.** `Plugins/ParquetExportPlugin/`
(`ParquetExportPlugin.swift`, `DuckDBStagingDatabase.swift`, `ParquetTypeMapper.swift`,
`ParquetExportModels.swift`); `Plugins/XLSXExportPlugin/`
(`XLSXExportPlugin.swift`, `XLSXWriter.swift`); `project.yml` target `ParquetExport` dan
`XLSXExport`; `Packages/TableProCore/Sources/TableProTabularIO` (zip pertama);
`docs/features/import-export.mdx`.

**Cara TablePro.** Parquet adalah plugin **registry-only**, bukan bundled, dan alasannya tertulis di
`project.yml`: ia meng-link salinan `libduckdb.a` sendiri, dan itu terlalu besar untuk ikut dalam
bundel hanya demi satu format. Alurnya: baris di-stream dari engine sumber, distage ke DuckDB
in-memory yang `SET temp_directory` ke direktori sementara supaya bisa spill (kalau tidak, tabel
lebih besar dari RAM akan gagal, bukan melambat), `CREATE TABLE` dengan tipe hasil pemetaan
`ParquetTypeMapper`, `INSERT` multi-baris per 2.000 baris, lalu `COPY … TO … (FORMAT PARQUET,
COMPRESSION …, ROW_GROUP_SIZE …)`. Setiap nilai non-`VARCHAR`/non-`BLOB` dibungkus `TRY_CAST` ke tipe
kolomnya, dengan alasan yang tepat: satu timestamp yang tidak bisa di-parse di antara sejuta baris
menulis null, bukan menggagalkan seluruh statement. `preserve_insertion_order = false` dilepas
karena DuckDB sendiri yang mengurus urutan untuk kompresi. Parquet satu tabel per file; ekspor
multi-tabel menulis satu file masing-masing dan memberi peringatan.

XLSX export adalah kebalikannya: penulis pertama (`XLSXWriter`, zip + XML) yang mengalirkan baris
per 5.000, `autoreleasepool` per batch, dan memecah sheet di batas 1.048.576 baris milik Excel.
Jadi penghematan memorinya beda: Parquet menahan staging DuckDB (dengan spill), XLSX benar-benar
streaming ke zip.

**Yang sudah ada di QueryHive.** Sembilan format streaming di `crates/qh-export` (`writers.rs`,
`zip.rs`, `xlsx.rs`, `xls.rs`, `dbf.rs`, `encoding.rs`), dengan part splitting, retry, `ZIP`, dan
`BATCH_SIZE`. Tidak ada format kolumnar.

**Layak diambil sebagai pola.**

- **Staging untuk membatasi kompleksitas encoder**, kalau encoder-nya memang rumit; dan **spill ke
  disk** kalau staging-nya di memori.
- **Cast per nilai dengan `TRY_CAST`** sehingga satu nilai buruk tidak menggagalkan file.
- **Satu file per tabel** untuk Parquet, karena Parquet memang satu tabel per file.
- **Pemecahan sheet di batas format** (Excel), bukan di batas memori.
- **`autoreleasepool` per batch** untuk menekan puncak memori saat menulis.

**Out of bounds.** `DuckDBStagingDatabase` dan `ParquetExportPlugin`. Selain AGPL, menjadikan
`libduckdb` dependency QueryHive adalah keputusan arsitektur besar yang tidak perlu: Rust sekarang
punya crate `parquet`/`arrow` (MIT/Apache-2.0) yang cocok dengan penulis streaming yang sudah ada.
Menyalin alur staging DuckDB justru berarti menambah library C besar untuk hal yang bisa ditulis
native.

**Rekomendasi Fase 5.2.** Format kesepuluh di `crates/qh-export` memakai crate `parquet` native,
bukan libduckdb. Pertahankan kontrak streaming yang sudah ada (jangan menahan result set), dan ikuti
pola `try_cast` yang setara di level Rust: satu nilai yang tidak bisa dipetakan ke tipe Arrow
menjadi null, bukan kegagalan ekspor. Kriteria selesai di rencana sudah benar: bandingkan hasilnya
dengan pembaca independen. Tambahan yang layak: ekspor satu file per tabel dan peringatan saat
tabelnya banyak, karena itu kelakuan yang benar untuk Parquet.

## 5. Safe Mode dan query timeout

Fase 3. Area ini menarik karena QueryHive sudah mengerjakannya, jadi yang dicari adalah hal yang
**belum** diambil, bukan fitur yang hilang.

**Yang dibaca.** `TablePro/Models/Connection/`
(`SafeModeLevel.swift`, `SafeModeFloor.swift`, `SafeModeStatus.swift`);
`TablePro/Core/Services/Execution/`
(`DefaultExecutionGate.swift`, `ExecutionGate.swift`, `ExecutionGateProvider.swift`,
`OperationRequest.swift`); `TablePro/Core/Utilities/SQL/QueryClassifier.swift`;
`TablePro/Core/Services/Policy/` (`ManagedPolicy.swift`, `AgentModeSafeModeFloor.swift`);
`Plugins/TableProPluginKit/` (`AsyncTimeout.swift`, `HttpQueryTimeout.swift`,
`HttpQueryTimeoutBox.swift`); `Plugins/MySQLDriverPlugin/`
(`MySQLQueryTimeout.swift`, `MySQLSocketTimeout.swift`); `Plugins/PostgreSQLDriverPlugin/LibPQDriverCore.swift`;
`TablePro/Core/Database/DatabaseManager+Health.swift`; `docs/features/safe-mode.mdx`.

**Cara TablePro.** Safe Mode punya **enam tingkat**, bukan tiga: `silent`, `alert`, `alertFull`,
`safeMode`, `safeModeFull`, `readOnly`, diurutkan dengan `strictness` (bukan urutan deklarasi).
Tingkat menentukan tiga hal yang berbeda: `blocksAllWrites`, `requiresConfirmation`, dan
`requiresAuthentication`; dua tingkat terakhir memakai Touch ID (fallback ke password macOS).
Tingkat `silent` bukan izin bebas: `DROP`, `TRUNCATE`, dan `DELETE` tanpa `WHERE` tetap memicu
peringatan query berbahaya bawaan. `readOnly` melampaui query: ia mematikan edit sel, tambah,
hapus, duplikat baris, truncate, drop, dan impor di antarmuka.

Ada konsep yang tidak muncul di analisis sebelumnya: **floor**. `SafeModeFloor` menaikkan tingkat
minimum dari empat kondisi yang independen — engine read-only, database remote file, kebijakan
organisasi lewat macOS configuration profile, dan Agent mode — dan memilih yang **paling ketat**
berdasarkan `strictness`, bukan yang pertama ditemukan. Ini bukan detail kosmetik: rantai
first-match lama hanya benar selama urutannya kebetulan tepat, dan menambah kondisi keempat
membuatnya salah. Yang penting untuk QueryHive: floor tidak pernah tertulis ke pengaturan
pengguna, jadi kondisi hilang = tingkat pengguna kembali.

Penegakannya ada di **execution gate actor** di sisi app, bukan di driver. Gate mengklasifikasi
tier statement, menandai query berbahaya, memeriksa kemampuan pemanggil (`mayWrite`,
`mayRunDestructive`, `mayRunMultiStatement`, `confirmationPreCleared`), menolak write di tingkat
read-only, lalu meminta konfirmasi dan autentikasi sesuai tingkat. Setiap keputusan — diizinkan
atau ditolak — dicatat ke execution log lokal; pernyataannya disimpan sebagai **SHA-256**, dan tiap
record membawa hash record sebelumnya sehingga rantai bisa diverifikasi ulang. Dokumentasinya jujur
bahwa ini tamper-evident, bukan tamper-proof.

Untuk klien eksternal ada **tiga kunci berurutan**, dan ini model yang sangat berguna untuk
QueryHive: (1) **External Clients** per koneksi (blocked/readOnly/readWrite), (2) **scope token**
per integrasi (`readOnly`/`readWrite`/`fullAccess`), dengan izin efektif = `MIN(token, connection)`,
(3) **Safe Mode** per query. `DROP` dan `TRUNCATE` selalu lewat tool `confirm_destructive_operation`
yang butuh token `fullAccess` dan persetujuan pengguna setiap kali. Klien yang menjawab prompt
elicitation-nya sendiri tidak dianggap sudah menyetujui; hanya token `fullAccess` yang boleh
menggantikan dialog TablePro.

Query timeout dikelola di satu tempat. Pengaturan global `general.queryTimeoutSeconds` (default 60)
disalin ke `additionalFields["queryTimeoutSeconds"]` setiap koneksi dibuat, dan
`applyTimeoutAndStartupCommands` memanggil `applyQueryTimeout(seconds)` pada setiap connect; driver
yang tidak bisa memakai default kosong. Postgres menerjemahkannya ke `SET statement_timeout = ms`.
MySQL memasang statement timeout server per flavor (`max_execution_time` / `max_statement_time`),
dan hanya jatuh ke **deadline sisi klien** kalau server menjawab `ERROR 1193 Unknown system
variable`; scope-nya beda (MariaDB semua statement, MySQL hanya SELECT), dan deadline klien
membunuh statement dengan `KILL QUERY`. Driver membedakan "deadline terlampaui" dari "socket
timeout" berdasarkan berapa lama ia menunggu, karena libmariadb melaporkan keduanya dengan kode dan
teks yang sama. Engine HTTP (ClickHouse, Cloudflare D1/R2, LibSQL, Elasticsearch) memakai
`HttpQueryTimeout`: timeout request = `serverTimeout + 30s` grace, dengan plafon resource 3.600
detik dan timeout bootstrap 60 detik. Ada juga helper `withTimeout` generik, tetapi komentarnya
menyatakan aturan yang mudah salah: `onTimeout` harus **memaksa** operasi selesai (menutup koneksi,
menggagalkan promise), karena anak task-nya tetap berjalan setelah deadline.

**Yang sudah ada di QueryHive.** Lebih banyak dari yang tertulis di rencana Fase 3. `ExecuteOptions`
di `crates/qh-driver/src/lib.rs:177` sudah punya `statement_timeout`, dan `Capabilities` di
`crates/qh-driver/src/lib.rs:147` sudah punya bit `statement_timeout` dengan dokumentasi yang
tepat: driver yang tidak bisa mengatakannya, dan pemanggil tidak menutupinya dengan timer lokal.
`SafeMode` sudah ada di `crates/qh-sql/src/classify.rs` dengan tiga tingkat (`full`, `no_ddl`,
`read_only`), ditegakkan di engine (`crates/qh-ffi/src/commands.rs` lewat fungsi `safe_mode` dan
`guard`), menolak **seluruh script** kalau satu statement ditolak, dan diuji di
`crates/qh-ffi/tests/safe_mode.rs`. `crates/qh-sql/src/classify.rs` juga memperlakukan statement
yang tidak bisa diklasifikasi sebagai write di mode selain `full`, yang sejalan dengan TablePro.

**Layak diambil sebagai pola.**

- **Penegakan di engine**, bukan UI. QueryHive sudah benar di sini dan justru lebih baik dari
  TablePro untuk CLI dan MCP (TablePro tidak punya engine terpisah, jadi gate-nya di app). Jangan
  pindahkan ke Swift.
- **Menolak seluruh script** kalau satu statement ditolak, dengan indeks dan jenis statement di
  pesan. Itu sudah ada dan alasannya benar.
- **Floor dengan alasan dan `max` by strictness**, bukan first-match. Belum dibutuhkan QueryHive
  sampai ada kebijakan terkelola atau Agent mode, tapi bentuknya layak dicatat.
- **Membedakan "app yang menolak" dari "server yang menolak"** di pesan, seperti TablePro lakukan.
- **Pesan yang menyebut batas waktu**, dan yang membedakan deadline dari socket timeout.
- **Untuk driver tanpa mekanisme server**: jangan berpura-pura. `Capabilities::statement_timeout`
  sudah menyatakannya; pesan ke pengguna harus menyebut bahwa driver tidak mendukungnya.
- **Urutan MySQL**: coba statement server dulu, jatuh ke deadline klien hanya pada error "unknown
  system variable", dan hanya untuk scope statement yang benar.
- **HTTP timeout = server + grace, dengan plafon resource.** Berlaku untuk driver HTTP seperti Trino.

**Out of bounds.** `DefaultExecutionGate`, `SafeModeLevel`, `SafeModeFloor`, `QueryClassifier`,
`AsyncTimeout`, `HttpQueryTimeout`, `MySQLQueryTimeout`. `AsyncTimeout` khususnya adalah helper
generik yang gampang disalin; jangan. Tulis versi Rust yang setara atau pakai `tokio::time::timeout`
dengan pemahaman bahwa timeout lokal tidak menghentikan pekerjaan server.

**Rekomendasi Fase 3.** Yang sudah mendarat menutup sebagian besar kriteria. Yang tersisa:
(1) pastikan setiap driver mengisi `Capabilities::statement_timeout` dengan jujur dan
menerjemahkan `ExecuteOptions::statement_timeout` ke mekanisme servernya (Trino
`query_max_run_time`, Postgres `statement_timeout`, MySQL `max_execution_time`); (2) uji bahwa
timeout memicu `EngineError::Timeout` dengan pesan yang menyebut batasnya, bukan hang; (3) untuk
`count`, pakai batas yang sama dan laporkan sebagai perkiraan bila tercapai; (4) untuk Safe Mode,
pertahankan tiga tingkat di engine, dan tulis ADR-nya sebelum menambah tingkat — enam tingkat
TablePro menyelesaikan masalah produk yang berbeda (konfirmasi, Touch ID) yang QueryHive belum
punya; (5) catat bahwa enforcement engine berarti `to_table` mode `replace` dan `DROP TABLE` di
koneksi read-only gagal di engine, sudah seperti yang diminta rencana.

## 6. Editor find/replace, folding, sorting grid, JSON viewer

Fase 4. Empat item, dan TablePro mengerjakannya di tempat yang berbeda-beda: find dan folding di
paket editor sendiri, sorting di grid dan server, JSON viewer di popover sel.

**Yang dibaca.** `Packages/TableProEditor/Sources/TableProEditorKit/`
(`LineFolding/Model/{LineFoldStorage,LineFoldCalculator,LineFoldModel,FoldRange}.swift`,
`LineFolding/LineFoldProviders/LineFoldProvider.swift`, `Find/FindMethod.swift`,
`Find/FindPanelTarget.swift`, `Find/ViewModel/FindPanelViewModel+Find.swift`,
`Find/ViewModel/FindPanelViewModel+Replace.swift`); `TablePro/Core/Utilities/SQL/Folding/`
(`SQLFoldScanner.swift`, `SQLFoldRegion.swift`); `TablePro/Views/Editor/Folding/`
(`SQLLineFoldProvider.swift`, `FoldProviderResolver.swift`);
`TablePro/Core/Coordinators/` (`FindCoordinator.swift`, `FindMatcher.swift`);
`TablePro/Views/Results/` (`JSONViewerView.swift`, `JSONViewerContentView.swift`);
`TablePro/Models/UI/JSONTreeNode.swift`; `TablePro/Models/Query/TableRowsSorting.swift`;
`TablePro/Views/Results/DataGridView.swift` (bagian `syncSortState`);
`docs/features/{code-folding,json-viewer,data-grid,sql-editor}.mdx`.

**Folding.** TableProEditor memakai engine teks sendiri (TextKit + rope), dan folding adalah
lapisan model, bukan mutasi teks. `SQLFoldScanner` memindai dokumen dalam **satu pass** dengan satu
stack frame untuk statement, grup tanda kurung, blok `BEGIN`/`CASE`, komentar blok, dan body
dollar-quoted. Region yang buka dan tutup di baris yang sama dibuang. Fold dimulai di **akhir baris
pembuka**, sehingga yang tersisa di layar adalah `CREATE TABLE users (`. Scanner memakai
**statement-boundary tracker yang sama** dengan kontrol Run di gutter, supaya fold dan run tidak
berbeda pendapat tentang di mana statement berakhir; ini poin yang mudah dilewatkan dan layak
ditiru. `SQLLineFoldProvider` memindai ulang hanya saat panjang teks berubah dan menjawab tiap
baris dengan lookup kamus, jadi biayanya linear per pass, bukan kuadratik. Batas: dokumen di atas
`foldingSizeLimit` (dan di atas 2 juta karakter) tidak dilipat sama sekali. Status buka/tutup
bertahan setelah tab ditutup dan dibuka lagi, dikunci oleh `(depth, start)` supaya fold yang tidak
lagi cocok setelah file berubah akan dibuang.

**Find/replace.** Editor punya panel sendiri, tidak memakai `performFindPanelAction` bawaan
`NSTextView`. Lima metode (`contains`, `matchesWord`, `startsWith`, `endsWith`,
`regularExpression`) semuanya dikompilasi ke `NSRegularExpression`: yang substring di-escape,
`matchesWord` menambah `\b`, `startsWith`/`endsWith` menambah anchor. Replace-all membungkus
seluruh operasi dalam `beginEditing`/`endEditing` dan **satu undo group**, mengganti dari match
terakhir ke pertama, dan menyesuaikan rentang match berikutnya dengan selisih panjang. Panel
didekopel dari editor lewat protokol `FindPanelTarget`, jadi editor lain bisa memakai panel yang
sama. Grid punya "find" yang lain lagi (`FindCoordinator`/`FindMatcher`): pemindaian sel di memori
dengan debounce 120 ms, dan opsi **escalate** yang mengubah istilah menjadi filter cross-column di
server, karena menemukan di baris yang belum dimuat tidak bisa dikerjakan di memori.

**Sorting grid.** TablePro menyortir di **server**, dengan menjalankan ulang query dan `ORDER BY`
baru, sehingga yang terurut adalah seluruh tabel, bukan halaman yang ada di layar. Yang menyortir di
memori hanya hasil yang sudah dimuat penuh (panel hasil agent, hasil query Cassandra/ScyllaDB),
lewat `TableRowsSorting` dengan `RowSortComparator` yang sama, supaya kolom integer tetap terurut
numerik, bukan sebagai teks. `SortColumnResolver` merekonsiliasi state sort terhadap kolom tampilan
setelah kolom disembunyikan, dipindah, atau diganti nama. Tiga jebakan: menyortir kolom `BLOB`,
`JSON`, atau spasial yang tidak bisa di-`ORDER BY` server **menggagalkan query**; kolom tersembunyi
tidak diambil kecuali PK dan kolom yang tersortir; dan "Don't Sort" adalah jawaban **ketiga** yang
kembali ke urutan server dan harus melekat, bukan sekadar menghapus arah.

**JSON viewer.** Chevron di sel membuka popover; mode **Text** menampilkan JSON ter-highlight dan
pretty-printed dan bisa diedit, mode **Tree** collapsible dengan pencarian. `JSONTreeParser`
membatasi input pada **100.000 unit UTF-16** dan pohon pada 5.000 node; melewati batas pertama
berarti "JSON Too Large" dan pengguna disuruh pindah ke Text, dan melewati batas kedua menyisipkan
node penanda potongan. Pretty-print punya batas sendiri 500.000 karakter; hex dump 10 KB; sel blob
di grid 64 byte; pratinjau gambar 16 MB; rasterisasi SVG 3 detik. Format tampilan per kolom
(Raw/Text/UUID/Unix timestamp/JSON/PHP) disimpan per koneksi+tabel dan hanya mengubah rendering,
bukan yang tersimpan; satu sel yang disalin memakai format itu, blok atau baris penuh selalu
menyalin nilai tersimpan. Deteksi pintar berjalan per sel (PNG yang sama bisa ada di `BLOB`,
`BYTEA`, atau `VARCHAR`), dan nilai yang gagal dibuka di satu mode tetap bisa dibuka di mode lain.

**Yang sudah ada di QueryHive.** `app/Sources/TrinoExporter/Views/SQLEditor.swift` adalah
`NSTextView` dengan `NSScrollView`, syntax colouring lewat layout manager (`Views/SQLSyntax.swift`),
dan autocomplete. Pencarian untuk `findBar`, `showFind`, `replaceAll`, dan `performFindPanelAction`
di `app/Sources` **tidak menemukan apa pun**. Tidak ada folding. `Views/ResultGrid.swift` tidak
punya sort. Tidak ada cell viewer. `crates/qh-sql/src/scan.rs` sudah memindai batas statement dan
literal/komentar dengan benar, dan `crates/qh-sql/src/classify.rs` memakainya.

**Layak diambil sebagai pola.**

- **Find/replace:** pakai panel `NSTextView` bawaan atau panel sendiri, tetapi kompilasi kelima
  metode ke regex dengan escape yang benar; replace-all dalam satu undo group dan satu
  `beginEditing`/`endEditing`; ganti dari belakang ke depan.
- **Folding sebagai daftar offset, bukan mutasi teks.** Ini yang paling penting untuk QueryHive,
  yang memakai `NSTextView` asli: jangan melipat dengan menghapus teks (itu merusak undo, posisi
  caret, dan offset scanner). Fold harus jadi urusan **layout manager** atau overlay tampilan,
  seperti `NSTextAttachment`/hidden range, bukan `textStorage`.
- **Satu scanner untuk fold dan batas statement.** QueryHive sudah punya `qh-sql`; jangan menulis
  pemindai kedua di Swift dengan aturan yang berbeda. Kalau Swift butuh daftar region, minta dari
  engine.
- **Fold mulai di akhir baris pembuka**, dan buang region yang buka-tutup di baris yang sama.
- **Cap ukuran dokumen** untuk folding (TablePro: 2 juta karakter) supaya file `.sql` besar tidak
  membekukan editor.
- **Sort:** untuk grid berhalaman, sort di server dengan query ulang. Untuk hasil yang sudah dimuat
  penuh dan tidak punya halaman berikutnya, sort di memori dengan komparator yang sadar tipe.
  Pilihan "in-memory atas baris yang sudah diambil" yang diusulkan rencana harus menyebut dengan
  tegas bahwa itu **hanya** benar untuk hasil yang dimuat penuh, karena halaman pertama dari tabel
  yang tersortir bukan halaman pertama yang diurutkan ulang.
- **"Don't Sort" sebagai keadaan eksplisit ketiga**, bukan sekadar menghapus arah.
- **JSON viewer:** batasi input sebelum parse (TablePro 100.000 unit, 5.000 node), dan sediakan
  jalan keluar "terlalu besar, pakai mode teks". Jangan memotong nilai diam-diam di grid; tampilkan
  potongan dengan penanda.

**Out of bounds.** Badan `SQLFoldScanner`, `FindPanelViewModel`, `JSONTreeNode`/`JSONTreeParser`,
`TableRowsSorting`, dan tentu saja engine teks TableProEditor (`TableProTextEngine`,
`_RopeModule`, `RangeStore`). Model folding TablePro dirancang untuk editor rope-nya sendiri, jadi
menyalinnya justru salah untuk `NSTextView`.

**Rekomendasi Fase 4.** Kerjakan dalam urutan biaya: (4.3) sorting dulu, karena ia butuh keputusan
semantik (server vs memori) lebih dulu dan QueryHive belum punya pagination kedua; ambil yang
in-memory dengan catatan di header grid bahwa yang tersortir adalah baris yang sudah diambil, dan
tandai jalan menuju server-sort sebagai pekerjaan lanjutan. (4.2) folding sesudahnya, dengan
scanner dari `qh-sql` dan mekanisme layout-manager pada `NSTextView`, bukan mutasi teks. (4.1)
find/replace dengan panel `NSTextView` dan regex ter-escape. (4.4) JSON viewer dengan batas
100.000 karakter / 5.000 node seperti TablePro, dan tombol "buka di jendela" karena nilai JSON besar
perlu ruang.

## 7. Server MCP

Fase 2 sudah mendarat di QueryHive, jadi bagian ini bukan "apa yang hilang" melainkan "apa yang
layak dipertimbangkan ulang".

**Yang dibaca.** `TablePro/Core/MCP/`
(`MCPPairingService.swift`, `PairingTypes.swift`, `MCPTokenStore.swift`, `MCPAuthPolicy.swift`,
`Auth/MCPScope.swift`, `Lifecycle/{MCPServerManager,MCPHandshakeFile,MCPServerComposition}.swift`,
`Transport/{MCPBridgeLogger,MCPStdioMessageTransport}.swift`,
`Protocol/Tools/MCPToolRegistry.swift`, `Protocol/**` sebagian);
`TablePro/CLI/{BridgeMain,BridgeProxy,Handshake}.swift`; `project.yml` target `mcp-server`;
`docs/external-api/{versioning,mcp-clients,mcp-protocol,tokens}.mdx`;
`docs/features/safe-mode.mdx` (bagian External clients).

**Pairing.** Ini PKCE OAuth persis, bukan "ala PKCE". Klien mengirim `clientName`, `challenge`
(base64url SHA-256, selalu 43 karakter), `redirectURL`, plus scope dan allowlist koneksi yang
diminta. Redirect divalidasi ketat: hanya loopback HTTP (`127.0.0.1`, `localhost`, `::1`) atau
private-use scheme yang terdaftar; `about`, `blob`, `data`, `file`, `ftp`, `javascript`, `vbscript`
ditolak, begitu juga URL yang membawa kredensial. Pengguna menyetujui lewat sheet di app. Server
membuat token (`tp_` + 32 byte acak), menyimpan **SHA-256 bersalt** dan prefiks 8 karakter untuk
ditampilkan, lalu menaruh token plaintext di balik `code` sekali-pakai yang terikat ke challenge.
Code berlaku 5 menit, maksimum 50 pending, dan **satu percobaan** — verifier yang tidak cocok
membakar code-nya, karena kalau tidak, jendela lima menit itu menjadi tebak-tebakan offline.
Rate limit: 5 kegagalan dalam 5 menit → lockout 15 menit. Token sendiri punya masa berlaku
default 90 hari, scope, allowlist koneksi, `isActive`, `expiresAt`, `lastUsedAt`, dan pencabutan
yang langsung membatalkan request yang sedang jalan (`cancelInflight`) serta membersihkan approval.

**Scope.** Empat scope: `tools:read`, `tools:write`, `resources:read`, `admin`. Tiga tingkat token
memetakan ke himpunan scope. Tool `execute_query` masuk `writeQueryTools` dan butuh `tools:write`;
`confirm_destructive_operation` butuh `admin`. Token anonim (tanpa token terbit) hanya dapat
`tools:read`/`resources:read`. Koneksi dibatasi `ConnectionAccess.all` atau `.limited(ids)`, dan
`safe-mode.mdx` menyebut izin efektifnya `MIN(token.scope, connection.externalAccess)`, ditambah
Safe Mode per query. Tool agregat membaca koneksi lewat `readableConnectionIds`, sehingga koneksi
yang tidak boleh dilihat tidak bocor lewat daftar.

**Lifecycle helper.** Ada executable terpisah `tablepro-mcp` yang disematkan di
`Contents/MacOS/tablepro-mcp` (target `mcp-server`, tipe `tool`), bicara stdio. App menjalankan
server HTTP di `127.0.0.1` dan menulis `mcp-handshake.json` (`0600`, direktori `0700`, ditulis lewat
staging + rename atomik) berisi version, port, token, pid, instanceId, protocolVersion, expiresAt.
Bridge membacanya dengan pemeriksaan kepercayaan: pemilik berkas = uid sendiri, mode tidak world
readable, direktori tidak world writable, pid hidup, dan executable pid itu berada di dalam `.app`
yang sama (atau bernama TablePro). Kalau tidak ada handshake yang layak, bridge menjalankan
`open -g tablepro://integrations/start-mcp` dan mem-poll tiap 200 ms sampai 10 detik. App membuat
token bridge `__stdio_bridge__` (Read & Write, umur 1 jam, dirotasi 15 menit sebelum kedaluwarsa),
dan menghapusnya saat stop. Karena helper dan app proses terpisah yang hanya berbagi berkas
handshake, mematikan MCP tidak menjatuhkan app; app juga tidak pernah men-spawn MCP.

**Kebijakan stabilitas.** `docs/external-api/versioning.mdx` adalah dokumen kontrak eksplisit.
Dalam satu versi mayor, External API hanya **aditif**: tool, prompt, resource, dan kode error baru
boleh muncul; field input baru boleh muncul kalau opsional dan punya default; field output baru
boleh muncul. Yang dilarang: path URL dihapus atau berubah makna, tool atau prompt dihapus/diganti
nama, field input wajib ditambah ke tool lama, field output dihapus atau berubah tipe, resource
dihapus. Negotasi versi protokol mengembalikan daftar yang didukung, dan versi yang tidak didukung
ditolak dengan kode `-32022` yang membawa daftar itu di `data.supported`. Kode error milik TablePro
pindah dari blok `-32000` (dibekukan spesifikasi) ke `-33000` ke atas. Yang **tidak** masuk kontrak
disebut terang-terangan: framing transport, routing internal, path HTTP selain `/mcp`, berkas
handshake, penyimpanan token, audit log, dan `connections.json`.

**Yang sudah ada di QueryHive.** `crates/qh-ffi/src/mcp.rs` (protokol JSON-RPC 2.0 per baris,
registry tool, aturan scope, handshake), `crates/qh-ffi/src/bin/mcp.rs`, `crates/qh-ffi/tests/mcp.rs`
dan `tests/mcp_stdio.rs`, migrasi `0005` dengan tabel `mcp_token` (SHA-256 heksadesimal, scope,
allowlist koneksi), ADR `0015-mcp-token-scope.md`, sembilan tool read-only, penolakan `to_table`
yang menyebut mode `replace`-nya, handshake di `~/Library/Application Support/QueryHive/`, dan
bukti bahwa membunuh MCP tidak berdampak pada app.

**Yang layak dipertimbangkan ulang.**

1. **Prefiks token.** TablePro menyimpan prefiks 8 karakter di samping hash, sehingga daftar token
   bisa dikenali manusia tanpa membocorkan rahasia. QueryHive hanya menyimpan hash; menambah kolom
   prefiks adalah perubahan kecil yang membuat UI token jauh lebih berguna.
2. **Pencabutan harus membatalkan request yang sedang jalan.** TablePro memanggil
   `cancelInflight(matchingTokenId:)` dan membersihkan approval. QueryHive perlu memutuskan apa
   yang terjadi pada `preview` yang sedang berjalan ketika tokennya dicabut.
3. **Tiga kunci ala TablePro.** Scope token × level "External Clients" per koneksi × Safe Mode.
   QueryHive sekarang punya scope + allowlist + SafeMode. "External Clients" sebagai konsep
   terpisah dari Safe Mode (read-only untuk klien luar, tetapi penuh untuk pengguna) adalah lapisan
   yang hilang kalau nanti MCP dipakai tim, bukan hanya individu.
4. **Kebijakan stabilitas sebagai dokumen.** ADR-0015 memuat model token; belum ada yang memuat
   janji tool/resource. TablePro menaruh janji itu di docs dan menegakkan negosiasi versi protokol.
   Untuk QueryHive, satu halaman `docs/` yang menyatakan "tool dan nama tidak dihapus dalam satu
   mayor" sudah cukup, asal ditulis sebelum tool ke-10 mendarat.
5. **Handshake yang memverifikasi pembacanya.** Handshake QueryHive sekarang tidak memeriksa
   kepemilikan/pemilik proses. TablePro memeriksa pemilik berkas, mode, pid hidup, dan executable
   pid itu. Ini pertahanan yang murah dan layak disalin polanya (bukan kodenya).
6. **Rate limit pada autentikasi.** TablePro membatasi 5 kegagalan/60 detik dengan lockout
   5 menit. Token QueryHive panjang dan acak, jadi risikonya rendah, tetapi pembatasan itu juga
   mencegah klien salah konfigurasi membanjiri log.
7. **Pairing flow** (PKCE + sheet persetujuan) adalah bagian yang paling jelas belum ada. Untuk
   Fase 2 ia memang belum dibutuhkan (token diterbitkan lewat CLI). Kalau MCP menjadi fitur
   pengguna, pola PKCE TablePro adalah cetak birunya, dan `deny`-nya terhadap redirect non-loopback
   adalah bagian yang tidak boleh dilonggarkan.
8. **Alamat handshake** QueryHive sudah benar di Application Support. `to_table` sudah ditolak.
   Allowlist kosong = tidak ada koneksi sudah benar (fail closed).

**Out of bounds.** `MCPPairingService`, `PairingTypes`, `MCPTokenStore`, `MCPAuthPolicy`,
`MCPServerManager`, `MCPHandshakeFile`, kodec wire, dan seluruh implementasi tool. Bentuk protokol
JSON-RPC boleh dipelajari dari spesifikasi MCP, bukan dari kode TablePro.

**Rekomendasi Fase 2 (lanjutan).** Tiga hal, dalam urutan nilai: (a) tambahkan kolom prefiks token
dan uji pencabutan saat request berjalan; (b) tulis halaman stabilitas tool/resource dan negosiasi
versi protokol yang mengembalikan daftar yang didukung; (c) putuskan apakah "External Clients"
perlu menjadi konsep terpisah dari Safe Mode sebelum MCP dipakai lebih dari satu orang. Uji dengan
klien MCP pihak ketiga tetap celah yang sudah tercatat di rencana.

## 8. Yang dikoreksi dari analisis awal

Analisis sebelumnya mengambil semuanya dari dokumentasi, `CLAUDE.md`, `README.md`, dan
`project.yml`. Berikut tempat sumber menyetujui, mempertajam, atau membantahnya.

**Bantahan.**

- **Jumlah plugin.** Analisis §3 menulis "Inventaris plugin di `Plugins/` berisi 42 entri: 41 plugin
  plus `TableProPluginKit`". Sumber: `Plugins/` berisi 42 direktori, tetapi satu di antaranya adalah
  target tes (`DamengDriverPluginTests`), dan `TablePro/CLAUDE.md` menyebut "the 40 plugin bundles".
  Jadi yang benar **40 plugin bundle plus TableProPluginKit**. Hitungan itu juga konsisten dengan
  isinya: 27 plugin driver plus 13 plugin format (9 ekspor, 4 impor).
- **Sorting grid.** Analisis §4.3 merekomendasikan "sort di memori atas baris yang sudah diambil,
  dan tulis di header grid bahwa yang tersortir adalah baris yang sudah diambil". Itu benar sebagai
  langkah murah, tetapi bukan yang TablePro lakukan: `docs/features/data-grid.mdx` dan
  `TablePro/Models/Query/TableRowsSorting.swift` menunjukkan TablePro menyortir **di server**
  dengan query ulang untuk tabel berhalaman, dan hanya menyortir di memori untuk hasil yang sudah
  dimuat penuh. Klaim "biaya kecil" tetap benar; klaim bahwa itu cukup untuk semua kasus tidak.
  QueryHive akan salah kalau menyortir di memori saat nanti ada pagination/`hasMoreRows`.
- **Premis Fase 3 sudah basi.** Rencana §5 dan invariant #7 menyatakan QueryHive "tidak punya query
  timeout sama sekali". Pohon ini sekarang punya `statement_timeout` di `ExecuteOptions` dan bit
  `Capabilities::statement_timeout` (`crates/qh-driver/src/lib.rs:147` dan `:177`), plus Safe Mode
  tiga tingkat di `crates/qh-sql/src/classify.rs` yang ditegakkan engine. Dokumen yang membacanya
  tanpa memeriksa ulang akan menyimpulkan celah yang sudah tertutup.

**Pertajaman.**

- **Safe Mode.** Analisis bilang "per koneksi, dari tanpa batas sampai read-only penuh". Sumber:
  enam tingkat dengan tiga perilaku terpisah (blokir/konfirmasi/autentikasi), plus konsep **floor**
  dari empat kondisi independen yang di-`max` by strictness, plus execution log ber-hash-chain.
  None of that is a port target, but the shape is worth knowing.
- **MCP pairing.** Analisis bilang "alur ala PKCE". Sumber: PKCE RFC 7636 yang sesungguhnya, dengan
  validasi redirect loopback/private-use, code sekali-pakai yang dibakar oleh verifier yang salah,
  jendela 5 menit, dan rate limit. Kalau QueryHive membangun pairing, ini yang perlu ditiru.
- **Change tracking.** Analisis §3 mendeskripsikannya (dari docs) sebagai "antre edit sel, insert
  baris, dan delete baris, tinjau SQL-nya, baru simpan". Sumber membenarkannya, dan menambah tiga
  hal yang tidak terlihat dari docs: cap urutan terpisah, verifikasi jumlah baris di dalam
  transaksi, dan kosakata partial-commit. Ketiganya adalah bagian yang paling berguna untuk Fase 5.3.
- **Parquet.** Analisis §3 dan matriks §5 sudah benar bahwa Parquet ada dan layak diambil. Yang baru
  dari sumber: plugin itu registry-only **karena** ia meng-link `libduckdb.a` sendiri, dan DuckDB
  itulah encoder-nya. Untuk QueryHive, itu berarti crate `parquet`/`arrow` native, bukan libduckdb.
- **Ekspor streaming.** Analisis §4 memuji QueryHive yang "tidak pernah menahan result set di
  memori". Pemeriksaan sumber menunjukkan TablePro **juga** tidak, dengan cara yang berbeda per
  plugin: CSV mmap + rentang baris, XLSX writer inkremental, Parquet staging DuckDB yang bisa spill.
  Jadi prinsip QueryHive bukan keunggulan unik; yang unik adalah part splitting dan retry-nya.
- **Impor streaming.** Rencana §7 menuntut "jangan pernah menahan file di memori" untuk impor.
  Sumber menunjukkan TablePro tidak selalu memenuhi standar itu: XLSX dibaca utuh karena formatnya
  memaksa, dan CSV mmap seluruh berkas. QueryHive justru bisa lebih ketat untuk CSV. Untuk XLSX,
  janji "streaming" tidak jujur dan tidak boleh ditulis.

## 9. Ringkasan untuk sesi berikutnya

Kalau hanya empat temuan yang dibaca dari dokumen ini, ini keempatnya.

1. **Fase 5.3:** bangun `plan` → `review` → `apply` sebagai tiga langkah dengan satu objek rencana di
   engine, verifikasi jumlah baris di dalam transaksi, dan kosakata partial-commit. Insert dan
   delete masuk sebagai jenis perubahan pada antrean yang sama. Antrean adalah nilai Rust, bukan
   tambahan di Swift.
2. **Fase 5.1:** pisahkan impor baris dari impor statement; runner bersama yang memiliki transaksi,
   tiga mode kesalahan, dan nomor baris di pesan. `skipAndContinue` tidak memakai transaksi. Baris
   yang tidak termapping ditolak.
3. **Fase 3:** sebagian besar sudah mendarat; yang tersisa adalah memastikan tiap driver mengisi
   `Capabilities::statement_timeout` dengan jujur dan menerjemahkan `ExecuteOptions` ke mekanisme
   servernya, lalu mengujinya sebagai timeout bertipe. Jangan menambahkan tingkat Safe Mode baru
   sebelum ada alasan produk.
4. **Fase 2:** token prefiks, pencabutan yang membatalkan request berjalan, halaman stabilitas
   tool/resource, dan pikirkan "External Clients" sebagai lapisan terpisah dari Safe Mode.

## 10. Yang tidak selesai dibaca

Supaya klaim di atas bisa ditelusuri dan tidak dibaca sebagai lebih luas dari yang diperiksa:

- **40 plugin driver dan format, kecuali yang disebut di atas.** Saya membaca
  `CSVImportPlugin`, `JSONImportPlugin`, `SQLImportPlugin`, `XLSXImportPlugin`,
  `ParquetExportPlugin`, `XLSXExportPlugin`, dan potongan timeout dari `MySQLDriverPlugin` dan
  `PostgreSQLDriverPlugin` (Postgres hanya `applyQueryTimeout`; sisanya tidak).
- **Engine teks TableProEditor di dalamnya.** `TableProTextEngine`, `_RopeModule`, `RangeStore`
  (penyimpanan fold), highlighter, klien tree-sitter, bahasa grammar, dan mode Vim. Saya hanya
  membaca model folding dan view model find/replace.
- **Sebagian besar `TablePro/Core/MCP/`.** Implementasi tiap tool di `Protocol/Tools/` hanya saya
  baca daftar namanya di registry; `Prompts/`, `Elicitation/`, `Completions/`, `Subscriptions/`,
  `Meta/`, `Results/`, `Outside/` (MCP server di luar yang dijalankan pengguna), isi wire codec
  (`Wire/`), dan skema `MCPAuditLogStorage` tidak dibaca.
- **Struktur editor, routines/triggers, user-defined types, compare & sync, backup & restore,
  object copy, server dashboard, users & roles, ER diagram, chart/map.** Tidak dibaca; itu area
  yang ditolak atau ditunda oleh rencana.
- **AI assistant dan Agent mode** (di luar perannya sebagai pemicu floor Safe Mode), **iCloud/CloudKit
  sync**, aplikasi **iOS**, dan `TableProMobile/`.
- **Semua tes** hanya dipakai sebagai penunjuk keberadaan perilaku; tidak ada yang dibaca untuk
  invariannya.
- **Sebagian besar `docs/`.** Saya membaca `change-tracking`, `safe-mode`, `code-folding`,
  `json-viewer`, `data-grid` (bagian sort), `mcp-clients`, dan `versioning`; sisanya tidak.
- **`scripts/`** (termasuk `check-mysql-query-timeout.sh`, `check-redis-command-routing.sh`, dan
  `check-plinkit-abi.sh`) tidak dibaca; jebakan di dalamnya bisa saja relevan untuk Fase 3 dan 5.
