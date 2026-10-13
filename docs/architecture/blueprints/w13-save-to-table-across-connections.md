# Blueprint W13-T20: Save to Table lintas koneksi

- **Status:** blueprint tingkat berkas.
  - Ditulis 14 Okt 2026.
  - Direview sekali oleh `architect-reviewer` (opus, putaran 1). Verdict-nya "perubahan diperlukan": tujuh temuan memblokir (B-1..B-7) dan tujuh belas tidak memblokir (N-1..N-17).
  - Direvisi 14 Okt 2026 (revisi 2). Pemetaan temuan ke bagian ada di paragraf "Revisi 2" di akhir.
  - **Menunggu review putaran 2, hanya untuk B-1..B-7.** Belum ada kode.
  - OQ-1..OQ-13 sudah diputuskan orkestrator lewat delegasi pemilik (tabel di Verdict). Keputusannya diterapkan di badan dokumen.
- **Untuk:** W13-T20 (ID usulan, XFER). Permintaan pemilik: "Save to Table" boleh menyeberang koneksi. Query dijalankan di koneksi A, tabelnya dibuat di koneksi B yang **berbeda**. Contohnya hasil Trino disimpan ke PostgreSQL atau MySQL, PostgreSQL ke PostgreSQL, atau MySQL ke PostgreSQL.
- **Sumber:** `AGENTS.md`; `development-plan.md` §1, §4, §5 (W12-T7c, W12-T9), §7; PRD §5.4, §6.4, §6.6, §7, §10; `docs/invariants.md` #1, #7, #11; ADR-0026, ADR-0027, ADR-0028; `blueprints/w13-connection-form.md` sebagai model format; `target/run/keep/ledger.md`.
- **Bukti:**
  - Revisi 2 dibaca ulang dari pohon kerja `main` di `a5fbf11` (14 Okt 2026). Lima commit editor dan perbaikan T-2 belum di-push ke `origin`, dan commit lane editor belum approved.
  - Setiap `path:baris` di bawah benar-benar dibaca, dan berlaku untuk `a5fbf11`.
  - Lane A (`404ee66`), Lane B (`affadcd`..`b5df993`), Lane C (`62c8488`..`bf9fcbb`), dan perbaikan T-2 (`a5fbf11`) sudah mendarat. Tidak satu pun menyentuh `crates/qh-ffi/src/commands.rs`, `lib.rs`, `host.rs`, atau `app/Generated/` (spot-check reviewer di Verdict, ditambah `git show --stat a5fbf11`).
  - Sumber crate pihak ketiga dibaca dari `~/.cargo/registry` pada versi yang dipatok `Cargo.lock`: tokio-postgres 0.7.18, mysql_async 0.36.2, dan mysql_common 0.35.5.
  - Tidak ada build, tes, atau koneksi database yang dijalankan.
- **TablePro (AGPL-3.0) dan dbx (Apache-2.0):** tidak dibaca untuk blueprint ini, dan tidak ada yang disalin.
- **Bahasa:** dokumen Indonesia. Identifier, setelan, nama event, string app, nama tes, dan pesan commit dalam bahasa Inggris.

## Ringkasan

1. **Jalur koneksi yang sama tidak berubah.** CTAS tetap di server, tidak ada baris yang lewat proses ini (`crates/qh-ffi/src/commands.rs:1139`), dan golden `to_table` tetap beku (PR-02, `prd-performance-and-parity.md:327`). App memilih jalur ini bila UUID koneksi tujuan sama dengan sumber.
2. **Lintas koneksi memakai perintah `to_table` yang sama, dan gagal-tertutup.** `to_table` bercabang ke modul baru `crates/qh-ffi/src/transfer/` begitu ada satu saja kunci berprefiks `DEST_`, walaupun nilainya kosong.
   - Sesudah bercabang, setelan tujuan yang tidak lengkap menjadi galat pemakaian. Itu berlaku untuk `DEST_DB_KIND`/`DEST_DB_URL` maupun `DEST_SAFE_MODE` yang wajib.
   - CTAS tidak pernah jalan diam-diam di koneksi sumber (§3.2).
   - Karena tidak ada `EngineCommand` baru, empat daftar invariant #11 tidak disentuh dan `app/Generated/` **tidak berubah**. Penolakan MCP berdasar nama sudah berlaku (`crates/qh-ffi/src/mcp.rs:1000-1003`), begitu pula rute LongOp (`crates/qh-ffi/src/host.rs:251`).
3. **Baris mengalir di proses engine, tidak pernah lewat Swift.** Satu batch dibaca dari kursor A lalu ditulis ke B, berurutan dan tanpa antrean. Backpressure-nya alami, dan memorinya terbatas (§3.4).
4. **Penulis per tujuan.**
   - PostgreSQL: `COPY … FROM STDIN` format teks dalam jendela-jendela (baris, byte, dan waktu), semuanya di dalam satu transaksi.
   - MySQL: INSERT multi-baris dengan parameter terikat. Tabelnya dibuat dengan `DEFAULT CHARSET=utf8mb4`.
   - `LOAD DATA LOCAL` ditolak.
   - Trino sebagai tujuan hanya lewat CTAS koneksi yang sama (§3.5).
5. **Tipe dipetakan dari deskriptor terstruktur yang baru.**
   - Deskriptornya `Cursor::column_types()`, aditif dan di luar event golden (§4.1).
   - Teks tipe dari server (`native`) **tidak pernah** disalin ke SQL. DDL hanya berasal dari tabel milik engine (§4.3).
6. **Setia atau ditolak.**
   - Setiap pasangan tipe hanya punya satu hasil: E, W, T, atau R.
   - Encoder hanya menerima varian `Value` yang diharapkan untuk tipe logisnya. Teks sisa parse (tanggal nol MySQL, `NaN` numeric, `infinity`) punya aturan sendiri.
   - Fallback teks hanya untuk sel T; sel R tetap R (OQ-7). Lihat §4.3 dan §4.4.
7. **Atomik, untuk create dan replace.** Append di luar v1 (OQ-5).
   - PostgreSQL memakai satu transaksi, jadi tidak ada sisa.
   - MySQL memakai scratch `qh_new_*` lalu satu `RENAME TABLE` (§5).
8. **Verifikasi.** Baris yang dibaca dari A harus sama dengan baris yang diakui server B sebelum swap atau `COMMIT` (§6.4).
9. **Cancel sebelum drop di setiap jalan keluar.** Selama kursor sumber hidup, setiap jalan keluar lewat `stop_session(source, Some(cursor))` dulu (`commands.rs:2050-2071`): Stop, galat tujuan, penolakan tipe atau encoder, dan galat internal. Tidak ada `?` di rentang itu (§6.1). Setiap galat membawa `side: "source" | "target"`.
10. **Gerbang keselamatan (tingkat Tinggi).**
    - Sumber wajib tepat satu statement bacaan, dan sesinya read-only di server.
    - Setiap statement **nyata** yang dikirim ke B diputuskan per statement dengan Safe Mode tujuan:
      - DDL lewat semantik `destructive_mode` (`commands.rs:445-459`), jadi create bertanya di `confirm` dan replace ditolak di `confirm`;
      - DML lewat `guard_confirmed`;
      - setiap statement dicek strukturnya sebelum dikirim.
    - Lembar transfer **selalu** tampil, dan menyebut kedua koneksi beserta host-nya (§7).
11. **Setelan kedua memakai prefiks `DEST_`.**
    - Redaksi `SECRET_KEYS` diperluas ke kunci berprefiks (`crates/qh-ffi/src/env.rs:44-52`).
    - `DEST_*` dikupas dari setelan sumber.
    - Sesi B dibuka lewat seam `Engine::for_settings`, dengan `PoolKey`, tunnel, dan passphrase miliknya sendiri (`crates/qh-ffi/src/host/pool.rs:156`).
12. **v1 dan urutan.**
    - Pasangan: PostgreSQL, MySQL, atau Trino sebagai sumber, ke tujuan PostgreSQL atau MySQL, mode create atau replace.
    - Langkah engine 1–7 mendarat dalam keadaan diam, terbukti dengan fake.
    - Verifikasi live (langkah 12) menunggu kontainer.
    - Langkah UI 8–10 tidak masuk `main` sebelum langkah 12 hijau (OQ-11, §13).

## 1. Keadaan sekarang

### 1.1 Pintu masuk di app

- **Tombol footer grid.** `PillButton` "Save to Table" hanya membuka wizard (`model.exportSettingsOpen = true`, `app/Sources/QueryHive/Views/ResultGrid.swift:914-921`).
  - Tombolnya mati bila `runBlockedReason` memberi alasan (`:911`, `:918`).
  - Help-nya masih berbunyi "let Trino write every row into the table" (`:920`).
- **Menu Run.** "Save to Table…" juga hanya membuka wizard (`Views/Workspace.swift:422-424`). Menu memakai alasan blokir yang sama (`Models/AppMenu.swift:106`).
- **Wizard** (`Views/ExportSettings.swift:15-155`):
  - popover lebar 400 (`:129`);
  - segmen File | Table (`:65`);
  - ScrollView setinggi tetap 396 (`:108`);
  - footer berisi Cancel dan HubButton "Save to Table" (`:113-127`).
  - Saat wizard terbuka, catalog dan schema koneksi **tab itu sendiri** dimuat (`:130-133`).
- **`TableTargetFields`** (`ExportSettings.swift:160-229`):
  - field disusun menurut jenis koneksi tab (`:164`, `:176-196`);
  - ada field Table (`:197-199`) dan tiga mode tulis (`:201-223`).
- **Default tujuan.** `prepareTableDestination` mengisi bagian target yang dimiliki driver, lewat `ConnectionKind.targetParts` (`Models/AppModel+Tree.swift:641-654`). Pemicunya `onChange(of: tab.destination)` (`ExportSettings.swift:36-38`).
- **Konfirmasi replace di wizard** (`ExportSettings.swift:39-44`) masih berkata "The existing table is dropped before the query runs. If the query then fails, the table is already gone." Teks itu basi sejak W12-T9 (T-1).

### 1.2 State tab

- **Destination dan mode.** `Destination` (`.file`, `.table`, `Models/QueryTab.swift:88-93`) dan `WriteMode` (`.create`, `.replace`, `.append`, `:96-141`). `statement` dan `help` untuk replace masih menyebut drop-first (`:129`, `:137`).
- **Field tujuan.** `targetCatalog`, `targetSchema`, `targetTable`, `writeMode`, dan `writtenTable` (`:404-409`). **Tidak ada koneksi tujuan**; tab hanya punya `connectionID` (`:378`).
- **Nama dan bagian yang hilang.** `target(for:)` menyusun nama dari `kind.targetParts` (`:1215-1217`). `missingTargetPart(for:)` menyebut bagian pertama yang kosong (`:1221-1223`).
- **`isDestructive`** bernilai benar bila tujuan tabel dan mode replace (`:1226`).
- **`ConnectionKind.targetParts`** (`Models/Connections.swift:61-67`): Trino catalog, schema, dan table; PostgreSQL schema dan table; MySQL catalog dan table.
- **Penyimpanan sesi.** `SessionTab` menyimpan field target dan mode, tanpa koneksi tujuan (`Models/Session.swift:20-38`, `:54-57`, `:81-84`).

### 1.3 AppModel menjalankan

- **`AppModel.run(_:)`** (`Models/AppModel+Export.swift:182-307`):
  - flush editor (`:184`);
  - command `to_table` (`:197`);
  - konfirmasi dari `destinationStatements` dengan Safe Mode **koneksi tab** (`:201-210`);
  - env dari `overrides` (`:213`);
  - `Engine.current.run` (`:255`);
  - event ditangani di `handle` (`:329-359`).
- **Teks Stop untuk tabel:** "Stopped. The coordinator decides what happens to a partly written table." (`:273-274`).
- **`overrides`** (`Models/AppModel+Run.swift:749-787`) mulai dari `connectionEnvironment(connection)`, lalu menambah `TARGET_*` dan `WRITE_MODE` (`:781-784`).
- **`destinationStatements` untuk replace** masih menyusun `DROP TABLE IF EXISTS` lalu `CREATE TABLE … AS` (`AppModel+Run.swift:123-136`, `:130`). Itu bukan rencana engine sekarang (T-1).
- **`runBlockedReason`** hanya meminta bagian yang dimiliki driver (`AppModel+Run.swift:36-50`, `:45-48`). Ini perbaikan T-2 (`a5fbf11`).
- **`connectionEnvironment(connection)`** (`Models/AppModel+Connections.swift:1023-1035`):
  - membaca Keychain dan melempar galat yang menyebut nama koneksi (`:1029`);
  - menambahkan `STATEMENT_TIMEOUT_MS` (`:1033`).

  Kunci yang dikirim ada di `:914-982`. Tidak ada `DB_READ_ONLY`; kunci itu hanya dibaca engine (`commands.rs:133`).
- **Batas statement bawaan app 60 s** (`Models/AppModel.swift:356-358`). Koneksi bisa menimpanya dengan batas sendiri (`Connections.swift:376`).
- **Kirim ke engine.** App meneruskan env apa adanya sebagai pasangan `Setting` (`Support/RustEngine.swift:137`).
- **History.** Export dan `to_table` tidak menulis history; hanya Run (`Models/AppModel+History.swift:60-66`).

### 1.4 Engine: `to_table` dan replace (W12-T9)

- **Masuk dan validasi.** `to_table` (`commands.rs:1149-1206`) membaca SQL, Safe Mode, timeout, dan koneksi, mewajibkan slot target per driver, lalu memvalidasi `WRITE_MODE`.
- **Badan query.** `body = sql.trim().trim_end_matches(';')` (`:1220`). `qh_sql::strip_terminator_dialect` ada di `crates/qh-sql/src/wrap.rs:58`, dan semantiknya dikunci tes Lane A (`404ee66`).
- **Guard.** Statement yang dibangun (`:1233-1237`) dijaga `guard_for` sebelum connect (`:1239-1246`).
- **Tanpa retry tulis.** Hanya connect yang di-retry (`:1248-1253`).
- **Event.** `step connect`, `step write`, `progress`, lalu `done` (`:1248`, `:1259`, `:1322-1340`).
- **Replace** (`commands.rs:1345-1720`):
  - `Replacement::plan` (`:1374-1446`) membangun `build` (`CREATE TABLE {scratch} AS {body}`, `:1405`) dan `drop_scratch` (`:1406`).
  - Swap per driver: PostgreSQL `BEGIN; DROP; ALTER … RENAME; COMMIT` (`:1417-1427`); MySQL satu `RENAME TABLE` lalu `DROP` backup (`:1428-1434`); Trino dua rename dengan undo (`:1435-1443`).
  - Logika jalannya ada di `replace_table` (`:1635-1720`).
  - `crates/qh-ffi/tests/to_table_replace.rs` memuat 14 fungsi tes. `ledger.md:503` mencatat "40 tes", dengan dasar hitung yang tidak bisa dicek.
- **Doc comment basi.** `CliError::Warned` masih menyebut drop-first (`crates/qh-ffi/src/lib.rs:176-179`, T-1).

### 1.5 Progres dan Stop

- **`run_stoppable`** (`commands.rs:1561-1605`) me-race `execute` dan setiap halaman dengan `cancel.cancelled()`. Fungsi itu mengirim `session.cancel()` sebelum menguras kursor; MySQL `KILL QUERY` memakai connection id yang dipublikasikan driver (`:1564-1568`).
- **`stop_session` dan `spawn_stop`** (`commands.rs:2027-2071`) menjalankan cancel, lalu drop kursor, lalu close. Komentar `:2065-2067`: producer MySQL berhenti saat kursornya di-drop, jadi cancel sesudahnya tidak punya apa pun untuk dinamai.
- **Batas.** `cancel_bounded` dan `STOP_CEILING` 10 s (`:1990-2008`). Grace hanya untuk Trino (`:1938-1944`).

### 1.6 Safe Mode

- **Tingkat dan lantai.**
  - `SAFE_MODE` yang absen atau kosong menjadi `full` (`commands.rs:161-172`).
  - Lantai menggabungkan `SAFE_MODE`, `DB_READ_ONLY`, read-only milik driver, dan `SAFE_MODE_FLOOR` (`:124-154`).
  - Dialeknya diturunkan dari koneksi (`:194-201`).
- **`guard_confirmed`** (`:228-247`) mengklasifikasi lewat `qh_sql::decisions_readings`. Di `confirm`, DDL ditolak apa pun konfirmasinya (ADR-0026 §1, `docs/decisions/0026-safe-mode-floor.md:41-55`). `guard_for` mengambil konfirmasi dari `SAFE_MODE_CONFIRMED` (`:298-300`).
- **`guard_destructive` dan `destructive_mode`** (`:407-433`, `:445-459`) adalah satu-satunya jalur yang **bertanya** untuk DDL di `confirm`. Keputusannya dibaca dari **entri** lantai, sehingga `confirm` ditambah lantai `no_ddl` tetap menolak (celah ADR-0027 tertutup).
- **`require_read`** menolak non-bacaan di semua tingkat (`:323-351`).
- **`open`** memasang read-only di server di bawah `read_only` (`:771-788`). Pernyataannya ada di driver PostgreSQL `lib.rs:838-840` dan MySQL `lib.rs:1022-1024`.
- **App:**
  - `RunConfirmation.request` hanya bertanya di `confirm` dan diam bila ada DDL (`Models/RunConfirmation.swift:39-51`, `:41`, `:43`);
  - `destructiveRequest` bertanya di `confirm` dan `full` (`:58-73`).
- **Label lingkungan.** `ConnectionEnvironment` hanya label dan tidak pernah sampai ke engine (`Connections.swift:177-181`). Label koneksi baru default-nya `nil` (`:373`, `:405`).

### 1.7 Host, sesi, dan tunnel

- **Rute.** `to_table` adalah LongOp (`host.rs:251`), dan `engine_for` membangun `pool.long_op_engine(settings.clone())` dengan **satu** `Settings` (`host.rs:617-624`, `pool.rs:454-459`).
- **Satu identitas per engine.**
  - `LongOpEngine::connect` memanggil `open_unleased(config, &self.settings)` (`host.rs:207-210`, `pool.rs:612-620`).
  - `PoolKey::of(config, settings)` mengambil passphrase kunci SSH dari `settings` (`pool.rs:156`).
  - `RealEngine::connect` membuka tunnel dengan `self.settings` (`crates/qh-ffi/src/lib.rs:436-460`).
- **Identitas database di `PoolKey`.** MySQL dengan database dan Trino memakai `DatabaseSlot::Switchable` (`pool.rs:175-178`). Safe Mode tidak masuk identitas.
- **Kunci tunnel.** `tunnel::open` membaca `SSH_KEY_PASSPHRASE`, `SSH_PASSWORD`, `SSH_HOST_KEY_ACCEPT`, dan lainnya dari `settings` (`crates/qh-ffi/src/tunnel.rs:158-341`).
- **Bastion di app.** `HostKeyGate.detailed` hanya melihat `SSH_HOST` (`Support/HostKeyGate.swift:34-39`), dan `report` membangun ulang trust dari env run (`:43-51`).
- **Detail host key di engine.** `host_key` pada `error` digerbangi `SSH_HOST_KEY_DETAIL` (`crates/qh-ffi/src/events.rs:166-171`).

### 1.8 Metadata kolom dan nilai yang tersedia

