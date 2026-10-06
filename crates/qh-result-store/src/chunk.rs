//! A sealed chunk: one Arrow `RecordBatch` plus its grid flags.
//!
//! `ChunkFlags` lives beside the batch, never inside it: DataFusion and IPC
//! see data only, while the grid's openable/numeric bits ride along for the
//! window path. `ColumnStats` accumulates per chunk at seal and feeds the
//! logical schema for SQL (§5.4).

use std::sync::Arc;

use arrow_array::{Array, RecordBatch, StringArray};
use arrow_buffer::BooleanBuffer;
use qh_core::{ColumnMeta, Value};

use qh_columnar::{value_at, Encoding, SealedChunk};

use crate::StoreError;

/// One chunk in memory: the batch, its encodings parsed once at seal, the
/// grid flags, and its accounted byte size.
pub struct StoreChunk {
    pub batch: RecordBatch,
    pub encodings: Arc<[Encoding]>,
    pub flags: ChunkFlags,
    pub bytes: usize,
}

/// Per-cell grid bits, one optional bitmap per column. `None` means no bit
/// is set (or the column is decided by type/encoding without a bitmap).
#[derive(Debug, Clone, Default)]
pub struct ChunkFlags {
    pub openable: Vec<Option<BooleanBuffer>>,
    pub numeric: Vec<Option<BooleanBuffer>>,
}

impl ChunkFlags {
    pub fn empty(width: usize) -> Self {
        Self {
            openable: vec![None; width],
            numeric: vec![None; width],
        }
    }

    /// Heap bytes held by the present bitmaps.
    pub fn bytes(&self) -> usize {
        let sum: usize = self
            .openable
            .iter()
            .chain(self.numeric.iter())
            .filter_map(|b| b.as_ref())
            .map(|b| b.inner().len())
            .sum();
        sum
    }
}
#[derive(Debug, Clone)]
pub struct ColumnStats {
    /// Bitmask of non-NULL `Value` kinds ever seen (the `V_*` constants).
    pub variants: u32,
    /// Largest decimal scale seen; `scale_uniform` clears on disagreement.
    pub max_scale: u8,
    pub scale_uniform: bool,
    /// The shared timestamp offset, if every zoned timestamp agrees.
    /// `Some(i32::MIN)` marks disagreement (no real zone is that value).
    pub offset: Option<i32>,
    /// A zoneless timestamp was seen (mixing with zoned falls to text).
    pub has_naive_ts: bool,
    /// A decimal past 38 digits or an overflowing interval was seen.
    pub overflow: bool,
    /// Rows observed (including NULLs).
    pub rows: u64,
}

pub const V_BOOL: u32 = 1;
pub const V_INT: u32 = 1 << 1;
pub const V_UINT: u32 = 1 << 2;
pub const V_FLOAT: u32 = 1 << 3;
pub const V_DEC: u32 = 1 << 4;
pub const V_DATE: u32 = 1 << 5;
pub const V_TIME: u32 = 1 << 6;
pub const V_TS: u32 = 1 << 7;
pub const V_TSZ: u32 = 1 << 8;
pub const V_INTERVAL: u32 = 1 << 9;
pub const V_TEXT: u32 = 1 << 10;
pub const V_JSON: u32 = 1 << 11;
pub const V_BYTES: u32 = 1 << 12;
pub const V_COMPOSITE: u32 = 1 << 13;
pub const V_UNKNOWN: u32 = 1 << 14;
impl Default for ColumnStats {
    fn default() -> Self {
        Self {
            variants: 0,
            max_scale: 0,
            scale_uniform: true,
            offset: None,
            has_naive_ts: false,
            overflow: false,
            rows: 0,
        }
    }
}

impl ColumnStats {
    /// Fold one cell into the summary.
    pub fn observe(&mut self, value: &Value) {
        self.rows += 1;
        match value {
            Value::Null => {}
            Value::Bool(_) => self.variants |= V_BOOL,
            Value::Int(_) => self.variants |= V_INT,
            Value::UInt(_) => self.variants |= V_UINT,
            Value::Float(_) => self.variants |= V_FLOAT,
            Value::Decimal { scale, unscaled } => {
                let first = self.variants & V_DEC == 0;
                self.variants |= V_DEC;
                if !first && *scale != self.max_scale {
                    self.scale_uniform = false;
                }
                self.max_scale = (*scale).max(self.max_scale);
                if unscaled.unsigned_abs() >= 10u128.pow(38) {
                    self.overflow = true;
                }
            }
            Value::Date { .. } => {
                self.variants |= V_DATE;
            }
            Value::Time { .. } => {
                self.variants |= V_TIME;
            }
            Value::Timestamp {
                offset_secs: None, ..
            } => {
                self.variants |= V_TS;
                self.has_naive_ts = true;
            }
            Value::Timestamp {
                offset_secs: Some(offset),
                ..
            } => {
                self.variants |= V_TSZ;
                match self.offset {
                    None => self.offset = Some(*offset),
                    Some(first) if first != *offset => self.offset = Some(i32::MIN),
                    _ => {}
                }
            }
            Value::Interval(interval) => {
                self.variants |= V_INTERVAL;
                if interval.micros.checked_mul(1000).is_none() {
                    self.overflow = true;
                }
            }
            Value::Text(_) => self.variants |= V_TEXT,
            Value::Json(_) => self.variants |= V_JSON,
            Value::Bytes(_) => self.variants |= V_BYTES,
            Value::Array(_) | Value::Row(_) | Value::Map(_) => self.variants |= V_COMPOSITE,
            Value::Unknown { .. } => self.variants |= V_UNKNOWN,
        }
    }
}

