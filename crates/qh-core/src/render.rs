//! Rendering a [`Value`] as text, or as JSON.
//!
//! This is the Rust side of `exporter/writers.py`'s `to_text` and `to_json_value`
//! (lines 38 and 58), and it is deliberately a *rendering* layer rather than a
//! conversion: the internal [`Value`] keeps its type, and text is produced only
//! when something is being written out. That is the difference the blueprint asks
//! for at §1.7 — the Python engine returned strings from the start, so a DECIMAL
//! could reach the export writers already rounded.
//!
//! The rules below were read off the Python source and, where the Python source
//! defines them by *calling Python*, off Python itself:
//!
//! ```text
//! datetime(2026,1,31,12,0,0,123456).isoformat(sep=" ")      2026-01-31 12:00:00.123456
//! datetime(.., tzinfo=timezone.utc).isoformat(sep=" ")      2026-01-31 12:00:00.123456+00:00
//! time(12,0,0,123456).isoformat()                           12:00:00.123456
//! time(12,0,0).isoformat()                                  12:00:00
//! bytes([0,255]).hex()                                      00ff
//! ```
//!
//! Three things follow from those, and each one is a trap in an implementation
//! that guesses instead:
//!
//! - a fraction is **omitted entirely** when it is zero, so `12:00:00` and not
//!   `12:00:00.000000`;
//! - an offset is rendered as `+00:00`, in the same `±HH:MM` shape for every zone;
//! - `bytes` are **hex**, not base64. Trino sends `varbinary` as base64 on the wire
//!   (see `qh-driver-trino`), so the two must not be confused: the wire encoding is
//!   an implementation of the protocol, this one is what the user sees in a file.
//!
//! # What is deliberately not reproduced
//!
//! Stated here rather than discovered by whoever compares an export:
//!
//! 1. **Float text.** Python's `str(float)` switches to exponent form for large and
//!    small magnitudes (`1e+30`); Rust's `{}` spells those out
//!    (`1000000000000000000000000000000`). Both round-trip, and matching Python
//!    exactly would mean reimplementing its repr algorithm. The divergence is
//!    limited to floats with an exponent.
//! 2. **`INTERVAL`.** The Python engine's rendering could not be established here —
//!    the `trino` client is not installed in this environment, so there was nothing
//!    to ask. [`to_text`] renders the three fields the value actually carries, and
//!    this note is the marker that it is unverified rather than confirmed.
//! 3. **A `json` value nested inside an array or map.** Python's `json.dumps` falls
//!    back to `str()` for an object it cannot serialise, so a `bytes` inside an
//!    array comes out as Python's repr (`b'\\x00\\xff'`). Reproducing a repr is
//!    reproducing an implementation detail, so nested bytes render as hex here,
//!    consistently with the top level.

use serde_json::{Map as JsonMap, Value as Json};

use crate::value::{IntervalValue, Value};

/// The canonical text form of a value, or `None` for a NULL.
///
/// `None` rather than an empty string because each writer decides what a NULL looks
/// like in its own format: a CSV cell is empty, an XML element gets `xsi:nil`, a
/// JSON value is `null`. Collapsing that decision here would take it away from the
/// writers.
pub fn to_text(value: &Value) -> Option<String> {
    Some(match value {
        Value::Null => return None,
        Value::Bool(flag) => if *flag { "true" } else { "false" }.to_owned(),
        Value::Int(number) => number.to_string(),
        Value::UInt(number) => number.to_string(),
        Value::Float(number) => format_float(*number),
        Value::Decimal { unscaled, scale } => format_decimal(*unscaled, *scale),
        Value::Text(text) => text.to_string(),
        Value::Bytes(bytes) => hex_encode(bytes),
        Value::Timestamp {
            micros,
            offset_secs,
        } => format_timestamp(*micros, *offset_secs),
        Value::Date { days } => format_date(*days),
        Value::Time { micros } => format_time(*micros),
        Value::Interval(interval) => format_interval(interval),
        // Kept verbatim: Postgres `jsonb` does not promise key order but `json`
        // does, and re-encoding would throw away the difference the user can see.
        Value::Json(text) => text.to_string(),
        // Python passes anything that is a list, tuple or dict to
        // `json.dumps(default=str, ensure_ascii=False)`, so a nested value is
        // rendered as JSON with Python's own separators.
        Value::Array(items) => json_like(&Value::Array(items.clone())),
        Value::Row(items) => json_like(&Value::Row(items.clone())),
        Value::Map(entries) => json_like(&Value::Map(entries.clone())),
        // A type this crate does not model. The text form is what the driver could
        // render, and it is better than losing the cell; the type name travels with
        // the value so a caller can still see what it was.
        Value::Unknown { text, raw, .. } => match (text, raw) {
            (Some(text), _) => text.to_string(),
            (None, Some(bytes)) => hex_encode(bytes),
            (None, None) => String::new(),
        },
    })
}

