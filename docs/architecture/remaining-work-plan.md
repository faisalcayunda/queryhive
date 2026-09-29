# Rencana sisa pekerjaan, setelah Gelombang 3

- **Status:** rencana kerja, 29 Sep 2026
- **Konteks:** hasil pemeriksaan ulang seluruh item yang belum dikerjakan, dengan koreksi terhadap
  beberapa klaim yang beredar di dokumen lama. Sumber tiap koreksi adalah kode di pohon ini.

## Yang sudah mendarat, supaya tidak diusulkan lagi

Gelombang 3 di-merge: `table_op` (truncate/drop di bawah Safe Mode), parameter binding di ketiga
driver, konfirmasi app yang mengirim `SAFE_MODE_CONFIRMED`, peta kolom impor, truncate/drop dari
pohon, dan sort server untuk hasil yang terpotong. Menyusul: pane General dengan preferensi startup
dan operasi tutup tab, fondasi identitas aplikasi dengan ADR-0029 (tabel `app_account` dan `profile`,
migrasi `0008`), dan rename akar sumber ke `Sources/QueryHive`. Gate terakhir: `cargo test` 768/0,
`swift test` 392/0.

## Empat temuan yang mengubah daftar

1. **Lubang ADR-0027 hari ini hanya bisa dicapai lewat CLI.** `SAFE_MODE_FLOOR` dibaca di satu tempat
   saja (`crates/qh-ffi/src/commands.rs:143`) dan di tes; app hanya mengirim `SAFE_MODE`
   (`app/Sources/QueryHive/Models/AppModel.swift:2850`), dan MCP memaksa `read_only`. Tetap harus
   ditutup lebih dulu, karena komentar di `commands.rs:109-111` menyebut floor itu sebagai "the one
   input a configuration profile would set": begitu profiles berjalan, lubangnya jadi nyata.
2. **Perbaikan yang benar bukan resolusi floor per jenis statement.** Guard biasa sudah benar di
   urutan strictness total (`crates/qh-sql/src/classify.rs:173-188`). Yang merusak monotonisitas
   hanya `guard_destructive` (`commands.rs:291`), karena ia menerima mode yang **sudah di-resolve**
   sehingga tidak bisa melihat floor. Perbaikannya sekitar dua puluh baris di fungsi itu, bukan
   perubahan di `qh-sql`.
3. **Penolakan versi protokol MCP sudah ada.** `crates/qh-ffi/src/mcp.rs:770-796` mengembalikan
   `-32022` beserta `error.data.supported` (commit `310c03d`), dan `docs/mcp-stability.md` sudah
   mencatatnya. Yang tersisa hanya teks basi: komentar `mcp.rs:79-83` yang masih menulis "falling
   back ... is the honest answer", dan kalimat terakhir plan §12.3.
4. **Studi settings melebih-lebihkan kategori "setting murah".** Nomor baris dan folding selalu
   menyala (`app/Sources/QueryHive/Views/SQLEditor.swift:76-79`), wrap di-hardcode (`:61`), dan
   keyword case, tab width, invisibles belum ada fiturnya sama sekali. Yang benar-benar murah dan
   berguna hanya default row limit untuk tab baru (`Models/QueryTab.swift:463`, sekarang `1000`).

Dua item lain ditutup tanpa dikerjakan: batas format kolom sudah terdokumentasi di
`Models/ColumnFormat.swift:3-12` ("rendering-only ... what an export writes"), dan item PROGRESS.md
tentang rotasi key, Apple Developer ID, serta riset web bukan pekerjaan agen.

## Rencana

> **Status 29 Sep 2026.** Batch 1 selesai. Lane A mendarat sebagai `30aef13` (floor ADR-0027
> ditutup, `cargo test` 770/0), Lane B sebagai `d502d0c` (default row limit dan Appearance
> master-detail, `swift test` 395/0), dan keempat keputusan Batch 2 dicatat di `2dbe952`.
> Batch 3 juga selesai: command akun dan profil sebagai `581ece6` (`cargo test` 773/0), dan Open
> Quickly sebagai `551b13a` (`swift test` 401/0). Yang tersisa dari rencana ini hanyalah pane
> Profiles sebagai UI, yang menunggu sign-in; rotasi key kenari tetap tugas pengguna.

### Batch 1, Lane A: floor Safe Mode (engine, serial)

Aksi pertama: dua tes gagal di `crates/qh-ffi/tests/safe_mode.rs`.

