# 0014 — Entitlements app: unsigned executable memory dan library validation

- **Status:** Diterima
- **Tanggal:** 2026-09-23
- **Konteks instruksi:** handoff task 6a, follow-up ADR-0007

## Konteks

ADR-0007 memutuskan aplikasi tidak memakai App Sandbox, dengan alasan akses `~/.ssh` dan
`known_hosts` untuk tunnel SSH (qh-tunnel). Keputusan itu menyebutkan "kami sendiri yang
menyediakan pembatasan (hardened runtime + entitlements minimal)" tapi tidak mendokumentasikan
entitlements mana yang diperlukan dan mengapa.

Entitlements aktif di `app/QueryHive.entitlements`:

```xml
<key>com.apple.security.cs.allow-unsigned-executable-memory</key>
<true/>
<key>com.apple.security.cs.disable-library-validation</key>
<true/>
```

Keduanya **melonggarkan** hardened runtime, bukan memperketatnya. ADR ini mendokumentasikan
mengapa kedua entitlement tersebut diperlukan dan tidak bisa dihindari tanpa mengubah
arsitektur FFI.

## Apa yang entitlements ini izinkan

### `com.apple.security.cs.allow-unsigned-executable-memory`

Mengizinkan aplikasi mengalokasikan memori yang **executable dan writable**, atau mengubah
perlindungan memori dari non-executable menjadi executable. Hardened runtime secara default
menolak ini untuk mencegah code injection.

**Mengapa ini diperlukan:** Swift runtime dan FFI dapat mengalokasikan executable memory untuk:
- JIT compilation dalam Swift runtime (meski ini jarang untuk app biasa)
- Dynamic dispatch optimizations
- **Foreign function interface (FFI) trampolines** — Swift memanggil Rust lewat
  `libqh_ffi.dylib`, dan bridge code dapat mengalokasikan executable memory

Tanpa entitlement ini, aplikasi akan crash saat memanggil FFI dengan error
`mach_vm_protect` atau `VM_PROT_WRITE | VM_PROT_EXECUTE`.

### `com.apple.security.cs.disable-library-validation`

Menonaktifkan validasi bahwa semua library yang dimuat harus ditandatangani oleh identitas
yang sama dengan executable utama, atau oleh Apple. Hardened runtime secara default
menegakkan ini untuk mencegat library injection.

**Mengapa ini diperlukan:** `libqh_ffi.dylib` dibangun dari workspace Rust
(`crates/qh-ffi/`) dan tidak ditandatangani dengan identitas yang sama dengan aplikasi
Swift. Jalur build-nya:

1. `cargo build --release -p qh-ffi` → `target/release/libqh_ffi.dylib`
2. `swift build` menautkannya dari `target/release` — dylib itu **tidak** disalin ke dalam
   bundle, jadi install name-nya tetap path absolut ke `target/release/deps/`
3. App me-load lib saat launch lewat FFI binding yang di-generate UniFFI

Langkah 2 itulah alasan entitlement ini masih diperlukan, dan sekaligus batas yang diketahui:
bundle hasil `app/build.sh` hanya jalan di mesin yang punya `target/release`. Membuat bundle
portabel berarti membangun crate ini sebagai `staticlib` dan mengemasnya sebagai XCFramework —
perubahan pada crate FFI, dicatat sebagai pekerjaan terpisah.

Tanpa entitlement ini, aplikasi akan crash saat launch dengan error signature validation
atau library load failure.

## Alternatif yang ditolak

### Tandatangani `libqh_ffi.dylib` dengan identitas yang sama

**Pros:** Bisa menonaktifkan `disable-library-validation`.

**Cons:**
- `cargo build` tidak menandatangani output-nya. Kita harus `codesign` manual setiap kali
  lib berubah.
- Menambah langkah build yang mudah terlupa: kalau developer build Rust tapi lupa sign,
  app launch akan crash dengan error yang tidak jelas.
- Signing membutuhkan developer certificate yang valid. Ini mempersulit local development:
  contributor baru harus setup signing identity lebih dulu.

Ditolak karena friction development terlalu tinggi untuk protection gain yang kecil — library
yang kita load **adalah milik kita sendiri**, bukan third-party arbitrary code. Validasi yang
sesungguhnya adalah bahwa lib itu keluar dari workspace kita, dan itu sudah terjaga.

