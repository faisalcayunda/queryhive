# Blueprint Fase 2: `EngineHost`, pool sesi, dan TTFR

- **Status:** blueprint tingkat berkas, 30 Sep 2026 (W2-A1). Belum ada kode yang ditulis. `architect-reviewer` sudah memeriksa blueprint ini (30 Sep 2026), dan koreksinya sudah diterapkan di badan dokumen. Verdict dan daftar keputusannya ada di bagian terakhir. AR memberi verdict lagi atas implementasinya sesudah W3-T1.
- **Untuk:** W3-T1 (FR-PERF-03, NFR-P1 S1/S3/S4, NFR-P8, NFR-S2). Pemeriksa sesuai `development-plan.md` W3-T1: RR, DB (§5, §7, §11), SEC (§4.1, §8), SF, TD, AR, CR.
- **Sumber:** `performance-plan.md` §5 (Fase 1) dan §6 (Fase 2); PRD §11 (O-6, O-7, P-05, P-06) dan NFR-S2; `development-plan.md` §4 (baris blueprint ini) dan §7 (kepemilikan berkas); `docs/invariants.md` #1 dan #11.
- **Prasyarat:** Fase 1 (W2-T1) sudah mendarat. Blueprint ini hanya memakai tiga perilaku darinya, apa pun nama akhirnya:
  - satu runtime per proses dari `qh_rt::build_main()` di balik `OnceLock`, yang aman di-`block_on` dari thread non-worker dan bisa di-`spawn` dari mana saja;
  - `CancelFlag` yang bisa di-await;
  - jalur cancel yang memanggil `session.cancel()` dengan batas 250 ms lalu menutup sesi.
- **Bukti:** kode di pohon ini dikutip dari commit `9892ddb`. Kode pihak ketiga dikutip dari `~/.cargo/registry` pada versi di `Cargo.lock`: tokio-postgres 0.7.18, mysql_async 0.36.2, russh 0.63.3.
- **Bahasa:** dokumen ini Indonesia. Identifier, komentar kode, pesan log, dan nama tes Inggris.

## Ringkasan

`EngineHost` adalah objek UniFFI yang hidup selama proses app. Ia memegang pool sesi per kunci koneksi dan satu handle SQLite lokal. `RustEngine.swift` memanggil `host.run(...)` dengan kontrak yang sama dengan `run(...)` bebas hari ini: sinkron, event lewat sink, dan galat sebagai event `error`.

Pool dipasang **di balik trait `Engine` yang sudah ada**:

- perintah tetap memanggil `open()` → `retry::connect(engine, …)` → `session.close()`;
- `engine` yang diberikan host mengembalikan sesi pinjaman;
- `close()` pada sesi pinjaman berarti "kembalikan ke pool dan reset".

Karena itu `commands.rs` hampir tidak berubah, dan `crate::run` untuk CLI, MCP, dan golden tidak berubah sama sekali. Wrapper `TunnelledSession` yang sudah ada diperluas menjadi satu-satunya wrapper sesi, bukan ditemani wrapper kedua.

Kode yang sekarang mengubah beberapa detail rencana (§1). Tiga temuan paling penting:

- sesi MySQL hari ini tidak pernah memakai ulang koneksinya, sehingga transaksi `apply_changes` di MySQL tidak atomik;
- `DISCARD ALL` akan merusak cache tipe tokio-postgres;
- pipelining `prepare` + `simple_query_raw` bisa deadlock pada tipe non-bawaan.

## 1. Temuan di kode yang mengubah rencana

1. **Sesi MySQL membuang koneksinya setelah setiap statement, jadi transaksi di MySQL tidak atomik hari ini.**
   - `run_statement` mengambil koneksi idle (`crates/qh-driver-mysql/src/lib.rs:305`) dan memindahkannya ke `Producer` (`:345-363`), yang menjatuhkannya saat selesai (`:616-618`). Statement berikutnya pada sesi yang sama membuka `Conn::new` baru (`:246-256`).
   - `apply_changes` menjalankan `BEGIN`, perubahan, lalu `COMMIT` lewat satu sesi (`crates/qh-ffi/src/apply.rs:140-145`, `:244`), dan `MysqlDriver` mengaku `transactions: true` (`:140`). Akibatnya `BEGIN` jalan di koneksi 1, setiap `UPDATE` autocommit di koneksi baru, dan `ROLLBACK` saat gagal tidak membatalkan apa pun, padahal hasilnya bisa dilaporkan sebagai "rolled back". `import_data` punya bentuk yang sama (`crates/qh-ffi/src/import.rs:329-362`).
   - Akibat lain dari akar yang sama (diperiksa AR):
     - `SET FOREIGN_KEY_CHECKS = 0` di `import_data` (`import.rs:218`) jalan di koneksi yang langsung dibuang, jadi baris impor berikutnya tetap diperiksa FK.
     - Pelacak `statement_timeout` sesi (`apply_max_execution_time`, `:1089-1104`) mencatat nilai yang dikirim ke koneksi 1. Statement kedua dengan batas yang sama di koneksi baru melewati `SET`, jadi jalan tanpa `max_execution_time`.
   - Tidak ada tes yang menangkapnya: `crates/qh-driver-mysql/tests/integration.rs` tidak punya kasus `BEGIN` atau `CONNECTION_ID()`.
   - Mengembalikan `Conn` dari producer ke sesi adalah prasyarat pool, dan sekaligus memperbaiki bug ini. Perbaikan ini masuk commit `fix(mysql)` tersendiri **sebelum** commit 1 (§15), karena bisa diuji tanpa pool.
   - Catatan untuk §6: driver MySQL memetakan **setiap** galat IO menjadi `EngineError::Connect`, termasuk yang terjadi saat query sudah berjalan (`:1071-1074`, pesannya "the connection failed while the query ran"). Jadi `Connect` dari MySQL tidak membuktikan statement belum terkirim.
2. **tokio-postgres tidak membuka status transaksi.** API publik `Client` (`tokio-postgres-0.7.18/src/client.rs:227-790`) tidak punya accessor status `ReadyForQuery`; yang ada hanya `__private_api_rollback` yang tersembunyi. Aturan "`ROLLBACK` bila status bukan idle" (`performance-plan.md` §6.3) tidak bisa ditulis tanpa fork. Karena itu reset selalu mengirim `ROLLBACK`. Di luar transaksi, server hanya menjawab dengan WARNING.
3. **`DISCARD ALL` merusak cache tipe tokio-postgres.**
   - Client menyimpan statement `typeinfo`, `typeinfo_composite`, dan `typeinfo_enum` yang ia prepare sendiri (`client.rs:74-81`, `prepare.rs:195-211`). `clear_type_cache()` hanya mengosongkan peta tipe (`client.rs:141-143`).
   - `DISCARD ALL` menjalankan `DEALLOCATE ALL`. Setelah itu, tipe non-bawaan berikutnya yang harus dicari client gagal dengan "prepared statement \"sN\" does not exist".
   - Karena itu reset memakai komponen `DISCARD ALL` yang didokumentasikan PostgreSQL, **tanpa** `DEALLOCATE ALL`, ditambah `DEALLOCATE` bernama untuk statement yang dibuat pengguna lewat SQL (§5).
4. **Pipelining `prepare` + `simple_query_raw` bisa deadlock.**
   - Setiap request punya kanal respons berkapasitas 1 (`client.rs:97`), dan task koneksi berhenti membaca socket saat kanal itu penuh (`connection.rs:146-166`).
   - Bila `prepare` menemukan OID non-bawaan (enum, domain, tipe ekstensi) yang belum ada di cache, ia mengirim query `typeinfo` **di belakang** query yang sudah di-pipeline (`prepare.rs:135-144`).
   - Jawaban `typeinfo` tertahan di belakang baris hasil yang tidak dibaca siapa pun, sampai pengguna menekan Stop.
   - Pipelining tetap dipakai, tetapi dengan penjaga dan fallback (§11.1).
5. **Trino tidak membatalkan query yang ditinggal.** `close` sengaja tidak mengirim `DELETE` (`crates/qh-driver-trino/src/lib.rs:1034-1040`), dan cursor yang mencapai `row_limit` berhenti membaca tanpa `DELETE` (`:1228-1233`). Preview yang terpotong hari ini meninggalkan query berjalan sampai Trino meninggalkannya sendiri.
6. **Pohon mengganti `DB_DATABASE` dan `DB_SCHEMA` per level** (`app/Sources/QueryHive/Models/AppModel.swift:1500-1540`). Bila keduanya masuk kunci pool, setiap katalog Trino, database MySQL, atau skema yang dibuka menjadi kunci baru dengan koneksi dingin, dan gate "level pohon hangat ≤ 1 RTT + 10 ms" tidak mungkin tercapai. Hanya database PostgreSQL yang benar-benar terikat ke koneksi (§4.1).
7. **App selalu mengirim `STATEMENT_TIMEOUT_MS`**, dengan default 60.000 (`AppModel.swift:3118`, `:2438-2448`). Reset mengembalikan timeout ke default server, jadi setiap Run sesudah reset membayar satu `SET`. Di PostgreSQL round trip ini ikut di-pipeline (§11.1).
8. **`count` tidak menguras cursor-nya** (`crates/qh-ffi/src/commands.rs:1652-1657`). Lease memutuskan reuse dari "cursor selesai bersih" (§4.4), jadi `count` harus membaca sampai `None`. Ongkosnya satu panggilan tambahan yang langsung selesai.
9. **`crates/qh-ffi/tests/golden.rs` dan `real_server.rs` bukan milik W3-T1** (`development-plan.md` §7). Semua tes baru masuk `crates/qh-ffi/tests/host.rs` (baru) dan tes integrasi driver. Paritas terhadap golden dibuktikan secara diferensial (T8, L12), tanpa mengubah `golden.rs`.
10. **`run` bebas UniFFI dipanggil `RustEngine.swift` (`:142`, `:160`) dan `crates/qh-ffi/examples/bench_ffi.rs:33`.** Pemanggil kedua ditambahkan di `b810592`, setelah commit yang dikutip blueprint ini, dan terlewat oleh grep awal yang hanya menyisir `app/`. `cargo test --workspace` dan `clippy --all-targets` (G-RUST) meng-compile example, jadi menghapus `run` tanpa memindahkan bench akan memerahkan G-RUST. Bench dipindah ke `EngineHost` di commit 1, dan memang harus begitu: W3-T3 mengukur jalur host, bukan jalur lama. Setelah itu `run` tidak punya pemanggil dan dihapus (D-11).
11. **Di luar lingkup, dicatat.** Jalur FFI app tidak pernah memasang execution log: `execution_log::install_from_settings` hanya dipanggil `main.rs:66` dan `bin/mcp.rs:91`. Akibatnya keputusan Safe Mode dari app tidak tercatat. Host tidak mengubah ini, karena itu keputusan fitur audit (W13).

## 2. Keputusan desain

