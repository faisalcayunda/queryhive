# 0040 — JWT Trino dan CA per koneksi, dengan TLS server name lewat tunnel

- **Status:** Direncanakan. Struktur data dan aturan validasi sudah ditentukan blueprint w11 §7 (D-14, D-15, D-16). Kode JWT, CA, dan server name sudah siap di blueprint yang sudah disetujui architect-reviewer (6 Okt 2026, dengan koreksi SEC satu temuan blocking). Keputusan W11-T2b belum menjalankan kode: **W11-T2b1d (JWT Trino) tidak mendarat di commit yang disebut ADR ini** (dipandu blueprint tapi belum coded); W11-T2b1 (tunnel dan SSH config parsing) ada di `401396c`; keputusan mana dari D-14/D-15 (Trino, PostgreSQL, MySQL) sebagai lapis pertama atau bersama-sama ditentukan di brief subtask T2b1d.
- **Tanggal:** 6 Okt 2026 (W11-D, perf-parity Fase 11)
- **Konteks instruksi:** `docs/architecture/development-plan.md` §4 (jadwal ADR, baris 0040) dan W11-D; `docs/architecture/blueprints/w11-metadata-and-connections.md` §2, §7 (D-14, D-15, D-16), §23; PRD `docs/architecture/prd-performance-and-parity.md` P-08, FR-CON-07 (JWT hanya HTTPS), FR-CON-08 (CA per koneksi), UC-01, UC-18; keputusan pemilik O-22, O-26.
- **Tidak menggantikan:** ADR lain. Pendamping: ADR-0039 (SSH TOFU, dalam satu W11-D), ADR-0038 (metadata), ADR-0031 (tunnel di engine).

## Konteks

Hari ini Trino dan PostgreSQL bisa dihubungi lewat tunnel SSH tanpa verifikasi nama TLS (memverifikasi terhadap `127.0.0.1`), membuat UC-01 (koneksi lewat bastion dengan sertifikat CA internal) dan UC-18 (TLS ke proxy yang tidak diverifikasi) tidak bisa digabung dengan keamanan. Trino tidak punya cara mengirim JWT (token bearer yang umum di platform modern), dan MySQL tidak bisa menggunakan CA internal karena `mysql_async` tidak mengekspor `RootCertStore`. FR-CON-07 dan FR-CON-08 menuntut satu setelan `DB_JWT` untuk Trino dengan pemeriksaan origin per-permintaan, dan setelan `DB_CA_FILE` untuk PostgreSQL dan Trino dengan pembacaan berkas sekali dan hash di kunci pool.

Blueprint dan tes live yang masih ditunggu menunjukkan celah keamanan di driver Trino yang ada: `Authorization: Bearer` dikirim ke URL yang diberikan server tanpa memeriksa bahwa URL itu dari origin sesi yang sama. Koreksi memerlukan satu fungsi `authorized(client, method, url, credentials)` yang memeriksa sebelum header dipasang. `nextUri` dari koordinator bisa `http`, berhost lain, atau redirect TLS ke HTTP, setiap skenario membawa token ke penyerang kecuali diperiksa.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Format JWT | **Bearer token opak, hanya PostgreSQL dan Trino, hanya `Require` atau `RequireNoVerify`, tidak bersama password** | Token buram (app tidak perlu decode), tanpa sila "kedaluwarsa": itu galat 401 dari server. Tidak perlu refresh. Satu setelan `DB_JWT` | Tidak bisa memvalidasi kedaluwarsa di klien; 401 dari server menyampaikan "token may have expired" |
| | JWT terstruktur dengan decode dan validasi | Kedaluwarsa diketahui lebih dulu | Token spesifik ke platform; decode perlu kunci publik; perubahan kunci perlu rollover |
| | Bersama password | Opsi fleksibel | Ambigu: mana yang dikirim? |
| Pemeriksaan origin | **Per permintaan: sebelum header dipasang, periksa skema, host, port terhadap origin sesi** | Menghentikan bocor header ke URL server dan ke redirect tanpa perlu perubahan klien HTTP | Duplikasi pemeriksaan di tiga pemanggil (POST, GET, DELETE); fungsi bersama yang sama menghindarinya, tetapi callee bisa lupa |
| | Pemeriksaan basis sesi saja | Lebih sederhana | `nextUri` dari server bisa berhost atau port lain, dan header akan dikirim tanpa pemeriksaan |
| | HTTPS mode yang ketat | Tidak mengirim ke `http` | Redirect TLS → HTTP pada host dan port yang sama lolos (reqwest default, hanya melihat host/port bukan skema) |
| CA per koneksi | **Berkas PEM, dibaca sekali, divalidasi di saat `config::build`, byte ikut ConnectionConfig, hash di kunci pool** | Tidak ada TOCTOU antara validasi dan connect; byte tidak berubah; hash membuat isi berkas masuk identitas pool | Berkas berubah di disk tidak terdeteksi (satu baca); berkas akses terbatas tidak bisa divalidasi tanpa ada (bukan saat connect) |
| | Berkas dibaca saat connect | Validasi terbaru | TOCTOU: berkas bisa dihapus atau diubah antara validasi di satu thread dan connect di thread lain |
| | Kirim jalur saja ke driver | Lebih ringan | Driver MySQL tidak mengekspor RootCertStore untuk menerima CA; duplikasi parsing di setiap driver |
| TLS server name | **Di ConnectionConfig, diisi `retarget` saat tunnel dibuat, dipakai PostgreSQL sebagai nama dan Trino sebagai part URL** | Nama asli terpisah dari alamat tunnel; verifikasi nama terhadap sertifikat bekerja; header `Host:` dan `User-Agent` terkesan benar | PostgreSQL `hostaddr` tidak bisa mengalihkan sepenuhnya (nama tetap dipakai SAN check); Trino resolve perlu port dari URL (bukan bisa dari argumen) |
| | Tetap `127.0.0.1` | Tidak perlu perubahan API | UC-01 dan UC-18 tidak bisa digabung (sertifikat bernama real database, bukan loopback) |