- `SAFE_MODE=confirm` dengan `SAFE_MODE_FLOOR=no_ddl` dan `SAFE_MODE_CONFIRMED=1`, `TABLE_OP=drop`:
  ditolak sebelum connect, memakai kalimat `no_ddl`.
- `SAFE_MODE=no_ddl` dengan `SAFE_MODE_FLOOR=confirm`: ditolak.

Lalu `guard_destructive` membaca entri floor (`SafeModeFloor::iter()`), bukan mode yang sudah
di-resolve: ada entri `no_ddl` atau `read_only` berarti refuse dengan kalimat mode itu; kalau tidak
ada tetapi ada `confirm` berarti tanya; selain itu allow. Di lane yang sama, perbaiki komentar basi
`mcp.rs:79-83` dan kalimat plan §12.3, lalu tulis addendum ADR-0027 yang mengoreksi kalimat "tempat
memperbaikinya adalah resolusi floor". Tidak ada command baru, jadi `lib.rs`, `COMMANDS`, dan
`tests/golden.rs` tidak tersentuh.

Gate: `cargo test`, target 770/0.

### Batch 1, Lane B: pane app (jalan selagi gate Lane A)

Satu berkas: `app/Sources/QueryHive/Views/SettingsView.swift`, plus `Models/QueryTab.swift` untuk
default-nya. Tidak ada irisan dengan Lane A.

- Default row limit untuk tab baru, di pane Data.
- Appearance master-detail: daftar dikelompokkan Dark/Light yang mengisi slot mode, detail berisi
  preview dan baris global yang sudah ada.

Gate: `swift build && swift test`, target 392/0. Kedua lane di-merge `--no-ff` setelah hijau.

### Batch 2: checkpoint keputusan, sudah dijawab 29 Sep 2026

1. **Profiles: kind pertama adalah `preference`.** Setelan aplikasi per akun. Konsumennya paling
   jelas dan tidak menyentuh koneksi, jadi ia dibangun lebih dulu; `connection` dan `saved_query`
   menyusul bila ada kebutuhan.
2. **Notifications: tidak dibangun.** App belum punya permukaan notifikasi dan belum ada pelacakan
   operasi panjang.
3. **Gelombang 4: hanya Open Quickly.** Structure editor, routines/UDT, backup & restore, copy
   object, dan external API tidak dikerjakan.
4. **Pane MCP: tetap ditunda** sampai MCP dipakai lebih dari satu orang.

Command profil di `lib.rs` dikerjakan sebagai lane tersendiri karena menyentuh `lib.rs`,
`tests/golden.rs`, dan `EngineCommand` di app. Sebuah profil **boleh** menetapkan
`SAFE_MODE_FLOOR`, dan itulah alasan floor ditutup lebih dulu di Lane A: sebelum addendum ADR-0027,
sebuah profil yang memaksa `no_ddl` masih bisa dilewati `SAFE_MODE_CONFIRMED`.

### Di luar agen

Rotasi key kenari, karena PROGRESS.md sudah menyatakannya bocor. Notarisasi dan kunci EdDSA Sparkle
diblokir Apple Developer ID, jadi dikerjakan saat mau rilis.

## Yang di-drop atau ditunda

| Item | Alasan |
|---|---|
| Format kolom diterapkan di ekspor | Batasnya sudah ditulis di kode; spec yang dijalankan engine adalah fitur yang belum diminta siapa pun |
| Sisa §12 (12.2.2, MCP §12.3) | 12.2.2 dinilai rendah oleh plan sendiri; MCP tidak menyisakan apa pun untuk dibangun |
| Toggle editor untuk nomor baris dan folding | Fiturnya selalu menyala; toggle untuk yang selalu menyala tidak berguna, dan sisanya fitur baru |
| Notifications | Belum ada keputusan, dan belum ada permukaan notifikasi di app |
| Command dan UI profiles | Sampai checkpoint Batch 2 dijawab; CRUD tanpa konsumen itu spekulatif |
| Gelombang 4 selain Open Quickly | Structure editor menambah permukaan DDL besar yang bentrok dengan Safe Mode; backup PostgreSQL berarti urusan versi `pg_dump`; external API sudah dicakup MCP |
| Riset Navicat/DataGrip dan versi minimum | Mencatat "versi teruji" adalah sikap yang jujur dan sudah terdokumentasi |

## Verifikasi

`cargo test` minimal 770/0 dengan dua tes baru; `swift build && swift test` minimal 392/0; dan satu
jalan manual `table_op` dengan `SAFE_MODE=confirm SAFE_MODE_FLOOR=no_ddl SAFE_MODE_CONFIRMED=1` yang
menunjukkan penolakan `no_ddl`.
