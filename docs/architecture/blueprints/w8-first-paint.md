# Blueprint W8-F1: first paint dan siklus hidup grid

- **Status:** diimplementasikan di tree `query_hive-wts1` (di atas `c66dae6`), semua gate Swift hijau (§6). Menunggu A/B eksklusif MAIN: aturan simpan dari W8-T2 adalah turun minimal 10% pada p50 atau p95 TTFR S1t tanpa regresi.
- **Untuk:** W8-F1 (keputusan W8-T2 §1 baris 1 dan 2, §3, §4). Menyentuh `AppModel+Run.swift`, `ResultGrid.swift`, `GridTableView.swift`, `StoreRows.swift`. `ResultGridTable.swift` tidak berubah.
- **Sumber:** `target/run/w8t2-decision.md`; kode di `5a7b0de`: `AppModel+Run.swift` (`runPreview`), `ResultGrid.swift` (`content`, `grid`), `ResultGridTable.swift` (`makeNSView`, `Coordinator.apply`, `pollRows`), `GridTableView.swift` (`viewDidMoveToWindow`, `draw`), `StoreRows.swift` (`poll`), `PerfSignposts.swift`, `RustEngine.swift` (`Sink.onEvent`), `crates/qh-ffi/src/commands.rs` (`StoreTarget`).
- **Cara membaca klaim.** Aplikasi tidak dijalankan di lane ini, jadi tidak ada angka TTFR di dokumen ini. Yang ada dua hal: (1) pembacaan kode, dengan nomor baris dari `5a7b0de`; (2) pengukuran tingkat unit di test host (`swift test`, build debug, jendela sungguhan di layar, mesin Mac16,12 yang dipakai lane lain, load average 4 sampai 12 pada 10 core). Engine-nya `StreamingEngine`, sebuah test double yang tidur sebesar waktu query lalu mendorong baris ke `FakeResultHandle` dan mengirim `columns`, `progress`, `done` satu hop per event seperti `Sink.onEvent`. Jadi angka di §3 mengukur **berapa kerja main thread yang pindah dari jalur kritis**, bukan TTFR. Dua jam dipakai: waktu dinding Run sampai `firstPaint`, dan **CPU main thread** dari hop `columns` sampai `firstPaint` (`thread_info`), yang tidak bergeser oleh load lane lain. Ulang dengan `QH_FIRST_PAINT_PROBE=1 swift test --filter testTheMainThreadCostOfARun` (di `FirstPaintTests.swift`); untuk membandingkan dua build, jalankan bergantian beberapa putaran, karena satu putaran di mesin yang ramai bisa meleset dua kali lipat.
- **Kontrak.** §4 mengikat: satu call site `ResultGridTable`, tabel dibangun saat Run mulai dan hidup lintas Run, `StoreRows.count` nol setelah `release()`. §3 dan §5 adalah bukti dan catatan, bukan kontrak.

## Ringkasan

Mesin bukan penyebab TTFR S1 yang meleset, dan yang memakan waktu bukan satu pembangunan ulang grid tetapi **dua**. Pertama saat event `columns` (tabel dibangun di cabang "ada placeholder"), kedua saat baris pertama tiba: `ResultGrid.grid` punya dua `ResultGridTable` di dua cabang `if`, dan bagi SwiftUI itu dua view berbeda, jadi baris pertama membuang tabel yang baru dibangun lalu membangun yang kedua (keputusan W8-T2 hanya menyebut yang pertama). Setiap Run juga membuang tabel lama lebih dulu (`preview = nil` mengganti panel dengan spinner). Selama main thread membangun tabel, hop `progress` dan `done` dari engine menumpuk dan baru diproses sesudahnya, itu sebabnya `ttfr > duration` di seluruh 20 sampel S1 10k.

Perbaikannya mengubah struktur, bukan menambah jalur cepat:

1. **Satu call site tabel.** Placeholder jadi anak kedua `VStack` di bawah tabel yang tingginya berganti, bukan cabang `if` lain. Baris pertama tidak lagi membangun tabel.
2. **Tabel dibangun saat Run dimulai**, di belakang spinner, dan **hidup lintas Run** (Run lagi, sort atau search di server). Biaya membangun tabel (4 sampai 7 ms di main thread dalam build debug) pindah ke waktu tunggu engine.
3. **Toolbar juga dibangun di belakang spinner** (SwiftUI saja, di-veil dengan `.opacity`). Tabel sendiri tidak boleh dibungkus: `.opacity` di atas `NSViewRepresentable` menyisipkan `_NSGraphicsView` dan memindahkan tabel keluar-masuk jendela (`viewDidMoveToWindow` dua kali per Run).
4. `StoreRows.count` nol setelah `release()`, supaya tabel yang masih memegang store lama tidak menggambar barisnya.

