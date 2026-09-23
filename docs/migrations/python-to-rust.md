# Migrasi dari penyimpanan versi lama

Dokumen ini adalah spesifikasi impor `connections.json` + item Keychain lama ke `qh-storage` dan
`qh-credentials`. Blueprint merujuk berkas ini di §3.5 dan §7.2; yang dijelaskan di sini adalah apa
yang **benar-benar diimplementasikan** di `crates/qh-storage/src/import.rs`, bukan rencana.

## 1. Apa yang dimigrasikan, dan dari mana

| Sumber | Isi | Tujuan |
|---|---|---|
| `~/Library/Application Support/QueryHive/connections.json` | Array JSON koneksi tersimpan | tabel `connection` |
| Item Keychain `kSecClassGenericPassword`, service `id.data-ecosystem.queryhive`, account = UUID koneksi | Satu password per koneksi | **tidak dipindahkan.** `qh-credentials` membaca item yang sama, di tempat yang sama |

Dua hal yang perlu ditegaskan lebih dulu, karena keduanya mengubah bentuk pekerjaan ini:

1. **Berkas ini bukan peninggalan era Python saja.** Aplikasi Swift versi sekarang masih
   menulis dan membacanya (`app/Sources/TrinoExporter/Models/Connections.swift:319`). Jadi
   kontrak yang harus dibaca adalah decoder aplikasi hari ini, bukan tebakan tentang apa yang
   dulu ditulis skrip Python.
2. **Password tidak ikut pindah.** Nama service Keychain dipertahankan justru supaya tidak perlu
   pindah: item yang sudah ada tetap ditemukan di tempatnya. Yang berubah hanya di mana *daftar
   koneksinya* disimpan.

## 2. Pemetaan bidang

Setiap nilai bawaan di bawah dibaca dari decoder Swift (`Connections.swift:189-210`), bukan
dipilih di sini. Kolom "bawaan" berarti "nilai yang dipakai bila kunci itu tidak ada di berkas".

| Bidang JSON | Kolom / tujuan | Bawaan |
|---|---|---|
| `id` (UUID, wajib) | `id` — UUID huruf kecil, satu ejaan | — (baris dilewati bila tidak ada/bukan UUID) |
| `name` (wajib) | `name` | — (baris dilewati bila tidak ada) |
| `kind` | `kind` | `trino` |
| `host` | `host` — string kosong menjadi `NULL` | `NULL` |
| `port` | `port` | port bawaan per kind: 8080 / 5432 / 3306 |
| `user` | `user_name` — string kosong menjadi `NULL` | `NULL` |
| `database` → `catalog` | `database_name` — string kosong menjadi `NULL` | `NULL` |
| `color` | `options_json.color` | `blue` |
| `scheme` → `httpScheme` | `options_json.scheme` | `https` |
| `sslmode` | `options_json.sslmode` | `""` (bawaan per kind dihitung saat dipakai, tidak disimpan) |
| `schema` | `options_json.schema` | `""` |
| `verify` | `options_json.verify` | `true` |
| `showAllSchemas` | `options_json.showAllSchemas` | `false` |
| — (urutan array) | `sort_order` | indeks baris, mulai 0 |
| — | `secret_ref` | `id` baris itu, dalam ejaan kanoniknya (huruf kecil) |
| — | `is_production`, `is_read_only`, `group_id` | `0`, `0`, `NULL` |
| — | `updated_at`, `version` | waktu impor, `1` |

`scheme` dan `sslmode` disimpan apa adanya meskipun kosong: keduanya bidang yang *dipakai* per
kind, dan bawaan `prefer`/`disable` dihitung saat koneksi dipakai, bukan disimpan. Menuliskan
bawaan itu ke baris justru akan membekukan keputusan yang hari ini dihitung ulang.

Dua nama lama dibaca dan **tidak pernah ditulis**: `httpScheme` dan `catalog`. Keduanya dibaca
dengan aturan "nama sekarang dulu, lalu nama lama" — sama seperti decoder aplikasi, yang membaca
dua kontainer terpisah, sehingga berkas yang memuat keduanya tetap terbaca alih-alih ditolak.

## 3. Empat perbedaan yang disengaja terhadap decoder aplikasi

Ini dicatat karena masing-masingnya akan terlihat sebagai "kok beda?" oleh yang membandingkan:

