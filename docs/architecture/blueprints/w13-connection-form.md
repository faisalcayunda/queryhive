# Blueprint W13-T19: form koneksi bersidebar

- **Status:** blueprint tingkat berkas, 13 Okt 2026. Sudah direview sekali oleh `architect-reviewer` (verdict "perlu perubahan", tiga temuan memblokir B-1..B-3 dan 13 temuan tidak memblokir N-1..N-13), sudah direvisi (13 Okt 2026, revisi 2; ringkasannya di paragraf "Revisi 2" di akhir), dan hanya menunggu implementasi. Belum ada kode yang ditulis. Keenam pertanyaan terbuka sudah dijawab lewat delegasi pemilik.
- **Untuk:** W13-T19 (ID usulan, CONN-FORM). Permintaan pemilik 2026-10-13 (`target/run/keep/ledger.md:649-652`): centang "Verify the TLS certificate" yang sekarang berdiri sendiri di satu baris selebar form dipindah ke samping kolom host, dan seluruh form koneksi ditata seperti jendela koneksi TablePro: sidebar bagian, baris berlabel kiri dan nilai kanan, dan footer.
- **Sumber:** `AGENTS.md`; `development-plan.md` §0.5, §1, §5 (W13), §7; PRD §6.5; `blueprints/w11-metadata-and-connections.md` §9.1–§9.3; `blueprints/w13-plan-activity.md` §8.2; `tablepro-adoption-plan.md` §8; enam tangkapan layar TablePro dari pemilik.
- **Bukti:** kode dibaca dari pohon kerja `main` (`3a5957b`), 13 Okt 2026. Setiap `path:baris` di bawah benar-benar dibaca hari ini. Kutipan `ConnectionsViews.swift:471-486` di `tablepro-feature-map.md:40` sudah basi; jangan dipakai. Sejak revisi 2, nomor baris yang bergeser di pohon kerja yang kotor ditulis sebagai nama simbol; nomor baris yang tersisa berlaku untuk `main` (`4b7854b`).
- **TablePro (AGPL-3.0):** yang dipelajari hanya tata letak dan angka. Konten jendela 820×620, minimum 720×560 (`TablePro/Views/ConnectionForm/ConnectionFormWindowController.swift:69-70`); sidebar 200–260 pt, detail ≥ 480 pt (`ConnectionFormSplitViewController.swift:18-20`). Tidak ada kode, aset, string, atau identifier yang disalin. Semua label di sini adalah label QueryHive yang sudah ada atau tulisan baru milik QueryHive.
- **Bahasa:** dokumen Indonesia. Identifier, string app, nama tes, dan pesan commit Inggris.

## Ringkasan

1. **Tetap sheet, bukan jendela.** Editor dipresentasikan dengan `.sheet(item: $model.editingConnection)` (`RootView.swift:80-83`). Ukurannya tetap 820×620; hari ini 620×660 (`ConnectionsViews.swift:272`). Sidebar adalah `HStack` biasa, bukan `NavigationSplitView`. Jendela sungguhan ditolak (OQ-2: tetap sheet).
2. **Empat bagian.** Bagiannya General, SSH Tunnel, Limits, dan Appearance. "General" dan "Appearance" adalah nama pane Settings QueryHive sendiri (`Pane.title` di `SettingsView.swift:61`, `:63`).
3. **TLS, rekomendasi opsi A.** Satu kontrol enkripsi ringkas (Menu) duduk di samping kolom Host, sama untuk ketiga driver. Trino mendapat empat pilihan, sesuai empat hasil di `Connections.swift:218-225`. PostgreSQL dan MySQL memakai kata sslmode mereka, dibaca bersama `verify`, karena `verify == false` juga menurunkan keduanya ke tanpa verifikasi di engine (§4.1). Baris CA pindah tepat di bawah Host, sehingga keterikatan "CA membatasi pilihan" terlihat di pane yang sama. Opsi B (bagian jaringan dengan baris mode SSL) dan varian checkbox harfiah dibandingkan di §4. Diputuskan: opsi A (OQ-1, didelegasikan ke pemilik).
4. **Tanpa Rust, FFI, atau migrasi.** `Connection` dan kunci Codable-nya tidak berubah (`Connections.swift:304-472`), begitu pula akun Keychain (`:877-899`). Pilihan enkripsi hanya cara lain menampilkan dan mengubah `scheme`, `verify`, dan `sslmode` yang sudah tersimpan.
5. **Satu draft.** Empat puluh tiga `@State` di sheet (`ConnectionsViews.swift:126-184`; 29 di antaranya field draft di `:130-168`, ditambah `removals`) diganti satu nilai `ConnectionDraft`. Invarian yang sekarang menempel di `.onChange` sebuah view pindah ke draft: port mengikuti scheme Trino (`:535-544`), dan CA memaksa verifikasi (`:725-730`). Invarian CA dijaga satu metode, `enforceCAConstraint()`, yang dipanggil dari `setCAFile`, `choose(_:)`, dan `apply(_:)`, tidak dari `init` (§6.1). Bila tidak dipindah, invarian itu berhenti berlaku saat bagiannya tidak sedang dipasang.
6. **Gate visual sheet ini adalah `ChromeParityTests`, bukan `VisualParityTests`.** Scene-nya `connection` dan `connection-ssh` dengan ukuran tertanam 620×660 (`ChromeParityTests.swift:80`, `:109-111`). Gate ini hanya jalan lokal, karena tes render di-skip di CI (`ledger.md:588`). Perubahan visual dideklarasikan sebagai **V-15**, karena V-14 sudah dipakai (`development-plan.md:738`, `ledger.md:273`). Risiko mode kompatibilitas macOS 27 (R-1) punya gate sendiri: build SDK 14 plus `uiprobe` (§8.2, §10).
7. **Posisi rantai: sebelum W13-T15.** T15 butuh G-LIVE TLS dan kontainer yang sudah dihapus, dan T15 menambah isi ke footer yang sama (§2).

## 1. Keadaan sekarang

### 1.1 Pintu masuk dan alur

- **Pintu masuk.** Semua jalur memanggil `presentConnectionEditor(_:startAtURL:previewTestCount:inGroup:)` (`AppModel+Connections.swift:170-179`), yang mengisi `editingConnection` (`AppModel.swift:133`) dengan `ConnectionEditorTarget` (`SchemaTree.swift:194-210`). Pemanggilnya: menu picker koneksi (`ConnectionsViews.swift:56-57`), sidebar (`SidebarTree.swift:79-80`, `:130-131`), dan menu pohon (`TreeMenu.swift:46`, `:50`, `:72`).
- **Tiga langkah dalam satu sheet** (`ConnectionsViews.swift:124`):
  - pemilih jenis berupa tile (`:303-360`);
  - URI (`:373-409`);
  - form (`:411-429`).
  - Koneksi tersimpan langsung masuk ke form (`:967`).
- **Header form** (`:475-498`) memuat:
  - tile merek 56 pt;
  - nama, atau "New Connection";
  - tanda "Unsaved changes";
  - "Change Type…", yang kembali ke pemilih jenis.
- **Footer** (`:922-958`) memuat:
  - Test Connection dan statusnya di kiri;
  - Delete (hanya saat edit), Cancel, dan Save di kanan.

### 1.2 Isi form per jenis

| Field | Trino | PostgreSQL | MySQL | Bukti |
|---|---|---|---|---|
| Name (wajib) | ya | ya | ya | `ConnectionsViews.swift:502-506` |
| Color (6 warna) | ya | ya | ya | `:507`; `Connections.swift:7-8` |
| Host (wajib) | ya | ya | ya | `:508-511` |
| Transport HTTPS/HTTP/Prefer | ya; hanya HTTPS bila ada CA | – | – | `:513-528` |
| Port (wajib, 1–65535) | ya | ya | ya | `:529-533`; `ConnectionFormIssues.swift:110` |
| User (wajib) | ya | ya | ya | `:545-548` |
| Password (kosong = pertahankan) | ya, kecuali auth JWT | ya | ya | `:549-561` |
| Catalog / Database | Catalog | Database (wajib) + "All Databases" | Database + "Show All" | `:562-584`, `:596-608`; `Connections.swift:88`, `:93` |
| Schema | ya | ya + "Show System Schemas" | – | `:585-595`; `Connections.swift:90` |
| SSL mode | – | 5 kata libpq; hanya verify-ca/verify-full bila ada CA | disable/prefer/require | `:610-615`; `Connections.swift:98-122` |
| "Verify the TLS certificate" | hanya transport HTTPS; nonaktif bila ada CA; untuk HTTP/Prefer diganti kalimat | – | – | `:616-644` (ChipToggle `:629-630`) |
| Safe Mode | ya | ya | ya | `:649-659` |
| Environment | ya | ya | ya | `:661-672` |
| Statement timeout (detik) | ya | ya | ya | `:676-683` |
| Authentication Password/JWT | ya | – | – | `:684`, `:696-707` |
| CA certificate | ya | ya | kalimat "no CA setting" | `:685-688`, `:710-731` |
| SSH tunnel (host atau alias, user, port, metode, kunci, passphrase/password) | ya | ya | ya | `:689`, `:733-800` |

Grup tidak ada di form, dan itu disengaja. Koneksi baru yang dibuat dari menu grup diberi `model.newConnectionGroup` saat Save (`ConnectionsViews.swift:1040-1047`; alasannya di `AppModel+Connections.swift:174-178`).

### 1.3 Validasi

`ConnectionFormIssues.fields` (`ConnectionFormIssues.swift:103-127`) memeriksa hal berikut:

- name, host, port 1–65535, dan user;
- database untuk PostgreSQL;
- saat tunnel menyala: SSH host, SSH port, SSH user (atau user dari alias), dan berkas kunci untuk metode `key` (atau berkas dari alias);
- berkas CA harus ada;
- timeout 0–600.

Kalimatnya dirakit `message` (`:136-145`). Kolom merah dan tulisan "Required" baru muncul setelah percobaan Save atau Test (`ConnectionsViews.swift:873-877`). Pesan validasi dititipkan ke `testState.failure`, jadi bercampur dengan hasil Test (`:1025`, `:1091`).

### 1.4 Penyimpanan

- **Keychain**, service `id.data-ecosystem.queryhive` (`Connections.swift:873`). Ada empat slot: password database (akun = UUID), `ssh-password:`, `ssh-passphrase:`, dan `jwt:` (`:877-888`, `:899`).
  - Form tidak pernah menampilkan rahasia tersimpan.
  - Field kosong berarti "pertahankan".
  - Rahasia dihapus hanya lewat "Remove saved…", atau bila koneksi berhenti memakai slot itu (`ConnectionFormIssues.swift:156-214`; `ConnectionsViews.swift:826-839`).
- **`connections.json`** di Application Support/QueryHive (`Connections.swift:782-800`). Berkas ini berisi semua field `Connection` (`:417-422`) di dalam `ConnectionsDocument` bersama grup (`:673-697`), ditambah satu salinan `.bak` (`:841-845`).
- **Urutan Save:** Keychain dulu, baru JSON (`ConnectionsViews.swift:1033-1050`). Item lama yang masih memunculkan prompt diperbarui sesudahnya (`:1059-1070`). `draftID` dipakai ulang antar-percobaan supaya item Keychain tidak yatim (`:181-184`).

### 1.5 Test, Save, Delete

- **Test** (`:1086-1136`):
  1. menjalankan validasi yang sama dengan Save;
  2. bila ada masalah, berhenti dengan pesan;
  3. bila tidak, menyusun koneksi draft; rahasia yang diketik menang atas yang tersimpan, kecuali penghapusannya sudah dijadwalkan (`:1099-1112`);
  4. menjalankan `Engine.current.run("test")` (`:1120`).
  - Hasilnya satu baris status (`:883-912`).
  - Kegagalan karena host key dibuka lewat `HostKeyCenter`, dan Test diulang setelah kunci dipercaya (`:290`, `:293-295`).
  - Pintasannya ⌘↩ (`:940`; `Shortcuts.swift:267-271`).
- **Save** (`:1021-1078`). Tidak ada "Save & Connect". Yang paling dekat: koneksi **baru** menjadi tujuan tab terpilih bila tab itu belum punya koneksi (`:1074-1076`).
- **Delete** memakai dialog konfirmasi (`:281-285`), lalu `model.deleteConnection` (`:1080-1084`).

### 1.6 Impor

