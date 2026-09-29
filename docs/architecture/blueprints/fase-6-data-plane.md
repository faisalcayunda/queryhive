# Blueprint Fase 6: data plane lewat result store

- **Status:** blueprint tingkat berkas, 30 Sep 2026 (W2-A3). Disegarkan terhadap seam Fase 5 di W6-A1 sebelum W6-T1 mulai.
- **Untuk:** W4-T3 (inti store: codec, view, spill terenkripsi), W4-T4 (fixture diferensial), W5-T2 (sink engine dan `ResultHandle` lewat UniFFI), W6-T1 (integrasi Swift). Pemeriksa: `architect-reviewer`; `security-reviewer` untuk §8.
- **Sumber:** `performance-plan.md` §7 (arti "off"), §9 (seam `ResultRows`), §10 (Fase 6), §13 (opsi); PRD FR-PERF-05, FR-GRID-03/04, NFR-P1 S2, NFR-P2, P3, P8, NFR-S3, O-8, O-9, O-12; `rust-engine-blueprint.md` §2.4, §2.6, §4.2; `docs/invariants.md` #1 dan #11.
- **Bahasa:** dokumen ini Indonesia. Identifier, komentar kode, pesan log, dan pesan galat Inggris.

## Ringkasan

Baris hasil app berhenti lewat JSON. `preview` dan `explain` yang dijalankan app menulis ke `ResultStore` di Rust, dan grid membaca jendela teks yang sudah dirender lewat satu panggilan UniFFI per halaman. Store menyimpan chunk kolumnar bertipe (fallback bertag untuk kolom campuran), dengan anggaran global 256 MiB. Kelebihannya tumpah ke disk, dienkripsi AES-256-GCM dengan kunci acak per proses. Sort, filter, search, dan daftar distinct untuk grid pindah ke Rust (rayon, permutasi `Vec<u32>`), dan implementasi Swift-nya dihapus setelah fixture diferensial lulus. Tab memegang paling banyak dua store: dasar dan aktif, sehingga "off" di Batch 7 tidak pernah menjadi query ketiga. CLI, MCP, golden, ekspor, dan semua perintah lain tetap NDJSON.

## 1. Fakta yang diperiksa sebelum merancang

1. **`qh-result-store` tidak dipakai crate mana pun.** Ia hanya anggota workspace (`Cargo.toml:21`, `:46`); `grep` atas `crates/` tidak menemukan pemakai. Codec dan API-nya bisa diganti tanpa migrasi pemanggil.
2. **Codec hari ini** menyimpan satu nilai bertag per sel dengan offset `u32` per kolom (`codec.rs:8-21`), dan `window()` mengembalikan `Vec<Vec<Value>>` (`store.rs:201-251`). Tag `TAG_NULL`…`TAG_UNKNOWN` (`codec.rs:30-46`) dipakai ulang sebagai encoding fallback. Ada cacat laten: `decode_value` mengubah `Unknown` bentuk `raw` menjadi teks lossy dan membuang `raw` (`codec.rs:317-337`), sehingga `to_text` setelah round trip memberi teks, bukan hex (`render.rs:93-97`). Tes round trip hanya memakai `Value::unknown` bentuk teks, jadi cacat ini lolos. W4-T3 memperbaikinya.
3. **Spill hari ini** adalah teks biasa di `$TMPDIR/queryhive-spill/spill-<pid>-<n>.bin`, dengan mode dari umask (`store.rs:320-345`). Komentar `Drop` menyebut "startup sweep in `qh-storage`" (`store.rs:107-109`). Sapuan itu **tidak ada**: satu-satunya kata "spill" di `qh-storage` adalah komentar `lib.rs:16`. Fase 6 membangunnya.
4. **`ring` 0.17.14** sudah ada di `Cargo.lock:2903` dengan lisensi `Apache-2.0 AND ISC`, dan keduanya ada di allow-list `deny.toml`. `ring::aead` menyediakan `LessSafeKey::seal_in_place_separate_tag` dan `open_in_place` (`ring-0.17.14/src/aead/less_safe_key.rs:78`, `:142`) serta `Nonce::assume_unique_for_key` (`nonce.rs:39`). AEAD tidak menambah crate.
5. **Crate Unicode yang sudah ada.** `unicode-normalization` 0.1.25 dan `unicode-properties` 0.1.4 sudah ada di `Cargo.lock` sebagai dependensi transitif `stringprep` (tokio-postgres). Keduanya dipakai langsung untuk NFC/NFD dan kategori umum tanpa crate baru. `rayon` (bersama `rayon-core`, `crossbeam-deque`, `crossbeam-epoch`, `either`) dan `unicode-segmentation` belum ada, sedangkan `crossbeam-utils` sudah. `zeroize` sudah menjadi dependensi workspace.
6. **Pool rayon harus dibangun di `qh-rt`,** karena `qh_rt::set_thread_qos` privat (`crates/qh-rt/src/lib.rs:203`).
7. **`Progress` tidak bisa dipakai untuk batas ≤ 1 event per 16 ms.** Ia punya aturan "minimal satu event per 1.000 baris" (`progress.rs:64-75`). Pada 380.000 baris/s itu berarti ~380 event/s.
8. **Fetch adaptif cukup diatur di pemanggil.** `ExecuteOptions::max_batch_rows` adalah plafon, bukan ukuran tetap (`qh-driver/src/lib.rs:242-251`), dan `next_batch(max_rows)` menerima ukuran per panggilan.
9. **`EngineError::StaleHandle` sudah ada** (`qh-core/src/error.rs:80`).
10. **Semantik Swift yang di-port**, dibaca dari kodenya:
    - `GridSort.compare/number/isPlainNumber` (`Models/GridSort.swift:83-149`);
    - `ColumnFilter.matches/matchesText/distinctValues` (`Models/QueryTab.swift:201-284`);
    - urutan filter → search → sort di `displayedRows` (`QueryTab.swift:824-848`);
    - `GridSearch.matches` (`Models/GridSearch.swift:21-29`);
    - `GridValue.isOpenable/looksLikeJSON/prettyPrinted` (`Models/GridValue.swift:55-97`);
    - `ColumnFormat.render` (`Models/ColumnFormat.swift:48-94`) dan `HexDump.decodedHex` (`Support/HexDump.swift:16-40`);
    - `naturalWidths` (`Views/ResultGrid.swift:82-96`);
    - tooltip sel memakai teks **terformat** dan hanya untuk teks tidak kosong (`ResultGrid.swift:1023-1033`);
    - perataan kanan ditentukan **tipe kolom** (`ResultGrid.swift:1164-1168`).
11. **Filter dan search bekerja atas teks tersimpan** (`to_text`), bukan teks terformat (`QueryTab.swift:829-839`). `isOpenable` dihitung atas nilai tersimpan **atau staged** (`ResultGrid.swift:748-770`).
12. **`RustEngine` satu-satunya pengimpor modul FFI** (`Support/RustEngine.swift:6-8`), dan `DatabaseEngine` belum punya handle hasil (`Support/DatabaseEngine.swift:17-25`).
13. **`crates/qh-ffi/src/host.rs` (Fase 2) belum ada di kode.** Bentuknya diambil dari `fase-2-engine-host.md` §3: `EngineHost::new()` tanpa argumen dan tanpa I/O, `host.run(...)` menggantikan fungsi bebas `run`, dan `preview`/`explain` dirutekan `Pooled(Query)`. Blueprint ini hanya menambah titik sambung di atasnya (§10.3).

## 2. Keputusan desain

| # | Keputusan | Alasan |
|---|---|---|
| D-1 | **Satu buffer untuk memori dan disk.** Chunk dikodekan sekali menjadi `Arc<[u8]>`. Spill mengenkripsi byte itu apa adanya, dan satu decoder membaca keduanya. | Prinsip "one encoder, one decoder" di `codec.rs` dipertahankan. Akuntansi memori sama dengan panjang buffer. Tidak ada jalur kedua yang bisa berselisih. |
| D-2 | **Encoding dipilih per (chunk, kolom)**, bukan per kolom dan bukan dari nama tipe. | Driver bisa mencampur varian dalam satu kolom, misalnya `numeric` PostgreSQL yang gagal di-parse menjadi `Value::Text` (`qh-driver-postgres/src/normalize.rs:93`). |
| D-3 | **Publikasi hanya lewat chunk yang disegel.** Tidak ada pembaca atas builder yang masih terbuka. Kunci hanya dipegang untuk menambah entri indeks. | Pembaca utama (main thread) tidak pernah menunggu penulis. Ini jawaban atas risiko "kontensi" di `performance-plan.md` §10. |
| D-4 | **Anggaran 256 MiB global di `StoreRegistry`, dengan spill LRU lintas store.** | O-12. Store dasar Batch 7 yang menganggur dan tab di latar tidak disentuh, sehingga tumpah lebih dulu. |
| D-5 | **Spill AES-256-GCM (`ring`) dengan satu kunci acak per registry, yaitu per proses di app.** Nonce diambil dari penghitung milik registry. Berkas di-`unlink` segera setelah dibuat, **sebelum byte pertama ditulis**. | Satu kunci dipakai banyak store, jadi penghitung per store akan mengulang nonce di bawah kunci yang sama. Dengan unlink segera, direktori spill selalu kosong dan crash tidak meninggalkan data. Di macOS (APFS/HFS+), fd yang path-nya sudah di-unlink tetap bisa `pread`/`pwrite`, `fstat` tetap jalan, dan blok dibebaskan saat fd terakhir ditutup. Sapuan startup tinggal menangani jendela antara `open` dan `unlink`. **Menyimpang** dari `<uuid>.bin` di `rust-engine-blueprint.md` §2.6: path spill hilang dari diagnostik, dan `store_stats()` menggantikannya. Akibatnya pemeriksaan "direktori spill kosong" di G-LEAK menjadi hampa, jadi G-LEAK juga harus memeriksa `store_stats().stores == 0` (§16). |
| D-6 | **`window()` mengembalikan satu `Vec<u8>` (UniFFI `Data`)** berisi teks yang sudah dirender, offset, dan flag. | ADR-0004 yang diamandemen 0030. Teks sudah melewati `qh_core::render::to_text` dan format kolom, jadi Swift menggambar tanpa memformat (`performance-plan.md` §13, opsi "data tampilan dihitung di Rust"). |
| D-7 | **Permutasi `Vec<u32>`. View identitas tidak punya permutasi.** | Plafon 5.000.000 baris jauh di bawah `u32::MAX`. Hasil tanpa filter dan sort tidak membayar 4 byte per baris. |
| D-8 | **Satu implementasi view.** Sort, filter, search, dan daftar distinct untuk grid hanya ada di Rust. `GridSort.order`, `ColumnFilter.matches/matchesText/distinctValues`, dan `GridSearch.matches` dihapus di W6-T1 setelah fixture W4-T4 lulus. Karena itu **Explain juga menulis ke store**, lewat `RowTarget` yang sama dengan preview. | **Menyimpang** dari `performance-plan.md` §10 butir 5 ("Explain dan inspector objek tetap ArrayRows lewat NDJSON"). Tanpa ini, sort in-memory atas plan (kasus fallback kedua O-8) butuh implementasi Swift kedua dengan kolasi yang berbeda. Plan digambar di grid yang sama dan hari ini ikut `displayedRows` (filter, search, sort; `QueryTab.swift:824-848`, `AppModel.swift:2697-2765`). Ongkos engine-nya nol: `explain` sudah lewat `emit_batches` yang sama dengan `preview` (`commands.rs:1433`, `:1482`), jadi pemilihan target di `pump_result` mencakup keduanya. CLI dan golden `explain` tidak berubah, karena tidak memasang `RESULT_SINK`. **Diputuskan AR: dipertahankan.** |
| D-9 | **`ColumnFormat.render` dan `GridValue.isOpenable` tetap ada di Swift, hanya untuk sel staged dan pembaca nilai.** Jalur gambar grid memakai hasil Rust. Kedua sisi diuji terhadap fixture yang sama. | Sel staged hanya beberapa. Fungsi FFI `format_cell` akan menambah permukaan demi kasus sekecil itu. Fixture dua sisi menangkap drift dari mana pun datangnya. |
| D-10 | **`column_widths()` mengembalikan hitungan grapheme, bukan piksel.** Rumus lebar tetap satu, di Swift. | Label header bisa diganti nama pengguna (`tab.columnLayout.label`), dan hanya Swift yang tahu. |
| D-11 | **Store sink dipilih setelan `RESULT_SINK=store`, dan store-nya diberikan lewat `Emitter::result_store()`.** Metode default trait mengembalikan `None`. | `JsonLines`, `Capture`, MCP, dan golden tidak bisa mencapai jalur store. `RESULT_SINK=store` tanpa store adalah galat usage, bukan fallback senyap. Signature `run()` tidak berubah. |
| D-12 | **Handle dibuat pemanggil sebelum run**, seperti `RunCancel` (`uniffi_api.rs:240-259`). | `run_with_store` memblokir sampai perintah selesai. Handle yang ia kembalikan akan datang setelah tidak ada lagi yang perlu di-poll. |
| D-13 | **Tambahan yang tidak disebut rencana, karena fitur yang ada membutuhkannya:** `distinct_values` (picker ≤ 10 nilai, PR-12), `rows_text` (copy, `WritePlan`, drag CSV), `store_from_rows` (scene `--snapshot` yang men-sort atau memfilter, dan tes Swift), dan `store_stats` (satu-satunya bukti G-LEAK bahwa store dan fd spill dilepas, karena D-5 membuat direktori spill selalu kosong). **`store_synthetic` tidak diekspor lewat UniFFI** (keputusan AR): ia hanya konstruktor Rust di blok `impl EngineHost` yang tidak di-export, seperti `with_connector` Fase 2, dan dipakai `bench_ffi` serta tes Rust. `--bench` Swift membangun store lewat `store_from_rows` di luar interval yang diukur, sama seperti hari ini ia membangun `PreviewResult` sintetis. | Tanpa keempatnya, fitur hari ini hilang atau G-LEAK tidak bisa membuktikan apa pun. Bench bukan fitur produk, jadi tidak menambah permukaan FFI. |
| D-14 | **Tidak ada perintah baru.** | Empat daftar invariant #11 tidak tersentuh. Permukaan baru hanya objek dan metode UniFFI, dan tetap mengikuti invariant #1 (`app/Generated` ikut di-commit). |

