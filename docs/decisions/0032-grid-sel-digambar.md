# 0032 — Grid dengan sel yang digambar: satu `NSTableView`, satu kolom, `draw(_:)` di level tabel (mengamandemen 0003)

- **Status:** Diterima. Implementasi mendarat di W5-T1 (`7b44e29` seam, `473f626` kode, `aac1518` baseline V-13,
  `2c7141f` perbaikan putaran 1, `3938971` tes, `13ea642` baseline). Putaran tinjau 2 memberi verdict **APPROVE** tanpa
  temuan blocking. Yang belum terukur dicatat di bagian Konsekuensi ("Belum ada"), bukan disembunyikan.
- **Tanggal:** 6 Okt 2026 (W5-D, perf-parity Fase 5)
- **Konteks instruksi:** `docs/architecture/development-plan.md` §4 (jadwal ADR, baris 0032) dan W5-D;
  `docs/architecture/blueprints/fase-5-grid.md` beserta verdict architect-reviewer (D-1 sampai D-15, §14, §15, §20);
  `docs/architecture/performance-plan.md` §9; PRD `docs/architecture/prd-performance-and-parity.md` FR-GRID-01,
  FR-GRID-07, NFR-P4, NFR-V, V-13 (§6.5), P-24.
- **Mengamandemen:** ADR-0003 (grid memakai `NSTableView`). Pilihan `NSTableView` dipertahankan; yang diubah adalah
  cara tabel itu dipakai (sel tidak lagi `NSTextField` atau view per sel, melainkan digambar), dan syarat revisi
  ADR-0003 tentang grid kustom belum terpicu (menunggu bench W5-T3). ADR-0003 diberi catatan di bagian atas; badannya tidak ditulis ulang.
- **Berhubungan dengan:** ADR-0030 (data plane, W6-D) yang mengisi `StoreRows` di belakang seam yang ditetapkan di sini;
  ADR-0033 (V-12, editor) sebagai contoh pola V-code yang sama.
- **Catatan penomoran:** D-1 sampai D-15 di ADR ini adalah nomor dari blueprint Fase 5. Nomor D yang sama di blueprint
  Fase 4B dan Fase 6 adalah keputusan yang berbeda.

## Konteks

ADR-0003 memilih `NSTableView` lewat `NSViewRepresentable` untuk menggantikan `LazyVStack` SwiftUI, dengan alasan
virtualisasi kolom, reuse sel, dan aksesibilitas bawaan. Ia tidak menetapkan bagaimana sel diisi. Pengukuran W1
(`docs/benchmarks.md`, sumbu 4) pada grid SwiftUI yang ada: `scroll-30x1m` hitch 910 ms/s dan p99 frame 262,7 ms;
`scroll-500x10k` hitch 2.953 ms/s, p99 8.468 ms, dan 2.017 ms sampai tergambar. Sumbernya: tiap `body` membangun
`Array(displayedRows.enumerated())`, dan tiap sel membaca `UserDefaults` (format kolom), mem-parse JSON (`isOpenable`),
lalu membuat view dengan modifier sendiri.

`performance-plan.md` §9.2 menurunkan ADR-0003 menjadi satu `NSTableColumn` per kolom data dan `NSTableRowView` yang
menggambar per baris. Pembacaan blueprint Fase 5 terhadap kode dan baseline menemukan tiga hal yang membuat rencana itu
tidak cukup:

1. **Sel hari ini bukan setinggi barisnya (blueprint §1.1).** Kotak sel dibaca dari sidecar baseline: 23 pt untuk sel
   data (`mono12` line 15 + padding 4+4) dan 25 pt untuk gutter (`code(10.5)` line 13 + padding 6+6), dipusatkan di baris
   setinggi 21, 25, atau 30 pt. Di Compact (21 pt) kotak itu keluar dari barisnya 1 pt (data) dan 2 pt (gutter), dan urutan
   cat baris N+1 sesudah N menentukan warna di pita tumpang tindih. Di Tall (30 pt) celahnya 3,5 dan 2,5 pt dan separator
   baris terakhir terukur 454 (data) dan 455 (gutter). Gate paritas visual menuntut angka yang sama.
