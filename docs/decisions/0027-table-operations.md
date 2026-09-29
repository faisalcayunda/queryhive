# 0027 — Operasi tabel (truncate/drop) lewat konfirmasi, dan menutup dua keputusan yang tak tercatat

- **Status:** Diterima
- **Tanggal:** 29 Sep 2026 (Gelombang 3, penutupan gap TablePro)
- **Konteks instruksi:** `docs/architecture/tablepro-adoption-plan.md` §13 (Gelombang 3);
  `docs/architecture/tablepro-feature-analysis.md` ("Table operations: drop, truncate,
  maintenance — Ambil truncate dan drop, lewat konfirmasi");
  `docs/architecture/tablepro-source-study.md` §5 (`confirm_destructive_operation`)

## Konteks

§13 mencatat tiga hal yang belum, setelah gelombang 2:

1. **Truncate/drop tabel lewat konfirmasi.** TablePro menaruh `DROP` dan `TRUNCATE` di balik
   `confirm_destructive_operation`, yang butuh persetujuan pengguna setiap kali. QueryHive belum
   punya perintahnya sama sekali.
2. **Keputusan MCP tidak tercatat.** `crates/qh-ffi/src/mcp.rs` memakai guard yang sama dengan CLI,
   tetapi binary `queryhive-mcp` belum memasang sink `execution_log`, jadi keputusan tool MCP tidak
   masuk log. ADR-0026 mencatatnya sebagai gap, bukan mengklaimnya selesai.
3. **Penolakan awal `import_data` tidak tercatat.** Pemeriksaan `safe.refusal(StatementKind::Dml)`
   di `crates/qh-ffi/src/import.rs` mendahului `guard` mana pun, sehingga satu-satunya penolakan yang
   dibuat sebelum import menyambung adalah satu-satunya keputusan engine tanpa baris.

Tiga batas di pohon ini mengikat keputusannya:

1. **Aturan ditegakkan di engine, bukan di UI** (ADR-0017). Engine punya tiga pemanggil — CLI, server
   MCP, dan app — dan hanya app yang punya jendela.
2. **ADR-0026 menetapkan `confirm` menolak DDL.** Rantai strictness `full < no_ddl < confirm <
   read_only` dibuat agar floor memilih yang paling ketat tanpa pernah melemahkan pilihan pengguna;
   classifier yang menolak DDL di `confirm` adalah bagian dari rantai itu.
3. **`crates/qh-driver*/` tidak disentuh**, dan permukaan FFI hidup di **empat daftar** yang harus
   sepakat (invariant #11): `COMMANDS`, `Command`, `EngineCommand`/`EVERY_COMMAND`, dan
   `RustEngine.commands`.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Bentuk perintah | **Satu `table_op` dengan `TABLE_OP=drop\|truncate`** | Mengikuti pola `to_table` (`WRITE_MODE`); satu perubahan di empat daftar; TablePro juga satu "table operations" | Satu kata perintah yang tidak langsung terbaca |
| | Dua perintah `drop` dan `truncate` | Terbaca apa adanya | Dua perubahan di empat daftar untuk satu irisan |
| Konfirmasi | **`table_op` sendiri yang meminta di `confirm`** | Inilah yang plan minta; `no_ddl`/`read_only` tetap menolak seperti DDL lain | Satu pengecualian terhadap classifier, dan satu biaya floor yang dinyatakan di bawah |
| | Ikut classifier (confirm menolak DDL) | Tidak ada pengecualian | Kedua perintah mustahil dijalankan di tingkat yang justru dibuat untuk bertanya; gap tidak tertutup |
| Sink MCP | **Dipasang binary `queryhive-mcp`** | Sejalan dengan ADR-0026 ("sink proses, dipasang binary"); library dan tes tetap tidak menulis database pengguna | Binary kedua yang harus memasangnya |
| Penolakan awal impor | **Helper `record_kind` yang memakai jalur log yang sama** | Tidak ada perubahan di `guard`; subjeknya di-hash seperti statement | Subjeknya berkas, bukan statement (tidak ada statement pada titik itu) |

## Keputusan

**Satu perintah baru `table_op` menjalankan `DROP TABLE` atau `TRUNCATE TABLE` pada satu target,
dijaga Safe Mode lewat jalur guard yang sama dengan setiap tulis lain; binary `queryhive-mcp`
memasang sink execution log; dan penolakan awal `import_data` dicatat lewat helper yang sama.**

Rincian yang mengikat:

1. **Bentuk.** `TABLE_OP=drop|truncate` (wajib, ditolak dengan menyebut namanya), target adalah
   `TARGET_CATALOG` / `TARGET_SCHEMA` / `TARGET_TABLE` seperti `to_table`, disaring lewat `slots`
   sehingga driver hanya diminta bagian yang dimilikinya. Satu run mengirim tepat satu statement.
2. **Safe Mode.** `read_only` dan `no_ddl` menolak **sebelum connect**, memakai kalimat classifier
   sendiri ("DDL is not allowed …"); `confirm` meminta `SAFE_MODE_CONFIRMED=1`; `full` menjalankan
   tanpa bertanya. `confirm` adalah satu-satunya perbedaan dari `guard_for`, dan ia hidup di
   `guard_destructive` — bukan di classifier.
3. **Jalur log yang sama.** `guard_destructive` memakai `record_decision`, fungsi yang sama yang
   dipakai `guard_confirmed`, sehingga kosakata `LogDecision` (allowed / confirmed / refused /
   needs_confirmation) dan `DecisionEntry` tidak punya salinan kedua.
4. **Sink MCP.** `src/bin/mcp.rs` memanggil `execution_log::install_from_settings` setelah token
   tervalidasi dan sebelum permintaan pertama. Library dan tes tetap tidak memasang apa pun
   (ADR-0026); hanya binary yang memasang.
5. **Penolakan awal impor.** `import.rs` memanggil `commands::record_kind(safe, Dml, subject)` sebelum
   mengembalikan errornya. `subject` adalah path berkas yang diimpor, dan hanya hash-nya yang
   tersimpan — sama seperti statement mana pun.
6. **Empat daftar.** `table_op` masuk ke `COMMANDS`, `Command`, `EngineCommand`/`EVERY_COMMAND`, dan
   `RustEngine.commands`, dan `app/Generated/` di-regenerate. Posisinya **sebelum `objects`**, karena
   sufiks usage line sejak `objects` adalah kontrak yang dibekukan golden corpus.
7. **`rows` di `done`** adalah count dari server, atau `-1` bila server tidak melaporkannya — aturan
   yang sama dengan `to_table`; nol yang dikarang tidak pernah dipakai.

## Alasan

1. **Konfirmasi adalah inti gap-nya, bukan detail.** Plan menulis "lewat konfirmasi", dan
   TablePro menaruh kedua operasi di balik persetujuan per pemakaian. Membiarkan classifier menolak
   DDL di `confirm` (ADR-0026) berarti kedua operasi mustahil dijalankan di satu-satunya tingkat yang
   bertanya; menaikkan pengguna ke `full` justru membuang pertanyaannya. Karena itu `table_op`
   meminta, bukan menolak.
2. **Pengecualiannya sengaja sempit.** Ia hidup di dalam `table_op`, bukan di `qh-sql`. Perintah lain
   yang melewati `guard` — `preview`, `export`, `count`, `explain`, `to_table`, `import_data`,
   `apply_changes` — masih menolak DDL di `confirm`, dan tes
   `confirm_refuses_a_write_until_the_caller_confirms_it` tetap hijau tanpa diubah.
3. **Satu jalur log, bukan dua.** `record_decision` diekstrak dari `guard_confirmed` alih-alih
   menyalin pemetaan `LogDecision`, karena dua salinan pemetaan dapat berbeda diam-diam.
4. **Memasang sink di binary, bukan di library, adalah yang membuat tes aman.** Sama dengan
   `queryhive-engine` (ADR-0026): `cargo test` tidak boleh membuka database pengguna.
5. **Subjek hash untuk impor jujur tentang apa yang dinilai.** Pada titik penolakan, impor belum
   membaca satu baris pun, jadi tidak ada statement. Yang dicatat adalah keputusan atas *operasi*;
   path berkas adalah subjek yang bisa diverifikasi ulang oleh pembaca log, dan hash menjaganya tidak
   tersimpan sebagai teks.

## Konsekuensi

- **Satu pengecualian terhadap rantai strictness, dan biayanya dinyatakan.** Untuk `table_op` yang
  **belum** dikonfirmasi, `no_ddl` dan `confirm` sama-sama menolak, jadi jaminan ADR-0026 ("floor
  tidak pernah melewatkan yang ditolak pilihan pengguna") tetap berlaku apa adanya. Yang baru: sebuah
  floor `no_ddl` + pengguna `confirm` resolve ke `confirm` (strictness 2 > 1), dan `table_op` yang
  **dikonfirmasi** bisa berjalan walau policy-nya `no_ddl`. Batas ini hanya untuk `table_op` dan hanya
  saat pemanggil sudah memberi `SAFE_MODE_CONFIRMED=1`; bila kelak policy `no_ddl` harus menang atas
  konfirmasi, tempat memperbaikinya adalah resolusi floor, bukan perintah ini. Dicatat sebagai biaya,
  bukan disembunyikan.
- **MCP tetap `read_only`, jadi `table_op` tidak terekspos di sana.** Tool MCP tidak bertambah; server
  memaksa `SAFE_MODE=read_only` pada setiap panggilan (ADR-0015/0017), dan tingkat itu tetap menolak
  `table_op`.
- **App belum punya UI.** `EngineCommand.tableOp` ada dan dipetakan `RustEngine.commands`, tetapi
  belum ada call site di app. Memilih bukan `full`/`confirm` dari picker tetap menolak, dan sampai
  jalur run app mengirim `SAFE_MODE_CONFIRMED`, `confirm` di app menolak tulisan (dinyatakan di ADR-0026).
  Ini irisan UI yang menyusul, bukan cacat wiring.
- **Execution log tumbuh oleh keputusan, termasuk yang baru.** Penolakan awal impor dan setiap
  keputusan tool MCP kini menjadi baris; retensi belum ada, sama seperti catatan ADR-0026.
- **Satu perintah lebih di usage line yang dibekukan golden.** `tests/golden/usage/unknown_command`
  tetap cocok karena perbandingannya hanya sufiks sejak `objects`, dan `table_op` diletakkan sebelum
  `objects`; assertion literal di `tests/golden.rs` diperbarui mengikuti daftar baru.
- **`table_op` belum punya tes live.** Yang diuji adalah gating Safe Mode dan penolakan sebelum
  connect (`tests/safe_mode.rs`); jalur statement sungguhan terhadap server belum, dan itu dicatat
  sebagai belum diverifikasi.
