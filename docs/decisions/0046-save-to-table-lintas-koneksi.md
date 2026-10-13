# 0046 — Save to Table lintas koneksi: sumber hanya dibaca, tujuan digerbangi Safe Mode-nya sendiri

- **Status:** Diterima dengan syarat. Belum ada kode. Blueprint disetujui architect-reviewer (opus) di putaran 2, 14 Okt 2026, dengan syarat C-1 sampai C-5; kelimanya diterapkan di ADR ini (keputusan 3, 5, 6, dan 8) dan diperiksa review batch engine (langkah 7). Bagian yang butuh server nyata tercatat "pending live" sampai langkah 12.
- **Tanggal:** 14 Okt 2026 (W13-T20 langkah 0)
- **Konteks instruksi:** `docs/architecture/blueprints/w13-save-to-table-across-connections.md` (§2.6 butir 4, §3, §4, §6.6, §7, §12, Verdict putaran 1 dan 2); `docs/architecture/prd-performance-and-parity.md` FR-RUN-08, UC-19, V-17, O-33, PR-02, PR-04; `docs/architecture/development-plan.md` §5 (W13-T20) dan §7; keputusan pemilik O-33 dan permintaan UI-nya (ledger, 14 Okt 2026).
- **Mengamandemen:** ADR-0026 §1 (create di tingkat `confirm`, hanya untuk jalur ini) dan ADR-0028 (`Parameter` mendapat varian `Bytes`). Badan kedua ADR itu sengaja tidak ditulis ulang.
- **Tidak menggantikan:** ADR-0017 (tiga tingkat Safe Mode), ADR-0027 (operasi tabel; celah `confirm` ditambah lantai `no_ddl` tetap tertutup), ADR-0031 (LongOp di luar pool).

## Konteks

Hari ini `to_table` menjalankan `CREATE TABLE … AS` di server koneksi yang sama dan tidak ada baris yang lewat proses engine (`crates/qh-ffi/src/commands.rs:1139`, PR-02). Pemilik meminta "Save to Table" boleh menyeberang koneksi (O-33): query jalan di koneksi A, tabelnya dibuat di koneksi B yang berbeda, misalnya hasil Trino ke PostgreSQL atau MySQL.

Begitu baris menyeberang, enam hal yang tidak ada di jalur CTAS ikut muncul:

1. Ada dua kredensial dan dua Safe Mode dalam satu run. Yang menulis adalah B, jadi Safe Mode B yang harus memutuskan, bukan A.
2. Tipe tidak lagi dipetakan oleh satu server. Setiap pasangan dialek punya nilai yang bisa berubah diam-diam (tanggal nol MySQL, `NaN`, `infinity`, zona waktu, presisi).
3. Nama kolom dan teks tipe datang dari server sumber dan akan masuk ke DDL di server lain.
4. Sumber yang lambat menunggu tujuan, jadi batas statement di sisi sumber memotong transfer panjang (PF-23).
5. Engine harus memutuskan sendiri apa yang bisa dikirim ke B tanpa melewati classifier, karena `BEGIN`, `COMMIT`, dan `SET` terbaca `Unknown` dan `confirm` menolaknya walaupun sudah dikonfirmasi (`crates/qh-sql/src/classify.rs:1637-1655`, `:1805-1807`).
6. Pesan galat server tujuan bisa mengutip nilai baris.

