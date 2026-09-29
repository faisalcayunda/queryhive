# Settings TablePro, dipetakan ke QueryHive

> Studi pendamping [`tablepro-adoption-plan.md`](tablepro-adoption-plan.md) dan
> [`tablepro-feature-analysis.md`](tablepro-feature-analysis.md). Dokumen ini hanya memetakan
> **dua belas pane Settings TablePro** ke apa yang QueryHive sudah punya, memisahkan *setting yang
> hilang* dari *fitur yang hilang*, dan menyebut di mana rencana yang sudah ada memutuskan
> pertanyaannya.
>
> **Target:** `/Users/isal/Workspaces/Lab/Experiments/TablePro`, HEAD `8fd32c80b`.
> **Pohon output:** `/Users/isal/Workspaces/Lab/Experiments/query_hive`.
> **Tanggal:** 29 Sep 2026. Ditulis read-only; tidak ada berkas lain yang disentuh.
>
> **Batas lisensi.** TablePro AGPL-3.0, QueryHive MIT (`docs/decisions/0002-keep-mit-license.md`).
> Semua nama setting, default, dan kunci penyimpanan di bawah ditulis sebagai **prosa**, bukan
> kutipan kode. Tempat yang tidak bisa dipisahkan dari menyalin kode ditandai **out of bounds**.
>
> **Catatan pohon yang bergerak.** Saat studi ini berjalan, HEAD QueryHive sempat berpindah dari
> `3deda4a` ke `8364e29` (merge "settings-shell"), dan `Views/SettingsView.swift` berubah dari empat
> pane menjadi lima: **General, Appearance, Editor, Data, Keyboard**. Semua temuan QueryHive di bawah
> mengacu ke keadaan `8364e291fe3d37298adff2ac3ce7a8c2f5b22455`, commit `8364e29`. Kalau HEAD sudah
> maju lagi, periksa ulang §1 (General) dan §3 (Editor) lebih dulu, karena keduanya yang baru berubah.

## 0. Cara membaca, dan apa yang dibaca

**Metode.** `grep`/`glob`, bukan membaca seluruh direktori. Dua berkas menjawab "apa yang bisa
diatur" di sisi TablePro: `TablePro/Models/Settings/` (model per kategori) dan
`Core/Storage/Preferences/PreferenceKeys.swift` (kunci di luar blob JSON). Sisi QueryHive dijawab
`Views/SettingsView.swift`, `Support/ThemeStore.swift`, dan `Models/AppModel.swift`.

**Yang dibaca di TablePro.** `Views/Settings/SettingsView.swift` (enum dua belas pane),
`SettingsWindowController.swift` (bar sebagai `NSToolbar`/`NSTabViewController` dengan
`tabStyle = .toolbar`), seluruh `*SettingsView.swift` dan `Sections/*.swift`, model di
`Models/Settings/`, `Core/Storage/AppSettingsStorage.swift`, `Core/Storage/Preferences/PreferenceKeys.swift`,
`Core/Storage/AppSettingsManager.swift`, `Models/UI/KeyboardShortcutModels.swift` (sebagian),
`Models/AI/AIModels.swift` (`AISettings`).

**Tiga akar di sisi TablePro yang perlu dibaca sekali supaya istilahnya konsisten.** Pertama, tiap
kategori disimpan sebagai **satu blob JSON** di `UserDefaults` dengan kunci `com.TablePro.settings.<kategori>`,
bukan satu kunci per setting. Kedua, "reset semua" (`GeneralSettingsView`) memanggil satu fungsi yang
menyimpan ulang tiap `.default`. Ketiga, pane **Data** menyimpan satu setting milik Editor
(`jsonViewerPreferredMode` hidup di `EditorSettings`, bukan `DataGridSettings`) — jadi "pane"
dan "model" tidak satu banding satu.

---

## 1. General

### Apa yang diatur

Dua belas kontrol plus tiga section berbasis berkas. **General** ([`GeneralSettings`]) menyimpan
`startupBehavior` (default **reopenLast**; alternatif "Show Welcome Screen"), `language`
(default **system**; Inggris, Vietnam, Cina sederhana/tradisional, Korea, Turki), `queryTimeoutSeconds`
(default **60**, `0` berarti tanpa batas), `shareAnalytics` (default **true**), dan delapan preferensi
tampilan: `showRecentTables` (false), `showObjectComments` (true), `showObjectIcons` (true),
`showSystemContainers` (false), `showPartitions` (true), `showWorkspaceRail` (true), `sidebarRowSize`
(matchSystem), `connectionHealthCheck` (every30Seconds). Semuanya di blob `com.TablePro.settings.general`.

Di luar blob itu: **Tabs** ([`TabSettings`], `com.TablePro.settings.tabs`) menyimpan
`enablePreviewTabs` (true) dan `overflow` (scroll). **Connections** menyimpan
`com.TablePro.connectionList.showsRecent` (`ConnectionListPreferences`, default false) plus tombol
"Clear Recent" atas ledger `com.TablePro.connectionList.recentConnections`. **Default layout** untuk
koneksi baru disimpan lewat `@AppStorage` di `SidebarPersistenceKey.defaultLayout`, bukan di
`GeneralSettings`. **Command Line** memasang/mencopot perintah `tablepro` (tanpa setelan tersimpan).
**Linked Folders** mendaftar folder yang dipantau untuk berkas `.tablepro` (`com.TablePro.linkedFolders`
dan `com.TablePro.linkedSQLFolders`) dengan sakelar per folder. **Trusted Links** mencabut entri
`ExternalConnectionTrustStore` (URL yang selalu boleh membuka koneksi). **Software Update** membaca
langsung properti Sparkle — `automaticallyChecksForUpdates` dan `automaticallyDownloadsUpdates` — jadi
tidak ada salinan di `GeneralSettings`; di bawahnya ada tanggal cek terakhir, tombol "Check for
Updates…", dan tautan "What's New". Terakhir, tombol reset seluruh setelan.

