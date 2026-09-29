# 0021 — Kedalaman change tracking: match policy tanpa kunci, kosakata partial-commit, budget batch, dan kolom DEFAULT

- **Status:** Diterima
- **Tanggal:** 29 Sep 2026 (lanjutan Fase 5.3)
- **Konteks instruksi:** `docs/architecture/tablepro-source-study.md` §2;
  `docs/decisions/0020-apply-changes-in-engine.md`;
  `docs/architecture/tablepro-adoption-plan.md` §7 (5.3)

## Konteks

ADR-0020 membuat rencana `DELETE`/`UPDATE`/`INSERT` yang ditinjau berjalan di dalam engine, dalam
satu transaksi, dengan jumlah baris tersentuh tiap statement diperiksa terhadap apa yang diharapkan
rencana. Studi sumber §2 menyebut empat hal yang TablePro lakukan dan rencana itu belum, dan ADR ini
adalah keempatnya:

1. **Match policy tanpa kunci.** Predikatnya sekarang mencocokkan **setiap** kolom asli. Sebuah
   `UPDATE` tanpa kunci pada tabel ber-`FLOAT` atau `JSON` karena itu tidak cocok dengan apa pun dan
   rencananya rollback — bug `FLOAT`/`JSON` MySQL yang dicatat studi. TablePro menjawabnya dengan
   `RowMatchPolicy`: kolom yang tidak bisa dibandingkan dengan aman dikecualikan, dan kolom yang teks
   grid-nya adalah render server dibandingkan lewat render itu (`CONCAT(col)` di MySQL).
2. **Kosakata partial-commit.** Rencana yang gagal sekarang melaporkan ketidakcocokan lalu rollback.
   `DataWritePartialCommitError` milik studi membedakan apa yang sudah ada di tabel (`written`) dari
   yang belum, karena mengulang yang pertama secara buta berarti menulis dua kali.
3. **Budget batch.** `SQLWriteBatchBudget` milik studi membatasi batch oleh baris, byte, dan plafon
   bind-parameter engine sekaligus, dan menutup batch **sebelum** baris yang akan melewati batas.
4. **Kolom hasil server dan `DEFAULT`.** Studi mencatat generator yang mengembalikan apa-apa untuk
   baris yang seluruh kolomnya dihitung server membuat barisnya menghilang sementara sisa batch
   commit dan melaporkan sukses.

Dua batas di pohon ini mengikat pekerjaannya, dan dinyatakan alih-alih diakali:

- App **tidak membawa metadata primary key**, jadi match policy tidak bisa bersandar pada kunci dan
  harus mengatakan apa yang tidak bisa dibandingkannya. ADR-0020 sudah menerima ini.
- App juga **tidak membawa metadata kolom generated atau default**, jadi "hasil server" tidak bisa
  dibaca dari skema di sini. Yang bisa diketahuinya adalah apa yang **diberikan** pengguna: sel
  sisipan yang tidak pernah diisi pengguna adalah sinyal bahwa server yang harus mengisinya.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Kolom yang tak bisa dibandingkan | **Kecualikan dan sebut namanya** | Predikat cocok sebisanya; rencana mengatakan apa yang dibuangnya | Predikatnya lebih lemah, jadi bisa mencocokkan lebih dari satu baris |
| | Pertahankan setiap kolom | Satu aturan, tanpa pengecualian | Kolom `FLOAT`/`JSON`/`BLOB` membuat seluruh `UPDATE` tidak cocok dengan apa pun |
| Kolom dengan teks tak stabil | **Bandingkan render server** | Teks grid itulah yang dibandingkan, jadi cocok | Satu ejaan per driver lagi untuk dijaga |
| | Bandingkan literal yang diambil langsung | Sederhana | Gagal untuk `FLOAT`/`JSON`, dan itulah bugnya |
| Tak ada kolom yang bisa dibandingkan | **Tolak statementnya dan katakan** | Tidak pernah melebar menjadi `DELETE FROM t` | Perubahan yang diantre pengguna tidak ditulis (terlihat) |
| | Buang `WHERE`-nya | Tulisannya terjadi | Ia menulis setiap baris; pemeriksaan jumlah menangkapnya hanya di dalam transaksi |
| Batching | **Baris + byte + parameter, tutup sebelum melewati** | Satu batas tidak menghabiskan batas lain; menahan nilai besar dari meledakkan batch | Tiga angka untuk dipertanggungjawabkan |
| | Baris saja | Lebih sederhana | Baris dengan `TEXT`/`JSON` besar mengirim permintaan tanpa batas |
| Parameter binding | **Tidak sekarang** | Bentuk rencananya siap untuk itu | Sumbunya dihitung tapi tidak dipakai |
| | Kerjakan sekarang | Satu pass | Protokol HTTP Trino tidak punya parameter; itu keputusan trait driver, bukan keputusan rencana |

