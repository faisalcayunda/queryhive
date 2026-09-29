# 0031 — Runtime per proses, dan Stop yang sampai ke server (EngineHost bagian 1)

- **Status:** Diterima sebagian — bagian 1 (runtime per proses dan cancel sampai server). Bagian 2
  (`EngineHost`, pool sesi 2+1, semantik reset) masuk di W3 dan akan melengkapi ADR ini.
- **Tanggal:** 30 Sep 2026 (Fase 0 dan Fase 1 perf-parity)
- **Konteks instruksi:** `docs/architecture/development-plan.md` W2-D; `docs/architecture/performance-plan.md`
  (Fase 1: cancel dan runtime); `docs/invariants.md`. Bukti kode: commit `af6018d` (engine) dan
  `a953344` (app menghormati `done.cancelled`).

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

## Item terbuka untuk bagian 2 (W3)

- **Cancel terlepas yang terlambat tidak boleh mengenai statement berikutnya pada sesi yang dipakai
  ulang.** Task detached bisa tiba setelah `EngineHost` memberikan sesi yang sama ke statement baru;
  `EngineHost` harus mengikat cancel ke identitas statement (query id atau generasi sesi), bukan ke
  sesi saja.
- Pool sesi 2+1, semantik reset, dan operasi panjang di luar pool didefinisikan di blueprint
  `docs/architecture/blueprints/fase-2-engine-host.md`.

## Referensi

- ADR-0009 (`catch_unwind` di FFI), ADR-0010 (QoS dan runtime utama), ADR-0016 (batas waktu
  statement, dasar pikir mekanisme server).
