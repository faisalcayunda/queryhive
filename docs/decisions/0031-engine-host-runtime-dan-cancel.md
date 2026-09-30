# 0031 — EngineHost: runtime per proses, Stop sampai server, dan pool sesi yang di-reset

- **Status:** Diterima. Bagian 1 (runtime per proses dan cancel sampai server) dan bagian 2
  (`EngineHost`, pool sesi 2+1, semantik reset, operasi panjang di luar pool) sudah mendarat.
- **Tanggal:** 30 Sep 2026 (Fase 0 sampai Fase 2 perf-parity)
- **Konteks instruksi:** `docs/architecture/development-plan.md` W2-D dan W3-D;
  `docs/architecture/performance-plan.md` (Fase 1: cancel dan runtime; Fase 2: pool);
  `docs/architecture/blueprints/fase-2-engine-host.md` (beserta verdict architect-reviewer);
  `docs/invariants.md`. Bukti kode bagian 1: commit `af6018d` (engine) dan `a953344` (app
  menghormati `done.cancelled`). Bagian 2: `a8174f8`, `d2bfe6b`, `83a9d5e`, `a25fa50`.

## Konteks

Sebelum perubahan ini, jalur FFI membangun runtime tokio baru untuk setiap run, dan Stop hanya
berarti "berhenti menunggu": future run di-drop, koneksi ikut jatuh, dan pekerjaan di server
dibiarkan. Tiga masalah nyata muncul dari situ, semuanya terukur:

1. **Stop lambat.** Di app, `cancel_ms` (dari klik Stop sampai UI berhenti) mencapai **28.500 ms**,
   karena cancel hanya diperiksa di antara batch; run yang sedang menunggu server (`pg_sleep`, query
   berat tanpa baris pertama) tidak bisa dihentikan sampai server menjawab.
2. **Query yatim di server.** Berhenti membaca tidak menghentikan statement. Ini masalah yang sama
   yang ditutup ADR-0016 untuk batas waktu: pekerjaan tetap memegang slot gudang.
3. **Biaya per perintah.** Membangun runtime per perintah lokal menambah overhead tetap pada setiap
   panggilan kecil (p50 sekitar 0,5 ms).

Ada juga temuan yang mengubah cara berpikir tentang "cukup drop koneksi". Pada runtime yang dipakai
bersama, meng-drop koneksi tidak membuang hasil yang sedang mengalir: task koneksi di latar
**menguras** respons yang tertunda. Diukur pada MySQL: **sekitar 6 detik pada 100% satu core** setelah
sebuah preview yang dibatasi selesai dan koneksinya di-drop. Pada runtime per run, drain itu ikut mati
bersama runtime dan tidak kelihatan; pada runtime bersama ia tetap hidup dan memakan CPU.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Runtime | **Satu runtime per proses, `OnceLock`, dibangun `qh_rt::build_main`** | Nol biaya bangun per perintah; QoS dan jumlah worker sesuai ADR-0010 di satu tempat | Drain koneksi yang di-drop tidak lagi mati bersama runtime (lihat Konteks) |
| | Runtime per run (keadaan sebelumnya) | Isolasi total; drain mati sendiri | Overhead per perintah; QoS ADR-0010 tidak berlaku di jalur app |
| | Runtime milik `EngineHost` | Selaras dengan bagian 2 | `EngineHost` belum ada; menunda perbaikan Stop |
| Cancel | **Preemptif: `CancelFlag` + `Notify`, tiap `await` dipacu dengan `select!`** | Berhenti walau sedang menunggu server | Setiap titik `await` harus diingat |
| | Periksa flag antar batch (sebelumnya) | Sederhana | Tidak menjangkau run yang sedang menunggu; 28.500 ms |
| Server | **`session.cancel()` lalu `close`, di task terlepas** | Statement benar-benar berhenti; `KILL` yang lambat tetap selesai | Perlu aturan urutan dan anggaran tunggu |
| | Drop koneksi saja | Nol kode | Tidak menghentikan statement, dan meninggalkan drain di latar |

## Keputusan

**Satu runtime tokio per proses. Cancel bersifat preemptif dan sampai ke server, dengan urutan dan
anggaran waktu yang tetap.**

Rincian yang mengikat:

1. **Runtime.** `OnceLock<tokio::runtime::Runtime>` di `qh-ffi`, dibangun oleh `qh_rt::build_main`
   pada pemakaian pertama. **Hanya build yang berhasil yang di-cache**; kegagalan dilaporkan sebagai
   galat dan dicoba lagi pada panggilan berikut, bukan disimpan sebagai kegagalan permanen.
