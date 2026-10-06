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
TLS wajib, dan sertifikat server diverifikasi terhadap system trust store, atau terhadap
berkas CA yang kamu tunjuk untuk koneksi itu (lihat "CA per koneksi" di bawah).

**Kapan dipakai:**
- Koneksi ke server production/cloud
- Server dengan sertifikat valid dari CA publik
- Kamu ingin proteksi penuh dari passive eavesdropping dan active MITM

**Yang dilindungi:** Passive eavesdropping dan active MITM attack (selama cert valid).

**Limitasi per-driver:**
- **PostgreSQL & Trino:** Memakai `rustls-platform-verifier`, jadi CA korporat yang
  dipasang di Keychain langsung dipercaya. CA yang tidak ada di Keychain bisa ditunjuk per
  koneksi lewat berkas PEM (lihat bawah).
- **MySQL:** Hanya memverifikasi terhadap bundled root store (webpki-roots). CA korporat
  **tidak dipercaya**, dan koneksi akan ditolak dengan error cert. Ini limitasi
  `mysql_async` 0.36 — upstream tidak expose hook untuk custom verifier, dan berkas CA per
  koneksi juga tidak bisa dipasang.

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

## CA per koneksi (PostgreSQL dan Trino)

Bila CA privat tidak ada di Keychain (atau kamu tidak mau memasangnya untuk seluruh
mesin), koneksi bisa menunjuk berkas PEM berisi CA-nya. Dari titik itu sertifikat server
diperiksa terhadap **bundel itu saja**, tanpa trust store sistem, dan **nama host tetap
diperiksa** terhadap SAN sertifikat.

Berkas dibaca dan divalidasi **sekali**, saat koneksi disusun. Byte hasil parse (DER) ikut
dibawa di `ConnectionConfig.tls_ca`, jadi tidak ada pembacaan kedua saat connect: berkas
yang ditukar di antara validasi dan pemakaian tidak mengubah apa yang dipercaya. Jalur
berkas ikut identitas koneksi dan isinya ikut hash kredensial pool.

**Berkas ditolak, dengan nama berkasnya dan alasannya, bila:**
- lebih besar dari 1 MiB (itu bukan bundel CA);
- memuat blok `PRIVATE KEY` dalam bentuk apa pun (`RSA`, `EC`, `ENCRYPTED`): kamu menunjuk
  berkas yang salah, dan program ini tidak boleh membaca kunci privat yang tidak diminta;
- tidak memuat sertifikat PEM satu pun (berkas DER harus dikonversi dulu, misalnya
  `openssl x509 -inform der`), atau PEM-nya rusak;
- memuat lebih dari 64 sertifikat;
- satu sertifikatnya tidak bisa dipakai sebagai trust anchor. Sertifikat itu **ditolak
  dengan nomornya**, tidak dibuang diam-diam, karena bundel yang lebih kecil dari yang kamu
  kira mengubah apa yang dipercaya tanpa kabar.

**Hanya berlaku di mode `Require`.** Di `Disable`, `Prefer`, `RequireNoVerify`, atau dengan
`DB_INSECURE`, berkas CA akan diabaikan, dan koneksi akan terlihat terpatok ke CA-mu padahal
tidak. Itu lebih buruk dari penolakan, jadi kombinasinya ditolak dengan `Usage` sebelum
socket dibuka. Driver memeriksanya sendiri (`ConnectionConfig::ca_for_verifying_mode`),
terlepas dari pemeriksaan di lapisan setelan.

**Isi berkasnya CA, bukan sertifikat servernya.** Sertifikat server yang ditandatangani
dirinya sendiri (mis. keystore self-signed bawaan Trino) tidak bisa dijadikan trust anchor
oleh `rustls`/`webpki`: yang diterima adalah CA (atau intermediate) yang menandatangani
sertifikat server. Untuk server self-signed yang kamu percaya, `RequireNoVerify` adalah
pilihan yang jujur.

