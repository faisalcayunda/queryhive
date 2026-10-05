# Blueprint Fase 5: grid `NSTableView` dengan sel yang digambar

- **Status:** blueprint tingkat berkas, 30 Sep 2026 (W3-A1). Belum ada kode yang ditulis. Diperiksa `architect-reviewer` 30 Sep 2026: **approved with corrections**, dan koreksinya sudah ditulis ke badan dokumen (ditandai "Koreksi AR"). AR memberi verdict lagi atas implementasinya sesudah W5-T1.
- **Untuk:** W5-T1 (FR-GRID-01, FR-GRID-07 dasar, NFR-P4, P-24). Pemeriksa sesuai `development-plan.md` W5-T1: SR, TD (`ResultRows`, `CellText`), AX, UX, AR, CR, PO.
- **Sumber:** `performance-plan.md` §9 (Fase 5) dan §14 butir 9; PRD FR-GRID-01, FR-GRID-07, NFR-P4, NFR-V (§6.5), P-24; `tablepro-design-audit.md` §5; `blueprints/fase-6-data-plane.md` §17 (`StoreRows`); `development-plan.md` W5-T1 dan §7 (kepemilikan berkas); `docs/invariants.md`.
- **Prasyarat:**
  - W4-T1 (Batch 7) mendarat lebih dulu: header membaca satu sumber sort, `tab.activeSort` (`performance-plan.md` §7). Blueprint ini hanya bergantung pada bentuknya, bukan pada isinya (§12.4).
  - W4-T3 dan W5-T2 tidak dibutuhkan. `StoreRows` (W6) menyusul dan tidak boleh memblokir W5 (§3.5).
- **Bukti:** kode di pohon ini dikutip dari commit `23d3ac7`. Nomor baris `ResultGrid.swift` mengacu ke commit itu dan bergeser setelah W4-T1. Angka geometri dibaca dari sidecar JSON di `app/Tests/QueryHiveTests/__Baselines__/` (bukan ditebak dari kode). Kode TablePro (AGPL) dibaca sebagai ide: baris yang menggambar sel di level baris, elemen aksesibilitas per sel yang dibuat saat dibutuhkan, cache tampilan per baris berbatas biaya. Tidak ada kode, aset, atau string yang disalin.
- **Bahasa:** dokumen ini Indonesia. Identifier, komentar kode, pesan log, string yang tampil di app, dan nama tes Inggris.

## Ringkasan

Hari ini grid adalah `LazyVStack` SwiftUI di dalam `ScrollView` dua sumbu. Setiap `body` membangun `Array(displayedRows.enumerated())`, lalu tiap sel membaca `UserDefaults` (format kolom), mem-parse JSON (`isOpenable`), dan membuat view dengan modifier sendiri. Angka W1 (`docs/benchmarks.md`, sumbu 4): `scroll-30x1m` hitch 910 ms/s dan p99 frame 262,7 ms; `scroll-500x10k` hitch 2.953 ms/s, p99 8.468 ms, dan 2.017 ms sampai tergambar.

Rancangan ini menggantinya dengan:

1. **Seam `ResultRows`** (protokol Swift). `ArrayRows` membungkus array yang ada; `StoreRows` (W6) menyusul tanpa mengubah grid. Semua pembaca `preview.rows`/`displayedRows` di kode produksi pindah ke seam di commit 5a, dan pikselnya tidak berubah karena renderernya belum diganti.
2. **Satu `NSTableView` (`GridTableView`) yang menggambar seluruh isinya sendiri** di commit 5b: gutter, stripe, wash seleksi dan staged, titik staged, pemisah, dan teks lewat `CTLine`. Ia punya **satu** `NSTableColumn`. Geometri kolom dimiliki `GridColumnGeometry`, nilai murni yang bisa diuji.
3. **Header `GridHeaderView`** (subclass `NSTableHeaderView`) yang juga menggambar sendiri.
4. **Cache berbatas** untuk teks tampilan per baris dan `CTLine`, dan model kursor sel serta elemen aksesibilitas yang dibuat saat AppKit menanyakannya (P-24).

Satu penyimpangan dari `performance-plan.md` §9 butir 2, dan alasannya (D-1): gambar dilakukan di level tabel, bukan di `NSTableRowView` per baris. Sisanya mengikuti rencana.

Tiga temuan yang paling menentukan rancangan:

- **Sel hari ini bukan setinggi barisnya.** Kotak sel setinggi teks ditambah padding (23 pt untuk sel data, 25 pt untuk gutter), dipusatkan di baris. Di Compact (21 pt) kotak itu keluar dari barisnya sebesar 1 dan 2 pt, dan baseline mengukurnya. Baris yang menggambar dirinya sendiri tidak bisa mereproduksi itu (§1.1).
- **Dokumen setinggi 25 juta pt.** 1 juta baris × 25 pt melewati 2^24, tempat presisi Float32 di compositor Core Animation turun ke 2 pt. **Koreksi AR:** di W5 batas ini tidak tercapai. Plafon produk 200.000 baris sampai Fase 6 (O-12, `AppModel.productRowLimitCeiling`) berarti paling tinggi 6 juta pt, dan fling `scroll-30x1m` hanya menempuh sekitar 9.000 pt dari atas (3 s × 3.000 pt/s, `BenchMode.swift`). P-1 karena itu bukan gerbang 5b, melainkan gerbang W6-T1 sebelum plafon naik ke 5 juta. 5b hanya wajib tidak menutup jalan mitigasinya (§5.4).
- **Teks adalah satu-satunya lapisan gate visual yang tidak bisa dijamin di atas kertas.** Geometri dan warna harus persis. Teks harus ≤ 0,1% piksel berbeda. Rancangan menyediakan probe dan tangga penyetelan, dan bila tetap gagal, berhenti dan melapor, bukan merekam ulang baseline (§14).

## 1. Temuan di kode yang mengubah rencana

### 1.1 Geometri sel yang diukur baseline

Dari sidecar `grid-kinds{,-compact,-tall,-nonum}-dark.json` (1x, sRGB) dan `ResultGrid.swift:702-739`. **Koreksi AR:** angka ini dari baseline `23d3ac7`. W4-T1 merekam ulang V-1 (banner sort) untuk `grid-sorted-*` dan kesepuluh `grid-kinds*` (scene itu juga bersort), jadi `bodyTop` dan tinggi banner harus dibaca ulang dari sidecar pasca-W4-T1 sebelum P-2. Geometri sel dan kolom di bawah tidak disentuh V-1.

| Ukuran | Nilai | Asal |
|---|---|---|
| Tinggi baris | 21 / 25 / 30 (`DataPreferences.RowHeight.points`) | `metrics.rowHeight` (stripe pitch) |
| Kotak sel data | tinggi 23 = `mono12` line 15 + padding vertikal 4+4 | separator data berhenti 1 px di atas dasar baris terakhir (normal: 396 vs 397) |
| Kotak gutter | tinggi 25 = `code(10.5)` line 13 + padding vertikal 6+6 | separator gutter berhenti tepat di dasar (397) |
| Header (tanpa banner) | 50 pt termasuk garis bawah 1 pt | `bodyTop` 76 dan aturan horizontal 25 dan 75; marker header memakai `bodyTop - 50` |
| Banner sort | 21 pt | `bodyTop` 97 dibanding 76 di scene bersort |

Kotak sel dipusatkan di baris (`HStack` dengan `.frame(height: rowHeight)`). Akibat yang harus direproduksi persis karena gate layout menuntut angka yang sama:

- **Normal (25):** celah 1 pt di atas dan bawah tiap sel data. Garis pemisah vertikal putus-putus 23 on / 2 off. Baseline menyimpan ini.
- **Tall (30):** celah 3,5 dan 2,5 pt. Dasar separator terukur 454 (data) dan 455 (gutter) untuk baris terakhir yang berakhir di 457: dasar kotak 453,5 dan 454,5 dibulatkan ke atas. **Aturan snap: tepi atas dan bawah masing-masing dibulatkan `.rounded()` (setengah menjauhi nol), lalu tinggi = selisihnya.** Ini dibaca dari angka, bukan dari kode SwiftUI.
- **Compact (21):** kotak 23 dan 25 lebih tinggi dari baris. Separator gutter berakhir di 351 = dasar baris (349) + 2, data di 350 = 349 + 1. Kotak baris N menutupi 1 pt baris N−1 dan N+1 (2 pt untuk gutter), dan urutan cat SwiftUI (baris N+1 dicat sesudah N) menentukan warna di pita tumpang tindih.
- Overlay sel diterapkan pada kotak berpadding: titik staged 4×4 pt di pojok kanan atas kotak dengan jarak 3 pt, separator 1 pt di tepi kanan kotak.

Ini alasan D-1: satu `draw(_:)` yang mengecat baris berurutan naik ke satu konteks otomatis punya urutan cat dan luapan yang sama dengan SwiftUI. Sebuah `NSTableRowView` terpotong di batas barisnya, dan luapan Compact pada baris terakhir jatuh di luar semua row view.

### 1.2 Yang dihitung ulang setiap `body`

- `naturalWidths` (`:82-96`) memindai `preview.rows.prefix(200)` × semua kolom, dan `widths(fitting:)` dipanggil setiap layout. Aturan yang harus dipertahankan: sampel adalah **200 baris pertama hasil yang diambil** (urutan server, sebelum filter dan sort), `value.count` dalam Character, nilai NULL dihitung 4, header dihitung dari label yang sudah di-rename.
- `ForEach(Array(displayedRows.enumerated()), id: \.offset)` (`:294`) membuat array tuple sebesar hasil (1 juta) tiap `body`.
- Tiap sel membaca `ColumnFormatStore.format` (`UserDefaults.dictionary`, `:762-765`), memanggil `GridValue.isOpenable` (parse JSON hingga 100.000 unit untuk `varchar` berawalan `{`, `:780`), dan `ColumnFormat.render`. Untuk format `unix_timestamp`, `DateFormatter` dibuat per panggilan (`ColumnFormat.swift`, `timestamp`).
- Tooltip `.help(value)` dipasang pada setiap sel teks tak-kosong (`:1043`). NULL dan `∅` tidak punya tooltip.

### 1.3 Perilaku yang mudah terlewat (harus tetap)

1. Klik kanan tidak mengubah seleksi. `Edit Cell…` dan `View Value…` bekerja atas sel kiri-atas seleksi (`:534-537`, `:950`).
2. Klik pertama pada sel memilihnya (`DragGesture(minimumDistance: 0)`). Klik kedua (`onTapGesture(count: 2)`): sel openable membuka pembaca bila panel inspektur tidak berdiri, dan **tidak melakukan apa pun** bila panel berdiri (`:790`); sel yang tidak openable membuka editor. (Koreksi AR: versi awal menulis "selain itu membuka editor", yang akan membuka editor di sel JSON saat panel berdiri.)
3. Drag memetakan baris lewat jarak tempuh pointer dibagi tinggi baris, dijepit ke [0, terakhir], tanpa autoscroll (`GridGeometry.cell`, `CellSelection.swift`). Kolom dijepit ke kolom terdekat, gutter dan ruang kosong di kanan berarti "sampai tepi".
4. `beginEdit` pada sel lain mengakhiri sesi yang terbuka lebih dulu (`:921`); klik di luar tanpa memulai edit lain **tidak** membatalkan (komentar di `:38` mengklaim sebaliknya, kodenya tidak). Blueprint mengikuti kode.
5. Salinan dibangun dari kolom **sumber** yang tercakup seleksi, diurutkan menurut sumber, dengan nama server (`:502-518`). Bagian ini tidak punya tes hari ini (§13, butir 3).
6. `isNumeric` (`:1174`) memakai `contains("int")`, jadi `point`, `interval`, dan `hstore` ikut rata kanan dan bertinta mint. Daftar `isNumeric` kedua di `VisualParityTests.swift:576` (`int`, `double`, `float`, `numeric`, `decimal`, `real`) berbeda dari daftar grid (ada `long`, `bigint`, `smallint`, `tinyint`). Paritas piksel menuntut kuirk ini dipertahankan; dicatat ke backlog, bukan diperbaiki di fase performa.
7. Chip tipe bisa lebih lebar dari kolomnya (`timestamp with time zone`). SwiftUI mungkin membungkusnya dan menaikkan header; **tidak terbukti**, baseline tidak memuat kasusnya (kolom itu tersembunyi di `grid-kinds`). Probe P-3 memutuskan.
8. Banner sort ada di dalam baris header dan ikut tergeser horizontal (`:563-587`).

### 1.4 Hal lain

- `AppModel.runPreview` mengganti `tab.preview` utuh tiap 200 ms selama streaming (`AppModel.swift:2365-2369`), dan tiap penggantian menaikkan `gridRevision` dan menghapus sort. Fase 5 tidak mengubah kadensnya (itu W6, §17.3 blueprint Fase 6). Tabel harus tahan diganti utuh 5 kali per detik dengan 1 juta baris.
- Rencana menyebut 28 pemakai di `ResultGrid.swift`, `QueryTab.swift`, dan `Snapshot.swift`. Hitungan sebenarnya juga 28, tetapi di berkas lain: `ResultGrid.swift` 18, `QueryTab.swift` 7, `Panels.swift` 1, `BenchMode.swift` 2. `Snapshot.swift` hanya **menulis** (`tab.preview?.rows[1][6] = …`, tujuh baris di `:606-625`) dan tidak berubah (§3.4). **Koreksi AR:** daftar ini potret `23d3ac7`. W4-T1 menambah pembaca baru (antara lain penyimpanan "hasil dasar ≤ 10.000 baris" untuk sort "off", `fase-6` §17.5), dan ia mendarat lebih dulu. Karena itu kriteria selesai 5a bukan tabel §3.3, melainkan aturan grep: setelah 5a, `/usr/bin/grep -rnE "preview(\?|!)?\.rows|displayedRows" app/Sources` hanya boleh menemukan definisi penyimpan di `QueryTab.swift` (`displayedRows` dan badannya), `PreviewResult.rowCount`, penulis di `AppModel.swift`, `Snapshot.swift`, dan `BenchMode.swift` (`tab.preview = …`), serta komentar. Tes (`VisualParityTests`, `GridColumnsTests`, `ResultGridTests`, `GridBenchTests`, `PanelDefaultTests`, `StoppedRunTests`) tetap membaca array (D-7).
- Nama bench di rencana (`scroll-1m`, `scroll-500c`, `open-500c`) sudah diganti di kode menjadi `scroll-30x1m`, `scroll-500x10k`, `open-500x10k` (`BenchMode.swift:205-207`). `BenchMode` sudah mencari `NSScrollView` terbesar dan menunggu `PerfSignposts.firstPaint`, jadi tabel baru bisa diukur tanpa mengubah driver.
- `Support/Snapshot.swift` dan `VisualParityTests` menangkap dengan `cacheDisplay` pada `NSHostingView` di jendela `.titled`, bukan `ImageRenderer`. `NSViewRepresentable` ikut tergambar, dan `settle` menunggu enam frame identik. Probe P-4 memastikannya sebelum 5b.

