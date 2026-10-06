# Blueprint W11: metadata, kepercayaan koneksi (SSH, JWT, CA), pemetaan MCP, dan alias SQL

- **Status:** blueprint tingkat berkas, 6 Okt 2026 (W11-A1). Belum ada kode yang ditulis. **§5 (protokol TOFU) harus ditinjau `security-reviewer` sebelum W11-T2 dimulai** (`development-plan.md` §5, W11-A1). `architect-reviewer` memeriksa blueprint ini, lalu implementasinya sesudah W11-T6. Bagian "Verdict architect-reviewer" di akhir berisi verdict atas blueprint ini (6 Okt 2026): satu temuan memblokir (JWT dan origin kredensial, §7.1) yang koreksinya sudah diterapkan dan menunggu pemeriksaan ulang (O-20), dan temuan non-memblokir yang juga sudah diterapkan.
- **Untuk:** W11-T1 … W11-T6 (`development-plan.md` §5). Cakupan: FR-TREE-01…03, FR-CON-01…05 dan 07…09, FR-SAFE-02 (sisi engine), FR-MCP-01, FR-ED-05; NFR-S1, S2, S4, S6; keputusan P-06, P-08, P-09; ADR 0038, 0039, 0040 (ditulis di W11-D).
- **Sumber:** PRD §4 UC-01, UC-02, UC-03, UC-15, UC-18, §5.1 dan §5.2, §6.4, §11.2, §12.1; `development-plan.md` §4, §5 W11, §7; `blueprints/fase-4b-editor-analysis.md` §8.3 dan D-14; `blueprints/fase-2-engine-host.md` (pool dan jalur Metadata); `docs/invariants.md` §11; `docs/tls-modes.md`; `docs/mcp-stability.md`; `tablepro-feature-map.md` §1.1, §1.2, §1.8, §1.12; `tablepro-design-audit.md`. Kode TablePro (AGPL) tidak dibuka untuk blueprint ini. Ide yang dipakai datang dari peta fitur: dua tool MCP bernama `describe`/DDL, alias `~/.ssh/config`, dan DDL read-only. Tidak ada kode, aset, atau string yang disalin.
- **Prasyarat:**
  - W9 mendarat: `AppModel` sudah dipecah (`AppModel+{Run,Tree,Connections,Edit,Export,History,Session,Completion,Focus}.swift`, W9-T0) dan pohon sudah `Views/SchemaOutline.swift` (W9-T3). Rujukan Swift di bawah menunjuk berkas **hari ini** (`AppModel.swift`, `SidebarTree.swift`) menurut simbol, dan berpindah ke `AppModel+*.swift` dan `SchemaOutline.swift` setelah W9.
  - W10 mendarat: lane FFI sudah lewat W10-T7, dan `Support/EditorAnalysis.swift` sudah dimiliki W10-T6 dan W10-T7.
- **Bukti:** kode Rust dikutip dari HEAD `8103478` di `work/perf-parity` (berkas Rust tidak disentuh lane lain selama penulisan). **Berkas Swift yang sedang diubah lane implementasi** (`App.swift`, `AppModel.swift`, `QueryTab.swift`, `DatabaseEngine.swift`, `RustEngine.swift`; working tree kotor, dan HEAD berpindah ke `2d14eea` selama penulisan) dikutip **menurut simbol**, bukan nomor baris, karena nomornya sudah bergeser sekali. Semua SQL metadata di §3 **dijalankan** terhadap server dev dan hasilnya ada di §3.2: PostgreSQL 17.11 (di dalam transaksi yang di-`ROLLBACK`, jadi tidak ada objek tertinggal; diperiksa sesudahnya), MySQL 8.4.11 dan Trino (hanya baca, objek bawaan). Yang tidak bisa dicoba tertulis di §18. Perilaku crate pihak ketiga dibaca dari sumbernya di `~/.cargo/registry` dengan nomor versi.
- **Bahasa:** dokumen ini Indonesia. Identifier, komentar kode, pesan log, string yang tampil di app, dan nama tes Inggris.

## Ringkasan

W11 mengisi tiga kekosongan yang membuat QueryHive belum bisa dipakai di depan bastion produksi dan skema asing: engine tidak punya metadata selain nama (tidak ada kolom, DDL, atau jenis objek), app tidak punya UI untuk tunnel SSH yang sudah ada di engine, dan Trino serta PostgreSQL tidak bisa dipakai dengan JWT atau CA internal tanpa mematikan verifikasi.

Rancangan, dalam urutan pengerjaan:

1. **Metadata read-only (T1).** Tiga perintah baru (`columns`, `ddl`, `execution_log`) dan satu setelan (`OBJECT_KINDS`) yang menambah larik `kinds` ke event `tables`. SQL-nya ditulis sebagai **fungsi murni di crate driver** dan dijalankan engine lewat `Session::execute`, bukan lewat metode `Session` baru (D-1).
2. **Kepercayaan koneksi (T2).** Protokol TOFU yang dipatok ke fingerprint dengan `known_hosts` milik app (§5), alias `~/.ssh/config` (§6), JWT Trino dan berkas CA per koneksi (§7). Seluruh keputusan kepercayaan diputuskan di engine. App hanya menampilkan dan meneruskan jawaban pengguna.
3. **App dan MCP (T3, T5).** Bagian SSH di form, tiga item Keychain baru, `HostKeySheet`, validasi bernama, ⌘↩, pemetaan Navicat, dan pemetaan `ConnectionRecord` ke `Settings` di MCP, plus tool `describe_table` dan `table_ddl`.
4. **Pohon dan editor (T4, T6).** Label jenis objek, kolom sebagai anak node, tab DDL read-only, lalu autocomplete kolom dengan resolusi alias dari API `EditorDocument::references`.

Temuan paling menentukan, semuanya dari membaca kode, bukan dari rencana:

- **Menjalankan metadata lewat `Session::execute` adalah satu-satunya cara agar NFR-S1 bisa diuji dengan harness yang ada.** `RecordingSession` di `crates/qh-ffi/tests/safe_mode.rs:612-661` hanya mencatat `execute`. SQL yang dikeluarkan dari dalam metode `Session` buatan driver tidak akan pernah terlihat tes itu. Ini juga membuat `Held` (`host/lease.rs:425`), pembungkus sesi pool, tidak perlu tahu perintah baru (§1.1, D-1).
- **TLS yang memverifikasi lewat tunnel SSH hari ini memeriksa nama `127.0.0.1`.** `retarget` menimpa `config.host` (`crates/qh-ffi/src/tunnel.rs:158-163`), dan PostgreSQL memberi `tokio-postgres` host itu sebagai nama TLS (`crates/qh-driver-postgres/src/lib.rs:202`; `tokio-postgres 0.7.18 connect.rs:62-76,160`). CA per koneksi tanpa memperbaiki ini hanya akan gagal dengan "name mismatch" di jalur bastion, yaitu UC-01 dan UC-18 sekaligus (D-16).
- **Driver Trino memasang kredensial pada setiap URL yang diberikan server, dan tidak ada yang memeriksanya (temuan memblokir review).** Halaman berikutnya (`fetch_page`, `crates/qh-driver-trino/src/lib.rs:1097-1110`) dan cancel (`:640-645`) memakai `nextUri` dari badan respons lewat `with_shared_headers` (`:581-592`). `nextUri` berskema `http`, atau ke host atau port lain, atau redirect `https` ke `http` pada host dan port yang sama (reqwest 0.12.28 hanya membuang `Authorization` bila host atau port berubah, `redirect.rs:239-251`) membawa kredensial keluar. Memeriksa skema basis sesi saja tidak memenuhi FR-CON-07. §7.1 mewajibkan pemeriksaan origin per permintaan dan klien bearer tanpa redirect (D-14).
- **`EngineHost` di app tidak pernah memasang sink execution log.** Hanya `main.rs:66` (CLI) dan `bin/mcp.rs:91` yang memanggil `install_from_settings`. Sejak engine jalan di dalam proses app (W3-T1), keputusan Safe Mode yang dibuat lewat app tidak tercatat. Pembaca log (FR-SAFE-02) tanpa perbaikan ini menampilkan log yang kosong (D-8).
- **Tabel `execution_log` tidak punya kolom koneksi** (`migrations/0007_execution_log.sql:23-35`), jadi "filter per koneksi" di FR-SAFE-02 tidak bisa dipenuhi tanpa migrasi dan versi rantai baru. Perlu keputusan pemilik (§3.5).
- **`russh` punya jalur yang melewati `check_server_key`**: KEX `none` menyelesaikan pertukaran kunci dengan `server_host_key: None` dan handler tidak dipanggil (`russh 0.63.3 client/kex.rs:158-176`, `client/mod.rs:1893-1902`). Daftar bawaan tidak memuat `none` (`negotiation.rs:162-176`), tetapi penjagaan di sisi kita tidak ada. T2 menambahnya (D-11).
- **Rencana menyebut berkas yang tidak ada.** `crates/qh-sql/src/editor/` tidak ada (D-4 blueprint 4B memilih crate `qh-editor`). Tidak ada kode Rust yang harus "digantikan" oleh `refs.rs`. Yang ada hanya `AppModel.suggestions(for:prefix:path:)` di Swift.
- **Store koneksi yang dibaca MCP hanya diisi sekali oleh CLI.** App tidak pernah memanggil `import_connections` (hanya peta kata di `RustEngine.commands`), dan impor punya penanda sekali jalan. Pemetaan MCP (FR-CON-05) karena itu butuh perubahan di `crates/qh-storage/src/import.rs:421-446`, yang tidak ada di daftar berkas T3 (§10.1).

## 1. Temuan di kode yang mengubah rencana

### 1.1 Metadata hari ini

- `Session` hanya punya `browse` (nama saja, `crates/qh-driver/src/lib.rs:490-495`) dan `objects` (grid teks, `:498`). Tidak ada kolom, DDL, atau jenis objek. Saya cek ulang dengan grep di `crates/*/src`: tidak ada `SHOW CREATE`, `information_schema.columns`, `SHOW COLUMNS`, atau `pg_attribute`. Satu-satunya `pg_get_*` adalah `pg_get_userbyid` di grid objek.
- **Pohon PostgreSQL tidak menampilkan view maupun materialized view.** `tables_sql` memakai `information_schema.tables ... table_type = 'BASE TABLE'` (`crates/qh-driver-postgres/src/lib.rs:899-905`), dan `information_schema.tables` tidak memuat materialized view sama sekali (dibuktikan di §3.2). Grid objek memakai `pg_class` dengan `relkind IN ('r','p')` (`:909-918`).
- **Pohon MySQL dan Trino sudah memuat view, tanpa label.** `SHOW TABLES FROM` (`crates/qh-driver-mysql/src/lib.rs:1435-1440`, `crates/qh-driver-trino/src/lib.rs:1355-1366`) mengembalikan tabel dan view. Jadi untuk keduanya FR-TREE-01 hanya menambah label, bukan objek baru.
- Perintah browse memancarkan `{"event":"tables","names":[...]}` (`crates/qh-ffi/src/commands.rs:750-769`, `:803-819`). Golden `tests/golden/tables/*` membekukan bentuk itu.
- Penamaan target tulis sudah punya kosakata: `TARGET_CATALOG`, `TARGET_SCHEMA`, `TARGET_TABLE`, dengan aturan slot per driver (`crates/qh-ffi/src/sql_ident.rs:36-74`: Trino tiga, PostgreSQL schema dan table, MySQL catalog sebagai database dan table) dan kutip per dialek (`:89-100`). Perintah metadata baru memakainya ulang. Tidak ada setelan nama tabel baru.
- Event bernama `columns` **sudah ada**: `preview` dan `export` memancarkannya dengan header bertipe `[{"name","type"}]` (`commands.rs:618-626`, dan `Event.columns` di struct `Event` pada `App.swift`). Tool MCP `columns` juga sudah ada dan bekerja dengan `SELECT * FROM <t> LIMIT 0` (`mcp.rs:261-267`, `:913-936`). Perintah baru `columns` memakai nama event lain (D-4), dan tool lama tetap (`docs/mcp-stability.md`: tool tidak boleh dihapus atau diganti nama).
- `Held` meneruskan `execute`, `execute_bound`, `browse`, dan `objects` secara eksplisit lewat `Op` dan `Reply` (`crates/qh-ffi/src/host/lease.rs:207-218`, `:425-486`). Metode `Session` baru dengan implementasi bawaan akan **diam-diam** menjawab "tidak didukung" untuk semua perintah yang lewat pool, karena `Held` tidak meneruskannya. Itu kegagalan yang tidak tertangkap tes mana pun yang memakai sesi palsu. Menjalankan lewat `execute` menghindarinya sama sekali.
- `rerunnable` (`lease.rs:48-59`) mengklasifikasi teks dengan `qh_sql::classify_readings` dan hanya mengulang statement `ReadOnly`. `SHOW` dan `SELECT` lolos (`crates/qh-sql/src/classify.rs:498-502`), jadi metadata ikut diulang bila koneksi pool putus.
- Hal yang harus diperbarui bersama setiap perintah baru (invariant 11, `docs/invariants.md:148-174`): `COMMANDS` dan `Command` di `crates/qh-ffi/src/lib.rs:424-489`, `EngineCommand` dan `EVERY_COMMAND` di `crates/qh-ffi/src/uniffi_api.rs:95-169`, dan `RustEngine.commands` di `app/Sources/QueryHive/Support/RustEngine.swift:84`. **Tambahan yang tidak tertulis di rencana:** `route()` di `crates/qh-ffi/src/host.rs:224-249` tanpa wildcard (perintah baru gagal dikompilasi sampai memilih rute), dan urutan `COMMANDS` yang kontraknya "perintah baru hanya boleh ditambah **sebelum** `objects`" (`lib.rs:453-461`). `Command::name()` memakai `COMMANDS[self as usize]` (`:526-528`), jadi urutan varian `Command` harus sama.

### 1.2 Event dan dekoder

- `struct Event` di `App.swift` adalah satu struct datar. Aturannya tertulis di komentarnya: satu kunci tidak boleh punya dua tipe. Kunci yang sudah terpakai: `step message rows columns files warnings queryId cancelled ok catalogCount names objectColumns data truncated elapsedMs host user table mode state entries queries cleared applied rejected errors errorsTruncated stoppedAt transaction disposition streams format merged action query renamed saved tabs activeTabId account profiles profile deleted`.
- Karena itu event baru memakai kunci yang belum ada (D-4): `kinds`, `object`, `fields`, `object_kind`, `ddl`, `redacted`, `decisions`, `chain`, `host_key` (properti Swift `objectKind` dan `hostKey`). `entries` **tidak** boleh dipakai ulang untuk log eksekusi (sudah `[HistoryEntry]`), dan `state` juga tidak (string di `to_table`).
- Event `error` dibentuk di dua tempat yang harus diubah serempak: `main.rs:94-108` (`report`) dan `uniffi_api.rs:371-390` (`fail`). Keduanya hanya membawa `message` dan `warnings`.
- `EngineError::Connect` hanya membawa `message` dan `kind` (`crates/qh-core/src/error.rs:58-59`). Tidak ada tempat untuk fingerprint terstruktur. Jalur dari `qh_tunnel::Error` ke event hari ini: `tunnel::open` → `EngineError::Connect{message}` → `CliError::Connect(String)` (`crates/qh-ffi/src/tunnel.rs:207-221`, `lib.rs:251-268`). Pesan `describe()` memuat fingerprint sebagai teks bebas dan menyuruh pengguna memakai `ssh-keyscan` (`tunnel.rs:230-259`), yang bukan verifikasi di luar jalur.

### 1.3 SSH hari ini

Verifikasi host key sudah ada dan baik. Yang belum ada adalah semua hal di sekitarnya.

- `qh-tunnel` memeriksa host key **selama key exchange, sebelum autentikasi** (`crates/qh-tunnel/src/tunnel.rs:301-361`; `russh 0.63.3 client/mod.rs:1893-1904` memanggil `check_server_key` sebelum `common.encrypted(...)`). `check` membaca satu berkas, mencocokkan byte kunci (bukan teks atau fingerprint), mengenali entri berhash, `@revoked`, `@cert-authority` (`known_hosts.rs:149-211`). Baris yang tidak bisa diparse adalah galat, bukan dilewati (`:291-338`).
- TOFU hari ini dua langkah: `open` dengan `HostKeyPolicy::Strict` menolak host asing dengan `HostKeyUnknown`, lalu pemanggil `append` dan `open` lagi, atau memberi `TrustNew(ServerKey)` (`tunnel.rs:63-70`, `:338-359`). Engine **tidak punya jalur terima**: ia selalu `Strict` (`crates/qh-ffi/src/tunnel.rs:190-196`), dan komentarnya menyebut protokol NDJSON satu arah (`:33-46`).
- `BastionConfig` menerima **satu** berkas `known_hosts` (`tunnel.rs:74-84`), dan setelan `SSH_KNOWN_HOSTS` mengganti default `~/.ssh/known_hosts` (`crates/qh-ffi/src/tunnel.rs:124-151`). Tidak ada berkas milik app. `RecordedKey` membawa nomor baris tetapi bukan nama berkasnya (`known_hosts.rs:57-97`), jadi dua berkas membuat "baris 3" ambigu.
- **Ketidakcocokan jenis kunci dibaca sebagai kunci berubah.** `check_text` mengembalikan `Mismatch` bila ada entri untuk host itu dan tidak ada yang cocok (`known_hosts.rs:203-205`), tanpa melihat jenis kunci. `russh` memilih algoritma host key dari daftar bawaan (`negotiation.rs:210-231`: Ed25519, ECDSA, RSA berurutan), tidak dari isi `known_hosts`. Server yang menawarkan Ed25519 padahal `known_hosts` hanya memuat RSA-nya menghasilkan alarm palsu. OpenSSH menghindarinya dengan mengurutkan algoritma menurut kunci yang tercatat. Ini diperbaiki di D-11, karena tanpanya penolakan keras berubah menjadi kebiasaan mengedit berkas.
- Alias `~/.ssh/config` belum ada, dan `SSH_*` tidak punya setelan CA, JWT, atau berkas app. Impor Navicat hanya membaca `SSH_Host` dan membuangnya (`app/Sources/QueryHive/Support/NavicatImport.swift:23-26,141`; `AppModel` hanya mencatat nama koneksi yang "tunnelled" di kedua cabang impor).
- `ConnectionConfig` dan `TunnelConfig` ditulis agar menambah kolom **memaksa** `PoolKey::of` diubah: ia membongkar keduanya tanpa `..` (`crates/qh-ffi/src/host/pool.rs:134-212`), dan komentarnya menyebut "a per-connection CA, a Trino JWT" secara eksplisit. W11-T2 memegang `host/pool.rs` karena itu, dan berkas ini belum ada di daftar T2.

### 1.4 TLS hari ini

- `TlsMode` empat nilai (`crates/qh-driver/src/lib.rs:405-419`); `docs/tls-modes.md` adalah penjelasannya. Hanya `Require` memverifikasi. `Prefer` dan `RequireNoVerify` tidak.
- PostgreSQL: `connect` memanggil `connect_with_verifier(config, tls::platform_verifier()?)` (`crates/qh-driver-postgres/src/lib.rs:172-180`). Seam `tls::verifier_with_roots(RootCertStore)` sudah ada dan sudah dipakai tes (`tls.rs:70-83`, `tests/tls.rs`).
- Trino: `client_for(tls, roots: Option<RootCertStore>)` sudah menerima root buatan pemanggil, tetapi `connect` memanggilnya dengan `None` (`crates/qh-driver-trino/src/lib.rs:256-298`, `:450`).
- MySQL: `Require` memverifikasi terhadap `webpki-roots` bawaan `mysql_async`, bukan trust store sistem, dan CA tidak bisa dipasang karena `PathOrBuf` tidak diekspor (`crates/qh-driver-mysql/src/tls.rs:34-71`; PRD §12.1; P-08). UI sudah menyembunyikan `verify-ca` dan `verify-full` untuk MySQL (`Connections.swift:94-119`), tetapi engine memetakan keduanya ke `Require` (`config.rs:296`), jadi CLI masih bisa mencapainya.
- **Trino mengirim `Authorization: Basic` bahkan saat `Prefer` turun ke `http`.** `send_post` dipanggil lagi dengan klien biasa dan `with_shared_headers` yang sama (`crates/qh-driver-trino/src/lib.rs:721-748`, `:581-592`). Server Trino menolak Basic lewat HTTP (komentar `config.rs:44-50`), tetapi header itu sudah di kabel. Untuk JWT ini tidak boleh terjadi (D-14). Untuk password, kasus `Prefer` ini tetap temuan terpisah yang dicatat di §16 dan tidak diperbaiki di W11. Kasus di butir berikutnya lebih luas: ia berlaku untuk setiap sesi `https`.
- **Trino memasang kredensial pada URL yang diberikan server, tanpa memeriksa skema atau origin-nya (koreksi review, memblokir).** Tiga macam permintaan membawa kredensial: POST ke `{base}/v1/statement` (`crates/qh-driver-trino/src/lib.rs:694-703`, dari basis sesi), GET ke `self.next_uri` untuk setiap halaman (`fetch_page`, `:1097-1110`), dan DELETE ke `running.next_uri` untuk cancel (`delete_running`, `:640-645`). `next_uri` datang dari badan respons (`Page.next_uri`, `:182`; disimpan oleh `running_from`, `:834-838`). Ketiganya lewat `with_shared_headers` (`:581-592`), yang memasang `Authorization` ke URL apa pun yang diberikan, dan tidak ada baris di berkas itu yang membandingkan skema, host, atau port `nextUri` dengan basis sesi (grep `next_uri`: hanya disimpan, diteruskan, dan dipakai). Akibatnya, untuk Basic hari ini dan untuk Bearer bila tidak dicegah:
  1. Koordinator di belakang load balancer yang mengakhiri TLS tanpa penanganan header forwarded mengembalikan `nextUri` berskema `http://` (salah konfigurasi yang umum), dan kredensial keluar sebagai teks jelas pada polling pertama walau sesi dibuka lewat `https`.
  2. `nextUri` yang menunjuk host lain mengirim kredensial ke host itu. DELETE cancel sama.
  3. Kebijakan redirect bawaan reqwest 0.12.28 adalah `Policy::limited(10)` (`redirect.rs:161-163`). Ia membuang `Authorization` hanya bila host **atau port** berubah (`remove_sensitive_headers`, `redirect.rs:239-251`, dipanggil dari `on_request`, `:336-338`), dan skema tidak dibandingkan: redirect `https` ke `http` pada host dan port eksplisit yang sama mempertahankan header itu. Satu-satunya yang peka terhadap penurunan skema adalah `Referer` (`make_referer`, `:294`).
- `config.rs` sudah punya aturan "naikkan, jangan turunkan" (password di Trino menaikkan mode ke `Require`, `config.rs:44-58`, `:275-278`). CA dan JWT mengikuti semangat itu tetapi menolak, bukan menaikkan diam-diam (D-14, D-15).

### 1.5 Keychain

- Satu layanan, `id.data-ecosystem.queryhive`; akun adalah UUID koneksi dalam huruf besar (`crates/qh-credentials/src/lib.rs:59`, `account_key` `:147-154`). Profil memakai awalan `profile:` (`:69-77`), dan awalan itu aman karena UUID selalu dibuka angka heksadesimal.
- Swift menulis item yang sama dengan `SecItemAdd`/`Update` dan akun `id.uuidString` (`Connections.swift:605-662`). Item tidak memasang `kSecAttrAccessible`, access group, atau data protection keychain (ad-hoc signed, ADR-0014). `account_key` hanya melipat akun yang **seluruhnya** berbentuk UUID, jadi akun berawalan harus melipat bagian UUID-nya sendiri (`account_for_profile` melakukannya, `:75-77`).
- Siapa yang membaca apa: app membaca lewat Swift (`AppModel.storedPassword`, dan `getWithoutPrompt` di `warmUp`), MCP membaca lewat `KeychainStore` (`mcp.rs:1341-1355`). Keduanya memakai akun yang sama, jadi kontrak nama akun harus identik di dua bahasa (D-17).
- Titik yang menyentuh item koneksi di app dan harus mencakup slot baru: hapus koneksi, duplikat, impor Navicat (kedua cabang), baca untuk run dan tree (`AppModel.storedPassword`, `warmUp`, `loadChildren`, `connectionEnvironment`), dan Test Connection di editor (`ConnectionsViews.swift:728-734`). Semuanya di `AppModel.swift` dan dicari menurut simbol `ConnectionKeychain.`.
- `Settings` menurunkan `Debug` yang mencetak semua nilai (`crates/qh-ffi/src/env.rs:41-44`), termasuk `DB_PASSWORD`, `SSH_PASSWORD`, `SSH_KEY_PASSPHRASE`, dan nanti `DB_JWT`. Saya tidak menemukan `{:?}` atas `Settings` maupun struct berturunan `Debug` yang memuatnya (grep di `crates/qh-ffi/src`), jadi ini bahaya laten, bukan kebocoran yang terjadi. NFR-S2 meminta ia ditutup (D-9).

### 1.6 MCP

- Sembilan tool (`mcp.rs:233-296`). Izin tool = nama dalam `scopes` token. `read_only_tool_names()` (`:299-301`) adalah scope bawaan token baru, jadi tool baru otomatis masuk ke token yang diterbitkan **sesudah** W11 dan tidak ke token lama (gagal tertutup).
- `connection_environment(record, options, password)` adalah fungsi murni yang menyalin `AppModel.connectionEnvironment` (`mcp.rs:429-524`) dan sudah punya tes murni (`crates/qh-ffi/tests/mcp.rs:777-850`). Setiap panggilan MCP dipaksa `SAFE_MODE=read_only` (`:1297-1300`).
- `options_json` diisi dari `connections.json` hanya untuk `schema`, `scheme`, `sslmode`, `verify`, `showAllSchemas` (`crates/qh-storage/src/import.rs:421-446`). `safeMode` pun tidak dipetakan. Setelan SSH, CA, dan JWT tidak punya tempat di store ini.

### 1.7 Editor

