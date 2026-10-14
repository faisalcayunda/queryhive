//! Values between dialects: exactly as they are, or refused (W13-T20 §4.4, B-4).
//!
//! A [`PlannedColumn`] says what a column is read as and written as. This module turns one
//! [`Value`] into what the target takes: a field of PostgreSQL's `COPY ... FROM STDIN` text
//! format ([`copy_row`]), or a bound parameter of a MySQL `INSERT` ([`mysql_cell`]).
//!
//! The rule is the same for both and it is strict:
//!
//! - **Only the variant a type is read as is accepted.** A `Text` in a date, timestamp,
//!   numeric or integer column is not a date, it is the driver giving up on a value it could
//!   not parse (a MySQL zero date, a numeric `NaN`), and writing it would be guessing.
//!   `Unknown`, and an `Interval` in a time column, are refused the same way.
//! - **The one exception is closed.** PostgreSQL to PostgreSQL forwards the spellings the
//!   PostgreSQL driver leaves as text (numeric `NaN` and `Infinity`, date and timestamp
//!   `infinity`, ISO `... BC` dates), and only when the text matches a closed pattern
//!   exactly (condition C-2). The patterns are hand-written below, with ASCII digits only.
//! - **Nothing is rounded.** A scale or a fraction the target cannot hold is a refusal, and
//!   so is a date a dialect cannot represent.
//! - **A refusal never contains the value.** [`Rule`] carries a rule and, at most, a limit
//!   of the target, so a message built from it cannot leak a row (R-9 is the server's, not
//!   ours).
//!
//! U+0000 cannot be stored by PostgreSQL in text, and a NUL byte would end a `COPY` field
//! early, so no NUL ever reaches [`copy_row`]'s output: a text with one is refused, and the
//! row is checked once more as bytes after it is built.
//!
//! # Precondition: the source session's text settings are fixed (condition C-3)
//!
//! A value the PostgreSQL driver leaves as text (an array, a `timetz`, an `inet`, a closed
//! spelling above) is forwarded **as the server printed it**, after a structural check only.
//! That is exact only if the server printed it in a form the target reads the same way: a
//! `date[]` printed under `DateStyle = 'SQL, DMY'` passes the check and would arrive with
//! its day and month swapped, which is the failure C-2 exists to stop. So no value from a
//! PostgreSQL source may reach [`pg_text`] or [`copy_row`] until the transfer has set the
//! source session to `DateStyle = ISO`, `IntervalStyle = postgres`, `extra_float_digits = 3`
//! and `bytea_output = hex` and seen those statements succeed. That is `transfer::run`'s
//! (step 7) to enforce; it is written here because this is the code that depends on it.

use std::collections::HashSet;
use std::fmt::Write as _;

use qh_core::render::{civil_from_days, format_decimal};
use qh_core::{IntervalValue, Value};
use qh_driver::DriverKind;

use super::types::{Base, PlannedColumn, SqlType, Target, TargetType, TextSize};

/// Why one value was refused. A rule, never a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// The value is not the kind this type is read as.
    WrongKind {
        expected: &'static str,
    },
    /// PostgreSQL cannot store U+0000 in text.
    Nul,
    OutOfRange {
        what: &'static str,
    },
    /// The value has more decimals than the target column declares (nothing is rounded).
    Scale {
        value: u32,
        target: u32,
    },
    WholeDigits {
        max: u32,
    },
    /// Characters (or bytes, for binary data) beyond the column.
    TooLong {
        max: u64,
        unit: &'static str,
    },
    NonFinite,
    /// More fractional-second digits than the column declares.
    Fraction {
        max: u32,
    },
    DuplicateKey,
    /// A JSON number MySQL `json` cannot keep as written: it stores a number as a 64-bit
    /// integer or a double, and rounds anything else.
    JsonNumber,
    NotJson,
    /// A PostgreSQL array with a lower bound other than 1 cannot be written as is (N-8).
    ArrayBounds,
    NotAnArray,
    NotCanonical {
        what: &'static str,
    },
    /// A value nested deeper than this transfer converts.
    TooDeep,
    /// The row does not have as many values as the table has columns.
    Arity,
}

impl Rule {
    /// A stable key for the rule, which the data-driven fixture names. Not a message.
    pub fn kind(&self) -> &'static str {
        match self {
            Rule::WrongKind { .. } => "wrong_kind",
            Rule::Nul => "nul",
            Rule::OutOfRange { .. } => "out_of_range",
            Rule::Scale { .. } => "scale",
            Rule::WholeDigits { .. } => "whole_digits",
            Rule::TooLong { .. } => "too_long",
            Rule::NonFinite => "non_finite",
            Rule::Fraction { .. } => "fraction",
            Rule::DuplicateKey => "duplicate_key",
            Rule::JsonNumber => "json_number",
            Rule::NotJson => "not_json",
            Rule::ArrayBounds => "array_bounds",
            Rule::NotAnArray => "not_an_array",
            Rule::NotCanonical { .. } => "not_canonical",
            Rule::TooDeep => "too_deep",
            Rule::Arity => "arity",
        }
    }
}

impl std::fmt::Display for Rule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rule::WrongKind { expected } => write!(
                formatter,
                "the value is not {expected}; the driver could not read it as the column's type"
            ),
            Rule::Nul => formatter.write_str("PostgreSQL cannot store U+0000 in text"),
            Rule::OutOfRange { what } => write!(formatter, "{what}"),
            Rule::Scale { value, target } => {
                write!(formatter, "scale {value} exceeds the target's {target}")
            }
            Rule::WholeDigits { max } => {
                write!(formatter, "more than {max} digits before the decimal point")
            }
            Rule::TooLong { max, unit } => {
                write!(formatter, "longer than the column's {max} {unit}")
            }
            Rule::NonFinite => {
                formatter.write_str("NaN and infinity have no counterpart in the target")
            }
            Rule::Fraction { max } => write!(
                formatter,
                "has more than the column's {max} fractional-second digits"
            ),
            Rule::DuplicateKey => formatter.write_str("a JSON object repeats a key"),
            Rule::JsonNumber => formatter.write_str(
                "a JSON number MySQL would round: it keeps only 64-bit integers and doubles",
            ),
            Rule::NotJson => formatter.write_str("the text is not valid JSON"),
            Rule::ArrayBounds => {
                formatter.write_str("an array whose lower bound is not 1 cannot be written as is")
            }
            Rule::NotAnArray => formatter.write_str("the text is not a PostgreSQL array literal"),
            Rule::NotCanonical { what } => write!(formatter, "not a canonical {what}"),
            Rule::TooDeep => formatter.write_str("nested too deeply to convert"),
            Rule::Arity => {
                formatter.write_str("the row has a different number of values than the table")
            }
        }
    }
}

/// A refused value: which column of the row, and the rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Violation {
    pub column: usize,
    pub rule: Rule,
}

impl Violation {
    /// `column "amount", source row 48213: scale 4 exceeds the target's 2`.
    ///
    /// The column name is shown escaped, so a newline in it cannot reshape the line. The
    /// value is not in it, by construction.
    pub fn describe(&self, columns: &[PlannedColumn], source_row: u64) -> String {
        let name = columns
            .get(self.column)
            .map_or_else(String::new, |column| format!("{:?}", column.name));
        format!("column {name}, source row {source_row}: {}", self.rule)
    }
}

/// One bound parameter of a MySQL `INSERT`.
///
/// `Bytes` is not in `qh_driver::Parameter` yet (it is added with the bulk-copy work, step
/// 6 of W13-T20); the rest convert one to one.
#[derive(Debug, Clone, PartialEq)]
pub enum MysqlCell {
    Null,
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
    Bytes(Vec<u8>),
}

// --------------------------------------------------------------------------- //
// PostgreSQL: COPY text
// --------------------------------------------------------------------------- //

/// Append one row in PostgreSQL's `COPY` text format: fields joined by a tab, NULL as `\N`,
/// a newline at the end. On a refusal nothing of the row stays in `out`.
pub fn copy_row<'a>(
    columns: &[PlannedColumn],
    values: impl IntoIterator<Item = &'a Value>,
    out: &mut Vec<u8>,
) -> Result<(), Violation> {
    let start = out.len();
    let outcome = write_copy_row(columns, values, out);
    if outcome.is_err() {
        out.truncate(start);
    }
    outcome
}

fn write_copy_row<'a>(
    columns: &[PlannedColumn],
    values: impl IntoIterator<Item = &'a Value>,
    out: &mut Vec<u8>,
) -> Result<(), Violation> {
    let start = out.len();
    let mut values = values.into_iter();
    for (index, column) in columns.iter().enumerate() {
        let violation = |rule| Violation {
            column: index,
            rule,
        };
        let Some(value) = values.next() else {
            return Err(violation(Rule::Arity));
        };
        if index > 0 {
            out.push(b'\t');
        }
        match pg_text(column, value).map_err(violation)? {
            None => out.extend_from_slice(b"\\N"),
            Some(text) => escape_copy(&text, out),
        }
    }
    if values.next().is_some() {
        return Err(Violation {
            column: columns.len(),
            rule: Rule::Arity,
        });
    }
    out.push(b'\n');
    // Belt and braces: a NUL anywhere in what was built would end a COPY field early.
    if out[start..].contains(&0) {
        return Err(Violation {
            column: 0,
            rule: Rule::Nul,
        });
    }
    Ok(())
}

/// The `COPY` text escapes: backslash, newline, carriage return and tab.
fn escape_copy(text: &str, out: &mut Vec<u8>) {
    for byte in text.bytes() {
        match byte {
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            other => out.push(other),
        }
    }
}