- **URI.** `ConnectionURL.parse` (`Connections.swift:552-583`) mengisi field form (`ConnectionsViews.swift:443-473`). Parameter yang diabaikan disebut dalam satu catatan (`:458`, `:675`).
- **Navicat, DBeaver, DataGrip, dan daftar QueryHive** tidak melewati form. Menu impor menutup sheet dulu (`:328-338`, `:362-368`).

### 1.7 Tes dan baseline yang menutupnya

- **Tes unit:**
  - `ConnectionFormIssuesTests` (validasi dan rencana rahasia, `:14-152`);
  - `ConnectionURLTests`, `ConnectionKeychainTests`, `ConnectionEnvironmentTests`, `ConnectionMappingTests`, `ConnectionGroupTests`, `ConnectionListTransferTests`.
- **Tidak ada tes unit untuk pemetaan state sheet** (`load`, `draftConnection`). Keduanya `private` dan hanya terlihat lewat gambar.
- **Render.**
  - `ShellTests.testASheetStillAttachesToTheWindow` (ditulis sebagai nama simbol; barisnya bergeser di pohon kerja yang kotor).
  - `ChromeParityTests` memakai scene `connection` (Trino tersimpan) dan `connection-ssh` (PostgreSQL bertunnel), dengan permukaan 620×660 (`ChromeParityTests.swift:76-83`, `:109-111`). Baselinenya `chrome-connection-{dark,light}` dan `chrome-connection-ssh-{dark,light}`.
  - Rekam dengan `QH_RECORD_CHROME` (`:19-27`). Menu dan popover tidak tertangkap (`:48-50`).
- **Scene snapshot tanpa baseline:** `new-connection`, `connection-uri`, `connection-trino`, `connection-postgres`, `connection-mysql`, `connection-tested` (`Snapshot.swift:552-556`, `:966-997`).

### 1.8 Temuan sampingan

- **T-1. Koneksi tersimpan terbuka dengan tanda "Unsaved changes".**
  - Sebab: `load()` menormalkan `sslmode` kosong ke bawaan driver (`ConnectionsViews.swift:989`), `draftConnection` menulisnya (`:211`), dan `isDirty` membandingkannya (`:198`).
  - Buktinya ada di baseline yang ter-commit: `__Baselines__/chrome-connection-dark.png` menampilkan "Unsaved changes" untuk "Trino production". Koneksi itu dibuat dengan `sslmode` bawaan `""` (`Snapshot.swift:297-299`; `Connections.swift:382`).
  - Diperbaiki di langkah 4 (§10).
- **T-2.** Pesan sukses Test untuk MySQL berbunyi "N catalogs" (`ConnectionsViews.swift:889`), padahal level MySQL adalah database (`Connections.swift:68`). Diperbaiki di langkah 4.
- **T-3. Item Keychain yatim (sudah ada sebelum T19; ke backlog, bukan tugas ini).** Pada koneksi baru, bila Keychain tertulis (`Save`, `ConnectionsViews.swift:1033-1050`) lalu tulis JSON gagal dan pengguna menekan Cancel, item di bawah `draftID` tertinggal. Langkah 1 netral dan harus mempertahankan perilaku ini persis. Perbaikannya (hapus slot yang baru di-`set` bila JSON gagal untuk koneksi baru) dicatat orkestrator sebagai backlog tersendiri (§2.4 butir 6). Selain itu, tidak ada jalur baru di blueprint ini yang bisa menghilangkan rahasia atau data.

## 2. Tugas, kepemilikan, dan tabrakan

### 2.1 ID dan posisi

- **ID.** W13-T19 belum dipakai (grep atas seluruh `*.md`).
- **Tingkat.** Sedang (fitur UI). Langkah 1 Tinggi, karena memindah jalur Save dan Keychain.
- **FR.** Tidak ada FR baru. FR-CON-09 tetap berlaku.
- **Rantai `ConnectionsViews.swift` dan `Connections.swift`** (`development-plan.md:1082`): W11-T3s → W13-T4b → W13-T15 → W13-T16 → bersyarat. Status hari ini:
  - **W11-T3s mendarat.** `ConnectionURLTests.swift` ada, dan `statementTimeoutMS` ada di `Connections.swift:376`.
  - **W13-T16 mendarat di luar urutan.** `DBeaverImport.swift`, `DataGripImport.swift`, dan `ConnectionListTransfer` (`Connections.swift:707-751`) sudah ada; lihat juga `handoff.md:27`.
  - **W13-T4b belum.** Migrasi berhenti di `0008`.
  - **W13-T15 belum.** `securityLabel` hanya ada definisinya (`Connections.swift:489`); lihat juga `handoff.md:31`.
- **Usulan rantai:** W11-T3s → **W13-T19** → W13-T4b → W13-T15 → (W13-T16 sudah) → W13-T11 → W13-T17 → W13-T10 → W13-T18. Alasannya:
  - T19 tidak butuh kontainer.
  - T15 butuh G-LIVE TLS dan G-GOLDEN (`development-plan.md:954`), sementara kontainernya sudah dihapus (`AGENTS.md`).
  - T15 menambah isi ke footer yang dirancang ulang di sini, jadi lebih murah menulisnya sekali di atas footer baru.
  - Satu-satunya baris Swift milik T4b ada di `connectionEnvironment` (`w13-plan-activity.md:667`, berkas `AppModel+Connections.swift`), yang tidak disentuh T19.

### 2.2 Berkas

**Dimiliki T19:**

- `app/Sources/QueryHive/Views/ConnectionsViews.swift`
- `Views/ConnectionForm.swift` (baru: sidebar, kartu, baris, kontrol enkripsi, empat bagian). Berisi **salinan** `SettingsCard` dan `RowDivider`, karena keduanya `private` di `SettingsView.swift` yang tidak boleh disentuh (§5.4)
- `Models/ConnectionDraft.swift` (baru)
- `Models/ConnectionTLSChoice.swift` (baru)
- `Models/ConnectionFormIssues.swift` (tidak punya pemilik §7 setelah W11-T3)
- `Support/Snapshot.swift` (hanya menambah `case`)
- Tes baru: `ConnectionDraftTests.swift`, `ConnectionTLSChoiceTests.swift`, `ConnectionSheetTests.swift` (berisi **salinan** `wait` dan `descendants`, yang `private` di `ShellTests.swift`; §8.2)
- Tes yang diubah: `ConnectionFormIssuesTests.swift`, `ChromeParityTests.swift`
- `app/DESIGN.md` (langkah terakhir; berkas ini juga milik W14-T5, lihat §2.3)
- `__Baselines__/chrome-connection*` (hanya orkestrator)

**Tidak disentuh:**

| Berkas | Alasan |
|---|---|
| `Models/Connections.swift` | tidak ada field baru |
| `AppModel+Connections.swift` | rantai W11-T3s → W13-T4b → W13-T16 |
| `SchemaTree.swift` (`ConnectionEditorTarget`) | rantai W9-T3 → W11-T4 → W12-T12 |
| `Support/Theme.swift` | W9-T9 → W10-T2 |
| `SettingsView.swift` | W9-T9 → W13-T8c → W13-T7 |
| `Models/Shortcuts.swift` | … → W12-T2 |
| `RootView.swift`, `App.swift` | – |
| `ShellTests.swift` | sedang disunting agen lain hari ini |
| `crates/**`, `app/Generated/` | – |

### 2.3 Tabrakan

- **`Support/Snapshot.swift`** (`development-plan.md:1083`; `w13-plan-activity.md:827`). W11-T4, W12-T4, W13-T5, W13-T6, dan W13-T8c belum mendarat. T19 hanya menambah lengan `case` dan dua kait statis.
  - Usul: lengan `case` boleh digabung tangan, seperti baris `mod` (`development-plan.md:1050`).
  - Bila orkestrator menolak, T19 menunggu W13-T6.
- **`ChromeParityTests.swift`** tidak punya baris §7. W13-T8c kemungkinan menambah scene di berkas yang sama, jadi aturan gabung tangannya sama.
- **`app/DESIGN.md`** juga milik W14-T5 (`development-plan.md:1166`). Aturan gabung tangannya sama dengan `Snapshot.swift`; T19 hanya menyentuh bagian editor koneksi (langkah 6).
- **`__Baselines__/`** hanya lewat langkah orkestrator (`development-plan.md:1110`, §0.5 `:89-95`).
- **W13-T7 (lokalisasi).** T19 harus mendarat sebelum T7, supaya string barunya ikut dikatalogkan.

### 2.4 Perubahan untuk orkestrator (dokumen yang tidak boleh disunting blueprint ini)

1. Tambah baris §5 W13:

   | W13-T19 | Form koneksi bersidebar (CONN-FORM, permintaan pemilik 2026-10-13): empat bagian dengan sidebar, baris berlabel, kontrol enkripsi di samping Host, ringkasan masalah di footer, lencana per bagian. Tanpa Rust, FFI, atau migrasi. Tingkat Sedang; langkah 1 Tinggi. bp: `blueprints/w13-connection-form.md`. | FR-CON-09; V-15 | (berkas §2.2) | GP-s | satu reviewer opus per putaran (O-20) dengan daftar periksa UX dan AX | G-SWIFT, ChromeParity, G-APP | M | `feat(connections): the connection form is four sections beside a sidebar` |

2. Ubah §7 `:1082` sesuai §2.1, dan tambahkan T19 ke rantai `Snapshot.swift` (`:1083`).
3. PRD §6.5 (`prd-performance-and-parity.md:288-302`): daftarkan V-14, yang sudah dipakai di `development-plan.md:738` dan `ledger.md:273,289` tetapi belum ada di tabel. Tambahkan juga **V-15**: "Form koneksi bersidebar: empat bagian, kontrol enkripsi di samping Host, footer" (W13-T19).
4. `development-plan.md:92` masih menyebut "V-1…V-12"; samakan dengan tabel PRD.
5. Salin keputusan OQ-1 sampai OQ-6 ke ledger sebelum langkah 3. Semuanya sudah terjawab lewat delegasi pemilik (bagian "Pertanyaan terbuka untuk pemilik").
6. Daftarkan dua hal di luar T19: backlog Keychain yatim (§1.8 T-3) dan tugas terpisah impor `verify: false` (bagian "Tugas terpisah").
7. Pastikan prasyarat ledger sebelum langkah 1: `ledger.md:652` meminta pekerjaan ini dimulai setelah bug Export dan rilis 0.2.2 beres, sedangkan `docs/releases/v0.2.2.md` masih untracked.

## 3. Arsitektur informasi

### 3.1 Bagian

| Bagian | Simbol SF | Isi |
|---|---|---|
| General | `gearshape` (sama dengan pane General, `SettingsView.swift:76`) | identitas, server, autentikasi |
| SSH Tunnel | `lock.shield` | tunnel SSH |
| Limits | `speedometer` | Safe Mode, statement timeout |
| Appearance | `paintbrush` (`SettingsView.swift:78`) | warna, label lingkungan |

Simbol dipilih dari SF Symbols 1–2, karena binary dibangun dengan SDK 14 (`ledger.md:640`).

### 3.2 Isi per bagian per jenis (opsi A)

| Bagian / kartu | Baris | Trino | PostgreSQL | MySQL |
|---|---|---|---|---|
| General / tanpa judul | catatan URL (bila ada, `:675`) | ✓ | ✓ | ✓ |
| | Name | ✓ | ✓ | ✓ |
| | Type: tile 18 pt + `kind.label` + "Change Type…" | ✓ | ✓ | ✓ |
| General / "Server" | Host [kolom][kontrol enkripsi] + keterangan bersyarat | ✓ (4 pilihan) | ✓ (5 kata) | ✓ (3 kata) |
| | Port | ✓ | ✓ | ✓ |
| | CA certificate [kolom][Choose…][Clear] + keterangan `:721-722` | ✓ | ✓ | kalimat `:686-687` |
| | Catalog / Database + sakelar di belakangnya | Catalog | Database + "All Databases" | Database + "Show All" |
| | Schema + sakelar di belakangnya | ✓ | ✓ + "Show System Schemas" | – |
| General / "Authentication" (`:697`) | User | ✓ | ✓ | ✓ |
| | Method: Password \| JWT (`Connections.swift:299`) | ✓ | – | – |
| | Password atau JWT + petunjuk `:865-871` + "Remove saved…" | ✓ | ✓ | ✓ |
| SSH Tunnel / "SSH tunnel" | InlineCheckbox "Connect through an SSH tunnel" (`:734`) | ✓ | ✓ | ✓ |
| | kalimat fingerprint (`:738`, terikat w11 §9.2) | ✓ | ✓ | ✓ |
| | Host [kolom][Aliases] + pratinjau alias (`:739-759`) | bila menyala | | |
| | User, Port, Authentication (Agent/Key file/Password) | bila menyala | | |
| | Key file [Choose…] / Passphrase / Password + "Remove saved…" | bila menyala | | |
| Limits / "Safe Mode" | Segmented + detail (`:649-659`) | ✓ | ✓ | ✓ |
| Limits / "Statement timeout" | Seconds + keterangan `:680-681` | ✓ | ✓ | ✓ |
| Appearance | Color (`ColorSwatchPicker`, `:87-105`) | ✓ | ✓ | ✓ |
| | Environment + "A label only. It does not change what Safe Mode allows." (`:667`) | ✓ | ✓ | ✓ |