### Yang sudah ada di QueryHive

Pohon ini baru saja menambah pane **General** sendiri
(`app/Sources/TrinoExporter/Views/SettingsView.swift`, `struct GeneralSettings`): kartu *Software
update* dengan `SoftwareUpdateRow`/`LiveSoftwareUpdateRow` yang membaca `Updater`
(`Support/Updater.swift`, pembungkus tipis `SPUStandardUpdaterController`) dan tombol "Check for
Updates…", plus kartu *Reset settings* yang memanggil `ThemeStore.reset()`. Reset itu hanya
mengembalikan appearance dan dua font — bukan riwayat, bukan timeout.

**Query timeout sudah ada, tetapi di pane Data:** `AppModel.statementTimeoutMS`
(`Models/AppModel.swift`, kunci `statementTimeoutMS`, default 60.000 ms). Session restore (Fase 1)
selalu hidup lewat `AppModel(persistsSession:)`, tanpa sakelar; skema shortcut memakai
`AppModel.shortcutScheme` (kunci `shortcutScheme`, default `dbeaver`). Per-connection
`showAllSchemas`/`showAllDatabases` ada di editor koneksi (`Models/Connections.swift`), bukan di
Settings global. Tidak ada: bahasa, startup/welcome, preview tab, workspace rail, recent tables,
ikon/komentar objek, row size sidebar, layout default, health check, command-line tool, linked
folders, trusted links, analytics, `automaticallyChecksForUpdates`/`automaticallyDownloadsUpdates`
sebagai kontrol.

### Celah

- **Setting hilang untuk fitur yang ada (murah):** sakelar kelahiran-session (reopen vs welcome) —
  mesinnya ada; sakelar auto-check/auto-download update — Sparkle sudah memilikinya, `Updater`
  hanya belum meneruskannya; versi `queryTimeoutSeconds` sudah ada di pane lain dan tinggal
  dipindah/dipertahankan.
- **Fitur hilang (mahal):** command-line tool (`tablepro` setara), linked folders (pengawas folder +
  format berkas koneksi), trusted external links (URL scheme/AppleScript), workspace rail, health
  check berkala, telemetry/analytics, pemilih bahasa, row size/gutter sidebar, recent tables,
  toggle ikon/komentar/partisi/system containers. Masing-masing bukan "kontrol plus tempat
  menyimpan", melainkan subsistem.

### Ongkos

Otak-otak paling murah: pindahkan `statementTimeoutMS` ke General (atau biarkan di Data) dan
tambahkan tiga sakelar Sparkle/session — beberapa jam, satu berkas UI plus `Updater`. Yang mahal
adalah delapan fitur terakhir di paragraf sebelumnya: linked folders butuh format berkas dan
pengawas; command-line tool butuh installer dan helper; trusted links butuh URL scheme dan
penyimpanan trust. Health check dan workspace rail adalah fitur UI/loop tersendiri.

### Diputuskan?

Tidak ada keputusan khusus untuk pane ini. Yang menyentuh: matriks analisis §5 baris
**Settings** menulis "5 tab → 2 tab → *Tambah tab Data saat grid dan history mendarat*", dan itu
sudah mendarat; komentar kelas `SettingsView` di `8364e29` menyatakan sendiri bahwa hanya pane yang
bisa diisi yang dimasukkan. `AI/Integrations/Plugins/Sync/License` ditolak §9. Selebihnya belum
diputuskan — jangan perlakukan "belum ada" di sini sebagai keputusan.

---

## 2. Appearance

### Apa yang diatur

[`AppearanceSettings`] (`com.TablePro.settings.appearance`) menyimpan `appearanceMode` (default
**auto**, pilihan light/dark/auto), `preferredLightThemeId` (default `tablepro.default-light`) dan
`preferredDarkThemeId` (default `tablepro.default-dark`). Pane-nya menampilkan daftar tema per
slot light/dark plus **editor tema**: `ThemeEditorColorsSection` dan `ThemeEditorFontsSection`
mengedit warna dan font sebuah tema, dan `ThemePreviewCard` menggambar pratinjau grid + editor.
Dengan kata lain, tema di TablePro bukan enum tetap — tema adalah entitas yang bisa dibuat dan
disunting, disimpan lewat `ThemeEngine`/`ThemeRegistryInstaller`.

### Yang sudah ada di QueryHive

Pane **Appearance** ada dan isinya setara secara permukaan, hanya sumbernya `ThemeStore`
(`Support/ThemeStore.swift`): `mode` (kunci `appearanceMode`, default `.system`), `darkTheme`
(`appTheme`, default `.midnight`) dan `lightTheme` (`lightTheme`, default `.daylight`), `accent`
(`accentChoice`, default `.ice`), `tone` (`surfaceTone`, default `.glow`), `glow`
(`glowIntensity`, default 1.0), plus tiga preset (`ThemePreset`). Tema di sini **enum tetap**
(`AppTheme`: midnight/graphite/nord/ink/daylight/cloud/paper); accent dan tone juga enum.

### Celah

Tidak ada setting hilang yang berarti: model dua-slot light/dark, mode, accent, tone, glow, dan
preset semuanya sudah ada. Yang hilang adalah **fitur** editor tema — membuat tema sendiri dan
menyunting warnanya. TablePro melakukannya; QueryHive sengaja memperlakukan tema sebagai enum.

### Ongkos

Membangun editor tema berarti memindahkan palet dari enum statis ke data tersimpan, menambah format
tema (warna editor + grid), validasi kontras, dan UI editor. Itu proyek kecil-menengah, bukan
setelan. Menyalin format/berkas tema TablePro adalah **out of bounds**; bentuknya harus ditemukan
sendiri. Tidak ada di rencana sebagai irisan.

