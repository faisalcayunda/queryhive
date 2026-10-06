# 0034 — View grid in-memory di Rust: kolasi natural, daftar divergensi, dan DataFusion hanya di helper opsional

- **Status:** Diterima. Sisi Rust sudah mendarat (`crates/qh-result-store`: `collate.rs`, `view.rs`, W4-T3 `e595877`;
  fixture diferensial W4-T4 `b1a1838`; `set_view` dan `distinct_values` lewat UniFFI, W5-T2 `bfb9680`). **Pemakaian dari
  Swift (`StoreRows.apply`, penjaga edit D-27) adalah W6-T1 dan belum dibangun**; bagian itu ditulis sebagai
  **direncanakan**. Fixture diferensial hanya mencakup sort, filter, dan search; paritas format, flag, dan lebar belum
  punya fixture (bagian "Belum ada"). **Kunci natural yang dibangun lebih sederhana daripada blueprint §13.4**: lipatan
  huruf hanya ASCII, satu `Vec<u8>` per baris tanpa arena, tanpa jalur prefiks, tanpa `TooLarge`, dan tanpa tes berbenih;
  memorinya tidak ditagih ke anggaran 256 MiB (butir 5, NEG-002, dan "Belum ada").
- **Tanggal:** 6 Okt 2026 (W6-D, perf-parity Fase 6)
- **Konteks instruksi:** `docs/architecture/development-plan.md` §4 (jadwal ADR, baris 0034) dan W6-D;
  `docs/architecture/blueprints/fase-6-data-plane.md` D-8, D-18, D-27, §2.7, §2.9, §13, §15, §23, R-1, R-2, R-27;
  PRD `docs/architecture/prd-performance-and-parity.md` FR-GRID-03, FR-GRID-04, NFR-P8, O-8, O-9, O-18.
- **Berhubungan dengan:** ADR-0030 (data plane; D-22 sampai D-29 dan `window()`), ADR-0003 dan ADR-0032 (sort dan filter
  grid dikerjakan di Rust, bukan Swift), ADR-0037 (spill; chunk yang tumpah didekripsi satu per satu saat kunci dibangun),
  ADR-0045 (helper DataFusion, W13-D, belum ditulis; UDF `qh_natural`).
- **Catatan penomoran:** D-8, D-18, dan D-27 di ADR ini adalah nomor dari blueprint Fase 6.

## Konteks

Grid mengurutkan, menyaring, dan mencari atas hasil yang sudah ada di memori. Hari ini itu terjadi di Swift
(`GridSort.order`, `ColumnFilter.matches`, `GridSearch.matches`, urutan filter → search → sort di
`QueryTab.displayedRows`, `QueryTab.swift:869-892`). Biayanya bukan lagi materialisasi koleksi di `body`: `displayedRows`
di-cache per `gridRevision` (`displayedCache`, `:862-863` dan `:889-890`), dan `result` membungkusnya dalam `ArrayRows`
yang juga di-cache per revisi (`:900-912`). `Array(filteredRows.enumerated())` di `body`, yang dicatat ADR-0003, sudah
tidak ada di kode. Biaya yang nyata: setiap baris hasil hidup di Swift sebagai `[[String?]]` (`PreviewResult.rows`,
`QueryTab.swift:295`), dan setiap revisi filter, search, atau sort membangun ulang array baris yang tersaring dan
terurut, sinkron di thread utama (grid membaca `tab.result` dari view SwiftUI, misalnya `ResultGrid.swift:89`). Setelah
ADR-0030, 500.000 baris × 30 kolom hidup di Rust sebagai array Arrow dan tidak boleh disalin ke Swift hanya untuk
diurutkan.

Semantik yang harus dipertahankan bukan semantik SQL, dan bukan semantik Arrow:

- NULL di akhir untuk naik, di depan untuk turun; angka dibandingkan per sel atas teks tersimpan (`GridSort.number`);
  seri diputus indeks baris; `>=`, `<=`, `>`, `<`, `=` pada filter teks dengan `swift_double` (termasuk float heksadesimal);
  search lintas semua kolom dengan pelipatan "localized"; teks dibandingkan lewat `localizedStandardCompare`.
- O-9 sudah menerima bahwa `localizedStandardCompare` (ICU, lewat Foundation) **tidak** direproduksi byte demi byte di
  Rust. Penggantinya adalah kunci natural buatan sendiri, dan daftar selisihnya harus tertulis (itulah isi ADR ini).
- O-8: sort server adalah jalur utama; sort in-memory adalah fallback (server menolak, atau grid menampilkan plan).
  Filter funnel dan search in-memory tetap lokal (FR-GRID-04).
