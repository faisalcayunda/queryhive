# 0028 — Parameter binding sebagai bagian dari trait driver, dan bit read-only di `Capabilities`

- **Status:** Diterima
- **Tanggal:** 29 Sep 2026 (Gelombang 3, penutupan gap TablePro)
- **Konteks instruksi:** `docs/architecture/tablepro-source-study.md` §2 (parameter binding adalah
  satu dari empat hal yang TablePro lakukan dan QueryHive belum) dan §5 (menyatakan mekanisme per
  driver); `docs/decisions/0021-change-tracking-depth.md` (keputusan 5 sengaja menundanya);
  `docs/decisions/0026-safe-mode-floor.md` (keputusan 5 mencatat bit read-only yang belum ada)

## Konteks

Studi sumber §2 mencatat bahwa pembangkit SQL TablePro memakai parameter, bukan escaping, dan
bentuk placeholder dipilih per dialect (`$1` atau `?`). QueryHive menunda itu di ADR-0021: budget
batch sudah menghitung sumbu parameter supaya bentuknya bertahan, tetapi rencananya masih menulis
literal, dan alasannya ditulis sebagai "protokol HTTP Trino tidak punya bind parameter, jadi apakah
akan mengikat sama sekali adalah keputusan trait driver".

Keputusan itu sekarang. Escaping adalah urusan yang tidak boleh menjadi milik pemanggil: begitu
sebuah nilai masuk ke teks statement, tanda kutip yang salah adalah statement yang salah, dan
nilai yang salah tidak bisa dibedakan di review dari nilai yang benar. Parameter binding
memindahkan escaping ke driver, yang memang tahu mekanisme servernya.

Dua batas di pohon ini mengikat bentuknya:

1. **Trino tidak bisa mengikat.** Protokol HTTP-nya tidak punya bind parameter sama sekali: query
   dikirim sebagai teks dan `nextUri` hanya halaman hasil. Driver yang tidak bisa mengikat harus
   mengatakannya, dan pemanggil yang meng-inline untuknya — bukan mengirim placeholder yang akan
   dibaca literal dan mengikat nol nilai.
2. **Dua bentuk "tidak bisa" sudah punya tempatnya.** ADR-0016 menaruh `statement_timeout` sebagai
   medan `ExecuteOptions` plus bit di `Capabilities`, dengan pola yang tepat: driver yang tidak bisa
   mengatakannya lewat capability, pemanggil bertanya dulu. Parameter mengikuti pola **capability**-nya
   (`Capabilities::parameters`), tetapi nilainya tidak bisa menjadi medan `ExecuteOptions` yang baru:
   tipe itu dibangun dengan literal struct di call site yang tidak tahu apa-apa soal binding, jadi
   medan wajib memecah kompilasinya. Karena itu nilainya lewat metode saudara, `execute_bound`.

Ada gap kecil yang ikut ditutup. `SafeModeFloor` punya `FloorSource::Driver` (ADR-0026 keputusan
5) tetapi tidak ada yang bisa menaikkannya, karena `Capabilities` tidak punya bit read-only.
Menambah bit itu adalah perubahan yang sama jenisnya dengan `statement_timeout`: satu fakta driver
yang dibaca sebelum connect.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Tempat parameter | **Metode `Session::execute_bound`, saudara `execute`** | `ExecuteOptions` tetap beku, jadi setiap literalnya di call site yang tidak tahu binding tetap kompilasi; satu implementasi default menolak untuk driver tanpa bind | Satu metode lagi; wrapper session harus meneruskannya |
| | Medan `ExecuteOptions::parameters` | Satu jalur ke statement | `ExecuteOptions` dibangun dengan literal di `commands.rs`, berkas yang tidak boleh disentuh irisan ini; menambah medan memecah kompilasinya |
| | Argumen baru di `Session::execute` | Parameter terlihat di signature | Memecah setiap implementasi dan setiap call site; dua jalur ke statement yang sama |
| Cara nilai diketik | **Enum `Parameter` tertutup (`Null`/`Bool`/`Int`/`UInt`/`Float`/`Text`)** | Driver tidak bisa menerima varian yang tidak bisa dibindnya; "apa yang bisa dibind" adalah daftar yang bisa dites | Nilai di luar daftar (timestamp, decimal) tetap di-inline pemanggil |
| | Pakai `qh_core::Value` | Satu tipe nilai di seluruh pohon | `Value` punya `Decimal`/`Timestamp`/`Array`/`Map` yang tidak bisa dibind klien tanpa crate tambahan; janji yang tidak bisa ditepati |
| | Semua sebagai string | Sederhana | Itu stringly, bukan typed; klien kehilangan tipe aslinya |
| Cara driver menyatakan dukungan | **`Capabilities::parameters: Option<ParameterStyle>`** | `None` = tidak bisa; `Some(Dollar/Question)` sekaligus menamai placeholder-nya | Enum tiga nilai, bukan bool; harus dibaca |
| | Bool `parameters` + medan gaya terpisah | Mengikuti bentuk `statement_timeout` | Dua medan yang bisa bertentangan (`false` + `Question`) |
| Driver tanpa bind | **Menolak parameter yang tidak kosong** | Tidak pernah mengirim placeholder yang dibaca literal | Satu varian error lagi di jalur itu |
| | Diam-diam mengabaikan parameternya | Tidak ada error | Query berjalan tanpa nilai yang dimaksud — bug senyap |
| Placeholder | **Milik driver, diumumkan lewat capability** | Statement dibangun **untuk** driver, bukan ditebak | App harus memetakan `ConnectionKind` ke gaya yang sama |
| | Pemanggil menebak dari dialect | Tidak ada medan | Tebakan yang salah = statement salah; persis masalah yang ditutup |
| Bit read-only | **`Capabilities::read_only: bool`** | `FloorSource::Driver` menjadi bisa dinaikkan; satu fakta, satu tempat | Ketiga driver menjawab `false` hari ini |
| | Biarkan `FloorSource::Driver` mati | Tidak ada perubahan | Varian yang tidak bisa dikonstruksi adalah kode mati yang menyesatkan |