### 3.3 Keputusan lama yang dipertahankan

- **Sakelar pohon tetap di belakang field yang dimodifikasinya.** Komentar `:568-608` menyebut tata letak yang memisahkannya "the previous layout's mistake".
- **Grup tetap bukan field form** (`:1040-1043`; `AppModel+Connections.swift:174-178`). Membalik keputusan ini adalah OQ-4, yang dijawab tidak (baris Group tidak ada).
- **Kalimat yang diikat w11 §9.2 dipakai apa adanya** (`w11-metadata-and-connections.md:712-714`): kalimat fingerprint, "JWT is sent only over HTTPS.", dan teks bantuan CA.
- **Keterikatan CA dengan mode TLS** (w11 §9.1, `:706`) dan **validasi bernama dengan ⌘↩** (w11 §9.3, `:718-719`) tetap berlaku.

### 3.4 Yang tidak dibawa dari tangkapan layar TablePro

| Fitur di tangkapan layar | Status di QueryHive | Bukti | Keputusan |
|---|---|---|---|
| Sumber kredensial dan pengelola profil kredensial | tidak ada; "profile" QueryHive adalah item milik akun, bukan kredensial | `SettingsView.swift:337-339`; `crates/qh-storage/src/identity.rs:1-19` | tidak dibawa |
| Minta password setiap kali connect | tidak ada; password kosong berarti connect tanpa password | `ConnectionsViews.swift:254-257` | tidak dibawa |
| Zona waktu, perintah SQL awal, skrip shell sebelum connect | tidak ada setelan engine-nya (grep `TIME_ZONE`, `STARTUP`, `INIT_SQL` di `crates/` kosong); skrip shell adalah permukaan eksekusi baru | – | tidak dibawa; kandidat PRD §10 bila pemilik mau |
| Batas waktu connect per koneksi | konstanta engine 15 s | `crates/qh-core/src/error.rs:29`; `crates/qh-tunnel/src/tunnel.rs:77` | tidak dibawa |
| Batas waktu query | ada, sebagai statement timeout per koneksi | `Connections.swift:376` | dibawa ke Limits |
| Kebijakan AI dan aturan AI | tidak ada AI di app; ditunda menunggu keputusan on-prem | `tablepro-adoption-plan.md:719-727` | tidak dibawa |
| Akses klien eksternal per koneksi | akses MCP diatur per token (allowlist koneksi), dikelola lewat CLI | `crates/qh-ffi/src/mcp.rs:33-40`; `SettingsView.swift:17-20` | tidak dibawa |
| Tag bebas | satu label lingkungan (None/Dev/Staging/Prod) | `Connections.swift:182-203` | dibawa sebagai Environment |
| Jalur selain langsung dan SSH | hanya tunnel SSH | `Connections.swift:360-379` | tidak dibawa |
| Warna "tanpa warna" | enam warna, tanpa "none" | `Connections.swift:7-8` | tidak dibawa |
| Safe Mode | ada, empat tingkat | `Connections.swift:145-175` | dibawa ke Limits |
| Grup | ada di data, sengaja tidak di form | §3.3 | OQ-4: tidak dibawa |
| Simpan lalu connect | tidak ada padanan | §1.5 | OQ-3: tidak dibawa |

## 4. Pertanyaan TLS

### 4.1 Batasan yang menentukan

- **Flag `verify` berlaku untuk ketiga driver, bukan hanya Trino.** Komentar `Connections.swift:324-325` ("Trino only") tidak menggambarkan perilakunya.
  - `connectionEnvironment` mengirim `DB_INSECURE=1` untuk setiap driver bila `verify == false`, kecuali Trino Prefer (`AppModel+Connections.swift:945-952`).
  - Engine memakainya untuk PostgreSQL dan MySQL: `crates/qh-ffi/src/config.rs:416-420` menurunkan prefer, require, verify-ca, dan verify-full ke `RequireNoVerify`. Jalur MCP sama (`crates/qh-ffi/src/mcp.rs:470-489`).
  - Untuk Trino, `verify` hanya berarti di transport HTTPS. Untuk `prefer`, engine tidak pernah memeriksa sertifikat.
  - Akibatnya, "verify-full" dengan `verify == false` berjalan **tanpa** verifikasi, dan "prefer" dengan `verify == false` berjalan sebagai require. Pilihan di menu harus membaca `(sslmode, verify)`, bukan `sslmode` saja (§6.2).
- **PostgreSQL dan MySQL menyatakan sebagian besar maksud verifikasi di dalam `sslmode`.** MySQL tidak punya kata verify (`Connections.swift:98-120`).
- **`verify == false` bisa sampai ke PostgreSQL dan MySQL lewat dua jalan:**
  - impor Navicat, DBeaver, dan DataGrip menulis `verify: false` (`AppModel+Connections.swift:499`, badan `applyImport` di `:391`), dan `applyTLS` (`:551-564`) hanya menyetel `verify` untuk Trino. PostgreSQL dari DBeaver dengan `ssl=true` (verify-full) berjalan tanpa verifikasi. Ini sudah ada hari ini dan menjadi tugas terpisah (bagian "Tugas terpisah");
  - "Change Type…" dari Trino yang unverified membawa `verify == false` ke PostgreSQL (`ConnectionsViews.swift:433-441`).
- **Berkas CA mengikat mode.** PostgreSQL dibatasi ke verify-ca/verify-full dengan `verify = true`, dan Trino dipaksa ke HTTPS dengan verifikasi menyala (`ConnectionsViews.swift:522`, `:614`, `:630`, `:725-730`). Hari ini hanya `setCAFile` yang menegakkan ini; `choose(_:)` (`:433-441`) dan penerapan URL (`:459-471`) bisa melanggarnya (§6.1).
- **Port Trino mengikuti scheme** (`:535-544`).
- **Trino punya tepat empat hasil** (`Connections.swift:218-225`). Pengimpor URL sudah memetakan kata libpq ke keempatnya (`:631-642`).
- **Trino dengan JWT hanya bisa HTTPS.** Engine menolak JWT dengan Prefer atau HTTP (`crates/qh-ffi/src/config.rs:352-364`).
- **Pilihan "HTTP" Trino tidak selalu berarti tanpa enkripsi.** Password, port 443, atau port 8443 menaikkannya ke HTTPS dengan `verify` tersimpan yang tersembunyi (`config.rs:385-393`).
- **Nilai `verify` yang tersimpan dipertahankan** selama transport bukan HTTPS (`ConnectionsViews.swift:623-626`).
- **Menu yang terbuka tidak tertangkap ChromeParity** (`ChromeParityTests.swift:48`).

### 4.2 Opsi A (direkomendasikan, diputuskan): kontrol enkripsi ringkas di samping Host

- **Bentuk.** Satu `Menu` dengan `.menuStyle(.button)`, `.buttonStyle(.plain)`, dan indikator bawaan disembunyikan. Preseden yang sama persis sudah ada di sheet ini: menu "Import…" di pemilih jenis (`ConnectionsViews.swift:331-352`), yang lebih tepat dipakai sebagai acuan daripada `ConnectionPickerButton` (`:44-84`). Lebarnya tetap 168 pt dan duduk di sebelah kanan kolom host, untuk ketiga driver. Isinya glyph, teks pendek, dan chevron.
- **Pilihan** (`ConnectionTLSChoice.options(kind:caFile:dbAuth:)`):
  - **Trino:** "HTTPS · verified", "HTTPS · unverified", "Prefer · unverified", "HTTP". Kosakatanya kosakata QueryHive sendiri (`Connections.swift:493-499`). Bila ada CA, hanya "HTTPS · verified". Bila `dbAuth` adalah JWT, hanya dua pilihan HTTPS.
  - **PostgreSQL:** prefer, disable, require, verify-ca, verify-full. Bila ada CA, hanya verify-ca dan verify-full.
  - **MySQL:** disable, prefer, require.
- **Judul pilihan saat ini membaca `verify`.** Untuk PostgreSQL dan MySQL dengan `verify == false` dan kata selain `disable`, judul yang tampil adalah `"<kata> · unverified"`, glyph `lock`, dan `spokenDescription` menyebut bahwa sertifikat tidak diperiksa. Satu keterangan di bawah Host menjelaskannya. Nilai itu tidak ada di `options()`; menu tetap menampilkannya sebagai judul, dan tidak pernah kosong.
- **Memilih sesuatu menulis `verify = true`** untuk setiap pilihan PostgreSQL dan MySQL (`disable` tidak bergantung pada `verify`, tetapi juga menulis `true` supaya seragam). Menu membaca dan menulis lewat `Binding` yang memanggil metode draft (§6.1), bukan `$draft.scheme` atau `$draft.sslmode` langsung.
- **Glyph:**
  - `lock.fill` untuk mode yang memverifikasi;
  - `lock` untuk mode terenkripsi tanpa verifikasi (termasuk `"<kata> · unverified"`);
  - `lock.open` untuk mode yang bisa berjalan tanpa enkripsi.

  Glyph bersifat dekoratif dan disembunyikan dari VoiceOver; maknanya dibawa teks.
- **Bantuan.** Teks `.help` di tiap pilihan memakai kalimat yang sudah ada (`:525-527`, `:632-642`). Keterangan di bawah baris Host hanya muncul bila perlu:
  - kalimat Prefer atau HTTP untuk Trino (`:632-633`, `:638-639`). Untuk HTTP, keterangan menyebut bahwa password, port 443, atau port 8443 menaikkannya ke HTTPS;
  - keterangan "unverified" untuk PostgreSQL atau MySQL ber-`verify == false`;
  - "Only modes that check the certificate are offered while a CA file is set." bila ada CA.
- **Yang hilang dari form:** picker Transport (`:513-528`), baris SSL mode (`:610-615`), dan baris ChipToggle "Verify the TLS certificate" (`:616-644`).
- **Kenapa A.** Keputusan verifikasi duduk di samping Host, seperti permintaan pemilik, dan kontrolnya sama untuk tiga driver. Empat hasil Trino menjadi empat pilihan eksplisit. Baris CA, satu-satunya yang membatasi pilihan, ada di kartu yang sama, dua baris di bawahnya.

### 4.3 Opsi B: bagian "SSH & TLS" dengan baris mode

- **Bentuk.** General menampilkan Host tanpa kontrol. Bagian kedua menjadi "SSH & TLS", dengan kartu "Encryption" berisi Mode (pilihan yang sama dengan §4.2), CA certificate, nanti sertifikat klien W13-T17, dan hasil TLS dari W13-T15. Di bawahnya kartu SSH tunnel.
- **Untung.** Seluruh keamanan transport ada di satu pane, ada ruang untuk kalimat penjelasan, dan menjadi rumah alami T15 dan T17. Bentuknya juga lebih dekat ke referensi.
- **Rugi.** Permintaan eksplisit pemilik tidak terpenuhi: di samping Host tidak ada apa pun. Status TLS juga tidak terlihat di tempat host diketik, dan butuh satu perpindahan pane untuk memeriksanya.

### 4.4 Varian A1 (checkbox harfiah), ditolak

- **Bentuk.** `InlineCheckbox` "Verify" di samping Host. Komponen ini memang dibuat untuk duduk di samping field (`Theme.swift:713-717`).
- **Alasan ditolak.** Checkbox itu hanya ada untuk Trino di transport HTTPS. Untuk dua dari tiga driver dan dua dari tiga transport, slotnya kosong atau berganti teks, sehingga tata letak melompat. Picker transport dan baris SSL mode juga tetap ada.
- **Kelebihannya** hanya biaya bangun yang paling murah.

### 4.5 Rekomendasi

**Opsi A.** Yang hilang bagi pemilik dengan A:

- toggle satu klik menjadi menu dua klik;
- semua pilihan tidak lagi terlihat sekaligus seperti di segmented;
- mode SSL tidak berada di pane jaringan seperti di referensi.

Yang hilang dengan B: permintaannya sendiri, dan status TLS tidak terlihat di samping host. **OQ-1 terjawab: A**, lewat delegasi pemilik. Bila keputusan ini dibuka lagi dan B yang dipilih:

- `ConnectionField.caFile` dipetakan ke bagian kedua;
- bagian kedua berganti nama menjadi "SSH & TLS";
- tes §8.2 butir 1 diganti dengan "baris enkripsi ada di SSH & TLS dan General tidak punya kontrol enkripsi".

## 5. Aturan tata letak

### 5.1 Sheet dan ukuran

- **Ukuran tetap** `.frame(width: 820, height: 620)`. Lebarnya mengikuti angka studi; tingginya ≤ 620 supaya muat di bawah title bar jendela utama yang minimum 700 pt (`RootView.swift:19`). Bingkai luar ini menyembunyikan ukuran asli isinya, jadi tes tinggi tidak boleh mengukur sheet yang berbingkai tetap (§8.2 butir 2).
- **Langkah pemilih jenis dan URI** memakai bingkai yang sama, dengan kolom isi paling lebar 620 di tengah supaya tampilannya tidak berubah.
- **Struktur:** `HStack(spacing: 0) { sidebar.frame(width: 200); hairline; content }`, tanpa `NavigationSplitView`.
- **Batas tinggi minimum.** Setiap pane yang menggulir atau meregang diberi `.frame(minHeight: 0, maxHeight: .infinity)`. Ini pelajaran dari `RootView.swift:55-59` dan `ledger.md:640`.
- **Latar** tetap `.background(Tone.canvas)` (`:273`). Tidak ada `NSWindow.backgroundColor`.

### 5.2 Sidebar

- **Lebar 200.** Latarnya `Tone.ink.opacity(0.03)` dengan hairline di kanan (`panelEdge(.trailing)`, `Theme.swift:421-425`).
- **Blok atas 64 pt:** `ConnectionBrandTile` 36 pt (`:1210-1241`), `kind.label` `.ui(12, weight: .semibold)`, dan subjudul "Not saved yet." atau "Saved \(kind) connection." (`:482`).
- **Item:**
  - tinggi 30, radius 8;
  - simbol 13 pt dan label `.ui(13)`;
  - item terpilih diisi `Tone.ink.opacity(0.14)`, sama dengan `paneItem` (`SettingsView.swift:185`);
  - lencana `exclamationmark.triangle.fill` 11 pt `Tone.amber` di kanan bila bagian itu punya masalah.
- **Dasar** (hanya saat edit): PillButton destructive compact "Delete Connection…", pindahan dari footer (`:947-949`). Dialog konfirmasinya tetap di root.

### 5.3 Kepala konten

- Tinggi 52.
- Judul `.ui(17, weight: .bold, rounded: true)`: "New Connection" atau nama koneksi (`:479`).
- Di kanan: "Unsaved changes" (`:487-492`).
- Hairline di bawahnya.

### 5.4 Kartu dan anatomi baris

- **Kartu** seperti `SettingsCard` (`SettingsView.swift:209-226`): `.glass(12)`, padding 14, judul `SectionLabel`. Jarak antar-kartu 18. Antar-baris ada `RowDivider` (`:260-264`). Keduanya `private` di berkas yang tidak boleh disentuh, jadi `ConnectionForm.swift` menulis salinannya sendiri (§2.2).
- **Baris:**
  - tinggi minimum 36;
  - kolom label 128 pt, `.ui(13)` `Tone.ink`, satu baris; bila terpotong, label lengkap tampil di `.help`;
  - kolom nilai fleksibel:
    - kolom teks mengisi lebar dengan `.field(invalid:)` (`Theme.swift:392-400`);
    - kolom angka (Port, SSH port) 96 pt;
    - segmented mengisi lebar;
    - menu dan checkbox rata kanan;
    - sakelar di belakang Database/Schema tetap `.fixedSize()` dan `.layoutPriority(1)`, seperti `:580-607`.
- **Keterangan** `.ui(11)` `Tone.secondary` di bawah baris, sejajar kolom nilai, dan membungkus.
- **Tanda wajib.** Label kehilangan akhiran "· Required" dan huruf besar (`LabeledField` tidak lagi dipakai di form). Status wajib disampaikan lewat:
  - ringkasan di footer;
  - lencana sidebar;
  - garis merah dan "Required" setelah percobaan Save atau Test (`:873-877`);
  - hint aksesibilitas "Required".
- **Label yang diperpendek** supaya muat 128 pt: "Statement timeout (seconds)" menjadi judul kartu "Statement timeout" plus baris "Seconds"; "SSH authentication" menjadi "Authentication" di dalam kartu SSH tunnel; "Key passphrase · leave blank to keep" menjadi "Passphrase" dengan placeholder "Unchanged" (`:789`).

### 5.5 Footer

- Tinggi 56, hairline di atas, padding 20.
- **Kiri:** Test Connection (`:937-941`), atau kapsul "Testing…" saat berjalan (`:926-936`), lalu slot status (fleksibel, `layoutPriority(-1)`, `:911`).
- **Kanan:** Cancel (Esc), lalu Save (HubButton, Return, nonaktif bila tidak ada perubahan, `:953`).
- **Prioritas slot status:**
  1. Test sedang berjalan: kosong.
  2. Hasil Test sejak perubahan terakhir: baris mint atau coral (`:883-912`).
  3. Ada masalah: baris amber dengan `exclamationmark.triangle` dan ringkasan (§5.6).
  4. Selain itu: kosong.
- **Hasil Test kembali ke idle** setiap kali pengguna mengubah draft (perilaku baru). Reset dipasang hanya di binding yang ditulis pengguna (atau lewat penanda "sudah dimuat"), **bukan** lewat `.onChange(of: draft)`: `load()` di `onAppear` ikut memicunya dan akan menghapus `previewTestCount` (scene `connection-tested`), jebakan yang sama dengan komentar di `load()` (§6.4).

### 5.6 Validasi dan lencana

- **Sumber masalah:** `ConnectionFormIssues.fields(for: draft.formState, …)`, dihitung terus-menerus.
- **Lencana dan ringkasan selalu tampil** (rekomendasi; OQ-5). Warnanya amber, bukan coral. Garis merah di field tetap hanya setelah percobaan.
- **Ringkasan.** Bagian yang sedang terlihat didahulukan, memakai `message` (`ConnectionFormIssues.swift:136-145`). Untuk bagian lain, ringkasan diawali judul bagiannya, misalnya "SSH Tunnel: SSH host is required.". Bila lebih dari satu bagian bermasalah, tambahkan "Also see \(section)."
- **Save atau Test dengan masalah:**
  1. `attemptedSave = true`;
  2. pindah ke bagian bermasalah pertama (bagian yang terlihat lebih dulu, lalu urutan General, SSH Tunnel, Limits, Appearance);
  3. fokus ke field bermasalah pertama setelah bagiannya terpasang, dipasang di `.task(id: section)` atau `.onAppear` milik bagian itu, bukan di "putaran run loop berikutnya". `@FocusState` mengabaikan diam-diam fokus ke field di bagian yang belum terpasang;
  4. `Announcer.post(summary)` (`Accessibility.swift:6`).

  `testState` tidak disentuh. Ini memisahkan pesan validasi dari hasil Test (`:1025`, `:1091`).

### 5.7 Keyboard dan fokus

- **Pintasan tidak berubah:** Return = Save, Esc = Cancel, ⌘↩ = Test (`SheetShortcut`, `Shortcuts.swift:267-271`). Tidak ada pintasan baru, supaya `Shortcuts.swift` dan `ShortcutConflictTests` tidak tersentuh.
- **Urutan Tab:** item sidebar (Button, masuk key-view loop), lalu isi dari atas ke bawah, lalu footer dari kiri ke kanan. Menu enkripsi dan Aliases dibuka dengan Space.
- **Fokus awal:** Name untuk koneksi baru; tidak ada untuk edit.
- **Pindah bagian dari sidebar:** fokus tetap di sidebar, dan VoiceOver mengumumkan nama bagian.

### 5.8 Aksesibilitas

- **Item sidebar:** label = judul; value = "\(n) fields need attention" bila ada masalah; trait `.isSelected` untuk item aktif (pola `SettingsView.swift:191-192`). Glyph lencana `accessibilityHidden(true)`.
- **Kontrol di setiap baris** diberi `.accessibilityLabel(<label baris>)`. Field wajib diberi `.accessibilityHint("Required")`. Keterangan pendek menjadi hint kontrolnya.
- **Kontrol enkripsi:** label "Encryption", value deskripsi panjang (mis. "HTTPS, certificate verified").
- **Lantai font 11 pt**, kecuali `SectionLabel` (`FontFloorTests.swift:50`). Tombol ikon wajib punya nama (`ChromeAccessibilityTests.swift:12`). Token amber, coral, dan mint sudah diuji kontrasnya (`ChromeAccessibilityTests.swift:166-211`).

### 5.9 Teks

- **Dipertahankan:**
  - "New Connection", "Change Type…", "Unsaved changes";
  - label field yang ada, "All Databases", "Show System Schemas", "Show All";
  - kalimat `tlsNote`, petunjuk password, kalimat Safe Mode dan Environment, kalimat SSH dan CA;
  - "Remove saved…", "Test Connection", "Testing…", "Cancel", "Save".
- **Baru**, Inggris, milik QueryHive: "Server", "SSH Tunnel", "Limits", "Encryption", "HTTPS · verified", "Prefer · unverified", "Seconds", "Delete Connection…", "\(n) fields need attention", "Also see \(section).", "Only modes that check the certificate are offered while a CA file is set."
- **Perbaikan T-2:** "Connected · N databases" untuk MySQL.

## 6. State dan aliran data

### 6.1 `ConnectionDraft` (baru, `Models/ConnectionDraft.swift`)

Struct `Equatable`. Isinya field yang sekarang menjadi `@State` (`ConnectionsViews.swift:130-168`, 29 field): name … timeoutText, ditambah `secrets: ConnectionSecretPlan.Draft`, yang dipakai ulang beserta `removals` (`ConnectionFormIssues.swift:171-186`). `ConnectionSecretPlan.Draft` belum `Equatable` (`:171`); konformansinya ditambahkan di langkah 1.

| Anggota | Asal hari ini |
|---|---|
| `init(_ connection:)` | `load()` `:983-1009`. **Tidak** menormalkan apa pun (lihat di bawah) |
| `static func new()` | `:973-981` |
| `connection(id:group:)` | `draftConnection` `:203-232` |
| `formState`, `timeoutMS` | `:242-247`, `:236-240` |
| `choose(_ kind:)` | `:433-441`, lalu `enforceCAConstraint()` |
| `apply(_ parsed: ConnectionURL.Parsed)` | `:459-471`; menyetel scheme sebelum port; lalu `enforceCAConstraint()` |
| `setScheme(_:)` (port mengikuti) | `:535-544` |
| `setCAFile(_:)` | `:725-730`, lalu `enforceCAConstraint()` |
| `enforceCAConstraint()` (privat) | baru; satu-satunya tempat invarian "CA memaksa verifikasi" |
| `setTLS(_ choice:)` | baru |

**`enforceCAConstraint()`.** Hanya berlaku bila `caFile` tidak kosong, dan dipanggil di akhir `setCAFile`, `choose(_:)`, dan `apply(_:)`, tetapi **tidak** dari `init`:

- PostgreSQL: `sslmode` di luar verify-ca/verify-full menjadi `verify-full`, dan `verify = true`.
- Trino: lewat `setScheme("https")`, supaya port 8080 ikut menjadi 8443 seperti rantai `:729` → `:535` hari ini, lalu `verify = true`.
- MySQL tidak punya pengaturan CA (`:686-687`); `choose(_:)` ke MySQL mengosongkan `caFile` seperti `:439`.

Kasus yang ditutupnya, semuanya bisa dicapai hari ini:

- Trino tersimpan dengan CA, lalu Change Type ke PostgreSQL: `load()` sudah mengisi `sslmode = "disable"` (`:989`, bawaan Trino `Connections.swift:122`), `choose` membiarkannya karena tidak kosong (`:436`), dan CA ikut terbawa. Hasilnya PostgreSQL dengan CA dan `disable`, nilai yang tidak ada di opsi `:614`, dan engine mengabaikan CA tanpa kabar.
- URL yang diterapkan setelah Change Type dengan CA masih di draft (`:463`).
- PostgreSQL dengan CA dan `scheme` tersimpan `""`, lalu ke Trino, menjadi CA + HTTP (`Connections.swift:237-243`).

