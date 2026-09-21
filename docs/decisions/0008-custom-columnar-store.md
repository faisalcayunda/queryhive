# 0008 — Result store kolumnar kustom; Arrow C Data Interface sebagai opsi, bukan dasar

- **Status:** Diterima (opsi Arrow ditinjau ulang bila benchmark Fase 3 menunjukkan salinan offset
  adalah bottleneck)
- **Tanggal:** sesi Fase 0
- **Konteks instruksi:** §6 (memori, time-to-first-row), §8 Bagian 2

## Konteks

Isi grid harus berpindah dari Rust ke Swift tanpa menjadi masalah P1 yang justru jadi alasan
migrasi ini (blueprint §1.5). Ukuran yang harus dilayani: 500.000 baris × 30 kolom, dengan
kebutuhan **window akses acak** (satu page berisi baris ke-300.000 sampai ke-300.256), dan
kemampuan **spill ke disk** saat melewati ambang memori.

## Opsi yang dipertimbangkan

| Opsi | Akses window acak | Spill | FFI | Dependency |
|---|---|---|---|---|
| Apache Arrow (`arrow-rs`) | `RecordBatch` dioptimasi untuk scan kolumnar; `row(i)` tidak jadi fokus | Tidak ada bawaan | Arrow C Data Interface = ABI stabil, bisa dibaca Swift tanpa binding | ~60 crate transitif, termasuk `chrono`/`serde`/opsi Parquet |
| **Kolumnar kustom (`qh-result-store`)** | `batches[batch_idx]` + offset per kolom = O(1) | Bawaan: satu batch = satu rentang mmap | Handle + offset buffer (blueprint §4.2) | `memmap2` saja (opsional) |
| `Vec<Vec<Option<String>>>` lewat UniFFI | Mudah | Tidak ada | Salinan penuh + jutaan alokasi `String` | — |

## Keputusan

**Store kolumnar kustom.** Arrow C Data Interface dicatat sebagai jalur peningkatan (dan sebagai
jalur ekspor opsional nanti), bukan sebagai fondasi.

## Alasan

1. **Kebutuhan sebenarnya bukan format interchange.** Arrow dirancang untuk data analitik yang
   dibaca kolumnar-berurutan oleh konsumen seperti mesin query. Kebutuhan di sini adalah window
   acak per viewport scroll plus spill — dua hal yang justru tidak disediakan Arrow. Memilih Arrow
   berarti menulis lapisan spill sendiri **dan** tetap menulis layer window sendiri.
2. **Tipe internal kita lebih sempit.** `qh_core::Value` menyimpan DECIMAL sebagai `i128 + scale`
   (blueprint §3.4). Memetakan ke `Decimal128` Arrow lalu kembali untuk setiap window berarti satu
   konversi per page tanpa manfaat yang dibayar oleh konsumennya.
3. **Jumlah dependency.** ~60 crate transitif pada fondasi proses desktop berumur panjang bukan
   angka netral: itu waktu build, ukuran audit `cargo-deny`, dan permukaan supply-chain (§7.1).
4. **Yang dikorbankan eksplisit:** kami memiliki bug store sendiri. Itu dibayar dengan `proptest`
   pada konversi tipe dan windowing, `cargo-fuzz` pada decoder nilai, dan test unit yang mengikat
   invarian (jumlah baris per batch, batas window, perilaku generation counter).

## Konsekuensi

- `qh-result-store` menjadi crate yang **tidak boleh** melewati batas FFI secara langsung; ia
  diakses lewat handle (blueprint §2.6).
- Invarian yang wajib diuji: window di luar jumlah baris mengembalikan rentang kosong (bukan
  panic), handle basi mengembalikan `StaleHandle` (bukan use-after-free), dan spill tidak mengubah
  hasil pembacaan dibandingkan data di memori.
- File spill hidup di `~/Library/Caches/<app>/spill/`, dihapus pada `Drop`, dan file yatim disapu
  saat startup (crash dapat meninggalkannya).
- **Syarat revisi:** `tools/bench/ffi_window_bench.rs` mengukur biaya nyata window per page. Bila
  angka terukurnya jauh di atas perkiraan ~61 KB per page (blueprint §4.2), ADR ini dibuka kembali
  dan Arrow C Data Interface diuji sebagai jalur data plane.