| ID | Keputusan | Alasan |
|---|---|---|
| D-1 | `EngineHost` adalah `uniffi::Object` dengan tiga metode: konstruktor, `run` (sinkron, memblokir), dan `warm_up` (tidak memblokir). Tidak ada yang melempar galat. | Kontrak `run` hari ini dipertahankan: satu jalur galat, lewat event `error`. `warm_up` tidak punya pihak yang perlu diberi tahu. |
| D-2 | Pool dipasang di balik trait `Engine`: `PooledEngine::connect` adalah checkout, dan `Session::close` pada sesi pinjaman adalah checkin. | `commands.rs`, `apply.rs`, `retry.rs`, dan `crate::run` tidak berubah bentuk. CLI, MCP, dan golden memakai `crate::run` dengan `RealEngine` persis seperti hari ini. |
| D-3 | `TunnelledSession` (`lib.rs:393-502`) diganti nama menjadi `Held` dan menjadi satu-satunya wrapper sesi. Ia memegang tunnel (opsional) dan status pinjaman (opsional). | Wrapper yang sudah menyelesaikan masalah `Sync` untuk `cancel(&self)` dipakai ulang, tidak disalin. |
| D-4 | Kunci terdiri dari identitas server dan auth, ditambah hash kredensial. Nama database masuk kunci hanya untuk PostgreSQL. Untuk MySQL, yang masuk kunci hanya "ada database atau tidak". Database MySQL serta katalog dan skema Trino adalah konteks per Run lewat `Session::set_context`. | Temuan 6. MySQL tidak bisa kembali ke "tanpa database" setelah `USE`, jadi dua keadaan itu dipisah di kunci. |
| D-5 | 2+1 (O-7) adalah **reservasi atas sesi idle**, bukan antrean. Checkout tidak pernah menunggu, kecuali paling lama 250 ms untuk sesi yang sedang di-reset, diisi ulang, atau di-warm-up. Bila lane penuh, dibuka sesi baru di luar reservasi, dengan biaya yang sama seperti hari ini. | Antrean akan membuat Run di tab ketiga menunggu ekspor atau preview lambat di dua tab lain, dan membuat pohon menunggu query. Menunggu satu reset (1 RTT) lebih murah daripada connect (3–8 RTT). |
| D-6 | Reset dijalankan saat checkin, di latar, di luar jalur kritis. Reset yang gagal atau melewati 2 dtk membuat sesi dibuang. Sesi yang di-cancel, rusak, atau (khusus PostgreSQL) ditinggal di tengah stream tidak di-reset sama sekali, langsung dibuang. | O-6. Membuang sesi selalu aman, sedangkan reset setengah jalan tidak. Cancel yang terlambat (`CancelRequest` PostgreSQL, `KILL QUERY` MySQL) tidak bisa mengenai Run berikutnya, karena sesinya tidak pernah kembali ke pool. |
| D-7 | Reconnect karena koneksi basi hanya untuk statement baca, dan hanya pada eksekusi pertama sesi yang dipakai ulang. Pool tidak pernah mengulang write, dan aturan `retry.rs` tidak berubah. | `performance-plan.md` §6.2, `retry.rs:62-67`. |
| D-8 | Rute setiap perintah ditentukan oleh satu `match` tanpa wildcard di `host.rs`. | Perintah baru gagal compile sampai rutenya dipilih. Ini pola yang sama dengan penjaga invariant #11. |
| D-9 | `test` tidak memakai pool dan tidak memakai tunnel bersama. `table_op` masuk lane query. | `test` harus membuktikan connect dan SSH dari nol. Sesi pool akan membuat "Test" lulus meski password sudah diganti di server. `table_op` interaktif dan pendek, tidak ada di daftar P-05 mana pun, dan bukan operasi berdurasi menit. |
| D-10 | Tunnel dibagi per kunci lewat `Arc<dyn TunnelHandle>`, dan setiap sesi memegang klonnya. Export, `to_table`, dan `import_data` (P-05) memakai tunnel kunci yang sama. | P-05. Trait kecil itu juga menjadi seam tes: tunnel palsu bisa menghitung open dan drop tanpa SSH. |
| D-11 | `run` bebas UniFFI dihapus. `SinkEmitter`, `fail`, dan konversi `Setting` menjadi `pub(crate)` dan dipakai host. `examples/bench_ffi.rs` pindah ke `EngineHost::new()` + `host.run(...)` di commit yang sama. | Temuan 10. Hasilnya satu jalur FFI, satu tempat runtime dipakai, dan tidak ada pintu belakang yang melewati pool. |
| D-12 | Satu handle SQLite per path `DB_PATH`, dibuka dan dimigrasi sekali, di balik `Mutex`. Hanya host yang memasangnya; `crate::run` tetap membuka handle per panggilan. | `performance-plan.md` §6.1. Perilaku CLI dan MCP tidak berubah. |
| D-13 | Tidak ada dependensi baru, dan `Cargo.lock` tidak berubah. | `Cargo.toml` milik W4-T3 (`development-plan.md` §7). Hash kredensial memakai `std::hash::RandomState`, dan pipelining memakai `futures-util` yang sudah ada di driver PostgreSQL. |

## 3. Bentuk UniFFI `EngineHost`

```rust
// crates/qh-ffi/src/host.rs
#[derive(uniffi::Object)]
pub struct EngineHost {
    pool: Arc<SessionPool>,
    storage: local::SharedStorage,
}

#[uniffi::export]
impl EngineHost {
    /// No I/O: an empty pool, SQLite opened on the first local command, the Fase 1
    /// runtime built the first time it is needed.
    #[uniffi::constructor]
    pub fn new() -> Arc<Self>;

    /// Today's free `run` contract: blocks until the command has ended, events go to
    /// `sink` on the calling thread, a failure is an `error` event.
    pub fn run(&self, command: EngineCommand, settings: Vec<Setting>,
               sink: Arc<dyn EventSink>, cancel: Arc<RunCancel>);

    /// Opens one session in the background for the key these settings describe, when
    /// that key has none. Returns at once. Failures are dropped.
    pub fn warm_up(&self, settings: Vec<Setting>);
}

// Rust only, not exported through UniFFI (tests and bench).
impl EngineHost {
    pub fn with_connector(connector: Arc<dyn Connector>) -> Arc<Self>;
    pub fn pool(&self) -> &SessionPool;   // stats(), settle()
}
```

Swift yang dihasilkan: `EngineHost()`, `host.run(command:settings:sink:cancel:)`, dan `host.warmUp(settings:)`. Tipe `EngineCommand`, `Setting`, `EventSink`, dan `RunCancel` dipakai ulang tanpa perubahan, jadi empat daftar invariant #11 tidak tersentuh.

**Threading.**

- `run` memanggil `block_on` pada runtime Fase 1 di thread pemanggil. Swift memanggilnya dari antrean `.userInitiated` (Fase 1.3), tidak pernah dari main. Beberapa `run` boleh berjalan bersamaan dari thread berbeda.
- `warm_up` hanya mem-parse settings lalu men-`spawn` ke runtime. Swift tetap memanggilnya dari antrean `.utility`, karena panggilan pertama bisa membangun runtime.
- Pekerjaan latar (reset, isi ulang, warm-up, evict) berjalan sebagai task di runtime yang sama. Runtime tidak pernah di-shutdown (Fase 1), jadi task yang umurnya melewati satu `run` aman.
- Lock di pool adalah `std::sync::Mutex` yang tidak pernah dipegang melewati `await`. Satu-satunya lock async adalah pembukaan tunnel per kunci (`tokio::sync::Mutex`), supaya dua checkout bersamaan tidak membuka dua tunnel.

**Galat.** Tidak ada tipe galat UniFFI baru.

- Runtime yang gagal dibangun dilaporkan sebagai event `error`, lewat jalur Fase 1.
- `warm_up` yang gagal (host key tidak dikenal, password salah, server mati) tidak meninggalkan state apa pun. Perintah sungguhan berikutnya melaporkan galat yang sama lewat jalur biasa.

**Rute.** `fn route(command: Command) -> Route` adalah satu `match` tanpa wildcard:

| Rute | Perintah | Engine yang dioper ke `crate::run_with` |
|---|---|---|
| `Local` | `connections`, `import_connections`, `credential`, `history`, `history_add`, `history_clear`, `saved_queries`, `session`, `account`, `profiles`, `profile_save`, `profile_delete` | engine koneksi pool (tidak dipakai), ditambah `Some(storage)` |
| `Fresh` | `db_drivers`, `test` | `RealEngine::with_settings(settings)`, seperti hari ini |
| `Pooled(Metadata)` | `catalogs`, `schemas`, `tables`, `objects` | `PooledEngine { lane: Metadata }` |
| `Pooled(Query)` | `preview`, `explain`, `count`, `apply_changes`, `table_op` | `PooledEngine { lane: Query }` |
| `LongOp` | `export`, `to_table`, `import_data` (dan `script` di W12) | `LongOpEngine`: sesi baru, dengan tunnel kunci yang dipakai bersama |

Perintah server baru di W11–W13 (misalnya aktivitas server, P-05: lane query) memilih rutenya di `match` ini.

**`crate::run` bebas tetap ada.** Signature `run(command, settings, out, engine, cancel)` tidak berubah. Isinya menjadi `run_with(..., storage: None)`. `main.rs`, `bin/mcp.rs`, `tests/golden.rs`, dan semua tes yang memanggil `crate::run` dengan `RealEngine` atau `FakeEngine` tidak tersentuh.

## 4. `SessionPool`

### 4.1 Kunci

`PoolKey { identity: Identity, credential: u64 }`. `Identity` punya `Hash` dan `Eq`, dan `Debug`-nya ditulis tangan.

| Masuk `identity` | Masuk `credential` (hash) | Konteks per Run, tidak masuk kunci | Tidak masuk sama sekali |
|---|---|---|---|
| `kind`; `host`; `port` (default driver sudah diisi oleh `commands::connection`); `user`; `tls`; `insecure`; `database`: nama untuk PostgreSQL, `Unset` atau `Switchable` untuk MySQL, `Switchable` untuk Trino; tunnel: `host`, `port`, `user`, jenis auth (`agent`, `key` + path, `password`), path `known_hosts` | `DB_PASSWORD`, `SSH_PASSWORD`, `SSH_KEY_PASSPHRASE` (JWT Trino menyusul di W11) | nama database MySQL; katalog dan skema Trino; skema PostgreSQL (hanya dipakai sebagai `ObjectPath`, bukan state sesi) | SQL, `LIMIT`, `STATEMENT_TIMEOUT_MS`, `SAFE_MODE*`, `DB_READ_ONLY`, `RETRIES`, `DB_ALL_SCHEMAS`, dan setelan lainnya |

- **Kunci dibangun dari config yang sudah di-resolve, dengan destrukturisasi lengkap (koreksi AR).**
  - `PoolKey::of(&ConnectionConfig, &Settings)` membongkar `ConnectionConfig` dan `TunnelConfig` **tanpa `..`**. Field baru, misalnya CA per koneksi (P-08, W11) atau JWT Trino, gagal compile sampai ditempatkan di salah satu dari empat kolom tabel di atas. Tanpa penjaga ini, sesi dengan trust atau kredensial lama bisa dipakai ulang diam-diam.
  - Password diambil dari `config.password` yang sudah di-resolve, bukan dari setelan `DB_PASSWORD` mentah. Password di dalam `DB_URL` (`user:secret@host`) mendarat di field itu (`crates/qh-ffi/src/config.rs:433`), jadi ikut membatalkan kunci. `DB_URL` mentah tidak pernah masuk kunci.
  - Password SSH diambil dari `TunnelAuth::Password`. Hanya passphrase yang dibaca dari settings, seperti yang dilakukan `tunnel::bastion`.
- **Hash kredensial (NFR-S2).**
  - SipHash-1-3 64 bit dari satu `std::hash::RandomState` per proses (`OnceLock`), jadi kuncinya acak per proses dan hanya ada di memori.
  - Setiap bagian di-hash sebagai (ada/tidak, panjang, byte), supaya `("ab", "")` dan `("a", "b")` tidak bertabrakan.
  - Nilainya tidak pernah ditulis ke log, event, atau disk.
  - `Debug` untuk `PoolKey` mencetak identitas dengan gaya `ConnectionConfig::redacted()`, ditambah `credential: <hashed>`.