## 3. Alur data

```
Run (Swift, main)
  RustEngine.runIntoStore
    (sekali, sinkron, sebelum tab pertama dipulihkan) host.configure_result_stores(spill_dir, 256 MiB) -> sapuan
    host.create_result_store()                     -> ResultHandle {store_id}   (dibuat sebelum run, D-12)
    [antrean .userInitiated] host.run_with_store(Preview|Explain, settings + RESULT_SINK=store, store, sink, cancel)
      commands::preview -> stream_rows -> pump_result(RowTarget::Store)
        cursor.next_batch(200 -> 800 -> 3.200 -> 12.800 -> 16.384)
        StoreWriter.push(batch) -> ChunkBuilder --segel per push--> Arc<[u8]> -> indeks (RwLock singkat) -> rows (AtomicU32, Release)
                                                 \-> registry.charge() -> evict LRU -> SpillCipher.seal -> pwrite (fd sudah di-unlink)
        event: step connect, columns, progress{rows} <= 1 per 16 ms, done
Grid (main, per frame selama streaming)
  displayLink -> handle.row_count()  (load atomik) -> noteNumberOfRowsChanged
  draw -> StoreRows.cell(r, c) -> halaman cache --miss--> handle.window(view, first, 128, kolom[<=32], format) -> Data -> WindowPage
  prefetch searah scroll (antrean .userInitiated)
Sort fallback / filter / search / "off"
  [antrean .userInitiated] handle.set_view(spec) -> qh_rt::view_pool() (rayon, P-core, USER_INITIATED) -> permutasi -> view_id baru
Tutup tab
  RustRun.terminate() -> handle.release() -> fase Released, chunk dilepas, anggaran dikembalikan, fd spill ditutup
```

## 4. Layout chunk, byte demi byte

Semua bilangan little-endian. Tidak ada syarat alignment, karena pembaca memakai `from_le_bytes` (crate ini tetap `forbid(unsafe_code)`). Setiap segmen kolom dimulai di offset kelipatan 8 supaya hexdump bisa dibaca dan Fase 7 bebas memilih cara baca.

### 4.1 Header dan direktori

| Offset | Ukuran | Isi |
|---|---|---|
| 0 | 4 | magic `"QHC1"` |
| 4 | 4 | `u32 rows`, 1…65.536 |
| 8 | 4 | `u32 column_count` (C) |
| 12 | 4 | `u32 reserved = 0` |
| 16 | 24 × C | direktori kolom |
| 16 + 24C | … | segmen kolom, berurutan |

Satu entri direktori (24 byte):

| Offset | Tipe | Isi |
|---|---|---|
| +0 | `u8` | `encoding`: 0 `TAGGED`, 1 `BOOL`, 2 `I64`, 3 `U64`, 4 `F64`, 5 `DEC128`, 6 `DATE32`, 7 `TIME64`, 8 `TIMESTAMP`, 9 `VARLEN` |
| +1 | `u8` | `kind`: tag varian `Value` dari `codec.rs` (`TAG_TEXT` 6, `TAG_BYTES` 7, `TAG_JSON` 12 untuk `VARLEN`; tag varian untuk encoding tetap; 0 untuk `TAGGED`) |
| +2 | `u8` | `flags`: bit0 `HAS_VALIDITY`, bit1 `ALL_NULL`, bit2 `HAS_OPENABLE`, bit3 `HAS_NUMERIC`, bit4 `TS_UNIFORM_OFFSET`, bit5 `TS_NAIVE` |
| +3 | `u8` | `scale` (`DEC128`); selain itu 0 |
| +4 | `i32` | `param`: offset detik bersama untuk `TIMESTAMP` dengan `TS_UNIFORM_OFFSET`; selain itu 0 |
| +8 | `u32` | offset segmen dari awal chunk |
| +12 | `u32` | panjang segmen |
| +16 | `u32` | `null_count` |
| +20 | `u32` | `reserved = 0` |

### 4.2 Isi segmen per encoding

Urutan sub-array di dalam segmen tetap: validitas (bila `HAS_VALIDITY`), data, lalu bitmap openable (bila `HAS_OPENABLE`), lalu bitmap numerik (bila `HAS_NUMERIC`). Semua bitmap berukuran `ceil(rows / 8)` byte, bit ke-i di `byte[i / 8] >> (i % 8)` (LSB dulu, konvensi Arrow).

| Encoding | Data | Catatan |
|---|---|---|
| validitas | bit 1 = bukan NULL | Tidak ada bila `null_count == 0` (semua valid). `ALL_NULL` berarti segmen kosong. |
| `BOOL` | bitmap nilai | |
| `I64`, `U64`, `F64`, `TIME64` | `rows × 8` | Slot NULL berisi 0. `F64` menyimpan pola bit utuh (NaN, `-0.0`), sama seperti tes `a_float_keeps_its_exact_bits` hari ini. |
| `DATE32` | `rows × 4` (`i32` hari sejak epoch) | |
| `DEC128` | `rows × 16` (`i128` unscaled) | Hanya bila semua sel bukan NULL memakai satu `scale`. Skala campuran jatuh ke `TAGGED`. |
| `TIMESTAMP` | `rows × 8` mikrodetik instan, lalu `rows × 4` offset `i32` dengan `i32::MIN` = tanpa zona | Array offset dihapus bila `TS_NAIVE` (semua tanpa zona) atau `TS_UNIFORM_OFFSET` (satu offset di `param`). `micros` adalah instan (`render.rs:269-278`). |
| `VARLEN` | `(rows + 1) × u32` offset, lalu byte | `kind` membedakan `Text`, `Json` (UTF-8) dan `Bytes` (byte mentah; `to_text` = hex saat dirender). |
| `TAGGED` | `(rows + 1) × u32` offset, lalu blob `encode_value` per sel (termasuk `TAG_NULL`) | Codec bertag yang ada, tag tetap hanya boleh ditambah (`codec.rs:27-29`). |

### 4.3 Pemilihan encoding

Builder mencatat varian setiap sel yang bukan NULL. Saat disegel:

| Sel bukan NULL dalam chunk-kolom | Encoding |
|---|---|
| tidak ada | `TAGGED` + `ALL_NULL` |
| semua `Bool` | `BOOL` |
| semua `Int` / `UInt` / `Float` | `I64` / `U64` / `F64` |
| semua `Decimal` dengan satu skala | `DEC128` |
| semua `Date` / `Time` / `Timestamp` | `DATE32` / `TIME64` / `TIMESTAMP` |
| semua `Text`, semua `Json`, atau semua `Bytes` | `VARLEN` |
| campuran, `Interval`, `Array`, `Row`, `Map`, `Unknown` | `TAGGED` |

Builder menahan nilai dalam bentuk bertipe (`Vec<i64>`, offset plus byte, dan seterusnya), bukan `Vec<Value>`. Ketika varian pertama yang tidak cocok datang, builder kolom itu dikonversi sekali ke bentuk bertag. `ChunkBuilder` punya metode append bertipe publik (`push_i64`, `push_str`, `push_null`, …), supaya `ColumnarBuilder` Fase 7 bisa menulis langsung ke sini lewat trait di `qh-core`.

### 4.4 Aturan segel

**Koreksi AR: satu push, satu segel.** Setiap `push` menyegel semua baris batch itu sebelum kembali. Batch dipecah menjadi beberapa chunk hanya bila melewati 65.536 baris atau perkiraan 2 MiB. Builder tidak pernah menahan baris di antara dua push, sehingga:

- batch pertama (200 baris) langsung terlihat, jadi TTFR S2 tidak butuh aturan khusus;
- stream yang menetes terlihat begitu batch-nya tiba. Timer 16 ms, `seal_if_due`, `publish_deadline`, dan `select!` terhadap tenggat di loop pump tidak dibangun. Timer itu pun tidak bisa menolong baris yang masih ada di dalam `next_batch` driver, yang di PostgreSQL mengisi sampai `max_rows` sebelum kembali;
- jumlah chunk dibatasi jumlah batch. Pada 5 juta baris dengan fetch adaptif, itu sekitar 310 chunk, jauh di bawah ambang 4.096 R-9. Menggabungkan batch sampai 64k baris (`performance-plan.md` §10 butir 1) tidak memberi apa pun yang terukur: spill dan dekripsi dibatasi 2 MiB, bukan jumlah baris, dan chunk yang lebih kecil justru menambah paralelisme view. Penggabungan boleh ditambahkan di Fase 7 bila angka menunjukkannya.

Batas byte menjaga cache miss sinkron atas chunk yang tumpah: dekripsi paling banyak 2 MiB (§20 R-3). Driver yang mengembalikan batch kecil (halaman Trino awal) menghasilkan chunk kecil. Kompaksi tidak dibangun di Fase 6; ia ditinjau bila jumlah chunk > 4.096 terukur (R-9).

### 4.5 Decoder

`ChunkView::parse(&[u8])` memvalidasi magic, `rows ≤ 65.536`, batas direktori dan segmen, panjang setiap sub-array, serta offset `VARLEN`/`TAGGED` yang monoton dan ≤ panjang data. Byte `Text`/`Json` diperiksa `std::str::from_utf8` saat dibaca. Setiap pelanggaran menjadi `StoreError::Corrupt` dan tidak pernah panic. `Reader` yang ada (`codec.rs:357-436`) dipakai ulang. Tes `a_truncated_batch_is_an_error_not_a_panic` dan `a_damaged_value_tag_is_an_error_not_a_panic` dipindah ke layout baru.

### 4.6 Akuntansi memori

Yang dihitung: Σ panjang buffer chunk resident, ditambah cache chunk yang sudah didekripsi, ditambah reservasi view. Builder hanya hidup selama satu `push` (≤ 2 MiB per store yang sedang streaming) dan tidak dihitung, dan itu dinyatakan di doc modul sebagai ongkos tetap per store aktif.

## 5. Bitmap openable dan numerik

### 5.1 Openable, port persis `GridValue.isOpenable(value:type:)`

1. NULL atau teks tersimpan `""` → tidak openable.
2. Dari nama tipe kolom (`ColumnMeta::type_name`, di-lowercase), dihitung **sekali per kolom**:
   - *structured*: berakhiran `[]`, atau memuat `array`, `map(`, `row(`, atau `json`;
   - *binary*: memuat `bytea`, `blob`, atau `binary`.

   Bila salah satu benar, flag `openable_by_type` menyala dan setiap sel yang bukan NULL dan tidak kosong openable. Tidak ada bitmap untuk kolom seperti ini.
3. Selain itu, openable bila **ketiganya** benar:
   - karakter pertama setelah trim `White_Space` adalah `{` atau `[`. `CharacterSet.whitespacesAndNewlines` = Zs ∪ {U+0009–U+000D, U+0085, U+2028, U+2029}, yang sama persis dengan `char::is_whitespace` Rust;
   - panjang UTF-16 ≤ 100.000 (`GridValue.parseLimit`). Jalan pintas: panjang byte ≤ 100.000 pasti lolos, dan panjang byte > 300.000 pasti gagal;
   - `json::validate(text)` lulus. Ini validator iteratif tanpa pohon dan tanpa alokasi per node, di atas **teks yang tidak di-trim**, seperti `JSONSerialization` membaca `text.data(using: .utf8)`. Aturannya: grammar RFC 8259; whitespace JSON hanya spasi, tab, LF, dan CR; fragmen top-level boleh; kedalaman tidak dibatasi selain oleh panjang input; bilangan diperiksa sintaksnya saja tanpa rentang; escape string `\" \\ \/ \b \f \n \r \t \uXXXX`; karakter kontrol < 0x20 tanpa escape ditolak.

   Titik yang belum pasti di `JSONSerialization` (surrogate tunggal dalam `\u`, BOM, eksponen raksasa, kunci ganda, sampah di ekor) dikunci oleh `openable.json` (§12). Validator menyesuaikan diri dengan fixture, bukan sebaliknya.

Dihitung saat ingest untuk sel `VARLEN` `Text`/`Json` dan sel `TAGGED`, atas `to_text`-nya. Pemeriksaan karakter pertama yang murah dijalankan lebih dulu, jadi parse hanya terjadi pada sel yang tampak seperti objek atau array. Bitmap hanya disimpan bila ada bit yang menyala (`HAS_OPENABLE`). Sel bertipe tetap tidak pernah openable kecuali lewat `openable_by_type`, dan itu sama dengan Swift: `"42"` pada kolom `bigint` tidak openable, `5` pada kolom bertipe `json` openable.

Flag ini selalu tentang nilai **tersimpan**. Sel yang punya edit staged dinilai Swift dengan `GridValue.isOpenable` atas teks staged (D-9).

### 5.2 Numerik, semantik `GridSort.number(text) != nil`

| Encoding | Numerik |
|---|---|
| `I64`, `U64`, `DEC128` | ya, untuk setiap sel bukan NULL |
| `F64` | `swift_plain_number(to_text)`, sama seperti teks. "Finite" saja tidak cukup: `to_text` non-finite adalah `nan`/`inf`/`-inf`, dan `1e300` lolos sintaks tetapi di luar rentang eksponen `Decimal` Swift (−128…127), sehingga Swift memperlakukannya sebagai teks. |
| `BOOL`, `DATE32`, `TIME64`, `TIMESTAMP` | tidak (teksnya memuat huruf, `-`, atau `:`) |
| `VARLEN` `Text`/`Json`, `TAGGED` | `swift_plain_number(to_text)` |

`swift_plain_number` (di `collate.rs`) adalah port `GridSort.number`:

1. Trim `CharacterSet.whitespaces`: kategori Zs ditambah TAB, **tanpa** baris baru.
2. Harus tidak kosong. Tanda `+`/`-` opsional di depan.
3. Lalu digit ASCII dengan paling banyak satu `.`, dan minimal satu digit.
4. Opsional `e`/`E`, tanda opsional, lalu ≥ 1 digit. Setelah itu tidak boleh ada apa pun.
5. Nilainya harus terwakili oleh `Decimal` Swift. Batas eksponennya dikunci `number.json`.

