//! The logical schema: one Arrow schema per result, derived from the per-column
//! statistics the writer accumulates at seal (§5.4).
//!
//! SQL-on-results (§14) sees this schema, not the physical one: a chunk whose
//! encoding already matches the logical type passes through uncloned, and a
//! chunk that falls to `Utf8` is rendered through the same renderer the grid
//! uses, so a column never loses data and never becomes a silent NULL.

use std::sync::Arc;

use arrow_array::builder::{Decimal128Builder, StringBuilder, TimestampMicrosecondBuilder};
use arrow_array::{Array, ArrayRef, NullArray, RecordBatch};
use arrow_schema::{DataType, Field, Schema, SchemaRef, TimeUnit};
use qh_columnar::Encoding;
use qh_core::{ColumnMeta, Value};

use crate::chunk::{ColumnStats, StoreChunk};
use crate::StoreError;

/// The logical plan for a whole result: its `Schema`, the logical type per
/// source column index, and the SQL column names.
pub struct LogicalSchema {
    pub schema: SchemaRef,
    /// Logical type per source column index.
    pub types: Vec<DataType>,
    /// The de-duplicated names, one per source column.
    pub names: Vec<String>,
}

/// `+HH:MM` for a whole-minute offset, or `UTC` when the offsets disagree or
/// a zoneless value was mixed in. `i32::MIN` is the "disagreement" sentinel
/// `ColumnStats::observe` writes.
fn zone(stats: &ColumnStats) -> String {
    match stats.offset {
        Some(offset) if offset != i32::MIN => {
            let sign = if offset < 0 { '-' } else { '+' };
            let magnitude = offset.unsigned_abs();
            format!(
                "{sign}{:02}:{:02}",
                magnitude / 3600,
                (magnitude % 3600) / 60
            )
        }
        _ => "UTC".to_owned(),
    }
}

/// The §5.4 table: the logical type for one column given its statistics.
///
/// `ponytail:` mixed decimal scales become `Utf8` rather than rescaling to
/// the maximum scale. The rescale can overflow `Decimal128(38, s)` (an
/// `i128`'s 39th digit), and detecting that requires knowing the final scale
/// before the summary is done. Upgrading path: track the max rescaled value
/// in `ColumnStats` and emit `Decimal128(38, max_scale)` when it fits.
pub fn logical_type(stats: &ColumnStats) -> DataType {
    let v = stats.variants;
    if v == 0 {
        return DataType::Null;
    }
    if !v.is_power_of_two() {
        return DataType::Utf8;
    }
    use crate::chunk::{
        V_BOOL, V_BYTES, V_DATE, V_DEC, V_FLOAT, V_INT, V_INTERVAL, V_JSON, V_TEXT, V_TIME, V_TS,
        V_TSZ, V_UINT,
    };
    if v == V_BOOL {
        DataType::Boolean
    } else if v == V_INT {
        DataType::Int64
    } else if v == V_UINT {
        DataType::UInt64
    } else if v == V_FLOAT {
        DataType::Float64
    } else if v == V_DEC {
        if stats.scale_uniform && !stats.overflow {
            DataType::Decimal128(38, stats.max_scale as i8)
        } else {
            DataType::Utf8
        }
    } else if v == V_DATE {
        DataType::Date32
    } else if v == V_TIME {
        DataType::Time64(TimeUnit::Microsecond)
    } else if v == V_TS {
        DataType::Timestamp(TimeUnit::Microsecond, None)
    } else if v == V_TSZ {
        // Zoneless mixed in, or the offsets disagree: the instant, in UTC.
        DataType::Timestamp(TimeUnit::Microsecond, Some(zone(stats).into()))
    } else if v == V_INTERVAL {
        if stats.overflow {
            DataType::Utf8
        } else {
            DataType::Interval(arrow_schema::IntervalUnit::MonthDayNano)
        }
    } else if v == V_TEXT {
        DataType::Utf8
    } else if v == V_JSON {
        // Json: logical type is Utf8 with arrow.json extension metadata.
        // Metadata is added in `logical_schema` per field.
        DataType::Utf8
    } else if v == V_BYTES {
        DataType::Binary
    } else {
        DataType::Utf8
    }
}

