//! The columnar result store: where a running query's rows live, and how the
//! grid reads a window of them.
//!
//! One result is a sequence of sealed Arrow chunks plus an index. The writer
//! seals at 65,536 rows or an estimated 2 MiB; the reader asks for a window of
//! rows and gets one packed `QHW1` buffer. A result larger than the budget
//! spills, encrypted with a key that exists only in this process.
//!
//! The pieces:
//!
//! | Module | What it owns |
//! |---|---|
//! | [`store`] | the shared state: chunks, phase, budget accounting |
//! | [`registry`] | store identity, the global budget, eviction order |
//! | [`chunk`] | one sealed chunk and its grid flags |
//! | [`view`] | filter, search, sort, and the distinct-value picker |
//! | [`render`] | the `QHW1` window buffer and the width stats |
//! | [`spill`] | AES-256-GCM records and the orphan sweep |
//! | [`collate`] | the Swift collation ports and the natural key |
//! | [`logical`] | the logical schema SQL sees (§5.4) |

#![forbid(unsafe_code)]

mod chunk;
mod collate;
mod logical;
mod registry;
mod render;
mod spill;
mod store;
mod view;

pub use chunk::{seal_store_chunk, ChunkFlags, ColumnStats, StoreChunk};
pub use collate::{
    ci_contains, ci_equal, fold, natural_key, natural_key_prefix, swift_double, swift_plain_number,
    NumKey,
};
pub use logical::{
    logical_batch, logical_batches, logical_names, logical_schema, logical_type, LogicalSchema,
};
pub use registry::{
    Origin, QueryLease, RegistryStats, Reservation, StoreConfig, StoreHandle, StoreId,
    StoreRegistry, StoreWriter,
};
pub use render::{
    json_validate, render_window, truncate_utf16, ColumnFormat, CowCell, HeadWidths, Window,
    WindowSpec, CELL_EMPTY, CELL_NULL, CELL_NUMERIC, CELL_OPENABLE, CELL_TRUNCATED, FLAG_COMPLETE,
    FLAG_CUT, FLAG_VIEWED, HEADER_LEN, TRUNCATE_UTF16,
};
pub use spill::{
    decode_record, encode_record, sweep_spill_dir, FaultyMedium, FileMedium, SpillCipher,
    SpillFile, SpillMedium, SweepReport,
};
pub use store::{ChunkRef, Outcome, Phase, StoreError, StoreShared};
pub use view::{
    compute_view, distinct_values, matches_text, DistinctValues, FilterSpec, SortKey, View,
    ViewInfo, ViewSpec,
};