## 2. Keputusan desain

| ID | Keputusan | Alasan |
|---|---|---|
| D-1 | **Gambar di `GridTableView.draw(_:)`, bukan di `NSTableRowView`.** Tabel mode sel (bukan view-based), `selectionHighlightStyle = .none`, `draw(_:)` di-override tanpa memanggil `super`. `GridRowPainter` (struct murni di `Views/GridRowView.swift`) yang mengecat; baris `r-1…r+1` diikutkan bila kotak sel meluap. | §1.1: luapan Compact dan urutan cat hanya bisa direproduksi bila baris berurutan dicat ke satu konteks. Tidak ada `NSView` per baris (tanpa alokasi saat scroll, tanpa backing store selebar 75.000 pt per baris pada 500 kolom). Penyimpangan dari `performance-plan.md` §9.2; nama berkas `GridRowView.swift` dipertahankan supaya kepemilikan W10-T2 tidak berubah. **Fallback bila P-4 gagal:** `GridRowView: NSTableRowView` memanggil painter yang sama, dengan tambahan mengecat luapan tetangga (§6.6). |
| D-2 | **Satu `NSTableColumn`** selebar jumlah kolom, `minWidth` 1 dan `maxWidth` `.greatestFiniteMagnitude` (bawaan 1000 akan menjepit). `GridColumnGeometry` menyimpan lebar, offset kumulatif, dan `column(atX:)` (pencarian biner). | Tidak ada resize, reorder, atau autoresize hari ini. 501 objek `NSTableColumn` dan sinkronisasi lebarnya (TablePro mengukurnya kuadratik) tidak membeli apa pun. Gate 500 kolom ≤ 30 ms jadi bebas dari biaya itu. |
| D-3 | **Seam `ResultRows` bergaya `StoreRows`, tanpa state format:** `cell(row:column:format:)` menerima format per panggilan. | Tidak ada protokol invalidasi. `StoreRows` menurunkan `formatSignature` dari argumen. Selisih terhadap `fase-6` §17.2 dicatat di §17.3 untuk W6-A1. |
| D-4 | **`CellText.text` adalah prefix teks tampilan, baris pertama saja, paling banyak 1.024 unit UTF-16.** Flag: `null`, `empty`, `truncated`, `numeric`, `openable`. Penggambar tidak membuat keputusan dari `numeric` dan `openable` (perataan kanan mengikuti tipe kolom, demi paritas). | Nilai 10 MB tidak pernah menjadi `String` per sel di jalur gambar. 1.024 unit × 7,2 pt lebih lebar dari kolom mana pun. `openable` hanya petunjuk; tindakan dobel-klik tetap memanggil `GridValue.isOpenable` penuh, jadi perilakunya sama dengan hari ini. **Koreksi AR:** potongan berhenti di batas `Character`, tidak pernah di tengah pasangan surrogate atau klaster. `numeric` dan `openable` tidak punya pemakai di W5 (kontraknya untuk W6), jadi `ArrayRows` mengisinya semurah mungkin: `numeric` dari tipe kolom, `openable` dari tipe atau karakter bukan-spasi pertama `{`/`[` tanpa alokasi. `GridValue.looksLikeJSON` tidak dipakai di sini karena ia memangkas seluruh string (salinan 10 MB per sel). |
| D-5 | **Pembacaan format dan `isOpenable` keluar dari jalur gambar.** Format dibaca sekali per kolom per perubahan lewat notifikasi `ColumnFormatStore.didChange` (diposting oleh `set`). | Menghapus `UserDefaults` per sel dan parse JSON per render. |
| D-6 | **`RowReading` kecil** (`func row(at:) -> [String?]?`) dipakai `WritePlan`, `UpdateStatements`, dan `CellEdits.fill/paste`. `Array<[String?]>` ikut memenuhinya, `ResultRows` mewarisinya. | Tes yang memanggil dengan array tidak berubah. "`WritePlan.build` menerima accessor" terpenuhi tanpa menyentuh tes. |
| D-7 | **`QueryTab.displayedRows` (array) tetap sampai W6**, sebagai penyimpan `ArrayRows`. Semua pembaca produksi lewat `tab.result`. Tes yang membaca `displayedRows` atau `preview.rows` (`VisualParityTests`, `GridColumnsTests`, `ResultGridTests`, `GridBenchTests`, `PanelDefaultTests`, `StoppedRunTests`) tidak berubah di W5. | `fase-6` §17.5 menghapusnya dan menulis ulang tes itu di W6-T1. Menyentuhnya dua kali tidak ada gunanya. |
| D-8 | **Perilaku pointer dan menu hidup di `Coordinator` sebagai metode biasa** (`press`, `drag`, `release`, `menuItems`), yang dipanggil `mouseDown` dan kawan-kawannya. | Tes memanggilnya tanpa `NSEvent`. |
| D-9 | **Editor sel adalah `NSTextField` overlay** di dalam tabel, dengan `typeCellEdit` per perubahan teks dan `endCellEdit` saat Return, persis jalur `typeCellEdit`/`endCellEdit` yang ada. | Penggabungan undo tetap satu langkah per sesi (`CellEditUndoTests`). |
| D-10 | **Aksesibilitas dibuat saat ditanya.** Elemen sel adalah `NSAccessibilityElement` (bukan `NSView`), dibuat hanya ketika AppKit memanggil `accessibilityChildren`/`accessibilityRows` pada tabel, dan hanya untuk baris dan kolom yang terlihat. Notifikasi baru diposting setelah pertanyaan pertama itu. | Efek yang dikehendaki audit §5 ("dipasang hanya setelah klien AX menempel") tanpa mendeteksi klien dan tanpa satu objek pun bila VoiceOver mati. |
| D-11 | **Kursor sel dimodelkan sekarang, tidak digambar dan tidak diikat ke tombol** (P-24). `GridCursor` (jangkar dan fokus) di `CellSelection.swift`, `QueryTab.cellCursor`. Ia hanya dipakai AX (fokus VoiceOver) dan disiapkan untuk W10-T1. | Fase performa tidak menambah fitur; V-9 dicadangkan untuk W10. |
| D-12 | **Cache berbatas dan dua generasi:** teks tampilan per baris (viewport ± 2 halaman, ≤ 16 MB) dan `CTLine` (≤ 8 MB, dua peta `current`/`previous`). Prebuild `CTLine` di luar main **hanya** dipertahankan bila bench membuktikan manfaat ≥ 10% (§9.4). | Satu baris baru per frame di 3.000 pt/s hanya butuh puluhan `CTLine` per frame. Kode yang tidak menyelamatkan gate tidak dimasukkan. |
| D-13 | **Banner sort tetap SwiftUI, di atas tabel**, dengan latar header yang sama. Ia tidak lagi ikut tergeser horizontal. **Koreksi AR, bersyarat:** ini hanya berlaku bila banner V-1 hasil W4-T1 berisi konten yang terjangkar kiri saja (tanpa `Spacer` yang mendorong tombol ke kanan). Di `grid-sorted` dokumen lebih lebar dari 1000 pt, jadi elemen yang terjangkar kanan akan pindah dari tepi dokumen ke tepi panel dan mengubah piksel. Bila syarat itu tidak terpenuhi, atau G-VIS pada `grid-sorted-*` dan `grid-kinds*` pasca-W4-T1 tidak hijau, banner di-host sebagai `NSHostingView` selebar ukuran pasnya di bagian atas `GridHeaderView`. Header menggambar pita `recess 0.30` selebar dokumen, dan banner ikut tergeser seperti hari ini. | Menggambar kalimat dengan tombol di dalam `NSView` tidak membeli apa pun. Pada offset scroll 0 pikselnya sama bila syarat di kiri terpenuhi. Perbedaan perilakunya (kalimat tetap di kiri pada hasil yang lebar) bukan perubahan piksel baseline, jadi tidak butuh kode V, tetapi harus tercatat di `performance-plan.md` §14 butir 9 (§14.3). |
| D-15 | **Koreksi AR: fokus dan keyboard.** Klik pada tabel menjadikannya first responder (bawaan `NSTableView`), sehingga ⌘C sampai ke `copy(_:)` tabel. Seleksi baris milik `NSTableView` dimatikan: delegate `selectionIndexesForProposedSelection` mengembalikan himpunan kosong, `selectAll(_:)` di-override tanpa efek dan divalidasi nonaktif, dan panah atas/bawah/kiri/kanan tidak diteruskan ke `super` (W10-T1 yang mengikatnya). Page Up/Down, Home, dan End tetap menggulir. `shouldShowCellExpansionFor` mengembalikan `false` (tooltip ekspansi mode sel). | Tanpa ini, panah dan ⌘A mengubah seleksi baris `NSTableView` yang tidak digambar tetapi diumumkan AX. Fokus yang pindah dari editor ke grid adalah perubahan perilaku yang terlihat meski piksel sama (§14.3 butir 6). |
| D-14 | **Popover filter, pembaca sel, dan lembar SwiftUI lain tetap SwiftUI**, dihosting di `NSPopover` lewat `NSHostingController` dan dijangkarkan ke rect sel atau funnel. Lembar (`RenameColumnSheet`, `SavePresetSheet`, `ChangeReview`) tidak berubah. | Menjaga permukaan diff kecil. `model.filterPopoverColumn` tetap satu-satunya pintu, karena scene snapshot mengisinya. |

## 3. Seam `ResultRows`

### 3.1 Bentuk

```swift
// Models/ResultRows.swift
protocol RowReading {
    /// The whole fetched row as the server sent it, or nil past the end.
    func row(at index: Int) -> [String?]?
}
extension Array: RowReading where Element == [String?] {
    func row(at index: Int) -> [String?]? { indices.contains(index) ? self[index] : nil }
}

struct CellFlags: OptionSet, Sendable {
    let rawValue: UInt8
    static let null = CellFlags(rawValue: 1), empty = CellFlags(rawValue: 2),
               truncated = CellFlags(rawValue: 4), numeric = CellFlags(rawValue: 8),
               openable = CellFlags(rawValue: 16)
}

struct CellText: Equatable, Sendable {
    /// First line of the display text (format applied), at most `prefixLimit` UTF-16 units.
    var text: String
    var flags: CellFlags
    static let prefixLimit = 1_024
}

protocol ResultRows: AnyObject, RowReading {
    /// Rows the grid draws: after filter, search and sort.
    var count: Int { get }
    /// Rows fetched before filter and search (the footer's "12 of 40").
    var fetched: Int { get }
    /// Source order, as the engine sent them.
    var columns: [Event.Column] { get }

    /// Display text of one cell. Main thread only: `StoreRows` fills pages from here.
    func cell(row: Int, column: Int, format: ColumnFormat) -> CellText
    /// The whole value under `format`; `.raw` gives what an edit, a copy and an export see.
    func fullValue(row: Int, column: Int, format: ColumnFormat) -> String?
    /// Raw values for copy and write plans, in the order of `columns` (source indices).
    func rows(in range: Range<Int>, columns: [Int]) -> [[String?]]
    /// Per source column, the widest of the first 200 *fetched* rows in Characters, capped at 64
    /// (a count of 42 or more already hits the 320 pt clamp), NULL counting 4.
    func naturalCharCounts() -> [Int]
    /// Distinct values of a column over the fetched rows, in first-seen order, empty past
    /// `ColumnFilter.valuePickerLimit`. Synchronous now; W6 makes it `async`.
    func distinctValues(column: Int) -> [String?]
}
```

`fetched` dan `count` memakai nama `fetched` dan `count` yang sama dengan `StoreRows` di `fase-6` §17.2. `row(at:)` punya implementasi bawaan lewat `rows(in:columns:)` untuk `StoreRows`; `ArrayRows` menimpanya langsung.

### 3.2 `ArrayRows`

```swift
final class ArrayRows: ResultRows, @unchecked Sendable {
    init(rows: [[String?]], sizing: [[String?]], columns: [Event.Column])
}
```

- `rows` adalah `tab.displayedRows` (sudah difilter dan disortir), `sizing` adalah `preview.rows` (urutan server). Keduanya array Swift yang copy-on-write: tidak ada salinan.
- Tidak berubah setelah dibuat, jadi aman dibaca dari antrean lain (untuk prebuild §9.4). `naturalCharCounts()` menghitung sekali dan menyimpannya.
- `cell` membangun prefix: `format.render` (kecuali `.raw`), potong di pemisah baris pertama (`\n`, `\r`, U+2028, U+2029), lalu di `prefixLimit`; `truncated` bila ada sisa. **Aturan baris pertama ditetapkan oleh probe P-3** terhadap `Text.lineLimit(1)` hari ini, dan `GridParityTests` menguncinya.
- `openable`: `GridValue.isStructured/isBinary` untuk tipe, atau karakter bukan-spasi pertama `{`/`[` yang dipindai tanpa alokasi; tidak ada parse (D-4).

`QueryTab` mendapat:

```swift
/// What the grid draws; rebuilt only when `gridRevision` changes.
var result: any ResultRows      // ArrayRows over displayedRows; an empty one without a preview
```

Disimpan dengan pola cache `displayedCache` yang ada (`@ObservationIgnored`, kunci `gridRevision`). `PreviewResult` mendapat `var rowCount: Int { rows.count }`, dipakai pembaca non-grid (§3.3).

### 3.3 Pembaca yang pindah (commit 5a)

Semua acuan ke `23d3ac7`.

| Berkas:baris | Bentuk hari ini | Pindah ke |
|---|---|---|
| `ResultGrid.swift:89` | `preview.rows.prefix(200)…` (lebar alami) | `result.naturalCharCounts()` lewat `GridMetrics.naturalWidths` (§10) |
| `:157` | alias `displayedRows` | dihapus; `tab.result` |
| `:167` | `GridPlaceholder.whenEmpty(shown: displayedRows.count, fetched: preview.rows.count, …)` | `result.count`, `result.fetched` |
| `:294` | `ForEach(Array(displayedRows.enumerated()))` | dihapus di 5b (di 5a: `0..<result.count` dengan `result.row(at:)` dan `format.render` seperti hari ini, **bukan** `result.cell`; koreksi AR: `CellText` membawa aturan baris pertama dan batas 1.024 yang baru diputuskan P-3, dan memakainya di renderer SwiftUI bisa mengubah piksel di 5a) |
| `:379`, `:402` | `preview.rows.count`, `displayedRows.count` | `result.fetched`, `result.count` |
| `:511` | `displayedRows[row]` di `selectionText` | `result.rows(in:columns:)` lewat `GridClipboard.text(result:…)` (§8.7) |
| `:583` | `tab.preview?.rows.count` (banner) | `result.fetched` |
| `:975` | `WritePlan.build(rows: displayedRows, …)` | `rows: tab.result` (`RowReading`) |
| `:988`, `:991` | `!displayedRows.isEmpty`, `lastRow: displayedRows.count - 1` | dihapus di 5b (di 5a: `result.count`) |
| `:1058`, `:1060`, `:1065`, `:1068`, `:1069` | `summaryText` | `result.fetched`, `result.count` |
| `:1113` | `ColumnFilter.distinctValues(in: preview.rows, column:)` | `result.distinctValues(column:)` |
| `:1154` | `tab.preview?.rows.count` | `result.fetched` |
| `QueryTab.swift:752`, `:759` | `cellEdits.fill/paste(rows: displayedRows, …)` | `rows: result` (`RowReading`) |
| `QueryTab.swift:784-785` | `fetchedValue(at:)` membaca `displayedRows` | `result.fullValue(row:column:format: .raw)` |
| `QueryTab.swift:829-851` | definisi `displayedRows` | tetap sebagai penyimpan; ditambah `result` |
| `Panels.swift:94` | `tab.preview.map { $0.rows.count }` | `tab.preview.map(\.rowCount)` |
| `BenchMode.swift:755`, `:759` | `tab.preview?.rows.count` | `tab.preview?.rowCount` |