- Autocomplete hari ini: `AppModel.suggestions(for:prefix:path:)` (`AppModel.swift`) mengambil anak dari `TreeNode` bertingkat, kolom hasil run terakhir (`tab.columns`), dan daftar kata kunci `SQLSuggestions.keywords`. Node yang anaknya belum dimuat dimuat di latar dan daftar baru benar pada ketikan berikutnya (komentar `offerChildren`). `node(atPath:)` memetakan jalur menurut `ConnectionKind.levels`.
- `TreeNode.Kind` tidak punya view maupun kolom, dan tabel adalah daun (`SchemaTree.swift:12-42`: `isExpandable` false untuk `.table`).
- Grammar vendor memuat simpul yang dibutuhkan alias: `relation` dengan field `alias`, `object_reference` dengan `database`, `schema`, `name`, `cte` dengan `argument` (daftar kolom), dan `term` dengan `alias`. Saya baca dari `crates/qh-sql-grammar/node-types.json`. Tidak ada simpul bernama `alias`.
- `qh-editor` hari ini tidak punya `refs` (`crates/qh-editor/src/lib.rs:22-30`) dan FFI-nya tidak punya `references` (`crates/qh-ffi/src/editor.rs`). Kontrak yang sudah disetujui ada di blueprint 4B §8.3: `EditorDocument::references(revision, offset)`, dihitung saat diminta di antrean latar, dengan cadangan leksikal bila statement ber-ERROR.

## 2. Keputusan desain

| ID | Keputusan | Alasan |
|---|---|---|
| D-1 | **SQL metadata ditulis di crate driver sebagai fungsi murni, dan dijalankan engine lewat `Session::execute`.** `Driver` mendapat satu metode bawaan `metadata(&self) -> Option<&dyn MetadataSql>`; `Session` tidak berubah. | §1.1. Tes Safe Mode melihat setiap statement lewat `RecordingSession`. `Held` dan belasan implementer `Session` (tiga driver dan sesi palsu di tes) tidak disentuh. SQL tetap milik driver (`explain_statement` memberi alasan yang sama), bisa dipatok oleh tes string tanpa server seperti `catalogs_sql`, dan diklasifikasi tes sebagai `ReadOnly`. Biayanya satu mesin langkah kecil di engine (§3.1). |
| D-2 | **Jenis objek lewat larik paralel `kinds` di event `tables`, hanya bila `OBJECT_KINDS=1`.** `names` tidak berubah. | P-06 dan NFR-C: golden dan CLI beku. Dekoder lama membaca `names` seperti biasa. Larik paralel (bukan larik objek) menjaga jalur dekode pohon yang ada. |
| D-3 | **Mode jenis di PostgreSQL membaca `pg_class`, dengan predikat visibilitas yang sama persis dengan `information_schema.tables`**, ditambah materialized view. | Himpunan tabel tidak boleh berubah hanya karena setelan dinyalakan. Predikat itu saya bandingkan di server dev: 5.213 baris di kedua sisi untuk `relkind IN ('r','p','v','f')`. Tanpa predikat, pohon akan menampilkan objek yang tidak boleh disentuh peran itu. Dipakai juga oleh `columns` dan `ddl` (§3.2). |
| D-4 | **Nama event dan kunci baru yang tidak bentrok:** `tables`+`kinds`; `table_columns` dengan `object` dan `fields`; `table_ddl` dengan `object`, `object_kind`, `ddl`, `redacted`; `execution_log` dengan `decisions` dan `chain`; `error` dengan `host_key` (properti Swift `hostKey`; begitu juga `object_kind` menjadi `objectKind`). Semua kunci `Event` baru ditambahkan **sekali, oleh T1**. | §1.2. Mengurangi sengketa `App.swift` (rantai kepemilikan di `development-plan.md` §7) menjadi satu commit aditif. T3 dan T4 hanya memakainya. |
| D-5 | **`columns` mengembalikan `name`, `type`, `nullable`, `default`, `extra`.** `extra` berisi `identity`, `generated`, `auto_increment`, atau kosong. | `default` saja menyesatkan: kolom `generated` di PostgreSQL punya ekspresi di `pg_attrdef` yang bukan default (terbukti di §3.2). MySQL dan PostgreSQL punya konsep ini secara asli; Trino kosong. |
| D-6 | **DDL: MySQL dan Trino lewat `SHOW CREATE`; PostgreSQL disusun dari katalog di Rust, dari baris teks yang dikembalikan server.** Resep bertahap (`DdlRecipe`) murni dan diuji dengan baris kaleng. Hasil melewati penyensoran kredensial (§3.6). | PostgreSQL tidak punya `SHOW CREATE`. Menyusun di Rust dari `format_type`, `pg_get_expr`, `pg_get_constraintdef`, `pg_get_indexdef`, dan `quote_ident` membuat teksnya mengikuti server, bukan penulisan ulang kita. Hasil adalah **rekonstruksi**, bukan `pg_dump`, dan teksnya mengatakan itu. |
| D-7 | **Batas tetap:** `columns` ≤ 10.000 baris, `ddl` ≤ 8 langkah dan ≤ 5.000 baris per langkah, semuanya di bawah `STATEMENT_TIMEOUT_MS` yang sama dengan perintah lain. Melewati batas menghasilkan `truncated:true`, bukan galat. | Katalog besar tidak boleh menahan sesi metadata yang hanya satu (O-7: 2+1). |
| D-8 | **`execution_log` adalah perintah lokal (`Route::Local`).** Pembaca memverifikasi rantai dan melaporkannya. Sink ditambahkan ke `EngineHost` lewat satu pintu `ensure_sink(settings)` yang **berkunci jalur database**: dipasang malas pada `run` pertama dan dipasang ulang bila jalur penyimpanan hasil `DB_PATH` berubah. Pembaca lewat pintu yang sama lalu `with_storage`, jadi pembaca dan penulis selalu satu database (§3.5). Filter per koneksi **tidak** ditawarkan di W11 (§3.5). | §1: sink tidak terpasang di app, dan tabel tidak punya kolom koneksi. `install` bersifat global per proses (`execution_log.rs:79-93`), jadi "sekali per proses" menangkap `DB_PATH` pertama untuk selamanya. |
| D-9 | **Nama setelan baru** (§4): `OBJECT_KINDS`, `EXECUTION_LOG_LIMIT`, `EXECUTION_LOG_VERIFY`, `SSH_USE_CONFIG`, `SSH_CONFIG_PATH`, `SSH_APP_KNOWN_HOSTS`, `SSH_HOST_KEY_ACCEPT`, `SSH_HOST_KEY_DETAIL`, `DB_JWT`, `DB_CA_FILE`. `Settings` mendapat `Debug` tersensor untuk kunci rahasia. | Satu kosakata `SSH_*` dan `DB_*` yang sudah ada diperluas, tidak diganti. |
| D-10 | **TOFU dipatok ke fingerprint, dicatat lebih dulu baru diterima, dan gagal tertutup.** Hanya keadaan `Unknown` tanpa `ca_covered` yang bisa diterima, hanya dengan pin yang sama dengan fingerprint yang disajikan, dan hanya bila catatan ke berkas app berhasil ditulis. (§5) | P-09, NFR-S4. |
| D-11 | **Urutan algoritma host key mengikuti kunci yang tercatat; `none` dilarang; penjaga "sudah diverifikasi" sebelum autentikasi.** (§5.5) | §1.3. Menutup alarm palsu tanpa melemahkan penolakan, dan menutup jalur `kex none`. |
| D-12 | **Alias `~/.ssh/config`:** `HostName`, `User`, `Port`, `IdentityFile`, dan `Include`; `Match`, `ProxyJump`, `ProxyCommand`, `HostKeyAlias`, `UserKnownHostsFile`, `CertificateFile`, dan setiap direktif tak dikenal di luar daftar kosmetik **ditolak dengan nama**. (§6) | FR-CON-02: "tidak diabaikan diam-diam". Penolakan adalah satu-satunya cara jujur tanpa kanal catatan di `connect`. |
| D-13 | **Pembawa galat terstruktur: varian `EngineError::HostKey` dan `CliError::HostKey`, dan objek `host_key` di event `error` hanya bila `SSH_HOST_KEY_DETAIL=1`.** | §1.2. Gerbang setelan memenuhi P-06 secara harfiah. |
| D-14 | **JWT: setelan `DB_JWT`, hanya Trino, hanya `Require` atau `RequireNoVerify`, tidak bersama `DB_PASSWORD`, dan tidak pernah dikirim lewat `http` **maupun ke origin selain origin sesi**: setiap permintaan ber-kredensial (POST, GET `nextUri`, DELETE cancel) memeriksa skema, host, dan port terhadap basis sesi sebelum header dipasang, dan klien bearer tidak mengikuti redirect (§7.1).** Penolakan bernama, bukan kenaikan diam-diam. | FR-CON-07 ("hanya lewat HTTPS") menyangkut setiap URL yang menerima header, bukan hanya basis sesi, dan dua dari tiga URL itu diberikan server. Token bearer yang bocor ke `http` atau ke host lain adalah kebocoran kredensial penuh. §1.4 dua butir terakhir. |
| D-15 | **CA per koneksi: setelan `DB_CA_FILE`, dibaca dan divalidasi saat `config::build`, hanya PostgreSQL dan Trino, hanya `Require`.** Kombinasi dengan `prefer`, `require`, `disable`, atau `DB_INSECURE` ditolak dengan nama. MySQL ditolak dengan alasan §12.1 PRD. | FR-CON-08, P-08. Byte PEM ikut `ConnectionConfig` jadi tidak ada TOCTOU antara validasi dan connect, dan hash-nya masuk kunci pool. |
| D-16 | **`ConnectionConfig.tls_server_name`: nama TLS asli yang dipertahankan lewat tunnel.** `retarget` mengisinya. PostgreSQL memakai `host` + `hostaddr`; Trino memakai `reqwest::ClientBuilder::resolve`; MySQL ditolak dengan pesan bernama untuk mode yang memverifikasi. | §1.4 dan ringkasan. Tanpa ini UC-01 dan UC-18 tidak bisa digabung. |
| D-17 | **Tiga item Keychain baru** pada layanan yang sama, akun `ssh-password:<UUID>`, `ssh-passphrase:<UUID>`, `jwt:<UUID>`, dengan UUID huruf besar. Kontrak nama diuji dengan literal di Rust dan Swift. | §1.5. Mengikuti pola `profile:`. |
| D-18 | **Field koneksi baru datar** di `connections.json` (`sshHost`, `sshPort`, `sshUser`, `sshAuth`, `sshKeyPath`, `sshUseConfig`, `caFile`, `dbAuth`), semuanya `decodeIfPresent`. Pemetaan MCP lewat `options_json` dengan nama yang sama, dan satu fixture JSON dipakai tes Rust dan Swift. | Mencegah "bekerja di app, tidak lewat MCP" (komentar `mcp.rs:429-437`). |
| D-19 | **MCP:** dua tool baru `describe_table` dan `table_ddl`; `columns` lama tetap. MCP **tidak pernah** mengirim `SSH_HOST_KEY_ACCEPT` dan selalu memasang `SSH_APP_KNOWN_HOSTS`. | NFR-S6, FR-MCP-01. Kanal tak-interaktif tidak boleh melakukan TOFU. |
| D-20 | **Satu titik untuk prompt host key di app: pembungkus `HostKeyGate` di sekitar `Engine.current`.** Ia membaca `hostKey` dari event `error` semua run, memunculkan satu sheet per host dan port, dan "Trust" menjalankan `test` dengan pin. Operasi yang gagal tidak diulang otomatis kecuali Test Connection. | Ada belasan titik `Engine.current.run` yang bisa membuka tunnel (§9.4). Menambal masing-masing berarti belasan jalur prompt. |
| D-21 | **Pohon:** `TreeNode.Kind` mendapat `view`, `materializedView`, `foreignTable`, `column`; objek berkolom bisa dibuka dan anaknya kolom. DDL membuka tab read-only yang **tidak dipulihkan** saat restore. | FR-TREE-01…03. Tab DDL adalah tampilan keadaan hidup, bukan teks pengguna. |
| D-22 | **API `references` mengembalikan relasi, CTE, dan sumber (`tree` atau `lexical`).** Snapshot per revisi dokumen hidup bersama dokumen, bukan cache metadata. Kolom tetap hanya di `TreeNode`. | PRD FR-ED-05 dan `performance-plan.md` §13: satu sumber kebenaran. |
| D-23 | **Navicat:** petakan `SSH_Host` dan atribut SSH lain yang **terverifikasi pada ekspor nyata**; sisanya dilaporkan sebagai tidak diimpor. | §18: hanya `SSH_Host` yang pernah dibaca kode ini. |
| D-24 | **App tidak punya tombol "Forget host key".** Rotasi dilakukan dengan `ssh-keygen -R` yang tercetak di pesan. | Jalur hapus di trust store adalah permukaan serangan baru. Satu klik di UI mengalahkan tujuan menolak keras. (§5.7) |
| D-25 | **Koreksi kepemilikan berkas** (§13.1): T1, T2, T3, T4, dan T6 menyentuh berkas yang tidak ada di baris rencana mereka. | Ditemukan dengan membaca jalur panggil, bukan menebak. |

## 3. Metadata read-only (W11-T1)

### 3.1 Bentuk di kode

**`crates/qh-driver/src/metadata.rs` (baru).** Tipe dan kontrak, tanpa I/O:

```rust
pub enum ObjectKind { Table, View, MaterializedView, ForeignTable }   // token: table, view, materialized_view, foreign_table
pub struct TableEntry { pub name: String, pub kind: Option<ObjectKind> }
pub struct ColumnInfo { pub name: String, pub data_type: String, pub nullable: Option<bool>,
                        pub default: Option<String>, pub extra: String }
pub struct ObjectDdl  { pub kind: Option<ObjectKind>, pub text: String, pub truncated: bool }
pub struct Rows { pub columns: Vec<String>, pub rows: Vec<Vec<Option<String>>> }   // semua sel teks

pub enum Step { Query(String), Done(ObjectDdl) }
pub trait DdlRecipe: Send {
    /// First call passes `None`; later calls pass the rows of the statement just run.
    fn next(&mut self, previous: Option<Rows>) -> Result<Step, EngineError>;
}
pub trait MetadataSql: Send + Sync {
    fn table_entries(&self, path: &ObjectPath) -> Result<String, EngineError>;
    fn parse_table_entries(&self, rows: &Rows) -> Result<Vec<TableEntry>, EngineError>;
    fn columns(&self, target: &ObjectPath) -> Result<String, EngineError>;
    fn parse_columns(&self, rows: &Rows) -> Result<Vec<ColumnInfo>, EngineError>;
    fn ddl(&self, target: &ObjectPath) -> Result<Box<dyn DdlRecipe>, EngineError>;
}
// crates/qh-driver/src/lib.rs, trait Driver:
fn metadata(&self) -> Option<&dyn MetadataSql> { None }
```

Alasan bentuk ini ada di D-1. Tiga hal yang mengikat implementer:

1. **Semua sel dikembalikan sebagai teks.** SQL-nya sendiri yang mengubah boolean menjadi `'YES'`/`'NO'` dan angka menjadi `::text`. Dengan begitu pembacaan tidak bergantung pada cara masing-masing driver mendekode tipe (`SHOW CREATE` MySQL memakai kolom `LONGTEXT`/blob, dan itu **harus dibuktikan live** di G-LIVE; §18).
2. **Identifier hanya masuk SQL lewat dua jalan:** `qh_sql::quote_ident` (MySQL dan Trino, lewat `sql_ident::qualified`) dan `quote_literal` (PostgreSQL membandingkan `nspname`/`relname` dengan literal). Tidak ada konkatenasi mentah. Tes: nama berisi `'`, `"`, backtick, `;`, dan `--` tetap menghasilkan satu statement `ReadOnly`.
3. **Setiap pembangun SQL terjangkau dari tes tanpa server**, lewat `Driver::metadata()` yang publik (pola `ssl_opts` MySQL yang `pub` supaya bisa dipatok), supaya tes klasifikasi di `crates/qh-ffi/tests/safe_mode.rs` bisa menjalankannya lewat `qh_sql::classify_readings` untuk setiap `Dialect::readings()`.

**`crates/qh-ffi/src/metadata.rs` (baru).** Menjalankan resep lewat `retry::execute` (sehingga `RETRIES` dihormati) dan `Cursor::next_batch`, mengubah `Value` menjadi sel teks (`NULL` menjadi `None`), dan memancarkan event. Satu fungsi pembantu `run_text(session, policy, sql, limit) -> Rows` menjadi satu-satunya tempat `execute` dipanggil, dengan batas baris D-7 dan `statement_timeout`. Perintah:

| Perintah | Setelan yang dibaca | Rute | Event |
|---|---|---|---|
| `columns` | koneksi, `TARGET_CATALOG`/`TARGET_SCHEMA`/`TARGET_TABLE` (slot per driver lewat `sql_ident::slots`) | `Pooled(Lane::Metadata)` | `table_columns` |
| `ddl` | sama | `Pooled(Lane::Metadata)` | `table_ddl` |
| `execution_log` | `EXECUTION_LOG_LIMIT`, `EXECUTION_LOG_VERIFY`, `DB_PATH` | `Local` | `execution_log` |
| `tables` (ada) | `OBJECT_KINDS=1` | tidak berubah | `tables` + `kinds` |

Slot yang kurang menghasilkan `Usage` yang menyebut setelannya, seperti `to_table` (`commands.rs:1056-1066`: "TARGET_SCHEMA and TARGET_TABLE required to ..."), **sebelum** koneksi dibuka. Driver tanpa `metadata()` menghasilkan `Usage` yang menyebut drivernya. `tables` tanpa `OBJECT_KINDS` tidak menyentuh kode baru sama sekali.

**Urutan yang mengikat di empat daftar invariant 11:** tambahkan `columns`, `ddl`, `execution_log` **di antara `table_op` dan `objects`** di `COMMANDS` (jadi `COMMANDS` menjadi 29 elemen), dengan urutan varian `Command` yang sama; `EngineCommand` dan `EVERY_COMMAND` (panjang 29) mengikuti urutan itu, dan `RustEngine.commands` mendapat tiga kata. `route()` di `host.rs` mendapat dua baris (`Columns | Ddl` ke `Pooled(Lane::Metadata)`, `ExecutionLog` ke `Local`). `local::*` menerima `Option<SharedStorage>` seperti perintah lokal lain, dan `run_with` meneruskannya.

### 3.2 SQL per driver, dengan bukti

Semua statement di bawah **dijalankan** pada 6 Okt 2026. PostgreSQL 17.11 di dalam `BEGIN … ROLLBACK` dengan skema `w11probe` (tabel dengan kolom identity dan generated, FK, `CHECK`, indeks parsial, view dengan nama berspasi, materialized view, tabel berpartisi dan partisinya). Sesudahnya `pg_namespace` dan `pg_roles` tidak memuat sisa. MySQL 8.4.11 dan Trino hanya baca terhadap objek yang sudah ada.

`<visible>` adalah satu konstanta bersama (D-3), dipatok tes string:

```sql
(pg_has_role(c.relowner, 'USAGE')
 OR has_table_privilege(c.oid, 'SELECT, INSERT, UPDATE, DELETE, TRUNCATE, REFERENCES, TRIGGER')
 OR has_any_column_privilege(c.oid, 'SELECT, INSERT, UPDATE, REFERENCES'))
```

Di server dev, peran `w11nobody` tanpa hak melihat 0 relasi di `w11probe`, dan himpunan untuk superuser sama dengan `information_schema.tables` (5.213 = 5.213).

#### PostgreSQL

**Daftar dengan jenis** (`OBJECT_KINDS=1`; `<s>` lewat `quote_literal`):

```sql
SELECT c.relname,
       CASE c.relkind WHEN 'r' THEN 'table' WHEN 'p' THEN 'table' WHEN 'v' THEN 'view'
                      WHEN 'm' THEN 'materialized_view' WHEN 'f' THEN 'foreign_table' END
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = '<s>' AND c.relkind IN ('r','p','v','m','f') AND <visible>
ORDER BY c.relname
```

Terbukti: `information_schema.tables` memuat `v` tetapi **tidak** `m`; partisi tampil sebagai `BASE TABLE` di sana, dan tetap `table` di sini (paritas).

**Kolom:**

```sql
SELECT a.attname::text, format_type(a.atttypid, a.atttypmod),
       CASE WHEN a.attnotnull THEN 'NO' ELSE 'YES' END,
       pg_get_expr(d.adbin, d.adrelid),
       CASE WHEN a.attidentity <> '' THEN 'identity'
            WHEN a.attgenerated <> '' THEN 'generated' ELSE '' END
FROM pg_attribute a
JOIN pg_class c ON c.oid = a.attrelid
JOIN pg_namespace n ON n.oid = c.relnamespace
LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
WHERE n.nspname = '<s>' AND c.relname = '<t>' AND c.relkind IN ('r','p','v','m','f')
  AND <visible> AND a.attnum > 0 AND NOT a.attisdropped
ORDER BY a.attnum
```

Terbukti: kolom materialized view muncul (`information_schema.columns` tidak memuatnya), `numeric(10,2)` dan `timestamp with time zone` tampil seperti yang diketik pengguna, dan kolom `doubled GENERATED ALWAYS AS (id * 2) STORED` mengembalikan `(id * 2)` di kolom default **dengan** `extra = 'generated'`. Tanpa `extra`, app akan menulis "default: (id * 2)", yang salah.

**DDL.** Resep `PostgresDdl` menjalankan, berurutan:

1. Kepala (relkind, nama terkutip server, definisi view, kunci partisi, batas partisi, induk partisi):

```sql
SELECT c.relkind::text, quote_ident(n.nspname), quote_ident(c.relname),
       CASE WHEN c.relkind IN ('v','m') THEN pg_get_viewdef(c.oid, true) END,
       CASE WHEN c.relkind = 'p' THEN pg_get_partkeydef(c.oid) END,
       CASE WHEN c.relispartition THEN pg_get_expr(c.relpartbound, c.oid) END,
       (SELECT quote_ident(pn.nspname) || '.' || quote_ident(pc.relname)
          FROM pg_inherits i JOIN pg_class pc ON pc.oid = i.inhparent
          JOIN pg_namespace pn ON pn.oid = pc.relnamespace
         WHERE i.inhrelid = c.oid AND c.relispartition)
FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = '<s>' AND c.relname = '<t>' AND c.relkind IN ('r','p','v','m','f') AND <visible>
```

   Nol baris berarti `Usage` "tidak ditemukan, atau peran ini tidak punya hak apa pun atasnya". `v` dan `m` selesai di sini: `CREATE [MATERIALIZED] VIEW <nama> AS` + definisi tanpa `;` penutup dan spasi pembuka (keluaran `pg_get_viewdef` berawalan spasi dan berakhir `;`, terbukti) + `;`. `f` menghasilkan `Usage` "DDL for foreign tables is not reconstructed" (D-6; tampil di pohon tetapi DDL belum).
2. Kolom: pernyataan "Kolom" di atas, tetapi dengan `quote_ident(a.attname)` dan `a.attidentity`/`a.attgenerated` mentah supaya Rust menulis `GENERATED ALWAYS|BY DEFAULT AS IDENTITY` atau `GENERATED ALWAYS AS (<ekspresi>) STORED`.
3. Constraint:

```sql
SELECT quote_ident(co.conname), co.contype::text, pg_get_constraintdef(co.oid, true)
FROM pg_constraint co JOIN pg_class c ON c.oid = co.conrelid JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = '<s>' AND c.relname = '<t>' AND co.contype IN ('p','u','f','c','x')
ORDER BY CASE co.contype WHEN 'p' THEN 0 WHEN 'u' THEN 1 WHEN 'x' THEN 2 WHEN 'c' THEN 3 ELSE 4 END, co.conname
```

4. Indeks yang tidak dimiliki constraint:

```sql
SELECT pg_get_indexdef(x.indexrelid)
FROM pg_index x JOIN pg_class t ON t.oid = x.indrelid JOIN pg_class i ON i.oid = x.indexrelid
JOIN pg_namespace n ON n.oid = t.relnamespace
WHERE n.nspname = '<s>' AND t.relname = '<t>'
  AND NOT EXISTS (SELECT 1 FROM pg_constraint co WHERE co.conindid = x.indexrelid)
ORDER BY i.relname
```

Rakitan Rust: `CREATE TABLE <nama> (` kolom (satu per baris, `NOT NULL`, `DEFAULT <ekspr>`, identity, generated), constraint bernama (`CONSTRAINT <n> <def>`), `)` dan `PARTITION BY <kunci>` bila `p`; bila partisi, `CREATE TABLE <nama> PARTITION OF <induk> <batas>` menggantikan daftar kolom; lalu setiap indeks pada baris sendiri. Baris pertama selalu komentar tetap: `-- Reconstructed from the catalog by QueryHive. Storage options, tablespace, ownership, grants, comments, triggers and rules are not included.` Terbukti di server dev: `pg_get_constraintdef` menghasilkan `FOREIGN KEY (parent_id) REFERENCES w11probe.parent(id) ON DELETE CASCADE`, `pg_get_indexdef` menghasilkan `CREATE INDEX child_parent_ix ON w11probe.child USING btree (parent_id) WHERE (id > 5)`, `pg_get_partkeydef` menghasilkan `RANGE (d)`, dan `quote_ident('My View')` menghasilkan `"My View"`.

Tes murni untuk rakitan: baris kaleng dari keluaran di atas menghasilkan teks yang ditempel persis di berkas golden `crates/qh-driver-postgres/tests/fixtures/ddl_*.sql`; pembangkit teks tidak boleh memuat `;` yang bukan penutup statement.

#### MySQL

```sql
-- daftar dengan jenis (OBJECT_KINDS=1): kolom ke-2 adalah BASE TABLE | VIEW | SYSTEM VIEW
SHOW FULL TABLES FROM `<db>`
-- kolom
SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT, EXTRA
FROM information_schema.COLUMNS WHERE TABLE_SCHEMA = '<db>' AND TABLE_NAME = '<t>'
ORDER BY ORDINAL_POSITION
-- DDL (berlaku untuk tabel DAN view)
SHOW CREATE TABLE `<db>`.`<t>`
```

Terbukti: `SHOW FULL TABLES FROM qh` mengembalikan `(type_zoo, BASE TABLE)`; `SHOW CREATE TABLE` pada view (`sys.version`, dijalankan sebagai root karena `SHOW VIEW`) mengembalikan baris `(View, Create View, character_set_client, collation_connection)`, dan pada tabel `(Table, Create Table)`. **Jenis objek DDL dibaca dari nama kolom pertama** (`View` atau `Table`), jadi satu langkah cukup. Teks DDL ada di kolom indeks 1. `EXTRA` berisi `auto_increment`, `DEFAULT_GENERATED`, `VIRTUAL GENERATED`, dan sebagainya, dan dipakai langsung sebagai `extra`. `COLUMN_DEFAULT` `NULL` menjadi `default: None`. Pemetaan `SYSTEM VIEW` ke `view`.

#### Trino

