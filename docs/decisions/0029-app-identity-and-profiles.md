# 0029 — Identitas aplikasi, dan profil lokal yang jadi miliknya

- **Status:** Diterima
- **Tanggal:** 29 Sep 2026
- **Konteks instruksi:** permintaan pengguna agar profil disimpan sebagai milik pengguna, disiapkan
  untuk SSO dan akses yang nanti dibagi lewat server, tetapi untuk sekarang lokal dan **tanpa
  menyentuh koneksi apa pun**; `docs/architecture/tablepro-settings-study.md` (pane Profiles: belum
  pernah diputuskan di matriks mana pun); `docs/architecture/tablepro-feature-analysis.md` §7
  (iCloud sync, app iOS, licensing ditolak); `docs/decisions/0002-keep-mit-license.md`

## Konteks

Supaya sebuah profil bisa "milik pengguna", harus ada pengguna lebih dulu, dan sampai hari ini tidak
ada satu pun konsep identitas di pohon ini: tidak ada tabel user, tidak ada sesi, tidak ada
principal. Yang ada hanya `connection` dengan `secret_ref` ke Keychain, dan itu identitas
**database**, bukan identitas orang yang menjalankan aplikasi.

Permintaannya mengandung dua hal yang mudah tertukar, dan ADR ini memisahkannya secara eksplisit:

1. **Identitas aplikasi.** Siapa yang memakai QueryHive di mesin ini, dan profil mana yang jadi
   miliknya. Ini yang dikerjakan.
2. **Identitas database.** Siapa yang dianggap server database saat sebuah koneksi dibuka. Ini
   **tidak** dikerjakan, dan tidak ada satu baris pun di ADR ini yang berubah di jalur connect:
   `connection` tetap memegang `secret_ref` dan setelan `SSH_*`-nya sendiri.

Pemisahan itu bukan kehati-hatian berlebihan. SSO ke database dan SSO ke aplikasi punya bentuk
teknis yang berbeda, dan yang kedua jauh lebih murah:

- **PostgreSQL 18** menambahkan autentikasi OAuth 2.0 di sisi server: server menjadi resource server
  yang memvalidasi bearer token dari identity provider, dengan metode `oauth` di `pg_hba.conf` dan
  dukungan token di libpq. Dokumentasinya sendiri mencatat bahwa PostgreSQL bukan OIDC client; yang
  memasang validator adalah ekstensi seperti `pg_oidc_validator`.
  (Sumber: `https://www.postgresql.org/docs/current/auth-oauth.html`.)
- **Trino** mendukung OAuth 2.0 lewat HTTPS dengan authorization-code flow, dan sisi klien memakai
  `--external-authentication`, yang membuka browser untuk sign in.
  (Sumber: `https://trino.io/docs/current/security/oauth2.html` dan
  `https://trino.io/docs/current/client/cli.html`.)

Konsekuensinya untuk klien desktop: SSO ke database adalah perjalanan lewat browser, dengan token
yang di-cache per pengguna dan per provider, dan cara menyajikan token berbeda per engine. Itu desain
tersendiri, dan mengikatkannya ke record profil sekarang berarti mengunci bentuk yang belum
diputuskan. Google bisa menjadi identity provider di kedua jalur, jadi memilih Google untuk aplikasi
tidak menghalangi jalur database nanti.

Yang dipilih karena itu adalah jalur aplikasi, dan cara termurah untuk mendapatkannya di klien
desktop: **Desktop app** OAuth client, authorization code dengan PKCE, dan redirect ke loopback
`127.0.0.1`. Google menyatakan loopback IP flow "will continue to be supported on desktop apps";
yang dideprekasi adalah client type iOS, Android, dan Chrome, bukan desktop.
(Sumber: `https://developers.google.com/identity/protocols/oauth2/resources/loopback-migration` dan
`https://developers.google.com/identity/protocols/oauth2/native-app`.) Jadi aplikasi yang ad-hoc
signed tetap bisa, tanpa registrasi URL scheme dan tanpa entitlement baru.

Satu batas yang harus ditulis sebelum ada yang salah harap. **Baris identitas bukan kontrol akses.**
Barisnya ada di berkas SQLite yang sama dengan seluruh data lain, dan siapa pun yang menjalankan
aplikasi di mesin ini bisa membukanya. Kolom `owner_id` memutuskan profil mana yang ditampilkan
untuk akun mana; ia tidak menyembunyikan apa pun dari orang yang bisa membaca berkasnya. Kalau
tujuannya kerahasiaan antar pengguna di satu Mac, mekanismenya lain: secret profil harus dienkripsi
dengan kunci yang diturunkan dari sign-in, bukan sekadar diberi pemilik. Yang dibangun di sini
adalah kepemilikan, bukan kerahasiaan.

## Opsi yang dipertimbangkan

| Keputusan | Opsi | Kelebihan | Kekurangan |
|---|---|---|---|
| Tempat identitas | **Baris `app_account` di SQLite** | Satu tempat dengan seluruh data aplikasi; ikut aturan sync sehingga akun bisa dikaitkan transport nanti; bisa dibaca dan diaudit | Bukan rahasia, dan tidak bisa dijadikan batas keamanan |
| | `UserDefaults` | Paling murah | Bukan record yang bisa dirujuk baris lain; dan ia memang bukan pengaturan |
| | Hanya di Keychain | Terlihat paling aman | Keychain menyimpan secret, bukan identitas; subjek dan email bukan secret, dan menaruhnya di sana membuatnya tidak bisa di-query |
| Pemilik profil | **`owner_id` ke `app_account.id`** | Dua ejaan satu email tidak bisa jadi dua pemilik; pergantian akun jadi revisi biasa | Satu join saat pembacaan |
| | Email sebagai string | Sederhana | Email bisa berpindah tangan, dan identitas yang mengikutinya akan salah orang |
| | Tanpa pemilik | Paling sederhana | Justru menghapus hal yang diminta |
| Penamaan secret profil | **Prefiks pada akun Keychain** | Tidak menyentuh `SERVICE` maupun akun koneksi yang sudah ada; `account_key` sudah menjamin satu ejaan untuk UUID | Satu string yang harus dijaga tidak berubah |
| | Service Keychain terpisah | Pemisahan paling tegas | Item yang sudah ada tetap di bawah service lama, jadi pembacaan jadi bercabang dua |
| | Memakai UUID koneksi | Tanpa kosakata baru | Bertabrakan langsung dengan akun koneksi, dan tabrakan itu berarti satu item dibaca dua cara |
| Cakupan | **Identitas aplikasi saja** | Tidak ada jalur connect yang berubah; bentuknya kecil dan bisa dilanjutkan | Belum menjawab akses database lewat SSO |

