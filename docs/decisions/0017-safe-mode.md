# 0017 — Safe Mode ditegakkan di engine, bukan di UI

- **Status:** Diterima
- **Tanggal:** 29 Sep 2026 (Fase 3)
- **Konteks instruksi:** `docs/architecture/tablepro-adoption-plan.md` §5 (3.2), §0, §10

## Konteks

Rencana §3.2 meminta Safe Mode per koneksi dengan tiga tingkatan: penuh, tolak DDL, read-only. Yang
menahan harus engine, bukan UI, karena engine punya tiga pemanggil dan hanya satu yang punya jendela:
CLI, server MCP, dan app. Aturan yang hidup di sebuah picker Swift adalah aturan yang dua pemanggil
lainnya tidak pernah lihat.

Dua hal di pohon ini yang mengikat keputusan itu: `to_table` punya mode `replace` yang menjalankan
`DROP TABLE IF EXISTS` sebelum query-nya (`crates/qh-ffi/src/commands.rs`), dan Fase 2 baru saja
membuka permukaan MCP yang, seperti dicatat ADR-0015, **belum** menegakkan apa pun pada level SQL.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Tempat menegakkan | **Engine (`qh-sql` + `qh-ffi`)** | Ketiga pemanggil tunduk pada aturan yang sama; MCP jadi read-only sungguhan | Aturan hidup jauh dari tombol yang menjelaskannya |
| | Picker di UI app | Dekat dengan pengguna | CLI dan MCP tidak melihatnya; janji keamanan yang tidak berlaku di dua dari tiga jalur |
| Bentuk level | **Tiga tingkat bernama** | "Boleh menulis tapi tidak mengubah struktur" adalah jalan tengah yang nyata | Satu tingkat lebih banyak untuk dijelaskan |
| | Boolean `is_read_only` | Sudah ada di skema | Tidak bisa menyatakan tingkat tengah |
| Cara mengenali statement | **Klasifikasi teks yang konservatif** | Tidak butuh server; berlaku sebelum connect | Tidak bisa melihat semantik: fungsi yang menulis, `WITH` yang menulis di klausa lain |
| | Serahkan ke privilege database | Paling benar | Bukan hal yang bisa dilakukan engine; pengguna tetap bisa salah jalankan |

## Keputusan

**Tiga tingkat `full` / `no_ddl` / `read_only`, ditegakkan di engine oleh `qh_sql::classify`, dengan
aturan konservatif — yang tidak bisa dikenali diperlakukan sebagai write dan ditolak di semua mode
selain `full`; tingkatnya disimpan per koneksi dan dikirim sebagai `SAFE_MODE`; server MCP selalu
berjalan `read_only`.**

Rincian yang mengikat:

1. **Klasifikasi adalah pengenalan sintaks, bukan semantik.** Ia melihat kata kunci pemimpin dan
   setiap kata telanjang di statement, semuanya setelah `qh_sql::scan` membuang literal, identifier
   berkutip, komentar dan body dollar-quoted — sehingga `DROP` di dalam string adalah teks dan tabel
   bernama `drop2` bukan `DROP`.
2. **Yang tidak dikenali ditolak, bukan diizinkan.** `SET`, `BEGIN`, `COMMIT`, `USE`, `PREPARE`
   tergolong `Unknown` dan ditolak oleh `no_ddl` maupun `read_only`. Statement yang tidak dipahami
   adalah statement yang efeknya tidak bisa diprediksi; menolaknya adalah satu-satunya kegagalan yang
   bisa dibatalkan.
3. **Satu script ditolak seluruhnya bila satu statement ditolak**, dan penolakannya menyebut nomor
   statement dan memenggalnya. Menjalankan bagian sebelum statement yang ditolak berarti menjalankan
   sesuatu yang tidak disetujui pengguna.
4. **`to_table` diklasifikasi pada statement yang benar-benar dikirim** — `DROP`/`CREATE TABLE AS`/
   `INSERT` hasil bangunan perintah, bukan hanya SELECT pemanggil — karena itulah yang berjalan.
5. **MCP selalu `read_only`.** Setiap panggilan MCP menambahkan `SAFE_MODE=read_only`, apa pun tingkat
   koneksinya. Ini yang menutup celah yang ADR-0015 nyatakan terbuka.
6. **Nilai yang tidak dikenal ditolak dengan menyebut namanya.** Sama seperti `sslmode`; salah ketik
   di setelan keamanan bukan keputusan yang boleh diambil diam-diam.

## Alasan

1. **Aturan keamanan harus berlaku di tempat query benar-benar dijalankan.** Karena `qh-ffi` adalah
   satu-satunya pintu masuk ketiga pemanggil, di sanalah satu-satunya tempat aturan ini bisa berlaku
   untuk semuanya.
2. **Konservatif karena kegagalan yang tidak bisa dibatalkan.** Alternatif "izinkan yang tidak bisa
   dibaca" membuat classifier gagal ke arah yang salah. Membaca teks tidak bisa setara dengan
   menganalisis query, dan batas itu ditulis di modulnya alih-alih disembunyikan.
3. **`SELECT … FOR UPDATE` ditolak** karena ia mengambil kunci baris. Secara sintaksis ia memang
   write, dan koneksi read-only yang mengizinkannya sedang membagikan kunci.
4. **Tingkat disimpan di koneksi, bukan di app.** Sebuah koneksi yang dipindah antara app dan command
   line berarti hal yang sama di keduanya, karena nilainya adalah kata milik engine sendiri.

## Konsekuensi

- **Safe Mode adalah guardrail, bukan pengganti privilege.** Ia tidak melihat fungsi yang menulis di
  dalam body-nya, `SELECT` yang memanggilnya, atau `WITH` yang menulis di klausa yang tidak dibaca.
  Ini dinyatakan di doc modul, bukan dijanjikan hilang.
- **`SHOW CREATE TABLE` tetap read-only** walau memuat kata `CREATE`: satu-satunya pengecualian yang
  sengaja dibuat, karena menolaknya adalah false positive yang jelas.
- **Koneksi lama membaca `full`.** `safeMode` di-decode dengan `decodeIfPresent` dan default-nya
  `full`, yaitu perilaku yang sudah dimilikinya; berkas `connections.json` sebelum setelan ini tetap
  terbaca.
- **Setelan timeout tetap milik run, bukan koneksi.** Keduanya soal keselamatan tetapi menjawab
  pertanyaan berbeda: timeout adalah berapa lama satu run boleh berjalan, Safe Mode adalah apa yang
  sebuah koneksi tolak sama sekali.
- **Tingkat keempat, floor, dan execution log menyusul di ADR-0026.** Tiga tingkat di sini tetap
  berlaku apa adanya; 0026 menambah `confirm`, floor yang memilih kondisi paling ketat, dan tabel
  `execution_log`.
