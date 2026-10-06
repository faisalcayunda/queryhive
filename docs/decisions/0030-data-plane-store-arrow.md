# 0030 - Data plane app: store hasil Arrow per chunk, `window()` terender, Explain di store (menggantikan 0013 dan 0008, mengamandemen 0004)

- **Status:** Diterima sebagai keputusan arsitektur, dan hampir seluruhnya sudah dibangun. Sisi Rust: W4-T3 `e595877`,
  W4-T4 `b1a1838`, W5-T2 `df445fd` dan `bfb9680`, penutupan W5-C `1b59154`. Sisi Swift (W6-T1): commit 6a `2d14eea` (kolom
  yang terlihat dan kontrak `distinctValues`) dan 6b `1e3c0ad` (`StoreRows`, halaman jendela, penjaga edit, display link,
  `RLIMIT_NOFILE`, kode Swift lama dihapus) ada di `main` dan `work/perf-parity`. Sesi bench eksklusif W6-T2/X2 (`23aa05a`)
  sudah berjalan. Yang masih **direncanakan** hanya D-29 (commit 6c, plafon 5.000.000), yang menunggu P-1 dari pemilik.
  Blueprint Fase 6 disegarkan di W6-A1 (`c19296e`); putaran architect-reviewer atas penyegaran itu memberi lima koreksi
  blocking (B1 sampai B5) yang sudah diterapkan, dan koreksinya sendiri belum ditinjau ulang (ledger W6-A1). Teks pertama
  ADR ini (`f2cbf46`, 08:08) menggambarkan Swift sebelum `1e3c0ad` (08:01); bagian "Verdict architect-reviewer"
  mencatat verifikasi ronde 2 terhadap `4ec7480`.
- **Tanggal:** 6 Okt 2026 (W6-D, perf-parity Fase 6)
- **Konteks instruksi:** `docs/architecture/development-plan.md` §4 (jadwal ADR, baris 0030) dan W6-D;
  `docs/architecture/blueprints/fase-6-data-plane.md` §3 (D-1 sampai D-15, D-22 sampai D-29), §23, §26;
  `docs/architecture/performance-plan.md` §10; PRD `docs/architecture/prd-performance-and-parity.md` FR-PERF-05,
  FR-GRID-03, FR-GRID-04, NFR-P1 (S2), NFR-P2, NFR-P3, NFR-P8, O-12, O-15, O-18.
- **Menggantikan:** ADR-0008 (store kolumnar kustom) dan ADR-0013 (target throughput dibatasi JSON). Keduanya diberi
  catatan bertanggal di bagian atas; badannya tidak ditulis ulang.
- **Mengamandemen:** ADR-0004 (UniFFI untuk control plane). Pemisahan control plane dan data plane bertahan; yang berubah
  adalah isi data plane (objek `ResultHandle` dan buffer `QHW1`, bukan sekadar "handle + offset").
- **Berhubungan dengan:** ADR-0032 (grid sel digambar; seam `ResultRows` yang diisi `StoreRows` di sini), ADR-0034
  (view in-memory di Rust, kolasi, divergensi), ADR-0037 (spill terenkripsi; format rekaman dan aturan kunci),
  ADR-0045 (helper analitik DataFusion, W13-D, belum ditulis).
- **Catatan penomoran:** D-1 sampai D-29 di ADR ini adalah nomor dari blueprint Fase 6. Nomor D yang sama di blueprint
  Fase 2, 4B, dan 5 adalah keputusan yang berbeda.

## Konteks

Sebelum `1e3c0ad` (W6-T1 6b), hasil query di app adalah `PreviewResult.rows: [[String?]]` yang ditumpuk di Swift dari
event NDJSON, lalu disaring, diurutkan, dan dicari di Swift. Biayanya bukan materialisasi koleksi di setiap `body`:
`QueryTab.displayedRows` (`QueryTab.swift:869-892` pada `1e3c0ad^`) di-cache per `gridRevision` lewat `displayedCache`
(`:862-863`, `:889-890`), dan `result` membungkusnya dalam `ArrayRows` yang juga di-cache per revisi (`:900-912`). Biaya yang
nyata ada di dua tempat. Setiap baris hidup di Swift sebagai `[[String?]]`. Lalu setiap revisi filter, search, atau sort
membangun salinan penuh yang tersaring (filter, lalu search) dan terurut (`GridSort.order`, `enumerated().sorted` dengan angka
per sel dan `localizedStandardCompare` untuk teks) di thread utama, karena grid membaca `tab.result` dari view SwiftUI (`ResultGrid.swift:89` pada
`1e3c0ad^`). Array luarnya baru; tiap baris berbagi penyimpanan lewat copy-on-write, tetapi seluruh baris dilewati di
tiap revisi. Semua kode itu dihapus di `1e3c0ad`.

Profil W1-T8 (jalur FFI in-process, `wide_500k`) menunjukkan ke mana waktu
pergi: decode driver 38,4%, transpose/clone 26,8%, `to_text` 13,2%, serde_json 19,6%, dan satu sel teks disalin sekitar
empat kali. Plafon `COPY` sekitar 990k baris/s. Klaim ADR-0013 bahwa JSON adalah 99,6% waktu tidak bertahan: angka itu
hanya `(elapsed − TTFR)/elapsed`. Profil yang sebenarnya menempatkan JSON di 11% (CLI) dan 20% (FFI), penulisan stdout
36% (CLI), dan decode driver 25 sampai 38%, jadi batasnya CPU satu thread engine, bukan protokol.

Tiga keadaan baru membuat ADR-0008 dan ADR-0013 tidak lagi memandu pekerjaan:

1. **Keputusan pemilik O-15.** Arrow dan Apache DataFusion diadopsi. ADR-0008 menolak Arrow sebagai fondasi karena
   dependensi (~60 crate transitif) dan karena Arrow tidak melayani window acak serta spill. Probe W2-A3 (`target/run/df-probe`)
   mengukur ulang: gugus arrow untuk store menambah **18** crate (bukan ~60), versinya sama dengan `parquet` 59.3 yang
   sudah ada di `Cargo.lock`, dan `qh-result-store` memakai kembali indeks chunk + `partition_point` untuk window acak.