## Keputusan

**Trino menerima token JWT bearer yang opak untuk autentikasi, hanya dalam mode TLS `Require` atau `RequireNoVerify`, bukan dengan password. PostgreSQL dan Trino menerima berkas CA yang dibaca sekali, divalidasi terhadap PEM, dan hash ikut kunci pool. Setiap permintaan yang membawa rahasia (JWT atau password) memeriksa origin (`scheme://host:port`) sebelum header dipasang, dan tolak permintaan ke origin lain dengan galat bernama `Permanent`. PostgreSQL dan Trino menggunakan nama TLS asli lewat `ConnectionConfig.tls_server_name`, diisi oleh `retarget` saat tunnel dibuat.**

Keputusan desain yang mengikat (blueprint §7, D-14, D-15, D-16), belum dijalankan:

1. **JWT di Trino.** Setelan `DB_JWT` (bawaan kosong) menerima token bearer yang opak, ≤ 8.192 karakter, karakter token68 saja (`A-Za-z0-9-._~+/=`). Validasi bentuk tanpa mengecho nilai (`Usage` tanpa token). Hanya untuk `DB_KIND=trino`, hanya mode `Require` atau `RequireNoVerify` (tolak `Prefer`, `Disable`, atau `DB_INSECURE=1` dengan kata `Usage`), tidak bersama `DB_PASSWORD` (satu-satunya: `Usage` "send one"). Rute token ke driver Trino sebagai `Credentials.bearer: Option<String>`, yang diteruskan ke fungsi `authorized()` sebelum dipasang ke header `Authorization: Bearer <token>` (**pengganti** Basic). Cites: blueprint §7.1 D-14.
2. **Origin per permutasi.** `Credentials` membawa `origin: Option<Origin>` (scheme huruf kecil, host huruf kecil, port eksplisit atau bawaan skema). Diisi saat `connect` dari basis sesi (diperhitungkan ulang bila `Prefer` fallback mengubah skema). `authorized(client, method, url, &credentials) -> Result<RequestBuilder, EngineError>` dijalankan **sebelum** permintaan dibangun: parse URL, bandingkan skema/host/port dengan origin, tolak dengan galat bernama `Permanent` bila beda ("the coordinator sent a page URL on http://…, not on https://… (the address this connection uses), so the credentials were not sent…"). Tiga pemanggil: POST ke basis sesi, GET ke `nextUri`, DELETE ke `running.nextUri`. Cites: blueprint §7.1 (b).
3. **Redirect tanpa header untuk bearer.** Klien yang dibangun untuk sesi dengan `DB_JWT` menggunakan `redirect(Policy::none())`, sehingga tidak ada redirect yang diikuti otomatis (3xx jatuh ke cabang galat yang ada). Klien Basic tetap redirect bawaan (risiko tetap ada; keputusan pemilik, §16 blueprint: menunggu O-22). Cites: blueprint §7.1 (b).
4. **CA per koneksi.** Setelan `DB_CA_FILE` (bawaan kosong) menerima jalur berkas PEM, `~` dikembangkan, ≤ 1 MiB, 1 sampai 64 sertifikat X.509 tanpa `PRIVATE KEY`. Validasi format di `config::build` (bukan saat connect), dengan galat bernama per sertifikat. Hasil `TlsCa` struct (jalur, `Arc<Vec<Vec<u8>>>` DER) yang isi byte ikut `ConnectionConfig` dan hash-nya di kunci pool (tidak ada TOCTOU). Cites: blueprint §7.2 D-15.
5. **Prasyarat CA.** Hanya PostgreSQL dan Trino. Mode efektif harus `Require` (`sslmode=verify-ca`, `verify-full`; Trino `https` tanpa `DB_INSECURE`). Kombinasi dengan `Prefer`, `Disable`, `RequireNoVerify`, atau `DB_INSECURE=1` ditolak dengan nama sebelum koneksi (tidak ada kenaikan diam-diam). MySQL ditolak dengan alasan di PRD §12.1 (driver tidak menyediakan `RootCertStore`). Cites: blueprint §7.2.
6. **Verifikasi SAN nama host.** PostgreSQL: `tls::verifier_with_roots(RootCertStore)` yang sudah ada, dipakai untuk CA uji. Trino: `ClientBuilder::with_root_certificates(cert)`, atau dengan `Policy::none()` bila bearer (§7.1 (b)). Host sebagai IP memerlukan sertifikat dengan SAN IP. Cites: blueprint §7.2.
7. **TLS server name lewat tunnel.** `ConnectionConfig.tls_server_name: Option<String>` diisi `retarget` saat tunnel dibuat (host database asli, bukan `127.0.0.1`). PostgreSQL: `pg.host(<nama>).hostaddr(127.0.0.1)`. Trino: URL dasar `https://<nama>:<port_lokal>` dengan `ClientBuilder::resolve(<nama>, 127.0.0.1)`. MySQL lewat tunnel dengan `Require` ditolak dengan `Usage` ("MySQL cannot verify…; use require (encrypted, not verified) or connect directly"). Cites: blueprint §7.3 D-16.
8. **Setelan baru** (D-9 blueprint): `DB_JWT`, `DB_CA_FILE`. `Settings` mendapat `Debug` tersensor untuk `DB_JWT` di samping `DB_PASSWORD` dll. Cites: blueprint §4.
9. **Keychain:** `jwt:<UUID>` di samping `ssh-password:<UUID>`, `ssh-passphrase:<UUID>`. Tidak pernah ke `connections.json`, log, atau `Debug` apa pun. Cites: blueprint §8.
10. **Tes di `crates/qh-driver-trino`:** `credential_origin.rs` (baru, 5 kasus dengan koordinator palsu), menguji bahwa tidak ada permintaan ke origin asing yang membawa header `Authorization`, tidak ada redirect yang diikuti untuk bearer, bentuk JWT yang salah ditolak, kombinasi mode yang salah ditolak, CA uji diterima dan CA lain ditolak, dan sertifikat tanpa SAN IP ditolak untuk host IP. Cites: blueprint §7.1 (c), §7.2 (tes TLS).

