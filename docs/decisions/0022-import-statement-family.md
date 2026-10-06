# 0022 — Keluarga statement pada impor, dan foreign key dinyalakan kembali

- **Status:** Diterima
- **Tanggal:** 29 Sep 2026 (Fase 5.1, irisan kedua)
- **Konteks instruksi:** `docs/architecture/tablepro-adoption-plan.md` §7 (5.1);
  `docs/architecture/tablepro-source-study.md` §3

## Konteks

ADR-0019 bernalar soal `import_data` dan mencatat apa yang sengaja ditinggalkannya: keluarga
**baris** mendarat (CSV dan XLSX ke tabel bernama), keluarga **statement** tidak. Keputusan desain
paling berharga dari studi sumber justru pembelahan itu — sebuah plugin membawa
`requiresTargetTable`, dan dua jawabannya adalah dua keluarga dengan satu runner, satu himpunan mode
kesalahan, dan satu kebijakan transaksi. Berkas `.sql` termasuk keluarga yang membawa statement-nya
sendiri.

Dua batas di pohon ini mengikat pilihannya. Engine punya tiga pemanggil (app, CLI, server MCP), jadi
pembelahannya harus hidup di `qh-ffi` tempat ketiganya menjangkaunya, bukan di sebuah sheet Swift. Dan
`qh-sql` sudah memiliki satu-satunya cara yang benar untuk menemukan akhir sebuah statement — `scan`
tahu bahwa `;` di dalam literal, komentar, atau body dollar-quoted Postgres adalah teks — jadi pemecah
kedua di jalur impor adalah mode kegagalan yang harus dihindari, bukan implementasinya.

Studi menyebut satu hal di area ini yang mudah setengah jalan: pemeriksaan foreign key dimatikan
untuk pemuatan massal dan harus dinyalakan **kembali di kedua jalur keluar**, commit dan rollback.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Permukaan | **Perluas `import_data` dengan jenis sumber** | Invariant #11 tidak tersentuh; `.csv` dan `.sql` gagal dengan cara yang sama | Bentuk `done` bercabang per keluarga |
| | Perintah kedua `import_sql` | Dua perintah yang tegas | Perintah baru berarti empat daftar yang dijaga tangan; lebih banyak permukaan untuk satu kebijakan bersama |
| Pemecah | **`qh_sql::statements_with_lines`** | Scanner yang sama dengan `check` dan classifier-nya | Tidak ada; itu scanner yang ada plus nomor baris |
| `GO` | **Bukan pemisah** | Satu pemecah, tanpa grammar client-command | Berkas ber-`GO` gagal di server |
| Default FK | **`FOREIGN_KEYS=on`** | Tidak butuh privilege, tidak ada baris yatim diam-diam | Pemuatan massal yang butuh pemeriksaan mati harus meminta |
| | `off` secara default | Sesuai perilaku pemuatan massal TablePro | `session_replication_role` butuh superuser di PostgreSQL |
| Pemulihan FK | **Satu `finish` untuk kedua jalur** | Epilognya tidak bisa terlupa di salah satu jalur | — |

## Keputusan

**`import_data` bertambah keluarga statement, dipilih dari `IMPORT_FORMAT` atau ekstensi berkasnya,
dan kedua keluarga berjalan di bawah satu kebijakan `ON_ERROR` dan satu kebijakan transaksi.
Pemecahnya milik `qh-sql`. `FOREIGN_KEYS=off` mematikan pemeriksaan server untuk impornya dan
menyalakannya kembali saat commit maupun saat rollback.**

Rincian yang mengikat:

1. **Tanpa perintah baru.** `IMPORT_FORMAT=sql`, atau ekstensi `.sql`, memilih keluarga statement;
   `TARGET_TABLE`, `COLUMNS`, delimiter dan sheet diabaikan. Keempat daftar invariant #11 tidak
   tersentuh dan permukaan FFI tidak berubah — keluarga statement adalah jenis sumber, bukan perintah.
