//! Trino page values into [`qh_core::Value`].
//!
//! Trino is the one driver whose result set is **not** a binary protocol: every
//! value arrives inside JSON. That shapes this file, and one property of the JSON
//! encoding matters more than the rest, so it is stated here rather than
//! discovered later:
//!
//! **`timestamp` and `time` arrive at the precision the client asked for.** Trino
//! encodes these two into JSON at the precision the *client says it can read*, and
//! the request header that says so is `X-Trino-Client-Capabilities`. Measured
//! against Trino 483, one header apart and nothing else:
//!
//! ```text
//! SELECT TIMESTAMP '2026-01-31 12:00:00.123456 +07:00', TIME '23:59:59.999999'
//!
//! without PARAMETRIC_DATETIME:
//!   types: "timestamp with time zone", "time"
//!   data:  "2026-01-31 12:00:00.123 +07:00", "00:00:00.000"
//! with PARAMETRIC_DATETIME:
//!   types: "timestamp(6) with time zone", "time(6)"
//!   data:  "2026-01-31 12:00:00.123456 +07:00", "23:59:59.999999"
//! ```
//!
//! `23:59:59.999999` arriving as `00:00:00.000` is the same rounding seen from the
//! other side: 999.999 ms carries into the next second. The server holds
//! microseconds either way — `typeof(CAST('…123456' AS TIMESTAMP(6)))` reports
//! `timestamp(6)` in both cases — so nothing is lost here; the announcement is what
//! decides, and `crate::CLIENT_CAPABILITIES` is where this driver makes it. Both
//! encodings are decoded by the same code, which is the reason `base_type` strips a
//! parameter list without losing a `with time zone` suffix that sits behind it.
//!
//! The rest of the encodings were taken the same way — by asking a real server
//! and reading what came back, never from prose:
//!
//! | Trino type | JSON on the wire |
//! |---|---|
//! | `decimal(38,10)` | **string**, `"1234567890123456789012345678.1234567890"` |
//! | `bigint` | number, including `-9223372036854775808` |
//! | `varbinary` | **base64** string, `"AP8="` for `X'00FF'` |
//! | `json` | a string whose *content* is JSON |
//! | `array(T)` | array |
//! | `map(K,V)` | object — keys arrive stringified, whatever `K` is |
//! | `row(...)` | array |
//! | `date` | `"2026-01-31"` |
//! | `timestamp(6) with time zone` | `"2026-01-31 12:00:00.123456 +07:00"` |
//! | `time(6)` | `"23:59:59.999999"` |
//! | `interval day to second` | **string**, `"3 04:05:06.000"` |
//! | `interval year to month` | **string**, `"2-3"` |
//!
//! The two precision-bearing rows are what the capability buys; without it the same
//! value arrives typed `timestamp with time zone` / `time` and rounded, and both
//! spellings decode through the same branches below.

use qh_core::{IntervalValue, Value};
use serde_json::Value as Json;

