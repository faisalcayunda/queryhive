# 0012 — Pengecualian CDLA-Permissive-2.0 untuk webpki-roots

- **Status:** Diterima
- **Tanggal:** 2026-09-23
- **Konteks instruksi:** handoff task 1

## Konteks

ADR-0002 menetapkan proyek tetap MIT dan daftar izin `cargo-deny` berisi `MIT`,
`Apache-2.0`, `BSD-3-Clause`, `ISC`, `Unicode-3.0`, `Zlib` — dengan aturan eksplisit bahwa
**setiap pengecualian baru harus melalui ADR**. ADR-0011 memberikan pengecualian pertama
untuk MPL-2.0 milik UniFFI, dinilai acceptable karena file-level copyleft dan §3.3 large
work exemption.

Graf yang Cargo resolve hari ini membawa tiga paket berlisensi `CDLA-Permissive-2.0`:

```
$ cargo deny check licenses 2>&1 | grep -A1 "license ="
   ┌─ /Users/isal/.cargo/registry/src/.../webpki-root-certs-1.0.9/Cargo.toml:26:12
26 │ license = "CDLA-Permissive-2.0"

   ┌─ /Users/isal/.cargo/registry/src/.../webpki-roots-0.26.11/Cargo.toml:25:12
25 │ license = "CDLA-Permissive-2.0"

   ┌─ /Users/isal/.cargo/registry/src/.../webpki-roots-1.0.9/Cargo.toml:26:12
26 │ license = "CDLA-Permissive-2.0"

$ cargo tree -i webpki-roots@1.0.9
webpki-roots v1.0.9
├── hyper-rustls v0.27.10
│   └── reqwest v0.12.28
│       └── qh-driver-trino v0.1.0
└── reqwest v0.12.28 (*)

$ cargo tree -i webpki-roots@0.26.11
webpki-roots v0.26.11
└── mysql_async v0.36.2
    └── qh-driver-mysql v0.1.0

$ cargo tree -i webpki-root-certs@1.0.9
webpki-root-certs v1.0.9
└── rustls-platform-verifier v0.6.2
    └── qh-driver-postgres v0.1.0
```

Ketiga paket ini adalah **bundled Mozilla CA root certificate data**, bukan kode eksekusi.
Mereka dibawa oleh `reqwest` (via `rustls`), `mysql_async`, dan `rustls-platform-verifier`
— jalur TLS yang benar-benar dikirim.

## Apa yang CDLA-Permissive-2.0 sebenarnya mewajibkan

