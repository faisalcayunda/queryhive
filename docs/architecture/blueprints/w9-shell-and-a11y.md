# Blueprint W9: fondasi desain (pecahan `AppModel`, pintasan, lantai aksesibilitas, pohon outline, shell native, lantai 11 pt)

- **Status:** blueprint tingkat berkas, 6 Okt 2026 (W9-A1). Belum ada kode yang ditulis. Bagian "Verdict architect-reviewer" di ujung sudah diisi pemeriksa (6 Okt 2026): koreksi yang memblokir sudah diterapkan dan menunggu pemeriksaan ulang (O-20).
- **Untuk:** W9-T0 sampai W9-T9 (`development-plan.md` §5 W9) dan satu tugas tambahan yang diusulkan di sini, W9-T0b (baseline chrome, D-19). Pemeriksa mengikuti kolom Gate tiap tugas: SR, AX, UX, AR, CR.
- **Sumber:** PRD FR-UI-01…10, FR-TREE-04/05, FR-RUN-06/07, FR-CON-06, FR-SAFE-01, FR-ED-07 (setelan), NFR-A1…A5, NFR-V (V-2…V-8), P-14, P-16, P-22; `tablepro-design-audit.md` §1, §3, §9 sampai §17; blueprint Fase 5 (grid, ADR-0032) dan Fase 6 §17 (seam pasca-W6); `app/DESIGN.md`; `docs/invariants.md`. TablePro (AGPL) hanya dibaca untuk ide lewat audit desain. Tidak ada kode, aset, atau string yang disalin.
- **Bukti:** kode di `8103478` (ditambah `2d14eea`, commit 6a W6-T1). Nomor baris `AppModel.swift`, `QueryTab.swift`, `ResultGrid.swift`, dan `ResultGridTable.swift` akan bergeser karena 6b W6-T1 sedang dikerjakan di lane lain, jadi implementer menurunkannya ulang lewat **nama simbol**. Berkas lain stabil. Angka kontras dihitung WCAG 2.x dari nilai heksadesimal (skrip sekali pakai, hasilnya ada di tabel §1.8), **bukan** diukur di layar. Nama dan ketersediaan API SwiftUI dan AppKit dibaca dari `swiftinterface` dan header SDK `MacOSX27.0.sdk` di host. Dokumen ini ditulis tanpa build, jadi perilaku runtime yang tidak bisa dibuktikan dari kode ditandai sebagai probe (§11.4) atau ditulis di "Yang tidak bisa diverifikasi" (§17.2).
- **Bahasa:** dokumen ini Indonesia. Identifier, komentar kode, string yang tampil di app, dan nama tes Inggris.

## Ringkasan

W9 mengganti fondasi chrome tanpa menambah fitur data. Isinya sembilan pekerjaan, dan tiap pekerjaan punya satu pemilik berkas:

1. **Pecahan `AppModel` (T0).** Satu kelas 3.444 baris menjadi `AppModel.swift` (hanya properti tersimpan, `init`, dan turunan dasar) ditambah sembilan extension per domain. Perpindahan murni, dengan 18 anggota `private` yang harus menjadi internal dan daftar tes pemindahan yang bisa dijalankan mesin (§3).
2. **Lantai aksesibilitas chrome (T1).** Chip tab menjadi tombol, tombol ikon wajib bernama, satu sumber untuk Reduce Motion, Reduce Transparency, dan Increase Contrast lewat `ThemeStore`, token kontras, dan tiga salinan basi (§4).
3. **Pintasan (T2).** Satu sumber data menu (`AppMenu`), tiga tabel (DBeaver, QueryHive, platform), tes konflik di atas gabungan semuanya, menu View, kunci tab, dan model fokus yang dipakai juga oleh W10 (§5).
4. **Pohon `NSOutlineView` (T3).** View-based, dengan tanda panah chevron yang sama, keyboard dan VoiceOver gratis dari AppKit, dan menu konteks dari satu deskripsi data (§6).
5. **Tag lingkungan dan Safe Mode di chrome (T4), banner galat dan lembar konfirmasi (T5), Open Quickly (T6), Settings (T7)** (§7 sampai §10).
6. **Shell native (T8):** `NavigationSplitView` dan `.toolbar`, dengan harness snapshot lewat `sceneBridgingOptions` (API terverifikasi ada di macOS 14) dan tiga probe yang menentukan jalan mundur (§11).
7. **Lantai 11 pt (T9):** pemetaan per pemanggilan, dan keputusan untuk **tidak** menyentuh berkas yang digambar grid dan editor sampai W10 (§12).

Lima temuan yang mengubah rencana (detail di §1):

- **G-VIS tidak menjaga chrome.** `VisualParityTests` hanya punya 32 baseline `grid-*` dan 16 `editor-*`. Perubahan V-2 sampai V-7 tidak punya baseline untuk dibandingkan, dan `SidebarRenderTests` memakai `ImageRenderer` yang tidak bisa menggambar `NSViewRepresentable`. Karena itu diusulkan W9-T0b (D-19).
- **Pintasan default bolong.** Skema DBeaver (default) tidak mengikat Close Tab, Open Quickly, Stop, dan Save. Keyboard pengguna tidak punya jalan ke Open Quickly dan ke Stop di skema default (D-6).
- **Truncate dan Drop Table langsung jalan di Safe Mode `full`** walau menu bertitik tiga (`AppModel.requestTableOperation`, doc di atasnya). Ini kontrak yang diuji (`RunConfirmationTests.testADestructiveOperationOnlyAsksAtConfirm`), jadi mengubahnya adalah keputusan yang ditandai untuk pemilik (D-11).
- **"Banner di atas hasil sebelumnya" (FR-RUN-06) tidak bisa dipenuhi apa adanya.** Run menghapus `tab.preview` di awal (`runPreview`), dan W6 melepas store lama (Fase 6 §18). Banner berdiri di atas badan kosong (D-13).
- **Pewarnaan kategorikal gagal kontras 3:1 di tiga kanvas terang** (amber 1,6:1, mint 1,5:1, coral 2,7:1). Ini menentukan tanda staged di W10-T2 dan garis bawah diagnostik di W10-T6, jadi nilai varian terang ditetapkan di sini (§1.8, D-7).

## 1. Fakta yang diperiksa sebelum merancang

### 1.1 `AppModel`

| Fakta | Bukti |
|---|---|
| Satu kelas `@Observable`, 3.444 baris, kelas berakhir di baris 3427, dua struct pendamping (`EngineLaunchError`, `PendingConfirmation`) sesudahnya. Belum ada `extension AppModel` di pohon ini. | `Models/AppModel.swift:9-10`, `:3431`, `:3440`; grep `extension AppModel` di `app/Sources` dan `app/Tests` kosong |
| Dua puluh `// MARK:`, tetapi anggota berbeda domain tetap bercampur: lebih dari lima puluh properti tersimpan tersebar antara baris 28 dan 2681, banyak di tengah fungsi domain lain (`parameterPrompt` baris 757, `completion` baris 1767, `searchTasks` baris 2233, `engine` baris 2652). | skrip pengelompokan anggota (scratchpad), daftar di §3.2 |
| Properti tersimpan **harus** tinggal di badan kelas: `@Observable` tidak mengizinkannya di extension, sedangkan `static` boleh. | aturan bahasa; `@Observable` dipakai di `AppModel.swift:9` |
| `private` tidak menyeberang berkas. Ada 32 anggota `private` dan 18 di antaranya dipakai dari domain yang akan berpindah berkas (daftar di §3.3). | skrip silang-pakai (scratchpad) |
| Tidak ada tes, skrip, atau dokumen yang membaca `AppModel.swift` sebagai teks. Hanya PRD yang menyebut nomor baris (`AppModel.swift:1248`) dan itu akan basi. | grep `AppModel.swift` di `app/Tests`, `app/*.sh`, `tools`, `docs/invariants.md` kosong |

### 1.2 Pintasan

| Fakta | Bukti |
|---|---|
| Skema bawaan adalah **DBeaver** (`?? .dbeaver`). Dua skema, empat belas aksi. | `AppModel.swift:53-58`, `Models/Shortcuts.swift:7-23`, `:83-86` |
| Tabel DBeaver mengikat delapan aksi. **Tidak terikat:** `countRows`, `stop`, `saveFile`, `exportData`, `closeTab`, `openQuickly`. | `Shortcuts.swift:117-136` |
| Tabel QueryHive mengikat ⌘E dua kali, untuk `explain` dan `exportData`. Di menu Query, Explain muncul lebih dulu, jadi ⌘E menjalankan Explain dan tombol Export tidak pernah menyala dengan kunci itu (urutan menu: dugaan dari `App.swift:99-114`, tidak dijalankan). | `Shortcuts.swift:141`, `:147`; `App.swift:106-111` |
| `countRows`, `saveFile`, `toggleResultPanel`, `commentLine`, dan `format` tidak punya pemakai di luar tabel Settings. Pane Keyboard menampilkan lima kunci yang tidak melakukan apa pun. | grep `shortcut(for:` dan `ShortcutAction` di `app/Sources`; `SettingsView.swift:1069-1090` |
| Kunci tetap di luar skema: ⌘⇧R "Reveal in Finder" (`Panels.swift:334`), ⌘T "Test Connection" di lembar koneksi (`ConnectionsViews.swift:607`, sama dengan New Query di skema QueryHive), `.cancelAction` dan `.defaultAction` di lembar koneksi (`:618-621`), panah dan Esc di Open Quickly lewat `onKeyPress` (`OpenQuickly.swift:28-30`). | baris tersebut |
| Kunci milik editor, diambil sebelum AppKit melihatnya: ⌘F, ⌥⌘F, ⌘G, ⇧⌘G, ⌥⌘[ , ⌥⌘], Esc, ⌃Spasi, ⌥Esc, dan panah, Return, Tab, Esc saat daftar saran aktif. | `Views/SQLEditor.swift:1100-1160`, `:1571-1590` |
| Skema DBeaver tanpa `closeTab` berarti ⌘W jatuh ke item Tutup bawaan sistem, yang menutup **jendela**, dan `applicationShouldTerminateAfterLastWindowClosed` mengembalikan `true` sehingga app berhenti. Bagian "item Tutup bawaan ada di menu File" tidak diverifikasi (§17.2). | `App.swift:141`, `Shortcuts.swift:117-136` |
| Seluruh perintah menu terpasang di scene `Settings`, bukan di `Window` utama. | `App.swift:59-119` |

### 1.3 Chrome dan aksesibilitas

| Fakta | Bukti |
|---|---|
| Chip tab: `onTapGesture`, bukan tombol. Tombol tutup di dalamnya `opacity(0)` tetapi, menurut audit desain §1, tetap ada di pohon AX (tidak dijalankan di sini). Status run hanya titik 6 pt yang beda warna. | `Views/Workspace.swift:111-165` (`onTapGesture` baris 143, `opacity` baris 133, `dot` baris 157-164) |
| Tab panel bawah (`PanelTabButton`) juga tombol tanpa trait `.isSelected`. | `Views/Panels.swift:102-138` |
| `IconButton` hanya punya `help:` sebagai nama. Lima belas pemanggilan di empat berkas. `Chip` adalah teks 11 pt tanpa label eksplisit untuk tipe ("varchar" dibaca tanpa konteks). | `Support/Theme.swift:380-406`, `:623-635`; grep `IconButton(` |
| Hanya lima belas pemanggilan `accessibility*` di luar grid, hampir semuanya di Settings. Nol pemeriksaan Reduce Motion, Reduce Transparency, atau kontras. | grep `accessibility`, `reduceMotion`, `reduceTransparency`, `colorSchemeContrast` di `app/Sources` |
| `HiveHero` bergoyang selamanya (`repeatForever`), `Backdrop` beranimasi 0,8 dtk, `glass()` dan latar sidebar selalu memakai material. Tiga pemanggil `glass()` ditambah `.thinMaterial` di sidebar. | `Theme.swift:227-267`, `:832-870`; `SettingsView.swift:220`, `SuggestionPopup.swift:37`, `SidebarTree.swift:46`, `:137` |
| `Tone.secondary` dipakai 140 kali, hairline `Tone.ink.opacity(0.07)` 27 kali di tujuh berkas, `0.08` 21 kali, `0.10` 10 kali. Semuanya literal, jadi tidak ada satu titik untuk menaikkannya. | grep `Tone.secondary`, `Tone.ink.opacity(` |
| Salinan basi: `EmptyWorkspace` ("Run streams the result straight to disk"), kalimat sidebar ("Add a Trino coordinator"), placeholder `hive.analytics.penerima_manfaat`, dan dua teks bantuan yang menulis ⌘T dan ⌘R padahal skema bawaan memakai ⌃] dan ⌘↩. | `Workspace.swift:65`, `:99`, `:215`, `:366`; `SidebarTree.swift:128` |
| Dari 295 literal ukuran font, 86 di bawah 11 pt: **55 teks** (`.ui`, `.code`, `NSFont`: 41 di 10,5, tujuh di 10, lima di 9,5, dua di 9) dan **31 glyph** SF Symbol (`.system(size:)`, 7 sampai 10,5). Teks ada di 14 berkas. 16 dari 55 ikut digambar grid dan editor (12 di `ResultGrid.swift`, satu di `GridMetrics.swift`, satu di `GridRowView.swift`, dua `NSFont` di `SQLEditor.swift`). | skrip penghitungan (scratchpad); daftar di §12 |
| Semua teks lewat satu seam: `Font.ui(_:)` dan `Font.code(_:)` (`Fonts.swift:183-190`). | `Support/Fonts.swift` |

### 1.4 Jaring gate yang ada

| Fakta | Bukti |
|---|---|
| `VisualParityTests` hanya menggambar `ResultGrid(tab:)` di 1000×520 dan editor. 32 PNG `grid*` dan 16 PNG `editor-*` di `__Baselines__`. **Tidak ada baseline untuk tab, toolbar, sidebar, status bar, sheet, atau Settings.** | `VisualParityTests.swift:579-740`, `:1039-1072`; `ls __Baselines__` |
| `captureGrid` menggambar `ResultGrid` utuh, termasuk toolbar pencarian dan footer. Mengubah ukuran font di `ResultGrid.swift` menggeser piksel di semua scene grid. | `VisualParityTests.swift:585-624`; `ResultGrid.swift` fungsi `gridToolbar`, `footer` |
| `SidebarRenderTests` menggambar `SidebarTree()` lewat `ImageRenderer`. `ImageRenderer` tidak bisa menggambar `NSViewRepresentable` (catatan di `Snapshot.swift:11-17`), jadi tes itu harus pindah ke jalur `NSHostingView` + `cacheDisplay` yang dipakai tes grid. | `Tests/QueryHiveTests/SidebarRenderTests.swift:55-60`; `Snapshot.swift:11-17` |
| Harness `--snapshot` membuat `NSWindow` ber-`[.titled, .fullSizeContentView]` dengan bilah judul transparan, memuat `RootView` di `NSHostingView`, lalu `cacheDisplay` pada `contentView`. Toolbar bawaan jendela bukan bagian `contentView`. | `Snapshot.swift:145-157`, `:188-195` |
| Scene chrome yang berisi waktu (`done`, `running`, `files`) menampilkan cap waktu relatif terhadap "sekarang" (`Date(timeIntervalSinceNow: -41)`), jadi dua tangkapan tidak identik piksel. | `Snapshot.swift:299-336` |

### 1.5 Shell