Hasil di test host (§3), tiga putaran bergantian lama dan baru: CPU main thread dari `columns` sampai paint pertama turun 30 sampai 47%, waktu dinding Run sampai paint turun 12% (Run baru) sampai 35% (Run lagi, engine cepat). Ini bukan prediksi TTFR: engine di sini hanya `sleep`, build-nya debug, dan 6 sampai 12 ms yang terhemat di sini dibandingkan 34 ms `runDone`→`firstPaint` yang diukur W8-T2 di aplikasi menunjukkan arah dan urutan besarnya saja.

Tiga hal yang diusulkan keputusan W8-T2 **tidak** dikerjakan, dengan alasan di §4.4: menggabungkan `columns` dan `progress` ke satu hop (diukur, tidak ada efek), `displayIfNeeded()` sinkron pada pertumbuhan pertama (tidak ada mekanisme yang menghematnya), dan flag bench (tidak perlu: struktur lama dan baru dibandingkan dengan menukar tiga file).

## 1. Yang terjadi per Run sebelum perubahan

Nomor baris dari `5a7b0de`.

| Langkah | Di mana | Apa |
|---|---|---|
| Run | `AppModel+Run.swift:335-368` | `previewing = true`, `tab.preview = nil` (:346), store baru dipasang |
| update SwiftUI | `ResultGrid.swift:105` | `loadingLabel != nil` dan `preview == nil`: seluruh panel jadi spinner. Tabel lama (kalau ada) dibuang: coordinator `deinit`, link tampilan di-invalidate |
| hop `columns` | `AppModel+Run.swift:387-397`, `RustEngine.swift:340-353` | `tab.preview = PreviewResult(...)` |
| update SwiftUI | `ResultGrid.swift:219-224` | `placeholder != nil` (0 baris, masih loading): cabang pertama, **tabel #1** dibangun (`makeNSView`, `apply(force:)`, header, scroll view), `viewDidMoveToWindow` memulai link dan polling pertama |
| hop `progress` | `AppModel+Run.swift:398-408`; `commands.rs:1998-2004` | engine selalu mengirim `progress` pada batch pertama, dan baris sudah ada di store sebelum eventnya (`writer.push` mendahului `emit`). `pollHook` membangunkan tabel #1 dan `fetchedRows` berubah |
| update SwiftUI | `ResultGrid.swift:225-228` | `tab.result.count > 0`: `placeholder == nil`, cabang `else`. **Tabel #1 dibuang, tabel #2 dibangun**, `apply(force:)` lagi, polling lagi, gambar pertama |
| hop `done` | `AppModel+Run.swift:409-413` | `applyPreviewDone` menetapkan `tab.preview` lagi: `gridRevision += 1`, `apply` penuh dan repaint seluruh tabel |
| `firstPaint` | `GridTableView.swift:337`, `PerfSignposts.swift:84-92` | satu putaran main queue setelah `draw` baris 0, dan `CATransaction.flush()` bila bench merekam |

Dua akibat yang bisa dibaca dari tabel itu tanpa mengukur. Satu, hop 2 dan 3 dikirim FFI thread satu atau dua milidetik setelah hop 1 (satu batch pertama untuk S1 1k), sedangkan dua pembangunan tabel memakan main thread jauh lebih lama dari itu; hop yang menumpuk diproses sesudahnya dalam satu putaran, jadi `runDone` terstempel **sesudah** `firstDraw`. Dua, `firstPaint` baru terstempel sesudah semua hop yang sudah antre, jadi putaran `done` (update SwiftUI dan repaint penuh) ikut terhitung di TTFR.

## 2. Hipotesis W8-T2 §3