/// Decode one cell, given the type text Trino reported for its column.
///
/// `type_text` is the column's `type` exactly as the page carried it, e.g.
/// `decimal(38, 10)` — with the space, which is how Trino writes it.
pub fn decode(type_text: &str, value: &Json) -> Value {
    // A NULL is a NULL whatever the column type says, and it must be checked
    // first: every branch below would otherwise have to repeat it.
    if value.is_null() {
        return Value::Null;
    }

    let base = base_type(type_text);
    match base {
        "boolean" => match value.as_bool() {
            Some(flag) => Value::Bool(flag),
            None => unmodelled(type_text, value),
        },

        "tinyint" | "smallint" | "integer" | "bigint" => as_int(type_text, value),
        "real" | "double" => as_float(type_text, value),

        // A JSON string that keeps all its digits. Reading it through a float is
        // the one way to lose a 38-digit decimal, so it is parsed as text into an
        // exact unscaled integer and never touches `f64`.
        "decimal" => match value.as_str() {
            Some(text) => parse_decimal(text)
                .map(|(unscaled, scale)| Value::Decimal { unscaled, scale })
                .unwrap_or_else(|| unmodelled(type_text, value)),
            None => unmodelled(type_text, value),
        },

        "varchar" | "char" | "uuid" => match value.as_str() {
            Some(text) => Value::Text(text.into()),
            None => unmodelled(type_text, value),
        },

        // Base64 on the wire. Passing the base64 text through as bytes would be
        // wrong in a way that only shows up on export, so it is decoded here.
        "varbinary" => match value.as_str().and_then(decode_base64) {
            Some(bytes) => Value::Bytes(bytes),
            None => unmodelled(type_text, value),
        },

        // Kept verbatim: re-serialising would reorder keys and change what the
        // user sees.
        "json" => match value.as_str() {
            Some(text) => Value::Json(text.into()),
            None => unmodelled(type_text, value),
        },

        "date" => match value.as_str().and_then(parse_date) {
            Some(days) => Value::Date { days },
            None => unmodelled(type_text, value),
        },

        "time" => match value.as_str().and_then(parse_time_micros) {
            Some(micros) => Value::Time { micros },
            None => unmodelled(type_text, value),
        },

        "timestamp" | "timestamp with time zone" => {
            match value
                .as_str()
                .and_then(|text| parse_timestamp(text, base.ends_with("with time zone")))
            {
                Some((micros, offset_secs)) => Value::Timestamp {
                    micros,
                    offset_secs,
                },
                None => unmodelled(type_text, value),
            }
        }

        "array" => match (value.as_array(), single_argument(type_text, "array")) {
            (Some(items), Some(element)) => {
                Value::Array(items.iter().map(|item| decode(&element, item)).collect())
            }
            _ => unmodelled(type_text, value),
        },

        "row" => match (value.as_array(), arguments(type_text, "row")) {
            (Some(items), Some(types)) if items.len() == types.len() => Value::Row(
                items
                    .iter()
                    .zip(types.iter())
                    .map(|(item, type_name)| decode(type_name, item))
                    .collect(),
            ),
            _ => unmodelled(type_text, value),
        },

        // JSON gives an object, so the keys are strings however the key type was
        // declared. `as_int` and `as_float` accept a numeric string for exactly
        // this reason, which is what makes `map(bigint, varchar)` come back with
        // integer keys rather than string ones.
        "map" => match (value.as_object(), arguments(type_text, "map")) {
            (Some(entries), Some(types)) if types.len() == 2 => Value::Map(
                entries
                    .iter()
                    .map(|(key, entry)| {
                        (
                            decode(&types[0], &Json::String(key.clone())),
                            decode(&types[1], entry),
                        )
                    })
                    .collect(),
            ),
            _ => unmodelled(type_text, value),
        },

        // Trino's two interval types, and the only branches here whose type name
        // arrives in upper case: measured against 483, the column says
        // `INTERVAL DAY TO SECOND` where every other type is lower case, so these
        // two comparisons cannot be plain `match` patterns.
        _ if base.eq_ignore_ascii_case("interval day to second") => {
            match value.as_str().and_then(parse_interval_day_to_second) {
                Some(interval) => Value::Interval(interval),
                None => unmodelled(type_text, value),
            }
        }
        _ if base.eq_ignore_ascii_case("interval year to month") => {
            match value.as_str().and_then(parse_interval_year_to_month) {
                Some(interval) => Value::Interval(interval),
                None => unmodelled(type_text, value),
            }
        }

        // Geometry, `inet`, and anything a future Trino adds. A normal outcome, not
        // a defect: the text form is kept so the grid shows something and an export
        // can still write it.
        _ => unmodelled(type_text, value),
    }
}

/// The type name without its parameter list, so `decimal(38, 10)` and
/// `array(integer)` both reduce to a word this file can match on.
fn base_type(type_text: &str) -> &str {
    let text = type_text.trim();
    // `timestamp(3) with time zone` keeps its suffix: the parenthesis is in the
    // middle, not at the end, so cutting at the first `(` would turn it into a
    // plain `timestamp` and silently drop the zone.
    if let Some(rest) = text.strip_prefix("timestamp") {
        if rest.contains("with time zone") {
            return "timestamp with time zone";
        }
    }
    match text.find('(') {
        Some(index) => text[..index].trim(),
        None => text,
    }
}

/// The contents of `name(...)`, e.g. `array(integer)` -> `integer`.
fn single_argument(type_text: &str, name: &str) -> Option<String> {
    let mut types = arguments(type_text, name)?;
    if types.len() == 1 {
        Some(types.remove(0))
    } else {
        None
    }
}

/// Split the top-level arguments of `name(...)`, so `row(integer, varchar(1))`
/// gives two types rather than four.
fn arguments(type_text: &str, name: &str) -> Option<Vec<String>> {
    let text = type_text.trim();
    let inner = text.strip_prefix(name)?.trim_start().strip_prefix('(')?;
    let inner = inner.strip_suffix(')')?;

    let mut types = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for character in inner.chars() {
        match character {
            '(' => {
                depth += 1;
                current.push(character);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(character);
            }
            ',' if depth == 0 => {
                types.push(current.trim().to_owned());
                current.clear();
            }
            _ => current.push(character),
        }
    }
    if !current.trim().is_empty() {
        types.push(current.trim().to_owned());
    }
    Some(types)
}