2. **Jembatan sinkron.** Thread `Dispatch` memanggil `block_on` pada runtime itu. Pembungkusnya
   memakai `catch_unwind` dan memetakan panic ke event `internal error`, sesuai firewall ADR-0009;
   panic tidak menembus FFI dan tidak meracuni runtime bersama.
3. **Cancel preemptif.** `CancelFlag` dipasangkan dengan `Notify`. **Setiap `await` yang bisa
   menunggu dipacu dengan `select!` terhadap cancel**: connect, `retry::execute`, batch pertama, dan
   probe verdict. Tidak ada `await` panjang yang lolos dari pengecekan.
4. **Stop sampai server.** Saat cancel, engine memanggil `session.cancel()` lalu `close`, di task
   terlepas (detached) supaya `KILL` yang lambat tetap selesai walau pemanggilnya sudah pergi.
   - **Stop dari pengguna** menunggu paling lama **250 ms** (`STOP_BUDGET`) untuk konfirmasi server.
     Bila belum terkonfirmasi, `done` tetap dikirim dengan **warning** yang mengatakan hal itu, bukan
     diam-diam mengaku sukses.
   - **Preview yang terpotong oleh batas baris** menghentikan server di latar dan mengirim `done`
     **segera**, tanpa menunggu.
5. **Cancel sebelum drop.** Urutannya wajib: `cancel()` dulu, baru koneksi di-drop. MySQL
   memerlukan connection id untuk `KILL QUERY`, dan driver **tidak lagi menghapusnya**; id
   dipublikasikan saat sesi connect dan diterbitkan ulang saat statement mulai.
6. **Trino.** `execute` di-poll sampai **250 ms** setelah Stop supaya query id sempat tercatat dan
   `cancel` punya sasaran (DELETE pada URI halaman yang sedang berjalan).
7. **Verdict.** Run yang berakhir karena cancel melaporkan `truncated: true`, bukan `false`: baris
   yang sudah ada valid tetapi tidak lengkap, dan UI tidak boleh menyajikannya sebagai hasil penuh.
8. **App menghormati `done.cancelled`.** `AppModel` menampilkan run yang dihentikan sebagai
   "berhenti", mempertahankan baris yang sudah diterima, dan tidak menyebutnya galat.

## Alasan

1. **Preemptif karena penantian server tidak punya batas atas.** Cek antar batch tidak pernah
   berjalan bila batch pertama belum datang; `select!` pada setiap `await` adalah satu-satunya bentuk
   yang menutup celah itu.
2. **Server-side karena berhenti membaca bukan berhenti bekerja.** Alasan yang sama dengan ADR-0016:
   query yang dibiarkan berjalan memegang sumber daya. Mekanisme server (CancelRequest protokol PostgreSQL lewat `CancelToken` tokio-postgres,
   `KILL QUERY`, DELETE Trino) adalah yang benar-benar menghentikannya.
3. **Detached, dengan anggaran 250 ms.** Pengguna yang menekan Stop menunggu jawaban jujur, tetapi
   tidak boleh menunggu server yang lambat. Anggaran itu membatasi tunggu tanpa membatasi `KILL`.
4. **Cancel sebelum drop karena drain.** Pada runtime bersama, drop tanpa cancel meninggalkan drain
   (MySQL: 6 detik pada 100% core). Cancel memberi tahu server untuk berhenti mengirim, sehingga
   tidak ada yang perlu dikuras. Ini juga alasan **jalur yang dibatasi (preview terpotong) harus
   ikut cancel**, bukan hanya jalur Stop pengguna: jalur itu paling sering terjadi dan paling mudah
   terlupa.
5. **Runtime per proses karena ADR-0010 hanya bermakna bila runtime-nya satu.** `build_main`
   menerapkan `worker_threads = P_core` dan QoS `USER_INITIATED`; runtime per run tidak pernah
   menikmatinya dan membayar biaya bangun setiap kali.

## Konsekuensi

### Hasil terukur

