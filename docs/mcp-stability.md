# Kebijakan stabilitas MCP

> Satu halaman janji: apa yang boleh berubah di permukaan MCP dan apa yang tidak. Ditulis ketika
> baru ada sembilan tool, dan dipakai tanpa perubahan saat tool kesepuluh dan kesebelas mendarat
> (W11, bagian "Tool metadata"), karena janji yang ditulis sesudahnya adalah janji yang
> menyesuaikan kenyataan. Keputusan token dan scope ada di
> `docs/decisions/0015-mcp-token-scope.md`; yang ini hanya tentang kompatibilitas.

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

## Tool metadata (W11)

`describe_table` dan `table_ddl` menjadi tool kesepuluh dan kesebelas. Keduanya aditif, jadi
tabel di atas berlaku untuk mereka seperti untuk sembilan yang lain, dan `columns` **tetap**:
namanya, skemanya, dan `LIMIT 0`-nya tidak berubah. Deskripsi `columns` hanya diperjelas dengan
petunjuk ke `describe_table`.

- **Masukan.** `connection` dan `table` wajib; `catalog` dan `schema` opsional dan, bila kosong,
  diambil dari koneksi. Slot yang dipakai mengikuti driver: PostgreSQL schema dan tabel, MySQL
  database (argumen `catalog`) dan tabel, Trino ketiganya. Slot yang tidak diisi argumen maupun
  koneksi ditolak dengan nama argumennya, sebelum koneksi dibuka.
- **Keluaran.** Event engine apa adanya: `table_columns` (`object`, `fields`, `truncated`) dan
  `table_ddl` (`object`, `object_kind`, `ddl`, `truncated`, `redacted`). Kunci baru boleh
  ditambahkan; yang ada tidak berubah arti.
- **Allowlist dan scope.** Allowlist diperiksa sebelum store dibaca, dan allowlist kosong berarti
  tidak ada koneksi: id yang ada dan id yang tidak pernah ada dijawab dengan kalimat yang sama. Token yang diterbitkan tanpa `--scope` memuat keduanya; token lama tidak
  (gagal tertutup), dan operator yang menginginkannya menerbitkan token baru.
- **Read-only.** Setiap panggilan MCP tetap dipaksa `read_only`, apa pun Safe Mode koneksinya.
  Perintah metadata tidak membawa SQL dari klien, setiap statement-nya satu pembacaan katalog,
  dan sesinya dipaksa read-only oleh engine.
- **DDL.** Engine menyensor kredensial yang dicetak server ke dalam DDL (`redacted: true`), dan
  itu usaha terbaik: definisi view dibawa apa adanya dan bisa membuka logika bisnis. Operator
  yang tidak menginginkannya menerbitkan token tanpa `table_ddl` di `--scope`. Apakah `table_ddl`
  keluar dari scope bawaan adalah keputusan SEC yang belum diambil; sampai itu diputuskan ia ikut
  scope bawaan.

## Batas hasil, scope nama, dan baseline permukaan (W11-T3r)

Semuanya aditif, jadi tabel di atas berlaku:

- **`preview.limit`** punya batas atas 10000 baris (bawaan tetap 1000, batas bawah tetap 1). Nilai di
  luar rentang dipotong, bukan ditolak.
- **`preview.cell_char_limit`** (opsional, bawaan 4096, rentang 16 sampai 1000000) memotong setiap sel
  teks per **karakter**, bukan byte. Event `rows` yang selnya terpotong membawa `cells_truncated`
  (jumlah sel); kunci itu tidak ada bila tidak ada yang terpotong. `explain` memakai bawaan yang sama.
- **Nama kualifikasi** (`catalog`, `schema`, `table`) harus satu baris teks biasa, paling banyak 255
  karakter, tanpa karakter kontrol. Pemeriksaan ini berjalan **sesudah** allowlist koneksi, jadi tidak
  bisa dipakai untuk menebak koneksi yang tidak boleh dijangkau token. Token tidak punya scope per
  schema atau database (itu akan butuh migrasi tabel `mcp_token`); scope-nya tetap koneksi.
  Untuk Trino dan MySQL, `preview` menerima SELECT apa pun yang boleh dijalankan user database itu,
  jadi membatasi `catalog` di `describe_table` saja akan jadi pagar tanpa pintu. **PostgreSQL
  berbeda**: tidak ada SELECT lintas database, dan `catalog` menjadi database yang disambungi engine,
  jadi argumen itu satu-satunya jalan keluar dari database koneksi. Karena itu, pada koneksi
  PostgreSQL yang punya database tersimpan, `catalog` yang tidak sama dengan database itu ditolak
  dengan kalimat "outside this token's scope" (sesudah allowlist) di `tables`, `objects`, `columns`,
  `describe_table`, dan `table_ddl`. Browsing lintas database PostgreSQL lewat MCP adalah keputusan
  owner yang belum diambil, bukan sesuatu yang sengaja diizinkan.
- **Statement timeout.** Setiap panggilan MCP kini berjalan dengan batas: `statementTimeoutMS`
  koneksi bila positif (maksimum 600000), selain itu 60000. Nilai 0 di app berarti tanpa batas dan
  **tidak** dihormati di MCP.
- **Baseline permukaan.** `crates/qh-ffi/tests/mcp_surface.json` membekukan nama tool, `required`,
  nama properti, prompt, dan bentuk URI resource. `tests/mcp.rs` gagal bila ada yang hilang atau
  menjadi wajib; yang bertambah lolos dan baseline direkam ulang dengan
  `QH_RECORD_MCP_SURFACE=1 cargo test -p qh-ffi --test mcp nothing_a_client`.

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