## Keputusan

**`Session` bertambah metode `execute_bound(sql, parameters, options)` dengan implementasi default
yang menolak daftar tak kosong; `Capabilities` bertambah `parameters: Option<ParameterStyle>` dan
`read_only: bool`; driver yang bisa mengikat menerjemahkan `Parameter` ke mekanisme kliennya;
driver yang tidak bisa menolak parameter yang tidak kosong dan pemanggil meng-inline untuknya;
`apply_changes`/`WritePlan` membangun SQL ber-placeholder plus nilainya, dan menampilkan bentuk
ter-inline.**

Rincian yang mengikat:

1. **Parameter hidup di metode `execute_bound`, bukan di `ExecuteOptions`.** `ExecuteOptions`
   dibangun dengan literal struct di call site yang tidak tahu apa-apa soal binding — termasuk tiga
   di `crates/qh-ffi/src/commands.rs`, berkas yang tidak boleh disentuh irisan ini — dan menambah
   satu medan wajib memecah kompilasi semuanya. `execute_bound` adalah **argumen kedua yang
   terpisah**: `execute(sql, options)` tidak berubah, tidak ada call site lama yang berubah, dan
   hanya satu perintah (`apply_changes`) yang memanggilnya. Implementasi default-nya adalah jawaban
   jujur untuk driver tanpa bind: daftar kosong diteruskan ke `execute`, daftar tak kosong menjadi
   `EngineError::Usage` yang menyebut capability-nya. Driver yang bisa mengikat menimpanya.

2. **Nilai diketik saat masuk, dan daftarnya tertutup.** `Parameter` adalah
   `Null | Bool | Int(i64) | UInt(u64) | Float(f64) | Text(String)`. Itu himpunan yang benar-benar
   bisa dibind ketiga klien; nilai di luar itu (timestamp, decimal eksak, array) tetap menjadi
   tanggung jawab pemanggil untuk meng-inline. `UInt` ada karena `BIGINT UNSIGNED` MySQL tidak
   muat di `i64`, alasan yang sama dengan `qh_core::Value::UInt`.

3. **`Capabilities::parameters` menamai gaya placeholder atau `None`.** `ParameterStyle` adalah
   `Dollar` (`$1`, `$2`, …) atau `Question` (`?`), dengan satu helper
   `placeholder(index) -> String`. Tiga driver menjawab: PostgreSQL `Some(Dollar)`, MySQL
   `Some(Question)`, Trino `None`. `None` berarti "tidak bisa mengikat, inline nilainya", dan itu
   capability yang dibaca sebelum connect, sama seperti `statement_timeout`.

4. **Placeholder adalah milik driver, dan statement dibangun untuk driver.** Pemanggil bertanya
   `Capabilities::parameters` lalu membangun teksnya dengan gaya itu; driver tidak pernah menebak
   gaya dari isi statement, dan pemanggil tidak menebak dari dialect. Di app, `WritePlan` memakai
   `ParameterStyle.forKind`, yang **menyalin** deklarasi driver persis seperti `ConnectionKind.levels`
   menyalin `Capabilities::levels`; salinan itu harus diperbarui bila driver keempat lahir.

5. **Driver yang tidak bisa mengikat menolak, bukan mengirim.** Implementasi default
   `Session::execute_bound` untuk `parameters` tidak kosong mengembalikan `EngineError::Usage` yang
   menyebut capability-nya; Trino menimpanya dengan pesan yang menyebut protokol HTTP-nya.
   Diam-diam mengirim `?` ke Trino berarti Trino membacanya sebagai operator/literal dan menjalankan
   statement yang salah. Itu kesalahan yang seluruh keputusan ini ada untuk mencegahnya.