| Ukuran | Sebelum | Sesudah |
|---|---|---|
| Stop PostgreSQL terkonfirmasi server | tidak dikonfirmasi | **< 250 ms** |
| MySQL: `KILL` sampai statement hilang | tidak ada `KILL` | **≤ 2,5 detik** (`KILL` diterima dan statement hilang) |
| Trino: Stop | tidak ada | `USER_CANCELED` dalam **23 sampai 105 ms** |
| App `cancel_ms` | **28.500 ms** | **27 sampai 34 ms** |
| Perintah lokal (p50) | ~0,5 ms | **~0,35 ms** |

### Positif

- Stop bekerja walau run sedang menunggu server, dan pekerjaan di server ikut berhenti.
- Overhead per perintah turun karena runtime tidak dibangun ulang.
- QoS ADR-0010 kini berlaku di jalur app, bukan hanya CLI dan MCP (lihat addendum di sana).
- Kegagalan membangun runtime tidak menjadi keadaan permanen.

### Negatif

- **Drain koneksi yang di-drop kini hidup di runtime bersama** dan bisa memakan CPU (MySQL: ~6 detik
  pada 100% core). Aturannya harus dijaga: **setiap jalur yang berhenti sebelum hasil habis wajib
  cancel sebelum drop**. Jalur baru yang lupa aturan ini menghidupkan kembali masalahnya.
- Setiap `await` baru yang bisa menunggu harus dipacu dengan `select!`; itu disiplin kode, bukan
  jaminan tipe.
- Panic yang ditangkap `catch_unwind` melaporkan `internal error` tetapi runtime bersama tetap
  hidup; state yang rusak akibat panic tidak dibersihkan oleh ADR ini.
- Konfirmasi Stop yang melewati 250 ms hanya berupa warning; pengguna bisa melihat "berhenti" sebelum
  server benar-benar berhenti.

## Bukti

- Engine: commit `af6018d`. Tes `crates/qh-ffi/tests/golden.rs` (perilaku event) dan
  `crates/qh-ffi/tests/real_server.rs` (server nyata: Stop pada preview dan count yang tidur
  terkonfirmasi < 250 ms; MySQL `Com_kill` bertambah dan statement hilang; Trino `USER_CANCELED`).
- App: commit `a953344`; `app/Tests/QueryHiveTests/StoppedRunTests.swift` memastikan run yang
  dihentikan menyatakan berhenti dan mempertahankan barisnya.

## Bagian 2: `EngineHost` dan pool sesi

### Konteks bagian 2

Setelah bagian 1, setiap Run masih membuka koneksi baru: TCP, TLS, autentikasi, dan terowongan SSH
bila ada. Itu biaya tetap di jalur Run pertama, di setiap level pohon objek, dan di setiap preview.
Keputusan pemilik O-6 (reset setiap sesi yang dikembalikan, sesi yang tidak bisa di-reset dibuang)
dan O-7 (sesi 2+1) meminta pool. Pool juga menutup item terbuka bagian 1: cancel terlepas yang
terlambat tidak boleh mengenai statement berikutnya pada sesi yang dipakai ulang.

Membaca kode sebelum membangun pool menemukan tiga hal yang mengubah rencana (blueprint §1):

1. **Sesi MySQL membuang koneksinya setelah setiap statement.** `BEGIN`, perubahan, dan `COMMIT` dari
   `apply_changes` jalan di koneksi berbeda, jadi transaksi di MySQL tidak atomik dan `ROLLBACK`
   tidak membatalkan apa pun. `FOREIGN_KEY_CHECKS = 0` di impor dan `max_execution_time` di statement
   kedua ikut hilang. Bug ini ada di CLI, MCP, dan app, dan dibetulkan lebih dulu (`a8174f8`).
2. **tokio-postgres tidak membuka status transaksi**, dan `DISCARD ALL` menjalankan `DEALLOCATE ALL`,
   yang menghapus statement `typeinfo` yang di-prepare client sendiri. Tipe non-bawaan berikutnya
   gagal dengan `prepared statement "sN" does not exist`.
3. **Pohon mengganti database, katalog, dan skema per level.** Bila semuanya masuk kunci pool,
   setiap level menjadi kunci baru dengan koneksi dingin.