2. **Keputusan pemilik O-18.** DataFusion hidup di executable terpisah, `queryhive-analytics` (W13-T8a), supaya app tetap
   sekitar 30 MB (DataFusion menambah +64,79 MB stripped, §2.2 blueprint). Store Arrow tetap di app, dan grid tidak
   boleh bergantung pada helper.
3. **Seam Fase 5 sudah ada.** ADR-0032 menetapkan `ResultRows` dengan `cell(row:column:format:)`, `fullValue`, `rows(in:columns:)`,
   `naturalCharCounts()`, dan `distinctValues(column:)` (`app/Sources/QueryHive/Models/ResultRows.swift`). Data plane
   tinggal mengisinya, dan sejak `1e3c0ad` `StoreRows` yang mengisinya.

Bentuk masalahnya tetap sama dengan ADR-0004: 500.000 baris × 30 kolom harus sampai ke grid tanpa menyalin ke Swift,
dengan window akses acak per viewport, spill di atas anggaran memori, dan time-to-first-row S2 yang tidak menunggu decode JSON.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Format store | **`RecordBatch` Arrow per chunk, encoding dipilih per (chunk, kolom), kolom `Binary` bertag untuk yang tak seragam** | Satu representasi untuk memori, spill (IPC), window, view, dan DataFusion di helper; round trip `Value` tanpa kehilangan, termasuk `Unknown` bentuk `raw` | Dua puluhan encoding dan codec bertag yang harus dijaga; skema fisik berbeda antar-chunk |
| | Store kolumnar kustom (ADR-0008) | Tanpa dependensi arrow | Konversi ke Arrow kelak untuk DataFusion; layout `QHC1` yang sudah disegel W2-A3 tidak pernah dibangun |
| | `Vec<Vec<Option<String>>>` lewat UniFFI | Sederhana | Persis masalah P1: salinan penuh dan jutaan alokasi |
| Jalur ke Swift | **`window()` mengembalikan satu buffer `QHW1` berisi teks terender, offset, dan flag** | Swift tidak tahu apa-apa soal Arrow; renderer `qh_core::render` tetap satu; satu salinan `RustBuffer → Data` | Teks dirender di Rust, jadi format kolom yang belum teruji paritasnya perlu pintu darurat (D-24) |
| | Arrow C Data Interface ke Swift | Tanpa render di Rust | Swift harus merender tiap tipe sendiri (dua renderer yang bisa berselisih); ditolak `performance-plan.md` §13 |
| Batas FFI | **UniFFI untuk objek `ResultHandle` (semua metode throwing); buffer data lewat `Vec<u8>`** | Pemetaan error dan `Sendable` tetap dari UniFFI | Satu penyeberangan UniFFI per halaman; `StoreWindowBench` mengukurnya (W6-T2/X2: jendela resident 64 × 32 p99 0,23 ms, jauh di bawah 0,5 ms); eskalasi C ABI Fase 8 hanya bila p99 > 0,5 ms, dan jendela yang tumpah (p99 1,2 sampai 1,4 ms) diteruskan ke W8-T2 |
| | C ABI manual sekarang | Tanpa generator | Biaya dan risiko bug besar, belum terbukti perlu |
| Explain | **Explain menulis ke store seperti Run** | Grid satu jalur; `RESULT_SINK=store` untuk `preview` dan `explain` | Menyimpang dari `performance-plan.md` §10 butir 5 (dicatat R-13); golden `explain` dijaga G-GOLDEN |
| | Explain tetap NDJSON ke `ArrayRows` | Tanpa perubahan | Mempertahankan penumpukan baris di Swift hanya untuk satu perintah, dan `ArrayRows` produksi tidak jadi habis |
| Tempat view | **Rust (ADR-0034)** | 500k baris tidak pernah disalin ke Swift | Semantik Swift harus di-port dan diuji diferensial |
| | Swift (keadaan sebelum `1e3c0ad`) | Tidak ada port | Setiap baris hidup di Swift sebagai `[[String?]]`, dan tiap revisi filter, search, atau sort membangun salinan penuh yang tersaring dan terurut di thread utama (`displayedRows` di-cache per `gridRevision` lewat `displayedCache`, dan `result` membungkusnya dalam `ArrayRows` yang di-cache per revisi; kode itu di `QueryTab.swift:869-892` pada `1e3c0ad^`, jadi biayanya per perubahan, bukan per `body`); tidak tahan 500k × 30. Kodenya sudah dihapus |

## Keputusan

**Hasil query di app disimpan di Rust sebagai deretan `RecordBatch` Arrow (arrow-rs 59.3), satu batch per chunk. Swift
membacanya hanya lewat `ResultHandle`: halaman teks terender `QHW1` untuk sel yang digambar, dan teks penuh lewat
`cell_text` dan `rows_text`. Explain menulis ke store yang sama. ADR ini menggantikan ADR-0008 dan ADR-0013 dan mengamandemen ADR-0004.**

Rincian yang mengikat. Dua bagian "Sudah dibangun" (Rust dan Swift) diperiksa dengan `git show work/perf-parity:<path>`
pada `4ec7480` (6 Okt 2026); nomor baris yang dikutip berlaku untuk commit itu. Hanya bagian "Masih direncanakan" yang belum
ada di repo.

### Sudah dibangun (sisi Rust)

1. **Store = `RecordBatch` Arrow per chunk (D-1).** Crate `qh-columnar` (D-15) memegang pemetaan `Value` ↔ Arrow,
   `ChunkBuilder`, codec bertag, dan `value_at` (`crates/qh-columnar/src/lib.rs`, `#![forbid(unsafe_code)]`).
   `qh-result-store` bergantung padanya, bukan sebaliknya, supaya driver menulis array Arrow tanpa menarik spill, kripto,
   dan rayon. Rencana W7-T1 (driver menulis langsung ke kolom store, `302899e`) dijatuhkan oleh aturan keep 10% dan
   dibatalkan di `f6f4a0b`; pemisahan crate tetap berlaku. Chunk disegel pada 65.536 baris atau perkiraan 2 MiB
   (`crates/qh-columnar/src/builder.rs:31` dan `:34`).
   Tipe teks adalah `Utf8`, bukan `Utf8View`, karena offset 4 byte lebih kecil dari view 16 byte dan selisihnya keluar
   dari anggaran 256 MiB (`qh-columnar/src/lib.rs`). Layout `QHC1` buatan sendiri tidak dibangun.