Batas yang mengikat bentuknya: empat daftar invariant #11 dan `app/Generated/` tidak boleh berubah tanpa alasan kuat, MCP tidak boleh mendapatkan jalur tulis baru (NFR-S6), dan golden `to_table` beku (PR-02).

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Bentuk permukaan | **Perluas `to_table` dengan setelan berprefiks `DEST_`** | Empat daftar invariant #11 dan `app/Generated/` tidak berubah. Penolakan MCP berdasar nama (`mcp.rs:1000-1003`) dan rute LongOp (`host.rs:251`) sudah ada | Satu perintah punya dua jalur, jadi pemicunya harus gagal-tertutup |
| | Perintah baru `transfer` | Jalur terpisah | Mengubah empat daftar invariant dan `app/Generated/`, dan penolakan MCP harus ditambah |
| Pemicu jalur lintas koneksi | **Kunci apa pun berprefiks `DEST_`, walaupun kosong** | Setelan tujuan yang sebagian tidak pernah jatuh ke CTAS di koneksi sumber | Dev shell yang menyisakan satu `DEST_` mengubah perilaku `to_table` (dibersihkan harness golden) |
| | `DEST_DB_KIND` atau `DEST_DB_URL` ada dan tidak kosong | Sederhana | Setelan kosong atau sebagian diam-diam menjalankan CTAS di sumber, yang bisa saja prod |
| Koneksi yang sama | **App memilih jalur: UUID sama berarti CTAS dan tidak ada `DEST_*`** | Engine tidak perlu identitas koneksi | Transfer ke koneksi yang identik tetap benar, hanya lebih lambat |
| | Engine menolak berdasar identitas `PoolKey` | Tegas | `PoolKey` tidak memuat Safe Mode dan memakai `DatabaseSlot::Switchable` untuk MySQL dan Trino (`pool.rs:175-178`), jadi sel v1 yang sah ikut ditolak |
| Penulis PostgreSQL | **`COPY … FROM STDIN` teks, dalam jendela, satu transaksi** | Tanpa crate baru, hitungan baris diakui server (`finish()`), `CopyFail` bila sink di-drop | Butuh encoder teks sendiri |
| | `COPY BINARY` | Lebih cepat | Butuh `ToSql` per tipe dan fitur crate. Ditunda |
| Penulis MySQL | **INSERT multi-baris dengan parameter terikat** | Nilai tidak pernah menjadi teks SQL | Butuh `Parameter::Bytes` |
| | `LOAD DATA LOCAL` | Cepat | Ditolak: tidak ada handler yang dipasang, dan tes SEC menguncinya |
| Pemetaan tipe | **Setia atau ditolak (E, W, T, R)** | Tidak ada nilai yang berubah diam-diam | Beberapa pasangan ditolak |
| | Fallback teks untuk semua tipe | Hampir selalu jalan | Kehilangan nilai diam-diam |
| Kontrol transaksi dan setup sesi | **Enum tertutup bertipe di luar classifier (C-4)** | Statement ini tidak mungkin berisi nama dari server, dan create di `confirm` bisa jalan | Satu enum dan satu tes per varian |
| | Lewat classifier | Satu jalur | `BEGIN`/`COMMIT`/`SET` terbaca `Unknown` dan ditolak di `confirm`, jadi create PostgreSQL di `confirm` tidak pernah bisa jalan |

## Keputusan

**`to_table` bercabang ke modul `transfer` begitu ada satu kunci berprefiks `DEST_`. Sumber hanya dibaca. Setiap statement nyata yang dikirim ke tujuan diputuskan per statement oleh Safe Mode tujuan sendiri. Tipe dipetakan setia atau ditolak. Lembar transfer selalu tampil. ADR-0026 §1 dan ADR-0028 diamandemen seperti di keputusan 4 dan 7.**

Rincian yang mengikat:

1. **Setelan dan gagal-tertutup (B-1).** Kunci tujuan sama dengan kunci sumber ditambah prefiks (`DEST_DB_*`, `DEST_SAFE_MODE`, `DEST_SAFE_MODE_CONFIRMED`, `DEST_SAFE_MODE_FLOOR`, `DEST_STATEMENT_TIMEOUT_MS`, `DEST_RETRIES`, `DEST_SSH_*`). `TARGET_*` tetap nama tabel.
   - Pemicunya semantik `is_set` (`env.rs:107-109`), bukan `text`, yang membaca kosong sebagai tidak ada.
   - Sesudah bercabang, `DEST_DB_KIND` dan `DEST_DB_URL` yang keduanya hilang atau kosong, serta `DEST_SAFE_MODE` yang hilang, kosong, atau salah eja, menjadi `Usage` sebelum `step` pertama dan tanpa statement ke sumber. Sisi tujuan tidak mewarisi aturan lama "absen berarti `full`" (`commands.rs:161-172`).
   - `WRITE_MODE=append` bersama `DEST_*` menjadi `Usage`. Mode v1 hanya `create` dan `replace` (OQ-5).
   - `DEST_SAFE_MODE_FLOOR` dan `SAFE_MODE_FLOOR` tingkat atas keduanya menjadi entri lantai lewat `raise` terpisah, dan tidak ada yang menimpa yang lain.
   - `DEST_*` dikupas dari setelan sumber, dan redaksi rahasia diperluas ke kunci berprefiks.
2. **Sumber hanya dibaca.** Tepat satu statement (`qh_sql::statements_dialect`), lalu `require_read` dengan alasan "Save to Table across connections reads the source and nothing else", lalu `guard_for`. Sesi sumber selalu `enforce_read_only`. Safe Mode sumber tidak berubah artinya.
3. **Tujuan digerbangi Safe Mode-nya sendiri, per statement nyata (B-2, B-3).** Keputusan diambil dari entri lantai tujuan (`DEST_SAFE_MODE`, `DEST_DB_READ_ONLY`, read-only milik driver, `DEST_SAFE_MODE_FLOOR`, dan `SAFE_MODE_FLOOR` tingkat atas) dengan dialek tujuan.
   - **DDL** memakai semantik `destructive_mode` atas entri lantai (`commands.rs:445-459`): entri yang melarang DDL menolak, entri `confirm` bertanya, selain itu diizinkan. `destructive_mode` dipecah menjadi fungsi murni atas `&SafeModeFloor`, dan `guard_destructive` yang ada tidak berubah perilakunya.
   - **DML** (`COPY`, `INSERT`) memakai `guard_confirmed` dengan Safe Mode, dialek, dan konfirmasi yang sama. Mode untuk DML diambil dari `resolve()` lantai gabungan tujuan, tidak pernah dari `safe_mode(&dest, engine)`, yang membangun lantainya sendiri tanpa `SAFE_MODE_FLOOR` tingkat atas (C-4).
   - **Pembersih** `DROP TABLE IF EXISTS qh_new_*` atas scratch milik engine dicatat dan diizinkan bila `CREATE` scratch itu diizinkan di run yang sama.
   - **Replace** dengan entri `confirm` apa pun ditolak sebelum connect, karena replace menghapus tabel pengguna (OQ-9).
   - **Statement nyata, bukan templat.** Setiap statement dibangun dari nama dan tipe yang sudah divalidasi, lalu sebelum dikirim dipecah di bawah dialek tujuan, harus tepat satu statement, dan `classify_readings`-nya harus sama dengan kind di rencana. Bila tidak, hasilnya `CliError::Internal`, tidak ada yang dikirim, dan rentang kursor sumber ditutup lewat keputusan 12.
   - Keputusan dicatat di execution log dengan hash statement nyata, sekali per teks yang berbeda. Penolakan di tahap tanpa jaringan dicatat dengan subjek rencananya.

   | Safe Mode tujuan (entri) | create | replace |
   |---|---|---|
   | `full` | jalan | jalan; app tetap meminta konfirmasi destruktif |
   | `no_ddl` | ditolak | ditolak |
   | `confirm` | bertanya, satu konfirmasi | ditolak |
   | `confirm` ditambah lantai `no_ddl` | ditolak (entri), sama dengan ADR-0027 | ditolak |
   | `read_only` | ditolak | ditolak |
