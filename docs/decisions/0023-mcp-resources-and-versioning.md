# 0023 — Resources MCP, prompts template, dan menolak revisi protokol yang tidak dikenal

- **Status:** Diterima
- **Tanggal:** 29 Sep 2026 (Fase 2)
- **Konteks instruksi:** `docs/architecture/tablepro-source-study.md` §7 (bagian MCP);
  `docs/mcp-stability.md`; `docs/decisions/0015-mcp-token-scope.md`

## Konteks

Fase 2 mengirim server MCP read-only dengan sembilan tool dan token yang membawa daftar nama tool dan
daftar id koneksi (`crates/qh-ffi/src/mcp.rs`). Dua bagian permukaan protokolnya masih hilang, dan
satu sudah dicatat sebagai celah:

1. **Negosiasi protokol.** `initialize` meng-echo revisi klien bila server mengenalinya dan kalau
   tidak menjawab dengan revisi server sendiri. Fallback itu ramah tetapi menyembunyikan
   ketidakcocokan: klien yang meminta revisi yang tidak dibangun siapa pun tidak bisa membedakannya
   dari dipahami. Halaman stabilitas mencatat ini sebagai belum.
2. **Resources dan prompts.** `initialize` hanya mendeklarasikan `capabilities.tools`. Klien yang
   mengharapkan dua kemampuan lainnya tidak menemukan keduanya.

Batasan yang mengikat adalah ADR-0015: scope sebuah token adalah **daftar nama tool**, dan sebuah
koneksi hanya boleh disebut bila ada di **allowlist** token; allowlist kosong berarti tidak ada
koneksi. Resources dan prompts tidak boleh mengarang model otorisasi kedua, dan tidak boleh menjadi
pintu samping di sekitar model yang sudah ditegakkan tool.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Revisi tak dikenal | **Tolak dengan `-32022` dan `data.supported`** | Ketidakcocokan terlihat; klien tahu apa yang bisa dimintanya | Mematahkan klien yang bersandar pada penurunan versi senyap |
| | Pertahankan fallback senyap | Tidak pernah galat | Menyembunyikan ketidakcocokan, yang justru celah yang dicatat |
| Gerbang resource | **Scope tool pendukung + allowlist** | Satu model otorisasi; tidak ada salinan kedua dari aturan mana pun | Resource tak terjangkau kecuali tool yang bersangkutan ada di scope |
| | Nama scope sintetis baru | Independen dari daftar tool | Scope adalah nama tool menurut ADR-0015; scope non-tool butuh aturan penerbitan baru |
| Himpunan resource | **Koneksi, dan objects/tables per koneksi** | Menutup data read-only yang sudah dijangkau klien lewat tool | Listing murah; sebuah read bisa menyentuh jaringan |
| | Koneksi saja | Tidak ada jaringan di `resources/read` | Melewatkan pohon objek yang disebut brief sebagai resource kedua yang jelas |
| Penolakan | **Satu `-32002` untuk yang tidak ada maupun yang tidak boleh** | Galatnya tidak bisa dipakai menyelidik id koneksi | Pesannya kehilangan beda antara "bukan milikmu" dan "sudah tidak ada" |
| Prompts | **Dua template, digerbangi tool yang disebutnya** | Jujur: tidak ada model di sisi server; klien tidak pernah diarahkan ke panggilan yang tak bisa dilakukannya | Token yang di-scope ke tool itu kehilangan prompt-nya saat scope berubah |
| | Daftar kosong | Tidak ada yang dijaga | Kemampuannya dideklarasikan tanpa alasan |

## Keputusan

**`initialize` menolak revisi protokol yang tidak dikenal server dengan kode JSON-RPC `-32022` yang
membawa `error.data.supported`, dan meng-echo revisi yang dikenalnya; `resources` dan `prompts`
dideklarasikan sebagai kemampuan; `resources/list` dan `resources/read` hanya mengekspos resource
yang terjangkau scope dan allowlist token, dan `prompts/list` serta `prompts/get` mengekspos template
murni tanpa generasi di sisi server.**

Rincian yang mengikat:

1. **Resources adalah koneksi yang boleh dijangkau token, dan satu resource pohon objek per koneksi.**
   `queryhive://connections` adalah tampilan agregat `connections_list`;
   `queryhive://connections/{id}` adalah metadata satu koneksi yang dipilih tangan;
   `queryhive://connections/{id}/objects` dan `.../tables` menjalankan perintah `objects`/`tables`,
   jadi read atas keduanya adalah jawaban tool itu sendiri. Resource sebuah koneksi membawa persis
   medan yang dibawa tool — `id`, `name`, `kind`, `host`, `port`, `database` — dan tidak pernah nama
   user, password, kantong `options_json`, atau `secret_ref`.
