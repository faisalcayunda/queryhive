# 0011 — Pengecualian MPL-2.0 untuk UniFFI

- **Status:** Diterima
- **Tanggal:** sesi migrasi (setelah ADR-0004)
- **Konteks instruksi:** §5 poin 8, §8

## Konteks

ADR-0002 menetapkan dua hal: proyek tetap MIT, dan daftar izin `cargo-deny` berisi `MIT`,
`Apache-2.0`, `BSD-3-Clause`, `ISC`, `Unicode-3.0`, `Zlib` — dengan aturan eksplisit bahwa
**setiap pengecualian baru harus melalui ADR**. ADR-0004 memilih UniFFI untuk control plane, dan
pilihan itu membawa lisensi yang tidak ada di daftar tersebut.

Graf yang Cargo benar-benar resolve hari ini:

```
$ cargo metadata --format-version 1 | jq '.packages | length'          # 456
# 14 anggota workspace, 442 paket non-workspace
$ cargo metadata --format-version 1 | jq -r '.packages[]
    | select(.license == "MPL-2.0") | "\(.name) \(.version)"' | sort
uniffi 0.32.1
uniffi_bindgen 0.32.1
uniffi_core 0.32.1
uniffi_internal_macros 0.32.1
uniffi_macros 0.32.1
uniffi_meta 0.32.1
uniffi_pipeline 0.32.1
uniffi_udl 0.32.1
```

Delapan baris, semuanya 0.32.1, semuanya dari crates.io (`registry+https://github.com/rust-lang/crates.io-index`,
dengan checksum, di `Cargo.lock:3886` dan seterusnya). Dari manifest masing-masing di
`~/.cargo/registry/src/index.crates.io-*/`, kedelapannya menulis `license = "MPL-2.0"`.
Satu detail yang perlu dicatat apa adanya: **tidak satu pun dari kedelapan crate itu mengirimkan
berkas `LICENSE`/`COPYING`/`NOTICE`** — baik di direktori registry maupun di dalam arsip
`.crate`-nya. Satu-satunya pernyataan lisensi adalah field SPDX di `Cargo.toml`. Teks lisensinya
berada di repositori hulu uniffi, bukan di dalam apa yang kita unduh.

Satu temuan lain dari pengukuran yang sama, yang **tidak** dicakup ADR ini: `CDLA-Permissive-2.0`
pada `webpki-roots` 0.26.11, `webpki-roots` 1.0.9, dan `webpki-root-certs` 1.0.9. Ketiganya juga
tidak ada di daftar izin ADR-0002, dan ketiganya masuk lewat jalur yang benar-benar dikirim
(`reqwest`/`hyper-rustls`, `mysql_async`, `rustls-platform-verifier`). Lihat Konsekuensi.

## Apa yang MPL-2.0 sebenarnya mewajibkan

