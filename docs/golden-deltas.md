# Golden deltas — perbedaan yang disengaja antara engine Python dan Rust

> Setiap perbedaan antara snapshot Python (`tests/golden/`) dan engine Rust **wajib** masuk salah
> satu kategori di bawah. Perbedaan yang belum diklasifikasi berarti regresi, dan membuat
> pembanding gagal.
>
> Status: **terverifikasi.** `crates/qh-ffi/tests/golden.rs` menjalankan 11 perintah engine Rust
> dengan sesi palsu — persis seperti `record.py` men-drive engine Python in-process — lalu
> membandingkan keluarannya dengan snapshot baris per baris. Hasilnya: **16 kasus identik**, dan 5
> kasus terklasifikasi di bawah ini. Kasus yang identik tidak disebut lagi di sini; daftarnya ada
> di `EXACT` pada berkas uji itu, dan sebuah penjaga menolak snapshot baru yang belum masuk salah
> satu daftar.

## Aturan

| Kategori | Arti | Tindakan |
|---|---|---|
| **Perbaikan disengaja** | Rust menghasilkan sesuatu yang berbeda dan lebih benar | Ditulis di sini beserta alasannya, dan diuji |
| **Bukan regresi** | Perbedaannya tidak dapat dihindari dan tidak bermakna bagi pengguna | Ditulis di sini dengan alasan mengapa perbandingan harus dinormalisasi |
| **Regresi** | Perubahan yang tidak dikehendaki | Gagal; diperbaiki, tidak boleh diterima |

## Sudah teridentifikasi

### D-3 — `timestamptz` dirender di zona waktu **sesi**, bukan zona saat ditulis · **Bukan regresi**

Ditemukan oleh uji integrasi terhadap server nyata
(`crates/qh-driver-postgres/tests/integration.rs`), bukan oleh penalaran.

`type_zoo.tz_aware` ditulis sebagai `TIMESTAMPTZ '2026-01-31 12:00:00.123456+07'`.
Yang kembali dari PostgreSQL adalah `2026-01-31 05:00:00.123456+00:00`.

Ini **perilaku server**, bukan offset yang hilang: PostgreSQL menyimpan instant-nya
saja, lalu merendernya di `TimeZone` milik sesi, yang default-nya UTC. Instant-nya
benar — `12:00:00.123456+07:00` dan `05:00:00.123456+00:00` adalah momen yang sama,
terpaut tujuh jam di jam dinding.

Bukti bahwa offset-nya tidak hilang: setelah `SET TIME ZONE 'Asia/Jakarta'`, kolom
yang sama dirender `2026-01-31 12:00:00.123456+07:00`. Kedua pernyataan itu diuji.

Konsekuensi yang mengikat:

1. **Perbandingan golden snapshot harus menyetel zona waktu sesi** sebelum merekam,
   atau membandingkan instant-nya dan bukan teksnya. Tanpa itu, perbedaan rendering
   akan terlihat seperti regresi padahal bukan.
2. **Driver tidak menyetel zona waktu sesi.** Siapa pun yang menentukan — server —
   yang benar, sama seperti keputusan untuk MySQL `TIMESTAMP` di tabel bawah. Jika
   nanti aplikasi ingin menampilkan nilai di zona pengguna, itu keputusan tampilan
   yang harus diambil eksplisit, bukan default diam-diam di driver.
3. Bentuk yang sama berlaku untuk kolom `timestamp` naive: ia **tidak** boleh
   mendapat offset, dan itu diuji terpisah.

### D-4 — Pesan `error` tanpa nama kelas exception Python · **Perbaikan disengaja**

Engine Python membungkus setiap kegagalan dengan nama kelasnya: `ValueError: SQL or SQL_PATH is
required`, `OSError: connection refused` (`queryhive_engine.py:1005`).

Bukti: `tests/golden/preview/blank_sql.ndjson` dan `tests/golden/test/connect_failure.ndjson`.

Rust memancarkan pesannya saja. Nama kelas exception bukan bagian dari protokol — yang dibaca app
adalah teksnya, dan pengguna tidak perlu tahu bahwa kegagalannya dulu datang dari `OSError`.

Yang harus dijaga test: pesannya sendiri tetap sama kata per kata, dan `warnings` tetap ikut pada
kegagalan yang sudah mengubah sesuatu (DROP dari `replace`).

### D-5 — Baris `usage` memuat nama binary, bukan nama skrip · **Bukan regresi**

`usage: queryhive_engine.py db_drivers|objects|…` menjadi `usage: queryhive-engine …`.

Bukti: `tests/golden/usage/unknown_command.ndjson`.

Yang diurai app adalah **akhiran**-nya (daftar perintah, urutannya tetap), bukan nama program —
dan nama program memang harus berubah, karena yang menjalankan bukan lagi skrip Python. Uji
`an_unknown_command_is_one_error_event` membandingkan akhiran itu dan memastikan hanya nama
programnya yang berbeda.

### D-6 — `to_table`: hitungan baris dari server, kecuali di dua driver · **Sebagian tertutup**