2. **Encoding per chunk-kolom (D-2).** Varian `Value` yang benar-benar datang menentukan encoding. Chunk-kolom yang
   tidak seragam menjadi `Binary` bertag (`qh.enc = tagged`), sehingga `Value` apa pun kembali utuh. Itu menutup
   cacat lama: `decode_value` di codec lama mengubah `Unknown` bentuk `raw` menjadi teks lossy (B-5). Skema logis
   untuk SQL disatukan terpisah di `crates/qh-result-store/src/logical.rs` (blueprint §5.4).
3. **Publikasi hanya lewat chunk yang disegel (D-3)**, dan **satu anggaran global 256 MiB di `StoreRegistry` (D-4)**
   (`registry.rs`). Sewa memori untuk helper (bagian D-4) belum ada: ia dibangun di W13-T8b.
4. **Spill (D-5)** mengikuti ADR-0037 apa adanya: rekaman `QHP1` (stream IPC Arrow + flag) disegel AES-256-GCM dengan
   kunci acak per proses, berkas di-unlink sebelum byte pertama, `pread`/`pwrite`, tanpa mmap.
5. **`window()` terender (D-6).** `ResultHandle::window(view_id, first_row, row_count, columns, formats)` mengembalikan
   satu `Vec<u8>` `QHW1` (`crates/qh-ffi/src/store_api.rs:610`): magic, versi, jumlah baris dan kolom, `source_rows`,
   offset teks, flag per sel (NULL 1, EMPTY 2, OPENABLE 4, NUMERIC 8, TRUNCATED 16), dan heap UTF-8. Teks dipotong di
   256 unit UTF-16 pada batas grapheme dan `TRUNCATED` menyala bila terpotong; `rows_text` dan `cell_text` tidak
   dipotong. Teks dirender lewat `qh_core::render::write_text` (`crates/qh-core/src/render.rs:79`), implementasi
   tunggal yang dibungkus `to_text`. Array Arrow tidak pernah menyeberang FFI. Batas: R ≤ 4.096, C ≤ 1.024, R × C ≤ 262.144.
6. **Explain di store (D-8, R-13).** `explain` memanggil `row_target(settings, out)` seperti `preview`
   (`crates/qh-ffi/src/commands.rs:1595`; `preview` di `:1543`), dan `RESULT_SINK=store` hanya diterima bila emitter membawa
   store dari engine host (`commands.rs:2034-2039`). Dari CLI, MCP, dan golden, nilai itu adalah galat usage, jadi mereka
   tetap NDJSON (D-11, D-14).
7. **Handle dibuat pemanggil sebelum run (D-12)**, dan metode `ResultHandle` semuanya throwing (amandemen D-1 Fase 2):
   `row_count`, `window`, `rows_text`, `cell_text`, `set_view`, `distinct_values`, `column_widths`, `release`
   (`store_api.rs`). Tambahan yang dibutuhkan fitur yang ada (D-13): `distinct_values`, `rows_text`, `store_from_rows`,
   `store_stats` (metode `EngineHost` di `crates/qh-ffi/src/host.rs`); `store_synthetic` hanya konstruktor Rust.
8. **Tidak ada perintah engine baru (D-14).** Empat daftar invariant #11 tidak tersentuh.

### Sudah dibangun (sisi Swift, W6-T1 6a `2d14eea` dan 6b `1e3c0ad`)

Nomor baris di bagian ini berlaku untuk `4ec7480`.

9. **`StoreRows: ResultRows` di `app/Sources/QueryHive/Models/StoreRows.swift`**, dengan `poll()` (`:262`), `apply(_:)`
   asinkron (`:295`; `applyBlocking` untuk tes, `:313`), `prepare(formats:)` (`:356`), `dropPages()` (`:455`),
   `rowsOrThrow(in:columns:)` (`:515`), `distinctValues(column:) async` (`:585`), dan `release()` idempoten (`:604`).
   `apply` menjalankan `set_view` di antrean serial per store, dan `install` mengabaikan view yang lebih tua dari yang
   terpasang, jadi `viewID` dan `count` hanya ditulis oleh hop apply. Produksi memakai `EmptyRows` (`:627`) untuk "belum ada
   hasil" (`QueryTab.noRows`, `QueryTab.swift:832`). Aturan "tipe FFI hanya di `StoreRows.swift` dan `Support/RustEngine.swift`"
   berlaku longgar: `DatabaseEngine.swift` tetap bebas FFI (hanya `import Foundation`), tetapi `QueryTab.swift` mengimpor
   `QueryHiveFFI` dan menangkap `StoreFfiError` di `scheduleViewApply` dan `viewApplied` (`:849-850`, `:881`).
10. **Dua store per tab untuk "off" (Batch 7).** `QueryTab.activeResult: StoreRows?` (`QueryTab.swift:810`) dan
    `baseResult: ResultSlot?` (`:571`; `ResultSlot` berisi `meta` dan `rows: StoreRows`, `:256-259`): paling banyak dua store
    per tab. `BaseResultCache` (aturan 10.000 baris) dihapus karena anggaran global dan spill menggantikannya, bersama
    `displayedRows`, `displayedCache`, `resultCache`, `previewPaintInterval`, `PreviewResult.rows`, dan `GridSort.order`; tidak
    satu pun masih ada di `app/Sources`. `GridSort` tinggal nilai yang menggambarkan sort memori. Tab yang masuk latar
    belakang menjatuhkan halaman Swift-nya lewat `dropPages()` (`AppModel+Focus.swift:214-215`) dan store-nya tumpah lebih dulu.
