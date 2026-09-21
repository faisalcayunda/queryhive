//! The batch byte format, shared by the in-memory and spilled forms.
//!
//! One encoder and one decoder, used for both. Spilling writes exactly the bytes
//! the in-memory batch holds, so a value read back from disk cannot differ from
//! the same value read from memory — there is no second implementation that
//! could drift.
//!
//! Layout
//! ------
//! ```text
//! u32 rows
//! u32 column_count
//! per column:
//!     u32 offset_count          (= rows + 1)
//!     offset_count × u32        offsets into this column's blob
//!     u32 blob_len
//!     blob bytes                one self-delimiting value per row
//! ```
//!
//! Every integer is little-endian. A cell is `blob[offsets[row]..offsets[row+1]]`
//! — two array indexes and a byte range, which is what makes a window cheap.

use qh_core::{IntervalValue, Value};

use crate::store::StoreError;

// Value tags. The numbers are part of the on-disk format: changing one makes
// every spill file written by an older build unreadable, so they are only ever
// appended to.
const TAG_NULL: u8 = 0;
const TAG_BOOL: u8 = 1;
const TAG_INT: u8 = 2;
const TAG_UINT: u8 = 3;
const TAG_FLOAT: u8 = 4;
const TAG_DECIMAL: u8 = 5;
const TAG_TEXT: u8 = 6;
const TAG_BYTES: u8 = 7;
const TAG_TIMESTAMP: u8 = 8;
const TAG_DATE: u8 = 9;
const TAG_TIME: u8 = 10;
const TAG_INTERVAL: u8 = 11;
const TAG_JSON: u8 = 12;
const TAG_ARRAY: u8 = 13;
const TAG_ROW: u8 = 14;
const TAG_MAP: u8 = 15;
const TAG_UNKNOWN: u8 = 16;

/// Encode one batch's columns into the wire layout above.
///
/// `columns` must be rectangular; the caller gets that guarantee from
/// [`qh_core::ColumnBatch`], which refuses a ragged shape at construction.
pub(crate) fn encode_batch(columns: &[Vec<Value>]) -> Vec<u8> {
    let rows = columns.first().map_or(0, Vec::len);
    let mut out = Vec::with_capacity(rows * columns.len() * 8 + 32);
    out.extend_from_slice(&(rows as u32).to_le_bytes());
    out.extend_from_slice(&(columns.len() as u32).to_le_bytes());

    for column in columns {
        let mut blob = Vec::new();
        let mut offsets = Vec::with_capacity(rows + 1);
        for value in column {
            offsets.push(blob.len() as u32);
            encode_value(&mut blob, value);
        }
        // A sentinel, so the last row's range is `offsets[rows-1]..offsets[rows]`
        // with no special case at the end.
        offsets.push(blob.len() as u32);

        out.extend_from_slice(&(offsets.len() as u32).to_le_bytes());
        for offset in offsets {
            out.extend_from_slice(&offset.to_le_bytes());
        }
        out.extend_from_slice(&(blob.len() as u32).to_le_bytes());
        out.extend_from_slice(&blob);
    }
    out
}

