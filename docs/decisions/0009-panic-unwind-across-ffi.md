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

**`panic = "unwind"` untuk seluruh graf kompilasi**, dengan `catch_unwind` di **setiap** entry
point `#[uniffi::export]` lewat helper `guard()` (blueprint §4.3). Rencana awal di ADR ini —
memberi binary CLI `qh-ffi` (yang dipakai harness golden dan tidak masuk aplikasi) `panic = "abort"`
karena tidak ada batas FFI yang harus dilindungi — tidak dijalankan dan tidak bisa dijalankan
seperti ditulis: profil `panic` berlaku untuk seluruh graf kompilasi, persis alasan di baris ketiga
tabel Opsi di bawah. Yang ada di pohon hari ini satu profil, `panic = "unwind"` (`Cargo.toml`,
`[profile.release]`), dan binary CLI-nya pun butuh unwinding karena ia menjalankan perintahnya di
dalam `catch_unwind` supaya panic menjadi `error` event alih-alih backtrace tanpa JSON
(`crates/qh-ffi/src/main.rs`).

## Alasan

Keandalan aplikasi pengguna mengalahkan ukuran binari. Perbedaan ukuran antara `unwind` dan `abort`
pada crate ini berasal dari unwinding table, dan itu dapat ditekan dengan `lto = "fat"` +
`strip = "symbols"`; sementara kegagalan menjaga panic berarti satu nilai rusak dari server bisa
menutup aplikasi dan menghilangkan pekerjaan pengguna yang belum disimpan. Tidak ada pertukaran
yang sepadan di sini.

## Konsekuensi

- Unwinding table menambah ukuran artefak, dan itu **sudah diukur** 24 Sep 2026, bukan lagi
  dijanjikan: `docs/benchmarks.md` §"Ongkos `panic = \"unwind\"`" mencatat 1,69 MiB pada binary app
  yang dikirim (16,32 MiB berbanding 14,63 MiB bila `panic = "abort"`), dan 9,93 MiB pada arsip
  mentah yang tidak pernah dikirim. Angka arsip sengaja ditulis berdampingan supaya tidak ada yang
  membacanya sebagai ongkos distribusi.
- `guard()` wajib ada di setiap entry point, dan **diuji**: satu test sengaja memicu panic pada
  jalur data server (mis. decoder `Value` dengan byte rusak) dan memastikan proses Swift tetap
  hidup serta menerima error yang bisa ditampilkan.
- Panic tetap dicatat dengan span `qh_ffi::panic` pada level `error`, karena panic berarti **bug
  kami**, bukan kesalahan pengguna — ia harus terlihat di log dan bisa dilampirkan ke bug report,
  bukan diam-diam menjadi pesan error biasa.
- Kebijakan `unsafe` (§7.4) tetap berlaku: setiap blok `unsafe` wajib berkomentar `// SAFETY:`.
  Daftar pengecualian yang semula ditulis di sini tidak bertahan terhadap kode: `qh-result-store`
  memakai `#![forbid(unsafe_code)]` dan tidak memuat `unsafe` sama sekali, dan satu-satunya blok
  `unsafe` di workspace hari ini ada di `qh-rt` (pengaturan QoS). `qh-ffi` juga tidak memuat blok
  `unsafe` sendiri — permukaan FFI-nya dihasilkan `uniffi` (`grep -rn "unsafe " crates/*/src/`).
