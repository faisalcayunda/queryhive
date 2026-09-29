# TablePro dan QueryHive: analisis celah fitur

> Dokumen ini memetakan permukaan fitur TablePro terhadap apa yang sudah ada di QueryHive, lalu
> memisahkan mana yang layak diambil. Rencana eksekusinya ada di
> [`tablepro-adoption-plan.md`](tablepro-adoption-plan.md).
>
> **Akses sumber:** 2026-09-29. **Batas lisensi:** TablePro AGPL-3.0, QueryHive MIT (`LICENSE`,
> ADR-0002). Tidak ada baris kode TablePro yang boleh masuk ke pohon ini.

## 1. Yang dibaca, dan yang belum

Dari TablePro:

| Sumber | Yang dipakai |
|---|---|
| `docs.tablepro.app/llms.txt` | indeks docs, 146 baris, satu baris per fitur dan per engine |
| `README.md` | ringkasan produk, arsitektur, plugin system |
| `CLAUDE.md` | invariant proyek, daftar jebakan performa, aturan PluginKit |
| `CONTRIBUTING.md` | alur kontribusi dan gate CI |
| `project.yml` | target Xcode dan daftar plugin di agregat |
| Daftar `Plugins/`, `docs/`, `.github/`, `.claude/` | inventaris plugin, berkas CI, aturan agen |

Yang **belum** dibaca: halaman fitur TablePro satu per satu, source app TablePro, dan source
plugin-nya. Deskripsi fitur TablePro di bawah karena itu berasal dari dokumentasinya sendiri, bukan
dari kodenya. Baris yang belum punya sumber ditandai `[perlu verifikasi]`.

Dari QueryHive, setiap klaim menyebut path di worktree ini.

## 2. Batas yang menentukan segalanya

TablePro dirilis AGPL-3.0. QueryHive MIT (`LICENSE`, keputusan ADR-0002, dijaga
`cargo deny check licenses`). Menyalin kode AGPL ke pohon MIT tidak bisa dilakukan: karya
gabungannya wajib AGPL, dan itu membatalkan ADR-0002 sekaligus policy di `deny.toml`.

Yang tetap berguna adalah pola, urutan kerja, daftar invariant, bentuk protokol, dan daftar jebakan.
Semua itu ide, dan ide tidak berlisensi. Seluruh isi dokumen ini berdiri di sisi itu.

Kalau nanti ada bagian yang hanya bisa selesai dengan menyalin kode TablePro, jalannya satu: proyek
turunan terpisah berlisensi AGPL-3.0, bukan tambalan di pohon ini.

## 3. Permukaan fitur TablePro

Sumber tiap baris adalah halaman docs yang tercantum di `docs.tablepro.app/llms.txt`.

**Engine.** Docs index menyebut 36 engine. Yang punya plugin di `Plugins/`: Beancount, BigQuery,
Cassandra/ScyllaDB, ClickHouse, Cloudflare D1, Cloudflare R2 SQL, Dameng DM8, DuckDB, DynamoDB,
Elasticsearch, etcd, Kafka, libSQL/Turso, MSSQL, MongoDB, MySQL, Oracle, PostgreSQL, Redis,
Snowflake, Spanner, SQLite, SurrealDB, Teradata, Trino, Typesense, Weaviate. MariaDB, OceanBase,
TiDB, Databend, Redshift, CockroachDB, PGlite berjalan di atas driver lain.

**Transport koneksi.** SSH tunneling, SSH profiles, credential profiles (satu user/password dipakai
banyak koneksi), remote database files (SQLite di server SSH), Cloudflare Tunnel (cloudflared
dikelola app), Cloud SQL Auth Proxy, SOCKS5 proxy dengan DNS remote, tunnel command (kubectl
port-forward, AWS SSM), AWS IAM auth untuk RDS/Aurora, SSL/TLS per driver, connection sharing
(ekspor, share link, impor dari app lain, resolusi password dari secret manager), Open Project
Folder (bikin koneksi dari config proyek).

**Plugin dan tema.** Registry plugin untuk driver, format impor/ekspor, dan tema, lengkap dengan
pembaruan.

