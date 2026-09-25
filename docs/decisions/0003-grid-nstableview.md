# 0003 — Grid hasil memakai NSTableView, bukan SwiftUI Table atau grid kustom

- **Status:** Diterima (wajib diukur ulang pada Fase 3; bila prototipe gagal, ADR ini direvisi)
- **Tanggal:** sesi Fase 0
- **Konteks instruksi:** §6 (scroll 60/120 fps), §7.7 (aksesibilitas), §5 poin 2 (UI dipertahankan)

## Konteks

Grid hasil hari ini adalah `SwiftUI` `ScrollView` + `LazyVStack` + `ForEach`
(`app/Sources/TrinoExporter/Views/ResultGrid.swift:106`), dan `Array(filteredRows.enumerated())`
di dalam `body` mematerialisasi seluruh koleksi pada setiap evaluasi render (blueprint §1.5 P2).
Target §6 adalah scroll stabil pada 500.000 baris × 30 kolom, dengan VoiceOver dan navigasi
keyboard (§7.7) yang harus tetap bekerja.

## Opsi yang dipertimbangkan

| Opsi | Virtualisasi baris | Virtualisasi kolom | Seleksi sel/rentang | Edit inline | Aksesibilitas | Usaha |
|---|---|---|---|---|---|---|
| SwiftUI `Table` | Ya | **Tidak** — 30 kolom dibangun meski terlihat 5 | baris; sel perlu kerja manual | sulit | otomatis, granularitas sel buruk | S |
| **`NSTableView` via `NSViewRepresentable`** | Ya | Ya | bawaan | `NSTextField` reuse | peran AppKit penuh (`AXTable`/`AXRow`/`AXCell`) | M |
| Grid kustom (Core Animation/Metal) | Ya | Ya | ditulis sendiri | ditulis sendiri | **ditulis sendiri** | L |

## Keputusan

**`NSTableView` lewat `NSViewRepresentable`.**

## Alasan

Yang menentukan bukan performa mentah — ketiganya bisa mencapai target bila virtualisasinya
benar — melainkan bahwa `NSTableView` adalah satu-satunya opsi yang memberi **virtualisasi kolom,
reuse sel untuk edit, dan aksesibilitas lengkap** tanpa menulisnya sendiri. Grid kustom
memindahkan seluruh permukaan aksesibilitas (§7.7) dan navigasi keyboard menjadi kode yang harus
kami tulis dan uji; itu risiko besar untuk keuntungan yang belum terbukti. `SwiftUI Table` gagal
pada virtualisasi kolom, dan justru 30 kolom × 30 `Text` per baris itulah biaya yang dominan.

## Konsekuensi

- Ditambahkan satu `NSViewRepresentable` ke codebase SwiftUI. Ini **penambahan**, bukan
  penggantian alur: toolbar, footer, context menu, dan pintasan tetap sama (§5 poin 2).
- Semua yang sudah ada harus tetap ada sesudahnya; daftar pemetaannya ada di blueprint §2.5
  (seleksi, copy TSV/CSV/JSON/SQL INSERT, edit inline, sort/filter dua bentuk, resize & reorder,
  keyboard, VoiceOver).
- Sort dan filter dikerjakan di Rust (`qh-result-store`), bukan di Swift, supaya 500k baris tidak
  pernah disalin ke sisi Swift.
- **Syarat revisi:** prototipe `tests/bench_grid.swift` diukur pada Fase 3 sebelum integrasi penuh.
  Bila `NSTableView` tidak mencapai 60 fps pada 500k × 30, ADR ini dibuka kembali dan grid kustom
  (opsi 3) dipertimbangkan dengan angka di tangan.