6. **Mekanisme per driver.**

   - **PostgreSQL memakai `$1` lewat extended protocol.** Statement di-`prepare` lebih dulu (tipe
     parameter disimpulkan server dari konteks `kolom = $1`, `SET kolom = $1`, `VALUES ($1)`), lalu
     nilainya dikirim dalam **format teks** dengan tipe yang disimpulkan itu. Implementasinya
     `ToSql` lokal yang `accepts` setiap tipe, `encode_format` = `Format::Text`, dan menulis byte
     UTF-8 dari nilai; server yang mem-parse angka, boolean, dan timestamp menurut tipe kolomnya,
     jadi lebar integer dan presisi `numeric` tidak perlu dipetakan di klien.
   - **MySQL memakai `?` lewat prepared protocol.** `Parameter` dipetakan ke
     `mysql_async::Value` native (`Int`, `UInt`, `Double`, `Bytes`, `NULL`), dan statement
     dijalankan dengan `exec_iter`. Ini typed di sisi klien, bukan stringly.
   - **Trino menjawab `None` dan menolak** parameter tidak kosong; pemanggil meng-inline.

7. **Batas PostgreSQL dinyatakan, bukan disembunyikan: binding hanya untuk statement yang tidak
   mengembalikan baris.** Extended protocol mengembalikan nilai dalam format biner, sementara
   decoder driver ini adalah jalur teks, dan justru itulah alasan "tipe yang tidak dikenal tidak
   boleh menggagalkan query" (`normalize.rs`). Karena itu `execute_bound` yang statement-nya punya
   kolom hasil **ditolak dengan pesan** alih-alih men-decode sebagian atau diam-diam menurunkan
   kualitas. Hari ini satu-satunya pemakai binding adalah `apply_changes`, yaitu
   `INSERT`/`UPDATE`/`DELETE`; `SELECT` berparameter belum didukung di PostgreSQL dan itu fakta yang
   ditulis, bukan celah yang menunggu ditemukan.

8. **`apply_changes`/`WritePlan` membangun dua bentuk dari satu pass.** `WriteStatement` menyimpan
   `sql` (bentuk review, nilai ter-inline) dan `boundSQL` + `parameters` (bentuk eksekusi). Keduanya
   dihasilkan bersama: generator menulis setiap nilai ke dua buffer sekaligus, jadi bentuk yang
   ditinjau dan bentuk yang dijalankan tidak bisa berbeda. `payload` mengirim `boundSQL` dan `params`
   ke engine; Trino mendapat `boundSQL == sql` tanpa params, yaitu perilaku lama. Ini pola TablePro
   yang dicatat studi: review menyalin parameter ke dalam SQL, run memakai bind.

9. **`Capabilities::read_only: bool` menutup `FloorSource::Driver`.** Ketiga driver menjawab
   `false` hari ini — ketiganya bisa menulis jika servernya mengizinkan, dan tak satu pun bisa tahu
   sebelum connect. Bit-nya ada supaya sebuah engine read-only (replica, katalog baca-saja) bisa
   dinyatakan lewat `Capabilities` alih-alih dipalsukan dengan kondisi lain. Wiring-nya ke engine
   (`SafeModeFloor::raise(FloorSource::Driver, SafeMode::ReadOnly)` bila bit-nya `true`) adalah satu
   baris di `crates/qh-ffi/src/commands.rs`, dan baris itu ditinggalkan untuk irisan yang memegang
   berkas itu.

## Alasan

1. **Escaping yang salah adalah statement yang salah, dan itu tidak bisa dilihat di review.** Nilai
   yang di-inline ke teks bergantung pada satu tanda kutip yang benar; bind menghapus seluruh kelas
   bug itu dengan memindahkan nilai keluar dari teks. Itu alasan study mencatatnya, dan alasan
   irisan ini ada.
2. **Capability adalah tempat yang benar untuk "tidak bisa".** Trino tidak bisa, dan perbedaannya
   bukan detail implementasi — ia mengubah apa yang pemanggil kirim. Menaruhnya di
   `Capabilities::parameters` membuat perbedaan itu bisa dibaca sebelum satu byte dikirim, sama
   seperti `statement_timeout` yang tidak bisa ditegakkan timer lokal.
