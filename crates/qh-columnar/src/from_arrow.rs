//! Arrow from outside the store, normalised to the store encodings (§5.6).
//!
//! DataFusion's output, a CSV reader's and a Parquet reader's batches carry whatever
//! Arrow types they like (`Utf8View`, `Int32`, `Timestamp(ns, "Europe/Paris")`,
//! `Dictionary`, `List`, ...). The store knows fourteen encodings (§5.1), and the app
//! accepts nothing else, so this module is the one place that maps the rest onto them.
//! It runs in the analytics helper only, behind the `from-arrow` feature, which is what
//! pulls in `arrow-cast`: the app never links it.
//!
//! The mapping goes through [`Value`] and [`seal_columns`], the same path every driver
//! takes. That is slower than re-labelling an array in place, and it is deliberate: the
//! choice of encoding per chunk-column (a mixed column becomes `tagged`, once) then has
//! exactly one implementation, and a cell the store cannot type never becomes a silent
//! NULL.
//!
//! | Arrow type | Becomes |
//! |---|---|
//! | `Int8/16/32/64`, `UInt8/16/32/64` | `i64` / `u64` |
//! | `Float16/32` | `f64` through the shortest decimal text, so `0.1f32` is `0.1`, not `0.10000000149011612` |
//! | `Utf8`, `LargeUtf8`, `Utf8View` | `text` (`json` when the field carries `arrow.json`) |
//! | `Binary`, `LargeBinary`, `BinaryView`, `FixedSizeBinary` | `bytes` |
//! | `Date32` | `date`; `Date64` becomes `ts` |
//! | `Time32`, `Time64` | `time` |
//! | `Timestamp(unit, zone)` | `ts` or `tsz`, in microseconds. **`ns` is truncated downwards** (floor), so the last three digits of a nanosecond timestamp are gone. A named zone is resolved per value; offsets that differ across the column make it `tagged`. |
//! | `Duration`, `Interval` | `interval` |
//! | `Decimal32/64/128` | `dec`; a negative scale is multiplied out |
//! | `Decimal256` | `dec` when the value fits an `i128`, otherwise `text` |
//! | `Dictionary`, `RunEndEncoded` | the type of their values |
//! | `List`, `LargeList`, `FixedSizeList`, `Struct`, `Map` | `tagged` (`Array`, `Row`, `Map`) |
//! | `Union` | the active child's value |
//! | anything else (`ListView`, ...) | `text`, through Arrow's own display |

use std::collections::HashMap;

use arrow_array::cast::AsArray;
use arrow_array::timezone::Tz;
use arrow_array::types::{
    Date32Type, Date64Type, Decimal128Type, Decimal256Type, Decimal32Type, Decimal64Type,
    DurationMicrosecondType, DurationMillisecondType, DurationNanosecondType, DurationSecondType,
    Float16Type, Float32Type, Float64Type, Int16Type, Int32Type, Int64Type, Int8Type,
    IntervalDayTimeType, IntervalMonthDayNanoType, IntervalYearMonthType, Time32MillisecondType,
    Time32SecondType, Time64MicrosecondType, Time64NanosecondType, TimestampMicrosecondType,
    TimestampMillisecondType, TimestampNanosecondType, TimestampSecondType, UInt16Type, UInt32Type,
    UInt64Type, UInt8Type,
};
use arrow_array::{Array, ArrayRef, RecordBatch};
use arrow_cast::display::array_value_to_string;
use arrow_schema::{DataType, Field, IntervalUnit, TimeUnit};
use chrono::{Offset, TimeZone};
use qh_core::{IntervalValue, Value};

use crate::builder::{seal_columns, SealedChunk};
use crate::ColumnarError;

/// The field metadata that marks a `Utf8` column as JSON text (§5.1).
const JSON_EXTENSION: &str = "arrow.json";

/// Normalise one batch into a sealed chunk of store encodings.
///
/// The batch is sealed whole: the caller splits a large batch at the chunk limits
/// first (65,536 rows or about 2 MiB, §14.12).
pub fn from_arrow(batch: &RecordBatch) -> Result<SealedChunk, ColumnarError> {
    let schema = batch.schema();
    let mut converter = Converter::default();
    let mut columns = Vec::with_capacity(batch.num_columns());
    for (index, array) in batch.columns().iter().enumerate() {
        columns.push(converter.cells(array, is_json(schema.field(index)))?);
    }
    seal_columns(columns)
}

