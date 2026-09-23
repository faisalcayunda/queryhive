# 0002 — Mempertahankan lisensi MIT

- **Status:** Diterima
- **Tanggal:** sesi Fase 0
- **Konteks instruksi:** §5 poin 8, §8

## Konteks

Repositori sudah dilisensikan MIT (`LICENSE:1`, commit `d9a4ce4 License QueryHive under MIT`).
§8 meminta rekomendasi lisensi beserta konsekuensinya diambil melalui ADR, dan §5 poin 8 meminta
setiap dependency diverifikasi kompatibel secara otomatis dengan `cargo-deny`.

## Opsi yang dipertimbangkan

| Lisensi | Kelebihan | Kekurangan |
|---|---|---|
| **MIT** (kondisi sekarang) | Paling permisif; cocok dengan hampir seluruh ekosistem Rust (dominan `MIT OR Apache-2.0`); tidak menghalangi adopsi korporat | Tidak ada hibah paten eksplisit |
| Apache-2.0 | Sama permisif + hibah paten eksplisit | Tidak kompatibel dengan kode GPL-2.0-only |
| MPL-2.0 | Copyleft level file; perubahan file proyek tetap terbuka | Menambah beban kepatuhan; tidak mengubah apa pun bagi pengguna akhir |
| GPL-3.0 | Menjamin turunan tetap terbuka | Menghalangi penggunaan tertutup; bertentangan dengan tujuan menjadi alternatif yang dipakai luas |

## Keputusan

**Tetap MIT.** Tidak ada perubahan berkas lisensi.

## Alasan

Tujuan produk (§2) adalah "alternatif open source yang lebih baik" yang dipakai dari developer
individu sampai tim perusahaan. MIT memaksimalkan adopsi tanpa syarat tambahan, dan seluruh
kandidat dependency utama (`tokio`, `tokio-postgres`, `mysql_async`, `rustls`, `rusqlite`,
`secrecy`, `zeroize`, `thiserror`, `tracing`) berada di jalur MIT/Apache-2.0 sehingga tidak ada
konflik. `cargo-deny` akan menolak apa pun yang copyleft atau berlisensi tidak dikenal,
jadi keputusan ini juga ditegakkan secara otomatis, bukan hanya dinyatakan.

**Koreksi (sesi migrasi, setelah ADR-0004):** satu kandidat yang semula dihitung di jalur itu
tidak berada di sana. `uniffi` — yang dipilih ADR-0004 untuk control plane dan karena itu ada di
graf — berlisensi **MPL-2.0**, copyleft level file: baris yang sudah dibandingkan di tabel Opsi di
atas, dan **bukan** salah satu lisensi di daftar izin `cargo-deny` pada bagian Konsekuensi
(`cargo metadata --format-version 1`; `docs/dependencies.md` mencatatnya per crate). Jadi
pengecualian yang diminta bagian Konsekuensi ("setiap pengecualian baru harus melalui ADR") belum
dibuat; sampai itu ada, daftar izin di bawah belum mencerminkan graf yang sebenarnya.

Ketiadaan hibah paten eksplisit (satu-satunya keunggulan nyata Apache-2.0) dinilai dapat
diterima: proyek ini tidak memiliki portofolio paten dan tidak berencana mengajukan tuntutan
paten terhadap pengguna.

## Konsekuensi

- `LICENSE` tidak berubah; tidak ada `NOTICE` yang perlu ditambahkan.
- `cargo-deny` dikonfigurasi untuk mengizinkan `MIT`, `Apache-2.0`, `BSD-3-Clause`, `ISC`,
  `Unicode-3.0`, dan `Zlib`; setiap pengecualian baru harus melalui ADR.
- Kontributor tidak perlu menandatangani CLA atau DCO khusus; `CONTRIBUTING.md` menyatakan bahwa
  kontribusi masuk dengan lisensi yang sama.
- Bila di masa depan ada bagian yang menautkan komponen GPL-2.0-only, keputusan ini harus dibuka
  ulang — pencatatan itu adalah alasan ADR ini ada.
