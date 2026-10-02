//! Per-chunk-column encodings and the `qh.enc` field metadata.
//!
//! Readers never guess an encoding from the `DataType`: `Binary` may be
//! `bytes` or `tagged`, and `Utf8` may be `text` or `json`. The metadata is
//! the source of truth, and a mismatch is corruption, not a fallback.

use std::collections::HashMap;

use arrow_schema::{DataType, Field};

/// The metadata key carrying a field's encoding.
pub const ENC_KEY: &str = "qh.enc";

/// How one chunk-column is stored. Chosen per (chunk, column) from the
/// `Value` variants that actually arrive; see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Encoding {
    Null,
    Bool,
    I64,
    U64,
    F64,
    Dec,
    Date,
    Time,
    Ts,
    Tsz,
    Interval,
    Text,
    Json,
    Bytes,
    Tagged,
}

impl Encoding {
    /// The metadata value written on the Arrow field.
    pub fn as_str(self) -> &'static str {
        match self {
            Encoding::Null => "null",
            Encoding::Bool => "bool",
            Encoding::I64 => "i64",
            Encoding::U64 => "u64",
            Encoding::F64 => "f64",
            Encoding::Dec => "dec",
            Encoding::Date => "date",
            Encoding::Time => "time",
            Encoding::Ts => "ts",
            Encoding::Tsz => "tsz",
            Encoding::Interval => "interval",
            Encoding::Text => "text",
            Encoding::Json => "json",
            Encoding::Bytes => "bytes",
            Encoding::Tagged => "tagged",
        }
    }

    /// Parse the metadata value back. Unknown names are an error, never a
    /// guess: a record from another build must fail loudly.
    pub fn parse(name: &str) -> Result<Self, crate::ColumnarError> {
        match name {
            "null" => Ok(Encoding::Null),
            "bool" => Ok(Encoding::Bool),
            "i64" => Ok(Encoding::I64),
            "u64" => Ok(Encoding::U64),
            "f64" => Ok(Encoding::F64),
            "dec" => Ok(Encoding::Dec),
            "date" => Ok(Encoding::Date),
            "time" => Ok(Encoding::Time),
            "ts" => Ok(Encoding::Ts),
            "tsz" => Ok(Encoding::Tsz),
            "interval" => Ok(Encoding::Interval),
            "text" => Ok(Encoding::Text),
            "json" => Ok(Encoding::Json),
            "bytes" => Ok(Encoding::Bytes),
            "tagged" => Ok(Encoding::Tagged),
            other => Err(crate::ColumnarError::UnknownEncoding(other.to_owned())),
        }
    }
}

impl std::fmt::Display for Encoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
/// Build a positional field `c<i>` carrying its encoding in metadata.
pub fn field_with_encoding(index: usize, data_type: DataType, enc: Encoding) -> Field {
    let mut metadata = HashMap::with_capacity(2);
    metadata.insert(ENC_KEY.to_owned(), enc.as_str().to_owned());
    if enc == Encoding::Json {
        metadata.insert("ARROW:extension:name".to_owned(), "arrow.json".to_owned());
    }
    Field::new(format!("c{index}"), data_type, true).with_metadata(metadata)
}

/// Read a field's encoding back from its metadata. Missing or unknown
/// metadata is corruption: the builder writes it on every field, so its
/// absence means the bytes did not come from a sealed chunk.
pub fn encoding_of(field: &Field) -> Result<Encoding, crate::ColumnarError> {
    match field.metadata().get(ENC_KEY) {
        Some(name) => Encoding::parse(name),
        None => Err(crate::ColumnarError::MissingEncoding(field.name().clone())),
    }
}

/// Check an encoding against the field's `DataType`. `enc` values that share
/// a physical type (`text`/`json` on `Utf8`, `bytes`/`tagged` on `Binary`)
/// are told apart by metadata alone, so this only rejects what cannot hold
/// the encoding at all.
pub fn check_compatible(enc: Encoding, data_type: &DataType) -> bool {
    matches!(
        (enc, data_type),
        (Encoding::Null, DataType::Null)
            | (Encoding::Bool, DataType::Boolean)
            | (Encoding::I64, DataType::Int64)
            | (Encoding::U64, DataType::UInt64)
            | (Encoding::F64, DataType::Float64)
            | (Encoding::Dec, DataType::Decimal128(38, _))
            | (Encoding::Date, DataType::Date32)
            | (
                Encoding::Time,
                DataType::Time64(arrow_schema::TimeUnit::Microsecond)
            )
            | (
                Encoding::Ts,
                DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None)
            )
            | (
                Encoding::Tsz,
                DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, Some(_))
            )
            | (
                Encoding::Interval,
                DataType::Interval(arrow_schema::IntervalUnit::MonthDayNano)
            )
            | (Encoding::Text | Encoding::Json, DataType::Utf8)
            | (Encoding::Bytes | Encoding::Tagged, DataType::Binary)
    )
}