- **`ColumnMeta`** hanya berisi `name` dan `type_name` (`crates/qh-core/src/batch.rs:37-46`).
- **PostgreSQL:**
  - `type_().name()` tanpa typmod (`crates/qh-driver-postgres/src/lib.rs:1128-1137`), padahal `Column::type_modifier()` ada (`tokio-postgres-0.7.18/src/statement.rs:114`);
  - teks yang tidak ter-parse jatuh ke `Value::Text` (`crates/qh-driver-postgres/src/normalize.rs:123`). Itu termasuk numeric `NaN`/`Infinity` (`:140`), tanggal dan timestamp `infinity` atau SM (`:142-157`).
  - Float `Infinity`/`-Infinity` dimodelkan (`:131-137`).
- **MySQL:**
  - `type_name` membuang presisi, panjang, dan `unsigned` (`crates/qh-driver-mysql/src/normalize.rs:142-175`), dan `binary(16)` bernama "char" (`:171`, `:198-209`);
  - tanggal nol dan tanggal tidak valid menjadi `Value::Text` (`:345-375`, `:541-543`), sesuai aturan driver "an unparseable value keeps its text" (`:22-24`);
  - metadata lengkapnya tersedia lewat `column_length()`, `decimals()`, `flags()`, dan `character_set()` (`mysql_common-0.35.5/src/packets/mod.rs:330-356`).
- **Trino:**
  - teks tipenya lengkap (`crates/qh-driver-trino/src/decode.rs:33-49`, `lib.rs:1634`);
  - kursornya tanpa kolom sampai batch pertama datang (`crates/qh-driver/src/lib.rs:766-771`);
  - pecahan detik lebih dari 6 digit, untuk timestamp **dan** time, menjadi `Value::Unknown` (`decode.rs:486-498`).
- **`Value`.** `Json` disimpan persis seperti dikirim server (`crates/qh-core/src/value.rs:77-82`). Lainnya ada di `:37-97`.
- **Golden.** Mengubah `type_name` berarti mengubah event `columns`, yang dibekukan golden (`crates/qh-ffi/src/lib.rs:14-21`).

### 1.9 Preseden streaming: `export`

- **Alur.** Query penuh dengan `row_limit: None` (`commands.rs:992-1002`), batch pertama diambil sebelum kolom diumumkan (`:1004-1008`), lalu pompa dengan cek cancel per baris (`:1102-1135`). Stop memanggil `stop_session` (`:1073-1078`).
- **Retry baca** (`crates/qh-ffi/src/retry.rs:7-67`):
  - halaman Trino aman di-retry (`:20-28`);
  - PostgreSQL dan MySQL di tengah stream tidak di-retry (`:52-61`);
  - statement tulis tidak pernah di-retry (`:62-67`).

### 1.10 Temuan sampingan

- **T-1. Teks drop-first yang basi** di `ExportSettings.swift:43`, `ResultGrid.swift:930`, `QueryTab.swift:129` dan `:137`, `AppModel+Run.swift:130`, dan `lib.rs:176-179`. Langkah 9 memperbaiki wizard dan `QueryTab.swift`; sisanya masuk backlog pemiliknya.
- **T-2. Diperbaiki di `main` (`a5fbf11`).** Save to Table PostgreSQL dan MySQL dulu tidak bisa diaktifkan, karena `runBlockedReason` meminta catalog **dan** schema untuk setiap driver. Perbaikannya:
  - `ConnectionKind.targetParts` (`Connections.swift:61-67`);
  - `QueryTab.missingTargetPart` (`QueryTab.swift:1221-1223`);
  - tes di `app/Tests/QueryHiveTests/SaveToTableTests.swift`.

  Langkah 8 dibangun di atasnya, memakai **jenis koneksi tujuan**, dan tesnya mulai dari tab baru, karena nilai sisa sesi bisa menyamarkan bug ini.
- **T-3. Dialog mati.** `confirmReplace` di `ResultGrid.swift:16` dan `:926-931` tidak pernah diisi `true`. Backlog.
- **T-4. Impor MySQL dengan literal tertanam** (`import.rs:1849-1861`) salah di `NO_BACKSLASH_ESCAPES`. Probe W12-T9 `sql_literal` (`commands.rs:1480-1488`) juga. Ini catatan untuk W12-T7c dan N-5. Transfer memakai parameter terikat.
- **T-5. PF-23 terukur.** Konsumen lambat terpotong batas statement di server: MySQL setelah 282.600 baris, PostgreSQL setelah 19.400 baris. Perilaku itu dikunci tes dan belum diperbaiki (`target/run/keep/ledger.md:456`). Export terkena juga; transfer menanganinya di §6.6.
- **T-6.** `qualifiedName` dan `quotedIdent` tidak menggandakan kutip yang tertanam (`Models/SchemaTree.swift:225-251`). Ini sudah di backlog ledger, dan transfer tidak memakainya (§7.3).

## 2. Tugas, kepemilikan, dan tabrakan

### 2.1 ID dan posisi

- **ID.** W13-T20, tingkat **Tinggi** (kredensial dua koneksi, Safe Mode, jalur yang bisa kehilangan data, engine host, driver).
- **FR baru:** FR-RUN-08 di PRD §5.4.
- **UC baru:** UC-19.
- **Keputusan pemilik baru:** O-33, yang mengeluarkan fitur ini dari "copy object: Later" (`prd-performance-and-parity.md:368`). Ditambah satu O baru untuk penyusunan ulang rantai driver (OQ-12).
- **ADR baru: ADR-0046.** Nomor 0041–0045 sudah dipesan (`development-plan.md:209-213`). ADR ini ditulis di **langkah 0** (N-13).
- **Perubahan visual: V-17.**

### 2.2 Bentuk permukaan: perluas `to_table`

**Diputuskan: perluas `to_table`.** Alasannya, dibandingkan dengan perintah baru:

- empat daftar invariant #11 tidak berubah (`docs/invariants.md:148-174`);
- `app/Generated/` tidak berubah;
- penolakan MCP berdasar nama dan rute LongOp sudah ada.

**Bentuknya:**

- Satu cabang di awal `to_table`: `if transfer::requested(settings) { return transfer::run(...) }`.
- Doc comment `to_table` ditambah satu kalimat: kontrak "No row crosses this process" (`commands.rs:1139`) berlaku untuk koneksi yang sama.
- `transfer::requested` benar bila ada kunci **apa pun** berprefiks `DEST_` (§3.2, B-1).

### 2.3 Berkas

**Dimiliki W13-T20:**

| Berkas | Isi |
|---|---|
| `crates/qh-ffi/src/transfer/mod.rs` (baru) | `requested`, `run`, urutan langkah, pipa, rentang kursor hidup (§6.1), verifikasi |
| `crates/qh-ffi/src/transfer/types.rs` (baru) | tipe logis, tabel DDL per dialek milik engine, allowlist OID PostgreSQL |
| `crates/qh-ffi/src/transfer/encode.rs` (baru) | encoder `COPY` teks, parameter MySQL, validasi varian dan nilai |
| `crates/qh-ffi/src/transfer/names.rs` (baru) | satu fungsi identifier (§4.6) |
| `crates/qh-ffi/src/transfer/plan.rs` (baru) | statement nyata, keputusan per statement, dan cek struktur (§7.2) |
| `crates/qh-ffi/src/table_swap.rs` (baru) | langkah 1 move-only dari `commands.rs`. Langkah 7 menambah konstruktor yang menerima statement build, dan swap PostgreSQL tanpa `BEGIN`/`COMMIT` di dalam transaksi yang sudah terbuka (N-1). |
| `crates/qh-ffi/src/commands.rs` | cabang `to_table`, pemindahan ke `table_swap.rs` |
| `crates/qh-ffi/src/env.rs` | `Settings::prefixed`, `Settings::without_prefix` (N-3), redaksi rahasia berprefiks |
| `crates/qh-ffi/src/lib.rs` | `Engine::for_settings` (default `None`), `CliError::Sided { side, inner }` dengan delegasi di `warnings()`, `position()`, `host_key()`, dan `message()` (`:253-285`). Grep tidak menemukan `match` lengkap atas `CliError` di luar berkas ini. |
| `crates/qh-ffi/src/host.rs` | `for_settings` untuk `LongOpEngine`. `PooledEngine` tetap `None` (N-2). |
| `crates/qh-ffi/src/host/lease.rs`, `crates/qh-ffi/src/retry.rs` | meneruskan `copy_in` dan `column_types` (preseden `lease.rs:489-505`, `retry.rs:319-325`) |
| `crates/qh-ffi/src/events.rs` | `side` pada `error`, dan detail host key sisi tujuan |
| `crates/qh-driver/src/lib.rs` | `SqlType`, `Cursor::column_types`, `Session::copy_in`, trait `CopyIn`, `Parameter::Bytes`. `Parameter` sengaja tertutup (`:230-236`), jadi ini amandemen ADR-0028. |
| `crates/qh-driver-postgres/src/lib.rs` | `column_types` dari typmod, `copy_in`, lengan `Parameter::Bytes` |
| `crates/qh-driver-mysql/src/{lib.rs,normalize.rs}` | `column_types`, lengan `Parameter::Bytes`, pembaca `max_allowed_packet` |
| `crates/qh-driver-trino/src/{lib.rs,decode.rs}` | `column_types` dari teks tipe. `Parameter` tidak disentuh: Trino menolak semua parameter (`lib.rs:1419-1435`). |
| Tes Rust | `crates/qh-ffi/tests/to_table_cross.rs` (baru), `crates/qh-ffi/tests/fixtures/transfer_types.json` (baru), `crates/qh-ffi/tests/fixtures/ident_quoting.json` (baru, dibangkitkan dari daftar Swift dan dibaca kedua sisi), `crates/qh-ffi/tests/safe_mode.rs`, `crates/qh-ffi/tests/host.rs`, `crates/qh-ffi/tests/mcp.rs`, dan tes unit per driver |
| App (langkah 8–10) | `Models/QueryTab.swift` (`targetConnectionID`), `Models/Session.swift` (`targetConnection` opsional), `Models/AppModel+Export.swift`, `Models/AppModel+Run.swift`, `Models/AppModel+Tree.swift`, `Models/RunConfirmation.swift` (`transferRequest`), `Support/HostKeyGate.swift`, `Support/EngineEvent.swift`, `Views/ExportSettings.swift`, `Views/TransferSheet.swift` (baru), `Support/Snapshot.swift` (lengan `case`) |
| Tes Swift | `SaveToTableTests.swift` (diperluas), `TableTransferEnvironmentTests.swift`, `TableTransferConfirmationTests.swift`, `SessionTargetConnectionTests.swift`, `IdentQuotingFixtureTests.swift` (semua baru), `HostKeyGateTests` (kasus `DEST_SSH_HOST`) |
| Dokumen | `docs/decisions/0046-…md` (langkah 0), dan bagian tujuan tabel di `app/DESIGN.md` (langkah 11) |

**Tidak disentuh:** `app/Generated/`, `crates/qh-ffi/src/uniffi_api.rs`, `Support/RustEngine.swift`, `crates/qh-ffi/src/mcp.rs`, `crates/qh-sql/**` (hanya dipakai), `crates/qh-editor/**`, `Views/ResultGrid.swift`, `Views/Workspace.swift`, `Models/SchemaTree.swift`, dan `Cargo.lock`.

### 2.4 Rantai §7 dan penempatan

| Berkas atau kelompok | Usulan |
|---|---|
| Lane FFI (`development-plan.md:1045`, `:1050`): hanya `lib.rs`, tanpa regenerasi | Bebas sekarang. `lib.rs` terakhir disentuh `5f7cc4d`. Lane C mendarat tanpa `lib.rs` atau `app/Generated/` (bukti di Verdict). W13-T20 masuk sebelum W12-T3. |
| `crates/qh-ffi/src/commands.rs` (`:1049`) | W12-T9 (`925e4b6`) → **W13-T20** → W12-T3 |
| Tiga driver dan `crates/qh-driver/src/lib.rs` (`:1051`) | **Disusun ulang (OQ-12):** … → W11-T7 (`d494e9c`) → **W13-T20 langkah 4 dan 6** → W11-T8 → W13-T4 → W12-T10 → W12-T7c (memakai ulang primitif W13-T20) → … |
| `host.rs` (`:1089`), `events.rs` (`:1092`), `crates/qh-ffi/tests/safe_mode.rs` (`:1095`) | **W13-T20** sebelum W12-T3 |
| `host/lease.rs` (`:1091`), `retry.rs` (`:1090`) | sesudah W11-T7 (`d494e9c`), sebelum W13-T4 dan W13-T9 |
| Berkas Swift berantai: `QueryTab.swift` (`:1066`), `AppModel+Run.swift` (`:1103`), `AppModel+Export.swift` (`:1097`), `RunConfirmation.swift` (`:1086`), `Snapshot.swift` (`:1083`) | **Diputuskan di langkah 0 terhadap keadaan rantai saat itu** (N-17), karena langkah 8–10 baru mendarat sesudah langkah 12. Perbaikan T-2 `a5fbf11` sekarang memegang `AppModel+Run.swift`, `AppModel+Tree.swift`, dan `QueryTab.swift`. |
| Modul baru, `env.rs`, `ExportSettings.swift`, `Session.swift`, `HostKeyGate.swift`, `EngineEvent.swift` | tidak ada baris §7, jadi bebas |

### 2.5 Tabrakan dan ketergantungan

- **Lane A, B, dan C sudah mendarat.**
  - `strip_terminator_dialect` dipakai apa adanya.
  - Aturan quoting diambil dari `Models/SQLIdentifierQuoting.swift` (`affadcd`, `638e3cc`).
  - Commit lane editor (`62c8488`..`bf9fcbb`) dan `a5fbf11` belum di-push dan belum approved. Langkah 5 dan 8 bergantung pada versi yang approved.
- **Perbaikan T-2 (`a5fbf11`):** langkah 8 dibangun di atasnya.
- **W12-T7c:** memakai ulang primitif W13-T20 (OQ-12, §3.8).
- **Rilis 0.2.2 yang tertahan** (`Cargo.lock`, `crates/qh-ffi/Cargo.toml` belum di-commit): tidak disentuh. Bila langkah 6 butuh fitur `sink` di `futures-util`, harus dibuktikan `Cargo.lock` tidak berubah.
- **W13-T10 (bersyarat, SAFE-SCOPE):** cukup menjaga urutan `commands.rs`.

### 2.6 Perubahan untuk orkestrator (langkah 0)

1. Plan §5: baris W13-T20 (bagian Ringkasan, berkas §2.3, langkah dan gate §13, ukuran L).
2. Plan §7: sesuai §2.4, termasuk penyusunan ulang rantai driver sebagai O baru. Brief W12-T7c diubah menjadi "pakai primitif W13-T20".
3. PRD: FR-RUN-08, UC-19, V-17, O-33, dan koreksi baris "copy object" (`prd:368`).
4. **ADR-0046 ditulis di langkah 0.** Isinya:
   - sumber hanya dibaca;
   - tujuan digerbangi Safe Mode-nya sendiri per statement;
   - amandemen ADR-0026 §1 (create bertanya di `confirm`), termasuk asimetrinya: CTAS koneksi yang sama tetap ditolak di `confirm`, masuk backlog;
   - amandemen ADR-0028 (`Parameter::Bytes`);
   - fallback tipe;
   - arti `STATEMENT_TIMEOUT_MS=0` (§6.6);
   - pesan server yang bisa mengutip nilai (R-9);
   - hak akses B (N-16).
5. Ledger: keputusan OQ-1..OQ-13 (tabel Verdict), dan T-1, T-3..T-6 ke backlog.

## 3. Arsitektur transfer

### 3.1 Urutan langkah di engine (`transfer::run`)

**1. Tanpa jaringan.** Semua penolakan di tahap ini terjadi sebelum `step` pertama (aturan 1, `commands.rs:16-19`).
- Validasi setelan tujuan (§3.2, B-1). `WRITE_MODE` harus `create` atau `replace`; `append` bersama `DEST_*` menjadi `Usage` (OQ-5).
- SQL sumber:
  - `strip_terminator_dialect` dengan dialek sumber;
  - dipecah dengan `qh_sql::statements_dialect` (`crates/qh-sql/src/classify.rs:1193`), dan harus **tepat satu** statement, selain itu `Usage` dengan namanya;
  - `require_read` dengan Safe Mode sumber, lalu `guard_for`.
- Pasangan driver harus termasuk v1 (§11), dan slot `TARGET_*` dibaca menurut jenis **tujuan**.
- Validasi nama (§4.6).
- **Keputusan awal tujuan:** keputusan per statement (§7.2) atas bentuk rencana yang sudah pasti (kind statement dan jenis tujuan). Tujuan `read_only`, `no_ddl`, atau replace di `confirm` ditolak tanpa membuka koneksi. Penolakan dicatat; izin baru dicatat untuk statement nyata.

**2. `step connect_target`.**
- Buka B lewat `engine.for_settings(dest)` dan `open`.
- Probe keberadaan tabel (N-5): PostgreSQL `SELECT to_regclass($1)`, MySQL `information_schema` dengan parameter terikat.
- Create dengan tabel yang sudah ada menjadi galat `side: target` di sini, sebelum query sumber yang mahal dijalankan.

**3. `step connect`.**
- Buka A dari setelan sumber yang **sudah dikupas** dari `DEST_*`. Sesinya selalu `enforce_read_only`.
- `retry::execute` dengan `row_limit: None`.
- **Begitu kursor ada, rentang kursor hidup dimulai** (§6.1).
- Batch pertama diambil. Kursor yang selesai tanpa kolom menjadi galat `side: source`.

**4. `start`.**
- Kolom dan `column_types()` dipetakan ke DDL dari tabel engine (§4.3).
- Statement nyata disusun lalu dicek strukturnya (§7.2).
- Penolakan di sini keluar lewat jalur rentang kursor hidup.

**5. `step write`.**
- PostgreSQL: `BEGIN`, lalu `CREATE TABLE` (create langsung bernama target; replace bernama scratch), lalu jendela-jendela `COPY`.
- MySQL: `CREATE TABLE` scratch (create maupun replace), lalu batch INSERT.

**6. `step verify`** (§6.4).

**7. `step swap`.**
- PostgreSQL: create langsung `COMMIT`; replace memakai swap tanpa `BEGIN` di dalam transaksi, lalu `COMMIT`.
- MySQL: `RENAME` (create) atau swap W12-T9 (replace).

**8. `done`,** lalu kedua sesi ditutup.

### 3.2 Setelan sisi tujuan (B-1: gagal-tertutup)

- **Pemicu.** `transfer::requested` benar bila ada kunci yang diawali `DEST_` (`Settings::is_set` semantik, `env.rs:107-109`), **walaupun nilainya kosong**.
  - `Settings::text` membaca kosong sebagai tidak ada (`env.rs:10-11`, `:120-128`). Karena itu pemicunya tidak boleh `text`.
  - Setelan tujuan yang sebagian atau kosong tidak pernah jatuh ke CTAS di koneksi sumber.
- **Wajib, lalu `Usage` sebelum connect.** Setelah bercabang, yang berikut menjadi galat pemakaian yang menamai kuncinya:
  - `DEST_DB_KIND` **dan** `DEST_DB_URL` keduanya hilang atau kosong;
  - `DEST_SAFE_MODE` hilang, kosong, atau ejaannya tidak dikenal. Sisi baru ini **tidak** mewarisi aturan lama "absen berarti `full`" (`commands.rs:161-172`).