### Batasi FFI call surface menjadi read-only

**Pros:** Kalau FFI tidak pernah perlu executable memory, bisa drop
`allow-unsigned-executable-memory`.

**Cons:**
- Bukan keputusan kita — Swift runtime dan libffi (yang UniFFI pakai) yang mengalokasikan
  executable memory untuk trampolines, bukan kode kita.
- Menonaktifkannya berarti app crash, bukan "FFI jadi read-only". Tidak ada cara
  opt-out dari trampoline allocation.

Ditolak karena tidak feasible — ini behavior Swift/libffi, bukan sesuatu yang bisa kita
batasi di app layer.

## Risiko yang diterima

Kedua entitlement ini **melemahkan hardened runtime**, dan itu adalah tradeoff yang disadari:

1. **`allow-unsigned-executable-memory` membuka vector code injection** — attacker yang bisa
   control memory allocation bisa inject executable code. Tapi:
   - Memory kita alokasikan terbatas pada FFI trampolines, bukan arbitrary user input.
   - Attacker butuh memory corruption bug di Rust atau Swift untuk memanfaatkannya.
   - Rust memory safety dan `#![forbid(unsafe_code)]` di semua crate kecuali `qh-rt`
     mengurangi attack surface ini.

2. **`disable-library-validation` mengizinkan loading unsigned libraries** — attacker yang
   bisa menulis ke bundle app bisa inject library. Tapi:
   - Kalau attacker sudah bisa menulis ke bundle, mereka bisa memodifikasi executable
     utama juga — library validation bukan defense terakhir.
   - macOS Gatekeeper dan notarization tetap memeriksa bundle saat pertama kali dibuka.

Kedua risiko ini **lebih kecil daripada friction yang dihindari**, dan lebih kecil daripada
risiko sandbox escape kalau kita memakai App Sandbox (ADR-0007 memilih no-sandbox karena
akses `~/.ssh` tidak bisa diminta lewat entitlement manapun).

## Keputusan

**Terima kedua entitlement ini sebagai konsekuensi arsitektur FFI**, dan dokumentasikan
mengapa masing-masing diperlukan.

Alasan:
1. **Ini bukan choice, ini requirement.** Swift FFI + Rust dylib membutuhkan keduanya untuk
   berfungsi. Alternative adalah tidak memakai FFI sama sekali — yaitu menulis ulang semua
   driver Rust dalam Swift, yang kontra dengan ADR-0001.

2. **Risiko terbatas oleh defense lain.** Memory safety Rust, `forbid(unsafe_code)` di
   hampir semua crate, dan Gatekeeper/notarization tetap berlaku.

3. **Friction development jauh lebih tinggi tanpa ini.** Developer harus sign lib setiap
   build, dan contributor baru butuh certificate. Ini menghambat kontribusi untuk protection
   gain yang minimal.

## Konsekuensi

1. **`app/QueryHive.entitlements` adalah file yang benar**, dan kedua entitlement di atas
   harus tetap ada selama aplikasi memakai FFI ke Rust dylib.

2. **App tidak boleh notarized tanpa review.** Apple notarization service memeriksa
   entitlements dan dapat menolak app dengan `allow-unsigned-executable-memory` kalau tidak
   ada justifikasi yang jelas. Saat submission:
   - Sebutkan FFI trampolines sebagai alasan `allow-unsigned-executable-memory`
   - Sebutkan unsigned Rust library sebagai alasan `disable-library-validation`

3. **Dokumentasi harus jujur tentang tradeoff ini.** Jangan klaim "fully sandboxed" atau
   "maximum security" — ini app dengan hardened runtime tapi **bukan** sandbox, dan dengan
   dua entitlement yang melonggarkan proteksi default.

4. **Future work: sign `libqh_ffi.dylib` bila friction berkurang.** Kalau cargo plugin
   untuk auto-sign atau CI yang handle signing jadi tersedia, revisit
   `disable-library-validation`. Tapi `allow-unsigned-executable-memory` tetap diperlukan
   selama Swift FFI memakai trampolines.

## Referensi

- ADR-0007: keputusan no-sandbox, menyebut "entitlements minimal" tapi tidak mendaftar
- Apple Hardened Runtime documentation:
  https://developer.apple.com/documentation/security/hardened_runtime
- Entitlement keys reference:
  https://developer.apple.com/documentation/bundleresources/entitlements