4. **Amandemen ADR-0026 §1.** ADR-0026 menetapkan bahwa di `confirm` DDL ditolak apa pun konfirmasinya. Untuk jalur lintas koneksi, create di tujuan `confirm` **bertanya**: satu konfirmasi (`DEST_SAFE_MODE_CONFIRMED=1`, ditambahkan lembar transfer untuk run itu saja) berlaku untuk seluruh rencana yang disebut di lembar, dan DDL di dalamnya diputuskan lewat `destructive_mode` per statement. Semua jalur lain tetap seperti ADR-0026: DDL ditolak di `confirm`.
   - **Asimetri yang diketahui.** `CREATE TABLE … AS` di koneksi yang sama tetap ditolak di `confirm` (lewat `guard_confirmed`), sedangkan create lintas koneksi ke tujuan `confirm` bisa jalan setelah satu konfirmasi. Asimetri ini diterima dan masuk backlog, bukan diperbaiki di sini.
   - Replace di `confirm` ditolak, dan `confirm` ditambah lantai `no_ddl` tetap menolak create (celah ADR-0027 tetap tertutup).
5. **Statement milik engine di luar classifier, sebagai tipe (C-4).** Kontrol transaksi, setup sesi, dan bacaan milik engine diekspresikan sebagai `enum EngineStatement { Begin, Commit, Rollback, StrictMode, Utf8mb4, MaxPacket, ProbePg, ProbeMysql }` dengan teks `&'static str`. `plan.rs` tidak pernah menyerahkan `String` hasil `format!` ke cabang ini. Tes memastikan setiap varian terbaca tepat satu statement di bawah dialeknya. Statement ini diizinkan hanya bila rencana tulisnya diizinkan, dan tidak dicatat di execution log, sama dengan statement sesi milik driver hari ini. Empat `SET` sisi sumber di keputusan 8 juga varian allowlist ini; nama variannya ditentukan saat implementasi, bukan di ADR ini.
6. **Pernyataan MySQL (C-5).** `StrictMode` memasang `SET SESSION sql_mode = CONCAT_WS(',', NULLIF(@@sql_mode, ''), 'STRICT_ALL_TABLES')`, karena `CONCAT(@@sql_mode, ',STRICT_ALL_TABLES')` meninggalkan koma di depan bila `sql_mode` kosong dan penerimaannya oleh MySQL belum dicek. Dibuktikan dengan `sql_mode` kosong di langkah 12. `Utf8mb4` memasang `SET NAMES utf8mb4`, dan tabel dibuat dengan `DEFAULT CHARSET=utf8mb4`. Sesi tujuan tidak di-pool, jadi setelan sesi itu tidak bocor.
7. **Amandemen ADR-0028: `Parameter::Bytes`.** `Parameter` sengaja tertutup (`crates/qh-driver/src/lib.rs:230-236`). Enum itu bertambah satu varian, `Bytes(Vec<u8>)`, dengan lengan di driver PostgreSQL dan MySQL. Driver Trino tidak berubah, karena ia menolak semua parameter. Alasannya satu: nilai biner ke MySQL tidak boleh menjadi literal di teks SQL (lihat T-4 di blueprint, impor MySQL dengan literal tertanam salah di `NO_BACKSLASH_ESCAPES`). Aditif di trait driver lain, semuanya dengan default: `Cursor::column_types`, `Session::copy_in` (default `Usage`), dan `CopyIn { send(Vec<u8>), finish() -> u64 }`. Primitif `copy_in`, pembaca `max_allowed_packet`, dan `Parameter::Bytes` dimiliki W13-T20 dan dipakai ulang W12-T7c (O-34).
8. **Aturan fallback tipe: setia atau ditolak.** Setiap pasangan tipe punya satu hasil: E (exact), W (widen: setia dan lebih lebar, dilaporkan bila berubah), T (fallback teks yang terdokumentasi dan dilaporkan per kolom), atau R (refuse).
   - **Fallback teks hanya untuk sel T** (OQ-7). Sel R tetap R, tidak pernah jatuh ke teks.
   - **Teks tipe dari server (`SqlType.native`) tidak pernah masuk SQL.** DDL hanya berasal dari tabel milik engine (`transfer/types.rs`), dikunci pada `SqlType` yang sudah di-parse. PostgreSQL ke PostgreSQL memakai allowlist OID tipe bawaan, tanpa `oid` dan `reg*` (identitasnya lokal per database). Domain tidak lolos sebagai tipe bawaan karena RowDescription mengirim tipe dasarnya (perilaku server, dibuktikan live di langkah 12), dan tipe ekstensi tetap R.
   - **`money` adalah R (C-1).** Teks `money` bergantung pada `lc_monetary` sesi sumber saat ditulis dan `lc_monetary` tujuan saat dibaca; dua server dengan locale berbeda mengubah nilainya tanpa galat. `money` dikeluarkan dari allowlist bawaan.
   - **Encoder hanya menerima varian `Value` yang diharapkan untuk tipe logisnya, ditambah `Null`.** `Text` di kolom date, timestamp, numeric, atau integer ditolak per nilai, begitu juga `Unknown` dan `Interval` di kolom `Time`.
   - **Ejaan khusus PostgreSQL diteruskan hanya dari PostgreSQL ke PostgreSQL, dan hanya yang cocok dengan pola tertutup ini (C-2)**, ditulis persis di `transfer/encode.rs` dan di fixture:
     - numeric: `NaN`, `Infinity`, `-Infinity`;
     - date dan timestamp: `infinity` dan `-infinity`;
     - date ISO SM: `^\d{4,}-\d{2}-\d{2} BC$`;
     - timestamp ISO SM: `^\d{4,}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(\.\d{1,6})? BC$`;
     - timestamptz ISO SM: seperti timestamp, dengan offset `[+-]\d{2}(:\d{2}){0,2}` sebelum ` BC`.

     Bentuk lain tetap R per nilai. Ke MySQL semuanya R. Alasannya: driver tidak memasang `DateStyle`, jadi tanggal SM dari role dengan `DateStyle = 'SQL, DMY'` dibaca tujuan `MDY` dengan hari dan bulan tertukar tanpa galat. Pola ISO tidak bergantung pada setelan sesi mana pun.
   - **Sesi sumber PostgreSQL memasang setelan bacaan yang tetap (C-3)**, empat `SET` sesi sesudah `enforce_read_only` (boleh di transaksi read-only), di sesi unleased yang ditutup sesudahnya: `SET extra_float_digits = 3`, `SET DateStyle = ISO`, `SET IntervalStyle = postgres`, dan `SET bytea_output = hex`. Tanpa itu, role dengan `extra_float_digits` di bawah 1 mengirim float dan geometri yang dibulatkan, dan driver mem-parse teks itu apa adanya (kehilangan nilai diam-diam).
   - Aturan lain: U+0000 di teks ke PostgreSQL adalah R per nilai; rentang date dan datetime MySQL (1000-01-01 sampai 9999-12-31) dicek per nilai; Trino `time(p>6)` dan `timestamp(p>6)` adalah R (OQ-10); nama kolom ganda ditolak (OQ-8); semua identifier lewat satu fungsi (`transfer/names.rs`) yang menggandakan kutip dialeknya dan menolak NUL, nama terlalu panjang, dan nama berakhir spasi di MySQL.
   - **Pelanggaran menghentikan transfer** dengan kolom, nomor baris sumber, dan aturannya. Nilainya tidak pernah disebut. Galatnya bersisi `target`.