```sql
-- daftar dengan jenis: 'BASE TABLE' | 'VIEW'
SELECT table_name, table_type FROM "<c>".information_schema.tables
WHERE table_schema = '<s>' ORDER BY table_name
-- kolom (information_schema.columns Trino tidak punya kolom extra)
SELECT column_name, data_type, is_nullable, column_default
FROM "<c>".information_schema.columns WHERE table_schema = '<s>' AND table_name = '<t>'
ORDER BY ordinal_position
-- DDL, langkah 1 lalu langkah 2
SELECT table_type FROM "<c>".information_schema.tables WHERE table_schema = '<s>' AND table_name = '<t>'
SHOW CREATE VIEW "<c>"."<s>"."<t>"      -- bila VIEW
SHOW CREATE TABLE "<c>"."<s>"."<t>"     -- selain itu
```

Terbukti: `SHOW CREATE TABLE tpch.tiny.nation` mengembalikan DDL satu sel; `SHOW CREATE VIEW` pada tabel menjawab `Relation 'tpch.tiny.nation' is a table, not a view`, dan `SHOW CREATE MATERIALIZED VIEW` pada tabel menjawab `... is a table, not a materialized view`, jadi **jenis harus diketahui lebih dulu** dan resep dua langkah memang perlu. Resep menangani satu pengecualian: bila `SHOW CREATE TABLE` gagal dengan teks `is a materialized view`, ia mencoba `SHOW CREATE MATERIALIZED VIEW` satu kali. Materialized view Trino **tidak bisa diuji** di lingkungan dev (§18). Di sisi pohon Trino hanya tabel dan view yang diberi label (FR-TREE-01).

`SHOW TABLES` (jalur lama) dan `information_schema.tables` (jalur jenis) menjawab himpunan yang sama: terbukti pada `tpch.tiny` (8 tabel, selisih kosong) dan pada MySQL `qh` (`SHOW TABLES` dan `SHOW FULL TABLES` sama, 2 tabel). Dev tidak punya view biasa di keduanya, jadi pernyataan bahwa kedua jalur sama-sama memuat view berasal dari dokumentasi server, bukan percobaan (§18). G-LIVE membandingkan keduanya pada setiap rekaman.

### 3.3 Event

```json
{"event":"tables","names":["a","b"],"kinds":["table","view"]}
{"event":"table_columns","object":{"catalog":"hive","schema":"analytics","table":"t"},
 "fields":[{"name":"id","type":"bigint","nullable":false,"default":null,"extra":"identity"}],"truncated":false}
{"event":"table_ddl","object":{"catalog":null,"schema":"analytics","table":"v"},
 "object_kind":"view","ddl":"CREATE VIEW ...;","truncated":false,"redacted":false}
{"event":"execution_log","decisions":[{"seq":12,"id":"...","at":1790000000000,"safe_mode":"read_only",
 "decision":"refused","statement_kind":"dml","statement_index":1,"statement_hash":"<hex>","reason":"..."}],
 "chain":{"verified":true,"rows":214},"writer":true}
```

Kunci `object` selalu memuat tiga kunci (`null` untuk slot yang tidak dimiliki driver). `nullable` dan `default` boleh `null` (tidak diketahui). `chain` yang rusak: `{"verified":false,"seq":N,"detail":"..."}`; hasilnya **tetap dikirim** dengan `decisions` karena melihat baris adalah cara mendiagnosisnya. Kolom `statement` tidak ada di mana pun: tabel hanya menyimpan hash (`0007_execution_log.sql:4-8`).

### 3.4 Perilaku `tables` dengan `OBJECT_KINDS=1`

- Panjang `kinds` sama dengan `names`, urutan sama. Driver tanpa `metadata()` mengembalikan `names` saja (tanpa kunci `kinds`), dan app memperlakukan nama tanpa jenis sebagai tabel, seperti hari ini.
- Himpunan nama berbeda dari mode biasa **hanya** di PostgreSQL: materialized view dan foreign table bertambah, dan view bertambah (yang hari ini tidak tampil sama sekali). Itu perubahan tampilan yang terdaftar sebagai V-11 (scene baru pohon), bukan perubahan piksel di baseline yang ada.
- `DB_ALL_SCHEMAS` tidak berpengaruh pada `tables` (komentar `commands.rs:808-809`).

### 3.5 Pembaca execution log (FR-SAFE-02, sisi engine)

- Perintah lokal. `EXECUTION_LOG_LIMIT` bawaan 200, tertinggi 5.000 (di atasnya `Usage`). `Storage::execution_log(limit)` sudah ada (`crates/qh-storage/src/execution_log.rs:227`), begitu juga `verify_execution_log()` (`:245`). Tidak ada perubahan di `qh-storage`.
- `EXECUTION_LOG_VERIFY` bawaan `1`. Verifikasi memuat semua baris dan menghitung SHA-256 satu per satu (O(N)). Untuk log yang membengkak, app boleh mengirim `0` dan menampilkan "belum diverifikasi". Hasil verifikasi bukan bukti keaslian (log hanya tamper-evident, bukan tamper-proof, kata modulnya sendiri).
- **Sink tidak terpasang di app (§1).** T1 memasang sink di `EngineHost`. `execution_log::install` bersifat **global per proses** (`SINK: OnceLock<Mutex<Option<Storage>>>`, `crates/qh-ffi/src/execution_log.rs:79-93`; `install` mengganti handle yang ada, `:88-92`). Jadi "dipasang malas pada `run` pertama, sekali per proses" menangkap `DB_PATH` mana pun yang tiba lebih dulu. Tes Swift berjalan dalam satu proses dengan `DB_PATH` per tes (`TestIsolation`) dan akan berbagi satu log, dan pembaca (`Route::Local`, membuka database dari `DB_PATH` sendiri) bisa membaca database lain daripada yang ditulis sink. Rancangannya:
  1. **Sink berkunci.** `execution_log` menyimpan jalur penyimpanan yang terpasang berdampingan dengan handle-nya, dan `EngineHost::ensure_sink(settings)` menghitung jalur dengan aturan `local::open_storage` yang sama (`DB_PATH`, atau berkas Application Support). Jalur sama dengan yang terpasang: tidak melakukan apa-apa (satu perbandingan per `run`). Beda atau belum ada: membuka dan memasang ulang. Di produksi app punya satu `DB_PATH`, jadi pemasangan ulang hanya terjadi di tes dan di CLI.
  2. **Pembaca lewat `with_storage`.** `execution_log` memanggil `ensure_sink(settings)` lebih dulu, lalu membaca dengan `execution_log::with_storage` (`:114`) dari handle yang sama dengan yang menulis. Hanya bila sink tidak bisa dipasang (galat membuka) ia membuka database dari `DB_PATH` hanya-baca, dengan `"writer": false`.
  3. **Batas yang dikatakan:** dua `run` serentak dengan `DB_PATH` berbeda dalam satu proses saling membalik sink. Itu tidak didukung dan hanya bisa terjadi di tes yang menjalankan beberapa database serentak dalam satu proses; tes seperti itu tidak boleh bergantung pada log, dan komentar `ensure_sink` mengatakannya.

  Kegagalan membuka log **tidak menghentikan perintah** (sama dengan CLI), tetapi tidak boleh diam: `execution_log` melaporkan `"writer": false` bila sink tidak terpasang, dan silent-failure-hunter adalah pemeriksa wajib di T1. Tes (`crates/qh-ffi/tests/host.rs`): `EngineHost::run(preview, SAFE_MODE=read_only, "DELETE FROM t")` menambah satu baris `refused` **di database `DB_PATH` run itu**; dua `EngineHost` berurutan dengan `DB_PATH` berbeda menulis ke database masing-masing dan `execution_log` masing-masing membaca barisnya sendiri; jalur yang sama tidak membuka ulang (hitungan pembukaan).
- **Filter per koneksi tidak bisa dipenuhi.** Tabel tidak punya kolom koneksi, dan komentar migrasi menegaskan "Hosts, users, passwords and the Keychain reference live nowhere here". Menambah `connection_id` berarti: kolom nullable via migrasi `0009`, **dan** versi rantai baru, karena `chain_hash` mencakup daftar field tetap (`execution_log.rs:93-134`). Menambahkannya di luar rantai membuat label bisa diubah tanpa terdeteksi. **Keputusan untuk pemilik sebelum W13-T6.** Rekomendasi: tunda ke W13, tambahkan sebagai kolom di dalam rantai dengan kolom `chain_version`, dan tulis amandemen FR-SAFE-02 bila pemilik memilih tidak. W11 mengirim semua baris.

### 3.6 Penyensoran kredensial di DDL

UC-15 menuntut "tidak ada kredensial di keluaran". DDL bisa memuatnya: tabel MySQL berengine FEDERATED memuat `CONNECTION='mysql://user:password@host:port/db/table'` di `SHOW CREATE TABLE`, dan `CREATE FOREIGN TABLE` PostgreSQL memuat opsi server. Aturan, diterapkan di `qh-ffi/src/metadata.rs` pada teks DDL **sebelum** event dikirim, sehingga tab app dan MCP sama:

- `CONNECTION='<skema>://<pengguna>:<sandi>@...'` mengganti `<sandi>` dengan `***`.
- Opsi bernama `password`, `passwd`, `secret`, `token`, atau `apikey` (tanpa memandang huruf besar) pada `OPTIONS (...)` mengganti nilainya dengan `***`.
- Disensor dilaporkan: `"redacted": true` pada event, dan app menampilkan satu baris catatan.

Ini best-effort, dan harus dikatakan begitu di ADR-0038: definisi view bisa memuat literal sensitif yang ditulis pemiliknya, dan tidak ada aturan yang menangkapnya. Itu data pengguna, bukan kredensial koneksi, dan batasnya sama dengan `preview`. Tes: fixture FEDERATED dan `OPTIONS` dengan tiga ejaan kunci.

### 3.7 Tes yang ditulis lebih dulu (T1)

- `crates/qh-driver-{postgres,mysql,trino}/src/metadata.rs`: string SQL dipatok persis (pola `the_metadata_statements_match_the_ones_they_replace`, `crates/qh-driver-postgres/src/lib.rs:1231-1267`); `quote_literal` dan `quote_ident` dengan nama berbahaya; parser dengan baris kaleng; rakitan DDL PostgreSQL ke berkas golden.
- `crates/qh-ffi/tests/safe_mode.rs` (NFR-S1): `columns`, `ddl`, dan `tables` dengan `OBJECT_KINDS=1` dijalankan lewat `RecordingSession` di bawah **setiap** `SAFE_MODE`, termasuk `read_only`. Semua statement yang tercatat harus `StatementKind::ReadOnly` di setiap `Dialect::readings()`. Nama berbahaya tidak menambah statement kedua. Perintah tidak menyebabkan `enforce_read_only` gagal.
- `crates/qh-ffi/tests/golden.rs`: kasus lama tidak berubah (G-GOLDEN); kasus `_live` baru untuk `columns` dan `ddl` PostgreSQL, MySQL, dan Trino direkam dengan `--record` dan setiap selisih terhadap server hidup diklasifikasi di `docs/golden-deltas.md`.
- Tes pooled: `columns` lewat `PooledEngine` (rute `Metadata`) menjawab, bukan "tidak didukung" (menjaga D-1 dari regresi ke metode `Session`).
- `uniffi_api.rs`: `EVERY_COMMAND`, `COMMANDS`, dan `arm` yang eksaustif sama-sama 29. `RustEngineTests` (Swift) membandingkan `commandWords` dengan `commandNames()`.
- `EventDecodingTests`: setiap kunci baru di §3.3 dan `hostKey` (§5.8) terdekode, tanpa mengubah dekode event lama.

## 4. Setelan baru dan yang berubah arti

Semua nama mengikuti kosakata yang ada: `DB_*` untuk koneksi, `SSH_*` untuk bastion (`crates/qh-ffi/src/tunnel.rs:11-31`), dan setelan lain huruf besar bersambung garis bawah. Tidak ada alias `TRINO_*` untuk setelan baru.

| Setelan | Dibaca di | Arti | Bawaan | Rahasia |
|---|---|---|---|---|
| `OBJECT_KINDS` | `commands::tables` | `1` menambah larik `kinds` ke event `tables` | `0` | tidak |
| `TARGET_CATALOG`, `TARGET_SCHEMA`, `TARGET_TABLE` | `columns`, `ddl` | **sudah ada** (`to_table`, `table_op`, `apply_changes`); slot yang dipakai per driver dari `sql_ident::slots` | kosong | tidak |
| `EXECUTION_LOG_LIMIT` | `execution_log` | jumlah baris terbaru, 1 sampai 5.000 | `200` | tidak |
| `EXECUTION_LOG_VERIFY` | `execution_log` | `0` melewati verifikasi rantai | `1` | tidak |
| `SSH_USE_CONFIG` | `tunnel::settings` | `1`: `SSH_HOST` adalah alias `Host` di `~/.ssh/config`, dan `HostName`, `User`, `Port`, `IdentityFile` diambil darinya bila setelan eksplisit kosong | `0` | tidak |
| `SSH_CONFIG_PATH` | `tunnel::settings` | berkas config (untuk tes dan pengguna yang memindahkannya) | `~/.ssh/config` | tidak |
| `SSH_APP_KNOWN_HOSTS` | `tunnel::settings` | berkas `known_hosts` milik app: **dibaca dan ditulis** (§5.2). Tanpa setelan ini engine tidak pernah menulis `known_hosts` dan tidak ada jalur TOFU. | kosong | tidak |
| `SSH_HOST_KEY_ACCEPT` | `tunnel::settings` | satu fingerprint `SHA256:<43 karakter base64 tanpa padding>`. Hanya berlaku untuk satu run, hanya untuk keadaan `unknown` (§5.3). Bentuk yang salah adalah `Usage` sebelum jaringan disentuh. Pin yang dihitung dari `ssh-keyscan` mengalahkan tujuannya (itu fingerprint dari jaringan yang sama) dan dicatat begitu di ADR-0039 (Q5). | kosong | tidak (publik), tetapi tidak pernah disimpan |
| `SSH_HOST_KEY_DETAIL` | `main.rs::report`, `uniffi_api.rs::fail` | `1` menambah objek `host_key` ke event `error` bila kegagalan itu host key (§5.8) | `0` | tidak |
| `DB_JWT` | `config::build` | token bearer Trino (§7.1) | kosong | **ya** |
| `DB_CA_FILE` | `config::build` | berkas PEM CA untuk PostgreSQL dan Trino (§7.2); `~` dikembangkan | kosong | tidak |

**Yang tidak berubah arti:** `SSH_HOST`, `SSH_PORT`, `SSH_USER`, `SSH_AUTH_METHOD`, `SSH_KEY_PATH`, `SSH_KEY_PASSPHRASE`, `SSH_PASSWORD`, dan `SSH_KNOWN_HOSTS`. `SSH_KNOWN_HOSTS` tetap berkas yang **hanya dibaca** dan mengganti `~/.ssh/known_hosts` bila diisi. Ia tidak pernah ditulis, bahkan oleh jalur TOFU. `/etc/ssh/ssh_known_hosts` ikut dibaca (hanya baca) di samping keduanya (§5.2).

**`Settings` mendapat `Debug` tersensor (T2, `env.rs`).** Implementasi tangan yang mencetak kunci dan mengganti nilai `DB_PASSWORD`, `TRINO_PASSWORD`, `DB_URL`, `TRINO_URL` (bisa memuat sandi), `DB_JWT`, `SSH_PASSWORD`, dan `SSH_KEY_PASSPHRASE` dengan `<redacted>`. Daftar kunci rahasia adalah satu konstanta yang dipatok tes, dan tes lain memeriksa bahwa setiap setelan yang dibaca `connection`/`tunnel::settings` lewat `Settings::raw` untuk nilai rahasia ada di daftar itu. Ini menutup bahaya laten §1.5.

## 5. Protokol TOFU untuk host key SSH (W11-T2, ditinjau SEC sebelum dimulai)

Bagian ini mengikat. Apa pun yang tidak tertulis di sini **tidak** diterima otomatis.

### 5.1 Aset, ancaman, dan batas

- **Aset:** kredensial bastion (password, tanda tangan kunci privat, passphrase) dan akses ke database di balik bastion; integritas daftar kunci tepercaya.
- **Ancaman yang dibela:**
  - A. Penyerang di jaringan pada koneksi **pertama**, yang menyajikan kuncinya sendiri.
  - B. Penyerang di jaringan pada koneksi **berikutnya**, setelah kunci asli tercatat.
  - C. Kunci yang diganti antara prompt dan percobaan ulang (TOCTOU, P-09).
  - D. Kunci yang sudah dicabut (`@revoked`).
  - E. Server yang menawarkan sertifikat host yang build ini tidak bisa memverifikasi, dan host yang `@cert-authority` di `known_hosts` pengguna sendiri nyatakan seharusnya menyajikan sertifikat tetapi menyajikan kunci polos (jalur penurunan bagi penyerang di jaringan, Q2).
  - F. Pengguna yang terburu-buru mengklik terima, dan saluran tak-interaktif (CLI tanpa pin, MCP, `warmUp`) yang tidak boleh punya jalur terima sama sekali.
  - G. Tanda tangan atau password yang terkirim ke bastion yang belum teridentifikasi.
- **Di luar batas, dan dikatakan di ADR-0039:** penyerang yang bisa menulis berkas pengguna macOS yang sama (ia juga bisa mengganti app); penyerang yang menang pada koneksi pertama karena pengguna tidak membandingkan fingerprint dengan sumber di luar jalur. Itu batas semua TOFU, dan satu-satunya pembelaannya adalah kata-kata di sheet (§5.4).

### 5.2 Penyimpanan

| Berkas | Jalur | Dibaca | Ditulis | Izin |
|---|---|---|---|---|
| Milik pengguna | `~/.ssh/known_hosts`, atau `SSH_KNOWN_HOSTS` bila diisi | ya | **tidak pernah** | tidak disentuh |
| Milik sistem | `/etc/ssh/ssh_known_hosts` (jalur `GlobalKnownHostsFile` bawaan OpenSSH; di `BastionConfig.known_hosts` sesudah berkas pengguna) | ya, bila ada | **tidak pernah** | tidak disentuh; berkas yang tidak ada = kosong |
| Milik app | `SSH_APP_KNOWN_HOSTS`; app mengisi `~/Library/Application Support/QueryHive/known_hosts` (direktori yang sama dengan `connections.json`; MCP memakai `qh_storage::import::legacy_directory()`) | ya | hanya oleh jalur penerimaan §5.5, hanya menambah | berkas `0600`, direktori `0700` bila dibuat (`known_hosts::append`, `known_hosts.rs:223-266`); berkas yang sudah ada tidak diubah modenya, tetapi ditolak bila tidak aman (butir 4), juga saat dibaca |

Aturan berkas app, semuanya diuji:

1. **Format OpenSSH**, satu baris per (host dan port, kunci). Host ditulis `host` untuk port 22 dan `[host]:port` selain itu (`host_spelling`, `known_hosts.rs:111-117`), huruf kecil (`append_if_absent` melipat host sendiri: `known_hosts_line` menulis `host_spelling(host)` apa adanya, `key.rs:59-66`, dan pencocokan sudah melipat, `known_hosts.rs:349-353`, jadi ini kosmetik dan hanya menjaga pemeriksaan idempoten konsisten), tidak berhash. Baris diakhiri komentar `# accepted by QueryHive <UTC ISO-8601>` (parser mengabaikan apa pun sesudah kunci, `known_hosts.rs:319`), supaya rotasi dan audit bisa melihat kapan sebuah kunci diterima.
2. **Identitas catatan adalah nama yang dihubungi**, yaitu `HostName` hasil resolusi alias (§6), bukan alias. Dua nama untuk satu bastion (nama dan IP) adalah dua catatan dan dua prompt. Itu disengaja: engine tidak menebak bahwa dua nama itu sama.
3. **Penambahan idempoten:** `append_if_absent` membaca ulang berkas (bukan cache) dan tidak menulis bila kunci yang sama sudah ada untuk host itu, pada deskriptor yang sama dengan yang dipakai menulis (butir 4). Dua penerimaan serentak tidak menggandakan baris. Tidak ada penguncian antar proses. Baris ganda tidak berbahaya, dan itu dicatat sebagai risiko tersisa.
4. **Fail-closed pada berkas yang tidak aman, tanpa TOCTOU (Koreksi SEC, Q4).** Pemeriksaannya proporsional dan dikerjakan **pada deskriptor yang sudah terbuka**, bukan pada jalur: berkas app dibuka dengan `O_NOFOLLOW` (symlink pada komponen terakhir gagal saat dibuka), lalu `fstat` atas deskriptor itu (bukan `lstat` atas jalur) harus menunjukkan pemilik `uid == geteuid()` dan `mode & 0o022 == 0`. Gagal → `HostKeyStoreUnsafe` yang menyebut `chmod 600` atau pemilik yang salah. Pemeriksaan yang sama berlaku **saat berkas app dibaca di `check_all`**, bukan hanya saat ditambah: berkas app yang dapat ditulis grup tetap bisa menghasilkan `Matched` bila hanya jalur tulis yang diperiksa. Satu fungsi `open_app_store` dipakai kedua jalur, dan `append_if_absent` membaca, memeriksa idempotensi, lalu menambah pada **satu deskriptor** (tidak ada pembukaan kedua di antara pemeriksaan dan penulisan). `O_NOFOLLOW` hanya melindungi komponen terakhir jalur; direktori induknya dibuat `0700` oleh kita (`create_dir_0700`), dan penyerang yang bisa menulis di dalamnya sudah di luar batas §5.1. `crates/qh-tunnel` memakai `unsafe_code = "forbid"` (`Cargo.toml`, `[lints.rust]`), jadi `geteuid` dan `O_NOFOLLOW` datang dari `rustix` (`process::geteuid`, `fs::OFlags::NOFOLLOW`, API aman; `rustix 1.1.5` sudah ada di `Cargo.lock` sebagai dependensi transitif dan menjadi dependensi langsung `qh-tunnel`, G-DENY berlaku; dibaca di sumbernya, tidak dikompilasi, §18). Berkas milik pengguna dan berkas sistem **tidak** diperiksa dengan cara ini: kita tidak menulisnya, dan modenya pilihan pemiliknya.
5. **App tidak pernah menghapus atau menulis ulang** baris (D-24, §5.7).

### 5.3 Keadaan dan transisi

Pemeriksaan membaca **semua** berkas (milik pengguna, lalu `/etc/ssh/ssh_known_hosts`, lalu milik app) lewat `known_hosts::check_all` yang baru. Hasilnya penggabungan, tanpa dipengaruhi urutan berkas atau urutan baris:

1. Baris yang tidak bisa diparse di berkas mana pun → galat (`MalformedKnownHosts`), tidak dilewati. Berkas yang ada tetapi tidak terbaca → galat. Berkas yang tidak ada → kosong (keadaan TOFU pertama; berkas sistem yang tidak ada adalah keadaan biasa). Berkas app dibuka lewat pemeriksaan §5.2 butir 4: yang tidak aman → `HostKeyStoreUnsafe`, tidak pernah `Matched` atau `Unknown`.
2. Kunci yang disajikan sama persis dengan baris `@revoked` yang cocok dengan host di berkas mana pun → **Revoked**.
3. Selain itu, ada baris biasa di berkas mana pun dengan byte kunci yang sama → **Matched** (jenis kunci diambil dari blob, bukan dari token teks; `key.rs:92-103`).
4. Selain itu, ada baris biasa untuk host itu di berkas mana pun → **Mismatch**, membawa semua catatan beserta **berkas asal** (`RecordedKey` mendapat `source: Option<PathBuf>`, karena nomor baris tanpa berkas ambigu; asalnya `app`, `user`, atau `system`).
5. Selain itu → **Unknown**, dengan `ca_covered` bila ada baris `@cert-authority` yang cocok di berkas mana pun. **Unknown dengan `ca_covered` adalah tolak keras** (Koreksi SEC, Q2): lihat tabel.

| Keadaan | `host_key.state` | Tanpa pin | Pin = fingerprint yang disajikan | Pin ≠ fingerprint yang disajikan |
|---|---|---|---|---|
| Matched | | lanjut | lanjut (pin diabaikan, tidak ditulis ulang) | lanjut (pin diabaikan) |
| Unknown, tanpa `ca_covered` | `unknown` | **tolak**, kirim detail | **catat, lalu terima** (§5.5) | **tolak** `pin_mismatch` |
| Mismatch | `changed` | **tolak keras** | **tolak keras** (pin tidak berlaku) | **tolak keras** |
| Revoked | `revoked` | **tolak keras** | **tolak keras** | **tolak keras** |
| Sertifikat host ditawarkan | `certificate` | tolak | tolak | tolak |
| Unknown dengan `ca_covered` (Koreksi SEC, Q2) | `certificate_expected` | **tolak keras** | **tolak keras** (pin tidak berlaku) | **tolak keras** |
| Unknown, `SSH_APP_KNOWN_HOSTS` kosong | | tolak | `Usage` sebelum jaringan: tidak ada tempat mencatat | `Usage` |
| Unknown, mencatat gagal | `record_failed` | | **tolak, tidak diterima** | |

"Tolak keras" berarti tidak ada setelan, argumen, atau jalur UI yang mengubah hasilnya selain pengguna menyunting berkas (§5.7); untuk `certificate_expected`, itu berarti menghapus baris `@cert-authority` yang menyatakan host itu seharusnya menyajikan sertifikat. Pesan galat untuk keadaan ini tidak menawarkan opsi menerima.

### 5.4 Prompt pertama (kontrak untuk `HostKeySheet`, W11-T3)

Isi wajib: host dan port yang benar-benar dihubungi, alias bila ada, jenis kunci, fingerprint `SHA256:…` utuh (dikelompokkan untuk dibaca, disalin tanpa spasi), dan tempat catatan akan ditulis (keadaan `ca_covered` tidak punya prompt: itu penolakan keras, butir 7). Kalimat wajib: fingerprint harus dibandingkan dengan sumber di luar jalur, misalnya administrator server atau `ssh-keygen -lf /etc/ssh/ssh_host_*_key.pub` di server itu. **Sheet tidak menyuruh menjalankan `ssh-keyscan` dari mesin ini**: itu mengambil kunci dari jaringan yang sama dan bukan verifikasi.

Invarian yang diuji `SR` dan `AX` dan diperiksa `SEC`:

1. Tombol bawaan adalah **Batal**. "Trust" bukan tombol bawaan, tanpa `.keyboardShortcut(.defaultAction)` dan tanpa ⏎.
2. Pin yang dikirim adalah `hostKey.fingerprint` dari event **percobaan yang sama** dengan yang ditampilkan. Ia tidak dihitung ulang, tidak di-cache antar percobaan, dan `HostKeyGate` menegaskan `(host, port, fingerprint)` event sama dengan yang di sheet sebelum mengirim.
3. Tidak ada "selalu percaya", "jangan tanya lagi", atau kotak centang yang menyimpan jawaban. Pin hidup dalam satu pemanggilan `run`, tidak pernah di `connections.json`, `Session`, atau `UserDefaults`.
4. Satu sheet per (host, port, fingerprint). Event yang sama dari run lain yang berjalan bersamaan (tree dan Run, §9.4) tidak membuka sheet kedua.
5. Prompt hanya karena aksi pengguna di latar depan. `warmUp` (`AppModel.warmUp`) menelan galatnya, dan tidak pernah memunculkan sheet.
6. VoiceOver membacakan fingerprint per kelompok empat karakter dan label tombol "Trust and connect to <host>".
7. Keadaan `changed`, `revoked`, `certificate`, dan `certificate_expected` memakai sheet yang **tidak punya tombol terima sama sekali**, hanya penjelasan dan perintah yang bisa disalin (§5.7). Untuk `certificate_expected`, penjelasannya: `known_hosts` pengguna sendiri menyatakan host ini seharusnya menyajikan sertifikat, build ini tidak bisa memverifikasi sertifikat, dan app menolak menerima kunci polos untuknya (kunci polos di host yang dikelola CA adalah tanda penyerang di jaringan, atau konfigurasi yang berubah). Bila pengguna memang tidak ingin host itu dikelola CA, ia menyunting baris `@cert-authority` itu sendiri; sheet menyebut berkas dan nomor barisnya bila diketahui.

### 5.5 Pada saat penerimaan, dan jaminan sisi engine

Urutan di dalam `HostKeyVerifier::check_server_key`, yang berjalan selama key exchange sebelum satu byte autentikasi:

1. Susun kunci yang disajikan (`ServerKey`) dan hitung fingerprint (`SHA256:` + base64 tanpa padding dari SHA-256 atas blob, `key.rs:74-76`; sama dengan `ssh-keygen -lf`, diverifikasi tes `fingerprint_matches_ssh_keygen`).
2. `check_all` memberi keadaan. Hanya **Unknown tanpa `ca_covered`** yang bisa lewat ke langkah 3; Unknown dengan `ca_covered` berakhir `HostKeyCertificateExpected` (Q2, §5.3). `HostKeyPolicy::TrustNew` (pola dua langkah untuk pemanggil perpustakaan) tidak berubah dan tidak pernah dipakai engine.
3. Bila `SSH_HOST_KEY_ACCEPT` kosong → `HostKeyUnknown` (§5.3). Bila tidak sama persis (perbandingan string kanonik) dengan fingerprint yang disajikan → `HostKeyPinMismatch`.
4. **Catat dulu:** `append_if_absent(app_file, host, port, blob)` (satu deskriptor, dengan pemeriksaan §5.2 butir 4). Bila gagal (disk penuh, izin, direktori tak ada dan tak bisa dibuat) → `HostKeyRecordFailed`, kunci **tidak diterima**. Tidak ada penerimaan yang tidak tercatat.
5. Baru kembalikan `Ok(true)`. Autentikasi berlanjut.

Penjaga tambahan (D-11), semuanya di `crates/qh-tunnel/src/tunnel.rs`:

- **Penjaga "sudah diverifikasi".** `HostKeyVerifier` mencatat ke `Arc<AtomicBool>` bahwa ia mencapai `Ok(true)` lewat Matched atau penerimaan tercatat. `Tunnel::open` menolak memanggil `authenticate` bila bendera itu belum menyala (`Error::HostKeyNotVerified`). Alasannya: `russh 0.63.3` tidak memanggil `check_server_key` bila pertukaran kunci selesai tanpa kunci host (`client/kex.rs:158-176` KEX `none`; `client/mod.rs:1893-1902` hanya memanggil handler bila ada sertifikat atau kunci). Daftar bawaan tidak memuat `none` (`negotiation.rs:162-176`), tetapi keamanan jangan bergantung pada bawaan crate lain.
- **`Handler::kex_done` menolak `kex::NONE`**, dan `Config.preferred` dibangun dari `Preferred::DEFAULT` dengan **hanya** daftar `key` yang diurutkan ulang. Tes: `preferred.kex` tidak memuat `NONE`; `preferred.host_key_certificates` kosong.
- **Urutan algoritma host key mengikuti kunci yang tercatat.** Sebelum connect, `known_hosts::recorded_key_types(files, host, port)` membaca jenis kunci tercatat untuk host itu, dan algoritma yang cocok ditaruh di depan `preferred.key` (`ssh-ed25519` ke Ed25519; `ecdsa-sha2-nistp*` ke ECDSA yang sesuai; `ssh-rsa` ke ketiga varian RSA). Tanpa ini, server ber-Ed25519 dengan catatan RSA menghasilkan `Mismatch` palsu (§1.3). Dengan ini, kunci yang berbeda (jenis apa pun) tetap `Mismatch` keras, dan server yang berhenti menawarkan jenis tercatat juga `Mismatch`, bukan "unknown", yang lebih ketat dari OpenSSH (yang menganggap jenis lain sebagai host baru). Itu disengaja (Q1 di §5.10). **Koreksi SEC (Q1):** aturan yang lebih ketat itu disetujui dengan syarat urutan algoritma ini terpasang. `recorded_key_types` hanya membaca baris **tanpa penanda** (baris `@revoked` dan `@cert-authority` diabaikan: bukan kunci host yang tercatat), **menyertakan entri berhash** (HMAC atas nama host, sama dengan `check_text`), dan jenis tercatat yang tidak bisa dinegosiasikan `russh` (mis. `sk-*` atau `ssh-dss`) tidak mengubah urutan tetapi tetap membuat host itu `Mismatch`, tidak pernah `Unknown` (satu baris biasa untuk host itu sudah cukup untuk `Mismatch`, `known_hosts.rs:203-205`).

### 5.6 Yang tidak pernah diterima otomatis

Daftar ini adalah kriteria penerimaan T2 dan masing-masing punya tes:

1. Host asing tanpa pin. Tidak ada `AcceptAnywhere`, tidak ada bendera "yes to all", dan bawaan `russh` (tolak) berlaku bila handler terlewat (`tunnel.rs:19-21`).
2. Host asing dengan pin yang tidak sama dengan yang disajikan.
3. Kunci yang berubah (`Mismatch`), dengan atau tanpa pin.
4. Kunci yang dicabut (`@revoked`), dengan atau tanpa pin, dan baris `@revoked` di bawah baris yang cocok (urutan baris tidak menentukan, `known_hosts.rs:198-202`).
5. Sertifikat host (`HostCertificateUnsupported`). `@cert-authority` bukan kunci host dan tidak pernah dicocokkan sebagai kunci. Host yang dicakup baris `@cert-authority` tetapi menyajikan kunci polos (`certificate_expected`, Q2) juga tidak pernah diterima, dengan atau tanpa pin: tanpa ini penyerang yang menyajikan kunci polos ke host yang dikelola CA mendapat jalur TOFU.
6. `known_hosts` yang tidak terbaca atau rusak.
7. Host asing saat pencatatan gagal.
8. Penerimaan dari saluran tak-interaktif: CLI tanpa `SSH_HOST_KEY_ACCEPT`, **seluruh MCP** (pemetaannya tidak pernah menghasilkan `SSH_HOST_KEY_ACCEPT`, dan ada tes yang membuktikannya, D-19), dan `warmUp`.
9. Direktif kepercayaan di `~/.ssh/config`: `StrictHostKeyChecking` bernilai `no`, `off`, atau `accept-new`, `UserKnownHostsFile`, `GlobalKnownHostsFile`, `HostKeyAlias`, dan `VerifyHostKeyDNS`. Tidak satu pun menjadi sumber kepercayaan, dan semuanya **ditolak dengan nama** (§6.1 butir 5), karena mengabaikannya diam-diam menyesatkan pengguna yang mengiranya berlaku. `StrictHostKeyChecking yes` dan `ask` dibiarkan (§6.1 butir 4): hasilnya paling lemah setara dengan perilaku kita.
10. Pin dari `~/.ssh/config`, dari `connections.json`, atau dari setelan yang disimpan. Pin hanya datang dari setelan satu-run.

### 5.7 Kunci berubah dan rotasi

- **Pesan `changed`** memuat: host dan port, fingerprint yang disajikan dan jenisnya, **setiap** catatan (fingerprint, jenis, berkas, nomor baris, dan apakah milik app atau milik pengguna), dan perintah yang bisa disalin: `ssh-keygen -R '[host]:port' -f '<berkas catatan>'` (host port 22 ditulis `host`). **Kedua nilai dikutip POSIX** (`'` di dalam nilai ditulis `'\''`) oleh satu fungsi `shell_quote` di `qh-tunnel`, dan host sudah divalidasi sebelumnya (§6.2 butir 5). Alasannya: host bisa datang dari berkas `.ncx` Navicat, `connections.json`, atau `~/.ssh/config`, dan jalur berkas diturunkan dari direktori rumah (nama pengguna bisa memuat `'`). Perintah ini ditempel pengguna ke terminal, jadi `SSH_Host` buatan yang memuat `'` tanpa ini menjadi injeksi shell. `ssh-keygen -R` bekerja karena berkas app tidak berhash; saya menjalankannya pada baris berformat §5.2 (bracket untuk port selain 22, polos untuk 22, dengan komentar akhir) dan ia menghapus entri yang benar serta meninggalkan salinan `known_hosts.old` di sebelahnya, yang perlu disebut di pesan. Tidak ada kata "accept", "trust", atau "continue".
- **Rotasi sah.** Administrator mengganti kunci dan memberi fingerprint baru lewat jalur terpisah. Pengguna menghapus catatan lama dengan perintah itu, menghubungkan lagi (sekarang `Unknown`), dan membandingkan fingerprint di sheet dengan yang diberikan administrator. Pin lama tidak berlaku untuk kunci baru.
- **Dua kunci sah berdampingan** (server punya Ed25519 dan RSA): catatan ditambah per jenis, dan urutan algoritma §5.5 menjaga agar jenis yang dinegosiasikan konsisten.
- **Tidak ada tombol "Forget host key" di app (D-24).** Jalur hapus di trust store akan berupa satu klik yang mengalahkan tujuan "tolak keras", tepat di sheet yang sedang menolak kunci yang berubah. Perintah di terminal memaksa langkah sadar, dan pengguna yang tidak nyaman dengannya sedang melihat peringatan yang benar.
- Bila catatan yang bentrok ada di `~/.ssh/known_hosts` pengguna, pesan menunjuk berkas itu dan perintah memakai `-f` ke berkas itu. App tidak menulis ke sana. Bila ada di `/etc/ssh/ssh_known_hosts`, pesan menunjuk berkas itu dan berkata bahwa hanya administrator yang mengubahnya; tidak ada perintah yang disalin untuknya. `RecordedKey.source` membedakan tiga asal: `app`, `user`, `system`.

### 5.8 Antarmuka engine dan app

`qh-tunnel` mendapat varian galat baru (D-13 menjelaskan pembawanya ke atas): `HostKeyPinMismatch`, `HostKeyRecordFailed`, `HostKeyNotVerified`, `HostKeyStoreUnsafe`, `HostKeyCertificateExpected` (Unknown dengan `ca_covered`, Q2). `RecordedKey` mendapat `source` (`app`, `user`, `system`). `BastionConfig` mendapat `known_hosts: Vec<PathBuf>` (dibaca: berkas pengguna, lalu `/etc/ssh/ssh_known_hosts`), `record_to: Option<PathBuf>`, dan `HostKeyPolicy::TrustFingerprint(String)` di samping `Strict` dan `TrustNew(ServerKey)` yang lama (perpustakaan tetap boleh memakai pola dua langkah). Di `qh-ffi/src/tunnel.rs`, galat itu dipetakan ke `EngineError::HostKey(Box<HostKeyFailure>)` (`qh-core`, data polos berisi string), yang `message()`-nya tetap teks yang bisa dibaca CLI.

**Klasifikasi retry.** `EngineError::HostKey` dan setiap varian galat host-key `qh-tunnel` (`HostKeyUnknown`, `HostKeyMismatch`, `HostKeyRevoked`, `HostCertificateUnsupported`, dan yang baru: `HostKeyPinMismatch`, `HostKeyRecordFailed`, `HostKeyNotVerified`, `HostKeyStoreUnsafe`, `HostKeyCertificateExpected`) harus `FailureKind::Permanent`: `retry` hanya mengulang `Transient` (`again`, `crates/qh-ffi/src/retry.rs:155-157`; `execute`, `:235`). Hari ini hasil itu datang dari pemetaan `Connect { kind: Permanent }` di `crates/qh-ffi/src/tunnel.rs:207-221`, dan varian baru **tidak mewarisinya**: `EngineError::failure_kind` (`crates/qh-core/src/error.rs:108-118`) mencocokkan tiap varian tanpa wildcard, jadi `HostKey` memaksa pilihan eksplisit, dan pilihannya `Permanent`. Satu tes di modul tes `retry.rs` (memakai `ScriptedSession`) memberi `execute` kegagalan `HostKey` di bawah `RetryPolicy` yang mengizinkan retry dan menegaskan tepat satu percobaan (`executes == 1`).

Objek `host_key` di event `error`, hanya bila `SSH_HOST_KEY_DETAIL=1`:

```json
{"event":"error","message":"...","host_key":{
  "state":"unknown",                       // unknown | changed | revoked | certificate | certificate_expected | pin_mismatch | record_failed | store_unsafe
  "host":"bastion.corp","port":22,"alias":"prod-bastion",
  "key_type":"ssh-ed25519","fingerprint":"SHA256:...",
  "app_known_hosts":"/Users/x/Library/Application Support/QueryHive/known_hosts",
  "ca_covered":false,
  "recorded":[{"fingerprint":"SHA256:...","key_type":"ssh-rsa","source":"user","path":"/Users/x/.ssh/known_hosts","line":3}],
  "pinned":null                            // diisi hanya pada pin_mismatch
}}
```

Kunci di kabel `snake_case` seperti event lain (`query_id`, `object_columns`), dan `EngineWire.decoder()` memakai `convertFromSnakeCase`, jadi propertinya di Swift `Event.hostKey: HostKeyDetail?` (ditambahkan T1, D-4). Komentar `//` di atas hanya untuk dibaca, bukan JSON. `recorded[].source` bernilai `app`, `user`, atau `system`.

### 5.9 Matriks tes

**Murni** (`crates/qh-tunnel/src/known_hosts.rs`, `tunnel.rs`, tanpa jaringan):

- `check_all`: tiga berkas (pengguna, sistem, app); cocok di berkas app dan beda di berkas pengguna menghasilkan Matched; `@revoked` di berkas pengguna atau berkas sistem mengalahkan cocok di berkas app; mismatch di satu berkas dan unknown di lain menghasilkan Mismatch; kunci yang berubah yang hanya tercatat di berkas sistem menghasilkan Mismatch dengan `source = system`, bukan Unknown; `RecordedKey.source` benar; berkas tak ada (sistem atau pengguna) = kosong; berkas rusak di salah satunya = galat yang menyebut berkas itu.
- `append_if_absent`: tidak menggandakan; menulis host huruf kecil (`Bastion.Corp` dan `bastion.corp` menjadi satu baris); komentar akhir tidak mengganggu pencocokan; tidak membuat berkas bila blob bukan kunci (sudah ada, `append_refuses_bytes_that_are_not_a_key`).
- Berkas aman (Q4), **di `check_all` maupun `append_if_absent`**: symlink, mode `0o666`, dan mode `0o620` ditolak (berkas app yang dapat ditulis grup tidak pernah menghasilkan `Matched`); pemilik yang bukan `euid` ditolak (fungsi menerima `euid` yang diinjeksi, jadi tes tidak butuh root); `0o600` dan `0o644` lolos; pemeriksaan memakai `fstat` atas deskriptor yang dibuka (tes: sesudah dibuka, mengganti jalur dengan symlink tidak mengubah hasil yang sudah dihitung).
- `recorded_key_types` (Q1): hanya baris tanpa penanda (`@revoked` dan `@cert-authority` diabaikan); menyertakan entri berhash; host yang hanya punya catatan `ssh-dss` atau `sk-ssh-ed25519@openssh.com` dan server yang menyajikan Ed25519 menghasilkan `Mismatch`, bukan `Unknown`, dan `Config.preferred.key` tidak berubah.
- `Unknown` dengan baris `@cert-authority` yang cocok (Q2): `HostKeyCertificateExpected` tanpa pin dan dengan pin yang benar; berkas app tidak terbuat.
- `shell_quote` dan validasi host (§5.7, §6.2 butir 5): nilai dengan `'`, spasi, `;`, `$(...)`, tanda petik balik, baris baru, dan awalan `-` dikutip sehingga `/bin/sh -c "printf %s <hasil>"` mengembalikan nilai asli; jalur app yang memuat `'` (mis. `/Users/O'Brien/...`) muncul di perintah `ssh-keygen -R` sebagai `'/Users/O'\''Brien/...'`; `tunnel::settings` menolak `SSH_HOST` (dan `HostName` hasil alias) yang memuat `'`, spasi, `;`, `/`, koma, `*`, baris baru, awalan `-`, atau panjang di atas 253 sebagai `Usage` sebelum jaringan disentuh, tanpa mengulang nilainya di pesan.
- Bentuk pin: `SHA256:` + 43 karakter base64 tanpa padding; selain itu `Usage`.
- `Config.preferred`: `kex` tidak memuat `NONE`; urutan `key` dengan catatan RSA menaruh RSA di depan; tanpa catatan sama dengan bawaan.
- Penjaga: `Tunnel::open` tidak memanggil `authenticate` bila bendera belum menyala (handler uji yang mengembalikan `Ok(true)` tanpa menyalakan bendera).

**Hidup** (`QH_TEST_SSH=1`, `qh-sshd-dev`, `crates/qh-tunnel/tests/sshd.rs` dan `crates/qh-ffi` untuk setelan; pola tes yang ada di `sshd.rs:179-420`):

| # | Kasus | Hasil yang diminta |
|---|---|---|
| L-1 | Host asing, tanpa pin | `HostKeyUnknown`; fingerprint sama dengan `ssh-keyscan -p 52222 127.0.0.1 \| ssh-keygen -lf -` (pembanding independen); **tidak ada byte autentikasi terkirim** (sudah dijaga tes yang ada) |
| L-2 | Pin benar | tunnel terbuka; berkas app terbuat `0600`, memuat tepat satu baris untuk `[127.0.0.1]:52222` dengan komentar; byte bisa dikirim bolak-balik |
| L-3 | Pin salah (fingerprint kunci lain) | `HostKeyPinMismatch`; berkas app **tidak terbuat** |
| L-4 | Sesudah L-2, tanpa pin | Matched, tidak ada penulisan |
| L-5 | Berkas app diganti dengan kunci lain untuk host yang sama, dengan dan tanpa pin | `Mismatch` keras kedua kali; pin tidak berpengaruh |
| L-6 | Entri `@revoked` untuk kunci server, dengan pin benar | `Revoked`; tidak diterima |
| L-7 | Pin tanpa `SSH_APP_KNOWN_HOSTS` | `Usage`, tanpa koneksi |
| L-8 | `SSH_APP_KNOWN_HOSTS` di direktori tak bisa ditulis, pin benar | `HostKeyRecordFailed`; tidak ada autentikasi |
| L-9 | Kunci pengguna ber-RSA di `~/.ssh/known_hosts`, server menawarkan Ed25519 dan RSA | urutan algoritma menegosiasikan RSA, Matched, tanpa alarm palsu |
| L-10 | CLI end-to-end: `SSH_HOST=127.0.0.1 SSH_PORT=52222 SSH_USER=qh SSH_AUTH_METHOD=key SSH_KEY_PATH=target/qh-sshd/id_ed25519 SSH_APP_KNOWN_HOSTS=<berkas uji> DB_KIND=postgres DB_HOST=<alamat database seperti terlihat dari dalam container sshd> DB_PORT=<port-nya> … queryhive-engine test` dengan dan tanpa `SSH_HOST_KEY_ACCEPT` (alamat yang dapat dijangkau dari container tidak saya verifikasi, §18) | gagal dengan fingerprint, lalu berhasil dengan pin |

**Bukti yang diminta SEC sebelum W11-T2 dimulai:** bacaan atas §5.2 sampai §5.6, jawaban atas Q1 sampai Q5 (§5.10), dan konfirmasi bahwa daftar §5.6 lengkap. Sesudah implementasi: verdict atas `tunnel.rs`, `known_hosts.rs`, `qh-ffi/src/tunnel.rs`, dan hasil L-1 sampai L-10.

### 5.10 Pertanyaan terbuka untuk SEC dan risiko yang tersisa

- **Q1 (ketat vs OpenSSH).** Kunci dengan jenis berbeda dari yang tercatat adalah `Mismatch` keras di sini, `HOST_NEW` di OpenSSH. Rancangan memilih lebih ketat dan menutup alarm palsu dengan urutan algoritma. Apakah SEC setuju, atau meminta perilaku OpenSSH untuk jenis yang tidak tercatat? **Jawaban SEC:** setuju dengan aturan yang lebih ketat (jenis kunci berbeda = `Mismatch` keras), dengan syarat urutan algoritma §5.5 terpasang. `recorded_key_types` hanya membaca baris tanpa penanda, menyertakan entri berhash, dan jenis yang tidak bisa dinegosiasikan `russh` (mis. `sk-*`, `ssh-dss`) tetap `Mismatch`, tidak pernah `Unknown`. Diterapkan di §5.5 dan §5.9.
- **Q2 (`ca_covered`).** Host yang dicakup `@cert-authority` tetapi menyajikan kunci biasa: rancangan semula mengizinkan TOFU dengan peringatan tambahan di sheet (kompatibel dengan OpenSSH). Alternatif: tolak keras karena konfigurasi pengguna sendiri menyatakan host itu seharusnya menyajikan sertifikat. **Jawaban SEC:** tolak, jangan tawarkan TOFU. Bila `ca_covered`, `known_hosts` pengguna sendiri menyatakan host itu seharusnya menyajikan sertifikat dan build ini tidak bisa memverifikasinya. Menawarkan TOFU membuka jalur penurunan bagi MITM yang menyajikan kunci polos ke host yang dikelola CA. Kata-katanya penolakan keras tanpa tombol terima, keadaan `certificate_expected` (§5.3, §5.4 butir 7, §5.8). Diterapkan.
- **Q3 (catat sebelum terima).** Rancangan mencatat sebelum autentikasi. Alternatif: catat sesudah autentikasi berhasil, yang menghindari kunci tercatat dari percobaan dengan kredensial salah tetapi membuat pengguna melihat prompt ulang setelah salah ketik password. **Jawaban SEC:** mencatat sebelum autentikasi sudah benar. Itu sama dengan OpenSSH, dan autentikasi tidak boleh pernah dikirim ke kunci yang belum tercatat. Biaya UX-nya ("kunci tercatat padahal koneksi gagal") ditutup pesan "host key trusted, but the connection failed" di §9.4. Tidak ada perubahan.
- **Q4 (berkas tidak aman).** §5.2 butir 4 menolak symlink dan mode yang dapat ditulis grup atau pihak lain. Apakah itu proporsional untuk berkas yang kita tulis sendiri? **Jawaban SEC:** proporsional, dengan cara tanpa TOCTOU: `O_NOFOLLOW`, `fstat` atas deskriptor yang terbuka (bukan `lstat` atas jalur), pemilik `== geteuid()`, `mode & 0o022 == 0`, dan pemeriksaan yang sama saat berkas app dibaca di `check_all`. Diterapkan di §5.2 butir 4.
- **Q5 (CLI skrip).** `SSH_HOST_KEY_ACCEPT` boleh dipakai skrip CLI. Pengguna bisa menghitungnya dari `ssh-keyscan`, yang mengalahkan tujuannya, tetapi itu pilihan pengguna, bukan penerimaan otomatis oleh program. Apakah perlu dibatasi ke app saja? **Jawaban SEC:** pertahankan pin CLI. Ia butuh nilai eksplisit per run dan bukan penerimaan otomatis. ADR-0039 mencatat bahwa menghitung pin dari `ssh-keyscan` mengalahkan tujuannya. Diterapkan di §4 dan §15.
- **Risiko yang tersisa:** TOFU pertama tanpa verifikasi fingerprint; satu catatan per nama (nama dan IP = dua prompt); tidak ada penguncian antar proses di berkas app; `HashKnownHosts` tidak dipakai untuk berkas app (nama host terbaca oleh siapa pun yang bisa membaca berkas `0600` itu, yaitu pengguna yang sama).
- **Vektor penurunan yang semula tidak tercantum, sekarang ditutup:** `/etc/ssh/ssh_known_hosts` (dipatok sistem, mis. oleh MDM di Mac korporat) semula tidak dibaca, sehingga host yang dipatok di sana tampak `Unknown` dan kunci yang berubah menjadi prompt TOFU, bukan penolakan keras. `check_all` sekarang membacanya (hanya baca, berkas yang tidak ada = kosong; §5.2, §5.3). Bila isinya tidak terparse itu galat yang menyebut berkas itu (konsisten dengan §5.3 butir 1), dan pengguna tidak bisa memperbaikinya sendiri (§16).
- **Rekey (risiko rendah, dicatat di ADR-0039):** `russh 0.63.3` memanggil `check_server_key` hanya pada key exchange awal (`client/mod.rs:1887-1902`, cabang "This is the initial kex"; cabang rekey `:1868-1886` tidak memanggilnya). Pada rekey, kunci host server tidak dibandingkan lagi dengan yang sudah diverifikasi. Hanya peer yang sudah terautentikasi yang bisa memicunya. Alternatif "patok blob terverifikasi di verifier lalu bandingkan di `kex_done`" tidak tersedia di versi ini: `Handler::kex_done(shared_secret, names, session)` tidak menerima kunci host (`client/mod.rs:2394-2401`). Diterima sebagai risiko tersisa; tinjau ulang bila `russh` mengeksposnya.

## 6. Alias `~/.ssh/config` (W11-T2)

### 6.1 Parser: `crates/qh-tunnel/src/ssh_config.rs` (baru, murni)

```rust
pub struct Resolved { pub host_name: String, pub user: Option<String>, pub port: Option<u16>,
                      pub identity_files: Vec<PathBuf> }
pub enum SshConfigError { NotFound { alias: String },
                          Unsupported { directive: String, file: PathBuf, line: usize },
                          UnsupportedToken { directive: String, token: String, file: PathBuf, line: usize },
                          TooManyIncludes, Malformed { file: PathBuf, line: usize },
                          Unreadable { path: PathBuf, source: std::io::Error } }
pub fn load(path: &Path) -> Result<SshConfig, SshConfigError>;      // follows Include
impl SshConfig { pub fn aliases(&self) -> Vec<String>;               // for the picker
                 pub fn resolve(&self, alias: &str) -> Result<Resolved, SshConfigError>; }
```