- **Kunci tujuan.** Kunci koneksi sama dengan sumber, ditambah prefiks:
  - `DEST_DB_*` (KIND, HOST, PORT, USER, PASSWORD, DATABASE, SCHEMA, SCHEME, SSLMODE, INSECURE, CA_FILE, JWT, URL, READ_ONLY);
  - `DEST_SAFE_MODE`, `DEST_SAFE_MODE_CONFIRMED`, `DEST_SAFE_MODE_FLOOR`;
  - `DEST_STATEMENT_TIMEOUT_MS`, `DEST_RETRIES`, `DEST_SSH_*`.
  - `TARGET_*` tetap nama tabel (`commands.rs:1161-1165`). `DEST_` belum dipakai di mana pun.
- **`Settings::prefixed("DEST_")`** mengembalikan kunci berprefiks yang dikupas, ditambah dari tingkat atas `DB_PATH` dan `PROGRESS_MS`.
- **Lantai: dua entri, tanpa tabrakan kunci.** `safe_mode_floor` hanya membaca satu kunci `SAFE_MODE_FLOOR` (`commands.rs:146-152`), dan `Settings` adalah peta berkunci tunggal (`env.rs:56-58`). Karena itu `prefixed("DEST_")` **tidak** menyalin `SAFE_MODE_FLOOR` tingkat atas ke peta tujuan; kalau disalin, ia akan bertabrakan dengan `DEST_SAFE_MODE_FLOOR` yang sudah dikupas, dan yang satu diam-diam menimpa yang lain. Mekanismenya:
  1. Lantai tujuan dibangun dengan `safe_mode_floor(&dest, engine)`. Itu membaca `DEST_SAFE_MODE`, `DEST_DB_READ_ONLY`, read-only milik driver tujuan, dan `DEST_SAFE_MODE_FLOOR` (sebagai `SAFE_MODE_FLOOR` di peta tujuan).
  2. `SAFE_MODE_FLOOR` tingkat atas di-parse terpisah, lalu ditambahkan dengan `floor.raise(FloorSource::Policy, mode)`. `SafeModeFloor` menyimpan entri sebagai daftar, dan entri yang berulang ditumpuk (`crates/qh-sql/src/classify.rs:330-345`).
  3. `destructive_mode` dipecah menjadi fungsi murni atas `&SafeModeFloor` (sekarang ia membangun lantainya sendiri, `commands.rs:445-446`). Dengan begitu keputusan DDL tujuan membaca kedua entri, dan `guard_destructive` yang ada tetap memakainya tanpa perubahan perilaku.
  4. Tes di §10.1 memakai dua nilai berbeda (misalnya `DEST_SAFE_MODE_FLOOR=confirm` dan `SAFE_MODE_FLOOR=no_ddl`, lalu sebaliknya), dan create ditolak di kedua urutan.
- **Kupas dari sumber.** `Settings::without_prefix("DEST_")` dipakai untuk semua yang dibaca sisi sumber (N-3). Tes: `a_dest_setting_never_reaches_the_source_and_back`.
- **Koneksi yang sama.** Engine **tidak** menolak berdasarkan identitas.
  - Identitas `PoolKey` memakai `DatabaseSlot::Switchable` untuk MySQL dan Trino (`pool.rs:175-178`) tanpa Safe Mode, sehingga penolakan berbasis `PoolKey` akan menolak sel v1 yang sah. Contohnya MySQL ke MySQL dengan database berbeda, atau pasangan `read_only` dan `full` ke database yang sama (B-7).
  - Yang memilih jalur CTAS adalah app: bila UUID tujuan sama dengan sumber, app tidak mengirim `DEST_*`.
  - Transfer ke koneksi yang identik tetap benar, hanya lebih lambat.

### 3.3 Seam engine kedua

- **Method baru:** `fn for_settings(&self, settings: Settings) -> Option<Box<dyn Engine>>` di `Engine` (`crates/qh-ffi/src/lib.rs:364-379`), default `None`.
- **Implementasi.** `RealEngine` (`:404-413`) dan `LongOpEngine` (`pool.rs:454-459`) mengembalikan engine baru. `PooledEngine` tetap `None`, karena sesi yang di-lease mengirim ulang statement setelah reconnect (`lease.rs:52-69`) dan dikembalikan ke pool (N-2).
- **Engine `None`** membuat transfer gagal dengan galat internal yang menamai seam-nya.
- **Sesi B** dibuka lewat `open_unleased` dengan `PoolKey` dan tunnel miliknya sendiri. Sesinya tidak di-pool dan tidak pernah mengirim ulang (`lease.rs:308-313`, `:344-355`).

### 3.4 Pipa baris, memori, dan backpressure

- **Satu task berurutan:** fetch A, encode, tulis B, cek cancel, ulang.
- **Memori puncak:** satu batch sumber (`TRANSFER_FETCH_ROWS`, 1.000), ditambah satu buffer encode: 1 MiB per kiriman ke `CopyIn` di PostgreSQL, atau maksimal min(`max_allowed_packet`/2, 16 MiB) di MySQL. Satu baris raksasa tetap lewat utuh.
- **Overlap baca dan tulis ditunda** sampai A/B W7 menunjukkan ≥ 10% (`development-plan.md:248`).

### 3.5 Penulis per driver tujuan

- **PostgreSQL: `COPY … FROM STDIN` teks** lewat `Client::copy_in` (`tokio-postgres-0.7.18/src/client.rs:630`).
  - `finish()` mengembalikan hitungan (`copy_in.rs:124-128`).
  - Sink yang di-drop mengirim `CopyFail` (`:55-58`).
  - Tanpa crate baru.
  - `COPY BINARY` ditunda, karena butuh `ToSql` per tipe dan crate fitur.
- **MySQL: INSERT multi-baris dengan `?` terikat** (`crates/qh-driver-mysql/src/lib.rs:793-806`).
  - Satu INSERT memuat min(1.000, ⌊65.535 / kolom⌋) baris, dibatasi min(`max_allowed_packet`/2, 16 MiB).
  - `max_allowed_packet` dibaca sekali per transfer.
  - Sesi tujuan memasang `sql_mode` ketat (`STRICT_ALL_TABLES` ditambahkan) dan `SET NAMES utf8mb4`.
  - Sesi LongOp tidak di-pool, jadi setelan sesi itu tidak bocor (`pool.rs:610-620`).
- **`LOAD DATA LOCAL` ditolak.** Tidak ada handler yang dipasang. Tes SEC menguncinya (N-12; `mysql_async-0.36.2/src/conn/routines/helpers.rs:93`, dikutip reviewer).
- **Trino:** koneksi yang sama memakai CTAS. Lintas koneksi ke Trino ditunda (`transactions: false`, `crates/qh-driver-trino/src/lib.rs:653`).

### 3.6 Aturan flush

- **Jendela `COPY` PostgreSQL** ditutup (`finish()`) pada syarat yang lebih dulu tercapai:
  - 50.000 baris;
  - 64 MiB;
  - min(`DEST_STATEMENT_TIMEOUT_MS`/4, 15 s).
- **Batas 0.** `DEST_STATEMENT_TIMEOUT_MS=0` berarti tanpa batas (`commands.rs:103-106`). Dalam kasus itu jendela ditutup pada 15 s.
- **Timer.** Batas waktu jendela adalah timer di `tokio::select!`, bukan dicek saat batch berikutnya datang (N-4). Dengan begitu sumber yang lambat tidak membuat jendela melewati batasnya.
- **MySQL:** satu INSERT per batch. `max_execution_time` tidak berlaku untuk write (`docs/invariants.md:108`).
- **Angka-angka di atas** adalah titik awal. Angka finalnya ditentukan A/B di langkah 12.

### 3.7 Result store, spill, dan `LIMIT`

- **Query dijalankan ulang penuh,** seperti Export (`ResultGrid.swift:919-921`, `commands.rs:998`).
- **`LIMIT` di footer grid tidak berlaku** (`ResultGrid.swift:896-908`).
- **Store dan spill tidak dipakai.** NFR-S5 (`prd:277`) tetap terpenuhi.

### 3.8 Primitif bersama dengan W12-T7c

Diputuskan (OQ-12). W13-T20 memiliki:

- `Session::copy_in` dan `CopyIn`;
- pembaca `max_allowed_packet`;
- `Parameter::Bytes`.

W12-T7c (`development-plan.md:894`) memakai ulang primitif itu, dan lingkupnya menyusut ke kebijakan impor.

## 4. Pemetaan tipe

### 4.1 Deskriptor tipe terstruktur

- **`Cursor::column_types(&self) -> Option<&[SqlType]>`,** default `None`, dengan bentuk sibling seperti `affected_rows` (`crates/qh-driver/src/lib.rs:824-826`).
- **Diteruskan** oleh `Tracked` (`lease.rs:90-104`) dan `RetryCursor` (`retry.rs:319-325`).
- **Isi `SqlType`:**
  - `base`: enum tertutup;
  - `precision`, `scale`, `length`, `unsigned`, `time_zone`, `fraction_digits`;
  - `element` untuk array, `fields` untuk row;
  - `oid` untuk PostgreSQL;
  - `native`: hanya untuk pesan, **tidak pernah untuk SQL** (B-3a).
- **Fungsi murni per driver,** terbukti offline (langkah 4):
  - PostgreSQL dari OID ditambah typmod `i32` (numeric `p = ((t-4) >> 16) & 0xffff`, `s = (t-4) & 0xffff`; varchar dan bpchar `n = t-4`);
  - MySQL dari `Column` mysql_common yang dibangun di tes: `column_length`, `decimals`, flag, dan charset. Presisi decimal = panjang − (s > 0 ? 1 : 0) − (unsigned ? 0 : 1). Panjang karakter = panjang / mbmaxlen;
  - Trino dari teks tipe (`decode.rs:193-250`).
- **Pemasangan ke kursor nyata** dikompilasi dan diuji unit, tetapi perilakunya terhadap server nyata **pending live**.
- **Deskriptor yang tidak ada** menjadi R: "the source driver did not describe its column types".

### 4.2 Tipe logis

| Tipe logis | Dari PostgreSQL | Dari MySQL | Dari Trino |
|---|---|---|---|
| Bool | `bool` | — (`tinyint(1)` tetap Int8) | `boolean` |
| Int8/UInt8, Int16/UInt16, Int24/UInt24, Int32/UInt32, Int64/UInt64 | `int2`, `int4`, `int8` | semua integer, dengan `unsigned`; `year` adalah Int16 | `tinyint`..`bigint` |
| Float32 / Float64 | `float4` / `float8` | `float` / `double` | `real` / `double` |
| Decimal(p,s) / Decimal(∞) | `numeric(p,s)` / `numeric` | `decimal(p,s)` | `decimal(p,s)` |
| Text(n) / Text(∞) / Char(n) | `varchar(n)` / `text` / `bpchar(n)` | string dan teks blob | `varchar(n)` / `varchar` / `char(n)` |
| Binary | `bytea` | biner (flag) | `varbinary` |
| Uuid | `uuid` | — | `uuid` |
| Date, Time(p), TimeTz(p), Timestamp(p), TimestampTz(p) | sesuai nama | `date`, `time`, `datetime`, `timestamp` (tanpa offset) | sesuai nama, p ≤ 12 |
| Interval | `interval` | — | dua tipe interval |
| Json / Jsonb | `json` / `jsonb` | `json` | `json` |
| Array(T), Map, Row | `_T` | — | `array`, `map`, `row` |
| Enum, Set | enum pengguna | `enum`, `set` | — |
| Builtin(oid) | tipe bawaan PostgreSQL di allowlist (§4.3) | — | — |
| Other | sisanya | `bit`, `geometry`, `vector` | sisanya |

### 4.3 Tabel DDL tujuan

- **Notasi:** E = Exact, W = Widen (setia, lebih lebar, dilaporkan bila ada perubahan), T = fallback teks yang terdokumentasi dan dilaporkan per kolom, R = Refuse.
- **Fallback hanya untuk sel T** (OQ-7). Sel R tetap R.
- **Teks DDL hanya berasal dari tabel ini** (`types.rs`), dikunci pada `SqlType` yang sudah di-parse (B-3a).

| Tipe logis | PostgreSQL | MySQL (`DEFAULT CHARSET=utf8mb4`) |
|---|---|---|
| Bool | `boolean` E | `tinyint(1)` W |
| Int8, UInt8 | `smallint` W | `tinyint` E, `tinyint unsigned` E |
| Int16 / UInt16 | `smallint` E / `integer` W | `smallint` E / `smallint unsigned` E |
| Int24 / UInt24 | `integer` W | `mediumint` E / `mediumint unsigned` E |
| Int32 / UInt32 | `integer` E / `bigint` W | `int` E / `int unsigned` E |
| Int64 / UInt64 | `bigint` E / `numeric(20,0)` W | `bigint` E / `bigint unsigned` E |
| Float32 / Float64 | `real` E / `double precision` E | `float` E / `double` E; NaN dan ±Inf menjadi R per nilai |
| Decimal(p,s), p ≤ 65, s ≤ 30 | `numeric(p,s)` E | `decimal(p,s)` E. Tidak pernah `DECIMAL` polos. |
| Decimal(p,s), p > 65 atau s > 30 | `numeric(p,s)` E | R |
| Decimal(∞) | `numeric` E | `decimal(65,30)` W dengan cek per nilai: skala ≤ 30 dan digit bulat ≤ 35, selain itu R per nilai. Dilaporkan (OQ-7). |
| Text(n) | `varchar(n)` E | `varchar(n)` E bila total baris ≤ 65.535 byte; selain itu `text`/`mediumtext`/`longtext` W |
| Text(∞) | `text` E | `longtext` W |
| Char(n) | `char(n)` E | `char(n)` E untuk n ≤ 255, selain itu `varchar(n)` W |
| Binary | `bytea` E | `varbinary(n)` E atau `longblob` W |
| Uuid | `uuid` E | `char(36)` T |
| Date | `date` E | `date` E, dengan rentang dicek per nilai (1000-01-01..9999-12-31) |
| Time(p ≤ 6) | `time(p)` E; MySQL `time` di luar 0..24 jam menjadi R per nilai | `time(p)` E |
| TimeTz(p ≤ 6) | `timetz(p)` E | R |
| Time atau TimeTz dengan p > 6 (Trino) | R (OQ-10) | R |
| Timestamp(p ≤ 6) | `timestamp(p)` E | `datetime(p)` E, dengan rentang dicek per nilai |
| TimestampTz(p ≤ 6) | `timestamptz(p)` E | `datetime(p)` T: dinormalisasi ke UTC, rentang dicek per nilai |
| Timestamp atau TimestampTz dengan p > 6 (Trino) | R (OQ-10) | R |
| MySQL `timestamp` sebagai sumber | `timestamp` T (jam dinding zona sesi sumber) | `datetime` T |
| Interval | `interval` E | R |
| Json (sumber `json` PostgreSQL, MySQL, atau Trino) | `json` E, teks disimpan persis | `json` W: MySQL menormalkan urutan kunci dan spasi; kunci ganda menjadi R per nilai |
| Jsonb (sumber `jsonb` PostgreSQL) | `jsonb` E | `json` W |
| Array(T), T punya E di PostgreSQL | `T[]` E; array dengan batas bawah bukan 1 menjadi R per nilai (N-8) | `json` T |
| Array lain, Map, Row | `jsonb` T: kunci map ganda dan array Trino bersarang yang tidak rata menjadi R per nilai | `json` T, dengan aturan yang sama |
| Enum, Set | `text` T | `varchar(255)` T |
| Builtin(oid), PostgreSQL ke PostgreSQL | DDL dari allowlist engine per OID: `inet`, `cidr`, `macaddr`, `macaddr8`, `bit(n)`, `varbit(n)`, `xml`, `tsvector`, `tsquery`, `point`, `line`, `lseg`, `box`, `path`, `polygon`, `circle`, `money`. **Tanpa** `oid` dan `reg*`. | R |
| Other (tipe ekstensi, domain, tipe pengguna selain enum; MySQL `bit`/`geometry`/`vector`; tipe Trino khusus) | R | R |

- **Tanpa NOT NULL.** Semua kolom nullable, tanpa kunci, indeks, default, constraint, trigger, atau grant (§8.1, N-7).
- **Batas baris MySQL.** Jumlah lebar `varchar` dihitung lebih dulu terhadap 65.535 byte. Bila lewat, kolom terlebar dinaikkan ke `text`, dan itu dilaporkan.
- **Trino `varchar(n)` dengan n > 10.485.760** menjadi `text` di PostgreSQL (N-8).

### 4.4 Nilai: encoding dan validasi (B-4)

**Varian yang diterima.** Encoder hanya menerima varian `Value` yang diharapkan untuk tipe logisnya, ditambah `Null`. Varian lain ditolak per nilai, termasuk `Text` di kolom date, timestamp, numeric, atau integer, `Unknown`, dan `Interval` di kolom `Time`. Pengecualiannya hanya **ejaan khusus PostgreSQL yang dicocokkan persis**, khusus PostgreSQL ke PostgreSQL:

| Kasus | PostgreSQL ke PostgreSQL | Ke MySQL |
|---|---|---|
| Tanggal nol atau tidak valid MySQL (`Value::Text` di date atau datetime, `normalize.rs:345-375`, `:541-543`) | — | R per nilai |
| Numeric `NaN`, `Infinity`, `-Infinity` (`Text` di kolom numeric, `normalize.rs:123`, `:140`) | diteruskan apa adanya, E. `Infinity` numeric butuh PostgreSQL ≥ 14; server lama menolak dengan galat bersisi `target`. | R per nilai |
| Date atau timestamp `infinity`, `-infinity`, dan ejaan SM `… BC` (`Text`, `normalize.rs:142-157`) | diteruskan bila cocok persis dengan pola ejaan PostgreSQL, E | R per nilai |
| Teks lain di kolom bertipe | R per nilai | R per nilai |

**Isi nilai yang diperiksa:**
- U+0000 di teks: R per nilai ke PostgreSQL ("PostgreSQL cannot store U+0000 in text"). E ke MySQL.
- Rentang MySQL: date dan datetime 1000-01-01..9999-12-31.
- Skala decimal tidak boleh melebihi skala tujuan (tanpa pembulatan), dan digit bulat harus muat.
- Panjang teks ≤ n.
- NaN dan Inf ke MySQL.
- `time` MySQL di luar 0..24 jam ke PostgreSQL.
- Kunci JSON ganda: diperiksa untuk tujuan `jsonb` (T) dan MySQL `json`, tidak untuk PostgreSQL `json`.

**Encoding:**
- `COPY` teks PostgreSQL:
  - NULL ditulis `\N`;
  - `\`, newline, CR, dan tab di-escape;
  - bytea ditulis `\\x` + hex;
  - float dengan representasi pulang-pergi terpendek, ditambah `NaN`, `Infinity`, dan `-Infinity`;
  - decimal eksak;
  - waktu ISO 8601 dengan offset;
  - interval ISO 8601;
  - literal array `{…}` dengan quoting elemen;
  - JSON apa adanya.
- MySQL: `Null`, `Int`, `UInt`, dan `Float`; `Text` untuk decimal, waktu, JSON, dan T; `Bytes` untuk biner.

**Pelanggaran** menghentikan transfer dengan kolom, nomor baris sumber, dan aturannya, misalnya `column "amount", source row 48213: scale 4 exceeds the target's 2`. Nilainya **tidak pernah** disebut. Galat ini bersisi `target` (N-6).

**Fixture.** Setiap kasus di §4.3 dan tabel di atas mendapat baris di `transfer_types.json` sebelum langkah 5 membekukannya.

### 4.5 Identifier dan nama tabel (B-3b)