9. **Arti `STATEMENT_TIMEOUT_MS=0` (PF-23).** Konsumen lambat terpotong batas statement di server (terukur: MySQL setelah 282.600 baris, PostgreSQL setelah 19.400 baris), dan transfer adalah konsumen lambat.
   - App mengirim `STATEMENT_TIMEOUT_MS=0` di sisi sumber **hanya bila** batas koneksi sumber adalah default app yang diwarisi (`statementTimeoutMS == nil`). Batas eksplisit per koneksi tetap dikirim dan ditampilkan di wizard (OQ-13).
   - Nilai 0 berarti `statement_timeout(settings)` menjadi `None` (`commands.rs:103-106`), dan driver tidak mengirim apa pun: PostgreSQL tidak mengirim `SET statement_timeout`, MySQL tidak mengirim `max_execution_time` pada sesi baru, Trino tidak mengirim header `query_max_run_time`. Jadi app tidak menambah batasnya sendiri, tetapi **batas yang dipasang DBA di role, database, atau server tetap berlaku**. Transfer yang terpotong olehnya menjadi galat `side: source` yang menyebut batasnya.
   - Sisi tujuan: batas koneksi tujuan berlaku per statement, dan jendela `COPY` menjaga setiap statement di bawahnya. `DEST_STATEMENT_TIMEOUT_MS=0` menutup jendela pada 15 s. `idle_in_transaction_session_timeout` di tujuan bisa memutus transaksi yang menunggu sumber lambat; server me-rollback, dan itu risiko yang dicatat (R-7).