11. **Umur handle dan penutupan tab.** `closeTab` menghentikan `process` **dan** `previewProcess`, lalu `releaseResults()`
    melepas kedua store (`AppModel+Focus.swift:156-163`; `terminate()` di `:158` dan `:159`). Sebelum `1e3c0ad` hanya
    `process?.terminate()`, padahal `preview` dan `explain` disimpan di `previewProcess` (TM-4); cacat itu sudah tertutup.
12. **Startup.** `RustEngine.ensureStoresConfigured(spillDir:budgetBytes:)` dipanggil sekali, sinkron, sebelum
    `QueryHiveApp.main()` (`App.swift:31`), dengan `~/Library/Caches/QueryHive/spill` dan anggaran 256 MiB. Pemanggil pertama
    mendapat sapuan, yang berikutnya `nil` (`RustEngine.swift:179`). Ini menyimpang dari komentar Rust "Call it off the main
    thread" (`crates/qh-ffi/src/host.rs:301`); blueprint §17.6 memutuskannya sengaja (sekali, sinkron, sebelum tab dipulihkan,
    supaya sapuan berkas sisa selesai sebelum Run pertama; ongkosnya satu `mkdir`, `chmod`, dan `read_dir` atas direktori
    yang normalnya kosong, di bawah 1 ms), dan komentar Rust belum diubah. `RustEngine.raiseFileLimit()` dipanggil di awal
    `main` (`App.swift:12`).
13. **Tiga commit** untuk W6-T1: 6a `2d14eea` (kolom terlihat dan kontrak `distinctValues`) dan 6b `1e3c0ad` (store masuk)
    sudah selesai; 6c (plafon) belum. Pelaksana sonnet (O-20). Satu putaran tinjau atas 6a dan 6b (swift-reviewer dan
    database-reviewer, opus) memberi lima temuan blocking (apply yang datang tidak berurutan, `viewBusy` macet dua kali,
    blok baris berpadding dua kali), semuanya diperbaiki dengan tes regresi (pesan commit `1e3c0ad`).

### Masih direncanakan (W6-T1 6c)

14. **Plafon baris 5.000.000 (D-29).** `productRowLimitCeiling` masih 200.000 (`AppModel+Run.swift:497`); `rowLimitCeiling`
    hanya ditulis `--bench` (`:499-501`). Naik di commit tersendiri sesudah P-1, lihat tabel di bawah.

### D-22 sampai D-29 (disegarkan W6-A1, 2026-10-06; D-22 sampai D-28 dibangun, D-29 direncanakan)

