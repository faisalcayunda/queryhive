# Invariants

Jebakan yang sudah menggigit sekali, dan apa yang menahannya sekarang. Ini bukan daftar keputusan:
keputusan ada di `docs/decisions/`, dan alasan di balik arsitekturnya ada di
`docs/architecture/rust-engine-blueprint.md`.

Aturan masuknya satu: sebuah entri hanya ditulis setelah ada kejadian nyata, dan entri itu menyebut
gejalanya, tempatnya, dan apa yang menahannya. Spekulasi tidak masuk.

---

## 1. `app/Generated/` di-commit, jadi ia bisa basi

**Gejala.** Build di mesin yang pertama meregenerasi berhasil, mesin lain gagal compile. Penyebabnya
binding UniFFI lama yang masih dipakai app, dan kegagalannya muncul jauh dari penyebabnya.

**Kenapa di-commit.** Permukaan FFI adalah kontrak yang app tulis terhadapnya, jadi perubahan di
sana seharusnya muncul di review sebagai diff di pohon app, bukan sebagai error di mesin yang
kebetulan meregenerasi lebih dulu. `app/build-ffi.sh` menuliskan alasan ini di kepalanya.

**Yang menahan.** `.github/workflows/verify.yml` menjalankan `./app/build-ffi.sh` lalu
`git diff --exit-code -- app/Generated`. Diff sekecil apa pun berarti binding basi.

**Saat mengubah permukaan FFI.** Jalankan `./app/build-ffi.sh` dan commit hasilnya bersama
perubahan Rust-nya, jangan sesudahnya.

## 2. Seam injeksi engine belum ada

**Gejala.** `MockEngine` ada dan diuji, tapi tidak bisa dipakai dari kode app. Tes yang butuh engine
harus memanggil `MockEngine` langsung, bukan menggantinya di `Engine.current`.

**Kenapa.** `Engine.current` adalah nilai tetap, dan `Support/DatabaseEngine.swift:14-17` mencatat
gagasan itu sebagai gap yang terukur: mock-nya ada, jalan masuknya belum. Ini bukan mock yang
kurang.

**Saat menambah tes app yang butuh engine.** Periksa dulu apakah seam-nya sudah dipasang; kalau
belum, tes itu tidak bisa memakai `Engine.current` dan harus membangun jalurnya sendiri.

## 3. Dua enum tertutup di dua sisi batas

**Gejala.** Menambah driver keempat ternyata menyentuh UI, padahal komentar di
`crates/qh-driver/src/lib.rs` menyatakan cukup menambah crate dan meregister.

**Kenapa.** `DriverKind` adalah enum tertutup di Rust (`crates/qh-driver/src/lib.rs:46`), dan Swift
punya `ConnectionKind` paralel yang di-parse dari string
(`app/Sources/TrinoExporter/Models/Connections.swift:356`). Yang benar untuk mesin tidak otomatis
benar untuk UI.

**Saat menambah driver.** Anggarkan satu varian Rust, satu case Swift, satu logo di
`Support/DriverLogos.swift`, dan satu tile di pemilih koneksi.

## 4. Satu angka macOS di tiga tempat, dan `.cargo/config.toml` yang memutuskan

**Gejala.** Bundle mengaku macOS 14.0 di `Info.plist` sementara objek C dan assembly di dalamnya
dibangun untuk SDK mesin yang membangunnya. Linker hanya melihat `minos` objeknya, jadi app tidak
bisa memberi tahu mana simbol yang sebenarnya tidak ada di sistem yang ia klaim dukung.

**Yang menahan.** `MACOSX_DEPLOYMENT_TARGET = "14.0"` di `.cargo/config.toml` dengan
`force = true`, dan angka itu harus sama dengan `platforms` di `app/Package.swift` serta heredoc
`LSMinimumSystemVersion` di `app/build.sh`. Tiga berkas, satu angka.

**Saat menaikkan minimum macOS.** Ubah di tiga tempat itu, bukan satu.

## 5. `cargo build` saja tidak cukup sebelum membangun app

**Gejala.** Link gagal karena `libqh_ffi.a` tidak ada, atau yang ter-link adalah dylib lama
berikut install name absolutnya.