/// One value as the text PostgreSQL parses for the column, before the `COPY` escapes.
/// `None` is NULL.
pub fn pg_text(column: &PlannedColumn, value: &Value) -> Result<Option<String>, Rule> {
    if value.is_null() {
        return Ok(None);
    }
    debug_assert_eq!(column.target, Target::Postgres);
    pg_value(&column.ty, &column.resolved.ty, column.source, value).map(Some)
}

fn wrong(expected: &'static str) -> Rule {
    Rule::WrongKind { expected }
}

fn pg_value(
    sql: &SqlType,
    target: &TargetType,
    source: DriverKind,
    value: &Value,
) -> Result<String, Rule> {
    let pg_to_pg = source == DriverKind::Postgres;
    match (sql.base, target) {
        (Base::Bool, _) => match value {
            Value::Bool(flag) => Ok(if *flag { "t" } else { "f" }.to_owned()),
            _ => Err(wrong("a boolean")),
        },

        (Base::Int8 | Base::Int16 | Base::Int24 | Base::Int32 | Base::Int64, _) => {
            integer(sql, value).map(|number| number.to_string())
        }

        (Base::Float32 | Base::Float64, _) => match value {
            Value::Float(number) if number.is_nan() => Ok("NaN".to_owned()),
            Value::Float(number) if number.is_infinite() => Ok(if *number > 0.0 {
                "Infinity"
            } else {
                "-Infinity"
            }
            .to_owned()),
            Value::Float(number) => Ok(number.to_string()),
            _ => Err(wrong("a floating-point number")),
        },

        (Base::Decimal, TargetType::Numeric { precision, scale }) => match value {
            Value::Decimal { unscaled, scale: s } => {
                decimal_text(*unscaled, *s, *precision, *scale)
            }
            // `NaN` fits any numeric; an infinity only the unconstrained one (PostgreSQL 14 and
            // later), because it exceeds every finite precision.
            Value::Text(text)
                if pg_to_pg
                    && is_numeric_special(text)
                    && (&**text == "NaN" || precision.is_none()) =>
            {
                Ok(text.to_string())
            }
            _ => Err(wrong("an exact decimal")),
        },

        (Base::Text | Base::Char | Base::Enum | Base::Set, _) => match value {
            Value::Text(text) => {
                no_nul(text)?;
                let limit = match target {
                    TargetType::Varchar(n) | TargetType::Char(n) => Some(u64::from(*n)),
                    _ => None,
                };
                if let Some(max) = limit {
                    if text.chars().count() as u64 > max {
                        return Err(Rule::TooLong {
                            max,
                            unit: "characters",
                        });
                    }
                }
                Ok(text.to_string())
            }
            _ => Err(wrong("text")),
        },

        (Base::Uuid, _) => match value {
            Value::Text(text) if is_uuid(text) => Ok(text.to_string()),
            Value::Text(_) => Err(Rule::NotCanonical { what: "uuid" }),
            _ => Err(wrong("a uuid")),
        },

        (Base::Binary, _) => match value {
            Value::Bytes(bytes) => Ok(format!("\\x{}", qh_core::render::hex_encode(bytes))),
            _ => Err(wrong("binary data")),
        },

        (Base::Date, _) => match value {
            Value::Date { days } => date_text(*days, 1, 9999_9999),
            Value::Text(text) if pg_to_pg && is_pg_infinity(text) => Ok(text.to_string()),
            Value::Text(text) if pg_to_pg && is_iso_date_bc(text) => Ok(text.to_string()),
            _ => Err(wrong("a date")),
        },

        (Base::Time, _) => match value {
            Value::Time { micros } => {
                if !(0..=86_400_000_000).contains(micros) {
                    return Err(Rule::OutOfRange {
                        what: "a time outside 00:00:00 to 24:00:00",
                    });
                }
                fraction_fits(*micros, sql.fraction())?;
                Ok(clock_text(*micros))
            }
            _ => Err(wrong("a time")),
        },

        (Base::TimeTz, _) => match value {
            Value::Text(text) if pg_to_pg && is_timetz(text) => Ok(text.to_string()),
            _ => Err(wrong("a time with time zone")),
        },

        (Base::Timestamp, _) => match value {
            Value::Timestamp {
                micros,
                offset_secs: None,
            } => timestamp_text(*micros, sql.fraction(), 1, 9999_9999),
            Value::Text(text) if pg_to_pg && is_pg_infinity(text) => Ok(text.to_string()),
            Value::Text(text) if pg_to_pg && is_iso_timestamp_bc(text, false) => {
                Ok(text.to_string())
            }
            _ => Err(wrong("a timestamp without a zone")),
        },

        (Base::TimestampTz, _) => match value {
            Value::Timestamp {
                micros,
                offset_secs: Some(_),
            } => {
                // The instant, written in UTC: `timestamptz` stores nothing else, and it keeps
                // an offset with seconds (historic local mean time) from being cut to minutes.
                timestamp_text(*micros, sql.fraction(), 1, 9999_9999).map(|text| text + "+00:00")
            }
            Value::Text(text) if pg_to_pg && is_pg_infinity(text) => Ok(text.to_string()),
            Value::Text(text) if pg_to_pg && is_iso_timestamp_bc(text, true) => {
                Ok(text.to_string())
            }
            _ => Err(wrong("a timestamp with a zone")),
        },

        (Base::Interval, _) => match value {
            Value::Interval(interval) => Ok(interval_text(*interval)),
            _ => Err(wrong("an interval")),
        },

        (Base::Json, _) => match value {
            Value::Json(text) => {
                no_nul(text)?;
                // `json` keeps the text exactly, repeated keys included.
                json_valid(text)?;
                Ok(text.to_string())
            }
            _ => Err(wrong("JSON")),
        },
        (Base::Jsonb, _) => match value {
            Value::Json(text) => {
                json_for_jsonb(text)?;
                Ok(text.to_string())
            }
            _ => Err(wrong("JSON")),
        },

        (Base::Array, TargetType::Array(inner)) => pg_array(sql, inner, source, value),
        (Base::Array | Base::Map | Base::Row, TargetType::Jsonb) => {
            json_of_composite(sql.base, value).and_then(|text| {
                json_for_jsonb(&text)?;
                Ok(text)
            })
        }

        (Base::Builtin(_), _) => match value {
            Value::Text(text) if pg_to_pg => {
                no_nul(text)?;
                Ok(text.to_string())
            }
            _ => Err(wrong("text")),
        },

        // A mapping that was refused never becomes a column; a type pair that was not
        // planned for is not written.
        _ => Err(wrong("a value this column was planned for")),
    }
}

/// A PostgreSQL array literal for a Trino array, or the PostgreSQL driver's own text.
fn pg_array(
    sql: &SqlType,
    inner: &TargetType,
    source: DriverKind,
    value: &Value,
) -> Result<String, Rule> {
    match value {
        // The PostgreSQL driver leaves arrays as the server's text. Forwarding it is exact
        // when the elements are written exactly (the mapping checked that), the session's
        // text settings were fixed on the way in (condition C-3), and the array has the
        // default lower bound, which is the only one whose text carries no `[a:b]=` prefix.
        Value::Text(text) if source == DriverKind::Postgres => {
            no_nul(text)?;
            if text.starts_with('[') {
                return Err(Rule::ArrayBounds);
            }
            if text.starts_with('{') && text.ends_with('}') {
                Ok(text.to_string())
            } else {
                Err(Rule::NotAnArray)
            }
        }
        Value::Array(items) if source != DriverKind::Postgres => {
            let element = sql
                .element
                .as_deref()
                .ok_or(wrong("an array with an element type"))?;
            let mut literal = String::from("{");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    literal.push(',');
                }
                if item.is_null() {
                    literal.push_str("NULL");
                    continue;
                }
                let text = pg_value(element, inner, source, item)?;
                literal.push('"');
                for character in text.chars() {
                    if character == '"' || character == '\\' {
                        literal.push('\\');
                    }
                    literal.push(character);
                }
                literal.push('"');
            }
            literal.push('}');
            Ok(literal)
        }
        _ => Err(wrong("an array")),
    }
}

// --------------------------------------------------------------------------- //
// MySQL: bound parameters
// --------------------------------------------------------------------------- //

/// One value as the parameter MySQL binds for the column.
pub fn mysql_cell(column: &PlannedColumn, value: &Value) -> Result<MysqlCell, Rule> {
    if value.is_null() {
        return Ok(MysqlCell::Null);
    }
    debug_assert_eq!(column.target, Target::Mysql);
    mysql_value(&column.ty, &column.resolved.ty, value)
}

/// A whole row of parameters, in column order.
pub fn mysql_row<'a>(
    columns: &[PlannedColumn],
    values: impl IntoIterator<Item = &'a Value>,
) -> Result<Vec<MysqlCell>, Violation> {
    let mut values = values.into_iter();
    let mut cells = Vec::with_capacity(columns.len());
    for (index, column) in columns.iter().enumerate() {
        let violation = |rule| Violation {
            column: index,
            rule,
        };
        let Some(value) = values.next() else {
            return Err(violation(Rule::Arity));
        };
        cells.push(mysql_cell(column, value).map_err(violation)?);
    }
    if values.next().is_some() {
        return Err(Violation {
            column: columns.len(),
            rule: Rule::Arity,
        });
    }
    Ok(cells)
}