| # | Keputusan | Status | Alasan singkat |
|---|---|---|---|
| D-22 | Halaman jendela **64 baris × blok 32 kolom sumber**, miss sinkron di main, tanpa prefetch. Cache paling banyak 24 halaman dan 8 MB, tidak pernah di bawah 8 halaman. `PAGE_ROWS`, `COL_BLOCK`, dan batas cache adalah konstanta **sementara**; W5-T3 dan `StoreWindowBench` mengonfirmasi atau menggantinya (128 baris bila p99 128 × 32 ≤ 0,4 ms termasuk UniFFI, 32 baris bila p99 64 × 32 > 0,4 ms). | Dibangun (`1e3c0ad`): `pageRows = 64`, `columnBlock = 32`, `maxPages = 24`, `minPages = 8`, `maxPageBytes = 8 << 20` (`StoreRows.swift:163-168`); miss dibaca sinkron di `cell` (`:378`) lewat `loadPage` (`:418`), dan `evictPages` (`:445`) memangkas cache; tanpa prefetch. Terukur di W6-T2/X2. | Rust sendiri: 128 × 30 bertipe 0,396 sampai 0,449 ms p99. Lewat UniFFI, jendela resident, sesi eksklusif di `487a18b` (layar 60 Hz, `23aa05a`): 64 × 32 p99 0,23 ms dan 128 × 32 p99 0,43 ms. Menurut aturan D-22, 128 × 32 di atas 0,4 ms jadi 128 baris tidak dipilih, dan 64 × 32 di bawah 0,4 ms jadi 32 baris tidak perlu: 64 bertahan. Komentar kode (`StoreRows.swift:162`) masih menulis "Provisional" dan boleh diperbarui. |
| D-23 | Tabel membangun dan mengecat **hanya kolom yang tergambar** (rentang dari `GridColumnGeometry.columns(in:)` ± satu lebar viewport), commit 6a di atas `ArrayRows`. | Dibangun (6a `2d14eea`): `GridTableView.swift:266` dan `GridHeaderView.swift:112` memakai `geometry.columns(in:)` atas dirty rect, dan `ResultGridTable.swift:493` meminta rentang terlihat ± margin. Tes `GridColumnRangeTests.swift`. | Cacat awalnya (blueprint TM-2, terhadap `465a0c6`): `GridTableView.draw` membuang rentang kolom dan `rowText` membaca semua kolom, jadi hasil 500 kolom membayar 500 pembacaan sel per baris. 6a lolos G-VIS 17/0 tanpa rekam ulang; `open-500x10k` turun dari 118 ms ke sekitar 77 ms (6a), lalu 22,6 ms median setelah 6b (X2, target 30 ms). |
| D-24 | Pintu darurat format = `StoreRows.swiftRenderedFormats`, awalnya `{.json}`: kolom di himpunan itu dibaca lewat `rows_text` dan `ColumnFormat.render` di Swift. Tanpa parameter FFI baru. Format lain pindah ke himpunan itu bila tes kembaran menemukan selisih yang bukan divergensi tercatat. **`Raw` tidak pernah pindah**: selisih `Raw` di luar divergensi tercatat adalah bug dan menghentikan W6-T1. Divergensi tercatat: potongan 256 UTF-16 dan `TRUNCATED` milik `window` (lawan 1.024 `Character` milik `ArrayRows`), dan `openable` kembaran yang lebih longgar. | Dibangun (`1e3c0ad`): `swiftRenderedFormats = [.json]` (`StoreRows.swift:170`), dipakai `loadPage` (`:421-425`) dan `fullValue` (`:463`). Tes `StoreRowsTests.swift:323`, `:336-337`, `:410` (setiap format dibaca seperti kembaran `ArrayRows` dalam divergensi tercatat), dan `:423`. | Paritas format di Rust belum teruji: fixture W4-T4 hanya mencakup sort, filter, dan search (lihat ADR-0034). Printer `Json` tidak punya satu pun tes paritas; `Json` kembali ke Rust hanya setelah fixture `format.json` dua sisi (B-21). Pesan commit `1e3c0ad` mencatat divergensi ketiga: `NUMERIC` dihitung per nilai di Rust dan per tipe kolom di kembaran (tidak digambar). |
| D-25 | `ArrayRows` dan implementasi sort, filter, search Swift **pindah ke target tes** (`ArrayRowsReference.swift`, `SwiftGridReference.swift`) sebagai kembaran acuan, tidak dihapus. | Dibangun (`1e3c0ad`): kedua berkas ada di `app/Tests/QueryHiveTests/`, dan `app/Sources` tidak lagi memanggil `ArrayRows(`. `SortFixtureExport.swift` memanggil `SwiftGridReference` (`:119`, `:146`, `:153`, `:159`). | Menjaga sepuluh tes `ResultRowsTests`, jaring paritas format dan flag, dan `SortFixtureExport`, yang memanggil implementasi Swift (TM-13) dan tidak akan terkompilasi bila dihapus. |
| D-26 | Mitigasi R-19: `RLIMIT_NOFILE` lunak dinaikkan ke min(batas keras, 4.096) saat peluncuran. Menutup fd store yang menganggur **tidak mungkin**. | Dibangun (`1e3c0ad`): `RustEngine.raiseFileLimit(to: 4_096)` (`RustEngine.swift:198-208`, `setrlimit` di `:208`), dipanggil di `App.swift:12`; tes `StoreLifecycleTests.swift:326`. Batas lunak 256 diukur di mesin ini (TM-12). | Berkas spill di-unlink sebelum byte pertama (D-5), jadi fd satu-satunya pegangan ke datanya. 4.096 adalah 16 kali lipat, jauh di atas 100 tab × 2 store. `EMFILE` tetap diperlakukan seperti disk penuh (chunk resident, store lain utuh); ADR-0037 NEG-005 mencatat penanganannya belum diverifikasi tes. |
| D-27 | `apply(ViewSpec)` **asinkron, dengan penjaga edit**: selama `viewBusy`, mengisi, menempel, dan menyimpan perubahan sel ditolak; saat view terpasang, seleksi dan antrean edit dibuang. | Dibangun (`1e3c0ad`): `viewBusy` dengan penomoran `viewGeneration`, diturunkan hanya oleh hop apply terbaru dan direset tiap `activeResult` berganti (`QueryTab.swift:810-887`). `refusedWhileBusy()` (`:745`) menolak staging, fill, dan paste (`:695`, `:719`, `:727`, `:735`), `typeCellEdit` (`:703`), dan `AppModel+Edit.swift:64` menolak `applyChanges`. `WritePlan.build(... viewBusy:)` menolak membangun rencana (`WritePlan.swift:128-133`) dan baris yang tak terbaca menjadi peringatan, bukan lompatan diam-diam. `viewApplied` membuang seleksi dan antrean edit (`QueryTab.swift:887-890`). | Edit dikunci menurut indeks baris tampilan (`CellKey`); antara `set_view` selesai di Rust dan hop ke main, indeks itu menunjuk baris lain, dan `WritePlan` membaca nilai asli lewat `row(at:)` untuk klausa `WHERE` pada `UPDATE` dan `DELETE`. Tanpa penjaga, satu edit bisa menulis atau menghapus baris yang salah (R-35). |
| D-28 | Hitungan footer dari `progress` disaring 5 Hz (`footerCountInterval` 0,2 detik); grid dipoll display link paling tinggi 60 Hz; `rowsDidGrow(from:to:)` hanya membatalkan baris baru. | Dibangun (`1e3c0ad`): `footerCountInterval = 0.2` (`AppModel+Run.swift:62`, dipakai di `:405`); `GridTableView` memakai `CADisplayLink` dengan `CAFrameRateRange(minimum: 30, maximum: 60, preferred: 60)` yang hidup hanya saat tabel ada di window dan `isPolling` (`GridTableView.swift:44-70`); `rowsDidGrow(from:to:)` di `ResultGridTable.swift:871`. | Mempertahankan kadens 200 ms dari sebelum W6-T1 untuk SwiftUI tanpa mengganti seluruh hasil. Baris yang sudah ada tidak berubah saat hasil bertambah, jadi `GridRowTextCache` tidak dikosongkan. |
| D-29 | Plafon 5.000.000 dinaikkan di commit tersendiri (6c) sesudah P-1. Lulus: naik. Gagal: `WindowedRows` masuk lebih dulu. Tidak terukur (izin OS): tetap 200.000. | **Direncanakan.** `productRowLimitCeiling` masih 200.000 (`AppModel+Run.swift:497`). | Aturan `development-plan.md` W6-T1 dan blueprint Fase 5 §5.4 dan §15. Dipisah supaya perpindahan data plane tidak tertahan izin Screen Recording. **Risiko** (ADR-0034 NEG-002): sort di plafon membangun satu kunci natural `Vec<u8>` per baris di luar anggaran 256 MiB (sedikitnya sekitar 325 MB isi kunci untuk 5.000.000 baris, hitungan tangan, belum diukur), jadi P-1 harus mengukur memori puncak sort. |

### Yang tidak berubah

CLI `queryhive-engine`, MCP, korpus golden, `export`, `to_table`, `import_data`, `apply_changes`, `table_op`, `count`,
dan perintah lokal tetap NDJSON (blueprint §20). Tes kesetaraan W5-T2 membandingkan `rows_text` store dengan array
`data` NDJSON sel demi sel, sebagai bukti bahwa renderer tetap satu.

## Alasan

1. **Satu representasi.** Memori, spill, window, view, dan (kelak) DataFusion membaca `RecordBatch` yang sama. Dengan
   store kustom, DataFusion berarti konversi per window atau per query. Keputusan pemilik O-15 menutup opsi itu.