1. **Baris yang tidak terbaca dilewati dan dilaporkan, bukan mematikan seluruh impor.** Decoder
   aplikasi melempar kesalahan untuk seluruh array, sehingga satu baris rusak membuat aplikasi
   memindahkan berkasnya dan mulai dari daftar kosong. Di sini baris lain tetap diimpor dan yang
   gagal masuk `ImportReport.skipped` beserta indeksnya di berkas. Sembilan belas koneksi dan satu
   salah ketik seharusnya tetap sembilan belas.
2. **Urutan array menjadi `sort_order`.** Berkas itu tidak punya bidang urutan, dan urutan
   penulisannya adalah urutan sidebar. Mengabaikannya berarti mengurutkan ulang sidebar setiap
   pengguna secara alfabetis.
3. **`secret_ref` diisi untuk setiap baris.** Nilainya adalah UUID koneksi itu sendiri — persis
   account yang dipakai aplikasi untuk mencari password. Referensi ini **diturunkan**, bukan
   hasil bertanya ke Keychain; bertanya berarti memunculkan dialog izin pada setiap impor.
   `qh-credentials` melipat account berbentuk UUID menjadi huruf besar sebelum dipakai, jadi
   ejaan huruf kecil di kolom ini menemukan item yang ditulis aplikasi.
4. **`host`/`user`/`database` yang kosong menjadi `NULL`.** SQL punya nilai untuk "tidak diisi",
   dan string kosong akan menjadi cara kedua mengatakan hal yang sama; bagi aplikasi pun keduanya
   sudah berarti sama.

## 4. Jaminan operasinya

Urutannya tetap, dan setiap langkah punya alasannya:

1. **Berhenti bila sudah pernah.** Bila `legacy_import` sudah punya baris untuk sumber ini,
   impor mengembalikan laporan `already_imported` **tanpa menulis apa pun dan tanpa membuat
   cadangan**. Ini yang membuat impor aman dijalankan setiap kali aplikasi dibuka.
2. **Salin berkasnya dulu.** Tujuan: `connections.json.before-import-<millis>`, di sebelah
   berkas aslinya — supaya pengguna yang harus memulihkan secara manual menemukannya di tempat
   yang wajar, bukan di direktori sementara yang harus diberitahukan. **Bila cadangan gagal,
   impor tidak dimulai**: berkas itu satu-satunya salinan dari apa yang sedang diimpor.
3. **Tulis barisnya** lewat `merge_connection`, yaitu aturan `qh-sync`: revisi lebih tinggi
   menang, dan baris yang lebih baru di database **tidak** ditimpa oleh berkas yang lebih tua.
   Laporan memisahkan `written` dari `kept`.
4. **Baca kembali dan bandingkan bidang demi bidang.** Baris yang ditulis harus sama persis
   dengan yang dimaksud; baris yang **dipertahankan** hanya diperiksa keberadaannya — membandingkannya
   dengan salinan dari berkas akan melaporkan keputusan merge itu sendiri sebagai kegagalan (dan
   memang begitu pada versi pertama kode ini; ujilah yang menemukannya).
5. **Tandai selesai hanya setelah verifikasi lolos.** Baris `legacy_import` ditulis di akhir.
   Impor yang gagal verifikasi tidak meninggalkan penanda, sehingga menjalankannya lagi adalah
   percobaan ulangnya.

## 5. Yang belum dicakup

- **`query_history`, `saved_query`, `session_restore`.** Berkas lama tidak memuatnya; riwayat
  query era Python tidak pernah dipersistensi ke berkas ini. Kalau ternyata ada di suatu tempat,
  itu impor terpisah dengan verifikasinya sendiri.
- **Aplikasinya belum memanggilnya.** Sambungan engine-nya sudah ada: `qh-ffi` bergantung pada
  `qh-storage` dan menyediakan perintah `import_connections` (`crates/qh-ffi/src/lib.rs`, yang
  menjalankan `crates/qh-ffi/src/local.rs`), jadi impor ini bisa dipanggil dari CLI hari ini.
  Yang belum: aplikasi Swift memanggilnya pada peluncuran — itu langkah UI berikutnya, dan sifat
  idempoten di atas yang membuatnya aman dijalankan tiap peluncuran. Yang ada sekarang: fungsinya,
  jaminannya, dan 11 uji terhadap berkas sungguhan di `crates/qh-storage/tests/import.rs`.
- **Berkas yang rusak.** Aplikasi memindahkan `connections.json` yang tidak bisa di-decode ke
  `connections.json.broken-<detik>`; importer ini tidak menyentuh berkas sama sekali (selain
  menyalinnya), jadi keduanya tidak saling berebut berkas yang sama. Bila berkasnya tidak bisa
  di-parse, importer mengembalikan `Malformed` dan **tidak menulis apa pun**.