fn mysql_value(sql: &SqlType, target: &TargetType, value: &Value) -> Result<MysqlCell, Rule> {
    match sql.base {
        Base::Bool => match value {
            Value::Bool(flag) => Ok(MysqlCell::Int(i64::from(*flag))),
            _ => Err(wrong("a boolean")),
        },

        Base::Int8 | Base::Int16 | Base::Int24 | Base::Int32 | Base::Int64 => {
            let number = integer(sql, value)?;
            Ok(match i64::try_from(number) {
                Ok(small) => MysqlCell::Int(small),
                Err(_) => MysqlCell::UInt(u64::try_from(number).map_err(|_| Rule::OutOfRange {
                    what: "an integer outside 64 bits",
                })?),
            })
        }

        Base::Float32 | Base::Float64 => match value {
            Value::Float(number) if number.is_finite() => Ok(MysqlCell::Float(*number)),
            Value::Float(_) => Err(Rule::NonFinite),
            _ => Err(wrong("a floating-point number")),
        },

        Base::Decimal => match (value, target) {
            (
                Value::Decimal { unscaled, scale },
                TargetType::Numeric {
                    precision,
                    scale: s,
                },
            ) => decimal_text(*unscaled, *scale, *precision, *s).map(MysqlCell::Text),
            _ => Err(wrong("an exact decimal")),
        },

        Base::Text | Base::Char | Base::Enum | Base::Set => match value {
            Value::Text(text) => {
                text_fits_mysql(text, target)?;
                Ok(MysqlCell::Text(text.to_string()))
            }
            _ => Err(wrong("text")),
        },

        Base::Uuid => match value {
            Value::Text(text) if is_uuid(text) => Ok(MysqlCell::Text(text.to_string())),
            Value::Text(_) => Err(Rule::NotCanonical { what: "uuid" }),
            _ => Err(wrong("a uuid")),
        },

        Base::Binary => match (value, target) {
            (Value::Bytes(bytes), TargetType::Binary(Some(max))) => {
                if bytes.len() as u64 > u64::from(*max) {
                    return Err(Rule::TooLong {
                        max: u64::from(*max),
                        unit: "bytes",
                    });
                }
                Ok(MysqlCell::Bytes(bytes.clone()))
            }
            (Value::Bytes(bytes), _) => Ok(MysqlCell::Bytes(bytes.clone())),
            _ => Err(wrong("binary data")),
        },

        Base::Date => match value {
            Value::Date { days } => date_text(*days, 1000, 9999).map(MysqlCell::Text),
            _ => Err(wrong("a date")),
        },

        Base::Time => match value {
            Value::Time { micros } => {
                // MySQL's `time` runs from -838:59:59 to 838:59:59.
                if micros.unsigned_abs() > 3_020_399_000_000 {
                    return Err(Rule::OutOfRange {
                        what: "a time outside -838:59:59 to 838:59:59",
                    });
                }
                fraction_fits(*micros, sql.fraction())?;
                Ok(MysqlCell::Text(signed_clock_text(*micros)))
            }
            _ => Err(wrong("a time")),
        },

        Base::Timestamp => match value {
            Value::Timestamp {
                micros,
                offset_secs: None,
            } => timestamp_text(*micros, sql.fraction(), 1000, 9999).map(MysqlCell::Text),
            _ => Err(wrong("a timestamp without a zone")),
        },
        // The instant as the UTC wall clock (a reported fallback).
        Base::TimestampTz => match value {
            Value::Timestamp {
                micros,
                offset_secs: Some(_),
            } => timestamp_text(*micros, sql.fraction(), 1000, 9999).map(MysqlCell::Text),
            _ => Err(wrong("a timestamp with a zone")),
        },

        Base::Json | Base::Jsonb => match value {
            Value::Json(text) => {
                json_for_mysql(text)?;
                Ok(MysqlCell::Text(text.to_string()))
            }
            _ => Err(wrong("JSON")),
        },
        Base::Array | Base::Map | Base::Row => {
            let text = json_of_composite(sql.base, value)?;
            json_for_mysql(&text)?;
            Ok(MysqlCell::Text(text))
        }

        // Not written to MySQL: the mapping refuses these, so no column has them.
        Base::TimeTz | Base::Interval | Base::Builtin(_) | Base::Other => {
            Err(wrong("a value this column was planned for"))
        }
    }
}

fn text_fits_mysql(text: &str, target: &TargetType) -> Result<(), Rule> {
    match target {
        TargetType::Varchar(n) | TargetType::Char(n) => {
            if text.chars().count() as u64 > u64::from(*n) {
                return Err(Rule::TooLong {
                    max: u64::from(*n),
                    unit: "characters",
                });
            }
        }
        TargetType::Text(size) => {
            let max: u64 = match size {
                TextSize::Text => 65_535,
                TextSize::Medium => 16_777_215,
                TextSize::Long => 4_294_967_295,
            };
            if text.len() as u64 > max {
                return Err(Rule::TooLong { max, unit: "bytes" });
            }
        }
        _ => {}
    }
    Ok(())
}

// --------------------------------------------------------------------------- //
// shared rules
// --------------------------------------------------------------------------- //

fn no_nul(text: &str) -> Result<(), Rule> {
    if text.contains('\0') {
        Err(Rule::Nul)
    } else {
        Ok(())
    }
}

/// An integer read as `sql`, inside the range the source type declares.
fn integer(sql: &SqlType, value: &Value) -> Result<i128, Rule> {
    let number = match value {
        Value::Int(number) => i128::from(*number),
        Value::UInt(number) => i128::from(*number),
        _ => return Err(wrong("an integer")),
    };
    let bytes = sql.base.int_bytes().unwrap_or(8);
    let bits = u32::from(bytes) * 8;
    let (low, high): (i128, i128) = if sql.unsigned {
        (0, (1i128 << bits) - 1)
    } else {
        (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
    };
    if number < low || number > high {
        return Err(Rule::OutOfRange {
            what: "an integer outside its column's range",
        });
    }
    Ok(number)
}

/// `unscaled / 10^scale` for a column of `precision` digits and `target_scale` decimals.
///
/// Trailing zeros beyond the target's scale are dropped (`1.50` into a one-decimal column is
/// exactly `1.5`); a digit that is not zero is a refusal, never a rounding.
fn decimal_text(
    mut unscaled: i128,
    mut scale: u8,
    precision: Option<u32>,
    target_scale: u32,
) -> Result<String, Rule> {
    if precision.is_some() {
        while u32::from(scale) > target_scale && unscaled % 10 == 0 {
            unscaled /= 10;
            scale -= 1;
        }
        if u32::from(scale) > target_scale {
            return Err(Rule::Scale {
                value: u32::from(scale),
                target: target_scale,
            });
        }
    }
    if let Some(precision) = precision {
        let digits = unscaled.unsigned_abs().to_string().len() as u32;
        let whole = digits.saturating_sub(u32::from(scale));
        let max = precision.saturating_sub(target_scale);
        if unscaled != 0 && whole > max {
            return Err(Rule::WholeDigits { max });
        }
    }
    Ok(format_decimal(unscaled, scale))
}

/// `fraction_digits` of a column against the microseconds of a value: nothing is rounded.
fn fraction_fits(micros: i64, digits: u32) -> Result<(), Rule> {
    if digits >= 6 {
        return Ok(());
    }
    let unit = 10i64.pow(6 - digits);
    if micros.rem_euclid(unit) != 0 {
        return Err(Rule::Fraction { max: digits });
    }
    Ok(())
}

/// `YYYY-MM-DD` for a count of days, inside `[low, high]` years.
fn date_text(days: i32, low: i64, high: i64) -> Result<String, Rule> {
    let (year, month, day) = civil_from_days(i64::from(days));
    if year < low || year > high {
        return Err(Rule::OutOfRange {
            what: if low >= 1000 {
                "a date outside 1000-01-01 to 9999-12-31"
            } else {
                "a date before year 1"
            },
        });
    }
    Ok(format!("{year:04}-{month:02}-{day:02}"))
}

/// `HH:MM:SS[.ffffff]` for a non-negative count of microseconds.
fn clock_text(micros: i64) -> String {
    let micros = micros.unsigned_abs();
    let seconds = micros / 1_000_000;
    let fraction = micros % 1_000_000;
    let mut text = format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds / 60) % 60,
        seconds % 60
    );
    if fraction != 0 {
        let _ = write!(text, ".{fraction:06}");
    }
    text
}

fn signed_clock_text(micros: i64) -> String {
    if micros < 0 {
        format!("-{}", clock_text(micros))
    } else {
        clock_text(micros)
    }
}

/// `YYYY-MM-DD HH:MM:SS[.ffffff]` for microseconds since the epoch read as a wall clock.
fn timestamp_text(micros: i64, digits: u32, low: i64, high: i64) -> Result<String, Rule> {
    fraction_fits(micros, digits)?;
    let days = micros.div_euclid(86_400_000_000);
    let within = micros.rem_euclid(86_400_000_000);
    let days = i32::try_from(days).map_err(|_| Rule::OutOfRange {
        what: "a timestamp beyond the supported years",
    })?;
    let date = date_text(days, low, high)?;
    Ok(format!("{date} {}", clock_text(within)))
}

/// PostgreSQL's `interval` input, ISO 8601 with designators: unambiguous under every
/// `IntervalStyle`. Each field keeps its own sign, as the server's `iso_8601` output does.
fn interval_text(interval: IntervalValue) -> String {
    let negative = interval.micros < 0;
    let magnitude = interval.micros.unsigned_abs();
    let whole = magnitude / 1_000_000;
    let fraction = magnitude % 1_000_000;
    let sign = if negative { "-" } else { "" };
    let seconds = if fraction == 0 {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{fraction:06}")
    };
    format!("P{}M{}DT{seconds}S", interval.months, interval.days)
}

// --------------------------------------------------------------------------- //
// the closed spellings (condition C-2)
// --------------------------------------------------------------------------- //

