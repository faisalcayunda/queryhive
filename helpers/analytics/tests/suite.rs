//! One test binary for the helper's tests.
//!
//! Every integration test of this crate links DataFusion, which makes each test binary
//! about 350 MB in a debug build and a link of tens of seconds. Putting the files under one
//! entry point keeps them where the blueprint names them (`tests/{pool,spill,...}.rs`) and
//! builds them once. `spill_unsafe_dir.rs` is the exception: it points `TMPDIR` at an empty
//! directory for the whole process, so it needs a process of its own.

mod common;

#[path = "confinement.rs"]
mod confinement;
#[path = "files.rs"]
mod files;
#[path = "natural.rs"]
mod natural;
#[path = "pool.rs"]
mod pool;
#[path = "remote_table.rs"]
mod remote_table;
#[path = "server.rs"]
mod server;
#[path = "spill.rs"]
mod spill;
#[path = "sql_surface.rs"]
mod sql_surface;
