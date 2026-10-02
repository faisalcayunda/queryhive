//! The tagged codec: one self-delimiting blob per cell.
//!
//! Moved from `qh-result-store/src/codec.rs` without changing a tag number,
//! so the only behavioral change is the `Unknown` fix below. Mixed,
//! composite, and unknown columns are stored as Arrow `Binary` holding one
//! blob per cell; NULLs use the Arrow validity bitmap, never `TAG_NULL`
//! (which is still recognized inside composites).

use qh_core::{IntervalValue, Value};

use crate::ColumnarError;

// Value tags. The numbers are part of the format: changing one makes every
// blob written by an older build unreadable, so they are only ever appended.
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

/// Presence bits for `Unknown`: bit 0 means `text` follows, bit 1 means
/// `raw` follows. Both may be set, and neither is lossy.
const HAS_TEXT: u8 = 0b01;
const HAS_RAW: u8 = 0b10;

/// The deepest composite nesting a value may have. A server (Trino `ROW`,
/// `ARRAY`, `MAP`) or a helper can hand us an arbitrarily deep value; without
/// a cap the recursive encoder and decoder overflow the stack and abort the
/// process, which no `guarded` wrapper can catch. 64 is far past anything a
/// real query returns (a handful of levels at most) and stays well inside a
/// test thread's stack even in a debug build.
pub const MAX_DEPTH: usize = 64;

/// Encode one cell. `Value::Null` is encoded for composites, which have no
/// validity bitmap; top-level NULLs use the Arrow bitmap instead.
///
/// Returns an error past [`MAX_DEPTH`] rather than recursing without bound.
pub fn encode_value(out: &mut Vec<u8>, value: &Value) -> Result<(), ColumnarError> {
    encode_at(out, value, 0)
}

fn encode_at(out: &mut Vec<u8>, value: &Value, depth: usize) -> Result<(), ColumnarError> {
    if depth > MAX_DEPTH {
        return Err(ColumnarError::Corrupt {
            detail: format!("value nests deeper than {MAX_DEPTH}"),
        });
    }
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
                // The flag keeps "no zone" distinct from "offset zero".
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
        Value::Array(items) => encode_composite(out, TAG_ARRAY, items, depth)?,
        Value::Row(fields) => encode_composite(out, TAG_ROW, fields, depth)?,
        Value::Map(pairs) => {
            out.push(TAG_MAP);
            out.extend_from_slice(&(pairs.len() as u32).to_le_bytes());
            for (key, item) in pairs {
                encode_at(out, key, depth + 1)?;
                encode_at(out, item, depth + 1)?;
            }
        }
        Value::Unknown {
            type_name,
            text,
            raw,
        } => {
            out.push(TAG_UNKNOWN);
            encode_bytes(out, type_name.as_bytes());
            // B-5: presence bits, so a `raw`-only value comes back with its
            // bytes instead of silently becoming lossy text.
            let mut flags = 0;
            if text.is_some() {
                flags |= HAS_TEXT;
            }
            if raw.is_some() {
                flags |= HAS_RAW;
            }
            out.push(flags);
            if let Some(text) = text {
                encode_bytes(out, text.as_bytes());
            }
            if let Some(raw) = raw {
                encode_bytes(out, raw);
            }
        }
    }
    Ok(())
}

fn encode_composite(
    out: &mut Vec<u8>,
    tag: u8,
    items: &[Value],
    depth: usize,
) -> Result<(), ColumnarError> {
    out.push(tag);
    out.extend_from_slice(&(items.len() as u32).to_le_bytes());
    for item in items {
        encode_at(out, item, depth + 1)?;
    }
    Ok(())
}