- `TlsMode` mendapat `derive(Hash)` di `qh-driver`.
- **Entry per identitas.** Pool memegang `HashMap<Identity, Arc<Entry>>`, dan satu identitas hanya punya satu entry. Checkout dengan `credential` berbeda **memensiunkan** entry lama:
  - sesi idle-nya ditutup saat itu juga;
  - sesi yang sedang dipinjam dibuang saat checkin;
  - entry baru dibuat.

  Inilah "perubahan kredensial membatalkan kunci", tanpa menunggu evict 5 menit.
- **Password di memori.** Password ada di memori pool (di `ConnectionConfig` yang disimpan lease untuk reconnect) dan di dalam sesi driver seperti hari ini. Ia tidak pernah masuk kunci, log, atau event.

### 4.2 Lane dan checkout

Konstanta: `QUERY_SESSIONS = 2`, `METADATA_SESSIONS = 1`, `IDLE_MAX = 3` (O-7).

State per entry:

- di bawah `std::sync::Mutex`: `idle: Vec<Idle>` (LIFO), `leased: [usize; 2]` per lane, `pending: usize` (reset, isi ulang, atau warm-up yang sedang menuju idle), dan `retired: bool`;
- `Notify` untuk perubahan `pending`;
- `tunnel: tokio::sync::Mutex<Option<Arc<dyn TunnelHandle>>>`.

`PooledEngine::connect(config)`:

1. Hitung kunci. Ambil atau buat entry, dan pensiunkan entry lama bila kredensial berubah.
2. Di bawah lock: bila `leased[lane] < cap[lane]` dan `idle` tidak kosong, ambil sesi idle terbaru. `reused = true`.
3. Bila tidak ada idle, `leased[lane] < cap[lane]`, dan `pending > 0`: lepas lock, tunggu `Notify` paling lama `CHECKIN_WAIT = 250 ms`, lalu ulangi langkah 2 sekali.
4. Selain itu, pastikan tunnel kunci siap. Tunnel dibuka bila kosong atau `!is_alive()`; tunnel yang mati juga membuat semua sesi idle entry itu ditutup. Lalu connect baru lewat `Connector`, dengan `reused = false`.
   - Langkah ini juga yang terjadi bila lane penuh: sesi dibuka di luar reservasi, dengan biaya hari ini, dan tidak ada yang menunggu.
5. `leased[lane] += 1` lewat guard RAII. Guard itu mengurangi hitungan saat lease berakhir, **termasuk bila future checkout di-drop di tengah connect** (Stop dari Fase 1).
6. Panggil `session.set_context(config.database, config.schema)` setiap kali, termasuk dengan `None`, supaya konteks Run sebelumnya tidak terbawa.
7. Kembalikan `Held { inner, tunnel, lease: Some(LeaseState { … }) }`.

`retry::connect` tetap membungkus semua langkah ini, jadi `RETRIES` berlaku untuk checkout seperti untuk connect hari ini.

### 4.3 Checkin dan reset

Checkin terjadi di `Held::close`, **atau** di `Drop` bila sesi dijatuhkan tanpa `close` (misalnya perintah keluar lewat `?` setelah galat server). Lease selalu kembali lewat salah satu jalan itu.

1. Guard lane dilepas, dan `pending += 1` (juga lewat guard RAII).
2. **Sesi dibuang tanpa reset** bila salah satu kondisi ini benar:
   - `cancelled`;
   - `broken`;
   - entry `retired`;
   - tunnel mati;
   - driver PostgreSQL dan cursor terakhirnya ditinggal (§4.4).
3. Selain itu, `spawn` ke runtime: `timeout(RESET_BUDGET = 2 s, inner.reset())`.
   - `Ok`: bila `idle.len() < IDLE_MAX`, sesi masuk idle dengan stempel waktu. Bila tidak, sesi ditutup.
   - `Err` atau lewat batas: sesi ditutup (best effort), tanpa galat ke pengguna, karena Run-nya sudah selesai.
4. `pending -= 1`, lalu `notify_waiters()`.
5. (Commit 2) Bila sesi dibuang karena preview PostgreSQL yang terpotong, dan entry tidak punya idle maupun pending, `spawn` isi ulang: satu connect ke idle, yang juga dihitung di `pending`.

Checkin tidak menahan `done`. `close()` hanya men-`spawn` lalu langsung kembali, jadi urutan event tidak berubah.

### 4.4 Lease (`Held`)

```rust
// crates/qh-ffi/src/lib.rs (was TunnelledSession)
pub(crate) struct Held {
    inner: tokio::sync::Mutex<Option<Box<dyn Session>>>,
    capabilities: Capabilities,          // mirrored, as today
    query_id: Option<String>,            // mirrored after execute, as today
    tunnel: Option<Arc<dyn TunnelHandle>>,
    lease: Option<host::LeaseState>,     // Some = borrowed from a pool
}
```

`LeaseState` memegang:

- `Arc<SessionPool>`, `Arc<Entry>`, dan lane-nya;
- `ConnectionConfig` Run ini (untuk reconnect);
- `reused`, `executed`, dan `broken`;
- `cancelled: AtomicBool`, karena `cancel` menerima `&self`;
- `last_cursor: Option<Arc<CursorEnd>>`.

Aturannya:

- **Forward.** Semua metode `Session` diteruskan seperti di `TunnelledSession` hari ini, termasuk `execute_bound` (komentar `lib.rs:441-446` tetap berlaku).
- **Cursor yang dilacak.** `execute` dan `execute_bound` membungkus cursor dengan `Tracked { inner, end: Arc<CursorEnd>, row_limit, delivered }`.
  - `end.clean = true` bila `next_batch` mengembalikan `Ok(None)` **dan** `row_limit` belum tercapai (`delivered < row_limit`), atau bila `next_batch` mengembalikan `Err` yang **membawa kode server** (`EngineError::code().is_some()`), karena hanya jawaban server yang membuktikan stream sudah berakhir di server. `Err` tanpa kode (galat decode di klien, transport) tidak membuat cursor bersih (koreksi AR).
  - Cursor yang di-drop tanpa salah satu keadaan itu dianggap **ditinggal**.
  - Saat checkin, cursor yang belum `clean` dihitung ditinggal, **termasuk cursor yang masih hidup** karena perintah memanggil `close()` sebelum menjatuhkannya (koreksi AR).
  - Cursor yang berhenti karena `row_limit` juga dianggap ditinggal, karena yang menghentikannya adalah batas, bukan akhir hasil di server.
- **`cancel`** menyetel `cancelled`, lalu meneruskan ke `inner.cancel()` (server-side, lewat jalur Fase 1).
- **Menukar `inner`** (reconnect, §6) menyetel `broken = true` sebelum `await` pertama, dan menghapusnya setelah berhasil. Future yang di-drop di tengah reconnect meninggalkan `broken`, dan sesi itu dibuang saat checkin.
- **`Drop` tanpa `close`:** bila `lease` masih ada, `inner` diambil dan checkin dijalankan lewat `tokio::runtime::Handle::try_current()`. Di luar runtime (tidak terjadi di jalur host), sesi hanya dijatuhkan, dan guard mengembalikan hitungannya.
- **Tanpa lease.** `Held` tanpa lease (CLI lewat tunnel, atau `LongOp` lewat tunnel) berperilaku persis seperti `TunnelledSession` hari ini.

### 4.5 Evict

Satu task per pool dimulai saat sesi pertama masuk idle. Ia memegang `Weak<SessionPool>` dan berhenti bila pool sudah tidak ada. Setiap `EVICT_TICK = 30 s`:

- sesi idle yang lebih tua dari `IDLE_TTL = 5 menit` dikeluarkan di bawah lock, lalu ditutup di luar lock;
- entry dihapus bila `idle`, `leased`, dan `pending` semuanya nol, **dan** tunnel-nya hanya dipegang entry itu (`Arc::strong_count == 1`). Ekspor yang masih berjalan menahan entry beserta tunnel-nya.

### 4.6 Statistik untuk tes

`SessionPool::stats() -> PoolStats { idle, leased_query, leased_metadata, pending, opened, reused, closed, tunnels_opened }`, dan `SessionPool::settle().await`, yang menunggu sampai `pending == 0`. Keduanya publik di Rust dan tidak diekspor ke UniFFI.

## 5. Reset per driver

`qh-driver` mendapat dua metode `Session` baru:

- `reset(&mut self) -> Result<(), EngineError>`. **Default-nya menolak** (`EngineError::Usage`), jadi driver atau sesi palsu yang tidak mengimplementasikannya tidak pernah dipakai ulang.
- `set_context(&mut self, database: Option<&str>, schema: Option<&str>)`. Default-nya tidak melakukan apa-apa.

| Driver | `reset` | `set_context` |
|---|---|---|
| PostgreSQL | Bila `client.is_closed()`, hasilnya `Err`. Selain itu, tiga pesan di-pipeline dalam satu penerbangan (`try_join3`; urutan poll = urutan kirim):<br>(1) `ROLLBACK`;<br>(2) `batch_execute("CLOSE ALL; SET SESSION AUTHORIZATION DEFAULT; RESET ALL; UNLISTEN *; SELECT pg_advisory_unlock_all(); DISCARD PLANS; DISCARD TEMP; DISCARD SEQUENCES")`, yaitu `DISCARD ALL` tanpa `DEALLOCATE ALL` (temuan 3);<br>(3) `SELECT name FROM pg_prepared_statements WHERE from_sql`.<br>Bila (3) mengembalikan nama, satu `batch_execute` berisi `DEALLOCATE <nama ter-quote>` untuk masing-masing.<br>Terakhir, `self.statement_timeout = None`, karena `RESET ALL` mengembalikan timeout ke default role. Satu RTT dalam kasus biasa. | Tidak ada. Database ada di kunci, dan skema hanya `ObjectPath`. |
| MySQL | Tunggu `Conn` kembali dari producer (§7). Batas waktunya `RESET_BUDGET` pool. Tanpa `Conn`, hasilnya `Err`.<br>`conn.reset()` menjalankan `COM_RESET_CONNECTION` (`mysql_async-0.36.2/src/conn/mod.rs:1162-1177`). Bila server tidak mendukungnya (`false`), pakai `conn.change_user(ChangeUserOpts::default())`.<br>Lalu `SELECT DATABASE()` untuk mengisi `current_db`, karena `USE` di SQL pengguna mengubah database tanpa sepengetahuan driver. Sesi di entry `Unset` yang ternyata punya database dikembalikan sebagai `Err`, karena MySQL tidak bisa kembali ke "tanpa database".<br>Terakhir, `statement_timeout = None`, `select_limit = None` (commit 2), dan `connection_id = 0`. | Simpan `wanted_db`. Sebelum statement berikutnya, bila `wanted_db` ada dan berbeda dari `current_db`, kirim ``USE `db` `` (di-quote lewat `qh_sql::quote_ident`) lalu perbarui `current_db`. `opts` ikut diperbarui, supaya koneksi baru dalam sesi itu masuk ke database yang sama. |
| Trino | Tidak ada state koneksi. Bila `running` masih ada (preview terpotong), kirim `DELETE` ke `nextUri`-nya. Isi `cancel` diekstrak menjadi `delete_running()` dan dipakai keduanya.<br>Galat `DELETE` diabaikan: tidak ada yang kotor di sisi klien, dan Trino akan meninggalkan query itu sendiri.<br>`last_id = None`. `reqwest::Client` dan keputusan fallback `Prefer` dipertahankan, karena itulah yang dipakai ulang. | `catalog` dan `schema` sesi diganti dengan nilai Run, atau string kosong bila `None`. |

