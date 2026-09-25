# Dependencies — resolved versions and licences

> Generated from `cargo metadata`; regenerate whenever a manifest changes. The
> table is produced by reading the graph Cargo actually resolved, so no version
> here is quoted from memory:
>
>     cargo metadata --format-version 1
>
> `tools/deps.py` — the script that turns that output into this table — arrives
> with the first CI job that runs `cargo deny check` (Fase 1, blueprint section
> 7.1). Until then, regenerate by hand from the same command.

Every version below is the one Cargo actually resolved, not a version quoted from
memory. A licence of UNKNOWN is a build failure once `cargo deny check licenses`
runs in CI (blueprint section 7.1): it means nobody has classified that crate yet.
There is none: every crate in the graph is classified. One classification is *not*
on ADR-0002's allow-list, and the table is where that shows: `uniffi` and the seven
`uniffi_*` crates it brings are `MPL-2.0`, file-level copyleft. ADR-0002 weighed that
licence and rejected it as a dependency licence; ADR-0004 chose the crate anyway. So
the exception ADR-0002 asks for ("every new exception goes through an ADR") has not
been written yet, and until it is, `cargo deny check licenses` would refuse this graph.

Two shapes in the list are worth knowing rather than re-reading every row:

- **`r-efi` is `MIT OR Apache-2.0 OR LGPL-2.1-or-later`.** That is a *disjunction* and the
  first two options are what this project takes; nothing here distributes it under the
  LGPL. It arrives through the UEFI target support of a build dependency rather than
  through anything the app ships.
- **A handful are `AND` rather than `OR`** — `ring` is `Apache-2.0 AND ISC`,
  `unicode-ident` is `(MIT OR Apache-2.0) AND Unicode-3.0`. Both obligations are
  permissive and both are met by shipping the notice, which is what `LICENSE` and the
  release SBOM are for.