- **Satu fungsi Rust** di `transfer/names.rs` untuk semua identifier (tabel, schema, database, kolom, scratch):
  - nama polos `[a-z_][a-z0-9_]*` yang tidak ada di daftar kata cadangan ditulis telanjang;
  - nama lain dikutip, dengan karakter kutip dialeknya digandakan. Semantiknya sama dengan `qh_sql::quote_ident` (`crates/qh-sql/src/ident.rs:21-26`).
- **Daftar kata cadangan** adalah satu daftar gabungan untuk semua dialek, sama dengan `Models/SQLIdentifierQuoting.swift:3-24` (316 kata di `bf9fcbb` dan `a5fbf11`).
- **Fixture `ident_quoting.json`** dibangkitkan dari daftar Swift itu, dan dibaca tes Rust dan Swift. Isinya mencakup nama dengan `"`, backtick, newline, `;`, spasi di akhir, dan huruf besar.
- **Nama yang ditolak:**
  - NUL;
  - PostgreSQL > 63 byte (server memotong diam-diam);
  - MySQL > 64 karakter atau berakhir spasi;
  - kosong atau `?column?` PostgreSQL;
  - duplikat (OQ-8);
  - dua kolom yang sama bila huruf diabaikan, untuk MySQL.

  `_col0` Trino dan teks ekspresi MySQL diterima dan dikutip bila perlu.
- **Slot target** mengikuti jenis tujuan (`ConnectionKind.targetParts`, `Connections.swift:61-67`, setara dengan `slots` di `sql_ident.rs`).
  - Database PostgreSQL adalah database koneksi tujuan.
  - Schema atau database tujuan harus sudah ada.

## 5. Mode dan atomisitas (create dan replace; append di luar v1)

### 5.1 Per mode dan tujuan

| Mode | PostgreSQL | MySQL |
|---|---|---|
| create | `BEGIN; CREATE TABLE target(...)`, jendela `COPY target`, verifikasi, `COMMIT`. Nama yang sudah dipakai gagal di probe atau di `CREATE`, sebelum baris pertama. | probe, `CREATE TABLE qh_new_*`, batch INSERT, verifikasi, `RENAME TABLE qh_new_* TO target`. Saat gagal: `DROP TABLE IF EXISTS qh_new_*`. |
| replace | `BEGIN; CREATE TABLE qh_new_*`, `COPY`, verifikasi, lalu swap tanpa `BEGIN` (`DROP TABLE IF EXISTS target; ALTER TABLE qh_new_* RENAME TO …`), `COMMIT` | scratch dan INSERT, verifikasi, `RENAME TABLE target TO qh_old_*, qh_new_* TO target`, `DROP qh_old_*` (`commands.rs:1428-1434`) |

- **`DROP` yang terhalang view dependen** di PostgreSQL gagal, lalu `ROLLBACK`, dan tabel lama utuh.
- **Kunci `ACCESS EXCLUSIVE`** pada target lama hanya dipegang dari `DROP` sampai `COMMIT`.
- **Help replace** menyatakan bahwa tabel baru tidak membawa kunci, indeks, trigger, atau grant, dan bahwa grant MySQL per nama tidak ikut `RENAME` (N-7).

### 5.2 Kenapa dua bentuk

PostgreSQL punya DDL transaksional (`commands.rs:1349-1350`). MySQL melakukan commit implisit pada DDL, jadi atomisitasnya datang dari `RENAME TABLE` (`:1350-1351`).

### 5.3 Yang bisa tertinggal

- **PostgreSQL:** tidak ada.
- **MySQL, galat atau Stop:** `discard_scratch` (`commands.rs:1608-1631`). Bila gagal, peringatan menyebut scratch-nya.
- **MySQL, proses mati:** `qh_new_<stamp>` tertinggal dan target tidak tersentuh. Ini dinyatakan di help. Pembersih otomatisnya backlog.
- **Drop backup MySQL yang gagal:** peringatan W12-T9 (`commands.rs:1708-1718`).

## 6. Pembatalan, kegagalan, dan verifikasi

### 6.1 Cancel sebelum drop di setiap jalan keluar (B-5)

- **Rentang kursor hidup.** Rentangnya dimulai saat `execute` sumber mengembalikan kursor (§3.1 langkah 3), dan berakhir saat kursor menjawab `Ok(None)`.
- **Satu pintu keluar.** Di rentang itu kursor dipegang tipe `LiveSource`. Satu-satunya jalan keluar adalah `LiveSource::finish(self, outcome)`. Untuk setiap hasil selain "kursor selesai", `finish` memanggil `stop_session(session, Some(cursor))` lebih dulu: cancel ke server, lalu drop kursor, lalu close (`commands.rs:2050-2069`). Setelah itu baru galat atau `done` dikembalikan.
- **Tanpa `?` di rentang itu.** Fungsi di rentang itu mengembalikan `Result<_, Exit>` yang ditangani satu `match` di `run`. Ini aturan review dan dikunci tes.
- **Jalan keluar yang dicakup:**
  - Stop;
  - galat tujuan di tengah stream;
  - galat `CREATE`, probe, atau `COPY`/INSERT;
  - penolakan tipe di `start` (sesudah batch pertama);
  - penolakan encoder;
  - galat internal;
  - panik yang ditangkap di batas FFI (`uniffi_api.rs:358-378`), yang tetap men-drop kursor tanpa cancel. Ini sisa risiko yang dicatat sebagai R-16.
- **Di luar rentang.** Verifikasi yang tidak cocok (§6.4) terjadi setelah kursor sumber menjawab `Ok(None)`. Saat itu tidak ada kursor yang perlu dibatalkan: sesi sumber ditutup biasa, dan tujuan di-rollback atau scratch-nya di-drop.
- **Stop sebelum kursor ada.** Untuk `execute` yang belum kembali, berlaku `execute_until_stopped` dengan grace Trino (`commands.rs:1906-1944`).
- **Urutan di sisi tujuan setelah sumber dihentikan** (N-4):
  - `cancel_bounded(target.cancel())` bila ada `COPY` atau statement yang berjalan;
  - sink `CopyIn` di-drop (`CopyFail`);
  - `ROLLBACK` dibatasi `STOP_CEILING`;
  - close bila waktunya habis.

  Di MySQL: `cancel` (`KILL QUERY`), lalu `discard_scratch`.
- **Stop setelah verifikasi** membuang hasil. Stop selama swap membiarkan swap selesai (`commands.rs:1659-1665`).

### 6.2 Kegagalan sebagian

| Kejadian | `side` | B setelahnya |
|---|---|---|
| A gagal connect, SQL ditolak, timeout, tanpa kolom, atau putus di tengah stream | `source` | tidak berubah |
| Probe, `CREATE`, izin, nilai, atau disk di B | `target` | tidak berubah |
| Penolakan tipe atau encoder | `target` (N-6) | tidak berubah |
| Verifikasi tidak cocok | `target` | tidak berubah |
| Drop backup MySQL gagal | — (peringatan) | tabel baru terpasang, `qh_old_*` disimpan |

### 6.3 Retry

Connect di kedua sisi di-retry (`RETRIES`, `DEST_RETRIES`). Halaman sumber mengikuti aturan `retry.rs` yang ada. Tulis tujuan tidak pernah di-retry (`commands.rs:1249-1253`). Transfer yang gagal diulang pengguna dari awal.

### 6.4 Verifikasi jumlah baris

- **Perbandingan:** `rows_read` dari A dibandingkan dengan jumlah `finish()` setiap jendela `COPY`, atau jumlah `affected_rows()` setiap INSERT.
- **Bila tidak sama,** transfer menjadi galat dan tidak ada swap atau `COMMIT`.
- **`done.verified: true`** hanya bila sama.
- **`COUNT(*)` ulang ditunda.**

### 6.5 Sisi galat

- **`CliError::Sided { side, inner }`** menjadi field `side` pada `error`. Jalur lain tidak membuatnya, jadi `match` yang ada (misalnya `commands.rs:2770`) tidak terpengaruh.
- **App menerjemahkan `side`** ke nama koneksi.
- **Detail host key tujuan** digerbangi `DEST_SSH_HOST_KEY_DETAIL`.

### 6.6 Batas waktu (PF-23, OQ-13)

- **PF-23 sudah terukur** (`target/run/keep/ledger.md:456`). Konsumen lambat terpotong batas statement di server: MySQL setelah 282.600 baris, PostgreSQL setelah 19.400 baris. Transfer adalah konsumen lambat: sumber menunggu tujuan.
- **Aturan sisi sumber (wajib).** App mengirim `STATEMENT_TIMEOUT_MS=0` di sisi sumber **hanya bila** batas koneksi sumber adalah default app yang diwarisi (`statementTimeoutMS == nil`, `Connections.swift:376`; default 60 s di `AppModel.swift:356-358`).
- **Batas eksplisit per koneksi** tetap dikirim dan ditampilkan di wizard. Itu pilihan sadar pengguna (OQ-13).
- **Arti 0:**
  - `statement_timeout(settings)` menjadi `None` (`commands.rs:103-106`);
  - PostgreSQL `timeout_statement(None, None)` tidak mengirim apa pun (`crates/qh-driver-postgres/src/lib.rs:1343-1352`);
  - MySQL tidak mengirim `max_execution_time` pada sesi baru (`crates/qh-driver-mysql/src/lib.rs:397-400`, `:1738-1749`);
  - Trino tanpa header `query_max_run_time` (`crates/qh-driver-trino/src/lib.rs:1354-1360`).

  Jadi app tidak menambah batasnya sendiri, tetapi **batas yang dipasang DBA** di role, database, atau server tetap berlaku. Transfer yang terpotong olehnya menjadi galat `side: source` yang menyebut batasnya. ADR-0046 menyatakannya.
- **Sisi tujuan:** batas koneksinya berlaku per statement. Jendela `COPY` (§3.6) menjaga setiap statement di bawahnya.
- **`idle_in_transaction_session_timeout`** di tujuan bisa memutus transaksi yang menunggu sumber yang lambat (R-7). Transaksinya dibuka setelah batch pertama, dan bila putus, rollback dilakukan server.

## 7. Keamanan dan keselamatan (tingkat Tinggi)

### 7.1 Sumber hanya dibaca

- Tepat satu statement (§3.1), lalu `require_read` dengan alasan "Save to Table across connections reads the source and nothing else", lalu `guard_for`. Urutannya aturan log di `commands.rs:318-322`.
- Sesi A selalu `enforce_read_only` (`crates/qh-driver/src/lib.rs:738-748`). Trino tidak punya sakelar itu (`:734-736`).

### 7.2 Penulisan: keputusan per statement nyata (B-2, B-3)

Keputusan diambil dari **entri lantai tujuan** (`DEST_SAFE_MODE`, `DEST_DB_READ_ONLY`, read-only milik driver, `DEST_SAFE_MODE_FLOOR`, dan `SAFE_MODE_FLOOR` tingkat atas), dengan dialek tujuan.

**Rencana per mode** (semua statement yang bisa dikirim):

| Statement | Kind | create PostgreSQL | create MySQL | replace PostgreSQL | replace MySQL |
|---|---|---|---|---|---|
| `BEGIN` / `COMMIT` / `ROLLBACK` | kontrol milik engine (classifier: `Unknown`) | ya | — | ya | — |
| `SET SESSION sql_mode = CONCAT(@@sql_mode, ',STRICT_ALL_TABLES')`, `SET NAMES utf8mb4` | setup sesi milik engine (`Unknown`) | — | ya | — | ya |
| `SELECT @@max_allowed_packet` | bacaan milik engine | — | ya | — | ya |
| probe: `SELECT to_regclass($1)` / `information_schema.tables` dengan parameter | bacaan milik engine | ya | ya | ya | ya |
| `CREATE TABLE target (...)` | DDL | ya | — | — | — |
| `CREATE TABLE qh_new_* (...)` | DDL | — | ya | ya | ya |
| `COPY … FROM STDIN` | DML | ya | — | ya | — |
| `INSERT INTO qh_new_* (...) VALUES (?, …)` | DML | — | ya | — | ya |
| `RENAME TABLE qh_new_* TO target` | DDL | — | ya | — | — |
| `DROP TABLE IF EXISTS target` dan `ALTER TABLE qh_new_* RENAME TO …` | DDL | — | — | ya | — |
| `RENAME TABLE target TO qh_old_*, qh_new_* TO target` dan `DROP TABLE qh_old_*` | DDL | — | — | — | ya |
| `DROP TABLE IF EXISTS qh_new_*` (pembersih) | DDL | — | ya | — | ya |

**Aturan keputusan:**
- **Kontrol transaksi, setup sesi, dan bacaan milik engine** tidak melewati classifier.
  - Classifier membaca `BEGIN`, `COMMIT`, `ROLLBACK`, dan `SET` sebagai `Unknown` (`crates/qh-sql/src/classify.rs:1636-1653`), dan `confirm` menolaknya walaupun sudah dikonfirmasi (`:1805-1807`). Kalau lewat classifier, create di `confirm` tidak pernah bisa berjalan di PostgreSQL. Preseden: rencana replace W12-T9 hari ini mem-`guard_for` `BEGIN`/`COMMIT` lewat `statements()` (`commands.rs:1244-1246`, `:1450-1460`), dan itu belum pernah diuji di `confirm`.
  - Aturannya: statement ini adalah **allowlist tertutup berteks persis** di `transfer/plan.rs` (bentuk dengan parameter untuk probe). Cek struktur (`statements_dialect` tepat satu) dan kecocokan teks persis dengan allowlist dilakukan sebelum kirim; teks di luar allowlist menjadi `Internal`.
  - Statement ini diizinkan **hanya bila rencana tulisnya diizinkan** (keputusan DDL dan DML di bawah). Ia tidak dicatat di execution log, sama dengan statement sesi milik driver hari ini (`enforce_read_only`, `crates/qh-driver/src/lib.rs:741-748`).
- **DDL** memakai semantik `destructive_mode` atas entri lantai (`commands.rs:445-459`):
  - entri yang melarang DDL (`no_ddl`, `read_only`) membuatnya ditolak;
  - entri `confirm` membuatnya **bertanya**;
  - selain itu diizinkan.

  Konfirmasi dibaca dari `DEST_SAFE_MODE_CONFIRMED`, dan satu konfirmasi berlaku untuk seluruh rencana yang disebut di lembar (§7.3).
- **DML** (`COPY`, `INSERT`) memakai `guard_confirmed` (`commands.rs:228-247`) dengan Safe Mode, dialek, dan konfirmasi yang sama.
- **Pembersih `DROP TABLE IF EXISTS qh_new_*`** atas scratch milik engine sendiri dicatat dan **diizinkan bila `CREATE` scratch itu diizinkan di run yang sama**. Tanpa aturan ini, create di `confirm` tidak pernah bisa membersihkan dirinya.
- **Replace dengan entri `confirm` apa pun ditolak** (OQ-9): replace menghapus tabel pengguna. Ditolak sebelum connect, dengan kalimat yang menyebut tingkatnya.

| Safe Mode tujuan (entri) | create | replace |
|---|---|---|
| `full` | jalan | jalan; app tetap meminta konfirmasi destruktif |
| `no_ddl` | ditolak | ditolak |
| `confirm` | **bertanya**, satu konfirmasi | ditolak |
| `confirm` ditambah lantai `no_ddl` | ditolak (entri), sama dengan ADR-0027 | ditolak |
| `read_only` | ditolak | ditolak |

**Statement nyata, bukan templat (B-3c).** Setiap statement yang benar-benar akan dikirim disusun dari nama dan tipe yang sudah divalidasi (§4.3, §4.5), lalu sebelum dikirim:
1. Dipecah dengan `qh_sql::statements_dialect` di bawah dialek tujuan, dan harus tepat **satu** statement.
2. `classify_readings`-nya harus sama dengan kind di rencana. Bila berbeda, hasilnya `CliError::Internal`, tidak ada yang dikirim, dan rentang kursor hidup ditutup lewat §6.1.
3. Keputusannya dicatat di execution log dengan hash statement nyata itu. Statement yang teksnya sama (jendela `COPY` berikutnya, INSERT dengan jumlah baris yang sama) dicatat sekali. Volume log terbatas pada jumlah teks berbeda, kira-kira tujuh baris per transfer (`development-plan.md:896`).
4. Penolakan di tahap tanpa jaringan (§3.1 langkah 1) dicatat dengan subjek rencananya.

**ADR-0046** mencatat amandemen ADR-0026 §1 (create di `confirm` bertanya, per statement lewat `destructive_mode`) dan asimetrinya: CTAS koneksi yang sama tetap ditolak di `confirm`, masuk backlog.

### 7.3 Lembar transfer selalu tampil (B-2, B-6)

- **`RunConfirmation.transferRequest(source:target:mode:table:)`** selalu mengembalikan permintaan untuk setiap run lintas koneksi, apa pun Safe Mode tujuannya. Tidak ada opsi "jangan tanya lagi". Bentuknya bergaya `destructiveRequest` (`RunConfirmation.swift:58-73`), bukan `request`, yang diam saat ada DDL (`:43`).
- **Isi lembar,** untuk sumber dan tujuan:
  - nama, jenis, `host:port`, dan database;
  - label lingkungan dan status TLS;
  - ditambah tabel tujuan, mode, dan kalimat "The table gets the query's columns only".
- **Nama ditulis dengan `SQLIdentifierQuoting.quoted`** (`Models/SQLIdentifierQuoting.swift:31-35`), bukan `qualifiedName`/`quotedIdent` (`SchemaTree.swift:225-251`), yang tidak menggandakan kutip (B-3d).
- **Persetujuan.** Bila tujuan `confirm` (create), persetujuan lembar ini menambah `DEST_SAFE_MODE_CONFIRMED=1` ke run itu saja. Bila tujuan `full` dan mode replace, lembar memakai gaya destruktif coral.
- **Tujuan menurut UUID.** Tujuan disimpan dan diresolusi hanya lewat `targetConnectionID`, lalu divalidasi ulang tepat sebelum run. Koneksi yang sudah dihapus menjadi `nil` dan memblokir run ("Choose the target connection again").
- **Tab hasil pemulihan sesi** tidak pernah menjalankan transfer tanpa lembar ini.

### 7.4 Label lingkungan (OQ-3, disesuaikan)

- **Pemicu.** Bila sumber berlabel `prod` dan tujuan **tidak** berlabel `prod` (staging, dev, **atau tanpa label**), lembar menampilkan banner dan meminta centang eksplisit ("I understand rows from <A> will be copied to <B>").
- **Sumber tanpa label** tidak memicunya. Label baru default-nya `nil` (`Connections.swift:373`, `:405`).
- **Label hanya di app** (`:177-181`), dan lembar B-6 tetap selalu tampil.

### 7.5 Kredensial dan Keychain untuk dua koneksi

- `connectionEnvironment` dipanggil dua kali (`AppModel+Connections.swift:1023-1035`). Galatnya menyebut nama koneksi (`:1029`).
- Kedua env dibangun sebelum run. Bila salah satu gagal, tidak ada yang dijalankan.
- Rahasia lintas FFI tetap berupa data `Setting` (`uniffi_api.rs:77-83`, `RustEngine.swift:137`).

### 7.6 SSH di kedua sisi

- **Tunnel sendiri per sisi** (§3.3).
- **`HostKeyGate`:**
  - memasang `DEST_SSH_HOST_KEY_DETAIL=1` bila `DEST_SSH_HOST` ada;
  - galat `side == "target"` dilaporkan dengan env yang dikupas dari prefiks.
- **Percobaan ulang trust untuk tujuan** memberi prefiks ulang pada `DEST_SSH_HOST_KEY_ACCEPT` dan `DEST_SSH_APP_KNOWN_HOSTS` (N-10).
- **Prompt biasanya muncul lebih awal,** saat wizard memuat metadata tujuan lewat perintah biasa (`AppModel+Tree.swift:555`, `:574`).

