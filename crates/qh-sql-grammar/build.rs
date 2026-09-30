//! Compiles the vendored parser and its external scanner.
//!
//! `MACOSX_DEPLOYMENT_TARGET` comes from `.cargo/config.toml` (docs/invariants.md), so these
//! objects link at the app's 14.0 floor like the bundled SQLite does.

fn main() {
    let mut build = cc::Build::new();
    build.std("c11").include("vendor");
    // `parser.c` is generated and carries a few thousand lines of table data; its own
    // warnings are not ours to fix.
    build.warnings(false);
    for file in ["vendor/parser.c", "vendor/scanner.c"] {
        build.file(file);
        println!("cargo:rerun-if-changed={file}");
    }
    build.compile("tree-sitter-sql");
}
