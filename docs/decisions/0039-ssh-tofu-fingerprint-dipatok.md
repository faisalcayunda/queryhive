# 0039 — SSH di app, TOFU dengan fingerprint dipatok dan berkas `known_hosts` milik app

- **Status:** Diterima. Implementasi trust core mendarat di commit `487a18b` (W11-T2a); tunnel half mendarat di `401396c` (W11-T2b-tun). Tinjauan keamanan putaran pertama meminta lima perubahan (Q1 sampai Q5 §5.10), semuanya diterapkan: aturan kunci berbeda lebih ketat, `ca_covered` ditolak keras, catat sebelum auth, berkas aman lewat fstat, dan CLI pin dipertahankan. Putaran kedua APPROVE. Tes live SSH 74 kasus unit dan 17 sshd; tes murni 500+. Semua kunci tidak pernah dilewatkan ke app atau FFI.
- **Tanggal:** 6 Okt 2026 (W11-D, perf-parity Fase 11)
- **Konteks instruksi:** `docs/architecture/development-plan.md` §4 (jadwal ADR, baris 0039) dan W11-D; `docs/architecture/blueprints/w11-metadata-and-connections.md` §5 (TOFU), §6 (alias `~/.ssh/config`), dan §23; PRD `docs/architecture/prd-performance-and-parity.md` P-09, UC-01, UC-18, NFR-S4, FR-CON-02; `docs/invariants.md` §11.
- **Tidak menggantikan:** ADR lain. Pendamping: ADR-0031 (tunnel di engine), ADR-0037 (enkripsi spill menjadi pola: satu kunci efemeral per proses, tidak menyeberang proses), keputusan pemilik O-26 (SSH di app).

## Konteks

Tunnel SSH sudah ada di engine sejak W3-T1, dengan verifikasi key yang sempurna namun jalur TOFU (kepercayaan on-first-use) yang belum ada. App hanya punya form SSH tanpa UI untuk menerima atau mengelola kunci host. Pengguna produksi di belakang bastion tidak bisa terhubung tanpa mengedit berkas `known_hosts` manual atau menolak verifikasi host key, dan tidak ada cara mengetahui fingerprint kunci host lain pada run selanjutnya. FR-CON-02 dan P-09 menuntut TOFU yang dipatok ke fingerprint publik dan tercatat di berkas app yang aman, dengan penolakan keras untuk kunci yang berubah.

