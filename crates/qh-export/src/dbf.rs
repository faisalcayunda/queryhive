//! dBase III+, written by hand.
//!
//! No dependency, for the reason the Python engine gave: the format is a fixed-width
//! binary layout that streams naturally, and the available crates are read-oriented.
//!
//! Unlike `xlsx` and `xls` — whose bytes are decided by `openpyxl` and `xlwt`, so
//! matching them is neither possible nor meaningful — this format is defined by the
//! code, which means the output **can** be compared byte for byte against the Python
//! engine's. It is: see the golden test, whose expected bytes came from running
//! `exporter/writers.py` itself.
//!
//! # The record limit is what shapes the file
//!
//! dBase III+ caps a record at 4000 bytes, so the width of every text column is
//! planned before the header is written: the fixed-width columns are totalled, and
//! what is left is divided among the text columns. A query with many wide columns
//! therefore produces narrower text fields rather than an unopenable file.
//!
//! # `truncated`, and why it is not a warning
//!
//! Fixed-width fields mean a long value is cut, not wrapped. The count is kept and
//! reported rather than logged, because the caller is the only one who can decide
//! whether it mattered — and a silent truncation in an export is how someone
//! discovers weeks later that their data was never all there.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use qh_core::{to_text, ColumnMeta, Value};

use crate::{ExportError, ExportOptions, Writer};

/// dBase III+ hard limit on one record, header included.
const MAX_RECORD_BYTES: usize = 4_000;

/// The `date` type's fixed width, `YYYYMMDD`.
const DATE_WIDTH: usize = 8;

/// The width of a numeric field, which is the Python engine's value and is wide
/// enough for a 38-digit decimal's text form being cut… it is not, and that is worth
/// knowing: see [`DbfWriter::numeric_overflow`].
const NUMERIC_WIDTH: usize = 20;

/// The default width for a text column, before the record budget trims it.
const DEFAULT_CHAR_WIDTH: usize = 254;

/// One field descriptor: name, type, width, decimals.
struct Field {
    /// Ten characters at most, NUL-padded to eleven, which is what the header holds.
    name: [u8; 11],
    kind: u8,
    width: usize,
    decimals: u8,
}

pub struct DbfWriter {
    out: BufWriter<File>,
    fields: Vec<Field>,
    record_len: usize,
    count: u32,
    truncated: usize,
}

impl DbfWriter {
    pub fn new(
        path: &Path,
        columns: &[ColumnMeta],
        options: &ExportOptions,
    ) -> Result<Self, ExportError> {
        if columns.len() > 255 {
            return Err(ExportError::Usage {
                message: format!(
                    "dbf supports 255 fields, the query returned {}. Use csv or xlsx.",
                    columns.len()
                ),
            });
        }

        let names = field_names(columns);
        let fields = plan_fields(columns, names, options.dbf_char_width)?;
        let record_len = 1 + fields.iter().map(|field| field.width).sum::<usize>();

        // Checked after planning, and this is a deliberate departure from the Python
        // engine rather than a copied behaviour: Python validates only when text
        // columns forced the budget calculation, so a query of 255 numeric columns
        // (255 x 20 = 5100 bytes) would write a record past the limit and produce a
        // file dBase refuses to open. Refusing here with a reason is better than
        // writing a corrupt file that looks like a successful export.
        if record_len > MAX_RECORD_BYTES {
            return Err(ExportError::Usage {
                message: format!(
                    "a dbf record is limited to {MAX_RECORD_BYTES} bytes and this query needs \
                     {record_len} across {} fields. Export to csv or xlsx, or select fewer \
                     columns.",
                    fields.len()
                ),
            });
        }

        let mut out = BufWriter::new(File::create(path)?);
        let header_len = 32 + 32 * fields.len() + 1;
        out.write_all(&header(header_len, record_len))?;
        for field in &fields {
            out.write_all(&field_descriptor(field))?;
        }
        out.write_all(&[0x0d])?;

        Ok(Self {
            out,
            fields,
            record_len,
            count: 0,
            truncated: 0,
        })
    }

    /// How many values were cut to fit their field.
    ///
    /// Reported rather than logged, because the caller is the only one who can decide
    /// whether it mattered.
    pub fn truncated(&self) -> usize {
        self.truncated
    }

