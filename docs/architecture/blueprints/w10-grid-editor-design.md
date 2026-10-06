# Blueprint W10: desain grid dan editor (kunci kursor, peek, tata bahasa staged, tombol tinjau, record, diagnostik, rotor)

- **Status:** blueprint tingkat berkas, 6 Okt 2026 (W10-A1). Belum ada kode yang ditulis. Bagian "Verdict architect-reviewer" di ujung sudah diisi pemeriksa (6 Okt 2026): koreksi yang memblokir sudah diterapkan dan menunggu pemeriksaan ulang (O-20).
- **Untuk:** W10-T1 sampai W10-T7 (`development-plan.md` §5 W10). Pemeriksa mengikuti kolom Gate tiap tugas: SR, AX, UX, CR, DB, RR.
- **Sumber:** PRD FR-GRID-06…15, FR-ED-06…10, NFR-A1…A5, NFR-V (V-9, V-10, V-11), NFR-S5, P-16, P-17, P-18, P-24, P-28; `tablepro-design-audit.md` §4, §5, §6, §8, HIG extras; blueprint Fase 5 (ADR-0032: seam, kursor P-24, pohon AX), Fase 6 §17 (seam pasca-W6: `StoreRows`, `rowsOrThrow`, `rowsDidGrow`, `viewBusy`), blueprint 4B (§4.8 masalah, §7.4 pemilik atribut sementara, §8 titik kait), dan **blueprint W9** (`w9-shell-and-a11y.md`: `Announcer`, `FocusRegion`, token D-7 dan D-8, `AppMenu`, kunci W10 yang dicadangkan). TablePro (AGPL) hanya dibaca untuk ide lewat audit desain. Tidak ada kode, aset, atau string yang disalin.
- **Prasyarat:** W9 selesai (khususnya T0 pecahan `AppModel`, T1 token dan `Announcer`, T2 `AppMenu` dan model fokus, T7 ukuran font) dan W6-T1 6b dan 6c mendarat. Nomor baris di dokumen ini adalah di `8103478` dan `2d14eea`, dan berkas yang disentuh W6-T1 (`ResultGrid.swift`, `ResultGridTable.swift`, `QueryTab.swift`, `AppModel.swift`) dibaca per **nama simbol**.
- **Bukti:** kode di pohon ini. Angka kontras dihitung WCAG 2.x dari heksadesimal (tabel di blueprint W9 §1.8), bukan diukur di layar. API Rust dan Swift pihak ketiga yang tidak ada di pohon ini (misalnya `tokio_postgres::DbError::position`) ditulis dari ingatan dan ditandai sebagai tidak diverifikasi (§14.2). Dokumen ditulis tanpa build.
- **Bahasa:** dokumen ini Indonesia. Identifier, komentar kode, string yang tampil di app, dan nama tes Inggris. "Kunci kursor" di rencana dibaca sebagai **kunci keyboard untuk kursor** (peta tombol), bukan penguncian kursor; PRD P-24 menulisnya sebagai "pengikatan kuncinya di W10".

## Ringkasan

W10 membangun fitur grid dan editor di atas komponen W9 dan seam W5/W6. Tujuh tugas, tiga lane (grid T1→T2→T3→T4, record T5 sesudah T1, editor T6→T7).

Tujuh temuan yang menentukan rancangan (bukti di §1):

1. **Baris yang ditambah dan dihapus tidak punya gambar.** `CellEdits` sudah punya `insertRow`, `deleteRow`, dan `WritePlan` sudah menyusun DELETE, UPDATE, dan INSERT, tetapi grid hanya menggambar baris hasil. T3 menambah satu ruang baris (`GridRowSpace`) di atas hasil.
2. **Warna tanda staged gagal 3:1 di kanvas terang** (amber 1,6, mint 1,5, coral 2,7). T2 memakai token varian terang dari W9 (D-8) dan memberi tiap status bentuk selain warna.
3. **Cincin kursor tidak bisa memakai `accent.glow`** (1,5 sampai 2,9:1 di kanvas terang untuk empat dari lima aksen). T1 memakai `Tone.focusRing`.
4. **Posisi galat server selalu `None` hari ini**: `EngineError::Query.position` ada, tetapi ketiga driver menulis `position: None`, dan `From<EngineError> for CliError` membuang `code` dan `position`. T6 harus mengalirkannya dari driver sampai event `error`, di balik setelan (P-06).
5. **Rotor "Statements" hari ini hanya model data.** Tidak ada `NSAccessibilityCustomRotor` yang terpasang di `SQLTextView` (grep `CustomRotor` kosong). T6 memasang rotor sungguhan untuk "Statements" dan "Query issues".
6. **Salin sebagai INSERT tidak aman untuk MySQL bila memakai `UpdateStatements.literal`**: `quoted` hanya menggandakan `'`, sedangkan MySQL membaca `\` sebagai escape. T4 butuh literal yang sadar dialek.
7. **Nomor gutter grid dan editor 10,5 pt melanggar lantai 11 pt, dan menyentuhnya mengubah geometri.** W9-T9 sengaja menundanya ke sini (T2 dan T7).

## 1. Fakta yang diperiksa sebelum merancang

### 1.1 Grid

| Fakta | Bukti |
|---|---|
| `GridTableView.keyDown` menelan empat panah dan meneruskan sisanya ke `super`; ⌘A ditolak; ⌘C hanya valid bila ada seleksi. Komentarnya: "W10-T1 is what binds them to the cell cursor". | `Views/GridTableView.swift` bagian "Focus and the keyboard" |
| Kursor sudah dimodelkan tetapi tidak digambar dan tidak punya tombol: `GridCursor(anchor, focus)`, `QueryTab.cellCursor`, `selectCells(anchor:focus:)` sebagai satu-satunya penulis pointer. `cellSelection` tetap sumber kebenaran tulis, dengan invarian "kursor ada di dalam seleksi". | `Models/CellSelection.swift` (`GridCursor`), `Models/QueryTab.swift` (`cellSelection` didSet, `selectCells`) |
| Pohon AX sel sudah ada dan dibuat saat ditanya: label "Row r, column c, header: value, changed", `accessibilityFocusedUIElement` = sel di kursor, pengumuman "N × M cells selected" hanya di `release()`. **Tidak ada setter fokus AX** (catatan W5: "AX focus setter → W10-T1"). | `Views/GridAccessibility.swift` (`GridAXCell.accessibilityLabel`, `GridAXTree.focusedCell`); `ResultGridTable.swift` `announceSelectionForAX` |
| Palet grid: `inkDim` 0,35 (nomor baris), `inkNull` 0,30 (NULL dan `∅`), `inkFaint` 0,05 (pemisah), `rule` 0,12, `stripe` 0,03, `selection` aksen 0,20, `amber` tetap `FFB547`. Dibangun per appearance dari `SwiftUI colorScheme` karena `NSAppearance` tidak bisa dipercaya di jendela yang di-host. | `Views/GridRowView.swift` `GridPalette` (baris 40-65) |
| Hit-area funnel = batas glyph (±11×11) di pojok kanan atas kolom; klik di tempat lain pada header menjalankan sort. | `Views/GridHeaderView.swift` `funnelBounds(content:)` |
| Gutter nomor baris memakai `codeNSFont(size: 10.5)`, dan kotak gutter 25 pt = tinggi baris `lineHeight` 13 ditambah padding 6+6. `GridMetricsTests` mengunci 23, 25, dan 50. | `Models/GridMetrics.swift` baris 127; `Views/GridRowView.swift` baris 143 |
| Footer: dua `IconButton` tak bernama untuk Review (`checkmark.circle`) dan Discard (`arrow.uturn.backward`); ⌘Z hanya lewat menu konteks sel. `QueryTab.editUndoManager` adalah `UndoManager` milik tab, tidak tersambung ke jendela. | `ResultGrid.swift` `footer`; `QueryTab.swift` (`editUndoManager`, `registerGridEdit`, nama aksi tetap "Edit Cell") |
| `CellEdits` sudah menyimpan `inserted: [InsertedRow]` (id negatif, `values: [kolom: teks]`), `deletedRows: [Int]`, `insertRow()`, `deleteRow(_:)`, `setInserted`, `isDeleted`, `insertedValue`. **Tidak ada** penghapusan baris yang disisipkan atau pemulihan baris yang ditandai hapus. | `Models/CellEdits.swift` |
| `WritePlan.build` sudah menyusun **DELETE, lalu UPDATE, lalu INSERT** dari `CellEdits`, dan `InsertStatements` mengikuti aturan: kolom yang tidak diisi dihilangkan, teks `DEFAULT` menjadi kata kunci, baris yang seluruhnya default tetap menjadi pernyataan. Tidak ada gestur untuk mengisinya. | `Models/WritePlan.swift`, `Models/InsertStatements.swift` |
| `UpdateStatements.literal(_:type:)` adalah satu-satunya perender literal bentuk tinjau, dan `quoted` hanya menggandakan `'` ("the one escape every dialect here agrees on"). Bentuk yang dijalankan memakai bind untuk PostgreSQL dan MySQL. | `Models/UpdateStatements.swift` `literal`, `quoted` |
| Panel inspektur 340 pt memuat `CellValueViewer` untuk satu sel (atau catatan "N × M cells chosen" untuk blok), muncul hanya bila `DataPreferences.autoShowInspector` dan ada seleksi. | `ResultGrid.swift` `inspector(_:)`, `inspectorRange` |
| Setelah W6 (Fase 6 §17): `ResultRows.cell` dan `fullValue` hanya di main; `rowsOrThrow(in:columns:)` boleh di luar main dan menolak hasil parsial; `StoreRows.row(at:)` membaca blok `4096/jumlahKolom` baris; `viewBusy` menolak edit selama `apply` view; hasil bertambah lewat `rowsDidGrow(from:to:)`. | Blueprint Fase 6 §17.2 sampai §17.4 |

### 1.2 Editor dan engine

| Fakta | Bukti |
|---|---|
| Dialek editor selalu `.generic`: `EditorDocument(text:, dialect: .generic)` dan `sqlStatementRanges(sql:, dialect: .generic)`. Dialek koneksi tab belum diteruskan. | `Support/EditorAnalysis.swift:111`, `:389` |
| Data masalah sudah mengalir: `issues.rs` (leksikal `UnclosedQuote`, `UnclosedIdentifier`, `UnclosedComment`, `UnclosedDollar`, `UnbalancedParen`; sintaks `SyntaxError`, `MissingToken`), `EditorOutline.issues`, `EditorIssueData` di Swift. Garis bawah belum digambar. | `crates/qh-editor/src/issues.rs`, `crates/qh-ffi/src/editor.rs:110-130`, `EditorAnalysis.swift:75-96` |
| `StatementsRotorSource` dan protokol `EditorRotorSource` ada, dibangun ulang tiap outline. **Tidak ada `accessibilityCustomRotors`** di mana pun. | `Views/SQLEditor.swift:1494-1530`; grep `CustomRotor` di `app/Sources` kosong |
| Pemilik kunci atribut sementara: `.foregroundColor` warna sintaks, `.backgroundColor` find, `.underlineStyle` dan `.underlineColor` milik W10-T6, pasangan kurung digambar di `drawBackground`. | Blueprint 4B §7.4 |
| `EngineError::Query` punya `position: Option<u32>` (1-based, "character offset"), tetapi setiap konstruksi di PostgreSQL (2 tempat), Trino (±10), dan MySQL (4) menulis `None`; komentar PostgreSQL: "the UI can hold the caret on it once the field is plumbed through". `CliError::Query(String)` menyimpan hanya pesan. Event `error` hanya membawa `message` dan `warnings`. | `crates/qh-core/src/error.rs:58-71`; `qh-driver-postgres/src/lib.rs:1056-1071`; `qh-driver-trino/src/lib.rs:652-1139`; `qh-driver-mysql/src/lib.rs:349`, `:1513-1545`; `qh-ffi/src/lib.rs:141-290`; `qh-ffi/src/main.rs:47`, `uniffi_api.rs:363-390` |
| `preview` mengirim teks pemanggil **apa adanya** (cap diterapkan dengan membaca baris, bukan membungkus), jadi posisi server relatif terhadap teks yang dikirim. `count` memakai pembungkus. | `crates/qh-ffi/src/commands.rs:1515` (doc `preview`), `crates/qh-sql/src/wrap.rs` |
| Gutter: isian putih 3,5% di atas permukaan, dan komentar kodenya sendiri berkata "say the word and it becomes a recess instead of a lift". Seam: `Tone.inkNS(0.07)`. Font nomor 10,5 dan font editor 12,5 (bukan 13 seperti di audit). | `Views/SQLEditor.swift:2200-2230` (draw), `:89`, `:205` |
| Baseline editor: 16 PNG `editor-*`. Gutter dan hitungan baris sudut dijaga satu bacaan oleh `ReadoutColourTests`. | `Tests/QueryHiveTests/ReadoutColourTests.swift`; `VisualParityTests.swift:1039-1072` |

## 2. Keputusan desain

