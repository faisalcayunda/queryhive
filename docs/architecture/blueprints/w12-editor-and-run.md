# Blueprint W12: editor dan eksekusi (formatter, skrip, impor JSON, notifikasi)

- **Status:** blueprint tingkat berkas, 6 Okt 2026 (W12-A1). Belum ada kode yang ditulis. Diperiksa `architect-reviewer` 6 Okt 2026 (satu putaran, O-20): **koreksi blokir B1 sampai B4 sudah ditulis ke badan dokumen** (ditandai "Koreksi AR") dan menunggu pemeriksaan ulang (pending review). B4 menunggu jawaban pemilik sebelum W12-T4 dibuka. Temuan tidak memblokir dicatat di bagian Verdict dan belum ditulis ke badan.
- **Untuk:** W12-T1 sampai W12-T6 (FR-ED-01…04, FR-RUN-01…05, FR-IMP-01, 02, PR-16; UC-03, UC-06, UC-11, UC-13, UC-16). Pemeriksa menurut `development-plan.md` §5 W12: T1 RR, DB, CR; T2 SR, UX, AX; T3 RR, DB, SF, SEC, CR; T4 SR, UX, AX, SF, AR; T5 RR, DB, SR, SF; T6 SR, AX, UX. T3 dan bagian Safe Mode §4.3 masuk tingkat risiko **tinggi** (O-17): Safe Mode, batas FFI, umur handle.
- **Sumber:** PRD §5.3, §5.4, §5.8, §6.4, §7 (PR-16), P-05, P-06, P-10, P-11, P-13, P-16, P-19, P-31, O-8, O-12, O-14; `development-plan.md` §4, §5 W12, §7; `blueprints/fase-4b-editor-analysis.md` D-13, D-21, §8.4; `blueprints/fase-6-data-plane.md` §12, §17.4, §18, §19; `docs/invariants.md` #8, #9, #11; ADR-0017, 0019, 0022, 0026, 0027, 0031; `tablepro-feature-map.md` §0 butir 1, §1.3, §1.4, §1.14; `tablepro-design-audit.md` §12; `AGENTS.md`.
- **Prasyarat:** W11 selesai menurut rencana. Blueprint ini ditulis lebih awal di jalur DOCS (ledger O-22), selagi W6-T1 sedang dikerjakan, jadi fakta kode dibaca di `2d14eea` ditambah pohon kerja W6-T1 yang belum di-commit (`QueryTab.activeResult`, `ResultSlot`, `StoreRows.swift` baru). Yang W9 sampai W11 akan ubah ditandai **(rencana)** dan **harus dibaca ulang dengan grep** sebagai langkah pertama setiap tugas (aturan di §11). Nomor baris Swift bergeser karena pohon kerja berubah; fungsi dan tipe yang disebut adalah acuannya.
- **Bukti:** kode di pohon ini, dikutip dengan berkas dan nomor baris. Kode TablePro (AGPL) hanya dibaca untuk ide: pemilih result set yang hilang bila hanya ada satu hasil, izin notifikasi yang diminta saat notifikasi pertama benar-benar akan tampil, dan seam protokol di depan pusat notifikasi supaya aturannya bisa diuji. Tidak ada kode, aset, atau string yang disalin. Tidak ada perintah `cargo`, `swift`, build, atau bench yang dijalankan untuk dokumen ini.
- **Bahasa:** dokumen ini Indonesia. Identifier, komentar kode, pesan log, string yang tampil di app, dan nama tes Inggris.

## Ringkasan

W12 mengisi lima hal yang hari ini hanya berupa label atau tidak ada: **Format SQL** dan toggle komentar, **tab terikat berkas `.sql`**, **Run Script** dengan hasil per statement, **impor `.sql` dan JSON dari app**, dan **notifikasi serta progres** untuk operasi panjang. Tiga hal paling menentukan rancangannya:

1. **"Run Script" hari ini adalah ekspor, dan tidak ada skrip.** Item menu itu memanggil `run(tab, from: .all)` (`App.swift`, `CommandMenu("Query")`), yaitu jalur `export`/`to_table`; komentar di `Workspace.swift` (di atas `actionButton`) menyatakan "this engine runs one statement per run, so there is no script to continue". Ketiga driver menjawab `multiple_result_sets: false` dan PostgreSQL menolak teks multi-statement (invariant #8). `import_data` untuk `.sql` (`import.rs:466-605`) bukan runner: ia membuang baris hasil, memakai transaksi sebagai default, dan hanya melaporkan nomor baris galat. Runner butuh perintah baru, `script` (P-11), di sesi tunggal yang hidup sepanjang skrip.
2. **Satu sesi untuk seluruh skrip menutup dua jalan pintas.** Memanggil `preview` per statement dari Swift ditolak: kolam mereset sesi setiap Run (O-6), jadi `CREATE TEMP TABLE` lalu `SELECT` dari tabel itu tidak mungkin, dan satu konfirmasi, satu entri history, serta satu Stop tidak bisa menutupi 40 panggilan. Menghentikan pembacaan di batas `LIMIT` (cara `preview`) juga ditolak: komentar `commands.rs:1763-1768` mencatat bahwa cancel yang datang telat mengenai statement berikutnya dari sesi yang dipakai ulang, dan di skrip sesi itu **pasti** dipakai ulang. Statement berbaris karena itu dibaca **sampai habis**, `LIMIT` baris pertama disimpan, dan sisanya dihitung (D-11).
3. **Store dan callback yang ada bentuknya satu-satu.** `run_with_store` hanya menerima `preview` dan `explain` dan satu store per Run (`host.rs:515`, `store_api.rs:307`), dan satu-satunya callback adalah `on_event(line: String)` (`uniffi_api.rs:219-222`). Skrip menghasilkan N store di tengah Run, jadi ia butuh jalur baru: `EngineHost::run_script` dengan penerima hasil terpisah (D-12), tanpa menyentuh kontrak `EventSink` yang dipakai semua perintah lain.

Dua kehati-hatian yang berulang di seluruh dokumen. **Formatter dan toggle komentar tidak boleh mengubah arti SQL**, jadi keduanya dibatasi oleh invarian byte yang diperiksa saat runtime (§3.1), dan keduanya mengikuti *semua* pembacaan leksikal yang dipakai penjaga Safe Mode, bukan satu pembacaan seperti editor (F-2). **Konfirmasi Safe Mode untuk skrip bersumber dari engine**, bukan dari tiruan Swift (`StatementScan`) yang tidak sadar dialek: engine memancarkan `plan`, app menggambar lembar konfirmasi dari situ, dan konfirmasi terikat pada digest teks skrip (§4.3).

## 1. Fakta yang diperiksa di kode

### 1.1 Mesin pemindai dan editor

- **F-1. `walk` satu kali jalan, tidak bisa dilanjutkan, dan hanya melaporkan sebagian.** `qh_sql::walk` (`scan.rs:448-703`) memanggil `Visitor::separator`, `opaque`, dan `word`. Ia tidak melaporkan spasi, angka, tanda baca, atau parameter. `qh_sql::lex` (`lex.rs:50-146`) menambah `Number`, `Param`, dan `Punct` di atasnya, tetapi **kehilangan byte**: spasi, byte kontrol, byte ≥ 0x80 yang bukan bagian kata (di semua lexer selain PostgreSQL, `scan.rs:147-150`), dan angka yang menempel pada huruf (`1abc`, `lex.rs:95-105`) tidak menjadi token. Konsekuensinya untuk formatter: teks **tidak bisa dirakit ulang dari token**. Formatter memegang byte asli dan hanya mengganti isi celah spasi di antara atom (§3.2). `AGENTS.md` menyebut keputusan terbuka soal `walk` yang bisa dilanjutkan; teksnya tidak saya temukan di `target/run/ledger.md`, jadi blueprint ini tidak bergantung padanya: formatter dan skrip memanggil `walk` atas teks utuh (dokumen, atau statement terpilih), dan skrip memecah satu kali.
- **F-2. Editor membaca dengan satu lexer, penjaga dengan semua.** `Analyzer::new` dan `classify` memakai `Lexer::from(dialect)` (`qh-editor/src/paint.rs:278`, `classify.rs:67`), yaitu pembacaan bawaan. Engine memakai `Dialect::readings()` (`scan.rs:237-250`): Generic 1, MySQL 6, PostgreSQL 4, Trino 1, dan gagal tertutup bila pembacaan berselisih tentang letak statement (`decisions_readings`, `classify.rs:691-730`; `statements_agreeing`, `:759`). Spasi di dalam string yang dibaca MySQL sebagai kode (`'\'` di bawah `NO_BACKSLASH_ESCAPES`) bernilai data; formatter yang menyetujui satu pembacaan akan mengubah data di pembacaan lain. Formatter dan toggle komentar karena itu mengikuti penjaga (D-1).
- **F-3. `crates/qh-sql/src/editor/` tidak ada lagi.** `ls crates/qh-sql/src` hanya berisi `classify.rs`, `ident.rs`, `lex.rs`, `lib.rs`, `scan.rs`, `wrap.rs`. Analisis editor hidup di `crates/qh-editor` (`statements.rs`, `paint.rs`, `keywords.rs`, `classify.rs`, `issues.rs`, `folds.rs`, `text.rs`, `view.rs`), dan statement dipecah oleh `walk` di sana (`qh-editor/src/statements.rs:12,72`). Baris W11-T6 di `development-plan.md` ("menggantikan `crates/qh-sql/src/editor/`") usang; W12 tidak merujuknya.
- **F-4. Daftar kata kunci editor tidak boleh masuk `qh-sql`.** `qh_editor::keywords::is_keyword` (`keywords.rs:52`) diturunkan dari grammar tree-sitter; D-21 blueprint 4B dan P-31 melarang `qh-sql` bergantung padanya. Formatter memakai daftar kata klausa miliknya sendiri, dan kasus kata kunci (FR-ED-01 "mengikuti setelan keyword case") dikerjakan di `qh-ffi/src/editor.rs`, tempat kedua crate bertemu (D-4).
- **F-5. Pemecah skrip tidak punya offset.** `ScriptStatement { line, text }` (`classify.rs:778-786`) memberi baris awal (1-based, baris byte signifikan pertama) dan teks yang sudah di-`trim`, tanpa `;` penutup dan tanpa offset. Event skrip karena itu memakai **nomor baris**, bukan offset (§4.2); app memetakan baris ke rentang lewat `LineIndex` yang sudah ada (`SQLEditor.swift`, `struct LineIndex`).
- **F-6. Pemecah Swift mengirim `.generic`.** `sqlStatements(in:)` memanggil `sqlStatementRanges(sql:dialect: .generic)` (`QueryTab.swift`, `func sqlStatements`); dialek per tab baru datang dengan W10-T6 **(rencana)**. Selama itu Format memakai gabungan semua pembacaan untuk `Generic` (D-1), bukan satu pembacaan.

### 1.2 Engine

- **F-7. Runner statement yang ada bukan runner skrip.** `import_statements` (`import.rs:466-605`) memecah dengan `statements_with_lines_dialect`, melakukan pemeriksaan pintu `guard(safe, &text, dialect)` (tanpa konfirmasi, jadi `confirm` menolak, ADR-0026), lalu menjalankan tiap potongan lewat `run()` (`import.rs:889-905`) yang **menguras kursor dan membuang baris** (`while cursor.next_batch(1_000)…`). Mode `stop`/`commit`/`skip` dan transaksi (`BEGIN`…`COMMIT` bila `capabilities().transactions`) adalah kebijakan impor (ADR-0019, 0022), bukan kebijakan skrip. Event `done` memuat `statements`, tidak memuat hasil per statement.
- **F-8. Rute dan store.** `route()` adalah `match` tanpa wildcard (`host.rs:224-249`): perintah baru gagal dikompilasi sampai memilih rute. `Export`, `ToTable`, `ImportData` adalah `LongOp` (sesi sendiri, tidak dikolam, berbagi `Arc<Tunnel>`, P-05); `Preview`/`Explain`/`Count`/`ApplyChanges`/`TableOp` dikolam dan direset tiap Run (O-6). `run_with_store` hanya menerima `Preview` dan `Explain` (`host.rs:515`), dan satu store menampung satu Run (`claim_run`, `store_api.rs:307-309`). `Emitter::result_store()` mengembalikan satu `StoreWriter` (`events.rs:31-43`, `StoreEmitter` `:47-71`). `EventSink` punya satu metode, `on_event(&self, line: String)` (`uniffi_api.rs:219-222`).
- **F-9. Mesin baca yang bisa dipakai ulang ada di `commands.rs`, tetapi privat.** Sebelas nama modul-privat yang dibutuhkan `script.rs`: `RowTarget` (`:1798-1822`), `StoreTarget` (`:1913-2018`), `STORE_FETCH_MAX` (`:1905`), `pump_result` (`:2062-2087`), `until_stopped` dan `execute_until_stopped` (`:1401-1437`), `stop_grace` (`:1444`), `stop_session` (`:1468-1481`), `STOP_BUDGET` (`:1395`), `STOP_UNCONFIRMED` (`:1453`), dan `record_decision` (`:255-290`, yang mencatat satu keputusan ke `execution_log`; `guard_confirmed` berhenti di galat pertama, sedangkan skrip harus mencatat semua keputusan). Perlu visibilitas `pub(crate)`: perubahan `commands.rs` yang tidak ada di rantai kepemilikan §7 (§10).
- **F-10. Kursor yang dihentikan di batas cap meninggalkan respons tertunda.** Kursor PostgreSQL yang mencapai `row_limit` berhenti membaca (`qh-driver-postgres/src/lib.rs:802-806`) dan aliran `simple_query_raw` masih membawa sisa baris; `stop_session` mendokumentasikan bahwa tugas koneksi akan terus menguras respons itu (`commands.rs:1459-1466`), dan `stream_rows` menyatakan alasan tidak ada cancel pada sesi kolam: "a cancel that lands late could hit the next statement of a session that is about to be reused" (`:1763-1768`). Perilaku menguras itu disimpulkan dari komentar tersebut, tidak dijalankan di sini (§13.1).
- **F-11. Retry.** `retry::execute` membungkus bacaan; perintah yang menulis sengaja tidak memakainya (`lib.rs:50-57`), dan impor memanggil `session.execute` langsung. `EngineError::Query { kind: FailureKind::Transient }` berarti koneksi putus saat statement berjalan (`qh-core/src/error.rs:19-31,58-71`): bagi penulisan, hasilnya **tidak diketahui**, bukan gagal.
- **F-12. Safe Mode.** `guard` (tanpa konfirmasi) dipakai jalur massal; perintah satu statement memakai `guard_for`, yang menghormati `SAFE_MODE_CONFIRMED` (`commands.rs:216-218`, `:297-299`). `confirm` menolak DDL dan statement tak terklasifikasi walau dikonfirmasi (ADR-0026). `SafeModeError::NeedsConfirmation` hanya menyebut statement **pertama** (`classify.rs:383-396`, `:418-437`). Lembar konfirmasi di app memakai tiruan Swift `StatementScan` (`RunConfirmation.swift`, `enum StatementScan`) yang membaca komentar `--` dan `/* */` saja: tanpa `#` MySQL, tanpa `E'…'`, tanpa dialek. Tiruan itu mengaku "bukan otoritas".
- **F-13. Digest sudah ada.** `qh_storage::hash_statement(sql) -> String` (SHA-256 heks, `qh-storage/src/execution_log.rs:77`, diekspor `lib.rs:42`) dan `qh-ffi` sudah bergantung pada `qh-storage`. Tidak perlu dependensi baru untuk digest §4.3.
- **F-14. Sesi aplikasi.** `SessionTab` adalah JSON milik app yang tidak ditafsirkan engine ("this command does not [interpret it]", `local.rs:404-451`; `Session.swift`, `struct SessionTab`), dan kuncinya tanpa garis bawah supaya `convertFromSnakeCase` tidak mengubahnya (`Session.swift` komentar kepala).

### 1.3 Sisi Swift hari ini

- **F-15. Run, Run Script, dan pintasan.** Run = `preview(tab)`; "Run the Whole Editor" = `preview(tab, from: .all)` (`Workspace.swift`, menu chevron); "Run Script" = `run($0, from: .all)` = ekspor (`App.swift`; `AppModel.swift`, `func run`). `ShortcutAction.runScript`, `.saveFile`, `.commentLine`, `.format` punya judul dan ikatan di kedua skema (`Shortcuts.swift:5-40,121,133-135,146,152`) tetapi **tidak ada pemanggil** selain daftar di Settings; skema `queryhive` tidak mengikat `.runScript` ("⌘⇧R is already Reveal in Finder") maupun `.format`.
- **F-16. Berkas.** Satu-satunya I/O berkas editor adalah `QueryTab.loadSQLFromFile`/`loadSQL(from:)` (UTF-8, tanpa ikatan, tanpa simpan; **menggantikan SQL tab aktif tanpa bertanya**) dan `AppModel.runSQLFile` (`AppModel.swift:1376`); `sqlContentTypes` hanya `sql` dan `txt` (`:24`). App tidak ber-sandbox (`QueryHive.entitlements` hanya berisi dua kunci hardened runtime), jadi jalur biasa cukup, tanpa bookmark.
- **F-17. Hasil.** Pohon kerja W6-T1 sudah punya `QueryTab.activeResult: StoreRows?`, `baseResult: ResultSlot?` (`struct ResultSlot { meta: PreviewResult; rows: StoreRows }`), `fetchedRows`, `viewBusy`, `viewError`, dan `result` yang mengembalikan `activeResult ?? noRows`. `sourceTable` hanya diisi jalur yang menulis SQL-nya sendiri (buka tabel dari pohon): hasil skrip tidak pernah memilikinya, jadi **tidak bisa diedit** dan tombol commit tidak muncul. `totalRows` ada bila pengguna meminta count.
- **F-18. `Event` adalah satu struct raksasa** (`App.swift:156`, `struct Event: Decodable`) yang menerjemahkan semua perintah dengan kunci opsional dan `convertFromSnakeCase`; kunci yang sama dengan tipe lain menggagalkan dekode seluruh event (dibuang diam-diam oleh `EngineWire.event(in:)`). Event skrip karena itu dapat tipe sendiri (D-10).
- **F-19. Sheet impor.** `ImportSourceFormat` hanya `csv`, `tsv`, `xlsx` (`ImportMapping.swift:35-74`); header dibaca **di Swift** oleh `ImportHeaderReader` (`:240-313`) sedangkan XLSX tanpa daftar field karena "a second reader in Swift would be a second answer" (`:57-63`); `ImportSheet.swift:283-292` menulis bahwa `.sql` "is not offered here"; `chooseFile` hanya menerima csv/tsv/xlsx (`:407-416`); `COLUMNS` adalah `[{source, target, include}]` (`ImportMapping.swift:138-150`, `import.rs:998-1030`).
- **F-20. Notifikasi, Dock, dan NSProgress belum ada.** Tidak ada `UNUserNotificationCenter`, `dockTile`, atau `NSProgress` di `app/Sources`. Pengumuman VoiceOver sudah punya pola: `announceSelectionForAX` memposting `.announcementRequested` ke `NSApp` (`ResultGridTable.swift:939-950`). Bundle id `id.data-ecosystem.queryhive`, ditandatangani ad-hoc (`app/build.sh:83,141`). `AppModel.swift` masih satu berkas 3.444 baris: `AppModel+*.swift` (W9-T0) belum ada **(rencana)**.

### 1.4 Impor dan ekspor

- **F-21. NULL dan kosong tidak terbedakan di `RawRow`.** `RawRow { line, cells: Vec<String> }` (`qh-import/src/lib.rs:82-85`) dan `NULL_TEXT` bawaan `""` (`import.rs:306`, `row_is_empty` `:1053`, `literal` `:1088`): sel kosong CSV adalah NULL. JSON punya `null`, `""`, dan kunci yang hilang sebagai tiga hal berbeda.
- **F-22. `qh-import` tidak punya JSON.** `Format` hanya `Csv` dan `Xlsx` (`lib.rs:44-47`), `RowReader` enum (`:133-173`), `Cargo.toml` tanpa `serde_json`. `serde_json` di workspace hanya berfitur `preserve_order` (`Cargo.toml:58`), versi 1.0.151 di `Cargo.lock`, dan fitur `raw_value` ada di crate itu. Tanpa `arbitrary_precision`, `serde_json::Number` menyimpan bilangan non-bulat sebagai `f64` (dari dokumentasi crate; tidak dijalankan di sini), sehingga `0.1234567890123456789` atau bilangan 30 digit rusak diam-diam bila dibaca sebagai `Value`.
- **F-23. Ekspor memindah nama berkas pertama.** Saat bagian kedua dibuka, `report.csv` diubah menjadi `report_part01.csv` (`qh-export/src/plan.rs:18-26,57-67`), dan event ekspor tidak membawa path sampai `done.files` (`commands.rs:893-972`). `progress` hanya membawa `rows`. `NSProgress` yang diterbitkan untuk `report.csv` akan menggantung setelah rename.

### 1.5 Ide dari TablePro (dibaca, tidak disalin)

- **Pemilih hasil** (`Views/Results/ResultSetMenu.swift`, `Models/Query/ResultSetMenuModel.swift`): strip tab 32 pt per tab query diganti pull-down yang **tidak ada bila hanya satu hasil** ("the strip's worst habit: a lone tab spending a band of height to say 'this is the result'"), berjudul "Result 2 of 4", dengan Pin, Close, dan Close Others; hasil yang di-pin tidak ikut Close Others. Dipakai sebagai perilaku, bukan tampilan (UX yang memutuskan).
- **Asal-usul tiap hasil** (`QueryExecutionCoordinator+MultiStatement.swift`, `statementOrigin`): hasil skrip tidak mewarisi kunci tabel hasil sebelumnya karena `UPDATE` bisa mencocokkan kunci tabel lain. QueryHive sudah aman: `sourceTable` kosong (F-17).
- **Notifikasi** (`Core/Services/Notifications/`): izin diminta saat notifikasi pertama benar-benar akan tampil, satu peminta untuk seluruh app, dan semuanya lewat protokol supaya tes tidak bergantung pada pusat notifikasi nyata (yang tanpa otorisasi di CI membuang semuanya diam-diam, sehingga tes yang ditulis terhadapnya lulus walau kodenya rusak).
- **Formatter** (`Core/Services/Formatting/SQLFormatterService.swift`): token-based dengan konteks klausa dan kedalaman, batas masukan 10 MiB, dan pemetaan kursor dengan rasio panjang. Rasio itu tidak dipakai di sini: invarian byte memberi pemetaan eksak (§3.4).

## 2. Keputusan desain

| ID | Keputusan | Alasan |
|---|---|---|
| D-1 | **Formatter dan toggle komentar mempertahankan seluruh byte non-spasi dan menolak daripada menebak.** Pembacaan yang dipakai adalah **semua** `dialect.readings()`; untuk `Generic` gabungan keempat keluarga (12 pembacaan). Bila region opaque berselisih antar pembacaan, atau `walk` berakhir di dalam region terbuka **selain komentar baris**, hasilnya galat bernama (`Ambiguous`, `Unterminated`) dan teks tidak berubah. **Koreksi AR (B1):** `EndState::Open(LineComment)` bukan galat. `walk` melaporkannya untuk komentar baris yang tidak punya baris baru sesudahnya (`scan.rs:418-426`, `:695-701`), jadi dokumen atau irisan statement yang baris terakhirnya `-- note` adalah SQL sah dan diformat seperti biasa. Hasil diperiksa sendiri sebelum dikembalikan (§3.1). | F-1, F-2, F-6. Spasi di dalam string yang dibaca server sebagai kode adalah data. Selaras dengan §8.4 blueprint 4B (pemeriksaan sendiri, tolak bila kutip terbuka), diperkuat ke tingkat byte dan semua pembacaan. Biayanya: SQL MySQL ber-`\'` dan PostgreSQL ber-backslash di string biasa tidak bisa diformat sampai kutipnya ditulis `''`, persis pesan yang sudah diberikan penjaga (`ambiguous_reason!`). |
| D-2 | **Kisi spasi.** Celah tidak-kosong tidak pernah menjadi kosong dan celah kosong tetap kosong. Pengecualiannya hanya karena `,` `;` `(` `)` tidak bisa membentuk operator multi-karakter: spasi sebelum `,` `;` `)` dan sesudah `(` boleh dihapus, dan sesudah `,` dan `;` boleh disisipkan pemisah. Spasi sebelum `(` dan sesudah `)` tidak pernah dihapus atau disisipkan. Di antara dua literal string bersebelahan dan di zona lanjutan string, celah dipertahankan apa adanya. Setelah komentar baris selalu ada pemisah baris. Tidak ada normalisasi spasi operator. | `$1` (`$` lalu angka), `::`, `->>`, `a:b`, dan lanjutan string PostgreSQL (`'a'` baris baru `'b'`) berubah arti bila spasi disisipkan atau dihapus. Hanya empat karakter struktural yang tidak bisa membentuk operator multi-karakter. Spasi di depan `(` tetap karena `count (*)` dan `count(*)` berbeda arti di MySQL tanpa `IGNORE_SPACE` (dokumentasi MySQL; tidak dijalankan di sini). Harganya: `a=b` tetap `a=b`. Gaya di dalam baris diputuskan penulis, di antara baris oleh formatter. |
| D-3 | **Tata letak dari aliran token dengan daftar kata klausa sendiri; tanpa pohon, tanpa pembungkusan lebar.** Bingkai kurung menentukan apakah kata klausa menyalakan baris baru (hanya bingkai subquery). | P-10, D-13 blueprint 4B. Pohon ber-ERROR pada 14–47% SQL valid (P-28); pembungkusan lebar membuat hasil bergantung pada tata letak sebelumnya dan merusak idempotensi. |
| D-4 | **Kasus kata kunci bukan bagian formatter.** `qh-ffi/src/editor.rs` menjalankannya sesudah format bila `EditorPreferences.autoUppercaseKeywords` menyala, hanya pada atom `Word` yang `qh_editor::keywords::is_keyword`, dan hanya mengubah huruf kecil ASCII menjadi besar. Invarian terpisah (I-K1): sama persis kecuali kasus huruf pada kata itu. | FR-ED-01 menuntut "token identik" sekaligus "mengikuti keyword case"; keduanya hanya konsisten bila kasus adalah langkah sendiri. Setelannya Bool (`EditorPreferences.swift`, `autoUppercaseKeywords`, bawaan mati), jadi hanya dua nilai: biarkan, atau besar. F-4. |
| D-5 | **Toggle komentar di Rust (`qh-sql/src/comment.rs`)**, satu rentang pengganti, menolak baris yang **mulai atau berakhir** di dalam string, identifier berkutip, komentar blok, atau dollar-quote. **Koreksi AR (B1):** "di dalam" berarti benar-benar di dalam (§3.3): posisi di tepi region tidak dihitung, dan komentar baris tidak pernah menolak. | Awalan `-- ` di dalam string mengubah data; menutup baris yang membuka string mengubah letak string. Memakai pembacaan yang sama dengan D-1 (F-2). Menambah satu berkas ke kepemilikan T1 (§10). |
| D-6 | **Format bekerja atas statement utuh:** seleksi tak kosong memformat statement yang bersinggungan dengannya, tanpa seleksi memformat dokumen. Hasil berupa penggantian terkecil (awalan dan akhiran sama dipangkas), satu langkah undo, kursor dan seleksi dipetakan dengan hitungan karakter non-spasi. Tidak berjalan selagi `hasMarkedText()`. Di atas 100.000 unit UTF-16 dihitung di antrean latar dan diterapkan hanya bila revisi `EditorDocument` belum berubah. | Invarian byte membuat hitungan non-spasi sebelum kursor sama di kedua teks, jadi pemetaan eksak dan tanpa heuristik. Pola `shouldChangeText`/`replaceCharacters`/`didChangeText` sudah dipakai replace-all (`SQLEditor.swift`, `replaceAllMatches`) dan menjaga `EditorDocument` tetap sinkron. |
| D-7 | **`script` adalah perintah baru dengan rute `LongOp`**: sesi sendiri, tidak dikolam, berbagi `Arc<Tunnel>` (P-05); tanpa transaksi implisit; tanpa retry statement sesudah terhubung (hanya koneksi awal lewat `open`). **Koreksi AR (B2):** "tanpa retry" harus dibuat eksplisit, tidak cukup diasumsikan. `execute_until_stopped` bukan pelaksana mentah: ia membungkus `retry::execute(session, policy, sql, options)` (`commands.rs:1418-1437`), yang mengirim ulang statement pada kegagalan `Transient` (`retry.rs:235-262`) dan mengembalikan `RetryCursor` yang juga mengulang permintaan halaman pada sesi yang cursornya bisa dilanjutkan (`retry.rs:312-343`). Jumlah ulangnya dari `RETRIES` (bawaan 5, `retry.rs:103`) lewat `RetryPolicy::from_settings`, dan `open` mengembalikan policy itu (`commands.rs:660-672`). Policy tersebut hanya untuk sambungan awal; bila dioper ke statement, sebuah tulis dikirim ulang sesudah koneksi putus, persis yang dilarang keputusan ini. Karena itu `script` meneruskan `RetryPolicy::new(0, Duration::ZERO)` yang dibuatnya sendiri ke setiap statement, tidak pernah `from_settings` dan tidak pernah policy hasil `open`; dengan nol ulang `again()` selalu salah (`retry.rs:155-157`), tepat satu percobaan. Alternatifnya varian `execute_until_stopped` yang memanggil `session.execute` langsung. | F-7, F-8, F-11. Satu sesi memberi kontinuitas `SET`, tabel sementara, dan `BEGIN`. Mengirim ulang statement tulis setelah koneksi putus bisa menggandakan efek. |
| D-8 | **Pecah satu kali di pintu, putuskan dengan `decisions_readings`, jalankan potongan yang sama.** Daftar potongan dari `statements_with_lines_dialect` (membawa `line`); tidak ada klasifikasi kedua per statement. Bila pembacaan berselisih: mode selain `full` menolak (seperti penjaga), `full` memecah dengan pembacaan bawaan dan menambah peringatan di `plan`. | F-2, F-5. Satu daftar mencegah selisih antara yang disetujui dan yang berjalan, dan tidak menggandakan baris `execution_log` (impor mencatat dua baris per statement). `full` tidak punya apa yang dijaga selain ketepatan pemecahan, dan tidak boleh mundur dibanding `import_data` (berkas dump MySQL ber-`\'`). |
| D-9 | **Konfirmasi skrip bersumber dari engine.** Sesudah pintu lolos, `script` memancarkan `plan` sebelum menyambung. Statement ber-keputusan `Confirm` yang belum dikonfirmasi berakhir `done { needs_confirmation: true }` tanpa koneksi. App menggambar lembar dari `plan.confirm`, lalu menjalankan ulang dengan `SAFE_MODE_CONFIRMED=1` dan `SCRIPT_PLAN_DIGEST`; engine menghitung ulang digest teks (`qh_storage::hash_statement`, atas byte SQL persis seperti diterima) dan menolak bila berbeda. Keputusan `Refuse` menolak seluruh skrip sebelum `plan` dan sebelum menyambung. | FR-RUN-01 menuntut konfirmasi yang menyebut semua write sekaligus; ADR-0026 menolak satu konfirmasi untuk rencana yang tidak dilihat pengguna, dan di sini pengguna melihat setiap statement yang dicakup. F-12: tiruan Swift bisa menyebut lebih sedikit daripada yang akan dijalankan engine. Digest mengikat persetujuan pada teks. **Keputusan ini bertingkat risiko tinggi: SEC memutuskan** (§14); cadangannya, tanpa digest, tetap aman karena lembar bersifat modal dan teks dibekukan. |
| D-10 | **Satu event `statement` dengan kunci `phase`, ditambah `plan` dan `done`; tipe dekode sendiri di Swift (`ScriptEvent`, `Support/ScriptWire.swift`).** Statement yang gagal bukan event `error`; sesudah `done` yang lengkap, perintah mengembalikan `Err(Warned)` bila ada statement gagal, supaya status keluar dan banner galat konsisten dengan `import_data`. | F-18, F-7. Menambah belasan kunci ke `Event` berisiko tabrakan tipe dan memperebutkan `App.swift`. Keluaran CLI tetap bermakna (status ≠ 0 pada kegagalan). |
| D-11 | **Statement berbaris dibaca sampai habis; `LIMIT` baris pertama disimpan, sisanya dihitung (`rows` = total, `kept` = tersimpan, `truncated`).** Paling banyak `SCRIPT_RESULTS_MAX` (awal 100) statement pertama yang berbaris mendapat store; sisanya dikuras dan dilaporkan dengan peringatan. | F-10: cancel-lalu-lanjut pada sesi yang sama berisiko mengenai statement berikutnya. Statement tulis ber-`RETURNING` juga tidak boleh dipotong di tengah jalan. Biayanya waktu membaca sisa hasil; Stop tetap menghentikan (via `stop_session`). Angka 100 adalah titik awal yang disetel di W12-T4 (R-7). |
| D-12 | **Hasil dikirim lewat `ResultSetSink` (trait asing UniFFI) dan `EngineHost::run_script`**, bukan lewat `EventSink`. Store dibuat engine saat statement mulai berbaris; Swift memegang `ResultHandle`-nya. Di CLI, MCP, dan golden (emitter tanpa penerima) hasil dikuras dan hanya dihitung. Cadangan bila UniFFI menolak argumen objek pada trait asing: `on_result_set(index, store_id)` dan `EngineHost::claim_result(store_id)`. | F-8. Kontrak `EventSink` dipakai setiap perintah dan setiap tes; menambah metode di sana memaksa semua pelaksananya berubah. `Released` di tengah statement (tab ditutup) tidak membatalkan skrip: baris berhenti disimpan, statement dikuras. |
| D-13 | **Hasil tiap statement ∈ {`ok`, `failed`, `unknown`, `cancelled`}**, dan `skipped` tersirat (indeks setelah yang terakhir dimulai). `unknown` untuk `EngineError::Query { kind: Transient }` sesudah statement terkirim. `done.open_transaction` memperingatkan skrip yang berakhir saat transaksi masih terbuka (`BEGIN`/`START` tanpa `COMMIT`/`ROLLBACK`/`END`, dibaca dari kata pembuka, hanya bila `capabilities().transactions`). | Skrip migrasi tanpa `COMMIT` diam-diam di-rollback saat sesi ditutup dan terbaca "berhasil". Heuristik: negatif palsu berarti tanpa peringatan, positif palsu berarti peringatan berlebih; keduanya tercatat di R-5. |
| D-14 | **Stop punya dua titik:** bendera dibaca sebelum tiap statement (sisa dilewati, sesi ditutup biasa), dan di dalam statement lewat `until_stopped` dan `stop_session` (cancel ke server, sesi ditutup). Keduanya mengakhiri skrip; `done.cancelled`. Cancel di dalam statement meng-rollback transaksi terbuka (sesi ditutup), dan itu dilaporkan lewat `open_transaction`. | FR-RUN-01, NFR-P6. |
| D-15 | **Model hasil di app:** `QueryTab.scriptResults` (daftar `ScriptResult`) dengan satu terpilih; yang terpilih dipasang sebagai `activeResult`, `baseResult`, dan `preview`, sehingga seluruh fitur grid berjalan tanpa diubah. Strip hanya muncul bila ≥ 2 hasil. Hasil skrip memakai **view in-memory saja** (sort, filter, search lewat `apply(viewSpec)`), karena menjalankan ulang satu statement ke server salah dan tidak aman (§5.2). Pin: paling banyak 8 per tab. **Koreksi AR (B3): `ScriptResult` memiliki store-nya.** Selama hasil skrip tampil, `activeResult` dan `baseResult` hanya alias tanpa kepemilikan; `releaseResults()` melewati store milik `ScriptResult` ter-pin, dan berpindah hasil tidak pernah memanggil `release()` (§5.1). | F-17, ide §1.5. **Ini kasus keempat fallback in-memory di luar tiga kasus O-8** (`performance-plan.md` §7 butir 3); diserahkan ke pemilik, dan **W12-T4 tidak dibuka sebelum jawabannya tercatat di ledger** (§5.2, §14). **Koreksi AR (B4):** menjalankan ulang statement skrip ke server untuk sort atau search juga tidak aman, bukan hanya salah. |
| D-16 | **Satu entri history per skrip lewat `history_add`, kolom yang ada saja.** `SQL` teks penuh, `OUTCOME` ok/error/cancelled, `ROW_COUNT` jumlah baris tersimpan ditambah baris terdampak, `ELAPSED_MS`, dan `ERROR_TEXT` "statement N of M (line L): pesan" bila gagal. Ringkasan statement dihitung panel History dari `SQL`. | `query_history` tidak punya kolom ringkasan (`qh-storage/src/history.rs`, `QueryHistoryRecord`); migrasi bukan lingkup W12. |
| D-17 | **"Run Script" diarahkan ke runner baru.** Item menu, `ShortcutAction.runScript`, dan "Run the Whole Editor" di chevron memanggil `AppModel.runScript`. Ekspor seluruh teks tetap lewat "Export…". | F-15. DBeaver mengikat ⌥X ke run script. Menyentuh `Shortcuts.swift` (rantai W9-T2) hanya untuk mengikat `.format` dan `.runScript` di skema `queryhive` bila ada tombol bebas; harus lulus `ShortcutConflictTests`. |
| D-18 | **Tab terikat berkas:** `QueryTab.file` (url, jejak terakhir: mtime, ukuran, SHA-256 isi). Kotor dihitung dari penghitung edit (`sqlEdits`), bukan perbandingan string. UTF-8 ketat (berkas non-UTF-8 ditolak, tidak pernah diubah diam-diam), akhir baris dideteksi dan dipulihkan saat simpan, teks di editor selalu `\n`. Simpan atomik. Perubahan luar diperiksa saat tab aktif (mtime dan ukuran) dan sebelum simpan (isi); app bertanya, tidak menimpa. Batas buka 8 MiB, di atasnya diarahkan ke impor `.sql`. Membuka berkas selalu menjadi tab baru (F-16: jalur lama menimpa SQL tab aktif tanpa bertanya). | FR-ED-03, PR-16. Berkas CRLF yang disimpan sebagai LF mengubah setiap baris di VCS: kerusakan senyap. |
| D-19 | **Sesi memuat `file`, `fileHash`, dan `dirty` (opsional, tanpa garis bawah).** SQL sesi selalu menang atas isi berkas (tidak ada SQL yang hilang); selisih dengan disk ditangani pemeriksaan perubahan luar saat aktif. Restore tidak menjalankan apa pun. | PR-16, UC-16, F-14. |
| D-20 | **Pembaca JSON: pembingkai sendiri, nilai mentah, NULL bertipe.** `RawRow.cells` menjadi `Vec<Option<String>>` (`None` = NULL pasti). Header dari sampel (≤ 1.000 elemen atau 16 MiB), kunci di luar sampel dihitung dan dilaporkan (`warnings`), bukan dibuang diam-diam. Angka dan boolean disimpan sebagai teks asli, objek dan larik bersarang sebagai teks JSON asli, string didekode. `COLUMNS` membawa `name` yang diverifikasi terhadap header untuk JSON. | F-21, F-22. Bilangan lewat `f64` rusak; `""` dan `null` tidak boleh disamakan; file yang berubah antara pratinjau dan impor tidak boleh menggeser pemetaan senyap. |
| D-21 | **Pratinjau lewat engine, bukan pembaca kedua di Swift:** setelan `IMPORT_PREVIEW=1` pada `import_data` membuka pembaca tanpa koneksi dan memancarkan `columns`, `rows` (`data`), `done`. Tidak menambah perintah, jadi tidak menyentuh empat daftar. | F-19: pembaca header Swift sudah jadi "jawaban kedua" untuk CSV. Memakai kunci `columns` dan `data` yang sudah dikenal `Event`. |
| D-22 | **`LongRunNotifier` di belakang protokol, kebijakan murni.** Notifikasi bila ≥ 20 dtk **dan** app tidak aktif saat selesai; izin diminta saat notifikasi pertama akan tampil; isi tanpa SQL, nama tabel, atau nilai; cadangan tanpa izin: `NSApp.requestUserAttention(.informationalRequest)`. Pengumuman VoiceOver untuk **setiap** Run (selesai, gagal, dihentikan). Progres Finder hanya untuk ekspor satu berkas yang path akhirnya diketahui sebelum byte pertama; progres Dock tak-tentu kecuali `tab.totalRows` diketahui. | FR-RUN-04, 05; F-20, F-23; NFR-S5. Notifikasi di layar kunci membocorkan isi. Tanpa cadangan, penolakan izin membuat fitur diam-diam tidak berbuat apa pun. |

## 3. Formatter dan toggle komentar (W12-T1)

### 3.1 Kontrak dan invarian

```rust
// crates/qh-sql/src/format.rs
pub struct FormatOptions { pub indent: Indent }                 // Spaces(1..=8) atau Tab
pub enum Indent { Spaces(u8), Tab }
pub struct Formatted { pub text: String }
pub enum FormatError {
    /// Pembacaan berselisih tentang region; `line` = baris region pertama yang berbeda.
    Ambiguous { line: usize },
    /// `walk` berakhir di dalam region terbuka (kutip, komentar blok, dollar-quote).
    /// Tidak pernah untuk `Open(LineComment)`: komentar baris di akhir teks tanpa baris baru adalah SQL sah (B1).
    Unterminated { kind: OpaqueKind, line: usize },
    TooLarge,
    /// Pemeriksaan sendiri gagal: bug di formatter. Teks asli tidak disentuh.
    SelfCheck,
}
pub fn format_sql(sql: &str, readings: &[Lexer], options: &FormatOptions) -> Result<Formatted, FormatError>;
```

Invarian, semuanya diperiksa saat runtime sebelum `Ok` (kecuali yang ditandai tes), dan gagal berarti `SelfCheck`, bukan hasil:

- **I-F1** byte non-spasi hasil sama persis, berurutan, dengan masukan. Spasi = `' ' \t \n \r \x0b \x0c`; NBSP dan karakter spasi Unicode lain adalah isi.
- **I-F2** isi setiap region opaque (byte demi byte) sama, dan jumlah serta jenis region sama. **Koreksi AR (B1):** `EndState` hasil juga sama dengan masukan: `Normal`, atau `Open(LineComment)` bila masukan berakhir di komentar baris tanpa baris baru. Hasil yang berakhir `Open` dengan jenis lain adalah `SelfCheck`.
- **I-F3** jumlah separator `;` sama (`walk` atas hasil).
- **I-F4 (tes)** idempoten: `format(format(x)) == format(x)`.
- **I-F5 (tes)** untuk setiap statement, `classify_readings` sebelum dan sesudah sama.
- **I-F6 (tes)** `walk` atas hasil di bawah tiap pembacaan menghasilkan daftar region yang sama dengan masukan di bawah pembacaan yang sama.

### 3.2 Algoritma

1. **Pintu.** `len > FORMAT_MAX_BYTES` → `TooLarge` (awal 4 MiB, disetel di T1 dari angka yang diukur untuk 1 MB). Untuk tiap lexer di `readings`, `walk` mengumpulkan `(OpaqueKind, start, end)` dan `EndState`. Daftar berbeda → `Ambiguous`; `EndState::Open(kind)` dengan `kind` selain `LineComment` → `Unterminated`. **Koreksi AR (B1):** `Open(LineComment)` diperlakukan tertutup. Komentar baris yang tidak diikuti baris baru memang berakhir `Open` (`scan.rs:418-426`, `:695-701`), dan itu keadaan biasa untuk dokumen yang diakhiri `-- catatan` serta untuk irisan statement D-6 yang berakhir di komentar. Menolaknya berarti menolak SQL sah.
2. **Atom.** `lex` atas pembacaan pertama memberi `Opaque`, `Word`, `Number`, `Param`, `Punct`. Byte non-spasi yang tidak tercakup token (F-1) menjadi atom `Other` yang menempel. `Punct` dipecah: `,` `(` `)` `;` menjadi atom struktural tunggal, sisanya tetap satu atom per deret. Celah di antara atom pasti hanya spasi (atau kosong).
3. **Pemisah yang diinginkan** per batas: `None`, `Space`, atau `Newline(indent)`, dihitung dari bingkai dan kata klausa. Daftar kata klausa sendiri (di `format.rs`, bukan daftar editor): `SELECT`, `FROM`, `WHERE`, `GROUP BY`, `HAVING`, `WINDOW`, `ORDER BY`, `LIMIT`, `OFFSET`, `FETCH`, `UNION`/`INTERSECT`/`EXCEPT` (+ `ALL`/`DISTINCT`), `WITH` (+ `RECURSIVE`), `VALUES`, `INSERT INTO`, `UPDATE`, `SET`, `DELETE FROM`, `RETURNING`, `ON CONFLICT`, dan `[NATURAL] [INNER/LEFT/RIGHT/FULL/CROSS] [OUTER] JOIN` dengan `ON`/`USING` diindentasi satu tingkat.
   - Bingkai: `(` membuka bingkai; bingkai adalah *bingkai query* bila atom kata pertamanya `SELECT`, `WITH`, `VALUES`, atau `TABLE`. Kata klausa hanya menyalakan baris baru di bingkai query. Karena itu `EXTRACT(year FROM d)`, `SUBSTRING(x FROM 1)`, `IS DISTINCT FROM`, dan `OVER (PARTITION BY a ORDER BY b)` tetap satu baris.
   - Daftar berkoma di tingkat nol sebuah klausa dengan ≥ 2 butir (daftar `SELECT`, `GROUP BY`, `ORDER BY`, penugasan `SET`) satu butir per baris, indentasi +1. `AND`/`OR` di tingkat nol `WHERE`/`ON`/`HAVING` memulai baris indentasi +1. `CASE … END`, daftar `IN`, dan argumen fungsi tetap satu baris (v1).
   - Komentar: komentar yang pada masukan **sebaris** dengan isi sebelumnya tetap menjadi komentar penutup baris (satu spasi); yang punya baris sendiri tetap di barisnya. Komentar blok mengikuti aturan yang sama. Baris kosong: ≥ 2 pemisah baris di masukan menjadi tepat satu baris kosong; selebihnya tidak ada penyisipan. Akhir dokumen: ada atau tidaknya baris baru penutup dipertahankan.
   - `;` menempel pada atom sebelumnya; sesudahnya satu pemisah baris.
4. **Rekonsiliasi per celah** (menjamin I-F1 dan D-2): celah asli kosong → tetap kosong (permintaan baris baru diabaikan), kecuali sesudah `,` dan `;` yang menerima pemisah; celah tidak-kosong dan diminta `None` → satu spasi, kecuali sebelum `,` `;` `)` dan sesudah `(` yang menjadi kosong; antara dua atom `Opaque` string (apa pun jenis kutipnya) → celah asli apa adanya; setelah komentar baris → pemisah baris, **kecuali tidak ada atom sesudahnya**: di akhir teks tidak ada pemisah yang disisipkan, jadi `-- note` tanpa baris baru tetap tanpa baris baru (Koreksi AR, B1); selain itu → yang diminta. Keluaran akhir selalu `\n`, bukan `\r\n`.
5. **Pemeriksaan sendiri** I-F1 sampai I-F3.

Yang sengaja tidak dikerjakan v1: pembungkusan lebar, spasi operator, daftar definisi `CREATE TABLE` satu kolom per baris, dan isi dollar-quote (opaque). Semuanya aditif dan tidak mengubah invarian.

### 3.3 Toggle komentar (`comment.rs`)

`toggle_line_comment(text, readings, first_line, last_line) -> Result<LineEdit, CommentError>`.

- Baris sasaran = baris penuh yang bersinggungan dengan seleksi (seleksi yang berakhir tepat di awal baris tidak menyertakan baris itu).
- Status awal dan akhir tiap baris dari region `walk` di semua pembacaan. **Koreksi AR (B1):** satu posisi dihitung "di dalam" region hanya bila **benar-benar di dalam** (`start < posisi < end`); posisi tepat di tepi tidak dihitung. **Baris yang awalnya atau akhirnya benar-benar di dalam string, identifier berkutip, komentar blok, atau dollar-quote menolak seluruh operasi** (`CommentError::InsideRegion { line }`; sama dengan D-5), dan catatan di Log menyebut barisnya. **Region `LineComment` tidak pernah menolak.** Tanpa aturan ini, `-- x` (awal baris = awal region) dan `SELECT 1 -- x` (akhir baris = akhir region, `scan.rs:418-426`) ditolak, padahal membuka dan menutup komentar baris adalah tujuan fitur ini. Region yang tidak pernah tertutup (`EndState::Open` selain `LineComment`) tidak punya ujung dan menutup seluruh sisa teks: baris mana pun yang memuat sebagian region itu menolak, seperti `Unterminated` di formatter.
- Semua baris tak-kosong sudah berawalan `--` (diikuti spasi atau akhir baris, supaya cocok dengan aturan MySQL) → buang `-- ` (atau `--`). Selain itu → sisipkan `-- ` pada **kolom indentasi terkecil** baris tak-kosong, jadi blok tetap sejajar; baris kosong tidak disentuh. Awalan `#` MySQL tidak dianggap komentar yang bisa dibuka.
- Hasil: satu rentang pengganti (UTF-16) dan seleksi baru, satu langkah undo.

### 3.4 Permukaan FFI (`crates/qh-ffi/src/editor.rs`, lane FFI)

Offset masuk dan keluar UTF-16, sama dengan `EditorDocument`.

```rust
#[derive(uniffi::Enum)]   pub enum KeywordCaseWire { Preserve, Upper }
#[derive(uniffi::Record)] pub struct FormatOptionsWire { pub indent_width: u8 /* 0 = tab */, pub keyword_case: KeywordCaseWire }
#[derive(uniffi::Record)] pub struct TextEdit { pub start: u32, pub len: u32, pub replacement: String,
                                                pub selection_start: u32, pub selection_len: u32 }
#[derive(uniffi::Error)]  pub enum FormatFfiError { Ambiguous { line: u32 }, Unterminated { line: u32 },
                                                    TooLarge, SelfCheck, OutOfBounds, SplitsCharacter }
#[uniffi::export] pub fn format_sql(sql: String, dialect: EditorDialect, options: FormatOptionsWire,
                                    selection_start: u32, selection_len: u32) -> Result<TextEdit, FormatFfiError>;
#[uniffi::export] pub fn toggle_line_comment(sql: String, dialect: EditorDialect,
                                             selection_start: u32, selection_len: u32) -> Result<TextEdit, CommentFfiError>;
```

`TextEdit` kosong (`len == 0`, `replacement == ""`) berarti tidak ada perubahan. Pemetaan seleksi: hitung karakter non-spasi sebelum kursor di teks lama, lalu temukan titik dengan hitungan yang sama di teks baru (I-F1 menjamin keberadaannya). Kasus kata kunci (D-4) dijalankan di sini sesudah `format_sql` dan memeriksa invarian terpisahnya (I-K1). Ekspor ini **bukan perintah**, jadi empat daftar invariant #11 tidak berubah; `app/Generated/` berubah dan di-commit bersama (invariant #1).

## 4. Perintah engine `script` (W12-T3)

### 4.1 Alur

`crates/qh-ffi/src/script.rs`, `pub async fn script(settings, out, engine, cancel)`:

1. **Pintu (tanpa jaringan).** `source_sql` (`SQL` atau `SQL_PATH`), `dialect(settings)`, `safe_mode(settings, engine)`, `statement_timeout`, `SCRIPT_POLICY` (`stop` bawaan atau `continue`, nama lain ditolak dengan nama), `LIMIT` (bawaan `PREVIEW_LIMIT`, tepi bawah satu), `SCRIPT_RESULTS_MAX` (bawaan 100). Potongan = `statements_with_lines_dialect`; kosong → galat usage. Keputusan = `decisions_readings(mode, sql, dialect.readings())` yang dicatat ke `execution_log` lewat `record_decision` seperti `guard_confirmed` (tidak ada klasifikasi kedua; D-8).
2. **Keputusan dibaca.** Ada `Refuse` → `CliError::Usage` dengan `SafeModeError::Refused` (menyebut indeks, jenis, dan potongan), tanpa `plan`, tanpa koneksi. Pembacaan berselisih dalam mode selain `full` sudah menjadi satu keputusan `Unknown` dari `decisions_readings` (ditolak). Ada `Confirm` dan `SAFE_MODE_CONFIRMED` tidak `1` → `plan` lalu `done { needs_confirmation: true }`, tanpa koneksi. `SAFE_MODE_CONFIRMED=1` dengan `SCRIPT_PLAN_DIGEST` yang berbeda dari `qh_storage::hash_statement(sql)` → galat usage "the script changed since it was confirmed". Tanpa `SCRIPT_PLAN_DIGEST` (CLI) konfirmasi diterima seperti pada perintah satu statement: pemanggil menjamin.
3. **`plan`** dipancarkan (`statements` memakai jumlah potongan, bukan jumlah keputusan), lalu `step: connect`, `open(settings, engine, &config)` (di dalamnya `enforce_read_only` bila `read_only`). **Koreksi AR (B2):** `open` mengembalikan `(Box<dyn Session>, RetryPolicy)`; policy-nya dari `RetryPolicy::from_settings` dan dipakai `retry::connect` di dalam `open` untuk sambungan awal saja (`commands.rs:660-672`). `script` membuang policy itu dan tidak meneruskannya ke statement.
4. **Loop statement.** Untuk tiap potongan: bendera cancel dibaca; `statement{start}`; `session.execute(text, &ExecuteOptions { row_limit: None, max_batch_rows: Some(STORE_FETCH_MAX), statement_timeout })` lewat `execute_until_stopped` dengan `&RetryPolicy::new(0, Duration::ZERO)` yang dibuat `script` sendiri, bukan `from_settings` dan bukan policy dari `open` (**Koreksi AR, B2:** `execute_until_stopped` membungkus `retry::execute`, jadi frasa awal "tanpa `retry::execute`" salah; lihat D-7); `pump_result` dengan `limit: None` menguras kursor ke `ScriptTarget` (§4.5); `affected_rows()` setelah habis untuk statement tanpa kolom. Gagal → `statement{failed|unknown}`, lalu kebijakan: `stop` keluar dari loop, `continue` lanjut.
5. **Penutup.** Tutup sesi (`close`, atau `stop_session` bila dibatalkan di tengah statement), hitung `open_transaction` (D-13), pancarkan `done`, dan bila ada statement gagal kembalikan `Err(CliError::Warned { message, warnings })` ("script stopped at statement 12 of 40 (line 87): …" atau "3 of 40 statements failed").

Tidak ada `BEGIN` implisit, tidak ada `COMMIT` implisit, dan tidak ada pengaturan sesi yang disisipkan engine selain yang dilakukan `open`.

### 4.2 Protokol event

Urutan: `plan`, `step`, lalu per statement `statement` (start, columns bila berbaris, progress, selesai atau gagal), dan `done`. Kunci baru dan tipenya (semua opsional di sisi Swift):

| Event | Kunci |
|---|---|
| `plan` | `statements` (int), `policy` (string), `digest` (string, SHA-256 heks atas teks), `needs_confirmation` (bool), `confirm` (larik `{index, line, kind, text}`, `text` = `snippet` yang sudah ada, ≤ 1.000 entri), `confirm_truncated` (bool), `warnings` (larik string) |
| `statement` `phase=start` | `index` (1-based), `line` |
| `statement` `phase=columns` | `index`, `columns` (`[{name, type}]`, bentuk `Event.Column` yang ada) |
| `statement` `phase=progress` | `index`, `rows` (int; dari `StoreTarget`, paling sering satu per 16 ms) |
| `statement` `phase=done` | `index`, `line`, `outcome` (`ok`, atau `cancelled` bila Stop memotong statement ini), `rows` (total baris yang dilihat, hanya statement berbaris), `kept` (tersimpan), `truncated`, `affected` (hanya statement tanpa kolom bila server melapor), `elapsed_ms`, `query_id` |
| `statement` `phase=failed` | `index`, `line`, `outcome` (`failed` atau `unknown`), `message`, `elapsed_ms`; `code` dan `position` hanya bila setelan galat-rinci W10-T6 terpasang (P-06) **(rencana)** |
| `done` | `statements`, `ran`, `failed`, `skipped`, `cancelled`, `policy`, `stopped_at` (indeks statement gagal pertama, bila ada), `open_transaction`, `needs_confirmation`, `elapsed_ms`, `warnings` |

Kunci `rows`, `truncated`, `elapsed_ms`, `query_id`, `message`, `columns`, `warnings`, `cancelled` memakai **tipe yang sama dengan `Event` yang ada**; kunci baru (`phase`, `index`, `line`, `kept`, `affected`, `outcome`, `statements`, `ran`, `failed`, `skipped`, `policy`, `digest`, `confirm`, …) tidak bertabrakan dengan properti `Event` mana pun. Karena app memakai `ScriptEvent` sendiri (D-10), ini terutama menjaga golden dan CLI tetap konsisten. `progress` per statement tidak memakai `Progress` throttle `PROGRESS_MS`; ia datang dari `StoreTarget` (16 ms) lewat `IndexedEmitter`, adaptor yang mengubah event `progress` dan `columns` milik target menjadi `statement` ber-`index` dan meneruskan `open_result_set` ke emitter di bawahnya. Daftar `confirm` dan seluruh `plan` tidak berisi kredensial: teksnya adalah teks pengguna, dan tidak dicatat di mana pun selain ditampilkan (execution log hanya menyimpan SHA-256, ADR-0026).

### 4.3 Safe Mode

- **Pintu mengikat semuanya.** Keputusan dibuat sekali (D-8). Matriks yang diuji di `tests/safe_mode.rs` (NFR-S1): empat mode × skrip campuran (baca, DML, DDL, tak terklasifikasi); `confirm` dengan dan tanpa `SAFE_MODE_CONFIRMED`, dengan digest benar dan salah; `read_only` menolak sebelum menyambung (penghitung `connect` pada engine palsu nol); MySQL dengan `\'` dan komentar `#` (NFR-S1, W3-T0); PostgreSQL dengan `E'…'`, `$tag$…$tag$`, dan `standard_conforming_strings`; Trino dengan `--` yang berakhir di `\r`.
- **Floor ADR-0027 tetap monoton**: `script` memakai `safe_mode()` yang sama (floor `max` by strictness), dan `table_op` tetap satu-satunya pengecualian "konfirmasi untuk DDL".
- **Dua pembacaan yang tidak setuju** dalam mode `full` memecah dengan pembacaan bawaan dan menambah peringatan `plan.warnings` ("statement boundaries depend on the server's sql_mode"), sama seperti `import_data` memperlakukannya hari ini.
- **Yang tidak berubah:** MCP tidak mendapat `script` (NFR-C, NFR-S6); MCP tetap `read_only`.

### 4.4 Stop, error, dan hasil tak diketahui

- Stop di antara statement: bendera dibaca sebelum `statement{start}`; sisa dilewati; `done.cancelled = true`, `skipped = N − ran`.
- Stop di dalam statement: `execute_until_stopped` dan `until_stopped` memenangkan balapan, `stop_session` mengirim cancel ke server dan menutup sesi (≤ 250 ms menunggu konfirmasi, `STOP_BUDGET`), statement itu `outcome: cancelled`; bila server tidak mengonfirmasi, `done.warnings` memuat `STOP_UNCONFIRMED` seperti `preview`.
- `EngineError::Timeout` → `failed` dengan pesan yang menyebut batas (`STATEMENT_TIMEOUT_MS`, bawaan app 60 dtk per statement, `AppModel.statementTimeoutMS`): migrasi dengan statement lebih lama dari itu gagal dengan alasan yang jelas (R-4).
- `Query { kind: Transient }` setelah kirim → `unknown`, pesan "the connection was lost while this statement ran; whether it was applied is not known", dan kebijakan `stop` berlaku. Koneksi putus saat membuka sesi awal adalah `error` biasa dan tidak ada yang berjalan.
- Kegagalan menulis event (`io::Error`) menghentikan skrip dengan galat, bukan melanjutkan tanpa pelapor.

### 4.5 Penerima hasil dan target per statement

```rust
// crates/qh-ffi/src/uniffi_api.rs
#[uniffi::export(foreign)]
pub trait ResultSetSink: Send + Sync {
    /// Called once per statement that returns columns, before its first row is stored.
    fn on_result_set(&self, index: u32, store: Arc<ResultHandle>);
}
// crates/qh-ffi/src/host.rs
#[uniffi::export]
impl EngineHost {
    pub fn run_script(&self, settings: Vec<Setting>, sink: Arc<dyn EventSink>,
                      results: Arc<dyn ResultSetSink>, cancel: Arc<RunCancel>);
}
// crates/qh-ffi/src/events.rs
pub trait Emitter {
    fn emit(&mut self, event: Json) -> io::Result<()>;
    fn result_store(&self) -> Option<StoreWriter> { None }
    /// A fresh store for statement `index` of a script, or `None` when this emitter keeps no results.
    fn open_result_set(&mut self, _index: usize) -> Option<StoreWriter> { None }
}
```

- `ScriptEmitter` (di `host.rs`) membungkus `SinkEmitter`, `Arc<StoreRegistry>`, dan `Arc<dyn ResultSetSink>`. `open_result_set` memanggil `registry.create()`, membuat `ResultHandle::new(handle, registry)`, memanggil `results.on_result_set(index, handle)`, dan mengembalikan `StoreWriter`-nya. Registry belum dikonfigurasi → statement berbaris gagal dengan galat bernama, bukan diam-diam tanpa hasil (`registry()` sudah begitu, `host.rs:490-500`). Engine tidak menyimpan `Arc<ResultHandle>`: umur store sepenuhnya di tangan Swift (D-12).
- `ScriptTarget` (di `script.rs`) adalah `RowTarget` yang membungkus `StoreTarget` bila `open_result_set` memberi penulis dan hanya penghitung bila tidak (CLI/MCP/golden). Pembungkus `CappedTarget` meneruskan paling banyak `LIMIT` baris pertama, menghitung sisanya (`seen`, `kept`), dan **menelan `StoreError::Released`**: handle yang dilepas berarti baris berhenti disimpan, statement tetap dikuras, dan skrip tidak dibatalkan (`pump_loop` memperlakukan `Released` sebagai cancel, `commands.rs:2183-2186`; pembungkus mencegah itu). Statement tanpa kolom tidak membuat store (`StoreTarget::begin` sudah menolak lebar nol, `:1939`). Statement berbaris ke-`SCRIPT_RESULTS_MAX + 1` dan sesudahnya dikuras tanpa store, dan `done.warnings` mencatat berapa yang tidak disimpan.
- `route(Command::Script) = Route::LongOp`; `EngineHost::run` biasa dengan `EngineCommand::Script` tetap bisa dipanggil (tanpa penerima hasil: dikuras), dipakai tes dan CLI.

### 4.6 Empat daftar invariant #11

`COMMANDS` (27 nama, `script` ditambahkan **di depan** `objects`, sesuai aturan urutan di `lib.rs:453-461`) dan `Command::{parse, Script}`; `EngineCommand::Script`, `EVERY_COMMAND` (27), dan `name()`; `RustEngine.commands`; tabel perintah-ke-event di kepala `lib.rs:22-40`. `RustRun.deliversAfterStop` ditambah `script` (Stop menghasilkan `statement`/`done` yang tetap harus sampai). `./app/build-ffi.sh`, commit `app/Generated/`, `swift test --filter RustEngineTests`.

## 5. Model hasil dan UI Run Script (W12-T4)

```swift
// Models/ScriptRun.swift (baru)
struct StatementOutcome: Equatable, Identifiable {
    var index: Int; var line: Int; var phase: Phase          // pending, running, ok, failed, unknown, cancelled
    var rows: Int?; var kept: Int?; var truncated: Bool; var affected: Int?
    var elapsedMS: Int?; var message: String?; var hasResult: Bool
    var id: Int { index }
}
@Observable final class ScriptResult: Identifiable {           // satu hasil berbaris
    let id = UUID(); let runID: UUID; let index: Int; let line: Int
    var slot: ResultSlot                                       // meta + StoreRows (W6-T1)
    var seen: Int                                              // total baris statement (>= kept)
    var pinned = false
    var label: String                                          // "#3 · SELECT * FROM users…", ≤ 40 karakter dari baris itu di teks
}
struct ScriptRun { let id: UUID; var statements: [StatementOutcome]; var total: Int; var cancelled: Bool; var openTransaction: Bool; var warnings: [String] }
```

- **`QueryTab`** mendapat `scriptRun: ScriptRun?`, `scriptResults: [ScriptResult]`, `selectedResultID: UUID?`. Memilih hasil memasang `slot.rows` sebagai `activeResult`, `slot.meta` sebagai `preview`, dan `baseResult = slot` (sehingga "off" kembali ke hasil itu); filter, seleksi, dan edit antrean dibuang seperti pada hasil baru. **Koreksi AR (B3):** pemasangan itu hanya menugaskan alias dan tidak pernah melepas store yang digantikan; pemiliknya `ScriptResult` (§5.1). Tab dengan satu hasil tidak menampilkan strip. `sourceTable` tidak diisi, jadi hasil skrip tidak bisa diedit (F-17).
- **Sort dan search (keputusan pemilik, §5.2).** `ScriptResult` membawa `origin = .script`; `sortOnServer`/`fireServerSearch` menolak dan jalur `applyMemorySort`/`apply(viewSpec)` dipakai (D-15). Banner "partial order" Batch 7 tidak berlaku karena hasil tersimpan adalah baris pertama dari kursor yang dikuras: urutan di dalam `kept` utuh, dan footer memakai `seen` ("1.000 of 1.503.221 rows · limit reached").
- **Pin.** `releaseResults()` (blueprint Fase 6 §19) melepas hasil tak ter-pin pada Run berikutnya dan semua hasil saat tab ditutup. **Koreksi AR (B3):** kalimat itu di versi awal tidak menyebut pemilik store; ditulis apa adanya, ia melepas hasil ter-pin yang sedang terpilih saat Run berikutnya mulai. Aturan yang mengikat ada di §5.1. Pin ke-9 ditolak dengan catatan ("Unpin a result first"). Hasil ter-pin tidak ikut sesi.
- **`AppModel.runScript(tab:from:confirmed:)`** (di `AppModel+Run.swift` **(rencana)**): `flushEditors`, `ParameterPrompt` bila ada `:name` (sama dengan `run`), lingkungan dari `previewEnvironment` ditambah `SCRIPT_POLICY` (pilihan di menu: Stop on error / Continue on error, tersimpan per tab) dan `SCRIPT_RESULTS_MAX`; panggilan pertama tanpa konfirmasi. `done.needs_confirmation` → `awaitConfirmation` dengan `RunConfirmation.Request` yang diisi dari `plan.confirm` (bukan `StatementScan`), lalu panggilan kedua dengan `SAFE_MODE_CONFIRMED=1` dan `SCRIPT_PLAN_DIGEST`. Memakai slot `previewing`/`previewProcess`/`previewToken` yang ada (satu Run per tab, seperti hari ini), sehingga Stop = `cancelPreview`. **Status keluar 1 dengan `done` yang sudah diterima berarti skrip selesai dengan kegagalan, bukan engine gagal:** hasil dan transkrip dipertahankan, dan pesan `error` menjadi banner (FR-RUN-06).
- **`DatabaseEngine.runScript(env:onEvent:onResultSet:onExit:)`** (di `DatabaseEngine.swift`, `RustEngine`, `MockEngine`, `SilentEngine`; protokol mewajibkannya supaya konformer yang terlewat gagal dikompilasi, seperti W6-T1). Sink Swift mengimplementasikan `EventSink` dan `ResultSetSink` pada satu kelas dan membawa keduanya lewat **satu** antrean utama berurutan, sehingga `onResultSet(index)` selalu tiba sebelum `statement{columns}` miliknya dibaca.
- **UI.** Menu Query dan chevron: "Run Script" (dua kebijakan di submenu atau saklar di sheet pilihan UX). Selama berjalan, bilah status menyebut "Running statement 7 of 40". Panel **Log** memuat satu baris per statement (`#7 · line 37 · 1,203 rows · 41 ms`, `#8 · line 41 · failed · pesan`), dan banner galat inline (V-4, W9-T5) untuk statement gagal dengan tombol "Reveal in Editor" yang memilih baris itu **hanya bila teks tab belum berubah sejak Run** (perbandingan string sekali, bukan per ketikan). Strip hasil (V-11, scene baru) ada hanya bila ≥ 2 hasil: tiap chip bernomor dan berlabel, ber-glyph pin, dapat dijangkau keyboard, dengan label VoiceOver "Result 2 of 4, statement 5, 1,000 rows, pinned" (NFR-A2, NFR-A3). Rincian visual diputuskan UX dan AX.
- **History.** `recordScriptHistory` memanggil `history_add` sekali di akhir (D-16). Entri statement tunggal lewat `runPreview` tidak berubah.
- **Anggaran memori.** Semua store lewat registry (anggaran 256 MiB, O-12) dan menumpah lebih dulu bila perlu; pin dan hasil aktif tidak menggandakan memori resident. `TabCloseTests` diperluas: menutup tab dengan skrip dan pin mengembalikan `store_stats().stores` ke nilai awal (G-LEAK).

### 5.1 Kepemilikan dan umur store (Koreksi AR, B3)

Versi awal D-15 memasang store hasil terpilih sebagai `activeResult` dan `baseResult = slot` tanpa menyebut siapa yang melepasnya. Kode hari ini melepas tanpa memeriksa pemilik: `releaseResults()` melepas `activeResult` dan `baseResult.rows` (`QueryTab.swift:885-893`) dan dipanggil di awal setiap Run preview (`AppModel.swift:2469`), awal explain (`:3008`), dan saat tab ditutup (`:717`); `showRows` melepas `activeResult` yang digantikan (`QueryTab.swift:900-902`). Dengan rancangan awal, hasil ter-pin yang sedang terpilih saat Run berikutnya mulai ikut di-`release()`: pin tidak bertahan (FR-RUN-02 gagal), dan berpindah hasil lewat jalur pasang yang ada menghancurkan hasil sebelumnya.

Aturan yang mengikat:

1. **`ScriptResult` memiliki `slot.rows`.** Hanya `ScriptResult.release()` yang melepasnya. Ia idempoten karena meneruskan ke `StoreRows.release()` yang idempoten (`StoreRows.swift:588-599`); `isReleased` tersedia untuk tes.
2. **Alias tanpa kepemilikan.** Selama hasil skrip tampil (`selectedResultID != nil`), `activeResult` dan `baseResult` menunjuk store milik `ScriptResult` terpilih dan tidak memilikinya. `QueryTab.ownsStore(_:)` menjawab apakah suatu store milik salah satu `ScriptResult` pada tab itu, dan `releaseUnlessOwned(_:)` adalah satu-satunya jalan yang boleh dipakai kode tab untuk melepas `activeResult` atau `baseResult`.
3. **Berpindah hasil tidak melepas apa pun.** `selectResult(_:)` hanya menugaskan alias (`activeResult`, `baseResult`, `preview`, `fetchedRows`) dan membuang filter, seleksi, dan edit antrean seperti di atas; tidak ada `release()` pada hasil sebelumnya.
4. **Jalur yang berubah** (grep ulang namanya; W9-T0 memindahkan sebagian ke `AppModel+*.swift`):
   - `QueryTab.releaseResults(includingPinned: Bool = false)` (`QueryTab.swift:885`): melepas dan mengeluarkan `ScriptResult` tak ter-pin lewat `ScriptResult.release()`; **melewati** setiap store milik `ScriptResult` ter-pin (lewat `releaseUnlessOwned`); lalu menihilkan `activeResult`, `baseResult`, dan `selectedResultID`. Hasil ter-pin tetap di `scriptResults`, hidup, dan dapat dipilih lagi dari strip. Awal Run preview (`AppModel.swift:2469`), awal explain (`:3008`), dan awal `runScript` semuanya memanggil `releaseResults()`, jadi perubahan di satu fungsi ini menutup ketiganya.
   - Sesudah Run preview atau explain, yang tampil adalah hasil Run itu (`selectedResultID = nil`), dan strip menambahkan satu chip untuk hasil terkini selama masih ada hasil ter-pin (bentuk visual: UX, V-11), supaya hasil ter-pin dapat dipilih kembali.
   - `closeTab` (`AppModel.swift:708-717`) memanggil `releaseResults(includingPinned: true)`: satu-satunya tempat hasil ter-pin dilepas. Itu satu baris di berkas yang memuat `closeTab` (tambahan kepemilikan, §10). `QueryTab.showRows` (`QueryTab.swift:900-902`; jalur fixture, snapshot, bench) memakai `releaseUnlessOwned` untuk `activeResult` yang digantikan.
5. **Tes bernama** (`ScriptResultTests`, `TabCloseTests`): `pinned_result_survives_next_preview_run` (jalankan skrip, pin #2, pilih #2, lalu Run preview: `isReleased` salah, sel #2 masih terbaca, `store_stats().stores` masih menghitungnya); `pinned_result_survives_explain_and_next_script_run`; `unpinned_results_are_released_once_on_next_run`; `switching_results_releases_nothing` (tiga hasil, pilih bolak-balik: tidak ada `release()` dan tidak ada `isReleased`); `close_tab_releases_pinned_results` (`store_stats().stores` kembali ke nilai awal, G-LEAK).

### 5.2 Sort dan search atas hasil skrip: keputusan pemilik sebelum T4 (Koreksi AR, B4)

D-15 membuat sort, filter, dan search atas hasil skrip berjalan lewat `apply(viewSpec)` di store (in-memory di Rust). O-8 (PRD §11) dan `performance-plan.md` §7 butir 3 hanya mengizinkan fallback in-memory pada tiga kasus: `ServerSort` atau `SearchStatement` menolak, hasil yang tampil adalah plan, hasil yang tampil adalah preview inspector objek. Hasil skrip adalah kasus **keempat**, jadi ia butuh keputusan pemilik. **W12-T4 tidak dibuka sebelum jawabannya tercatat di `target/run/ledger.md`** (nomor O berikutnya).

Alasan blueprint ini memilih in-memory lebih kuat daripada "tidak sama dengan statement itu di dalam skrip". Menjalankan ulang satu statement ke server untuk sort atau search itu **salah dan tidak aman**:

- **Salah.** Sort atau search ke server mengirim statement itu sendirian ke sesi kolam yang direset tiap Run (O-6, F-8): tabel sementara dan `SET` milik skrip tidak ada, jadi jawabannya bisa lain atau gagal.
- **Tidak aman.** Pembungkusnya `SELECT * FROM (<statement>) AS queryhive_sort ORDER BY …` (`ServerSort.swift:50`), dan mengirimnya berarti menjalankan statement skrip itu lagi, satu kali per klik header atau per pencarian. Untuk skrip yang berisi `INSERT/UPDATE … RETURNING` atau `CALL`, tulisan terjadi lagi bila server menerima bentuk itu. Penjaga Safe Mode memang membaca setiap kata tulis di dalam statement (`classify.rs:499-515`), jadi pembungkus yang memuat `INSERT` terklasifikasi menulis dan meminta konfirmasi atau ditolak. Tetapi di mode `full`, atau sesudah satu klik konfirmasi, tulisan berjalan lagi tanpa pengguna menyadari bahwa pemicunya klik pada header; dan `SELECT` yang memanggil fungsi penulis (`nextval`, fungsi buatan pengguna) tidak terlihat penjaga sama sekali, batas yang ia nyatakan sendiri (`classify.rs:46-48`). Bahwa PostgreSQL menolak `INSERT` di dalam `FROM (…)` (disimpulkan dari sintaks, tidak dijalankan di sini) kebetulan menguntungkan, tetapi bukan jaminan yang boleh dipegang app.

Pilihan untuk pemilik:

- **(a) In-memory saja untuk hasil skrip (D-15, rekomendasi).** Fallback keempat; dicatat di `performance-plan.md` §7 butir 3 dan satu nomor O baru.
- **(b) Sort dan search dinonaktifkan untuk hasil skrip.** Aman dan tanpa fallback baru; filter tetap jalan karena memang in-memory (`viewSpec`).
- **(c) Menjalankan ulang ke server: dicoret oleh blueprint ini**, jangan ditawarkan sebagai pilihan.

## 6. Berkas `.sql`, tab terikat, dan restore (W12-T2)

- **`FileBinding`** (di `QueryTab.swift`): `url`, `lastKnown: (mtimeNS, size, sha256)`, `eol: LF/CRLF/CR`, `hadBOM`. `sqlEdits &+= 1` di `didSet` `sql` (tanpa membandingkan string); kotor = `sqlEdits != savedEdits`. Menyimpan lalu mengundo kembali ke teks tersimpan tetap kotor (diterima).
- **Buka:** `AppModel.openFile(url:)` (dari ⌘O, dari drop, dan dari "Open SQL File…" di pohon). **Perubahan perilaku yang disengaja:** hari ini "Load SQL File…" menimpa SQL tab aktif tanpa bertanya (F-16); sesudahnya membuka berkas selalu menjadi tab baru, dan berkas yang sudah terbuka memilih tabnya, bukan menduplikasi. Dibaca UTF-8 ketat; deteksi akhir baris dominan dan BOM, teks di editor dinormalkan ke `\n`; > 8 MiB → ditolak dengan arahan ke Import `.sql` (`CEILING_UTF16` 2.000.000 unit sudah mematikan warna dan lipatan; 8 MiB adalah angka awal yang diukur dengan `type-2m` di T2, R-9).
- **Simpan / Simpan Sebagai (⌘S, ⇧⌘S).** Aksi responder-chain (P-16): `SQLTextView.saveDocument(_:)` memanggil `AppModel.save(tab)`; grid yang punya edit menangani ⌘S sendiri (W10-T3). Sebelum menulis: baca isi disk dan bandingkan SHA-256 dengan `lastKnown`; berbeda → lembar "File changed on disk": **Reload**, **Keep Mine** (tandai kotor, simpan berikutnya menimpa setelah konfirmasi kedua), **Save As…**. Penulisan atomik (`replaceItemAt`; perilaku atribut berkas tidak diverifikasi di sini, R-10), akhir baris dipulihkan. Tab tanpa berkas → Save menjadi Save As (panel simpan, nama dari judul tab).
- **Perubahan luar saat aktif:** `selectTab` memeriksa mtime dan ukuran (murah); berbeda → pertanyaan yang sama. Berkas hilang → tab dilepas dari ikatan dengan catatan di Log, isi editor tetap.
- **Drop:** `.onDrop(of: [.fileURL])` di `Workspace`, ekstensi `sql`/`txt`; beberapa berkas = beberapa tab; koneksi mengikuti tab terpilih.
- **Chip tab:** nama berkas, titik kotor (bukan hanya warna, NFR-A3), dan `help` dengan path lengkap.
- **Sesi (D-19):** `SessionTab` + `file`, `fileHash`, `dirty` (opsional, tanpa garis bawah; blob lama tetap terdekode). Restore: `sql` dari sesi, ikatan dibangun ulang, **tidak ada eksekusi**; berkas hilang/berubah ditangani pemeriksaan aktif. `SessionTests` menambah: restore tab terikat tidak menjalankan, SQL kotor tetap, blob lama tanpa kunci baru terdekode.
- **Aksi editor:** `SQLTextView.formatSQL(_:)` dan `toggleLineComment(_:)` (D-6, §3.3) dipanggil dari menu Edit lewat responder chain dan divalidasi (nonaktif tanpa editor aktif); galat (`Ambiguous`, `Unterminated`, `InsideRegion`) tampil sebagai catatan di Log dan pengumuman VoiceOver, bukan alert. Ikatan `.format` dan `.runScript` di skema `queryhive` (D-17).

## 7. Impor `.sql` dan JSON (W12-T5)

### 7.1 `.sql` dari app (FR-IMP-01)

Engine sudah punya keluarga ini (`import.rs:466-605`). Pekerjaan app: `ImportSourceFormat.sql`; sheet menyembunyikan bagian Target dan Fields (berkas membawa targetnya sendiri), menampilkan kebijakan `ON_ERROR` dan kunci asing, dan menghapus teks "not offered here" (`ImportSheet.swift:288-291`); `chooseFile` menerima `sql`; `ImportOutcome` memuat `statements`. Salinan sheet menjelaskan bedanya dengan Run Script: **impor = transaksi, semua-atau-tidak (bawaan)**; **skrip = per statement, tanpa transaksi implisit**. `import_data` untuk `.sql` membaca berkas **utuh** ke memori (`read_to_string`, `import.rs:482`): ditambah `IMPORT_SQL_MAX_BYTES` (bawaan 512 MiB) dengan galat usage yang menyebut batas dan ukuran berkas, sebagai penjaga proses (R-8). Event `done` sudah memuat `statements` dan `format: "sql"`; `Event` mendapat `statements: Int?` (satu baris di `App.swift`, rantai §10).

### 7.2 JSON dan JSONL (FR-IMP-02, P-19)

`crates/qh-import/src/json_source.rs`:

```rust
pub struct JsonSource { /* BufReader<File>, pembingkai, header, antrean sampel, baris, kunci_tak_terpetakan */ }
impl JsonSource {
    pub fn open(path: &Path, options: &Options) -> Result<Self, ImportError>;
    pub fn header(&self) -> Option<&[String]>;
    pub fn next_row(&mut self) -> Result<Option<RawRow>, ImportError>;      // cells: Vec<Option<String>>
    pub fn unmapped_keys(&self) -> &[String];                              // ≤ 20 nama
}
```

- **Pembingkai.** Mengabaikan BOM UTF-8 dan spasi, lalu: `[` → mode larik (elemen dipisah `,` sampai `]`); selain itu → nilai bersambung (JSONL dan JSON cetak-rapi). Batas elemen ditentukan dengan menghitung kedalaman `{[`/`}]` dan keadaan string/escape pada byte; tiap elemen (≤ `MAX_ELEMENT_BYTES` = 64 MiB, selebihnya galat dengan nomor baris) didekode `serde_json::from_slice`. Memori = elemen terbesar. `StreamDeserializer` ditolak: ia tidak memahami koma larik. Elemen harus objek; selain itu galat "element N (line L) is not an object". UTF-8 saja; UTF-16 ditolak dengan namanya. Nomor baris = baris tempat elemen dimulai.
- **Nilai.** Objek dibaca sebagai `Vec<(String, Box<RawValue>)>` (visitor `MapAccess` kecil, fitur `raw_value`): urutan kunci terjaga, kunci ganda = yang terakhir menang. String didekode; `null` dan kunci yang hilang → `None`; angka dan `true`/`false` → teks asli; objek/larik → teks JSON asli tanpa diubah. `""` tetap `Some("")`. Pembaca kedua yang menulis rentang nilai sendiri ditolak: dua parser untuk satu format.
- **Header.** Gabungan kunci urutan-pertama-terlihat dari ≤ 1.000 elemen pertama atau 16 MiB pertama; elemen sampel diantrekan sehingga berkas dibaca sekali. Kunci yang baru muncul setelah sampel tidak diimpor; pembaca mencatat sampai 20 nama dan `done.warnings` menyebutnya ("keys found after the first 1,000 rows were not imported: …"), bukan membuangnya diam-diam (SF).
- **`NULL_TEXT`.** Untuk JSON, `Plan.null_text` menjadi `Option<String>` dan bawaannya `None`; `Some` hanya bila pemanggil menyetelnya secara eksplisit. `literal` menerima `Option<&str>` (`None` selalu NULL). `row_is_empty` menganggap `None` kosong. Dua berkas sumber lain (`csv_source.rs`, `xlsx_source.rs`) berubah satu baris masing-masing (`cells` dibungkus `Some`) dan perilaku CSV/XLSX tidak berubah (R-16).
- **`COLUMNS`.** Entri boleh membawa `name`; untuk JSON engine memverifikasinya terhadap `header[source]` dan menolak ketidakcocokan ("COLUMNS[2] names 'email' but the file's column 2 is 'mail'; reload the file in the sheet"). CSV/XLSX tidak diverifikasi (header Swift dan engine bisa berbeda soal BOM dan kutip).
- **Pratinjau (D-21).** `IMPORT_PREVIEW=1` (dan `IMPORT_PREVIEW_ROWS`, bawaan 5): `import_data` membuka pembaca sebelum membaca konfigurasi koneksi, memancarkan `columns` (`[{name, type: "text"}]`), `rows` (`data`, baris contoh dengan `null` untuk NULL), dan `done { format, streams }`, tanpa menyambung. Sheet memakai ini untuk daftar field JSON; XLSX bisa ikut memakainya (menghapus batas "tidak ada daftar field"), **opsional** dan diputuskan implementer hanya bila kode pemetaan menjadi lebih sederhana. Pembaca header CSV Swift tidak disentuh di W12 (dicatat sebagai backlog).
- **`Format`.** `Format::Json` (`json`, `jsonl`, `ndjson` semuanya diterima `Format::parse`, nama keluaran `json`); ekstensi `json`, `jsonl`, `ndjson` di `source_of`; `RowReader::Json`; `streams()` benar. Sheet: `ImportSourceFormat.json` (tanpa pemisah dan tanpa "baris pertama berisi nama"), catatan "JSON null and missing keys become NULL; an empty string stays an empty string".
- **Transaksi.** Ketiga kebijakan (`stop`, `commit`, `skip`) dan penjaga Safe Mode jalur massal tidak berubah: sumber baris hanyalah pembaca lain di belakang `import_rows`. B-13 N2 di ledger (deadlock MySQL di tengah impor `commit` yang dilaporkan menulis awalan padahal rollback) **diperbaiki di T5** karena ia menyentuh berkas yang sama (`import.rs`) dan laporan `done` yang sama; tesnya tanpa server, dengan sesi palsu yang menggagalkan `COMMIT`.

## 8. Notifikasi, pengumuman, progres Finder dan Dock (W12-T6)

```swift
// Support/LongRunNotifier.swift (baru)
enum LongRunKind { case export, toTable, script }
enum LongRunOutcome { case done, failed, cancelled }
enum Authorization { case notDetermined, denied, authorized }
struct LongRunReport { let kind: LongRunKind; let outcome: LongRunOutcome; let elapsed: TimeInterval; let summary: String }
protocol NotificationPresenting: AnyObject {            // seam di depan UNUserNotificationCenter
    func authorizationStatus() async -> Authorization
    func requestAuthorization() async -> Bool
    func post(identifier: String, title: String, body: String, tabID: UUID) async throws
}
protocol RunAnnouncing { func announce(_ text: String) }   // NSAccessibility.post ke NSApp, seperti announceSelectionForAX
final class LongRunNotifier {
    static let threshold: TimeInterval = 20
    init(presenter: NotificationPresenting, announcer: RunAnnouncing, isAppActive: @escaping () -> Bool, attention: @escaping () -> Void)
    func finished(_ report: LongRunReport, tab: UUID) async     // kebijakan murni, mudah diuji
}
```

- **Kebijakan.** `elapsed > 20` **dan** `!isAppActive()` saat selesai, **dan** hasil bukan `cancelled` oleh pengguna (menekan Stop berarti ia di app). Ekspor, `to_table`, dan skrip memanggil `finished(...)` dari `AppModel+Export`/`AppModel+Run`. Pengumuman VoiceOver dipicu untuk **setiap** Run (preview, skrip, ekspor): "Query finished, 1,203 rows", "Query failed: …" (80 karakter pertama pesan), "Query stopped".
- **Izin.** Diminta saat notifikasi pertama akan tampil, oleh satu peminta, opsi `.alert` saja. Ditolak atau belum ditentukan → `attention()` (`NSApp.requestUserAttention(.informationalRequest)`), dan Log tab mencatat "notifications are off". Tidak ada dialog izin saat peluncuran.
- **Isi.** Judul "Export finished", "Script failed at statement 12", dan sejenisnya; badan berisi hitungan dan durasi. **Tanpa teks SQL, nama tabel, nama koneksi, atau nilai.** `userInfo` membawa id tab; klik mengaktifkan app dan memilih tab. `LongRunNotifier` sendiri menjadi `UNUserNotificationCenterDelegate` saat pertama dipakai (tanpa menyentuh `App.swift`); klik pada notifikasi yang tiba setelah app dimatikan dan diluncurkan ulang tidak memilih tab (R-11).
- **Pusat notifikasi nyata hanya dibuat lazy.** `UNUserNotificationCenter.current()` tanpa bundel yang layak melempar pengecualian (§13.1); presenter nyata tidak pernah dibuat di `--snapshot`, `--bench`, atau proses tes (dijaga `Bundle.main.bundleIdentifier` dan argumen proses). Tes memakai presenter palsu.
- **`FileProgress`** (`Support/FileProgress.swift`): membungkus `Progress` dan `NSDockTile`. **Finder:** hanya bila ekspor satu berkas, tanpa zip, `splitRows == 0`, dan format tidak membagi sendiri (`ExportFormat.splitsItself`); path akhir = `<outputDirectory>/<name>.<ext>`, diterbitkan (`publish()`) sesudah event `start` (berkas sudah dibuka) dan dicabut saat `done`. Untuk kasus lain (bagian bernomor, zip) tidak ada progres Finder, karena `report.csv` diganti nama saat bagian kedua dibuka (F-23). **Dock:** `badgeLabel` berisi jumlah baris ringkas ("1.2M") untuk ekspor tanpa total; bila `tab.totalRows` dihitung untuk `previewedSQL` yang sama, bilah progres tentu di `dockTile.contentView`. Keduanya dibersihkan di jalur `done`, `error`, dan Stop. Event `progress.rows` sudah ada (tidak ada perubahan engine).
- **Yang tidak bisa diverifikasi tanpa menjalankan app** dan diserahkan ke pemilik (`development-plan.md` §9, tindakan pemilik butir 6): pengiriman notifikasi dan perilaku dialog izin pada bundel bertanda-tangan ad-hoc, tampilnya progres Finder untuk berkas yang baru dibuat, dan tampilan Dock tile. Gate hanya notifier palsu.

## 9. Perubahan per tugas, berkas per berkas

Berkas bertanda **(+)** tidak ada di kolom Berkas `development-plan.md` §5 W12 dan ditambahkan di sini (§10).

### W12-T1. `feat(sql): a formatter that moves whitespace and nothing else`

| Berkas | Perubahan |
|---|---|
| `crates/qh-sql/src/format.rs` (baru) | §3.1-3.2: `format_sql`, `FormatOptions`, `FormatError`, atom, kisi spasi, bingkai, pemeriksaan sendiri |
| `crates/qh-sql/src/comment.rs` (baru) (+) | §3.3 |
| `crates/qh-sql/src/lib.rs` | `mod format; mod comment;` dan ekspor |
| `crates/qh-sql/tests/format.rs` (baru) (+) | tes properti dan korpus (§12) |
| `crates/qh-sql/tests/fixtures/format/*.sql` (baru) (+) | pasangan masukan dan hasil yang diharapkan per dialek |
| `crates/qh-ffi/src/editor.rs` | `format_sql`, `toggle_line_comment`, rekaman dan galat (§3.4), kasus kata kunci |
| `app/Generated/` | diregenerasi |

Gate: G-RUST, G-FFI. Tidak ada perintah baru (empat daftar tetap).

### W12-T3. `feat(engine): a script command that runs statements one by one with a stop or continue policy`

| Berkas | Perubahan |
|---|---|
| `crates/qh-ffi/src/script.rs` (baru) | §4.1: pintu, loop, `ScriptTarget`, `CappedTarget`, `IndexedEmitter`, penutup |
| `crates/qh-ffi/src/lib.rs` | `mod script`, `Command::Script`, `COMMANDS` 27, `run_with`, tabel kepala |
| `crates/qh-ffi/src/uniffi_api.rs` | `EngineCommand::Script`, `EVERY_COMMAND`, `name()`, `ResultSetSink` |
| `crates/qh-ffi/src/host.rs` (+) | `route`, `ScriptEmitter`, `EngineHost::run_script`; tes rute |
| `crates/qh-ffi/src/events.rs` (+) | `Emitter::open_result_set` bawaan `None` |
| `crates/qh-ffi/src/commands.rs` (+) | hanya visibilitas `pub(crate)` pada sebelas nama (F-9) |
| `app/Generated/` | diregenerasi |
| `Support/RustEngine.swift` | `commands`, `deliversAfterStop`, `runScript`, sink ganda |
| `Support/DatabaseEngine.swift` (+) | `runScript` pada protokol, `SilentEngine` |
| `Support/ScriptWire.swift` (baru) (+) | `ScriptEvent` dan dekoder |
| `crates/qh-ffi/tests/script.rs` (baru) (+) | sesi palsu seperti `import_sql.rs` |
| `crates/qh-ffi/tests/golden.rs`, `tests/golden/script/*` | kasus kursor palsu, direkam `--record` |
| `crates/qh-ffi/tests/safe_mode.rs` | matriks §4.3 |
| `Tests/QueryHiveTests/MockEngine.swift` (+), `RustEngineTests.swift` | `runScript` palsu; kesetaraan `commandWords` dengan `commandNames()` |

Gate: G-RUST, G-FFI, G-SWIFT, G-LIVE PostgreSQL (§12), G-GOLDEN.

### W12-T2. `feat(editor): format, toggle comment, and .sql tabs that save back to their file`

| Berkas | Perubahan |
|---|---|
| `Models/QueryTab.swift` | `FileBinding`, `sqlEdits`, `loadSQL` menjadi `openFile` |
| `Models/Session.swift` | kunci `file`, `fileHash`, `dirty` |
| `Models/AppModel+Files.swift` (baru, **(rencana)** setelah W9-T0) | `openFile`, `save`, `saveAs`, `checkExternalChange`, drop |
| `Models/Shortcuts.swift` (+) | ikat `.format` dan `.runScript` di skema `queryhive` |
| `App.swift` | menu File (Save, Save As…), menu Edit (Format SQL, Toggle Comment) lewat responder chain |
| `Views/SQLEditor.swift` | `formatSQL(_:)`, `toggleLineComment(_:)`, `saveDocument(_:)`, penjaga IME, penerapan satu undo |
| `Views/Workspace.swift` | chip berkas dan titik kotor, drop, lembar perubahan-luar |
| Tes | `FileBindingTests` (baru), `FormatActionTests` (baru), `SessionTests` diperluas |

Gate: G-SWIFT, G-VIS bila chip berubah (V-11 untuk permukaan baru).

### W12-T4. `feat(app): Run Script shows each statement's outcome and keeps a result per statement`

| Berkas | Perubahan |
|---|---|
| `Models/ScriptRun.swift` (baru) (+) | §5 |
| `Models/QueryTab.swift` | `scriptRun`, `scriptResults`, `selectedResultID`, pin, dan kepemilikan store (§5.1): `ScriptResult.release()`, `ownsStore`, `releaseUnlessOwned`, `releaseResults(includingPinned:)`, `showRows` |
| `Models/AppModel+Run.swift` | `runScript`, konfirmasi dari `plan`, history, Stop |
| berkas yang memuat `closeTab` (`AppModel.swift:708` hari ini) (+) | satu baris: `releaseResults(includingPinned: true)` (§5.1, B3) |
| `Views/ResultSetStrip.swift` (baru) (+) | strip hasil (tetap kecil, tidak menambah `ResultGrid.swift`) |
| `Views/ResultGrid.swift` | menyisipkan strip, footer memakai `seen` |
| `Views/Panels.swift` | baris Log per statement, panel History: jumlah statement |
| `App.swift` (+) | "Run Script" ke runner baru (rantai §10) |
| `Views/Workspace.swift` (+) | "Run the Whole Editor" diganti "Run Script", pilihan kebijakan |
| Tes | `ScriptRunTests`, `ScriptResultTests` (baru), `TabCloseTests`, scene snapshot baru `script-results` |

Gate: G-SWIFT, G-VIS (scene baru, V-11), G-LEAK untuk store skrip.

### W12-T5. `feat(import): .sql and JSON files from the app, under the same transaction policy`

| Berkas | Perubahan |
|---|---|
| `crates/qh-import/src/json_source.rs` (baru) | §7.2 |
| `crates/qh-import/src/lib.rs` | `Format::Json`, `RawRow.cells: Vec<Option<String>>`, `RowReader::Json`, `ImportError::Json` |
| `crates/qh-import/src/{csv_source,xlsx_source}.rs` (+) | `Some(...)` satu baris |
| `crates/qh-import/Cargo.toml`, `Cargo.lock` (+) | `serde`, `serde_json` (`raw_value`) |
| `crates/qh-ffi/src/import.rs` | JSON, `null_text` opsional, verifikasi `name`, `IMPORT_PREVIEW`, `IMPORT_SQL_MAX_BYTES`, `warnings` kunci tak terpetakan, perbaikan B-13 N2 |
| `crates/qh-ffi/tests/import_json.rs` (baru) | §12 |
| `Models/ImportMapping.swift`, `Views/ImportSheet.swift` | `.sql`, `.json`, pratinjau, salinan |
| `App.swift` (+) | `Event.statements` (satu baris) |
| Tes | `ImportMappingTests` diperluas |

Gate: G-RUST, G-SWIFT, G-LIVE PostgreSQL (ketiga kebijakan untuk JSON).

### W12-T6. `feat(app): long exports report progress in Finder and the Dock, and say when they finish`

| Berkas | Perubahan |
|---|---|
| `Support/LongRunNotifier.swift` (baru) | §8 |
| `Support/FileProgress.swift` (baru) | §8 |
| `Models/AppModel+Export.swift` | `FileProgress` dan `finished` di jalur ekspor dan `to_table` |
| `Models/AppModel+Run.swift` | `finished` dan pengumuman di akhir `runPreview` dan `runScript` |
| Tes | `LongRunNotifierTests` (baru): ambang 20 dtk, app aktif, dibatalkan, izin ditolak jatuh ke `attention`, isi tanpa SQL |

Gate: G-SWIFT (notifier palsu).

## 10. Selisih terhadap dokumen lain

- **`development-plan.md` §5 W12, berkas.** Tambahan bertanda (+) di §9: T1 `comment.rs`, tes dan fixture; T3 `host.rs`, `events.rs`, `commands.rs` (visibilitas), `DatabaseEngine.swift`, `ScriptWire.swift`, `MockEngine.swift`, `tests/script.rs`; T2 `Shortcuts.swift`; T4 `ScriptRun.swift`, `ResultSetStrip.swift`, `App.swift`, `Workspace.swift`, dan satu baris `closeTab` di berkas yang memuatnya (B3); T5 `csv_source.rs`, `xlsx_source.rs`, `Cargo.toml`, `Cargo.lock`, `App.swift` (satu baris). Berkas `Support/EngineWire.swift` di baris T3 **dicoret**: event skrip memakai `ScriptWire.swift`, dan `Event` ada di `App.swift`, bukan `EngineWire.swift`.
- **`development-plan.md` §7, rantai kepemilikan.** `crates/qh-ffi/src/commands.rs`: … → W11-T1 → **W12-T3** → W13-T2. `crates/qh-ffi/src/{host,events}.rs`: ikut lane FFI (… → W10-T6 untuk `events.rs` → W12-T3). `Cargo.toml`/`Cargo.lock`: … → W7-T5 → **W12-T5** → W13-T8a. `App.swift`: … → W10-T3 → **W12-T5** (satu baris) → W12-T2 → **W12-T4** → W13-T8b. `Models/Shortcuts.swift`: W9-T2 → W12-T2. `crates/qh-sql/src/lib.rs`: W3-T2 → W12-T1 (tetap).
- **`development-plan.md` §5 W12, urutan batch.** Tidak berubah; T4 bergantung pada T2 (rantai `QueryTab.swift`/`App.swift`) dan T3 (protokol). T5 di batch 1 menyentuh `App.swift` sebelum T2 di batch 2: tidak bersamaan.
- **`development-plan.md` §5 W11-T6.** Kalimat "menggantikan `crates/qh-sql/src/editor/`" usang (F-3).
- **PRD FR-ED-01.** "Deretan token non-spasi identik" dibaca sebagai invarian byte (I-F1), dan "mengikuti setelan keyword case" sebagai langkah terpisah dengan invarian sendiri (D-4).
- **PRD FR-RUN-02 dan O-8.** Hasil skrip memakai view in-memory saja (D-15): kasus keempat di luar tiga kasus fallback `performance-plan.md` §7 butir 3. Perlu persetujuan pemilik **sebelum W12-T4 dibuka**, dan jawabannya dicatat di ledger. **Koreksi AR (B4):** alternatif "menjalankan ulang satu statement ke server" dicoret: bukan hanya salah bila statement bergantung pada tabel sementara, tetapi tidak aman karena statement tulis yang berefek (`INSERT/UPDATE … RETURNING`, `CALL`, fungsi penulis) berjalan lagi (§5.2). Bila in-memory ditolak, satu-satunya alternatif aman adalah menonaktifkan sort dan search untuk hasil skrip.
- **PRD FR-RUN-03.** Ringkasan hasil di history tidak punya kolom; dihitung dari teks (D-16). Kolom `summary` memerlukan migrasi `qh-storage` dan di luar W12.
- **PRD FR-ED-03.** "Load SQL File…" tidak lagi menimpa SQL tab aktif (D-18): perubahan perilaku yang disengaja dan dicatat di pesan commit T2.
- **`blueprints/fase-6-data-plane.md` §12.3 dan §17.** `run_script` dan `ResultSetSink` menambah jalur store di samping `run_with_store`; `ResultHandle` dan registry tidak berubah. `StoreRows(handle:)` dibangun di sisi Swift dari handle yang diterima.
- **`docs/invariants.md` #8.** Tidak ada pemecahan di driver: `script` memecah di `qh-sql` dan mengirim satu statement per `execute`.
- **ADR-0019/0022.** Tidak diubah. `script` bukan jalur impor dan tidak menyentuh kebijakan transaksinya; dua ADR baru di §14.

## 11. Urutan kerja dan aturan membuka tugas

1. **T1** (lane FFI) dan **T5** (Rust `qh-import` dan `import.rs`, Swift sheet) berjalan bersama; ledger O-22 mengusulkan memecah T1 dan T5 menjadi bagian Rust-inti (a) dan Swift/FFI (b).
2. **T3** (lane FFI, setelah T1 mendarat) dan **T2** (setelah T1).
3. **T4** (setelah T2 dan T3, **dan sesudah pemilik menjawab D-15 dengan jawaban tercatat di ledger**; Koreksi AR, B4).
4. **T6**.
5. **W12-D** (ADR 0041 dan 0042) dan **W12-C**, lalu gate W12 (G-HEAVY, G-BENCHQ ketikan).

**Aturan pembuka setiap tugas (brief implementer):** grep ulang nama yang dirujuk dokumen ini terhadap kepala branch, karena W6 sampai W11 mengubah banyak: `QueryTab.activeResult`/`ResultSlot`/`StoreRows` (W6-T1), `AppModel+Run`/`+Export`/`+Files` (W9-T0), `RunConfirmationSheet`/`RunConfirmation` (W9-T5), dialek per tab di `EditorDocument` (W10-T6), setelan galat-rinci dan `position`/`code` di `events.rs` (W10-T6), `Shortcuts.swift` (W9-T2), dan `runIntoStore` di `DatabaseEngine`/`MockEngine` (W6-T1). Selisih terhadap dokumen ini dicatat di pesan commit.

**Implementer:** sonnet untuk semua (O-17/O-20). T3 dan bagian konfirmasi §4.3 adalah satu-satunya yang menyentuh Safe Mode dan batas FFI sekaligus: reviewer opus (RR, DB, SF, SEC) satu putaran, dengan temuan memblokir diperbaiki sonnet lalu dicatat "pending review" di ledger (O-20).

## 12. Tes yang ditulis lebih dulu

**T1 (`crates/qh-sql/tests/format.rs`, fixture di `tests/fixtures/format/`):**

- Properti I-F1…I-F6 atas korpus tetap dan atas generator acak berbiji (xorshift seperti `store_synthetic`, tanpa dependensi baru) yang menyusun sup token dari: kata, angka, string dengan kutip ganda dan escape per dialek, komentar `--`/`#`/`/* */` (bersarang di PostgreSQL), dollar-quote, `:name`, `::`, `$1`, `->>`, dan byte ≥ 0x80. Seluruh pembacaan per dialek.
- Kasus bernama yang tidak boleh berubah artinya: `a - -b`, `a--b` (MySQL: bukan komentar), `$1`, `x::int`, `'a'` baris baru `'b'` (PostgreSQL), `E'a\'b'`, `$$ … ; … $$`, `/*! … */` (MySQL, ditolak `Ambiguous`), `SELECT 'C:\temp\'` (PostgreSQL, ditolak `Ambiguous`), kutip terbuka (`Unterminated`), `:name` dalam string.
- Format: `EXTRACT(year FROM d)`, `IS DISTINCT FROM`, `OVER (PARTITION BY … ORDER BY …)`, subquery, CTE, `INSERT … SELECT`, `UPDATE … SET a = 1, b = 2`, join bertingkat, `UNION ALL`, komentar penutup baris dan komentar sendiri, baris kosong di antara statement. Hasil dibandingkan dengan fixture, per dialek.
- Toggle komentar: baris di dalam string, komentar blok, dollar-quote (ditolak); baris yang membuka string (ditolak); blok sejajar dengan indentasi campuran; buang komentar; baris kosong tak tersentuh; seleksi yang berakhir di awal baris.
- **Koreksi AR (B1), tes bernama:** `format_accepts_line_comment_at_eof_without_newline` (`SELECT 1 -- note` tanpa baris baru, dan irisan statement D-6 yang berakhir `-- note`: `Ok`, komentar dan tiadanya baris baru terjaga, I-F1 sampai I-F3 lulus, idempoten); `format_rejects_unterminated_block_comment_at_eof` (`SELECT 1 /* note` → `Unterminated`, pembanding agar `Open` selain `LineComment` tetap ditolak); `toggle_uncomments_a_line_comment_line` (`-- x` → `x`, bukan `InsideRegion`); `toggle_comments_a_line_ending_in_a_line_comment` (`SELECT 1 -- x` → `-- SELECT 1 -- x`); `toggle_refuses_a_line_that_starts_inside_a_block_comment` (pembanding).
- FFI: offset UTF-16 dengan surrogate, `SplitsCharacter`, pemetaan seleksi dengan spasi bertambah dan berkurang, kasus kata kunci (I-K1), `Generic` memakai 12 pembacaan.
- Angka (bukan gerbang): format 1 MB SQL dicatat di pesan commit.

**T3 (`crates/qh-ffi/tests/script.rs`, sesi palsu; `safe_mode.rs`; `golden.rs`):**

- Urutan dan isi event untuk: tiga statement sukses; kebijakan `stop` (statement ke-2 gagal, `skipped`, `stopped_at`, status keluar lewat `Err(Warned)`); kebijakan `continue`; cancel di antara statement; cancel di dalam statement (sesi palsu yang menggantung, `stop_session` dipanggil sekali); `unknown` pada `Transient`; `open_transaction` (`BEGIN` tanpa `COMMIT`, `BEGIN` di dalam dollar-quote tidak dihitung); peringatan `SCRIPT_RESULTS_MAX`; `Released` di tengah statement tidak membatalkan skrip; `LIMIT` memotong penyimpanan tetapi `rows` menghitung semua dan sesi dipakai ulang untuk statement berikutnya; tanpa penerima hasil, baris dikuras.
- Tidak ada retry statement (sesi palsu yang gagal sekali tidak dipanggil dua kali). **Koreksi AR (B2):** diuji dua kali, tanpa `RETRIES` (`script_never_resends_a_statement`) dan dengan `RETRIES=3` (`script_never_resends_a_statement_with_retries_3`): sesi palsu yang gagal `Transient` pada `execute` dipanggil tepat satu kali per statement dan hasilnya `unknown`; cursor palsu yang gagal `Transient` pada `next_batch`, dari sesi tanpa `persistent_connection` (yang dianggap `RetryCursor` bisa dilanjutkan), tidak diminta ulang. Pembanding `script_connect_still_retries`: sambungan awal tetap mengulang menurut `RETRIES`, supaya perbaikan ini tidak mematikan retry koneksi.
- Safe Mode (matriks §4.3), dengan penghitung `connect` nol untuk skrip yang ditolak dan untuk `needs_confirmation`; digest benar, salah, dan teks yang berubah; baris `execution_log` satu per keputusan.
- Golden (kursor palsu): `plan` dan `statement` untuk satu kasus sukses dan satu gagal; direkam dengan `--record`.
- **G-LIVE PostgreSQL** (`QH_TEST_POSTGRES=1`): `CREATE TEMP TABLE` lalu `SELECT` dari tabel itu dalam satu skrip; `SET search_path` yang bertahan antar statement; `stop` pada galat sintaks di statement ke-3 dari 5; `continue`; Stop di dalam `pg_sleep(30)` dengan pengakuan ≤ 100 ms (NFR-P6) dan `done.cancelled`; `SELECT generate_series(1, 1500000)` ber-`LIMIT 1000` menghasilkan `rows = 1.500.000`, `kept = 1000`, lalu statement berikutnya berhasil di sesi yang sama; `BEGIN` tanpa `COMMIT` memicu `open_transaction`.
- Swift: `RustEngineTests` (kesetaraan 27 nama), `ScriptWireTests` (dekode setiap fase, kunci tak dikenal diabaikan), `MockEngine.runScript` memutar event dan hasil palsu.

**T2:** `FileBindingTests` (buka UTF-8, tolak non-UTF-8, CRLF dipulihkan saat simpan, BOM, kotor lewat `sqlEdits`, perubahan luar terdeteksi lewat isi walau mtime dan ukuran sama, Keep Mine tidak menimpa tanpa konfirmasi, berkas hilang, membuka tidak menimpa SQL tab aktif), `FormatActionTests` (satu langkah undo, seleksi dipetakan, `hasMarkedText` menolak, galat jadi catatan Log), `SessionTests` (restore tanpa eksekusi, SQL kotor tetap, blob lama terdekode).

**T4:** `ScriptRunTests` (urutan event ke `StatementOutcome`, konfirmasi dari `plan` bukan `StatementScan`, panggilan kedua membawa digest, Stop = `cancelPreview`, `deliversAfterStop`, status keluar 1 dengan `done` mempertahankan hasil), `ScriptResultTests` (strip hanya ≥ 2 hasil, memilih hasil memasang `activeResult` dan mengosongkan seleksi, pin tahan Run ulang (**Koreksi AR, B3:** termasuk saat hasil ter-pin itu sedang terpilih; tes bernama di §5.1), pin ke-9 ditolak, sort server ditolak untuk hasil skrip), `TabCloseTests` (`store_stats().stores` kembali ke awal, juga dengan hasil ter-pin), scene `script-results`.

**T5 (`crates/qh-ffi/tests/import_json.rs` dan unit di `json_source.rs`):** JSONL, larik, cetak-rapi bersambung, BOM, elemen bukan objek, elemen > batas, kunci ganda (terakhir menang), `null` vs `""` vs hilang, angka 30 digit dan `1.10` utuh, objek bersarang ke kolom `jsonb`, header dari sampel dan kunci setelahnya masuk `warnings`, verifikasi `name` pada `COLUMNS`, `IMPORT_PREVIEW` tanpa koneksi, ketiga kebijakan (`stop`, `commit`, `skip`) atas sumber JSON dengan sesi palsu, B-13 N2, `IMPORT_SQL_MAX_BYTES`. G-LIVE PostgreSQL untuk ketiga kebijakan. Swift: `ImportMappingTests` (`.sql` tanpa target, `.json` tanpa pemisah, `COLUMNS` membawa `name`).

**T6 (`LongRunNotifierTests`):** 19 dtk tidak, 21 dtk ya; app aktif tidak; dibatalkan tidak; izin ditolak memanggil `attention` sekali; satu permintaan izin walau dua notifikasi; isi tidak memuat SQL; presenter nyata tidak dibuat di proses tes.

## 13. Risiko

| ID | Risiko | Mitigasi |
|---|---|---|
| R-1 | **Formatter mengubah arti SQL** lewat spasi yang disisipkan atau dihapus. | Kisi D-2, tiga pemeriksaan runtime, tes properti atas semua pembacaan, `SelfCheck` mengembalikan teks asli. |
| R-2 | **Format menolak terlalu banyak** (MySQL `\'`, PostgreSQL backslash di string biasa, komentar eksekutabel `/*!`). | Pesan menyebut baris dan cara keluar (`''`, `E''`); sama dengan penjaga. Format per statement (awalan yang disepakati semua pembacaan) dicatat sebagai backlog, bukan v1. |
| R-3 | **Konfirmasi skrip menyetujui lebih dari yang dilihat pengguna.** | Daftar dari engine (`plan.confirm`), digest teks, lembar modal; SEC memeriksa D-9. Tiruan `StatementScan` tidak dipakai untuk skrip. |
| R-4 | **Timeout 60 dtk per statement** (bawaan app) menggagalkan migrasi yang sah. | Pesan galat menyebut batas; satu setelan (`statementTimeoutMS`); UX memutuskan apakah Run Script menawarkan "no limit" per Run. Tidak ada pengecualian diam-diam. |
| R-5 | **Heuristik `open_transaction`** salah baca (`BEGIN` sebagai kata pembuka di blok prosedural MySQL, varian `START TRANSACTION`). | Hanya peringatan, tidak pernah memblokir; dollar-quote sudah opaque; tes bernama. |
| R-6 | **Satu statement berbaris sangat besar** dikuras lama karena tidak boleh dipotong (D-11). | Stop berfungsi lewat `stop_session`; progres `rows` terus bergerak; dicatat sebagai harga D-11. Penguras tanpa dekode (`row_limit` sisi driver) adalah perbaikan driver di W7 dan bukan prasyarat. |
| R-7 | **100 hasil per skrip dan 8 pin** adalah tebakan; dump dengan ribuan `SELECT setval(...)` menekan anggaran dan fd (R-19 Fase 6). | Statement ke-101 dan seterusnya dikuras dengan peringatan; angka disetel di T4 terhadap `tabs-100-held`. |
| R-8 | **`.sql` dibaca utuh ke memori** (`import.rs:482`) sehingga dump multi-GB menekan proses. | `IMPORT_SQL_MAX_BYTES` (512 MiB) dan arahan pada galat; pembaca bertahap butuh pemecah yang bisa dilanjutkan (F-1) dan di luar W12. |
| R-9 | **8 MiB** sebagai batas buka berkas adalah angka awal; NSTextView di atas `CEILING_UTF16` belum diukur untuk ketikan. | `type-2m` di T2; batas disetel dan dicatat di pesan commit. |
| R-10 | **Penulisan atomik** mengganti inode dan bisa mengubah izin atau atribut berkas pengguna. | Gunakan `replaceItemAt` dan periksa izin sebelum dan sesudah di tes; dicatat sebagai tak terverifikasi di sini. |
| R-11 | **Notifikasi** tidak tampil pada bundel ad-hoc, atau klik pasca-peluncuran ulang tidak memilih tab. | Cadangan `requestUserAttention`; pemilik memeriksa pengiriman nyata; batas klik dicatat. |
| R-12 | **UniFFI menolak objek sebagai argumen trait asing.** | Cadangan terdokumentasi: `on_result_set(index, store_id)` dan `EngineHost::claim_result`; diputuskan T3 dengan satu percobaan kecil sebelum menulis sisanya. |
| R-13 | **Kebijakan `continue` di dalam `BEGIN … COMMIT` PostgreSQL**: transaksi gagal, statement berikutnya semuanya gagal ("current transaction is aborted"). | Perilaku server yang jujur; salinan menu `continue` menyebutnya; tidak ada deteksi di engine. |
| R-14 | **Trino tidak menyimpan sesi antar statement** (invariant #9): `SET SESSION`, `USE` di dalam skrip mungkin tidak bertahan. | Dicatat di ADR-0042; perilaku driver, tidak diverifikasi di sini. |
| R-15 | **Event `statement` dan hasil tiba lewat dua saluran** dan urutannya salah. | Satu kelas sink Swift dan satu antrean utama berurutan (§5); tes urutan pada `MockEngine`. |
| R-16 | **Perubahan `RawRow.cells`** mengubah perilaku CSV/XLSX tanpa sengaja. | `Some(...)` mekanis, `row_is_empty` dan `literal` memetakan `Some` persis seperti dulu; tes CSV/XLSX yang ada tidak berubah. |
| R-17 | **Fitur `raw_value` dan `serde` di `qh-import`** menyatukan fitur di seluruh graf. | `raw_value` hanya menambah tipe; `cargo deny check licenses` di G-DENY dan G-RUST. |

### 13.1 Yang tidak terverifikasi di dokumen ini

Dokumen ini ditulis tanpa menjalankan `cargo`, `swift`, build, atau bench. Hal berikut dibaca atau disimpulkan, tidak dijalankan, dan setiap implementer harus membuktikannya sebelum mengandalkannya:

- Bahwa trait asing UniFFI menerima `Arc<ResultHandle>` sebagai argumen (R-12). Hari ini hanya `EventSink` dengan `String` yang terbukti.
- Bahwa kursor PostgreSQL yang dihentikan di cap meninggalkan respons yang dikuras tugas koneksi, dan bahwa pernyataan berikutnya di sesi itu menunggu pengurasan (F-10): disimpulkan dari komentar `commands.rs:1459-1466` dan `:1763-1768` serta `qh-driver-postgres/src/lib.rs:802-806`.
- Bahwa `serde_json::Number` tanpa `arbitrary_precision` merusak bilangan non-bulat panjang (F-22): dokumentasi crate.
- Perilaku `replaceItemAt` terhadap izin dan atribut berkas (R-10), tampilnya `NSProgress` di Finder untuk berkas yang baru dibuat, tampilan `NSDockTile` dengan `contentView`, pengiriman `UNUserNotificationCenter` dari bundel ber-tanda-tangan ad-hoc, dan pengecualian saat `UNUserNotificationCenter.current()` dipanggil tanpa bundel (§8).
- Apakah `SET SESSION`/`USE` bertahan di dalam skrip Trino (R-14).
- Teks keputusan terbuka tentang `walk` yang bisa dilanjutkan yang disebut `AGENTS.md` (F-1): tidak ditemukan di ledger, dan blueprint ini tidak bergantung padanya.
- Angka waktu: formatter 1 MB, 12 pembacaan atas 4 MiB, penguras 1,5 juta baris. Semuanya hanya diukur di T1 dan T3 dan dicatat, bukan gerbang.

## 14. Untuk pemeriksa dan untuk ADR-0041, ADR-0042

**Pemeriksa (AR, SEC, DB, SF, UX, AX, TD) perlu memutuskan:**

1. **D-1 dan D-2 (RR, DB).** Apakah kisi spasi yang konservatif (tanpa normalisasi spasi operator) dan penolakan atas pembacaan berselisih bisa diterima sebagai v1, dan apakah `Generic` = gabungan 12 pembacaan benar.
2. **D-9 (SEC, DB).** Konfirmasi skrip bersumber dari `plan` dengan digest teks, dan hubungannya dengan ADR-0026 ("satu konfirmasi tidak menutupi rencana"): apakah "pengguna melihat setiap statement yang dicakup" cukup membedakannya dari impor.
3. **D-8 (DB, SEC).** Satu klasifikasi di pintu tanpa klasifikasi kedua per statement, dan perlakuan `full` saat pembacaan berselisih.
4. **D-11 (DB, SF).** Membaca statement berbaris sampai habis, dan mencegah cancel-lalu-lanjut di sesi yang sama (F-10).
5. **D-12 (AR, TD).** `ResultSetSink` dan `run_script` di samping `run_with_store`; cadangan `claim_result`.
6. **D-13 (SF).** `unknown`, `open_transaction`, dan `Err(Warned)` sesudah `done`.
7. **D-15 (pemilik).** Kasus keempat fallback in-memory di luar O-8. **Koreksi AR (B4):** keputusan ini menggerbangi W12-T4. Pemilik memilih (a) in-memory saja atau (b) sort dan search dinonaktifkan untuk hasil skrip; menjalankan ulang ke server dicoret karena salah dan tidak aman (§5.2). Jawabannya dicatat di `target/run/ledger.md` sebelum T4.
8. **D-20 dan D-21 (DB, SF).** `RawRow.cells: Vec<Option<String>>`, kunci tak terpetakan sebagai peringatan, verifikasi `name`, dan `IMPORT_PREVIEW`.
9. **D-22 (UX, AX).** Notifikasi tanpa isi sensitif, cadangan perhatian Dock, dan batas progres Finder (satu berkas).
10. **Pembagian tugas.** T3 memegang `host.rs`/`events.rs`/`commands.rs` yang tidak ada di daftar berkasnya (§10), dan T5 menyentuh `Cargo.lock` di luar rantainya.

**ADR-0041 (Formatter, W12-D)** memuat: kontrak byte dan invarian I-F1…I-F6 (§3.1); kisi spasi dan alasannya (D-2); pembacaan gabungan dan `Ambiguous`/`Unterminated` (D-1); daftar kata klausa dan bingkai query (D-3); kasus kata kunci sebagai langkah terpisah (D-4); toggle komentar dan penolakan baris di dalam region (D-5); pemetaan seleksi dan pengiriman penggantian terkecil (D-6); dan yang sengaja tidak ada di v1.

**ADR-0042 (Script runner dan result set berganda, W12-D)** memuat: rute `LongOp`, tanpa transaksi implisit dan tanpa retry statement (D-7), dengan buktinya: `execute_until_stopped` membungkus `retry::execute` (`commands.rs:1418-1437`), sehingga `script` wajib meneruskan `RetryPolicy::new(0, …)` secara eksplisit dan tidak pernah `RetryPolicy::from_settings` (Koreksi AR, B2); pintu satu klasifikasi (D-8); alur `plan` dan digest (D-9); protokol event (§4.2) dan `Err(Warned)` (D-10); statement berbaris dikuras dengan `LIMIT` penyimpanan (D-11); `ResultSetSink` dan `run_script` (D-12); hasil `unknown` dan `open_transaction` (D-13); Stop dua titik (D-14); model hasil, pin, dan kepemilikan store (D-15, §5.1), serta keputusan pemilik atas sort dan search hasil skrip (§5.2); satu entri history (D-16); batas yang dinyatakan: Trino, `continue` di dalam transaksi, timeout per statement, dan keluarnya `script` dari MCP.

## Verdict architect-reviewer

**Verdict: koreksi blokir diterapkan, menunggu pemeriksaan ulang (pending review, O-20: satu putaran sudah dipakai).** 6 Okt 2026, W12-A1. Empat temuan memblokir (B1 sampai B4) sudah ditulis ke badan blueprint dan ditandai "Koreksi AR". B1 sampai B3 tidak menunggu siapa pun. **B4 menunggu pemilik: W12-T4 tidak dibuka sebelum jawaban atas D-15 tercatat di `target/run/ledger.md`.** Temuan tidak memblokir dicatat di bawah dan **belum ditulis ke badan** (brief koreksi hanya mengizinkan B1 sampai B4); implementer tugas terkait menerapkannya saat membuka tugas (aturan pembuka §11) dan menyebutnya di pesan commit.

### Temuan memblokir (sudah diterapkan)

| ID | Temuan | Bukti | Ditulis di |
|---|---|---|---|
| B1 | **Formatter dan toggle komentar menolak SQL sah (salah membaca `walk`).** §3.2 langkah 1 dan D-1 memetakan setiap `EndState::Open` ke `Unterminated`, padahal `walk` mengembalikan `Open(LineComment)` untuk komentar baris tanpa baris baru sesudahnya. Dokumen atau irisan statement yang berakhir `-- note` tidak bisa diformat. Aturan §3.3 "baris yang mulai atau berakhir di dalam region" menolak `-- x` (awal baris = awal region) dan `SELECT 1 -- x` (akhir baris = akhir region), sehingga membuka komentar dan menutup baris yang berakhir komentar sama-sama ditolak. D-5 hanya menyebut string, identifier berkutip, komentar blok, dan dollar-quote, jadi §3.3 juga bertentangan dengan D-5. | `scan.rs:418-426` (dokumen `EndState::Open`: "a line comment with no newline after it"), `:695-701` (`walk` melaporkan `Open(region_kind)` di akhir teks) | D-1, D-5, §3.1 (`Unterminated`, I-F2), §3.2 langkah 1 dan 4, §3.3, tes bernama di §12 T1 |
| B2 | **Klaim salah tentang kode, berisiko tulis ganda.** D-7 dan §4.1 langkah 4 menyatakan `execute_until_stopped` berjalan "tanpa `retry::execute`". Ia justru membungkusnya. `retry::execute` mengirim ulang pada `Transient` dan mengembalikan `RetryCursor` yang mengulang permintaan halaman; jumlah ulangnya dari `RETRIES` lewat `from_settings`, dan `open` mengembalikan policy itu. Implementer yang membangun policy seperti perintah lain akan mengirim ulang tulis sesudah koneksi putus, yang dilarang D-7. | `commands.rs:1418-1437`, `retry.rs:235-262`, `:312-343`, `:155-157`, `commands.rs:660-672` | D-7, §4.1 langkah 3 dan 4, ADR-0042 (§14), tes T3 di §12 (kasus `RETRIES=3`) |
| B3 | **Umur store kurang ditentukan dan bertabrakan dengan jalur lepas yang ada: hasil ter-pin hilang.** D-15 dan §5 memasang store terpilih sebagai `activeResult` dan `baseResult = slot` tanpa aturan pemilik. `releaseResults()` melepas keduanya tanpa memeriksa pemilik, dipanggil di setiap Run preview, explain, dan penutupan tab; `showRows` melepas `activeResult` yang digantikan. Hasil ter-pin yang terpilih saat Run berikutnya mulai ikut di-`release()`, sehingga FR-RUN-02 ("pin supaya bertahan saat Run ulang") gagal, dan berpindah hasil lewat jalur pasang yang ada menghancurkan hasil sebelumnya. | `QueryTab.swift:885-893`, `:900-902`; `AppModel.swift:717`, `:2469`, `:3008`; `StoreRows.swift:588-599` (`release()` idempoten, `deinit` memanggilnya juga) | D-15, §5 (tiga butir), §5.1 baru, tabel T4 di §9, §10, tes T4 di §12 |
| B4 | **Bertentangan dengan O-8, pemilik harus memutuskan sebelum T4.** D-15 menjadikan hasil skrip fallback in-memory keempat di luar tiga kasus O-8. Blueprint sudah mengeskalasinya (§10, §14 butir 7), tetapi T4 tidak boleh mulai sebelum jawabannya ada, dan alasannya harus lebih kuat: menjalankan ulang satu statement ke server untuk sort atau search bukan hanya salah, tetapi tidak aman. Untuk skrip berisi `INSERT/UPDATE … RETURNING` atau `CALL`, sort server mengirim tulisan itu lagi. Pengecekan pemeriksa atas kode: penjaga membaca setiap kata tulis di dalam statement, jadi sebagian bentuk akan terklasifikasi menulis, tetapi di mode `full` atau setelah satu klik konfirmasi tulisan berjalan lagi, dan `SELECT` yang memanggil fungsi penulis tak terlihat penjaga (batas yang ia nyatakan sendiri). | PRD §11 O-8; `performance-plan.md` §7 butir 3; `ServerSort.swift:50`; `classify.rs:46-48`, `:499-515` | D-15, §5.2 baru, §10, §11 (urutan T4), §14 butir 7 |

### Temuan tidak memblokir (belum ditulis ke badan)

1. **Jumlah perintah usang.** §4.6, §12 T3, dan tabel T3 menyebut `COMMANDS`/`EVERY_COMMAND` = 27. Pohon kerja punya 26 (`crates/qh-ffi/src/lib.rs:462`, `COMMANDS: [&str; 26]`); W11-T1 menambah `columns`, `ddl`, `execution_log` sehingga 29 (`blueprints/w11-metadata-and-connections.md:172`, `:344`). W12 dibangun sesudah W11, jadi 30, dengan `script` disisipkan di depan `objects` setelah tambahan W11. Kompiler menangkap selisihnya, tetapi teks dan ekspektasi `RustEngineTests` (kesetaraan 27 nama) salah.
2. **F-9 "sebelas nama" kurang dua.** `source_sql` (`commands.rs:498`) dan `columns_json` (`:619`) juga privat dan dibutuhkan `script.rs` (target hitung tanpa store tetap harus memancarkan `columns`). Menjadi tiga belas nama, tetap perubahan visibilitas saja pada `commands.rs`.
3. **R-6 "progres `rows` terus bergerak" tidak benar.** `StoreTarget` memancarkan `progress` hanya pada baris yang ia terima (`commands.rs:1985-1992`). `CappedTarget` menelan semua baris sesudah `LIMIT`, dan jalur tanpa store (CLI, golden) tidak punya progres sama sekali. Statement 1,5 juta baris diam sesudah 1.000 baris pertama. `CappedTarget` harus memancarkan `statement{progress}` sendiri, di-throttle, membawa `seen`. Perbarui §4.2 dan R-6.
4. **D-9/§4.2: batas 1.000 entri `plan.confirm` dengan `confirm_truncated` bertentangan dengan alasan D-9** ("pengguna melihat setiap statement yang dicakup"). SEC memutuskan: (a) menolak mengonfirmasi plan yang terpotong, atau (b) lembar wajib menulis "and N more writes" dan jaminan yang lebih lemah diterima dan dicatat di ADR-0042.
5. **D-4, kasus kata kunci tidak netral di MySQL.** `is_keyword` berasal dari set `keyword_*` grammar ditambah `EXTRA_KEYWORDS` (`crates/qh-editor/src/keywords.rs:13-52`) dan mencakup kata yang lazim dipakai sebagai nama (`user`, `status`, `type`). Mengubahnya ke huruf besar pada MySQL dengan `lower_case_table_names=0` (nama tabel peka kapital) mengubah arti walau I-K1 (sama persis kecuali kasus huruf) lolos. Pilih: lewati kata pada posisi nama (sesudah `FROM`, `JOIN`, `INTO`, `UPDATE`, dan `.`) atau lewati dialek MySQL; catat pilihannya di ADR-0041.
6. **Berpindah hasil dan view yang menempel pada store.** `StoreRows.apply` mengubah view store itu di tempat (`QueryTab.swift:829-850`; `StoreRows.swift:291`). Bila `viewSpec` dan `activeSort` tab di-reset saat berpindah sementara store sebelumnya mempertahankan view-nya, kembali ke hasil itu menampilkan baris terurut tanpa indikator. Tentukan salah satu: `viewSpec` disimpan per hasil, atau spec kosong diterapkan saat berpindah (menyentuh §5.1 butir 3).
7. **D-19, asal `FileBinding.lastKnown` saat restore.** Isi dari `fileHash` yang tersimpan di sesi dengan mtime dan ukuran tidak diketahui, sehingga aktivasi pertama memaksa satu perbandingan isi. Bila diisi dari `stat` disk saat restore, pemeriksaan murah mtime/ukuran melewatkan edit yang terjadi selagi app tertutup. Pemeriksaan SHA sebelum simpan tetap mencegah penimpaan, jadi ini bukan risiko kehilangan data.
8. **Fakta PostgreSQL untuk ADR-0042 dan R-13.** Driver men-`prepare` setiap statement sebelum `simple_query_raw` (`qh-driver-postgres/src/lib.rs:542-567`) dan mengirim `SET statement_timeout` hanya bila nilainya berubah (`:533`, `:568`). Akibatnya: `SET statement_timeout = 0` di dalam skrip diam-diam mencabut batas 60 dtk app untuk statement sesudahnya, karena driver mengira nilainya masih berlaku; dan di bawah `continue`, describe setiap statement gagal di dalam transaksi yang sudah aborted.
9. **I-F2/I-F3 (§3.1) dan pemeriksaan sendiri "`walk` atas hasil"** harus dijalankan di bawah **setiap** pembacaan di `readings`, bukan hanya yang pertama; kalau tidak satu pembacaan lolos sementara yang lain berubah.
10. **Biaya D-1 yang harus tampil di salinan UX.** Dengan gabungan 12 pembacaan, tab `Generic` selalu mendapat `Ambiguous` untuk backtick (kutip di MySQL, tanda baca di PostgreSQL dan Trino), badan `$$`, dan `#`. Dapat diterima untuk v1, tetapi pengguna akan melihatnya; pesan galat perlu menyebut penyebab dan jalan keluarnya (pilih dialek tab, W10-T6).
11. **Penjadwalan T6.** Ledger O-22 menarik W12-T6 lebih awal ke lane WT-S, sedangkan T6 di blueprint ini mengaitkan `runScript` di `AppModel+Run.swift` yang baru ada sesudah T4. Pecah: **T6a** (ekspor, `to_table`, preview: `LongRunNotifier`, `FileProgress`, `AppModel+Export`, hook di `runPreview`) lebih awal, dan **T6b** (hook `runScript`) bersama atau sesudah T4.
12. **§10, berkas `Cargo.toml` milik T5.** Nyatakan apakah suntingannya pada manifes root workspace (rantai W7-T5 → W13-T8a) atau `crates/qh-import/Cargo.toml`. Rencana hanya butuh manifes crate ditambah `Cargo.lock` root.
13. **Akurasi kecil (nomor baris bergeser; substansi tidak berubah).** F-15: `.runScript` punya pemanggil, `App.swift:111-112` (`.keyboardShortcut(model.shortcut(for: .runScript))`), jadi "tidak ada pemanggil selain daftar di Settings" tinggal berlaku untuk `.saveFile`, `.commentLine`, dan `.format` (grep `shortcut(for:` di `app/Sources`). F-3: `crates/qh-editor/src` juga memuat `syntax.rs`. F-18: `Event` kini mulai di `App.swift:164`.

### Klaim yang diperiksa dan benar (tanpa perubahan)

- **Engine host.** `route()` tanpa wildcard (`host.rs:224-249`); `run_with_store` hanya menerima `preview` dan `explain` (`host.rs:514`); `claim_run` (`store_api.rs:307-309`); `EventSink` (`uniffi_api.rs:219-222`); `Emitter` dan `StoreEmitter` (`events.rs`).
- **Mesin baca di `commands.rs`.** `record_decision` privat (`:255`); `STOP_BUDGET`, `until_stopped`, `stop_grace`, `stop_session` (`:1395-1481`); komentar cancel telat (`:1763-1768`); `StoreTarget::begin` menolak lebar nol (`:1939`); `pump_loop` memperlakukan `Released` sebagai cancel (`:2183`).
- **`qh-sql`.** `ScriptStatement` dan `statements_with_lines_dialect` sejajar dengan indeks `decisions_readings` (`classify.rs:691-851`); pembacaan Generic 1, MySQL 6, PostgreSQL 4, Trino 1 (`scan.rs:207-250`); `lex` kehilangan byte (`lex.rs:45-120`).
- **Digest dan impor.** `hash_statement` (`qh-storage/src/execution_log.rs:77`, `lib.rs:42`); `RawRow` dan `Format` (`qh-import/src/lib.rs:44-85`); `serde_json` 1.0.151 dengan `preserve_order`; `import.rs:466`, `:482`, `:306`, `:1053`, `:1088`; `ImportMapping` hanya mengirim `NULL_TEXT` bila tidak kosong (`ImportMapping.swift:164`), yang mendukung bawaan `None` pada D-20.
- **Ekspor, history, sesi.** Penggantian nama bagian pertama ekspor (`qh-export/src/plan.rs:18-26`, `:57-67`); kunci `history_add` dan hasil ok/error/cancelled; kunci `SessionTab` tanpa garis bawah.
- **Diverifikasi ulang saat koreksi ditulis** (dibaca di pohon kerja, bukan hanya dari laporan pemeriksa): semua bukti B1 sampai B3, `ServerSort.swift:50`, `classify.rs:46-48` dan `:499-515`, `COMMANDS` = 26, `source_sql` dan `columns_json` privat, urutan `prepare` lalu `simple_query_raw` dan `SET statement_timeout` bersyarat di driver PostgreSQL, `App.swift:111-112` dan `:164`, dan `syntax.rs`.

### Perubahan rencana untuk orkestrator

1. **T4 digerbang jawaban pemilik atas D-15** (§5.2). Pilihan (a) in-memory saja, atau (b) sort dan search dinonaktifkan untuk hasil skrip. Catat di ledger sebagai nomor O baru sebelum T4 dibuka; bila (a), perbarui juga `performance-plan.md` §7 butir 3.
2. **T6 dipecah** menjadi T6a (awal, lane WT-S) dan T6b (bersama atau sesudah T4) menurut temuan 11.
3. **Kepemilikan berkas bertambah:** T4 menyentuh satu baris `closeTab` di berkas yang memuatnya (B3, §10), dan T3 memegang tiga belas nama visibilitas di `commands.rs`, bukan sebelas (temuan 2).
4. **Pemeriksaan ulang** koreksi B1 sampai B3 dicatat "pending review" di ledger menurut O-20, dengan gerbang tes bernama di §12 (T1, T3, T4) sebagai penjaganya; tidak ada putaran ketiga.
5. **Risiko terbuka yang tidak memblokir:** temuan 3 sampai 10 dan 13 di atas. Temuan 4 (SEC) dan 5 (DB, RR) perlu putusan pemeriksa terkait sebelum T3 dan T1 selesai.