fn encode_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}
/// Decode one cell from its blob. Every read is bounds-checked and returns
/// an error rather than panicking, so a damaged spill record cannot take
/// the process down. Nesting deeper than [`MAX_DEPTH`] is an error, not a
/// stack overflow.
pub fn decode_value(reader: &mut Reader<'_>) -> Result<Value, ColumnarError> {
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
                    return Err(ColumnarError::Corrupt {
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
            let mut pairs = Vec::with_capacity(count.min(1_000_000));
            for _ in 0..count {
                let key = decode_nested(reader)?;
                pairs.push((key, decode_nested(reader)?));
            }
            Ok(Value::Map(pairs))
        }
        TAG_UNKNOWN => {
            let type_name = reader.text()?;
            let flags = reader.u8()?;
            if flags & !(HAS_TEXT | HAS_RAW) != 0 {
                return Err(ColumnarError::Corrupt {
                    detail: format!("unknown-value flags {flags}"),
                });
            }
            let text = if flags & HAS_TEXT != 0 {
                Some(reader.text()?)
            } else {
                None
            };
            let raw = if flags & HAS_RAW != 0 {
                Some(reader.length_prefixed()?.to_vec())
            } else {
                None
            };
            Ok(Value::Unknown {
                type_name,
                text,
                raw,
            })
        }
        other => Err(ColumnarError::Corrupt {
            detail: format!("value tag {other}"),
        }),
    }
}

fn decode_composite(reader: &mut Reader<'_>) -> Result<Vec<Value>, ColumnarError> {
    let count = reader.u32()? as usize;
    let mut items = Vec::with_capacity(count.min(1_000_000));
    for _ in 0..count {
        items.push(decode_nested(reader)?);
    }
    Ok(items)
}

/// Descend one level, refusing past [`MAX_DEPTH`]. The depth lives on the
/// reader so it survives across the recursive calls without threading a
/// parameter through every arm.
fn decode_nested(reader: &mut Reader<'_>) -> Result<Value, ColumnarError> {
    if reader.depth >= MAX_DEPTH {
        return Err(ColumnarError::Corrupt {
            detail: format!("value nests deeper than {MAX_DEPTH}"),
        });
    }
    reader.depth += 1;
    let value = decode_value(reader);
    reader.depth -= 1;
    value
}
/// A bounds-checked cursor over one blob.
pub struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
    depth: usize,
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            position: 0,
            depth: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.position >= self.bytes.len()
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], ColumnarError> {
        let end = self
            .position
            .checked_add(count)
            .ok_or_else(|| ColumnarError::Corrupt {
                detail: "length overflow".to_owned(),
            })?;
        let slice = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| ColumnarError::Corrupt {
                detail: format!(
                    "wanted {count} bytes at {} of {}",
                    self.position,
                    self.bytes.len()
                ),
            })?;
        self.position = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, ColumnarError> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, ColumnarError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    fn i32(&mut self) -> Result<i32, ColumnarError> {
        Ok(self.u32()? as i32)
    }

    fn u64(&mut self) -> Result<u64, ColumnarError> {
        let bytes = self.take(8)?;
        let mut array = [0u8; 8];
        array.copy_from_slice(bytes);
        Ok(u64::from_le_bytes(array))
    }

    fn i64(&mut self) -> Result<i64, ColumnarError> {
        Ok(self.u64()? as i64)
    }

    fn i128(&mut self) -> Result<i128, ColumnarError> {
        let bytes = self.take(16)?;
        let mut array = [0u8; 16];
        array.copy_from_slice(bytes);
        Ok(i128::from_le_bytes(array))
    }

    fn length_prefixed(&mut self) -> Result<&'a [u8], ColumnarError> {
        let len = self.u32()? as usize;
        if len > 256 * 1024 * 1024 {
            return Err(ColumnarError::Corrupt {
                detail: format!("length prefix {len} over 256 MiB"),
            });
        }
        self.take(len)
    }

    fn text(&mut self) -> Result<Box<str>, ColumnarError> {
        let bytes = self.length_prefixed()?;
        match std::str::from_utf8(bytes) {
            Ok(text) => Ok(Box::from(text)),
            // Text was written from a Rust `str`, so invalid UTF-8 means the
            // bytes changed underneath us. Reported rather than silently
            // replaced, which would hide a damaged spill record.
            Err(_) => Err(ColumnarError::Corrupt {
                detail: "invalid UTF-8 in text".to_owned(),
            }),
        }
    }
}