| ID | Keputusan | Alasan |
|---|---|---|
| D-1 | **Satu penulis, dua pembaca.** `selectCells(anchor:focus:)` tetap satu-satunya penulis `cellSelection` dan `cellCursor`. Gerak keyboard dihitung oleh fungsi murni `GridKeyMap` dan `GridCursorMath` (tanpa `NSEvent`) lalu ditulis lewat `selectCells`. `cellSelection` tidak dibalik menjadi turunan. | Semua tes dan penulis hari ini menulis `cellSelection`. Membaliknya menyentuh puluhan tes tanpa membeli apa pun. |
| D-2 | **Ruang baris = hasil + baris yang ditambah.** `GridRowSpace(fetched:, inserted:)` memetakan baris tabel ke `.fetched(row)` atau `.inserted(id)`. Tabel melaporkan `fetched + inserted.count` baris, `CellKey.row` untuk sel yang ditambah adalah **id negatif** `InsertedRow.id`, dan semua konversi tetap lewat `Coordinator.resultRow(forTableRow:)` dan `tableRow(forResultRow:)`. **Seleksi dan kursor (`cellSelection`, `cellCursor`) tinggal di ruang baris tabel** (indeks `0..<fetched + inserted.count`, kolom tetap posisi tampilan): id negatif tidak pernah masuk `CellPos`, `CellRange`, atau `GridCursor`, dan tiap mutator memilah blok seleksi menurut jenis baris lewat `GridRowSpace` (§5.1). | `CellEdits` sudah memakai id negatif untuk memisahkan identitas baris yang ditambah dari indeks baris hasil. Jendela baris W6 (bila P-1 gagal) hanya menambah `base` di fungsi yang sama. |
| D-3 | **Cincin kursor = `Tone.focusRing`, 2 pt, di dalam kotak sel**, bukan cincin sistem. Bila tabel bukan first responder atau jendela tidak kunci, cincin digambar 40%. | §1.8 W9: aksen glow gagal 3:1 di kanvas terang. Kursor harus tetap terlihat saat fokus pindah ke panel peek. |
| D-4 | **Tanda staged punya bentuk dan varian terang.** Diubah: titik 4 pt (ada). Ditambah: tanda `+` di gutter dan wash mint seluruh baris. Dihapus: tanda `−` di gutter, coretan pada teks, wash coral seluruh baris. Nada memakai `Tone.markAmber`, `markMint`, `markCoral`. Tanpa gutter (`showRowNumbers == false`), tanda `+`/`−` digambar 10 pt di dalam sel pertama dan menggeser teks 12 pt hanya pada baris itu. | FR-GRID-09 dan §1.8 W9. |
| D-5 | **Pemisah ≥ 3:1 hanya di Increase Contrast.** Pada kontras normal pemisah tetap hairline 0,05 dan aturan header 0,12; di `enhanced` pemisah 0,45 dan aturan 0,50. Nomor baris dan NULL ≥ 4,5:1 selalu (0,60), funnel ≥ 3:1 selalu (0,50). | Menaikkan semua pemisah ke 0,42 mengubah grid menjadi kisi tebal. FR-GRID-08 menyebut "separator ≥ 3:1" tanpa membedakan; ini tafsiran dan **ditandai untuk pemilik**. |
| D-6 | **⌘S dan ⌘Z tidak lewat menu SwiftUI.** ⌘S menjalankan `AppModel.saveFocused()` yang memilih menurut `currentRegion` (W9): hasil dengan edit staged membuka tinjauan, editor menyimpan berkas (W12-T2). ⌘Z dan ⇧⌘Z ditangani `GridTableView` sendiri (`undo(_:)`, `redo(_:)`, dan `undoManager` yang mengembalikan `tab.editUndoManager`), sehingga item Edit sistem memakai nama aksi ("Undo Edit Cell"). | P-16 ("lewat responder chain") dipenuhi tujuannya: simpan hal yang sedang fokus. Item menu SwiftUI tidak divalidasi responder, jadi `.disabled(!canSave)` dihitung dari keadaan model. |
| D-7 | **Peek = `NSPanel` nonaktivasi**, bukan `QLPreviewPanel` (P-17), memuat `CellValueViewer` yang ada dan **membaca nilai lewat `rowsOrThrow` di luar main**, dipotong 64 KiB. Tidak ada berkas sementara (NFR-S5). | `fullValue` hanya di main dan satu nilai 10 MB per langkah panah akan membuat frame tersendat. |
| D-8 | **Record = mode panel samping**, bukan jendela. Keadaan `QueryTab.recordMode`, tombol ⌥⌘I. Panel yang sama (340 pt) dengan sakelar dua segmen "Cell" dan "Record". | FR-GRID-13, audit §6. Tidak menambah permukaan baru selain panel. |
| D-9 | **Salin sebagai INSERT memakai literal sadar dialek.** `UpdateStatements.literal(_:type:)` mendapat parameter `kind:` (bawaan `nil` = perilaku hari ini, jadi tes yang ada dan teks tinjau tidak berubah). Untuk MySQL, `\` digandakan. Kolom biner **ditolak** dengan catatan, bukan diubah menjadi sembarang literal. Reviewer DB wajib. | §1.1: `quoted` hanya menggandakan `'`. |
| D-10 | **Seret keluar = `NSFilePromiseProvider`** (CSV), dimulai hanya bila `mouseDown` jatuh **di dalam** seleksi sekarang dan pointer bergerak > 4 pt. Selain itu seret tetap memperluas seleksi. Berkas ditulis saat dijatuhkan, dari antrean latar. | FR-GRID-15, NFR-S5. Aturan Finder: seret pada item terpilih = seret keluar. |
| D-11 | **Diagnostik di editor hanya leksikal dan posisi galat server.** Garis bawah lewat atribut sementara `.underlineStyle` dan `.underlineColor` (pemilik W10-T6). Galat sintaks tree-sitter tetap mati (P-28). Tanda galat server dilepas pada **setiap** edit teks (revisi dokumen berbeda), karena diagnostik basi berbohong. | FR-ED-06, blueprint 4B §4.8 dan §7.4. |
| D-12 | **Posisi galat server satu satuan:** `position` = offset **skalar Unicode** 1-based dalam teks yang dikirim ke server. PostgreSQL memberinya langsung, Trino mengubah `errorLocation` (baris, kolom), MySQL mengubah "at line N" menjadi awal baris N. Event `error` membawanya **hanya** bila app memasang setelan `ERROR_POSITION=1` (P-06), dan app memasangnya hanya untuk Run (`preview`), bukan Explain (§8.2). CLI, MCP, dan golden tidak berubah. | NFR-C. Satu satuan menjaga Swift sederhana (satu konversi skalar ke UTF-16). |
| D-13 | **Rotor sungguhan untuk dua sumber.** `SQLTextView.accessibilityCustomRotors` mengembalikan "Statements" dan "Query issues" lewat `NSAccessibilityCustomRotor` dengan delegat yang membaca `EditorRotorSource` terbaru. | §1.2: hari ini hanya model. |
| D-14 | **Pasangan kurung dan kutip digambar di `drawBackground`**, dari `bracket_pair` di Rust yang dijalankan di antrean analisis dengan pemeriksaan revisi. Dua pita `Tone.accent 0,22` dengan garis tepi 1 pt `accent 0,8` (bentuk, bukan warna saja). Tidak ada atribut sementara. | Blueprint 4B §7.4 dan §8.5; NFR-P5 (main p99 ≤ 4 ms). |
| D-15 | **Gutter terang = recess**: hitam 4,5% pada kanvas terang, putih 3,5% tetap di gelap. Dibuktikan dengan selisih luminans gutter dan area teks ≥ 4/255 di ketujuh kanvas, dihitung dari tangkapan scene editor. Seam memakai `Tone.hairline` (W9). | FR-ED-10, komentar kode sendiri. |
| D-16 | **⌘+, ⌘−, ⌘0 mengikuti wilayah fokus** (`currentRegion`, W9): editor mengubah `EditorPreferences.fontSize` (langkah 1, rentang 10 sampai 28, reset 12,5), hasil mengubah `DataPreferences.gridFontSize` (11 sampai 16, reset 12), selain itu editor. | Satu kunci, satu arti per wilayah, dan dua setelan yang sudah ada dari W9-T7. |
| D-17 | **Nomor gutter grid dan editor menjadi 11 pt**, bersama hitungan baris sudut editor. Kotak gutter grid dikunci 25 pt (konstanta) dan teks dipusatkan, bukan diturunkan dari padding. | D-18 W9: menaikkan satu dari pasangan "bacaan yang sama" memisahkan keduanya. Mengunci kotak 25 menjaga geometri pitch dan pemisah. |
| D-18 | **Rekam ulang baseline per kode V**, satu commit per scene: V-9 (T1: scene dengan kursor; T2: semua scene grid; T3: scene dengan edit staged), V-10 (T6 dan T7: scene editor). Scene baru (V-11) direkam sebagai berkas baru. | §0.5 `development-plan.md`. Tiga tugas menyentuh `grid-edits`, jadi sampai tiga commit untuk scene itu. |

## 3. W10-T1: kursor sel, peta kunci, panel peek, AX per sel

Ukuran L. Berkas (daftar plan): `Views/ResultGridTable.swift`, `Views/GridAccessibility.swift`, `Views/GridKeyboard.swift` (baru), `Views/CellPeekPanel.swift` (baru); ditambah `Views/GridTableView.swift` (`keyDown`, fokus), `Views/GridRowView.swift` (cincin; T2 mengambilnya sesudahnya), `Models/CellSelection.swift` (`GridCursorMath`), `Models/AppMenu.swift` dan `Models/Shortcuts.swift` (`peekCell` menjadi tersedia), dan tes. Review SR, AX, UX, CR. Commit: `feat(grid): a cell cursor for the keyboard, a Space preview, and VoiceOver that reads each cell`. V-9 (scene dengan kursor).

### 3.1 Peta kunci grid

`Views/GridKeyboard.swift` mendefinisikan `GridKey` (kode tombol dan pengubah, tanpa `NSEvent`) dan `GridKeyMap.action(for:in:) -> GridKeyAction?`. `GridTableView.keyDown` mengubah `NSEvent` menjadi `GridKey`, menanyakan peta, dan menjalankan aksi lewat `Coordinator`. Yang tidak dikenali diteruskan ke `super` (Page Up/Down, Home, End kini milik peta ini).

| Kunci | Aksi | Catatan |
|---|---|---|
| ← → ↑ ↓ | pindahkan kursor satu sel, seleksi runtuh ke kursor | dijepit di tepi; tidak membungkus |
| ⇧ + panah | perluas: jangkar tetap, fokus bergerak | |
| ⌘ + panah | lompat ke tepi (kolom pertama atau terakhir, baris pertama atau terakhir) | |
| ⌘⇧ + panah | perluas sampai tepi | |
| Page Up / Page Down | pindah sejauh `baris terlihat − 1`; dengan ⇧ memperluas | |
| Home / End | kolom pertama atau terakhir pada baris kursor; dengan ⌘ sel kiri-atas atau kanan-bawah | |
| Return | `beginEdit` pada kursor, atau pembaca bila sel *openable* dan panel tidak berdiri (aturan dobel-klik §8.3 Fase 5) | sedang mengedit: ditangani overlay |
| Esc | berurutan: batalkan edit → tutup peek → runtuhkan seleksi ke kursor → hapus seleksi | satu langkah per ketukan |
| Tab / ⇧Tab | kursor ke sel berikutnya atau sebelumnya, membungkus ke baris sebelah; di sel terakhir Tab meninggalkan grid | |
| Space | buka atau tutup peek untuk kursor | juga ⌘Y (menu View) |
| ⌘C / ⌘⇧C | salin / salin dengan header | sudah ada |
| ⌫ / ⌦ | tandai baris seleksi untuk dihapus (T3) | hanya bila tabel dapat diedit |
| huruf, angka | tidak melakukan apa-apa | mengetik-untuk-mengedit di luar lingkup |

`GridCursorMath` (di `Models/CellSelection.swift`) adalah fungsi murni `(cursor, bounds, key) -> GridCursor`, dengan `bounds` = (jumlah baris dalam ruang baris D-2, jumlah kolom terlihat, tinggi halaman). Hasilnya ditulis lewat `tab.selectCells(anchor:focus:)` (D-1).

**Gulir.** Setelah tiap gerak, `Coordinator.scrollToVisible(cell:)` memanggil `table.scrollToVisible` pada rect sel (gulir minimal, satu panggilan per ketukan), memakai `GridColumnGeometry.edges(of:)` untuk sumbu horizontal. Tidak ada animasi.

**Gerak saat hasil bertambah (W6).** Indeks kursor adalah indeks hasil, stabil terhadap `rowsDidGrow(from:to:)`. Saat `apply` view memasang view baru, W6 mengosongkan seleksi dan kursor ikut hilang (`cellSelection = nil`).

### 3.2 Cincin kursor

`GridRowPainter` menggambar cincin setelah wash dan teks dan titik staged: stroke 2 pt `Tone.focusRing` pada `box.insetBy(dx: 1, dy: 1)` sudut 3 pt (di `enhanced`: 2,5 pt). Tabel bukan first responder atau jendela bukan kunci: alfa 0,40. `GridInputs` mendapat `cursor: CellPos?` dan `focused: Bool` (`Equatable`), dan `GridPaintDiff` mendapat `cursorInvalidations(old:new:)` yang hanya meng-invalidate rect sel lama dan baru (ditambah `overflow` baris Compact, aturan Fase 5 §5.3). `becomeFirstResponder` dan `resignFirstResponder` memanggil `coordinator.focusChanged()` yang meng-invalidate sel kursor saja. Cincin tidak mengubah geometri, jadi lapis tata letak dan warna G-VIS pada sel tanpa kursor tidak berubah.

Scene yang berubah (V-9): `grid-selection`, `grid-inspector`, `grid-edits`, dan `grid-counted` (bila punya seleksi). Mereka direkam ulang, satu per satu, di commit sendiri. Scene lain tidak boleh bergerak.

### 3.3 Panel peek (P-17)

`Views/CellPeekPanel.swift`: `final class CellPeekPanel: NSPanel` dengan `styleMask [.nonactivatingPanel, .utilityWindow, .borderless]`, `becomesKeyOnlyIfNeeded = true`, `hidesOnDeactivate = true`, level `.floating`. Isinya `NSHostingView(CellValueViewer(value:column:type:connectionID:table:placement: .popover))`: pembaca yang ada, tanpa mengubah `CellValueViewer.swift` (milik T5).

- **Posisi.** Di bawah sel kursor (konversi rect sel ke layar lewat `convert(_:to: nil)` dan `convertToScreen`), dibalik ke atas bila tak muat, dan dipindah saat kursor bergerak atau tabel menggulir. Sel yang tergulir keluar dari jendela menutup panel.
- **Isi.** Nilai dibaca dengan `rowsOrThrow(in: r..<r+1, columns: [c])` di antrean latar dan dipotong `CellPeekPanel.maxCharacters = 65_536`, dengan catatan "Showing the first 64 KiB of N. Copy takes the whole value." Selama baca lebih dari 50 ms panel menampilkan "Reading…". Panah saat peek terbuka memindah kursor dan **memperbarui** panel (perilaku Quick Look di Finder), tanpa mengambil fokus.
- **Penutupan.** Space atau ⌘Y lagi, Esc (langkah kedua di peta), klik di grid, jendela nonaktif, atau kursor hilang.
- **Tanpa berkas.** Tidak ada `QLPreviewPanel` dan tidak ada berkas sementara (NFR-S5).
- **Pengumuman.** Saat terbuka: `Announcer.post("Peek: <kolom>, <nilai 160 karakter>")`.
- **AX.** Panel `accessibilityLabel("Cell peek")`, isinya teks statis, dan tidak merebut fokus VoiceOver.

### 3.4 AX per sel (FR-GRID-07)

- **Setter fokus.** `GridAXCell.setAccessibilityFocused(_:)` memanggil `coordinator.focusCellFromAX(key)` yang menulis `cellCursor` **tanpa** mengubah seleksi (D-11 Fase 5), menggulir sel ke terlihat, dan tidak memposting pengumuman. Gerak kursor oleh keyboard memposting `.focusedUIElementChanged` pada `GridAXCell` baru (hanya bila klien AX terpasang, `axClientAttached`), supaya VoiceOver mengikuti.
- **Label.** "Row r, column c, <header>: <nilai>" ditambah status: ", changed" (diubah), ", new row" (disisipkan), ", marked for deletion" (dihapus). NULL dibaca "null" **apa pun** `nullDisplay`-nya, termasuk bila pengguna mengosongkannya (label hari ini membaca teks tampilan, yang kosong bila disetel kosong).
- **Pengumuman seleksi.** "N × M cells selected" tetap di `release()`, dan ditambah satu pengumuman 250 ms **sesudah ketukan perluasan terakhir** (bukan per ketukan).
- **Header** menyebut sort ("sorted ascending") seperti sekarang, dan aksi kustom "Filter".