10. **Pesan server yang bisa mengutip nilai (R-9).** Galat yang dibangun engine tidak pernah memuat nilai baris. Pesan server tujuan (pelanggaran tipe, panjang, atau unik) bisa mengutip nilai, dan pesan itu sampai ke event `error` dan panel log. Risiko ini diterima dan dicatat. Transfer tetap di luar `recordHistory`, dan execution log hanya menyimpan hash statement.
11. **Hak akses koneksi tujuan (N-16).** Engine menulis dengan kredensial B dan hanya Safe Mode B yang menggerbangi tulisan itu. Engine tidak bisa melihat grant B. Akibatnya, baris yang hanya terbaca lewat identitas A menjadi terbaca oleh siapa pun yang bisa membaca B. Engine tidak punya kendali di sini. Mitigasinya ada di app: lembar transfer yang selalu tampil dan menyebut kedua koneksi (nama, jenis, `host:port`, database, label lingkungan, status TLS), peringatan ditambah centang eksplisit bila sumber berlabel `prod` dan tujuan tidak (termasuk tanpa label, OQ-3), tujuan terikat UUID dan divalidasi ulang tepat sebelum run, dan tidak ada opsi "jangan tanya lagi".
12. **Cancel sebelum drop di setiap jalan keluar (B-5), atomisitas, dan verifikasi.**
    - Selama kursor sumber hidup, satu-satunya jalan keluar adalah `LiveSource::finish`, yang memanggil `stop_session(source, Some(cursor))` lebih dulu untuk setiap hasil selain "kursor selesai". Tidak ada `?` di rentang itu. Panik yang ditangkap di batas FFI men-drop kursor tanpa cancel; sisa risiko itu diakui (R-16).
    - PostgreSQL memakai satu transaksi (tidak ada sisa). MySQL memakai scratch `qh_new_*` lalu satu `RENAME TABLE`; scratch yang tertinggal bila proses mati dinyatakan di help dan pembersihnya backlog.
    - `rows_read` dari sumber harus sama dengan hitungan yang diakui B sebelum swap atau `COMMIT`. `done.verified: true` hanya bila sama.
13. **UI di belakang sakelar (keputusan pemilik, 14 Okt 2026).** Pemilik meminta UI-nya dibangun sekarang. Pemilih koneksi tujuan ada di belakang sakelar Settings "Experimental: save to a table on another connection", bawaannya **mati** sampai satu run live terhadap server nyata lulus (langkah 12). Pemilik boleh menyalakannya kapan saja. Sakelar itu menggeser scene `chrome-settings*` dari pane yang memuatnya, dan itu dideklarasikan di V-17. Ini menggantikan OQ-11 (UI ditahan sampai verifikasi live). Engine tetap diam selama app tidak mengirim `DEST_*`.
14. **MCP dan CLI.** MCP tidak mendapatkan jalur ini: `to_table` ditolak berdasar nama (`mcp.rs:1000-1003`) dan `settings_for` membangun setelan dari kunci tetap lalu memaksa `read_only`, jadi `DEST_*` tidak bisa disuntik. CLI mendapatkannya; harness golden dan shell pengembang membersihkan `DEST_*`.
15. **Keputusan OQ-1 sampai OQ-13.** Pemilik mendelegasikan pertanyaan terbuka blueprint ke orkestrator, dan semuanya diputuskan 14 Okt 2026. Tabelnya ada di ledger (`target/run/keep/ledger.md`, bagian "W13-T20 step 0 done") dan di bagian "Keputusan orkestrator" blueprint. Penyesuaian yang berlaku di ADR ini: OQ-3 (centang juga untuk tujuan tanpa label), OQ-7 (fallback hanya sel T), OQ-10 (Trino `time(p>6)` ikut ditolak), OQ-11 (digantikan keputusan 13), OQ-12 (primitif milik W13-T20), OQ-13 (0 hanya menggantikan batas yang diwarisi).