**Editor.** SQL editor dengan multi-statement execution, find and replace, dan formatter bawaan.
Code folding untuk statement, CTE, subquery, dan blok BEGIN, juga di setiap code view lain. SQL files
dibuka sebagai tab dan disimpan kembali, termasuk mendeteksi perubahan dari luar app. Autocomplete
schema-aware untuk keyword, tabel, kolom, dan fungsi. Query parameters `:name` dengan panel pengisian
dan prepared statement. Vim mode dengan motion, text object, register, mark, dan search. Query
history di SQLite lokal dengan full-text search. Query insights: query yang paling sering dijalankan,
paling mahal, dan yang melambat.

**Grid dan hasil.** Data grid: sort, ubah lebar, sembunyikan kolom, highlight, chart, map, copy.
Query results: memilih di antara beberapa result, pinning, row cap, dan tampilan untuk statement yang
gagal atau bukan SELECT. Filtering: bangun WHERE dari filter bar atau tulis manual, simpan sebagai
preset. Change tracking: antre edit sel, insert baris, dan delete baris, tinjau SQL-nya, baru simpan.
Cell and row viewers: JSON, PHP serialized, gambar, nilai binary, format tampilan per kolom, dan
inspeksi satu baris penuh di sidebar. Data files: buka CSV/TSV/JSON/Excel di jendela sendiri,
filter, bersihkan, simpan atau ekspor tanpa impor.

**Struktur dan operasi skema.** Table structure: lihat dan ubah kolom, index, foreign key, trigger,
dan DDL lewat editor visual. Routines/triggers: browse stored procedure, function, dan trigger per
schema, baca sumbernya. User-defined types: enum, composite, domain, range, termasuk tambah dan
ganti nama label enum. Table operations: drop, truncate, maintenance, kelola view, pindah database
dari sidebar. ER diagram interaktif. EXPLAIN visualization dalam bentuk diagram, tree, atau raw.
Server dashboard: sesi aktif, metrik server, slow query secara real time. Users & roles: kelola user,
role, dan privilege tanpa menulis GRANT sendiri.

**Perpindahan data.** Import & export: ekspor ke CSV, JSON, SQL, MQL, XLSX; impor SQL, JSON, dan CSV
dengan column mapping dan transaction safety. Copy and duplicate: salin tabel atau database ke
database lain, koneksi lain, atau engine lain, struktur saja, data saja, atau keduanya. Backup &
restore memakai tool bawaan tiap engine, dengan scope, progress, cancel, dan reuse SSH tunnel.
Compare & sync: bandingkan dua database dan hasilkan skrip SQL yang menyamakannya.

**AI.** AI assistant dengan chat bertool-calling, inline suggestion, serta review, explain, optimize,
dan fix-error, di 14 provider. Agent mode memberi satu sesi AI seluruh connection window: sesinya,
percakapannya, dan SQL yang diusulkan.

**MCP.** MCP server bawaan dengan 47 tool, resources, prompts, dan subscriptions. Outside MCP
servers: sesi AI boleh memanggil tool di MCP server yang kamu jalankan sendiri, dibatasi ke koneksi
pilihan. Pairing memakai alur ala PKCE untuk menerbitkan token berscope. Token punya scope,
allowlist koneksi, masa berlaku, pencabutan, dan activity log. Ada pula kebijakan stabilitas untuk
URL scheme, tool, dan resource.

**Workspace.** Connection window, query tab yang menyimpan SQL, hasil, sorting, dan filter sendiri
lalu kembali setelah restart, connections strip, Open Quickly (cari objek dan query di satu atau
semua koneksi terbuka), favorites (pin database dan tabel, simpan SQL, tautkan folder `.sql`), dan
keyboard shortcut yang bisa di-rebind di Settings > Keyboard.

**Platform.** iCloud sync untuk koneksi, grup, dan tag ke Mac/iPhone/iPad, plus settings, table
favorites, dan SSH profiles antar Mac. Handoff untuk melanjutkan koneksi atau tabel di perangkat
lain. Licensing dan team plan. App iPhone dan iPad untuk sepuluh engine.

**Settings.** General (bahasa, perilaku startup, tab, layout sidebar, koneksi, query timeout, command
line tool, update, reset), Appearance (tema, font per tema, kustomisasi warna, warna koneksi), Editor
(editor, vim, nomor baris, indentasi, query parameter), Data (grid, pagination, result cap, JSON
viewer, query history), Notifications (operasi mana yang memberi notifikasi, durasi minimum, isi
notifikasinya).