`done.rows` pada snapshot berisi `5` dari `cursor.rowcount` klien trino, dan `progress` mendahuluinya.
Engine Rust sekarang memancarkan keduanya dengan bentuk yang sama, dan `to_table_create` sudah naik
dari "perbedaan yang diterima" menjadi **kasus yang identik** di uji paritas.

Sumber angkanya sudah diukur, bukan diasumsikan: Trino mengirim `updateCount` **di level atas page**,
bukan di dalam `stats` — `CREATE TABLE AS SELECT` menjawab 25, `INSERT` menjawab 3, dan `SELECT` atau
`DROP` tidak menjawab apa pun (Trino 483, `deploy/dev`). Field itulah yang dibaca klien Python untuk
mengisi `rowcount`, jadi inilah yang membuat laporan sebuah penulisan sama dengan yang dulu diberikan
engine Python. Terhadap server sungguhan: `create` → 25, `append` → 3, `replace` → 2 (DROP tidak
menghasilkan hitungan, jadi angka yang bertahan adalah milik CREATE).

Yang masih terbuka, dan alasannya berbeda-beda:

| Bagian | Keadaan |
|---|---|
| `progress.state` | Selalu `null`. Nilainya dulu datang dari `stats_callback` klien trino (`RUNNING`, `writtenRows`), sebuah aliran yang tidak dimiliki trait `Cursor`. Snapshot juga mencatat `null`, jadi ini setara — tapi bukan aliran progres sungguhan |
| PostgreSQL | `-1`. `CommandComplete` untuk `CREATE TABLE AS` tidak membawa jumlah baris, jadi `cursor.rowcount` psycopg pun `-1`: ini **paritas**, bukan kekurangan |
| MySQL | `-1`. Paket OK MySQL membawa `affected_rows`, tapi driver ini belum mem-parse-nya. Ini yang paling mudah ditutup berikutnya |

### D-7 — Setting yang dibaca lalu diabaikan · **Keterbatasan yang diketahui**

| Setting | Mengapa diabaikan |
|---|---|
| `RETRIES` | Retry adalah milik lapisan session (blueprint §1.7) yang belum dibangun. Pada koneksi yang putus di tengah ekspor, ini perbedaan perilaku nyata — bukan detail |
| `ENCODING` | Semua writer engine Rust adalah UTF-8; CSV/teks `cp1252` tidak didukung |
| `DBF_ENCODING` | Code page `dbf` tetap cp1252 |

Ketiganya tetap **diterima tanpa error**, supaya koneksi tersimpan yang membawanya tidak mendadak
gagal. Tidak satupun muncul di snapshot, jadi tidak ada kasus uji yang terpengaruh.

Diverifikasi terhadap server sungguhan: PostgreSQL melaporkan interval `14 months, 3 days, 4:05:06`
(`SELECT * FROM type_zoo`), dan itulah teks yang dipancarkan engine — dengan bagian bulan, yang
tidak bisa dinyatakan `timedelta` Python sama sekali. Jadi untuk nilai yang Python bisa pegang,
teksnya identik kecuali kutipnya; untuk yang tidak bisa, bagian bulannya ditambahkan dan itu
ekstensi yang jujur, bukan tebakan.

### D-1 — Notasi ilmiah pada DECIMAL kecil · **Perbaikan disengaja**

`Decimal("-0.0000000001")` dirender engine Python sebagai `-1E-10`.

Bukti: `tests/golden/preview/type_zoo.ndjson`, baris `rows`, sel kedua
(`exporter/writers.py:41` memakai `str(value)`, dan `str(Decimal)` memilih notasi ilmiah).

Rust akan merender `-0.0000000001`. Alasannya: notasi ilmiah memaksa pengguna membaca ulang
eksponen mental untuk menilai besaran, dan kolom DECIMAL pada grid adalah data, bukan keluaran
kalkulator. Nilai numeriknya **identik** — yang berubah hanya tampilannya, dan `i128 + scale`
tetap tidak melewati `f64`.

Yang harus dijaga test: nilai tidak boleh kehilangan digit, dan `scale` tidak boleh ikut berubah.

### D-2 — Teks INTERVAL: kutip JSON dibuang, dan bulan ikut tampil · **Perbaikan disengaja**

`timedelta(days=3, hours=4, minutes=5, seconds=6)` dirender sebagai `"3 days, 4:05:06"`
(dengan tanda kutip sebagai bagian dari string).

Bukti: `tests/golden/preview/type_zoo.ndjson`, sel kedelapan. Penyebabnya
`exporter/writers.py:50` melewati `json.dumps(value, default=str, ensure_ascii=False)`: tipe yang
tidak dikenali menjadi objek JSON, dan hasil `str()`-nya masuk sebagai *string JSON*, lengkap
dengan kutipnya.

Rust akan merender `3 days, 4:05:06`, tanpa kutip. Isi teksnya sengaja **dipertahankan sama**
supaya perbedaannya hanya pada kutip — perbedaan yang sekecil mungkin dan mudah diuji.

## Temuan yang sudah ditutup

### T-1 — Dua aturan berbeda untuk `timestamptz` · **Ditutup 22 Sep 2026**

