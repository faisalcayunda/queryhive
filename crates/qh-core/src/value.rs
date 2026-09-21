//! The canonical value a driver hands upward, and its text rendering.
//!
//! [`Value::render_text`] is the successor to the Python engine's `to_text`
//! (`exporter/writers.py:38`), and it exists because the golden snapshots in
//! `tests/golden/` pin what that function produced. Every rule below is either a
//! deliberate match or a deliberate, documented difference — see
//! `docs/golden-deltas.md` for the two differences that are already known.
//!
//! The rules `to_text` encoded, all preserved here:
//!
//! | Input | Text |
//! |---|---|
//! | `NULL` | `None` — the writer decides how a NULL looks in its own format |
//! | boolean | `true` / `false`, never `1` / `0` |
//! | integer, float, decimal | the number, in full |
//! | datetime | `2026-01-31 12:00:00.123456+07:00` — a space, not a `T` |
//! | date, time | ISO-8601 |
//! | bytes | lowercase hex, no `0x` |
//! | text | itself |
//! | anything else | its JSON form |

use std::fmt::Write as _;

/// An interval, kept in the three parts servers actually report.
///
/// Months, days and microseconds are not interchangeable — a month is not a
/// fixed number of days and a day is not a fixed number of microseconds once
/// daylight saving is involved — so they are never collapsed into one number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IntervalValue {
    pub months: i32,
    pub days: i32,
    pub micros: i64,
}

/// A value from any supported server, in one shape.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// Signed 64-bit: Postgres `bigint`, every SQL integer type below it.
    Int(i64),
    /// MySQL's `UNSIGNED BIGINT` reaches 18_446_744_073_709_551_615, which does
    /// not fit in an `i64`. Kept separate rather than wrapped or widened to a
    /// float, because either would silently change the number.
    UInt(u64),
    Float(f64),
    /// An exact decimal. `unscaled / 10^scale`, so `NUMERIC(38,10)` keeps all 38
    /// digits: no `f64` is involved at any point on this path.
    Decimal {
        unscaled: i128,
        scale: u8,
    },
    Text(Box<str>),
    /// Raw bytes: `bytea`, `BLOB`, `VARBINARY`. Never forced through UTF-8 —
    /// doing so is how a NUL or an invalid sequence silently becomes `?`.
    Bytes(Vec<u8>),
    /// A point in time.
    ///
    /// `micros` is microseconds since the Unix epoch. `offset_secs` is the
    /// offset the *server* reported, or `None` for a timestamp with no zone
    /// (`TIMESTAMP`, MySQL `DATETIME`). It is kept as reported rather than
    /// normalised to UTC, because normalising is what loses the `+07:00` that
    /// the user asked the database for.
    Timestamp {
        micros: i64,
        offset_secs: Option<i32>,
    },
    /// Days since the Unix epoch.
    Date {
        days: i32,
    },
    /// Microseconds since midnight.
    Time {
        micros: i64,
    },
    Interval(IntervalValue),
    /// JSON text, kept exactly as the server sent it.
    ///
    /// Not re-serialised: Postgres `jsonb` does not promise key order, but
    /// `json` does, and re-encoding would throw away the difference the user
    /// can see.
    Json(Box<str>),
    Array(Vec<Value>),
    /// A Trino `ROW`.
    Row(Vec<Value>),
    /// A Trino `MAP`. Held as pairs, in the server's order, because keys are not
    /// necessarily unique once they are strings and a map would silently drop a
    /// duplicate.
    Map(Vec<(Value, Value)>),
    /// A type this crate does not model yet.
    ///
    /// This is a normal outcome, not a defect: geometry, `inet`, and vendor types
    /// arrive here. Carrying both a text and a raw form means the grid can show
    /// the user something useful and an export can still write the exact bytes.
    Unknown {
        type_name: Box<str>,
        text: Option<Box<str>>,
        raw: Option<Vec<u8>>,
    },
}

impl Value {
    /// Build an [`Value::Unknown`] from a type name and a text rendering.
    pub fn unknown(type_name: impl Into<Box<str>>, text: impl Into<Box<str>>) -> Self {
        Value::Unknown {
            type_name: type_name.into(),
            text: Some(text.into()),
            raw: None,
        }
    }

