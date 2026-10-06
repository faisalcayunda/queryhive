# Blueprint Fase 6: data plane lewat result store Arrow, siap DataFusion

- **Status:** blueprint tingkat berkas, revisi 30 Sep 2026 atas dua keputusan pemilik: O-15 (Arrow dan Apache DataFusion diadopsi) dan O-18 (DataFusion menjadi komponen opsional terpisah, supaya app tetap sekitar 30 MB, sedangkan store Arrow tetap di app). Revisi ini menggantikan rancangan W2-A3 (codec kustom `QHC1` + rayon). Rancangan lama dan verdict AR-nya disimpan di bagian "Riwayat" di akhir. Verdict AR atas revisi ini ada di bagian "Verdict architect-reviewer (Arrow/DataFusion)" sebelum Riwayat. **Disegarkan W6-A1 (2026-10-06)** terhadap seam Fase 5 yang dibangun W5-T1 (`7b44e29` sampai `13ea642`, ADR-0032) dan permukaan engine yang dibangun W5-T2 (`df445fd`, `bfb9680`) dan ditutup W5-C (`1b59154`, `465a0c6`); bagian yang berubah ditandai dengan frasa itu: §1.1 fakta 11, §1.3, §2.9, D-22 sampai D-29, §11.4, §11.5, §12, §13.10, §15, §16.1, §17 sampai §19, §21, §22, §25, dan §26. Tinjau architect-reviewer atas penyegaran ini menemukan lima koreksi blocking (B1 sampai B5: baseline yang sudah usang, cache halaman saat streaming, balapan `poll()` dengan `apply`, host tes yang tidak bisa menumpahkan, dan aturan tes kembaran); semuanya sudah diterapkan di sini (2026-10-06). Ronde 2 (putaran terakhir menurut O-19 dan O-20) menemukan satu koreksi blocking lagi, urutan `apply` dan `set_view` (§13.2, §17.2, §19, §21.4), yang sudah dikoreksi; revisi ini berstatus pending review dan tidak ada putaran ketiga (§26).
- **Untuk:** W4-T3 (inti store Arrow, spill terenkripsi, view), W4-T4 (fixture diferensial), W5-T2 (sink engine dan `ResultHandle` lewat UniFFI), W6-T1 (integrasi Swift), W7-T1 (driver menulis array Arrow langsung), dan tugas baru W13-T8a (helper analitik `queryhive-analytics`), W13-T8b (klien helper di app, unduh, dan verifikasi), serta W13-T8c (hook komponen di Settings). Pemeriksa: `architect-reviewer`; `security-reviewer` untuk §10, §14.2, §14.8, dan §14.9.
- **Sumber:** keputusan pemilik O-15 dan O-18 (2026-09-30); `app/build.sh`, `app/release.sh`, `app/build-dmg.sh`, `app/QueryHive.entitlements`, `app/sparkle-public-key.txt`; `performance-plan.md` §7 (arti "off"), §9 (seam `ResultRows`), §10 (Fase 6), §11 (Fase 7.1), §13 (alasan lama menolak DataFusion), §17; PRD FR-PERF-05, FR-GRID-03/04, NFR-P1 S2, NFR-P2, P3, P8, NFR-S3, NFR-S5, NFR-C, O-8, O-9, O-12; profil W1-T8 (`target/run/w1-t8-profile.md`); `deny.toml`; ADR-0002, 0007, 0008, 0011, 0012, 0013, 0014, 0018; `docs/invariants.md` #1, #4, #10, #11. Angka di §2 diukur dengan probe sekali pakai di `target/run/df-probe`.
- **Bahasa:** dokumen ini Indonesia. Identifier, komentar kode, pesan log, dan pesan galat Inggris.

## Ringkasan

Store hasil app menjadi deretan `RecordBatch` Arrow (arrow-rs 59.3, versi yang sama dengan `parquet` 59.3 yang sudah ada di `Cargo.lock`), satu batch per chunk. Encoding setiap chunk-kolom dipilih dari varian `Value` yang benar-benar datang. Chunk-kolom yang variannya tidak seragam jatuh ke kolom biner bertag, jadi `Value` apa pun kembali utuh, termasuk `Unknown` bentuk `raw`. Spill tetap AES-256-GCM dengan kunci per proses. Isi rekamannya kini stream IPC Arrow, bukan layout buatan sendiri. Grid tetap membaca jendela teks yang dirender `qh_core::render::to_text`, sehingga Swift tidak tahu apa-apa soal Arrow.

Sort, filter, dan search grid tetap di Rust, dengan array Arrow dan rayon, dan tidak pernah bergantung pada DataFusion. Semantik paritas Swift (kolasi natural, NULL di akhir, seri stabil, operator filter hasil port) bukan semantik SQL, dan sejak O-18 DataFusion bahkan tidak selalu terpasang. Probe juga tidak menunjukkan DataFusion lebih cepat untuk pekerjaan ini (§2.7, dengan catatan jumlah thread). Kunci kolasi natural dibuat memcmp-able, sehingga SQL bisa memakainya lewat UDF `qh_natural()`.

DataFusion hidup di executable terpisah, `queryhive-analytics`, yang dibangun dari workspace Cargo sendiri dan tidak pernah ditaut ke `libqh_ffi.a` (O-18, D-16). App mengunduhnya saat fitur analitik pertama kali dipakai, dengan izin pengguna, lalu memverifikasi tanda tangan EdDSA (kunci Sparkle yang sama) dan SHA-256 yang dipatok di `Info.plist` sebelum menjalankannya sebagai proses anak yang dikurung: tanpa jaringan, dan tanpa menulis berkas selain spill terenkripsinya sendiri (§14.2, §14.9). Data berpindah sebagai IPC Arrow lewat pipa, chunk demi chunk sesuai permintaan helper (§14.4). Kunci spill app tidak pernah keluar dari proses app; helper punya kunci spill sendiri (§14.8). Memori helper disewa dari anggaran global 256 MiB yang sama, paling banyak 128 MiB per query (§14.5). Helper yang crash hanya menggagalkan query yang sedang berjalan (§14.6). Grid, view, spill, dan semua yang dibangun W4–W7 bekerja tanpa helper (§14.14). Harga helper terukur, sekitar 65 MB stripped (§2.2), dan tidak masuk bundel. API minimalnya ditetapkan di sini dan dibangun di W13-T8a–c. UI analitik menjadi item PRD baru. CLI, MCP, golden, ekspor, dan semua perintah lain tetap NDJSON.

## 0. Yang berubah dari rancangan W2-A3

| Bidang | W2-A3 (digantikan) | Sekarang | Alasan |
|---|---|---|---|
| Format chunk | Layout biner `QHC1` buatan sendiri (§Riwayat) | `RecordBatch` Arrow + `ChunkFlags` sampingan (§5, §6) | Keputusan pemilik: DataFusion membaca `RecordBatch` apa adanya, jadi tidak ada konversi kelak |
| Kolom campuran | `TAGGED` di dalam `QHC1` | Kolom `Binary` bertag (`qh.enc = tagged`) di dalam batch Arrow | Round trip `Value` tetap tanpa kehilangan (§5.2) |
| Spill | Byte `QHC1` disegel AES-GCM | Stream IPC Arrow + `ChunkFlags` disegel AES-GCM; kunci, nonce, AAD, unlink, dan `pread` tidak berubah (§10) | IPC adalah format serialisasi Arrow yang sudah diuji; enkripsinya tetap milik kita |
| Spill operator SQL | tidak ada | Di proses helper: DataFusion `DiskManagerMode::Custom` dengan kunci spill milik helper, format rekaman dan aturan berkas yang sama (§14.8) | Bawaan DataFusion menulis IPC teks biasa ke `$TMPDIR` (fakta F-4), melanggar NFR-S3 dan NFR-S5. Kunci app tidak boleh keluar proses. |
| Anggaran | Chunk resident, cache dekripsi, reservasi view | Ditambah sewa memori untuk helper dari registry app, paling banyak setengah anggaran per query (§9.4, §14.5) | O-12: satu anggaran 256 MiB global |
| View grid | rayon atas codec `QHC1` | rayon atas array Arrow; kunci natural memcmp-able (§13) | Semantik paritas Swift bukan SQL, dan grid harus jalan tanpa helper (O-18). Numerik jauh lebih cepat dari DataFusion; natural belum diukur setara thread (§2.7). |
| Ingest Fase 7.1 | `qh_core::ColumnarBuilder` menulis ke layout `QHC1` | Driver menulis ke `qh_columnar::ChunkBuilder` (builder Arrow) (§16.4) | Satu builder untuk store, SQL, dan driver |
| SQL atas hasil, berkas lokal | ditolak (`performance-plan.md` §13) | Helper `queryhive-analytics` yang diunduh saat pertama dipakai, klien dan API UniFFI minimal di app (W13-T8a–c); UI kemudian (§14) | O-15 |
| Letak DataFusion | — | Workspace Cargo terpisah (`helpers/analytics/`), proses terpisah; tidak pernah di `libqh_ffi.a`, CLI, atau MCP (D-16) | O-18: app tetap sekitar 30 MB |
| Tetap | — | `window()` terkemas `QHW1` dan renderer tunggal, `StoreId(u64)`, registry dan LRU, kunci spill per proses, unlink segera, Explain di store, dua store per tab untuk "off", NDJSON untuk CLI/MCP/golden, semua sisi Swift | Tidak disentuh keputusan ini |

## 1. Fakta yang diperiksa sebelum merancang

### 1.1 Dari kode repo

1. **`qh-result-store` tidak dipakai crate mana pun.** Ia hanya anggota workspace (`Cargo.toml:21`, `:46`). Codec dan API-nya bisa diganti tanpa migrasi pemanggil.
2. **Cacat `Unknown` `raw` di codec hari ini.** `decode_value` mengubah `Unknown` bentuk `raw` menjadi teks lossy dan membuang `raw` (`codec.rs:317-337`), sehingga `to_text` setelah round trip memberi teks, bukan hex (`qh-core/src/render.rs:93-97`). Codec bertag pindah ke `qh-columnar` di W4-T3 dan diperbaiki di sana (B-5).
3. **Spill hari ini** teks biasa di `$TMPDIR/queryhive-spill/spill-<pid>-<n>.bin`, dengan mode dari umask dan `create(true).truncate(true)` (`store.rs:320-345`). Sapuan yang disebut komentar `Drop` (`store.rs:107-109`) tidak ada.
4. **`ring` 0.17.14** sudah ada (`Cargo.lock`, lisensi `Apache-2.0 AND ISC`, keduanya diizinkan). `LessSafeKey::seal_in_place_separate_tag`, `open_in_place`, dan `Nonce::assume_unique_for_key` tersedia.
5. **Crate Unicode** `unicode-normalization` 0.1.25 dan `unicode-properties` 0.1.4 sudah transitif. `unicode-segmentation` masuk bersama DataFusion atau langsung; `rayon` belum ada.
6. **Pool rayon harus dibangun di `qh-rt`**, karena `qh_rt::set_thread_qos` privat (`crates/qh-rt/src/lib.rs:203`). Runtime tokio helper analitik juga dibangun lewat builder di sana (§14.3).
7. **`Progress` tidak cocok untuk ≤ 1 event per 16 ms** (aturan "satu event per 1.000 baris", `progress.rs:64-75`).
8. **Fetch adaptif diatur pemanggil.** `ExecuteOptions::max_batch_rows` adalah plafon (`qh-driver/src/lib.rs:242-251`), dan `next_batch(max_rows)` menerima ukuran per panggilan (`:547`).
9. **`EngineError::StaleHandle` sudah ada** (`qh-core/src/error.rs:80`).
10. **Semantik Swift yang di-port** (tidak berubah dari W2-A3): `GridSort.compare/number/isPlainNumber` (`Models/GridSort.swift:83-149`), `ColumnFilter.matches/matchesText/distinctValues` (`Models/QueryTab.swift:201-284`), urutan filter → search → sort di `displayedRows` (`QueryTab.swift:824-848`), `GridSearch.matches` (`Models/GridSearch.swift:21-29`), `GridValue.isOpenable/looksLikeJSON/prettyPrinted` (`Models/GridValue.swift:55-97`), `ColumnFormat.render` (`Models/ColumnFormat.swift:48-94`), `HexDump.decodedHex` (`Support/HexDump.swift:16-40`), `naturalWidths` (`Models/GridMetrics.swift:45-50`, dipanggil dari `ResultGrid.columnWidths`, `Views/ResultGrid.swift:295-301`), tooltip terformat (`Coordinator.view(_:stringForToolTip:point:userData:)`, `Views/ResultGridTable.swift:803-824`), perataan kanan per tipe kolom (`context.numeric`, `Views/ResultGridTable.swift:288-291`, dipakai `GridRowPainter.paintCell`, `Views/GridRowView.swift:262` dan `:280`). Filter dan search bekerja atas teks tersimpan (`QueryTab.swift:829-839`).
11. **`RustEngine` satu-satunya pengimpor modul FFI** (`Support/RustEngine.swift:6-8`). **Disegarkan W6-A1 (2026-10-06): tidak lagi benar.** Sejak W4, `Support/EditorAnalysis.swift` dan `Models/QueryTab.swift` (`sqlStatementRanges`) memakai FFI, dan `Models/ResultRows.swift`, `Models/CellSelection.swift`, dan `Models/GridMetrics.swift` mengimpornya tanpa memakai satu pun tipe FFI. Yang tetap dijaga untuk data plane: tipe `ResultHandle` dan kawan-kawannya hanya muncul di `Models/StoreRows.swift` dan `Support/RustEngine.swift` (§17.1), dan `DatabaseEngine.swift` tetap bebas FFI.
12. **Driver mencampur varian dalam satu kolom.** PostgreSQL membaca lewat protokol teks dan menyimpan teks apa adanya bila parse gagal (`qh-driver-postgres/src/normalize.rs:69-134`): `numeric` `'NaN'`, `timestamp` `'infinity'`, dan `date` `'infinity'` menjadi `Value::Text` di kolom bertipe. `numeric` tanpa typmod membawa skala per nilai (`1.5`, `1.25`). Array PostgreSQL tetap teks. Trino mengirim `Array`, `Row`, `Map`, dan `Unknown` (`qh-driver-trino/src/decode.rs:546-547`, `lib.rs:1285`). `base_type()` (`normalize.rs:138`) dihitung per sel lewat `from_text` (`normalize.rs:74`, dipanggil dari `lib.rs:714`), dan baris ditranspos di `lib.rs:737-748`.
13. **Profil W1-T8** (jalur FFI in-process, `wide_500k`): decode driver 38,4%, transpose/clone 26,8%, `to_text` 13,2%, serde_json 19,6%; satu sel teks disalin sekitar 4 kali; plafon `COPY` sekitar 990k baris/s.

### 1.2 Dari arrow-rs 59.3 dan DataFusion 55.1

F-1. **Versi sejajar.** `parquet` 59.3.0 sudah ada di `Cargo.lock` (dari `qh-export`). DataFusion 55.1.0, rilis terbaru, bergantung pada `arrow` ≥ 59.2 (`datafusion-55.1.0/Cargo.toml`), jadi arrow di graf tetap satu versi, 59.3.0.

F-2. **MSRV.** DataFusion 55.1.0 menyatakan `rust-version = 1.94.0`; workspace menyatakan `rust-version = "1.85"` (`Cargo.toml:30`); toolchain terpasang 1.98.1. arrow 59.3 menyatakan 1.85. Graf yang memuat DataFusion harus menyatakan 1.94, kalau tidak angka itu bohong. Sejak O-18 graf itu hanya workspace helper (D-16), jadi angka 1.94 dinyatakan di sana, dan workspace utama tetap 1.85.

F-3. **Unifikasi fitur `parquet`.** Fitur `parquet` DataFusion menyalakan `parquet` dengan `default-features = true` plus `arrow`, `async`, `object_store`. Default `parquet` memuat `snap`, `brotli`, `flate2-zlib-rs`, `lz4`, `zstd`. Bila DataFusion berada di graf yang sama dengan `qh-export` (`parquet = { default-features = false }`, dengan komentar "`arrow` is deliberately not enabled"), `qh-export` ikut mendapatkannya. Sejak O-18 itu tidak terjadi: unifikasi hanya berlaku di workspace helper, yang tidak memuat `qh-export`, jadi komentar di `crates/qh-export/Cargo.toml` dan ADR-0018 tetap benar. Codec ini dibutuhkan helper untuk membaca Parquet dunia nyata, yang umumnya snappy atau zstd. `zstd-sys` mengompilasi C, dan invariant #4 tetap berlaku karena `.cargo/config.toml` repo juga dibaca saat workspace helper, yang ada di dalam pohon repo, dibangun.

F-4. **Spill DataFusion.** `DiskManagerMode` punya `OsTmpDirectory` (bawaan), `Directories`, `Custom(Arc<dyn TempFileFactory>)`, dan `Disabled` (`datafusion-execution-55.1.0/src/disk_manager.rs:161-177`). Mode bawaan membuat berkas `tempfile` di direktori temp OS dan menulis stream IPC teks biasa ke sana (`disk_manager.rs:342-380`, `datafusion-physical-plan-55.1.0/src/spill/in_progress_spill_file.rs:85`). `Custom` menerima `SpillFile { size, read_stream -> Stream<Bytes>, open_writer -> SpillWriter: io::Write }` (`spill_file.rs:27-52`). Di luar tes, tidak ada jalur spill yang memanggil `path()`. DataFusion membaca spill-nya sendiri dengan `StreamDecoder::with_skip_validation(true)` (`spill/mod.rs:96`), jadi byte harus sudah terautentikasi sebelum sampai ke decoder. AEAD `open` memberikan jaminan itu.

F-5. **Permukaan SQL.** `SQLOptions::with_allow_ddl/with_allow_dml/with_allow_statements` (`datafusion-55.1.0/src/execution/context/mod.rs:2306-2318`) dan `sql_with_options` (`:642`). Tabel URL (`SELECT * FROM '/path/x.csv'`) hanya aktif lewat `enable_url_table()` (`:414`) dan mati secara bawaan. `map_string_types_to_utf8view` dan `schema_force_view_types` bawaannya `true` (`datafusion-common-55.1.0/src/config.rs:296`, `:1246`), jadi keluaran DataFusion memakai `Utf8View`.

F-6. **Zona waktu tetap di Arrow** hanya `±HH:MM`, `±HHMM`, atau `±HH` (`arrow-array-59.3.0/src/timezone.rs:25-49`). Offset dengan detik (LMT historis, misalnya `+07:07:12`) tidak bisa diwakili sebagai zona kolom.

F-7. **`Decimal128`**: presisi ≤ 38, skala ≤ presisi (`arrow-array-59.3.0/src/types.rs:1412-1440`). `Value::Decimal` memegang `i128` (sampai 39 digit) dan skala `u8`.

F-8. **Graf crate** (normal + build, `aarch64-apple-darwin`, `cargo tree` dan `cargo metadata`): `qh-ffi` hari ini 367 crate. Gugus arrow untuk store (`arrow-array`, `-schema`, `-buffer`, `-ipc`, `-ord`, `-select`, `-cast`) menambah 18. DataFusion dengan fitur minimal (§2.3) menambah 88, termasuk codec Parquet. DataFusion dengan fitur bawaan menambah 107. Jadi "ratusan crate" di `performance-plan.md` §13 terlalu besar: yang benar 88 untuk konfigurasi yang dipakai.

### 1.3 Yang ada sekarang (temuan W6-A1, 2026-10-06)

**Disegarkan W6-A1 (2026-10-06).** Diperiksa terhadap commit `465a0c6` (HEAD `work/perf-parity`). Penyegaran ini dimulai dari `5e5b6f9`; dua commit W5-C masuk sesudahnya dan sudah diperhitungkan: `1b59154` (sisi Rust: `claim_run`, pra-taksiran `rows_text`, `spilled_bytes` yang nyata, satu snapshot view di `row_count()`) dan `465a0c6` (sisi Swift: `SilentEngine`, `ResultGrid`, `ResultGridTable`, `DatabaseEngine`). Berkas-berkas itu kini stabil, jadi nomor baris di bagian ini dan di §17 berlaku di `465a0c6`. Setiap butir menunjuk bagian yang memakainya.

TM-1. **Seam sebagaimana dibangun W5-T1.** `ResultRows` (`Models/ResultRows.swift`) berisi `count`, `fetched`, `columns`, `cell(row:column:format:) -> CellText`, `fullValue(row:column:format:)`, `rows(in:columns:)`, `naturalCharCounts()` (hitungan `Character`, batas 64), dan `distinctValues(column:)` yang sinkron. Ia mewarisi `RowReading.row(at:)` (`Models/CellSelection.swift`) tanpa implementasi bawaan. Tidak ada metode yang melempar, dan tidak ada `poll`, `phase`, `viewID`, `apply`, `release`, atau `columnWidths()`. Selisih terhadap rancangan W2-A3 ada di §17.0.

TM-2. **Tabel membangun dan mengecat semua kolom di setiap baris.** `Coordinator.rowText(_:)` (`ResultGridTable.swift:436`) memanggil `rows.cell` (`:458`) untuk setiap kolom di `visibleSources` (`:442`), dan `GridRowPainter.paint` (`GridRowView.swift:175`) mengecat `0..<geometry.widths.count` (`:199`). `GridTableView.draw` (`GridTableView.swift:208`) menghitung `geometry.columns(in:)` lalu membuangnya (`_ = columns`, `:236`). Hasil 500 kolom membayar 500 pembacaan sel per baris yang digambar, termasuk kolom di luar layar; dengan halaman 32 kolom itu 16 panggilan `window` per halaman baris. Rancangan lama mengandaikan pembacaan per sel hanya untuk kolom terlihat. Diperbaiki di commit 6a (§17.3, D-23).

TM-3. **Regresi W5-T1 pada daftar nilai unik.** `ArrayRows.distinctValues` (`ResultRows.swift:108`) berhenti di 10 nilai pertama (urutan kemunculan) dan mengembalikannya. `ResultGrid.filterEditor` memutuskan `browsable = values.count <= ColumnFilter.valuePickerLimit` (`ResultGrid.swift:678`), jadi kolom dengan 50 nilai unik kini mendapat pemilih berisi 10 nilai. `ColumnFilter.distinctValues(in:column:)` (`QueryTab.swift:215`) mengembalikan himpunan lengkap, dan itulah yang membuat kolom seperti itu jatuh ke kolom pencarian teks. Tidak ada tes yang menjaganya. Diperbaiki di 6a lewat kontrak baru (§17.2).

TM-4. **`closeTab` tidak menghentikan Run dan Explain yang sedang berjalan.** `AppModel.closeTab` (`AppModel.swift:708`) memanggil `tabs[index].process?.terminate()` (`:710`), sedangkan `preview` dan `explain` disimpan di `previewProcess` (`:2475` dan `:2973`). `TabCloseTests` tidak mencakupnya. Dengan store, run itu baru berhenti saat `push` berikutnya melihat `Released`; query yang masih menunggu server (belum ada batch pertama) berjalan terus. Diperbaiki di §19.

TM-5. **Pembaca dan penulis `PreviewResult.rows`.** Produksi: `QueryTab` (`displayedRows`, `result`), `ResultGrid` (`summaryText`, `filterEditor`), dan `PreviewResult.summary`. Penulis: `AppModel` 6 situs, `Snapshot` 8 situs ditambah 7 mutasi di tempat (`tab.preview?.rows[i][j] = …`), `BenchMode` 1 situs. Tes: 21 situs konstruksi di 8 berkas, pembacaan `displayedRows` di 5 berkas (termasuk `VisualParityTests.swift`, yang menurut ADR-0032 "tidak diubah" tetapi ikut berubah di W6-T1), dan `baseResult`/`BaseResultCache` di `Batch7Tests`.

TM-6. **Bentuk engine di Swift.** `RustEngine` adalah `struct` dengan `private static let host = EngineHost()` (`RustEngine.swift:60`). `Engine.current` adalah `static let` (`DatabaseEngine.swift:75`). `SilentEngine` untuk `--snapshot` (`DatabaseEngine.swift:80`) sudah ter-commit di `465a0c6`, dan hari ini hanya mengimplementasikan `run`, `terminateAll`, dan `runBlocking`. `Sink.onEvent` memindahkan setiap event ke main. `RustRun.deliversAfterStop` sudah benar untuk `preview` dan `explain`. Selain `MockEngine` di tes dan `SilentEngine` untuk `--snapshot`, tidak ada seam untuk menyuntikkan engine.

TM-7. **Bit flag QHW1 tidak sama dengan `CellFlags`.** Rust: NULL 1, EMPTY 2, OPENABLE 4, NUMERIC 8, TRUNCATED 16 (`render.rs`). Swift: `null` 1, `empty` 2, `truncated` 4, `numeric` 8, `openable` 16. `CellFlags(rawValue: byte)` akan menukar `openable` dan `truncated` tanpa galat kompilasi.

TM-8. **`column_widths()`: dokumentasi Rust salah, dan NULL bernilai 0.** Komentar `column_widths()` di `store_api.rs:685` menulis "UTF-16 units", padahal `HeadWidths::observe` menghitung grapheme (batas 256) dan memperlakukan NULL sebagai teks kosong (0), bukan 4 seperti §8 dan `ArrayRows`. Tidak berdampak pada lebar: rumus `min(max(n × 7,2 + 20, 84), 320) + 22` memberi 84 untuk semua n ≤ 8, dan selisih 0 lawan 4 hanya menyentuh n di bawah itu. Komentar itu masih utuh di `465a0c6`: W5-C sudah ditutup (`1b59154`, `465a0c6`) tanpa memperbaikinya, jadi koreksinya masuk backlog ledger, bukan W5-C.

TM-9. **`StoreStats.spilled_bytes` nyata sejak `1b59154`.** Di `bfb9680` nilainya selalu 0. Di `465a0c6`, `host.rs:397` meneruskan `stats.spilled_bytes` dari `registry.rs:561`, yang menjumlahkan `StoreShared::spilled_bytes()` (byte yang sudah ditulis ke berkas spill) atas store yang masih hidup. Jadi nol sesudah semua tab ditutup mengikuti `stores == 0`, dan angkanya bermakna selama store masih hidup (R-29, G-BENCH(3)).

TM-10. **Fixture diferensial hanya mencakup sort, filter, dan search.** W4-T4 membangun satu berkas, `differential.json` (210 kasus: 6 sort, 171 filter, 27 search, 6 pipeline). Sembilan berkas §15.2 (`format`, `openable`, `width`, `number`, dan seterusnya) tidak dibuat. Printer `Json` dan port format `Text`, `Uuid`, `UnixTimestamp` di `render.rs` hanya punya empat tes unit (dua UUID, satu timestamp, satu JSON), sehingga paritasnya terhadap `ColumnFormat.render` belum diuji sama sekali. Ini yang menentukan §11.4.

TM-11. **Cakupan angka jendela W5-T2.** `bench_ffi window` mengukur panggilan Rust `ResultHandle::window` atas 1 juta × 30 sel yang seluruhnya resident: tanpa penyeberangan UniFFI dan tanpa chunk yang tumpah (§2.9).

TM-12. **Plafon baris dan batas fd.** `AppModel.productRowLimitCeiling` masih 200.000 (`rowLimitCeiling` hanya ditimpa `--bench`). Di mesin ini `launchctl limit maxfiles` memberi 256 (lunak) dan tak terbatas (keras), dan `kern.maxfilesperproc` 61.440. Tidak ada bench yang mengukur fd per store yang tumpah; ADR-0037 NEG-005 menyatakan penanganan `EMFILE` belum diverifikasi tes.

TM-13. **`SortFixtureExport.swift` memanggil implementasi Swift yang akan dihapus** (`GridSort.order`, `ColumnFilter.matchesText`, `GridSearch.matches`), jadi "dilewati setelah W6-T1" (§15.1) tidak cukup: berkas itu tidak akan terkompilasi.

TM-14. **Skenario bench sintetis membangun datanya di Swift.** `scroll-30x1m` memakai `syntheticResult(rows: 1_000_000, columns: 30)` (30 juta sel `String?`), `scroll-500x10k` dan `open-500x10k` 5 juta sel, dan `tabs-100` 2.000 × 10 per tab dengan satu tab hidup dalam satu waktu. `store_synthetic` hanya konstruktor Rust (D-13).

## 2. Pengukuran

### 2.1 Metode

- **Probe** `target/run/df-probe` (sekali pakai, `target`-nya sudah dihapus): crate `staticlib` yang menaut `qh-ffi` apa adanya lewat path dependency, dengan tiga konfigurasi:
  1. acuan: `qh-ffi` saja;
  2. gugus arrow: `arrow-array`, `-schema`, `-buffer`, `-ipc`, `-ord`, `-select`, `-cast` 59.3, dengan builder untuk setiap encoding §5.1, `sort_to_indices`, `take`, `cast`, dan round trip IPC;
  3. DataFusion 55.1 dengan fitur §14.1: `RuntimeEnv` dengan pool dan disk manager, `MemTable`, SQL `GROUP BY`/`JOIN`/`ORDER BY`, UDF, `SQLOptions`, `register_csv`, dan `register_parquet`.
- **Profil dan lockfile:** `[profile.release]` repo (`lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`, `panic = "unwind"`). `Cargo.lock` repo disalin sebagai titik awal, jadi versi yang sudah ada tidak bergeser. `.cargo/config.toml` repo berlaku (invariant #4).
- **Mesin:** Mac16,12 M4, 10 core (4P + 6E), 16 GiB, rustc 1.98.1.
- **Ukuran:** staticlib ditaut ke program C kecil yang mereferensikan 72 simbol FFI dari `app/Generated/qh_ffiFFI/qh_ffiFFI.h` (yang direferensikan app) plus titik masuk probe. Penautan memakai `-dead_strip` dan framework yang sama dengan `app/Package.swift`. Staticlib dengan tiga tipe crate tidak menjalani LTO: `libqh_ffi.a` asli 154,6 MB dan staticlib acuan probe 154,65 MB, jadi yang diukur sama dengan yang ditaut app.
- **Bench:** biner release terpisah (subperintah sort, window, session, render), median 5 run setelah satu pemanasan untuk sort, dan persentil atas 200.000 (1 baris) atau 20.000 (128 baris) jendela acak. Beban 1 menit sekitar 3 selama semua bench.
- **Build:** `/usr/bin/time -p` atas `cargo build --release --lib` lalu `cargo build --lib` di direktori target kosong, acuan dan DataFusion berurutan, dengan wall dan CPU (user + sys). Beban 1 menit 3–11 selama putaran ini.

### 2.2 Build, ukuran, dan graf

| Konfigurasi | Crate (normal + build) | Build rilis bersih | Build debug bersih | `target/` | Biner `hello` | Stripped | `__TEXT` |
|---|---|---|---|---|---|---|---|
| `qh-ffi` saja | 367 | 113 dtk (CPU 498 dtk) | 57 dtk (CPU 306 dtk) | 1,2 GB rilis, 3,4 GB debug | 14,05 MB | 12,17 MB | 10,58 MB |
| + gugus arrow | 385 (+18) | — | — | — | 22,72 MB (+8,66) | 19,52 MB (+7,35) | 17,53 MB (+6,95) |
| + DataFusion §14.1 | 455 (+88) | **364 dtk** (CPU 2.594 dtk) | **140 dtk** (CPU 725 dtk) | 2,3 GB rilis, **9,5 GB** debug | 95,81 MB (**+81,76**) | 76,95 MB (**+64,79**) | 69,88 MB (**+59,29**) |
| + DataFusion inti (hanya `sql`) | 450 (+83) | 331 dtk | — | — | 88,84 MB (+74,78) | 71,24 MB (+59,07) | 64,50 MB (+53,92) |

- Biner app hari ini (`app/dist/QueryHive.app/Contents/MacOS/QueryHive`) 23.666.336 byte (23,7 MB, 22,6 MiB), termasuk Swift. Dengan gugus arrow, sekitar 31 MB. Itu batas atas: sejak normalisasi tipe Arrow luar pindah ke helper (§5.6), app tidak menaut `arrow-cast` dan `arrow-ord`. DataFusion tidak masuk bundel (O-18). Bundel juga memuat `queryhive-mcp` (10,9 MB hari ini) dan Sparkle.
- **Proksi ukuran helper.** Biner bench (DataFusion + arrow + rayon + ring, fat LTO, stripped, tanpa `qh-ffi`) 65,3 MB adalah pengukuran terdekat untuk `queryhive-analytics`. Ukuran terkompresi LZFSE yang benar-benar diunduh belum diukur, dan dicatat di W13-T8a.
- **Keterlacakan angka ukuran.** `hello/link.sh` mencetak ukuran, tetapi keluarannya tidak disimpan ke log dan binari `hello` sudah dihapus. Angka ukuran di tabel karena itu tidak bisa diperiksa ulang dari `target/run`. Waktu build dan ukuran `target/` cocok dengan `df-build2.log`, dan lisensi cocok dengan `deny-*.log`.
- Fitur bukan tuasnya. `parquet` beserta codec-nya dan kelompok fungsi (nested, tanggal, string, unicode, regex) hanya menambah 5,7 MB stripped di atas DataFusion inti. Lantainya adalah DataFusion itu sendiri, sekitar 59 MB.
- LTO tidak banyak menolong. Biner bench (DataFusion + arrow + rayon + ring, fat LTO, stripped, tanpa `qh-ffi`) tetap 65,3 MB, karena fungsi bawaan DataFusion terdaftar di registri saat runtime dan karena itu terjangkau.
- Baris "gugus arrow" adalah batas atas untuk store. Ia ikut menaut `arrow-cast` dan `arrow-ord` untuk semua tipe, sedangkan `qh-result-store` hanya butuh array, schema, buffer, select, dan IPC (§21.1).
- **Relink.** Menyentuh crate yang menginstansiasi generik DataFusion lalu membangun ulang staticlib: 24,3 dtk (acuan 0,9 dtk). Relink biner fat-LTO yang menaut DataFusion (biner bench): 395–433 dtk. Angka ini dulu menjadi alasan fitur Cargo `analytics` di `qh-ffi`. Sejak O-18 DataFusion tidak masuk graf `qh-ffi` sama sekali (D-16), jadi fitur itu tidak dibangun.
- Putaran pertama (acuan 160,5 dtk pada beban 3,5; arrow 191,7 dtk pada beban sekitar 25; DataFusion 487,9 dtk pada beban 50–67, karena agen lain sedang membangun) tidak dipakai untuk selisih. Angka di tabel berasal dari putaran kedua yang berurutan.

### 2.3 Lisensi

Lihat §24. Singkatnya: gugus arrow dan DataFusion §14.1 lulus `deny.toml` apa adanya. Fitur bawaan DataFusion gagal di dua crate.

### 2.4 Memori

| Data 500k × 30 | Isi | Memori Arrow |
|---|---|---|
| `wide` (bentuk `wide_500k`) | 1 `Int64` + 29 `Utf8` `row-<g>-cNN`, chunk 4.096 baris | **280,8 MiB** dengan kapasitas cadangan builder; **≈ 250 MiB** setelah `shrink_to_fit`, tepat di bawah anggaran 256 MiB |
| `typed` | 5 × (`Int64`, `Float64`, `Decimal128(38,10)`, `Timestamp(µs,+07:00)`, `Date32`, `Utf8` `KPM <n>`), chunk 8.192 baris | 152,8 MiB |

Satu chunk `wide`, 29 kolom teks: `Utf8` 2.379.276 byte, `Utf8View` 3.804.336 byte (+60%).

**Koreksi AR.** Probe mengukur `get_array_memory_size()` atas array hasil `collect`, yang menghitung kapasitas buffer, termasuk cadangan builder yang tumbuh berlipat. Satu chunk `wide` 4.096 baris memakai 2.412.140 byte memori untuk 1.953.928 byte IPC (`bench-window.log`). Data `wide_500k` yang sebenarnya (`deploy/dev/seed-postgres.sql`: `'row-' || g || '-cNN'`, rata-rata 13,8 byte) adalah 8 + 29 × (13,8 + 4) ≈ 524 byte per baris, yaitu ≈ 250 MiB untuk 500k baris setelah `shrink_to_fit` (§6.4). Bentuk ini berada di tepi anggaran, bukan jauh di atasnya. Akibatnya G-BENCH(3) di 500k peka terhadap perubahan kecil (bisa tumpah beberapa puluh MiB, bisa tidak sama sekali), jadi bench itu mencatat `spilled_bytes` bersama angka memori (R-29).

### 2.5 Spill: IPC + AES-256-GCM per chunk

| Chunk | Byte IPC | Encode IPC | Segel | Buka | Decode IPC (tervalidasi) | Decode tanpa salinan |
|---|---|---|---|---|---|---|
| `wide`, 4.096 baris | 1.953.928 | 47 µs | 233 µs | 238 µs | 180 µs | ya |
| `typed`, 8.192 baris | 2.364.616 | 66 µs | 281 µs | 289 µs | 65 µs | ya |

Miss atas chunk yang tumpah butuh sekitar 0,35–0,45 ms CPU ditambah `pread`. Evict satu chunk butuh sekitar 0,3 ms CPU ditambah `pwrite`. Decode memakai `StreamDecoder` dengan validasi (API aman), dan array hasilnya menunjuk ke buffer yang sudah didekripsi.

### 2.6 Jendela (sisi Rust, tanpa UniFFI dan tanpa format kolom)

| Data | 1 baris × 30 kolom, p50 / p99 | 128 × 30, p50 / p99 |
|---|---|---|
| `wide` | 1,17 µs / **1,63 µs** | 33 µs / 38 µs |
| `typed` | 5,75 µs / **10,6 µs** | 398 µs / **442 µs** |

Ongkos `render::to_text` per sel (1 juta nilai, alokasi termasuk): `Int` 21 ns, `Text` 14 ns (salinan), `Date` 62 ns, `Decimal(38,10)` 68 ns, `Float` 92 ns, `Timestamp +07:00` **288 ns**. Jalur `QHC1` lama akan membayar ongkos render yang sama, karena ia juga membangun `Value` lalu memanggil `to_text`. Jadi temuan ini soal renderer, bukan soal Arrow. Jawabannya D-6 (`write_text`) dan R-28.

### 2.7 Sort 500k baris

Median 5 run, 31 chunk × 16.384 baris, dengan permutasi baris sebagai keluaran. Teks A = `row-<g>-c01` dengan `g` diacak (kasus natural terburuk: run digit). Teks B = nama campuran huruf besar/kecil dan beraksen, plus angka. "Natural" = proxy kunci §13.4 (level primer, tersier, dan byte mentah; tanpa NFD dan tabel lipat, yang menambah ongkos untuk non-ASCII).

| Jalur | Numerik | Teks A, byte | Teks B, byte | Teks A, natural | Teks B, natural |
|---|---|---|---|---|---|
| **rayon** (`par_sort_unstable` atas `(i64, row)`; kunci natural paralel + `par_sort_unstable_by`) | **2,7 ms** | — | — | **28,6 ms** | **27,2 ms** |
| Kernel arrow `sort_to_indices`, 1 thread (concat + sort) | 6,8 ms | 42,5 ms | 43,4 ms | 72,2 ms (kunci biner) | 51,1 ms (kunci biner) |
| DataFusion `SELECT rid … ORDER BY`, 1 partisi (SQL + rencana + collect) | 36,0 ms | 47,8 ms | 46,8 ms | 66,8 ms (UDF) | 53,4 ms (UDF) |
| DataFusion, 4 partisi | 18,5 ms | 34,8 ms | 36,4 ms | 55,0 ms (UDF) | 47,7 ms (UDF) |
| DataFusion, 10 partisi | 22,9 ms | 37,0 ms | 34,8 ms | 61,5 ms (UDF) | 51,1 ms (UDF) |

Semua jalur jauh di bawah NFR-P8 (numerik ≤ 100 ms, teks ≤ 300 ms). Untuk urutan byte, DataFusion paralel sedikit lebih cepat dari kernel arrow 1 thread, tetapi grid tidak memakai urutan byte.

**Koreksi AR: jumlah thread tidak setara.** Baris rayon memakai pool global rayon dengan 10 thread (`bench-sort.log`: `rayon_threads=10`). DataFusion berjalan di runtime tokio 4 worker (`df-probe/src/bin/bench.rs`: `worker_threads(4)`), jadi baris 10 partisi pun hanya punya 4 worker. `view_pool` produk hanya memakai P-core, yaitu 4 di M4 (§13.1). Untuk numerik selisihnya cukup besar (2,7 ms dengan 10 thread lawan 18,5 ms, dan kernel arrow 1 thread pun 6,8 ms), sehingga kesimpulannya aman. Untuk natural, rayon dengan 4 thread belum diukur dan bisa setara dengan DataFusion 4 partisi (48–55 ms). Klaim "rayon 6,9× dan 1,7–1,9× lebih cepat" di revisi sebelumnya dicabut. D-8 tidak bergantung pada angka ini, dan `bench_ffi view-*` di W5-T2 mengukurnya dengan `view_pool` yang sebenarnya.

### 2.8 DataFusion: sesi dan query

- `SessionContext::new_with_config_rt` + registrasi UDF: **39 µs**. Membuat sesi per tab, bahkan per query, praktis gratis.
- `SELECT count(*)` atas 500k: 0,1 ms (dari statistik `MemTable`). `GROUP BY i0 % 100` dengan `sum(Decimal128)`, `avg(Float64)`, dan `count(*)` atas data `typed` 500k × 30, 4 partisi: 1,2 ms, 199 grup (diperiksa).

### 2.9 Angka W5-T2: jendela dan view di sisi Rust

**Disegarkan W6-A1 (2026-10-06).** Sumber: `target/run/w5t2-bench-window.json` (lima ulangan) dan `target/run/w5t2-bench.json` (tiga ulangan), `bench_ffi` rilis di M4 (`view_pool` 4 thread), commit `bfb9680`. Store sintetis 1 juta × 30 (kolom bigint, double, dan teks bergantian, seluruhnya resident). Jendela: 5.000 panggilan acak per ulangan setelah 200 pemanasan, lewat `ResultHandle::window` (termasuk salinan buffer, tanpa penyeberangan UniFFI).

| Kasus | p50 | p95 | p99 | Catatan |
|---|---|---|---|---|
| `window` 128 × 30 (3.840 sel, 73.010 byte) | 0,369–0,380 ms | 0,388–0,406 ms | **0,396–0,449 ms** | sekitar 103–117 ns per sel di p99. Target lokal blueprint (p99 ≤ 0,25 ms, §21.1) **meleset**. Target NFR-P8 (≤ 0,5 ms termasuk UniFFI) belum bisa dinilai |
| `window-json` 128 × 8 (1.024 sel, 253.476 byte) | 1,73–1,75 ms | 1,79–2,82 ms | 1,91–5,88 ms | dicatat, tidak digate (§11.5); satu kali maksimum 51,8 ms |

| View di 1 juta × 30 (`set_view`, 3 ulangan) | Waktu | Ekstrapolasi linear ke 500k (belum diukur) | Target |
|---|---|---|---|
| sort bigint | 166–221 ms | 83–111 ms | NFR-P8 ≤ 100 ms: di tepi |
| sort teks, kunci natural | 153–182 ms | 76–91 ms | NFR-P8 ≤ 300 ms: lulus |
| filter `Text` | 25–27 ms | 12,5–13,3 ms | sasaran ≤ 300 ms: lulus |
| search semua kolom | 768–796 ms | 384–398 ms | sasaran ≤ 300 ms: **meleset**, dicatat |

Yang tidak tercakup: penyeberangan UniFFI (`StoreWindowBench`, W6-T1), chunk yang tumpah (sekitar 0,4 ms tambahan untuk dekripsi dan decode per miss chunk, §2.5), dan 500k nyata (W5-T3 dan W6-T2). Search 800 ms atas 1 juta baris adalah alasan `apply` bersifat asinkron dan grid tetap menampilkan view lama sampai yang baru siap (§17.2).

## 3. Keputusan desain

| # | Keputusan | Alasan |
|---|---|---|
| D-1 | **Store = `RecordBatch` Arrow per chunk (arrow-rs 59.3).** Satu representasi untuk memori, spill (IPC), jendela grid, view, dan DataFusion. Layout `QHC1` tidak dibangun. | Keputusan pemilik: data plane siap DataFusion sejak awal, bukan codec kustom yang dikonversi kelak. Gugus arrow untuk store menambah 18 crate (F-8), dan versinya sama dengan `parquet` yang sudah ada (F-1). |
| D-2 | **Encoding dipilih per (chunk, kolom) dari varian `Value` yang datang.** Chunk-kolom yang tidak seragam menjadi kolom `Binary` bertag. Skema fisik boleh berbeda antar-chunk. Skema logis untuk SQL disatukan saat tabel didaftarkan (§5.4). | Driver mencampur varian dalam satu kolom (fakta 12), dan W4-T3 menerima `Value` sebelum driver menyatakan tipe (W7-T1). Skema tetap per hasil akan memaksa pengecualian per sel di jalur panas atau kehilangan data. |
| D-3 | **Publikasi hanya lewat chunk yang disegel.** Tidak ada pembaca atas builder yang masih terbuka. Kunci hanya dipegang untuk menambah entri indeks. | Main thread tidak pernah menunggu penulis (`performance-plan.md` §10, risiko kontensi). Tidak berubah. |
| D-4 | **Satu anggaran 256 MiB global di `StoreRegistry`** untuk chunk resident, cache dekripsi, reservasi view, **dan** sewa memori helper analitik. Satu sewa paling banyak setengah anggaran. | O-12. Query SQL boleh mendorong store yang menganggur ke spill, tetapi tidak boleh mengosongkan grid (§9.4, §14.5). Helper adalah proses lain, jadi memorinya disewa di muka, bukan ditagih per alokasi. |
| D-5 | **Spill AES-256-GCM dengan satu kunci acak per registry (per proses di app)**, nonce dari penghitung milik registry, berkas di-unlink sebelum byte pertama ditulis, `pread`/`pwrite`, tanpa mmap. **Isi rekaman: stream IPC Arrow + `ChunkFlags`.** Spill operator DataFusion terjadi di proses helper lewat `DiskManagerMode::Custom`, dengan `SpillCipher` milik helper (kunci acak per proses helper), tag domain AAD `QHD1`, dan direktori spill helper sendiri (§14.8). Kunci tidak pernah menyeberang proses. | Rancangan kripto W2-A3 sudah disetujui AR dan tidak bergantung pada format plaintext-nya. Aturannya kini "satu kunci per proses yang menulis spill". Enkripsi berbingkai kita sendiri atas buffer IPC dipilih karena IPC Arrow tidak punya enkripsi. Bawaan DataFusion menulis IPC teks biasa ke `$TMPDIR` (F-4). mmap tetap ditolak: crate ini `forbid(unsafe_code)`, dan buka plus decode satu chunk 2 MiB sekitar 0,35–0,45 ms (§2.5). |
| D-6 | **`window()` mengembalikan satu `Vec<u8>` (`QHW1`) berisi teks yang sudah dirender, offset, dan flag.** Array Arrow tidak pernah menyeberang FFI. Nilai bertipe dirender lewat `render::write_text(&Value, &mut String)`, implementasi tunggal yang dibungkus `to_text`, tanpa alokasi per sel. `bytes` di jendela terpotong hanya meng-hex prefiks yang muat (§11.2). | Renderer tetap satu. Arrow C Data Interface ke Swift tetap ditolak (`performance-plan.md` §13), karena Swift harus merender tipe sendiri. Swift tidak berubah sama sekali. `to_text` hari ini mengalokasikan sampai empat `String` per timestamp (288 ns per sel), sehingga jendela 128 × 30 yang penuh tipe sudah 442 µs p99 di sisi Rust (§2.6). |
| D-7 | **Permutasi `Vec<u32>`. View identitas tidak punya permutasi.** | Plafon 5.000.000 baris ≪ `u32::MAX`. Tidak berubah. |
| D-8 | **View grid (sort fallback, filter, search, distinct) = array Arrow + rayon di `qh-result-store`, bukan DataFusion, dan tidak pernah bergantung pada helper.** Kunci kolasi natural adalah byte memcmp-able dari satu fungsi (`collate::natural_key`), dipakai sort grid dan diekspos ke SQL sebagai UDF `qh_natural(text)`. Explain tetap menulis ke store. | O-18: DataFusion opsional dan bisa tidak terpasang, jadi grid tidak boleh membutuhkannya. Semantik grid adalah port Swift: NULL di akhir, numerik per sel atas teks, seri diputus baris, filter `>=`/`<=` dengan `swift_double`, search lintas semua kolom dengan pelipatan. Di DataFusion semuanya harus menjadi UDF, dan perencanaan query ikut dibayar di setiap klik. Perluasan filter selama streaming (§13.8) juga tidak cocok dengan model query batch. Angka §2.7 tidak membalik keputusan ini: numerik rayon jauh lebih cepat, sedangkan natural dengan jumlah thread yang setara belum diukur. Satu fungsi kunci berarti grid dan SQL tidak punya dua kolasi natural yang bisa berselisih. |
| D-9 | **`ColumnFormat.render` dan `GridValue.isOpenable` tetap di Swift, hanya untuk sel staged dan pembaca nilai.** | Tidak berubah dari W2-A3. |
| D-10 | **`column_widths()` mengembalikan hitungan grapheme, bukan piksel.** | Tidak berubah. |
| D-11 | **Store sink dipilih `RESULT_SINK=store`, dan store diberikan lewat `Emitter::result_store()`.** | CLI, MCP, dan golden tidak bisa mencapainya. Tidak berubah. |
| D-12 | **Handle dibuat pemanggil sebelum run.** | Tidak berubah. |
| D-13 | **Tambahan yang dibutuhkan fitur yang ada:** `distinct_values`, `rows_text`, `store_from_rows`, `store_stats`. `store_synthetic` hanya konstruktor Rust. | Tidak berubah. |
| D-14 | **Tidak ada perintah engine baru, termasuk untuk analitik.** SQL atas hasil adalah objek UniFFI (`AnalyticsSession`), bukan perintah NDJSON. | Empat daftar invariant #11 tidak tersentuh. NFR-C: CLI dan MCP tidak mendapatkan SQL atas hasil. |
| D-15 | **Crate baru `qh-columnar` di bawah driver:** pemetaan `Value` ↔ Arrow, `ChunkBuilder`, codec bertag, `value_at`. `qh-result-store` bergantung padanya. | W7-T1 membuat driver menulis array Arrow langsung. Driver tidak boleh bergantung pada store, yang membawa spill, kripto, dan rayon. Membuatnya sekarang berarti W7-T1 tidak memindahkan kode. |
| D-16 | **DataFusion hanya di executable helper `queryhive-analytics`,** dibangun dari workspace Cargo terpisah `helpers/analytics/` (paket `qh-analytics`: lib + bin) yang memakai `qh-core`, `qh-columnar`, `qh-result-store`, dan `qh-analytics-proto` lewat path. Workspace utama, `libqh_ffi.a`, `queryhive-engine`, dan `queryhive-mcp` tidak pernah punya DataFusion di grafnya. App hanya menaut klien helper (`qh-ffi/src/analytics/`) dan crate protokol kecil `qh-analytics-proto`, yang tidak membawa DataFusion. `qh-result-store` hanya memakai crate arrow dan tetap `forbid(unsafe_code)`. Sejak W4-T3, kesiapan data plane dijaga oleh skema logis (§5.4) dan tesnya; DataFusion sendiri diuji mulai W13-T8a. | O-18: app tetap sekitar 30 MB (§2.2: DataFusion +65 MB stripped). Workspace terpisah membuat G-RUST tidak pernah membangun DataFusion (§2.2: +6,1 GB `target/debug`, build bersih 113 → 364 dtk rilis), `rust-version` workspace utama tetap 1.85 (F-2), dan unifikasi fitur `parquet` tidak menyentuh `qh-export` (F-3). CLI dan MCP secara struktural tidak punya SQL atas hasil (NFR-C, D-14). Fitur Cargo `analytics` di `qh-ffi` dari revisi sebelumnya tidak dibangun. |
| D-17 | **Satu proses helper per proses app, dijalankan malas saat pemakaian analitik pertama dan berhenti setelah 5 menit tanpa query. Di dalamnya satu `RuntimeEnv` dan satu `SessionContext` per sesi app (per tab).** Sesi app (`AnalyticsSession`) menyimpan pendaftarannya sendiri, sehingga helper yang berhenti atau crash dibangun ulang tanpa tindakan pengguna. SQL dikunci di helper: tanpa DDL, DML, `COPY`, `SET`, dan tabel URL. Keluaran SQL menjadi store baru di app, jadi grid menampilkannya dengan mesin yang sama. | Pool memori dan spill satu per helper, di dalam sewa dari anggaran O-12, sedangkan nama tabel harus per tab. Membuat `SessionContext` hanya 39 µs (§2.8). Proses terpisah juga menggantikan runtime tokio analitik kedua di app: CPU DataFusion tidak pernah berbagi worker dengan I/O driver. NFR-S5: SQL tidak boleh bisa menulis berkas, dan kurungan helper (D-21) menjaganya di tingkat kernel. |
| D-18 | **Urutan SQL adalah urutan DataFusion** (byte untuk teks; `nulls_max`, yaitu `NULLS LAST` untuk naik seperti PostgreSQL). `ORDER BY qh_natural(col)` memberi kolasi grid. | SQL punya semantiknya sendiri, sama seperti sort server Batch 7 memakai kolasi server. Divergensinya dicatat di ADR-0034 bersama daftar O-9. |
| D-19 | **Helper dikirim lewat unduhan saat pertama dipakai, bukan di dalam bundel.** Artefaknya satu Mach-O arm64 terkompresi LZFSE, aset rilis GitHub yang sama dengan DMG, dan dipatok per build app: URL, panjang, tanda tangan EdDSA (kunci Sparkle yang sama), dan SHA-256 executable ditulis `build.sh` ke `Info.plist`. Swift memverifikasi sebelum memasang, dan Rust memeriksa ulang SHA-256 sebelum setiap spawn (§14.2). | O-18: app kecil. Tanda tangan kode ad-hoc (distribusi hari ini; notarisasi di luar lingkup PRD §10) hanya membuktikan integritas, bukan asal. Asal dibuktikan kunci EdDSA yang sudah dijaga jalur rilis Sparkle, dan hash yang dipatok mengikat helper ke satu build app, sehingga tidak ada selisih versi. |
| D-20 | **IPC: bingkai biner di atas stdin/stdout helper, data sebagai stream IPC Arrow, chunk ditarik helper sesuai kebutuhan.** App mengirim batch logis (§5.4). Helper mengirim chunk yang sudah dinormalkan ke encoding store (§5.1). Helper tidak pernah melihat berkas spill app atau kuncinya (§14.3, §14.4). | Pipa anonim tidak punya path dan listener yang bisa dihubungi proses lain. IPC Arrow adalah format yang stabil lintas versi arrow-rs. Data menyeberang sekali lewat kernel dan di-decode tanpa salinan. App tidak butuh `arrow-cast`, dan memvalidasi keluaran helper dengan validator rekaman spill (§6.3). Alternatif yang ditolak: helper membaca spill app (kunci harus keluar proses) dan memori bersama (butuh `unsafe` dan aturan umur lintas proses). |
| D-21 | **Helper dikurung:** `sandbox-exec` dengan profil tetap. Baca berkas boleh, kecuali data app dan Keychain; tulis hanya ke direktori spill helper; tanpa jaringan; tanpa exec lain. Lingkungan dikosongkan, dan hanya tiga fd standar yang diwariskan. Tanpa kurungan, helper tidak dijalankan (§14.9). | NFR-S5 dijaga kernel, bukan hanya oleh `SQLOptions`. App sendiri tidak disandbox (ADR-0007), tetapi helper bisa. Entitlement App Sandbox butuh tanda tangan Developer ID untuk diverifikasi, dan mesin ini tidak punya (ADR-0014), jadi itu jalur migrasi di ADR-0045. |
| D-22 | **Disegarkan W6-A1 (2026-10-06). Halaman jendela 64 baris × blok 32 kolom sumber, sementara; miss sinkron di main; tanpa prefetch.** `PAGE_ROWS`, `COL_BLOCK`, dan batas cache (≤ 24 halaman, ≤ 8 MB, tidak pernah di bawah 8 halaman) adalah konstanta yang W5-T3 dan `StoreWindowBench` mengonfirmasi atau mengganti. | 128 × 30 bertipe sudah 0,396–0,449 ms p99 di sisi Rust (§2.9), jadi 128 × 32 tidak punya margin terhadap 0,5 ms setelah UniFFI. 64 × 32 ≈ 0,22–0,24 ms. Satu halaman menutupi 1.344–1.920 pt, jadi fling 3.000 pt/s hanya memicu sekitar dua miss per detik (§17.2). |
| D-23 | **Disegarkan W6-A1 (2026-10-06). Tabel membangun dan mengecat hanya kolom yang tergambar** (rentang dari `GridColumnGeometry.columns(in:)` ± satu lebar viewport), dikerjakan di commit 6a di atas `ArrayRows`. | TM-2. Tanpa ini hasil 500 kolom membaca 500 sel per baris. 6a diverifikasi dengan G-VIS tanpa rekam ulang sebelum store masuk. |
| D-24 | **Disegarkan W6-A1 (2026-10-06). Pintu darurat format = `StoreRows.swiftRenderedFormats`, awalnya `{.json}`.** Kolom di himpunan itu dibaca lewat `rows_text` (teks penuh) dan `ColumnFormat.render` di Swift untuk sel yang digambar. Tanpa parameter FFI baru. Format lain pindah ke himpunan itu bila tes kembaran W6-T1 menemukan selisih yang **bukan divergensi tercatat** (§17.2): potongan 256 unit UTF-16 dan semantik `TRUNCATED` milik `window` (lawan 1.024 `Character` milik `ArrayRows`), serta `openable` kembaran yang lebih longgar, adalah divergensi, bukan selisih. **`Raw` tidak pernah pindah**: selisih `Raw` di luar divergensi tercatat adalah bug dan menghentikan W6-T1. | TM-10: tidak ada fixture paritas format. Perilaku Swift hari ini terjaga persis, dan biayanya hanya jatuh pada kolom berformat yang dipilih pengguna. Tanpa divergensi tercatat, setiap format, `Raw` termasuk, akan selisih pada sel panjang (potongan 256 lawan 1.024), dan aturan "selisih memindahkan format" akan memindahkan `Raw` ke jalur Swift, yang menggagalkan rancangan jendela. |
| D-25 | **Disegarkan W6-A1 (2026-10-06). `ArrayRows` dan implementasi sort, filter, dan search Swift pindah ke target tes sebagai kembaran acuan**, tidak dihapus. Produksi memakai `EmptyRows` untuk "belum ada hasil". | Pemakai produksi `ArrayRows` habis setelah W6-T1. Sebagai kembaran ia memberi jaring paritas format dan flag yang tidak dibuat W4-T4, menjaga sepuluh tes `ResultRowsTests`, dan membuat `SortFixtureExport` tetap bisa meregenerasi fixture (TM-13). |
| D-26 | **Disegarkan W6-A1 (2026-10-06). Mitigasi R-19: `RLIMIT_NOFILE` lunak dinaikkan ke min(batas keras, 4.096) saat peluncuran.** Menutup fd store yang menganggur **tidak mungkin**. | Berkas spill di-unlink sebelum byte pertama (D-5), sehingga fd adalah satu-satunya pegangan ke datanya: menutupnya membuang data. Batas lunak launchd 256 terukur (TM-12). 4.096 adalah 16 kali lipat, jauh di atas 100 tab × 2 store. |
| D-27 | **Disegarkan W6-A1 (2026-10-06). `apply(ViewSpec)` asinkron, dengan penjaga edit.** Selama `viewBusy`, mengisi, menempel, dan menyimpan perubahan sel ditolak. Saat view terpasang, seleksi dan antrean edit dibuang. | Edit dikunci menurut indeks baris tampilan (`CellKey`). Antara `set_view` selesai di Rust dan hop ke main, indeks itu menunjuk baris lain, dan `WritePlan` membaca nilai asli lewat `row(at:)` untuk klausa `WHERE` pada `UPDATE` dan `DELETE`. Tanpa penjaga, satu edit bisa menulis atau menghapus baris yang salah. |
| D-28 | **Disegarkan W6-A1 (2026-10-06). Hitungan footer dari `progress` disaring 5 Hz (`footerCountInterval` 0,2 detik); grid dipoll display link paling tinggi 60 Hz; `rowsDidGrow(from:to:)` hanya membatalkan baris baru.** | Mempertahankan kadens 200 ms hari ini untuk SwiftUI tanpa lagi mengganti seluruh hasil, dan menghindari evaluasi `body` per frame. Baris yang sudah ada tidak berubah saat hasil bertambah, jadi `GridRowTextCache` tidak dikosongkan. |
| D-29 | **Disegarkan W6-A1 (2026-10-06). Plafon 5.000.000 dinaikkan di commit tersendiri sesudah P-1.** Lulus: naik. Gagal: `WindowedRows` masuk lebih dulu. Tidak terukur (izin OS): tetap 200.000. | Aturan `development-plan.md` W6-T1 dan blueprint Fase 5 §5.4 dan §15. Dipisah supaya perpindahan data plane tidak tertahan izin Screen Recording. |

## 4. Alur data

```
Run (Swift, main)
  RustEngine.runIntoStore
    (sekali, sinkron, di QueryHiveMain.main sebelum tab dipulihkan, §17.6) host.configure_result_stores(spill_dir, 256 MiB) -> sapuan
    host.create_result_store()                     -> ResultHandle {store_id}   (dibuat sebelum run, D-12)
    [antrean .userInitiated] host.run_with_store(Preview|Explain, settings + RESULT_SINK=store, store, sink, cancel)
      commands::preview -> stream_rows -> pump_result(RowTarget::Store)
        cursor.next_batch(200 -> 800 -> 3.200 -> 12.800 -> 16.384)        (W7-T1: cursor.next_chunk -> ChunkBuilder)
        StoreWriter.push(batch) -> qh_columnar::ChunkBuilder --segel per push--> StoreChunk { RecordBatch, ChunkFlags }
                                -> indeks (RwLock singkat) -> rows (AtomicU32, Release)
                                \-> registry.charge() -> evict LRU -> IPC + SpillCipher.seal -> pwrite (fd sudah di-unlink)
        event: step connect, columns, progress{rows} <= 1 per 16 ms, done
Grid (main, per frame selama streaming)
  displayLink (<= 60 Hz) -> StoreRows.poll() = handle.row_count() (load atomik) -> Coordinator.rowsDidGrow(from:to:)
  draw -> rowText(r, kolom yang tergambar) -> StoreRows.cell(r, c) -> halaman cache --miss--> handle.window(view, first, 64, kolom[<=32], format)
                               -> render dari array Arrow lewat render::to_text -> Data (QHW1) -> WindowPage
Sort fallback / filter / search / "off"
  [antrean serial per store, .userInitiated] handle.set_view(spec) -> qh_rt::view_pool() (rayon) -> permutasi -> view_id baru
SQL atas hasil / berkas lokal (W13-T8a–c; UI kemudian)
  [Swift, sekali] komponen terpasang? tidak -> izin -> unduh -> EdDSA -> buka -> SHA-256 -> codesign -> Application Support
  host.configure_analytics(path, sha256, spill_dir)                 (tanpa spawn; buat dan sapu spill_dir)
  AnalyticsSession (per tab, di app) -> register_result("r", handle) | register_file("f", path, Csv|Parquet)
  session.run_sql(sql, out_store, sink, cancel)
      klien: helper hidup? tidak -> SHA-256 ulang -> spawn lewat sandbox-exec -> Hello -> putar ulang pendaftaran
      registry.reserve_query(<= 128 MiB) -> sewa -> RunSql{sewa} ke helper
      helper: DataFusion -> RemoteStoreTable -> ChunkRequest{token, chunk, kolom}
      app (worker blocking, bukan main): chunk resident atau didekripsi -> batch logis (§5.4) -> IPC -> pipa
      helper: operator di dalam sewa; spill operator -> SpillCipher helper (kunci helper)
      helper: keluaran -> from_arrow -> ResultChunk (IPC, encoding §5.1) -> app: validasi (§6.3) -> StoreWriter(out_store)
      Done -> sewa kembali; helper crash -> satu event error, sewa kembali, helper dibangun ulang saat dipakai lagi
Tutup tab
  closeTab: RustRun.terminate() (process dan previewProcess, §19) -> StoreRows.release() / session.close() -> chunk dilepas, anggaran dikembalikan, fd spill ditutup,
                                                             CloseSession ke helper bila hidup
```

**Disegarkan W6-A1 (2026-10-06).** Baris peluncuran, Grid (halaman 64 baris, sementara, D-22), dan Tutup tab disesuaikan dengan §17 dan §19.

## 5. Pemetaan tipe: `Value` ↔ Arrow

### 5.1 Encoding per chunk-kolom

Setiap field di batch fisik bernama posisional `c<i>` dan membawa metadata `qh.enc`. Nama kolom asli tetap di `ColumnMeta` milik store, karena hasil SQL boleh punya nama kembar atau kosong (`SELECT 1 AS a, 2 AS a`), sedangkan DataFusion menolak nama kembar. Pembaca tidak pernah menebak encoding dari `DataType`: `Binary` bisa `bytes` atau `tagged`, dan `Utf8` bisa `text` atau `json`.

| Sel bukan NULL dalam chunk-kolom | `qh.enc` | `DataType` Arrow | Round trip ke `Value` |
|---|---|---|---|
| tidak ada | `null` | `Null` | `Null` |
| semua `Bool` | `bool` | `Boolean` | tepat |
| semua `Int` | `i64` | `Int64` | tepat |
| semua `UInt` | `u64` | `UInt64` | tepat (`u64::MAX` tetap) |
| semua `Float` | `f64` | `Float64` | pola bit utuh: NaN, `-0.0` (tes `a_float_keeps_its_exact_bits` dipindah) |
| semua `Decimal` dengan satu skala `s ≤ 38` dan `\|unscaled\| < 10^38` | `dec` | `Decimal128(38, s)` | tepat. Presisi selalu 38, karena `Value` tidak membawa presisi (F-7). |
| semua `Date` | `date` | `Date32` | tepat |
| semua `Time` | `time` | `Time64(Microsecond)` | tepat |
| semua `Timestamp` tanpa zona | `ts` | `Timestamp(Microsecond, None)` | tepat. `micros` jam dinding sebagai UTC, sama dengan semantik Arrow tanpa zona. |
| semua `Timestamp` ber-offset, satu offset dalam menit utuh | `tsz` | `Timestamp(Microsecond, Some("+HH:MM"))` | tepat. `micros` adalah instan (`render.rs:269-278`), dan offset dibaca dari zona kolom. |
| semua `Interval`, `micros × 1000` tidak overflow | `interval` | `Interval(MonthDayNano)` | tepat (`nanos / 1000`) |
| semua `Text` | `text` | `Utf8` | tepat |
| semua `Json` | `json` | `Utf8` + `ARROW:extension:name = arrow.json` | tepat, teks server apa adanya |
| semua `Bytes` | `bytes` | `Binary` | tepat |
| campuran; `Timestamp` dengan offset berbeda, offset berdetik (F-6), atau campur zona dan tanpa zona; `Decimal` skala campuran atau > 38 digit; `Interval` overflow; `Array`, `Row`, `Map`, `Unknown` | `tagged` | `Binary` | tepat: satu blob `tagged::encode` per sel, NULL lewat bit validitas |

`Utf8` dipakai, bukan `Utf8View`. Untuk bentuk `wide_500k`, view 16 byte per sel membuat memori teks satu chunk naik dari 2,38 MB menjadi 3,80 MB (+60%, §2.4), sedangkan offset `i32` hanya 4 byte. Selisih itu langsung memakan anggaran 256 MiB. Keluaran DataFusion yang berupa `Utf8View` (F-5) dikonversi saat masuk store (§5.6).

### 5.2 Codec bertag

`qh_columnar::tagged` adalah codec `codec.rs` hari ini, dipindah tanpa mengubah tag (tag hanya boleh ditambah). Dua perubahan:

1. `TAG_UNKNOWN` menulis satu byte kehadiran (bit0 `text`, bit1 `raw`), lalu `type_name`, `text` bila ada, dan `raw` bila ada. `decode` mengembalikan ketiganya apa adanya. Ini perbaikan B-5: `Unknown` bentuk `raw` kembali sebagai `raw`, dan `to_text`-nya tetap hex.
2. NULL di kolom bertag memakai validitas Arrow, bukan `TAG_NULL`. `TAG_NULL` tetap dikenali di dalam komposit (`Array` berisi NULL).

Tes wajib: generator berbenih yang membangkitkan setiap varian `Value`, termasuk komposit bersarang, `Unknown` tiga bentuk, `Decimal` 39 digit, `Float` NaN dengan payload, offset berdetik, dan interval ekstrem. Tes itu memeriksa `value_at(seal(push(v))) == v` dan `to_text` yang sama, baik resident maupun setelah spill.

### 5.3 Pemilihan encoding di builder

`ChunkBuilder` menahan builder Arrow bertipe per kolom (`Int64Builder`, `StringBuilder`, …), bukan `Vec<Value>`. Encoding kolom ditetapkan oleh sel bukan NULL pertama dalam chunk. Ketika sel yang tidak cocok datang (varian lain, skala lain, offset lain), kolom itu dikonversi **sekali** ke `tagged`: sel yang sudah masuk di-encode ulang, dan sisanya langsung bertag. Karena satu push satu segel (§6.2), keputusan ini per chunk, bukan per hasil. Satu `'NaN'` di kolom `numeric` hanya membuat satu chunk bertag.

`ChunkBuilder` punya metode append bertipe publik (`push_i64`, `push_str`, `push_null`, `push_value`, …). Driver W7-T1 menulis lewat metode itu tanpa `Value` perantara (§16.2).

### 5.4 Skema logis untuk SQL

DataFusion melihat skema logis per kolom, yang dihitung dari `ColumnStats` milik store. Penulis memperbarui ringkasan ini per chunk saat segel: himpunan varian yang pernah muncul, skala maksimum, offset (seragam atau tidak), dan penanda overflow.

| Varian bukan NULL di seluruh hasil | Tipe logis |
|---|---|
| tidak ada | `Null` |
| hanya satu dari `Bool`, `Int`, `UInt`, `Float`, `Date`, `Time`, `Bytes` | tipe encoding-nya |
| hanya `Decimal` | `Decimal128(38, skala_maks)` bila setiap nilai muat setelah diskalakan; selain itu `Utf8` |
| hanya `Timestamp` tanpa zona | `Timestamp(µs, None)` |
| hanya `Timestamp` ber-offset | `Timestamp(µs, "+HH:MM")` bila satu offset di seluruh hasil; selain itu `Timestamp(µs, "UTC")`, berupa instan. Offset asli tetap di store dan tetap terlihat di grid. |
| hanya `Interval` | `Interval(MonthDayNano)` |
| hanya `Text` | `Utf8` |
| hanya `Json` | `Utf8` dengan ekstensi `arrow.json` |
| `Array`, `Row`, `Map`, `Unknown`, atau campuran lain | `Utf8` berisi `to_text`, teks yang sama dengan grid |

Konversi fisik ke logis terjadi di app, per chunk, saat app melayani permintaan chunk helper (§14.4). Chunk yang encoding-nya sudah sama dengan tipe logis lolos tanpa salinan, dan itu kasus umum. Chunk `null` menjadi array bertipe yang semuanya NULL. Relabel zona dan penskalaan desimal lewat `arrow_cast`. Chunk bertag di-decode lalu dibangun ulang. Kolom yang jatuh ke `Utf8` tidak pernah kehilangan data: SQL melihat teks yang sama dengan pengguna, dan tetap bisa `CAST`. Pilihan lain, yaitu sel yang tidak cocok dijadikan NULL diam-diam, ditolak.

Nama logis adalah nama kolom asli. Nama kembar diberi akhiran `_2`, `_3`, dan seterusnya, dan nama kosong menjadi `column_<i>`. Pemetaannya dikembalikan ke Swift lewat `register_result` supaya UI bisa menunjukkannya.

### 5.5 Membaca: Arrow → `Value` dan teks

- `qh_columnar::value_at(array, enc, i) -> Value` adalah kebalikan persis §5.1. Dipakai `values(range)` (tes, diagnostik), kunci sort bertipe, dan `from_arrow`.
- Jalur jendela tidak membangun `Value` untuk `text`/`json` (meminjam `&str`) dan `bytes` (`render::hex_encode`). Untuk encoding lain ia membangun `Value` di stack lalu memanggil `render::to_text` (§11.2). Renderer tetap satu.

### 5.6 Arrow dari luar: keluaran DataFusion, CSV, Parquet (W13-T8a, di helper)

`qh_columnar::from_arrow(batch) -> SealedChunk` menormalkan tipe Arrow apa pun ke encoding §5.1 dengan tabel tetap. Ia berjalan di helper, di belakang fitur `from-arrow` milik `qh-columnar` (yang menyalakan `arrow-cast`), dan hanya helper yang menyalakan fitur itu. App hanya menerima encoding §5.1 dan memvalidasinya seperti rekaman spill (§6.3), jadi app tidak menaut `arrow-cast` dan tidak pernah memproses tipe Arrow di luar tabel §5.1. Tabel itu diuji terhadap setiap `DataType` yang bisa dihasilkan SQL DataFusion dan pembaca CSV/Parquet-nya.

| Masuk | Menjadi |
|---|---|
| `Int8/16/32`, `UInt8/16/32` | `i64` / `u64` |
| `Float16/32` | `f64` lewat teks desimal terpendek, supaya `0.1f32` tampil `0.1`, bukan `0.10000000149011612` |
| `Utf8View`, `LargeUtf8` | `text` (`Utf8`) |
| `BinaryView`, `LargeBinary`, `FixedSizeBinary` | `bytes` |
| `Timestamp(s/ms/ns, zona)` | `ts`/`tsz` dalam mikrodetik. `ns` dipotong ke bawah, dan itu dinyatakan di doc modul. Zona bernama (misalnya dari Parquet) diselesaikan per nilai dengan `Tz` Arrow; offset yang tidak seragam menjadi `tagged`. |
| `Date64`, `Time32`, `Time64(ns)`, `Duration` | `ts` / `time` / `interval` (`Duration` → `Interval(0, 0, µs)`) |
| `Decimal32/64` | `dec` |
| `Decimal256` | `dec` bila muat `i128`, selain itu `text` |
| `Dictionary`, `RunEndEncoded` | dibuka ke tipe nilainya |
| `List`, `LargeList`, `FixedSizeList`, `Struct`, `Map`, `Union` | `tagged` (`Value::Array`, `Row`, `Map`) |

## 6. Chunk, segel, dan akuntansi

### 6.1 Tipe

```rust
pub struct StoreChunk {
    pub batch: RecordBatch,             // fisik: field c<i>, metadata qh.enc
    pub encodings: Arc<[Encoding]>,     // di-parse sekali saat segel atau decode
    pub flags: ChunkFlags,              // bitmap openable dan numerik per kolom, opsional
    pub bytes: usize,                   // get_array_memory_size() + flags, dihitung sekali
}
pub struct ChunkFlags { pub openable: Vec<Option<BooleanBuffer>>, pub numeric: Vec<Option<BooleanBuffer>> }
```

`ChunkFlags` sengaja di luar `RecordBatch`. Batch hanya berisi data, jadi DataFusion dan IPC tidak pernah melihat kolom tersembunyi. Flag adalah turunan tampilan milik grid (§7).

### 6.2 Aturan segel

Tetap **satu push, satu segel.** Setiap `push` menyegel semua baris batch itu sebelum kembali. Batch dipecah hanya bila melewati 65.536 baris atau perkiraan 2 MiB. Akibatnya tetap sama dengan W2-A3:

- batch pertama (200 baris) langsung terlihat, jadi TTFR S2 tidak butuh aturan khusus;
- stream yang menetes terlihat begitu batch-nya tiba, tanpa timer segel;
- jumlah chunk dibatasi jumlah batch: sekitar 310 chunk untuk 5 juta baris dengan fetch adaptif. Untuk `wide_500k` (± 590 B per baris di Arrow, §2.4), batas 2 MiB berarti sekitar 3.550 baris per chunk, jadi sekitar 140 chunk.

Batas 2 MiB menjaga cache miss sinkron atas chunk yang tumpah: dekripsi plus decode IPC ≤ 2 MiB (§2.5). Kompaksi tidak dibangun dan ditinjau bila jumlah chunk > 4.096 terukur (R-9).

### 6.3 Validasi dan decode

Batch yang dibangun builder divalidasi oleh `RecordBatch::try_new`. Batch dari spill di-decode dengan `arrow_ipc::reader::StreamDecoder` dengan validasi menyala (API aman; `with_skip_validation` bersifat `unsafe` dan tidak dipakai). Metadata `qh.enc` diperiksa terhadap `DataType`: `enc` yang tidak dikenal atau tidak cocok menjadi `StoreError::Corrupt`. Blob bertag di-decode dengan `Reader` yang ada, yang tidak pernah panic. Setiap pelanggaran menjadi `StoreError::Corrupt`.

### 6.4 Akuntansi memori

Yang dihitung: Σ `StoreChunk::bytes` untuk chunk resident, ditambah cache chunk yang sudah didekripsi, ditambah reservasi view dan sewa helper analitik. `bytes` memakai `RecordBatch::get_array_memory_size()`, yang menghitung kapasitas buffer, jadi keluaran builder di-`shrink_to_fit` sebelum segel supaya kapasitas cadangan builder tidak ikut dihitung. Builder hanya hidup selama satu `push` (≤ 2 MiB per store yang sedang streaming) dan tidak dihitung. Itu dinyatakan di doc modul sebagai ongkos tetap per store aktif.

## 7. Bitmap openable dan numerik

### 7.1 Openable, port persis `GridValue.isOpenable(value:type:)`

1. NULL atau teks tersimpan `""` → tidak openable.
2. Dari nama tipe kolom (`ColumnMeta::type_name`, di-lowercase), dihitung **sekali per kolom**:
   - *structured*: berakhiran `[]`, atau memuat `array`, `map(`, `row(`, atau `json`;
   - *binary*: memuat `bytea`, `blob`, atau `binary`.

   Bila salah satu benar, flag `openable_by_type` menyala dan setiap sel yang bukan NULL dan tidak kosong openable. Tidak ada bitmap untuk kolom seperti ini.
3. Selain itu, openable bila **ketiganya** benar:
   - karakter pertama setelah trim `White_Space` adalah `{` atau `[`. `CharacterSet.whitespacesAndNewlines` = Zs ∪ {U+0009–U+000D, U+0085, U+2028, U+2029}, yang sama persis dengan `char::is_whitespace` Rust;
   - panjang UTF-16 ≤ 100.000 (`GridValue.parseLimit`). Jalan pintas: panjang byte ≤ 100.000 pasti lolos, dan panjang byte > 300.000 pasti gagal;
   - `json::validate(text)` lulus. Ini validator iteratif tanpa pohon dan tanpa alokasi per node, di atas **teks yang tidak di-trim**, seperti `JSONSerialization` membaca `text.data(using: .utf8)`. Aturannya: grammar RFC 8259; whitespace JSON hanya spasi, tab, LF, dan CR; fragmen top-level boleh; kedalaman tidak dibatasi selain oleh panjang input; bilangan diperiksa sintaksnya saja tanpa rentang; escape string `\" \\ \/ \b \f \n \r \t \uXXXX`; karakter kontrol < 0x20 tanpa escape ditolak.

   Titik yang belum pasti di `JSONSerialization` (surrogate tunggal dalam `\u`, BOM, eksponen raksasa, kunci ganda, sampah di ekor) dikunci oleh `openable.json` (§15). Validator menyesuaikan diri dengan fixture, bukan sebaliknya.

Dihitung saat segel untuk sel chunk-kolom `text`, `json`, dan `tagged`, atas `to_text`-nya. Pemeriksaan karakter pertama yang murah dijalankan lebih dulu, jadi parse hanya terjadi pada sel yang tampak seperti objek atau array. Bitmap hanya disimpan di `ChunkFlags::openable` bila ada bit yang menyala. Sel bertipe tetap tidak pernah openable kecuali lewat `openable_by_type`, dan itu sama dengan Swift: `"42"` pada kolom `bigint` tidak openable, `5` pada kolom bertipe `json` openable.

Flag ini selalu tentang nilai **tersimpan**. Sel yang punya edit staged dinilai Swift dengan `GridValue.isOpenable` atas teks staged (D-9).

### 7.2 Numerik, semantik `GridSort.number(text) != nil`

| Encoding | Numerik |
|---|---|
| `i64`, `u64`, `dec` | ya, untuk setiap sel bukan NULL |
| `f64` | `swift_plain_number(to_text)`, sama seperti teks. "Finite" saja tidak cukup: `to_text` non-finite adalah `nan`/`inf`/`-inf`, dan `1e300` lolos sintaks tetapi di luar rentang eksponen `Decimal` Swift (−128…127), sehingga Swift memperlakukannya sebagai teks. |
| `bool`, `date`, `time`, `ts`, `tsz`, `interval` | tidak (teksnya memuat huruf, `-`, `:`, atau spasi) |
| `text`, `json`, `tagged`, `bytes`\* | `swift_plain_number(to_text)` |

`swift_plain_number` (di `collate.rs`) adalah port `GridSort.number`:

1. Trim `CharacterSet.whitespaces`: kategori Zs ditambah TAB, **tanpa** baris baru.
2. Harus tidak kosong. Tanda `+`/`-` opsional di depan.
3. Lalu digit ASCII dengan paling banyak satu `.`, dan minimal satu digit.
4. Opsional `e`/`E`, tanda opsional, lalu ≥ 1 digit. Setelah itu tidak boleh ada apa pun.
5. Nilainya harus terwakili oleh `Decimal` Swift. Batas eksponennya dikunci `number.json`.

Swift memakai `Character.isNumber`, yang juga menerima digit non-ASCII. Apakah `Decimal(string:)` lalu menerimanya ditentukan fixture. Bila tidak, hasilnya sama. Bila ya, masuk daftar divergensi.

\* `bytes` dirender sebagai hex, dan hex yang hanya berisi digit (`"0123"`) lolos `swift_plain_number`. Maka `bytes` diperlakukan seperti `text` atas hex-nya, sama dengan Swift yang melihat teks tersebut.

Bitmap `ChunkFlags::numeric` hanya ada untuk `text`, `json`, `tagged`, dan `bytes`. Untuk `f64`, flag dihitung saat dibaca dari teks yang memang sedang dirender. Pemakainya dua: pembangun kunci sort (sel non-numerik langsung ke kunci teks, sel numerik di-parse menjadi `NumKey`) dan flag `NUMERIC` di jendela (untuk `CellText` di seam Fase 5). **Perataan kanan tetap per tipe kolom** (`isNumeric(type)`) demi paritas visual.

## 8. Statistik lebar kolom

- `StoreShared::head_widths: Mutex<Vec<u32>>` diperbarui penulis selama baris ke-0…199 masuk (urutan kedatangan, sama dengan `preview.rows.prefix(200)` hari ini). Untuk setiap sel: NULL → 4; selain itu `min(graphemes(to_text(v)), 256)` lewat `UnicodeSegmentation::graphemes(s, true)` (extended), yang berhenti menghitung di 256. Nilai per kolom adalah maksimumnya.
- Kenapa 256 aman: rumus `min(max(n × 7,2 + 20, 84), 320) + 22` jenuh di n ≥ 42 (42 × 7,2 + 20 = 322,4 > 320). Batas berapa pun ≥ 42 memberi lebar yang sama.
- `column_widths()` mengembalikan salinan vektor itu. Swift menghitung `max(label.count, widths[source])` dengan label yang mungkin sudah diganti nama, lalu menerapkan rumus dan pembagian slack yang ada (D-10).
- Swift meminta ulang hanya pada batch pertama, saat `fetched` melewati 200, dan saat hasil lengkap. Tidak per frame dan tidak per body.
- Yang dihitung adalah teks **tersimpan**, bukan teks terformat, sama dengan `value.count` hari ini.
- Risiko versi Unicode antara `unicode-segmentation` dan runtime Swift dikunci `width.json` (R-11).

- **Disegarkan W6-A1 (2026-10-06).** Sebagaimana dibangun, NULL dihitung 0, bukan 4, dan itu tidak mengubah lebar (TM-8). Komentar `column_widths()` di `store_api.rs` yang menyebut UTF-16 salah: yang dihitung grapheme.

## 9. `StoreRegistry`, identitas store, anggaran global, dan urutan spill

### 9.1 Tipe

```rust
pub struct StoreConfig {                // menggantikan StoreConfig per store hari ini
    pub budget_bytes: usize,            // 256 MiB (O-12)
    pub low_water_bytes: usize,         // budget_bytes - 32 MiB
    pub spill_dir: Option<PathBuf>,     // None: spill mati (tes, pemakaian tanpa app)
    pub chunk_max_rows: u32,            // 65_536
    pub chunk_target_bytes: usize,      // 2 MiB
    pub decoded_cache_chunks: usize,    // 8 (performance-plan.md §10 butir 1)
    pub query_share_bytes: usize,       // budget_bytes / 2: plafon satu sewa helper (§14.5, ditambahkan W13-T8b)
}

pub struct StoreRegistry { /* config, cipher, live: Mutex<HashMap<StoreId, Weak<StoreShared>>>, next_id, resident, query_reserved, clock, decoded */ }
#[derive(Clone, Copy, PartialEq, Eq, Hash)] pub struct StoreId(pub u64);   // dari AtomicU64, tidak pernah dipakai ulang
pub struct StoreHandle { /* id, Arc<StoreShared>, Arc<StoreRegistry> */ }   // token pemilik; Drop = release
#[derive(Clone)] pub struct StoreWriter { /* Arc<StoreShared>, Arc<StoreRegistry> */ }

impl StoreRegistry {
    pub fn new(config: StoreConfig) -> (Arc<Self>, SweepReport);   // buat dir 0700, sapu yatim
    pub fn create(self: &Arc<Self>) -> StoreHandle;                  // kolom menyusul lewat writer.begin
    pub fn from_text_rows(self: &Arc<Self>, columns: Vec<ColumnMeta>, rows: Vec<Vec<Option<String>>>)
        -> Result<StoreHandle, StoreError>;
    pub fn synthetic(self: &Arc<Self>, rows: u32, columns: u32, seed: u64) -> Result<StoreHandle, StoreError>;
    pub fn stats(&self) -> RegistryStats;
}

impl StoreWriter {
    pub fn begin(&self, columns: Vec<ColumnMeta>) -> Result<(), StoreError>;   // sekali
    pub fn push(&self, batch: &ColumnBatch) -> Result<(), StoreError>;         // segel seluruh batch (§6.2), bisa evict
    pub fn push_chunk(&self, chunk: SealedChunk) -> Result<(), StoreError>;    // W7-T1 dan from_arrow: batch Arrow yang sudah jadi
    pub fn finish(&self, outcome: Outcome) -> Result<(), StoreError>;          // Complete{truncated} | Cancelled | Failed
    pub fn rows(&self) -> u32;
}
```

### 9.2 Identitas store dan handle basi

**Koreksi AR W2-A3: tanpa slab slot.** Rancangan awal memakai slab `(slot, generation)` dengan free list. Itu idiom untuk handle berupa indeks. Di sini handle tidak pernah berupa indeks: `ResultHandle` memegang `Arc<StoreShared>` miliknya sendiri, jadi pemakaian ulang slot tidak mungkin membuat handle lama menunjuk store baru, dan use-after-free lintas FFI mustahil secara konstruksi (UniFFI memegang `Arc`, `window` mengembalikan salinan `Vec<u8>`, tidak ada pointer pinjaman yang menyeberang).

- `StoreId(u64)` diambil dari `next_id: AtomicU64` milik registry dan tidak pernah dipakai ulang selama umur proses. Ia berperan sebagai "generation" di `performance-plan.md` §10 butir 3 dan NFR-S3: satu nilai yang unik per store, bukan pasangan slot dan generation.
- Registry menyimpan `live: Mutex<HashMap<StoreId, Weak<StoreShared>>>` hanya untuk evict lintas store dan `stats()`. `release` menghapus entrinya.
- Setiap operasi publik memeriksa `phase != Released`. Bila sudah dilepas, hasilnya `StoreError::Released`, yang menjadi `StaleHandle` di FFI. Ini jawaban `rust-engine-blueprint.md` §2.6 untuk handle basi setelah tab ditutup.
- `StoreId` ikut di AAD spill (§10.3), sehingga rekaman satu store tidak pernah terautentikasi untuk store lain. Header jendela tidak membawanya: cache halaman Swift dimiliki satu `StoreRows`, yang memegang satu handle (§17.2).

### 9.3 Indeks chunk dan publikasi

- `chunks: RwLock<Vec<ChunkEntry { first_row: u32, rows: u32, cell: Arc<ChunkCell> }>>`, dengan `ChunkCell { residency: Mutex<Residency>, last_access: AtomicU64 }` dan `Residency = Resident(Arc<StoreChunk>) | Spilled { offset: u64, len: u32, nonce: u64, bytes: usize }`.
- `stats: Mutex<Vec<ColumnStats>>` (§5.4) diperbarui penulis saat segel, di luar kunci indeks.
- Penulis menyegel chunk di luar kunci, mengambil kunci tulis hanya untuk `push` entri, melepasnya, lalu `rows.store(total, Release)`.
- Pembaca melakukan `rows.load(Acquire)`, mengambil kunci baca, menemukan chunk lewat `partition_point` atas `first_row` (seperti `batch_index_for` hari ini), meng-clone `Arc` yang perlu, lalu melepas kunci.
- `phase: AtomicU8` bernilai `Empty → Streaming → Complete | Cancelled | Failed → Released`.

### 9.4 Anggaran

- `resident: AtomicUsize` menghitung chunk resident, cache dekripsi, reservasi view, dan sewa helper. `query_reserved: AtomicUsize` adalah bagian sewa di dalamnya.
- `charge(bytes, origin)`: bila hasilnya > anggaran dan asalnya penulis atau view, `evict_until(low_water)` dijalankan sinkron di thread pemanggil. Thread penulis adalah thread FFI yang menjalankan run, jadi bukan main.
- **Main thread tidak pernah menulis ke disk.** Cache dekripsi (paling banyak 8 chunk × 2 MiB = 16 MiB) punya jatah tetap di dalam anggaran dan hanya membuang entrinya sendiri.
- `reserve(bytes) -> Reservation` untuk scratch view. Ia meng-evict untuk memberi ruang, atau gagal dengan `BudgetExceeded { needed, budget }`. `Drop` pada reservasi mengembalikan anggaran.
- `reserve_query(max) -> QueryLease` untuk helper analitik (§14.5), ditambahkan W13-T8b bersama pemakainya. Ia meng-evict store yang menganggur, lalu memberi sewa sebesar yang tersedia sampai `min(max, query_share_bytes)`, atau gagal dengan `BudgetExceeded` bila kurang dari 32 MiB. Evict-nya berjalan sinkron di thread klien helper, tidak pernah di main. `Drop` pada sewa mengembalikan anggaran, dan klien menjatuhkan sewa saat query selesai, dibatalkan, atau helper mati.

### 9.5 Urutan evict (LRU)

`last_access` adalah jam logis registry (`AtomicU64`), diperbarui saat chunk disegel, dibaca jendela, atau dipindai view. `evict_until(target)`:

1. Buang entri cache dekripsi, yang tertua dulu. Tanpa I/O.
2. Kumpulkan chunk resident dari semua store hidup, kecuali chunk yang sedang dipinjam (`Arc::strong_count > 1`: sedang dibaca jendela, disemat sort, atau sedang dikirim ke helper). Urutkan menurut `last_access` naik, lalu tumpahkan satu per satu sampai `resident ≤ target`. **Kunci `residency` tidak dipegang selama serialisasi IPC, enkripsi, dan `pwrite`** (koreksi AR W2-A3): evictor meng-clone `Arc` di bawah kunci, melepasnya, menyegel dan menulis, lalu mengambil kunci lagi hanya untuk menukar `Resident` menjadi `Spilled`. Tanpa ini, jendela di main yang menyentuh chunk itu menunggu I/O ~2 MiB, dan D-3 ("pembaca tidak pernah menunggu penulis") batal. Pemeriksaan `strong_count` bersifat heuristik: pembaca yang meng-clone tepat sesudahnya tetap aman, hanya akuntansinya sesaat lebih rendah dari kenyataan.
3. Bila spill mati atau gagal, galatnya kembali ke pemanggil `charge`.

Akibatnya, yang tumpah lebih dulu adalah store dasar Batch 7 yang menganggur, tab di latar, dan chunk tua dari hasil yang sedang streaming. Halaman yang sedang dilihat tidak. Sewa helper tidak pernah di-evict. Di dalam sewanya, operator helper yang bisa spill menumpahkan datanya lewat spill helper (§14.8), dan yang tidak bisa gagal dengan pesan anggaran. Pemindaian O(jumlah chunk) dibayar sekali per putaran evict, dan histeresis 32 MiB (~16 chunk) mengamortisasinya.

### 9.6 Release

`registry.release(key)` bersifat idempoten:

- menandai `Released`;
- mengosongkan indeks chunk dan mengembalikan anggaran;
- menutup fd spill, sehingga kernel membebaskan blok;
- menyalakan flag cancel view yang sedang berjalan;
- membuat permintaan chunk helper berikutnya atas store ini dijawab "the result was closed", sehingga query-nya berakhir dengan galat itu;
- menghapus entri `live`.

`StoreHandle::drop` memanggil `release`. Pendaftaran di `AnalyticsSession` (§14.4) tidak memegang `StoreHandle`, hanya `StoreReader` yang mengecek fase, jadi tabel terdaftar tidak menahan store tetap hidup setelah tab ditutup. `register_result` tidak menyimpan `Arc<ResultHandle>` yang diterimanya. `StoreWriter` yang masih dipegang run mendapat `Released` pada push berikutnya, dan loop pump memperlakukannya sebagai cancel. Pembaca yang masih memegang `Arc<StoreChunk>` selesai dengan aman, dan memorinya bebas sesudahnya.

### 9.7 Sapuan startup

`StoreRegistry::new`, bila `spill_dir` diisi:

1. `symlink_metadata` diperiksa **lebih dulu**, karena `set_permissions` mengikuti symlink. Bila path itu symlink atau bukan direktori, spill dimatikan dengan alasan tertulis.
2. Bila belum ada, direktori dibuat dengan `DirBuilder::new().recursive(true).mode(0o700)`. Bila sudah ada, `set_permissions(0o700)`.
3. Untuk setiap entri `read_dir`: berkas biasa atau symlink di-`remove_file` (tidak pernah diikuti, tidak pernah rekursif). Subdirektori dibiarkan.

Peninggalan `$TMPDIR/queryhive-spill/spill-*.bin` dari store hari ini **tidak** disapu (koreksi AR W2-A3): crate itu tidak pernah dipakai build produk mana pun (fakta 1), jadi berkas seperti itu hanya bisa berasal dari `cargo test` di mesin pengembang. Direktori spill helper (`…/QueryHive/spill-analytics`, §14.8) dibuat dan disapu `configure_analytics` dengan aturan yang sama, sebelum helper pertama dijalankan.

Sapuan aman walau ada instance app lain yang hidup. Berkas mereka sudah di-unlink, dan bila sapuan menang balapan di jendela antara `open` dan `unlink`, akibatnya sama dengan unlink mereka sendiri (`ENOENT` di pihak mereka diabaikan). `SweepReport { removed }` dicatat host ke log.

## 10. Spill terenkripsi dan siklus hidup kunci (NFR-S3)

### 10.1 Kunci

- 32 byte dari `ring::rand::SystemRandom::new().fill()` → `UnboundKey::new(&AES_256_GCM, &bytes)` → `LessSafeKey`. Array asal di-`zeroize` segera sesudahnya. `ring` tidak meng-zeroize jadwal kunci AES di dalam `LessSafeKey` saat drop. Itu diterima dan ditulis di doc modul, karena umur kunci sama dengan proses; dokumen tidak boleh mengklaim kunci "dihapus dari memori".
- Kunci hanya hidup di dalam `LessSafeKey`, di memori proses. Ia tidak pernah ditulis ke disk, dicatat di log, dicetak `Debug` (`SpillCipher` punya `Debug` manual yang menulis `<redacted>`), atau menyeberang FFI.
- Umur kunci sama dengan registry, yaitu `EngineHost`, yaitu proses app. Saat proses berakhir kunci hilang, dan sisa data apa pun tidak bisa dibaca lagi. Sapuan adalah kebersihan, bukan kerahasiaan.
- Bila `SystemRandom::fill` gagal, spill dimatikan. Tidak ada fallback ke kunci tetap.

### 10.2 Nonce

- `counter: AtomicU64` milik `SpillCipher` dimulai dari 1. Setiap segel mengambil `fetch_add(1)`, dan nonce = `[0, 0, 0, 0] ‖ counter.to_be_bytes()` (12 byte).
- Kunci dan penghitung hidup di satu objek, dan penghitung hanya naik, jadi nonce tidak pernah dipakai ulang di bawah kunci yang sama, di store mana pun. Helper punya `SpillCipher` sendiri dengan kunci dan penghitung sendiri (§14.8), jadi tidak ada penghitung yang dibagi lintas proses. Pada `u64::MAX` spill menolak (`SpillUnavailable`).
- Nonce disimpan di indeks memori, bukan di berkas.

### 10.3 AAD dan format rekaman

- AAD (16 byte) = `b"QHS2" ‖ store_id u64 LE ‖ chunk_index u32 LE`. `store_id` tidak pernah dipakai ulang (§9.2), jadi ia memenuhi "id store dan generation" NFR-S3 sekaligus. Tag domain naik dari `QHS1` ke `QHS2` karena format plaintext berubah. Rekaman spill operator di helper memakai tag domain `QHD1` dan kunci helper (§14.8). Tag domain tetap memisahkan jenis rekaman seandainya kelak satu kunci melayani keduanya.
- Rekaman di berkas hanya `ciphertext ‖ tag 16 byte`, disambung di `next_offset` dan tidak pernah ditulis ulang. Berkas tidak punya header dan tidak punya metadata teks biasa sama sekali. Nonce, offset, dan panjang ada di indeks memori.
- **Plaintext satu rekaman:**

  | Offset | Ukuran | Isi |
  |---|---|---|
  | 0 | 4 | magic `"QHP1"` |
  | 4 | 4 | `u32` panjang stream IPC = I |
  | 8 | 4 | `u32` panjang bagian flag = F |
  | 12 | 52 | nol (padding sampai 64, supaya buffer IPC tetap sejajar 16 byte dan decode tidak menyalin) |
  | 64 | I | stream IPC Arrow: pesan skema, satu pesan record batch, EOS. Ditulis `arrow_ipc::writer::StreamWriter` dengan `IpcWriteOptions` bawaan (alignment 64, **tanpa** kompresi IPC). |
  | 64 + I | F | `ChunkFlags`: per kolom satu byte kehadiran (bit0 openable, bit1 numerik), lalu bitmap yang hadir, masing-masing `ceil(rows / 8)` byte |

  Skema ikut di setiap rekaman karena skema fisik boleh berbeda per chunk (D-2). Ongkosnya beberapa ratus byte per rekaman 2 MiB.
- **Segel:** serialisasi chunk ke `Vec` baru (bukan scratch bersama, supaya evict dari dua thread tidak saling mengunci), `seal_in_place_separate_tag(nonce, Aad::from(aad), &mut buf)`, tambahkan tag, `write_all_at(offset)`. Catat `Spilled { offset, len = plain + 16, nonce, bytes }`.
- **Buka:** `read_exact_at`, lalu `open_in_place(nonce, aad, &mut buf)`, lalu `Buffer::from_vec(buf)` dan `StreamDecoder` atas potongan `[64, 64 + I)`. Array hasil decode menunjuk ke buffer yang sudah didekripsi tanpa salinan kedua (diperiksa di §2.5). Hasilnya masuk cache dekripsi sebagai `Arc<StoreChunk>`. Kegagalan autentikasi menjadi `StoreError::SpillAuth { chunk }`. Byte yang tidak terautentikasi tidak pernah sampai ke decoder IPC.
- **Kenapa bukan berkas IPC Arrow utuh per store:** format berkas IPC (footer, blok) tidak punya enkripsi. Mengenkripsi berkas itu sebagai satu kesatuan akan memaksa dekripsi seluruh berkas untuk satu chunk. Rekaman terenkripsi per chunk dengan indeks di memori mempertahankan `pread` satu chunk, yaitu rancangan yang sudah disetujui.

### 10.4 Berkas

- Dibuat saat spill pertama sebuah store: `spill_dir/qhs-<pid>-<store_id>-<16 hex acak>.spill` (helper: `qhd-<pid>-<file_id>-<16 hex acak>.spill` di direktori spill helper), lewat `OpenOptions::new().read(true).write(true).create_new(true).mode(0o600)`. `create_new` berarti `O_EXCL`, jadi path yang sudah ada dan symlink ditolak.
- Segera sesudahnya, **sebelum byte pertama ditulis**, `remove_file(path)`. `ENOENT` (sapuan instance lain menang balapan) dianggap berhasil. Galat unlink lain menutup fd dan menjadi `StoreError::Io`: spill tanpa unlink tidak dijalankan, supaya direktori tetap kosong. `File` tetap terbuka, dan blok dibebaskan saat fd ditutup (release, drop, atau crash). Direktori `~/Library/Caches/QueryHive/spill` (dari Swift, §17.6) karena itu selalu kosong. Karena kosong itu pasti, G-LEAK membuktikan pelepasan lewat `store_stats().stores == 0`, bukan lewat isi direktori.
- Satu fd per store yang pernah tumpah. Berkas spill operator milik proses helper dan tidak dihitung di batas fd app; app hanya menambah tiga fd pipa per helper. Tab di latar tumpah lebih dulu, jadi 100 tab terbuka bisa memegang ~200 fd, dan batas lunak app GUI macOS adalah 256. `EMFILE` diperlakukan seperti disk penuh: chunk tetap resident dan run yang memicunya mendapat satu event `error` (R-19).
- I/O lewat trait `SpillMedium { write_at, read_at, len }` dengan `FileMedium` untuk produk dan `FaultyMedium` untuk tes injeksi `ENOSPC`.
- **Disk penuh:** `ErrorKind::StorageFull` atau `raw_os_error() == Some(28)` menjadi `StoreError::DiskFull`; galat I/O lain menjadi `StoreError::Io`. Chunk tetap resident dan store lain tidak terpengaruh. Run yang memicu evict berakhir dengan satu event `error`: "The result is larger than the memory budget and the disk has no room to spill it. Free some disk space or lower the row limit."

### 10.5 Tes wajib (W4-T3, `crates/qh-result-store/tests/spill.rs` dan unit di `spill.rs`)

| Tes | Isi |
|---|---|
| `no_plaintext_on_disk` | Canary ASCII, UTF-8, dan UTF-16 di kolom teks, canary `i64` di kolom angka, dan canary di kolom bertag. Anggaran kecil memaksa spill. Byte mentah dibaca lewat fd (`SpillFile::read_raw_for_test`, `#[doc(hidden)]`). Canary, magic `"QHP1"`, penanda kelanjutan IPC `FF FF FF FF`, dan string `qh.enc` tidak boleh ditemukan. |
| `a_wrong_key_does_not_open` | Rekaman disegel cipher A lalu dibuka cipher B: `SpillAuth`. |
| `tampering_is_detected` | Satu bit dibalik, dua rekaman ditukar (AAD indeks berbeda), rekaman diputar ulang ke store lain (AAD `store_id` berbeda), dan rekaman diputar ulang dengan tag domain `QHD1`. Semuanya `SpillAuth`, tanpa panic dan tanpa data salah. |
| `nonces_are_never_reused` | 10.000 segel di tiga store dan satu penulis bertag `QHD1` pada cipher yang sama: semua nonce unik. |
| `a_spilled_chunk_reads_back_identical` | Setiap encoding §5.1 dan chunk bertag: `value_at` dan teks jendela sama sebelum dan sesudah spill. |
| `modes_are_0700_and_0600` | Mode direktori lewat metadata, mode berkas lewat `File::metadata()` (fstat pada fd yang sudah di-unlink). |
| `the_spill_directory_stays_empty` | Setelah spill terjadi, `read_dir` kosong. `SpillFile::create` baru mengembalikan fd setelah unlink berhasil. |
| `orphans_are_swept_without_following_links` | Berkas, symlink ke luar direktori, dan subdirektori. Berkas dan symlink hilang, target symlink utuh, subdirektori tetap. |
| `disk_full_is_an_error_not_a_panic` | `FaultyMedium` mengembalikan `ENOSPC`. `push` → `DiskFull`, dan baris resident tetap terbaca. |
| `a_corrupt_record_is_an_error_not_a_panic` | Rekaman yang terautentikasi tetapi isinya rusak (dibuat lewat cipher tes): IPC terpotong, `qh.enc` tidak dikenal, dan blob bertag rusak. Semuanya `Corrupt`. |
| `the_key_is_not_in_debug_output` | `format!("{registry:?}")` tidak memuat byte kunci. |

## 11. `window()`: buffer terkemas dan potongan 256 UTF-16

### 11.1 Layout (little-endian, tanpa padding; Swift membaca dengan `loadUnaligned`)

| Offset | Ukuran | Tipe | Isi |
|---|---|---|---|
| 0 | 4 | `[u8; 4]` | magic `"QHW1"` |
| 4 | 2 | `u16` | versi = 1 |
| 6 | 2 | `u16` | flag global: bit0 `CUT` (1 = jendela tampilan, 0 = `rows_text`), bit1 `COMPLETE` (store tidak akan bertambah), bit2 `VIEWED` (view bukan identitas) |
| 8 | 4 | `u32` | `first_row` (indeks tampilan baris pertama setelah clamp) |
| 12 | 4 | `u32` | `row_count` = R |
| 16 | 4 | `u32` | `column_count` = C, sama dengan jumlah kolom yang diminta, dalam urutan permintaan |
| 20 | 4 | `u32` | `visible_total` (jumlah baris view saat jendela dipotong) |
| 24 | 4 | `u32` | `heap_len` = H |
| 28 | 4 | `u32` | reserved = 0 |
| 32 | 4R | `u32[R]` | `source_rows`: indeks baris store untuk setiap baris tampilan |
| S0 = 32 + 4R | 4(RC + 1) | `u32[RC + 1]` | `text_offsets` relatif ke awal heap; sel k = r × C + c adalah `heap[off[k] .. off[k+1]]` |
| S1 = S0 + 4(RC + 1) | RC | `u8[RC]` | `cell_flags` |
| S2 = S1 + RC | H | `u8[H]` | heap UTF-8 |

**Koreksi AR W2-A3:** gema `slot`, `generation`, `view_id`, dan daftar kolom dihapus dari rancangan awal. Semuanya sudah diketahui Swift dari permintaannya sendiri: handle dimiliki satu `StoreRows`, `view_id` dikirim sebagai argumen (dan view yang tidak cocok menjadi `StaleView`, bukan jendela), dan kolom adalah argumen. Prefetch async membawa `view_id` permintaannya di closure. Gema hanya menambah medan yang harus divalidasi tanpa menangkap kesalahan baru. Layout ini sudah minimum untuk NFR-P8: satu buffer, satu salinan `RustBuffer → Data`, offset `u32` per sel, dan tidak ada string per sel yang di-marshal. Alternatif yang lebih sederhana (misalnya `Vec<Option<String>>` lewat UniFFI) membayar alokasi dan konversi per sel, yaitu 4.096 string per jendela.

Panjang total harus sama dengan `S2 + H`. Swift memeriksa magic, versi, panjang total, dan bahwa offset monoton serta ≤ H sebelum membaca. Pemeriksaannya O(RC) dan murah.

Flag sel:

| Bit | Nama | Arti |
|---|---|---|
| 0 | `NULL` | teks kosong. Grid menggambar gaya NULL (`DataPreferences.nullDisplay`). |
| 1 | `EMPTY` | tersimpan `""` bukan NULL. Grid menggambar `∅`. |
| 2 | `OPENABLE` | §7.1, atas nilai tersimpan |
| 3 | `NUMERIC` | §7.2 |
| 4 | `TRUNCATED` | teks tampilan dipotong di 256 unit UTF-16; nilai penuh lewat `cell_text` |
| 5–7 | — | 0, dicadangkan |

### 11.2 Render satu sel dari array Arrow

1. Temukan chunk lewat `partition_point` atas `first_row`, sekali per baris jendela, lalu ambil encoding kolom dari `StoreChunk::encodings`.
2. Ambil teks per encoding:
   - `text`, `json`: pinjam `&str` dari `StringArray::value(i)`. Untuk varian ini `to_text` adalah identitas (`render.rs:72`, `:83`).
   - `bytes`: hex dari prefiks yang dibutuhkan saja. Jendela terpotong meng-hex paling banyak 128 byte (256 unit) lalu menyalakan `TRUNCATED` bila masih ada sisa; `cell_text` dan `rows_text` meng-hex semuanya. `render::hex_encode` hari ini memanggil `format!` per byte (`qh-core/src/render.rs:369-372`), sehingga blob 10 MB berarti 10 juta alokasi untuk satu sel. `write_text` (D-6) menulis hex dari tabel tanpa alokasi.
   - `bool`, `i64`, `u64`, `f64`, `dec`, `date`, `time`, `ts`, `tsz`, `interval`: bangun `Value` di stack dari nilai primitif (offset `tsz` dari zona kolom, di-parse sekali per chunk), lalu `qh_core::render::to_text`.
   - `tagged`: `tagged::decode`, lalu `to_text`.
   - validitas 0: flag `NULL`, teks kosong.

   Satu tes memastikan bahwa untuk setiap encoding, teks jendela sama dengan `to_text(value_at(…))` dan sama dengan `to_text` nilai asli sebelum masuk store. Renderer tetap satu.
3. Flag dibaca dari validitas dan `ChunkFlags`.
4. Format kolom diterapkan (§11.4).
5. Mode tampilan: potong (§11.3). Mode penuh (`rows_text`, `cell_text`): tidak dipotong.
6. Tulis ke heap.

### 11.3 Potongan layout 256 UTF-16

- Jalan pintas: panjang byte ≤ 256 berarti tidak perlu dipotong, karena jumlah unit UTF-16 ≤ jumlah byte UTF-8.
- Selain itu, ambil prefiks terpanjang yang berakhir di batas extended grapheme cluster dengan panjang UTF-16 ≤ 256. Bila grapheme pertama saja sudah > 256 unit (zalgo), potong di batas `char` terakhir yang muat. Surrogate pair tidak pernah terbelah, karena pemotongan terjadi di batas `char`.
- Tanpa elipsis: Fase 5 menggambar token pemotongnya sendiri. Flag `TRUNCATED` menyala.
- Baris baru dibiarkan apa adanya. Cara menggambarnya dalam satu baris adalah urusan Fase 5.

### 11.4 Port `ColumnFormat` (`render.rs` di crate store)

| Format | Aturan (port persis) |
|---|---|
| `Raw` | teks tersimpan |
| `Text` | Bila tipe kolom *binary* (§7.1): untuk sel `Bytes` bertipe, pakai byte mentahnya langsung (setara dengan jalan hex bolak-balik). Untuk sel teks, port `HexDump.decodedHex`: buang prefiks `\x`/`\X`/`0x`/`0X`; jumlah `Character` harus genap; semua scalar hex; bila gagal, pakai byte UTF-8 teks. Hasilnya `String::from_utf8_lossy`, yang aturan penggantian maximal-subpart-nya sama dengan `String(decoding:as:)` (dikunci fixture). Tipe lain: teks tersimpan. |
| `Uuid` | Semua karakter harus hex digit atau `-`. Hex digit versi Swift juga mencakup bentuk fullwidth U+FF10–FF19, U+FF21–FF26, dan U+FF41–FF46. Digit = lowercase lalu filter hex; bila tepat 32, tulis 8-4-4-4-12; selain itu teks tersimpan. |
| `UnixTimestamp` | Trim `White_Space`, lalu `swift_double`. Harus finite. Detik = n / 1000 bila \|n\| ≥ 1e11, selain itu n. Pecahan dibuang ke bawah (floor; dikunci fixture untuk nilai negatif). Tanggal lewat `qh_core::render::civil_from_days`, format `yyyy-MM-dd HH:mm:ss` UTC. Tahun < 1 dan > 9999 mengikuti fixture. Bukan bilangan: teks tersimpan. |
| `Json` | Panjang UTF-16 ≤ 100.000, parse (`serde_json::Value`), lalu cetak meniru `JSONSerialization` `[.prettyPrinted, .fragmentsAllowed, .sortedKeys]`: indentasi dua spasi, `"key" : value`, `/` di-escape menjadi `\/`, non-ASCII apa adanya, urutan kunci dan bentuk bilangan sesuai fixture. Lalu `\n` → spasi. Gagal parse: teks tersimpan. **Urutan kunci ditulis eksplisit oleh printer**, bukan diwarisi dari `serde_json::Map`: workspace menyalakan `serde_json` fitur `preserve_order` (`Cargo.toml:52`), dan feature unification membuat `Map` di crate ini juga menjaga urutan input. |

**Pintu darurat `Json`, diputuskan W6-A1.** **Disegarkan W6-A1 (2026-10-06).** Syaratnya terpenuhi, dan dengan kuat: W4-T4 tidak menguji paritas format sama sekali (TM-10), jadi printer `Json` di `render.rs` belum terbukti sama dengan `ColumnFormat.render`. Bentuknya, tanpa parameter FFI baru: `StoreRows` menyimpan himpunan `swiftRenderedFormats`, awalnya `{.json}` (D-24). Halaman untuk kolom di himpunan itu dibaca lewat `rows_text` (teks penuh, `CUT` mati, batas 64 MiB; `TooLarge` membagi dua baris sampai satu), dan `ColumnFormat.render` dijalankan di Swift hanya untuk sel yang benar-benar digambar. Jadi tidak ada panggilan FFI per sel dan tidak ada potongan 256 untuk kolom itu; kolom berformat lain tidak terpengaruh. Format `Text`, `Uuid`, dan `UnixTimestamp` tetap di Rust, tetapi W6-T1 menulis tes kembaran (`ArrayRows` acuan lawan `StoreRows` atas korpus setiap format, termasuk awalan heks `\x`, UUID fullwidth, dan detik negatif); format yang selisih di luar divergensi tercatat (§17.2) ikut masuk `swiftRenderedFormats`, kecuali `Raw`, yang tidak pernah pindah. `Json` kembali ke Rust hanya setelah ada fixture `format.json` dua sisi yang lulus (backlog baru B-21, di luar W6-T1). Pengukuran `window-json` (§2.9) tidak mengubah ini: 1,7 ms per jendela 128 × 8 di Rust tidak digate, dan jalur Swift hari ini membayar ongkos parse yang sama per sel yang digambar.

### 11.5 Batas dan target

- `row_count ≤ 4.096`, `column_count ≤ 1.024`, R × C ≤ 262.144; di luar itu `InvalidArgument`. `rows_text` yang total bytenya > 64 MiB menjadi `TooLarge`, dan Swift memecah permintaannya.
- Target: jendela tampilan 128 baris × 32 kolom dari chunk resident, p99 ≤ 0,5 ms termasuk UniFFI (NFR-P8), untuk format `Raw`, `Text`, `Uuid`, dan `UnixTimestamp`. Angka probe untuk bagian Rust-nya (tanpa UniFFI dan tanpa format kolom) ada di §2.6. Format `Json` mem-parse dan mencetak ulang setiap sel sampai 100.000 unit, jadi satu jendela bisa jauh di atas 0,5 ms. Ia diukur terpisah (`bench_ffi window-json`) dan dicatat, bukan digate, karena pengguna memilihnya per kolom dan jalur Swift hari ini membayar ongkos yang sama per render. Diukur `bench_ffi window` (Rust) dan `StoreWindowBench` (Swift). Bila meleset, jalurnya eskalasi C ABI Fase 8.

**Disegarkan W6-A1 (2026-10-06).** Hasil W5-T2 (§2.9): jendela 128 × 30 bertipe 0,396–0,449 ms p99 di sisi Rust saja, jadi 0,5 ms termasuk UniFFI hanya mungkin untuk halaman yang lebih kecil dari 128 × 32. Halaman Swift karena itu 64 baris × 32 kolom (D-22, sementara), dan `StoreWindowBench` mengukurnya lewat UniFFI, resident dan tumpah. Bila p99 64 × 32 masih di atas 0,5 ms, jalurnya eskalasi C ABI Fase 8 (W8-T2), bukan halaman yang lebih kecil lagi.

## 12. Permukaan FFI

**Disegarkan W6-A1 (2026-10-06).** Kode di §12.1–§12.3 cocok dengan `crates/qh-ffi/src/store_api.rs` dan `host.rs` di `465a0c6` (permukaan FFI tidak berubah sejak `bfb9680`), kecuali yang tercantum di §12.4. Nama Swift hasil UniFFI, batas permintaan, dan perilaku yang dibangun ada di §12.4.

### 12.1 Rekaman, enum, galat (`crates/qh-ffi/src/store_api.rs`)

```rust
#[derive(uniffi::Record)] pub struct ColumnWire { pub name: String, pub type_name: String }
#[derive(uniffi::Record)] pub struct RowCount { pub fetched: u32, pub visible: u32, pub view_id: u64, pub phase: StorePhase }
#[derive(uniffi::Enum)]   pub enum StorePhase { Empty, Streaming, Complete, Cancelled, Failed }
#[derive(uniffi::Record)] pub struct SortSpec { pub column: u32, pub descending: bool }
#[derive(uniffi::Enum)]   pub enum FilterSpec {
    Values { column: u32, values: Vec<Option<String>> },   // None = NULL (pengganti nullToken)
    Text { column: u32, needle: String },
}
#[derive(uniffi::Record)] pub struct ViewSpec { pub sort: Option<SortSpec>, pub filters: Vec<FilterSpec>, pub search: Option<String> }
#[derive(uniffi::Record)] pub struct ViewInfo { pub view_id: u64, pub visible: u32, pub fetched: u32 }
#[derive(uniffi::Enum)]   pub enum CellFormat { Raw, Text, Uuid, UnixTimestamp, Json }
#[derive(uniffi::Record)] pub struct DistinctValues { pub values: Vec<Option<String>>, pub more: bool }
#[derive(uniffi::Record)] pub struct StoreStats { pub stores: u32, pub resident_bytes: u64, pub spilled_bytes: u64,
                                                  pub budget_bytes: u64, pub spill_enabled: bool }
#[derive(uniffi::Record)] pub struct StoreSweep { pub removed: u32, pub spill_enabled: bool, pub reason: Option<String> }

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum StoreFfiError {
    #[error("result handle is stale")]                             StaleHandle,
    #[error("the view changed; the current view is {current}")]    StaleView { current: u64 },
    #[error("a newer view replaced this one")]                     Superseded,
    #[error("sorting waits until every row has arrived")]          Streaming,
    #[error("{message}")] TooLarge { needed_bytes: u64, budget_bytes: u64, message: String },
    #[error("{message}")] Spill { message: String },          // DiskFull, SpillAuth, SpillUnavailable, Io
    #[error("{message}")] InvalidArgument { message: String },
    #[error("{message}")] Corrupt { message: String },
    #[error("internal error: {message}")] Internal { message: String },   // panic yang tertangkap, kunci poisoned
}
```

### 12.2 `ResultHandle`: semua metode throwing

```rust
#[derive(uniffi::Object)]
pub struct ResultHandle { inner: qh_result_store::StoreHandle }

#[uniffi::export]
impl ResultHandle {
    pub fn row_count(&self) -> Result<RowCount, StoreFfiError>;               // load atomik saja, tanpa efek samping (§13.8)
    pub fn columns(&self) -> Result<Vec<ColumnWire>, StoreFfiError>;
    pub fn window(&self, view_id: u64, first_row: u32, row_count: u32,
                  columns: Vec<u32>, formats: Vec<CellFormat>) -> Result<Vec<u8>, StoreFfiError>;
    pub fn rows_text(&self, view_id: u64, first_row: u32, row_count: u32,
                     columns: Vec<u32>) -> Result<Vec<u8>, StoreFfiError>;     // layout §11.1, CUT = 0, Raw
    pub fn cell_text(&self, view_id: u64, row: u32, column: u32,
                     format: CellFormat) -> Result<Option<String>, StoreFfiError>;  // penuh; None = NULL
    pub fn column_widths(&self) -> Result<Vec<u32>, StoreFfiError>;
    pub fn set_view(&self, spec: ViewSpec) -> Result<ViewInfo, StoreFfiError>; // memblokir; dipanggil off-main
    pub fn distinct_values(&self, column: u32, limit: u32) -> Result<DistinctValues, StoreFfiError>;
    pub fn release(&self) -> Result<(), StoreFfiError>;                         // idempoten
}
```

Setiap badan metode dibungkus `guarded(|| …)`: `catch_unwind(AssertUnwindSafe)`, lalu payload panic menjadi `Internal { message }` dan dicatat sebagai bug (ADR-0009). Store di belakang kunci yang poisoned ditandai `Failed`. Karena semua metode throwing, panic yang lolos dari `guarded` pun menjadi galat Swift (`rustPanic`), bukan `try!` yang crash. `view_id` yang tidak sama dengan view aktif menjadi `StaleView { current }`, dan Swift memuat ulang.

### 12.3 `EngineHost` (sambungan ke `host.rs` Fase 2)

```rust
// EngineHost::new() Fase 2 tetap tanpa argumen dan tanpa I/O (fase-2-engine-host.md §3).
pub fn configure_result_stores(&self, spill_dir: Option<String>, budget_bytes: u64)
    -> Result<StoreSweep, StoreFfiError>;              // sekali, off-main: bangun registry dan sapu (§9.7)
pub fn create_result_store(&self) -> Result<Arc<ResultHandle>, StoreFfiError>;
pub fn run_with_store(&self, command: EngineCommand, settings: Vec<Setting>, store: Arc<ResultHandle>,
                      sink: Arc<dyn EventSink>, cancel: Arc<RunCancel>);     // seperti host.run: kegagalan = event
pub fn store_from_rows(&self, columns: Vec<ColumnWire>, rows: Vec<Vec<Option<String>>>)
    -> Result<Arc<ResultHandle>, StoreFfiError>;
pub fn store_stats(&self) -> Result<StoreStats, StoreFfiError>;

// Rust saja, tidak di-export (bench_ffi dan tes), seperti `with_connector` Fase 2.
impl EngineHost {
    pub fn store_synthetic(&self, rows: u32, columns: u32, seed: u64) -> Result<Arc<ResultHandle>, StoreFfiError>;
}
```

- **Amandemen Fase 2 D-1.** `fase-2-engine-host.md` D-1 menyatakan tidak ada metode host yang melempar galat dan tidak ada tipe galat UniFFI baru. Metode store di atas melempar `StoreFfiError`, karena hasilnya nilai (handle, statistik), bukan aliran event. `run` dan `run_with_store` tetap tidak melempar. Dicatat di ADR-0030.

- `run_with_store` hanya menerima `Preview` dan `Explain`. Perintah lain menjadi event `error` usage.
- Ia menimpa `RESULT_SINK=store` di settings, membungkus sink dalam `StoreEmitter { sink, writer }` yang `result_store()`-nya mengembalikan writer, lalu berjalan di runtime host, sama seperti `host.run`.
- Badannya juga di-`guarded`, dan panic menjadi event `error`.
- `EngineHost::new()` di blueprint Fase 2 sengaja tanpa I/O, jadi registry tidak dibangun di sana. `configure_result_stores` membangun `StoreRegistry` (termasuk sapuan) dan mengembalikan `StoreSweep { removed: u32, spill_enabled: bool, reason: Option<String> }`, yang dicatat app ke log. Panggilan kedua menjadi `InvalidArgument`.
- **Tidak ada registry bawaan implisit** (koreksi AR W2-A3). `create_result_store` dan `store_from_rows` sebelum `configure_result_stores` menjadi `InvalidArgument("result stores are not configured")`. Tes dan scene snapshot memanggil `configure_result_stores(None, …)` sendiri (spill mati). Rancangan awal membangun registry bawaan tanpa spill secara diam-diam. Bila app memanggil `configure` di antrean latar dan Run yang dipulihkan sesi lebih dulu membuat store, registry tanpa spill itu menang, `configure` lalu ditolak, dan hasil 5 juta baris gagal di anggaran tanpa sebab yang terlihat. Itu fallback senyap.
- Bila direktori spill tidak aman, registry tetap hidup dengan spill mati. Ingest yang melewati anggaran lalu gagal dengan pesan yang menyebut alasannya.
- `run_with_store` dirutekan seperti `preview`/`explain` di `host.run`, yaitu `Pooled(Query)` (fase-2-engine-host.md §3). Jalur sesi dan reset tidak berubah; yang berbeda hanya emitter-nya.

### 12.4 Sebagaimana dibangun W5-T2 dan ditutup W5-C (`df445fd`, `bfb9680`, `1b59154`)

**Disegarkan W6-A1 (2026-10-06).** Selisih dan fakta yang W6-T1 butuhkan, dibaca dari `store_api.rs`, `host.rs`, `commands.rs`, `tests/store_sink.rs`, dan `app/Generated/QueryHiveFFI/qh_ffi.swift`, di `465a0c6`. `claim_run`, pra-taksiran `rows_text` (`too_large`), `spilled_bytes` yang nyata, dan snapshot view tunggal di `row_count()` datang di `1b59154` (W5-C), bukan di `bfb9680`.

**Batas permintaan** (`store_api.rs`): `MAX_WINDOW_ROWS` = 4.096, `MAX_WINDOW_COLUMNS` = 1.024, `MAX_WINDOW_CELLS` = 262.144, `MAX_ROWS_TEXT_BYTES` = 64 MiB. Tiga batas pertama berlaku juga untuk `rows_text` dan `cell_text`, bukan hanya `window`. Jadi Swift memecah salinan besar menurut sel (4.096 baris × 64 kolom adalah satu panggilan penuh), dan `TooLarge` (64 MiB) dibagi dua. Panggilan yang melewati batas menjawab `InvalidArgument`.

**Nama di Swift (UniFFI):** `ResultHandle` dengan `ResultHandleProtocol: AnyObject, Sendable`. Metodenya: `rowCount() -> RowCount` (`fetched`, `visible`, `viewId`, `phase`), `columns() -> [ColumnWire]`, `window(viewId:firstRow:rowCount:columns:formats:) -> Data`, `rowsText(viewId:firstRow:rowCount:columns:) -> Data`, `cellText(viewId:row:column:format:) -> String?`, `columnWidths() -> [UInt32]`, `setView(spec:) -> ViewInfo`, `distinctValues(column:limit:) -> DistinctValues`, dan `release()`. Semuanya `throws StoreFfiError` (`.StaleHandle`, `.StaleView(current:)`, `.Superseded`, `.Streaming`, `.TooLarge(neededBytes:budgetBytes:message:)`, `.Spill(message:)`, `.InvalidArgument(message:)`, `.Corrupt(message:)`, `.Internal(message:)`). `EngineHost`: `configureResultStores(spillDir:budgetBytes:) throws -> StoreSweep`, `createResultStore() throws -> ResultHandle`, `runWithStore(command:settings:store:sink:cancel:)` (tidak melempar), `storeFromRows(columns:rows:) throws -> ResultHandle`, dan `storeStats() throws -> StoreStats`.

**Perilaku** (dibaca dari `host.rs` dan `commands.rs`, dijaga `store_sink.rs`):

- Satu store, satu run. `run_with_store` memakai `claim_run` atomik (`1b59154`; `store_api.rs:307`, dipanggil di `host.rs:524`). Store yang sudah memegang run menjawab event `error` "this result store already holds a run", store yang dilepas menjawab "the result was closed", dan perintah selain `preview` dan `explain` menjawab "a result store takes only preview and explain". Karena itu Swift membuat store baru untuk setiap run (D-12).
- Bila run berakhir sebelum pump (gagal sebelum baris pertama, atau dihentikan), store yang masih `Empty` atau `Streaming` ditutup `Cancelled` bila dibatalkan dan `Failed` bila tidak, sehingga poll selalu melihat fase terminal. **`phase == .failed` sendiri bukan galat bagi UI**: galat datang dari event `error` dan status keluar.
- Pernyataan tanpa kolom (tulis) mengirim `columns` kosong lalu `done`, tanpa `writer.begin`; store berakhir `Complete` dengan nol kolom dan nol baris. Swift melepasnya seketika dan tidak menggambar grid.
- `StoreTarget`: `fetch_size` 200 → 800 → 3.200 → 12.800 → 16.384 (`STORE_FETCH_MAX`), dipotong sisa batas baris; `progress{rows}` paling sering sekali per 16 ms; `pump_result` memotong pada jumlah baris yang diterima; keluaran NDJSON identik byte (G-GOLDEN). `Cursor::next_chunk` belum ada (W7-T1).
- `rows_text` menaksir ukuran sebelum merender (`1b59154`; `read_window`, `store_api.rs:379`, dengan `limit` yang hanya diberikan `rows_text`: `window` dan `cell_text` tidak diperiksa). Untuk setiap potongan chunk ia menjumlahkan bagian kolom yang diminta dari `get_array_memory_size()` ditambah 8 byte per sel, dan menjawab `TooLarge` lewat `too_large` (`store_api.rs:249`) begitu taksiran melewati `MAX_ROWS_TEXT_BYTES`. Ukuran sebenarnya diperiksa lagi setelah tiap potongan dibangun dan setelah buffer disambung. Permintaan yang pasti terlalu besar gagal sebelum sel pertama dirender, dan Swift tetap membaginya dua baris sampai satu (§17.2).
- `row_count()` membaca satu snapshot view (`1b59154`, `store_api.rs:581-589`): `visible` dan `view_id` selalu milik view yang sama, dan keduanya milik view **baru** sejak `set_view` memasangnya di Rust, sebelum hop ke main. Karena itu `poll()` tidak boleh memakai hitungan dari view yang `viewId`-nya bukan `viewID` yang sedang digambar (§17.2).
- `EngineHost::store_synthetic` (Rust saja, tidak diekspor) membangun store sendiri: kolom k bertipe `bigint`, `double`, atau `text` menurut `k % 3`. `StoreStats.spilled_bytes` nyata sejak `1b59154` (TM-9; `host.rs:397` dari `registry.rs:561`). `StoreFfiError` belum punya `AnalyticsUnavailable` (W13-T8b).

**Selisih bernama terhadap §12.1–§12.3:** `column_widths()` menghitung grapheme dengan NULL = 0 (TM-8), dan `view_id` yang tidak cocok menjawab `StaleView { current }`, bukan jendela. Selebihnya §12.1–§12.3 berlaku apa adanya.

## 13. View di Rust

### 13.1 Pool

`qh_rt::view_pool() -> &'static rayon::ThreadPool`, dibangun sekali (`OnceLock`) dengan `num_threads = cores().performance.max(1)`, `thread_name("qh-view-{i}")`, dan `start_handler(|_| set_thread_qos(Qos::UserInitiated))`. Semua kerja paralel view berjalan di dalam `view_pool().install(…)`. Pool ini tidak dipakai ingest, jadi ingest tidak bersaing dengan dirinya sendiri.

### 13.2 Pipeline `set_view(spec)`

Urutannya sama dengan `displayedRows` (`QueryTab.swift:824-848`): filter (AND per kolom), lalu search, lalu sort.

1. Ambil snapshot indeks chunk dan `fetched`.
2. Bila ada filter atau search, jalankan paralel per chunk. Setiap chunk menghasilkan `Vec<u32>` baris sumber yang lolos, dan hasilnya disambung dalam urutan chunk, sehingga urutan sumber terjaga. Bila tidak ada, kandidatnya `0..fetched`.
3. Bila ada sort: syaratnya fase `Complete` atau `Cancelled`; bila masih streaming, jawabannya `Streaming`. Bangun kunci secara paralel, lalu `par_sort_unstable_by` dengan indeks baris sumber sebagai pemutus seri terakhir. Hasilnya setara sort stabil.
4. Build tidak membaca flag cancel. Flag itu milik `View` yang sudah terpasang: ia dibaca perluasan view streaming (`expand_view`) dan pembacaan `rows` dan `source_rows`, dan dinyalakan oleh `set_view` berikutnya atau oleh `release`. Build yang sedang berjalan tidak bisa dibatalkan, ia selesai dulu.
5. Id view diambil di awal panggilan (`view_seq.fetch_add`, sebelum build), bukan saat menukar. Begitu build selesai, `current_view` ditukar di bawah write lock (`guard.replace`) dan flag cancel milik view yang digantikan dinyalakan. Spec kosong (tanpa sort, tanpa filter, search kosong setelah trim) menghasilkan view identitas tanpa permutasi.

**Rust tidak mengurutkan panggilan `set_view` yang berjalan bersamaan** (diverifikasi terhadap `store.rs` di `4ec7480`; koreksi ronde 2). Yang terpasang adalah build yang selesai paling akhir, apa pun urutan panggilannya, dan yang dibatalkan hanya view yang digantikannya. Panggilan yang lebih lama dan selesai belakangan memasang view-nya (id lebih kecil) di atas yang lebih baru, lalu `set_view` menjawab `Ok` dengan id itu, bukan `Superseded`, sehingga `view_id` dan `visible` bisa mundur. Pemanggil karena itu wajib menjalankan `set_view` satu per satu untuk satu store, dan Swift melakukannya (§17.2). `Superseded` hanya dijawab pembacaan yang lewat `read_window` (`window`, `rows_text`, `cell_text`) yang sudah memegang view saat view itu digantikan atau store dilepas; pembacaan yang tiba dengan id lama sesudah penggantian dijawab `StaleView`. Grid tetap menampilkan view lama sampai yang baru siap.

### 13.3 Kunci sort dan comparator

Urutan naik: `Num` < `Temporal` < `Text` < `Null`. Menurun membalik seluruh urutan, jadi NULL di depan, seperti Swift (`GridSort.swift:73-77`). Seri selalu diputus indeks baris sumber naik, di kedua arah, sama dengan `left.offset < right.offset`.

| Sel | Kunci |
|---|---|
| NULL | `Null` |
| `Int`, `UInt`, `Decimal`, `Float` finite | `Num` |
| `Float` non-finite, `Bool`, `Bytes`, `Interval`, komposit, `Json`, `Unknown`, `Text` | jalur teks atas `to_text`: bila `swift_plain_number`, `Num`; selain itu `Text` (kunci natural) |
| `Date`, `Timestamp` | `Temporal(0, micros instan)`; tanggal = tengah malam |
| `Time` | `Temporal(1, micros)` |

- **`NumKey`** membandingkan nilai desimal eksak: tanda, eksponen yang disesuaikan, lalu mantissa. `-0 == 0`. Float diubah lewat representasi terpendeknya (teks yang juga dilihat Swift), sehingga urutan antar-float sama dengan urutan `f64` dan konsisten terhadap desimal. Presisi mengikuti `Decimal` Swift, 38 digit signifikan. Apakah digit ke-39 dan seterusnya dibulatkan atau dipotong dikunci `number.json`.
- **Nilai bertipe diurutkan menurut nilainya.** Timestamp dengan offset campuran menjadi kronologis (`performance-plan.md` §14 butir 4). Tes diferensial hanya memakai nilai `Text` (§15), jadi paritas Swift diuji atas jalur teks, dan jalur bertipe diuji oleh tes unit Rust sebagai perilaku yang disengaja.
- **Total order wajib.** Sejak Rust 1.81, sort bisa panic bila comparator tidak total. Comparator adalah perbandingan leksikografis atas level-level yang masing-masing total, lalu indeks baris.
- **Seri `Num` dan `Temporal` langsung diputus indeks baris, tanpa byte mentah** (koreksi AR W2-A3). Swift menganggap `"1.0"` dan `"1"` sama (`Decimal ==`, lalu `offset`), sehingga pemutus byte di antaranya akan diam-diam mengubah urutan relatif baris yang sama nilainya. Level byte mentah (§13.4 butir 5) hanya milik kunci `Text`. Tes acak berbenih memeriksa antisimetri, transitivitas atas triple, dan konsistensi prefiks.

### 13.4 Kunci natural (pengganti `localizedStandardCompare`, O-9)

Level-level ini dibandingkan berurutan:

1. **Primer.** Barisan elemen:

   | Kelas | Isi | Urutan di dalam kelas |
   |---|---|---|
   | 0 | akhir string | string yang lebih pendek lebih dulu |
   | 1 | `White_Space` | — |
   | 2 | tanda baca dan simbol, termasuk emoji (bukan huruf, bukan angka) | code point setelah lipat lebar |
   | 3 | run digit ASCII | nilai numerik: nol di depan dibuang, jumlah digit signifikan, lalu digit |
   | 4 | huruf Latin setelah dilipat | huruf dasar |
   | 5 | huruf dan angka skrip lain (Yunani, Kiril, Hangul, Kana, Han, …) | code point; Katakana dipetakan ke Hiragana |

   Pelipatan: NFD (`unicode-normalization`), buang tanda kombinasi (kategori Mn, lewat `unicode-properties`), lowercase penuh, fullwidth ASCII → ASCII, lalu tabel kecil `ß/ẞ → ss`, `æ → ae`, `œ → oe`, `ø → o`, `đ → d`, `ł → l`, `ı → i`.
2. **Sekunder.** Tanda aksen per elemen; yang tanpa aksen lebih dulu.
3. **Tersier.** Huruf kecil sebelum huruf besar.
4. **Kuarterner.** Run digit dengan nol di depan yang lebih sedikit lebih dulu.
5. **Akhir.** Byte UTF-8 mentah (setara `forcedOrdering`), lalu indeks baris.

**Satu fungsi kunci.** `collate::natural_key(s: &str, out: &mut Vec<u8>)` menulis kelima level di atas sebagai satu deret byte yang memcmp-able. Urutannya: level primer (satu byte kelas per elemen; huruf Latin 1 byte; simbol dan skrip lain 3 byte code point big-endian; run digit berupa satu byte jumlah digit signifikan, dengan 255 berarti "≥ 255", lalu digitnya), `0x00`, level sekunder, `0x00`, tersier, `0x00`, kuarterner, `0x00`, lalu byte UTF-8 mentah. Byte kelas selalu ≥ 1, jadi `0x00` di akhir level membuat string yang lebih pendek lebih dulu. Membandingkan dua baris berarti `memcmp` kunci, lalu indeks baris. Tidak ada comparator kedua yang harus sepakat dengan kunci. Fungsi yang sama menjadi UDF `qh_natural(text) -> binary` di helper DataFusion (§14.10), jadi grid dan SQL memakai satu kolasi natural.

**Materialisasi.** Sort membangun kunci per chunk secara paralel ke satu arena per chunk (offset `i32` + byte, bentuk `BinaryArray`). Setelah itu `par_sort_unstable_by` atas `u32` baris membandingkan kunci dengan `memcmp`, lalu indeks. Arena dicadangkan lewat `reserve` sebelum dibangun. Untuk `row-N-c01` (14 byte), kuncinya sekitar 50 byte, jadi sekitar 30 MB untuk 500k baris termasuk offset dan indeks. Bila cadangan tidak cukup, dipakai jalur prefiks: setiap entri hanya menyimpan 16 byte pertama kunci (`u128`) dan baris (20 byte per baris). Seri prefiks lalu diputus dengan menghitung ulang kunci penuh kedua sisi ke buffer per worker. Kedua jalur memakai fungsi yang sama, jadi hasilnya identik, dan tes berbenih membandingkannya.

- **Chunk resident** dipinjam lewat `Arc` yang disemat selama kunci dibangun. Chunk yang disemat tidak bisa di-evict (§9.5).
- **Chunk yang tumpah** didekripsi satu per satu lewat cache dekripsi, kuncinya dibangun, lalu chunk dilepas lagi. Yang dicadangkan hanya arena kunci, bukan teksnya. Bila jalur prefiks pun tidak muat, hasilnya `TooLarge { needed_bytes }`, dan Swift menampilkan "Sorting these rows in memory needs about N MB, more than the result budget allows."
- **Kunci bertipe** (`Num`, `Temporal`) dibaca langsung dari array primitif (`Int64Array::value`, `Decimal128Array::value`, `TimestampMicrosecondArray::value`) tanpa `Value`. Sel `tagged` lewat `value_at`, dan sel teks lewat `swift_plain_number` lalu `NumKey` atau kunci natural.

**Kandidat divergensi**, yang dikonfirmasi atau dibantah fixture lalu didaftar di `differential.rs`:

- urutan antar-skrip (ICU menaruh Hangul sebelum Kana dan Han);
- emoji dan simbol;
- digit non-ASCII dan fullwidth di dalam run numerik;
- pengurutan khusus lokal (misalnya `ch` di cs, `å` di sv);
- urutan tersier ICU yang sebenarnya.

### 13.5 Filter (port `ColumnFilter.matches`)

- `Values { values }`: sel NULL cocok bila `None ∈ values`. Sel lain cocok bila `to_text` sama dengan salah satu nilai, dibandingkan **setelah NFC**, karena kesetaraan `String` Swift menghormati kesetaraan kanonik. `values` kosong tidak cocok dengan apa pun, sama dengan Swift. Token `"\u{0}null"` digantikan `None` di batas Swift. Bedanya dengan Swift: nilai nyata `"\u{0}null"` tidak lagi ikut cocok saat NULL dipilih. Ini divergensi yang didaftar, dan Rust yang benar.
- `Text { needle }`, port `matchesText` baris demi baris:
  1. `needle' = trim(needle, whitespaces)` (Zs + TAB). Bila kosong, semua cocok. Sel NULL tidak cocok.
  2. Untuk `op` dalam urutan `>=`, `<=`, `>`, `<`, `=`: bila `needle'` diawali `op`, maka `operand = trim(sisa, whitespaces)`.
     - Bila `operand` kosong, **keluar dari loop** dan lanjut ke langkah 3 dengan `needle'` utuh.
     - `=` berarti `ci_equal(value, operand)`.
     - Bila `swift_double(value)` dan `swift_double(operand)` keduanya ada, bandingkan sebagai `f64`.
     - Selain itu bandingkan string setelah NFC, dalam urutan scalar. `<` Swift = urutan scalar atas bentuk ternormalisasi.
  3. Selain itu `ci_contains(value, needle')`.
- `swift_double` = `f64::from_str` (desimal, `inf`/`infinity`/`nan` tanpa peduli huruf besar) ditambah parser float heksadesimal (`0x1.8p3`) yang ditulis sendiri, karena `Double(String)` Swift menerimanya. Tidak ada whitespace di depan maupun di belakang. Kasus tepinya dikunci `filter.json`.

### 13.6 Search (port `GridSearch.matches`)

`term' = trim(term, White_Space)`. Bila kosong, tidak ada search. Sebuah baris cocok bila ada sel bukan NULL di **semua** kolom (termasuk yang disembunyikan) yang `to_text`-nya `ci_contains(term')`. Teksnya teks tersimpan, bukan teks terformat.

### 13.7 Case-insensitive yang "localized"

`fold(s)` = NFC, lowercase penuh, lalu tabel pelipatan khusus (`ß → ss`, ligatur U+FB00–FB06, `ς → σ`, `İ → i̇`). `ci_contains(h, n)` = `fold(n)` dicari di `fold(h)`, dan `ci_equal` = kesamaan setelah `fold`.

Jalan pintas ASCII: bila keduanya ASCII, dipakai pencarian ASCII case-insensitive tanpa alokasi. Buffer lipat per worker rayon dipakai ulang.

Kandidat divergensi:

- aturan Turki ketika locale tr;
- kecocokan di dalam cluster kombinasi yang tidak punya bentuk precomposed (Foundation menghormati batas composed character sequence);
- lipatan khusus lokal.

Locale saat ekspor dicatat di manifest fixture.

### 13.8 View selama streaming

Filter dan search boleh diterapkan saat streaming; sort tidak. View filter menyimpan `rows: RwLock<Vec<u32>>` dan `scanned: u32`. **Perluasan dipicu penulis, bukan pembaca** (koreksi AR W2-A3): setelah `push` mempublikasikan chunk, bila view aktif adalah view filter dengan `scanned < fetched`, penulis menjadwalkan paling banyak satu tugas perluasan di `view_pool` (penjaga `AtomicBool`) yang memindai `[scanned, fetched)` lalu menambahkan hasilnya. `set_view` yang selesai saat stream masih berjalan memeriksa sekali lagi, supaya chunk yang terbit di antaranya tidak tertinggal. Dengan begitu `row_count()` tetap load atomik murni seperti di `performance-plan.md` §10 butir 3, dan main thread tidak pernah menjadwalkan kerja. View identitas: `visible == fetched`.

### 13.9 Daftar distinct (port `ColumnFilter.distinctValues`)

`distinct_values(column, limit)` memindai baris yang sudah diambil dalam urutan store, **bukan** view terfilter, sama seperti `preview.rows` hari ini. Ia mengumpulkan himpunan terurut byte (setelah NFC) dan menandai `has_null`. Begitu `(jumlah bukan NULL + has_null) > limit`, pemindaian berhenti dengan `more = true` dan daftar kosong. Selain itu hasilnya `[None bila has_null] + bukan NULL terurut`. Swift memanggilnya dengan `limit = valuePickerLimit + 1 = 11` dan tetap memakai aturannya sendiri: bisa dipilih bila `!more && values.count ≤ 10`.

### 13.10 Target

NFR-P8: sort 500k numerik ≤ 100 ms, teks ≤ 300 ms, off-main. Filter dan search 500k × 30 dicatat (sasaran ≤ 300 ms). Semua diukur di `bench_ffi` skenario `view-*`. Angka probe untuk bentuk kunci ini ada di §2.7.

**Disegarkan W6-A1 (2026-10-06).** Angka W5-T2 untuk 1 juta × 30 dengan 4 thread (§2.9): sort bigint 166–221 ms, sort teks natural 153–182 ms, filter 25–27 ms, search semua kolom 768–796 ms. Ekstrapolasi linear ke 500k memberi 83–111, 76–91, 12,5–13,3, dan 384–398 ms: sort numerik di tepi 100 ms, dan search meleset dari sasaran 300 ms. Gate 500k yang nyata ada di W5-T3 dan W6-T2. Search yang meleset dicatat (bukan gate), dan UI menanganinya sebagai kerja asinkron (§17.2).

## 14. DataFusion: komponen analitik terpisah (W13-T8a–c)

Bagian ini menetapkan letak DataFusion setelah O-18, cara app mendapatkan dan menjalankannya, dan cara helper memakai store, anggaran, serta spill tanpa melemahkan NFR-S3 dan NFR-S5. UI analitik (editor SQL atas hasil, pivot, join antar-hasil, membuka CSV/Parquet) adalah item PRD baru dan tidak dibangun di program ini. Yang dibangun hanya mesinnya, klien di app, dan hook komponen di Settings (W13-T8c).

### 14.1 Letak, crate, dan fitur

`helpers/analytics/` adalah workspace Cargo sendiri: `[workspace]` ada di manifest-nya, dan root `Cargo.toml` mencantumkannya di `exclude`. Isinya satu paket, `qh-analytics`, dengan lib (semua logika, `forbid(unsafe_code)`) dan `[[bin]] queryhive-analytics` (hanya `main`). Ia memakai crate repo lewat path: `qh-core`, `qh-columnar` (fitur `from-arrow`), `qh-result-store` (`collate`, `SpillCipher`, `SpillFile`), `qh-rt` (builder runtime ber-QoS), dan `qh-analytics-proto`.

Protokol ada di crate kecil `crates/qh-analytics-proto`, anggota workspace utama, karena app dan helper sama-sama memakainya. Crate ini hanya bergantung pada `serde`, `serde_json`, `arrow-array`, `arrow-ipc`, dan `thiserror`, jadi tidak membawa DataFusion ke app.

```toml
# helpers/analytics/Cargo.toml
datafusion = { version = "55.1", default-features = false, features = [
    "sql", "parquet", "nested_expressions", "datetime_expressions",
    "string_expressions", "unicode_expressions", "regex_expressions",
] }
```

- Tanpa `compression` (bz2, xz, gzip, zstd untuk **CSV/JSON** terkompresi) dan tanpa `crypto_expressions`, `encoding_expressions`, `recursive_protection`, dan `avro`. Dengan set ini `cargo deny check licenses` lulus terhadap `deny.toml` apa adanya (§24).
- `recursive_protection` tidak dibutuhkan: parser SQL DataFusion membatasi kedalaman rekursi (`datafusion.sql_parser.recursion_limit`, bawaan 50, `datafusion-common-55.1.0/src/config.rs:304`), dan helper tidak membangun pohon ekspresi sendiri. Bila kelak dibutuhkan, ia membawa `ar_archive_writer` (`Apache-2.0 WITH LLVM-exception`, hanya saat build) dan butuh pengecualian lewat ADR (§24).
- `rust-version = "1.94"` hanya di workspace helper (F-2). Workspace utama tetap 1.85.
- Lock sendiri, `helpers/analytics/Cargo.lock`, diawali salinan `Cargo.lock` utama supaya versi yang sudah ada tidak bergeser. Yang menyeberang proses adalah format IPC, bukan tipe Rust, jadi versi arrow di helper tidak wajib sama dengan di app. Keduanya tetap dinaikkan bersama sebagai satu tugas (R-22), karena `qh-columnar` dan `qh-result-store` dikompilasi di kedua workspace.
- `[profile.release]` sama dengan repo (`lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`, `panic = "unwind"`). `[profile.dev] debug = "line-tables-only"` untuk menekan `target` debug (9,5 GB dengan debuginfo penuh, §2.2). Angka barunya dicatat di W13-T8a.
- `.cargo/config.toml` repo berlaku, karena workspace helper ada di dalam pohon repo (invariant #4; `zstd-sys` mengompilasi C).
- `qh-ffi` tidak punya fitur Cargo `analytics`, dan `app/build-ffi.sh` tidak berubah. `object_store` di DataFusion hanya memakai fitur `fs` (`datafusion-execution-55.1.0/Cargo.toml`), dan helper tidak bergantung pada `qh-ffi` atau driver, jadi graf helper tidak memuat klien HTTP (diperiksa G-ANALYTICS).

### 14.2 Pengiriman: unduh, verifikasi, simpan, perbarui, hapus

- **Artefak.** `queryhive-analytics-<CFBundleVersion>-arm64.lzfse`: satu Mach-O arm64 stripped, dikompresi LZFSE, dan dibuka dengan `NSData.decompressed(using: .lzfse)` tanpa dependensi baru. Ia ditandatangani seperti app: ad-hoc secara bawaan; Developer ID dengan hardened runtime dan **tanpa entitlement** bila `build-dmg.sh --developer-id` dipakai; dan dinotarisasi (zip berisi Mach-O ke `notarytool`) bila `--notarize`. Helper Rust murni tidak butuh `allow-unsigned-executable-memory` (ADR-0014), karena tidak ada trampolin FFI.
- **Tempat terbit.** Aset rilis GitHub yang sama dengan DMG dan `appcast.xml`, lewat URL bertag (`releases/download/<tag>/<aset>`), bukan `latest`, karena helper terikat satu build app.
- **Dipatok di app.** `app/build.sh` dengan `ANALYTICS=1` membangun helper, mengompres, menandatangani arsip dengan `sign_update` Sparkle (kunci EdDSA feed yang sama, dari keychain), lalu menulis ke `Info.plist`: `QHAnalyticsURL`, `QHAnalyticsLength`, `QHAnalyticsEdSignature` (atas arsip), dan `QHAnalyticsSHA256` (atas executable setelah dibuka). Kunci publiknya tetap `SUPublicEDKey` (`app/sparkle-public-key.txt`). Satu kunci dipakai untuk dua hal karena keduanya melindungi kode yang dijalankan app, dan menambah kunci kedua hanya menambah rahasia yang harus dijaga. Build tanpa `ANALYTICS=1` tidak punya kunci-kunci ini, dan fitur tampil "tidak tersedia di build ini", bukan gagal diam-diam. `release.sh` selalu membangun dengan `ANALYTICS=1`, mengunggah aset helper di rilis yang sama, dan memeriksa ulang URL-nya setelah rilis naik, sama seperti feed (invariant #10).
- **Izin.** Pertama kali pengguna memakai fitur analitik, app menanyakan izin mengunduh, dengan ukuran dari `QHAnalyticsLength`. Tidak ada unduhan di latar tanpa izin. Setelah diizinkan, unduhan ulang karena app diperbarui tidak bertanya lagi, tetapi tetap menampilkan kemajuan dan bisa dibatalkan.
- **Verifikasi** (Swift, `Support/AnalyticsComponent.swift`, off-main), berurutan. Kegagalan di langkah mana pun menghapus unduhan dan menampilkan alasannya.
  1. Panjang sama dengan `QHAnalyticsLength`, dan unduhan berhenti di panjang itu.
  2. Tanda tangan EdDSA atas arsip valid terhadap `SUPublicEDKey` (CryptoKit `Curve25519.Signing.PublicKey`, Ed25519 yang sama dengan Sparkle). Byte yang belum terverifikasi tidak pernah sampai ke dekompresor.
  3. Dibuka dengan batas 256 MB, lalu SHA-256 executable harus sama dengan `QHAnalyticsSHA256`.
  4. Ditulis ke `queryhive-analytics.partial` (mode `0700`), `fsync`, lalu `rename` atomik ke `queryhive-analytics`.
  5. `SecStaticCodeCheckValidity` lulus. Bila app sendiri bertanda tangan Developer ID, requirement-nya menuntut Team ID yang sama. Build ad-hoc hanya bisa memeriksa integritas tanda tangan, dan asalnya dibuktikan langkah 2 dan 3.

  App tidak memasang atribut karantina pada berkas ini, jadi Gatekeeper tidak menilainya. Kepercayaan datang dari langkah di atas.
- **Tempat simpan.** `~/Library/Application Support/QueryHive/Components/analytics/<CFBundleVersion>/queryhive-analytics`, dengan direktori `0700`. Saat peluncuran, app menghapus direktori build lain di bawah `analytics/`, yaitu sisa versi lama.
- **Pembaruan.** Mengikuti app. Sparkle mengganti app, `Info.plist` baru mematok hash baru, helper lama tidak cocok lagi, dan helper baru diunduh saat fitur dipakai berikutnya. Helper tidak punya jalur pembaruan sendiri.
- **Hapus.** Settings → Analitik → Hapus: `shutdown_analytics`, lalu direktori komponen dihapus dan izin unduh dicabut.
- **Tanpa jaringan.** Settings → Analitik → "Pasang dari berkas…" menerima arsip yang diunduh pengguna dari halaman rilis, dengan verifikasi yang sama.
- **Build debug dan tes.** Hanya di build `DEBUG` dan di tes, `QH_ANALYTICS_HELPER=<path>` menunjuk helper hasil `cargo build`. Hash-nya dihitung saat konfigurasi, dan langkah EdDSA dilewati. Build rilis mengabaikan variabel ini.
- **Sebelum setiap spawn**, Rust membaca ulang executable dan membandingkan SHA-256-nya (`ring::digest`, sudah ada) dengan hash yang diberikan `configure_analytics`. Bila berbeda, helper tidak dijalankan, komponen ditandai rusak, dan Swift menawarkan unduh ulang. Ongkosnya sekali per spawn, off-main, dan diukur di W13-T8b (perkiraan puluhan ms untuk 65 MB).
- **Model ancaman.** Verifikasi menjaga dari unduhan, cermin, atau cache yang dirusak atau salah versi. Proses lain milik pengguna yang sama bisa menukar berkas di antara hash dan `exec` (macOS tidak punya `fexecve`). Itu di luar model, sama seperti proses itu bisa mengubah bundel app sendiri.

### 14.3 Proses dan protokol

- **Satu helper per proses app,** dimiliki `EngineHost`. Ia dijalankan malas oleh `run_sql` pertama dan berhenti setelah 5 menit tanpa query. Pendaftaran tidak menahannya hidup, karena sesi memutarnya ulang (§14.6).
- **Spawn dari Rust** (`std::process::Command`), bukan dari Swift, karena store ada di Rust dan jalur data tidak boleh lewat Swift. Perintahnya `/usr/bin/sandbox-exec -p <profil> -D … <helper> --protocol 1` (§14.9), dengan `env_clear()`, direktori kerja = direktori spill helper, stdin dan stdout berupa pipa ke app, dan stderr berupa pipa ke log app (baris demi baris, panjangnya dibatasi). Tidak ada fd lain yang diwariskan, karena std Rust membuka berkas dengan `O_CLOEXEC`.
- **Handshake.** Pesan pertama helper adalah `Hello { protocol: 1, build }`, dalam 5 detik. Protokol yang berbeda, atau `build` yang bukan build app, membuat helper dihentikan dan komponen ditandai rusak.
- **Bingkai `QHA1`:** `u32 LE panjang` ‖ `u8 jenis` ‖ `u64 LE query_id` ‖ muatan. Panjang ≤ 256 MiB. Panjang lebih dari itu atau jenis yang tidak dikenal adalah pelanggaran protokol, dan helper dibunuh. Jenis `Control` membawa JSON (enum `serde` di `qh-analytics-proto`). Jenis `Batch` membawa satu stream IPC Arrow lengkap (skema, satu batch, EOS), bentuk yang sama dengan isi rekaman spill (§10.3).
- **Pesan.** App → helper: `OpenSession`, `CloseSession`, `RegisterResult { session, name, token, schema, rows, chunks }`, `RegisterFile { session, name, path, format }`, `Deregister`, `RunSql { session, sql, lease_bytes, row_cap }`, `Cancel`, `ChunkData` (jawaban), dan `Shutdown`. Helper → app: `Hello`, `TableInfo`, `ChunkRequest { token, chunk, columns }`, `ResultColumns`, `ResultChunk` (batch), `Progress`, dan `Done { rows, truncated, cancelled, error }`.
- **Thread di app.** Satu thread baca dan satu thread tulis per helper, jadi bingkai tidak pernah disisipkan. Permintaan chunk dilayani di pool blocking runtime engine, tidak pernah di main. Galat dari helper tidak pernah menjadi panic di app: jalur baca memakai `Result`, dan badan UniFFI tetap `guarded()`.
- **QoS.** Helper membangun runtime tokio-nya lewat `qh-rt` (worker sebanyak P-core, `USER_INITIATED`, nama `qh-sql-{i}`), mengikuti pola `view_pool`.

### 14.4 Serah terima data tanpa salinan ganda

- **Pendaftaran.** `register_result(name, handle)` di app mensyaratkan fase `Complete` atau `Cancelled`; hasil yang masih streaming ditolak dengan `Streaming`, sama seperti sort fallback. App menghitung skema logis (§5.4) dari `ColumnStats`, menyimpan `StoreReader` (bukan `StoreHandle`, §9.6) di bawah token acak 128-bit milik sesi itu, lalu mengirim `RegisterResult` dengan skema dan jumlah chunk. Helper membuat `RemoteStoreTable` untuk nama itu.
- **Scan.** `RemoteStoreTable::scan(projection, filters, limit)` mengembalikan rencana dengan `target_partitions` partisi, dan chunk dibagi bergiliran ke partisi. Setiap partisi menarik chunk-nya dengan `ChunkRequest`, paling banyak dua permintaan terbuka per partisi, sehingga memori transit terbatas (4 partisi × 2 × ≤ 2 MiB). `limit` dihormati per partisi. Pushdown filter tidak dibangun (`Unsupported`). Statistik: `num_rows` pasti, ukuran byte perkiraan dari `StoreChunk::bytes`.
- **App melayani `ChunkRequest`.** Token harus milik sesi yang meminta, dan kolom harus ada: helper hanya bisa membaca apa yang didaftarkan kepadanya. Chunk diambil resident (`Arc` disemat) atau didekripsi lewat cache dekripsi, diproyeksikan, diubah ke batch logis (§5.4; tanpa salinan bila encoding sudah sama dengan tipe logis), lalu ditulis sebagai stream IPC langsung ke pipa. Semat dilepas setelah ditulis.
- **Salinan.** Data menyeberang sekali: kernel menyalin dari buffer app ke pipa, lalu dari pipa ke buffer helper. Tidak ada salinan kedua di ruang pengguna. `StreamWriter` menulis buffer array langsung ke pipa, dan helper men-decode tanpa salinan atas buffer yang ia baca (seperti §2.5), dengan validasi menyala.
- **Yang ditolak.** Helper membaca berkas spill app: kunci per proses harus keluar dari proses app, dan chunk resident memang tidak ada di disk. Memori bersama (`mmap` anonim): butuh `unsafe` di kedua sisi dan aturan umur lintas proses, hanya untuk menghemat salinan yang murah. Mengirim seluruh store saat pendaftaran: menggandakan memori dan mengabaikan proyeksi.
- **Throughput** pipa belum diukur. Target W13-T8b: ≥ 1 GB/s untuk chunk 2 MiB (`bench_ffi sql-scan-500k`). Bila meleset, `UnixStream::pair()` (API std yang aman) dengan buffer soket besar menggantikan pipa tanpa mengubah protokol.
- **Keluaran** helper selalu dalam encoding store §5.1, karena `from_arrow` berjalan di helper (§5.6). App memvalidasinya dengan validator rekaman spill (§6.3): `qh.enc` harus dikenal dan cocok dengan `DataType`, dan blob bertag di-decode dengan `Reader` yang tidak pernah panic. Pelanggaran menjadi `Corrupt`, query gagal, dan helper dibunuh.

### 14.5 Anggaran lintas proses: sewa

- Anggaran tetap satu dan milik `StoreRegistry` di app (O-12). Helper tidak menagih apa pun ke registry secara langsung. Ia bekerja di dalam **sewa**.
- Pada `run_sql`, klien memanggil `registry.reserve_query(query_share_bytes)` (§9.4). Registry meng-evict store yang menganggur, lalu memberi sewa sebesar yang tersedia sampai 128 MiB, atau gagal di bawah 32 MiB dengan `TooLarge` ("The result budget is full. Close a tab or lower the row limit."). Besar sewa dikirim di `RunSql`.
- Di helper, `LeasePool: MemoryPool` membatasi total reservasi DataFusion pada jumlah sewa query yang sedang berjalan. `try_grow` yang melewatinya menjawab `ResourcesExhausted`. Operator yang bisa spill (sort, agregasi, sort-merge join) lalu menumpahkan datanya (§14.8), dan yang tidak bisa gagal dengan "This query needs more memory than the result budget allows." Query yang berjalan bersamaan berbagi total sewanya. Keadilan antar-query tidak dijamin, dan itu diterima karena SQL paralel lintas tab jarang.
- Sewa kembali saat `Done`, saat cancel, dan saat helper mati. Sewa tidak pernah di-evict, dan chunk yang sedang dikirim ke helper disemat, jadi query tidak meng-evict datanya sendiri. Batas setengah anggaran (D-4) menjaga grid: query boleh mendorong store yang menganggur ke spill, tetapi tidak bisa mengambil lebih dari 128 MiB dari chunk yang sedang dilihat.
- Yang tidak masuk sewa: batch transit (dibatasi kredit §14.4), struktur kecil DataFusion, dan memori dasar proses helper. Karena itu NFR-P3 untuk skenario SQL diukur sebagai jumlah `phys_footprint` app dan helper (`bench_ffi sql-*`, W13-T8b), dan ambangnya ditetapkan dari angka itu di gate W13 (R-23).
- Keluaran SQL ditulis ke store app biasa, jadi ditagih seperti ingest.

### 14.6 Cancel, crash, dan umur proses

- **Cancel.** `RunCancel` menjadi `Cancel { query_id }`. Helper menjatuhkan stream-nya (DataFusion berhenti di batch berikutnya) dan menjawab `Done { cancelled: true }`. Bila jawaban tidak datang dalam 2 detik, klien membunuh helper (`Child::kill`), dan query lain yang sedang berjalan berakhir dengan galat "the analytics component was restarted".
- **Crash helper.** EOF atau galat baca di thread baca membuat helper ditandai mati dan di-`wait`, jadi tidak ada zombie. Setiap query yang sedang berjalan mendapat satu event `error` ("The analytics component stopped unexpectedly. Run the query again."). Store keluarannya berakhir `Failed`, dan semua sewa dikembalikan. App tidak pernah ikut mati: `SIGPIPE` diabaikan sejak peluncuran (§17.6), sehingga menulis ke pipa yang putus menjadi `EPIPE` biasa. Tiga crash dalam 60 detik mematikan komponen sampai app dijalankan ulang, dengan alasannya di Settings.
- **Pemulihan.** `AnalyticsSession` di app menyimpan pendaftarannya (nama → `StoreReader` atau berkas). Saat helper dijalankan ulang, setelah crash, idle, atau hapus-pasang, sesi memutar ulang `OpenSession` dan pendaftarannya sebelum query berikutnya. Pengguna tidak perlu mendaftar ulang.
- **App mati.** stdin helper tertutup, helper melihat EOF, lalu keluar. Tidak ada helper yatim, dan berkas spill-nya sudah di-unlink.
- **Tutup tab.** `session.close()` mengirim `CloseSession` bila helper hidup. Hasil yang di-release (§9.6) membuat permintaan chunk berikutnya atas hasil itu dijawab "the result was closed".

### 14.7 Sesi dan SQL yang dikunci (di helper)

- `SessionConfig`: `target_partitions` = jumlah P-core (4 di M4; di §2.7, 4 partisi lebih cepat dari 10); `information_schema` mati. `map_string_types_to_utf8view` dibiarkan bawaan, dan keluarannya dinormalkan `from_arrow` di helper (§5.6).
- SQL selalu lewat `sql_with_options` dengan `allow_ddl(false)`, `allow_dml(false)`, dan `allow_statements(false)`. `CREATE EXTERNAL TABLE`, `COPY … TO`, `INSERT`, dan `SET` ditolak setelah rencana logis dibuat dan sebelum apa pun dieksekusi (`BadPlanVisitor`, `context/mod.rs:2326-2356`; `COPY` dihitung DML). `enable_url_table()` tidak pernah dipanggil, jadi SQL tidak bisa membaca path yang tidak didaftarkan app.
- Nama tabel per sesi, jadi `r` di tab A dan `r` di tab B tidak bertabrakan. `RuntimeEnv` dan `LeasePool` satu per helper.
- Kuncian ini dijaga dua kali: `SQLOptions` di helper, dan kurungan kernel (§14.9) yang tetap menolak tulis berkas dan jaringan seandainya kuncian SQL kelak bocor.

### 14.8 Spill operator di helper

`EncryptedTempFiles: TempFileFactory` → `EncryptedSpillFile: SpillFile` (F-4), dipasang lewat `DiskManagerMode::Custom`.

- **Kunci.** `SpillCipher` baru milik proses helper: 32 byte acak dari `ring` saat helper mulai. Kunci ini tidak pernah keluar dari proses helper dan hilang saat helper keluar. Kunci app tidak pernah dikirim ke helper, dan helper tidak pernah membaca spill app.
- **Berkas.** `qhd-<pid>-<file_id>-<16 hex>.spill` di `~/Library/Caches/QueryHive/spill-analytics`, yang dibuat `0700` dan disapu `configure_analytics` sebelum helper pertama dijalankan. `create_new`, `0600`, dan unlink sebelum byte pertama ditulis, sama dengan §10.4. `SpillMedium` dipakai bersama.
- **`open_writer`** mengembalikan `SpillWriter` (`io::Write`) yang menampung plaintext sampai 1 MiB, lalu menyegel satu rekaman dengan AAD `b"QHD1" ‖ file_id u64 LE ‖ record_index u32 LE` dan nonce dari penghitung cipher helper. `finish` menyegel sisa tampungan.
- **`read_stream`** membaca rekaman berurutan dengan `pread`, `open_in_place`, lalu mengeluarkan `Bytes` plaintext. DataFusion men-decode spill-nya tanpa validasi (F-4), dan itu tetap aman karena byte yang gagal autentikasi tidak pernah dikeluarkan. Kegagalan autentikasi menjadi galat DataFusion `External(SpillAuth)`.
- `size()` = byte ciphertext, supaya kuota disk DataFusion (`max_temp_directory_size`) menghitung yang benar-benar ada di disk.
- Mode `OsTmpDirectory` dan `Directories` tidak pernah dipakai. Bila direktori spill helper tidak aman (§9.7), mode yang dipakai `Disabled`, dan operator yang butuh spill gagal dengan pesan anggaran. Tidak pernah jatuh ke teks biasa.

### 14.9 Kurungan helper

- App tidak disandbox (ADR-0007), tetapi helper bisa. Jalur tanpa entitlement yang tersedia hari ini adalah `sandbox-exec` dengan profil SBPL tetap, `crates/qh-ffi/src/analytics/profile.sb` (di-`include_str!`). `sandbox-exec` sudah usang di dokumentasi Apple, tetapi SwiftPM sendiri masih memakainya untuk mengompilasi manifest (komentar `--disable-sandbox` di `app/build.sh`).
- **Isi profil:** `deny default`. Baca berkas diizinkan, karena berkas CSV/Parquet pengguna bisa ada di mana saja, **kecuali** `~/Library/Application Support/QueryHive` selain direktori komponen, `~/Library/Caches/QueryHive/spill` (spill app), dan `~/Library/Keychains`. Tulis hanya di direktori spill helper. Tanpa jaringan (`deny network*`). `process-exec` hanya untuk helper itu sendiri, dan tanpa `fork`. `mach-lookup` hanya yang dibutuhkan dyld dan libsystem, tanpa `com.apple.SecurityServer`, jadi tanpa Keychain.
- **Dibuktikan tes, bukan argumen.** `helpers/analytics/tests/confinement.rs` menjalankan helper persis seperti app menjalankannya, dengan perintah tersembunyi `--self-test-confinement`, lalu memeriksa bahwa: menulis ke `$TMPDIR` dan ke direktori rumah gagal; `connect` ke `127.0.0.1:55432` gagal; `exec /bin/sh` gagal; membaca berkas di `~/Library/Application Support/QueryHive` di luar direktori komponen gagal; dan membaca fixture CSV berhasil.
- Bila `/usr/bin/sandbox-exec` tidak ada atau menolak profil, helper **tidak** dijalankan tanpa kurungan. Fitur analitik tampil tidak tersedia, dengan alasannya. Jalur penggantinya, yaitu entitlement App Sandbox pada helper bertanda tangan Developer ID dengan berkas pengguna diserahkan lewat bookmark, dicatat di ADR-0045. Jalur itu baru bisa diverifikasi di mesin dengan sertifikat Developer ID (ADR-0014).
- Graf dependensi helper tidak memuat klien HTTP (§14.1). G-ANALYTICS memeriksanya dengan `cargo tree`: tidak ada `reqwest`, `hyper`, `h2`, atau `rustls`.
- **Core dump.** macOS mematikannya secara bawaan (`RLIMIT_CORE` 0), dan helper tidak menaikkannya, jadi crash tidak menulis isi memori, termasuk data hasil, ke disk. Laporan crash sistem hanya berisi tumpukan panggilan.

### 14.10 UDF

- `qh_natural(text) -> binary`: `collate::natural_key` (§13.4). `ORDER BY qh_natural(nama)` memberi urutan grid. Fungsinya satu, jadi grid dan SQL tidak bisa punya dua kolasi natural yang berselisih.
- Tidak ada UDF lain di W13-T8a. Operator filter grid (`>= 10` dengan `swift_double`) tidak diekspos ke SQL; SQL punya operatornya sendiri.

### 14.11 Berkas lokal

`register_file(name, path, format)`:

- `format`: `Csv { has_header, delimiter, quote }` atau `Parquet`. Path datang dari app (panel buka `NSOpenPanel` di UI kelak): satu berkas, atau satu direktori berisi berkas sejenis. App memeriksa dengan `symlink_metadata` bahwa path itu berkas biasa atau direktori sebelum meneruskannya. Helper membacanya lewat `ListingTable` dan `object_store` lokal, jadi berkas hanya dibaca, dan kurungan (§14.9) menjamin tidak ada yang ditulis.
- Berkas tidak disalin ke store dan tidak ditagih ke anggaran sampai dibaca query. Batch transitnya ada di dalam sewa query.
- CSV terkompresi (`.csv.gz` dan lainnya) tidak didukung di W13-T8a, karena fitur `compression` mati (§14.1). Parquet terkompresi didukung (snappy, zstd, gzip, brotli, lz4), karena codec itu datang bersama fitur `parquet`.

### 14.12 Keluaran SQL menjadi store

`run_sql(sql, out: ResultHandle, sink, cancel)`:

1. Helper merencanakan dengan `sql_with_options` (§14.7). Galat parse atau rencana menjadi satu event `error`, dengan posisi bila ada.
2. `execute_stream` di helper. Setiap `RecordBatch` → `from_arrow` (§5.6) → dipecah di 65.536 baris / 2 MiB seperti ingest → `ResultChunk`. App memvalidasinya (§14.4) lalu `StoreWriter::push_chunk`.
3. Event sama dengan `run_with_store`: `columns`, `progress { rows }` paling sering sekali per 16 ms, dan `done { rows, elapsed_ms, cancelled? }`. Grid, view, "off", dan salin bekerja tanpa kode baru.
4. Cancel mengikuti §14.6, dan `done` membawa `cancelled: true`.
5. Plafon baris sama dengan Run (5.000.000). Helper berhenti di `row_cap`, dan app tetap berwenang: `StoreWriter` menolak melewati plafon. Melewatinya membuat `done` membawa `truncated: true`.

### 14.13 Permukaan UniFFI minimal (`crates/qh-ffi/src/analytics_api.rs`)

```rust
#[derive(uniffi::Enum)]   pub enum FileFormat { Csv { has_header: bool, delimiter: u8, quote: u8 }, Parquet }
#[derive(uniffi::Record)] pub struct TableInfo { pub name: String, pub columns: Vec<ColumnWire>, pub rows: Option<u64> }
#[derive(uniffi::Record)] pub struct AnalyticsStatus { pub configured: bool, pub running: bool, pub restarts: u32,
                                                       pub reason: Option<String> }
#[derive(uniffi::Object)] pub struct AnalyticsSession { /* Arc<analytics::Session> di app */ }

#[uniffi::export]
impl EngineHost {
    pub fn configure_analytics(&self, helper_path: String, sha256_hex: String, spill_dir: String)
        -> Result<(), StoreFfiError>;                 // tanpa spawn; buat dan sapu spill_dir; butuh configure_result_stores
    pub fn analytics_status(&self) -> AnalyticsStatus;
    pub fn shutdown_analytics(&self);                 // untuk "Hapus"; idempoten
    pub fn create_analytics_session(&self) -> Result<Arc<AnalyticsSession>, StoreFfiError>;
}
#[uniffi::export]
impl AnalyticsSession {
    pub fn register_result(&self, name: String, result: Arc<ResultHandle>) -> Result<TableInfo, StoreFfiError>;
    pub fn register_file(&self, name: String, path: String, format: FileFormat) -> Result<TableInfo, StoreFfiError>;
    pub fn deregister(&self, name: String) -> Result<(), StoreFfiError>;
    pub fn tables(&self) -> Result<Vec<TableInfo>, StoreFfiError>;
    pub fn run_sql(&self, sql: String, out: Arc<ResultHandle>, sink: Arc<dyn EventSink>, cancel: Arc<RunCancel>);  // kegagalan = event
    pub fn close(&self) -> Result<(), StoreFfiError>;    // idempoten; juga dari Drop
}
```

- `StoreFfiError` mendapat varian `AnalyticsUnavailable { message }`: belum dikonfigurasi, hash tidak cocok, kurungan tidak tersedia, atau komponen dimatikan setelah crash berulang. Swift memakainya untuk menawarkan unduh atau menampilkan alasan. `create_analytics_session` sebelum `configure_analytics` menjadi varian ini.
- `register_file` dan `register_result` menjalankan helper bila belum hidup, karena `TableInfo` untuk berkas datang dari helper.
- Semua badan metode dibungkus `guarded()` (ADR-0009). Galat dari helper dipetakan ke `InvalidArgument` (SQL salah, tabel tidak ada), `TooLarge` (`ResourcesExhausted`, sewa gagal), `Spill`, `AnalyticsUnavailable`, atau `Internal`.
- `TableInfo.columns` memuat nama logis (§5.4), supaya UI bisa menulis SQL yang benar untuk nama kembar.
- Bukan perintah NDJSON, jadi invariant #11 tidak tersentuh (D-14). `app/Generated` diregenerasi (invariant #1). `MockEngine` di Swift tidak butuh apa pun sampai UI ada.

### 14.14 Yang bekerja tanpa helper

Semua yang dibangun W4–W7: store Arrow, spill, `window`, view grid (sort fallback, filter, search, dan distinct lewat rayon), Batch 7 "off", Explain di store, salin dan `rows_text`, ekspor, CLI, MCP, dan golden. Diperiksa per berkas: tidak ada berkas di §21.1–§21.5 yang bergantung pada `datafusion`, `qh-analytics`, atau `qh-analytics-proto`. `logical.rs` dan `collate::natural_key` hanya memakai crate arrow dan tidak tahu DataFusion. `query_share_bytes` dan `reserve_query` baru ditambahkan di W13-T8b. G-RUST, G-FFI, G-SWIFT, dan G-APP tidak pernah membangun atau menjalankan helper; hanya G-ANALYTICS dan tes W13-T8b yang melakukannya.

### 14.15 Tes (W13-T8a dan W13-T8b)

| Tes | Isi |
|---|---|
| `crates/qh-analytics-proto/tests/frames.rs` | Round trip setiap pesan; bingkai terpotong, panjang di atas batas, jenis tidak dikenal, dan JSON rusak menjadi galat, tanpa panic |
| `helpers/analytics/tests/remote_table.rs` | Setiap baris tabel §5.4 lewat protokol tiruan: kolom campuran → `Utf8` dengan teks sama dengan grid, `tsz` campuran → `UTC`, desimal skala campuran → skala maksimum; kredit permintaan dihormati; "the result was closed" menjadi galat query, bukan panic |
| `helpers/analytics/tests/spill.rs` | Sort 2 juta baris dengan sewa kecil memaksa spill helper. Hasilnya benar, direktori spill helper kosong, byte mentah tidak memuat canary, rekaman yang dirusak menjadi galat. Direktori tidak aman → `Disabled` → galat anggaran, bukan berkas di `$TMPDIR` (diperiksa dengan `TMPDIR` terisolasi). |
| `helpers/analytics/tests/pool.rs` | `LeasePool` tidak pernah melewati sewa, dan reservasi kembali nol setelah query selesai atau dibatalkan |
| `helpers/analytics/tests/sql_surface.rs` | `CREATE EXTERNAL TABLE`, `COPY … TO`, `INSERT`, `SET`, dan `SELECT * FROM '/etc/hosts'` ditolak |
| `helpers/analytics/tests/files.rs` | CSV dan Parquet (tanpa kompresi, snappy, zstd) dibaca dari fixture kecil, lalu `from_arrow` untuk setiap tipe keluaran |
| `helpers/analytics/tests/natural.rs` | `ORDER BY qh_natural(x)` sama dengan permutasi `set_view` sort grid atas korpus `sort.json` |
| `helpers/analytics/tests/confinement.rs` | §14.9 |
| `crates/qh-columnar/tests/from_arrow.rs` | Normalisasi §5.6 untuk setiap `DataType` di tabelnya |
| `crates/qh-ffi/tests/analytics_api.rs` | Memakai helper hasil W13-T8a: `run_sql` ke store, event sama dengan `run_with_store`, cancel, `TooLarge`, helper dibunuh di tengah query (satu event `error`, sewa kembali, app hidup, query berikutnya jalan setelah replay), hash salah → `AnalyticsUnavailable`, keluaran helper yang rusak → `Corrupt` |
| `app/Tests/QueryHiveTests/AnalyticsComponentTests.swift` | Verifikasi dengan pasangan kunci uji: tanda tangan salah, panjang salah, hash salah, dan arsip terpotong ditolak, dan tidak ada berkas tersisa; arsip benar terpasang dengan mode `0700` |
| `app/Tests/QueryHiveTests/AnalyticsSmokeTests.swift` | Build `DEBUG` dengan `QH_ANALYTICS_HELPER`: `register_result` + `run_sql` + `window` |

## 15. Tes diferensial dan format fixture (W4-T4)

### 15.1 Pembangkit di Swift

`app/Tests/QueryHiveTests/SortFixtureExport.swift` berisi dua tes:

- `testExportGridFixtures`: hanya berjalan bila `QH_EXPORT_FIXTURES=1` (selain itu `XCTSkip`), dengan pola yang sama dengan `LexerFixtureExport` di W3-T2. Ia menjalankan implementasi Swift atas korpus dan menulis JSON ke `crates/qh-result-store/tests/fixtures/grid/`, dengan path diturunkan dari `#filePath`.
- `testCommittedFixturesStillMatchSwift` (di kode bernama `testFixturesMatchTheCommittedFile`): selalu berjalan. Ia membaca fixture yang ter-commit dan memeriksa bahwa implementasi Swift acuan menghasilkan hal yang sama. **Disegarkan W6-A1 (2026-10-06):** setelah W6-T1 implementasi itu pindah ke target tes (`SwiftGridReference`, D-25), bukan dihapus, sehingga tes ini dan eksportir tetap berjalan; yang hilang dari produksi hanya penyedia jalur panas. Rencana lama ("dilewati karena implementasinya sudah dihapus") tidak terkompilasi karena eksportir memanggil implementasi itu (TM-13).

Korpusnya ditulis tangan per kategori, ditambah generator berbenih (SplitMix64, benih tetap di manifest) yang mencampur kelas karakter:

- ASCII;
- Latin beraksen (`é`, `ñ`, `ü`, `Å`, `ß`, `Ø`);
- nama Indonesia (`Siti Nurhaliza`, `Ma'ruf`, `I Gusti Ngurah Rai`, `Nur-Aini`, gelar dan singkatan);
- CJK (Han, Kana, Hangul);
- emoji (ZWJ, warna kulit, bendera);
- campuran digit (`KPM 9`/`KPM 10`, `a01`/`a1`, `v2.10.3`, `1.5`/`1.10`, nol di depan);
- bilangan (negatif, eksponen, 39 digit, `.5`, `5.`, `+5`, ` 7 `, digit Arab-Indik, fullwidth);
- NULL, `""`, whitespace saja;
- variasi huruf besar-kecil, NFC dan NFD.

Setiap korpus ≤ 2.000 nilai, dan total fixture < 1 MB.

### 15.2 Format berkas (JSON, satu berkas per perhatian, `null` = NULL)

| Berkas | Bentuk | Sumber Swift |
|---|---|---|
| `manifest.json` | `{"version":1,"generator":"SortFixtureExport.swift","swift":"…","macos":"…","locale":"…","seed":"0x…"}` | — |
| `sort.json` | `{"cases":[{"name":"ascii","values":["b",null,…],"ascending":[1,0,…],"descending":[…]}]}`, berupa permutasi indeks | `GridSort(column:0, direction:).order` atas baris satu kolom |
| `number.json` | `{"cases":[{"text":" 7 ","number":"7"},{"text":"1e400","number":null}]}` | `GridSort.number(text)?.description` |
| `filter.json` | `{"cases":[{"filter":{"text":">= 10"}` atau `{"values":["a",null]}`, `"values":[…],"matches":[true,…]}]}` | `ColumnFilter.matches` |
| `search.json` | `{"cases":[{"term":"sukamaju","rows":[["32.01","KPM Sukamaju","4"],…],"matches":[…]}]}` | `GridSearch.matches` |
| `distinct.json` | `{"cases":[{"values":[…],"distinct":[null,"a","b"]}]}` | `ColumnFilter.distinctValues` |
| `openable.json` | `{"cases":[{"type":"varchar","value":"{\"a\":1}","openable":true}]}` | `GridValue.isOpenable` |
| `format.json` | `{"cases":[{"format":"uuid","type":"bytea","value":"…","rendered":"…"}]}` | `ColumnFormat.render` |
| `width.json` | `{"cases":[{"value":"👩‍👩‍👧","count":1}]}` | `String.count` |

**Disegarkan W6-A1 (2026-10-06).** W4-T4 membangun satu berkas, `differential.json` (210 kasus), bukan sembilan berkas di atas. Tabel ini adalah rencana: baris selain sort, filter, dan search belum ada (TM-10), dan paritas format, `openable`, dan lebar dijaga di Swift oleh tes kembaran W6-T1 (D-24, D-25).

### 15.3 Pemeriksa di Rust

`crates/qh-result-store/tests/differential.rs` membangun store lewat `StoreRegistry::from_text_rows`, sehingga semua nilai adalah `Value::Text`. Ia menjalankan API publik yang sama dengan yang dipakai app (`set_view`, `render_window`, `distinct_values`, `column_widths`) dan membandingkan hasilnya.

Divergensi didaftar eksplisit:

```rust
struct Divergence { file: &'static str, case: &'static str, detail: &'static str }
const KNOWN_DIVERGENCES: &[Divergence] = &[ /* diisi W4-T4 dari ketidakcocokan yang teramati, masing-masing dengan alasan */ ];
```

Tes gagal karena salah satu dari dua hal: ada ketidakcocokan yang tidak terdaftar, atau ada entri terdaftar yang tidak lagi berbeda (entri basi). Dengan begitu daftarnya tetap jujur. Laporan sort menyebut sampai 20 pasangan nilai yang urutannya berbeda. Daftar akhirnya masuk ADR-0034.

Tes acak berbenih tanpa dependensi baru menguji totalitas comparator dan konsistensi prefiks (§13.3). `proptest` hanya dipakai bila `cargo deny` lulus, seperti W3-T2.

## 16. Ingest

### 16.1 `RowTarget` dan `pump_result` (`crates/qh-ffi/src/commands.rs`)

`emit_batches` dipecah menjadi satu loop dan dua target, supaya aturan cap, verdict (`VERDICT_FETCH`), dan cancel tetap satu salinan, sesuai catatan di `stream_rows`: "neither may grow a second copy of the batching rule".

```rust
trait RowTarget {
    fn begin(&mut self, out: &mut dyn Emitter, columns: &[ColumnMeta]) -> Result<(), CliError>;  // event `columns`
    fn fetch_size(&mut self) -> usize;
    fn accept(&mut self, out: &mut dyn Emitter, batch: &ColumnBatch, rows: Range<usize>) -> Result<(), CliError>;
    fn finish(&mut self, out: &mut dyn Emitter, outcome: Outcome) -> Result<u64, CliError>;   // baris yang dikirim
}
struct NdjsonTarget { pending: Vec<Json> }             // perilaku hari ini, dipindah apa adanya
struct StoreTarget { writer: StoreWriter, next_fetch: usize, last_progress: Option<Instant> }

async fn pump_result(out, cursor, primed, limit, cancel, target: &mut dyn RowTarget)
    -> Result<(u64, bool /*truncated*/, bool /*cancelled*/), CliError>;
```

- **`NdjsonTarget`:** event `rows` per `PREVIEW_BATCH`, `to_text` per sel, fetch selalu `PREVIEW_BATCH`, tanpa tenggat. Golden adalah penjaganya: tidak boleh ada selisih satu byte pun.
- **`StoreTarget`:**
  - `begin` memanggil `writer.begin(columns)` lalu mengirim event `columns` yang sama.
  - `fetch_size` berturut-turut 200 → 800 → 3.200 → 12.800 → 16.384 (`STORE_FETCH_MAX`). Batch pertama tetap 200 supaya TTFR tidak berubah; sesudahnya ukuran naik demi throughput. Nilainya dipotong oleh sisa cap di `pump_result`.
  - `accept` memanggil `writer.push(&batch.slice_rows(rows))`. Slice dipakai hanya di batch yang melewati cap; selain itu batch di-push utuh.
  - `progress { rows }` dikirim paling sering sekali per 16 ms, dengan throttle sendiri dan tanpa aturan 1.000 baris milik `Progress` (fakta 7).
  - `finish` memanggil `writer.finish(outcome)` **sebelum** `done` dikirim, jadi saat Swift menerima `done`, `row_count` sudah final.
- **`ExecuteOptions::max_batch_rows`:** `Some(16_384)` untuk store, `Some(PREVIEW_BATCH)` untuk NDJSON (tidak berubah).
- **Tanpa tenggat publikasi** (koreksi AR W2-A3): `accept` menyegel seluruh batch (§6.2), jadi loop pump tidak butuh timer. Fetch tetap dibungkus `select!` terhadap cancel dari Fase 1, tanpa cabang kedua. Baris terlihat begitu batch-nya keluar dari `next_batch`.
- **Pemilihan target:** `preview` dan `explain` membaca `settings.text("RESULT_SINK", "")`.
  - `"store"` dengan `out.result_store()` berisi → `StoreTarget`;
  - `"store"` tanpa store → `CliError::Usage("RESULT_SINK=store needs a result store, which only the app's engine host attaches")`;
  - selain itu → `NdjsonTarget`.
- **Galat store:** `CliError::Store(#[from] StoreError)` memetakan `DiskFull`, `SpillUnavailable`, dan `SpillAuth` ke pesan untuk pengguna. `Released` diperlakukan sebagai cancel (tab sudah ditutup), sehingga `done` membawa `cancelled: true` dan tidak ada event `error`.

**Disegarkan W6-A1 (2026-10-06). Sebagaimana dibangun (`commands.rs`, W5-T2).** `RowTarget` memakai `max_batch_rows()` dan `fetch_size(remaining)`, ditambah `abort(&CliError)` yang menutup store `Failed`. `StoreTarget::begin` tidak memanggil `writer.begin` untuk pernyataan tanpa kolom, dan `finish` menelan `Released`. Selebihnya sama dengan di atas.

### 16.2 Event di mode store

`step connect`, `columns` (payload sama), `progress { rows }` (≤ 1 per 16 ms), lalu `done { rows, truncated, query_id, elapsed_ms, cancelled? }`, bentuknya sama dengan `done` NDJSON. Tidak ada event `rows`. `error` tetap satu event. Tabel event di doc modul `lib.rs` mendapat satu baris untuk mode ini.

### 16.3 Publikasi dan TTFR S2

Batch pertama (200 baris) langsung disegel dan dipublikasikan (§6.2). **Disegarkan W6-A1 (2026-10-06):** event `progress` pertama, yang keluar segera setelah batch itu diterima, membangunkan koordinator untuk satu poll langsung (§17.3); tick display link (dibatasi 60 Hz, paling lama 16,7 ms) hanya jaring pengaman. Poll melihat 200 baris, lalu `rowsDidGrow(from:to:)`, lalu satu `window` sinkron, lalu gambar. Tidak ada decode JSON di antaranya, sehingga target TTFR S2 p95 ≤ 50 ms tidak bergantung pada cap.

### 16.4 W7-T1: driver menulis array Arrow langsung

Menggantikan `qh_core::ColumnarBuilder` di `performance-plan.md` §11 butir 1. Builder yang dipakai driver adalah `qh_columnar::ChunkBuilder` yang sama dengan yang dipakai store (§5.3), jadi hanya ada satu builder.

```rust
// crates/qh-driver/src/lib.rs
#[async_trait]
pub trait Cursor: Send {
    fn columns(&self) -> &[ColumnMeta];
    async fn next_batch(&mut self, max_rows: usize) -> Result<Option<ColumnBatch>, EngineError>;
    /// Appends up to `max_rows` rows straight into `out` and returns how many it appended
    /// (0 = finished). The default goes through `next_batch`, so a driver that has not been
    /// taught Arrow, and every export writer, keeps working unchanged.
    async fn next_chunk(&mut self, out: &mut ChunkBuilder, max_rows: usize) -> Result<usize, EngineError> { … }
}
```

- `StoreTarget` memanggil `next_chunk`, lalu `writer.push_chunk(out.seal())`. `NdjsonTarget` dan penulis ekspor tetap memakai `next_batch`, sehingga golden dan ekspor tidak tersentuh.
- Kesetaraan dijaga tes: untuk setiap kasus `type_zoo` (fake dan live), `value_at` atas chunk dari `next_chunk` sama dengan `Value` dari `next_batch`, sel demi sel. Ini gate `type_zoo live identik` W7-T1.

**PostgreSQL, protokol teks (jalur hari ini).**

- `base_type()` dihitung sekali per kolom saat cursor dibuat, menjadi `ColumnParser` (`Int`, `Float`, `Numeric`, `Bytea`, `Date`, `Time`, `Timestamp`, `Timestamptz`, `Interval`, `Json`, `Text`). Hari ini ia dihitung per sel, sekitar 15% profil W1-T8.
- Per baris, per kolom: `row.get(i) -> Option<&str>` langsung di-parse ke append bertipe (`push_i64`, `push_decimal`, `push_timestamp`, …). Teks disalin **sekali**, dari buffer kawat ke buffer data `StringBuilder`. Hari ini salinannya sekitar 4 kali (W1-T8): `Value::Text`, transpose, clone `row_of`, `to_string`.
- Parse yang gagal memanggil `push_value(Value::Text(…))`, dan builder menjadikan chunk-kolom itu bertag (§5.3). Aturan normalize ("teks yang tidak bisa di-parse tetap teks") tidak berubah.
- `Vec<Vec<Value>>` dan transpose (`lib.rs:703-748`) hilang dari jalur store.

**PostgreSQL `COPY (<sql>) TO STDOUT` (W7-T2).** Baris datang sebagai baris teks dipisah `\t`, dengan `\N` untuk NULL dan escape backslash. Parser `memchr` memecah field, meng-unescape ke scratch per field hanya bila ada `\`, lalu memanggil `ColumnParser` yang sama. Teks setiap nilai berasal dari fungsi output server yang sama dengan protokol teks, jadi paritasnya dijaga tes kesetaraan di atas dan golden.

**MySQL** (`mysql_async`, protokol teks) memakai pola yang sama: parser per kolom dari tipe kolom, lalu append bertipe. **Trino:** `DeserializeSeed` streaming atas `data` langsung ke `ChunkBuilder`, tanpa pohon `serde_json::Value`. Komposit (`array`, `row`, `map`) dibangun sebagai `Value` lalu `push_value`, karena chunk-kolomnya memang bertag.

**Yang tidak berubah:** aturan cap, verdict, dan cancel tetap satu salinan di `pump_result`. `max_batch_rows` tetap plafon. Batas segel 65.536 baris / 2 MiB diterapkan `ChunkBuilder`: driver berhenti menambah baris saat `out.is_full()`, tanpa perlu tahu angkanya. `push_chunk` menghitung `ChunkFlags` dan `ColumnStats` saat segel, sama seperti `push`.

## 17. Sisi Swift

**Disegarkan W6-A1 (2026-10-06)** terhadap seam yang dibangun W5-T1 (ADR-0032) dan permukaan yang dibangun W5-T2. Rancangan W2-A3 mengandaikan seam yang belum ada; bagian ini menggantikannya seluruhnya. Diperiksa terhadap `465a0c6`: W5-C sudah ditutup, jadi nomor baris untuk `ResultGrid.swift`, `ResultGridTable.swift`, dan `DatabaseEngine.swift` berlaku di commit itu.

### 17.0 Seam yang ada, dan selisihnya terhadap rancangan lama

| Hal | Rancangan W2-A3 | Dibangun W5-T1 dan W5-T2 | Keputusan W6-A1 |
|---|---|---|---|
| Protokol baris | `StoreRows` dengan `poll()`, `phase`, `viewID`, `apply`, `release`, `columnWidths()`, dan `distinctValues` yang mengembalikan tuple | `ResultRows`: `count`, `fetched`, `columns`, `cell`, `fullValue`, `rows(in:columns:)`, `naturalCharCounts()`, `distinctValues` sinkron; mewarisi `RowReading.row(at:)` tanpa implementasi bawaan; tidak melempar (TM-1) | Protokol tetap. Extension memberi implementasi bawaan untuk `poll()`, `isLive`, `prepare(formats:)`, dan `release()`, sehingga kembaran acuan tidak berubah. `distinctValues` menjadi `async`. `StoreRows` mengimplementasikan `row(at:)` lewat blok baris. Kontrak "tidak melempar" tetap; galat dicatat di `lastFailure` dan muncul sebagai banner, sedangkan jalur salin memakai varian yang melempar (§17.2) |
| Lebar kolom | `columnWidths()`, hitungan grapheme | `naturalCharCounts()`, hitungan `Character` dengan batas 64 | `StoreRows` mengisinya dari `column_widths()` sebagai `min(n, 64)` (TM-8) |
| Format sel | format per kolom di permintaan jendela | `cell(...format:)` per panggilan, tanpa protokol invalidasi (D-3 ADR-0032) | Satu halaman memuat 32 kolom, sedangkan `cell` hanya tahu format satu kolom. `prepare(formats:)` memberi format semua kolom dan membuang halaman blok yang formatnya berubah (§17.2); argumen `format` di `cell` hanya diperiksa |
| Teks sel | teks jendela 256 unit UTF-16 | `CellText.text` = baris pertama, ≤ 1.024 unit, berhenti di batas `Character` | `StoreRows` memakai fungsi baris-pertama yang sama dengan kembaran acuan (`CellText.make`), supaya aturan P-3 hanya punya satu salinan |
| Flag sel | bit QHW1 | `CellFlags` | Pemetaan eksplisit dengan tes (TM-7) |
| Pembacaan kolom | per sel, kolom terlihat | semua kolom setiap baris (TM-2) | rentang kolom (D-23) |
| Hasil bertambah | display link memanggil `noteNumberOfRowsChanged` | `Coordinator.rowsDidGrow()` membatalkan seluruh tampilan | `rowsDidGrow(from:to:)` hanya membatalkan baris baru |
| `ArrayRows` | dihapus | `QueryTab.result` dan nilai awal `Coordinator.rows` | pindah ke target tes (D-25); produksi memakai `EmptyRows` |

### 17.1 Batas FFI

`Models/StoreRows.swift` mengimpor `QueryHiveFFI` dan menerima `any ResultHandleProtocol`, protokol yang dibangkitkan UniFFI (`ResultHandle: ResultHandleProtocol`). Protokol cermin dengan tipe Swift sendiri, seperti rancangan lama, tidak dibangun: ia menggandakan sekitar sembilan tipe tanpa menambah apa pun yang tidak dimiliki protokol bangkitan, dan impor FFI sudah tidak eksklusif (fakta 11). Tes memakai kelas yang memenuhi `ResultHandleProtocol` (handle palsu yang menjawab `StaleView`, `TooLarge`, `Superseded`, atau jendela buatan) dan handle asli dari host tes.

`Support/DatabaseEngine.swift` tetap bebas FFI; API-nya berbicara dalam `StoreRows`:

```swift
func makeResultStore() throws -> StoreRows          // kosong; kolom menyusul lewat event `columns`
@discardableResult
func runIntoStore(_ command: String, env: [String: String], store: StoreRows,
                  onEvent: @escaping (Event) -> Void,
                  onExit: @escaping (_ status: Int32, _ stderr: String) -> Void) -> (any EngineRun)?
func storeFromRows(columns: [Event.Column], rows: [[String?]]) throws -> StoreRows
```

`RustEngine` mengimplementasikannya. `makeResultStore` membungkus `host.createResultStore()`. `runIntoStore` meniru `run` (`RustRun`, `Sink`, `runQueue`, `deliversAfterStop`) dan memanggil `host.runWithStore`; hanya `preview` dan `explain` yang diterima (selain itu `onExit(cannotStart, …)`), dan store yang handle-nya bukan `ResultHandle` asli juga `cannotStart`. `storeFromRows` membungkus `host.storeFromRows`. `RustEngine.ensureStoresConfigured(spillDir:budgetBytes:) -> StoreSweep?` bersifat statik dan aman dipanggil berulang: pemanggil pertama menang dan memanggil `host.configureResultStores`, pemanggil berikutnya mendapat `nil`. Ia memakai flag di bawah kunci, bukan pencocokan pesan galat. Pemakainya: `QueryHiveMain.main()` (§17.6), `--bench`, `Snapshot`, dan `TestStores`.

`SilentEngine` sudah ter-commit di `465a0c6` (`DatabaseEngine.swift:80`) dan hari ini hanya mengimplementasikan `run`, `terminateAll`, dan `runBlocking`. W6-T1 menambahkan `makeResultStore` dan `storeFromRows` yang mendelegasikan ke `RustEngine()`, karena membuat store tidak membuka jaringan dan `--snapshot` memerlukannya; `runIntoStore` hanya menerima tanpa menjalankan, seperti `run` hari ini. `MockEngine` memakai host nyata lewat `TestStores`, supaya tes grid memeriksa Rust yang sama dengan produksi. `DatabaseEngine` mewajibkan ketiga metode, jadi `MockEngine` dan `SilentEngine` yang belum diubah tidak akan terkompilasi; itu disengaja.

**Host bersama tidak bisa menumpahkan (koreksi AR B4).** `RustEngine.host` adalah satu-satunya host produksi (`RustEngine.swift:60`), dan anggaran serta direktori spilnya ditetapkan sekali: `ensureStoresConfigured` memberi `nil` kepada pemanggil kedua, dan `configure_result_stores` kedua yang langsung ke host menjawab `InvalidArgument` (`host.rs:319-323`). `TestStores` memanggilnya dengan `(nil, 64 MiB)`, jadi di proses tes host bersama itu tanpa spill (`spill_enabled == false`) dan korpus kecil tidak pernah melewati anggarannya. Proses tes tidak pernah bisa menumpahkan lewat host itu, siapa pun pemanggil pertamanya, dan tes yang membutuhkan galat `.Spill` nyata, chunk yang benar-benar tumpah, atau miss tumpah akan lulus tanpa menumpahkan apa pun atau gagal menurut urutan tes. Tes dan bench semacam itu karena itu tidak memakai host bersama; mereka membangun `EngineHost()` sendiri. Konstruktor UniFFI-nya publik dan registry-nya per instance (`ResultHandleSmokeTests.testTheRegistryIsConfiguredOnceAndNotImplicitly` sudah begitu). Urutannya: `configureResultStores(spillDir: <direktori sementara unik>, budgetBytes: <kecil>)`, `host.storeFromRows(...)`, lalu `StoreRows(handle:)` di atas handle itu (konstruktornya menerima `any ResultHandleProtocol`, §17.2). `storeStats()` dibaca dari host itu sendiri, dan direktorinya dihapus di `tearDown`. `TestStores` menyediakan pembantunya, `spillingHost(budgetBytes:)` (§21.4). Bench spill di luar target tes berjalan lewat `--bench` di proses sendiri, yang mengonfigurasi anggarannya sendiri sebagai pemanggil pertama (§17.6). Selebihnya host bersama dipakai seperti rencana.

### 17.2 `StoreRows`

```swift
final class StoreRows: ResultRows, @unchecked Sendable {      // state di bawah NSLock
    init(handle: any ResultHandleProtocol, columns: [Event.Column] = [])
    private(set) var columns: [Event.Column]               // dari event `columns`, bukan handle.columns()
    private(set) var count: Int                            // visible: baris view
    private(set) var fetched: Int
    private(set) var phase: StorePhase
    private(set) var viewID: UInt64
    var isLive: Bool                                          // streaming, atau apply sedang berjalan
    var lastFailure: StoreFailure?                            // galat yang bukan StaleHandle/StaleView/Superseded

    func setColumns(_ columns: [Event.Column])                // main, dari event `columns`
    func poll() -> PollResult                                 // main; row_count() = load atomik; `count` hanya mengikuti `viewID` yang sama
    @discardableResult
    func prepare(formats: [ColumnFormat]) -> IndexSet         // main; membuang halaman blok yang formatnya berubah; mengembalikan kolom sumber yang berubah
    func cell(row: Int, column: Int, format: ColumnFormat) -> CellText       // main; halaman; miss sinkron, juga untuk baris di luar halaman pendek
    func fullValue(row: Int, column: Int, format: ColumnFormat) -> String?   // cell_text
    func row(at index: Int) -> [String?]?                     // blok baris
    func rows(in range: Range<Int>, columns: [Int]) -> [[String?]]           // aman-kosong
    func rowsOrThrow(in range: Range<Int>, columns: [Int]) throws -> [[String?]]
    func naturalCharCounts() -> [Int]
    func distinctValues(column: Int) async -> DistinctSample
    func apply(_ spec: ViewSpec) async throws -> ViewInfo     // memblokir di antrean serial milik store ini (.userInitiated)
    func applyBlocking(_ spec: ViewSpec) throws -> ViewInfo   // untuk tes
    func dropPages()                                          // tab ke latar
    func release()                                            // idempoten; deinit memanggilnya juga
}
struct PollResult: Equatable { var grewFrom: Int?; var finished: Bool }
struct DistinctSample: Equatable { var values: [String?]; var more: Bool }
final class EmptyRows: ResultRows { /* count 0, fetched 0, columns [] */ }
```

`StoreFailure` membungkus `StoreFfiError` bersama nama operasi yang gagal, untuk banner dan log.

- **Halaman.** `PAGE_ROWS` = 64 baris dan `COL_BLOCK` = 32 kolom **sumber** (bukan kolom tampilan), dikunci `(viewID, pageIndex, columnBlock)`. Satu miss adalah satu panggilan `window(viewID, first, 64, kolomBlok, formatBlok)`. Nilai ini **sementara** (D-22), diturunkan dari angka W5-T2 (§2.9): 128 × 30 bertipe 0,396–0,449 ms p99 di sisi Rust berarti 103–117 ns per sel, jadi 128 × 32 = 4.096 sel sekitar 0,42–0,48 ms sebelum UniFFI, tanpa margin terhadap 0,5 ms (R-28). Untuk 64 × 32 = 2.048 sel, ekstrapolasi linear memberi sekitar 0,21–0,24 ms. W5-T3 dan `StoreWindowBench` mengukur 64 × 32 lewat UniFFI, resident dan tumpah, lalu mengonfirmasi atau mengganti: 128 baris bila p99 128 × 32 ≤ 0,4 ms termasuk UniFFI, 32 baris bila p99 64 × 32 > 0,4 ms.
- **Halaman mencatat jumlah barisnya (koreksi AR B2).** Halaman yang dibaca selagi hasil masih mengalir pendek: dengan 200 baris, halaman 3 (baris 192 sampai 255) hanya memuat 192 sampai 199, yaitu 8 dari 64. View filter yang streaming juga bertambah di bawah `viewID` yang sama (§13.8, hook perluasan di `store.rs`), dan `rowsDidGrow(from:to:)` hanya membatalkan rect, bukan cache. Kunci `(viewID, pageIndex, columnBlock)` tidak berubah, tetapi setiap halaman menyimpan `first` dan `rows`, yaitu `row_count` di header QHW1 (jumlah baris yang benar-benar dikembalikan `window`, dijepit pada `visible`). `cell(row: r, …)` dengan `r < count` dilayani dari halaman hanya bila `r < first + rows`; selain itu **miss**: halaman dibaca ulang dengan satu panggilan `window` dan menggantikan entrinya (kunci sama, jadi cache tidak membengkak). Selama fase belum terminal (`Empty`, `Streaming`) halaman pendek pasti basi, tetapi aturannya memakai jumlah baris halaman, bukan fase: halaman yang direkam saat streaming tetap pendek sesudah fase menjadi terminal, dan perluasan terakhir view filter berjalan di `finish`, sesudah fase terminal (`store.rs`: `finish` memanggil `expand_view_now` setelah mengubah fase). Syarat "fase belum terminal" saja akan meninggalkan baris kosong selamanya. Karena `r < count ≤ visible` dan baris hanya bertambah dalam satu view, pembacaan ulang selalu memuat `r`; bila ternyata tidak, sel dijawab kosong dengan satu baris log (bug), tanpa pembacaan ulang kedua dalam frame yang sama. Baris yang sudah digambar tidak berubah saat hasil bertambah, jadi `GridRowTextCache` tidak perlu dikosongkan (D-28). Blok `row(at:)` `(viewID, blok)` memakai aturan yang sama (di bawah).
- **Cache.** Paling banyak 24 halaman dan 8 MB, tidak pernah di bawah 8 halaman (satu viewport 40–60 baris dan 1–2 blok kolom memakai 2–4 halaman, dan batas byte saja membuat hasil teks panjang menggilas dirinya sendiri dalam satu frame). Pembuangan menurut pemakaian terakhir. Tab yang pindah ke latar memanggil `dropPages()` (dari `selectTab`), supaya NFR-P3 (anggaran + 64 MB) tidak dimakan cache Swift dari banyak tab.
- **Tanpa prefetch** (D-22). Satu halaman 64 baris menutupi 1.344–1.920 pt pada baris 21–30 pt, jadi fling 3.000 pt/s melewati sekitar dua halaman per detik: dua miss sinkron per detik sekitar 0,25 ms, kira-kira 0,05% waktu main thread. Prefetch menambah jalur asinkron (basi menurut `viewID`, balapan dengan `apply`) untuk menghemat jumlah itu. Miss pada chunk yang tumpah menambah sekitar 0,4 ms (§2.5) dan diamortisasi cache dekripsi 8 chunk (satu chunk 2 MiB `wide_500k` melayani sekitar 55 halaman). Bila W6-T2 mengukur frame > 1 ms yang disebabkan miss tumpah, prefetch ditambahkan saat itu dengan aturan simpan ≥ 10% yang sama dengan prebuild di D-12 Fase 5.
- **`WindowPage`.** Memegang `Data` apa adanya, memeriksa magic, versi, panjang total, dan offset monoton ≤ H sebelum membaca (O(RC)), lalu mendekode sel secara lazy dengan `String(decoding:as: UTF8.self)`. Flag QHW1 dipetakan **eksplisit**: NULL → `.null`, EMPTY → `.empty`, OPENABLE (bit 2) → `.openable`, NUMERIC (bit 3) → `.numeric`, TRUNCATED (bit 4) → `.truncated`. Tes: satu sel untuk setiap bit (TM-7). Teks jendela lalu melewati `CellText.make` (baris pertama, 1.024 unit).
- **Format.** `ColumnFormat` dipetakan ke `CellFormat`: `raw`, `text`, `uuid`, `unixTimestamp`. `swiftRenderedFormats` (awal `[.json]`, D-24) dilayani lewat `rows_text` dan `ColumnFormat.render` seperti §11.4. **`prepare(formats:)` membuang halaman (koreksi AR B2).** Format tidak ada di kunci halaman, jadi pembuangan ini satu-satunya yang menjaga halaman dari teks berformat lama. `prepare(formats:)` membandingkan vektor format dengan yang terakhir diterimanya. Untuk setiap blok kolom yang salah satu formatnya berubah, ia membuang **semua** halaman blok itu (semua `pageIndex`, view mana pun) dan mengembalikan kolom sumber yang berubah; blok yang formatnya tetap tidak disentuh. Koordinator memanggilnya **sebelum** membangun ulang teks, dan baris `GridRowTextCache` yang rentang terbangunnya memuat kolom yang berubah ikut dibuang (`textCache.removeAll()` memenuhi syarat itu, dan itu yang dilakukan `formatChanged` dan `reloadFormats` hari ini). Blok `row(at:)` tidak terpengaruh, karena ia selalu `Raw`. `fullValue(format:)` memakai jalur yang sama dengan `cell` untuk format itu (`cell_text` untuk format Rust, `cell_text(.raw)` lalu `ColumnFormat.render` untuk format Swift), supaya tooltip dan sel tidak berselisih.
- **Tes kembaran.** `StoreRowsTests` membandingkan sel demi sel `ArrayRows` acuan dengan `StoreRows` (lewat `storeFromRows`) atas korpus kecil untuk setiap format, setiap bentuk flag, dan lebar. Aturan keputusan: selisih yang bukan divergensi tercatat memindahkan format itu ke `swiftRenderedFormats`, kecuali `Raw`. **Divergensi tercatat** (ditulis di berkas tes, bukan kegagalan):
  1. *Potongan teks dan flag `TRUNCATED` (koreksi AR B5).* `window` memotong teks sel di 256 unit UTF-16 pada batas grapheme dan menyalakan `TRUNCATED` bila terpotong (§11.3, `store_api.rs:609`), sedangkan `ArrayRows` memotong baris pertama di 1.024 `Character` (`ResultRows.swift:134`). Akibatnya setiap format, `Raw` termasuk, berbeda pada sel panjang, di teks maupun flag. Tes membandingkan 256 unit pertama: teks `StoreRows` harus sama dengan baris pertama dari awalan 256 unit (batas grapheme, aturan §11.3) atas teks terender penuh acuan (`ArrayRows.fullValue`), dan `.truncated` dibandingkan hanya dengan aturan Rust (menyala bila teks terender penuh melebihi 256 unit UTF-16), tidak dengan `.truncated` acuan. Sel yang terender penuhnya ≤ 256 unit dibandingkan utuh, teks dan flag. Kolom di `swiftRenderedFormats` dipotong 1.024 oleh jalur Swift yang sama (`CellText.make`), jadi untuknya perbandingan utuh berlaku tanpa divergensi ini.
  2. *`openable`.* Kembaran lebih longgar (karakter pertama `{` atau `[`) daripada Rust (validasi JSON penuh, §7.1).
  **`Raw` tidak pernah pindah** ke `swiftRenderedFormats`: ia teks tersimpan apa adanya, dan memindahkannya membuat seluruh jalur jendela tidak dipakai. Selisih `Raw` di luar dua divergensi itu adalah bug di Rust atau di pemetaan jendela, dan **menghentikan tugas** (laporkan; jangan atasi dengan memindahkan format). Format `Text`, `Uuid`, dan `UnixTimestamp` yang selisih di luar divergensi tercatat pindah ke `swiftRenderedFormats`, dan hasilnya dicatat di pesan commit.
- **Tes koreksi AR untuk B2 dan B3** (handle palsu yang memenuhi `ResultHandleProtocol` dan mencatat setiap panggilan `window`; masing-masing satu tes):
  - `testAShortPageReadWhileStreamingIsRefetchedWhenRowsArrive` (B2a): handle mulai dengan 200 baris (halaman 3 memuat 8 dari 64) dan `cell(row: 195, …)` mengisi cache. Handle tumbuh ke 400 baris di `viewId` yang sama, lalu `poll()`, lalu `cell(row: 200, …)` mengembalikan teks nyata, bukan sel kosong, dengan tepat satu panggilan `window` tambahan untuk halaman itu. Varian view filter yang `visible`-nya bertambah di `viewId` yang sama, dan `row(at: 200)` untuk blok barisnya, ada di tes yang sama.
  - `testPrepareFormatsDropsThePagesOfTheBlocksWhoseFormatChanged` (B2b): dua blok kolom; format satu kolom di blok kedua diubah. `prepare(formats:)` mengembalikan kolom itu, `cell` di blok kedua membaca ulang dan mengembalikan teks berformat baru, halaman blok pertama tetap dari cache (jumlah panggilan `window` untuk blok itu tidak bertambah), dan baris `GridRowTextCache` yang memuat kolom itu dibangun ulang (diperiksa lewat `rowText` di koordinator tes).
  - `testPollIgnoresTheNewViewUntilTheApplyHopInstallsIt` (B3): handle palsu yang `rowCount()`-nya sudah menjawab `viewId` baru dengan `visible` lebih kecil (filter menyusutkan hasil), sementara `window(viewId: lama, …)` masih dilayani. `poll()` memperbarui `fetched` dan `phase` tetapi `count` tetap dan `grewFrom == nil` (jadi tidak ada `rowsDidGrow(from:to:)` dengan `from > to`), dan `cell` tetap menjawab teks view lama. Sesudah `applyBlocking` memasang `viewID` dan `count` baru, `poll()` mengikuti view baru.
- **`row(at:)`** adalah baris sumber penuh, dibaca lewat blok: `rowBlock = max(1, 4096 / jumlahKolom)` baris per panggilan `rows_text` (sekitar 0,4 ms), paling banyak 8 blok, dikunci `(viewID, blok)`. Blok mencatat `first` dan `rows`, dan `row(at: i)` dengan `i < count` yang jatuh di luar `rows` membaca ulang blok itu (aturan halaman pendek di atas). `CellEdits.fill` dan `paste`, `WritePlan`, dan `UpdateStatements` memanggilnya per sel; tanpa blok, mengisi 10.000 baris adalah 10.000 panggilan yang masing-masing menyusun seluruh baris.
- **`rows(in:columns:)` dan `rowsOrThrow`** memakai `rows_text`, dipecah menurut batas FFI: paling banyak 4.096 baris dan 262.144 sel per panggilan, dan `TooLarge` (64 MiB) membagi dua baris. Jalur salin (`GridClipboard.text(result:…)`) dan pembangun rencana tulis memakai `rowsOrThrow` dan **menolak hasil parsial**: `StaleHandle` dan `StaleView` membatalkan tanpa pesan, galat lain muncul sebagai banner, dan tidak pernah sebagai sel kosong yang sampai ke papan klip atau ke klausa `WHERE`. `rows(in:columns:)` yang tidak melempar tetap ada untuk pembaca yang aman-kosong dan mencatat galat pertama di `lastFailure`.
- **`distinctValues(column:) async -> DistinctSample`** memanggil `distinct_values(column, limit: valuePickerLimit + 1)` di antrean `.userInitiated`. Pemilih bisa dipilih bila `!more && values.count <= ColumnFilter.valuePickerLimit`; selain itu kolom pencarian teks. `filterEditor` memuatnya lewat `.task` dengan keadaan "Loading…". Daftar dari Rust terurut (NULL dulu, lalu byte setelah NFC), sama dengan `ColumnFilter.distinctValues(in:column:)` yang lama. Kembaran acuan diperbaiki di 6a (TM-3): urutan itu, himpunan lengkap, dan `more` bila melewati batas.
- **`naturalCharCounts()`** membaca `column_widths()` saat pertama diminta dan mengulang selama `fetched < 200` dan fase streaming; sesudah `fetched ≥ 200` atau fase terminal hasilnya dibekukan (Rust membekukan statistik di baris ke-200). Hasilnya `min(n, 64)`; NULL = 0 di Rust tidak mengubah lebar (TM-8).
- **`poll()` dan view yang sedang dipasang (koreksi AR B3).** `isLive` juga menyala selama `apply` berjalan, jadi display link mem-poll tepat saat `set_view` selesai di Rust dan sebelum hop ke main. Sejak `1b59154` `row_count()` membaca satu snapshot view (§12.4): dari saat `set_view` memasang view baru, `visible` dan `view_id` yang dijawabnya milik view **baru**, sedangkan Swift masih menggambar `viewID` lama (nilai awalnya 0 = tanpa view, sama dengan `view_id` Rust bila belum ada view). Aturan: `poll()` selalu memperbarui `fetched` dan `phase`; `count` diperbarui, dan `grewFrom` dikembalikan, **hanya bila `rc.viewId == viewID` dan `rc.visible > count`**. Bila `rc.viewId != viewID`, `count` tetap dan `grewFrom == nil`. Hop `apply` adalah satu-satunya penulis `viewID` dan `count` untuk pergantian view: ia memasang keduanya sekaligus dari `ViewInfo`. Tanpa aturan ini grid memakai hitungan view baru dengan `viewID` lama, setiap miss dijawab `StaleView` dan digambar kosong, dan filter yang menyusutkan hasil menghasilkan `rowsDidGrow(from:to:)` dengan `from > to`. `rc.visible < count` pada `viewID` yang sama tidak mungkin (baris hanya bertambah dalam satu view); `poll()` mengabaikannya, mencatat satu baris log (bug), dan tidak pernah menyusutkan `count`.
- **`apply`.** `QueryTab.viewSpec` dibangun dari `columnFilters` (kolom sumber; `ColumnFilter.nullToken` menjadi `nil`; `.text(needle)` menjadi `FilterSpec.Text`), `effectiveLocalSearch`, dan `memorySort`. `set_view` memblokir, dan Rust tidak mengurutkan panggilan yang berjalan bersamaan (§13.2): ia memasang build yang selesai paling akhir, membatalkan hanya view yang digantikannya, dan tidak pernah menjawab `Superseded` untuk `set_view`. Rancangan sebelumnya (antrean konkuren, "yang lama menjawab `Superseded` dan diabaikan") salah dan menghasilkan urutan terbalik: grid menampilkan baris filter atau sort yang sudah diganti pengguna, dan `viewID` serta `count` bisa mundur. Tinjauan satu putaran W6-T1 menangkapnya sebagai temuan blocking ("out-of-order applies", pesan commit `1e3c0ad`), dan kodenya menutupnya di tiga tempat (`StoreRows.swift`, diperiksa di `4ec7480`):
  1. **Antrean serial per store.** Setiap `StoreRows` punya satu `DispatchQueue` serial `.userInitiated` (`viewQueue`), dan `apply` mengirim `handle.setView` ke sana. Panggilan untuk satu store sampai ke Rust satu per satu menurut urutan `apply` dipanggil, jadi yang terakhir diminta adalah yang terakhir dipasang.
  2. **Hop ke main adalah satu-satunya penulis `viewID` dan `count` untuk pergantian view** (lihat `poll()` di atas). `install` memasang keduanya dari `ViewInfo`, dan **mengabaikan** `ViewInfo` yang `viewId`-nya lebih kecil dari `viewID` yang sedang ditampilkan (`guard info.viewId >= _viewID`). Penjaga ini ada supaya ask terbaru tetap yang ditampilkan walau hop-nya tiba terbalik (komentar di `apply`); tanpanya hop yang terlambat menarik grid kembali ke view yang lebih tua.
  3. Setelah `install`, `tab.gridRevision += 1`. Sampai saat itu grid menampilkan view lama.

  `Superseded` tidak diharapkan dari `set_view` di jalur ini. `QueryTab.scheduleViewApply` tetap memperlakukannya, bersama `StaleHandle`, sebagai apply yang tidak memasang apa-apa (menurunkan `viewBusy` bila itu ask terbaru), sebagai jaring. `Streaming` tidak mungkin sampai ke sini (tombol sort mati selama streaming, §17.3); `TooLarge` dan `Spill` menjadi banner. Tes urutan ada di `StoreLifecycleTests` (§21.4).
- **Galat `window`.** `StaleHandle`: sel kosong. `StaleView`: `apply` baru memasang view dan hop ke main belum tiba, jadi sel kosong tanpa cache untuk satu frame. `InvalidArgument`, `Corrupt`, `Internal`, `Spill`: sel kosong, `lastFailure` terisi, dan banner grid. Tidak pernah `try!`.
- **Thread.** State di bawah `NSLock`. `cell` dan `fullValue` hanya di main menurut kontrak (`dispatchPrecondition` di build debug). `apply`, `distinctValues`, dan `rowsOrThrow` boleh dipanggil dari luar main.

### 17.3 Penggambaran dan polling

**Rentang kolom (commit 6a, D-23).** `GridTableView.draw` meneruskan rentang kolom yang sudah dihitungnya (`geometry.columns(in:)`, kini dipakai) ke `Coordinator.rowText(_:columns:)` dan `GridRowPainter.paint(columns:)`. `GridRowText` memegang `first: Int` dan teks untuk rentang itu saja. `GridRowTextCache` menyimpan rentang yang sudah dibangun per baris, dan membangun ulang baris itu bila rentang yang diminta keluar darinya; rentang yang dibangun diperluas satu lebar viewport ke kiri dan kanan supaya scroll horizontal pendek tidak membangun ulang. Painter hanya mengecat kolom dalam rentang; stripe, gutter, dan separator tidak berubah, jadi G-VIS tidak bergerak. Pembaca yang membutuhkan kolom lain (AX, tooltip, salin) memakai `fullValue` atau `rows(in:)`, bukan `rowText`. Biaya `GridColumnGeometry.edges(of:)` yang O(n) per kolom ikut turun karena hanya rentang yang dicat; jumlah kumulatif sekali jalan adalah urusan W5-T3.

**Polling (commit 6b).** `GridTableView` membuat `displayLink(target:selector:)` (`NSView`, macOS 14) di `viewDidMoveToWindow`, menambahkannya ke run loop main pada mode `.common` (supaya tetap berdetak saat tracking scroll), dengan `preferredFrameRateRange` paling tinggi 60 Hz dan `isPaused = true` secara bawaan. `Coordinator` menyalakannya saat `rows.isLive`. Setiap tick memanggil `pollRows()`: `rows.poll()` lalu, bila `grewFrom != nil`, `rowsDidGrow(from:to:)`. `poll()` hanya melaporkan pertumbuhan view yang sedang digambar (`rc.viewId == viewID`, §17.2), jadi `from < to` selalu. Link berhenti sesudah satu poll terakhir bila fase terminal, dan saat view keluar dari jendela; tab di latar tidak mem-poll, dan saat tab kembali ke depan satu poll langsung dijalankan. `rowsDidGrow(from:to:)` memanggil `noteNumberOfRowsChanged()` dan membatalkan hanya rect baris `from..<to`. Ia tidak menyentuh `textCache`, geometri, atau pohon AX (kecuali klien AX terpasang, lewat `noteResultChangedForAX`). Perubahan **view** tidak lewat polling: `apply` yang selesai menaikkan `gridRevision`, dan jalur `Coordinator.apply(_:force:)` yang ada menangani refresh penuh. Event `progress` pertama membangunkan koordinator sekali (satu `pollRows()` langsung, mekanisme bebas), supaya TTFR S2 tidak menunggu tick.

**Footer dan SwiftUI (D-28).** `progress` (paling sering satu per 16 ms dari engine) disaring `AppModel` menjadi `tab.fetchedRows` paling sering 5 Hz (`AppModel.footerCountInterval` = 0,2 detik, kadens yang sama dengan `previewPaintInterval` hari ini tetapi hanya untuk satu angka), dan `done` menulis nilai akhir. `ResultGrid.body` membaca `tab.fetchedRows`, sehingga Observation menjalankannya ulang; `StoreRows` sendiri bukan `@Observable`. `naturalCharCounts()` dibaca di `body`, jadi lebar kolom berubah bersama `fetchedRows` saat `fetched` melewati 200 dan saat selesai; `GridInputs.layout` berubah dan refresh penuh terjadi dua sampai tiga kali per hasil, bukan lima kali per detik.

**Header selama streaming.** `GridStyle.sortEnabled = !tab.previewing`, dan chevron diredupkan (`performance-plan.md` §14 butir 2). `sortOnServer` dan `fireServerSearch` sudah menolak saat `previewing`; jalur `applyMemorySort` mendapat penjaga yang sama, karena Rust menjawab `Streaming` untuk sort sebelum semua baris tiba.

**Signpost.** `PerfSignposts.firstRowsEvent()` hari ini dipanggil dari event `rows`, yang tidak ada lagi di mode store. Ia dipindah ke `poll()` pertama yang melihat `fetched > 0`. `firstPaint` tetap dari `draw`. Jalur S2: batch 200 baris disegel → `progress` → poll langsung → `draw`, tanpa decode JSON.

### 17.4 `AppModel` dan `QueryTab`

- **`PreviewResult`** kehilangan `rows` dan mendapat `rowCount` yang disimpan (diisi `done`; `summary` memakainya). Pembaca produksi `preview.rows` (TM-5) pindah ke `tab.result.fetched` dan `tab.fetchedRows`; badge `Panels` memakai `tab.fetchedRows`.
- **`QueryTab`:** `activeResult: StoreRows?`, `baseResult: ResultSlot?` (`ResultSlot` = `PreviewResult` + `StoreRows`), `fetchedRows`, `viewBusy`, dan `viewSpec`. `result` mengembalikan `activeResult ?? EmptyRows`. `displayedRows`, `displayedCache`, `displayedCacheRevision`, `resultCache`, dan `resultCacheRevision` dihapus; `gridRevision` tetap dan kini naik saat view terpasang. `columnFilters`, `gridSearch`, dan `applyMemorySort` tetap membuang seleksi, antrean edit, undo, dan sort memori, dan mengganti `gridRevision += 1` dengan `scheduleViewApply()`.
- **`runPreview`:** `makeResultStore()` sebelum run (D-12) → `tab.activeResult = store` → `runIntoStore("preview", …)`. Event `columns` → `store.setColumns` dan `tab.preview = PreviewResult(columns:, rowCount: 0, …)`; `columns` kosong (pernyataan tulis) melepas store seketika. `progress` → `fetchedRows` dan pemicu poll. `done` → `PreviewResult` akhir (`truncated`, `queryID`, `elapsedMS`, `stopped`, `rowCount`). `error` atau status keluar ≠ 0 melepas store (UX hari ini: grid dikosongkan dan galat ditampilkan). `phase == .failed` tanpa event `error` bukan galat (§12.4). Cancel mempertahankan baris. Nasib `activeResult` dan `baseResult` saat Run, sort, search, dan "off" ada di §18.
- **`explain`:** jalur yang sama dengan `runIntoStore("explain", …)` (D-8). Loop penumpukan di `explain()` hilang.
- **`previewEnvironment`:** plafon di §17.7.
- **Fixture, Snapshot, BenchMode, dan tes.** `QueryTab.showRows(columns:rows:truncated:queryID:elapsedMS:stopped:)` membuat store lewat `Engine.current.storeFromRows` dan memasangnya seperti `done`. `RustEngine.ensureStoresConfigured(spillDir: nil, budgetBytes: 64 MiB)` dipanggil `Snapshot.seeded`, `BenchMode`, dan `TestStores`. Situs yang berubah (TM-5): `AppModel` 6, `Snapshot` 8 (mutasi di tempat `tab.preview?.rows[i][j] = …` menjadi perubahan array sebelum store dibangun), `BenchMode` 1, dan 21 situs di tes. Untuk `scroll-30x1m` (30 juta sel), `store_from_rows` dipanggil sekali di luar interval ukur dengan anggaran bench 2 GiB (tanpa spill, sebanding dengan Fase 0); waktu dan puncak RSS-nya dicatat (R-38).
- **Penjaga edit (D-27).** `viewBusy` menyala di `scheduleViewApply()` dan padam saat view terpasang atau gagal. Selama menyala: `typeCellEdit`, `fill`, `paste`, dan commit ditolak dengan catatan "The grid is updating its rows", dan tombol Review dan Save mati. Saat hop "view terpasang" tiba di main, `cellSelection = nil` dan `cellEdits.discard()` dijalankan lagi bila ada isinya, karena edit yang lolos sebelum `viewBusy` menyala menunjuk indeks view lama. Pembangun rencana tulis (`WritePlan.build`, `UpdateStatements.generate`) menolak bila `viewBusy`. Rencana itu menyusun `UPDATE` dan `DELETE` dengan klausa `WHERE` dari nilai asli baris (`UpdateStatements.appendMatch`), jadi baris yang salah berarti pernyataan yang menulis atau menghapus baris yang salah. Baris yang tidak terbaca karena galat (bukan karena di luar rentang) harus menjadi peringatan di rencana; `WritePlan.build` hari ini melewatinya dengan `continue` tanpa suara. Tes: (a) `apply` yang selesai dengan edit yang di-stage membuang antreannya, (b) staging ditolak saat `viewBusy`, (c) pembangun rencana menolak saat `viewBusy`. Jalur ini termasuk yang ditinjau reviewer database (§21.4).
- **Sesi.** Tab yang dipulihkan dari sesi tidak punya store (`baseResult == nil`); "off" menjalankan ulang SQL dasar (§18).

### 17.5 Yang dihapus, dipindah, dan dipertahankan di W6-T1

| Yang | Nasib | Syarat |
|---|---|---|
| `AppModel.previewPaintInterval`, dan penumpukan `rows` di `runPreview` dan `explain` | dihapus; `footerCountInterval` (0,2 detik) menggantikan hanya untuk hitungan footer | `StoreRows` dan `progress` bekerja (W5-T2 hijau) |
| `QueryTab.displayedCache`, `displayedCacheRevision`, `displayedRows`, `resultCache`, `resultCacheRevision`, `BaseResultCache` | dihapus | `StoreRows` menjadi sumber |
| `PreviewResult.rows` | dihapus; `rowCount` disimpan | TM-5 selesai |
| `ArrayRows` | **pindah** ke `app/Tests/QueryHiveTests/ArrayRowsReference.swift` sebagai kembaran acuan; produksi memakai `EmptyRows` | pemakai produksinya habis (hanya penampung "belum ada hasil"). Menjaga sepuluh tes `ResultRowsTests`, dan menjadi jaring paritas format dan flag (D-24, D-25) |
| `GridSort.order`, `compare`, `value`, `number`, `isPlainNumber`, `isExponent`; `ColumnFilter.matches`, `matchesText`, `distinctValues(in:column:)`; `GridSearch.matches` | **pindah** ke `app/Tests/QueryHiveTests/SwiftGridReference.swift` (enum `SwiftGridReference`), bukan dihapus | `SortFixtureExport` memanggilnya (TM-13); dengan begini eksportir dan `testFixturesMatchTheCommittedFile` tetap berjalan. Dari produksi hilang jalur panasnya |
| Pemindaian `naturalWidths` | sudah diganti `naturalCharCounts()` di W5-T1 | `StoreRows` mengisinya dari `column_widths()` |
| `isOpenable` dan `ColumnFormat.render` di jalur gambar | diganti flag dan teks jendela; `ColumnFormat.render` tetap untuk sel staged, pembaca nilai, tooltip format Swift, dan `swiftRenderedFormats` | D-9, D-24 |
| `import QueryHiveFFI` di `ResultRows.swift`, `CellSelection.swift`, `GridMetrics.swift` | dihapus (tidak memakai tipe FFI, fakta 11) | — |

Tetap di produksi: `GridSort` sebagai struct (`Direction`, `next`, `column`), enum `ColumnFilter` (`isEmpty`, `label`, `nullToken`, `valuePickerLimit`), `GridSearch.debounceInterval` dan `searchableTerm`, `SearchStatement`, `ServerSort`, `GridValue`, dan `ColumnFormat.render`.

Tes Swift yang memanggil fungsi yang dipindah (`ResultGridTests.swift` untuk `GridSort.number` dan `value`; `GridColumnsTests.swift` untuk `GridSearch.matches`) ditulis ulang terhadap `StoreRows` yang dibangun lewat `storeFromRows`: asersinya sama, tetapi kini menguji Rust. Hitungan tes tidak turun (NFR-Q; 603 dengan 5 dilewati pada `465a0c6` menurut ledger).

### 17.6 Startup, direktori spill, dan batas fd

`QueryHiveMain.main()` memanggil `RustEngine.ensureStoresConfigured(spillDir:budgetBytes:)` **sekali, sinkron, sebelum `QueryHiveApp.main()`**, jadi sebelum `AppModel(persistsSession: true)` memulihkan tab. Argumennya `spillDir` = `~/Library/Caches/QueryHive/spill` (`FileManager.urls(for: .cachesDirectory, …)`) dan anggaran 256 MiB (O-12). Letaknya sesudah cabang `--snapshot` dan `--bench`, yang mengonfigurasi anggarannya sendiri (spill mati atau di direktori scratch) sebelum memanggil fungsi yang sama; karena pemanggil pertama menang, skenario `launch`, yang kembali ke `QueryHiveApp.main()`, tidak terkonfigurasi dua kali. Urutan ini menjamin sapuan (§9.7) selesai sebelum Run pertama membuat store, tanpa balapan dengan Run yang dipulihkan sesi (§12.3). Ongkosnya satu `mkdir` dan `chmod` serta satu `read_dir` atas direktori yang normalnya kosong, di bawah 1 ms, dan dicatat skenario `launch`. `StoreSweep` dicatat ke log; `spillEnabled == false` dicatat dengan alasannya. Diagnostik membaca `store_stats()`.

**Batas fd (D-26, menutup R-19).** `setrlimit(RLIMIT_NOFILE)` menaikkan batas lunak ke min(batas keras, 4.096) bila lebih rendah, di tempat yang sama. Dua alternatif yang disebut R-19 dinilai: *menutup fd store yang menganggur* **tidak bisa**, karena berkas spill di-unlink sebelum byte pertama ditulis (D-5) dan fd-nya adalah satu-satunya pegangan ke data itu; menutupnya membuang chunk yang tumpah. *Menaikkan batas* satu baris dan tidak mengubah perilaku lain. Pengukuran: `launchctl limit maxfiles` di mesin ini 256 lunak dan tak terbatas keras, `kern.maxfilesperproc` 61.440 (TM-12). Penanganan `EMFILE` sebagai disk penuh (§10.4) tetap sebagai jaring terakhir. Bukti dari W6-T1: tes kecil bahwa batas lunak sesudah panggilan ≥ min(batas keras, 4.096), dan skenario `tabs-100-held` (§19) yang mencatat `open_fds_peak` dengan 100 tab yang masing-masing menahan store yang tumpah.

### 17.7 Plafon 5.000.000 dan P-1

`AppModel.productRowLimitCeiling` bernilai 200.000 sampai P-1 (blueprint Fase 5 §5.4 dan §15) terukur lewat tangkapan compositor, yang dijalankan pemilik (izin Screen Recording) atau di sesi bench eksklusif. Commit 6b **tidak** mengubah angka itu. Commit tersendiri sesudahnya, `feat(app): raise the row limit ceiling to 5,000,000`, mengubah konstanta, teks `SettingsView`, dan `RowLimitSettingTests` (D-29):

- P-1 lulus: plafon naik ke 5.000.000.
- P-1 gagal: `WindowedRows: ResultRows` (blueprint Fase 5 §5.4, ukuran S sampai M, dipetakan lewat `Coordinator.resultRow(forTableRow:)`) masuk lebih dulu di commit yang sama, lalu plafon naik.
- P-1 tidak terukur (izin OS): plafon tetap 200.000 dan dicatat "tidak diukur (izin OS)".

Catatan aritmetika, bukan keputusan: 2^24 pt dibagi tinggi baris terbesar (30 pt) adalah 559.240 baris, jadi hasil sampai sekitar 500.000 baris tidak pernah melewati 2^24 pt pada tinggi baris mana pun. Bila pemilik ingin plafon antara yang tidak butuh P-1, angka itu aman secara geometri; memori tidak menjadi syarat karena store spill menanggungnya.

## 18. Batch 7 "off": dua store per tab

**Disegarkan W6-A1 (2026-10-06).** `QueryTab` memegang `baseResult: ResultSlot?` (`ResultSlot` = metadata `PreviewResult` + `StoreRows`; hasil Run dalam urutan server) dan `activeResult: StoreRows?` (yang digambar grid; metadatanya `tab.preview`). Invarian: paling banyak dua store per tab, dan `activeResult === baseResult.rows` kecuali sort atau search server sedang aktif. `BaseResultCache` (aturan 10.000 baris) dihapus: anggaran global dan spill menggantikannya, dan menyimpan base tidak menggandakan memori resident karena base yang menganggur tumpah lebih dulu (§9.5). Hari ini `baseResult` diisi di `applyPreviewDone` bila Run tidak dihentikan; aturan "tidak dihentikan" tetap, supaya "off" tidak memulihkan hasil parsial.

| Aksi | Akibat |
|---|---|
| Run (`runPreview(baseRun: true)`) atau Explain | Lepas `activeResult` dan `baseResult`, buat store baru, `activeResult = baru`. Saat `done` tanpa stop, `baseResult = ResultSlot(baru)`. |
| Sort atau search server (`activeSort.origin == .server`, `runPreview(baseRun: false)`) | Bila `activeResult !== baseResult?.rows`, lepas `activeResult`. Buat store baru S, `activeResult = S`. Base tetap utuh untuk "off". |
| Sort in-memory (fallback O-8: `ServerSort` menolak, atau grid menampilkan plan) | `activeResult.apply(viewSpec)`. Tanpa store baru. |
| Filter funnel dan search in-memory | `activeResult.apply(viewSpec)` (FR-GRID-04, P-26). |
| "Off" (`clearSort`, `clearSearch`; klik ketiga, atau search dikosongkan) | Bila `activeResult !== baseResult.rows`: lepas `activeResult`, lalu `activeResult = baseResult.rows` dan `tab.preview = baseResult.meta`, dengan view yang memuat filter funnel dan search in-memory tab saat ini tanpa sort (identitas bila keduanya kosong; tanpa query). Koreksi AR tetap berlaku: `apply(ViewSpec())` yang kosong akan diam-diam membuang filter yang masih tampil aktif di header. Bila `activeResult === baseResult.rows` dengan view sort: `apply` tanpa sort. Hanya bila `baseResult == nil` (tab dipulihkan dari sesi, Run dihentikan, atau galat) `rerunBaseSQL` menjalankan ulang SQL dasar. |
| Tab ditutup | Lepas keduanya (§19). |

Penjaga edit Fase 3 tetap berlaku (sort atau search server ditolak bila `cellEdits` tidak kosong), dan D-27 menambah penolakan selama `viewBusy`.

## 19. Umur handle dan penutupan tab

**Disegarkan W6-A1 (2026-10-06).** Langkah 1 dikoreksi terhadap kode (TM-4): rancangan lama menyebut `process?.terminate()` seolah itu yang menghentikan Run, padahal `process` hanya dipakai export dan perintah panjang.

1. `closeTab` menghentikan **`tabs[index].process` dan `tabs[index].previewProcess`** (flag cancel; `previewProcess` memegang `preview` dan `explain`), **lalu** memanggil `tab.releaseResults()`, yang memanggil `release()` pada `activeResult` dan pada `baseResult.rows` dan menjadikan keduanya `nil`. Keduanya segera. `closeOtherTabs`, `closeTabs(after:)`, dan `closeAllTabs` sudah lewat `closeTab`.
2. Rust membebaskan chunk dan anggaran, menutup fd spill, menandai store `Released`, dan membatalkan view yang sedang berjalan (§9.6).
3. Run yang masih menulis mendapat `Released`, lalu berakhir sebagai cancel (`closing_the_tab_mid_stream_ends_the_run_cancelled_and_not_failed` dan `closing_the_tab_before_the_first_page_ends_cancelled_and_not_as_an_error` di `store_sink.rs`). Event-nya sampai di `RustRun` yang sudah berhenti (`RustEngine.swift`, `Sink.onEvent`), yang untuk `preview` dan `explain` tetap meneruskan `done{cancelled}`. `releaseResults()` karena itu juga mengganti `previewToken`, supaya event susulan dibuang oleh `guard tab.previewToken == run`.
4. Yang masih terbang saat `release()` datang menjawab menurut titik datangnya (koreksi ronde 2, diperiksa di `4ec7480`). Pembacaan `window`, `rows_text`, dan `cell_text` yang sudah memegang view mendapat `Superseded`, karena `release` menyalakan flag cancel view yang berjalan; pembacaan yang tiba sesudahnya mendapat `StaleHandle`. `set_view` tidak pernah menjawab `Superseded` (§13.2): bila release datang sebelum ia mulai, jawabannya `StaleHandle`; bila datang di tengah build, daftar chunk yang sudah dikosongkan bisa menjawab galat store lain (mis. `Corrupt`), atau build sempat selesai, memasang view di store yang sudah dilepas, dan menjawab `Ok`. `distinctValues` sesudah release menjawab `StaleHandle`. Semua jawaban itu aman dan diabaikan: `scheduleViewApply` memperlakukan `Superseded` dan `StaleHandle` sebagai apply yang tidak memasang apa-apa, dan `viewApplied(on:generation:)` membuang hop yang `activeResult`-nya sudah bukan store itu (`releaseResults()` mengosongkannya), apa pun jawabannya. `viewBusy` ikut padam bersama tab.
5. `StoreRows.deinit` memanggil `release()` lagi secara idempoten, dari thread mana pun, sebagai jaring bila `QueryTab` masih dirujuk closure undo atau sejenisnya.
6. Tab yang dipindah ke latar memanggil `dropPages()` (§17.2); store-nya tetap, dan tumpah lebih dulu bila anggaran menuntut (§9.5).
7. Saat app keluar (`terminateAll`), fd ditutup kernel. Berkas sudah di-unlink, jadi tidak ada yang tersisa.

Tes yang ditambahkan ke `TabCloseTests` (TM-4): menutup tab menghentikan `previewProcess` (handle `MockEngine` mencatat `terminate`), melepas kedua store (`store_stats().stores` kembali ke nilai awal), dan tidak meninggalkan `viewBusy` menyala.

**G-LEAK.** `--bench tabs-100` (100× buka dan tutup tab dengan store dari `store_from_rows`, 2.000 × 10 seperti hari ini, dan anggaran kecil supaya spill benar-benar terjadi) harus menghasilkan `leaks` nol dan `store_stats().stores == 0` di akhir. Dua catatan yang berubah. (1) `spilled_bytes` sudah nyata (`1b59154`, TM-9), jadi pemeriksaan `spilled_bytes == 0` di akhir berlaku. Ia dihitung atas store yang masih hidup, sehingga nol di akhir mengikuti `stores == 0`; direktori spill kosong selalu (D-5). (2) Skenario baru `tabs-100-held` menahan 100 tab terbuka bersamaan, masing-masing dengan store yang tumpah (anggaran bench 2 MiB), mencatat jumlah fd proses (`open_fds_peak`, dibaca lewat `proc_pidinfo(PROC_PIDLISTFDS)`) sebelum, puncak, dan sesudah semuanya ditutup; hasil akhir harus kembali ke nilai awal. Ini pengukuran R-19 (§17.6). `development-plan.md` §1 G-LEAK perlu menambahkan pemeriksaan fd ini.

## 20. Jalur yang tetap NDJSON

| Jalur | Setelah Fase 6 |
|---|---|
| CLI `queryhive-engine` (semua perintah, termasuk `preview` dan `explain`) | NDJSON `rows`, tidak berubah. `RESULT_SINK=store` dari CLI menjadi galat usage. |
| Korpus golden (`crates/qh-ffi/tests/golden.rs`, `tools/golden/live_cases.py`) | tidak berubah. Tidak ada yang memasang `RESULT_SINK`. |
| MCP (`queryhive-mcp`) | tidak berubah. Proses terpisah tanpa `EngineHost`. |
| `export`, `to_table`, `import_data`, `apply_changes`, `table_op` | tidak berubah. PR-01: ekspor tidak memakai store. |
| `count`, `objects`, `catalogs`/`schemas`/`tables`, `test`, perintah lokal | tidak berubah |
| `host.run(…)` di app untuk `preview`/`explain` tanpa store | NDJSON; dipakai tes, dan tetap berfungsi sebagai jalur lama |
| App: `preview` `LIMIT 1` yang hanya membaca `columns` (detail objek `AppModel.swift:558`, probe kolom impor `loadImportColumns` di `:2175`) | NDJSON lewat `host.run`, tidak berubah. Keduanya tidak pernah menggambar baris, jadi tidak butuh store. |
| App: Run, sort server, search server, Explain | store (`run_with_store`) |
| App: SQL atas hasil dan berkas lokal (W13-T8a–c, UI kemudian) | store (`AnalyticsSession::run_sql`, dieksekusi helper). CLI dan MCP tidak mendapatkannya (NFR-C, D-14, D-16). |

Tes kesetaraan di W5-T2 menjalankan kasus golden fake-cursor lewat kedua target dan memastikan `rows_text` store sama persis dengan array `data` NDJSON, sel demi sel. Itu bukti bahwa renderer tetap satu.

## 21. Perubahan per tugas

### 21.1 W4-T3: inti store Arrow (Rust murni)

**Disegarkan W6-A1 (2026-10-06).** Selesai di `e595877`. Tinjau keamanan putaran 1 menemukan enam temuan blocking yang semuanya diperbaiki; putaran 2 tertunda dan tercatat "pending review" di ledger. `qh-columnar` dan `qh-result-store` dibangun menurut tabel ini. Selisih yang W6-T1 butuhkan ada di §1.3 dan §12.4.

**Dibuat:**

| Berkas | Isi | Prioritas |
|---|---|---|
| `crates/qh-columnar/Cargo.toml` | `qh-core`, `arrow-array`, `arrow-schema`, `arrow-buffer` (59.3), `thiserror`; `[lints.rust] unsafe_code = "forbid"` | P0 |
| `crates/qh-columnar/src/lib.rs` | Doc modul: kontrak `Value` ↔ Arrow (§5.1), aturan pemilihan encoding, kenapa `Utf8` dan bukan `Utf8View` | P0 |
| `crates/qh-columnar/src/encoding.rs` | `Encoding`, kunci metadata `qh.enc`, `encoding_of(field)`, pemeriksaan `enc` terhadap `DataType` | P0 |
| `crates/qh-columnar/src/builder.rs` | `ChunkBuilder`: append bertipe, `push_value`, konversi sekali ke `tagged`, batas 65.536 baris / 2 MiB, `is_full`, `seal() -> SealedChunk` | P0 |
| `crates/qh-columnar/src/read.rs` | `value_at`, `text_at` (pinjam untuk `text`/`json`) | P0 |
| `crates/qh-columnar/src/tagged.rs` | Codec bertag dipindah dari `qh-result-store/src/codec.rs`, tag tidak berubah, perbaikan `Unknown` `raw` (B-5, §5.2), `Reader` yang tidak pernah panic | P0 |
| `crates/qh-columnar/tests/roundtrip.rs` | Generator berbenih semua varian, chunk campuran → `tagged`, overflow → `tagged`, `to_text` sama, `Float` bit utuh | P0 |
| `crates/qh-result-store/src/chunk.rs` | `StoreChunk`, `ChunkFlags`, `ColumnStats`; segel: flag openable/numerik (§7), statistik (§5.4), `shrink_to_fit`, `bytes` | P0 |
| `crates/qh-result-store/src/registry.rs` | `StoreRegistry`, `StoreId`, anggaran, LRU evict, cache dekripsi, sapuan, `from_text_rows`, `synthetic`, `stats` (§9) | P0 |
| `crates/qh-result-store/src/spill.rs` | `SpillCipher` (kunci, nonce, AAD dengan tag domain), `SpillFile` (0600, unlink), `SpillMedium`, `FaultyMedium`, rekaman `QHP1` (IPC + flag) (§10) | P0 |
| `crates/qh-result-store/src/logical.rs` | Skema logis (§5.4) dan konversi chunk → batch logis. Pemakai pertamanya `tests/logical.rs`, lalu klien helper yang melayani permintaan chunk di W13-T8b (§14.4). | P0 |
| `crates/qh-result-store/src/view.rs` | `ViewSpec`, `View`, pipeline, kunci sort dari array Arrow, filter, search, perluasan saat streaming, `distinct_values`, cancel dan supersede (§13) | P0 |
| `crates/qh-result-store/src/collate.rs` | `natural_key` memcmp-able dan jalur prefiks, `fold`, `ci_contains`, `ci_equal`, `swift_plain_number`, `swift_double`, `NumKey` | P0 |
| `crates/qh-result-store/src/render.rs` | Writer buffer `QHW1` dari array Arrow (§11), potongan 256 UTF-16, port `ColumnFormat`, validator JSON, openable, `head_widths` | P0 |
| `crates/qh-result-store/tests/spill.rs` | Tes NFR-S3 (§10.5) | P0 |
| `crates/qh-result-store/tests/window.rs` | Layout byte demi byte, clamp, `StaleView`, potongan (ASCII, CJK, emoji ZWJ, zalgo), teks jendela = `to_text` untuk setiap encoding, resident dan tumpah | P0 |
| `crates/qh-result-store/tests/view.rs` | Semantik sort (NULL, numerik, seri stabil, desc, `"1.0"`/`"1"`), filter, search, streaming, totalitas dan kesamaan jalur penuh vs prefiks, `Streaming`, `Superseded`, `TooLarge` | P0 |
| `crates/qh-result-store/tests/logical.rs` | Setiap baris tabel §5.4: store campuran (semua encoding, chunk `null`, chunk bertag, chunk tumpah) → `logical_batches()` memberi **satu** skema untuk semua batch, tipe sesuai tabel, nilai sama dengan `value_at` (atau `to_text` untuk kolom yang jatuh ke `Utf8`), dan batch lolos `RecordBatch::try_new` serta round trip IPC. Ini persis yang diterima helper lewat pipa (§14.4), diuji tanpa DataFusion. Metadata field ikut dibandingkan, karena DataFusion menolak batch yang skemanya tidak identik dengan skema tabel. | P0 |
| `crates/qh-core/src/render.rs` | `write_text(&Value, &mut String)` sebagai implementasi tunggal; `to_text` membungkusnya. Formatter timestamp, tanggal, waktu, desimal, float, dan hex (`hex_encode` tanpa `format!` per byte) menulis ke buffer yang sama tanpa `String` perantara. Keluaran byte demi byte tidak berubah (tes `value.rs`/`render.rs` yang ada dan G-GOLDEN). | P0 |

**Diubah:**

| Berkas | Perubahan | Prioritas |
|---|---|---|
| `crates/qh-result-store/src/codec.rs` | **Dihapus.** Codec bertag pindah ke `qh-columnar/src/tagged.rs`, dan layout `QHC1` tidak dibangun. Tes lama yang masih bermakna (round trip, input terpotong, tag rusak) dipindah ke `qh-columnar`. | P0 |
| `crates/qh-result-store/src/store.rs` | `StoreShared`, `StoreHandle`, `StoreWriter` (`push`, `push_chunk`), indeks dan publikasi (§9.3), aturan segel, `Residency` dengan `Arc<StoreChunk>`. `window(range)` lama menjadi `values(range) -> Vec<Vec<Value>>` lewat `value_at`. Klaim palsu "startup sweep in `qh-storage`" dihapus. | P0 |
| `crates/qh-result-store/src/lib.rs` | Doc modul ditulis ulang (Arrow, anggaran, spill terenkripsi, view). `forbid(unsafe_code)` tetap. | P0 |
| `crates/qh-result-store/Cargo.toml` | `qh-columnar`, `qh-rt`, `arrow-array`, `arrow-schema`, `arrow-buffer`, `arrow-ipc` (`default-features = false`), `rayon`, `ring = "0.17"`, `unicode-segmentation`, `unicode-normalization = "0.1"`, `unicode-properties = "0.1"`, `zeroize`, `serde_json` | P0 |
| `crates/qh-rt/src/lib.rs`, `crates/qh-rt/Cargo.toml` | `view_pool()` rayon ber-QoS (§13.1) dan tes QoS worker-nya | P0 |
| `Cargo.toml` (root) | anggota `crates/qh-columnar`; `[workspace.dependencies]` `qh-columnar`, `arrow-array`/`-schema`/`-buffer`/`-ipc = "59.3"` (sejajar `parquet` 59.3), `rayon = "1"`, `unicode-segmentation = "1"`. `rust-version` tetap 1.85 (arrow 59.3 menyatakan 1.85). | P0 |
| `Cargo.lock` | gugus arrow store, `flatbuffers`, keluarga `rayon`, `unicode-segmentation` | P0 |

Verifikasi: G-RUST, G-DENY, G-GOLDEN (keluaran `render.rs` tidak berubah), tes §10.5, tes view, window, round trip, dan skema logis, ditambah `cargo test -p qh-result-store -- --ignored bench_*` untuk angka lokal (dicatat, bukan gate). Target lokal: jendela 128 × 30 bertipe p99 ≤ 250 µs di sisi Rust (§2.6 mencatat 442 µs sebelum `write_text`). Pemeriksa: RR, SEC (§10), TD, SF (disk penuh, rekaman rusak), CR, dan AR (verdict baru).

### 21.2 W4-T4: fixture diferensial

**Disegarkan W6-A1 (2026-10-06).** Selesai di `b1a1838`, tetapi hanya sebagian dari tabel di bawah: satu berkas `differential.json` (210 kasus), bukan sembilan berkas (TM-10).

Tidak berubah dari W2-A3. `differential.rs` membangun store lewat `StoreRegistry::from_text_rows`, jadi semua nilai adalah `Value::Text` dan jalurnya melewati chunk `text` Arrow.

| Berkas | Isi | Prioritas |
|---|---|---|
| `app/Tests/QueryHiveTests/SortFixtureExport.swift` | Eksportir bergerbang `QH_EXPORT_FIXTURES=1` dan pemeriksa fixture ter-commit (§15.1) | P0 |
| `crates/qh-result-store/tests/fixtures/grid/*.json` | Sembilan berkas §15.2 | P0 |
| `crates/qh-result-store/tests/differential.rs` | Pemeriksa dan `KNOWN_DIVERGENCES` (§15.3) | P0 |

Verifikasi: `QH_EXPORT_FIXTURES=1 swift test --filter SortFixtureExport` (regenerasi), `swift test --filter SortFixtureExport` (pemeriksa), `cargo test -p qh-result-store --test differential`.

### 21.3 W5-T2: sink engine dan `ResultHandle`

Tidak berubah dari W2-A3, kecuali bahwa `qh-ffi` kini menaut gugus arrow store lewat `qh-result-store`. Ukuran app naik sekitar selisih baris "arrow" di §2.2.

**Dibuat:** `crates/qh-ffi/src/store_api.rs` (§12), `crates/qh-ffi/tests/store_sink.rs`, `app/Tests/QueryHiveTests/ResultHandleSmokeTests.swift`.

**Diubah:** `crates/qh-ffi/src/{commands.rs,events.rs,host.rs,lib.rs,uniffi_api.rs}`, `crates/qh-ffi/Cargo.toml` (`qh-result-store`), `Cargo.lock` (tepi `qh-ffi → qh-result-store`, tanpa crate baru), `crates/qh-ffi/examples/bench_ffi.rs` (`window` atas 1M × 30 sintetis Arrow, `window-json`, `view-*`), `app/Generated/*`. Isi per berkas sama dengan tabel W2-A3 (`git show dc4186f:docs/architecture/blueprints/fase-6-data-plane.md`, §18.3).

Verifikasi: G-RUST, G-FFI, G-SWIFT, G-GOLDEN (tanpa selisih), `cargo run --release -p qh-ffi --example bench_ffi -- window`.

**Disegarkan W6-A1 (2026-10-06).** Selesai di `df445fd` (perbaikan chunk-kolom NULL) dan `bfb9680`. Gate: G-RUST 1074/0/1, G-FFI (`RustEngineTests` 5/0, `ResultHandleSmokeTests` 6/0), G-SWIFT 601/5 dilewati/0, G-GOLDEN 11/22 dengan set klasifikasi tidak berubah. Tinjau satu putaran (rust-reviewer dan security-reviewer, model terkuat) dengan satu temuan blocking yang diperbaiki di `df445fd`. Perilaku dan deviasi ada di §12.4, angka di §2.9, dan backlog W5-C dari tinjauan ada di ledger (baris W5-T2). Backlog itu ditutup W5-C: sisi Rust di `1b59154` (G-RUST 1081/0/1) dan sisi Swift di `465a0c6` (G-SWIFT 603/5 dilewati/0, G-VIS 17/0), tinjau ringan satu putaran APPROVE, menurut ledger (baris W5-C).

### 21.4 W6-T1: integrasi Swift

**Disegarkan W6-A1 (2026-10-06).** Versi lama menulis "tidak berubah" dan mewariskan daftar berkas W2-A3; itu tidak cocok dengan seam yang dibangun W5-T1 dan permukaan yang dibangun W5-T2. Ini rencana kerja yang presisi. Ukuran L. Pelaksana: **sonnet** (O-20 menggantikan GP-o di `development-plan.md` §5 dan §6). Tinjau: risiko tinggi (rewrite grid dan data plane, penggunaan FFI, jalur edit yang menulis ke database), jadi model terkuat, **satu putaran** (O-20), dengan reviewer database untuk penjaga edit dan jalur `WritePlan` (§17.4). Temuan blocking diperbaiki, diverifikasi dengan gate, di-commit, dan dicatat "pending review" di ledger.

**Tiga commit**, mengikuti preseden W5-T1 dan pelajaran I-1 di ledger (setiap commit harus lolos `swift build` dan tesnya sendiri):

| Commit | Isi | Gate |
|---|---|---|
| **6a** `refactor(grid): build only the columns on screen, and give the distinct list its real contract` | D-23 (rentang kolom di `rowText`, painter, dan `GridRowTextCache`), `rowsDidGrow(from:to:)` (belum ada pemanggil), extension bawaan `ResultRows`, `prepare(formats:)`, `distinctValues` `async` dengan `DistinctSample`, perbaikan TM-3 di `ArrayRows`, tes baru. Semuanya di atas `ArrayRows`; engine dan Rust tidak disentuh. | G-SWIFT, G-VIS 17/0 tanpa rekam ulang, `GridParityTests`, `GridAccessibilityTests`, `GridMetricsTests` |
| **6b** `perf(app): results live in the Rust store, and the grid reads windows` | sisanya (tabel di bawah) | G-SWIFT, G-VIS 17/0 terhadap baseline Fase 5, `cargo test -p qh-result-store --test differential` dan `swift test --filter SortFixtureExport`, G-FFI (`app/Generated/` tidak boleh berubah; bila berubah, berhenti dan tanyakan), G-APP, G-LEAK, G-BENCH(1 S2, 2, 3), dan `StoreWindowBench` |
| **6c** `feat(app): raise the row limit ceiling to 5,000,000` | §17.7, bersyarat P-1 | G-SWIFT, `RowLimitSettingTests`, bukti P-1 |

**Dibuat (6b):**

| File | Isi | Prioritas |
|---|---|---|
| `app/Sources/QueryHive/Models/StoreRows.swift` | `StoreRows`, `WindowPage` (dekode QHW1 dan pemetaan flag), `PollResult`, `DistinctSample`, `StoreFailure`, `EmptyRows`, dan `CellText.make` bila belum ada di `ResultRows.swift` | P0 |
| `app/Tests/QueryHiveTests/StoreRowsTests.swift` | handle palsu (`StaleView`, `Superseded`, `TooLarge`, jendela buatan); kembaran `ArrayRows` lawan `StoreRows` (setiap format, bentuk flag, lebar, dengan divergensi tercatat dan aturan `Raw` di §17.2); setiap bit flag QHW1; `row(at:)` per blok; `rows(in:)` terpecah dan `rowsOrThrow`; `apply` dan penjaga edit; view basi; tiga tes handle palsu koreksi AR (halaman pendek saat streaming, `prepare(formats:)`, `poll()` saat `apply` berjalan; §17.2); tes yang membutuhkan `Spill` nyata memakai `EngineHost()` sendiri (§17.1) | P0 |
| `app/Tests/QueryHiveTests/StoreLifecycleTests.swift` | penjaga edit `viewBusy` (D-27) dan generasi apply (`testOnlyTheLatestApplyEndsViewBusy`); **urutan apply (koreksi ronde 2, §13.2 dan §17.2)**: `testTwoAppliesReachRustInCallOrderSoTheLaterSpecIsTheOneInstalled` (handle palsu yang membuat ask pertama lambat; spec sampai ke Rust menurut urutan panggilan dan `viewID` akhir milik ask kedua) dan `testTheStoreEndsOnTheLatestViewEvenWhenTheHopsComeBackOutOfOrder` (ask kedua dijawab `viewId` lebih kecil; `install` mempertahankan view dan `count` yang lebih baru); `closeTab` melepas kedua store dan menghentikan kedua run (§19) | P0 |
| `app/Tests/QueryHiveTests/Bench/StoreWindowBench.swift` | `window` 64 × 32 dan 128 × 32 lewat UniFFI, resident (host bersama `TestStores`) dan tumpah (`EngineHost()` sendiri dengan direktori spill sementara dan anggaran kecil: host bersama tidak punya spill dan tidak bisa dikonfigurasi ulang, §17.1), p50/p95/p99; `rows_text`; ongkos `Data` | P0 |
| `app/Tests/QueryHiveTests/TestStores.swift` | host tes satu proses: `ensureStoresConfigured(nil, 64 MiB)` (spill mati, dan tidak bisa dikonfigurasi ulang), `makeStore(columns:rows:)`, dan pemeriksaan bahwa `stores` kembali ke nilai awal setelah tiap tes; pembantu `spillingHost(budgetBytes:)` yang membangun `EngineHost()` baru dengan direktori spill sementara (dihapus di `tearDown`) untuk tes dan bench yang membutuhkan spill nyata (§17.1, R-39) | P0 |
| `app/Tests/QueryHiveTests/ArrayRowsReference.swift` (pindahan) | `ArrayRows` acuan | P0 |
| `app/Tests/QueryHiveTests/SwiftGridReference.swift` (pindahan) | `GridSort.order` dan kawan-kawannya, `ColumnFilter.matches`, `matchesText`, `distinctValues(in:column:)`, dan `GridSearch.matches` | P0 |

**Diubah:**

| File | Perubahan | Commit |
|---|---|---|
| `Models/ResultRows.swift` | extension bawaan, `distinctValues` async, `CellText.make`; `ArrayRows` keluar; impor FFI sisa dibuang | 6a, lalu 6b |
| `Views/GridRowView.swift` | `GridRowText.first`, `GridRowPainter.paint(columns:)`, `GridRowTextCache` per rentang | 6a |
| `Views/GridTableView.swift` | `draw` meneruskan rentang; display link | 6a (rentang), 6b (link) |
| `Views/ResultGridTable.swift` | `rowText(_:columns:)`, `rowsDidGrow(from:to:)`, `pollRows`, `prepare(formats:)` di `reloadFormats` dan `formatChanged`, salin lewat `rowsOrThrow` | 6a, 6b |
| `Views/ResultGrid.swift` | `summaryText`, `filterEditor` (`.task`), banner galat, `preview.rows` menjadi `tab.result` dan `fetchedRows` | 6a, 6b |
| `Views/CellValueViewer.swift`, `Views/Panels.swift`, `Views/SettingsView.swift` | pembaca nilai lewat `fullValue`; badge memakai `fetchedRows`; teks plafon | 6b, 6c |
| `Models/CellSelection.swift` | `GridClipboard.text(result:…)` memakai `rowsOrThrow`; impor FFI sisa dibuang | 6b |
| `Models/QueryTab.swift`, `Models/AppModel.swift` | §17.4, §18, §19: `PreviewResult`, `activeResult` dan `baseResult`, `viewSpec`, `viewBusy`, `runPreview`, `explain`, `closeTab`, `selectTab`, `clearSort`, `clearSearch`, `footerCountInterval` | 6b |
| `Models/GridSort.swift`, `Models/GridSearch.swift` | implementasi acuan keluar (§17.5) | 6b |
| `Models/WritePlan.swift` | tanpa perubahan tanda tangan (`RowReading`); baris yang gagal dibaca menjadi peringatan, bukan `continue` diam-diam (`WritePlanTests` ditambah); `row(at:)`, yang kini bisa mahal, tidak dipanggil di loop yang panas | 6b |
| `Support/DatabaseEngine.swift`, `Support/RustEngine.swift` | §17.1 | 6b |
| `Support/Snapshot.swift`, `Support/BenchMode.swift` | `showRows` dan `ensureStoresConfigured`; `scroll-30x1m` lewat `store_from_rows`; `tabs-100-held`; hitungan fd | 6b |
| `Support/PerfSignposts.swift` | `firstRowsEvent` dari `poll()` pertama | 6b |
| `App.swift` | §17.6: konfigurasi store, sapuan, `RLIMIT_NOFILE` | 6b |

Tes yang berubah: `ResultRowsTests`, `GridTestSupport`, `SortFixtureExport`, `Bench/GridBenchTests` (digantikan `StoreWindowBench`), `VisualParityTests` (dua pembacaan `displayedRows`, di sekitar baris 603 dan 668, menjadi `tab.result.rows(in:columns:)`; **berkas ini tidak ada di daftar `development-plan.md` W6-T1**), `Batch7Tests`, `GridColumnsTests`, `ResultGridTests`, `CellEditUndoTests`, `FilterPresetTests`, `PanelDefaultTests`, `StoppedRunTests`, `TabCloseTests`, `MockEngine`, `EngineContract`, dan `RowLimitSettingTests` (6c). Itu mencakup 21 situs `PreviewResult(` di 8 berkas (TM-5).

**Urutan kerja:**

1. Catat baseline: jumlah tes Swift (603 dengan 5 dilewati pada `465a0c6` menurut ledger) dan G-VIS 17/0 pada HEAD.
2. **6a.** Tes dulu: `rowText` atas hasil 500 kolom memanggil `cell` hanya untuk rentang yang diminta; piksel painter berentang sama dengan painter penuh pada dirty rect yang sama; `distinctValues` untuk 11 nilai unik menjawab `more`. Lalu perubahan. G-VIS harus hijau tanpa rekam ulang sebelum lanjut. Commit.
3. **6b**, dalam urutan ini: `TestStores` dan `ensureStoresConfigured`; `StoreRows` dan `StoreRowsTests` (handle palsu dan host nyata); metode `DatabaseEngine` dan `RustEngine`; `QueryTab` dan `AppModel` (`runPreview`, `explain`, `viewSpec`, penjaga edit, `closeTab`); `Snapshot`, `BenchMode`, dan `App.swift`; display link; **penghapusan kode lama paling akhir** (§17.5) bersama penulisan ulang tes.
4. Tes kembaran format: korpus per format. Format yang selisih di luar divergensi tercatat (§17.2) masuk `swiftRenderedFormats`, dan hasilnya dicatat di pesan commit. `Raw` tidak pernah pindah: selisih `Raw` di luar divergensi tercatat adalah bug dan menghentikan tugas.
5. `StoreWindowBench`: 64 × 32 dan 128 × 32, resident dan tumpah (yang tumpah lewat `EngineHost()` sendiri, §17.1). Angkanya dicatat dan menjadi masukan W5-T3 untuk `PAGE_ROWS` (D-22).
6. G-LEAK (termasuk `tabs-100-held`), G-BENCH(1 S2, 2, 3), `scroll-30x1m` dan `scroll-500x10k` terhadap angka Fase 5. Bila `store_from_rows` untuk `scroll-30x1m` melewati 60 detik atau 8 GiB RSS, berhenti dan laporkan (R-38).
7. **6c** menunggu P-1. Bila belum terukur, W6-T1 selesai tanpa 6c dan dilaporkan sebagai "tertunda: P-1".

**Kriteria selesai** (AGENTS.md): berkas sesuai daftar kepemilikan (tambahan di atas dicatat di ledger), setiap gate yang disebut hijau, tinjau satu putaran terpenuhi, dan di-commit di `work/perf-parity`. Yang tertunda (P-1, 6c, angka W5-T3) dilaporkan terpisah dari yang selesai.

**Di luar W6-T1:** prefetch (D-22), `WindowedRows` (bersyarat P-1), `Json` kembali ke Rust (B-21), ekspor `store_synthetic` (R-38), jumlah kumulatif untuk `edges(of:)`, dan CLI, MCP, serta golden, yang tidak tersentuh.

### 21.5 W7-T1: driver menulis array Arrow (§16.4)

| Berkas | Perubahan | Prioritas |
|---|---|---|
| `crates/qh-driver/src/lib.rs`, `crates/qh-driver/Cargo.toml` | `Cursor::next_chunk(&mut ChunkBuilder, max_rows)` dengan default lewat `next_batch`; dependensi `qh-columnar` | P0 |
| `crates/qh-driver-postgres/src/normalize.rs` | `ColumnParser` per kolom (hasil `base_type` sekali), parse ke append bertipe, teks gagal → `push_value(Text)` | P0 |
| `crates/qh-driver-postgres/src/lib.rs` | `next_chunk` tanpa `Vec<Vec<Value>>` dan tanpa transpose; `next_batch` tetap untuk ekspor dan NDJSON | P0 |
| `crates/qh-driver-mysql/src/lib.rs` (dan modul decode-nya) | `next_chunk` dengan parser per kolom | P0 |
| `crates/qh-driver-trino/src/{lib.rs,decode.rs}` | `DeserializeSeed` streaming untuk `data` ke `ChunkBuilder`; komposit lewat `push_value` | P0 |
| `crates/qh-columnar/src/builder.rs` | Append yang dibutuhkan parser driver dan belum ada (misalnya `push_decimal_str`) | P1 |
| `crates/qh-ffi/src/commands.rs` | `StoreTarget` memakai `next_chunk` + `push_chunk`; `NdjsonTarget` tidak berubah | P0 |
| `crates/qh-driver-{postgres,mysql,trino}/tests/chunk_parity.rs` (baru) | `value_at(next_chunk)` = `next_batch` untuk `type_zoo` fake; varian live di G-LIVE | P0 |

`qh_core::ColumnarBuilder` dari rencana lama (`performance-plan.md` §11 butir 1) tidak dibangun. Verifikasi: G-RUST, G-GOLDEN (`type_zoo` live identik), G-LIVE, G-BENCH(2) A/B. Pemeriksa: RR, CR, PO.

### 21.6 W13-T8a (baru): helper analitik `queryhive-analytics`

| Berkas | Isi | Prioritas |
|---|---|---|
| `crates/qh-analytics-proto/{Cargo.toml,src/lib.rs}` (baru, anggota workspace utama) | Bingkai `QHA1`, pesan kontrol (`serde`), batas ukuran, versi protokol; tanpa DataFusion | P0 |
| `crates/qh-analytics-proto/tests/frames.rs` (baru) | §14.15 | P0 |
| `helpers/analytics/Cargo.toml` (baru) | `[workspace]` sendiri, `rust-version = "1.94"`, profil §14.1; paket `qh-analytics` (lib + `[[bin]] queryhive-analytics`); dependensi path `qh-core`, `qh-columnar` (fitur `from-arrow`), `qh-result-store`, `qh-rt`, `qh-analytics-proto`; `datafusion` (fitur §14.1), `tokio`, `futures`, `thiserror`; `forbid(unsafe_code)` | P0 |
| `helpers/analytics/Cargo.lock` (baru) | Lock sendiri, diawali salinan `Cargo.lock` utama | P0 |
| `helpers/analytics/src/lib.rs` | Doc modul: siapa memegang memori (sewa), siapa menulis ke disk (hanya spill helper), permukaan SQL yang dikunci, kurungan | P0 |
| `helpers/analytics/src/{main.rs,server.rs}` | Loop protokol di stdin/stdout, `Hello`, keluar saat EOF, `--self-test-confinement` | P0 |
| `helpers/analytics/src/session.rs` | Sesi, `SQLOptions` terkunci, daftar nama (§14.7) | P0 |
| `helpers/analytics/src/pool.rs` | `LeasePool: MemoryPool` (§14.5) | P0 |
| `helpers/analytics/src/spill.rs` | `EncryptedTempFiles`, `EncryptedSpillFile`, writer bertampung 1 MiB, kunci helper (§14.8) | P0 |
| `helpers/analytics/src/remote_table.rs` | `RemoteStoreTable` dan `ExecutionPlan` yang menarik chunk dengan kredit (§14.4) | P0 |
| `helpers/analytics/src/{files.rs,udf.rs,output.rs}` | `register_file` (§14.11), `qh_natural` (§14.10), keluaran → `from_arrow` → `ResultChunk` (§14.12) | P0 |
| `helpers/analytics/tests/{remote_table,spill,pool,sql_surface,files,natural,confinement}.rs` | §14.15 | P0 |
| `crates/qh-columnar/src/from_arrow.rs`, `crates/qh-columnar/tests/from_arrow.rs`, `crates/qh-columnar/Cargo.toml` | Normalisasi §5.6 di belakang fitur `from-arrow` (menyalakan `arrow-cast`) | P0 |
| `crates/qh-result-store/src/spill.rs` | `SpillCipher` dan `SpillFile` dibuka `pub` untuk helper; tag domain `QHD1` | P0 |
| `crates/qh-rt/src/lib.rs` | Builder runtime tokio ber-QoS untuk helper, dan tes QoS-nya | P0 |
| `Cargo.toml` (root), `Cargo.lock` | Anggota `crates/qh-analytics-proto`; `exclude = ["helpers/analytics"]`. `rust-version` tetap 1.85. | P0 |

Verifikasi: G-RUST (workspace utama, tanpa DataFusion), G-DENY untuk dua workspace, dan G-ANALYTICS (baru, §26), termasuk ukuran helper stripped dan terkompresi, dicatat terhadap §2.2. Pemeriksa: RR, SEC (§14.7, §14.8, §14.9), SF, AR. Commit: `feat(analytics): a separate DataFusion helper that runs SQL over results inside a leased budget and encrypted spill`.

### 21.7 W13-T8b (baru): klien helper di app, unduh, dan verifikasi

| Berkas | Isi | Prioritas |
|---|---|---|
| `crates/qh-ffi/src/analytics/{mod.rs,client.rs,serve.rs,lease.rs}` (baru) | Spawn lewat `sandbox-exec`, SHA-256 ulang, handshake, thread baca/tulis, melayani chunk, sewa, cancel, crash, replay, idle (§14.3–§14.6) | P0 |
| `crates/qh-ffi/src/analytics/profile.sb` (baru) | Profil kurungan (§14.9) | P0 |
| `crates/qh-ffi/src/analytics_api.rs` (baru), `crates/qh-ffi/src/{host.rs,lib.rs,uniffi_api.rs}`, `crates/qh-ffi/Cargo.toml` (`qh-analytics-proto`) | Permukaan §14.13 | P0 |
| `crates/qh-result-store/src/registry.rs` | `query_share_bytes`, `reserve_query` → `QueryLease` (§9.4) | P0 |
| `crates/qh-result-store/src/logical.rs` | Penyajian chunk logis dengan proyeksi untuk permintaan helper | P1 |
| `crates/qh-ffi/tests/analytics_api.rs` (baru) | §14.15 | P0 |
| `crates/qh-ffi/examples/bench_ffi.rs` | `sql-scan-500k` (throughput pipa), `sql-sort-2m`, `sql-group-500k` (waktu, `phys_footprint` app + helper) | P1 |
| `app/Sources/QueryHive/Support/AnalyticsComponent.swift` (baru) | Unduh, verifikasi (EdDSA CryptoKit, panjang, SHA-256, `SecStaticCodeCheckValidity`), pasang, hapus, pasang dari berkas, bersihkan build lama (§14.2) | P0 |
| `app/Sources/QueryHive/App.swift` | `signal(SIGPIPE, SIG_IGN)`; `configure_analytics` bila terpasang (§17.6) | P0 |
| `app/Sources/QueryHive/Support/RustEngine.swift` | `AnalyticsSession` lewat pengimpor FFI tunggal | P0 |
| `app/Tests/QueryHiveTests/{AnalyticsComponentTests,AnalyticsSmokeTests}.swift` (baru) | §14.15 | P0 |
| `app/build.sh` | `ANALYTICS=1`: bangun helper, kompres, `sign_update`, tulis kunci `QHAnalytics*` ke `Info.plist`, dan tanda tangani helper dengan loop yang sama | P0 |
| `app/build-dmg.sh` | Helper ikut ditandatangani Developer ID dan dinotarisasi pada `--developer-id`/`--notarize` | P1 |
| `app/release.sh` | Aset helper di rilis yang sama; periksa URL asetnya setelah rilis naik (invariant #10) | P0 |
| `app/Generated/*` | Regenerasi (invariant #1) | P0 |

Verifikasi: G-RUST, G-FFI, G-SWIFT, G-APP, G-ANALYTICS, dan `./app/release.sh --dry-run`. Pemeriksa: RR, SEC (§14.2, §14.4, §14.9), SF, SR, AR, CR. Commit: `feat(app): fetch, verify and confine the analytics helper, and hand it results chunk by chunk`.

### 21.8 W13-T8c (baru): hook komponen di Settings

| Berkas | Isi | Prioritas |
|---|---|---|
| `app/Sources/QueryHive/Views/SettingsView.swift` | Panel Analitik: status (`analytics_status`), ukuran unduhan, Unduh, Pasang dari berkas…, Hapus, alasan bila tidak tersedia | P0 |
| `app/Sources/QueryHive/Models/AppModel+Analytics.swift` (baru) | Lembar izin saat pemakaian pertama, kemajuan unduh yang bisa dibatalkan | P0 |
| `app/Tests/QueryHiveTests/AnalyticsSettingsTests.swift` (baru), scene snapshot baru | Status dan tombol untuk: tidak tersedia di build ini, belum terpasang, terpasang, rusak, dimatikan setelah crash | P0 |

Harus selesai sebelum W13-T7 (lokalisasi, serial dan terakhir), supaya string barunya lewat katalog. Verifikasi: G-SWIFT, G-VIS (scene baru). Pemeriksa: SR, UX, AX. Commit: `feat(app): an Analytics pane that installs, repairs and removes the optional helper`.

## 22. Urutan build

1. **W4-T3a, `qh-columnar`:** encoding, builder, codec bertag (dengan perbaikan B-5), `value_at`, tes round trip. Gate: G-RUST, G-DENY. (selesai, `e595877`)
2. **W4-T3b, store dan spill:** `chunk.rs`, `store.rs`, `registry.rs`, `spill.rs` (rekaman `QHP1`), tes NFR-S3. Gate: G-RUST, SEC. (selesai, `e595877`)
3. **W4-T3c, render, view, logis:** `qh-core` `write_text` (golden hijau lebih dulu), `collate.rs`, `render.rs`, `view.rs`, `logical.rs`, `qh-rt::view_pool`, tes window, view, dan skema logis. Gate: G-RUST, G-GOLDEN. (selesai, `e595877`)
4. **W4-T4, fixture:** ekspor dari Swift (implementasi Swift masih ada), pemeriksa Rust, divergensi didaftar. (selesai, `b1a1838`; hanya `differential.json`, §21.2)
5. **W5-T2, integrasi engine:** `events.rs`, lalu `commands.rs` (golden hijau **sebelum** `StoreTarget`), lalu `store_api.rs` dan `host.rs`, regenerasi `app/Generated`, smoke Swift, `bench_ffi window` dan `view-*`. (selesai, `df445fd` dan `bfb9680`; backlog tinjauannya ditutup W5-C di `1b59154` dan `465a0c6`)
6. **Disegarkan W6-A1 (2026-10-06).** **W6-A1** (penyegaran seam Fase 5; selesai, dokumen ini), lalu **W6-T1** dalam tiga commit (§21.4): 6a (refactor grid di atas `ArrayRows`), 6b (store dan penghapusan kode lama paling akhir), dan 6c (plafon 5.000.000, bersyarat P-1).
7. **W7-T1:** `next_chunk` di `qh-driver`, lalu PostgreSQL, MySQL, Trino, masing-masing dengan tes paritas chunk, lalu `StoreTarget`.
8. **W13-T8a,** setelah W13-T4 dan di lajur Rust sendiri: `qh-analytics-proto`, `from_arrow`, workspace helper (server, pool, spill, tabel jarak jauh, sesi, berkas, UDF, keluaran), tes kurungan, G-ANALYTICS.
9. **W13-T8b,** di lajur FFI setelah W13-T4 dan W13-T8a: klien, sewa, `analytics_api.rs`, regenerasi, `AnalyticsComponent.swift`, `App.swift`, `build.sh`, `release.sh`, tes.
10. **W13-T8c,** setelah W13-T8b dan sebelum W13-T7.
11. **Dokumen:** ADR-0037 (W4-D), ADR-0030 dan 0034 (W6-D), ADR-0045 dan adendum 0007, 0010, 0014 (W13-D). Rinciannya di §23.

## 23. ADR yang terdampak

| ADR | Nasib | Isi |
|---|---|---|
| 0008 store kolumnar kustom | **Digantikan** oleh 0030 | Store kustom tidak dibangun. Alasan 0008 dijawab satu per satu: (1) jendela acak dilayani `partition_point` + indeks array, (2) spill kita tulis sendiri dan dienkripsi, dan IPC menggantikan layout buatan sendiri, (3) `Decimal128` memetakan `i128 + scale` tanpa konversi per halaman untuk skala seragam, (4) dependensinya terukur (§2.2). Arrow C Data Interface ke Swift tetap tidak dipakai (D-6). |
| 0013 throughput terikat JSON | **Digantikan** oleh 0030 | Tidak berubah dari rencana: klaim 99,6% diganti profil W1-T8. |
| 0004 UniFFI control plane | Diamandemen oleh 0030 | Tidak berubah dari rencana: data plane = `ResultHandle` + buffer `QHW1`. Ditambah `AnalyticsSession` (W13-T8b) sebagai objek UniFFI kedua di data plane; DataFusion sendiri di proses lain (0045). |
| 0030 data plane app | Dibuat (W6-D), isinya berubah | Store = `RecordBatch` Arrow per chunk dengan encoding per chunk-kolom (D-1, D-2), `window()` terender (D-6), Explain di store (D-8), `RESULT_SINK` (D-11), handle sebelum run (D-12), amandemen D-1 Fase 2 (metode store yang melempar galat), dan crate `qh-columnar` (D-15). **Disegarkan W6-A1 (2026-10-06):** ditambah D-22 sampai D-29 (halaman jendela, rentang kolom, pintu darurat format, kembaran acuan, batas fd, penjaga edit, kadens footer, plafon) dan angka §2.9. |
| 0034 view in-memory di Rust | Dibuat (W6-D), isinya berubah | View grid = rayon atas array Arrow, dan tidak bergantung pada helper (D-8, O-18). Satu fungsi kunci natural memcmp-able, juga dipakai UDF `qh_natural` di helper. Baris "DataFusion ditolak" diganti "DataFusion untuk SQL di helper opsional, bukan untuk view grid", dengan angka §2.7 **dan catatan jumlah threadnya**. Daftar divergensi O-9 ditambah divergensi SQL (D-18). **Disegarkan W6-A1 (2026-10-06):** ditambah angka view W5-T2 (§2.9) dan D-27 (view asinkron dengan penjaga edit). |
| 0037 spill terenkripsi | Dibuat (W4-D), diperluas | D-5 dan §10 apa adanya, ditambah plaintext `QHP1` (IPC + flag), tag domain AAD `QHS2`/`QHD1`, aturan "satu kunci per proses yang menulis spill; kunci tidak pernah menyeberang proses", dan spill operator helper lewat `DiskManagerMode::Custom` (§14.8). Mode `OsTmpDirectory` dilarang. |
| 0045 DataFusion sebagai komponen analitik terpisah | **Baru** (W13-D) | Keputusan pemilik O-15 dan O-18. Letak (workspace dan proses terpisah, D-16), pengiriman dan verifikasi (D-19, §14.2), protokol dan serah terima data (D-20, §14.3–§14.4), sewa anggaran (§14.5), cancel dan crash (§14.6), SQL yang dikunci (§14.7), kurungan dan jalur migrasi ke App Sandbox (D-21, §14.9), fitur minimal dan lisensi (§14.1, §24), angka build dan ukuran (§2.2), MSRV 1.94 hanya di workspace helper (F-2). Nomor 0045 adalah nomor kosong berikutnya setelah 0044 (`development-plan.md`, jadwal ADR). |
| 0007 tanpa App Sandbox | Adendum (W13-D) | App tetap tanpa sandbox. Helper analitik dikurung `sandbox-exec` (D-21). |
| 0014 entitlement app | Adendum (W13-D) | Helper tidak butuh entitlement apa pun, termasuk di hardened runtime. Jalur App Sandbox untuk helper menunggu sertifikat Developer ID. |
| 0010 QoS | Adendum | Runtime tokio helper ber-QoS `USER_INITIATED`, di samping `view_pool()`. |
| 0018 writer Parquet native | Tetap | Sejak O-18 unifikasi fitur `parquet` hanya terjadi di workspace helper (F-3), jadi catatan ADR ini dan komentar `qh-export` tetap benar. |
| 0002 lisensi MIT, 0011, 0012 | Tetap | `deny.toml` tidak berubah untuk fitur §14.1 (§24). |
| 0009 panic unwind | Tetap | Semua ekspor baru throwing, termasuk `AnalyticsSession`. |

## 24. Lisensi dan kebijakan `deny.toml`

Diperiksa dengan `cargo deny --offline [--features X] check licenses` di probe, dengan salinan `deny.toml` repo apa adanya. Graf probe = `qh-ffi` hari ini ditambah fitur yang diuji.

| Konfigurasi | Hasil | Crate baru | Ekspresi lisensi baru |
|---|---|---|---|
| `qh-ffi` saja (acuan) | lulus | — | — |
| + gugus arrow store | lulus | 18 | `Apache-2.0` (8), `MIT/Apache-2.0` (6, keluarga `lexical-*`), `MIT OR Apache-2.0` (2), `Apache-2.0 AND MIT` (`arrow-array`), `MIT` (`atoi`) |
| + DataFusion fitur §14.1 | **lulus** | 88 | Di atas, ditambah `BSD-3-Clause` (`snap`, `zstd-safe`, `zstd-sys`, `alloc-no-stdlib`, `alloc-stdlib`), `BSD-3-Clause AND MIT` (`brotli`), `BSD-3-Clause/MIT` (`brotli-decompressor`), `Unlicense/MIT` (`walkdir`, `same-file`; MIT yang dipakai), `Apache-2.0 OR MIT`, `MIT` (`comfy-table`, `dashmap`, `lz4_flex`, `phf`, …). Semuanya sudah di allow-list. |
| + DataFusion fitur bawaan | **gagal, 2 crate** | 107 | `ar_archive_writer` 0.5.3: `Apache-2.0 WITH LLVM-exception`, dependensi build `psm` ← `stacker` ← `recursive` (fitur `recursive_protection`); `libbz2-rs-sys` 0.2.5: `bzip2-1.0.6`, lewat `bzip2` (fitur `compression`). Lainnya lulus: `blake3` (`CC0-1.0 OR Apache-2.0 OR …`, Apache dipakai), `constant_time_eq` (`CC0-1.0 OR MIT-0 OR Apache-2.0`). |

Sejak O-18, graf helper adalah bagian dari graf probe DataFusion §14.1 (tanpa `qh-ffi` dan driver-nya), jadi hasil "lulus" berlaku untuknya. Graf app hanya bertambah gugus arrow. G-DENY dijalankan untuk dua workspace: `cargo deny check licenses` dan `cargo deny --manifest-path helpers/analytics/Cargo.toml check licenses --config deny.toml`. Karena helper didistribusikan sebagai berkas sendiri, backlog B-12 (belum ada berkas pemberitahuan pihak ketiga) berlaku untuk dua artefak, app dan helper.

**Usulan kebijakan:** tidak ada perubahan `deny.toml`. Fitur DataFusion dipatok ke set §14.1, dan komentar di `helpers/analytics/Cargo.toml` menyebut kenapa `recursive_protection` dan `compression` mati. Bila salah satunya kelak dibutuhkan, jalurnya ADR dan satu entri `exceptions` per crate, seperti ADR-0011, 0012, dan 0018, tidak pernah lewat `allow`:

- `{ allow = ["Apache-2.0 WITH LLVM-exception"], crate = "ar_archive_writer" }`. Pengecualian LLVM hanya melonggarkan Apache-2.0, dan crate ini hanya dipakai saat build, tidak ikut ke binari.
- `{ allow = ["bzip2-1.0.6"], crate = "libbz2-rs-sys" }`. Lisensi permisif gaya BSD tanpa copyleft. Hanya dibutuhkan bila CSV `.bz2` harus dibaca.

`tiny-keccak` (CC0, ADR-0018) tetap dipakai lewat `const-random` ← `ahash`, jadi pengecualiannya tidak menjadi `license-exception-not-encountered`.

## 25. Risiko

| # | Risiko | Mitigasi |
|---|---|---|
| R-1 | Kunci natural berbeda dari `localizedStandardCompare` | O-9 menerimanya. Fixture dan `KNOWN_DIVERGENCES` membuatnya eksplisit, lalu masuk ADR-0034. |
| R-2 | Comparator yang tidak total membuat sort Rust panic (≥ 1.81) | Desain leksikografis dengan pemutus byte dan baris, tes totalitas berbenih, dan `guarded()` sebagai jaring terakhir |
| R-3 | Miss sinkron atas chunk yang tumpah (dekripsi ≤ 2 MiB di main) menyebabkan hitch | **Diputuskan W6-A1 (2026-10-06):** tetap sinkron di main, tanpa prefetch dan tanpa sel kosong sementara (D-22). Batas 2 MiB per chunk, cache dekripsi 8 chunk, dan satu chunk `wide_500k` yang melayani sekitar 55 halaman membuat miss tumpah jarang dan murah (sekitar 0,4 ms). Bila W6-T2 mengukur frame > 1 ms karena itu, prefetch ditambahkan dengan aturan simpan ≥ 10%. |
| R-4 | Akuntansi memori meleset (chunk yang disemat, builder terbuka, kapasitas buffer Arrow) | Chunk dengan `strong_count > 1` tidak di-evict. Builder ≤ 2 MiB per store aktif dinyatakan di doc. `shrink_to_fit` sebelum segel (probe menunjukkan kapasitas builder menambah sekitar 12% tanpa itu, §2.4), dan `get_array_memory_size` menghitung kapasitas, bukan panjang. Tes "resident ≤ anggaran + satu chunk" dan G-BENCH(3). |
| R-5 | Ongkos UniFFI per jendela > 0,5 ms | **Disegarkan W6-A1 (2026-10-06):** sisi Rust 128 × 30 sudah 0,396–0,449 ms p99 (§2.9). Halaman 64 × 32 (D-22) sekitar 0,22–0,24 ms, dan `StoreWindowBench` mengukur penyeberangan UniFFI. Bila p99 64 × 32 > 0,5 ms, eskalasi C ABI Fase 8 (W8-T2). |
| R-6 | Balapan antara publikasi chunk dan perluasan view saat streaming | `Release`/`Acquire` pada `rows`, indeks append-only, satu tugas perluasan dalam satu waktu, tes penulis konkuren |
| R-7 | Handle dilepas saat `set_view`, prefetch, ingest, atau permintaan chunk helper masih berjalan | Fase atomik, cancel view saat release, `Released` → cancel di pump, "the result was closed" untuk helper, galat basi diabaikan Swift. Tes di `view.rs`, `store_sink.rs`, dan `analytics_api.rs`. |
| R-8 | Berkas yang di-unlink tidak terlihat di `du` atau Finder | `store_stats()` di diagnostik, dan pesan disk penuh yang eksplisit |
| R-9 | Stream yang menetes menghasilkan banyak chunk kecil | Dibiarkan di Fase 6. Kompaksi ditinjau bila jumlah chunk > 4.096 terukur. |
| R-10 | Kripto salah pakai (nonce, AAD, kunci) | Penghitung per kunci, satu kunci per proses, AAD dengan `store_id` dan indeks, tes §10.5 dan §14.15, dan `security-reviewer` di W4-T3 dan W13-T8a |
| R-11 | Hitungan grapheme berbeda versi Unicode antara `unicode-segmentation` dan Swift | `width.json`. Selisih pada rangkaian langka hanya menggeser lebar kolom, dan batasnya 320 pt. |
| R-12 | Paritas validator JSON dan printer format `Json` | Fixture dua sisi. Pintu darurat `Json` (§11.4) diputuskan di W4-T4. |
| R-13 | Explain lewat store menyimpang dari `performance-plan.md` §10 butir 5 | Dicatat di D-8 dan ADR-0030. Golden `explain` dijaga G-GOLDEN. |
| R-14 | Blueprint Fase 2 dan Fase 5 belum ada; nama `EngineHost`, `ResultRows`, `CellText` bisa berbeda | **Teratasi W6-A1 (2026-10-06):** kedua blueprint ada dan sudah dibangun (W3-T1, W5-T1); §12 dan §17 disegarkan terhadap kode nyata (§1.3, §12.4, §17.0). |
| R-15 | Daftar berkas di `development-plan.md` belum memuat `qh-columnar`, `logical.rs`, tes kontrak skema logis, W7-T1 versi Arrow, dan W13-T8a–c | Orkestrator memperbarui `development-plan.md` sebelum W4-T3 mulai. Daftarnya di §26. |
| R-16 | `Decimal` Swift 38 digit berbeda dengan desimal eksak | `NumKey` meniru 38 digit, dan aturan pembulatannya dikunci `number.json` |
| R-17 | Rayon view bersaing CPU dengan ingest di P-core | View hanya fallback dan jarang. Pool terpisah dari runtime tokio. Diukur di sesi bench W6-T2. |
| R-18 | Disk penuh di tengah streaming | Event `error`, store lain utuh, tanpa crash (tes W4-T3 dan W5-T2) |
| R-19 | Satu fd per store yang tumpah; banyak tab di latar mendekati batas lunak 256 fd app GUI | **Diputuskan W6-A1 (2026-10-06), D-26:** naikkan `RLIMIT_NOFILE` lunak ke min(batas keras, 4.096) saat peluncuran. Menutup fd store yang menganggur tidak mungkin, karena berkas di-unlink (D-5). `EMFILE` tetap menjadi galat seperti disk penuh: chunk tetap resident, store lain utuh (§10.4). `tabs-100-held` mengukur puncak fd (§19). |
| R-20 | Ukuran binari app naik (§2.2) | O-18: DataFusion tidak masuk bundel. App hanya menambah gugus arrow (≤ 7,35 MB stripped, batas atas), dicatat di W5-T2 terhadap §2.2. |
| R-21 | Waktu build, relink, dan disk naik (§2.2) | DataFusion hanya di workspace helper; G-RUST, G-FFI, dan G-SWIFT tidak membangunnya. G-ANALYTICS hanya di W13-T8a–b dan gate W13–W14, dengan debuginfo `line-tables-only`. |
| R-22 | API DataFusion berubah di setiap major, majornya sering, dan `TempFileFactory` masih baru | Versi dipatok `55.1` dan dinaikkan bersama `arrow`/`parquet` sebagai satu tugas. Churn API terkurung di helper; yang dilihat app hanya protokol `QHA1` dan IPC Arrow. Tes helper (§14.15) gagal keras bila antarmukanya berubah. |
| R-23 | Alokasi DataFusion di luar `MemoryReservation` dan memori dasar proses helper membuat NFR-P3 meleset saat SQL berjalan | Sewa paling banyak setengah anggaran (D-4), kredit permintaan membatasi batch transit, dan `bench_ffi sql-*` mengukur `phys_footprint` app + helper. Ambang NFR-P3 untuk SQL ditetapkan dari angka itu di gate W13. |
| R-24 | Spill DataFusion membaca tanpa validasi (F-4) | Byte hanya sampai ke decoder setelah AEAD `open` berhasil. Tes manipulasi di §14.15. |
| R-25 | Unifikasi fitur `parquet` membawa codec dan C (`zstd-sys`) ke graf helper (F-3) | Hanya di workspace helper; `.cargo/config.toml` (invariant #4) tetap mematok `MACOSX_DEPLOYMENT_TARGET` untuk objek C. |
| R-26 | Chunk-kolom bertag membuat SQL melihat teks, bukan tipe (satu `'NaN'` di kolom `numeric` hanya membuat satu chunk bertag, tetapi skema logis kolom itu menjadi `Utf8`) | Disengaja: tidak ada data yang diubah diam-diam (§5.4). `TableInfo` memberi tahu UI tipe logis tiap kolom, dan pengguna bisa `TRY_CAST`. |
| R-27 | Kunci natural memcmp-able memakan memori (sekitar 50 byte per baris untuk `row-N-c01`) | Dicadangkan lewat `reserve`, dengan jalur prefiks 20 byte per baris saat cadangan tidak cukup. Hasilnya identik dan dijaga tes. |
| R-28 | Jendela bertipe mendekati NFR-P8: 128 × 30 penuh tipe 442 µs p99 di sisi Rust sebelum UniFFI dan format kolom (§2.6) | `write_text` tanpa alokasi di W4-T3 (D-6), dengan target lokal ≤ 250 µs. Hex hanya untuk prefiks yang tampil (§11.2). **Diputuskan W6-A1 (2026-10-06):** target lokal ≤ 250 µs meleset (0,396–0,449 ms, §2.9), jadi halaman 64 baris dipilih (D-22), sementara sampai W5-T3 mengonfirmasi; eskalasi C ABI Fase 8 bila p99 64 × 32 masih > 0,5 ms. |
| R-29 | `wide_500k` ≈ 250 MiB di Arrow setelah `shrink_to_fit`, di tepi anggaran 256 MiB (§2.4) | Spill memang dirancang untuk ini. G-BENCH(3) mencatat `spilled_bytes` di samping memori, supaya hasil 500k yang tumpah atau tidak tidak dibaca sebagai regresi. |
| R-30 | Unduhan helper gagal atau tidak mungkin (tanpa jaringan, jaringan tertutup, GitHub tidak terjangkau) | "Pasang dari berkas…" dengan verifikasi yang sama (§14.2). Semua fungsi di luar analitik tidak terpengaruh (§14.14). |
| R-31 | `sandbox-exec` usang dan bisa hilang di macOS mendatang | Tanpa kurungan, helper tidak dijalankan, dengan alasan yang terlihat (§14.9). Jalur App Sandbox dicatat di ADR-0045. Tes kurungan gagal keras di macOS yang mengubah perilakunya. |
| R-32 | Throughput pipa macOS terlalu rendah untuk scan besar | Target ≥ 1 GB/s diukur di W13-T8b. Pengganti `UnixStream::pair()` tidak mengubah protokol (§14.4). |
| R-33 | App tanpa `SIGPIPE` diabaikan mati saat helper crash | `signal(SIGPIPE, SIG_IGN)` di `App.swift` (§17.6), dan tes "helper dibunuh di tengah query" di `analytics_api.rs` dan smoke Swift |
| R-34 | Helper dan app berbeda versi | SHA-256 dipatok per build dan diperiksa ulang sebelum setiap spawn, ditambah handshake `protocol` + `build` (§14.2, §14.3) |
| R-35 | **Disegarkan W6-A1 (2026-10-06).** Edit menunjuk baris yang salah setelah `set_view` terpasang (D-27): `CellKey` memakai indeks baris tampilan, dan `WritePlan` membaca nilai asli untuk klausa `WHERE` | `viewBusy` menolak staging, pembangun rencana menolak saat sibuk, seleksi dan antrean dibuang saat view terpasang, dan tes untuk ketiganya (§17.4). Reviewer database memeriksa jalur ini |
| R-36 | **Disegarkan W6-A1 (2026-10-06).** Bit flag QHW1 salah dipetakan ke `CellFlags` (TM-7): `openable` dan `truncated` tertukar tanpa galat kompilasi | Pemetaan eksplisit di `WindowPage` dan tes satu sel per bit |
| R-37 | **Disegarkan W6-A1 (2026-10-06).** Paritas format di Rust belum teruji (TM-10): printer `Json` dan format `Text`, `Uuid`, `UnixTimestamp` bisa berbeda dari `ColumnFormat.render` | `swiftRenderedFormats` (awal `{.json}`), tes kembaran di W6-T1, dan B-21 untuk fixture dua sisi (D-24) |
| R-38 | **Disegarkan W6-A1 (2026-10-06).** `scroll-30x1m` membangun 30 juta sel lewat `store_from_rows` (TM-14): marshalling UniFFI satu kali dan `Vec<Vec<Option<String>>>` di Rust, beberapa GB sementara | Dibangun di luar interval ukur dengan anggaran bench 2 GiB; waktu dan puncak RSS dicatat. Bila lebih dari 60 detik atau 8 GiB, `store_synthetic` diekspor lewat UniFFI sebagai metode khusus bench (perubahan kecil di lajur FFI, bukan keputusan arsitektur baru) |
| R-39 | **Disegarkan W6-A1 (2026-10-06).** Tes bergantung pada host nyata: registry dikonfigurasi sekali per host dan tidak bisa dikonfigurasi ulang (`host.rs:319-323`), jadi host bersama proses tes tidak pernah bisa menumpahkan (spill mati, 64 MiB). Tes spill yang memakainya lulus tanpa menumpahkan apa pun, atau gagal menurut urutan tes | `TestStores` satu kali untuk semua yang resident, `ensureStoresConfigured` idempoten, tiap tes membuat dan melepas storenya sendiri dan memeriksa `stores` kembali ke awal. Tes dan bench yang butuh spill nyata (galat `.Spill`, miss chunk tumpah, `StoreWindowBench` tumpah) membangun `EngineHost()` sendiri dengan direktori spill sementara dan anggaran kecil (§17.1), atau berjalan lewat `--bench` |
| R-40 | **Disegarkan W6-A1 (2026-10-06).** Query tetap berjalan di server setelah tab ditutup (TM-4) | `closeTab` menghentikan `previewProcess` (§19), dengan tes di `TabCloseTests` |
| R-41 | **Disegarkan W6-A1 (2026-10-06).** `StoreFfiError.Spill`, `Corrupt`, atau `Internal` muncul di tengah scroll (disk penuh sesudah spill, rekaman rusak) | `lastFailure` menjadi banner grid dan sel kosong, tidak pernah crash atau `try!`; dites dengan handle palsu (§17.2) |

## 26. Yang disegarkan di W6-A1, dan catatan untuk orkestrator

**Disegarkan W6-A1 (2026-10-06).** W6-A1 selesai: dokumen ini disegarkan terhadap seam Fase 5 (W5-T1) dan permukaan engine (W5-T2). Tinjau architect-reviewer atas penyegaran ini menemukan lima koreksi blocking, B1 sampai B5, yang sudah diterapkan (bullet pertama di bawah). Ronde 2 menemukan satu koreksi blocking lagi dan sudah dikoreksi (catatan "Verifikasi ronde 2" di akhir bagian ini). Putaran review habis (O-19 dan O-20), jadi tidak ada verdict akhir; statusnya pending review (pemeriksa menurut `development-plan.md` W6-A1: `architect-reviewer`).

- **Koreksi blocking dari tinjau architect-reviewer atas penyegaran ini (2026-10-06).** B1 baseline: dokumen diperiksa ulang terhadap `465a0c6`, setelah `1b59154` dan `465a0c6` masuk (§1.3 TM-2 sampai TM-9, nomor baris di fakta 10, §12.4, §17 dan §17.1, §19 G-LEAK, §20, §21.3, §26). B2: halaman `StoreRows` dan blok `row(at:)` mencatat jumlah barisnya, dan `prepare(formats:)` membuang halaman blok yang formatnya berubah (§17.2). B3: `poll()` hanya mengikuti `viewID` yang sama, dan hop `apply` satu-satunya penulis `viewID` dan `count` (§17.2, §17.3). B4: host bersama tidak bisa menumpahkan, jadi tes dan bench spill membangun `EngineHost()` sendiri atau lewat `--bench` (§17.1, §21.4, R-39). B5: potongan 256 dan `TRUNCATED` milik `window` adalah divergensi tercatat, dan `Raw` tidak pernah pindah (D-24, §17.2).
- **Verdict AR** atas revisi Arrow dan O-18 ada di bagian berikut. `security-reviewer` tetap wajib untuk §10 di W4-T3 (selesai, putaran 2 tertunda), dan untuk §14.2, §14.8, dan §14.9 di W13-T8a–b.
- **Yang ditutup W6-A1:** pintu darurat `Json` (D-24, §11.4), nasib `ArrayRows` (D-25), `PAGE_ROWS` dan `COL_BLOCK` sementara (D-22), mitigasi R-19 (D-26), bentuk `StoreRows` (§17.2), dan plafon (D-29). Daftar yang dulu terbuka di verdict dan di `development-plan.md` §11 butir 7 kini terjawab; yang menunggu hanya angka halaman dari W5-T3.
- **Temuan yang tidak diminta tetapi mengubah rencana:** TM-2 (tabel membangun semua kolom), TM-3 (regresi pemilih nilai unik), TM-4 (`closeTab` tidak menghentikan Run), TM-7 (bit flag), TM-10 (tidak ada fixture paritas format), dan D-27 (balapan antara edit dan view).
- **Edit yang diminta dari orkestrator** (W6-A1 hanya menyentuh berkas ini):
  - `development-plan.md` §11 butir 7: tandai terjawab (D-22, D-24, D-25, D-26).
  - `development-plan.md` §5 W6-T1: pelaksana **sonnet** (O-20); tiga commit (6a, 6b, 6c); tambahkan ke berkas yang dimiliki `Views/GridTableView.swift`, `Views/GridRowView.swift`, `Models/ResultRows.swift`, `Models/CellSelection.swift`, serta tes `VisualParityTests.swift`, `ResultRowsTests.swift`, `SortFixtureExport.swift`, `GridTestSupport.swift`, `StoppedRunTests.swift`, `CellEditUndoTests.swift`, `FilterPresetTests.swift`, `PanelDefaultTests.swift`, `EngineContract.swift`, dan berkas baru §21.4; tinjau: model terkuat satu putaran ditambah reviewer database.
  - `development-plan.md` §7 (rantai kepemilikan): tambahkan `Views/GridTableView.swift` ke rantai `ResultGridTable`, `GridRowView`, `GridHeaderView`, dan `GridAccessibility` (W5-T1 → W6-T1 → W10-T1 …; `GridRowView` sudah ada di rantai tetapi tidak di daftar berkas W6-T1), dan `VisualParityTests.swift` ikut W6-T1.
  - `development-plan.md` §1 G-LEAK: tambahkan `open_fds_peak` dan `tabs-100-held`; `spilled_bytes` sudah nyata (`1b59154`), jadi pemeriksaan `spilled_bytes == 0` berlaku apa adanya.
  - Ledger: B-21 (fixture `format.json` dua sisi; `Json` kembali ke Rust), cacat TM-3 dan TM-4 sebagai backlog yang ditutup W6-T1, dan TM-8 (komentar `column_widths()`, `store_api.rs:685`) sebagai backlog ledger: W5-C sudah ditutup tanpa memperbaikinya, dan daftar yang W5-C teruskan ke W6-C tidak memuatnya, jadi ia belum punya gelombang pemilik.
  - ADR-0030 dan 0034 (W6-D): masukkan D-22 sampai D-29 dan tabel §2.9.
- **Ledger:** B-5 (Unknown `raw`, sapuan, penimpaan spill) tetap milik W4-T3, sekarang lewat `qh-columnar/tagged.rs` dan `spill.rs`. B-12 (pemberitahuan pihak ketiga) kini mencakup helper.
- **Keputusan yang diserahkan ke pemilik lewat laporan akhir:** D-8 (view grid tanpa DataFusion), D-16 dan O-18 (DataFusion baru ada di W13-T8a–b, sebagai helper terpisah), D-19 (unduhan saat pertama dipakai; ukurannya diukur di W13-T8a), D-21 (kurungan `sandbox-exec`, dan fitur mati bila kurungan tidak tersedia), anggaran 256 MiB yang kini di tepi untuk `wide_500k` (R-29), serta dua keputusan W2-A3 yang tetap (Explain lewat store, unlink segera). Satu hal baru dari W6-A1: P-1 (izin Screen Recording atau sesi eksklusif) sebelum plafon 5.000.000 (D-29).
- **Verifikasi ronde 2 (2026-10-06): 1 blocking, dikoreksi; pending review (O-20).** Koreksi B3 di ronde 1 belum lengkap: teks `apply` (§17.2) dan §13.2 masih menggambarkan Rust yang membatalkan `set_view` lama dan menjawab `Superseded`, padahal di `store.rs` (`4ec7480`) `set_view` mengambil id di awal, membangun tanpa kunci, memasang build yang selesai paling akhir, dan hanya membatalkan view yang digantikannya; build yang lebih tua tidak pernah dibatalkan, dan jawabannya `Ok` dengan id lama, bukan `Superseded`. Pembangun yang mengikuti teks lama akan menampilkan baris filter atau sort yang sudah diganti pengguna, dengan `viewID` dan `count` yang bisa mundur. Dikoreksi terhadap `StoreRows.swift` di `work/perf-parity`: antrean serial `viewQueue` per store, dan `install` yang mengabaikan `viewId` lebih kecil (yang berubah: §13.2 langkah 4 dan 5 serta paragraf penutup, diagram alur data di §4, tanda tangan dan butir `apply` di §17.2, §19 langkah 4, dan satu baris baru di §21.4). Dua tes urutan yang sudah ada tinggal di `StoreLifecycleTests.swift`, bukan `StoreRowsTests.swift` seperti yang disebut temuannya, jadi barisnya ditambahkan untuk berkas itu.
- **Non-blocking ronde 2, belum disentuh dan masuk backlog gelombang berikutnya (pending review, O-20).** Kembaran memakai `NSRecursiveLock` (bukan `NSLock`) dan tidak ada `dispatchPrecondition` di `cell` dan `fullValue` (§17.2 bullet Thread); `cell()` mengadopsi format dan membuang blok itu, bukan hanya memeriksanya; divergensi ketiga NUMERIC (per nilai di Rust, per tipe kolom di kembaran, `StoreRowsTests.swift:396-404`) belum ada di daftar §17.2; satuan prefiks kembaran (§17.0 "unit" lawan §17.2 "Character"); deviasi §17.4 dan §18 yang dicatat di `1e3c0ad` (`viewBusy` bergenerasi, `rerunBaseSQL`, sort memori yang dibersihkan, bench per proses); `StoreWindowBench` memakai `spillingHost(1<<30)` untuk "resident"; dan fd spill yang belum dikembalikan saat `release` (blueprint benar, kodenya tertinggal, milik W6-T1/X2 G-LEAK dan `tabs-100-held`).

## Verdict architect-reviewer (Arrow/DataFusion)

**Verdict: disetujui dengan koreksi** (30 Sep 2026, atas O-15 dan O-18). Koreksi di bawah sudah diterapkan di dokumen ini. Inti Fase 6 (§1–§13, §15–§20, W4-T3 sampai W7-T1) siap untuk W4-T3 setelah orkestrator membuat edit di bagian "Edit yang wajib dibuat orkestrator". §14 siap untuk W13-T8a setelah `security-reviewer` memeriksa §14.2, §14.8, dan §14.9. Dampak arsitekturnya **tinggi**: format data plane berganti, dan satu proses baru dengan jalur unduh masuk ke produk. Tetapi batasnya bersih: Swift di W6 tidak berubah, NDJSON tidak berubah, dan helper bisa dicabut tanpa menyentuh store.

### Yang diperiksa

- Dokumen: brief aturan run, blueprint ini sebelum dan sesudah revisi, `performance-plan.md` §10, §13, §15, §17; PRD §6.1, §6.3, §6.4, §10, §11.1 (O-1 sampai O-13; O-15 dan O-18 dari ledger `target/run/ledger.md`); `development-plan.md` §1, §3, §4, W13, §6, §7; `docs/invariants.md` #4, #5, #10; `app/build.sh`, `app/release.sh`, `app/build-dmg.sh`, `app/QueryHive.entitlements`, `app/sparkle-public-key.txt`.
- Pengukuran: `bench-sort.log`, `bench-window.log`, `bench-render.log`, `bench-session.log`, `df-build.log`, `df-build2.log`, `deny-{none,arrow,df,dfdefault}.log`, `tree-df.txt`, dan sumber probe `df-probe/src/bin/bench.rs`.
- Sumber pihak ketiga di registry lokal: `datafusion-55.1.0/Cargo.toml` (`rust-version = "1.94.0"`, `object_store` tanpa fitur bawaan), `datafusion-execution-55.1.0` (`DiskManagerMode` empat varian; `object_store` hanya `fs`), `arrow-ipc-59.3.0/src/reader/stream.rs:125` (`with_skip_validation` memang `unsafe`), `datafusion-physical-plan-55.1.0/src/spill/mod.rs:96` (DataFusion memanggilnya di blok `unsafe`), `arrow-array-59.3.0` (`Array::shrink_to_fit` ada).
- Kode repo: `qh-core/src/render.rs` (`format_timestamp` membangun 4–5 `String`; `hex_encode` memanggil `format!` per byte), biner app di `app/dist` (23.666.336 byte) dan `queryhive-mcp` (10.856.384 byte), `SIGPIPE` hanya diabaikan di `BenchMode.swift:112`, seed `wide_500k`.

### Keputusan atas klaim brief

- **Arrow 59.3 sejajar `parquet` 59.3.** Benar (F-1). Setelah O-18 kesejajaran ini tidak lagi wajib lintas proses, karena yang menyeberang adalah IPC. Tetapi ia tetap disengaja, karena `qh-columnar` dan `qh-result-store` dikompilasi di dua workspace.
- **Encoding per chunk-kolom, `Binary` bertag untuk kolom campuran, B-5.** Sehat. Round trip `Value` tanpa kehilangan adalah syarat yang benar, karena driver memang mencampur varian (fakta 12), dan skema tetap per hasil akan memaksa pilihan antara pengecualian per sel dan kehilangan data. B-5 wajib di `qh-columnar/tagged.rs`, sebab `codec.rs:317-337` hari ini mengubah `Unknown` `raw` menjadi teks secara diam-diam.
- **Skema SQL yang disatukan, kolom campuran sebagai `Utf8` = teks grid.** Disetujui. Menolak NULL diam-diam adalah keputusan yang benar. Konversi fisik ke logis tetap di app (`logical.rs`), sehingga helper tidak perlu tahu `qh.enc` sama sekali, dan kontrak W4-T3 (`tests/logical.rs`) adalah persis apa yang diterima helper.
- **Spill IPC + AES-256-GCM.** Disetujui tanpa perubahan kripto: kunci per proses, nonce dari penghitung, AAD `QHS2 ‖ store_id ‖ chunk`, unlink sebelum tulisan pertama, `pread`, dan decode tanpa salinan dengan validasi menyala (`bench-window.log`: `zero_copy_decode=true`). Sejak O-18 aturannya menjadi "satu kunci per proses yang menulis spill": helper punya kuncinya sendiri, dan kunci app tidak pernah menyeberang (D-5, §14.8).
- **`DiskManagerMode::Custom` lewat cipher yang sama, spill teks biasa di `$TMPDIR` dilarang.** Disetujui, dengan koreksi letak: `Custom` dipasang di proses helper dengan cipher milik helper. Kegagalan kurungan atau direktori tidak aman menjadi `Disabled`, tidak pernah `OsTmpDirectory`.
- **`window()` (`QHW1`) dan Swift tidak berubah.** Benar untuk W5–W6. Swift baru berubah di W13-T8b–c (komponen, `SIGPIPE`, panel Settings), dan itu di luar seam grid.
- **`write_text` tanpa alokasi.** Wajib, dan diperluas ke hex. 442 µs p99 untuk 128 × 30 penuh tipe di sisi Rust, sebelum UniFFI dan format kolom, terlalu dekat dengan 0,5 ms. Target lokal ≤ 250 µs masuk akal: timestamp 288 ns × 640 sel saja sudah sekitar 184 µs, dan hampir semuanya alokasi. Blob `bytes` besar sebelumnya akan membayar satu `format!` per byte sebelum dipotong. §11.2 kini hanya meng-hex prefiks yang tampil.
- **View grid tetap rayon atas Arrow; kunci natural memcmp-able dipakai bersama grid dan UDF `qh_natural`.** Disetujui, tetapi alasan kecepatannya dikoreksi. Probe membandingkan rayon 10 thread dengan DataFusion di 4 worker tokio (§2.7). Numerik tetap jelas lebih cepat, sedangkan natural belum terbukti. Keputusan ini berdiri karena alasan lain yang lebih kuat: semantik paritas Swift bukan SQL, perencanaan query dibayar per klik, perluasan filter saat streaming tidak cocok dengan query batch, dan sejak O-18 grid wajib bekerja tanpa helper. Satu fungsi kunci untuk grid dan SQL adalah pilihan yang tepat.
- **Satu `RuntimeEnv` per app, `SessionContext` per tab, `RegistryPool` menagih anggaran 256 MiB dengan plafon SQL 128 MiB.** Diganti oleh O-18 menjadi satu helper per app, satu `RuntimeEnv` per helper, sesi per tab di dalam helper, dan **sewa** 32–128 MiB dari registry app per query (§14.5). Penagihan per alokasi lintas proses akan butuh bolak-balik IPC di dalam `try_grow` yang sinkron; sewa di muka menghindarinya dan tetap menghormati O-12.
- **SQL dikunci (tanpa DDL, DML, `COPY`, `SET`, dan tabel URL).** Disetujui. Kuncian ini kini punya lapis kedua di tingkat kernel (§14.9).
- **Crate baru `qh-columnar` (W4-T3) dan `qh-analytics` (W13-T8).** `qh-columnar` di bawah driver adalah arah dependensi yang benar: driver tidak boleh menarik spill, kripto, dan rayon. `qh-analytics` kini paket di workspace helper, dan fitur `analytics` di `qh-ffi` dihapus (D-16). Crate protokol kecil `qh-analytics-proto` ditambahkan sebagai seam bersama. Aturan `forbid(unsafe_code)` berlaku untuk ketiganya.
- **DataFusion baru ditaut di W13-T8, bukan "sekarang".** **Menghormati maksud pemilik.** Maksud O-15 adalah analitik atas hasil besar tanpa konversi di kemudian hari, dan itu dipenuhi di W4-T3: format store sudah Arrow, skema logis dan tesnya ada sejak W4-T3, dan `qh_natural` sudah menjadi fungsi kunci yang sama. Menaut DataFusion di W4 tidak punya pemakai, dan setiap G-RUST akan membayar +6,1 GB `target/debug` serta build 3×. O-18 lalu menegaskan arah ini: DataFusion adalah komponen opsional. Syaratnya satu: W13-T8a–b tetap di lingkup program ini. Bila tugas itu dipotong, orkestrator melaporkannya ke pemilik sebagai penyimpangan dari O-15, bukan diam-diam.
- **CLI dan MCP tidak pernah menaut DataFusion.** Kini benar secara struktural, karena DataFusion bahkan tidak ada di workspace utama.
- **`rust-version` → 1.94 di W13-T8.** Tidak lagi dibutuhkan di workspace utama. Angka 1.94 hanya dinyatakan di workspace helper (F-2).
- **Lisensi.** Diperiksa terhadap log: fitur minimal lulus (`deny-df.log`: `licenses ok`), dan fitur bawaan gagal tepat di `ar_archive_writer` (`recursive_protection`) serta `libbz2-rs-sys` (`compression`) (`deny-dfdefault.log`). Graf helper adalah bagian dari graf yang lulus. Keputusan "kedua fitur mati, tanpa perubahan `deny.toml`" disetujui.
- **Ukuran biner +64,8 MB.** Tidak saya putuskan; pemilik sudah memutuskannya lewat O-18 (tidak di bundel). Yang tersisa untuk dicatat adalah ukuran unduhan terkompresi, di W13-T8a. Angka ukuran §2.2 tidak bisa diperiksa ulang dari log (keluaran `link.sh` tidak disimpan), tetapi metodenya benar, dan biner bench 65,3 MB konsisten dengannya.
- **`wide_500k` 280,8 MiB > 256 MiB; apakah anggaran masih tepat; `Utf8` atau `Utf8View`.** Angka 280,8 MiB memuat kapasitas cadangan builder. Data sebenarnya sekitar 250 MiB (§2.4). Anggaran 256 MiB tetap benar sebagai **bawaan**: ia keputusan pemilik (O-12), NFR-P3 dibangun di atasnya, dan jalur spill memang dirancang untuk hasil yang lebih besar. Tetapi bentuk tolok ukur utamanya kini tepat di tepi, jadi G-BENCH(3) wajib mencatat `spilled_bytes` (R-29). Seandainya pemilik ingin analitik atas hasil besar terasa tanpa spill di mesin 16 GiB, menaikkan bawaan adalah keputusan pemilik terpisah; `budget_bytes` sudah parameter `configure_result_stores`, jadi perubahannya satu angka di Swift. `Utf8` dipilih dengan benar. Untuk string ≤ 12 byte, `Utf8` (4 + n byte) tidak pernah lebih besar dari view 16 byte, dan untuk bentuk `wide` selisihnya +60%. Keluaran `Utf8View` DataFusion dinormalkan di helper.
- **R-22, R-23, R-28.** R-22 membaik: churn API DataFusion kini terkurung di helper. R-23 berubah bentuk: memori helper di luar sewa kini proses lain, jadi NFR-P3 untuk SQL harus diukur sebagai jumlah dua proses, dengan ambang yang ditetapkan dari angka W13-T8b. R-28 tetap nyata, dan mitigasinya (`write_text`, hex prefiks, halaman 64 baris, eskalasi C ABI) sudah berurutan dengan benar.
- **Batch 7 "off" dua store.** Sehat: paling banyak dua store per tab, dan base yang menganggur tumpah lebih dulu. Satu koreksi diterapkan di §18: kembali ke base memakai filter funnel dan search in-memory tab saat ini, bukan `ViewSpec()` kosong, yang akan membuang filter yang masih tampil aktif.
- **Umur handle (`StoreId` + `Arc`).** Benar. `StoreId` yang tidak pernah dipakai ulang di AAD memenuhi "id store dan generation" NFR-S3. `ResultHandle` memegang `Arc` sendiri, jadi use-after-free lintas FFI mustahil secara konstruksi. Untuk helper, capability-nya token acak per sesi yang menunjuk `StoreReader`, dan helper tidak bisa meminta store yang tidak didaftarkan kepadanya. Ditambahkan secara eksplisit bahwa `register_result` tidak menyimpan `Arc<ResultHandle>`, supaya pendaftaran tidak menunda `Drop = release`.
- **Tes NFR-S3.** Lengkap untuk store (§10.5) dan kini untuk helper (§14.15: canary, manipulasi, `Disabled`, `TMPDIR` terisolasi, kurungan). Dua tes di §10.5 yang dulu menyebut "berkas DataFusion" disesuaikan menjadi uji tag domain pada cipher yang sama.
- **NDJSON untuk CLI, MCP, dan golden.** Tetap (§20). SQL atas hasil bukan perintah, dan invariant #11 tidak tersentuh.

### Keputusan atas O-18 (komponen terpisah)

1. **Pengiriman: unduh saat pertama dipakai.** Aset rilis yang sama dengan DMG, dipatok per build di `Info.plist` (URL, panjang, EdDSA, SHA-256), diverifikasi Swift sebelum dipasang, dan SHA-256 diperiksa ulang Rust sebelum setiap spawn. Kunci EdDSA yang dipakai adalah kunci Sparkle yang sudah ada. Disimpan di `~/Library/Application Support/QueryHive/Components/analytics/<build>/`, diperbarui mengikuti app, dihapus dari Settings, dan bisa dipasang dari berkas tanpa jaringan (§14.2). Menaruh helper di dalam bundel ditolak, karena bertentangan dengan maksud O-18.
2. **IPC: pipa stdin/stdout, bingkai `QHA1`, data sebagai IPC Arrow, chunk ditarik helper dengan kredit.** Kunci spill app tidak pernah keluar dari proses app. Helper tidak membaca spill app, dan app men-decrypt lalu mengirim chunk sesuai permintaan, dengan data menyeberang sekali. Anggaran lintas proses lewat sewa. Cancel lewat pesan, dan helper dibunuh bila tidak menjawab dalam 2 detik. Crash helper menjadi satu event `error` per query, sewa kembali, sesi diputar ulang, dan app tetap hidup karena `SIGPIPE` diabaikan (§14.3–§14.6).
3. **Tanpa helper.** Semua yang dibangun W4–W7 berjalan tanpa helper, dan tidak ada berkas di §21.1–§21.5 yang bergantung pada DataFusion (§14.14).
4. **Poin tinjauan keamanan.** Integritas unduhan (EdDSA sebelum dekompresi, hash yang dipatok, `SecStaticCodeCheckValidity`, tanpa karantina, tulis atomik), kurungan helper (`sandbox-exec`: tulis hanya spill helper, tanpa jaringan, tanpa Keychain dan data app, tanpa exec; tanpa kurungan berarti tidak jalan), kuncian SQL di helper, graf helper tanpa klien HTTP, spill helper dengan kunci helper, validasi keluaran helper di app, dan batas ukuran bingkai (§14.2, §14.4, §14.7–§14.9).
5. **Pembagian tugas.** W13-T8a (helper dan protokol), W13-T8b (klien, sewa, unduh, verifikasi, rilis), dan W13-T8c (panel Settings, sebelum W13-T7). Semuanya dicatat di ADR-0045, ditambah adendum ADR-0007, 0010, dan 0014, serta perluasan ADR-0037 (§21.6–§21.8, §23).

### Koreksi yang diterapkan

1. O-18 dilipat ke seluruh dokumen: Ringkasan, §0, D-4, D-5, D-16, D-17, D-19–D-21 (baru), §4, §5.4, §5.6, §9.1, §9.4–§9.7, §10.2–§10.5, §14 (ditulis ulang), §17.6, §20, §21.6–§21.8, §22–§26.
2. §2.7 dan D-8: perbandingan rayon 10 thread lawan DataFusion 4 worker dinyatakan tidak setara, dan klaim "6,9×" serta "1,7–1,9×" dicabut. D-8 kini bertumpu pada semantik dan O-18.
3. §2.4 dan R-29: 280,8 MiB memuat kapasitas builder; data sebenarnya sekitar 250 MiB, di tepi anggaran. G-BENCH(3) mencatat `spilled_bytes`.
4. §2.2: biner app 23,7 MB (22,6 MiB); dengan arrow sekitar 31 MB, dan itu batas atas karena app tidak lagi menaut `arrow-cast`/`arrow-ord`. Keterlacakan angka ukuran dicatat.
5. F-2 dan F-3: `rust-version` workspace utama tetap 1.85, dan komentar `qh-export` serta ADR-0018 tidak menjadi basi.
6. §5.6: `from_arrow` berjalan di helper di belakang fitur `from-arrow`. App memvalidasi keluaran helper dengan validator rekaman spill.
7. §11.2 dan D-6: `bytes` hanya meng-hex prefiks yang tampil, dan `write_text` mencakup hex tanpa `format!` per byte.
8. §18: "off" mempertahankan filter dan search in-memory.
9. §9.6: `register_result` tidak menyimpan `Arc<ResultHandle>`.
10. §17.6: `signal(SIGPIPE, SIG_IGN)` di `App.swift` sebelum helper pertama.
11. §10.4: fd spill operator milik proses helper, bukan app.
12. Risiko baru R-30 sampai R-34.

### Yang tetap terbuka

- Ukuran unduhan helper terkompresi (W13-T8a), throughput pipa (W13-T8b, target ≥ 1 GB/s), dan ambang NFR-P3 untuk SQL (gate W13).
- Sort natural rayon dengan `view_pool` 4 thread (W5-T2, `bench_ffi view-*`).
- Perilaku profil `sandbox-exec` di macOS 26, dibuktikan `confinement.rs`; jalur App Sandbox menunggu sertifikat Developer ID.
- Dari W2-A3: bentuk pintu darurat `Json`, nasib `ArrayRows`, `PAGE_ROWS`/`COL_BLOCK`, dan mitigasi R-19. **Diselesaikan W6-A1 (2026-10-06):** D-22, D-24, D-25, dan D-26 (§3); angka `PAGE_ROWS` dan `COL_BLOCK` bersifat sementara sampai W5-T3.

### Edit yang wajib dibuat orkestrator

**`prd-performance-and-parity.md`**

- §11.1: tambah **O-15** (2026-09-30: Arrow dan DataFusion diadopsi untuk analitik atas hasil besar: SQL atas hasil, agregasi, CSV/Parquet lokal; store Arrow sejak W4-T3) dan **O-18** (2026-09-30: DataFusion komponen opsional terpisah yang diunduh saat pertama dipakai, app tetap sekitar 30 MB, store Arrow di app).
- §5: bagian baru **5.11 Analitik atas hasil**: FR-ANL-01 SQL atas hasil (mesin W13-T8a–b, UI kemudian); FR-ANL-02 agregasi dan pivot (UI kemudian); FR-ANL-03 join antar-hasil (UI kemudian); FR-ANL-04 CSV/Parquet lokal (mesin W13-T8a–b, UI kemudian); FR-ANL-05 komponen analitik: izin unduh, verifikasi, pasang dari berkas, hapus, dan status di Settings (W13-T8b–c).
- §6.1 NFR-P3: tambah "Untuk skenario SQL, diukur sebagai jumlah `phys_footprint` app dan helper; ambangnya ditetapkan di gate W13 dari angka `bench_ffi sql-*`."
- §6.3 NFR-L: ganti "Yang direncanakan: `rayon`, `unicode-segmentation`, dan bersyarat `mimalloc`" dengan daftar yang memuat juga `arrow-array`/`-schema`/`-buffer`/`-ipc` 59.3 (app) dan `datafusion` 55.1 fitur minimal plus `arrow-cast` (hanya helper). Tambah: "workspace helper juga lulus `cargo deny`; fitur `recursive_protection` dan `compression` mati."
- §6.4 NFR-S3: "Nonce adalah penghitung per chunk" → "Nonce dari penghitung per kunci yang tidak pernah berulang; satu kunci per proses yang menulis spill, dan kunci tidak pernah menyeberang proses (helper analitik punya kuncinya sendiri)." NFR-S5: tambah "Helper analitik tidak bisa menulis berkas selain spill terenkripsinya dan tidak punya jaringan; dijaga kurungan kernel." Tambah **NFR-S7**: "Komponen yang diunduh diverifikasi (EdDSA dengan kunci Sparkle, SHA-256 yang dipatok per build, tanda tangan kode) sebelum dipasang, SHA-256 diperiksa ulang sebelum setiap eksekusi, dan komponen tidak pernah dijalankan tanpa kurungan."
- §6.6 NFR-C: tambah "SQL atas hasil adalah objek UniFFI, bukan perintah NDJSON; CLI dan MCP tidak mendapatkannya."
- §10: tambah "UI analitik (editor SQL atas hasil, pivot, join, pembuka CSV/Parquet): rilis berikutnya; program ini hanya membangun mesin dan panel komponen di Settings." Catat bahwa notarisasi tetap di luar lingkup, dan helper bekerja dengan tanda tangan ad-hoc.
- §13: tambah risiko unduhan tidak tersedia (tanpa jaringan), dengan mitigasi "Pasang dari berkas…".

**`performance-plan.md`**

- §10 butir 1: codec bertipe per chunk → `RecordBatch` Arrow per chunk, encoding per chunk-kolom, kolom campuran sebagai `Binary` bertag; spill = IPC Arrow + AES-256-GCM. Butir 3: "generation" → `StoreId(u64)` yang tidak pernah dipakai ulang.
- §11 butir 1: `qh_core::ColumnarBuilder` → `qh_columnar::ChunkBuilder`.
- §13: baris DataFusion → "Ya, sebagai helper terpisah yang diunduh saat pertama dipakai (O-15, O-18): SQL atas hasil dan berkas lokal; bukan untuk view grid (semantik paritas Swift, dan grid harus jalan tanpa helper)". Baris group-by → "Mesin di W13-T8a–b, UI kemudian". Baris Arrow C Data Interface tetap "Tidak". Tambah baris "arrow-rs sebagai format store: Ya (W4-T3)".
- §15: 0008 → digantikan 0030; baris 0034 "DataFusion ditolak" → "DataFusion untuk SQL di helper opsional, bukan untuk view"; tambah 0045, perluasan 0037, dan adendum 0007, 0010, 0014.
- §17: hapus "Ditolak: DataFusion, `arrow`"; tambah `arrow-*` 59.3 (app, Apache-2.0) dan `datafusion` 55.1 fitur §14.1 (hanya helper, lisensi per §24).
- §18: tambah risiko proses helper (R-30 sampai R-34 blueprint).

**`development-plan.md`**

- §1: tambah **G-ANALYTICS**: `cargo test --manifest-path helpers/analytics/Cargo.toml && cargo build --release --manifest-path helpers/analytics/Cargo.toml && cargo deny --manifest-path helpers/analytics/Cargo.toml check licenses --config deny.toml`, ditambah `cargo tree --manifest-path helpers/analytics/Cargo.toml -e normal` yang tidak boleh memuat `reqwest`, `hyper`, `h2`, atau `rustls`, serta ukuran helper stripped dan terkompresi dicatat. Kapan: W13-T8a, W13-T8b, gate W13, dan W14. G-DENY: tambah baris untuk manifest helper. G-HEAVY: tambah G-ANALYTICS untuk W13 dan W14 saja. G-BENCH(3): catat `spilled_bytes`.
- §3: jumlah tugas W13 naik tiga (W13-T8a, T8b, T8c).
- §4 jadwal ADR: 0045 di W13-D, adendum 0007, 0010, 0014 di W13-D; isi 0030, 0034, dan 0037 menurut blueprint §23.
- §5 W4-T3: berkas menurut blueprint §21.1: `crates/qh-columnar/**`, `crates/qh-result-store/src/{chunk.rs,logical.rs}`, `crates/qh-result-store/tests/logical.rs`, dan `crates/qh-core/src/render.rs` (`write_text` dan hex, keluaran tidak berubah); `codec.rs` dihapus. Verifikasi W4-T3 ditambah G-GOLDEN. Commit: `feat(store): Arrow chunks with a lossless Value mapping, a rayon view, and spill encrypted with a per-process key`.
- §5 W7-T1: berkas menurut blueprint §21.5; `crates/qh-core` (`ColumnarBuilder`) keluar dari daftar.
- §5 W13: tambah W13-T8a, W13-T8b, dan W13-T8c dengan berkas, verifikasi, pemeriksa, dan commit dari blueprint §21.6–§21.8. W13-T8c sebelum W13-T7.
- §6: W13-T8a implementer Rust **opus** (kripto, pool, protokol); W13-T8b implementer **opus** (umur proses, sewa, verifikasi unduhan); W13-T8c GP-s **sonnet**; `security-reviewer` wajib di W13-T8a dan W13-T8b.
- §7 rantai kepemilikan: `Cargo.toml`/`Cargo.lock` W4-T3 → W5-T2 → W7-T2 → W7-T5 → W13-T8a; lajur FFI (`app/Generated`, `uniffi_api.rs`, `Support/RustEngine.swift`) … → W13-T4 → W13-T8b; `crates/qh-rt/src/lib.rs` W2-T1 → W4-T3 → W13-T8a; `crates/qh-result-store/src/{registry.rs,logical.rs}` W4-T3 → W13-T8b; `crates/qh-result-store/src/spill.rs` W4-T3 → W13-T8a; `crates/qh-columnar/**` W4-T3 → W7-T1 → W13-T8a; `app/build.sh` … → W13-T8b → W13-T7; `app/release.sh` dan `app/build-dmg.sh` → W13-T8b; `App.swift` … → W13-T8b; `Views/SettingsView.swift` W6-T1 → … → W13-T8c → W13-T7. `helpers/analytics/**` baru, milik W13-T8a.
- §10: tambah titik rawan "W13-T8a membangun DataFusion (sekitar 2,3 GB `target` rilis, debug dikurangi dengan `line-tables-only`); periksa ruang disk sebelum mulai."
- §11: tambah pertanyaan terbuka dari bagian "Yang tetap terbuka" di atas.

## Riwayat: codec kustom + rayon (digantikan)

Rancangan W2-A3 (30 Sep 2026, commit `dc4186f`) memakai layout chunk biner buatan sendiri, `QHC1`. Isinya: header 16 byte, direktori 24 byte per kolom, encoding per (chunk, kolom) `TAGGED`, `BOOL`, `I64`, `U64`, `F64`, `DEC128`, `DATE32`, `TIME64`, `TIMESTAMP` (dengan array offset atau offset seragam) dan `VARLEN`, serta bitmap validitas, openable, dan numerik di dalam segmen. Semuanya dibaca dengan `from_le_bytes` di bawah `forbid(unsafe_code)`. Spill menyegel byte `QHC1` apa adanya ("satu buffer untuk memori dan disk"). Kunci natural memakai prefiks `u128` dan comparator penuh yang terpisah. `performance-plan.md` §13 menolak DataFusion karena dependensinya berat, kolasinya byte-order, dan SQL atas hasil tidak diminta. Bagian kepatuhan verdict menulis "tidak ada Arrow, mmap, atau DataFusion".

Keputusan pemilik 2026-09-30 membatalkan alasan ketiga. Revisi ini menjawab dua lainnya: dependensinya diukur (§2.2, §24), dan kolasi natural tetap milik kita lewat satu fungsi kunci yang juga menjadi UDF (D-8). Yang dipertahankan dari rancangan lama tercantum di §0. Teks lengkap rancangan lama: `git show dc4186f:docs/architecture/blueprints/fase-6-data-plane.md`.

Verdict AR atas rancangan lama disimpan apa adanya di bawah. Nomor bagian di dalamnya merujuk dokumen lama. Yang tidak berlaku lagi: semua yang menyebut `QHC1`, codec, prefiks `u128` sebagai satu-satunya jalur, dan "tidak ada Arrow atau DataFusion". Yang tetap berlaku: keputusan atas D-8 (Explain lewat store), D-5 (unlink segera), D-13, nonce dan umur kunci, aturan segel, layout `window()`, serta koreksi 1–11.

### Verdict architect-reviewer W2-A3 (arsip)

**Verdict: disetujui dengan koreksi** (W2-A3, 30 Sep 2026). Koreksi di bawah sudah diterapkan di dokumen ini. Blueprint siap untuk W4-T3 setelah orkestrator memperbarui `development-plan.md` sesuai daftar R-15. Tinjauan ulang AR di W6-A1 tetap berlaku untuk §10.3 dan §14.

#### Yang diperiksa

- Rencana dan PRD: `performance-plan.md` §7, §9, §10, §13; FR-PERF-05, FR-GRID-03/04, NFR-P1 S2, NFR-P2, P3, P8, NFR-S3, NFR-C, O-8, O-9, O-12; `docs/invariants.md` #1 dan #11; `fase-2-engine-host.md` §2 dan §3.
- Klaim kode, dicek langsung:
  - `codec.rs:317-337` memang men-decode `Unknown` bentuk 2 (`raw`) menjadi `text` lossy dengan `raw: None`, sehingga `to_text` memberi teks, bukan hex (`qh-core/src/render.rs:93-97`). Ini perubahan data senyap yang nyata, dan perbaikannya di W4-T3 wajib.
  - Sapuan spill memang tidak ada: `qh-storage` hanya menyebut spill di komentar `lib.rs:16`, sementara `SpillFile::drop` (`store.rs:105-111`) mengandalkannya. Spill hari ini teks biasa dengan `create(true).truncate(true)` dan penghitung per store, jadi dua store dalam satu proses bisa menimpa berkas yang sama. Crate ini belum dipakai siapa pun, jadi cacat itu laten.
  - `ring` 0.17.14 ada di `Cargo.lock:2903`. `aead` dan `rand` modul publik tanpa gerbang fitur, dan `LessSafeKey::seal_in_place_separate_tag`, `open_in_place`, serta `Nonce::assume_unique_for_key` ada di baris yang disebut. ISC dan Apache-2.0 ada di allow-list `deny.toml`.
  - `explain` dan `preview` sama-sama lewat `emit_batches` (`commands.rs:1433`, `:1482`). `rayon`, `unicode-segmentation`, dan kawan-kawannya belum ada di `Cargo.lock`. `unicode-normalization`, `unicode-properties`, `zeroize`, dan `crossbeam-utils` sudah ada.

#### Keputusan atas penyimpangan

- **D-8: Explain lewat store. Dipertahankan.** Plan digambar di grid yang sama dan hari ini ikut filter, search, dan sort `displayedRows`. Mempertahankan `ArrayRows` untuk Explain berarti mempertahankan `GridSort`, `ColumnFilter`, dan `GridSearch` di Swift, yaitu dua implementasi dengan kolasi berbeda untuk satu grid. Itu bertentangan dengan butir "implementasi Swift dihapus" di rencana yang sama. Ongkos engine nol. Dua `preview` yang hanya membaca `columns` (`AppModel.swift:545`, `:2107`) tetap NDJSON dan kini tercatat di §17. Penyimpangan dari `performance-plan.md` §10 butir 5 masuk ADR-0030.
- **D-5: unlink segera. Sehat di macOS.** fd yang path-nya sudah di-unlink tetap bisa `pread`/`pwrite`/`fstat` di APFS dan HFS+, dan blok bebas saat fd terakhir ditutup, termasuk saat crash. macOS tidak punya `O_TMPFILE`, jadi `create_new` lalu unlink adalah cara standarnya. Dua syarat ditambahkan: unlink sebelum byte pertama ditulis, dan unlink yang gagal berarti tidak ada spill. Akibat sampingnya, pemeriksaan direktori di G-LEAK menjadi hampa, jadi `store_stats` menjadi bukti pelepasan. Risiko fd (R-19) ditambahkan.
- **D-13: empat dari lima dipertahankan.** `distinct_values` (PR-12), `rows_text` (copy, `WritePlan`, drag), `store_from_rows` (scene `--snapshot` dan tes), dan `store_stats` (G-LEAK di bawah D-5) punya pemakai yang ada. `store_synthetic` hanya melayani bench, jadi tidak diekspor lewat UniFFI. Ia tinggal sebagai konstruktor Rust untuk `bench_ffi`, dan `--bench` Swift memakai `store_from_rows` di luar interval ukur.
- **Nonce dan umur kunci: benar dan minimal.** Satu kunci per registry, satu `AtomicU64` di objek yang sama, nonce `[0; 4] ‖ counter` big-endian: tidak ada nonce yang terulang di bawah satu kunci, di store mana pun. Setiap segel mengambil nonce baru, dan chunk yang sudah tumpah tidak pernah disegel ulang. NFR-S3 menulis "penghitung per chunk"; penghitung global per segel memenuhi maksudnya dan lebih kuat. Dua koreksi kecil: AAD memakai `store_id` u64 yang tidak pernah dipakai ulang sebagai pengganti `(slot, generation)`, dan dokumen tidak lagi menyiratkan bahwa jadwal kunci di dalam `LessSafeKey` di-zeroize.
- **Aturan segel: disederhanakan.** Satu push, satu segel, dipecah hanya di 65.536 baris atau 2 MiB. Timer 16 ms, `seal_if_due`, `publish_deadline`, dan cabang tenggat di `select!` dihapus. Timer itu tidak bisa menolong baris yang masih di dalam `next_batch` driver, dan sekitar 310 chunk untuk 5 juta baris jauh di bawah ambang R-9. TTFR S2 tetap dilayani, karena batch pertama langsung terbit.
- **Layout `window()`: dipangkas, bentuk dasarnya tetap.** Satu buffer dengan offset `u32` dan satu salinan ke `Data` sudah minimum untuk NFR-P8. Gema `slot`, `generation`, `view_id`, dan daftar kolom dihapus, sehingga header turun dari 48 ke 32 byte, tanpa medan yang harus divalidasi tapi tidak menangkap apa pun. Target 0,5 ms dibatasi ke format selain `Json`. `Json` mem-parse sampai 100.000 unit per sel, jadi ia diukur terpisah dan dicatat.

#### Koreksi lain yang diterapkan

1. Slab slot dengan generation diganti `StoreId(u64)` (§7.2). `ResultHandle` memegang `Arc<StoreShared>` sendiri, jadi use-after-free lintas FFI mustahil secara konstruksi, dan slab hanya menambah free list serta aturan pensiun.
2. Tidak ada registry bawaan implisit (§10.3). `configure_result_stores` dipanggil sinkron sebelum sesi dipulihkan (§14.6). Rancangan awal punya balapan: Run yang dipulihkan bisa membangun registry tanpa spill lebih dulu, lalu `configure` ditolak. Itu fallback senyap.
3. Enkripsi dan `pwrite` saat evict berjalan di luar kunci `residency` (§7.5), supaya jendela di main tidak menunggu I/O.
4. `row_count()` tidak lagi punya efek samping. Perluasan view filter saat streaming dipicu penulis (§11.8).
5. Seri `Num` dan `Temporal` diputus langsung oleh indeks baris (§11.3). Pemutus byte di antaranya akan diam-diam mengubah urutan `"1.0"` dan `"1"` dibanding Swift.
6. Numerik `F64` mengikuti `swift_plain_number(to_text)` (§5.2). `1e300` finite, tetapi di luar rentang `Decimal` Swift.
7. Printer format `Json` wajib mengurutkan kunci sendiri (§9.4), karena `serde_json` di workspace memakai `preserve_order` (`Cargo.toml:52`). Pintu darurat `Json` tidak boleh menambah panggilan FFI per sel.
8. Tab di latar membuang cache halamannya (§14.2), demi NFR-P3.
9. Sapuan `$TMPDIR/queryhive-spill` dari codec lama dihapus (§7.7), karena tidak ada build produk yang pernah menulisnya. Scratch segel bersama diganti `Vec` per segel (§8.3).
10. Nama skenario bench diselaraskan dengan `BenchMode.swift` (`scroll-30x1m`, `scroll-500x10k`, `open-500x10k`, `tabs-100`).
11. Amandemen D-1 Fase 2 (metode store melempar `StoreFfiError`) dicatat di §10.3 dan di daftar ADR-0030.

#### Kepatuhan

- NFR-C dan invariant #11: tidak ada perintah baru. `RESULT_SINK=store` hanya bisa dicapai lewat `Emitter::result_store()` milik host, jadi CLI, MCP, golden, dan ekspor tetap NDJSON. Invariant #1 dijaga W5-T2.
- §13 rencana: tidak ada Arrow, mmap, atau DataFusion. `forbid(unsafe_code)` tetap di crate store.
- NFR-S3: AES-256-GCM `ring`, kunci acak per proses di memori saja, nonce tidak berulang, AAD berisi store id (yang juga generation) dan indeks chunk, direktori `0700`, berkas `0600` lewat `create_new`, sapuan tanpa mengikuti symlink, dan tes §8.5. Teks biasa tidak pernah menyentuh disk, karena berkas hanya berisi ciphertext dan tag, nonce ada di memori, dan unlink terjadi sebelum tulisan pertama. Swap macOS terenkripsi oleh sistem.
- Perubahan tampilan yang disengaja terbatas pada yang sudah tercatat (`performance-plan.md` §14 butir 1, 2, 4) ditambah divergensi fixture yang wajib didaftar. Perbaikan `Unknown` `raw` mengubah teks sel menjadi hex yang benar. Itu perbaikan, dan tercatat di fakta 2.

#### Daftar R-15 untuk orkestrator (`development-plan.md`)

- **W4-T3** (§5): tambahkan `crates/qh-result-store/src/registry.rs`, `src/collate.rs`, `src/render.rs`, `tests/spill.rs`, `tests/window.rs`, `tests/view.rs`, dan `crates/qh-rt/Cargo.toml`.
- **W5-T2** (§5): tambahkan `crates/qh-ffi/src/events.rs`, `crates/qh-ffi/Cargo.toml`, `crates/qh-ffi/examples/bench_ffi.rs`, `crates/qh-ffi/tests/store_sink.rs`, dan `Cargo.lock`.
- **W6-T1** (§5): tambahkan `Support/DatabaseEngine.swift`, `Views/ResultGrid.swift`, `Support/Snapshot.swift`, `Support/BenchMode.swift`, `Views/SettingsView.swift`, `Tests/QueryHiveTests/StoreRowsTests.swift`, `Tests/QueryHiveTests/Bench/StoreWindowBench.swift`, `ResultGridTests.swift`, `GridColumnsTests.swift`, `Batch7Tests.swift`, `TabCloseTests.swift`, `MockEngine.swift`, dan `RowLimitSettingTests.swift`.
- **Rantai kepemilikan** (§7):
  - `Cargo.toml`, `Cargo.lock`: W4-T3 → W5-T2 (hanya `Cargo.lock`, tepi `qh-ffi → qh-result-store`) → W7-T2 → W7-T5.
  - `Views/ResultGrid.swift`: sisipkan W6-T1 di antara W5-T1 dan W9-T5.
  - `Support/Snapshot.swift`: W1-T3 → W5-T1 → W6-T1 → W9-T8.
  - `Support/BenchMode.swift`: belum punya rantai. Tambahkan dengan W6-T1 sebagai pemilik Fase 6.
- **G-LEAK** (§1): tambahkan pemeriksaan `store_stats().stores == 0` dan `spilled_bytes == 0` di akhir `tabs-100`.

#### Yang tetap terbuka untuk W6-A1

Bentuk pintu darurat `Json` di permintaan jendela, nasib `ArrayRows`, `PAGE_ROWS`/`COL_BLOCK` dari angka W5-T3, dan mitigasi R-19 bila batas fd terukur. **Diselesaikan W6-A1 (2026-10-06):** lihat §3 D-22, D-24, D-25, dan D-26.