2. **Satu pemecah.** Statement datang dari `qh_sql::statements_with_lines`, yang berbagi scan dengan
   `qh_sql::statements` dan `qh_sql::check`. Sebuah statement dikirim sendiri, dijaga terhadap Safe
   Mode sekali lagi saat dikirim (keluarga baris menjaga `INSERT`-nya dengan cara yang sama). Kalau
   sebuah `;` membelah berbeda di sini dan di editor SQL, itu bug di salah satu scanner, bukan
   ketidaksepakatan antara dua.
3. **Tanpa `GO`.** `GO` adalah client command di SQL Server dan bukan statement di PostgreSQL, MySQL
   atau Trino. Menghormatinya berarti scanner kedua dengan aturannya sendiri; baris `GO` dikirim
   sebagai bagian dari statement-nya dan servernya menolaknya.
4. **Kebijakan yang sama dengan baris.** `stop` (default) rollback, `commit` mempertahankan prefix dan
   melaporkan di mana ia berhenti, `skip` melewati satu statement buruk dan tidak memakai transaksi —
   alasannya milik ADR-0019 dan tidak diulang. Driver tanpa transaksi melaporkan
   `disposition: "written"` alih-alih mengklaim rollback.
5. **Nomor baris dan cap error.** Setiap pesan menyebut baris 1-based statement-nya, dari scan yang
   sama yang membelahnya, dan cap `MAX_ERRORS` 1.000 memotong berkas yang salah di mana-mana.
   Keduanya sudah ada untuk baris; ini keluarga statement yang menyamakannya.
6. **Foreign key.** `FOREIGN_KEYS` default `on`, yang membiarkan servernya apa adanya. `off`
   menjalankan prolog sebelum `BEGIN` dan epilog setelah transaksi berakhir, dari satu `finish` yang
   dipanggil di jalur error, jalur commit, dan jalur rollback. PostgreSQL adalah
   `SET session_replication_role = replica` / `DEFAULT`; MySQL adalah `FOREIGN_KEY_CHECKS = 0` / `1`;
   Trino tidak punya sakelar dan `FOREIGN_KEYS=off` ditolak dengan menyebut namanya sebelum koneksi
   dibuka. `done` melaporkan `foreign_keys`.
7. **Bentuk `done` keluarga statement.** `statements` (jumlah yang diterapkan), `format: "sql"`,
   `streams: false` (berkasnya dibaca utuh karena pembelahannya membutuhkannya utuh), plus kunci
   `mode`, `transaction`, `disposition`, `errors`, `errors_truncated`, `stopped_at`, `cancelled`,
   `foreign_keys` dan `query_id` yang sama dengan keluarga baris. Ia membuang `rows` dan `rejected`,
   yang tidak berarti untuk sebuah script; kunci yang absen bukan null, dan decoder app-nya belum
   dibangun untuk kedua keluarga.

## Alasan

1. **Satu kebijakan, dua keluarga adalah pelajaran studi itu sendiri.** Bagian yang menarik bukan
   batas plugin-nya, melainkan bahwa runner-nya memiliki transaksi dan mode-mode-nya, dan kedua
   keluarga mendapatkannya. Perintah kedua akan menduplikasi kebijakannya atau membelahnya.
2. **Pemecah kedua adalah bug kebenaran yang menunggu terjadi.** `qh-sql` ada karena `rstrip(";")`
   yang naif menghapus karakter yang diketik pengguna. Impor `.sql` yang membelah dengan `split(';')`
   akan menemui cacat yang sama pada body fungsi dollar-quoted atau `';'` di dalam nilai.
3. **Barisnya satu-satunya pegangan pada berkas besar.** "Statement 8.412 gagal" adalah yang membuat
   pemuatan sepuluh ribu statement bisa diperbaiki; keluarga baris sudah tahu ini.
4. **Pemulihannya bagian yang mudah setengah jalan.** Studi menyebutnya, dan kegagalannya senyap:
   impor yang mematikan pemeriksaan lalu kembali di jalur error meninggalkan sesinya tanpa penjaga.
   Menyalurkan setiap jalan keluar lewat satu `finish` adalah perbaikannya, dan kedua jalur keluar
   diuji.