**Host berupa IP:** sertifikat harus punya SAN bertipe IP. SAN DNS tidak cocok dengan
alamat IP.

**Tidak ada TOFU untuk sertifikat TLS.** Tidak ada prompt "percayai sertifikat ini" dan
tidak ada pin sertifikat server. Kepercayaan TLS hanya datang dari trust store sistem, dari
berkas CA yang **kamu pilih** untuk koneksi itu, atau dari `RequireNoVerify` yang kamu pilih
dan yang diberi peringatan.

**Status:** lapisan driver (`TlsCa`, `roots_from`, pemeriksaan mode) ada di W11-T2b1d. Setelan
`DB_CA_FILE` (pengembangan `~`, pesan `Usage` per setelan, MySQL ditolak dengan alasannya)
dibaca `config::build` di W11-T2b2.

---

## Nama TLS lewat tunnel SSH

Lewat tunnel SSH, engine mengarahkan koneksi ke `127.0.0.1:<port lokal>`, sedangkan
sertifikat server diterbitkan untuk nama database aslinya. Tanpa penanganan khusus, mode yang
memverifikasi memeriksa `127.0.0.1` terhadap sertifikat yang tidak memuatnya, jadi setiap
mode verifikasi gagal lewat tunnel.

`ConnectionConfig.tls_server_name` membawa nama aslinya. Alamat (`host`) tetap loopback tunnel,
dan driver memisahkan **nama yang diperiksa** dari **alamat yang dihubungi**:

- **PostgreSQL:** `host = <nama>` dan `hostaddr = 127.0.0.1`. `tokio-postgres` memakai `host`
  untuk nama TLS (dan SNI), `hostaddr` untuk soket.
- **Trino:** URL sesi memakai `https://<nama>:<port lokal>`, dan `reqwest` meresolusikan
  `<nama>` ke loopback (`ClientBuilder::resolve`). SNI, verifikasi, dan header `Host` memakai
  nama asli, soketnya ke tunnel. Port diambil dari URL, bukan dari alamat hasil resolusi.
- **MySQL:** tidak bisa memverifikasi sertifikat lewat tunnel (nama yang diperiksa tetap
  `127.0.0.1`). Pemakai memilih `require` (terenkripsi, tidak diverifikasi: tunnel sudah
  mengautentikasi bastion) atau terhubung langsung. Penolakannya dengan pesan bernama ada di
  lapisan setelan (W11-T2b2).

`host` harus berupa alamat IP bila `tls_server_name` diisi: nama itu butuh tempat untuk dituju,
dan driver tidak melakukan lookup DNS untuknya. Host yang bukan alamat ditolak dengan `Usage`.

Tes membuktikan celahnya lebih dulu, baru perbaikannya: tanpa `tls_server_name`, sertifikat
untuk `db.internal` ditolak saat dihubungi lewat `127.0.0.1`; dengan nama itu, diterima.

---

## Kredensial Trino hanya dikirim ke satu alamat

Dua dari tiga URL yang dipakai satu statement Trino ditentukan **server**: `nextUri` pada setiap
polling halaman, dan `DELETE` untuk cancel. Redirect bisa datang dari mana saja. Memasang
`Authorization` pada URL apa pun yang datang berarti: koordinator di belakang load balancer yang
mengakhiri TLS tanpa meneruskan header skema akan mengembalikan `nextUri` berskema `http://`, dan
password atau token keluar sebagai teks jelas pada polling pertama walau sesi dibuka lewat
`https`. `nextUri` ke host lain mengirim kredensial ke host itu.

Aturannya, berlaku untuk **password Basic maupun token bearer**:

1. **Pemeriksaan origin per permintaan.** Sebelum request dibangun, skema, host, dan port URL
   dibandingkan dengan alamat yang dipakai sesi. Beda berarti galat bernama, `Permanent`, yang
   menyebut kedua alamat (tanpa path dan tanpa kredensial) dan bahwa kredensial tidak dikirim.
   Cancel yang ditolak tidak mengirim apa pun dan tidak jatuh ke URL turunan basis.
