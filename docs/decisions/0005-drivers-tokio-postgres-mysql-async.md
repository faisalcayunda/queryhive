# 0005 — Driver PostgreSQL/MySQL: `tokio-postgres` + `mysql_async`, bukan `sqlx`

- **Status:** Diterima
- **Tanggal:** sesi Fase 0
- **Konteks instruksi:** §5 poin 6, §8 Bagian 3

## Konteks

Kebutuhan driver di aplikasi ini berbeda dari kebutuhan aplikasi yang query-nya ditulis saat
compile-time:

- Query datang dari **pengguna**, jadi bentuk kolom tidak diketahui sampai runtime.
- Hasil harus **streaming** dalam batch, bukan dikumpulkan.
- **Cancel** harus benar-benar membatalkan di server (§2.7).
- Tipe tak dikenal dari server **tidak boleh** menyebabkan panic (§7.2) — harus jatuh ke `Unknown`.

## Opsi yang dipertimbangkan

| Opsi | Kelebihan | Kekurangan |
|---|---|---|
| `sqlx` (terverifikasi 0.9.0, `MIT OR Apache-2.0`, rilis 2026-05-21, lihat `docs/dependencies.md`) | Pool + migrasi + TLS terintegrasi; satu API untuk PG/MySQL/SQLite | Model `Row`/`Decode` berorientasi tipe konkret; untuk nilai dinamis harus melalui `AnyRow`/`sqlx::ValueRef`, yang tetap menyalin per kolom. Cancel Postgres tidak diekspos. |
| **`tokio-postgres` + `mysql_async`** | Nilai dinamis native (`postgres_types::Type` + raw bytes); `tokio-postgres` mengekspos `cancel_token()`; `mysql_async` memberi `Conn::id()` (dibutuhkan `KILL QUERY`) dan `QueryResult` streaming yang bisa di-`collect` per batch | Tidak ada pool bawaan → harus menyusun sendiri; dua API berbeda untuk dua database |
| `mysql` + `postgres` (sinkron) | API sederhana | Memblokir thread tokio; bertentangan dengan §5 poin 4 pada skalanya (menghabiskan worker thread) |

## Keputusan

`tokio-postgres` untuk PostgreSQL, `mysql_async` untuk MySQL, pool disusun dari `deadpool-postgres`
dan pool bawaan `mysql_async`. `sqlx` tidak dipakai, dan tetap tercatat di `docs/dependencies.md`
sebagai alternatif yang dievaluasi.

## Alasan

1. **Cancel adalah fitur wajib (§6 < 500 ms), bukan tambahan.** `tokio-postgres` menyediakan
   `cancel_token()`; dengan `sqlx` ini harus dirakit dari nol lewat `postgres-protocol`. Ini
   argumen terkuat dan berdiri sendiri.
2. **Nilai dinamis tanpa perantara.** Titik masuk driver adalah "beri saya batch nilai apa adanya";
   lapisan `Decode` `sqlx` akan menyalin setiap kolom menjadi tipe Rust konkret, lalu kami
   konversi lagi ke `qh_core::Value` — dua konversi untuk data yang sama, pada jalur yang justru
   sedang diperbaiki.
3. **Konsistensi dengan §5 poin 6.** Trait driver tidak boleh mengasumsikan pool TCP persisten
   (Trino tidak punya). Pool yang hidup di dalam crate driver, bukan di trait, membuat asumsi itu
   tidak bocor ke abstraksi bersama — dan itu lebih mudah dilakukan tanpa pool terintegrasi
   `sqlx` yang mendorong desain "semua driver punya pool".

## Konsekuensi

- `deadpool-postgres` menambah satu dependency di luar daftar utama; lisensinya diperiksa
  `cargo-deny` bersama yang lain.
- Protokol biner harus ditangani sendiri: setiap tipe Postgres/MySQL dipetakan di
  `normalize()` per driver ke `qh_core::Value`. Ini pekerjaan yang harus dilakukan pada opsi
  mana pun (normalisasi tetap diperlukan), tetapi di sini tanggung jawabnya eksplisit.
- Pengelolaan siklus hidup koneksi (kembalikan ke pool hanya setelah `ReadyForQuery` benar
  diterima pasca-cancel) menjadi kode kami. Ini justru salah satu hal yang diuji §7.5.
- Bila di masa depan driver ke-4 memerlukan ORM/migrasi, `sqlx` tetap dapat ditambahkan sebagai
  crate terpisah; keputusan ini tidak mengunci arsitektur.
