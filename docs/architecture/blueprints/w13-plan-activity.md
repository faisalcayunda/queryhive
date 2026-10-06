# Blueprint W13: paginasi, rencana query, aktivitas server, penampil audit, dan katalog string

- **Status:** blueprint tingkat berkas, 6 Okt 2026 (W13-A1). Belum ada kode yang ditulis. Menunggu verdict `architect-reviewer` (bagian terakhir dibiarkan kosong).
- **Untuk:** W13-T1 (paginasi, "Tampilkan SQL"), W13-T2 (EXPLAIN JSON dan ANALYZE di engine), W13-T3 (pohon rencana), W13-T4 dan W13-T5 (aktivitas server), W13-T6 (penampil execution log), W13-T7 (katalog string). W13-T8a, T8b, dan T8c **tidak dirancang ulang di sini**: desainnya ada di `blueprints/fase-6-data-plane.md` §14 dan §21.6 sampai §21.8, dan §10 dokumen ini hanya memuat titik temu.
- **Sumber:** `development-plan.md` §4 (baris blueprint ini), §5 (W13), §7 (kepemilikan berkas); PRD FR-GRID-02 dan 05, FR-PLAN-01 dan 02, FR-SAFE-02 dan 03, FR-UI-11, NFR-S1, NFR-C, UC-04, UC-07, UC-10, UC-17, P-05, P-06, P-07, P-12, P-19, P-20, V-11; `tablepro-feature-map.md` §1.4, §1.10, §1.11, dan §1.13; `tablepro-design-audit.md` §17 (lokalisasi); ADR-0016, 0017, 0026, 0027; `docs/invariants.md` #7, #8, #11.
- **Bukti:** kode dikutip dari commit `8103478` (cabang `work/perf-parity`, pohon bersih). Nomor baris Swift bergeser setelah W9-T0 memecah `AppModel.swift`, jadi tiap rujukan Swift menyebut nama fungsinya juga. Server dev yang disentuh, semuanya hanya baca atau dengan sesi uji milik sendiri: Trino 483 (`127.0.0.1:58080`), PostgreSQL 17.11 (`55432`), MySQL 8.4.11 (`53306`). Blueprint saudara yang ditulis bersamaan dan belum di-commit dibaca untuk keselarasan: `w9-shell-and-a11y.md`, `w11-metadata-and-connections.md`, `w12-editor-and-run.md` (rujukan ke isinya mengikuti teks hari ini dan bisa bergeser). Kode TablePro (AGPL) dibaca sebagai ide saja (cara membaca waktu per node plan, pita severity per bagian biaya, pelajaran `LIMIT` dari estimasi); tidak ada kode, aset, atau string yang disalin.
- **Bahasa:** dokumen ini Indonesia. Identifier, komentar kode, pesan log, string yang tampil di app, dan nama tes Inggris.

## Ringkasan

W13 menambah lima hal kecil-ke-sedang di atas engine dan grid yang sudah ada, dan memeriksa dua sambungan yang ternyata belum ada. Tujuh temuan menentukan rancangan:

1. **Trino 483 tidak mendukung `EXPLAIN ANALYZE (FORMAT JSON)`.** Terbukti lewat HTTP: `SYNTAX_ERROR`, `mismatched input 'FORMAT'` (§1.1). ANALYZE di Trino hanya teks, sesuai cadangan rencana. `EXPLAIN (TYPE DISTRIBUTED, FORMAT JSON)` tanpa ANALYZE bekerja dan hasilnya adalah objek berkunci id fragmen.
2. **App tidak pernah menulis execution log.** Sink-nya (`execution_log::install_from_settings`) hanya dipasang oleh dua binary, `queryhive-engine` (`crates/qh-ffi/src/main.rs:66`) dan `queryhive-mcp` (`crates/qh-ffi/src/bin/mcp.rs:91`). Host dalam proses yang dipakai app (`RustEngine`, `EngineHost`) tidak memasangnya, dan tidak ada panggilan Swift yang melakukannya. W11-A1 menemukan hal yang sama secara independen dan memutuskan memasangnya malas di `EngineHost` (W11 D-8, §3.5). W13 bergantung pada itu: tanpanya penampil W13-T6 kosong untuk semua Run dari app, dan "setiap cancel dicatat" (FR-SAFE-03) tidak punya tempat menulis (§8.1).
3. **Execution log tidak punya kolom koneksi** (`crates/qh-storage/migrations/0007_execution_log.sql`), dan app tidak mengirim `CONNECTION_ID` ke perintah Run. "Filter per koneksi" di FR-SAFE-02 tidak bisa dipenuhi tanpa migrasi 0009 dan versi rantai baru. W11 §3.5 menunda keputusan itu ke W13, jadi blueprint ini mendefinisikannya sebagai sub-tugas W13-T4b (§8.2), dengan persetujuan pemilik (OQ-3).
4. **Paginasi = menjalankan ulang statement dengan cap lebih tinggi (P-07), dan itu berbahaya untuk statement yang menulis.** `INSERT … RETURNING` dengan `LIMIT` terpotong akan ditulis dua kali oleh "Ambil lebih banyak". Aksi paginasi hanya boleh untuk statement yang terbaca sebagai baca (§3.2). Selain itu `rowLimit` disimpan di sesi, jadi "Ambil semua" di Trino produksi akan menjadi default Run berikutnya kecuali dipulihkan (§3.4).
5. **Guard `explain` sudah menilai statement milik pemanggil sebelum awalan `EXPLAIN` dipasang** (`commands.rs:1590`, awalan di `:1601`). Karena itu "ANALYZE dinilai seperti statement di dalamnya" (FR-PLAN-01) terpenuhi tanpa mengubah perilaku `classify.rs`. Yang tersisa adalah keputusan yang lebih keras: ANALYZE **mengeksekusi** statement, jadi blueprint ini mengusulkan ANALYZE hanya untuk baca (D-10, butuh persetujuan pemilik).
6. **Perintah aktivitas dirutekan ke lajur metadata, karena ongkos koneksi, bukan karena antrean.** Pool query punya 2 sesi dan pool metadata 1 sesi (`pool.rs:34-36`), dan pool adalah reservasi, bukan antrean: checkout tidak pernah menunggu Run lain selesai, dan bila lajur penuh sesi baru dibuka di luar reservasi (`pool.rs:3-7`, checkout di `:486-581`; PRD O-7). Poll aktivitas karena itu tidak akan menunggu di lajur mana pun. Lajur metadata memakai ulang satu sesi hangat tiap poll 3 detik dan tidak bersaing dengan dua reservasi Run; di lajur query, selama kedua sesi query dipegang statement liar, setiap poll membuka lalu menutup koneksi baru (connect, TLS, SSH tiap tick) dan menambah koneksi ke server produksi yang sudah terbebani (§6.1).
7. **Katalog string tidak memakai resource SwiftPM.** `Bundle.module` pada app yang dirakit `build.sh` belum pernah dicoba dan akses resource-nya berisiko (butir 8 di §10 `development-plan.md`). Rancangan: `Localizable.xcstrings` di `app/Resources/`, dikompilasi `build.sh` dengan `xcstringstool` (ada di Xcode di mesin ini) ke `Contents/Resources/<bahasa>.lproj`, dan dibaca lewat `Bundle.main` (§9).

## 1. Fakta yang diperiksa sebelum merancang

### 1.1 Trino 483: EXPLAIN dan EXPLAIN ANALYZE (jawaban persis)

Diuji 6 Okt 2026 lewat protokol HTTP (`POST /v1/statement`, header `X-Trino-User: qh`, mengikuti `nextUri`) terhadap `qh-trino` (`/v1/info`: `nodeVersion 483`, `coordinator: true`, `environment: docker`, `Up 2 days (healthy)`). Skrip uji ada di scratchpad sesi, bukan di repo.

| Statement | Hasil |
|---|---|
| `EXPLAIN ANALYZE (FORMAT JSON) SELECT 1` | **`FAILED`, `SYNTAX_ERROR`, errorCode 1:** `line 1:18: mismatched input 'FORMAT'. Expecting: '(', 'SELECT', 'TABLE', 'VALUES'` |
| `EXPLAIN ANALYZE (FORMAT TEXT) SELECT 1` | `SYNTAX_ERROR`, pesan yang sama (kolom 18) |
| `EXPLAIN ANALYZE VERBOSE (FORMAT JSON) SELECT 1` | `SYNTAX_ERROR`, `line 1:26 mismatched input 'FORMAT'` |
| `EXPLAIN (ANALYZE, FORMAT JSON) SELECT 1` | `SYNTAX_ERROR`: `line 1:10: mismatched input 'ANALYZE'. Expecting: '(', 'FORMAT', 'SELECT', 'TABLE', 'TYPE', 'VALUES'` |
| `EXPLAIN ANALYZE SELECT 1` | **berhasil.** Satu kolom `Query Plan varchar`, satu baris berisi teks multi-baris (`Trino version: 483`, `Queued: …, Analysis: …, Planning: …, Execution: …`, lalu `Fragment 1 [SINGLE]` dengan `CPU:`, `Scheduled:`, `Blocked`, `Input:`, `Output:`, `Peak Memory:`, dan per node `Estimates:`, `CPU: … (?%)`, `Input avg.:`) |
| `EXPLAIN ANALYZE VERBOSE SELECT 1` | berhasil, teks yang sama ditambah distribusi buffer dan task |
| `EXPLAIN (FORMAT JSON) <join+agregat>` | berhasil. Satu kolom `Query Plan varchar(8397)`, satu baris, JSON berindentasi |
| `EXPLAIN (TYPE DISTRIBUTED, FORMAT JSON) …` | berhasil, bentuk sama dengan baris di atas (**TYPE bawaan adalah DISTRIBUTED**) |
| `EXPLAIN (TYPE LOGICAL, FORMAT JSON) …` | berhasil, objek dengan satu kunci `"0"` |
| `EXPLAIN (TYPE IO, FORMAT JSON) …` | berhasil, bentuk lain (`inputTableColumnInfos`), tidak dipakai |

**Jawaban:** `EXPLAIN ANALYZE (FORMAT JSON)` tidak didukung di Trino 483. Tata bahasanya menerima setelah `EXPLAIN ANALYZE` hanya `VERBOSE` opsional lalu sebuah statement, dan daftar opsi `( … )` hanya menerima `FORMAT` dan `TYPE`, bukan `ANALYZE`. **Cadangan yang berlaku:** `EXPLAIN ANALYZE [VERBOSE] <sql>` dan hasilnya teks. Rencana yang di-JSON-kan hanya tanpa ANALYZE.

Bentuk JSON tanpa ANALYZE (diperiksa pada dua rencana, satu `tiny` dan satu `sf1`):

- Akar adalah **objek berkunci id fragmen sebagai string** (`"0"`, `"1"`, …), bahkan untuk `TYPE LOGICAL` (satu kunci).
- Tiap nilai adalah node: `id` (string), `name`, `descriptor` (objek string ke string, mis. `{"table":"tpch:tiny:customer","filterPredicate":"…"}`), `outputs` (`[{type,name}]`), `details` (array string, mis. `"Distribution: REPLICATED"`), `estimates` (array berisi nol atau satu objek `{outputRowCount, outputSizeInBytes, cpuCost, memoryCost, networkCost}`), dan `children`.
- Node `RemoteSource` dan `RemoteMerge` menaut ke fragmen lain lewat `descriptor.sourceFragmentIds`, sebuah **string** `"[2]"`, bukan array.
- Angka biaya bisa berupa string `"NaN"` (sering), dan `estimates` bisa `[]` (selalu untuk `RemoteSource`). Biaya per node bukan kumulatif pada dua rencana yang diperiksa (node `Output` punya `cpuCost` 0,0 sementara anaknya 28.467,96), tetapi ini kesimpulan dari dua contoh, bukan dari dokumentasi Trino.
- Tidak ada angka waktu aktual di JSON mana pun.

`GET /v1/query/<id>` (REST `QueryInfo`) menjawab tanpa autentikasi di server dev dan memuat kunci `stages`, jadi statistik aktual per stage secara prinsip bisa diambil lewat jalur itu. Isi `stages` tidak diperiksa dan jawabannya sudah hilang pada panggilan kedua (query dikeluarkan dari riwayat). Jalur itu transport lain dari yang dipakai driver dan **tidak dibangun di W13**.

### 1.2 Trino: aktivitas dan cancel

- `system.runtime.queries` terbaca dengan 15 kolom: `query_id, state, user, source, query, resource_group_id, queued_time_ms, analysis_time_ms, planning_time_ms, created, started, last_heartbeat, end, error_type, error_code`. `user` dan `end` dikutip di SQL. Stempel waktu kembali sebagai teks `2026-10-06 00:09:44.225 UTC`.
- Tidak ada `current_query_id()` (`FUNCTION_NOT_FOUND`). Query pendaftar daftar itu sendiri muncul sebagai `RUNNING`, jadi penyaringnya dikerjakan di Rust dengan `session.query_id()`, bukan di SQL.
- `date_diff('millisecond', started, now())` bisa negatif beberapa milidetik (terlihat −31 pada query pendaftarnya sendiri). Di-clamp ke 0.
- `CALL system.runtime.kill_query(query_id => 'x', message => 'y')` ada: untuk id yang tidak ada, jawabannya `NOT_FOUND`, `Target query not found: …`. Mematikan query yang hidup **tidak diuji** (hanya id yang tidak ada), dan jalur `ACCESS_DENIED` untuk query milik pengguna lain tidak diuji.
- Driver Trino tidak mengirim `X-Trino-Source` (`grep -i x-trino` di `crates/qh-driver-trino/src/lib.rs` hanya menemukan `User` `:586`, `Session` `:705`, `Catalog` `:708`, `Schema` `:711`, dan `Client-Capabilities` `:165`), jadi kolom `source` untuk query QueryHive `null`. Satu query bertanda `queryhive` terlihat `FINISHING` selama 146 detik di server dev; tidak diselidiki.

### 1.3 PostgreSQL 17.11 dan MySQL 8.4.11

PostgreSQL (container `qh-postgres`, peran `qh` adalah superuser, `pg_signal_backend` dan `pg_read_all_stats` anggota):

- `EXPLAIN (FORMAT JSON) SELECT 1` mengembalikan satu baris, satu kolom `QUERY PLAN` bertipe `json`: array dengan satu objek yang memuat `"Plan"`. Dengan `ANALYZE` objek itu juga memuat `"Planning"`, `"Planning Time"`, `"Triggers"`, `"Execution Time"`. Dengan `TIMING OFF` kunci `Actual Total Time` hilang tetapi `Actual Rows` tetap ada.
- Anak node ada di `"Plans"`, dengan `"Parent Relationship"` (`Outer`, `Inner`, juga `InitPlan` dan `SubPlan`). `Actual Total Time` adalah rata-rata **per loop** dalam milidetik dan `Actual Loops` mengalikannya (terlihat: node `Memoize` dengan `Actual Loops: 3`). Driver menyimpan `json` apa adanya sebagai `Value::Json(text)` (`crates/qh-driver-postgres/src/normalize.rs:119`), jadi app menerima JSON utuh sebagai satu sel teks.
- Waktu dan biaya antar-node, diukur pada dua rencana dengan `InitPlan`, `SubPlan`, dan `LIMIT`: waktu node `InitPlan` **tidak** termasuk dalam waktu induknya (induk 0,167 ms, dua `InitPlan` 0,114 dan 0,022 ms), waktu `SubPlan` **termasuk** (`Seq Scan` 5,901 ms memuat `SubPlan` 2,959 ms), dan node `Limit` punya `Total Cost` jauh lebih kecil daripada anaknya (9,85 melawan 4.985,72; 1.171,30 melawan 91.720,35). Ini dasar aturan §5.1.
- **ANALYZE menulis sungguhan.** Pada tabel temp sesi (hilang bersama sesi), `EXPLAIN (ANALYZE, FORMAT JSON) INSERT INTO t VALUES (1)` diikuti `SELECT count(*)` memberi 1, dan `EXPLAIN INSERT INTO t VALUES (2)` biasa sesudahnya tetap 1. Itu dasar D-10.
- `pg_stat_activity` dan `pg_blocking_pids(pid)` diuji dengan sesi uji milik sendiri (`pg_sleep(12)`, `application_name = w13_probe`): baris muncul dengan `wait_event_type:wait_event = Timeout:PgSleep`, umur query sekitar 2.000 ms; `pg_cancel_backend(pid)` mengembalikan `t` dan sesi uji berhenti dengan `ERROR: canceling statement due to user request`. `pg_cancel_backend(0)` mengembalikan `f` dengan `WARNING: PID 0 is not a PostgreSQL backend process`. Driver sudah memasang `application_name("QueryHive")` (`crates/qh-driver-postgres/src/lib.rs:205`), jadi sesi app bisa ditandai.

MySQL (container `qh-mysql`, server 8.4.11, `performance_schema` hidup):

- `information_schema.PROCESSLIST` terbaca. Untuk pengguna `qh` yang tidak punya hak `PROCESS`, hanya sesinya sendiri yang terlihat. `sys.innodb_lock_waits` dan `performance_schema.data_lock_waits` **ditolak** untuk `qh` dengan `ERROR 1142 (42000) SELECT command denied`, dan kosong (tanpa galat) untuk root karena tidak ada lock yang menunggu. Jadi daftar kunci harus pembacaan opsional yang gagal dengan sopan.
- `KILL QUERY <id>` diuji dengan sesi uji `SELECT SLEEP(12)`: sesi uji kembali dengan `1` (nilai `SLEEP` yang terinterupsi) alih-alih menunggu 12 detik. Thread daemon `event_scheduler` muncul di daftar dan harus disaring (`COMMAND <> 'Daemon'`).

### 1.4 Engine: jalur `explain`, guard, dan pendaftaran perintah