Swift memakai `Character.isNumber`, yang juga menerima digit non-ASCII. Apakah `Decimal(string:)` lalu menerimanya ditentukan fixture. Bila tidak, hasilnya sama. Bila ya, masuk daftar divergensi.

Bitmap `HAS_NUMERIC` hanya ada untuk `VARLEN`/`TAGGED`. Untuk `F64`, flag dihitung saat dibaca dari teks yang memang sedang dirender. Pemakainya dua: pembangun kunci sort (sel non-numerik langsung ke kunci teks, sel numerik di-parse menjadi `NumKey`) dan flag `NUMERIC` di jendela (untuk `CellText` di seam Fase 5). **Perataan kanan tetap per tipe kolom** (`isNumeric(type)`) demi paritas visual.

## 6. Statistik lebar kolom

- `StoreShared::head_widths: Mutex<Vec<u32>>` diperbarui penulis selama baris ke-0…199 masuk (urutan kedatangan, sama dengan `preview.rows.prefix(200)` hari ini). Untuk setiap sel: NULL → 4; selain itu `min(graphemes(to_text(v)), 256)` lewat `UnicodeSegmentation::graphemes(s, true)` (extended), yang berhenti menghitung di 256. Nilai per kolom adalah maksimumnya.
- Kenapa 256 aman: rumus `min(max(n × 7,2 + 20, 84), 320) + 22` jenuh di n ≥ 42 (42 × 7,2 + 20 = 322,4 > 320). Batas berapa pun ≥ 42 memberi lebar yang sama.
- `column_widths()` mengembalikan salinan vektor itu. Swift menghitung `max(label.count, widths[source])` dengan label yang mungkin sudah diganti nama, lalu menerapkan rumus dan pembagian slack yang ada (D-10).
- Swift meminta ulang hanya pada batch pertama, saat `fetched` melewati 200, dan saat hasil lengkap. Tidak per frame dan tidak per body.
- Yang dihitung adalah teks **tersimpan**, bukan teks terformat, sama dengan `value.count` hari ini.
- Risiko versi Unicode antara `unicode-segmentation` dan runtime Swift dikunci `width.json` (R-11).

## 7. `StoreRegistry`, identitas store, anggaran global, dan urutan spill

### 7.1 Tipe

```rust
pub struct StoreConfig {                // menggantikan StoreConfig per store hari ini
    pub budget_bytes: usize,            // 256 MiB (O-12)
    pub low_water_bytes: usize,         // budget_bytes - 32 MiB
    pub spill_dir: Option<PathBuf>,     // None: spill mati (tes, pemakaian tanpa app)
    pub chunk_max_rows: u32,            // 65_536
    pub chunk_target_bytes: usize,      // 2 MiB
    pub decoded_cache_chunks: usize,    // 8 (performance-plan.md §10 butir 1)
}

pub struct StoreRegistry { /* config, cipher, live: Mutex<HashMap<StoreId, Weak<StoreShared>>>, next_id, resident, clock, decoded */ }
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
    pub fn push(&self, batch: &ColumnBatch) -> Result<(), StoreError>;         // segel seluruh batch (§4.4), bisa evict
    pub fn finish(&self, outcome: Outcome) -> Result<(), StoreError>;          // Complete{truncated} | Cancelled | Failed
    pub fn rows(&self) -> u32;
}
```

### 7.2 Identitas store dan handle basi

**Koreksi AR: tanpa slab slot.** Rancangan awal memakai slab `(slot, generation)` dengan free list. Itu idiom untuk handle berupa indeks. Di sini handle tidak pernah berupa indeks: `ResultHandle` memegang `Arc<StoreShared>` miliknya sendiri, jadi pemakaian ulang slot tidak mungkin membuat handle lama menunjuk store baru, dan use-after-free lintas FFI mustahil secara konstruksi (UniFFI memegang `Arc`, `window` mengembalikan salinan `Vec<u8>`, tidak ada pointer pinjaman yang menyeberang).

- `StoreId(u64)` diambil dari `next_id: AtomicU64` milik registry dan tidak pernah dipakai ulang selama umur proses. Ia berperan sebagai "generation" di `performance-plan.md` §10 butir 3 dan NFR-S3: satu nilai yang unik per store, bukan pasangan slot dan generation.
- Registry menyimpan `live: Mutex<HashMap<StoreId, Weak<StoreShared>>>` hanya untuk evict lintas store dan `stats()`. `release` menghapus entrinya.
- Setiap operasi publik memeriksa `phase != Released`. Bila sudah dilepas, hasilnya `StoreError::Released`, yang menjadi `StaleHandle` di FFI. Ini jawaban `rust-engine-blueprint.md` §2.6 untuk handle basi setelah tab ditutup.
- `StoreId` ikut di AAD spill (§8.3), sehingga rekaman satu store tidak pernah terautentikasi untuk store lain. Header jendela tidak membawanya: cache halaman Swift dimiliki satu `StoreRows`, yang memegang satu handle (§14.2).

### 7.3 Indeks chunk dan publikasi

- `chunks: RwLock<Vec<ChunkEntry { first_row: u32, rows: u32, cell: Arc<ChunkCell> }>>`, dengan `ChunkCell { residency: Mutex<Residency>, last_access: AtomicU64 }` dan `Residency = Resident(Arc<[u8]>) | Spilled { offset: u64, len: u32, nonce: u64 }`.
- Penulis menyegel chunk di luar kunci, mengambil kunci tulis hanya untuk `push` entri, melepasnya, lalu `rows.store(total, Release)`.
- Pembaca melakukan `rows.load(Acquire)`, mengambil kunci baca, menemukan chunk lewat `partition_point` atas `first_row` (seperti `batch_index_for` hari ini), meng-clone `Arc` yang perlu, lalu melepas kunci.
- `phase: AtomicU8` bernilai `Empty → Streaming → Complete | Cancelled | Failed → Released`.

### 7.4 Anggaran

- `resident: AtomicUsize` menghitung buffer chunk resident, cache dekripsi, dan reservasi.
- `charge(bytes, origin)`: bila hasilnya > anggaran dan asalnya penulis atau view, `evict_until(low_water)` dijalankan sinkron di thread pemanggil. Thread penulis adalah thread FFI yang menjalankan run, jadi bukan main.
- **Main thread tidak pernah menulis ke disk.** Cache dekripsi (paling banyak 8 chunk × 2 MiB = 16 MiB) punya jatah tetap di dalam anggaran dan hanya membuang entrinya sendiri.
- `reserve(bytes) -> Reservation` untuk scratch view. Ia meng-evict untuk memberi ruang, atau gagal dengan `BudgetExceeded { needed, budget }`. `Drop` pada reservasi mengembalikan anggaran.

### 7.5 Urutan evict (LRU)

`last_access` adalah jam logis registry (`AtomicU64`), diperbarui saat chunk disegel, dibaca jendela, atau dipindai view. `evict_until(target)`:

1. Buang entri cache dekripsi, yang tertua dulu. Tanpa I/O.
2. Kumpulkan chunk resident dari semua store hidup, kecuali chunk yang sedang dipinjam (`Arc::strong_count > 1`: sedang dibaca jendela atau disemat sort). Urutkan menurut `last_access` naik, lalu tumpahkan satu per satu sampai `resident ≤ target`. **Kunci `residency` tidak dipegang selama enkripsi dan `pwrite`** (koreksi AR): evictor meng-clone `Arc` di bawah kunci, melepasnya, menyegel dan menulis, lalu mengambil kunci lagi hanya untuk menukar `Resident` menjadi `Spilled`. Tanpa ini, jendela di main yang menyentuh chunk itu menunggu I/O ~2 MiB, dan D-3 ("pembaca tidak pernah menunggu penulis") batal. Pemeriksaan `strong_count` bersifat heuristik: pembaca yang meng-clone tepat sesudahnya tetap aman, hanya akuntansinya sesaat lebih rendah dari kenyataan.
3. Bila spill mati atau gagal, galatnya kembali ke pemanggil `charge`.

Akibatnya, yang tumpah lebih dulu adalah store dasar Batch 7 yang menganggur, tab di latar, dan chunk tua dari hasil yang sedang streaming. Halaman yang sedang dilihat tidak. Pemindaian O(jumlah chunk) dibayar sekali per putaran evict, dan histeresis 32 MiB (~16 chunk) mengamortisasinya.

### 7.6 Release

`registry.release(key)` bersifat idempoten:

- menandai `Released`;
- mengosongkan indeks chunk dan mengembalikan anggaran;
- menutup fd spill, sehingga kernel membebaskan blok;
- menyalakan flag cancel view yang sedang berjalan;
- menghapus entri `live`.

`StoreHandle::drop` memanggil `release`. `StoreWriter` yang masih dipegang run mendapat `Released` pada push berikutnya, dan loop pump memperlakukannya sebagai cancel. Pembaca yang masih memegang `Arc<[u8]>` selesai dengan aman, dan memorinya bebas sesudahnya.

### 7.7 Sapuan startup

`StoreRegistry::new`, bila `spill_dir` diisi:

1. `symlink_metadata` diperiksa **lebih dulu**, karena `set_permissions` mengikuti symlink. Bila path itu symlink atau bukan direktori, spill dimatikan dengan alasan tertulis.
2. Bila belum ada, direktori dibuat dengan `DirBuilder::new().recursive(true).mode(0o700)`. Bila sudah ada, `set_permissions(0o700)`.
3. Untuk setiap entri `read_dir`: berkas biasa atau symlink di-`remove_file` (tidak pernah diikuti, tidak pernah rekursif). Subdirektori dibiarkan.

Peninggalan `$TMPDIR/queryhive-spill/spill-*.bin` dari codec lama **tidak** disapu (koreksi AR): crate itu tidak pernah dipakai build produk mana pun (fakta 1), jadi berkas seperti itu hanya bisa berasal dari `cargo test` di mesin pengembang.

Sapuan aman walau ada instance app lain yang hidup. Berkas mereka sudah di-unlink, dan bila sapuan menang balapan di jendela antara `open` dan `unlink`, akibatnya sama dengan unlink mereka sendiri (`ENOENT` di pihak mereka diabaikan). `SweepReport { removed }` dicatat host ke log.

## 8. Spill terenkripsi dan siklus hidup kunci (NFR-S3)

### 8.1 Kunci

- 32 byte dari `ring::rand::SystemRandom::new().fill()` → `UnboundKey::new(&AES_256_GCM, &bytes)` → `LessSafeKey`. Array asal di-`zeroize` segera sesudahnya. `ring` tidak meng-zeroize jadwal kunci AES di dalam `LessSafeKey` saat drop. Itu diterima dan ditulis di doc modul, karena umur kunci sama dengan proses; dokumen tidak boleh mengklaim kunci "dihapus dari memori".
- Kunci hanya hidup di dalam `LessSafeKey`, di memori proses. Ia tidak pernah ditulis ke disk, dicatat di log, dicetak `Debug` (`SpillCipher` punya `Debug` manual yang menulis `<redacted>`), atau menyeberang FFI.
- Umur kunci sama dengan registry, yaitu `EngineHost`, yaitu proses app. Saat proses berakhir kunci hilang, dan sisa data apa pun tidak bisa dibaca lagi. Sapuan adalah kebersihan, bukan kerahasiaan.
- Bila `SystemRandom::fill` gagal, spill dimatikan. Tidak ada fallback ke kunci tetap.

### 8.2 Nonce

- `counter: AtomicU64` milik `SpillCipher` dimulai dari 1. Setiap segel mengambil `fetch_add(1)`, dan nonce = `[0, 0, 0, 0] ‖ counter.to_be_bytes()` (12 byte).
- Kunci dan penghitung hidup di satu objek, dan penghitung hanya naik, jadi nonce tidak pernah dipakai ulang di bawah kunci yang sama, di store mana pun. Pada `u64::MAX` spill menolak (`SpillUnavailable`).
- Nonce disimpan di indeks memori, bukan di berkas.

### 8.3 AAD dan format rekaman

- AAD (16 byte) = `b"QHS1" ‖ store_id u64 LE ‖ chunk_index u32 LE`. `store_id` tidak pernah dipakai ulang (§7.2), jadi ia memenuhi "id store dan generation" NFR-S3 sekaligus. Ditambah indeks chunk dan tag domain.
- Rekaman di berkas hanya `ciphertext ‖ tag 16 byte`, disambung di `next_offset` dan tidak pernah ditulis ulang. Berkas tidak punya header dan tidak punya metadata teks biasa sama sekali.
- **Segel:** salin byte chunk ke `Vec` baru (bukan scratch bersama, supaya evict dari dua thread penulis tidak saling mengunci), `seal_in_place_separate_tag(nonce, Aad::from(aad), &mut buf)`, tambahkan tag, `write_all_at(offset)`. Salinan diperlukan karena pembaca mungkin masih memegang `Arc` yang sama. Catat `Spilled { offset, len = plain + 16, nonce }`.
- **Buka:** `read_exact_at`, lalu `open_in_place(nonce, aad, &mut buf)`. Hasilnya masuk cache dekripsi sebagai `Arc<[u8]>`. Kegagalan autentikasi menjadi `StoreError::SpillAuth { chunk }`. Byte yang tidak terautentikasi tidak pernah dikembalikan.

### 8.4 Berkas