- O-18: DataFusion adalah komponen opsional di proses terpisah dan bisa tidak terpasang, jadi grid tidak boleh membutuhkannya.

Pertanyaan yang ADR ini jawab: siapa yang menghitung view, dengan semantik siapa, dan apa saja yang boleh berbeda dari Swift.

## Opsi yang dipertimbangkan

| Opsi | Kelebihan | Kekurangan |
|---|---|---|
| **rayon atas array Arrow di `qh-result-store`, dengan satu fungsi kunci natural memcmp-able** | Grid jalan tanpa helper; semantik grid adalah port Swift apa adanya; satu kolasi natural untuk grid dan SQL; 500k baris tidak keluar dari Rust | Port yang harus diuji diferensial; kunci natural memakan memori: satu `Vec<u8>` per baris yang diurutkan, belum ditagih ke anggaran (NEG-002) |
| Tetap di Swift | Tanpa port | 500k × 30 hidup di Swift sebagai `[[String?]]`; tiap revisi filter, search, atau sort membangun ulang array tersaring dan terurut di thread utama (di-cache per revisi, bukan per `body`) |
| Kernel `arrow-ord` / `sort_to_indices` | Tanpa kode sort sendiri | Urutan byte, bukan urutan grid; NULL, numerik per sel atas teks, dan seri bukan semantik grid; 1 thread (6,8 ms numerik, 42 sampai 72 ms teks, §2.7) |
| **DataFusion di app untuk view grid** | Satu mesin | Semantik grid harus menjadi UDF; perencanaan query dibayar di setiap klik; perluasan filter selama streaming tidak cocok dengan model query batch; **ditolak** juga karena DataFusion tidak ada di app (O-18) |
| **DataFusion di helper opsional, hanya untuk SQL atas hasil dan berkas lokal** | Mesin SQL penuh untuk analitik; UDF `qh_natural(text)` memakai kunci yang sama | Bukan untuk grid; urutannya milik SQL (D-18), jadi punya divergensinya sendiri |

## Keputusan

**View grid (sort fallback, filter, search, daftar distinct) dihitung di Rust, di `qh-result-store`, dengan rayon atas
array Arrow. Semantik adalah port Swift, dengan satu pengecualian yang diterima (O-9): teks diurutkan dengan kunci
natural buatan sendiri, bukan `localizedStandardCompare`. View tidak pernah bergantung pada DataFusion. DataFusion
hanya dipakai untuk SQL atas hasil dan berkas lokal, di proses helper opsional (D-16, ADR-0045).**

Bagian "Sudah dibangun" diperiksa terhadap kode di `crates/qh-result-store/src/` dan `crates/qh-rt/src/lib.rs`.

### Sudah dibangun

1. **Pool (blueprint §13.1).** `qh_rt::view_pool()` adalah pool rayon yang dibangun sekali dengan
   `cores().performance.max(1)` thread (4 di M4), nama `qh-view-{i}`, dan QoS `UserInitiated`
   (`crates/qh-rt/src/lib.rs:200`). Semua kerja paralel view berjalan di `view_pool().install(…)`. Pool ini tidak dipakai
   ingest.
2. **Pipeline `set_view(spec)` (§13.2).** Filter (AND per kolom), lalu search, lalu sort, sama dengan `displayedRows`.
   Filter dan search berjalan paralel per chunk dan menghasilkan `Vec<u32>` baris sumber dalam urutan chunk. Sort
   membutuhkan fase `Complete` atau `Cancelled`; saat streaming jawabannya `StoreError::Streaming`
   (`view.rs:375`). `set_view` baru membatalkan yang lama (`Superseded`), dan spec kosong menghasilkan view identitas
   tanpa permutasi `Vec<u32>` (D-7; plafon 5.000.000 baris ≪ `u32::MAX`).
3. **Comparator total (R-2).** Urutan naik: `Num` < `Temporal` < `Text` < `Null`; menurun membalik semuanya. Seri `Num` dan
   `Temporal` langsung diputus indeks baris, tanpa byte mentah, karena Swift menganggap `"1.0"` dan `"1"` sama. Sejak
   Rust 1.81 sort bisa panic bila comparator tidak total, jadi comparator adalah perbandingan leksikografis atas
   level yang masing-masing total, lalu indeks baris (`cmp_keys`, `view.rs:67-86`; pemutus terakhir di `:444`).
   Totalitas mengikuti bentuk comparator, tetapi belum dijaga tes: urutannya dicakup kasus sort di `tests/view.rs` dan
   fixture diferensial, sedangkan **tes antisimetri dan transitivitas berbenih belum ada** (lihat "Belum ada").
