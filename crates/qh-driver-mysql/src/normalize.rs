//! Turning what MySQL sent into a [`Value`].
//!
//! Two shapes arrive, and both are handled here
//! --------------------------------------------
//! MySQL has two protocols and the value it hands over depends on which one ran:
//!
//! * the **text protocol** (`Conn::query_iter`) sends every value as bytes, so a
//!   `DECIMAL` arrives as `b"1234.5678"` and a `DATETIME` as `b"2026-01-31 12:00:00"`;
//! * the **binary protocol** (prepared statements) sends `Value::Int`, `Value::Date`,
//!   `Value::Time` and so on.
//!
//! This module keys off the **column type** the server reported rather than off the
//! Rust variant, so both paths land in the same place. That matters because the two
//! must not disagree: a `DECIMAL` that parsed to `i128 + scale` over one protocol and
//! something else over the other would be a bug that only shows up in one code path.
//!
//! Why nothing goes through `f64`
//! ------------------------------
//! `DECIMAL(38,10)` has more significant digits than a double holds. Parsing the
//! text form straight into `i128 + scale` keeps all of them, which is the same rule
//! the PostgreSQL driver follows and the reason the internal type exists.
//!
//! The rule across every function: **an unparseable value keeps its text.** Never
//! dropped, never defaulted to zero, never a panic.

use mysql_async::consts::{ColumnFlags, ColumnType};
use mysql_async::Value as MyValue;
use qh_core::Value;

/// The character set MySQL uses for binary columns.
///
/// `TEXT` and `BLOB` are **the same protocol type** (`MYSQL_TYPE_BLOB`), and
/// `CHAR(16) BINARY` is the same protocol type as `CHAR(16)`. The column type
/// alone therefore cannot tell them apart — only the character set can, and 63 is
/// `binary`. Taken from the server rather than from memory:
///
/// ```text
/// SELECT ID, COLLATION_NAME FROM information_schema.COLLATIONS
///   WHERE COLLATION_NAME = 'binary';   -- 63   binary
/// ```
const BINARY_CHARSET: u16 = 63;

/// Whether a column holds bytes rather than text.
pub fn is_binary(column: &mysql_async::Column) -> bool {
    column.character_set() == BINARY_CHARSET
}

/// Convert one value, given the column type the server reported.
///
/// `is_binary` comes from [`is_binary`] and decides the one case the column type
/// cannot: a `BLOB`-typed column is text when its character set says so and bytes
/// when it does not. Getting this wrong turns an empty `TEXT` value into
/// `Bytes([])`, which is a different value in the grid and in every export.
pub fn from_value(column_type: ColumnType, value: Option<&MyValue>, is_binary: bool) -> Value {
    let Some(value) = value else {
        return Value::Null;
    };
    if matches!(value, MyValue::NULL) {
        return Value::Null;
    }

    match column_type {
        // Integers. The text protocol hands these over as bytes, so both forms
        // are accepted rather than assuming the protocol.
        ColumnType::MYSQL_TYPE_TINY
        | ColumnType::MYSQL_TYPE_SHORT
        | ColumnType::MYSQL_TYPE_INT24
        | ColumnType::MYSQL_TYPE_LONG
        | ColumnType::MYSQL_TYPE_LONGLONG
        | ColumnType::MYSQL_TYPE_YEAR => as_integer(value),

        ColumnType::MYSQL_TYPE_FLOAT | ColumnType::MYSQL_TYPE_DOUBLE => as_float(value),

        // MySQL's exact decimal. Never a float.
        ColumnType::MYSQL_TYPE_DECIMAL | ColumnType::MYSQL_TYPE_NEWDECIMAL => as_decimal(value),

        // A BIT column is bytes, not a number: reading it as an integer would
        // reinterpret the bit order.
        ColumnType::MYSQL_TYPE_BIT => as_bytes(value),

        ColumnType::MYSQL_TYPE_DATE | ColumnType::MYSQL_TYPE_NEWDATE => as_date(value),

        // `DATETIME` is a wall clock with no zone; `TIMESTAMP` is converted by the
        // server into the session's time zone and comes back rendered that way.
        // Both are kept as a timestamp carrying the offset the server reported,
        // which for MySQL is the session's. That difference is the server's
        // decision to make, and it stays visible rather than being smoothed over.
        ColumnType::MYSQL_TYPE_DATETIME
        | ColumnType::MYSQL_TYPE_DATETIME2
        | ColumnType::MYSQL_TYPE_TIMESTAMP
        | ColumnType::MYSQL_TYPE_TIMESTAMP2 => as_timestamp(value),

        ColumnType::MYSQL_TYPE_TIME | ColumnType::MYSQL_TYPE_TIME2 => as_time(value),

        // Key order is preserved by not re-encoding: MySQL normalises JSON keys,
        // and what the server returned is what the user should see. Bytes that
        // are not UTF-8 stay bytes rather than becoming a NULL.
        ColumnType::MYSQL_TYPE_JSON => match value {
            MyValue::Bytes(bytes) => match std::str::from_utf8(bytes) {
                Ok(text) => Value::Json(text.into()),
                Err(_) => Value::Bytes(bytes.clone()),
            },
            other => fallback(other),
        },

        // A `BLOB`-typed column is a `TEXT` column unless the charset says
        // binary: MySQL reports both as the same protocol type, so this is where
        // the two are told apart. Binary keeps its bytes — forcing them through
        // UTF-8 is how a NUL or an invalid sequence becomes a replacement
        // character and the real bytes become unrecoverable.
        ColumnType::MYSQL_TYPE_TINY_BLOB
        | ColumnType::MYSQL_TYPE_MEDIUM_BLOB
        | ColumnType::MYSQL_TYPE_LONG_BLOB
        | ColumnType::MYSQL_TYPE_BLOB => text_or_bytes(value, is_binary),

        // Geometry returns WKB, which is never text.
        ColumnType::MYSQL_TYPE_GEOMETRY | ColumnType::MYSQL_TYPE_VECTOR => as_bytes_value(value),

        // ENUM and SET come back as their **labels**, not their ordinals — which
        // is the representation a user recognises and the one the grid should show.
        ColumnType::MYSQL_TYPE_ENUM
        | ColumnType::MYSQL_TYPE_SET
        | ColumnType::MYSQL_TYPE_STRING
        | ColumnType::MYSQL_TYPE_VAR_STRING
        | ColumnType::MYSQL_TYPE_VARCHAR => text_or_bytes(value, is_binary),

        // Anything this driver has not been taught about. Reported as text when
        // the bytes are UTF-8, as bytes otherwise, and never as a panic.
        _ => text_or_bytes(value, is_binary),
    }
}

