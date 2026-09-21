# Golden deltas — perbedaan yang disengaja antara engine Python dan Rust

> Setiap perbedaan antara snapshot Python (`tests/golden/`) dan engine Rust **wajib** masuk salah
> satu kategori di bawah. Perbedaan yang belum diklasifikasi berarti regresi, dan membuat
> pembanding gagal.
>
> Status: **dua perbedaan sudah teridentifikasi dari snapshot**, keduanya belum diimplementasikan
> di Rust (kode Rust baru mulai ditulis). Jadi berkas ini masih catatan kontrak, bukan laporan
> hasil perbandingan.

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

### D-1 — Notasi ilmiah pada DECIMAL kecil · **Perbaikan disengaja**

`Decimal("-0.0000000001")` dirender engine Python sebagai `-1E-10`.

Bukti: `tests/golden/preview/type_zoo.ndjson`, baris `rows`, sel kedua
(`exporter/writers.py:41` memakai `str(value)`, dan `str(Decimal)` memilih notasi ilmiah).

Rust akan merender `-0.0000000001`. Alasannya: notasi ilmiah memaksa pengguna membaca ulang
eksponen mental untuk menilai besaran, dan kolom DECIMAL pada grid adalah data, bukan keluaran
kalkulator. Nilai numeriknya **identik** — yang berubah hanya tampilannya, dan `i128 + scale`
tetap tidak melewati `f64`.

Yang harus dijaga test: nilai tidak boleh kehilangan digit, dan `scale` tidak boleh ikut berubah.

### D-2 — Teks INTERVAL terbungkus tanda kutip JSON · **Perbaikan disengaja**

`timedelta(days=3, hours=4, minutes=5, seconds=6)` dirender sebagai `"3 days, 4:05:06"`
(dengan tanda kutip sebagai bagian dari string).

Bukti: `tests/golden/preview/type_zoo.ndjson`, sel kedelapan. Penyebabnya
`exporter/writers.py:50` melewati `json.dumps(value, default=str, ensure_ascii=False)`: tipe yang
tidak dikenali menjadi objek JSON, dan hasil `str()`-nya masuk sebagai *string JSON*, lengkap
dengan kutipnya.

Rust akan merender `3 days, 4:05:06`, tanpa kutip. Isi teksnya sengaja **dipertahankan sama**
supaya perbedaannya hanya pada kutip — perbedaan yang sekecil mungkin dan mudah diuji.

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
