# 0019 — Impor menolak baris rusak dengan rollback sebagai default

- **Status:** Diterima
- **Tanggal:** 29 Sep 2026 (Fase 5.1)
- **Konteks instruksi:** `docs/architecture/tablepro-adoption-plan.md` §7 (5.1), §10;
  `docs/architecture/tablepro-source-study.md` §3

## Konteks

Rencana §5.1 meminta `import_data`: streaming seperti ekspor, dengan column mapping dan transaction
safety, karena "impor yang setengah jalan lebih buruk daripada impor yang gagal". Studi §3
menunjukkan TablePro memecah impor menjadi dua keluarga (baris vs statement) dan punya tiga mode
kesalahan — `stopAndRollback`, `stopAndCommit`, `skipAndContinue` — dengan catatan bahwa
`skipAndContinue` **dimatikan transaksinya**, karena rollback tidak bisa "melewati" baris yang gagal.

Yang harus diputuskan untuk QueryHive bukan "pakai mode apa" tetapi **apa defaultnya dan apa yang
terjadi pada baris buruk di tengah file**, karena file impor adalah jalur masuk data yang tidak
tepercaya (rencana §10). Dua batas di pohon ini mengikat: engine punya tiga pemanggil (app, CLI,
MCP) dan Safe Mode ditegakkan di engine (ADR-0017), dan tidak semua driver bisa membuka transaksi
(`Capabilities::transactions`; Trino `false`).

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Default | **`stop` dengan rollback** | Tidak ada yang separuh mendarat | Impor besar yang punya satu baris buruk di ujung mengulang dari nol |
| | `commit` | Kerja yang sudah benar tetap tersimpan | Tabel berisi subset diam-diam kalau tidak dibaca |
| | `skip` | Paling toleran | Tidak transaksional, dan itu harus dikatakan |
| Batas transaksi | **`skip` tanpa transaksi** | Sejalan dengan TablePro dan dengan alasan yang benar | Perilaku berbeda antar mode |
| Baris tanpa nilai | **Ditolak, bukan dihitung sukses** | Angka "N baris" berarti N baris | Satu baris bisa hilang kalau mode `skip` |
| Driver tanpa transaksi | **Tetap jalan, laporkan `written`** | Tidak menolak Trino hanya karena tidak bisa rollback | "stop" tidak berarti "tidak ada yang tertulis" di sana |

## Keputusan

**Tiga mode `ON_ERROR`: `stop` (default, `BEGIN`/`COMMIT`/`ROLLBACK` dan tidak ada yang mendarat
kalau ada baris buruk), `commit` (pertahankan prefix dan laporkan di baris mana berhentinya), dan
`skip` (lewati barisnya, **tanpa transaksi**). Baris yang tidak punya satu pun nilai yang mencapai
kolom termapping ditolak, bukan dihitung sebagai ter-insert.**

Rincian yang mengikat:

1. **Default adalah `stop`.** Rencana menyebut impor setengah jalan lebih buruk daripada impor yang
   gagal, jadi default harus yang tidak meninggalkan setengah. `commit` dan `skip` harus diminta
   dengan menyebut namanya.
2. **`skip` tidak memakai transaksi**, persis alasan TablePro: rollback tidak bisa melewati baris
   yang gagal, jadi transaksi yang tetap dibuka hanya akan di-rollback pada baris pertama yang gagal
   dan membuang baris baik sesudahnya. Tanpa transaksi, `done` melaporkan `transaction: false`.
3. **Baris kosong ditolak.** Sebuah baris yang semua sel termapping-nya kosong tidak dihitung
   sukses; ia masuk `rejected` dan, di mode `stop`/`commit`, menghentikan impor. Ini yang mencegah
   "import selesai" melebih-lebihkan apa yang mendarat.
4. **Driver tanpa transaksi tidak berpura-pura.** Trino menjawab `transactions: false`, jadi mode
   `stop` di sana melaporkan `disposition: "written"` dan berapa baris yang sudah tertulis, bukan
   mengklaim rollback yang tidak terjadi. Kosakatanya milik studi: `pending` berarti ditahan
   transaksi, `written` berarti sudah di tabel.
5. **Safe Mode diperiksa sebelum connect.** Impor menulis, jadi mode yang menolak statement DML
   menolak seluruh impor sebelum koneksi dibuka (ADR-0017). Setiap statement yang benar-benar
   dikirim juga lewat `qh_sql::check` sekali lagi.
6. **Nomor baris ada di setiap pesan.** Mode `skip` mengirim satu baris per statement, jadi pesannya
   menyebut baris persisnya. Mode `stop`/`commit` mengirim satu `INSERT` multi-baris per batch (200
   baris), dan server tidak memberi tahu baris mana di dalam batch itu yang gagal — pesannya menyebut
   baris pertama batch plus pesan server, dan itu dinyatakan, bukan diakali.

## Alasan

1. **Default harus aman.** Pengguna yang tidak menyetel apa pun mendapat jaminan terkuat yang bisa
   diberikan driver: satu transaksi dan rollback. Mengubahnya berarti kerja yang tidak diminta.
2. **Transaksi di `skip` adalah janji palsu.** Membukanya lalu rollback pada baris gagal pertama
   membuang baris baik sesudahnya, yaitu kebalikan dari "lewati dan lanjutkan".
3. **Baris kosong yang dihitung ter-insert menggelembungkan laporan.** Ini pelajaran yang TablePro
   tulis di doc string-nya, dan alasannya berlaku sama di sini.
4. **Engine yang tahu driver mana yang bisa transaksi.** App tidak punya daftar itu dan tidak
   seharusnya punya; `Capabilities` sudah menjadi tempatnya.

## Konsekuensi

- **`import_data` adalah perintah FFI kedua puluh satu.** Keempat daftar (invariant #11) disentuh,
   `EngineCommand` dan `RustEngine.commands` termasuk; `./app/build-ffi.sh` dijalankan.
- **Column mapping ada di engine, bukan di UI.** `COLUMNS` adalah JSON `[{source,target,include}]`;
   tanpa itu, pemetaan diturunkan dari baris header. Lembar mapping di app **belum dibangun**, dan
   itu dinyatakan di §7 rencana.
- **XLSX tidak streaming dan tidak mengaku streaming.** `done` membawa `streams: false` untuk XLSX,
   karena formatnya menuntut workbook dibaca utuh; CSV membawa `true`.
- **Yang belum.** Membuat tabel target dari tipe yang disimpulkan (TablePro `ImportTypeMapper`) tidak
   dibangun: tabel target harus sudah ada. Impor statement (`import_sql` dari file `.sql`) juga belum;
   keluarga baris yang mendarat, keluarga statement tidak.