### 7.7 TLS

Setiap sisi memakai modenya sendiri. Sisi `RequireNoVerify` (`crates/qh-driver/src/lib.rs:584`) ditampilkan di wizard dan lembar dengan label W13-T19 (`"<kata> · unverified"`). Backlog impor `verify: false` (`ledger.md:659-660`) berlaku untuk kedua sisi.

### 7.8 Log, data baris, dan rahasia

- **Redaksi berprefiks.** Redaksi menjadi `SECRET_KEYS.contains(key) || key.strip_prefix("DEST_").is_some_and(|k| SECRET_KEYS.contains(&k))` (`env.rs:44-52`, `:63-76`), dengan tes yang dipatok.
- **Kupas dari sumber.** `DEST_*` dikupas dari setelan sumber (N-3).
- **Nilai baris tidak pernah ditulis** ke event, galat engine, stderr, atau execution log. Galat encoder hanya menyebut kolom dan baris; MySQL memakai parameter terikat; log hanya menyimpan hash (ADR-0026 §7, `0026-safe-mode-floor.md:77-78`).
- **Sisa risiko R-9:** pesan server tujuan bisa mengutip nilai.

### 7.9 MCP dan CLI

- **MCP:** tidak mendapatnya. `to_table` ditolak berdasar nama (`mcp.rs:1000-1003`). `settings_for` membangun setelan dari kunci tetap lalu memaksa `read_only` (`mcp.rs:1456-1484`), sehingga `DEST_*` tidak bisa disuntik. Tes di `crates/qh-ffi/tests/mcp.rs` menguncinya.
- **CLI:** mendapatnya. CLI hanya untuk developer; `app/build.sh:57-62` hanya membundel `queryhive-mcp`. Harness golden dan shell pengembang wajib membersihkan `DEST_*` (N-9), karena satu `DEST_` sisa membuat `to_table` bercabang (B-1).

### 7.10 Galat izin di tujuan

Jawaban server tidak di-retry (`retry.rs:33-40`). Galatnya bersisi `target`, dan B tidak berubah.

### 7.11 Execution log

Kedua sisi menulis ke log yang sama (`execution_log.rs:177-202`). Entri tidak membawa identitas koneksi (`:73-83`), jadi keputusan kedua sisi hanya bisa dibedakan dari hash statement nyata (§7.2). Kolom sisi masuk backlog (migrasi `qh-storage`).

### 7.12 Hak akses koneksi tujuan (N-16)

Engine menulis dengan kredensial B, dan hanya Safe Mode B yang menggerbangi tulisan itu. Baris yang hanya terbaca lewat identitas A menjadi terbaca oleh siapa pun yang bisa membaca B. Engine tidak bisa melihat grant B. Mitigasinya lembar §7.3 dan §7.4; ADR-0046 menyatakannya.

## 8. UI (langkah 8–10; tidak masuk `main` sebelum langkah 12)

### 8.1 Wizard

1. **Connection.** `Menu` (bukan popover bersarang, `ExportSettings.swift:157-159`). Default "Same connection (<nama>)". Setiap item menampilkan logo, nama, label lingkungan, dan Safe Mode. Koneksi Trino lain tampil nonaktif dengan alasannya.
2. **Bagian target** menurut `targetParts` jenis **tujuan** (`Connections.swift:61-67`). Opsinya dari `loadCatalogs`, `loadSchemas`, dan `targetChoices` dengan `targetConnectionID` (`AppModel+Tree.swift:555`, `:574`, `:602`).
3. **Table,** dengan cek tabrakan (§8.3).
4. **"If the table already exists":** hanya Create dan Replace untuk lintas koneksi (append disembunyikan, OQ-5). Teksnya diperbaiki (T-1), dan help replace mengikuti N-7. Mode yang ditolak Safe Mode tujuan tampil nonaktif dengan kalimat tingkatnya.
5. **Blok lintas koneksi:**
   - "Rows are read from <A> and written to <B> by QueryHive";
   - "The new table has the query's columns only";
   - batas statement sumber yang berlaku (§6.6);
   - banner TLS tanpa verifikasi;
   - banner prod (§7.4).
6. **Ringkasan kepala popover** (`ExportSettings.swift:136-144`): `"<mode> <target> on <B>"`.

### 8.2 State

- **`QueryTab.targetConnectionID: UUID?`,** dengan `nil` berarti sama dengan sumber.
- **`target(for:)`, `missingTargetPart(for:)`, `runSummary`, dan `isDestructive`** dipanggil dengan jenis koneksi tujuan. Fondasi T-2 dari `a5fbf11` (`QueryTab.swift:1215-1226`, `AppModel+Run.swift:45-48`).
- **Tes `SaveToTableTests.swift`** diperluas dan selalu mulai dari tab baru.
- **`SessionTab.targetConnection: String?`** di-decode bila ada, tanpa migrasi. Tab disimpan sebagai blob JSON (`crates/qh-storage/migrations/0001_init.sql:59-61`), dan engine hanya memeriksa bahwa isinya JSON (`crates/qh-ffi/src/local.rs:424-438`).
  - UUID yang tidak lagi ada saat pemulihan menjadi `nil` dan memblokir run (§7.3).
- **`prepareTableDestination`** (`AppModel+Tree.swift:641-654`) memakai koneksi tujuan. Saat tujuan berganti, bagian yang tidak dimiliki jenis barunya dikosongkan.
- **`handle`** untuk `to_table` membaca `start` baru tanpa memperlakukannya sebagai awal Export (N-11). Transfer tetap di luar `recordHistory` (`AppModel+History.swift:60-66`).

### 8.3 Cek tabrakan

- **`loadTableOptions(for:catalog:schema:)`** menjalankan `tables` dengan env tujuan.
- **Labelnya:** create "exists, Create will fail"; replace "will be replaced".
- **Probe engine (§3.1) yang mengikat.** Cek di app hanya petunjuk.

### 8.4 Progres, Stop, dan ringkasan

- Step baru tampil di log.
- `progress.rows` adalah baris tertulis; `rows_read` tampil di log.
- Teks Stop menjadi "Stopped. Nothing was written to <B>.", atau peringatan scratch.
- `done`: "<N> rows copied from <A> to <B> · <table> (verified)", ditambah peringatan fallback dan pelebaran.

### 8.5 Aturan tata letak (mode kompatibilitas macOS 27)

- Popover dan `Menu` berlebar pasti; popover utama tetap `.frame(width: 400)`.
- Setiap pane yang menggulir memakai `.frame(minHeight: 0, maxHeight: .infinity)` (`ledger.md:640`).
- Blok lintas koneksi masuk **di dalam** ScrollView tetap 396 (`ExportSettings.swift:54-59`, `:108`).
- Latar `Color` tidak mengisi safe area (`ledger.md:656`).
- Lembar transfer (`Views/TransferSheet.swift`) berukuran pasti.
- Gate: build SDK 14 dan `target/run/uiprobe/probe.sh` dengan `QH_SUPPORT_ROOT` sekali pakai di jendela 1400×700.

### 8.6 V-17 dan scene

- Scene `table-target` (`Support/Snapshot.swift:954-956`) tidak punya baseline.
- Scene baru: `table-target-cross`, `table-target-cross-refused`, dan `transfer-sheet` (prod ke tanpa label).
- Baseline ChromeParity terang dan gelap direkam orkestrator sebagai V-17.
- Scene grid tidak bergeser, karena `ResultGrid.swift` tidak disentuh.

## 9. Permukaan FFI dan engine

- **UniFFI:** tidak berubah. G-FFI hanya sebagai bukti netral (`git diff --exit-code -- app/Generated`).
- **Trait driver,** aditif dengan default:
  - `Cursor::column_types`;
  - `Session::copy_in` (default `Usage`);
  - `CopyIn { send(Vec<u8>), finish() -> u64 }`;
  - `Parameter::Bytes(Vec<u8>)` (lengan PostgreSQL dan MySQL).
- **Engine:** `Engine::for_settings` dan `CliError::Sided`.

**Setelan** (hanya di jalur `DEST_*`):

| Setelan | Arti |
|---|---|
| `DEST_*` | koneksi tujuan (§3.2). `DEST_DB_KIND`/`DEST_DB_URL` dan `DEST_SAFE_MODE` wajib. |
| `WRITE_MODE` | `create` atau `replace`; `append` menjadi `Usage` |
| `TRANSFER_FETCH_ROWS`, `TRANSFER_COPY_ROWS`, `TRANSFER_COPY_BYTES`, `TRANSFER_BATCH_ROWS` | §3.4 dan §3.6 |

**Event jalur lintas koneksi:**

| Event | Field |
|---|---|
| `step` | `connect_target`, `connect`, `prepare`, `write`, `verify`, `swap`, dalam urutan itu |
| `start` | `columns`, `query_id` (sumber), `target_columns` (`[{name, type}]`), `fallbacks` (`[{column, from, to}]`) |
| `progress` | `rows` (tertulis dan diakui), `rows_read` |
| `done` | `rows`, `rows_read`, `table`, `mode`, `query_id`, `cancelled`, `warnings`, `verified`, `target_kind` |
| `error` | ditambah `side` opsional. `host_key` tujuan digerbangi `DEST_SSH_HOST_KEY_DETAIL`. |

`Support/EngineEvent.swift` menambah field opsional `rowsRead`, `verified`, `side`, `targetColumns`, dan `fallbacks`.

## 10. Rencana tes

### 10.1 Dengan fake (wajib hijau di langkahnya)

Fake untuk dua sisi ada di `crates/qh-ffi/tests/to_table_cross.rs`, dengan pola `to_table_replace.rs:46-250`:
- `FakeEngine` yang `for_settings`-nya memilih engine menurut `DB_HOST`;
- sesi sumber dengan kursor bernaskah;
- sesi tujuan yang merekam statement, byte `copy_in`, parameter, dan hitungan, dan bisa gagal di statement atau batch ke-k.

**Kasus tes:**

- **B-1:**
  - `DEST_` kosong;
  - `DEST_*` sebagian tanpa `DEST_DB_KIND`;
  - `DEST_SAFE_MODE` hilang, kosong, atau salah eja;
  - `WRITE_MODE=append` bersama `DEST_*`;
  - `DEST_SAFE_MODE_FLOOR` bersama `SAFE_MODE_FLOOR` dengan nilai berbeda, dalam kedua urutan, sehingga keduanya terbukti menjadi entri dan tidak saling menimpa.

  Semuanya `Usage` sebelum `step`, dan **tidak ada statement yang terkirim ke sesi sumber**.
- **B-2** (`safe_mode.rs`): matriks §7.2 untuk setiap statement per mode, per tujuan, per entri lantai. Termasuk `confirm` ditambah lantai `no_ddl`, pembersih scratch di `confirm`, replace di `confirm` yang ditolak sebelum connect, dan create PostgreSQL di `confirm` yang benar-benar mengirim `BEGIN`/`COMMIT` (allowlist kontrol, bukan classifier).
- **B-3:**
  - nama dengan `"`, backtick, newline, `;`, NUL, terlalu panjang, dan berakhir spasi;
  - `SqlType` dengan `native` jahat (misalnya `int); DROP TABLE x; --`) tidak pernah muncul di SQL;
  - OID di luar allowlist menjadi R;
  - pembangun statement palsu yang menghasilkan dua statement atau kind yang salah ditolak dengan `Internal` sebelum dikirim.
- **B-4:** baris fixture untuk tanggal nol MySQL, numeric `NaN`/`Infinity` (PostgreSQL ke PostgreSQL diteruskan, ke MySQL R), timestamp `infinity` dan SM, Trino `time(9)` dan `timestamp(9)`, U+0000, JSON dengan kunci ganda dan urutan, rentang datetime MySQL, `utf8mb4` di DDL, dan numeric tanpa batas ke `decimal(65,30)`.
- **B-5:** perekam urutan memastikan "cancel sumber sebelum kursor sumber di-drop" untuk Stop, galat tujuan di tengah stream, penolakan encoder, penolakan tipe sesudah batch pertama, dan galat `CREATE`/probe tujuan. Verifikasi tidak cocok dites terpisah: kursor sudah selesai, sesi sumber ditutup biasa, dan tujuan di-rollback.
- **Lainnya:**
  - PostgreSQL create dan replace (satu transaksi, jendela per baris, byte, dan waktu dengan jam palsu);
  - MySQL create dan replace (scratch, `RENAME`, pembersih);
  - sumber tanpa kolom;
  - verifikasi;
  - redaksi `DEST_<secret>`;
  - setelan sumber tanpa `DEST_*`;
  - seam host dengan `with_connector` (`host.rs:433-435`);
  - MCP tidak bisa menyuntik `DEST_*`;
  - G-RUST tetap memuat `crates/qh-ffi/tests/golden.rs` offline, sebagai bukti golden `to_table` tidak berubah (N-14).
- **Swift:**
  - env `DEST_` per kunci fixture `connection_env.json`;
  - blokir per jenis tujuan dari tab baru;
  - lembar selalu tampil, termasuk tujuan `full`;
  - centang prod ke tanpa label;
  - UUID yang dihapus;
  - pemulihan sesi lama;
  - `HostKeyGate` dengan `DEST_SSH_HOST`;
  - fixture `ident_quoting.json`.

### 10.2 Hanya live (pending sampai kontainer dibuat ulang)

- **`COPY` nyata:** jendela, hitungan, `CopyFail`, `ROLLBACK` saat koneksi putus, dan `statement_timeout` per jendela.
- **MySQL nyata:** `Bytes`, `max_allowed_packet` kecil, `STRICT_ALL_TABLES`, `RENAME`, `KILL QUERY` di tengah batch, `lower_case_table_names`, dan `utf8mb4`.
- **Deskriptor terhadap server nyata.**
- **Pasangan tipe:** `type_zoo` Trino ke PostgreSQL dan MySQL, PostgreSQL ke MySQL, dan MySQL ke PostgreSQL, dibaca balik sel per sel.
- **Batas DBA** tetap berlaku dengan `STATEMENT_TIMEOUT_MS=0`.
- **SSH di dua sisi** lewat `qh-sshd-dev`, dan toxiproxy `reset_peer` di tujuan.
- **Angka:** rows/s dan RSS untuk 1 juta baris (n = 5, aturan W7).
- **`tools/golden/live_cases.py`.**

## 11. Matriks pasangan

| Sumber ↓ / Tujuan → | PostgreSQL | MySQL | Trino |
|---|---|---|---|
| PostgreSQL | **v1**, termasuk koneksi lain ke server yang sama | **v1** | ditunda |
| MySQL | **v1** | **v1**, termasuk koneksi lain ke server yang sama dengan database berbeda | ditunda |
| Trino | **v1** | **v1** | koneksi yang sama: CTAS; lintas koneksi ditunda |
| UUID yang sama | CTAS server-side, tidak berubah (PR-02) | sama | sama |

**Ditunda:** append (OQ-5), tujuan Trino lintas koneksi, `COPY BINARY`, `LOAD DATA LOCAL`, pemetaan kolom manual, membuat schema, database lain di server PostgreSQL tujuan, "simpan hasil grid", overlap baca-tulis, `COUNT(*)` ulang, pembersih `qh_new_*` yatim, dan optimasi server yang sama (`INSERT … SELECT`).

## 12. Risiko

- **R-1. Kehilangan nilai diam-diam.** Mitigasi: §4.3–§4.4 (setia atau tolak, encoder per varian, `STRICT_ALL_TABLES`, `utf8mb4`), fixture, dan baca balik live.
- **R-2. Rahasia `DEST_` tercetak.** Mitigasi: §7.8, dengan SEC di langkah 2.
- **R-3. Sesi B memakai identitas A.** Mitigasi: §3.3.
- **R-4. Tulisan ke tujuan produksi tanpa pertanyaan, atau CTAS diam-diam di sumber.** Mitigasi: gagal-tertutup B-1 (§3.2), keputusan per statement (§7.2), dan lembar yang selalu tampil (§7.3).
- **R-5. Scratch MySQL tertinggal bila proses mati** (§5.3). Diterima.
- **R-6. Batas statement memotong transfer panjang.** PF-23 terukur (`ledger.md:456`). Mitigasinya §6.6: 0 untuk batas yang diwarisi. Batas DBA tetap bisa memotong, dengan galat `side: source` yang menyebut batasnya.
- **R-7. `idle_in_transaction_session_timeout` di tujuan** (§6.6).
- **R-8. Wrapper lupa meneruskan method baru.** Mitigasi: transfer dites lewat `EngineHost::with_connector`.
- **R-9. Pesan server mengutip nilai.** Diterima dan dicatat di ADR-0046.
- **R-10. Commit editor dan `a5fbf11` belum approved atau belum di-push** (§2.5). Langkah 5 dan 8 bergantung pada versi yang approved.
- **R-11. Rantai driver disusun ulang** (OQ-12). W11-T8 dan sesudahnya bergeser satu mata rantai.
- **R-12. Fitur tampil sebelum terbukti live.** Mitigasi: langkah 8–10 tidak masuk `main` sebelum langkah 12 (§13).
- **R-13. Lisensi.** TablePro dan dbx tidak dibaca.
- **R-14. Volume log.** Satu baris per teks statement yang berbeda (§7.2).
- **R-15. Hak akses B** (§7.12).
- **R-16. Panik di rentang kursor hidup.** Panik ditangkap di batas FFI (`uniffi_api.rs:358-378`), lalu kursor di-drop tanpa cancel. Mitigasi: rentang itu bebas `unwrap`/`expect` (aturan review). Sisa risikonya diakui.

## 13. Urutan build

**Aturan umum:**
- Satu commit per langkah.
- Anggaran bukti: satu tes terarah untuk perilaku yang disentuh, ditambah satu G-RUST atau G-SWIFT penuh di mesin yang menganggur.
- `CARGO_TARGET_DIR` absolut di SSD (`AGENTS.md`).
- **"Fake"** berarti terbukti seluruhnya dengan fake atau tes unit tanpa server.
- **"Pending live"** berarti ada bagian yang hanya bisa dibuktikan di langkah 12.

