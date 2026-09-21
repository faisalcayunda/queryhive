# Benchmarks

> Setiap angka di berkas ini dihasilkan oleh `deploy/dev/bench_fetch.py`; tidak ada yang
> ditulis dengan tangan. Hasil mentah ada di `deploy/dev/bench-results.jsonl`.
> Kebijakan: angka yang belum diukur ditulis sebagai **[belum diukur]**, bukan diperkirakan.

## Cara mengukur

```bash
deploy/dev/up.sh all
python3 deploy/dev/bench_fetch.py --kind postgres --label baseline-python
python3 deploy/dev/bench_fetch.py --kind mysql    --label baseline-python
python3 deploy/dev/bench_fetch.py --report-only
```

Engine dijalankan sebagai proses anak persis seperti aplikasi menjalankannya, setiap baris
stdout-nya diberi cap waktu saat tiba. Dua angka time-to-first-row dilaporkan karena
keduanya berguna dan hanya salah satunya cocok untuk target §6:

- **dari connect** — jarak dari event `step connect` ke event `rows` pertama. Engine
  mengirim `step connect` sebelum menyentuh jaringan, jadi ini connect + submit + halaman
  pertama. Inilah yang dibandingkan dengan target §6 (< 200 ms sejak server mulai
  mengirim hasil).
- **dari start** — jarak dari proses dijalankan. Ini yang dirasakan pengguna, termasuk
  waktu start interpreter.

Menganchor pada event `columns` akan salah: event itu baru muncul setelah halaman pertama
sudah ada, sehingga selisihnya hampir nol dan menyembunyikan seluruh waktu tunggu.

**Throughput** dihitung antara baris pertama dan baris terakhir, sehingga waktu start dan
connect tidak ikut dihitung sebagai kecepatan fetch. **Peak RSS** dibaca dari
`/usr/bin/time -l`, yang melaporkan puncak satu proses anak, bukan angka kumulatif.

Kondisi uji: `SELECT * FROM wide_500k` (30 kolom), tanpa retry, database lokal di container.

## Baseline engine Python

| Kind | Baris | Baris pertama (dari connect) | Baris pertama (dari start) | Total | Fetch | Throughput | Peak RSS | Python |
|---|---|---|---|---|---|---|---|---|
| postgres | 500,000 | 722 ms | 871 ms | 4,234 ms | 3,340 ms | 149,687 baris/s | 546 MB | 3.12.12 |
| mysql | 500,000 | 3,193 ms | 3,261 ms | 6,687 ms | 3,415 ms | 146,423 baris/s | 1,080 MB | 3.12.12 |

## Engine Rust

**[belum diukur]** — engine Rust belum punya CLI yang setara `preview`, jadi belum ada yang bisa diukur.

## Perbandingan dengan target §6

| Metrik | Target | Baseline Python | Rust | Status |
|---|---|---|---|---|
| Throughput fetch | ≥ 5× baseline Python | 149,687 baris/s | [belum diukur] | Menunggu engine Rust |
| Time-to-first-row | < 200 ms sejak server mulai mengirim hasil | 722 ms | [belum diukur] | Menunggu engine Rust |
| Memori proses (500k × 30) | < 800 MB | 546 MB | [belum diukur] | Menunggu engine Rust |
| Scroll grid 60 fps | — | — | — | [belum diukur] |
| Cold start < 1 dtk | — | — | — | [belum diukur] |
| Introspeksi 5.000 tabel < 1 dtk | — | — | — | [belum diukur] |
| Pembatalan < 500 ms | — | — | — | [belum diukur] |
| Nol leak lintas FFI | — | — | — | [belum diukur] |

## Temuan dari baseline

- **postgres: baris pertama datang 722 ms setelah connect**, sementara targetnya < 200 ms. Engine menunggu halaman pertama utuh sebelum mengirim apa pun, jadi target ini tidak bisa dicapai tanpa streaming per halaman.
- postgres: 149,687 baris/s adalah **angka dasar yang harus dilampaui 5×** menurut §6, jadi target absolutnya sekitar 748,435 baris/s.
- **mysql: memori sudah melewati target §6 sekarang.** 1,080 MB untuk 500k × 30, sedangkan targetnya < 800 MB. Ini bukan regresi yang diperkenalkan Rust; ini batas engine Python, dan salah satu alasan store Rust memakai encoding kolumnar dengan spill.
- **mysql: baris pertama datang 3,193 ms setelah connect**, sementara targetnya < 200 ms. Engine menunggu halaman pertama utuh sebelum mengirim apa pun, jadi target ini tidak bisa dicapai tanpa streaming per halaman.
- mysql: 146,423 baris/s adalah **angka dasar yang harus dilampaui 5×** menurut §6, jadi target absolutnya sekitar 732,115 baris/s.

