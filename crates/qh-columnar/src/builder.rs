//! `ChunkBuilder`: one push, one sealed Arrow chunk.
//!
//! The builder stages values per column and seals them into typed Arrow
//! arrays in one go. Staging is bounded — a seal happens at 65,536 rows or
//! an estimated 2 MiB — so the staging never outgrows one chunk.
//!
//! Deviation from §5.3: that section holds typed Arrow builders during the
//! push and converts once to tagged on conflict. This holds `Value`s and
//! picks the encoding once at seal, which is behavior-identical (one push,
//! one seal, same encodings, same conflict rule) without re-encoding arrays
//! mid-push. The typed `push_*` appends are thin wrappers over `push_value` today: the W7-T1
//! driver path that was meant to skip the `Value` was reverted (it missed its keep rule) and the
//! W8-F2 re-A/B reuses them, so they stay and `tests/typed_push.rs` keeps them honest.

use std::sync::Arc;

use arrow_array::builder::{
    BinaryBuilder, BooleanBuilder, Date32Builder, Decimal128Builder, Float64Builder, Int64Builder,
    IntervalMonthDayNanoBuilder, StringBuilder, Time64MicrosecondBuilder,
    TimestampMicrosecondBuilder, UInt64Builder,
};
use arrow_array::types::IntervalMonthDayNano;
use arrow_array::{ArrayRef, NullArray, RecordBatch};
use arrow_schema::Schema;
use qh_core::{ColumnBatch, Value};

use crate::encoding::{field_with_encoding, Encoding};
use crate::tagged;
use crate::ColumnarError;

/// Seal at most this many rows per chunk (§6.2).
pub const CHUNK_MAX_ROWS: usize = 65_536;
/// Seal past this estimated byte size, so a spilled chunk miss stays under
/// ~2 MiB of decrypt plus IPC decode (§2.5, §6.2).
pub const CHUNK_TARGET_BYTES: usize = 2 * 1024 * 1024;

/// `10^38`: decimals at or above it do not fit `Decimal128(38, s)` (F-7).
const DECIMAL_LIMIT: i128 = 10i128.pow(38);

/// A sealed chunk: the batch plus its per-column encodings, parsed once.
pub struct SealedChunk {
    pub batch: RecordBatch,
    pub encodings: Vec<Encoding>,
}

/// Stages one chunk's rows, then seals them into a `RecordBatch`.
pub struct ChunkBuilder {
    width: usize,
    columns: Vec<Vec<Value>>,
    estimated_bytes: usize,
}