**Bila reset gagal.** Sesi ditutup dan dilupakan. Tidak ada event, tidak ada percobaan reset kedua, dan tidak ada isi ulang: server yang menolak reset bisa sedang bermasalah, dan isi ulang hanya menambah bebannya. Run berikutnya membuka sesi baru lewat jalur checkout biasa.

**Yang sengaja tidak di-reset.**

- Cache tipe tokio-postgres. Ia per client dan tetap valid seumur koneksi, karena reset tidak men-`DEALLOCATE` statement protokol.
- Cache statement `mysql_async`. Yang ini dikosongkan oleh `Conn::reset` sendiri.

## 6. Reconnect untuk baca

Idle NAT atau server yang restart meninggalkan sesi idle yang sudah mati. Checkout tidak melakukan ping (`performance-plan.md` §6.2). Sebagai gantinya, `Held::execute`, `execute_bound`, `browse`, dan `objects` menjalankan aturan berikut:

Satu predikat menentukan boleh-tidaknya statement dijalankan di koneksi pengganti: `rerunnable(sql) = qh_sql::classify(sql) == StatementKind::ReadOnly || sql` sama persis dengan `"BEGIN"` atau `"START TRANSACTION"` (teks yang dikirim `apply.rs` dan `import.rs` sendiri). `browse` dan `objects` selalu `rerunnable`. `BEGIN` aman diulang, karena transaksi di koneksi yang mati ikut mati bersama koneksinya.

1. Bila `broken` **dan** statement ini `rerunnable`, ganti `inner` dengan sesi baru dari `Connector`, lewat tunnel kunci yang sama, sebelum menjalankan apa pun. Bila `broken` dan statement ini **tidak** `rerunnable`, kembalikan galat `Connect` yang ada ("the pooled connection was lost; the statement was not sent again") tanpa menyentuh server.
2. Bila ini eksekusi pertama sesi pakai ulang (`reused && !executed`), galatnya membuktikan server tidak menjawab (`retry::produced_no_page(&error)`, dijadikan `pub(crate)`, perilakunya tidak berubah), dan statement-nya `rerunnable`: reconnect sekali lalu jalankan ulang.
3. Semua galat lain diteruskan apa adanya. Galat tanpa jawaban server menyetel `broken`. Karena langkah 1, retry dari `retry::execute` hanya mendapat koneksi baru untuk statement `rerunnable`. Write yang di-retry tetap jatuh ke sesi yang rusak dan gagal cepat, persis seperti PostgreSQL hari ini (client mati, `is_closed`).

**Kenapa bukan "`Connect` berarti belum terkirim, jadi ulangi statement apa pun" (versi awal blueprint, ditolak AR).** MySQL memetakan galat IO di tengah query menjadi `Connect` (temuan 1). Write yang sudah dieksekusi server, tetapi jawabannya hilang, akan dijalankan dua kali tanpa tanda apa pun. Rencana (`performance-plan.md` §6.2) juga hanya meminta reconnect untuk statement baca. Versi awal juga membuat `broken` memberi `retry::execute` koneksi segar untuk write, sehingga `UPDATE` yang di-Run di editor bisa terulang di PostgreSQL. Hari ini hal itu tidak terjadi, karena retry jatuh ke client yang sudah mati.

Reconnect di langkah 2 tidak dihitung sebagai retry `RETRIES` dan tidak memakai backoff, karena yang ia tebus adalah artefak pool (sesi yang menganggur), bukan kecelakaan server. Pool tidak pernah mengulang write. Aturan `retry.rs` untuk write, termasuk `to_table` yang sama sekali tidak memakai `retry::execute` (`retry.rs:62-67`), tidak berubah. Ongkosnya: write pertama pada sesi idle yang sudah basi gagal sekali, dengan pesan yang jelas, dan Run berikutnya mendapat sesi baru (R-5).

Untuk mengurangi sesi basi, commit 2 memasang keepalive TCP 60 dtk: `tokio_postgres::Config::keepalives_idle` untuk PostgreSQL dan `OptsBuilder::tcp_keepalive` untuk MySQL. Koneksi lewat tunnel sudah punya keepalive SSH 30 dtk (`crates/qh-tunnel/src/tunnel.rs:157`).

## 7. Preview ber-cap pada sesi pool

`stream_rows` mengoper `row_limit: Some(limit + VERDICT_FETCH)` (commit 2). Semantik event tidak berubah:

- `emit_batches` tetap memotong dan mengambil satu baris vonis;
- cursor yang berhenti di `limit + 1` memberi baris yang sama dengan hari ini;
- golden memakai `FakeCursor` yang mengabaikan `row_limit` (`crates/qh-ffi/tests/golden.rs:337-375`).

| Driver | Preview yang terpotong |
|---|---|
| PostgreSQL | Cursor dianggap ditinggal (§4.4). Sesi **dibuang** seperti hari ini, lalu diisi ulang di latar (§4.3 langkah 5). Menjatuhkan client menutup socket, dan backend berhenti saat menulis berikutnya.<br>Cancel lalu drain tidak dipakai, karena `CancelRequest` yang terlambat bisa mengenai statement berikutnya, dan drain bisa berlangsung selama sisa hasilnya. |
| MySQL | `row_limit` diterjemahkan menjadi `SET SESSION sql_select_limit = limit+1`, digabung dengan `max_execution_time` dalam satu `SET` bila salah satunya berubah. Server berhenti di `limit + 1` untuk `SELECT` tingkat atas, producer membaca sampai EOF, `Conn` kembali ke sesi, dan reset mengembalikan `sql_select_limit` ke default.<br>**Hanya untuk statement tunggal yang `qh_sql::classify` sebut `ReadOnly`** (koreksi AR). Statement lain (`INSERT … SELECT`, `CREATE TABLE … SELECT`, `SELECT … INTO`, `SELECT … FOR UPDATE`, `CALL`) tidak pernah dikirim di bawah `sql_select_limit` selain default, supaya batas preview tidak pernah bisa mengurangi baris yang ditulis atau dikunci. Cursor-nya tetap berhenti di `row_limit` di sisi klien.<br>Bila cursor ditinggal sebelum EOF (`LIMIT` eksplisit yang lebih besar, `SHOW`, statement di luar aturan di atas), `Conn` dijatuhkan, reset gagal, dan sesi dibuang, sama seperti commit 1. Drain `DRAIN_ROWS` dari versi awal blueprint dihapus: menguras sampai 10.000 baris lewat kawat lebih mahal daripada connect baru, dan rencana tidak memintanya. |
| Trino | Cursor berhenti di `row_limit`, lalu reset mengirim `DELETE` ke `nextUri` (§5). `close` tanpa pool (CLI) tetap tidak mengirim `DELETE`, dengan alasan yang tertulis di `lib.rs:1035-1038`. |

**Aturan producer MySQL** (sudah di commit `fix(mysql)`, §15):

- `Conn` dikembalikan lewat `oneshot` saat hasil selesai dan saat galat **server** (`mysql_async::Error::Server`), **sebelum** `sender` dijatuhkan. Dengan begitu, `None` di cursor berarti `Conn` sudah siap dipakai statement berikutnya pada sesi yang sama. Galat lain (IO, decode) menjatuhkan `Conn`, karena result set-nya belum tentu habis.
- `consume` mengembalikan `Complete` atau `Abandoned`. Pada `Abandoned`, `Conn` dijatuhkan, di semua commit.
- `MysqlSession::connection()` menunggu `Conn` yang sedang kembali sebelum membuka yang baru. Sender `oneshot` yang dijatuhkan berarti `Conn` hilang, dan barulah dibuka koneksi baru.
- **`connection()` tidak pernah menunggu cursor yang masih hidup** (koreksi AR). Bila cursor statement sebelumnya belum selesai dan belum di-drop, producer bisa tertahan di `send` (kanal `BATCH_BACKLOG` penuh), sehingga menunggunya berarti deadlock. Membuka koneksi kedua diam-diam adalah bug temuan 1 dalam bentuk lain. Karena itu `connection()` mengembalikan `EngineError::Usage("a statement on this session is still being read")`. Hari ini tidak ada jalur di `commands.rs`, `apply.rs`, maupun `import.rs` yang melakukannya; aturan ini menjaga yang akan datang. Cara sesi mengetahui cursornya masih hidup (flag bersama yang dilepas oleh `Drop` cursor dan oleh akhir stream) diserahkan ke implementer.

Commit 1 sudah benar tanpa optimasi di tabel ini. Preview MySQL yang terpotong membuat producer menjatuhkan `Conn`, reset gagal, dan sesi dibuang, sama seperti PostgreSQL. Commit 2 adalah optimasi yang menyimpan sesinya untuk preview `SELECT` tanpa `LIMIT` eksplisit.

## 8. Tunnel

- **Seam.** `pub trait TunnelHandle: Send + Sync + Debug { fn local_port(&self) -> u16; fn is_alive(&self) -> bool; }`, diimplementasikan untuk `qh_tunnel::Tunnel`.
  - `Tunnel` menyimpan klon `Arc<Mutex<Handle>>`.
  - `is_alive()` mencoba `try_lock()`, lalu mengembalikan `!handle.is_closed()` (`russh-0.63.3/src/client/mod.rs:313`). Lock yang sedang dipegang dianggap hidup.
- **Satu per kunci.** Entry membuka tunnel paling banyak sekali, di bawah `tokio::sync::Mutex` per entry.
  - Sesi pool, sesi di luar reservasi, dan sesi `LongOp` memegang klon `Arc` yang sama.
  - `tunnel::bastion` membaca `SSH_KEY_PASSPHRASE` dari settings Run yang membuka tunnel, seperti hari ini.
- **Umur.** Entry memegang satu `Arc` selama entry hidup, dan entry hanya dihapus oleh evict (§4.5). Tunnel tertutup saat `Arc` terakhir dijatuhkan, yaitu setelah sesi idle terakhir kunci itu kedaluwarsa dan tidak ada ekspor yang masih memakainya.
- **Tunnel mati.** Checkout yang menemukan `!is_alive()` menjatuhkan `Arc` entry dan menutup sesi idle entry itu (socket mereka lewat tunnel yang sama), lalu membuka tunnel baru.
- **Host key.** Tidak berubah: host yang tidak dikenal ditolak dengan `FailureKind::Permanent` (`lib.rs:349-357`), dan tidak ada yang tersimpan di entry.
- **`test`** membuka tunnelnya sendiri (rute `Fresh`), jadi Test Connection tetap menguji SSH dari nol.
- **Refaktor.** Isi `RealEngine::connect` (`lib.rs:332-372`) dipindah ke dua tempat:
  - `tunnel::open(description, settings, target) -> Result<qh_tunnel::Tunnel, EngineError>`;
  - `RealConnector::connect(config, tunnel)`.

  `RealEngine::connect` memanggil keduanya lalu membungkus hasilnya dengan `Held { tunnel, lease: None }`, jadi perilaku CLI tetap sama.

## 9. Handle SQLite lokal

- **Tipe.** `local::SharedStorage(Arc<Mutex<Option<(String, Storage)>>>)`. Kuncinya string `DB_PATH` setelah `expand_user`, dan string kosong berarti path default `qh_storage::open_default()`.
- **Akses.** `local::with_storage(settings, shared: Option<&SharedStorage>, work)`:
  - tanpa `shared`: buka dan migrasi seperti `open_storage` hari ini (CLI, MCP, tes);
  - dengan `shared`: pakai handle yang ada bila path-nya sama. Bila berbeda, buka dan migrasi path baru, lalu ganti isinya.

  Semua ini berjalan di dalam closure `on_blocking` yang sudah ada, jadi SQLite tetap tidak pernah berjalan di worker tokio.
