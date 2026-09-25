# 0007 — Tidak memakai App Sandbox

- **Status:** Diterima
- **Tanggal:** sesi Fase 0
- **Konteks instruksi:** §7.1 (distribusi)

## Konteks

§7.1 meminta keputusan App Sandbox diambil melalui ADR dengan mempertimbangkan akses file SSH key
dan `known_hosts`, serta distribusi di luar App Store. Fitur yang direncanakan menyentuh:

- `~/.ssh/` untuk membaca private key dan `known_hosts`,
- `ssh-agent` lewat socket di `SSH_AUTH_SOCK`,
- file kunci tambahan yang dipilih pengguna dari lokasi sembarang,
- `~/Library/Logs/` dan `~/Library/Caches/` untuk log dan file spill,
- koneksi jaringan ke host database (dan lewat SSH tunnel ke host lain).

## Opsi yang dipertimbangkan

| Opsi | Kelebihan | Kekurangan |
|---|---|---|
| **Tanpa sandbox** (hardened runtime + entitlements minimal) | `~/.ssh`, `SSH_AUTH_SOCK`, dan file pilihan pengguna dapat diakses tanpa panel NSOpenPanel untuk setiap file; distribusi di luar App Store dengan notarisasi tetap berjalan | Tidak ada batas sistem terhadap apa yang bisa disentuh aplikasi; perlindungan harus kami rancang sendiri |
| Sandbox + security-scoped bookmarks untuk key | Bisa masuk App Store | Setiap key dan setiap `known_hosts` perlu dipilih pengguna lewat panel; SSH agent socket tidak dapat diakses dalam sandbox tanpa entitlement khusus; `~/.ssh` tidak dapat diakses sama sekali secara default |
| Sandbox tanpa dukungan SSH key | Paling sederhana | Menghapus kemampuan SSH tunnel yang diwajibkan §7.1 |

## Keputusan

**Tanpa App Sandbox.** Distribusi lewat GitHub Releases + Homebrew Cask dengan hardened runtime,
code signing, dan notarisasi (§8.5). Aplikasi tidak akan diajukan ke Mac App Store.

## Alasan

Batas sandbox dan kebutuhan produk bertabrakan secara langsung, bukan secara teoretis:
`~/.ssh/known_hosts` harus dibaca **sendiri** oleh aplikasi untuk memverifikasi host key (§7.1),
dan SSH key harus bisa diserahkan ke `ssh-agent` lewat `SSH_AUTH_SOCK`. Memaksa setiap file
kunci/known_hosts melewati pemilih file pengguna akan membuat alur SSH tunnel tidak dapat dipakai,
dan itu fitur yang diwajibkan. Karena distribusi memang tidak lewat App Store, kehilangan
kemampuan masuk App Store tidak berbiaya apa pun di sini.

## Konsekuensi

**Karena tidak ada sandbox, batas keamanannya harus lebih banyak ada di kami sendiri:**

- **Entitlements minimal.** Hanya `com.apple.security.cs.allow-jit` bila benar-benar diperlukan
  (tidak perlu untuk Rust tanpa JIT) dan `com.apple.security.network.client`. Tidak ada
  `allow-unsigned-executable-memory`, tidak ada `disable-library-validation` kecuali terpaksa
  (bila terpaksa, alasannya dicatat di ADR baru).
- **Hardened runtime wajib**, bukan opsional; notarisasi menuntutnya dan ia yang mencegah injeksi
  library.
- **Akses file dibatasi oleh kami:** aplikasi hanya membuka file yang pengguna pilih atau yang ada
  di direktori miliknya sendiri (`~/Library/Application Support`, `Logs`, `Caches`). Tidak ada
  pemindaian direktori pengguna.
- **Tidak ada eksekusi kode dari jaringan.** Tidak ada plugin, tidak ada skrip yang diunduh. Ini
  berbeda dari engine Python sebelumnya, yang mengeksekusi skrip dari bundel — dengan Rust dan
  tanpa interpreter tertanam, permukaan itu hilang (nilai tambah yang tidak direncanakan).
- **Update harus ditandatangani** (Sparkle EdDSA), karena tanpa sandbox tidak ada lapisan sistem
  yang memverifikasi asal update.
- Threat model (§7.1) **wajib** mencakup konsekuensi ini secara eksplisit; tanpa sandbox,
  kegagalan merancang pembatasan sendiri berarti tidak ada pembatasan.
- Bila kelak App Store menjadi target, ADR ini dibuka kembali dan fitur SSH kemungkinan harus
  dipisahkan ke helper terpisah.
