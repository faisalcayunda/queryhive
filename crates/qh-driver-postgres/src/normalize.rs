//! Turning what PostgreSQL said into a [`Value`].
//!
//! Why the values arrive as text
//! -----------------------------
//! Reading results goes through the simple query protocol, which always returns
//! text. That is a deliberate choice with three payoffs:
//!
//! 1. **No unknown type can fail a query.** The extended protocol decodes each
//!    column into a Rust type, and every type the driver has not been taught
//!    about is a decode failure for the whole row. Text always arrives, so a
//!    geometry or an `inet` becomes [`Value::Text`] and the query still works.
//! 2. **Exact decimals come for free.** `NUMERIC(38,10)` arrives as
//!    `1234567890123456789012345678.1234567890` and parses into `i128 + scale`
//!    with every digit intact. The binary path would need a decimal library or a
//!    lossy `f64`.
//! 3. **The column type still comes from the server.** The driver asks for the
//!    statement's description separately (`Client::prepare`), so each value is
//!    parsed with full knowledge of its column type rather than guessed at.
//!
//! What it costs: this module does the parsing, so this is where a conversion bug
//! would live. That is why it is one module, and why its tests use the values in
//! `tests/golden/preview/type_zoo.ndjson` — what the Python engine produced for
//! the same types.
//!
//! The rule across every function here: **an unparseable value keeps its text.**
//! Never dropped, never defaulted to zero, never a panic. A value that arrives in
//! a shape nobody expected shows the user the server's own rendering, which is
//! what someone debugging a query actually wants.
//!
//! Three renderings that deliberately differ from the Python engine
//! --------------------------------------------------------------
//!
//! `tests/golden/preview/postgres_type_zoo_live.ndjson` is the previous engine's
//! own output for `type_zoo`, and `tests/golden/RECORDED.md` puts its cells beside
//! this one's. Three of those cells differ on purpose, and each was decided here
//! rather than matched:
//!
//! - **`interval` keeps months, days and the clock apart** — `14 months, 3 days,
//!   4:05:06` where the previous engine wrote `"428 days, 4:05:06"`. psycopg handed
//!   that engine a `timedelta`, which cannot hold a month, so it folded
//!   `1 year 2 mons 3 days` into days before the value ever reached the renderer.
//!   A month is not a fixed number of days, so that is a loss rather than a
//!   re-spelling: `428 days` cannot be turned back into what the server sent. This
//!   driver keeps the three parts the server sent separately, and the rendering is
//!   `qh_core::value::format_interval`'s (see its own doc: the month part is an
//!   extension, because Python had no way to express one).
//! - **An array keeps the server's own literal** — `{1,NULL,3}`, with NULL spelled
//!   PostgreSQL's way, where the previous engine wrote psycopg's JSON list
//!   `[1, null, 3]`. Neither spelling loses anything, so the question is what reads
//!   the cell, and nothing parses one as JSON: the app decodes the preview payload
//!   as `[[String?]]` (`app/Sources/TrinoExporter/App.swift:156`), and the export
//!   writers stringify `Value::Json` exactly as they stringify `Value::Text`
//!   (`crates/qh-core/src/render.rs:83` and the fallback arm of `to_json_value` at
//!   `:146`), so both spellings reach a file as one string. Producing the JSON form
//!   means writing a PostgreSQL array-literal parser — nested braces, quoted
//!   elements, escapes, any dimension — to reformat a string nobody parses, so the
//!   server's text is kept.
//! - **`uuid` is bare.** The previous engine's quotes were not part of the value:
//!   `json.dumps(default=str)` wrapped a `uuid.UUID` in JSON quotes, the same
//!   fallback that put quotes around an interval (delta D-2 in
//!   `docs/golden-deltas.md`). A UUID has one text form and it has no quotes in it.

use qh_core::{IntervalValue, Value};

