//! Canonical value types and the error taxonomy every driver reports through.
//!
//! The point of this crate is that Postgres, MySQL and Trino types stop here: a
//! driver's job is to turn whatever the server said into a [`Value`], and every
//! consumer above it (result store, grid, exporters) only ever sees [`Value`].
//! Adding a driver must not require touching those consumers.
//!
//! Two rules hold across the whole crate:
//!
//! 1. **No value from a server can panic.** A type nobody has taught this crate
//!    about becomes [`Value::Unknown`], which still renders. `unwrap` and
//!    `expect` are not used on server data.
//! 2. **A DECIMAL never becomes a float.** It is stored as `i128` plus a scale,
//!    so `NUMERIC(38,10)` survives intact. The Python engine got this right for
//!    the grid and wrong for its JSON path (`exporter/writers.py:62` calls
//!    `float(value)`); the grid path here is the one being kept.

#![forbid(unsafe_code)]

mod batch;
mod error;
// Public as a module as well as through the re-exports below: `qh-export` needs the
// individual format helpers (`format_decimal`, `hex_encode`) to build a writer that
// renders cells without going through a `String` first.
pub mod render;
mod value;

pub use batch::{BatchError, ColumnBatch, ColumnMeta};
pub use error::{EngineError, FailureKind};
pub use render::{to_json_value, to_text};
pub use value::{unsupported, IntervalValue, Value};
