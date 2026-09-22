# Kompatibilitas versi — apa yang diuji, apa yang dijanjikan

> Isi dokumen ini berasal dari dua hal saja, dan tidak ada yang lain: **kebijakan versi
> hulu** yang dibaca dari situs resmi masing-masing proyek, dan **versi yang benar-benar
> dijalankan** di container dev. Tanggal akses seluruh sumber: **2026-09-22**.
>
> Angka versi di sini tidak ada yang dikutip dari ingatan. Kalau ada baris yang belum
> punya sumber, ia ditandai **belum**, bukan diisi dengan perkiraan — §5 poin 9 blueprint
> melarang mengarang sumber.

## Ringkasan

| Mesin | Teruji di sini | Lantai terukur | Kebijakan hulu | Sumber |
|---|---|---|---|---|
| PostgreSQL | **17.11** | **9.5.25** — suite lulus penuh; 9.4.26 gagal satu uji karena `pg_stat_ssl` | Rilis mayor ~1×/tahun, tiap mayor didukung **5 tahun** | [versioning policy](https://www.postgresql.org/support/versioning/) |
| MySQL | **8.4.11** (Community) | **5.7.44** untuk SQL; 5.7 gagal di satu uji TLS | Dua jalur: **LTS** dan **Innovation** | [MySQL Releases](https://docs.oracle.com/cd/E17952_01/mysql-8.4-en/mysql-releases.html) |
| Trino | **483** (uji integrasi driver) | **belum** — hanya 483 yang pernah dijalankan | Rilis mingguan, tanpa semver, **tanpa jaminan antarversi** | [diskusi #20032](https://github.com/trinodb/trino/discussions/20032), [release notes](https://trino.io/docs/current/release.html) |

"Teruji di sini" berarti seluruh uji integrasi dijalankan terhadapnya. "Lantai terukur"
adalah angka terendah yang benar-benar dijalankan dan lulus di mesin ini; §"Versi minimum per
mesin" memuat tabel per versi beserta kegagalannya. Keduanya **bukan** klaim tentang versi
lain: versi lain tidak ditolak, tetapi juga tidak dijanjikan.

## PostgreSQL

Kebijakannya, dari halaman resminya:

- Rilis mayor baru sekitar sekali setahun; perbaikan bug dan keamanan keluar minimal tiap
  tiga bulan sebagai rilis minor.
- Setiap mayor didukung **5 tahun** sejak rilis awal, lalu satu rilis minor terakhir dan
  berstatus *end-of-life*.
- Sejak PostgreSQL 10, mayor ditandai oleh angka pertama (10 → 11); minor oleh angka
  berikutnya.
- Naik mayor tidak bisa di tempat: perlu dump/reload atau `pg_upgrade`.

| Versi | Minor terkini | Didukung | Rilis pertama | Berakhir |
|---|---|---|---|---|
| 18 | 18.6 | Ya | 2025-09-25 | 2030-11-14 |
| 17 | 17.11 | Ya | 2024-09-26 | 2029-11-08 |
| 16 | 16.15 | Ya | 2023-09-14 | 2028-11-09 |
| 15 | 15.19 | Ya | 2022-10-13 | 2027-11-11 |
| 14 | 14.24 | Ya | 2021-09-30 | **2026-11-12** |
| 13 | 13.23 | Tidak | 2020-09-24 | 2025-11-13 |

Yang perlu diperhatikan: **PostgreSQL 14 berakhir sekitar tujuh minggu setelah dokumen ini
ditulis**, dan 15 menyusul pada 2027. Menjadikan 15 sebagai lantai yang didukung adalah
saran yang masuk akal berdasarkan tabel di atas — dan itu **saran dari data ini**, bukan
kebijakan yang sudah pernah diukur.

Yang benar-benar dipakai driver: `information_schema.schemata`,
`information_schema.tables`, `pg_class`, `pg_namespace`, `pg_get_userbyid`,
`array_to_string`, `pg_database`, dan `pg_cancel_backend`.

## MySQL

Kebijakannya, dari dokumentasi rilis Oracle:

- Ada dua jalur, dan **keduanya dianggap kualitas produksi**: **LTS** dan **Innovation**.
- **LTS** mengikuti Oracle Lifetime Support Policy: **5 tahun premier support + 3 tahun
  extended support**. Di dalam satu seri LTS tidak ada penghapusan, fungsionalitas dan
  format data tidak berubah, sehingga naik **dan turun** versi di dalam seri itu bisa
  dilakukan di tempat. Fitur hanya bisa ditambah atau dihapus di rilis LTS pertama (mis.
  8.4.0), tidak di rilis LTS berikutnya.
- **Innovation** didukung hanya sampai rilis Innovation berikutnya.
- Konektor tetap kompatibel dengan semua versi server yang didukung: dokumentasinya
  menyebut Connector/Python 9.7.0 kompatibel dengan server 8.0, 8.4, dan 9.x.

Dua konsekuensi untuk proyek ini, keduanya diturunkan dari kebijakan di atas:

1. Menargetkan jalur **LTS** lebih dapat dipertahankan daripada Innovation, karena
   jendela dukungannya 5+3 tahun dan naik/turun versi di dalam seri dijamin aman.
2. Server **8.0 masih didukung**, jadi ia tidak boleh ditolak hanya karena lebih tua dari
   8.4.

Yang benar-benar dipakai driver: `information_schema.SCHEMATA`,
`information_schema.TABLES`, `SHOW TABLES FROM`, dan `KILL QUERY`. Pembedaan `TEXT` dari
`BLOB` **tidak** memakai versi apa pun: ia membaca character set (63 = binary) dari
definisi kolom, karena kedua tipe itu dikirim sebagai tipe protokol yang sama.

## Trino

Di sinilah kebijakan hulunya berbeda jenis, dan perbedaannya penting:

- Rilis diusahakan **setiap Rabu**.
- **Tidak ada semantic versioning** — versinya naik satu-satu saja.
- **Setiap versi berpotensi memuat breaking change**, dan sejak rilis 432 hal itu ditandai
  di release notes. Apakah sesuatu benar-benar rusak bagi pemakaian tertentu sangat
  bergantung pada konektor dan seberapa jauh lompatan versinya.
- Maintainer-nya menyatakan terus terang: *"We can not make any guarantees from one version
  to another."*
- Sisi yang menenangkan: **JDBC driver dan client API** disebut sangat stabil dan kompatibel
  luas — dan justru itu yang dipakai driver Trino di proyek ini, bukan konektornya.
- Rilis terbaru saat dokumen ini ditulis: **483 (17 Juli 2026)**, sebelumnya 482 (25 Juni
  2026).

Akibat langsung untuk proyek ini: **tidak boleh ada klaim "bekerja dengan Trino"** secara
umum. Driver Trino harus diuji terhadap rilis tertentu yang disebutkan nomornya, dan angka
itu dicatat di sini saat pengujiannya benar-benar terjadi. Sampai itu, baris Trino di tabel
ringkasan tetap **belum pernah dijalankan**.

Versi yang berjalan di sini: **483**, dari `/v1/info` (`nodeVersion.version`), setelah VM
podman dinaikkan dari 2 GiB ke 4 GiB. Ia melayani sekitar **sepuluh detik** setelah
container dijalankan.

Yang sudah diverifikasi adalah **protokol kliennya**, bukan konektor atau SQL lengkap:

| Langkah | Hasil yang terlihat |
|---|---|
| `GET /v1/info` | `{"state":"ACTIVE","nodeVersion":{"version":"483"},"coordinator":true}` |
| `POST /v1/statement` (`SELECT 1 AS n, 'x' AS t`) | 200 dengan `id`, `infoUri`, `nextUri`, dan `stats.state = QUEUED` |
| `GET nextUri` (poll 1–2) | masih `QUEUED`, **`columns` belum ada**, `data` belum ada |
| `GET nextUri` (poll 3) | `RUNNING`, `columns` 2 buah, `data = [[1, "x"]]`, `nextUri` masih ada |
| `GET nextUri` (poll 4) | `FINISHED`, `data` kosong, **`nextUri` tidak ada** |

**Satu hal dari tabel itu yang wajib diingat saat driver-nya ditulis:** `columns` **tidak**
ada selama state masih `QUEUED`. Skema tidak boleh diasumsikan datang di respons pertama
`POST`, karena untuk query yang tidak langsung dieksekusi ia memang belum tersedia. Ini
kelas masalah yang sama dengan yang baru saja ditemukan di driver MySQL — di sana
`execute` menunggu metadata yang baru datang di akhir, dan akibatnya cancel tidak pernah
bisa dijangkau. Di sini bentuknya berbeda tetapi jebakannya sejenis, dan sudah diketahui
sebelum satu baris pun ditulis.

Keempat header yang dipakai proyek ini: `X-Trino-User`, `X-Trino-Catalog`,
`X-Trino-Schema`, dan `Content-Type: text/plain` pada `POST`.

## Versi minimum per mesin

Bagian ini menggantikan klaim "belum diketahui" yang sebelumnya ada di sini. Isinya
**pengukuran**, bukan perkiraan: tiap baris adalah versi server yang benar-benar dijalankan
di mesin ini, dengan suite integrasi yang sudah ada, dan hasil apa adanya.

Caranya sama untuk ketiga mesin, dan skripnya ikut di repo sehingga bisa diulang:

    deploy/dev/qh-pg-old.sh    <tag>     # qh-pg-old,    127.0.0.1:55434
    deploy/dev/qh-mysql-old.sh <tag>     # qh-mysql-old, 127.0.0.1:53308
    deploy/dev/qh-trino-old.sh <tag>     # qh-trino-old, 127.0.0.1:58082

Nama dan port itu sengaja berbeda dari container dev (`qh-postgres` 55432, `qh-mysql` 53306,
`qh-trino` 58080) supaya pengukuran tidak menyentuh lingkungan yang sedang dipakai. Tiap
skrip hanya menghapus container yang ia buat sendiri. Uji integrasi diarahkan ke server tua
lewat `QH_PG_PORT` / `QH_MYSQL_PORT` / `QH_TRINO_PORT`, variabel yang memang sudah dibaca
suite-nya.

### PostgreSQL — 14 uji integrasi

Perintah tiap baris: `deploy/dev/qh-pg-old.sh <tag>`, yang menjalankan
`QH_TEST_POSTGRES=1 QH_PG_PORT=55434 cargo test -p qh-driver-postgres --test integration`.

Suite ini berisi **14 uji** saat pengukuran terakhir dilakukan (HEAD `9b24e9a`). Jumlahnya
bertambah dari 13 di tengah sesi ini karena uji TLS sedang ditambahkan ke driver, dan satu
uji baru itulah yang menemukan lantai di bawah — jadi seluruh baris diukur ulang dengan suite
yang berisi 14 uji, bukan dicampur antara dua versi suite.

| Versi server | Suite | Catatan |
|---|---|---|
| 17.11 (`qh-postgres` dev) | **14/14 lulus** | versi yang dipakai sehari-hari |
| 15.19 | **14/14 lulus** | |
| 14.24 | **14/14 lulus** | |
| 13.23 | **14/14 lulus** | |
| 12.22 | **14/14 lulus** | |
| 11.22 | **14/14 lulus** | |
| 10.23 | **14/14 lulus** | |
| 9.6.24 | **14/14 lulus** | |
| 9.5.25 | **14/14 lulus** | |
| 9.4.26 | **13/14 lulus** | satu uji gagal; rilis terakhir seri 9.4, EOL sejak Februari 2020 |

Satu-satunya kegagalan yang ditemukan, persis apa adanya:

    ---- prefer_falls_back_to_plaintext_against_a_server_that_does_not_offer_tls ----
    panicked at crates/qh-driver-postgres/tests/integration.rs:539:10:
    execute: Query { message: "relation \"pg_stat_ssl\" does not exist:
      SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
      code: Some("42P01"), position: None, kind: Permanent }

**Yang gagal bukan drivernya.** Pernyataan yang gagal itu ada di berkas uji, bukan di kode
driver: `grep -rn pg_stat_ssl crates/*/src/` tidak menemukan apa pun, sedangkan di
`crates/qh-driver-postgres/tests/` ia dipakai di tiga tempat. Uji itu memanggil
`session.execute()` lebih dulu dan **berhasil terhubung** dengan `TlsMode::Prefer`; yang
gagal adalah kueri *verifikasi*-nya, yang bertanya ke server sendiri apakah koneksinya
terenkripsi. Jadi pemisahannya jelas:

- **Lantai driver sendiri: tidak ada yang ditemukan sampai 9.4.26.** Seluruh uji yang
  menguji SQL milik driver lulus di 9.4.26 — `numeric(38,10)` tanpa pembulatan,
  `timestamptz` beserta zona sesi, `bytea` dengan NUL dan byte non-UTF-8, streaming 1.000
  baris dalam empat batch, `pg_cancel_backend`, dan penelusuran metadata lewat
  `information_schema` + `pg_class`/`pg_namespace`.
- **Lantai suite uji: 9.5.** Bukan dari ingatan: batas itu dikonfirmasi dengan bertanya
  langsung ke kedua server. Di 9.4.26, `SELECT count(*) FROM pg_stat_ssl` menjawab
  `ERROR: relation "pg_stat_ssl" does not exist`; di 9.5.25 kueri yang sama menjawab `1` dan
  `SELECT to_regclass('pg_stat_ssl')` menjawab `pg_stat_ssl`. View itu ada sejak 9.5, dan
  kueri verifikasi di suite ini memakainya.

Di bawah 9.4 **belum diuji**, dan tidak ada gambar container yang dicoba.

Karena itu angka minimum yang jujur untuk PostgreSQL ada dua, tergantung pertanyaannya.
Secara teknis driver ini belum menemukan batas sampai 9.4.26. Secara operasional, yang bisa
dipertahankan adalah versi yang masih didukung hulu: **14** (berakhir 2026-11-12) sebagai
minimum, atau **15** bila ingin jendela yang lebih panjang. Yang **tidak** benar adalah
menulis "PostgreSQL 15+" seolah driver-nya menolak 14 — ia tidak menolak.

Satu catatan kecil tentang cara mengukur: `pg_isready` **tidak** cukup sebagai penanda siap
pada gambar 9.6. Entrypoint image itu menjalankan server sementara untuk `initdb`, dan
`pg_isready` menjawab "ready" terhadap server sementara itu sebelum database `POSTGRES_DB`
selesai dibuat. Versi pertama dari skrip ini karena itu melaporkan "database qh does not
exist" dan 12 uji gagal — kegagalan skrip pengukuran, bukan kegagalan driver. Skripnya
sekarang menunggu satu kueri sungguhan (`SELECT 1`) sebelum menyimpulkan apa pun.

### MySQL — 17 uji integrasi

Perintahnya: `deploy/dev/qh-mysql-old.sh <tag>` dengan
`QH_TEST_MYSQL=1 QH_MYSQL_PORT=53308 cargo test -p qh-driver-mysql --test integration`.
Suite ini juga berisi **17 uji** saat pengukuran terakhir, dan ketiga baris di bawah diukur
dengan suite yang sama.

| Versi server | Fixture | Suite | Catatan |
|---|---|---|---|
| 8.4.11 (`qh-mysql` dev) | `seed-mysql.sql`, ok | **17/17 lulus** | versi yang dipakai sehari-hari |
| 8.0.46 | `seed-mysql.sql`, ok | **17/17 lulus** | |
| 5.7.44 | `seed-mysql.sql` **GAGAL**, lihat di bawah | **16/17 lulus** | dijalankan lewat emulasi `linux/amd64`: tidak ada gambar arm64 untuk 5.7 |

Pada 5.7.44 fixture aslinya berhenti di baris ke-38:

    ERROR 1193 (HY000) at line 38: Unknown system variable 'cte_max_recursion_depth'

`seed-mysql.sql` mengisi `wide_500k` dengan `WITH RECURSIVE` dan menaikkan
`cte_max_recursion_depth`, keduanya fitur MySQL 8.0. Karena klien `mysql` berhenti pada error
pertama, **tidak ada satu pun tabel yang terbentuk** — termasuk `type_zoo` — sehingga uji-uji
pertama gagal dengan `Table 'qh.type_zoo' doesn't exist`, bukan karena driver menolak 5.7.
Itu kegagalan **fixture dev kami**, bukan lantai versi, dan setelah confound itu dihapus
(fixture pengganti tanpa CTE, 20.000 baris, memakai loop prosedur; lihat
`deploy/dev/qh-mysql-old-seed-prefixed.sql`) seluruh uji SQL milik driver lulus di 5.7.44:
`json`, `datetime(6)`, `time(6)`, `decimal(38,10)`, `blob` dengan NUL, `ENUM`, `KILL QUERY`
(kode 1317), dan `information_schema`.

Satu uji tetap gagal di 5.7.44, dan kegagalannya bukan tentang SQL:

    ---- a_certificate_nothing_trusts_is_refused_rather_than_downgraded ----
    panicked at crates/qh-driver-mysql/tests/integration.rs:669:9:
    Prefer failed, but not over the certificate: Connect { message:
      "mysql://qh@127.0.0.1:53308/qh: Input/output error: ... received fatal alert:
      HandshakeFailure", kind: Transient }

Artinya perlu dibaca dengan hati-hati, karena "gagal" di sini **bukan** downgrade yang lolos:

- Driver **tetap menolak** koneksi itu untuk `Prefer` maupun `Require` — tidak ada penurunan
  diam-diam ke plaintext.
- Yang berbeda hanya **alasannya**. Uji itu menuntut pesannya memuat "certificate", dan di
  5.7.44 yang terjadi adalah kegagalan handshake TLS sebelum sertifikat sempat diperiksa.
- Sebabnya terukur dari sisi server, bukan disimpulkan. Pada 5.7.44,
  `SHOW VARIABLES` menyebut `tls_version = TLSv1,TLSv1.1,TLSv1.2` dengan `ssl_cipher` kosong
  (daftar bawaan 5.7); pada 8.4.11, `tls_version = TLSv1.2,TLSv1.3`. Tumpukan TLS 5.7
  mendahului cipher suite modern yang dinegosiasikan backend rustls driver ini.

Jadi 5.7.44 adalah **16/17, dengan satu kegagalan di jalur TLS dan bukan di jalur SQL**.
Untuk pemakaian tanpa TLS ia lulus penuh. Untuk koneksi 5.7 yang memakai TLS, ini batas nyata
yang harus disebut, bukan disembunyikan di balik angka 17 uji yang lain.

Versi di bawah 5.7 **belum diuji**: `type_zoo` memakai `json` yang baru ada sejak 5.7.8, jadi
fixture itu tidak bisa dimuat apa adanya di 5.6 tanpa diubah lebih jauh.

### Trino — hanya 483 yang terukur

| Rilis | Suite | Catatan |
|---|---|---|
| 483 (`qh-trino` dev) | **18 lulus, 1 gagal** dari 19 uji | yang gagal uji TLS yang butuh coordinator TLS; bukan soal versi Trino — lihat di bawah |
| 400 | **hasil tidak sah** | container dev mati kehabisan memori di tengah run; angkanya tidak dipakai |

**Lantai Trino belum ditetapkan, dan itu dinyatakan sebagai belum, bukan diisi dengan
perkiraan.** Satu percobaan pada rilis 400 dijalankan, tetapi tidak menghasilkan angka yang
layak dikutip: VM podman di mesin ini 3,6 GiB tanpa swap, dan container dev `qh-trino`
(58080) sudah memakai sebagian besarnya. Coordinator kedua menghabiskan sisa memori dan
kernel membunuh container dev itu (`Exited (137)`) di tengah pengujian, sehingga kegagalan
yang terlihat di run 400 tidak bisa dipisahkan antara "rilis 400 berbeda" dan "servernya
sedang mati". Container dev dipulihkan segera setelahnya.

Pengukuran lain di sesi yang sama — menjalankan image MySQL 5.7 lewat emulasi `amd64`, yang
memang berat — memicu kejadian yang sama sekali lagi: `qh-trino` kembali mati dengan
`Exited (137)`. Penyebab yang sama pada keduanya bukan kebetulan, melainkan satu hal yang
bisa diperiksa: VM itu punya beberapa container milik pekerjaan berbeda yang hidup
bersamaan, `qh-trino` sendiri **tanpa batas memori** (`-XX:MaxRAMPercentage=80` dari seluruh
VM), sehingga dialah korban pertama OOM killer begitu total permintaan lewat kapasitas. Yang
kedua kali: kontainer penulis ini dihapus, dan `qh-trino` **tidak** dinyalakan ulang karena
saat itu sisa memori hanya ~500 MiB sementara satu coordinator butuh lebih dari itu —
menyalakannya akan membunuh container milik pekerjaan lain, jadi membiarkannya mati adalah
pilihan yang lebih kecil kerugiannya, dan itu dicatat di sini alih-alih dilakukan diam-diam.

Kesimpulan untuk siapa pun yang ingin menutup celah Trino: butuh **jendela eksklusif** di
mana `qh-trino` boleh dimatikan, bukan percobaan bersamaan. Ini batas lingkungan, bukan batas
Trino.

Yang benar-benar terukur dari percobaan itu, dan tetap berguna meski hasil suite-nya tidak
sah, adalah satu perbedaan protokol pada `/v1/info`:

| Rilis | Badan `/v1/info` |
|---|---|
| 400 | `{"nodeVersion":{"version":"400"},"environment":"docker","coordinator":true,"starting":false,"uptime":"1.70m"}` |
| 483 | `{"nodeId":"...","state":"ACTIVE","nodeVersion":{"version":"483"},"environment":"docker","coordinator":true,"coordinatorId":"pe766","starting":true,"uptime":"649.17ms"}` |

Field `state` **tidak ada** di 400 dan ada di 483. Ini contoh konkret dari "tidak ada jaminan
antarversi" di bagian Trino: klien yang menunggu `state == "ACTIVE"` untuk menyimpulkan
coordinator siap akan menunggu selamanya pada rilis yang lebih tua, walaupun servernya sudah
melayani `POST /v1/statement`. Driver di repo ini tidak membaca `/v1/info` sama sekali, jadi
ia tidak terpengaruh — tetapi siapa pun yang menambahkan pemeriksaan kesiapan lewat endpoint
itu harus tahu bahwa `state` bukan field yang bisa diandalkan lintas rilis.

Satu uji yang gagal di 483 juga perlu dicatat supaya tidak salah dibaca sebagai batas versi.
Pada pengukuran terakhir (HEAD `9b24e9a`), suite integrasi `qh-driver-trino` berisi 19 uji:
**18 lulus, 1 gagal** —

    ---- require_no_verify_reaches_the_tls_coordinator ----
    panicked at crates/qh-driver-trino/tests/integration.rs:62:33:

Uji itu mengharapkan coordinator yang melayani TLS, dan pada saat itu belum ada container
seperti itu. Ia bagian dari pekerjaan TLS yang sedang berjalan di cabang `crates/qh-driver-*`
(container `qh-trino-tls` menyusul setelahnya), bukan sesuatu tentang rilis Trino. Sebelumnya
di sesi yang sama uji TLS lain sempat panik di dalam rustls dengan
`Could not automatically determine the process-level CryptoProvider from Rustls crate
features`, yang juga milik lapisan TLS kami dan bukan milik Trino. Keduanya dilaporkan, bukan
ditambal di sini: crate itu bukan milik penulis bagian ini.

Hal yang sama berlaku untuk jumlah uji di tabel-tabel di atas. Suite `qh-driver-postgres`
tumbuh dari 13 uji menjadi 14 di tengah sesi ini, dan `qh-driver-mysql` dari uji tanpa TLS
menjadi 17 uji termasuk uji TLS. Karena itu seluruh baris diukur ulang dengan suite yang
sedang berlaku saat itu, bukan dicampur; angka di tabel adalah angka pada HEAD `9b24e9a`,
dan suite yang berubah setelahnya tidak otomatis mengubahnya, hanya membuatnya perlu diukur
ulang.

### Apa yang dipakai driver, menurut crate-nya sendiri

Lantai yang "didokumentasikan" berbeda dari lantai yang diukur, dan penting dipisahkan:

- **`tokio-postgres` 0.7.18** tidak menyebut lantai versi PostgreSQL sama sekali. README-nya
  tidak memuat satu pun nomor versi server (hanya nomor versi crate), dan `CHANGELOG.md`-nya
  hanya menetapkan lantai **Rust**, bukan lantai server:
  `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-postgres-0.7.18/CHANGELOG.md:22`
  — *"Upgraded to Rust edition 2024, minimum Rust version 1.85."*
  Jadi untuk PostgreSQL tidak ada klaim upstream yang bisa dikutip; yang ada hanya hasil
  pengukuran di atas.
- **`mysql_async` 0.36.2** menyebut satu versi server, tetapi sebagai **server uji crate itu
  sendiri**, bukan sebagai pernyataan kompatibilitas.
  `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/mysql_async-0.36.2/README.md:387`
  (bagian `## Testing`) menjalankan container uji dengan
  `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/mysql_async-0.36.2/README.md:403`
  — *"mysql:8.0 \"*. Itu satu-satunya nomor server di dokumentasi crate itu, dan `5.7` tidak
  pernah disebut. Pengukuran di atas karena itu **lebih rendah** daripada apa yang pernah
  diuji upstream secara terbuka.
- **Trino** tidak punya crate: kliennya ditulis tangan di repo ini (HTTP biasa lewat
  `reqwest`), jadi tidak ada dokumentasi pihak ketiga yang bisa dikutip. Satu-satunya
  pegangan tetap pernyataan maintainer-nya bahwa tidak ada jaminan antarversi.

## Yang belum ditetapkan

Bagian sebelumnya menutup pertanyaan "versi minimum per mesin" sejauh yang bisa diukur di
mesin ini. Yang **masih** belum:

- **PostgreSQL di bawah 9.4.** Tidak ada gambar container yang dicoba. Secara protokol
  kemungkinan besar masih jalan (klien ini memakai protokol 3.0), tetapi itu perkiraan dan
  bukan hasil pengukuran. Yang terukur adalah batas 9.4/9.5, dan yang menentukan batas itu
  adalah `pg_stat_ssl` di suite uji, bukan SQL driver.
- **MySQL 5.6 dan lebih tua.** Belum dijalankan; lihat alasan fixture di atas.
- **Trino selain 483.** Ini celah terbesar di dokumen ini. Menutupnya butuh coordinator
  kedua, dan mesin ini (3,6 GiB, tanpa swap) tidak dapat menjalankan dua sekaligus
  bersamaan dengan container dev yang sedang dipakai. Cara yang jujur menutupnya: satu
  jendela eksklusif di mana `qh-trino` boleh dimatikan, lalu jalankan
  `deploy/dev/qh-trino-old.sh <tag>` untuk beberapa rilis berurutan.
- **MariaDB.** Tidak diuji sama sekali — driver ini menyasar MySQL, dan MariaDB bukan
  sinonimnya walaupun protokolnya mirip.
- **Versi minor di dalam satu mayor.** Hanya satu minor per mayor yang dijalankan (yang
  tersedia di tag gambar saat itu); perilaku minor lain tidak diuji. Untuk PostgreSQL,
  minor yang diuji adalah yang tertinggi di setiap seri saat pengukuran.
- **Jalur TLS lintas versi.** Yang terukur hanya bahwa 5.7 gagal di handshake dan 8.0/8.4
  lulus. Versi 8.0 minor lain, dan varian TLS pada PostgreSQL 9.5–13, tidak dijalankan
  terpisah.

Untuk Trino, versi yang diuji adalah **483**, lewat uji integrasi `qh-driver-trino` yang
dijalankan terhadap container nyata. Karena Trino tidak memberi jaminan antarversi, setiap
klaim tentangnya harus menyebut nomor rilis, dan menguji pada rilis lain berarti menjalankan
ulang pengujiannya pada rilis itu — bukan mengasumsikan hasilnya berlaku.

Satu batas yang ditemukan saat pengujian itu dan tidak bisa diperbaiki di sisi kami:
**protokol Trino memotong `timestamp` dan `time` sampai milidetik.** Server melaporkan
`timestamp(6)` (dikonfirmasi lewat `typeof`) tetapi JSON membawa `.123` dari nilai `.123456`.
Jadi presisi mikrodetik tidak hilang karena decoder ini, melainkan tidak pernah dikirim.

## Cara memverifikasi ulang dokumen ini

    podman exec qh-postgres psql -U qh -d qh -tAc "SELECT version()"
    podman exec -e MYSQL_PWD=qh-dev-only qh-mysql mysql -uqh -N -B -e "SELECT VERSION()"
    curl -s http://127.0.0.1:58080/v1/info

Tabel versi lama di "Versi minimum per mesin" dihasilkan oleh skrip-skrip ini, dan
menjalankannya lagi adalah cara mengulang pengukurannya:

    deploy/dev/qh-pg-old.sh    9.5-alpine
    deploy/dev/qh-mysql-old.sh 8.0
    PLATFORM=linux/amd64 deploy/dev/qh-mysql-old.sh 5.7
    deploy/dev/qh-trino-old.sh 400          # hanya bila ada jendela memori

Kebijakan hulu berubah menurut jadwalnya sendiri — tabel dukungan PostgreSQL dan kebijakan
Oracle paling tidak perlu dibaca ulang sekali setahun, dan Trino setiap kali driver-nya
disentuh.
