# PRD: performa dan paritas QueryHive terhadap TablePro

- **Status:** disetujui untuk dieksekusi, 29 Sep 2026. Belum ada yang dikerjakan.
- **Pasangan:** `docs/architecture/development-plan.md` (siapa mengerjakan apa, dalam urutan apa, dan dengan gate apa). Dokumen ini menjawab *apa* dan *seberapa baik*.
- **Sumber:** `docs/architecture/performance-plan.md` (Fase 0–8, sumbu, ADR 0030–0036), `docs/architecture/tablepro-feature-map.md` (peta fitur, 18 use case, must/should/later/skip), `docs/architecture/tablepro-design-audit.md` (P0/P1/P2), `docs/architecture/tablepro-adoption-plan.md` (§0 gate berat, §12, §13), `docs/architecture/remaining-work-plan.md` (Batch 7), `docs/architecture/tablepro-feature-analysis.md`, `docs/invariants.md`, `docs/benchmarks.md`, `app/DESIGN.md`. Di akar repo tidak ada `DESIGN.md`. Kontrak desainnya adalah `app/DESIGN.md`.
- **Batas lisensi:** TablePro berlisensi AGPL-3.0, sedangkan QueryHive MIT (`LICENSE`, ADR-0002, `deny.toml`). TablePro hanya dibaca untuk mempelajari ide dan angka. Kode, aset, dan string UI-nya tidak pernah masuk ke pohon ini.

## 1. Masalah

QueryHive sudah menang di engine: lewat CLI, 191.251 baris/s, baris pertama 11 ms setelah connect, dan RSS 9 MB untuk 500k × 30 (`docs/benchmarks.md:65`). Jalur app belum pernah diukur. Dari kodenya, jalur app kalah di lima tempat (`performance-plan.md` §1):

- runtime tokio dan koneksi dibangun baru untuk setiap perintah;
- baris lewat JSON dan di-decode di thread engine;
- seluruh hasil ditahan sebagai `[[String?]]`;
- grid adalah SwiftUI `LazyVStack` yang membangun view per sel;
- editor memindai seluruh dokumen pada setiap ketikan.

Stop pada preview juga tidak sampai ke server.

Di sisi fitur, pengguna utamanya bekerja lewat bastion SSH, di tabel besar, dengan akses prod. Tujuh hal yang ia pakai setiap hari masih belum ada:

- UI SSH;
- grid tervirtualisasi dengan paginasi;
- formatter, toggle comment, dan simpan `.sql` (ketiganya sekarang hanya label);
- script runner;
- metadata read-only (kolom, DDL, view);
- penanda prod;
- gestur insert/delete baris.

Di sisi desain, grid dan pohon tidak punya jalur keyboard. VoiceOver hampir buta. Kontras teks grid di bawah 4,5:1. Pengaturan Reduce Motion, Reduce Transparency, dan Increase Contrast belum dihormati.

## 2. Tujuan dan bukan-tujuan

**Tujuan**

| ID | Tujuan | Diukur oleh |
|---|---|---|
| G-1 | Lebih cepat dan lebih hemat daripada TablePro di ketujuh sumbu `performance-plan.md` §2. Bila TablePro tidak bisa diukur, cukup memenuhi target absolutnya. | NFR-P1…P7, `docs/benchmarks.md` |
| G-2 | Menutup semua must-have dan should-have peta fitur | FR per domain (§5), UC yang ditandai Masuk (§4) |
| G-3 | Memenuhi lantai aksesibilitas dan jangkauan keyboard (P0 dan P1 audit desain) | NFR-A |
| G-4 | Tidak kehilangan satu pun keunggulan yang sudah ada | PR-01…PR-16 (§7) |
| G-5 | Kontrak tetap stabil: NDJSON untuk CLI, MCP, dan golden tidak berubah, kecuali lewat perintah atau setelan baru yang tercatat | NFR-C |
| G-6 | Tampilan identik dengan hari ini, kecuali perubahan yang sengaja dan terdaftar | NFR-V |

**Bukan tujuan**

- Menambah engine, plugin ABI, sinkronisasi iCloud, app iOS, atau lisensi berbayar.
- Merilis ke pengguna. Notarisasi, DMG, push, dan PR tidak termasuk; hasilnya hanya merge ke `main` lokal.
- Fase 8 (eskalasi). Fase ini hanya dikerjakan bila sebuah gate gagal.
- Menulis ulang jalur ekspor. Jalur itu sudah menang dan harus dijaga (PR-01).

## 3. Persona

| Persona | Kerja harian | Yang paling ia butuhkan |
|---|---|---|
| **P-1 Data engineer gudang data** | Trino on-prem, tabel fakta ratusan juta baris, ekspor untuk mitra, `to_table` | Grid besar yang tetap lancar, sort/search atas seluruh hasil, ekspor dengan memori datar, autocomplete dari katalog |
| **P-2 Backend engineer** | PostgreSQL/MySQL prod di balik bastion, edit data kecil, tuning query | SSH di app, penanda prod, insert/delete lewat tinjauan, pohon rencana EXPLAIN, file `.sql` |
| **P-3 Engineer on-call** | Query liar, lock, migrasi yang macet | Stop yang sampai ke server, tampilan sesi aktif dengan cancel |
| **P-4 Pengguna keyboard dan VoiceOver** | Semua hal di atas tanpa pointer | Jangkauan keyboard ke pohon, grid, editor, dan tab; label VoiceOver; kontras |
| **Aktor sistem: agen AI lewat MCP** | Membaca skema dan preview | `describe_table` dan DDL di dalam allowlist, tanpa write |

## 4. Use case

Diambil dari `tablepro-feature-map.md` §3 dan dipertajam. Kolom **Lingkup**: *Masuk*, *Sebagian* (bagian yang keluar disebut), atau *Sudah ada, dijaga*.