/// De-duplicate column names: a duplicate gets `_2`, `_3`, ...; an empty
/// name becomes `column_<i>`.
pub fn logical_names(columns: &[ColumnMeta]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(columns.len());
    let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for (index, meta) in columns.iter().enumerate() {
        let name = if meta.name.is_empty() {
            format!("column_{index}")
        } else {
            meta.name.to_string()
        };
        let key = name.to_ascii_lowercase();
        let used = counts.get_mut(&key).copied().unwrap_or(0);
        counts.insert(key, used + 1);
        let name = if used == 0 {
            name
        } else {
            format!("{name}_{}", used + 1)
        };
        out.push(name);
    }
    out
}

/// Build the logical schema from the result's column metadata and the running
/// statistics. Call once the result is finished (or a chunk later): the types
/// only widen as more rows arrive.
pub fn logical_schema(columns: &[ColumnMeta], stats: &[ColumnStats]) -> LogicalSchema {
    let types: Vec<DataType> = stats.iter().map(logical_type).collect();
    let names = logical_names(columns);
    let fields: Vec<_> = names
        .iter()
        .zip(types.iter())
        .zip(stats.iter())
        .map(|((name, ty), stat)| {
            let field = Field::new(name.as_str(), ty.clone(), true);
            if stat.variants == (1 << 11) {
                // Json: mark the extension so DataFusion and the helper read the
                // column back as `arrow.json` rather than plain text.
                field.with_metadata(
                    std::iter::once(("ARROW:extension:name".to_owned(), "arrow.json".to_owned()))
                        .collect(),
                )
            } else {
                field
            }
        })
        .collect();
    let schema = Arc::new(Schema::new(fields));
    LogicalSchema {
        schema,
        types,
        names,
    }
}

/// Convert the physical chunk to the logical one: same row count, one column
/// per logical target, typed per `logical.schema`. Columns whose physical
/// encoding already matches the logical type are cloned, not rebuilt.
pub fn logical_batch(
    chunk: &StoreChunk,
    targets: &[usize],
    schema: &LogicalSchema,
) -> Result<RecordBatch, StoreError> {
    let rows = chunk.batch.num_rows();
    let mut arrays: Vec<ArrayRef> = Vec::with_capacity(targets.len());
    for (index, &column) in targets.iter().enumerate() {
        let target = column.min(chunk.batch.num_columns().saturating_sub(1));
        let array = chunk.batch.column(target);
        let ty = &schema.types[index];
        arrays.push(build_logical_array(
            array,
            chunk
                .encodings
                .get(target)
                .copied()
                .unwrap_or(Encoding::Null),
            ty,
            rows,
            index,
        )?);
    }
    let batch = RecordBatch::try_new(schema.schema.clone(), arrays)?;
    Ok(batch)
}

fn build_logical_array(
    array: &ArrayRef,
    enc: Encoding,
    ty: &DataType,
    rows: usize,
    index: usize,
) -> Result<ArrayRef, StoreError> {
    match ty {
        DataType::Utf8 => utf8_array(array, enc, index),
        DataType::Decimal128(38, scale) if matches!(enc, Encoding::Dec) => {
            if let DataType::Decimal128(_, chunk_scale) = array.data_type() {
                if chunk_scale == scale {
                    return Ok(array.clone());
                }
            }
            rescale_decimals(array, rows, *scale)
        }
        DataType::Timestamp(TimeUnit::Microsecond, tz) if matches!(enc, Encoding::Tsz) => {
            shift_timestamps(array, rows, tz.as_deref())
        }
        DataType::Null => Ok(typed_nulls(rows)),
        _ if array.data_type() == ty => Ok(array.clone()),
        _ => {
            // ponytail: everything else falls through to a rebuilt column.
            utf8_array(array, enc, index)
        }
    }
}

/// Build a `Utf8` array whose cells are the result of `to_text` on the
/// physical value, matching the grid.
fn utf8_array(array: &ArrayRef, enc: Encoding, _index: usize) -> Result<ArrayRef, StoreError> {
    let rows = array.len();
    let mut builder = StringBuilder::new();
    for row in 0..rows {
        let text = text_of(array, enc, row)?;
        if text.is_empty() {
            builder.append_null();
        } else {
            builder.append_value(&text);
        }
    }
    Ok(Arc::new(builder.finish()))
}