/// The server's own type name, for the grid's type chip.
///
/// Kept as the MySQL spelling rather than a friendly label: `decimal(38,10)` says
/// more to someone debugging a query than `Decimal` does, and the Python engine
/// showed the server's word too.
///
/// The type code alone cannot name every column: an `ENUM`, a `SET`, a `CHAR` and a
/// `BINARY` all arrive as `MYSQL_TYPE_STRING` (254), so [`flags_name`] is asked
/// first and the code is only the fallback.
pub fn type_name(column: &mysql_async::Column) -> String {
    if let Some(name) = flags_name(column.column_type(), column.flags()) {
        return name.to_owned();
    }
    let base = match column.column_type() {
        ColumnType::MYSQL_TYPE_TINY => "tinyint",
        ColumnType::MYSQL_TYPE_SHORT => "smallint",
        ColumnType::MYSQL_TYPE_INT24 => "mediumint",
        ColumnType::MYSQL_TYPE_LONG => "int",
        ColumnType::MYSQL_TYPE_LONGLONG => "bigint",
        ColumnType::MYSQL_TYPE_FLOAT => "float",
        ColumnType::MYSQL_TYPE_DOUBLE => "double",
        ColumnType::MYSQL_TYPE_DECIMAL | ColumnType::MYSQL_TYPE_NEWDECIMAL => "decimal",
        ColumnType::MYSQL_TYPE_DATE | ColumnType::MYSQL_TYPE_NEWDATE => "date",
        ColumnType::MYSQL_TYPE_DATETIME | ColumnType::MYSQL_TYPE_DATETIME2 => "datetime",
        ColumnType::MYSQL_TYPE_TIMESTAMP | ColumnType::MYSQL_TYPE_TIMESTAMP2 => "timestamp",
        ColumnType::MYSQL_TYPE_TIME | ColumnType::MYSQL_TYPE_TIME2 => "time",
        ColumnType::MYSQL_TYPE_YEAR => "year",
        ColumnType::MYSQL_TYPE_BIT => "bit",
        ColumnType::MYSQL_TYPE_JSON => "json",
        ColumnType::MYSQL_TYPE_ENUM => "enum",
        ColumnType::MYSQL_TYPE_SET => "set",
        ColumnType::MYSQL_TYPE_TINY_BLOB => "tinyblob",
        ColumnType::MYSQL_TYPE_MEDIUM_BLOB => "mediumblob",
        ColumnType::MYSQL_TYPE_LONG_BLOB => "longblob",
        ColumnType::MYSQL_TYPE_BLOB => "blob",
        ColumnType::MYSQL_TYPE_GEOMETRY => "geometry",
        ColumnType::MYSQL_TYPE_VECTOR => "vector",
        ColumnType::MYSQL_TYPE_VARCHAR | ColumnType::MYSQL_TYPE_VAR_STRING => "varchar",
        ColumnType::MYSQL_TYPE_STRING => "char",
        _ => "unknown",
    };
    base.to_owned()
}

