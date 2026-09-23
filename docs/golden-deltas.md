# Golden deltas — perbedaan yang disengaja antara engine Python dan Rust

> Setiap perbedaan antara snapshot Python (`tests/golden/`) dan engine Rust **wajib** masuk salah
> satu kategori di bawah. Perbedaan yang belum diklasifikasi berarti regresi, dan membuat
> pembanding gagal.
>
> Status: **terverifikasi.** `crates/qh-ffi/tests/golden.rs` menjalankan **14 perintah** engine Rust
> dengan sesi palsu — persis seperti `record.py` men-drive engine Python in-process — lalu
> membandingkan keluarannya dengan snapshot baris per baris. Dari 43 kasus
> terekam: **17 identik**, 4 terklasifikasi di bawah ini, dan 22 diklasifikasi
> `LIVE` — direkam dari server nyata, yang hasilnya ada di `tests/golden/RECORDED.md` karena yang
> dapat dibandingkan di sini hanyalah kasus yang bisa dijalankan tanpa jaringan. Kasus yang identik
> tidak disebut lagi di sini; daftarnya ada di `EXACT` pada berkas uji itu, dan penjaganya menolak
> snapshot baru yang belum masuk salah satu daftar, dengan id yang **di-parse** dari tabel kasus
> live, bukan dicocokkan sebagai potongan teks.

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

### D-6 — `to_table` tidak memancarkan aliran `progress.state` · **Keterbatasan yang diketahui**

Hitungan barisnya sendiri sudah setara. `done.rows` pada snapshot berisi `5` dari `cursor.rowcount`
klien trino, `progress` mendahuluinya dengan `state: null`, dan engine Rust memancarkan keduanya
dengan bentuk yang sama — `to_table_create` sudah menjadi **kasus yang identik** di uji paritas.

Angkanya datang dari server, dan itu diukur, bukan diasumsikan:

| Driver | Sumber | Bukti terhadap `deploy/dev` |
|---|---|---|
| Trino | `updateCount` **di level atas page**, bukan di dalam `stats` | `CREATE TABLE AS SELECT` → 25, `INSERT` → 3, `SELECT`/`DROP` tidak menjawab apa pun (Trino 483) |
| MySQL | `affected_rows` dari paket OK, lewat `QueryResult::affected_rows()` | `create`/`append`/`replace` pada `type_zoo` → 1 baris, dan tabelnya benar-benar berisi angka itu (uji integrasi `a_write_reports_the_rows_the_server_said_it_wrote`) |
| PostgreSQL | `-1` | `CommandComplete` untuk `CREATE TABLE AS` memang tidak membawa jumlah baris, jadi `cursor.rowcount` psycopg pun `-1`. Ini **paritas**, bukan kekurangan |

Satu hal yang benar-benar belum: **`progress.state` selalu `null`.** Nilainya dulu datang dari
`stats_callback` klien trino (`{"state": "RUNNING", "writtenRows": …}`), sebuah aliran kejadian yang
tidak punya tempat di trait `Cursor` — trait itu menjawab "batch berikutnya", bukan "seberapa jauh
sekarang". Snapshot pun mencatat `null`, jadi tidak ada kasus uji yang menuntutnya; yang hilang
adalah bilah progres yang bergerak sendiri pada penulisan tabel yang lama.

Perhatikan juga satu perbedaan makna yang disengaja: MySQL selalu menjawab, dengan `0` untuk
statement yang tidak mengubah baris apa pun, sementara Trino tidak menjawab sama sekali (`None`).
Karena itu `Some(0)` dan `None` tidak boleh diperlakukan sama oleh pemanggil — lihat doc
`Cursor::affected_rows`.

### D-7 — Setting yang dibaca lalu diabaikan · **DITUTUP 23 Sep 2026**

Ketiganya sekarang berlaku, jadi D-7 berhenti menjadi perbedaan perilaku. Yang tersisa darinya satu
perbedaan bentuk yang disengaja: nilai yang **tidak dikenal ditolak dengan menyebut namanya**,
mengikuti bentuk yang sudah dipakai `sslmode` dan `DELIMITER` — dulu nilainya diterima diam-diam.

| Setting | Perilaku sekarang |
|---|---|
| `RETRIES` | Retry sesi sungguhan (blueprint §1.7). `RETRIES` = jumlah percobaan ulang, jadi `RETRIES=0` berarti tepat satu percobaan dan default-nya 5, sama dengan engine Python. Hanya kegagalan `Transient` yang diulang; statement yang ditolak server, cancel, dan fetch di tengah stream pada driver ber-cursor (PostgreSQL, MySQL) tidak diulang |
| `ENCODING` | Code page untuk `txt` dan `csv` lewat `Codec` (`crates/qh-export/src/encoding.rs`), dengan `errors="replace"` dan gerbang BOM yang mengikuti `writers.py:126` |
| `DBF_ENCODING` | Code page medan karakter `dbf`, plus byte language-driver di header yang kini **mengikuti** code page — engine Python selalu menulis `0x03` apa pun codec-nya, yang memberi tahu pembaca untuk men-decode UTF-8 sebagai cp1252 |