| H | Pernyataan | Bukti di lane ini | Status |
|---|---|---|---|
| H1 | Grid dibangun ulang tiap Run: `gridAttached` − `columns` ≥ 1 frame | Baseline di test host: 8 sampai 16 ms (median per putaran) antara hop `columns` dan `viewDidMoveToWindow` pertama, yaitu satu pembangunan tabel penuh di jalur kritis; sebagian besar waktunya ada di AppKit dan SwiftUI, yang bukan debug. Ditambah pembangunan kedua yang tidak disebut W8-T2 (§1, baris "update SwiftUI" kedua) | Terkonfirmasi, dan lebih buruk dari yang diduga: dua pembangunan |
| H2 | Main thread sibuk: `runDone` − `engine.done` ≥ 10 ms pada mode 22 ms | Di test double `engine.done` tidak distempel (tidak lewat `Sink`), tapi `done` dikirim sekitar 1 ms setelah `columns` dan diproses 15 sampai 30 ms sesudahnya di baseline, 10 sampai 16 ms pada varian tengah (satu call site, tanpa toolbar awal) | Terdukung secara tidak langsung. X7 harus membaca `engine_done` vs `run_done` di aplikasi sungguhan |
| H3 | Menunggu siklus tampilan: `firstDraw` − `firstRows` tersebar merata 0 sampai 16,7 ms | Di test host tidak ada menunggu vsync di jalur ini: `needsDisplay` dilayani commit Core Animation di akhir putaran yang sama dengan update SwiftUI, bukan link tampilan. `firstDraw` − `firstRows` 1 sampai 8 ms, bergantung pada apakah hop `progress` sudah antre sebelum update SwiftUI mulai. Tidak terpisahkan dari H1 di sini | Tidak terdukung. Jika X7 menunjukkan sebaran merata, ada jalur yang digerakkan link di aplikasi sungguhan (jendela tertutup sebagian, misalnya) yang tidak ada di test host |
| H4 | Biaya flush: `firstPaint` − `firstDraw` | 14 sampai 29 ms di baseline dan 5 sampai 14 ms sesudah perubahan, dan **bukan hanya flush**: di antaranya ada putaran `done` (H7) dan hop `async` milik `firstPaint`. `CATransaction.flush()` hanya dijalankan saat bench merekam | Terkonfirmasi sebagian. Laporkan `firstDraw` di samping `firstPaint` agar flush bisa dipisahkan |
| H5 | Tidak ada paint di tengah stream pada 10k: `firstDraw` setelah `engine.done` | Di test double `done` tiba 1 ms setelah `columns` dan paint pertama 10 sampai 22 ms sesudahnya, dalam build lama maupun baru; jadi paint selalu sesudah `done` selama satu update SwiftUI lebih lama dari engine mendorong satu batch. Engine nyata untuk 10k membutuhkan puluhan ms, sehingga paint di tengah stream mungkin kalau main thread bebas, dan dua pembangunan tabel itulah yang membuatnya tidak bebas | Terdukung oleh H1 dan H2. Tidak bisa dipastikan tanpa mesin nyata |
| H6 (baru) | Dua call site `ResultGridTable` di dua cabang `if` = dua view | Kode (`ResultGrid.swift:219-229`); `FirstPaintTests.testOneTableCarriesARunFromItsStartToItsEnd` gagal di struktur lama (tabel tidak ada saat Run mulai, dan beda objek sesudah baris pertama) | Terkonfirmasi |
| H7 (baru) | Putaran `done` mengulang `apply` penuh dan repaint seluruh tabel | `preview.didSet` menaikkan `gridRevision` pada setiap penetapan, termasuk `done` yang hanya mengubah `rowCount`, `truncated`, `elapsedMS` (`QueryTab.swift:435-447`); `apply` lalu menjalankan `refreshRowsAndGeometry`, `textCache.removeAll()`, `needsDisplay = true` | Terbaca di kode; ikut dalam 5 sampai 14 ms antara `firstDraw` dan `firstPaint` (H4), tidak dipisahkan di sini. **Tidak diperbaiki di sini**: berkas `QueryTab.swift` di luar daftar F1. Kandidat F-lanjutan |

## 3. Pengukuran di test host

Satu putaran = `swift test --filter testTheMainThreadCostOfARun`, 20 sampel per kasus setelah 2 pemanasan, median. Tiga putaran bergantian lama (`5a7b0de` ditambah stempel) dan baru. "Run lagi" artinya Run kedua dan seterusnya di tab yang hasilnya masih tampil; "Run baru" mereset `preview` dan store dulu, seperti `BenchMode.fetch`. Engine 20 ms kecuali dicatat.

