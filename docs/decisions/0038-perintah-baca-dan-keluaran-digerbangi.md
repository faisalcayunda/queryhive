# 0038 — Perintah baca baru dan keluaran yang digerbangi setelan (metadata)

- **Status:** Diterima. Implementasi metadata (`columns`, `ddl`, `execution_log`, `OBJECT_KINDS=1`) mendarat di commit `1dc81ba` (W11-T1). Tinjauan pertama menunjukkan dua masalah blocking terkait tes live (snapshot menulis ke database pengguna; yang pertama kali connect membayar biaya pembukaan dan migrasi) diperbaiki dengan tes yang diuji mutasi. Tes safe-mode, golden, dan golden live berjalan; penyensoran DDL terverifikasi.
- **Tanggal:** 6 Okt 2026 (W11-D, perf-parity Fase 11)
- **Konteks instruksi:** `docs/architecture/development-plan.md` §4 (jadwal ADR, baris 0038) dan W11-D; `docs/architecture/blueprints/w11-metadata-and-connections.md` §2, §3, §4, dan §23; PRD `docs/architecture/prd-performance-and-parity.md` P-06, UC-02, UC-15, §5.1; keputusan pemilik O-27a (log eksekusi di `EngineHost`).
- **Tidak menggantikan:** ADR lain. Pendamping: ADR-0031 (pool dan `EngineHost`), ADR-0017 dan ADR-0026 (Safe Mode tes `RecordingSession`).

## Konteks

Engine QueryHive belum menyediakan metadata detail: hanya nama tabel saja dari `Session::browse`, tanpa kolom, jenis objek, atau DDL. Aplikasi tidak bisa menampilkan schema tersembunyi, objek view terpisah dari tabel, atau membantu pengguna memahami struktur database asing. FR-TREE-01…03 dan FR-CON-01 menuntut tiga perintah baca (`columns`, `ddl`, `table_entries` dengan jenis), dan FR-SAFE-02 menuntut log eksekusi yang terverifikasi untuk audit koneksi terpercaya. P-06 menetapkan gerbang setelan untuk `OBJECT_KINDS` supaya golden dan CLI tetap beku.

Rangkaian temuan dalam blueprint menunjukkan satu-satunya jalan yang melewati tes Safe Mode tanpa memodifikasi dua puluh implementer `Session` adalah menjalankan SQL metadata melalui jalur yang sama dengan perintah lain: `Session::execute`, bukan metode `Session` baru. Hasil diparsing per driver di `Driver::metadata()`, dan pembaca log dipasang di `EngineHost` malas per `DB_PATH`.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Sumber SQL metadata | **Fungsi murni di crate driver (`qh_driver::MetadataSql`), dijalankan engine lewat `Session::execute`** | Tes Safe Mode melihat statement; `Held` (sesi pool) tidak disentuh; Satu kontrak untuk setiap driver. SQL bisa dipatok tes tanpa server | Satu mesin langkah kecil di engine untuk menjalankan berbeda dengan perintah lain |
| | Metode `Session` baru | Cakupan sudah `execute`; tidak ada deduplikasi kode | `RecordingSession` tes palsu tidak melihatnya; belasan implementer butuh perubahan bawaan |
| Bentuk keluaran jenis | **Larik paralel `kinds` di event `tables`, hanya bila `OBJECT_KINDS=1`** | Golden dan CLI beku; decoder lama membaca `names` saja; FFI tidak berubah | Dua larik harus urutan sama dan panjang sama |
| | Larik objek struktural | Menjaga setiap kolom | Jalur dekode pohon berubah; versi schema event naik |
| Pembacaan execution log | **Perintah lokal (`Route::Local`), pembaca dan penulis melalui pintu `ensure_sink(settings)` yang berkunci `DB_PATH`** | Pembaca dan penulis melihat database sama; malas per proses; fail-closed | Dua run serentak dengan `DB_PATH` berbeda dalam satu proses saling membalik sink (jarang, cuma di tes) |
| | Sink global per app | Tidak perlu kunci | App tidak pernah dipasang, satu database per app; tidak bisa diuji tes dengan berbagai `DB_PATH` |
| Gerbang `OBJECT_KINDS` | **P-06: setelan `OBJECT_KINDS=1` mengisi larik `kinds`, `0` membiarkan event `tables` tanpa kunci itu** | Kompatibel mundur; output CLI tidak berubah kecuali setelan; daftar pengecualian di blueprint milik tes app, bukan engine | Gerbang itu satu-satunya; perintah metadata baru tidak bisa dimatikan per perintah |