5. **Default `off` akan mengubah perilaku dan butuh superuser.** Di PostgreSQL sakelarnya
   `session_replication_role`, yang butuh superuser atau privilege `SET`, dan ia mematikan lebih dari
   foreign key (setiap trigger dan rule). Mengizinkan baris yatim secara diam-diam bukan default yang
   pantas dipilih untuk orang lain. `off` adalah permintaan yang pemanggil ajukan.

## Konsekuensi

- **Permukaan tidak berubah.** `import_data` tetap perintah kedua puluh satu; tidak ada daftar
  invariant #11 yang bergerak, dan `./app/build-ffi.sh` tidak diperlukan karena tidak ada signature
  yang diekspor berubah. Peristiwa `done` bertambah kunci (`statements`, `foreign_keys`) dan, untuk
  berkas `.sql`, melaporkan `format: "sql"`; protokol peristiwa bersifat aditif menurut catatan
  stabilitas MCP, dan tidak ada kasus golden beku yang mencakup `import_data`.
- **`qh-sql` bertambah satu fungsi publik.** `statements_with_lines` dan nilai `ScriptStatement`,
  dengan `statements` kini mendelegasikan ke keduanya supaya keduanya tidak bisa berbeda.
- **PostgreSQL hidup membuktikan sakelarnya, bukan hanya urutannya.** Baris anak dimuat sebelum
  induknya dengan `FOREIGN_KEYS=off`, dan pemeriksaannya berlaku lagi setelah commit maupun rollback;
  `crates/qh-ffi/tests/import_live.rs` adalah buktinya, digerbangi `QH_TEST_POSTGRES=1`.
- **Masih belum dibangun.** Lembar mapping impor app; MySQL tidak diuji hidup untuk ini (tidak ada
  container MySQL di sesi ini), jadi ejaan `FOREIGN_KEY_CHECKS`-nya hanya dipatok tes unit dan
  pembacaan kode. Penolakan Trino adalah tes unit; ia tidak butuh server.

## Addendum 7 Okt 2026 (W12-T7b, langkah 1): badan rutin, direktif klien, dekode byte

Addendum ini mengubah butir 2 dan 3 di atas hanya sejauh yang dibutuhkan satu pemecah yang jujur
untuk berkas hasil dump. Langkah 2 (menghormati `DELIMITER`, mengosongkan baris kontrol psql di
tempat) tetap menunggu keputusan pemilik dan tidak dikerjakan di sini.

### Keputusan

1. **Badan program tersimpan MySQL adalah satu statement.** `CREATE PROCEDURE p() BEGIN DELETE FROM
   a; DELETE FROM b; END` dibaca seperti grammar server (`sp_proc_stmt`): `;` di dalam badan adalah
   isi, bukan pemisah. Sebelumnya `DELETE FROM b` bisa berjalan sendirian di bawah `ON_ERROR=skip`
   atau `SCRIPT_POLICY=continue` begitu potongan pertama gagal. Pemecahnya
   `qh_sql::statements_with_lines_dialect` (dan `statements_dialect`, `statements_agreeing`,
   keputusan guard), jadi guard dan pengiriman melihat statement yang sama. `walk` dan
   `scan_dialect` tidak berubah: `Scan.separators` tetap posisi `;` mentah, karena daftar statement
   editor dibangun dari `walk` per jendela dan tidak bisa membawa status badan lintas jendela.
2. **Yang dikenali sebagai badan.** Setelah header `CREATE [DEFINER = …] {PROCEDURE | FUNCTION |
   TRIGGER | EVENT}` (dan `ALTER EVENT … DO`), bila statement pertamanya sebuah blok: `BEGIN`, `IF`,
   `CASE`, `LOOP`, `WHILE`, `REPEAT`, boleh diawali `label:`. Badan sederhana (`… RETURN 1;`)
   berakhir di `;` pertamanya seperti biasa. `BEGIN NOT ATOMIC` (MariaDB) juga dikenali. Di dalam
   badan, blok hanya dibuka dan ditutup di awal statement; awal statement mengikuti `;`, `BEGIN`,
   `LOOP`, `REPEAT`, `THEN`, `ELSE`, `WHILE … DO`, dan daftar kondisi `DECLARE … HANDLER FOR`. `CASE`
   di tengah statement adalah ekspresi, jadi `END`-nya bukan penutup blok. `BEGIN` sendirian tetap
   transaksi.