Aturan yang mengikat:

1. **Sintaks.** `Keyword value` atau `Keyword=value`; kutip ganda untuk nilai berspasi; komentar `#`; kata kunci tidak peka huruf besar. `Host` menerima beberapa pola dengan `*`, `?`, dan `!`. Pencocok pola memakai pencocok iteratif yang sudah ada di `known_hosts.rs:408-434` (dipindah ke modul bersama `pattern.rs`; tes eksponensialnya ikut).
2. **Semantik OpenSSH yang ditiru:** blok dievaluasi menurut urutan berkas; nilai pertama menang per kata kunci; `IdentityFile` menumpuk. Blok cocok bila ada pola positif yang cocok dan tidak ada pola negasi yang cocok.
3. **Didukung:** `HostName` (token `%h` dan `%%` saja), `User`, `Port`, `IdentityFile` (`~/` dikembangkan; token lain ditolak), dan `Include`. `Include`: relatif terhadap `~/.ssh`, `~` dikembangkan, glob `*` dan `?` hanya di komponen terakhir, kedalaman ≤ 4, total berkas ≤ 32 (`TooManyIncludes`), hasil glob terurut. Berkas yang tidak ada atau glob tanpa hasil dilewati (seperti OpenSSH); berkas yang ada tetapi tidak terbaca adalah galat. Pengguna Docker Desktop, OrbStack, dan Colima punya `Include` di config bawaannya, jadi ini tidak opsional.
4. **Kosmetik, diabaikan** (tidak bisa mengubah tujuan, kepercayaan, kredensial, atau algoritma): `AddKeysToAgent`, `UseKeychain`, `ServerAliveInterval`, `ServerAliveCountMax`, `TCPKeepAlive`, `Compression`, `ForwardAgent`, `ForwardX11`, `ForwardX11Trusted`, `LogLevel`, `SendEnv`, `SetEnv`, `HashKnownHosts`, `ControlMaster`, `ControlPath`, `ControlPersist`, `VisualHostKey`, `ConnectTimeout`, `ConnectionAttempts`, `BatchMode`, `NumberOfPasswordPrompts`, `AddressFamily`, `RequestTTY`, `Tag`, `IPQoS`, dan `StrictHostKeyChecking` dengan nilai `yes` atau `ask` (hasilnya paling lemah setara dengan perilaku kita). `IdentitiesOnly no` (bawaan OpenSSH, tidak mengubah apa pun) juga dibiarkan. Setiap tambahan ke daftar ini butuh alasan "tidak mengubah tujuan, kepercayaan, kredensial, atau algoritma" di PR-nya.
5. **Ditolak dengan nama (D-12)** — semua direktif lain di blok yang cocok, termasuk: `ProxyJump`, `ProxyCommand`, `ProxyUseFdpass`, `HostKeyAlias`, `UserKnownHostsFile`, `GlobalKnownHostsFile`, `CertificateFile`, `IdentityAgent`, `VerifyHostKeyDNS`, `CanonicalizeHostname`, `StrictHostKeyChecking` bernilai `no`, `off`, atau `accept-new`, `IdentitiesOnly` bernilai `yes`, `PreferredAuthentications` (nilai apa pun), `HostKeyAlgorithms`, `KexAlgorithms`, `Ciphers`, `MACs`, `PubkeyAcceptedAlgorithms`, `LocalForward`, `RemoteForward`, `DynamicForward`, dan `RemoteCommand`. **`Match` di berkas mana pun yang dimuat menolak resolusi**, karena kriterianya tidak dievaluasi dan bisa saja menambah `ProxyJump` untuk alias itu. Pesannya: `~/.ssh/config line N: "<direktif>" is not supported by QueryHive, so alias "<alias>" cannot be used. Fill in host, user and key explicitly instead.` Nama direktif dan nomor baris selalu ada. Nilainya tidak. **`IdentitiesOnly` dan `PreferredAuthentications` bukan kosmetik** (koreksi review): keduanya menentukan kredensial mana yang ditawarkan. Autentikasi agen mencoba **setiap** identitas yang dipegang agen berurutan (`authenticate_with_agent`, `crates/qh-tunnel/src/tunnel.rs:424` dan `:437`), jadi mengabaikan `IdentitiesOnly yes` menawarkan semua kunci agen ke bastion dan membocorkan identitas kunci di luar kehendak pengguna. Alternatif yang tidak diambil sekarang: menghormati `IdentitiesOnly` di jalur agen dengan menyaring identitas agen terhadap kunci publik `IdentityFile` hasil resolusi (butuh membaca `.pub` dan mencocokkan blob); layak bila penolakan ini ternyata terlalu mahal di praktik.
6. **Harga yang diketahui:** `Host *` yang memuat direktif di luar dua daftar membuat setiap alias tidak bisa dipakai (`IdentitiesOnly yes` termasuk). Itu lebih baik daripada menghubungi tujuan yang salah atau melemahkan kepercayaan diam-diam, dan jalan keluarnya satu langkah (isi kolom eksplisit). Daftar kosmetik adalah titik penyetelan, bukan pelonggaran otomatis.
7. **Batas:** berkas config ≤ 1 MiB per berkas, ≤ 20.000 baris total, `aliases()` ≤ 500 nama, pola tanpa wildcard dan tanpa `!`.

### 6.2 Pemakaian di jalur connect

`tunnel::settings` (`crates/qh-ffi/src/tunnel.rs:83-136`) membaca `SSH_USE_CONFIG`. Bila `1`:

1. `ssh_config::load(SSH_CONFIG_PATH atau ~/.ssh/config)` dan `resolve(SSH_HOST)`. Alias tidak ada → `NotFound` (bukan "pakai nama itu sebagai host", karena itu menyembunyikan salah ketik).
2. **Setelan eksplisit menang atas config:** `SSH_USER`, `SSH_PORT`, `SSH_KEY_PATH` yang tidak kosong mengganti nilai dari config. `SSH_AUTH_METHOD` **tidak pernah** diambil dari config (bawaan tetap `agent`). Bila metodenya `key` dan `SSH_KEY_PATH` kosong, `IdentityFile` pertama yang ada di disk dipakai; tidak ada → `KeyPathMissing`. Hanya satu `IdentityFile` yang dicoba, dan itu dikatakan di §6.4 (OpenSSH mencoba semuanya berurutan).
3. `TunnelConfig.host` adalah `HostName` hasil resolusi, jadi pool, `known_hosts`, dan prompt memakai nama yang sama. `TunnelConfig` mendapat `alias: Option<String>` untuk ditampilkan; ia tidak masuk identitas pool (host, port, dan user sudah di sana).
4. Tanpa `SSH_USE_CONFIG`, `SSH_HOST` diperlakukan literal seperti hari ini, jadi perilaku CLI dan MCP lama tidak berubah.
5. **Validasi nama host sebelum jaringan (dengan dan tanpa `SSH_USE_CONFIG`).** `tunnel::settings` memeriksa host yang akan dihubungi (`SSH_HOST`, atau `HostName` hasil resolusi alias) terhadap himpunan karakter nama host atau alamat IP: huruf ASCII, angka, `.`, `-`, `_`, dan `:` (literal IPv6), 1 sampai 253 karakter, tidak berawalan `-`. Selain itu `Usage` ("SSH_HOST is not a valid host name or address"), **tanpa mengulang nilainya** di pesan. Alasan: host itu dicetak ke dalam perintah `ssh-keygen -R` yang ditempel pengguna ke terminal (§5.7) dan ditulis ke berkas app (§5.2) sebagai kolom pertama baris `known_hosts`; sumbernya bisa berkas `.ncx` Navicat, `connections.json`, atau `~/.ssh/config`, jadi `SSH_Host` buatan yang memuat `'`, spasi, koma, `*`, atau baris baru adalah injeksi shell dan, di berkas app, injeksi kolom atau baris. Kutip POSIX di §5.7 tetap dipasang sebagai lapis kedua, karena jalur berkas dari direktori rumah juga bisa memuat `'`.

**Ekspor FFI (dua fungsi bebas di `uniffi_api.rs`, dimiliki T2, §13.1):**

```rust
#[uniffi::export] pub fn ssh_config_hosts() -> Vec<String>;
#[uniffi::export] pub fn ssh_config_resolve(alias: String) -> Result<SshResolution, SshConfigFfiError>;
```

Keduanya memakai kode §6.1 yang sama dengan jalur connect, jadi pratinjau di form dan hasil connect tidak bisa berbeda. `ssh_config_hosts` tidak pernah gagal karena `Match` (hanya mendaftar). `SshConfigFfiError` membawa pesan yang sama dengan §6.1 butir 5. Mengubah permukaan UniFFI berarti G-FFI, dan T2 masuk **lane FFI** di antara T1 dan T6 (§13.1).

### 6.3 Tes

- Murni: korpus config (blok `Host *` dengan kosmetik; alias dengan dan tanpa `HostName`; negasi `!`; `Include` relatif, glob, siklus, kedalaman 5; token `%h`, `%d`; `Match`; `ProxyJump`; `StrictHostKeyChecking no`; `IdentitiesOnly yes` dan `PreferredAuthentications` (ditolak) dengan `IdentitiesOnly no` (lolos); baris rusak). Setiap penolakan menyebut direktif dan nomor baris. Pencocok pola tidak eksponensial.
- Hidup: alias `qh-dev` (`HostName 127.0.0.1`, `Port 52222`, `User qh`, `IdentityFile target/qh-sshd/id_ed25519`) membuka tunnel ke `qh-sshd-dev` dengan `SSH_USE_CONFIG=1` dan hanya `SSH_HOST=qh-dev`.
- `ssh_config_hosts` tidak memuat pola `*` dan `!`.

### 6.4 Yang tidak dikerjakan

`Match`, `ProxyJump`, `ProxyCommand`, banyak `IdentityFile` berurutan, `CertificateFile`, `IdentityAgent` (agen 1Password dan Secretive memakainya; tertolak dengan nama sampai ada alasan memasangnya), `/etc/ssh/ssh_config`, dan `Include` di luar `~/.ssh` yang bergantung pada variabel lingkungan.

## 7. JWT Trino, CA per koneksi, dan nama TLS lewat tunnel (W11-T2)

### 7.1 JWT (FR-CON-07, D-14)

- `DB_JWT` dibaca di `config::build` dan hanya untuk Trino (`DB_KIND` lain adalah `Usage` yang menyebut kedua setelan). Tidak boleh bersama `DB_PASSWORD` atau sandi di `DB_URL`: keduanya terisi adalah `Usage` ("send one"). App mengirim hanya salah satunya menurut `dbAuth`.
- **Validasi bentuk tanpa mengecho nilai:** tidak kosong, ≤ 8.192 karakter, hanya karakter token68 (`A-Za-z0-9-._~+/=`). Selain itu `Usage` "DB_JWT is not a valid bearer token" tanpa nilai. Engine tidak mendekode atau memverifikasi JWT (token buram); kedaluwarsa muncul sebagai penolakan 401 dari koordinator, dan pesannya menyebut "the token may have expired".
- **TLS:** JWT dengan mode `Disable` (`DB_SCHEME=http` atau `DB_SSLMODE=disable`) atau `Prefer` adalah `Usage` yang menyebut mode itu, karena `Prefer` bisa turun ke `http` (§1.4). Tidak ada kenaikan diam-diam bila diminta eksplisit. Bila skema dan `sslmode` **tidak diisi**, mode dinaikkan ke `Require` dan port bawaan 443, memperluas aturan password yang sudah ada (`config.rs:205-208`, `:275-278`).
- **Driver:** `ConnectionConfig.bearer: Option<String>` (dengan `Debug` tersensor); `Credentials` di Trino mendapat `bearer`, dan header dipasang oleh satu fungsi (butir berikut) sebagai `Authorization: Bearer <token>` **sebagai ganti** Basic. `X-Trino-User` tetap dikirim. Versi pertama blueprint ini hanya memeriksa skema basis sesi (header dipasang bila basis `https`). Itu **tidak memenuhi FR-CON-07** ("`Authorization: Bearer` hanya lewat HTTPS"), karena driver tidak hanya mengirim ke basis sesi: dua dari tiga URL yang menerima header diberikan server (§1.4). Pemeriksaan skema basis tetap ada, sebagai bagian dari pemeriksaan origin di bawah dan sebagai pertahanan berlapis di belakang penolakan di `config`.
- **Origin kredensial per permintaan (koreksi review, memblokir).** Rahasia (bearer, dan password Basic) hanya boleh dikirim ke satu origin, yaitu origin basis sesi:
  - **(a) Pemeriksaan per permintaan.** `Credentials` membawa `origin: Option<Origin>` (`Origin { scheme, host, port }`; skema dan host huruf kecil; port eksplisit atau bawaan skema; diparse dengan `reqwest::Url`), diisi di `connect` dari basis sesi (dihitung ulang bila fallback `Prefer` mengubah skema basis, `lib.rs:734`, yang hanya mungkin tanpa bearer). `with_shared_headers(request, credentials)` (`:581-592`) diganti oleh `authorized(client, method, url, &credentials) -> Result<RequestBuilder, EngineError>`: ia memparse `url`, membandingkan skema, host, dan port dengan `credentials.origin`, dan bila beda mengembalikan galat bernama **sebelum header apa pun dipasang dan sebelum permintaan dibangun**. Tiga pemanggil hari ini memakainya: POST (`send_post`, `:694-703`), GET `nextUri` (`fetch_page`, `:1097-1110`), dan DELETE cancel (`delete_running`, `:640-645`). Rahasia tanpa origin gagal tertutup (header tidak dipasang, galat bernama), jadi tidak ada pemanggil yang bisa membawa rahasia melewatinya. `TrinoCursor` memegang salinan `Credentials` sendiri (`:1071-1073`), jadi origin ikut ke kursor tanpa argumen tambahan.
  - **Galatnya bernama dan `Permanent`.** POST: `EngineError::Connect`; GET dan DELETE: `EngineError::Query` dengan `code: None`; semuanya `FailureKind::Permanent`, sehingga `retry` tidak mengulangnya (`again` hanya mengulang `Transient`, `crates/qh-ffi/src/retry.rs:155-157`). Pesan menyebut kedua origin (`skema://host:port`, **tanpa path dan tanpa nilai kredensial**) dan bahwa kredensial tidak dikirim, mis. `the coordinator sent a page URL on http://trino.corp:8080, not on https://trino.corp:443 (the address this connection uses), so the credentials were not sent. Check the coordinator's external address and its forwarded-header handling.` Cancel yang ditolak tidak mengirim apa pun dan tidak jatuh ke URL turunan basis (`{base}/v1/statement/{id}`, `:640-642`): URL yang sah untuk query itu hanya yang diberikan server, dan `reset` sudah mengabaikan kegagalan DELETE (komentarnya: koordinator meninggalkan query itu sendiri).
  - **Lingkup.** Pemeriksaan berlaku untuk setiap permintaan yang membawa rahasia, **password Basic termasuk** (fungsi yang sama; akibatnya di §16). Sesi tanpa rahasia (tanpa password dan tanpa bearer) tidak diperiksa: tidak ada yang bocor, dan koneksi tanpa autentikasi yang bergantung pada `nextUri` berhost lain hari ini bekerja.
  - **(b) Tanpa redirect untuk klien bearer.** `client_for(tls, roots)` (publik, dipakai `tests/tls.rs:383,582`) tidak berubah. Ditambah `client_for_secret(tls, roots)` yang membangun klien yang sama dengan `redirect(reqwest::redirect::Policy::none())` lewat satu pembangun privat bersama, dan `connect` memakainya bila `config.bearer.is_some()`. Alasan: reqwest 0.12.28 membuang `Authorization` pada redirect hanya bila host atau port berubah (`redirect.rs:239-251`), tidak melihat skema, dan `https_only` hanya menolak skema bukan-https tanpa melihat origin (`:323-328`). Respons 3xx lalu jatuh ke cabang non-sukses yang sudah ada (`lib.rs:758-773`) sebagai `Query` `Permanent` berkode status. Klien Basic tidak diubah di W11 (keputusan pemilik, §16).
  - **(c) Tes dengan koordinator palsu** (`crates/qh-driver-trino/tests/credential_origin.rs`, baru; berjalan di `cargo test --workspace`, tanpa podman). Pola dan fixture dari `tests/tls.rs` (`rcgen`, `tokio-rustls`, `TcpListener`), tetapi penadahnya menyimpan **kepala permintaan utuh**, bukan hanya baris permintaan seperti `record` di sana. Kasus 1 sampai 3 dijalankan dua kali, dengan token bearer sentinel dan dengan password Basic sentinel (pemeriksaan origin berlaku untuk keduanya); kasus 4 untuk bearer (butir (b)). Semuanya menegaskan satu hal: **tidak ada permintaan di listener plaintext atau listener asing yang membawa header `Authorization`**, dan nilai sentinel tidak muncul di kepala mana pun selain di listener sesi.
    1. `an_http_next_uri_is_refused_before_it_is_requested`: koordinator TLS (basis `https://127.0.0.1:A`) menjawab POST dengan `nextUri` `http://127.0.0.1:B/...`, B listener plaintext. Hasil: galat bernama yang menyebut kedua origin, `Permanent`; B tidak menerima koneksi sama sekali.
    2. `a_next_uri_on_another_host_or_port_is_refused`: dua varian, host lain (`localhost` dan `127.0.0.1` adalah dua host bagi pemeriksa) dan host sama dengan port lain, keduanya TLS. B nol permintaan.
    3. `cancel_and_reset_refuse_a_foreign_next_uri`: POST dijawab dengan `nextUri` asing (`running_from` menyimpannya saat POST, `:834-838`, jadi cancel memakainya), lalu `cancel()` gagal bernama dan `reset()` tidak mengirim apa pun. B nol permintaan.
    4. `an_https_to_http_redirect_on_the_same_host_and_port_is_not_followed`: **satu** listener pada satu port yang mengintip byte pertama (`0x16`, rekaman handshake TLS: layani TLS; selain itu: layani HTTP polos). Port yang sama itulah yang membuat redirect lolos dari pembuangan header reqwest (host dan port sama). POST via TLS dijawab `307` dengan `Location: http://127.0.0.1:<port yang sama>/v1/statement`. Hasil: galat `Permanent` dari cabang non-sukses; listener mencatat tepat satu permintaan (TLS) dan nol permintaan plaintext. Untuk Basic, kasus ini menunggu keputusan pemilik di §16: bila `client_for_secret` disetujui juga untuk Basic, kasus yang sama dijalankan untuk keduanya.
    5. Kontrol (membuktikan fake bisa melihat kebocoran, pola "celah dibuktikan lebih dulu" seperti §7.3): klien `reqwest` polos dengan kebijakan redirect bawaan dan header `Authorization` melawan fake yang sama **menghasilkan** permintaan plaintext ber-`Authorization`. Tanpa kontrol ini, asersi "nol" di atas bisa lulus karena fake-nya buta.
    6. Murni, di `lib.rs`: `Origin::of` (host huruf besar sama; `https://h` sama dengan `https://h:443`; `http://h:443` beda dari `https://h:443`; `[::1]`; URL yang tidak terparse adalah galat), rahasia tanpa origin gagal tertutup, dan sesi tanpa rahasia tidak diperiksa (perilaku hari ini tidak berubah).
    7. Struktural: `include_str!("lib.rs")` dihitung, `basic_auth`, `bearer_auth`, dan literal `Authorization` hanya muncul di `authorized`. Itu menjaga janji "satu fungsi" (modul doc `lib.rs:43`) tetap benar bila ada jalur permintaan baru.
- **Pool:** JWT masuk hash kredensial (NFR-S2). `Debug` `ConnectionConfig`, `Credentials`, dan `Settings` tidak mencetaknya (tes `format!("{:?}")` dengan token sentinel pada ketiganya).
- **Keychain:** `jwt:<UUID>` (§8). Tidak pernah ke `connections.json`, log, galat, event, atau keluaran MCP.

### 7.2 CA per koneksi (FR-CON-08, D-15, P-08)

- `DB_CA_FILE` dibaca di `config::build`: `~` dikembangkan; berkas ≤ 1 MiB; PEM diparse dengan `rustls_pki_types::pem::PemObject` (sudah ada di `Cargo.lock` sebagai `rustls-pki-types 1.15.1`; `rustls-pemfile` tidak dipakai di kode produksi, ia dev-dependency PostgreSQL saja dan sudah tidak dirawat); 1 sampai 64 sertifikat; **berkas yang memuat blok `PRIVATE KEY` ditolak** (pengguna menunjuk berkas yang salah, dan kunci privat tidak boleh dibaca program ini); sertifikat yang tidak valid sebagai X.509 ditolak satu per satu (bukan `add_parsable_certificates` yang membuang diam-diam). Hasilnya DER yang dipegang `ConnectionConfig.tls_ca: Option<TlsCa { path, certs: Arc<Vec<Vec<u8>>> }>`: **byte dibaca sekali**, jadi tidak ada celah waktu antara validasi dan connect, dan hash DER ikut kunci pool (jalur berkas di identitas, isi di hash kredensial).
- **Prasyarat, ditolak dengan nama bila tidak terpenuhi:** driver PostgreSQL atau Trino; mode efektif `Require` (`sslmode=verify-ca` atau `verify-full`; Trino `https` tanpa `DB_INSECURE`); bukan `Disable`, `Prefer`, `RequireNoVerify`, atau `DB_INSECURE=1`. Mengabaikan CA di mode yang tidak memverifikasi akan membuat pengguna mengira kepercayaannya sudah dipatok.
- **Efek:** kepercayaan menjadi **bundel itu saja** (FR-CON-08), tanpa trust store sistem. Nama host tetap diperiksa terhadap SAN (`WebPkiServerVerifier` di PostgreSQL lewat `tls::verifier_with_roots`, `crates/qh-driver-postgres/src/tls.rs:70-83`; `with_root_certificates` di Trino lewat `client_for(tls, Some(roots))`, atau `client_for_secret(tls, Some(roots))` bila ada bearer (§7.1 (b)), `crates/qh-driver-trino/src/lib.rs:272-279`). Untuk host berupa IP, sertifikat harus punya SAN IP, dan form menyebutnya.
- **Tidak ada TOFU untuk sertifikat TLS.** Tidak ada prompt "trust this certificate" dan tidak ada pin sertifikat server. Kepercayaan TLS hanya datang dari trust store sistem (bawaan), dari berkas CA yang **dipilih pengguna** untuk koneksi itu (FR-CON-08), atau dari `RequireNoVerify` yang dipilih eksplisit dan diberi peringatan. Alur "ambil rantai server, tampilkan fingerprint, pin" memungkinkan, tetapi bukan permintaan PRD dan memperluas permukaan kepercayaan, jadi tidak dikerjakan di W11.
- **MySQL:** `DB_CA_FILE` adalah `Usage`: "DB_CA_FILE is not supported for MySQL: mysql_async 0.36 cannot take a CA (docs/tls-modes.md)". Form tidak menawarkan kolomnya, dan batasnya dijelaskan di teks bantuan (PRD §12.1).
- **Tes TLS dengan CA uji** (gate T2): untuk PostgreSQL dan Trino, sertifikat server yang ditandatangani CA uji (fixture yang ada di `crates/qh-driver-postgres/tests/tls/` dan `deploy/dev/qh-trino-tls.sh`) diterima dengan `DB_CA_FILE`; ditolak dengan CA lain; ditolak bila nama tidak cocok; CA kosong, bukan PEM, memuat kunci privat, dan 65 sertifikat ditolak sebelum jaringan; `prefer` dan `require` dan `DB_INSECURE=1` dengan CA ditolak dengan nama; MySQL dengan CA ditolak.

### 7.3 Nama TLS lewat tunnel SSH (D-16)

Hari ini `retarget` menimpa `config.host` dengan `127.0.0.1` (`crates/qh-ffi/src/tunnel.rs:158-163`) dan driver memakainya sebagai nama TLS. Dibaca di kode, **belum dicoba live** (§18): mode `Require` lewat tunnel memverifikasi sertifikat terhadap `127.0.0.1`.

- `ConnectionConfig.tls_server_name: Option<String>`; `retarget` mengisinya dengan host database aslinya. `PoolKey::of` mengecualikannya secara eksplisit (ia diturunkan dari host yang sudah ada di identitas).
- **PostgreSQL:** `pg.host(&nama).hostaddr(127.0.0.1).port(port_lokal)`. `tokio-postgres 0.7.18` memakai `host` sebagai nama TLS dan `hostaddr` sebagai alamat TCP (`config.rs:134-144,409-414`; `connect.rs:62-76,160`).
- **Trino:** URL dasar `https://<nama>:<port_lokal>` dengan `reqwest::ClientBuilder::resolve(nama, 127.0.0.1:0)`. `reqwest 0.12.28` memakai port dari URL, bukan dari alamat yang di-resolve (`async_impl/client.rs:2266-2272`). Header `Host` yang terkirim memuat `<nama>:<port_lokal>`; dampaknya pada koordinator di balik proxy **tidak saya verifikasi** (§18). **`resolve` memetakan setiap URL berhost `<nama>` ke loopback pada port yang tertulis di URL**, jadi `nextUri` yang membawa port lain dari koordinator (mis. port TLS aslinya, bukan `<port_lokal>`) akan menghubungi `127.0.0.1:<port lain>`, yaitu layanan lokal yang bukan tunnel, dengan kredensial di kepala. Pemeriksaan origin §7.1 menolaknya sebelum header dipasang, dengan galat bernama yang menyebut kedua port, bukan menghubungi layanan itu. Untuk sesi tanpa rahasia perilakunya tidak berubah (§16).
- **MySQL:** lewat tunnel dengan `Require` ditolak di `config` dengan `Usage`: "MySQL cannot verify a server certificate through an SSH tunnel (the name checked would be 127.0.0.1). Use require (encrypted, not verified: the tunnel already authenticates the bastion), or connect directly." Hari ini kegagalannya berupa kesalahan handshake yang tidak menjelaskan apa-apa. Ini perbaikan kecil di luar baris rencana dan boleh dicoret pemilik.
- Tes: PostgreSQL dan Trino dengan server TLS uji di belakang proxy TCP loopback yang berperan sebagai tunnel; sertifikat bernama `db.internal`; koneksi lewat `127.0.0.1:<port proxy>` berhasil dengan `DB_CA_FILE`, dan gagal tanpa `tls_server_name` (membuktikan celahnya ada). Satu kasus tambahan untuk Trino: koordinator uji yang menjawab `nextUri` dengan port aslinya (bukan port proxy) menghasilkan galat origin bernama, dan proxy loopback kedua tidak menerima koneksi.