| UC | Judul | Lingkup | Berhasil bila | FR |
|---|---|---|---|---|
| UC-01 | Konek ke Postgres prod lewat bastion | Masuk | Konek tanpa `ssh -L` manual. Host key dicek terhadap known_hosts, dan penerimaan host baru dipatok ke fingerprint yang ditampilkan. Rahasia SSH hanya ada di Keychain. Koneksi yang sama bisa dipakai CLI dan MCP. Ditandai `prod`. | FR-CON-01…06 |
| UC-02 | Menjelajahi skema asing | Masuk | View dan materialized view PostgreSQL tampil dan berlabel. Kolom terlihat tanpa menjalankan query. DDL terbuka dalam ≤ 2 klik. Double-click melakukan preview. | FR-TREE-01…03 |
| UC-03 | Menulis query dengan autocomplete sungguhan | Masuk | Setelah `a.`, kolom tabel ber-alias `a` muncul tanpa Run sebelumnya dan tanpa menahan ketikan. Format dengan satu tombol. | FR-ED-01, FR-ED-05 |
| UC-04 | Menjelajahi tabel 500k+ baris | Masuk | Scroll memenuhi NFR-P4. Memori ≤ anggaran + 64 MB. Baris pertama memenuhi NFR-P1. "Ambil lebih banyak", "Ambil semua", dan "Ke baris…" tersedia. Sort mengikuti server. | FR-GRID-01…05, FR-PERF-05 |
| UC-05 | Membaca satu baris tabel yang sangat lebar | Masuk | Mode Record menampilkan semua kolom sebagai pasangan label dan nilai, tanpa scroll horizontal. Nama field bisa dicari. Sel JSON terbuka sebagai pohon. | FR-GRID-13 |
| UC-06 | Menjalankan skrip migrasi 40 statement | Masuk | Status tiap statement terlihat. Stop berhenti di antara dan di dalam statement. Kebijakan stop/continue berlaku. Nomor dan pesan statement yang gagal ditampilkan. Berkas tersimpan balik. Ada satu entri history. Ada notifikasi bila app tidak di depan. | FR-RUN-01…04, FR-ED-03 |
| UC-07 | Menghentikan query liar di prod | Masuk | Server mengonfirmasi cancel dalam NFR-P6. Pesan timeout menyebut batasnya. Sesi bisa diperiksa di tampilan aktivitas. | FR-PERF-01, FR-SAFE-03 |
| UC-08 | Menjauhkan write dari koneksi prod | Masuk | Engine menolak sebelum connect, dan penolakan yang sama berlaku lewat CLI dan MCP. Tingkat Safe Mode dan lingkungan selalu terlihat di tab, breadcrumb, dan status bar. | FR-SAFE-01, FR-CON-06 |
| UC-09 | Memperbaiki beberapa baris dengan aman | Masuk | Edit sel, tambah baris, dan hapus baris masuk antrean. SQL yang ditinjau sama dengan yang dijalankan. Hitungan yang meleset membuat rollback. ⌘S membuka tinjauan dan ⌘Z membatalkan. | FR-GRID-09…11 |
| UC-10 | Menyetel query lambat | Sebagian (perbandingan plan dan insights keluar) | EXPLAIN (ANALYZE) PostgreSQL dan EXPLAIN JSON Trino tampil sebagai pohon dengan cost, rows, dan waktu per node, dengan node terpanas ditandai. ANALYZE tunduk pada Safe Mode seperti statement di dalamnya. | FR-PLAN-01…02 |
| UC-11 | Ekspor 10 juta baris untuk mitra | Sudah ada, dijaga, plus progres | Memori datar, retry, part split. Progres terlihat di Finder dan Dock. Notifikasi saat selesai. | PR-01, FR-RUN-04…05 |
| UC-12 | Mematerialisasi hasil di dalam Trino | Sudah ada, dijaga | Baris tidak pernah meninggalkan cluster. `replace` ditolak di koneksi read-only. | PR-02, PR-04 |
| UC-13 | Memuat CSV/XLSX/JSON/`.sql` ke tabel | Masuk (JSON dan `.sql` baru) | Kebijakan transaksi ADR-0019 dan ADR-0022 berlaku untuk keempat format. CSV dan JSONL mengalir. | FR-IMP-01…02, PR-07 |
| UC-14 | Menemukan query minggu lalu | Sudah ada, plus Open Quickly P1 (frecency keluar) | Hit FTS < 1 dtk. Highlight tetap terlihat saat panah melewati tepi. Filter lingkup All/Objects/Saved/History tersedia. | FR-UI-09 |
| UC-15 | Agen AI membaca gudang dengan aman | Masuk | `describe_table` dan `table_ddl` hanya jalan di dalam allowlist dan scope. `read_only` dipaksa. Tidak ada kredensial di keluaran. | FR-MCP-01, PR-06 |
| UC-16 | Melanjutkan kerja setelah restart | Masuk | Tab yang terikat berkas membuka berkasnya lagi. Tidak ada SQL yang hilang. Tidak ada eksekusi ulang otomatis. | FR-ED-03, PR-16 |
| UC-17 | Mencari siapa yang mengunci tabel | Masuk | Sesi, wait, dan pid pemblokir terlihat dalam < 1 menit tanpa menulis query. Cancel query berjalan di bawah Safe Mode. Terminate tidak ditawarkan. | FR-SAFE-03 |
| UC-18 | Trino dengan JWT dan CA internal | Masuk (MySQL CA keluar, §12.1) | Konek tanpa mematikan verifikasi TLS. CA yang terpasang di Keychain sistem sudah dipercaya hari ini untuk PostgreSQL dan Trino. Berkas CA per koneksi ditambahkan untuk keduanya. | FR-CON-07…08 |

## 5. Kebutuhan fungsional

Kolom **Tugas** menunjuk ID di `development-plan.md`.

### 5.1 Koneksi, SSH, TLS, dan auth

| ID | Kebutuhan | UC | Sumber | Tugas |
|---|---|---|---|---|
| FR-CON-01 | Form koneksi punya bagian SSH: host, port, user, metode (agent/key/password), path key, dan passphrase. Password dan passphrase SSH disimpan sebagai item Keychain terpisah dari password database. | UC-01 | must #1 | W11-T2, W11-T3 |
| FR-CON-02 | Alias `Host` dari `~/.ssh/config` bisa dipilih dan di-resolve ke `HostName`, `User`, `Port`, dan `IdentityFile`. Direktif yang tidak didukung, seperti `ProxyJump`, disebut dalam pesan dan tidak diabaikan diam-diam. | UC-01 | must #1 | W11-T2, W11-T3 |
| FR-CON-03 | Host key yang belum dikenal membuka prompt berisi fingerprint SHA256. Penerimaan dipatok ke fingerprint itu, sehingga kunci lain pada percobaan ulang tetap ditolak. Kunci yang diterima ditulis ke known_hosts milik app (`0600`). `~/.ssh/known_hosts` dibaca tetapi tidak pernah ditulis. Kunci yang berubah ditolak keras, tanpa jalur terima. | UC-01 | must #1 | W11-T2, W11-T3 |
| FR-CON-04 | Impor Navicat memetakan entri SSH. Entri seperti itu tidak lagi dibuang (`Models/AppModel.swift:1248`). | UC-01 | must #1 | W11-T3 |
| FR-CON-05 | Setelan SSH, JWT, dan CA ikut dipetakan di jalur MCP (`crates/qh-ffi/src/mcp.rs`, dari `ConnectionRecord` ke `Settings`) dan diuji sebagai fungsi murni. | UC-01, UC-15 | turunan | W11-T3 |
| FR-CON-06 | Tag lingkungan per koneksi: tidak ada, `dev`, `staging`, atau `prod`. Tag dibaca dengan `decodeIfPresent`. Tag ini hanya presentasi dan tidak mengubah Safe Mode di engine. | UC-01, UC-08 | must #6 | W9-T4 |
| FR-CON-07 | Auth JWT untuk Trino: `Authorization: Bearer` hanya lewat HTTPS. Token disimpan di Keychain dan disensor oleh `Debug`. | UC-18 | should | W11-T2, W11-T3 |
| FR-CON-08 | Berkas CA per koneksi (PEM) untuk PostgreSQL dan Trino. Bila diisi, kepercayaan menjadi bundel itu saja, dan hostname tetap dicek. Untuk MySQL kolom ini tidak ditawarkan, dan batasnya dijelaskan (§12.1). | UC-18 | should | W11-T2, W11-T3 |
| FR-CON-09 | Form menyebut field yang kurang dengan kata ("Host dan User wajib diisi"). Test Connection pindah dari ⌘T ke ⌘↩. | UC-01 | audit §2 P1 | W11-T3 |

### 5.2 Pohon skema dan metadata

| ID | Kebutuhan | UC | Sumber | Tugas |
|---|---|---|---|---|
| FR-TREE-01 | Pohon menampilkan tabel, view, dan materialized view dengan label di PostgreSQL, serta tabel dan view dengan label di MySQL dan Trino. Engine mengirim jenis objek hanya bila app memasang setelan baru, sehingga golden tidak berubah. | UC-02 | must #5 | W11-T1, W11-T4 |
| FR-TREE-02 | Perintah read-only `columns` (nama, tipe, nullable, default). Kolom tampil sebagai anak node tabel dan dimuat saat node dibuka. | UC-02, UC-03 | must #5 | W11-T1, W11-T4 |
| FR-TREE-03 | Perintah read-only `ddl`: `pg_get_viewdef` untuk view PostgreSQL, DDL tabel PostgreSQL disusun dari katalog, serta `SHOW CREATE` di MySQL dan Trino. Hasilnya terbuka di tab read-only dengan highlighter editor. | UC-02 | must #5 | W11-T1, W11-T4 |
| FR-TREE-04 | Pohon dibangun ulang sebagai `NSOutlineView` dengan visual yang sama. Mendukung ←/→, type-select, Return untuk membuka, dan semantik outline VoiceOver. | UC-02 | audit §3 P0 dan P1 | W9-T3 |
| FR-TREE-05 | Tabel bisa di-drag dari pohon ke editor untuk menyisipkan nama berkualifikasi. | UC-02 | audit HIG P1 | W9-T3 |