Penulis yang **tidak** berubah di W5: `AppModel.swift` (enam `PreviewResult(rows:)`), `Snapshot.swift` (tujuh penulisan langsung ke `preview.rows`), dan tes yang membangun `PreviewResult`. Semuanya ikut hilang atau berubah di W6-T1.

### 3.4 `WritePlan`, `UpdateStatements`, `CellEdits`

`WritePlan.build(edits:rows:columns:table:kind:budget:)`, `UpdateStatements.generate(edits:rows:…)`, `CellEdits.fill(_:over:rows:columns:)`, dan `paste(_:at:rows:columnCount:columns:)` menerima `some RowReading`. `rows.indices.contains(row)` menjadi `rows.row(at: row) != nil`, dan `rows[row]` menjadi `rows.row(at: row)`. Perilaku, urutan, dan pesan tidak berubah; `WritePlanTests`, `CellEditsTests`, `WriteBatchBudgetTests` lulus tanpa disentuh.

### 3.5 Yang W6 harus tahu

`StoreRows: ResultRows` menambah `poll()`, `apply(_:)`, `release()`. Kontrak yang dijaga:

- `cell` dan `fullValue` **hanya di main** (cache halaman `StoreRows` dimutasi di main). Prebuild memanggil `cell` di main lalu membawa nilai `CellText` (Sendable) ke antrean lain (§9.4).
- Tidak ada metode yang melempar galat. `StaleHandle` dan `Superseded` menjadi hasil kosong.
- `count` boleh naik antar frame; tabel menyediakan `Coordinator.rowsDidGrow()` yang dipanggil W6 dari display link (tidak ditulis di W5).
- (Koreksi AR.) Indeks baris di seam selalu indeks **hasil**. Tabel menerjemahkannya hanya lewat `Coordinator.resultRow(forTableRow:)` dan kebalikannya (§5.4), supaya jendela baris W6 bila P-1 gagal tidak menyentuh `ResultRows`, seleksi, maupun antrean edit.

## 4. Geometri: `Models/GridMetrics.swift`

Nilai murni, tanpa AppKit, semuanya diuji.

```swift
enum GridMetrics {
    static let cellPadding: CGFloat = 8
    static let gutterContent: CGFloat = 44
    static func gutterWidth(showRowNumbers: Bool) -> CGFloat        // 60 or 0
    /// Content width per drawn column (no padding), the formula of ResultGrid.swift:94.
    static func naturalWidths(headerCounts: [Int], sampleCounts: [Int]) -> [CGFloat]
    /// Slack shared out in proportion, gutter subtracted (ResultGrid.swift:101-110).
    static func fitted(_ natural: [CGFloat], available: CGFloat, gutter: CGFloat) -> [CGFloat]
    static func isNumeric(type: String) -> Bool                     // the current contains-list, quirk kept
    static func lineHeight(of font: NSFont) -> CGFloat              // ceil(ascent + descent + leading)
    /// Box a cell occupies inside its row, snapped the way §1.1 measured.
    static func box(inRow row: CGRect, contentHeight: CGFloat, verticalPadding: CGFloat) -> CGRect
    static func headerHeight(labelLine: CGFloat, chipLine: CGFloat) -> CGFloat   // 6 + l + 3 + (c + 6) + 6
}
```

- Rumus lebar tidak berubah: `min(max(n × 7,2 + 20, 84), 320) + 22`, dengan `n = max(jumlah karakter header, terpanjang di sampel)`, ditambah padding 16 saat digambar (`GridColumnGeometry` menyimpan lebar penuh). Sisa ruang dibagi proporsional, gutter dikeluarkan lebih dulu.
- `lineHeight` adalah **hipotesis** yang harus menghasilkan 15 (`mono12`), 13 (`code(10.5)`), dan tinggi header 50 untuk font sistem. Tes `GridMetricsTests.testTheBoxesAreTheSizeTheBaselinesMeasured` mengunci 23, 25, dan 50; bila hipotesis salah, probe P-2 mengganti rumus (`floor`, `round`) sampai ketiganya cocok. Tinggi header dan kotak turun dari metrik font, bukan konstanta, supaya font kode pilihan pengguna tetap menggeser header seperti hari ini.
- `box` menghitung tepi atas dan bawah terpisah dengan `.rounded()`, dan boleh menghasilkan kotak lebih tinggi dari baris (Compact).

```swift
struct GridColumnGeometry: Equatable {
    init(gutter: CGFloat, widths: [CGFloat])       // widths include the 2 × cellPadding
    var totalWidth: CGFloat
    func edges(of position: Int) -> (left: CGFloat, right: CGFloat)   // snapped, position = drawn column
    func column(atX x: CGFloat) -> Int?            // nil in the gutter
    func clampedColumn(atX x: CGFloat, last: Int) -> Int   // the drag's rule: gutter -> 0, past the end -> last
    func columns(in range: ClosedRange<CGFloat>) -> Range<Int>
    func row(atY y: CGFloat, rowHeight: CGFloat, count: Int) -> Int   // clamped to [0, count - 1]
}
```

`GridGeometry` (`CellSelection.swift`) dan enam tesnya diganti: `column(at:widths:last:)` menjadi `clampedColumn(atX:last:)`, dan `cell(at:inRow:…)` (yang memetakan jarak tempuh dari baris awal) menjadi `row(atY:…)`, karena koordinat tabel sudah absolut. Jumlah tes tidak turun (NFR-Q).

## 5. `GridTableView`

### 5.1 Konfigurasi

Subclass `NSTableView` di `Views/ResultGridTable.swift`. Tidak ada `viewFor`, dan tidak ada `NSView` per baris.

```swift
rowSizeStyle = .custom                                  // rowHeight is ours, not the system size
rowHeight = DataPreferences.shared.rowHeight.points     // fixed
usesAutomaticRowHeights = false
intercellSpacing = .zero                                // the default (3, 2) would shift every row
gridStyleMask = []
usesAlternatingRowBackgroundColors = false
selectionHighlightStyle = .none
allowsColumnReordering = false;  allowsColumnResizing = false
allowsColumnSelection = false;   allowsMultipleSelection = false;  allowsTypeSelect = false
columnAutoresizingStyle = .noColumnAutoresizing
focusRingType = .none
style = .plain
backgroundColor = .clear                                // the panel behind shows, as today
headerView = GridHeaderView(...)                        // height from GridMetrics.headerHeight
cornerView = nil                                        // no header-styled square over a legacy scroller
```

`NSScrollView`: `drawsBackground = false`, `borderType = .noBorder`, `hasVerticalScroller`/`hasHorizontalScroller` true, `automaticallyAdjustsContentInsets = false`, `contentInsets = .zero`. Klip: `drawsBackground = false`. **Koreksi AR:** `copiesOnScroll` dihapus dari rancangan. Properti itu usang sejak macOS 11 dan diabaikan di hierarki berlapis layer; AppKit sudah hanya menggambar area yang baru terbuka. Delegate juga memasang aturan D-15 (seleksi baris kosong, tanpa tooltip ekspansi).

`dataSource` hanya `numberOfRows(in:)` dan `tableView(_:objectValueFor:row:)` yang mengembalikan `nil` (mode sel mewajibkannya untuk AX; tidak dipakai menggambar).

Header tetap tergeser horizontal bersama isi dan tetap di atas saat scroll vertikal (bawaan `NSTableHeaderView` di `NSScrollView`), sama dengan header `pinnedViews` hari ini. Gutter ikut tergeser (tidak di-pin), demi paritas.

### 5.2 `draw(_:)`

1. `rows = firstRow…lastRow` yang memotong `dirtyRect`, diperluas `overflow` baris di kedua sisi, dengan `overflow = ceil((boxHeight - rowHeight) / 2 / rowHeight)` (0 kecuali Compact).
2. `columns = geometry.columns(in: dirty.minX…dirty.maxX)`; gutter ikut bila `dirty.minX < gutter`.
3. Untuk tiap baris naik: `GridRowPainter.paint(row:columns:in:)` (§6). Klip konteks hanya `dirtyRect`.
4. Ekor: bila `overflow > 0` dan baris terakhir tampil, pita di bawah baris terakhir dicat oleh loop yang sama, karena `draw(_:)` mencat seluruh bounds tabel dan bukan per baris (ini yang tidak bisa dilakukan row view).
5. `PerfSignposts.firstPaint()` dipanggil sekali saat baris 0 dicat (menggantikan `.onAppear` baris 0, `ResultGrid.swift:297`).

### 5.3 Invalidasi

Tidak ada `reloadData()` kecuali hasil berganti. Aturan (`GridPaintDiff`, murni dan diuji):

- **Seleksi berubah:** baris dalam selisih simetris kedua rentang, seluruh lebar terlihat; untuk baris di irisan, hanya kolom di selisih simetris rentang kolom. Tiap rect diperluas `overflow` baris.
- **Staged berubah:** rect sel untuk kunci yang berbeda antara `CellEdits` lama dan baru (`values`), ditambah entri cache `CTLine` sel itu dibuang.
- **Sort, funnel, rename:** hanya header (`headerView.needsDisplay`); sort mengganti hasil sehingga isi ikut (lihat di bawah).
- **Hasil berganti** (`gridRevision` berubah): cache teks dan `CTLine` dikosongkan, `noteNumberOfRowsChanged()`, `needsDisplay = true`. Posisi scroll dipertahankan, seperti hari ini. Kolom berganti (layout, sembunyikan, pindah, rename, jumlah kolom) membangun ulang `GridColumnGeometry` dan lebar `NSTableColumn` (satu panggilan).
- **Gaya berganti** (tinggi baris, teks NULL, baris selang-seling, nomor baris, font kode, aksen, appearance): cache dikosongkan, geometri dibangun ulang, `needsDisplay`.

### 5.4 Scroll dan dokumen besar

Tinggi dokumen = `rows × rowHeight`. **Koreksi AR, diputuskan:**

- **Mana yang nyata.** Geometri AppKit memakai `CGFloat` = `Double`. `rect(ofRow:)`, `row(at:)`, konversi koordinat, hit-test, dan `draw(_:)` ke dalam tile tetap persis jauh di atas 5 juta baris. Yang berisiko hanya komposisi di compositor Core Animation (WindowServer), yang menyimpan posisi dan bounds layer sebagai float tunggal. Di atas 2^24 pt, offset scroll yang sampai ke layar terkuantisasi 2 pt, lalu 4 pt di atas 2^25, 8 pt di atas 2^26, dan 16 pt di atas 2^27 (5 juta × 30 pt = 150 juta pt). Asal tile kelipatan 256 pt, jadi masih persis, dan gejala yang diharapkan adalah isi yang melompat per 2 sampai 16 pt dan tidak lagi sejajar dengan hit-test, bukan jahitan antar-tile. Ini dugaan yang harus diukur, bukan fakta.
- **Kapan tercapai.** Tidak di W5: plafon produk 200.000 baris (O-12) × 30 pt = 6 juta pt, dan bench `scroll-30x1m` hanya menempuh sekitar 9.000 pt dari atas. P-1 tidak memblokir 5b. Ia menjadi gerbang **W6-T1 sebelum plafon naik ke 5 juta** (§15).
- **Yang wajib di 5b supaya mitigasi tetap murah.** Setiap konversi baris tabel ke baris hasil lewat satu fungsi, `Coordinator.resultRow(forTableRow:)` dan kebalikannya. Di W5 fungsinya identitas. Seleksi, `CellKey`, kursor, AX, tooltip, dan popover menyimpan baris **hasil**, tidak pernah indeks baris `NSTableView`.
- **Mitigasi minimal bila P-1 gagal:** jendela baris. Tabel menampilkan paling banyak `floor(2^24 / rowHeight)` baris (798.915 pada 21 pt, 671.088 pada 25 pt, 559.240 pada 30 pt) mulai dari `base`. Sebuah pembungkus `WindowedRows: ResultRows` di atas hasil memberi `count` dan `cell` bergeser `base`, dan satu baris status di atas footer ("Rows 1–671,088 of 5,000,000", "Next block") menggeser `base`, sama dengan "Ke baris…" (UC-04). Pemetaan di atas hanya menambah `base`. Ukurannya S sampai M, dan painter tidak berubah.
- **Yang ditolak:** menjepit tinggi baris (5 juta baris butuh < 3,4 pt), dan virtualisasi scroll dengan pemetaan knob non-linear sebagai bawaan. Virtualisasi merusak proporsi scroller, indeks baris AX, dan kesetaraan hit-test, dan ukurannya L. Ia hanya dipertimbangkan bila pemilik menolak jendela baris.

## 6. `GridRowPainter` (`Views/GridRowView.swift`)

### 6.1 Urutan cat per baris (sama dengan SwiftUI)

1. Stripe: bila `alternateRows` dan baris ganjil, `ink 0.03` seluruh tinggi baris dan seluruh lebar dokumen (bukan hanya sampai kolom terakhir).
2. Gutter bila `showRowNumbers`: teks `index + 1` rata kanan di lebar 44, font `code(10.5)`, `ink 0.35`; kotak 25 pt; separator 1 pt di tepi kanan kotak, `ink 0.05`.
3. Untuk tiap kolom terlihat, dalam urutan: wash → teks → titik staged → separator.
   - **Wash** di seluruh kotak sel: staged `amber 0.20` menang atas seleksi `accent 0.20`.
   - **Teks** rata kiri, atau rata kanan bila `isNumeric(type)`. Lebar tersedia = lebar kolom tanpa padding; padding kiri 8.
   - **Titik staged** 4×4 pt, `amber`, di pojok kanan atas kotak dengan jarak 3.
   - **Separator** 1 pt di tepi kanan kotak, `ink 0.05`.

Teks:

- `null`: teks `DataPreferences.nullDisplay`, miring, `ink 0.3`.
- `empty`: `∅`, `ink 0.3`.
- selain itu: `ink 0.9`, `CTLine` dari `text` (+ `…` bila `truncated`), dipotong ekor bila melebihi lebar (`CTLineCreateTruncatedLine` dengan token `…` bergaya sama).
- Font: `FontChoice.codeNSFont(size: 12, weight: nil)`; miring lewat trait italic yang sama dengan `.italic()` (diverifikasi P-3).
- Posisi vertikal: pusat kotak sel; baseline dibulatkan sesuai aturan yang ditetapkan P-2. Sel staged menampilkan teks staged dengan format yang sama (`stored.map { format.render }`).

### 6.2 Warna

`GridPalette` (nilai `NSColor`/`CGColor`, dibangun per appearance dan gaya):

- `ink` = putih di dark, hitam di light (sama dengan `Tone.ink`), dengan alfa di atas.
- `amber` tetap `0xFFB547`; `accent` dari `ThemeStore.shared.accent.glow` (dibaca sekali per perubahan gaya).
- Warna dinamis diresolusi dengan `NSAppearance.performAsCurrentDrawingAppearance` di main sebelum masuk cache; `CTLine` di cache membawa `CGColor` yang sudah jadi, dan appearance atau aksen berganti mengosongkan cache. Ini juga yang membuat prebuild di luar main aman (§9.4).

### 6.3 Penyetelan yang ditentukan probe, bukan kode

Dua hal tidak bisa ditetapkan dari kode SwiftUI, dan probe P-2 memutuskannya terhadap baseline: (a) pembulatan tepi horizontal kolom (`.rounded()` seperti tepi vertikal, atau `floor`/`ceil`), dan (b) pembulatan baseline dan origin x teks. Keputusan diambil dengan menghitung selisih terhadap ke-30 scene grid yang ditangkap `captureGrid` (15 per tampilan, baseline pasca-W4-T1; dua `grid-json` adalah popover pembaca dan tidak tersentuh) dan memilih varian yang memberi nol kegagalan layout dan warna.

### 6.4 Idempoten

Menggambar ulang rect yang sama dua kali harus menghasilkan piksel yang sama (tidak ada pencampuran ganda alfa pada invalidasi parsial). `GridParityTests.testRepaintingARowTwiceGivesTheSamePixels` mengunci ini (P-4).

### 6.5 Luapan Compact

Karena loop di §5.2 mengecat baris `r-1…r+1` ke satu konteks berurutan, pita tumpang tindih otomatis mengikuti urutan SwiftUI: stripe baris N dicat di atas luapan N−1, lalu sel N, lalu luapan N+1. Tidak ada kode khusus selain `overflow` di §5.2.

### 6.6 Fallback row view

Bila P-4 memutuskan mode sel tidak bisa dipakai: `GridRowView: NSTableRowView` memanggil painter yang sama untuk `bounds`-nya, dan menggambar pita luapan `r-1` (dasar) dan `r+1` (atas) di dalam bounds sendiri dengan urutan (a) luapan N−1, (b) stripe N, (c) sel N, (d) luapan N+1; baris terakhir menggambar ekornya lewat `GridTableView.draw(_:)`. Painter tidak berubah; hanya tuan rumahnya.

## 7. Header: `GridHeaderView`

Subclass `NSTableHeaderView`, `draw(_:)` di-override sepenuhnya, dengan hit-test sendiri dari `GridColumnGeometry`. Latar `recess 0.30`, garis bawah 1 pt `ink 0.12`, tinggi `GridMetrics.headerHeight` (50 pt untuk font sistem).

Per kolom (kotak selebar kolom, padding 8/6):

- Label: `code(12, .semibold)`, `ink 0.92`, satu baris dipotong ekor; rata kanan bila numerik. Label mengikuti `columnLayout.label(source, original:)`.
- Titik rename `accent 0.7` 4 pt bila `isRenamed`; chevron sort (`chevron.up`/`chevron.down`, 8 pt bold, `accent`) bila kolom ini yang disortir. Lebar label dikurangi keduanya, seperti `HStack` hari ini.
- Chip tipe: kapsul `tint 0.14` dengan teks `code(11, .semibold)` bertinta `typeTint` (mint numerik, violet bool, amber tanggal/waktu, ice lainnya), padding 8/3. **Koreksi AR:** `Chip` hari ini adalah `Text` tanpa `lineLimit` (`Theme.swift:612-624`) di dalam `frame(width:)`, jadi teks tipe yang lebih lebar dari kolom **dibungkus**, dan header seluruh baris ikut meninggi. Ini kasus biasa, bukan kasus tepi: kolom sempit (84 + 22 pt) bertipe `character varying` sudah melewatinya. Bawaannya adalah **meniru pembungkusan** (`CTFramesetter` dengan font dan lebar isi yang sama; tinggi header = maksimum atas kolom terlihat), dengan aturan pemenggalan yang dipastikan P-3. Memotong menjadi satu baris adalah perubahan piksel di luar daftar PRD §6.5, jadi hanya boleh dengan persetujuan pemilik dan kode V baru.
- Funnel: `line.3.horizontal.decrease.circle(.fill)` 10 pt, `accent` bila filter aktif, selain itu `ink 0.30`; di pojok kanan atas dengan jarak kanan 6 dan atas 4. **Area klik = bounding box simbol** (sekitar 11×11), persis hari ini; W10-T2 yang menaikkannya.
- Separator 1 pt `ink 0.05` di tepi kanan. Kolom `#` untuk gutter bila nomor baris aktif.
- Simbol SF diambil sekali per (nama, ukuran, berat) dan ditinta, lalu di-cache.

Interaksi: klik di funnel memanggil `actions.openFilter(source, rect)`; klik di tempat lain memanggil `actions.sortClick(source)` (siklus `GridSort.next` atau pengganti W4, §12.4). Klik saat streaming dinonaktifkan hanya di W6. Menu konteks header adalah `NSMenu` dengan item, urutan, dan judul yang sama dengan `columnMenu` (`:666-694`): Sort Ascending/Descending; bila hasil terpotong, Sort on Server (Ascending/Descending); Move Left/Right (dinonaktifkan di ujung); Rename Column…; Hide Column; Show All Columns; Reset Column Layout. Tooltip header: "Sort by \<label\> — over the rows already fetched, not the whole result" dan "Filter this column"/"Filtered by \<label\>", lewat pengendali tooltip yang sama dengan sel (§8.5).

## 8. Interaksi

### 8.1 Pointer dan seleksi

`mouseDown` → `coordinator.press(at:clickCount:)`; `mouseDragged` → `drag(to:)`; `mouseUp` → `release()`.

- `press`: `row = geometry.row(atY:)`, `column = clampedColumn(atX:)`; jangkar = sel itu; `tab.selectCells(anchor:focus:)`. Bila `clickCount == 2`, jalankan tindakan dobel-klik atas sel itu (§8.3).
- `drag`: sel target dihitung dari titik tabel; bila sama dengan fokus sekarang, tidak ada yang ditulis. Bila berbeda, `tab.selectCells(anchor:focus:)` dan invalidasi selisih (§5.3) langsung, tanpa menunggu putaran SwiftUI (`applied.selection` diperbarui agar `updateNSView` tidak mengerjakannya dua kali).
- `release`: `inspectedRange` diselesaikan (callback ke `ResultGrid`) dan pengumuman VoiceOver "3 × 2 cells selected" diposting (§11).
- Tidak ada autoscroll saat drag melewati tepi (fitur baru; W10). Baris di luar viewport tetap terhitung dari posisi pointer, sama dengan hari ini.

### 8.2 Menu konteks tubuh

`menu(for:)` mengembalikan `NSMenu` dengan judul dan urutan `selectionMenu` (`:528-553`): Copy, Copy with Headers, —, Edit Cell…, View Value…, Paste, —, Review N Changes…, Discard N Changes, —, Undo Edit, Redo Edit. Status enabled dihitung saat menu dibuka. **Klik kanan tidak mengubah seleksi** (§1.3 butir 1).

### 8.3 Dobel-klik dan editor

`openable = GridValue.isOpenable(value: stored, type:)` (penuh, bukan flag) atas nilai staged-atau-asli:

- openable dan panel inspektur tidak berdiri: popover pembaca (§8.4).
- openable dan panel berdiri: tidak ada tindakan (koreksi AR, `:790`).
- tidak openable: editor.

Editor: `GridCellEditor: NSTextField` (subview tabel), `mono12`, tanpa border dan latar, `focusRingType = .none`, satu baris, di rect isi sel (padding 8, dipusatkan vertikal). Diisi `tab.cellValue(at:) ?? ""`; `tab.beginCellEdit(at:)`.

- Perubahan teks (`controlTextDidChange`) → `tab.typeCellEdit(text)`. Sesi menggabung ketukan menjadi satu langkah undo, seperti hari ini.
- Return (`control(_:textView:doCommandBy: insertNewline:)`) → `commitEdit`: bila seleksi lebih dari satu sel, `tab.fillCellEdits(text, over:)`, selain itu `typeCellEdit` + `endCellEdit`; overlay dilepas.
- Esc (`cancelOperation`) → `cancelCellEdit`; overlay dilepas.
- `beginEdit` pada sel lain mengakhiri sesi yang terbuka lebih dulu (`tab.endCellEdit()`), sama dengan `:921`.
- **Perubahan kecil, disengaja (koreksi AR atas aturannya):** overlay dibatalkan (`cancelCellEdit`) hanya bila revisi baru membuat kunci selnya tidak lagi menunjuk baris yang sama: filter, search, atau sort berubah; sort, filter, atau search aktif saat hasil diganti (penggantian mengosongkan sort dan mengurutkan ulang baris); jumlah kolom berubah; atau `key.row ≥ result.count`. Penggantian streaming yang hanya menambah baris (kolom sama, tanpa sort, filter, atau search) **tidak** membatalkan overlay. Versi awal membatalkan pada setiap `gridRevision`, dan itu akan membuang teks yang sedang diketik setiap 200 ms selama streaming (jalur kehilangan teks pengguna, risiko tinggi). Hari ini `TextField` yang tersisa bisa muncul di atas baris lain dan menulis ke baris yang salah (§14.3).

### 8.4 Popover

`NSPopover(behavior: .transient)` dengan `NSHostingController(rootView: CellValueViewer(value: stored ?? "", column:, type:, connectionID:, table:))`, dijangkarkan di rect sel pada tepi bawah (tabel flipped, jadi `preferredEdge: .maxY`). Tidak pernah dibuka bila `inspectorRange != nil`. Status `viewingCell` tetap di `ResultGrid`, dan pembuka/penutup dicocokkan di `updateNSView`. Popover filter cara yang sama, dijangkarkan ke rect funnel, dengan isi `filterEditor(index)` yang ada; ia dibuka dan ditutup mengikuti `model.filterPopoverColumn`, dan `popoverDidClose` menuliskan `nil`.

### 8.5 Tooltip

- Bawah tabel dan header: `addToolTip(_:owner:userData:)` per sel dan per header **yang terlihat**, dibangun ulang 100 ms setelah scroll berhenti (`boundsDidChange` yang dikoalesensi) dan saat hasil, kolom, atau gaya berganti. Tidak ada pekerjaan per frame saat scroll.
- Pemilik mengembalikan string secara lazy lewat `view(_:stringForToolTip:point:userData:)`: `result.fullValue(row:column:format:)` (format tampilan), hanya untuk sel tak-kosong dan bukan NULL, seperti hari ini. **Batas 8.192 unit UTF-16 dengan penanda "…"** (perlindungan terhadap tooltip 10 MB; perubahan kecil, §14.3).

### 8.6 Format kolom

`Coordinator` menyimpan `formats: [ColumnFormat]` per kolom sumber, dari `ColumnFormatStore.identity(connection:table:column:)` sekali per hasil dan per notifikasi `ColumnFormatStore.didChange`. Penutupan pembaca (yang memanggil `ColumnFormatStore.set`, `CellValueViewer.swift:302`) tidak lagi mengandalkan "perubahan state memaksa gambar ulang" (komentar `:759-761`).

### 8.7 Salin

`⌘C` = `@objc copy(_:)` pada tabel (responder chain), dengan validasi lewat `validateUserInterfaceItem`. Menu dan tombol footer memanggil `copySelection(withHeaders:)` di `ResultGrid` yang menuliskan ke `NSPasteboard.general`. Teks dibangun oleh fungsi murni baru:

```swift
// Models/CellSelection.swift
extension GridClipboard {
    static func text(result: any ResultRows, selection: CellRange, visible: [Int],
                     withHeaders: Bool) -> String?
}
```

Ia memindahkan `selectionText` (`:502-518`) apa adanya: kolom sumber yang tercakup, diurutkan menurut sumber, header dari nama server, baris lewat `result.rows(in:columns:)`. Pembangunan di luar main bila lebih dari 10.000 sel (Fase 5 §9.4 di rencana) lewat `Task.detached` dengan `ArrayRows` yang Sendable; `StoreRows` di W6 memakai `rows_text` sendiri. **Koreksi AR:** pada jalur di luar main, papan klip ditulis saat teks selesai, dan hanya bila `NSPasteboard.general.changeCount` belum berubah sejak ⌘C ditekan. Bila sudah berubah (pengguna menyalin hal lain), hasilnya dibuang. Tanpa penjaga ini, salinan lambat bisa menimpa salinan yang lebih baru. `GridClipboard.text(rows:headers:selection:withHeaders:)` yang ada tetap, dan sepuluh tesnya tidak berubah.

## 9. Cache

### 9.1 Teks tampilan per baris

`GridRowTextCache`: `[Int: [CellText?]]` (baris → sel per kolom terlihat, terisi lazy). Batas:

- baris di luar `[first − 2 halaman, last + 2 halaman]` dipangkas saat scroll berhenti (halaman = jumlah baris terlihat);
- bila estimasi byte (`32 + utf8.count` per sel) melebihi 16 MB, pangkas ke ± 1 halaman, lalu ± 0.

Sel staged tidak masuk cache ini; teksnya dibangun dari nilai staged dan disimpan di cache `CTLine` saja.

### 9.2 `CTLine`

`GridLineCache`: dua peta (`current`, `previous`); saat `current` melewati 32.768 entri atau 8 MB estimasi, `previous = current; current = [:]`, dan pencarian memeriksa keduanya lalu mempromosikan. Kunci `(row: Int32, column: Int32)` dengan `(line, width, epoch)`. `epoch` naik saat gaya, appearance, atau font berubah. Batas 32.768 adalah tebakan awal; W5-T3 mengukur dan menyetelnya dari `peak_footprint_bytes` dan hit rate. Total kedua cache ≤ 24 MB, di dalam anggaran NFR-P3 (anggaran + 64 MB).

### 9.3 Bila hasil sangat besar

Cache tidak pernah memegang per-baris untuk seluruh hasil. `ArrayRows` membaca dari array yang sudah ada tanpa salinan, dan 1 juta baris tidak menambah memori Swift di luar array asalnya.