| Kasus | Lama: wall / CPU | Baru: wall / CPU | Selisih wall / CPU |
|---|---|---|---|
| Run baru, 1k baris × 10 kolom | 51,9 / 24,3 | 45,7 / 17,1 | −12% / −30% |
| Run baru, 10k × 10 | 51,1 / 22,9 | 44,5 / 15,4 | −13% / −33% |
| Run baru, 1k × 100 kolom | 58,1 / 31,1 | 50,9 / 21,1 | −12% / −32% |
| Run lagi, 1k × 10 | 55,2 / 24,3 | 42,1 / 13,4 | −24% / −45% |
| Run lagi, 1k × 10, engine 5 ms | 33,3 / 23,6 | 21,7 / 12,5 | −35% / −47% |

Satuan ms; median dari median tiga putaran (tiap putaran 20 sampel). Rentang antar putaran 1 sampai 5 ms, kecuali dua putaran "baru" yang tertimpa beban lane lain (Run baru 1k × 10: 52 ms dan 24,0 CPU; 1k × 100: 60 ms dan 31,2 CPU); median tetap benar karena dua putaran lainnya sepakat.

Bacaan:

- Selisih terbesar ada di CPU setelah `columns`, karena itu kerja yang dipindahkan. Waktu dinding turun lebih kecil karena 20 ms engine dan flush bench tetap ada.
- Kasus "Run lagi, engine 5 ms" paling jelas: tabel sudah ada, jadi yang tersisa di jalur kritis hanya `apply` kolom baru dan gambar.
- Toolbar yang dibangun di belakang spinner menghemat 3 sampai 4 ms lagi (CPU 21,0 menjadi 17,1 pada 1k × 100 kolom; 17,5 menjadi 13,6 pada 1k × 10; putaran bergantian varian "overlay saja" lawan akhir, tiga putaran masing-masing, rentang 1 ms), sekitar seperlima dari sisa jalur kritis. Itu alasan toolbar ikut dibangun lebih awal (§4.2).
- Satu hal yang tidak berubah banyak: `firstDraw` − `columns` hanya turun 1 sampai 2,5 ms (12 sampai 15 menjadi 10 sampai 14). Sisa penghematan CPU ada di antara gambar pertama dan `firstPaint`. Mekanismenya tidak dipisahkan di sini (kandidat: putaran `done` kini mengenai satu tabel yang sudah stabil, dan tabel pertama tidak perlu dibuang), jadi jangan dibaca sebagai bukti untuk salah satunya; X7 dengan `sample` yang akan memisahkannya.

## 4. Keputusan

### 4.1 Satu call site tabel

`ResultGrid.grid(_:covered:)` menaruh `ResultGridTable` di satu tempat. Placeholder (`placeholderBody`) jadi anak berikutnya di `VStack`, hanya bila `GridPlaceholder.whenEmpty` memberi nilai. Tinggi tabel: tinggi header selama ada placeholder, sisa panel bila tidak, lewat `.frame(minHeight:maxHeight:)` yang nilainya berganti. Identitas struktural tabel tidak berubah, jadi tidak ada pembangunan ulang. Tata letaknya sama dengan sebelumnya (G-VIS dan ChromeParity tidak bergeser, §6).

### 4.2 Tabel dibangun saat Run mulai dan hidup lintas Run

`ResultGrid.content` memasang tabel bila `awaitingColumns` (Run berjalan, `preview == nil`) atau hasil punya kolom. Selama menunggu kolom: tabel kosong (tidak menggambar apa pun tanpa kolom), toolbar dibangun tapi tidak terlihat dan tidak bisa dijangkau (`.opacity(0)`, `.allowsHitTesting(false)`, `.disabled`, `.accessibilityHidden`), banner dan placeholder tidak ada, dan spinner penuh panel jadi `.overlay`. Hasilnya di layar sama dengan spinner satu panel yang dulu. Setelah `columns`, overlay hilang dan toolbar tampak; tabel yang sama menerima kolom lewat `updateNSView`.

Mengapa overlay dan bukan `.opacity` di atas tabel: varian pertama membungkus tabel dengan `.opacity`, dan SwiftUI menyisipkan `_NSGraphicsView` (alpha 0) sehingga tabel dipindah keluar dan masuk jendela pada tiap Run (stempel `gridAttached` terpicu di Run kedua padahal tabelnya sama, objeknya tetap). Pemindahan itu menjalankan `viewDidMoveToWindow` lagi: link tampilan dibuat ulang, observer dilepas dan dipasang, `closePeek()`, polling. Murah per kali, tapi ada di jalur kritis dan tidak perlu. `FirstPaintTests.testARunDoesNotMoveTheTableOutOfItsWindow` menjaganya. Toolbar tidak punya masalah itu, karena isinya SwiftUI.