Rangkaian temuan di blueprint menunjukkan:
- Engine hanya menawarkan `HostKeyPolicy::Strict` dan `TrustNew` (pola perpustakaan untuk dua langkah); tidak ada jalur penerimaan.
- `russh 0.63.3` punya jalur kex `none` yang melewati verifikasi kunci.
- Urutan algoritma kunci di daftar bawaan tidak cocok dengan yang tercatat, sehingga perubahan jenis kunci dari RSA ke Ed25519 menjadi alarm palsu `Mismatch`.
- Aturan OpenSSH atas jenis kunci berbeda (`HOST_NEW` di OpenSSH, Mismatch di sini) memerlukan keputusan eksplisit untuk security reviewer.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Penyimpanan kunci host | **Berkas milik app dengan mode `0600`, dibaca dan ditulis oleh engine, bukan dipilih user** | Jalur trust store app terpisah dari `~/.ssh/known_hosts`; pengguna tidak perlu bercerita tentang direktori; engine menulisnya hanya saat penerimaan explicit | Satu berkas per app; tidak ada multitenancy manual per pengguna OS |
| | `~/.ssh/known_hosts` saja | Standard OpenSSH | Engine harus menulis ke direktori pengguna; tidak ada jalur terpisah untuk impor cli atau cross-trust |
| Protokol penerimaan | **TOFU (Trust-On-First-Use) dipatok ke fingerprint SHA256 yang disajikan, dicatat sebelum autentikasi** | Penerimaan hanya satu kali per host; catat terlebih dahulu = tanpa autentikasi ke kunci yang belum tercatat | Pengguna harus membandingkan fingerprint dengan sumber luar jalur (§5.1 out-of-band verification) |
| | TOFU dengan verifikasi otomatis | Tidak perlu verifikasi manual | Penyerang di jaringan koneksi pertama memenangkan penerimaan |
| | Alternatif password/sandi | Opsional | Masih perlu TOFU untuk agen yang tidak diketahui |
| Reject ketat saat kunci berubah | **Jenis kunci berbeda (mis. RSA ke Ed25519) adalah `Mismatch` keras, bukan `HOST_NEW`** | Lebih konservatif; server yang berhenti menawarkan jenis tercatat ditangani sama | Urutan algoritma SSH harus menjaga jenis tercatat di depan daftar |
| | Kompatibel OpenSSH (kunci baru = `HOST_NEW`) | Sesuai standar de facto | Alarm palsu bila server punya dua kunci dan yang satu saja tercatat |
| Keychain dan Form | **Tiga item Keychain baru** (ssh-password, ssh-passphrase, jwt) di samping password database; form menampilkan picker SSH, validasi bernama, tab autocomplete dari ssh_config | Field rata, lebih mudah disimpan; SSH_AUTH_METHOD eksplisit per run | Gerbang setelan lima `SSH_*` baru; perubahan `AppModel.connectionEnvironment` |
| | Semua di `connections.json` terenkripsi | Satu file | Keychain buruk untuk passphrase; `connections.json` bukan enkripsi end-to-end |

## Keputusan

**SSH trust di engine lewat TOFU yang dipatok fingerprint dan tercatat di berkas app milik engine dengan mode `0600`. Aturan kunci berbeda adalah penolakan keras (Q1). `ca_covered` adalah penolakan keras tanpa TOFU (Q2). Catat sebelum autentikasi (Q3). Berkas aman lewat `O_NOFOLLOW` dan `fstat` (Q4). CLI dan MCP pin dipertahankan, walau pin dari ssh-keyscan mengalahkan tujuannya (Q5). App membungkus engine dengan `HostKeyGate` dan menampilkan sheet prompt untuk `unknown`, serta sheet hanya-info untuk `changed`, `revoked`, `certificate`, `certificate_expected`.**

Keputusan desain yang mengikat, setiap klaim "sudah dibangun" dikutip dari commit `487a18b` dan `401396c`:

1. **Berkas `known_hosts` milik app.** Setelan `SSH_APP_KNOWN_HOSTS` (bawaan kosong, disisi app diisi ke `~/Library/Application Support/QueryHive/known_hosts`). Format OpenSSH, satu host:port per baris, tidak berhash. Identitas catatan adalah `HostName` hasil resolusi alias, huruf kecil, `[host]:port` untuk port bukan 22. Komentar akhir `# accepted by QueryHive <ISO-8601>`. Cites: `crates/qh-tunnel/src/known_hosts.rs:111-117` (host spelling), `:223-266` (append), `:319` (komentar diabaikan saat parse).
2. **Pemeriksaan keadaan.** `check_all` membaca berkas pengguna (`~/.ssh/known_hosts` atau `SSH_KNOWN_HOSTS`), sistem (`/etc/ssh/ssh_known_hosts`, hanya baca, tidak wajib ada), dan app (wajib bila TOFU). Keadaan: `Matched` (byte kunci sama di berkas mana pun), `Mismatch` (ada catatan untuk host itu tetapi byte berbeda), `Revoked` (baris `@revoked` cocok), `Unknown` (tidak ada catatan), atau `HostKeyCertificateExpected` (Q2: `@cert-authority` cocok dan kunci yang disajikan bukan sertifikat). Cites: `crates/qh-tunnel/src/known_hosts.rs:191-211` (check_text).
3. **Penolakan keras untuk perubahan.** Kunci yang disajikan dengan jenis berbeda dari yang tercatat (mis. catatan Ed25519 tetapi server menawarkan RSA) adalah `Mismatch` keras, tidak ada TOFU alternatif. Penyerang di jaringan atau rotasi jenis yang tidak terencana keduanya ditolak (Q1). Cites: `crates/qh-tunnel/src/tunnel.rs:369-380` (check_server_key menolak `Mismatch`), `:350-359` (hanya `Unknown` yang bisa diubah oleh pin).
4. **Urutan algoritma kunci mengikuti yang tercatat.** `recorded_key_types` di `known_hosts.rs:350-406` membaca jenis kunci yang tercatat untuk host itu, mengabaikan `@revoked` dan `@cert-authority`, menyertakan entri berhash. Jenis tercatat ditaruh di depan daftar `russh::Config.preferred.key` sebelum connect. Tanpa ini, server dengan dua kunci (dua jenis) di mana satu saja tercatat menghasilkan algoritma yang salah negosiasi. Cites: `crates/qh-tunnel/src/tunnel.rs:229-242` (preferred di Config.kex_init), `:356-358` (recorded_key_types dipanggil).
5. **`kex none` dilarang.** `russh 0.63.3` punya jalur kex tanpa kunci (`none`), yang melewati handler verifikasi. `Handler::kex_done` menolak `kex::NONE`, dan `Config.preferred` tidak memuatnya (tidak ada `none` di daftar key yang dipilih ulang). Cites: `crates/qh-tunnel/src/tunnel.rs:265-273` (kex_done), `:230-231` (preferred tanpa `none`).
6. **Penjaga "sudah diverifikasi".** Handler mencatat ke `Arc<AtomicBool>` saat mencapai `Ok(true)` (Matched atau penerimaan tercatat). `Tunnel::open` menolak memanggil `authenticate` bila bendera belum menyala. Alasannya: penjagaan ganda terhadap mekanisme `russh` yang bisa dilewati. Cites: `crates/qh-tunnel/src/tunnel.rs:276-283` (check), `:393-398` (open panggilan check).
7. **Catat sebelum autentikasi.** `append_if_absent` di langkah penerimaan pin sebelum `authenticate` dipanggil (Q3). Hasil: kunci tercatat walau autentikasi gagal (ulang password yang salah). Umat: pesan di sheet mengatakan "host key trusted, but the connection failed: …". Cites: `crates/qh-tunnel/src/tunnel.rs:347-355` (append_if_absent di check_server_key).
8. **Berkas aman, tanpa TOCTOU (Q4).** `O_NOFOLLOW` di open (symlink di komponen terakhir gagal). `fstat` pada deskriptor yang terbuka (bukan `lstat` pada jalur): pemilik `== geteuid()`, mode `& 0o022 == 0`. Pemeriksaan yang sama berlaku saat dibaca di `check_all` (bukan hanya saat ditambah). Satu fungsi `open_app_store` untuk kedua jalur. `append_if_absent` membaca, memeriksa idempoten (kunci sudah ada?), dan menambah pada **satu deskriptor** (tanpa pembukaan kedua di antara pemeriksaan dan penulisan). Cites: `crates/qh-tunnel/src/known_hosts.rs:464-485` (open_app_store dengan pemeriksaan), `:223-266` (append_if_absent), `:291-320` (check_all memanggil open_app_store).
9. **Pin dipertahankan di CLI dan MCP (Q5).** `SSH_HOST_KEY_ACCEPT=SHA256:…` satu run, hanya untuk keadaan `unknown` tanpa `ca_covered`. Bentuk pin yang salah adalah `Usage` sebelum jaringan. Nilai hanya bisa datang dari setelan satu-run, tidak pernah disimpan `connections.json` atau Keychain. Keputusan untuk ADR-0039 (bukan 0040): Q5 mengatakan "pertahankan pin CLI" dan "menghitung pin dari ssh-keyscan mengalahkan tujuannya" — keduanya dicatat di sini. Cites: `crates/qh-ffi/src/tunnel.rs:118-136` (settings), `:350-359` (tunnel.rs check_server_key).
10. **Alias `~/.ssh/config`.** Parser `ssh_config.rs` (baru) mendukung `HostName`, `User`, `Port`, `IdentityFile`, `Include`. Menolak `ProxyJump`, `Match`, `HostKeyAlias`, `UserKnownHostsFile`, `StrictHostKeyChecking` dengan nilai lemah, dan setiap direktif tak dikenal **dengan nama** (D-12 blueprint). Cites: `crates/qh-tunnel/src/ssh_config.rs:1-100` (parser), `:500-530` (penolakan).
11. **Galat terstruktur di app.** `qh-tunnel` mengirim varian galat baru: `HostKeyPinMismatch`, `HostKeyRecordFailed`, `HostKeyNotVerified`, `HostKeyStoreUnsafe`, `HostKeyCertificateExpected`. Dipetakan ke `EngineError::HostKey` yang membawa struct `HostKeyFailure` (data polos, string). FFI mendapat D-13 (§5.8 blueprint): objek `host_key` di event `error` hanya bila `SSH_HOST_KEY_DETAIL=1`. Cites: `crates/qh-tunnel/src/error.rs:1-100` (varian galat), `crates/qh-ffi/src/tunnel.rs:207-221` (pemetaan ke EngineError).
12. **Setelan baru** (D-9 blueprint): `SSH_USE_CONFIG`, `SSH_CONFIG_PATH`, `SSH_APP_KNOWN_HOSTS`, `SSH_HOST_KEY_ACCEPT`, `SSH_HOST_KEY_DETAIL`. Cites: `crates/qh-ffi/src/env.rs` definisi, `crates/qh-ffi/src/tunnel.rs:11-31` dokuumentasi.
13. **Retry policy.** Galat host-key semua varian adalah `FailureKind::Permanent`: tidak diulang otomatis. Cites: `crates/qh-core/src/error.rs:108-118` (failure_kind cocok tiap varian tanpa wildcard).