- Dibuat saat spill pertama sebuah store: `spill_dir/qhs-<pid>-<store_id>-<16 hex acak>.spill`, lewat `OpenOptions::new().read(true).write(true).create_new(true).mode(0o600)`. `create_new` berarti `O_EXCL`, jadi path yang sudah ada dan symlink ditolak.
- Segera sesudahnya, **sebelum byte pertama ditulis**, `remove_file(path)`. `ENOENT` (sapuan instance lain menang balapan) dianggap berhasil. Galat unlink lain menutup fd dan menjadi `StoreError::Io`: spill tanpa unlink tidak dijalankan, supaya direktori tetap kosong. `File` tetap terbuka, dan blok dibebaskan saat fd ditutup (release, drop, atau crash). Direktori `~/Library/Caches/QueryHive/spill` (dari Swift, §14.6) karena itu selalu kosong. Karena kosong itu pasti, G-LEAK membuktikan pelepasan lewat `store_stats().stores == 0`, bukan lewat isi direktori.
- Satu fd per store yang pernah tumpah. Tab di latar tumpah lebih dulu, jadi 100 tab terbuka bisa memegang ~200 fd, dan batas lunak app GUI macOS adalah 256. `EMFILE` diperlakukan seperti disk penuh: chunk tetap resident dan run yang memicunya mendapat satu event `error` (R-19).
- I/O lewat trait `SpillMedium { write_at, read_at, len }` dengan `FileMedium` untuk produk dan `FaultyMedium` untuk tes injeksi `ENOSPC`.
- **Disk penuh:** `ErrorKind::StorageFull` atau `raw_os_error() == Some(28)` menjadi `StoreError::DiskFull`; galat I/O lain menjadi `StoreError::Io`. Chunk tetap resident dan store lain tidak terpengaruh. Run yang memicu evict berakhir dengan satu event `error`: "The result is larger than the memory budget and the disk has no room to spill it. Free some disk space or lower the row limit."

### 8.5 Tes wajib (W4-T3, `crates/qh-result-store/tests/spill.rs` dan unit di `spill.rs`)

| Tes | Isi |
|---|---|
| `no_plaintext_on_disk` | Canary ASCII, UTF-8, dan UTF-16 di kolom teks, ditambah canary `i64` di kolom angka. Anggaran kecil memaksa spill. Byte mentah dibaca lewat fd (`SpillFile::read_raw_for_test`, `#[doc(hidden)]`). Canary dan magic `"QHC1"` tidak boleh ditemukan. |
| `a_wrong_key_does_not_open` | Rekaman disegel cipher A lalu dibuka cipher B: `SpillAuth`. |
| `tampering_is_detected` | Satu bit dibalik, dua rekaman ditukar (AAD indeks berbeda), dan rekaman diputar ulang ke store lain (AAD `store_id` berbeda). Semuanya `SpillAuth`, tanpa panic dan tanpa data salah. |
| `nonces_are_never_reused` | 10.000 segel di tiga store: semua nonce unik. |
| `modes_are_0700_and_0600` | Mode direktori lewat metadata, mode berkas lewat `File::metadata()` (fstat pada fd yang sudah di-unlink). |
| `the_spill_directory_stays_empty` | Setelah spill terjadi, `read_dir` kosong. Urutan unlink-sebelum-tulis dijaga oleh bentuknya: `SpillFile::create` baru mengembalikan fd setelah unlink berhasil. |
| `orphans_are_swept_without_following_links` | Berkas, symlink ke luar direktori, dan subdirektori. Berkas dan symlink hilang, target symlink utuh, subdirektori tetap. |
| `disk_full_is_an_error_not_a_panic` | `FaultyMedium` mengembalikan `ENOSPC`. `push` → `DiskFull`, dan baris resident tetap terbaca. |
| `the_key_is_not_in_debug_output` | `format!("{registry:?}")` tidak memuat byte kunci. |

## 9. `window()`: buffer terkemas dan potongan 256 UTF-16

### 9.1 Layout (little-endian, tanpa padding; Swift membaca dengan `loadUnaligned`)

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

**Koreksi AR:** gema `slot`, `generation`, `view_id`, dan daftar kolom dihapus dari rancangan awal. Semuanya sudah diketahui Swift dari permintaannya sendiri: handle dimiliki satu `StoreRows`, `view_id` dikirim sebagai argumen (dan view yang tidak cocok menjadi `StaleView`, bukan jendela), dan kolom adalah argumen. Prefetch async membawa `view_id` permintaannya di closure. Gema hanya menambah medan yang harus divalidasi tanpa menangkap kesalahan baru. Layout ini sudah minimum untuk NFR-P8: satu buffer, satu salinan `RustBuffer → Data`, offset `u32` per sel, dan tidak ada string per sel yang di-marshal. Alternatif yang lebih sederhana (misalnya `Vec<Option<String>>` lewat UniFFI) membayar alokasi dan konversi per sel, yaitu 4.096 string per jendela.

Panjang total harus sama dengan `S2 + H`. Swift memeriksa magic, versi, panjang total, dan bahwa offset monoton serta ≤ H sebelum membaca. Pemeriksaannya O(RC) dan murah.

Flag sel:

| Bit | Nama | Arti |
|---|---|---|
| 0 | `NULL` | teks kosong. Grid menggambar gaya NULL (`DataPreferences.nullDisplay`). |
| 1 | `EMPTY` | tersimpan `""` bukan NULL. Grid menggambar `∅`. |
| 2 | `OPENABLE` | §5.1, atas nilai tersimpan |
| 3 | `NUMERIC` | §5.2 |
| 4 | `TRUNCATED` | teks tampilan dipotong di 256 unit UTF-16; nilai penuh lewat `cell_text` |
| 5–7 | — | 0, dicadangkan |

### 9.2 Render satu sel

1. Ambil nilai dari chunk. `VARLEN` `Text`/`Json` meminjam `&str` langsung, karena untuk varian ini `to_text` adalah identitas (`render.rs:72`, `:83`). `Bytes` memakai `render::hex_encode`. Encoding tetap membangun `Value` di stack lalu memanggil `qh_core::render::to_text`. `TAGGED` memakai `decode_value`, lalu `to_text`. Satu tes memastikan bahwa untuk setiap encoding, teks jendela sama dengan `to_text(nilai hasil decode)`. Renderer tetap satu.
2. Flag dibaca dari validitas dan bitmap.
3. Format kolom diterapkan (§9.4).
4. Mode tampilan: potong (§9.3). Mode penuh (`rows_text`, `cell_text`): tidak dipotong.
5. Tulis ke heap.

### 9.3 Potongan layout 256 UTF-16

- Jalan pintas: panjang byte ≤ 256 berarti tidak perlu dipotong, karena jumlah unit UTF-16 ≤ jumlah byte UTF-8.
- Selain itu, ambil prefiks terpanjang yang berakhir di batas extended grapheme cluster dengan panjang UTF-16 ≤ 256. Bila grapheme pertama saja sudah > 256 unit (zalgo), potong di batas `char` terakhir yang muat. Surrogate pair tidak pernah terbelah, karena pemotongan terjadi di batas `char`.
- Tanpa elipsis: Fase 5 menggambar token pemotongnya sendiri. Flag `TRUNCATED` menyala.
- Baris baru dibiarkan apa adanya. Cara menggambarnya dalam satu baris adalah urusan Fase 5.

### 9.4 Port `ColumnFormat` (`render.rs` di crate store)

| Format | Aturan (port persis) |
|---|---|
| `Raw` | teks tersimpan |
| `Text` | Bila tipe kolom *binary* (§5.1): untuk sel `Bytes` bertipe, pakai byte mentahnya langsung (setara dengan jalan hex bolak-balik). Untuk sel teks, port `HexDump.decodedHex`: buang prefiks `\x`/`\X`/`0x`/`0X`; jumlah `Character` harus genap; semua scalar hex; bila gagal, pakai byte UTF-8 teks. Hasilnya `String::from_utf8_lossy`, yang aturan penggantian maximal-subpart-nya sama dengan `String(decoding:as:)` (dikunci fixture). Tipe lain: teks tersimpan. |
| `Uuid` | Semua karakter harus hex digit atau `-`. Hex digit versi Swift juga mencakup bentuk fullwidth U+FF10–FF19, U+FF21–FF26, dan U+FF41–FF46. Digit = lowercase lalu filter hex; bila tepat 32, tulis 8-4-4-4-12; selain itu teks tersimpan. |
| `UnixTimestamp` | Trim `White_Space`, lalu `swift_double`. Harus finite. Detik = n / 1000 bila \|n\| ≥ 1e11, selain itu n. Pecahan dibuang ke bawah (floor; dikunci fixture untuk nilai negatif). Tanggal lewat `qh_core::render::civil_from_days`, format `yyyy-MM-dd HH:mm:ss` UTC. Tahun < 1 dan > 9999 mengikuti fixture. Bukan bilangan: teks tersimpan. |
| `Json` | Panjang UTF-16 ≤ 100.000, parse (`serde_json::Value`), lalu cetak meniru `JSONSerialization` `[.prettyPrinted, .fragmentsAllowed, .sortedKeys]`: indentasi dua spasi, `"key" : value`, `/` di-escape menjadi `\/`, non-ASCII apa adanya, urutan kunci dan bentuk bilangan sesuai fixture. Lalu `\n` → spasi. Gagal parse: teks tersimpan. **Urutan kunci ditulis eksplisit oleh printer**, bukan diwarisi dari `serde_json::Map`: workspace menyalakan `serde_json` fitur `preserve_order` (`Cargo.toml:52`), dan feature unification membuat `Map` di crate ini juga menjaga urutan input. |

**Pintu darurat `Json`.** Bila W4-T4 menunjukkan paritas printer JSON belum 100% di korpus, kolom berformat `Json` dikirim dengan teks tersimpan, dan Swift menerapkan `ColumnFormat.render` untuk sel yang terlihat saja. Keputusan itu dicatat di `differential.rs` dan di ADR-0030. Format lain tidak punya pintu darurat. Syarat pintu darurat: tidak boleh ada panggilan FFI per sel di jalur gambar. Jendela untuk kolom itu dikirim tanpa potongan 256 (flag per kolom di permintaan, diputuskan W6-A1), supaya Swift punya teks penuh untuk diformat tanpa `cell_text` per sel.

### 9.5 Batas dan target

- `row_count ≤ 4.096`, `column_count ≤ 1.024`, R × C ≤ 262.144; di luar itu `InvalidArgument`. `rows_text` yang total bytenya > 64 MiB menjadi `TooLarge`, dan Swift memecah permintaannya.
- Target: jendela tampilan 128 baris × 32 kolom dari chunk resident, p99 ≤ 0,5 ms termasuk UniFFI (NFR-P8), untuk format `Raw`, `Text`, `Uuid`, dan `UnixTimestamp`. Format `Json` mem-parse dan mencetak ulang setiap sel sampai 100.000 unit, jadi satu jendela bisa jauh di atas 0,5 ms. Ia diukur terpisah (`bench_ffi window-json`) dan dicatat, bukan digate, karena pengguna memilihnya per kolom dan jalur Swift hari ini membayar ongkos yang sama per render. Diukur `bench_ffi window` (Rust) dan `StoreWindowBench` (Swift). Bila meleset, jalurnya eskalasi C ABI Fase 8.

## 10. Permukaan FFI

### 10.1 Rekaman, enum, galat (`crates/qh-ffi/src/store_api.rs`)

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

### 10.2 `ResultHandle`: semua metode throwing

```rust
#[derive(uniffi::Object)]
pub struct ResultHandle { inner: qh_result_store::StoreHandle }

#[uniffi::export]
impl ResultHandle {
    pub fn row_count(&self) -> Result<RowCount, StoreFfiError>;               // load atomik saja, tanpa efek samping (§11.8)
    pub fn columns(&self) -> Result<Vec<ColumnWire>, StoreFfiError>;
    pub fn window(&self, view_id: u64, first_row: u32, row_count: u32,
                  columns: Vec<u32>, formats: Vec<CellFormat>) -> Result<Vec<u8>, StoreFfiError>;
    pub fn rows_text(&self, view_id: u64, first_row: u32, row_count: u32,
                     columns: Vec<u32>) -> Result<Vec<u8>, StoreFfiError>;     // layout §9.1, CUT = 0, Raw
    pub fn cell_text(&self, view_id: u64, row: u32, column: u32,
                     format: CellFormat) -> Result<Option<String>, StoreFfiError>;  // penuh; None = NULL
    pub fn column_widths(&self) -> Result<Vec<u32>, StoreFfiError>;
    pub fn set_view(&self, spec: ViewSpec) -> Result<ViewInfo, StoreFfiError>; // memblokir; dipanggil off-main
    pub fn distinct_values(&self, column: u32, limit: u32) -> Result<DistinctValues, StoreFfiError>;
    pub fn release(&self) -> Result<(), StoreFfiError>;                         // idempoten
}
```

Setiap badan metode dibungkus `guarded(|| …)`: `catch_unwind(AssertUnwindSafe)`, lalu payload panic menjadi `Internal { message }` dan dicatat sebagai bug (ADR-0009). Store di belakang kunci yang poisoned ditandai `Failed`. Karena semua metode throwing, panic yang lolos dari `guarded` pun menjadi galat Swift (`rustPanic`), bukan `try!` yang crash. `view_id` yang tidak sama dengan view aktif menjadi `StaleView { current }`, dan Swift memuat ulang.

### 10.3 `EngineHost` (sambungan ke `host.rs` Fase 2)

```rust
// EngineHost::new() Fase 2 tetap tanpa argumen dan tanpa I/O (fase-2-engine-host.md §3).
pub fn configure_result_stores(&self, spill_dir: Option<String>, budget_bytes: u64)
    -> Result<StoreSweep, StoreFfiError>;              // sekali, off-main: bangun registry dan sapu (§7.7)
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
- **Tidak ada registry bawaan implisit** (koreksi AR). `create_result_store` dan `store_from_rows` sebelum `configure_result_stores` menjadi `InvalidArgument("result stores are not configured")`. Tes dan scene snapshot memanggil `configure_result_stores(None, …)` sendiri (spill mati). Rancangan awal membangun registry bawaan tanpa spill secara diam-diam. Bila app memanggil `configure` di antrean latar dan Run yang dipulihkan sesi lebih dulu membuat store, registry tanpa spill itu menang, `configure` lalu ditolak, dan hasil 5 juta baris gagal di anggaran tanpa sebab yang terlihat. Itu fallback senyap.
- Bila direktori spill tidak aman, registry tetap hidup dengan spill mati. Ingest yang melewati anggaran lalu gagal dengan pesan yang menyebut alasannya.
- `run_with_store` dirutekan seperti `preview`/`explain` di `host.run`, yaitu `Pooled(Query)` (fase-2-engine-host.md §3). Jalur sesi dan reset tidak berubah; yang berbeda hanya emitter-nya.

## 11. View di Rust

### 11.1 Pool

`qh_rt::view_pool() -> &'static rayon::ThreadPool`, dibangun sekali (`OnceLock`) dengan `num_threads = cores().performance.max(1)`, `thread_name("qh-view-{i}")`, dan `start_handler(|_| set_thread_qos(Qos::UserInitiated))`. Semua kerja paralel view berjalan di dalam `view_pool().install(…)`. Pool ini tidak dipakai ingest, jadi ingest tidak bersaing dengan dirinya sendiri.