### 9.4 Prebuild di luar main (bersyarat)

Bila diaktifkan: setelah scroll, main memanggil `cell()` untuk jendela baris (viewport ± 1 halaman) × kolom (viewport ± 1 layar), lalu mengirim `[CellText]` bersama `GridStyle.resolved` (font `CTFont` dan `CGColor` yang sudah diresolusi) ke satu antrean serial `.userInitiated`; hasil `CTLine` dipasang di main bila `epoch` dan `revision` masih sama, selain itu dibuang. Kode ini ±60 baris. **Aturan simpan:** dipertahankan hanya bila `scroll-500x10k` p99 atau hitch membaik ≥ 10% dengan prebuild dibanding tanpa; selain itu dihapus sebelum commit 5b.

## 10. Lebar kolom

Rumus di §4, dihitung sekali per hasil dan per perubahan layout kolom, bukan per `body`:

1. `Coordinator` menyimpan `natural: [CGFloat]` (satu per kolom **terlihat**, urutan tampilan) dengan kunci `(gridRevision, columnLayout)`.
2. `fitted(natural, available:, gutter:)` dipanggil saat tata letak dan saat `contentSize.width` berubah. Bila `total ≥ available − gutter` (kasus lebar, termasuk 500 kolom), hasilnya `natural` dan tidak ada lebar yang berubah, jadi ukuran ulang jendela tidak melakukan apa pun. Bila ada sisa (kolom sedikit), pembagian proporsional atas `n` kecil.
3. Hasilnya menjadi `GridColumnGeometry` dan lebar satu `NSTableColumn`.
4. `naturalCharCounts()` membatasi hitungan di 64, hasil lebar sama karena jepitan 320 sudah tercapai di 42 karakter. Biayanya 200 × 500 sel per hasil, dibayar sekali dan dalam anggaran `open-500x10k`. W6 menggantinya dengan `columnWidths()` (`fase-6` §17.5).

## 11. Aksesibilitas dan model kursor

### 11.1 Model kursor (P-24)

```swift
// Models/CellSelection.swift
struct CellPos: Equatable, Hashable { var row: Int; var column: Int }   // display position
struct GridCursor: Equatable {
    var anchor: CellPos
    var focus: CellPos
    var range: CellRange { CellRange(from: (anchor.row, anchor.column), to: (focus.row, focus.column)) }
}
```

`QueryTab.cellCursor: GridCursor?`. `cellSelection: CellRange?` **tetap sumber kebenaran tulis** (semua penulis dan tes hari ini tidak berubah). `cellSelection.didSet` menjaga invarian "kursor ada di dalam seleksi": nil menghapus kursor; nilai baru yang tidak memuat kursor mereset kursor ke sudut kiri-atas. `tab.selectCells(anchor:focus:)` menulis rentang lalu kursor eksplisit, dipakai `press` dan `drag`. W10-T1 menjadikan kursor otoritatif, menambah tombol, dan menggambar cincinnya (V-9). Di W5 kursor tidak digambar dan tidak ada tombol.

### 11.2 Pohon AX

`GridTableView`: `accessibilityRole = .table`, label "Result grid", `accessibilityRowCount`/`ColumnCount` dari `result.count` dan kolom terlihat. `accessibilityChildren`/`accessibilityRows` membangun (lalu menyimpan per baris) elemen hanya untuk baris terlihat:

- Baris: `.row`, anak-anaknya sel terlihat.
- Sel (`GridAXCell: NSAccessibilityElement`): `.cell`; label **"Row r, column c, \<header\>: \<nilai\>, changed"** (`changed` hanya bila staged; nilai dipotong 256 unit dengan "…"; NULL dibaca sebagai teks NULL yang tampil); `accessibilitySelected` dari seleksi; `accessibilityFrame` dihitung saat ditanya dari geometri (scroll tidak mengubah elemen).
- Header (`GridAXHeader`): `.button`, label nama kolom, nilai tipe, **`accessibilitySortDirection`** dari indikator sort yang sama dengan chevron, aksi tekan = klik sort, aksi kustom "Filter". Header grup tersedia lewat `accessibilityHeaderGroup`.
- `accessibilitySelectedCells` mengembalikan irisan seleksi dengan yang terlihat (terbatas).
- `accessibilityFocusedUIElement`: sel di kursor bila ada. Saat VoiceOver memfokuskan sebuah sel, kursor diatur ke sel itu (tanpa mengubah seleksi) dan sel digulirkan ke terlihat (`scrollToVisible`).
- Notifikasi (`.selectedCellsChanged`, `.layoutChanged` saat hasil berganti) diposting hanya setelah tabel pernah ditanya `accessibilityChildren`. `release()` mengumumkan `announcementRequested` "3 × 2 cells selected" (FR-GRID-07).

Elemen dibuang bila hasil, kolom, atau kolom terlihat berganti. Tanpa klien AX tidak ada objek yang dibuat.

**Koreksi AR, pelengkap pohon.** Tabel mode sel dengan satu `NSTableColumn` punya AX bawaan yang mengaku satu kolom, jadi semua atribut tabel yang dibaca VoiceOver harus di-override, bukan hanya `accessibilityChildren`/`accessibilityRows`:

- `accessibilityColumns` (elemen kolom `.column` yang ringan, satu per kolom terlihat), `accessibilityColumnCount`, `accessibilityVisibleRows`, `accessibilityVisibleCells`, `accessibilityColumnHeaderUIElements` (elemen `GridAXHeader`; header bawaan `NSTableHeaderView` juga hanya tahu satu kolom, jadi AX header di-override di sana), dan `accessibilityCell(forColumn:row:)` untuk posisi mana pun, termasuk di luar viewport, karena navigasi tabel VoiceOver memakainya lalu menggulir.
- Sel dan baris membawa `accessibilityRowIndexRange`/`accessibilityColumnIndexRange` dan `accessibilityIndex` dalam koordinat hasil (§5.4), bukan koordinat tabel. `accessibilityRows` sengaja hanya baris terlihat, dan `accessibilityRowCount` membawa jumlah sebenarnya.
- **Identitas stabil, biaya terbatas.** Elemen di-cache dengan kunci `(revision, baris hasil, kolom sumber)`, supaya VoiceOver tidak kehilangan fokus karena objek dibuat ulang setiap kali ditanya. Cache dipangkas ke viewport ± 1 halaman saat scroll berhenti, kecuali elemen yang sedang difokus. Biayanya sekitar baris terlihat × kolom terlihat (± 800 objek pada 40 × 20), hanya selama ada klien AX.
- **Label dan nilai dihitung saat ditanya** (`accessibilityLabel()` membaca `result.cell` dan `cellEdits` saat itu), tidak disimpan saat elemen dibuat. Elemen yang di-cache tidak boleh mengumumkan nilai basi setelah edit.
- `accessibilityFrame` lewat `accessibilityFrameInParentSpace` dengan induk baris dan induk tabel, supaya AppKit yang mengonversi ke layar setelah scroll.
- `.selectedCellsChanged` dan pengumuman "3 × 2 cells selected" hanya diposting di `release()`, tidak per langkah drag.
- Klien AX lain selain VoiceOver (Voice Control, pengelola jendela, Accessibility Inspector) juga memicu pembuatan. Batas di atas tetap berlaku, dan itu yang membuat "hanya saat ditanya" aman tanpa mendeteksi VoiceOver.

Yang bukan bagian W5: keyboard, cincin kursor, kontras, dan tampilan staged baru (W10). String label berbahasa Inggris seperti string UI lain di app.

## 12. Aliran data: `ResultGrid` ⇄ tabel

### 12.1 Struktur `ResultGrid`

Bagian SwiftUI yang tetap: `content`, `status`, `placeholderBody`, `gridToolbar`, footer, sheet, inspektur, `GridSortBanner`, `GridServerSortBanner`, dan filter serta preset. `grid(preview)` menjadi:

```swift
VStack(spacing: 0) {
    gridToolbar(preview)
    banner            // GridSortBanner / GridServerSortBanner, same background as the header
    ResultGridTable(tab: tab, model: model, inputs: gridInputs, commands: gridCommands)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    placeholder body  // only when there are no rows: the table is given the header's height
}
```

Tanpa baris, tabel diberi tinggi header (`GridMetrics.headerHeight`) dan `placeholderBody` mengisi sisanya, sama dengan tata letak dua-bagian `:284-289` hari ini (header tergeser horizontal, isi dipusatkan sepanjang panel). `.onCopyCommand` dan `.contextMenu` dihapus; digantikan responder dan `menu(for:)` tabel.

### 12.2 `GridInputs`

Nilai `Equatable` yang dibaca `ResultGrid.body` (sehingga dependensi Observation terdaftar) dan dioper ke `updateNSView`:

```swift
struct GridInputs: Equatable {
    var revision: Int                    // tab.gridRevision
    var layout: GridColumnLayout
    var selection: CellRange?
    var edits: CellEdits
    var sort: SortIndicator?             // (source column, direction) from W4's activeSort
    var filtered: Set<Int>               // columns with an active filter
    var style: GridStyle                 // row height, alternate rows, row numbers, null text,
                                         // code font family, accent, sortEnabled
    var filterPopover: Int?              // model.filterPopoverColumn
    var viewing: CellKey?                // reader popover
}
```

`updateNSView` membandingkan dengan `coordinator.applied`, mengerjakan hanya selisihnya (§5.3), dan menyimpan `applied`. Tanpa selisih, ia tidak melakukan apa pun. Bila probe P-5 menemukan bahwa pembacaan di `updateNSView` tidak cukup untuk memicunya (seharusnya cukup, karena `body` yang membaca), fallback-nya `withObservationTracking` di `Coordinator`.

### 12.3 `GridCommands`

Struct penutup yang dioper dari `ResultGrid` (yang memegang `@State` untuk lembar dan popover):

```swift
struct GridCommands {
    var sortClick: (Int) -> Void                 // source column
    var openFilter: (Int, CGRect) -> Void
    var rename: (Int) -> Void
    var review: () -> Void
    var copy: (Bool) -> Void
    var viewValue: () -> Void
    var beginEdit: (CellKey) -> Void
    var commitEdit: (CellKey, String) -> Void
    var cancelEdit: () -> Void
    var settleSelection: () -> Void
    var paste: () -> Void
}
```

Tujuannya: `GridHeaderView` dan tabel tidak mengenal `AppModel`, `QueryTab.setGridSort`, atau API Batch 7.

### 12.4 Ketergantungan pada W4-T1

W5 hanya membutuhkan dua hal dari W4-T1: (a) `tab.activeSort` yang menyediakan `(column, direction)` untuk chevron, dan (b) sebuah fungsi klik header yang dipanggil `commands.sortClick`. Bila W4-T1 memberi nama lain, hanya pembentukan `SortIndicator` dan penutup `sortClick` di `ResultGrid.swift` yang berubah. Menu header memakai `commands` yang sama untuk "Sort on Server". Banner (V-1) tetap SwiftUI dan tidak disentuh W5.

## 13. Daftar paritas fitur, dengan tes yang membuktikannya

"Ada" berarti tesnya sudah ada di `23d3ac7` dan harus tetap hijau. "Baru" berarti ditulis di W5 (§18).

| # | Perilaku | Bukti |
|---|---|---|
| 1 | Klik memilih satu sel, drag memilih blok, jepit di tepi | `CellSelectionTests` (rentang), baru: `GridParityTests.testPressAndDragSelectABlockAndClampAtTheEdges` |
| 2 | Filter, sort, sembunyi, pindah menghapus seleksi | ada: `CellSelectionTests.testChangingAFilterDropsTheSelection`, `GridColumnsTests.testASelectionIsClearedByAHideOrAMove…`, `ResultGridTests.testChangingTheSortDropsTheSelectionAndTheQueuedEdits` |
| 3 | Salin: TSV, dengan header, NULL kosong, kutip, baris pendek, kolom sumber terurut pada grid yang dipindah atau disembunyikan | ada: 10 tes `GridClipboard`; baru: `GridParityTests.testCopyIsBuiltFromSourceColumnsInSourceOrder` (bagian ini tidak punya tes hari ini) |
| 4 | Edit: dobel-klik, Return menyimpan, Esc membatalkan, blok diisi satu langkah, sesi lama diakhiri | ada: `CellEditsTests`, `CellEditUndoTests`; baru: `GridParityTests.testTypingInTheOverlayIsOneUndoStep`, `…testReturnOverABlockFillsIt`, `…testEscLeavesTheQueueUntouched` |
| 5 | Sel staged: wash amber, titik, teks staged berformat | scene `grid-edits` (`VisualParityTests`), marker `staged[r,c].dot` |
| 6 | Sel openable membuka pembaca; tidak bila inspektur berdiri; "View Value…" | ada: `GridValueTests`; baru: `GridParityTests.testDoubleClickOpensTheReaderOnlyForAnOpenableCellAndNotBesideThePanel` |
| 7 | Sembunyikan, pindah, ganti nama, reset kolom, lewat menu header dan menu Columns | ada: `GridColumnsTests` (16 tes); baru: `GridParityTests.testTheHeaderMenuHasTheSameItemsAndEnabledStates` |
| 8 | Sort: siklus klik menurut arah pertama, chevron, banner in-memory dan server | ada: `ResultGridTests.testAHeaderClickCycles…`, `DataPreferencesTests.testTheFirstSortDirection…`, scene `grid-sorted`; baru: `…testAHeaderClickCallsSortClickAndTheChevronFollowsTheIndicator` |
| 9 | Filter per kolom (picker atau teks), funnel terisi bila aktif, preset | ada: `GridColumnsTests`, `FilterPresetTests`; baru: `…testTheFunnelHitAreaOpensTheFilterAndTheRestOfTheHeaderSorts` |
| 10 | Search lintas kolom dan eskalasinya | ada: `GridColumnsTests.testTheInMemorySearch…`, `ServerSortTests`, `QuickSearchTests` (toolbar tidak berubah) |
| 11 | Inspektur di samping grid, terselesaikan saat drag berakhir | scene `grid-inspector`; baru: `…testTheInspectorFollowsTheSettledSelection` |
| 12 | Batch 6: tinggi baris (tiga), tampilan NULL, baris selang-seling, nomor baris, arah klik pertama, viewer default | `DataPreferencesTests`; scene `grid-kinds{-altoff,-nonum,-tall,-compact}` |
| 13 | Format kolom Raw/Text/UUID/Unix/JSON, dan perubahannya menggambar ulang | ada: `ColumnFormatTests`; baru: `…testAFormatChangeRepaintsTheColumnThroughTheNotification` |
| 14 | Placeholder (memuat, tanpa baris, difilter habis, dihentikan), footer, kontrol limit | ada: `GridPlaceholderTests`, `StoppedRunTests`, `RowLimitSettingTests`, `CountRowsTests`; scene `grid-empty`, `grid-filtered-out`, `grid-counted` |
| 15 | Rata kanan numerik, NULL miring, `∅`, elipsis ekor | marker `cell[r,c].null/.empty/.long` di scene `grid-kinds` |
| 16 | Lebar kolom: rumus, jepit 84–320, sisa proporsional, gutter dikurangkan | baru: `GridMetricsTests` (rumus belum punya tes hari ini); `columnEdges` di ke-30 scene grid |
| 17 | Tooltip nilai penuh | baru: `GridParityTests.testTheTooltipIsTheFullFormattedValueAndAbsentForNullAndEmpty` |
| 18 | Undo/redo edit lewat menu | ada: `CellEditUndoTests`; baru: `…testTheMenuEnablesUndoAndRedoFromTheQueue` |
| 19 | Paste blok | ada: `CellEditsTests.testPasting…` |
| 20 | Rencana tulis (`ChangeReview`, `WritePlan`) tidak berubah | ada: `WritePlanTests`, `WriteBatchBudgetTests`, `MatchPolicyTests`, `UpdateLiteralTests` |
| 21 | Streaming: hasil diganti utuh tanpa kehilangan posisi scroll dan membuang sort | ada: `ResultGridTests.testNewRowsClearTheSort`; baru: `…testReplacingTheResultKeepsTheScrollOffset` |
| 22 | Scene snapshot | `VisualParityTests.testGridScenesMatchTheirBaselines` (32 scene: 30 grid dan 2 `grid-json`), dan `testGridGeometryIsTheSameInDarkAndLight` |
| 23 | `PerfSignposts.firstPaint` saat baris 0 tergambar | baru: `…testTheFirstPaintSignpostFiresOnceWhenRowZeroIsDrawn` |
| 24 | Menu tubuh: item, urutan, status aktif (termasuk "View Value…" nonaktif saat panel berdiri), dan klik kanan tidak mengubah seleksi (koreksi AR) | baru: `…testTheCellMenuHasTheSameItemsAndEnabledStatesAndLeavesTheSelection` |
| 25 | Tooltip header ("Sort by …", "Filter this column", "Filtered by …") (koreksi AR) | baru: `…testTheHeaderTooltipsAreTheSameStrings` |
| 26 | Explain di grid: baris plan dengan spasi awal, footer "Query plan · N lines", placeholder "No plan…" (koreksi AR; scene `explain` tidak ada di gate) | ada: `GridPlaceholderTests`; baru: `…testAPlanKeepsItsLeadingSpacesAndTheFooterCountsLines`; cek tangan di laporan 5b |
| 27 | Fokus dan ⌘C: klik menjadikan tabel first responder, ⌘C lewat responder chain hanya aktif dengan seleksi, panah dan ⌘A tidak mengubah apa pun (D-15) (koreksi AR) | baru: `…testCopyGoesThroughTheResponderChainAndRowSelectionStaysEmpty` |
| 28 | Appearance, aksen, atau font kode berganti saat tabel tampil menggambar ulang dengan palet baru (`viewDidChangeEffectiveAppearance`, `epoch`) (koreksi AR) | baru: `…testAnAppearanceChangeBumpsTheEpochAndRepaints` |
| 29 | Overlay editor bertahan saat streaming menambah baris dan dibatalkan saat urutan berubah (§8.3) (koreksi AR) | baru: `…testTheEditorSurvivesAnAppendAndEndsOnAReorder` |
| 30 | IME di overlay: Return menyelesaikan komposisi, bukan menyimpan edit (koreksi AR) | cek tangan (Jepang atau Cina), dicatat di laporan 5b |