`crates/qh-core/src/value.rs` (`Value::render_text`) **menambahkan** offset ke `micros` saat
merender timestamp berzona, sementara `crates/qh-core/src/render.rs` (`to_text`) mencetak `micros`
apa adanya lalu menempelkan offset-nya.

Yang benar adalah yang kedua: driver menyimpan **jam dinding server** di `micros` dan zonenya di
`offset_secs` (dipatok uji `crates/qh-driver-trino/src/decode.rs`: `"2026-01-31 12:00:00.123
+07:00"` → `micros = 1_769_860_800_123_000`, `offset_secs = Some(25_200)`), dan hanya dengan
aturan itu hasilnya sama dengan snapshot (`type_zoo` identik). Aturan di `value.rs` akan
menghasilkan `19:00:00+07:00` untuk nilai yang sama.

Dampaknya: `qh-result-store` (yang memakai `render_text`) bisa merender `timestamptz` satu zona
lebih maju daripada jalur ekspor. Belum ada uji yang menangkapnya karena type zoo hanya diuji
lewat jalur perintah. Dua aturan untuk satu kontrak harus menjadi satu — pekerjaan berikutnya,
dan `render::to_text` adalah rumah yang benar.

**Bukan hanya dua renderer: dua decoder juga berbeda.** Trino menyimpan **jam dinding** di
`micros` dan zonenya di `offset_secs`; PostgreSQL menyimpan **instant** (`parse_timestamp`
mengurangi offsetnya). Dua konvensi berarti tidak ada satu aturan render yang benar untuk
keduanya, jadi yang harus disatukan lebih dulu adalah **model nilainya**.

Yang dipilih adalah model Python, dan model itu **instant + zona**: sebuah `datetime` adalah satu
instant dengan `tzinfo`, dan `isoformat(sep=" ")` mencetak jam dinding *di zona itu*. Maka
`2026-01-31 12:00:00+07:00` disimpan sebagai 05:00Z dengan offset 25200 — yang sudah dilakukan
PostgreSQL — dan decoder Trino diubah untuk mengurangi offsetnya. Renderer-nya kini satu: `micros`
digeser ke zona yang dilaporkan sebelum tanggal dan jamnya diambil, lalu offsetnya dicetak.

**Bukti hidup (22 Sep 2026).** Zona default database dev diubah ke `Asia/Jakarta`, dan baris yang
sama di `type_zoo` dirender:

| Zona sesi | Keluaran engine |
|---|---|
| `UTC` | `2026-01-31 05:00:00.123456+00:00` |
| `Asia/Jakarta` | `2026-01-31 12:00:00.123456+07:00` |

Instant yang sama, ditampilkan di zona yang dilaporkan server — persis perilaku Python. Zona
container dikembalikan ke default setelah pengukuran. Aturan lama di jalur ekspor akan mencetak
`05:00:00.123456+07:00` untuk baris kedua: jam UTC yang dilabeli `+07:00`, meleset tujuh jam
tanpa terlihat salah.

**Yang ikut dibersihkan.** Duplikasi yang menyebabkan perbedaan ini dihapus: `Value::render_text`
kini memanggil [`qh_core::render::to_text`], dan 338 baris salinan kedua di `value.rs` (termasuk
`format_float` yang tidak setia, `format_sequence`/`format_map`, dan `civil_from_days`) hilang.
Aturan float yang setia dari `value.rs` — termasuk pergantian ke notasi eksponen di ambang Python
(1e16 ke atas, di bawah 1e-4) — diangkat ke `render.rs`, jadi catatan lama di modul itu yang
menyebut eksponen sebagai "divergensi yang dicatat" tidak berlaku lagi.

## Yang belum dapat diverifikasi

Perbedaan berikut hanya muncul pada server nyata dan tidak dapat direkam pada sesi ini (podman
tersedia, tetapi compose dan dataset uji belum dibuat). Semuanya masih **[perlu diverifikasi]**:

| Kandidat | Kategori dugaan | Cara menutup |
|---|---|---|
| Urutan kunci `jsonb` Postgres | **Bukan regresi** | `jsonb` tidak menjamin urutan; perbandingan harus kanonik. Diuji dengan perbandingan kanonik, bukan teks |
| DECIMAL presisi penuh tidak lagi menjadi `float` di jalur JSON-native | **Perbaikan disengaja** | `exporter/writers.py:62` memakai `float(value)`, yang membuang presisi. Jalur `preview` tidak boleh mewarisinya |
| Geometri dirender sebagai teks | **Perbaikan disengaja** | Hari ini bergantung pada `str()`; Rust memakai `Value::Unknown` secara eksplisit |
| `elapsed_ms` dan `query_id` berbeda | **Bukan regresi** | Runtime berbeda; sudah dinormalisasi oleh `tools/golden/record.py` |
| Kata-kata pada pesan error berbeda | **Bukan regresi** | Yang dikontrak adalah *informasi* (pesan, kode, posisi), bukan string identik |
| MySQL `TIMESTAMP` dikonversi ke zona sesi | **Perbaikan disengaja** (dipertahankan) | Server yang memutuskan; perbedaan dari `DATETIME` harus terlihat, bukan disamarkan |