2. **Dengan 500 kolom, satu `NSTableColumn` per kolom mahal.** 501 objek kolom dan sinkronisasi lebarnya (TablePro
   mengukurnya kuadratik) tidak membeli apa pun, karena hari ini tidak ada resize, reorder, atau autoresize kolom.
3. **Dokumen vertikal sangat tinggi.** 1 juta baris × 25 pt melewati 2^24 pt, tempat presisi Float32 di compositor Core
   Animation turun ke 2 pt. Koreksi AR: di W5 batas ini tidak tercapai (plafon produk 200.000 baris sampai Fase 6, O-12,
   paling tinggi sekitar 6 juta pt, dan fling `scroll-30x1m` menempuh sekitar 9.000 pt), jadi ini menjadi gerbang W6-T1.

Selain itu banyak pembaca `preview.rows` atau `displayedRows` tersebar di kode produksi, dan W6 akan mengganti array itu
dengan store Arrow. Grid harus berhenti bergantung pada array sebelum W6, tanpa mengubah piksel.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Tempat menggambar | **`GridTableView.draw(_:)` di level tabel, tanpa `NSView` per baris** | Satu konteks gambar untuk baris berurutan, sehingga urutan cat dan luapan Compact sama dengan SwiftUI tanpa usaha; tanpa alokasi view saat scroll; tanpa backing store selebar 75.000 pt per baris pada 500 kolom | `draw(_:)` dan penghitungan invalidasi ditulis sendiri; teks tidak lagi lewat SwiftUI |
| | `NSTableRowView` per baris (rencana `performance-plan.md` §9.2) | Lebih dekat ke idiom AppKit | Terpotong di batas barisnya: luapan Compact baris terakhir jatuh di luar semua row view; backing store per baris sangat lebar. **Dijadikan fallback** (blueprint §6.6, bila P-4 gagal) |
| | View-based dengan `NSTextField` per sel (ADR-0003 apa adanya) | Mudah | 30 kolom × baris terlihat jadi ratusan view yang dibuat dan di-layout saat scroll; ini biaya yang ADR-0003 sendiri tunjuk sebagai dominan |
| Jumlah kolom | **Satu `NSTableColumn`** selebar jumlah kolom (`minWidth` 1, `maxWidth` `.greatestFiniteMagnitude`; bawaan 1000 akan menjepit), geometri di `GridColumnGeometry` | Gate 500 kolom ≤ 30 ms bebas dari biaya objek kolom; geometri murni dan bisa diuji | Tidak bisa memakai resize atau reorder kolom bawaan AppKit (hari ini memang tidak ada) |
| | Satu `NSTableColumn` per kolom data | Idiom AppKit | 501 objek dan sinkronisasi lebar; tidak membeli apa pun |
| Sumber data | **Seam `ResultRows` (protokol) dengan `ArrayRows` di W5 dan `StoreRows` di W6** | Grid tidak berubah saat W6; penyebaran pembaca array berhenti | Satu lapis tak langsung; array tetap hidup sampai W6 (D-7) |
| | Grid membaca `displayedRows` langsung | Tanpa kode baru | Dua kali menyentuh semua pembaca (W5 lalu W6) dan tak ada tempat bagi nilai 10 MB agar tidak jadi `String` per sel |
| Grid kustom penuh (Metal/Core Animation) | Opsi 3 di ADR-0003 | Kendali penuh | Aksesibilitas dan keyboard ditulis sendiri. **Belum terpicu**: syarat revisi ADR-0003 (60 fps pada 500k × 30) baru diputuskan oleh bench W5-T3, dan opsi ini tidak diperlukan selama `NSTableView` yang menggambar sendiri sudah memberi kendali piksel |
| Tanpa perubahan | Mempertahankan grid SwiftUI | Tanpa risiko | Hitch 910 sampai 2.953 ms/s; tidak mencapai NFR-P4 |

