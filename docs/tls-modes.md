# TLS modes — empat pilihan enkripsi untuk koneksi database

QueryHive mendukung empat mode TLS untuk koneksi PostgreSQL, MySQL, dan Trino. Mode yang
sama berlaku untuk ketiga driver, meski implementasinya berbeda per driver karena batasan
upstream.

## Empat mode

### `Disable`
Koneksi plaintext, tidak ada enkripsi.

**Kapan dipakai:**
- Development lokal di `127.0.0.1` atau `localhost`
- Server internal yang sengaja tidak memakai TLS
- Debug koneksi ketika TLS bermasalah

**Risiko:** Semua traffic (termasuk password dan query) dikirim dalam bentuk teks terbuka.

---

### `Prefer` (default)
Coba TLS lebih dulu, fallback ke plaintext bila server tidak support.

**Semantik penting:** Mode ini **tidak memverifikasi sertifikat**. Ini keputusan yang
disengaja untuk kompatibilitas dengan psycopg dan pymysql, yang berperilaku sama pada
mode `prefer` mereka.

**Kapan dipakai:**
- Default untuk semua koneksi baru
- Server internal yang bisa pakai TLS tapi tidak punya sertifikat valid
- Kamu ingin enkripsi bila tersedia, tapi tidak ingin koneksi ditolak hanya karena cert

**Yang dilindungi:** Passive eavesdropping (orang yang hanya bisa mendengarkan traffic).

**Yang TIDAK dilindungi:** Active MITM attack. Attacker yang bisa memodifikasi traffic bisa
downgrade ke plaintext atau present cert palsu — dan mode ini akan terima keduanya.

**Fallback hanya pada satu error:** `NoClientSslFlagFromServer` (PostgreSQL/MySQL) atau TLS
handshake pertama gagal (Trino). Handshake yang rusak atau cert invalid **tidak** fallback
— itu tetap error.

---

### `Require`
TLS wajib, dan sertifikat server diverifikasi terhadap system trust store.

**Kapan dipakai:**
- Koneksi ke server production/cloud
- Server dengan sertifikat valid dari CA publik
- Kamu ingin proteksi penuh dari passive eavesdropping dan active MITM

**Yang dilindungi:** Passive eavesdropping dan active MITM attack (selama cert valid).

**Limitasi per-driver:**
- **PostgreSQL & Trino:** Memakai `rustls-platform-verifier`, jadi CA korporat yang
  dipasang di Keychain langsung dipercaya.
- **MySQL:** Hanya memverifikasi terhadap bundled root store (webpki-roots). CA korporat
  **tidak dipercaya**, dan koneksi akan ditolak dengan error cert. Ini limitasi
  `mysql_async` 0.36 — upstream tidak expose hook untuk custom verifier.

---

### `RequireNoVerify`
TLS wajib, tapi sertifikat server **tidak** diverifikasi.

**Kapan dipakai:**
- Server internal dengan self-signed cert yang kamu percaya (misalnya kamu yang setup)
- Development/staging dengan cert sementara
- Kamu ingin enkripsi tapi tidak peduli identitas server

**Yang dilindungi:** Passive eavesdropping (sama seperti `Prefer` bila TLS berhasil).

**Yang TIDAK dilindungi:** Active MITM attack. Attacker bisa present cert apapun dan
koneksi akan diterima.

**Peringatan UI:** Mode ini harus diberi warning di UI — "This is what an attacker on the
network needs to read everything." Ini bukan mode "aman tapi permissive"; ini mode
"enkripsi tanpa autentikasi".

---

## Pemetaan libpq `sslmode` → `TlsMode`

Aplikasi menerima kosakata libpq untuk PostgreSQL compatibility, tapi artinya **tidak
persis sama**:

| libpq `sslmode` | QueryHive `TlsMode` | Catatan |
|---|---|---|
| `disable` | `Disable` | Sama |
| `prefer` atau kosong | `Prefer` | Default, tidak verifikasi cert |
| `require` | `RequireNoVerify` | **Beda:** libpq `require` juga tidak verifikasi |
| `verify-ca`, `verify-full` | `Require` | Ini yang verifikasi cert |