/// The JSON-native form of a value.
///
/// Reproduces `writers.py:58`, **including its one lossy step**: a `Decimal`
/// becomes a float, so a 38-digit decimal exported as JSON loses digits. That is
/// the Python engine's behaviour and the blueprint keeps it ("Sama"), so it is
/// preserved rather than quietly improved — an export that changes shape between
/// the two engines is worse than one that is consistently lossy, and the exact
/// text form is still available through the other writers.
pub fn to_json_value(value: &Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Bool(flag) => Json::Bool(*flag),
        Value::Int(number) => Json::from(*number),
        Value::UInt(number) => Json::from(*number),
        // `Number::from_f64` refuses NaN and infinity, yielding `null`. Python's
        // `json.dumps` writes `NaN` and `Infinity` instead, which are not valid JSON
        // and nothing else will read; `null` is the better answer and is not claimed
        // to be a byte-for-byte match.
        Value::Float(number) => Json::from(*number),
        Value::Decimal { unscaled, scale } => match decimal_to_f64(*unscaled, *scale) {
            Some(number) => Json::from(number),
            // A decimal too large for a float: `null` rather than an infinity,
            // because `Infinity` is not JSON that anything else will read.
            None => Json::Null,
        },
        Value::Text(text) => Json::String(text.to_string()),
        Value::Array(items) => Json::Array(items.iter().map(to_json_value).collect()),
        Value::Row(items) => Json::Array(items.iter().map(to_json_value).collect()),
        // `preserve_order` is on in Cargo.toml for this reason: a `serde_json::Map` is
        // a sorted map by default, so a Trino map and an exported row would both come
        // out in alphabetical key order instead of the order the server sent. Python's
        // dicts keep insertion order, and an export that reorders columns is wrong in
        // a way that reads as correct.
        Value::Map(entries) => {
            let mut object = JsonMap::new();
            for (key, entry) in entries {
                // Python stringifies a dict key, and a Trino map arrives with
                // string keys already.
                let key = to_text(key).unwrap_or_default();
                object.insert(key, to_json_value(entry));
            }
            Json::Object(object)
        }
        // Everything else becomes text, which is what `to_text` already decided.
        other => match to_text(other) {
            Some(text) => Json::String(text),
            None => Json::Null,
        },
    }
}

