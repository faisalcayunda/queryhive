# 0037 — Spill terenkripsi (IPC Arrow dalam AES-256-GCM) dengan kunci efemeral per proses

- **Status:** Diterima. Implementasi mendarat di `crates/qh-result-store` (commit `e595877`). Tinjauan keamanan
  putaran pertama meminta perubahan (6 temuan blocking, semuanya diperbaiki di commit yang sama); putaran kedua belum
  dilakukan, jadi implementasi berstatus **menunggu tinjau ulang** sesuai kebijakan dua putaran (O-19, O-20).
  Bagian "Belum ada" di Konsekuensi menyebut yang belum tersambung ke app.
- **Tanggal:** 2 Okt 2026 (W4-D, perf-parity Fase 6-core)
- **Konteks instruksi:** `docs/architecture/development-plan.md` §4 (jadwal ADR, baris 0037) dan W4-T3;
  `docs/architecture/blueprints/fase-6-data-plane.md` D-5, §10, §14.8, dan tabel ADR di §23;
  PRD `docs/architecture/prd-performance-and-parity.md` NFR-S3, NFR-S5, O-15, O-18.
- **Berhubungan dengan:** ADR-0008 (store kolumnar kustom, spill ke `~/Library/Caches/<app>/spill/`), yang akan digantikan
  ADR-0030 di W6-D; ADR-0045 (helper analitik, W13-D) yang memakai aturan kunci di sini untuk prosesnya sendiri.
  ADR ini tidak menunggu keduanya: aturan kunci dan format rekaman berdiri sendiri.

## Konteks

Hasil query besar tidak boleh memakan memori tanpa batas, jadi store hasil menumpahkan chunk ke disk ketika anggaran
memori global terlampaui (`StoreRegistry`, LRU antar store). NFR-S3 menuntut spill yang tidak membocorkan isi hasil:
data yang tertulis ke disk adalah salinan data pengguna dari database mereka, dan NFR-S5 melarang menulis data hasil ke
disk selain ekspor, spill terenkripsi, dan file promise yang diminta pengguna.

Tiga keadaan membuat ini lebih dari sekadar "enkripsi di atas berkas":

1. **Format store berubah.** Store menjadi `RecordBatch` Arrow per chunk (O-15; ADR-0030 nanti), jadi isi rekaman spill
   menjadi stream IPC Arrow, bukan layout buatan sendiri (`QHC1`) yang disegel pada rancangan W2-A3.
2. **Akan ada proses kedua yang menulis spill.** O-18 menetapkan DataFusion sebagai helper terpisah (`queryhive-analytics`,
   W13-T8). DataFusion punya spill operator sendiri, dan modenya yang bawaan, `OsTmpDirectory`, menulis IPC teks biasa ke
   `$TMPDIR` (fakta F-4 di blueprint Fase 6), yang melanggar NFR-S3 dan NFR-S5.
3. **Kebocoran lewat kunci bersama lebih buruk daripada tanpa enkripsi bersama.** Bila app dan helper memakai satu kunci,
   kunci itu harus menyeberang proses.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Format rekaman | **Stream IPC Arrow + flag chunk, disegel per chunk** | IPC adalah format serialisasi yang sudah diuji; indeks di memori mempertahankan `pread` satu chunk; decode tanpa salinan kedua | Enkripsi berbingkai milik sendiri karena IPC Arrow tidak punya enkripsi |