| Langkah | Isi dan berkas | Gate | Bukti | Review | Commit |
|---|---|---|---|---|---|
| 0 | Orkestrator: §2.6 (plan §5 dan §7 dengan penyusunan ulang rantai driver, PRD, ledger) dan **ADR-0046** | review ringan dokumen | — | Rendah | `docs(plan): schedule Save to Table across connections as W13-T20, with ADR-0046` |
| 1 | Move-only `Replacement`, `discard_scratch`, `table_exists`, `finish_statement` ke `crates/qh-ffi/src/table_swap.rs` | G-RUST (termasuk `golden.rs` offline dan `to_table_replace` offline, identik) | fake | Rendah, digabung ke review batch engine | `refactor(engine): the table swap lives in its own module` |
| 2 | `env.rs`: `prefixed`, `without_prefix`, redaksi `DEST_<secret>` | G-RUST | fake | Tinggi: SEC | `feat(engine): a second connection's settings under DEST_, with its secrets redacted` |
| 3 | `Engine::for_settings` (default `None`), `RealEngine` dan `LongOpEngine`, `PooledEngine` `None`, tes host. Berkas: `lib.rs`, `host.rs`, `crates/qh-ffi/tests/host.rs` | G-RUST; G-FFI hanya bukti netral | fake | Tinggi: RR | `feat(engine): a command can open a second connection with that connection's own settings` |
| 4 | `SqlType` dan `Cursor::column_types`: fungsi pemetaan murni per driver, pemasangan di tiga kursor, diteruskan di `lease.rs` dan `retry.rs`. Rantai driver: sesudah W11-T7 (`d494e9c`), sebelum W11-T8. | G-RUST | fake untuk pemetaan; pemasangan di kursor nyata pending live | Tinggi: RR dan DB | `feat(drivers): describe each result column's type with its precision, length and sign` |
| 5 | `transfer/{types,encode,names}.rs` murni beserta fixture `transfer_types.json` (semua kasus B-4) dan `ident_quoting.json` (dari daftar Swift 316 kata, B-3) | G-RUST | fake | Tinggi: DB; review digabung dengan langkah 7 | `feat(engine): map column types and values between dialects, faithfully or not at all` |
| 6 | `Session::copy_in` dan `CopyIn`, implementasi PostgreSQL, `Parameter::Bytes` (PostgreSQL dan MySQL), `max_allowed_packet`, diteruskan di `lease.rs` | G-RUST; G-DENY bila `Cargo.toml` driver berubah, dengan `Cargo.lock` tetap | fake untuk kompilasi dan unit; perilaku pending live | Tinggi: RR dan DB | `feat(drivers): a bulk copy into PostgreSQL and binary parameters for MySQL` |
| 7 | `transfer/{mod,plan}.rs`: B-1, B-2 per statement, B-3c cek struktur, B-5 `LiveSource`, verifikasi, `side` (`events.rs`, `lib.rs`), konstruktor `table_swap` dan swap PostgreSQL di dalam transaksi (N-1). Tes `to_table_cross.rs`, `safe_mode.rs`, `mcp.rs`. | G-RUST (termasuk `golden.rs` offline) | fake; server nyata pending live | Tinggi: putaran 1 RR (opus), SEC, dan DB untuk batch langkah 1–7; putaran 2 hanya untuk yang memblokir | `feat(engine): save a query's rows into a table on another connection` |
| — | **Push batch 1–7** setelah review langkah 7 approved atau tercatat "pending review" (O-20). Engine diam selama app tidak mengirim `DEST_*`. | — | — | — | — |
| 12 | **Live**, setelah pemilik setuju membuat ulang kontainer, lewat CLI tanpa UI: matriks §10.2, A/B angka flush, dan `live_cases.py` (selisih baru di `docs/golden-deltas.md`) | G-LIVE PostgreSQL, MySQL, Trino, dan SSH; G-GOLDEN | live | Tinggi: DB satu putaran | `test(live): cross-connection Save to Table against real servers` |
| 8 | App: `targetConnectionID`, `SessionTab`, `overrides` (env `DEST_`, aturan batas §6.6), blokir per jenis tujuan di atas `a5fbf11`, `transferRequest` (B-2, B-6), label prod, validasi UUID, `HostKeyGate`, `EngineEvent`, dan teks | G-SWIFT | fake (`MockEngine` dan tes unit) | Tinggi: SR (opus) dan SEC | `feat(app): a table destination can name another connection` |
| 9 | Wizard, lembar transfer, cek tabrakan, teks mode (T-1), dan scene baru | G-SWIFT; G-VIS (tidak ada baseline lama yang bergeser); G-APP dan `--snapshot`; build SDK 14 dan `uiprobe` di 1400×700 | fake dan probe | Sedang: SR dengan daftar periksa UX dan AX | `feat(app): pick the connection a table is saved to` |
| 10 | Baseline ChromeParity V-17 (orkestrator) | rekam lalu bandingkan, hijau; UX dan AX approved | — | (§0.5) | `test(visual): record the cross-connection table target scenes for V-17` |
| 11 | Bagian tujuan tabel di `app/DESIGN.md` | review ringan | — | Rendah | `docs(app): describe saving a result to another connection` |

**Urutan kerja:**
- Langkah 0, lalu langkah 1, 2, dan 5 paralel (berkas terpisah).
- Langkah 3 setelah langkah 2.
- Langkah 4, lalu langkah 6 (berkas driver yang sama).
- Langkah 7 setelah langkah 1–6.
- Push batch 1–7.
- **Langkah 12 menunggu kontainer.**
- Langkah 8–11 boleh dikerjakan di worktree, tetapi **tidak masuk `main` sebelum langkah 12 hijau** (O-21 mem-push setiap commit `main`; OQ-11).
- W13-T20 selesai setelah langkah 11. Sebelum itu statusnya "pending live".

**Batas review:** maksimal dua putaran per batch. Hanya temuan yang memblokir membuka putaran 2.

## 14. Rencana review dan lisensi

- **Batch engine** (langkah 1–7): `rust-reviewer` opus, ditambah `security-reviewer` (redaksi, dua kredensial, B-1, B-2, B-3, MCP) dan `database-reviewer` (tipe, nilai, atomisitas, `COPY`, `RENAME`, verifikasi).
- **Batch app** (langkah 8–9): `swift-reviewer` opus ditambah `security-reviewer`. Langkah 9 dengan daftar periksa UX dan AX.
- **Blueprint ini:** putaran 2 `architect-reviewer`, hanya untuk B-1..B-7.
- **Lisensi:** TablePro dan dbx tidak dibaca, dan tidak ada yang disalin. QueryHive tetap MIT (ADR-0002). Tidak ada crate baru.

## 15. Yang tidak bisa diverifikasi

- Tidak ada build, tes, atau koneksi database. Kontainer `qh-*` tidak dibuat ulang.
- **PF-23 sudah terukur** (`ledger.md:456`). Yang belum diukur: transfer dengan `STATEMENT_TIMEOUT_MS=0` terhadap batas DBA.
- **Rumus presisi MySQL dan typmod PostgreSQL** untuk kolom ekspresi: dibuktikan dengan fixture di langkah 4, dan dengan server di langkah 12.
- **Kebutuhan fitur `sink` di `futures-util`** untuk `CopyInSink` belum dicek dengan kompilasi.
- **Penerusan ejaan khusus PostgreSQL** (§4.4) mengasumsikan teks driver sama dengan teks input PostgreSQL. Dibuktikan live.
- **Status rantai:** W11-T7 sudah mendarat (`d494e9c`). Status W11-T8, W13-T4, dan W12-T10 tetap harus diaudit per artefak di langkah 0 (`AGENTS.md`).

## Pertanyaan terbuka untuk pemilik

Semua sudah **terjawab** lewat delegasi pemilik kepada orkestrator (tabel "Keputusan orkestrator" di Verdict). Teks singkatnya dipertahankan sebagai catatan, dengan bagian tempat keputusan itu diterapkan.

1. **OQ-1, pasangan v1.** **Terjawab:** seperti usulan (§11).
2. **OQ-2, default.** **Terjawab:** koneksi yang sama, mode create (§8.1).
3. **OQ-3, prod ke lebih rendah.** **Terjawab:** peringatan ditambah centang, juga untuk tujuan tanpa label (§7.4).
4. **OQ-4, batas ukuran.** **Terjawab:** tanpa batas, dengan Stop dan progres (§3.4, §8.4).
5. **OQ-5, append.** **Terjawab:** di luar v1, dan engine menolaknya bersama `DEST_*` (§3.1, §3.2, §5).
6. **OQ-6, CLI dan MCP.** **Terjawab:** CLI ya, MCP tidak (§7.9).
7. **OQ-7, fallback teks.** **Terjawab:** hanya sel T dan dilaporkan; numeric tanpa batas ke `decimal(65,30)` dengan cek per nilai (§4.3).
8. **OQ-8, kolom duplikat.** **Terjawab:** ditolak (§4.5).
9. **OQ-9, tujuan `confirm`.** **Terjawab:** create bertanya per statement lewat `destructive_mode`; replace ditolak (§7.2).
10. **OQ-10, presisi Trino > 6.** **Terjawab:** ditolak, untuk timestamp dan time (§4.3).
11. **OQ-11, tampil sebelum live.** **Terjawab:** langkah 8–10 tidak masuk `main` sebelum langkah 12 (§13).
12. **OQ-12, W12-T7c.** **Terjawab:** primitif milik W13-T20; langkah 4 dan 6 sesudah W11-T7, sebelum W11-T8 (§2.4, §3.8).
13. **OQ-13, batas sumber.** **Terjawab:** 0 hanya menggantikan default yang diwarisi; batas eksplisit tetap dipakai (§6.6).

## Verdict architect-reviewer

- **Reviewer:** architect-reviewer (opus), putaran 1, 14 Okt 2026, read-only.
- **Basis kode:** `main` `bf9fcbb` (HEAD). Pohon kerja saat review memuat perbaikan T-2 yang belum di-commit (`AppModel+Run.swift`, `AppModel+Tree.swift`, `QueryTab.swift`, `Connections.swift`, dan `SaveToTableTests.swift` baru). Karena itu setiap kutipan Swift di bawah dibaca dari `git show HEAD:…`, bukan dari pohon kerja.
- **Yang tidak dilakukan:** build, tes, dan pembuatan kontainer.
- **Verdict: perubahan diperlukan (changes required).**
  - Arsitektur intinya diterima (lihat bagian berikut).
  - Tujuh temuan memblokir (B-1..B-7) harus masuk ke blueprint sebelum langkah 0.
  - Putaran 2 hanya untuk ketujuh temuan itu. Yang lain masuk backlog wave.

### Yang diterima, dengan bukti

- **Tanpa perintah FFI baru dan tanpa perubahan `app/Generated/`: benar.**
  - App meneruskan env apa adanya sebagai pasangan `Setting` (`Support/RustEngine.swift:137`), jadi `DEST_*` sampai ke engine tanpa perubahan UniFFI.
  - `to_table` sudah LongOp (`host.rs:251`), dan `engine_for` membangun `LongOpEngine` dari `Settings` yang sama (`host.rs:617-623`).
  - Stop adalah satu `CancelFlag` untuk satu run (`uniffi_api.rs:278-308`), jadi cukup untuk dua sisi.
  - Event `error` dibangun aditif (`events.rs:151-173`).
  - MCP menolak `to_table` berdasar nama (`mcp.rs:1000-1003`). `settings_for` membangun pasangan dari kunci tetap lalu memaksa `read_only` (`mcp.rs:1456-1484`), jadi `DEST_*` tidak bisa disuntik.
  - Lane C mendarat tanpa menyentuh `lib.rs` atau `app/Generated/`: `git log 7c8f1b5..HEAD -- crates/qh-ffi/src/lib.rs app/Generated` hanya menunjukkan `5f7cc4d`.
- **`Engine::for_settings` cocok dengan model pool.**
  - `LongOpEngine::connect` memanggil `open_unleased(config, &self.settings)` (`host.rs:207-210`, `pool.rs:612-620`) dengan `PoolKey` miliknya sendiri (`pool.rs:156`).
  - Sesi unleased tidak punya lease, sehingga tidak pernah reconnect atau mengirim ulang statement (`lease.rs:308-313`, `:344-355`). Sesinya ditutup, tidak dikembalikan ke pool.
- **Atomisitas PostgreSQL dalam satu transaksi: benar.**
  - Satu sesi memegang satu `Client` (`crates/qh-driver-postgres/src/lib.rs:421-424`).
  - Men-drop `CopyInSink` mengirim `CopyFail` (`tokio-postgres-0.7.18/src/copy_in.rs:55-58`), dan `finish()` mengembalikan hitungan baris (`:128`).
- **Scratch dan `RENAME` MySQL W12-T9 ada dan bisa dipakai.** Lihat `commands.rs:1428-1434`, `discard_scratch` di `:1608-1631`, dan peringatan drop backup di `:1708-1718`.
- **R-9 dan NFR-S5 bertahan.**
  - Export dan `to_table` tidak menulis history (`AppModel+History.swift:65`).
  - Hanya Run yang menyimpan `ERROR_TEXT` (`:95`).
  - Langkah 8 wajib menjaga transfer tetap di luar `recordHistory`.
- **Risiko `LOCAL INFILE` tertutup.** mysql_async 0.36.2 mengiklankan `CLIENT_LOCAL_FILES` (`opts/mod.rs:1095`), tetapi menjawab `NoHandler` bila tidak ada handler (`conn/routines/helpers.rs:93`). Tidak ada handler yang dipasang di driver.

### Spot-check `path:baris` (lebih dari 60 klaim dicek)

**Benar di `bf9fcbb`:**

- **`commands.rs`:**
  - `:103-106`, `:124-154`, `:194-201`, `:298-300`, `:323-351`, `:771-788`;
  - `:992-1008`, `:1073-1078`, `:1139`, `:1149`, `:1161-1165`, `:1220`, `:1233-1237`, `:1239-1246`, `:1248-1253`, `:1259`, `:1322-1340`;
  - `:1353-1444` (`plan` di `:1374`; nama di `:1378`; cabang PostgreSQL `:1417-1427`, MySQL `:1428-1434`, Trino `:1435-1443`);
  - `:1472-1477`, `:1569-1605`, `:1608-1631`, `:1635-1720`, `:1938-1944`, `:1990-2008`, `:2027-2071` (komentar `:2065-2067`), `:2770`.
- **`lib.rs`:** `:5-12`, `:14-21`, `:176-179`, `:253-285`, `:364-379`, `:404-413`, `:436-460`.
- **Host dan engine:**
  - `env.rs:44-52`, `:63-76`;
  - `host.rs:207-210`, `:433-435`;
  - `pool.rs:444-459`;
  - `mcp.rs:236`, `:1470-1478`;
  - `events.rs:166-171`.
- **Trait driver** (`crates/qh-driver/src/lib.rs`): `:230-249`, `:257-262`, `:584`, `:734-748`, `:766-771`, `:778-827`.
- **Driver:**
  - PostgreSQL: `lib.rs:711-731`, `:838-840`, `:1128-1137`;
  - MySQL: `lib.rs:793-806`, `:1022-1024`, `normalize.rs:142-175`, `:198-209`;
  - Trino: `lib.rs:653`, `:1419-1435`, `:1634`, `decode.rs:33-49`, `:486-498`.
- **`qh-sql`:** `classify.rs:1193` dan `:1593`; `wrap.rs:58`.
- **Wrapper dan impor:** `lease.rs:90-104`, `:489-505`; `retry.rs:319-325`; `import.rs:687-701`, `:1555-1563`, `:1849-1861`.
- **Lainnya:** `migrations/0001_init.sql:59-61`; tokio-postgres `statement.rs:114` dan `client.rs:630`.
- **Swift:**
  - `AppModel+Run.swift:36-50`, `:127-129`, `:732-739`, `:748-786`;
  - `AppModel+Export.swift:182`, `:184`, `:197`, `:202-205`, `:213`, `:255`, `:274`, `:329`;
  - `QueryTab.swift:88-141`, `:378`, `:404-409`, `:1207-1216`;
  - `Session.swift:20-38`, `:54-57`, `:81-84`;
  - `ExportSettings.swift:36-44`, `:108`, `:129-144`, `:160-229`;
  - `ResultGrid.swift:16`, `:914-931`; `Workspace.swift:422-424`;
  - `RunConfirmation.swift:39-51`; `HostKeyGate.swift:34-51`;
  - `Connections.swift:177-181`, `:376`; `AppModel.swift:353-359`;
  - `AppModel+Connections.swift:907-910`, `:1023-1035`.

**Salah atau basi:**

1. **`AppModel+Tree.swift` bergeser +14 baris.** Penyebabnya `62c8488` dan `ce9ace5` (Lane C) yang mendarat **setelah** `b5df993`. Angka yang benar di HEAD:
   - `prepareTableDestination`: `:641-656` (dikutip `:627-642`);
   - T-2: `:650-653` (dikutip `:636-637`);
   - `loadCatalogs` `:555`, `loadSchemas` `:574`, `isLoadingOptions` `:595-598`, `targetChoices` `:602` (dikutip `:541-575`, `:541-598`, `:560-575`);
   - catatan "app tidak menolak lebih dulu": `:658-665` (dikutip `:647-651`).
2. **§6.6, R-6, T-5, dan §15 "belum diukur" sudah basi.** PF-23 terukur (`target/run/keep/ledger.md:456`): konsumen lambat terpotong `statement_timeout` server, MySQL setelah 282.600 baris dan PostgreSQL setelah 19.400 baris, dan perilaku itu sudah dikunci tes.
3. **§2.5 dan R-10: kekhawatiran Lane C sudah tidak berlaku.** Lane C diduga akan menyentuh `lib.rs` atau meregenerasi `app/Generated/`, tetapi mendarat tanpa keduanya (lihat bukti di atas).
4. **§15, status W11-T7 "tidak diaudit":** W11-T7 sudah ada di `main` sebagai `d494e9c`, dan W11-T6b bagian 1 sebagai `5f7cc4d`.
5. **§1.4 "memuat 40 tes":** `to_table_replace.rs` hanya memuat 14 fungsi tes. Angka 40 dikutip dari `ledger.md:503`, dan dasar hitungnya tidak bisa dicek. Tidak menentukan apa pun.
6. **`main` bukan lagi `c9d82c3`.** HEAD sekarang `bf9fcbb`.

### T-2: terkonfirmasi (dari kode, belum dari app)

**Bukti di HEAD:**

- `runBlockedReason(for:)` mensyaratkan catalog **dan** schema untuk setiap jenis koneksi (`AppModel+Run.swift:45-46`).
- `prepareTableDestination` hanya mengisi schema untuk PostgreSQL dan hanya catalog untuk MySQL (`AppModel+Tree.swift:650-653`).
- Wizard tidak punya field catalog untuk PostgreSQL dan tidak punya field schema untuk MySQL (`ExportSettings.swift:186-195`).
- Ketiga pintu masuk mati karenanya: `ExportSettings.swift:26`, `ResultGrid.swift:911`, dan `AppMenu.swift:106`.
- Engine hanya meminta slot per driver (`commands.rs:1170-1186`), jadi app lebih ketat daripada engine.

**Yang bisa menyamarkan bug ini:** nilai sisa dari sesi yang dipulihkan (`Session.swift:54-55`, `:81-82`), atau dari koneksi Trino/MySQL sebelumnya. Karena itu tesnya harus mulai dari tab baru.

**Akibat untuk langkah 8:** perbaikan T-2 sedang dikerjakan di pohon kerja. Langkah 8 dibangun di atas commit perbaikan itu, dan T-2 keluar dari lingkup langkah 8.

### Temuan memblokir

**B-1. Kontrak `DEST_*` harus gagal-tertutup.**

- **Bukti:**
  - §3.2: transfer diminta "bila `DEST_DB_KIND` atau `DEST_DB_URL` ada".
  - `Settings::text` membaca kosong sebagai tidak ada (`env.rs:10-11`).
  - `SAFE_MODE` yang absen atau kosong menjadi `full` (`commands.rs:161-174`).
- **Dua kegagalan terbuka yang timbul:**
  - Bila app mengirim `DEST_*` sebagian atau kosong, `to_table` diam-diam menjalankan CTAS **di koneksi sumber**, yang bisa saja prod, dengan nama `TARGET_*`.
  - `DEST_SAFE_MODE` yang hilang membuat tujuan berjalan sebagai `full`.
- **Perbaikan:**
  - `transfer::requested` benar bila ada kunci apa pun berprefiks `DEST_` (`is_set`, walaupun kosong).
  - Setelah itu, `DEST_DB_KIND`/`DEST_DB_URL` yang hilang atau kosong menjadi `Usage` sebelum connect.
  - `DEST_SAFE_MODE` wajib ada dan tidak kosong. Sisi baru ini tidak mewarisi aturan lama "absen berarti `full`".
  - `SAFE_MODE_FLOOR` tingkat atas dan `DEST_SAFE_MODE_FLOOR` keduanya menjadi entri floor; yang satu tidak menimpa yang lain.
  - `WRITE_MODE=append` bersama `DEST_*` menjadi `Usage` (OQ-5).
  - Tes untuk setiap poin ada di `to_table_cross.rs` dan `safe_mode.rs`.