### 3.5 Tes

- `GridKeyMapTests` (baru, murni): setiap baris tabel §3.1, termasuk jepit di tepi, perluasan, tepi, halaman, Tab membungkus, dan urutan Esc.
- `GridCursorMathTests`: sama, atas `bounds` yang mencakup baris yang ditambah.
- `GridParityTests` (diperluas): `keyDown` panah memindah kursor dan seleksi lewat `coordinator`, cincin menginvalidasi dua rect, `focusChanged` menginvalidasi sel kursor, panah tidak mengubah seleksi baris `NSTableView` (D-15 Fase 5 tetap).
- `GridAccessibilityTests` (diperluas): setter fokus mengatur kursor tanpa mengubah seleksi, label dengan status, NULL dibaca "null" untuk `nullDisplay == ""`, pengumuman tertunda setelah ketukan perluasan.
- `CellPeekPanelTests`: pemotongan 64 KiB dengan catatan, pembaruan saat kursor bergerak, penutupan oleh Esc, tanpa berkas di direktori sementara (membandingkan isi `NSTemporaryDirectory()` sebelum dan sesudah).
- Bench: G-BENCHQ (`scroll-30x1m`, `scroll-500x10k`) tidak boleh mundur lebih dari 5% (NFR-P9); cincin hanya satu sel.

### 3.6 Probe

| ID | Pertanyaan | Lulus bila | Bila gagal |
|---|---|---|---|
| P-1a | Apakah panel nonaktivasi tetap memberi `GridTableView` semua ketukan panah saat terbuka? | `keyDown` sampai ke tabel | `NSPanel` bukan `.utilityWindow`; jatuh ke popover |
| P-1b | Apakah `setAccessibilityFocused` dipanggil VoiceOver pada elemen yang dibuat saat ditanya? | uji manual pemilik; uji otomatis memanggil metode langsung | tambahkan `accessibilityFocusedUIElement` setter pada tabel |

## 4. W10-T2: lantai kontras, Increase Contrast, tata bahasa staged, funnel

Ukuran M. Berkas (daftar plan): `Views/GridRowView.swift`, `Views/GridHeaderView.swift`, token grid di `Support/Theme.swift`, `Models/CellEdits.swift` (status tampilan); ditambah `Models/GridMetrics.swift` (kotak gutter), `Views/ResultGridTable.swift` (`GridInputs` memuat kontras), dan tes. Review AX, UX, SR. Commit: `fix(grid): row numbers, NULL and the funnel meet the contrast floor, and staged rows say what they are`. V-9: seluruh scene grid direkam ulang (palet dan font gutter berubah di semuanya).

### 4.1 Palet dan Increase Contrast

`GridPalette` membaca `ThemeStore.shared.surface.enhanced` (W9 D-7) lewat `GridStyle.contrast` (nilai `Equatable` yang mengganti palet dan epoch cache `CTLine` saat berubah; sumber yang sama dengan `isDark` agar jendela yang di-host tidak menyimpang).

| Elemen | Hari ini | Normal | `enhanced` | Dasar (W9 §1.8) |
|---|---|---|---|---|
| nomor baris (`inkDim`) | 0,35 | **0,60** | 0,80 | 0,60 lolos 4,5:1 di ketujuh kanvas (5,60 sampai 7,32) |
| NULL dan `∅` (`inkNull`) | 0,30 | **0,60** | 0,80 | sama |
| funnel idle | 0,30 | **0,50** | 0,70 | ≥ 3:1 (minimum 0,42) |
| teks sel (`inkStrong`) | 0,90 | 0,90 | 1,00 | |
| pemisah kolom (`inkFaint`) | 0,05 | 0,05 | **0,45** | D-5: ≥ 3:1 hanya di `enhanced` |
| aturan header (`rule`) | 0,12 | 0,12 | **0,50** | |
| stripe | 0,03 | 0,03 | 0,06 | |

Ini satu-satunya ketidakparitasan yang disengaja (audit §5): nomor baris dan NULL menjadi jelas lebih terang. UX memeriksa hierarki di dua appearance dari scene baru `grid-contrast-dark/-light` (V-11, tanpa baseline lama) sebelum rekam ulang.

### 4.2 Tata bahasa staged (FR-GRID-09, D-4)

| Status | Wash | Bentuk (bukan warna) | Teks | Label AX |
|---|---|---|---|---|
| diubah (sel) | `markAmber` 0,20 pada kotak sel | titik 4×4 `markAmber` di pojok kanan atas, jarak 3 | nilai staged | ", changed" |
| disisipkan (baris) | `markMint` 0,16 selebar baris | tanda `+` 11 pt bold `markMint` di gutter | nilai terisi `ink 0,9`; sel kosong menampilkan `DEFAULT` miring `ink 0,60` | ", new row" |
| dihapus (baris) | `markCoral` 0,14 selebar baris | tanda `−` 11 pt bold `markCoral` di gutter dan coretan 1 pt `ink 0,9` melintasi teks tiap sel | `ink 0,75` | ", marked for deletion" |

- Tanpa gutter (`showRowNumbers == false`) tanda `+` dan `−` digambar 10 pt di dalam sel pertama dan menggeser teks 12 pt pada baris itu saja.
- Aturan yang ada dipertahankan: wash staged menang atas wash seleksi; cincin kursor tetap terlihat di atasnya (§3.2). Sel staged yang terseleksi tetapi bukan kursor hanya tampak amber; ini batas yang diterima (paritas).
- `CellEdits` mendapat `enum CellState { unchanged, modified, inserted, deleted }`, `state(of: CellKey)`, dan `rowState(_ row: Int)` (baca saja; mutasi baru di T3). `Coordinator.rowText` dan `GridRowPainter.paint` menerima status baris, dan baris yang disisipkan dibangun dari `edits` seperti baris staged hari ini (`rowText` sudah menghindari cache untuk baris dengan edit).

### 4.3 Gutter dan funnel

- Font nomor baris `codeNSFont(size: 11)`. **Kotak gutter dikunci 25 pt** sebagai konstanta (`GridMetrics.gutterBoxHeight`) dan teks dipusatkan di dalamnya, menggantikan `lineHeight + 2 × padding`. Pada tinggi baris 21 (Compact) luapan 2 pt tetap persis seperti sekarang, jadi pitch, pemisah, dan urutan cat tidak bergeser. `GridMetricsTests.testTheBoxesAreTheSizeTheBaselinesMeasured` tetap 23/25/50.
- **Probe P-2a:** gambar `grid-kinds`, `-compact`, dan `-tall` dengan font 11 dan konfirmasi pemisah gutter berakhir di tempat yang sama seperti baseline lama (selisih hanya piksel teks).
- Funnel: area klik = **seluruh tinggi header × 20 pt di tepi kanan kolom**, dengan prioritas atas sort dan atas chip tipe yang melebar di bawahnya (`funnelBounds(content:)` mengembalikan strip itu). Glyph tetap 10 pt di pojok kanan atas. Tooltip dan aksi AX "Filter" tidak berubah.

### 4.4 Tes

- `GridPaletteContrastTests`: tujuh kanvas × normal dan `enhanced`: nomor baris dan NULL ≥ 4,5, funnel ≥ 3, pemisah dan aturan ≥ 3 di `enhanced`, wash staged dengan teks `ink 0,9` ≥ 4,5 (nilai di W9 §1.8: terendah 6,81 di Nord).
- `GridParityTests` (diperluas): empat status menggambar penanda di tempat yang benar (marker `sat` untuk titik, `ink` untuk coretan), tanda gutter dan tanda dalam-sel, `testTheFunnelHitAreaIsTheWholeHeaderHeightAt20pt`, `testRepaintingARowTwiceGivesTheSamePixels` untuk baris staged.
- `CellEditsStateTests`: `state(of:)` dan `rowState` untuk setiap kombinasi.
- Scene baru: `grid-contrast-*` dan `grid-staged-*` (baris disisipkan dan dihapus; V-11).

## 5. W10-T3: tambah dan hapus baris, tombol tinjau ⌘S, undo

Ukuran M. Berkas (daftar plan): `Views/ResultGrid.swift` (footer; `pasteIntoSelection`), `Views/ResultGridTable.swift` (menu; `press`, `drag`, `validKey`, `resultRow(forTableRow:)`), `Models/{CellEdits,WritePlan}.swift`, `Models/AppModel+Edit.swift`, `App.swift` (Save); ditambah `Models/CellSelection.swift` (`GridClipboard.text` memilah blok menurut jenis baris, §5.1), `Views/GridAccessibility.swift` (indeks AX atas `rowSpace`; berkas ini sudah ada di rantai grid §7), `Models/QueryTab.swift` (`rowSpace`, `reviewRequested`, `registerGridEdit(_:actionName:)`, `reconcileSelectionWithRowSpace`, `fillCellEdits`, `pasteCellEdits`, dan cabang kunci baris negatif di `cellValue`, `fetchedValue`, `beginCellEdit`, dan `endCellEdit`; `recordMode` menyusul di T5), `Views/GridTableView.swift` (`undo:`, `redo:`, `undoManager`), `Models/AppMenu.swift`, dan tes. Review SR, DB, UX, AX. Commit: `feat(grid): add and delete rows, review them with ⌘S, and undo with ⌘Z`. V-9 (scene dengan edit staged).

### 5.1 Ruang baris (D-2)

`GridRowSpace(fetched: Int, inserted: [CellEdits.InsertedRow])` adalah nilai murni: `count`, `kind(ofTableRow:) -> .fetched(Int) | .inserted(id: Int, index: Int)`, `tableRow(forInsertedID:)`, dan tiga fungsi yang menutup celah antara ruang tabel dan ruang kunci edit (di bawah). `QueryTab.rowSpace` membangunnya dari jumlah baris `result` dan `cellEdits.inserted`.

**Ruang seleksi.** `cellSelection`, `cellCursor`, `CellPos`, `CellRange`, dan `GridCursor` berada di **ruang baris tabel**: indeks baris `NSTableView`, `0..<rowSpace.count`, dengan kolom tetap berupa posisi tampilan. Baris tabel `r < fetched` adalah baris hasil `r`; `r >= fetched` adalah baris yang ditambah ke-`r - fetched` menurut urutan `cellEdits.inserted`. Baris yang ditambah selalu **mengikuti** semua baris hasil, jadi blok yang melintasi batas selalu berupa awalan baris hasil diikuti akhiran baris yang ditambah. **Id negatif tidak pernah masuk `CellPos`, `CellRange`, atau `GridCursor`:** `CellRange` memakai `min`/`max`, `top...bottom`, dan `rowCount = bottom - top + 1` (`Models/CellSelection.swift`), yang hanya bermakna di ruang yang kontigu, dan semua mutator hari ini mengindeks `result` dengan nomor baris itu. Id negatif hidup hanya di `CellKey.row`, dan satu-satunya jalan dari posisi tabel ke `CellKey` adalah:

```swift
extension GridRowSpace {
    /// Id negatif hanya untuk `row >= fetched`; nil di luar `0..<count`.
    func cellKey(forTableRow row: Int, source: Int) -> CellKey?
    func tableRow(for key: CellKey) -> Int?
    /// Awalan baris hasil dan akhiran baris yang ditambah dari satu blok baris tabel.
    func split(_ rows: ClosedRange<Int>)
        -> (fetched: ClosedRange<Int>?, inserted: [(tableRow: Int, id: Int)])
}
```

`Coordinator.resultRow(forTableRow:)` dan `tableRow(forResultRow:)` (identitas hari ini, `ResultGridTable.swift` ≈ :243-244) mendelegasikan ke `rowSpace`; yang pertama menjadi `Int?`, `nil` untuk baris yang ditambah.

- `numberOfRows(in:)` melaporkan `rows.count + inserted.count`. `Coordinator.press` dan `drag` menjepit ke `rowSpace.count` (hari ini `rows.count`, `ResultGridTable.swift` ≈ :523-547), dan `validKey(row:source:)` (hari ini menolak `row >= rows.count`, ≈ :596) memakai `cellKey(forTableRow:source:)`.
- Sel yang ditambah memakai `CellKey(row: id, column: source)` dengan `id < 0`. `QueryTab.cellValue(at:)`, `beginCellEdit`, dan `endCellEdit` mencabangkan pada tanda `id` ke `insertedValue` dan `setInserted`. Mengetik teks kosong menghapus nilai (kolom dihilangkan dari INSERT, aturan 1 `InsertStatements`); mengetik `DEFAULT` menjadi kata kunci.
- `QueryTab.fetchedValue(at:)` memanggil `result.fullValue(row:)` langsung (≈ :776-778). Untuk `key.row < 0` ia mengembalikan `nil` tanpa menyentuh store: sel yang ditambah tidak punya nilai hasil, dan `original` untuk edit-nya `nil`.
- Menambah baris **dinonaktifkan** selama `previewing` atau `viewBusy` (W6 D-27), supaya jumlah baris hasil tidak bergeser di bawah baris yang ditambah, dan bila `tab.sourceTable == nil` ("Open a table to add rows") atau Safe Mode `read_only` ("This connection is read-only"). `no_ddl` tetap boleh (DML).
- Baris yang ditambah tidak ikut sort, filter, atau search in-memory, dan bila ada edit staged, sort atau search server ditolak seperti hari ini (FR-GRID-03).

**Apa yang dilakukan tiap mutator dan pembaca dengan baris yang ditambah.** Tiap pemakai memilah blok lewat `rowSpace.split`; tidak ada yang mengulang `top...bottom` atas `result`.