## Keputusan

**Grid adalah satu `NSTableView` mode sel dengan satu `NSTableColumn`, yang menggambar seluruh isinya sendiri di
`GridTableView.draw(_:)` (gutter, stripe, wash seleksi dan staged, titik staged, pemisah, dan teks lewat `CTLine`),
membaca data lewat seam `ResultRows`, dengan header `GridHeaderView` yang juga menggambar sendiri. `NSTableView` tetap
pilihannya (ADR-0003); yang berubah adalah bahwa sel tidak lagi view.**

Keputusan yang mengikat, nomor dari blueprint Fase 5 dan diperiksa terhadap `app/Sources/QueryHive`:

1. **D-1, D-2.** `draw(_:)` di-override tanpa memanggil `super`; `selectionHighlightStyle = .none`. `GridRowPainter`
   (struct murni, `Views/GridRowView.swift`) mengecat; baris `r-1…r+1` ikut dicat bila kotak sel meluap. Aturan snap
   geometri yang dibaca dari angka: tepi atas dan bawah kotak masing-masing dibulatkan `.rounded()` (setengah menjauhi
   nol), tinggi adalah selisihnya. Nama berkas `GridRowView.swift` dipertahankan supaya kepemilikan W10-T2 tidak berubah.
2. **D-3.** `ResultRows.cell(row:column:format:)` menerima format per panggilan; tidak ada protokol invalidasi, dan
   `StoreRows` menurunkan `formatSignature` dari argumen. `ArrayRows` menjawab tiga hal berbeda: di luar jangkauan
   `flags: []`, NULL `[.null]`, string kosong `.empty`. `fullValue` mengembalikan nilai mentah tanpa batas.
3. **D-4.** `CellText.text` adalah prefix teks tampilan, baris pertama saja, paling banyak 1.024 unit UTF-16, berhenti di
   batas `Character` (tidak pernah di tengah pasangan surrogate atau klaster). Flag: `null`, `empty`, `truncated`,
   `numeric`, `openable`. `numeric` dan `openable` belum punya pemakai di W5 (kontrak untuk W6); `ArrayRows` mengisinya
   semurah mungkin, tanpa `GridValue.looksLikeJSON` karena ia memangkas seluruh string. Tindakan dobel-klik tetap
   memanggil `GridValue.isOpenable` penuh.
4. **D-5.** Format kolom dibaca sekali per kolom per perubahan lewat `ColumnFormatStore.didChange`, bukan
   `UserDefaults` per sel; parse JSON per render hilang.
5. **D-6, D-7.** `RowReading` (`func row(at:) -> [String?]?`) dipakai `WritePlan`, `UpdateStatements`, dan
   `CellEdits.fill/paste`; `Array<[String?]>` ikut memenuhinya sehingga tes lama tidak berubah. `QueryTab.displayedRows`
   tetap sampai W6 sebagai penyimpan `ArrayRows`; semua pembaca produksi lewat `tab.result`.
6. **D-8, D-9.** Perilaku pointer dan menu hidup di `Coordinator` sebagai metode biasa (`press`, `drag`, `release`,
   `menuItems`) supaya tes memanggilnya tanpa `NSEvent`. Editor sel adalah `NSTextField` overlay, dengan jalur
   `typeCellEdit`/`endCellEdit` yang ada, jadi undo tetap satu langkah per sesi.
7. **D-10.** Aksesibilitas dibuat saat ditanya: elemen sel adalah `NSAccessibilityElement` (bukan `NSView`), dibuat
   hanya ketika AppKit memanggil `accessibilityChildren`/`accessibilityRows`, untuk baris dan kolom terlihat. Tanpa
   VoiceOver tidak ada satu objek pun.
8. **D-11.** Kursor sel dimodelkan (`GridCursor` di `CellSelection.swift`, `QueryTab.cellCursor`) tetapi tidak digambar
   dan tidak diikat ke tombol; hanya AX yang memakainya, dan W10-T1 yang mengikatnya (P-24).