fn encode_value(out: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Null => out.push(TAG_NULL),
        Value::Bool(flag) => {
            out.push(TAG_BOOL);
            out.push(u8::from(*flag));
        }
        Value::Int(number) => {
            out.push(TAG_INT);
            out.extend_from_slice(&number.to_le_bytes());
        }
        Value::UInt(number) => {
            out.push(TAG_UINT);
            out.extend_from_slice(&number.to_le_bytes());
        }
        Value::Float(number) => {
            out.push(TAG_FLOAT);
            out.extend_from_slice(&number.to_le_bytes());
        }
        Value::Decimal { unscaled, scale } => {
            out.push(TAG_DECIMAL);
            out.extend_from_slice(&unscaled.to_le_bytes());
            out.push(*scale);
        }
        Value::Text(text) => {
            out.push(TAG_TEXT);
            encode_bytes(out, text.as_bytes());
        }
        Value::Bytes(bytes) => {
            out.push(TAG_BYTES);
            encode_bytes(out, bytes);
        }
        Value::Json(text) => {
            out.push(TAG_JSON);
            encode_bytes(out, text.as_bytes());
        }
        Value::Timestamp {
            micros,
            offset_secs,
        } => {
            out.push(TAG_TIMESTAMP);
            out.extend_from_slice(&micros.to_le_bytes());
            match offset_secs {
                // The flag keeps "no zone" distinct from "offset zero". A
                // TIMESTAMP and a TIMESTAMPTZ at UTC are not the same value.
                Some(offset) => {
                    out.push(1);
                    out.extend_from_slice(&offset.to_le_bytes());
                }
                None => out.push(0),
            }
        }
        Value::Date { days } => {
            out.push(TAG_DATE);
            out.extend_from_slice(&days.to_le_bytes());
        }
        Value::Time { micros } => {
            out.push(TAG_TIME);
            out.extend_from_slice(&micros.to_le_bytes());
        }
        Value::Interval(interval) => {
            out.push(TAG_INTERVAL);
            out.extend_from_slice(&interval.months.to_le_bytes());
            out.extend_from_slice(&interval.days.to_le_bytes());
            out.extend_from_slice(&interval.micros.to_le_bytes());
        }
        Value::Array(items) => encode_composite(out, TAG_ARRAY, items),
        Value::Row(fields) => encode_composite(out, TAG_ROW, fields),
        Value::Map(pairs) => {
            out.push(TAG_MAP);
            out.extend_from_slice(&(pairs.len() as u32).to_le_bytes());
            for (key, item) in pairs {
                encode_value(out, key);
                encode_value(out, item);
            }
        }
        Value::Unknown {
            type_name,
            text,
            raw,
        } => {
            out.push(TAG_UNKNOWN);
            encode_bytes(out, type_name.as_bytes());
            match (text, raw) {
                (Some(text), _) => {
                    out.push(1);
                    encode_bytes(out, text.as_bytes());
                }
                (None, Some(raw)) => {
                    out.push(2);
                    encode_bytes(out, raw);
                }
                // Neither form: the tag still records that the cell existed, so
                // a row cannot silently lose a column.
                (None, None) => out.push(0),
            }
        }
    }
}

fn encode_composite(out: &mut Vec<u8>, tag: u8, items: &[Value]) {
    out.push(tag);
    out.extend_from_slice(&(items.len() as u32).to_le_bytes());
    for item in items {
        encode_value(out, item);
    }
}

fn encode_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}

/// A parsed view over one encoded batch, positioned so cells are addressable.
pub(crate) struct BatchView<'a> {
    rows: usize,
    /// One entry per column: the offset table and the blob it indexes.
    columns: Vec<(Vec<u32>, &'a [u8])>,
}

impl<'a> BatchView<'a> {
    pub(crate) fn parse(bytes: &'a [u8]) -> Result<Self, StoreError> {
        let mut reader = Reader::new(bytes);
        let rows = reader.u32()? as usize;
        let column_count = reader.u32()? as usize;
        let mut columns = Vec::with_capacity(column_count);

        for _ in 0..column_count {
            let offset_count = reader.u32()? as usize;
            let mut offsets = Vec::with_capacity(offset_count);
            for _ in 0..offset_count {
                offsets.push(reader.u32()?);
            }
            let blob_len = reader.u32()? as usize;
            let blob = reader.take(blob_len)?;
            columns.push((offsets, blob));
        }

        // A column with fewer offsets than rows + 1 would panic later; refuse it
        // here instead, where the error can still say which batch it was.
        for (index, (offsets, _)) in columns.iter().enumerate() {
            if offsets.len() != rows + 1 {
                return Err(StoreError::Corrupt {
                    detail: format!(
                        "column {index} has {} offsets for {rows} rows",
                        offsets.len()
                    ),
                });
            }
        }

        Ok(Self { rows, columns })
    }

    pub(crate) fn rows(&self) -> usize {
        self.rows
    }

    pub(crate) fn width(&self) -> usize {
        self.columns.len()
    }

    /// Decode one cell.
    pub(crate) fn value(&self, row: usize, column: usize) -> Result<Value, StoreError> {
        let (offsets, blob) = self.columns.get(column).ok_or(StoreError::Corrupt {
            detail: format!("no column {column}"),
        })?;
        if row >= self.rows {
            return Err(StoreError::Corrupt {
                detail: format!("row {row} out of range for {} rows", self.rows),
            });
        }
        let start = offsets[row] as usize;
        let end = offsets[row + 1] as usize;
        let slice = blob.get(start..end).ok_or(StoreError::Corrupt {
            detail: format!("cell {row},{column} spans {start}..{end} outside the blob"),
        })?;
        let mut reader = Reader::new(slice);
        let value = decode_value(&mut reader)?;
        Ok(value)
    }
}