**Keamanan dan privasi.** Safe Mode: kontrol eksekusi per koneksi, dari tanpa batas sampai read-only
penuh. Halaman privacy mendaftar setiap request keluar dan sakelarnya. Managed updates lewat
configuration profile untuk armada.

**External API.** URL scheme `tablepro://`, AppleScript dengan scripting dictionary, integrasi
terminal dan ddev, ekstensi Raycast, iOS Shortcuts, dan daftar klien MCP yang didukung (Claude
Desktop, Claude Code, Cursor, VS Code, Cline, Continue, Zed, Windsurf, Antigravity, Goose).

**Pengembangan.** Arsitektur, setup, building (termasuk plugin compile check dan apa yang dijalankan
CI pada pull request), releasing (notarization, DMG, static library, tag), code style, plugin
development (protokol PluginKit, bundel `.tableplugin`), plugin registry (format manifest, pemilihan
binary, publishing, kompatibilitas PluginKit), dan cara menguji plugin lokal.

Inventaris plugin di `Plugins/` berisi 42 entri: 41 plugin plus `TableProPluginKit`. Ekspor yang
tersedia sebagai plugin: CSV, HTML, JSON, Markdown, MQL, Parquet, SQL, XLSX, XML. Impor: CSV, JSON,
SQL, XLSX.

## 4. Permukaan fitur QueryHive hari ini

**Engine.** Workspace Rust, 14 perintah di `crates/qh-ffi/src/lib.rs:464`, permukaan UniFFI,
di-link statis ke app. Dua konsumen dari permukaan yang sama: app dan CLI `queryhive-engine`
(`crates/qh-ffi/src/main.rs`).

**Driver.** Trino (`crates/qh-driver-trino`), PostgreSQL, MySQL. Ketiganya crate compile-time.

**Koneksi.** Tiga jenis, dipilih dari tile; URL `trino://`, `postgresql://`, `mysql://`; disimpan di
`connections.json` dengan password di Keychain; impor dari ekspor Navicat `.ncx`
(`app/Sources/TrinoExporter/Support/NavicatImport.swift`). SSH bastion lewat `qh-tunnel`, TLS lewat
`docs/tls-modes.md`.

**Object tree.** connection → catalog → schema → table, dimuat saat di-expand
(`app/Sources/TrinoExporter/Models/SchemaTree.swift`). Bentuk tiap level mengikuti driver.

**Editor.** NSTextView-backed (`Views/SQLEditor.swift`) dengan syntax colouring (`Views/SQLSyntax.swift`)
dan autocomplete yang meranking objek di atas keyword (`Models/SQLSuggestions.swift`). Aksi yang bisa
di-bind (`Models/Shortcuts.swift`): `run`, `runScript`, `explain`, `countRows`, `stop`, `newQuery`,
`openFile`, `saveFile`, `exportData`, `toggleResultPanel`, `commentLine`, `format`, `closeTab`. Jadi
script multi-statement, buka/simpan file `.sql`, dan format SQL sudah ada. Pemisahan statement
dikerjakan `qh-sql`, bukan driver: PostgreSQL menolak multi-statement
(`crates/qh-driver-postgres/src/lib.rs:19`).

**Hasil.** Run mengambil N baris pertama (default 1000) dan melukisnya di grid
(`Views/ResultGrid.swift`) dengan nomor baris, header bertipe, dan filter per kolom
(`columnFilters`). Export di footer grid menjalankan ulang statement dan men-stream seluruh hasil.
Tidak ada sorting di grid.

**Edit sel.** `Models/CellEdits.swift` menyimpan nilai sel yang berubah, dan
`Models/UpdateStatements.swift` menghasilkan `UPDATE` beserta predikatnya. Belum ada insert atau
delete baris.

**Destinasi tulis.** `to_table` dengan tiga mode: `CREATE TABLE AS`, `DROP TABLE IF EXISTS` +
`CREATE` (`crates/qh-ffi/src/commands.rs:771`), dan `INSERT INTO … SELECT`.

**Ekspor.** Sembilan format streaming: `txt`, `csv`, `json`, `xml`, `html`, `sql`, `xls`, `xlsx`,
`dbf`. Ada part splitting, retry dengan backoff, `ZIP`, `ROWS_PER_FILE`, `BATCH_SIZE`.

