# Kompatibilitas versi — apa yang diuji, apa yang dijanjikan

> Isi dokumen ini berasal dari dua hal saja, dan tidak ada yang lain: **kebijakan versi
> hulu** yang dibaca dari situs resmi masing-masing proyek, dan **versi yang benar-benar
> dijalankan** di container dev. Tanggal akses seluruh sumber: **2026-09-22**.
>
> Angka versi di sini tidak ada yang dikutip dari ingatan. Kalau ada baris yang belum
> punya sumber, ia ditandai **belum**, bukan diisi dengan perkiraan — §5 poin 9 blueprint
> melarang mengarang sumber.

## Ringkasan

| Mesin | Teruji di sini | Kebijakan hulu | Sumber |
|---|---|---|---|
| PostgreSQL | **17.11** | Rilis mayor ~1×/tahun, tiap mayor didukung **5 tahun** | [versioning policy](https://www.postgresql.org/support/versioning/) |
| MySQL | **8.4.11** (Community) | Dua jalur: **LTS** dan **Innovation** | [MySQL Releases](https://docs.oracle.com/cd/E17952_01/mysql-8.4-en/mysql-releases.html) |
| Trino | **483** | Rilis mingguan, tanpa semver, **tanpa jaminan antarversi** | [diskusi #20032](https://github.com/trinodb/trino/discussions/20032), [release notes](https://trino.io/docs/current/release.html) |

"Teruji di sini" berarti seluruh uji integrasi dijalankan terhadapnya. Ia **bukan** klaim
tentang versi lain: versi lain tidak ditolak, tetapi juga tidak dijanjikan.

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

## Yang belum ditetapkan

**Versi minimum per mesin belum diketahui.** Yang tercatat di atas adalah versi *teruji*,
bukan versi *minimum*. Menuliskan "PostgreSQL 15+" atau "MySQL 8.0+" tanpa mengukurnya akan
menjadi klaim yang tidak bersumber — persis yang dihindari dokumen ini.

Cara menutupnya sudah jelas dan mekanis, karena uji integrasinya sudah ada:

1. Jalankan `deploy/dev/up.sh` pada versi yang lebih tua (mis. PostgreSQL 15, MySQL 8.0).
2. Jalankan `QH_TEST_POSTGRES=1 QH_TEST_MYSQL=1 cargo test --workspace`.
3. Catat versi terendah yang seluruh ujinya lulus, dan versi pertama yang gagal beserta
   kegagalannya.
4. Perbarui tabel ringkasan dengan hasilnya.

Untuk Trino, versi yang berjalan adalah 483 tetapi **belum ada uji sama sekali** — yang ada
baru pemeriksaan protokol manual. Karena Trino tidak memberi jaminan antarversi, setiap
klaim tentangnya harus menyebut nomor rilis, dan menguji pada rilis lain berarti menjalankan
ulang pengujiannya pada rilis itu.

## Cara memverifikasi ulang dokumen ini

    podman exec qh-postgres psql -U qh -d qh -tAc "SELECT version()"
    podman exec -e MYSQL_PWD=qh-dev-only qh-mysql mysql -uqh -N -B -e "SELECT VERSION()"

Kebijakan hulu berubah menurut jadwalnya sendiri — tabel dukungan PostgreSQL dan kebijakan
Oracle paling tidak perlu dibaca ulang sekali setahun, dan Trino setiap kali driver-nya
disentuh.