| Pemakai hari ini | Baris hasil (awalan blok) | Baris yang ditambah (akhiran blok) |
|---|---|---|
| `QueryTab.fillCellEdits(_:over:)` → `CellEdits.fill`, yang mengulang `range.top...range.bottom` dan membaca `original` dari `rows` (`CellEdits.swift` ≈ :82-90) | `CellEdits.fill` atas sub-rentang hasil saja, seperti sekarang | `setInserted(text, row: id, column: source)` per sel (teks kosong menghapus nilai, `DEFAULT` menjadi kata kunci); tidak pernah `stage` |
| `QueryTab.pasteCellEdits` → `CellEdits.paste`, yang memberi kunci `origin.row + down` (≈ :99-107) | asal paste adalah baris tabel; tiap baris tempelan lewat `cellKey(forTableRow: origin + down, …)`; baris hasil → `stage` | `setInserted`; baris tempelan di luar `rowSpace.count` dibuang (paste tidak menambah baris, sama dengan aturan "tidak membungkus" yang ada) |
| Hapus: ⌫, ⌦, ⌘⌫, menu konteks (`AppModel.deleteRows(in:)`, §5.2) | `edits.deleteRow(_:)` dengan indeks hasil | `edits.removeInserted(row: id)`; tidak pernah `deleteRow` dengan id negatif atau indeks `>= fetched` |
| Restore (`restoreRows`) | `edits.restoreRow(_:)` | tidak melakukan apa-apa |
| Salin (`GridClipboard.text(result:selection:visible:withHeaders:)`, yang membaca `rowsOrThrow(in: top..<bottom + 1)`, `CellSelection.swift` ≈ :139-154), "Copy as INSERT" dan seret keluar CSV (T4) | `rowsOrThrow` atas sub-rentang hasil, seperti sekarang | nilai staged dari `insertedValue` (kosong → bidang kosong; di Copy as INSERT kolom dihilangkan); hasil digabung dalam urutan baris tabel |
| Pembaca, inspektur, peek (`selectionValue`, `inspectorRange`, `CellPeekPanel`) | `rowsOrThrow` | `cellValue(at:)` lewat `cellKey`; tidak membaca store |
| Pohon AX sel (`rowCountForAX` dan `visibleRowRange`, ≈ `ResultGridTable.swift:902-915`; `GridAXTree.cell(row:column:)`) | indeks hasil | `rowSpace.count` dan `cellKey` dengan id negatif; label dengan status (", new row") |

**Kestabilan.** Indeks baris tabel sebuah baris yang ditambah bergeser bila baris yang ditambah lebih awal dihapus, dan jumlah baris berubah pada tambah, hapus, undo, redo, dan discard. Setiap perubahan `cellEdits` yang mengubah `inserted.count` memanggil `QueryTab.reconcileSelectionWithRowSpace()`, yang menjepit `cellSelection` dan `cellCursor` ke `0..<rowSpace.count` lewat `selectCells` (D-1), atau menghapusnya bila `count == 0`. `registerGridEdit` memanggilnya juga di penutup undo dan redo-nya, dan `discardCellEdits()` (yang hari ini membiarkan seleksi) ikut.

**Invarian dan pertahanan.** Kontraknya: `CellEdits.values` hanya memuat kunci `0 <= row < fetched`, `deletedRows` hanya indeks `< fetched`, dan id negatif hanya ada di `inserted`. `CellEdits.fill` melewati baris yang `rows.row(at:) == nil`, seperti `paste` (≈ :106); hari ini `fill` menyimpan edit dengan `original: nil` untuk baris di luar hasil. `WritePlan.build` tetap menolak kunci yang tak terbaca lewat peringatan "could not be read" (`WritePlan.swift` ≈ :141-145, :162-164), tetapi itu jaring terakhir untuk SQL yang menulis ke database, bukan kontrak.

### 5.2 Gestur

| Gestur | Aksi |
|---|---|
| tombol **Add Row** (footer, `plus`), menu konteks "Add Row", ⌥⌘N | `edits.insertRow()`, kursor ke sel pertama baris baru, gulir ke bawah, `Announcer`: "Row added. N pending changes." |
| tombol **Delete Rows** (`minus`), menu konteks "Delete Row" atau "Delete N Rows", ⌘⌫, ⌫ saat grid fokus | baris hasil di seleksi → `edits.deleteRow`; baris yang ditambah → `edits.removeInserted(row:)` (baru) |
| menu konteks "Restore Row" (pada baris yang ditandai hapus) | `edits.restoreRow(_:)` (baru) |

`CellEdits` mendapat `removeInserted(row id: Int)` dan `restoreRow(_ row: Int)`. Semua mutasi ada di `AppModel+Edit.swift` (`addRow(in:)`, `deleteRows(in:)`, `restoreRows(in:)`), yang mencatat undo lewat `QueryTab.registerGridEdit(_:)` (snapshot `before` dan `after` pada `tab.editUndoManager`). Fungsi itu hari ini `private` dengan nama aksi tetap "Edit Cell"; T3 memegang `QueryTab.swift` sehingga menjadikannya internal dengan parameter `actionName` (bawaan "Edit Cell") untuk "Add Row", "Delete Row", "Delete Rows", dan "Restore Row".

### 5.3 Footer dan tombol tinjau (FR-GRID-11)

Kedua `IconButton` tanpa nama diganti:

```
… · 3 changes   [Review 3 Changes…]  [Discard]      [+][−]      LIMIT …  [Export]
   ^ Tone.markAmber (teks ≥ 4,5:1, W9 §1.8)
```

`PillButton(title: "Review \(pluralized(n, "Change"))…", symbol: "checkmark.circle")` dengan bantuan "Review and run the changes (<kunci skema>)", `PillButton(title: "Discard", symbol: "arrow.uturn.backward", role: .quiet)`, dan dua `IconButton` bernama ("Add row", "Delete selected rows") yang tampil bila tabel dapat diedit. **Tidak ada `.keyboardShortcut` pada tombol tinjau**: ⌘S milik menu (§5.4), dan dua pengikat untuk satu kunci akan memicu keduanya. Teks ringkasan footer yang hari ini berwarna `Tone.amber` (teks 1,6:1 di kanvas terang) berpindah ke `Tone.markAmber`.

### 5.4 ⌘S dan ⌘Z (D-6)

- `App.swift`: `CommandGroup(replacing: .saveItem) { Button("Save") { model.saveFocused() }.keyboardShortcut(model.shortcut(for: .saveFile)).disabled(!model.canSaveFocused) }`, dirender dari `AppMenu`. `saveFocused()` memeriksa `currentRegion`: `.results` dengan `cellEdits` tak kosong dan bukan `viewBusy` menaikkan `QueryTab.reviewRequested` yang diamati `ResultGrid` (`.onChange` mengatur `@State reviewingChanges`); `.editor` meneruskan ke W12-T2 (sampai saat itu tidak melakukan apa-apa dan tombol dinonaktifkan).
- `GridTableView`: `@objc func undo(_:)` dan `redo(_:)` memanggil `tab.undoCellEdit()` dan `redoCellEdit()`, `validateUserInterfaceItem` memakai `canUndoCellEdit` dan `canRedoCellEdit`, dan `override var undoManager` mengembalikan `coordinator.tab.editUndoManager`. Menu konteks yang ada (Undo Edit, Redo Edit) tetap.
- **Probe P-3a:** `NSApp.target(forAction: Selector(("undo:")), to: nil, from: nil)` mengembalikan `GridTableView` saat tabel first responder, dan item Edit > Undo menampilkan "Undo Edit Cell". Bila tidak, item Edit SwiftUI diganti `CommandGroup(replacing: .undoRedo)` yang memanggil `model.undoFocused()`.

### 5.5 Tes

- `CellEditsRowTests` (baru): `removeInserted`, `restoreRow`, `deleteRow` menghapus edit baris, urutan `insertRow` dan id negatif; `CellEdits.fill` melewati baris yang tak terbaca. **Blok yang melintasi batas** (hasil 5 baris, 2 baris ditambah, blok baris tabel 3 sampai 6): `fill` menghasilkan edit UPDATE hanya untuk baris hasil 3 dan 4 dan `setInserted` untuk kedua baris yang ditambah; `paste` empat baris dari baris tabel 4 menaruh satu baris di hasil dan satu di tiap baris yang ditambah, dan baris keempat (baris tabel 7, di luar `rowSpace.count`) dibuang tanpa menambah baris; hapus (⌫) atas blok itu menandai hanya baris hasil 3 dan 4 dan membuang kedua baris yang ditambah, dengan satu langkah undo yang memulihkan semuanya; salin memberi nilai hasil untuk baris hasil dan nilai staged untuk baris yang ditambah, dalam urutan baris tabel; `WritePlan.build` atas hasilnya memuat tepat DELETE, UPDATE, dan INSERT yang diharapkan tanpa peringatan "could not be read"; setelah tiap mutator `cellEdits.values.keys.allSatisfy { 0 <= $0.row && $0.row < fetched }` dan `deletedRows` hanya berisi indeks `< fetched`. Satu kasus di tingkat `QueryTab`: `fetchedValue(at:)` untuk `row < 0` memberi `nil` tanpa menyentuh store (store uji yang gagal bila dibaca).
- `GridRowSpaceTests` (baru): pemetaan, jumlah baris, indeks AX; `split` atas blok yang melintasi batas (awalan hasil dan akhiran yang ditambah) dan atas blok yang seluruhnya di satu sisi; `cellKey(forTableRow:source:)` memberi id negatif **jika dan hanya jika** `row >= fetched`, dan `tableRow(for:)` membalikkannya; tidak ada nilai negatif di `CellRange` mana pun yang dibangun dari baris tabel; menghapus baris yang ditambah lebih awal menggeser baris tabel berikutnya dan `reconcileSelectionWithRowSpace` menjepit seleksi dan kursor (nil bila `count == 0`), juga setelah undo, redo, dan discard.
- `WritePlanTests` (diperluas): rencana dari satu baris yang ditambah, satu dihapus, dan satu diubah menghasilkan DELETE, UPDATE, INSERT berurutan dengan `expectedRows`; baris yang ditambah lalu dihapus tidak menghasilkan pernyataan.
- `CellEditUndoTests` (diperluas): satu langkah undo per tambah dan per hapus, nama aksi, dan `redo`.
- `ResultGridTests` (diperluas): footer memuat tombol bernama dan tidak memuat `.keyboardShortcut` pada tinjau; Add dinonaktifkan di `read_only`, tanpa `sourceTable`, dan selama `viewBusy`.
- **G-LIVE PostgreSQL** (`QH_TEST_POSTGRES=1`): `apply_changes` dengan satu INSERT, satu DELETE, dan satu UPDATE dalam satu transaksi; hitungan yang meleset menggulung balik (sudah ada untuk UPDATE dan DELETE, baru untuk INSERT dan urutan).
- Reviewer DB memeriksa jalur ini (SQL yang dibangun app) bersama T4.

## 6. W10-T4: salin sebagai INSERT, seret keluar sebagai CSV

Ukuran M. Berkas (daftar plan): `Models/CellSelection.swift` (`GridClipboard`), `Models/InsertStatements.swift`, `Views/ResultGridTable.swift` (sumber seret); ditambah `Models/UpdateStatements.swift` (literal sadar dialek), `Views/ResultGrid.swift` (menu dan lembar nama tabel), dan tes. Review DB, SR. Commit: `feat(grid): copy a selection as INSERT statements, or drag it out as a CSV file`.

### 6.1 Salin sebagai INSERT (FR-GRID-12)

```swift
// Models/InsertStatements.swift
extension InsertStatements {
    struct Copy: Equatable { var text: String; var warnings: [String] }
    static func copyScript(rows: [[String?]], columns: [Event.Column], selected: [Int],
                           table: String, kind: ConnectionKind) -> Copy
}
// Models/UpdateStatements.swift
static func literal(_ text: String, type: String, kind: ConnectionKind? = nil) -> String   // nil = hari ini
```