/// Convert one text value, given the server's type name.
///
/// `text` is `None` for a SQL NULL. The type name comes from the column rather
/// than the value, precisely so a NULL can still be reported with its type.
pub fn from_text(type_name: &str, text: Option<&str>) -> Value {
    let Some(text) = text else {
        return Value::Null;
    };

    match base_type(type_name) {
        "bool" => match text {
            "t" | "true" => Value::Bool(true),
            "f" | "false" => Value::Bool(false),
            _ => Value::Text(text.into()),
        },
        "int2" | "int4" | "int8" | "oid" => match text.parse::<i64>() {
            Ok(number) => Value::Int(number),
            Err(_) => Value::Text(text.into()),
        },
        "float4" | "float8" => match text.parse::<f64>() {
            Ok(number) => Value::Float(number),
            Err(_) => match text {
                // PostgreSQL's spelling of the non-finite values.
                "Infinity" => Value::Float(f64::INFINITY),
                "-Infinity" => Value::Float(f64::NEG_INFINITY),
                _ => Value::Text(text.into()),
            },
        },
        "numeric" | "decimal" => parse_decimal(text).unwrap_or_else(|| Value::Text(text.into())),
        "bytea" => parse_bytea(text),
        "date" => {
            parse_days(text).map_or_else(|| Value::Text(text.into()), |days| Value::Date { days })
        }
        "time" => parse_time_micros(text)
            .map_or_else(|| Value::Text(text.into()), |micros| Value::Time { micros }),
        "timestamp" => parse_timestamp(text).map_or_else(
            || Value::Text(text.into()),
            |(micros, _)| Value::Timestamp {
                micros,
                offset_secs: None,
            },
        ),
        "timestamptz" => parse_timestamp(text).map_or_else(
            || Value::Text(text.into()),
            |(micros, offset)| Value::Timestamp {
                micros,
                offset_secs: Some(offset),
            },
        ),
        "interval" => {
            parse_interval(text).map_or_else(|| Value::Text(text.into()), Value::Interval)
        }
        // Key order is preserved by not re-encoding: `json` promises an order and
        // `jsonb` does not, and that difference is the user's to see.
        "json" | "jsonb" => Value::Json(text.into()),
        // A UUID has one text form and no quotes in it. The Python engine's quotes
        // were `json.dumps(default=str)`'s, not the server's — see the module doc.
        "uuid" | "text" | "varchar" | "bpchar" | "char" | "name" | "citext" => {
            Value::Text(text.into())
        }
        // Arrays, geometry, `inet`, ranges, and anything PostgreSQL gains later.
        // Kept as text: it renders correctly in the grid and in every exporter,
        // and the user sees the server's own format rather than a guess at
        // structure. `{1,NULL,3}` is the server's own array output; the Python
        // engine's `[1, null, 3]` was psycopg's decoding, and nothing downstream
        // reads a cell as JSON, so there is nothing to decode it for. A PostgreSQL
        // array-literal parser is a separate task.
        _ => Value::Text(text.into()),
    }
}

/// Strip the modifier PostgreSQL appends: `numeric(38,10)`, `varchar(255)`,
/// `timestamp(6) with time zone`, `character varying(10)`.
fn base_type(type_name: &str) -> &str {
    let trimmed = type_name.trim();
    let without_modifier = match trimmed.find('(') {
        Some(position) => &trimmed[..position],
        None => trimmed,
    };
    without_modifier.trim_end()
}

/// `1234567890123456789012345678.1234567890` into an exact decimal.
///
/// No `f64` anywhere on this path. A `NUMERIC(38,10)` has more significant digits
/// than a double holds, so routing it through one would round it invisibly.
fn parse_decimal(text: &str) -> Option<Value> {
    let text = text.trim();
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };

    // `numeric` can carry an exponent, so it is applied rather than refused:
    // refusing would push a legitimate value into the text fallback.
    let (mantissa, exponent) = match digits.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, exponent.parse::<i32>().ok()?),
        None => (digits, 0),
    };

    let (whole, fraction) = mantle_parts(mantissa)?;
    let mut scale = fraction.len() as i32 - exponent;
    let mut combined = String::with_capacity(whole.len() + fraction.len());
    combined.push_str(whole);
    combined.push_str(fraction);
    if scale < 0 {
        let zeros = (-scale) as usize;
        // A hostile exponent must not be able to ask for a gigabyte of zeros.
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
    let unscaled = if negative { -unscaled } else { unscaled };
    Some(Value::Decimal {
        unscaled,
        scale: scale as u8,
    })
}