/// The names the column's **flags** carry, which its type code cannot.
///
/// `ENUM` and `SET` are not their own protocol types. MySQL reports both as
/// `MYSQL_TYPE_STRING` — 254, the same code as `CHAR(36)` and `BINARY(16)` — so a
/// `SELECT * FROM type_zoo` describes three columns identically and any mapping
/// from the code alone has to call an enum a `char`. The spelling survives in the
/// flags the server sets on the column instead, and this is where the driver reads
/// it. Measured against MySQL 8.4.11 rather than taken from the docs:
///
/// ```text
/// SELECT * FROM type_zoo
///   a_enum        enum('sad','ok','happy')  254  flags 256        ENUM_FLAG
///   a_char_uuid   char(36)                  254  flags 0
///   a_binary_uuid binary(16)                254  flags 128        BINARY_FLAG
/// ```
///
/// The three lines are what `the_flags_tell_an_enum_from_a_char_though_the_type_code_is_the_same`
/// reads back off the container, bit for bit.
///
/// Only a `MYSQL_TYPE_STRING` column is asked about, so an extension outside MySQL
/// that happens to set the same bit is not renamed by accident.
fn flags_name(column_type: ColumnType, flags: ColumnFlags) -> Option<&'static str> {
    if column_type != ColumnType::MYSQL_TYPE_STRING {
        return None;
    }
    if flags.contains(ColumnFlags::ENUM_FLAG) {
        return Some("enum");
    }
    if flags.contains(ColumnFlags::SET_FLAG) {
        return Some("set");
    }
    None
}

// --------------------------------------------------------------------------- //
// helpers
// --------------------------------------------------------------------------- //

fn as_integer(value: &MyValue) -> Value {
    match value {
        MyValue::Int(number) => Value::Int(*number),
        MyValue::UInt(number) => Value::UInt(*number),
        // A `BIGINT UNSIGNED` above i64::MAX only fits in u64, and the text
        // protocol sends it as digits rather than a variant.
        MyValue::Bytes(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => match text.parse::<i64>() {
                Ok(number) => Value::Int(number),
                Err(_) => match text.parse::<u64>() {
                    Ok(number) => Value::UInt(number),
                    Err(_) => Value::Text(text.into()),
                },
            },
            Err(_) => Value::Bytes(bytes.clone()),
        },
        other => fallback(other),
    }
}

fn as_float(value: &MyValue) -> Value {
    match value {
        MyValue::Float(number) => Value::Float(f64::from(*number)),
        MyValue::Double(number) => Value::Float(*number),
        MyValue::Int(number) => Value::Float(*number as f64),
        MyValue::UInt(number) => Value::Float(*number as f64),
        MyValue::Bytes(bytes) => std::str::from_utf8(bytes).map_or_else(
            |_| Value::Bytes(bytes.clone()),
            |text| match text.parse::<f64>() {
                Ok(number) => Value::Float(number),
                Err(_) => Value::Text(text.into()),
            },
        ),
        other => fallback(other),
    }
}

fn as_decimal(value: &MyValue) -> Value {
    match value {
        MyValue::Bytes(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => parse_decimal(text).unwrap_or_else(|| Value::Text(text.into())),
            Err(_) => Value::Bytes(bytes.clone()),
        },
        // The binary protocol never sends a decimal as a float — it sends the
        // bytes — but an integer column read through a decimal view would land
        // here, and an integer is an exact decimal with scale 0.
        MyValue::Int(number) => Value::Decimal {
            unscaled: i128::from(*number),
            scale: 0,
        },
        MyValue::UInt(number) => Value::Decimal {
            unscaled: i128::from(*number),
            scale: 0,
        },
        other => fallback(other),
    }
}

/// `1234.5678` into an exact decimal. No `f64` on this path.
fn parse_decimal(text: &str) -> Option<Value> {
    let text = text.trim();
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (mantissa, exponent) = match digits.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().ok()?),
        None => (digits, 0),
    };
    let (whole, fraction) = match mantissa.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (mantissa, ""),
    };
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    if !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }

    let mut scale = fraction.len() as i32 - exponent;
    let mut combined = String::with_capacity(whole.len() + fraction.len());
    combined.push_str(whole);
    combined.push_str(fraction);
    if scale < 0 {
        let zeros = (-scale) as usize;
        if zeros > 64 {
            return None;
        }
        combined.push_str(&"0".repeat(zeros));
        scale = 0;
    }
    if scale > 38 {
        return None;
    }

    let unscaled = combined.parse::<i128>().ok()?;
    Some(Value::Decimal {
        unscaled: if negative { -unscaled } else { unscaled },
        scale: scale as u8,
    })
}