- `commands.rs:1579-1657` `explain`: `source_sql` → `safe_mode` → **`guard_for(settings, mode, &sql)` pada statement pemanggil (`:1590`)** → `statement_timeout` → `connection` → `open` → `session.explain_statement(&sql)` (`:1601`) → `execute_until_stopped` dengan `row_limit: None`. Awalan `EXPLAIN` dipasang sesudah guard dan driver yang mengejanya (`PostgreSQL :663-671`, `MySQL :802-807`, `Trino :1033` dan `explain_sql :1396`). Trait `Session::explain_statement(&self, sql) -> String` ada di `crates/qh-driver/src/lib.rs:506`.
- `host/lease.rs:494-505` meneruskan `explain_statement` ke sesi dalam, dengan cadangan `format!("EXPLAIN {sql}")`. Setiap metode `Session` baru yang tidak diteruskan di sini jatuh ke implementasi bawaan trait. Untuk `explain_statement_with` itu bisa berarti EXPLAIN biasa tanpa ANALYZE yang lolos diam-diam, jadi §4.2 membuat bawaannya gagal keras.
- Sembilan implementasi `Session` palsu mengimplementasikan `explain_statement`: tujuh di `crates/qh-ffi/tests` (`golden.rs:387`, `host.rs:383`, `store_sink.rs:125`, `apply_changes.rs:194`, `import_sql.rs:180`, `safe_mode.rs:646`, `retry.rs:179`), satu di `crates/qh-driver/src/lib.rs:816`, dan satu di `crates/qh-ffi/src/retry.rs:492`. Metode trait baru **dengan implementasi bawaan** tidak menyentuh satu pun dari mereka. Struct `Capabilities` dibangun dengan literal di sekitar sepuluh tempat (tiga driver, `qh-driver/src/lib.rs:748` dan `:774`, `apply_changes.rs:90`, `import_sql.rs:76`, `retry.rs:137` dan `:216`, `src/retry.rs:451`), jadi menambah field ke sana akan menyentuh semuanya; rancangan di §4.2 menghindarinya.
- `host.rs:224-249` `route` tanpa wildcard: perintah baru tidak terkompilasi sampai dirutekan. Lajur: `Pooled(Metadata)` untuk `catalogs`, `schemas`, `tables`, `objects`; `Pooled(Query)` untuk `preview`, `explain`, `count`, `apply_changes`, `table_op`. `QUERY_SESSIONS = 2` dan `METADATA_SESSIONS = 1` (`host/pool.rs:34-36`).
- Pendaftaran perintah (invariant #11): `COMMANDS` (`lib.rs:462`, 26 entri, "perintah baru hanya boleh ditambahkan sebelum `objects`", `lib.rs:457`), enum `Command` (`lib.rs:424`), `EngineCommand` dan `EVERY_COMMAND` (`uniffi_api.rs:142`, panjang 26), dan `RustEngine.commands`. Sudah ada perintah bernama `session` (penyimpanan sesi jendela, `local.rs:439`), satu huruf dari `sessions` (§6.1).
- `db_drivers` (`commands.rs:694-712`) menyusun JSON dari field yang disebut satu per satu, jadi menambah kemampuan di Rust tidak mengubah keluaran golden.
- `STATEMENT_TIMEOUT_MS` selalu dikirim app dengan bawaan 60.000 ms (`AppModel.swift:3377`, ADR-0016), dan berlaku untuk ANALYZE.
- `qh-ffi/src/mcp.rs:954-959` menyusun setelan `explain` dari kunci tetap (`SQL`), sehingga MCP tidak bisa menyalakan `EXPLAIN_ANALYZE`.

### 1.5 Classifier Safe Mode

`classify.rs:494-516`: untuk awalan `EXPLAIN` (dan `SELECT`, `WITH`, …) setiap kata telanjang diperiksa dan yang paling ketat menang. `ANALYZE` dan `ANALYSE` ada di `DDL_WORDS` (`:579-582`), jadi `EXPLAIN ANALYZE SELECT 1` adalah `Ddl` (tes `:1214`, `:1847`), `EXPLAIN ANALYZE EXECUTE p(1)` adalah `Unknown` (`:1838`), dan `EXPLAIN /* x */ -- note\r ANALYZE INSERT …` adalah tulis (tes `:1756`, kasus yang pernah lolos di W3-T0b). `EXPLAIN (FORMAT JSON) SELECT 1` dan `EXPLAIN (TYPE DISTRIBUTED, FORMAT JSON) SELECT 1` tidak mengandung kata tulis, jadi menurut pembacaan kode ini terbaca `ReadOnly` (dibaca dari kode, belum dijalankan; tes T2 mengunci keduanya, §4.5). Matriks keputusan (`classify.rs:202-221`): `Dml` → `Full` boleh, `NoDdl` boleh, `Confirm` bertanya, `ReadOnly` menolak. Urutan ketat `ReadOnly > Confirm > NoDdl > Full` monoton terhadap keputusan itu, sehingga mode terselesaikan sudah cukup untuk guard cancel (D-13).

### 1.6 App: jalur explain, cap baris, footer

- `AppModel.explain(_:from:confirmed:)` (`AppModel.swift:2936-3012`) memanggil `Engine.current.run("explain", …)`, menumpuk baris di `PreviewResult.rows`, dan menyalakan `tab.showingPlan`. Setelah W6-T1 jalur itu `runIntoStore("explain", …)` dan loop penumpukan hilang (`fase-6` §17.4), sehingga JSON rencana dibaca lewat `StoreRows.fullValue(row: 0, column: 0, format: .raw)` (`cell_text`, `store_api.rs:659`). Parser rencana menerima `String`, jadi tidak bergantung pada kedua bentuk.
- `showingPlan` membuat footer berhenti menyebut baris dan menyembunyikan `LIMIT` (`ResultGrid.swift:620`, `:756`), dan `SortPolicy.route(… showingPlan:)` memaksa sort in-memory.
- Cap: `QueryTab.rowLimit` (`:479`, bawaan 1000), `AppModel.productRowLimitCeiling = 200_000` (`:2692`), `clampedRowLimit` (`:2698`), `previewEnvironment` mengirim `LIMIT` (`:3083-3088`). Fase 6 menaikkan plafon ke 5.000.000 bila P-1 lulus (`fase-6` §17.7). `Session.swift:31`, `:51`, `:78` menyimpan dan memulihkan `rowLimit` per tab.
- `done.truncated` datang dari engine (`commands.rs:1561`), dan engine membaca `limit + VERDICT_FETCH` baris untuk tahu apakah masih ada (`:1720-1723`). `applyPreviewDone` (`:2546`) menandai hasil yang dihentikan sebagai terpotong.
- Sort dan search server membungkus statement di tabel turunan lewat `ServerSort.order` (`ServerSort.swift:30-51`) dan `SearchStatement.crossColumn` (`GridSearch.swift:42`, `:101`), menjalankan ulang lewat `runPreview`, dan menolak bila ada edit yang di-staged (`serverActionBlockedByEdits`, `:2337`). `tab.previewedSQL` adalah statement yang benar-benar berjalan (terbungkus), `tab.previewBaseSQL` statement pengguna. `runPreview` mencatat history untuk setiap penyelesaian (`:2521-2534`), dan mengosongkan `columnFilters` dan (bila `clearSearch`) `gridSearch` (`:2452-2456`).
- Lembar tinjauan yang ada, `ChangeReview` (`ResultGrid.swift:897-976`), `private`, menampilkan `WriteStatement` dan punya tombol Run yang menerapkan rencana. `RunConfirmation.destructiveRequest(for:title:safeMode:)` (`RunConfirmation.swift:57`) membuat sheet konfirmasi untuk operasi destruktif dan bisa dipakai ulang untuk cancel.
- `PanelTab` (`QueryTab.swift:328`) berisi `result, log, files, history, saved`; `HistoryPanel` (`Panels.swift:378`) adalah pola panel global berfilter yang diikuti penampil audit. `ShortcutAction` (`Shortcuts.swift:6`) belum punya aksi paginasi.
- `Event` (`App.swift:156`) sudah memuat `columns`, `data`, `warnings`, `cancelled`, `queryId`; tidak perlu field baru untuk W13 (D-12).

### 1.7 Pengemasan app dan alat string

- `app/Package.swift`: `swift-tools-version:5.10`, satu `executableTarget` di `Sources/QueryHive`, **tanpa `resources:`** dan tanpa `defaultLocalization`; test target `exclude: ["__Baselines__"]`.
- `app/build.sh` membuat `Contents/{MacOS,Resources}` (`:49`), menulis `Info.plist` dari heredoc (`:77-95`, **tanpa** `CFBundleDevelopmentRegion` dan `CFBundleLocalizations`), menyalin `AppIcon.icns` ke `Contents/Resources` (`:117`), lalu menandatangani.
- `xcstringstool` ada di `/Applications/Xcode.app/Contents/Developer/usr/bin/` (subperintah `compile`, `extract`, `sync`, `print`, `generate-symbols`, `installloc`). `compile <input> --output-directory <dir>` menghasilkan `.strings` dan `.stringsdict` per bahasa (`--format stringsAndStringsdict` bawaan; hanya `--help` yang dibaca, kompilasi tidak dijalankan). Tidak ada di `/Library/Developer/CommandLineTools/usr/bin`, jadi mesin tanpa Xcode tidak punya alat ini.
- `pluralized(_:_:_:)` (`Theme.swift:618`) memakai aturan Inggris `n == 1 ? singular : singular + "s"`: tidak bisa dilokalkan, dan dipakai di banyak string model.
- `String(localized:)` dan literal `Text("…")`/`Button("…")`/`.help("…")` SwiftUI sudah mencari di `Bundle.main`. Hari ini nol: `grep -rnE "String\(localized|NSLocalizedString|LocalizedStringKey|\.xcstrings|\.lproj"` di `app/Sources`, `app/Package.swift`, dan `app/build.sh` tidak menemukan apa pun (cocok dengan audit desain §17).

### 1.8 Tumpang tindih dengan helper analitik

`fase-6` §14 (DataFusion di proses terpisah, diunduh, dikurung `sandbox-exec`, sewa anggaran, UniFFI minimal §14.13, tes §14.15) dan §21.6 sampai §21.8 (daftar berkas T8a, T8b, T8c) sudah lengkap. Blueprint ini **tidak mengulangnya** dan tidak mengubah satu keputusannya. Yang beririsan hanya empat titik, dan semuanya dikelola urutan (§10): lajur FFI bersama (`uniffi_api.rs`, `app/Generated/`, `Support/RustEngine.swift`) antara T4 dan T8b; `App.swift` antara T5 (bila memakai scene jendela) dan T8b; string panel Analitik T8c yang harus lewat katalog T7; dan Settings (`SettingsView.swift`) yang dimiliki T8c lalu T7.

## 2. Keputusan desain

| ID | Keputusan | Alasan |
|---|---|---|
| D-1 | **Paginasi menaikkan `tab.rowLimit` satu tingkat lalu menjalankan ulang statement yang sama.** Tingkat: 1.000, 10.000, 100.000, 1.000.000, plafon (`AppModel.rowLimitCeiling`). Tidak ada `OFFSET`, tidak ada kursor yang ditahan terbuka. | P-07 dan FR-GRID-02: `rowLimit` tetap satu-satunya cap, dan kursor yang diparkir menahan snapshot di produksi. |
| D-2 | **Aksi paginasi hanya untuk statement yang semuanya terbaca baca.** Dinilai dengan `StatementScan.classify` atas tiap statement di `previewBaseSQL`; `dml`, `ddl`, dan `unknown` mematikan aksinya dengan alasan tertulis. | Menjalankan ulang berarti mengulang efeknya (§3.2). Engine tidak bisa menolak ini: ia menilai izin, bukan idempotensi. |
| D-3 | **Cap yang dinaikkan paginasi bersifat sementara.** `QueryTab.paginationRaisedFrom` menyimpan nilai milik pengguna, `Session` menyimpan nilai itu, dan Run baru dari editor memulihkannya. | `rowLimit` disimpan per tab; "Ambil semua" di Trino tidak boleh menjadi default Run besok (§3.4). |
| D-4 | **"Ke baris N" memakai tingkat terkecil yang memuat N** dan menyebut biayanya sebelum jalan. Di luar plafon ditolak dengan kalimat yang menunjuk ke Export. | Kosakata cap satu-satunya adalah tingkat itu; PRD tidak meminta cap kustom. |
| D-5 | **"Tampilkan SQL" membongkar `ChangeReview` menjadi `SQLReviewSheet`** (judul, catatan, blok SQL berlabel, Salin, Tutup, tindakan utama opsional). `ChangeReview` menjadi pemakainya dan tetap punya Run; Tampilkan SQL tidak punya tombol Run. | FR-GRID-05: "memakai lembar tinjauan yang sudah ada". Sheet baca-saja tidak boleh membawa tombol tulis. |
| D-6 | **Dua setelan engine baru: `EXPLAIN_FORMAT` (`text` bawaan, atau `json`) dan `EXPLAIN_ANALYZE` (flag, bawaan mati).** Nilai yang tidak dikenal ditolak dengan menyebut nama setelan, seperti `SAFE_MODE`. Bawaan tidak mengubah satu byte keluaran, jadi korpus golden dan CLI tetap beku. | P-06, FR-PLAN-01, NFR-C. |
| D-7 | **Ejaan EXPLAIN tetap milik driver lewat metode trait baru berimplementasi bawaan** `Session::explain_statement_with(sql, ExplainOptions) -> Result<String, EngineError>` dan `Driver::explain_support() -> ExplainSupport`. `explain_statement` lama tidak diubah dan tetap yang dipanggil bila opsi bawaan. `host/lease.rs` meneruskan metode baru. | Menghindari ubah `Capabilities` (sepuluh literal) dan sembilan `Session` palsu; ejaan tetap di driver (komentar `qh-driver/src/lib.rs:502-506`). |
| D-8 | **Ejaan per driver** (§4.3): PostgreSQL `(ANALYZE, BUFFERS, FORMAT JSON)` dan variannya; Trino `(TYPE DISTRIBUTED, FORMAT JSON)` tanpa ANALYZE, dan `EXPLAIN ANALYZE <sql>` (teks) dengan ANALYZE; MySQL tidak berubah. Bila driver menurunkan format yang diminta, engine menaruh satu kalimat di `done.warnings`. | §1.1 (Trino tidak punya ANALYZE JSON), §1.3. |
| D-9 | **Pengenalan format di app memakai sniffing JSON, bukan field protokol baru.** Sel pertama yang diawali `[` atau `{` dan lolos `QueryPlan.parse` menjadi pohon; selain itu Raw. | `Event` (`App.swift`) tidak berubah, dan parser tetap benar bila server mengembalikan sesuatu yang tak terduga. |
| D-10 | **Guard ANALYZE = guard statement di dalamnya, ditambah syarat baca.** Guard yang ada (`commands.rs:1590`) menilai statement pemanggil sebelum awalan dipasang, dan penilaiannya tetap; hanya urutannya yang bergeser: pemeriksaan ANALYZE dijalankan **sebelum** `guard_for`, supaya ANALYZE yang ditolak meninggalkan tepat satu baris `refused` di log dan tidak pernah baris `allowed` untuk tulis yang tidak jalan (§4.4). Tambahannya: `EXPLAIN_ANALYZE=1` ditolak (usage, sebelum menyambung) bila ada statement yang bukan `ReadOnly` menurut `qh_sql`, di mode apa pun. `classify.rs` **tidak berubah perilakunya**; T2 hanya menambah tes yang mengunci ejaan baru. Teks yang diketik pengguna `EXPLAIN ANALYZE …` tetap `Ddl` seperti hari ini. | Lebih keras dari FR-PLAN-01 ("seperti statement di dalamnya"): ANALYZE mengeksekusi statement: di PostgreSQL 17.11 `EXPLAIN (ANALYZE) INSERT` pada tabel temp menulis satu baris dan `EXPLAIN INSERT` biasa tidak (§1.3); Trino menjalankan statement juga (terukur untuk SELECT, §1.1), efek tulisnya tidak dicoba. **Butuh persetujuan pemilik (§12).** |
| D-11 | **Pohon rencana diparse di Swift** (`Models/QueryPlan.swift`), murni, dari `String`. Node terpanas: waktu eksklusif bila ada ANALYZE (PostgreSQL), selain itu bagian biaya eksklusif (PostgreSQL) atau `cpuCost` numerik tertinggi (Trino). Tidak ditandai bila tidak ada yang melewati 20% dari jumlah nilai eksklusif. | §1.1 dan §1.3. Penandaan bukan hanya warna. |
| D-12 | **Perintah `sessions` dan `session_cancel` (nama dari PRD NFR-S1), dirutekan ke `Pooled(Metadata)`.** Hasil `sessions` berbentuk `columns` + `rows` + `done`, seperti `explain`. Tidak ada field `Event` baru. | P-05. Pool adalah reservasi, bukan antrean (`pool.rs:3-7`; PRD O-7), jadi poll di lajur query tidak akan menunggu. Alasan memilih lajur metadata adalah ongkos: ia memakai ulang satu sesi hangat dan tidak bersaing dengan dua reservasi Run, sedangkan di lajur query, selama kedua sesi query dipegang, setiap poll membuka dan menutup koneksi baru (§6.1). |
| D-13 | **Guard cancel sendiri:** keputusan yang sama dengan `StatementKind::Dml` (Full dan NoDdl boleh, Confirm bertanya dan menghormati `SAFE_MODE_CONFIRMED`, ReadOnly menolak), dicatat ke log dengan subjek `cancel session <id>` (bukan SQL korban). Hanya cancel; tidak ada terminate. | P-12: `KILL QUERY` tak terklasifikasi dan akan ditolak di `confirm` bila lewat teks. §1.5: mode terselesaikan sudah monoton untuk keputusan ini, jadi floor ADR-0027 tidak punya lubang di sini. |
| D-14 | **Tampilan aktivitas adalah jendela sendiri** (`WindowGroup(for: UUID.self)` per koneksi), bukan sheet, dengan sheet sebagai cadangan bila `App.swift` tidak boleh disentuh T5. Pembaruan tiap 3 detik selama jendela terlihat dan app aktif. Setiap cancel lewat sheet konfirmasi di mode apa pun kecuali ReadOnly, tempat tombolnya mati dengan alasan. | Diagnosis macet butuh jendela tetap terbuka di samping editor. Cancel menyentuh pekerjaan orang lain di produksi: tindakan keluar yang perlu ditanya. |
| D-15 | **Penampil audit memakai kontrak pembaca dari W11-A1** (§8.3). Dua prasyarat: sink terpasang di host app (milik W11-T1, §8.1) dan kolom koneksi (sub-tugas baru W13-T4b, §8.2, bergantung pada OQ-3). | Ringkasan butir 2 dan 3. |
| D-16 | **Lokalisasi tanpa resource SwiftPM:** `app/Resources/Localizable.xcstrings`, dikompilasi `build.sh` ke `Contents/Resources`, dibaca lewat `Bundle.main`. Kunci = teks Inggris. `Package.swift` tidak berubah. String literal SwiftUI tanpa interpolasi otomatis lokal; semua yang berisi nilai lewat `L10n`. | §1.7 dan butir 8 §10 `development-plan.md`. |
| D-17 | **W13-T8a sampai T8c tidak berubah.** Hanya urutan dan kepemilikan berkas yang dikelola di §10. | `fase-6` §14 sudah lengkap. |

## 3. Paginasi dan "Tampilkan SQL" (W13-T1)

### 3.1 Tingkat

```swift
// Models/AppModel+Run.swift (setelah W9-T0), di samping clampedRowLimit
enum RowLimitTiers {
    static let steps = [1_000, 10_000, 100_000, 1_000_000]
    /// The cap one step above `current`, never above `ceiling`; nil when `current` already is the ceiling.
    static func next(after current: Int, ceiling: Int) -> Int?
    /// The smallest step that holds `row` (1-based); nil when `row` is beyond `ceiling`.
    static func holding(row: Int, ceiling: Int) -> Int?
}
```

Daftar tingkat efektif adalah `steps.filter { $0 < ceiling } + [ceiling]`. Contoh: plafon 200.000 (sampai P-1) memberi 1k, 10k, 100k, 200k; plafon 5.000.000 memberi 1k, 10k, 100k, 1M, 5M. `next(after: 1_500)` adalah 10.000: cap kustom di footer naik ke tingkat berikutnya di atasnya.

### 3.2 Kapan aksinya tersedia

`AppModel.paginationBlockedReason(_ tab:) -> String?` (nil berarti tersedia), dipakai tombol dan menu untuk `disabled` dan `help`:

| Syarat | Alasan yang ditampilkan |
|---|---|
| `previewing`, `explaining`, atau `stage == .running` | (tombol mati, tanpa kalimat: sedang berjalan) |
| `showingPlan` atau `isObjects` | "A plan has no rows to fetch." / "Object lists are not capped." |
| `preview.stopped` | "Stopped early. Run again to fetch rows." |
| hasil berasal dari Run Script (`ScriptResult.origin == .script`, W12-A1 D-15; periksa nama terhadap kepala branch) | "Script results are not re-run one by one." |
| `!preview.truncated` | "This result is complete." |
| `!cellEdits.isEmpty` | `AppModel.stagedEditsMessage` ("Save or discard your changes first.") |
| ada statement di `previewBaseSQL ?? previewedSQL` yang bukan `readOnly` menurut `StatementScan.classify` | "Fetching more runs the statement again, and this one writes." |
| `rowLimit >= rowLimitCeiling` (hanya "Ambil lebih banyak" dan "Ambil semua") | "At the {ceiling} row limit. Export has no cap." |

Syarat baca adalah D-2. Alasannya konkret: `INSERT … RETURNING` dengan `LIMIT` terpotong menjalankan INSERT lagi pada "Ambil lebih banyak", dan `SELECT nextval(…)` menggeser sekuens. Nilai yang salah dibaca `readOnly` oleh cermin app (`StatementScan` adalah cermin sempit, `RunConfirmation.swift:73-79`) adalah risiko yang sama dengan menekan Run dua kali, bukan risiko izin: pertama kali statement itu sudah lolos guard engine.

### 3.3 Menjalankan ulang

`AppModel.fetchRows(_ tab:, upTo cap: Int, scrollTo row: Int? = nil)`, urutannya:

1. Periksa `paginationBlockedReason`; bila ada, catat di log tab dan berhenti.
2. Simpan **titik pulih** `PaginationUndo { preview, activeResult, baseResult, rowLimit, paginationRaisedFrom }`: pegangan ke store yang sedang tampil, bukan salinan baris.
3. `tab.paginationRaisedFrom = tab.paginationRaisedFrom ?? tab.rowLimit`; `tab.rowLimit = cap`; `tab.pendingScrollRow = row ?? tab.result.fetched` (indeks berbasis 0 baris pertama yang baru).
4. Jaga invarian "paling banyak dua store per tab" (`fase-6` §18, dan W9 D-13 menolak store ketiga): bila sort atau search server aktif, `baseResult` dilepas lebih dulu, sehingga "off" nanti menjalankan ulang statement dasar dengan cap sekarang lewat `rerunBaseSQL`; yang hidup selama run adalah store lama (titik pulih) dan store baru. Tanpa sort atau search, store lama adalah base sekaligus active, dan store baru yang kedua.
5. Panggil `runPreview` (`AppModel.swift:2431`; sesudah W9-T0 di `AppModel+Run.swift`) dengan statement terbungkus `tab.previewedSQL`, `baseSQL: tab.previewBaseSQL`, `activeSort: tab.activeSort`, `serverSearch: tab.serverSearch`, `clearSearch: false`, dan parameter baru `paginating: true`. Nama `activeResult`, `baseResult`, dan `ResultSlot` mengikuti W6-T1 (`fase-6` §17.4); periksa terhadap kepala branch.

`paginating: true` mengubah tiga hal yang selama ini dikerjakan `runPreview` tanpa syarat:

| Hal | Perilaku `paginating: true` | Alasan |
|---|---|---|
| `columnFilters`, `gridSearch`, `totalRows`, `countError` | dipertahankan. Setelah `done`, filter dipasang kembali ke store baru (`scheduleViewApply()` pada W6) | Statement sama, hanya jendelanya lebih lebar. "Count all" itu mahal dan jangan hilang karena paginasi. |
| `recordHistory` | dilewati | Pengguna tidak meminta Run baru, dan history menyimpan statement terbungkus (perilaku sort dan search hari ini) sebagai baris tambahan. |
| `tab.preview = nil` dan pelepasan store lama di awal | diganti titik pulih (langkah 2) | Run biasa menghapus hasil di awal, dan W9 D-13 menerima itu karena menahan store tambahan mahal. Untuk paginasi, galat atau timeout pada 1.000.000 baris tidak boleh menghapus 1.000 baris yang sudah ada di layar. |

Akhir run:

- **`done` tanpa stop:** lepas store lama; store baru menjadi `activeResult`, dan `baseResult` bila tidak ada sort atau search server. `pendingScrollRow` dipakai (§3.6).
- **Galat, atau proses keluar dengan status ≠ 0 sebelum `done`:** pulihkan titik pulih (`preview`, `activeResult`, `rowLimit`, `paginationRaisedFrom`), lepas store baru, dan tampilkan banner galat W9 di atas hasil lama: "Fetching more failed: {galat}. Showing the previous {n} rows."
- **Stop oleh pengguna:** bila store baru sudah memuat paling sedikit sebanyak yang lama, hasil baru (terhenti, terpotong) dipertahankan; bila tidak, titik pulih. Paginasi tidak pernah berakhir dengan baris lebih sedikit daripada sebelumnya.
- Selama run, grid menampilkan baris baru saat mengalir, dari atas. Hasil lama hanya ditahan di memori (anggaran dan spill menanggungnya) untuk dipulihkan, tidak ditahan di layar: menunggu store baru menyusul sebelum menukar menambah jalur asinkron untuk keuntungan yang kecil (dipertimbangkan dan ditolak).

Pengurutan baris tidak dijamin sama antar-run tanpa `ORDER BY`. Catatan bantuan tombol memuatnya: "The statement runs again; without ORDER BY the server may return rows in a different order."

### 3.4 Cap yang dinaikkan tidak bocor ke Run berikutnya (D-3)

- `QueryTab.rowLimit` mendapat `didSet`: perubahan di luar `fetchRows` (pengguna mengetik di bidang LIMIT) mengosongkan `paginationRaisedFrom`, karena itu keputusan pengguna.
- `QueryTab.persistedRowLimit` mengembalikan `paginationRaisedFrom ?? rowLimit`, dan `Session.swift:51` memakainya. Format sesi tidak berubah.
- Run dari editor (`AppModel.preview(_:from:)`, `AppModel.swift:1956`) memulihkan `rowLimit = paginationRaisedFrom` lebih dulu dan mencatat satu baris log "Row limit back to {n}". Run yang dipicu paginasi tidak melewati jalur itu.

### 3.5 "Ambil semua"

`cap = rowLimitCeiling`. Selalu lewat `.confirmationDialog` (pola `confirmReplace` di footer, `ResultGrid.swift:787`). Judul "Fetch up to {ceiling} rows?". Isi: "This runs the statement again and reads up to {ceiling} rows into memory." Untuk koneksi Trino ditambah: "On Trino the whole statement runs again on the cluster, which costs warehouse time on a large table." Tombol konfirmasi "Fetch Up to {ceiling} Rows". Teks tombol menyebut "up to", dan bila hasil masih terpotong di plafon, footer tetap berkata "limit reached" dengan bantuan "Export has no cap": "semua" tidak pernah berjanji lebih dari plafon. (Pelajaran dari TablePro: `LIMIT` yang dibangun dari estimasi diam-diam membuang sisa baris sementara bilah mengaku halaman selesai; footer di sini tidak pernah mengaku lengkap bila engine berkata terpotong.)

### 3.6 "Ke baris…"

Popover dari footer dengan bidang bilangan bulat berbasis 1. `AppModel.goToRow(_ tab:, row: Int) -> GoToRowOutcome`:

| Keadaan | Hasil |
|---|---|
| filter funnel atau search in-memory aktif | tombol mati: "Row numbers refer to the rows as listed. Clear the filter first." |
| `row <= tab.result.count` | `tab.pendingScrollRow = row - 1`, seleksi sel pertama baris itu |
| `row > fetched`, hasil lengkap | pesan di popover: "This result has {fetched} rows." |
| `row > fetched`, terpotong, `holding(row:)` ada | popover berkata "Row {row} needs the first {tier} rows. The statement runs again." dengan tombol Fetch; `fetchRows(upTo: tier, scrollTo: row - 1)` |
| `row > rowLimitCeiling` | "Row {row} is beyond the {ceiling} row limit. Use Export to reach it." |

Pengguliran memakai `tab.pendingScrollRow: Int?`, diamati `Coordinator` di `updateNSView` dan dipakai sekali (`scrollToVisible(row:)`, `ResultGridTable.swift:865`, lalu `pendingScrollRow = nil`). Jalur yang sama dipakai "Ambil lebih banyak" dan "Ke baris", sehingga tidak ada penggulir kedua. Pada W6, penggulirnya menunggu `done` (baris baru belum ada sebelum itu); `Coordinator` mengabaikan nilai yang di luar `rows.count` dan menyimpannya sampai hitungan cukup atau `done` tiba.

### 3.7 "Tampilkan SQL" (D-5)

`SQLReviewSheet` (di `ResultGrid.swift`) menerima `title`, `note`, `blocks: [(label: String, sql: String)]`, dan `primary: (title, role, action)?`. `ChangeReview` dibangun ulang di atasnya (Run tetap, dengan `role: .destructive`), dan perilakunya tidak berubah.

Isi untuk Tampilkan SQL:

- Blok "Sent to the server": `tab.previewedSQL`.
- Bila `previewBaseSQL != previewedSQL`: blok "Your statement": `previewBaseSQL`, dan catatan satu baris yang menyebut pembungkusnya dari `tab.activeSort` ("Sorted on the server by {column}, ascending/descending") atau `tab.serverSearch` ("Searched on the server for “{term}” across every column").
- Catatan tetap: "The row limit ({rowLimit}) is not part of the SQL: the engine stops reading at that many rows."
- Salin SQL menyalin blok pertama dengan `;`.

Titik masuk: item menu footer "Show SQL" (selama `previewedSQL != nil` dan bukan hasil objek), dan tautan pada banner sort W4-T1 bila banner itu menampilkan sort server. Penataan akhir footer (satu `Menu` untuk Ambil semua, Ke baris, Tampilkan SQL, ditambah pil "Fetch more" yang hanya tampil bila tersedia) diserahkan ke ui-ux-designer; blueprint hanya mengunci kontraknya.

### 3.8 Perubahan per berkas (W13-T1)

| Berkas | Isi | Prioritas |
|---|---|---|
| `app/Sources/QueryHive/Models/AppModel+Run.swift` | `RowLimitTiers`, `paginationBlockedReason`, `fetchRows`, `goToRow`, parameter `paginating` pada `runPreview`, pemulihan cap di `preview` | P0 |
| `app/Sources/QueryHive/Models/QueryTab.swift` | `paginationRaisedFrom`, `pendingScrollRow`, `didSet` pada `rowLimit`, `persistedRowLimit` | P0 |
| `app/Sources/QueryHive/Models/Session.swift` | satu baris: `rowLimit = tab.persistedRowLimit` (`:51`). **Tidak ada di daftar berkas T1** | P0 |
| `app/Sources/QueryHive/Views/ResultGrid.swift` | kontrol footer, `GoToRowPopover`, dialog Ambil semua, `SQLReviewSheet` dan `ChangeReview` di atasnya | P0 |
| `app/Sources/QueryHive/Views/ResultGridTable.swift` | `Coordinator` mengonsumsi `pendingScrollRow`. **Tidak ada di daftar berkas T1** | P0 |
| `app/Sources/QueryHive/Support/Snapshot.swift`, `app/Tests/QueryHiveTests/VisualParityTests.swift` | scene V-11: footer terpotong, footer di plafon, popover Ke baris, lembar Tampilkan SQL. **Tidak ada di daftar berkas T1** | P1 |
| `app/Tests/QueryHiveTests/PaginationTests.swift` (baru), `ShowSQLTests.swift` (baru) | §3.9 | P0 |

Tanpa perubahan engine, FFI, atau `Generated/`. Pintasan keyboard (`Models/Shortcuts.swift`, `App.swift`) tidak disentuh T1; bila UX meminta pintasan Ke baris, itu masuk peta pintasan W9-A1 atau ditambahkan T1 setelah kepemilikan `Shortcuts.swift` dan `App.swift` diberikan (§12).

### 3.9 Tes (W13-T1)

`PaginationTests` (tes lebih dulu, tanpa engine; memakai `TestIsolation`):

- Tingkat: `next(after:ceiling:)` untuk 1.000, 1.500, 100.000, 1.000.000, plafon, dan plafon 200.000; `holding(row:ceiling:)` untuk 1, 1.000, 1.001, 15.000, `ceiling`, `ceiling + 1`.
- `paginationBlockedReason` untuk setiap baris tabel §3.2, termasuk `INSERT … RETURNING` dan `WITH x AS (DELETE …) SELECT` (tak tersedia), `SELECT 1` (tersedia), dan statement terbungkus sort server yang membungkus tulis.
- `fetchRows` dengan `MockEngine`: lingkungan yang dikirim memuat `LIMIT` tingkat berikutnya; `activeSort` dan `serverSearch` dipertahankan; tidak ada pemanggilan `history_add`; `columnFilters` dan `totalRows` dipertahankan; `baseResult` dilepas bila sort server aktif dan diganti bila tidak.
- Titik pulih: galat sebelum `done` memulihkan `preview`, `activeResult`, dan `rowLimit` dan menampilkan banner; Stop dengan store baru lebih kecil memulihkan, dengan store baru lebih besar mempertahankan; sort server aktif melepas `baseResult` sebelum store baru dibuat (jumlah store ≤ 2, lewat `store_stats()`); `done` melepas store lama.
- Pemulihan cap: setelah `fetchRows`, `Session` menyimpan nilai pengguna; `preview(_:)` dari editor memulihkannya; mengetik di bidang LIMIT mengosongkan `paginationRaisedFrom`.
- `goToRow` untuk lima keadaan §3.6; `pendingScrollRow` dikonsumsi tepat sekali.
- "Ambil semua": teks dialog memuat kalimat gudang hanya untuk `.trino`.

`ShowSQLTests`: blok dan catatan untuk run biasa, sort server (naik dan turun), dan search server; `ChangeReview` masih menghasilkan skrip yang sama (tes `WritePlanTests` yang ada tidak boleh berubah).

Gate: G-SWIFT, G-VIS (scene baru saja, baseline direkam di langkah merge orkestrator, §0.5).

## 4. EXPLAIN JSON dan ANALYZE di engine (W13-T2)

### 4.1 Setelan

`commands.rs` mendapat `explain_options(settings) -> Result<ExplainOptions, CliError>`:

- `EXPLAIN_FORMAT`: kosong atau `text` (bawaan), atau `json`. Nilai lain: `CliError::Usage("unknown EXPLAIN_FORMAT 'x'; expected text, json")`. Spasi dan huruf besar ditoleransi seperti `SafeMode::parse`.
- `EXPLAIN_ANALYZE`: `settings.flag("EXPLAIN_ANALYZE", false)`.

Kedua kunci hanya dibaca `explain`. `preview`, `count`, dan perintah lain tidak melihatnya, dan MCP tidak bisa menyetelnya (§1.4). Dengan kedua kunci absen, jalur yang berjalan adalah `session.explain_statement(&sql)` hari ini, byte demi byte (golden `explain` dan kasus `*_explain_live` tidak berubah).

### 4.2 Seam driver (D-7)

```rust
// crates/qh-driver/src/lib.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExplainFormat { #[default] Text, Json }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExplainOptions { pub format: ExplainFormat, pub analyze: bool }

/// What a driver can spell, readable before connecting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExplainSupport { pub json: bool, pub analyze: bool, pub analyze_json: bool }

impl ExplainSupport {
    /// The options the driver will actually run, and the sentence to report when they differ.
    /// `Err` when ANALYZE is asked and the driver has none: refused before anything is opened.
    pub fn resolve(&self, asked: ExplainOptions)
        -> Result<(ExplainOptions, Option<&'static str>), EngineError>;
}

trait Driver  { fn explain_support(&self) -> ExplainSupport { ExplainSupport::default() } /* … */ }
trait Session {
    /// The statement for `sql` under options `resolve` already accepted. The default refuses
    /// anything but the default options instead of quietly spelling a plain EXPLAIN.
    fn explain_statement_with(&self, sql: &str, options: ExplainOptions)
        -> Result<String, EngineError> { /* default → explain_statement(sql), else Usage */ }
}
```

Bentuk ini memenuhi tiga syarat: ejaan tetap milik driver; sembilan `Session` palsu dan sepuluh literal `Capabilities` tidak berubah; dan **implementasi bawaan yang lupa diteruskan gagal keras**, bukan diam-diam menurunkan ANALYZE menjadi EXPLAIN biasa. `host/lease.rs:494` harus meneruskan `explain_statement_with` ke sesi dalam, dan tes mengunci bahwa sesi yang dipinjam dari pool menjawab sama dengan sesi aslinya (tes `lease.rs`).

### 4.3 Ejaan per driver (D-8)

| Driver | Opsi bawaan | `json` | `analyze` | `analyze` + `json` |
|---|---|---|---|---|
| PostgreSQL (`explain_support`: json, analyze, analyze_json) | `EXPLAIN <sql>` (hari ini) | `EXPLAIN (FORMAT JSON) <sql>` | `EXPLAIN (ANALYZE, BUFFERS) <sql>` | `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) <sql>` |
| Trino (json, analyze; **bukan** analyze_json) | `EXPLAIN <sql>` | `EXPLAIN (TYPE DISTRIBUTED, FORMAT JSON) <sql>` | `EXPLAIN ANALYZE <sql>` | diturunkan ke baris `analyze` (teks), dengan peringatan |
| MySQL (tidak ada) | `EXPLAIN <sql>` | diturunkan ke teks, dengan peringatan | `Usage`: "EXPLAIN ANALYZE is not available on MySQL" (sebelum menyambung) | `Usage`, sama |

Catatan:

- `<sql>` selalu keluaran `strip_terminator_dialect(sql, dialect)` yang sama dengan hari ini, karena PostgreSQL dan Trino menolak `;` penutup (`crates/qh-driver-postgres/src/lib.rs:664-669`, `crates/qh-driver-trino/src/lib.rs:1385-1397`) dan `;` di dalam literal harus selamat.
- PostgreSQL: `BUFFERS` ikut ANALYZE karena murah dan memberi blok dibaca per node; `VERBOSE`, `SETTINGS`, `WAL`, dan `TIMING OFF` tidak dipakai. `BUFFERS` diterima sejak 9.0, `FORMAT JSON` sejak 9.0, jadi tidak ada syarat versi baru.
- Trino: `TYPE DISTRIBUTED` ditulis eksplisit walau bawaan server sama (terukur §1.1), supaya perubahan bawaan di rilis Trino lain tidak mengubah bentuk yang diparse app. Tanpa ANALYZE JSON-nya objek berkunci fragmen; dengan ANALYZE teks.
- Peringatan penurunan, di `done.warnings`: Trino: "Trino has no JSON form of EXPLAIN ANALYZE; the plan is text."; MySQL: "MySQL plans are returned as text." Satu kalimat per penurunan.

### 4.4 Urutan di `explain`

Semua pemeriksaan yang murni (tanpa I/O) dijalankan **sebelum** `guard_for`. Alasannya integritas log: `guard_for` -> `guard_confirmed` -> `record_decision` (`commands.rs:227-290`, `:297-299`) selalu menulis satu baris, dan `LogDecision::Allowed` didokumentasikan sebagai "Ran without asking" (`execution_log.rs:42-50`). Bila `guard_for` jalan lebih dulu, sebuah `INSERT` yang dikirim dengan `EXPLAIN_ANALYZE=1` di koneksi `full` tercatat `allowed` (atau `confirmed` di `confirm` dengan `SAFE_MODE_CONFIRMED=1`), lalu ditolak oleh syarat baca dan tercatat `refused` sekali lagi: log yang tamper-evident akan mengklaim sebuah tulis berjalan padahal tidak pernah.

1. `source_sql` dan `safe_mode(settings, engine)?` (`commands.rs:1585-1589`): tidak berubah. Mode efektif dibutuhkan di sini karena baris `refused` di langkah 3 memuatnya.
2. `explain_options(settings)?` (usage bila nilai tak dikenal). Murni: tanpa I/O, tanpa baris log.
3. **`analyze_requires_read`** (D-10, hanya bila `analyze`): lihat §4.5. Penilaiannya murni; satu-satunya efeknya, bila ada statement yang bukan baca, adalah tepat satu baris log `refused` (statement pertama yang gagal), lalu `Usage`. `guard_for` tidak pernah dijalankan untuk run itu.
4. `engine.driver(config.kind).explain_support().resolve(options)?` (hanya bila opsi bukan bawaan; `config` dari `connection(settings, engine)?`, yang murni: `config::build` dan port bawaan driver, `commands.rs:81-90`): menolak ANALYZE di MySQL dan menurunkan format. Tanpa I/O, tanpa baris log. Jalur bawaan melewati langkah ini, jadi urutan galat konfigurasi terhadap guard tidak berubah dari hari ini.
5. `guard_for(settings, mode, &sql)`: **dipindah ke sini dari posisi pertama**, dan satu-satunya yang menulis baris log untuk run yang diteruskan. Statement pemanggil dinilai tanpa awalan, jadi untuk ANALYZE yang dinilai adalah statement yang akan dieksekusi.
6. `statement_timeout`, `connection` (sudah dihitung di langkah 4 bila opsi bukan bawaan), `open`, lalu `session.explain_statement_with(&sql, resolved)?`, lalu `execute_until_stopped` dengan `row_limit: None`, persis seperti sekarang. `STATEMENT_TIMEOUT_MS` berlaku untuk ANALYZE (invariant #7), dan Stop mencapai server lewat `stop_session` seperti Explain hari ini.
7. `done` membawa `warnings` sebagai daftar: peringatan penurunan, dan peringatan stop bila ada (hari ini satu string, `commands.rs:1650`).

Akibat pada log, yang dikunci tes §4.6:

| Permintaan | Baris log |
|---|---|
| ANALYZE dari statement tulis (mode apa pun, termasuk `confirm` dengan `SAFE_MODE_CONFIRMED=1`) | tepat satu, `refused`, dari langkah 3 |
| `EXPLAIN_FORMAT` tak dikenal | nol |
| ANALYZE di MySQL | nol |
| ANALYZE dari statement baca | satu, `allowed`, dari `guard_for` di langkah 5 (statement baca selalu `Allow`, tidak pernah meminta konfirmasi) |

ANALYZE tidak dibatasi `LIMIT`: server mengeksekusi seluruh statement dan hanya baris rencananya yang kembali. Itu sebabnya app meminta konfirmasi sebelum ANALYZE (§5.3) dan statement timeout tetap menjadi pagarnya.

### 4.5 `analyze_requires_read` (D-10)

```rust
/// EXPLAIN ANALYZE runs the statement it explains, and neither PostgreSQL nor Trino rolls an
/// ANALYZE of a write back. Offered for reads only, in every Safe Mode: the mode decides whether
/// a statement may run, this decides whether it is a statement ANALYZE may run at all.
fn analyze_requires_read(settings: &Settings, mode: SafeMode, sql: &str) -> Result<(), CliError>
```

- Menilai `qh_sql::decisions_readings(SafeMode::ReadOnly, sql, dialect(settings).readings())`: setiap statement harus `Decision::Allow` (jadi `ReadOnly`; `Unknown` gagal tertutup, termasuk dua bacaan leksikal MySQL yang berselisih).
- Bila ada yang tidak lolos (statement pertama yang gagal; fungsi berhenti di situ): `CliError::Usage("EXPLAIN ANALYZE runs the statement, so it is offered for reads only: <alasan classifier>")`, dan keputusan dicatat, sebagai satu-satunya baris untuk run itu, lewat `record_decision` dengan **mode efektif koneksi**, jenis dari classifier, `LogDecision::Refused`, dan alasan tetap itu. `record_decision` (`commands.rs:255`, hari ini privat) menjadi `pub(crate)`. Memanggil `guard_confirmed(SafeMode::ReadOnly, …)` langsung salah: kolom `safe_mode` log akan berkata `read_only` untuk koneksi `full`.
- Statement baca lolos tanpa menulis baris apa pun di fungsi ini (ia murni sampai menemukan penolakan): satu-satunya baris untuk run itu ditulis `guard_for` di langkah 5 (§4.4).

`classify.rs` **tidak berubah perilakunya.** T2 menambah tes di `mod tests` yang mengunci hal yang sekarang hanya terbukti lewat bacaan kode: `EXPLAIN (FORMAT JSON) SELECT 1` dan `EXPLAIN (TYPE DISTRIBUTED, FORMAT JSON) SELECT 1` adalah `ReadOnly`; `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) SELECT 1`, `EXPLAIN ANALYZE SELECT 1`, dan `EXPLAIN (ANALYZE) UPDATE t SET a = 1` tidak pernah lebih longgar daripada statement di dalamnya. Teks yang diketik pengguna `EXPLAIN ANALYZE SELECT …` tetap `Ddl` (`classify.rs:1214`, `:1847`) dan ditolak di `no_ddl` dan di atasnya: jalur Explain di toolbar adalah cara memakai ANALYZE di koneksi terkunci. Memperlonggar teks yang diketik berada di area risiko tinggi (classifier) dan tidak diminta satu pun use case, jadi tidak dilakukan.

### 4.6 Tes dan gate (W13-T2)

Tes lebih dulu (`crates/qh-ffi/tests/safe_mode.rs`, memakai `CountingEngine` dan `RecordingSession`, `:26` dan `:586`). Driver palsu di berkas itu meng-override `explain_support` (json dan analyze menyala) dan sesinya meng-override `explain_statement_with` untuk merekam ejaan; tanpa itu bawaan trait menolak opsi bukan-bawaan, sesuai rancangan (§4.2):

| Tes | Isi |
|---|---|
| `explain_analyze_of_a_read_is_allowed_in_every_mode` | `SELECT 1` dengan `EXPLAIN_ANALYZE=1` lolos di `full`, `no_ddl`, `confirm`, `read_only`, dan sesi merekam ejaan ANALYZE |
| `explain_analyze_of_a_write_is_refused_before_connecting` | `INSERT`, `UPDATE`, `DELETE`, `WITH x AS (DELETE …) SELECT`, `SELECT … FOR UPDATE`, `SELECT … INTO`, `EXECUTE p(1)`, dan `DO` di `full` dan `confirm` dengan `SAFE_MODE_CONFIRMED=1`: `Usage`, `connects == 0`, dan **tepat satu** baris log, `refused`, dengan mode efektif (tidak ada baris `allowed` atau `confirmed` sebelumnya) |
| `explain_analyze_keeps_the_mysql_and_postgres_lexical_readings` | `SELECT '\'' ; DELETE …` pada MySQL dan `E'\''` pada PostgreSQL tetap ditolak sebelum menyambung, dengan `EXPLAIN_ANALYZE=1` (NFR-S1: escape backslash dan komentar `#` untuk MySQL) |
| `explain_analyze_is_not_available_on_mysql` | `Usage` sebelum menyambung, tanpa baris log |
| `explain_format_json_downgrades_with_a_warning_on_mysql_and_on_a_plain_session` | `done.warnings` memuat kalimat penurunan |
| `an_unknown_explain_format_is_refused_by_name` | sebut `EXPLAIN_FORMAT`, tanpa baris log |
| `floors_stay_monotone_for_explain_analyze` | `SAFE_MODE=full` dengan `SAFE_MODE_FLOOR=read_only` menolak ANALYZE dari tulis dengan alasan yang sama (ADR-0027) |

Tes unit driver (string persis, termasuk terminator dan `;` di dalam literal): PostgreSQL (3 varian di §4.3), Trino (2 varian dan penurunan), MySQL (varian bawaan dan galat). `ExplainSupport::resolve` diuji per baris tabel. `lease.rs`: sesi pinjaman meneruskan opsi.

Gate: G-RUST; G-GOLDEN (tidak ada selisih baru: kunci absen); **G-LIVE** `QH_TEST_POSTGRES=1 cargo test -p qh-ffi --test real_server` dan `QH_TEST_TRINO=1 cargo test -p qh-driver-trino`, dengan kasus baru: PostgreSQL JSON dan ANALYZE+JSON (sel pertama lolos `serde_json`, kunci `Execution Time` ada untuk ANALYZE); Trino `(TYPE DISTRIBUTED, FORMAT JSON)` (objek dengan kunci `"0"`), `EXPLAIN ANALYZE` (teks memuat `Fragment 1`), dan ANALYZE+JSON (peringatan penurunan di `done`, sel bukan JSON). Pemeriksa: RR, DB, SEC. Commit: `feat(engine): JSON plans and EXPLAIN ANALYZE, guarded like the statement they run`.

### 4.7 Perubahan per berkas (W13-T2)

| Berkas | Isi | Prioritas |
|---|---|---|
| `crates/qh-driver/src/lib.rs` | `ExplainFormat`, `ExplainOptions`, `ExplainSupport` (+ `resolve`), `Driver::explain_support`, `Session::explain_statement_with` (bawaan). **Tidak ada di daftar berkas T2** | P0 |
| `crates/qh-driver-postgres/src/lib.rs`, `crates/qh-driver-trino/src/lib.rs`, `crates/qh-driver-mysql/src/lib.rs` | `explain_support`, `explain_statement_with`, tes string persis | P0 |
| `crates/qh-ffi/src/host/lease.rs` | meneruskan `explain_statement_with`. **Tidak ada di daftar berkas T2** | P0 |
| `crates/qh-ffi/src/commands.rs` | `explain_options`, `analyze_requires_read`, urutan §4.4, `warnings` daftar, `record_decision` `pub(crate)` | P0 |
| `crates/qh-sql/src/classify.rs` | **hanya tes** (§4.5) | P1 |
| `crates/qh-ffi/tests/safe_mode.rs` | §4.6 | P0 |
| `crates/qh-ffi/tests/real_server.rs`, `crates/qh-driver-trino/tests/integration.rs` | kasus live. **Tidak ada di daftar berkas T2** | P1 |

Tanpa perubahan `lib.rs` (`COMMANDS`), `uniffi_api.rs`, atau `app/Generated/`: tidak ada perintah baru, dan setelan baru lewat `Vec<Setting>` yang sudah ada.

## 5. Pohon rencana (W13-T3)

### 5.1 Model dan parser (`Models/QueryPlan.swift`, murni, tanpa FFI)

```swift
struct PlanNode: Identifiable, Equatable {
    let id: Int                      // urutan DFS, stabil untuk satu rencana
    var title: String                // "Hash Join", "Aggregate", "ScanFilterProject"
    var subtitle: String?            // relasi/indeks (PostgreSQL), tabel atau kriteria (Trino)
    var facts: [(String, String)]    // Filter, Index Cond, Join Type, descriptor Trino, details
    var estimatedRows: Double?
    var actualRows: Double?          // total (Actual Rows × Actual Loops)
    var loops: Int?
    var selfMillis: Double?          // eksklusif, dari total node dikurangi total anak
    var share: Double?               // 0...1 terhadap jumlah nilai eksklusif semua node, untuk batang dan penandaan
    var children: [PlanNode]
}
struct QueryPlan: Equatable {
    enum Dialect { case postgres, trino }
    var dialect: Dialect
    var roots: [PlanNode]            // PostgreSQL: satu. Trino: fragmen 0 dengan RemoteSource diperluas
    var planningMillis: Double?, executionMillis: Double?
    var hottest: PlanNode.ID?        // nil bila tidak ada yang melewati 20%
    var hotMetric: HotMetric         // .selfTime, .estimatedCost, .estimatedCPU
    var nodeCount: Int
    static func parse(_ text: String) -> QueryPlan?     // nil → Raw
}
```

Aturan yang dikunci tes, dari §1.1 dan §1.3:

- **PostgreSQL:** akar adalah array dengan satu objek bertanda `"Plan"`. Anak di `"Plans"`. Waktu node = `Actual Total Time × Actual Loops` (nilai itu rata-rata per loop); waktu eksklusif = waktu node dikurangi jumlah waktu anaknya, di-clamp ke ≥ 0, **kecuali anak `InitPlan`**: pada dua rencana di PostgreSQL 17.11 waktu `InitPlan` berada di luar waktu induknya (induk 0,167 ms, anak `InitPlan` 0,114 dan 0,022 ms), sedangkan waktu `SubPlan` berada di dalamnya (`Seq Scan` 5,901 ms memuat `SubPlan` 2,959 ms). Di bawah node paralel (`Gather`, `Gather Merge`, atau `Parallel Aware`) durasi pekerja berjalan bersamaan dan **tidak dijumlahkan**. Tanpa ANALYZE: biaya eksklusif = `Total Cost` node dikurangi jumlah `Total Cost` anaknya, di-clamp ke ≥ 0. Node `Limit` memiliki `Total Cost` **lebih kecil** daripada anaknya (9,85 melawan 4.985,72 pada rencana yang diukur), jadi biaya dibagi `Total Cost` akar akan melewati 1. **Bagian (`share`) selalu eksklusif node dibagi jumlah eksklusif semua node** untuk metrik yang dipakai, sehingga berada di 0...1 dan berjumlah 1.
- **Trino:** akar adalah objek berkunci id fragmen. Fragmen `"0"` jadi akar; setiap node dengan `descriptor.sourceFragmentIds` (string seperti `"[1, 2]"`, diparse sebagai bilangan bulat) mendapat akar fragmen itu sebagai anak, dengan penjaga siklus (fragmen yang sudah dikunjungi tidak diperluas lagi). `estimates` bisa `[]`, dan angka biaya bisa string `"NaN"`: nilai bukan-hingga dianggap tidak ada. Node terpanas dipilih dari `cpuCost` numerik tertinggi (biaya per node, §1.1; kesimpulan dari dua contoh, jadi label UI berbunyi "Highest estimated CPU", bukan "slowest"), dengan `outputRowCount` sebagai pemecah seri. Tanpa satu pun biaya numerik: tidak ada penandaan.
- Parser tidak pernah mempercayai server: kedalaman dibatasi 256 dan jumlah node 20.000 (lebih dari itu `parse` mengembalikan `nil` dan Raw tampil dengan catatan), angka yang bukan angka dibuang, kunci yang hilang berarti nilai `nil`, dan tidak ada `try!` atau `fatalError`. Dikerjakan di luar main (`Task.detached`), hasilnya ditulis ke `tab.plan` di main.
- Penandaan terpanas bukan hanya warna: simbol dan teks "Hottest" di baris, ditambah persentase, dan label aksesibilitas yang menyebutnya. Pita severity bertingkat empat dari TablePro dipertimbangkan dan tidak dipakai: satu penanda yang jelas lebih mudah dibaca daripada empat pita, dan perbandingan dengan plan keluar (P-19).

### 5.2 State dan alur (`AppModel.explain`)

- `QueryTab` mendapat `plan: QueryPlan?`, `planMode: PlanMode` (`.tree` bawaan, `.raw`), `planAnalyzed: Bool`, dan `planNote: String?` (mis. peringatan penurunan). `releaseResults()` dan awal Run/Explain baru mengosongkannya.
- `explain(_ tab:, from:, analyze: Bool = false, confirmed:)`: untuk koneksi PostgreSQL dan Trino mengirim `EXPLAIN_FORMAT=json`; untuk MySQL tidak mengirim apa-apa. `analyze` mengirim `EXPLAIN_ANALYZE=1`. Konfirmasi `confirm`-level yang ada (`RunConfirmation.request(for:command: "explain")`, `AppModel.swift:2941`) tetap.
- Pada `done`: `QueryPlan.parse(tab.result.fullValue(row: 0, column: 0, format: .raw) ?? "")` di luar main. Berhasil berarti `plan` terisi dan `planMode = .tree`; gagal berarti `plan = nil` dan Raw (grid yang sama, PR-14).
- **Cadangan hanya untuk EXPLAIN tanpa ANALYZE:** bila server menolak `EXPLAIN_FORMAT=json` (galat sintaks, server lama), app menjalankan ulang sekali tanpa setelan itu dan mencatat satu baris. ANALYZE **tidak pernah** dijalankan ulang otomatis: menjalankannya dua kali berarti mengeksekusi dua kali.
- Toolbar dan menu Explain (`Workspace.swift`, `App.swift`) tidak berubah. ANALYZE ditawarkan dari footer rencana (§5.3), sehingga T3 tidak membutuhkan berkas yang dimiliki rantai lain.

### 5.3 Tampilan (`Views/PlanTreeView.swift`, `ResultGrid.swift`)

- `content` di `ResultGrid.swift:93` mendapat cabang sebelum `grid(preview)`: `tab.showingPlan && tab.planMode == .tree && tab.plan != nil` menampilkan `PlanTreeView`. Raw adalah `grid(preview)` yang ada.
- `PlanTreeView`: `List` dengan `OutlineGroup(plan.roots, children: \.optionalChildren)` (semantik outline dan keyboard bawaan), tiap baris `HStack`: judul dan subjudul, kolom angka (estimasi baris, baris aktual × loop, ms eksklusif atau biaya), batang bagian dari total, penanda terpanas. Detail (`facts`) muncul sebagai baris kedua yang dapat dibuka. Seluruh nilai data memakai `Text(verbatim:)`. Label aksesibilitas per baris menggabungkan judul, relasi, baris, waktu, dan "hottest node". Menyalin baris atau seluruh rencana menyalin JSON/teks mentah.
- Footer saat `showingPlan`: ringkasan "Query plan · {n} nodes · Planning {p} ms · Execution {e} ms" (dua yang terakhir hanya bila ada), kontrol segmen Tree/Raw (mati dengan bantuan bila `plan == nil`, dengan `planNote` atau alasan), dan tombol "Analyze" yang menampilkan `.confirmationDialog`: "EXPLAIN ANALYZE runs the statement and reads everything it produces. {Trino: On Trino this runs on the cluster and costs warehouse time.}" Tombol Analyze mati (dengan alasan) di MySQL dan untuk statement yang bukan baca (`StatementScan` atas statement sumber, penilai baca yang sama dengan §3.2).
- Di Trino setelah ANALYZE, catatan "Trino returns EXPLAIN ANALYZE as text; the tree is not available." menggantikan kontrol Tree.
- Penataan visual akhir, kepadatan, dan scene V-11 (`plan-tree-postgres`, `plan-tree-trino`, `plan-raw-trino-analyze`) diserahkan ke ui-ux-designer dan a11y-architect; blueprint mengunci kontrak data dan aksesibilitasnya.

### 5.4 Fixture dan tes (W13-T3)

Fixture **diambil dari server dev, bukan ditulis tangan** (angka palsu menyembunyikan kasus seperti `Actual Loops` dan `"NaN"`), setelah T2 mendarat: `queryhive-engine explain` dengan `EXPLAIN_FORMAT=json` (dan `EXPLAIN_ANALYZE=1` untuk PostgreSQL), dimasukkan sebagai string mentah di `app/Tests/QueryHiveTests/PlanFixtures.swift` (tanpa resource tes, jadi `Package.swift` tidak berubah). Himpunan minimal: PostgreSQL join tiga tabel tanpa ANALYZE; PostgreSQL ANALYZE dengan `Nested Loop` dan node `Memoize` ber-loop > 1; PostgreSQL paralel (`Gather`, `Workers Launched`); PostgreSQL dengan `InitPlan` dan `SubPlan` serta `LIMIT` (dua rencana yang terukur di §5.1 adalah kandidatnya); Trino terdistribusi dengan tiga fragmen dan `RemoteSource`; Trino dengan `"NaN"` dan `estimates: []`; Trino `TYPE LOGICAL`; dan tiga masukan rusak (JSON terpotong, objek asing, kedalaman 300).

`QueryPlanTests`: setiap aturan §5.1 (total = rata-rata × loop; eksklusif ≥ 0; pekerja paralel tidak dijumlahkan; `InitPlan` tidak mengurangi induk tetapi `SubPlan` mengurangi; biaya di bawah `Limit` tetap menghasilkan `share` ≤ 1; tautan fragmen dan penjaga siklus; `"NaN"` dibuang; tidak ada penandaan bila tak ada yang ≥ 20%; terpanas yang dipilih); parse 5.000 node < 50 ms; masukan rusak mengembalikan `nil` tanpa crash. `ExplainFlowTests` dengan `MockEngine`: lingkungan memuat `EXPLAIN_FORMAT=json` hanya untuk PostgreSQL dan Trino; `EXPLAIN_ANALYZE` hanya dari tombol Analyze; cadangan sekali tanpa ANALYZE; tidak pernah dua kali dengan ANALYZE; `planMode` jatuh ke Raw bila `parse` mengembalikan `nil`.

Gate: G-SWIFT, G-VIS (scene baru). Pemeriksa: SR, UX, AX. Commit: `feat(app): plans read as a tree with the hottest node marked`.

### 5.5 Perubahan per berkas (W13-T3)

| Berkas | Isi | Prioritas |
|---|---|---|
| `app/Sources/QueryHive/Models/QueryPlan.swift` (baru) | model dan parser §5.1 | P0 |
| `app/Sources/QueryHive/Views/PlanTreeView.swift` (baru) | §5.3 | P0 |
| `app/Sources/QueryHive/Views/ResultGrid.swift` | cabang `content`, footer plan, dialog Analyze | P0 |
| `app/Sources/QueryHive/Models/QueryTab.swift` dan `Models/AppModel+Run.swift` | state plan dan `explain(analyze:)`. **Tidak ada di daftar berkas T3**; keduanya sudah dimiliki T1 dan T3 berjalan sesudahnya | P0 |
| `app/Tests/QueryHiveTests/{QueryPlanTests,ExplainFlowTests,PlanFixtures}.swift` (baru) | §5.4 | P0 |
| `Support/Snapshot.swift`, `VisualParityTests.swift` | scene V-11. **Tidak ada di daftar berkas T3** | P1 |

Di luar lingkup: diagram, perbandingan rencana (P-19), insight otomatis, dan statistik aktual Trino lewat `GET /v1/query/<id>` (§1.1).

## 6. Aktivitas server di engine (W13-T4)

### 6.1 Perintah, rute, dan empat daftar

- Dua perintah: `sessions` (daftar) dan `session_cancel` (satu cancel). Nama berasal dari PRD NFR-S1 dan dipertahankan; **catatan:** sudah ada `session` (penyimpanan sesi jendela, `local.rs:439`), satu huruf dari `sessions`. Tes `RustEngineTests` menjaga kedua daftar tetap terpisah, dan bila pemilik lebih suka `activity` dan `activity_cancel` (cocok dengan nama berkas dan ADR 0043), penggantiannya hanya dua string di empat tempat plus PRD NFR-S1.
- `COMMANDS` (`lib.rs:462`): keduanya **sebelum `objects`** (aturan di `lib.rs:457`), `Command`, `EngineCommand` dan `EVERY_COMMAND` (`uniffi_api.rs:142`) bertambah dua dari panjangnya saat T4 jalan (26 hari ini; W11-A1 menambah tiga perintah dan W12-A1 satu, jadi dua lagi menjadi 32 bila keduanya mendarat lebih dulu), dan `RustEngine.commands`. Penambahan W11 ditaruh di antara `table_op` dan `objects`; milik T4 di titik yang sama, tepat sebelum `objects`. Baris usage yang dibandingkan `golden.rs:1184-1187` bertambah dua nama sebelum `objects`, dan literal harapan di tes itu diperbarui (sufiks beku setelah `objects` tidak berubah). `app/Generated/` diregenerasi (`./app/build-ffi.sh`, invariant #1 dan #11), dan `swift test --filter RustEngineTests` adalah pengawas yang asli.
- `host.rs:224` `route`: keduanya `Route::Pooled(Lane::Metadata)`. Alasannya ongkos koneksi dan beban server, **bukan** menghindari antrean. `host/pool.rs:3-7` berkata: "It is a reservation, not a queue: a checkout never waits for another Run to finish ... When the lane is full a new session is opened outside the reservation" (checkout di `:486-581`; PRD O-7 sama), jadi poll di lajur mana pun tidak menunggu Run. Bedanya ada di apa yang terjadi saat Run macet (UC-07). Lajur metadata memakai ulang satu sesi hangat yang dicadangkan untuk poll 3 detik dan tidak bersaing dengan dua reservasi Run (`pool.rs:34-36`). Di lajur query, selama kedua sesi query dipegang, tiap poll membuka koneksi baru di luar reservasi (`Take::Open`), dan sesi kelebihan itu ditutup lagi begitu daftar idle penuh (`park`, `pool.rs:695`): ongkos connect, TLS, dan SSH tiap tick, plus koneksi tambahan ke server produksi yang justru sedang diperiksa karena terbebani. P-05 sudah menyebut aktivitas sebagai perintah berpool.

### 6.2 Seam driver

Di `crates/qh-driver/src/lib.rs`, dengan implementasi bawaan `Usage` sehingga `Session` palsu tidak berubah:

```rust
pub const ACTIVITY_COLUMNS: [&str; 12] = ["id", "user", "database", "state", "application", "client",
    "query_ms", "xact_ms", "state_ms", "wait", "blocked_by", "query"];

pub struct ActivityPage {
    pub rows: Vec<Vec<Option<String>>>,   // sesuai ACTIVITY_COLUMNS; bilangan sebagai teks
    pub notes: Vec<String>,               // mis. "Lock waits are not shown: …"; menjadi done.warnings
}
pub enum CancelOutcome { Requested, NoSuchSession }

trait Session {
    async fn activity(&mut self) -> Result<ActivityPage, EngineError>;                    // bawaan: Usage
    async fn cancel_session(&mut self, id: &str) -> Result<CancelOutcome, EngineError>;   // bawaan: Usage
}
```

`host/lease.rs` meneruskan keduanya lewat `Op` dan `Reply` yang sudah dipakai `objects` (`lease.rs:487-492`); tanpa itu sesi pinjaman menjawab `Usage` untuk driver yang sebenarnya mendukung, jadi tes pool memeriksanya. Kolom yang tidak dimiliki sebuah driver berisi `NULL`.

### 6.3 SQL per driver (teks yang diuji)

**PostgreSQL** (teks akhir dijalankan di 17.11 tanpa galat, dan varian dengan `left(…, 60)` memberi baris yang benar untuk sesi uji, §1.3; `backend_type` butuh PostgreSQL 10 atau lebih baru, server lebih lama mengembalikan galat server apa adanya):

```sql
SELECT a.pid::text AS id, a.usename AS "user", a.datname AS database, a.state,
       a.application_name AS application, a.client_addr::text AS client,
       greatest(0, (extract(epoch FROM (clock_timestamp() - a.query_start)) * 1000))::bigint AS query_ms,
       greatest(0, (extract(epoch FROM (clock_timestamp() - a.xact_start)) * 1000))::bigint AS xact_ms,
       greatest(0, (extract(epoch FROM (clock_timestamp() - a.state_change)) * 1000))::bigint AS state_ms,
       CASE WHEN a.wait_event IS NULL THEN NULL ELSE a.wait_event_type || ':' || a.wait_event END AS wait,
       array_to_string(pg_blocking_pids(a.pid), ',') AS blocked_by,
       left(a.query, 2000) AS query
  FROM pg_stat_activity a
 WHERE a.backend_type = 'client backend' AND a.pid <> pg_backend_pid()
 ORDER BY (a.state = 'active') DESC, a.query_start NULLS LAST
 LIMIT 500
```

Umur dihitung di server (`clock_timestamp()`), bukan dari jam klien, supaya selisih jam tidak membuat durasi negatif. Untuk peran tanpa `pg_read_all_stats`, PostgreSQL mengisi `query` dengan `<insufficient privilege>` dan `state` dengan `NULL` untuk sesi milik peran lain (pengetahuan dokumentasi PostgreSQL; peran dev adalah superuser, jadi tidak diuji, §15).

**MySQL** (teks akhir dijalankan sebagai root tanpa galat; variannya dengan alias `db` dijalankan sebagai root dan sebagai `qh`, dan yang terakhir hanya melihat sesinya sendiri. `database` **harus** dikutip dengan backtick: `AS database` tanpa kutip menghasilkan `ERROR 1064` karena `DATABASE` kata cadangan MySQL):

```sql
SELECT CAST(p.ID AS CHAR) AS id, p.USER AS user, p.DB AS `database`, p.COMMAND AS state, p.HOST AS client,
       p.TIME * 1000 AS query_ms, p.STATE AS wait, LEFT(p.INFO, 2000) AS query
  FROM information_schema.PROCESSLIST p
 WHERE p.ID <> CONNECTION_ID() AND p.COMMAND <> 'Daemon'
 ORDER BY (p.COMMAND = 'Query') DESC, p.TIME DESC
 LIMIT 500
```

`xact_ms` dan `state_ms` kosong; `blocked_by` diisi dari pembacaan **kedua** yang opsional, `SELECT CAST(waiting_pid AS CHAR), CAST(blocking_pid AS CHAR) FROM sys.innodb_lock_waits`. Galat 1142, 1143, atau 1146 pada pembacaan kedua bukan kegagalan: daftar tetap kembali, dengan catatan "Lock waits are not shown: this user cannot read sys.innodb_lock_waits.", dan untuk pengguna tanpa `PROCESS` catatan kedua "MySQL lists only this user's own sessions." Kecocokan `waiting_pid` dengan `PROCESSLIST.ID` berasal dari dokumentasi skema `sys`, bukan dari uji dengan kunci nyata (§15).

**Trino** (teks akhir dijalankan di 483 dan mengembalikan satu baris, query uji itu sendiri, §1.2):

```sql
SELECT query_id AS id, "user", source AS application, state,
       greatest(0, date_diff('millisecond', coalesce(started, created), now())) AS query_ms,
       substr(query, 1, 2000) AS query
  FROM system.runtime.queries
 WHERE state NOT IN ('FINISHED', 'FAILED')
 ORDER BY created
 LIMIT 500
```

Baris yang `id`-nya sama dengan `session.query_id()` (query pendaftar itu sendiri) dibuang di Rust sesudah kursor habis. Driver Trino **tidak** diubah untuk mengirim `X-Trino-Source` (§13, R-6).

### 6.4 Cancel

`session_cancel` menerima `SESSION_ID`. Identifier dicek bentuknya **sebelum** guard dan sebelum menyambung, dan tidak pernah disisipkan mentah ke SQL:

| Driver | Bentuk id | Pernyataan | Hasil |
|---|---|---|---|
| PostgreSQL | bilangan bulat 1..2³¹−1 | `SELECT CASE WHEN <pid> = pg_backend_pid() THEN NULL ELSE pg_cancel_backend(<pid>) END` | `t` → `Requested`; `f` → `NoSuchSession`; `NULL` → `Usage` ("that is this connection"); galat hak akses server diteruskan apa adanya |
| MySQL | digit saja, ≤ 20 | `KILL QUERY <id>` | sukses → `Requested`; galat 1094 → `NoSuchSession`; 1095 → galat server apa adanya; id sama dengan `CONNECTION_ID()` sesi sendiri → `Usage` |
| Trino | `[0-9A-Za-z_]{1,64}` | `CALL system.runtime.kill_query(query_id => '<id>', message => 'Cancelled from QueryHive')` | sukses → `Requested`; `NOT_FOUND` → `NoSuchSession`; `ACCESS_DENIED` diteruskan |

Hanya cancel yang dibangun. `pg_terminate_backend`, `KILL CONNECTION`, dan padanan Trino tidak ada di kode (PRD UC-17: "Terminate tidak ditawarkan"). `Requested` berarti permintaan terkirim, bukan bahwa query sudah berhenti: PostgreSQL mengembalikan `t` saat sinyal terkirim, dan app menyegarkan daftar untuk melihat hasilnya. `NoSuchSession` bukan galat (cancel idempoten, seperti `Session::cancel`): event `done` membawa `warnings: ["no session with that id; it may have just ended"]`.

### 6.5 Guard cancel (D-13)

`activity.rs` memuat:

```rust
/// A cancel stops work somebody else is doing, so it is a write as far as Safe Mode is concerned:
/// `full` and `no_ddl` allow it, `confirm` asks, `read_only` refuses. The statement text is never
/// the victim's SQL: the log holds `CANCEL SESSION <id>` and nothing else.
fn guard_cancel(settings: &Settings, engine: &dyn Engine, id: &str) -> Result<(), CliError>
```

Mengambil `safe_mode(settings, engine)?` (mode terselesaikan, sudah memasukkan `DB_READ_ONLY`, `SAFE_MODE_FLOOR`, dan kemampuan driver), lalu memanggil `record_decision` dengan `StatementKind::Dml`, `mode.decision(Dml)`, `mode.refusal(Dml)`, subjek `CANCEL SESSION <id>`, dan `safe_mode_confirmed(settings)`. Mode terselesaikan **cukup**: urutan `ReadOnly > Confirm > NoDdl > Full` monoton terhadap keputusan itu (§1.5), jadi tidak ada padanan lubang ADR-0027 yang membutuhkan `destructive_mode` (yang diperlukan karena DDL ditolak di `confirm`, sedangkan cancel tidak punya perlakuan khusus di `confirm`). Tes mengunci kemonotonan itu, bukan menganggapnya.

`sessions` tidak dijaga dan tidak dicatat: SQL-nya dibuat engine, hanya membaca katalog sistem, dan tidak mengubah apa pun. Tampilan isinya dibatasi hak server (§6.3). `STATEMENT_TIMEOUT_MS` berlaku; app mengirim 5.000 ms untuk daftar supaya polling tidak pernah menjadi beban di produksi.

Pencatatan bergantung pada sink yang terpasang di host app, yang hari ini **belum ada** dan dipasang W11-T1 (§8.1). Tanpa itu cancel tetap bekerja tetapi tidak tercatat, dan itu melanggar FR-SAFE-03; karena itu T4 tidak boleh mendarat sebelum sink itu ada.

### 6.6 Bentuk event

`sessions`: `step connect`, `columns` (12 kolom `ACTIVITY_COLUMNS`, semua bertipe `varchar`), `rows` (batch `PREVIEW_BATCH`), `done {rows, elapsed_ms, warnings?}`. `session_cancel`: `step connect`, `done {rows: 0, elapsed_ms, warnings?}`. Hanya pembentukan event baru; `Event` Swift tidak berubah (`columns`, `data`, `warnings`).

### 6.7 Tes dan gate (W13-T4)

`crates/qh-ffi/tests/safe_mode.rs` (NFR-S1), dengan `CountingEngine` dan `RecordingSession`:

| Tes | Isi |
|---|---|
| `a_cancel_is_refused_in_read_only_before_connecting` | `connects == 0`, satu baris log `refused` dengan subjek `CANCEL SESSION 4123` |
| `a_cancel_asks_in_confirm_and_runs_once_confirmed` | tanpa `SAFE_MODE_CONFIRMED` → `NeedsConfirmation`; dengan → jalan dan log `confirmed` |
| `a_cancel_runs_in_full_and_no_ddl` | keduanya `allowed` |
| `the_cancel_guard_is_monotone_across_the_floors` | semua pasangan `SAFE_MODE` x `SAFE_MODE_FLOOR`: hasilnya sama dengan mode yang lebih ketat (ADR-0027) |
| `a_session_id_is_checked_before_anything_else` | `4123; DROP TABLE t`, `1 OR 1=1`, `'`, string kosong, angka di luar rentang, dan id Trino dengan tanda kutip: `Usage`, `connects == 0`, tidak ada baris log |
| `sessions_is_allowed_in_read_only` | bacaan tetap jalan di mode paling ketat |
| `the_own_session_is_not_listed` | sesi palsu mengembalikan baris dirinya (`pg_backend_pid`, `CONNECTION_ID`, `query_id`) dan engine membuangnya (Trino) atau SQL-nya yang membuang (diperiksa lewat teks SQL) |
| `mysql_cancel_keeps_the_backslash_and_hash_readings` | id MySQL hanya digit, jadi teks `\` dan `#` tidak punya jalan masuk; kasus escape ada demi NFR-S1 |

Tes driver: teks SQL persis per driver, pemetaan `Requested` dan `NoSuchSession`, catatan opsional MySQL saat pembacaan kedua ditolak (kode 1142 palsu). `RustEngineTests` memeriksa kedua nama. Gate: G-RUST, G-FFI, G-SWIFT, G-LIVE (**cancel pada `pg_sleep` di sesi lain** persis seperti §1.3; MySQL dengan `SELECT SLEEP(12)`; Trino dengan query panjang di sesi lain dan `kill_query`, yang belum terbukti di §1.2). Pemeriksa: DB, SEC, RR, SF. Commit: `feat(engine): list server sessions and cancel one, under Safe Mode`.

### 6.8 Perubahan per berkas (W13-T4)

| Berkas | Isi | Prioritas |
|---|---|---|
| `crates/qh-driver/src/lib.rs` | `ACTIVITY_COLUMNS`, `ActivityPage`, `CancelOutcome`, dua metode bawaan. **Tidak ada di daftar berkas T4** | P0 |
| tiga driver | `activity`, `cancel_session`, SQL dan tes teks | P0 |
| `crates/qh-ffi/src/host/lease.rs` | meneruskan keduanya. **Tidak ada di daftar berkas T4** | P0 |
| `crates/qh-ffi/src/activity.rs` (baru) | `sessions`, `session_cancel`, `guard_cancel`, validasi id | P0 |
| `crates/qh-ffi/src/{lib.rs,uniffi_api.rs,host.rs}`, `Support/RustEngine.swift`, `app/Generated/` | empat daftar, `route`; baris usage di `tests/golden.rs`. `host.rs` **tidak ada di daftar berkas T4** | P0 |
| `crates/qh-ffi/tests/safe_mode.rs`, `real_server.rs` | §6.7 | P0 |

Sub-tugas **W13-T4b** (kolom koneksi di execution log, §8.2) berjalan di lajur yang sama sesudah T4 dan sebelum T6; berkasnya ada di §8.2.

## 7. Tampilan aktivitas server (W13-T5)

### 7.1 Model

`ServerSession` (di `AppModel+Activity.swift`) dibangun dari satu baris `[String?]` **menurut nama kolom** dari event `columns`, bukan menurut posisi: kolom yang hilang menjadi `nil`, kolom asing diabaikan. Bagian: `id`, `user`, `database`, `state`, `application`, `client`, `queryMillis`, `xactMillis`, `stateMillis`, `wait`, `blockedBy: [String]` (dipecah koma), `query`. `isThisApp`: `application == "QueryHive"` (PostgreSQL; MySQL dan Trino tidak punya tanda itu).

`ActivityState` per koneksi: `sessions`, `notes` (dari `done.warnings`), `refreshedAt`, `error`, `inFlight`, `paused`, `cancelling: Set<String>`.

### 7.2 Pembaruan

- `Engine.current.run("sessions", env:, …)` dengan `connectionEnvironment(connection)` plus `STATEMENT_TIMEOUT_MS=5000`. Satu pembacaan sekaligus: tick yang jatuh saat pembacaan sebelumnya belum selesai dilewati.
- Interval 3 detik selama jendela terlihat dan app aktif; berhenti saat jendela tertutup, tersembunyi, atau app nonaktif, dan satu pembacaan langsung saat kembali. Tombol Segarkan dan Jeda tersedia. Galat (koneksi putus, hak ditolak) ditampilkan sebagai banner dan polling mundur ke 10 detik sampai berhasil.
- Daftar yang kembali menggantikan yang lama utuh, dengan pilihan baris dipertahankan menurut `id`.

### 7.3 Cancel

1. Tombol Cancel Query aktif bila ada baris terpilih dan `connection.safeMode != .readOnly`. Bila barisnya `isThisApp`, sheet menambah satu kalimat: "This is a QueryHive session; cancelling it stops a run in one of your tabs." (diperingatkan, tidak diblokir). Di `read_only` tombol mati dengan bantuan "This connection is read-only, and cancelling someone's query is a write."
2. Sheet konfirmasi di **semua mode kecuali `read_only`** (D-14), dihosting jendela aktivitas sendiri (bukan `awaitConfirmation` jendela utama, yang `private` di `AppModel.swift:1989` dan menampilkan sheet di jendela lain). Isinya memakai bentuk `RunConfirmation.Request` (`RunConfirmation.swift:20`, tanpa mengubah berkas itu); `destructiveRequest` (`:57`) hanya mengembalikan nilai di `confirm`, jadi T5 membuat `Request` sendiri untuk mode lain. Judul "Cancel query {id}?", isi pengguna, durasi, dan 200 karakter pertama query (`Text(verbatim:)`), catatan "The server stops the statement it is running. The session stays connected. This cannot be undone." Pada `confirm`, persetujuan juga membawa `SAFE_MODE_CONFIRMED=1` (`RunConfirmation.approvalSettings(true)`); pada mode lain, engine tidak memintanya.
3. `Engine.current.run("session_cancel", env: connectionEnvironment + ["SESSION_ID": id])`. `done` tanpa peringatan berarti "Cancel requested for {id}"; dengan `warnings` ditampilkan; `error` tampil di banner jendela. Setelah 500 ms daftar disegarkan satu kali supaya hasilnya terlihat. Teks query korban tidak masuk `tab.note`, history, atau log aplikasi.

### 7.4 Tampilan

- **Jendela sendiri** (D-14): `WindowGroup("Server Activity", for: UUID.self)` di `App.swift`, dibuka dari menu konteks node koneksi di pohon (`SchemaOutline.swift`, item "Server Activity…") lewat `openWindow(value:)`. Judul jendela memuat nama koneksi. Cadangan bila T5 tidak diberi `App.swift`: `.sheet(item:)` dari `SchemaOutline.swift`; sheet itu modal terhadap jendela, jadi diagnosis macet tidak bisa berlangsung sambil mengedit.
- `Table` SwiftUI (macOS 14) dengan kolom Id, User, Database, State, Duration, Wait, Blocked by, Query; urutan bawaan: yang diblokir lebih dulu, lalu yang aktif terlama. Baris yang memblokir sesi lain ditandai (simbol dan teks "Blocking {n}"), baris yang diblokir menampilkan "Blocked by {id}": bukan hanya warna. Menu baris: Cancel Query…, Copy Query, Copy Id. Footer: "{n} sessions · refreshed {waktu}", status jeda, dan catatan `notes` (hak MySQL, lock wait tak terbaca, baris `<insufficient privilege>`).
- Query ditampilkan satu baris dengan `.help` penuh dan `.textSelection(.enabled)`; tidak pernah diteruskan ke editor otomatis.
- Aksesibilitas: setiap baris satu elemen dengan label "{user}, {state}, {durasi}, query {…}, {blocking/blocked}"; Table dapat dijangkau keyboard; Cancel punya pintasan `⌘⌫` hanya saat jendela aktif dan baris terpilih (didaftarkan di jendela, bukan di menu global).

### 7.5 Tes dan berkas (W13-T5)

`ServerSessionTests` (parse menurut nama kolom, kolom hilang, `blocked_by` kosong dan berisi dua id), `ActivityFlowTests` dengan `MockEngine` (tidak ada tick tumpang tindih, jeda dan lanjut, galat memperlambat polling, lingkungan Cancel memuat `SESSION_ID` dan `SAFE_MODE_CONFIRMED` hanya setelah persetujuan pada `confirm`, `read_only` tidak pernah memanggil engine, cancel tidak menulis ke history atau log tab), dan scene V-11: `server-activity-postgres-blocked`, `server-activity-mysql-limited`, `server-activity-trino`, `server-activity-readonly`, `cancel-confirm-sheet`. Gate: G-SWIFT, G-VIS (scene baru). Pemeriksa: SR, UX, AX. Commit: `feat(app): a server activity view with cancel`.

| Berkas | Isi | Prioritas |
|---|---|---|
| `Views/ServerActivity.swift` (baru), `Models/AppModel+Activity.swift` (baru) | §7.1 sampai §7.4 | P0 |
| `Views/SchemaOutline.swift` | item menu konteks | P0 |
| `App.swift` | scene jendela. **Tidak ada di daftar berkas T5**; rantai `App.swift` di §7 `development-plan.md` perlu "… → W12-T2 → W13-T5 → W13-T8b" | P0 (atau cadangan sheet) |
| `Support/Snapshot.swift`, `VisualParityTests.swift` | scene V-11. **Tidak ada di daftar berkas T5** | P1 |
| `Tests/QueryHiveTests/{ServerSessionTests,ActivityFlowTests}.swift` (baru) | di atas | P0 |

## 8. Penampil execution log (W13-T6)

### 8.1 Prasyarat 1: sink terpasang di host app (milik W11-T1)

`execution_log::install_from_settings` hanya dipanggil `queryhive-engine` (`main.rs:66`) dan `queryhive-mcp` (`bin/mcp.rs:91`). `grep -rn "execution_log::install"` tidak menemukan pemanggil lain di `crates/qh-ffi/src`, dan tidak ada panggilan Swift terkait di `app/Sources` atau `app/Generated`. Selama app memakai `EngineHost` dalam proses, keputusan Safe Mode app tidak ditulis ke log; hanya Run lewat CLI dan MCP yang tercatat.

W11-A1 menemukan hal yang sama sendiri dan memutuskan (W11 D-8, §3.5): `execution_log` adalah perintah `Route::Local`; sink dipasang malas di `EngineHost` pada `run` pertama (`execution_log::install(local::open_storage(settings))`, sekali per proses); kegagalan membuka log tidak menghentikan perintah tetapi dilaporkan lewat `"writer": false`; dan satu tes `EngineHost::run(preview, SAFE_MODE=read_only, "DELETE FROM t")` menambah satu baris `refused`. Blueprint ini **bergantung** pada keputusan itu dan hanya menambah syarat dari sisi W13:

- T4: tes `EngineHost::run(session_cancel, SAFE_MODE=read_only, SESSION_ID=4123)` menambah satu baris `refused` dengan subjek `CANCEL SESSION 4123`, dan `confirm` dengan `SAFE_MODE_CONFIRMED=1` menambah baris `confirmed`.
- T6: bila `writer == false`, penampil menampilkan banner "The log is not being written to in this session" dan bukan daftar kosong yang tampak sehat.
- Tes Swift yang menjalankan Run lewat host nyata harus memakai `DB_PATH` terisolasi (`TestIsolation`), karena sink bersifat per proses dan akan menulis ke database pengguna bila tidak.

Bila W11-T1 tidak mendarat dengan sink, W13-T4 tidak boleh mendarat sebelum langkah itu ditambahkan ke dalamnya.

### 8.2 Prasyarat 2: kolom koneksi (W13-T4b, bergantung pada OQ-3)

FR-SAFE-02 meminta "filter per koneksi". Tabel `execution_log` tidak punya kolom koneksi (`0007_execution_log.sql`), `NewExecution` dan `DecisionEntry` tidak membawanya, dan `connectionEnvironment` di app (`AppModel.swift:3312-3361`) tidak mengirim `CONNECTION_ID` (hanya `history_add` dan `saved_queries` yang melakukannya). W11 §3.5 menunda keputusan ini ke W13 dan merekomendasikan kolom di dalam rantai dengan versi rantai. Blueprint ini mengambilnya sebagai **W13-T4b**, satu commit sendiri di lajur Rust sesudah T4 (cancel juga perlu id koneksi di barisnya) dan sebelum T6:

- Migrasi `0009_execution_log_connection.sql`: `ALTER TABLE execution_log ADD COLUMN connection_id TEXT` dan `ADD COLUMN chain_version INTEGER NOT NULL DEFAULT 1`. Baris lama tetap versi 1 dan `connection_id` `NULL`.
- Tata letak hash menurut `chain_version`: versi 1 tetap larik 9 elemen (rantai lama diverifikasi tanpa perubahan), versi 2 larik 10 elemen dengan `connection_id` sebelum `prev_hash`. Baris baru selalu versi 2 (`connection_id` boleh `null` di dalam larik), supaya tata letak baris baru seragam. Membalik `chain_version` atau mengisi atau menghapus `connection_id` pada baris yang sudah ada memecahkan hash baris itu, sehingga pemalsuan tetap terdeteksi; `verify_execution_log` memilih tata letak dari kolom itu. (Pilihan lain yang setara: tata letak menurut kehadiran `connection_id` tanpa kolom versi; kolom versi dipilih karena eksplisit dan siap untuk perubahan berikutnya, sesuai rekomendasi W11.)
- `NewExecution`, `ExecutionRecord`, `COLUMNS`, `read_record`, dan `append_execution` membawa `connection_id: Option<&str>`.
- `execution_log::record` mengambil id koneksi dari konteks runtime (task-local yang dipasang `crate::run` dan dispatch host dari `CONNECTION_ID` di setelan), bukan dari parameter: `guard` dan `guard_confirmed` sengaja tidak membawa `Settings` (alasan sink di `execution_log.rs`), dan jalur `apply.rs`, `import.rs`, dan `mcp.rs` memanggil guard yang sama. Task-local tidak menyeberang `spawn`; guard berjalan inline di tugas perintah, dan tes memeriksa tiap pemanggil guard.
- Pembaca W11 menambah `connection_id` pada tiap keputusan dan setelan `EXECUTION_LOG_CONNECTION_ID` (saringan; verifikasi rantai tetap atas seluruh tabel).
- App mengirim `CONNECTION_ID` di `connectionEnvironment(_ connection:)` (satu baris; wrapper instans mengenal `connection.id`; berkas koneksi milik W11).
- Teks migrasi 0007 menyatakan "Hosts, users, passwords and the Keychain reference live nowhere here". Id koneksi adalah UUID buram, bukan host, pengguna, atau rahasia, dan komentar migrasi 0009 mengatakannya.

T4b menyentuh format rantai audit, jadi SEC adalah pemeriksa wajib (selain DB dan RR). Gate: G-RUST (termasuk tes bahwa rantai versi 1 yang direkam lebih dulu tetap terverifikasi, dan rantai campuran versi 1 dan 2 terverifikasi), G-SWIFT. Commit: `feat(storage): the execution log records which connection a decision was about`. Bila pemilik menolak (OQ-3), T4b dan filter di penampil dilewati dan FR-SAFE-02 dicatat terpenuhi sebagian.

### 8.3 Kontrak pembaca (dari W11-A1)

Perintah `execution_log`, `Route::Local`, hanya baca (`w11-metadata-and-connections.md` §3.3 dan §3.5):

- Setelan: `EXECUTION_LOG_LIMIT` (bawaan 200, paling banyak 5.000; di atasnya `Usage`) dan `EXECUTION_LOG_VERIFY` (bawaan `1`). T4b menambah `EXECUTION_LOG_CONNECTION_ID`.
- Satu event `execution_log` dengan `decisions` (larik objek `seq, id, at, safe_mode, decision, statement_kind, statement_index, statement_hash, reason`, ditambah `connection_id` setelah T4b; terbaru lebih dulu, `Storage::execution_log(limit)` di `execution_log.rs:227`), `chain` (`{"verified":true,"rows":N}`, atau `{"verified":false,"seq":N,"detail":"…"}` yang **tetap dikirim bersama baris**, dari `Storage::verify_execution_log`, `:245`), dan `writer` (bool). Tidak ada kolom statement: tabel hanya menyimpan hash. Kunci `Event` baru (`decisions`, `chain`) ditambahkan W11-T1 sekali di `App.swift`; T6 hanya memakainya.
- Tidak ada `BEFORE_SEQ`: W11 memilih satu batas. "Load older" di penampil menaikkan `EXECUTION_LOG_LIMIT` (200, 1.000, 5.000), cap tunggal yang sama dengan D-1. Lebih dari 5.000 baris tidak ditawarkan di W13 (backlog: ekspor).
- Verifikasi atas seluruh tabel berjalan di engine, O(N) dengan satu SHA-256 per baris; app boleh mengirim `EXECUTION_LOG_VERIFY=0` pada log yang membengkak dan menampilkan "not verified".

Bila kontrak W11 berubah, §8.4 mengikutinya dan bagian ini diperbarui.

### 8.4 Tampilan

`PanelTab.audit` ("Audit") di `BottomPanel` (`Panels.swift:6`, pola `HistoryPanel`, `:378`), `Views/ExecutionLogView.swift`, `Models/AppModel+Audit.swift`:

- Daftar: waktu, mode efektif, keputusan (simbol dan teks: allowed, confirmed, refused, needs_confirmation), jenis, 12 karakter pertama hash statement (`Text(verbatim:)`, menu Salin hash penuh), alasan, dan nama koneksi (id dipetakan ke nama; id yang sudah tidak ada tampil "(removed connection)"). Filter: koneksi (bawaan: koneksi tab, hanya bila T4b ada) dan keputusan. "Load older" menaikkan `EXECUTION_LOG_LIMIT` (§8.3).
- Banner `writer == false` (§8.1) dan banner status rantai di atas daftar: "Chain intact · {n} decisions" atau "Chain broken at row {seq}: rows were edited, inserted or removed after they were written." dengan bantuan: "The chain detects changes made after the fact. Someone who can rewrite the whole file can rewrite the chain too." (kalimat jujur dari `qh-storage/src/execution_log.rs:10-16`: tamper-evident, bukan tamper-proof; tidak dijual lebih dari itu.)
- Hanya baca: tidak ada hapus, kosongkan, atau ekspor (log append-only, migrasi 0007). Statement tampil sebagai hash (FR-SAFE-02); mencari "apakah statement ini pernah dinilai" lewat hash yang dihitung dari teks adalah item backlog karena pemecah statement engine tidak boleh diduplikasi di Swift.
- Badge jumlah di tab kosong (`count` di `Panels.swift:91` mengembalikan `nil`).
- `PanelTab.audit` menyentuh `Models/QueryTab.swift` (enum, `label`), yang dimiliki T1 di W13: T6 jalan sesudah T1 pada berkas itu.

### 8.5 Tes dan berkas (W13-T6)

`ExecutionLogViewTests`: parse event `execution_log` (`decisions`, `chain`, `writer`), banner utuh, putus, dan `writer == false`, filter koneksi dan keputusan, "Load older" (batas 200, 1.000, 5.000), pemetaan id ke nama dan "(removed connection)", tidak ada teks statement di mana pun. Scene V-11: `audit-chain-intact`, `audit-chain-broken`. Gate: G-SWIFT, G-VIS (scene baru). Pemeriksa: SR, UX, AX, SEC. Commit: `feat(safety): read the execution log and whether its chain holds`.

| Berkas | Isi | Prioritas |
|---|---|---|
| `Views/ExecutionLogView.swift` (baru), `Models/AppModel+Audit.swift` (baru) | §8.4 | P0 |
| `Views/Panels.swift` | tab Audit di `BottomPanel` | P0 |
| `Models/QueryTab.swift` | `PanelTab.audit`. **Tidak ada di daftar berkas T6** | P0 |
| `Support/Snapshot.swift`, `VisualParityTests.swift` | scene V-11. **Tidak ada di daftar berkas T6** | P1 |
| `Tests/QueryHiveTests/ExecutionLogViewTests.swift` (baru) | di atas | P0 |

## 9. Katalog string di bundel (W13-T7)

### 9.1 Tata letak dan build (D-16)

- `app/Resources/Localizable.xcstrings`: JSON katalog (`sourceLanguage: "en"`, `version: "1.0"`, `extractionState: "manual"` karena tidak ada ekstraksi Xcode di SwiftPM). Letaknya di luar `Sources/QueryHive`, jadi SwiftPM tidak melihatnya dan **`Package.swift` tidak berubah**: tanpa `resources:` tidak ada `Bundle.module` dan tidak ada kebutuhan `defaultLocalization` (baris T7 di rencana menyebutnya; hanya berlaku bila resource SwiftPM kelak dipakai).
- `app/build.sh`, sesudah penyalinan ikon (`:117`) dan sebelum loop penandatanganan: `xcrun --find xcstringstool` (gagal dengan pesan jelas bila tidak ada, karena mesin tanpa Xcode tidak punya alatnya, §1.7), `xcstringstool compile Resources/Localizable.xcstrings --output-directory "$app/Contents/Resources"`, `plutil -replace CFBundleDevelopmentRegion -string en` dan `plutil -replace CFBundleLocalizations -json '["en"]'` pada `Info.plist` (heredoc di `:77-95` tidak memuatnya), lalu pemeriksaan: `en.lproj/Localizable.strings` ada, tidak kosong, dan lolos `plutil -lint`. Langkah pertama T7 menjalankan `--dry-run` untuk mencatat tata letak keluaran sebenarnya (hanya `--help` yang dibaca di blueprint ini).
- Pembacaan lewat `Bundle.main`, bundel app yang dirakit `build.sh`. Di `swift test` dan `--snapshot`, `Bundle.main` tidak memuat katalog, sehingga setiap string kembali ke nilai bawaannya (teks Inggris, kunci itu sendiri): **G-VIS tidak berubah** dan baseline tidak direkam ulang.
- Bahasa: hanya `en`. Terjemahan Indonesia tidak ada di lingkup W13 (P-20 menyebut infrastruktur dan string di permukaan yang disentuh).

### 9.2 `Support/L10n.swift` dan aturan

```swift
enum L10n {
    /// Looked up in the main bundle's Localizable table. The English key is its own fallback.
    static func string(_ key: String.LocalizationValue, comment: StaticString) -> String
}
extension L10n {
    enum Pagination {
        static var fetchMore: String { L10n.string("Fetch more rows", comment: "Footer button: raises the row limit and runs the statement again") }
        static func goToRowNeeds(row: Int, limit: Int) -> String {
            L10n.string("Row \(row) needs the first \(limit) rows. The statement runs again.", comment: "Go to row popover")
        }
    }
    // Plan, Activity, Audit, Analytics (T8c), …
}
```

1. **Kunci = teks Inggris**, bukan kunci simbolik. Literal SwiftUI (`Text("…")`, `Button("…")`, `Label`, `.help`, `.accessibilityLabel`, `Toggle`, `Picker`, `Section`) dan `L10n.string` memakai satu ruang kunci yang sama.
2. **Di berkas yang dikonversi, literal SwiftUI tidak berisi interpolasi.** Apa pun yang berisi nilai lewat fungsi `L10n` (kunci hasil interpolasi `%lld` dan `%@`). `Text(variabel)` dengan `String` berarti verbatim (tanpa pencarian), jadi tidak bisa menyelinap sebagai string yang tampak terlokalisasi.
3. **Data tidak pernah masuk katalog:** SQL, nama objek, hash, teks rencana, pesan server dan engine (Rust menulis Inggris dan berada di luar app). Selalu `Text(verbatim:)` atau `String`.
4. **Jamak lewat variasi katalog** (`one` dan `other`), dengan fungsi `L10n` yang menerima `Int`. `pluralized(_:_:_:)` (`Theme.swift:618`) tetap untuk string yang belum dikonversi dan tidak dipakai di berkas yang dikonversi.
5. Komentar untuk penerjemah wajib.
6. Pemformatan angka tetap `formatted()` (mengikuti locale), tanpa format manual.

### 9.3 Cakupan (P-20)

- **Dikonversi penuh:** setiap berkas yang dibuat di W9 sampai W13 (di W13: `PlanTreeView.swift`, `ServerActivity.swift`, `ExecutionLogView.swift`, dan panel Analitik T8c).
- **Dikonversi sebagian:** pada berkas yang hanya diubah, string yang ada di hunk yang ditambah atau diubah W9 sampai W13 (diperoleh dari `git diff <commit sebelum W9>..HEAD -- app/Sources`).
- **Tidak disentuh:** string lama di luar hunk. Pesan commit T7 mencatat jumlah kunci dan daftar berkas yang dikonversi.
- Inventaris kandidat dapat dibantu `xcstringstool extract` atas berkas yang dipilih; perilakunya belum diuji di sini (§15).

### 9.4 Tes dan gate

- `L10nCatalogTests`: katalog adalah JSON sah; setiap entri punya nilai `en` dan komentar; tidak ada kunci ganda; setiap kunci literal di `L10n.swift` ada di katalog dan sebaliknya (tanpa yatim); untuk daftar berkas yang dikonversi (larik di tes), setiap literal tanpa interpolasi pada panggilan SwiftUI di atas ada di katalog, dan literal berinterpolasi pada panggilan itu menggagalkan tes (aturan 2).
- `L10nBundleTests`: bundel sementara berisi `xx.lproj/Localizable.strings` buatan: `L10n.string` (varian internal yang menerima `Bundle`) memakai terjemahan, dan kunci yang tidak ada kembali ke teks Inggris.
- G-APP (bundel memuat string dari katalog): setelah `./app/build.sh`, `Contents/Resources/en.lproj/Localizable.strings` ada dan lolos `plutil -lint`; `Info.plist` memuat `CFBundleDevelopmentRegion` dan `CFBundleLocalizations`; jumlah kunci di berkas yang dikompilasi sama dengan katalog. G-SWIFT hijau dan G-VIS tidak berubah (nol baseline direkam ulang).

Gate: G-SWIFT, G-VIS (tidak berubah), G-APP. Pemeriksa: SR, UX, AX. Commit: `feat(app): a string catalog, and the strings in the new surfaces go through it`.

### 9.5 Perubahan per berkas (W13-T7)

| Berkas | Isi | Prioritas |
|---|---|---|
| `app/Resources/Localizable.xcstrings` (baru) | katalog `en` | P0 |
| `app/build.sh` | §9.1 | P0 |
| `app/Sources/QueryHive/Support/L10n.swift` (baru) | §9.2 | P0 |
| berkas W9 sampai W13 yang dikonversi | §9.3 | P0 |
| `app/Tests/QueryHiveTests/{L10nCatalogTests,L10nBundleTests}.swift` (baru) | §9.4 | P0 |
| `app/Package.swift` | **tidak berubah** (rencana menyebut `defaultLocalization`) | n/a |

## 10. Titik temu dengan helper analitik (T8a sampai T8c)

Tidak ada duplikasi dari `fase-6` §14, §21.6 sampai §21.8. Satu-satunya hal yang W13 tambahkan adalah urutan pada berkas bersama:

| Berkas bersama | Urutan | Catatan |
|---|---|---|
| `app/Generated/`, `uniffi_api.rs`, `Support/RustEngine.swift`, empat daftar (lajur FFI) | T4 → T8b | T4 menambah dua perintah NDJSON (invariant #11); T8b menambah objek UniFFI `AnalyticsSession` yang **bukan** perintah (`fase-6` §14.13). Tidak berebut daftar; `Generated` diregenerasi dua kali, dan T8b memulai dari kepala yang sudah memuat T4 |
| `crates/qh-ffi/src/host.rs` | W11-T1 (sink) → T4 (rute) → T4b → T8b | serial |
| `App.swift` | T5 (scene jendela) → T8b (`SIGPIPE`, `configure_analytics`) | `fase-6` §21.7 tidak berubah |
| `Views/SettingsView.swift` | T8c → T7 | rantai yang ada; string panel Analitik masuk katalog di T7 |
| `app/build.sh` | T8b (`ANALYTICS=1`) → T7 (xcstringstool) | dua blok mandiri sebelum loop penandatanganan |
| `Support/Snapshot.swift`, `VisualParityTests.swift` | T1, T3, T5, T6 → T8c | scene V-11 |
| `Cargo.toml`, `Cargo.lock` | hanya T8a | T1 sampai T7 tidak menambah crate; T2 dan T4 memakai yang sudah ada |

T1 sampai T7 tidak menjalankan G-ANALYTICS dan tidak boleh mengimpor apa pun dari helper (`fase-6` §14.14: semua yang bekerja tanpa helper tetap begitu). NFR-P3 untuk skenario SQL dan gate W13 (ambang dari angka `bench_ffi sql-*`) tidak disentuh blueprint ini.

## 11. Urutan build dan gate

0. **Prasyarat lintas gelombang, sebelum W13-T4:** sink execution log di host app, milik W11-T1 (W11 D-8, §8.1).
1. **Batch 1:** T1 dan T2. **T6** dipindah ke akhir Batch 2 (sesudah T1 pada `QueryTab.swift`, sesudah T4b bila ada filter koneksi, dan bergantung pada pembaca dan sink W11-T1); rencana menaruhnya di Batch 1, dan satu-satunya yang memaksa pemindahan adalah T4b.
2. **Batch 2:** T3 (setelah T1 dan T2), T4 (setelah T2: berbagi `qh-driver`, tiga driver, dan `lease.rs`), lalu **T4b** (kolom koneksi, bila OQ-3 disetujui) sebelum T6 membaca filternya.
3. **Batch 3:** T5 (setelah T4) dan T8a.
4. **Batch 4:** T8b. **Batch 5:** T8c. **Batch 6:** T7 sendirian.
5. **W13-D** (ADR 0043, 0044, 0046, adendum 0038, 0007, 0010, 0014) dan **W13-C**, lalu gate W13: G-HEAVY, G-ANALYTICS, G-BENCHQ (NFR-P9: footer dan `explain` tidak boleh memundurkan sumbu 1, 4, atau 5 lebih dari 5%), dan ambang NFR-P3 untuk SQL.

| Tugas | Gate yang dijalankan | Pemeriksa |
|---|---|---|
| T1 | G-SWIFT, G-VIS | SR, UX, AX |
| T2 | G-RUST, G-GOLDEN (tanpa selisih), G-LIVE PostgreSQL dan Trino | RR, DB, SEC |
| T3 | G-SWIFT, G-VIS | SR, UX, AX |
| T4 | G-RUST, G-FFI, G-SWIFT, G-LIVE (cancel di sesi lain) | DB, SEC, RR, SF |
| T4b | G-RUST (rantai versi 1, campuran, dan versi 2 terverifikasi), G-SWIFT | DB, SEC, RR |
| T5 | G-SWIFT, G-VIS | SR, UX, AX |
| T6 | G-SWIFT | SR, UX, AX, SEC |
| T7 | G-SWIFT, G-APP, G-VIS (tidak berubah) | SR, UX, AX |

Risiko tinggi menurut `AGENTS.md` (guard Safe Mode, jalur FFI, jalur yang dapat menghilangkan data): T2 (guard di engine; classifier hanya tes), T4 (guard cancel dan FFI), dan T4b (format rantai audit) direview model terkuat ditambah reviewer keamanan dan database, satu putaran (O-20); T1, T3, T5, T6 sedang, T7 rendah.

## 12. Perubahan rencana dan keputusan untuk pemilik

### 12.1 Keputusan yang butuh pemilik

| ID | Pertanyaan | Bawaan blueprint | Alternatif |
|---|---|---|---|
| OQ-1 | ANALYZE hanya untuk statement baca (D-10)? | **Ya**, di semua mode | (a) sesuai PRD harfiah: ANALYZE untuk tulis di bawah keputusan Safe Mode yang sama, plus konfirmasi selalu di app; (b) ANALYZE tulis di dalam transaksi yang di-rollback (hanya PostgreSQL, butuh ADR, tidak melindungi sekuens dan efek luar) |
| OQ-2 | Tampilan aktivitas sebagai jendela (D-14)? | **Jendela**, butuh T5 memiliki `App.swift` | sheet dari `SchemaOutline.swift` (modal) |
| OQ-3 | Filter per koneksi di penampil audit (§8.2)? | kerjakan W13-T4b (migrasi 0009 dengan `chain_version`), seperti rekomendasi W11 §3.5 | tolak: penampil tanpa filter koneksi, FR-SAFE-02 terpenuhi sebagian dan diamandemen |
| OQ-4 | Nama perintah (§6.1)? | `sessions`, `session_cancel` (PRD NFR-S1) | `activity`, `activity_cancel` (hindari kemiripan dengan `session`) |

### 12.2 Perubahan pada `development-plan.md` (untuk orkestrator)

1. **Sink execution log di host app** (§8.1): sudah diambil W11-A1 (D-8) untuk W11-T1; tidak ada perubahan rencana selain mencatat bahwa W13-T4 dan T6 bergantung padanya, dan bahwa T4 tidak boleh mendarat tanpanya.
2. **Sub-tugas baru W13-T4b** (§8.2, OQ-3): migrasi 0009, `chain_version`, konteks runtime di `execution_log::record`, setelan `EXECUTION_LOG_CONNECTION_ID` pada pembaca W11, dan satu baris `CONNECTION_ID` di `connectionEnvironment`. Ukuran S sampai M, GP-s, reviewer DB, SEC, RR. Berkasnya: `crates/qh-storage/migrations/0009_execution_log_connection.sql`, `crates/qh-storage/src/{execution_log.rs,lib.rs}` (+ tes), `crates/qh-ffi/src/{execution_log.rs,lib.rs,host.rs}`, `crates/qh-ffi/tests/{execution_log.rs,safe_mode.rs}`, dan satu baris Swift di berkas koneksi W11. `qh-storage` belum punya pemilik di §7.
3. **Kepemilikan berkas, tambahan di §7:**
   - `crates/qh-driver/src/lib.rs`: W11-T1 → **W13-T2 → W13-T4**.
   - `crates/qh-ffi/src/host/lease.rs`: **W13-T2 → W13-T4**.
   - `crates/qh-ffi/src/host.rs`: W11-T1 (sink) → W13-T4 → **W13-T4b** → W13-T8b.
   - `crates/qh-ffi/src/execution_log.rs` dan `crates/qh-storage/**`: W11-T1 (bila menyentuh) → **W13-T4b**.
   - `crates/qh-ffi/tests/real_server.rs` dan `crates/qh-driver-trino/tests/integration.rs`: **W13-T2 → W13-T4**.
   - `crates/qh-ffi/tests/golden.rs` (literal usage): W12-T3 → **W13-T4**.
   - `Models/Session.swift`: W12-T2 → **W13-T1**. `Views/ResultGridTable.swift`: … → W10-T4 → **W13-T1**.
   - `Models/QueryTab.swift`: … → W13-T1 → **W13-T3 → W13-T6**. `Models/AppModel+Run.swift`: … → W13-T1 → **W13-T3**.
   - `Support/Snapshot.swift` dan `VisualParityTests.swift`: … → **W13-T1 → W13-T3 → W13-T5 → W13-T6 → W13-T8c** (scene V-11; baseline direkam hanya di langkah merge, §0.5).
   - `App.swift`: … → W12-T2 → **W13-T5** → W13-T8b (bila OQ-2 jendela). Hari ini `Window("QueryHive", id: "main")` (`App.swift:41-52`); `WindowGroup("Server Activity", for: UUID.self)` ditambahkan di sebelahnya dan tidak bergantung pada bentuk shell W9-T8 (W9 D-17).
   - `Views/Panels.swift`: … → W12-T4 → **W13-T6** (tab Audit; W9-T5 hanya menyentuh `PanelTabButton`).
   - `Models/Shortcuts.swift` dan `App.swift` untuk pintasan Ke baris: bila UX memintanya, melalui peta pintasan W9-A1.
4. **Baris T2:** `classify.rs` hanya tes (§4.5); `crates/qh-driver/src/lib.rs`, `lease.rs`, tes live ditambahkan. **Baris T3:** fixture sebagai string mentah di `PlanFixtures.swift`, bukan resource tes. **Baris T7:** `Package.swift` tidak berubah.
5. **ADR:** jadwal W13-D hanya memuat 0043 sampai 0045. Tambahkan **0046** (lokalisasi) dan adendum 0038 untuk setelan EXPLAIN (§14).
6. **Tafsir T2:** `docs/architecture/development-plan.md` §5 W13-T2 menyebut "dengan guard statement di dalamnya"; D-10 menafsirkannya sebagai guard statement di dalamnya **plus** syarat baca (OQ-1).

## 13. Risiko

| ID | Risiko | Penanganan |
|---|---|---|
| R-1 | Menjalankan ulang bisa mengembalikan baris dalam urutan lain atau data yang berubah | Bantuan tombol menyebutnya; tidak ada klaim "lanjutan" di footer (D-1). Bukan bug. |
| R-2 | ANALYZE mengeksekusi statement (biaya, efek samping fungsi) | Hanya baca (D-10), konfirmasi di app, `STATEMENT_TIMEOUT_MS` (60 detik bawaan) sebagai pagar, Stop sampai server. Fungsi bersifat tulis di dalam SELECT sama berisikonya dengan Run biasa. |
| R-3 | Parser rencana salah baca (paralel, `InitPlan`, `Limit`, `"NaN"`) | Aturan dari pengukuran (§1.3, §5.1), fixture nyata, Raw sebagai cadangan, label "estimated". |
| R-4 | Trino lain versi: `FORMAT JSON` mungkin tidak ada | Hanya 483 yang diuji. EXPLAIN tanpa ANALYZE jatuh ke teks sekali; ANALYZE tidak pernah diulang. |
| R-5 | Polling aktivitas membebani produksi | 500 baris, 5 detik timeout, 3 detik interval, satu pembacaan sekaligus, berhenti saat tak terlihat, mundur saat galat. Beban nyata di server besar tidak diukur. |
| R-6 | Mengirim `X-Trino-Source` untuk menandai app | **Tidak dilakukan**: pemilih resource group Trino bisa memakai `source`, sehingga mengubah header mengubah tempat query QueryHive masuk di klaster bersama. Penandaan "this app" hanya di PostgreSQL (`application_name` sudah ada). |
| R-7 | Cancel adalah balapan: bisa sudah selesai, atau ditolak hak | `Requested` bukan "berhenti"; `NoSuchSession` bukan galat; galat hak diteruskan dari server; daftar disegarkan setelah cancel. |
| R-8 | SQL pengguna lain terlihat di layar | Dibatasi hak server; tidak disimpan, tidak dicatat, tidak masuk history; Salin hanya atas tindakan pengguna; dipotong 2.000 karakter. |
| R-9 | `session` dan `sessions` mudah tertukar | Tes nama, dan OQ-4 untuk nama lain. |
| R-10 | Penampil audit kosong atau menjanjikan lebih dari yang dibuktikan rantai; T4b mengubah format rantai | §8.1 (banner `writer == false`); kalimat bantuan jujur "tamper-evident, bukan tamper-proof" (§8.4); T4b menjaga rantai versi 1 tetap terverifikasi dan memakai SEC sebagai pemeriksa. |
| R-11 | Lokalisasi: alat butuh Xcode; kunci dengan dua argumen; literal SwiftUI yang menyelinap | Pesan build jelas; langkah pertama T7 mencoba kunci dua argumen lewat kompilasi nyata; aturan 2 dan tes katalog. |
| R-12 | Pergeseran kode: blueprint ditulis di `8103478`, sebelum W9 sampai W12 | Rujukan Swift menyebut nama fungsi; implementer membaca ulang W9-A1, W11-A1, W12-A1, dan memeriksa nomor baris sebelum mulai. |

## 14. Untuk ADR (W13-D)

- **0043, aktivitas server dan guard cancel:** D-12, D-13, §6.3 sampai §6.5; mengapa cancel saja, mengapa lajur metadata (ongkos koneksi per poll, bukan antrean; pool adalah reservasi, `pool.rs:3-7`, §6.1), mengapa guard setara DML dengan mode terselesaikan (kemonotonan terbukti, bukan diasumsikan), mengapa id dicek sebelum guard, mengapa `X-Trino-Source` tidak dikirim (R-6), dan prasyarat sink (§8.1); format rantai versi 2 (§8.2) bila T4b disetujui, atau ADR-0026 diamandemen. Ditolak: terminate; mengklasifikasikan teks `KILL` (tak terklasifikasi, P-12); lajur query (bukan karena poll akan menunggu, pool tidak membuat checkout menunggu, melainkan karena selama kedua sesi query dipegang tiap poll membuka dan menutup koneksi baru, §6.1); rute `Fresh` (gagal saat server penuh koneksi).
- **0044, paginasi dan cap tunggal:** D-1 sampai D-4; alasan menaikkan `rowLimit` dan bukan `OFFSET` (tidak stabil tanpa `ORDER BY`, tidak cocok dengan statement bebas pengguna), kursor yang diparkir (P-07), cap kedua terpisah (melanggar "satu-satunya cap"), cap sementara dan `Session`, dan syarat baca. Menyebut ketergantungan pada plafon (200.000 atau 5.000.000, P-1).
- **Adendum 0038 (setelan EXPLAIN):** D-6 sampai D-10, termasuk bahwa Trino 483 tidak mendukung `EXPLAIN ANALYZE (FORMAT JSON)` (bukti §1.1), ANALYZE hanya untuk baca, dan `classify.rs` tidak diubah.
- **0046 (usulan), lokalisasi:** D-16, aturan §9.2, dan mengapa bukan resource SwiftPM.
- Adendum 0007, 0010, 0014 dan ADR 0045: milik T8, lihat `fase-6` §23.

## 15. Yang tidak bisa diverifikasi

- **Trino:** efek tulis `EXPLAIN ANALYZE INSERT` (tidak dicoba, agar server dev tidak diubah), `kill_query` pada query yang hidup (hanya id yang tidak ada diuji), `ACCESS_DENIED` untuk query pengguna lain, isi `stages` di `GET /v1/query/<id>`, makna `cpuCost` per node (kesimpulan dari dua rencana), perilaku `EXPLAIN (FORMAT JSON)` di versi Trino selain 483, dan sebab satu query `FINISHING` 146 detik di server dev.
- **PostgreSQL:** hanya peran superuser yang tersedia, jadi `<insufficient privilege>` dan galat `pg_cancel_backend` untuk peran biasa tidak diuji; `pg_blocking_pids` tidak diuji dengan kunci nyata (daftarnya kosong atau tanpa pemblokir); versi di bawah 10 tidak diuji.
- **MySQL:** kecocokan `sys.innodb_lock_waits.waiting_pid` dengan `PROCESSLIST.ID` dan hasil pembacaan itu dengan kunci nyata tidak diuji (kosong untuk root, ditolak untuk `qh`); makna galat 1094 dan 1095 dari pengetahuan dokumentasi, bukan dari uji; hanya `KILL QUERY` terhadap sesi uji milik sendiri yang dicoba.
- **Jalur driver:** bahwa `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)` melewati `prepare` PostgreSQL dan penormal `json` (`normalize.rs:119`) tanpa masalah dibaca dari kode, tidak dijalankan lewat driver (tidak boleh membangun atau menjalankan cargo di sesi ini). Tes live T2 yang membuktikannya.
- **Swift dan alat:** perilaku `Bundle.module` pada app terakit tidak dicoba (tidak ada perintah swift); karena itu rancangan menghindarinya. Hasil `xcstringstool compile` (tata letak `.lproj`, `.stringsdict` untuk jamak, kunci dua argumen) hanya dibaca dari `--help`; `xcstringstool extract` tidak dicoba. Pengaruh `CFBundleLocalizations` tidak dicoba.
- **Kode yang belum ada:** `AppModel+Run.swift`, `SchemaOutline.swift`, `Panels` setelah W12, kontrak pembaca `execution_log` W11-T1, dan peta pintasan W9-A1 belum ada di pohon; rujukannya ke bentuk rencana, bukan kode.
- **Beban:** dampak polling aktivitas pada server besar dan waktu parse rencana 5.000 node (target < 50 ms, belum diukur) dicatat sebagai sasaran, bukan hasil.
- **TablePro:** dibaca untuk ide saja. Tidak ada angka dari TablePro yang dipakai sebagai target, dan tidak ada kodenya yang disalin.

## Verdict architect-reviewer

**Verdict: perlu perbaikan, dua temuan blocking (B-1, B-2). Keduanya sudah diterapkan di dokumen ini oleh penulis blueprint (W13-A1) dan menunggu tinjauan ulang satu putaran (O-20); sampai itu terjadi bagian yang disentuh berstatus "pending review".** Temuan non-blocking di bawah dicatat untuk backlog dan **belum** diterapkan: teks badan dokumen untuk butir-butir itu tidak berubah, dan implementer tugas pemiliknya membaca daftar ini sebelum mulai (R-12).

### Blocking (diterapkan, menunggu tinjauan ulang)

- **B-1, integritas audit (D-10, §4.4 terhadap §4.5 dan §4.6).** Urutan semula menaruh `guard_for` (langkah 1) sebelum `analyze_requires_read` (langkah 3). `guard_for` -> `guard_confirmed` -> `record_decision` (`crates/qh-ffi/src/commands.rs:227-290`, `:297-299`) selalu menulis satu baris. Sebuah `INSERT` dengan `EXPLAIN_ANALYZE=1` di koneksi `full` akan tercatat `allowed` (atau `confirmed` di `confirm` dengan `SAFE_MODE_CONFIRMED=1`) lalu `refused` oleh pemeriksaan ANALYZE. `LogDecision::Allowed` berarti "Ran without asking" (`crates/qh-ffi/src/execution_log.rs:42-50`), jadi log yang tamper-evident akan menyatakan sebuah tulis berjalan padahal tidak pernah. Ini juga bertentangan dengan tes `explain_analyze_of_a_write_is_refused_before_connecting` (§4.6), yang mengharapkan satu baris `refused`. **Koreksi:** di `explain`, tepat setelah `safe_mode(settings, engine)?`, jalankan `explain_options(settings)?`, `analyze_requires_read`, dan `explain_support().resolve()`, ketiganya murni dan tanpa I/O; `guard_for` baru dipanggil sesudahnya. ANALYZE yang ditolak meninggalkan tepat satu baris, dan `EXPLAIN_FORMAT` tak dikenal atau ANALYZE di MySQL tidak meninggalkan baris. **Diterapkan:** D-10, §4.4 (langkah 1 sampai 7 ditulis ulang, ditambah tabel akibat pada log), dua butir di §4.5, dan tiga baris tes di §4.6 ("tepat satu baris log"; "tanpa baris log" untuk MySQL dan format tak dikenal).
- **B-2, klaim keliru tentang kode (D-12, ADR 0043).** Ringkasan butir 6, D-12, §6.1, dan §14 ("Ditolak: lajur query") menyatakan aktivitas harus memakai lajur metadata karena kalau tidak ia akan menunggu selama kedua sesi query dipegang statement liar. Pool tidak bekerja begitu: `crates/qh-ffi/src/host/pool.rs:3-7` berkata "It is a reservation, not a queue: a checkout never waits for another Run to finish ... When the lane is full a new session is opened outside the reservation" (checkout `:486-581`; PRD O-7 sama). **Keputusan tetap, alasan dikoreksi:** lajur metadata memakai ulang satu sesi hangat untuk poll 3 detik dan tidak bersaing dengan dua reservasi Run; di lajur query, selama kedua sesi query dipegang, tiap poll membuka dan menutup koneksi baru, yang berarti ongkos connect, TLS, dan SSH tiap tick dan koneksi tambahan ke server produksi yang sudah terbebani. **Diterapkan:** Ringkasan butir 6, D-12, §6.1, dan §14, sebelum ADR 0043 mewarisi alasan yang salah.

### Non-blocking (backlog, belum diterapkan)

1. **D-2 / §3.2, gerbang baca-saja untuk menjalankan ulang hanya bertumpu pada cermin di app.** `sqlStatements` memecah dengan `dialect: .generic` (`app/Sources/QueryHive/Models/QueryTab.swift:1124-1137`), dan `StatementScan` menyebut dirinya "not an authority" (`RunConfirmation.swift:73-79`). Ia juga tidak punya pemeriksaan locking clause: `SELECT ... FOR UPDATE` terbaca `readOnly` di Swift tetapi `Dml` di engine (`classify.rs:509`), yang bertentangan dengan invarian `AGENTS.md` "classify SQL with the connection's dialect". Kasus MySQL multi-statement sudah ditutup `is_single_statement` (`crates/qh-driver-mysql/src/lib.rs:555-571`), tetapi pembacaan PostgreSQL `E'\''` masih bisa menyembunyikan CTE tulis dari leksikal generik. **Saran:** T1 mengirim setelan seperti `REQUIRE_READ=1` pada `fetchRows` ulang, dan `preview` di engine menegakkannya dengan helper yang sama dengan `analyze_requires_read` (berkas sama, pemilik T2 yang sama). Pemeriksaan Swift tetap hanya untuk mematikan tombol.
2. **§3.4 D-3, `didSet` pada `QueryTab.rowLimit` tidak bisa membedakan tulis oleh `fetchRows` dari tulis oleh pengguna.** `QueryTab` adalah kelas `@Observable` (`QueryTab.swift:369-370`). Pada langkah 3 seperti tertulis (set `paginationRaisedFrom`, lalu `rowLimit = cap`), `didSet` menghapus nilai yang baru disimpan. Tulis urutannya (tangkap nilai lama, set `rowLimit`, baru set `paginationRaisedFrom`) atau pakai satu helper mutasi. Pemulihan dari titik pulih punya masalah urutan yang sama.
3. **§4.6, tidak ada driver palsu di `crates/qh-ffi/tests/safe_mode.rs`.** `CountingEngine::driver` dan `RecordingEngine::driver` mendelegasikan ke `RealEngine` (`:50`, `:674`), jadi `explain_support` datang dari driver asli. Itu cukup, tetapi kalimat yang berkata driver palsu meng-override `explain_support` salah. Hanya `RecordingSession` (`:586`) yang perlu meng-override `explain_statement_with`.
4. **§4.2, `ExplainSupport::resolve` tidak tahu identitas driver.** Ia mengembalikan `Option<&'static str>`, padahal §4.3 butuh kalimat penurunan yang berbeda untuk Trino dan MySQL. Taruh kalimatnya di `ExplainSupport`, atau selesaikan pada `Driver`.
5. **§6.2 / §6.5, `Session::activity(&mut self)` tidak punya parameter timeout**, sementara teksnya berkata `STATEMENT_TIMEOUT_MS` (5.000 ms dari app) berlaku. Teruskan `Option<Duration>` seperti `ExecuteOptions`, atau tulis bahwa `activity.rs` membungkus panggilan dengan `tokio::time::timeout` dan apa artinya di sisi server.
6. **§5.2, Raw di Trino menjadi satu sel JSON.** App selalu mengirim `EXPLAIN_FORMAT=json` untuk Trino, jadi tampilan Raw berupa satu sel JSON, bukan teks rencana yang terbaca seperti hari ini, dan itu bertentangan dengan PR-14 ("raw plan still shown"). Pertimbangkan Raw di Trino menampilkan bentuk teks (jalankan ulang sekali, aman tanpa ANALYZE) atau JSON yang dirapikan.
7. **§8.2 T4b.** Pertimbangkan menaruh `chain_version` di payload hash v2 secara eksplisit, bukan hanya mengandalkan panjang array. Nyatakan juga bahwa CLI dan MCP (`connection_settings`) mengirim `CONNECTION_ID`, jadi baris non-app tidak selalu `NULL`. Pembawa task-local cocok dengan sink global proses yang ada, tetapi tes harus mencakup setiap pemanggil guard (`apply.rs`, `import.rs`, `mcp.rs`), seperti tertulis di teks.
8. **§11 langkah 5, daftar ADR W13-D.** Baris itu memuat "0043, 0044, 0046, adendum 0038, 0007, 0010, 0014" dan melewatkan 0045 (DataFusion, `development-plan.md:200` dan `:737`). §14 menaruh 0045 di bawah T8, tetapi daftar gate harus cocok dengan rencana.
9. **Pergeseran nomor baris terhadap `8103478`.** Dialog `confirmReplace` ada di `ResultGrid.swift:793` (dikutip `:787`), label `LIMIT` di `:763` (dikutip `:756`), dan `scrollToVisible(row:)` di `ResultGridTable.swift:842` (dikutip `:865`; `:883` adalah pohon kerja yang kotor). Nama-namanya benar, jadi tidak blocking, sejalan dengan R-12.
10. **§12.2, penambahan kepemilikan berkas konsisten dengan §7 dan benar ditandai sebagai perubahan orkestrator:** `host.rs`, `host/lease.rs`, `qh-driver/src/lib.rs`, `golden.rs` setelah W12-T3, `Session.swift`, `ResultGridTable.swift` setelah W10-T4, `Snapshot.swift` dan `VisualParityTests.swift`, `App.swift` (W13-T5 bila OQ-2), dan `qh-storage/**`. `crates/qh-ffi/src/lib.rs` sudah mengikuti lajur FFI, jadi dimiliki T4.

### Terverifikasi benar terhadap kode

- Guard `explain` berada sebelum awalan driver (`commands.rs:1590` dan `:1601`); penerusan lease dengan cadangan `EXPLAIN {sql}` (`lease.rs:494-505`); sembilan `Session` palsu dengan `explain_statement`.
- Rute tanpa wildcard (`host.rs:224-249`); `QUERY_SESSIONS = 2` dan `METADATA_SESSIONS = 1` (`pool.rs:34-36`); `COMMANDS` berisi 26 entri dengan aturan "hanya sebelum `objects`" (`lib.rs:450-490`).
- Fakta classifier (`classify.rs:494-516`, `DDL_WORDS :579-582`, tes `:1214`, `:1756`, `:1838`, `:1847`); matriks keputusan `Dml` monoton pada `ReadOnly > Confirm > NoDdl > Full` (`classify.rs:201-221`), jadi klaim D-13 berlaku.
- Sink hanya dipasang di `main.rs:66` dan `bin/mcp.rs:91`; `0007` tidak punya kolom koneksi dan hash adalah array tetap 9 field (`qh-storage/src/execution_log.rs:93-131`); `explain` MCP hanya memakai SQL (`mcp.rs:954-959`).
- `xcode-select` menunjuk ke Xcode.app dan `xcstringstool` ada; `Connection.id` adalah UUID, jadi `WindowGroup(for: UUID.self)` cocok.
- D-10 / OQ-1 lebih ketat daripada PRD FR-PLAN-01 / UC-10, dan dengan benar diajukan sebagai pertanyaan pemilik, bukan penyimpangan diam-diam.