**Pengikatan.** Kolom CA, Choose…, Clear, dan menu enkripsi mengikat lewat `Binding` yang memanggil metode draft (`setCAFile`, `setTLS`, `setScheme`), bukan `$draft.caFile` atau `$draft.scheme` langsung. Bila implementer memilih `didSet`, `apply(_:)` harus tetap menyetel scheme sebelum port, supaya URL `http://…:8443` tidak kehilangan portnya.

**`init` tidak menormalkan.** Koneksi tersimpan yang sudah melanggar CA, atau yang PostgreSQL/MySQL-nya ber-`verify == false`, dibuka apa adanya. Normalisasi di `init` akan menandainya "dirty", dan untuk `prefer` justru menurunkan TLS dari require ke prefer. Menu menampilkan nilai tersimpan (B-1, §6.2). Normalisasi baru terjadi bila pengguna memilih atau mengisi CA.

Alasannya: view bagian yang ada di berkas lain tidak bisa membaca `@State` privat milik sheet. Satu binding juga menggantikan 43 `@State`, dan invarian tinggal di tipe, bukan di `.onChange` view yang bisa saja tidak terpasang.

### 6.2 `ConnectionTLSChoice` (baru)

- **Kasus:** `trinoHTTPSVerified`, `trinoHTTPSUnverified`, `trinoPrefer`, `trinoHTTP`, `sslmode(String, verified: Bool)`.
- **Membaca draft: `(kind, scheme, sslmode, verify)` untuk ketiga driver.**
  - Trino lewat `TrinoTransport(stored:)` (`Connections.swift:237-243`) ditambah `verify`.
  - PostgreSQL dan MySQL: `sslmode.isEmpty ? kind.defaultSSLMode : sslmode`, lalu `verify`. Bila `verify == false` dan kata bukan `disable`, kasusnya `sslmode(kata, verified: false)`: judul `"<kata> · unverified"`, glyph `lock`, `spokenDescription` yang menyebut sertifikat tidak diperiksa.
- **Menulis** (selalu lewat `ConnectionDraft.setTLS`):
  - HTTPS verified → `scheme = "https"`, `verify = true`;
  - HTTPS unverified → `"https"`, `false`;
  - Prefer → `"prefer"`, `verify` tidak disentuh;
  - HTTP → `"http"`, `verify` tidak disentuh;
  - PostgreSQL atau MySQL → `sslmode = kata`, **`verify = true`**.
- **Properti tampilan:** `title`, `spokenDescription`, `glyph`, `help`, dan `static func options(kind:caFile:dbAuth:)`. `dbAuth` JWT membatasi Trino ke dua pilihan HTTPS (N-7).
- **Tidak memakai `securityLabel`** (`Connections.swift:489-503`). Memakai atau menghapusnya adalah keputusan W13-T15 (`development-plan.md:954`); T15 bisa menghapusnya demi tipe ini.
- **Di luar T19:** impor yang menulis `verify: false` untuk PostgreSQL dan MySQL, dan pertanyaan apakah `DB_INSECURE` boleh dikirim di luar Trino (bagian "Tugas terpisah").

### 6.3 Bagian dan pemetaan masalah (`ConnectionFormIssues.swift`)

- Enum `ConnectionFormSection`: `general`, `sshTunnel`, `limits`, `appearance`, masing-masing dengan `title` dan `symbol`.
- `ConnectionField.section` (fungsi murni):
  - name, host, port, user, database, caFile → `general` (opsi A);
  - sshHost, sshPort, sshUser, sshKeyFile → `sshTunnel`;
  - timeout → `limits`.
- Ditambah `ConnectionFormIssues.bySection(_:)` dan `summary(_:visible:)`.

### 6.4 Sheet

- **State UI yang tetap di sheet:** `step`, `section`, `testState`, `testProcess`, `testRun`, `attemptedSave`, `draftID`, `editingID`, `confirmDelete`, `lastTestWasHostKey`, `aliasPreview`, `urlText`, `urlError`, `urlNote`.
- **`isDirty`** sama dengan `:191-199`, hanya dibaca dari draft.
- **Logika Save, Test, dan Delete tidak berubah**, kecuali sumber datanya draft dan validasi tidak lagi masuk `testState`.
- **`previewTestCount` harus selamat dari `load()`.** `load()` jalan di `onAppear` dan jangan sampai mereset hasil Test yang dititipkan scene `connection-tested`. Reset hasil Test hanya di binding yang ditulis pengguna atau lewat penanda "sudah dimuat" (§5.5), bukan `.onChange(of: draft)`.
- **Pengubah yang wajib tetap di root**, bukan di view bagian: `confirmationDialog` (`:281-285`), `onAppear` dan `onDisappear` (`:286-287`), `.hostKeySheet(active: true)` (`:290`), dan `onChange` atas `trustedCount` (`:293-295`). Test bisa ditekan dari bagian mana pun, sedangkan prompt host key datang dari engine.
- **Kait snapshot:** `revealTunnelForSnapshot` (`:187`) menjadi `initialSectionForSnapshot: ConnectionFormSection?`, ditambah `newKindForSnapshot: ConnectionKind?` untuk membuka koneksi baru langsung di form. Keduanya statis dan direset di `tearDown` ChromeParity (`:164`). `ConnectionEditorTarget` tidak diubah. Trik `ScrollViewReader` (`:415-425`) dihapus. Fokus ke field di bagian lain dipasang di `.task(id: section)` atau `.onAppear` bagian itu (§5.6).

### 6.5 Tanpa Rust dan FFI

- Env dibangun oleh `AppModel.connectionEnvironment`, yang tidak berubah (`AppModel+Connections.swift:914-1035`).
- Test memakai perintah `test` yang sudah ada.
- `SSHConfigAliases` tidak berubah (`ConnectionFormIssues.swift:34-44`).
- Bila implementer merasa butuh Rust: berhenti, catat penyimpangan, rencanakan ulang. Rantai FFI serial.

## 7. Migrasi data tersimpan

**Tidak ada.**

- Field `Connection`, kunci Codable, dan dekodenya tidak berubah (`Connections.swift:304-472`). Begitu juga `ConnectionsDocument` (`:673-697`) dan akun Keychain (`:877-899`).
- **Kasus tepi:**
  - Scheme Trino kosong atau tak dikenal dibaca sebagai HTTP (`Connections.swift:237-243`), dan baru ditulis bila pengguna memilih.
  - `sslmode` kosong ditampilkan sebagai bawaan driver, dan sejak perbaikan T-1 disimpan tetap kosong. Env-nya identik, karena kosong dikirim sebagai bawaan (`AppModel+Connections.swift:945-947`).
  - `verify` dipertahankan untuk HTTP dan Prefer.
  - PostgreSQL atau MySQL ber-`verify == false` (hasil impor, atau Change Type dari Trino unverified) dibuka apa adanya dan ditampilkan sebagai `"<kata> · unverified"` (§6.2); `ConnectionDraft.init` tidak menormalkan. Nilai baru `verify = true` baru ditulis bila pengguna memilih pilihan menu atau mengisi CA, dan itu perubahan data yang disengaja dan terlihat. `ConnectionTLSChoice` hanya membaca dan menulis field yang sudah ada.
  - Koneksi tersimpan yang melanggar invarian CA tidak dinormalkan saat dibuka (§6.1).
- **Rollback:** revert commit. Berkas data tetap cocok ke dua arah.

## 8. Rencana tes

### 8.1 Unit (G-SWIFT)

**`ConnectionDraftTests`:**

- bolak-balik `Connection` → draft → `Connection` identik untuk tiap jenis, dengan dan tanpa tunnel, JWT, CA, dan timeout;
- trim;
- tunnel mati mengosongkan field SSH saat Save, tetapi nilai tetap ada di draft;
- `choose` mereset JWT dan CA seperti `:438-439`;
- `apply(URL)`;
- `setCAFile` memaksa verify-full atau HTTPS+verify;
- `setScheme` menggeser port 8080↔8443;
- `enforceCAConstraint()` (B-2):
  - Trino+CA → Change Type ke PostgreSQL menghasilkan `verify-full` dan `verify == true`;
  - PostgreSQL+CA dengan scheme `""` → Trino menghasilkan HTTPS terverifikasi, bukan HTTP;
  - URL yang diterapkan setelah Change Type dengan CA tetap memenuhi invarian;
  - `setCAFile` pada Trino HTTP 8080 menghasilkan HTTPS terverifikasi di 8443;
  - `apply(URL)` untuk `http://…:8443` tidak kehilangan portnya;
  - `init` dari koneksi yang melanggar tidak menormalkannya;
- B-1: PostgreSQL ber-`verify == false` lalu `setCAFile` menghasilkan `verify == true`;
- T-1 (langkah 4): membuka koneksi ber-`sslmode` kosong tidak membuatnya "dirty". Fixture T-1 mencakup kasus lama yang hari ini juga terbuka "dirty":
  - timeout bukan kelipatan 1000 ms;
  - spasi di nama atau host hasil impor;
  - MySQL ber-`caFile`;
  - PostgreSQL ber-`dbAuth: jwt`;
  - scheme Trino `"HTTPS"` huruf besar (engine menolaknya di `config.rs:334`, tetapi `TrinoTransport` membacanya sebagai https).

**`ConnectionTLSChoiceTests`:**

- opsi per jenis, dengan dan tanpa CA, dan Trino dengan `dbAuth` JWT (hanya dua pilihan HTTPS);
- membaca nilai lama (scheme kosong, sslmode kosong);
- HTTP dan Prefer mempertahankan `verify`;
- tabel (B-1): PostgreSQL {prefer, require, verify-full} dan MySQL {prefer, require}, masing-masing × `verify` {true, false}. Setiap baris memeriksa judul yang tampil (`"<kata> · unverified"` untuk `false`) dan `DB_INSECURE`;
- setiap pilihan PostgreSQL atau MySQL yang dipilih menulis `verify == true`;
- untuk setiap pilihan, `AppModel.connectionEnvironment(_:secrets:)` menghasilkan `DB_SCHEME`, `DB_SSLMODE`, dan `DB_INSECURE` sesuai tabel `Connections.swift:220-225`;
- baris Trino HTTP + password (N-8): pilihan "HTTP" tetap menyimpan `verify`, dan keterangan menyebut kenaikan ke HTTPS.

**Tambahan di `ConnectionFormIssuesTests`:**

- setiap `ConnectionField` punya tepat satu bagian;
- `bySection` dan `summary` (dengan awalan bagian, dan bagian terlihat lebih dulu).

### 8.2 Rig render `ConnectionSheetTests` (berkas baru, gaya `ShellTests`)

Pola rig: `NSHostingView` di `NSWindow` nyata, `wait`, `descendants`, dan penanda `FrameMarker` (struct di `RootView.swift`) dengan id `connection-host-field`, `connection-encryption`, `connection-footer`, `connection-section-<raw>` (pola rig di `ShellTests.swift`). `wait` dan `descendants` `private` di `ShellTests.swift`, yang sedang kotor dan tidak boleh disentuh, jadi `ConnectionSheetTests.swift` menyalinnya.

1. `testTheEncryptionControlSitsOnTheHostRow`: selisih `midY` ≤ 2 pt, `minX` kontrol > `maxX` field, dan keduanya di dalam sheet. Diuji untuk tiga jenis.
2. `testTheSheetNeverAsksForMoreThanItsFrame`: untuk setiap jenis × bagian, ukur bagian yang **tidak menggulir** (sidebar, kepala, footer) **tanpa bingkai luar tetap** dan pastikan jumlah tingginya ≤ 620. Mengukur `fittingSize` sheet yang dibingkai `.frame(width: 820, height: 620)` selalu menghasilkan 820×620 dan tidak bisa gagal (B-3). Alternatifnya, periksa `contentMinSize.height ≤ 620` pada jendela sheet yang benar-benar dipresentasikan, dan pastikan `connection-footer` ada di dalamnya.
3. `testTheSheetFitsAWindowAtTheShellMinimum`: `RootView` di jendela 1400×700, `presentConnectionEditor(nil)`, sheet menempel, dan `connection-footer` utuh di dalam jendela sheet. Tes ini jalan di xctest yang di-stamp SDK 27 dan tidak pernah lewat jalur mode kompatibilitas (`ledger.md:640`), jadi ia **bukan** gate R-1. Gate R-1 adalah build SDK 14 plus probe di bawah.
4. `testTypedValuesSurviveASectionSwitch`: Host dan password yang diketik masih ada setelah General → SSH Tunnel → General.
5. `testTheGeneralBadgeFollowsTheMissingFields`, termasuk value aksesibilitasnya.
6. `testAHostKeyPromptAttachesFromAnySection`: `HostKeyCenter.shared.report(…)` saat berada di Limits membuat sheet bertingkat menempel.
7. `testSavingWithProblemsOpensTheirSection`: SSH host kosong saat berada di General; Save memindah ke SSH Tunnel.