fn is_json(field: &Field) -> bool {
    field
        .metadata()
        .get("ARROW:extension:name")
        .is_some_and(|name| name == JSON_EXTENSION)
}

/// Zones are parsed once per name, not once per cell: `chrono-tz` lookups are not free.
#[derive(Default)]
struct Converter {
    zones: HashMap<String, Tz>,
}

fn corrupt(detail: impl Into<String>) -> ColumnarError {
    ColumnarError::Corrupt {
        detail: detail.into(),
    }
}

/// Microseconds from a count of `unit`, or `None` past the `i64` range. Nanoseconds are
/// floored, so a timestamp before 1970 truncates towards the past like one after it.
fn micros(value: i64, unit: TimeUnit) -> Option<i64> {
    match unit {
        TimeUnit::Second => value.checked_mul(1_000_000),
        TimeUnit::Millisecond => value.checked_mul(1_000),
        TimeUnit::Microsecond => Some(value),
        TimeUnit::Nanosecond => Some(value.div_euclid(1_000)),
    }
}

/// The shortest decimal text of an `f32`, read back as an `f64`. Rust's `Display` for
/// `f32` already prints the shortest text that round-trips, so `0.1f32` is `0.1`.
fn shortest(value: f32) -> f64 {
    value.to_string().parse().unwrap_or(f64::NAN)
}

/// The same for a half float, whose own `Display` goes through `f32` and would print
/// `0.099975586` for `0.1f16`. Try one digit, then two, until the text reads back as
/// the same half; NaN never does and falls through to the plain conversion.
fn shortest_half(value: half::f16) -> f64 {
    let wide = value.to_f32();
    for digits in 0..9 {
        let text = format!("{wide:.digits$e}");
        if let Ok(back) = text.parse::<f32>() {
            if half::f16::from_f32(back) == value {
                return text.parse().unwrap_or(f64::from(wide));
            }
        }
    }
    f64::from(wide)
}

/// `unscaled * 10^-scale` for a negative Arrow scale, or `None` on overflow.
fn rescale(unscaled: i128, scale: i8) -> Option<(i128, u8)> {
    if scale >= 0 {
        return Some((unscaled, scale as u8));
    }
    let factor = 10i128.checked_pow(u32::from(scale.unsigned_abs()))?;
    Some((unscaled.checked_mul(factor)?, 0))
}

