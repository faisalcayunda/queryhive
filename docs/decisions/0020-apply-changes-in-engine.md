# 0020 — Rencana perubahan dijalankan di engine, dengan verifikasi jumlah baris

- **Status:** Diterima
- **Tanggal:** 29 Sep 2026 (Fase 5.3)
- **Konteks instruksi:** `docs/architecture/tablepro-adoption-plan.md` §7 (5.3);
  `docs/architecture/tablepro-source-study.md` §2

## Konteks

Sampai Fase 5, jalur edit sel QueryHive berhenti di tinjauan: `CellEdits` menampung nilai sel,
`UpdateStatements` menghasilkan `UPDATE`, dan `ChangeReview` menampilkan SQL-nya — tetapi tidak ada
yang menjalankannya, karena tidak ada perintah tulis dan tidak ada verifikasi jumlah baris. Studi §2
menyebut ini bagian TablePro yang paling matang, dan yang QueryHive belum punya justru tiga hal:
**satu nilai yang memiliki invariant lintas-perubahan**, **rencana yang dibaca baik oleh tinjauan
maupun oleh save**, dan **verifikasi jumlah baris di dalam transaksi**.

Dua batas di pohon ini mengikat. App tidak melihat `affected_rows` server — ia pemanggil FFI yang
membaca event — jadi verifikasi tidak bisa hidup di app. Dan app tidak punya metadata primary key,
jadi predikat baris adalah **seluruh kolom pada nilai saat diambil** (aturan yang sudah ditulis di
`UpdateStatements`).

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Tempat menjalankan | **Satu perintah engine `apply_changes`** | CLI dan MCP dapat memakai rencana yang sama; engine punya cursor dan transaksi | Permukaan FFI bertambah |
| | Dari app, statement satu per satu | Tidak menyentuh engine | Tidak ada transaksi, tidak ada verifikasi; dua jalur data |
| Verifikasi | **Dalam transaksi, gagal → rollback** | Statement yang mencocokkan lebih dari satu baris bisa dibatalkan | Butuh driver yang melaporkan `affected_rows` |
| | Tanpa verifikasi | Sederhana | Lubang "WHERE cocok nol baris tapi lapor sukses" tetap terbuka |
| Aturan ber-PK | **`actual > expected` = gagal** | MySQL melaporkan 0 untuk update yang menulis nilai yang sudah ada | — |
| Aturan tanpa PK | **`actual != expected` = gagal** | Baris hilang dan baris kembar dua-duanya tertangkap | — |
| Urutan | **Rencananya, tidak diurut ulang engine** | DELETE yang membebaskan nilai unik jalan sebelum INSERT yang mengambilnya | Pemanggil harus benar |

## Keputusan

**Perintah `apply_changes` menerima `CHANGES` berupa array JSON `{sql, expected, keyed}` dan
menjalankannya berurutan di dalam satu transaksi, memeriksa `affected_rows` tiap statement terhadap
`expected`, dan me-rollback seluruh rencana bila satu gagal. App membangun rencananya sekali sebagai
`WritePlan`, menampilkan `sql`-nya, dan mengirim `payload`-nya — string yang sama.**

Rincian yang mengikat:

1. **Satu `WritePlan` dibaca dua kali.** Lembar tinjauan membaca `plan.sql`; `apply_changes` menerima
   `plan.payload`. Keduanya berasal dari nilai yang sama, jadi tidak ada jalur pembangkitan kedua
   yang bisa menampilkan satu SQL dan mengirim SQL lain.
2. **Urutannya deletes → updates → inserts**, dan engine tidak mengurutkan ulang. DELETE yang
   membebaskan nilai unik harus jalan sebelum INSERT yang mengambilnya; inilah alasan studi mencatat
   cap urutan pada tiap perubahan.
3. **Keyless adalah jawabannya sekarang.** App tidak punya metadata PK, jadi `keyed` selalu `false`
   dan aturannya dua arah (`actual != expected`). Kolom `keyed` ada supaya pemanggil yang kelak
   memang punya PK bisa memakai aturan satu arah tanpa mengubah engine.
4. **Guard Safe Mode sebelum connect**, untuk setiap statement di rencana; koneksi read-only menolak
   seluruh rencana tanpa membukanya.
5. **Driver tanpa transaksi tetap jalan dan melaporkan `disposition: "written"`.** Rencana yang gagal
   di tengah di sana menjalankan statement satu per satu, dan itu dinyatakan.

## Alasan

1. **Verifikasi hanya bisa di tempat yang melihat count.** App tidak pernah menerima `affected_rows`;
   menaruh aturan di app berarti aturan itu tidak pernah berjalan.
2. **Rollback adalah satu-satunya hal yang bisa membatalkan baris kedua yang sudah tertulis.** Count
   datang setelah statement selesai, jadi menemukan bahwa predikat mencocokkan dua baris hanya
   berguna kalau transaksinya masih terbuka.
3. **`actual > expected` untuk ber-PK adalah pelajaran yang sudah dibayar.** MySQL melaporkan nol
   baris untuk `UPDATE` yang menulis nilai yang sudah ada, dan `!=` akan melaporkan itu sebagai
   kegagalan padahal save-nya berhasil.
4. **Satu objek rencana menutup kesenjangan "dilihat vs dijalankan".** Kriteria fase memintanya
   secara eksplisit.

## Konsekuensi

- **PostgreSQL kini melaporkan `affected_rows`.** Driver-nya dulu mengabaikan tag
  `CommandComplete`; sekarang `PostgresCursor` menguras stream untuk statement tanpa result set dan
  menyimpannya. Efek sampingnya: `to_table` di PostgreSQL melaporkan jumlah baris sungguhan alih-alih
  `-1` seperti di Trino.
- **Insert dan delete baris masuk ke antrean yang sama.** `CellEdits` bertambah `inserted` dan
  `deletedRows` dengan cap urutan; menghapus baris membuang edit selnya, karena satu baris tidak bisa
  sekaligus dihapus dan di-update.
- **Yang belum.** Tombol "Add row" dan "Delete row" di grid belum dipasang, jadi UI-nya belum punya
  gestur untuk mengisi antrean insert/delete; model, rencana, engine, dan tesnya ada. Ini dinyatakan
  di §7 rencana, bukan diklaim selesai.