### Opsi yang dipertimbangkan (bagian 2)

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Tempat pool | **Di balik trait `Engine`: checkout = `connect`, `close` pada sesi pinjaman = checkin** | `commands.rs`, `apply.rs`, `retry.rs`, dan `crate::run` (CLI, MCP, golden) tidak berubah bentuk | Wrapper sesi (`Held`) memegang lebih banyak aturan |
| | Pool di dalam tiap perintah | Kontrol penuh per perintah | Setiap perintah mengulang aturan reset dan lease |
| Makna 2+1 | **Reservasi atas sesi idle, tanpa antrean** | Run di tab ketiga tidak menunggu ekspor atau preview lambat di tab lain | O-7 dibaca sebagai kapasitas yang dipertahankan, bukan batas koneksi ke server |
| | Antrean dengan batas keras 2+1 | Jumlah koneksi terbatas | Head-of-line blocking, tanpa menambah kebenaran |
| Reset | **Di latar saat checkin; gagal atau lewat 2 dtk = sesi dibuang** | Run yang selesai tidak menunggu reset | Sesi berikutnya bisa menunggu reset yang sedang jalan (maksimal 250 ms) |
| | Reset saat checkout | Sederhana | Reset masuk jalur kritis Run berikutnya |
| Sesi cancel atau rusak | **Dibuang tanpa reset** | Cancel yang terlambat tidak pernah bisa mengenai Run lain | Satu connect ekstra setelah Stop |
| | Reset lalu pakai ulang | Hemat connect | Reset pada sesi berstatus tak pasti tidak bisa dibuktikan benar |
| Kunci pool | **Identitas server dan auth, ditambah hash kredensial; database hanya untuk PostgreSQL** | Level pohon MySQL dan Trino tetap hangat | `set_context` baru di trait |
| | Seluruh `DB_*` masuk kunci | Sederhana | Gate "level pohon hangat" tidak mungkin tercapai |
| Operasi panjang | **Sesi sendiri di luar pool, memakai terowongan kunci yang sama** | Ekspor berjam-jam tidak menahan reservasi | Satu koneksi tambahan selama operasi |
| | Lane ketiga di pool | Satu mekanisme | Ekspor menghabiskan reservasi atau memaksa antrean |

### Keputusan bagian 2

**Satu `EngineHost` per proses app memegang pool sesi per kunci koneksi. Setiap sesi di-reset di
latar saat dikembalikan, dan sesi yang tidak bisa dipercaya dibuang, bukan di-reset. Operasi panjang
memakai sesi sendiri di luar pool.**

Rincian yang mengikat:

1. **`EngineHost`** adalah objek UniFFI dengan `new()`, `run(command, settings, sink, cancel)`
   (sinkron, kontrak sama dengan `run` bebas yang dulu), dan `warm_up(settings)` (tidak memblokir).
   `run` bebas UniFFI dihapus supaya tidak ada pintu belakang yang melewati pool; `bench_ffi`
   pindah ke host. `crate::run` untuk CLI, MCP, dan golden tetap ada dan tidak memakai pool.
2. **Rute** ditentukan satu `match` tanpa wildcard di `crates/qh-ffi/src/host.rs`, sehingga perintah
   baru gagal compile sampai memilih rute:
   - `Local`: penyimpanan koneksi, riwayat, query tersimpan, sesi, akun, profil, `credential`.
     Tanpa sesi database; satu handle SQLite bersama per path `DB_PATH`.
   - `Fresh`: `db_drivers` dan `test`. Sesi dan terowongan baru, karena Test Connection harus
     membuktikan connect dan SSH dari nol.
   - `Pooled(Metadata)`: `catalogs`, `schemas`, `tables`, `objects`.
   - `Pooled(Query)`: `preview`, `explain`, `count`, `apply_changes`, `table_op`.
   - `LongOp`: `export`, `to_table`, `import_data`.
3. **Pool 2+1.** Per kunci: `QUERY_SESSIONS = 2`, `METADATA_SESSIONS = 1`, `IDLE_MAX = 3`. Ini
   reservasi atas sesi idle, **bukan antrean dan bukan batas koneksi**. Checkout tidak pernah menunggu
   Run lain; satu-satunya tunggu adalah `CHECKIN_WAIT = 250 ms` untuk sesi yang sedang di-reset.
   Bila lane penuh, dibuka sesi baru di luar reservasi dengan biaya yang sama seperti sebelum ada pool,
   dan sesi itu tidak disimpan melewati `IDLE_MAX`. Sesi idle kedaluwarsa setelah `IDLE_TTL = 5 menit`
   (dicek tiap `EVICT_TICK = 30 dtk`).