fn decode_value(reader: &mut Reader<'_>) -> Result<Value, StoreError> {
    let tag = reader.u8()?;
    match tag {
        TAG_NULL => Ok(Value::Null),
        TAG_BOOL => Ok(Value::Bool(reader.u8()? != 0)),
        TAG_INT => Ok(Value::Int(reader.i64()?)),
        TAG_UINT => Ok(Value::UInt(reader.u64()?)),
        TAG_FLOAT => Ok(Value::Float(f64::from_bits(reader.u64()?))),
        TAG_DECIMAL => {
            let unscaled = reader.i128()?;
            let scale = reader.u8()?;
            Ok(Value::Decimal { unscaled, scale })
        }
        TAG_TEXT => Ok(Value::Text(reader.text()?)),
        TAG_BYTES => Ok(Value::Bytes(reader.length_prefixed()?.to_vec())),
        TAG_JSON => Ok(Value::Json(reader.text()?)),
        TAG_TIMESTAMP => {
            let micros = reader.i64()?;
            let offset_secs = match reader.u8()? {
                0 => None,
                1 => Some(reader.i32()?),
                other => {
                    return Err(StoreError::Corrupt {
                        detail: format!("timestamp offset flag {other}"),
                    })
                }
            };
            Ok(Value::Timestamp {
                micros,
                offset_secs,
            })
        }
        TAG_DATE => Ok(Value::Date {
            days: reader.i32()?,
        }),
        TAG_TIME => Ok(Value::Time {
            micros: reader.i64()?,
        }),
        TAG_INTERVAL => Ok(Value::Interval(IntervalValue {
            months: reader.i32()?,
            days: reader.i32()?,
            micros: reader.i64()?,
        })),
        TAG_ARRAY => Ok(Value::Array(decode_composite(reader)?)),
        TAG_ROW => Ok(Value::Row(decode_composite(reader)?)),
        TAG_MAP => {
            let count = reader.u32()? as usize;
            let mut pairs = Vec::with_capacity(count);
            for _ in 0..count {
                let key = decode_value(reader)?;
                let item = decode_value(reader)?;
                pairs.push((key, item));
            }
            Ok(Value::Map(pairs))
        }
        TAG_UNKNOWN => {
            let type_name = reader.text()?;
            let text = match reader.u8()? {
                0 => None,
                1 => Some(reader.text()?),
                2 => Some(Box::from(
                    String::from_utf8_lossy(reader.length_prefixed()?)
                        .into_owned()
                        .as_str(),
                )),
                other => {
                    return Err(StoreError::Corrupt {
                        detail: format!("unknown-value form {other}"),
                    })
                }
            };
            Ok(Value::Unknown {
                type_name,
                text,
                raw: None,
            })
        }
        other => Err(StoreError::Corrupt {
            detail: format!("value tag {other}"),
        }),
    }
}

fn decode_composite(reader: &mut Reader<'_>) -> Result<Vec<Value>, StoreError> {
    let count = reader.u32()? as usize;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        items.push(decode_value(reader)?);
    }
    Ok(items)
}