### 11.2 Pipeline `set_view(spec)`

Urutannya sama dengan `displayedRows` (`QueryTab.swift:824-848`): filter (AND per kolom), lalu search, lalu sort.

1. Ambil snapshot indeks chunk dan `fetched`.
2. Bila ada filter atau search, jalankan paralel per chunk. Setiap chunk menghasilkan `Vec<u32>` baris sumber yang lolos, dan hasilnya disambung dalam urutan chunk, sehingga urutan sumber terjaga. Bila tidak ada, kandidatnya `0..fetched`.
3. Bila ada sort: syaratnya fase `Complete` atau `Cancelled`; bila masih streaming, jawabannya `Streaming`. Bangun kunci secara paralel, lalu `par_sort_unstable_by` dengan indeks baris sumber sebagai pemutus seri terakhir. Hasilnya setara sort stabil.
4. Flag cancel diperiksa per chunk di setiap fase.
5. Tukar `current_view` secara atomik dengan id baru (`view_seq.fetch_add`). Spec kosong (tanpa sort, tanpa filter, search kosong setelah trim) menghasilkan view identitas tanpa permutasi.

`set_view` baru menyalakan flag cancel milik yang sebelumnya, dan yang lama menjawab `Superseded`. Grid tetap menampilkan view lama sampai yang baru siap.

### 11.3 Kunci sort dan comparator

Urutan naik: `Num` < `Temporal` < `Text` < `Null`. Menurun membalik seluruh urutan, jadi NULL di depan, seperti Swift (`GridSort.swift:73-77`). Seri selalu diputus indeks baris sumber naik, di kedua arah, sama dengan `left.offset < right.offset`.

| Sel | Kunci |
|---|---|
| NULL | `Null` |
| `Int`, `UInt`, `Decimal`, `Float` finite | `Num` |
| `Float` non-finite, `Bool`, `Bytes`, `Interval`, komposit, `Json`, `Unknown`, `Text` | jalur teks atas `to_text`: bila `swift_plain_number`, `Num`; selain itu `Text` (kunci natural) |
| `Date`, `Timestamp` | `Temporal(0, micros instan)`; tanggal = tengah malam |
| `Time` | `Temporal(1, micros)` |

- **`NumKey`** membandingkan nilai desimal eksak: tanda, eksponen yang disesuaikan, lalu mantissa. `-0 == 0`. Float diubah lewat representasi terpendeknya (teks yang juga dilihat Swift), sehingga urutan antar-float sama dengan urutan `f64` dan konsisten terhadap desimal. Presisi mengikuti `Decimal` Swift, 38 digit signifikan. Apakah digit ke-39 dan seterusnya dibulatkan atau dipotong dikunci `number.json`.
- **Nilai bertipe diurutkan menurut nilainya.** Timestamp dengan offset campuran menjadi kronologis (`performance-plan.md` §14 butir 4). Tes diferensial hanya memakai nilai `Text` (§12), jadi paritas Swift diuji atas jalur teks, dan jalur bertipe diuji oleh tes unit Rust sebagai perilaku yang disengaja.
- **Total order wajib.** Sejak Rust 1.81, sort bisa panic bila comparator tidak total. Comparator adalah perbandingan leksikografis atas level-level yang masing-masing total, lalu indeks baris.
- **Seri `Num` dan `Temporal` langsung diputus indeks baris, tanpa byte mentah** (koreksi AR). Swift menganggap `"1.0"` dan `"1"` sama (`Decimal ==`, lalu `offset`), sehingga pemutus byte di antaranya akan diam-diam mengubah urutan relatif baris yang sama nilainya. Level byte mentah (§11.4 butir 5) hanya milik kunci `Text`. Tes acak berbenih memeriksa antisimetri, transitivitas atas triple, dan konsistensi prefiks.

### 11.4 Kunci natural (pengganti `localizedStandardCompare`, O-9)

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

**Prefiks.** Setiap entri sort menyimpan `prefix: u128`, yaitu 16 byte pertama enkoding primer yang memcmp-able: satu byte kelas per elemen; huruf Latin 1 byte; simbol dan skrip lain 3 byte code point big-endian; run digit berupa satu byte jumlah digit signifikan (255 berarti "≥ 255", dan sisanya dibandingkan penuh) lalu digitnya. Bila prefiks sama, perbandingan jatuh ke comparator penuh atas string asli.

String untuk comparator penuh:

- **Chunk resident** dipinjam lewat `Arc` yang disemat selama sort. Chunk yang disemat tidak bisa di-evict (§7.5).
- **Chunk yang tumpah** hanya kolom sort-nya yang didekripsi ke scratch yang dicadangkan (`reserve`). Bila cadangan tidak cukup, hasilnya `TooLarge { needed_bytes }`, dan Swift menampilkan "Sorting these rows in memory needs about N MB, more than the result budget allows."

Entri berukuran 24 byte (`prefix`, `row`, rujukan string), jadi 12 MB untuk 500k baris, dan dicadangkan.

**Kandidat divergensi**, yang dikonfirmasi atau dibantah fixture lalu didaftar di `differential.rs`:

- urutan antar-skrip (ICU menaruh Hangul sebelum Kana dan Han);
- emoji dan simbol;
- digit non-ASCII dan fullwidth di dalam run numerik;
- pengurutan khusus lokal (misalnya `ch` di cs, `å` di sv);
- urutan tersier ICU yang sebenarnya.

### 11.5 Filter (port `ColumnFilter.matches`)

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

### 11.6 Search (port `GridSearch.matches`)

`term' = trim(term, White_Space)`. Bila kosong, tidak ada search. Sebuah baris cocok bila ada sel bukan NULL di **semua** kolom (termasuk yang disembunyikan) yang `to_text`-nya `ci_contains(term')`. Teksnya teks tersimpan, bukan teks terformat.

### 11.7 Case-insensitive yang "localized"

`fold(s)` = NFC, lowercase penuh, lalu tabel pelipatan khusus (`ß → ss`, ligatur U+FB00–FB06, `ς → σ`, `İ → i̇`). `ci_contains(h, n)` = `fold(n)` dicari di `fold(h)`, dan `ci_equal` = kesamaan setelah `fold`.

Jalan pintas ASCII: bila keduanya ASCII, dipakai pencarian ASCII case-insensitive tanpa alokasi. Buffer lipat per worker rayon dipakai ulang.

Kandidat divergensi:

- aturan Turki ketika locale tr;
- kecocokan di dalam cluster kombinasi yang tidak punya bentuk precomposed (Foundation menghormati batas composed character sequence);
- lipatan khusus lokal.

Locale saat ekspor dicatat di manifest fixture.

### 11.8 View selama streaming

Filter dan search boleh diterapkan saat streaming; sort tidak. View filter menyimpan `rows: RwLock<Vec<u32>>` dan `scanned: u32`. **Perluasan dipicu penulis, bukan pembaca** (koreksi AR): setelah `push` mempublikasikan chunk, bila view aktif adalah view filter dengan `scanned < fetched`, penulis menjadwalkan paling banyak satu tugas perluasan di `view_pool` (penjaga `AtomicBool`) yang memindai `[scanned, fetched)` lalu menambahkan hasilnya. `set_view` yang selesai saat stream masih berjalan memeriksa sekali lagi, supaya chunk yang terbit di antaranya tidak tertinggal. Dengan begitu `row_count()` tetap load atomik murni seperti di `performance-plan.md` §10 butir 3, dan main thread tidak pernah menjadwalkan kerja. View identitas: `visible == fetched`.

### 11.9 Daftar distinct (port `ColumnFilter.distinctValues`)

`distinct_values(column, limit)` memindai baris yang sudah diambil dalam urutan store, **bukan** view terfilter, sama seperti `preview.rows` hari ini. Ia mengumpulkan himpunan terurut byte (setelah NFC) dan menandai `has_null`. Begitu `(jumlah bukan NULL + has_null) > limit`, pemindaian berhenti dengan `more = true` dan daftar kosong. Selain itu hasilnya `[None bila has_null] + bukan NULL terurut`. Swift memanggilnya dengan `limit = valuePickerLimit + 1 = 11` dan tetap memakai aturannya sendiri: bisa dipilih bila `!more && values.count ≤ 10`.

### 11.10 Target

NFR-P8: sort 500k numerik ≤ 100 ms, teks ≤ 300 ms, off-main. Filter dan search 500k × 30 dicatat (sasaran ≤ 300 ms). Semua diukur di `bench_ffi` skenario `view-*`.

## 12. Tes diferensial dan format fixture (W4-T4)

### 12.1 Pembangkit di Swift

`app/Tests/QueryHiveTests/SortFixtureExport.swift` berisi dua tes:

- `testExportGridFixtures`: hanya berjalan bila `QH_EXPORT_FIXTURES=1` (selain itu `XCTSkip`), dengan pola yang sama dengan `LexerFixtureExport` di W3-T2. Ia menjalankan implementasi Swift atas korpus dan menulis JSON ke `crates/qh-result-store/tests/fixtures/grid/`, dengan path diturunkan dari `#filePath`.
- `testCommittedFixturesStillMatchSwift`: selalu berjalan. Ia membaca fixture yang ter-commit dan memeriksa bahwa implementasi Swift yang **masih ada** menghasilkan hal yang sama. Setelah W6-T1, bagian sort, filter, search, dan distinct dilewati karena implementasinya sudah dihapus; `format`, `openable`, dan `width` tetap diperiksa (D-9).

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

### 12.2 Format berkas (JSON, satu berkas per perhatian, `null` = NULL)

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

### 12.3 Pemeriksa di Rust

`crates/qh-result-store/tests/differential.rs` membangun store lewat `StoreRegistry::from_text_rows`, sehingga semua nilai adalah `Value::Text`. Ia menjalankan API publik yang sama dengan yang dipakai app (`set_view`, `render_window`, `distinct_values`, `column_widths`) dan membandingkan hasilnya.

Divergensi didaftar eksplisit:

```rust
struct Divergence { file: &'static str, case: &'static str, detail: &'static str }
const KNOWN_DIVERGENCES: &[Divergence] = &[ /* diisi W4-T4 dari ketidakcocokan yang teramati, masing-masing dengan alasan */ ];
```

Tes gagal karena salah satu dari dua hal: ada ketidakcocokan yang tidak terdaftar, atau ada entri terdaftar yang tidak lagi berbeda (entri basi). Dengan begitu daftarnya tetap jujur. Laporan sort menyebut sampai 20 pasangan nilai yang urutannya berbeda. Daftar akhirnya masuk ADR-0034.

Tes acak berbenih tanpa dependensi baru menguji totalitas comparator dan konsistensi prefiks (§11.3). `proptest` hanya dipakai bila `cargo deny` lulus, seperti W3-T2.

## 13. Ingest

### 13.1 `RowTarget` dan `pump_result` (`crates/qh-ffi/src/commands.rs`)

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
- **Tanpa tenggat publikasi** (koreksi AR): `accept` menyegel seluruh batch (§4.4), jadi loop pump tidak butuh timer. Fetch tetap dibungkus `select!` terhadap cancel dari Fase 1, tanpa cabang kedua. Baris terlihat begitu batch-nya keluar dari `next_batch`.
- **Pemilihan target:** `preview` dan `explain` membaca `settings.text("RESULT_SINK", "")`.
  - `"store"` dengan `out.result_store()` berisi → `StoreTarget`;
  - `"store"` tanpa store → `CliError::Usage("RESULT_SINK=store needs a result store, which only the app's engine host attaches")`;
  - selain itu → `NdjsonTarget`.
- **Galat store:** `CliError::Store(#[from] StoreError)` memetakan `DiskFull`, `SpillUnavailable`, dan `SpillAuth` ke pesan untuk pengguna. `Released` diperlakukan sebagai cancel (tab sudah ditutup), sehingga `done` membawa `cancelled: true` dan tidak ada event `error`.

### 13.2 Event di mode store

`step connect`, `columns` (payload sama), `progress { rows }` (≤ 1 per 16 ms), lalu `done { rows, truncated, query_id, elapsed_ms, cancelled? }`, bentuknya sama dengan `done` NDJSON. Tidak ada event `rows`. `error` tetap satu event. Tabel event di doc modul `lib.rs` mendapat satu baris untuk mode ini.

### 13.3 Publikasi dan TTFR S2

Batch pertama (200 baris) langsung disegel dan dipublikasikan (§4.4). Tick `displayLink` berikutnya (≤ 8,3 ms pada 120 Hz) melihat 200 baris, lalu `noteNumberOfRowsChanged`, lalu satu `window` sinkron, lalu gambar. Tidak ada decode JSON di antaranya, sehingga target TTFR S2 p95 ≤ 50 ms tidak bergantung pada cap.

## 14. Sisi Swift

### 14.1 Batas FFI tetap di satu berkas

`Models/StoreRows.swift` mendefinisikan protokol Swift `ResultStoreHandle`, yang mencerminkan `ResultHandle` (`rowCount`, `window`, `rowsText`, `cellText`, `columnWidths`, `setView`, `distinctValues`, `release`) dengan tipe Swift sendiri. Kesesuaiannya, `extension ResultHandle: ResultStoreHandle`, ditulis di `Support/RustEngine.swift`. Aturan "RustEngine satu-satunya pengimpor FFI" tetap berlaku, dan `StoreRows` bisa diuji dengan handle palsu.

`DatabaseEngine` mendapat:

```swift
func makeResultStore() -> (any ResultStoreHandle)?
@discardableResult
func runIntoStore(_ command: String, env: [String: String], store: any ResultStoreHandle,
                  onEvent: @escaping (Event) -> Void,
                  onExit: @escaping (_ status: Int32, _ stderr: String) -> Void) -> (any EngineRun)?
func storeFromRows(columns: [Event.Column], rows: [[String?]]) -> (any ResultStoreHandle)?
```

