# 0018 — Parquet ditulis oleh crate Rust native, bukan DuckDB

- **Status:** Diterima
- **Tanggal:** 29 Sep 2026 (Fase 5.2)
- **Konteks instruksi:** `docs/architecture/tablepro-adoption-plan.md` §7 (5.2), §0, §10;
  `docs/architecture/tablepro-source-study.md` §4

## Konteks

Rencana §5.2 meminta Parquet sebagai format kesepuluh di `crates/qh-export`. Pohon ini punya
sembilan format streaming dan tidak satu pun kolumnar, sementara pemakainya nyata: Trino, Iceberg,
Spark.

Studi sumber menjawab pertanyaan yang paling penting lebih dulu: Parquet di TablePro **bukan**
encoder sendiri, melainkan DuckDB yang di-link sebagai static library (`libduckdb.a`), dan itu
sebabnya plugin-nya registry-only — terlalu besar untuk ikut ke dalam bundel. Untuk QueryHive,
menjadikan `libduckdb` sebagai dependensi adalah keputusan arsitektur besar: satu pustaka C yang
besar, di pohon MIT, untuk pekerjaan yang Rust sekarang bisa kerjakan sendiri.

Dua hal lain di pohon ini yang mengikat keputusan: `qh-export` adalah penulis **streaming** (aturan
`README.md`: result set tidak pernah ditahan di memori), dan `cargo deny check licenses` adalah
penegak kebijakan lisensi ADR-0002.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Encoder | **Crate `parquet` native** | MIT/Apache; tanpa C yang di-link; satu bahasa | Graf dependensi baru dan sebuah exception lisensi |
| | DuckDB (`libduckdb.a`) | Sudah teruji di TablePro | Pustaka C besar; lisensi MIT DuckDB bukan masalahnya, ukurannya yang; alur `COPY` menahan staging |
| | Tulis format sendiri | Tidak ada dependensi | Parquet adalah format thrift + encoding berlapis; pekerjaan besar untuk hasil yang dipertanyakan |
| Streaming | **Baris dikumpulkan per row group, lalu dilepas** | Puncak memori satu row group, bukan seluruh hasil | Bukan flat-per-baris seperti penulis teks |
| | Tahan seluruh result set lalu encode | Paling sederhana | Melanggar aturan `README.md` |
| Pemetaan tipe | **Boolean/INT64/DOUBLE/BYTE_ARRAY(UTF8) dari nama tipe** | Cukup untuk pembaca nyata | Decimal, tanggal, waktu jadi teks, bukan logical type |

## Keputusan

**Format kesepuluh di `crates/qh-export` memakai crate `parquet` (59.3) native, tanpa fitur arrow,
dan menulis kolom lewat API level-rendah `SerializedFileWriter`.** Baris dikumpulkan ke dalam sebuah
row group (65.536 baris) dan column chunk-nya di-encode lalu dilepas; puncak memori adalah satu row
group. Tidak ada `arrow` yang dinyalakan, jadi tidak ada tabel kolumnar in-memory yang dibangun.

Rincian yang mengikat:

1. **Compression `UNCOMPRESSED`.** Fitur codec tidak dinyalakan, jadi tidak ada codec C yang ikut.
   Kompresi adalah penyetelan lanjutan, bukan syarat format.
2. **Pemetaan tipe dari nama tipe kolom**, dipotong pada `(` seperti pencocokan tipe di sisi app:
   bool ke `BOOLEAN`, keluarga integer ke `INT64`, float/double ke `DOUBLE`, sisanya ke
   `BYTE_ARRAY`(UTF8). Nilai yang tidak cocok dengan tipenya menjadi NULL, bukan kegagalan ekspor —
   bentuk `TRY_CAST` yang studi catat dari TablePro.
3. **Decimal, tanggal, waktu, dan timestamp ditulis sebagai teks kanonik** (`qh-core::render`),
   bukan logical type Parquet. `DECIMAL` Parquet terbatas 38 digit dan timestamp-nya tidak punya
   tempat untuk offset yang server laporkan; teks tidak kehilangan keduanya.
4. **Satu file per tabel.** `Format::Parquet::max_rows` adalah `None`; Parquet tidak dipecah
   otomatis, karena beberapa file Parquet adalah beberapa tabel, bukan beberapa bagian.
5. **Satu exception lisensi.** `parquet` menarik `ahash` → `const-random` → `tiny-keccak`, dan
   `tiny-keccak` berlisensi CC0-1.0, yang tidak ada di allow-list ADR-0002. CC0 adalah dedikasi
   domain publik, bukan copyleft; ia masuk sebagai `exceptions` untuk crate itu saja, dengan alasan
   yang sama seperti exception MPL-2.0 dan CDLA: crate lain yang datang CC0 tetap harus berargumen
   lewat ADR sendiri.

## Alasan

1. **Pola yang diambil adalah polanya, bukan pustakanya.** Studi §4 menyebut encoder native sebagai
   rekomendasinya, dan `parquet`/`arrow` adalah MIT/Apache-2.0. Yang out of bounds adalah
   `DuckDBStagingDatabase` dan plugin Parquet TablePro, bukan ide "tulis Parquet dengan sesuatu yang
   bisa streaming".
2. **Aturan streaming tidak dilonggarkan.** row group adalah batas memori yang jujur: formatnya
   memang kolumnar dan encodernya memang menahan satu grup, jadi yang dijanjikan adalah "satu row
   group", bukan "satu baris" — dan itu dikatakan di doc modul, bukan disembunyikan.
3. **Nilai yang buruk menjadi NULL, bukan kegagalan.** Satu timestamp yang tidak bisa diparse di
   antara sejuta baris tidak boleh menggagalkan seluruh file; ini keputusan yang sama yang studi
   catat dari `TRY_CAST` TablePro.
4. **Teks untuk tipe yang Parquet tidak bisa jaga.** Mengubah `NUMERIC(38,10)` menjadi logical type
   berarti memilih antara presisi dan format; proyek ini punya tipe nilai yang ada justru untuk
   menjaga presisi itu.

## Konsekuensi

- **Graf dependensi bertambah dan `docs/dependencies.md` diperbarui.** `parquet`, `calamine`, `csv`
  (untuk impor, ADR-0019) beserta transitifnya masuk tabel. `cargo deny check licenses` tetap hijau.
- **Kriteria selesai dipenuhi dengan pembaca independen.** Berkas yang ditulis engine dibaca kembali
  oleh `pyarrow` 21.0.0 (implementasi C++ Apache Arrow, bukan crate Rust yang menulisnya); skema dan
  nilainya cocok. Perbandingan itu dicatat di §7 rencana.
- **Yang belum.** Tidak ada varian logical type (decimal/timestamp) dan tidak ada pemilihan
  kompresi; keduanya penyempurnaan, bukan celah yang disembunyikan. `Format::renderable` tetap
  `false` untuk Parquet, jadi jalur render paralel tidak menyentuhnya.