fn digits_between(text: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// `NaN`, `Infinity`, `-Infinity` exactly.
pub fn is_numeric_special(text: &str) -> bool {
    matches!(text, "NaN" | "Infinity" | "-Infinity")
}

/// `infinity`, `-infinity` exactly.
pub fn is_pg_infinity(text: &str) -> bool {
    matches!(text, "infinity" | "-infinity")
}

/// `HH:MM:SS` with an optional fraction of one to six digits. Returns the length matched.
fn clock_prefix(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.len() < 8 {
        return None;
    }
    // Byte checks only: the text may be anything, and slicing it at a byte that is not a
    // character boundary would panic.
    let digit = |index: usize| bytes[index].is_ascii_digit();
    if !([0, 1, 3, 4, 6, 7].iter().all(|index| digit(*index))
        && bytes[2] == b':'
        && bytes[5] == b':')
    {
        return None;
    }
    if bytes.get(8) == Some(&b'.') {
        let fraction = bytes[9..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if (1..=6).contains(&fraction) {
            return Some(9 + fraction);
        }
        return None;
    }
    Some(8)
}

/// `[+-]HH`, optionally `:MM`, optionally `:SS`, and nothing after.
fn is_offset(text: &str) -> bool {
    let Some(rest) = text.strip_prefix(['+', '-']) else {
        return false;
    };
    let mut parts = rest.split(':');
    let hours_ok = parts.next().is_some_and(|part| digits_between(part, 2, 2));
    let rest_ok = parts.clone().count() <= 2 && parts.all(|part| digits_between(part, 2, 2));
    hours_ok && rest_ok
}

/// `^[0-9]{4,}-[0-9]{2}-[0-9]{2}$`, the date part of an ISO date.
fn is_iso_date(text: &str) -> bool {
    let mut parts = text.splitn(3, '-');
    let (Some(year), Some(month), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    year.len() >= 4
        && year.bytes().all(|byte| byte.is_ascii_digit())
        && digits_between(month, 2, 2)
        && digits_between(day, 2, 2)
}

/// `^[0-9]{4,}-[0-9]{2}-[0-9]{2} BC$`.
pub fn is_iso_date_bc(text: &str) -> bool {
    text.strip_suffix(" BC").is_some_and(is_iso_date)
}

/// `^date time(\.[0-9]{1,6})? BC$`, and with an offset before ` BC` when `zoned`.
pub fn is_iso_timestamp_bc(text: &str, zoned: bool) -> bool {
    let Some(body) = text.strip_suffix(" BC") else {
        return false;
    };
    let Some((date, rest)) = body.split_once(' ') else {
        return false;
    };
    if !is_iso_date(date) {
        return false;
    }
    let Some(clock) = clock_prefix(rest) else {
        return false;
    };
    let tail = &rest[clock..];
    if zoned {
        is_offset(tail)
    } else {
        tail.is_empty()
    }
}

/// `^HH:MM:SS(\.f{1,6})?[+-]HH(:MM(:SS)?)?$`: a `timetz` as PostgreSQL prints it.
pub fn is_timetz(text: &str) -> bool {
    clock_prefix(text).is_some_and(|clock| is_offset(&text[clock..]))
}

/// The canonical `8-4-4-4-12` hexadecimal form.
fn is_uuid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12].iter().zip(&groups).all(|(length, group)| {
            group.len() == *length && group.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

// --------------------------------------------------------------------------- //
// JSON
// --------------------------------------------------------------------------- //

const MAX_DEPTH: usize = 64;

fn json_valid(text: &str) -> Result<serde_json::Value, Rule> {
    serde_json::from_str::<serde_json::Value>(text).map_err(|_| Rule::NotJson)
}

fn json_has_nul(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(text) => text.contains('\0'),
        serde_json::Value::Array(items) => items.iter().any(json_has_nul),
        serde_json::Value::Object(map) => map
            .iter()
            .any(|(key, value)| key.contains('\0') || json_has_nul(value)),
        _ => false,
    }
}

/// `jsonb` drops repeated keys, reorders keys, and cannot hold `\u0000`: each is refused
/// rather than changed.
fn json_for_jsonb(text: &str) -> Result<(), Rule> {
    no_nul(text)?;
    let parsed = json_valid(text)?;
    if json_has_nul(&parsed) {
        return Err(Rule::Nul);
    }
    if has_duplicate_keys(text) {
        return Err(Rule::DuplicateKey);
    }
    Ok(())
}

/// MySQL `json` keeps the last of a repeated key, and keeps a number only as a signed or
/// unsigned 64-bit integer or a double: refused rather than changed.
fn json_for_mysql(text: &str) -> Result<(), Rule> {
    json_valid(text)?;
    if has_duplicate_keys(text) {
        return Err(Rule::DuplicateKey);
    }
    json_numbers_fit_mysql(text)
}

/// Every number token of valid JSON `text`, outside its strings, must be one MySQL keeps as it
/// was written: an integer that fits `i64` or `u64`, or a number with a point or an exponent
/// whose nearest `f64`, printed in its shortest round-trip form, is the very same decimal value
/// (so `0.1`, `1.50` and `1e3` pass, and `12345678901234567890.123456789` does not).
fn json_numbers_fit_mysql(text: &str) -> Result<(), Rule> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'"' => {
                index += 1;
                while index < bytes.len() {
                    match bytes[index] {
                        b'\\' => index += 2,
                        b'"' => {
                            index += 1;
                            break;
                        }
                        _ => index += 1,
                    }
                }
            }
            b'-' | b'0'..=b'9' => {
                let start = index;
                while index < bytes.len()
                    && matches!(bytes[index], b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
                {
                    index += 1;
                }
                // The token is ASCII, so both ends are character boundaries.
                if !mysql_keeps_number(&text[start..index]) {
                    return Err(Rule::JsonNumber);
                }
            }
            _ => index += 1,
        }
    }
    Ok(())
}

fn mysql_keeps_number(token: &str) -> bool {
    if !token.contains(['.', 'e', 'E']) {
        return token.parse::<i64>().is_ok() || token.parse::<u64>().is_ok();
    }
    let Ok(nearest) = token.parse::<f64>() else {
        return false;
    };
    if !nearest.is_finite() {
        return false;
    }
    match (decimal_value(token), decimal_value(&format!("{nearest:e}"))) {
        (Some(written), Some(kept)) => written == kept,
        _ => false,
    }
}

/// A decimal number's value as `(negative, digits, power of ten)` with no leading or trailing
/// zero digit, so two spellings of one value compare equal. Zero is `(false, "", 0)`.
fn decimal_value(token: &str) -> Option<(bool, String, i64)> {
    let (negative, rest) = match token.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, token),
    };
    let (mantissa, exponent) = match rest.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i64>().ok()?),
        None => (rest, 0),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut power = exponent.checked_sub(i64::try_from(fraction.len()).ok()?)?;
    let digits = format!("{whole}{fraction}");
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let digits = digits.trim_start_matches('0');
    let trimmed = digits.trim_end_matches('0');
    if trimmed.is_empty() {
        return Some((false, String::new(), 0));
    }
    power = power.checked_add(i64::try_from(digits.len() - trimmed.len()).ok()?)?;
    Some((negative, trimmed.to_owned(), power))
}

/// Whether any JSON object in `text` repeats a key, after the keys' escapes are decoded.
///
/// `text` must be valid JSON (the callers parse it first), which bounds the nesting at the
/// parser's own limit and lets this scan trust the structure.
pub fn has_duplicate_keys(text: &str) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let mut position = 0;
    scan_value(&chars, &mut position, 0)
}

fn skip_space(chars: &[char], position: &mut usize) {
    while chars
        .get(*position)
        .is_some_and(|c| matches!(c, ' ' | '\t' | '\n' | '\r'))
    {
        *position += 1;
    }
}

/// Reads a string at `position` (on its opening quote) and returns it decoded.
fn scan_string(chars: &[char], position: &mut usize) -> String {
    let mut decoded = String::new();
    *position += 1;
    while let Some(&character) = chars.get(*position) {
        *position += 1;
        match character {
            '"' => break,
            '\\' => {
                let Some(&escape) = chars.get(*position) else {
                    break;
                };
                *position += 1;
                match escape {
                    'n' => decoded.push('\n'),
                    't' => decoded.push('\t'),
                    'r' => decoded.push('\r'),
                    'b' => decoded.push('\u{8}'),
                    'f' => decoded.push('\u{c}'),
                    'u' => {
                        let code = read_hex4(chars, position);
                        if (0xD800..0xDC00).contains(&code)
                            && chars.get(*position) == Some(&'\\')
                            && chars.get(*position + 1) == Some(&'u')
                        {
                            let save = *position;
                            *position += 2;
                            let low = read_hex4(chars, position);
                            if (0xDC00..0xE000).contains(&low) {
                                let combined = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                                decoded.push(char::from_u32(combined).unwrap_or('\u{fffd}'));
                                continue;
                            }
                            *position = save;
                        }
                        decoded.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                    }
                    other => decoded.push(other),
                }
            }
            other => decoded.push(other),
        }
    }
    decoded
}

fn read_hex4(chars: &[char], position: &mut usize) -> u32 {
    let mut code = 0u32;
    for _ in 0..4 {
        let digit = chars
            .get(*position)
            .and_then(|c| c.to_digit(16))
            .unwrap_or(0);
        code = code * 16 + digit;
        *position += 1;
    }
    code
}

