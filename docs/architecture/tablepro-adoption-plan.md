# Rencana adopsi: dari analisis TablePro ke QueryHive

> Pasangan dari [`tablepro-feature-analysis.md`](tablepro-feature-analysis.md), yang memuat bukti dan
> matriks celahnya. Dokumen ini hanya memuat apa yang dikerjakan, dalam urutan apa, dan bagaimana
> sebuah irisan dinyatakan selesai.
>
> **Status:** Fase 0 dan Fase 1 selesai, keduanya diukur dan dicatat di §2 dan §3. Fase 1 ditutup
> 29 Sep 2026 oleh session restore (§3). Fase 2 mendarat 29 Sep 2026 dan diukur di §4, dengan satu
> caveat yang disebut di sana (kliennya digulung sendiri, bukan klien MCP pihak ketiga). Fase 3
> mendarat pada tanggal yang sama dan diukur di §5. Fase 4 sampai 5 belum disentuh.

## 0. Prinsip dan aturan gate

**Tidak menyalin kode.** TablePro AGPL-3.0, pohon ini MIT. Semua item di bawah adalah pekerjaan baru
yang ditulis di sini. Setiap irisan yang mengambil pola dari TablePro menyebut alasannya, bukan
sumber kodenya.

**Sebuah irisan selesai hanya kalau perintah verifikasinya dijalankan dan hasilnya dicatat.** Angka
yang belum diukur ditulis `belum diukur`, mengikuti kebijakan `docs/benchmarks.md`.