## Alasan

1. **Bearer opak menutup kebutuhannya tanpa perlu decode atau refresh di app.** Kedaluwarsa adalah tanggung jawab server (401 response), yang sudah ada pesan "token may have expired".
2. **Pemeriksaan origin per permintaan menghentikan bocor bearer ke host atau skema lain** di tiga pemanggil Trino (POST basis, GET dan DELETE dari server). HTTPS-only mode di reqwest tidak melihat skema (hanya host/port untuk redirect dan 3xx), jadi pemeriksaan itu perlu.
3. **Klien bearer tanpa redirect** mencegah redirect TLS → HTTP pada host/port yang sama (yang reqwest izinkan, karena header `Authorization` hanya dibuang saat host atau port berubah).
4. **CA sekali-baca dengan hash di kunci pool** menghindari TOCTOU antara validasi dan connect, dan membuat isi berkas bagian dari identitas pool (dua koneksi dengan CA berbeda tidak berbagi jalur).
5. **TLS server name asli dipertahankan lewat tunnel** agar UC-01 dan UC-18 bisa digabung: sertifikat bernama real database, bukan `127.0.0.1`.

## Konsekuensi

### Positif

- **POS-001.** Trino bisa dihubungi dengan JWT, standar bearer untuk platform cloud dan SaaS.
- **POS-002.** PostgreSQL dan Trino bisa memakai CA internal tanpa trust store sistem (FR-CON-08).
- **POS-003.** Koneksi lewat bastion dengan sertifikat bernama tidak perlu `RequireNoVerify` (UC-01 dan UC-18 terpenuhi bersama).
- **POS-004.** Bearer tidak bocor ke host atau skema lain, dan tidak ada redirect otomatis yang membawa token.