fn mantle_parts(mantissa: &str) -> Option<(&str, &str)> {
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
    Some((whole, fraction))
}

/// `\x0001ff` (hex, the default) and `\001\377` (octal, when the server has
/// `bytea_output = escape`).
fn parse_bytea(text: &str) -> Value {
    if let Some(hex) = text.strip_prefix("\\x") {
        if hex.len() % 2 != 0 {
            return Value::Text(text.into());
        }
        let mut bytes = Vec::with_capacity(hex.len() / 2);
        for pair in hex.as_bytes().chunks(2) {
            let (Some(high), Some(low)) = (
                (pair[0] as char).to_digit(16),
                (pair[1] as char).to_digit(16),
            ) else {
                return Value::Text(text.into());
            };
            bytes.push((high * 16 + low) as u8);
        }
        return Value::Bytes(bytes);
    }

    let mut bytes = Vec::with_capacity(text.len());
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\\' {
            let mut buffer = [0u8; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        if characters.peek() == Some(&'\\') {
            characters.next();
            bytes.push(b'\\');
            continue;
        }
        let mut octal = String::new();
        while octal.len() < 3 {
            match characters.peek() {
                Some(digit) if digit.is_digit(8) => {
                    octal.push(*digit);
                    characters.next();
                }
                _ => break,
            }
        }
        match u8::from_str_radix(&octal, 8) {
            Ok(byte) => bytes.push(byte),
            Err(_) => return Value::Text(text.into()),
        }
    }
    Value::Bytes(bytes)
}

fn parse_days(text: &str) -> Option<i32> {
    let (year, month, day) = parse_ymd(text.trim())?;
    Some(days_from_civil(year, month, day) as i32)
}