## Alasan

1. **TOFU yang dipatok fingerprint adalah satu-satunya protokol yang tidak menerima kunci dari penyerang di jaringan koneksi pertama dan tetap tidak mau dilekatkan key lama yang berubah.** OpenSSH `StrictHostKeyChecking yes` sama (tolak asing, tolak perubahan). Perbedaan dengan `accept-new` OpenSSH (Q1): lebih ketat di sini (jenis berbeda = `Mismatch`, bukan `HOST_NEW`), dengan dukungan urutan algoritma untuk menghindari alarm palsu.
2. **Berkas milik app dengan mode `0600` membuat trust store app terpisah dari pengguna**, sehingga tidak perlu edit manual, dan engine bisa menulis saat penerimaan explicit.
3. **Catat sebelum autentikasi** menghilangkan jalur "catat gagal, ulang password, tidak ada catatan pada percobaan kedua", yang membuat pengguna melihat prompt ulang.
4. **Urutan algoritma dari catatan menghindari alarm palsu** bila server punya dua kunci (yang umum di rotasi kunci).
5. **`O_NOFOLLOW` dan `fstat` tanpa TOCTOU** adalah pertahanan berlapis terhadap symlink dan pengguna lain yang menulis berkas.

## Konsekuensi

### Positif

- **POS-001.** Koneksi pertama lewat bastion memunculkan prompt fingerprint di app, bukan kegagalan diam atau penolakan brute.
- **POS-002.** Kunci yang berubah ditolak keras tanpa TOFU alternatif, melindungi dari MITM di jaringan yang stabil.
- **POS-003.** Alias `~/.ssh/config` (HostName, User, Port, IdentityFile) tersedia untuk pengguna yang sudah punya file itu.
- **POS-004.** Shell script dan MCP bisa pin host key di setelan satu-run tanpa menyimpan di `connections.json`.
- **POS-005.** Aturan kunci berbeda lebih ketat daripada OpenSSH mencegah alarm palsu tanpa mengorbankan ketat (hanya dengan dukungan urutan algoritma).