4. **Kunci.** `PoolKey` = identitas (jenis, host, port, user, TLS, insecure, database hanya untuk
   PostgreSQL, terowongan) ditambah hash SipHash berkunci acak per proses atas kredensial (password
   DB, password SSH, passphrase kunci SSH). Kunci dibangun dari config yang sudah di-resolve dengan
   destrukturisasi tanpa `..`, jadi field baru gagal compile sampai ditempatkan. Hash tidak pernah
   ditulis ke log, event, atau disk, dan `Debug` mencetak `credential: <hashed>`. Kredensial yang
   berubah memensiunkan entry lama. Database MySQL serta katalog dan skema Trino adalah konteks per
   Run lewat `Session::set_context`. Safe Mode, timeout, dan setelan lain tidak masuk kunci.
5. **Reset per Run**, di latar, dengan anggaran `RESET_BUDGET = 2 dtk`:
   - **PostgreSQL:** `ROLLBACK` selalu (tidak ada status transaksi publik), lalu komponen `DISCARD ALL`
     satu per satu **tanpa `DEALLOCATE ALL`** (`CLOSE ALL`, `SET SESSION AUTHORIZATION DEFAULT`,
     `RESET ALL`, `UNLISTEN *`, `pg_advisory_unlock_all()`, `DISCARD PLANS/TEMP/SEQUENCES`), ditambah
     `DEALLOCATE` bernama untuk statement buatan pengguna (`pg_prepared_statements WHERE from_sql`).
     Statement protokol milik client dibiarkan. Ketiga pesan awal dikirim dalam satu penerbangan.
   - **MySQL:** `COM_RESET_CONNECTION` (`COM_CHANGE_USER` bila server tidak mendukungnya), lalu
     `SET NAMES utf8mb4 COLLATE utf8mb4_general_ci` (`utf8` sebelum 5.5.3), karena reset mengembalikan
     charset ke default server dan `mysql_async` hanya menyatakannya saat handshake. Lalu
     `SELECT DATABASE()`. Sesi yang dibuka tanpa database tetapi berakhir di suatu database ditolak.
   - **Trino:** `DELETE` ke `nextUri` query yang belum selesai dibaca; galatnya diabaikan. Client HTTP
     dipakai ulang.
   - Reset yang gagal atau lewat anggaran menutup sesi, tanpa event ke pengguna dan tanpa isi ulang.
   - Pelacak `statement_timeout` dan `sql_select_limit` sesi dikembalikan ke "default server",
     karena reset menghapusnya di server.
6. **Sesi dibuang tanpa reset** (`Fate::Discard`) bila di-cancel, rusak, panggilannya berhenti di
   tengah (misalnya Stop), entry-nya sudah dipensiunkan, atau terowongannya mati. Karena sesi seperti
   itu tidak pernah kembali, `CancelRequest` PostgreSQL atau `KILL QUERY` MySQL yang datang terlambat
   tidak bisa mengenai Run berikutnya. Ini jawaban atas item terbuka bagian 1: cancel terikat pada
   sesi yang tidak akan dipakai ulang, bukan pada identitas statement.
7. **Satu sesi MySQL memegang satu koneksi.** `Conn` dikembalikan producer ke sesi saat hasil dibaca
   sampai habis atau server menolak; statement yang ditinggal menjatuhkannya. Statement kedua saat
   cursor pertama masih hidup ditolak dalam 250 ms, bukan membuka koneksi kedua diam-diam. Transaksi
   yang koneksinya hilang menandai `transaction_lost`, dan sesudahnya hanya `ROLLBACK` yang diterima,
   supaya `COMMIT` tidak berhasil di koneksi autocommit yang baru.
8. **Preview yang terpotong menyimpan sesinya** di pool (`83a9d5e`):
   - **MySQL:** `SET SESSION sql_select_limit = limit+1` (digabung dengan `max_execution_time` dalam
     satu `SET`), hanya untuk statement tunggal yang terbaca `ReadOnly` di semua pembacaan leksikal
     server, supaya batas tidak pernah mengurangi baris yang ditulis `INSERT ... SELECT` atau dikunci
     `FOR UPDATE`. Server berhenti sendiri, koneksi kembali, dan reset membersihkan batasnya.
   - **Trino:** query dihapus saat reset.
   - **PostgreSQL:** sesi ditutup (menutup socket menghentikan backend) dan pool mengisi ulang satu
     sesi di latar (`Fate::DiscardAndRefill`). Cancel lalu drain ditolak, karena `CancelRequest` yang
     terlambat bisa mengenai statement berikutnya.
   - CLI dan MCP tetap melakukan cancel seperti sebelumnya.