fn scan_value(chars: &[char], position: &mut usize, depth: usize) -> bool {
    if depth > 256 {
        // The parser refuses past 128, so this is unreachable for validated text; stop
        // rather than recurse without bound if it ever were not.
        return true;
    }
    skip_space(chars, position);
    match chars.get(*position) {
        Some('{') => {
            *position += 1;
            let mut keys: HashSet<String> = HashSet::new();
            loop {
                skip_space(chars, position);
                match chars.get(*position) {
                    Some('}') => {
                        *position += 1;
                        return false;
                    }
                    Some(',') => {
                        *position += 1;
                    }
                    Some('"') => {
                        let key = scan_string(chars, position);
                        if !keys.insert(key) {
                            return true;
                        }
                        skip_space(chars, position);
                        if chars.get(*position) == Some(&':') {
                            *position += 1;
                        }
                        if scan_value(chars, position, depth + 1) {
                            return true;
                        }
                    }
                    _ => return false,
                }
            }
        }
        Some('[') => {
            *position += 1;
            loop {
                skip_space(chars, position);
                match chars.get(*position) {
                    Some(']') => {
                        *position += 1;
                        return false;
                    }
                    Some(',') => {
                        *position += 1;
                    }
                    Some(_) => {
                        if scan_value(chars, position, depth + 1) {
                            return true;
                        }
                    }
                    None => return false,
                }
            }
        }
        Some('"') => {
            scan_string(chars, position);
            false
        }
        Some(_) => {
            while chars
                .get(*position)
                .is_some_and(|c| !matches!(c, ',' | ']' | '}' | ' ' | '\t' | '\n' | '\r'))
            {
                *position += 1;
            }
            false
        }
        None => false,
    }
}

/// JSON text for a map, row or array value (a reported stand-in for what the target cannot
/// hold). Numbers are written from their exact digits; a value with no JSON form is refused.
fn json_of_composite(base: Base, value: &Value) -> Result<String, Rule> {
    match (base, value) {
        (Base::Array, Value::Array(_))
        | (Base::Row, Value::Row(_))
        | (Base::Map, Value::Map(_)) => {
            let mut out = String::new();
            json_write(value, &mut out, 0)?;
            Ok(out)
        }
        (Base::Array, _) => Err(wrong("an array")),
        (Base::Row, _) => Err(wrong("a row")),
        _ => Err(wrong("a map")),
    }
}

fn json_write(value: &Value, out: &mut String, depth: usize) -> Result<(), Rule> {
    if depth > MAX_DEPTH {
        return Err(Rule::TooDeep);
    }
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(flag) => out.push_str(if *flag { "true" } else { "false" }),
        Value::Int(number) => {
            let _ = write!(out, "{number}");
        }
        Value::UInt(number) => {
            let _ = write!(out, "{number}");
        }
        Value::Float(number) if number.is_finite() => {
            let _ = write!(out, "{number}");
        }
        Value::Float(_) => return Err(Rule::NonFinite),
        Value::Decimal { unscaled, scale } => out.push_str(&format_decimal(*unscaled, *scale)),
        Value::Text(text) => json_string(text, out),
        Value::Date { days } => json_string(&date_text(*days, 1, 9999_9999)?, out),
        Value::Time { micros } => json_string(&clock_text(*micros), out),
        Value::Timestamp {
            micros,
            offset_secs,
        } => {
            let text = timestamp_text(*micros, 6, 1, 9999_9999)?;
            json_string(
                &if offset_secs.is_some() {
                    text + "+00:00"
                } else {
                    text
                },
                out,
            );
        }
        Value::Json(text) => {
            json_valid(text)?;
            out.push_str(text);
        }
        Value::Array(items) | Value::Row(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                json_write(item, out, depth + 1)?;
            }
            out.push(']');
        }
        Value::Map(pairs) => {
            out.push('{');
            let mut seen: HashSet<String> = HashSet::new();
            for (index, (key, item)) in pairs.iter().enumerate() {
                let key = json_key(key)?;
                if !seen.insert(key.clone()) {
                    return Err(Rule::DuplicateKey);
                }
                if index > 0 {
                    out.push(',');
                }
                json_string(&key, out);
                out.push(':');
                json_write(item, out, depth + 1)?;
            }
            out.push('}');
        }
        // Raw bytes, an interval and a value the driver could not model have no JSON form
        // that reads back as what they were.
        Value::Bytes(_) => return Err(wrong("a value with a JSON form")),
        Value::Interval(_) => return Err(wrong("a value with a JSON form")),
        Value::Unknown { .. } => return Err(wrong("a value the driver could read")),
    }
    Ok(())
}

/// A map key as the string it becomes in a JSON object.
fn json_key(key: &Value) -> Result<String, Rule> {
    match key {
        Value::Text(text) => Ok(text.to_string()),
        Value::Int(number) => Ok(number.to_string()),
        Value::UInt(number) => Ok(number.to_string()),
        Value::Bool(flag) => Ok(flag.to_string()),
        Value::Decimal { unscaled, scale } => Ok(format_decimal(*unscaled, *scale)),
        Value::Date { days } => date_text(*days, 1, 9999_9999),
        _ => Err(wrong("a map key with a text form")),
    }
}