### 5.3 Editor

| ID | Kebutuhan | UC | Sumber | Tugas |
|---|---|---|---|---|
| FR-ED-01 | Formatter per dialek di `qh-sql`, ditulis sendiri. Deretan token non-spasi sebelum dan sesudah format identik. Hasilnya idempoten. Komentar, string, dollar-quote, dan `:name` utuh. Mengikuti setelan keyword case. | UC-03 | must #3 | W12-T1, W12-T2 |
| FR-ED-02 | Toggle comment (`-- `) untuk baris atau seleksi, bisa di-undo sebagai satu langkah. | UC-06 | must #3 | W12-T2 |
| FR-ED-03 | Save dan Save As `.sql`. Tab terikat berkas dengan penanda kotor. Perubahan dari luar dideteksi saat tab aktif dan sebelum menyimpan, lalu app bertanya dan tidak menimpa diam-diam. Session restore membuka ulang berkas tanpa menjalankannya. | UC-06, UC-16 | must #3 | W12-T2 |
| FR-ED-04 | Berkas `.sql` yang di-drop ke jendela terbuka sebagai tab. | UC-06 | audit HIG P1 | W12-T2 |
| FR-ED-05 | Autocomplete kolom dari katalog dengan resolusi alias di statement aktif. Kolom yang belum dimuat diambil asinkron lewat sesi metadata dan disimpan di `TreeNode`, satu-satunya cache (`performance-plan.md` §13). Ketikan tidak pernah menunggu. | UC-03 | must #5 | W11-T6 |
| FR-ED-06 | Diagnostik: posisi galat dari server (PostgreSQL `position`, Trino `errorLocation`, dan baris MySQL bila ada) serta masalah leksikal (kutip atau komentar yang tidak tertutup) digambar sebagai garis bawah. Ada rotor VoiceOver "Query issues" dan readout di status. | UC-03 | audit §4 P1 | W10-T6 |
| FR-ED-07 | Ukuran font editor bisa diatur di Settings, dengan ⌘+, ⌘−, dan ⌘0. | — | audit §4, §14 P1 | W9-T7, W10-T7 |
| FR-ED-08 | Pasangan kurung dan kutip ditandai. | — | audit §4 (keputusan P-18) | W10-T7 |
| FR-ED-09 | Aksesibilitas `NSTextView` tetap utuh, dan hook rotor terpasang sejak rewrite 4B. | — | audit §4 P0 | W4-T2 |
| FR-ED-10 | Gutter pada kanvas terang digambar sebagai cekungan (recess), bukan lift putih 3,5%. | — | audit §4 (keputusan P-18) | W10-T7 |

### 5.4 Eksekusi, skrip, dan umpan balik

| ID | Kebutuhan | UC | Sumber | Tugas |
|---|---|---|---|---|
| FR-RUN-01 | Perintah engine `script`: memecah dengan `qh-sql` lalu menjalankan statement satu per satu. Kebijakan `stop` (default) atau `continue`. Tanpa transaksi implisit. Safe Mode per statement, dengan konfirmasi yang menyebut semua write sekaligus. Stop sampai ke server. Setiap statement melaporkan indeks, baris awal, elapsed, rows/affected, dan galat. | UC-06 | must #4 | W12-T3, W12-T4 |
| FR-RUN-02 | Setiap statement yang mengembalikan baris mendapat tab hasilnya sendiri (handle store, dibatasi `rowLimit` dan anggaran global). Hasil bisa di-pin supaya bertahan saat Run ulang. | UC-06 | §1.4, must #4 | W12-T4 |
| FR-RUN-03 | Satu entri history per skrip, berisi teks penuh dan ringkasan hasil. | UC-06 | adoption §1.4 | W12-T4 |
| FR-RUN-04 | Notifikasi sistem bila export, `to_table`, atau skrip berjalan > 20 dtk dan selesai saat app tidak di depan. Pengumuman VoiceOver saat Run selesai, gagal, atau dibatalkan. | UC-06, UC-11 | audit §12 P1 | W12-T6 |
| FR-RUN-05 | `NSProgress` per berkas ekspor, sehingga progres terlihat di Finder, plus progres atau badge di Dock untuk export dan `to_table`. | UC-11 | audit HIG P1 | W12-T6 |
| FR-RUN-06 | Galat tampil sebagai banner inline di atas hasil sebelumnya. Teksnya bisa diseleksi, dengan tombol Copy, Dismiss, dan "Tampilkan di Log". | UC-07 | audit §12 P0 | W9-T5 |
| FR-RUN-07 | Lembar konfirmasi write: Esc membatalkan, gaya destruktif coral, kata kerja bernama ("Jalankan 2 Write"), dan Return tidak menjadi default bila prompt dimunculkan engine. Aturan yang sama berlaku untuk Truncate dan Drop. | UC-08 | audit §11 P0 | W9-T5, W9-T3 |

### 5.5 Grid hasil

| ID | Kebutuhan | UC | Sumber | Tugas |
|---|---|---|---|---|
| FR-GRID-01 | Grid `NSTableView` dengan sel yang digambar, di belakang seam `ResultRows`. Seluruh daftar paritas fitur `performance-plan.md` §9 terpenuhi. | UC-04 | must #2, Fase 5 | W5-T1 |
| FR-GRID-02 | Paginasi. "Ambil lebih banyak" menaikkan cap bertingkat (1k → 10k → 100k → 1M → plafon). "Ambil semua" langsung ke plafon, dengan peringatan bahwa statement dijalankan ulang (di Trino disebut sebagai biaya gudang). "Ke baris…" melompat ke baris tertentu. `rowLimit` tetap satu-satunya cap. | UC-04 | must #2 | W13-T1 |
| FR-GRID-03 | Sort server-first untuk hasil terpotong **dan** lengkap. View in-memory Rust hanya dipakai pada tiga kasus fallback Fase 3 dan untuk "off". Sumber indikator header hanya satu (`tab.activeSort`). Galat server ditampilkan, tidak diganti fallback diam-diam. Sort atau search ditolak bila ada edit yang di-staged. | UC-04 | Batch 7 = Fase 3 | W4-T1, W6-T1 |
| FR-GRID-04 | Search server-first: debounce 250 ms, minimal 3 karakter, dan search yang sedang jalan dibatalkan. Filter funnel per kolom tetap in-memory atas baris yang diambil (di Rust setelah Fase 6), dengan banner jujur bila hasilnya terpotong. | UC-04 | Fase 3, 6 | W4-T1, W6-T1 |
| FR-GRID-05 | "Tampilkan SQL" untuk sort dan search server, memakai lembar tinjauan yang sudah ada. | UC-04 | audit §8 P1 | W13-T1 |
| FR-GRID-06 | Kursor sel keyboard: panah, ⇧ untuk memperluas, ⌘⇧ untuk tepi, ↩ untuk edit, Esc, Tab/⇧Tab, ⌘C, dan ⌘⇧C dengan header. Space membuka panel peek bergaya Quick Look tanpa berkas sementara. | UC-04, UC-05 | audit §5 P0, HIG P1 | W10-T1 |
| FR-GRID-07 | VoiceOver per sel ("baris r, kolom c, ‹header›: nilai, ‹status staged›"). Header menyebut status sort. "N × M dipilih" diumumkan saat seleksi selesai. | UC-04 | audit §5 P0, Fase 5.7 | W5-T1, W10-T1 |
| FR-GRID-08 | Lantai kontras: nomor baris dan NULL ≥ 4,5:1, funnel dan separator ≥ 3:1, ditambah varian Increase Contrast. | — | audit §5 P0 | W10-T2 |
| FR-GRID-09 | Tata bahasa perubahan staged. Diubah: amber dengan titik. Disisipkan: mint dengan tanda plus. Dihapus: coral dengan coretan. Tidak ada status yang hanya ditandai warna. | UC-09 | audit §5 P0 | W10-T2 |
| FR-GRID-10 | Gestur Add Row dan Delete Row (tombol, menu konteks, dan kunci) yang mengisi `CellEdits` dan `WritePlan` yang sudah ada. | UC-09 | must #7 | W10-T3 |
| FR-GRID-11 | Commit menjadi `PillButton` berlabel "Tinjau N Perubahan…" dengan ⌘S saat grid fokus. Undo tersambung ke undo manager jendela (⌘Z). | UC-09 | audit §5 P0 | W10-T3 |
| FR-GRID-12 | Salin sebagai INSERT sesuai dialek. Nama tabel diambil dari konteks "Open table", atau ditanyakan bila tidak diketahui. | UC-09 | should | W10-T4 |
| FR-GRID-13 | Mode Record di panel samping: semua kolom baris aktif sebagai pasangan label dan nilai, dengan chip tipe, tanda staged, dan cari nama field. Klik pada field memfokuskan sel di grid. | UC-05 | should, audit §6 P1 | W10-T5 |
| FR-GRID-14 | Area klik funnel setinggi header penuh × 20 pt di tepi kanan. | — | audit §8 P1 | W10-T2 |
| FR-GRID-15 | Seleksi grid bisa di-drag keluar sebagai file promise CSV. | — | audit HIG P1 | W10-T4 |

