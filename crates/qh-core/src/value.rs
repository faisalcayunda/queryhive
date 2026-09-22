//! The canonical value a driver hands upward, and its text rendering.
//!
//! [`Value::render_text`] is the successor to the Python engine's `to_text`
//! (`exporter/writers.py:38`), and it exists because the golden snapshots in
//! `tests/golden/` pin what that function produced. Every rule below is either a
//! deliberate match or a deliberate, documented difference — see
//! `docs/golden-deltas.md` for the two differences that are already known.
//!
//! The rules `to_text` encoded, all preserved, and implemented in
//! [`crate::render`]:
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
    ///
    /// A method rather than a free function because every caller has a `Value` in
    /// hand, and a method reads as the value's own text. The rules themselves live in
    /// [`crate::render::to_text`], which is the only implementation of them: this used
    /// to carry a second copy, and the two had already drifted over what a
    /// `timestamptz`'s `micros` means (T-1 in `docs/golden-deltas.md`).
    pub fn render_text(&self) -> Option<String> {
        crate::render::to_text(self)
    }
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
