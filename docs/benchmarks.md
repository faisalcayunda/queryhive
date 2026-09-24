# Benchmarks

> Setiap angka di berkas ini dihasilkan oleh `deploy/dev/bench_fetch.py`; tidak ada yang
> ditulis dengan tangan. Hasil mentah ada di `deploy/dev/bench-results.jsonl`.
> Kebijakan: angka yang belum diukur ditulis sebagai **[belum diukur]**, bukan diperkirakan.

## Cara mengukur

```bash
deploy/dev/up.sh all
cargo build --release --bin queryhive-engine
python3 deploy/dev/bench_fetch.py --kind postgres --label <sesi> --repeat 3
python3 deploy/dev/bench_fetch.py --kind mysql    --label <sesi> --repeat 3
python3 deploy/dev/bench_fetch.py --report-only
```

Baseline Python di bawah ini adalah **rekaman beku**: barisnya sudah ada di
`deploy/dev/bench-results.jsonl` sebelum mesin Python dihapus dari pohon, dan tidak bisa
direkam ulang. Harness hanya bisa menjalankan engine Rust.

Harness menjalankan engine lewat satu jalur: perintah `preview` pada CLI `qh-ffi`, dengan
nama setelan yang sama dibaca dari environment. Setiap baris stdout-nya diberi cap waktu saat
tiba. Dua angka time-to-first-row dilaporkan karena keduanya berguna dan hanya salah satunya
cocok untuk target §6:

- **dari connect** — jarak dari event `step connect` ke event `rows` pertama. Engine
  mengirim `step connect` sebelum menyentuh jaringan, jadi ini connect + submit + halaman
  pertama. Inilah yang dibandingkan dengan target §6 (< 200 ms sejak server mulai
  mengirim hasil).
- **dari start** — jarak dari proses dijalankan. Ini yang dirasakan pengguna, termasuk
  waktu start interpreter.

Menganchor pada event `columns` akan salah: event itu baru muncul setelah halaman pertama
sudah ada, sehingga selisihnya hampir nol dan menyembunyikan seluruh waktu tunggu.

**elapsed_ms** adalah angka engine sendiri, diambil dari event `done`. Engine menstempelnya
tepat sebelum mengirim `step connect`, jadi cakupannya sama di setiap baris rekaman
(connect + fetch + emit) dan tidak memuat waktu start proses — inilah satu-satunya angka
yang mengukur rentang identik tanpa penjadwalan harness di tengahnya. **Throughput**
dihitung antara baris pertama dan baris terakhir, sehingga waktu start dan connect tidak
ikut dihitung sebagai kecepatan fetch. **Peak RSS** dibaca dari `/usr/bin/time -l`, yang
melaporkan puncak satu proses anak, bukan angka kumulatif.

**Rata-rata beban mesin** dicatat pada setiap run (`load_avg_1m_before`/`_after`): angka
yang diambil saat mesin sibuk menggambarkan mesinnya, bukan engine-nya.

Kondisi uji: `SELECT * FROM wide_500k` (30 kolom), tanpa retry, database lokal di container.
Trino tidak diukur: katalog `memory` di container dev tidak punya `wide_500k`.

## Baseline engine Python (rekaman beku)

| Kind | Label | Build | n | Baris | Baris pertama (dari connect) | Baris pertama (dari start) | Total (proses) | elapsed_ms (engine) | Fetch | Throughput | Peak RSS | load 1m |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| postgres | baseline-python | — | 1 | 500,000 | 722 ms | 871 ms | 4,234 ms | — | 3,340 ms | 149,687 baris/s | 546 MB | — |
| mysql | baseline-python | — | 1 | 500,000 | 3,193 ms | 3,261 ms | 6,687 ms | — | 3,415 ms | 146,423 baris/s | 1,080 MB | — |
| postgres | py-20260923 | — | 3 | 500,000 | 624 ms [602 ms–632 ms] | 695 ms [666 ms–701 ms] | 4,327 ms [4,304 ms–4,359 ms] | 4,236 ms [4,217 ms–4,276 ms] | 3,613 ms [3,586 ms–3,674 ms] | 138,405 [136,087–139,445] baris/s | 545 MB [545 MB–545 MB] | 3.16 [3.13–3.24] |
| mysql | py-20260923 | — | 3 | 500,000 | 3,299 ms [3,257 ms–3,407 ms] | 3,365 ms [3,325 ms–3,473 ms] | 6,981 ms [6,809 ms–7,229 ms] | 6,901 ms [6,732 ms–7,148 ms] | 3,645 ms [3,434 ms–3,742 ms] | 137,177 [133,617–145,604] baris/s | 1,093 MB [1,093 MB–1,094 MB] | 3.14 [3.04–3.46] |

Setiap sel adalah **median [min–max]** dari n repeat; satu angka saja berarti semua repeat sepakat. Statistik yang dipakai median, bukan yang tercepat: pada mesin yang dipakai bersama, satu run yang kebetulan sepi bukan kecepatan engine.

## Engine Rust