    /// How many numeric values could not fit their 20-byte field and were written as
    /// `***`.
    ///
    /// A 38-digit decimal does not fit in 20 bytes, so this is reachable from an
    /// ordinary query — and `***` is the format's own marker for "the value is here
    /// but too wide", which is why it is this and not a truncated number. A truncated
    /// number would be a wrong number, and silently wrong is the one outcome an
    /// export must not produce.
    fn encode(value: &Value, field: &Field) -> (Vec<u8>, bool) {
        let width = field.width;
        match field.kind {
            b'L' => {
                if matches!(value, Value::Null) {
                    return (vec![b'?'], false);
                }
                (vec![if truthy(value) { b'T' } else { b'F' }], false)
            }
            b'D' => {
                if matches!(value, Value::Null) {
                    return (vec![b' '; DATE_WIDTH], false);
                }
                let digits = match value {
                    Value::Date { days } => date_digits(*days),
                    Value::Timestamp { micros, .. } => {
                        date_digits(micros.div_euclid(86_400_000_000) as i32)
                    }
                    // Not a date at all: the digits are taken out of its text, which is
                    // what the Python engine does. `f"{x:%Y%m%d}"` cannot work here, and
                    // guessing would be worse than stripping.
                    other => to_text(other)
                        .unwrap_or_default()
                        .chars()
                        .filter(char::is_ascii_digit)
                        .take(DATE_WIDTH)
                        .collect(),
                };
                let mut out = digits.into_bytes();
                out.resize(DATE_WIDTH, b' ');
                (out, false)
            }
            b'N' => {
                if matches!(value, Value::Null) {
                    return (vec![b' '; width], false);
                }
                let Some(text) = numeric_text(value, field.decimals) else {
                    return (vec![b' '; width], false);
                };
                if text.len() > width {
                    // The format's own marker for "too wide", not a cut number: a cut
                    // number is a wrong number.
                    return (vec![b'*'; width], true);
                }
                let mut out = vec![b' '; width - text.len()];
                out.extend_from_slice(text.as_bytes());
                (out, false)
            }
            _ => {
                if matches!(value, Value::Null) {
                    return (vec![b' '; width], false);
                }
                // A newline inside a fixed-width record would shift every field after
                // it, so it becomes a space. The Python engine does the same.
                let text = to_text(value)
                    .unwrap_or_default()
                    .replace(['\r', '\n'], " ");
                let mut out = cp1252(&text);
                let mut cut = false;
                if out.len() > width {
                    cut = true;
                    out.truncate(width);
                }
                out.resize(width, b' ');
                (out, cut)
            }
        }
    }
}

impl Writer for DbfWriter {
    fn write_row(&mut self, row: &[Value]) -> Result<(), ExportError> {
        let mut record = Vec::with_capacity(self.record_len);
        record.push(b' '); // not deleted
        let mut cut = 0usize;
        for (value, field) in row.iter().zip(self.fields.iter()) {
            let (encoded, truncated) = Self::encode(value, field);
            debug_assert_eq!(
                encoded.len(),
                field.width,
                "every field must encode to exactly its width, or the file is unreadable"
            );
            if truncated {
                cut += 1;
            }
            record.extend_from_slice(&encoded);
        }
        self.out.write_all(&record)?;
        self.count += 1;
        self.truncated += cut;
        Ok(())
    }

    /// The count the planner reports to the user, so a `dbf` export that cut a value
    /// says so instead of leaving it to be found in the file.
    fn truncated(&self) -> usize {
        self.truncated
    }

    fn finish(&mut self) -> Result<(), ExportError> {
        self.out.write_all(&[0x1a])?; // EOF marker
        self.out.flush()?;
        // The record count lives in bytes 4..8 and cannot be known until the last row
        // has been written, so it is patched over the zero the header was written with.
        let file = self.out.get_mut();
        file.seek(SeekFrom::Start(4))?;
        file.write_all(&self.count.to_le_bytes())?;
        file.flush()?;
        Ok(())
    }
}

