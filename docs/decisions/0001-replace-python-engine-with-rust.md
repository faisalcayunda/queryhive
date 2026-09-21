# 0001 — Mengganti engine Python dengan Rust secara langsung

- **Status:** Diterima
- **Tanggal:** sesi Fase 0
- **Konteks instruksi:** §5 poin 3, §4.1

## Konteks

Engine hari ini adalah proses Python terpisah yang dijalankan dengan `Process`
(`app/Sources/TrinoExporter/Support/Engine.swift:30`) dan berbicara NDJSON di atas stdout.
Masalah utama yang dirasakan pengguna adalah fetch hasil query yang lambat. Diagnosis di
blueprint §1.5 menemukan lima penyebab (P1–P5), dan **tidak satu pun** bisa dihilangkan dengan
optimasi in-place di Python: P1 adalah biaya struktural protokol teks dua proses, P2 dan P3 ada
di sisi SwiftUI, P4 tidak punya mekanisme spill, P5 tidak punya kanal cancel.

## Opsi yang dipertimbangkan

| Opsi | Kelebihan | Kekurangan |
|---|---|---|
| A. Optimasi Python (orjson, batching, buffer biner) | Tidak menyentuh UI; bisa dikejar bertahap | Menyisakan batas proses, dua runtime, dua bahasa, dan P2/P3 tetap ada karena ada di Swift |
| B. Strangler: jalankan Rust dan Python berdampingan di belakang satu flag | Rilis lebih kecil risikonya; rollback mudah | Dua engine dipelihara selama transisi; permukaan FFI dan protokol ikut terduplikasi; satu engineer tidak sanggup memikul dua jalur |
| C. **Replace langsung ke Rust** | Satu engine, satu runtime; P1–P5 dihilangkan sekaligus; permukaan FFI dirancang sekali | Ada jendela di mana UI belum berjalan penuh di atas Rust (Fase 1) |

## Keputusan

**Opsi C.** Migrasi replace langsung di branch `feat/rust-engine`. UI SwiftUI dipertahankan.
Engine Python dipertahankan **hanya** untuk merekam golden snapshot sebagai acuan paritas, dan
commit terakhir yang masih memuatnya diberi tag `python-engine-final` sebelum dihapus.

## Alasan

1. Penyebab kelambatan bersifat arsitektural, bukan kekurangan implementasi (blueprint §1.5).
   Memperbaiki P1 saja tidak mengubah P2–P5, yang justru yang membuat scroll 500k baris mustahil.
2. Strangler memerlukan kedua engine hidup berdampingan, dan itu berarti memelihara protokol
   NDJSON **dan** FFI sekaligus — beban yang tidak realistis untuk 1 engineer (§3).
3. Golden snapshot memberi jaring pengaman yang setara dengan feature flag untuk hal yang
   sebenarnya penting: **paritas perilaku**, bukan paritas rilis. Snapshot direkam dari engine
   Python sebelum ia dihapus, sehingga regresi tetap tertangkap.
4. Jendela Fase 1 dikelola dengan CLI NDJSON di sisi Rust: seluruh harness golden bekerja sebelum
   FFI ada, sehingga Fase 1 dapat diselesaikan dan diuji tanpa menyentuh UI.

## Konsekuensi

**Positif:** satu runtime; tidak ada lagi spawn proses per operasi; session hidup lintas operasi
manual; cancel sungguhan dapat diimplementasikan; memori datar lewat spill.

**Negatif / harus dikelola:**
- Tidak ada rollback rilis sebagian; satu-satunya rollback adalah kembali ke tag
  `python-engine-final`. Ini diterima karena aplikasi desktop ini belum punya basis pengguna yang
  harus dijaga selama migrasi.
- Migrasi data pengguna (koneksi, history, saved query) menjadi wajib, idempoten, dengan backup
  dan rollback (§7.2) — biaya yang tidak akan ada pada opsi A.
- Selama Fase 1, build aplikasi masih memuat engine Python; ukuran bundel tidak turun sampai Fase 2.