- **Satu pernyataan per baris**, `INSERT INTO <tabel> (<kolom>) VALUES (<literal>);`, dipisah baris baru, kolom dalam urutan tampilan dengan nama server dan dikutip lewat `quotedIdent(_:for:)`. NULL menjadi `NULL`. Nilai angka dan boolean mengikuti `literal`.
- **Literal sadar dialek (D-9).** Teks tetap dikutip `'…'` dengan `'` digandakan. Untuk MySQL, `\` digandakan pula, karena MySQL membaca `\` sebagai escape pada mode bawaan. Parameter `kind: nil` mempertahankan perilaku hari ini, jadi `UpdateLiteralTests` dan teks tinjau `WritePlan` tidak berubah.
- **Kolom biner ditolak.** Bila seleksi memuat kolom yang `GridValue.isBinary(type:)`, hasilnya kosong dengan catatan "Binary columns can't be copied as INSERT. Export them instead." Tidak ada tebakan literal heksadesimal.
- **Nama tabel.** `tab.sourceTable` (konteks "Open table"). Bila `nil`, lembar kecil `TableNameSheet` menanyakannya; nama tidak disimpan di tab.
- **Batas.** Paling banyak 100.000 baris dan 5.000.000 sel per salinan. Di atas 1.000 baris, teks dibangun di luar main lewat `rowsOrThrow` (menolak hasil parsial), dengan penjaga `changeCount` pasteboard yang sama dengan salinan biasa (Fase 5 §8.7).
- **Letak.** Menu konteks "Copy as INSERT" dan item Edit "Copy as INSERT Statements" tanpa kunci. Baris yang ditambah ikut dengan nilai staged-nya (§5.1).
- **Risiko.** MySQL dengan `NO_BACKSLASH_ESCAPES` membaca `\\` sebagai dua karakter. Teks salinan dirancang untuk mode bawaan dan itu dicatat di catatan hasil ("Written for MySQL's default string mode."). Reviewer DB memutuskan apakah catatan cukup.

### 6.2 Seret keluar sebagai CSV (FR-GRID-15, D-10)

- **Mulai seret.** `mouseDown` di dalam seleksi (lebih dari satu sel) menunda keruntuhan seleksi sampai `mouseUp`. `mouseDragged` melewati 4 pt memulai `beginDraggingSession` dengan `NSFilePromiseProvider(fileType: UTType.commaSeparatedText.identifier, delegate:)`. Press di luar seleksi tetap memilih dan memperluas seperti sekarang.
- **Penulis berkas.** `filePromiseProvider(_:fileNameForType:)` mengembalikan "`<tabel atau 'result'>` `<baris>`×`<kolom>`.csv" yang dibersihkan dari `/` dan `:`. `filePromiseProvider(_:writePromiseTo:completionHandler:)` berjalan di antrean latar (`operationQueue(for:)`), membaca potongan lewat `rowsOrThrow`, dan menulis secara atomik ke URL. Galat (disk penuh, hasil basi) dikembalikan lewat `completionHandler`.
- **Aturan CSV.** UTF-8 tanpa BOM, pemisah koma, baris pertama nama kolom server untuk kolom terpilih (urutan tampilan), bidang yang memuat koma, kutip, CR, atau LF dikutip dan kutipnya digandakan, NULL menjadi bidang kosong, akhir baris `\n`. Aturan ini ditulis di satu fungsi murni `GridCSV.encode(rows:columns:)` yang diuji tanpa seret.
- **Gambar seret.** Kapsul "N rows × M columns".
- **Tanpa berkas sebelum dijatuhkan** (NFR-S5): tidak ada berkas sementara dibuat saat seret dimulai.
- **Jalan non-pointer.** Seret hanya untuk pointer, jadi item menu konteks "Save Selection as CSV…" (`NSSavePanel`, `GridCSV.encode` yang sama) disertakan sebagai padanan keyboard dan VoiceOver. Item itu tambahan terhadap FR-GRID-15 dan dinyatakan di commit.

### 6.3 Tes

- `InsertStatementsTests` (diperluas): satu pernyataan per baris, urutan kolom tampilan, NULL, angka tidak dikutip menurut tipe, kutip tunggal digandakan, `\` digandakan hanya untuk MySQL, kolom biner menolak, baris yang ditambah memakai nilai staged, batas baris.
- `UpdateLiteralTests`: `kind: nil` tidak mengubah satu pun keluaran yang ada.
- `GridCSVTests`: bidang dengan koma, kutip, CR/LF, NULL, nama kolom dengan koma, putaran penuh lewat pembaca CSV uji.
- `GridParityTests` (diperluas): tekan di dalam seleksi tidak meruntuhkannya sebelum `mouseUp`, seret melewati 4 pt memulai sesi (`coordinator.dragSource` bisa dipanggil tanpa `NSEvent`), tidak ada berkas sementara bertambah.
- Probe P-4a: `NSFilePromiseProvider` yang dimulai dari subclass `NSTableView` menulis berkas saat dijatuhkan di Finder dan di Numbers (uji manual pemilik); uji otomatis memanggil delegat langsung.

## 7. W10-T5: mode Record

Ukuran M. Berkas (daftar plan): `Views/RecordPanel.swift` (baru), `Views/CellValueViewer.swift` (sakelar mode); ditambah `Views/ResultGrid.swift` (panel dan sakelar), `Models/QueryTab.swift` (`recordMode`), `Models/AppMenu.swift` (`toggleRecord`), dan tes. Review SR, UX, AX. Commit: `feat(grid): a record view that reads a whole row as fields`. V-11 (permukaan baru, tanpa baseline lama).

### 7.1 Perilaku (FR-GRID-13, D-8)

- **Sakelar.** Kepala panel samping (340 pt) mendapat `Segmented` dua opsi "Cell" dan "Record" (tombol bernama; trait `.isSelected` ditambahkan di `Segmented` oleh W9-T6). `QueryTab.recordMode: Bool` menyimpannya per tab. ⌥⌘I (menu View) dan tombol di footer mengubahnya. Dalam mode Record panel tampil walau `DataPreferences.autoShowInspector` mati; keluar dari Record mengembalikan perilaku sebelumnya.
- **Baris.** Baris kursor (`tab.cellCursor.focus.row` dalam ruang baris §5.1). Tanpa kursor: "Select a cell to see its row." Baris yang ditambah menampilkan nilai staged dengan tandanya.
- **Isi.** `LazyVStack` berisi satu bidang per kolom terlihat: nama kolom (11,5 mono semibold), chip tipe (`Chip(text:kind: "Type")`), dan nilai (12 mono) maksimal 6 baris dengan "Show more" yang memperluas bidang itu. NULL tampil dengan teks NULL yang diatur pengguna, redup 0,60. Tanda staged per bidang memakai bentuk §4.2 (titik, `+`, coretan). Satu baris dibaca dengan `rows.rowsOrThrow(in: r..<r+1, columns: <semua sumber terlihat>)`, nilai tampilan memakai `ColumnFormat` kolom itu, dan nilai penuh hanya dimuat saat "Show more".
- **Cari bidang.** Kolom "Find field" menyaring nama kolom (huruf besar-kecil tidak dibedakan). Sakelar "Edited only" menyaring bidang yang punya edit staged.
- **Interaksi.** Klik bidang menaruh kursor di sel itu (`selectCells`) dan menggulir ke sana. Panel tidak mengedit; pengeditan tetap di grid (audit §6).
- **Lebar.** 500 kolom: daftar malas, dan pencarian bekerja atas nama kolom yang sudah ada di memori tanpa membaca nilai.
- **Pembaca satu nilai tetap**: mode "Cell" memakai `CellValueViewer` tanpa perubahan perilaku; "Show more" membuka pembaca yang sama untuk bidang itu bila nilainya terstruktur.

### 7.2 AX dan keyboard

Daftar bidang adalah kontainer dengan label "Record", tiap bidang satu elemen gabungan "<kolom>, <tipe>: <nilai>, <status>" dengan aksi "Show in grid". Urutan fokus: sakelar, kolom cari, daftar. ⌥⌘3 mengembalikan fokus ke grid. Pergantian baris kursor diumumkan lewat `Announcer` ("Record: row 12") dengan debounce 250 ms.

### 7.3 Tes

- `RecordPanelTests` (baru): bidang per kolom terlihat dalam urutan tampilan (kolom disembunyikan dan dipindah), pencarian, "Edited only", tanda staged, baris yang ditambah, pemotongan enam baris, 500 kolom membangun daftar malas, "Show in grid" menaruh kursor.
- `ResultGridTests` (diperluas): sakelar menyimpan `recordMode` per tab, panel tampil di Record tanpa `autoShowInspector`.
- Scene baru `grid-record-*` (V-11).

## 8. W10-T6: diagnostik editor, rotor, dan readout

Ukuran M, dua commit (6a Rust, 6b Swift; pemisahan opsional dari ledger O-22). Berkas (daftar plan): `crates/qh-core/src/error.rs`, pemetaan galat di tiga driver, `crates/qh-ffi/src/events.rs`, `Support/EditorAnalysis.swift`, `Views/SQLEditor.swift`, `Support/EditorDiagnostics.swift` (baru); ditambah `crates/qh-ffi/src/{lib.rs,main.rs,uniffi_api.rs}` (varian galat dan pemancar `fail`), `App.swift` (`Event`), `Models/QueryTab.swift`, `Models/AppModel+Run.swift`, `Views/Workspace.swift` (dialek dan readout), dan tes. Review RR, SR, AX, UX, CR. Commit: `feat(editor): server errors and unclosed quotes are underlined, listed in a rotor, and read out`. **Tidak mengubah permukaan UniFFI** (event `error` adalah NDJSON), jadi `app/Generated/` tidak diregenerasi; tetapi `uniffi_api.rs` ada di lane FFI, jadi T6 menempati urutan lane tepat sebelum T7 (§13).

### 8.1 Posisi galat server di engine (6a)

Satu satuan (D-12): `position` = offset **skalar Unicode** 1-based ke dalam teks yang dikirim ke server.

| Driver | Sumber | Pemetaan |
|---|---|---|
| PostgreSQL | `DbError::position()` (`ErrorPosition::Original(n)`), diketahui sebagai offset karakter 1-based (tidak diverifikasi, §14.2) | `Some(n)`; `Internal` (galat di dalam fungsi) diabaikan |
| Trino | objek `error` JSON punya `errorLocation { lineNumber, columnNumber }`. `WireError` hari ini tidak memuatnya | tambah `error_location` di `WireError` (`qh-driver-trino/src/lib.rs:214-222` hari ini hanya `message`, `error_code`, `error_name`, `error_type`); `map_error` (`:847`) menerima teks statement dan memanggil `offset_of_line_column` |
| MySQL | pesan server berakhir "… at line N" | ambil N dengan pola `at line (\d+)$`, lalu `offset_of_line` (awal baris N; kasar dan dinyatakan begitu) |

Dua fungsi murni di `qh-core` (diuji sendiri): `offset_of_line_column(sql, line, column) -> Option<u32>` dan `offset_of_line(sql, line) -> Option<u32>`, keduanya dalam skalar Unicode. Semua konstruksi `EngineError::Query { position: None }` yang berasal dari kegagalan statement (PostgreSQL 1, Trino 1, MySQL 1) diisi; konstruksi lain (batal, koneksi) tetap `None`. Doc `position` di `error.rs` diperbarui dengan satuannya.

Pengalir: `From<EngineError> for CliError` hari ini memetakan `Query` ke `CliError::Query(message)` dan **membuang `code` dan `position`** (`qh-ffi/src/lib.rs:259`). Varian baru `CliError::QueryAt { message, position }` dihasilkan bila `position` ada; `Display` dan `message()` identik dengan `Query`, dan lima pemakai `CliError::Query` (dua di `From`, tiga di `commands.rs:2333-2344`) tidak berubah. `fail(out, error, settings)` di `uniffi_api.rs:376` dan jalur `main.rs:47` menambahkan `"position": N` ke event `error` **hanya** bila `settings.flag("ERROR_POSITION", false)`. App memasang `ERROR_POSITION=1` hanya untuk `preview` (Run); `explain` dikeluarkan karena ketiga driver membungkus teks pemanggil dengan `EXPLAIN `, sehingga posisinya relatif terhadap teks terbungkus (§8.2). Nama setelan dicadangkan di sini dan dilaporkan ke W11-A1 (pemilik nama setelan baru). Golden, CLI, dan MCP tidak memasangnya, jadi keluarannya tidak berubah (NFR-C).

### 8.2 Posisi di editor (6b)

`position` dari server adalah offset ke dalam teks yang **dikirim**, dan teks itu tidak selalu dokumen. Dari kode: `runSelectedTab` memanggil `preview(tab)` dengan `source: .selection` sebagai bawaan (`AppModel.runSelectedTab` dan `preview(_:from:)`, ≈ `AppModel.swift:1953-1956` dan `:1964`), dan bila seleksi kosong `QueryTab.sql(for: .selection)` jatuh ke **seluruh dokumen** (`guard selection.length > 0 … else { return sql }`, ≈ `QueryTab.swift:966-969`). `.statement` mengembalikan teks yang sudah dipangkas spasi dan baris baru (`sqlStatements`, ≈ `QueryTab.swift:1180`), sedangkan rentangnya mulai dari `;` sebelumnya. Awal teks yang dikirim karena itu tidak bisa diturunkan dari `selection.location` atau dari awal rentang statement; ia diukur **sesudah** fallback seleksi kosong dan **sesudah** pemangkasan. Engine mengirim teks itu apa adanya (`source_sql` mengembalikan `settings.raw("SQL")`, `commands.rs:498-502`).

- **Teks yang dikirim dan awalnya.** `QueryTab.sent(for: QuerySource) -> SentSQL` memuat logika `sql(for:)`, dan `sql(for:)` menjadi `sent(for:).text` (satu definisi):

  ```swift
  struct SentSQL: Equatable {
      let text: String
      let documentStart: Int   // offset UTF-16 di `QueryTab.sql` dari unit pertama `text`
  }
  ```

  | Sumber | `text` | `documentStart` |
  |---|---|---|
  | `.all` | `sql` | 0 |
  | `.selection`, panjang > 0 dan di dalam dokumen | potongan seleksi | `selection.location` |
  | `.selection`, kosong atau di luar dokumen (fallback) | `sql` | **0**, bukan lokasi caret |
  | `.statement`, ada statement | teks statement yang sudah dipangkas | awal rentang statement (UTF-16) + panjang UTF-16 spasi dan baris baru di depan yang terpangkas |
  | `.statement`, tak ada statement | `sql` | 0 |

  Untuk `.statement`, `sqlStatement(in:atUTF16Offset:)` menjadi pembungkus tipis atas saudaranya `sqlStatementLocated(in:atUTF16Offset:) -> (text: String, start: Int)?`, yang mengukur `start` dari `range.lowerBound` ditambah skalar spasi di depan teks rentang; hanya ada satu cara memotong statement. Jalur `catch` di `sqlStatements` (FFI menolak teks) mengembalikan seluruh skrip sebagai satu potong dan diukur dengan cara yang sama.
- **Yang memasang `ERROR_POSITION`.** `AppModel.preview(_:from:)`, sesudah `previewEnvironment` dan sebelum cabang konfirmasi; **bukan** di `previewEnvironment`, yang dipakai juga `explain`, `rerunBaseSQL`, dan sort atau search di server. `explain` dikeluarkan: PostgreSQL, MySQL, dan Trino semuanya membungkus teks pemanggil dengan `EXPLAIN ` (`qh-driver-postgres/src/lib.rs:668`, `qh-driver-mysql/src/lib.rs:806`, `qh-driver-trino/src/lib.rs:1397`), jadi posisinya bergeser 8 unit dan galat di dalam awalan itu sendiri tidak punya tempat di dokumen. Mengurangi awalan di driver berarti tiga tempat yang harus setuju soal awalan dan terminator yang dibuang `strip_terminator_dialect`; satu kontrak (`position` relatif terhadap teks yang diserahkan pemanggil, D-12) lebih murah, dan pesan galat Explain tetap tampil seperti hari ini. Garis bawah untuk Explain, bila dibutuhkan, adalah pekerjaan terpisah.
- `Event` (di `App.swift`) mendapat `var position: Int?`.
- `QueryTab.errorMark: ServerErrorMark?` dengan `ServerErrorMark { sqlSnapshot: String, sent: SentSQL, scalarOffset: Int }`. `runPreview` menerima `sent: SentSQL?` (bawaan `nil`); hanya `preview(_:from:)` mengisinya, dan `runPreview` membuat tanda pada kegagalan bila event membawa `position` dan `sent != nil`. Sort atau search di server (teks yang dibungkus app) dan `rerunBaseSQL` (teks Run lama, dokumen mungkin sudah berubah) tidak mengisinya, jadi tidak ada tanda untuk teks yang bukan potongan dokumen sekarang. Run yang memakai substitusi `:name` juga tidak membuat tanda (posisi bergeser bila teks diganti).
- `QueryTab.sql` `didSet` membuang `errorMark` bila teks tidak lagi sama dengan `sqlSnapshot` (D-11).
- Konversi: `ServerErrorMark.range(in text: NSString) -> NSRange?` menelusuri `scalarOffset - 1` skalar dari awal `sent.text` untuk mendapat offset UTF-16, menambah `sent.documentStart`, lalu memperluas sampai akhir kata (huruf, angka, garis bawah), minimal 1 dan maksimal 64 unit, dijepit ke akhir baris dan akhir dokumen; `nil` bila di luar dokumen.

### 8.3 Diagnostik dan garis bawah

`Support/EditorDiagnostics.swift` (baru): `EditorDiagnostic { kind (.lexical(EditorIssueKind) | .server), range, message }`, fungsi murni `EditorDiagnostics.merge(issues:server:text:)` (hanya jenis leksikal; `SyntaxError` dan `MissingToken` dibuang, P-28), dan `DiagnosticLabels`:

| Sumber | Label rotor | Garis bawah |
|---|---|---|
| `UnclosedQuote` | "Unclosed quote, line N" | putus-putus, `markAmber` |
| `UnclosedIdentifier` | "Unclosed quoted identifier, line N" | putus-putus, `markAmber` |
| `UnclosedComment` | "Unclosed comment, line N" | putus-putus, `markAmber` |
| `UnclosedDollar` | "Unclosed dollar quote, line N" | putus-putus, `markAmber` |
| `UnbalancedParen` | "Unbalanced parenthesis, line N" | putus-putus, `markAmber` |
| server | "Server error, line N: <80 karakter pesan>" | tebal utuh, `markCoral` |

`DiagnosticsPainter` menerapkan `.underlineStyle` dan `.underlineColor` lewat `addTemporaryAttribute` dan `removeTemporaryAttribute` pada jendela terlihat (kunci milik T6, blueprint 4B §7.4; tidak menyentuh `.foregroundColor` atau `.backgroundColor`), melewati rentang yang sudah benar seperti `apply` warna (§7.3 4B), dan berjalan pada giliran outline yang sudah di-debounce, bukan per ketikan. Warna dari token D-8, jadi lolos 3:1 di ketiga kanvas terang. Garis bawah **tidak** terbaca VoiceOver (atribut sementara tidak ikut `AXAttributedString`, 4B §7.5), jadi rotor dan readout adalah jalur AX-nya.

### 8.4 Rotor sungguhan (D-13, FR-ED-09)

`SQLTextView.accessibilityCustomRotors()` mengembalikan dua `NSAccessibilityCustomRotor`: "Statements" dan "Query issues". Delegat `EditorRotorDelegate: NSAccessibilityCustomRotorItemSearchDelegate` membaca `EditorRotorSource.items` terbaru dari koordinator, mencari item berikutnya atau sebelumnya relatif terhadap `searchParameters.currentItem` atau posisi caret, dan mengembalikan `NSAccessibilityCustomRotor.ItemResult(targetElement: textView)` dengan `targetRange` dan `customLabel`. `StatementsRotorSource` yang ada dipakai apa adanya, dan `IssuesRotorSource` baru berisi item dari `DiagnosticLabels`. `EditorRotorTests` yang ada tetap.

### 8.5 Readout

Di `EditorPane` (`Workspace.swift`, overlay sudut kanan bawah yang memuat hitungan baris): bila ada diagnostik, tambah "· N issues" dengan glyph `exclamationmark.triangle` (nada `markAmber`, atau `markCoral` bila ada galat server), `ink 0,9`, bukan abu-abu readout, karena ia peringatan. Label AX "N query issues". Hilang bila nol. Galat server sudah diumumkan oleh banner W9-T5; masalah leksikal **tidak** diumumkan saat mengetik (terlalu berisik).

### 8.6 Dialek tab

`SQLEditor` mendapat parameter `dialect: EditorDialect`, dihitung `EditorPane` dari `model.connection(for: tab)?.kind` (`.trino` → `.trino`, `.postgres` → `.postgres`, `.mysql` → `.mySql`, tanpa koneksi → `.generic`). `EditorDocument` dibuat dengan dialek itu (`EditorAnalysis.swift:111`), dan `updateNSView` yang melihat dialek berubah membuat ulang analisis (dialek tetap sejak konstruksi, catatan non-blocking W3-T2) lalu menjalankan warna dan outline penuh. `sqlStatements(in:)` dan `sqlStatementRanges` (`EditorAnalysis.swift:389`) mendapat `dialect:` di semua pemanggil (`AppModel+Run.statements(in:)`, `QueryTab.sql(for:)`, `SQLEditor`), supaya batas statement di editor sama dengan yang dibaca engine (4B D-20).

### 8.7 Tes

- Rust (`qh-core`): `offset_of_line_column` dan `offset_of_line` untuk baris pertama, tengah, terakhir, CRLF, emoji dan CJK (skalar, bukan byte atau UTF-16), baris di luar rentang → `None`.
- Rust (driver): pemetaan murni tiap driver (fungsi tanpa jaringan); satu tes bahwa event `error` **tidak** memuat `position` tanpa setelan dan memuatnya dengan setelan (`crates/qh-ffi/tests`). **G-LIVE**: PostgreSQL `SELEC 1` memberi posisi 1, Trino `SELECT * FRM t` memberi lokasi, MySQL `SELEC 1` memberi "at line 1". G-GOLDEN tidak berubah.
- Swift: `EditorDiagnosticsTests` (penggabungan, label, baris), `ServerErrorMarkTests` (skalar ke UTF-16 dengan emoji dan CJK, substitusi `:name` tanpa tanda, edit teks membuang tanda, perluasan kata; **seleksi kosong** dengan caret di tengah dokumen: `sent(for: .selection).documentStart == 0` dan posisi N menggarisbawahi skalar ke-N dokumen, bukan caret + N; seleksi tak kosong: `documentStart == selection.location`; **statement berspasi di depan** (`"select 1;\n\n   selec 2"`, caret di statement kedua): `sent.text == "selec 2"` dan `documentStart == 14`, juga bila statement sebelumnya memuat emoji (dua unit UTF-16, satu skalar); `sent(for:).text == sql(for:)` untuk semua sumber; Explain, sort atau search di server, dan `rerunBaseSQL` tidak memasang `ERROR_POSITION` dan tidak membuat tanda), `EditorRotorTests` (diperluas: dua rotor, pencarian maju dan mundur, label, target), `DiagnosticsPainterTests` (atribut sementara ada dengan gaya dan warna yang benar, hilang setelah perbaikan, kunci lain tidak tersentuh), `EditorDialectTests` (peta jenis ke dialek, perubahan membuat ulang analisis), dan `EditorReadoutTests`.
- Bench: G-BENCHQ `type-10k` (NFR-P5).
- Scene baru `editor-issues-*` (V-11, tanpa baseline lama); scene `editor-*` yang ada tidak berubah di T6.

## 9. W10-T7: ukuran font editor, pasangan kurung, gutter recess

Ukuran M. Berkas (daftar plan): `crates/qh-editor/src/brackets.rs` (baru), `crates/qh-ffi/src/editor.rs`, `app/Generated/`, `Support/EditorAnalysis.swift`, `Views/SQLEditor.swift`, `Support/EditorPreferences.swift`; ditambah `crates/qh-editor/src/lib.rs` (satu baris `mod`), `Views/Workspace.swift` (hitungan sudut 11 pt), `Models/AppMenu.swift` dan `Models/Shortcuts.swift` (tiga aksi font), `Models/AppModel+Focus.swift` (`adjustFontSize`, §9.2), dan tes. Lane FFI. Review RR, SR, UX, AX. Commit: `feat(editor): a font size you can change, matching brackets, and a gutter that recesses on light canvases`. V-10: seluruh 16 baseline editor direkam ulang.

### 9.1 Pasangan kurung dan kutip (FR-ED-08, D-14)

```rust
// crates/qh-editor/src/brackets.rs
/// Delimiter pair at the caret, as [start, len, start, len] in UTF-16 units; empty when none.
pub fn bracket_pair(statement_text: &str, caret_in_statement_utf16: u32, lexer: Lexer) -> Vec<u32>
// crates/qh-ffi/src/editor.rs   (diekspor di EditorDocument, lane FFI)
pub fn bracket_pair(&self, revision: u64, offset_utf16: u32) -> Result<Vec<u32>, EditorError>   // Stale bila revisi berbeda
```

Dihitung dari aliran `walk` dan `qh_sql::lex` atas statement di bawah caret (bukan pohon tree-sitter, yang diizinkan 4B §8.5 tetapi tidak dibutuhkan): kurung dan siku di region kode dihitung berkedalaman, dan region opak (kutip `'`, `"`, backtick, `$tag$`, komentar) tidak dihitung, sehingga kurung di dalam string atau komentar tidak pernah berpasangan. Aturan kedekatan: karakter tepat **sebelum** caret diperiksa lebih dulu, lalu yang **sesudah**. Pasangan kutip: bila caret menempel pada pembuka atau penutup region tertutup, kedua pembatas ditandai. Tanpa pasangan (tidak tertutup): kosong (masalah leksikal sudah ditandai T6). Statement di atas 1 MiB mengembalikan kosong.