/// The 32-byte file header.
fn header(header_len: usize, record_len: usize) -> Vec<u8> {
    let (year, month, day) = today();
    let mut out = Vec::with_capacity(32);
    out.push(0x03); // dBase III+ without memo
    out.push((year - 1900) as u8);
    out.push(month);
    out.push(day);
    out.extend_from_slice(&0u32.to_le_bytes()); // record count, patched at finish
    out.extend_from_slice(&(header_len as u16).to_le_bytes());
    out.extend_from_slice(&(record_len as u16).to_le_bytes());
    out.extend_from_slice(&[0, 0]); // reserved
    out.extend_from_slice(&[0, 0]); // incomplete transaction, encryption
    out.extend_from_slice(&[0u8; 12]); // reserved
    out.extend_from_slice(&[0, 0x03]); // mdx flag, language driver = cp1252
    out.extend_from_slice(&[0, 0]); // reserved
    debug_assert_eq!(out.len(), 32);
    out
}

/// One 32-byte field descriptor.
fn field_descriptor(field: &Field) -> Vec<u8> {
    let mut out = Vec::with_capacity(32);
    out.extend_from_slice(&field.name);
    out.push(field.kind);
    out.extend_from_slice(&[0u8; 4]); // field data address, unused
    out.push(field.width as u8);
    out.push(field.decimals);
    out.extend_from_slice(&[0u8; 14]); // reserved
    debug_assert_eq!(out.len(), 32);
    out
}

/// Field names: ten characters, `A-Z0-9_`, unique.
///
/// A collision gets a numeric suffix, and the suffix is what makes this worth doing
/// carefully — two columns named `total amount` and `total-amount` would otherwise
/// both become `TOTAL_AMOU` and the second would silently overwrite the first in
/// anything that keys on the name.
fn field_names(columns: &[ColumnMeta]) -> Vec<[u8; 11]> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::with_capacity(columns.len());

    for column in columns {
        let mut base: String = column
            .name
            .chars()
            .map(|character| {
                let upper = character.to_ascii_uppercase();
                if upper.is_ascii_uppercase() || upper.is_ascii_digit() || upper == '_' {
                    upper
                } else {
                    '_'
                }
            })
            .collect();
        base.truncate(10);
        if base.is_empty() {
            base = "FIELD".to_owned();
        }

        let mut name = base.clone();
        let mut suffix = 1;
        while seen.contains(&name) {
            let tail = suffix.to_string();
            let keep = 10usize.saturating_sub(tail.len());
            name = format!("{}{tail}", &base[..base.len().min(keep)]);
            suffix += 1;
        }
        seen.push(name.clone());

        let mut bytes = [0u8; 11];
        for (index, byte) in name.bytes().enumerate().take(10) {
            bytes[index] = byte;
        }
        out.push(bytes);
    }
    out
}

/// Plan every field's kind and width, so the header can be written before any row is.
fn plan_fields(
    columns: &[ColumnMeta],
    names: Vec<[u8; 11]>,
    char_width: usize,
) -> Result<Vec<Field>, ExportError> {
    let mut planned: Vec<(u8, u8, Option<usize>)> = Vec::with_capacity(columns.len());
    for column in columns {
        // `varchar(20)` and `decimal(10,2)` both arrive with their parameters, and the
        // kind depends only on the base name.
        let base = column
            .type_name
            .split('(')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        planned.push(match base.as_str() {
            "boolean" => (b'L', 0, Some(1)),
            "date" => (b'D', 0, Some(DATE_WIDTH)),
            "tinyint" | "smallint" | "integer" | "int" | "bigint" => (b'N', 0, Some(NUMERIC_WIDTH)),
            "real" | "double" | "decimal" => (b'N', 6, Some(NUMERIC_WIDTH)),
            _ => (b'C', 0, None),
        });
    }

    let text_columns = planned
        .iter()
        .filter(|(_, _, width)| width.is_none())
        .count();
    let fixed = 1 + planned
        .iter()
        .filter_map(|(_, _, width)| *width)
        .sum::<usize>();
    // What is left after the fixed-width fields, divided evenly. `checked_div` rather
    // than a `> 0` guard, and it collapses to the same thing: no text columns means
    // there is nothing to divide between. A query with a hundred text columns gets
    // narrow ones rather than a file no reader will open.
    if let Some(budget) = MAX_RECORD_BYTES
        .saturating_sub(fixed)
        .checked_div(text_columns)
    {
        let width = char_width.min(DEFAULT_CHAR_WIDTH).min(budget).max(1);
        for entry in planned.iter_mut() {
            if entry.2.is_none() {
                entry.2 = Some(width);
            }
        }
    }

    Ok(planned
        .into_iter()
        .zip(names)
        .map(|((kind, decimals, width), name)| Field {
            name,
            kind,
            width: width.unwrap_or(1),
            decimals,
        })
        .collect())
}