## Keputusan

**Tiga perintah baca baru dijalankan melalui `Session::execute` dengan SQL yang ditulis di crate driver sebagai fungsi murni. Event baru memakai kunci yang belum terpakai di `Event`. Jenis objek lewat larik paralel `kinds` di event `tables`, hanya bila `OBJECT_KINDS=1`. Pembaca execution log berkunci pada `DB_PATH` dan dipasang malas di `EngineHost` saat run pertama.**

Keputusan desain yang mengikat, setiap klaim "sudah dibangun" dikutip dari commit `1dc81ba`:

1. **SQL metadata milik driver.** Tiga trait di `crates/qh-driver/src/metadata.rs` (baru): `MetadataSql` dengan tiga metode (`table_entries`, `columns`, `ddl`), `DdlRecipe` untuk resep bertahap (`next(previous) -> Step`), dan dua struct hasil (`ColumnInfo` dengan nama, tipe, nullable, default, extra; `ObjectDdl` dengan kind, teks, dan flag truncated). `Driver::metadata() -> Option<&dyn MetadataSql>` bawaan `None`, dan setiap driver `::{postgres,mysql,trino}/src/metadata.rs` (baru) mengimplementasikan-nya.
2. **Perintah di engine dijalankan lewat `Session::execute`.** `run_text(session, policy, dialect, sql, limit, timeout)` di `crates/qh-ffi/src/metadata.rs` (baru) menjalankan lewat `retry::execute` (sehingga `RETRIES` dihormati). Setiap perintah (`commands`, `columns`, `ddl`, `execution_log`) memakai `run_text` atau langsung membaca storage lokal. Rute perintah: `columns` dan `ddl` lewat `Pooled(Lane::Metadata)`, `execution_log` lewat `Local`. Tidak ada lane baru `Metadata` di driver maupun app; lane itu hanya untuk pool di engine agar sesi metadata tidak menghilangkan koneksi database reguler.
3. **Jenis objek lewat larik paralel.** Event `tables` memperoleh kunci `kinds: [String]` hanya bila `OBJECT_KINDS=1`. Panjang dan urutan sama dengan `names`. Nilai per driver: `table`, `view`, `materialized_view`, `foreign_table`, atau `null` bila driver tidak tahu. Cites file:line: `crates/qh-driver-postgres/src/metadata.rs:195-203` (SELECT dari `pg_class` dengan `relkind`), `crates/qh-driver-mysql/src/metadata.rs:275` (SHOW FULL TABLES menjawab jenis di kolom indeks 1), `crates/qh-driver-trino/src/metadata.rs:287-290` (SELECT dari `information_schema.tables` dengan kolom `table_type`).
4. **Kolom metadata:** name (teks), type (teks query server, mis. `numeric(10,2)`), nullable (Ya/Tidak atau null), default (nilai literal atau ekspresi, atau null), extra (teks: `identity`, `generated`, `auto_increment`, atau kosong). Cites: `crates/qh-driver-postgres/src/metadata.rs:212-223` (SELECT dari `pg_attribute` dengan `format_type`, `pg_get_expr`); `crates/qh-driver-mysql/src/metadata.rs:276-277` (SELECT dari `information_schema.COLUMNS`); `crates/qh-driver-trino/src/metadata.rs:292-293` (SELECT dari `information_schema.columns`).
5. **DDL disusun atau diambil** per driver dengan batas: MySQL dan Trino menggunakan `SHOW CREATE {TABLE|VIEW}`; PostgreSQL menyusun dari katalog dengan `pg_get_expr`, `quote_ident`, `pg_get_constraintdef` (resep bertahap di `crates/qh-driver-postgres/src/metadata.rs:227-268`). Setiap DDL mencetakkan kalimat: "Reconstructed from the catalog by QueryHive" (PostgreSQL) atau tipe objek pembuat (MySQL, Trino).
6. **Penyensoran kredensial di DDL.** Tabel MySQL ber-engine FEDERATED dan `CREATE FOREIGN TABLE` PostgreSQL bisa memuat password. Penyensoran di sisi engine sebelum event dikirim (cites: `crates/qh-ffi/src/metadata.rs`, penyensoran `CONNECTION='…'` dan opsi `OPTIONS (password='…')` menjadi `***`), sehingga tab app dan output MCP sama. `redacted: true` di event.
7. **Execution log** dibaca dari storage lokal dengan verifikasi SHA-256 per baris (sudah ada `Storage::execution_log` dan `verify_execution_log`). Perintah lokal di `crates/qh-ffi/src/metadata.rs`. Sink dipasang di `EngineHost::ensure_sink(settings)` (baru, T1), berkunci `DB_PATH` yang divalidasi seperti `local::open_storage`. Malas pada run pertama, dipasang ulang bila `DB_PATH` berubah antar run dalam satu proses (jarang, cuma di tes; tes yang berjalan serentak dengan `DB_PATH` berbeda tidak boleh mengandalkan log).
8. **Rute baru di `route()`.** `Columns | Ddl` ke `Pooled(Lane::Metadata)`, `ExecutionLog` ke `Local`. Invariant 11 (empat daftar COMMANDS, EngineCommand, EVERY_COMMAND, RustEngine.commands) semuanya 29 elemen setelah T1 (ada: 26, tambah: 3). Cites: `crates/qh-ffi/src/lib.rs:453-461` (urutan COMMANDS, jadi perintah baru hanya sebelum `objects`); `crates/qh-ffi/src/uniffi_api.rs:95-169` (EngineCommand dan EVERY_COMMAND); `app/Sources/QueryHive/Support/RustEngine.swift` (RustEngine.commands bernilai tiga kata baru).
9. **Event baru.** Kunci yang ditambahkan satu kali oleh T1: event `tables` mendapat `kinds`; event baru `table_columns` dengan `object` (tiga slot `catalog`, `schema`, `table`), `fields` (array ColumnInfo), dan `truncated`; event baru `table_ddl` dengan `object`, `object_kind`, `ddl`, `truncated`, `redacted`; event baru `execution_log` dengan `decisions` (array dengan seq, id, timestamp, safe_mode, decision, reason) dan `chain` (verified boolean dan seq atau detail). Cites: `app/Generated/QueryHiveFFI/qh_ffi.swift` (Event struct, properti bahasa Swift untuk snake_case engine: `objectKind`, `tableColumns`, `tableDdl`); `app/Tests/QueryHiveTests/EventDecodingTests.swift`.
10. **Setelan baru** D-9 di blueprint: `OBJECT_KINDS` (bawaan 0), `TARGET_CATALOG`, `TARGET_SCHEMA`, `TARGET_TABLE` (sudah ada, dari `sql_ident`), `EXECUTION_LOG_LIMIT` (bawaan 200, maksimal 5000), `EXECUTION_LOG_VERIFY` (bawaan 1). Cites: `crates/qh-ffi/src/env.rs` untuk definisi; `crates/qh-ffi/src/metadata.rs` dan `execution_log.rs` untuk penggunaan.