**Gate berat, dijalankan sebelum fase apa pun ditutup:**

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check licenses
cd app && swift build && swift test
/usr/bin/python3 tools/golden/live_cases.py
./app/build.sh
```

**Satu ADR per keputusan yang mengubah kontrak.** Yang sudah pasti akan diminta: permukaan UniFFI
untuk perintah baru (Fase 1), model token dan scope MCP (Fase 2), dan timeout statement sebagai
bagian trait driver (Fase 3). Formatnya mengikuti `docs/decisions/0001` dan seterusnya.

## 1. Ringkasan fase

| Fase | Isi | Ukuran | Risiko | Alasan urutannya |
|---|---|---|---|---|
| 0 | CI, kontrak jujur, berkas invariant, golden live | S | rendah | Menjaga semua fase berikutnya |
| 1 | Query history dan saved queries | S | rendah | Tabelnya sudah ada, gap-nya terbesar per jam kerja |
| 2 | MCP server read-only | M | sedang | Permintaan tertinggi, dan primitifnya sudah lengkap |
| 3 | Query timeout dan Safe Mode | M | sedang | Keselamatan, dan menyentuh trait driver sekali jalan |
| 4 | Editor harian: find/replace, folding, sort, JSON viewer | M | rendah | Biaya kecil, dipakai tiap hari |
| 5 | Impor CSV/XLSX, ekspor Parquet, insert/delete baris | M | sedang | Perpindahan data, inti pekerjaan data |
| 6 | Kandidat besar, belum dijadwalkan | L | tinggi | Butuh keputusan produk lebih dulu |

Fase 0 dan 1 bisa jalan bersamaan. Fase 2 tidak bergantung pada 1, tapi sebaiknya menunggu CI
mendarat supaya binernya punya penjaga sejak awal.

## 2. Fase 0: fondasi

Tujuannya dua: pohon ini punya penjaga otomatis, dan kontraknya berhenti berbohong.

**0.1 `.github/workflows/repo-hygiene.yml`.** Runner `ubuntu-latest`, isinya `actionlint` untuk
seluruh workflow dan `shellcheck --severity=warning` untuk seluruh skrip di `app/*.sh`,
`tools/*.sh`, dan `deploy/`. Tanpa langkah ini, `app/release.sh` yang mem-push tag dan menyentuh
GitHub Releases tidak diperiksa apa pun.

**0.2 `.github/workflows/verify.yml`.** Runner `macos-latest`, karena pohon ini bergantung pada hal
yang tidak ada di Linux. Yang sudah terverifikasi ada di sini: `crates/qh-credentials` memakai
Keychain, dan `rusqlite` dibangun dengan fitur `bundled` (`Cargo.toml:62`), jadi membangun seluruh
workspace di Linux bukan asumsi yang aman. Isinya gate berat dari §0 minus `live_cases.py`, karena
kasus live butuh container Trino.

Langkah pertama sebelum menulis workflow-nya: ukur mana yang benar-benar portabel.

```bash
cargo check --workspace --exclude qh-credentials
```

Kalau hasilnya bersih, job Rust bisa dipindah ke `ubuntu-latest` dan hanya job Swift yang tinggal di
macOS. Itu menghemat menit runner, jadi ukur dulu, jangan tebak.

**0.3 Rekonsiliasi `app/DESIGN.md` dengan kode.** Bagian Scope (baris 18 sampai 19) menyatakan empat
hal out of scope yang sudah mendarat: result grid, edit data, file `.sql`, dan notarization. Perbaiki
daftarnya, lalu tambahkan catatan pembacaan di kepala berkas seperti yang sudah dilakukan
`docs/architecture/folder-proposal.md`, yang menyebut apa yang berubah dan kapan. Berkas itu menyebut
dirinya kontrak dan meminta pelapor berhenti saat ada konflik, jadi selama daftarnya salah, setiap
sesi agen berikutnya berpotensi berhenti pada konflik palsu.

**0.4 `docs/invariants.md`.** Berkas baru untuk jebakan, bukan untuk keputusan. ADR sudah memuat
keputusan; yang belum ada tempatnya adalah hal seperti urutan `Engine.current` terhadap
`DatabaseEngine`, mengapa seam injeksi masih belum bisa dipakai (`Support/DatabaseEngine.swift:14`),
dan mengapa `DriverKind` tertutup padahal komentarnya tidak. Tiap entri menyebut gejala, path, dan
commit atau tes yang menahannya. Aturan: satu entri masuk hanya setelah ada kejadian nyata, bukan
spekulasi.

**0.5 Golden live dijadikan terukur, bukan prosa.** `PROGRESS.md` mencatat 12/22 lulus dan
`docs/golden-deltas.md` mencatat 17 identik dari 43 kasus. Dua angka ini benar per 24 Sep 2026 dan
tidak ada yang menjalankannya berkala. Yang dikerjakan: jalankan ulang `tools/golden/live_cases.py`,
perbarui angka di kedua berkas dengan tanggal baru, dan klasifikasikan 10 kasus yang gagal sesuai
kategori yang sudah dipakai `docs/golden-deltas.md`. Perbedaan yang belum masuk satu kategori mana pun
adalah regresi dan harus dilaporkan, bukan ditulis sebagai "selisih yang diketahui".

**Kriteria selesai Fase 0.** Kedua workflow hijau pada satu pull request. `app/DESIGN.md` tidak lagi
menyebut out of scope hal yang sudah ada. `docs/invariants.md` ada dengan minimal tiga entri nyata.
Angka golden di `PROGRESS.md` dan `docs/golden-deltas.md` bertanggal dan cocok dengan hasil jalannya
terakhir.

**Hasil 29 Sep 2026.** Gate berat dijalankan di mesin ini dan hijau: `cargo fmt --all --check`
bersih, `cargo clippy --workspace --all-targets -- -D warnings` bersih, `cargo test --workspace`
589 lulus / 0 gagal, `cargo deny check licenses` melaporkan `licenses ok`, `swift build` selesai, dan
`swift test` 161 tes lulus / 0 gagal.

Dua angka yang sengaja tidak lagi dipatok sebagai fakta tetap. Hitungan tesnya naik di setiap irisan,
jadi angka di sini adalah larian terakhir dan bukan janji. Dan `app/Generated/`: `./app/build-ffi.sh`
idempoten, tapi hasilnya memang **berbeda** dari yang di-commit selama empat varian `EngineCommand`
yang baru belum ikut di-commit, jadi "identik dengan yang di-commit" hanya berlaku sesudah commit,
bukan sebelum.

Dua hal yang belum: workflow-nya belum pernah jalan di GitHub, jadi "hijau" di sini berarti
perintahnya lulus lokal dan YAML-nya parses, bukan bahwa CI sudah membuktikannya. Dan kasus golden
live baru sebagian.

**Golden live, 29 Sep 2026, sebagian.** `deploy/dev/up.sh postgres` dinyalakan (hanya postgres;
image-nya sudah ada lokal, MySQL dan Trino belum diunduh), lalu delapan kasus PostgreSQL dijalankan:
`postgres_catalogs_live`, `postgres_tables_live`, `postgres_objects_live`, dan `postgres_count_live`
cocok; `postgres_type_zoo_live`, `postgres_batching_live`, `postgres_explain_live`, dan
`postgres_export_live` berbeda pada kunci `columns`. Keempatnya **sudah terklasifikasi**, dan tidak
ada entri baru yang dibutuhkan: `columns` adalah `D-8`, dan D-8 sendiri menyatakan bahwa kasus live
yang hanya berbeda di medan itu diklasifikasikan olehnya. Dua selisih sisanya juga sudah tertutup.
Baris data `postgres_type_zoo_live` berbeda di enam sel yang semuanya milik D-1 (`tiny_negative`),
D-2 (`an_interval`, `a_uuid`), dan D-9 (`ints`, `texts`, `nested`). Dan `files.bytes` pada
`postgres_export_live`, 508 menjadi 499, adalah akibat D-9 yang angkanya sudah tertulis di sana.

Yang ditambahkan pada 29 Sep 2026 karena sebelumnya tidak ada: D-8 kini menyebut kasus mana saja yang
dijelaskannya, dan D-2 kini menyebut sel `a_uuid` yang ikut kehilangan kutip JSON-nya, yang penting
karena sel itu bagian dari selisih byte di D-9. Empat belas kasus sisanya belum dijalankan.

Satu jebakan lingkungan yang ditemukan saat itu dan layak dicatat: sebelum container dinyalakan, ada
PostgreSQL lain yang mendengarkan `127.0.0.1:55432` di mesin ini, dan karena `up.sh` menerbitkan
port container di alamat IPv6, koneksi ke `127.0.0.1` diarahkan ke Postgres lokal itu, bukan ke
container. Gejalanya bukan pesan port, melainkan kegagalan login. Postgres lokal itu berhenti
mendengarkan sebelum pengukuran di atas selesai; `up.sh` tidak menyentuhnya (tidak ada `lsof`, `kill`,
atau `pkill` di dalamnya) dan tidak ada perintah yang dijalankan untuk menghentikannya.

## 3. Fase 1: query history dan saved queries

Ini celah paling murah di seluruh analisis, karena skemanya sudah hidup. Yang kurang cuma jalannya.

**1.1 Perintah baru di permukaan engine.** Tambah ke `Command` di `crates/qh-ffi/src/lib.rs:464`.
Bentuknya mengikuti perintah yang sudah ada (satu argumen, pembacaan lewat environment di CLI, JSON
per baris di stdout):

| perintah | isi |
|---|---|
| `history` | daftar riwayat, dengan filter rentang dan pencarian |
| `history_add` | catat satu eksekusi, termasuk koneksi, durasi, jumlah baris, dan status |
| `history_clear` | hapus riwayat, seluruhnya atau per koneksi |
| `saved_queries` | daftar, baca, tulis, hapus, ganti nama |

**1.2 FTS5. Selesai 29 Sep 2026, dan rencananya keliru.** Rencana ini menyebut fitur `fts5` perlu
dinyalakan di `rusqlite`. Itu salah pada dua hitungan: `libsqlite3-sys` sudah mengompilasi FTS5 di
build `bundled` (`libsqlite3-sys-0.35.0/build.rs:132`, `-DSQLITE_ENABLE_FTS5`), dan rusqlite 0.37
tidak punya fitur bernama `fts5` sama sekali, jadi menambahkannya justru menggagalkan resolusi
dependensi. Tidak ada perubahan `Cargo.toml`, dan tidak ada `cargo deny` yang perlu dijalankan ulang
untuk alasan ini. Yang mendarat sebagai gantinya: `crates/qh-storage/migrations/0003_history_search.sql`
dengan tabel FTS5 eksternal di atas `query_history`, tiga trigger yang menjaganya, dan backfill untuk
baris yang sudah ada sebelum migrasi itu.

**1.3 Tabel yang sudah ada dipakai.** `query_history` dan `saved_query` ada di
`crates/qh-storage/migrations/0001_init.sql:44` dan `:52`, dengan index `idx_history_dedupe` dan
`idx_history_started` yang sudah dibuat. `session_restore` (baris 59) juga belum tersambung; ini
saat yang tepat untuk memasangnya sekaligus supaya tab kembali setelah restart, seperti yang
TablePro lakukan.

**1.4 Tulis dari setiap jalur eksekusi.** Riwayat harus terisi dari `run`, `runScript`, `export`,
`to_table`, `explain`, dan `countRows`, bukan hanya dari Run. Kalau salah satu jalur terlewat, yang
hilang justru perintah yang paling mahal.

**1.5 UI.** Panel History di `Views/Panels.swift`, dengan kolom waktu, koneksi, durasi, dan jumlah
baris; pencarian; double-click memuat statement kembali ke `Views/SQLEditor.swift`; dan daftar
favorit di `Views/SidebarTree.swift`. Tab **Data** baru di `Views/SettingsView.swift` untuk batas
riwayat dan sakelar apakah perintah dicatat.

**Kriteria selesai Fase 1.** Tes storage untuk skema, migrasi, dedupe, dan jalur revive; tes FFI
untuk empat perintah baru di `crates/qh-ffi/tests/local.rs`; `swift test` hijau; dan bukti manual:
jalankan satu query, tutup app, buka lagi, riwayatnya masih ada dan pencariannya menemukannya.

**Hasil 29 Sep 2026.** Sisi engine-nya sudah mendarat. `crates/qh-storage/src/history.rs` berisi
lapisan penyimpanan lengkap dengan tesnya; empat perintah ada di `crates/qh-ffi/src/local.rs`; dan
`EngineCommand` bertambah empat varian sehingga app benar-benar bisa memanggilnya. Gate:
`cargo fmt --all --check` bersih, clippy bersih dengan `-D warnings`, `cargo test --workspace` lulus
dengan 0 gagal (588 saat irisan ini ditulis, dan hitungannya bertambah di irisan sesudahnya), `swift
test` 161 tes / 0 gagal, `cargo deny check licenses` melaporkan `licenses ok`, dan `app/Generated/`
diregenerasi.

Bukti fungsionalnya lewat binari asli, bukan unittest: `history_add` dua kali untuk event yang sama
mengembalikan id yang sama dengan `merged:true`, `history` membacanya kembali lengkap,
`saved_queries` menyimpan lalu menampilkannya, `history_clear` melaporkan `cleared:1`, lalu daftarnya
kosong.

Dua hal yang ditemukan di jalan dan sudah diperbaiki. Pertama, pesan kegagalan connect PostgreSQL
hanya berbunyi `db error` karena `Display` dari `tokio_postgres::Error` memang begitu; sekarang
alasannya dibaca dari `as_db_error()`. Kedua, menambah perintah FFI menyentuh empat daftar, dan
pengawas di sisi Rust ternyata tautologi sehingga empat perintah baru sempat tidak bisa dipanggil app
tanpa ada satu tes Rust pun yang gagal. Yang menangkapnya `swift test`. Yang kedua punya entri di
`docs/invariants.md`, nomor 11. Yang pertama belum punya entri di sana, dan itu celah: pesan kegagalan
connect itu akan ditulis ulang lagi oleh orang berikutnya yang tidak tahu kenapa `db error` tidak
cukup, dan tidak ada halaman yang memberitahunya.

**Sisi app, ditambahkan 29 Sep 2026.** `AppModel.preview` kini memanggil `history_add` di `onExit`,
jadi setiap Run meninggalkan satu baris riwayat: pernyataannya, koneksinya, waktu mulai, elapsed,
jumlah baris, dan hasilnya, yaitu `ok`, `error`, atau `cancelled`. Hanya Run yang mencatat. Explain
tidak, karena rencana bukan hasil. Export tidak, karena riwayat yang mencampur "saya melihat ini"
dengan "saya menulis ini ke suatu tempat" tidak menjawab keduanya. Panggilannya fire-and-forget,
dan `history_add`-nya yang men-dedupe, jadi dua kali selesai pada satu run tetap satu baris. Bentuk
env yang dikirim sudah diverifikasi lewat binari asli dengan `DB_PATH` menunjuk database sementara,
untuk jalur `ok` maupun jalur `error`.

**Penampilnya, ditambahkan 29 Sep 2026.** `PanelTab` bertambah `history` dan `saved`, jadi panel di
bawah editor sekarang punya lima tab: Result, Log, Files, History, Saved. History menampilkan riwayat
terbaru dulu dengan lambang hasilnya, jumlah baris, dan elapsed; mengklik satu baris menaruh
pernyataannya kembali ke editor. Saved menampilkan daftar, menyimpan pernyataan yang sedang di editor
lewat kolom nama inline, memuatnya kembali, dan menghapusnya. Keduanya membaca lewat perintah yang
sama yang dipakai CLI, jadi tidak ada jalur data kedua yang harus dijaga.

Bentuk wire keempat peristiwa itu dikunci oleh tiga tes baru di
`app/Tests/TrinoExporterTests/EventDecodingTests.swift`, dengan contoh verbatim dari keluaran engine,
karena `tests/golden` belum memuat kasus untuk perintah-perintah ini.

**Pencarian, ditambahkan 29 Sep 2026.** Migrasi `0003_history_search.sql` memasang tabel FTS5
eksternal di atas `query_history`, dengan tiga trigger yang menjaganya dan backfill untuk baris yang
sudah ada. `Storage::search_history` dan kunci `HISTORY_SEARCH` di perintah `history` menyambungkannya
ke app, dan panel History punya kolom pencarian yang men-debounce 250 ms supaya satu kata tidak
menjalankan satu perintah per huruf.

Setiap kata pengguna menjadi frasa berkutip dengan awalan `*`, lalu di-AND-kan. Kutipnya yang membuat
`select *` dan `don't` jadi pencarian alih-alih error sintaks FTS5, dan `*`-nya yang membuat
`penerima` menemukan `penerima_manfaat` meski `unicode61` menganggap yang kedua satu token. Sembilan tes storage, satu di antaranya untuk backfill baris lama, plus satu tes FFI menutup jalur
itu, termasuk input yang sintaksnya berbahaya.

**Fase 1 belum selesai.** Kalimat di dokumen ini sempat berbunyi "semua kriteria selesai Fase 1
terpenuhi kecuali bukti manualnya", dan itu berlebihan: §1.5 sendiri meminta favorit di
`SidebarTree`, kolom koneksi di baris riwayat, dan tab Data di `SettingsView`.

Dua dari tiga mendarat 29 Sep 2026. Tab Data berisi batas riwayat dan sakelar pencatatan: keduanya
preferensi sungguhan yang bertahan di `UserDefaults` (`AppModel.historyLimit` dan `recordsHistory`,
pola yang sama dengan `shortcutScheme`), bukan lagi angka tetap `200` di kode. Baris riwayat
menampilkan nama koneksinya di depan, karena pernyataan yang sama terhadap dua database adalah dua
fakta yang berbeda. Sakelarnya dijaga di satu tempat, `recordHistory`, yang menerima `recording: Bool`
alih-alih membaca `UserDefaults` sendiri.

**Favorit, mendarat 29 Sep 2026, dan itu menutup §1.5.** Bentuknya: kolom `favourite` pada
`saved_query` lewat migrasi 0004, aksi `favourite` pada perintah `saved_queries`, bintang di baris
panel Saved, dan bagian FAVOURITES di atas pohon objek pada sidebar.

Tiga keputusan yang layak dicatat, karena ketiganya bisa saja sebaliknya. Favorit itu **kolom, bukan
tabel**: tabel favorit akan menjadi identitas kedua untuk baris yang sama, bebas berbeda pendapat
soal apakah baris itu masih hidup. Menyalakan favorit itu **revisi**, seperti rename, supaya
perangkat yang merekonsiliasi baris ini melihat satu suntingan; tapi mengulang permintaan yang sama
**bukan** revisi, karena revisi yang mengabarkan tidak ada perubahan akan membuat setiap rekonsiliasi
melihat suntingan yang tidak pernah terjadi. Dan bintangnya **selalu** tergambar, tidak muncul saat
pointer mendekat, karena favorit yang menyembunyikan diri tidak memberi tahu siapa pun apa pun.

Nilai `FAVOURITE` yang ketiga ditolak dengan menyebut namanya, bukan dibaca sebagai `false`: "maybe"
itu bug pemanggil, dan membatalkan favorit secara diam-diam adalah cara terburuk untuk melaporkannya.

Bukti persistensinya naik satu tingkat:
`a_history_entry_outlives_the_connection_that_wrote_it` menulis lewat satu koneksi, menutupnya,
membuka file yang sama, lalu membaca baris itu kembali dan mencarinya. Semua tes lain di berkas itu
berbagi satu koneksi, jadi tulisan yang tidak pernah ter-commit akan lulus semuanya.

**App-nya sudah dijalankan lagi, 29 Sep 2026, dengan batas yang jelas.** Kalimat sebelumnya di
dokumen ini menulis "UI-nya belum pernah dijalankan sama sekali", dan itu keliru: app ini sudah pernah
dipakai sebelum panel-panel baru ini ada, dan itu terbaca dari `connections.json` bertanggal 28 Sep
11:47 yang berisi koneksi sungguhan. Yang belum adalah menjalankannya **sejak panel-panel baru ini
ditambahkan**.

Yang dijalankan binari debug dari `.build/out/Products/Debug/QueryHive`, bukan bundel dari
`app/build.sh`. Ia hidup sembilan detik tanpa crash dan tanpa satu baris pun di stderr, lalu
dimatikan. Jadi "app-nya tidak meledak saat dibuka" sekarang fakta, bukan asumsi.

Yang itu **tidak** membuktikan apa-apa soal panelnya: tidak satu pun tab diklik, jadi History, Saved,
kolom pencarian, dan tab Data belum pernah dilihat siapa pun sejak ditambahkan. Itu tetap gap, dan
binari debug bukan bundel yang dikirim, jadi keduanya juga belum setara.

**Jalur penyimpanan produksinya diuji, dan itu bagian yang lebih berguna.** Engine dijalankan tanpa
`DB_PATH`, persis seperti app memanggilnya. Pada database yang belum ada di
`~/Library/Application Support/QueryHive/queryhive.sqlite3` ia membuat berkasnya, menerapkan ketiga
migrasi sampai `user_version = 3`, memasang tabel FTS beserta tiga trigger-nya, lalu menjawab
`{"event":"history","entries":[]}` dengan exit 0. Sebelum ini migrasi pada database produksi yang
benar-benar baru belum pernah dijalankan siapa pun: semua tes memakai direktori sementara atau
in-memory.

**Session restore, mendarat 29 Sep 2026, dan itu menutup §1.3 sekaligus Fase 1.** Tabel
`session_restore` yang sudah ada di `0001_init.sql:59` kini tersambung: perintah `session`
(`save` | `load` | `clear`) menulis satu baris tetap di bawah id `00000000-0000-7000-8000-000000000001`,
dengan seluruh tab sebagai satu blob JSON di `tab_json` dan tab depan di `active_tab_id`.

Tiga keputusan bentuk, karena ketiganya bisa saja sebaliknya. Yang disimpan **satu baris, bukan satu
per tab**: urutan tab ikut dipulihkan dan himpunan baris tidak punya urutan sendiri — tabelnya tidak
punya kolom sortir, dan menambahkannya berarti migrasi untuk urutan yang sudah dibawa JSON. Blob-nya
**opaque**: yang tersimpan adalah byte yang app tulis, jadi field baru pada tab tidak butuh migrasi —
alasan yang sama dengan `options` pada koneksi. Dan tab yang disimpan adalah **pekerjaannya, bukan
hasilnya**: SQL, koneksi, dan tujuan Run; grid hasil dan log sengaja tidak ikut, karena result set
milik `qh-result-store` dan memulihkan baris kemarin adalah janji tentang data yang mungkin sudah
berubah. Satu hal yang tidak ikut juga: tab daftar objek kembali sebagai tab query kosong, bukan
listing yang langsung berjalan lagi — menjalankan ulang browse saat launch adalah panggilan jaringan
yang tidak diminta siapa pun.

`session_restore` adalah perintah FFI kesembilan belas, jadi invariant #11 kembali terpakai:
`COMMANDS`, `Command`, `EngineCommand`/`EVERY_COMMAND`, dan `RustEngine.commands` disentuh bersamaan,
lalu `./app/build-ffi.sh` dijalankan. Tes yang menahannya tetap tes yang sama —
`RustEngineTests` membandingkan `commandWords` dengan `commandNames()` — dan tidak ada yang perlu
ditambah di sana karena daftarnya memang diturunkan dari yang sama.

Sisi app: `AppModel` menyimpan sesi saat himpunan tab berubah (buka, tutup, pindah tab) dan sekali
lagi saat aplikasi berhenti. Yang terakhir butuh jalur yang **memblokir**: `run` mengabarkan selesai
dengan melompat ke main queue, dan saat `applicationWillTerminate` main queue itulah yang sedang
menunggu, jadi completion yang dijadwalkan di sana tidak akan pernah berjalan. Karena itu
`DatabaseEngine` bertambah satu metode, `runBlocking`, dengan satu pemanggil dan alasannya ditulis di
protokolnya. Sesi ditulis ke database default engine yang sama dengan history dan saved queries;
`DB_PATH` hanya ditambahkan bila sesuatu mengalihkan `ConnectionStore` — suite tes — supaya tes tidak
menyentuh database pengguna.

Buktinya bertingkat. Tes app (`SessionTests`) menanam sesi lewat engine sungguhan ke database
sementara, membangun `AppModel(persistsSession: true)`, dan menunggu tab kembali — jalur penuh dari
inisialisasi sampai `SessionTab.tab()`. Lalu **bundel sungguhan**: `./app/build.sh` membangun
`app/dist/QueryHive.app`, sesi bertanda `SELECT 424242` ditanam ke database produksi lewat binari
engine, app dijalankan dengan `open`, lalu diminta berhenti lewat AppleScript. Baris sesi sesudahnya
memuat id, judul, dan SQL yang ditanam **plus** `outputDirectory` yang diisi `QueryTab.init` — bukti
bahwa app benar-benar membangun ulang tabnya lalu menuliskannya kembali, bukan sekadar menggemakan
blob lama. Database pengguna dikembalikan dari salinan sebelum pengukuran, jadi tidak ada sisa uji di
dalamnya.

**Fase 1 selesai.** §1.5 (favorit, kolom koneksi, tab Data) ditutup 29 Sep 2026, dan §1.3
(`session_restore`) ditutup pada tanggal yang sama. Yang tersisa dari daftar ini hanya yang memang
milik fase lain.

Yang belum untuk menutup Fase 0: workflow-nya belum pernah jalan di GitHub. Angka golden-nya
**diperiksa ulang 29 Sep 2026 dan ternyata tidak basi**: korpusnya memang 43 kasus, dan
`docs/golden-deltas.md:18` menulis 43 karena itu jawaban yang benar. Premis di paragraf sebelumnya —
bahwa berkas itu menulis angka lama — keliru, sama seperti premis FTS5 di §1.2. Yang benar-benar
belum cuma larian live: 14 kasus MySQL dan Trino belum dijalankan karena image-nya belum ada di
mesin ini, jadi angka `12/22` tetap milik larian parsial 29 Sep 2026. Empat selisih golden
PostgreSQL **sudah** terklasifikasi, sebagai D-8, D-9, D-1 dan D-2; lihat §3 di atas.

## 4. Fase 2: MCP server

Ini kandidat dengan nilai tertinggi, dan bisa dikerjakan tanpa logika baru yang berarti: semua
primitifnya sudah ada sebagai perintah di `crates/qh-ffi`. Pola yang diambil dari TablePro: server
MCP sebagai executable terpisah di dalam bundel, bicara stdio, tidak menyeret app ikut gagal.

**2.1 Binari MCP.** `crates/qh-ffi` sekarang menghasilkan library `libqh_ffi` (lib, cdylib,
staticlib), `uniffi-bindgen`, dan `queryhive-engine` (`crates/qh-ffi/Cargo.toml:32` dan `:39`).
Tambah `queryhive-mcp` sebagai `[[bin]]` ketiga dengan `src/bin/mcp.rs`, lalu ikutkan ke
`app/build.sh` supaya masuk `dist/QueryHive.app` dan DMG.

**2.2 Tool set MVP, read-only.** Sengaja sempit supaya bisa dilepas cepat dan aman:

| tool | memetakan ke |
|---|---|
| `db_drivers` | `db_drivers` |
| `connections_list` | `connections`, hanya nama, host, dan jenis; tanpa kredensial |
| `objects` `tables` `columns` | `objects` `tables`, dan kolom dari `preview` |
| `preview` `count` `explain` | `preview` `count` `explain` |
| `export_to_file` | `export` |

`to_table` **tidak** masuk MVP. Ia punya mode yang menjalankan `DROP TABLE IF EXISTS` sebelum
query-nya berjalan (`crates/qh-ffi/src/commands.rs:771`), dan itu tidak pantas dibuka ke klien yang
belum punya model otorisasi yang diuji.

**2.3 Protokol dan lifecycle.** Handshake file dan bridge logger mengikuti pola TablePro, karena
masalahnya sama: klien MCP perlu tahu server mana yang hidup untuk koneksi mana, dan helper yang mati
tidak boleh menjatuhkan app. Alamat handshake harus di `~/Library/Application Support/QueryHive/`,
satu direktori dengan `connections.json` yang sudah dipakai.

**2.4 Token dan scope.** Satu token per klien, disimpan sebagai hash dan bukan sebagai teks, dengan
scope tool dan allowlist koneksi. Token harus bisa dicabut, punya masa berlaku, dan setiap
pemakaiannya tercatat. Ini permukaan keamanan baru, jadi ADR-nya wajib dan kredensial tidak boleh
pernah muncul di log, error, atau keluaran tool. Aturan ini juga berlaku pada `connections_list`,
yang hanya mengembalikan nama.

**2.5 Tes.** Tes protokol di Rust untuk handshake, framing, dan penolakan tool di luar scope. Satu
tes end-to-end dengan klien MCP nyata lewat stdio, dan satu tes negatif yang membuktikan `to_table`
ditolak dengan pesan yang bisa dibaca.

**Kriteria selesai Fase 2.** Dari klien MCP, `connections_list` dan `preview` berjalan pada koneksi
Trino dev, `to_table` ditolak, dan membunuh proses MCP tidak berdampak pada app. Binari MCP ada di
dalam DMG hasil `./app/build.sh`.

**Hasil 29 Sep 2026.** Kelima item §4 mendarat, dan dua kriteria penerimaannya dijalankan penuh.

**Yang dibangun.** `crates/qh-ffi/src/bin/mcp.rs` adalah binari ketiga paket itu, dan
`crates/qh-ffi/src/mcp.rs` memuat protokol, registry tool, aturan scope, dan handshake. Protokolnya
JSON-RPC 2.0 per baris di stdin/stdout; **stdout tidak pernah memuat apa pun selain pesan protokol**,
dan seluruh log ke stderr. Sembilan tool read-only dipetakan ke perintah yang sudah ada lewat
`qh_ffi::run`, dengan `Settings` yang dibangun di Rust dari `ConnectionRecord`; pemetaan
`DB_KIND`/`DB_SCHEME`/`DB_SSLMODE`/`DB_INSECURE` menyalin `AppModel.connectionEnvironment`, dan itu
diuji sebagai fungsi murni. `to_table` tidak ada di registry dan ditolak dengan menyebut mode
`replace`-nya sebelum registry dilihat. Token disimpan sebagai SHA-256 heksadesimal di tabel baru
`mcp_token` (migrasi `0005`), dengan scope tool + allowlist koneksi; allowlist kosong berarti tidak ada
koneksi. Keputusan bentuknya ada di `docs/decisions/0015-mcp-token-scope.md`.

**Gate, 29 Sep 2026.** `cargo fmt --all --check` bersih; `cargo clippy --workspace --all-targets --
-D warnings` bersih; `cargo test --workspace` **636 lulus / 0 gagal**; `cargo deny check licenses`
melaporkan `licenses ok`; `swift build` selesai dan `swift test` **169 tes / 0 gagal**;
`./app/build.sh` mencetak `Built dist/QueryHive.app` dan `Contents/MacOS/` berisi dua Mach-O,
`QueryHive` dan `queryhive-mcp` (yang kedua ikut ditandatangani loop yang sudah ada).
`./app/build-ffi.sh` dijalankan dan **`app/Generated/` tidak berubah**: permukaan FFI memang tidak
disentuh, jadi invariant #11 tidak terpakai.

**Penerimaan hidup, dan premisnya ternyata basi lagi.** Catatan sesi sebelumnya menulis bahwa hanya
image PostgreSQL yang ada. Diperiksa 29 Sep 2026: `podman images` memuat `trinodb/trino:latest` dan
`mysql:8.4`, dan ketiga kontainer dev (`qh-postgres`, `qh-mysql`, `qh-trino`) sudah hidup. Jadi
penerimaannya dijalankan pada **Trino 483 di `127.0.0.1:58080`**, target yang memang diminta rencana.

Alurnya lewat bundel, bukan `cargo run`: `connections.json` berisi satu koneksi Trino
(`tpch.tiny`, skema `http`, tanpa password) diimpor lewat `queryhive-engine import_connections` ke
database sementara, token diterbitkan lewat `queryhive-mcp issue`, lalu server dijalankan dari
`app/dist/QueryHive.app/Contents/MacOS/queryhive-mcp` dengan stdin berupa tujuh baris permintaan.

| panggilan | hasil |
|---|---|
| `initialize` | `protocolVersion 2025-06-18`, `capabilities.tools {}`, `serverInfo.queryhive-mcp 0.1.0` |
| `tools/list` | sembilan tool, `to_table` tidak ada |
| `connections_list` | satu baris allowlist: `id`, `name`, `kind`, `host`, `port`, `database`; tanpa user/password/options |
| `preview` `SELECT nationkey, name FROM nation LIMIT 3` | kolom `nationkey`/`name`, tiga baris `ALGERIA`/`ARGENTINA`/`BRAZIL`, `done rows:3` |
| `count` `SELECT * FROM nation` | `rows:25` |
| `columns nation` | empat kolom (`nationkey`, `name`, `regionkey`, `comment`), `done rows:0` — `LIMIT 0` di statement, bukan di setting, karena setting itu di-floor ke satu |
| `export_to_file` CSV `nation` ke direktori sementara | `done rows:25`, satu berkas 308 byte dengan header + 25 baris |
| `to_table` | `isError:true` dengan alasan mode penulisan |

Handshake terhapus saat proses keluar bersih, dan stderr hanya berisi satu baris
`queryhive-mcp 0.1.0 serving token 'live'` — tokennya tidak muncul di sana, di `list`, maupun di
`stdout`.

**Membunuh MCP tidak berdampak pada app, diuji dengan app sungguhan.** Aplikasi dijalankan dari bundel
dengan `open -n`, server MCP dijalankan dari bundel yang sama, lalu MCP di-`kill -9`. App masih hidup
sesudahnya. Database pengguna disalin sebelum pengukuran dan dikembalikan sesudahnya, jadi tidak ada
sisa uji di dalamnya. Ini sejalan dengan bentuknya: keduanya proses terpisah yang hanya berbagi berkas
SQLite dan Keychain, dan app tidak pernah men-spawn MCP.

**Yang belum.** Klien MCP-nya **digulung sendiri** — tujuh baris `write!`/`read_line` di
`tests/mcp_stdio.rs`, karena tidak ada crate klien MCP di ruang kerja ini — jadi yang terbukti adalah
transpor dan framing-nya, bukan bahwa satu klien MCP tertentu menerima deskripsi tool-nya. Jalur
password Keychain belum teruji hidup karena koneksi Trino dev tidak memakai password (koneksi tanpa
password membaca `None` dari Keychain, yang memang bukan jalur yang sama dengan password yang
tersimpan). Dan `./app/build-dmg.sh` belum dijalankan; yang diverifikasi adalah `./app/build.sh`
menaruh binernya di `QueryHive.app`, sesuai kriteria yang menamai `build.sh`. Tanpa klien pihak
ketiga, kriteria "dari klien MCP" karena itu dipenuhi sebagian.

## 5. Fase 3: keselamatan eksekusi

QueryHive tidak punya query timeout sama sekali. Satu-satunya timeout di seluruh `crates/` adalah
`connect_timeout` di `crates/qh-driver-trino/src/lib.rs:262`, dan tidak ada batas waktu statement di
`ExecuteOptions` (`crates/qh-driver/src/lib.rs:167`). `preview` pada tabel besar karena itu tidak
punya langit kedua selain tombol Stop, dan Stop sendiri bergantung pada driver yang menghormati
`cancel`.

**3.1 Timeout statement.** Tambah ke `ExecuteOptions`; implementasikan per driver memakai kemampuan
servernya masing-masing, dan jangan menyamarkan ketiadaan dukungan sebagai timeout lokal. Kalau
sebuah driver tidak bisa, `Capabilities` yang menyatakannya, seperti `cancel` sekarang.

**3.2 Safe Mode per koneksi.** Tingkatannya: penuh, tolak DDL, read-only. Yang menahan adalah engine,
bukan UI, karena engine juga dipakai CLI dan akan dipakai MCP. `to_table` mode Replace pada koneksi
read-only harus gagal di engine.

**3.3 Batas untuk `count`.** `count` pada tabel besar sama mahalnya dengan query penuh. Beri batas
waktu yang sama, dan laporkan hasilnya sebagai perkiraan bila batas itu tercapai.

**Kriteria selesai Fase 3.** Tes yang membuktikan timeout memicu error bertipe dengan pesan yang
menyebut batas waktunya, bukan hang. Tes yang membuktikan safe mode menolak `DROP TABLE` di engine
dan bukan hanya di UI. Setting timeout bisa diubah dan bertahan setelah restart.

**Hasil 29 Sep 2026.** Ketiga item mendarat dan kriteria selesainya dijalankan.

**3.1 Timeout statement.** `ExecuteOptions` bertambah `statement_timeout: Option<Duration>` dan
`Capabilities` bertambah `statement_timeout: bool` (`crates/qh-driver/src/lib.rs`). Ketiga driver
memakai mekanisme servernya sendiri, bukan timer lokal: PostgreSQL `statement_timeout` (SQLSTATE
`57014`, diperiksa bersama pesannya karena kode itu juga dipakai cancel biasa), Trino
`query_max_run_time` lewat `X-Trino-Session` pada POST statement (`EXCEEDED_TIME_LIMIT` dan
`QUERY_EXCEEDED_MAX_EXECUTION_TIME`), MySQL `max_execution_time` (error `3024`, dan **tidak** mencakup
write — dinyatakan di doc modulnya, bukan disembunyikan). Batas yang terlampaui datang sebagai
`EngineError::Timeout` yang menyebut batasnya (`crates/qh-core/src/error.rs`). Setelan
`STATEMENT_TIMEOUT_MS`: `0` berarti tanpa batas, dan nilai negatif ditolak dengan menyebut namanya.
Keputusannya di `docs/decisions/0016-statement-timeout.md`.

**3.2 Safe Mode.** Tiga tingkat `full`/`no_ddl`/`read_only`, diklasifikasi di
`crates/qh-sql/src/classify.rs` dan ditegakkan di engine untuk `preview`, `explain`, `export`, `count`
dan `to_table`. `to_table` diklasifikasi pada statement yang benar-benar dikirim — `DROP`/`CREATE
TABLE AS`/`INSERT` hasil bangunan perintah — jadi mode `replace` gagal pada koneksi read-only
**sebelum** connect. Aturannya konservatif: yang tidak bisa dikenali diperlakukan sebagai write dan
ditolak di semua mode selain `full`. Tingkatnya disimpan per koneksi (`Connection.safeMode`, dibaca
`decodeIfPresent` sehingga berkas lama tetap `full`) dan dipilih di editor koneksi; server MCP selalu
menambahkan `SAFE_MODE=read_only`, yang menutup celah yang ADR-0015 nyatakan terbuka.
Keputusannya di `docs/decisions/0017-safe-mode.md`.

**3.3 Batas untuk `count`.** `count` memakai batas yang sama dan, pada timeout, melaporkan galat
bertipe alih-alih angka. Tidak ada driver di ruang kerja ini yang bisa memberi perkiraan murah untuk
sebuah *statement* (`reltuples`, `TABLE_ROWS` dan `$partitions` menjawab untuk *tabel*), jadi
perkiraan tidak tersedia dan itu dikatakan alih-alih dikarang.

**Verifikasi, dan bagaimana.** `crates/qh-ffi/tests/safe_mode.rs` membuktikan penolakan terjadi
**sebelum** engine connect, jadi ia tidak butuh server: sebuah `CountingEngine` yang gagal saat
connect dipakai, dan `connects() == 0` adalah assertionnya. Cakupannya: `read_only` menolak `DROP`
dan `to_table replace`; `no_ddl` menolak DDL tetapi membiarkan write sampai connect; `read_only`
membiarkan read sampai connect; `full` tidak menolak apa pun; `SAFE_MODE` yang tidak dikenal ditolak
dengan menyebut namanya; `count` digerbangi pada statement pemanggil dan bukan pada pembungkusnya;
script multi-statement menyebut nomor statement yang ditolak.

Timeout diuji **hidup** terhadap PostgreSQL 17 lokal — cluster sementara di `127.0.0.1:55432`,
karena mesin podman hilang sebelum verifikasi. Hasilnya: `SELECT pg_sleep(5)` di bawah
`STATEMENT_TIMEOUT_MS=500` kembali dalam **1,25 detik** (bukan 5) dengan pesan

    the statement exceeded the 500 ms statement timeout and was cancelled by the server:
    canceling statement due to statement timeout

dan tanpa batas, `SELECT pg_sleep(1)` selesai dengan `elapsed_ms: 1007`. Empat tes
`crates/qh-ffi/tests/real_server.rs` dijalankan dengan `QH_TEST_POSTGRES=1` terhadap cluster yang sama
dan lulus, termasuk `a_count_that_times_out_names_the_bound_and_invents_no_number`.

**Setelan bertahan setelah restart.** `statementTimeoutMS` hidup di `UserDefaults` dengan default
60.000 ms, dan `object(forKey:)` dipakai alih-alih `integer(forKey:)` karena `0` di sini adalah nilai
nyata — "tanpa batas" — bukan ketiadaan.

**Yang belum.** MySQL dan Trino tidak diuji hidup pada Fase 3: mesin podman sudah tidak ada saat
verifikasi (hilang bersama peristiwa disk penuh), jadi `max_execution_time` dan `query_max_run_time`
hanya terbukti lewat uji unit pemetaan error di masing-masing driver, bukan lewat larian yang bisa
dilihat. Keduanya tercatat sebagai belum di sini, bukan sebagai lulus.

## 6. Fase 4: editor harian

Empat celah kecil yang terasa tiap hari. Tidak ada yang bergantung satu sama lain, jadi bisa
dikerjakan terpisah.

**4.1 Find and replace** di `Views/SQLEditor.swift`. Pencarian untuk `findBar`, `showFind`,
`replaceAll`, dan `performFindPanelAction` di `app/Sources` tidak menemukan apa pun hari ini.

**4.2 Code folding.** Tidak ditemukan penanda lipat kode di source app. Yang dilipat minimal statement
dan CTE; `qh-sql` sudah memindai batas statement (`crates/qh-sql/src/scan.rs:43`), jadi batasnya tidak
perlu dihitung ulang di Swift dengan cara berbeda.

**4.3 Sorting di grid** (`Views/ResultGrid.swift`). Perlu diputuskan lebih dulu: sort di memori atas
baris yang sudah diambil, atau sort di server lewat query ulang. Pilihan pertama murah dan sejalan
dengan makna row cap saat ini; pilihan kedua benar tapi berarti mengubah statement. Ambil yang
pertama, dan tulis di header grid bahwa yang tersortir adalah baris yang sudah diambil.

**4.4 JSON cell viewer.** Kolom `ARRAY`, `MAP`, `ROW`, dan `JSON` sudah diserialkan sebagai teks JSON
di ekspor. Di grid, nilai seperti itu perlu bisa dibuka dan dibaca, bukan dipotong.

**Kriteria selesai Fase 4.** `swift test` hijau untuk tiap item, plus satu render `--snapshot` per
item supaya perubahannya bisa dilihat, sesuai cara tema ditinjau sekarang.

## 7. Fase 5: perpindahan data

**5.1 Impor CSV dan XLSX ke tabel.** Perintah `import_data`, streaming seperti ekspor: baca sebagian,
kirim sebagian, jangan pernah menahan file di memori. Ini prinsip yang sudah tertulis di `README.md`
dan tidak boleh dilanggar hanya karena arahnya terbalik. Column mapping dan transaction safety
diambil sebagai syarat, karena impor yang setengah jalan lebih buruk daripada impor yang gagal.

**5.2 Ekspor Parquet.** Pohon ini punya sembilan format dan tidak satu pun kolumnar, padahal
TablePro menyediakan Parquet sebagai plugin dan pemakainya jelas: Trino, Iceberg, Spark. Ini format
kesepuluh di `crates/qh-export`, dan kandidatnya paling jelas untuk siapa pun yang bekerja dengan
gudang data.

**5.3 Insert dan delete baris.** `Models/CellEdits.swift` menyimpan nilai sel dan
`Models/UpdateStatements.swift` menghasilkan `UPDATE`. Insert dan delete baris menyusul dengan pola
yang sama: antre, tinjau SQL-nya, baru jalankan. Meninjau sebelum eksekusi adalah bagian dari
polanya, bukan tambahan.

**Kriteria selesai Fase 5.** Impor bolak-balik: ekspor tabel ke CSV, impor ke tabel baru, bandingkan
`count` keduanya. Parquet dibandingkan terhadap pembaca independen. Insert dan delete punya tes yang
membuktikan statement yang ditinjau sama dengan statement yang dijalankan.

## 8. Fase 6: kandidat besar, belum dijadwalkan

Tidak dikerjakan sebelum ada keputusan produk: structure editor, routines dan user-defined types,
backup dan restore untuk PostgreSQL, Open Quickly, external API di luar MCP, dan plugin ABI runtime.
Masing-masing sudah punya bagiannya di matriks celah analisis, dengan alasan kenapa ditunda.

## 9. Yang tidak dikerjakan

Dari analisis §7, ditulis ulang sebagai keputusan supaya tidak dibuka lagi tiap sesi: 36 engine,
plugin registry dan ABI runtime, iCloud sync, app iOS, licensing dan team plan, chart dan map di grid,
ER diagram, compare and sync, dan server dashboard.

## 10. Risiko

**FTS5 mengubah binary SQLite.** Fitur ini menarik kode SQLite tambahan ke dalam `rusqlite` yang
dibangun `bundled`. Ukuran bundel dan hasil `cargo deny check licenses` harus diukur ulang, dan
bundle-nya harus dibangun dan dijalankan sesudahnya, bukan hanya di-`cargo test`.

**MCP adalah permukaan keamanan baru.** Token, scope, dan allowlist harus diuji termasuk kasus
negatifnya. Batas yang tidak bisa dilanggar: tidak ada kredensial di log, error, atau keluaran tool;
`to_table` tidak dibuka; dan koneksi read-only tetap read-only walau lewat MCP.

**Job macOS di CI berbayar menitnya berbeda dari Linux.** Ukur dulu portabilitas workspace (§0.2)
sebelum menerima biayanya.

**`app/release.sh` menyentuh tag dan GitHub Releases.** Setiap perubahan CI harus dibuktikan tidak
menyentuh jalur rilis. `--dry-run` ada untuk itu, dan jalur ini tidak boleh diubah tanpa menjalankan
`./app/release.sh --dry-run` sesudahnya.

**Impor adalah jalur masuk data yang tidak tepercaya.** File besar, encoding campur, dan baris rusak
di tengah adalah keadaan normal, bukan kasus tepi.

## 11. Ukuran keberhasilan

Fase 0: dua workflow hijau, dan angka golden yang tercatat cocok dengan hasil jalannya terakhir.
Fase 1: satu query yang dijalankan hari ini bisa ditemukan lagi setelah app di-restart.
Fase 2: satu klien MCP bisa membaca objek dan preview pada koneksi Trino, dan tidak bisa menulis.
Fase 3: satu `preview` pada tabel besar berhenti sendiri pada batas waktunya dengan pesan yang
menyebut batas itu.
Fase 4: tidak ada lagi alasan membuka editor lain untuk mencari kata di file `.sql`.
Fase 5: satu tabel pindah ke Parquet dan kembali, dengan `count` yang cocok.