### 5.6 Rencana query

| ID | Kebutuhan | UC | Sumber | Tugas |
|---|---|---|---|---|
| FR-PLAN-01 | Engine: `EXPLAIN (FORMAT JSON)` dan opsi `ANALYZE` untuk PostgreSQL, `EXPLAIN (FORMAT JSON)` untuk Trino, keduanya lewat setelan baru (golden tetap). `ANALYZE` diklasifikasikan Safe Mode sebagai statement di dalamnya. MySQL tidak berubah. | UC-10 | should | W13-T2 |
| FR-PLAN-02 | Pohon rencana dengan cost, rows, dan waktu per node, node terpanas disorot, dan sakelar Tree/Raw. Plan mentah tetap tampil di grid yang sama (PR-14). | UC-10 | should | W13-T3 |

### 5.7 Keselamatan

| ID | Kebutuhan | UC | Sumber | Tugas |
|---|---|---|---|---|
| FR-SAFE-01 | Tingkat Safe Mode dan lingkungan tampil sebagai glyph plus label di breadcrumb, chip tab, dan status bar. Tint prod bersifat opsional. | UC-08 | must #6, audit P0 #3 | W9-T4 |
| FR-SAFE-02 | Penampil execution log read-only: daftar, filter per koneksi, dan status verifikasi rantai hash. Statement tampil sebagai hash, sesuai desain migrasi `0007`. | UC-08 | should | W11-T1, W13-T6 |
| FR-SAFE-03 | Aktivitas server. PostgreSQL: `pg_stat_activity` plus `pg_blocking_pids`. MySQL: processlist dan lock wait bila hak aksesnya ada. Trino: `system.runtime.queries`. Cancel query saja, dengan guard sendiri: ditolak di `read_only`, butuh konfirmasi di `confirm`, dan floor ADR-0027 dihormati. Setiap cancel dicatat ke execution log. | UC-07, UC-17 | should | W13-T4, W13-T5 |

### 5.8 Impor dan MCP

| ID | Kebutuhan | UC | Sumber | Tugas |
|---|---|---|---|---|
| FR-IMP-01 | Impor `.sql` dari app lewat `import_data` dengan sumber Statements dan kebijakan ADR-0019/0022. | UC-13 | should | W12-T5 |
| FR-IMP-02 | Impor JSON (array objek) dan JSONL, dengan mapping kolom di `ImportSheet`. JSONL mengalir, dan array di-parse per elemen. | UC-13 | should (P-19) | W12-T5 |
| FR-MCP-01 | Tool MCP `describe_table` dan `table_ddl`, memakai scope, allowlist, dan `read_only` yang sama. `docs/mcp-stability.md` diperbarui. | UC-15 | must #5 | W11-T5 |

### 5.9 Shell, keyboard, dan aksesibilitas

| ID | Kebutuhan | Sumber | Tugas |
|---|---|---|---|
| FR-UI-01 | Chip tab menjadi `Button` dengan trait `.isSelected`. Tahap run ditandai bentuk (spinner, tanda seru) dan nilai AX. Tombol tutup yang tak terlihat keluar dari pohon AX. | audit §1 P0 | W9-T1 |
| FR-UI-02 | ⌘1–9, ⌃Tab/⌃⇧Tab, dan ⌘⇧[ / ]. Ada tes yang menjamin tidak ada dua aksi berbagi kunci dalam satu skema, sekaligus memperbaiki ⌘E ganda (§12.2). | audit §1 P0 | W9-T2 |
| FR-UI-03 | Menu View: Toggle Sidebar ⌃⌘S, Toggle Result Panel, Focus Editor/Grid/Tree, Next/Previous Tab. | audit §16 P0 | W9-T2 |
| FR-UI-04 | Setiap `IconButton` dan `Chip` punya `accessibilityLabel` eksplisit, misalnya "tipe varchar", "Stop", "Commit". | audit §15 P0 | W9-T1 |
| FR-UI-05 | Satu modifier permukaan yang membaca `accessibilityReduceTransparency` dan `colorSchemeContrast`: isian solid, hairline sekitar 0,20, secondary sekitar 0,85, dan glow mati. Animasi `HiveHero` dan `Backdrop` tunduk pada Reduce Motion. | audit §13, §10 P0 | W9-T1 |
| FR-UI-06 | Lantai teks 11 pt. 10 pt hanya untuk label dekoratif huruf besar dengan ink ≥ 0,68. Ukuran font grid bisa diatur. | audit §14 P1 | W9-T9, W9-T7 |
| FR-UI-07 | Salinan basi diperbaiki: teks `EmptyWorkspace`, kalimat sidebar "Add a Trino coordinator", dan placeholder `hive.analytics.penerima_manfaat`. | audit §10 P0 | W9-T1, W9-T3 |
| FR-UI-08 | Shell native: split view dengan divider yang bisa diubah lewat keyboard, dan toolbar yang memuat breadcrumb serta Run. Ukuran minimum jendela diukur ulang. | audit §1 P1 | W9-T8 |
| FR-UI-09 | Open Quickly: baris yang disorot di-scroll agar terlihat, lingkup All/Objects/Saved/History, dan hasil sebagai list AX dengan baris terpilih. | audit §16 P1 | W9-T6 |
| FR-UI-10 | Judul jendela Settings mengikuti pane. `HelpHint` bisa dijangkau keyboard dan VoiceOver. Ukuran font editor dan grid diatur di sini. | audit §9 P1 | W9-T7 |
| FR-UI-11 | Fondasi lokalisasi: string catalog di bundel dan helper. String di permukaan yang disentuh W9–W13 lewat katalog. | audit P1 | W13-T7 |

### 5.10 Performa sebagai perilaku

| ID | Kebutuhan | Fase | Tugas |
|---|---|---|---|
| FR-PERF-01 | Satu runtime per proses. Cancel preview, explain, dan count sampai ke server (`session.cancel()` dibatasi 250 ms, lalu sesi ditutup). | 1 | W2-T1 |
| FR-PERF-02 | Antrean Swift `.userInitiated`. `rowLimit` dibatasi 200.000 sampai Fase 6. | 1 | W2-T2 |
| FR-PERF-03 | `EngineHost` dengan pool 2 sesi query + 1 sesi metadata per kunci, reset per Run, warm-up saat koneksi dipilih, tunnel dibagi per kunci, dan handle SQLite lokal yang tetap terbuka. | 2 | W3-T1 |
| FR-PERF-04 | Jalur panas editor Swift, lalu analisis di Rust. Pewarnaan dan folding hidup sampai 2.000.000 unit UTF-16. | 4 | W2-T3, W3-T2, W4-T2 |
| FR-PERF-05 | Data plane result store dengan anggaran global 256 MB. Spill terenkripsi dengan kunci efemeral per proses, berkas `0600`, dan disapu saat startup. Plafon naik ke 5.000.000. | 6 | W4-T3, W5-T2, W6-T1 |
| FR-PERF-06 | Plafon ingest: builder bebas alokasi dan COPY. Opsi bersyarat diadopsi bila angkanya lolos. | 7 | W7-* |
| FR-PERF-07 | `--bench`, signpost, `bench_ffi`, microbench Swift, dan harness `qhbench`. | 0 | W1-* |