`MockEngine` di target tes mengimplementasikannya dengan store palsu, dan invariant #2 tetap seperti adanya.

### 14.2 `StoreRows: ResultRows`

```swift
final class StoreRows: ResultRows {
    let handle: any ResultStoreHandle
    let columns: [Event.Column]
    private(set) var count: Int             // visible (view)
    private(set) var fetched: Int
    private(set) var phase: StorePhase
    private(set) var viewID: UInt64
    func poll() -> Bool                                          // per frame; true bila count berubah
    func cell(row: Int, column: Int) -> CellText                 // cache halaman; fetch sinkron saat miss
    func fullValue(row: Int, column: Int, format: ColumnFormat) -> String?   // cell_text (tooltip terformat, pembaca Raw)
    func rows(in range: Range<Int>, columns: [Int]) -> [[String?]]           // rows_text; off-main bila > 10k sel
    func apply(_ spec: ViewSpec) async throws -> ViewInfo                    // set_view di antrean .userInitiated
    func distinctValues(column: Int) async -> (values: [String?], more: Bool)
    func columnWidths() -> [Int]
    func release()
}
```

- **Halaman:** 128 baris × blok 32 kolom sumber (dalam urutan tampilan). Cache milik satu `StoreRows` (satu handle), dengan kunci `(viewID, pageRow, columnBlock, formatSignature)`. Paling banyak 12 halaman dan ≤ 8 MB; yang dibuang paling jauh dari viewport. Tab yang pindah ke latar membuang semua halamannya, supaya NFR-P3 (anggaran + 64 MB) tidak dimakan cache Swift dari banyak tab. `PAGE_ROWS` dan `COL_BLOCK` adalah konstanta yang disetel dari angka W5-T3.
- **`WindowPage`** memegang `Data` apa adanya dan mendekode sel secara lazy dengan `String(decoding: slice, as: UTF8.self)`. Cache tampilan per baris milik Fase 5 menyimpan `CellText` yang sudah jadi. Validasi header ada di §9.1.
- **Miss sinkron** terjadi di main (target p99 ≤ 0,5 ms). **Prefetch:** bila viewport masuk ke halaman terakhir yang di-cache searah scroll, halaman berikutnya diambil di antrean `.userInitiated` dan dipasang di main. Hasil yang basi (`viewID` permintaan berbeda dari view saat dipasang, atau `StoreRows` sudah dilepas) dibuang.
- **Format** dibaca sekali per kolom (aturan Fase 5) dan dipetakan `ColumnFormat → CellFormat`. Perubahan format meng-invalidasi halaman kolom itu.
- **Galat:** `StaleHandle` dan `Superseded` diabaikan tanpa pesan (`rust-engine-blueprint.md` §2.6). `StaleView` memuat ulang dengan `viewID` terkini. Galat lain muncul di banner grid.

### 14.3 Polling selama streaming (`Views/ResultGridTable.swift`)

`NSView.displayLink(target:selector:)` (macOS 14) dipasang di run loop main dengan mode `.common`, supaya tetap berdetak saat tracking scroll. Setiap tick:

1. `if rows.poll() { tableView.noteNumberOfRowsChanged() }`.
2. Hitungan footer diperbarui paling sering sekali per frame.
3. Lebar kolom diminta ulang saat `fetched` melewati 200.

Display link berhenti saat fase bukan `streaming` (setelah satu poll terakhir) dan saat view keluar dari jendela. Tab di latar tidak mem-poll. Ketika tab kembali ke depan, satu poll langsung dijalankan. Selama streaming, klik header sort dinonaktifkan dan chevron diredupkan (`performance-plan.md` §14 butir 2), baik untuk sort server maupun fallback.

### 14.4 `AppModel` dan `QueryTab`

- **`runPreview`:** `store = engine.makeResultStore()`, lalu `engine.runIntoStore("preview", …)`. Event `columns` memasang kolom dan membuat `StoreRows`. `progress` memperbarui hitungan footer bila grid tidak terlihat. `done` menyimpan `truncated`, `queryID`, dan `elapsedMS`. Galat atau exit bukan nol melepas store (UX hari ini: grid dikosongkan dan galat ditampilkan). Cancel mempertahankan baris.
- **`explain`:** jalurnya sama (D-8). Duplikasi loop penumpukan di `explain()` hilang.
- **`previewEnvironment`:** clamp `LIMIT` naik ke 5.000.000 (O-12), ditambah `SettingsView` dan `RowLimitSettingTests`.
- **`QueryTab`:** `displayedRows` dan cache-nya diganti `activeResult: StoreRows?` beserta view-nya. `columnFilters`, `gridSearch`, dan sort memory tetap menjadi sumber state di Swift. Setiap perubahan menyusun `ViewSpec` lalu memanggil `apply`. Aturan yang ada tetap: filter atau search menghapus seleksi, edit, dan undo (`QueryTab.swift:542-571`).

### 14.5 Yang dihapus di W6-T1

| Yang dihapus | Syarat |
|---|---|
| `AppModel.previewPaintInterval` dan penumpukan `rows` di `runPreview` | W5-T2 hijau |
| Penumpukan `rows` di `explain` | sama |
| `QueryTab.displayedCache`, `displayedCacheRevision`, `displayedRows` (array) | `StoreRows` menjadi sumber |
| Snapshot "hasil dasar ≤ 10.000 baris sebagai `[[String?]]`" dari W4-T1 | §15 berlaku |
| `GridSort.order`, `compare`, `value`, `number`, `isPlainNumber`, `isExponent` | `differential.rs` W4-T4 lulus, dan tes integrasi W6-T1 lulus |
| `ColumnFilter.matches`, `matchesText`, `distinctValues` | sama |
| `GridSearch.matches` (dan enum `GridSearch` bila kosong) | sama |
| Pemindaian `naturalWidths` (versi Fase 5) | diganti `columnWidths()` |
| `isOpenable` dan `ColumnFormat.render` di jalur gambar | diganti flag dan teks jendela; fungsinya tetap untuk sel staged dan pembaca (D-9) |
| `ArrayRows` untuk preview dan explain | tetap ada hanya sebagai kembaran tes dan tampilan tanpa view; dihapus seluruhnya bila W6-A1 menemukan pemakainya habis |

Yang tetap: `GridSort.Direction` dan `GridSort.next` (siklus klik), enum `ColumnFilter` beserta `isEmpty`, `label`, `nullToken`, dan `valuePickerLimit` (spec dan UI), `SearchStatement`, `ServerSort`, `GridValue` (sel staged dan pembaca), serta `ColumnFormat.render` (sel staged).

Tes Swift yang memanggil fungsi yang dihapus (`ResultGridTests.swift:53-83` untuk `GridSort.number` dan `value`; `GridColumnsTests.swift:253-257` untuk `GridSearch.matches`) ditulis ulang terhadap `StoreRows` yang dibangun lewat `storeFromRows`. Hitungan tes tidak turun (NFR-Q).

### 14.6 Startup dan direktori spill

`App.swift`, saat peluncuran, **secara sinkron dan sebelum sesi dipulihkan** (koreksi AR), memanggil `host.configureResultStores(spillDir:budgetBytes:)` dengan `spill_dir = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask)[0]/QueryHive/spill` dan anggaran 256 MiB. Ongkosnya satu `mkdir`/`chmod` dan satu `read_dir` atas direktori yang normalnya kosong, jadi di bawah 1 ms dan dicatat oleh skenario `launch`. Urutan ini menjamin sapuan (§7.7) selesai sebelum Run pertama bisa membuat store, tanpa balapan dengan Run yang dipulihkan sesi (§10.3). `--bench` dan `--snapshot` mengonfigurasi anggarannya sendiri. Diagnostik membaca `store_stats()`.

## 15. Batch 7 "off": dua store per tab

`QueryTab` memegang `baseResult: StoreRows?` (hasil Run, dalam urutan server) dan `activeResult: StoreRows?` (yang ditampilkan grid). Invarian: paling banyak dua store per tab, dan `activeResult === baseResult` kecuali sort atau search server sedang aktif.

| Aksi | Akibat |
|---|---|
| Run atau Explain | Lepas base dan active, buat store baru. `base = active = baru`. |
| Sort atau search server (`activeSort.origin == .server`) | Buat store baru S. Bila `active !== base`, lepas active. `active = S`. Base tetap utuh untuk "off". |
| Sort in-memory (fallback O-8: `ServerSort` menolak, atau grid menampilkan plan) | `active.apply(ViewSpec(sort: …))`. Tanpa store baru. |
| Filter funnel dan search in-memory | `active.apply(…)` (FR-GRID-04, P-26). |
| "Off" (klik ketiga, atau search dikosongkan) | Bila `active !== base`: lepas active, lalu `active = base` dengan view identitas (`apply(ViewSpec())`, tanpa query). Bila `active === base` dengan view sort: `apply` tanpa sort. Hanya bila base sudah dilepas (tab dipulihkan dari sesi, atau galat), SQL dasar (`previewBaseSQL`) dijalankan ulang. |
| Tab ditutup | Lepas keduanya (§16). |

Anggaran global membuat base yang menganggur tumpah lebih dulu (§7.5), jadi menyimpan dua store tidak menggandakan memori resident. Penjaga edit Fase 3 tetap berlaku: sort atau search server ditolak bila `cellEdits` tidak kosong.

## 16. Umur handle dan penutupan tab

1. `closeTab` memanggil `process?.terminate()` (flag cancel) **lalu** `tab.releaseResults()`, yang memanggil `release()` pada base dan active. Keduanya segera.
2. Rust membebaskan chunk dan anggaran, menutup fd spill, menandai store `Released`, dan membatalkan view yang sedang berjalan (§7.6).
3. Run yang masih menulis mendapat `Released`, lalu berakhir sebagai cancel. Event-nya sampai di `RustRun` yang sudah berhenti dan dibuang (`RustEngine.swift`, `Sink.onEvent`).
4. Prefetch atau `apply` yang masih terbang mendapat `StaleHandle`/`Superseded` dan diabaikan.
5. Objek UniFFI yang akhirnya di-dealloc Swift, dari thread mana pun, menjalankan `Drop`, yang memanggil `release` lagi secara idempoten.
6. Saat app keluar (`terminateAll`), fd ditutup kernel. Berkas sudah di-unlink, jadi tidak ada yang tersisa.

G-LEAK (`--bench tabs-100`, 100× buka dan tutup tab dengan store dari `store_from_rows`, 2.000 × 10 seperti hari ini, dan anggaran kecil supaya spill benar-benar terjadi) harus menghasilkan `leaks` nol, direktori spill kosong, dan `store_stats().stores == 0` serta `spilled_bytes == 0` di akhir. Dua syarat terakhir yang bermakna: store yang masih tercatat di registry masih terjangkau, jadi `leaks` tidak melihatnya, dan direktori selalu kosong karena D-5. `development-plan.md` §1 G-LEAK perlu menambahkan pemeriksaan `store_stats` ini (lihat verdict).

## 17. Jalur yang tetap NDJSON

| Jalur | Setelah Fase 6 |
|---|---|
| CLI `queryhive-engine` (semua perintah, termasuk `preview` dan `explain`) | NDJSON `rows`, tidak berubah. `RESULT_SINK=store` dari CLI menjadi galat usage. |
| Korpus golden (`crates/qh-ffi/tests/golden.rs`, `tools/golden/live_cases.py`) | tidak berubah. Tidak ada yang memasang `RESULT_SINK`. |
| MCP (`queryhive-mcp`) | tidak berubah. Proses terpisah tanpa `EngineHost`. |
| `export`, `to_table`, `import_data`, `apply_changes`, `table_op` | tidak berubah. PR-01: ekspor tidak memakai store. |
| `count`, `objects`, `catalogs`/`schemas`/`tables`, `test`, perintah lokal | tidak berubah |
| `host.run(…)` di app untuk `preview`/`explain` tanpa store | NDJSON; dipakai tes, dan tetap berfungsi sebagai jalur lama |
| App: `preview` `LIMIT 1` yang hanya membaca `columns` (detail objek `AppModel.swift:545`, probe kolom `:2107`) | NDJSON lewat `host.run`, tidak berubah. Keduanya tidak pernah menggambar baris, jadi tidak butuh store. |
| App: Run, sort server, search server, Explain | store (`run_with_store`) |

Tes kesetaraan di W5-T2 menjalankan kasus golden fake-cursor lewat kedua target dan memastikan `rows_text` store sama persis dengan array `data` NDJSON, sel demi sel. Itu bukti bahwa renderer tetap satu.

## 18. Perubahan per tugas

### 18.1 W4-T3: inti store (Rust murni)

**Dibuat:**

| Berkas | Isi | Prioritas |
|---|---|---|
| `crates/qh-result-store/src/registry.rs` | `StoreRegistry`, `StoreId`, anggaran, LRU evict, cache dekripsi, sapuan, `from_text_rows`, `synthetic`, `stats` (§7) | P0 |
| `crates/qh-result-store/src/spill.rs` | `SpillCipher` (kunci, nonce, AAD), `SpillFile` (0600, unlink), `SpillMedium`, `FaultyMedium` (tes) (§8) | P0 |
| `crates/qh-result-store/src/view.rs` | `ViewSpec`, `View`, pipeline, kunci sort, filter, search, perluasan saat streaming, `distinct_values`, cancel dan supersede (§11) | P0 |
| `crates/qh-result-store/src/collate.rs` | kunci natural dan prefiks, `fold`, `ci_contains`, `ci_equal`, `swift_plain_number`, `swift_double` (termasuk float heksadesimal), `NumKey` | P0 |
| `crates/qh-result-store/src/render.rs` | writer buffer jendela (§9.1), potongan 256 UTF-16, port `ColumnFormat`, validator JSON, openable, `head_widths` | P0 |
| `crates/qh-result-store/tests/spill.rs` | tes NFR-S3 (§8.5) | P0 |
| `crates/qh-result-store/tests/window.rs` | layout byte demi byte, clamp, `StaleView`, potongan (ASCII, CJK, emoji ZWJ, zalgo), kesamaan teks jendela dengan `to_text` untuk setiap encoding | P0 |
| `crates/qh-result-store/tests/view.rs` | semantik sort (NULL, numerik, seri stabil, desc, `"1.0"`/`"1"` tetap dalam urutan baris sumber), filter, search, streaming, totalitas comparator berbenih, `Streaming`, `Superseded`, `TooLarge` | P0 |