    /// Build an [`Value::Unknown`] from a type name and raw bytes.
    ///
    /// Used when the bytes are not valid UTF-8, so there is no text form to keep.
    pub fn unknown_bytes(type_name: impl Into<Box<str>>, raw: Vec<u8>) -> Self {
        Value::Unknown {
            type_name: type_name.into(),
            text: None,
            raw: Some(raw),
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// The canonical text form, or `None` for a NULL.
    ///
    /// `None` rather than the string `"NULL"` because the four characters are a
    /// legitimate string value, and a writer for CSV, SQL or JSON each needs to
    /// tell those two apart. The Python engine made the same call.
    pub fn render_text(&self) -> Option<String> {
        match self {
            Value::Null => None,
            Value::Bool(true) => Some("true".to_owned()),
            Value::Bool(false) => Some("false".to_owned()),
            Value::Int(value) => Some(value.to_string()),
            Value::UInt(value) => Some(value.to_string()),
            Value::Float(value) => Some(format_float(*value)),
            Value::Decimal { unscaled, scale } => Some(format_decimal(*unscaled, *scale)),
            Value::Text(text) => Some(text.to_string()),
            Value::Bytes(bytes) => Some(hex_encode(bytes)),
            Value::Timestamp {
                micros,
                offset_secs,
            } => Some(format_timestamp(*micros, *offset_secs)),
            Value::Date { days } => Some(format_date(*days)),
            Value::Time { micros } => Some(format_time(*micros)),
            Value::Interval(interval) => Some(format_interval(*interval)),
            Value::Json(text) => Some(text.to_string()),
            Value::Array(items) => Some(format_sequence('[', ']', items, None)),
            Value::Row(fields) => Some(format_sequence('[', ']', fields, None)),
            Value::Map(pairs) => Some(format_map(pairs)),
            Value::Unknown { text, raw, .. } => match (text, raw) {
                (Some(text), _) => Some(text.to_string()),
                (None, Some(raw)) => Some(hex_encode(raw)),
                // Neither form is present. This cannot be produced by the
                // constructors above; returning an empty string keeps the "never
                // panic" rule true for a value that arrives by another route.
                (None, None) => Some(String::new()),
            },
        }
    }
}

// --------------------------------------------------------------------------- //
// numbers
// --------------------------------------------------------------------------- //

/// Decimal as plain positional text, never scientific.
///
/// `str(Decimal("-0.0000000001"))` in Python is `-1E-10`; this returns
/// `-0.0000000001`. Recorded as delta D-1 in `docs/golden-deltas.md`: a grid cell
/// is data to be read, not a calculator readout, and the numeric value is
/// unchanged.
fn format_decimal(unscaled: i128, scale: u8) -> String {
    let digits = unscaled.unsigned_abs().to_string();
    let scale = usize::from(scale);
    let body = if scale == 0 {
        digits
    } else if digits.len() > scale {
        let split = digits.len() - scale;
        format!("{}.{}", &digits[..split], &digits[split..])
    } else {
        let zeros = "0".repeat(scale - digits.len());
        format!("0.{zeros}{digits}")
    };
    if unscaled < 0 {
        format!("-{body}")
    } else {
        body
    }
}

/// A float the way Python's `str()` presented it, because that is what the
/// golden snapshots recorded.
///
/// The two rules that differ from Rust's own `{}`:
///
/// * an integral value keeps a `.0` (`1.0`, not `1`), so a float column does not
///   change shape depending on the row;
/// * a large or tiny magnitude uses scientific notation, matching Python's
///   switch at 1e16 and 1e-4.
fn format_float(value: f64) -> String {
    if value.is_nan() {
        return "nan".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_negative() {
            "-inf".to_owned()
        } else {
            "inf".to_owned()
        };
    }
    if value == 0.0 {
        // Python keeps the sign of zero, and the snapshot has "-0.0" in it.
        return if value.is_sign_negative() {
            "-0.0".to_owned()
        } else {
            "0.0".to_owned()
        };
    }

    let magnitude = value.abs().log10().floor() as i32;
    if !(-4..16).contains(&magnitude) {
        return format_scientific(value);
    }

    let mut text = format!("{value}");
    if !text.contains('.') {
        text.push_str(".0");
    }
    text
}

/// `1.5e+20`-style text: Rust's `{:e}` with Python's exponent spelling.
fn format_scientific(value: f64) -> String {
    let rusty = format!("{value:e}"); // 1.5e20, 1.5e-7
    let Some((mantissa, exponent)) = rusty.split_once('e') else {
        return rusty;
    };
    let (sign, digits) = match exponent.strip_prefix('-') {
        Some(rest) => ('-', rest),
        None => ('+', exponent),
    };
    // A two-digit minimum, like Python: e+07, not e+7.
    format!("{mantissa}e{sign}{:0>2}", digits)
}

// --------------------------------------------------------------------------- //
// date and time
// --------------------------------------------------------------------------- //

const MICROS_PER_SECOND: i64 = 1_000_000;
const MICROS_PER_DAY: i64 = 86_400 * MICROS_PER_SECOND;

/// `2026-01-31 12:00:00.123456+07:00`, or `2026-01-31 12:00:00` when there is no
/// zone and no sub-second part.
///
/// Both omissions match Python's `datetime.isoformat(sep=" ")`, which is what the
/// snapshot recorded.
fn format_timestamp(micros: i64, offset_secs: Option<i32>) -> String {
    let (wall_micros, suffix) = match offset_secs {
        // The offset is applied to reach the server's wall clock, and then also
        // printed: the instant is unambiguous and the zone is still visible.
        Some(offset) => (micros + i64::from(offset) * MICROS_PER_SECOND, Some(offset)),
        None => (micros, None),
    };

    let days = wall_micros.div_euclid(MICROS_PER_DAY);
    let since_midnight = wall_micros.rem_euclid(MICROS_PER_DAY);

    let (year, month, day) = civil_from_days(days);
    let seconds_of_day = since_midnight / MICROS_PER_SECOND;
    let sub_second = since_midnight % MICROS_PER_SECOND;

    let mut text = format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        seconds_of_day / 3600,
        (seconds_of_day / 60) % 60,
        seconds_of_day % 60,
    );
    if sub_second != 0 {
        let _ = write!(text, ".{sub_second:06}");
    }
    if let Some(offset) = suffix {
        text.push_str(&format_offset(offset));
    }
    text
}

fn format_offset(offset_secs: i32) -> String {
    let sign = if offset_secs < 0 { '-' } else { '+' };
    let magnitude = offset_secs.abs();
    format!(
        "{sign}{:02}:{:02}",
        magnitude / 3600,
        (magnitude % 3600) / 60
    )
}

fn format_date(days: i32) -> String {
    let (year, month, day) = civil_from_days(i64::from(days));
    format!("{year:04}-{month:02}-{day:02}")
}

fn format_time(micros: i64) -> String {
    let since_midnight = micros.rem_euclid(MICROS_PER_DAY);
    let seconds_of_day = since_midnight / MICROS_PER_SECOND;
    let sub_second = since_midnight % MICROS_PER_SECOND;
    let mut text = format!(
        "{:02}:{:02}:{:02}",
        seconds_of_day / 3600,
        (seconds_of_day / 60) % 60,
        seconds_of_day % 60,
    );
    if sub_second != 0 {
        let _ = write!(text, ".{sub_second:06}");
    }
    text
}

/// `3 days, 4:05:06`, matching Python's `str(timedelta)`.
///
/// The inner text is deliberately identical to what the Python engine produced.
/// The only difference is that Python's `json.dumps(..., default=str)` wrapped it
/// in JSON quotes; that is delta D-2 in `docs/golden-deltas.md` and is kept to a
/// single, obvious change.
fn format_interval(interval: IntervalValue) -> String {
    let mut text = String::new();
    if interval.months != 0 {
        let _ = write!(
            text,
            "{} month{}",
            interval.months,
            if interval.months.abs() == 1 { "" } else { "s" }
        );
    }
    if interval.days != 0 {
        if !text.is_empty() {
            text.push_str(", ");
        }
        let _ = write!(
            text,
            "{} day{}",
            interval.days,
            if interval.days.abs() == 1 { "" } else { "s" }
        );
    }

    let negative = interval.micros < 0;
    let magnitude = interval.micros.unsigned_abs();
    let seconds = magnitude / 1_000_000;
    let sub_second = magnitude % 1_000_000;
    // The hour is deliberately not zero-padded: Python's `str(timedelta)` prints
    // `3 days, 4:05:06` and `4:05:06`, never `04:05:06`, and the snapshot holds
    // that wording. Minutes and seconds are padded.
    let time = if sub_second == 0 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            (seconds / 60) % 60,
            seconds % 60
        )
    } else {
        format!(
            "{}:{:02}:{:02}.{sub_second:06}",
            seconds / 3600,
            (seconds / 60) % 60,
            seconds % 60
        )
    };
    if !text.is_empty() {
        text.push_str(", ");
    }
    if negative {
        text.push('-');
    }
    text.push_str(&time);
    text
}