### Negatif

- **NEG-001.** Token JWT opak: app tidak tahu kedaluwarsa sampai server menolak (401). Yang lama `BasicAuth` masih perlu pemeriksaan origin di W11 hanya untuk Trino; Basic lewat HTTP lain masih ada risiko (keputusan pemilik, O-22, keputusan di luar ADR).
- **NEG-002.** Berkas CA dibaca sekali saat `config::build`, bukan saat connect: perubahan berkas di disk tidak terdeteksi sampai koneksi baru. Itu disengaja (no TOCTOU), tetapi berarti restart butuh untuk CA yang berputar dengan cepat.
- **NEG-003.** MySQL tidak bisa memakai CA per koneksi (`mysql_async 0.36` tidak mengekspor RootCertStore). App dan form bisa menyembunyikan field untuk MySQL, tetapi engine tetap menolak setelan itu dengan nama.
- **NEG-004.** TLS server name lewat tunnel belum dicoba live dengan proxy nyata yang mengubah header `Host:` atau forwarded. Koordinator di balik load balancer bisa melihat `Host: real-db:443` tanpa port proxy (Trino URL). Dampak di koordinator tidak diuji.

### Belum ada (kontrak, bukan kode)

- Kode JWT, origin check, CA validation di driver Trino (W11-T2b1d, belum directed).
- CA validation di driver PostgreSQL, MySQL rejection (bagian dari W11-T2b1, belum terjadi).
- TLS server name di PostgreSQL, Trino, MySQL (bagian dari W11-T2b, masih dalam planning: beberapa file belum di daftar T2b1, §13.1 blueprint).
- Form app dan Keychain `jwt:<UUID>` (W11-T3).
- Tes integrasi CA uji dengan PostgreSQL dan Trino live (W11-T2, gate TLS).

## Bukti

- Blueprint: `docs/architecture/blueprints/w11-metadata-and-connections.md` keputusan D-14, D-15, D-16, §7 (JWT, CA, TLS server name), §23 (daftar ADR, "0040 JWT Trino dan CA per koneksi, blueprint w11 §7, keputusan pemilik O-22").
- Tes (belum dijalankan): blueprint §7.1 (c) dan §7.2 menjelaskan struktur tes dengan koordinator palsu dan sertifikat uji.
- Verdict architect-reviewer: 6 Okt 2026, satu temuan blocking (JWT dan origin kredensial, D-14) diperbaiki dan dicatat §7.1 (b); temuan non-blocking yang juga diperbaiki mencakup "IdentitiesOnly yes" dan "PreferredAuthentications" di SSH config (bukan bagian ADR ini).
- Tanggal keputusan pemilik O-22 (Basic JWT dan CA) belum tercatat di PRD atau development-plan, dijumpai di blueprint §4.

## Referensi

- ADR-0038 (metadata), ADR-0039 (SSH TOFU), ADR-0031 (tunnel).
- `docs/architecture/blueprints/w11-metadata-and-connections.md` keputusan D-14, D-15, D-16, §7 (JWT, CA, TLS server name), §13.3 (W11-T2b blueprint), §23 (daftar ADR).
- `docs/architecture/prd-performance-and-parity.md` P-08, FR-CON-07 (JWT HTTPS), FR-CON-08 (CA per koneksi), UC-01, UC-18.
- Tugas penerus: W11-T2b1d (JWT Trino), W11-T2b1 (CA validation, TLS server name), W11-T3 (form app, Keychain), dan tes live gate TLS.
- Catatan: blueprint disetujui architect-reviewer 6 Okt 2026; subtask W11-T2b yang menjalankan kode belum dimulai dan belum ada commit yang menunjukkan mana dari D-14/D-15 prioritas utama atau dikerjakan bersamaan.