2. **Jawaban atas empat alasan ADR-0008, satu per satu.** (a) Window acak dilayani indeks chunk + `partition_point`,
   bukan scan kolumnar; terukur 1,63 µs p99 untuk satu baris × 30 kolom `wide` dan 38 µs p99 untuk 128 × 30 `wide` (§2.6).
   (b) Arrow memang tidak punya spill, jadi spill tetap kita tulis dan enkripsi (ADR-0037); IPC hanya menggantikan layout
   buatan sendiri. (c) `Decimal128` memetakan `i128 + scale` tanpa konversi per halaman untuk skala seragam; nilai di
   luar presisi 38 jatuh ke `tagged`, tanpa kehilangan. (d) Dependensi terukur: +18 crate dan ≤ +7,35 MB stripped
   (batas atas, §2.2), bukan ~60 crate.
3. **Teks terender di Rust.** Swift tidak perlu tahu tipe Arrow, dan hanya ada satu renderer. Ongkosnya: `to_text` lama
   mengalokasikan sampai empat `String` per timestamp (288 ns per sel), sehingga `write_text` tanpa alokasi (D-6) wajib.
4. **ADR-0013 digantikan oleh profil, bukan oleh rasa.** Klaim "JSON 99,6%" salah; throughput yang dibatasi satu thread
   engine dijawab dengan menghapus transpose, clone, dan penumpukan baris di Swift, bukan dengan mengganti serializer.
5. **Halaman 64 × 32 (D-22).** Satu halaman menutupi 1.344 sampai 1.920 pt, jadi fling 3.000 pt/s hanya memicu sekitar
   dua miss sinkron per detik, sekitar 0,05% waktu main thread. Prefetch menambah jalur asinkron (basi menurut
   `viewID`, balapan dengan `apply`) untuk menghemat jumlah itu.
6. **Pintu darurat sempit (D-24).** Memindahkan hanya format yang terbukti selisih menjaga perilaku Swift persis, dan
   menolak memindahkan `Raw` menjaga seluruh alasan jalur jendela.

## Konsekuensi

### Positif

- **POS-001**: Jalur panas tidak lagi menyalin sel ke Swift. Tab yang menganggur tumpah lebih dulu, jadi NFR-P3 tidak
  dimakan base store yang tidak dipakai.
- **POS-002**: Satu format untuk app dan helper. Data plane siap DataFusion tanpa konversi, dan grid bekerja tanpa
  helper (O-18).
- **POS-003**: Round trip `Value` lossless, termasuk `Unknown` `raw` (B-5 tertutup di `qh-columnar/tagged.rs`).
- **POS-004**: Penyeberangan FFI terbatas pada satu `Vec<u8>` per halaman. Swift memeriksa magic, versi, panjang, dan
  offset monoton sebelum membaca (O(RC)).
- **POS-005**: `closeTab` yang benar (TM-4) menghentikan query yang masih berjalan di server, yang sebelum `1e3c0ad` bocor
  (`AppModel+Focus.swift:158-159`).

### Negatif

- **NEG-001**: Kita memiliki bug store sendiri: codec bertag, flag chunk, penghitung nonce, dan dekoder QHW1. Dijaga
  tes properti, tes spill, dan tes kesetaraan golden, tetapi permukaannya lebih besar daripada sekadar memakai Arrow.
- **NEG-002**: Paritas format di Rust belum teruji (TM-10). Printer `Json` dan format `Text`, `Uuid`, `UnixTimestamp`
  di `render.rs` hanya punya empat tes unit. D-24 menanganinya dengan pintu darurat, bukan dengan bukti.
- **NEG-003**: Anggaran 256 MiB berada di tepi untuk `wide_500k` (kira-kira 250 MiB setelah `shrink_to_fit`, R-29). Hasil
  500k bisa tumpah beberapa puluh MiB atau tidak sama sekali, jadi G-BENCH(3) mencatat `spilled_bytes` di samping memori.
- **NEG-004**: Target lokal window 128 × 30 p99 ≤ 0,25 ms **meleset** (0,396 sampai 0,449 ms, §2.9). Halaman 64 baris
  adalah mitigasinya. Lewat UniFFI (W6-T2/X2) jendela resident 64 × 32 p99 0,23 ms dan 128 × 32 0,43 ms, jadi eskalasi C ABI
  tidak dipicu untuk jendela resident. Jendela yang tumpah p99 1,2 sampai 1,4 ms, di atas 0,5 ms, dan diteruskan ke W8-T2.
- **NEG-005**: Explain lewat store menyimpang dari `performance-plan.md` §10 butir 5, dan `ArrayRows` hanya hidup
  sebagai kembaran tes, sehingga dua implementasi Swift lama harus dijaga agar tetap terkompilasi.
- **NEG-006**: Satu fd per store yang tumpah. Batas lunak 256 fd di app GUI terukur di mesin ini (TM-12), dan mitigasinya
  (D-26, `RustEngine.swift:198-208`) sudah ada. Bench `tabs-100` kini menghitung fd yang terbuka (`78dafd3`: 17, puncak 113,
  lalu 17; X2 mencatat 15, 111, 15), dan `crates/qh-result-store/tests/fd_leak.rs` menjaga agar store yang dilepas tidak
  tertahan bersama fd spill-nya. Yang masih terbuka: perilaku di bawah `EMFILE` (ADR-0037 NEG-005), dan backlog `78dafd3`
  bahwa `spill_one` tidak memeriksa `Phase::Released` sebelum membuat berkas spill (fd sementara, anggaran terhitung kurang).
- **NEG-007**: Search atas 1 juta × 30 memakan 768 sampai 796 ms (ekstrapolasi 500k: sekitar 384 sampai 398 ms), di atas
  sasaran 300 ms; itu sebabnya `apply` asinkron dan grid menampilkan view lama sampai yang baru siap.

### Belum ada

- **D-29, commit 6c.** Plafon 5.000.000 menunggu P-1 (tangkapan compositor yang butuh izin Screen Recording, dijalankan
  pemilik). Bila P-1 gagal, `WindowedRows` masuk lebih dulu. Memori puncak sort di plafon itu belum diukur (ADR-0034 NEG-002).