**Tampilan.** Mode System/Light/Dark, tiga preset, tujuh canvas, lima accent, tiga tone, plus CLI
`--snapshot` untuk review tema (`Support/Theme.swift`, `Support/ThemeStore.swift`,
`Support/Snapshot.swift`).

**Settings.** Appearance dan Keyboard (`Views/SettingsView.swift`).

**Penyimpanan dan sinkronisasi.** `crates/qh-storage` dengan migrasi `0001_init.sql` yang membuat
tabel `connection`, `connection_group`, `query_history` (baris 44), `saved_query` (baris 52), dan
`session_restore` (baris 59). `crates/qh-credentials` untuk Keychain, `crates/qh-sync` untuk
prefetch baris, `crates/qh-result-store` untuk store grid.

**Distribusi.** `app/build.sh`, `app/build-dmg.sh` (termasuk `--notarize`), dan `app/release.sh`
yang memaket, menandatangani feed Sparkle, menandai tag, dan menerbitkan release dengan dua aset:
DMG dan `appcast.xml`.

**Verifikasi.** Golden corpus di `tests/golden/` berisi 43 perintah terekam. Harness yang berlaku:
`cargo test --workspace`, `swift test`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo deny check licenses`, dan `/usr/bin/python3 tools/golden/live_cases.py`.

Diukur ulang 29 Sep 2026: `cargo test --workspace` 559 lulus / 0 gagal, `swift test` 158 tes lulus,
`cargo fmt --all --check` bersih, clippy bersih dengan `-D warnings`, `cargo deny check licenses`
melaporkan `licenses ok`, `./app/build-ffi.sh` menghasilkan `app/Generated/` yang identik dengan yang
di-commit, dan `swift build` selesai 5,19 detik.

Kasus golden live diukur sebagian pada tanggal itu, delapan kasus PostgreSQL saja: **4 cocok**
(`postgres_catalogs_live`, `postgres_tables_live`, `postgres_objects_live`, `postgres_count_live`)
dan **4 berbeda pada kunci `columns`** (`postgres_type_zoo_live`, `postgres_batching_live`,
`postgres_explain_live`, `postgres_export_live`). Tujuh kasus MySQL dan tujuh kasus Trino belum
dijalankan. Angka 12/22 di `PROGRESS.md` karena itu masih milik 24 Sep 2026 dan belum digantikan.

**Belum ada.** Query history dan saved queries tidak sampai ke engine; tidak ada MCP; tidak ada AI;
tidak ada impor data; tidak ada find and replace di editor; tidak ada code folding yang ditemukan di
source app; tidak ada query timeout; tidak ada Safe Mode; tidak ada `.github` sama sekali.

## 5. Matriks celah

| Area | TablePro | QueryHive | Putusan |
|---|---|---|---|
| Engine | 36 | 3 (Trino, Postgres, MySQL) | Jangan dikejar. QueryHive menang di kedalaman Trino and gudang data |
| Transport | SSH, SOCKS5, cloudflared, Cloud SQL proxy, tunnel command, IAM, profile | SSH + TLS | Ambil SOCKS5 dan tunnel command bila ada permintaan nyata |
| Query history | SQLite + FTS | tabelnya ada, tidak diekspos | Ambil, termurah di daftar |
| Saved queries | ada, plus favorites | tabelnya ada, tidak diekspos | Ambil bersama history |
| Editor: find/replace | ada | tidak ada | Ambil, biaya kecil |
| Editor: folding | ada | tidak ditemukan | Ambil, biaya kecil |
| Editor: parameter `:name` | ada | tidak ada | Tunda, butuh jalur prepared statement |
| Editor: vim mode | ada | tidak ada | Tunda, besar dan opinian |
| Query insights | ada | tidak ada | Tunda, turunan dari history |
| Grid: sort | ada | tidak ada | Ambil, biaya kecil |
| Grid: chart/map | ada | tidak ada | Tolak, keluar dari lingkup |
| Change tracking | sel, insert, delete, tinjau SQL | `UPDATE` dari sel saja | Ambil insert dan delete |
| Cell viewer | JSON, gambar, binary, format per kolom | tidak ada | Ambil untuk JSON, sisanya tunda |
| Data files | CSV/TSV/JSON/Excel di jendela sendiri | tidak ada | Tunda |
| Structure editor | kolom, index, FK, trigger, DDL | tidak ada | Tunda, ini proyek tersendiri |
| Routines, UDT | ada | tidak ada | Tunda, tergantung driver |
| Table operations | drop, truncate, maintenance | tidak ada | Ambil truncate dan drop, lewat konfirmasi |
| ER diagram | ada | tidak ada | Tolak untuk sekarang |
| EXPLAIN | diagram, tree, raw | raw lewat `explain` | Ambil bentuk tree bila murah |
| Server dashboard | ada | tidak ada | Tolak, butuh dukungan per driver |
| Users & roles | ada | tidak ada | Tolak, administratif |
| Import | SQL, JSON, CSV, XLSX, column mapping | tidak ada | Ambil CSV/XLSX ke tabel |
| Export | 9 plugin, termasuk Parquet | 9 format streaming, tanpa Parquet | Ambil Parquet |
| Copy object antar engine | ada | tidak ada | Tunda |
| Backup & restore | tool bawaan engine | tidak ada | Tunda, tapi catat untuk Postgres |
| Compare & sync | ada | tidak ada | Tolak, proyek tersendiri |
| AI assistant | 14 provider | tidak ada | Tunda, dan putuskan dulu soal on-prem |
| MCP server | 47 tool + resources + prompts | tidak ada | Ambil, ini kandidat terkuat |
| Workspace: Open Quickly | ada | tidak ada | Ambil setelah tree punya index |
| Favorites | ada | tidak ada | Ambil bersama saved queries |
| iCloud sync | ada | tidak ada | Tolak |
| iOS app | ada | tidak ada | Tolak |
| Settings | 5 tab | 2 tab | Tambah tab Data saat grid dan history mendarat |
| Safe Mode | per koneksi, sampai read-only | tidak ada | Ambil, ini soal keselamatan |
| Query timeout | ada di Settings | tidak ada | Ambil, ini soal keselamatan |
| External API | URL scheme, AppleScript, Raycast, Shortcuts | tidak ada | Tunda, MCP lebih dulu |
| Plugins | registry + ABI | driver compile-time | Tolak, ABI runtime bukan langkah berikutnya |
| CI | workflow + quarantine + baseline | tidak ada `.github` | Ambil, ini pintu masuk semua fase lain |

## 6. Temuan di sisi kita

Ini cacat yang sudah terverifikasi, bukan sekadar celah fitur.

**`app/DESIGN.md` bertentangan dengan kodenya sendiri.** Bagian Scope (baris 18 sampai 19) menulis
out of scope: "universal or Intel builds, notarization, editing table data, saved query files, a
result data grid". Empat dari lima sudah mendarat: `Views/ResultGrid.swift` untuk grid hasil,
`Models/CellEdits.swift` dan `Models/UpdateStatements.swift` untuk edit data,
`openFile`/`saveFile` di `Models/Shortcuts.swift` untuk file SQL, dan `--notarize` di
`app/build-dmg.sh`. Berkas itu menyebut dirinya "the design contract" dan meminta pelapor berhenti
saat ada konflik, jadi ini bukan salah tulis biasa: kontraknya sudah tidak menggambarkan aplikasinya.

**Skema penyimpanan lebih kaya daripada permukaan engine.** `query_history` dan `saved_query`
dibuat di `crates/qh-storage/migrations/0001_init.sql` (baris 44 dan 52) dan diuji di
`crates/qh-storage/src/lib.rs:356` dan `:372`, tapi `Command` hanya mengekspos `connections`,
`import_connections`, dan `credential` sebagai perintah lokal. `Support/DatabaseEngine.swift:22`
mencatat sendiri bahwa engine tidak mengekspos history atau saved queries. Fiturnya setengah jadi:
tabelnya hidup, jalannya tidak ada.

**Dua enum tertutup di kedua sisi batas.** `DriverKind` adalah enum tertutup di Rust
(`crates/qh-driver/src/lib.rs:46`), dan Swift punya enum paralel `ConnectionKind` yang di-parse dari
string di `Models/Connections.swift:356`. Komentar di `crates/qh-driver/src/lib.rs:1` menyatakan
driver keempat cukup menambah crate dan meregister. Untuk mesin itu benar, untuk UI tidak: satu
driver baru berarti satu varian Rust, satu case Swift, satu logo, dan satu tile.

**Tidak ada CI.** Tidak ada direktori `.github` di pohon ini. Satu-satunya berkas YAML di luar
`target/` dan `app/.build/` adalah `deploy/dev/compose.yaml`. Yang dijaga skrip tangan:
`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`, `cargo deny check licenses`, `swift test`, dan `tools/golden/live_cases.py`.

**Korpus golden melenceng dari klaimnya.** `PROGRESS.md` mencatat klaim "22/22" sekarang jadi 12/22
dan "16 kasus identik" sekarang 17, dengan anotasi di tempatnya. Selisih 10 kasus sudah
terklasifikasi, tapi tidak ada yang menjalankannya berkala.

**Tidak ada query timeout.** Satu-satunya timeout di seluruh `crates/` adalah
`connect_timeout` di `crates/qh-driver-trino/src/lib.rs:262`. Tidak ada batas waktu statement di
trait driver maupun di `ExecuteOptions`. `preview` pada tabel besar karena itu tidak punya langit
kedua selain tombol Stop.

**Editor belum punya find and replace.** Pencarian untuk `findBar`, `showFind`, `replaceAll`, dan
`performFindPanelAction` di `app/Sources` tidak menemukan apa pun. Untuk editor yang menangani file
`.sql`, ini celah harian.

**Kegagalan koneksi PostgreSQL membuang alasan dari server, dan ini sudah diperbaiki.** `open()` di
`crates/qh-driver-postgres/src/lib.rs` membentuk pesan connect dengan `format!("{}: {error}", ...)`,
sementara `Display` dari `tokio_postgres::Error` untuk jawaban server hanyalah `db error`. Diukur
29 Sep 2026 dengan meminta role yang tidak ada: sebelum perbaikan pesannya berakhir `: db error`,
sesudahnya `password authentication failed for user "qh_absent" (SQLSTATE 28P01)`. `map_query_error`
di berkas yang sama sudah membaca `as_db_error()` dan `classify_connect_error` juga, jadi perbaikan
ini mengembalikan kesepakatan di dalam satu berkas, bukan menambah aturan baru. Belum ada tes yang
mengunci perilakunya: `tokio_postgres::Error` tidak punya konstruktor publik untuk dibangun di dalam
unit test, jadi buktinya masih berupa reproduksi manual terhadap server nyata.

## 7. Yang sebaiknya tidak diambil

**36 engine.** Ini justru keunggulan TablePro dan bukan lapangan yang bisa dimenangkan dari belakang.
QueryHive sudah memilih Trino sebagai warga kelas satu (ADR-0006, klien hand-rolled), dan itu posisi
yang TablePro layani lewat satu plugin di antara 41.

**Plugin registry dan ABI runtime.** TablePro membayarnya dengan `TableProPluginKit`, versioning ABI,
checker kompatibilitas, dan registry manifest. Untuk QueryHive itu berarti `repr(C)`, versioning
ABI, dan registry, sementara ketiga driver sekarang berbagi tipe Rust biasa. Tidak ada yang menahan
pekerjaan sekarang karena driver masih compile-time.

**iCloud sync, app iOS, licensing, team plan.** Itu fitur produk komersial, bukan fitur alat lab.

**Chart, map, dan ER diagram di grid.** Keluar dari lingkup "ekspor adalah hasil", dan masing-masing
menyeret renderer sendiri.

**Compare & sync dan backup & restore sebagai satu paket.** Keduanya proyek tersendiri, dan
TablePro mengerjakannya dengan memanggil tool bawaan tiap engine plus reuse tunnel. Layak dicatat,
tidak layak dijadikan fase awal.

## 8. Posisi

TablePro adalah klien database umum untuk 36 engine; QueryHive adalah alat Trino yang mengalirkan
hasil ke sembilan format tanpa pernah menahan result set di memori. Dua hal di QueryHive yang tidak
dimiliki TablePro dalam bentuk yang sama: penulisan streaming dengan part splitting dan retry, dan
destinasi `to_table` yang menyerahkan penulisan ke koordinator sehingga barisnya tidak pernah
menyeberangi jaringan ke laptop.

Karena itu arah adopsinya bukan menambah engine, melainkan menutup hal-hal yang membuat alat ini
dipakai tiap hari: riwayat query, akses dari agen lewat MCP, impor, keamanan eksekusi, dan penjagaan
otomatis supaya yang sudah hijau tetap hijau.