**Gate R-1 (B-3), bukan tes xctest.** `--snapshot` menggambar ke PNG tanpa jendela (`App.swift`), jadi tidak mengukur apa pun soal tinggi sheet. Gate-nya: build SDK 14 dengan `./app/build.sh`, lalu `target/run/uiprobe/probe.sh` dengan `QH_SUPPORT_ROOT` sekali pakai (direktori sementara, supaya data pemilik tidak tersentuh). Probe membuka sheet di jendela 1400×700, memastikan footer terlihat, dan memastikan sheet tidak lebih tinggi dari jendela. Hasilnya dicatat di pesan commit langkah 3 dan 4.

### 8.3 ChromeParity dan V-15

- **Permukaan** `connectionSheet` menjadi 820×620 (`ChromeParityTests.swift:80`).
- **Scene** (7 × 2 tampilan):
  - `connection` (General, Trino tersimpan);
  - `connection-ssh` (bagian SSH Tunnel lewat kait statis);
  - `connection-trino` (Prefer, dengan keterangan);
  - `connection-mysql`;
  - `connection-new`: scene baru, PostgreSQL baru dengan lencana dan ringkasan;
  - `connection-limits`: scene baru;
  - `connection-appearance`: scene baru.
- **Rekam:** `QH_RECORD_CHROME=connection,connection-ssh,connection-trino,connection-mysql,connection-new,connection-limits,connection-appearance swift test --filter ChromeParityTests` (`:19-27`). Dua gambar identik sudah dijamin oleh rig.
- **Commit sendiri:** `test(visual): re-record chrome-connection scenes for V-15` (`handoff.md:44`). Verdict ui-ux-designer dan a11y harus approved (§0.5 `:92-94`).
- **Push ditahan sampai langkah 5.** Langkah 3 dan 4 sengaja meninggalkan ChromeParity merah, sedangkan O-21 hanya mendorong commit yang gate-nya hijau. Langkah 3 dan 4 commit lokal; ketiganya didorong bersama setelah rekam ulang langkah 5 hijau (§10).
- **Hanya lokal**, karena di-skip di CI (`ledger.md:588`). Harness 2x tetap, dan tampilan utama tetap jadi syarat (`ledger.md:289`).

### 8.4 Pemeriksaan manual pemilik

- Keadaan menu terbuka.
- Satu lintasan VoiceOver.
- Keychain nyata (`QH_TEST_KEYCHAIN=1` belum pernah jalan, `handoff.md:80`).

## 9. Risiko

- **R-1. Ukuran minimum sheet diambil dari tata letak** (mode kompatibilitas macOS 27; `RootView.swift:55-59`, `ledger.md:640`). Mitigasi: `.frame(minHeight: 0, maxHeight: .infinity)` pada ScrollView dan sidebar. Gate yang bisa gagal: tes §8.2 butir 2 (mengukur bagian yang tidak menggulir tanpa bingkai luar) **dan** build SDK 14 plus `target/run/uiprobe/probe.sh` dengan `QH_SUPPORT_ROOT` sekali pakai di jendela 1400×700, di langkah 3 dan 4 (§8.2, §10). Butir 3 xctest bukan gate R-1, karena xctest di-stamp SDK 27.
- **R-2. `NSWindow.backgroundColor` kustom menyembunyikan traffic light** (`ledger.md:640`). Gugur karena OQ-2 memilih tetap sheet; relevan lagi hanya bila kelak dibuka sebagai jendela. Jangan pernah menyetelnya; pakai latar SwiftUI.
- **R-3. Pengubah, popover, dan invarian di bagian yang tidak terpasang.**
  - Lihat §6.4 untuk pengubah root.
  - Popover `HelpHint` memakai state lokal (`Theme.swift:864-889`) dan aman. State `isPresented` untuk jangkar di bagian lain tidak boleh disimpan di sheet.
  - Invarian `:535-544` dan `:725-730` pindah ke draft (§6.1), dan invarian CA dijaga satu `enforceCAConstraint()` yang dipanggil dari `setCAFile`, `choose(_:)`, dan `apply(_:)`.
  - Fokus ke field di bagian yang belum terpasang diabaikan `@FocusState`; fokus dipasang di bagian itu sendiri (§5.6).
- **R-4. Rahasia yang sudah diketik hilang saat pindah bagian.** Mitigasi: state di draft; tes §8.2 butir 4.
- **R-5. Regresi jalur Save** (Keychain dulu `:1033-1050`, renew `:1059-1070`, `draftID` `:181-184`). Mitigasi: langkah 1 netral piksel, tes unit, review Tinggi.
- **R-6. Gate visual hanya lokal dan bergantung pada layar** (`ledger.md:289`, `:588`).
- **R-7. Perbaikan T-1 mengubah perilaku**: Save nonaktif saat koneksi lama dibuka. Ini disengaja dan dikunci tes.
- **R-8. Lisensi.** Reviewer mencocokkan setiap string baru dengan tangkapan layar TablePro; hanya angka yang dipelajari.
- **R-9. Kepemilikan.** `Snapshot.swift` dan `ChromeParityTests.swift` digabung tangan (§2.3). `ShellTests.swift` sedang kotor, jadi tes baru masuk berkas baru.
- **R-10. Lebar kolom.** Host 236 pt di 820 lebar hanya estimasi; nama host panjang menggulir di dalam field, dan label terpotong punya `.help`.
- **R-11. Menu tidak terlihat di baseline** (`ChromeParityTests.swift:48`). Logika pilihannya ditutup tes unit.
- **R-12. `verify: false` dari impor** membuat PostgreSQL dan MySQL hasil impor berjalan tanpa verifikasi walau katanya verify-full (§4.1). T19 hanya menampilkannya dengan jujur (`"<kata> · unverified"`); memperbaiki datanya adalah tugas terpisah (bagian "Tugas terpisah").
- **R-13. Item Keychain yatim** pada Cancel setelah tulis JSON gagal (§1.8 T-3) sudah ada sebelum T19. Langkah 1 mempertahankannya persis; perbaikannya backlog.
- **R-14. Tabrakan `app/DESIGN.md` dengan W14-T5** (§2.3). Langkah 6 hanya menyentuh bagian editor koneksi dan digabung tangan.

## 10. Urutan build

Satu commit per langkah. Anggaran bukti setiap langkah: satu tes terarah ditambah satu G-SWIFT penuh di mesin yang menganggur. G-RUST tidak diperlukan, karena tidak ada perubahan Rust. G-APP butuh SSD terpasang dan `CARGO_TARGET_DIR` di sana (`AGENTS.md`).

| Langkah | Isi dan berkas | Gate | Tingkat review | Commit |
|---|---|---|---|---|
| 0 | Orkestrator: §2.4 (plan §5 dan §7, PRD §6.5, plan §0.5, ledger), plus keputusan OQ-1..OQ-6 yang sudah terjawab, dan prasyarat ledger (bug Export dan rilis 0.2.2 beres, `docs/releases/v0.2.2.md` tidak lagi untracked) | review ringan dokumen | Rendah | `docs(plan): schedule the sectioned connection form as W13-T19` |
| 1 | Ekstraksi `ConnectionDraft`. Perilaku dan piksel tidak berubah: normalisasi `sslmode` lama dibiarkan dulu, dan `.onChange` lama memanggil metode draft. Berkas: `Models/ConnectionDraft.swift`, `Views/ConnectionsViews.swift`, `Models/ConnectionFormIssues.swift` (hanya konformansi `Equatable` untuk `ConnectionSecretPlan.Draft`), `ConnectionDraftTests.swift`. Perilaku Keychain yatim (§1.8 T-3) dipertahankan persis | G-SWIFT; `swift test --filter ChromeParityTests` hijau **tanpa** rekam ulang sebagai bukti netral; `--filter ConnectionDraftTests` | **Tinggi**: satu reviewer opus; ditambah reviewer keamanan hanya bila pemanggilan `ConnectionSecretPlan` atau `AppModel.apply` berubah makna | `refactor(connections): the connection form edits one draft value` |
| 2 | `ConnectionTLSChoice` dan `ConnectionField.section`, tanpa UI. Berkas: `Models/ConnectionTLSChoice.swift`, `Models/ConnectionFormIssues.swift`, dan kedua tesnya | G-SWIFT | Rendah; review digabung dengan langkah 3 | `feat(connections): name each driver's encryption choices once, over the stored fields` |
| 3 | Kerangka: sidebar, kartu, baris, footer, Delete pindah ke sidebar, kait snapshot, scene baru, ukuran ChromeParity. Isi pindah apa adanya; baris TLS lama masih ada. `SettingsCard` dan `RowDivider` disalin ke `ConnectionForm.swift`, `wait` dan `descendants` ke `ConnectionSheetTests.swift`. Berkas: `Views/ConnectionForm.swift`, `Views/ConnectionsViews.swift`, `Support/Snapshot.swift`, `ChromeParityTests.swift`, `ConnectionSheetTests.swift` (butir 2, 3, 4, 6, 7) | G-SWIFT; ChromeParity merah **hanya** untuk scene V-15 yang terdaftar di §8.3; `FontFloorTests` dan `ChromeAccessibilityTests` hijau; G-APP: `./app/build.sh` lalu `app/dist/QueryHive.app/Contents/MacOS/QueryHive --snapshot target/run/<scene>.png --scene <scene>` untuk tujuh scene; **gate R-1:** build SDK 14 (`./app/build.sh`) lalu `target/run/uiprobe/probe.sh` dengan `QH_SUPPORT_ROOT` sekali pakai di jendela 1400×700, footer terlihat dan sheet tidak lebih tinggi dari jendela, hasil dicatat di pesan commit. **Commit lokal saja; push ditahan sampai langkah 5.** | Sedang: satu reviewer opus dengan daftar periksa UX dan AX | `feat(connections): the connection form is four sections beside a sidebar` |
| 4 | Kontrol enkripsi di samping Host, CA di bawah Port, baris Transport/SSL/Verify dihapus, lencana dan ringkasan, validasi terpisah dari hasil Test, hasil Test reset saat ada perubahan, T-1, T-2. Berkas: sama dengan langkah 3, ditambah `ConnectionDraft.swift` dan tes (butir 1 dan 5) | sama dengan langkah 3, termasuk gate R-1 (build SDK 14 dan `uiprobe`); `enforceCAConstraint()` dan tabel `(sslmode, verify)` hijau di `ConnectionDraftTests` dan `ConnectionTLSChoiceTests`. **Commit lokal saja; push ditahan sampai langkah 5** | Sedang; boleh satu putaran review bersama langkah 3 | `feat(connections): encryption sits beside the host, and the footer names what is missing` |
| 5 | Rekam ulang 14 pasang baseline `chrome-connection*` (orkestrator) | rekam lalu bandingkan, hijau; verdict UX dan AX approved. **Setelah ini hijau, dorong langkah 3, 4, dan 5 bersama** (O-21) | (§0.5) | `test(visual): re-record chrome-connection scenes for V-15` |
| 6 | `app/DESIGN.md`, bagian editor koneksi (cari judul bagiannya; barisnya bergeser), digabung tangan dengan W14-T5 (§2.3), dan status ledger | review ringan | Rendah | `docs(connections): describe the sectioned connection form` |

- **OQ-3 dan OQ-4 dijawab "tidak"**, jadi langkah 4b tidak ada. Bila kelak dibalik, kerjakan sebelum langkah 5 sebagai langkah 4b (perilaku dan piksel baru, gate sama), supaya baseline direkam sekali saja.
- **Jendela sungguhan (OQ-2)** ditolak: tetap sheet. Bila kelak dibuka lagi, itu tugas lain di rantai `App.swift`, bukan bagian T19; latar `.background(Tone…)` di root dan sidebar ikut mengisi safe area dan akan masuk ke bawah title bar dan traffic light, jadi catat itu di tugas jendela.
- **Keychain yatim (§1.8 T-3)** dan **impor `verify: false`** tidak dikerjakan di T19 (bagian "Tugas terpisah").
- **Batas review:** maksimal dua putaran. Temuan yang tidak memblokir masuk backlog.

