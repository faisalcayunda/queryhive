# 0026 — Tingkat konfirmasi, floor, dan execution log untuk Safe Mode

- **Status:** Diterima
- **Tanggal:** 29 Sep 2026 (Gelombang 2, penutupan gap TablePro)
- **Konteks instruksi:** `docs/architecture/tablepro-source-study.md` §5;
  `docs/architecture/tablepro-adoption-plan.md` §13 (Gelombang 2)

## Konteks

ADR-0017 menetapkan tiga tingkat `full` / `no_ddl` / `read_only`, ditegakkan di engine oleh
`qh_sql::classify`. Studi §5 menemukan tiga pola TablePro yang belum diambil: **tingkat
konfirmasi** (enam tingkat TablePro menentukan tiga hal berbeda: `blocksAllWrites`,
`requiresConfirmation`, `requiresAuthentication`), **floor** yang menaikkan tingkat minimum dari
beberapa kondisi independen dan memilih yang **paling ketat**, dan **execution log** yang mencatat
setiap keputusan izinkan/tolak dengan statement sebagai SHA-256 dan tiap record dirantai ke hash
record sebelumnya.

Tiga batas di pohon ini mengikat keputusan-keputusannya:

1. Engine punya tiga pemanggil — CLI, server MCP, dan app — dan hanya app yang punya jendela.
   Konfirmasi yang hidup di Swift adalah konfirmasi yang dua pemanggil lain tidak pernah lihat.
2. `apply.rs`, `import.rs`, dan `mcp.rs` memanggil `safe_mode`/`guard` yang sama, dan tidak boleh
   disentuh. Apa pun yang baru harus masuk lewat fungsi yang mereka sudah panggil, bukan lewat
   signature baru di setiap call site.
3. `crates/qh-driver*/` tidak disentuh, jadi `Capabilities` tidak bisa bertambah bit read-only.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Bentuk konfirmasi | **Tingkat `confirm` + setting `SAFE_MODE_CONFIRMED`** | CLI dan MCP melihat tingkat yang sama; app tidak perlu jendela di jalur ini | App belum bisa memberi konfirmasi, dan itu dinyatakan |
| | Boolean "izin menulis" | Tidak ada tingkat baru | Tidak bisa membedakan "tolak" dari "tanya" |
| | Touch ID di engine | — | Engine tidak punya jendela; yang bisa diungkapkan engine hanya boolean yang sudah diputuskan pemanggil |
| Urutan tingkat | **Rantai strictness `full < no_ddl < confirm < read_only`** | Floor bisa memilih satu tingkat tanpa pernah melemahkan pilihan pengguna | Satu tingkat lebih banyak untuk dijelaskan |
| Memilih floor | **`max` by strictness, tie by sumber** | Benar berapa pun banyaknya kondisi | Sedikit kode lebih dari `if` berurutan |
| | Rantai first-match | Sederhana | Hanya benar selama urutannya kebetulan tepat |
| Execution log | **Tabel `execution_log` (migrasi 0007), statement sebagai SHA-256, rantai `prev_hash`/`chain_hash`** | Bisa diverifikasi ulang; tidak menyimpan SQL atau kredensial | Bukan anti-rusak, hanya bukti perubahan |
| Penulis log | **Satu sink per proses, dipasang binary** | `apply.rs`/`import.rs`/`mcp.rs` tidak perlu diubah; tes tidak menulis DB pengguna | Global proses, dan satu pengecualian aturan "SQLite jangan di worker tokio" |

## Keputusan

**Safe Mode bertambah satu tingkat, `confirm`: sebuah write berjalan hanya bila pemanggil mengirim
`SAFE_MODE_CONFIRMED=1`; DDL dan statement yang tak terklasifikasi tetap ditolak. Tingkat efektif
sebuah run adalah hasil `max` by strictness atas kondisi-kondisi floor, dan setiap keputusan
(izinkan, konfirmasi, tolak, butuh-konfirmasi) dicatat ke `execution_log` sebagai rantai hash.**