fn as_date(value: &MyValue) -> Value {
    match value {
        MyValue::Date(year, month, day, ..) => date_from_parts(*year, *month, *day)
            .unwrap_or_else(|| Value::Text(format!("{year:04}-{month:02}-{day:02}").into())),
        MyValue::Bytes(bytes) => std::str::from_utf8(bytes).map_or_else(
            |_| Value::Bytes(bytes.clone()),
            |text| match parse_date_days(text) {
                Some(days) => Value::Date { days },
                None => Value::Text(text.into()),
            },
        ),
        other => fallback(other),
    }
}

fn date_from_parts(year: u16, month: u8, day: u8) -> Option<Value> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(i64::from(year), u32::from(month), u32::from(day));
    i32::try_from(days).ok().map(|days| Value::Date { days })
}

/// A timestamp, keeping the offset the server reported.
///
/// MySQL's text form carries no offset at all: the server has already applied the
/// session's time zone. So an offset is derived from the session and the columns
/// keep the wall clock the server printed — the same reasoning as delta D-3 for
/// PostgreSQL, and the reason the offset is a parameter rather than assumed.
fn as_timestamp(value: &MyValue) -> Value {
    match value {
        MyValue::Date(year, month, day, hour, minute, second, sub_micros) => {
            match timestamp_micros(i64::from(*year), *month, *day, *hour, *minute, *second) {
                Some(base) => Value::Timestamp {
                    micros: base + i64::from(*sub_micros),
                    offset_secs: None,
                },
                None => Value::Text(
                    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
                        .into(),
                ),
            }
        }
        MyValue::Bytes(bytes) => std::str::from_utf8(bytes).map_or_else(
            |_| Value::Bytes(bytes.clone()),
            |text| match parse_timestamp_naive(text) {
                Some(micros) => Value::Timestamp {
                    micros,
                    offset_secs: None,
                },
                None => Value::Text(text.into()),
            },
        ),
        other => fallback(other),
    }
}

fn as_time(value: &MyValue) -> Value {
    match value {
        MyValue::Time(negative, days, hours, minutes, seconds, micros) => {
            let total = (i64::from(*days) * 86_400
                + i64::from(*hours) * 3600
                + i64::from(*minutes) * 60
                + i64::from(*seconds))
                * 1_000_000
                + i64::from(*micros);
            let total = if *negative { -total } else { total };
            // MySQL's TIME is a duration that can exceed 24 hours or be negative,
            // unlike a time of day. A value that fits in a day is a time; anything
            // else is an interval, and calling it a time of day would be a lie.
            if (0..86_400_000_000).contains(&total) {
                Value::Time { micros: total }
            } else {
                Value::Interval(qh_core::IntervalValue {
                    months: 0,
                    days: 0,
                    micros: total,
                })
            }
        }
        MyValue::Bytes(bytes) => std::str::from_utf8(bytes).map_or_else(
            |_| Value::Bytes(bytes.clone()),
            |text| match parse_time_micros(text) {
                Some(total) if (0..86_400_000_000).contains(&total) => {
                    Value::Time { micros: total }
                }
                Some(total) => Value::Interval(qh_core::IntervalValue {
                    months: 0,
                    days: 0,
                    micros: total,
                }),
                None => Value::Text(text.into()),
            },
        ),
        other => fallback(other),
    }
}

fn as_bytes(value: &MyValue) -> Value {
    match value {
        MyValue::Bytes(bytes) => Value::Bytes(bytes.clone()),
        MyValue::Int(number) => Value::Bytes(number.to_be_bytes().to_vec()),
        MyValue::UInt(number) => Value::Bytes(number.to_be_bytes().to_vec()),
        other => fallback(other),
    }
}

fn as_bytes_value(value: &MyValue) -> Value {
    match value {
        MyValue::Bytes(bytes) => Value::Bytes(bytes.clone()),
        other => fallback(other),
    }
}

/// Text when the column is not binary and the bytes are UTF-8; raw bytes
/// otherwise.
///
/// Never lossy: a replacement character would replace real data with something the
/// user cannot recover, so a byte sequence that is not valid UTF-8 stays bytes
/// even in a text column.
fn text_or_bytes(value: &MyValue, is_binary: bool) -> Value {
    match value {
        MyValue::Bytes(bytes) => {
            if is_binary {
                return Value::Bytes(bytes.clone());
            }
            match std::str::from_utf8(bytes) {
                Ok(text) => Value::Text(text.into()),
                Err(_) => Value::Bytes(bytes.clone()),
            }
        }
        other => fallback(other),
    }
}