## Keputusan

1. **Satu baris `app_account`, dan skemanya yang menjamin tunggal.** `id`-nya dipatok `CHECK (id =
   '00000000-0000-7000-8000-000000000000')`, jadi baris kedua gagal di database, bukan bergantung
   pada kode yang ingat untuk tidak menulisnya. Id-nya tetap UUID supaya ia `SyncId` seperti identitas
   lain di sini.
2. **Profil adalah record kelas satu.** Tabel `profile` dengan `owner_id`, `kind`
   (`saved_query | connection | preference`), `name`, dan `payload_json`; `kind` disimpan sebagai teks
   tanpa CHECK karena himpunannya bertumbuh. Tidak ada kolom `secret_ref`: kind yang punya secret
   membawanya di `payload_json` di bawah penamaan akun yang butir 3 tetapkan.
3. **Secret profil memakai prefiks akun.** `qh_credentials::PROFILE_ACCOUNT_PREFIX` (`profile:`),
   dan id profil dilewatkan `account_key` lebih dulu. Ini tidak mungkin dibaca sebagai akun koneksi:
   akun koneksi adalah UUID yang dibuka digit heksadesimal, sedangkan prefiksnya dibuka `p`.
   `SERVICE` dan perilaku `account_key` untuk akun yang sudah ada tidak berubah.
4. **Identitas ini tidak menyentuh jalur koneksi.** Tidak ada fungsi di modul ini yang dipanggil saat
   sebuah koneksi dibuka, dan tidak ada medan baru di `ConnectionRecord`.
5. **Sign-in belum dibangun, dan itu disengaja.** Yang dibangun adalah tempat ia menulis. Sampai
   client ID ada, sign-in tidak bisa diuji terhadap Google, jadi pengembangan berikutnya memakai
   provider palsu dan client ID menjadi satu baris konfigurasi.
6. **Registrasi OAuth client adalah tindakan pengguna, bukan tugas agen.** Ia hidup di Google Cloud
   Console milik pemilik proyek, tipe *Desktop app*, dan tidak ada di repositori ini.

## Alasan

Tujuan "profil milik pengguna" hanya menuntut dua hal: sebuah identitas yang bisa dirujuk, dan
sebuah kolom pemilik di baris profil. Keduanya selesai tanpa menyentuh database mana pun, dan itulah
sebabnya cakupannya dipisah dari SSO ke database: menunda yang kedua tidak menunda yang pertama,
sementara menggabungkannya akan mengunci bentuk record profil pada keputusan yang belum diambil.

Baris akun diletakkan di SQLite dan bukan di `UserDefaults` karena profil adalah record yang dirujuk
record lain, dan karena seluruh tabel di database ini sudah mengikuti aturan sync (`qh-sync`): ada
identitas tetap, ada revisi, ada cara mengatakan dihapus. Akun yang tidak mengikutinya akan jadi satu
baris yang harus dimigrasikan lebih dulu begitu ada transport, dan itu justru pekerjaan yang ADR ini
hindari.

Prefiks akun dipilih di antara tiga cara memisahkan item Keychain karena dua yang lain membayar lebih
mahal: service terpisah membuat pembacaan bercabang dua selamanya, dan memakai UUID koneksi
menabrakkan dua arti pada satu string, yang gejalanya adalah secret tertukar.

Yang tidak diambil, dan alasannya: enkripsi secret dengan kunci turunan sign-in. Ia menjawab
kerahasiaan, bukan kepemilikan, dan mengerjakannya sekarang berarti memaksa bentuk enkripsi sebelum
ada sign-in yang bisa menurunkan kuncinya.

## Konsekuensi

- Migrasi `0008_identity.sql` menambah tabel `app_account` dan `profile`, plus indeks parsial
  `profile_owner_kind` untuk pembacaan yang benar-benar dijalankan aplikasi.
- `docs/architecture/tablepro-settings-study.md` mencatat pane Profiles sebagai satu-satunya pane
  yang belum pernah diputuskan; setelah ADR ini ia punya tempat, tetapi UI-nya belum dibangun.
- Penolakan iCloud sync di `tablepro-feature-analysis.md` §7 dan plan §9 tetap berdiri: baris profil
  sync-ready, dan tidak ada transport. Yang sync-ready bukan berarti yang tersinkron.
- Arah "akses dibagi lewat server" yang menjadi alasan permintaan ini punya konsekuensi yang tercatat
  di sini: begitu server yang memegang identitas database, klien berhenti memegang password database
  sama sekali. Itu model berbeda dari `connection.secret_ref`, dan ADR ini sengaja tidak
  mendahuluinya.
- Sign-in dengan Google belum ada; yang ada adalah tabel, modul storage, dan jalur Keychain-nya.
  Langkah berikutnya butuh client ID di Google Cloud Console, tipe *Desktop app*.