9. **D-12.** Cache berbatas dua generasi: teks tampilan per baris (`GridRowTextCache`, viewport ± 2 halaman, ≤ 16 MB) dan
   `CTLine` (`GridLineCache`, dua peta `current`/`previous`, 32.768 entri atau 8 MB, kunci `(row, column)` dengan
   `epoch`). **Aturan prebuild:** prebuild `CTLine` di luar main dipertahankan hanya bila bench membuktikan manfaat
   ≥ 10% pada `scroll-500x10k` (p99 atau hitch); selain itu dihapus. Saat ini ia ada di
   `ResultGridTable.prebuildIfWorthIt()` di balik `#if GRID_PREBUILD` (mati secara bawaan), menunggu bench W5-T3.
10. **D-13, D-14, D-15.** Banner sort tetap SwiftUI di atas tabel (tidak ikut tergeser horizontal); popover filter,
    pembaca sel, dan lembar tetap SwiftUI dihosting di `NSPopover`. Seleksi baris milik `NSTableView` dimatikan
    (`selectionIndexesForProposedSelection` mengembalikan himpunan kosong, `selectAll(_:)` tanpa efek, panah tidak
    diteruskan ke `super`), dan `shouldShowCellExpansionFor` mengembalikan `false`.
11. **Perubahan perilaku kecil yang diizinkan tanpa kode V (blueprint §14.3):** banner sort tidak tergeser; overlay
    editor dibatalkan saat urutan atau bentuk hasil berubah (hari ini sesi yang tersisa menulis ke baris yang salah);
    tooltip dibatasi 8.192 unit UTF-16; `NSTextField` menggantikan `TextField`; klik pada grid memindahkan fokus dari
    editor ke grid dan ⌘C menyalin blok. Chip tipe di header **tetap dibungkus** (memotongnya mengubah piksel dan butuh
    kode V). Header tooltip memakai string grid lama, bukan blueprint §12.4, karena klik header merutekan sort ke
    server lebih dulu (`AppModel.setSort`).
12. **V-13.** Migrasi renderer terdaftar sebagai V-13 (PRD §6.5), dengan baseline grid direkam ulang di commit sendiri
    (keputusan pemilik O-10). Lihat Alasan 4.

## Alasan

1. **Geometri hanya bisa direproduksi bila baris berurutan dicat ke satu konteks.** Luapan Compact, urutan cat, dan
   celah Tall semuanya konsekuensi dari kotak sel 23/25 pt di baris 21/25/30 pt. `draw(_:)` di level tabel mendapatkannya
   otomatis; `NSTableRowView` terpotong di batas barisnya. Bukti terukurnya: geometri kolom dan tinggi header direproduksi
   (header 50 pt, `bodyTop` 76), dan G-VIS lulus 17/0 pada pohon yang di-commit.
2. **Satu kolom menghapus biaya objek tanpa kehilangan fitur.** Tidak ada resize, reorder, atau autoresize hari ini;
   `GridColumnGeometry` (lebar, offset kumulatif, `column(atX:)` lewat pencarian biner) murni dan diuji
   (`GridMetricsTests`, 13 tes).
3. **Seam mengakhiri penyebaran pembaca array.** Semua pembaca produksi pindah ke `ResultRows` di commit 5a, yang tidak
   mengubah piksel karena renderer belum diganti; 5b mengganti renderer di atas seam yang sama. Itu sebabnya dua commit
   terpisah, dan W6 hanya menambah `StoreRows`.