/// A value in a shape this column type did not expect.
///
/// Rather than guess, it is rendered as text if it is text and as bytes if it is
/// not. A `SET` column read as bytes, or a driver change that shifts a variant,
/// lands here and the user still sees their data.
fn fallback(value: &MyValue) -> Value {
    match value {
        MyValue::NULL => Value::Null,
        MyValue::Bytes(bytes) => text_or_bytes(&MyValue::Bytes(bytes.clone()), false),
        MyValue::Int(number) => Value::Int(*number),
        MyValue::UInt(number) => Value::UInt(*number),
        MyValue::Float(number) => Value::Float(f64::from(*number)),
        MyValue::Double(number) => Value::Float(*number),
        MyValue::Date(..) | MyValue::Time(..) => match value.render_fallback() {
            Some(text) => Value::Text(text.into()),
            None => Value::Text(format!("{value:?}").into()),
        },
    }
}

/// A small extension so the fallback path can render a date/time without
/// duplicating the formatting above.
trait RenderFallback {
    fn render_fallback(&self) -> Option<String>;
}

impl RenderFallback for MyValue {
    fn render_fallback(&self) -> Option<String> {
        match self {
            MyValue::Date(year, month, day, hour, minute, second, _) => Some(format!(
                "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
            )),
            MyValue::Time(negative, days, hours, minutes, seconds, _) => Some(format!(
                "{}{:02}:{:02}:{:02}",
                if *negative { "-" } else { "" },
                *days * 24 + u32::from(*hours),
                minutes,
                seconds
            )),
            _ => None,
        }
    }
}

fn parse_date_days(text: &str) -> Option<i32> {
    let (year, month, day) = parse_ymd(text)?;
    i32::try_from(days_from_civil(year, u32::from(month), u32::from(day))).ok()
}

/// `YYYY-MM-DD`. The parts stay `u8` because that is what the value variants and
/// the timestamp builder take; widening them here only to narrow them again was
/// what the first version got wrong.
fn parse_ymd(text: &str) -> Option<(i64, u8, u8)> {
    let mut parts = text.trim().splitn(3, '-');
    let year = parts.next()?.parse::<i64>().ok()?;
    let month = parts.next()?.parse::<u8>().ok()?;
    let day = parts.next()?.parse::<u8>().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

fn timestamp_micros(
    year: i64,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
) -> Option<i64> {
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year, u32::from(month), u32::from(day));
    let seconds = i64::from(hour) * 3600 + i64::from(minute) * 60 + i64::from(second);
    Some(days * 86_400 * 1_000_000 + seconds * 1_000_000)
}

fn parse_timestamp_naive(text: &str) -> Option<i64> {
    let text = text.trim();
    // MySQL prints a zero date as `0000-00-00`, which is not a real date and must
    // not be silently turned into 1970.
    if text.starts_with("0000-00-00") {
        return None;
    }
    let (date_part, time_part) = text.split_once([' ', 'T'])?;
    let (year, month, day) = parse_ymd(date_part)?;
    let mut parts = time_part.split(':');
    let hour = parts.next()?.parse::<u8>().ok()?;
    let minute = parts.next()?.parse::<u8>().ok()?;
    let seconds_field = parts.next()?;
    let (second, micros) = match seconds_field.split_once('.') {
        Some((second, fraction)) => (second.parse::<u8>().ok()?, parse_fraction(fraction)?),
        None => (seconds_field.parse::<u8>().ok()?, 0),
    };
    Some(timestamp_micros(year, month, day, hour, minute, second)? + micros)
}

fn parse_fraction(fraction: &str) -> Option<i64> {
    if fraction.is_empty() || fraction.len() > 6 {
        return None;
    }
    if !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value = fraction.parse::<i64>().ok()?;
    Some(value * 10i64.pow((6 - fraction.len()) as u32))
}

/// `HH:MM:SS[.ffffff]`, or the `HHH:MM:SS` a TIME longer than a day prints as.
fn parse_time_micros(text: &str) -> Option<i64> {
    let text = text.trim();
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let mut parts = rest.split(':');
    let hours = parts.next()?.parse::<i64>().ok()?;
    let minutes = parts.next()?.parse::<i64>().ok()?;
    let seconds_field = parts.next()?;
    let (seconds, micros) = match seconds_field.split_once('.') {
        Some((seconds, fraction)) => (seconds.parse::<i64>().ok()?, parse_fraction(fraction)?),
        None => (seconds_field.parse::<i64>().ok()?, 0),
    };
    if minutes > 59 || seconds > 60 {
        return None;
    }
    let total = (hours * 3600 + minutes * 60 + seconds) * 1_000_000 + micros;
    Some(if negative { -total } else { total })
}