/// Integers, accepting a JSON string as well as a number.
///
/// The string form is not a quirk of this driver: `map(bigint, ...)` arrives as a
/// JSON object, and JSON object keys are strings, so an integer key can only be
/// an integer if it is parsed back out.
fn as_int(type_text: &str, value: &Json) -> Value {
    if let Some(number) = value.as_i64() {
        return Value::Int(number);
    }
    if let Some(text) = value.as_str() {
        if let Ok(number) = text.parse::<i64>() {
            return Value::Int(number);
        }
    }
    if let Some(number) = value.as_u64() {
        // Above `i64::MAX` only on types that cannot reach it, but a value that
        // does is better kept as unsigned than wrapped.
        return Value::UInt(number);
    }
    unmodelled(type_text, value)
}

fn as_float(type_text: &str, value: &Json) -> Value {
    if let Some(number) = value.as_f64() {
        return Value::Float(number);
    }
    if let Some(text) = value.as_str() {
        if let Ok(number) = text.parse::<f64>() {
            return Value::Float(number);
        }
    }
    unmodelled(type_text, value)
}

/// `"1234567890123456789012345678.1234567890"` into `(unscaled, scale)`.
///
/// `unscaled / 10^scale`, so nothing is rounded and no float is involved.
fn parse_decimal(text: &str) -> Option<(i128, u8)> {
    let text = text.trim();
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (whole, fraction) = match digits.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (digits, ""),
    };
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    if !whole.chars().all(|c| c.is_ascii_digit()) || !fraction.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // Trino writes a decimal with the declared scale, so the fraction's length is
    // the scale. Keeping it rather than trimming zeros preserves `1.50` as
    // `1.50`, which is the point of an exact decimal.
    let scale = u8::try_from(fraction.len()).ok()?;
    let mut unscaled: i128 = 0;
    for character in whole.chars().chain(fraction.chars()) {
        unscaled = unscaled
            .checked_mul(10)?
            .checked_add(i128::from(character as u8 - b'0'))?;
    }
    if negative {
        unscaled = -unscaled;
    }
    Some((unscaled, scale))
}

/// `"2026-01-31"` as days since the Unix epoch.
fn parse_date(text: &str) -> Option<i32> {
    let mut parts = text.trim().split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    i32::try_from(days_from_civil(year, month, day)).ok()
}

/// `"12:00:00.123"` as microseconds since midnight.
fn parse_time_micros(text: &str) -> Option<i64> {
    let mut parts = text.trim().split(':');
    let hours: i64 = parts.next()?.parse().ok()?;
    let minutes: i64 = parts.next()?.parse().ok()?;
    let seconds_text = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let (seconds, fraction) = match seconds_text.split_once('.') {
        Some((seconds, fraction)) => (seconds, fraction),
        None => (seconds_text, ""),
    };
    let seconds: i64 = seconds.parse().ok()?;
    Some(
        hours * 3_600_000_000
            + minutes * 60_000_000
            + seconds * 1_000_000
            + fraction_to_micros(fraction)?,
    )
}

/// `"3 04:05:06.000"` as an [`IntervalValue`], with months left at zero.
///
/// The sign is a property of the **whole interval**, not of the day field, and that
/// is the fact this function exists to get right. Measured against 483, three
/// expressions and nothing else between them:
///
/// ```text
/// INTERVAL '-3' DAY + INTERVAL '-4' HOUR   -3 04:00:00.000   -(3d 4h)
/// INTERVAL '-3' DAY + INTERVAL  '4' HOUR   -2 20:00:00.000   -(2d 20h)
/// INTERVAL  '1' DAY - INTERVAL  '4' HOUR    0 20:00:00.000
/// INTERVAL '-4' HOUR                       -0 04:00:00.000   -4h, and the days are "-0"
/// ```
///
/// So `-3 04:00:00.000` is not three negative days with four positive hours: the
/// server normalises to a magnitude plus one sign, and the second row is that
/// normalisation visible (`-68h` printed as `-(2d 20h)`). Reading the two fields with
/// independent signs would put a value on screen that is 8 hours away from the one
/// the server sent.
///
/// `"-0"` is why the sign is read from the text rather than left to `parse`, which
/// answers zero for `-0` and would turn the third measurement — the ordinary case of a
/// negative sub-day interval — into a positive one.
///
/// **The wire carries milliseconds and no more.** `INTERVAL '6.000007' SECOND` comes
/// back as `0 00:00:06.000`, because `INTERVAL DAY TO SECOND` is stored in millis on
/// the server, so the `micros` this produces is always a multiple of 1000 and the
/// truncation happened before the driver saw it. Nothing is thrown away here.
fn parse_interval_day_to_second(text: &str) -> Option<IntervalValue> {
    let (days_text, clock_text) = text.trim().split_once(' ')?;
    let negative = days_text.starts_with('-');
    let days: i64 = days_text.trim_start_matches(['+', '-']).parse().ok()?;
    let clock = parse_time_micros(clock_text)?;

    let sign = if negative { -1 } else { 1 };
    Some(IntervalValue {
        months: 0,
        days: i32::try_from(sign * days).ok()?,
        micros: sign * clock,
    })
}