**Kenapa.** `app/Package.swift` mencari arsipnya di `target/ffi/static/<profile>/`, dan hanya
`app/build-ffi.sh` yang menaruh arsip di sana. Direktori itu sengaja terpisah dari
`target/<profile>/`, supaya `ld` tidak menemukan dylib di sebelah arsipnya.

**Saat membangun app dari sumber.** Jalankan `./app/build-ffi.sh`, bukan `cargo build`.

## 6. FTS5 sudah ada di dalam `bundled`; `rusqlite` tidak punya sakelarnya

**Keadaan.** `rusqlite = { version = "0.37", features = ["bundled"] }` (`Cargo.toml:62`), dan itu
sudah cukup. FTS5 dikompilasi oleh `libsqlite3-sys`, bukan dinyalakan lewat fitur `rusqlite`:
`libsqlite3-sys-0.35.0/build.rs:132` menulis `-DSQLITE_ENABLE_FTS5` untuk build `bundled`. Dan
rusqlite 0.37 memang tidak punya fitur bernama `fts5` sama sekali; dari 34 fiturnya, tidak satu pun
mengandung `fts`. Jadi menambahkan `features = ["fts5"]` bukan sekadar tidak berguna, melainkan
menggagalkan resolusi dependensi sebelum satu baris pun dikompilasi.

**Kenapa ini pernah tertulis salah.** Catatan sebelumnya di halaman ini menyatakan FTS5 "belum
dinyalakan" dan menyuruh menjalankan ulang `cargo deny`. Itu keliru. Yang membuatnya layak dicatat
bukan kesalahannya, melainkan tempat pemeriksaannya: jawabannya ada di crate lain, `libsqlite3-sys`,
bukan di `rusqlite`, dan `Cargo.toml` proyek ini tidak menunjuk ke sana sama sekali.

**Yang tetap berlaku.** `bundled` memutuskan versi SQLite yang dipakai. Mengganti versi itu tetap
menuntut `cargo deny check licenses` dan bukan hanya `cargo test`, karena versi baru membawa kode
baru beserta lisensinya.

## 7. Tidak ada query timeout — **DITUTUP 29 Sep 2026**

**Keadaan saat itu.** Satu-satunya timeout di seluruh `crates/` adalah `connect_timeout` di
`crates/qh-driver-trino/src/lib.rs:262`. `ExecuteOptions` di `crates/qh-driver/src/lib.rs:167` tidak
punya batas waktu statement, dan tidak ada driver yang memasangnya.

**Konsekuensinya saat itu.** `preview` pada tabel besar tidak punya batas selain tombol Stop, dan
Stop sendiri bergantung pada driver yang menghormati `cancel` (lihat `Capabilities.cancel`).

**Yang menahannya sekarang.** `ExecuteOptions::statement_timeout` dan `Capabilities::statement_timeout`
di `crates/qh-driver/src/lib.rs`, dengan tiap driver memakai mekanisme servernya sendiri —
PostgreSQL `statement_timeout`, Trino `query_max_run_time`, MySQL `max_execution_time` — dan batas
yang terlampaui datang sebagai `EngineError::Timeout` yang menyebut batasnya. Setelannya
`STATEMENT_TIMEOUT_MS`. Alasannya di `docs/decisions/0016-statement-timeout.md`.

**Yang tetap berlaku.** MySQL tidak menerapkan `max_execution_time` pada write; itu sifat servernya,
dan ia dinyatakan di doc modul driver alih-alih disembunyikan. Sebuah driver yang tidak bisa
menegakkan batas menjawab `Capabilities::statement_timeout == false`, bukan diam-diam mengabaikannya.

## 8. PostgreSQL menolak SQL multi-statement

**Gejala.** Skrip berisi dua statement gagal dengan keluhan server, bukan dengan pesan dari app.

**Kenapa.** `prepare` hanya mendeskripsikan satu statement, jadi driver menolaknya secara sengaja
(`crates/qh-driver-postgres/src/lib.rs:19`). Memecah skrip menjadi statement adalah pekerjaan
`qh-sql`, yang sudah memindai literal dan komentar dengan benar (`crates/qh-sql/src/scan.rs:43`).