**Diubah:**

| Berkas | Perubahan | Prioritas |
|---|---|---|
| `crates/qh-result-store/src/codec.rs` | Layout chunk (§4) dengan `ChunkBuilder` dan `ChunkView`. `encode_value`/`decode_value` bertag dipertahankan. Perbaikan `Unknown` `raw` (fakta 2) beserta tesnya. Tes lama dipindah ke layout baru. | P0 |
| `crates/qh-result-store/src/store.rs` | `StoreShared`, `StoreHandle`, `StoreWriter`, indeks dan publikasi (§7.3), aturan segel, residency. `window(range)` lama menjadi `values(range) -> Vec<Vec<Value>>` untuk tes dan diagnostik, sehingga tes lama tetap bermakna. Klaim palsu soal "startup sweep in `qh-storage`" dihapus. | P0 |
| `crates/qh-result-store/src/lib.rs` | Doc modul ditulis ulang (layout, anggaran, spill terenkripsi, view). Ekspor baru. `forbid(unsafe_code)` tetap. | P0 |
| `crates/qh-result-store/Cargo.toml` | `qh-rt`, `rayon`, `ring = "0.17"`, `unicode-segmentation`, `unicode-normalization = "0.1"`, `unicode-properties = "0.1"`, `zeroize`, `serde_json` (untuk printer format JSON), dev `serde_json` | P0 |
| `crates/qh-rt/src/lib.rs`, `crates/qh-rt/Cargo.toml` | `view_pool()` rayon ber-QoS (§11.1) dan tes QoS worker-nya, mengikuti pola tes `the_main_runtime_puts_its_workers_at_user_initiated` | P0 |
| `Cargo.toml` (root) | `rayon = "1"`, `unicode-segmentation = "1"` di `[workspace.dependencies]` | P0 |
| `Cargo.lock` | `rayon`, `rayon-core`, `crossbeam-deque`, `crossbeam-epoch`, `either`, `unicode-segmentation` | P0 |

Verifikasi: G-RUST, G-DENY, tes spill §8.5, tes view dan window, ditambah `cargo test -p qh-result-store -- --ignored bench_*` untuk angka lokal (dicatat, bukan gate).

### 18.2 W4-T4: fixture diferensial

**Dibuat:**

| Berkas | Isi | Prioritas |
|---|---|---|
| `app/Tests/QueryHiveTests/SortFixtureExport.swift` | eksportir bergerbang `QH_EXPORT_FIXTURES=1` dan pemeriksa fixture ter-commit (§12.1) | P0 |
| `crates/qh-result-store/tests/fixtures/grid/*.json` | sembilan berkas §12.2 | P0 |
| `crates/qh-result-store/tests/differential.rs` | pemeriksa dan `KNOWN_DIVERGENCES` (§12.3) | P0 |

Verifikasi: `QH_EXPORT_FIXTURES=1 swift test --filter SortFixtureExport` (regenerasi), `swift test --filter SortFixtureExport` (pemeriksa), `cargo test -p qh-result-store --test differential`. Kegagalan paritas diperbaiki di Rust (W4-T3 dibuka lagi), atau didaftar dengan alasan bila termasuk kolasi (O-9).

### 18.3 W5-T2: sink engine dan `ResultHandle`

**Dibuat:**

| Berkas | Isi | Prioritas |
|---|---|---|
| `crates/qh-ffi/src/store_api.rs` | rekaman, enum, `StoreFfiError`, `ResultHandle`, `guarded()`, `StoreEmitter`, pemetaan `StoreError → StoreFfiError` (§10) | P0 |
| `crates/qh-ffi/tests/store_sink.rs` | event mode store (tanpa `rows`); kesetaraan `rows_text` dengan NDJSON atas kasus golden fake; `RESULT_SINK=store` tanpa store menjadi galat usage; disk penuh menjadi event `error`; release saat streaming menjadi `cancelled`; `progress` ≤ 1 per 16 ms; satu handle dipakai dua `run_with_store` menjadi galat usage di run kedua (`begin` hanya sekali) | P0 |
| `app/Tests/QueryHiveTests/ResultHandleSmokeTests.swift` | `store_from_rows`, decode `window`, `cell_text`, `set_view`, `release`, lalu `StaleHandle` tertangkap sebagai galat Swift; `InvalidArgument` untuk kolom di luar rentang | P0 |

**Diubah:**