4. **Teks digambar `CTLine`, dan selisihnya terhadap SwiftUI bersifat kategoris, bukan perilaku.** Probe teks W5-T1 5b
   dikerjakan secara mekanis, bukan diperdebatkan, dalam tiga sapuan:
   - `shouldSmoothFonts`, `shouldSubpixelPositionFonts`, dan `shouldSubpixelQuantizeFonts`, masing-masing sendiri dan
     ketiganya bersama, **tidak mengubah satu piksel pun** pada `grid-counted-dark` (5,158% di kedua sisi);
     `setShouldAntialias(false)` memperburuk (5,790%).
   - Pertanyaan font tertutup: `monospacedSystemFont(ofSize:12,weight:.regular)`, `systemFont(ofSize:12)` dengan
     `fontDescriptor.withDesign(.monospaced)`, dan `systemFont(ofSize:12,weight:.regular)` dengan desain yang sama
     semuanya menghasilkan `.AppleSystemUIFontMonospaced-Regular`, jadi grid sudah menggambar muka yang dipakai baseline.
   - Delapan kombinasi dari {baseline fraksional, x teks bulat, alpha tinta penuh} pada `grid-counted-dark`: 5,158
     (tanpa), 6,646, **4,998** (x bulat), 6,664, 5,526, 6,893, 5,457, 6,924. Terbaik 4,998% terhadap anggaran 0,1%;
     tak satu pun mendekati dalam dua orde besaran.
   - Satu temuan yang layak disimpan: `Text(...).foregroundStyle(Tone.ink.opacity(0.9))` mengomposit glyph-nya **sekali di
     dalam layer**, sehingga tepinya tidak pernah bercampur dengan baris di belakangnya, sedangkan CoreText mengomposit
     warna yang sama per `CTLineDraw`. Karena itu alpha tinta sel adalah satu-satunya tuas yang menggerakkan angka,
     tetapi tidak gratis: tinta 1,0 membuat teks terlihat lebih terang dan menggagalkan lapis warna.
   - Sisanya kategoris: teks SwiftUI ber-antialias **subpiksel** (pinggiran warna di baseline, puncak `#acadb3`),
     `CTLineDraw` ke bitmap `cacheDisplay` ber-antialias **grayscale** (puncak `#e8e8ea`), dan tidak ada yang dapat
     dijangkau dari `draw(_:)` yang menjembataninya. Harness mandiri yang menggambar satu sel dan mencari origin terbaik
     ±5 px juga tidak mereproduksi baseline, sehingga crop yang dinilai gate tidak bisa direproduksi di luar view asli.
   - V-13 mencatat tiga sebab terukur di luar kendali `draw(_:)`: tepi kolom fraksional membulat ke piksel lain dari
     layout `HStack` yang menerus (56 assertion layout, 1 pt), antialiasing teks (30 assertion piksel), dan chevron
     sort 10×6 px terhadap 6×4 dari `Image(systemName:)` (14 assertion marker).
   Karena itu blueprint §14 menyediakan tangga penyetelan dengan aturan berhenti-dan-lapor, dan pemilik memutuskan
   merekam ulang di bawah V-13 daripada membiarkan G-VIS merah (alasan: tangga §14.2 sudah habis dengan angka).
5. **Perekaman ulang kedua tidak menambah kode V.** Tinjauan putaran 1 menemukan dua regresi yang perekaman V-13
   menyembunyikan: NULL tergambar sebagai string kosong (kini miring `nullDisplay`) dan teks panjang tidak dipotong
   (kini `CTLineCreateTruncatedLine` dengan `…` di ujung pada lebar berhingga mana pun). Penandanya: 12 scene melewati
   anggaran 0,1% pada 14 assertion; 22 baseline grid bergerak dan 16 baseline editor kembali **identik byte**, bukti
   bahwa perekaman hanya menulis yang digerakkan perbaikan. Piksel bergerak menuju target V-13 (paritas isi dengan grid
   lama): menukar baseline pra-V-13 kembali (`target/run/visual_oldbase.log`) tidak menghasilkan kegagalan penanda isi sel,
   hanya kejenuhan `header[2].label`, penyebab yang sudah terdaftar di V-13.