Nilai yang sah tetap **diterima tanpa error**, supaya koneksi tersimpan yang membawanya tidak
mendadak gagal; yang tidak sah ditolak. Tidak satupun muncul di snapshot, jadi tidak ada kasus uji
yang terpengaruh.

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

### D-8 — `columns.type` adalah **nama tipe**, bukan kode DBAPI · **Perbaikan disengaja**

Mesin Python mengirim apa yang diberikan deskripsi DBAPI: sebuah **kode angka** — `23` untuk
`int4` Postgres, `3` untuk `int` MySQL, `254` untuk kolom enum MySQL. Mesin Rust mengirim
namanya: `int4`, `int`, `enum`.

Kode itu bukan desain, melainkan sisa dari cara pustaka klien mendeskripsikan kolom: artinya
berbeda di tiap DBAPI, berubah antar versi pustaka, dan — yang membuatnya tidak bisa
dipertahankan — **tidak cukup untuk mengatakan apa kolomnya**. MySQL melaporkan `ENUM`, `CHAR`
dan `BINARY` semuanya sebagai `254`, jadi `254` dan `char` sama-sama berarti "mungkin enum".
`enum` tidak. Itu satu-satunya dari ketiga selisih di kasus tipe MySQL yang diperbaiki, dan
perbaikannya membaca flag kolom, bukan kode yang dipakai bersama.

Nama juga satu-satunya dari keduanya yang bisa dipakai pemanggil: aplikasi bisa menampilkannya
dan bercabang atasnya; sebuah kode angka membuatnya harus menghafal tabel milik pustaka lain.

**Akibatnya pada klasifikasi:** beberapa kasus live berbeda **hanya** di medan ini. Mereka
diklasifikasikan oleh entri ini, bukan ditimbang ulang satu per satu tiap kali. Peringatannya:
satu perubahan penamaan di sisi driver menggerakkan beberapa kasus sekaligus, jadi perubahan
seperti itu dilakukan dengan sadar, bukan sebagai efek samping.

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

## Server nyata

Bagian ini dulu berisi kandidat yang **belum dapat diverifikasi**: podman sudah ada tetapi compose
dan dataset uji belum dibuat, jadi perbedaan yang hanya muncul pada server nyata tidak bisa direkam.
Itu sudah tidak berlaku. 22 kasus sudah direkam dari server nyata dan hasilnya — beserta setiap
sel yang berbeda dan sebabnya — ada di `tests/golden/RECORDED.md`, yang diperbarui dengan angka yang
sama setiap kali kasus live dijalankan ulang (`tools/golden/live_cases.py`, dan tanpa argumen ia
**membandingkan**, tidak merekam). Kandidat di bawah ini dipertahankan hanya sebagai daftar hal yang
dulu diduga berbeda; yang sudah punya kasus live, jawabannya ada di berkas itu, bukan di sini:

| Kandidat (historis) | Kategori dugaan | Cara menutup |
|---|---|---|
| Kandidat | Kategori dugaan | Cara menutup |
|---|---|---|
| Urutan kunci `jsonb` Postgres | **Bukan regresi** | `jsonb` tidak menjamin urutan; perbandingan harus kanonik. Diuji dengan perbandingan kanonik, bukan teks |
| DECIMAL presisi penuh tidak lagi menjadi `float` di jalur JSON-native | **Perbaikan disengaja** | `exporter/writers.py:62` memakai `float(value)`, yang membuang presisi. Jalur `preview` tidak boleh mewarisinya |
| Geometri dirender sebagai teks | **Perbaikan disengaja** | Hari ini bergantung pada `str()`; Rust memakai `Value::Unknown` secara eksplisit |
| `elapsed_ms` dan `query_id` berbeda | **Bukan regresi** | Runtime berbeda; sudah dinormalisasi oleh `tools/golden/record.py` |
| Kata-kata pada pesan error berbeda | **Bukan regresi** | Yang dikontrak adalah *informasi* (pesan, kode, posisi), bukan string identik |
| MySQL `TIMESTAMP` dikonversi ke zona sesi | **Perbaikan disengaja** (dipertahankan) | Server yang memutuskan; perbedaan dari `DATETIME` harus terlihat, bukan disamarkan |
