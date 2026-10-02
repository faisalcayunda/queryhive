# Third-party notices

QueryHive links open-source code. This file lists what ends up in the app binary and
under which licence. It starts with the first code linked for the tree-sitter editor
analysis (W4-T2 commit A). The full crate inventory, and installing this file into the
bundle, is W14's job as a release criterion: that gap predates the editor work.

## tree-sitter runtime (MIT)

- What: the `tree-sitter` crate 0.26.13 (the parser runtime) and `tree-sitter-language`
  0.1.7 (the language plug), both pinned with `=` in the workspace `Cargo.toml`.
- Upstream: <https://github.com/tree-sitter/tree-sitter>
- Licence: MIT, Copyright (c) 2018 Max Brunsfeld.
- Covered by `cargo deny check licenses` (see `deny.toml`).

## Unicode data files inside the runtime (Unicode licence)

- What: the UTF decoding headers the runtime bundles (`src/unicode/LICENSE` in the
  `tree-sitter` crate sources). Outside `cargo deny`'s reach, like the grammar below.
- Licence: the Unicode License Agreement — Data Files and Software, copyright Unicode,
  Inc. It permits use, copying, modification and distribution provided the copyright and
  permission notices travel with the files. The full text is the `LICENSE` file quoted
  above, inside the `tree-sitter` crate sources.

## SQL grammar (MIT, vendored)

- What: the C parser and scanner in `crates/qh-sql-grammar/vendor/`, copied from the
  `tree-sitter-sequel` crate 0.3.11, plus `vendor/scanner.patch` (upstream PR #361 and
  one local fix). Provenance, checksums and the patch rationale: PROVENANCE.md next
  to the crate.
- Upstream: <https://github.com/DerekStride/tree-sitter-sql>
- Licence: MIT, Copyright (c) 2021 Derek Stride. The crate's `LICENSE` is vendored as
  `crates/qh-sql-grammar/LICENSE`.
- This C is compiled into the app by `crates/qh-sql-grammar/build.rs`; the only Rust
  `unsafe` of the editor stack lives in that crate's language declaration.
