# 0016 — Batas waktu statement sebagai bagian dari trait driver

- **Status:** Diterima
- **Tanggal:** 29 Sep 2026 (Fase 3)
- **Konteks instruksi:** `docs/architecture/tablepro-adoption-plan.md` §5 (3.1), §0

## Konteks

QueryHive tidak punya batas waktu statement sama sekali sebelum Fase 3. Satu-satunya timeout di
seluruh `crates/` adalah `connect_timeout` di driver Trino, dan `ExecuteOptions` tidak punya medan
untuk satu pun. Akibatnya `preview` pada tabel besar tidak punya langit kedua selain tombol Stop —
dan Stop sendiri hanya bergantung pada driver yang menghormati `cancel`, yang berarti berhenti
membaca, bukan menghentikan pekerjaan di server.

Rencana §0 sudah menyatakan bahwa keputusan ini akan mengubah kontrak dan karena itu butuh ADR:
batas waktu statement masuk ke trait driver, bukan hanya ke satu driver atau ke UI.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Siapa yang menegakkan | **Mekanisme server tiap driver** | Statement benar-benar dibatalkan dan sumber dayanya dilepas; batasnya berlaku walau proses ini mati | Tiga mekanisme berbeda, dan MySQL tidak mencakup write |
| | Timer lokal di engine | Satu implementasi, seragam | Hanya berhenti *membaca*; query tetap jalan dan memegang slot gudang — persis masalah yang mau ditutup |
| Bentuk kontrak | **Medan di `ExecuteOptions` + `Capabilities`** | Satu tempat; driver yang tidak bisa mengatakannya lewat capability, bukan diam-diam mengabaikan | Menyentuh trait dan ketiga driver sekaligus |
| | Setelan global yang dibaca driver sendiri | Tidak menyentuh trait | Driver tidak punya kontrak yang bisa dites; "driver mana yang menghormatinya" jadi tak terjawab |
| Cara melaporkan | **`EngineError::Timeout` bertipe, menyebut batasnya** | Pemanggil bisa membedakan "lambat" dari "salah" tanpa mengurai pesan; pesan selalu menyebut batas | Satu varian error baru di `qh-core` |
| | Pesan galat server apa adanya | Tidak ada tipe baru | `57014` juga dipakai user-cancel, dan "query terlalu lambat" tenggelam di teks |
| Default | **`0` = tanpa batas di engine; app memberi default 60.000 ms** | CLI dan korpus golden tidak berubah; app tetap terlindungi | Dua default yang berbeda, dan itu harus ditulis |

## Keputusan

**`ExecuteOptions` bertambah `statement_timeout: Option<Duration>`; `Capabilities` bertambah
`statement_timeout: bool`; tiap driver menerjemahkannya ke mekanisme servernya sendiri; batas yang
terlampaui datang sebagai `EngineError::Timeout` yang menyebut batasnya; setelan `STATEMENT_TIMEOUT_MS`
dengan `0` berarti tanpa batas dan nilai negatif ditolak dengan menyebut namanya.**

Rincian yang mengikat:

1. **PostgreSQL** memakai parameter sesi `statement_timeout`. Sinyalnya `SQLSTATE 57014`, tetapi kode
   itu juga dipakai `pg_cancel_backend`; karena itu pemeriksaannya membaca kode **dan** memastikan
   pesannya menyebut `statement timeout`, supaya cancel biasa tidak salah dibaca sebagai timeout.
2. **Trino** memakai properti sesi `query_max_run_time`, dikirim pada `X-Trino-Session` di POST
   statement — di situlah koordinator membaca properti sesi. Sinyalnya
   `EXCEEDED_TIME_LIMIT` (yang dihasilkan properti ini, diukur pada Trino 483) atau
   `QUERY_EXCEEDED_MAX_EXECUTION_TIME` (ejaan konfigurasi koordinator); keduanya diterima, karena
   mencocokkan hanya yang kedua akan melewatkan setiap timeout yang driver ini sendiri pasang.
3. **MySQL** memakai `max_execution_time`, dan ini **tidak mencakup write**: MySQL hanya menerapkannya
   pada statement read-only. Itu dinyatakan di doc modulnya, bukan disembunyikan, karena batas yang
   diam-diam tidak berlaku pada separuh statement lebih buruk daripada batas yang jujur soal
   cakupannya. Sinyalnya error `3024` (`ER_QUERY_TIMEOUT`).
4. **Batas yang tidak diketahui tetap jujur.** `EngineError::statement_timeout(None, …)` ada untuk
   kasus server membatalkan karena batas yang tidak dipasang engine ini, dan pesannya tetap menyebut
   "statement timeout" tanpa mengarang angka.
5. **Timeout bersifat permanen, bukan transient.** Mengulang statement yang sama di bawah batas yang
   sama hanya membuat pengguna menunggu timeout yang sama lagi, jadi `FailureKind`-nya `Permanent`
   dan retry tidak menyentuhnya.

## Alasan

1. **Mekanisme server adalah satu-satunya yang menutup masalahnya.** Timer lokal menghentikan
   pembacaan sambil meninggalkan query berjalan di koordinator; itu bukan batas, itu tombol Stop
   kedua. Rencana §5 menulis "jangan menyamarkan ketiadaan dukungan sebagai timeout lokal", dan
   bentuk kontraknya mengikuti kalimat itu.
2. **`Capabilities` supaya ketiadaan dukungan bisa dikatakan.** Sama seperti `cancel` dan
   `persistent_connection`: driver yang tidak bisa menegakkan batas menyatakannya, dan pemanggil
   bertanya dulu alih-alih berasumsi.
3. **Varian error sendiri karena pemanggil harus bisa bercabang.** "Query ini terlalu lambat" adalah
   batas yang bisa dinaikkan pengguna; "query ini salah" adalah jawaban server. Keduanya tidak boleh
   hanya bisa dibedakan dengan membaca prosa.
4. **Default `0` di engine karena CLI dan korpus golden adalah kontrak beku.** Fitur baru tidak boleh
   mengubah perilaku yang sudah direkam; app memberi default satu menit karena app adalah tempat
   pengguna benar-benar menjalankan query yang bisa lupa ditutup.

## Konsekuensi

- **`count` tidak mengarang perkiraan.** Batas yang sama berlaku, dan pada timeout ia melaporkan galat
  bertipe, bukan angka. Tidak ada driver di ruang kerja ini yang bisa memberi perkiraan murah untuk
  sebuah *statement* (`reltuples`, `TABLE_ROWS` dan `$partitions` menjawab untuk *tabel*), jadi
  perkiraan tidak tersedia dan itu dikatakan.
- **MySQL dan write.** Batas tidak berlaku pada `INSERT`/`UPDATE`/`DELETE` di MySQL. Ini batas
  pengetahuan yang dinyatakan, bukan bug yang menunggu ditemukan.
- **`Capabilities::statement_timeout` adalah janji yang bisa dites.** Ketiga driver menjawab `true`
  karena ketiganya memakai mekanisme server; menambah driver keempat berarti menjawab pertanyaan itu,
  bukan mewarisi jawaban.