4. **`NumKey` (§13.3).** Desimal eksak: tanda, eksponen yang disesuaikan, lalu mantissa; `-0 == 0`; float masuk lewat
   representasi terpendeknya; presisi 38 digit signifikan meniru `Decimal` Swift (`collate.rs`).
5. **Kunci natural (§13.4), seperti yang dibangun.** `collate::natural_key(s, out)`
   (`crates/qh-result-store/src/collate.rs:261-363`) menulis lima level sebagai satu deret byte: primer (kelas elemen; run
   digit ASCII dibandingkan nilainya), sekunder (tanda aksen), tersier (huruf kecil sebelum besar), kuarterner (jumlah nol
   di depan), lalu byte UTF-8 mentah, dipisah `0x00`. Perbandingan dua baris adalah `memcmp` kunci lalu indeks baris, jadi
   tidak ada comparator kedua yang harus sepakat dengan kunci. Yang dibangun **lebih sempit daripada blueprint §13.4**:
   - **Lipatan.** Teks dipecah per NFD (`split_elements`, `:368`); tanda kombinasi (Mn) masuk level sekunder, bukan
     dibuang. Hanya huruf besar ASCII yang dilipat ke huruf kecil (`:305-309`); selisihnya ada di level tersier. Huruf
     Latin beraksen yang punya dekomposisi NFD ikut terlipat lewat huruf dasarnya. Semua huruf dan angka lain (Yunani,
     Kiril, fullwidth, dan huruf Latin tanpa dekomposisi seperti `ß`, `æ`, `œ`, `ø`, `đ`, `ł`, `ı`) masuk kelas `0x05` dan
     diurutkan menurut code point; satu-satunya lipatan di kelas itu adalah Katakana ke Hiragana (`other_script_cp`,
     `:408-415`). Jadi kunci sort tidak punya lowercase penuh, tidak punya fullwidth → ASCII, dan tidak punya tabel
     `ß/ẞ → ss`, `æ → ae`, `œ → oe`, `ø → o`, `đ → d`, `ł → l`, `ı → i`. Tabel `ß → ss` dan ligatur hanya ada di `fold()`
     (`:213-229`), yang dipakai `ci_contains` dan `ci_equal` untuk filter dan search (butir 6), bukan oleh `natural_key`.
   - **Materialisasi.** `SortKey::of` (`view.rs:40-62`) membuat satu `Vec<u8>` di heap per sel (kunci natural untuk teks, digit untuk `NumKey`;
     hanya `Temporal` dan `Null` tanpa heap). `compute_view`
     membangun kunci paralel per chunk di `view_pool` untuk setiap baris tersimpan dan membuang yang tidak lolos filter
     (`:421-434`), lalu satu `sort_unstable_by` atas `(u32, SortKey)` (`:437`), yang sekuensial di dalam
     `view_pool().install` (blueprint menyebut `par_sort_unstable_by`). Tidak ada arena per chunk. Tidak ada cadangan
     memori: `StoreRegistry::reserve` (`registry.rs:345`) tidak punya pemanggil. Tidak ada jalur prefiks:
     `natural_key_prefix` (`collate.rs:429`) diekspor di `lib.rs:35` tetapi tidak dipanggil di mana pun. Tidak ada
     `TooLarge` untuk sort: varian `StoreError::TooLarge` (`store.rs:90`) dipetakan di FFI (`store_api.rs:188`) tetapi tidak
     dibuat oleh kode `qh-result-store`. Memori kunci karenanya **tidak ditagih ke anggaran 256 MiB** (NEG-002).
   - **Tes.** Kunci dicakup tes unit `natural_key_is_memcmp_able`, `natural_key_nulls_last`, dan
     `natural_key_digit_runs_order_by_value`, kasus sort di `tests/view.rs`, dan fixture diferensial. Tes berbenih untuk
     antisimetri, transitivitas, dan identitas jalur prefiks (R-27) belum ada.
6. **Filter dan search (§13.5, §13.6).** Port baris demi baris `ColumnFilter.matches` dan `GridSearch.matches` atas teks
   tersimpan; `swift_double` (`collate.rs:176`) menerima desimal dan float heksadesimal; `ci_contains` (`collate.rs:234`) melipat
   (NFC, lowercase penuh, `ß → ss`, ligatur U+FB00 sampai FB06, `ς → σ`, `İ → i̇`) lalu mencari byte, dengan jalan
   pintas ASCII tanpa alokasi.
7. **View selama streaming (§13.8).** Filter dan search boleh diterapkan saat streaming; perluasan dipicu penulis (paling
   banyak satu tugas di `view_pool`), jadi `row_count()` tetap satu load atomik. Sort tidak diizinkan saat streaming.