/// Days since the Unix epoch to a civil (proleptic Gregorian) date.
///
/// Howard Hinnant's `civil_from_days`, which is exact for the whole `i64` range
/// and needs no lookup tables — worth having here rather than pulling in a date
/// crate for two conversions.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Shift the epoch to 0000-03-01 so leap days land at the end of the cycle.
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365; // [0, 399]
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100); // [0, 365]
    let month_prime = (5 * day_of_year + 2) / 153; // [0, 11]
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32; // [1, 31]
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32; // [1, 12]
    (if month <= 2 { year + 1 } else { year }, month, day)
}

// --------------------------------------------------------------------------- //
// text and composites
// --------------------------------------------------------------------------- //

/// Lowercase hex, no prefix and no separators: `exporter/writers.py:47` used
/// `bytes.hex()`, and the snapshot has `01ff` for `b"\x01\xff"`.
fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[usize::from(byte >> 4)] as char);
        text.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    text
}

fn format_sequence(open: char, close: char, items: &[Value], _unused: Option<()>) -> String {
    let mut text = String::new();
    text.push(open);
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            // Python's json.dumps default separator is ", ", with the space.
            text.push_str(", ");
        }
        text.push_str(&json_element(item));
    }
    text.push(close);
    text
}

fn format_map(pairs: &[(Value, Value)]) -> String {
    let mut text = String::new();
    text.push('{');
    for (index, (key, value)) in pairs.iter().enumerate() {
        if index > 0 {
            text.push_str(", ");
        }
        // A key is rendered as text even when it is not a string, because JSON
        // object keys are strings and Python's json.dumps coerced them too.
        text.push_str(&json_string(
            key.render_text()
                .unwrap_or_else(|| "null".to_owned())
                .as_str(),
        ));
        text.push_str(": ");
        text.push_str(&json_element(value));
    }
    text.push('}');
    text
}