Swift: koordinator meminta pasangan di antrean analisis 30 ms sesudah caret berhenti bergerak (tidak pada tiap perpindahan), menerapkan hasil bila `revision` sama, menyimpannya di `SQLTextView.bracketRanges`, dan meng-invalidate baris terkait. `drawBackground(in:)` menggambar dua pita (sudut 2, `Tone.accent` 0,22) dengan garis tepi 1 pt `accent` 0,8 (1,5 di `enhanced`), memakai `boundingRect(forGlyphRange:in:)` per rentang. Bentuk (garis tepi) membuatnya tidak bergantung pada warna. Bukan atribut sementara, supaya tidak berebut `.backgroundColor` dengan find. Seleksi non-kosong membersihkannya.

### 9.2 Ukuran font (FR-ED-07, D-16)

- Aksi `fontBigger` (⌘+, alias ⌘=), `fontSmaller` (⌘−), `fontReset` (⌘0) menjadi tersedia di `AppMenu` (menu View). Alias ⌘= ditangani oleh kasus tambahan pada router kunci W9.
- `AppModel.adjustFontSize(_:)` (di `Models/AppModel+Focus.swift`, bersama `currentRegion` yang dibacanya) memilih menurut `currentRegion`: `.results` mengubah `DataPreferences.gridFontSize` (langkah 1, 11 sampai 16, reset 12); selain itu `EditorPreferences.fontSize` (langkah 1, 10 sampai 28, reset 12,5). Setelan ini dan kontrolnya sudah ada dari W9-T7.
- Penerapan di editor: tulis ulang atribut dasar seluruh dokumen dan `typingAttributes`, lalu `mark_dirty(0, len)` dan warna ulang (4B §7.3 dan §7.6); posisi gulir dijaga dengan menjangkarkan indeks karakter paling atas yang terlihat. Satu kali per perubahan, tidak per ketukan.
- Nomor gutter dan hitungan baris sudut menjadi **11 pt** bersama (D-17); `LineNumberRulerView.textOriginX` dihitung ulang, dan `LineNumberRulerTests` serta 16 baseline editor mengikuti (V-10).

### 9.3 Gutter recess (FR-ED-10, D-15)

`LineNumberRulerView.draw` memakai warna dinamis: gelap putih 3,5% (tetap), terang **hitam 4,5%**, dan seam `Tone.hairline` (W9). Bukti numerik: `GutterRecessTests` membaca piksel scene editor dan menegaskan selisih luminans gutter terhadap area teks ≥ 4/255 di ketiga kanvas terang dan ≥ 4 di empat kanvas gelap (nilai gelap hari ini 7,4 menurut komentar kode).

### 9.4 Tes

- Rust: `BracketPairTests` (kurung bersarang, siku, kurung di dalam string dan komentar diabaikan, kutip, `$tag$`, backtick MySQL, aturan sebelum-lalu-sesudah, tak berpasangan, statement > 1 MiB), tes FFI untuk `Stale` dan batas.
- Swift: `EditorBracketsTests` (hasil basi diabaikan, pita digambar di tempat yang benar, seleksi membersihkan), `EditorFontSizeTests` (jepit, pemilihan wilayah, jangkar gulir, penerapan ke dokumen 2M tanpa melewati anggaran sekali jalan), `GutterRecessTests`, `LineNumberRulerTests` (diperbarui).
- G-FFI (`./app/build-ffi.sh`, `app/Generated/` di-commit bersama Rust), G-BENCHQ `type-10k` dan `type-2m` (NFR-P5: permintaan pasangan tidak menambah waktu main thread).

## 10. Spesifikasi UX (sudut pandang ui-ux-designer)

Ukuran dalam poin. Teks 11 pt ke atas. Warna tanda memakai token W9 D-8 (varian dalam di appearance terang). Kode V menandai perubahan terhadap baseline.

| Permukaan | Bentuk | Keadaan dan gerak | Salinan |
|---|---|---|---|
| **Cincin kursor** (V-9) | Stroke 2 pada kotak sel, inset 1, sudut 3, `Tone.focusRing` (2,5 di Increase Contrast). | Aktif: penuh. Tabel tidak fokus atau jendela tidak kunci: 40%. Tanpa animasi. | |
| **Peek** (V-11) | Panel mengambang selebar pembaca (maks 592×460, min 320×160), 4 di bawah sel, dibalik ke atas bila tak muat. Tanpa judul. | Muncul dengan pudar 0,12 dtk (tanpa gerak bila Reduce Motion), mengikuti kursor, menutup oleh Space, ⌘Y, Esc, klik, dan jendela nonaktif. | "Reading…", "Showing the first 64 KiB of N. Copy takes the whole value." |
| **Baris staged** (V-9) | Tiga status pada §4.2: titik 4 (diubah), `+` 11 bold dan wash mint (disisipkan), `−` 11 bold, coretan, dan wash coral (dihapus). | Pemisah dan aturan tetap hairline; di Increase Contrast 0,45 dan 0,50. | `DEFAULT` miring untuk sel kosong pada baris yang ditambah. |
| **Footer** (V-9) | Ringkasan, "· N changes" (`markAmber`), `PillButton` "Review N Changes…" dan "Discard", dua `IconButton` 22 (Add row, Delete selected rows) bila dapat diedit. | Add dan Delete dinonaktifkan dengan bantuan alasannya (read-only, tanpa tabel, hasil sedang memuat). | "Review 1 Change…", "Review 3 Changes…". Bantuan: "Review and run the changes (<kunci>)". |
| **Gutter grid** (V-9) | Nomor baris 11 pt `ink 0,60` di kotak 25. Tanda `+` dan `−` menggantikan nomor pada baris yang bersangkutan. | | |
| **Funnel** (V-9) | Glyph 10 di pojok kanan atas, area klik seluruh tinggi header × 20 di tepi kanan. Idle `ink 0,50`. | Aktif aksen. | Tooltip tidak berubah. |
| **Panel Record** (V-11) | Panel 340 yang sama. Kepala 36: dua segmen "Cell" dan "Record". Kolom "Find field" 28. Daftar bidang berjarak 10; tiap bidang: baris label (nama 11,5 mono semibold secondary, chip tipe di kanan) di atas nilai (12 mono, `ink 0,9`, maks 6 baris). | Bidang staged membawa tandanya. Tanpa animasi. | "Select a cell to see its row.", "Show more", "Show in grid", "Edited only". |
| **Garis bawah diagnostik** (V-11) | Leksikal: garis 1 putus-putus `markAmber`. Server: garis tebal `markCoral`. | Dilepas saat teks berubah (galat server). | |
| **Readout** (V-11) | Di sudut kanan bawah editor bersama hitungan baris: glyph segitiga dan "· N issues", `ink 0,9`. | Hilang bila nol. | "· 2 issues" |
| **Pasangan kurung** (V-10) | Dua pita `accent 0,22` sudut 2 dengan tepi 1 `accent 0,8`. | Muncul 30 ms sesudah caret diam. | |
| **Gutter editor** (V-10) | Terang: hitam 4,5% (recess). Gelap: putih 3,5%. Nomor 11 pt. Seam `Tone.hairline`. | | |
| **Ukuran font** | Editor 10 sampai 28 (bawaan 12,5), grid 11 sampai 16 (bawaan 12). | Satu penerapan per ketukan kunci. | "Increase Font Size", "Decrease Font Size", "Actual Size" |

