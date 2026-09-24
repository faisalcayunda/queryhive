# 0006 — Client Trino ditulis sendiri di atas `reqwest`

- **Status:** Diterima
- **Tanggal:** sesi Fase 0
- **Konteks instruksi:** §5 poin 6

## Konteks

Trino **dulu** diakses lewat paket Python `trino==0.339.0` (`app/engine-requirements.txt`), dan
`exporter/source.py` bahkan memuat penanganan khusus untuk perilakunya: auto-upgrade HTTP→HTTPS
(`source.py:242`) dan `describe_error` (`source.py:64`) ada karena keanehan klien itu. Kedua berkas
itu **sudah tidak ada** — keputusan di ADR ini sudah dilaksanakan, dan client-nya sekarang
`crates/qh-driver-trino` (`CLIENT_CAPABILITIES` di `src/lib.rs`). Kutipan path di atas dibiarkan
sebagai catatan konteks saat keputusan diambil.

Protokol Trino bukan protokol SQL biner. Ia HTTP murni: `POST /v1/statement` → respons berisi
`nextUri` → `GET nextUri` berulang sampai selesai; cancel = `DELETE nextUri`; hasil di-decode dari
format page Trino (`/v1/statement` JSON dengan `columns` + `data`).

## Opsi yang dipertimbangkan

| Opsi | Kelebihan | Kekurangan |
|---|---|---|
| Crate `trino` (Rust) | Ada yang memelihara | Modelnya sinkron/blokir dan mengasumsikan penggunaan seperti SQL client biasa; menambah dependency besar untuk protokol yang sederhana |
| Crate `prusto` | Membungkus API Trino + `nextUri` | Cakupan & riwayat pemeliharaan tidak sebanding dengan risiko; tetap perlu adapter agar cocok dengan trait driver (§3.3) |
| **Implementasi sendiri di atas `reqwest`** | Tepat sebesar kebutuhan; cancel `DELETE` dan polling `nextUri` sepenuhnya terkendali; tidak ada dependency yang asumsinya bertentangan dengan §5 poin 6 | ~400 baris protokol menjadi tanggung jawab kami; setiap perubahan protokol Trino harus kami ikuti |

## Keputusan

Implementasi sendiri di `qh-driver-trino`, memakai `reqwest` (dengan `rustls`) sebagai transport.
Tidak memakai pool koneksi: Trino diperlakukan sebagai **driver tanpa koneksi persisten**, dan
`Capabilities::persistent_connection = false` menyatakannya.

## Alasan

1. **Justru inilah pasien yang membuat trait driver harus berbentuk seperti sekarang.** §5 poin 6
   melarang trait driver mengasumsikan semua driver punya koneksi TCP persisten atau pool. Memakai
   crate yang *mengasumsikan* hal itu akan mendorong asumsi itu naik ke abstraksi bersama, dan
   membatalkan alasan pembatasan itu ada.
2. **Permukaan yang dibutuhkan kecil dan terdefinisi**: connect/probe, eksekusi, streaming page,
   delete/cancel, plus query metadata (`SHOW CATALOGS`/`SHOW SCHEMAS`/`SHOW TABLES`) yang sudah
   jadi SQL biasa di `drivers.py`. Tidak ada kebutuhan transaksi, prepared statement, atau tipe
   biner kompleks seperti di PG/MySQL.
3. **Cancel-nya sederhana di Trino**: `DELETE nextUri` (blueprint §2.7). Membungkus ini dengan
   crate yang punya state machine sendiri justru menambah tempat bug bisa bersembunyi.

## Konsekuensi

- Diperlukan test integrasi terhadap container Trino nyata di CI untuk setiap versi di matriks
  (§7.6) — ini biaya yang mau tidak mau ada pada opsi mana pun, karena paritas perilaku harus
  dibuktikan.
- Penanganan khusus Python (`source.py:242`, `source.py:64`) **tidak** perlu dipindahkan: keduanya
  ada karena bug/keanehan klien Python, dan di sini tidak relevan. Ini dicatat di tabel pemetaan
  fitur (blueprint §1.7) sebagai perbaikan disengaja.
- Format page Trino adalah JSON, jadi decoding nilainya tetap melewati JSON — tetapi hanya untuk
  Trino, dan `qh_core::Value` yang dihasilkan tetap bertipe (DECIMAL presisi penuh tidak
  melewati `f64`).
- Bila di masa depan crate Trino Rust menjadi matang dan cocok dengan trait driver, `qh-driver-trino`
  dapat diganti tanpa mengubah driver lain maupun lapisan FFI/UI — itu alasan batasnya ditegakkan
  di crate ini saja.
