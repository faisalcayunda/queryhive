# 0009 — `panic = "unwind"` pada crate FFI, dengan firewall `catch_unwind`

- **Status:** Diterima
- **Tanggal:** sesi Fase 0
- **Konteks instruksi:** §7.4 (kebijakan panic & unsafe), §4.4

## Konteks

Dua persyaratan bertemu di satu titik:

- §7.4 meminta panic **tidak boleh** melewati batas FFI, dan §4.4 mengulanginya sebagai aturan
  keamanan yang harus diuji. Panic yang menyeberang ke Swift adalah perilaku tidak terdefinisi dan
  menjatuhkan aplikasi — persis yang dilarang §7.2 ("tidak ada panic yang bisa menjatuhkan
  aplikasi karena data dari server").
- §7.4 juga meminta build Release yang dioptimalkan, dan `panic = "abort"` adalah cara umum
  mengecilkan binari Rust sekaligus menghindari unwinding table.

Keduanya tidak bisa dipenuhi bersamaan: **`panic = "abort"` membuat `catch_unwind` tidak berguna**
(panic langsung menghentikan proses, tidak ada yang bisa ditangkap).

## Opsi yang dipertimbangkan

| Opsi | Binari | Panic di batas FFI |
|---|---|---|
| `panic = "abort"` di seluruh workspace | Paling kecil | Menjatuhkan aplikasi; melanggar §7.4 dan §7.2 |
| `panic = "unwind"` di seluruh workspace + `catch_unwind` di setiap entry point FFI | Sedikit lebih besar (unwinding table) | Ditangkap dan diubah menjadi `EngineError::Internal` |
| `abort` di library, `unwind` hanya di crate FFI | Tidak dapat dicampur per-crate dengan cara yang aman: profil `panic` berlaku untuk seluruh graf kompilasi | — |

## Keputusan

**`panic = "unwind"` untuk seluruh graf kompilasi yang masuk XCFramework**, dengan `catch_unwind`
di **setiap** entry point `#[uniffi::export]` lewat helper `guard()` (blueprint §4.3). Binary CLI
`qh-ffi` (dipakai harness golden, tidak masuk aplikasi) memakai `panic = "abort"` karena tidak ada
batas FFI yang harus dilindungi — kestabilannya justru lebih baik bila crash terlihat jelas.

## Alasan

Keandalan aplikasi pengguna mengalahkan ukuran binari. Perbedaan ukuran antara `unwind` dan `abort`
pada crate ini berasal dari unwinding table, dan itu dapat ditekan dengan `lto = "fat"` +
`strip = "symbols"`; sementara kegagalan menjaga panic berarti satu nilai rusak dari server bisa
menutup aplikasi dan menghilangkan pekerjaan pengguna yang belum disimpan. Tidak ada pertukaran
yang sepadan di sini.

## Konsekuensi

- XCFramework membawa unwinding table; ukuran akhirnya dicatat di `docs/benchmarks.md` sebagai
  angka, bukan dugaan.
- `guard()` wajib ada di setiap entry point, dan **diuji**: satu test sengaja memicu panic pada
  jalur data server (mis. decoder `Value` dengan byte rusak) dan memastikan proses Swift tetap
  hidup serta menerima error yang bisa ditampilkan.
- Panic tetap dicatat dengan span `qh_ffi::panic` pada level `error`, karena panic berarti **bug
  kami**, bukan kesalahan pengguna — ia harus terlihat di log dan bisa dilampirkan ke bug report,
  bukan diam-diam menjadi pesan error biasa.
- Kebijakan `unsafe` (§7.4) tetap berlaku: `#![forbid(unsafe_code)]` di semua crate kecuali
  `qh-ffi`, `qh-result-store`, dan `qh-rt`, dan setiap blok `unsafe` wajib berkomentar `// SAFETY:`.