| | Layout buatan sendiri (`QHC1`, rancangan W2-A3) | Sudah disetujui | Format serialisasi sendiri yang harus dijaga, padahal store sudah Arrow |
| | Berkas IPC Arrow utuh per store, dienkripsi sekaligus | Satu berkas standar | Footer dan blok tidak terenkripsi; mengenkripsi utuh memaksa dekripsi seluruh berkas untuk membaca satu chunk |
| Pembacaan | **`pread`/`pwrite`, tanpa mmap** | `qh-result-store` memakai `forbid(unsafe_code)`; buka dan decode satu chunk 2 MiB sekitar 0,35 sampai 0,45 ms | Satu salinan baca ke buffer, lalu array menunjuk ke buffer itu |
| | mmap | Tanpa salinan baca | Butuh `unsafe` |
| Kunci | **Satu kunci acak per proses yang menulis spill, hanya di memori, tidak pernah menyeberang proses** | Berkas yatim tidak bisa dibaca sesudah proses mati; tidak ada kunci yang harus disimpan, diputar, atau dilindungi di luar proses | Spill tidak bisa dibaca lintas restart (memang tidak dibutuhkan: sapuan startup membuang yatim) |
| | Satu kunci dibagi app dan helper | Helper bisa membaca spill app | Kunci harus menyeberang proses; ditolak (blueprint D-20) |
| | Kunci tetap atau turunan dari nilai yang diketahui | Spill bisa dipakai bila RNG gagal | Tidak ada kerahasiaan; ditolak: bila RNG gagal, spill dimatikan |
| Spill operator DataFusion | **Di proses helper, lewat `DiskManagerMode::Custom` dengan cipher milik helper** | NFR-S3 dan NFR-S5 tetap berlaku untuk operator | Dibangun di W13-T8, belum ada |
| | `DiskManagerMode::OsTmpDirectory` (bawaan) | Tanpa kode | Teks biasa di `$TMPDIR`; **dilarang** |
| Tanpa spill enkripsi | Tidak menumpahkan sama sekali | Tanpa kripto | Hasil besar memakan memori sampai anggaran, atau gagal |

## Keputusan

**Spill hasil query ditulis sebagai rekaman per chunk: plaintext `QHP1` (header 64 byte, stream IPC Arrow, flag chunk)
yang disegel AES-256-GCM dengan kunci acak efemeral milik registry. Aturannya: satu kunci per proses yang menulis spill,
dan kunci tidak pernah menyeberang proses.**

Rincian yang mengikat, semuanya diperiksa terhadap `crates/qh-result-store/src/spill.rs` dan `registry.rs`:

1. **Kunci.** 32 byte dari `ring::rand::SystemRandom` ke `UnboundKey::new(&AES_256_GCM, …)` dan `LessSafeKey`; array
   asal di-`zeroize` segera. `ring` tidak men-zeroize jadwal kunci AES di dalam `LessSafeKey` saat drop; itu diterima
   dan ditulis di doc modul, karena umur kunci sama dengan umur proses, dan dokumen tidak boleh mengklaim kunci
   "dihapus dari memori". Kunci hanya hidup di dalam `LessSafeKey`: tidak ditulis ke disk, tidak dicatat, `Debug`
   manual `SpillCipher` mencetak `<redacted>`, dan tidak menyeberang FFI.
2. **Gagal tertutup.** Bila `SystemRandom::fill` gagal, `SpillCipher::new` mengembalikan `SpillUnavailable` dan registry
   dibangun tanpa cipher; spill mati, tidak ada kunci cadangan. Tanpa cipher atau tanpa direktori spill, chunk tetap
   resident (`store.rs`, jalur evict mengembalikan `Ok(None)`).
3. **Nonce.** Penghitung `AtomicU64` milik `SpillCipher`, mulai dari 1; nonce 12 byte = empat byte nol diikuti penghitung
   big-endian. Kunci dan penghitung satu objek, penghitung hanya naik, jadi nonce tidak berulang di store mana pun.
   Klaim penghitung memakai `fetch_update` yang menolak pada `u64::MAX` (`SpillUnavailable`, "nonce space exhausted"),
   tidak membungkus ke 0. Nonce disimpan di indeks memori (`Residency::Spilled`), bukan di berkas.
4. **AAD 16 byte** = tag domain 4 byte (`QHS2` untuk rekaman app) ‖ `store_id` u64 LE ‖ `chunk_index` u32 LE. `StoreId`
   berasal dari `AtomicU64` registry dan tidak pernah dipakai ulang (`next_id`), jadi ia memenuhi "id store dan
   generation" di NFR-S3. Tag domain naik dari `QHS1` ke `QHS2` karena plaintext berubah. Rekaman spill operator helper
   memakai `QHD1` dengan kunci helper (`SpillCipher::with_domain`), supaya jenis rekaman terpisah seandainya satu kunci
   kelak melayani keduanya.