## 8. Item Keychain (W11-T3, sisi Rust dan Swift)

| Item | Akun | Isi | Dibuat bila | Siapa membaca |
|---|---|---|---|---|
| Password database (**ada**) | `<UUID>` | password | form disimpan dengan password terisi | app, MCP, CLI `credential` |
| Password SSH | `ssh-password:<UUID>` | password bastion | `sshAuth == password` dan field terisi | app, MCP |
| Passphrase kunci SSH | `ssh-passphrase:<UUID>` | passphrase `IdentityFile` | `sshAuth == key` dan field terisi | app, MCP |
| JWT Trino | `jwt:<UUID>` | token bearer | `dbAuth == jwt` dan field terisi | app, MCP |

Layanan tetap `id.data-ecosystem.queryhive`; UUID **huruf besar** (`id.uuidString` di Swift). Awalan tidak bisa dikira UUID: akun berawalan dilipat bagian UUID-nya sendiri, seperti `account_for_profile` (`crates/qh-credentials/src/lib.rs:75-77`).

**Rust (`qh-credentials`):**

```rust
pub enum Slot { Database, SshPassword, SshPassphrase, Jwt }
pub fn account_for(slot: Slot, connection_id: &str) -> String;   // Database => account_key(id), unchanged
pub fn delete_all(store: &dyn SecretStore, connection_id: &str) -> Result<(), CredentialError>;
```

Tidak ada perintah `credential` baru: app membaca dan menulis item lewat `ConnectionKeychain` Swift, dan MCP lewat `account_for` + `KeychainStore`. `credential` CLI tetap hanya untuk password database.

**Swift (`ConnectionKeychain`):** `enum Slot { case database, sshPassword, sshPassphrase, jwt }` dan `set/get/getWithoutPrompt/delete(slot:for:)`. Pemanggil lama (`for:`) tetap berarti `.database`.

**Siklus hidup:**

1. **Simpan:** field kosong berarti **tetap** (PF-5, diamandemen 2026-10-06): formulir tidak bisa menampilkan rahasia yang tersimpan, jadi kosong hanya bisa berarti "tidak diketik ulang", dan menghapus item karenanya akan menghapus rahasia pengguna tanpa ia memintanya. Field berisi menimpa. Penghapusan hanya terjadi lewat tombol "Remove saved…" yang eksplisit (ditahan sampai Simpan) atau pergantian pemakaian: mengganti `sshAuth` atau `dbAuth`, mematikan tunnel, atau mengganti driver menghapus item yang tidak lagi dipakai, supaya rahasia yang tidak terpakai tidak menggantung. Tesnya: Edit lalu Simpan dengan field kosong membiarkan keempat slot utuh (`ConnectionFormIssuesTests`).
2. **Hapus koneksi:** keempat slot dihapus (`AppModel.swift`, `deleteConnection`; sekarang hanya password).
3. **Duplikat:** menyalin slot yang ada, seperti password hari ini (`duplicate`).
4. **Impor Navicat:** menulis `ssh-password` dan `ssh-passphrase` bila berhasil didekripsi (§9.5); kegagalan Keychain masuk `keychainFailures` yang sudah ada.
5. **Baca untuk run, tree, dan Test:** semua slot yang relevan bagi `dbAuth` dan `sshAuth` koneksi itu; nilai yang diketik di editor menang atas yang tersimpan (pola `credential.isEmpty ? storedPassword : credential` di Test Connection). `warmUp` memakai `getWithoutPrompt` untuk setiap slot dan **melewati warm-up** bila salah satu butuh prompt.
6. **Tidak disimpan di Keychain:** pin host key (satu-run), kunci host (publik, ada di `known_hosts`), isi CA (jalurnya ada di koneksi dan isinya dibaca saat connect), dan token MCP (sudah di SQLite sebagai hash).

**ACL:** item ditulis app (ad-hoc signed) dan dibaca MCP (biner lain), jadi pembacaan pertama MCP memunculkan prompt Keychain. Ini sudah terjadi untuk password database, dan tidak berubah. Label (`kSecAttrLabel`) tidak diisi; Keychain Access menampilkan nama layanan. Bukan tujuan W11.

**Tes kontrak:** literal `ssh-password:E621E1F8-C36C-495A-93FC-0C247A3E6E5F` (dan dua lainnya) diperiksa di `qh-credentials` (Rust) dan `ConnectionKeychainTests` (Swift): kedua sisi harus menghasilkan string persis sama, dan akun UUID huruf kecil dari Rust jatuh ke akun yang sama dengan Swift. `QH_TEST_KEYCHAIN=1`: tulis, `contains`, baca, dan hapus ketiga akun, serta bukti ketiganya tidak menyentuh akun `<UUID>` maupun `profile:*`.

## 9. App: koneksi, form, prompt host key, Navicat (W11-T3)

### 9.1 Model dan pemetaan ke setelan

`Connection` (`Models/Connections.swift`, struct `Connection`) mendapat field **datar**, semuanya `decodeIfPresent` dengan bawaan yang sama dengan perilaku sebelumnya (D-18), sehingga `connections.json` lama dekode tanpa perubahan dan tidak dipindahkan sebagai rusak:

| Field | Tipe | Bawaan | Setelan engine |
|---|---|---|---|
| `sshHost` | String | `""` | `SSH_HOST` (nama host, atau alias bila `sshUseConfig`) |
| `sshUseConfig` | Bool | `false` | `SSH_USE_CONFIG=1` |
| `sshPort` | Int | `0` (tidak diisi) | `SSH_PORT` bila bukan 0 |
| `sshUser` | String | `""` | `SSH_USER` bila tidak kosong |
| `sshAuth` | `agent`, `key`, `password` | `agent` | `SSH_AUTH_METHOD` |
| `sshKeyPath` | String | `""` | `SSH_KEY_PATH` |
| `dbAuth` | `password`, `jwt` (Trino saja) | `password` | `jwt`: `DB_JWT` dan `DB_PASSWORD` dikosongkan |
| `caFile` | String | `""` | `DB_CA_FILE` |

Rahasia tidak masuk struct: password SSH, passphrase, dan JWT hanya di Keychain (§8). `AppModel.connectionEnvironment(kind:host:…)` menerima struct `SSHSettings` dan `ConnectionSecrets { password, sshPassword, sshPassphrase, jwt }` (dengan `description` tersensor) menggantikan parameter `password`; tiga pemanggilnya (instance, statis `(_:password:)`, dan Test Connection di `ConnectionEditorSheet`) ikut. Bila `sshHost` tidak kosong, env selalu memuat `SSH_APP_KNOWN_HOSTS` (`<ConnectionStore.directory()>/known_hosts`). `SSH_HOST_KEY_DETAIL=1` **tidak** dipasang di sini: `HostKeyGate` (§9.4) menambahkannya ke setiap run yang menyebut `SSH_HOST`, jadi satu tempat saja dan pemetaan murni app tetap sama persis dengan sisi MCP (fixture bersama). **Tidak pernah** `SSH_HOST_KEY_ACCEPT`: itu hanya dipasang `HostKeyGate` untuk satu run (§9.4).

Pemilihan CA mengikat mode TLS di sisi app: bila `caFile` terisi, picker `sslmode` PostgreSQL dibatasi ke `verify-ca` dan `verify-full` (bawaan `verify-full`), dan Trino memaksa transport `https` dengan verifikasi menyala. Engine tetap menolak kombinasi yang salah (§7.2). Form hanya mencegah pengguna sampai ke penolakan itu.

### 9.2 Form

`ConnectionEditorSheet` (`Views/ConnectionsViews.swift`) mendapat tiga bagian. Visual dan tata letaknya milik spesifikasi UX dan AX (V-11, scene baru); yang mengikat di sini adalah perilaku:

1. **SSH tunnel** (kontrol aktif/nonaktif; mati menghapus nilai `ssh*` pada Simpan tetapi tidak menyentuh Keychain sampai Simpan): picker alias dari `ssh_config_hosts()` plus kolom bebas (memilih alias mengisi `sshHost` dan menyalakan `sshUseConfig`; mengetik host sendiri mematikannya); pratinjau hasil `ssh_config_resolve(alias)` read-only ("→ bastion.corp:22 sebagai deploy, kunci ~/.ssh/id_ed25519") dan galatnya (direktif yang tidak didukung) tampil di bawah picker dengan teks dari engine; port; user; metode (agent/key/password); jalur kunci (panel buka berkas); passphrase atau password (`SecureField`). Bagian ini menyebut satu kalimat: "The first time you connect, QueryHive shows the server's fingerprint and asks you to verify it."
2. **Autentikasi Trino:** kontrol Password / JWT. JWT adalah `SecureField` yang menerima tempel token panjang. Teks bantuan: "JWT is sent only over HTTPS."
3. **CA (PostgreSQL dan Trino):** baris "CA certificate" dengan Choose… dan Clear, teks bantuan "The server certificate is checked against this file only, and its name must match. For an IP address the certificate needs an IP SAN." Untuk MySQL baris itu tidak ada, dan satu kalimat menjelaskan batasnya (PRD §12.1).

### 9.3 Validasi bernama dan ⌘↩ (FR-CON-09)

- Fungsi murni `ConnectionFormIssues.missing(for: FormState, resolved: SSHResolution?) -> [String]` mengembalikan nama field yang kurang, dan `ConnectionFormIssues.message(_:)` merangkainya: `"Host and User are required."`, `"Host, Database and SSH user are required."`. Aturan: aturan wajib yang sudah ada di editor ditambah `SSH host` bila tunnel menyala, `SSH user` bila tidak ada di form **dan** tidak disediakan alias, `SSH key file` bila metode `key` (tanpa `IdentityFile` dari alias), `CA file` yang jalurnya tidak ada, dan port di luar 1…65535. Test Connection dengan masalah menampilkan pesan itu, tidak menjalankan apa pun.
- Test Connection pindah dari ⌘T ke ⌘↩ (`.keyboardShortcut(.return, modifiers: .command)` di tombol yang sekarang memakai `"t"`). Ia hanya berlaku di cakupan sheet, sehingga tidak bentrok dengan ⌘↩ di editor (Run). Terdaftar di peta pintasan W9-T2. **Perhatian untuk W9-T2 dan T3:** `blueprints/w9-shell-and-a11y.md` §4.4 mencatat ⌘T lembar koneksi sebagai satu-satunya `knownConflicts` yang ditutup W11-T3, tetapi tabel kunci di sana mengikat `run` ke ⌘↩ pada skema DBeaver, jadi ⌘↩ lembar koneksi akan terbaca sebagai pasangan kembar dalam gabungan uji. T3 tidak boleh menambah pengecualian baru: lembar koneksi adalah lembar modal (jendela kunci, kunci menu tidak sampai), dan `ShortcutConflictTests` harus memodelkan lembar modal sebagai cakupan sendiri. Itu perubahan kecil di tes milik W9-T2, dikerjakan T3 bersama pemetaan kuncinya.

### 9.4 `HostKeyGate`: satu titik untuk prompt (D-20)

Ada belasan pemanggil `Engine.current.run` yang bisa membuka tunnel (run, preview, count, explain, tree, objects, `table_op`, `import_data`, `apply_changes`, Test). Menambal prompt di masing-masing tidak masuk akal. Rancangannya:

- `HostKeyGate: DatabaseEngine` membungkus `RustEngine` dan menjadi nilai `Engine.current` (`Support/DatabaseEngine.swift`, composition root; `SilentEngine` untuk snapshot tidak berubah). Ia meneruskan semua event, dan untuk run yang envnya memuat `SSH_HOST` ia menambahkan `SSH_HOST_KEY_DETAIL=1` supaya pemanggil tidak perlu mengingatnya.
- **Protokol `DatabaseEngine` punya tujuh requirement** (`Support/DatabaseEngine.swift`, protokol `DatabaseEngine`: `run`, `terminateAll`, `runBlocking`, `runIntoStore`, `storeFromRows`, `makeResultStore`, `warmUp`), dan `HostKeyGate` mengimplementasikan **semuanya secara eksplisit**:
  - `run(_:env:onEvent:onExit:)`: meneruskan, menambah `SSH_HOST_KEY_DETAIL=1`, dan mengamati event `error` ber-`hostKey`.
  - `runIntoStore(_:env:store:onEvent:onExit:)`: sama dengan `run`. **Preview dan explain berjalan lewat sini** (dokumen protokolnya: "Runs `preview` or `explain` with its rows going into `store`"), jadi gate yang hanya mengamati `run` membuat operasi yang paling sering membuka tunnel tidak pernah memunculkan prompt.
  - `terminateAll()`: meneruskan.
  - `runBlocking(_:env:)`: meneruskan. Ia tidak punya kanal event (dipakai menyimpan sesi saat terminasi), jadi tidak ada yang diamati.
  - `makeResultStore()` dan `storeFromRows(columns:rows:)`: meneruskan.
  - `warmUp(env:)`: meneruskan **secara eksplisit** ke engine yang dibungkus. Protokol memberinya implementasi bawaan kosong di `extension DatabaseEngine`; gate yang tidak mengimplementasikannya memakai bawaan itu dan **mematikan warm-up untuk seluruh app** tanpa galat apa pun. Warm-up tidak melewati `HostKeyCenter` (§5.4 butir 5).
- Event `error` dengan `hostKey` dari `run` **dan** `runIntoStore` diteruskan ke `HostKeyCenter` (`@Observable`, `@MainActor`), tanpa mengubah event: operasi yang gagal tetap menampilkan galatnya (node pohon bergalat, log tab), dan teksnya sudah menyebut bahwa host key belum dipercaya.
- Keadaan `unknown` memunculkan `HostKeySheet` (§5.4) **satu kali per (host, port, fingerprint)**. Keadaan `changed`, `revoked`, `certificate`, `certificate_expected`, `pin_mismatch`, `record_failed`, dan `store_unsafe` memunculkan sheet informasi tanpa tombol terima, dengan perintah §5.7 yang bisa disalin.
- **Trust** menjalankan `test` dengan env dari run yang gagal ditambah `SSH_HOST_KEY_ACCEPT=<fingerprint dari event itu>`. Env itu disimpan **di memori saja** selama sheet hidup (maksimal 5 menit, lalu dibuang) karena memuat rahasia. Hasil: sukses menyiarkan `hostKeyTrusted(host, port)`, yang membuat node pohon bergalat memuat ulang dan Test Connection di editor menjalankan ulang dirinya. Operasi lain tidak diulang otomatis (percaya adalah tindakan terpisah dari mencoba ulang). Gagal setelah kunci tercatat melaporkan "host key trusted, but the connection failed: …": kunci sudah tercatat dan itu benar.
- `warmUp` tidak melalui `HostKeyCenter`: galatnya ditelan seperti sekarang (§5.4 butir 5), dan gate meneruskannya secara eksplisit ke engine yang dibungkus (butir tujuh requirement di atas).
- Gate tidak punya metode yang menerima fingerprint dari mana pun selain event yang sedang dibahas: `trust(_ prompt: HostKeyPrompt)` mengambil `prompt` yang dibangun gate sendiri dari event, bukan string.

### 9.5 Impor Navicat (FR-CON-04, D-23)

`ImportedConnection` (`Support/NavicatImport.swift`) mendapat `sshPort`, `sshUser`, `sshAuth`, `sshPassword`, `sshKeyPath`, dan `sshPassphrase` di samping `sshHost`. Pemetaan ke `Connection` dan Keychain (`ssh-password`, `ssh-passphrase`) mengikuti pola password (`NavicatImport.decrypt`, kegagalan masuk `keychainFailures`). Ringkasan impor (`importSummary`) tidak lagi berkata entri bertunnel "tidak akan tersambung"; ia berkata SSH diimpor dan host key akan diminta diverifikasi saat pertama kali. **Nama atribut `.ncx` selain `SSH_Host` tidak terverifikasi** (§18): sebelum menulis kode, implementer memeriksanya terhadap ekspor nyata (pemilik punya satu, komentar `NavicatImport.swift` menyebut 40 entri). Atribut yang tidak ditemukan dipetakan sebagai "tidak diimpor" dan disebut di ringkasan, bukan dibuang.

### 9.6 Tes (Swift)

`ConnectionsDecodeTests`: berkas lama tanpa kunci baru dekode dengan bawaan, dan encode-decode bolak-balik; `ConnectionEnvironmentTests` (dibuat W9-T4 untuk tag lingkungan; T3 menambah kasus): pemetaan SSH, JWT, dan CA per jenis dan metode, `SSH_HOST_KEY_ACCEPT` tidak pernah muncul dari `connectionEnvironment`, `ConnectionSecrets` tidak tercetak; **tes paritas** yang membaca `crates/qh-ffi/tests/fixtures/connection_env.json` (jalur dari `#filePath`; bila dilarang paket, fixture diduplikasi dengan tes kesamaan byte) dan membandingkan keluaran `connectionEnvironment` dengan `mcp::connection_environment` untuk kasus yang sama; `HostKeyGateTests` (dengan `MockEngine`): `unknown` melapor, Trust mengirim `test` dengan pin sama dengan fingerprint event, event untuk fingerprint lain tidak bisa di-trust dengan prompt lama, `changed` dan lainnya tidak punya jalur terima, koalesi dua event sama, `warmUp` tidak melapor dan **sampai ke engine yang dibungkus** (double uji `RecordingEngine` di dalam `HostKeyGateTests.swift` yang mencatat `warmUp(env:)`; `MockEngine` tidak mengimplementasikannya dan memakai bawaan kosong dari extension, jadi tidak bisa membuktikannya), `runIntoStore` dengan event `error` ber-`hostKey` melapor seperti `run` (preview dan explain), ketujuh requirement protokol diteruskan, env dibuang sesudah dismiss; `ConnectionFormIssuesTests`; `NavicatImportTests`; `ConnectionKeychainTests` (§8). Scene V-11 baru: `connection-ssh`, `host-key-unknown`, `host-key-changed`.

## 10. MCP: pemetaan dan dua tool baru (W11-T3 pemetaan, W11-T5 tool)

### 10.1 Pemetaan `ConnectionRecord` ke `Settings` (FR-CON-05)

- **Penulis:** `crates/qh-storage/src/import.rs::options_json` (`:421-446`) menambah `sshHost`, `sshPort`, `sshUser`, `sshAuth`, `sshKeyPath`, `sshUseConfig`, `caFile`, `dbAuth` dengan nama yang sama dengan `connections.json` (D-18), dengan bawaan yang sama dengan Swift. Berkas ini **tidak ada** di daftar T3 dan harus ditambahkan (§13.1).
- **Pembaca:** `mcp::connection_environment` (`mcp.rs:438-524`) menerima `&Secrets { password, ssh_password, ssh_passphrase, jwt }` (Debug tersensor) menggantikan `password: &str`, dan menghasilkan `SSH_*`, `DB_CA_FILE`, dan `DB_JWT` dengan aturan yang sama dengan sisi app. Selalu memasang `SSH_APP_KNOWN_HOSTS` (`qh_storage::import::legacy_directory()/known_hosts`) bila ada `SSH_HOST`. **Tidak pernah** `SSH_HOST_KEY_ACCEPT` dan tidak `SSH_HOST_KEY_DETAIL`.
- `settings_for` (`mcp.rs:1277-1305`) membaca hanya slot yang dibutuhkan opsi koneksi (`sshAuth`, `dbAuth`) lewat `account_for`, dan galat Keychain disebut dengan nama koneksi dan kata-kata Keychain, **tidak** nilainya (pola `password_for`, `:1341-1355`).
- Host asing lewat MCP: ditolak dengan pesan yang memuat fingerprint dan petunjuk "open QueryHive, connect once, and verify the fingerprint". Tidak ada prompt dan tidak ada jalur terima. Bila pengguna sudah menerimanya di app, berkas app dibaca MCP dan koneksi jalan.
- **Celah yang sudah ada dan tidak diperbaiki W11:** store SQLite hanya diisi oleh CLI `import_connections` sekali per sumber (`already_imported` menjadi penanda, `import.rs:233-258`), dan app tidak pernah memanggilnya (§1.6). Koneksi yang dibuat atau diubah di app **sesudah** impor itu tidak terlihat MCP. Pemetaan murni di atas membuat field SSH, CA, dan JWT ikut mengalir saat celah itu ditutup. Menutupnya (app memanggil `import_connections` sesudah menyimpan, dengan semantik penggantian) adalah keputusan terpisah untuk pemilik.
- **Tes murni** (`crates/qh-ffi/tests/mcp.rs`, pola `the_trino_transports_map_the_way_the_app_maps_them`, `:777`): peta lengkap per jenis dan metode; `SSH_HOST_KEY_ACCEPT` dan `SSH_HOST_KEY_DETAIL` tidak pernah ada untuk kombinasi opsi apa pun (diulang atas semua kombinasi); `SSH_APP_KNOWN_HOSTS` ada bila `SSH_HOST` ada; `Secrets` tidak tercetak; fixture bersama dengan Swift.

### 10.2 Tool `describe_table` dan `table_ddl` (FR-MCP-01)

- Dua `ToolSpec` baru (`TOOLS` menjadi 11 elemen), skema `columns_schema` (`connection`, `table`, `catalog?`, `schema?`). Dispatcher memakai `resolve` (allowlist **sebelum** pembacaan store) dan `settings_for` dengan tambahan `TARGET_CATALOG`, `TARGET_SCHEMA`, `TARGET_TABLE` yang diturunkan dengan aturan slot `sql_ident::slots` (PostgreSQL: schema dan table; MySQL: database di `TARGET_CATALOG` dan table; Trino: ketiganya), lalu `Command::Columns` atau `Command::Ddl`. Keluarannya event engine apa adanya.
- `SAFE_MODE=read_only` tetap dipaksa oleh `settings_for` (`mcp.rs:1297-1300`), apa pun Safe Mode koneksi. Perintah metadata tidak membawa SQL pengguna, jadi tidak ada yang perlu ditolak guard, tetapi setiap statement-nya diuji `ReadOnly` (§3.7) dan sesinya dipaksa read-only oleh `enforce_read_only`, jadi tidak ada tulis yang bisa lewat.
- Tool `columns` lama **tetap** (kebijakan `docs/mcp-stability.md`) dan tetap `LIMIT 0`. `describe_table` adalah yang lebih kaya (nullable, default, extra). Deskripsi `columns` boleh diperjelas, tidak diubah artinya.
- Scope: token baru yang diterbitkan tanpa `--scope` otomatis memuat kedua tool (`read_only_tool_names()`); token lama tidak (gagal tertutup). **Catatan untuk SEC:** definisi view bisa mengungkap logika bisnis walau `read_only`. Pengguna yang tidak menginginkannya mempersempit `--scope`. ADR-0038 menyebutnya, dan SEC memutuskan apakah `table_ddl` perlu dikeluarkan dari scope bawaan.
- Penyensoran DDL (§3.6) terjadi di engine, jadi keluaran MCP ikut tersensor dan hal itu diuji di `mcp.rs`.
- `docs/mcp-stability.md` diperbarui: bagian "Tool metadata (W11)" (dua tool aditif; `columns` tetap), dan kalimat pembuka yang menyebut "tool kesepuluh" dibetulkan.
- **Tes** (`crates/qh-ffi/tests/mcp.rs`, `mcp_stdio.rs`): allowlist kosong menolak keduanya tanpa menyebut apakah koneksi ada; koneksi di luar allowlist ditolak seperti `columns`; token tanpa scope ditolak dengan pesan; `read_only` terjaga (statement tercatat `ReadOnly`); daftar `tools/list` mencakup 11 tool untuk token penuh dan difilter untuk token sempit; keluaran tidak memuat password, JWT, atau `secret_ref`.

## 11. Pohon, kolom, dan tab DDL (W11-T4)

- **Jenis node.** `TreeNode.Kind` (`Models/SchemaTree.swift`) mendapat `view`, `materializedView`, `foreignTable`, dan `column`. Empat jenis pertama yang mirip tabel (table, view, materializedView, foreignTable) adalah **tingkat tabel** untuk keperluan jalur: `node(atPath:)` dan `ConnectionKind.levels` tidak berubah, jadi `SuggestionScopeTests` dan resolusi `hive.analytics.` tetap bekerja. `isExpandable` benar untuk jenis mirip-tabel dan salah untuk `column`. `insertableText` hari ini mengembalikan nil kecuali `kind == .table` (`SchemaTree.swift:128-131`) dan harus diperlebar ke jenis mirip-tabel, bila tidak view tidak bisa di-drag atau di-klik ganda ke editor.
- **Label.** Setiap baris jenis selain tabel menampilkan labelnya sebagai teks (bukan ikon saja; NFR-A): `view`, `materialized view`, `foreign table`. Kolom menampilkan nama dan tipe, dan `NOT NULL` bila tidak nullable. Label AX mengikuti konvensi W9-T3 (`accessibilityLabel` = judul, `accessibilityValue` = jenis): nilai "view", "materialized view", atau "foreign table" untuk objek, dan "<tipe>, not null" untuk kolom. Spesifikasi UX dan AX mengisi ikon dan tata letak.
- **Memuat.** Pemuatan anak tabel di `loadChildren(of:)` (`AppModel.swift`, kelak `AppModel+Tree.swift`) mengirim `OBJECT_KINDS=1` dan membaca `kinds`; jenis tak dikenal jatuh ke `table`. Membuka node mirip-tabel menjalankan `columns` dengan `TARGET_*` dari node (`database`, `schema`, `title`) dan memuat anak `column`. Hasil **disimpan di `TreeNode.children`**, satu-satunya cache (D-22). Galat tampil di node (`node.error`) seperti level lain, dan `HostKeyGate` menangani prompt.
- **Tab DDL.** `QueryTab` belum punya konsep jenis tab (`Kind` di `QueryTab.swift` hanya milik `LogLine`). T4 menambah `QueryTab.ddlSource: DDLSource?` (koneksi, tiga slot target, jenis objek). Tab dengan `ddlSource`: editor tidak bisa diedit (`isEditable = false`), Run, Explain, dan ⌘↩ nonaktif dan diumumkan sebagai tidak tersedia, judul "DDL · schema.table", penyorot editor memakai dialek koneksi, dan ada aksi "Copy" serta "Open as query" (membuat tab biasa dengan salinan teks; bukan tab yang sama). Teks diambil lewat `ddl`; bila `redacted` atau `truncated`, satu baris catatan tampil di atasnya. **Tab DDL tidak dipulihkan** saat restore (`SessionTab` tidak memuatnya; D-21): ia adalah tampilan keadaan hidup dan bukan teks pengguna, jadi tidak ada risiko kehilangan teks.
- **Akses (UC-02, ≤ 2 klik).** Menu konteks node mirip-tabel: "Show DDL" (klik kanan lalu satu item), ditambahkan ke `TreeMenu.items(for:in:)` di `Models/TreeMenu.swift` (dibuat W9-T3) dan diuji di `TreeMenuTests`. Klik ganda tetap preview. Tidak ada pintasan baru kecuali yang dijatah peta W9-T2.
- **Tes.** `TreeNodeStalenessTests` (node baru ikut aturan basi), `SidebarRenderTests` (label), `SuggestionScopeTests` tidak berubah dan lulus (bukti tingkat tabel), `MockEngine` dengan `kinds` dan `fields`, tab DDL tidak bisa diedit dan tidak masuk `Session`; scene V-11 baru `tree-kinds` dan `ddl-tab`.