Sort dan search ke server, serta `explain`, lewat jalur yang sama (`runPreview`, `explain`): semuanya menetapkan `preview = nil` lalu menunggu `columns`, jadi tabelnya juga dipakai ulang. Run yang berakhir tanpa kolom (tulis, atau Stop sebelum kolom) atau gagal membuang tabel seperti dulu; yang berubah hanya bahwa tabel itu sempat dibangun selama menunggu.

`AppModel+Run.runPreview` sendiri tidak diubah: `preview = nil` tetap berarti "tidak ada metadata hasil selama Run", yang dibaca footer, ekspor, dan sort.

### 4.3 `StoreRows.count` nol setelah release

Tabel yang bertahan memegang store lama sampai update SwiftUI berikutnya memasang yang baru (`apply` mengganti `rows`). Dulu tabel dibuang bersama panel, jadi celah itu tidak ada. Sekarang `count` mengembalikan 0 untuk store yang sudah di-`release()`, `numberOfRows` jadi 0 dan `draw` berhenti di `guard rowCount > 0`; sebelumnya `cell` yang sudah aman (mengembalikan teks kosong) tetap menggambar garis dan zebra untuk baris yang tidak ada. `FirstPaintTests.testNoRowIsDrawnFromAStoreThatWasReleasedUnderTheTable` memakai stempel `firstDraw` sebagai mata-mata: stempel itu hanya terpasang pada `draw` yang punya baris.

### 4.4 Yang tidak dikerjakan

- **Menggabungkan `columns` dan `progress`** (memanggil `pollHook` di handler `columns`). Dikerjakan dan diukur (varian B di putaran dua dan tiga): tidak lebih baik dari varian tanpa itu di kasus mana pun yang bisa dibedakan dari derau (CPU setelah `columns`, 1k × 10, dua putaran bergantian: 15,6 dan 15,6 dengan lawan 15,3 dan 18,3 tanpa). Mekanismenya juga tidak ada: hop `progress` dan `done` memang diproses sebelum update SwiftUI bila sudah antre, dan sesudahnya bila belum; hasilnya satu update dengan baris atau dua, dan biaya update-nya yang menentukan. Dibuang.
- **`displayIfNeeded()` sinkron pada pertumbuhan pertama.** `needsDisplay` dilayani commit Core Animation di akhir putaran yang sama, jadi gambar sinkron tidak memajukan apa pun, dan di dalam `viewDidMoveToWindow` atau layout ia berisiko re-entrancy. Jika X7 menunjukkan H3 (sebaran merata), perbaikannya ada di tempat lain (lihat H3).
- **Flag bench.** Tidak ada flag di kode akhir. Pembandingan lama dan baru dilakukan dengan menukar tiga berkas (`ResultGrid.swift`, `AppModel+Run.swift`, `StoreRows.swift`) dan menjalankan probe yang sama.
- **Menjeda link tampilan selama menunggu kolom.** Tabel yang dibangun lebih awal menjalankan link (≤ 60 Hz, satu panggilan FFI `row_count` per tik) selama "Running…", yang pada query panjang bisa menit. Sekitar 0,1% CPU. Tidak dijeda karena polling pertama hanya dijamin oleh `progress` atau link; layak dipertimbangkan bila energi jadi soal.

### 4.5 Stempel

W8-F0 menyediakan nama tahap; F1 menambah call site: `columns` (`AppModel+Run.swift`, hop `columns`), `gridAttached` (`GridTableView.viewDidMoveToWindow`, hanya bila ada jendela), `firstDraw` (`GridTableView.draw`, hanya bila ada baris, sebelum flush). Semuanya `onlyFirst`, dan `PerfSignposts.recording` mati di luar `--bench`, jadi biayanya satu pembacaan `Bool`. Pada Run yang memakai ulang tabel, `gridAttached` tidak terstempel: tahap yang tidak distempel dibiarkan keluar dari baris (kontrak `Stage`).

## 5. Risiko dan catatan untuk MAIN