2. **Sebuah resource muncul hanya bila tool pendukungnya ada di scope.** Resource agregat dan metadata
   per koneksi butuh `connections_list`; resource objek dan tabel butuh `objects` dan `tables`.
   Resource per koneksi dibangun dari baris tersaring yang sama yang dikembalikan `connections_list`,
   jadi koneksi di luar allowlist tidak pernah dienumerasi.
3. **Penolakan tidak mengatakan apa-apa soal keberadaan.** `resources/read` menjawab setiap URI di
   luar jangkauan token — tidak ada, di luar allowlist, di luar scope, atau sekadar bukan bentuk URI
   server ini — dengan `-32002` "resource not found" yang sama. Pesannya tidak memuat "not allowed"
   atau "was not found", jadi ia tidak bisa membedakan id asli dari yang dikarang. Allowlist-nya
   diperiksa sebelum pembacaan store mana pun, persis seperti di jalur tool.
4. **Prompts adalah template, bukan percakapan.** `explain_query` dan `summarize_tables` menyulihkan
   argumennya ke teks tetap; tidak ada apa pun di server yang memanggil model, dan tidak ada yang
   membaca koneksi. Setiap prompt menyebut tool yang diperintahkannya untuk dipanggil klien, dan hanya
   ditawarkan bila tool itu ada di scope token.
5. **Baris koneksi yang dipilih tangan hanya punya satu salinan.** `visible_connections`
   membangunnya, dan baik tool `connections_list` maupun resource koneksi membacanya. Medan yang
   ditambahkan ke satu jalur tidak bisa terlewat di jalur lain.

## Alasan

1. **Satu model otorisasi, bukan dua.** Memakai ulang `McpToken::allows` dan
   `McpToken::allows_connection` berarti sebuah resource tidak bisa dijangkau token yang akan ditolak
   tool yang bersangkutan. Kosakata scope kedua akan menyimpang dari daftar tool yang divalidasi
   perintah issue.
2. **Non-disclosure adalah inti `-32002` bersama itu.** ADR-0015 sudah menolak koneksi yang tidak
   diizinkan sebelum membaca store, jadi galatnya tidak bisa dipakai meng-enumerasi id. Sebuah
   resource read yang mengatakan "not allowed" untuk satu id dan "not found" untuk id lain akan
   membuka kembali penyelidikan itu di permukaan baru.
3. **Menolak revisi tak dikenal lebih jujur daripada menurunkannya.** Klien menetapkan versi yang ia
   tuturkan; menjawab dengan versi lain membiarkannya percaya bahwa ia dipahami. `-32022` plus daftar
   dukungan adalah satu jawaban yang membiarkannya mencoba lagi dengan benar.
4. **Prompt template tidak bisa berbohong soal model yang tidak dimiliki server.** Rendering-nya
   `format!` atas teks tetap; tidak ada generasi, tidak ada tool call, dan tidak ada akses data yang
   bersembunyi di balik `prompts/get`.
5. **Resource objects/tables adalah jawaban tool itu sendiri.** Menjalankan `Command::Objects` /
   `Command::Tables` dengan settings yang sama yang dibangun tool berarti sebuah resource read dan
   tool call tidak bisa berbeda pendapat soal bentuk pohon sebuah koneksi.

## Konsekuensi

- **Permukaan FFI tidak berubah.** Tidak ada varian `EngineCommand` yang ditambahkan; binari MCP tetap
  memetakan ke perintah yang sudah ada (invariant #11 tidak berlaku).
- **`resources/read` mencatat pemakaian token.** Ia adalah akses ke data token, jadi ia menyetel
  `last_used_at` seperti `tools/call`; `resources/list` dan metode prompt tidak.
- **Catatan celah di halaman stabilitas ditutup di perubahan yang sama.** `docs/mcp-stability.md` kini
  menyatakan penolakannya, dan tabel aditifnya membawa resources dan prompts di samping tools.
- **Resource objects/tables bersifat lazy, bukan hidup.** `resources/list` tidak pernah membuka
  koneksi; hanya `resources/read` atas resource `objects`/`tables` yang membukanya, dan read itu gagal
  dengan galat engine yang sama yang akan dikembalikan tool.
- **`resources/templates/list` tidak diimplementasikan.** Setiap URI yang diterbitkan server ini
  konkret; tidak ada keluarga bertemplate untuk diiklankan, dan daftar template kosong hanya kebisingan.
- **Ketersediaan prompt mengikuti scope tool-nya.** Token yang kehilangan scope `explain` kehilangan
  `explain_query` bersamanya. Itu disengaja: prompt-nya ada untuk menggerakkan panggilan yang tetap
  harus boleh dilakukan token.