2. **Tanpa redirect.** Klien yang memegang password atau token tidak mengikuti redirect
   (`client_for_secret`). `reqwest` membuang `Authorization` saat redirect hanya bila host atau
   port berubah, tidak bila hanya skema yang berubah: redirect `https` ke `http` pada host dan
   port yang sama akan membawa kredensialnya ke teks jelas. Respons 3xx dikembalikan apa adanya
   sebagai galat.
3. **`Prefer` dengan password tidak turun ke `http`.** Satu-satunya sinyal yang membolehkan
   fallback (koordinator tidak berbicara TLS) adalah persis kasus ketika password terkirim
   jelas. Sesi dengan secret tidak punya klien plaintext cadangan, dan galatnya menjelaskan
   alasannya.
4. **Token bearer hanya lewat `https`**, dan hanya di mode `Require` atau `RequireNoVerify`.
   `Disable` dan `Prefer` ditolak saat connect.

Sesi **tanpa** password dan tanpa token tidak diperiksa dan tidak berubah: tidak ada yang bisa
bocor, dan koordinator tanpa autentikasi yang mengumumkan alamat lain tetap berjalan.

**Harganya, disengaja.** Koneksi password yang `nextUri`-nya berhost atau berport lain dari yang
kamu ketik kini gagal dengan galat bernama (misalnya tersambung lewat IP sementara koordinator
mengumumkan nama DNS-nya, atau lewat tunnel dengan port TLS asli di `nextUri`). Jalan keluarnya:
sambung lewat alamat yang diumumkan koordinator, atau perbaiki alamat eksternal dan penanganan
header forwarded di koordinator. Alternatifnya mengirim password ke alamat yang tidak kamu pilih.

Tes (`crates/qh-driver-trino/tests/credential_origin.rs`) memakai koordinator palsu dan, untuk
tiap kasus, memeriksa bahwa tidak ada request yang sampai ke alamat salah membawa
`Authorization`. Satu tes kontrol membuktikan fake-nya bisa melihat kebocoran: klien `reqwest`
biasa dengan kebijakan redirect bawaan **bocor** pada fake yang sama.

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
  Keychain CA), atau dengan `WebPkiServerVerifier` atas bundel CA koneksi bila ada
  (`tls::roots_from` + `tls::verifier_with_roots`)
- `RequireNoVerify`: `rustls` connector dengan `NoVerifier` (accept any cert)
- Lewat tunnel: `host(<nama>)` + `hostaddr(127.0.0.1)`

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
- `Require`: `scheme = "https"` dengan `rustls-platform-verifier`, atau dengan bundel CA
  koneksi bila ada (`roots_from` + `with_root_certificates`)
- `RequireNoVerify`: `scheme = "https"` dengan `NoVerifier`
- Klien yang memegang password atau token: `client_for_secret`, tanpa redirect
- Lewat tunnel: URL memakai `tls_server_name`, di-resolve ke loopback

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
- `crates/qh-driver/src/ca.rs` — bundel CA per koneksi (`TlsCa`): parse dan validasi PEM
- `crates/qh-driver-postgres/src/tls.rs` — implementasi PostgreSQL TLS
- `crates/qh-driver-mysql/src/lib.rs:31-40` — implementasi MySQL TLS + limitasi
- `crates/qh-driver-trino/src/lib.rs` — implementasi Trino TLS (`client_for`,
  `client_for_secret`, `authorized`)
- `crates/qh-driver-trino/tests/credential_origin.rs`, `ca_file.rs` dan
  `crates/qh-driver-postgres/tests/ca_file.rs` — tes CA, nama TLS, dan origin kredensial
- `docs/architecture/blueprints/w11-metadata-and-connections.md` §7 — rancangan