Rincian yang mengikat:

1. **Empat tingkat adalah rantai yang ketat.** `full` (0) < `no_ddl` (1) < `confirm` (2) <
   `read_only` (3). Tiap langkah menolak lebih banyak statement **yang belum dikonfirmasi**:
   `no_ddl` membiarkan `DELETE` jalan; `confirm` menolak `DELETE` itu sampai dikonfirmasi dan tetap
   menolak DDL; `read_only` menolaknya langsung. Karena itu `confirm` berada di atas `no_ddl`, dan
   floor tidak pernah bisa menurunkan pilihan pengguna.
2. **Konfirmasi adalah boolean, bukan biometrik.** `SAFE_MODE_CONFIRMED=1` adalah seluruh konfirmasi
   sejauh yang bisa dilihat engine: boolean yang hanya bisa di-set pemanggil yang sudah bertanya
   ke penggunanya. Dialog, Touch ID, dan fallback password adalah urusan app, dan **tidak** dimodelkan
   di engine — CLI dan MCP tidak punya jendela untuk memunculkannya. App belum mengirim flag ini di
   jalur run-nya; sampai itu ada, memilih `confirm` membuat app menolak semua write, dan itu
   dinyatakan di teks picker-nya, bukan disembunyikan.
3. **`check` yang lama tidak berubah artinya.** `qh_sql::check(mode, sql)` tetap "tanpa konfirmasi",
   jadi `apply_changes` dan `import_data` — dua jalur massal — menolak rencana di koneksi `confirm`.
   Satu konfirmasi tidak mencakup sebuah plan; itu batas yang disengaja, bukan kelalaian.
   Perintah single-statement (`preview`, `explain`, `export`, `count`, `to_table`) memakai
   `guard_confirmed` dan membaca flag-nya.
4. **Floor itu tiga kondisi di sini.** `SAFE_MODE` (pilihan pengguna), `DB_READ_ONLY` (koneksi
   ditandai read-only; kolom `is_read_only` sudah ada di skema lokal), dan `SAFE_MODE_FLOOR` (minimum
   yang dipin dari luar koneksi — pemanggil embedding, gate klien eksternal, kebijakan terkelola).
   Yang terakhir adalah tempat sebuah macOS configuration profile akan menulis bila proyek ini punya
   satu; tidak ada di pohon ini, dan itu dinyatakan.
5. **Sumber `Driver` ada tapi belum ada yang menaikkannya.** `Capabilities` tidak punya bit
   read-only dan crate driver di luar cakupan perubahan ini, jadi tidak ada driver yang bisa
   dinyatakan read-only oleh engine. Variannya tetap ada supaya bit itu, bila kelak ditambahkan,
   hanya perlu memanggil `raise`; fakta bahwa tidak ada driver yang memicunya ditulis di sini alih-alih
   dipalsukan dengan kondisi lain.
6. **Floor tidak pernah ditulis kembali.** Ia nilai untuk satu run, dihitung dari setting, dan tidak
   ada jalur tulis dari engine ke `SAFE_MODE` koneksi. Kondisi hilang = tingkat pengguna kembali.
7. **Statement tidak pernah masuk log.** Yang disimpan hanya `statement_hash` (SHA-256 heks) dan
   `reason` (kalimat tetap classifier). Tidak ada host, user, password, atau `secret_ref`.
8. **Rantai itu tamper-evident, bukan tamper-proof.** `chain_hash` = SHA-256 atas JSON kanonik
   `[id, at, safe_mode, decision, statement_kind, statement_index, statement_hash, reason,
   prev_hash]`; `prev_hash` baris pertama adalah `genesis`. Siapa pun yang memegang berkas bisa
   menulis ulang seluruh rantai, dan itu dinyatakan di doc modulnya.
9. **Log ditulis oleh sink proses, dipasang binary.** `qh-ffi` punya `execution_log` dengan `install`,
   `record`, dan `with_storage`; `queryhive-engine` memasangnya di `main`. Library dan tes tidak
   memasang apa pun, sehingga `cargo test` tidak pernah membuka database pengguna.