8. **`distinct_values(column, limit)` (§13.9)** memindai baris yang sudah diambil dalam urutan store, bukan view
   terfilter, dengan himpunan terurut byte setelah NFC dan penanda `has_null`; `more = true` melewati `limit`
   (`view.rs:464`). Swift memanggilnya dengan `limit = valuePickerLimit + 1 = 11`.

### Direncanakan (W6-T1, belum ada di repo)

9. **`StoreRows.apply(ViewSpec)` asinkron** di antrean `.userInitiated`; grid menampilkan view lama sampai view baru
   terpasang oleh hop ke main, **satu-satunya penulis `viewID` dan `count`** untuk pergantian view (D-27). `poll()` hanya
   mengikuti hitungan view yang sedang digambar (`rc.viewId == viewID`).
10. **Penjaga edit (D-27, R-35).** Selama `viewBusy` (apply berjalan), mengisi, menempel, dan menyimpan perubahan sel
    ditolak; saat view terpasang, seleksi dan antrean edit dibuang; `WritePlan` menolak membangun rencana saat sibuk.
    `CellKey` memakai indeks baris tampilan dan `WritePlan` membaca nilai asli lewat `row(at:)` untuk klausa `WHERE`
    pada `UPDATE` dan `DELETE`, jadi tanpa penjaga satu edit bisa menulis atau menghapus baris yang salah. Reviewer
    database memeriksa jalur ini di W6-T1.
11. **Implementasi Swift (`GridSort.order`, `ColumnFilter.matches`, `matchesText`, `distinctValues(in:column:)`,
    `GridSearch.matches`) pindah ke `SwiftGridReference.swift` di target tes**, supaya `SortFixtureExport` tetap bisa
    meregenerasi fixture (D-25, TM-13).
12. **Regresi daftar nilai unik (TM-3).** `ArrayRows.distinctValues` hari ini berhenti di 10 nilai pertama, sehingga kolom
    dengan 50 nilai unik mendapat pemilih berisi 10 nilai. Kontrak barunya (`DistinctSample { values, more }`, himpunan
    lengkap sampai batas) diperbaiki di commit 6a dan dijaga tes baru.

### SQL punya semantik sendiri (D-18)

Di helper DataFusion, urutan SQL adalah urutan DataFusion: byte untuk teks, `nulls_max` (NULL terakhir untuk naik, seperti
PostgreSQL). `ORDER BY qh_natural(col)` memberi kolasi grid, karena UDF `qh_natural(text) -> binary` memanggil fungsi
`natural_key` yang sama. Jadi grid dan SQL tidak punya dua kolasi natural yang bisa berselisih. UDF itu ada di helper
(W13-T8a) dan **belum dibangun**.

## Alasan

1. **Grid harus jalan tanpa helper.** O-18 membuat DataFusion opsional. Bila view bergantung padanya, grid tidak berfungsi
   pada instalasi biasa.
2. **Semantik Swift bukan SQL.** NULL di akhir, numerik per sel atas teks, seri diputus baris, operator filter hasil
   port, dan search lintas kolom dengan pelipatan masing-masing harus menjadi UDF di DataFusion, dan perencanaan query
   dibayar di setiap klik. Perluasan filter selama streaming juga tidak cocok dengan query batch.
3. **Angka tidak membalik keputusan, dan jujur soal batasnya.** Sort 500k, median 5 run, 31 chunk × 16.384 baris
   (§2.7; probe `target/run/df-probe`):

   | Jalur | Numerik | Teks natural A | Teks natural B |
   |---|---|---|---|
   | rayon (pool global **10 thread**) | 2,7 ms | 28,6 ms | 27,2 ms |
   | kernel arrow `sort_to_indices`, 1 thread | 6,8 ms | 72,2 ms | 51,1 ms |
   | DataFusion 1 partisi | 36,0 ms | 66,8 ms | 53,4 ms |
   | DataFusion 4 partisi (**4 worker tokio**) | 18,5 ms | 55,0 ms | 47,7 ms |

   **Jumlah thread tidak setara.** Baris rayon memakai 10 thread, DataFusion 4 worker, dan `view_pool` produk hanya memakai
   P-core (4 di M4). Untuk numerik selisihnya cukup besar (kernel arrow 1 thread pun 6,8 ms) sehingga kesimpulan aman.
   Untuk natural, rayon dengan 4 thread **belum diukur** dan bisa setara dengan DataFusion 4 partisi (48 sampai 55 ms).
   Klaim lama "rayon 6,9× dan 1,7 sampai 1,9× lebih cepat" dicabut. Keputusan ini tidak bergantung pada angka itu.