- **Pemanggil.** Dua belas fungsi lokal menerima `Option<SharedStorage>` (klon `Arc`, supaya closure-nya `'static`). `crate::run_with` mengopernya, dan `crate::run` mengoper `None`.
- **Aman untuk banyak proses.** `Storage` sudah memakai WAL dan `busy_timeout` 5 dtk (`crates/qh-storage/src/lib.rs:225-237`), jadi handle yang terus terbuka tetap aman bersama proses CLI atau MCP yang menulis ke berkas yang sama.
- **Tes.** `SharedStorage::opens()` (doc-hidden) menghitung pembukaan untuk T10.

## 10. Warm-up

- **Rust.** `warm_up(settings)`:
  1. `commands::connection(settings, engine)`. Settings yang tidak valid membuat fungsi langsung kembali.
  2. Hitung kunci. Bila entry sudah punya sesi idle, sesi yang dipinjam, atau pending, kembali.
  3. Selain itu, `pending += 1` lalu `spawn`: tunnel, connect, `set_context`, masuk idle.

  Hanya connect, tanpa query (`performance-plan.md` §6.6). Prefetch data tetap ditolak (§12 rencana itu). Untuk Trino, connect tidak melakukan I/O, jadi TCP dan TLS baru terjadi di `POST` pertama.
- **Swift, `RustEngine`.** `func warmUp(env:)` men-`dispatch` ke `DispatchQueue.global(qos: .utility)`, lalu memanggil `Self.host.warmUp(settings:)`.
- **Swift, `DatabaseEngine`.** `func warmUp(env: [String: String])` masuk protokol dengan extension default yang kosong, jadi `MockEngine` tidak berubah.
- **Swift, `AppModel` (commit 2).** Satu helper privat `warmUp(for tab:)` membangun environment yang sama dengan Run: `connectionEnvironment(connection)`, ditambah `DB_DATABASE = database(for: tab)` dan `DB_SCHEMA = schema(for: tab)`. Tujuannya supaya kunci PostgreSQL, yang memuat nama database, sama dengan kunci Run. Helper ini dipanggil di tiga tempat:
  - di akhir `selectTab(_:)` (`AppModel.swift:763`);
  - di akhir `newTab(connectionID:)` (`:608`);
  - setelah restore sesi memilih tab, sehingga S4 (Run pertama setelah app dibuka) menemukan sesi hangat.
- **Pohon tidak berubah.** `loadChildren`, `loadCatalogs`, dan `loadSchemas` (`AppModel.swift:1487-1640`) sudah lewat `Engine.current.run`, dan host mengarahkannya ke lane metadata.

## 11. Hemat round trip

Hitungan di bawah adalah jumlah round trip protokol, bukan angka ukur. Angka ukurnya datang dari W3-T3.

| Jalur | Hari ini (koneksi baru per Run) | Commit 1 (pool) | Commit 2 |
|---|---|---|---|
| Preview PostgreSQL hangat | TCP 1, TLS 0–2, startup dan SCRAM 2–3, `SET` 1, `prepare` 1, query 1 | `SET` 1, `prepare` 1, query 1 | 1 (`SET`‖`prepare`‖query) untuk statement baca di sesi yang sudah pernah menjalankan statement; 2 selain itu |
| Level pohon PostgreSQL | connect seperti di atas, lalu 1 | 1 | 1 |
| Preview MySQL hangat | TCP 1, handshake dan auth 2, TLS 0–2, `SET` 1, `prep` 1, query 1 | `SET` 1, `prep` 1, query 1, ditambah `USE` 1 bila database berganti | `SET` 1 (timeout dan `sql_select_limit` sekaligus), query 1 |
| Trino | TCP dan TLS 2–3, lalu `POST` dan poll | `POST` dan poll, lewat keep-alive client yang sama | sama |

### 11.1 PostgreSQL: pipelining

- **Selalu.** `SET statement_timeout` (bila berubah) di-pipeline dengan `prepare`. Keduanya mengonsumsi responsnya sendiri, jadi tidak ada kanal yang tertahan.
  - Nilai timeout di-clamp ke `i32::MAX` ms di `apply_statement_timeout`, supaya `SET` tidak bisa gagal karena rentang lalu meninggalkan query tanpa batas.
  - Fungsi itu dipecah menjadi `timeout_statement(current, wanted) -> Option<String>`, supaya bisa dipakai di jalur pipelining.
- **Bersyarat.** `prepare` dan `simple_query_raw` dikirim dalam penerbangan yang sama bila empat syarat terpenuhi:
  - (a) `qh_sql::classify(sql) == ReadOnly`;
  - (b) sesi ini sudah pernah menjalankan statement (`executed_any`);
  - (c) `qh_sql::statement_count(sql, &scan) == 1` (koreksi AR). Tanpa syarat ini, `SELECT 1; SELECT pg_terminate_backend(…)` (yang `ReadOnly` menurut `classify`) sudah dieksekusi seluruhnya oleh simple query sebelum `prepare` sempat menolak multi-statement. Janji invariant #8, "ditolak", berubah diam-diam menjadi "dijalankan lalu dilaporkan gagal";
  - (d) `options.row_limit.is_some()`, yang berarti preview (koreksi AR, lihat fallback).

  Syarat (b) membuat CLI, yang memakai satu statement per sesi, tidak pernah masuk jalur ini, jadi urutan kawatnya sama dengan hari ini. Sesi pool sudah punya cache tipe yang hangat.
- **Cara menggerakkannya.**
  - Poll pertama berurutan `SET` → `prepare` → query, jadi ketiganya sudah ada di kawat sebelum respons mana pun ditunggu.
  - Setelah itu, `prepare` dan stream di-poll **bersamaan**. Selama `prepare` masih pending, pesan stream (text row mentah) ditampung di buffer, paling banyak `row_limit` baris. Kanal stream tidak pernah penuh, jadi task koneksi terus membaca socket, dan respons `typeinfo` yang antre di belakang hasil query tetap sampai (temuan 4). Tidak perlu mendeteksi "kasus lookup".
  - Setelah `prepare` selesai, baris di buffer di-decode dengan tipe dari `prepare`, lalu cursor berjalan seperti jalur non-pipelined.
- **Bila buffer penuh sebelum `prepare` selesai** (hanya mungkin saat `prepare` menunggu `typeinfo` di belakang hasil yang lebih besar dari cap):
  1. Kirim `cancel_token.cancel_query(NoTls)` dan tunggu sampai selesai, dengan batas 250 ms.
  2. Jatuhkan stream. Task koneksi menguras sisanya (`connection.rs:152-157` terus membaca meski penerimanya sudah pergi), jadi respons `typeinfo` tetap sampai.
  3. Tunggu `prepare`. Bila ia gagal dengan SQLSTATE `57014` (cancel mengenai query `typeinfo`), ulangi `prepare` sekali tanpa pipelining. Describe tidak punya efek samping.
  4. Cursor mengembalikan baris di buffer (`row_limit` baris, jadi `truncated` benar) lalu berakhir. Menurut §4.4 cursor ini ditinggal, jadi sesinya dibuang saat checkin. Cancel yang terlambat pun tidak bisa mengenai Run lain.

  **Statement tidak pernah dijalankan dua kali.** Versi awal blueprint menjalankan ulang query lewat jalur non-pipelined, dan itu ditolak AR: `classify` menyebut `SELECT nextval('s')` atau `SELECT f()` yang menulis sebagai `ReadOnly`, sehingga efek sampingnya terjadi dua kali setelah statement pertama sukses. Preseden `retry::execute` tidak berlaku, karena ia hanya mengulang statement yang gagal sebelum halaman pertama.
- Sisa risiko yang diterima: bila `prepare` gagal karena alasan yang tidak dialami eksekusi (hampir tidak ada untuk satu statement `ReadOnly`), statement-nya sudah berjalan sekali sebelum galat dilaporkan. Di jalur non-pipelined, statement itu tidak pernah berjalan.
- `execute_bound` (dipakai `apply_changes`), `explain`, dan `count` tidak di-pipeline (syarat d).

### 11.2 MySQL: tanpa `prep`

- `prep` ada untuk satu alasan: supaya `execute` kembali sebelum query yang memblokir selesai, sehingga Stop punya cursor untuk di-cancel (`crates/qh-driver-mysql/src/lib.rs:320-331`).
- Sejak Fase 1, Stop tidak lagi butuh cursor. `select!` membatalkan await `execute`, dan `session.cancel()` memakai `connection_id` yang sudah dipublikasikan producer sebelum `query_iter` (`:642-644`).
- Commit 2 menghapus `prep` dan `close(statement)`. Semua statement lewat jalur `announce_columns` yang sudah ada (`:353-357`, `:697-730`), yang membaca kolom dari definisi result set teks.
- **Penjaga.**
  - Golden live `mysql_type_zoo_live` (G-GOLDEN) harus identik.
  - Tes cancel Fase 1 pada `SELECT SLEEP(30)` harus tetap ≤ 100 ms.

  Bila salah satunya gagal, hanya sub-langkah ini yang dicabut dari commit 2.

## 12. Safe Mode dan `statement_timeout` per Run

- **Safe Mode** tetap diputuskan per Run, **sebelum** checkout. `safe_mode()` dan `guard_for()` membaca settings Run itu (`commands.rs:1366-1367`, dan pola yang sama di setiap perintah).
  - Safe Mode tidak ada di kunci maupun di sesi.
  - Run `read_only` yang ditolak tidak meminjam sesi apa pun.
  - Lantai driver dibaca lewat `engine.driver(kind).capabilities()`, dan `PooledEngine::driver` meneruskannya ke `RealEngine`.
- **Timeout** tetap dilacak per sesi oleh driver (`statement_timeout` di PostgreSQL dan MySQL).
  - Reset mengembalikan pelacaknya ke `None`, sesuai state server setelah `RESET ALL` atau `COM_RESET_CONNECTION`.
  - Sesi baru dan sesi yang baru di-reset sama-sama berpelacak `None`. Karena itu jalur `Some → None` yang mengirim `= 0` (`qh-driver-postgres/src/lib.rs:980-998`) hanya bisa terjadi di dalam satu Run, persis seperti hari ini.
  - Trino mengirim `query_max_run_time` di setiap `POST` dan tidak punya state.
- **ADR-0016** tetap cocok (temuan 14 di rencana performa). Addendum-nya di W3-D cukup menyebut bahwa reset menghapus timeout yang sedang berlaku.

## 13. Tes yang ditulis lebih dulu

`tdd-guide` menulis tes ini sebelum implementasi.

- Tes offline memakai `Connector` palsu di `crates/qh-ffi/tests/host.rs`.
- Tes live memakai penjaga `QH_TEST_*` dan variabel koneksi yang sama dengan `crates/qh-ffi/tests/real_server.rs` serta tes integrasi driver. Tanpa penjaga, tes live dilewati dengan pesan yang jelas.