## 12. API referensi tabel dan alias SQL (W11-T6)

### 12.1 Rust: `crates/qh-editor/src/refs.rs` (baru)

Kontrak sudah disetujui di blueprint 4B §8.3 dan D-14. Tidak ada kode di `crates/qh-sql/src/editor/` untuk digantikan (§1: direktori itu tidak ada), jadi baris rencana T6 "menggantikan `crates/qh-sql/src/editor/`" dibaca sebagai "tidak ada yang digantikan".

```rust
pub enum RefSource { Tree, Lexical }
pub enum RelationKind { Table, Subquery, Cte }
pub struct Relation { pub kind: RelationKind,
    pub catalog: Option<String>, pub schema: Option<String>, pub name: Option<String>,   // name is None for a subquery
    pub quoted: bool,                    // true when the last part was written in quotes
    pub alias: Option<String>,
    pub name_start_utf16: u32, pub name_len_utf16: u32 }
pub struct Cte { pub name: String, pub columns: Vec<String> }  // columns from the declared list, if any
pub struct References { pub revision: u64, pub statement_start_utf16: u32, pub statement_len_utf16: u32,
                         pub relations: Vec<Relation>, pub ctes: Vec<Cte>, pub source: RefSource }
impl Document { pub fn references(&mut self, revision: u64, offset_utf16: u32) -> Result<References, EditError>; }
```

- **Cakupan statement** dari `qh_sql::walk` (statement di bawah `offset`), bukan dari pohon (invarian 4B). Statement di luar batas statement (offset di spasi di antara dua statement) memakai statement sebelumnya bila `offset` berada di ekornya, atau mengembalikan daftar kosong.
- **Dari pohon:** `relation` dengan `object_reference` (`database`, `schema`, `name`) dan `alias`; `cte` dengan nama dan `argument`; subquery dalam `FROM` ber-alias menjadi `RelationKind::Subquery`. **Cadangan leksikal** bila statement `has_error()`: pola `FROM|JOIN|UPDATE|INTO|TABLE nama[.nama[.nama]] [AS] alias` di atas `qh_sql::lex`, dan `source = Lexical`. Cadangan tidak melihat subquery.
- **Ambigu itu kosong.** Alias yang dideklarasikan dua kali dalam satu statement (subquery berbeda) menghasilkan dua entri; pemakai (Swift) tidak menyarankan kolom untuk alias yang ambigu. Aturan di sini: **lebih baik tidak ada saran daripada saran yang salah**, sama dengan komentar di `AppModel.suggestions` ("a path that resolves to nothing offers nothing").
- **Tanpa kerja per ketikan.** Dihitung saat diminta, di antrean latar yang sama dengan `outline`; tidak membaca atau menulis `paint`. `references` memakai pohon statement yang sudah ada di cache dan tidak memicu parse baru kecuali statement itu belum punya pohon.
- **Batas:** ≤ 64 relasi per statement; statement > `GIANT_STATEMENT_BYTES` memakai cadangan leksikal pada jendela sekitar `offset` saja.

### 12.2 FFI

`crates/qh-ffi/src/editor.rs` menambah `EditorReferences`, `EditorRelation`, `EditorCte`, `EditorRefSource` (`uniffi::Record`/`Enum`) dan `EditorDocument::references(revision, offset_utf16) -> Result<EditorReferences, EditorError>` yang menolak revisi basi seperti `outline` (`EditorError` yang sama). Permukaan UniFFI berubah, jadi `app/Generated/` ikut di-commit dan G-FFI berlaku. T6 adalah lane FFI terakhir di W11.

### 12.3 Alur di Swift

- `Support/EditorAnalysis.swift` (milik W4-T2 → W10-T6 → W10-T7) mendapat satu aksesor tipis `references(at:)`; ia tidak ada di baris rencana T6 dan harus ditambahkan (§13.1).
- `suggestions(for:prefix:path:)` (`AppModel+Completion.swift`) menerima **snapshot terakhir** `References` untuk tab itu dan revisinya. Ketikan tidak pernah menunggu analisis: bila snapshot tidak sama dengan revisi dokumen, saran dihitung dari snapshot lama yang masih valid atau tanpa kolom, dan permintaan `references` dikirim ke latar. Saat jawabannya tiba, daftar saran dihitung ulang pada ketikan berikutnya (tawar-menawar yang sama dengan `offerChildren`). Snapshot ini adalah **hasil analisis yang terikat revisi dokumen**, bukan cache metadata (D-22); ia dibuang bila revisi berubah.
- **Resolusi:** untuk `a.` dengan `a` bukan nama schema atau catalog di pohon, cari `Relation` ber-alias `a` (tidak peka huruf besar; kutip memaksa cocok persis lebih dulu). Bagian nama (`catalog`, `schema`, `name`) dipetakan ke jalur pohon menurut `ConnectionKind.levels` seperti `node(atPath:)`; nama tanpa kualifikasi memakai database dan schema bawaan tab (`database(for:)`, `schema(for:)`). Bila node tabel belum ada karena induknya belum dimuat, rantai pemuatan (`loadChildren` per induk, maksimal tiga tingkat) dijalankan di latar. Bila node ada dan `children == nil`, `columns` dikirim lewat sesi metadata (lane `Metadata`), dan hasilnya mengisi `TreeNode.children` (§11). **Tidak ada cache kedua.**
- **Hanya tabel, view, dan CTE bercolumn.** Subquery ber-alias dan CTE tanpa daftar kolom tidak menghasilkan saran kolom (v1), dan itu terdokumentasi di tes. Kata telanjang (tanpa `a.`) menawarkan kolom dari relasi di statement yang kolomnya **sudah dimuat**, dan memicu pemuatan untuk maksimal delapan relasi yang belum.
- Saran kolom memakai `SQLSuggestion.Kind.column` yang sudah ada dan tipe sebagai teks tambahan (tidak mengubah teks yang disisipkan).

### 12.4 Tes

- `crates/qh-editor/tests/refs.rs` dan `tests/fixtures/refs/*.sql` dengan `*.refs` golden (`QH_BLESS=1` menulis ulang, pola G1): PostgreSQL, MySQL, dan Trino; nama berkutip dan berkualifikasi; alias dengan dan tanpa `AS`; `JOIN` berantai; CTE dengan dan tanpa daftar kolom; `UPDATE`, `INSERT INTO`, `DELETE FROM`; subquery ber-alias; statement ber-ERROR yang jatuh ke cadangan.
- **Diferensial:** pada statement tanpa ERROR, hasil cadangan leksikal untuk pola sederhana sama dengan hasil pohon. Acak berbenih (SplitMix64 seperti G2): `references` tidak panik, tidak pernah mengembalikan rentang di luar statement, dan menolak revisi basi.
- Swift `SuggestionScopeTests` (baru: alias, ambigu, kutip, kolom belum dimuat memicu satu `columns`, tidak ada saran dari alias subquery, revisi basi tidak dipakai); tes `EditorAnalysisTests` untuk aksesor.
- **G-BENCHQ (`type-10k`) tidak boleh mundur lebih dari 5%** (NFR-P9), dan tidak ada pemanggilan `references` di jalur ketikan: bench `type-10k` dijalankan dengan completion menyala.

## 13. Perubahan per berkas

### 13.1 Koreksi kepemilikan terhadap `development-plan.md` (D-25)

Baris rencana W11 dan matriks kepemilikan §7 ditulis sebelum jalur panggil dibaca. Penambahan di bawah **harus** masuk ke baris tugas dan §7, dan semuanya ditemukan karena kode yang disebut di §1 memaksanya (kompilasi gagal, atau fungsi mati diam-diam).

| Tugas | Berkas yang ditambahkan | Mengapa |
|---|---|---|
| T1 | `crates/qh-driver/src/metadata.rs` (baru); `crates/qh-driver-{postgres,mysql,trino}/src/metadata.rs` (baru) | D-1: SQL dan parser murni di crate driver |
| T1 | `crates/qh-ffi/src/host.rs` | `route()` tanpa wildcard; pemasangan sink execution log di `EngineHost` (D-8) |
| T1 | `app/Sources/QueryHive/App.swift` (hanya struct `Event`, aditif) dan `Tests/QueryHiveTests/EventDecodingTests.swift` | D-4: semua kunci event baru ditambahkan sekali di sini, termasuk `hostKey` yang dipakai T3 |
| T1 | `tools/golden/live_cases.py`, `docs/golden-deltas.md`, `crates/qh-ffi/tests/host.rs` | kasus `_live` baru; selisih terklasifikasi; tes sink |
| T2 | `crates/qh-tunnel/src/{error.rs,tunnel.rs,pattern.rs (baru)}`, `crates/qh-tunnel/tests/{sshd.rs,ssh_config.rs (baru)}` | protokol §5 dan alias §6 mengubah galat, handler, dan pencocok pola |
| T2 | `crates/qh-core/src/error.rs`, `crates/qh-ffi/src/{lib.rs,main.rs,uniffi_api.rs,env.rs}`, `crates/qh-ffi/src/host/pool.rs` | D-13 (varian galat, `report`, `fail`), `Debug` `Settings`, dan `PoolKey::of` yang sengaja gagal dikompilasi (§1.3) |
| T2 | `crates/qh-driver/src/{tunnel.rs,ca.rs (baru)}`, `Cargo.toml` tiga crate driver dan `crates/qh-tunnel/Cargo.toml`, dan `Cargo.lock` (`rustls-pki-types` jadi dependensi langsung driver, `rustix` jadi dependensi langsung `qh-tunnel`) | `TunnelConfig`, `TlsCa`, parse PEM; G-DENY berlaku |
| T2 | `docs/tls-modes.md` | CA per koneksi dan nama TLS lewat tunnel |
| T2 | `crates/qh-driver-trino/tests/credential_origin.rs` (baru), `crates/qh-ffi/src/retry.rs` (hanya modul tes) | tes origin kredensial dan redirect dengan koordinator palsu (§7.1 (c)); tes bahwa kegagalan host-key tidak pernah diulang (§5.8). `crates/qh-driver-trino/src/lib.rs` sudah milik T2, tetapi isinya bertambah: pemeriksaan origin dan `client_for_secret`. |
| T2 | **masuk lane FFI** di antara T1 dan T6 (`uniffi_api.rs`, `app/Generated/`) dan gate **G-FFI** | dua ekspor `ssh_config_*` (§6.2). T2 berjalan di batch 2 sesudah T1 dan sebelum T6 (batch 3), jadi tidak berebut. |
| T3 | `crates/qh-credentials/src/lib.rs`, `crates/qh-storage/src/import.rs`, `crates/qh-ffi/tests/fixtures/connection_env.json` (baru) | `Slot` dan `account_for`; `options_json`; fixture paritas |
| T3 | `Support/{DatabaseEngine.swift,HostKeyGate.swift (baru)}`, `Models/{HostKeyCenter.swift (baru),ConnectionFormIssues.swift (baru),Shortcuts.swift}`, `Models/AppModel+Run.swift` bila di sanalah `warmUp` dan `connectionEnvironment` berada setelah W9-T0, `Support/Snapshot.swift`, `Tests/.../{ShortcutConflictTests,VisualParityTests}.swift` dan baseline scene baru | §9.4, §9.3, scene V-11 |
| T4 | `Views/SQLEditor.swift` (mode baca-saja), `Views/Workspace.swift`, `Models/Session.swift` (tab DDL tidak dipulihkan), `Models/TreeMenu.swift` dan `Tests/.../{TreeMenuTests,SchemaOutlineTests}.swift` (dari W9-T3), `Support/Snapshot.swift` | §11 |
| T6 | `crates/qh-editor/src/{lib.rs,paint.rs}` (modul `refs` dan `Document::references`), `crates/qh-editor/tests/refs.rs` dan fixture, `Support/EditorAnalysis.swift`, `Tests/.../SuggestionScopeTests.swift` | §12 |

Perubahan di §7 yang diusulkan: rantai lane FFI menjadi `… → W10-T7 → W11-T1 → W11-T2 → W11-T6 → W12-T1 …`; `App.swift` mendapat W11-T1 (aditif, `Event` saja) di antara W10-T3 dan W12-T2 (kunci `hostKey` sudah termasuk, jadi T3 tidak menyentuhnya lagi); `Support/DatabaseEngine.swift` dan `Support/Snapshot.swift` masing-masing mendapat baris W11. `Cargo.lock` mendapat W11-T2.

### 13.2 W11-T1 (lane FFI, batch 1)

| Berkas | Perubahan |
|---|---|
| `crates/qh-driver/src/{lib.rs,metadata.rs}` | `Driver::metadata()` bawaan `None`; tipe dan trait §3.1 |
| `crates/qh-driver-postgres/src/metadata.rs` | `PostgresMetadata`: SQL §3.2, `<visible>`, parser, `PostgresDdl` (resep dan rakitan) |
| `crates/qh-driver-mysql/src/metadata.rs` | `MysqlMetadata`: `SHOW FULL TABLES`, `COLUMNS`, `SHOW CREATE TABLE` |
| `crates/qh-driver-trino/src/metadata.rs` | `TrinoMetadata`: `information_schema`, resep dua langkah, pengecualian MV |
| `crates/qh-ffi/src/metadata.rs` | perintah `columns`, `ddl`, `execution_log`; `run_text`; penyensoran §3.6 |
| `crates/qh-ffi/src/{lib.rs,uniffi_api.rs,host.rs}`, `app/.../RustEngine.swift` | empat daftar invariant 11 (29 perintah), `route()`, pemasangan sink |
| `crates/qh-ffi/src/commands.rs` | `tables` membaca `OBJECT_KINDS` dan menambah `kinds` |
| `app/Generated/`, `App.swift` | regenerasi; kunci `Event` baru |
| `crates/qh-ffi/tests/{safe_mode,golden,host}.rs`, `tools/golden/live_cases.py`, `docs/golden-deltas.md` | §3.7 |

### 13.3 W11-T2 (Rust, lane FFI, batch 2)

| Berkas | Perubahan |
|---|---|
| `crates/qh-tunnel/src/known_hosts.rs` | `check_all` (tiga berkas: pengguna, `/etc/ssh/ssh_known_hosts`, app), `append_if_absent` (**melipat host ke huruf kecil sendiri**: `known_hosts_line`, `key.rs:59-66`, menulis `host_spelling(host)` apa adanya, sedangkan pencocokan sudah melipat, `known_hosts.rs:349-353`, jadi ini kosmetik dan hanya menjaga baris tertulis dan pemeriksaan idempoten konsisten), `recorded_key_types`, `open_app_store` (`O_NOFOLLOW` dan `fstat`, §5.2 butir 4; dipakai `check_all` dan `append_if_absent`), `shell_quote`, `RecordedKey.source` (`app`, `user`, `system`), komentar akhir baris |
| `crates/qh-tunnel/src/tunnel.rs` | `HostKeyPolicy::TrustFingerprint`, `BastionConfig.{known_hosts: Vec, record_to}`, penjaga terverifikasi, `kex_done`, `Config.preferred` terurut, `HostKeyCertificateExpected` untuk Unknown dengan `ca_covered` |
| `crates/qh-tunnel/src/{error.rs,ssh_config.rs,pattern.rs,lib.rs}` | varian galat baru (`HostKeyPinMismatch`, `HostKeyRecordFailed`, `HostKeyNotVerified`, `HostKeyStoreUnsafe`, `HostKeyCertificateExpected`); parser §6.1 (`IdentitiesOnly yes` dan `PreferredAuthentications` ditolak); pola bersama |
| `crates/qh-tunnel/Cargo.toml` | `rustix` (fitur `fs` dan `process`) sebagai dependensi langsung untuk `geteuid` dan `O_NOFOLLOW` tanpa `unsafe` (§5.2 butir 4); G-DENY |
| `crates/qh-driver/src/{lib.rs,tunnel.rs,ca.rs}` | `ConnectionConfig.{tls_ca, bearer, tls_server_name}` dengan `Debug` tersensor; `TunnelConfig.{app_known_hosts, host_key_accept, alias}`; parse PEM |
| `crates/qh-driver-postgres/src/{lib.rs,tls.rs}` | CA lewat `verifier_with_roots`; `host`+`hostaddr` |
| `crates/qh-driver-trino/src/lib.rs` | `client_for(config.tls, roots)` tetap; `client_for_secret` (tanpa redirect) untuk bearer; `Credentials.{bearer, origin}`; `authorized` menggantikan `with_shared_headers` dan memeriksa origin untuk POST, `nextUri`, dan cancel (§7.1); `resolve` |
| `crates/qh-driver-trino/tests/credential_origin.rs` (baru) | koordinator palsu: `nextUri` `http://`, `nextUri` host atau port lain, cancel ke `nextUri` asing, redirect `https` ke `http` pada port yang sama, dan tes kontrol (§7.1 (c)) |
| `crates/qh-core/src/error.rs` | `EngineError::HostKey(Box<HostKeyFailure>)`; `failure_kind()` memetakannya ke `Permanent` secara eksplisit (§5.8) |
| `crates/qh-ffi/src/{tunnel.rs,config.rs,events.rs,lib.rs,main.rs,uniffi_api.rs,env.rs}`, `host/pool.rs` | setelan baru §4; validasi host (§6.2 butir 5); daftar berkas kepercayaan (pengguna atau `SSH_KNOWN_HOSTS`, lalu `/etc/ssh/ssh_known_hosts`) dan pemetaan galat host-key ke `EngineError::HostKey`; `describe()` dan pesan; `events.rs` memuat pembangun JSON `host_key` yang dipakai `report` dan `fail`; ekspor `ssh_config_*`; `Debug` `Settings`; `PoolKey::of` dan `TunnelIdentity` (di bawah) |
| `crates/qh-ffi/src/retry.rs` (hanya modul tes) | satu tes: kegagalan `HostKey` tidak pernah diulang (§5.8) |
| Tes | §5.9, §6.3, §7.1 (c), §7.2, §7.3; `crates/qh-ffi/tests/host.rs` untuk kunci pool (di bawah) |

**Identitas pool (T2).** `PoolKey::of` membongkar `TunnelConfig` tanpa `..` (`crates/qh-ffi/src/host/pool.rs`, destructuring di `:168-174`), dan `TunnelIdentity` hari ini memuat `known_hosts: Option<PathBuf>` (`:109-114`, diisi di `:190-196`). Setelah W11 `TunnelIdentity` memuat **semua berkas kepercayaan yang ikut menentukan keputusan**, bukan satu: `known_hosts: Vec<PathBuf>` (berkas pengguna atau `SSH_KNOWN_HOSTS`, lalu `/etc/ssh/ssh_known_hosts`) **dan** `app_known_hosts: Option<PathBuf>` (`SSH_APP_KNOWN_HOSTS`), dalam urutan bacanya. Alasannya: tunnel yang diverifikasi terhadap satu himpunan berkas tidak boleh dipakai ulang oleh run yang menyebut himpunan lain, atau memilih berkas menjadi cara melewati verifikasi lewat pool. **Dikecualikan dengan sengaja:** pin `SSH_HOST_KEY_ACCEPT` (satu-run; tunnel yang sudah diverifikasi dan tercatat tetap sah sesudahnya), `alias` (turunan host), dan `tls_server_name`. Yang masuk identitas hanya jalur, bukan isi berkas: tunnel yang sudah terbuka tidak diverifikasi ulang bila berkasnya berubah. Tes `crates/qh-ffi/tests/host.rs`: dua konfigurasi yang beda hanya di berkas app, atau hanya di daftar berkas pengguna dan sistem, memberi `PoolKey` berbeda; beda hanya di pin memberi `PoolKey` sama; beda di JWT atau CA memberi `PoolKey` berbeda (hash kredensial).

### 13.4 W11-T3, T4, T5, T6

Lihat §9 sampai §12. Ringkas:

- **T3 (batch 3):** `ConnectionsViews.swift`, `HostKeySheet.swift` (baru), `Connections.swift`, `NavicatImport.swift`, `AppModel+Connections.swift` (+ `HostKeyGate`, `HostKeyCenter`, `ConnectionFormIssues`), `crates/qh-credentials`, `crates/qh-storage/src/import.rs`, `crates/qh-ffi/src/mcp.rs` (pemetaan) dan `tests/mcp.rs`, fixture paritas.
- **T4 (batch 2, sesudah T1):** `SchemaOutline.swift`, `SchemaTree.swift`, `AppModel+Tree.swift`, `QueryTab.swift`, `SQLEditor.swift`, `Workspace.swift`, `Session.swift`.
- **T5 (batch 4):** `mcp.rs`, `docs/mcp-stability.md`, `tests/{mcp,mcp_stdio}.rs`.
- **T6 (batch 3, lane FFI):** `qh-editor/src/refs.rs`, `qh-ffi/src/editor.rs`, `app/Generated/`, `EditorAnalysis.swift`, `SQLSuggestions.swift`, `AppModel+Completion.swift`.

## 14. Urutan, gate, dan commit

1. **Sebelum T2:** SEC membaca §5 dan menjawab Q1 sampai Q5 (§5.10). Jawabannya masuk ke bagian ini sebagai koreksi bertanda "Koreksi SEC" sebelum implementer dipanggil. **Sudah dijawab (6 Okt 2026):** jawaban ada di §5.10 dan penerapannya ditandai **Koreksi SEC** di §5.2, §5.3, §5.5, §5.7, dan §5.9; kriteria penerimaan T2 bertambah dengan butir-butir itu.
2. **Batch 1, T1.** Gate: G-RUST, G-FFI, G-SWIFT, G-GOLDEN (kasus lama tidak berubah). Reviewer rencana: RR, DB, SF, CR. **Rekomendasi:** tambahkan SEC secara ringan untuk penyensoran DDL (§3.6) dan pemasangan sink (§3.5), karena keduanya menyentuh kredensial dan log audit (risiko tinggi menurut `AGENTS.md`).
3. **Batch 2, T2 dan T4.** T2: G-RUST (termasuk tes origin kredensial §7.1 dan tes retry §5.8, keduanya tanpa podman), **G-FFI** (baru, karena ekspor `ssh_config_*`), G-DENY (dependensi langsung `rustls-pki-types` dan `rustix`), G-LIVE (SSH: L-1 sampai L-10; TLS dengan CA uji PostgreSQL dan Trino; `QH_TEST_KEYCHAIN` tidak perlu). Reviewer: SEC, RR, SF, AR, CR (risiko tinggi: kepercayaan, kredensial, FFI). T4: G-SWIFT, scene V-11 baru.
4. **Batch 3, T3 dan T6.** T3: G-SWIFT, G-RUST, G-LIVE (Keychain, CLI end-to-end §5.9 L-10), scene V-11; reviewer SEC, SR, RR, UX, AX. T6: G-RUST, G-FFI, G-SWIFT (`SuggestionScopeTests`), G-BENCHQ (`type-10k`); reviewer RR, SR, CR.
5. **Batch 4, T5.** G-RUST (allowlist kosong menolak; `read_only` tetap); reviewer SEC, RR.
6. **W11-D:** ADR 0038, 0039, 0040 (§15), `PROGRESS.md`, dan `docs/invariants.md` bila ada invarian baru (kandidat: nama TLS lewat tunnel; `Settings` tidak boleh dicetak). **W11-C**, lalu gate W11: G-HEAVY dan G-LIVE (SSH, TLS).
7. **Commit** mengikuti baris rencana; T2 sebaiknya satu commit per lapisan bila tebal (parser `ssh_config`, protokol TOFU, JWT dan CA), karena SEC memeriksanya per lapisan. Subjek tetap Inggris berstil repo, tanpa co-author.

## 15. Bahan untuk ADR (W11-D)

- **0038, perintah baca baru dan keluaran digerbangi setelan (P-06):** D-1 sampai D-9, bentuk event §3.3, rekonstruksi DDL PostgreSQL bukan `pg_dump`, penyensoran kredensial yang best-effort, sink execution log di `EngineHost` dan tidak adanya filter koneksi (keputusan pemilik).
- **0039, SSH di app, TOFU dengan fingerprint dipatok:** §5 utuh (ancaman, penyimpanan, keadaan, rotasi, daftar tidak-pernah), D-10 sampai D-13, D-20, D-24, alias §6 dan daftar penolakannya, dan jawaban Q1 sampai Q5. Tambahan dari review: keadaan `certificate_expected` dan alasannya (Q2); `/etc/ssh/ssh_known_hosts` dibaca (vektor penurunan yang ditutup, dengan harga: isi yang tidak terparse memblokir koneksi); pemeriksaan berkas app tanpa TOCTOU (Q4); pin CLI dan bahwa menghitungnya dari `ssh-keyscan` mengalahkan tujuannya (Q5); risiko sisa rekey (`russh 0.63.3` memeriksa kunci host hanya pada key exchange awal, dan `kex_done` tidak menerima kunci host); penolakan `IdentitiesOnly yes` dan `PreferredAuthentications` di `ssh_config`; validasi host dan kutip POSIX untuk perintah `ssh-keygen -R`.
- **0040, JWT Trino dan CA per koneksi, termasuk batas MySQL:** D-14 sampai D-16, §7, **dua temuan paparan kredensial pada driver Trino, dicatat terpisah dan jangan digabung:** (1) `Prefer` yang turun ke `http` masih mengirim Basic (§16; sempit, karena basis sesi sendiri yang `http`); (2) setiap permintaan ber-kredensial (POST, GET `nextUri`, DELETE cancel) dipasangi header ke URL yang diberikan server tanpa pemeriksaan origin, dan redirect `https` ke `http` pada host dan port yang sama mempertahankan `Authorization` (§1.4; **lebih luas**: berlaku untuk setiap sesi `https` ber-password hari ini, dan dicegah sejak awal untuk bearer). Pemeriksaan origin per permintaan (D-14, §7.1) menutup (2) untuk bearer **dan** Basic; klien tanpa redirect hanya untuk bearer, dan untuk Basic menjadi keputusan pemilik (§16). Termasuk akibat bagi tunnel: `nextUri` ber-port lain gagal dengan galat bernama alih-alih menghubungi `127.0.0.1:<port lain>` (§7.3). Lalu batas MySQL (§12.1 PRD) beserta penolakan MySQL lewat tunnel.

