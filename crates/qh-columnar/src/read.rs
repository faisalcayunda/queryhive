//! Reading: Arrow arrays back to `Value` and text.
//!
//! `value_at` is the exact inverse of the encoding table: same variants,
//! same scales, same offsets, same bytes. The window path avoids building a
//! `Value` for `text`/`json` (borrowing `&str`) and for `bytes` (hexing the
//! displayed prefix only).

use std::borrow::Cow;

use arrow_array::Array;
use arrow_array::{
    BinaryArray, BooleanArray, Date32Array, Decimal128Array, Float64Array, Int64Array,
    IntervalMonthDayNanoArray, StringArray, Time64MicrosecondArray, TimestampMicrosecondArray,
    UInt64Array,
};
use qh_core::Value;

use crate::encoding::{check_compatible, Encoding};
use crate::tagged;
use crate::ColumnarError;

fn downcast<T: 'static>(array: &dyn Array, enc: Encoding) -> Result<&T, ColumnarError> {
    array
        .as_any()
        .downcast_ref::<T>()
        .ok_or_else(|| ColumnarError::Mismatch {
            encoding: enc.as_str().to_owned(),
            data_type: array.data_type().to_string(),
        })
}

/// Read one cell as an owned `Value`. `Value::Null` for unset validity.
pub fn value_at(array: &dyn Array, enc: Encoding, row: usize) -> Result<Value, ColumnarError> {
    if !check_compatible(enc, array.data_type()) {
        return Err(ColumnarError::Mismatch {
            encoding: enc.as_str().to_owned(),
            data_type: array.data_type().to_string(),
        });
    }
    if array.is_null(row) {
        return Ok(Value::Null);
    }
    match enc {
        Encoding::Null => Ok(Value::Null),
        Encoding::Bool => Ok(Value::Bool(
            downcast::<BooleanArray>(array, enc)?.value(row),
        )),
        Encoding::I64 => Ok(Value::Int(downcast::<Int64Array>(array, enc)?.value(row))),
        Encoding::U64 => Ok(Value::UInt(downcast::<UInt64Array>(array, enc)?.value(row))),
        Encoding::F64 => Ok(Value::Float(
            downcast::<Float64Array>(array, enc)?.value(row),
        )),
        Encoding::Dec => {
            let array = downcast::<Decimal128Array>(array, enc)?;
            let scale = match array.data_type() {
                arrow_schema::DataType::Decimal128(38, scale) => *scale as u8,
                other => {
                    return Err(ColumnarError::Mismatch {
                        encoding: enc.as_str().to_owned(),
                        data_type: other.to_string(),
                    })
                }
            };
            Ok(Value::Decimal {
                unscaled: array.value(row),
                scale,
            })
        }
        Encoding::Date => Ok(Value::Date {
            days: downcast::<Date32Array>(array, enc)?.value(row),
        }),
        Encoding::Time => {
            let micros = downcast::<Time64MicrosecondArray>(array, enc)?.value(row);
            Ok(Value::Time { micros })
        }
        Encoding::Ts => {
            let micros = downcast::<TimestampMicrosecondArray>(array, enc)?.value(row);
            Ok(Value::Timestamp {
                micros,
                offset_secs: None,
            })
        }
        Encoding::Tsz => {
            let array = downcast::<TimestampMicrosecondArray>(array, enc)?;
            let zone = match array.data_type() {
                arrow_schema::DataType::Timestamp(_, Some(zone)) => zone.to_string(),
                other => return Err(mismatch(enc, &other.to_string())),
            };
            let offset = offset_of(&zone).ok_or_else(|| ColumnarError::Corrupt {
                detail: format!("bad column zone {zone}"),
            })?;
            Ok(Value::Timestamp {
                micros: array.value(row),
                offset_secs: Some(offset),
            })
        }
        Encoding::Interval => {
            let part = downcast::<IntervalMonthDayNanoArray>(array, enc)?.value(row);
            let interval = qh_core::IntervalValue {
                months: part.months,
                days: part.days,
                micros: part.nanoseconds / 1000,
            };
            Ok(Value::Interval(interval))
        }
        Encoding::Text => Ok(Value::Text(
            downcast::<StringArray>(array, enc)?.value(row).into(),
        )),
        Encoding::Json => Ok(Value::Json(
            downcast::<StringArray>(array, enc)?.value(row).into(),
        )),
        Encoding::Bytes => Ok(Value::Bytes(
            downcast::<BinaryArray>(array, enc)?.value(row).to_vec(),
        )),
        Encoding::Tagged => {
            let blob = downcast::<BinaryArray>(array, enc)?.value(row);
            let mut reader = tagged::Reader::new(blob);
            let value = tagged::decode_value(&mut reader)?;
            if !reader.is_empty() {
                return Err(ColumnarError::Corrupt {
                    detail: "trailing bytes".to_owned(),
                });
            }
            Ok(value)
        }
    }
}

/// Read one cell's text. `text`/`json` borrow the array; anything else
/// renders through the single renderer into an owned string. `None` is NULL.
pub fn text_at<'a>(
    array: &'a dyn Array,
    enc: Encoding,
    row: usize,
) -> Result<Option<Cow<'a, str>>, ColumnarError> {
    if array.is_null(row) {
        return Ok(None);
    }
    match enc {
        Encoding::Text | Encoding::Json => Ok(Some(Cow::Borrowed(
            downcast::<StringArray>(array, enc)?.value(row),
        ))),
        Encoding::Bytes => {
            let bytes = downcast::<BinaryArray>(array, enc)?.value(row);
            Ok(Some(Cow::Owned(qh_core::render::hex_encode(bytes))))
        }
        _ => Ok(qh_core::render::to_text(&value_at(array, enc, row)?).map(Cow::Owned)),
    }
}

fn mismatch(enc: Encoding, data_type: &str) -> ColumnarError {
    ColumnarError::Mismatch {
        encoding: enc.as_str().to_owned(),
        data_type: data_type.to_owned(),
    }
}
/// Parse an Arrow timezone (`±HH:MM`, `±HHMM`, or `±HH`) to seconds (F-6).
/// Anything else — named zones, second-precision offsets — is `None`, and
/// the builder stores such columns tagged instead.
pub fn offset_of(zone: &str) -> Option<i32> {
    let (sign, digits) = match zone.strip_prefix('+') {
        Some(rest) => (1, rest),
        None => (-1, zone.strip_prefix('-')?),
    };
    let (hours, minutes) = match digits.len() {
        2 => (digits.parse::<i32>().ok()?, 0),
        4 => (
            digits[..2].parse::<i32>().ok()?,
            digits[2..].parse::<i32>().ok()?,
        ),
        5 if digits.as_bytes()[2] == b':' => (
            digits[..2].parse::<i32>().ok()?,
            digits[3..].parse::<i32>().ok()?,
        ),
        _ => return None,
    };
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (hours * 3600 + minutes * 60))
}