fn json_string(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", control as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

#[cfg(test)]
// The tests pass a one-column table by value into helpers that take a slice, and read better
// with the clone than with `std::slice::from_ref` on every line.
#[allow(clippy::cloned_ref_to_slice_refs)]
mod tests {
    use super::*;
    use crate::transfer::types::plan_table;

    fn column(ty: SqlType, source: DriverKind, target: Target) -> PlannedColumn {
        let plan = plan_table(source, target, &[("c", &ty)]).expect("a column that maps");
        plan.columns.into_iter().next().unwrap()
    }

    fn pg(ty: SqlType, source: DriverKind) -> PlannedColumn {
        column(ty, source, Target::Postgres)
    }

    fn row_of(columns: &[PlannedColumn], values: &[Value]) -> Result<String, Violation> {
        let mut out = Vec::new();
        copy_row(columns, values.iter(), &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn copy_text_escapes_backslash_newline_carriage_return_and_tab_and_writes_null_as_n() {
        let c = pg(SqlType::of(Base::Text), DriverKind::Postgres);
        let columns = [c.clone(), c];
        let row = row_of(
            &columns,
            &[Value::Text("a\\b\nc\rd\te".into()), Value::Null],
        )
        .unwrap();
        assert_eq!(row, "a\\\\b\\nc\\rd\\te\t\\N\n");
        // The text `\N` is a value and not a NULL.
        let row = row_of(
            &columns,
            &[Value::Text("\\N".into()), Value::Text("".into())],
        )
        .unwrap();
        assert_eq!(row, "\\\\N\t\n");
    }

    #[test]
    fn a_nul_never_reaches_the_output() {
        let c = pg(SqlType::of(Base::Text), DriverKind::Mysql);
        let columns = [c];
        let mut out = b"keep".to_vec();
        let error = copy_row(&columns, [&Value::Text("a\0b".into())], &mut out).unwrap_err();
        assert_eq!(error.rule, Rule::Nul);
        assert_eq!(out, b"keep", "a refused row leaves nothing behind");
    }

    #[test]
    fn a_wrong_variant_is_refused_and_a_message_has_no_value() {
        let c = pg(SqlType::of(Base::Date), DriverKind::Mysql);
        let columns = [c];
        // A MySQL zero date arrives as text; PostgreSQL would reject it or, worse, guess.
        let error = copy_row(
            &columns,
            [&Value::Text("0000-00-00".into())],
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(error.rule, Rule::WrongKind { expected: "a date" });
        let message = error.describe(&columns, 48_213);
        assert_eq!(
            message,
            "column \"c\", source row 48213: the value is not a date; the driver could not read it as the column's type"
        );
        assert!(!message.contains("0000"));
    }

    #[test]
    fn a_decimal_that_does_not_fit_is_refused_and_a_zero_tail_is_not_a_rounding() {
        let amount = column(
            SqlType::decimal(10, 2),
            DriverKind::Postgres,
            Target::Postgres,
        );
        let columns = [amount];
        let write = |unscaled, scale| row_of(&columns, &[Value::Decimal { unscaled, scale }]);
        assert_eq!(write(12345, 2).unwrap(), "123.45\n");
        assert_eq!(write(5, 0).unwrap(), "5\n");
        // 1.5000 into two decimals is exactly 1.50.
        assert_eq!(write(15000, 4).unwrap(), "1.50\n");
        let error = write(12_345_678, 4).unwrap_err();
        assert_eq!(
            error.rule,
            Rule::Scale {
                value: 4,
                target: 2
            }
        );
        assert_eq!(
            error.describe(&columns, 48_213),
            "column \"c\", source row 48213: scale 4 exceeds the target's 2"
        );
        // 10 digits, 2 of them decimals: 8 whole digits fit, 9 do not.
        assert!(write(9_999_999_999, 2).is_ok());
        assert_eq!(
            write(99_999_999_999, 2).unwrap_err().rule,
            Rule::WholeDigits { max: 8 }
        );
    }

    #[test]
    fn the_closed_spellings_are_forwarded_only_postgres_to_postgres_and_only_exactly() {
        let numeric = pg(SqlType::decimal(10, 2), DriverKind::Postgres);
        let date = pg(SqlType::of(Base::Date), DriverKind::Postgres);
        let ts = pg(SqlType::of(Base::Timestamp), DriverKind::Postgres);
        let tstz = pg(SqlType::of(Base::TimestampTz), DriverKind::Postgres);
        let ok = |c: &PlannedColumn, text: &str| {
            row_of(std::slice::from_ref(c), &[Value::Text(text.into())]).is_ok()
        };
        // NaN fits any numeric; an infinity only one with no precision.
        let unbounded = pg(SqlType::of(Base::Decimal), DriverKind::Postgres);
        assert!(ok(&numeric, "NaN"));
        for text in ["NaN", "Infinity", "-Infinity"] {
            assert!(ok(&unbounded, text), "{text}");
        }
        for text in ["Infinity", "-Infinity"] {
            assert!(!ok(&numeric, text), "{text}");
        }
        for text in ["nan", "+Infinity", "Infinity ", "inf", "NaN\n", "1e5", ""] {
            assert!(!ok(&numeric, text), "{text:?}");
            assert!(!ok(&unbounded, text), "{text:?}");
        }
        for c in [&date, &ts, &tstz] {
            assert!(ok(c, "infinity") && ok(c, "-infinity"));
            assert!(!ok(c, "Infinity") && !ok(c, "infinity ") && !ok(c, "+infinity"));
        }
        assert!(ok(&date, "0044-03-15 BC"));
        assert!(ok(&date, "10000-03-15 BC"));
        assert!(!ok(&date, "15/03/0044 BC"), "DateStyle SQL, DMY");
        assert!(!ok(&date, "0044-03-15 BC "));
        assert!(!ok(&date, "044-03-15 BC"));
        assert!(!ok(&date, "0044-3-15 BC"));
        assert!(!ok(&date, "0044-03-15 bc"));
        assert!(!ok(&date, "٠٠٤٤-03-15 BC"), "non-ASCII digits");
        assert!(ok(&ts, "0044-03-15 10:20:30 BC"));
        assert!(ok(&ts, "0044-03-15 10:20:30.5 BC"));
        assert!(ok(&ts, "0044-03-15 10:20:30.123456 BC"));
        assert!(
            !ok(&ts, "0044-03-15 10:20:30.1234567 BC"),
            "seven fractional digits"
        );
        assert!(!ok(&ts, "0044-03-15 10:20:30.+01 BC"));
        assert!(
            !ok(&ts, "0044-03-15 10:20:30+01 BC"),
            "an offset on a timestamp"
        );
        assert!(ok(&tstz, "0044-03-15 10:20:30+01 BC"));
        assert!(ok(&tstz, "0044-03-15 10:20:30.25-07:30 BC"));
        assert!(ok(&tstz, "0044-03-15 10:20:30+01:02:03 BC"));
        assert!(!ok(&tstz, "0044-03-15 10:20:30 BC"), "no offset");
        assert!(!ok(&tstz, "0044-03-15 10:20:30+01:02:03:04 BC"));
        assert!(!ok(&tstz, "0044-03-15 10:20:30+1 BC"));

        // The same text from another source is only text in a column that wants a date.
        for source in [DriverKind::Mysql, DriverKind::Trino] {
            let d = pg(SqlType::of(Base::Date), source);
            assert!(!ok(&d, "infinity"));
            let n = pg(SqlType::decimal(10, 2), source);
            assert!(!ok(&n, "NaN"));
        }
    }

    #[test]
    fn a_timetz_is_forwarded_postgres_to_postgres_when_it_has_the_iso_form() {
        let c = pg(SqlType::of(Base::TimeTz), DriverKind::Postgres);
        let ok = |text: &str| row_of(std::slice::from_ref(&c), &[Value::Text(text.into())]).is_ok();
        assert!(ok("10:20:30+07"));
        assert!(ok("10:20:30.5-07:30"));
        assert!(!ok("10:20:30"));
        assert!(!ok("10:20:30 +07"));
        assert!(!ok("10:20:30.1234567+07"));
    }

    #[test]
    fn dates_and_timestamps_keep_their_calendar_and_refuse_what_a_dialect_cannot_hold() {
        let date = pg(SqlType::of(Base::Date), DriverKind::Trino);
        assert_eq!(
            row_of(&[date.clone()], &[Value::Date { days: 0 }]).unwrap(),
            "1970-01-01\n"
        );
        assert_eq!(
            row_of(&[date.clone()], &[Value::Date { days: -1 }]).unwrap(),
            "1969-12-31\n"
        );
        // Before year 1 has no ISO text PostgreSQL reads; refused, not guessed.
        assert!(matches!(
            row_of(&[date], &[Value::Date { days: -800_000 }])
                .unwrap_err()
                .rule,
            Rule::OutOfRange { .. }
        ));

        let my_date = column(SqlType::of(Base::Date), DriverKind::Mysql, Target::Mysql);
        assert_eq!(
            mysql_cell(&my_date, &Value::Date { days: 0 }).unwrap(),
            MysqlCell::Text("1970-01-01".into())
        );
        // 0999-12-31 and 10000-01-01 are outside MySQL's range.
        for days in [-354_286, 2_932_897] {
            assert!(
                matches!(
                    mysql_cell(&my_date, &Value::Date { days }),
                    Err(Rule::OutOfRange { .. })
                ),
                "{days}"
            );
        }

        let ts = pg(
            SqlType::of(Base::Timestamp).with_fraction(3),
            DriverKind::Postgres,
        );
        let stamp = |micros| Value::Timestamp {
            micros,
            offset_secs: None,
        };
        assert_eq!(
            row_of(&[ts.clone()], &[stamp(1_000)]).unwrap(),
            "1970-01-01 00:00:00.001000\n"
        );
        // A microsecond a three-digit column cannot hold is a refusal, not a rounding.
        assert_eq!(
            row_of(&[ts.clone()], &[stamp(1_500)]).unwrap_err().rule,
            Rule::Fraction { max: 3 }
        );
        // A zoned value in a zoneless column is not the same kind of value.
        assert!(row_of(
            &[ts],
            &[Value::Timestamp {
                micros: 0,
                offset_secs: Some(3600)
            }]
        )
        .is_err());
    }

    #[test]
    fn a_timestamptz_is_written_as_its_instant_in_utc_and_to_mysql_as_the_utc_wall_clock() {
        let tz = pg(SqlType::of(Base::TimestampTz), DriverKind::Trino);
        // 2026-01-31 12:00:00+07:00 is 05:00Z; an offset with seconds changes nothing.
        let instant = 1_769_835_600_000_000;
        for offset in [25_200, 25_632] {
            let value = Value::Timestamp {
                micros: instant,
                offset_secs: Some(offset),
            };
            assert_eq!(
                row_of(&[tz.clone()], &[value]).unwrap(),
                "2026-01-31 05:00:00+00:00\n"
            );
        }
        let my = column(
            SqlType::of(Base::TimestampTz),
            DriverKind::Postgres,
            Target::Mysql,
        );
        assert_eq!(
            mysql_cell(
                &my,
                &Value::Timestamp {
                    micros: instant,
                    offset_secs: Some(0)
                }
            )
            .unwrap(),
            MysqlCell::Text("2026-01-31 05:00:00".into())
        );
        // A zoned column wants a zone: a naive value is not an instant.
        assert!(row_of(
            &[tz],
            &[Value::Timestamp {
                micros: 0,
                offset_secs: None
            }]
        )
        .is_err());
    }

    #[test]
    fn mysql_datetime_range_and_time_range_are_checked_per_value() {
        let dt = column(
            SqlType::of(Base::Timestamp).with_fraction(0),
            DriverKind::Mysql,
            Target::Mysql,
        );
        let at = |year_days: i64| Value::Timestamp {
            micros: year_days * 86_400_000_000,
            offset_secs: None,
        };
        assert!(mysql_cell(&dt, &at(0)).is_ok());
        assert!(mysql_cell(&dt, &at(-354_286)).is_err(), "0999-12-31");
        assert!(mysql_cell(&dt, &at(2_932_897)).is_err(), "10000-01-01");

        let time = column(
            SqlType::of(Base::Time).with_fraction(0),
            DriverKind::Mysql,
            Target::Mysql,
        );
        assert_eq!(
            mysql_cell(
                &time,
                &Value::Time {
                    micros: -3_020_399_000_000
                }
            )
            .unwrap(),
            MysqlCell::Text("-838:59:59".into())
        );
        assert!(mysql_cell(
            &time,
            &Value::Time {
                micros: 3_020_400_000_000
            }
        )
        .is_err());
        // The same value into PostgreSQL: outside a day, so refused.
        let pg_time = pg(SqlType::of(Base::Time).with_fraction(0), DriverKind::Mysql);
        assert!(row_of(
            &[pg_time.clone()],
            &[Value::Time {
                micros: 90_000_000_000
            }]
        )
        .is_err());
        assert!(row_of(&[pg_time.clone()], &[Value::Time { micros: -1_000_000 }]).is_err());
        assert_eq!(
            row_of(
                &[pg_time.clone()],
                &[Value::Time {
                    micros: 86_399_000_000
                }]
            )
            .unwrap(),
            "23:59:59\n"
        );
        // An interval is not a time.
        assert!(row_of(&[pg_time], &[Value::Interval(IntervalValue::default())]).is_err());
    }

    #[test]
    fn floats_keep_every_digit_and_nan_goes_to_postgres_only() {
        let f = pg(SqlType::of(Base::Float64), DriverKind::Postgres);
        for (value, text) in [
            (0.1, "0.1\n"),
            (-0.0, "-0\n"),
            (1e21, "1000000000000000000000\n"),
            (f64::NAN, "NaN\n"),
            (f64::INFINITY, "Infinity\n"),
            (f64::NEG_INFINITY, "-Infinity\n"),
        ] {
            assert_eq!(row_of(&[f.clone()], &[Value::Float(value)]).unwrap(), text);
        }
        // The shortest text reads back as the very same double.
        for value in [0.1f64, 1.0 / 3.0, 5e-324, f64::MAX, 123456.789e-10] {
            let text = row_of(&[f.clone()], &[Value::Float(value)]).unwrap();
            assert_eq!(text.trim().parse::<f64>().unwrap(), value);
        }
        let my = column(
            SqlType::of(Base::Float64),
            DriverKind::Postgres,
            Target::Mysql,
        );
        assert_eq!(
            mysql_cell(&my, &Value::Float(0.5)).unwrap(),
            MysqlCell::Float(0.5)
        );
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                mysql_cell(&my, &Value::Float(value)).unwrap_err(),
                Rule::NonFinite
            );
        }
    }

    #[test]
    fn integers_are_checked_against_the_type_they_were_read_as() {
        let tiny = column(
            SqlType::of(Base::Int8).unsigned(),
            DriverKind::Mysql,
            Target::Postgres,
        );
        assert_eq!(
            row_of(&[tiny.clone()], &[Value::Int(255)]).unwrap(),
            "255\n"
        );
        assert!(row_of(&[tiny], &[Value::Int(256)]).is_err());
        let big = column(
            SqlType::of(Base::Int64).unsigned(),
            DriverKind::Mysql,
            Target::Postgres,
        );
        assert_eq!(
            row_of(&[big.clone()], &[Value::UInt(u64::MAX)]).unwrap(),
            "18446744073709551615\n"
        );
        let my = column(
            SqlType::of(Base::Int64).unsigned(),
            DriverKind::Mysql,
            Target::Mysql,
        );
        assert_eq!(
            mysql_cell(&my, &Value::UInt(u64::MAX)).unwrap(),
            MysqlCell::UInt(u64::MAX)
        );
        assert_eq!(mysql_cell(&my, &Value::Int(7)).unwrap(), MysqlCell::Int(7));
        // A text where an integer belongs is the driver giving up on the value.
        assert!(row_of(&[big], &[Value::Text("12".into())]).is_err());
    }

    #[test]
    fn binary_uuid_and_intervals_have_one_text_each() {
        let bytes = pg(SqlType::of(Base::Binary), DriverKind::Mysql);
        assert_eq!(
            row_of(&[bytes], &[Value::Bytes(vec![0, 1, 0xff])]).unwrap(),
            "\\\\x0001ff\n"
        );
        let my = column(
            SqlType::of(Base::Binary).with_length(2),
            DriverKind::Mysql,
            Target::Mysql,
        );
        assert_eq!(
            mysql_cell(&my, &Value::Bytes(vec![1, 2])).unwrap(),
            MysqlCell::Bytes(vec![1, 2])
        );
        assert!(mysql_cell(&my, &Value::Bytes(vec![1, 2, 3])).is_err());

        let uuid = column(SqlType::of(Base::Uuid), DriverKind::Postgres, Target::Mysql);
        let id = "123e4567-e89b-12d3-a456-426614174000";
        assert_eq!(
            mysql_cell(&uuid, &Value::Text(id.into())).unwrap(),
            MysqlCell::Text(id.into())
        );
        assert!(mysql_cell(&uuid, &Value::Text("123e4567".into())).is_err());

        let interval = pg(SqlType::of(Base::Interval), DriverKind::Postgres);
        let value = Value::Interval(IntervalValue {
            months: 14,
            days: -3,
            micros: -3_500_000,
        });
        assert_eq!(
            row_of(&[interval.clone()], &[value]).unwrap(),
            "P14M-3DT-3.500000S\n"
        );
        assert_eq!(
            row_of(&[interval], &[Value::Interval(IntervalValue::default())]).unwrap(),
            "P0M0DT0S\n"
        );
    }

    #[test]
    fn json_goes_as_it_came_to_json_and_is_checked_for_jsonb_and_mysql() {
        let json = pg(SqlType::of(Base::Json), DriverKind::Mysql);
        let jsonb = pg(SqlType::of(Base::Jsonb), DriverKind::Postgres);
        let my = column(SqlType::of(Base::Json), DriverKind::Postgres, Target::Mysql);
        let dup = Value::Json(r#"{"a":1,"a":2}"#.into());
        // `json` keeps the text and its repeated key; `jsonb` and MySQL would drop one.
        assert_eq!(
            row_of(&[json.clone()], &[dup.clone()]).unwrap(),
            "{\"a\":1,\"a\":2}\n"
        );
        assert_eq!(
            row_of(&[jsonb.clone()], &[dup.clone()]).unwrap_err().rule,
            Rule::DuplicateKey
        );
        assert_eq!(mysql_cell(&my, &dup).unwrap_err(), Rule::DuplicateKey);
        // A key repeated through an escape is still repeated; the same key in two objects is not.
        assert_eq!(
            mysql_cell(&my, &Value::Json(r#"{"a":1,"a":2}"#.into())).unwrap_err(),
            Rule::DuplicateKey
        );
        assert!(mysql_cell(
            &my,
            &Value::Json(r#"{"a":{"a":1},"b":[{"a":1},{"a":2}]}"#.into())
        )
        .is_ok());
        // \u0000 cannot live in jsonb; it is fine in json text and in MySQL.
        let nul = Value::Json(r#"{"a":"x\u0000y"}"#.into());
        assert_eq!(
            row_of(&[jsonb.clone()], &[nul.clone()]).unwrap_err().rule,
            Rule::Nul
        );
        assert!(row_of(&[json], &[nul.clone()]).is_ok());
        assert!(mysql_cell(&my, &nul).is_ok());
        // Not JSON at all.
        assert_eq!(
            row_of(&[jsonb], &[Value::Json("{".into())])
                .unwrap_err()
                .rule,
            Rule::NotJson
        );
    }

    #[test]
    fn mysql_json_keeps_a_number_only_as_a_64_bit_integer_or_a_double() {
        let my = column(SqlType::of(Base::Json), DriverKind::Postgres, Target::Mysql);
        let json = |text: &str| mysql_cell(&my, &Value::Json(text.into()));
        for refused in [
            "12345678901234567890.123456789",
            "[1, 12345678901234567890123]",
            "18446744073709551616",
            "-9223372036854775809",
            "{\"a\": [0.12345678901234567]}",
            "1e-999",
            "4.9e-325",
        ] {
            assert_eq!(json(refused).unwrap_err(), Rule::JsonNumber, "{refused}");
        }
        for kept in [
            "18446744073709551615",
            "-9223372036854775808",
            "0",
            "-0",
            "0.1",
            "1.50",
            "1e3",
            "1E+3",
            "-1.5e-7",
            "123456.789",
            "1.7976931348623157e308",
            "5e-324",
            "{\"s\": \"12345678901234567890123\", \"n\": [1, 2.5]}",
        ] {
            assert!(json(kept).is_ok(), "{kept}");
        }
        // A number too large for a double is not even JSON to the parser that reads it first.
        assert_eq!(json("1e999").unwrap_err(), Rule::NotJson);
        // A number inside a string is a string, wherever its quotes and escapes fall.
        assert!(json(r#"["1\"2345678901234567890123", "\\"]"#).is_ok());
        // PostgreSQL's jsonb keeps numeric exactly, so the same digits go there.
        let jsonb = pg(SqlType::of(Base::Jsonb), DriverKind::Mysql);
        assert!(row_of(
            &[jsonb],
            &[Value::Json("[12345678901234567890.123456789]".into())]
        )
        .is_ok());
    }

    #[test]
    fn a_trino_decimal_inside_a_composite_goes_to_mysql_only_if_a_double_holds_it() {
        let map = column(SqlType::of(Base::Map), DriverKind::Trino, Target::Mysql);
        let entry = |unscaled: i128, scale: u8| {
            Value::Map(vec![(
                Value::Text("v".into()),
                Value::Decimal { unscaled, scale },
            )])
        };
        assert_eq!(
            mysql_cell(&map, &entry(150, 2)).unwrap(),
            MysqlCell::Text("{\"v\":1.50}".into())
        );
        assert_eq!(
            mysql_cell(&map, &entry(123_456_789_012_345_678_901_234_567_890, 5)).unwrap_err(),
            Rule::JsonNumber
        );
        // The same decimal to jsonb is exact.
        let to_pg = pg(SqlType::of(Base::Map), DriverKind::Trino);
        assert!(row_of(
            &[to_pg],
            &[entry(123_456_789_012_345_678_901_234_567_890, 5)]
        )
        .is_ok());
    }

    #[test]
    fn a_trino_array_of_an_exact_element_is_a_postgres_array_and_other_composites_are_json() {
        let ints = SqlType::array_of(SqlType::of(Base::Int32));
        let strings = SqlType::array_of(SqlType::of(Base::Text));
        let c = pg(ints, DriverKind::Trino);
        let value = Value::Array(vec![Value::Int(1), Value::Null, Value::Int(3)]);
        assert_eq!(row_of(&[c], &[value]).unwrap(), "{\"1\",NULL,\"3\"}\n");
        let c = pg(strings, DriverKind::Trino);
        let value = Value::Array(vec![
            Value::Text("a\"b".into()),
            Value::Text("NULL".into()),
            Value::Text("c\\d".into()),
        ]);
        // Quotes and backslashes are escaped for the array literal and then for COPY.
        assert_eq!(
            row_of(&[c.clone()], &[value]).unwrap(),
            "{\"a\\\\\"b\",\"NULL\",\"c\\\\\\\\d\"}\n"
        );
        assert_eq!(
            row_of(&[c.clone()], &[Value::Array(vec![])]).unwrap(),
            "{}\n"
        );
        // A NUL inside an element is refused like any other text.
        assert!(row_of(&[c], &[Value::Array(vec![Value::Text("a\0".into())])]).is_err());

        // An array of arrays, a map and a row have no element type here: JSON.
        let nested = SqlType::array_of(SqlType::array_of(SqlType::of(Base::Int32)));
        let c = pg(nested, DriverKind::Trino);
        let value = Value::Array(vec![
            Value::Array(vec![Value::Int(1)]),
            Value::Array(vec![]),
        ]);
        assert_eq!(row_of(&[c], &[value]).unwrap(), "[[1],[]]\n");
        let map = pg(SqlType::of(Base::Map), DriverKind::Trino);
        let value = Value::Map(vec![
            (
                Value::Text("k".into()),
                Value::Decimal {
                    unscaled: 150,
                    scale: 2,
                },
            ),
            (Value::Int(2), Value::Null),
        ]);
        assert_eq!(
            row_of(&[map.clone()], &[value]).unwrap(),
            "{\"k\":1.50,\"2\":null}\n"
        );
        // A key that repeats once stringified is refused, as is a value with no JSON form.
        let dup = Value::Map(vec![
            (Value::Int(1), Value::Null),
            (Value::Text("1".into()), Value::Null),
        ]);
        assert_eq!(
            row_of(&[map.clone()], &[dup]).unwrap_err().rule,
            Rule::DuplicateKey
        );
        let bytes = Value::Map(vec![(Value::Text("k".into()), Value::Bytes(vec![1]))]);
        assert!(row_of(&[map], &[bytes]).is_err());
        let row = pg(SqlType::of(Base::Row), DriverKind::Trino);
        let value = Value::Row(vec![Value::Int(1), Value::Text("x\u{0}".into())]);
        assert_eq!(
            row_of(&[row], &[value]).unwrap_err().rule,
            Rule::Nul,
            "\\u0000 in jsonb"
        );

        // To MySQL the same values are json text.
        let my = column(
            SqlType::array_of(SqlType::of(Base::Int32)),
            DriverKind::Trino,
            Target::Mysql,
        );
        assert_eq!(
            mysql_cell(&my, &Value::Array(vec![Value::Int(1), Value::Null])).unwrap(),
            MysqlCell::Text("[1,null]".into())
        );
    }

    #[test]
    fn a_postgres_array_is_forwarded_as_its_text_unless_its_bounds_are_not_the_default() {
        let c = pg(
            SqlType::array_of(SqlType::of(Base::Int32)),
            DriverKind::Postgres,
        );
        let ok = |text: &str| row_of(std::slice::from_ref(&c), &[Value::Text(text.into())]);
        assert_eq!(ok("{1,2,3}").unwrap(), "{1,2,3}\n");
        assert_eq!(ok("{{1,2},{3,4}}").unwrap(), "{{1,2},{3,4}}\n");
        assert_eq!(ok("[0:1]={5,6}").unwrap_err().rule, Rule::ArrayBounds);
        assert_eq!(ok("1,2").unwrap_err().rule, Rule::NotAnArray);
        // Neither a Trino array nor a PostgreSQL one reaches MySQL as such: refused up front.
        let int_array = SqlType::array_of(SqlType::of(Base::Int32));
        assert!(plan_table(DriverKind::Postgres, Target::Mysql, &[("c", &int_array)]).is_err());
    }

    #[test]
    fn text_is_checked_against_the_declared_length_in_characters() {
        let c = pg(SqlType::of(Base::Text).with_length(3), DriverKind::Postgres);
        assert!(row_of(&[c.clone()], &[Value::Text("héé".into())]).is_ok());
        assert_eq!(
            row_of(&[c], &[Value::Text("héél".into())])
                .unwrap_err()
                .rule,
            Rule::TooLong {
                max: 3,
                unit: "characters"
            }
        );
        let my = column(
            SqlType::of(Base::Text).with_length(3),
            DriverKind::Postgres,
            Target::Mysql,
        );
        assert!(
            mysql_cell(&my, &Value::Text("日本語".into())).is_ok(),
            "utf8mb4, 3 characters"
        );
        assert!(mysql_cell(&my, &Value::Text("日本語!".into())).is_err());
        // A NUL is fine for MySQL and not for PostgreSQL.
        assert!(mysql_cell(&my, &Value::Text("a\0".into())).is_ok());
    }

    #[test]
    fn a_row_with_the_wrong_number_of_values_is_refused() {
        let c = pg(SqlType::of(Base::Int32), DriverKind::Postgres);
        let columns = [c.clone(), c];
        assert_eq!(
            row_of(&columns, &[Value::Int(1)]).unwrap_err().rule,
            Rule::Arity
        );
        assert_eq!(
            row_of(&columns, &[Value::Int(1), Value::Int(2), Value::Int(3)])
                .unwrap_err()
                .rule,
            Rule::Arity
        );
        assert_eq!(
            row_of(&columns, &[Value::Int(1), Value::Null]).unwrap(),
            "1\t\\N\n"
        );
    }

    /// One value of every variant of `Value`. The match has no wildcard: a new variant
    /// fails to compile here until it is added to the list.
    fn one_of_each_variant() -> Vec<Value> {
        let samples = vec![
            Value::Null,
            Value::Bool(true),
            Value::Int(7),
            Value::UInt(7),
            Value::Float(1.5),
            Value::Decimal {
                unscaled: 150,
                scale: 2,
            },
            Value::Text("7".into()),
            Value::Bytes(vec![7]),
            Value::Timestamp {
                micros: 0,
                offset_secs: None,
            },
            Value::Timestamp {
                micros: 0,
                offset_secs: Some(0),
            },
            Value::Date { days: 0 },
            Value::Time { micros: 0 },
            Value::Interval(IntervalValue::default()),
            Value::Json("7".into()),
            Value::Array(vec![Value::Int(7)]),
            Value::Row(vec![Value::Int(7)]),
            Value::Map(vec![(Value::Text("k".into()), Value::Int(7))]),
            Value::unknown("x", "7"),
        ];
        let kinds: HashSet<u8> = samples
            .iter()
            .map(|value| match value {
                Value::Null => 0,
                Value::Bool(_) => 1,
                Value::Int(_) => 2,
                Value::UInt(_) => 3,
                Value::Float(_) => 4,
                Value::Decimal { .. } => 5,
                Value::Text(_) => 6,
                Value::Bytes(_) => 7,
                Value::Timestamp { .. } => 8,
                Value::Date { .. } => 9,
                Value::Time { .. } => 10,
                Value::Interval(_) => 11,
                Value::Json(_) => 12,
                Value::Array(_) => 13,
                Value::Row(_) => 14,
                Value::Map(_) => 15,
                Value::Unknown { .. } => 16,
            })
            .collect();
        assert_eq!(kinds.len(), 17, "a variant has no sample");
        samples
    }

    #[test]
    fn every_type_meets_every_kind_of_value_without_a_panic_and_a_unknown_never_passes() {
        let samples = one_of_each_variant();
        for source in [DriverKind::Postgres, DriverKind::Mysql, DriverKind::Trino] {
            for target in Target::ALL {
                for base in Base::all() {
                    let mut ty = SqlType::of(base)
                        .with_length(8)
                        .with_fraction(6)
                        .with_native("int); DROP TABLE x; --");
                    ty.precision = Some(10);
                    ty.scale = Some(2);
                    if base == Base::Array {
                        ty.element = Some(Box::new(SqlType::of(Base::Int32)));
                    }
                    let Ok(plan) = plan_table(source, target, &[("c", &ty)]) else {
                        continue;
                    };
                    for value in &samples {
                        let accepted = match target {
                            Target::Postgres => {
                                let mut out = Vec::new();
                                let result = copy_row(&plan.columns, [value], &mut out);
                                if result.is_ok() {
                                    assert!(!out.contains(&0), "{base:?} {value:?}");
                                    assert!(out.ends_with(b"\n"));
                                } else {
                                    assert!(out.is_empty(), "a refused row leaves nothing");
                                }
                                result.is_ok()
                            }
                            Target::Mysql => mysql_cell(&plan.columns[0], value).is_ok(),
                        };
                        if matches!(value, Value::Unknown { .. }) {
                            assert!(
                                !accepted,
                                "{base:?} accepted a value the driver could not read"
                            );
                        }
                        if value.is_null() {
                            assert!(accepted, "{base:?} refused a NULL");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn no_text_makes_a_matcher_panic_or_pass() {
        // Multi-byte characters at every position a matcher slices at, and the empty text.
        let texts = [
            "",
            "é",
            "é:é:é",
            "10:é:30+07",
            "10:20:30.é",
            "10:20:30+é",
            "😀😀😀😀😀😀😀😀😀😀",
            "0044-03-15 é:20:30 BC",
            "0044-03-15 10:20:30.12345é BC",
            "é044-03-15 BC",
            "\u{0}",
            "NaN\u{0}",
            "{é}",
            "[é",
        ];
        let kinds = [
            Base::Date,
            Base::Timestamp,
            Base::TimestampTz,
            Base::TimeTz,
            Base::Decimal,
            Base::Uuid,
            Base::Array,
        ];
        for base in kinds {
            let mut ty = SqlType::of(base);
            if base == Base::Array {
                ty.element = Some(Box::new(SqlType::of(Base::Int32)));
            }
            let c = pg(ty, DriverKind::Postgres);
            for text in texts {
                let result = row_of(std::slice::from_ref(&c), &[Value::Text(text.into())]);
                if let Ok(row) = result {
                    // None of these is a closed spelling; only a braced text passes as an
                    // array literal (the server checks the elements).
                    assert!(
                        base == Base::Array && text == "{é}",
                        "{base:?} accepted {text:?}: {row:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn unknown_values_are_refused_whatever_the_column() {
        let unknown = Value::unknown("timestamp(9)", "2026-01-01 00:00:00.123456789");
        for base in [Base::Timestamp, Base::Text, Base::Int32, Base::Json] {
            let c = pg(SqlType::of(base), DriverKind::Trino);
            assert!(row_of(&[c], &[unknown.clone()]).is_err(), "{base:?}");
        }
    }
}