| ID | Nama tes | Jenis | Yang dibuktikan | Commit |
|---|---|---|---|---|
| T1 | `the_pool_reuses_a_session_and_resets_it_between_runs` | offline | Run kedua memakai sesi yang sama. `reset` dipanggil sekali per checkin. `done` tidak menunggu reset. | 1 |
| T2 | `pool_exhaustion_opens_a_session_outside_the_reservation_instead_of_waiting` | offline | Kunci dengan tiga idle, tiga preview bersamaan: dua memakai ulang, satu connect baru, dan metadata tetap mendapat idle ketiga. Metadata kedua connect baru. Tidak ada checkout yang menunggu lebih lama dari `CHECKIN_WAIT`. Setelah `settle`, `idle == 3` dan sesi sisanya ditutup. | 1 |
| T3 | `a_changed_credential_retires_the_old_sessions` | offline | Password p1 lalu p2: connect baru, idle lama ditutup, lease lama dibuang saat checkin. `format!("{key:?}")` tidak memuat p1 maupun p2. | 1 |
| T4 | `a_cancelled_or_dropped_lease_is_handled` | offline | Lease yang di-cancel tidak kembali ke idle. Lease yang di-drop tanpa `close` tetap checkin. Future checkout yang di-drop mengembalikan hitungan lane. | 1 |
| T5 | `a_stale_read_reconnects_once_and_a_write_does_not` | offline | `Connect` atau galat transport tanpa kode di eksekusi pertama memicu satu reconnect untuk `SELECT` dan `BEGIN`. `UPDATE` dengan galat yang sama (termasuk `Connect`) diteruskan, lease menjadi `broken`, dan `retry::execute` dengan `RETRIES=2` **tidak** menjalankan `UPDATE` itu di koneksi baru (`Connector` palsu menghitung nol connect tambahan). | 1 |
| T6 | `safe_mode_is_decided_per_run_on_a_shared_key` | offline | A (`full`) menjalankan DML. B (`read_only`, kunci sama) ditolak sebelum checkout, dan `stats` tidak berubah. C (`full`) memakai ulang sesi A. | 1 |
| T7 | `one_tunnel_serves_every_session_of_a_key` | offline | Dengan `TunnelHandle` palsu: dua query, satu metadata, dan satu `export` membuka satu tunnel. Tunnel hidup selama ekspor berjalan, lalu di-drop setelah evict. Bila `is_alive() == false`, tunnel dibuka ulang dan idle lama ditutup. | 1 |
| T8 | `the_host_path_emits_what_the_free_run_emits` | offline, diferensial | Kasus preview, explain, count, catalogs, objects, dan cancel sebelum batch pertama menghasilkan event yang sama lewat `crate::run` dan lewat host (setelah `elapsed_ms` dinormalkan). | 1 |
| T9 | `a_truncated_cursor_is_abandoned_and_a_drained_one_is_clean` | offline | Aturan `Tracked`, termasuk `count` yang kini menguras cursor-nya. | 1 |
| T10 | `the_shared_store_is_opened_and_migrated_once_per_path` | offline | `history_add` dua kali lalu `history` pada `DB_PATH` yang sama menghasilkan `opens() == 1`. Path lain dibuka sendiri, dan isinya terpisah. | 1 |
| L1 | `postgres_search_path_does_not_leak_between_runs` | `QH_TEST_POSTGRES` | A: `SET search_path = …`. B: `SHOW search_path` sama dengan default server, dan `pg_backend_pid()` sama (sesi memang dipakai ulang). Tabel temp dan advisory lock juga diperiksa. | 1 |
| L2 | `postgres_a_failed_transaction_is_rolled_back_before_reuse` | `QH_TEST_POSTGRES` | Lewat lease: `BEGIN`, `SELECT 1/0`, lalu checkin. Checkout berikutnya mendapat pid yang sama, dan `SELECT 1` lulus (bukan `25P02`). | 1 |
| L3 | `postgres_timeout_changes_between_runs_are_honoured` | `QH_TEST_POSTGRES` | Batas 100 ms dengan `pg_sleep(0.5)` menghasilkan timeout. Batas 0 lulus. `SHOW statement_timeout` setelah Run ber-batas sama dengan default role. Pid sama. | 1 |
| L4 | `postgres_reset_keeps_the_clients_own_statements` | `QH_TEST_POSTGRES` | Dua tipe enum di skema scratch: Run A memakai tipe 1, reset, lalu Run B memakai tipe 2 tanpa galat "prepared statement … does not exist". `PREPARE qh_leak` di A, lalu `PREPARE qh_leak` di B lulus. | 1 |
| L5 | `mysql_one_session_keeps_one_connection` | `QH_TEST_MYSQL`, di `crates/qh-driver-mysql/tests/integration.rs` | `CONNECTION_ID()` sama di dua `execute` pada satu sesi. `BEGIN`, `INSERT`, `ROLLBACK` tidak meninggalkan baris (temuan 1). `SET FOREIGN_KEY_CHECKS = 0` masih berlaku di statement berikutnya. Dengan batas 1 dtk di dua statement berturut-turut, `SELECT @@SESSION.max_execution_time` di statement kedua = 1000. `execute` kedua saat cursor pertama masih hidup dan belum dibaca mengembalikan `Usage` dalam 1 dtk, bukan menggantung (§7). | F |
| L6 | `mysql_timeout_changes_between_runs_are_honoured` | `QH_TEST_MYSQL` | `max_execution_time` dengan `SELECT SLEEP(0.5)`, dan connection id tetap sama. | 1 |
| L7 | `mysql_session_state_does_not_leak_between_runs` | `QH_TEST_MYSQL` | `@x`, `sql_mode`, tabel temp, `GET_LOCK`, dan `USE` di SQL pengguna tidak terbawa ke Run berikutnya. | 1 |
| L8 | `trino_context_follows_each_run` | `QH_TEST_TRINO` | Dua Run dengan katalog berbeda pada kunci yang sama menjawab dari katalog masing-masing. | 1 |
| L9 | `ssh_one_tunnel_for_preview_objects_and_export` | `QH_TEST_SSH` + `QH_TEST_POSTGRES` | `stats().tunnels_opened == 1`. Target di balik bastion diambil dari `QH_TEST_SSH_DB_HOST` (default `host.containers.internal:55432`); bila tidak terjangkau, tes dilewati dengan pesan yang jelas. | 1 |
| L10 | `postgres_truncated_preview_discards_and_refills` | `QH_TEST_POSTGRES` | Preview cap 10 atas `generate_series(1, 1e6)`: Run berikutnya mendapat pid berbeda, ada idle setelah `settle`, dan pid lama hilang dari `pg_stat_activity` dalam 1 dtk. | 2 |
| L11 | `postgres_pipelined_read_with_a_custom_type_does_not_hang` | `QH_TEST_POSTGRES` | Sesi dihangatkan dengan `SELECT 1`, lalu preview `SELECT 'a'::qh_mood FROM generate_series(1, 50000)` di bawah `timeout(5 s)` mengembalikan baris yang benar. Run kedua dengan tipe yang sama juga benar. Dengan fungsi scratch `qh_bump()` yang menaikkan counter di tabel scratch: preview `SELECT qh_bump(), 'a'::qh_mood2` (tipe baru di sesi hangat) menaikkan counter **tepat 1**, dan `SELECT qh_bump(), 'a'::qh_mood3 FROM generate_series(1, 50000)` dengan cap 10 (jalur buffer penuh) menaikkan counter paling banyak 50.000, yaitu satu eksekusi. `SELECT 1; SELECT 2` di sesi hangat ditolak tanpa pipelining, dengan galat yang sama seperti hari ini. | 2 |
| L12 | `postgres_type_zoo_through_the_host_matches_the_cli` | `QH_TEST_POSTGRES` | Kasus type_zoo lewat host dua kali (Run kedua di-pipeline) sama dengan `crate::run` + `RealEngine`. | 2 |
| L13 | `mysql_capped_preview_keeps_its_session` | `QH_TEST_MYSQL` | Cap 10: connection id sama di Run berikutnya, dan `@@SESSION.sql_select_limit` kembali ke default. `LIMIT 100000` eksplisit dengan cap 10: baris benar, tanpa galat. `INSERT INTO scratch SELECT …` 1.000 baris lewat preview dengan cap 10 menulis 1.000 baris. | 2 |
| L14 | `trino_truncated_preview_deletes_its_query` | `QH_TEST_TRINO` | Setelah preview cap 10 atas `tpch.sf1.lineitem`, query itu tidak lagi `RUNNING` di `system.runtime.queries` dalam 2 dtk. | 2 |

Penjaga tambahan tanpa tes baru: G-GOLDEN `mysql_type_zoo_live` harus identik (pencabutan `prep`), dan kasus `cancel_*` di golden tetap lulus.

Tujuh tes reset yang diwajibkan `development-plan.md` W3-T1 dan `performance-plan.md` §6 dipetakan sebagai berikut:

| Tes wajib | Tes di tabel |
|---|---|
| `search_path` tidak bocor | L1 |
| Transaksi gagal dipulihkan | L2 |
| Timeout antar-Run | L3, L6 |
| Safe Mode per Run | T6 |
| Tunnel dipakai ulang | T7, L9 |
| Pool penuh | T2 |
| Kredensial berubah | T3 |

## 14. Perubahan per berkas

| Berkas | Perubahan | Commit |
|---|---|---|
| `crates/qh-driver/src/lib.rs` | Tambah `Session::reset` (default menolak) dan `Session::set_context` (default kosong). `TlsMode` mendapat `Hash`. Tes: reset default menolak. | 1 |
| `crates/qh-driver-postgres/src/lib.rs` | Tambah `reset` (§5). `execute` dan `execute_bound` memeriksa `is_closed()` dan mengembalikan `EngineError::Connect` sebelum mengirim. `apply_statement_timeout` di-clamp ke `i32::MAX` ms dan dipecah menjadi `timeout_statement`. | 1 |
|  | Jalur pipelining dan fallback (§11.1), flag `executed_any`, dan `keepalives_idle(60 s)`. | 2 |
| `crates/qh-driver-mysql/src/lib.rs` | Producer mengembalikan `Conn` lewat `oneshot` (§7). Tambah `MysqlSession.returning`; `connection()` menunggu `Conn` yang kembali, dan menolak dengan `Usage` bila cursor sebelumnya masih hidup. | F |
|  | Tambah `reset` dan `set_context` (`wanted_db`, `current_db`, `USE` yang lazy, `opts` yang diperbarui). | 1 |
|  | Hapus `prep` dan `close(statement)` (§11.2). `SET` gabungan `max_execution_time` + `sql_select_limit` dari `row_limit`, hanya untuk statement tunggal `ReadOnly`. `tcp_keepalive(60 s)`. | 2 |
| `crates/qh-driver-mysql/tests/integration.rs` | L5 | F |
| `crates/qh-ffi/examples/bench_ffi.rs` | Pindah dari `uniffi_api::run` ke `EngineHost::new()` + `host.run(...)`. Satu host per proses bench, supaya `preview-wide` mengukur sesi yang dipakai ulang. | 1 |
| `crates/qh-driver-trino/src/lib.rs` | Isi `cancel` diekstrak ke `delete_running()`. Tambah `reset` dan `set_context`. | 1 |
| `crates/qh-tunnel/src/tunnel.rs` | `Tunnel` menyimpan klon handle. Tambah `is_alive()`. | 1 |
| `crates/qh-ffi/src/lib.rs` | `TunnelledSession` diganti nama menjadi `Held` (§4.4). `RealEngine::connect` memakai `tunnel::open` + `RealConnector`. Tambah `run_with(…, storage)`; `run` mendelegasikan ke sana dengan `None`. Dispatch lokal mengoper `storage`. Tambah `pub mod host;`. | 1 |
| `crates/qh-ffi/src/host.rs` (baru) | `EngineHost`, `route`, `Connector`, `RealConnector`, `TunnelHandle`, `SessionPool`, `PoolKey`/`Identity`, `Entry`, `Lane`, `LeaseState`, `Tracked`, `PooledEngine`, `LongOpEngine`, evict, `stats`, `settle`, dan `warm_up`. Bila berkas ini melewati kira-kira 700 baris, bagian pool dipecah ke `host/pool.rs`. | 1 |
|  | Isi ulang setelah preview PostgreSQL yang terpotong. | 2 |
| `crates/qh-ffi/src/uniffi_api.rs` | Hapus `run` bebas UniFFI beserta runtime per panggilannya. `SinkEmitter`, `fail`, dan `settings_of(Vec<Setting>)` menjadi `pub(crate)`. Doc modul diperbarui. Tes modul memakai `EngineHost::new()`. | 1 |
| `crates/qh-ffi/src/local.rs` | Tambah `SharedStorage` dan `with_storage`. Dua belas fungsi lokal menerima `Option<SharedStorage>`. `open_storage` tetap ada untuk `execution_log` dan MCP. | 1 |
| `crates/qh-ffi/src/tunnel.rs` | Tambah `open(description, settings, target)`, dipindah dari `RealEngine::connect` termasuk pemetaan `Permanent`. Tambah `identity(&TunnelConfig)` untuk kunci. | 1 |
| `crates/qh-ffi/src/retry.rs` | `produced_no_page` menjadi `pub(crate)`. Perilaku tidak berubah. | 1 |
| `crates/qh-ffi/src/commands.rs` | `count` menguras cursor-nya sampai `None`. | 1 |
|  | `stream_rows` mengoper `row_limit: Some(limit + VERDICT_FETCH)`. | 2 |
| `crates/qh-ffi/tests/host.rs` (baru) | T1–T10, L1–L4, L6–L9. | 1 |
|  | L10–L14. | 2 |
| `app/Sources/QueryHive/Support/RustEngine.swift` | Tambah `private static let host = EngineHost()`. `run` dan `runBlocking` memanggil `Self.host.run`. Tambah `warmUp(env:)`. Komentar tentang runtime per panggilan dihapus. | 1 |
| `app/Sources/QueryHive/Support/DatabaseEngine.swift` | Tambah `warmUp(env:)` di protokol, dengan extension default kosong. | 1 |
| `app/Sources/QueryHive/Models/AppModel.swift` | Tambah `warmUp(for:)`, dipanggil dari `selectTab`, `newTab`, dan restore sesi. | 2 |
| `app/Generated/` | Diregenerasi: `EngineHost` masuk, `run` bebas keluar. Hanya di commit 1. | 1 |

