# Provenance of `qh-sql-grammar`

This crate holds a vendored copy of the C sources of a tree-sitter grammar. Nothing under
`vendor/` is written by QueryHive except the one patch below.

## Upstream

| | |
|---|---|
| Project | DerekStride/tree-sitter-sql, https://github.com/DerekStride/tree-sitter-sql |
| Published as | crate `tree-sitter-sequel` 0.3.11 (released 2025-10-01) |
| Crate checksum (`.crate`, sha256) | `9d198ad3c319c02e43c21efa1ec796b837afcb96ffaef1a40c1978fbdcec7d17` |
| Licence | MIT, "Copyright (c) 2021 Derek Stride"; `Cargo.toml` of the crate says `license = "MIT"`, and `LICENSE` here is the upstream file (fetched from the `main` branch, 2026-09-30). The published `.crate` does not ship a `LICENSE` file, so the copyright line is taken from the repository. |
| Why vendored | The crates.io release has the scanner leak below, and `main` is many commits ahead without a release. Vendoring pins exactly what the editor was measured against (blueprint `docs/architecture/blueprints/fase-4b-editor-analysis.md` §1.2, §3). |

## Files

Copied byte for byte from the 0.3.11 crate unless the last column says otherwise.

| Here | In the crate | sha256 (this file) |
|---|---|---|
| `vendor/parser.c` | `src/parser.c` | `852e088fb8470952cdb2a1b78c1c58626c7d91562b26baa4672d51f9754bf580` |
| `vendor/scanner.c` | `src/scanner.c`, **patched** (below) | `db5ac8fcb43b9902c7fd23c08163313921571406929eaa70bc975c0609b47c6e` |
| `vendor/tree_sitter/alloc.h` | `src/tree_sitter/alloc.h` | `b29c1c9fb7cc82f58c84b376df1297d6e2737a1d655fd356db0859e3c29c2fea` |
| `vendor/tree_sitter/array.h` | `src/tree_sitter/array.h` | `5bdf6ed1a78e3409fd443e085ca967a64c188a5d082aaf7f819bccd53a471c94` |
| `vendor/tree_sitter/parser.h` | `src/tree_sitter/parser.h` | `a1f6ef161fbaf48a0e10fca90ef5290a062462b307b3898aa562993853b9f80a` |
| `grammar.js` | `grammar.js` | `6b1f0554a355d8fe3b341ef25c7236cf4b433f85b632e39ade62931b78de16bc` |
| `node-types.json` | `src/node-types.json` | `227cb2197775686cc0b0064aa08e709f7f9575b724b2339371b10fb209e37d73` |
| `LICENSE` | (upstream repository) | `3b3e4e4252d5d7d1c18e1257005f23242bf0580ad619204fd093c99b8c56748d` |

The unpatched `src/scanner.c` of the crate has sha256
`de17b5cffc3c86f56cf6630abb969330c3c839a81ece0b901bdac0ecee93c403`, and `vendor/scanner.patch`
(sha256 `2b1db3b89f8c5f332606adeaf1fa56646c6a7ac3b6efc93500762cd1c73940e7`) turns it into
`vendor/scanner.c`. `grammar.js` is here only as the source to regenerate from and for
reading; the build uses `vendor/parser.c` and does not run `grammar.js`.

`parser.c` was generated with tree-sitter ABI 14, which the `tree-sitter` runtime pinned in
the workspace (0.26.13, ABI 13 to 15) loads.

## The patch: upstream PR #361 plus local fixes

`vendor/scanner.patch` is the `src/scanner.c` part of
https://github.com/DerekStride/tree-sitter-sql/pull/361 ("fix(scanner): don't leak start_tag
across serialize/deserialize"), head `77e33d3dbe07ca6af1d8e2da672d73671a666ad8`, merged as
`97614d051eebfd3bc5d97c0bdb5a1638719ca811` on 2026-09-19. It is not in a release yet. (The PR also
adds a test corpus case in `test/corpus/procedures.txt`, which is not vendored.)

Three changes, in `serialize` and `deserialize`:

1. `serialize` no longer frees `start_tag`. Serializing must not destroy the live state.
2. `deserialize` frees the previous `start_tag` before overwriting it. Before, every re-parse
   of a statement holding a `$body$ … $body$` leaked one tag (about 74 bytes per edit).
3. `deserialize` returns when `malloc` fails, instead of copying into a null pointer.

The rest of the patch is local, **not upstream**:

4. In the `DOLLAR_QUOTED_STRING` branch of `scan`, the early `return false` taken when the new
   tag equals the live `state->start_tag` did not free the 1024-byte `start_tag` buffer.
   `free(start_tag);` now precedes it. Found by a security review.
5. Every `malloc` in the scanner is checked (backlog B-19). `create` returns NULL and the
   other entry points accept a NULL payload; `add_char` frees and returns NULL, so an
   out-of-memory scan finds no tag. The `size_t` counter in `scan_dollar_string_tag` moved
   from the heap to the stack, and `add_char` grows with `realloc` by doubling.
6. `add_char` took a `char`, so a tag character was cut to its low byte: `$Ł$` (U+0141) read
   as `$A$`, and U+0100 wrote a NUL that ended the tag. It now stores the UTF-8 encoding of
   the code point (tree-sitter's decode error, surrogates and values past U+10FFFF become
   U+FFFD), and a NUL in a tag makes it a non-tag. Guarded by
   `a_non_ascii_dollar_quote_tag_is_not_narrowed_onto_an_ascii_one` in `src/lib.rs`.
7. Compiler warnings only: `create(void)`, `size_t tag_length`. `build.rs` now builds
   `scanner.c` with `-Wall -Wextra` in its own archive (backlog B-20); only the generated
   `parser.c` stays silenced. Upstream already tripped `-Wsign-compare` in `add_char`.

`tests/scanner_leak.rs` (G6) is the guard: 10,000 incremental edits inside a `$body$` function
must not grow the heap by more than 128 KiB. It also re-parses `CREATE FUNCTION f() RETURNS text AS $body$ SELECT 1 WHERE a = $body$ LANGUAGE sql;` and `DO $x$ SELECT $x$ $x$` (the local fix's inputs) 1,000 times each. Run against the unpatched scanner the same test
grows the heap by 750,080 bytes and fails.

## Runtime

The `tree-sitter` runtime (0.26.13) and `tree-sitter-language` (0.1.7) are ordinary crates.io
dependencies, pinned with `=` in the workspace `Cargo.toml`. The runtime is MIT
(Copyright (c) 2018 Max Brunsfeld) and bundles ICU's UTF headers under the Unicode licence
(`src/unicode/LICENSE` in the runtime crate); both go into `THIRD-PARTY-NOTICES.md` when the
app first links this code (W4-T2 commit A). `cargo deny check licenses` covers the runtime;
this vendored C is outside its reach, which is why it is recorded here.

## Updating

A grammar update is its own task, with these gates: the dialect probes of blueprint §1.2, the
incremental differential of §1.5, the bench of §1.3, and the binary size of §1.7. Steps:

1. Fetch the new crate (or regenerate from `grammar.js` with a pinned `tree-sitter-cli`) and
   replace the files above; update every checksum in this file.
2. Re-apply `vendor/scanner.patch`; if upstream has released PR #361, delete the patch and this
   section.
3. Run `cargo test -p qh-sql-grammar` and the `qh-editor` golden tests.
4. Forking the grammar for syntax it lacks (`:name`, Trino) is not planned: `qh_sql::lex` and the
   editor's `:name` view cover the colouring.