## 6. Kebutuhan non-fungsional

### 6.1 Performa (NFR-P)

Target diambil dari `performance-plan.md` §2. Angka hari ini ada di sana dan di `docs/benchmarks.md`. Kolom "vs TablePro" hanya dinilai bila TablePro terukur (keputusan P-04).

| ID | Sumbu | Target absolut | vs TablePro | Gate di fase |
|---|---|---|---|---|
| NFR-P1 | TTFR sampai baris pertama tergambar | S1 hangat cap 1k/10k: p50 ≤ 25 ms, p95 ≤ 40 ms. S2 cap 500k: p95 ≤ 50 ms. S3 RTT 30 ms: hangat ≤ 1 RTT + 20 ms. | S1 ≤ 0,5×; S2 ≤ 0,1×; S3, S4 ≤ 1,0× | 2 (S1, S3, S4), 6 (S2) |
| NFR-P2 | Baris/s ke grid | ≥ 575.000 baris/s, atau ≥ 80% plafon COPY hari yang sama bila plafonnya lebih rendah. Interim Fase 6: ≥ 380.000. | ≥ 1,5× (termasuk Trino lineitem) | 6 (interim), 7 |
| NFR-P3 | Memori puncak | ≤ anggaran store (256 MB) + 64 MB, berapa pun jumlah barisnya | ≤ 0,5× | 6 |
| NFR-P4 | Frame saat scroll | hitch ≤ 1 ms/s; p99 frame ≤ 8,3 ms (120 Hz); hasil 500 kolom tergambar ≤ 30 ms | hitch ≤ 1,0× | 5 |
| NFR-P5 | Latensi ketikan | main thread p99 ≤ 4 ms (10k baris) dan ≤ 8 ms (2M karakter), dengan pewarnaan menyala | input-to-photon p95 ≤ 1,0× | 4 |
| NFR-P6 | Latensi cancel | PostgreSQL/MySQL p95 ≤ 100 ms; Trino ≤ 300 ms; UI berhenti ≤ 1 frame | ≤ 1,0× | 1 |
| NFR-P7 | Cold start | hangat ≤ 400 ms; dingin ≤ 1 dtk | ≤ 1,0× | 2 (tidak mundur), 8 |
| NFR-P8 | Sekunder | Perintah lokal p50 ≤ 2 ms. Level pohon hangat ≤ 1 RTT + 10 ms. Introspeksi 5.000 tabel < 1 dtk. Fallback sort 500k: numerik ≤ 100 ms, teks ≤ 300 ms, off-main. `window` p99 ≤ 0,5 ms. Nol leak dan nol spill tersisa setelah 100× buka/tutup tab. | — | 1, 2, 6 |
| NFR-P9 | Penjaga regresi | Setiap gelombang fitur (W9–W13) tidak memundurkan sumbu 1, 4, atau 5 lebih dari 5% pada subset `--bench` | — | W9–W13 |

**Pengukuran.** Angka absolut QueryHive **wajib** dan diambil lewat signpost in-app dan `--bench`. Jalur ini tidak butuh izin OS. Harness black-box `qhbench` butuh izin Screen Recording dan Accessibility, dan koneksi TablePro harus disetel tangan. Keduanya tidak bisa diberikan run otonom. Karena itu head-to-head bersifat *best-effort*, dan bila terhalang ditulis `tidak diukur (izin OS)`. Start dingin butuh `sudo purge`, sehingga ditulis `tidak diukur (butuh sudo)` dan diserahkan ke pemilik.

### 6.2 Aksesibilitas (NFR-A)

- **NFR-A1 Kontras.** Teks yang harus dibaca ≥ 4,5:1 terhadap kanvas di ketujuh kanvas. Pengecualiannya hanya token `comment` di editor gelap, yang memang dirancang surut. Komponen UI (funnel, separator, fokus) ≥ 3:1. Ada varian Increase Contrast. Dijaga tes kontras seperti `ReadoutColourTests`.
- **NFR-A2 Jangkauan keyboard.** Pohon, tab, editor, grid, panel, Open Quickly, dan semua sheet bisa dicapai dan dioperasikan tanpa pointer.
- **NFR-A3 VoiceOver.** Tidak ada `IconButton` atau `Chip` tanpa label (diuji). Sel grid dan header diumumkan (FR-GRID-07). Pohon punya semantik outline. Status tab tidak disampaikan lewat warna saja. Ada rotor diagnostik.
- **NFR-A4 Pengaturan sistem.** Reduce Motion, Reduce Transparency, dan Increase Contrast dihormati dari satu tempat (FR-UI-05).
- **NFR-A5 Tipografi.** Lantai 11 pt (FR-UI-06).
- Smoke manual VoiceOver dan IME dijalankan pemilik dari laporan akhir. Run otonom hanya menjalankan tes pohon AX.

### 6.3 Lisensi (NFR-L)

- Tidak ada kode, aset, atau string TablePro. Tidak ada vendoring dari `TablePro/Packages/TableProEditor`.
- Setiap crate baru lulus `cargo deny check licenses` dengan allow-list ADR-0002. Yang direncanakan: `rayon`, `unicode-segmentation`, dan bersyarat `mimalloc`. `ring` dan `memchr` sudah ada di `Cargo.lock` (§12.3).
- Paket SwiftPM ditinjau manual. Tidak ada yang direncanakan.
- Build TablePro hanya untuk pengukuran lokal, di luar repo, dan binarinya tidak pernah didistribusikan.

### 6.4 Keselamatan dan keamanan (NFR-S)

- **NFR-S1.** Setiap perintah baru (`columns`, `ddl`, `execution_log`, `script`, `sessions`, `session_cancel`) dan setiap setelan baru (EXPLAIN ANALYZE) punya tes di `crates/qh-ffi/tests/safe_mode.rs`. Floor ADR-0027 tetap monoton.
- **NFR-S2.** Tidak ada kredensial (password, passphrase, JWT) di log, galat, event, keluaran MCP, atau kunci pool. Versi kredensial masuk sidik jari pool sebagai hash.
- **NFR-S3 Spill.**
  - Kripto: AEAD `ring` (AES-256-GCM) dengan kunci acak per proses yang hanya ada di memori. Nonce adalah penghitung per chunk dan tidak pernah dipakai ulang dengan kunci yang sama. AAD berisi id store, indeks chunk, dan generation.
  - Berkas: direktori `0700` dan berkas `0600`. Berkas yatim disapu saat startup.
  - Tes: teks biasa tidak muncul di berkas; kunci salah dan manipulasi berkas gagal; disk penuh menjadi event galat.
- **NFR-S4.** Host key SSH mengikuti FR-CON-03. Tidak ada terima diam-diam.
- **NFR-S5.** Tidak ada data hasil yang ditulis ke disk selain lewat ekspor yang diminta pengguna, spill terenkripsi, atau file promise CSV yang di-drop pengguna. Panel peek tidak memakai berkas sementara.
- **NFR-S6.** MCP tetap proses terpisah, fail-closed, dan dipaksa `read_only`.

### 6.5 Paritas visual (NFR-V)

