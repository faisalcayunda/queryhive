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

| Crate | Version | Licence | Source |
|---|---|---|---|
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| quote | 1.0.47 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| syn | 3.0.6 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| thiserror | 2.0.20 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| thiserror-impl | 2.0.20 | MIT OR Apache-2.0 | registry+https://github.com/rust-lang/crates.io-index |
| unicode-ident | 1.0.26 | (MIT OR Apache-2.0) AND Unicode-3.0 | registry+https://github.com/rust-lang/crates.io-index |

Total: 6 transitive packages.