4. **Angka W5-T2 dengan `view_pool` sebenarnya** (§2.9; `bench_ffi` rilis, M4, 4 thread, commit `bfb9680`, store sintetis
   1 juta × 30 seluruhnya resident, 3 ulangan; ekstrapolasi linear ke 500k **belum diukur**):

   | `set_view` di 1 juta × 30 | Waktu | Ekstrapolasi 500k | Target |
   |---|---|---|---|
   | sort bigint | 166 sampai 221 ms | 83 sampai 111 ms | NFR-P8 ≤ 100 ms: di tepi |
   | sort teks, kunci natural | 153 sampai 182 ms | 76 sampai 91 ms | NFR-P8 ≤ 300 ms: lulus |
   | filter `Text` | 25 sampai 27 ms | 12,5 sampai 13,3 ms | sasaran ≤ 300 ms: lulus |
   | search semua kolom | 768 sampai 796 ms | 384 sampai 398 ms | sasaran ≤ 300 ms: **meleset**, dicatat |

   Gate 500k yang nyata adalah W5-T3 dan W6-T2 dan belum dijalankan. Search yang meleset adalah alasan `apply`
   asinkron dan view lama tetap tampil sampai yang baru siap.
5. **Satu fungsi kunci** berarti tidak ada comparator kedua yang bisa menyimpang dari kunci, dan sort grid serta SQL tidak
   punya dua kolasi natural yang berselisih.

## Daftar divergensi dari Swift (O-9)

Sumber kebenarannya adalah `DIVERGENCES` di `crates/qh-result-store/tests/differential.rs`, bukan dokumen ini.
Fixture `crates/qh-result-store/tests/fixtures/differential.json` ditulis oleh tes Swift `SortFixtureExport` yang
menjalankan `GridSort`, `ColumnFilter`, dan `GridSearch` asli atas korpus tetap (40 baris × 3 kolom). Saya menghitung isi
berkasnya pada 6 Okt 2026: **228 kasus**, yaitu 6 sort, 186 filter (153 `text`, 33 `values`), 30 search, dan 6 pipeline.
Tes mensyaratkan lebih dari 200. Angka "210 kasus" di blueprint dan ledger tidak cocok dengan berkas ini dan jangan dikutip.

Mekanisme penjaga: kasus yang berbeda dan tidak terdaftar gagal (`UNLISTED`); kasus terdaftar yang kini identik gagal
(`STALE`); kasus terdaftar yang barisnya berubah gagal (`CHANGED`); entri yang tidak menamai kasus fixture gagal. Jadi
divergensi tidak bisa menghilang atau membesar diam-diam. Aturannya: setiap entri **diterima** oleh blueprint atau
O-9; selisih terhadap port baris demi baris adalah cacat yang diperbaiki, bukan entri baru.

Tujuh kasus terdaftar, dengan tiga alasan:

| Alasan | Kasus fixture | Yang terjadi |
|---|---|---|
| **NATURAL** (O-9) | `sort column=2 descending=false`, `sort column=2 descending=true`, `pipeline filter=1:"<10" search="" sort=2:true` | Urutan teks adalah kunci natural, bukan `localizedStandardCompare`. Digraf (U+01C5, U+01C6), `ı` tanpa titik, `İ`, ligatur `ﬁ`, dan jamo Hangul berurutan berbeda dari ICU. |
| **NUMERAL** (O-9) | `sort column=1 descending=false`, `sort column=1 descending=true` | Foundation membaca digit Arab-Indik dan fullwidth sebagai angka saat mengurutkan teks, sehingga U+0663 dan `１２` fullwidth terurut menurut nilai di antara sel teks; kunci natural menaruhnya setelah teks berdigit ASCII. Keduanya bukan angka bagi `GridSort.number`. |
| **CLUSTER** | `filter column=2 needle="i"`, `search term="i"` | `ci_contains` melipat lalu mencari byte, jadi `i` cocok di dalam `İ` terlipat (i + U+0307) dan ligatur `ﬁ` (fi); Foundation mencocokkan seluruh grapheme cluster dan tidak. |

Divergensi yang diputuskan di blueprint tetapi **tidak ada di fixture**, sehingga belum diuji diferensial:

- **Token NULL di filter nilai (§13.5).** Swift memakai token `"\u{0}null"` sebagai pengganti NULL; Rust memakai `None` di
  batas Swift. Nilai nyata `"\u{0}null"` tidak lagi ikut cocok saat NULL dipilih. Divergensi yang dicatat, dan Rust yang benar.
  Saya mencari token itu di `differential.rs` dan `differential.json` dan tidak menemukannya.
- **Nilai bertipe diurutkan menurut nilainya (§13.3).** Timestamp dengan offset campuran menjadi kronologis. Tes
  diferensial hanya memakai nilai `Text`, jadi jalur bertipe diuji oleh tes unit Rust sebagai perilaku yang disengaja,
  bukan terhadap Swift.