impl Converter {
    /// Every cell of `array` as a `Value`, `array.len()` of them.
    fn cells(&mut self, array: &ArrayRef, json: bool) -> Result<Vec<Value>, ColumnarError> {
        let rows = array.len();
        // The text of one cell, for the types with no store encoding and for values
        // that overflow one. Arrow's display never fails on a valid array.
        let text = |row: usize| -> Value {
            Value::Text(
                array_value_to_string(array, row)
                    .unwrap_or_default()
                    .into_boxed_str(),
            )
        };
        macro_rules! each {
            ($array:expr, $row:ident, $value:expr) => {{
                let array = $array;
                (0..rows)
                    .map(|$row| {
                        if array.is_null($row) {
                            Value::Null
                        } else {
                            $value
                        }
                    })
                    .collect::<Vec<Value>>()
            }};
        }
        let cells = match array.data_type() {
            DataType::Null => vec![Value::Null; rows],
            DataType::Boolean => each!(
                array.as_boolean(),
                i,
                Value::Bool(array.as_boolean().value(i))
            ),
            DataType::Int8 => each!(
                array.as_primitive::<Int8Type>(),
                i,
                Value::Int(array.as_primitive::<Int8Type>().value(i).into())
            ),
            DataType::Int16 => each!(
                array.as_primitive::<Int16Type>(),
                i,
                Value::Int(array.as_primitive::<Int16Type>().value(i).into())
            ),
            DataType::Int32 => each!(
                array.as_primitive::<Int32Type>(),
                i,
                Value::Int(array.as_primitive::<Int32Type>().value(i).into())
            ),
            DataType::Int64 => each!(
                array.as_primitive::<Int64Type>(),
                i,
                Value::Int(array.as_primitive::<Int64Type>().value(i))
            ),
            DataType::UInt8 => each!(
                array.as_primitive::<UInt8Type>(),
                i,
                Value::UInt(array.as_primitive::<UInt8Type>().value(i).into())
            ),
            DataType::UInt16 => each!(
                array.as_primitive::<UInt16Type>(),
                i,
                Value::UInt(array.as_primitive::<UInt16Type>().value(i).into())
            ),
            DataType::UInt32 => each!(
                array.as_primitive::<UInt32Type>(),
                i,
                Value::UInt(array.as_primitive::<UInt32Type>().value(i).into())
            ),
            DataType::UInt64 => each!(
                array.as_primitive::<UInt64Type>(),
                i,
                Value::UInt(array.as_primitive::<UInt64Type>().value(i))
            ),
            DataType::Float16 => each!(array.as_primitive::<Float16Type>(), i, {
                Value::Float(shortest_half(array.as_primitive::<Float16Type>().value(i)))
            }),
            DataType::Float32 => each!(array.as_primitive::<Float32Type>(), i, {
                Value::Float(shortest(array.as_primitive::<Float32Type>().value(i)))
            }),
            DataType::Float64 => each!(
                array.as_primitive::<Float64Type>(),
                i,
                Value::Float(array.as_primitive::<Float64Type>().value(i))
            ),
            DataType::Utf8 => {
                let strings = array.as_string::<i32>();
                each!(
                    strings,
                    i,
                    if json {
                        Value::Json(strings.value(i).into())
                    } else {
                        Value::Text(strings.value(i).into())
                    }
                )
            }
            DataType::LargeUtf8 => {
                let strings = array.as_string::<i64>();
                each!(
                    strings,
                    i,
                    if json {
                        Value::Json(strings.value(i).into())
                    } else {
                        Value::Text(strings.value(i).into())
                    }
                )
            }
            DataType::Utf8View => {
                let strings = array.as_string_view();
                each!(
                    strings,
                    i,
                    if json {
                        Value::Json(strings.value(i).into())
                    } else {
                        Value::Text(strings.value(i).into())
                    }
                )
            }
            DataType::Binary => each!(
                array.as_binary::<i32>(),
                i,
                Value::Bytes(array.as_binary::<i32>().value(i).to_vec())
            ),
            DataType::LargeBinary => each!(
                array.as_binary::<i64>(),
                i,
                Value::Bytes(array.as_binary::<i64>().value(i).to_vec())
            ),
            DataType::BinaryView => each!(
                array.as_binary_view(),
                i,
                Value::Bytes(array.as_binary_view().value(i).to_vec())
            ),
            DataType::FixedSizeBinary(_) => each!(
                array.as_fixed_size_binary(),
                i,
                Value::Bytes(array.as_fixed_size_binary().value(i).to_vec())
            ),
            DataType::Date32 => each!(
                array.as_primitive::<Date32Type>(),
                i,
                Value::Date {
                    days: array.as_primitive::<Date32Type>().value(i)
                }
            ),
            DataType::Date64 => each!(array.as_primitive::<Date64Type>(), i, {
                match micros(
                    array.as_primitive::<Date64Type>().value(i),
                    TimeUnit::Millisecond,
                ) {
                    Some(micros) => Value::Timestamp {
                        micros,
                        offset_secs: None,
                    },
                    None => text(i),
                }
            }),
            DataType::Time32(TimeUnit::Second) => {
                each!(array.as_primitive::<Time32SecondType>(), i, {
                    Value::Time {
                        micros: i64::from(array.as_primitive::<Time32SecondType>().value(i))
                            * 1_000_000,
                    }
                })
            }
            DataType::Time32(_) => each!(array.as_primitive::<Time32MillisecondType>(), i, {
                Value::Time {
                    micros: i64::from(array.as_primitive::<Time32MillisecondType>().value(i))
                        * 1_000,
                }
            }),
            DataType::Time64(TimeUnit::Nanosecond) => {
                each!(array.as_primitive::<Time64NanosecondType>(), i, {
                    Value::Time {
                        micros: array
                            .as_primitive::<Time64NanosecondType>()
                            .value(i)
                            .div_euclid(1_000),
                    }
                })
            }
            DataType::Time64(_) => each!(array.as_primitive::<Time64MicrosecondType>(), i, {
                Value::Time {
                    micros: array.as_primitive::<Time64MicrosecondType>().value(i),
                }
            }),
            DataType::Timestamp(unit, zone) => {
                let zone = match zone {
                    Some(name) => Some(self.zone(name)?),
                    None => None,
                };
                let raw = |row: usize| match unit {
                    TimeUnit::Second => array.as_primitive::<TimestampSecondType>().value(row),
                    TimeUnit::Millisecond => {
                        array.as_primitive::<TimestampMillisecondType>().value(row)
                    }
                    TimeUnit::Microsecond => {
                        array.as_primitive::<TimestampMicrosecondType>().value(row)
                    }
                    TimeUnit::Nanosecond => {
                        array.as_primitive::<TimestampNanosecondType>().value(row)
                    }
                };
                (0..rows)
                    .map(|row| {
                        if array.is_null(row) {
                            return Value::Null;
                        }
                        let Some(micros) = micros(raw(row), *unit) else {
                            return text(row);
                        };
                        match &zone {
                            None => Value::Timestamp {
                                micros,
                                offset_secs: None,
                            },
                            Some(zone) => match offset_secs(zone, micros) {
                                Some(offset) => Value::Timestamp {
                                    micros,
                                    offset_secs: Some(offset),
                                },
                                None => text(row),
                            },
                        }
                    })
                    .collect()
            }
            DataType::Duration(unit) => (0..rows)
                .map(|row| {
                    if array.is_null(row) {
                        return Value::Null;
                    }
                    let raw = match unit {
                        TimeUnit::Second => array.as_primitive::<DurationSecondType>().value(row),
                        TimeUnit::Millisecond => {
                            array.as_primitive::<DurationMillisecondType>().value(row)
                        }
                        TimeUnit::Microsecond => {
                            array.as_primitive::<DurationMicrosecondType>().value(row)
                        }
                        TimeUnit::Nanosecond => {
                            array.as_primitive::<DurationNanosecondType>().value(row)
                        }
                    };
                    match micros(raw, *unit) {
                        Some(micros) => Value::Interval(IntervalValue {
                            months: 0,
                            days: 0,
                            micros,
                        }),
                        None => text(row),
                    }
                })
                .collect(),
            DataType::Interval(IntervalUnit::YearMonth) => {
                each!(array.as_primitive::<IntervalYearMonthType>(), i, {
                    Value::Interval(IntervalValue {
                        months: array.as_primitive::<IntervalYearMonthType>().value(i),
                        days: 0,
                        micros: 0,
                    })
                })
            }
            DataType::Interval(IntervalUnit::DayTime) => {
                each!(array.as_primitive::<IntervalDayTimeType>(), i, {
                    let part = array.as_primitive::<IntervalDayTimeType>().value(i);
                    Value::Interval(IntervalValue {
                        months: 0,
                        days: part.days,
                        micros: i64::from(part.milliseconds) * 1_000,
                    })
                })
            }
            DataType::Interval(IntervalUnit::MonthDayNano) => {
                each!(array.as_primitive::<IntervalMonthDayNanoType>(), i, {
                    let part = array.as_primitive::<IntervalMonthDayNanoType>().value(i);
                    Value::Interval(IntervalValue {
                        months: part.months,
                        days: part.days,
                        micros: part.nanoseconds.div_euclid(1_000),
                    })
                })
            }
            DataType::Decimal32(_, scale) => each!(array.as_primitive::<Decimal32Type>(), i, {
                decimal(
                    i128::from(array.as_primitive::<Decimal32Type>().value(i)),
                    *scale,
                    || text(i),
                )
            }),
            DataType::Decimal64(_, scale) => each!(array.as_primitive::<Decimal64Type>(), i, {
                decimal(
                    i128::from(array.as_primitive::<Decimal64Type>().value(i)),
                    *scale,
                    || text(i),
                )
            }),
            DataType::Decimal128(_, scale) => each!(array.as_primitive::<Decimal128Type>(), i, {
                decimal(
                    array.as_primitive::<Decimal128Type>().value(i),
                    *scale,
                    || text(i),
                )
            }),
            DataType::Decimal256(_, scale) => each!(array.as_primitive::<Decimal256Type>(), i, {
                match array.as_primitive::<Decimal256Type>().value(i).to_i128() {
                    Some(unscaled) => decimal(unscaled, *scale, || text(i)),
                    None => Value::Text(
                        array
                            .as_primitive::<Decimal256Type>()
                            .value_as_string(i)
                            .into(),
                    ),
                }
            }),
            DataType::Dictionary(_, value_type) => {
                let values = arrow_cast::cast(array, value_type)?;
                return self.cells(&values, json);
            }
            DataType::RunEndEncoded(_, value_field) => {
                let values = arrow_cast::cast(array, value_field.data_type())?;
                return self.cells(&values, json);
            }
            DataType::List(_) => {
                let list = array.as_list::<i32>();
                let children = self.cells(list.values(), false)?;
                let offsets = list.value_offsets();
                each!(
                    list,
                    i,
                    Value::Array(children[offsets[i] as usize..offsets[i + 1] as usize].to_vec())
                )
            }
            DataType::LargeList(_) => {
                let list = array.as_list::<i64>();
                let children = self.cells(list.values(), false)?;
                let offsets = list.value_offsets();
                each!(
                    list,
                    i,
                    Value::Array(children[offsets[i] as usize..offsets[i + 1] as usize].to_vec())
                )
            }
            DataType::FixedSizeList(_, width) => {
                let list = array.as_fixed_size_list();
                let children = self.cells(list.values(), false)?;
                let width =
                    usize::try_from(*width).map_err(|_| corrupt("a negative list width"))?;
                each!(
                    list,
                    i,
                    Value::Array(children[i * width..(i + 1) * width].to_vec())
                )
            }
            DataType::Struct(_) => {
                let record = array.as_struct();
                let columns = record
                    .columns()
                    .iter()
                    .map(|column| self.cells(column, false))
                    .collect::<Result<Vec<_>, _>>()?;
                each!(
                    record,
                    i,
                    Value::Row(columns.iter().map(|column| column[i].clone()).collect())
                )
            }
            DataType::Map(_, _) => {
                let map = array.as_map();
                let keys = self.cells(map.keys(), false)?;
                let values = self.cells(map.values(), false)?;
                let offsets = map.value_offsets();
                each!(map, i, {
                    let range = offsets[i] as usize..offsets[i + 1] as usize;
                    Value::Map(
                        keys[range.clone()]
                            .iter()
                            .cloned()
                            .zip(values[range].iter().cloned())
                            .collect(),
                    )
                })
            }
            DataType::Union(fields, _) => {
                let union = array.as_union();
                let mut children = HashMap::new();
                for (type_id, _) in fields.iter() {
                    children.insert(type_id, self.cells(union.child(type_id), false)?);
                }
                (0..rows)
                    .map(|row| children[&union.type_id(row)][union.value_offset(row)].clone())
                    .collect()
            }
            // `ListView`, `LargeListView`: no store shape. Text keeps the data.
            _ => (0..rows)
                .map(|row| {
                    if array.is_null(row) {
                        Value::Null
                    } else {
                        text(row)
                    }
                })
                .collect(),
        };
        Ok(cells)
    }

    fn zone(&mut self, name: &str) -> Result<Tz, ColumnarError> {
        if let Some(zone) = self.zones.get(name) {
            return Ok(*zone);
        }
        let zone: Tz = name
            .parse()
            .map_err(|error| corrupt(format!("unknown time zone {name}: {error}")))?;
        self.zones.insert(name.to_owned(), zone);
        Ok(zone)
    }
}

/// The offset `zone` has at this instant, in seconds east of UTC.
fn offset_secs(zone: &Tz, micros: i64) -> Option<i32> {
    let utc = chrono::DateTime::from_timestamp_micros(micros)?.naive_utc();
    Some(zone.offset_from_utc_datetime(&utc).fix().local_minus_utc())
}

/// A decimal cell, with a negative scale multiplied out. Past `i128` it is text.
fn decimal(unscaled: i128, scale: i8, text: impl FnOnce() -> Value) -> Value {
    match rescale(unscaled, scale) {
        Some((unscaled, scale)) => Value::Decimal { unscaled, scale },
        None => text(),
    }
}