### Teks blueprint yang digantikan oleh ADR ini

Badan blueprint tidak diubah. Bila ada beda, ADR ini yang berlaku:

- `blueprints/w13-save-to-table-across-connections.md:490`: `money` ada di allowlist bawaan. **Digantikan C-1:** `money` adalah R.
- `:661-663` dan `:676-678`: kontrol transaksi dan setup sesi sebagai "allowlist tertutup berteks persis", dengan `SET SESSION sql_mode = CONCAT(@@sql_mode, ',STRICT_ALL_TABLES')`. **Digantikan C-4 dan C-5:** enum bertipe `EngineStatement`, dan `CONCAT_WS(',', NULLIF(@@sql_mode, ''), 'STRICT_ALL_TABLES')`.
- `:504-505`: ejaan PostgreSQL diteruskan bila "cocok persis dengan pola ejaan PostgreSQL". **Dipersempit C-2:** hanya pola tertutup di keputusan 8.
- Tabel §7.2 tidak memuat empat `SET` sisi sumber PostgreSQL. **Ditambah C-3**, sebagai varian allowlist keputusan 5.
- `:60`, `:766`, `:933`, `:972`, `:1007`, `:1251`, dan `:1397`: UI tidak masuk `main` sebelum langkah 12 (OQ-11). **Digantikan keputusan 13.**

## Alasan

1. **Tujuan yang menulis, jadi tujuan yang memutuskan.** Safe Mode A melindungi A. Menulis ke B dengan kredensial B di bawah aturan A akan membuat koneksi `full` menjadi jalan masuk ke tujuan `read_only`. Memutuskan per statement atas entri lantai B menjaga semantik ADR-0026 dan ADR-0027 (termasuk `confirm` ditambah lantai `no_ddl`).
2. **Gagal-tertutup lebih penting daripada praktis.** Setelan tujuan yang kosong atau sebagian yang jatuh ke CTAS akan menulis ke sumber, yang bisa saja prod, dengan nama tabel yang dimaksudkan untuk B.
3. **Perluasan `to_table` menang atas perintah baru** karena menjaga empat daftar invariant #11 dan `app/Generated/` tidak bergerak, dan penolakan MCP berdasar nama sudah ada.
4. **Setia atau ditolak** adalah satu-satunya aturan yang bisa dites per pasangan tipe. Fallback teks yang diam-diam adalah cara data pindah dengan arti berbeda; satu pengecualian (sel T) dilaporkan per kolom.
5. **Teks yang masuk ke SQL tujuan hanya boleh berasal dari engine atau dari nama yang sudah divalidasi dan dikutip.** Allowlist bertipe membuat statement milik engine tidak bisa dipalsukan oleh nama kolom dari server sumber, dan cek struktur per statement menutup sisanya.
6. **Parameter terikat untuk MySQL** memindahkan escaping ke driver, alasan yang sama dengan ADR-0028, dan menghindari literal tertanam yang salah di `NO_BACKSLASH_ESCAPES`.
7. **`STATEMENT_TIMEOUT_MS=0` hanya menggantikan default yang diwarisi** supaya pilihan sadar pemilik koneksi tetap dihormati, dan karena PF-23 sudah terukur.

