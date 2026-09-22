//! The binding generator, run from this workspace's own pinned `uniffi`.
//!
//! Through this binary rather than a `cargo install`ed one, so the generator and the runtime
//! in `Cargo.lock` are the same version: a mismatch shows up as bindings that do not compile
//! against the library they were generated for, which is a confusing way to learn it.
//!
//!     cargo run -p qh-ffi --bin uniffi-bindgen -- generate --library \
//!       target/debug/libqh_ffi.dylib --language swift --out-dir <dir>
//!
//! `--library` reads the compiled `cdylib` rather than the source, which is the only way the
//! generator can see the types the proc macros produced.
fn main() {
    uniffi::uniffi_bindgen_main()
}