## 11. Titik temu dengan tugas lain

- **W13-T15:** menambah TLS hasil negosiasi dan versi server ke baris sukses di slot status footer (§5.5), lalu memutuskan nasib `securityLabel` (§6.2).
- **W13-T17 (bersyarat):** baris sertifikat dan kunci klien. Di opsi A masuk kartu Server di bawah CA; di opsi B masuk kartu Encryption.
- **W13-T10 (bersyarat):** tanda produksi per database masuk Limits.
- **W13-T18 (bersyarat):** pemilih tunnel bersama masuk SSH Tunnel.
- **W13-T11 (bersyarat):** hanya teks Safe Mode di `Connections.swift`; tidak memengaruhi tata letak.
- **W13-T4b:** tidak ada sentuhan bersama (§2.1).
- **Tugas terpisah impor `verify: false`:** menyentuh `AppModel+Connections.swift` (rantai W11-T3s → W13-T4b → W13-T16), bukan T19.
- **W13-T7:** mengatalogkan string T19 (§2.3).

## 12. Yang tidak bisa diverifikasi

- Tidak ada build atau tes yang dijalankan di sesi blueprint ini maupun di revisi 2.
- Tinggi sheet nyata di mode kompatibilitas macOS 27 belum diukur; gate-nya (build SDK 14 plus `uiprobe`, §8.2) ada di langkah 3 dan 4.
- Lebar teks dan kolom (168 pt kontrol, 236 pt host) hanya estimasi.
- Menu `.menuStyle(.button)` di sheet ini sudah ada preseden (menu "Import…", `ConnectionsViews.swift:331-352`); yang belum dicoba hanya isi label kontrol enkripsi (glyph, teks, chevron) dalam gaya itu.
- Apakah koneksi milik pemilik menyimpan `sslmode` kosong, atau PostgreSQL/MySQL ber-`verify == false`, tidak diperiksa: data pemilik sengaja tidak dibaca.
- TablePro hanya dibaca untuk angka ukuran.

## Tugas terpisah

**Impor menurunkan verifikasi TLS PostgreSQL dan MySQL (di luar T19).** Impor Navicat, DBeaver, dan DataGrip menulis `verify: false` (`AppModel+Connections.swift:499`, badan `applyImport` di `:391`), dan `applyTLS` (`:551-564`) hanya menyetel `verify` untuk Trino. Akibatnya PostgreSQL dan MySQL hasil impor dengan `ssl` berjalan sebagai `RequireNoVerify` (`crates/qh-ffi/src/config.rs:416-420`; MCP `mcp.rs:470-489`), walau katanya verify-full. T19 hanya menampilkannya dengan jujur (§4.2). Perbaikannya butuh tugas sendiri:

- **Tingkat Tinggi**, dengan reviewer keamanan.
- **Isi:** bawaan `verify` saat impor, dan pertanyaan apakah `DB_INSECURE` boleh dikirim di luar Trino. Pertanyaan kedua adalah kontrak bersama Swift dan Rust (`connection_env.json` belum punya kasus PostgreSQL atau MySQL ber-`verify: false`).
- **Butuh keputusan pemilik**, karena sebagian koneksi impor akan mulai memverifikasi sertifikat dan bisa gagal connect.
- **Rantai:** `AppModel+Connections.swift` W11-T3s → W13-T4b → W13-T16.

## Pertanyaan terbuka untuk pemilik

Keenam pertanyaan sudah **terjawab** lewat delegasi pemilik kepada orkestrator (13 Okt 2026, setelah verdict architect-reviewer). Teks pertanyaan dipertahankan sebagai catatan.

1. **OQ-1, penempatan TLS:** A (kontrol enkripsi di samping Host, direkomendasikan), B (bagian "SSH & TLS" dengan baris mode), atau A1 (checkbox harfiah, Trino saja)? **Terjawab: A** (§4.2).
2. **OQ-2:** tetap sheet, atau jendela sendiri dengan traffic light, bisa diubah ukurannya, dan tidak modal? Jendela sendiri adalah tugas terpisah. **Terjawab: tetap sheet**; R-2 gugur.
3. **OQ-3:** tambahkan "Save and Open" (simpan, buka tab query baru di koneksi itu, dan bentangkan node pohonnya), atau tidak ada padanan "simpan lalu connect"? **Terjawab: tidak ada "Save and Open" di tugas ini**; langkah 4b gugur.
4. **OQ-4:** baris Group di Appearance? Ini membalik keputusan bahwa grup adalah urusan pohon (`ConnectionsViews.swift:1040-1043`). **Terjawab: tidak ada baris Group**; `group` tetap dari `original` dan `newConnectionGroup`.
5. **OQ-5:** lencana dan ringkasan tampil sejak awal (rekomendasi), atau baru setelah Save atau Test pertama, seperti perilaku sekarang? **Terjawab: tampil sejak awal** (§5.6); form baru langsung amber, dan memang begitu maksudnya.
6. **OQ-6:** nama bagian General / SSH Tunnel / Limits / Appearance, setuju? **Terjawab: diterima** (§3.1).

## Verdict architect-reviewer

**Verdict: perlu perubahan.** Direview 13 Okt 2026 dari pohon kerja `main` (`4b7854b`, kotor oleh agen lain di `RootView.swift`, `ShellTests.swift`, dan enam berkas lain). Tidak ada build atau tes yang dijalankan. Arah besarnya benar: tetap sheet, satu `ConnectionDraft`, tanpa Rust atau FFI, pengubah root tetap di root, dan urutan Keychain lalu JSON tidak disentuh. Tiga temuan di bawah memblokir. Ketiganya bisa ditutup dengan menyunting blueprint ini, tanpa mengubah rancangannya, lalu dibaca ulang satu putaran.

### Temuan yang memblokir

**B-1. Pemetaan TLS PostgreSQL dan MySQL tidak tepat, karena `verify` juga berlaku untuk keduanya.** §4.1 ("Flag `verify` hanya untuk Trino"), §6.2 (driver lain dibaca dari `sslmode` saja), dan tabel tes §8.1 (hanya baris Trino) mengikuti komentar `Connections.swift:324`, bukan perilakunya.

- **Bukti:**
  - `AppModel+Connections.swift:952` mengirim `DB_INSECURE=1` untuk **setiap** driver bila `verify == false`.
  - Engine memakainya untuk PostgreSQL dan MySQL: `crates/qh-ffi/src/config.rs:416-420` menurunkan prefer, require, verify-ca, dan verify-full ke `RequireNoVerify`.
  - Jalur MCP sama: `crates/qh-ffi/src/mcp.rs:470-489`.
  - Setiap koneksi baru hasil impor Navicat, DBeaver, atau DataGrip ditulis dengan `verify: false` (`AppModel+Connections.swift:499`, satu badan `applyImport` di `:391`). `applyTLS` (`:551-564`) hanya menyetel `verify` untuk Trino.
  - Akibatnya, PostgreSQL dari DBeaver dengan `ssl=true` (`DBeaverImport.swift:409`, menjadi verify-full) berjalan **tanpa** verifikasi. PostgreSQL impor ber-`prefer` justru berjalan sebagai require.
  - Menambah CA lewat form tidak menolong: `ConnectionsViews.swift:728` hanya mengubah `sslmode`. "Change Type…" dari Trino yang unverified membawa `verify == false` ke PostgreSQL (`:433-441`).
  - Dengan opsi A, menu akan menampilkan "verify-full" dengan `lock.fill`, padahal sertifikat tidak diperiksa.
- **Perbaikan di dalam T19:**
  - `ConnectionTLSChoice` membaca `(kind, scheme, sslmode, verify)` untuk ketiga driver. Untuk PostgreSQL dan MySQL dengan `verify == false` dan kata selain `disable`, pilihan saat ini tampil jujur:
    - judul `"<kata> · unverified"`;
    - glyph `lock`;
    - `spokenDescription` yang menyebut sertifikat tidak diperiksa;
    - satu keterangan di bawah Host.
  - Setiap pilihan PostgreSQL atau MySQL di menu, `setCAFile`, dan `choose(_:)` ke driver non-Trino menulis `verify = true`.
  - `ConnectionDraft.init` **tidak** menormalkan. Normalisasi akan menandai koneksi "dirty", dan untuk `prefer` justru menurunkan TLS dari require ke prefer.
  - Tes `ConnectionTLSChoiceTests` ditambah baris PostgreSQL {prefer, require, verify-full} dan MySQL {prefer, require}, masing-masing × `verify` {true, false}. Setiap baris memeriksa `DB_INSECURE` dan judul yang tampil.
  - `ConnectionDraftTests` ditambah kasus "PostgreSQL ber-`verify == false` lalu `setCAFile` menghasilkan `verify == true`".
  - Teks §4.1, §6.2, §7, dan §8.1 dibetulkan.
- **Di luar T19, untuk orkestrator:** satu tugas Tinggi dengan reviewer keamanan. Isinya bawaan `verify` saat impor (`:499`; rantai `AppModel+Connections.swift` W11-T3s → W13-T4b → W13-T16) dan pertanyaan apakah `DB_INSECURE` boleh dikirim di luar Trino. Pertanyaan kedua adalah kontrak bersama Swift dan Rust (`connection_env.json` belum punya kasus PostgreSQL atau MySQL ber-`verify: false`). Butuh keputusan pemilik, karena sebagian koneksi impor akan mulai memverifikasi dan bisa gagal.

**B-2. Invarian "CA memaksa verifikasi" hanya dipasang di `setCAFile`, padahal jalur lain melanggarnya.** §6.1 menyalin `choose` (`:433-441`) dan `apply` (`:459-471`) apa adanya.

- **Bukti:** semua contoh berikut bisa dicapai hari ini.
  - Trino tersimpan dengan CA, lalu Change Type ke PostgreSQL:
    - `load()` sudah mengisi `sslmode = "disable"` (`:989`, bawaan Trino `Connections.swift:122`);
    - `choose` membiarkannya karena tidak kosong (`:436`);
    - CA ikut terbawa, karena hanya dikosongkan untuk MySQL (`:439`).
    - Hasilnya PostgreSQL dengan CA dan `disable`. Nilai itu tidak ada di opsi `:614`, dan engine mengabaikan CA tanpa kabar.
  - URL yang diterapkan setelah Change Type dengan CA masih di draft (`:463`) melanggarnya dengan cara yang sama.
  - PostgreSQL dengan CA dan `scheme` tersimpan `""`, lalu ke Trino, menjadi CA + HTTP (`Connections.swift:237-243`).
- **Perbaikan:**
  - Satu `enforceCAConstraint()` privat di `ConnectionDraft`, dipanggil di akhir `setCAFile`, `choose(_:)`, dan `apply(_:)`, tetapi tidak dari `init`:
    - PostgreSQL: `sslmode` di luar verify-ca/verify-full menjadi `verify-full`, dan `verify = true` (B-1);
    - Trino: lewat `setScheme("https")`, supaya port 8080 ikut menjadi 8443 seperti rantai `:729` → `:535` hari ini, lalu `verify = true`.
  - Kolom CA, Choose…, Clear, dan menu enkripsi mengikat lewat `Binding` yang memanggil metode draft, bukan `$draft.caFile` atau `$draft.scheme` langsung.
  - Bila implementer memilih `didSet`, `apply(URL)` harus menyetel scheme sebelum port, supaya URL `http://…:8443` tidak kehilangan portnya.
  - Koneksi tersimpan yang sudah melanggar tidak dinormalkan saat dibuka. Menu menampilkan nilai tersimpan sebagai judul walau nilai itu tidak ada di `options()`, dan tidak pernah kosong.
  - Tes:
    - Trino+CA → PostgreSQL;
    - PostgreSQL+CA → Trino dengan scheme `""`;
    - URL setelah Change Type;
    - `setCAFile` pada Trino HTTP 8080 menghasilkan HTTPS terverifikasi di 8443.

**B-3. R-1 (mode kompatibilitas macOS 27) tidak punya gate yang bisa gagal, jadi langkah 3 dan 4 salah gate.**

