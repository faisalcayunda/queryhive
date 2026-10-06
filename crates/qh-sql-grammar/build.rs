//! Compiles the vendored parser and its external scanner.
//!
//! `MACOSX_DEPLOYMENT_TARGET` comes from `.cargo/config.toml` (docs/invariants.md), so these
//! objects link at the app's 14.0 floor like the bundled SQLite does.

fn main() {
    // Two archives so that only the generated table can be silenced. `parser.c` carries a few
    // thousand lines of generated data; its warnings are not ours to fix. `scanner.c` is
    // hand-written C that we patch, so it builds with the `cc` default (`-Wall -Wextra`) and
    // a warning there is ours.
    for (file, lib, warnings) in [
        ("vendor/parser.c", "tree-sitter-sql", false),
        ("vendor/scanner.c", "tree-sitter-sql-scanner", true),
    ] {
        cc::Build::new()
            .std("c11")
            .include("vendor")
            .warnings(warnings)
            .file(file)
            .compile(lib);
        println!("cargo:rerun-if-changed={file}");
    }
}