### Diputuskan?

Tidak ada di §8/§9 maupun §5. QueryHive sudah punya pane-nya; editor tema belum diputuskan dan tidak
mendesak.

---

## 3. Editor

### Apa yang diatur

[`EditorSettings`] (`com.TablePro.settings.editor`) adalah meja kerja SQL. `showLineNumbers`
(true), `highlightCurrentLine` (true), `highlightCurrentStatement` (true), `wordWrap` (false),
`codeFoldingEnabled` (true), `showStatementRunControls` (true — tombol Run di samping tiap
statement), `showInvisibleCharacters` (true), `tabWidth` (default **4**, pilihan 2/4/8,
di-clamp 1–16), `keywordCase` (default; ada juga legasi `uppercaseKeywords` di kunci wire),
`queryParametersEnabled` (true — sintaks `:name`), `vimModeEnabled` (false), dan
`jsonViewerPreferredMode` (default text; alternatif tree) yang **tampil di pane Data** meski tinggal
di model ini. Pane-nya plus `AppearanceThemeEditSlot` menyediakan font editor lewat editor tema.

### Yang sudah ada di QueryHive

Pane **Editor** baru berisi **font saja**: `ThemeStore.uiFontFamily` (`uiFontFamily`) dan
`codeFontFamily` (`codeFontFamily`), dengan pratinjau keluarga. Komentar kelasnya menulis sendiri
bahwa ini tempat preferensi editor akan pergi kalau app tumbuh. Fitur editornya sendiri ada tetapi
tidak bisa diatur: `SQLEditor.swift` selalu menggambar gutter nomor baris dan marka lipatan
(`SQLFolding`), find/replace (`EditorFind.swift`, `SQLFindBar.swift`) punya perilaku yang sekarang
tetap, parameter binding ada di engine (gelombang 3). `QueryTab.rowLimit` (default 1000),
`nullText`, `format`, `delimiter` adalah setelan **per tab**, bukan setelan editor/grid global.

### Celah

- **Setting hilang untuk fitur yang ada (murah):** line numbers (gutter selalu ada), folding
  (selalu ada), find case/whole-word/regex (sudah ada sebagai perilaku, belum sebagai default),
  `queryParametersEnabled` (binding sudah ada), `keywordCase` (fun formatting sudah ada),
  `showInvisibleCharacters`, word wrap, `tabWidth`.
- **Fitur hilang (mahal):** vim mode (analisis §5 menandainya "Tunda, besar dan opinian"),
  `showStatementRunControls` (tombol Run inline per statement belum ada), `highlightCurrentStatement`
  sebagai preferensi (perilaku highlight ada? belum tentu — periksa di `SQLEditor`), `jsonViewerPreferredMode`
  (default viewer JSON sudah ada sebagai mode, jadi ini setting, bukan fitur).

### Ongkos

Mengisi pane Editor dengan sakelar yang menggerakkan `NSTextView`/gutter adalah pekerjaan murah dan
terlokal: tiap setting adalah satu properti di editor + satu kontrol. Vim mode mahal dan sudah
dinyatakan ditunda. Tombol Run per-statement menengah (butuh menggambar kontrol di gutter/overlay).

### Diputuskan?

Analisis §5 memutuskan per item: find/replace **ambil**; folding **ambil**; parameter `:name`
**tunda, butuh jalur prepared statement** — sekarang sudah mendarat sebagai gelombang 3, jadi
"tunda"-nya sudah kedaluwarsa untuk binding, tinggal preferensinya; vim mode **tunda, besar dan
opinionasi**. §8/§9 tidak menyebut pane ini.

---

## 4. Data

### Apa yang diatur