## Alasan

1. **Tingkat konfirmasi harus di engine karena alasan yang sama dengan tiga tingkat sebelumnya.**
   Satu aturan di `qh-sql`/`qh-ffi` berlaku untuk CLI, MCP, dan app; aturan yang hidup di Swift tidak.
2. **Rantai strictness adalah satu-satunya bentuk yang tidak bergantung pada urutan penambahan
   kondisi.** Rantai first-match hanya benar selama urutannya kebetulan tepat; studi §5 mencatat
   bahwa kondisi keempat TablePro justru yang membuktikannya salah.
3. **SHA-256 dan rantai adalah dua properti berbeda.** Hash menjawab "statement apa yang dinilai"
   tanpa menyimpan SQL; rantai menjawab "apakah ada baris yang disisipkan, dihapus, atau diedit".
   Yang kedua bukan tanda tangan, dan tidak diklaim sebagai.
4. **Memasang sink di binary, bukan default di library, adalah yang membuat tes aman.** Alternatif
   "log aktif kecuali ada flag" berarti `cargo test` menulis fixture ke berkas Application Support
   pengguna. Sink yang tidak dipasang tidak menulis apa-apa.
5. **`guard` tidak bisa membaca setting, dan itu bukan alasan untuk tidak mencatat.** Sink proses
   memberi `guard` akses log tanpa mengubah signature-nya, jadi `apply.rs`, `import.rs`, dan `mcp.rs`
   tetap utuh dan tetap tercatat.

## Konsekuensi

- **MCP selalu `read_only`, jadi `confirm` tidak pernah terpakai di sana.** Ini bukan celah: MCP
  menambahkan `SAFE_MODE=read_only` pada setiap panggilan (ADR-0015, ADR-0017), dan tingkat
  `confirm` tidak menurunkannya.
- **Server MCP tidak menulis log.** `crates/qh-ffi/src/mcp.rs` di luar cakupan perubahan ini dan
  binary MCP belum memasang sink-nya. Keputusan MCP tetap ditegakkan (read-only), tetapi barisnya
  tidak tercatat. Ini dinyatakan sebagai gap, bukan diklaim selesai.
- **Satu pengecualian aturan blocking.** `guard` berjalan di jalur async dan menulis satu `INSERT`
  SQLite langsung, sementara aturan crate adalah "jangan pakai `rusqlite` dari worker tokio".
  Pengecualiannya disengaja: alternatifnya (`spawn`) membuat keputusan bisa sampai ke pemanggil
  sebelum sampai ke log, dan rantai yang berlomba adalah rantai yang tidak bisa diverifikasi.
- **Log tumbuh tanpa batas.** Setiap keputusan `guard` — termasuk `SELECT` di mode `full` — menjadi
  satu baris. Indeks `idx_execution_log_at` melayani pembacaan terbaru; pemangkasan (retensi) belum
  ada dan dicatat sebagai pekerjaan lanjutan.
- **Tabel tanpa kolom sync.** `execution_log` append-only dan tidak disinkronkan, jadi ia tidak
  membawa `updated_at`/`deleted_at`/`version` seperti tabel lain; alasannya ditulis di migrasinya.
- **`import_data` yang ditolak di awal tidak tercatat.** Pemeriksaan `safe.refusal(Dml)` di
  `import.rs` mendahului `guard`, dan berkas itu di luar cakupan; kegagalan impor massal karena Safe
  Mode tidak menghasilkan baris log. Jalur guard di dalam importer tetap tercatat.
- **`ConnectionSafeMode` bertambah satu case.** Sebuah `connections.json` dengan `"confirm"` harus
  tetap terbaca; tanpa case itu, `decodeIfPresent` gagal dan seluruh berkas ditolak, jadi case-nya
  ditambahkan demi kompatibilitas ke depan sekaligus untuk picker-nya.