**Saat menambah driver.** Jangan memecah statement di dalam driver. Kalau servernya menerima
beberapa statement sekaligus, itu keputusan driver; kalau tidak, pemecahannya milik `qh-sql`.

## 9. Trino tidak punya koneksi persisten

**Gejala.** Kode yang memasang idle timeout, keepalive, atau pool berperilaku aneh pada koneksi
Trino, karena tidak ada socket yang bisa dipegang.

**Yang menahannya.** `Capabilities.persistent_connection` di `crates/qh-driver/src/lib.rs:158`.
Setiap query Trino adalah pertukaran HTTP, jadi apa pun yang mengasumsikan socket harus bertanya
dulu.

## 10. Rilis Sparkle: feed harus jadi aset rilis `latest`, dan build number yang dibandingkan

**Gejala.** DMG terbit, tapi tidak ada satu pun pengguna yang diberi tahu ada versi baru, tanpa
apa pun di layar yang mengatakan demikian.

**Kenapa.** `Info.plist` membakar
`https://github.com/faisalcayunda/queryhive/releases/latest/download/appcast.xml`, jadi feed harus
menjadi aset rilis yang GitHub sebut *latest*, bukan hanya DMG-nya. Sparkle juga membandingkan build
number, bukan versi, jadi rilis yang build number-nya tidak lebih baru tidak akan pernah sampai.

**Yang menahan.** `app/release.sh` menolak jalan pada pohon kotor, di luar `main`, saat tag sudah
ada, atau saat build numbernya tidak lebih baru dari feed yang hidup, lalu memeriksa ulang URL feed
itu setelah rilis naik.

**Saat menyentuh CI atau jalur rilis.** Buktikan dengan `./app/release.sh --dry-run`.

## 11. Satu perintah FFI hidup di empat daftar, dan satu pengawasnya sempat palsu

**Gejala.** Empat perintah baru ditambahkan, usage line bertambah, semua tes Rust lulus, dan app tetap
tidak bisa memanggil satu pun dari mereka. Yang menangkapnya hanya `swift test`.

**Empat tempat yang harus sepakat.** `COMMANDS` di `crates/qh-ffi/src/lib.rs` adalah kata-kata yang
dipakai CLI; `Command` di berkas yang sama adalah dispatch-nya; `EngineCommand` di
`crates/qh-ffi/src/uniffi_api.rs` adalah tipe yang app pegang lewat UniFFI; dan
`RustEngine.commands` di `app/Sources/TrinoExporter/Support/RustEngine.swift` adalah pemetaan kata ke
case, yang harus ada karena UniFFI menghasilkan enum dan daftar nama tetapi tidak menghasilkan cara
kembali dari nama ke case.

**Pengawas yang asli.** `app/Tests/TrinoExporterTests/RustEngineTests.swift` membandingkan
`RustEngine.commandWords` dengan `commandNames()`. Itulah tes yang gagal dan memberi tahu.

**Pengawas yang dulu palsu.** Tes di `uniffi_api.rs` membandingkan `command_names()` dengan
`crate::COMMANDS`, padahal `command_names()` dibangun dari `crate::COMMANDS` itu sendiri. Assertion
yang tidak bisa gagal. Sekarang `command_names()` membaca `EVERY_COMMAND`, daftar tetap yang ditulis
tangan di samping enum (19 sejak `session` mendarat 29 Sep 2026), jadi dua daftar yang benar-benar
terpisah dibandingkan dan selisih panjangnya menggagalkan tes.

**Yang tidak bisa menangkapnya.** Pemeriksaan `git diff -- app/Generated` di
`.github/workflows/verify.yml` juga tidak bisa: `app/Generated` hanya berubah kalau enum UniFFI-nya
berubah, dan justru itu yang terlewat.

**Saat menambah perintah.** Sentuh keempatnya, jalankan `./app/build-ffi.sh`, commit `app/Generated/`
bersama perubahan Rust-nya, lalu jalankan `swift test` sebelum menganggapnya selesai.