fn parse_ymd(text: &str) -> Option<(i64, u32, u32)> {
    let mut parts = text.splitn(3, '-');
    let year = parts.next()?.parse::<i64>().ok()?;
    let month = parts.next()?.parse::<u32>().ok()?;
    let day = parts.next()?.parse::<u32>().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

fn parse_time_micros(text: &str) -> Option<i64> {
    let (clock, _offset) = split_offset(text.trim());
    let (hours, minutes, seconds, micros) = parse_clock(clock)?;
    Some(i64::from(hours * 3600 + minutes * 60 + seconds) * 1_000_000 + micros)
}

/// Split a trailing `+07`, `+07:00`, `-05:30` or `+07:00:30` off a value.
///
/// Starts from the last sign and tries to read everything after it as an offset,
/// rather than walking back over every digit. Walking back is the obvious
/// implementation and it is wrong: in `12:00:00+07` the colon and the digits of
/// the clock are all `[0-9:]`, so the scan swallows the whole time and leaves an
/// empty head. The offset is also bounds-checked, so a date's `-31` can never be
/// read as a UTC offset of thirty-one hours.
fn split_offset(text: &str) -> (&str, Option<i32>) {
    if let Some(position) = text.rfind(['+', '-']) {
        let preceded_by_digit = position > 0 && text.as_bytes()[position - 1].is_ascii_digit();
        if preceded_by_digit && position + 1 < text.len() {
            if let Some(offset) = parse_offset_seconds(&text[position..]) {
                return (text[..position].trim_end(), Some(offset));
            }
        }
    }
    (text, None)
}

fn parse_offset_seconds(text: &str) -> Option<i32> {
    let (sign, rest) = match text.split_at_checked(1)? {
        ("+", rest) => (1, rest),
        ("-", rest) => (-1, rest),
        _ => return None,
    };
    let mut parts = rest.split(':');
    let hours = parts.next()?.parse::<i32>().ok()?;
    let minutes = match parts.next() {
        Some(minutes) => minutes.parse::<i32>().ok()?,
        None => 0,
    };
    let seconds = match parts.next() {
        Some(seconds) => {
            let whole = seconds.split_once('.').map_or(seconds, |(whole, _)| whole);
            whole.parse::<i32>().ok()?
        }
        None => 0,
    };
    if parts.next().is_some() {
        return None;
    }
    // The real range is -12:00 to +14:00. Rejecting anything larger is what keeps
    // a date's `-31` from being read as an offset.
    if hours > 14 || minutes > 59 || seconds > 59 {
        return None;
    }
    Some(sign * (hours * 3600 + minutes * 60 + seconds))
}

/// `HH:MM:SS[.ffffff]` to hours, minutes, seconds and microseconds.
fn parse_clock(text: &str) -> Option<(u32, u32, u32, i64)> {
    let mut parts = text.trim().split(':');
    let hours = parts.next()?.trim().parse::<u32>().ok()?;
    let minutes = parts.next()?.trim().parse::<u32>().ok()?;
    let seconds_field = parts.next()?.trim();
    if parts.next().is_some() {
        return None;
    }
    let (seconds, micros) = match seconds_field.split_once('.') {
        Some((seconds, fraction)) => (seconds.parse::<u32>().ok()?, parse_fraction(fraction)?),
        None => (seconds_field.parse::<u32>().ok()?, 0),
    };
    if hours > 24 || minutes > 59 || seconds > 60 {
        return None;
    }
    Some((hours, minutes, seconds, micros))
}

/// A fractional-second field of up to six digits, padded to microseconds.
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

/// `2026-01-31 12:00:00.123456+07` into microseconds since the epoch, plus the
/// offset the server reported.
///
/// The offset is returned rather than folded in: the instant is computed in UTC
/// for storage and the offset travels with it, so the value can be shown the way
/// the server meant it. Folding it in and discarding it is what makes a `+07:00`
/// timestamp come back looking like a UTC one.
fn parse_timestamp(text: &str) -> Option<(i64, i32)> {
    let text = text.trim();
    let (date_part, rest) = text.split_once([' ', 'T'])?;
    let (year, month, day) = parse_ymd(date_part)?;
    let (clock, offset) = split_offset(rest);
    let (hours, minutes, seconds, micros) = parse_clock(clock)?;
    if year < 1 {
        return None;
    }

    let days = days_from_civil(year, month, day);
    let seconds_of_day = i64::from(hours * 3600 + minutes * 60 + seconds);
    let local_micros = days * 86_400 * 1_000_000 + seconds_of_day * 1_000_000 + micros;
    let offset = offset.unwrap_or(0);
    Some((local_micros - i64::from(offset) * 1_000_000, offset))
}

/// `1 year 2 mons 3 days 04:05:06`, and the shapes PostgreSQL varies between.
///
/// Years are folded into months here (`1 year 2 mons` is `14 months`) because
/// [`IntervalValue`] has no year field, and months rather than days is the fold that
/// keeps the value: the server sent a calendar interval, and a month is not a fixed
/// number of days. Folding into days — which is what psycopg's `timedelta` did to
/// the Python engine — turns `14 months, 3 days` into `428 days` and cannot be
/// undone.
fn parse_interval(text: &str) -> Option<IntervalValue> {
    let mut interval = IntervalValue::default();
    let mut tokens = text.split_whitespace().peekable();
    let mut saw_any = false;

    while let Some(token) = tokens.next() {
        let Ok(amount) = token.parse::<i64>() else {
            // A clock field rather than a counted unit.
            let (hours, minutes, seconds, micros) = parse_clock(token)?;
            let sign = if interval.micros < 0 { -1 } else { 1 };
            let clock = i64::from(hours * 3600 + minutes * 60 + seconds) * 1_000_000 + micros;
            interval.micros = interval.micros.abs() + sign * clock;
            saw_any = true;
            continue;
        };
        let unit = tokens.next()?.trim_end_matches(',');
        match unit.trim_end_matches('s') {
            "year" => interval.months += (amount * 12) as i32,
            "mon" | "month" => interval.months += amount as i32,
            "day" => interval.days += amount as i32,
            "hour" => interval.micros += amount * 3_600_000_000,
            "min" | "minute" => interval.micros += amount * 60_000_000,
            "sec" | "second" => interval.micros += amount * 1_000_000,
            _ => return None,
        }
        saw_any = true;
    }

    saw_any.then_some(interval)
}

/// Days since the Unix epoch from a civil date. Howard Hinnant's
/// `days_from_civil`, the inverse of the conversion in `qh-core`.
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

    #[test]
    fn a_decimal_keeps_every_digit() {
        // The value tests/golden/preview/type_zoo.ndjson holds for NUMERIC(38,10).
        assert_eq!(
            from_text("numeric", Some("1234567890123456789012345678.1234567890")),
            Value::Decimal {
                unscaled: 12345678901234567890123456781234567890,
                scale: 10
            }
        );
        assert_eq!(
            from_text("numeric", Some("-0.0000000001")),
            Value::Decimal {
                unscaled: -1,
                scale: 10
            }
        );
        assert_eq!(
            from_text("numeric", Some("42")),
            Value::Decimal {
                unscaled: 42,
                scale: 0
            }
        );
        assert_eq!(
            from_text("numeric", Some("0")),
            Value::Decimal {
                unscaled: 0,
                scale: 0
            }
        );
    }

    #[test]
    fn a_decimal_never_becomes_a_float() {
        // Through f64 this comes back as ...5678.1235, losing ten digits.
        let Value::Decimal { unscaled, scale } =
            from_text("numeric", Some("1234567890123456789012345678.1234567890"))
        else {
            panic!("a numeric did not become a decimal");
        };
        assert_eq!(unscaled, 12345678901234567890123456781234567890);
        assert_eq!(scale, 10);
    }

    #[test]
    fn a_decimal_exponent_is_applied_rather_than_refused() {
        assert_eq!(
            from_text("numeric", Some("1.5e3")),
            Value::Decimal {
                unscaled: 1500,
                scale: 0
            }
        );
        // A hostile exponent must not ask for a gigabyte of zeros.
        assert!(matches!(
            from_text("numeric", Some("1e999999")),
            Value::Text(_)
        ));
    }

    #[test]
    fn integers_and_floats_keep_their_kind() {
        assert_eq!(
            from_text("int8", Some("-9223372036854775808")),
            Value::Int(i64::MIN)
        );
        assert_eq!(from_text("float8", Some("1.5")), Value::Float(1.5));
        assert_eq!(
            from_text("float8", Some("Infinity")),
            Value::Float(f64::INFINITY)
        );
        assert_eq!(
            from_text("float8", Some("-Infinity")),
            Value::Float(f64::NEG_INFINITY)
        );
        // An integer that will not parse stays text instead of becoming zero.
        assert_eq!(from_text("int4", Some("nope")), Value::Text("nope".into()));
    }

    #[test]
    fn booleans_use_the_servers_spelling() {
        assert_eq!(from_text("bool", Some("t")), Value::Bool(true));
        assert_eq!(from_text("bool", Some("f")), Value::Bool(false));
        assert_eq!(from_text("bool", Some("true")), Value::Bool(true));
    }

    #[test]
    fn bytea_arrives_as_its_exact_bytes() {
        // The zoo's value: a NUL and 0xFF, neither of which survives a UTF-8
        // round trip. Hex decoding is why it does.
        assert_eq!(
            from_text("bytea", Some("\\x0001ff")),
            Value::Bytes(vec![0x00, 0x01, 0xff])
        );
        assert_eq!(from_text("bytea", Some("\\x")), Value::Bytes(vec![]));
        // The legacy escape format, still what bytea_output=escape sends.
        assert_eq!(
            from_text("bytea", Some("\\000\\001\\377")),
            Value::Bytes(vec![0, 1, 0xff])
        );
        assert_eq!(
            from_text("bytea", Some("\\xZZ")),
            Value::Text("\\xZZ".into())
        );
        // An odd number of hex digits is not a byte string.
        assert_eq!(from_text("bytea", Some("\\x0")), Value::Text("\\x0".into()));
    }

    #[test]
    fn a_timestamp_keeps_its_offset_and_renders_back() {
        let value = from_text("timestamptz", Some("2026-01-31 12:00:00.123456+07"));
        assert_eq!(
            value.render_text().unwrap(),
            "2026-01-31 12:00:00.123456+07:00",
            "the offset must survive the round trip"
        );
        let Value::Timestamp { offset_secs, .. } = value else {
            panic!("a timestamptz did not become a timestamp");
        };
        assert_eq!(offset_secs, Some(7 * 3600));
    }

    #[test]
    fn a_timestamp_without_an_offset_does_not_gain_one() {
        assert_eq!(
            from_text("timestamp", Some("1970-01-01 00:00:00")),
            Value::Timestamp {
                micros: 0,
                offset_secs: None
            }
        );
        // 2026-01-31 12:00:00 UTC, which is 1769860800 seconds after the epoch.
        assert_eq!(
            from_text("timestamp", Some("2026-01-31 12:00:00.123456")),
            Value::Timestamp {
                micros: 1_769_860_800_123_456,
                offset_secs: None
            }
        );
    }

    #[test]
    fn a_date_split_on_its_hyphens_is_not_read_as_an_offset() {
        // `2026-01-31` ends in `-31`; reading that as an offset of thirty-one
        // hours would move the value by more than a day.
        assert_eq!(
            from_text("date", Some("2026-01-31")),
            Value::Date { days: 20_484 }
        );
        // And a clock with no offset stays offset-free.
        assert_eq!(
            from_text("time", Some("23:59:59.999999")),
            Value::Time {
                micros: 86_399_999_999
            }
        );
    }

    #[test]
    fn offsets_are_read_in_every_shape_the_server_writes() {
        for (text, expected) in [
            ("2026-01-31 12:00:00+07", 7 * 3600),
            ("2026-01-31 12:00:00+07:00", 7 * 3600),
            ("2026-01-31 12:00:00-05:30", -(5 * 3600 + 30 * 60)),
            ("2026-01-31 12:00:00+00", 0),
        ] {
            let Value::Timestamp { offset_secs, .. } = from_text("timestamptz", Some(text)) else {
                panic!("{text} did not parse");
            };
            assert_eq!(offset_secs, Some(expected), "{text}");
        }
    }

    #[test]
    fn dates_and_times_convert_without_a_clock() {
        assert_eq!(
            from_text("date", Some("2026-01-31")),
            Value::Date { days: 20_484 }
        );
        assert_eq!(
            from_text("date", Some("1970-01-01")),
            Value::Date { days: 0 }
        );
        assert_eq!(
            from_text("date", Some("1969-12-31")),
            Value::Date { days: -1 }
        );
        assert_eq!(
            from_text("time", Some("23:59:59.999999")),
            Value::Time {
                micros: 86_399_999_999
            }
        );
    }

    #[test]
    fn the_date_conversion_matches_the_rendering_in_qh_core() {
        // Round trip, so the two directions cannot drift apart.
        for text in [
            "1970-01-01",
            "2026-01-31",
            "2000-02-29",
            "1900-02-28",
            "1969-12-31",
        ] {
            assert_eq!(
                from_text("date", Some(text)).render_text().unwrap(),
                text,
                "{text}"
            );
        }
    }

    #[test]
    fn an_interval_reads_its_units() {
        assert_eq!(
            from_text("interval", Some("1 year 2 mons 3 days 04:05:06")),
            Value::Interval(IntervalValue {
                months: 14,
                days: 3,
                micros: 14_706_000_000
            })
        );
        assert_eq!(
            from_text("interval", Some("00:00:30")),
            Value::Interval(IntervalValue {
                months: 0,
                days: 0,
                micros: 30_000_000
            })
        );
    }

    #[test]
    fn the_zoos_interval_keeps_its_months_instead_of_folding_them_into_days() {
        // tests/golden/preview/postgres_type_zoo_live.ndjson, `an_interval`. The
        // previous engine wrote `"428 days, 4:05:06"`: psycopg's `timedelta`, with
        // the months already gone and the JSON quotes still on. This value is the
        // server's own `1 year 2 mons 3 days 04:05:06`, kept as the three parts it
        // was sent as and rendered back in the server's own units.
        let value = from_text("interval", Some("1 year 2 mons 3 days 04:05:06"));
        assert_eq!(
            value,
            Value::Interval(IntervalValue {
                months: 14,
                days: 3,
                micros: 14_706_000_000
            })
        );
        assert_eq!(
            value.render_text().unwrap(),
            "14 months, 3 days, 4:05:06",
            "a month is not a fixed number of days, so the parts stay apart"
        );
    }

    #[test]
    fn an_array_keeps_the_servers_own_literal() {
        // tests/golden/preview/postgres_type_zoo_live.ndjson, `ints`, `texts` and
        // `nested`. The previous engine decoded these with psycopg and rendered its
        // lists as JSON -- `[1, null, 3]` for the first -- while this keeps the
        // server's text, NULL spelled the server's way. Nothing reads a cell as JSON
        // (see the module doc), so the decode would buy nothing and cost a
        // PostgreSQL array-literal parser.
        // `_int4`, `_text`, `_int4` are the names the server reports for these three
        // columns, and an array's name is its element's with a leading underscore —
        // never the element's own name, which is what keeps an array out of the
        // integer arm above.
        for (type_name, text) in [
            ("_int4", "{1,NULL,3}"),
            ("_text", "{a,NULL,c}"),
            ("_int4", "{{1,2},{3,NULL}}"),
        ] {
            assert_eq!(
                from_text(type_name, Some(text)),
                Value::Text(text.into()),
                "{type_name} {text}"
            );
        }
    }

    #[test]
    fn a_uuid_is_bare_because_the_quotes_were_never_the_servers() {
        // tests/golden/preview/postgres_type_zoo_live.ndjson, `a_uuid`. The previous
        // engine's quotes came from `json.dumps(default=str)`, the same fallback
        // delta D-2 describes for an interval, not from PostgreSQL.
        let value = from_text("uuid", Some("550e8400-e29b-41d4-a716-446655440000"));
        assert_eq!(
            value.render_text().unwrap(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[test]
    fn json_is_kept_as_text_so_key_order_survives() {
        assert_eq!(
            from_text("json", Some(r#"{"b": 1, "a": 2}"#)),
            Value::Json(r#"{"b": 1, "a": 2}"#.into())
        );
        assert_eq!(
            from_text("jsonb", Some(r#"{"a": 2, "b": 1}"#)),
            Value::Json(r#"{"a": 2, "b": 1}"#.into())
        );
    }

    #[test]
    fn text_types_are_text() {
        for type_name in ["text", "varchar", "bpchar", "char", "name", "uuid"] {
            assert_eq!(
                from_text(type_name, Some("value")),
                Value::Text("value".into()),
                "{type_name}"
            );
        }
        // An empty string is a value, not a NULL; the four characters NULL are
        // not a NULL either.
        assert_eq!(from_text("text", Some("")), Value::Text("".into()));
        assert_eq!(from_text("text", Some("NULL")), Value::Text("NULL".into()));
    }

    #[test]
    fn a_type_modifier_does_not_confuse_the_parser() {
        assert_eq!(
            from_text("numeric(38,10)", Some("1.5")),
            Value::Decimal {
                unscaled: 15,
                scale: 1
            }
        );
        assert_eq!(
            from_text("character varying(10)", Some("x")),
            Value::Text("x".into())
        );
    }

    #[test]
    fn a_type_nobody_has_taught_this_module_about_is_text_not_a_panic() {
        assert_eq!(
            from_text("point", Some("(1,2)")),
            Value::Text("(1,2)".into())
        );
        assert_eq!(
            from_text("inet", Some("10.0.0.1/32")),
            Value::Text("10.0.0.1/32".into())
        );
        assert_eq!(
            from_text("integer[]", Some("{1,NULL,3}")),
            Value::Text("{1,NULL,3}".into())
        );
        assert_eq!(
            from_text("mood", Some("happy")),
            Value::Text("happy".into())
        );
    }

    #[test]
    fn a_malformed_value_keeps_its_text_rather_than_becoming_a_default() {
        for (type_name, text) in [
            ("date", "not a date"),
            ("timestamp", "2026-13-45 99:99:99"),
            ("time", "99:99"),
            ("interval", "wat"),
            ("bool", "maybe"),
        ] {
            assert_eq!(
                from_text(type_name, Some(text)),
                Value::Text(text.into()),
                "{type_name}"
            );
        }
    }
}
