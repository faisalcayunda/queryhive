//! The columnar result store: where a running query's rows live, and how the
//! grid reads a window of them.
//!
//! Why this exists rather than a `Vec<Vec<Value>>`
//! ----------------------------------------------
//! The problem being solved is described in the blueprint, section 1.5: the grid
//! has to scroll 500,000 rows by 30 columns without the UI stalling, and the
//! process has to stay under 800 MB doing it. A row-major nested `Vec` fails both
//! — every cell is an enum in its own heap slot, and reading row 300,000 walks
//! through 300,000 rows of pointers first.
//!
//! The layout
//! ----------
//! Each batch is encoded column by column. Per column: an array of `u32` offsets
//! into that column's blob, one offset per row plus a final sentinel. A cell is
//! therefore two array indexes and a byte range — no walking, no pointer chase,
//! and a window of rows costs one slice per column.
//!
//! The cost of that convenience is explicit: four bytes of offset per cell, so a
//! full 500,000 × 30 result carries 60 MB of offsets. That is inside the memory
//! budget and is what buys random access; it is stated here rather than
//! discovered later.
//!
//! Spilling
//! --------
//! Past a configurable threshold, the oldest batches are written to one file and
//! dropped from memory. A spilled batch reads back with `read_exact_at`, and the
//! decoded values are identical to the in-memory ones — there is one encoder and
//! one decoder, and spill stores the same bytes the in-memory form holds, so
//! there is no second code path that could disagree.
//!
//! What is deliberately not here yet
//! ---------------------------------
//! [`ResultStore::window`] returns decoded, owned values for the rows asked for.
//! The blueprint's zero-copy plan (section 4.2) has the grid read offsets into a
//! borrowed buffer instead. That is deferred to Fase 3 with a measurement, per
//! ADR-0008, because the decision depends on the measured cost of copying a page
//! and building it before measuring would be guessing. Until then a window pays
//! for one allocation per requested cell — bounded by the page size the grid
//! asks for, not by the result size.

#![forbid(unsafe_code)]

mod codec;
mod store;

pub use store::{ResultStore, StoreConfig, StoreError};