## Alasan

1. **Jalur `Session::execute` menutup lubang Safe Mode tes tanpa mengubah belasan implementer `Session`.** `RecordingSession` di tes palsu melihat setiap statement yang lewat `execute`. Metode `Session` baru akan diam-diam menjawab "tidak didukung" melalui sesi pool yang tidak mengerti mereka.
2. **Jenis objek lewat larik paralel, bukan objek struktural, menjaga jalur dekode pohon yang ada.** Dekoder app tidak perlu berubah, golden tetap beku.
3. **Sink execution log malas per `DB_PATH` adalah satu-satunya jalan agar tes dengan berbagai database tidak saling mengacaukan catatan.** Tes swift (satu proses) dan CLI (multiproses) kedua memakai `TestIsolation.dbPath` per tes.
4. **Gerbang `OBJECT_KINDS` memenuhi P-06**: keluaran CLI dan dekode event lama tidak berubah kecuali setelan itu diisi.
5. **Penyensoran berkode di engine membuat keluaran tab app dan MCP sama** tanpa duplikasi.

## Konsekuensi

### Positif

- **POS-001.** Engine menyediakan skema lengkap (kolom, jenis, DDL) untuk pohon dan editor tanpa permintaan khusus dari setiap driver.
- **POS-002.** Log eksekusi terverifikasi tersedia untuk FR-SAFE-02 (audit koneksi terpercaya) dengan rantai SHA-256.
- **POS-003.** Jenis objek (tabel, view, materialized view, foreign table) bisa ditampilkan pohon pohon terpisah per jenis (V-11).
- **POS-004.** Metadata SQL lewat `Session::execute` membuat setiap tes Safe Mode otomatis melihat statement itu tanpa tes tambahan per metode.