Dicek terhadap teks lisensinya sendiri, bukan dari ingatan (https://www.mozilla.org/en-US/MPL/2.0/):

- **§1.10 "Modifications"** mendefinisikan perubahan pada tingkat **berkas**: "any file in Source
  Code Form that results from an addition to, deletion from, or modification of the contents of
  Covered Software; or any new file in Source Code Form that contains any Covered Software."
  Inilah "file-level copyleft" itu: yang terikat adalah berkas milik Covered Software, bukan
  program yang memakainya.
- **§3.1** mewajibkan distribusi Covered Software dalam bentuk Source Code Form tetap di bawah
  lisensi ini, dan penerima diberi tahu.
- **§3.2** berlaku bila Covered Software didistribusikan dalam **Executable Form**: Source Code
  Form-nya harus tetap tersedia (dan penerima diberi tahu bagaimana memperolehnya), sementara
  Executable Form-nya sendiri boleh didistribusikan "under the terms of this License, or
  sublicense it under different terms" — selama lisensi Executable Form itu tidak mengurangi hak
  penerima atas Source Code Form.
- **§3.3** adalah pasal yang menentukan bagi kita: "You may create and distribute a Larger Work
  under terms of Your choice, provided that You also comply with the requirements of this License
  for the Covered Software." Larger Work (§1.7) adalah karya yang menggabungkan Covered Software
  dengan materi lain *dalam berkas terpisah* yang bukan Covered Software.

Terapan untuk repo ini:

1. **Kode kita tetap MIT.** Seluruh kode di workspace ini adalah berkas terpisah dari berkas
   uniffi, jadi ia Larger Work dalam pengertian §1.7, dan §3.3 memberi kebebasan memilih lisensi
   untuknya. Tidak ada satu pun kewajiban MPL-2.0 yang menempel pada `crates/qh-*`, dan `LICENSE`
   tidak berubah.
2. **Kita memang mendistribusikan uniffi dalam Executable Form, jadi §3.2 berlaku.** Lib yang
   dikirim aplikasi adalah `libqh_ffi.dylib` (`app/build-ffi.sh:27`), dibangun dari
   `crate-type = ["lib", "cdylib"]` (`crates/qh-ffi/Cargo.toml:15`), dan runtime uniffi benar-benar
   ada di dalamnya: `nm -a target/debug/libqh_ffi.dylib | grep -c uniffi_core` → **1731** simbol.
   (`target/release/libqh_ffi.dylib` menunjukkan 0 karena profil release memakai
   `strip = "symbols"` — `Cargo.toml:71`; kodenya ada, namanya dibuang.) Binding yang di-commit pun
   memanggil scaffolding uniffi secara langsung, mis. `ffi_qh_ffi_rustbuffer_from_bytes` dan
   `ffi_qh_ffi_rustbuffer_free` (`app/Generated/QueryHiveFFI/qh_ffi.swift:28`).
3. **Yang §3.2 minta dari kita karena itu bukan apa-apa:** Source Code Form yang harus tersedia
   adalah Source Code Form uniffi — versi yang persis, tanpa perubahan dari kita. `Cargo.lock`
   menyebut versi dan checksum-nya, crates.io menyediakan sumbernya, dan repositori hulu uniffi
   menyediakan teks lisensinya. Tidak ada "Modifications" dalam pengertian §1.10, jadi tidak ada
   yang perlu kita publikasikan. Yang tersisa hanyalah kewajiban memberi tahu, yang dicatat di
   Konsekuensi.
4. **§3.4** melarang menghapus atau mengubah substansi pemberitahuan lisensi di dalam berkas
   Covered Software. Karena crate yang kita unduh tidak membawa berkas lisensi sama sekali,
   tidak ada yang bisa terhapus; yang penting adalah tidak ada perubahan yang kita buat pada
   sumbernya.

## Apakah ada berkas MPL-2.0 yang dimodifikasi atau disalin ke pohon ini

**Tidak ada.** Ini yang membuat jawabannya sederhana; kalau ada satu saja berkas uniffi yang
disalin atau ditempel, §1.10 akan menariknya menjadi Covered Software dan kewajibannya berbeda.
Cara saya memastikannya:

| Kemungkinan | Pemeriksaan | Hasil |
|---|---|---|
| Vendoring | `ls -d vendor` | tidak ada direktori `vendor/` |
| Patch lokal | `grep -rn --include=Cargo.toml -E '^\[patch|git\s*=' .` | tidak ada `[patch]`, tidak ada dependency `git`; kedelapan baris bersumber `registry+https://…crates.io-index` dengan checksum (`Cargo.lock:3886`) |
| Fork | seluruh Cargo.toml workspace | hanya `uniffi = { version = "0.32", features = ["cli"] }` (`crates/qh-ffi/Cargo.toml:57`), tanpa `path`/`git` |
| Template UDL | `find . -name '*.udl'` | tidak ada berkas `.udl` di pohon |
| **Template codegen disalin** | 145 berkas non-`.rs` di `uniffi_bindgen-0.32.1` (38 Swift, 43 Kotlin, 42 Python, 21 Ruby) — SHA-256 masing-masing dibandingkan dengan 6.042 berkas di pohon ini | **0 kecocokan**; satu-satunya nama yang sama adalah `README.md` |
| Generator ditulis ulang | `crates/qh-ffi/src/bin/uniffi-bindgen.rs` | 5 baris: `uniffi::uniffi_bindgen_main()` |

Template itu penting justru karena ia satu-satunya cara jawaban ini bisa berubah: template askama
adalah berkas Source Code Form milik uniffi, dan menyalinnya ke sini akan membuat berkas itu
Covered Software — beserta kewajiban mempublikasikan modifikasinya. Tidak ada yang disalin.

Satu hal yang saya sebut terang-terangan meski kesimpulannya tetap "tidak": berkas di
`app/Generated/` (mis. `qh_ffi.swift`) adalah **keluaran** template, bukan teks template itu —
tidak ada satu pun hash-nya yang cocok dengan template mana pun. Menurut pembacaan saya, keluaran
codegen bukan Source Code Form dari template, jadi ia bukan Covered Software. Tapi ini satu-satunya
titik di seluruh analisis ini di mana orang lain bisa berpendapat lain, jadi ia ditulis di sini
alih-alih dibiarkan tersirat; kalau pemilik ingin konservatif, tempat berdebatnya adalah kalimat
ini, bukan yang lain.

## Opsi yang dipertimbangkan

| Opsi | Kelebihan | Kekurangan |
|---|---|---|
| **Pengecualian sempit per-crate untuk `MPL-2.0`** (dipilih) | Kode kita tetap MIT; nol perubahan pada kode, binding, dan perilaku; kewajibannya dapat dipenuhi dengan menyebut sumber yang sudah ada di `Cargo.lock` | Satu lisensi copyleft masuk ke graf; kontributor harus tahu §3.2 dan tidak boleh menyalin template; upgrade harus diperiksa ulang |
| Buang UniFFI, tulis permukaan FFI dengan tangan | Nol copyleft di graf; tidak ada generator di jalur build | ADR-0004 sudah menolak opsi C ABI manual karena alasan yang sama: seluruh pemetaan `Result`→`throws`, `Sendable`, kepemilikan memori, dan marshalling string harus ditulis tangan untuk empat belas perintah; kehilangan binding yang di-generate dan pemeriksaan CI bahwa binding tidak basi. Biaya L, dan membuka ulang ADR-0004 |
| Ganti dengan alternatif permisif (`swift-bridge`) | Lisensi permisif | ADR-0004 menimbang ini dan mencatat ekosistem serta riwayat produksinya lebih kecil dari UniFFI; menggantinya berarti menulis ulang seluruh control plane yang sudah berjalan |
| Vendor uniffi dan hindari codegen | Menghilangkan ketergantungan pada registry | **Tidak menyelesaikan apa pun, malah memperburuk**: tanpa codegen, binding Swift harus ditulis tangan (sama dengan opsi kedua) *dan* vendoring berarti kita mendistribusikan Source Code Form uniffi dari pohon kita sendiri, sehingga kewajiban §3.1/§3.2 menjadi pekerjaan kita, bukan sesuatu yang cukup ditunjuk |
| Naikkan proyek ini ke MPL-2.0 | Tidak perlu ADR pengecualian | Bertentangan langsung dengan ADR-0002 dan mengubah `LICENSE`; §8 memilih MIT agar adopsi seluas mungkin |
| Taruh `MPL-2.0` di `allow` global | Satu baris, sederhana | Berarti lisensi copyleft itu diterima untuk **crate apa pun** yang muncul di masa depan, tanpa ADR — persis pelonggaran senyap yang ADR-0002 melarang, dan persis alasan `exceptions` ada |

## Keputusan

**Pengecualian sempit: `MPL-2.0` diizinkan hanya untuk delapan crate uniffi, dan untuk tidak ada
yang lain.** Ditegakkan oleh `deny.toml` di akar repositori (berkas baru):

```toml
[licenses]
allow = ["MIT", "Apache-2.0", "BSD-3-Clause", "ISC", "Unicode-3.0", "Zlib"]
exceptions = [
    { allow = ["MPL-2.0"], crate = "uniffi" },
    ... delapan nama, satu per crate ...
]
```

Daftar `allow` itu adalah daftar ADR-0002, tidak kurang dan tidak lebih. Kedelapan nama ditulis
satu per satu karena package spec `cargo-deny` **tidak punya wildcard untuk nama crate**:
`crate = "uniffi*"` adalah nama literal yang tidak cocok dengan apa pun, dan percobaan pertama
memang gagal seperti itu (`warning[license-exception-not-encountered]`, sementara kedelapan crate
tetap ditolak). Konsekuensinya disengaja: upgrade uniffi yang memunculkan crate `uniffi_*`
kesembilan akan **gagal** sampai ia didaftarkan di sini dan ADR ini dibaca ulang — arah yang aman.
Tidak ada batasan `version` pada entri itu, karena `cargo update` rutin tidak mengubah nama maupun
lisensinya, dan pemeriksaan yang harus disunting setiap kali naik versi adalah pemeriksaan yang
akhirnya dihapus orang.

Verifikasi (`cargo deny check licenses`, cargo-deny 0.20.2):

- `error[rejected]` tersisa tepat **3**, dan **tidak satu pun** menyebut MPL-2.0 — kedelapan
  pengecualian cocok. Sebelum pengecualian ini ditulis, kedelapan baris MPL-2.0 itulah yang ditolak.
- Ketiga sisanya adalah `CDLA-Permissive-2.0` pada `webpki-root-certs` 1.0.9, `webpki-roots`
  0.26.11, dan `webpki-roots` 1.0.9.

## Alasan

1. Copyleft MPL-2.0 bekerja pada tingkat berkas dan hanya menyentuh Covered Software (§1.10);
   Larger Work boleh berlisensi apa pun (§3.3). Tidak ada berkas uniffi di pohon ini, jadi tidak
   ada bagian dari kode kita yang tertarik olehnya dan `LICENSE` tetap MIT.
2. Tidak ada berkas MPL-2.0 yang dimodifikasi atau disalin (tabel di atas), sehingga tidak ada
   kewajiban publikasi yang timbul dari §3.1. Yang tersisa hanya §3.2, dan itu pun terpenuhi
   dengan menunjuk sumber yang sudah tercatat persis di `Cargo.lock`.
3. Kegagalan yang hendak dicegah ADR-0002 bukan "ada crate MPL-2.0 di graf", melainkan
   **"lisensi copyleft masuk tanpa keputusan yang tercatat"**. ADR ini adalah keputusan yang
   tercatat itu, dan ia dibatasi sesempit mungkin supaya tidak menjadi izin umum.
4. Biaya alternatif utama — membuang UniFFI — nyata dan besar (lihat tabel Opsi), sedangkan
   manfaat menghindari MPL-2.0 di sini hampir tidak ada: tidak ada kode kita yang menjadi
   copyleft, dan produk tetap boleh dijual sebagai perangkat tertutup.

## Konsekuensi

**Kewajiban yang lahir dari ADR ini:**

- **Saat uniffi naik versi**, tiga hal diperiksa ulang: (a) `license` di manifest kedelapannya
  masih `MPL-2.0` — bukan lisensi lain yang kebetulan ditangani exception yang sama; (b) tidak ada
  berkas uniffi yang mulai di-vendor, di-patch, atau disalin; (c) tidak ada crate `uniffi_*` baru
  yang muncul — kalau ada, tambahkan ke `deny.toml` setelah membaca ADR ini. Pemeriksaannya
  sengaja gagal-tertutup: nama yang tidak terdaftar ditolak.
- **`Cargo.lock` adalah bagian dari cara kita memenuhi §3.2.** Versi dan checksum di sana yang
  memberi tahu penerima di mana Source Code Form yang persis itu diperoleh; catatan rilis dan SBOM
  menyebut versi uniffi yang dikirim. Membuang pinning itu melemahkan pemenuhan §3.2, bukan hanya
  soal reproduksibilitas build.
- **Kontributor tidak boleh menyalin template uniffi ke pohon ini** — berkas askama (`.swift`,
  `.kt`, `.py`, `.rb`) di `uniffi_bindgen`. Menyalin satu berkas saja menjadikannya Covered
  Software, dan §3.1 mewajibkan modifikasi atasnya dipublikasikan di bawah MPL-2.0. Kalau ada
  kebutuhan menyesuaikan keluaran codegen, yang boleh berubah adalah berkas di `app/Generated/`
  yang **di-generate ulang** lewat generator yang dipin (`crates/qh-ffi/src/bin/uniffi-bindgen.rs`),
  bukan templatenya. Kebutuhan menyalin template berarti ADR ini harus dibuka ulang lebih dulu.
- **`MPL-2.0` tidak boleh ditambahkan ke `allow` global**, dan field `license` pada crate uniffi
  tidak boleh diubah. Keduanya dinyatakan di sini supaya perubahan itu terlihat sebagai perubahan
  kebijakan, bukan sebagai penyesuaian konfigurasi.
- **`CDLA-Permissive-2.0` belum punya ADR, dan ADR ini tidak memberinya.** `cargo deny check
  licenses` gagal pada tiga crate `webpki-*` hari ini. Itu keputusan lisensi terpisah dengan
  pertimbangannya sendiri (datanya adalah daftar CA Mozilla, bukan kode yang kita tautkan), dan
  sampai keputusan itu ditulis, pemeriksaannya memang merah. Menambahkannya diam-diam ke
  `deny.toml` akan menjadi persis pelonggaran yang bagian ini melarang.

## Bila pemilik tidak setuju

Yang harus di-overrule adalah kalimat di bagian Keputusan: bahwa `MPL-2.0` diterima untuk uniffi.
Cara mengoverrule-nya sudah tersedia dan tidak ambigu — **buang uniffi dari graf**. Biayanya
dinyatakan terang-terangan, bukan dikubur: seluruh permukaan FFI empat belas perintah harus
ditulis tangan (pemetaan `Result`→`throws`, `Sendable`, kepemilikan buffer, dan pemetaan error yang
justru menjadi alasan UniFFI dipilih di ADR-0004), binding Swift yang di-commit diganti dengan
yang dirawat tangan beserta pemeriksaan kesegarannya, dan ADR-0004 harus dibuka ulang. Memindahkan
uniffi menjadi build-dependency saja **tidak** menyelesaikannya: yang dikirim aplikasi adalah
`libqh_ffi.dylib`, dan di dalamnya ada runtime uniffi (`nm -a target/debug/libqh_ffi.dylib` → 1731
simbol `uniffi_core`), jadi selama binding yang di-generate masih dipakai, `uniffi_core` tetap
tertaut. Perhatikan juga bahwa ongkos itu dibayar untuk menghilangkan lisensi yang, sesuai analisis
di atas, tidak menuntut apa pun dari kode kita selain menyebut sumbernya.