/// Seal a built chunk into a `StoreChunk`: flags, stats delta, and the
/// accounted byte size. `columns` aligns with the batch fields.
pub fn seal_store_chunk(
    sealed: SealedChunk,
    columns: &[ColumnMeta],
) -> Result<(StoreChunk, Vec<ColumnStats>), StoreError> {
    let rows = sealed.batch.num_rows();
    let width = sealed.batch.num_columns();
    let mut flags = ChunkFlags::empty(width);
    let mut stats = vec![ColumnStats::default(); width];
    for (column, (array, enc)) in sealed
        .batch
        .columns()
        .iter()
        .zip(sealed.encodings.iter())
        .enumerate()
    {
        let type_name = columns
            .get(column)
            .map(|meta| meta.type_name.as_ref())
            .unwrap_or("");
        let by_type = openable_by_type(type_name);
        let want_open =
            !by_type && matches!(enc, Encoding::Text | Encoding::Json | Encoding::Tagged);
        let want_num = matches!(
            enc,
            Encoding::Text | Encoding::Json | Encoding::Tagged | Encoding::Bytes
        );
        let mut open_bits = Vec::with_capacity(if want_open { rows } else { 0 });
        let mut num_bits = Vec::with_capacity(if want_num { rows } else { 0 });
        // Text and JSON cells are the bulk of every wire result: read them as
        // borrowed `&str` instead of building a `Value` and a rendered
        // `String` per cell. Anything else takes the generic path.
        let text_array = match enc {
            Encoding::Text | Encoding::Json => array.as_any().downcast_ref::<StringArray>(),
            _ => None,
        };
        if let Some(strings) = text_array {
            let kind = if *enc == Encoding::Text {
                V_TEXT
            } else {
                V_JSON
            };
            let column_stats = &mut stats[column];
            for row in 0..rows {
                column_stats.rows += 1;
                if strings.is_null(row) {
                    if want_open {
                        open_bits.push(false);
                    }
                    num_bits.push(false);
                    continue;
                }
                column_stats.variants |= kind;
                let text = strings.value(row);
                if want_open {
                    open_bits.push(cell_openable_text(text));
                }
                num_bits.push(crate::collate::is_swift_plain_number(text));
            }
        } else {
            for row in 0..rows {
                let value = value_at(array, *enc, row).map_err(store_corrupt)?;
                stats[column].observe(&value);
                if want_open {
                    open_bits.push(cell_openable(&value));
                }
                if want_num {
                    let text = cell_text(&value);
                    num_bits.push(!value.is_null() && crate::collate::is_swift_plain_number(&text));
                }
            }
        }
        if want_open && open_bits.iter().any(|bit| *bit) {
            flags.openable[column] = Some(bits_to_buffer(&open_bits));
        }
        if want_num && num_bits.iter().any(|bit| *bit) {
            flags.numeric[column] = Some(bits_to_buffer(&num_bits));
        }
    }
    let bytes = sealed.batch.get_array_memory_size() + flags.bytes();
    let chunk = StoreChunk {
        batch: sealed.batch,
        encodings: sealed.encodings.into(),
        flags,
        bytes,
    };
    Ok((chunk, stats))
}

fn store_corrupt(error: qh_columnar::ColumnarError) -> StoreError {
    StoreError::Corrupt {
        detail: error.to_string(),
    }
}

/// The stored text of a cell (empty for NULL). Seal-time only: the window
/// path borrows or clips instead of rendering whole cells.
fn cell_text(value: &Value) -> String {
    qh_core::render::to_text(value).unwrap_or_default()
}

fn bits_to_buffer(bits: &[bool]) -> BooleanBuffer {
    let bytes: Vec<u8> = bits
        .chunks(8)
        .map(|byte| {
            let mut packed = 0u8;
            for (index, bit) in byte.iter().enumerate() {
                if *bit {
                    packed |= 1 << index;
                }
            }
            packed
        })
        .collect();
    BooleanBuffer::new(bytes.into(), 0, bits.len())
}
/// Port of `GridValue.isOpenable`, type half: computed once per column.
/// Structured or binary columns open every non-empty cell by type alone.
fn openable_by_type(type_name: &str) -> bool {
    let name = type_name.to_lowercase();
    let structured = name.ends_with("[]")
        || name.contains("array")
        || name.contains("map(")
        || name.contains("row(")
        || name.contains("json");
    let binary = name.contains("bytea") || name.contains("blob") || name.contains("binary");
    structured || binary
}

fn cell_openable(value: &Value) -> bool {
    match value {
        Value::Text(text) | Value::Json(text) => cell_openable_text(text),
        _ => cell_openable_text(&cell_text(value)),
    }
}

fn cell_openable_text(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let mut chars = text.chars();
    let first = loop {
        match chars.next() {
            None => return false,
            Some(char) if char.is_whitespace() => continue,
            Some(char) => break char,
        }
    };
    if first != '{' && first != '[' {
        return false;
    }
    if text.len() > 100_000 && text.encode_utf16().count() > 100_000 {
        return false;
    }
    crate::render::json_validate(text)
}