- **Seri `Num` dan `Temporal` tanpa pemutus byte (koreksi AR W2-A3)** adalah kecocokan dengan Swift, bukan divergensi.

**Kandidat divergensi yang belum dikonfirmasi atau dibantah fixture** (blueprint §13.4 dan §13.7): urutan antar-skrip
(ICU menaruh Hangul sebelum Kana dan Han); emoji dan simbol; digit non-ASCII dan fullwidth di dalam run numerik di luar
yang sudah tercakup NUMERAL; pengurutan khusus lokal (`ch` di cs, `å` di sv); urutan tersier ICU yang sebenarnya; aturan Turki
saat locale `tr`; kecocokan di dalam cluster kombinasi yang tidak punya bentuk precomposed; dan lipatan khusus lokal.
Blueprint menyatakan locale saat ekspor dicatat di manifest fixture; berkas `differential.json` hari ini tidak punya
manifest atau kunci locale (kuncinya hanya `columns`, `filters`, `pipelines`, `rows`, `searches`, `sorts`).

**Divergensi SQL (D-18)**, hanya berlaku di helper: urutan byte untuk teks dan `nulls_max`, kecuali bila SQL memakai
`ORDER BY qh_natural(col)`. Ini tidak masuk daftar fixture karena tidak ada pembanding Swift; ia dijaga tes helper (W13-T8a).

## Konsekuensi

### Positif

- **POS-001**: 500.000 baris tidak pernah disalin ke Swift untuk sort, filter, atau search. Sort numerik 500k diperkirakan
  83 sampai 111 ms dan teks natural 76 sampai 91 ms (ekstrapolasi, belum diukur di 500k).
- **POS-002**: Grid tidak bergantung pada helper atau pada DataFusion; app tetap sekitar 30 MB (O-18).
- **POS-003**: Satu kolasi natural untuk grid dan SQL, dan divergensi terhadap Swift tertulis serta dijaga oleh tes yang
  gagal bila daftar menyimpang.
- **POS-004**: Comparator total dan sort stabil secara efektif (indeks baris sebagai pemutus terakhir) menutup risiko
  panic sort Rust ≥ 1.81.

### Negatif

- **NEG-001**: Pengguna melihat urutan teks yang berbeda dari Foundation pada rangkaian tertentu (tujuh kasus
  terdaftar di atas). Diterima O-9, tetapi nyata.
- **NEG-002**: Kunci natural memakan memori dan tidak ditagih ke anggaran. Kode yang dibangun membuat satu `Vec<u8>` di
  heap per baris teks yang diurutkan (`view.rs:40-62`, `:421-434`), tanpa arena, tanpa cadangan lewat `StoreRegistry::reserve`,
  dan tanpa jalur prefiks; sort tidak pernah ditolak dengan `TooLarge`, jadi tidak ada penolakan terkendali bila memori
  tidak cukup. Hitungan tangan dari `natural_key`, **belum diukur**: kunci `row-123456-c01` berisi 65 byte (primer 28,
  sekunder 7, tersier 7, kuarterner 9, mentah 14), kapasitas `Vec` hasil penggandaan tanpa `shrink_to_fit` bisa 112 byte,
  dan tiap baris membawa tuple `(u32, SortKey)`. Untuk teks sebentuk itu, isi kunci saja sedikitnya 32,5 MB untuk 500k baris
  dan 325 MB untuk plafon 5.000.000 baris (D-29), belum termasuk kapasitas `Vec`, tuple, bitmap `included`, dan `Vec<u32>`
  baris, dan semuanya di luar anggaran 256 MiB. **Risiko terhadap D-29**: plafon dinaikkan ke 5.000.000 tanpa batas memori untuk sort, jadi P-1 dan W6-T2 harus
  mengukur memori puncak sort di plafon itu. Angka "sekitar 50 byte per baris, sekitar 30 MB untuk 500k" di blueprint
  §13.4 dan R-27 menggambarkan rancangan arena, bukan kode ini.
- **NEG-003**: Port yang harus dijaga. Setiap perubahan semantik di Swift (`ColumnFilter`, `GridSearch`) harus diikuti
  fixture baru; `SortFixtureExport` bergantung pada `SwiftGridReference.swift` yang secara produksi sudah mati.
- **NEG-004**: Search 1 juta × 30 sekitar 0,8 detik meleset dari sasaran 300 ms dan hanya dicatat; UI menanganinya sebagai
  kerja asinkron, bukan sebagai gate.