6. **Aksesibilitas saat ditanya** memberi efek yang dikehendaki audit TablePro §5 ("dipasang hanya setelah klien AX
   menempel") tanpa mendeteksi klien. Kode TablePro (AGPL) dibaca sebagai ide; tidak ada kode, aset, atau string yang disalin.

## Konsekuensi

### Positif

- **POS-001.** Jalur gambar tidak lagi membaca `UserDefaults`, tidak mem-parse JSON, dan tidak membuat view per sel;
  `NSView` per baris dan per sel tidak ada. Sasaran: hitch 910 sampai 2.953 ms/s turun ke target NFR-P4 (belum diukur, lihat Belum ada).
- **POS-002.** Seam `ResultRows` memutus ketergantungan grid pada array; W6 mengganti sumbernya tanpa menyentuh renderer.
- **POS-003.** Geometri (`GridMetrics`, `GridColumnGeometry`) dan perilaku pointer (`Coordinator.press/drag/release`)
  adalah nilai murni dan metode biasa yang bisa diuji tanpa `NSEvent`. Tes baru: `GridMetricsTests` 13, `ResultRowsTests` 10,
  `GridAccessibilityTests` 10, `GridParityTests` 11 (44 tes); swift test 595, 5 dilewati, 0 gagal pada `13ea642`.
- **POS-004.** Nilai sel 10 MB tidak pernah menjadi `String` per sel di jalur gambar (D-4).
- **POS-005.** Syarat revisi ADR-0003 (grid kustom bila `NSTableView` gagal 60 fps) belum terpicu, dan jalur ke grid
  kustom tidak ditutup: renderer sudah berada di balik seam dan `draw(_:)`.

### Negatif

- **NEG-001.** Teks grid kini ber-antialias grayscale, bukan subpiksel; ini perbedaan visual yang disengaja dan terdaftar
  (V-13). Tidak ada tuas yang menutupnya (5,158% terukur, terbaik 4,998% terhadap 0,1%).
- **NEG-002.** Commit `473f626` sendirian membuat G-VIS **merah** karena masih membandingkan baseline pra-migrasi yang
  digantikan `aac1518`; bisect berhenti di sana. Ketiga commit hijau di `7e32f5f`. Dipilih daripada squash agar perubahan
  visual bisa ditinjau dan dibatalkan sendiri (O-10).
- **NEG-003.** Banyak yang dulu gratis dari SwiftUI dan AppKit kini ditulis sendiri dan harus diuji: hit-testing sel,
  invalidasi (`GridPaintDiff.invalidatedColumns`), tooltip (registrasi dan batas), pohon AX, dan editor overlay.
  Tinjauan putaran 1 menemukan tujuh temuan blocking di area ini (pointer, rekursi `beginEdit`/`commitEdit`, `stagedKeys`,
  invalidasi, AX, tooltip), semuanya diperbaiki; itu ukuran biayanya.
- **NEG-004.** Perubahan perilaku kecil terlihat pengguna meski piksel sama (butir 11 Keputusan), dan harus dicatat di
  `performance-plan.md` §14 butir 9 (butir 1, 3, dan 6 dari §14.3).
- **NEG-005.** `QueryTab.displayedRows` dan `ArrayRows` bertahan sampai W6, jadi memori Swift untuk array tidak turun di W5.
- **NEG-006.** Satu kolom `NSTableColumn` berarti fitur kolom bawaan AppKit (resize, reorder) tidak bisa dipakai bila
  kelak diminta; harus ditulis di `GridColumnGeometry` dan header.

### Belum ada (kontrak, bukan kode, atau belum terukur)

- **Bench W5-T3 belum dijalankan.** Angka sumbu 4 sesudah penggantian (`scroll-30x1m`, `scroll-500x10k`, `open-500x10k`),
  keputusan prebuild (`GRID_PREBUILD` mati), dan penyetelan batas cache dari `peak_footprint_bytes` dan hit rate
  menunggu sesi bench eksklusif. ADR ini tidak mengklaim target tercapai.
- **P-1 (presisi compositor di atas 2^24 pt) tidak diukur**, butuh `screencapture -l` dengan izin Screen Recording
  (`cacheDisplay`, `bitmapImageRepForCachingDisplay`, dan `CALayer.render(in:)` menggambar di CPU dengan `Double` dan
  tidak bisa menunjukkan cacatnya). Ia adalah gerbang W6-T1; plafon 200.000 baris tetap sampai terukur, dengan
  mitigasi jendela baris (blueprint §5.4) disiapkan.
- **Satu pertanyaan terbuka:** AppKit tidak mendokumentasikan sistem koordinat `point` pada
  `view(_:stringForToolTip:point:userData:)` (diperiksa di header SDK dan JSON dokumentasi). `GridToolTip.local` memutuskannya
  dari rect jendela yang didaftarkan ke AppKit, dan pada **seri** (grid flipped, tanpa scroll, region memenuhi jendela)
  memakai pembacaan view. Satu hover manual di build dev menutupnya.
- **Tidak tercakup tes (§13):** `mouseDown(with:)` header (tanpa seam bebas `NSEvent`), setter fokus AX (D-11 dan P-24
  menuju W10-T1), pembangunan menu, signpost, rantai responder.
- **Backlog W5-C:** `announceSelectionForAX()` terpanggil dari `release()` meski `tab.cellSelection` nil dan mengumumkan
  "0 × 0 cells selected" ke VoiceOver (tambah `guard`); `commitEdit` jalur block-fill meninggalkan `tab.editSession`
  terbuka; `GridTableHandle` tidak dipanggil di mana pun selain `handle.coordinator = self` (pakai atau hapus).
- **Probe blueprint §15:** P-2 (nol kegagalan layout dan warna) dipenuhi oleh G-VIS 17/0 sesudah V-13; P-3, P-4, P-6
  tercermin di `GridParityTests` dan tes tooltip; P-5 (berapa kali `body` dievaluasi per drag) tidak punya hasil tercatat
  di ledger, jadi tidak diklaim di sini.

## Bukti

- Kode: `app/Sources/QueryHive/Views/GridTableView.swift`, `ResultGridTable.swift`, `GridRowView.swift`;
  `Models/ResultRows.swift`, `Models/GridMetrics.swift`, `Models/CellSelection.swift`.
- Tes: `GridMetricsTests`, `ResultRowsTests`, `GridAccessibilityTests`, `GridParityTests`, dan `VisualParityTests` (tidak diubah, §14.1).
- Gate yang tercatat di ledger untuk `13ea642`: `cargo fmt`, `cargo clippy -D warnings`, `cargo test`, `cargo deny check licenses`
  hijau; golden 11/22 seluruhnya terklasifikasi (D-8, D-1, D-2 diperluas, D-10 baru); `swift build` baik; swift test 595 /
  5 dilewati / 0 gagal; G-VIS 17/0; `./app/build.sh` baik. ADR ini tidak menjalankan ulang gate.
- Blueprint: `docs/architecture/blueprints/fase-5-grid.md` D-1 sampai D-15, §1.1, §9, §14.3, §15, §20 dan verdict architect-reviewer.
- Ledger (scratch, gitignored): `target/run/ledger.md` baris W5-T1 5a, 5b, 5b text probe, 5b review r1 dan r2.

## Referensi

- ADR-0003 (grid `NSTableView`; diamandemen ADR ini), ADR-0030 (data plane, W6-D), ADR-0033 (pola V-code V-12).
- `docs/architecture/prd-performance-and-parity.md` V-13 (§6.5), FR-GRID-01, FR-GRID-07, NFR-P4, NFR-V, P-24, O-10, O-12, O-20.
- `docs/architecture/performance-plan.md` §9.1, §9.2 dan §14 butir 9 (perlu diperbarui sesuai §17.3 blueprint).
- `docs/architecture/blueprints/fase-6-data-plane.md` §17.2 dan §17.5 (`StoreRows`, selisih `cell(row:column:format:)` untuk W6-A1).
- Tugas penerus: W5-T3 (bench), W5-C (backlog), W6-T1 (P-1 dan `StoreRows`), W10-T1 (kursor dan keyboard), W10-T2.