5. **Plaintext rekaman.** Offset 0: magic `QHP1`; 4: `u32` panjang IPC; 8: `u32` panjang flag; 12 sampai 63: nol (supaya
   buffer IPC mulai di 64 dan tetap sejajar 16 byte untuk decode tanpa salinan); 64: stream IPC Arrow (skema, satu record
   batch, EOS, `StreamWriter` dengan opsi bawaan, **tanpa** kompresi IPC); lalu `ChunkFlags` (per kolom satu byte
   kehadiran bit openable dan numerik, lalu bitmap masing-masing `ceil(rows / 8)` byte). Skema ikut di setiap rekaman
   karena skema fisik boleh berbeda per chunk.
6. **Di berkas** hanya `ciphertext ‖ tag 16 byte`, ditambahkan di `next_offset` dan tidak pernah ditulis ulang. Berkas
   tidak punya header dan tidak punya metadata teks biasa.
7. **Autentikasi sebelum decode.** `open` (`open_in_place`) berjalan lebih dulu; byte yang tidak lolos GCM menjadi
   `StoreError::SpillAuth { chunk }` dan tidak pernah sampai ke decoder IPC. Decode IPC sendiri berjalan dengan validasi
   dan dibungkus `catch_unwind`: rekaman yang lolos GCM tetap diperlakukan sebagai masukan tak tepercaya (kasus helper),
   dan panic decoder Arrow pada badan IPC cacat menjadi `Corrupt`, bukan abort proses. Skema juga diperiksa
   (`encoding_of`, `check_compatible`).
8. **Berkas dan direktori.** Direktori spill `0700` (`sweep_spill_dir`: menolak symlink atau bukan-direktori, membuat
   `0700` lalu `set_permissions`). Berkas dibuat per store saat spill pertama, bernama `qhs-<pid>-<store_id>-<16 hex acak>.spill`,
   dengan `OpenOptions` `create_new` (`O_EXCL`; path yang ada dan symlink ditolak) dan mode `0600`, lalu **di-unlink
   sebelum byte pertama ditulis**. `ENOENT` pada unlink dianggap berhasil (sapuan instance lain menang balapan); galat
   unlink lain menutup fd dan menonaktifkan spill, karena spill tanpa unlink tidak boleh berjalan. Karena itu direktori
   selalu kosong, dan blok dibebaskan saat fd ditutup (release, drop, atau crash proses).
9. **Sapuan startup.** `StoreRegistry::new` menyapu direktori spill: berkas biasa dan symlink dihapus tanpa mengikuti
   tautan dan tanpa rekursi; subdirektori dibiarkan. Sapuan adalah kebersihan, bukan kerahasiaan: sesudah proses mati
   kuncinya hilang, sehingga sisa data tidak bisa dibaca.
10. **I/O lewat `SpillMedium { write_at, read_at, … }`** (`FileMedium` di produksi, `FaultyMedium` untuk injeksi `ENOSPC`
    di tes). **Disk penuh** (`ErrorKind::StorageFull` atau `raw_os_error() == Some(28)`) menjadi `StoreError::DiskFull`;
    chunk tetap resident dan store lain tidak terpengaruh. Galat I/O lain menjadi `StoreError::Io`.
11. **Lintas proses.** Helper analitik (W13-T8) punya `SpillCipher` sendiri dengan kunci dan penghitung sendiri, direktori
    spill sendiri, dan nama berkas `qhd-…`. Helper tidak pernah melihat berkas spill app atau kuncinya (blueprint D-20, §14.3),
    dan app tidak pernah melihat kunci helper. Spill operator DataFusion memakai `DiskManagerMode::Custom` dengan cipher
    helper; **`DiskManagerMode::OsTmpDirectory` dilarang**. Bagian ini adalah kontrak untuk W13, bukan kode yang sudah ada.

## Alasan

1. **Kunci efemeral per proses menghapus pengelolaan kunci.** Tidak ada kunci untuk disimpan, diputar, atau diselamatkan;
   spill hanya berguna selama proses hidup, jadi umur kunci yang sama dengan proses sudah tepat.
2. **"Satu kunci per proses yang menulis spill"** menjaga penghitung nonce tidak dibagi lintas proses (dua proses dengan
   satu kunci dan penghitung masing-masing akan memakai nonce yang sama) dan menjaga kunci tidak menyeberang batas
   kepercayaan. Aturannya kini umum untuk app dan helper, dan ADR ini adalah tempatnya ditetapkan.