/// A float the way Python's `str()` writes it, for the magnitudes where the two
/// agree.
///
/// Rust's `{}` produces the shortest string that round-trips, which is the same
/// guarantee Python's repr gives; what differs is when either switches to exponent
/// form. See the module note: the divergence is recorded, not hidden.
pub fn format_float(number: f64) -> String {
    if number.is_nan() {
        return "nan".to_owned();
    }
    if number.is_infinite() {
        return if number > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    let mut text = format!("{number}");
    // Rust writes a whole number as `1`, Python as `1.0`. A reader comparing the
    // two exports sees that immediately, so it is matched here.
    if !text.contains('.') && !text.contains('e') && !text.contains('E') {
        text.push_str(".0");
    }
    text
}

/// `unscaled / 10^scale` as text, with the scale's trailing zeros kept.
///
/// `1.50` is not `1.5` in an exact type, and Python's `str(Decimal("1.50"))` says
/// so.
pub fn format_decimal(unscaled: i128, scale: u8) -> String {
    if scale == 0 {
        return unscaled.to_string();
    }
    let negative = unscaled < 0;
    let digits = unscaled.unsigned_abs().to_string();
    let scale = usize::from(scale);
    let padded = if digits.len() <= scale {
        format!("{}{}", "0".repeat(scale - digits.len() + 1), digits)
    } else {
        digits
    };
    let split = padded.len() - scale;
    format!(
        "{}{}.{}",
        if negative { "-" } else { "" },
        &padded[..split],
        &padded[split..]
    )
}

/// The nearest float, or `None` when the value cannot be one.
fn decimal_to_f64(unscaled: i128, scale: u8) -> Option<f64> {
    let divisor = 10f64.powi(i32::from(scale));
    let number = unscaled as f64 / divisor;
    number.is_finite().then_some(number)
}

/// Days since the epoch as `YYYY-MM-DD`.
pub fn format_date(days: i32) -> String {
    let (year, month, day) = civil_from_days(i64::from(days));
    format!("{year:04}-{month:02}-{day:02}")
}

/// Microseconds since midnight as `HH:MM:SS`, with a fraction only when there is
/// one — Python omits `.000000` entirely.
pub fn format_time(micros: i64) -> String {
    let negative = micros < 0;
    let micros = micros.unsigned_abs();
    let seconds_of_day = micros / 1_000_000;
    let fraction = micros % 1_000_000;

    let hours = seconds_of_day / 3600;
    let minutes = (seconds_of_day % 3600) / 60;
    let seconds = seconds_of_day % 60;

    let sign = if negative { "-" } else { "" };
    let mut text = format!("{sign}{hours:02}:{minutes:02}:{seconds:02}");
    if fraction != 0 {
        text.push_str(&format!(".{fraction:06}"));
    }
    text
}

/// A point in time as `YYYY-MM-DD HH:MM:SS[.ffffff][±HH:MM]`.
///
/// The offset is rendered from what the server reported rather than normalising to
/// UTC first: normalising is what loses the `+07:00` the user asked the database
/// for.
pub fn format_timestamp(micros: i64, offset_secs: Option<i32>) -> String {
    // Floored division, so a value before the epoch lands on the right day rather
    // than one day later with a positive remainder.
    let days = micros.div_euclid(86_400_000_000);
    let within_day = micros.rem_euclid(86_400_000_000);

    let mut text = format!("{} {}", format_date(days as i32), format_time(within_day));
    if let Some(offset) = offset_secs {
        let sign = if offset < 0 { '-' } else { '+' };
        let offset = offset.unsigned_abs();
        text.push_str(&format!(
            "{sign}{:02}:{:02}",
            offset / 3600,
            (offset % 3600) / 60
        ));
    }
    text
}

/// The three fields an interval carries.
///
/// Unverified against the Python engine — see the module note. Rendered as
/// `<months>-<days> <H:MM:SS>` only because that is a shape a reader can act on;
/// it is not claimed to be what the Python engine wrote.
pub fn format_interval(interval: &IntervalValue) -> String {
    let sign = if interval.micros < 0 { "-" } else { "" };
    let micros = interval.micros.unsigned_abs();
    let seconds = micros / 1_000_000;
    format!(
        "{}{}-{} {}{}:{:02}:{:02}",
        sign,
        interval.months,
        interval.days,
        if interval.months < 0 || interval.days < 0 {
            "-"
        } else {
            ""
        },
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

/// Lowercase hex, which is what Python's `bytes.hex()` produces.
pub fn hex_encode(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// The civil date for a count of days since the Unix epoch.
///
/// Howard Hinnant's `civil_from_days`, the inverse of the `days_from_civil` the
/// Trino driver uses. Exact for every date this type can carry, with no leap-year
/// table to get wrong.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}

/// Render a structured value the way Python's `json.dumps(default=str,
/// ensure_ascii=False)` would.
///
/// Python's default separators are `", "` and `": "`, and `ensure_ascii=False`
/// leaves non-ASCII characters alone rather than escaping them. Both show up in
/// exported files, so both are matched.
fn json_like(value: &Value) -> String {
    match value {
        Value::Array(items) | Value::Row(items) => {
            let rendered: Vec<String> = items.iter().map(json_like).collect();
            format!("[{}]", rendered.join(", "))
        }
        Value::Map(entries) => {
            let rendered: Vec<String> = entries
                .iter()
                .map(|(key, entry)| {
                    let key = to_text(key).unwrap_or_default();
                    format!("{}: {}", json_string(&key), json_like(entry))
                })
                .collect();
            format!("{{{}}}", rendered.join(", "))
        }
        Value::Null => "null".to_owned(),
        // JSON's own scalars are written as scalars. This is not a detail: quoted,
        // `[1, 2]` becomes `["1", "2"]`, and a downstream reader then has strings
        // where the query returned numbers.
        Value::Bool(flag) => if *flag { "true" } else { "false" }.to_owned(),
        Value::Int(number) => number.to_string(),
        Value::UInt(number) => number.to_string(),
        Value::Float(number) => format_float(*number),
        // Everything else reaches Python's `default=str` and arrives as a quoted
        // string, a Decimal included: `json` has no exact-decimal scalar, and Python
        // would raise rather than guess.
        other => match to_text(other) {
            Some(text) => json_string(&text),
            None => "null".to_owned(),
        },
    }
}

/// A JSON string literal with Python's `ensure_ascii=False` escaping.
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
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            // Everything else, non-ASCII included, is written as itself.
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_null_has_no_text_form_because_the_writers_decide_what_it_looks_like() {
        // Not an empty string: a CSV cell and an XML element need different things,
        // and collapsing that here would take the decision from the writers.
        assert_eq!(to_text(&Value::Null), None);
        assert_eq!(to_json_value(&Value::Null), Json::Null);
    }

    #[test]
    fn the_boolean_and_text_forms_match_python() {
        assert_eq!(to_text(&Value::Bool(true)).as_deref(), Some("true"));
        assert_eq!(to_text(&Value::Bool(false)).as_deref(), Some("false"));
        assert_eq!(to_text(&Value::Text("x".into())).as_deref(), Some("x"));
        // Non-ASCII is not escaped on this path: it is the value's own text.
        assert_eq!(
            to_text(&Value::Text("café".into())).as_deref(),
            Some("café")
        );
    }

    #[test]
    fn bytes_render_as_hex_and_not_as_base64() {
        // The distinction that matters: Trino puts base64 on the wire, and the
        // Python engine's canonical text form is hex (`bytes.hex()`). Confusing them
        // produces a file that looks plausible and is wrong.
        assert_eq!(
            to_text(&Value::Bytes(vec![0x00, 0xff])).as_deref(),
            Some("00ff")
        );
        assert_eq!(
            to_text(&Value::Bytes(b"hi".to_vec())).as_deref(),
            Some("6869")
        );
        assert_eq!(to_text(&Value::Bytes(Vec::new())).as_deref(), Some(""));
    }

    #[test]
    fn a_timestamp_fraction_is_omitted_when_it_is_zero() {
        // Python's isoformat omits `.000000` entirely, so `12:00:00` is right and
        // `12:00:00.000000` is not. A whole-second timestamp is the case that shows
        // whether the fraction is conditional.
        let midnight = 0;
        assert_eq!(
            to_text(&Value::Timestamp {
                micros: midnight,
                offset_secs: None
            })
            .as_deref(),
            Some("1970-01-01 00:00:00")
        );
        assert_eq!(
            to_text(&Value::Timestamp {
                micros: 123_456,
                offset_secs: None
            })
            .as_deref(),
            Some("1970-01-01 00:00:00.123456")
        );
        // And a fraction of one microsecond still gets its six digits.
        assert_eq!(
            to_text(&Value::Timestamp {
                micros: 1,
                offset_secs: None
            })
            .as_deref(),
            Some("1970-01-01 00:00:00.000001")
        );
    }

    #[test]
    fn an_offset_is_rendered_from_what_the_server_reported() {
        let micros = 1_769_860_800_123_456;
        assert_eq!(
            to_text(&Value::Timestamp {
                micros,
                offset_secs: Some(0)
            })
            .as_deref(),
            Some("2026-01-31 12:00:00.123456+00:00")
        );
        assert_eq!(
            to_text(&Value::Timestamp {
                micros,
                offset_secs: Some(25_200)
            })
            .as_deref(),
            Some("2026-01-31 12:00:00.123456+07:00")
        );
        assert_eq!(
            to_text(&Value::Timestamp {
                micros,
                offset_secs: Some(-18_000)
            })
            .as_deref(),
            Some("2026-01-31 12:00:00.123456-05:00")
        );
        // No zone, no suffix: a `TIMESTAMP` must not acquire a `+00:00` it was never
        // given.
        assert_eq!(
            to_text(&Value::Timestamp {
                micros,
                offset_secs: None
            })
            .as_deref(),
            Some("2026-01-31 12:00:00.123456")
        );
    }

    #[test]
    fn dates_and_times_survive_their_edges() {
        assert_eq!(
            to_text(&Value::Date { days: 0 }).as_deref(),
            Some("1970-01-01")
        );
        assert_eq!(
            to_text(&Value::Date { days: -1 }).as_deref(),
            Some("1969-12-31")
        );
        assert_eq!(
            to_text(&Value::Date { days: 20_485 }).as_deref(),
            Some("2026-02-01")
        );
        // A leap day, which a table-free implementation must still get right.
        assert_eq!(
            to_text(&Value::Date { days: 19_782 }).as_deref(),
            Some("2024-02-29")
        );
        assert_eq!(
            to_text(&Value::Time { micros: 0 }).as_deref(),
            Some("00:00:00")
        );
        assert_eq!(
            to_text(&Value::Time {
                micros: 43_200_123_456
            })
            .as_deref(),
            Some("12:00:00.123456")
        );
    }

    #[test]
    fn a_timestamp_before_the_epoch_keeps_its_day() {
        // The case a truncating division gets wrong: `micros` of -1 belongs to
        // 1969-12-31 23:59:59.999999, not to the epoch with a positive remainder.
        assert_eq!(
            to_text(&Value::Timestamp {
                micros: -1,
                offset_secs: None
            })
            .as_deref(),
            Some("1969-12-31 23:59:59.999999")
        );
    }

    #[test]
    fn a_decimal_keeps_its_scale_and_its_sign() {
        assert_eq!(format_decimal(150, 2), "1.50");
        assert_eq!(format_decimal(-125, 2), "-1.25");
        assert_eq!(format_decimal(100, 0), "100");
        assert_eq!(format_decimal(1, 6), "0.000001");
        assert_eq!(format_decimal(0, 2), "0.00");
        assert_eq!(
            format_decimal(12_345_678_901_234_567_890_123_456_781_234_567_890, 10),
            "1234567890123456789012345678.1234567890"
        );
    }

    #[test]
    fn a_whole_float_keeps_its_point_because_python_writes_one() {
        // Python writes `1.0`; Rust's formatter writes `1`. A reader comparing two
        // exports notices that immediately.
        assert_eq!(format_float(1.0), "1.0");
        assert_eq!(format_float(1.5), "1.5");
        assert_eq!(format_float(-0.25), "-0.25");
        assert_eq!(format_float(f64::INFINITY), "inf");
        assert_eq!(format_float(f64::NAN), "nan");
    }

    #[test]
    fn a_nested_value_is_rendered_the_way_python_would() {
        // Python hands a list to `json.dumps(default=str, ensure_ascii=False)`, whose
        // separators are ", " and ": ". That is why `[1, 2]` has a space in it.
        assert_eq!(
            to_text(&Value::Array(vec![Value::Int(1), Value::Int(2)])).as_deref(),
            Some("[1, 2]")
        );
        assert_eq!(
            to_text(&Value::Row(vec![Value::Int(1), Value::Text("a".into())])).as_deref(),
            Some("[1, \"a\"]")
        );
        assert_eq!(
            to_text(&Value::Map(vec![(Value::Text("a".into()), Value::Int(1))])).as_deref(),
            Some("{\"a\": 1}")
        );
        // `ensure_ascii=False`: a non-ASCII character is written as itself.
        assert_eq!(
            to_text(&Value::Array(vec![Value::Text("café".into())])).as_deref(),
            Some("[\"café\"]")
        );
        // A quote inside a string is escaped, and a control character is spelled out.
        assert_eq!(
            to_text(&Value::Array(vec![Value::Text("a\"b".into())])).as_deref(),
            Some("[\"a\\\"b\"]")
        );
        assert_eq!(
            to_text(&Value::Array(vec![Value::Text("a\nb".into())])).as_deref(),
            Some("[\"a\\nb\"]")
        );
    }

    #[test]
    fn json_native_form_is_the_one_place_a_decimal_loses_precision() {
        // Preserved from `writers.py:58` on purpose: the blueprint keeps it ("Sama"),
        // and an export that changes shape between the two engines would be worse
        // than one that is consistently lossy. The exact text form is still available
        // from `to_text` and from every non-JSON writer.
        let lossy = to_json_value(&Value::Decimal {
            unscaled: 150,
            scale: 2,
        });
        assert_eq!(lossy, Json::from(1.5));

        // Numbers stay numbers, which is the point of the JSON-native form: a grid
        // or a downstream tool gets a number and not a quoted string.
        assert_eq!(to_json_value(&Value::Int(42)), Json::from(42));
        assert_eq!(to_json_value(&Value::Bool(true)), Json::Bool(true));
        assert_eq!(to_json_value(&Value::Float(1.5)), Json::from(1.5));

        // A map becomes an object with string keys, as Python's dict does.
        let object = to_json_value(&Value::Map(vec![
            (Value::Int(1), Value::Text("a".into())),
            (Value::Text("k".into()), Value::Int(2)),
        ]));
        assert_eq!(object["1"], Json::String("a".to_owned()));
        assert_eq!(object["k"], Json::from(2));
    }

    #[test]
    fn a_type_this_crate_does_not_model_still_renders() {
        assert_eq!(
            to_text(&Value::unknown("geometry", "POINT (30 10)")).as_deref(),
            Some("POINT (30 10)")
        );
        // With no text form, the raw bytes are the honest answer.
        assert_eq!(
            to_text(&Value::unknown_bytes("geometry", vec![0x00, 0xff])).as_deref(),
            Some("00ff")
        );
    }
}