## 14. Gate paritas visual lulus tanpa rekam ulang

### 14.1 Apa yang diuji dan bagaimana tabel memenuhinya

`VisualParityTests` menghosting `ResultGrid(tab:)` di `NSHostingView` (1000×520, jendela `.titled`), menangkap dengan `cacheDisplay`, dan memeriksa lima lapis (§ komentar berkas). Untuk grid:

| Lapis | Tuntutan | Cara dipenuhi |
|---|---|---|
| Layout | tiap separator (x, atas, bawah), aturan horizontal, awal baris, pitch stripe: **persis sama** | `GridColumnGeometry` dan `GridMetrics.box` mereproduksi §1.1 (kotak 23/25, celah, luapan Compact, snap tepi). Tinggi header dan banner sama. |
| Warna | RGBA di titik sampel dalam padding: **persis sama** | alfa dan urutan cat yang sama, `sourceOver`, sRGB. P-2 memastikan tidak ada selisih ±1 dari pencampuran. |
| Marker | titik staged, chevron, funnel, chip, NULL/∅/elipsis: hitungan dalam 0,5×–2× | dicat oleh painter dan header |
| Piksel | ≤ 0,1% piksel selisih > 16/255 | teks lewat `CTLine`, font dan origin disetel P-2 |

`VisualParityTests`, tes komparator, dan baseline tidak diedit. V-9 dicadangkan untuk W10, jadi tidak ada perubahan visual yang boleh masuk di Fase 5.

**Koreksi setelah implementasi (V-13).** Klaim "tidak ada perubahan visual" di atas **tidak tercapai**, dan sebabnya bukan implementasi yang belum selesai: tiga lapis gate punya batas yang tidak bisa dilewati dari dalam `draw(_:)`. Tepi kolom fraksional membulat ke piksel yang berbeda dari layout `HStack` yang menerus (56 assertion layout, 1 pt). Teks SwiftUI ber-antialias subpiksel dan `CTLineDraw` ke bitmap `cacheDisplay` ber-antialias grayscale; delapan kombinasi langkah §14.2 (0–3 flag konteks, baseline fraksional, x bulat, alpha tinta penuh, dan seluruh pasangannya) semuanya mengukur ≥ 4,998% piksel berbeda dari anggaran 0,1%. Chevron sort 10×6 px sementara `.font(.system(size: 8, weight: .bold))` pada `Image(systemName:)` menghasilkan 6×4 (14 assertion marker); `SymbolConfiguration` tidak punya `init(font:)`.

Langkah §14.2 dijalankan sampai habis dan berhenti pada "berhenti dan lapor". Karena V-9 milik W10 dan hanya menyentuh kursor, kontras, staged, dan funnel — bukan penggantian renderer — keputusan pemilik adalah mendaftarkan **V-13** di PRD §6.5 untuk migrasi renderer ini, lalu merekam ulang 30 scene grid **di commitnya sendiri** (`W5-T1` commit 5b, terpisah dari commit kode). Geometri kolom, tinggi header, dan fitur kolom sudah direproduksi persis; yang direkam ulang adalah rasterisasi teks dan pembulatan tepi. Gate tetap hidup: rekam ulang berikutnya butuh kode V baru.

### 14.2 Tangga bila teks di luar toleransi

Teks adalah satu-satunya lapis yang tidak bisa dijamin dari kode. Urutan, berhenti di langkah pertama yang lulus:

1. Baseline dibulatkan (`floor`, `round`, `ceil`) dan origin x integer atau pecahan.
2. `CTLineDraw` dengan `CGContext` yang sama dibanding `NSAttributedString.draw(at:)`; `shouldSmoothFonts` dan `shouldSubpixelPositionFonts` dicoba pada kedua nilai.
3. Font: `NSFont.monospacedSystemFont` dibanding font yang dibuat SwiftUI untuk `.system(size:design:.monospaced)` (`Fonts.swift:135-143` mencatat bahwa keduanya bisa berbeda).

Bila semua gagal, **berhenti dan lapor** dengan angka per scene. Rekam ulang baseline tidak boleh dilakukan tanpa persetujuan pemilik (`development-plan.md` §0.5), dan tidak ada V-n yang dicadangkan untuk teks. Keputusannya milik pemilik.

### 14.3 Perbedaan perilaku yang disengaja dan kecil

Tidak terlihat di scene mana pun; dicatat di pesan commit 5b **dan** di `performance-plan.md` §14 butir 9 (perubahan perilaku yang terlihat meski piksel sama). Kode V hanya untuk perubahan piksel, jadi butir yang diizinkan di bawah tidak memakainya. Keputusan AR per butir:

1. Banner sort tidak ikut tergeser horizontal (D-13). **Diizinkan tanpa kode V, bersyarat:** banner V-1 terjangkar kiri saja dan G-VIS `grid-sorted-*`/`grid-kinds*` pasca-W4-T1 hijau. Pada offset 0, pita `recess 0.30` yang terpecah dua (banner SwiftUI dan `GridHeaderView`) menghasilkan warna yang sama di atas kanvas yang sama, dan tinggi banner bulat (21 pt), jadi tidak ada jahitan. Bila syarat gagal, pakai jalan kedua D-13.
2. Overlay editor dibatalkan saat urutan atau bentuk hasil berubah, tidak saat streaming menambah baris (§8.3). **Diizinkan:** hari ini sesi yang tersisa menulis ke baris yang salah.
3. Tooltip dibatasi 8.192 unit (§8.5). **Diizinkan**, tetapi `performance-plan.md` §14 butir 9 menulis "nilainya penuh" dan harus diubah menjadi "penuh hingga 8.192 unit UTF-16".
4. ~~Chip tipe satu baris dan dipotong.~~ **Tidak diizinkan tanpa kode V.** Chip hari ini dibungkus (§7), dan memotongnya mengubah piksel dan tinggi header di hasil nyata meski tidak ada di baseline. Bawaannya meniru pembungkusan.
5. `NSTextField` menggantikan `TextField`: perilaku Return, Esc, dan undo dikunci tes (`performance-plan.md` §14 butir 9). **Diizinkan**, sudah terdaftar.
6. (Koreksi AR, baru.) Klik pada grid memindahkan fokus keyboard dari editor ke grid, dan ⌘C menyalin blok (D-15). Hari ini gestur SwiftUI tidak mengambil fokus. **Diizinkan**, karena ⌘C di grid ada di daftar paritas, dan harus ditambahkan ke `performance-plan.md` §14 butir 9.

## 15. Probe sebelum kode 5b

Diselesaikan implementer sebagai langkah pertama 5b (kode sekali pakai di `target/run/`, hasilnya masuk laporan). Tidak ada yang menyentuh berkas produksi. **Koreksi AR:** P-1 dikeluarkan dari urutan 5b (§5.4). P-2 sampai P-6 tetap di sini.

| ID | Pertanyaan | Lulus bila | Bila gagal |
|---|---|---|---|
| P-1 | **Gerbang W6-T1, bukan 5b.** Apakah isi yang dikomposisi WindowServer tetap sejajar dengan geometri AppKit di atas 2^24 pt? Tabel 5 juta baris pada 21, 25, dan 30 pt, di dev build lewat path dengan data terisolasi. Offset uji: 2^24 − 1 halaman, 2^24 + 1 halaman, 2^25, 2^26, 2^27 (bila tercapai), dan ujung. Bukti harus lewat compositor: tangkapan jendela oleh WindowServer (`screencapture -l <window id>`, butuh izin Screen Recording, jadi dijalankan pemilik atau di sesi bench eksklusif). `cacheDisplay`, `bitmapImageRepForCachingDisplay`, dan `CALayer.render(in:)` menggambar di CPU dengan `Double` dan **tidak bisa** menunjukkan cacatnya: hasil lulus dari jalur itu tidak sah. | Di setiap offset: (a) 16 langkah origin klip berturut-turut sebesar 1 pt masing-masing menggeser gambar tepat 1 pt (2 px di 2x); (b) indeks baris yang tergambar di satu titik jendela sama dengan `row(at:)` untuk titik itu; (c) jarak stripe di layar sama dengan `rowHeight` di seluruh tinggi jendela. | Mitigasi jendela baris (§5.4) masuk W6-T1 sebelum plafon naik. Virtualisasi hanya bila pemilik menolak jendela baris. Hasil yang tidak terukur (izin OS) dicatat `tidak diukur (izin OS)`, dan plafon tetap 200.000 sampai terukur. |
| P-2 | Apakah kotak, pembulatan tepi, baseline, dan warna menghasilkan nol kegagalan layout dan warna terhadap 30 scene grid (baseline pasca-W4-T1)? | nol kegagalan lapis 1 dan 2, lapis 4 ≤ 0,1% | tangga §14.2 |
| P-3 | Perilaku `Text` SwiftUI hari ini: baris baru, tab, karakter kontrol, miring monospaced, chip lebih lebar dari kolom, `ImageRenderer` di `target/run` | aturan yang ditulis di §3.2, §6.1, dan §7 cocok | aturan diubah mengikuti hasil probe; tes `GridParityTests` mengunci hasilnya |
| P-4 | Apakah mode sel dengan `draw(_:)` sendiri tergambar di `cacheDisplay`, tidak mencampur ganda saat invalidasi parsial, dan `maxWidth` tak-hingga menjaga dokumen 75.000 pt? (Koreksi AR: `cacheDisplay` atas `NSScrollView` di dalam `NSViewRepresentable` sudah terbukti oleh scene editor di gate yang sama. Yang tersisa ditambah: header tidak menumpuk di atas isi, yaitu baris 0 mulai tepat di bawah garis header; `NSScrollView` tidak menggambar latar atau garis header sistem sendiri; tidak ada `cornerView`.) | ya | fallback row view (§6.6) |
| P-5 | Apakah membaca properti `@Observable` di `body` yang membangun `GridInputs` memicu `updateNSView`, dan berapa kali `body` dievaluasi per drag? | ≤ 1 per perubahan sel | `withObservationTracking` di `Coordinator` |
| P-6 | Apakah `addToolTip` per sel terlihat dan popover di rect sel berperilaku benar setelah scroll dan pada tabel flipped? | ya | tooltip lewat satu rect besar dan `stringForToolTip` per titik |

## 16. Bench dan gate performa

Skenario sudah ada di `BenchMode.swift`; tidak ada yang perlu ditulis.

| Skenario | Batas (NFR-P4) | Hari ini (W1) |
|---|---|---|
| `scroll-30x1m`, fling vertikal | hitch ≤ 1 ms/s, p99 frame ≤ 8,3 ms | 910,37 ms/s, 262,7 ms |
| `scroll-500x10k`, horizontal + vertikal | hitch ≤ 1 ms/s, p99 ≤ 8,3 ms | 2.952,93 ms/s, 8.468,1 ms |
| `open-500x10k` | `render_ms` ≤ 30 | 2.017,4 ms |
| TTFR S1 (`firstPaint`), `ttfr-s1-1k` dan `ttfr-s1-10k` | tidak mundur | median 66,4 ms dan 76,7 ms (`docs/benchmarks.md`, sumbu 1) |

Anggaran `open-500x10k` (30 ms): kolom dan lebar ≤ 8 ms (satu kolom, `naturalCharCounts` 200 × 500), gambar pertama ≤ 8 ms (sekitar 8 kolom × 40 baris), sisanya SwiftUI. Memori: puncak footprint tidak boleh naik lebih dari cache (≤ 24 MB) di atas baseline.