3. **Metode saudara menjaga `ExecuteOptions` tetap beku.** Menambah medan wajib berarti setiap
   literal `ExecuteOptions` di call site yang tidak tahu binding harus ditulis ulang, termasuk yang
   berada di berkas milik irisan lain. `execute_bound` hanya menambah satu metode dengan default
   yang aman: tidak ada pemanggil lama yang berubah, dan hanya satu perintah yang memakainya.
   Ini pilihan yang sama semangatnya dengan ADR-0016 — letakkan hal baru di satu tempat yang
   pemanggilnya memilih untuk menyentuh — tanpa memaksa menyentuh yang tidak perlu.
4. **Enum tertutup memaksa kejujuran.** `qh_core::Value` yang bisa membawa decimal eksak dan
   timestamp akan mengundang janji "semua nilai bisa dibind" yang tidak bisa ditepati klien tanpa
   crate desimal. `Parameter` kecil dan jujur lebih berguna daripada `Value` besar yang setengahnya
   harus ditolak.
5. **Bentuk teks untuk PostgreSQL adalah yang membuat typed dan eksak sekaligus.** Kalau klien
   memetakan lebar integer sendiri, satu kolom `int2` yang dikirim sebagai `i64` gagal; kalau
   memetakan `numeric` ke `f64`, digitnya hilang. Mengirim teks dan membiarkan server mem-parse
   menurut tipe yang ia sendiri simpulkan adalah satu-satunya bentuk yang benar untuk semua tipe
   sekaligus, dan tetap satu jalur.
6. **Dua bentuk dari satu pass adalah satu-satunya cara "reviewed == ran" tetap bermakna setelah
   bind masuk.** Menghasilkan SQL ber-`$1` untuk run lalu mengganti placeholder dengan literal untuk
   review berarti dua generasi; dua generasi adalah bug yang `WritePlan` ada untuk mencegahnya.
7. **Bit read-only adalah dua baris kode dan satu varian yang berhenti berbohong.** Varian
   `FloorSource::Driver` tanpa cara mengonstruksinya adalah kode mati yang membuat pembaca mengira
   floor driver mungkin padahal tidak.

## Konsekuensi

- **`ExecuteOptions` tidak berubah.** Ia tetap `Eq`, dan setiap literalnya tetap kompilasi. Yang
  bertambah adalah `Session::execute_bound` dengan implementasi default.
- **`Capabilities` bertambah dua medan, jadi setiap konstruktor literalnya bertambah dua baris.**
  Selain tiga driver, itu lima fake di tes `qh-ffi` dan dua fake di `qh-driver`; bukan perubahan
  perilaku.
- **`crates/qh-ffi/src/lib.rs`'s `TunnelledSession` perlu satu forward.** Wrapper itu, dan setiap
  wrapper session lain, mewarisi implementasi default `execute_bound` yang menolak parameter tidak
  kosong, jadi binding lewat SSH tunnel akan ditolak sampai wrapper meneruskan panggilan ke session
  di dalamnya. Berkas itu milik irisan lain di sesi ini; forward satu metode itu ditinggalkan untuk
  orkestrator, bersama wiring `FloorSource::Driver`.
- **Payload `CHANGES` bertambah `params` secara aditif.** Rencana lama tanpa `params` tetap valid:
  `boundSQL`-nya sama dengan `sql`, dan engine meng-inline seperti sebelumnya. CLI dan MCP yang
  menulis `CHANGES` dengan tangan tidak berubah.
- **Yang terverifikasi dan tidak.** Binding PostgreSQL diuji terhadap server hidup (cluster
  sekali-pakai) untuk `UPDATE`/`INSERT` ber-`$1`, termasuk nilai teks yang mengandung tanda kutip
  dan `NULL`; binding MySQL dan penolakan Trino diuji unit karena tidak ada server MySQL/Trino di
  mesin ini, dan itu dinyatakan. `WritePlan`/`apply_changes` diuji di kedua sisi: bentuk Swift
  (placeholder + params + review ter-inline) dan pembacaan `params` di `apply.rs`.
- **Batas yang dinyatakan alih-alih disembunyikan:**
  - `SELECT` berparameter di PostgreSQL ditolak (keputusan 7). MySQL mendukungnya karena
    `normalize`-nya menangani nilai biner protokol prepared.
  - Binding tidak menjangkau tipe yang bukan `Parameter`: nilai `DEFAULT` dan `NULL` tetap kata
    kunci/`IS NULL` di teks (tidak ada yang perlu di-escape), dan nilai lain di luar enum tetap
    di-inline oleh `WritePlan`.
  - App menyalin gaya placeholder dari `ConnectionKind`, bukan membacanya dari engine, karena
    permukaan FFI `Capabilities` tidak diekspos ke Swift; itu pola yang sama dengan `levels`, dan
    salinan itu harus diperbarui bila driver keempat lahir.
  - Wiring `FloorSource::Driver` ke `commands.rs` belum dikerjakan; bit-nya ada dan diuji, baris
    yang memakainya satu, dan berkas itu milik irisan lain.