### Negatif

- **NEG-001.** Berkas app yang dapat ditulis grup atau pihak lain tetap membaca sebagai `Unknown` walaupun lolos `check_all`, di luar jalur yang sering diuji.
- **NEG-002.** Tidak ada tombol "Forget host key" di app (D-24 blueprint): pengguna harus menjalankan `ssh-keygen -R` manual. Itu disengaja untuk menghindari satu klik yang mengalahkan ketat.
- **NEG-003.** Satu catatan per nama (nama dan IP = dua prompt masing-masing). Pengguna yang menghubungi host melalui IP terlebih dahulu lalu nama akan diminta dua kali.
- **NEG-004.** `russh 0.63.3` hanya memeriksa host key pada kex awal, bukan saat rekey. Penjaga ditambahkan di sini (catat di verifier); upstream bisa mengekspos ulang. (Dikatakan risiko tersisa di §5.10 blueprint: rekey Q-risiko rendah.)
- **NEG-005.** TOFU pertama tanpa verifikasi fingerprint by definition. Batas TOFU adalah "pengguna membandingkan dengan sumber luar jalur" (§5.1 blueprint).

### Belum ada (kontrak, bukan kode)

- Form SSH di app (W11-T3): sheet modal dengan picker SSH, validator, tiga item Keychain baru, dan `HostKeyGate` sebagai pembungkus `Engine.current`.
- Tool SSH_config_hosts dan ssh_config_resolve di FFI (W11-T2, eksport uniffi baru; lane FFI).

## Bukti

- Kode: `crates/qh-tunnel/src/known_hosts.rs` (check_all, append, safety), `crates/qh-tunnel/src/tunnel.rs` (handler, open, keadaan), `crates/qh-tunnel/src/ssh_config.rs` (parser), `crates/qh-tunnel/src/error.rs` (varian galat), `crates/qh-ffi/src/tunnel.rs` (pemetaan EngineError).
- Commit: `487a18b` (W11-T2a, trust core), `401396c` (W11-T2b-tun, tunnel dengan timeout).
- Tes: 74 unit (crates/qh-tunnel/tests/; murni known_hosts, ssh_config, tunnel.rs tanpa jaringan), 17 sshd live (QH_TEST_SSH, `crates/qh-tunnel/tests/sshd.rs` dengan qh-sshd-dev), tes Rust dan Swift paritas.
- Gate yang tercatat: G-RUST (fmt, clippy, test), G-DENY, G-LIVE (SSH).
- Blueprint: `docs/architecture/blueprints/w11-metadata-and-connections.md` keputusan D-10, D-11, D-12, §5 (TOFU), §6 (alias).

## Referensi

- ADR-0031 (tunnel di engine), ADR-0037 (pola kunci efemeral per proses).
- `docs/architecture/blueprints/w11-metadata-and-connections.md` keputusan D-10…D-12, §5 (TOFU, pertanyaan SEC Q1…Q5 dan risiko tersisa), §6 (alias).
- `docs/architecture/prd-performance-and-parity.md` P-09, UC-01, UC-18, NFR-S4, FR-CON-02.
- Tes keamanan: putaran pertama lima temuan blocking (Q1…Q5 diperbaiki), putaran kedua APPROVE per `target/run/ledger.md` W11-T2.
- Risiko tersisa: rekey tidak dimonitor ulang (Q-risiko rendah), ssh-keyscan pin mengalahkan tujuan (Q5 dicatat ADR ini), tidak ada penguncian antar proses di berkas app, tidak ada `HashKnownHosts` untuk privasi nama host di berkas app.
- Tugas penerus: W11-T3 (form app, `HostKeyGate`, sheet), W11-T2 (ekspor ssh_config di FFI).