Dicek terhadap teks lisensinya sendiri (https://cdla.dev/permissive-2-0/, diakses 2026-09-23):

- **§2.1 Grant of Data License**: lisensi non-eksklusif untuk menggunakan, memodifikasi, dan
  mendistribusikan Data.
- **§2.2 Publication of Data**: bila mempublikasikan Data, "make available … a copy of the
  Data and **any Additions** You created" — **ini** yang copyleft: modifikasi Data harus
  dibagikan.
- **§1.2 "Data"** didefinisikan sebagai informasi yang dilisensikan, dalam konteks ini adalah
  root certificate data Mozilla.
- **§1.1 "Additions"** adalah modifikasi atau penambahan pada Data itu sendiri.
- **§1.7 "Results"**: produk analisis, pengayaan, atau pemrosesan Data. Lisensi **tidak**
  mewajibkan Results dibagikan — hanya Additions.

Bandingkan dengan MPL-2.0 (file-level copyleft untuk kode): CDLA-Permissive adalah
**data-level copyleft**. Bila kamu mengubah daftar root CA-nya sendiri (menambah, mengurangi,
atau mengganti entri), perubahan itu harus dibagikan. Bila kamu menggunakan data itu untuk
memverifikasi koneksi TLS — yaitu membuat Results — kamu bebas.

## Terapan untuk repo ini

1. **Kita tidak memodifikasi Data-nya.** `webpki-roots` dipakai apa adanya: dependency
   transitif yang hanya dibaca oleh `rustls` untuk memverifikasi server certificate. Tidak ada
   Additions yang kami buat, jadi tidak ada kewajiban publikasi (§2.2 tidak terpicu).

2. **Kita mendistribusikan Data dalam Executable Form.** Root certificate data ter-embed dalam
   `libqh_ffi.dylib` sebagai konstanta, dan aplikasi mengirimkan lib itu. Tapi itu adalah
   **distribusi data yang tidak berubah**, bukan Additions — CDLA-Permissive mengizinkannya
   (§2.1).

3. **Source Code Form Data tetap tersedia, tanpa tindakan dari kita.** Sumber asli adalah
   `crates.io` (registry publik), yang menyimpan arsip `.crate` setiap versi. Pengguna dapat
   mengunduh `webpki-roots-1.0.9.crate` dari `https://crates.io/api/v1/crates/webpki-roots/1.0.9/download`
   — path yang Cargo sendiri sudah gunakan. `Cargo.lock` mencatat versi eksak yang kami pakai.

4. **Kode kita tetap MIT.** CDLA-Permissive adalah lisensi Data, bukan kode. Kode driver dan
   FFI yang *memakai* data itu adalah Results (§1.7), dan lisensi tidak membatasi Results.
   `LICENSE` tidak berubah.

## Alternatif yang ditolak

**Tidak memakai bundled root store, dan mengandalkan `rustls-platform-verifier` saja.**

Pros:
- `rustls-platform-verifier` membaca CA dari sistem (Keychain di macOS), jadi tidak ada
  bundled data yang perlu dilisensikan.
- CA korporat yang pengguna pasang di sistem langsung dipercaya, tanpa konfigurasi tambahan.

Cons:
- `mysql_async` 0.36.2 tidak mengekspos hook untuk custom verifier — `build_tls_connector`
  dan cached connector keduanya `pub(crate)`, dan setter root store-nya menerima tipe yang
  tidak di-re-export (probe gagal dengan `E0603`). Jadi untuk MySQL kita **tetap** perlu
  bundled root, dan menyingkirkannya untuk PostgreSQL/Trino saja tidak menghilangkan
  dependensi lisensi ini.
- `PROGRESS.md:542-544` sudah mencatat ini sebagai pekerjaan tersendiri yang membutuhkan
  hook dari upstream atau fork — bukan perbaikan cepat.

Karena `mysql_async` memblokir jalur ini hari ini, menolak `webpki-roots` untuk dua driver
lainnya adalah pekerjaan parsial yang tidak menyelesaikan masalah lisensi. Keputusan: terima
CDLA-Permissive untuk ketiga paket, dan catat pembatasan MySQL sebagai known limitation yang
memang sudah dicatat di `PROGRESS.md`.

## Konsekuensi

1. `deny.toml` diperluas dengan tiga entri `exceptions` untuk `webpki-roots` 0.26.11,
   `webpki-roots` 1.0.9, dan `webpki-root-certs` 1.0.9. Sama seperti MPL-2.0 di ADR-0011,
   ini adalah `exceptions` (bukan `allow`) supaya crate lain yang datang berlisensi
   CDLA-Permissive tetap harus disetujui eksplisit.

2. Komentar di `deny.toml:57-64` yang menyebut "NOT covered, on purpose" dihapus — ADR ini
   adalah coverage-nya.

3. `cargo deny check licenses` kini hijau tanpa melebarkan `allow`, dan setiap dependency
   baru yang membawa lisensi di luar daftar ADR-0002 tetap ditolak sampai ada ADR yang
   membenarkannya.

4. Limitation yang sudah dicatat di `PROGRESS.md:542-544` tetap berlaku: MySQL tidak bisa
   memverifikasi terhadap CA korporat dari Keychain hari ini, dan menutupnya butuh hook dari
   `mysql_async` atau fork. Keputusan ini tidak memperburuk limitation itu — ia menerima
   situasi yang sudah ada.

5. ADR ini **tidak** memberi blanket approval untuk setiap dependency berlisensi
   CDLA-Permissive di masa depan. Ia memberi approval untuk **webpki-roots family**, karena
   alasan yang spesifik untuk bundled root data: kita tidak memodifikasi Data-nya, dan
   sumber aslinya tersedia publik di `crates.io`.