9. **Statement yang teksnya sudah terkirim tidak pernah dikirim ulang.**
   - Kegagalan setelah teks keluar dari client (MySQL: soket putus, task berakhir) adalah galat
     `Permanent` dengan kode `2013`, bukan `Transient`, sehingga `retry::execute` tidak mengulangnya.
   - Sesi pakai ulang yang gagal pada statement pertamanya tanpa pernah dijawab server
     (galat `Connect` yang membuktikan teks belum terkirim) diganti **sekali** dan statement dikirim
     ulang, hanya bila ia baca (`ReadOnly`) atau tepat `BEGIN` / `START TRANSACTION`. Write tidak
     pernah dikirim ulang; sesi ditandai `broken` dan Run berikutnya mendapat sesi baru.
   - MySQL memetakan galat IO di tengah query ke `Connect`, jadi `Connect` saja bukan bukti belum
     terkirim di driver itu; itu sebab aturan di atas dipersempit dari versi awal blueprint.
10. **Pipelining PostgreSQL dihapus (penyimpangan dari blueprint).** Blueprint §11.1 menggabungkan
    `prepare` dan `simple_query_raw` dalam satu penerbangan untuk preview baca. Review menunjukkan
    bahwa dengan itu Run `read_only` bisa menghapus baris lewat teks yang dibaca lexer generik sebagai
    satu statement (escape `E''`, komentar bersarang, `$` di identifier), karena query sudah jalan
    sebelum `prepare` sempat menolaknya. PostgreSQL kembali men-describe dulu, dan tes regresi
    menjalankan tiga payload itu pada sesi pakai ulang yang read-only. Sub-langkah pencabutan `prep`
    MySQL di blueprint tidak dijadikan syarat; MySQL hanya melewati `prepare` terpisah untuk preview
    terpotong.
11. **Read-only ditegakkan server** (`a25fa50`). `Session::enforce_read_only` mengirim
    `SET SESSION default_transaction_read_only = on` (PostgreSQL) atau
    `SET SESSION TRANSACTION READ ONLY` (MySQL); Trino tidak punya padanan dan tidak mendapat apa-apa.
    `commands::open` memanggilnya setiap kali mode Safe Mode yang terselesaikan adalah `read_only`, dan
    Run gagal bila sesi tidak bisa dibuat read-only. Lease mengingat bahwa sesinya read-only dan
    menerapkannya lagi setelah reconnect. Reset membersihkannya (`RESET ALL`, `COM_RESET_CONNECTION`),
    dan Safe Mode tetap diputuskan per Run, sebelum checkout. Lapisan ini ada supaya salah baca
    classifier tidak cukup untuk menulis data.
12. **Terowongan** dibuka sekali per kunci (paling lama `TUNNEL_OPEN = 30 dtk`, karena `qh-tunnel`
    tidak punya timeout connect) dan dipakai bersama oleh sesi pool, sesi di luar reservasi, dan sesi
    `LongOp`. Terowongan yang mati membuat sesi idle entry itu ditutup dan terowongan dibuka ulang.
13. **Operasi panjang di luar pool.** `export`, `to_table`, dan `import_data` lewat `LongOpEngine`:
    sesi baru yang tidak pernah masuk pool, di atas terowongan kunci yang sama. Ekspor yang masih
    berjalan menahan entry dan terowongannya dari evict.
14. **Warm-up.** Memilih koneksi memanggil `warm_up`, yang membuka satu sesi di latar bila kunci
    belum punya. Hanya connect, tanpa query, dan membaca Keychain di luar main thread tanpa prompt.
    Kegagalannya dibuang; Run sungguhan melaporkan galat yang sama lewat jalur biasa.
15. **Keepalive TCP 60 dtk** di PostgreSQL (`keepalives_idle`) dan MySQL (`tcp_keepalive`) untuk
    mengurangi sesi idle yang basi.

### Alasan bagian 2

1. **Pool di balik trait `Engine`** karena mengubah bentuk perintah akan menyentuh CLI, MCP, dan
   korpus golden, yang kontraknya beku. Dengan seam ini paritas dibuktikan secara diferensial.
2. **Reservasi, bukan antrean**, karena antrean membuat Run atau pohon menunggu ekspor lambat di tab
   lain. Menunggu satu reset (1 RTT) lebih murah daripada connect (3 sampai 8 RTT), dan hanya itu yang
   ditunggu.