- **NEG-005**: Ekstrapolasi ke 500k dan klaim natural-vs-DataFusion dengan jumlah thread setara belum terukur (R-17,
  `development-plan.md` §11 butir 5).
- **NEG-006**: Rayon view bersaing CPU dengan ingest di P-core (R-17). Dimitigasi pool terpisah dari runtime tokio dan
  fakta bahwa view hanya fallback dan jarang; diukur di sesi bench W6-T2.

### Belum ada

- Sisi Swift (`apply`, penjaga edit, `SwiftGridReference`, regresi TM-3), seluruhnya W6-T1.
- Fixture paritas format (`format`, `openable`, `width`, `number`, dan lainnya, blueprint §15.2): hanya `differential.json`
  yang dibuat (TM-10). Itu sebabnya ADR-0030 D-24 memakai pintu darurat, dan `Json` kembali ke Rust hanya setelah B-21.
- Fixture token NULL, nilai bertipe, dan manifest locale (di atas).
- Bench 500k nyata (W5-T3, W6-T2) dan natural dengan 4 thread rayon dibanding DataFusion 4 partisi.
- UDF `qh_natural` dan divergensi SQL (W13-T8a).
- Putaran tinjau kedua atas W4-T3 (kebijakan dua putaran, O-19 dan O-20; lihat status ADR-0037).
- Backlog (rancangan blueprint §13.4 dan R-27 yang tidak ada di kode): arena kunci per chunk, cadangan memori lewat
  `StoreRegistry::reserve`, jalur prefiks 16 byte (`natural_key_prefix`, tidak dipanggil), dan `TooLarge` untuk sort.
  Selama belum ada, memori kunci sort tidak ditagih ke anggaran 256 MiB (risiko D-29, NEG-002).
- Backlog: tes berbenih untuk antisimetri dan transitivitas `cmp_keys` serta identitas jalur prefiks (R-27).
- Backlog, belum diputuskan: lipatan yang digambarkan blueprint §13.4 (lowercase penuh, fullwidth → ASCII, tabel
  `ß/ẞ → ss`, `æ`, `œ`, `ø`, `đ`, `ł`, `ı`) tidak ada di `natural_key`; bangun atau ubah blueprint agar sesuai dengan kode.

## Bukti

- Kode: `crates/qh-result-store/src/collate.rs`, `view.rs`, `store.rs`; `crates/qh-rt/src/lib.rs`;
  `crates/qh-ffi/src/store_api.rs` (`set_view` `:691`, `distinct_values` `:738`).
- Tes: `crates/qh-result-store/tests/differential.rs` (`rust_view_matches_the_swift_fixtures`; env `QH_DIFFERENTIAL_FIXTURE`
  menunjuk salinan lain, yang membuktikan perbandingan bisa gagal), `tests/view.rs`; tes unit di `collate.rs`
  (`natural_key_is_memcmp_able`, `natural_key_nulls_last`, `natural_key_digit_runs_order_by_value`, `swift_double_*`, `ci_contains_*`).
- Pengukuran: blueprint Fase 6 §2.7 (probe sort) dan §2.9 (W5-T2: `target/run/w5t2-bench.json`, commit `bfb9680`, scratch
  gitignored). ADR ini tidak menjalankan ulang gate, tes, atau bench.

## Verdict architect-reviewer

**Verdict: dua koreksi blocking, sudah diterapkan di dokumen ini (6 Okt 2026); menunggu tinjau ulang (pending review, O-20).**
Menurut O-20 review hanya satu putaran. Koreksi diperiksa terhadap kode oleh penulis (pembacaan kode, tanpa menjalankan
gate, tes, atau bench) dan dicatat sebagai pending review, bukan putaran ketiga.

**Blocking, diterapkan**

1. "Sudah dibangun" butir 5 dan 3 mengaku diperiksa terhadap kode, tetapi menggambarkan rancangan blueprint §13.4
   (`fase-6-data-plane.md:843`), bukan kode: (a) `natural_key` (`collate.rs:261-345`) tidak punya lowercase penuh,
   fullwidth → ASCII, dan tabel `ß/ẞ → ss`, `æ → ae`, `œ → oe`, `ø → o`, `đ → d`, `ł → l`, `ı → i`; hanya huruf besar ASCII
   yang dilipat (`:305-309`), huruf lain berkunci code point di kelas `0x05` dengan Katakana ke Hiragana saja
   (`other_script_cp`, `:408-415`), dan tabel `ß` dan ligatur hanya ada di `fold()` untuk `ci_contains`. (b)
   `natural_key_prefix` (`:429`) diekspor tetapi tidak dipanggil; `view.rs:421-447` membangun satu `Vec<u8>` per baris lewat
   `SortKey::of` tanpa arena, cadangan, fallback, atau `TooLarge`. (c) Tidak ada tes berbenih antisimetri, transitivitas,
   atau identitas prefiks (R-27) di `collate.rs` maupun `tests/view.rs`. NEG-002 mewarisi kesalahan yang sama ("sekitar 50
   byte per baris, sekitar 30 MB untuk 500k termasuk offset", "jalur prefiks menggantikannya", "sort bisa berakhir
   `TooLarge`"). Diperbaiki: butir 3, butir 5, NEG-002, kalimat status, dan "Belum ada" (arena, jalur prefiks, `TooLarge`,
   dan tes berbenih menjadi backlog). Memori kunci yang tidak ditagih dicatat sebagai risiko terhadap plafon D-29. Penjelasan
   divergensi NATURAL dan NUMERAL diperiksa ulang dan tetap berlaku seperti tertulis.