fn text_of(array: &ArrayRef, enc: Encoding, row: usize) -> Result<String, StoreError> {
    let value =
        qh_columnar::value_at(array.as_ref(), enc, row).map_err(|e| StoreError::Corrupt {
            detail: e.to_string(),
        })?;
    Ok(match &value {
        Value::Null => String::new(),
        _ => {
            let mut text = String::new();
            qh_core::render::write_text(&value, &mut text);
            text
        }
    })
}

/// Rescale `Decimal128(_, s)` to `(_, target)` by multiplying the unscaled
/// value by `10^(target - s)`.
fn rescale_decimals(array: &ArrayRef, rows: usize, target: i8) -> Result<ArrayRef, StoreError> {
    use arrow_array::Decimal128Array;
    let array = array
        .as_any()
        .downcast_ref::<Decimal128Array>()
        .ok_or_else(|| StoreError::Corrupt {
            detail: "logical decimal target is not a decimal array".to_owned(),
        })?;
    let scale = match array.data_type() {
        DataType::Decimal128(_, s) => *s,
        _ => unreachable!("decimal128 array"),
    };
    let factor = if target > scale {
        10i128.pow((target - scale) as u32)
    } else {
        0 // ponytail: rounding down a decimal scale is not supported.
    };
    let mut builder =
        Decimal128Builder::with_capacity(rows).with_precision_and_scale(38, target)?;
    for row in 0..rows {
        if array.is_null(row) {
            builder.append_null();
        } else {
            let value = array.value(row);
            let result = if factor == 0 {
                value
            } else {
                value
                    .checked_mul(factor)
                    .ok_or_else(|| StoreError::Corrupt {
                        detail: format!("decimal rescale overflow at row {row}"),
                    })?
            };
            builder.append_value(result);
        }
    }
    Ok(Arc::new(
        builder.finish().with_precision_and_scale(38, target)?,
    ))
}

/// Shift a `Timestamp(Microsecond, Some(+hh:mm))` array's micros to the
/// target zone, and rebuild it under the new zone.
fn shift_timestamps(
    array: &ArrayRef,
    rows: usize,
    target_tz: Option<&str>,
) -> Result<ArrayRef, StoreError> {
    use arrow_array::TimestampMicrosecondArray;
    let array = array
        .as_any()
        .downcast_ref::<TimestampMicrosecondArray>()
        .ok_or_else(|| StoreError::Corrupt {
            detail: "logical timestamp target is not a timestamp array".to_owned(),
        })?;
    let source_zone = match array.data_type() {
        DataType::Timestamp(_, Some(zone)) => zone.clone(),
        _ => {
            return Err(StoreError::Corrupt {
                detail: "source timestamp has no zone but is being relabelled".to_owned(),
            })
        }
    };
    let source_offset = qh_columnar::offset_of(&source_zone).unwrap_or(0);
    let target_offset = match target_tz {
        Some(tz) => qh_columnar::offset_of(tz).unwrap_or(0),
        None => 0,
    };
    let delta_micros = ((source_offset - target_offset) as i64) * 1_000_000;
    let mut builder = TimestampMicrosecondBuilder::with_capacity(rows);
    for row in 0..rows {
        if array.is_null(row) {
            builder.append_null();
        } else {
            let shifted =
                array
                    .value(row)
                    .checked_add(delta_micros)
                    .ok_or_else(|| StoreError::Corrupt {
                        detail: format!("timestamp shift overflow at row {row}"),
                    })?;
            builder.append_value(shifted);
        }
    }
    match target_tz {
        Some(tz) => Ok(Arc::new(builder.finish().with_timezone(tz.to_owned()))),
        None => Ok(Arc::new(builder.finish())),
    }
}

fn typed_nulls(rows: usize) -> ArrayRef {
    Arc::new(NullArray::new(rows))
}

// Build a logical batch for the whole result, chunk by chunk.
pub fn logical_batches(
    chunks: &[Arc<StoreChunk>],
    targets: &[usize],
    schema: &LogicalSchema,
) -> Result<Vec<RecordBatch>, StoreError> {
    chunks
        .iter()
        .map(|chunk| logical_batch(chunk, targets, schema))
        .collect()
}