**B-2. OQ-9 (create bertanya di `confirm`) diputuskan per statement, lewat `destructive_mode`.**

- **Bukti:**
  - Jalur `guard_confirmed` menolak DDL di `confirm` apa pun isi konfirmasinya (ADR-0026 §1, `commands.rs:228-247`).
  - Hanya `guard_destructive`/`destructive_mode` (`commands.rs:407-450`) yang bertanya untuk DDL di `confirm`. Jalur itu juga membaca **entri** floor, sehingga celah ADR-0027 (`confirm` ditambah floor `no_ddl`) tetap tertutup.
  - Tabel §7.2 masih menulis create "ditolak" di `confirm`.
  - Rencana create MySQL mengirim `CREATE` scratch, `RENAME`, dan saat gagal `DROP TABLE IF EXISTS` scratch (`commands.rs:1405-1406`, `:1429`). Ketiganya DDL.
  - `RunConfirmation.request` mengembalikan `nil` bila ada DDL (`RunConfirmation.swift:43`).
- **Perbaikan:**
  - Matriks §7.2 di `confirm`: create bertanya (satu konfirmasi, `DEST_SAFE_MODE_CONFIRMED=1`), replace tetap ditolak, append tidak ada.
  - Setiap templat rencana create mendapat keputusan yang bernama:
    - DDL lewat semantik `guard_destructive` atas entri floor tujuan;
    - `COPY`/`INSERT` lewat `guard_confirmed` dengan konfirmasi yang sama;
    - `DROP` pembersih untuk scratch milik engine sendiri dicatat dan diizinkan bila `CREATE`-nya diizinkan. Tanpa aturan ini, create di `confirm` tidak pernah bisa selesai atau bersih-bersih.
  - ADR-0046 mencatat amandemen ADR-0026 §1, dan juga asimetrinya: CTAS koneksi yang sama tetap ditolak di `confirm`. Asimetri itu masuk backlog.
  - App memakai permintaan bergaya `destructiveRequest` (`RunConfirmation.swift:61`).

**B-3. SQL tujuan dibangun dari nama dan teks tipe milik server.**

- **Bukti:**
  - §7.2 hanya menjaga **templat**. DDL, kepala `COPY`, `INSERT`, dan swap yang benar-benar dikirim, yang dibangun dari nama kolom pilihan server sumber, tidak pernah diklasifikasi.
  - §4.3 "Other dari PostgreSQL ke PostgreSQL, tipe bawaan `pg_catalog`: nama tipe yang sama E" memasukkan teks tipe dari server (`SqlType.native`, §4.1) ke `CREATE TABLE`. Itu jalur injeksi kedua, dan sebuah domain atau tipe pengguna yang dinamai seperti tipe bawaan menembus cek berbasis nama.
  - `qualifiedName` dan `quotedIdent` di app tidak menggandakan kutip (`SchemaTree.swift:225-251`; backlog ledger 2026-10-14).
- **Perbaikan:**
  - **(a) Teks tipe.** Teks tipe tujuan hanya berasal dari tabel milik engine, dikunci pada `SqlType` yang sudah di-parse. `native` tidak pernah disalin ke SQL. Sel PostgreSQL "Other" menjadi allowlist OID tipe bawaan (tanpa `oid`/`reg*`, yang identitasnya lokal per database), atau R.
  - **(b) Identifier.** Semua identifier lewat satu fungsi Rust yang menggandakan kutip dialeknya bila nama itu tidak polos (semantik `qh_sql::quote_ident`, `crates/qh-sql/src/ident.rs:21-26`). Nama ditolak bila mengandung NUL, lebih dari 63 byte di PostgreSQL, atau lebih dari 64 karakter atau berakhir spasi di MySQL.
  - **(c) Cek struktur sebelum kirim.** Setiap statement nyata harus terbaca sebagai tepat satu statement dengan `StatementKind` yang sama dengan templatnya, di bawah dialek tujuan. Bila tidak, hasilnya `Internal` dan tidak ada yang dikirim.
  - **(d) Lembar konfirmasi app.** Lembar itu memakai `SQLIdentifierQuoting.quoted` (`SQLIdentifierQuoting.swift:31-35`), bukan `qualifiedName`/`quotedIdent`.
  - **(e) Fixture `ident_quoting.json`.** Fixture dibangkitkan dari daftar Swift (316 kata di `bf9fcbb`), dan memuat nama dengan `"`, backtick, NUL, newline, dan `;`.

**B-4. Tabel tipe dan nilai belum lengkap untuk aturan "setia atau tolak".**

- **Bukti per kasus:**
  - Tanggal nol MySQL dan tanggal tidak valid datang sebagai `Value::Text` di kolom date atau datetime (`normalize.rs:349-375`, `:541-543`, tes `:864-874`). Aturan driver juga menyimpan teks untuk nilai apa pun yang tidak bisa di-parse (`normalize.rs:22-24`).
  - PostgreSQL lewat simple query mengirim teks untuk numeric `NaN`/`±Infinity`, timestamp `±infinity`, dan tanggal SM (`crates/qh-driver-postgres/src/lib.rs:11-20`). Tidak ada aturan untuk itu.
  - Trino `time(p>6)` juga menjadi `Unknown` (`decode.rs:486-498`), padahal OQ-10 hanya menyebut timestamp.
  - U+0000 di teks tidak bisa disimpan PostgreSQL.
  - `jsonb` disebut E untuk JSON dari MySQL dan Trino, padahal `jsonb` membuang kunci ganda dan mengubah urutan, sementara `Value::Json` disimpan persis (`value.rs:80-85`).
  - Rentang date dan datetime MySQL (1000 sampai 9999) tidak diperiksa.
  - DDL MySQL tanpa charset mewarisi default database, yang bisa saja latin1.
- **Perbaikan:**
  - Encoder hanya menerima varian `Value` yang diharapkan untuk tiap tipe logis, ditambah `Null`. Varian lain, termasuk `Text` di kolom date, decimal, atau integer, `Unknown`, dan `Interval` di kolom `Time`, ditolak per nilai. Pesannya menyebut kolom dan baris sumber, tanpa nilainya.
  - JSON ke PostgreSQL menjadi `json`, dan `jsonb` hanya untuk sumber `jsonb`.
  - Tabel MySQL dibuat dengan `DEFAULT CHARSET=utf8mb4`.
  - Setiap kasus di atas mendapat baris di `transfer_types.json` sebelum langkah 5 membekukan fixture-nya.

**B-5. Cancel sebelum drop di **setiap** jalan keluar, bukan hanya Stop.**

- **Bukti:**
  - Invariant engine di AGENTS.md.
  - `commands.rs:2065-2067`: producer MySQL berhenti saat kursornya di-drop, sehingga cancel yang dikirim sesudahnya tidak punya apa-apa lagi untuk dinamai.
  - §6.1 hanya menjelaskan Stop. Penolakan tipe di `start` (sesudah batch pertama, saat kursor sumber masih hidup), penolakan encoder, dan galat probe atau tujuan tidak punya urutan yang mengikat. Satu `?` di pipa sudah men-drop kursor lebih dulu.
- **Perbaikan:**
  - Selama kursor sumber belum selesai, setiap jalan keluar lewat `stop_session(source, Some(cursor))` sebelum galatnya dikembalikan. Di rentang itu tidak boleh ada `?`.
  - Tes perekam urutan untuk:
    - Stop;
    - galat tujuan di tengah stream;
    - penolakan encoder;
    - penolakan tipe sesudah batch pertama;
    - galat `CREATE`/probe tujuan.

**B-6. Niat pengguna dan eksfiltrasi: lembar transfer selalu tampil.**

- **Bukti:**
  - `RunConfirmation.request` hanya bertanya di `confirm` (`RunConfirmation.swift:40-41`).
  - §7.3 tidak menyatakan lembar transfer muncul tanpa syarat.
  - §7.4: koneksi tanpa label tidak memicu peringatan, dan label default koneksi baru memang `nil` (`Connections.swift:373`, `:405`).
  - Sesi yang dipulihkan mempersenjatai ulang tujuannya (§8.2).
- **Perbaikan:**
  - Setiap run lintas koneksi menampilkan lembar transfer, apa pun Safe Mode tujuannya, tanpa opsi "jangan tanya lagi". Lembar itu menyebut, untuk sumber dan tujuan: nama, jenis, `host:port`, database, label lingkungan, dan status TLS; ditambah tabel tujuan dan mode.
  - Tujuan disimpan dan diresolusi hanya lewat UUID, lalu divalidasi ulang saat run. Koneksi yang sudah dihapus menjadi `nil` dan memblokir run.
  - Tab hasil pemulihan tidak pernah menjalankan transfer tanpa lembar ini.

**B-7. Blueprint harus menyerap keputusan dan fakta yang berubah.**

- **OQ-5.** Append dihapus dari Ringkasan butir 12, §3.1 langkah 2 dan 5, §5.1, §5.4, §7.2, §9, §10, `types.rs::assignable`, dan pengecualian `confirm` di ADR-0046.
- **OQ-13 dan PF-23.**
  - §6.6, R-6, T-5, dan §15 ditulis ulang dengan PF-23. Dengan bukti itu, `STATEMENT_TIMEOUT_MS=0` di sisi sumber adalah syarat, bukan rekomendasi, untuk batas bawaan yang diwarisi. Batas eksplisit per koneksi tetap dipakai (OQ-13).
  - Dokumen juga harus menyebut arti nilai 0: untuk PostgreSQL, `statement_timeout` menjadi `None` (`commands.rs:103-106`), dan `timeout_statement(None, None)` tidak mengirim apa pun (`crates/qh-driver-postgres/src/lib.rs:1343-1352`). Batas yang dipasang DBA di role atau database tetap berlaku, dan ADR-0046 menyatakannya.
- **Identitas `PoolKey` (§3.2) bertentangan dengan §11.**
  - `PoolKey` memakai `DatabaseSlot::Switchable` untuk MySQL dengan database dan untuk Trino (`pool.rs:175-178`), dan tidak memuat Safe Mode.
  - Akibatnya dua koneksi MySQL tersimpan dengan kredensial yang sama tetapi database berbeda (sel v1 di §11), atau pasangan `read_only` dan `full` ke database PostgreSQL yang sama, akan ditolak.
  - Tentukan identitasnya (jenis, host, port, user, database efektif, dan Safe Mode), atau buang penolakan itu.

### Tidak memblokir (backlog wave)

- **N-1. `Replacement` bukan pakai-ulang murni.** `plan` membangun `CREATE TABLE … AS {body}` (`commands.rs:1405`), dan swap PostgreSQL memuat `BEGIN`/`COMMIT` sendiri (`:1417-1427`). Pakai ulang apa adanya hanya berhasil karena PostgreSQL membaca `BEGIN` bersarang sebagai WARNING.
  - Perbaikan: konstruktor yang menerima statement build, dan swap PostgreSQL tanpa `BEGIN`/`COMMIT` di dalam transaksi yang sudah terbuka.
  - Langkah 1 tetap move-only. Langkah 7 mengubah `table_swap.rs`, dengan gate tes offline `to_table_replace.rs` yang tidak berubah.
- **N-2. `PooledEngine::for_settings` mengembalikan `None`.** Sesi yang di-lease mengirim ulang `BEGIN` setelah reconnect (`lease.rs:52-69`) dan di-reset ke pool. `sql_mode` sesi MySQL di §3.5 bergantung pada sesi yang tidak di-pool.
- **N-3. Kupas `DEST_*` dari `Settings` sumber** sebelum dipakai, sebagai tambahan atas redaksi.
- **N-4. Stop di tujuan PostgreSQL.** Urutannya `cancel_bounded(target.cancel())` saat ada `COPY` atau statement yang berjalan, lalu sink di-drop, lalu `ROLLBACK` dibatasi `STOP_CEILING`, lalu close bila habis waktu. Batas waktu jendela `COPY` dipasang sebagai timer di `select!`, bukan dicek saat batch berikutnya datang (R-7).
- **N-5. Probe.**
  - Probe MySQL memakai `sql_literal` yang menggandakan backslash (`commands.rs:1480-1488`), sehingga salah di `NO_BACKSLASH_ESCAPES`.
  - `information_schema.tables` di PostgreSQL menyembunyikan tabel tanpa hak akses.
  - Keduanya gagal-tertutup tetapi pesannya menyesatkan. Untuk transfer, pakai parameter terikat di MySQL dan `to_regclass` di PostgreSQL.
- **N-6. `side` untuk setiap kelas galat di §6.2.** Penolakan encoder bersisi `target`.
- **N-7. Help replace** menyatakan bahwa tabel baru tidak membawa kunci, indeks, trigger, atau grant. Grant MySQL per nama tidak ikut `RENAME`.
- **N-8. Kasus tepi tipe.**
  - Array PostgreSQL dengan batas bawah bukan 1, dan array bersarang Trino yang tidak rata, ditolak atau dikirim sebagai `jsonb` T.
  - Trino `varchar(n)` dengan n lebih dari 10.485.760 menjadi `text`.
- **N-9. CLI hanya untuk developer.** `build.sh:57-62` hanya membundel `queryhive-mcp`. Harness golden (`tools/golden/live_cases.py:94-127`) dan shell pengembang harus membersihkan `DEST_*`.
- **N-10. Percobaan ulang trust host key untuk tujuan** memberi prefiks ulang pada `DEST_SSH_HOST_KEY_ACCEPT` dan `DEST_SSH_APP_KNOWN_HOSTS` (`tunnel.rs:35`, `:54`, `:236`).
- **N-11. Handler app.** `handle` untuk `to_table` tidak boleh membaca `start` baru sebagai awal Export.
- **N-12. `LOCAL INFILE`.** Handler tidak pernah dipasang, dan tes SEC menguncinya.
- **N-13. ADR-0046 ditulis sebelum langkah 2.** Langkah 2, 6, dan 7 bergantung pada amandemennya (ADR-0026 §1, ADR-0028 `Parameter::Bytes`). Langkah 11 dipindah ke langkah 0.
- **N-14. Golden `to_table` tidak berubah bisa dibuktikan offline.** `crates/qh-ffi/tests/golden.rs` berjalan dengan sesi palsu atas `tests/golden/to_table/`, jadi bukti itu masuk G-RUST. Hanya `live_cases.py` yang menunggu kontainer.
- **N-15. Nomor baris** diperbarui sesuai daftar "salah atau basi" di atas.
- **N-16. Hak akses koneksi tujuan.** Engine menulis dengan kredensial B, dan hanya Safe Mode B yang menggerbangi tulisan itu. Engine tidak bisa melihat grant B. Sisa risikonya: baris yang hanya terbaca lewat identitas A menjadi terbaca oleh siapa pun yang bisa membaca B. Engine tidak punya kendali di sini; mitigasinya lembar B-6 dan OQ-3, dan ADR-0046 menyatakannya.
- **N-17. Rantai Swift §7.** Usulan §2.4 untuk menggabung tangan `QueryTab.swift` (`development-plan.md:1066`), `AppModel+Run.swift` (`:1103`), `RunConfirmation.swift` (`:1086`), dan `Snapshot.swift` (`:1083`) tidak diputuskan sekarang. Langkah 8 sampai 10 ditahan sampai sesudah langkah 12, jadi posisinya diputuskan ulang di langkah 0 terhadap keadaan rantai saat itu.

### Keputusan orkestrator (OQ-1..OQ-13)

Pemilik mendelegasikan semua pertanyaan terbuka ke orkestrator. Keputusannya dicatat sebagai resolved:

| OQ | Keputusan |
|---|---|
| OQ-1 | Pasangan v1 seperti usulan: PostgreSQL, MySQL, dan Trino sebagai sumber, ke tujuan PostgreSQL dan MySQL. Trino sebagai tujuan lintas koneksi ditunda. |
| OQ-2 | Default: koneksi yang sama, mode create. |
| OQ-3 | Peringatan ditambah centang eksplisit. **Disesuaikan:** dipicu bila sumber berlabel prod dan tujuan tidak berlabel prod, **termasuk tanpa label**. Sumber tanpa label tidak memicunya. Lembar B-6 tetap selalu tampil. |
| OQ-4 | Tanpa batas baris atau byte, tetapi Stop dan progres wajib. |
| OQ-5 | Append di luar v1. Engine menolak `WRITE_MODE=append` bersama `DEST_*` (B-1). |
| OQ-6 | CLI ya, MCP tidak. Penolakan MCP yang ada tetap berlaku. |
| OQ-7 | Fallback teks aktif dan dilaporkan per kolom. **Ditegaskan:** hanya untuk sel T di §4.3 setelah B-4, dan sel R tetap R. Numeric PostgreSQL tanpa batas ke `decimal(65,30)` hanya boleh dengan cek per nilai (skala ≤ 30, digit bulat ≤ 35) dan dilaporkan. |
| OQ-8 | Nama kolom ganda ditolak. |
| OQ-9 | Create di tujuan `confirm` bertanya, dengan syarat B-2. Replace di `confirm` tetap ditolak. |
| OQ-10 | Trino `timestamp(p>6)` ditolak dengan pesan jelas. **Diperluas** ke `time(p>6)` dan `time(p>6) with time zone`. |
| OQ-11 | UI ditahan sampai verifikasi live. Karena O-21 mem-push setiap commit `main`, langkah 8 sampai 10 tidak mendarat di `main` sebelum langkah 12. Langkah engine 1 sampai 7 mendarat dalam keadaan diam (app tidak mengirim `DEST_*`). |
| OQ-12 | Primitif `COPY` dan batch dimiliki W13-T20, lalu dipakai ulang W12-T7c. Konsekuensinya rantai driver §7 diubah: langkah 4 dan 6 tepat setelah W11-T7 (`d494e9c`) dan sebelum W11-T8, dicatat sebagai O baru. |
| OQ-13 | Transfer mengirim `STATEMENT_TIMEOUT_MS=0` di sisi sumber. Sekarang wajib karena PF-23. **Disesuaikan:** nilai 0 hanya menggantikan batas bawaan app yang diwarisi. Batas eksplisit per koneksi (`statementTimeoutMS` bukan `nil`, `Connections.swift:374-376`) tetap dipakai dan ditampilkan di wizard, karena itu pilihan sadar pengguna untuk koneksi tersebut. |

### Urutan mulai implementasi (mengingat yang sudah mendarat)

1. **Revisi blueprint** menurut B-1..B-7 dan N-13, lalu review putaran 2 (hanya B-1..B-7).
2. **Langkah 0:** plan §5 dan §7, termasuk penyusunan ulang rantai driver; jawaban OQ ke ledger; dan ADR-0046 yang dipindah ke sini.
3. **Segera setelah langkah 0, paralel karena berkasnya terpisah, semuanya offline:**
   - langkah 1 (move-only `commands.rs` ke `table_swap.rs`; W12-T9 `925e4b6` sudah mendarat, W12-T3 belum);
   - langkah 2 (`env.rs`);
   - langkah 5 (modul murni `transfer/{types,encode,names}.rs` beserta fixture B-3 dan B-4).