2. Konteks dan tabel opsi mengutip `QueryTab.swift:824-848` untuk `displayedRows`, padahal baris itu `cellValue`,
   `fetchedValue`, dan `clearEditUndo`; `displayedRows` ada di `QueryTab.swift:869-892` (berkas tidak berubah terhadap HEAD).
   Kalimat bahwa `Array(filteredRows.enumerated())` di `body` mematerialisasi koleksi "hari ini" juga salah: kode itu sudah
   tidak ada, `displayedRows` di-cache per `gridRevision` (`displayedCache`, `:862-863` dan `:889-890`), dan `result`
   membungkusnya dalam `ArrayRows` yang di-cache per revisi (`:900-912`). Diperbaiki di Konteks dan di baris "Tetap di Swift"
   pada tabel opsi: biaya yang nyata adalah semua baris hidup di Swift sebagai `[[String?]]` dan tiap revisi filter, search,
   atau sort membangun ulang salinan tersaring dan terurut di thread utama. Keputusannya tidak berubah.

**Non-blocking, dicatat dan belum diterapkan**

- Butir 8 "Sudah dibangun" (kalimat terakhir) menyebut "Swift memanggilnya dengan `limit = valuePickerLimit + 1 = 11`". Pada
  HEAD `ResultRows.distinctValues` masih sinkron, mengembalikan `[String?]`, dan berhenti di 10; `limit + 1` hanya ada di
  pohon kerja 6a yang belum di-commit. Pindahkan kalimat itu ke "Direncanakan".
- Satu temuan penulis saat memeriksa ulang, di luar daftar reviewer: `view.rs:437` memakai `sort_unstable_by` sekuensial,
  bukan `par_sort_unstable_by` seperti di blueprint §13.4. Sudah dicatat di butir 5; dampaknya terhadap angka W5-T2 dan
  keputusan rayon (Alasan 3 dan 4) belum dinilai.
- Temuan non-blocking untuk dokumen lain (ADR-0030, `development-plan.md`, ADR-0037) ada di bagian "Verdict
  architect-reviewer" ADR-0030.

**Terverifikasi benar oleh reviewer, tanpa tindakan:** `qh_rt::view_pool` di P-core pada `UserInitiated` (`qh-rt/src/lib.rs:200`);
penolakan `Streaming` (`view.rs:375`); urutan `Num` < `Temporal` < `Text` < `Null` dan pemutus indeks baris
(`view.rs:27-75`, `:437-445`); `distinct_values` atas urutan store dengan NFC dan `more` (`view.rs:464`); `swift_double`,
`ci_contains`, `natural_key`, dan `natural_key_prefix` di `collate.rs:176`, `:234`, `:261`, `:429`; fixture diferensial
memang 228 kasus (6 sort, 186 filter = 153 teks + 33 nilai, 30 search, 6 pipeline, korpus 40 × 3, tanpa manifest atau kunci
locale); `DIVERGENCES` berisi tepat tujuh kasus dengan alasan NATURAL, NUMERAL, dan CLUSTER seperti tertulis; filter nilai
dengan `[null]` ada di fixture, sedangkan tabrakan token `"\u{0}null"` tidak.

## Referensi

- ADR-0003 (sort dan filter di Rust), ADR-0030, ADR-0032, ADR-0037.
- `docs/architecture/blueprints/fase-6-data-plane.md` §2.7, §2.9, §13, §15, §23, §25 (R-1, R-2, R-17, R-27, R-35).
- `docs/architecture/performance-plan.md` §13 (alasan lama menolak DataFusion, kini hanya untuk grid) dan §14 butir 2 dan 4.
- PRD O-8, O-9, O-18, NFR-P8.
- Tugas penerus: W6-T1, W5-T3, W6-T2, W13-T8a (UDF `qh_natural`), dan ADR-0045.