3. **IPC Arrow di dalam bingkai milik sendiri** karena IPC Arrow tidak punya enkripsi, sementara mengenkripsi berkas IPC
   utuh akan memaksa dekripsi seluruh berkas untuk satu chunk. Rekaman per chunk dengan indeks di memori mempertahankan
   `pread` satu chunk, yaitu rancangan yang sudah disetujui sebelum format berganti ke IPC.
4. **`pread`/`pwrite` tanpa mmap** karena crate ini `forbid(unsafe_code)` dan biaya terukurnya kecil: miss atas chunk
   yang tumpah sekitar 0,35 sampai 0,45 ms CPU ditambah `pread`; evict satu chunk sekitar 0,3 ms CPU ditambah `pwrite`
   (blueprint Fase 6 §2.5, chunk 2 MiB; tidak diukur ulang untuk ADR ini).
5. **Unlink sebelum byte pertama** membuat direktori selalu kosong, sehingga G-LEAK dapat membuktikan pelepasan lewat
   `store_stats().stores == 0` alih-alih isi direktori, dan crash tidak meninggalkan berkas yang bisa dibaca.

## Konsekuensi

### Positif

- **POS-001.** Teks hasil tidak muncul di disk: tes `no_plaintext_on_disk` memakai canary ASCII, UTF-8, dan numerik dan
  memeriksa bahwa canary, magic `QHP1`, dan string `qh.enc` tidak ada di byte mentah.
- **POS-002.** Manipulasi terdeteksi: kunci salah, satu bit dibalik, dua rekaman ditukar, rekaman dipindah ke store lain,
  dan rekaman dengan tag domain `QHD1` semuanya `SpillAuth` (tes `a_wrong_key_does_not_open`, `tampering_is_detected`).
- **POS-003.** Disk penuh, rekaman rusak yang terautentikasi, dan nonce habis menjadi galat bertipe, bukan panic atau data
  salah (tes `disk_full_is_an_error_not_a_panic`, `a_corrupt_record_is_an_error_not_a_panic`).
- **POS-004.** Aturan kunci yang sama berlaku untuk helper analitik, sehingga W13 tidak perlu keputusan kripto baru.

### Negatif

- **NEG-001.** Kunci `ring` tidak di-zeroize di dalam `LessSafeKey` saat drop. Diterima karena umur kunci sama dengan
  proses; salinan di memori proses tidak dijanjikan hilang.
- **NEG-002.** Spill tidak bisa dibaca lintas restart. Ini disengaja, tetapi berarti tidak ada pemulihan hasil dari spill.
- **NEG-003.** Setiap rekaman membawa skema IPC (beberapa ratus byte per rekaman 2 MiB), dan setiap miss membayar dekripsi
  (sekitar 0,35 sampai 0,45 ms) ditambah decode.
- **NEG-004.** Desain ini bergantung pada disiplin: kunci tidak boleh diekspor. `StoreRegistry::cipher()` adalah metode
  publik yang mengembalikan `&SpillCipher` (`registry.rs`), dipakai crate lain di workspace; saat ini tidak ada crate
  di luar `qh-result-store` yang bergantung padanya, tetapi tidak ada tipe yang mencegah W5-T2 mengeksposnya lewat FFI.
  Tes `the_key_is_not_in_debug_output` hanya menjaga `Debug`.
- **NEG-005.** Satu fd per store yang pernah tumpah, dan batas lunak fd app GUI macOS adalah 256; `EMFILE` diperlakukan
  seperti disk penuh (blueprint §10.4, R-19). Penanganan `EMFILE` belum diverifikasi oleh tes di ADR ini.
- **NEG-006.** `StoreError::DiskFull` menghentikan spill tetapi tidak memberi jalan keluar lain: hasil melebihi anggaran
  memori tetap harus dibatasi pengguna (batas baris), seperti pesan galat di blueprint §10.4.

### Belum ada (kontrak, bukan kode)