/// Days since the Unix epoch. Howard Hinnant's `days_from_civil`, matching the
/// conversion in `qh-core` and in the PostgreSQL driver so all three agree.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_prime = i64::from(if month > 2 { month - 3 } else { month + 9 });
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ordinary case: a column whose character set is not binary.
    fn plain(column_type: ColumnType, value: Option<&MyValue>) -> Value {
        from_value(column_type, value, false)
    }

    /// A column the server reported as binary.
    fn raw(column_type: ColumnType, value: &MyValue) -> Value {
        from_value(column_type, Some(value), true)
    }

    fn bytes(text: &str) -> MyValue {
        MyValue::Bytes(text.as_bytes().to_vec())
    }

    #[test]
    fn a_null_is_a_null_whatever_the_column_type() {
        assert_eq!(plain(ColumnType::MYSQL_TYPE_NEWDECIMAL, None), Value::Null);
        assert_eq!(
            plain(ColumnType::MYSQL_TYPE_BLOB, Some(&MyValue::NULL)),
            Value::Null
        );
    }

    #[test]
    fn a_decimal_keeps_every_digit_and_never_becomes_a_float() {
        // The same value the PostgreSQL driver is tested with, so the two
        // databases cannot drift apart on it.
        assert_eq!(
            plain(
                ColumnType::MYSQL_TYPE_NEWDECIMAL,
                Some(&bytes("1234567890123456789012345678.1234567890"))
            ),
            Value::Decimal {
                unscaled: 12345678901234567890123456781234567890,
                scale: 10
            }
        );
        assert_eq!(
            plain(
                ColumnType::MYSQL_TYPE_NEWDECIMAL,
                Some(&bytes("-0.0000000001"))
            ),
            Value::Decimal {
                unscaled: -1,
                scale: 10
            }
        );
        assert_eq!(
            plain(ColumnType::MYSQL_TYPE_DECIMAL, Some(&bytes("42"))),
            Value::Decimal {
                unscaled: 42,
                scale: 0
            }
        );
    }

    #[test]
    fn integers_read_the_same_from_either_protocol() {
        // Text protocol.
        assert_eq!(
            plain(
                ColumnType::MYSQL_TYPE_LONGLONG,
                Some(&bytes("-9223372036854775808"))
            ),
            Value::Int(i64::MIN)
        );
        // Binary protocol.
        assert_eq!(
            plain(ColumnType::MYSQL_TYPE_LONGLONG, Some(&MyValue::Int(-5))),
            Value::Int(-5)
        );
        // `BIGINT UNSIGNED` above i64::MAX only fits in u64.
        assert_eq!(
            plain(
                ColumnType::MYSQL_TYPE_LONGLONG,
                Some(&bytes("18446744073709551615"))
            ),
            Value::UInt(u64::MAX)
        );
        assert_eq!(
            plain(
                ColumnType::MYSQL_TYPE_LONGLONG,
                Some(&MyValue::UInt(u64::MAX))
            ),
            Value::UInt(u64::MAX)
        );
    }

    #[test]
    fn a_text_column_is_text_and_a_blob_column_is_bytes_though_both_arrive_as_blob() {
        // This is the case the column type cannot answer. `empty_text` and
        // `raw_bytes` are both `MYSQL_TYPE_BLOB` on the wire; only the character
        // set separates them, and getting it wrong turns an empty string into
        // `Bytes([])` — a different value in the grid and in every export.
        let empty = MyValue::Bytes(Vec::new());
        assert_eq!(
            plain(ColumnType::MYSQL_TYPE_BLOB, Some(&empty)),
            Value::Text(String::new().into()),
            "a TEXT column reading empty is an empty string"
        );
        assert_eq!(
            raw(ColumnType::MYSQL_TYPE_BLOB, &empty),
            Value::Bytes(Vec::new()),
            "a BLOB column reading empty is empty bytes"
        );

        let word = MyValue::Bytes(b"hello".to_vec());
        assert_eq!(
            plain(ColumnType::MYSQL_TYPE_BLOB, Some(&word)),
            Value::Text("hello".into())
        );
        assert_eq!(
            raw(ColumnType::MYSQL_TYPE_BLOB, &word),
            Value::Bytes(b"hello".to_vec())
        );

        // `CHAR(16) BINARY` is the same protocol type as `CHAR(16)`.
        let uuid = MyValue::Bytes(vec![
            0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44,
            0x00, 0x00,
        ]);
        assert_eq!(
            raw(ColumnType::MYSQL_TYPE_STRING, &uuid),
            Value::Bytes(uuid_bytes())
        );
        // The same bytes in a non-binary column are not valid UTF-8, so they stay
        // bytes rather than becoming replacement characters.
        assert!(matches!(
            plain(ColumnType::MYSQL_TYPE_STRING, Some(&uuid)),
            Value::Bytes(_)
        ));
    }

    fn uuid_bytes() -> Vec<u8> {
        vec![
            0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44,
            0x00, 0x00,
        ]
    }

    #[test]
    fn a_blob_keeps_its_bytes_including_a_nul() {
        let raw = MyValue::Bytes(vec![0x00, 0x01, 0xff]);
        assert_eq!(
            plain(ColumnType::MYSQL_TYPE_BLOB, Some(&raw)),
            Value::Bytes(vec![0x00, 0x01, 0xff])
        );
    }

    #[test]
    fn a_bit_column_is_bytes_not_a_number() {
        // Reading a BIT as an integer would reinterpret the bit order.
        let bits = MyValue::Bytes(vec![0b0000_0101]);
        assert_eq!(
            plain(ColumnType::MYSQL_TYPE_BIT, Some(&bits)),
            Value::Bytes(vec![5])
        );
    }

    #[test]
    fn an_enum_comes_back_as_its_label_not_its_ordinal() {
        // The label is what the user recognises and what the Python engine showed.
        assert_eq!(
            plain(ColumnType::MYSQL_TYPE_ENUM, Some(&bytes("ok"))),
            Value::Text("ok".into())
        );
        assert_eq!(
            plain(ColumnType::MYSQL_TYPE_SET, Some(&bytes("a,b"))),
            Value::Text("a,b".into())
        );
    }

    /// The type chip's name for the three columns MySQL describes identically.
    ///
    /// `ENUM`, `CHAR(36)` and `BINARY(16)` all arrive as `MYSQL_TYPE_STRING`, so the
    /// type code names all three `char` and a user cannot tell an enum from a char.
    /// The flag is the only thing that separates them; the values are the ones the
    /// dev server puts on `type_zoo`, read in
    /// `the_flags_tell_an_enum_from_a_char_though_the_type_code_is_the_same`.
    #[test]
    fn an_enum_is_named_by_its_flag_not_by_the_code_it_shares_with_char() {
        assert_eq!(
            flags_name(ColumnType::MYSQL_TYPE_STRING, ColumnFlags::ENUM_FLAG),
            Some("enum")
        );
        assert_eq!(
            flags_name(
                ColumnType::MYSQL_TYPE_STRING,
                ColumnFlags::BLOB_FLAG | ColumnFlags::BINARY_FLAG
            ),
            None,
            "a binary column is still a char, not an enum"
        );
        assert_eq!(
            flags_name(ColumnType::MYSQL_TYPE_STRING, ColumnFlags::SET_FLAG),
            Some("set"),
            "a SET is spelled by its flag too, since it is 254 as well"
        );
        assert_eq!(
            flags_name(ColumnType::MYSQL_TYPE_STRING, ColumnFlags::empty()),
            None
        );
        // The flag only renames the type code it belongs to.
        assert_eq!(
            flags_name(ColumnType::MYSQL_TYPE_BLOB, ColumnFlags::ENUM_FLAG),
            None
        );
    }

    #[test]
    fn a_date_converts_from_either_protocol() {
        assert_eq!(
            plain(ColumnType::MYSQL_TYPE_DATE, Some(&bytes("2026-01-31"))),
            Value::Date { days: 20_484 }
        );
        assert_eq!(
            plain(
                ColumnType::MYSQL_TYPE_DATE,
                Some(&MyValue::Date(2026, 1, 31, 0, 0, 0, 0))
            ),
            Value::Date { days: 20_484 }
        );
        // And it renders back to what was sent.
        assert_eq!(
            plain(ColumnType::MYSQL_TYPE_DATE, Some(&bytes("2026-01-31")))
                .render_text()
                .unwrap(),
            "2026-01-31"
        );
    }

    #[test]
    fn a_datetime_keeps_the_wall_clock_the_server_printed() {
        assert_eq!(
            plain(
                ColumnType::MYSQL_TYPE_DATETIME,
                Some(&bytes("2026-01-31 12:00:00.123456"))
            ),
            Value::Timestamp {
                micros: 1_769_860_800_123_456,
                offset_secs: None
            }
        );
        assert_eq!(
            plain(
                ColumnType::MYSQL_TYPE_DATETIME,
                Some(&bytes("1970-01-01 00:00:00"))
            ),
            Value::Timestamp {
                micros: 0,
                offset_secs: None
            }
        );
    }

    #[test]
    fn mysqls_zero_date_is_not_silently_seventeen_seventy() {
        // `0000-00-00` is what MySQL stores for "no date" when it is allowed to.
        // Turning it into 1970 would invent a value the user never stored.
        let value = plain(
            ColumnType::MYSQL_TYPE_DATETIME,
            Some(&bytes("0000-00-00 00:00:00")),
        );
        assert_eq!(value, Value::Text("0000-00-00 00:00:00".into()));
        assert_eq!(
            plain(ColumnType::MYSQL_TYPE_DATE, Some(&bytes("0000-00-00"))),
            Value::Text("0000-00-00".into())
        );
    }

    #[test]
    fn a_time_inside_a_day_is_a_time_of_day() {
        assert_eq!(
            plain(ColumnType::MYSQL_TYPE_TIME, Some(&bytes("23:59:59.999999"))),
            Value::Time {
                micros: 86_399_999_999
            }
        );
    }

    /// The time of day, with no JSON quotes around it.
    ///
    /// PyMySQL hands MySQL's `TIME` back as a `timedelta`, which the Python engine's
    /// `to_text` fallback passed through `json.dumps` — so the cell it wrote for
    /// `type_zoo.a_time` was `"23:59:59.999999"`, quotes included as part of the
    /// text (`tests/golden/preview/mysql_type_zoo_live.ndjson`). Decoding the value
    /// is what removes them; this is the same difference `docs/golden-deltas.md`
    /// records as D-2 for an INTERVAL, and it must not come back.
    #[test]
    fn a_time_renders_without_the_json_quotes_the_python_fallback_added() {
        let rendered = plain(ColumnType::MYSQL_TYPE_TIME, Some(&bytes("23:59:59.999999")))
            .render_text()
            .expect("a time has a text form");
        assert_eq!(rendered, "23:59:59.999999");
        assert!(!rendered.contains('"'), "quotes are not part of a time");
    }

    #[test]
    fn a_time_beyond_a_day_is_a_duration_not_a_time_of_day() {
        // MySQL's TIME is a duration: it can exceed 24 hours and be negative.
        // Calling 100 hours a time of day would be a lie.
        let long = plain(ColumnType::MYSQL_TYPE_TIME, Some(&bytes("100:00:00")));
        assert!(
            matches!(long, Value::Interval(_)),
            "100 hours should be a duration, got {long:?}"
        );
        let negative = plain(ColumnType::MYSQL_TYPE_TIME, Some(&bytes("-01:00:00")));
        assert!(matches!(negative, Value::Interval(_)), "got {negative:?}");
    }

    #[test]
    fn json_is_kept_as_text_so_what_the_server_returned_survives() {
        assert_eq!(
            plain(
                ColumnType::MYSQL_TYPE_JSON,
                Some(&bytes(r#"{"a": 2, "b": 1}"#))
            ),
            Value::Json(r#"{"a": 2, "b": 1}"#.into())
        );
    }

    #[test]
    fn text_types_are_text_and_an_empty_string_is_a_value() {
        for column_type in [
            ColumnType::MYSQL_TYPE_VARCHAR,
            ColumnType::MYSQL_TYPE_VAR_STRING,
            ColumnType::MYSQL_TYPE_STRING,
        ] {
            assert_eq!(
                plain(column_type, Some(&bytes("value"))),
                Value::Text("value".into()),
                "{column_type:?}"
            );
            assert_eq!(plain(column_type, Some(&bytes(""))), Value::Text("".into()));
        }
        assert_eq!(
            plain(ColumnType::MYSQL_TYPE_STRING, Some(&bytes("NULL"))),
            Value::Text("NULL".into())
        );
    }

    #[test]
    fn a_geometry_column_keeps_its_bytes_rather_than_being_mangled() {
        // MySQL returns WKB, which is not text. A lossy decode would destroy it.
        let wkb = MyValue::Bytes(vec![0x00, 0x00, 0x00, 0x00, 0x01, 0x40]);
        assert_eq!(
            plain(ColumnType::MYSQL_TYPE_GEOMETRY, Some(&wkb)),
            Value::Bytes(vec![0x00, 0x00, 0x00, 0x00, 0x01, 0x40])
        );
    }

    #[test]
    fn a_malformed_value_keeps_its_text_rather_than_becoming_a_default() {
        for (column_type, text) in [
            (ColumnType::MYSQL_TYPE_NEWDECIMAL, "not a number"),
            (ColumnType::MYSQL_TYPE_LONGLONG, "nope"),
            (ColumnType::MYSQL_TYPE_DATE, "2026-13-45"),
        ] {
            assert_eq!(
                plain(column_type, Some(&bytes(text))),
                Value::Text(text.into()),
                "{column_type:?}"
            );
        }
    }

    #[test]
    fn a_type_this_module_has_not_been_taught_about_is_text_not_a_panic() {
        assert_eq!(
            plain(ColumnType::MYSQL_TYPE_UNKNOWN, Some(&bytes("whatever"))),
            Value::Text("whatever".into())
        );
        // Invalid UTF-8 in an unmodelled column stays bytes.
        let invalid = MyValue::Bytes(vec![0xff, 0xfe]);
        assert_eq!(
            plain(ColumnType::MYSQL_TYPE_UNKNOWN, Some(&invalid)),
            Value::Bytes(vec![0xff, 0xfe])
        );
    }
}