- **Bukti:**
  - Tes §8.2 butir 2 mengukur `fittingSize` sheet yang dibingkai tetap `.frame(width: 820, height: 620)`. Hasilnya selalu 820×620.
  - Butir 3 jalan di xctest yang di-stamp SDK 27 dan tidak pernah lewat jalur kompatibilitas (pesan commit `3a5957b`; `ledger.md:640`, "xctest is stamped SDK 27").
  - `--snapshot` di gate G-APP menggambar ke PNG tanpa jendela (`App.swift:4`).
- **Perbaikan:**
  - Butir 2 mengukur bagian yang tidak menggulir (sidebar, kepala, footer) tanpa bingkai luar, untuk setiap jenis × bagian. Alternatifnya, butir 2 memeriksa `contentMinSize.height ≤ 620` pada jendela sheet yang benar-benar dipresentasikan dan memastikan `connection-footer` ada di dalamnya.
  - Tambahkan gate langkah 3 dan 4: build SDK-14 (`./app/build.sh`), lalu `target/run/uiprobe/probe.sh` dengan `QH_SUPPORT_ROOT` sekali pakai. Probe membuka sheet di jendela 1400×700 dan memastikan footer terlihat serta sheet tidak lebih tinggi dari jendela. Hasilnya dicatat di pesan commit.

### Temuan yang tidak memblokir (backlog atau dikerjakan saat menyentuh baris itu)

- **N-1. Push ditahan.** Langkah 3 dan 4 sengaja meninggalkan ChromeParity merah, sedangkan O-21 hanya mendorong commit yang gate-nya hijau. Commit lokal dulu, dan dorong ketiganya bersama setelah langkah 5.
- **N-2. Reset hasil Test.** Mereset hasil Test lewat `.onChange(of: draft)` ikut terpicu oleh `load()` di `onAppear`. Itu menghapus `previewTestCount` (scene `connection-tested`), jebakan yang sama dengan komentar `:969`. Reset hanya di binding yang ditulis pengguna, atau lewat penanda "sudah dimuat".
- **N-3. Fokus ke bagian lain.** Fokus ke field di bagian yang belum terpasang diabaikan diam-diam oleh `@FocusState`. Pasang fokus di `.task(id: section)` atau `.onAppear` milik bagian itu, bukan di "putaran run loop berikutnya".
- **N-4. Komponen privat.** `SettingsCard` (`SettingsView.swift:209`) dan `RowDivider` (`:260`) bersifat `private` di berkas yang tidak boleh disentuh. Begitu pula `wait` dan `descendants` di `ShellTests.swift` (HEAD `:97`, `:115`). Tulis salinannya di `ConnectionForm.swift` dan `ConnectionSheetTests.swift`, dan sebutkan itu di §2.2.
- **N-5. Tabrakan `app/DESIGN.md`.** Berkas ini juga milik W14-T5 (`development-plan.md:1166`). Tambahkan ke aturan gabung tangan §2.3.
- **N-6. Prasyarat ledger.** `ledger.md:652` meminta pekerjaan ini dimulai setelah bug Export dan rilis 0.2.2 beres, sedangkan `docs/releases/v0.2.2.md` masih untracked. Orkestrator memastikan prasyarat itu sebelum langkah 1.
- **N-7. JWT dan pilihan Trino.** Trino dengan JWT plus Prefer atau HTTP ditolak engine (`config.rs:352-364`). `options(kind:caFile:)` sebaiknya juga menerima `dbAuth` dan hanya menawarkan dua pilihan HTTPS saat JWT.
- **N-8. HTTP yang naik ke HTTPS.** Pilihan "HTTP" Trino tidak selalu berarti tanpa enkripsi. Password, port 443, atau port 8443 menaikkannya ke HTTPS dengan `verify` tersimpan yang tersembunyi (`config.rs:385-393`). Keterangan di bawah Host untuk HTTP harus menyebut ketiganya, dan tabel tes memuat baris HTTP + password.
- **N-9. Lingkup fixture T-1.** Tes bolak-balik T-1 perlu fixture lama yang hari ini juga terbuka "dirty":
  - timeout bukan kelipatan 1000 ms;
  - spasi di nama atau host hasil impor;
  - MySQL ber-`caFile`;
  - PostgreSQL ber-`dbAuth: jwt`;
  - scheme Trino `"HTTPS"` huruf besar (engine menolaknya di `config.rs:334`, tetapi `TrinoTransport` membacanya sebagai https).
- **N-10. Draft rahasia belum `Equatable`.** `ConnectionSecretPlan.Draft` (`ConnectionFormIssues.swift:171`) belum `Equatable`, padahal `ConnectionDraft` membutuhkannya. Tambahkan konformansinya di langkah 1.
- **N-11. Item Keychain yatim (sudah ada sebelum T19, bukan dari blueprint).** Pada koneksi baru, bila Keychain tertulis (`:1035`) lalu tulis JSON gagal (`:1049`) dan pengguna menekan Cancel, item di bawah `draftID` tertinggal. Langkah 1 harus mempertahankan perilaku ini persis, karena langkahnya netral. Perbaikannya (hapus slot yang baru di-`set` bila JSON gagal untuk koneksi baru) masuk backlog tersendiri. Selain itu, tidak ada jalur baru di blueprint yang bisa menghilangkan rahasia atau data.
- **N-12. Klaim yang basi atau salah:**
  - "Dua puluh enam `@State`" (§Ringkasan 5): yang benar 43, dengan 29 field draft di `:130-168` ditambah `removals`.
  - `SettingsView.swift:64` adalah "Editor"; "Appearance" ada di `:63`.
  - §12 "Menu `.menuStyle(.button)` di sheet belum dicoba": sheet ini sudah punya menu Import… dengan gaya yang sama (`ConnectionsViews.swift:331-352`), dan menu itu contoh yang lebih tepat daripada `ConnectionPickerButton`.
  - `RootView.swift:250` dan `ShellTests.swift:539-545` benar di HEAD, tetapi bergeser ke `:254` dan `:571` di pohon kerja yang kotor.
- **N-13. Latar bila OQ-2 dibuka lagi.** `.background(Tone…)` di root dan sidebar ikut mengisi safe area. Di sheet itu aman, tetapi bila OQ-2 kelak menjadi jendela, latar itu akan masuk ke bawah title bar dan traffic light. Catat di tugas jendela.

### Klaim yang dicek dan benar

- **`ConnectionsViews.swift`:** 272, 281-287, 290, 293-295, 433-441, 443-473, 475-498, 502-533, 535-544, 610-644 (ChipToggle 629-630), 725-730, 733-800, 826-839, 865-877, 883-912, 922-958, 960-1010 (989), 1021-1078 (1033-1050, 1040-1047, 1059-1070, 1074-1076), dan 1086-1136.
- **`Connections.swift`:** 7-8, 68, 88/90/93, 98-122, 218-225, 237-243, 376, 382, 417-422, 489-503, 552-583, 631-642, 673-697, 707, 782-800, 841-845, 873, 877-888, dan 899.
- **`ConnectionFormIssues.swift`:** 34-44, 103-127, 110, 136-145, dan 156-214.
- **`AppModel+Connections.swift`:** 170-179, 914-1035, dan 945-952.
- **`ChromeParityTests.swift`:** 19-27, 48-50, 80, 109-111, dan 164.
- **`Snapshot.swift`:** 297-299, 552-556, dan 966-1012.
- **Lain-lain:** `RootView.swift:19`, `:55-59`, dan `:80-83`; `Theme.swift` 392-400, 421-425, dan 864-889; `development-plan.md` 89-95, 954, 1082, 1083, dan 1110.

### Butir yang diminta

1. **Klaim `path:baris`:** lebih dari 40 dicek. Yang salah atau basi tercantum di N-12, dan klaim perilaku di §4.1 dibahas di B-1.
2. **Tanpa Rust, FFI, atau migrasi:** benar, asalkan B-1 diperbaiki di sisi Swift. Perubahan kontrak env adalah tugas lain.
   - Urutan Keychain lalu JSON, pemakaian ulang `draftID`, prinsip "kosong = pertahankan", dan tempat `.hostKeySheet` tetap terjaga.
   - Port mengikuti scheme bila `setCAFile` lewat `setScheme`.
   - CA memaksa verifikasi baru utuh setelah B-2.
3. **Kepemilikan:** usulan rantai §2.1 masuk akal, dan W13-T4b memang hanya menyentuh `connectionEnvironment`. Tambahan ada di N-4 dan N-5.
4. **Urutan build:** keenam langkah kecil dan bisa hijau sendiri-sendiri, kecuali gate R-1 (B-3) dan aturan push (N-1). Risiko popover dan pengubah yang tidak terpasang, serta `Menu` di sheet, sudah tertutup (§6.4, R-3, N-12).
5. **Kontrol TLS:**
   - Empat pilihan Trino cocok dengan `connectionEnvironment` (`:940-952`): HTTPS verified/unverified mengubah `verify`; Prefer mengirim `DB_SSLMODE=prefer` tanpa `DB_INSECURE`; HTTP menyimpan `verify` (N-8).
   - Kata `sslmode` cocok, termasuk `""` yang dibaca sebagai bawaan. Pemetaannya belum tepat untuk PostgreSQL dan MySQL ber-`verify == false` (B-1) dan untuk nilai lama yang melanggar CA (B-2).
6. **Permintaan pemilik:** terpenuhi. Kontrol duduk di samping Host, dan tata letaknya sidebar, baris label dan nilai, serta footer.
   - Tidak ada teks atau kode TablePro. Nama bagian berbeda dari referensi (`ledger.md:651`), dan "General" serta "Appearance" sudah dipakai Settings QueryHive.
   - Angka 820×620 adalah fakta ukuran yang dipelajari, bukan salinan.

### Keputusan pemilik yang didelegasikan

OQ-1 A (menu enkripsi ringkas di samping Host): tanpa cacat sendiri, tetapi memperlihatkan B-1 dan B-2. OQ-2 tetap sheet: tanpa cacat, dan R-2 gugur. OQ-3 tanpa "Save and Open": tanpa cacat, dan langkah 4b gugur. OQ-4 tanpa baris Group: tanpa cacat, karena `group` tetap dari `original` dan `newConnectionGroup`. OQ-5 lencana dan ringkasan sejak awal: tanpa cacat; form baru langsung amber, dan memang begitu maksudnya. OQ-6 nama bagian General / SSH Tunnel / Limits / Appearance: tanpa cacat.

### Revisi 2

Direvisi 13 Okt 2026 dari verdict di atas (teks verdict tidak diubah). **B-1** ditutup di §4.1 (`verify` berlaku untuk ketiga driver), §4.2 (judul `"<kata> · unverified"`, pilihan menulis `verify = true`), §6.2 (`ConnectionTLSChoice` membaca `(kind, scheme, sslmode, verify)`), §7 (`init` tidak menormalkan), §8.1 (tabel PostgreSQL/MySQL × `verify`), dan bagian "Tugas terpisah" (impor `verify: false`, di luar T19). **B-2** ditutup di §6.1 (`enforceCAConstraint()` dari `setCAFile`, `choose(_:)`, `apply(_:)`, bukan dari `init`; pengikatan lewat metode draft), §Ringkasan butir 5, R-3 di §9, dan §8.1 (empat tes B-2). **B-3** ditutup di §8.2 (butir 2 mengukur bagian yang tidak menggulir tanpa bingkai luar, butir 3 bukan gate R-1, gate build SDK 14 plus `uiprobe`), §5.1, R-1 di §9, dan langkah 3 dan 4 di §10. Temuan tidak memblokir: **N-1** di §8.3 dan §10 (push ditahan sampai langkah 5); **N-2** di §5.5 dan §6.4 (`previewTestCount` selamat dari `load()`); **N-3** di §5.6, §6.4, dan R-3; **N-4** di §2.2, §5.4, §8.2, dan langkah 3; **N-5** di §2.3, R-14, dan langkah 6; **N-6** di §2.4 butir 7 dan langkah 0; **N-7** dan **N-8** di §4.1, §4.2, §6.2, dan §8.1; **N-9** dan **N-10** di §8.1 dan langkah 1; **N-11** di §1.8 (T-3), R-13, dan §2.4 butir 6 (backlog, bukan tugas ini); **N-12** di §Ringkasan butir 2 dan 5 (43 `@State`, 29 field draft; `Pane.title` di `:61`/`:63`), §1.7 dan §8.2 (nama simbol untuk `ShellTests` dan `FrameMarker`), §4.2 dan §12 (preseden menu "Import…"); **N-13** di §10. Keenam pertanyaan terbuka dicatat terjawab di bagian "Pertanyaan terbuka untuk pemilik".