**Gerak.** Reduce Motion mematikan pudar peek. Tidak ada animasi lain di W10.

**Hierarki kontras.** Nomor baris dan NULL naik ke 0,60, jadi lebih dekat ke teks sel (0,90) daripada sebelumnya. UX memutuskan dari `grid-contrast-*` bahwa hierarki masih terbaca, dan bila tidak, 0,60 tidak bisa diturunkan (itu batas 4,5:1 di kanvas Cloud); yang bisa diubah adalah menaikkan teks sel ke 1,0 pada kontras normal.

## 11. Spesifikasi AX (sudut pandang a11y-architect)

| Permukaan | Peran dan trait | Label | Nilai dan petunjuk | Aksi dan kunci | Pengumuman |
|---|---|---|---|---|---|
| **Grid** | tabel | "Result grid" | jumlah baris dan kolom | panah, ⇧panah, ⌘panah, Page, Home/End, Return, Esc, Tab, Space, ⌫, ⌘C, ⌘⇧C, ⌥⌘3 | "N × M cells selected" 250 ms sesudah ketukan perluasan terakhir dan di `release()` |
| **Sel** | sel; bisa difokuskan | "Row r, column c, <kolom>: <nilai>" + ", changed" / ", new row" / ", marked for deletion" | NULL dibaca "null" | VoiceOver memfokuskan sel → kursor pindah tanpa mengubah seleksi | |
| **Header** | tombol | nama kolom | tipe; arah sort | tekan = sort; aksi "Filter" (area klik 20 × tinggi header) | |
| **Peek** | panel mengambang | "Cell peek" | isi sebagai teks statis | Space, ⌘Y, Esc | "Peek: <kolom>, <nilai 160>" saat dibuka |
| **Tombol footer** | tombol | "Review N changes", "Discard changes", "Add row", "Delete selected rows" | bantuan = alasan bila nonaktif | ⌘S (tinjau), ⌥⌘N, ⌘⌫ | "Row added. N pending changes." |
| **Panel Record** | grup; sakelar dengan `.isSelected` | "Record", "Cell", "Record" (segmen) | bidang: "<kolom>, <tipe>: <nilai>, <status>" | "Show in grid"; ⌥⌘I, ⌥⌘3 | "Record: row N" (debounce 250 ms) |
| **Editor** | teks (`NSTextView`) | tidak berubah | | rotor "Statements" dan "Query issues" | galat server lewat banner W9-T5 |
| **Rotor** | `NSAccessibilityCustomRotor` | "Statements", "Query issues" | item: label §8.3 | VO-U, panah | |
| **Readout** | teks statis | "N query issues" | | | |
| **Pasangan kurung** | visual saja | | | | tidak diumumkan |

**Urutan fokus** (melengkapi W9 §14): editor → tab panel → grid → panel Record (bila terbuka) → footer. Esc dari peek mengembalikan fokus ke grid tanpa mengubah kursor.

**Pemeriksaan otomatis:** `GridAccessibilityTests`, `GridKeyMapTests`, `CellPeekPanelTests`, `RecordPanelTests`, `EditorRotorTests`, `GridPaletteContrastTests`. **Dijalankan pemilik** dari laporan akhir: smoke VoiceOver pada grid (navigasi sel dengan kursor, staged, peek), rotor editor, dan IME di overlay editor sel (Return menyelesaikan komposisi, bukan menyimpan; sudah tercatat di Fase 5 §13 butir 30).

## 12. Gate, scene, dan urutan

### 12.1 Peta V ke scene

| Kode | Tugas | Scene (satu commit `test(visual): re-record <scene> for V-n` per scene) |
|---|---|---|
| V-9 | T1 | `grid-selection`, `grid-inspector`, `grid-edits`, `grid-counted` bila punya seleksi (cincin) |
| V-9 | T2 | semua 30 scene grid (palet dan font gutter); baru: `grid-contrast-*`, `grid-staged-*` |
| V-9 | T3 | scene dengan edit staged (`grid-edits`) dan footer |
| V-10 | T7 | 16 scene `editor-*` (gutter, nomor 11 pt) |
| V-11 | T1, T5, T6 | baru tanpa baseline lama: peek (lewat tes render), `grid-record-*`, `editor-issues-*` |

`grid-edits` dapat direkam ulang sampai tiga kali (T1, T2, T3), masing-masing karena perubahan yang berbeda dan di commit sendiri. Persetujuan UX dan AX diperlukan tiap kali (§0.5). Scene di luar daftar yang bergerak adalah gate gagal.

### 12.2 Tes yang ditulis lebih dulu

T1 `GridKeyMapTests`, `GridCursorMathTests`, `CellPeekPanelTests`, perluasan `GridParityTests` dan `GridAccessibilityTests`. T2 `GridPaletteContrastTests`, `CellEditsStateTests`, perluasan `GridParityTests` dan `GridMetricsTests`. T3 `CellEditsRowTests`, `GridRowSpaceTests`, perluasan `WritePlanTests`, `CellEditUndoTests`, `ResultGridTests`, dan G-LIVE PostgreSQL. T4 perluasan `InsertStatementsTests` dan `UpdateLiteralTests`, `GridCSVTests`. T5 `RecordPanelTests`. T6 tes Rust `qh-core` dan driver, tes `crates/qh-ffi/tests` untuk event `error`, `EditorDiagnosticsTests`, `ServerErrorMarkTests`, `DiagnosticsPainterTests`, `EditorDialectTests`, perluasan `EditorRotorTests`. T7 `BracketPairTests` (Rust), `EditorBracketsTests`, `EditorFontSizeTests`, `GutterRecessTests`. Hitungan tes Rust dan Swift tidak turun (NFR-Q).

### 12.3 Gate W10

G-HEAVY (G-RUST, G-DENY, G-FFI, G-SWIFT, G-VIS, G-GOLDEN, G-APP) dan G-BENCHQ (`scroll-30x1m`, `scroll-500x10k`, `type-10k`, dan `ttfr-s1-1k`; tidak mundur lebih dari 5%, NFR-P9). Pemindai: daftar izin `FontFloorTests` untuk berkas grid dan editor **kosong**, dan tidak ada `Tone.ink.opacity(0.07)` tersisa. G-LIVE PostgreSQL untuk `apply_changes` dan G-LIVE tiga driver untuk posisi galat.

### 12.4 Urutan dan kepemilikan berkas

Lane (dari plan): grid T1 → T2 → T3 → T4, record T5 sesudah T1, editor T6 → T7; maksimal tiga lane serentak.

Perubahan yang dibutuhkan di `development-plan.md`:

- **§5 W10, berkas:** T1 `+ Views/GridTableView.swift, Views/GridRowView.swift, Models/CellSelection.swift, Models/AppMenu.swift, Models/Shortcuts.swift`; T2 `+ Models/GridMetrics.swift, Views/ResultGridTable.swift`; T3 `+ Models/QueryTab.swift, Models/CellSelection.swift, Views/GridAccessibility.swift, Views/GridTableView.swift, Models/AppMenu.swift`; T4 `+ Models/UpdateStatements.swift, Views/ResultGrid.swift`; T5 `+ Views/ResultGrid.swift, Models/QueryTab.swift, Models/AppMenu.swift`; T6 `+ crates/qh-ffi/src/{lib.rs,main.rs,uniffi_api.rs}, App.swift, Models/QueryTab.swift, Models/AppModel+Run.swift, Views/Workspace.swift`; T7 `+ crates/qh-editor/src/lib.rs, Views/Workspace.swift, Models/AppMenu.swift, Models/Shortcuts.swift, Models/AppModel+Focus.swift`.
- **§7, rantai:**

| Berkas | Rantai |
|---|---|
| `Models/QueryTab.swift` | ... W6-T1 → **W10-T3 → W10-T5 → W10-T6** → W11-T4 → W12-T2 → W12-T4 → W13-T1 |
| `Views/ResultGrid.swift` | W6-T1 → W9-T5 → W10-T3 → **W10-T4 → W10-T5** → W12-T4 → W13-T1 → W13-T3 |
| `Views/Workspace.swift` | ... W9-T8 → **W10-T6 → W10-T7** → W12-T2 |
| `App.swift` | ... W9-T8 → W10-T3 → **W10-T6** → W12-T2 → W13-T8b |
| `Models/AppMenu.swift`, `Models/Shortcuts.swift` | W9-T2 → W10-T1 → W10-T3 → W10-T5 → W10-T7 |
| `Models/AppModel+Focus.swift` | W9-T2 → W9-T6 → **W10-T7** → W12-T2 (`adjustFontSize`, §9.2) |
| `Models/CellSelection.swift` | W10-T1 (`GridCursorMath`) → **W10-T3** (`GridClipboard.text` memilah blok, §5.1) → W10-T4 (`GridClipboard`) |
| Lane FFI (`uniffi_api.rs` dan sekutunya) | ... W6-T1 → **W10-T6** → W10-T7 → W11-T1 → ... (T6 tidak meregenerasi `app/Generated/`) |
| `crates/qh-core/src/error.rs` | W10-T6 |

- **PRD:** FR-ED-09 (rotor sungguhan dipasang di T6, bukan sejak 4B), FR-GRID-08 (tafsiran separator, D-5), FR-GRID-11 ("responder chain", D-6), FR-ED-06 (satu satuan `position`), dan P-06 (nama setelan `ERROR_POSITION`, dilaporkan ke W11-A1).
- **`app/DESIGN.md` (W10-D):** bagian grid (peta kunci, tata bahasa staged, Record, peek) dan editor (diagnostik, rotor, kurung, ukuran).

## 13. Risiko

| ID | Risiko | Mitigasi |
|---|---|---|
| R-1 | Ruang baris D-2 dan jendela baris W6 (`WindowedRows`, bila P-1 gagal) sama-sama memetakan baris tabel. | Keduanya lewat `resultRow(forTableRow:)`; `GridRowSpaceTests` menambah kasus dengan `base ≠ 0`. |
| R-2 | Rekam ulang `grid-edits` tiga kali. | Tiap commit hanya membawa perubahan tugasnya; urutan T1 → T2 → T3 sudah ditetapkan. |
| R-3 | Panel peek merebut fokus atau menelan panah. | P-1a; jatuh ke popover. |
| R-4 | Undo tidak mencapai `GridTableView` lewat item Edit sistem. | P-3a dan jalan mundur `CommandGroup(replacing: .undoRedo)`. |
| R-5 | `NSFilePromiseProvider` dari subclass `NSTableView` berperilaku berbeda di Finder dan Numbers. | P-4a (manual); jalan non-pointer "Save Selection as CSV…" ada. |
| R-6 | Literal MySQL di salinan INSERT salah pada `NO_BACKSLASH_ESCAPES`. | Catatan hasil, dan reviewer DB memutuskan. |
| R-7 | Semantik posisi (PostgreSQL karakter, kolom Trino, "at line N" MySQL) berbeda dari dugaan. | Tes G-LIVE per driver sebelum Swift (6a lebih dulu), dan fungsi murni diuji dengan emoji dan CJK. |
| R-8 | `uniffi_api.rs` ada di lane FFI. | T6 diurutkan tepat sebelum T7 di lane itu. |
| R-9 | Tafsiran separator (D-5) berbeda dari maksud pemilik. | Ditandai; mengubahnya hanya mengganti nilai di tabel §4.1. |
| R-10 | Mengunci kotak gutter 25 pt mengubah luapan Compact. | P-2a; G-VIS lapis tata letak mendeteksinya. |
| R-11 | `accessibilityCustomRotors` pada subclass `NSTextView` di dalam `NSViewRepresentable` tidak muncul di VoiceOver. | `EditorRotorTests` menguji delegat langsung; smoke VoiceOver pemilik. |
| R-12 | `QueryTab.sql` `didSet` membanding teks panjang tiap edit. | Hanya bila `errorMark != nil`, bandingkan panjang lebih dulu. |
| R-13 | Konflik `App.swift` dan `QueryTab.swift` antar-tugas. | Rantai §7 di atas. |
| R-14 | Blok seleksi yang melintasi baris hasil dan baris yang ditambah menghasilkan UPDATE atau DELETE berkunci baris yang tidak ada, atau iterasi dari id negatif ke indeks positif. | Seleksi di ruang baris tabel, id negatif hanya lewat `GridRowSpace` dan tiap mutator memilah blok dengan `split` (§5.1); invarian kunci edit diuji setelah tiap mutator (`CellEditsRowTests`). |
| R-15 | Posisi galat server dipetakan ke tempat yang salah di editor: awal teks yang dikirim berbeda dari caret, awal statement, atau awal dokumen. | `SentSQL.documentStart` diukur sesudah fallback dan pemangkasan (§8.2); `explain` tidak memasang `ERROR_POSITION`; `ServerErrorMarkTests` untuk seleksi kosong dan statement berspasi di depan. |

### 13.1 Yang tidak bisa diverifikasi (ditulis tanpa build atau jalan)

1. Tanda tangan dan semantik `tokio_postgres::DbError::position()` (tipe `ErrorPosition::Original`) dan bahwa satuannya karakter, bukan byte.
2. Satuan `columnNumber` Trino dan bahwa `errorLocation` ada untuk semua galat sintaks.
3. Format pesan MySQL "at line N" di semua versi dan MariaDB.
4. Bahwa `NSTableView` subclass dapat memulai `NSFilePromiseProvider` tanpa berebut dengan penanganan `mouseDown` sendiri.
5. Bahwa `NSPanel` nonaktivasi tidak mengubah first responder, dan `hidesOnDeactivate` menutupnya saat app mundur.
6. Bahwa item Edit > Undo sistem menemukan `undo:` pada first responder `GridTableView` dan memakai `undoManager`-nya untuk nama aksi.
7. Perilaku `NSAccessibilityCustomRotor` dengan target `NSTextView` dan `targetRange`, dan pembacaan VoiceOver nyata untuk rotor, sel grid, dan peek.
8. `GridMetrics.lineHeight` untuk font 11 pt (hipotesis Fase 5 menghasilkan 13 untuk 10,5; nilai untuk 11 belum dihitung) dan selisih luminans gutter terang.
9. Anggaran waktu `bracket_pair` untuk statement besar; 1 MiB dan 1 ms adalah anggaran usulan, belum diukur.
10. Bahwa tidak ada pemanggil `CliError::Query` selain lima yang ditemukan grep (`crates/`), dan bahwa `Settings::flag("ERROR_POSITION", false)` terjangkau dari kedua pemancar `fail`.