3. **Arah gagalnya.** Keraguan selalu jatuh ke pembacaan lama: header yang tidak dikenali dipecah di
   setiap `;` seperti sebelumnya, dan badan yang tidak pernah tertutup menelan sisa berkas menjadi
   satu statement, yang ditolak server sebagai galat sintaks tanpa menjalankan satu pun potongannya.
   Statement yang membuka badan selalu diawali `CREATE`, `ALTER` atau `BEGIN`, yang ditolak setiap
   mode di bawah `full`, jadi sisa yang tertelan tidak lolos dari Safe Mode.
4. **Direktif klien ditolak dengan namanya sebelum connect** (`qh_sql::client_directive`, dipanggil
   `import_statements` sebelum guard): baris yang diawali `DELIMITER` (MySQL), dicari per baris dan bukan per statement karena klien mysql
   membacanya juga setelah `USE db` atau `\G` yang tidak memakai `;`, sehingga tidak ada statement yang
   diawali dengannya; kolom bernama `delimiter` di awal baris ikut ditolak (gagal tertutup, backtick
   menghindarinya); baris backslash
   psql di luar string, komentar dan nama berkutip, termasuk `\restrict` dan `\unrestrict` milik
   pg_dump (PostgreSQL); dan `COPY … FROM STDIN`, yang barisnya adalah baris berikutnya di berkas.
   Pesannya menyebut nomor baris dan nama direktifnya. `\restrict` tidak lagi menempel ke statement
   pertama.
5. **Dekode byte** (DBX-56 bagian a, pola `json_source.rs`): BOM UTF-8 dibuang, UTF-16 dengan BOM (LE
   dan BE) didekode. Berkas lain harus UTF-8 sah; galatnya menyebut offset dan baris byte pertama
   yang buruk, dan menyarankan `--hex-blob` untuk kolom biner. Fallback GBK tidak diadopsi.

### Ditolak dengan alasan tertulis

- **B-14a, `/*!40101 SET … */;` ditolak classifier di bawah mode non-`full`.** Tetap ditolak. `SET`
  bisa mengubah `sql_mode` atau character set, dan keduanya menggeser cara byte berikutnya dibaca,
  yaitu justru yang dijaga scanner. Classifier tidak mengklaim tahu, jadi `Unknown`. Restore yang
  butuh baris itu berjalan di bawah `full`. Yang menjadi kewajiban pemecah hanyalah tiap baris
  menjadi statement sendiri, dan itu dipatok tes. `classify.rs` tidak berubah.
- **B-13a, `DELIMITER`.** Berkas mysqldump dengan rutin selalu memakai `DELIMITER ;;`, jadi di
  langkah 1 berkas semacam itu ditolak dengan nama (butir 4), bukan dipecah di tengah badan. Yang
  berjalan sekarang adalah rutin tanpa `DELIMITER` (skrip tulisan tangan, keluaran alat lain).
  Menghormati `DELIMITER` adalah langkah 2.
- **DBX-56 bagian b, literal `_binary '…'` mysqldump.** Belum. Byte mentah di dalam literal bukan
  UTF-8 dan butuh pemecah berbasis byte; berkasnya kini ditolak dengan offset dan baris (butir 5).
  Bergantung pada keputusan langkah 2.

### Konsekuensi

- Permukaan FFI tidak berubah (`./app/build-ffi.sh` tidak perlu). `qh-sql` bertambah
  `client_directive` dan `ScriptRefusal`.
- Daftar statement editor (`qh-editor`, dari `walk`) masih memecah di setiap `;`. Run di bawah kursor
  pada badan rutin masih mengirim potongan; menyatukannya butuh status badan di analyzer inkremental
  editor dan dicatat sebagai tindak lanjut.
- PostgreSQL 14+ `BEGIN ATOMIC … END` (badan SQL-standard, dipakai pg_dump) belum dikenali dan masih
  dipecah di setiap `;`. Ditolak sebagai lingkup W12-T7b, dicatat sebagai tindak lanjut.
- `statements_with_lines_dialect` menghitung nomor baris secara inkremental; sebelumnya setiap
  statement memindai ulang berkas dari awal.
