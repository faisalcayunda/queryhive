//! The `Value` to Arrow mapping, the chunk builder, and tagged codec.
//!
//! Every sealed chunk is an Arrow `RecordBatch` whose fields are named
//! positionally (`c<i>`) and carry their encoding in `qh.enc` metadata.
//! A chunk-column that is not uniform becomes tagged `Binary`, so any
//! `Value` round-trips losslessly, including `Unknown` in `raw` form.
//!
//! `Utf8` is used, not `Utf8View`: the 4-byte offset is smaller than the
//! 16-byte view, and the difference comes out of the 256 MiB budget.
#![forbid(unsafe_code)]

pub mod builder;
pub mod encoding;
#[cfg(feature = "from-arrow")]
pub mod from_arrow;
pub mod read;
pub mod tagged;

pub use builder::{ChunkBuilder, SealedChunk, CHUNK_MAX_ROWS, CHUNK_TARGET_BYTES};
pub use encoding::Encoding;
#[cfg(feature = "from-arrow")]
pub use from_arrow::from_arrow;
pub use read::{offset_of, text_at, value_at};

/// Everything that can go wrong mapping a `Value` to or from Arrow.
#[derive(Debug, thiserror::Error)]
pub enum ColumnarError {
    /// A field's `qh.enc` names no known encoding, or is missing.
    #[error("unknown chunk encoding: {0}")]
    UnknownEncoding(String),
    /// A field carries no `qh.enc` metadata at all.
    #[error("chunk field {0} carries no qh.enc metadata")]
    MissingEncoding(String),
    /// The encoding and the Arrow type cannot hold each other.
    #[error("encoding {encoding} does not fit {data_type}")]
    Mismatch { encoding: String, data_type: String },
    /// A tagged blob is truncated or uses an unknown tag.
    #[error("tagged value is damaged: {detail}")]
    Corrupt { detail: String },
    /// A batch whose shape does not match the builder's width.
    #[error(transparent)]
    Shape(#[from] qh_core::BatchError),
    /// A pushed batch had a different width from the builder.
    #[error("batch has {found} columns but the builder holds {expected}")]
    Width { expected: usize, found: usize },
    /// Arrow refused the batch (invalid lengths, overflow, ...).
    #[error("arrow: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
}