Alat: `app/dist/QueryHive.app/Contents/MacOS/QueryHive --bench <skenario>` dengan data terisolasi, lewat path (aturan 12). W5-T1 menjalankan tiap skenario sekali sebagai asap dan hasilnya bukan angka resmi; angka resmi dan verdict PO milik sesi eksklusif W5-T3. Tabel dibandingkan dengan `qhbench` head-to-head oleh pemilik.

## 17. Yang dihapus, perubahan per berkas, dan commit

### 17.1 Dihapus di 5b

- Dari `ResultGrid.swift`: `naturalWidths`, `widths(fitting:)`, `gutterWidth` (pindah ke `GridMetrics`), `rowView`, `cellView`, `cell`, `gutter`, `cellBackground`, `headerRow`, `headerCell`, `columnMenu` (pindah menjadi item `NSMenu` yang memanggil `commands`), `selectionDrag`, `geometry`, `dragAnchor`, `editingCell`, `editingText`, `editorFocused`, `popoverBinding`, `isNumeric`, `typeTint` (pindah), `LazyVStack` dan `ForEach`, `.onCopyCommand`, `.contextMenu`, dan `.onAppear { PerfSignposts.firstPaint() }`.
- `GridGeometry` (`CellSelection.swift`), diganti `GridColumnGeometry` beserta enam tesnya.
- Pembacaan `ColumnFormatStore.format` dan `GridValue.isOpenable` per sel di jalur gambar.

Yang **tidak** dihapus di W5 dan menunggu W6: `QueryTab.displayedRows`, `displayedCache`, `GridSort.order`, `ColumnFilter.matches`, `GridSearch.matches` (`fase-6` §17.5).

### 17.2 Perubahan per berkas

**Commit 5a. `refactor(grid): every reader of rows goes through a ResultRows seam`**

| Berkas | Perubahan |
|---|---|
| `Models/ResultRows.swift` (baru) | `RowReading`, `CellFlags`, `CellText`, `ResultRows`, `ArrayRows` |
| `Models/GridMetrics.swift` (baru) | rumus lebar (diekstrak), `isNumeric`, `lineHeight`, `box`, `headerHeight`, `GridColumnGeometry` |
| `Models/QueryTab.swift` | `result`, `PreviewResult.rowCount`, `fetchedValue`, `fillCellEdits`, `pasteCellEdits` lewat seam; `cellCursor`, `selectCells` |
| `Models/CellSelection.swift` | `CellPos`, `GridCursor`, `GridClipboard.text(result:…)`; `GridGeometry` dipertahankan sampai 5b |
| `Models/{WritePlan,UpdateStatements,CellEdits}.swift` | parameter `some RowReading` |
| `Models/ColumnFormat.swift` | notifikasi `ColumnFormatStore.didChange` dari `set` |
| `Views/ResultGrid.swift` | semua pembaca §3.3 lewat `tab.result`; lebar lewat `GridMetrics`; salinan lewat `GridClipboard.text(result:…)`. Renderer SwiftUI tetap. |
| `Views/Panels.swift`, `Support/BenchMode.swift` | `rowCount` |
| Tes (baru) | `ResultRowsTests`, `GridMetricsTests` (rumus lebar dan kotak, diuji terhadap implementasi yang ada sebelum dihapus) |

Gate 5a: G-SWIFT, G-VIS (**tanpa rekam ulang**, karena tidak ada piksel yang berubah), `ResultGridTests` dan `GridColumnsTests` hijau, bench asap tidak lebih buruk. Ini membuktikan ekstraksi rumus lebar tanpa menyentuh renderer.

**Commit 5b. `perf(grid): an NSTableView that draws its cells, and the SwiftUI grid is gone`**

| Berkas | Perubahan |
|---|---|
| `Views/ResultGridTable.swift` (baru) | `ResultGridTable: NSViewRepresentable`, `GridTableView`, `Coordinator` (data source, diff `GridInputs`, pointer, editor overlay, popover, tooltip, menu, cache) |
| `Views/GridRowView.swift` (baru) | `GridRowPainter`, `GridPalette`, `GridRowTextCache`, `GridLineCache`, `GridPaintDiff` |
| `Views/GridHeaderView.swift` (baru) | header (§7) |
| `Views/GridAccessibility.swift` (baru) | `GridAXCell`, `GridAXRow`, `GridAXHeader`, perakitan pohon |
| `Views/ResultGrid.swift` | badan diganti (§12.1); penghapusan §17.1 |
| `Models/CellSelection.swift` | `GridGeometry` dihapus |
| Tes (baru) | `GridParityTests`, `GridAccessibilityTests`; `CellSelectionTests` memindahkan enam tes `GridGeometry` ke `GridColumnGeometry` |

Gate 5b: G-SWIFT, **G-VIS tanpa rekam ulang**, G-APP (`./app/build.sh`), daftar paritas §13 semuanya hijau, bench asap §16, smoke test VoiceOver (manual, dicatat), verdict AR.

### 17.3 Selisih terhadap dokumen lain

- **`development-plan.md` W5-T1, berkas:** `Snapshot.swift` dicoret (hanya menulis). `Models/GridValue.swift` juga dicoret (koreksi AR: pemindaian `openable` yang murah ditulis di `ArrayRows`, D-4). Ditambahkan: `Models/GridMetrics.swift` (baru), `Models/{UpdateStatements,CellEdits}.swift`, `Views/Panels.swift`, `Support/BenchMode.swift`; tes `ResultRowsTests`, `GridMetricsTests`, `GridAccessibilityTests`, `CellSelectionTests`. Baris kepemilikan §7 untuk `Models/QueryTab.swift` dan `Views/ResultGrid.swift` tetap.
- **`development-plan.md` W5-T1, verifikasi:** nama bench menjadi `scroll-30x1m`, `scroll-500x10k`, `open-500x10k`.
- **`performance-plan.md` §9.2:** D-1 dan D-2 (tabel-level, satu kolom). Ringkasan untuk ADR-0032.
- **`fase-6-data-plane.md` §17.2 dan §17.5 (W6-A1):** `cell(row:column:format:)` membawa format per panggilan; `fetched` menggantikan hitungan `preview.rows.count`; `naturalCharCounts()` ↔ `columnWidths()`; `distinctValues` sinkron di W5 dan `async` di W6; `rowCount` pada `PreviewResult` menjadi hitungan store; `Coordinator.rowsDidGrow()` menjadi pintu display link (§17.3 blueprint Fase 6).
- **`fase-4b-editor-analysis.md`:** tidak ada interaksi.
- **Koreksi AR, tambahan:** `performance-plan.md` §9.1 (pemakai ada di `ResultGrid`, `QueryTab`, `Panels`, `BenchMode`, bukan `Snapshot`) dan §14 butir 9 (§14.3 butir 1, 3, dan 6); `development-plan.md` W6-T1 mendapat P-1 sebagai verifikasi sebelum plafon `rowLimit` naik (§5.4, §15); `fase-6-data-plane.md` §17.3 (W6-A1): `rowsDidGrow` dan `WindowedRows` memakai pemetaan baris tunggal `Coordinator.resultRow(forTableRow:)`.

### 17.4 Urutan

W4-T1 → W4-T2b → **5a** → probe P-2 sampai P-6 → **5b**. `Views/ResultGrid.swift` dan `Models/QueryTab.swift` diubah W4-T1 lebih dulu; blueprint ini tidak bergantung pada isi perubahannya selain §12.4. **Koreksi AR:** 5a dimulai dengan grep ulang pembaca setelah W4-T1 (§1.4), dan P-2 membaca baseline yang direkam ulang W4-T1 (V-1). P-1 tidak ada di urutan ini; ia gerbang W6-T1 (§5.4, §15). Urutan ini sesuai rantai kepemilikan `development-plan.md` §7 (`QueryTab.swift`: W4-T1 → W4-T2b → W5-T1; `ResultGrid.swift`: W4-T1 → W5-T1). W5-T2 berjalan paralel dan tidak berbagi berkas.

## 18. Tes yang ditulis lebih dulu

Ditulis sebelum kode yang mereka uji.

**`ResultRowsTests`** (5a):

- `ArrayRows`: `cell` memotong di baris pertama dan di `prefixLimit`, menandai `truncated`, `null`, `empty`; format `uuid`, `unix_timestamp`, `json`, `text` (biner) diterapkan; `fullValue(.raw)` mengembalikan nilai asli; `rows(in:columns:)` mengembalikan kolom sumber yang diminta; baris pendek menjadi nil, bukan geser.
- `naturalCharCounts()` sama dengan rumus lama (`ResultGrid.swift:89-93`) pada kumpulan sampel yang sama, termasuk NULL = 4 dan hasil terfilter (sampel tetap dari urutan server).
- `RowReading` untuk `Array` dan untuk `ArrayRows`; `WritePlan.build` dengan kedua bentuk menghasilkan pernyataan yang sama.

**`GridMetricsTests`** (5a, 5b):

- rumus lebar: 7,2 × n + 20, jepit 84 dan 320, + 22; sisa proporsional; gutter dikurangkan sebelum pembagian; tanpa sisa tidak ada yang berubah.
- `testTheBoxesAreTheSizeTheBaselinesMeasured`: 23, 25, dan 50 untuk font sistem; snap 453,5 → 454 dan 454,5 → 455.
- `GridColumnGeometry`: `column(atX:)`, `clampedColumn`, `row(atY:)` (enam tes `GridGeometry` dipindahkan), `columns(in:)`.
- `GridPaintDiff`: seleksi bergeser satu baris menginvalidasi dua baris; tidak berubah menginvalidasi nol; perluasan `overflow` di Compact.

**`GridParityTests`** (5b, nama di §13): pointer lewat `coordinator.press/drag/release`, overlay editor dan undo, menu, tooltip, header, notifikasi format, posisi scroll saat hasil diganti, dan `testRepaintingARowTwiceGivesTheSamePixels`. Semuanya membangun `GridTableView` di jendela luar layar dan memanggil metode `Coordinator`, tanpa `NSEvent`.

**`GridAccessibilityTests`** (5b): tidak ada elemen sebelum `accessibilityChildren` dipanggil; setelahnya hanya baris dan kolom terlihat; label sel "Row 3, column 2, nama: …, changed"; `accessibilitySortDirection` header mengikuti indikator; `announcementRequested` "3 × 2 cells selected" setelah `release`; fokus VoiceOver mengatur kursor tanpa mengubah seleksi; elemen dibuang saat hasil berganti.

## 19. Risiko

| ID | Risiko | Mitigasi |
|---|---|---|
| R-1 | **Dokumen di atas 2^24 pt** (5 juta baris setelah O-12 × 21 sampai 30 pt = 105 sampai 150 juta pt). Bisa membuat isi di layar melompat dan tidak sejajar dengan hit-test di ujung bawah. Tidak tercapai di W5 (plafon 200.000). | P-1 lewat compositor sebagai gerbang W6-T1; satu fungsi pemetaan baris di 5b; mitigasi jendela baris (§5.4). |
| R-2 | **Teks di luar 0,1% piksel** meski geometri dan warna persis. | Tangga §14.2; berhenti dan lapor, tanpa rekam ulang. |
| R-3 | Mode sel `NSTableView` dianggap usang, atau `draw(_:)` sendiri tidak tergambar di jalur `cacheDisplay`. | P-4; fallback row view memakai painter yang sama (§6.6). |
| R-4 | Pembulatan tepi horizontal SwiftUI berbeda dari `.rounded()` di beberapa scene (lebar pecahan karena sisa). | P-2 menyetel; kriteria nol kegagalan layout. |
| R-5 | Fokus dan undo `NSTextField` bertabrakan dengan undo manager tab (`⌘Z` jendela tidak tersambung ke `editUndoManager`, `:546`). | Perilaku hari ini dipertahankan; `GridParityTests` mengunci satu langkah undo per sesi. Field editor `NSTextView` diberi `allowsUndo = false` bila mengganggu. |
| R-6 | Observation: `updateNSView` tidak terpicu atau terpicu berlebihan saat drag. | P-5 dan `GridInputs` `Equatable`; fallback `withObservationTracking`. |
| R-7 | Konflik gabung `ResultGrid.swift` dengan W4-T1 (banner, sumber sort). | 5a dan 5b mendarat sesudah W4-T1 (§17.4); nomor baris di blueprint ini hanya acuan `23d3ac7`. |
| R-8 | Cache melewati anggaran memori (NFR-P3). | Batas byte dua cache, dan `peak_footprint_bytes` di W5-T3; 32.768 entri `CTLine` adalah tebakan awal yang disetel di sana. |
| R-9 | Tooltip di 120 Hz. | Dibangun setelah scroll berhenti, bukan per frame (§8.5); P-6. |
| R-10 | `lineHeight` (hipotesis) tidak menghasilkan 15/13/50 untuk font sistem. | `GridMetricsTests` gagal lebih dulu; P-2 mengganti rumus. Font kode pilihan pengguna menggeser header seperti hari ini. |
| R-11 | Kuirk `isNumeric("int")` dan dua salinan yang berbeda. | Dipertahankan demi paritas piksel; dicatat ke backlog. |
| R-12 | AX: perilaku `accessibilityChildren` pada tabel mode sel dan VoiceOver nyata tidak bisa diuji otomatis sepenuhnya. | `GridAccessibilityTests` menguji pohon; smoke test VoiceOver manual dan verdict AX. |
| R-13 | Prebuild di luar main menambah kompleksitas tanpa manfaat. | Aturan simpan §9.4: dihapus bila bench tidak membuktikan ≥ 10%. |

## 20. Untuk pemeriksa dan untuk ADR-0032

**Pemeriksa (AR, SR, AX, UX, TD)** perlu memutuskan:

1. **D-1 dan D-2.** Penyimpangan dari `performance-plan.md` §9.2 (gambar di level tabel, satu kolom) beserta bukti §1.1 dan fallback §6.6.
2. **P-1 sebagai gerbang 5b.** Apakah 5b boleh mulai sebelum P-1, dan siapa yang memutuskan bila gagal.
3. **Tangga teks §14.2** dan aturan berhenti-dan-lapor.
4. **Perubahan kecil §14.3**, terutama chip satu baris dan banner yang tidak tergeser.
5. **D-6 dan D-7**: `RowReading` dan `displayedRows` yang bertahan sampai W6.
6. **D-10 dan §11**: AX dibuat saat ditanya, dan kursor yang tidak digambar.

**ADR-0032** (W5-D) memuat: seam `ResultRows` dan `CellText` (D-3, D-4); tabel yang menggambar sendiri (D-1, D-2) dengan bukti geometri §1.1; cache dan aturan prebuild (D-12); aksesibilitas saat ditanya (D-10); model kursor (D-11); daftar §14.3; dan hasil probe P-1 sampai P-6.

## Verdict architect-reviewer

**Verdict: approved with corrections applied** (30 Sep 2026, W3-A1, satu putaran). Koreksi di bawah sudah ditulis ke badan blueprint dan ditandai "Koreksi AR". W5-T1 boleh mulai dari versi ini sesudah W4-T1 dan W4-T2b mendarat.