4. **Langkah 3** (`lib.rs`, `host.rs`) setelah langkah 2. Lane FFI bebas: `lib.rs` terakhir disentuh `5f7cc4d`, W11-T7 sudah mendarat, dan W12-T3 belum. Tidak ada regenerasi; G-FFI hanya bukti netral.
5. **Langkah 4** setelah penyusunan ulang §7 tercatat.
   - Pemetaan deskriptor ditulis sebagai fungsi murni atas primitif: typmod `i32`, `Column` mysql_common yang dibangun di tes, dan teks tipe Trino. Bagian ini terbukti offline.
   - Pemasangannya di kursor nyata menunggu langkah 12.
6. **Langkah 6** setelah langkah 4 (berkas driver yang sama). Statusnya diam dan "pending live", dan tidak dihitung selesai.
7. **Langkah 7** setelah langkah 1 sampai 6, dengan fake dan G-RUST (termasuk `golden.rs` offline). Reviewnya satu putaran RR, SEC, dan DB untuk batch engine.
8. **Langkah 12** hanya setelah pemilik setuju membuat ulang kontainer, dan dijalankan lewat CLI tanpa UI. Sampai saat itu W13-T20 tercatat "pending live".
9. **Langkah 8 sampai 10** setelah langkah 12 hijau **dan** setelah commit perbaikan T-2 mendarat, karena perbaikan itu sekarang memegang `AppModel+Run.swift`, `AppModel+Tree.swift`, dan `QueryTab.swift`. Sebelum itu boleh dikerjakan di worktree, tetapi tidak di-merge ke `main`.

### Revisi 2

Revisi 2 dibuat 14 Okt 2026 di atas `a5fbf11`. Teks verdict di atas tidak diubah. Setiap temuan memblokir dipetakan ke bagian badan dokumen tempat ia ditangani:

| Temuan | Ditangani di | Isi singkat |
|---|---|---|
| B-1 | §3.2, §3.1 langkah 1, §9, §10.1 | Kunci `DEST_` apa pun memicu jalur transfer, walaupun kosong. `DEST_DB_KIND`/`DEST_DB_URL` dan `DEST_SAFE_MODE` wajib. Dua entri lantai lewat `raise` terpisah, tanpa tabrakan kunci, dengan `destructive_mode` dipecah menjadi fungsi murni atas lantai. `append` bersama `DEST_*` menjadi `Usage`. Tes membuktikan sumber tidak menerima statement apa pun. |
| B-2 | §7.2, §7.3, §2.6 butir 4 | Rencana per mode dan per statement. DDL lewat `destructive_mode` (create bertanya di `confirm`), DML lewat `guard_confirmed`, pembersih scratch diizinkan bersama `CREATE`-nya, replace di `confirm` ditolak, dan kontrol transaksi, setup sesi, serta probe milik engine menjadi allowlist berteks persis di luar classifier (yang membacanya `Unknown`). App memakai `transferRequest` bergaya `destructiveRequest`. Amandemen dan asimetrinya masuk ADR-0046. |
| B-3 | §4.1, §4.3, §4.5, §7.2, §7.3 | `native` tidak pernah masuk SQL; PostgreSQL "Other" menjadi allowlist OID tanpa `oid`/`reg*`. Satu fungsi identifier dengan kutip digandakan dan daftar penolakan. Cek struktur dan klasifikasi per statement nyata. Lembar app memakai `SQLIdentifierQuoting.quoted`. Fixture dari daftar 316 kata. |
| B-4 | §4.3, §4.4, §3.5 | Encoder per varian. Tanggal nol MySQL, `NaN`/`infinity`/SM PostgreSQL, Trino `time(p>6)`, U+0000, `json` vs `jsonb`, rentang datetime MySQL, dan `DEFAULT CHARSET=utf8mb4` masing-masing punya aturan dan baris fixture. |
| B-5 | §6.1, §10.1, R-16 | Rentang kursor hidup dengan satu pintu keluar `LiveSource::finish` dan tanpa `?`. Tes perekam urutan untuk lima jalan keluar yang dites. Verifikasi yang tidak cocok dinyatakan di luar rentang, karena kursor sudah selesai. |
| B-6 | §7.3, §7.4, §8.2, §10.1 | Lembar selalu tampil dan menyebut kedua koneksi beserta `host:port`. Tujuan terikat UUID dan divalidasi ulang. Pemulihan sesi tidak pernah melewati lembar. OQ-3 berlaku juga untuk tujuan tanpa label. |
| B-7 | Ringkasan, §3.1, §3.2, §5, §6.6, §8.1, §9, §11, §12 (R-6), §15, T-5 | Append dihapus. PF-23 dan arti nilai 0 (batas DBA tetap berlaku). Penolakan berbasis `PoolKey` dibuang; jalur CTAS dipilih app menurut UUID. |

**Juga diterapkan:**

- **Keputusan OQ-1..OQ-13** termasuk penyesuaian OQ-3, OQ-7, OQ-10, OQ-11, OQ-12, dan OQ-13 (bagian "Pertanyaan terbuka").
- **Fakta basi dikoreksi:**
  - nomor baris `AppModel+Tree.swift` di `a5fbf11`;
  - W11-T7 (`d494e9c`) sudah mendarat;
  - Lane C mendarat tanpa `lib.rs`;
  - `to_table_replace.rs` memuat 14 fungsi tes;
  - T-2 diperbaiki di `a5fbf11` (§1.10).
- **N-1..N-17** dilipat ke bagian terkait bila murah: N-1 (§2.3, langkah 7), N-2 (§3.3), N-3 (§3.2), N-4 (§3.6, §6.1), N-5 (§3.1), N-6 (§6.2), N-7 (§5.1), N-8 (§4.3), N-9 (§7.9), N-10 (§7.6), N-11 (§8.2), N-12 (§3.5), N-13 (langkah 0), N-14 (§10.1), N-16 (§7.12), N-17 (§2.4).

**Satu penyimpangan yang sengaja, untuk putaran 2.** §4.4 meneruskan ejaan khusus PostgreSQL (`NaN`, `Infinity`, `infinity`, tanggal SM) **hanya** untuk PostgreSQL ke PostgreSQL, bila cocok persis. B-4 meminta setiap `Text` di kolom bertipe ditolak. Alasan penyimpangan:

- tujuannya tipe PostgreSQL yang sama, yang menerima ejaan itu, jadi hasilnya setia;
- menolaknya akan menggagalkan salinan PostgreSQL ke PostgreSQL yang sah;
- ke MySQL nilai itu tetap R.

Bila reviewer menolak, cukup hapus baris pengecualian itu.

### Putaran 2 (architect-reviewer, final)

- **Reviewer:** architect-reviewer (opus), putaran 2 dan terakhir (O-20; tidak ada putaran 3), 14 Okt 2026, read-only.
- **Basis:** `main` `a5fbf11`. Tidak ada build, tes, atau kontainer.
- **Lingkup:** hanya revisi untuk B-1..B-7, penyimpangan B-4 yang diajukan penulis, dua celah tambahan yang dilaporkan penulis, urutan build, dan hal baru yang dibawa revisi.
- **Verdict: approved, dengan syarat C-1..C-5.**
  - Ketujuh temuan memblokir sudah ditangani di badan dokumen, dan klaim kodenya cocok dengan `a5fbf11`.
  - Syarat C-1..C-5 bukan temuan memblokir baru. Kelimanya koreksi kecil: diterapkan orkestrator di langkah 0 tanpa putaran review baru, lalu diperiksa review batch engine (langkah 7) dan DB.
  - Sesuai O-20, statusnya dicatat di ledger sebagai "approved with conditions, pending check at step 7".

#### B-1..B-7: diverifikasi

| Temuan | Status | Bukti di kode |
|---|---|---|
| B-1 | tertutup | `Settings::is_set` ada (`env.rs:107-109`) dan `Settings` berupa peta berkunci tunggal, jadi pemicu "kunci `DEST_` apa pun" butuh satu method baru di `env.rs`, yang memang dimiliki W13-T20. `DEST_SAFE_MODE` wajib menutup default "absen berarti `full`" (`commands.rs:161-172`). Tes "tidak ada statement ke sumber" ada di §10.1. |
| B-2 | tertutup | `destructive_mode` membaca entri lantai (`commands.rs:445-459`). `SafeModeFloor` menumpuk entri yang berulang (`crates/qh-sql/src/classify.rs:330-345`), jadi `raise` terpisah untuk `SAFE_MODE_FLOOR` tingkat atas benar. `destructiveRequest` bertanya di `confirm` dan `full` (`RunConfirmation.swift:61-74`). Replace di `confirm` ditolak sebelum connect. |
| B-3 | tertutup, dengan C-1 | `native` hanya untuk pesan. Allowlist OID tanpa `oid`/`reg*`. Ada satu fungsi identifier, cek struktur per statement nyata, dan fixture dengan `native` jahat. Domain tidak lolos sebagai tipe bawaan, karena RowDescription PostgreSQL mengirim tipe dasar domain (perilaku server, dibuktikan live di langkah 12) dan tipe ekstensi tetap R. |
| B-4 | tertutup, dengan C-2 dan C-3 | Aturan varian ada di §4.4. Kasus-kasusnya ada di normalize PostgreSQL (`normalize.rs:118-160`: `Text` untuk numeric `NaN`, `infinity`, dan SM) dan normalize MySQL (`:345-375`, `:541-543`). Semua kasus punya baris fixture (§10.1). |
| B-5 | tertutup | `LiveSource::finish` adalah satu-satunya pintu keluar, tanpa `?`. Tes urutan mencakup lima jalan keluar, ditambah verifikasi sebagai kasus terpisah. Stop sebelum kursor ada memakai `execute_until_stopped` (`commands.rs:1912`). Panik di batas FFI (`uniffi_api.rs:358-378`) men-drop future tanpa cancel; itu diakui sebagai R-16, dan dapat diterima. |
| B-6 | tertutup | Lembar selalu tampil, dengan `host:port`. Tujuan terikat UUID, divalidasi ulang tepat sebelum run. Pemulihan sesi tidak pernah melewati lembar. OQ-3 juga berlaku untuk tujuan tanpa label (`Connections.swift:373`, `:405`). |
| B-7 | tertutup | Grep atas badan dokumen: append hanya tersisa sebagai "di luar v1" atau `Usage`. Arti 0 benar untuk ketiga driver: PostgreSQL tidak mengirim `SET` (`crates/qh-driver-postgres/src/lib.rs:1343-1352`), MySQL tidak mengirim `max_execution_time` pada sesi baru (`crates/qh-driver-mysql/src/lib.rs:397-400`), dan Trino tidak mengirim header (`crates/qh-driver-trino/src/lib.rs:1358-1360`). Penolakan berbasis `PoolKey` dibuang. |

#### Penyimpangan B-4 penulis: diterima, dengan C-2

Meneruskan ejaan khusus PostgreSQL dari PostgreSQL ke PostgreSQL itu setia **hanya bila** teksnya bentuk ISO yang tidak bergantung pada setelan sesi mana pun.

- **Masalahnya: driver tidak memasang `DateStyle`.** Grep atas `crates/qh-driver-postgres/src` tidak menemukan `DateStyle`, `IntervalStyle`, atau `extra_float_digits`.
  - Role sumber dengan `DateStyle = 'SQL, DMY'` mengirim tanggal SM sebagai `15/03/0044 BC`.
  - Tujuan dengan `MDY` akan membacanya dengan hari dan bulan tertukar, tanpa galat.
- **C-2.** Pengecualian itu berupa regex tertutup yang ditulis di `transfer/encode.rs` dan di fixture, persis begini:
  - numeric: `NaN`, `Infinity`, `-Infinity`;
  - date dan timestamp: `infinity` dan `-infinity`;
  - date ISO SM: `^\d{4,}-\d{2}-\d{2} BC$`;
  - timestamp ISO SM: `^\d{4,}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(\.\d{1,6})? BC$`;
  - timestamptz ISO SM: seperti timestamp, dengan offset `[+-]\d{2}(:\d{2}){0,2}` sebelum ` BC`.

  Bentuk lain tetap R per nilai. Ke MySQL semuanya tetap R.
- **Dengan C-2 dan C-3, menolak seluruhnya tidak perlu.** Hasilnya sama persis dengan input PostgreSQL di sisi tujuan.

#### Celah tambahan yang dilaporkan penulis

**1. Allowlist kontrol di luar classifier: diterima, dengan C-4.**

- **Masalahnya terbukti.** Classifier membaca `BEGIN`, `COMMIT`, `ROLLBACK`, dan `SET` sebagai `Unknown` (`crates/qh-sql/src/classify.rs:1637-1655`). `confirm` menolaknya walaupun dikonfirmasi (`:1805-1807`). Tanpa allowlist, create di `confirm` tidak pernah bisa jalan di PostgreSQL.
- **Bisakah statement jahat menyamar sebagai statement di allowlist?** Tidak, selama yang dibandingkan adalah konstanta milik engine.
  - Statement yang dibangun dari nama selalu memuat nama terkutip di dalam teks `CREATE`/`COPY`/`INSERT`. Teks itu tidak pernah sama dengan `COMMIT` atau `SET NAMES utf8mb4`.
  - Kecocokan teks persis gagal-tertutup: varian dengan spasi, komentar, atau huruf lain tidak cocok, lalu menjadi `Internal`.
- **Apakah allowlist memperluas kemampuan Safe Mode?** Tidak, melebihi yang diputuskan OQ-9.
  - Statement di allowlist hanya diizinkan bila rencana tulisnya diizinkan.
  - Rencana tulis tidak pernah diizinkan di `read_only` atau `no_ddl`, jadi allowlist hanya hidup di `full` dan untuk create di `confirm` yang sudah dikonfirmasi.
  - `SET sql_mode` menambah `STRICT_ALL_TABLES` (lebih ketat), dan `SET NAMES` tidak mengubah data.
- **C-4: allowlist diekspresikan sebagai tipe, bukan sebagai pencocokan string.** Bentuknya `enum EngineStatement { Begin, Commit, Rollback, StrictMode, Utf8mb4, MaxPacket, ProbePg, ProbeMysql }` dengan teks `&'static str`. `plan.rs` tidak pernah menyerahkan `String` hasil `format!` ke cabang allowlist. Tes memastikan setiap varian terbaca tepat satu statement di bawah dialeknya.

**2. Tabrakan `SAFE_MODE_FLOOR` dan `DEST_SAFE_MODE_FLOOR`: diterima.**

- `raise` terpisah dan `destructive_mode` sebagai fungsi murni atas `&SafeModeFloor` itu benar. `guard_destructive` yang ada tidak berubah perilakunya.
- **Satu syarat di C-4.** Mode untuk DML (`guard_confirmed`) diambil dari `resolve()` lantai gabungan tujuan, **tidak pernah** dari `safe_mode(&dest, engine)`. Fungsi itu membangun lantainya sendiri tanpa `SAFE_MODE_FLOOR` tingkat atas (`commands.rs:161-167`). Tes §10.1 dengan dua nilai dalam kedua urutan juga mencakup `COPY`/`INSERT`, bukan hanya DDL.

#### Hal baru dari revisi

- **C-1. `money` dikeluarkan dari allowlist bawaan (§4.3), menjadi R.** Teks `money` bergantung pada `lc_monetary` sesi sumber saat ditulis, dan pada `lc_monetary` tujuan saat dibaca. Dua server dengan locale berbeda mengubah nilainya tanpa galat.
- **C-3. Sesi sumber PostgreSQL memasang setelan bacaan yang tetap.** Setelannya:
  - `SET extra_float_digits = 3`;
  - `SET DateStyle = ISO`;
  - `SET IntervalStyle = postgres`;
  - `SET bytea_output = hex`.

  Keempatnya statement milik engine di allowlist C-4 sisi sumber, dikirim setelah `enforce_read_only` (`SET` sesi tetap boleh di transaksi read-only), di sesi unleased yang ditutup sesudahnya.

  Alasannya: role dengan `extra_float_digits` di bawah 1 mengirim float dan geometri yang dibulatkan, dan driver mem-parse teks itu apa adanya (`normalize.rs:130-138`). Itu kehilangan nilai diam-diam (R-1). `bytea_output = escape` sendiri hanya akan berakhir R per nilai (`parse_bytea`, `normalize.rs:262-266`), tetapi ikut dipasang supaya transfer tidak gagal tanpa perlu.
- **C-5. Pernyataan `sql_mode` MySQL.**
  - `CONCAT(@@sql_mode, ',STRICT_ALL_TABLES')` meninggalkan koma di depan bila `sql_mode` kosong, dan penerimaannya oleh MySQL belum dicek.
  - Pakai `CONCAT_WS(',', NULLIF(@@sql_mode, ''), 'STRICT_ALL_TABLES')`, dan buktikan dengan `sql_mode` kosong di langkah 12.
- **Tidak memblokir:**
  - Parameter `to_regclass($1)` harus nama berkualifikasi yang sudah dikutip oleh `names.rs`, karena `to_regclass` melipat huruf pada teks tanpa kutip. Probe yang salah tetap gagal-tertutup (`CREATE` gagal), tetapi pesannya menyesatkan.
  - `to_regclass` juga menemukan view. Replace atas sebuah view gagal di `DROP TABLE`, lalu `ROLLBACK`. Itu benar, dan cukup disebut di help.

#### Urutan build dan gate

- **Konsisten dengan plan §7,** dengan penyusunan ulang rantai driver yang dicatat sebagai O baru di langkah 0:
  - langkah 4 dan 6 sesudah W11-T7 (`d494e9c`) dan sebelum W11-T8;
  - `lease.rs` dan `retry.rs` sesudah W11-T7;
  - `commands.rs`, `host.rs`, `events.rs`, dan `tests/safe_mode.rs` sebelum W12-T3, yang belum mendarat (`COMMANDS` belum memuat `script`);
  - `lib.rs` tanpa regenerasi.
- **Pending live:**
  - langkah 4 (pemasangan deskriptor di kursor nyata);
  - langkah 6 (perilaku `COPY`, `Bytes`, dan `max_allowed_packet`);
  - langkah 7 terhadap server nyata;
  - C-2 dan C-3 terhadap server dengan `DateStyle` dan `extra_float_digits` bukan bawaan;
  - C-5.

  Semuanya dibuktikan di langkah 12. Sampai saat itu W13-T20 tercatat "pending live" dan tidak selesai.
- **OQ-11 dipegang.** Langkah 8 sampai 11 tidak masuk `main` sebelum langkah 12 hijau, karena O-21 mem-push setiap commit `main`. Pekerjaan app boleh di worktree.
- **R-10 tetap berlaku.** `a5fbf11` dan commit lane editor belum di-push dan belum approved. Langkah 5 (fixture dari daftar Swift) dan langkah 8 menunggu versi yang approved.

#### Urutan mulai implementasi

1. **Langkah 0:** plan §5 dan §7 (dengan O baru untuk rantai driver), PRD, ledger, ADR-0046, dan C-1..C-5 dilipat ke badan dokumen.
2. **Paralel, semuanya fake:**
   - langkah 1 (move-only);
   - langkah 2 (`env.rs`, dengan pemicu `DEST_` dan redaksi);
   - langkah 5 (modul murni dan fixture, termasuk C-1 dan C-2), begitu daftar Swift approved.
3. **Langkah 3** setelah langkah 2.
4. **Langkah 4, lalu langkah 6.**
5. **Langkah 7** setelah langkah 1 sampai 6, termasuk allowlist bertipe C-4 dan setelan sumber C-3. Satu putaran review RR, SEC, dan DB untuk batch 1 sampai 7, lalu push batch.
6. **Langkah 12** setelah pemilik mengizinkan kontainer dibuat ulang, lewat CLI.
7. **Langkah 8 sampai 11** sesudah langkah 12 hijau.