3. **Buang, bukan reset, untuk sesi yang diragukan.** Membuang selalu benar; reset pada sesi
   berstatus tak pasti tidak bisa dibuktikan benar. Ini juga yang menutup cancel terlambat.
4. **`DISCARD ALL` diurai** karena `DEALLOCATE ALL` merusak cache tipe client, dan reset harus tetap
   memenuhi O-6.
5. **Tidak ada resend setelah teks terkirim** karena `classify` tidak tahu bahwa `SELECT nextval('s')`
   atau `SELECT f()` menulis, dan galat IO tidak membuktikan server belum menjalankan statement.
6. **Server sebagai lapis kedua Safe Mode** karena pembacaan teks bisa salah, dan kesalahan itu
   pernah terbukti (lexer PostgreSQL, Trino, dan MySQL di commit `1f3182d` dan `a25fa50`).

### Konsekuensi bagian 2

#### Positif

- Run pada koneksi hangat tidak lagi membayar TCP, TLS, dan autentikasi; level pohon MySQL dan Trino
  tetap hangat karena database dan katalog adalah konteks, bukan kunci.
- Transaksi MySQL kini atomik di CLI, MCP, dan app, dan `FOREIGN_KEY_CHECKS` serta
  `max_execution_time` berlaku pada statement berikutnya.
- Cancel terlambat tidak bisa mengenai Run lain, tanpa perlu mengikat cancel ke identitas statement.
- Safe Mode `read_only` di PostgreSQL dan MySQL tidak lagi bergantung sepenuhnya pada pembacaan teks.
- Satu handle SQLite dan satu terowongan per kunci mengurangi biaya perintah lokal dan ekspor.

#### Negatif

- **Sesi idle memegang koneksi server:** sampai 3 per kunci ditambah sesi di luar reservasi, dan
  PostgreSQL membuat satu kunci per database yang dibuka di pohon. Kedaluwarsa 5 menit membatasinya;
  tidak ada batas koneksi global lintas kunci.
- **State sesi yang lolos dari reset** (koneksi `dblink`, state ekstensi PostgreSQL) bisa terbawa ke
  Run berikutnya. Reset mengikuti kontrak `DISCARD ALL` dan `COM_RESET_CONNECTION`, bukan lebih.
- **Preview PostgreSQL yang terpotong membayar satu connect** di latar (sesi dibuang lalu diisi
  ulang), dan pool penuh membuka sesi di luar reservasi dengan biaya lama.
- **Write pertama pada sesi idle yang basi gagal sekali** dengan pesan jelas dan tidak diulang; Run
  berikutnya mendapat sesi baru.
- **Read-only server tidak ada di Trino**, sehingga di sana Safe Mode tetap hanya bergantung pada
  classifier. Pembacaan teks yang terlalu hati-hati menolak beberapa teks tidak berbahaya
  (menyebut `client_encoding`, kolom bernama `key`/`no`/`share` tepat sesudah `FOR`).
- Perbaikan sesudah review ronde pertama (charset MySQL setelah reset, tunggu 250 ms dan
  `transaction_lost`, perbaikan Safe Mode sesudah review keamanan) terverifikasi oleh tes live tetapi
  belum ditinjau ulang; kebijakan dua ronde review menghentikannya di situ.

#### Bukti bagian 2

- `crates/qh-ffi/tests/host.rs` (offline, dengan `Connector` palsu, dan live per driver), tes
  integrasi `qh-driver-mysql` untuk satu sesi satu koneksi, dan tes regresi tiga payload Safe Mode
  pada sesi PostgreSQL pakai ulang yang read-only. Gate yang dilaporkan di commit: G-RUST hijau
  (914 tes), tes live `safe_mode`, `real_server`, dan `host` (81), golden 12/22 dengan 10 delta yang
  sama dan sudah terklasifikasi.
- Angka Fase 2 (S1, S3, S4, level pohon hangat) diukur di sesi bench W3-T3, bukan di ADR ini.

## Referensi

- ADR-0009 (`catch_unwind` di FFI), ADR-0010 (QoS dan runtime utama), ADR-0016 (batas waktu
  statement, dasar pikir mekanisme server; addendum tentang reset), ADR-0017 dan ADR-0026 (Safe Mode
  dan lantainya, yang kini punya lapis server).
- `docs/architecture/blueprints/fase-2-engine-host.md` (desain dan verdict architect-reviewer),
  `docs/architecture/performance-plan.md` §5 dan §6.