## Untuk pemeriksa

AR, UX, AX, DB, dan RR diminta memutuskan:

1. **D-1 dan D-2:** satu penulis (`selectCells`) dan ruang baris dengan id negatif untuk baris yang ditambah.
2. **D-4 dan D-5:** bentuk tanda staged dan tafsiran separator ≥ 3:1 hanya di Increase Contrast (keputusan pemilik).
3. **D-6:** ⌘S lewat `currentRegion` dan ⌘Z lewat responder; apakah itu memenuhi P-16.
4. **D-9:** literal sadar dialek untuk salin INSERT dan penolakan kolom biner.
5. **D-10:** aturan seret keluar (hanya dari dalam seleksi) dan padanan non-pointer.
6. **D-11 dan D-12:** diagnostik hanya leksikal dan galat server, satu satuan `position`, setelan `ERROR_POSITION`.
7. **D-13:** pemasangan rotor sungguhan di T6, yang bukan bagian 4B walau FR-ED-09 menyebut "sejak rewrite 4B".
8. **D-14 sampai D-17:** kurung dari `walk` (bukan pohon), recess gutter 4,5%, ⌘+/− per wilayah, dan nomor gutter 11 pt dengan kotak 25 terkunci.
9. Kelengkapan §10 dan §11, dan apakah peta kunci §3.1 cukup untuk pengguna keyboard.

## Verdict architect-reviewer

**Verdict: perlu revisi (changes requested), 6 Okt 2026.** Tiga temuan memblokir. Ketiganya **sudah diterapkan** ke dokumen ini (D-2 dan D-12 di §2, §5.1, §5.5, §8.1, §8.2, §8.7, §9, §12.4, R-14, R-15) dan **menunggu pemeriksaan ulang** menurut O-20: satu putaran review, temuan memblokir diperbaiki dan diverifikasi terhadap kode yang dikutip, dicatat "pending review" di ledger, tanpa putaran ketiga. Koreksi ditulis tanpa build atau jalan. Butir non-blocking di bawah dicatat dan **tidak** diterapkan. W10-T2 tidak boleh mulai sebelum pemilik menjawab D-5.

### Klaim yang diperiksa di kode

Benar menurut pemeriksa:

- Setiap driver menulis `position: None`, dan `From<EngineError> for CliError` membuang `code` dan `position` (`crates/qh-ffi/src/lib.rs:259`).
- `WireError` Trino tidak punya `errorLocation`.
- Dialek editor `.generic` (`Support/EditorAnalysis.swift:111`, `:389`).
- Tidak ada `accessibilityCustomRotors` di mana pun di `app/Sources`.
- `registerGridEdit` `private` dengan nama aksi tetap "Edit Cell".
- Font gutter 10,5 di `Models/GridMetrics.swift:126` dan `Views/GridRowView.swift:143`.

Salah atau tidak lengkap, **memblokir** (diterapkan):

1. **§5.1/D-2 tidak menyebut ruang koordinat `cellSelection` dan `cellCursor` setelah ada baris yang ditambah, dan mutator massal mengindeks `result` dengan nomor baris.** `CellEdits.fill` mengulang `range.top...range.bottom` dan menyimpan edit UPDATE dengan `original` dari `rows` (`Models/CellEdits.swift:82-90`); `paste` memberi kunci `origin.row + down` (`:99-107`); `QueryTab.fetchedValue` memanggil `result.fullValue(row:)` langsung (`Models/QueryTab.swift:775-777`); `Coordinator.drag` menjepit ke `rows.count` (`Views/ResultGridTable.swift:544-547`). Blueprint hanya mencabangkan `cellValue`, `beginCellEdit`, dan `endCellEdit`. Isi, hapus (⌫), salin, atau Copy as INSERT atas blok yang melintasi baris hasil ke baris yang ditambah akan menyimpan edit UPDATE atau DELETE berkunci baris hasil yang tidak ada, atau mengulang dari id negatif ke indeks positif, dan jalur itu membangun SQL yang menulis ke database. **Diterapkan:** seleksi dan kursor didefinisikan di ruang baris tabel dengan konversi per sel lewat `GridRowSpace` (`cellKey(forTableRow:source:)`, `tableRow(for:)`, `split`); tabel di §5.1 menyatakan perilaku tiap mutator dan pembaca (isi dan tempel lewat `setInserted`, hapus lewat `removeInserted`, salin membaca nilai staged), kestabilan indeks (`reconcileSelectionWithRowSpace`), dan invarian kunci edit; `fetchedValue` mengembalikan `nil` untuk baris negatif; `CellEdits.fill` melewati baris tak terbaca; `GridRowSpaceTests` dan `CellEditsRowTests` mendapat kasus blok yang melintasi batas (§5.5).
2. **§8.2: pemetaan posisi server ke posisi editor salah.** `documentOffset(for: .selection) = selection.location` gagal di jalur Run bawaan: `runSelectedTab` memanggil `preview(tab)` dengan `source: .selection` (`Models/AppModel.swift:1953-1955`, `:1964`), dan dengan seleksi kosong `QueryTab.sql(for: .selection)` jatuh ke seluruh dokumen (`Models/QueryTab.swift:966-969`), jadi offset harus 0, bukan posisi caret, atau setiap Run tanpa seleksi menggarisbawahi tempat yang salah. `.statement` mengembalikan teks yang dipangkas spasi dan baris baru (`sqlStatements`, `QueryTab.swift:1180`), jadi "awal statement" bukan awal teks yang dikirim. Rencana juga memasang `ERROR_POSITION=1` untuk `explain`, padahal driver MySQL dan Trino mengirim `EXPLAIN <sql terpangkas>` (`qh-driver-mysql/src/lib.rs:806`, `qh-driver-trino/src/lib.rs:1397`), sehingga posisinya relatif terhadap teks terbungkus. **Diterapkan:** `QueryTab.sent(for:) -> SentSQL` mengembalikan teks yang dikirim bersama awal UTF-16-nya, diukur sesudah pemangkasan dan sesudah fallback seleksi kosong (`sqlStatementLocated` untuk `.statement`); `explain` dikeluarkan dari `ERROR_POSITION` (§8.1 dan §8.2); `runPreview` hanya membuat tanda untuk Run (`preview(_:from:)`); `ServerErrorMarkTests` mendapat kasus seleksi kosong dan statement berspasi di depan (§8.7).
3. **Kepemilikan berkas: usulan perubahan §5 dan §7 meninggalkan berkas yang disunting tugas, dan itu melanggar aturan satu pemilik (`development-plan.md` §7).** Untuk W10: W10-T7 menambah `AppModel.adjustFontSize` (§9.2) tetapi tidak ada extension `AppModel+` di daftar berkas T7, dan rantai `AppModel+Focus.swift` (W9-T2 → W9-T6 → W12-T2) tidak memuat W10-T7. **Diterapkan:** T7 `+ Models/AppModel+Focus.swift`, baris rantai `Models/AppModel+Focus.swift` = W9-T2 → W9-T6 → W10-T7 → W12-T2, di §9, §9.2, dan §12.4. Koreksi no. 1 menambah `Models/CellSelection.swift` dan `Views/GridAccessibility.swift` ke daftar T3 dan baris rantai `Models/CellSelection.swift` (W10-T1 → W10-T3 → W10-T4). Bagian W9 dari temuan yang sama (Snapshot, `AppModel.swift`, `AppModel+Tree.swift`) diterapkan di W9 §16.2.

Diperiksa ulang oleh penerap koreksi dengan membaca kode (tanpa build): `CellEdits.fill` dan `paste` (`CellEdits.swift` :82-90, :99-110, `fill` menyimpan edit dengan `original: nil` untuk baris di luar hasil); `GridClipboard.text(result:…)` memanggil `rowsOrThrow(in: top..<bottom + 1)` (`CellSelection.swift` :139-154); `WritePlan.build` hanya menolak kunci tak terbaca dengan peringatan "could not be read" (`WritePlan.swift` :141-145, :162-164); `Coordinator.press` dan `drag` menjepit ke `rows.count` dan `validKey` menolak `row >= rows.count` (`ResultGridTable.swift` :523-547, :596-599); `commitEdit` memanggil `fillCellEdits` (`:690-691`) dan `pasteIntoSelection` memberi asal `selection.top` (`ResultGrid.swift` :617-622); `cellSelection.didSet` (`QueryTab.swift` :930-940); `sql(for:)` (`:962-974`), `sqlStatements` memangkas (`:1180`); `source_sql` mengembalikan `settings.raw("SQL")` tanpa memangkas (`commands.rs:498-502`); ketiga driver membungkus `EXPLAIN ` (`qh-driver-postgres/src/lib.rs:668` juga, bukan hanya MySQL dan Trino). `Coordinator.resultRow(forTableRow:)` dan `tableRow(forResultRow:)` hari ini tidak punya pemanggil selain dokumentasi.

### Keputusan atas pertanyaan di "Untuk pemeriksa"

1. **D-1 dan D-2:** satu penulis (`selectCells`) diterima. D-2 tidak lengkap sampai ruang seleksi dan perilaku tiap mutator dinyatakan; sekarang dinyatakan di §5.1 (diterapkan, menunggu pemeriksaan ulang). Lihat butir non-blocking tentang setter fokus AX.
2. **D-4 dan D-5:** **terbuka untuk pemilik.** D-5 (pemisah ≥ 3:1 hanya di Increase Contrast) adalah tafsiran FR-GRID-08, yang menulis "separator ≥ 3:1" tanpa pembeda, dan sudah ditandai dengan benar. **W10-T2 tidak boleh mulai sebelum pemilik menjawab D-5.** Bentuk tanda staged (D-4) tidak dipersoalkan.
3. **D-6:** lihat butir non-blocking tentang `canSaveFocused`. Pertanyaan apakah D-6 memenuhi P-16 tidak diputuskan di putaran ini.
4. **D-11 dan D-12:** §8.2 memblokir (diterapkan). Satu satuan `position` dan nama setelan `ERROR_POSITION` tidak dipersoalkan; pemancar galat di §8.1 punya butir non-blocking.
5. Pertanyaan 4, 5, 7, 8, dan 9 (D-9, D-10, D-13, D-14 sampai D-17, dan kelengkapan §10 dan §11): tidak ada temuan tercatat di putaran ini.

### Perubahan rencana yang dibutuhkan (untuk orkestrator)

**Memblokir, diterapkan.** Daftar "Perubahan yang dibutuhkan di `development-plan.md`" di §12.4 sudah dikoreksi: T3 `+ Models/CellSelection.swift, Views/GridAccessibility.swift`; T7 `+ Models/AppModel+Focus.swift`; baris rantai baru `Models/AppModel+Focus.swift` dan `Models/CellSelection.swift`. Versi W9 dari rantai `AppModel+Focus.swift` (dengan W10-T7) ada di W9 §16.2 dan harus sama. Orkestrator menyalin keduanya ke `development-plan.md` §5 dan §7 **sebelum implementer pertama dikirim**, sesudah pemeriksaan ulang.

**Tidak memblokir.** §12.4 menyusun rantai `ResultGrid.swift` T3 → T4 → T5, `QueryTab.swift` T3 → T5 → T6, `AppMenu.swift` T1 → T3 → T5 → T7, dan `App.swift` T3 → T6, sehingga lane record dan editor menunggu lane grid dan rencana "tiga lane serentak" praktis serial. Jadwal harus ditulis ulang apa adanya.

### Risiko terbuka (tidak memblokir)

Dicatat dan **belum diterapkan**:

- **D-6/§5.4:** `.disabled(!model.canSaveFocused)` bergantung pada `currentRegion`, yang menurut W9 §5.5 dihitung saat ditanya dan tidak teramati, jadi status menu basi sesudah fokus pindah. Aktifkan item dari keadaan teramati (tab terpilih punya edit staged) dan putuskan menurut wilayah di dalam `saveFocused()`. Sama untuk setiap `AppMenu.isEnabled` yang membaca `currentRegion`.
- **§3.4:** setter fokus AX menulis `cellCursor` tanpa mengubah seleksi. Itu bertentangan dengan D-1 (`selectCells` satu-satunya penulis) dan dengan invarian "kursor di dalam seleksi" di `cellSelection.didSet` (`QueryTab.swift:930-939`). Tambahkan penulis kedua yang sah dan dokumentasikan, atau runtuhkan seleksi seperti gerak keyboard.
- **§8.1:** pemancar galat yang dikutip di `main.rs:47` salah (baris itu melaporkan kegagalan membangun runtime). Jalur kegagalan query adalah `report()` (≈ `main.rs:95`) dan jalur UniFFI adalah `fail` di `uniffi_api.rs:355`/`:376`, yang butuh `settings` dialirkan ke dalamnya. `CliError::QueryAt` juga butuh cabang baru di setiap `match` yang menyeluruh (`message()`, `warnings()`, pemetaan kode keluar), jadi "lima pemakai tidak berubah" meremehkan diff. Kalimat itu dan `main.rs:47` di §8.1 sengaja dibiarkan sampai pemeriksaan ulang.
- **§12.1 dan D-18:** "semua 30 scene grid" salah. `__Baselines__` berisi 16 scene grid × 2 appearance = 32 PNG, ditambah 8 scene editor × 2 = 16 PNG (dihitung ulang 6 Okt 2026). Perbaiki hitungan supaya daftar rekam ulang V-9 bisa dicek.
- **§12.4:** jadwal serial, lihat bagian di atas.
- **§1.1:** GridPalette dibangun dari `NSAppearance` lewat `performAsCurrentDrawingAppearance` (`Views/GridRowView.swift:38-65`, bukan dari `colorScheme` SwiftUI). Kata-katanya tidak berbahaya, tetapi token `Tone.mark*` baru butuh bentuk `NSColor` dinamis untuk jalur AppKit ini.
- **`App.swift`:** W10-T3 (`CommandGroup(replacing: .saveItem)`) dan W10-T6 (`Event.position`, `App.swift:164`) sama-sama menyunting `App.swift`; lihat butir W9 D-3 di dokumen W9. Belum diketahui apakah `.saveItem` juga memegang item Close ⌘W sistem; uji bersama §17.2 butir 1 W9.
- **Keputusan pemilik yang masih terbuka:** D-5 (di atas).

Ditemukan saat menerapkan koreksi, di luar daftar pemeriksa, **tidak diterapkan** (untuk pemeriksaan ulang):

- `saveFocused()` dan `canSaveFocused` (§5.4) tidak disebut berada di extension mana. Bila ditaruh di `AppModel+Focus.swift`, T3 harus masuk rantai berkas itu; bila di `AppModel+Edit.swift` (berkas T3), tidak ada perubahan rantai. Hal yang sama untuk penangan `peekCell` (T1) dan `toggleRecord` (T5) di `AppMenu.perform`.