## Konsekuensi

### Positif

- **POS-001.** Hasil query bisa dipindah antar koneksi tanpa file perantara, dengan Safe Mode tujuan yang menegakkan.
- **POS-002.** Jalur koneksi yang sama dan golden `to_table` tidak berubah.
- **POS-003.** Tidak ada perintah FFI baru dan tidak ada regenerasi `app/Generated/`. Engine diam sampai app mengirim `DEST_*`.
- **POS-004.** Primitif `copy_in`, `max_allowed_packet`, dan `Parameter::Bytes` dipakai ulang impor (W12-T7c).

### Negatif

- **NEG-001.** Asimetri `confirm`: create lintas koneksi ke tujuan `confirm` bisa jalan setelah satu konfirmasi, sedangkan CTAS koneksi yang sama ditolak. Backlog.
- **NEG-002.** Hak akses B tidak terlihat engine (keputusan 11). Sisa risikonya diterima.
- **NEG-003.** Pesan server tujuan bisa mengutip nilai (R-9).
- **NEG-004.** Panik di rentang kursor hidup men-drop kursor tanpa cancel (R-16). Rentang itu bebas `unwrap` dan `expect` sebagai aturan review.
- **NEG-005.** Scratch MySQL tertinggal bila proses mati (R-5); pembersih otomatisnya backlog.
- **NEG-006.** Batas yang dipasang DBA tetap memotong transfer panjang, dengan galat `side: source` yang menyebut batasnya (R-6).
- **NEG-007.** Execution log tidak membawa identitas koneksi, jadi keputusan kedua sisi hanya terbedakan dari hash statement nyata. Kolom sisi masuk backlog (migrasi `qh-storage`).

### Belum ada (kontrak, bukan kode)

- Seluruh modul `transfer/`, `table_swap.rs`, `Engine::for_settings`, `CliError::Sided`, `SqlType`, `Session::copy_in`, dan `Parameter::Bytes`.
- Bukti live: deskriptor di kursor nyata, `COPY`, `Bytes`, `max_allowed_packet`, C-2 dan C-3 terhadap server dengan `DateStyle` dan `extra_float_digits` bukan bawaan, C-5 dengan `sql_mode` kosong, dan angka rows/s serta RSS. Semuanya langkah 12, menunggu pemilik menyetujui kontainer `qh-*` dibuat ulang.
- UI dan sakelarnya (langkah 8 sampai 11).

## Bukti

- Blueprint: `docs/architecture/blueprints/w13-save-to-table-across-connections.md` §2.6, §3, §4, §6.6, §7, §12, "Verdict architect-reviewer" (putaran 1: perubahan diperlukan, B-1 sampai B-7; putaran 2: approved dengan syarat C-1 sampai C-5, 14 Okt 2026).
- Klaim kode di blueprint dibaca ulang terhadap `main` di `a5fbf11` oleh reviewer. Tidak ada build, tes, atau koneksi database yang dijalankan untuk ADR ini.
- PF-23 terukur: `target/run/keep/ledger.md` (MySQL setelah 282.600 baris, PostgreSQL setelah 19.400 baris; perilaku dikunci tes).
- Status: C-1 sampai C-5 "approved with conditions, pending check at step 7" (O-20).

## Referensi

- ADR-0017 (Safe Mode), ADR-0026 §1 (diamandemen), ADR-0027 (operasi tabel), ADR-0028 (parameter binding; diamandemen), ADR-0031 (LongOp dan tunnel), ADR-0039 (SSH TOFU), ADR-0040 (TLS per koneksi).
- `docs/architecture/prd-performance-and-parity.md` FR-RUN-08, UC-19, V-17, O-33, PR-02, PR-04, NFR-S2, NFR-S6, §10 (copy object).
- `docs/architecture/development-plan.md` §5 (W13-T20) dan §7 (rantai, O-34).
- `docs/invariants.md` #1, #7, #11.
- Tugas penerus: W13-T20 langkah 1 sampai 12.
