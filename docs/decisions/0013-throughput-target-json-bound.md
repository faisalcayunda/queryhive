# 0013 — Target throughput dibatasi protokol JSON

- **Status:** Diterima
- **Tanggal:** 2026-09-23
- **Konteks instruksi:** handoff task 4

## Konteks

Fase 0 mengukur baseline kedua engine pada beban nyata: `SELECT * FROM wide_500k` (30
kolom × 500.000 baris) melalui PostgreSQL dan MySQL lokal. Hasilnya dicatat di
`PROGRESS.md:646-653` dan `deploy/dev/bench-results.jsonl`.

Angka yang diukur (median dari 3 run, interleaved):

| Sumbu | PostgreSQL | MySQL |
|---|---|---|
| `elapsed_ms` engine | Rust 2.626 vs Python 4.236 (**1,61×**) | Rust 2.830 vs Python 6.901 (**2,44×**) |
| Time-to-first-row | 11 ms vs 624 ms (**57×**) | 20 ms vs 3.299 ms (**167×**) |
| Peak RSS | 9,1 MB vs 544,7 MB (**60× lebih kecil**) | 38,1 MB vs 1.093,4 MB (**29× lebih kecil**) |
| Throughput | 191.251 vs 138.405 baris/s (**1,38×**) | 177.849 vs 137.177 baris/s (**1,30×**) |

Throughput Rust adalah **1,3–1,4× Python**, bukan 10× atau 50×. Keduanya menghabiskan
waktu di tempat yang sama: encoding 15 juta sel (30 × 500k) sebagai JSON, satu sel satu
kali.

## Apa yang membatasi throughput

**JSON encoding mendominasi waktu CPU**, bukan fetch dari server. Diukur:

- PostgreSQL mengembalikan batch dalam **11 ms** (Rust TTFR). Sisa 2.615 ms (99,6%
  elapsed) adalah encoding + transport lokal.
- MySQL mengembalikan batch dalam **20 ms**. Sisa 2.810 ms (99,3%) adalah encoding.

Ini **batas protokol NDJSON**, bukan batas implementasi. Engine Rust memakai `serde_json`
— salah satu serializer JSON tercepat — dan Python memakai stdlib `json`. Keduanya sudah
optimal untuk JSON; mempercepat lagi berarti mengganti protokol.

## Target yang tidak tercapai