### Negatif

- **NEG-001.** Kolom `execution_log` tabel tidak memuat `connection_id`, jadi filter per koneksi tidak bisa dipenuhi tanpa migrasi versi rantai (keputusan untuk W13, §3.5 blueprint).
- **NEG-002.** Penyensoran DDL adalah best-effort: definisi view bisa memuat literal sensitif yang ditulis pemiliknya, tidak ada aturan yang menangkapnya (sama dengan `preview`).
- **NEG-003.** `OBJECT_KINDS=1` hanya di PostgreSQL mengubah himpunan (view dan materialized_view bertambah); MySQL dan Trino sudah memuat view sebelumnya. Paritas visual tergantung setelan (V-11 terdaftar, baseline direkam ulang).

### Belum ada (kontrak, bukan kode)

- Sink tidak terpasang di app saat ini: rute `EngineHost` di app belum ada. `EngineHost` di engine (CLI dan MCP) hanya di `main.rs:66` (CLI) dan `bin/mcp.rs:91` (MCP).
- Koneksi di MCP: pemetaan field SSH, JWT, dan CA (D-18 blueprint) dari `connections.json` ke `options_json` belum dijalankan (berkas `crates/qh-storage/src/import.rs` tidak ada di daftar T1; seharusnya ditambahkan T3, §10.1 blueprint).
- Two tool MCP baru `describe_table` dan `table_ddl` belum dibangun (T5 W11).

## Bukti

- Kode: `crates/qh-driver/src/metadata.rs` (trait dan struct), `crates/qh-driver-{postgres,mysql,trino}/src/metadata.rs` (SQL per driver), `crates/qh-ffi/src/metadata.rs` (engine), `crates/qh-ffi/src/execution_log.rs` (sink), `crates/qh-ffi/src/host/engine_host.rs` (T1, baris `ensure_sink`).
- Commit: `1dc81ba` (W11-T1, metadata dan log).
- Gate yang tercatat: G-RUST 1296/0/2, G-FFI, G-SWIFT 810/6/0, G-GOLDEN (9 kasus live baru, 20/31 cocok), G-LIVE (PostgreSQL 80, MySQL 90, Trino 82).
- Blueprint: `docs/architecture/blueprints/w11-metadata-and-connections.md` §2, §3, §4.

## Referensi

- ADR-0017, ADR-0026 (Safe Mode tes lewat `RecordingSession`), ADR-0031 (EngineHost dan pool).
- `docs/architecture/blueprints/w11-metadata-and-connections.md` keputusan D-1…D-9, §3 (SQL), §4 (setelan), §10.2 (MCP).
- `docs/architecture/prd-performance-and-parity.md` P-06, UC-02, UC-15, FR-TREE-01…03, FR-SAFE-02.
- Tugas penerus: T3 (pemetaan field SSH dan JWT ke `options_json`), T5 (dua tool MCP), W13-T6 (filter log per koneksi dengan versi rantai baru).