/// `"2-3"` as an [`IntervalValue`]: two years, three months, so 27 months.
///
/// Years are folded into **months** and never into days. A year is exactly twelve
/// months, so that fold is lossless and reversible; a month is not a fixed number of
/// days, so the other fold is not, and it is what the previous engine's `timedelta`
/// did when it turned a calendar interval into `428 days`. [`IntervalValue`] has no
/// year field, so months is the only place a year can go — the same constraint, and
/// the same choice, as PostgreSQL's decoder
/// (`crates/qh-driver-postgres/src/normalize.rs`, `parse_interval`).
///
/// The sign leads the whole value, as in `-1-9` for
/// `INTERVAL '-2' YEAR + INTERVAL '3' MONTH` — 21 months, normalised by the server so
/// the month field is always under twelve, once more with the magnitude and the sign
/// kept apart.
fn parse_interval_year_to_month(text: &str) -> Option<IntervalValue> {
    let text = text.trim();
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let (years, months) = rest.split_once('-')?;
    let total = years
        .trim()
        .parse::<i64>()
        .ok()?
        .checked_mul(12)?
        .checked_add(months.trim().parse().ok()?)?;

    let sign = if negative { -1 } else { 1 };
    Some(IntervalValue {
        months: i32::try_from(sign * total).ok()?,
        days: 0,
        micros: 0,
    })
}

/// `"2026-01-31 12:00:00.123"`, optionally with `" UTC"` or `"+07:00"`.
///
/// **`micros` is the instant**, and the zone travels beside it in `offset_secs`
/// rather than being folded in and forgotten. That is the model Python's `datetime`
/// has — an instant plus a `tzinfo` — and therefore the model the snapshots were
/// recorded with: the server says `12:00 +07:00`, the instant is 05:00Z, and a
/// renderer asked for `+07:00` prints 12:00 again.
///
/// The alternative, keeping the wall clock in `micros`, is what PostgreSQL's decoder
/// used to disagree with this one about. Two drivers with two conventions and one
/// renderer cannot both be right; `docs/golden-deltas.md` T-1 has the details.
fn parse_timestamp(text: &str, has_zone: bool) -> Option<(i64, Option<i32>)> {
    let text = text.trim();
    let (rest, offset_secs) = if has_zone {
        let (rest, offset) = split_offset(text)?;
        (rest, Some(offset))
    } else {
        (text, None)
    };

    let (date_text, time_text) = rest.split_once(' ')?;
    let days = i64::from(parse_date(date_text)?);
    let time_micros = parse_time_micros(time_text)?;
    let wall_micros = days * 86_400_000_000 + time_micros;
    // Subtracting the offset is what turns the wall clock into the instant. A
    // zone-less timestamp keeps its reading, which is the same thing with an offset
    // of zero.
    let micros = wall_micros - i64::from(offset_secs.unwrap_or(0)) * 1_000_000;
    Some((micros, offset_secs))
}

/// Split `"2026-01-31 12:00:00.123 UTC"` into its wall clock and offset.
fn split_offset(text: &str) -> Option<(&str, i32)> {
    let (rest, zone) = text.rsplit_once(' ')?;
    let zone = zone.trim();
    if zone.eq_ignore_ascii_case("utc") || zone.eq_ignore_ascii_case("z") {
        return Some((rest, 0));
    }

    // `+07:00`, `-05:00`, or `+07`.
    let (sign, digits) = match zone.strip_prefix('+') {
        Some(digits) => (1, digits),
        None => (-1, zone.strip_prefix('-')?),
    };
    let (hours, minutes) = match digits.split_once(':') {
        Some((hours, minutes)) => (hours, minutes),
        None => (digits, "0"),
    };
    let hours: i32 = hours.parse().ok()?;
    let minutes: i32 = minutes.parse().ok()?;
    Some((rest, sign * (hours * 3600 + minutes * 60)))
}