- Store belum tersambung ke app: `qh-result-store` hanya direferensikan workspace; tepi `qh-ffi → qh-result-store`
  ditambahkan di W5-T2 (`development-plan.md` §7). Direktori spill (`~/Library/Caches/QueryHive/spill`, dari Swift,
  blueprint §17.6) belum dikonfigurasi dari app; `StoreConfig.spill_dir` bernilai `None` secara bawaan, artinya spill mati.
- Helper analitik (`helpers/` tidak ada di repo) dan spill operatornya adalah W13-T8a sampai c. Poin 11 di atas adalah kontraknya.
- `Store::spill_path()` selalu `None` (berkas sudah di-unlink); diagnostik tidak bisa menunjuk berkasnya.

### Temuan tinjauan dan perbaikannya (commit `e595877`)

Tinjauan keamanan putaran 1 menemukan enam masalah blocking yang berlaku di dekat jalur spill dan decode; semuanya
diperbaiki di commit yang sama dan diverifikasi gate, bukan putaran 2:

- cache hasil decode terhitung dua kali (anggaran `resident` membungkus ke 2^64 dan menolak semua charge berikutnya): kini
  dicharge saat insert, `uncharge` saturating lewat `fetch_update`, dan `release` membuang entri decode milik store itu;
- kolom di luar jangkauan pada filter, sort, atau window membuat panic di `RecordBatch::column` dan, dari tugas rayon
  yang lepas, mengabort proses: kolom divalidasi (`check_columns`, `columns().get`);
- `trim_zs_tab` dan `trim_white_space` memakai jumlah karakter sebagai indeks byte (panic pada NBSP dan spasi ideografis):
  kini `char_indices`;
- `decode_record` panic pada beberapa badan IPC cacat yang terautentikasi: kini `catch_unwind` menjadi `Corrupt`;
- codec bertag rekursif tanpa batas kedalaman (stack overflow pada nilai Trino dalam): `MAX_DEPTH = 64` di encode dan decode;
- `push_chunk` tidak memeriksa lebar chunk terhadap daftar kolom: kini `ColumnCount`.

Temuan non-blocking yang ikut diperbaiki termasuk nonce yang membungkus (kini gagal tertutup) dan `spill_path` yang
selalu `None`. Rincian lengkapnya ada di baris ledger W4-T3 (`target/run/ledger.md`, scratch gitignored).

## Bukti

- Kode: `crates/qh-result-store/src/spill.rs`, `registry.rs`, `store.rs`, `chunk.rs`; `crates/qh-columnar/`.
- Tes: `crates/qh-result-store/tests/spill.rs` (20 tes, termasuk `no_plaintext_on_disk`, `a_wrong_key_does_not_open`,
  `tampering_is_detected`, `nonces_are_never_reused`, `a_spilled_chunk_reads_back_identical`, `modes_are_0700_and_0600`,
  `the_spill_directory_stays_empty`, `orphans_are_swept_without_following_links`, `disk_full_is_an_error_not_a_panic`,
  `a_corrupt_record_is_an_error_not_a_panic`, `the_key_is_not_in_debug_output`).
- Gate yang tercatat di ledger untuk `e595877`: `cargo fmt`, `cargo clippy --workspace --all-targets -D warnings`,
  `cargo test` 1047 lulus, `cargo deny check licenses` baik, golden 11/22 dengan kumpulan terklasifikasi yang sama.
  ADR ini tidak menjalankan ulang gate.
- Blueprint: `docs/architecture/blueprints/fase-6-data-plane.md` D-5, §2.5 (pengukuran), §10 (rancangan), §14.8 (helper).

## Referensi

- ADR-0008 (store kolumnar kustom, nanti digantikan ADR-0030) dan ADR-0013; ADR-0009 (panic unwind: `catch_unwind` di
  `decode_record` bekerja karena `panic = "unwind"`).
- `docs/architecture/prd-performance-and-parity.md` NFR-S3 dan NFR-S5, O-15, O-18.
- `docs/architecture/blueprints/fase-6-data-plane.md` §23 (daftar ADR: 0037 "dibuat (W4-D), diperluas", 0045 untuk helper).
- Tugas penerus: W5-T2 (tepi FFI), W6-D (ADR-0030 dan ADR-0034), W13-T8a sampai c dan ADR-0045 (helper dan spill operatornya).
