# 0004 — UniFFI untuk control plane, bukan data plane

- **Status:** Diterima
- **Tanggal:** sesi Fase 0
- **Konteks instruksi:** §8 Bagian 4, §7.4 (Swift 6 strict concurrency)

## Konteks

UI perlu memanggil Rust untuk connect, introspeksi, eksekusi, cancel, dan event — operasi kecil
dengan frekuensi rendah–sedang. Ia juga perlu mengambil isi grid, operasi besar dengan frekuensi
tinggi. Keduanya tidak punya tuntutan yang sama, dan memaksakan satu mekanisme untuk keduanya
membuat salah satunya buruk.

## Opsi yang dipertimbangkan

| Opsi | Kelebihan | Kekurangan |
|---|---|---|
| **UniFFI untuk keduanya** | Satu mekanisme, satu binding | Isi grid lewat `Vec<Vec<Option<String>>>` mengembalikan persis masalah P1 (blueprint §1.5) yang jadi alasan migrasi ini |
| C ABI manual untuk keduanya | Kontrol penuh, tanpa generator | Seluruh pemetaan `Result`→`throws`, `Sendable`, dan pemilik memori ditulis tangan; biaya besar dan sumber bug |
| `swift-bridge` | Sintaksis ergonomis | Ekosistem & riwayat produksi lebih kecil dari UniFFI; dukungan async terbatas |
| **UniFFI (control) + handle/offset (data)** | Masing-masing memakai mekanisme yang cocok | Dua jalur yang harus didokumentasikan dan diuji terpisah |

## Keputusan

**Control plane lewat UniFFI; data plane lewat handle + offset buffer, terpisah sepenuhnya.**
Untuk stream, pola utamanya adalah callback Rust → `AsyncThrowingStream` Swift (blueprint §4.3).

## Alasan

1. Control plane nilainya ada pada **pemetaan error dan tipe yang benar**: UniFFI mengubah
   `Result<T, EngineError>` menjadi `throws` dengan tipe error yang bisa dibaca Swift, dan menjaga
   tipe agar `Sendable` — persis syarat §7.4 dan §5 poin 4.
2. Data plane nilainya ada pada **tidak menyalin**. Untuk 30 kolom × 256 baris, biaya offset
   buffer adalah ~61 KB per page (blueprint §4.2), jauh di bawah biaya marshalling string per sel.
3. Nilai UniFFI terbesar justru pada hal yang tidak dibutuhkan data plane: menghasilkan binding
   untuk ratusan tipe kecil. Untuk satu panggilan `window(handle, start, count)` yang mengembalikan
   buffer, generator hanya menambah lapisan.

## Konsekuensi

- Binding Swift **di-commit** dan CI memverifikasi bahwa binding yang di-commit sama dengan hasil
  generate ulang, supaya tidak ada binding basi.
- Risiko kematangan dukungan async UniFFI dikelola dengan tidak menggantungkan diri padanya:
  stream memakai callback, dan `async fn` UniFFI dipakai hanya untuk operasi non-stream.
- Setiap entry point `#[uniffi::export]` wajib melewati firewall panic (ADR-0009).
- Bila terukur bahwa callback→`AsyncThrowingStream` bermasalah, jalur stream dapat dipindah ke
  channel UniFFI tanpa mengubah trait driver maupun protocol `DatabaseEngine` — itu alasan
  pemisahan ini dicatat sebagai ADR.