**Mengapa berbeda:** libpq `require` mengenkripsi tanpa verifikasi — itu `RequireNoVerify`
di kosakata kami. Kita tidak punya mode "wajib TLS dan verifikasi" yang namanya `require`,
karena nama itu sudah dipakai libpq untuk "wajib TLS tanpa verifikasi".

Ejaan yang tidak dikenal **ditolak** dengan error, bukan diam-diam menjadi default. Salah
ketik `sslmode` tidak boleh menentukan apakah koneksi dienkripsi tanpa memberi tahu siapa
pun.

---

## Implementasi per-driver

### PostgreSQL (`qh-driver-postgres`)
- `Disable`: `NoTls` connector
- `Prefer`: Coba `rustls` connector, fallback `NoTls` pada `NoClientSslFlagFromServer`
- `Require`: `rustls` connector dengan `rustls-platform-verifier` (system trust store +
  Keychain CA)
- `RequireNoVerify`: `rustls` connector dengan `NoVerifier` (accept any cert)

**Verifikasi:** `PROGRESS.md:513-549` merekam keputusan `Prefer` tidak verifikasi untuk
match psycopg behavior.

### MySQL (`qh-driver-mysql`)
- `Disable`: `SslOpts::default()` (plaintext)
- `Prefer`: `mysql_async` tidak support prefer; kami emulate dengan `RequireNoVerify` +
  retry plaintext bila handshake gagal
- `Require`: `SslOpts` dengan `rustls` + bundled root store (`webpki-roots`)
- `RequireNoVerify`: `SslOpts` dengan `rustls` + `NoVerifier`

**Limitasi:** `mysql_async` 0.36 tidak expose hook untuk `rustls-platform-verifier`.
`build_tls_connector` dan cached connector keduanya `pub(crate)`, dan setter root store
menerima tipe yang tidak di-re-export (probe gagal dengan `E0603`). Jadi `Require` hanya
verifikasi terhadap bundled root — CA korporat di Keychain **tidak dipercaya** dan koneksi
akan ditolak.

Workaround butuh upstream hook atau fork, plus medan `ssl_ca` di `ConnectionConfig` —
pekerjaan tersendiri yang dicatat di `PROGRESS.md:542-544`.

### Trino (`qh-driver-trino`)
- `Disable`: `scheme = "http"`
- `Prefer`: `scheme = "https"` dengan `NoVerifier`, fallback `http` bila TLS handshake
  pertama gagal
- `Require`: `scheme = "https"` dengan `rustls-platform-verifier`
- `RequireNoVerify`: `scheme = "https"` dengan `NoVerifier`

**Perbedaan dari database lain:** Trino tidak punya flag "server support SSL" yang bisa
dicek. Jadi `Prefer` berarti "coba HTTPS, bila gagal coba HTTP sekali". Error handshake
yang bukan koneksi ditolak (misal cert invalid) **tidak** fallback — itu tetap error.

---

## Rekomendasi pemilihan

| Skenario | Mode yang tepat |
|---|---|
| Development lokal (127.0.0.1) | `Disable` |
| Server internal tanpa cert valid, kamu percaya jaringan | `Prefer` |
| Server internal self-signed yang kamu setup sendiri | `RequireNoVerify` + warning |
| Production/cloud dengan cert valid | `Require` |
| Tidak tahu, ingin enkripsi bila bisa | `Prefer` (default) |

**Jangan pakai `RequireNoVerify` kecuali kamu tahu mengapa.** Ini bukan "Require yang
lebih permissive" — ini "enkripsi tanpa autentikasi", yang sama rentannya dengan plaintext
terhadap active attacker.

---

## Referensi

- `crates/qh-driver/src/lib.rs` — definisi `TlsMode` enum
- `PROGRESS.md:513-549` — keputusan `Prefer` tidak verifikasi + pemetaan libpq
- `crates/qh-driver-postgres/src/tls.rs` — implementasi PostgreSQL TLS
- `crates/qh-driver-mysql/src/lib.rs:31-40` — implementasi MySQL TLS + limitasi
- `crates/qh-driver-trino/src/lib.rs:212-244` — implementasi Trino TLS