impl ChunkBuilder {
    /// An empty builder for a result this wide. Width zero seals to an
    /// error: a result set always has at least one column.
    pub fn new(width: usize) -> Self {
        Self {
            columns: vec![Vec::new(); width],
            width,
            estimated_bytes: 0,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    /// Rows staged so far.
    pub fn rows(&self) -> usize {
        self.columns.first().map_or(0, Vec::len)
    }

    pub fn is_empty(&self) -> bool {
        self.rows() == 0
    }

    /// True past either seal limit: callers seal and start a new chunk.
    pub fn is_full(&self) -> bool {
        self.rows() >= CHUNK_MAX_ROWS || self.estimated_bytes >= CHUNK_TARGET_BYTES
    }

    /// Append one cell of any shape. A value the column's encoding cannot
    /// hold makes the whole chunk-column tagged at seal (§5.3).
    pub fn push_value(&mut self, column: usize, value: Value) {
        let width = match &value {
            Value::Text(text) | Value::Json(text) => text.len() + 9,
            Value::Bytes(bytes) => bytes.len() + 9,
            Value::Unknown { text, raw, .. } => {
                text.as_ref().map_or(0, |text| text.len()) + raw.as_ref().map_or(0, Vec::len) + 21
            }
            Value::Array(items) | Value::Row(items) => 9 + items.len() * 16,
            _ => 16,
        };
        self.estimated_bytes += width;
        self.columns[column].push(value);
    }

    pub fn push_null(&mut self, column: usize) {
        self.push_value(column, Value::Null);
    }

    pub fn push_bool(&mut self, column: usize, value: bool) {
        self.push_value(column, Value::Bool(value));
    }

    pub fn push_i64(&mut self, column: usize, value: i64) {
        self.push_value(column, Value::Int(value));
    }

    pub fn push_u64(&mut self, column: usize, value: u64) {
        self.push_value(column, Value::UInt(value));
    }

    pub fn push_f64(&mut self, column: usize, value: f64) {
        self.push_value(column, Value::Float(value));
    }

    pub fn push_decimal(&mut self, column: usize, unscaled: i128, scale: u8) {
        self.push_value(column, Value::Decimal { unscaled, scale });
    }

    pub fn push_str(&mut self, column: usize, value: &str) {
        self.push_value(column, Value::Text(value.into()));
    }

    pub fn push_json(&mut self, column: usize, value: &str) {
        self.push_value(column, Value::Json(value.into()));
    }

    pub fn push_bytes(&mut self, column: usize, value: &[u8]) {
        self.push_value(column, Value::Bytes(value.to_vec()));
    }

    pub fn push_date(&mut self, column: usize, days: i32) {
        self.push_value(column, Value::Date { days });
    }

    pub fn push_time(&mut self, column: usize, micros: i64) {
        self.push_value(column, Value::Time { micros });
    }

    pub fn push_timestamp(&mut self, column: usize, micros: i64, offset_secs: Option<i32>) {
        self.push_value(
            column,
            Value::Timestamp {
                micros,
                offset_secs,
            },
        );
    }

    pub fn push_interval(&mut self, column: usize, interval: qh_core::IntervalValue) {
        self.push_value(column, Value::Interval(interval));
    }

    /// Append rows from a column batch, stopping at the seal limit.
    /// Returns rows accepted; the caller seals and retries the rest.
    pub fn push_batch(&mut self, batch: &ColumnBatch) -> Result<usize, ColumnarError> {
        if batch.width() != self.width {
            return Err(ColumnarError::Width {
                expected: self.width,
                found: batch.width(),
            });
        }
        let mut accepted = 0;
        for row in 0..batch.rows() {
            if self.is_full() {
                break;
            }
            for column in 0..self.width {
                let cell = batch.value(row, column).cloned().unwrap_or(Value::Null);
                self.push_value(column, cell);
            }
            accepted += 1;
        }
        Ok(accepted)
    }

    /// Seal everything staged into a `RecordBatch`, one field per column.
    /// One push, one seal: the batch is visible as soon as this returns.
    pub fn seal(self) -> Result<SealedChunk, ColumnarError> {
        seal_columns(self.columns)
    }
}

/// Seal explicit columns. Used by tests and by paths that already hold
/// `Vec<Value>` per column.
pub fn seal_columns(columns: Vec<Vec<Value>>) -> Result<SealedChunk, ColumnarError> {
    if columns.is_empty() {
        return Err(ColumnarError::Width {
            expected: 1,
            found: 0,
        });
    }
    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(columns.len());
    let mut encodings = Vec::with_capacity(columns.len());
    let mut fields = Vec::with_capacity(columns.len());
    for (index, column) in columns.iter().enumerate() {
        let kind = pick_encoding(column);
        let array = build_column(&kind, column)?;
        fields.push(field_with_encoding(
            index,
            array.data_type().clone(),
            kind.encoding(),
        ));
        arrays.push(array);
        encodings.push(kind.encoding());
    }
    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(schema, arrays)?;
    Ok(SealedChunk { batch, encodings })
}

/// The encoding decision for one chunk-column, with the parameters the
/// builders need (decimal scale, timestamp offset).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnKind {
    Null,
    Bool,
    I64,
    U64,
    F64,
    Dec(u8),
    Date,
    Time,
    Ts,
    Tsz(i32),
    Interval,
    Text,
    Json,
    Bytes,
    Tagged,
}

impl ColumnKind {
    fn encoding(self) -> Encoding {
        match self {
            ColumnKind::Null => Encoding::Null,
            ColumnKind::Bool => Encoding::Bool,
            ColumnKind::I64 => Encoding::I64,
            ColumnKind::U64 => Encoding::U64,
            ColumnKind::F64 => Encoding::F64,
            ColumnKind::Dec(_) => Encoding::Dec,
            ColumnKind::Date => Encoding::Date,
            ColumnKind::Time => Encoding::Time,
            ColumnKind::Ts => Encoding::Ts,
            ColumnKind::Tsz(_) => Encoding::Tsz,
            ColumnKind::Interval => Encoding::Interval,
            ColumnKind::Text => Encoding::Text,
            ColumnKind::Json => Encoding::Json,
            ColumnKind::Bytes => Encoding::Bytes,
            ColumnKind::Tagged => Encoding::Tagged,
        }
    }
}

/// Choose the encoding from the variants that actually arrived. A column
/// whose non-NULL cells disagree — including mixed scales, mixed offsets,
/// and any composite or unknown — becomes tagged, exactly once per chunk.
fn pick_encoding(values: &[Value]) -> ColumnKind {
    let mut kind: Option<ColumnKind> = None;
    for value in values {
        let single = match value {
            Value::Null => continue,
            Value::Bool(_) => ColumnKind::Bool,
            Value::Int(_) => ColumnKind::I64,
            Value::UInt(_) => ColumnKind::U64,
            Value::Float(_) => ColumnKind::F64,
            Value::Decimal { unscaled, scale } => {
                if *scale > 38 || unscaled.unsigned_abs() >= DECIMAL_LIMIT as u128 {
                    ColumnKind::Tagged
                } else {
                    ColumnKind::Dec(*scale)
                }
            }
            Value::Date { .. } => ColumnKind::Date,
            Value::Time { .. } => ColumnKind::Time,
            Value::Timestamp {
                offset_secs: None, ..
            } => ColumnKind::Ts,
            Value::Timestamp {
                offset_secs: Some(offset),
                ..
            } => {
                if offset % 60 == 0 {
                    ColumnKind::Tsz(*offset)
                } else {
                    ColumnKind::Tagged
                }
            }
            Value::Interval(interval) => {
                if interval.micros.checked_mul(1000).is_some() {
                    ColumnKind::Interval
                } else {
                    ColumnKind::Tagged
                }
            }
            Value::Text(_) => ColumnKind::Text,
            Value::Json(_) => ColumnKind::Json,
            Value::Bytes(_) => ColumnKind::Bytes,
            Value::Array(_) | Value::Row(_) | Value::Map(_) | Value::Unknown { .. } => {
                ColumnKind::Tagged
            }
        };
        kind = Some(match kind {
            None => single,
            Some(current) => merge_kinds(current, single),
        });
    }
    kind.unwrap_or(ColumnKind::Null)
}

/// Merge two single-value decisions. Anything that disagrees — a second
/// variant, a second decimal scale, a second offset — becomes tagged.
fn merge_kinds(current: ColumnKind, single: ColumnKind) -> ColumnKind {
    if current == single {
        current
    } else {
        ColumnKind::Tagged
    }
}

/// Build one typed Arrow array from staged values of a decided kind.
/// NULLs go through the validity bitmap on every encoding.
fn build_column(kind: &ColumnKind, values: &[Value]) -> Result<ArrayRef, ColumnarError> {
    let len = values.len();
    match kind {
        ColumnKind::Null => Ok(Arc::new(NullArray::new(len))),
        ColumnKind::Bool => {
            let mut builder = BooleanBuilder::with_capacity(len);
            for value in values {
                match value {
                    Value::Bool(flag) => builder.append_value(*flag),
                    Value::Null => builder.append_null(),
                    _ => builder.append_null(),
                }
            }
            Ok(Arc::new(builder.finish()))
        }
        ColumnKind::I64 => {
            let mut b = Int64Builder::with_capacity(len);
            for v in values {
                match v {
                    Value::Int(n) => b.append_value(*n),
                    _ => b.append_null(),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        ColumnKind::U64 => {
            let mut b = UInt64Builder::with_capacity(len);
            for v in values {
                match v {
                    Value::UInt(n) => b.append_value(*n),
                    _ => b.append_null(),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        ColumnKind::F64 => {
            let mut b = Float64Builder::with_capacity(len);
            for v in values {
                // Bits preserved: NaN payloads and -0.0 survive.
                match v {
                    Value::Float(n) => b.append_value(*n),
                    _ => b.append_null(),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        ColumnKind::Dec(scale) => {
            let mut b = Decimal128Builder::with_capacity(len);
            for v in values {
                match v {
                    Value::Decimal { unscaled, .. } => b.append_value(*unscaled),
                    _ => b.append_null(),
                }
            }
            let array = b.finish().with_precision_and_scale(38, *scale as i8)?;
            Ok(Arc::new(array))
        }
        ColumnKind::Date => {
            let mut b = Date32Builder::with_capacity(len);
            for v in values {
                match v {
                    Value::Date { days } => b.append_value(*days),
                    _ => b.append_null(),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        ColumnKind::Time => {
            let mut b = Time64MicrosecondBuilder::with_capacity(len);
            for v in values {
                match v {
                    Value::Time { micros } => b.append_value(*micros),
                    _ => b.append_null(),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        ColumnKind::Ts => {
            let mut b = TimestampMicrosecondBuilder::with_capacity(len);
            for v in values {
                match v {
                    Value::Timestamp { micros, .. } => b.append_value(*micros),
                    _ => b.append_null(),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        ColumnKind::Tsz(offset) => {
            let mut b = TimestampMicrosecondBuilder::with_capacity(len);
            for v in values {
                match v {
                    Value::Timestamp { micros, .. } => b.append_value(*micros),
                    _ => b.append_null(),
                }
            }
            Ok(Arc::new(b.finish().with_timezone(format_tz(*offset))))
        }
        ColumnKind::Interval => {
            let mut b = IntervalMonthDayNanoBuilder::with_capacity(len);
            for v in values {
                match v {
                    Value::Interval(i) => {
                        let nanos =
                            i.micros
                                .checked_mul(1000)
                                .ok_or_else(|| ColumnarError::Corrupt {
                                    detail: "interval micros overflow".to_owned(),
                                })?;
                        b.append_value(IntervalMonthDayNano {
                            months: i.months,
                            days: i.days,
                            nanoseconds: nanos,
                        });
                    }
                    _ => b.append_null(),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        ColumnKind::Text | ColumnKind::Json => {
            let mut b = StringBuilder::with_capacity(len, len * 16);
            for v in values {
                match v {
                    Value::Text(t) | Value::Json(t) => b.append_value(t.as_ref()),
                    _ => b.append_null(),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        ColumnKind::Bytes => {
            let mut b = BinaryBuilder::with_capacity(len, len * 16);
            for v in values {
                match v {
                    Value::Bytes(bytes) => b.append_value(bytes),
                    _ => b.append_null(),
                }
            }
            Ok(Arc::new(b.finish()))
        }
        ColumnKind::Tagged => {
            let mut b = BinaryBuilder::with_capacity(len, len * 16);
            let mut scratch = Vec::new();
            for v in values {
                match v {
                    Value::Null => b.append_null(),
                    other => {
                        scratch.clear();
                        tagged::encode_value(&mut scratch, other)?;
                        b.append_value(&scratch);
                    }
                }
            }
            Ok(Arc::new(b.finish()))
        }
    }
}

/// `+HH:MM` for a whole-minute offset (checked at pick time).
fn format_tz(offset_secs: i32) -> String {
    let sign = if offset_secs < 0 { '-' } else { '+' };
    let magnitude = offset_secs.unsigned_abs();
    format!(
        "{sign}{:02}:{:02}",
        magnitude / 3600,
        (magnitude % 3600) / 60
    )
}