| Fakta | Bukti |
|---|---|
| `Window("QueryHive", id: "main")`, `.windowStyle(.hiddenTitleBar)`, `.frame(minWidth: 1120, minHeight: 700)`, ukuran awal 1320×880. Angka 1120 diukur untuk toolbar tujuan-tabel yang sudah tidak ada. | `App.swift:41-52`; komentar `:44-46`; `app/DESIGN.md` "One toolbar" |
| `RootView` = `VStack { TitleStrip (40 pt, kosong); HStack { SidebarTree(width: model.sidebarWidth); SidebarResizer (7 pt); Workspace }; StatusBar }`. Resizer hanya bisa ditarik dengan pointer, dijepit 190…460. | `RootView.swift:5-20`, `:173-197` (jepit baris 192) |
| `Metrics`: `titleStrip` 40, `tabStrip` 36, `toolbar` 46, `statusBar` 26, `control` 28, `treeRow` 23, `treeIndent` 15. `ContextCascade` memakai tiga level selebar tetap 200 pt. | `Theme.swift:308-328`; `ContextCascade.swift:33` |
| `NSHostingView.sceneBridgingOptions` dan `NSHostingController.sceneBridgingOptions` ada, `@available(macOS 14.0)`, dengan opsi `.title`, `.toolbars`, `.all`. Target deployment app macOS 14 (invariant #4). | `SwiftUI.swiftinterface` baris 1080, 12098, 24117 |
| `NSOutlineView`: disclosure kustom lewat `makeView(withIdentifier: NSOutlineViewDisclosureButtonKey)`, `indentationPerLevel` bawaan 16, `NSTableViewStylePlain`, atribut AX `Disclosing`, `DisclosedRows`, `DisclosureLevel`. | `NSOutlineView.h:98-100`, `:396-403`; `NSTableView.h:77-95`; `NSAccessibilityConstants.h:319-322` |

### 1.6 Safe Mode, lingkungan, dan operasi destruktif

| Fakta | Bukti |
|---|---|
| `Connection.safeMode` punya empat nilai (`full`, `no_ddl`, `confirm`, `read_only`), didekode `decodeIfPresent`. **Tidak ada tag lingkungan.** Safe Mode hanya tampil di editor koneksi dan `ImportSheet`. | `Models/Connections.swift:144-175`, `:288`, `:351`; grep `safeMode` di `Views` |
| Truncate dan Drop dari menu pohon: di `confirm` aplikasi bertanya (`RunConfirmation.destructiveRequest`), di `full` **langsung jalan** dan hasilnya hanya notice "Drop done", di `no_ddl` dan `read_only` engine menolak. Judul menu bertitik tiga. | `AppModel.requestTableOperation` dan doc di atasnya; `RunConfirmation.swift:57-64`; `SidebarTree.swift:469-470` |
| `RunConfirmationSheet` tidak punya `.cancelAction` maupun `.defaultAction`. Tombol setujunya `HubButton` beraksen yang sama dengan Run, berjudul "Approve and Run". | `Views/RunConfirmationSheet.swift:11-69` |

### 1.7 Galat

`runPreview` menulis `tab.previewError = nil` dan `tab.preview = nil` di awal. Pada kegagalan ia menulis `previewError` dan `tab.preview = nil` lagi. `ResultGrid.content` menggantikan **seluruh badan** dengan teks coral di tengah, tidak bisa diseleksi dan tanpa tombol salin. Tiga jalur lain menulis `previewError` sebelum run berangkat (galat peluncuran engine). Fase 6 §18 melepas `activeResult` dan `baseResult` saat Run, jadi hasil lama tidak ada lagi saat galat tiba.

### 1.8 Kontras (dihitung, bukan diukur)

Ink = putih di gelap, hitam di terang, dicampur alfa di atas kanvas heksadesimal `AppTheme.canvas`. Tujuh kanvas: Midnight `0A0B1E`, Graphite `151517`, Nord `2E3440`, Ink `060709`, Daylight `F4F6FB`, Cloud `F5F5F7`, Paper `FAF8F4`.

| Alfa ink | Midnight | Graphite | Nord | Ink | Daylight | Cloud | Paper |
|---|---|---|---|---|---|---|---|
| 0,30 (NULL, funnel hari ini) | 2,61 | 2,70 | 2,55 | 2,55 | 2,10 | 2,09 | 2,10 |
| 0,35 (nomor baris hari ini) | 3,15 | 3,22 | 2,94 | 3,10 | 2,42 | 2,42 | 2,43 |
| 0,55 | 6,23 | 6,15 | 4,98 | 6,26 | 4,67 | 4,66 | 4,69 |
| 0,60 | 7,25 | 7,10 | 5,61 | 7,32 | 5,61 | 5,60 | 5,64 |
| 0,68 (`Tone.secondary`) | 9,10 | 8,81 | 6,71 | 9,25 | 7,61 | 7,59 | 7,67 |
| 0,85 | 13,99 | 13,29 | 9,51 | 14,39 | 14,20 | 14,12 | 14,41 |

Alfa minimum agar ≥ 3:1: 0,33 sampai 0,36 di kanvas gelap, 0,42 di kanvas terang. Alfa minimum agar ≥ 4,5:1: 0,45 sampai 0,51 di gelap, 0,54 sampai 0,55 di terang. Jadi **0,60 lolos 4,5:1 di ketujuh kanvas** dan **0,45 lolos 3:1**.

Warna kategorikal tetap, sebagai tanda (non-teks, ambang 3:1), terhadap kanvas:

| Warna | Midnight | Graphite | Nord | Ink | Daylight | Cloud | Paper |
|---|---|---|---|---|---|---|---|
| coral `FF5E6C` | 6,5 | 6,1 | 4,2 | 6,8 | **2,7** | **2,7** | **2,8** |
| amber `FFB547` | 11,1 | 10,4 | 7,1 | 11,5 | **1,6** | **1,6** | **1,7** |
| mint `3EE6A8` | 12,1 | 11,4 | 7,8 | 12,6 | **1,5** | **1,5** | **1,5** |
| ice `4FD8FF` | 11,7 | 10,9 | 7,5 | 12,1 | **1,5** | **1,5** | **1,6** |
| coral dalam `D6283A` | 3,9 | 3,7 | 2,5 | 4,1 | 4,6 | 4,6 | 4,7 |
| amber dalam `9A5B00` | 3,6 | 3,4 | 2,3 | 3,7 | 5,0 | 5,0 | 5,1 |
| mint dalam `0B7D5E` | - | - | - | - | 4,7 | 4,7 | 4,8 |

Glow aksen sebagai cincin fokus: ice, mint, blue, dan magenta turun ke 1,5 sampai 2,9:1 di kanvas terang. Aksen dalam (`deep`) lolos di terang kecuali mint (2,8). Violet glow di Nord tepat 3,0.

## 2. Keputusan desain

| ID | Keputusan | Alasan |
|---|---|---|
| D-1 | **`AppModel` dipecah menjadi `AppModel.swift` plus sembilan extension yang namanya dari rencana**: `+Run`, `+Tree`, `+Connections`, `+Edit`, `+Export`, `+History`, `+Session`, `+Completion`, `+Focus`. Pemetaan per anggota di §3.2. `+Focus` mencakup navigasi (siklus hidup tab, panel, Open Quickly, fokus), `+Edit` mencakup status staged dan preset filter, `+Export` mencakup juga impor, `+Session` mencakup akun dan profil. | Sepuluh berkas akan mengubah §7 tanpa membeli apa pun. Akun dan profil adalah identitas **aplikasi** (ADR-0029 memisahkannya dari identitas database), jadi tidak ditaruh di `+Connections`. |
| D-2 | **Semua properti tersimpan tetap di `AppModel.swift`.** Keadaan baru per domain adalah `struct` bernilai yang dideklarasikan di berkas extension-nya, dan inti hanya mendapat **satu** properti tersimpan untuk tiap struct (`var navigation = NavigationState()`). | Tugas W9–W13 yang menambah keadaan tidak perlu menyentuh inti lagi, jadi aturan "satu pemilik per extension" (`development-plan.md` §7) bertahan. Pengamat Observation melihat struct sebagai satu nilai, cukup untuk keadaan berfrekuensi rendah. |
| D-3 | **Menu adalah data.** `AppMenu.specs(for: scheme)` mengembalikan `[MenuSpec]` (id, judul, `Shortcut?`, kelompok), dan `App.swift` hanya merendernya. Tabel Settings, tes konflik, dan menu membaca daftar yang sama. | Hari ini tiga tempat (menu, tombol toolbar, Settings) membaca `shortcut(for:)` satu per satu dan ⌘E ganda lolos. Data bisa diuji tanpa menjalankan SwiftUI. |
| D-4 | **Tiga kelas kunci, satu aturan.** (a) tabel skema (hanya perintah editor SQL yang punya kunci khas DBeaver atau QueryHive), (b) tabel **platform** yang sama di kedua skema (konvensi macOS: Simpan, Tutup, Stop, Open Quickly, navigasi, tampilan), (c) kunci lokal (editor, lembar, grid). Tes konflik berjalan atas gabungan (a)+(b)+(c) per skema, ditambah daftar kunci **sistem** yang tidak boleh dipakai. | Skema DBeaver sengaja hanya mengikat apa yang DBeaver deklarasikan. Tanpa kelas (b), pengguna default tidak punya kunci untuk Stop, Close Tab, dan Open Quickly (UC-07, NFR-A2). |
| D-5 | **Perbaikan ⌘E:** `exportData` di skema QueryHive menjadi ⇧⌘E. `explain` tetap ⌘E (yang dikirim app sebelum skema ada). | ⌘E sudah menjalankan Explain. Export mendapat kunci yang benar-benar menyala. |
| D-6 | **Tabel platform:** `stop` ⌘., `saveFile` ⌘S, `closeTab` ⌘W, `openQuickly` ⇧⌘O, `revealOutput` ⇧⌘R (dipromosikan dari literal di `Panels.swift:334`), `toggleSidebar` ⌃⌘S, `focusSidebar` ⌥⌘1, `focusEditor` ⌥⌘2, `focusResults` ⌥⌘3, `nextTab` ⇧⌘], `previousTab` ⇧⌘[, serta ⌘1…⌘9 untuk "Go to Tab". Alias ⌃Tab dan ⌃⇧Tab lewat monitor kunci (§5.3). Kunci W10 dicadangkan di tabel yang sama (§5.2). | Konvensi macOS dan Xcode. Pada skema QueryHive tidak ada yang berubah selain ⇧⌘E dan tambahan baru, karena `stop`, `saveFile`, `closeTab`, dan `openQuickly` sudah punya kunci itu di sana. |
| D-7 | **Satu sumber aksesibilitas visual: `ThemeStore`.** Ia membaca `NSWorkspace.shared.accessibilityDisplayShouldReduceMotion`, `...ShouldReduceTransparency`, dan `...ShouldIncreaseContrast`, mengamati `NSWorkspace.accessibilityDisplayOptionsDidChangeNotification` di tempat `AppDelegate` sudah mengamati tampilan, dan menyediakan token dinamis (`Tone.secondary`, `Tone.hairline`, `Tone.outline`, `Tone.focusRing`) serta kebijakan permukaan untuk `glass()`, `Backdrop`, dan `HiveHero`. Snapshot dan tes memakai `pin(...)`. | `Tone` statis dan dibaca di 140 tempat; sebuah `ViewModifier` yang membaca environment tidak bisa mencapainya. Jalur ini sama dengan `systemIsDark`, sehingga Observation sudah menggambar ulang pembacanya. |
| D-8 | **Warna tanda punya varian terang.** Token `Tone.markAmber`, `markMint`, `markCoral` bernilai warna kategorikal di appearance gelap, dan `9A5B00`, `0B7D5E`, `D6283A` di appearance terang (≥ 3:1 di ketiga kanvas terang). Cincin fokus `Tone.focusRing` memilih yang lebih kontras dari {glow, deep} terhadap kanvas aktif, dan jatuh ke `ink` bila keduanya < 3:1. | §1.8. Dipakai W10-T1/T2/T6, ditetapkan di W9 karena `Theme.swift` milik W9-T1. |
| D-9 | **`IconButton` wajib punya `label:`.** Inisialisasi lama (hanya `help:`) tetap ada selama pemanggil di berkas milik tugas lain belum pindah, dengan label diturunkan dari `help`. Tes pemindai sumber menjaga daftar pengecualian per berkas dan harus kosong di gate W9. | Memaksa semua pemanggil dalam satu commit menabrak kepemilikan berkas (T3 menulis ulang `SidebarTree.swift`, T5 dan W10-T3 memiliki `ResultGrid.swift`). |
| D-10 | **Pohon = `NSOutlineView` view-based** dengan `NSTableCellView` berisi `NSImageView` dan `NSTextField`, bukan hosting SwiftUI per baris. Tombol disclosure kustom menggambar chevron 8 pt yang sama. Pilihan tetap di `model.selectedNodeID`, ekspansi tetap di `node.expanded`; ekspansi sementara karena filter dipegang terpisah dan dibatalkan saat filter kosong. | Baris AppKit asli memberi `AXOutlineRow`, level, `Disclosing`, type-select, dan ←/→ tanpa kode. Hosting SwiftUI per baris mahal dan semantik AX-nya lebih lemah. |
| D-11 | **Truncate dan Drop bertanya juga di `full`**, memakai `RunConfirmationSheet` yang sama dan kata kerja bernama ("Drop Table", "Truncate Table"). `RunConfirmation.destructiveRequest` mengembalikan permintaan untuk `confirm` **dan** `full` (catatan berbeda), dan satu tes berubah. **Ditandai untuk pemilik:** ini mengubah kontrak yang tertulis di doc `requestTableOperation` dan diuji; membatalkannya satu baris. | Dua klik pada menu bertitik tiga menjatuhkan tabel produksi tanpa pertanyaan di `full`. DBeaver selalu bertanya. |
| D-12 | **Tag lingkungan `ConnectionEnvironment?` (`dev`, `staging`, `prod`)** didekode `decodeIfPresent`, hanya presentasi, tidak pernah mengubah Safe Mode. Tampil sebagai **glyph plus teks** di chip tab, breadcrumb, dan status bar. Teks chip 11 pt, bukan 10 (ia informasi, bukan dekorasi). | FR-CON-06, FR-SAFE-01. Warna saja gagal kontras di kanvas terang (§1.8). |
| D-13 | **Banner galat berdiri di atas badan kosong**, bukan di atas hasil sebelumnya. Teks bisa diseleksi, tombol Copy, Dismiss, dan Show in Log. | §1.7. Mempertahankan hasil lama berarti menahan store ketiga per tab (Fase 6 §18 menetapkan paling banyak dua). Dilaporkan sebagai penyimpangan dari kalimat FR-RUN-06. |
| D-14 | **Lembar konfirmasi:** Esc membatalkan (`.cancelAction`), **tanpa** `.defaultAction`, fokus awal di Cancel, tombol setuju memakai kata kerja bernama dan gaya destruktif coral. `RunConfirmation.Request` mendapat `confirmTitle` dan `isDestructive`. | FR-RUN-07. Ketukan Return yang masih dalam perjalanan ke editor tidak boleh menyetujui tulis. |
| D-15 | **Open Quickly tetap overlay** (bukan `NSPanel`), dengan `ScrollViewReader`, kontrol lingkup All/Objects/Saved/History, daftar AX dengan baris terpilih, dan pemulihan fokus ke wilayah sebelumnya saat ditutup. Lingkup disimpan di `NavigationState`, bukan di inti. | FR-UI-09, D-2. `NSPanel` memutus `focusedValue` dan pengujian lewat `NSHostingView`. |
| D-16 | **Setelan ukuran font disimpan di W9-T7 dan dikonsumsi di tugas yang sama:** `EditorPreferences.fontSize` dibaca `SQLEditor.swift`, `DataPreferences.gridFontSize` dibaca `GridMetrics`/`GridRowView`. Ini menambah `SQLEditor.swift`, `GridMetrics.swift`, dan `GridRowView.swift` ke berkas W9-T7 dan menyisipkan W9-T7 di rantai §7. ⌘+/⌘−/⌘0 tetap W10-T7. | Setelan di Settings yang tidak mengubah apa pun adalah yang dilarang komentar `SettingsView.swift` ("A pane that pretends to configure something the app cannot do"). Nilai bawaan harus menghasilkan piksel yang sama. |
| D-17 | **Shell = SwiftUI-native:** `NavigationSplitView` dua kolom dan `.toolbar`, harness memakai `sceneBridgingOptions = [.toolbars, .title]` dan menangkap bingkai jendela. `TabStrip` dan `StatusBar` tetap SwiftUI di kolom detail. Tiga probe (§11.4) menentukan apakah jalan ini cukup; bila tidak, shell AppKit (`NSSplitViewController` dan `NSToolbar`) adalah **re-plan** oleh `planner`, bukan perluasan T8. | Diff terkecil yang memenuhi FR-UI-08, dan API-nya terverifikasi ada di deployment target. T8 dikerjakan sonnet (O-17), jadi jalan yang memerlukan mesin jendela baru bukan jalan bawaan. |
| D-18 | **Lantai 11 pt hanya untuk teks.** Glyph SF Symbol dikecualikan dan wajib bernama (D-9). Label huruf besar dekoratif boleh 10 pt dengan ink ≥ 0,68. 10,5 menjadi 11. Berkas yang digambar grid dan editor (`ResultGrid.swift`, `GridMetrics.swift`, `GridRowView.swift`, dua `NSFont` 10,5 di `SQLEditor.swift`) **tidak** disentuh T9 dan dikerjakan W10-T2, W10-T3, dan W10-T7 di bawah V-9 dan V-10. | Footer dan toolbar `ResultGrid` ikut tertangkap 30 baseline grid. Menyentuhnya di W9 lalu merekam ulang lagi di W10 membayar dua siklus rekam ulang yang masing-masing butuh persetujuan UX dan AX. |
| D-19 | **W9-T0b (baru): baseline chrome.** `ChromeParityTests` memakai pembanding lapis piksel dan ukuran gambar yang sudah ada, atas scene chrome yang stabil (§15.1), dua appearance. Direkam sebelum T1 di HEAD dan hanya merekam **berkas baru**, jadi tidak mengubah baseline yang ada. | §1.4. Tanpa ini V-2 sampai V-7 tidak punya apa yang dibandingkan, dan T0 (pemindahan murni) tidak punya bukti piksel. Bila ditolak, G-VIS di W9 hanya berarti "grid dan editor tidak berubah" dan V-2…V-7 hanya terjaga oleh pasangan berdampingan W14-T3 dan tinjauan pemilik. |
| D-20 | **Konsumen W10 disiapkan di W9:** `FocusRegion` dan `FocusRegistry` (D-2), `Announcer`, token D-7 dan D-8, `AppMenu`, dan kunci W10 yang dicadangkan di tabel platform. | W10 dibangun di atas komponen W9 (`development-plan.md` §3). |

## 3. W9-T0: pecahan `AppModel` (perpindahan murni)

Ukuran M. Implementer GP-s, review SR. Commit: `refactor(app): AppModel split into one extension per domain, with no change in behaviour`. Dikerjakan **sesudah** W6-T1 mendarat (rantai §7: W6-T1 → W9-T0), jadi tabel di bawah dibaca per nama, bukan per baris.

### 3.1 Aturan

1. Tiap anggota dipindah utuh, bersama doc comment-nya, dengan urutan relatif yang sama di berkas barunya. Tidak ada penggantian nama, penyederhanaan, atau perubahan urutan di `init`.
2. **Properti tersimpan instans tinggal di `AppModel.swift`**, dikelompokkan di bawah `// MARK:` per domain dengan doc comment aslinya. `static let` dan `static var` ikut domain mereka (extension boleh memuatnya).
3. Anggota `private` yang dipakai dari berkas lain menjadi internal (§3.3). Tidak ada `fileprivate`. Anggota `private` yang hanya dipakai di berkas barunya tetap `private`.
4. Tiap berkas memulai dengan impor yang dipakainya (`AppKit`, `SwiftUI`, `UniformTypeIdentifiers`). Kompiler menentukan sisanya.
5. `EngineLaunchError` dan `PendingConfirmation` tetap di `AppModel.swift`.
6. Berkas diberi nama `AppModel+<Domain>.swift` di `app/Sources/QueryHive/Models/`.

### 3.2 Pemetaan per anggota

Ukuran adalah perkiraan (±10%), tanpa properti tersimpan yang pindah ke inti. Baris adalah awal chunk di `8103478`.

| Berkas | Anggota (baris awal) | ≈ baris |
|---|---|---|
| `AppModel.swift` (inti) | `flushEditors` (11), `flushEditorsNow` (20), `sqlContentTypes` (24); **semua properti tersimpan instans**: `connections`, `groups`, `tree`, `selectedNodeID`, `treeFilter`, `openQuickly*`, `shortcutScheme`, `filterPresets`, `tabs`, `selectedTabID`, `persistsSession`, `sessionReady`, `terminationObserver`, `sidebarWidth`, `panelHeight`, `panelCollapsed`, `panelExpanded`, `editingConnection`, `notice`, `pendingConfirmation`, `importDraft`, `tabCounter`, `historySearch`, `historyRead`, `historyEntries`, `savedQueries`, `account`, `profiles`, `accountNotice`, `signingIn`, `historyError`, `savedError`, `parameterPrompt`, `restoreTabsOnLaunch`, `newConnectionGroup`, `targetPopoverOpen`, `exportSettingsOpen`, `filterPopoverColumn`, `pendingDeletion`, `groupNaming`, `catalogOptions`, `schemaOptions`, `loadingOptions`, `completion`, `searchTasks`, `historyLimit`, `recordsHistory`, `statementTimeoutMS`, `engine`, `defaultRowLimit`, `rowLimitClampNote`; `shortcut(for:)` (60); `init` (175), `deinit` (193); `selectedTab` (199), `selectedConnection` (203), `database(for:)` (208), `schema(for:)` (216), `connection(for:)` (293); dua struct pendamping | 520 |
| `+Tree` | `selectedNode` (310); `openTable`, `openObjects`, `loadObjects`, `clearObjectSelection`, `selectObject`, `openObject`, `insertObject`, `objectNode` (382-619); `allNodes` (1390); `revealNode`, `rebuildTree`, `connectionNode`, `toggleExpansion`, `expand`, `refresh`, `reloadTree`, `loadChildren`, `insert` (1434-1642); `loadCatalogs`, `loadSchemas`, `optionKey`, `isLoadingOptions`, `targetChoices`, `loadedNames`, `prepareTableDestination` (1654-1761); `requestTableOperation`, `runTableOperation`, `parent(of:)` (2014-2083) | 600 |
| `+Run` | `confirmParameters`, `cancelParameters` (759-776); `runBlockedReason` (dua), `runSelectedTab`, `previewPaintInterval`, `preview`, `statements(in:)`, `awaitConfirmation`, `destinationStatements` (1908-2012); `searchOnServer`, `searchFailureMessage`, `sortOnServer`, `toggleSort`, `setSort`, `clearSort`, `rerunBaseSQL`, `scheduleServerSearch`, `fireServerSearch`, `clearSearch` (2235-2427, tanpa `stagedEditsMessage` dan `serverActionBlockedByEdits`); `runPreview`, `applyPreviewDone` (2429-2566); `productRowLimitCeiling`, `rowLimitCeiling`, `rowLimitRange`, `clampedRowLimit`, `rowLimitClampMessage` (2689-2706); `cancelPreview`, `explain`, `countRows`, `applyCountEvent`, `previewEnvironment`, `stopSelectedTab`, `stop` (2925-3105); `overrides(for:connection:sql:confirmed:)` (3388) | 560 |
| `+Connections` | `statusConnection`, `statusConnectionTarget`, `connectionID(fromNodeID:)`, `connectionState` (315-380); `benchPassword`, `storedPassword` (621-628); `presentConnectionEditor`, `requestDelete`, `presentNewGroup`, `presentRenameGroup`, `commitGroupNaming`, `deleteGroup`, `move`, `persistConnections`, `deleteConnection`, `duplicateConnection` (946-1120); `presentNavicatImport`, `importNavicatConnections`, `noticeTitle`, `importSummary` (1122-1320); `setColor`, `toggleShowAllSchemas`, `toggleShowAllDatabases`, `toggleBrowseFlag` (1322-1372); `connectionName(id:)` (1386); tiga `connectionEnvironment` (3306-3379) | 500 |
| `+Export` | `rememberDestination` (829); `runSQLFile` (1374); `presentImport` (tiga), `configure`, `loadImportColumns`, `runImport` (2085-2229); `run(_:from:confirmed:substituting:)` (3107); `handle(_:in:)` (3274) | 300 |
| `+Session` | `storedRestoreTabsOnLaunch` (808), `restoreTabsKey` (806); `restoreSession`, `saveSession`, `saveSessionBlocking`, `sessionSaveEnvironment`, `localEnvironment`, `observeTermination` (834-944); `loadAccount`, `loadProfiles`, `signInWithGoogle`, `signIn`, `signOut`, `deleteProfile` (2772-2871) | 210 |
| `+History` | `favouriteQueries`, `toggleFavourite` (256-278); `connectionName(for:)` (298); `clearHistory` (778); `recordHistory` (2568); `loadHistory`, `loadSavedQueries` (2708-2770); `saveQuery`, `deleteSavedQuery`, `loadIntoEditor`, `lastLine` (2873-2923) | 160 |
| `+Focus` | `newTab`, `warmUp`, `panelHasContent`, `syncPanelToSelectedTab`, `closeTab`, `closeSelectedTab`, `closeOtherTabs`, `closeTabs`, `closeAllTabs` (630-754); `selectTab` (815); `quickResults`, `openQuickly`, `moveQuickHighlight`, `applyQuickResult` (1404-1432). W9-T2 menambah navigasi dan fokus (§5.5) | 160 |
| `+Edit` | `presetIdentity`, `filterPresets(for:)`, `saveFilterPreset`, `deleteFilterPreset` (73-114); `stagedEditsMessage`, `serverActionBlockedByEdits` (2333-2339); `applyChanges` (3220) | 100 |
| `+Completion` | `suggestions(for:prefix:path:)` (1769), `node(atPath:connectionID:)` (1852) | 140 |

### 3.3 Anggota `private` yang menjadi internal

Sembilan properti tersimpan atau statik yang dipakai extension di berkas lain:

`flushEditorsNow` (statik; dipakai `+Session`, `+Run`, `+History`, `+Export`), `sessionReady`, `terminationObserver`, `tabCounter` (`+Focus` dan `+Session`), `historyRead`, `catalogOptions`, `schemaOptions`, `loadingOptions` (hari ini `private` dengan setter terbatas, dan `+Tree` menulisnya, jadi menjadi `var` internal biasa), `searchTasks`.

Sembilan fungsi yang menyeberang berkas:

`connectionID(fromNodeID:)` (dipakai di enam extension), `restoreSession` dan `observeTermination` (dipanggil `init` di inti), `statements(in:)` (`+Run` dan `+Edit`), `awaitConfirmation` (`+Run`, `+Tree`, `+Export`), `destinationStatements` (`+Run` dan `+Export`), `recordHistory` (statik; `+History` dan `+Run`), `connectionEnvironment(_:)` (privat; dipakai lima extension), `overrides(for:connection:sql:confirmed:)` (`+Run` dan `+Export`).

Yang **tetap** `private` karena semua pemakainya ada di berkas yang sama: `objectNode`, `warmUp`, `sessionSaveEnvironment`, `noticeTitle`, `importSummary`, `toggleBrowseFlag`, `optionKey`, `runTableOperation`, `presentImport(from:into:)`, `searchFailureMessage`, `runPreview`, `previewEnvironment`, `lastLine`, `handle`.

### 3.4 Bukti bahwa ini perpindahan murni

1. `swift build` dan `swift test` dengan hitungan tes **sama** dengan sebelum commit (jumlah dicatat di pesan commit, bukan angka tetap di dokumen ini, karena 6b W6-T1 mengubahnya).
2. Skrip perbandingan sekali pakai di `target/run/` (tidak di-commit): ekstrak teks tiap anggota tingkat atas sebelum dan sesudah, normalkan spasi, urutkan, dan bandingkan. Perbedaan yang diizinkan hanya 18 kata kunci `private` di §3.3. Hasil (daftar perbedaan) ditempel di laporan tugas.
3. `git diff --stat` menunjukkan baris dihapus ≈ baris ditambah, dikurangi header impor.
4. G-VIS tidak berubah (32+16 baseline), dan G-APP (`--snapshot --scene done`) merender.
5. Tidak ada tes yang diedit. Bila satu gagal karena akses, itu tanda satu anggota di §3.3 terlewat, bukan alasan mengubah tes.

### 3.5 Risiko

| Risiko | Mitigasi |
|---|---|
| Urutan pemindahan bertabrakan dengan 6b W6-T1 yang mengubah `runPreview`, `explain`, `closeTab`, `selectTab`, `clearSort`, `clearSearch`. | Rantai kepemilikan §7 sudah W6-T1 → W9-T0. T0 mulai dari kepala branch sesudah 6b dan 6c. |
| `@Observable` dan properti dengan setter terbatas. | Hanya `loadingOptions`; menjadi `var` internal tanpa pembatas. |
| Nama berkas extension baru dan `Package.swift`. | SwiftPM mengambil semua berkas di direktori target; tidak ada daftar berkas yang perlu diedit. |
| PRD menyebut `AppModel.swift:1248` (Navicat). | Dicatat di W9-D. Bukan blocker. |

## 4. W9-T1: lantai aksesibilitas chrome dan salinan basi

Ukuran M. Berkas: `Support/Theme.swift`, `Views/Workspace.swift` (daftar plan), ditambah `Support/ThemeStore.swift`, `Views/Panels.swift` (hanya `PanelTabButton`), `Support/Accessibility.swift` (baru), `Support/Snapshot.swift` (tiga flag `--reduce-motion`, `--reduce-transparency`, `--increase-contrast`, §4.1; rantai di §16.2), dan tes. Review SR, AX, UX. Commit: `feat(a11y): tabs are buttons, icons have names, and motion and glass follow the system settings`.

### 4.1 Satu sumber untuk Reduce Motion, Reduce Transparency, dan Increase Contrast (D-7)

`ThemeStore` (sudah `@Observable`, sudah menyimpan `systemIsDark`) mendapat:

```swift
struct SurfacePolicy: Equatable {
    var reduceMotion: Bool
    var reduceTransparency: Bool
    var increaseContrast: Bool
    var enhanced: Bool { reduceTransparency || increaseContrast }   // isian solid, hairline naik, secondary naik, glow mati
}
var surface: SurfacePolicy { get }                  // dibaca NSWorkspace; diperbarui oleh notifikasi
func pin(reduceMotion: Bool?, reduceTransparency: Bool?, increaseContrast: Bool?)   // snapshot dan tes
```

Sumbernya `NSWorkspace.shared.accessibilityDisplayShouldReduceMotion`, `...ShouldReduceTransparency`, dan `...ShouldIncreaseContrast`. `ThemeStore.init` mendaftar `NSWorkspace.accessibilityDisplayOptionsDidChangeNotification` di pusat notifikasi `NSWorkspace`. Sinyal `--snapshot` mendapat `--reduce-motion`, `--reduce-transparency`, `--increase-contrast` yang masuk lewat `pin`, supaya review tidak mengubah setelan sistem pengguna.

Token dinamis (dibaca dalam `body`, jadi Observation menggambar ulang pembacanya seperti `canvas` dan `accent` hari ini):

| Token | Normal | `enhanced` | Menggantikan |
|---|---|---|---|
| `Tone.secondary` | ink 0,68 | ink 0,85 | nilai yang sama, kini dari store (140 pembaca tidak perlu diedit) |
| `Tone.readout` | ink 0,55 | ink 0,75 | |
| `Tone.hairline` | ink 0,07 | ink 0,20 | 27 literal `Tone.ink.opacity(0.07)` |
| `Tone.outline` | ink 0,10 | ink 0,30 | literal 0,08 / 0,10 / 0,12 pada tepi kontrol |
| `Tone.focusRing` | yang paling kontras dari {glow, deep} terhadap kanvas; jatuh ke ink bila keduanya < 3:1 | sama, 0,5 pt lebih tebal | baru (D-8) |
| `Tone.markAmber`, `markMint`, `markCoral` | warna kategorikal di appearance gelap; `9A5B00`, `0B7D5E`, `D6283A` di terang | sama | baru (D-8); dipakai W10 |

Literal lama tetap berfungsi dengan nilai yang sama pada kontras normal. Tiap tugas W9 memindahkan literal di berkas **miliknya** ke token saat ia menyentuh berkas itu, dan W9-T9 (serial, menyentuh semua view) menyapu sisanya. Gate W9 memuat tes pemindai yang gagal bila `Tone.ink.opacity(0.07)` masih ada di luar daftar izin berkas milik W10 (§15).

Kebijakan permukaan:

- `glass()` dan latar sidebar (`.thinMaterial`) lewat satu modifier `.surface(radius:tint:)`: bila `enhanced`, isian solid `Tone.canvas` ditambah `Tone.recess.opacity(0.30)`, tanpa material dan tanpa gradien cahaya atas; tepi memakai `Tone.hairline`.
- `Backdrop`: tanpa dua gradien radial bila `enhanced`; tanpa `.animation(…0.8)` bila `reduceMotion`.
- `HiveHero`: `bob` tidak dinyalakan bila `reduceMotion` (kondisi di `onAppear`, bukan modifier `.animation` yang tetap melekat).
- `PressScale` dan animasi hover `HubButton`: durasi 0 bila `reduceMotion`. Transisi hover pendek (≤ 0,2 dtk) selain itu dipertahankan.

### 4.2 Nama untuk tombol ikon dan chip

```swift
IconButton(symbol:, label:, help: = "", tint:, diameter:, action:)     // accessibilityLabel(label); help bawaan = label
```

`label` wajib. Inisialisasi lama (hanya `help:`) bertahan sementara dengan `label = help` dan ditandai komentar "migrasi: milik W9-T3/W9-T5/W10-T3". `Chip(text:tint:kind:)` mendapat `kind` opsional untuk bacaan "Type: varchar". Ikon yang berdampingan dengan teks bernama memakai `.accessibilityHidden(true)`.

### 4.3 Chip tab dan tab panel

```swift
// pure, diuji tanpa pohon AX
struct TabChipSpec: Equatable {
    let label: String                 // tab.title
    let value: String?                // "running" | "finished" | "failed" | nil untuk idle
    let isSelected: Bool
    let glyph: StageGlyph             // none | spinner | checkmark | exclamation
    let actions: [String]             // ["Close"]
}
```

Struktur tampilan: grup berisi dua `Button` bersaudara, bukan satu `Button` yang membungkus `Button` lain:

1. tombol pilih (glyph tahap, judul, lencana lingkungan dan Safe Mode dari T4) dengan `accessibilityLabel`, `accessibilityValue`, dan trait `.isSelected` bila terpilih;
2. tombol tutup (`xmark`) yang terlihat saat hover atau terpilih. Bila tidak terlihat ia `.accessibilityHidden(true)`, dan penutupan untuk VoiceOver ada sebagai aksi kustom "Close" pada tombol pilih.

Glyph tahap menggantikan titik 6 pt yang hanya beda warna: `running` = spinner mini (diam sebagai `circle.dotted` bila `reduceMotion`), `done` = `checkmark` bertinta `markMint`, `failed` = `exclamationmark.triangle.fill` bertinta `markCoral`, `idle` = tanpa glyph. Ukuran glyph 9 pt (glyph dikecualikan dari lantai teks, D-18).

`PanelTabButton` mendapat `.accessibilityAddTraits(.isSelected)` saat terpilih, `accessibilityLabel` = nama panel, dan `accessibilityValue` = hitungan bila ada.

### 4.4 Salinan basi

| Tempat | Sekarang | Menjadi |
|---|---|---|
| `EmptyWorkspace` (`Workspace.swift:65`) | "Open a query tab, write SQL, and Run streams the result straight to disk." | "Open a query tab, write SQL, and press Run to see the rows. Export writes a file." |
| Placeholder editor (`Workspace.swift:366`) | `SELECT * FROM hive.analytics.penerima_manfaat` | menurut dialek tab: Trino `SELECT * FROM catalog.schema.table`, PostgreSQL `SELECT * FROM schema.table`, MySQL `SELECT * FROM database.table`, tanpa koneksi `SELECT * FROM table` |
| Bantuan tab baru (`Workspace.swift:99`) | "New Query (⌘T)" | "New Query (<kunci skema aktif>)" lewat `model.keyHint(for: .newQuery)` |
| Bantuan Run (`Workspace.swift:215`) | "Run the query and show the rows (⌘R)" | sama, kunci dari skema |
| Kalimat kosong sidebar (`SidebarTree.swift:128`) | "Add a Trino coordinator to browse its catalogs." | "Add a connection to browse its catalogs, schemas and tables." (ditulis T3 di berkas pengganti) |

Placeholder dan kalimat kosong adalah string tampil, jadi semuanya masuk katalog string di W13-T7 tanpa perubahan struktur.

### 4.5 `Announcer`

`Support/Accessibility.swift`: `enum Announcer` dengan `static func post(_ message: String, priority: NSAccessibilityPriorityLevel = .medium)` dan `static var last: String?` (diisi di dalam, hanya dibaca dari luar). Ia memposting `NSAccessibility.post(element: NSApp, notification: .announcementRequested, userInfo: [.announcement: message, .priority: priority.rawValue])` hanya bila `NSWorkspace.shared.isVoiceOverEnabled` (jaring terhadap kerja sia-sia), dan menyimpan `last` supaya tes bisa membacanya (pola yang sama dengan `Coordinator.lastAXAnnouncement` grid). Pemakai W9: pindah tab, panel, sidebar, banner galat, fokus tanpa hasil. Pemakai W10 dan W12: seleksi, tinjauan perubahan, Run selesai.

### 4.6 Tes

- `AccessibilityLabelTests`: (a) pemindai sumber: tiap `IconButton(` memberi `label:` kecuali daftar izin per berkas milik tugas yang belum jalan (`SidebarTree.swift` sampai T3, `ResultGrid.swift` sampai T5 dan W10-T3), dan daftar itu **kosong** di gate W9 untuk berkas yang dimiliki W9; (b) probe P-1a: jalan pohon AX dari tampilan yang di-host untuk scene `done`, `empty`, dan `fresh-tab`, dan setiap elemen berperan tombol punya label tak kosong.
- `TabChipSpecTests`: label, nilai, trait, aksi, dan glyph untuk empat tahap, terpilih dan tidak.
- `SurfacePolicyTests`: `pin` mengubah `Tone.secondary`, `hairline`, dan kebijakan `glass()`; notifikasi `NSWorkspace` mengubah `surface`.
- `ChromeContrastTests`: untuk tujuh kanvas dan tiap aksen: `Tone.secondary` ≥ 4,5, `Tone.readout` ≥ 4,5, `Tone.hairline` (`enhanced`) ≥ 3, tiga `Tone.mark*` ≥ 3 di appearance masing-masing, `Tone.focusRing` ≥ 3. Fungsi kontras dipindah dari `SyntaxPaletteTests` ke berkas pendukung bersama.
- `EmptyCopyTests`: lima string di §4.4.

## 5. W9-T2: menu, pintasan, dan model fokus

Ukuran S sampai M. Berkas: `App.swift`, `Models/Shortcuts.swift`, `Models/AppModel+Focus.swift` (daftar plan), ditambah `Models/AppMenu.swift` (baru), `Views/RootView.swift` (satu baris, §5.6), dan `Models/AppModel.swift` (satu baris, `var navigation`, §5.6; rantai di §16.2). Review SR, AX, UX. Commit: `feat(app): a View menu, focus moves and tab keys, and no key bound twice`.

### 5.1 Bentuk

```swift
// Models/Shortcuts.swift
enum ShortcutAction: String, CaseIterable, Identifiable {
    // yang ada: run, runScript, explain, countRows, stop, newQuery, openFile, saveFile,
    //           exportData, toggleResultPanel, commentLine, format, closeTab, openQuickly
    case revealOutput, toggleSidebar, focusSidebar, focusEditor, focusResults, nextTab, previousTab
    // dicadangkan untuk W10 (belum punya pemakai, jadi isAvailable == false):
    case peekCell, toggleRecord, addRow, deleteRow, fontBigger, fontSmaller, fontReset

    enum Scope { case scheme, platform }
    var scope: Scope { get }               // stop, saveFile, closeTab, openQuickly, dan semua yang baru: .platform
    var isAvailable: Bool { get }          // false untuk commentLine, format (sampai W12-T2) dan kunci W10
}

enum ShortcutScheme { /* dbeaver, queryhive, seperti sekarang */
    func shortcut(for action: ShortcutAction) -> Shortcut?     // scheme table, lalu platformTable
    static let platformTable: [ShortcutAction: Shortcut]       // sama untuk kedua skema
}

// Models/AppMenu.swift
struct MenuSpec: Equatable { let id: String; let title: String; let shortcut: Shortcut?; let group: MenuGroup }
enum AppMenu {
    static func specs(for scheme: ShortcutScheme) -> [MenuSpec]       // menu, termasuk ⌘1...⌘9
    static func perform(_ spec: MenuSpec, in model: AppModel)         // satu switch atas id; satu-satunya tempat aksi menu dipetakan
    static func isEnabled(_ spec: MenuSpec, in model: AppModel) -> Bool
}
```

`App.swift` merender `AppMenu.specs` ke `CommandGroup` dan `CommandMenu` secara generik (judul, kunci, `perform`, `isEnabled`), sehingga tugas berikutnya (W10-T1, T3, T5, T7) menambah aksi menu hanya di `AppMenu.swift` dan `Shortcuts.swift`, tanpa menyentuh `App.swift`. Tombol di view (Run di toolbar, Stop, tombol Tab Baru) membaca `model.shortcut(for:)` seperti sekarang, dan teks bantuannya ikut skema: `help: "Run (\(model.shortcutScheme.shortcut(for: .run)?.display ?? "no key"))"` menggantikan literal "(⌘R)" dan "(⌘T)" di `Workspace.swift:99` dan `:215` (kedua baris dimiliki W9-T1; T2 hanya menyediakan helper `model.keyHint(for:)`).

### 5.2 Tabel kunci final

Skema DBeaver tidak berubah untuk delapan aksi yang sudah dibaca dari sumbernya. Kunci tambahan ada di tabel platform yang berlaku di **kedua** skema, dan Settings menulis kalimat itu terang-terangan di bawah tabel: "Where DBeaver defines no key, QueryHive uses the macOS convention."

| Aksi | DBeaver | QueryHive | Kelas | Catatan |
|---|---|---|---|---|
| `run` | ⌘↩ | ⌘R | skema | |
| `runScript` | ⌥X | tidak terikat | skema | QueryHive tidak pernah mengikatnya |
| `explain` | ⌃⇧E | ⌘E | skema | |
| `countRows` | tidak terikat | ⇧⌘K | skema | pemakai baru: item "Count Rows" di menu Query |
| `exportData` | tidak terikat | **⇧⌘E** (dulu ⌘E) | skema | D-5 |
| `newQuery` | ⌃] | ⌘T | skema | |
| `openFile` | ⌃⌥⇧O | ⌘O | skema | |
| `toggleResultPanel` | ⌃T | **⇧⌘Y** | skema | pemakai baru: menu View. ⇧⌘Y adalah ikatan Xcode untuk area bawah |
| `commentLine` | ⌃/ | ⌘/ | skema | disembunyikan di Settings sampai W12-T2 |
| `format` | ⌃⇧F | tidak terikat | skema | disembunyikan sampai W12-T2. Kunci QueryHive diputuskan di sana; ⌥⌘F (ganti di editor) dan ⌃⌘F (layar penuh) dilarang |
| `stop` | ⌘. | ⌘. | platform | skema DBeaver tidak mendeklarasikan kunci untuk `cancel.query`; ⌘. adalah konvensi macOS (UC-07) |
| `saveFile` | ⌘S | ⌘S | platform | pemakai: W10-T3 (tinjau perubahan di grid), W12-T2 (berkas) |
| `closeTab` | ⌘W | ⌘W | platform | tanpa ini ⌘W menutup jendela dan app berhenti (§1.2) |
| `openQuickly` | ⇧⌘O | ⇧⌘O | platform | |
| `revealOutput` | ⇧⌘R | ⇧⌘R | platform | dulu literal di `Panels.swift:334` |
| `toggleSidebar` | ⌃⌘S | ⌃⌘S | platform | konvensi sistem |
| `focusSidebar` / `focusEditor` / `focusResults` | ⌥⌘1 / ⌥⌘2 / ⌥⌘3 | sama | platform | urutan spasial kiri, tengah, bawah. ⌘1…9 sudah milik tab |
| `nextTab` / `previousTab` | ⇧⌘] / ⇧⌘[ | sama | platform | alias ⌃Tab dan ⌃⇧Tab (§5.3) |
| "Go to Tab" 1…9 | ⌘1…⌘9 | sama | platform | ⌘9 = tab terakhir. Bukan `ShortcutAction`; hanya ada di `AppMenu` |
| `peekCell` | ⌘Y | sama | platform | dicadangkan, W10-T1 |
| `toggleRecord` | ⌥⌘I | sama | platform | dicadangkan, W10-T5 |
| `addRow` / `deleteRow` | ⌥⌘N / ⌘⌫ | sama | platform | dicadangkan, W10-T3 |
| `fontBigger` / `fontSmaller` / `fontReset` | ⌘+ / ⌘− / ⌘0 | sama | platform | dicadangkan, W10-T7 |

Kunci lokal, di luar tabel tetapi masuk gabungan uji: editor (⌘F, ⌥⌘F, ⌘G, ⇧⌘G, ⌥⌘[ , ⌥⌘], Esc, ⌃Spasi, ⌥Esc, dan panah, Return, Tab, Esc saat saran aktif), Open Quickly (↑, ↓, Return, Esc), lembar koneksi (⌘T Test sampai W11-T3 memindahkannya ke ⌘↩, Esc, Return), dan grid (peta kunci W10-T1, blueprint W10 §3).

Daftar **kunci sistem** yang tidak boleh dipakai aksi mana pun: ⌘Q, ⌘H, ⌥⌘H, ⌘M, ⌘,, ⌘N, ⌘P, ⌘Z, ⇧⌘Z, ⌘X, ⌘C, ⌘V, ⌘A, ⌘`, ⌘Tab, ⌘Spasi, ⌃⌘Spasi, ⌃⌘F, ⇧⌘3, ⇧⌘4, ⇧⌘5. ⌘E bukan "kunci sistem" di dalam app ini karena skema QueryHive sudah memakainya untuk Explain; itu dicatat sebagai keputusan yang diwarisi, bukan dibela.

### 5.3 Alias tab dan jalur kunci yang tidak lewat menu

SwiftUI memberi satu kunci per item menu. Item "Show Next Tab" membawa ⇧⌘], dan ⌃Tab serta ⌃⇧Tab ditangani oleh monitor lokal `NSEvent` (`keyDown`) yang dipasang `AppDelegate` bila jendela utama adalah jendela kunci dan tidak ada sheet. Logikanya murni dan bisa diuji tanpa `NSEvent`:

```swift
enum TabKeyRouter { static func route(keyCode: UInt16, control: Bool, shift: Bool, command: Bool, option: Bool) -> TabKeyAction? }
```

### 5.4 `ShortcutConflictTests` (baru)

1. `testNoKeyIsBoundTwiceInTheDbeaverScheme` dan `…InTheQueryhiveScheme`: gabungan `AppMenu.specs(for:)`, kunci lokal, dan `TabKeyRouter` tidak punya pasangan (tombol, pengubah) kembar. Pengecualian tercatat lewat satu daftar `knownConflicts` dengan komentar tugas yang menutupnya (hari ini hanya ⌘T di lembar koneksi, ditutup W11-T3).
2. `testThePlatformTableCollidesWithNeitherScheme`.
3. `testNoActionUsesASystemKey` atas daftar §5.2.
4. `testExportAndExplainDifferInTheQueryhiveScheme` (⌘E dan ⇧⌘E).
5. `testEveryAvailableActionHasAMenuItem` dan `testEveryMenuItemWithAKeyIsAnAction`.
6. `testNoViewHardCodesAKeyboardShortcut`: pemindai sumber (`app/Sources`) untuk `.keyboardShortcut("` dengan literal di luar daftar izin per berkas (lembar koneksi, `RunConfirmationSheet`).
7. `testTabKeyRouterMapsControlTabAndShiftControlTab` dan `testItIgnoresTabInATextField`.
8. `testTheKeyboardSettingsTableListsOnlyAvailableActions`.

### 5.5 Navigasi dan fokus (`+Focus`)

```swift
enum FocusRegion: String { case sidebar, editor, results }

struct NavigationState: Equatable {          // satu properti tersimpan di inti: var navigation = NavigationState()
    var sidebarHidden = false                // sampai T8 menggantinya dengan toggle native
    var quicklyScope = QuickScope.all        // diisi T6
}

extension AppModel {
    func focus(_ region: FocusRegion)        // makeFirstResponder pada tampilan di jendela kunci
    var currentRegion: FocusRegion?          // dihitung saat ditanya dari first responder; tidak teramati
    func toggleSidebar()
    func selectTab(offset: Int)              // melingkar
    func selectTab(position: Int)            // ⌘1…⌘9; 9 = terakhir
    func toggleResultPanel()
}
```

- **Mencari tampilan tanpa menyentuh berkas pemilik.** `focus(_:)` mencari keturunan jendela berdasarkan tipe, seperti `Snapshot.findTextView`: `SQLTextView` (editor), `GridTableView` (hasil), dan outline pohon (T3). Tiga berkas itu dimiliki tugas lain (`SQLEditor.swift` oleh rantai W4-T2 → W9-T7 → W10-T6, `GridTableView.swift` oleh W10-T1, `SchemaOutline.swift` oleh T3), jadi mendaftarkan diri ke registri berarti menyentuh mereka. Pencarian tipe tidak. `currentRegion` memakai pengelompokan yang sama pada `firstResponder` dan induknya.
- **Tanpa grid.** Bila tab tidak punya `GridTableView` (placeholder atau galat), `focus(.results)` mengumumkan "No results to focus" lewat `Announcer` dan tidak memindah fokus.
- **`toggleSidebar` di T2.** Menyetel `navigation.sidebarHidden` dan `RootView` menyembunyikan kolom pohon dan resizer. T8 mengganti pembacanya dengan toggle native `NavigationSplitView`, dan menu tidak berubah.
- **Pengumuman.** Pindah tab, tampilkan atau sembunyikan panel, dan sidebar: kalimat pendek lewat `Announcer` (§4.5).
- **Open Quickly** memulihkan wilayah sebelumnya saat ditutup (T6).
- **Siklus hidup tab** (`newTab`, `closeTab`, `selectTab`, `warmUp`, `syncPanelToSelectedTab`) pindah ke berkas ini murni di T0.

### 5.6 Berkas dan urutan

| Berkas | Perubahan |
|---|---|
| `Models/Shortcuts.swift` | aksi baru, `scope`, `isAvailable`, `platformTable`, `TabKeyRouter`, ⇧⌘E |
| `Models/AppMenu.swift` (baru) | `MenuSpec`, `AppMenu` |
| `Models/AppModel+Focus.swift` | bagian di atas |
| `Models/AppModel.swift` | satu baris: `var navigation = NavigationState()` (satu-satunya sentuhan T2 pada inti; pemilik sesudah T0, §16.2) |
| `App.swift` | menu dirender dari `AppMenu`, monitor tab, item View, Window "Go to Tab", "Count Rows", "Reveal Output" memakai `revealOutput` |
| `Views/RootView.swift` | satu kondisi pada `model.navigation.sidebarHidden` (berkas ini belum ada di §7; urutan: W9-T2 → W9-T4 → W9-T8, lihat §16) |
| `Views/SettingsView.swift` | **bukan** di T2: tabel Keyboard membaca `isAvailable` lewat `AppMenu`; perubahannya satu baris dan masuk W9-T7 |
| Tes | `ShortcutConflictTests` (baru), `AppMenuTests` (baru) |

## 6. W9-T3: pohon `NSOutlineView`

Ukuran L. Berkas (daftar plan): `Views/SchemaOutline.swift` (baru), `Views/SidebarTree.swift`, `Models/SchemaTree.swift`, `Models/AppModel+Tree.swift`; ditambah `Models/TreeMenu.swift` (baru) dan tes. Review SR, AX, UX, AR, CR. Commit: `feat(tree): the object tree becomes an outline view that the keyboard and VoiceOver can walk`. Dikerjakan paralel dengan T1 dan T2 di batch 2, jadi ia tidak menyentuh `Theme.swift` dan memakai token T1 hanya sesudah T1 mendarat (urutan di §16).

### 6.1 Pembagian SwiftUI dan AppKit

Kolom sidebar tetap satu `VStack`: header (tanda "QUERYHIVE", menu +, tombol muat ulang, kolom filter) dan `FavouritesSection` tetap SwiftUI. Daftar objek (`ScrollView` dan `LazyVStack` hari ini) diganti `SchemaOutline`, sebuah `NSViewRepresentable` yang membungkus `NSScrollView` berisi `OutlineView: NSOutlineView`. Keadaan kosong ("No connections yet", "Nothing loaded matches…") tetap SwiftUI dan menggantikan outline, seperti sekarang.

### 6.2 Data

- **Item.** `final class OutlineItem: NSObject` berkunci `id` (`TreeNode.id`, atau `"msg:<parentID>:<kind>"` untuk baris pesan). Identitas dijaga lewat kamus `id → OutlineItem`, karena `NSOutlineView` membandingkan item secara identitas. Jenis item: `.node(TreeNode)` dan `.message(Loading | Empty | Error(String))`. Baris pesan adalah **baris sungguhan** (hari ini `messageRow` di bawah `TreeRow`), supaya VoiceOver membacanya sebagai bagian pohon.
- **Snapshot.** `OutlineSnapshot.make(model:filter:)` meratakan node yang terlihat menjadi nilai (`id`, `depth`, `kind`, `title`, `symbol`, `tintKey`, `expanded`, `expandable`, `message`) dan membaca properti `@Observable` yang dipakai, sehingga Observation memicu `updateNSView`. Pola sama dengan `GridInputs`. Biayanya sebanding jumlah baris terbuka, bukan seluruh pohon.
- **Muat ulang.** Bila hanya anak satu induk yang berubah (kasus umum: `loadChildren` selesai), `reloadItem(parent, reloadChildren: true)`. Selain itu `reloadData()` lalu memulihkan ekspansi dan pilihan dari `node.expanded` dan `model.selectedNodeID`. `NSOutlineView` hanya membuat tampilan untuk baris yang terlihat, jadi 5.000 tabel terbuka tidak membangun 5.000 tampilan.
- **Ekspansi.** Sumber kebenaran tetap `node.expanded`. `outlineViewItemDidExpand/Collapse` memanggil `model.expand` atau `toggleExpansion` (yang memuat anak lazily). Filter hari ini membuka semua level tanpa mengubah `node.expanded` (`TreeRow.showChildren`). Di outline, filter membuka node lewat `expandItem` dan mencatatnya di himpunan sementara; saat filter kosong, node di himpunan itu yang `node.expanded == false` ditutup lagi.
- **Pilihan.** Satu pilihan, `model.selectedNodeID`. `outlineViewSelectionDidChange` menulisnya, dan `updateNSView` menyetel pilihan outline dari model tanpa memicu penulisan balik.

### 6.3 Paritas visual (V-5)

Yang sama dengan hari ini: tinggi baris 23 (`Metrics.treeRow`), indentasi 15 (`indentationPerLevel = 15`, bawaan AppKit 16), chevron `chevron.right` 8 pt semibold yang berputar 90° (tombol disclosure kustom lewat `makeView(withIdentifier: NSOutlineViewDisclosureButtonKey)`, kotak 11×11), ubin driver 16 pt pada baris koneksi, ikon level bertinta (catalog violet, database ice, schema amber, tabel secondary), judul 12 pt (semibold untuk koneksi), pemotongan tengah, isian pilihan `Tone.ink` 0,13 sudut 6 dan hover 0,06, tinggi baris pesan menyesuaikan teks (dua baris untuk galat).

Baris menggambar sendiri lewat `OutlineRowView: NSTableRowView` (`drawSelection(in:)` dan pelacak hover). **Yang berubah dan didaftarkan sebagai V-5:** saat outline menjadi first responder, baris terpilih menggambar cincin fokus 1,5 pt `Tone.focusRing` (inset 1 pt) di atas isian pilihan, dan isian pilihan tanpa fokus tetap seperti sekarang. `focusRingType = .none` di outline supaya cincin sistem tidak ikut tergambar.

Teks 11 pt: pesan baris pesan 10,5 menjadi 11, label "QUERYHIVE" 10,5 dan "FAVOURITES" 9,5 menjadi 10 (dekoratif huruf besar, ink ≥ 0,68), sebagai bagian T3 karena berkasnya diganti.

### 6.4 Keyboard dan type-select

| Kunci | Aksi | Sumber |
|---|---|---|
| ↑ ↓ | pindah pilihan | bawaan |
| → | buka node, atau pindah ke anak pertama bila sudah terbuka | bawaan |
| ← | tutup node, atau pindah ke induk | bawaan |
| Return | `openNode` (aksi dobel-klik: buka tabel, daftar objek schema, atau ekspansi) | `OutlineView.keyDown` |
| ⌥→ / ⌥← | buka dan tutup seluruh subpohon | bawaan |
| ketik huruf | pilih baris berikutnya yang judulnya berawalan itu | `allowsTypeSelect = true` dan `outlineView(_:typeSelectStringFor:item:)` memakai `title` |
| Tab | keluar dari outline ke kontrol berikutnya | bawaan |
| ⌥⌘1 | memfokuskan outline | T2 |

### 6.5 Menu konteks dari satu deskripsi

`Models/TreeMenu.swift` mendefinisikan `TreeMenu.items(for node: TreeNode, in model: AppModel) -> [TreeMenuItem]` (judul, status centang atau aktif, peran destruktif, tindakan, submenu). Isinya **dipindahkan apa adanya** dari `TreeRow.menu` dan `groupMenu` (judul, urutan, pemisah, kondisi "hanya PostgreSQL" untuk dua saklar). `OutlineView.menu(for:)` membangun `NSMenu` dari deskripsi itu untuk node di `clickedRow`, dan tidak mengubah pilihan (sama dengan klik kanan SwiftUI hari ini, yang tidak memindah pilihan). Apakah AppKit menyorot baris yang diklik-kanan tanpa mengubah pilihan tidak diverifikasi (§17.2). Item Truncate dan Drop memanggil `model.requestTableOperation` tanpa perubahan di T3; gaya dialognya milik T5 (D-11, §8).

### 6.6 Seret ke editor (FR-TREE-05)

`outlineView(_:pasteboardWriterForItem:)` untuk node `.table` mengembalikan `NSPasteboardItem` dengan `.string` = `TreeNode.insertableText` (nama berkualifikasi dan terkutip sesuai driver) dan tipe privat berisi `id` node. `NSTextView` menerima jatuhan teks di titik jatuh secara bawaan (probe P-3d), jadi `SQLEditor.swift` tidak berubah. Pratinjau seretan: gambar baris. Node selain tabel tidak bisa diseret.

### 6.7 Aksesibilitas

Baris view-based mendapat `AXOutlineRow`, level, `Disclosing`, dan `DisclosedRows` dari AppKit. Yang ditulis T3:

- outline: `accessibilityLabel` "Object tree", peran bawaan `AXOutline`;
- sel: `accessibilityLabel` = judul, `accessibilityValue` untuk koneksi = `ConnectionState.label` ("Connected", "Not browsed yet"), untuk lainnya jenis ("schema", "table"); `accessibilityHelp` = kalimat yang hari ini jadi `.help` ("Catalog X — expand to list schemas");
- aksi kustom pada baris: "Open" (aksi dobel-klik) dan "Refresh";
- baris pesan: teks statis; baris galat diumumkan lewat `Announcer` saat muncul;
- tinggi baris, judul, dan pemotongan tidak mengubah label (label memuat judul penuh).

### 6.8 Tes

- `SchemaOutlineTests` (baru, di `NSHostingView` dan `cacheDisplay` seperti `GridParityTests`, bukan `ImageRenderer`): pohon fixture menggambar baris; ekspansi lazy memuat anak sekali; pilihan model dan outline sinkron dua arah tanpa putaran; filter membuka dan memulihkan ekspansi; `tree-large` (30k node) hanya membangun tampilan untuk baris yang terlihat; type-select memilih berdasarkan judul; Return memanggil aksi yang sama dengan dobel-klik untuk setiap jenis node; pasteboard seretan tabel = `insertableText`; atribut AX (peran, level, `Disclosing`, nilai, aksi).
- `TreeMenuTests` (baru): judul, urutan, dan status aktif sama dengan menu lama untuk setiap jenis node, termasuk dua saklar PostgreSQL.
- `SidebarRenderTests`: tiga tes ditulis ulang ke jalur hosting. Asersi "menggambar sesuatu dan menampilkan favorit" dipertahankan.
- `TreeNodeStalenessTests` tidak berubah (model).
- Scene chrome T0b `tree-large`, `groups`, dan `tree-empty-schema` direkam ulang sebagai V-5 di commit sendiri.

### 6.9 Probe

| ID | Pertanyaan | Lulus bila | Bila gagal |
|---|---|---|---|
| P-3a | Apakah `NSOutlineView` di dalam `NSViewRepresentable` tergambar oleh `cacheDisplay` di jendela `.titled` seperti grid? | baris dan chevron terlihat di PNG | jatuh ke `bitmapImageRepForCachingDisplay` pada outline langsung |
| P-3b | Apakah tombol disclosure kustom (`NSOutlineViewDisclosureButtonKey`) menerima klik dan menjaga trait AX "disclosing"? | klik membuka, `Disclosing` benar | pakai tombol bawaan dan terima V-5 lebih besar (ditandai untuk UX) |
| P-3c | Apakah `accessibilityRows` pada outline melaporkan level dan anak seperti yang diharapkan VoiceOver? | tes AX di atas lulus | koreksi lewat override `accessibilityRows` seperti grid |
| P-3d | Apakah `NSTextView` menerima jatuhan `NSPasteboardItem` ber-`.string` dari outline tanpa kode tambahan? | teks masuk di titik jatuh | `SQLTextView` menerima tipe privat (menyentuh `SQLEditor.swift`; dilaporkan, bukan dilakukan diam-diam) |

## 7. W9-T4: tag lingkungan dan chrome Safe Mode

Ukuran M. Berkas (daftar plan): `Models/Connections.swift`, `Views/ConnectionsViews.swift` (pemilih), `Views/ContextCascade.swift`, `Views/Workspace.swift` (lencana tab), `Views/RootView.swift` (status bar); ditambah `Support/Snapshot.swift` (satu scene baru, `badges`; rantai §7 menjadi ... W6-T1 → W9-T1 → W9-T4 → W9-T7 → W9-T8, §16.2) dan tes. Review SR, UX, AX, CR. Commit: `feat(safety): the connection's environment and Safe Mode are visible wherever a query is written`.

### 7.1 Model

```swift
enum ConnectionEnvironment: String, CaseIterable, Codable, Identifiable { case dev, staging, prod }   // label: "DEV" "STAGING" "PROD"
struct Connection { var environment: ConnectionEnvironment? }   // CodingKeys += environment
```

Didekode lunak: nilai tak dikenal menjadi `nil` dan **tidak** membuat `connections.json` dipindahkan sebagai rusak (kebijakan `ConnectionStore.load` hari ini untuk berkas tak terbaca). Berkas lama tanpa kunci itu dimuat dengan `nil`. Tag tidak pernah masuk ke lingkungan engine (`connectionEnvironment` tidak membacanya), tidak mengubah Safe Mode, dan tidak masuk MCP sampai W11-T3 memutuskannya. Editor koneksi mendapat kontrol "Environment: None / Dev / Staging / Prod" di dekat Safe Mode dengan kalimat "A label only. It does not change what Safe Mode allows." `isDirty` ikut menghitungnya.

### 7.2 Lencana

Satu komponen `ConnectionBadges(connection:, style:)` dan satu `BadgeSpec` murni (glyph, label, label AX, nada) yang diuji.

| Sumber | Glyph | Label | Nada |
|---|---|---|---|
| Safe Mode `full` | `lock.open` | Full | ink |
| Safe Mode `no_ddl` | `lock.shield` | No DDL | ink |
| Safe Mode `confirm` | `shield.lefthalf.filled` (sama dengan lembar konfirmasi) | Confirm | ink |
| Safe Mode `read_only` | `lock.fill` | Read only | ink |
| Lingkungan `dev` | `hammer` | DEV | `markMint` |
| Lingkungan `staging` | `testtube.2` | STAGING | `markAmber` |
| Lingkungan `prod` | `exclamationmark.octagon.fill` | PROD | `markCoral` |

Nama SF Symbol di kolom glyph adalah usulan; implementer memastikan semuanya ada di target macOS 14 dan menggantinya bila tidak. Teks lencana `code(11, .semibold)` dengan kapsul `tint 0,14`, **bukan** 10 pt, karena ia informasi dan bukan label dekoratif (D-12, D-18). Nada hanya menguatkan; glyph dan teks yang membedakan. Tidak ada tint prod pada permukaan lain (opsional di FR-SAFE-01, tidak diambil, supaya vokabuler warna koneksi tidak bertabrakan).

Aturan tampil:

- **Status bar:** selalu, kedua lencana (termasuk `Full`), sesudah teks koneksi dan titik status. Label AX: "Safe Mode: Confirm. Reads run, writes wait for approval." (dari `ConnectionSafeMode.detail`).
- **Breadcrumb dan chip tab:** lencana tingkat terbatas (`no_ddl`, `confirm`, `read_only`) dan lingkungan bila ada. `full` tanpa lingkungan tidak menampilkan apa pun, karena tidak adanya lencana berarti tidak ada batasan. Lencana breadcrumb berdiri di antara breadcrumb dan grup Run, rata kanan, supaya tiga level berlebar tetap tidak bergeser antar-tab (PR-13). Di lebar sempit `ViewThatFits` menjatuhkan label dan menyisakan glyph, dengan label AX dan tooltip utuh. Di T8 lencana pindah ke subjudul jendela dan toolbar.
- **Chip tab:** lencana lingkungan dan Safe Mode di dalam tombol pilih (§4.3), sehingga bacaan VoiceOver tab menjadi "penerima_manfaat, running. Environment: production. Safe Mode: Confirm."

### 7.3 Tes

- `ConnectionEnvironmentTests`: berkas lama tanpa tag dimuat `nil`; putaran penuh; tag asing menjadi `nil` tanpa memindahkan berkas; `Equatable` memperhitungkan tag; tag tidak ada di `connectionEnvironment(_:)`.
- `BadgeSpecTests`: empat level × empat lingkungan (termasuk `nil`) menghasilkan glyph, label, label AX, dan aturan tampil per lokasi yang benar.
- `StatusBarBadgeTests`: status bar memuat dua lencana untuk koneksi `prod` + `confirm`.
- Scene `badges` (V-11, tanpa baseline lama): satu koneksi `prod` + `confirm` dan satu `dev` + `full`. Scene chrome T0b `cascade-*` dan `done` direkam ulang sebagai V-3 di commit sendiri.

## 8. W9-T5: banner galat inline dan lembar konfirmasi

Ukuran S. Berkas (daftar plan): `Views/ResultGrid.swift` (badan galat), `Views/RunConfirmationSheet.swift`, `Models/RunConfirmation.swift`; ditambah `Support/Accessibility.swift` (pemakai `Announcer`) dan tes. Review SR, UX, AX. Commit: `feat(app): errors you can select and copy, and a write confirmation that Return cannot approve by accident`. Catatan: di W6 `ResultGrid.content` berubah, jadi tugas ini mulai dari kepala branch sesudah 6b dan mengambil bentuknya dari sana.

### 8.1 Banner galat (FR-RUN-06, D-13)

`ErrorBanner(message:, onCopy:, onShowLog:, onDismiss:)` berdiri di atas `HStack { content; inspector }`, di bawah toolbar pencarian:

- glyph `exclamationmark.triangle.fill` (`markCoral`), judul "Query failed" 12 pt semibold, dan pesan 11,5 pt dengan `.textSelection(.enabled)`, maksimal 120 pt tinggi lalu menggulir;
- tombol `PillButton` kompak: **Copy** (menyalin seluruh pesan), **Show in Log** (`tab.panel = .log`; log sudah memuat baris galat lewat `tab.note(.error, …)`), **Dismiss** (`tab.previewError = nil`);
- di bawahnya badan kembali ke kalimat tanpa hasil yang ada ("Press Run to see the rows…"), karena Run menghapus hasil lama di awal dan W6 melepas store-nya. Kalimat FR-RUN-06 "di atas hasil sebelumnya" dengan demikian **tidak dipenuhi apa adanya**. Memenuhinya berarti menahan store ketiga per tab; itu dilaporkan sebagai penyimpangan dan menjadi pilihan pemilik bila ia menginginkannya, dengan harga memori yang ditulis di Fase 6 §18;
- `Announcer.post("Query failed: <160 karakter pertama>", priority: .high)` saat banner muncul.

`ResultGrid.content` kehilangan cabang `tab.previewError` yang menggantikan seluruh badan, dan teks sumbernya tetap `tab.previewError`. Tiga jalur lain yang menulis `previewError` sebelum run berangkat (galat peluncuran) memakai banner yang sama tanpa perubahan.

### 8.2 Lembar konfirmasi (FR-RUN-07, D-14, D-11)

- Cancel: `PillButton(role: .quiet)` dengan `.keyboardShortcut(.cancelAction)` dan fokus awal (`.defaultFocus`, macOS 14).
- Tombol setuju: `PillButton(role: .destructive)` (coral) berjudul kata kerja bernama dari `request.confirmTitle`: "Run Write", "Run 3 Writes", "Drop Table", "Truncate Table". **Tanpa** `.keyboardShortcut(.defaultAction)`, jadi Return tidak menyetujui apa pun.
- `RunConfirmation.Request` mendapat `confirmTitle: String` (bernilai bawaan, supaya inisialisasi `Request` yang ada, termasuk di tes, tetap berkompilasi). Semua permintaan bergaya destruktif, jadi tidak perlu `isDestructive`.
- **D-11, ditandai untuk pemilik.** `destructiveRequest(for:title:confirmTitle:safeMode:)`, dengan `confirmTitle: String? = nil`, mengembalikan permintaan untuk `confirm` **dan** `full`, dengan catatan untuk `full`: "This permanently removes the table and its data. It cannot be undone from this app." (Drop) atau "This deletes every row in the table. It cannot be undone from this app." (Truncate). `no_ddl` dan `read_only` tetap `nil`: engine menolak sebelum membuka koneksi, dan bertanya sebelum penolakan tidak ada gunanya. Bila `confirmTitle` nil, ia diturunkan dari `title` tanpa tanda tanya penutup ("Drop Table?" menjadi "Drop Table", "Truncate Table?" menjadi "Truncate Table"; `TableOperation.title` sudah berupa kata kerja bernama), sehingga pemanggil di `+Tree` (`requestTableOperation`, `AppModel+Tree.swift`, milik T3) **tidak diedit** oleh T5. `RunConfirmationTests.testADestructiveOperationOnlyAsksAtConfirm` berubah menjadi `…AsksAtConfirmAndFullButNotWhereTheEngineRefuses`. Bila pemilik menolak D-11, perubahan ini dibatalkan dengan satu baris (`guard safeMode == .confirm`) dan satu tes.

### 8.3 Tes

- `ErrorBannerTests`: pesan bisa diseleksi (trait teks statis dan `.textSelection`), Copy menulis seluruh pesan ke pasteboard, Dismiss mengosongkan `previewError`, Show in Log memilih panel `.log`, banner muncul tanpa mengganti badan, `Announcer.last` berisi kalimat galat.
- `RunConfirmationTests` (diperluas): `confirmTitle` untuk satu dan banyak write dan untuk Drop dan Truncate; perubahan D-11.
- `ConfirmationSheetKeysTests`: probe P-5a, Esc memanggil `onCancel` dan Return tidak memanggil `onApprove` pada sheet yang di-host. Bila `performKeyEquivalent` tidak mencapai `.keyboardShortcut` di jendela uji, tes memeriksa `ConfirmationSheetSpec` (cancelAction ada, defaultAction nihil, label setuju kata kerja bernama) yang dipakai view, dan perilaku diverifikasi tangan di laporan.
- `AppSheetRenderTests.testTheConfirmationSheetRenders` tetap.

## 9. W9-T6: Open Quickly

Ukuran S. Berkas (daftar plan): `Views/OpenQuickly.swift`, `Models/QuickSearch.swift`; ditambah `Models/AppModel+Focus.swift` (`quickResults`, `quicklyScope`, pemulihan fokus; urutan T2 → T6) dan `Support/Theme.swift` (trait `.isSelected` pada `Segmented`; rantai Theme: T1 → T6 → T7). Review SR, AX. Commit: `feat(app): Open Quickly keeps the highlight in view and filters by scope`.

- **Lingkup.** `enum QuickScope: String, CaseIterable { all, objects, saved, history }`, disimpan di `NavigationState.quicklyScope`. `QuickSearch.results(query:nodes:savedQueries:history:scope:limit:)` menyaring sumber kandidat; `.all` identik dengan hari ini, jadi enam tes yang ada tidak berubah. Kontrol: `Segmented` empat opsi di bawah kolom pencarian. **Tab** dan **⇧Tab** memutar lingkup selama kolom punya fokus (palet ini satu kolom modal, Tab tidak punya tugas lain di dalamnya), dan petunjuk "Tab changes scope" tampil di baris hitungan.
- **Sorotan terlihat.** `ScrollViewReader` membungkus daftar, tiap baris diberi `.id(result.id)`, dan `onChange(of: model.openQuicklyIndex)` memanggil `proxy.scrollTo(id)` tanpa jangkar (gulir minimal). Hari ini panah melewati baris yang terlihat memindah sorotan ke luar layar (`OpenQuickly.swift:49-62`, tanpa `ScrollViewReader`).
- **Hasil sebagai daftar AX.** Kontainer `accessibilityLabel("Results")`, tiap baris satu elemen gabungan (judul dan subjudul), nilai "n of m", trait `.isSelected` untuk baris yang disorot. Hitungan hasil diumumkan lewat `Announcer` 400 ms setelah ketikan berhenti ("12 results" atau "No results").
- **Fokus.** `openQuickly()` mencatat `currentRegion`, dan penutupan (Esc, pilih, atau ketuk latar) memulihkannya. Kolom pencarian memakai `accessibilityLabel("Open Quickly")`.
- **Teks.** Subjudul `code(10.5)` menjadi 11 (T9 menyapunya bila T6 tidak).
- **Tes.** `QuickSearchTests` (empat tes lingkup), `OpenQuicklyNavigationTests` (batas sorotan atas hasil berlingkup, pemutaran lingkup, pemulihan wilayah saat tutup).

## 10. W9-T7: Settings

Ukuran M. Berkas (daftar plan): `Views/SettingsView.swift`, `Support/Theme.swift` (`HelpHint`), `Support/EditorPreferences.swift`, `Support/DataPreferences.swift`; ditambah, karena D-16, `Views/SQLEditor.swift` (satu pembacaan ukuran), `Models/GridMetrics.swift`, `Views/GridRowView.swift`, dan `Views/GridHeaderView.swift` (pembacaan ukuran font sel dan label header), serta `Support/Snapshot.swift` (flag `--grid-font`, P-7b). Rantai §7 untuk empat berkas pertama disisipi W9-T7: `SQLEditor.swift` sesudah W4-T2, grid sesudah W6-T1; `Snapshot.swift` sesudah W9-T4 dan sebelum W9-T8 (§16.2). Review SR, UX, AX. Commit: `feat(settings): panes title the window, help is reachable by keyboard, and fonts have a size`.

- **Judul jendela mengikuti pane.** `SettingsView` menyetel judul = `pane.title`. Probe P-7a: apakah `.navigationTitle(pane.title)` pada akar `SettingsView` mengganti judul jendela scene `Settings`? Bila tidak, `WindowTitleBinder` (`NSViewRepresentable` yang menulis `view.window?.title` saat pane berganti).
- **`HelpHint` terjangkau.** Menjadi `Button` polos berglyph `questionmark.circle` dengan `.popover(isPresented:)` yang memuat teksnya, `.help(text)` dipertahankan untuk hover, `accessibilityLabel("Help")` dan `accessibilityHint(text)`, dan area ketuk diperluas 4 pt (`contentShape`) tanpa mengubah ukuran glyph. Dipakai di tiga tempat (`SettingsView.swift:214`, `:1274`, `:1288`).
- **Ukuran font editor.** `EditorPreferences.fontSize` (kunci `editorFontSize`, bawaan 12,5 = angka yang dipakai `SQLEditor.swift:89` hari ini, rentang 10 sampai 28, langkah 0,5 di Settings dan 1 untuk ⌘+/⌘− di W10-T7). `SQLEditor` membaca nilainya lewat `FontChoice.codeNSFont`, dan perubahan ukuran memakai jalur penggantian font yang sudah ada (Blueprint 4B §7.6: `mark_dirty` seluruh dokumen).
- **Ukuran font grid.** `DataPreferences.gridFontSize` (kunci `gridFontSize`, bawaan 12, rentang 11 sampai 16 bilangan bulat). Font sel dan label header mengikuti. Tinggi baris efektif = `preset.points + 2 × (fontSize − 12)`, sehingga ukuran 12 menghasilkan geometri hari ini **persis** (23/25/50 di `GridMetricsTests`) dan ukuran lain naik monoton. Rumus itu usulan; probe P-7b menggambar `grid-kinds`, `-compact`, dan `-tall` pada ukuran 11, 14, dan 16 lewat flag `--grid-font` baru di `Snapshot` (UX memutuskan dari gambar). Font gutter nomor baris tetap 10,5 sampai W10-T2.
- **Kontrol.** Satu baris "Font size" di kartu Editor dan kartu "Data grid": tombol kurang dan tambah bernama ("Decrease font size", "Increase font size"), pembacaan angka 11 pt, dan "Reset".
- **Tabel Keyboard.** Membaca `AppMenu` dan menyembunyikan aksi `isAvailable == false`, dan menulis kalimat platform dari §5.2 di bawah tabel.
- **Tes.** `EditorPreferencesTests` (bawaan, jepit, persisten lewat `UserDefaults` scratch), `DataPreferencesTests` (bawaan 12, jepit 11…16), `GridMetricsTests` (ukuran 12 persis 23/25/50, monoton di atasnya), `SettingsWindowTitleTests` (judul per pane), `HelpHintTests` (label dan hint AX).

## 11. W9-T8: shell native

Ukuran L. Implementer GP-s (O-17). Berkas (daftar plan): `Views/RootView.swift`, `Views/Workspace.swift` (toolbar), `App.swift` (jendela, ukuran minimum), `Views/ContextCascade.swift`, `Support/Snapshot.swift`; ditambah `Views/SidebarTree.swift` dan `Views/SchemaOutline.swift` (kepala kolom sidebar, hanya bila perlu) dan `ShellTests`. Review SR, UX, AX, AR, CR. Commit: `feat(app): a native split view and toolbar around the same surfaces`.

### 11.1 Bentuk

```
┌───────────────────────────────────────────────────────────────────────┐
│ ● ● ●  [⊟]  penerima_manfaat                     (judul + subjudul)    │ bilah judul dan toolbar native
│        [conn ▾][catalog ▾][schema ▾]      [PROD][Confirm]  [▶ Run ⌄][■][☰]
├──────────────────┬────────────────────────────────────────────────────┤
│ ⬡ QUERYHIVE  + ⟳ │ [Query 1][Query 2][+]                              │ 36  TabStrip (SwiftUI)
│ [ filter…      ] ├────────────────────────────────────────────────────┤
│ ▾ outline        │ SQL editor                                         │
│                  ╞═══════════════ PanelResizer ═════════════════════╡
│                  │ BottomPanel                                        │
│                  ├────────────────────────────────────────────────────┤
│                  │ ● conn · Connected  [badges]       Query OK · …    │ 26  StatusBar (SwiftUI)
└──────────────────┴────────────────────────────────────────────────────┘
```

- **Kerangka:** `NavigationSplitView(columnVisibility:)` dua kolom. Sidebar = kolom pohon (kepala, favorit, outline). Detail = `VStack { Workspace; StatusBar }`. Lebar sidebar `.navigationSplitViewColumnWidth(min: 190, ideal: 264, max: 460)`, angka yang sama dengan jepit `SidebarResizer` (`RootView.swift:192`) dan `AppModel.sidebarWidth` bawaan.
- **Toolbar:** `.toolbar { … }` pada kolom detail. Breadcrumb (`ContextCascade`, tiga level 200 pt) di tengah atau di tepi depan (probe P-8c memilih yang tidak terpotong). Kelompok kanan: `BadgeCluster` (T4) dan grup Run (Run, menu varian, Stop, Explain). `HubButton` dan `IconButton` tetap dipakai di dalamnya.
- **Judul:** `.navigationTitle(tab.title)` dan `.navigationSubtitle("<koneksi> · <target>")`, termasuk lencana lingkungan dan Safe Mode sebagai teks bila ada.
- **Jendela:** `.windowStyle(.hiddenTitleBar)` dihapus, `.windowToolbarStyle(.unified)` ditambah, `.windowResizability(.contentMinSize)` dipertahankan. `minWidth` dan `minHeight` diukur ulang (§11.4, P-8c); 1120 diukur untuk toolbar yang sudah tidak ada (`App.swift:44-47`).
- **Dihapus:** `TitleStrip` dan `Metrics.titleStrip` (40 pt), `SidebarResizer`, `AppModel.sidebarWidth` dan `NavigationState.sidebarHidden`. `QueryToolbar` sebagai baris 46 pt di dalam konten hilang karena isinya pindah ke toolbar, jadi editor mendapat sekitar 46 pt. Toggle sidebar memakai aksi responder bawaan (`toggleSidebar:`) dari item menu T2.
- **Tetap:** `TabStrip`, `StatusBar`, `PanelResizer`, `Backdrop`, dan seluruh sheet dan overlay (`RootView` tetap tempat `.sheet`, `.alert`, dan overlay Open Quickly dipasang, pada akar `NavigationSplitView`).
- **Divider yang bisa diubah lewat keyboard (FR-UI-08).** Divider sidebar adalah `NSSplitView` milik `NavigationSplitView`, yang bisa dijangkau lewat Full Keyboard Access dan VoiceOver; ini tidak diverifikasi (§17.2). `PanelResizer` mendapat `accessibilityAdjustableAction` (naik atau turun 24 pt, dijepit 96…560 seperti gestur `Workspace.swift:445`), label "Result panel height", dan nilai tinggi dalam poin.

### 11.2 Harness snapshot

`Snapshot.run` hari ini membuat `NSWindow` ber-`[.titled, .fullSizeContentView]` dengan bilah judul transparan dan menangkap `contentView` (`Snapshot.swift:145-157`, `:188-195`). Toolbar bukan bagian `contentView`, jadi tanpa perubahan semua scene shell kehilangan toolbar mereka. Perubahan T8:

1. Untuk scene `RootView`: `hosting.sceneBridgingOptions = [.toolbars, .title]` (API terverifikasi di SDK, macOS 14), jendela `[.titled, .closable, .resizable]` tanpa `.fullSizeContentView` dan tanpa bilah judul transparan.
2. `capture` memakai `window.contentView?.superview` bila `window.toolbar != nil`, selain itu `contentView`.
3. Scene Settings, `grid-json`, dan `parameters` tidak berubah. `VisualParityTests` menggambar `ResultGrid` dan editor langsung, jadi tidak tersentuh.

### 11.3 Perubahan piksel (V-7)

Bilah judul native dengan judul dan subjudul menggantikan strip kosong 40 pt. Toolbar menjadi toolbar sistem (kaca di macOS 26) dan tidak lagi baris `Tone.ink 0,03`. Sidebar setinggi jendela dengan divider sistem, dan status bar hanya selebar kolom detail. Editor naik sekitar 46 pt. Semua scene chrome T0b yang memuat shell direkam ulang sebagai V-7 di commit sendiri, satu per satu (§0.5 `development-plan.md`), dengan persetujuan UX dan AX.

### 11.4 Probe sebelum kode

| ID | Pertanyaan | Lulus bila | Bila gagal |
|---|---|---|---|
| P-8a | Apakah `NSHostingView` dengan `sceneBridgingOptions = [.toolbars, .title]` di `NSWindow` biasa memasang `NSToolbar` dari `.toolbar { }`? | `window.toolbar?.items` terisi setelah satu putaran run loop | coba `NSHostingController` sebagai `contentViewController` (opsi yang sama ada di sana) |
| P-8b | Apakah `cacheDisplay` pada `window.contentView?.superview` memuat piksel item toolbar dan judul? | irisan di tempat tombol Run punya piksel jenuh | harness menggambar `ToolbarFallbackStrip` (view toolbar yang sama dalam baris 46 pt di atas `contentView`); selisihnya dicatat sebagai batas harness |
| P-8c | Lebar jendela terkecil tempat breadcrumb, lencana, dan Run semuanya terlihat dan tidak ada yang masuk menu limpahan; penempatan breadcrumb yang tidak terpotong | angka diukur dengan `--snapshot --scene table --width N` seperti pengukuran 1120 dulu, dan dicatat di `DESIGN.md` | naikkan `minWidth`; lebar level tidak boleh dikecilkan (PR-13) |
| P-8d | Apakah `ignoresSafeArea` di akar dan toolbar native saling mengganggu, dan apakah sheet tetap menempel? | semua scene merender | pindahkan `ignoresSafeArea` ke latar saja |

**Bila P-8a dan P-8b sama-sama gagal, T8 dinyatakan terblokir dan `planner` menyusun ulang.** Jalan alternatifnya adalah shell AppKit: `MainWindowController` (`NSSplitViewController` dan `NSToolbar` yang isinya `NSHostingView`) yang dibuat `AppDelegate` dan dipakai juga oleh `Snapshot.run`, dengan scene `Settings` dan `.commands` tetap SwiftUI dan `AppModel` dipegang oleh pemegang bersama. Itu satu tugas baru berukuran L, bukan perluasan T8.

### 11.5 Tes

- `ShellTests` (baru): semua scene `--snapshot` chrome merender tanpa galat; ukuran jendela minimum sama dengan angka P-8c; item toolbar yang diharapkan ada; `toggleSidebar:` mencapai split view; `PanelResizer` bisa disetel lewat aksi AX.
- `ChromeParityTests` (T0b): scene shell direkam ulang sebagai V-7.
- `Snapshot` mendapat `--scene shell-narrow` pada lebar minimum sebagai bukti P-8c.

## 12. W9-T9: lantai 11 pt (pemetaan per pemanggilan)

Ukuran M. Serial dan terakhir di W9. Berkas: semua view yang punya pemanggilan teks di bawah 11 pt **kecuali** yang dikecualikan di bawah. Review UX, AX, SR. Commit: `fix(a11y): nothing a person must read is smaller than 11pt`. Karena D-18, T9 tidak merekam ulang baseline grid atau editor; hanya scene chrome T0b yang berubah (V-8, satu commit per scene).

### 12.1 Aturan

1. **Teks yang harus dibaca ≥ 11 pt.** 10,5 menjadi 11. 10 dan 9,5 dan 9 menjadi 11, kecuali butir 2.
2. **Label dekoratif huruf besar** boleh 10 pt bila inknya ≥ 0,68 (`Tone.secondary`). Dibuat dengan `DecorativeLabel(text:weight:)` (menggantikan modifikasi manual; `SectionLabel` memakainya), dan pemanggil lain wajib memberi komentar `// decorative-label`.
3. **Glyph SF Symbol** (`Image(systemName:)` dengan `.system(size:)`, 31 pemanggilan di 7 sampai 10,5 pt) dikecualikan dari lantai. Syaratnya: tidak berdiri sendiri tanpa nama (§4.2).
4. **Teks yang digambar AppKit di grid dan editor** tidak disentuh di W9 (D-18).

### 12.2 Pemetaan

| Berkas | Baris di `8103478` | Sekarang | Menjadi | Catatan |
|---|---|---|---|---|
| `Support/Theme.swift` | 691 | `.ui(10.5, .semibold)` (SectionLabel, huruf besar) | `DecorativeLabel` 10 | butir 2 |
| `Views/ConnectionsViews.swift` | 819 | `.code(10.5)` ("port N") | 11 | |
| `Views/ImportSheet.swift` | 53 | `.code(10.5)` | 11 | |
| | 128, 258, 286, 290 | `.ui(10.5)` | 11 | kalimat bantu; tinggi sheet tumbuh sedikit |
| `Views/JSONTreeView.swift` | 47 | `.code(10.5)` | 11 | hitungan pencarian |
| `Views/OpenQuickly.swift` | 89 | `.code(10.5)` | 11 | disapu T6 bila T6 belum |
| `Views/Panels.swift` | 117 | `.ui(10, .semibold)` | 11 | angka di kapsul tab; tinggi `PanelTabButton` tetap 21 |
| | 180 | `.code(10.5)` | 11 | cap waktu log |
| | 237, 256 (semibold), 330, 357 | `.ui(10.5)` | 11 | |
| | 285 | `.code(10.5)` | 11 | jalur folder |
| | 473 | `.ui(10)` | 11 | |
| | 479 | `.code(10)` | 11 | tanggal riwayat |
| `Views/SettingsView.swift` | 176 | `.ui(10.5)` (label pane) | 11 | item 92 pt muat |
| | 287, 330, 968, 1086 | `.ui(10.5)` | 11 | |
| | 374 | `.code(10)` | 11 | |
| | 675 | `.ui(9.5, .semibold)` (judul grup huruf besar) | `DecorativeLabel` 10 | butir 2 |
| | 696 | `.ui(9.5)` | 11 | detail tampilan tersimpan |
| | 818 | `.ui(9)` (nama tema di kotak) | 11 | periksa lebar tujuh kotak ("Midnight" paling panjang) |
| `Views/SuggestionPopup.swift` | 26 | `.ui(10)` (jenis saran) | 11 | |
| `Views/Workspace.swift` | 488 | `.code(10.5, .semibold)` | 11 | |
| | 865 | `.code(10.5)` | 11 | |
| | 899, 904 | `.ui(10.5)` | 11 | |
| | 917 | `.code(10)` | 11 | tipe kolom |
| | 947 | `.ui(9.5, .bold)` (huruf besar) | `DecorativeLabel` 10 | butir 2 |
| | 955 | `.ui(9.5, .semibold)` (label field, bukan huruf besar) | 11 | |

Milik tugas lain (tidak disentuh T9, dikerjakan di sana):

| Berkas | Baris | Pemilik | Catatan |
|---|---|---|---|
| `Views/SidebarTree.swift` | 76 ("QUERYHIVE", 10,5 bold), 171 ("FAVOURITES", 9,5 bold), 337 (pesan, 10,5) | W9-T3 | 76 dan 171 menjadi `DecorativeLabel` 10, 337 menjadi 11 |
| `Views/Workspace.swift` | 402 (hitungan baris di sudut editor, `.code(10.5)`) | W10-T7 | **Sengaja ditunda.** `ReadoutColourTests` menetapkan nomor gutter dan hitungan sudut sebagai satu bacaan. Menaikkan satu saja memisahkan keduanya; W10-T7 menaikkan kedua-duanya ke 11 di bawah V-10. |
| `Views/ResultGrid.swift` | 12 pemanggilan teks (toolbar, footer, "LIMIT" 10 huruf besar, pemilih nilai, dll.) | W10-T3 | ikut tertangkap 30 baseline grid; W10-T3 merekam ulang di bawah V-9 |
| `Models/GridMetrics.swift:127`, `Views/GridRowView.swift:143` | gutter nomor baris 10,5 | W10-T2 | kotak gutter 25 pt = tinggi baris 13 + padding 12; 11 pt mengubah geometri baris dan harus diturunkan di bawah V-9 |
| `Views/SQLEditor.swift` | 205 dan 2024 (font nomor gutter 10,5) | W10-T7 | V-10 |

### 12.3 Tes

- `FontFloorTests` (baru, pemindai sumber atas `app/Sources`): literal `.ui(` dan `.code(` di bawah 11 hanya lolos bila baris itu memuat `DecorativeLabel` atau komentar `// decorative-label`, dan nilainya ≥ 10. Daftar izin tertulis untuk lima berkas milik W10 di atas, dengan tugas penutupnya. Daftar itu mengecil sampai kosong di gate W10.
- `DecorativeLabelTests`: ink 0,68 dan ukuran 10.
- `ChromeContrastTests` (T1) dijalankan ulang.
- Scene chrome T0b yang memuat Panels, Settings, Workspace, dan Import direkam ulang sebagai V-8, satu per satu.

## 13. Spesifikasi UX (sudut pandang ui-ux-designer)

Ukuran dalam poin. Ukuran teks 11 pt ke atas kecuali label dekoratif (§12). Semua nilai kontras dari §1.8. Perubahan terhadap hari ini ditandai dengan kode V.

| Permukaan | Bentuk dan ukuran | Keadaan | Salinan dan kunci |
|---|---|---|---|
| **Strip tab** (V-2) | Tinggi 36. Chip tinggi 27, sudut 7, padding 10. Glyph tahap 9 di depan judul. Judul 12 (semibold bila terpilih), satu baris, dipotong ekor. Lencana (T4) di belakang judul. Tombol tutup target 20×20. Tombol "+" 24×24. Strip menggulir horizontal. | Terpilih: isian `ink 0,13` dan cincin aksen 0,35. Hover: `ink 0,06`. Fokus keyboard: cincin `Tone.focusRing` 2. Berjalan: spinner mini (diam bila Reduce Motion). Gagal: segitiga seru coral. Selesai: centang mint. Idle: tanpa glyph. | "+" bantuan: "New Query (<kunci skema>)". Menu konteks tab tidak berubah. ⌘1…9 dan ⇧⌘[ ] (§5). |
| **Pohon** (V-5) | Sidebar 190…460, ideal 264. Baris 23, indentasi 15, chevron 8 pt di kotak 11, ubin driver 16, ikon level 16, judul 12. Pesan baris 11. Kepala: "QUERYHIVE" 10 dekoratif, menu +, muat ulang, filter 24. | Pilihan: isian 0,13. Pilihan dengan fokus: ditambah cincin 1,5. Hover 0,06. Memuat, kosong, dan galat sebagai baris pesan. | Kosong: "No connections yet." dan "Add a connection to browse its catalogs, schemas and tables." |
| **Toolbar** (V-7) | Toolbar native. Breadcrumb tiga level selebar tetap 200 (PR-13). Kelompok kanan: lencana, Run, menu varian, Stop, Explain. | Run nonaktif bernuansa kosong dengan teks redup (seperti hari ini). Stop selalu ada. | Bantuan menyebut kunci skema aktif. |
| **Judul** | Judul = tab. Subjudul = koneksi dan target, ditambah lencana sebagai teks. | | |
| **Status bar** | Tinggi 26, di kolom detail. Titik status 7 dan teks koneksi 11. Lencana Safe Mode dan lingkungan (selalu). Ringkasan tab di kanan. | Gagal: ringkasan coral semibold. | |
| **Lencana** (V-3) | Kapsul `tint 0,14`, glyph 10, teks 11 mono semibold, padding 8/3. | Di lebar sempit hanya glyph. | PROD, STAGING, DEV. Full, No DDL, Confirm, Read only. |
| **Banner galat** (V-4) | Bilah penuh di atas badan: padding 12, isian `markCoral 0,10`, garis bawah `Tone.hairline`. Glyph 14, judul 12 semibold, pesan 11,5 maks 120 lalu menggulir. Tiga `PillButton` kompak di kanan. | Tanpa hasil di bawahnya (D-13). | "Query failed", Copy, Show in Log, Dismiss. |
| **Lembar konfirmasi** (V-4) | Lebar 620 seperti sekarang. Glyph perisai 14 `markAmber`. Pernyataan di kotak `recess 0,30` maks 260. | Tombol setuju coral dan kata kerja bernama. Cancel mendapat fokus awal. Return tidak berbuat apa-apa. | "Run Write", "Run 3 Writes", "Drop Table", "Truncate Table". Esc membatalkan. |
| **Open Quickly** (V-6) | Lebar 560. Kolom 46. Di bawahnya empat lingkup (tinggi 28) dan baris hitungan. Baris hasil 36: judul 12,5, subjudul 11 mono. Daftar maks 320, menggulir mengikuti sorotan. | Sorotan: isian aksen 0,16 dan teks tetap `ink`. | "Search objects, saved queries and history". Tab mengubah lingkup. |
| **Settings** (V-6) | Jendela 560×640. Bar pane tidak berubah. Judul jendela = pane. `HelpHint` membuka popover maks 280. | Fokus keyboard pada `HelpHint` memberi cincin. | Baris "Font size" dengan tombol kurang dan tambah, angka, Reset. |
| **Ruang kerja kosong** (V-2) | Tidak berubah selain salinan. | Reduce Motion: pahlawan diam. | "No query open" dan kalimat baru (§4.4). |
| **Pemisah panel** | 7 pt, garis 1. Dapat disetel lewat aksi AX. | Kursor ubah ukuran saat hover seperti sekarang. | |

**Gerak.** Reduce Motion mematikan: goyang `HiveHero`, crossfade `Backdrop` 0,8 dtk, spring `PressScale`, dan spinner (diganti glyph statis). Transisi hover ≤ 0,2 dtk tetap.

**Permukaan.** Reduce Transparency atau Increase Contrast: isian solid, hairline 0,20, secondary 0,85, glow mati, di semua permukaan yang lewat `.surface` (§4.1), termasuk latar sidebar.

**Warna dan non-warna.** Tidak ada status yang hanya ditandai warna: tahap tab punya bentuk, lencana punya teks, banner punya glyph dan kata. Tanda dan garis bawah yang memakai warna kategorikal di appearance terang memakai varian dalam (§1.8, D-8).

## 14. Spesifikasi AX (sudut pandang a11y-architect)

| Permukaan | Peran dan trait | Label | Nilai dan petunjuk | Aksi dan kunci | Pengumuman |
|---|---|---|---|---|---|
| **Strip tab** | grup berisi tombol | "Query tabs" | | ⌘1…9, ⌃Tab, ⇧⌘[ ] | pindah tab: "<judul>, <tahap>" |
| **Chip tab** | tombol, `.isSelected` bila terpilih | `tab.title` | `running`, `finished`, `failed`, atau tak ada; lalu "Environment: production. Safe Mode: Confirm." bila ada | tekan = pilih; aksi kustom "Close" | |
| **Tombol tutup tab** | tombol | "Close <judul>" | | tersembunyi dari AX saat tak terlihat | |
| **Tab panel** | tombol, `.isSelected` | nama panel | hitungan bila ada | | |
| **Tombol ikon** | tombol | `label:` wajib | `help` sebagai petunjuk | | |
| **Pohon** | `AXOutline` | "Object tree" | | ↑↓←→, Return, type-select, ⌥⌘1 | galat muat: "<judul>: <galat>" |
| **Baris pohon** | `AXOutlineRow` (AppKit) | judul | koneksi: "Connected" atau "Not browsed yet"; lainnya: jenis. Petunjuk: kalimat bantuan | aksi "Open", "Refresh" | |
| **Toolbar** | tombol dan pemilih | "Run", "Run options", "Stop", "Explain", "Connection", "Catalog" atau "Database", "Schema" | pemilih: nilai sekarang | tombol Run: kunci skema | |
| **Lencana** | teks statis | "Environment: production" / "Safe Mode: Confirm" | `ConnectionSafeMode.detail` sebagai petunjuk | | |
| **Status bar** | grup | "Status" | "<koneksi>, <status>. <ringkasan tab>" | | perubahan status koneksi diumumkan sopan (`.low`) |
| **Banner galat** | grup, peringatan | "Query failed" | teks pesan (statis, bisa diseleksi) | Copy, Show in Log, Dismiss | muncul: "Query failed: <160 karakter>", prioritas tinggi |
| **Lembar konfirmasi** | sheet (sistem) | judul permintaan | pernyataan sebagai teks statis; tombol setuju = kata kerja bernama | Esc batal; fokus awal Cancel | |
| **Open Quickly** | kolom pencarian dan daftar | "Open Quickly", "Results" | baris: judul + subjudul, "n of m", `.isSelected` untuk sorotan | ↑↓, Return, Esc, Tab (lingkup) | "n results" / "No results" setelah ketikan berhenti |
| **Lingkup Open Quickly** | tombol, `.isSelected` | All, Objects, Saved, History | | Tab, ⇧Tab | |
| **`HelpHint`** | tombol | "Help" | petunjuk = teks bantuan | Space atau Return membuka popover | |
| **Pemisah panel** | dapat disetel | "Result panel height" | tinggi dalam poin | aksi naik dan turun | |
| **Judul jendela Settings** | judul jendela | nama pane | | | |

**Urutan fokus.** Toolbar, kolom filter pohon, pohon, strip tab, editor, tab panel, grid, status bar. ⌥⌘1, ⌥⌘2, ⌥⌘3 melompat ke pohon, editor, dan hasil. Esc di Open Quickly dan di lembar mengembalikan fokus ke wilayah sebelumnya.

**Pengaturan sistem.** Reduce Motion, Reduce Transparency, dan Increase Contrast dibaca dari satu tempat (§4.1) dan dipakai di semua permukaan chrome. Grid memakai sumber yang sama di W10-T2.

**Pemeriksaan otomatis.** `AccessibilityLabelTests` (§4.6), `TabChipSpecTests`, `SchemaOutlineTests` (atribut AX), `BadgeSpecTests`, `ErrorBannerTests`, `ConfirmationSheetKeysTests`, `ChromeContrastTests`. **Dijalankan pemilik** dari laporan akhir: smoke VoiceOver pada pohon, strip tab, banner galat, dan Open Quickly.

## 15. Gate visual dan tes

### 15.1 W9-T0b: baseline chrome (tugas tambahan, D-19)

test-engineer · sonnet, review CR ringan (tingkat rendah). Berkas: `app/Tests/QueryHiveTests/ChromeParityTests.swift` (baru) dan PNG serta JSON sidecar `chrome-<scene>-<dark|light>` di `__Baselines__/`. **Tidak menyentuh `VisualParityTests.swift`** (berada di rantai grid §7): ia memakai ulang `Pixels`, `Sidecar`, dan `VisualParityTests.compare` yang bukan `private`, dan menyalin pembantu `host`, `settle`, `png` (±60 baris) karena ketiganya `private`. Commit: `test(visual): chrome baselines for the shell, the tree, Open Quickly and Settings`. Dijalankan setelah T0 dan **sebelum** T1.

- **Lapis yang dipakai:** ukuran gambar (bagian dari tata letak) dan lapis piksel (≤ 0,1% piksel selisih > 16/255). Lapis tata letak, warna titik sampel, dan penanda tidak ada untuk chrome.
- **Scene (calon):** `empty`, `fresh-tab`, `groups`, `tree-large`, `tree-empty-schema`, `tree-databases`, `quickly`, `cascade-long`, `cascade-postgres`, `cascade-mysql`, `objects`, `settings`, `settings-editor`, `settings-data`, `settings-keyboard`, `connection`. Dua appearance, jadi sekitar 32 PNG.
- **Aturan stabil:** sebelum merekam, tiap scene digambar dua kali dan harus identik byte demi byte. Scene dengan cap waktu (log panel seed, `Date(timeIntervalSinceNow:)`) dinormalkan di tes (`tab.logLines = []`, `tab.panel = .result`) atau dikeluarkan, dan daftar yang dikeluarkan ditulis di laporan. `done`, `running`, dan `files` hampir pasti dikeluarkan (§1.4).
- **Merekam hanya berkas baru**, jadi tidak ada baseline yang ada berubah. Persetujuan pemilik yang dibutuhkan §0.5 untuk rekam ulang tidak berlaku, dan preseden 5a (scene tambahan) ada di ledger.
- **Bila ditolak:** G-VIS di W9 berarti "grid dan editor tidak berubah", dan V-2…V-8 hanya terjaga oleh pasangan berdampingan W14-T3 dan tinjauan pemilik.

### 15.2 Peta V ke scene

| Kode | Tugas | Scene chrome yang direkam ulang (satu commit `test(visual): re-record <scene> for V-n` per scene) |
|---|---|---|
| V-2 | T1 | `empty`, `fresh-tab` |
| V-3 | T4 | `cascade-long`, `cascade-postgres`, `cascade-mysql`; scene baru `badges` |
| V-4 | T5 | scene baru `error-banner`; lembar konfirmasi lewat `AppSheetRenderTests` |
| V-5 | T3 | `tree-large`, `groups`, `tree-empty-schema`, `tree-databases` |
| V-6 | T6, T7 | `quickly`, `settings*` |
| V-7 | T8 | semua scene yang memuat shell |
| V-8 | T9 | scene yang memuat `Panels`, `Settings`, `Workspace`, dan impor |

Grid dan editor (48 PNG) **tidak berubah di W9**. Persetujuan UX dan AX diperlukan untuk tiap rekam ulang (§0.5).

### 15.3 Tes yang ditulis lebih dulu

Per tugas, tes ditulis sebelum kodenya: `AppModelSplit` tidak punya tes baru (kompilasi dan skrip §3.4); T1 `AccessibilityLabelTests`, `TabChipSpecTests`, `SurfacePolicyTests`, `ChromeContrastTests`, `EmptyCopyTests`; T2 `ShortcutConflictTests`, `AppMenuTests`, `FocusMoveTests`; T3 `SchemaOutlineTests`, `TreeMenuTests`; T4 `ConnectionEnvironmentTests`, `BadgeSpecTests`; T5 `ErrorBannerTests`, `ConfirmationSheetKeysTests`, `RunConfirmationTests` (diperluas); T6 `QuickSearchTests` (diperluas), `OpenQuicklyNavigationTests`; T7 `EditorPreferencesTests`, `DataPreferencesTests`, `GridMetricsTests` (diperluas), `SettingsWindowTitleTests`, `HelpHintTests`; T8 `ShellTests`; T9 `FontFloorTests`, `DecorativeLabelTests`. Hitungan tes Swift tidak boleh turun (NFR-Q).

### 15.4 Gate W9

G-HEAVY, G-BENCHQ (alias `scroll-1m`, `ttfr-pg`, dan `type-10k` di `BenchMode.aliases` masih berlaku; **tambahkan `launch-warm`** karena shell berubah, NFR-P7), tes lantai aksesibilitas (§15.3), dan pemindai: tidak ada `Tone.ink.opacity(0.07)` di luar daftar izin berkas milik W10, tidak ada `IconButton` tanpa `label:` di berkas milik W9, dan tidak ada literal `.ui(`/`.code(` di bawah 11 di luar daftar izin W10.

## 16. Urutan, kepemilikan berkas, dan perubahan pada dokumen lain

### 16.1 Urutan batch (dari `development-plan.md` W9, dengan tambahan)

1. **T0** sendirian, lalu **T0b** (sebelum T1).
2. T1, T2, T3 (maksimal tiga implementer). T3 tidak menyentuh `Theme.swift` dan memakai token T1 hanya sesudah T1 mendarat; sebelum itu ia memakai literal lama dan T9 menyapu.
3. T4 (sesudah T1), T5, T6 (sesudah T2).
4. T7 (sesudah T1) dan T8 (sesudah T1, T2, T3, dan T4; T3 ditambahkan karena sidebar T8 memuat outline). Keduanya mengubah `Support/Snapshot.swift`, jadi untuk berkas itu T7 mendarat lebih dulu (ia hanya menambah flag `--grid-font`) dan T8 mengambilnya dari kepala sesudahnya.
5. T9 sendirian.

### 16.2 Perubahan rencana yang dibutuhkan (untuk orkestrator)

`development-plan.md` §5, baris W9:

- tambah **W9-T0b** (§15.1);
- W9-T1 berkas: `+ Support/ThemeStore.swift`, `Views/Panels.swift` (hanya `PanelTabButton`), `Support/Accessibility.swift` (baru), `Support/Snapshot.swift` (flag `--reduce-motion`, `--reduce-transparency`, `--increase-contrast`, §4.1);
- W9-T2 berkas: `+ Models/AppMenu.swift` (baru), `Views/RootView.swift`, `Models/AppModel.swift` (satu baris, `var navigation`, §5.6);
- W9-T3 berkas: `+ Models/TreeMenu.swift` (baru); kalimat "dialog Truncate/Drop" pindah ke W9-T5 (D-11);
- W9-T4 berkas: `+ Support/Snapshot.swift`;
- W9-T5 berkas: `Models/RunConfirmation.swift` memuat D-11 (`destructiveRequest` dan `Request.confirmTitle` bernilai bawaan, jadi `Models/AppModel+Tree.swift`, milik T3, tidak disentuh T5, §8.2);
- W9-T6 berkas: `+ Models/AppModel+Focus.swift`, `Support/Theme.swift` (trait `Segmented`);
- W9-T7 berkas: `+ Views/SQLEditor.swift`, `Models/GridMetrics.swift`, `Views/GridRowView.swift`, `Views/GridHeaderView.swift` (D-16), `Support/Snapshot.swift` (flag `--grid-font`, P-7b);
- W9-T9: "semua view" menjadi daftar §12.2, dan V-8 menjadi chrome saja.

`development-plan.md` §7, rantai kepemilikan:

| Berkas | Rantai |
|---|---|
| `Views/RootView.swift` (baris baru) | W9-T2 → W9-T4 → W9-T8 |
| `Models/AppModel.swift` | ... W6-T1 → W9-T0 → **W9-T2** (satu baris: `var navigation`). Sesudahnya, satu pemilik per extension |
| `Support/Snapshot.swift` | W1-T3 → W5-T1 → W6-T1 → **W9-T1** (flag `--reduce-*`, `--increase-contrast`) → **W9-T4** (scene `badges`) → **W9-T7** (flag `--grid-font`) → W9-T8 |
| `Support/Theme.swift` | W9-T1 → **W9-T6** → W9-T7 → W10-T2 |
| `Support/ThemeStore.swift` (baris baru) | W9-T1 |
| `Views/SQLEditor.swift` | W1-T4 → W2-T3 → W4-T2 → **W9-T7** → W10-T6 → W10-T7 → W12-T2 |
| `Views/ResultGridTable.swift`, `GridTableView`, `GridRowView`, `GridHeaderView`, `Models/GridMetrics.swift` | W5-T1 → W6-T1 → **W9-T7** → W10-T1 → … |
| `Models/AppModel+Focus.swift` | W9-T2 → W9-T6 → **W10-T7** (`adjustFontSize`, blueprint W10 §9.2) → W12-T2 |
| `Models/RunConfirmation.swift`, `Views/RunConfirmationSheet.swift` | W9-T5 |

`development-plan.md` §1: G-BENCHQ dan G-LEAK tidak berubah selain tambahan `launch-warm` di W9.

PRD: FR-RUN-06 (kalimat "di atas hasil sebelumnya", D-13), FR-UI-06 (grid dan editor di W10, D-18), V-8 (chrome saja), dan butir "tanpa tint prod" di FR-SAFE-01. `app/DESIGN.md` (W9-D): bagian Shell, Shortcut schemes (tiga tabel), dan Appearance (token, kebijakan permukaan).

## 17. Risiko dan batas bukti

### 17.1 Risiko

| ID | Risiko | Mitigasi |
|---|---|---|
| R-1 | T8: `NavigationSplitView` dan toolbar SwiftUI tidak tertangkap harness, atau toolbar memotong breadcrumb lebar. | P-8a sampai P-8d sebelum kode. Bila gagal, T8 terblokir dan `planner` menyusun jalan AppKit (§11.4). Dua tugas lain tidak bergantung padanya kecuali T9. |
| R-2 | T3: perilaku tabel disclosure kustom dan AX outline tidak seperti yang diharapkan VoiceOver, dan hanya smoke manual yang bisa memastikannya. | P-3a sampai P-3d dan `SchemaOutlineTests`; smoke VoiceOver dicatat di laporan akhir. |
| R-3 | T0b: scene chrome tidak stabil (material kaca, cap waktu, animasi). | Aturan dua tangkapan identik, normalisasi log, dan pengecualian tertulis. |
| R-4 | T0 bertabrakan dengan 6b W6-T1 di `AppModel.swift`. | Rantai §7 (W6-T1 → W9-T0), pemetaan per nama. |
| R-5 | Migrasi literal ke token menyentuh hampir semua berkas view. | Token mempertahankan nilai lama pada kontras normal; tiap tugas hanya memindahkan berkasnya; pemindai di gate W9. |
| R-6 | Bendera NSWorkspace berbeda di jendela uji atau snapshot. | `pin(...)`, sama dengan `systemIsDark`. |
| R-7 | D-6 mengubah perilaku untuk pengguna skema DBeaver: ⌘W kini menutup tab (bukan jendela), ⌘. menghentikan, ⇧⌘O membuka palet. | Dicatat di W9-D dan di kalimat Settings. Perubahannya menutup bolong, bukan mengambil kunci yang dipakai. |
| R-8 | D-11 mengubah kontrak yang diuji. | Ditandai untuk pemilik; satu baris dan satu tes bila dibatalkan. |
| R-9 | Font grid > 12 membuat Compact terpotong. | Rumus tinggi baris D-16 dan probe P-7b; UX memutuskan dari gambar. |
| R-10 | `ViewThatFits` di dalam item `NSToolbar` hasil SwiftUI mungkin tidak berpindah varian. | P-8c; cadangannya dua varian statis menurut lebar jendela yang diukur. |
| R-11 | Fokus tab dan menu di dalam sheet: ⌘1…9 tetap menyala di jendela di belakang sheet. | Item menu dinonaktifkan saat `NSApp.modalWindow != nil` atau sheet terpasang (diperiksa di `AppMenuTests`). |

### 17.2 Yang tidak bisa diverifikasi (ditulis tanpa build atau jalan)

1. Apakah scene `Window` SwiftUI memasang item File > Close (⌘W) bawaan, sehingga skema DBeaver tanpa `closeTab` menutup jendela (§1.2). Alasannya dari kode (`closeTab` tidak ada di tabel, `applicationShouldTerminateAfterLastWindowClosed` benar), bukan dari jalan.
2. Bahwa item Explain mendahului Export di menu Query sehingga ⌘E menjalankan Explain (§1.2).
3. Apakah ⌘S Eclipse (`org.eclipse.ui.file.save`) memang M1+S di DBeaver. Tabel DBeaver di kode dibaca dari berkas SQL editor, bukan dari platform Eclipse; ⌘S di skema DBeaver di sini adalah konvensi macOS.
4. Perilaku klik kanan `NSOutlineView` (menyorot baris tanpa mengubah pilihan).
5. Apakah divider `NavigationSplitView` dapat disetel lewat Full Keyboard Access dan VoiceOver.
6. Apakah `.navigationTitle` pada akar `SettingsView` mengganti judul jendela scene `Settings`.
7. Ketersediaan nama SF Symbol usulan (`testtube.2`, `lock.shield`, `hammer`, `exclamationmark.octagon.fill`) di target macOS 14.
8. `.defaultFocus` pada sheet macOS 14, dan penerapan `.keyboardShortcut(.cancelAction)` oleh `performKeyEquivalent` di jendela uji.
9. Bahwa `NSTextView` menerima jatuhan `NSPasteboardItem` dari `NSOutlineView` tanpa kode tambahan (P-3d).
10. Pembacaan VoiceOver nyata untuk pohon, chip tab, banner, dan Open Quickly. Hanya pohon AX yang diuji otomatis.
11. Angka kontras adalah hitungan WCAG dari heksadesimal, belum diukur dari piksel yang dirender (kaca dan material menggeser kanvas efektif).

## Untuk pemeriksa

AR, UX, dan AX diminta memutuskan:

1. **D-1 dan D-2:** sembilan extension (bukan sepuluh), properti tersimpan di inti, dan struct keadaan per domain.
2. **D-4 sampai D-6:** kelas kunci platform di kedua skema, terutama bahwa skema DBeaver tidak lagi "hanya apa yang DBeaver deklarasikan".
3. **D-7:** `ThemeStore` sebagai satu sumber, dan token `hairline`, `outline`, `focusRing`, `mark*`.
4. **D-11:** pertanyaan di Truncate dan Drop pada `full` (keputusan pemilik).
5. **D-13:** banner tanpa hasil di bawahnya.
6. **D-17:** SwiftUI-native sebagai bawaan shell, dan jalan mundur AppKit sebagai re-plan.
7. **D-18 dan D-19:** lantai 11 pt hanya teks dan tanpa grid dan editor, dan baseline chrome sebagai tugas tambahan.
8. Kelengkapan §13 dan §14 sebagai spesifikasi, dan apakah lencana di breadcrumb cukup di lebar minimum.

## Verdict architect-reviewer

**Verdict: perlu revisi (changes requested), 6 Okt 2026.** Satu temuan memblokir (kepemilikan berkas di "Perubahan rencana"). Ia **sudah diterapkan** ke dokumen ini (§4, §5, §5.6, §7, §8.2, §10, §16.1, §16.2) dan **menunggu pemeriksaan ulang** menurut O-20: satu putaran review, temuan memblokir diperbaiki dan diverifikasi terhadap kode yang dikutip, dicatat "pending review" di ledger, tanpa putaran ketiga. Koreksi ditulis tanpa build atau jalan. Butir non-blocking di bawah dicatat dan **tidak** diterapkan. Dua keputusan pemilik di dokumen ini (D-11, D-13) dan satu di W10 (D-5) tetap terbuka.

### Klaim yang diperiksa di kode

Benar menurut pemeriksa:

- `AppModel` adalah satu kelas `@Observable` tanpa extension. 18 anggota `private` yang harus menjadi internal cocok dengan kode (kini 3506 baris dengan 6b; `restoreBase` baru dan hanya dipakai `clearSearch` dan `clearSort` di `+Run`).
- Skema DBeaver membiarkan `stop`, `saveFile`, `closeTab`, dan `openQuickly` tanpa kunci. Skema QueryHive mengikat ⌘E dua kali.
- Semua command dipasang di scene Settings, dan `applicationShouldTerminateAfterLastWindowClosed` mengembalikan `true`.
- `destructiveRequest` hanya bertanya di `confirm`. `RunConfirmationSheet` tidak punya `cancelAction` maupun `defaultAction`.
- `SidebarRenderTests` memakai `ImageRenderer`. `sceneBridgingOptions` ada di swiftinterface SDK.

Salah atau tidak lengkap, **memblokir** (diterapkan):

1. **Kepemilikan berkas: usulan perubahan §5 dan §7 meninggalkan berkas yang disunting tugas, dan itu melanggar aturan satu pemilik (`development-plan.md` §7).**
   - W9-T1 menambah flag snapshot `--reduce-motion`, `--reduce-transparency`, `--increase-contrast` (§4.1) dan W9-T7 menambah `--grid-font` (P-7b). Keduanya menyunting `Support/Snapshot.swift`, tetapi tidak ada yang mencantumkannya, dan rantai usulan hanya W6-T1 → W9-T4 → W9-T8. **Diterapkan:** rantai W6-T1 → W9-T1 → W9-T4 → W9-T7 → W9-T8, berkas ditambahkan ke daftar T1 dan T7, dan §16.1 langkah 4 menetapkan T7 mendarat sebelum T8 untuk berkas itu (urutan T7 sebelum T8 dipilih penerap koreksi: T7 hanya menambah satu flag, dan T8 bisa terblokir oleh probe P-8a dan P-8b).
   - W9-T2 menambah `var navigation` ke `Models/AppModel.swift` (§5.6), tetapi daftar T2 di §16.2 dan semua rantai tidak memuatnya; rantai `AppModel.swift` di `development-plan.md` §7 berhenti di W9-T0. **Diterapkan:** T2 `+ Models/AppModel.swift` (satu baris) dan baris rantai `... W6-T1 → W9-T0 → W9-T2`.
   - W9-T5 mengubah pemanggil `requestTableOperation` di `AppModel+Tree.swift` (§8.2), berkas milik T3 yang tidak ada di daftar T5. **Diterapkan:** `destructiveRequest` dan `Request.confirmTitle` diberi nilai bawaan (`confirmTitle` nil diturunkan dari `title`), jadi T5 tidak menyunting `+Tree`.
   - W10-T7 menambah `AppModel.adjustFontSize` (blueprint W10 §9.2), tetapi tidak ada extension `AppModel+` di daftar T7, dan rantai `AppModel+Focus.swift` (W9-T2 → W9-T6 → W12-T2) tidak memuat W10-T7. **Diterapkan:** baris rantai W9-T2 → W9-T6 → W10-T7 → W12-T2 di §16.2 (dan di W10 §12.4, dengan bunyi yang sama).

Diperiksa ulang oleh penerap koreksi dengan membaca kode (tanpa build): `RunConfirmation.destructiveRequest(for:title:safeMode:)` dan `Request` (`Models/RunConfirmation.swift` :57-65 dan struct di atasnya); `requestTableOperation` memanggil `destructiveRequest(for:title: "\(operation.title)?", safeMode:)` (`Models/AppModel.swift` ≈ :2032-2044), dan `TableOperation.title` adalah "Truncate Table" dan "Drop Table" (`RunConfirmation.swift` :292-297); Snapshot mem-parse flag di `Support/Snapshot.swift` :56-67; scene grid dan chrome didefinisikan sebagai `case "..."` di berkas yang sama (:340-920).

### Keputusan atas pertanyaan di "Untuk pemeriksa"

1. **D-1 dan D-2:** sembilan extension dan properti tersimpan di inti diterima. Satu-satunya sentuhan sesudah T0 pada inti adalah `var navigation` dari T2, dan kepemilikannya kini dinyatakan (butir memblokir di atas).
2. **D-4 sampai D-6:** lihat butir non-blocking (⌘⌫, urutan T1 dan T2, klaim D-3).
3. **D-7:** tidak ada temuan tercatat di putaran ini.
4. **D-11 (Truncate dan Drop bertanya juga di `full`):** **terbuka untuk pemilik**, ditandai dengan benar. Ia mengubah kontrak yang diuji (`RunConfirmationTests.testADestructiveOperationOnlyAsksAtConfirm`) dan doc comment `requestTableOperation` (`AppModel.swift:2024-2031`).
5. **D-13 (banner di atas badan kosong):** **terbuka untuk pemilik**, ditandai dengan benar. Ini penyimpangan dari FR-RUN-06 "di atas hasil sebelumnya".
6. Pertanyaan 6, 7, dan 8 (D-17, D-18 dan D-19, dan kelengkapan §13 dan §14): tidak ada temuan memblokir. Hitungan di D-18 punya butir non-blocking.

### Perubahan rencana yang dibutuhkan (untuk orkestrator)

**Memblokir, diterapkan.** Daftar di §16.2 sudah dikoreksi dan menjadi versi yang disalin ke `development-plan.md` §5 (daftar berkas W9-T1, T2, T5, T7) dan §7 (baris `Models/AppModel.swift`, `Support/Snapshot.swift`, `Models/AppModel+Focus.swift`) **sebelum implementer pertama dikirim**, sesudah pemeriksaan ulang. Rantai `AppModel+Focus.swift` harus sama dengan yang ada di W10 §12.4.

### Risiko terbuka (tidak memblokir)

Dicatat dan **belum diterapkan**:

- **§5.2:** mengikat `deleteRow` ke kunci menu tabel platform ⌘⌫ mengambil kunci itu dari editor SQL: kunci menu menyala sebelum `deleteToBeginningOfLine:` milik `NSTextView`. Daftar kunci lokal editor dan daftar kunci sistem di `ShortcutConflictTests` hanya memuat penangan editor sendiri, bukan ikatan sistem teks, jadi tes tidak akan menangkapnya. Simpan ⌘⌫ lokal di `GridKeyMap`, atau tambahkan ikatan standar `NSTextView` (⌘⌫, ⌥⌫, ⌘←/→, ⌃A/E/K, dan seterusnya) ke gabungan yang diperiksa tes.
- **D-18 dan §12.2:** "30 baseline grid" salah. `__Baselines__` berisi 16 scene grid × 2 appearance = 32 PNG, ditambah 8 scene editor × 2 = 16 PNG (dihitung ulang 6 Okt 2026; Ringkasan, §3.4, dan §15.2 sudah menyebut 32+16 dan 48). Perbaiki hitungan supaya daftar rekam ulang V-9 bisa dicek.
- **§16.1:** T1 dan T2 berjalan paralel, tetapi T1 menulis ulang `Workspace.swift:99` dan `:215` dengan `model.keyHint(for:)`, yang disediakan T2. Urutkan T1 sesudah T2 untuk dua baris itu, atau biarkan T1 memakai literal sampai T9. `ShortcutConflictTests` no. 8 (tabel Keyboard di Settings hanya memuat aksi yang tersedia) milik T2, tetapi perubahan Settings ada di T7, jadi tes itu gagal di gate T2; pindahkan ke T7.
- **D-3:** klaim bahwa tugas berikutnya menambah aksi menu tanpa menyentuh `App.swift` tidak benar: W10-T3 (`CommandGroup(replacing: .saveItem)`) dan W10-T6 (`Event.position`, `App.swift:164`) sama-sama menyunting `App.swift`. Arahkan Save lewat `AppMenu`, atau hapus klaimnya. Belum diketahui apakah `.saveItem` juga memegang item Close ⌘W sistem; uji bersama §17.2 butir 1.
- **§5.5 dan W10 D-6:** `currentRegion` dihitung saat ditanya dan tidak teramati, jadi setiap `AppMenu.isEnabled` yang membacanya akan basi sesudah fokus pindah. Aturan untuk `isEnabled` harus tertulis di §5.1: hanya keadaan teramati.
- **Keputusan pemilik yang masih terbuka:** D-11 dan D-13 (di atas), dan D-5 di W10. W10-T2 tidak boleh mulai sebelum D-5 dijawab.

Ditemukan saat menerapkan koreksi, di luar daftar pemeriksa, **tidak diterapkan** (untuk pemeriksaan ulang):

- Doc comment `requestTableOperation` (`AppModel.swift` ≈ :2024-2031, pindah ke `AppModel+Tree.swift` di T0) menulis kontrak yang diubah D-11 ("at `full` they run without asking"). Bila ia harus diperbarui, T5 menyentuh berkas milik T3 lagi. Usul: T3 menulis ulang komentar itu supaya menunjuk ke doc `RunConfirmation.destructiveRequest` dan tidak mengulang kontraknya; T5 mengubah doc `destructiveRequest` di berkasnya sendiri.
- §15.2 menyebut scene baru `error-banner` (T5) tanpa menyebut tempat didefinisikannya. Bila ia dibuat sebagai scene `--snapshot` seperti `badges` di T4, T5 masuk rantai `Support/Snapshot.swift`.