Pane Data menggabungkan tiga model. [`DataGridSettings`] (`com.TablePro.settings.dataGrid`):
`rowHeight` (default normal/24; compact 20, normal 24, comfortable 28, spacious 32), `dateFormat`
(iso8601), `nullDisplay` (string "NULL", maks 20 karakter, gagal validasi → "NULL"), `defaultPageSize`
(1000, rentang 10–100.000), `showAlternateRows` (true), `showRowNumbers` (true), `autoShowInspector`
(false), `enableSmartValueDetection` (true), `countRowsIfEstimateLessThan` (100.000, opsi "Always
count" = `Int.max`), `queryResultRowCap` (10.000, rentang 100–500.000; `0` tak terbatas),
`truncateQueryResults` (true), `defaultSortBehavior` (none; alternatif primary key / first column),
`defaultSortDirection` (ascending). [`HistorySettings`] (`com.TablePro.settings.history`):
`maxEntries` (10.000, 0 tak terbatas), `maxDays` (90, 0 selamanya), `autoCleanup` (true), dan
`keepRewindHistory` (true — satu-satunya setelan yang menyimpan nilai baris nyata ke disk). Ditambah
"Clear History" dan "Clear Saved Changes". Pane juga memuat `jsonViewerPreferredMode` (text/tree,
milik `EditorSettings`).

### Yang sudah ada di QueryHive

Pengaturan yang ada di pane **Data** baru: `AppModel.recordsHistory` (kunci `recordsHistory`,
default true) dan `AppModel.historyLimit` (kunci `historyLimit`, default 200, UI 50–5000). Grid
punya fitur yang setara dengan banyak setting di atas, tetapi nilainya tetap di kode atau per tab:
tinggi baris tetap 25 (`ResultGrid.swift`), gutter nomor baris selalu digambar, NULL digambar kosong
miring-redup (bukan string yang bisa diatur), `QueryTab.rowLimit` (default 1000) diatur di header
grid, `GridSort` ada tetapi per tab tanpa default, `CellValueViewer` punya mode teks/JSON/tree/hex,
dan format tampilan per kolom disimpan `ColumnFormatStore` (kunci `columnFormats`, per
koneksi+tabel+kolom). Preset filter disimpan `FilterPresetStore` (kunci `filterPresets`). NULL untuk
ekspor punya `QueryTab.nullText` (default kosong). Tidak ada: tinggi baris, format tanggal global,
string NULL tampilan, page size default, baris berselang, auto-show inspector, smart value
detection, ambang estimasi count, row cap global, default sort, retensi `maxDays`/`autoCleanup`,
Rewind.

### Celah

- **Setting hilang untuk fitur yang ada (murah):** tinggi baris; toggle nomor baris; string NULL
  tampilan; default sort behavior + arah; JSON viewer default mode; default `rowLimit` (per tab,
  tinggal tambah default); ambang count (perintah `countRows` ada); retensi riwayat
  (`maxDays`/`autoCleanup` — engine history bisa dibatasi).
- **Fitur hilang (mahal):** Data Rewind (`keepRewindHistory` — *snapshot* baris terenkripsi, 7 hari,
  senyap), inspector baris auto, smart value detection, page size/paginasi eksplisit, format
  tanggal global (sekarang `ColumnFormat` per kolom, bukan default global).

### Ongkos

Sembilan setting "murah" di atas adalah pekerjaan sehari sampai beberapa hari: menyimpan di
`UserDefaults` dan mengalirkannya ke `ResultGrid`/`GridSort`/`CellValueViewer`. Data Rewind adalah
fitur tersendiri (skema penyimpanan + enkripsi + clear), dan TablePro menjadikannya Pro-gated.

### Diputuskan?

Analisis §5 memiliki baris untuk **Grid: sort** ("Ambil, biaya kecil" — sudah mendarat),
**Cell viewer** ("Ambil untuk JSON, sisanya tunda" — sudah mendarat gelombang 1), **Favorites**
(sudah mendarat), dan **Grid: chart/map** ("Tolak, keluar dari lingkup"). §12.1.5 mencatat sort
server untuk hasil berhalaman masih tertunda. Tidak ada baris untuk tinggi baris/NULL/default sort;
jadi setting-setting itu belum diputuskan, tetapi tidak ada yang menolaknya.

---

## 5. Keyboard

### Apa yang diatur

[`KeyboardSettings`] (`com.TablePro.settings.keyboard`) menyimpan **satu peta** aksi→binding
(`shortcuts`), tapi yang membuatnya hidup adalah mesin di sekelilingnya: `ShortcutAction` (kategori
editor, data grid, navigasi, connections, app — puluhan aksi), `ShortcutConflictResolver`,
`SystemHotkeyChecker`, panel perekam (`ShortcutRecorderView`), aturan "butuh ⌘/⌃", dan
`MainMenuBuilder`/`MainMenuKeyEquivalentSync` yang menulis ulang key-equivalent menu saat binding
berubah. Pane-nya punya pencarian, reset per aksi, dan tiga jenis peringatan (konflik dengan aksi
lain, sistem-reserved, reserved-tertentu).

### Yang sudah ada di QueryHive

`AppModel.shortcutScheme` (kunci `shortcutScheme`, default `dbeaver`) dan dua skema tetap,
`ShortcutScheme.dbeaver` / `.queryhive` (`Models/Shortcuts.swift`), dengan tabel per aksi
(`ShortcutAction` 13 aksi) dan tampilan tabel di pane Keyboard. **Tidak ada rebound**: skema adalah
pilihan, bukan binding yang bisa diubah; tidak ada penyimpanan binding kustom, tidak ada deteksi
konflik.

### Celah

**Fitur hilang (menengah):** binding kustom. Bukan sekadar kontrol-plus-penyimpanan, karena menu
AppKit harus disinkronkan ulang tiap kali binding berubah (TablePro punya enumerator khusus untuk
itu). Yang "murah" adalah memperbanyak skema; yang mahal adalah mengizinkan pengguna mengubah satu
binding.

### Ongkos

Menambah binding kustom = penyimpanan peta aksi→(key, modifier), UI perekam, aturan modifier wajib,
deteksi konflik, dan penulisan ulang menu. Ini fitur orde "state plus integrasi menu", bukan setelan.
Menambah skema ketiga hanya tabel data.

### Diputuskan?

Tidak disebut di §8/§9 atau §5. Pane Keyboard QueryHive sudah ada; rebound belum diputuskan.

---

## 6. Profiles

### Apa yang diatur

Dua daftar yang tidak hidup di `UserDefaults` melainkan di penyimpanan sendiri.
**Credential Profiles** (`CredentialProfileStorage`, editor `CredentialProfileEditorSheet`) menyimpan
nama, username, `passwordMode`, dan daftar secure field; satu profil dipakai banyak koneksi, dan
pane menampilkan "Used by N". **SSH Servers** (`SSHProfileStorage`, `SSHProfileEditorView`)
menyimpan nama, host, username untuk satu bastion yang dipakai banyak koneksi. Keduanya bisa
tambah/sunting/duplikat, dan kedua notifikasi perubahan (`credentialProfilesDidChange`,
`sshProfilesDidChange`) menyegarkan daftar.

### Yang sudah ada di QueryHive

**Tidak ada.** Kredensial disimpan per koneksi (Keychain lewat `qh-credentials`), bukan sebagai
profil bersama. Tidak ada SSH tunnel sama sekali; `NavicatImport` bahkan mencatat entri ber-SSH
sebagai "this app doesn't open". Safe Mode dan TLS diatur per koneksi di editor koneksi
(`Models/Connections.swift`), bukan di Settings.

### Celah

**Fitur hilang (mahal):** konsep profil bersama. Untuk kredensial, ini berarti model profil +
rujukan dari koneksi + UI; untuk SSH, ini berarti **membangun tunneling lebih dulu** (analisis §5:
"Ambil SOCKS5 dan tunnel command bila ada permintaan nyata"). Tanpa tunneling, setengah pane ini
tidak punya isi.

### Ongkos

Credential profiles menengah: skema, penyimpanan aman, rujukan koneksi, UI, dan hitungan
pemakaian. SSH profiles menunggu keputusan transport; harganya satu proyek transport + profil.
Menyalin bentuk `SSHProfile`/tunnel TablePro adalah **out of bounds**.

### Diputuskan?

Analisis §5 menulis transport TablePro "SSH, SOCKS5, cloudflared, Cloud SQL proxy, tunnel command,
IAM, profile" vs QueryHive "SSH + TLS", putusan **"Ambil SOCKS5 dan tunnel command bila ada
permintaan nyata"**. Jadi tunnel (dan karenanya profil SSH) adalah *deferred, needs demand*, bukan
ditolak. Profil kredensial bersama tidak disebut.

---

## 7. Notifications

### Apa yang diatur

[`NotificationSettings`] (`com.TablePro.settings.notifications`): `isEnabled` (default true),
`thresholdSeconds` (default 20, rentang 5–600, UI 5 detik sampai 10 menit), dan `disabledKindIds`
(disimpan sebagai **yang dimatikan**, bukan yang dinyalakan, supaya jenis baru dari rilis berikutnya
langsung aktif). Jenisnya adalah `TrackedOperationKind`: query, batch statement, row edit,
perubahan struktur, impor, ekspor, copy objek, backup, fetch all, query AI/MCP, query AppleScript.
Pane menampilkan status izin notifikasi sistem dan tombol "Open System Settings".
`NotificationAuthorization` meminta izin, dan layout klasik macOS "hanya saat hasil tidak ada di
layar".

### Yang sudah ada di QueryHive

**Tidak ada.** Tidak ada `UserNotifications`, tidak ada `TrackedOperationKind`, tidak ada
pemberitahuan penyelesaian pekerjaan panjang.

### Celah

**Fitur hilang (mahal-menengah):** seluruh subsistem notifikasi — kategori izin OS, pelacakan
operasi panjang (di app ini: query, ekspor, impor, `apply_changes`), ambang, dan jenis yang bisa
dimatikan. Ini bukan setelan; tidak ada yang bisa diatur sebelum ada yang memberi tahu.

### Ongkos

Menambah loop pelacakan operasi + izin + UI adalah pekerjaan menengah dan menyentuh setiap jalur
eksekusi (preview, export, apply, import). Tidak ada di rencana sebagai irisan.

### Diputuskan?

Tidak disebut di §8/§9 maupun §5. Pane ini salah satu dari dua yang sama sekali belum diputuskan.

---

## 8. AI

### Apa yang diatur

[`AISettings`] (`com.TablePro.settings.ai`) adalah model terbesar: `enabled` (true),
`providers` (daftar `AIProviderConfig` — id, nama, tipe dari 14 provider, model, endpoint,
maxOutputTokens, telemetry, reasoningEffort), `activeProviderID`, `inlineSuggestionsEnabled`
(false), `inlineSuggestionDebounceMs` (500, rentang 100–3000), `maxToolRoundtripsEnabled` (true),
`maxToolRoundtrips` (25, rentang 5–200), `includeSchema`/`includeCurrentQuery`
(true) / `includeQueryResults` (false), `maxSchemaTables` (20), `defaultConnectionPolicy`
(askEachTime; alternatif alwaysAllow/never), `chatMode` (ask/edit/agent), `composerHighlightEnabled`
(true). Plus **Custom Slash Commands** (template dengan `{{query}}`, `{{schema}}`, `{{database}}`,
`{{body}}`), kunci API per provider (`AIKeyStorage`), dan status OAuth tiap provider. Provider
mencakup cloud (Claude/ChatGPT/Gemini/xAI/Cursor), lokal (Ollama, llama.cpp, MLX), dan agen.

### Yang sudah ada di QueryHive

**Tidak ada.** Tidak ada provider, chat, inline suggestion, atau slash command.

### Celah

**Fitur hilang (proyek, bukan irisan):** seluruh AI assistant. Pane-nya pun tidak bisa dibangun
sebelum providernya ada.

### Ongkos

Kelas proyek: chat bertool-calling, inline suggestion, review/explain/optimize, agent mode, dan
belasan provider. Menyalin registry provider atau pipeline telemetri TablePro adalah **out of
bounds**; yang bisa dipakai hanya pola produknya.

### Diputuskan?

Ditolak? **Tidak — ditunda dengan syarat, dan §8 mengatakannya eksplisit.** Kutipannya: AI assistant
"ada di sini dan bukan di §9"; "Matriks analisis menandainya 'Tunda, dan putuskan dulu soal on-prem'
— bukan ditolak"; yang menahan "bukan kodenya melainkan satu keputusan kebijakan: mengirim skema,
nama tabel, dan query ke LLM cloud berarti data pengguna meninggalkan mesin, dan pengguna alat ini
bekerja dengan data pemerintah"; dan "pane AI di Settings baru masuk akal sesudahnya: pane itu
mengatur provider, dan provider belum ada." Jangan buka lagi; yang dibutuhkan keputusan produk
on-prem vs cloud, bukan pekerjaan setting.

---

## 9. Integrations (MCP)

### Apa yang diatur

[`MCPSettings`] (`com.TablePro.settings.mcp`): `enabled` (default false), `port` (23508),
`defaultRowLimit` (500), `maxRowLimit` (10.000), `queryTimeoutSeconds` (30), `logQueriesInHistory`
(true), `requireAuthentication` (true), `connectionApproval` (oncePerConnection). Pane-nya menambah:
daftar token (buat/cabut/hapus, nama, permission, allowlist koneksi, masa berlaku, prefix),
daftar **grant** koneksi (`MCPGrantListView`), **Outside MCP Servers** (`MCPServerStore`: satu
server MCP yang *dipanggil* sessinya, dengan endpoint, token, dan `allowedConnectionIds`), tombol
"Connect a Client…" dan "View Activity…", plus indikator status server. Ini permukaan keamanan:
token, scope, dan allowlist.

### Yang sudah ada di QueryHive

**Servernya ada, UI-nya tidak.** `queryhive-mcp` adalah binari ketiga `crates/qh-ffi` (plan §4),
protokol JSON-RPC stdio, sembilan tool read-only, `to_table` ditolak, token sebagai SHA-256 di tabel
`mcp_token` (migrasi `0005`, plus prefix di `0006`), scope + allowlist (ADR-0015). Tapi semuanya
dikelola dari CLI: `queryhive-mcp issue`, dan handshake lewat berkas. **Tidak ada** pane Settings,
tidak ada penyimpanan setelan app untuk port/limit/timeout/log, tidak ada daftar token/grant di app,
tidak ada outside servers, tidak ada activity log di app. Komentar kelas `SettingsView` (8364e29)
menyatakan ini sendiri: "MCP's token management is CLI-only today."

### Celah

- **Setting hilang (murah, kalau app mau memiliki konfigurasi server):** port, default/max row
  limit, query timeout, log-in-history, dan mungkin sakelar auth. Nilainya sudah ada di engine/CLI;
  yang hilang adalah tempat menyimpan di app dan kontrol yang menuliskannya. Tetapi perlu keputusan
  dulu: siapa yang memiliki konfigurasi server, app atau berkas/CLI.
- **Fitur hilang (mahal):** manajemen token/grant/approval di app. Ini bukan kontrol-plus-setelan,
  melainkan UI atas `mcp_token` dan allowlist, plus pertanyaan keamanan yang §12.3 sengaja tunda
  ("External Clients" menunggu MCP dipakai lebih dari satu orang). Outside MCP servers dan activity
  log adalah fitur tersendiri. Pairing PKCE adalah cetak biru, bukan pekerjaan setelan.

### Ongkos

Menampilkan port/limit/timeout sebagai setelan app: kalau app tidak memiliki lifecycle server,
setelan ini hanya dekoratif — jadi harganya termasuk memutuskan dan menyambungkan kepemilikan
konfigurasi. Manajemen token di app menengah-besar dan menyentuh permukaan keamanan yang sudah
punya ADR; kerjakan hanya kalau MCP dipakai banyak orang.

### Diputuskan?

Analisis §5: **MCP server** ("47 tool + resources + prompts" vs "tidak ada") → **"Ambil, ini
kandidat terkuat"**, dan §4 sudah mengeksekusinya (server, token, scope). Yang **belum** diputuskan
adalah UI pengelolaannya di app: §12.3 menunda "External Clients" sampai MCP dipakai lebih dari satu
orang, dan komentar `SettingsView` menyebut token management "CLI-only today". Jadi pane ini
sebagian sudah dieksekusi, sebagian ditunda dengan syarat, dan tidak ada yang ditolak.

---

## 10. Plugins

### Apa yang diatur

Dua sub-tab (Installed/Browse) di atas registry plugin: `InstalledPluginsView`,
`BrowsePluginsView`, `RegistryPluginDetailView`, `TrustedDevelopersView`, `PluginIconView`.
TablePro membayarnya dengan `TableProPluginKit`, versioning ABI, checker kompatibilitas, dan
registry manifest. Tidak ada setelan di `UserDefaults`; statusnya hidup di penyimpanan plugin.

### Yang sudah ada di QueryHive

**Tidak ada, dan sengaja.** Driver adalah crate compile-time; tidak ada ABI runtime.

### Celah

**Fitur hilang (refused):** registry + ABI. Menyalin bentuknya **out of bounds** (`repr(C)`,
versioning ABI, registry manifest adalah kode).

### Ongkos

Proyek besar, tidak sepadan untuk alat lab ini.

### Diputuskan?

**Ditolak.** Adopsi §9: "…plugin registry dan ABI runtime…". Analisis §5 baris **Plugins**:
"registry + ABI" vs "driver compile-time" → **"Tolak, ABI runtime bukan langkah berikutnya"**.
Jangan dibuka lagi.

---

## 11. Sync

### Apa yang diatur

[`SyncSettings`] (`com.TablePro.settings.sync`): `enabled` (default false), lalu kategori
`syncConnections`, `syncGroupsAndTags`, `syncSettings`, `syncSSHProfiles`, `syncCredentialProfiles`,
`syncTableFavorites`, `syncDatabaseFavorites`, `syncSQLFavorites` (semua true kecuali
`syncPasswords` false). Pane menampilkan status akun iCloud, tanggal sync terakhir, "Sync Now",
dan peringatan pause saat lisensi kedaluwarsa/tak terverifikasi. Gating Pro via
`ProFeature.iCloudSync`. Kategori `sync`/`mcp` **tidak ikut** disinkronkan (device-local). Sinkronnya
lewat iCloud/CloudKit.

### Yang sudah ada di QueryHive

**Tidak ada.** Tidak ada CloudKit, tidak ada akun.

### Celah

**Fitur hilang (refused).** Menyalin konfigurasi CloudKit TablePro adalah **out of bounds**.

### Ongkos

Fitur produk komersial, bukan alat lab.

### Diputuskan?

**Ditolak.** Adopsi §9: "…iCloud sync, app iOS, licensing dan team plan…". Analisis §5 baris
**iCloud sync** → **"Tolak"**; §7 juga menulis "iCloud sync, app iOS, licensing, team plan. Itu
fitur produk komersial, bukan fitur alat lab."

---

## 12. License

### Apa yang diatur

Pane License/License: aktivasi kunci (kunci + machine id/name, app version, OS version), status
(unlicensed/active/expired/suspended/deactivated/validationFailed), refresh, daftar perangkat
aktivasi (`LicenseDevicesSection`, deaktivasi perangkat lain, batas aktivasi), roster tim
(`LicenseTeamSection`, seat used/max) untuk lisensi Team, tautan billing, dan "Deactivate on This
Mac". Model `License` menyimpan payload tersandi RSA + signature server, dengan cache yang
mengikat ke machine id. Gating lewat `ProFeature` (iCloudSync, encryptedExport, envVarReferences,
linkedFolders, queryInsights, resultCharts, compareSync, dataRewind, teamCatalog, teamLibrary) dan
`LicenseTier`. Tidak ada setelan di `UserDefaults`.

### Yang sudah ada di QueryHive

**Tidak ada.** MIT, tanpa kunci, tanpa gating.

### Celah

**Fitur hilang (refused).** Skema signature/verifikasi adalah **out of bounds**.

### Ongkos

Fitur produk komersial.

### Diputuskan?

**Ditolak.** Adopsi §9 dan analisis §5 baris terakhir + §7: "licensing dan team plan" ditolak.

---

## 13. Tabel ringkas

Legenda: **S.ada?** = setelan sudah ada di QueryHive; **F.ada?** = fitur yang disetel sudah ada;
**Ongkos** = murah (kontrol + penyimpanan) / menengah / proyek; **Diputuskan** = kutipan lokasi.

| Pane | S.ada? | F.ada? | Ongkos | Diputuskan |
|---|---|---|---|---|
| General | Sebagian (timeout, session, skema, reset) | Sebagian (update, session) | murah-sebagian; 8 fitur = menengah/proyek | Belum; analisis §5 "tambah tab Data" sudah dijalankan; §9 menolak tetangga |
| Appearance | Ya, lengkap | Ya (tema enum) | editor tema = menengah | Belum |
| Editor | Font saja | Gutter/fold/find/param ada | setting murah; vim/Tombol Run = menengah | Analisis §5: find & fold *ambil*, param *tunda* (kini mendarat), vim *tunda* |
| Data | Sebagian (history) | Grid/sort/viewer/rowLimit ada | 9 setting murah; Rewind = menengah | Analisis §5: sort & viewer *ambil* (mendarat), chart *tolak*; §12.1.5 sort server tertunda |
| Keyboard | Skema saja | Tidak (tak ada rebound) | menengah (menu sync) | Belum |
| Profiles | Tidak | Tidak (SSH tak ada) | menengah; SSH = proyek | Analisis §5: SOCKS5/tunnel *"bila ada permintaan nyata"* |
| Notifications | Tidak | Tidak | menengah | Belum |
| AI | Tidak | Tidak | proyek | §8 **tunda**, butuh keputusan on-prem (bukan ditolak) |
| Integrations/MCP | Tidak (server ada) | Sebagian (CLI) | setting murah *setelah* kepemilikan disepakati; token UI = menengah | Analisis §5 *ambil*; §12.3 tunda UI lebih jauh; `SettingsView` "CLI-only today" |
| Plugins | Tidak | Tidak | proyek | §9 **tolak**; analisis §5 *tolak* |
| Sync | Tidak | Tidak | proyek | §9 **tolak**; analisis §5 *tolak* |
| License | Tidak | Tidak | proyek | §9 **tolak**; analisis §5/§7 *tolak* |

---

## 14. Daftar berperingkat

### A. Murni "setting hilang" (murah, masuk akal, fitur sudah ada)

1. **Data grid** — tinggi baris, toggle nomor baris, string NULL tampilan, default sort
   (behavior + arah), JSON viewer default mode, default `rowLimit`, ambang estimasi count, retensi
   riwayat. Fitur semuanya ada; nilainya sekarang tetap atau per tab.
2. **Editor** — line numbers, folding, opsi find (case/whole word/regex), `queryParametersEnabled`,
   `keywordCase`, `showInvisibleCharacters`, word wrap, `tabWidth`. Fitur editornya ada; yang hilang
   hanya kontrol dan tempat menyimpan.
3. **General** — sakelar auto-check/auto-download Sparkle di atas `Updater`, dan sakelar
   reopen-vs-welcome di atas session restore yang sudah ada.
4. **Integrations/MCP (separuh)** — port, row limit, timeout, log-in-history: murah **hanya kalau**
   kepemilikan konfigurasi server diputuskan dulu; tanpa itu kontrolnya dekoratif.
5. **One-off** — pindahkan/mirror `statementTimeoutMS` ke General bila pane Data dianggap bukan
   rumahnya (kosmetik, bukan fitur).

### B. Butuh fitur dibangun (menengah)

1. **Keyboard** — binding kustom + sinkronisasi menu + deteksi konflik (skema tetap sudah ada).
2. **Profiles (kredensial)** — model profil bersama + rujukan koneksi.
3. **Notifications** — pelacakan operasi panjang + izin + ambang.
4. **Editor/Data sisa** — vim mode (dinyatakan ditunda oleh analisis), tombol Run per statement.
5. **Data Rewind** — snapshot baris terenkripsi + clear.
6. **Integrations/MCP** — UI token/grant/approval (permukaan keamanan; §12.3 menahan lebih jauh).

### C. Sudah ditolak atau ditunda oleh rencana (jangan dibuka lagi)

- **Ditolak §9:** Plugins (registry + ABI runtime), Sync (iCloud), License (licensing + team plan),
  plus tetangganya (iOS, 36 engine, chart/map, ER diagram, compare & sync, server dashboard).
- **Ditunda §8 (butuh keputusan produk):** **AI** — dan itu keputusan *on-prem vs cloud*, bukan
  pekerjaan pane. AI bukan ditolak; jangan menulisnya sebagai ditolak.
- **Ditunda dengan syarat:** transport SSH/SOCKS5/tunnel (analisis §5, "bila ada permintaan nyata")
  — dan karenanya Profiles/SSH; UI MCP lebih dari token CLI (§12.3, "kalau dipakai lebih dari satu
  orang"); sort server untuk hasil berhalaman (§12.1.5).

### D. Belum diputuskan (dua panenya)

**Notifications** dan **Profiles** tidak muncul di §5, §8, maupun §9. Kalau diambil, keduanya butuh
keputusan produk lebih dulu (notifikasi: jenis apa yang layak; profil: apakah kredensial bersama
merupakan kebutuhan).

---

## 15. Yang tidak dibaca, dan setting yang maknanya tidak bisa dipastikan

### Tidak dibaca (di TablePro)

- Isi internal **editor tema** (`ThemeEditorView`, `ThemeEditorColorsSection`,
  `ThemeEditorFontsSection`, `ThemeOverview*`) — jadi format penyimpanan tema kustom tidak saya
  pastikan, hanya keberadaannya.
- Sub-pane **Plugins** (`BrowsePluginsView`, `RegistryPluginDetailView`, `TrustedDevelopersView`,
  `PluginIconView`, `PluginsSettingsNavigation`) dan registry/ABI-nya.
- Bagian dalam **License** (`LicenseDevicesSection`, `LicenseTeamSection`, `LicenseActivationForm`)
  dan penyimpanan lisensi (apakah Keychain atau berkas) — tidak dikonfirmasi.
- **AI** lanjutan: `AIProviderDetailSheet`, daftar lengkap `AIProviderType`, `AIProviderRegistry`,
  `AIKeyStorage`, registry OAuth, pipeline telemetry, `ClaudeAgentDisclosureSection`.
- **MCP** lanjutan: `MCPTokenListView`, `MCPTokenCreateSheet`, `MCPTokenRevealSheet`,
  `MCPGrantListView`, `MCPOutsideServerSheet`, `MCPConnectionApproval` (hanya default
  `oncePerConnection` yang terbaca), bentuk `TokenPermissions`.
- `ShortcutRecorderView` internal, `ShortcutConflictResolver`, `SystemHotkeyChecker`,
  `BoundKey`/`ShortcutCombo`, dan sisa `KeyboardShortcutModels.swift` (terpotong di baris 120;
  daftar `ShortcutAction` lengkap dan aturan `allowsBareKey`/reserved tidak saya baca penuh).
- Enum kecil pendukung: kasus `ConnectionHealthCheck`, `SidebarRowSizePreference`,
  `EditorTabStripOverflow`, `DateFormatOption`, `SQLKeywordCase`, `TrackedOperationKind` (labelnya
  terbaca lewat ekstensi di `NotificationsSettingsView`, kasusnya tidak), `LicenseTier`, `TeamRole`.
- Implementasi `ConnectionListPreferences`, `LinkedFolderStorage`, `ExternalConnectionTrustStore`,
  `CredentialProfileStorage`, `SSHProfileStorage`, `CommandLineToolInstaller`.
- Terminal `AppSettingsStorage` tidak saya baca sampai baris terakhir (terbaca 262 baris, lalu ada
  potongan setelahnya di `AppSettingsManager`).
- `SyncCoordinator`, `SyncChangeTracker`, dan transport `TableProSyncTransport` (hanya kategori
  sinkronnya yang terbaca).

### Tidak dibaca (di QueryHive)

- `App.swift` penuh (hanya bagian application lifecycle yang tersentuh lewat grep).
- `Views/ExportSettings.swift` penuh dan `Views/Workspace.swift` penuh (setelan ekspor per tab).
- `Models/GridSort.swift`, `Models/CellEdits.swift`, `Models/Session.swift`,
  `Models/UpdateStatements.swift` penuh.
- Sisa `Models/QueryTab.swift` setelah baris ~500 (default `format`, `delimiter` terbaca,
  sisanya tidak).
- Mesin Rust: default `LIMIT` di engine, dan bentuk lengkap `Capabilities`/Safe Mode di driver.

### Setting yang maknanya tidak bisa dipastikan

- **`jsonViewerPreferredMode` tinggal di `EditorSettings` tetapi tampil di pane Data.** Apakah ini
  disengaja (kategori penyimpanan mengikuti fitur) atau warisan migrasi, tidak saya pastikan.
- **`shareAnalytics`** ada sebagai sakelar dan default true, tetapi ke mana datanya pergi tidak
  saya telusuri.
- **`connectionApproval`** hanya default-nya yang terbaca; kasus lain (mis. per-request) dan
  penjelasan tiap kasus tidak saya pastikan.
- **`syncSQLFavorites`** dan kategori sinkron lainnya: nama jelas, tetapi tidak saya telusuri toko
  mana yang masing-masing baca/tulis.
- **`keepRewindHistory`** menyimpan "nilai baris nyata" ke disk; format, enkripsi, dan retensi
   (7 hari) disebut di UI tetapi tidak saya verifikasi di kode.
- **Default layout sidebar** disimpan di luar `GeneralSettings` (`SidebarPersistenceKey.defaultLayout`)
  sementara kontrolnya di pane General — perbedaan rumah model/UI ini saya catat, bukan saya
  selesaikan.
- **Font editor**: `EditorFontResolver` (TablePro) memilih keluarga monospace; QueryHive punya
  `FontChoice.uiFamilies`/`codeFamilies`. Daftar font dan aturan penyaringannya tidak dibandingkan
  satu per satu.

### Catatan out of bounds (agar tidak disalin)

Yang tidak boleh diambil sebagai kode karena tak terpisahkan dari implementasinya: skema
signature/verifikasi lisensi dan cache yang mengikat machine id; registry plugin + versioning ABI;
transport sync CloudKit TablePro; format/berkas tema kustom; registry provider AI dan pipeline
telemetrinya; resolver konflik hotkey dan penulisan ulang menu AppKit. Pola dan kosakatanya boleh
dipakai (dan sudah dipakai di `SettingsView` QueryHive); kodenya tidak.