/// A bounds-checked cursor. Every read returns an error rather than panicking, so
/// a truncated or damaged spill file cannot take the process down — required by
/// blueprint section 7.2.
struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], StoreError> {
        let end = self
            .position
            .checked_add(count)
            .ok_or_else(|| StoreError::Corrupt {
                detail: "length overflow".to_owned(),
            })?;
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| StoreError::Corrupt {
                detail: format!(
                    "wanted {count} bytes at {} of {}",
                    self.position,
                    self.bytes.len()
                ),
            })?;
        self.position = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, StoreError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, StoreError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn i32(&mut self) -> Result<i32, StoreError> {
        Ok(self.u32()? as i32)
    }

    fn u64(&mut self) -> Result<u64, StoreError> {
        let bytes = self.take(8)?;
        let mut array = [0u8; 8];
        array.copy_from_slice(bytes);
        Ok(u64::from_le_bytes(array))
    }

    fn i64(&mut self) -> Result<i64, StoreError> {
        Ok(self.u64()? as i64)
    }

    fn i128(&mut self) -> Result<i128, StoreError> {
        let bytes = self.take(16)?;
        let mut array = [0u8; 16];
        array.copy_from_slice(bytes);
        Ok(i128::from_le_bytes(array))
    }

    fn length_prefixed(&mut self) -> Result<&'a [u8], StoreError> {
        let len = self.u32()? as usize;
        self.take(len)
    }

    fn text(&mut self) -> Result<Box<str>, StoreError> {
        let bytes = self.length_prefixed()?;
        match std::str::from_utf8(bytes) {
            Ok(text) => Ok(Box::from(text)),
            // Text was written from a Rust `str`, so invalid UTF-8 means the
            // bytes changed underneath us. Reported rather than silently
            // replaced with U+FFFD, which would hide a damaged spill file.
            Err(_) => Err(StoreError::Corrupt {
                detail: "invalid UTF-8 in text".to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant, so a newly added one that nobody taught the codec about
    /// fails here rather than in the grid.
    fn every_value() -> Vec<Value> {
        vec![
            Value::Null,
            Value::Bool(true),
            Value::Bool(false),
            Value::Int(0),
            Value::Int(i64::MIN),
            Value::Int(i64::MAX),
            Value::UInt(u64::MAX),
            Value::Float(0.0),
            Value::Float(-0.0),
            Value::Float(1.5),
            Value::Float(f64::MIN),
            Value::Decimal {
                unscaled: 0,
                scale: 0,
            },
            Value::Decimal {
                unscaled: -1,
                scale: 10,
            },
            Value::Decimal {
                unscaled: 12345678901234567890123456781234567890,
                scale: 10,
            },
            Value::Decimal {
                unscaled: i128::MIN,
                scale: 38,
            },
            Value::Text("".into()),
            Value::Text("hello".into()),
            Value::Text("é中😀".into()),
            Value::Text("with\u{0}nul".into()),
            Value::Text("tab\tand\nnewline".into()),
            Value::Bytes(vec![]),
            Value::Bytes(vec![0x00, 0x01, 0xff]),
            Value::Timestamp {
                micros: 1_769_835_600_123_456,
                offset_secs: Some(7 * 3600),
            },
            Value::Timestamp {
                micros: 1_769_835_600_000_000,
                offset_secs: None,
            },
            Value::Timestamp {
                micros: 0,
                offset_secs: Some(0),
            },
            Value::Timestamp {
                micros: -1,
                offset_secs: Some(-5 * 3600),
            },
            Value::Date { days: 0 },
            Value::Date { days: -25_509 },
            Value::Time {
                micros: 86_399_999_999,
            },
            Value::Interval(IntervalValue {
                months: 1,
                days: 3,
                micros: 14_706_000_000,
            }),
            Value::Interval(IntervalValue {
                months: -2,
                days: -1,
                micros: -1,
            }),
            Value::Json(r#"{"b":1,"a":2}"#.into()),
            Value::Array(vec![]),
            Value::Array(vec![Value::Int(1), Value::Null, Value::Int(3)]),
            Value::Array(vec![Value::Array(vec![Value::Null])]),
            Value::Row(vec![Value::Int(1), Value::Text("a".into())]),
            Value::Map(vec![]),
            Value::Map(vec![(Value::Text("k".into()), Value::Null)]),
            Value::unknown("point", "(1,2)"),
            Value::unknown("inet", ""),
        ]
    }

    #[test]
    fn every_value_survives_a_round_trip() {
        let values = every_value();
        let columns = vec![values.clone()];
        let encoded = encode_batch(&columns);
        let view = BatchView::parse(&encoded).unwrap();

        assert_eq!(view.rows(), values.len());
        assert_eq!(view.width(), 1);
        for (row, expected) in values.iter().enumerate() {
            let found = view.value(row, 0).unwrap();
            assert_eq!(&found, expected, "row {row}");
            // And the rendering is unchanged, which is what the grid shows.
            assert_eq!(found.render_text(), expected.render_text(), "row {row}");
        }
    }

    #[test]
    fn a_float_keeps_its_exact_bits() {
        // Through a NaN and back: comparing with == would fail for NaN, so the
        // bits are what the codec must preserve.
        let columns = vec![vec![
            Value::Float(f64::NAN),
            Value::Float(f64::INFINITY),
            Value::Float(-0.0),
        ]];
        let encoded = encode_batch(&columns);
        let view = BatchView::parse(&encoded).unwrap();
        for (row, expected) in columns[0].iter().enumerate() {
            let Value::Float(found) = view.value(row, 0).unwrap() else {
                panic!("row {row} is not a float");
            };
            let Value::Float(expected) = expected else {
                unreachable!()
            };
            assert_eq!(found.to_bits(), expected.to_bits(), "row {row}");
        }
    }

    #[test]
    fn a_timestamp_without_a_zone_is_not_a_timestamp_at_utc() {
        // Both encode to the same eight bytes of micros; only the flag tells
        // them apart, and the rendering differs.
        let columns = vec![vec![
            Value::Timestamp {
                micros: 0,
                offset_secs: None,
            },
            Value::Timestamp {
                micros: 0,
                offset_secs: Some(0),
            },
        ]];
        let encoded = encode_batch(&columns);
        let view = BatchView::parse(&encoded).unwrap();
        assert_eq!(
            view.value(0, 0).unwrap().render_text().unwrap(),
            "1970-01-01 00:00:00"
        );
        assert_eq!(
            view.value(1, 0).unwrap().render_text().unwrap(),
            "1970-01-01 00:00:00+00:00"
        );
    }

    #[test]
    fn columns_are_independent() {
        let columns = vec![
            vec![Value::Int(1), Value::Int(2)],
            vec![Value::Text("a".into()), Value::Text("bb".into())],
            vec![Value::Null, Value::Bool(true)],
        ];
        let encoded = encode_batch(&columns);
        let view = BatchView::parse(&encoded).unwrap();
        assert_eq!(view.width(), 3);
        assert_eq!(view.value(1, 0).unwrap(), Value::Int(2));
        assert_eq!(view.value(1, 1).unwrap(), Value::Text("bb".into()));
        assert_eq!(view.value(1, 2).unwrap(), Value::Bool(true));
    }

    #[test]
    fn an_empty_batch_round_trips() {
        let columns = vec![Vec::new(), Vec::new()];
        let encoded = encode_batch(&columns);
        let view = BatchView::parse(&encoded).unwrap();
        assert_eq!(view.rows(), 0);
        assert_eq!(view.width(), 2);
        assert!(view.value(0, 0).is_err());
    }

    #[test]
    fn a_truncated_batch_is_an_error_not_a_panic() {
        let columns = vec![vec![Value::Int(1), Value::Int(2)]];
        let encoded = encode_batch(&columns);
        for cut in 0..encoded.len() {
            // Every prefix has to fail cleanly. A damaged spill file must not be
            // able to take the process down (blueprint section 7.2).
            let result = BatchView::parse(&encoded[..cut]);
            if let Ok(view) = result {
                let _ = view.value(0, 0);
                let _ = view.value(1, 0);
            }
        }
    }

    #[test]
    fn a_damaged_value_tag_is_an_error_not_a_panic() {
        // The blob's last byte is the tag of the only cell, so overwriting it
        // with a number no version has used models a spill file that changed
        // underneath us.
        let columns = vec![vec![Value::Int(1)]];
        let mut encoded = encode_batch(&columns);
        let tag_position = encoded.len() - 9;
        assert_eq!(encoded[tag_position], TAG_INT);
        encoded[tag_position] = 200;

        let view = BatchView::parse(&encoded).unwrap();
        assert!(view.value(0, 0).is_err(), "tag 200 should not decode");
    }

    #[test]
    fn offsets_are_four_bytes_of_overhead_per_cell() {
        // Stated in the module docs as the price of random access; pinned here so
        // a change to the layout that doubles it is a decision, not an accident.
        let rows = 100;
        let columns = vec![vec![Value::Null; rows]; 4];
        let encoded = encode_batch(&columns);
        let offsets = columns.len() * (rows + 1) * 4;
        let fixed_header = 8 + columns.len() * 8;
        let tags = rows * columns.len(); // one tag byte per NULL cell
        assert_eq!(encoded.len(), fixed_header + offsets + tags);
    }
}