- **Tiga lapis** (`performance-plan.md` §4.8): geometri identik secara numerik; warna identik di titik sampel; teks dalam toleransi (≤ 0,1% piksel dengan delta kanal > 16/255 per scene). Atribut editor harus identik per rentang.
- Baseline PNG di-commit di `app/Tests/QueryHiveTests/__Baselines__/`, direkam dari commit P.
- Fase performa tidak boleh mengubah piksel, kecuali V-1 di bawah.
- **Perubahan yang disengaja** hanya yang terdaftar di bawah. Scene lain harus lulus tanpa perubahan.

| Kode | Perubahan | Tugas |
|---|---|---|
| V-1 | Banner sort Batch 7 | W4-T1 |
| V-2 | Glyph tahap pada chip tab, salinan `EmptyWorkspace`, placeholder | W9-T1 |
| V-3 | Lencana Safe Mode dan lingkungan | W9-T4 |
| V-4 | Banner galat inline dan lembar konfirmasi | W9-T5 |
| V-5 | Cincin fokus dan seleksi pohon outline | W9-T3 |
| V-6 | Lingkup Open Quickly, judul dan help Settings | W9-T6, W9-T7 |
| V-7 | Shell native | W9-T8 |
| V-8 | Lantai 11 pt | W9-T9 |
| V-9 | Kursor sel, kontras grid, tanda staged, funnel, tombol tinjau | W10-T1…T3 |
| V-10 | Garis bawah diagnostik, kurung, gutter terang | W10-T6, W10-T7 |
| V-11 | Permukaan baru tanpa baseline lama: record, pohon plan, aktivitas, audit, footer paginasi, tab hasil skrip, bagian SSH, tab DDL | W10–W13 |

- Setiap rekam ulang punya commit sendiri. Baseline lama tetap bisa diambil dari riwayat git. Pemilik meninjau pasangan berdampingan di laporan akhir (keputusan O-10, P-01).

### 6.6 Kontrak (NFR-C)