| Berkas | Perubahan | Prioritas |
|---|---|---|
| `crates/qh-ffi/src/commands.rs` | `RowTarget`, `NdjsonTarget`, `StoreTarget`, `pump_result` menggantikan `emit_batches`; pemilihan `RESULT_SINK` di `preview`/`explain`/`stream_rows`; fetch adaptif; tenggat segel (§13) | P0 |
| `crates/qh-ffi/src/events.rs` | `Emitter::result_store(&self) -> Option<StoreWriter> { None }` | P0 |
| `crates/qh-ffi/src/host.rs` | registry di `EngineHost` dan enam metode §10.3 | P0 |
| `crates/qh-ffi/src/lib.rs` | `pub mod store_api;`, `CliError::Store`, tabel event di doc modul | P0 |
| `crates/qh-ffi/src/uniffi_api.rs` | doc modul: data plane kini ada di `store_api.rs` (ADR-0004 → 0030) | P1 |
| `crates/qh-ffi/Cargo.toml` | `qh-result-store.workspace = true` | P0 |
| `Cargo.lock` | tepi dependensi `qh-ffi → qh-result-store` (tanpa crate baru). Pemilik `Cargo.lock` di `development-plan.md` §7 adalah lajur W4-T3 → W7-T2 → W7-T5, jadi W5-T2 harus disisipkan (R-15). | P0 |
| `crates/qh-ffi/examples/bench_ffi.rs` | skenario `window` (1M × 30 sintetis lewat `EngineHost::store_synthetic` Rust-only, jendela acak 128 × 32, p50/p99), `window-json` (dicatat, bukan gate, §9.5), dan `view-*` | P0 |
| `app/Generated/*` | regenerasi (invariant #1) | P0 |

Verifikasi: G-RUST, G-FFI, G-SWIFT, G-GOLDEN (tanpa selisih), `cargo run --release -p qh-ffi --example bench_ffi -- window`.

### 18.4 W6-T1: integrasi Swift

**Dibuat:**

| Berkas | Isi | Prioritas |
|---|---|---|
| `app/Sources/QueryHive/Models/StoreRows.swift` | `ResultStoreHandle`, `StoreRows`, `WindowPage`, pemetaan `ViewSpec`/`CellFormat` (§14.2) | P0 |
| `app/Tests/QueryHiveTests/StoreRowsTests.swift` | decode halaman, cache dan eviksi, prefetch, poll, `StaleView`, release | P0 |
| `app/Tests/QueryHiveTests/Bench/StoreWindowBench.swift` | `QH_BENCH=1`: p99 `window` lewat UniFFI plus decode | P1 |

**Diubah:**

| Berkas | Perubahan | Prioritas |
|---|---|---|
| `Support/RustEngine.swift` | `makeResultStore`, `runIntoStore`, `storeFromRows`, `extension ResultHandle: ResultStoreHandle` | P0 |
| `Support/DatabaseEngine.swift` | tiga persyaratan protokol (§14.1) | P0 |
| `Models/AppModel.swift` | `runPreview`/`explain` lewat store, penghapusan §14.5, "off" §15, `closeTab` §16, clamp 5.000.000 | P0 |
| `Models/QueryTab.swift` | `baseResult`/`activeResult`, `releaseResults`, `displayedRows` diganti view, penghapusan `ColumnFilter.matches*`/`distinctValues` | P0 |
| `Models/GridSort.swift`, `Models/GridSearch.swift` | penghapusan §14.5 | P0 |
| `Models/WritePlan.swift` | accessor baris lewat `rows(in:)` untuk baris yang diedit saja | P0 |
| `Views/ResultGridTable.swift` | polling `displayLink`, prefetch saat scroll, header dinonaktifkan selama streaming | P0 |
| `Views/ResultGrid.swift` | picker filter lewat `distinctValues` async, footer dan placeholder dari hitungan store | P0 |
| `Views/CellValueViewer.swift` | nilai penuh dibaca lazy (`fullValue(…, .raw)`) saat dibuka | P1 |
| `App.swift` | `configureResultStores` saat peluncuran (sapuan), `spill_dir` | P0 |
| `Views/SettingsView.swift`, `Tests/QueryHiveTests/RowLimitSettingTests.swift` | plafon 5.000.000 | P0 |
| `Support/Snapshot.swift` | scene yang men-sort atau memfilter memakai `storeFromRows` | P0 |
| `Support/BenchMode.swift` | `tabs-100`, `scroll-30x1m`, `scroll-500x10k`, `open-500x10k` (nama skenario yang ada hari ini) lewat `store_from_rows`, dibangun di luar interval yang diukur | P0 |
| `Tests/QueryHiveTests/{ResultGridTests,GridColumnsTests,Batch7Tests,TabCloseTests,MockEngine}.swift` | ditulis ulang terhadap `StoreRows`; tes "off" dengan dua store; release saat tutup tab; konformitas mock | P0 |

Verifikasi: G-SWIFT, G-VIS (terhadap baseline Fase 5, tanpa rekam ulang), tes diferensial W4-T4, G-LEAK, dan G-BENCH(1 S2, 2, 3) ditambah `window`.

## 19. Urutan build

1. **W4-T3a, tipe dan codec:** `codec.rs` (layout chunk, perbaikan `Unknown`), `store.rs` (writer, indeks, publikasi), tes codec dan store yang dipindah. Gate: G-RUST.
2. **W4-T3b, registry dan spill:** `registry.rs`, `spill.rs`, tes NFR-S3. Gate: G-RUST, G-DENY, SEC.
3. **W4-T3c, render dan view:** `collate.rs`, `render.rs`, `view.rs`, `qh-rt::view_pool`, tes window dan view. Gate: G-RUST.
4. **W4-T4, fixture:** ekspor dari Swift (implementasi Swift masih ada), pemeriksa Rust, lalu divergensi didaftar. W4-T3 dibuka lagi bila ada ketidakcocokan yang bukan kolasi.
5. **W5-T2, integrasi engine:** `events.rs` hook, lalu `commands.rs` (`RowTarget`, dengan golden hijau **sebelum** `StoreTarget` ditambahkan), lalu `store_api.rs` dan `host.rs`, regenerasi `app/Generated`, smoke test Swift, lalu `bench_ffi window`.
6. **W6-A1:** blueprint disegarkan terhadap seam Fase 5 yang sudah ada (nama `ResultRows`, `CellText`, `ArrayRows`, cache Fase 5).
7. **W6-T1, UI:** `StoreRows` dan tesnya, lalu jalur Run dan Explain, lalu polling, lalu "off", lalu tutup tab. **Terakhir** penghapusan kode Swift §14.5, setelah semua tes di atas hijau.
8. **Dokumen (W4-D, W6-D):** ADR-0037 (spill), ADR-0030 (data plane, termasuk D-8 dan D-5), ADR-0034 (view dan daftar divergensi).

## 20. Risiko

| # | Risiko | Mitigasi |
|---|---|---|
| R-1 | Kunci natural berbeda dari `localizedStandardCompare` | O-9 menerimanya. Fixture dan `KNOWN_DIVERGENCES` membuatnya eksplisit, lalu masuk ADR-0034. |
| R-2 | Comparator yang tidak total membuat sort Rust panic (≥ 1.81) | Desain leksikografis dengan pemutus byte dan baris, tes totalitas berbenih, dan `guarded()` sebagai jaring terakhir |
| R-3 | Miss sinkron atas chunk yang tumpah (dekripsi ≤ 2 MiB di main) menyebabkan hitch | Batas 2 MiB per chunk, prefetch searah scroll, cache dekripsi 8 chunk, dan angka W5-T3. Bila gagal, halaman berikutnya dimuat async dan sel sementara kosong (keputusan W6-A1). |
| R-4 | Akuntansi memori meleset (chunk yang disemat, builder terbuka) | Chunk dengan `strong_count > 1` tidak di-evict. Builder ≤ 2 MiB per store aktif dinyatakan di doc. Tes "resident ≤ anggaran + satu chunk" dan G-BENCH(3). |
| R-5 | Ongkos UniFFI per jendela > 0,5 ms | Halaman 128 × 32, satu `Data` per panggilan. Bila p99 masih > 0,5 ms, eskalasi C ABI Fase 8 (W8-T2). |
| R-6 | Balapan antara publikasi chunk dan perluasan view saat streaming | `Release`/`Acquire` pada `rows`, indeks append-only, satu tugas perluasan dalam satu waktu, tes penulis konkuren |
| R-7 | Handle dilepas saat `set_view`, prefetch, atau ingest masih berjalan | Generation dan fase atomik, cancel view saat release, `Released` → cancel di pump, galat basi diabaikan Swift. Tes di `view.rs` dan `store_sink.rs`. |
| R-8 | Berkas yang di-unlink tidak terlihat di `du` atau Finder | `store_stats()` di diagnostik, dan pesan disk penuh yang eksplisit |
| R-9 | Stream yang menetes menghasilkan banyak chunk kecil | Dibiarkan di Fase 6. Kompaksi ditinjau bila jumlah chunk > 4.096 terukur. |
| R-10 | Kripto salah pakai (nonce, AAD, kunci) | Penghitung global per kunci, AAD dengan `store_id` dan indeks, tes §8.5, dan `security-reviewer` di W4-T3 |
| R-11 | Hitungan grapheme berbeda versi Unicode antara `unicode-segmentation` dan Swift | `width.json`. Selisih pada rangkaian langka hanya menggeser lebar kolom, dan batasnya 320 pt. |
| R-12 | Paritas validator JSON dan printer format `Json` | Fixture dua sisi. Pintu darurat `Json` (§9.4) diputuskan di W4-T4. |
| R-13 | Explain lewat store menyimpang dari `performance-plan.md` §10 butir 5 | Dicatat di D-8 dan ADR-0030. Golden `explain` dijaga G-GOLDEN. |
| R-14 | Blueprint Fase 2 dan Fase 5 belum ada; nama `EngineHost`, `ResultRows`, `CellText` bisa berbeda | W6-A1 menyegarkan §10.3 dan §14 terhadap kode nyata. Inti W4-T3 tidak bergantung pada keduanya. |
| R-15 | Daftar berkas di `development-plan.md` kurang di W4-T3, W5-T2, dan W6-T1, termasuk `Cargo.lock` di W5-T2 dan dua rantai kepemilikan (`ResultGrid.swift`, `Snapshot.swift`) yang tidak memuat W6-T1 | Orkestrator memperbarui §5 dan §7 sebelum tugas dimulai. Daftar lengkapnya di §21 dan di verdict. |
| R-19 | Satu fd per store yang tumpah; banyak tab di latar mendekati batas lunak 256 fd app GUI | `EMFILE` menjadi galat seperti disk penuh, chunk tetap resident, store lain utuh (§8.4). Bila terukur di G-LEAK atau bench, W6-A1 memutuskan antara menaikkan `RLIMIT_NOFILE` saat peluncuran atau menutup fd store yang menganggur. |
| R-16 | `Decimal` Swift 38 digit berbeda dengan desimal eksak | `NumKey` meniru 38 digit, dan aturan pembulatannya dikunci `number.json` |
| R-17 | Rayon view bersaing CPU dengan ingest di P-core | View hanya fallback dan jarang. Pool terpisah dari runtime tokio. Diukur di sesi bench W6-T2. |
| R-18 | Disk penuh di tengah streaming | Event `error`, store lain utuh, tanpa crash (tes W4-T3 dan W5-T2) |

## 21. Yang disegarkan di W6-A1, dan catatan untuk orkestrator

- **W6-A1** mencocokkan §10.3 dan §14 dengan `host.rs` (W3-T1) serta `ResultRows`/`CellText`/`ArrayRows` (W5-T1) yang sudah mendarat. W6-A1 juga memutuskan nasib `ArrayRows` (tetap sebagai kembaran tes, atau dihapus), menetapkan `PAGE_ROWS` dan `COL_BLOCK` dari angka W5-T3, dan memastikan pintu darurat `Json` (§9.4) dari hasil W4-T4.
- **Kepemilikan berkas** (`development-plan.md` §5 dan §7):
  - W4-T3: tambahkan `crates/qh-result-store/src/{registry.rs,collate.rs,render.rs}`, `crates/qh-result-store/tests/{spill.rs,window.rs,view.rs}`, dan `crates/qh-rt/Cargo.toml`.
  - W5-T2: tambahkan `crates/qh-ffi/src/events.rs`, `crates/qh-ffi/Cargo.toml`, `crates/qh-ffi/examples/bench_ffi.rs`, `crates/qh-ffi/tests/store_sink.rs`, dan `Cargo.lock`. Rantai `Cargo.toml`, `Cargo.lock` menjadi W4-T3 → W5-T2 (hanya `Cargo.lock`) → W7-T2 → W7-T5.
  - W6-T1: tambahkan `Support/DatabaseEngine.swift`, `Views/ResultGrid.swift`, `Support/Snapshot.swift`, `Support/BenchMode.swift`, `Views/SettingsView.swift`, serta tes `StoreRowsTests.swift`, `Bench/StoreWindowBench.swift`, `ResultGridTests.swift`, `GridColumnsTests.swift`, `Batch7Tests.swift`, `TabCloseTests.swift`, `MockEngine.swift`, dan `RowLimitSettingTests.swift`. Rantai `Views/ResultGrid.swift` menjadi … → W5-T1 → W6-T1 → W9-T5 → …, dan rantai `Support/Snapshot.swift` menjadi W1-T3 → W5-T1 → W6-T1 → W9-T8.
  - G-LEAK di `development-plan.md` §1: tambahkan `store_stats().stores == 0` dan `spilled_bytes == 0` (§16), karena direktori spill selalu kosong di bawah D-5.
- **ADR:**
  - 0037 (W4-D): D-5 dan §8 apa adanya.
  - 0030 (W6-D): D-1, D-6, D-8, D-11, D-12, D-13, amandemen 0004 dan 0008, penggantian 0013, dan amandemen D-1 blueprint Fase 2 (metode store yang melempar galat, §10.3).
  - 0034 (W6-D): §11 dan daftar divergensi akhir.
- **Keputusan yang diserahkan ke pemilik lewat laporan akhir:** D-8 (Explain lewat store) dan D-5 (unlink segera dan hilangnya path spill dari diagnostik). Keduanya bisa dibalik tanpa mengubah layout atau API inti.

## Verdict architect-reviewer

**Verdict: disetujui dengan koreksi** (W2-A3, 30 Sep 2026). Koreksi di bawah sudah diterapkan di dokumen ini. Blueprint siap untuk W4-T3 setelah orkestrator memperbarui `development-plan.md` sesuai daftar R-15. Tinjauan ulang AR di W6-A1 tetap berlaku untuk §10.3 dan §14.

### Yang diperiksa

- Rencana dan PRD: `performance-plan.md` §7, §9, §10, §13; FR-PERF-05, FR-GRID-03/04, NFR-P1 S2, NFR-P2, P3, P8, NFR-S3, NFR-C, O-8, O-9, O-12; `docs/invariants.md` #1 dan #11; `fase-2-engine-host.md` §2 dan §3.
- Klaim kode, dicek langsung:
  - `codec.rs:317-337` memang men-decode `Unknown` bentuk 2 (`raw`) menjadi `text` lossy dengan `raw: None`, sehingga `to_text` memberi teks, bukan hex (`qh-core/src/render.rs:93-97`). Ini perubahan data senyap yang nyata, dan perbaikannya di W4-T3 wajib.
  - Sapuan spill memang tidak ada: `qh-storage` hanya menyebut spill di komentar `lib.rs:16`, sementara `SpillFile::drop` (`store.rs:105-111`) mengandalkannya. Spill hari ini teks biasa dengan `create(true).truncate(true)` dan penghitung per store, jadi dua store dalam satu proses bisa menimpa berkas yang sama. Crate ini belum dipakai siapa pun, jadi cacat itu laten.
  - `ring` 0.17.14 ada di `Cargo.lock:2903`. `aead` dan `rand` modul publik tanpa gerbang fitur, dan `LessSafeKey::seal_in_place_separate_tag`, `open_in_place`, serta `Nonce::assume_unique_for_key` ada di baris yang disebut. ISC dan Apache-2.0 ada di allow-list `deny.toml`.
  - `explain` dan `preview` sama-sama lewat `emit_batches` (`commands.rs:1433`, `:1482`). `rayon`, `unicode-segmentation`, dan kawan-kawannya belum ada di `Cargo.lock`. `unicode-normalization`, `unicode-properties`, `zeroize`, dan `crossbeam-utils` sudah ada.

### Keputusan atas penyimpangan

- **D-8: Explain lewat store. Dipertahankan.** Plan digambar di grid yang sama dan hari ini ikut filter, search, dan sort `displayedRows`. Mempertahankan `ArrayRows` untuk Explain berarti mempertahankan `GridSort`, `ColumnFilter`, dan `GridSearch` di Swift, yaitu dua implementasi dengan kolasi berbeda untuk satu grid. Itu bertentangan dengan butir "implementasi Swift dihapus" di rencana yang sama. Ongkos engine nol. Dua `preview` yang hanya membaca `columns` (`AppModel.swift:545`, `:2107`) tetap NDJSON dan kini tercatat di §17. Penyimpangan dari `performance-plan.md` §10 butir 5 masuk ADR-0030.
- **D-5: unlink segera. Sehat di macOS.** fd yang path-nya sudah di-unlink tetap bisa `pread`/`pwrite`/`fstat` di APFS dan HFS+, dan blok bebas saat fd terakhir ditutup, termasuk saat crash. macOS tidak punya `O_TMPFILE`, jadi `create_new` lalu unlink adalah cara standarnya. Dua syarat ditambahkan: unlink sebelum byte pertama ditulis, dan unlink yang gagal berarti tidak ada spill. Akibat sampingnya, pemeriksaan direktori di G-LEAK menjadi hampa, jadi `store_stats` menjadi bukti pelepasan. Risiko fd (R-19) ditambahkan.
- **D-13: empat dari lima dipertahankan.** `distinct_values` (PR-12), `rows_text` (copy, `WritePlan`, drag), `store_from_rows` (scene `--snapshot` dan tes), dan `store_stats` (G-LEAK di bawah D-5) punya pemakai yang ada. `store_synthetic` hanya melayani bench, jadi tidak diekspor lewat UniFFI. Ia tinggal sebagai konstruktor Rust untuk `bench_ffi`, dan `--bench` Swift memakai `store_from_rows` di luar interval ukur.
- **Nonce dan umur kunci: benar dan minimal.** Satu kunci per registry, satu `AtomicU64` di objek yang sama, nonce `[0; 4] ‖ counter` big-endian: tidak ada nonce yang terulang di bawah satu kunci, di store mana pun. Setiap segel mengambil nonce baru, dan chunk yang sudah tumpah tidak pernah disegel ulang. NFR-S3 menulis "penghitung per chunk"; penghitung global per segel memenuhi maksudnya dan lebih kuat. Dua koreksi kecil: AAD memakai `store_id` u64 yang tidak pernah dipakai ulang sebagai pengganti `(slot, generation)`, dan dokumen tidak lagi menyiratkan bahwa jadwal kunci di dalam `LessSafeKey` di-zeroize.
- **Aturan segel: disederhanakan.** Satu push, satu segel, dipecah hanya di 65.536 baris atau 2 MiB. Timer 16 ms, `seal_if_due`, `publish_deadline`, dan cabang tenggat di `select!` dihapus. Timer itu tidak bisa menolong baris yang masih di dalam `next_batch` driver, dan sekitar 310 chunk untuk 5 juta baris jauh di bawah ambang R-9. TTFR S2 tetap dilayani, karena batch pertama langsung terbit.
- **Layout `window()`: dipangkas, bentuk dasarnya tetap.** Satu buffer dengan offset `u32` dan satu salinan ke `Data` sudah minimum untuk NFR-P8. Gema `slot`, `generation`, `view_id`, dan daftar kolom dihapus, sehingga header turun dari 48 ke 32 byte, tanpa medan yang harus divalidasi tapi tidak menangkap apa pun. Target 0,5 ms dibatasi ke format selain `Json`. `Json` mem-parse sampai 100.000 unit per sel, jadi ia diukur terpisah dan dicatat.

### Koreksi lain yang diterapkan

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

### Kepatuhan

- NFR-C dan invariant #11: tidak ada perintah baru. `RESULT_SINK=store` hanya bisa dicapai lewat `Emitter::result_store()` milik host, jadi CLI, MCP, golden, dan ekspor tetap NDJSON. Invariant #1 dijaga W5-T2.
- §13 rencana: tidak ada Arrow, mmap, atau DataFusion. `forbid(unsafe_code)` tetap di crate store.
- NFR-S3: AES-256-GCM `ring`, kunci acak per proses di memori saja, nonce tidak berulang, AAD berisi store id (yang juga generation) dan indeks chunk, direktori `0700`, berkas `0600` lewat `create_new`, sapuan tanpa mengikuti symlink, dan tes §8.5. Teks biasa tidak pernah menyentuh disk, karena berkas hanya berisi ciphertext dan tag, nonce ada di memori, dan unlink terjadi sebelum tulisan pertama. Swap macOS terenkripsi oleh sistem.
- Perubahan tampilan yang disengaja terbatas pada yang sudah tercatat (`performance-plan.md` §14 butir 1, 2, 4) ditambah divergensi fixture yang wajib didaftar. Perbaikan `Unknown` `raw` mengubah teks sel menjadi hex yang benar. Itu perbaikan, dan tercatat di fakta 2.

### Daftar R-15 untuk orkestrator (`development-plan.md`)

- **W4-T3** (§5): tambahkan `crates/qh-result-store/src/registry.rs`, `src/collate.rs`, `src/render.rs`, `tests/spill.rs`, `tests/window.rs`, `tests/view.rs`, dan `crates/qh-rt/Cargo.toml`.
- **W5-T2** (§5): tambahkan `crates/qh-ffi/src/events.rs`, `crates/qh-ffi/Cargo.toml`, `crates/qh-ffi/examples/bench_ffi.rs`, `crates/qh-ffi/tests/store_sink.rs`, dan `Cargo.lock`.
- **W6-T1** (§5): tambahkan `Support/DatabaseEngine.swift`, `Views/ResultGrid.swift`, `Support/Snapshot.swift`, `Support/BenchMode.swift`, `Views/SettingsView.swift`, `Tests/QueryHiveTests/StoreRowsTests.swift`, `Tests/QueryHiveTests/Bench/StoreWindowBench.swift`, `ResultGridTests.swift`, `GridColumnsTests.swift`, `Batch7Tests.swift`, `TabCloseTests.swift`, `MockEngine.swift`, dan `RowLimitSettingTests.swift`.
- **Rantai kepemilikan** (§7):
  - `Cargo.toml`, `Cargo.lock`: W4-T3 → W5-T2 (hanya `Cargo.lock`, tepi `qh-ffi → qh-result-store`) → W7-T2 → W7-T5.
  - `Views/ResultGrid.swift`: sisipkan W6-T1 di antara W5-T1 dan W9-T5.
  - `Support/Snapshot.swift`: W1-T3 → W5-T1 → W6-T1 → W9-T8.
  - `Support/BenchMode.swift`: belum punya rantai. Tambahkan dengan W6-T1 sebagai pemilik Fase 6.
- **G-LEAK** (§1): tambahkan pemeriksaan `store_stats().stores == 0` dan `spilled_bytes == 0` di akhir `tabs-100`.

### Yang tetap terbuka untuk W6-A1

Bentuk pintu darurat `Json` di permintaan jendela, nasib `ArrayRows`, `PAGE_ROWS`/`COL_BLOCK` dari angka W5-T3, dan mitigasi R-19 bila batas fd terukur.