**Tidak disentuh:** `main.rs`, `bin/mcp.rs`, `apply.rs`, `import.rs`, `tests/golden.rs`, `tests/real_server.rs`, `Cargo.toml`, `Cargo.lock`, dan empat daftar invariant #11.

**Kepemilikan (untuk orkestrator).** Daftar berkas W3-T1 di `development-plan.md` belum memuat `crates/qh-ffi/examples/bench_ffi.rs`, `crates/qh-ffi/src/retry.rs`, `crates/qh-ffi/tests/host.rs` (baru), dan `crates/qh-driver-mysql/tests/integration.rs`. Keempatnya tidak sedang dimiliki tugas lain menurut §7 rencana itu, tetapi brief W3-T1 harus mencantumkannya secara eksplisit.

## 15. Urutan commit

Kolom "Commit" di §13 dan §14: **F** = commit `fix(mysql)`, **1** dan **2** = dua commit pool.

**Commit F: `fix(mysql): one session keeps one connection, so a transaction is one`** (diputuskan AR: wajib terpisah, dan mendarat sebelum commit 1)

- **Isi:** baris bertanda F di §14, yaitu aturan producer MySQL di §7 dan L5. Tanpa host, tanpa pool, tanpa perubahan trait.
- **Kenapa terpisah.** Ini bug data yang diam dan ada di CLI, MCP, maupun app hari ini: `apply_changes` bisa melaporkan "rolled back" untuk perubahan yang sudah tertulis, `FOREIGN_KEY_CHECKS = 0` di `import_data` tidak berlaku, dan `max_execution_time` hilang di statement kedua. L5 membuktikannya tanpa pool. Commit sendiri bisa di-bisect dan di-revert terpisah dari pekerjaan performa, dan reset MySQL di commit 1 berdiri di atasnya.
- **Urutan kerja:** L5 ditulis dan gagal lebih dulu (`CONNECTION_ID()` berbeda, baris tertinggal setelah `ROLLBACK`), lalu perbaikan producer.
- **Verifikasi:**
  - G-RUST;
  - `QH_TEST_MYSQL=1 cargo test -p qh-driver-mysql`;
  - G-GOLDEN dengan `mysql_type_zoo_live` identik;
  - tes cancel Fase 1 pada `SELECT SLEEP(30)` tetap ≤ 100 ms, karena jalur producer berubah.
- Kontrak NDJSON tidak berubah. Yang berubah hanya kebenaran: pesan "rolled back" di `apply.rs` sekarang benar.
- Commit ini di luar batas "boleh dua" W3-T1. Orkestrator mencatatnya sebagai commit ketiga W3-T1 dengan gate RR, DB, SF, dan CR.

**Commit 1: `perf(engine): an engine host with a session pool that resets every run`**

- **Isi:** semua baris bertanda 1 di §14. Setelah commit ini, app memakai host, pool, reset, tunnel bersama, dan handle SQLite bersama. Preview yang terpotong masih membuang sesinya di PostgreSQL dan MySQL, sedangkan Trino menyimpannya lewat `DELETE` saat reset.
- **Urutan kerja:**
  1. Tes T1–T10, L1–L4, dan L6–L9 ditulis dan gagal lebih dulu.
  2. Trait di `qh-driver` dan tiga driver (reset dan `set_context`). L5 sudah hijau sejak commit F.
  3. `qh-tunnel`.
  4. `host.rs`, `lib.rs`, `local.rs`, `uniffi_api.rs`.
  5. `./app/build-ffi.sh`, lalu Swift.
- **Verifikasi:**
  - G-RUST, G-FFI, G-SWIFT;
  - G-GOLDEN tanpa selisih;
  - G-LIVE untuk PostgreSQL, Trino, dan SSH, ditambah `QH_TEST_MYSQL=1 cargo test -p qh-driver-mysql`;
  - `cargo test -p qh-ffi --test host` dengan semua penjaga menyala;
  - `cargo run --release -p qh-ffi --example bench_ffi -- local-loop` berjalan lewat host.

**Commit 2: `perf(engine): capped previews keep their session, warm-up on select, one round trip fewer`**

- **Isi:** baris bertanda 2 di §14, dalam tiga sub-langkah:
  - (a) cap `limit + 1`, `sql_select_limit` MySQL untuk statement tunggal `ReadOnly`, isi ulang PostgreSQL, dan hook warm-up di Swift;
  - (b) pipelining PostgreSQL;
  - (c) pencabutan `prep` MySQL.

  Sub-langkah (b) dan (c) bisa dicabut satu per satu bila penjaganya gagal. Angka yang meleset (misalnya S3) masuk jalur Fase 8 (P-21).
- **Tidak ada perubahan UniFFI** di commit ini, jadi `app/Generated/` tidak berubah lagi.
- **Verifikasi:**
  - G-RUST, G-SWIFT;
  - G-GOLDEN, dengan `mysql_type_zoo_live` identik;
  - G-LIVE dan L10–L14;
  - lalu W3-T3: G-BENCH(1) untuk S1, S3, dan S4, level pohon hangat, 5.000 tabel, cold start yang tidak mundur, dan perintah lokal lewat host (NFR-P8).

## 16. Risiko

| ID | Risiko | Dampak | Mitigasi |
|---|---|---|---|
| R-1 | State sesi yang tidak dicakup reset: di PostgreSQL, apa pun yang juga lolos dari `DISCARD ALL` (koneksi `dblink`, state ekstensi); di MySQL, apa pun yang lolos dari `COM_RESET_CONNECTION` | Hasil Run B dipengaruhi Run A | Reset memakai kontrak `DISCARD ALL` dan `COM_RESET_CONNECTION` yang didokumentasikan server; L1, L2, L4, L7; pemeriksaan DB reviewer |
| R-2 | Cancel yang terlambat mengenai Run berikutnya | Statement Run B mati tanpa sebab yang jelas | Sesi yang pernah di-cancel tidak pernah kembali ke pool (D-6); T4 |
| R-3 | Pipelining mengeksekusi statement sebelum `prepare` selesai | Statement `ReadOnly` tunggal yang `prepare`-nya gagal sudah berjalan sekali (jalur lama tidak menjalankannya). Tidak ada eksekusi ganda: fallback menjalankan ulang dihapus AR (§11.1). | Syarat (a) sampai (d) di §11.1; L11 membuktikan counter naik tepat satu kali; sub-langkah 2b bisa dicabut sendiri |
| R-4 | `sql_select_limit` mengubah `SQL_CALC_FOUND_ROWS` di preview MySQL | `FOUND_ROWS()` berbeda | Hanya untuk statement tunggal `ReadOnly`, jadi `FOR UPDATE`, `INSERT … SELECT`, `CREATE … SELECT`, dan `SELECT … INTO` tidak pernah terkena (L13). Teks SQL tidak diubah. Dicatat di ADR-0031. |
| R-5 | Sesi basi pada write pertama setelah idle | Write pertama gagal sekali dengan galat koneksi, tanpa dijalankan ulang (§6) | Keepalive 60 dtk; `BEGIN` dan baca di-reconnect sekali; `broken` membuat Run berikutnya mendapat sesi baru |
| R-6 | Jumlah koneksi ke server | Sampai 3 idle per kunci, ditambah sesi di luar reservasi. PostgreSQL membuat satu kunci per database yang dibuka di pohon. | Idle kedaluwarsa dalam 5 menit; sesi di luar reservasi tidak disimpan melewati `IDLE_MAX`. Tidak lebih buruk dari hari ini, yang membuka satu koneksi per perintah tanpa batas. |
| R-7 | Kebocoran pool | Sesi atau tunnel tidak pernah ditutup | Guard RAII untuk semua hitungan; checkin di `Drop`; `settle` dan `stats` di setiap tes; G-LEAK di Fase 6 |
| R-8 | Tabrakan hash kredensial | Dua password berbeda berbagi kunci | Peluang tabrakan 64 bit bisa diabaikan; kunci SipHash acak per proses; nilai tidak pernah keluar dari memori; pemeriksaan SEC |
| R-9 | Handle SQLite menunjuk berkas yang sudah dihapus | Tulisan hilang bila path yang sama dibuat ulang | Hanya terjadi di tes yang memakai ulang path. Tes memakai direktori sementara yang unik, dan app memakai satu path tetap. |
| R-10 | Fase 1 mendarat dengan nama lain | Blueprint menyebut nama yang tidak ada | Blueprint hanya bergantung pada tiga perilaku (lihat Prasyarat); implementer memakai nama dari W2-T1 |
| R-11 | Deadlock pipelining yang tidak terdeteksi fallback | Run menggantung sampai Stop | Deteksi berdasarkan urutan kawat (§11.1); L11 dengan `timeout(5 s)`; sub-langkah bisa dicabut |
| R-12 | Pohon memuat banyak node sekaligus, sementara lane metadata hanya satu | Node selain yang pertama membayar connect dingin | Tidak lebih buruk dari hari ini, dan idle yang tersisa tetap dipakai. Bila gate pohon meleset, cap metadata dinaikkan lewat keputusan terpisah. |

## 17. Untuk pemeriksa dan untuk ADR-0031

**Penyimpangan dari teks rencana yang butuh verdict AR:**