## Keputusan

**Match policy-nya eksplisit dan disebut namanya; rencana yang gagal membawa disposisi yang menyebut
engine-nya; baris yang ditambahkan di-batch pada tiga sumbu sekaligus; dan baris yang diisi server
seluruhnya tetap mendapat statement alih-alih menghilang.**

1. **Match policy tanpa kunci.** `app/Sources/TrinoExporter/Models/MatchPolicy.swift` memberi setiap
   kolom satu dari tiga aturan, dari nama tipe hasil itu sendiri:

   - `match` — bandingkan langsung (`col = literal`, `col IS NULL`).
   - `serverText` — bandingkan render server: `CONCAT(col)` di MySQL (ejaan studi), `col::text` di
     PostgreSQL, `CAST(col AS varchar)` di Trino. `FLOAT`/`REAL`/`DOUBLE` dan `JSON`/`JSONB`.
   - `excluded(reason)` — nilai biner (`bytea`, `blob`, …), spasial (`geometry`, …) dan bersarang
     (`array`, `map`, `row`, …), yang tidak bisa dibandingkan sebagai teks.

   Aturannya ditegakkan di `UpdateStatements.match`, satu-satunya tempat `WHERE` sebuah baris yang
   diambil dibangun. Kolom yang dikecualikan **disebut namanya**, baik di komentar SQL di ekor
   statement yang ditinjau maupun di `WritePlan.warnings`. Baris yang seluruh kolomnya dikecualikan
   menghasilkan **tanpa predikat dan tanpa statement** — sebagai entri `WritePlan.warnings` — karena
   alternatifnya adalah `DELETE FROM t` / `UPDATE t SET …` telanjang yang mencocokkan seluruh tabel.
   Pemeriksaan jumlah tanpa kunci milik engine (`actual != expected`) tetap menjadi jaring pengaman
   untuk predikat yang dilemahkan satu pengecualian: baris yang cocok dengan duplikat gagal dan
   rollback.

2. **Kosakata partial-commit.** `crates/qh-ffi/src/apply.rs` bertambah `Disposition` dengan dua
   nilai, dipakai setiap kali sebuah statement gagal, jumlah tidak cocok, `COMMIT` gagal, atau run
   dibatalkan:

   - `written` — tidak ada transaksi yang terbuka, atau transaksinya tidak bisa dibatalkan dengan
     bersih. Barisnya ada (atau mungkin ada) di tabel.
   - `pendingInSessionTransaction` — rencananya berjalan di dalam transaksi dan tidak commit; engine
     me-rollback-nya, jadi tidak satu pun ada di tabel.

   Pesannya menyebut **engine**-nya (`postgres`/`trino`/`mysql`), berapa dari berapa statement yang
   sudah berjalan, disposisinya, dan langkah berikutnya. `written` mengirim pengguna ke tabel dan
   secara eksplisit **bukan** ke percobaan ulang; `pendingInSessionTransaction` mengatakan rencananya
   milik pengguna untuk diperbaiki dan dijalankan lagi. Saat sukses, `done` kini melaporkan
   `disposition: written`, karena saat itu commit-nya sudah berjalan.

3. **Budget batch.** `app/Sources/TrinoExporter/Models/WriteBatchBudget.swift` memegang `maxRows`,
   `maxBytes` dan `maxParameters`, dan `InsertStatements.build` menutup batch **sebelum** baris yang
   akan melewati salah satunya. Angkanya, per driver:

   | Sumbu | Nilai | Alasan |
   |---|---|---|
   | baris | 1.000 | engine memang sudah membaca dalam batch 1.000 baris |
   | byte | 1 MiB (1.048.576) | membatasi satu permintaan terlepas dari jumlah barisnya |
   | parameter | 65.535 | plafon statement PostgreSQL dan MySQL sendiri; tanpa batas di Trino, yang protokol HTTP-nya tidak punya |

   Baris hanya berbagi satu `INSERT` multi-baris bila menulis himpunan kolom yang sama —
   pengelompokan per himpunan kolom milik studi; batch itu per himpunan kolom, jadi satu daftar kolom
   tidak pernah diminta menjelaskan dua bentuk.

4. **Kolom hasil server dan `DEFAULT`.** `InsertStatements.build`:
   - Kolom yang **tidak diberikan** pengguna dibiarkan keluar dari `INSERT`, jadi server mengambil
     default-nya atau menghasilkannya. Menulis `NULL` di atasnya entah gagal pada kunci `NOT NULL`
     atau menimpa default diam-diam.
   - Sel yang seluruh teksnya `DEFAULT` adalah **sentinel**, dirender sebagai kata kunci dan tidak
     pernah dikutip, baik di `INSERT` maupun `UPDATE`.
   - Baris yang seluruh kolomnya milik server tetap menghasilkan statement: `INSERT INTO t DEFAULT
     VALUES` (PostgreSQL) atau `INSERT INTO t () VALUES ()` (MySQL). Trino tidak punya bentuk itu,
     dan di sana barisnya dilaporkan di `WritePlan.warnings` dan **tidak** dijatuhkan diam-diam —
     yang justru bug yang dicatat studi.