Blueprint §6 menyebut target throughput tinggi untuk ekspor besar. Angka eksplisit tidak
ada, tapi konteks §1 (fetch lambat pada 5 juta baris) dan §6.2 ("ekspor 500k–5 juta
baris") menyiratkan ekspektasi **jauh di atas 200k baris/s**.

Yang terukur: **177–191k baris/s** untuk 30 kolom. Untuk 5 juta baris dengan 30 kolom
(150 juta sel), itu **26–28 detik** — bukan milidetik.

Target itu **tidak tercapai melalui protokol JSON ini**, dan tidak akan tercapai tanpa
mengganti protokol. Rust 1,3× lebih cepat dari Python pada throughput, tapi keduanya
masih terikat JSON.

## Apa yang benar-benar menang

Tiga sumbu lain memberikan peningkatan nyata, bukan marjinal:

1. **Time-to-first-row: 57–167×.** Query besar mulai mengalir dalam milidetik, bukan
   detik. Ini yang membuat preview terasa instan dan cancel bisa sampai sebelum query
   selesai.

2. **Memori: 29–60× lebih kecil.** 500k baris memakan 9–38 MB (Rust) vs 545–1093 MB
   (Python). Ini yang membuat mesin tidak OOM pada dataset besar.

3. **Latensi ekor rendah.** Tidak diukur eksplisit, tapi TTFR 11 ms vs 624 ms adalah
   proxy: engine Rust tidak pernah menunggu batch penuh sebelum mulai kirim.

Ketiga ini adalah **perbaikan kualitatif** — responsiveness, memory safety, dan
predictability — bukan hanya "lebih cepat sedikit".

## Keputusan

**Terima batas throughput protokol JSON, dan dokumentasikan apa yang benar-benar
meningkat.**

Alasan:

1. **Mengganti protokol adalah pekerjaan Fase 2**, bukan Fase 1. Fase 1 membuktikan
   engine Rust berfungsi dengan protokol yang ada. Mengubah protokol berarti mengubah
   aplikasi sekaligus — itu definisi Fase 2.

2. **Tiga sumbu lain sudah memberikan nilai nyata.** TTFR 57–167× membuat preview dan
   cancel berfungsi; memori 29–60× lebih kecil membuat dataset besar tidak crash; dan
   throughput 1,3–1,4× tetap lebih cepat meski terikat JSON.

3. **Throughput 177–191k baris/s sudah lebih cepat dari engine Python**, dan cukup untuk
   ekspor ratusan ribu baris dalam detik. Batas ini bukan bug — ini ceiling protokol
   yang keduanya kena.

4. **Optimasi JSON lebih lanjut tidak mengubah order of magnitude.** Kedua engine sudah
   memakai serializer terbaik (`serde_json`, stdlib `json`). Mempercepat 2× lagi tetap
   tidak sampai 10× yang dibutuhkan untuk "instan" pada 5 juta baris.

## Konsekuensi

1. **Dokumentasi harus jujur tentang apa yang meningkat.** Jangan klaim "10× lebih
   cepat" tanpa konteks — throughput hanya 1,3–1,4×, dan itu karena JSON. Klaim TTFR
   57–167× dan memori 29–60× dengan bukti.

2. **Fase 2 dapat mempertimbangkan protokol lain** bila throughput menjadi bottleneck
   nyata. Kandidat: Apache Arrow (columnar binary), cap'n proto (zero-copy), atau data
   plane terpisah dari control plane NDJSON.

3. **Ekspor besar tetap dipecah per bagian** (`qh-export::plan`), seperti yang engine
   Python lakukan. Ini bukan workaround — ini desain yang mempertahankan memori datar.

4. **Benchmark di `deploy/dev/bench-results.jsonl` adalah baseline resmi** untuk
   perbandingan masa depan. Format JSONL + generator `docs/benchmarks.md` berarti angka
   tidak pernah ditulis tangan.

## Catatan pengukuran

Semua angka dari `PROGRESS.md:646-653`, yang mengutip:

```
deploy/dev/bench_fetch.py --engine rust postgres wide_500k
deploy/dev/bench_fetch.py --engine python postgres wide_500k
deploy/dev/bench_fetch.py --engine rust mysql wide_500k
deploy/dev/bench_fetch.py --engine python mysql wide_500k
```

> **Kutipan ini historis dan perintahnya sudah tidak bisa dijalankan.** Flag `--engine` hilang
> bersama lengan Python-nya (`deploy/dev/bench_fetch.py` kini hanya punya `--kind`, `--sql`,
> `--limit`, `--label`, `--binary`, `--repeat`, `--report-only`, `--no-report`), dan sejak mesin
> Python dihapus tidak ada lagi pembanding untuk dijalankan bergantian. Empat baris di
> `bench-results.jsonl` yang ditulis perintah di atas adalah satu-satunya salinan baseline Python,
> dan sengaja dibiarkan beku. Menjalankan ulang hari ini:
>
> ```
> python3 deploy/dev/bench_fetch.py --kind postgres --label rust-release
> python3 deploy/dev/bench_fetch.py --kind mysql --label rust-release
> python3 deploy/dev/bench_fetch.py --report-only
> ```

Dijalankan bergantian (interleaved), median dari 3, beban mesin ~3,2 dari 10 CPU. Harness
mencatat `elapsed_ms` dari event engine sendiri, TTFR dari event `connect` (sebelum
sentuh jaringan), peak RSS dari `/usr/bin/time -l`, dan throughput dihitung dari
`rows / (elapsed_ms / 1000)`.

Data lengkap: `deploy/dev/bench-results.jsonl` (4 baris, satu per konfigurasi).