- **Target yang meleset di W6-T2/X2** (`23aa05a`): TTFR S1-1k 61,9 dan 78,6 ms, S1-10k 73,9 dan 85,7 ms (target 25 dan 40;
  sekitar 40 ms antara akhir Run dan grid pertama tergambar), TTFR S2 sekitar 100 ms (target p95 50), throughput
  `rows-wide-500k` 73,6k baris/s (target 575k; W7), dan jendela tumpah p99 1,2 sampai 1,4 ms (W8-T2). Frame p99 8,3 ms tidak
  bisa dinilai di layar 60 Hz. Tidak terukur di sesi itu: `spilled_bytes` di skenario DB (W6-C `2ee80c5` menambahkan
  keluarannya) dan `mem-5m`.
- **Tinjau ulang W6-A1.** Putaran architect-reviewer atas penyegaran blueprint sudah ada (lima koreksi blocking B1 sampai B5,
  diterapkan di `c19296e`); yang belum ada adalah tinjau ulang atas koreksinya (ledger W6-A1, "pending review of corrections").
- `TM-8`: komentar `column_widths()` di `crates/qh-ffi/src/store_api.rs:685` masih salah (menulis "UTF-16 units" padahal
  hitungan grapheme); backlog ledger tanpa gelombang pemilik.
- Komentar Rust `crates/qh-ffi/src/host.rs:301` ("Call it off the main thread") belum diselaraskan dengan penyimpangan yang
  disengaja di `App.swift:31` (butir 12).
- Helper analitik dan `AnalyticsSession` (W13-T8a sampai c), ADR-0045.

## Bukti

- Kode: `crates/qh-columnar/`, `crates/qh-result-store/`, `crates/qh-ffi/src/store_api.rs`, `crates/qh-ffi/src/commands.rs`
  (`row_target`), `crates/qh-rt/src/lib.rs`, `app/Sources/QueryHive/Models/ResultRows.swift`, `StoreRows.swift`,
  `QueryTab.swift`, `AppModel+Focus.swift`, `AppModel+Edit.swift`, `Support/RustEngine.swift`, dan `App.swift`. Kode Swift
  yang sudah dihapus dibaca dengan `git show 1e3c0ad^:app/Sources/QueryHive/Models/QueryTab.swift`.
- Tes: `crates/qh-ffi/tests/store_sink.rs` (`RESULT_SINK=store`, `closing_the_tab_mid_stream_ends_the_run_cancelled_and_not_failed`);
  `crates/qh-result-store/tests/window.rs`, `spill.rs`, `view.rs`, `logical.rs`, `differential.rs`, `fd_leak.rs`;
  `app/Tests/QueryHiveTests/StoreRowsTests.swift`, `GridColumnRangeTests.swift`, `StoreLifecycleTests.swift`,
  `SortFixtureExport.swift`, dan `Bench/StoreWindowBench.swift`.
- Pengukuran: blueprint Fase 6 §2.2 (ukuran), §2.4 (memori), §2.5 (spill), §2.6 (window Rust), §2.9 (angka W5-T2:
  `target/run/w5t2-bench-window.json`, `target/run/w5t2-bench.json`, commit `bfb9680`, scratch gitignored).
  Profil W1-T8: `target/run/w1-t8-profile.md`. Sisi Swift: sesi eksklusif W6-T2/X2 pada `487a18b`, dicatat di `23aa05a`
  (`docs/benchmarks.md`, `deploy/dev/bench-results.jsonl`) dan di ledger.
- ADR ini tidak menjalankan ulang gate atau bench. Pemeriksaan keberadaan kode memakai `git show work/perf-parity:<path>`
  pada `4ec7480` (6 Okt 2026), tanpa menjalankan cargo, swift, build, tes, atau app, karena sesi bench eksklusif sedang
  berjalan di mesin yang sama.

## Verdict architect-reviewer

**Verdict: ronde 1 meminta satu koreksi blocking dan ronde 2 meminta satu lagi; keduanya sudah diterapkan (6 Okt 2026) dan
menunggu tinjau ulang (pending review, O-20).** Ronde 2 adalah putaran terakhir yang diizinkan (O-19, O-20). Koreksinya
diperiksa terhadap kode oleh penulis (pembacaan kode lewat `git show work/perf-parity:<path>` pada `4ec7480`, tanpa
menjalankan gate, tes, atau bench) dan dicatat sebagai pending review, bukan putaran ketiga. Temuan blocking atas ADR-0034
ada di bagian verdict ADR itu.

**Ronde 1, blocking (diterapkan, lalu dikoreksi lagi di ronde 2)**

1. Baris "Tempat view / Swift (status quo)" pada tabel opsi mengulang bahwa `displayedRows` mematerialisasi koleksi di tiap
   `body`. Ronde 1 menggantinya dengan biaya yang nyata: semua baris hidup di Swift sebagai `[[String?]]`, dan tiap revisi
   filter, search, atau sort membangun salinan tersaring dan terurut di thread utama, dengan `displayedRows` di-cache per
   `gridRevision` (`displayedCache`) dan `result` membungkusnya dalam `ArrayRows` yang di-cache per revisi
   (`QueryTab.swift:869-892`, `:862-863`, `:889-890`, `:900-912`). Deskripsi itu benar untuk kode sebelum `1e3c0ad`, tetapi
   ronde 1 menyajikannya sebagai kode yang berjalan "hari ini". Ronde 2 membetulkannya (di bawah). Baris D-29 diberi catatan
   risiko memori kunci sort yang tidak ditagih (ADR-0034 NEG-002).

**Ronde 1, non-blocking, status di `4ec7480`**

- **Ditutup oleh `92e22b5` (O-23):** routing model W6-T1 (sonnet menurut O-20; daftar opus di `development-plan.md` §6 tidak
  lagi memuat W6-T1, dan "Referensi" menunjuk `development-plan.md` §6, bukan PRD); celah kepemilikan berkas (`GridColumnRangeTests.swift`
  kini ada di daftar berkas W6-T1, `development-plan.md:561`); `signal(SIGPIPE, SIG_IGN)` keluar dari W6-T1 dan milik W13-T8b
  (di app hanya `BenchMode.swift:128` yang memanggilnya); `store_from_rows` dan `store_stats` adalah metode `EngineHost` di
  `host.rs`; kata "akan datang" di ADR-0013 dan ADR-0037.