1. Reset PostgreSQL: `ROLLBACK` selalu, dan `DISCARD ALL` tanpa `DEALLOCATE ALL` (temuan 2 dan 3).
2. Kunci: nama database hanya untuk PostgreSQL, ada/tidaknya database untuk MySQL, dan `set_context` baru di trait (temuan 6, D-4).
3. 2+1 sebagai reservasi tanpa antrean (D-5).
4. `table_op` masuk pool, sedangkan `test` di luar pool (D-9).
5. Trino: `DELETE` dikirim saat reset, bukan saat `close` (§7).
6. Pipelining PostgreSQL bersyarat, dengan fallback yang menjalankan ulang statement baca (§11.1, R-3).
7. `run` bebas UniFFI dihapus (D-11).
8. Perbaikan transaksi MySQL (temuan 1) masuk W3-T1.

**Yang sengaja tidak dikerjakan:**

- **Pool untuk MCP.** MCP adalah proses terpisah (NFR-S6), dan `crate::run` tetap.
- **Menyiapkan TLS Trino saat warm-up** (`GET /v1/info`). Itu memang bukan query, tetapi sudah di luar "hanya connect".
- **Memasang ulang timeout saat reset.** Ini butuh `RESET statement_timeout` atau `= DEFAULT` untuk `None`, sedangkan jalur `SET` yang di-pipeline sudah menutup round trip-nya di PostgreSQL.
- **Cancel lalu drain untuk preview PostgreSQL yang terpotong**, supaya sesinya tersimpan. Ini kandidat Fase 8 bila S1 atau S3 meleset karena isi ulang.
- **API `forget` untuk koneksi yang diedit atau dihapus.** Kunci lama kedaluwarsa sendiri dalam 5 menit, dan kredensial yang berubah langsung memensiunkannya.
- **Batas koneksi global lintas kunci.**
- **Memasang execution log di jalur app** (temuan 11).

## Verdict architect-reviewer

**Verdict: approved with corrections applied** (30 Sep 2026, W2-A1). Koreksi di bawah sudah ditulis ke badan blueprint. W3-T1 boleh mulai dari versi ini.

### Klaim yang diperiksa di kode

| Klaim | Hasil | Bukti |
|---|---|---|
| Sesi MySQL membuang koneksinya per statement, jadi `BEGIN`/`COMMIT` di `apply.rs` tidak atomik | **Benar** | `run_statement` memindahkan `Conn` ke `Producer` (`qh-driver-mysql/src/lib.rs:305`, `:345-363`), `Producer::run` menjatuhkannya (`:604-618`), dan `idle` hanya diisi lagi oleh `text_rows` dan jalur gagal `SET` (`:290`, `:315`). `apply.rs:141-145` mengirim `BEGIN` lewat sesi itu. Dampak tambahan yang ditemukan: `FOREIGN_KEY_CHECKS = 0` di impor tidak berlaku, dan `max_execution_time` hilang di statement kedua (`:1089-1104`). |
| `DISCARD ALL` merusak cache tipe tokio-postgres | **Benar** | `CachedTypeInfo` menyimpan statement `typeinfo*` (`tokio-postgres-0.7.18/src/client.rs:70-85`). `clear_type_cache` hanya mengosongkan `types` (`:141-143`). `typeinfo_statement` memakai ulang statement yang ter-cache (`prepare.rs:195-209`). |
| Tidak ada status transaksi publik | **Benar** | Hanya ada `__private_api_rollback` (`client.rs:775`) |
| Pipelining `prepare` + `simple_query_raw` bisa deadlock pada tipe non-bawaan | **Benar** | Kanal respons berkapasitas 1 (`client.rs:97`). Task koneksi berhenti membaca saat `poll_ready` pending (`connection.rs:159-166`), tetapi terus menguras bila penerimanya sudah di-drop (`:152-157`). |
| `run` bebas UniFFI hanya dipanggil `RustEngine.swift` | **Salah** | `crates/qh-ffi/examples/bench_ffi.rs:33` juga memanggilnya (commit `b810592`). Sudah dikoreksi (temuan 10, D-11, §14). |

### Keputusan atas penyimpangan di §17

1. **Reset PostgreSQL (`ROLLBACK` selalu, `DISCARD ALL` tanpa `DEALLOCATE ALL`, ditambah `DEALLOCATE` untuk statement `from_sql`): diterima.** Hanya ini yang benar tanpa fork tokio-postgres, dan komponennya sama dengan urutan `DISCARD ALL` yang didokumentasikan PostgreSQL.
2. **Kunci (database hanya untuk PostgreSQL, ada/tidaknya database untuk MySQL, `set_context` baru di trait): diterima dengan koreksi.** Tanpa ini, gate pohon hangat tidak bisa tercapai (temuan 6). Koreksinya: kunci dibangun dengan destrukturisasi lengkap dari config yang sudah di-resolve, supaya field baru (CA per koneksi, JWT) gagal compile dan password dari `DB_URL` ikut membatalkan kunci (§4.1).
3. **2+1 sebagai reservasi tanpa antrean: diterima.** Antrean menimbulkan head-of-line blocking dan tidak membeli kebenaran apa pun. Tafsiran ini harus ditulis terang di ADR-0031 dan di laporan akhir untuk pemilik: O-7 dibaca sebagai kapasitas sesi yang dipertahankan, bukan batas koneksi ke server. Lonjakan di atas 2+1 membuka sesi seperti hari ini dan tidak disimpan. `CHECKIN_WAIT` 250 ms dipertahankan: satu RTT reset lebih murah daripada 3–8 RTT connect.
4. **`table_op` di lane query: diterima.** Perintahnya satu statement dan interaktif. Karena checkout tidak pernah mengantre, `DROP` yang tertahan lock hanya memakan satu slot reservasi. **`test` di luar pool: diterima.** Test Connection harus membuktikan auth dan SSH dari nol.
5. **Trino mengirim `DELETE` saat reset, bukan saat `close`: diterima.** Semantik `close` di CLI (`qh-driver-trino/src/lib.rs:1034-1039`) tetap, dan query yang ditinggal di jalur pool memang tidak akan dibaca siapa pun.
6. **Pipelining PostgreSQL bersyarat: diterima dengan koreksi, fallback yang menjalankan ulang statement ditolak.** Dari dua alternatif (jalankan ulang, atau tampung lalu cancel), yang dipilih adalah **tampung sampai `row_limit`, lalu cancel**. Alternatif ini tidak pernah menjalankan statement dua kali dan tidak butuh deteksi "kasus lookup". Syarat tambahannya: statement tunggal (`statement_count == 1`) dan hanya bila `row_limit` ada (§11.1, R-3).
7. **`run` bebas UniFFI dihapus: diterima dengan koreksi.** Dari dua alternatif (hapus lalu pindahkan bench, atau pertahankan sebagai wrapper di atas host global), yang dipilih adalah **hapus dan pindahkan `bench_ffi.rs` ke `EngineHost` di commit 1**. Jalur FFI kedua yang melewati pool hanya akan menjadi pintu belakang, dan W3-T3 memang harus mengukur jalur host.
8. **Perbaikan transaksi MySQL masuk W3-T1: diterima, sebagai commit terpisah.** `fix(mysql): one session keeps one connection, so a transaction is one` mendarat **sebelum** commit 1. Perbaikan ini bisa diuji sendiri lewat L5 tanpa pool, memperbaiki bug data di CLI, MCP, dan app sekaligus, serta bisa di-bisect dan di-revert terpisah (§15).

### Keputusan lain

- **Reconnect di §6 dipersempit.** Statement hanya dijalankan ulang di koneksi pengganti bila `ReadOnly`, atau bila teksnya persis `BEGIN`/`START TRANSACTION`. Versi awal menganggap `Connect` sebagai "belum terkirim", padahal MySQL memetakan galat IO di tengah query ke `Connect` (`lib.rs:1071-1074`). Versi awal juga memberi `retry::execute` koneksi segar untuk write. Keduanya bisa menulis dua kali tanpa tanda. T5 diubah untuk membuktikan hal ini.
- **`sql_select_limit` MySQL hanya untuk statement tunggal `ReadOnly`**, supaya batas preview tidak pernah mengurangi baris yang ditulis `INSERT … SELECT` atau `CREATE … SELECT`, atau yang dikunci `FOR UPDATE`. L13 menambah kasus `INSERT … SELECT`.
- **`MysqlSession::connection()` tidak pernah menunggu cursor yang masih hidup.** Pilihannya adalah galat `Usage` yang terlihat, bukan menunggu (deadlock) dan bukan membuka koneksi kedua diam-diam (bug temuan 1 dalam bentuk lain). Hari ini tidak ada jalur yang memicunya; L5 menjaganya.
- **Aturan `Tracked` dipertajam.** Hanya `Err` yang membawa kode server yang membuat cursor bersih. Cursor yang masih hidup saat checkin dihitung ditinggal.
- **Dihapus karena berlebihan:** drain `DRAIN_ROWS` MySQL (connect baru lebih murah dan tidak diminta rencana) dan deteksi "kasus lookup" pada pipelining (digantikan polling bersamaan).
- **Dipertahankan meski menambah kode:** seam `Connector` dan `TunnelHandle` (tes offline T1–T10 butuh keduanya), `match` rute tanpa wildcard, dan `CHECKIN_WAIT`.
- **Kepemilikan berkas:** brief W3-T1 harus menambahkan `examples/bench_ffi.rs`, `src/retry.rs`, `tests/host.rs`, dan `qh-driver-mysql/tests/integration.rs` (§14).

### Pemeriksaan terhadap sumber yang mengikat

- **O-6:** reset setiap checkin, dan sesi yang tidak bisa di-reset dibuang. Terpenuhi.
- **O-7:** terpenuhi dengan tafsiran di keputusan 3.
- **P-05:** `export`, `to_table`, dan `import_data` lewat `LongOpEngine` dengan `Arc` tunnel yang sama. Terpenuhi.
- **NFR-S2:** kredensial hanya masuk sebagai hash SipHash berkunci acak per proses, `Debug` ditulis tangan, dan tidak ada log atau event. Terpenuhi, diperkuat koreksi di §4.1.
- **NFR-C:** `crate::run`, CLI, MCP, dan golden tidak berubah bentuk. Commit F hanya mengubah kebenaran MySQL, bukan bentuk event. `row_limit` di `stream_rows` (commit 2) juga berlaku di CLI tetapi menghasilkan event yang sama; G-GOLDEN live `mysql_type_zoo_live` menjaganya.
- **Invariant #1 dan #11:** `app/Generated/` diregenerasi di commit 1 saja, dan empat daftar perintah tidak disentuh. Terpenuhi.
- **Invariant #8:** dijaga oleh syarat (c) pipelining.
- **Invariant #9:** Trino tidak mendapat keepalive atau state socket. Terpenuhi.

### Risiko terbuka (tidak memblokir, untuk reviewer DB dan SF di W3-T1)

- **Risiko yang sudah ada, bukan diperkenalkan blueprint ini:** `retry::execute` mengulang statement apa pun pada galat `Transient` sebelum halaman pertama. Di MySQL, dan setelah commit F juga (IO menjatuhkan `Conn`, lalu `connection()` membuka yang baru), ini bisa menjalankan ulang write dari editor. Blueprint tidak memperburuknya. Perbaikannya keputusan terpisah di `retry.rs`.
- Dua koneksi tersimpan dengan identitas sama tetapi password berbeda akan saling memensiunkan entry. Hasilnya tetap benar, hanya lebih lambat.
- Mutex tunnel per entry dipegang selama handshake SSH. Checkout lain untuk kunci yang sama menunggu, dan Stop menjatuhkan future-nya, jadi ini bukan deadlock. Tetapi `crates/qh-tunnel/src/tunnel.rs` tidak punya timeout connect (grep hanya menemukan `inactivity_timeout`). Bastion yang menggantung akan menahan semua Run kunci itu sampai Stop. SF memutuskan apakah `Tunnel::open` perlu dibungkus `timeout` di W3-T1.