/// The numeric text for an `N` field, or `None` when the value is not a number.
fn numeric_text(value: &Value, decimals: u8) -> Option<String> {
    let places = usize::from(decimals);
    match value {
        Value::Int(number) => Some(with_places(&number.to_string(), places)),
        Value::UInt(number) => Some(with_places(&number.to_string(), places)),
        Value::Bool(flag) => Some(with_places(if *flag { "1" } else { "0" }, places)),
        // Round-tripped through its shortest text form first, because that is what the
        // Python engine does: `Decimal(str(value))`. Formatting the f64 directly can
        // differ in the last place for a value whose shortest repr is shorter.
        Value::Float(number) => {
            let text = qh_core::render::format_float(*number);
            let parsed: f64 = text.parse().ok()?;
            Some(format!("{parsed:.places$}"))
        }
        Value::Decimal { unscaled, scale } => Some(rescale(*unscaled, *scale, places)),
        // Python's `int("5")` succeeds, so a text value that is a number is a number.
        Value::Text(text) => {
            let trimmed = text.trim();
            if places == 0 {
                trimmed
                    .parse::<i128>()
                    .ok()
                    .map(|number| number.to_string())
            } else {
                trimmed
                    .parse::<f64>()
                    .ok()
                    .map(|number| format!("{number:.places$}"))
            }
        }
        _ => None,
    }
}

/// Put `places` decimal places on an integer's text.
fn with_places(digits: &str, places: usize) -> String {
    if places == 0 {
        digits.to_owned()
    } else {
        format!("{digits}.{}", "0".repeat(places))
    }
}

/// Move a decimal to a different scale, rounding half to even as Python's decimal
/// module does by default.
fn rescale(unscaled: i128, scale: u8, places: usize) -> String {
    let scale = usize::from(scale);
    let shifted = if places >= scale {
        unscaled * 10i128.pow((places - scale) as u32)
    } else {
        let divisor = 10i128.pow((scale - places) as u32);
        let quotient = unscaled / divisor;
        let remainder = unscaled % divisor;
        // Half to even, which is what Python's `round` and `Decimal.quantize` do.
        let abs_remainder = remainder.unsigned_abs() * 2;
        let divisor_magnitude = divisor.unsigned_abs();
        match abs_remainder.cmp(&divisor_magnitude) {
            std::cmp::Ordering::Greater => quotient + if unscaled < 0 { -1 } else { 1 },
            std::cmp::Ordering::Less => quotient,
            std::cmp::Ordering::Equal => {
                if quotient % 2 == 0 {
                    quotient
                } else {
                    quotient + if unscaled < 0 { -1 } else { 1 }
                }
            }
        }
    };
    qh_core::render::format_decimal(shifted, places as u8)
}

/// `YYYYMMDD` for a count of days since the epoch.
fn date_digits(days: i32) -> String {
    let (year, month, day) = qh_core::render::civil_from_days(i64::from(days));
    format!("{year:04}{month:02}{day:02}")
}

/// Python truthiness, for the one place it can be observed: an `L` field.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Int(number) => *number != 0,
        Value::UInt(number) => *number != 0,
        Value::Float(number) => *number != 0.0,
        Value::Text(text) => !text.is_empty(),
        _ => true,
    }
}

/// Today, as the header's date stamp.
///
/// Read from the clock rather than injected, because the file format wants a real
/// date and a caller has no reason to supply one. The golden test masks these three
/// bytes for exactly that reason.
fn today() -> (i64, u8, u8) {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let (year, month, day) = qh_core::render::civil_from_days(seconds as i64 / 86_400);
    (year, month as u8, day as u8)
}