- NDJSON `rows` untuk CLI, MCP, dan golden tidak berubah. Keluaran baru hanya muncul bila app memasang setelan baru (keputusan P-06), atau lewat perintah baru.
- Menambah perintah menyentuh empat daftar invariant #11 sekaligus (`COMMANDS` dan `Command` di `crates/qh-ffi/src/lib.rs`, `EngineCommand`/`EVERY_COMMAND` di `uniffi_api.rs`, `RustEngine.commands`), dan `swift test` menjadi penjaganya.
- `app/Generated/` diregenerasi dan di-commit bersama perubahan Rust-nya (invariant #1).
- Satu ADR per keputusan yang mengubah kontrak. ADR adalah catatan, bukan kontrak.
- Deployment target macOS 14.0 di tiga tempat tidak berubah (invariant #4).

### 6.7 Kualitas (NFR-Q)

- Gate berat (`tablepro-adoption-plan.md` §0 ditambah `performance-plan.md` §0) hijau di setiap penutupan gelombang.
- Hitungan tes Rust dan Swift tidak turun.
- Golden live tidak punya selisih yang belum terklasifikasi di `docs/golden-deltas.md`.
- Angka hanya masuk `docs/benchmarks.md` lewat generator. Yang belum diukur ditulis `[belum diukur]`, `tidak diukur (izin OS)`, atau `tidak mendukung`.

## 7. Keunggulan yang dijaga

| ID | Keunggulan | Penjaganya |
|---|---|---|
| PR-01 | Ekspor streaming dengan memori terbatas (500k × 30, 9 MB) | Jalur ekspor tidak memakai store. Bench ekspor tidak naik > 10% RSS. |
| PR-02 | `to_table` di server | Golden `to_table` tidak berubah. Operasi panjang tidak memakai pool (P-05). |
| PR-03 | Penulis Parquet native, memori per row group | Tes `qh-export` dan pembaca pyarrow tetap lulus |
| PR-04 | Safe Mode ditegakkan engine, dengan floor dan execution log | NFR-S1 |
| PR-05 | Timeout statement ditegakkan server | Tes "perubahan timeout antar-run dihormati" di Fase 2, dan addendum ADR-0016 |
| PR-06 | MCP proses terpisah, fail-closed, read-only | `tests/mcp.rs` dan `tests/mcp_stdio.rs`, termasuk tool baru |
| PR-07 | Kebijakan transaksi impor, CSV benar-benar streaming | Tes impor, termasuk JSONL |
| PR-08 | Klien Trino yang dalam: query id, capabilities, presisi timestamp | Golden `type_zoo` Trino |
| PR-09 | Satu permukaan engine untuk app, CLI, dan MCP, dengan golden | NFR-C |
| PR-10 | Keadaan grid yang jujur: "First N rows · limit reached", Count all, tiga kalimat kosong, banner | Tes string dan scene paritas |
| PR-11 | Kosakata warna kategorikal terlindungi, dan palet sintaks terang terukur | Tes kontras, `app/DESIGN.md` §Appearance |
| PR-12 | Filter kolom yang bentuknya ditentukan data (picker ≤ 10 nilai) | Daftar paritas Fase 5 |
| PR-13 | Breadcrumb dengan level lebar tetap | Scene paritas, dan W9-T8 menjaganya di toolbar |
| PR-14 | Explain di grid yang sama | Mode Raw tetap ada di samping pohon |
| PR-15 | Harness `--snapshot` dengan 60+ scene | Diperluas, tidak diganti. Menjadi dasar gate paritas. |
| PR-16 | Session restore tanpa eksekusi ulang | Tes di W12-T2 |

## 8. Metrik keberhasilan

1. **Performa:** ketujuh sumbu punya angka QueryHive di `deploy/dev/bench-results.jsonl`, dan target absolutnya terpenuhi. Target vs TablePro terpenuhi di setiap sumbu yang terukur.
2. **Fitur:** setiap UC bertanda Masuk atau Sebagian punya bukti otomatis: tes engine, tes app, atau scene snapshot. Pemetaannya ada di laporan akhir.
3. **Aksesibilitas:** nol kontrol ikon tanpa label, tes kontras lulus, tes fokus keyboard lulus untuk pohon, grid, editor, dan tab, dan tes pohon AX grid serta pohon lulus.
4. **Paritas:** semua scene non-V lulus toleransi. Semua perubahan V punya pasangan berdampingan.
5. **Kualitas:** gate berat hijau di `main` lokal setelah merge. Nol selisih golden yang belum terklasifikasi. Nol leak setelah 100× buka/tutup tab.

## 9. Kriteria rilis (merge ke `main` lokal)

- Gate berat hijau di kepala branch kerja, lalu sekali lagi di `main` setelah merge.
- `./app/build.sh` menghasilkan bundel yang bisa diluncurkan, dan semua scene `--snapshot` merender tanpa galat.
- Laporan benchmark diregenerasi, dan setiap sumbu punya angka QueryHive.
- Laporan paritas visual tersedia, termasuk pasangan untuk setiap perubahan V.
- Dokumen diperbarui: `PROGRESS.md`, `app/DESIGN.md`, `docs/benchmarks.md` (lewat generator), status di rencana-rencana terkait, dan ADR.
- Tugas yang terblokir tercantum beserta buktinya. Tidak ada tugas yang diam-diam hilang.
- Tidak ada push, PR, atau rilis. Persetujuan akhir dari pemilik.

## 10. Di luar lingkup

Hal-hal berikut didokumentasikan tetapi tidak dibangun.

- **Later di peta fitur:**
  - query insights, rutin dan UDT, maintenance, backup/restore, copy object;
  - shortcut yang bisa di-rebind, Vim;
  - credential profiles, SSH profiles;
  - UI token MCP, retensi history;
  - AI assistant;
  - frecency Open Quickly, link ke UI Trino.
- **Skip di peta fitur:**
  - engine tambahan, plugin ABI;
  - iCloud, iOS, lisensi;
  - chart dan map, ER diagram;
  - compare dan sync;
  - editor users & roles, structure editor;
  - Data Rewind, editor tema;
  - URL scheme, AppleScript, Raycast;
  - tunnel cloud (Cloudflare, Cloud SQL, AWS IAM), SOCKS5, tunnel command.
- **P2 audit desain:**
  - tampilan Structure (Data | Structure dengan indeks);
  - recents di workspace kosong;
  - App Intents, Spotlight, Services, Share/Print;
  - reorder dan tear-off tab, preview tab.
- **Batas yang dinyatakan:**
  - perbandingan plan (UC-10 langkah 4);
  - terminate backend (hanya cancel);
  - WHERE builder di server untuk filter kolom;
  - `ProxyJump`;
  - Kerberos/OAuth2 Trino;
  - CA per koneksi untuk MySQL;
  - default row sort, deteksi nilai per sel, format tanggal global.
- **Proses:**
  - Fase 8, kecuali gate gagal;
  - notarisasi, DMG, push, PR;
  - build universal atau Intel.

## 11. Log keputusan

### 11.1 Keputusan pemilik (final)

| ID | Keputusan |
|---|---|
| O-1 | **Lingkup:** Fase 0–7 dikerjakan (Fase 8 hanya bila gate gagal), ditambah semua must-have dan should-have peta fitur, serta P0 dan P1 audit desain. Later, P2, dan skip hanya didokumentasikan. |
| O-2 | **Urutan:** performa dulu, baru desain dan fitur. Pekerjaan di grid dan editor dibangun di atas grid Fase 5 dan editor Fase 4, tidak pernah di atas yang lama. |
| O-3 | **Eksekusi:** orkestrator menjalankan semuanya secara otonom, dan pemilik menerima satu laporan akhir. |
| O-4 | **Git:** orkestrator (bukan subagen) meng-commit WIP pemilik sebagai commit sendiri di `main`, lalu bekerja di satu branch lokal dengan satu commit per tugas atau fase yang lulus gate, dan merge ke `main` lokal di akhir. Tidak ada push atau PR. |
| O-5 | ADR adalah catatan, bukan kontrak. |
| O-6 | **Sesi:** di-reset setiap Run. |
| O-7 | **Pool:** 2 sesi query + 1 sesi metadata = 3 per kunci. Ini menggantikan "maksimum 4 per kunci" di `performance-plan.md` §6.2. **2+1 dibaca sebagai kapasitas sesi yang dipertahankan (reservasi tanpa antrean), bukan batas koneksi ke server.** Lonjakan di atas 2+1 membuka sesi seperti hari ini dan tidak disimpan (verdict AR `blueprints/fase-2-engine-host.md`, ditulis terang di ADR-0031). |
| O-8 | **Batch 7:** sort dan search server-first adalah default untuk hasil terpotong **dan** lengkap. View in-memory Rust hanya fallback (tiga kasus Fase 3) dan penyimpan "off". Tidak ada pintasan in-memory untuk hasil lengkap. Tiga pertanyaan Batch 7 dijawab oleh `performance-plan.md` §7: "off" memakai hasil dasar yang disimpan, indikator memakai `tab.activeSort` tunggal, dan fallback hanya pada tiga kasus dengan galat server tidak disamarkan. |
| O-9 | **Kolasi fallback:** kunci natural di Rust, dengan divergensi terhadap `localizedStandardCompare` yang dicatat dari tes diferensial. |
| O-10 | **Paritas visual:** geometri identik, teks dalam toleransi (≤ 0,1% piksel), dan pemilik meninjau berdampingan di laporan akhir. |
| O-11 | **Baseline:** PNG di-commit. |
| O-12 | **Batas memori dan spill:** clamp `rowLimit` 200.000 sampai Fase 6, lalu 5.000.000. Ini menggantikan angka 100.000 di `performance-plan.md` §5.4. Anggaran store 256 MB global. Spill menyala, dienkripsi dengan kunci efemeral per proses, berkas `0600`, dan disapu saat startup. |
| O-13 | **TablePro:** dibangun lokal pada commit yang dipatok, hanya untuk pengukuran. Angkanya masuk `docs/benchmarks.md`. `qhbench` butuh izin OS, jadi head-to-head bersifat best-effort dan ditulis `tidak diukur (izin OS)` bila terhalang. |

### 11.2 Keputusan perencana

Keputusan ini diturunkan dari keputusan pemilik dan kode. Pemilik bisa membatalkannya di laporan akhir.

| ID | Keputusan | Alasan |
|---|---|---|
| P-01 | **Rekam ulang baseline hanya untuk perubahan V**, satu commit per rekam ulang, dan ditinjau pemilik di laporan akhir. Persetujuan "sebelum rekam ulang" di `performance-plan.md` §4.8 dan §7 dipindah ke laporan akhir. | O-3: tidak ada titik henti di tengah run |
| P-02 | **VM podman diperbesar secara otonom** dengan pagar pengaman (`development-plan.md` §2) | Hanya menyangkut container dev sekali pakai. VM 2 CPU / 1,86 GiB tidak muat Trino, dan server menjadi batas di bench throughput. |
| P-03 | **Angka in-app wajib, black-box best-effort.** Start dingin diserahkan ke pemilik (`sudo purge`). | Izin TCC dan sudo tidak bisa diberikan run otonom |
| P-04 | Klausul "≤ TablePro" hanya dinilai bila TablePro terukur. Selebihnya gate memakai target absolut dan mencatat alasannya. | Kebijakan `performance-plan.md` §4 Risiko |
| P-05 | **Pool hanya untuk perintah interaktif:** preview, explain, count, browse, metadata, `apply_changes`, dan aktivitas. Export, `to_table`, `import_data`, dan `script` membuka sesi sendiri tetapi berbagi `Arc<Tunnel>`. | Dengan 2 sesi query, satu ekspor panjang akan membuat Run menunggu. Ongkos connect tidak berarti untuk operasi berdurasi menit. Dicatat di ADR-0031. Tafsiran 2+1 (reservasi, bukan batas koneksi server): lihat O-7. |
| P-06 | **Keluaran baru hanya lewat setelan yang dipasang app** (jenis objek, detail galat, format EXPLAIN) atau lewat perintah baru | Golden dan CLI tetap beku (NFR-C) |
| P-07 | **Paginasi = menaikkan satu cap** dengan menjalankan ulang. Cursor tidak ditahan terbuka. | Cursor yang diparkir menahan snapshot atau transaksi di prod. Portal cap adalah eskalasi Fase 8. |
| P-08 | **CA per koneksi** hanya untuk PostgreSQL dan Trino, dan bila diisi kepercayaannya hanya bundel itu. MySQL tidak didukung. | Batas API `mysql_async` (§12.1) |
| P-09 | **TOFU dipatok ke fingerprint** dengan known_hosts milik app | Mencegah TOCTOU antara prompt dan percobaan ulang. Engine menolak host asing hari ini (`crates/qh-ffi/src/tunnel.rs:33-46`). |
| P-10 | **Formatter ditulis sendiri** di `qh-sql` di atas mesin `scan.rs` (region opaque) dan modul editor 4B, tanpa crate formatter | Satu pemindai statement untuk engine, editor, dan formatter. Formatter di atas lexer warna akan mewarisi badan `$tag$` yang dilex sebagai kode dan `+--` yang tidak dianggap komentar. Token dan `:name` terjaga. |
| P-11 | **Script runner** memakai perintah baru `script`, kebijakan stop/continue, tanpa transaksi implisit, dan satu entri history per skrip | `import_data` tidak mengembalikan baris per statement |
| P-12 | **Aktivitas server:** cancel saja, dengan guard sendiri | Statement `KILL QUERY` tak terklasifikasi dan akan ditolak di `confirm` |
| P-13 | **Notifikasi masuk lingkup**, menggantikan keputusan Batch 2 #2 di `remaining-work-plan.md` | O-1 memasukkan P1 audit |
| P-14 | **Pohon langsung dibangun ulang sebagai `NSOutlineView`**, tanpa tambalan keyboard sementara | P1 masuk lingkup, jadi tambalan hanya akan dibuang |
| P-15 | **Tooltip sel tetap**, lewat AppKit dan dibaca lazy | `performance-plan.md` §9.4 sudah menghapus ongkos per selnya. Butir "hapus tooltip" audit §5 tidak ada di daftar P0. |
| P-16 | **⌘S lewat responder chain:** editor menyimpan berkas, grid dengan edit membuka tinjauan. ⌘E ganda diperbaiki. | Konvensi macOS: "simpan hal yang sedang fokus" |
| P-17 | **Peek = panel sendiri**, bukan `QLPreviewPanel` | Quick Look butuh berkas sementara berisi data hasil |
| P-18 | **Pasangan kurung dan gutter recess masuk** | Keduanya rekomendasi rewrite editor audit §4 dan murah di atas lexer 4B |
| P-19 | **Impor JSON masuk** (daftar should menang atas "later" di tabel §1.6). Recents di workspace kosong keluar (daftar prioritas menaruhnya di P2). Tampilan Structure keluar, tetapi DDL dan kolom masuk lewat pohon. Perbandingan plan keluar. | Kontradiksi internal di peta fitur dan audit |
| P-20 | **Lokalisasi = infrastruktur** plus string di permukaan yang disentuh W9–W13 saja | Konversi penuh menyentuh semua view, dan rawan konflik |
| P-21 | **Kegagalan gate angka tidak memblokir tugas fungsional sesudahnya.** Gate fungsional memblokir. | Fase 8 adalah jalur resmi untuk angka yang meleset |
| P-22 | **`AppModel.swift` (3.185 baris) dipecah** menjadi extension per domain sebelum gelombang fitur | Hampir setiap fitur menyentuhnya |
| P-23 | **Nomor ADR:** 0030–0036 dipakai sesuai `performance-plan.md` §15. ADR fitur dimulai dari 0037. 0024 dan 0025 dibiarkan kosong. | Dokumen lain sudah merujuk 0030–0036 |
| P-24 | **Blueprint Fase 5 sudah memodelkan kursor sel dan AX.** Pengikatan kuncinya di W10. | Fase performa tidak boleh menambah fitur (`performance-plan.md` §18) |
| P-25 | **Prasyarat P dianggap terpenuhi** oleh commit `3ba01ae`, `f4873c4`, dan `546b9d7`. Orkestrator memverifikasinya di W0-T1. | Snapshot `git status` saat perencanaan hanya menunjukkan dokumen yang belum dilacak |
| P-26 | **Filter funnel per kolom tetap in-memory** dengan banner jujur. Tidak ada WHERE builder server. | Batch 7 dan Fase 3 hanya mencakup sort dan search. Builder bukan P0 atau P1. |

## 12. Koreksi terhadap dokumen lama

Semua koreksi ini ditemukan saat perencanaan.

1. **TLS: kepercayaan sistem sudah dipakai.** PostgreSQL dan Trino sudah memakai trust store macOS lewat `rustls-platform-verifier` (`crates/qh-driver-trino/src/lib.rs:58-60`, `crates/qh-driver-postgres/src/tls.rs:4-60`). CA korporat yang dipasang di Keychain sistem **sudah** dipercaya hari ini. Yang belum ada hanyalah berkas CA per koneksi. MySQL memakai `webpki-roots` bawaan `mysql_async` dan tidak bisa diberi verifier atau berkas CA karena `PathOrBuf` tidak diekspor (`crates/qh-driver-mysql/src/tls.rs:34-71`). Ini mengoreksi baris TLS di peta fitur §1.1 dan cara UC-18 dirumuskan.
2. **⌘E terikat ganda.** Skema pintasan QueryHive mengikat ⌘E ke Explain **dan** Export Data (`app/Sources/QueryHive/Models/Shortcuts.swift:141`, `:147`).
3. **Dependensi yang sudah ada.** `ring` 0.17.14 sudah ada di `Cargo.lock:2903`, lewat rustls di ketiga driver dan `russh` dengan fitur `ring` (`crates/qh-tunnel/Cargo.toml:22`). AEAD untuk spill karena itu tidak menambah crate. `memchr` 2.8.3 juga sudah transitif (`Cargo.lock:1887`). `rayon`, `unicode-segmentation`, `mimalloc`, dan `proptest` belum ada.
4. **Build TablePro bergantung pada jaringan.** Build-nya butuh XcodeGen (tidak terpasang) dan `scripts/download-libs.sh` yang mengunduh library prebuilt. `.xcodeproj` dihasilkan, tidak ada di git (`TablePro/CONTRIBUTING.md:5`, `:11-12`, `:105`, `:110`).
5. **Batas Trino di VM.** `deploy/dev/up.sh:86-96` tidak menyalakan Trino secara default, dan komentar `compose.yaml` menyebut VM 2 GiB tidak muat Trino.
6. **`app/DESIGN.md` akan basi oleh pekerjaan ini.** Kalimat "Stop on a Run takes effect when the statement returns", "there is no formatter", dan "one statement runs per run" akan salah. Kalimat-kalimat itu diperbarui di W14-T5.
7. **Kontradiksi internal.** Impor JSON ("later" di tabel, tetapi masuk daftar should) dan recents (P1 di §10 audit, tetapi P2 di daftar prioritas) diputuskan di P-19.

## 13. Risiko

| Risiko | Dampak | Mitigasi |
|---|---|---|
| Fase 5 (grid) dan Fase 6 (data plane) berukuran L, dengan permukaan paritas yang luas | Gelombang fitur grid tertunda | Blueprint lebih dulu. Seam `ResultRows` di-commit terpisah. Implementer `opus`. Jalur lama dihapus di fase yang sama begitu gate lulus. |
| Paritas lexer 4B dan pemetaan UTF-16 | Warna berubah di layar, gate paritas gagal | Fixture dari Swift, uji acak berbenih, dan tes Swift lewat FFI **sebelum** regex dihapus |
| State bocor lewat sesi pool | Hasil salah antar-Run | Tes reset ditulis lebih dulu (tdd-guide), ditambah pemeriksaan database-reviewer |
| Enkripsi spill salah pakai (nonce, AAD) | Data hasil terbaca di disk | security-reviewer dan tes negatif (kunci salah, manipulasi, teks biasa) |
| Kepercayaan SSH | MITM atau bastion palsu | Fingerprint dipatok, kunci yang berubah ditolak keras, security-reviewer, dan tes terhadap `qh-sshd-dev` |
| Plafon kawat VM (gvproxy) di bawah 575k baris/s | Target absolut sumbu 2 mustahil di rig ini | Klausul "≥ 80% plafon COPY hari yang sama" sudah ada di §2. Plafonnya dicatat. |
| Izin OS dan build TablePro | Tidak ada angka head-to-head | P-03 dan P-04. Langkah pemilik ada di laporan akhir. |
| Disk penuh (pernah terjadi, `tablepro-adoption-plan.md` §5) | Container dan worktree hilang | Cek ruang di W0. Worktree dan target-nya dihapus setelah dipakai. Hanya artefak sendiri yang dibersihkan. |
| Konflik `AppModel.swift`, `QueryTab.swift`, dan `app/Generated/` | Merge gagal, kerja paralel macet | P-22 dan aturan kepemilikan berkas di `development-plan.md` §7 |
| Shell native dan lokalisasi menyentuh banyak berkas | Regresi visual luas | Keduanya serial. Paritas per scene. Perubahan V didaftar. |
| Biaya Trino untuk search dan sort server-first | Query gudang per ketikan atau klik | Debounce 250 ms, minimal 3 karakter, dan search yang sedang jalan di-cancel (Stop sampai server sejak Fase 1) |
| Durasi run otonom | Run terhenti di tengah | Satu commit per tugas, ledger di scratchpad, dan `PROGRESS.md` diperbarui per gelombang supaya run bisa dilanjutkan |
