# 0015 — MCP sebagai binari terpisah, token sebagai hash dengan scope tool dan allowlist koneksi

- **Status:** Diterima
- **Tanggal:** 29 Sep 2026 (Fase 2)
- **Konteks instruksi:** `docs/architecture/tablepro-adoption-plan.md` §1 Fase 2, §4, §10

## Konteks

Fase 2 membuka server MCP read-only. Bentuknya sudah ditentukan rencana: executable terpisah di
dalam bundel yang bicara stdio, dengan primitifnya memetakan ke perintah `qh-ffi` yang sudah ada.
Yang belum diputuskan adalah hal-hal yang mengubah kontrak keamanan: bagaimana token disimpan,
apa arti "scope", dan apa yang terjadi pada koneksi yang tidak ada di daftar. Tiga hal itu
menentukan sifat permukaan serang, jadi ketiganya dicatat di sini, bukan hanya di kode.

Titik acuan yang mengikat: `to_table` punya mode `replace` yang menjalankan `DROP TABLE IF EXISTS`
sebelum query-nya (`crates/qh-ffi/src/commands.rs`), dan `connections_list` berjalan pada tabel yang
memuat `secret_ref` serta `options_json`. Permukaan ini tidak boleh memberi klien yang belum punya
model otorisasi yang diuji kemampuan menulis atau membaca kredensial.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Tempat server | **Binari terpisah di dalam bundel** | Klien yang membunuhnya tidak menyentuh app; app tidak perlu tahu MCP ada | Satu binari lagi untuk ditandatangani dan diuji; dua proses memakai berkas database yang sama |
| | Server hidup di dalam proses app | Satu proses, satu hidup | Klien MCP yang mati bisa menjatuhkan app, dan app harus membuka listener sendiri — persis yang rencana hindari |
| | Server eksternal di luar bundel | Tidak menyentuh bundel | Bukan fitur produk: pengguna harus memasang dan menjalankannya sendiri |
| Penyimpanan token | **Hanya hash SHA-256 di `mcp_token`** | Bocornya tabel tidak langsung membocorkan token kerja; token tetap bisa dicabut dan dicatat | Token hilang selamanya kalau tidak dicatat saat `issue`; hash tidak bisa ditampilkan ulang |
| | Token sebagai teks di database | Bisa ditampilkan ulang | Satu backup atau dump dukungan berisi kredensial yang bekerja |
| | Token di Keychain, id-nya di database | Keychain sudah dipakai untuk password koneksi | Satu item Keychain per token, dan MCP harus membacanya di jalur yang sama dengan operasi rahasia lain; `issue/list/revoke` jadi menyentuh Keychain, bukan sekadar SQLite |
| Model scope | **Daftar nama tool + allowlist id koneksi** | Dua pertanyaan yang berbeda dijawab dengan dua daftar; allowlist bisa dikosongkan tanpa mematikan tool | Dua daftar yang bisa salah; id koneksi harus disalin dari `connections_list` |
| | Hanya daftar tool | Lebih sederhana | Sebuah token untuk semua koneksi berarti satu token yang bocor adalah seluruh workspace |
| | Peran bernama (read/export) | Lebih mudah dipahami | Menyembunyikan keputusan di balik nama; menambah tool berarti mengubah definisi peran |
| Arti allowlist kosong | **Tidak ada koneksi sama sekali** | Default dari daftar izin harus "tidak memberi apa pun" | Token harus selalu diberi `--connection`; kesalahan operator terlihat sebagai penolakan, bukan akses |
| | Semua koneksi | Nyaman | Salah ketik `--connection ""` menjadi token yang bisa membaca seluruh workspace |

## Keputusan

**Binari `queryhive-mcp` di dalam `QueryHive.app/Contents/MacOS/`, bicara MCP lewat stdio; satu
token per klien disimpan sebagai SHA-256 heksadesimal di tabel `mcp_token`; scope adalah daftar nama
tool dan allowlist id koneksi; allowlist kosong berarti tidak ada koneksi; `to_table` tidak pernah
menjadi tool.**

Rincian yang mengikat:

1. **Token dibaca dari `--token` atau `QH_MCP_TOKEN`.** Tidak ada jalur lain. Token yang hilang,
   tidak dikenal, kedaluwarsa, atau dicabut menghasilkan satu baris di **stderr** tanpa menyebut
   tokennya, lalu keluar dengan status bukan-nol.
2. **`tools/list` disaring oleh daftar tool token; `tools/call` menolak tool di luar itu.** Nama tool
   yang tidak ada di registry juga ditolak, dan `to_table` ditolak **sebelum** registry dilihat,
   supaya alasannya menyebut mode penulisan dan bukan sekadar "unknown tool".