5. **Parameter binding sengaja tidak diimplementasikan.** Budget batch menghitung sumbu parameternya
   supaya bentuknya bertahan, tetapi rencananya masih menulis literal. Protokol HTTP Trino tidak
   punya bind parameter, jadi apakah akan mengikat sama sekali adalah keputusan trait driver
   (`Capabilities`), bukan perubahan pada rencana ini. Itu milik irisan lain.

## Alasan

1. **Predikat yang tidak cocok dengan apa pun terlihat seperti save yang berhasil.** Itu bug yang
   dicatat studi dan alasan match policy-nya eksplisit: edit yang hilang setelah reload lebih buruk
   daripada penolakan.
2. **Rencananya harus mengatakan apa yang tidak bisa dilakukannya.** Predikat yang lebih sempit bisa
   mencocokkan lebih banyak baris; baris yang seluruhnya dikecualikan bisa mencocokkan seluruh tabel.
   Menyebut kolom yang dibuang dan menolak predikat kosong menjaga "rencananya melakukan lebih
   sedikit dari yang kamu minta" tetap terlihat saat review, dengan pemeriksaan jumlah sebagai jaring
   pengaman saat runtime.
3. **`written` dan `pending` adalah tindakan yang berbeda bagi pengguna.** Percobaan ulang setelah
   `written` menggandakan barisnya; setelah `pending` aman. Satu pesan "gagal" tidak bisa
   membedakannya, jadi disposisinya bagian dari galatnya.
4. **Satu batas bukan budget.** Baris, byte dan parameter gagal secara independen; studi mengukur
   ketiganya, dan ongkos membawa yang ketiga hanyalah satu angka.
5. **Baris default yang menghilang adalah bug kehilangan data yang melaporkan sukses.** Spell per
   engine menjaga barisnya tetap ada di rencana, dan di tempat spell itu tidak ada, rencananya
   mengatakannya alih-alih commit seporsi lalu menyebutnya selesai.

## Konsekuensi

- **`WriteStatement` dan `WritePlan` bertambah.** `WriteStatement.unmatchedColumns` dan
  `WritePlan.warnings` membawa apa yang tidak bisa dicocokkan atau direncanakan; `WritePlan` mendapat
  initializer eksplisit supaya call site yang ada tetap terkompilasi. Payload `CHANGES` tidak
  berubah: ini fakta saat review, bukan input engine.
- **`apply_changes` memancarkan `disposition: written` saat sukses**, yang dulu mengatakan `pending`
  bahkan setelah `COMMIT`. Belum ada yang membaca kuncinya, jadi ini koreksi, bukan perubahan
  kontrak.
- **Apa yang terverifikasi dan tidak.** Keempat perilakunya ditutup tes unit dan integrasi:
  `MatchPolicyTests`, `InsertStatementsTests`, `WriteBatchBudgetTests` (Swift), `apply_changes.rs`
  dan tes modul `apply` (Rust). Tidak ada tes server-hidup baru; mesin podman sudah hilang.
- **Batas yang dinyatakan alih-alih disembunyikan:**
  - Tidak ada metadata primary key, jadi setiap statement tanpa kunci dan tabel dengan dua baris
    identik tetap tidak bisa membedakannya. Tidak berubah dari ADR-0020.
  - Tidak ada metadata kolom generated atau default, jadi "hasil server" disimpulkan dari apa yang
    diberikan pengguna plus sentinel `DEFAULT`. Kolom dengan nilai yang diketik pengguna selalu
    ditulis.
  - String literal yang persis kata `DEFAULT` belum bisa ditulis lewat grid, karena sentinelnya
    mengklaimnya. Jalan keluar di UI (atau sakelar null tersendiri, yang sekaligus membuat `NULL`
    eksplisit bisa dinyatakan) belum dibangun.
  - Di Trino, kolom yang dibiarkan menjadi `NULL` alih-alih default (Trino mengisi kolom yang tidak
    disebut dengan `NULL` dan tidak punya kata kunci `DEFAULT` di `INSERT`), jadi spell all-default
    tidak tersedia di sana dan dilaporkan.
  - Lembar review (`Views/ResultGrid.swift`) masih hanya merender `plan.sql`; catatan match-nya ada
    di SQL-nya, dan `WritePlan.warnings` dicatat `AppModel.applyChanges`. Merender warning-nya di
    lembar itu ditunda karena berkas itu dimiliki irisan lain di sesi ini.