| Kind | Label | Build | n | Baris | Baris pertama (dari connect) | Baris pertama (dari start) | Total (proses) | elapsed_ms (engine) | Fetch | Throughput | Peak RSS | load 1m |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| postgres | rust-release-20260923 | release | 3 | 500,000 | 11 ms [9 ms–13 ms] | 16 ms [15 ms–22 ms] | 2,637 ms [2,618 ms–2,644 ms] | 2,626 ms [2,610 ms–2,637 ms] | 2,614 ms [2,602 ms–2,627 ms] | 191,251 [190,341–192,173] baris/s | 9 MB [9 MB–9 MB] | 3.26 [3.05–3.42] |
| mysql | rust-release-20260923 | release | 3 | 500,000 | 20 ms [12 ms–26 ms] | 26 ms [18 ms–32 ms] | 2,838 ms [2,790 ms–2,847 ms] | 2,830 ms [2,782 ms–2,840 ms] | 2,811 ms [2,771 ms–2,815 ms] | 177,849 [177,639–180,430] baris/s | 38 MB [35 MB–39 MB] | 3.26 [3.13–3.39] |

Setiap sel adalah **median [min–max]** dari n repeat; satu angka saja berarti semua repeat sepakat. Statistik yang dipakai median, bukan yang tercepat: pada mesin yang dipakai bersama, satu run yang kebetulan sepi bukan kecepatan engine.

## Perbandingan dengan target §6

| Metrik | Target | Baseline Python | Rust | Status |
|---|---|---|---|---|
| Throughput fetch | ≥ 5× baseline Python | 138,405 [136,087–139,445] baris/s | 191,251 baris/s (median) | Belum memenuhi — 1.38× baseline Python |
| Time-to-first-row | < 200 ms sejak server mulai mengirim hasil | 624 ms [602 ms–632 ms] | 11 ms | Memenuhi — 11 ms |
| Memori proses (500k × 30) | < 800 MB | 545 MB [545 MB–545 MB] | 9 MB | Memenuhi — 9 MB |
| Scroll grid 60 fps | — | — | — | [belum diukur] |
| Cold start < 1 dtk | — | — | — | [belum diukur] |
| Introspeksi 5.000 tabel < 1 dtk | — | — | — | [belum diukur] |
| Pembatalan < 500 ms | — | — | — | [belum diukur] |
| Nol leak lintas FFI | — | — | — | [belum diukur] |

## Ongkos `panic = "unwind"` pada ukuran artefak

ADR-0009 memilih `panic = "unwind"` supaya panic dari data server tidak menjatuhkan aplikasi, dan
konsekuensinya dijanjikan dicatat sebagai angka. Ini angkanya, diukur 24 Sep 2026 dengan
membangun `qh-ffi` dua kali dan mengganti satu baris di `Cargo.toml` (`[profile.release]`).
Profil lain identik: `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`.

| Artefak | `panic = "unwind"` | `panic = "abort"` | Selisih |
|---|---|---|---|
| `libqh_ffi.a` (arsip yang ditautkan app) | 131,52 MiB | 121,59 MiB | **9,93 MiB** |
| `libqh_ffi.dylib` (dibaca generator) | 9,54 MiB | 8,15 MiB | **1,39 MiB** |
| `QueryHive` (binary app yang dikirim) | 16,32 MiB | 14,63 MiB | **1,69 MiB** |

Yang menentukan bagi pengguna adalah baris terakhir: **unwinding table berbiaya 1,69 MiB**, sekitar
10% dari binary app. Baris arsipnya jauh lebih besar dan sama sekali tidak relevan untuk distribusi,
sebab app tidak pernah mengirim arsip itu — kode yang dipakai disalin ke binary app, dan sisanya
dibuang. Angka 9,93 MiB itu adalah selisih blob mentah, bukan ukuran yang sampai ke siapa pun.

Catatan cara mengukurnya: tiga angka di atas diambil dari build yang sama sekali berbeda (bukan
inkremental) untuk kedua nilai, karena `panic` mengubah seluruh graf. Setelah pengukuran, `Cargo.toml`
dikembalikan ke `unwind` dan `app/build.sh` dijalankan ulang, jadi binary di `app/dist/` cocok
dengan profil yang berlaku.

## Temuan

- **python/postgres (baseline-python, n=1): baris pertama 722 ms setelah connect**, sementara targetnya < 200 ms.
- python/postgres (baseline-python, n=1): 149,687 baris/s (median); target §6 berarti ≥ 748,435 baris/s.
- **python/mysql (baseline-python, n=1): memori melewati target §6.** 1,080 MB untuk 500k × 30, sedangkan targetnya < 800 MB.
- **python/mysql (baseline-python, n=1): baris pertama 3,193 ms setelah connect**, sementara targetnya < 200 ms.
- python/mysql (baseline-python, n=1): 146,423 baris/s (median); target §6 berarti ≥ 732,115 baris/s.
- **python/postgres (py-20260923, n=3): baris pertama 624 ms setelah connect**, sementara targetnya < 200 ms.
- python/postgres (py-20260923, n=3): 138,405 baris/s (median); target §6 berarti ≥ 692,025 baris/s.
- **python/mysql (py-20260923, n=3): memori melewati target §6.** 1,093 MB untuk 500k × 30, sedangkan targetnya < 800 MB.
- **python/mysql (py-20260923, n=3): baris pertama 3,299 ms setelah connect**, sementara targetnya < 200 ms.
- python/mysql (py-20260923, n=3): 137,177 baris/s (median); target §6 berarti ≥ 685,885 baris/s.
- rust/postgres (rust-release-20260923, release, n=3): 191,251 baris/s (median); target §6 berarti ≥ 956,255 baris/s.
- rust/mysql (rust-release-20260923, release, n=3): 177,849 baris/s (median); target §6 berarti ≥ 889,245 baris/s.