3. **Argumen `connection` diperiksa terhadap allowlist sebelum database dibaca.** Token yang tidak
   boleh menyentuh sebuah koneksi tidak boleh bisa membedakan id yang ada dari id yang dikarang
   lewat pesan galat.
4. **`connections_list` mengembalikan subset yang dipilih tangan** — `id`, `name`, `kind`, `host`,
   `port`, `database` — dan tidak pernah `user_name`, `secret_ref`, `options_json`, atau password.
5. **`last_used_at` ditulis pada setiap `tools/call`**, berhasil maupun ditolak: yang dicatat adalah
   "token ini dipakai", bukan "token ini berhasil".
6. **Handshake ditulis di Application Support (`QH_MCP_HANDSHAKE` menimpanya) dan dihapus saat keluar
   bersih.** Handshake yang pid-nya sudah mati diabaikan, karena SIGKILL tidak memberi kesempatan
   menghapus berkasnya.

## Alasan

1. **Binari terpisah adalah satu-satunya bentuk di mana "membunuh MCP tidak berdampak pada app" itu
   struktural, bukan janji.** Keduanya berbagi berkas SQLite (mode WAL sudah menyediakan pembaca
   tanpa memblokir penulis) dan Keychain, dan tidak ada memori bersama. Server di dalam app akan
   membuat pernyataan itu bergantung pada kebenaran implementasi; di sini ia benar karena tidak ada
   yang bisa dibagi.
2. **SHA-256 cukup karena tokennya 256 bit acak dari CSPRNG.** Hash password yang lambat ada untuk
   entropi yang sedikit; token begini tidak punya ruang tebak untuk diperlambat, sementara biaya
   kerja akan memperlambat hal yang justru jadi alasan tabel ini ada — menjawab "token ini hidup?"
   di depan setiap panggilan.
3. **Dua daftar karena dua pertanyaan berbeda.** "Tool apa yang boleh dipanggil" dan "koneksi mana
   yang boleh disentuh" tidak saling menyimpulkan: token yang boleh `preview` tetap tidak boleh
   preview ke database produksi. Menggabungkannya jadi peran bernama akan menyembunyikan keputusan
   ini di balik satu kata.
4. **Allowlist kosong berarti tidak ada koneksi karena default daftar izin harus menolak.** Alternatif
   "kosong berarti semua" membuat kesalahan yang paling mungkin — lupa memberi `--connection` —
   justru menjadi token dengan akses terluas, dan itu kegagalan yang tidak terlihat sampai tokennya
   bocor.
5. **`to_table` tidak pernah menjadi tool karena mode `replace`-nya menjalankan DROP.** Menyembunyikan
   nama di registry tidak cukup; penolakan eksplisit menjaga nama itu tetap ditolak walau registry
   berubah.

## Konsekuensi

- **Permukaan FFI tidak berubah.** `queryhive-mcp` adalah binari ketiga di paket yang sama dan tidak
  menambah satu varian `EngineCommand` pun; `app/Generated/` tetap sama. Invariant #11 tidak terpakai.
- **Token tidak bisa ditampilkan ulang.** `issue` mencetaknya sekali; kehilangannya berarti mencabut
  dan menerbitkan yang baru. Itu disengaja, dan pesan `issue` tidak menyimpan salinannya.
- **`mcp_token` adalah tabel baru tanpa sync meta penuh.** Revocation adalah `revoked_at`, bukan
  tombstone, supaya `list` masih bisa melaporkan token yang sudah dicabut. `deleted_at` ada untuk
  konvensi tabel dan purge di masa depan, bukan untuk mencabut.
- **Scope tidak membatasi SQL, dan sekarang ditutup di lapisan lain.** Sebuah token dengan `preview`
  bisa menjalankan SQL apa pun yang diterima server, termasuk DDL — dokumen ini hanya memastikan MCP
  tidak menambah jalur tulis baru. Batas read-only yang sesungguhnya mendarat bersama Fase 3:
  `SAFE_MODE` ditegakkan di engine (`docs/decisions/0017-safe-mode.md`), dan server MCP kini
  menambahkan `SAFE_MODE=read_only` pada **setiap** panggilan, apa pun tingkat koneksinya.
- **Server memakai `KeychainStore` langsung.** Seam `CREDENTIAL_STORE=memory` milik perintah
  `credential` tidak dipakai MCP, jadi pengujian otomatis hanya mencakup tool yang tidak membaca
  password; jalur password diuji lewat penerimaan hidup dengan koneksi tanpa password (Trino dev).
- **Handshake adalah berkas, bukan protokol.** Dua server yang hidup bersamaan akan saling menimpa
  berkas itu, dan tidak ada yang menjaganya; saat ini hanya satu klien yang diasumsikan per pengguna.
