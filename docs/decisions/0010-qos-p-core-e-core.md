# 0010 — Pemetaan QoS eksplisit ke P-core dan E-core

- **Status:** Diterima
- **Tanggal:** sesi Fase 0
- **Konteks instruksi:** §5 poin 1 (manfaatkan karakteristik Apple Silicon), §5 poin 4

## Konteks

§5 poin 1 meminta pemanfaatan eksplisit karakteristik Apple Silicon, menyebut pembagian P-core/E-core
melalui QoS class. Yang perlu diputuskan: pekerjaan mana yang mendapat kelas mana, dan berapa
worker thread yang dipakai runtime tokio.

Bahaya yang nyata: `std::thread::available_parallelism()` mengembalikan **jumlah total** core
(termasuk E-core) dan tidak membedakan keduanya. Runtime yang mengalokasikan worker sejumlah angka
itu akan menaruh pekerjaan interaktif di E-core, dan itu langsung terasa sebagai lag pada momen
yang justru paling terlihat — scroll dan fetch pertama.

## Opsi yang dipertimbangkan

| Opsi | Kelebihan | Kekurangan |
|---|---|---|
| Biarkan OS memutuskan (satu pool, QoS default `USER_INITIATED`) | Nol kode tambahan | macOS menaikkan QoS thread saat dibangunkan dari MainActor, sehingga pekerjaan fetch juga dapat E-core saat sistem sibuk; tidak ada kontrol |
| `available_parallelism()` worker, tanpa QoS | Sederhana | Melebihkan jumlah worker (menghitung E-core sebagai setara), dan mengabaikan kelas |
| **Worker = jumlah P-core untuk fetch/scroll; pool terpisah ber-QoS `UTILITY` untuk indeks metadata; `BACKGROUND` untuk spill** | Pekerjaan yang menunggu terlihat pengguna tetap di core cepat; pekerjaan latar tidak merebut P-core | Satu blok `unsafe` (pengaturan QoS) dan satu helper `qh-rt`; jumlah core harus dibaca dari `sysctl`, bukan dari API standar |

## Keputusan

`qh-rt` membaca `hw.perflevel0.physicalcpu` (P-core) dan `hw.perflevel1.physicalcpu` (E-core) lewat
`sysctlbyname`, lalu:

- runtime utama tokio: `worker_threads = P_core`, thread di-set ke `QOS_CLASS_USER_INITIATED`;
- pool `spawn_blocking` untuk indeks metadata/autocomplete: jumlah `E_core`, QoS `UTILITY`;
- pekerjaan spill/kompaksi/diagnostics: QoS `BACKGROUND`.

Seluruh sentuhan QoS berada di satu tempat: helper `qh_rt::spawn_at(qos, fut)`, dengan
`// SAFETY:` pada blok `unsafe` yang memanggil `pthread_set_qos_class_self_np`.

## Alasan

Karena QoS adalah mekanisme **satu-satunya** di macOS untuk memilih jenis core, dan karena
pekerjaan di aplikasi ini terbagi jelas antara "menunggu terlihat pengguna" (connect, execute,
fetch, scroll) dan "boleh lambat" (indeks metadata, spill), pemisahan ini dapat dilakukan dengan
sedikit kode dan memberi hasil yang dapat diukur. Membiarkan default berarti macOST — bukan kita —
yang memutuskan, dan itu bertentangan dengan permintaan §5 poin 1 untuk memanfaatkannya **secara
eksplisit**.

## Konsekuensi

- `qh-rt` menjadi satu-satunya crate yang menyentuh API sistem untuk penjadwalan, sehingga aturan
  `#![forbid(unsafe_code)]` (§7.4) tetap berlaku di crate lain, dan satu blok `unsafe` itu punya
  komentar `// SAFETY:` yang menjelaskan invariant-nya.
- Jumlah worker **tidak** boleh di-hardcode; ia dibaca saat startup. Bila `sysctl` gagal (nama kunci
  berubah pada macOS mendatang), `qh-rt` jatuh ke `available_parallelism()`, dan **penurunan ini
  dicatat sebagai warning pada log** — bukan diam-diam, sesuai larangan fallback senyap (§4.4).
- Efektivitasnya wajib diukur, bukan diklaim: `os_signpost` menandai setiap span dengan kelas QoS,
  dan Instruments (Core ML/Time Profiler) dipakai memverifikasi bahwa worker fetch benar-benar
  mendarat di P-core. Hasilnya dicatat di `docs/benchmarks.md`.
- Karena pilihan ini bergantung pada kunci `sysctl` yang spesifik, ia ditambahkan ke daftar yang
  diuji saat naik versi macOS mayor — satu test yang memverifikasi bahwa jumlah P-core/E-core yang
  dibaca masuk akal (positif dan jumlahnya sama dengan total core).
