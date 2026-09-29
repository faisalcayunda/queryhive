# Kebijakan stabilitas MCP

> Satu halaman janji: apa yang boleh berubah di permukaan MCP dan apa yang tidak. Ditulis sebelum
> tool kesepuluh mendarat, karena janji yang ditulis sesudahnya adalah janji yang menyesuaikan
> kenyataan. Keputusan token dan scope ada di `docs/decisions/0015-mcp-token-scope.md`; yang ini
> hanya tentang kompatibilitas.

## Yang dijanjikan

Dalam satu versi mayor, permukaan MCP bersifat **aditif saja**:

| Boleh | Tidak boleh |
|---|---|
| Tool baru muncul di `tools/list` | Tool yang ada dihapus atau diganti nama |
| Field input baru pada tool lama, asalkan **opsional dan punya default** | Field input baru yang **wajib** pada tool lama |
| Field baru pada keluaran tool | Field keluaran yang dihapus atau berubah tipe |
| Kode galat baru | Kode galat lama yang berubah makna |
| Deskripsi tool yang diperjelas | Deskripsi yang berubah sehingga tool yang sama berarti lain |
| Resource baru di `resources/list` | Resource yang ada dihapus atau URI-nya berubah makna |
| Prompt baru di `prompts/list` | Prompt yang ada dihapus atau diganti nama |
| Argumen prompt baru yang opsional | Argumen prompt lama yang menjadi wajib |

Yang **tidak** masuk kontrak, dan karena itu boleh berubah kapan saja tanpa dianggap melanggar:
framing transpor (JSON-RPC satu baris per baris), berkas handshake, skema tabel `mcp_token`,
penyimpanan token, dan `connections.json`. Semua itu urusan dalam, bukan permukaan yang klien
pegang.

## Versi protokol

`initialize` mengembalikan `protocolVersion`: versi klien yang diminta bila server mengenalinya,
kalau tidak server **menolak** dengan kode `-32022` yang membawa daftar versi yang didukung di
`error.data.supported`. Klien yang tidak menyebut versi sama sekali dijawab dengan versi server.
Versi yang didukung sekarang: `2025-06-18`, `2025-03-26`, `2024-11-05`.

Menolak lebih baik daripada turun diam-diam ke versi server: negosiasi turun menyembunyikan
ketidakcocokan, dan klien tidak punya cara mengetahui versi mana yang boleh ia minta.

## Keputusan atas sisa pengerasan §12.3

Tiga item lain di rencana §12.3 diputuskan, bukan dikerjakan, dan alasannya ditulis supaya tidak
dibuka ulang tiap sesi:

1. **Pencabutan membatalkan request yang sedang jalan.** Tidak dikerjakan, karena tidak ada request
   yang sedang jalan untuk dibatalkan: `serve` membaca satu baris, menanganinya sampai selesai, baru
   membaca baris berikutnya. Sebuah `preview` yang panjang memblokir loop, jadi `revoke` tidak bisa
   tiba di tengahnya. Ini berubah kalau server menjadi konkuren, dan pada saat itu pencabutan harus
   ikut ditinjau.
2. **Rate limit pada autentikasi.** Ditunda. Server stdio memvalidasi token **sekali** saat start dan
   keluar bila gagal, jadi tidak ada percobaan berulang di dalam satu proses; membatasi lintas
   proses menuntut state persisten, dan terhadap token 256-bit acak tidak ada ruang tebak yang
   dilindungi. Yang lebih mungkin dibatasi adalah operator yang salah konfigurasi, bukan penyerang.
3. **"External Clients" sebagai lapisan terpisah dari Safe Mode.** Diputuskan nanti, ketika MCP
   dipakai lebih dari satu orang. Sekarang izin efektif sebuah token adalah `MIN(scope token,
   allowlist koneksi, Safe Mode koneksi)` — dan MCP selalu `read_only` — yang sudah fail-closed untuk
   satu pengguna. Lapisan ketiga baru punya arti ketika ada klien luar yang harus dibatasi tanpa
   membatasi pengguna aplikasinya.