/// One element of a composite, as JSON: text is quoted and escaped, a NULL is
/// the literal `null`, numbers and booleans are bare.
fn json_element(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(true) => "true".to_owned(),
        Value::Bool(false) => "false".to_owned(),
        Value::Int(number) => number.to_string(),
        Value::UInt(number) => number.to_string(),
        Value::Float(number) => format_float(*number),
        Value::Decimal { unscaled, scale } => format_decimal(*unscaled, *scale),
        Value::Array(items) => format_sequence('[', ']', items, None),
        Value::Row(fields) => format_sequence('[', ']', fields, None),
        Value::Map(pairs) => format_map(pairs),
        other => json_string(other.render_text().unwrap_or_default().as_str()),
    }
}

/// JSON string escaping, `ensure_ascii=False` style: non-ASCII stays literal,
/// only the characters JSON requires are escaped.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            control if (control as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", control as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// What a driver reports when it meets a value it cannot normalise.
///
/// Kept in this crate so every driver says the same thing, and so the "unknown
/// type is not a panic" rule is stated once rather than three times. A value
/// whose bytes are valid UTF-8 keeps them as text; anything else keeps the raw
/// bytes, because a lossy UTF-8 conversion would replace real data with `?`.
pub fn unsupported(type_name: &str, bytes: &[u8]) -> Value {
    match std::str::from_utf8(bytes) {
        Ok(text) => Value::unknown(type_name, text),
        Err(_) => Value::unknown_bytes(type_name, bytes.to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every expectation here is read off a golden snapshot, so a failure means
    /// the Rust renderer and the frozen Python output have drifted apart.
    #[test]
    fn decimals_keep_every_digit_in_positional_form() {
        // tests/golden/preview/type_zoo.ndjson
        assert_eq!(
            Value::Decimal {
                unscaled: 12345678901234567890123456781234567890,
                scale: 10
            }
            .render_text()
            .unwrap(),
            "1234567890123456789012345678.1234567890"
        );
        assert_eq!(
            Value::Decimal {
                unscaled: -1,
                scale: 10
            }
            .render_text()
            .unwrap(),
            "-0.0000000001"
        );
        assert_eq!(
            Value::Decimal {
                unscaled: 0,
                scale: 0
            }
            .render_text()
            .unwrap(),
            "0"
        );
    }

    #[test]
    fn a_decimal_never_goes_through_a_float() {
        // The point of i128 + scale: all 38 digits survive. A round trip through
        // f64 would return 1234567890123456789012345678.1234567 and lose the tail.
        let value = Value::Decimal {
            unscaled: 12345678901234567890123456781234567890,
            scale: 10,
        };
        assert_eq!(
            value.render_text().unwrap(),
            "1234567890123456789012345678.1234567890"
        );
        assert_eq!(
            Value::Decimal {
                unscaled: 12_345_678_901_234_567_890,
                scale: 10
            }
            .render_text()
            .unwrap(),
            "1234567890.1234567890"
        );
    }

    #[test]
    fn a_missing_scale_still_renders_a_leading_zero() {
        assert_eq!(
            Value::Decimal {
                unscaled: 5,
                scale: 3
            }
            .render_text()
            .unwrap(),
            "0.005"
        );
        assert_eq!(
            Value::Decimal {
                unscaled: -5,
                scale: 3
            }
            .render_text()
            .unwrap(),
            "-0.005"
        );
    }

    #[test]
    fn booleans_are_words_and_null_is_not_the_word_null() {
        assert_eq!(Value::Bool(true).render_text().unwrap(), "true");
        assert_eq!(Value::Bool(false).render_text().unwrap(), "false");
        // None, so a writer can tell a NULL from the four characters "NULL".
        assert_eq!(Value::Null.render_text(), None);
        assert_eq!(Value::Text("NULL".into()).render_text().unwrap(), "NULL");
        // An empty string is a value, not a NULL.
        assert_eq!(Value::Text("".into()).render_text().unwrap(), "");
    }

    #[test]
    fn big_unsigned_integers_keep_their_value() {
        assert_eq!(
            Value::UInt(u64::MAX).render_text().unwrap(),
            "18446744073709551615"
        );
        assert_eq!(
            Value::Int(i64::MIN).render_text().unwrap(),
            "-9223372036854775808"
        );
    }

    #[test]
    fn floats_keep_the_shape_python_printed() {
        assert_eq!(Value::Float(1.5).render_text().unwrap(), "1.5");
        // The snapshot holds "-0.0"; Rust's own Display would give "-0".
        assert_eq!(Value::Float(-0.0).render_text().unwrap(), "-0.0");
        // An integral float stays a float, so a column does not change shape.
        assert_eq!(Value::Float(1.0).render_text().unwrap(), "1.0");
        assert_eq!(Value::Float(1e20).render_text().unwrap(), "1e+20");
        assert_eq!(Value::Float(1.5e-7).render_text().unwrap(), "1.5e-07");
    }

    #[test]
    fn bytes_are_lowercase_hex() {
        // The snapshot holds "01ff", not "01FF" and not "0x01ff".
        assert_eq!(
            Value::Bytes(vec![0x01, 0xff]).render_text().unwrap(),
            "01ff"
        );
        assert_eq!(Value::Bytes(vec![0x00]).render_text().unwrap(), "00");
        assert_eq!(Value::Bytes(vec![]).render_text().unwrap(), "");
    }

    #[test]
    fn a_timestamp_keeps_the_offset_the_server_reported() {
        // 2026-01-31T12:00:00.123456+07:00, which is 05:00:00.123456Z.
        let utc_micros = 1_769_835_600_123_456;
        assert_eq!(
            Value::Timestamp {
                micros: utc_micros,
                offset_secs: Some(7 * 3600)
            }
            .render_text()
            .unwrap(),
            "2026-01-31 12:00:00.123456+07:00"
        );
    }

    #[test]
    fn a_timestamp_without_a_zone_does_not_gain_one() {
        let micros = 1_769_835_600_000_000;
        assert_eq!(
            Value::Timestamp {
                micros,
                offset_secs: None
            }
            .render_text()
            .unwrap(),
            "2026-01-31 05:00:00"
        );
        // No sub-second part means no ".000000", exactly as Python printed it.
        assert!(!Value::Timestamp {
            micros,
            offset_secs: None
        }
        .render_text()
        .unwrap()
        .contains('.'));
    }

    #[test]
    fn a_negative_offset_is_spelled_out() {
        let micros = 1_769_835_600_000_000;
        assert_eq!(
            Value::Timestamp {
                micros,
                offset_secs: Some(-5 * 3600)
            }
            .render_text()
            .unwrap(),
            "2026-01-31 00:00:00-05:00"
        );
    }

    #[test]
    fn dates_and_times_are_iso() {
        // 2026-01-31 is 20484 days after the epoch.
        assert_eq!(
            Value::Date { days: 20_484 }.render_text().unwrap(),
            "2026-01-31"
        );
        assert_eq!(Value::Date { days: 0 }.render_text().unwrap(), "1970-01-01");
        assert_eq!(
            Value::Date { days: -1 }.render_text().unwrap(),
            "1969-12-31"
        );
        assert_eq!(
            Value::Time {
                micros: 86_399_999_999
            }
            .render_text()
            .unwrap(),
            "23:59:59.999999"
        );
        assert_eq!(Value::Time { micros: 0 }.render_text().unwrap(), "00:00:00");
    }

    #[test]
    fn leap_days_and_century_rules_are_right() {
        // 2000 was a leap year (divisible by 400), 1900 was not (divisible by
        // 100 but not 400). Getting these wrong shifts every later date.
        assert_eq!(
            Value::Date { days: 11_016 }.render_text().unwrap(),
            "2000-02-29"
        );
        assert_eq!(
            Value::Date { days: -25_509 }.render_text().unwrap(),
            "1900-02-28"
        );
    }

    #[test]
    fn an_interval_matches_pythons_timedelta_wording() {
        // The snapshot has "3 days, 4:05:06" wrapped in JSON quotes; the quotes
        // are delta D-2 in docs/golden-deltas.md, the wording is not.
        assert_eq!(
            Value::Interval(IntervalValue {
                months: 0,
                days: 3,
                micros: 14_706_000_000
            })
            .render_text()
            .unwrap(),
            "3 days, 4:05:06"
        );
        assert_eq!(
            Value::Interval(IntervalValue {
                months: 1,
                days: 0,
                micros: 0
            })
            .render_text()
            .unwrap(),
            "1 month, 0:00:00"
        );
    }

    #[test]
    fn json_text_is_passed_through_unchanged() {
        // Key order is the server's; re-encoding would destroy the difference
        // between json and jsonb, which is information the user chose.
        assert_eq!(
            Value::Json(r#"{"b":1,"a":2}"#.into())
                .render_text()
                .unwrap(),
            r#"{"b":1,"a":2}"#
        );
    }

    #[test]
    fn composites_render_as_json_with_nulls_inside() {
        let array = Value::Array(vec![Value::Int(1), Value::Null, Value::Int(3)]);
        assert_eq!(array.render_text().unwrap(), "[1, null, 3]");

        let row = Value::Row(vec![Value::Int(1), Value::Text("a".into())]);
        assert_eq!(row.render_text().unwrap(), "[1, \"a\"]");

        let map = Value::Map(vec![(Value::Text("k".into()), Value::Int(2))]);
        assert_eq!(map.render_text().unwrap(), "{\"k\": 2}");
    }

    #[test]
    fn text_inside_a_composite_is_escaped() {
        let array = Value::Array(vec![Value::Text("tab\tand\nnewline".into())]);
        assert_eq!(array.render_text().unwrap(), "[\"tab\\tand\\nnewline\"]");

        // Non-ASCII stays literal, like json.dumps(ensure_ascii=False).
        let unicode = Value::Array(vec![Value::Text("é中😀".into())]);
        assert_eq!(unicode.render_text().unwrap(), "[\"é中😀\"]");
    }

    #[test]
    fn an_unknown_type_renders_instead_of_panicking() {
        // Geometry, inet and vendor types land here. The rule is that the user
        // sees something, and no value from a server can crash the app.
        let point = Value::unknown("point", "(1,2)");
        assert_eq!(point.render_text().unwrap(), "(1,2)");
        assert!(!point.render_text().unwrap().is_empty());
    }

    #[test]
    fn bytes_that_are_not_utf8_still_render() {
        // b"\x00\x01\xff" has no valid UTF-8 reading, so it keeps its raw form
        // and is shown as hex rather than being replaced with a lossy character.
        let value = unsupported("bytea", &[0x00, 0x01, 0xff]);
        assert_eq!(value.render_text().unwrap(), "0001ff");
        assert_eq!(
            unsupported("varchar", b"hello").render_text().unwrap(),
            "hello"
        );
    }

    #[test]
    fn text_survives_a_null_byte() {
        // Python kept it; a grid cell holding a NUL is unusual but not a reason
        // to lose data.
        assert_eq!(
            Value::Text("embedded-nul\u{0}".into())
                .render_text()
                .unwrap(),
            "embedded-nul\u{0}"
        );
    }
}