## 16. Risiko

| Risiko | Dampak | Mitigasi |
|---|---|---|
| Penolakan `ssh_config` terlalu ketat (`Host *` berisi direktif asing) | Alias tidak bisa dipakai oleh banyak pengguna, dan alasannya tampak seperti kerewelan | Pesan menyebut direktif dan nomor baris; daftar kosmetik adalah titik penyetelan; jalan keluar satu langkah (kolom eksplisit). Q1 dan §6.1 butir 6. |
| Penolakan `changed` tanpa tombol terima menjadi kebiasaan mengedit berkas | Pengguna menghapus entri tanpa verifikasi | Perintah di pesan dan kata-kata sheet; urutan algoritma §5.5 menghapus penyebab alarm palsu terbesar; D-24 dengan alasan |
| DDL PostgreSQL hasil rekonstruksi berbeda dari `pg_dump` | Pengguna menyalin DDL yang kehilangan opsi penyimpanan, hak, atau trigger | Komentar tetap di baris pertama; tes golden; teks di ADR-0038 |
| Penyensoran DDL tidak menangkap semua rahasia | Kredensial di keluaran MCP atau tab | Aturan §3.6 sempit dan diuji; batas "data pengguna di dalam view" tertulis; SEC meninjau |
| Memasang sink execution log di `EngineHost` | Dua koneksi SQLite ke satu berkas; tulis sinkron di thread async | WAL dan `busy_timeout` 5 detik sudah ada (`crates/qh-storage/src/lib.rs:225-237`); tulis sinkron sudah diputuskan di modul log (rantai tidak boleh berlomba). Tes memasang dan menulis di dalam `EngineHost`. Sink dikunci ke jalur database (`ensure_sink`, §3.5): `install` global per proses (`execution_log.rs:79-93`), jadi tanpa kunci ia menangkap `DB_PATH` pertama untuk selamanya dan tes Swift per-`DB_PATH` berbagi satu log. |
| Satu sesi metadata (O-7: 2+1) melayani pohon, kolom, DDL, dan autocomplete sekaligus | Autocomplete menunggu antrean metadata | Hanya relasi di statement yang aktif, maksimal delapan, dan pemuatan di latar; ketikan tidak menunggu (§12.3). G-BENCHQ memeriksa jalur ketikan. |
| Env berisi rahasia dipegang `HostKeyGate` selama sheet hidup | Rahasia di memori lebih lama | Hanya di memori, maksimal 5 menit, dibuang saat dismiss; tidak pernah ke disk atau log; SEC meninjau |
| `Prefer` Trino turun ke `http` dengan header `Authorization: Basic` (**temuan terpisah**, `crates/qh-driver-trino/src/lib.rs:721-748`) | Password terkirim jelas ke koordinator yang tidak berbicara TLS (koordinator Trino menolaknya, tetapi header sudah di kabel). **Ini kasus yang sempit** (basis sesi sendiri yang jatuh ke `http`); paparan yang lebih luas ada di baris berikutnya | Tidak diperbaiki di W11 (di luar tugas). Untuk JWT dicegah total (D-14). Perbaikan untuk password adalah keputusan pemilik: menghapus kredensial di permintaan turun versi, atau menolak `Prefer` bila ada password. |
| Kredensial Trino dipasang pada URL yang diberikan server (POST, GET `nextUri`, DELETE cancel), dan redirect `https` ke `http` pada host dan port yang sama mempertahankan `Authorization` (§1.4; **temuan memblokir review**) | **Berlaku untuk password Basic hari ini juga, pada setiap sesi `https`, bukan hanya kasus `Prefer`**: `nextUri` `http://` dari koordinator di belakang load balancer tanpa penanganan header forwarded mengirim password atau token sebagai teks jelas pada polling pertama; `nextUri` ke host lain mengirimnya ke host itu | Pemeriksaan origin (skema, host, port) per permintaan ber-kredensial sebelum header dipasang, untuk bearer dan Basic (§7.1 (a)); klien bearer tanpa redirect (§7.1 (b)); tes dengan koordinator palsu (§7.1 (c)). **Sisa yang menjadi keputusan pemilik:** klien Basic masih mengikuti redirect (perubahannya satu baris di `client_for_secret`, tetapi mengubah koneksi password yang sudah ada; direkomendasikan diterapkan juga) dan jatuhnya `Prefer` ke `http` (baris di atas). |
| Pemeriksaan origin membuat koneksi password Trino yang `nextUri`-nya berhost atau berport lain dari yang diketik pengguna gagal (mis. disambung lewat IP sementara koordinator mengumumkan nama DNS-nya, atau lewat tunnel dengan port TLS asli di `nextUri`) | Koneksi yang hari ini jalan (dan mengirim password ke alamat yang diumumkan koordinator) gagal dengan galat bernama | Galat menyebut kedua origin dan bahwa kredensial tidak dikirim; jalan keluarnya menyambung lewat alamat yang diumumkan koordinator atau memperbaiki alamat eksternal koordinator. Sesi tanpa rahasia tidak diperiksa dan tidak berubah. Harga ini disengaja: alternatifnya mengirim password ke alamat yang tidak dipilih pengguna. |
| `/etc/ssh/ssh_known_hosts` dibaca, dan barisnya tidak terparse (mis. penanda yang tidak dikenal parser kita) | Semua koneksi SSH di mesin itu gagal dengan `MalformedKnownHosts` yang menyebut berkas sistem, dan pengguna tidak bisa menyuntingnya | Konsisten dengan prinsip "baris yang tidak terparse adalah baris yang mungkin diabaikan (mis. `@revoked`)" (§5.3 butir 1); pesan menyebut berkas dan baris supaya administrator bisa memperbaikinya. Bila terbukti sering, keputusan terpisah: kebijakan khusus untuk berkas sistem. Isi berkas MDM nyata tidak diperiksa (§18). |
| Store koneksi MCP yang basi (§10.1) | MCP memakai koneksi lama tanpa field SSH | Tidak diperbaiki; keputusan terpisah untuk pemilik |
| `retarget` dan nama TLS mengubah perilaku koneksi `Require` lewat tunnel yang hari ini (mungkin) gagal | Koneksi yang tadinya gagal sekarang berhasil, dan sebaliknya tidak mungkin | Perubahan hanya membuat verifikasi memeriksa nama yang benar; tes dengan dan tanpa `tls_server_name` membuktikan celah dan perbaikannya |
| Perubahan kepemilikan berkas §13.1 belum masuk rencana | Orkestrator menjalankan T1 sampai T6 dengan daftar berkas yang kurang, dan implementer berhenti atau menyentuh berkas orang | Orkestrator memperbarui baris W11 dan §7 sebelum dispatch (bagian "Untuk orkestrator" di verdict) |

## 17. Untuk pemeriksa

- **SEC:** §5 utuh dan jawaban Q1 sampai Q5 beserta penerapannya (**Koreksi SEC**); §3.6 (penyensoran); §7.1 (JWT hanya HTTPS **dan hanya ke origin sesi**, tanpa redirect, tanpa kenaikan diam-diam; tes dengan koordinator palsu); §7.2 (CA hanya bundel itu, penolakan kombinasi, PEM tanpa kunci privat); §9.4 (env di memori); §10.2 (`table_ddl` di scope bawaan?); §3.5 (sink). Bukti yang diminta: baca sumber `russh` untuk klaim §5.5 (`client/kex.rs:158-176`, `client/mod.rs:1893-1902`), dan hasil L-1 sampai L-10.
- **AR:** D-1 (jalur `execute`, bukan metode `Session`) dan harganya (mesin langkah kecil); D-13 (varian di `qh-core`); D-4 dan D-25 (kepemilikan, lane FFI T1 → T2 → T6, `App.swift`); D-16 (`tls_server_name` di `ConnectionConfig`); D-22 (snapshot revisi bukan cache metadata); apakah `host_key` di event `error` dengan gerbang setelan memenuhi P-06 (§4).
- **DB:** semua SQL §3.2 (khususnya `<visible>`, penggunaan `pg_class` bukan `information_schema`, dan rekonstruksi DDL), `SHOW FULL TABLES`, `information_schema.COLUMNS`, dan resep Trino.
- **SF:** kegagalan memasang sink tidak boleh diam; `kinds` yang hilang; `ssh_config_resolve` yang gagal; galat Keychain; `HostKeyGate` yang menelan galat; `HostKeyRecordFailed`.
- **UX dan AX:** §5.4 (invarian sheet), §9.2 (form), §11 (label, tab DDL).
- **RR dan SR:** bentuk tipe di §3.1, §6.1, §7.2; `Sendable` dan aktor untuk `HostKeyCenter`; `Debug` tersensor.

## 18. Fakta yang tidak bisa saya verifikasi

Tidak ada dari ini yang membatalkan rancangan. Masing-masing punya tes atau pemeriksaan yang menutupnya di tugas yang disebut.

1. **Dekode `SHOW CREATE TABLE` MySQL lewat `Session::execute`** (kolom `LONGTEXT` atau blob menjadi `Value::Text` atau `Value::Bytes`?). Saya hanya menjalankannya dengan klien `mysql`, bukan lewat driver. G-LIVE T1 membuktikannya; resep memakai pembantu `cell_text` yang menerima keduanya (UTF-8).
2. **Materialized view Trino.** Connector `memory` di dev tidak mendukungnya, jadi saya tidak tahu apakah `information_schema.tables` melaporkannya sebagai `BASE TABLE`, dan teks galat `SHOW CREATE TABLE` pada MV hanya saya perkirakan dari pola dua galat yang saya lihat. Jalur MV tidak diklaim bekerja sampai ada Trino dengan connector yang mendukungnya.
3. **MySQL `SHOW CREATE VIEW` untuk peran biasa.** Saya menjalankannya sebagai root karena peran `qh` tidak punya hak di `sys`. Perilaku `SHOW VIEW` pada peran terbatas belum diuji.
4. **Celah nama TLS lewat tunnel** (§7.3): dibaca di `retarget` dan `tokio-postgres`, **belum dicoba** (tidak ada harness TLS-di-balik-SSH). Tes §7.3 membuktikannya lebih dulu sebelum perbaikan.
5. **Header `Host` `<nama>:<port_lokal>` pada koordinator Trino di balik proxy** (§7.3), dan SNI dengan `resolve`: dibaca dari `reqwest 0.12.28`, tidak dicoba. Host dan port mana yang akan dimuat `nextUri` koordinator di balik proxy juga tidak diketahui; pemeriksaan origin §7.1 menjadikannya galat bernama, bukan tebakan.
6. **API `russh`.** Tanda tangan `Handler::kex_done(shared_secret, names, session)` dan `names.kex` saya baca dari pemanggilannya di `client/mod.rs:1860-1862` (0.63.3), tidak dikompilasi. Implementer memverifikasi sebelum menulis penjaga D-11. Begitu juga pemetaan algoritma `Algorithm::{Ed25519, Ecdsa{curve}, Rsa{hash}}` ke token `known_hosts`.
7. **API `rustls_pki_types::pem`.** `PemObject::pem_slice_iter` ada di 1.15.1 (`src/pem.rs`), tetapi penggunaannya tidak dikompilasi, dan fitur yang diperlukan (`std` atau `alloc`) tidak diperiksa terhadap `Cargo.toml` tiga driver.
8. **Atribut `.ncx` Navicat selain `SSH_Host`** (§9.5): nama `SSH_Port`, `SSH_UserName`, `SSH_AuthenMethod`, `SSH_Password`, `SSH_PrivateKey`, dan `SSH_Passphrase` adalah dugaan dari bentuk format yang saya kenal. Tidak ada contoh di repo.
9. **Keterjangkauan database dari dalam container `qh-sshd-dev`** untuk L-10. Alamat target di tes end-to-end harus yang terlihat dari sana. Konfigurasi jaringan podman tidak saya periksa.
10. **Apakah sink log yang tidak terpasang di app disengaja.** `target/run/ledger.md` tidak menyebutnya (satu-satunya hit `execution log` adalah backlog B-8). Dugaan saya: tidak disengaja, akibat pindah ke `EngineHost`. Pemilik perlu mengonfirmasi sebelum T1 memasangnya.
11. **Konkurensi dua koneksi SQLite** (sink dan `SharedStorage`) di satu proses: dibaca (WAL dan `busy_timeout`), tidak dijalankan.
12. **View di `SHOW TABLES` MySQL dan Trino.** Server dev tidak punya view biasa (hanya view sistem di `sys`, yang tidak terlihat peran `qh`), jadi klaim di §1.1 bahwa pohon MySQL dan Trino sudah memuat view berasal dari dokumentasi kedua perintah. G-LIVE T1 menambah satu view di skema uji dan memeriksanya.
13. **Nomor baris Swift.** `App.swift`, `AppModel.swift`, `QueryTab.swift`, `DatabaseEngine.swift`, dan `RustEngine.swift` sedang diubah lane implementasi (working tree kotor, HEAD berpindah dari `8103478` ke `2d14eea` selama penulisan ini). Saya menyebut simbolnya. Nomor baris Rust dari `8103478` dan berkas Rust tidak disentuh lane lain selama penulisan.
14. **API `rustix` untuk `geteuid` dan `O_NOFOLLOW`** (§5.2 butir 4): `rustix::process::geteuid() -> Uid` (`rustix 1.1.5 src/process/id.rs:53`), `rustix::fs::OFlags::NOFOLLOW` (`src/backend/libc/fs/types.rs:280-282`), dan fitur `fs` dan `process` di `Cargo.toml`-nya saya baca di sumbernya, tidak dikompilasi. Implementer memverifikasi sebelum menulis. Tidak ada alternatif tanpa dependensi baru, karena `unsafe_code = "forbid"`.
15. **Isi `/etc/ssh/ssh_known_hosts` di mesin MDM** (§5.2, §5.10): tidak ada contoh di repo, dan mesin dev ini tidak punya berkasnya (`ls` menjawab tidak ada). Apakah parser kita menerima semua baris yang lazim dipatok administrator tidak diketahui.
16. **Koordinator Trino di belakang load balancer pengakhir TLS mengembalikan `nextUri` `http://`** (§1.4, §7.1): klaim review dan pengetahuan umum tentang konfigurasi proxy, tidak dicoba di sini (tidak ada harness load balancer). Tes §7.1 (c) memakai fake yang meniru jawabannya.
17. **API reqwest untuk koreksi §7.1:** `redirect::Policy::none()` (`reqwest 0.12.28 src/redirect.rs:58`), `ClientBuilder::redirect` (`src/async_impl/client.rs:1383`), dan `reqwest::Url` (`src/lib.rs:280`) saya baca di sumbernya, tidak dikompilasi. Perilaku "tidak mengikuti redirect" (respons 3xx dikembalikan apa adanya) dibuktikan oleh tes §7.1 (c) kasus 4, bukan oleh pembacaan.

## Verdict architect-reviewer

*Diperiksa oleh `architect-reviewer` (6 Okt 2026) terhadap blueprint ini, kode, dan `development-plan.md`; dicatat di sini oleh penulis blueprint. **Verdict: perubahan diminta, satu temuan memblokir.** Koreksi untuk temuan itu sudah diterapkan di dokumen ini (§1.4, §2 D-14, §7.1, §7.3, §13.1, §13.3, §14, §15, §16, §18) dan **menunggu pemeriksaan ulang** (O-20: satu ronde; blueprint belum "approved" sebelum itu). Temuan non-memblokir juga sudah diterapkan sebagai koreksi dan dicatat di bawah supaya pemeriksa ulang bisa mencocokkannya. Verdict atas implementasi diberikan sesudah W11-T6.*

### Klaim yang diperiksa di kode

**Temuan memblokir: rencana JWT (§7.1, D-14) tidak memenuhi FR-CON-07.** Penjaga bearer hanya memeriksa skema basis URL sesi, sedangkan driver Trino mengirim setiap permintaan halaman dan setiap cancel ke URL yang diberikan server: `fetch_page` memanggil GET pada `self.next_uri` (`crates/qh-driver-trino/src/lib.rs:1098-1110`), `cancel` memanggil DELETE pada `running.next_uri` (`:640-645`), dan keduanya lewat `with_shared_headers` (`:581-592`) yang memasang kredensial ke URL apa pun. Tidak ada pemeriksaan skema atau origin atas `nextUri` di berkas itu. Koordinator di belakang load balancer pengakhir TLS tanpa penanganan header forwarded mengembalikan `nextUri` `http://` (salah konfigurasi yang umum), dan token keluar sebagai teks jelas pada polling pertama. `nextUri` ke host lain mengirim token ke host itu. Kebijakan redirect bawaan reqwest mempertahankan `Authorization` pada redirect `https` ke `http` bila host dan port eksplisit sama. **Koreksi yang diminta dan diterapkan:** (a) pemeriksaan origin per permintaan ber-kredensial (POST, GET `nextUri`, DELETE cancel; skema, host, dan port harus sama dengan basis sesi, bila tidak galat bernama `Permanent` sebelum header dipasang); (b) klien bearer dibangun dengan `redirect(Policy::none())`; (c) tes dengan koordinator palsu untuk `nextUri` `http://`, `nextUri` lintas host, dan redirect `https` ke `http` pada host dan port yang sama, yang menegaskan bahwa header `Authorization` tidak pernah muncul pada permintaan plaintext atau asing; (d) §16 dan ADR-0040 diperbarui: password Basic yang ada punya paparan `nextUri` yang sama, lebih luas daripada kasus fallback `Prefer` yang tercatat, dan tidak cocoknya port `nextUri` di belakang tunnel (§7.3, §18 butir 5) kini gagal dengan galat jelas alih-alih menghubungi `127.0.0.1:<port lain>`.

Klaim kode yang diperiksa reviewer dan dinyatakan benar:

- `russh 0.63.3`: KEX `none` melewati pemeriksaan host key (`client/kex.rs:157-176`, `client/mod.rs:1893-1902`); `SAFE_KEX_ORDER` tidak memuat `none` (`negotiation.rs:162-176`); tanda tangan `kex_done` (`client/mod.rs:2394-2401`).
- Semantik `check_text` dan rujukan barisnya di `known_hosts.rs` (`:111`, `:128`, `:149-211`, `:223`, `:291`, `:319`, `:408`).
- Engine selalu `Strict`, dan `retarget` ada di `crates/qh-ffi/src/tunnel.rs:158-163`.
- Hanya `main.rs:66` dan `bin/mcp.rs:91` yang memasang sink execution log.
- `Settings` menurunkan `Debug` (`env.rs:41`); `route()` tanpa wildcard (`host.rs:224`); `COMMANDS` berisi 26 entri dengan aturan "sebelum `objects`" (`lib.rs:453-461`); `PoolKey::of` membongkar tanpa `..`.
- `base_pairs` MCP hanya membawa `DB_PATH` (`bin/mcp.rs:247-252`), jadi `SSH_HOST_KEY_ACCEPT` tidak bisa masuk dari lingkungan proses MCP; `legacy_directory()` sama dengan `ConnectionStore.directory()` di luar tes.
- Celah kepemilikan berkas di §13.1 dan D-25 ditandai dengan benar.

Klaim yang dipakai koreksi, diperiksa penulis di kode untuk revisi ini (HEAD `2d14eea`, Rust tidak disentuh lane lain): tiga pemanggil `with_shared_headers` (`lib.rs:645`, `:699`, `:1103`) dan tidak ada pemeriksaan `next_uri`; `running_from` menyimpan `nextUri` saat POST (`:834-838`); `reqwest 0.12.28` `redirect.rs:239-251` (host dan port, bukan skema) dan `Policy::default` = `limited(10)` (`:161-163`); `russh` `client/mod.rs:1868-1902` (rekey tanpa `check_server_key`) dan `:2394-2401` (`kex_done` tanpa kunci host); `authenticate_with_agent` mencoba semua identitas agen (`tunnel.rs:424`, `:437`); `execution_log::install` global dan menimpa (`execution_log.rs:79-93`); `TunnelIdentity` (`pool.rs:109-114`, `:190-196`); tujuh requirement `DatabaseEngine` dan `warmUp` bawaan kosong di extension; `known_hosts_line` tidak melipat huruf (`key.rs:59-66`) sedangkan pencocokan melipat (`known_hosts.rs:349-353`); `EngineError::failure_kind` tanpa wildcard (`error.rs:108-118`).

### Keputusan atas pertanyaan di §17

Jawaban SEC atas Q1 sampai Q5 (§5.10), semuanya diterapkan:

- **Q1:** setuju dengan aturan yang lebih ketat (jenis kunci berbeda = `Mismatch` keras), dengan syarat urutan algoritma terpasang. `recorded_key_types` hanya membaca baris tanpa penanda (`@revoked` dan `@cert-authority` diabaikan), menyertakan entri berhash, dan jenis yang tidak bisa dinegosiasikan `russh` (mis. `sk-*`, `ssh-dss`) tetap `Mismatch`, tidak pernah `Unknown`.
- **Q2:** tolak, jangan tawarkan TOFU. Bila `ca_covered`, `known_hosts` pengguna sendiri menyatakan host itu seharusnya menyajikan sertifikat yang build ini tidak bisa verifikasi. Wording penolakan keras tanpa tombol terima, keadaan `certificate_expected` (atau memakai ulang `certificate`). Ini juga menutup jalur penurunan bagi MITM yang menyajikan kunci polos ke host yang dikelola CA.
- **Q3:** mencatat sebelum autentikasi sudah benar. Sama dengan OpenSSH, dan autentikasi tidak boleh dikirim ke kunci yang belum tercatat. Pesan "trusted but connection failed" di §9.4 menutup biaya UX-nya.
- **Q4:** proporsional, dengan cara tanpa TOCTOU: `O_NOFOLLOW`, `fstat` atas deskriptor yang terbuka (bukan `lstat` atas jalur), pemilik `== geteuid()`, `mode & 0o022 == 0`; pemeriksaan yang sama saat berkas app dibaca di `check_all`, bukan hanya saat ditambah, karena berkas app yang dapat ditulis grup tetap bisa menghasilkan `Matched`.
- **Q5:** pertahankan pin CLI. Ia butuh nilai eksplisit per run dan bukan penerimaan otomatis. ADR-0039 mencatat bahwa menghitung pin dari `ssh-keyscan` mengalahkan tujuannya.

### Perubahan rencana yang dibutuhkan (untuk orkestrator)

1. **Memblokir, diterapkan, menunggu pemeriksaan ulang:** origin kredensial dan klien bearer tanpa redirect (§7.1 (a) sampai (c), §16, §15 ADR-0040). Pemeriksa ulang mencocokkan §7.1 dengan butir (a) sampai (d) di atas.
2. **Kepemilikan berkas harus masuk `development-plan.md` §5 dan §7 sebelum dispatch** (D-25, §13.1), termasuk T2 masuk lane FFI (rantai `... → W10-T7 → W11-T1 → W11-T2 → W11-T6 → W12-T1 ...`), `App.swift` untuk W11-T1, baris W11 untuk `Support/DatabaseEngine.swift` dan `Support/Snapshot.swift`, dan `Cargo.lock` untuk W11-T2.
3. **Tambahan kepemilikan T2 dari putaran ini** (sudah di §13.1 dan §13.3): `crates/qh-driver-trino/tests/credential_origin.rs` (baru), `crates/qh-ffi/src/retry.rs` (hanya modul tes), `crates/qh-tunnel/Cargo.toml` (`rustix`).
4. **Kriteria penerimaan T2 bertambah:** tes origin kredensial dan redirect (§7.1 (c)); `check_all` membaca `/etc/ssh/ssh_known_hosts`; `HostKeyCertificateExpected`; pemeriksaan berkas app tanpa TOCTOU di jalur baca dan tulis; `recorded_key_types` sesuai Q1; `shell_quote` dan validasi host; `IdentitiesOnly yes` dan `PreferredAuthentications` ditolak; `TunnelIdentity` memuat semua berkas kepercayaan; semua galat host-key `Permanent` dengan satu tes di `retry.rs`.
5. **Kriteria penerimaan T1 dan T3 bertambah:** T1, sink `EngineHost` berkunci jalur dan pembaca lewat `with_storage` (§3.5), dengan tes di `crates/qh-ffi/tests/host.rs`. T3, `HostKeyGate` mengimplementasikan ketujuh requirement protokol (§9.4), dengan `runIntoStore` diamati dan `warmUp` diteruskan, dan `HostKeyGateTests` membuktikan `warmUp` sampai ke engine yang dibungkus (§9.6).
6. **Keputusan pemilik yang menunggu:** (i) klien password Basic ikut `client_for_secret` (tanpa redirect)? Direkomendasikan ya; (ii) perbaikan `Prefer` yang turun ke `http` untuk password (§16).

### Risiko terbuka (tidak memblokir)

- **Rekey `russh`:** `check_server_key` hanya dipanggil pada key exchange awal (`client/mod.rs:1887-1902`), jadi pada rekey kunci host server tidak dibandingkan lagi. Hanya peer yang sudah terautentikasi yang bisa memicunya, dan `kex_done` tidak menerima kunci host di 0.63.3. Dicatat di §5.10 dan ADR-0039.
- **`/etc/ssh/ssh_known_hosts`:** sekarang dibaca (vektor penurunan ditutup), dengan harga isi tak terparse memblokir koneksi di mesin itu (§16). Isi berkas MDM nyata tidak diperiksa (§18).
- **Penolakan `ssh_config`:** `IdentitiesOnly yes` kini ditolak dengan nama, yang bisa mahal bagi pengguna dengan `Host *` berisi direktif itu. Alternatif (menghormatinya di jalur agen) tercatat di §6.1 butir 5.
- **Pemeriksaan origin pada koneksi password Trino yang ada:** koneksi yang bergantung pada `nextUri` berhost atau berport lain kini gagal dengan galat bernama (§16). Disengaja.
- **Klien Basic masih mengikuti redirect, dan `Prefer` masih bisa turun ke `http` dengan Basic:** keputusan pemilik (§16).
- **API yang dibaca tetapi tidak dikompilasi:** `rustix` (`geteuid`, `O_NOFOLLOW`), `reqwest` (`Policy::none`), dan tanda tangan `russh` (§18 butir 6, 14, 17).
- **Tidak ada penguncian antar proses di berkas app**, dan satu catatan per nama (nama dan IP = dua prompt): tidak berubah (§5.10).