### Klaim yang diperiksa di kode

| Klaim | Hasil | Bukti |
|---|---|---|
| Kotak sel 23 pt (data) dan 25 pt (gutter), dipusatkan, dengan snap `.rounded()` per tepi | **Benar** | Sidecar: normal 396/397 untuk dasar baris 397, tall 454/455 untuk 457, compact 350/351 untuk 349. Aritmetika pusat ± 11,5 dan ± 12,5 cocok di ketiganya. |
| Header 50 pt, banner 21 pt | **Benar**, dengan catatan | `grid-dark`: aturan 25 dan 75, `bodyTop` 76. `grid-sorted` dan juga `grid-kinds*` (scene itu bersort, `Snapshot.swift:615-635`): `bodyTop` 97. Karena itu V-1 di W4-T1 menyentuh sebelas pasang baseline, bukan hanya `grid-sorted`. |
| 28 pembaca: `ResultGrid` 18, `QueryTab` 7, `Panels` 1, `BenchMode` 2; `Snapshot` hanya menulis | **Benar di `23d3ac7`** | grep `preview(\?)?\.rows|displayedRows|\.rows\b` di `app/Sources`. Yang terlewat bukan di kode hari ini: pembaca baru dari W4-T1, dan dua berkas tes (`PanelDefaultTests`, `StoppedRunTests`). |
| Gate `scroll-30x1m` menuntut dokumen 25 juta pt benar di ujung bawah | **Salah** | `ScrollDriver` menggulir 3 s × 3.000 pt/s dari atas (`BenchMode.swift:57`, `:489-490`), sekitar 9.000 pt. Plafon produk 200.000 (`AppModel.swift:2558`). |
| 48 baseline grid | **Salah** | `gridScenes()` (`VisualParityTests.swift:707-741`) menghasilkan 30 tangkapan grid dan 2 `grid-json`; `__Baselines__` berisi 32 PNG `grid*`. |
| Dobel-klik: openable membuka pembaca, "selain itu" editor | **Salah** | Sel openable dengan panel berdiri tidak melakukan apa pun (`ResultGrid.swift:790`). |
| Chip tipe mungkin dibungkus, tidak terbukti | **Kode menjawabnya** | `Chip` adalah `Text` tanpa `lineLimit` (`Theme.swift:612-624`) di dalam `frame(width:)`, jadi ia dibungkus bila kolom lebih sempit. Aturan pemenggalan persisnya tetap urusan P-3. |
| `copiesOnScroll = true` mengaktifkan jalur strip | **Salah** | Properti ini usang sejak SDK macOS 11 dan diabaikan untuk hierarki berlapis layer. Dihapus dari §5.1. |
| Enam tes `GridGeometry`; dua daftar `isNumeric` berbeda | **Benar** | `CellSelectionTests.swift:165-220`; `ResultGrid.swift:1174`, `VisualParityTests.swift:583`. |
| Gate menangkap lewat `cacheDisplay` pada `NSHostingView` di jendela `.titled` | **Benar** | `VisualParityTests.swift:519-580`. Scene editor sudah membuktikan bahwa `NSScrollView` di dalam `NSViewRepresentable` tertangkap, jadi separuh P-4 sudah terjawab. |

### Keputusan atas pertanyaan di §20 dan brief

1. **D-1 dan D-2 (gambar di `GridTableView.draw(_:)`, satu `NSTableColumn`): diterima sebagai bawaan. Fallback row view tetap fallback.**
   - Scroll: tidak ada `NSView` per baris, AppKit hanya meminta area yang baru terbuka, dan biaya per frame sekitar satu baris × kolom terlihat saat fling vertikal 3.000 pt/s. Row view selebar dokumen (75.000 pt pada 500 kolom) justru membawa backing store yang tidak dibutuhkan.
   - Seleksi: seleksi grid adalah blok sel. `NSTableRowView.isSelected` hanya tahu baris, jadi row view tidak memberi apa pun. Invalidasi rect yang tepat (`GridPaintDiff`) lebih murah di tingkat tabel.
   - Header: `GridHeaderView` dan tubuh membaca `GridColumnGeometry` yang sama, dan sinkronisasi horizontal header adalah bawaan `NSScrollView` untuk `NSTableView`. Dengan satu kolom, header dan isi tidak mungkin menyimpang karena tidak ada 501 objek kolom yang harus disinkronkan.
   - Gutter: ikut tergeser seperti hari ini. Gutter beku di W10 tetap mungkin di kedua tuan rumah lewat `NSScrollView.addFloatingSubview(_:for: .horizontal)`, jadi keputusan ini tidak menutup jalannya.
   - AX: tidak dipengaruhi pilihan tuan rumah, karena elemen sel harus ditulis sendiri di kedua kasus. Yang dipengaruhi adalah AX bawaan tabel mode sel yang mengaku satu kolom; koreksinya di §11.2.
   - Koreksi yang dibutuhkan agar D-1 aman: D-15 (seleksi baris dan keyboard `NSTableView` dimatikan, fokus), `rowSizeStyle = .custom`, `cornerView = nil`, tanpa tooltip ekspansi, dan kriteria P-4 tambahan (header tidak menumpuk isi, tidak ada latar header sistem).
2. **P-1: bukan gerbang 5b, menjadi gerbang W6-T1.** Geometri AppKit (`Double`) persis. Yang mungkin rusak hanya komposisi WindowServer, dan hanya di atas 2^24 pt, yang tidak tercapai di W5. Kriteria lulus dan gagal kini konkret (§15): tiga pemeriksaan per offset, dan bukti hanya sah lewat tangkapan compositor. Mitigasi minimal adalah jendela baris (`WindowedRows`, paling banyak `floor(2^24 / rowHeight)` baris per jendela). Menjepit tinggi baris ditolak, dan virtualisasi scroll hanya bila pemilik menolak jendela baris. 5b wajib menyalurkan semua konversi baris lewat `Coordinator.resultRow(forTableRow:)`.
3. **Tangga teks §14.2 dan berhenti-dan-lapor: diterima.** Tidak ada kode V untuk teks, dan rekam ulang milik pemilik.
4. **Perubahan kecil §14.3: diputuskan per butir di §14.3.** Butir 1 bersyarat, 2 dengan aturan yang dipersempit, 3 dan 5 diizinkan, 4 (chip dipotong) **tidak** diizinkan tanpa kode V, dan butir 6 (fokus) ditambahkan. Kode V hanya untuk piksel; butir yang diizinkan masuk `performance-plan.md` §14 butir 9.
5. **D-6 dan D-7: diterima.** `RowReading` kecil dan membuat tes lama tetap utuh. `displayedRows` bertahan sampai W6-T1, dan daftar tes pembacanya dilengkapi.
6. **D-10 dan §11: diterima dengan koreksi.** Pelengkap pohon AX (kolom, `accessibilityCell(forColumn:row:)`, header kolom, rentang indeks), identitas elemen yang stabil dengan pemangkasan ke viewport ± 1 halaman, label yang dihitung saat ditanya, dan notifikasi hanya di `release()`. Biayanya sekitar baris × kolom terlihat selama ada klien AX, nol tanpa klien. Kursor yang tidak digambar sesuai P-24.
7. **Pemecahan 5a/5b dan urutannya: diterima dengan koreksi.** 5a tetap tanpa perubahan piksel hanya bila renderer SwiftUI tidak memakai `CellText` (koreksi di §3.3). Kriteria selesai 5a adalah aturan grep, bukan tabel `23d3ac7`, karena W4-T1 menambah pembaca. P-2 dijalankan terhadap baseline pasca-V-1.

### Keputusan lain

- **Pembatalan overlay editor dipersempit (§8.3).** Aturan awal membuang teks yang sedang diketik setiap 200 ms selama streaming. Ini jalur kehilangan teks pengguna, jadi masuk kategori risiko tinggi. Sekarang overlay hanya berakhir bila kunci selnya tidak lagi menunjuk baris yang sama.
- **Dobel-klik diperbaiki** di §1.3 dan §8.3 mengikuti `:790`.
- **Salinan di luar main** menulis papan klip hanya bila `changeCount` belum berubah (§8.7).
- **`openable` tanpa `looksLikeJSON`** di jalur `cell` dan potongan di batas `Character` (D-4).
- **Daftar paritas §13 ditambah** butir 24 sampai 30: menu tubuh dan klik kanan, tooltip header, Explain di grid, fokus dan ⌘C, pergantian appearance, overlay saat streaming, dan IME. Explain dan IME tidak tercakup gate dan diperiksa tangan.
- **Dinilai berlebihan tetapi tidak diblokir:** dua cache (`GridRowTextCache` dan `GridLineCache`) di atas `ArrayRows`, dan di W6 di atas cache halaman `StoreRows`, berarti tiga lapis. Rencana §9 butir 5 dan `fase-6` §17.2 memintanya, jadi ia tetap. W5-T3 memutuskan penggabungan ke satu cache bila hit rate cache teks tidak mengubah p99. Prebuild di luar main sudah punya aturan simpan yang benar (§9.4). `CellFlags.numeric/openable` tetap di tipe sebagai kontrak W6, tetapi diisi semurah mungkin.
- **Dipertahankan meski menambah kode:** `GridCursor` (P-24), `GridPaintDiff` (bisa diuji tanpa jendela), `GridCommands` (header dan tabel tidak mengenal `AppModel`), dan probe P-5/P-6.

### Pemeriksaan terhadap sumber yang mengikat

- **`performance-plan.md` §9:** semua butir terpenuhi. Butir 2 menyimpang (D-1/D-2) dengan alasan geometri §1.1, dan butir 5 (prebuild) bersyarat bench. Daftar paritas rencana tercakup §13.
- **FR-GRID-01:** seam `ResultRows` di 5a dan tabel di 5b. Terpenuhi.
- **FR-GRID-07 (dasar):** label per sel, status sort header (`accessibilitySortDirection`), dan pengumuman "N × M" saat seleksi selesai. Terpenuhi dengan pelengkap §11.2.
- **NFR-P4:** skenario dan batasnya di §16. Angka resmi milik W5-T3.
- **NFR-V:** lapis 1 dan 2 persis, lapis 3 marker, lapis 4 teks lewat tangga. Tidak ada rekam ulang di W5, dan chip dipotong tidak masuk tanpa kode V.
- **P-24:** kursor dimodelkan, tidak digambar, tanpa tombol. Terpenuhi.
- **`tablepro-design-audit.md` §5:** rekomendasi 2 (VoiceOver) dipenuhi dasarnya. Rekomendasi 1, 3, 4, 5, dan 6 milik W10, dan kontras memang satu-satunya non-paritas yang disengaja. Daftar paritas visual audit tercakup gate, kecuali `explain` dan `grid-loading` yang tidak ada di gate.
- **`fase-6-data-plane.md` §17:** `cell` dan `fullValue` hanya di main cocok dengan cache halaman `StoreRows`. Selisih format per panggilan, `distinctValues` sinkron, dan `rowsDidGrow` tercatat di §17.3 untuk W6-A1, ditambah aturan pemetaan baris.
- **`development-plan.md` §7:** urutan kepemilikan `QueryTab.swift` dan `ResultGrid.swift` dipatuhi. W5-T1 tidak menyentuh `app/Generated/`.

### Perubahan rencana yang dibutuhkan (untuk orkestrator)

- `performance-plan.md` §9 butir 1: "28 pemakai di `ResultGrid.swift`, `QueryTab.swift`, dan `Snapshot.swift`" menjadi `ResultGrid`, `QueryTab`, `Panels`, `BenchMode`, ditambah pembaca dari W4-T1; `Snapshot` hanya menulis.
- `performance-plan.md` §9 butir 2: gambar di `GridTableView.draw(_:)` mode sel dengan satu `NSTableColumn`, `GridRowPainter` di `GridRowView.swift`, tanpa `viewFor` dan tanpa `NSTableRowView` kecuali fallback P-4.
- `performance-plan.md` §14 butir 9: "nilainya penuh" menjadi "penuh hingga 8.192 unit UTF-16"; tambah banner yang tidak ikut tergeser horizontal (bila D-13 jalan pertama), overlay editor yang berakhir saat urutan berubah, dan klik grid yang mengambil fokus keyboard.
- `development-plan.md` W5-T1, berkas: tambah `Models/GridMetrics.swift` (baru), `Models/{UpdateStatements,CellEdits}.swift`, `Views/Panels.swift`, `Support/BenchMode.swift`, tes `ResultRowsTests`, `GridMetricsTests`, `GridAccessibilityTests`, `CellSelectionTests`; coret `Support/Snapshot.swift` dan `Models/GridValue.swift`. Verifikasi: nama bench `scroll-30x1m`, `scroll-500x10k`, `open-500x10k`.
- `development-plan.md` §7: baris `Views/ResultGridTable.swift`, `GridRowView`, `GridHeaderView` ditambah `Views/GridAccessibility.swift` (W10-T1 menyentuhnya).
- `development-plan.md` W6-T1, verifikasi: P-1 lewat compositor (dijalankan pemilik atau di sesi eksklusif) sebelum `productRowLimitCeiling` naik ke 5.000.000; gagal berarti `WindowedRows` masuk W6-T1.
- `fase-6-data-plane.md` §17.2, §17.3, §17.5 (W6-A1): butir di §17.3 blueprint ini.
- Rekomendasi, butuh persetujuan pemilik (§0.5): 5a merekam scene tambahan dari renderer SwiftUI yang masih ada, `explain-*` dan satu scene grid berisi baris baru, tab, spasi awal, karakter kontrol, dan chip yang dibungkus. Tambahan ini tidak mengubah baseline yang ada, dan membuat P-3 menjadi bagian gate di 5b alih-alih probe sekali pakai.

### Risiko terbuka (tidak memblokir)

- **Teks (R-2)** tetap satu-satunya lapis gate yang tidak bisa dijamin di atas kertas. Tangga §14.2 dan aturan berhenti-dan-lapor sudah benar.
- **Mode sel `NSTableView`** disebut usang di dokumentasi Apple meski API Swift-nya tidak ditandai. Bila P-4 gagal karena mesin tabelnya, bukan karena menggambar, pilihan yang menjaga satu konteks cat D-1 adalah `NSView` biasa sebagai dokumen dengan header sebagai floating subview (`addFloatingSubview(_:for: .vertical)`), sebelum row view. Implementer mencatatnya di laporan P-4 bila terjadi.
- **Label AX yang memuat "Row r, column c"** bersama rentang indeks bisa membuat VoiceOver menyebut posisi dua kali. Label diminta FR-GRID-07, dan rentang indeks dibutuhkan navigasi tabel. Pemeriksa AX memutuskan dari smoke test.
- **Overdraw responsive scrolling** memanggil `draw(_:)` di luar area terlihat. Biaya gambar harus tetap sebanding dengan `dirtyRect`, dan bench W5-T3 yang membuktikannya.