| Crate | Version | Licence | Source |
|---|---|---|---|
| adler2 | 2.0.1 | 0BSD OR MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| aead | 0.6.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| aes | 0.9.3 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| aes-gcm | 0.11.1 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| aho-corasick | 1.1.5 | Unlicense OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| allocator-api2 | 0.2.21 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| android_system_properties | 0.1.6 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| anstyle | 1.0.14 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| anyhow | 1.0.104 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| argon2 | 0.6.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| askama | 0.16.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| askama_derive | 0.16.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| askama_macros | 0.16.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| askama_parser | 0.16.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| asn1-rs | 0.7.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| asn1-rs-derive | 0.6.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| asn1-rs-impl | 0.2.0 | MIT/Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| async-trait | 0.1.92 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| atomic-waker | 1.1.2 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| autocfg | 1.5.1 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| base16ct | 1.0.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| base64 | 0.22.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| base64 | 0.23.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| base64ct | 1.8.3 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| basic-toml | 0.1.10 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| bcrypt-pbkdf | 0.11.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| bit-vec | 0.9.1 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| bitflags | 2.13.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| blake2 | 0.11.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| block-buffer | 0.10.4 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| block-buffer | 0.12.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| block-padding | 0.4.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| blowfish | 0.10.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| btoi | 0.4.3 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| bumpalo | 3.20.3 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| byteorder | 1.5.0 | Unlicense OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| bytes | 1.12.1 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| camino | 1.2.6 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| cargo-platform | 0.3.3 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| cargo_metadata | 0.23.1 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| cbc | 0.2.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| cc | 1.4.7 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| cesu8 | 1.1.0 | Apache-2.0/MIT | registry+https://github.com/rust-lang/crates.io-index |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| cfg_aliases | 0.2.2 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| chacha20 | 0.10.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| chrono | 0.4.45 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| cipher | 0.5.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| clap | 4.6.7 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| clap_builder | 4.6.7 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| clap_derive | 4.6.7 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| clap_lex | 1.1.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| cmov | 0.5.4 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| combine | 4.6.8 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| const-oid | 0.9.6 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| const-oid | 0.10.2 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| core-foundation | 0.10.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| core-foundation-sys | 0.8.7 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| cpubits | 0.1.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| cpufeatures | 0.2.17 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| cpufeatures | 0.3.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| crc32fast | 1.5.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| crossbeam-queue | 0.3.14 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| crossbeam-utils | 0.8.23 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| crypto-bigint | 0.7.5 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| crypto-common | 0.1.7 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| crypto-common | 0.2.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| crypto-primes | 0.7.2 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| ctr | 0.10.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| ctutils | 0.4.2 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| curve25519-dalek | 5.0.0 | BSD-3-Clause | registry+https://github.com/rust-lang/crates.io-index |
| curve25519-dalek-derive | 0.1.1 | MIT/Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| data-encoding | 2.11.1 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| delegate | 0.13.5 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| der | 0.7.10 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| der | 0.8.2 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| der-parser | 10.0.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| der_derive | 0.7.3 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| deranged | 0.5.8 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| des | 0.9.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| digest | 0.10.7 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| digest | 0.11.3 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| displaydoc | 0.2.7 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| ecdsa | 0.17.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| ed25519 | 3.0.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| ed25519-dalek | 3.0.0 | BSD-3-Clause | registry+https://github.com/rust-lang/crates.io-index |
| elliptic-curve | 0.14.1 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| enum_dispatch | 0.3.13 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| equivalent | 1.0.2 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| errno | 0.3.14 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| fallible-iterator | 0.2.0 | MIT/Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| fallible-iterator | 0.3.0 | MIT/Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| fallible-streaming-iterator | 0.1.9 | MIT/Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| fastrand | 2.5.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| ff | 0.14.0 | MIT/Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| fiat-crypto | 0.3.0 | MIT OR Apache-2.0 OR BSD-1-Clause | registry+https://github.com/rust-lang/crates.io-index |
| find-msvc-tools | 0.1.13 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| flagset | 0.4.7 | Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| flate2 | 1.1.10 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| foldhash | 0.1.5 | Zlib | registry+https://github.com/rust-lang/crates.io-index |
| foldhash | 0.2.0 | Zlib | registry+https://github.com/rust-lang/crates.io-index |
| form_urlencoded | 1.2.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| fs-err | 3.3.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| futures | 0.3.34 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| futures-channel | 0.3.34 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| futures-core | 0.3.34 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| futures-executor | 0.3.34 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| futures-io | 0.3.34 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| futures-macro | 0.3.34 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| futures-sink | 0.3.34 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| futures-task | 0.3.34 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| futures-util | 0.3.34 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| generic-array | 0.14.7 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| generic-array | 1.4.5 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| getrandom | 0.2.17 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| getrandom | 0.3.4 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| getrandom | 0.4.3 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| ghash | 0.6.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| glob | 0.3.4 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| goblin | 0.8.2 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| group | 0.14.0 | MIT/Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| hashbrown | 0.15.5 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| hashbrown | 0.16.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| hashbrown | 0.17.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| hashlink | 0.10.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| heck | 0.5.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| hex | 0.4.3 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| hex-literal | 1.1.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| hkdf | 0.13.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| hmac | 0.12.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| hmac | 0.13.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| http | 1.5.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| http-body | 1.1.0 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| http-body-util | 0.1.5 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| httparse | 1.10.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| hybrid-array | 0.4.15 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| hyper | 1.11.1 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| hyper-rustls | 0.27.10 | Apache-2.0 OR ISC OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| hyper-util | 0.1.20 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| iana-time-zone | 0.1.65 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| iana-time-zone-haiku | 0.1.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| icu_collections | 2.3.0 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| icu_locale_core | 2.3.0 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| icu_normalizer | 2.3.0 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| icu_normalizer_data | 2.3.0 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| icu_properties | 2.3.0 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| icu_properties_data | 2.3.0 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| icu_provider | 2.3.1 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| idna | 1.1.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| idna_adapter | 1.2.2 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| indexmap | 2.14.2 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| inout | 0.2.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| ipnet | 2.12.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| itoa | 1.0.18 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| jni | 0.21.1 | MIT/Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| jni | 0.22.4 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| jni-macros | 0.22.4 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| jni-sys | 0.3.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| jni-sys | 0.4.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| jni-sys-macros | 0.4.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| js-sys | 0.3.105 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| keccak | 0.2.2 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| kem | 0.3.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| keyed_priority_queue | 0.4.2 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| lazy_static | 1.5.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| libc | 0.2.189 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| libredox | 0.1.25 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| libsqlite3-sys | 0.35.0 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| linux-raw-sys | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| litemap | 0.8.3 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| lock_api | 0.4.14 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| log | 0.4.34 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| lru | 0.16.4 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| lru-slab | 0.1.3 | MIT OR Apache-2.0 OR Zlib | registry+https://github.com/rust-lang/crates.io-index |
| md-5 | 0.11.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| md5 | 0.8.1 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| memchr | 2.8.3 | Unlicense OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| minimal-lexical | 0.2.1 | MIT/Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| miniz_oxide | 0.9.1 | MIT OR Zlib OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| mio | 1.2.3 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| ml-kem | 0.3.2 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| module-lattice | 0.2.3 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| mysql_async | 0.36.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| mysql_common | 0.35.5 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| nix | 0.31.3 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| nom | 7.1.3 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| num-bigint | 0.4.8 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| num-bigint | 0.5.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| num-conv | 0.2.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| num-integer | 0.1.47 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| num-traits | 0.2.19 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| objc2-core-foundation | 0.3.2 | Zlib OR Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| objc2-system-configuration | 0.3.2 | Zlib OR Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| oid-registry | 0.8.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| once_cell | 1.21.4 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| openssl-probe | 0.2.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| p256 | 0.14.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| p384 | 0.14.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| p521 | 0.14.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| pageant | 0.2.3 | Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| parking_lot | 0.12.5 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| parking_lot_core | 0.9.12 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| password-hash | 0.6.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| pbkdf2 | 0.13.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| pem | 3.0.6 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| pem | 4.0.0 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| pem-rfc7468 | 1.0.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| percent-encoding | 2.3.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| phc | 0.6.1 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| phf | 0.13.1 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| phf_shared | 0.13.1 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| pin-project-lite | 0.2.17 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| pkcs1 | 0.8.0-rc.4 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| pkcs5 | 0.8.1 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| pkcs8 | 0.11.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| pkg-config | 0.3.34 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| plain | 0.2.3 | MIT/Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| poly1305 | 0.9.1 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| polyval | 0.7.3 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| postgres-protocol | 0.6.12 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| postgres-types | 0.2.14 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| potential_utf | 0.1.6 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| powerfmt | 0.2.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| ppv-lite86 | 0.2.21 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| primefield | 0.14.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| primeorder | 0.14.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| quinn | 0.11.12 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| quinn-proto | 0.11.18 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| quinn-udp | 0.5.15 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| quote | 1.0.47 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| r-efi | 5.3.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | registry+https://github.com/rust-lang/crates.io-index |
| r-efi | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | registry+https://github.com/rust-lang/crates.io-index |
| rand | 0.9.5 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| rand | 0.10.3 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| rand_chacha | 0.9.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| rand_core | 0.9.5 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| rand_core | 0.10.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| rand_pcg | 0.10.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| rcgen | 0.14.10 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| redox_syscall | 0.5.18 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| regex | 1.13.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| regex-automata | 0.4.18 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| regex-syntax | 0.8.11 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| reqwest | 0.12.28 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| rfc6979 | 0.6.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| ring | 0.17.14 | Apache-2.0 AND ISC | registry+https://github.com/rust-lang/crates.io-index |
| rsa | 0.10.0-rc.18 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| rusqlite | 0.37.0 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| russh | 0.63.3 | Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| russh-cryptovec | 0.62.0 | Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| russh-util | 0.52.0 | Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| rustc-hash | 2.1.3 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| rustc_version | 0.4.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| rusticata-macros | 4.1.0 | MIT/Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| rustix | 1.1.5 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| rustls | 0.23.45 | Apache-2.0 OR ISC OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| rustls-native-certs | 0.8.4 | Apache-2.0 OR ISC OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| rustls-pemfile | 2.2.0 | Apache-2.0 OR ISC OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| rustls-pki-types | 1.15.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| rustls-platform-verifier | 0.6.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| rustls-platform-verifier | 0.7.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| rustls-platform-verifier-android | 0.1.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| rustls-webpki | 0.103.15 | ISC | registry+https://github.com/rust-lang/crates.io-index |
| rustversion | 1.0.23 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| ryu | 1.0.23 | Apache-2.0 OR BSL-1.0 | registry+https://github.com/rust-lang/crates.io-index |
| salsa20 | 0.11.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| same-file | 1.0.6 | Unlicense/MIT | registry+https://github.com/rust-lang/crates.io-index |
| saturating | 0.1.0 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| schannel | 0.1.29 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| scroll | 0.12.0 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| scroll_derive | 0.12.1 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| scrypt | 0.12.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| sec1 | 0.8.1 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| secrecy | 0.10.3 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| security-framework | 3.7.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| security-framework-sys | 2.17.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| semver | 1.0.28 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| serde | 1.0.229 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| serde_core | 1.0.229 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| serde_json | 1.0.151 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| serde_spanned | 1.1.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| serde_urlencoded | 0.7.1 | MIT/Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| serdect | 0.4.3 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| sha1 | 0.10.7 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| sha1 | 0.11.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| sha2 | 0.10.9 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| sha2 | 0.11.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| sha3 | 0.11.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| sha3 | 0.12.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| shlex | 2.0.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| signal-hook-registry | 1.4.8 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| signature | 3.0.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| simd-adler32 | 0.3.10 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| simd_cesu8 | 1.2.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| simdutf8 | 0.1.5 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| siphasher | 1.0.3 | MIT/Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| slab | 0.4.12 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| smallvec | 1.16.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| smawk | 0.3.3 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| socket2 | 0.5.10 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| socket2 | 0.6.5 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| spki | 0.7.3 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| spki | 0.8.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| sponge-cursor | 0.1.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| ssh-cipher | 0.3.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| ssh-encoding | 0.3.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| ssh-key | 0.7.0-rc.11 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| stable_deref_trait | 1.2.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| static_assertions | 1.1.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| stringprep | 0.1.5 | MIT/Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| strsim | 0.11.1 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| subtle | 2.6.1 | BSD-3-Clause | registry+https://github.com/rust-lang/crates.io-index |
| syn | 2.0.119 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| syn | 3.0.6 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| sync_wrapper | 1.0.2 | Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| synstructure | 0.13.2 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| synstructure | 0.14.0 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| tempfile | 3.27.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| textwrap | 0.16.4 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| thiserror | 1.0.69 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| thiserror | 2.0.20 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| thiserror-impl | 1.0.69 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| thiserror-impl | 2.0.20 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| time | 0.3.55 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| time-core | 0.1.9 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| time-macros | 0.2.32 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| tinystr | 0.8.4 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| tinyvec | 1.13.3 | Zlib OR Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| tls_codec | 0.4.2 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| tls_codec_derive | 0.4.2 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| tokio | 1.53.1 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| tokio-macros | 2.7.2 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| tokio-postgres | 0.7.18 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| tokio-postgres-rustls | 0.14.0 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| tokio-rustls | 0.26.5 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| tokio-util | 0.7.19 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| toml | 1.1.6+spec-1.1.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| toml_datetime | 1.1.1+spec-1.1.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| toml_parser | 1.1.3+spec-1.1.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| toml_writer | 1.1.2+spec-1.1.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| tower | 0.5.3 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| tower-http | 0.6.11 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| tower-layer | 0.3.3 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| tower-service | 0.3.3 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| tracing | 0.1.44 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| tracing-core | 0.1.36 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| try-lock | 0.2.5 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| twox-hash | 2.1.4 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| typenum | 1.20.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| unicode-bidi | 0.3.18 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| unicode-ident | 1.0.26 | (MIT OR Apache-2.0) AND Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| unicode-normalization | 0.1.25 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| unicode-properties | 0.1.4 | MIT/Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| unicode-width | 0.2.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| uniffi | 0.32.1 | MPL-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| uniffi_bindgen | 0.32.1 | MPL-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| uniffi_core | 0.32.1 | MPL-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| uniffi_internal_macros | 0.32.1 | MPL-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| uniffi_macros | 0.32.1 | MPL-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| uniffi_meta | 0.32.1 | MPL-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| uniffi_pipeline | 0.32.1 | MPL-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| uniffi_udl | 0.32.1 | MPL-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| universal-hash | 0.6.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| untrusted | 0.9.0 | ISC | registry+https://github.com/rust-lang/crates.io-index |
| url | 2.5.8 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| utf8_iter | 1.0.4 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| uuid | 1.26.1 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| vcpkg | 0.2.15 | MIT/Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| version_check | 0.9.5 | MIT/Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| walkdir | 2.5.0 | Unlicense/MIT | registry+https://github.com/rust-lang/crates.io-index |
| want | 0.3.1 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| wasi | 0.11.1+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| wasi | 0.14.7+wasi-0.2.4 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| wasip2 | 1.0.4+wasi-0.2.12 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| wasite | 1.0.2 | Apache-2.0 OR BSL-1.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| wasm-bindgen | 0.2.128 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| wasm-bindgen-futures | 0.4.78 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| wasm-bindgen-macro | 0.2.128 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| wasm-bindgen-macro-support | 0.2.128 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| wasm-bindgen-shared | 0.2.128 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| web-sys | 0.3.105 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| web-time | 1.1.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| webpki-root-certs | 1.0.9 | CDLA-Permissive-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| webpki-roots | 0.26.11 | CDLA-Permissive-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| webpki-roots | 1.0.9 | CDLA-Permissive-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| weedle2 | 5.0.0 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| whoami | 2.1.3 | Apache-2.0 OR BSL-1.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| winapi-util | 0.1.11 | Unlicense OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| windows | 0.62.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows-collections | 0.3.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows-core | 0.62.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows-future | 0.3.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows-implement | 0.60.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows-interface | 0.59.3 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows-link | 0.2.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows-numerics | 0.3.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows-result | 0.4.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows-strings | 0.5.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows-sys | 0.45.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows-sys | 0.52.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows-targets | 0.42.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows-targets | 0.52.6 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows-threading | 0.2.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows_aarch64_gnullvm | 0.42.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows_aarch64_gnullvm | 0.52.6 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows_aarch64_msvc | 0.42.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows_aarch64_msvc | 0.52.6 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows_i686_gnu | 0.42.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows_i686_gnu | 0.52.6 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows_i686_gnullvm | 0.52.6 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows_i686_msvc | 0.42.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows_i686_msvc | 0.52.6 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows_x86_64_gnu | 0.42.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows_x86_64_gnu | 0.52.6 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows_x86_64_gnullvm | 0.42.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows_x86_64_gnullvm | 0.52.6 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows_x86_64_msvc | 0.42.2 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| windows_x86_64_msvc | 0.52.6 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| winnow | 1.0.4 | MIT | registry+https://github.com/rust-lang/crates.io-index |
| wit-bindgen | 0.57.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| wnaf | 0.14.1 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| writeable | 0.6.4 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| x509-cert | 0.2.5 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| x509-parser | 0.18.1 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| yasna | 0.6.0 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| yoke | 0.8.3 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| yoke-derive | 0.8.3 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| zerocopy | 0.8.57 | BSD-2-Clause OR Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| zerocopy-derive | 0.8.57 | BSD-2-Clause OR Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| zerofrom | 0.1.8 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| zerofrom-derive | 0.1.8 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| zeroize | 1.9.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| zeroize_derive | 1.5.0 | Apache-2.0 OR MIT | registry+https://github.com/rust-lang/crates.io-index |
| zerotrie | 0.2.5 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| zerovec | 0.11.8 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| zerovec-derive | 0.11.6 | Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |
| zlib-rs | 0.6.8 | Zlib | registry+https://github.com/rust-lang/crates.io-index |
| zmij | 1.0.23 | MIT | registry+https://github.com/rust-lang/crates.io-index |