1. **`BenchMode.fetch` mereset `tab.preview = nil` dan tidur satu detik sebelum tiap sampel** (`BenchMode.swift:1207`, juga `warmUp`, `cancel`, `rerun`). Itu melepas tabel (panel kembali ke "Press Run") sebelum tiap Run bench, jadi S1/S2 di bench mengukur "tabel dibangun saat Run" tetapi tidak "tabel hidup lintas Run". Yang kedua hanya diukur `rerun-capped` (`timedRun`, tanpa reset). A/B tetap sah, tapi untuk mengukur manfaat penuh pada Run ulang, hapus reset itu di `fetch` (berkas F0).
2. **Definisi TTFR.** `firstPaint` terstempel setelah semua hop yang sudah antre, jadi untuk S1 1k ia mencakup putaran `done` (H7) dan flush bench (H4). Perbaikan H7 di `QueryTab` (bukan di F1) menurunkan TTFR yang terukur tanpa mengubah apa yang dilihat pengguna.
3. **Tab yang berpindah.** `BottomPanel` tidak diberi `.id(tab.id)`, sehingga satu tabel dipakai tab mana pun yang tampil (sudah begitu sebelum ini). Perubahan ini menambah satu kasus: pindah ke tab yang sedang Run. `apply` memuat ulang baris karena tata letak "tanpa kolom" selalu beda dengan hasil, jadi tidak ada celah baru. Celah lama (dua tab dengan `gridRevision` dan tata letak yang sama persis) tidak disentuh.
4. **Run tanpa kolom** (DML) sekarang membangun lalu membuang tabel (sekitar 4 sampai 7 ms di main thread, saat menunggu server). Murah, tapi bukan nol.
5. **Review.** Siklus hidup tabel berubah (dibangun lebih awal, hidup lebih lama), jadi sesuai §4 W8-T2 satu putaran swift-reviewer ditambah AR. Yang perlu dilihat penelaah: veil toolbar (`.opacity` di atas `gridToolbar`, tidak di atas tabel), `StoreRows.count` untuk store yang dilepas, dan bahwa tidak ada yang membaca `tab.preview` sebagai "Run sedang jalan".

## 6. Pengujian

Baru, di `app/Tests/QueryHiveTests`:

- `FirstPaintTests.swift` (14 tes, 1 dilewati kecuali `QH_FIRST_PAINT_PROBE=1`): `StreamingEngine` (test double yang mengirim satu hop per event, dipakai tes lain) dan `HostedGrid` (`ResultGrid` di jendela sungguhan). Tabel sudah ada selagi engine bekerja; satu tabel yang sama dari mulai Run sampai baris terakhir; Run lagi memakai ulang tabel dan melepas store pertama; tabel tidak keluar dari jendelanya saat Run; spinner menutup tabel kosong dan toolbar tak terlihat; header-only setinggi header; gagal atau tanpa kolom tidak menyisakan tabel; Explain juga; store yang dilepas tidak digambar; urutan stempel; probe biaya main thread.
- `ResultGridTests.testARunAgainStartsAtTheTopWithNothingSelected`: tabel yang bertahan tidak membawa seleksi dan posisi gulir hasil sebelumnya.
- `TabCloseGridTests` (di `TabCloseTests.swift`): menutup tab saat grid tampil dan saat Run berjalan melepas store dan tabel tidak menggambar apa pun lagi.
- `StoppedRunGridTests` (di `StoppedRunTests.swift`): Stop setelah beberapa baris mempertahankan tabel dan baris parsial; Stop sebelum kolom tidak menyisakan tabel.
- `Batch7GridTests` (di `Batch7Tests.swift`): sort ke server memakai ulang tabel dan menahan base; mematikan sort mengembalikan tabel yang sama ke base tanpa query.

Struktur lama untuk A/B di MAIN: ambil `ResultGrid.swift`, `AppModel+Run.swift` dan `StoreRows.swift` dari `c66dae6` (`git show c66dae6:<path>`). Berkas stempel (`GridTableView.swift`) boleh tetap, karena stempel hanya membaca satu `Bool` di luar `--bench`. Sebagian tes baru memang gagal di struktur lama (itu tujuannya).

Gate: `cd app && swift build && swift test`: build selesai tanpa error, 948 tes, 0 gagal, 7 dilewati (semuanya mikrobenchmark yang menunggu `QH_BENCH=1`, ditambah probe `QH_FIRST_PAINT_PROBE=1`). VisualParityTests (18 tes) dan ChromeParityTests (3 tes) hijau di dalam run penuh itu; tidak ada baseline yang berubah (`git status` bersih di `__Baselines__`).