- **Diterapkan di ronde 2:** penyimpangan `ensureStoresConfigured` sinkron di main thread terhadap komentar `host.rs:301`
  dicatat di butir 12 dan di "Belum ada".
- **Masih terbuka, diserahkan ke orkestrator:** baris status ADR-0008 dan ADR-0013 masih "Diterima" di bawah catatan
  "digantikan". Itu sama dengan pola catatan ADR-0003, tetapi "Digantikan oleh ADR-0030" di baris status akan menolong
  pembaca yang hanya melihat status.

**Terverifikasi benar, terhadap `4ec7480`, tanpa tindakan:** konstanta chunk `crates/qh-columnar/src/builder.rs:31` dan
`:34`; `qh-columnar` `#![forbid(unsafe_code)]` (`lib.rs:10`) dan `Utf8` bukan `Utf8View` (`lib.rs:8`); `store_api.rs:610`
(`window`), `:685` (komentar `column_widths`, TM-8), `:691` (`set_view`), `:738` (`distinct_values`); flag sel `QHW1`
1/2/4/8/16 dan potongan 256 unit UTF-16 (`crates/qh-result-store/src/render.rs:23-35`); batas jendela 4.096 / 1.024 /
262.144 (`store_api.rs:37-41`); `crates/qh-core/src/render.rs:79` (`write_text`); `host.rs:377`, `:390`, `:434`; `qh_rt::view_pool`
(`crates/qh-rt/src/lib.rs:200`); `commands.rs:1595` dan `:2034-2039` (`row_target`, `RESULT_SINK=store` ditolak tanpa
store host). Garis dasar lama `465a0c6` (`AppModel.swift:710`, `AppModel.swift:2692`, "tidak ada `setrlimit`") tidak berlaku
lagi: kode itu sudah berubah di `1e3c0ad`.

**Verifikasi ronde 2 (2026-10-06): 1 blocking, dikoreksi; pending review (O-20).**

Temuan blocking: ronde 1 mengganti satu deskripsi Swift yang salah dengan deskripsi lain yang salah. `1e3c0ad` (6b, 08:01) adalah
leluhur `f2cbf46` (ADR ini, 08:08) dan sudah menghapus `displayedRows`, `displayedCache`, `resultCache`,
`PreviewResult.rows`, `BaseResultCache`, `previewPaintInterval`, dan `GridSort.order`. ADR ini tetap menulis seluruh sisi
Swift sebagai belum dibangun (status, bagian "Direncanakan", D-22 sampai D-28, butir 11 dan 12, "Belum ada") dan mengutip
garis dasar `465a0c6`. Pembaca atau orkestrator bisa menjadwalkan ulang W6-T1 dan W6-T2 yang sudah selesai.

Yang dikoreksi, semuanya terhadap `git show work/perf-parity:<path>` pada `4ec7480`:

1. Status, Konteks, dan tabel opsi ditulis ulang dalam bentuk lampau ("sebelum `1e3c0ad`"). Biaya Swift lama dibaca dari
   `git show 1e3c0ad^:app/Sources/QueryHive/Models/QueryTab.swift`: `displayedRows` di-cache per `gridRevision`, `result`
   membungkusnya dalam `ArrayRows` yang di-cache per revisi, dan biayanya adalah `[[String?]]` untuk setiap baris plus salinan
   penuh yang tersaring dan terurut di thread utama pada tiap revisi.
2. Butir 9 sampai 13 dan D-22 sampai D-28 pindah ke "Sudah dibangun (sisi Swift)" dengan rujukan `StoreRows.swift`,
   `App.swift:31`, `RustEngine.swift:198-208`, `AppModel+Focus.swift:156-163`, `QueryTab.swift:810-887`, dan
   `AppModel+Edit.swift:64`. Hanya D-29 (6c, `productRowLimitCeiling` 200.000 di `AppModel+Run.swift:497`) tetap direncanakan.
3. D-23 ("belum di-commit") diperbaiki: 6a adalah `2d14eea`. D-22 memuat angka X2: 64 × 32 p99 0,23 ms dan 128 × 32 p99 0,43 ms.
4. "Belum ada" memuat hasil W6-T2/X2, adanya putaran AR atas W6-A1, dan target yang meleset. NEG-004 dan NEG-006 diperbarui
   dengan angka dan mitigasi yang kini ada.
5. Non-blocking ronde 2 yang diterapkan: `builder.rs` kini ditulis dengan crate (`crates/qh-columnar/src/builder.rs:31`, `:34`);
   `commands.rs` `:1595` dan `:2034-2039`; baris status tidak lagi bertentangan dengan "Belum ada" soal verdict W6-A1; TM-8
   tetap akurat di `store_api.rs:685`; penyimpangan `host.rs:301` dicatat.

Aturan dua putaran dipenuhi: tidak ada putaran ketiga. Koreksi ini diverifikasi hanya dengan membaca kode; gate, tes, dan
bench tidak dijalankan karena benchmark eksklusif sedang berjalan.

## Referensi

- ADR-0004 (diamandemen), ADR-0008 dan ADR-0013 (digantikan), ADR-0032, ADR-0034, ADR-0037, ADR-0009 (panic unwind
  lintas FFI), ADR-0010 (QoS).
- `docs/architecture/blueprints/fase-6-data-plane.md` §3, §11, §12, §17 sampai §20, §23, §25, §26.
- `docs/architecture/prd-performance-and-parity.md` O-12, O-15, O-18.
- `docs/architecture/development-plan.md` §6 (W6-T1 implementer per O-20).
- Tugas penerus: W6-T1 6c (plafon, setelah P-1), W8-T2 (jendela yang tumpah), W13-T8a sampai c dan ADR-0045 (helper
  DataFusion). W6-T1 6a dan 6b, W6-T2 (bench eksklusif, G-LEAK) sudah selesai; W7-T1 dijatuhkan.