/// `.123` as microseconds, padding a short fraction so `.1` is 100 ms.
fn fraction_to_micros(fraction: &str) -> Option<i64> {
    if fraction.is_empty() {
        return Some(0);
    }
    if !fraction.chars().all(|c| c.is_ascii_digit()) || fraction.len() > 6 {
        return None;
    }
    let mut digits = fraction.to_owned();
    while digits.len() < 6 {
        digits.push('0');
    }
    digits.parse().ok()
}

/// Days since the Unix epoch for a proleptic Gregorian date.
///
/// Howard Hinnant's `days_from_civil`, which is exact for every date this type can
/// carry and has no dependency and no leap-year table to get wrong.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * if month > 2 { month - 3 } else { month + 9 } + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Standard base64, which is what Trino puts on the wire for `varbinary`.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = text
        .trim()
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();

    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut buffer = 0u32;
    let mut bits = 0u32;
    for byte in bytes {
        if byte == b'=' {
            break;
        }
        let index = ALPHABET.iter().position(|candidate| *candidate == byte)? as u32;
        buffer = (buffer << 6) | index;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    Some(out)
}

/// A value for a type this file does not model.
///
/// The text form is kept when the JSON can be rendered as one, so the grid shows
/// the user something and the type name still travels with it.
fn unmodelled(type_text: &str, value: &Json) -> Value {
    match value {
        Json::String(text) => Value::unknown(type_text, text.clone()),
        other => Value::unknown(type_text, other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(text: &str) -> Json {
        serde_json::from_str(text).expect("test JSON")
    }

    #[test]
    fn a_null_is_a_null_for_every_type() {
        // Checked before the type is looked at, so a column of any type can hold
        // one and no branch has to remember to handle it.
        for type_text in [
            "bigint",
            "decimal(38, 10)",
            "array(integer)",
            "row(integer)",
            "json",
        ] {
            assert_eq!(decode(type_text, &Json::Null), Value::Null, "{type_text}");
        }
    }

    #[test]
    fn a_decimal_keeps_every_digit_and_its_scale() {
        // The measured encoding: a JSON *string*, not a number, which is what
        // makes 38 digits survive at all.
        let value = decode(
            "decimal(38, 10)",
            &json("\"1234567890123456789012345678.1234567890\""),
        );
        assert_eq!(
            value,
            Value::Decimal {
                unscaled: 12_345_678_901_234_567_890_123_456_781_234_567_890,
                scale: 10
            }
        );

        // The scale is the fraction's length, so a trailing zero is kept: `1.50`
        // is not `1.5` in an exact type.
        assert_eq!(
            decode("decimal(3, 2)", &json("\"1.50\"")),
            Value::Decimal {
                unscaled: 150,
                scale: 2
            }
        );
        assert_eq!(
            decode("decimal(3, 0)", &json("\"100\"")),
            Value::Decimal {
                unscaled: 100,
                scale: 0
            }
        );
        assert_eq!(
            decode("decimal(3, 2)", &json("\"-1.25\"")),
            Value::Decimal {
                unscaled: -125,
                scale: 2
            }
        );
    }

    #[test]
    fn varbinary_is_base64_decoded_rather_than_passed_through() {
        // X'00FF' arrives as "AP8=". Keeping the text would be wrong in a way
        // that only shows up on export.
        assert_eq!(
            decode("varbinary", &json("\"AP8=\"")),
            Value::Bytes(vec![0x00, 0xff])
        );
        assert_eq!(decode("varbinary", &json("\"\"")), Value::Bytes(Vec::new()));
        // "aGVsbG8=" is "hello", and the padding variants are the ones an
        // off-by-one in the decoder gets wrong.
        assert_eq!(
            decode("varbinary", &json("\"aGVsbG8=\"")),
            Value::Bytes(b"hello".to_vec())
        );
        assert_eq!(
            decode("varbinary", &json("\"aGk=\"")),
            Value::Bytes(b"hi".to_vec())
        );
        assert_eq!(
            decode("varbinary", &json("\"eA==\"")),
            Value::Bytes(b"x".to_vec())
        );
    }

    #[test]
    fn a_parametric_timestamp_and_time_keep_all_six_digits() {
        // What the coordinator sends once the client announces
        // `PARAMETRIC_DATETIME` (`crate::CLIENT_CAPABILITIES`): the precision
        // travels in the type text *and* in the value, so the decoder has to strip
        // `(6)` from the type without losing it from the digits.
        assert_eq!(
            decode("timestamp(6)", &json("\"2026-01-31 12:00:00.123456\"")),
            Value::Timestamp {
                micros: 1_769_860_800_123_456,
                offset_secs: None
            }
        );
        assert_eq!(
            decode(
                "timestamp(6) with time zone",
                &json("\"2026-01-31 12:00:00.123456 +07:00\"")
            ),
            Value::Timestamp {
                micros: 1_769_835_600_123_456,
                offset_secs: Some(25_200)
            }
        );
        // 23:59:59.999999 rendered back is the value that would round up into the
        // next second if the announcement were dropped: 86_399_999_999 µs, not
        // zero.
        assert_eq!(
            decode("time(6)", &json("\"23:59:59.999999\"")),
            Value::Time {
                micros: 86_399_999_999
            }
        );

        // A parameterised type nested in another one goes through `arguments`
        // before `base_type`, which is the second place a `(6)` could be mistaken
        // for the end of the type name.
        assert_eq!(
            decode(
                "array(timestamp(6) with time zone)",
                &json("[\"2026-01-31 12:00:00.123456 +07:00\"]")
            ),
            Value::Array(vec![Value::Timestamp {
                micros: 1_769_835_600_123_456,
                offset_secs: Some(25_200)
            }])
        );
    }

    #[test]
    fn a_parametric_type_name_still_reduces_to_the_branch_it_matches() {
        // `base_type` is the one place the `(6)` is removed, and every branch above
        // shares it, so `time(6)` has to reach the `time` arm and
        // `timestamp(6) with time zone` the zoned one. The unprefixed cases are
        // pinned next door; what is added here is the parameter a client that
        // announced `PARAMETRIC_DATETIME` is answered with.
        assert_eq!(base_type("time(6)"), "time");
        assert_eq!(
            base_type("timestamp(6) with time zone"),
            "timestamp with time zone"
        );
    }

    #[test]
    fn timestamps_keep_the_instant_and_the_zone_separately() {
        // Noon on 2026-01-31 in UTC, which is the same instant the next case shows as
        // 19:00+07:00 and as 12:00+07:00 when the zone it was reported in is applied.
        // The zone travels beside the instant rather than being folded in and lost.
        let utc = decode(
            "timestamp with time zone",
            &json("\"2026-01-31 12:00:00.123 UTC\""),
        );
        assert_eq!(
            utc,
            Value::Timestamp {
                micros: 1_769_860_800_123_000,
                offset_secs: Some(0)
            }
        );

        // The same reading as 12:00 in +07:00 is 05:00Z: seven hours earlier as an
        // instant, which is what a value has to mean for two zones to agree about the
        // moment they describe.
        let plus_seven = decode(
            "timestamp with time zone",
            &json("\"2026-01-31 12:00:00.123 +07:00\""),
        );
        assert_eq!(
            plus_seven,
            Value::Timestamp {
                micros: 1_769_835_600_123_000,
                offset_secs: Some(25_200)
            }
        );
        // And the reading is what comes back out: the instant shown in its own zone.
        assert_eq!(
            plus_seven.render_text().as_deref(),
            Some("2026-01-31 12:00:00.123000+07:00")
        );
    }

    #[test]
    fn the_zone_suffix_is_not_mistaken_for_a_parameter_list() {
        // `timestamp(3) with time zone` has its parenthesis in the middle. Cutting
        // at the first `(` would make it a zone-less timestamp and drop the offset
        // without a word.
        assert_eq!(
            base_type("timestamp(3) with time zone"),
            "timestamp with time zone"
        );
        assert_eq!(
            base_type("timestamp with time zone"),
            "timestamp with time zone"
        );
        assert_eq!(base_type("timestamp(6)"), "timestamp");
        assert_eq!(base_type("decimal(38, 10)"), "decimal");
    }

    #[test]
    fn a_row_type_splits_on_top_level_commas_only() {
        // `row(integer, varchar(1))` has one comma at depth zero, not three: a
        // naive split would produce `varchar(1` and ``.
        assert_eq!(
            arguments("row(integer, varchar(1))", "row"),
            Some(vec!["integer".to_owned(), "varchar(1)".to_owned()])
        );
        assert_eq!(
            arguments("map(varchar(1), array(bigint))", "map"),
            Some(vec!["varchar(1)".to_owned(), "array(bigint)".to_owned()])
        );
        assert_eq!(
            arguments("array(integer)", "array"),
            Some(vec!["integer".to_owned()])
        );
    }

    #[test]
    fn arrays_rows_and_maps_carry_their_own_types() {
        assert_eq!(
            decode("array(integer)", &json("[1, 2]")),
            Value::Array(vec![Value::Int(1), Value::Int(2)])
        );
        assert_eq!(
            decode("row(integer, varchar(1))", &json("[1, \"a\"]")),
            Value::Row(vec![Value::Int(1), Value::Text("a".into())])
        );
        // A map arrives as a JSON object, so the key is a string on the wire. It
        // is decoded with the *declared key type*, which is what makes an integer
        // key an integer rather than "1".
        assert_eq!(
            decode("map(integer, integer)", &json("{\"1\": 10}")),
            Value::Map(vec![(Value::Int(1), Value::Int(10))])
        );
        assert_eq!(
            decode("map(varchar(1), integer)", &json("{\"a\": 1}")),
            Value::Map(vec![(Value::Text("a".into()), Value::Int(1))])
        );
        // Nested, because a one-level decoder passes its own test and fails here.
        assert_eq!(
            decode(
                "array(row(integer, varchar(1)))",
                &json("[[1, \"a\"], [2, \"b\"]]")
            ),
            Value::Array(vec![
                Value::Row(vec![Value::Int(1), Value::Text("a".into())]),
                Value::Row(vec![Value::Int(2), Value::Text("b".into())]),
            ])
        );
    }

    #[test]
    fn the_widest_and_narrowest_integers_survive() {
        // i64::MIN is the one a sign-handling bug drops, and it is in the range
        // Trino's bigint actually reaches.
        assert_eq!(
            decode("bigint", &json("-9223372036854775808")),
            Value::Int(i64::MIN)
        );
        assert_eq!(
            decode("bigint", &json("9223372036854775807")),
            Value::Int(i64::MAX)
        );
        assert_eq!(decode("tinyint", &json("7")), Value::Int(7));
    }

    #[test]
    fn dates_and_times_are_the_documented_values() {
        // 2026-01-31 is 20458 days after the epoch; the epoch itself is the case a
        // leap-year bug gets wrong, so it is checked too.
        assert_eq!(
            decode("date", &json("\"1970-01-01\"")),
            Value::Date { days: 0 }
        );
        assert_eq!(
            decode("date", &json("\"2026-01-31\"")),
            Value::Date {
                days: days_from_civil(2026, 1, 31) as i32
            }
        );
        // A leap day, which no table-free implementation should get wrong.
        assert_eq!(
            decode("date", &json("\"2024-02-29\"")),
            Value::Date {
                days: days_from_civil(2024, 2, 29) as i32
            }
        );
        assert_eq!(
            decode("time", &json("\"12:00:00.123\"")),
            Value::Time {
                micros: 12 * 3_600_000_000 + 123_000
            }
        );
        // `.1` is 100 ms, not 1 microsecond: a short fraction is padded, not
        // read at face value.
        assert_eq!(
            decode("time", &json("\"00:00:00.1\"")),
            Value::Time { micros: 100_000 }
        );
    }

    #[test]
    fn both_interval_spellings_become_an_interval_value() {
        // The column type is upper case on the wire, unlike every other type name,
        // so both arms are matched without case — these two type texts are the ones
        // a real 483 coordinator reported for the two expressions below.
        let day_to_second = decode("INTERVAL DAY TO SECOND", &json("\"3 04:05:06.000\""));
        assert_eq!(
            day_to_second,
            Value::Interval(IntervalValue {
                months: 0,
                days: 3,
                micros: 14_706_000_000
            })
        );
        // The point of the change: the cell reaches the shared renderer, which is the
        // same one PostgreSQL's interval goes through, rather than the wire text
        // `3 04:05:06.000` being passed straight to the grid.
        assert_eq!(
            day_to_second.render_text().as_deref(),
            Some("3 days, 4:05:06")
        );

        let year_to_month = decode("INTERVAL YEAR TO MONTH", &json("\"2-3\""));
        assert_eq!(
            year_to_month,
            Value::Interval(IntervalValue {
                months: 27,
                days: 0,
                micros: 0
            })
        );
        // Months, not days: a year is exactly twelve months, a month is not 30 days.
        // `IntervalValue` has no year field, so 2y3m can only be 27 months.
        assert_eq!(
            year_to_month.render_text().as_deref(),
            Some("27 months, 0:00:00")
        );
    }

    #[test]
    fn the_intervals_sign_covers_both_fields() {
        // `-3 04:00:00.000` is -(3d 4h), not -3d +4h: the server normalises to a
        // magnitude and one leading sign. Reading the fields independently would put
        // a value on screen 8 hours from the one sent.
        assert_eq!(
            decode("interval day to second", &json("\"-3 04:00:00.000\"")),
            Value::Interval(IntervalValue {
                months: 0,
                days: -3,
                micros: -14_400_000_000
            })
        );
        assert_eq!(
            decode("interval day to second", &json("\"-2 20:00:00.000\"")),
            Value::Interval(IntervalValue {
                months: 0,
                days: -2,
                micros: -72_000_000_000
            })
        );

        // `-0 04:00:00.000` is the negative sub-day case, and `"-0".parse()` is 0:
        // the sign has to be read off the text, or this decodes as +4 hours.
        let negative_hours = decode("interval day to second", &json("\"-0 04:00:00.000\""));
        assert_eq!(
            negative_hours,
            Value::Interval(IntervalValue {
                months: 0,
                days: 0,
                micros: -14_400_000_000
            })
        );
        assert_eq!(negative_hours.render_text().as_deref(), Some("-4:00:00"));

        // A negative year-to-month leads with its sign, and the server has already
        // normalised -2y+3m into -1y9m.
        assert_eq!(
            decode("interval year to month", &json("\"-1-9\"")),
            Value::Interval(IntervalValue {
                months: -21,
                days: 0,
                micros: 0
            })
        );
    }

    #[test]
    fn a_zero_interval_and_a_millisecond_one_keep_their_reading() {
        // `INTERVAL '0' SECOND` is measured as `0 00:00:00.000`, and it renders the
        // way Python's `str(timedelta(0))` did.
        let zero = decode("interval day to second", &json("\"0 00:00:00.000\""));
        assert_eq!(
            zero,
            Value::Interval(IntervalValue {
                months: 0,
                days: 0,
                micros: 0
            })
        );
        assert_eq!(zero.render_text().as_deref(), Some("0:00:00"));

        // The sub-second part survives as far as the wire carries it:
        // `INTERVAL '1.5' SECOND` is `0 00:00:01.500`.
        assert_eq!(
            decode("interval day to second", &json("\"0 00:00:01.500\"")),
            Value::Interval(IntervalValue {
                months: 0,
                days: 0,
                micros: 1_500_000
            })
        );
        // Three fraction digits always, so `.000` is not `None`: `INTERVAL '9' SECOND`
        // is `0 00:00:09.000`.
        assert_eq!(
            decode("interval day to second", &json("\"0 00:00:09.000\"")),
            Value::Interval(IntervalValue {
                months: 0,
                days: 0,
                micros: 9_000_000
            })
        );
    }

    #[test]
    fn a_malformed_interval_is_unmodelled_rather_than_a_panic() {
        // Same rule as every other branch: a server that changes its encoding
        // degrades, it does not take the process down.
        for (type_text, payload) in [
            ("interval day to second", "\"not an interval\""),
            ("interval day to second", "\"3 4:05\""),
            ("interval day to second", "5"),
            // A year-to-month has to have the `-`, or there is no second field.
            ("interval year to month", "\"27\""),
            ("interval year to month", "\"2-3-4\""),
        ] {
            match decode(type_text, &json(payload)) {
                Value::Unknown { .. } => {}
                other => panic!("{type_text} with {payload} should be unmodelled, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_nested_interval_keeps_its_case_insensitive_type_name() {
        // `array(INTERVAL DAY TO SECOND)` — the element type text travels inside the
        // parentheses and is upper case there too.
        assert_eq!(
            decode(
                "array(INTERVAL DAY TO SECOND)",
                &json("[\"1 00:00:00.000\", \"-0 00:00:30.000\"]")
            ),
            Value::Array(vec![
                Value::Interval(IntervalValue {
                    months: 0,
                    days: 1,
                    micros: 0
                }),
                Value::Interval(IntervalValue {
                    months: 0,
                    days: 0,
                    micros: -30_000_000
                }),
            ])
        );
    }

    #[test]
    fn a_type_this_driver_does_not_model_still_shows_something() {
        // Geometry and anything a future release adds. Losing the value would be
        // worse than showing its text, and the type name travels with it so the
        // chip still says what it is.
        match decode("geometry", &json("\"POINT (30 10)\"")) {
            Value::Unknown {
                type_name, text, ..
            } => {
                assert_eq!(&*type_name, "geometry");
                assert_eq!(text.as_deref(), Some("POINT (30 10)"));
            }
            other => panic!("expected an unmodelled value, got {other:?}"),
        }
    }

    #[test]
    fn a_malformed_payload_is_unmodelled_rather_than_a_panic() {
        // A server that changes its encoding should degrade, not take the process
        // down: a panic here crosses into the UI.
        for (type_text, payload) in [
            ("decimal(38, 10)", "1.5"),
            ("date", "\"not a date\""),
            ("timestamp", "\"not a timestamp\""),
            ("varbinary", "\"!!!not base64!!!\""),
            ("bigint", "\"not a number\""),
            ("array(integer)", "5"),
        ] {
            match decode(type_text, &json(payload)) {
                Value::Unknown { .. } => {}
                other => panic!("{type_text} with {payload} should be unmodelled, got {other:?}"),
            }
        }
    }
}