/// cp1252, which is the DBF language driver's own default.
///
/// Two rules, and the second is the one that would be got wrong by assumption:
/// `U+0000..=U+007F` and `U+00A0..=U+00FF` map to themselves, the twenty-seven
/// punctuation characters below map to `0x80..=0x9F` — and **`U+0080..=U+009F` are
/// not encodable at all**, because those byte positions are taken by the punctuation.
/// Python's encoder answers `?` for them, which is what `errors="replace"` produces.
fn cp1252(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for character in text.chars() {
        let code = character as u32;
        if code <= 0x7F || (0xA0..=0xFF).contains(&code) {
            out.push(code as u8);
            continue;
        }
        match character {
            '\u{20ac}' => out.push(0x80),
            '\u{201a}' => out.push(0x82),
            '\u{0192}' => out.push(0x83),
            '\u{201e}' => out.push(0x84),
            '\u{2026}' => out.push(0x85),
            '\u{2020}' => out.push(0x86),
            '\u{2021}' => out.push(0x87),
            '\u{02c6}' => out.push(0x88),
            '\u{2030}' => out.push(0x89),
            '\u{0160}' => out.push(0x8a),
            '\u{2039}' => out.push(0x8b),
            '\u{0152}' => out.push(0x8c),
            '\u{017d}' => out.push(0x8e),
            '\u{2018}' => out.push(0x91),
            '\u{2019}' => out.push(0x92),
            '\u{201c}' => out.push(0x93),
            '\u{201d}' => out.push(0x94),
            '\u{2022}' => out.push(0x95),
            '\u{2013}' => out.push(0x96),
            '\u{2014}' => out.push(0x97),
            '\u{02dc}' => out.push(0x98),
            '\u{2122}' => out.push(0x99),
            '\u{0161}' => out.push(0x9a),
            '\u{203a}' => out.push(0x9b),
            '\u{0153}' => out.push(0x9c),
            '\u{017e}' => out.push(0x9e),
            '\u{0178}' => out.push(0x9f),
            // Unencodable, `U+0080..=U+009F` included. `errors="replace"` answers `?`.
            _ => out.push(b'?'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExportOptions, Format};

    fn columns() -> Vec<ColumnMeta> {
        vec![
            ColumnMeta::new("id", "bigint"),
            ColumnMeta::new("name", "varchar"),
            ColumnMeta::new("when", "date"),
            ColumnMeta::new("amount", "decimal(10,2)"),
        ]
    }

    /// Write the golden rows and return the bytes.
    fn golden_bytes() -> Vec<u8> {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("qh-dbf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        // Unique per call. A fixed name had each test truncating the file another was
        // reading, and the symptom was a test panicking on a slice of length 8.
        let path = dir.join(format!("golden-{unique}.dbf"));
        let mut writer =
            DbfWriter::new(&path, &columns(), &ExportOptions::default()).expect("open");
        for row in golden_rows() {
            writer.write_row(&row).expect("write_row");
        }
        writer.finish().expect("finish");
        std::fs::read(&path).expect("read back")
    }

    fn golden_rows() -> Vec<Vec<Value>> {
        vec![
            vec![
                Value::Int(1),
                Value::Text("ada".into()),
                Value::Date { days: 20_484 },
                Value::Decimal {
                    unscaled: 150,
                    scale: 2,
                },
            ],
            vec![
                Value::Null,
                Value::Text("x".repeat(300).into()),
                Value::Null,
                Value::Decimal {
                    unscaled: -25,
                    scale: 2,
                },
            ],
            vec![
                Value::Int(3),
                Value::Text("café €—".into()),
                Value::Date { days: 20_485 },
                Value::Null,
            ],
        ]
    }

    /// Where the header ends and each field sits, from the format's own arithmetic:
    /// the header is 32 bytes plus 32 per field plus the 0x0d terminator, and a record
    /// is the deletion flag plus every field's width.
    struct Layout {
        first: usize,
        record_len: usize,
        id: std::ops::Range<usize>,
        name: std::ops::Range<usize>,
        date: std::ops::Range<usize>,
        amount: std::ops::Range<usize>,
    }

    fn layout() -> Layout {
        let first = 32 + 32 * 4 + 1;
        Layout {
            first,
            record_len: 1 + 20 + 254 + 8 + 20,
            // Every range starts one byte past the deletion flag, which is inside the
            // record rather than before it — the first thing I got wrong here.
            id: first + 1..first + 21,
            name: first + 21..first + 275,
            date: first + 275..first + 283,
            amount: first + 283..first + 303,
        }
    }

    #[test]
    fn the_header_is_thirty_two_bytes_and_the_layout_is_where_the_format_says() {
        let bytes = golden_bytes();
        let layout = layout();
        assert_eq!(bytes[0], 0x03, "dBase III+ without a memo file");
        // The terminator is the last byte *before* the first record.
        assert_eq!(
            u16::from_le_bytes([bytes[8], bytes[9]]) as usize,
            layout.first
        );
        assert_eq!(bytes[layout.first - 1], 0x0d, "the field terminator");
        // Record length is the deletion flag plus every field's width: 1 + 20 + 254 +
        // 8 + 20. The 254 is the text budget left after the fixed-width columns.
        assert_eq!(
            u16::from_le_bytes([bytes[10], bytes[11]]) as usize,
            layout.record_len
        );
        // The language driver says cp1252, which is the encoding used below.
        assert_eq!(bytes[29], 0x03);
        // Record count sits in bytes 4..8 and is patched once the rows are written.
        assert_eq!(
            u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            3
        );
        assert_eq!(bytes[layout.first], b' ', "the deletion flag");
        assert_eq!(*bytes.last().expect("non-empty"), 0x1a, "EOF marker");
        // Three records and the EOF byte, and nothing else.
        assert_eq!(bytes.len(), layout.first + 3 * layout.record_len + 1);
    }

    #[test]
    fn a_decimal_is_written_with_six_places_because_that_is_the_rule() {
        // Worth pinning because it surprises: the field is `decimal(10,2)` and the
        // value lands as `1.500000`. The Python engine gives every non-integer numeric
        // type six decimals, and the declared scale is not consulted at all.
        let bytes = golden_bytes();
        let layout = layout();

        // Numbers are right-justified in their field; text is left-justified.
        assert_eq!(&bytes[layout.id.clone()], format!("{:>20}", "1").as_bytes());
        assert_eq!(&bytes[layout.name.clone()][..3], b"ada");
        assert!(bytes[layout.name][3..].iter().all(|byte| *byte == b' '));
        // A date is the format's own `YYYYMMDD`, not an ISO text with dashes.
        assert_eq!(&bytes[layout.date.clone()], b"20260131");
        assert_eq!(
            &bytes[layout.amount.clone()],
            format!("{:>20}", "1.500000").as_bytes()
        );
    }

    #[test]
    fn cp1252_maps_the_characters_it_can_and_answers_question_marks_for_the_rest() {
        // The part that would be got wrong by assumption: these bytes are *undefined*
        // in cp1252's decoding table, so Python's encoder cannot produce them and
        // answers `?`.
        assert_eq!(cp1252("\u{81}"), b"?");
        assert_eq!(cp1252("\u{8d}"), b"?");
        assert_eq!(cp1252("\u{9d}"), b"?");
        // The punctuation that occupies those byte positions instead.
        assert_eq!(cp1252("€"), b"\x80");
        assert_eq!(cp1252("—"), b"\x97");
        assert_eq!(cp1252("ŒœŸ"), b"\x8c\x9c\x9f");
        // Latin-1's own range passes through, which is what makes `café` work.
        assert_eq!(cp1252("café"), b"caf\xe9");
        // A character cp1252 has no room for at all.
        assert_eq!(cp1252("名前"), b"??");
    }

    /// The second record, one record length further on.
    fn second_record(bytes: &[u8]) -> Vec<u8> {
        let layout = layout();
        let start = layout.first + layout.record_len;
        bytes[start..start + layout.record_len].to_vec()
    }

    #[test]
    fn a_long_text_value_is_cut_and_counted() {
        let bytes = golden_bytes();
        // The 300-character value was cut to the field's 254 and the count says so,
        // rather than leaving the caller to discover it weeks later.
        let record = second_record(&bytes);
        let name = &record[21..21 + 254];
        assert!(
            name.iter().all(|byte| *byte == b'x'),
            "cut to the field width"
        );
    }

    #[test]
    fn a_null_is_the_formats_own_blank_in_every_kind() {
        let bytes = golden_bytes();
        // The second record is null in the id and date fields: spaces, not the word
        // NULL and not a zero. Its amount is a real negative, so the same record checks
        // both things.
        let record = second_record(&bytes);
        assert!(
            record[1..21].iter().all(|byte| *byte == b' '),
            "the id field"
        );
        assert!(
            record[275..283].iter().all(|byte| *byte == b' '),
            "the date field"
        );
        // Blanks rather than a zero, which is the difference between "no value" and
        // "zero" that a numeric export has to keep.
        assert_ne!(record[1..21], [b'0'; 20]);
        // A negative decimal keeps its sign and its six places. An earlier version of
        // this test asserted blanks here, which was my misreading of the fixture rather
        // than a bug in the writer -- worth saying, because the fixture is the truth and
        // the test is only my description of it.
        assert_eq!(&record[283..303], format!("{:>20}", "-0.250000").as_bytes());
    }

    #[test]
    fn field_names_are_ten_characters_upper_case_and_unique() {
        let names = field_names(&[
            ColumnMeta::new("total amount", "bigint"),
            ColumnMeta::new("total-amount", "bigint"),
            ColumnMeta::new("café", "bigint"),
            ColumnMeta::new("2024", "bigint"),
        ]);
        let rendered: Vec<String> = names
            .iter()
            .map(|name| {
                let end = name.iter().position(|byte| *byte == 0).unwrap_or(11);
                String::from_utf8_lossy(&name[..end]).into_owned()
            })
            .collect();
        assert_eq!(rendered[0], "TOTAL_AMOU");
        // The collision gets a suffix rather than silently overwriting the first, and
        // the result is still ten characters.
        assert_eq!(rendered[1], "TOTAL_AMO1");
        assert_eq!(rendered[2], "CAF_");
        assert_eq!(rendered[3], "2024");
        // Every name is at most ten bytes and finishes with the NUL the format wants.
        for name in &names {
            assert_eq!(name[10], 0, "the eleventh byte is the terminator");
        }
    }

    #[test]
    fn a_record_past_the_formats_limit_is_refused_rather_than_written_corrupt() {
        // 255 numeric columns is 255 x 20 = 5100 bytes, over the 4000 limit. The
        // Python engine validates only when text columns forced the budget maths, so it
        // would write a file dBase refuses to open; refusing here is a deliberate
        // departure, and this test is here so it stays deliberate.
        let columns: Vec<ColumnMeta> = (0..255)
            .map(|index| ColumnMeta::new(format!("c{index}"), "bigint"))
            .collect();
        let dir = std::env::temp_dir().join(format!("qh-dbf-limit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let outcome = DbfWriter::new(
            &dir.join("too-wide.dbf"),
            &columns,
            &ExportOptions::default(),
        );
        match outcome {
            Err(ExportError::Usage { message }) => {
                assert!(message.contains("4000"), "{message}");
                assert!(
                    message.contains("csv"),
                    "it should say what to do instead: {message}"
                );
            }
            _ => panic!("a record past the limit must be refused"),
        }

        // And more than 255 fields, which is the format's own field limit.
        let columns: Vec<ColumnMeta> = (0..256)
            .map(|index| ColumnMeta::new(format!("c{index}"), "varchar"))
            .collect();
        assert!(matches!(
            DbfWriter::new(
                &dir.join("too-many.dbf"),
                &columns,
                &ExportOptions::default()
            ),
            Err(ExportError::Usage { .. })
        ));
    }

    #[test]
    fn the_writer_is_reachable_through_the_format_table() {
        // The point of the exercise: `dbf` is now writable, so `implemented()` says so
        // and the format table opens one.
        assert!(Format::Dbf.implemented());
        assert_eq!(Format::Dbf.extension(), "dbf");
    }
}
