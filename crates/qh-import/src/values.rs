//! Reading the values a file spells in its own locale, and handing the target the one
//! spelling it cannot misread.
//!
//! Two things go wrong silently when text is pasted into an `INSERT`: a date such as
//! `03/04/2024` is read by the server's own `DateStyle` (PostgreSQL's default, `ISO, MDY`,
//! turns it into 4 March), and a number such as `1.500,00` is either refused or, worse,
//! read as 1.5. Both are decided here, by the caller's declared format, and emitted as ISO
//! dates and plain decimal numbers. Neither is ever guessed: with no format declared the
//! text goes through untouched, and a value that does not fit the declared format is an
//! error the caller turns into a rejected row.
//!
//! # `DATE_FORMAT`
//!
//! A pattern in the Unicode / Java spelling the app's import sheet shows:
//!
//! | token | meaning |
//! |---|---|
//! | `yyyy`, `yy` | year; two digits read 00-68 as 2000-2068 and 69-99 as 1969-1999 |
//! | `MM`, `M` | month, 1-12 |
//! | `dd`, `d` | day of month, validated against the month and the leap year |
//! | `HH`, `H` | hour 0-23 |
//! | `hh`, `h` + `a` | hour 1-12 and `AM`/`PM` |
//! | `mm`, `ss` | minute, second |
//! | `S`..`SSSSSSSSS` | a fraction of a second; any 1 to 9 digits are read |
//! | `'text'` | literal text, so `yyyy-MM-dd'T'HH:mm:ss` works |
//!
//! Every other letter is an error when the pattern is parsed, not a literal. The pattern
//! must name a year, a month and a day. A doubled token (`MM`, `dd`, `HH`...) takes exactly
//! two digits and a single one (`M`, `d`, `H`) takes one or two, so `3/4/2024` needs `d/M/yyyy`.

use std::fmt;

/// One piece of a compiled pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Year4,
    Year2,
    /// `(width, ..)`: 2 for the doubled token, 1 for "one or two digits".
    Month(u8),
    Day(u8),
    Hour24(u8),
    Hour12(u8),
    Minute,
    Second,
    Fraction,
    AmPm,
    Literal(String),
}

/// A compiled `DATE_FORMAT`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatePattern {
    tokens: Vec<Token>,
    source: String,
}

/// Whether literal pattern text spells a time zone: `Z`, `UTC`, `GMT`, or a sign followed by
/// digits (`+07:00`). The pattern would match it and then discard it, and the server would
/// read the zoneless timestamp in its own session zone: a wrong value with no error.
fn carries_zone(text: &str) -> bool {
    let upper = text.to_ascii_uppercase();
    if upper.contains('Z') || upper.contains("UTC") || upper.contains("GMT") {
        return true;
    }
    let chars: Vec<char> = text.chars().collect();
    chars
        .windows(2)
        .any(|pair| matches!(pair[0], '+' | '-') && pair[1].is_ascii_digit())
}

/// A date (and perhaps a time) read through a [`DatePattern`], as ISO text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IsoDateTime {
    date: String,
    time: Option<String>,
}

impl IsoDateTime {
    /// `YYYY-MM-DD`, for a `date` column.
    pub fn date(&self) -> &str {
        &self.date
    }

    /// `YYYY-MM-DD HH:MM:SS[.fraction]`, for a timestamp column. A pattern with no time
    /// reads as midnight.
    pub fn timestamp(&self) -> String {
        format!(
            "{} {}",
            self.date,
            self.time.as_deref().unwrap_or("00:00:00")
        )
    }
}

/// Why a value or a pattern was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueError(String);

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ValueError {}

fn refuse<T>(message: impl Into<String>) -> Result<T, ValueError> {
    Err(ValueError(message.into()))
}

impl DatePattern {
    /// The pattern as the caller wrote it, for messages.
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Compile `pattern`, or say which letter or rule it breaks.
    pub fn parse(pattern: &str) -> Result<Self, ValueError> {
        let chars: Vec<char> = pattern.chars().collect();
        let mut tokens = Vec::new();
        let mut index = 0;
        while index < chars.len() {
            let c = chars[index];
            if c == '\'' {
                // A quoted run is literal; `''` is one quote.
                let mut text = String::new();
                index += 1;
                loop {
                    match chars.get(index) {
                        None => {
                            return refuse(format!("DATE_FORMAT '{pattern}' has an unclosed '"))
                        }
                        Some('\'') if chars.get(index + 1) == Some(&'\'') => {
                            text.push('\'');
                            index += 2;
                        }
                        Some('\'') => {
                            index += 1;
                            break;
                        }
                        Some(other) => {
                            text.push(*other);
                            index += 1;
                        }
                    }
                }
                if text.is_empty() {
                    text.push('\'');
                }
                tokens.push(Token::Literal(text));
            } else if c.is_ascii_alphabetic() {
                let run = chars[index..].iter().take_while(|next| **next == c).count();
                let token = match (c, run) {
                    ('y', 4) => Token::Year4,
                    ('y', 2) => Token::Year2,
                    ('M', 1 | 2) => Token::Month(run as u8),
                    ('d', 1 | 2) => Token::Day(run as u8),
                    ('H', 1 | 2) => Token::Hour24(run as u8),
                    ('h', 1 | 2) => Token::Hour12(run as u8),
                    ('m', 2) => Token::Minute,
                    ('s', 2) => Token::Second,
                    ('S', 1..=9) => Token::Fraction,
                    ('a', 1) => Token::AmPm,
                    _ => {
                        return refuse(format!(
                            "DATE_FORMAT '{pattern}': '{}' is not a pattern letter this import \
                             reads (use yyyy, yy, MM, dd, HH, hh a, mm, ss, S, and 'quoted' text)",
                            c.to_string().repeat(run)
                        ))
                    }
                };
                tokens.push(token);
                index += run;
            } else {
                match tokens.last_mut() {
                    Some(Token::Literal(text)) => text.push(c),
                    _ => tokens.push(Token::Literal(c.to_string())),
                }
                index += 1;
            }
        }
        for token in &tokens {
            if let Token::Literal(text) = token {
                if carries_zone(text) {
                    return refuse(format!(
                        "DATE_FORMAT '{pattern}': '{text}' is a time zone, and this pattern has no \
                         zone token, so it would be read and dropped. Leave the zone in the value \
                         and unset DATE_FORMAT, or convert the file to one zone first"
                    ));
                }
            }
        }
        let has = |wanted: fn(&Token) -> bool| tokens.iter().any(wanted);
        if !(has(|t| matches!(t, Token::Year4 | Token::Year2))
            && has(|t| matches!(t, Token::Month(_)))
            && has(|t| matches!(t, Token::Day(_))))
        {
            return refuse(format!(
                "DATE_FORMAT '{pattern}' must name a year, a month and a day"
            ));
        }
        if has(|t| matches!(t, Token::Hour12(_))) != has(|t| matches!(t, Token::AmPm)) {
            return refuse(format!(
                "DATE_FORMAT '{pattern}': a 12-hour clock needs both hh and a"
            ));
        }
        Ok(Self {
            tokens,
            source: pattern.to_owned(),
        })
    }

    /// Read `text` through the pattern. `Err` names the value and the pattern.
    pub fn read(&self, text: &str) -> Result<IsoDateTime, ValueError> {
        let bad = |why: &str| {
            ValueError(format!(
                "'{text}' is not a valid date for DATE_FORMAT {} ({why})",
                self.source
            ))
        };
        let bytes = text.trim().as_bytes();
        let mut at = 0;
        let (mut year, mut month, mut day) = (None, None, None);
        let (mut hour, mut minute, mut second) = (None, 0u32, 0u32);
        let mut fraction: Option<&str> = None;
        let mut pm = None;

        for token in &self.tokens {
            // The digits a numeric token takes: exactly `exact`, or one or two.
            let digits = |at: &mut usize, min: usize, max: usize| -> Option<u32> {
                let run = bytes[*at..]
                    .iter()
                    .take(max)
                    .take_while(|b| b.is_ascii_digit())
                    .count();
                if run < min {
                    return None;
                }
                let value = std::str::from_utf8(&bytes[*at..*at + run])
                    .ok()?
                    .parse()
                    .ok()?;
                *at += run;
                Some(value)
            };
            match token {
                Token::Year4 => year = Some(digits(&mut at, 4, 4).ok_or_else(|| bad("year"))?),
                Token::Year2 => {
                    let two = digits(&mut at, 2, 2).ok_or_else(|| bad("year"))?;
                    year = Some(if two <= 68 { 2000 + two } else { 1900 + two });
                }
                Token::Month(width) => {
                    let min = usize::from(*width);
                    month = Some(digits(&mut at, min.min(2), 2).ok_or_else(|| bad("month"))?)
                }
                Token::Day(width) => {
                    let min = usize::from(*width);
                    day = Some(digits(&mut at, min.min(2), 2).ok_or_else(|| bad("day"))?)
                }
                Token::Hour24(width) | Token::Hour12(width) => {
                    let min = usize::from(*width);
                    hour = Some(digits(&mut at, min.min(2), 2).ok_or_else(|| bad("hour"))?)
                }
                Token::Minute => minute = digits(&mut at, 2, 2).ok_or_else(|| bad("minute"))?,
                Token::Second => second = digits(&mut at, 2, 2).ok_or_else(|| bad("second"))?,
                Token::Fraction => {
                    let run = bytes[at..]
                        .iter()
                        .take(9)
                        .take_while(|b| b.is_ascii_digit())
                        .count();
                    if run == 0 {
                        return Err(bad("fraction of a second"));
                    }
                    fraction = Some(&text.trim()[at..at + run]);
                    at += run;
                }
                Token::AmPm => {
                    let word = text.trim().get(at..at + 2).ok_or_else(|| bad("AM or PM"))?;
                    pm = match word.to_ascii_uppercase().as_str() {
                        "AM" => Some(false),
                        "PM" => Some(true),
                        _ => return Err(bad("AM or PM")),
                    };
                    at += 2;
                }
                Token::Literal(literal) => {
                    if !text
                        .trim()
                        .get(at..)
                        .is_some_and(|rest| rest.starts_with(literal))
                    {
                        return Err(bad(&format!("expected '{literal}'")));
                    }
                    at += literal.len();
                }
            }
        }
        if at != bytes.len() {
            return Err(bad("unexpected text after the date"));
        }

        let (year, month, day) = (
            year.expect("pattern names a year"),
            month.expect("pattern names a month"),
            day.expect("pattern names a day"),
        );
        if !(1..=12).contains(&month) {
            return Err(bad("month is not 1-12"));
        }
        if day == 0 || day > days_in_month(year, month) {
            return Err(bad("no such day in that month"));
        }
        let hour = match (hour, pm) {
            (Some(h), Some(pm)) => {
                if !(1..=12).contains(&h) {
                    return Err(bad("hour is not 1-12"));
                }
                Some(h % 12 + if pm { 12 } else { 0 })
            }
            (hour, _) => hour,
        };
        if hour.is_some_and(|h| h > 23) || minute > 59 || second > 59 {
            return Err(bad("time of day out of range"));
        }
        let time = hour.map(|h| match fraction {
            Some(digits) => format!("{h:02}:{minute:02}:{second:02}.{digits}"),
            None => format!("{h:02}:{minute:02}:{second:02}"),
        });
        Ok(IsoDateTime {
            date: format!("{year:04}-{month:02}-{day:02}"),
            time,
        })
    }
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 31,
    }
}

/// Read a number written with `decimal` as its decimal separator and, when `grouping` is
/// set, that character between thousands, and return it as plain `-1234.5` text.
///
/// Strict on purpose. A grouping separator is only accepted where a person would put it
/// (1-3 digits, then groups of exactly three), so `1.5` under `1.500,00`'s rules is refused
/// rather than read as 1500 or 1.5. No exponent, no currency sign, no percent: those are
/// text, and the server decides what to do with text.
pub fn normalize_number(text: &str, decimal: char, grouping: Option<char>) -> Option<String> {
    let text = text.trim();
    let (sign, body) = match text.chars().next()? {
        sign @ ('+' | '-') => (Some(sign), &text[1..]),
        _ => (None, text),
    };
    let (whole, fraction) = match body.split_once(decimal) {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (body, None),
    };
    if fraction.is_some_and(|f| f.is_empty() || !f.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    let digits: String = match grouping {
        Some(group) if whole.contains(group) => {
            let mut groups = whole.split(group);
            let first = groups.next()?;
            if !(1..=3).contains(&first.len()) {
                return None;
            }
            let rest: Vec<&str> = groups.collect();
            if rest.iter().any(|g| g.len() != 3) {
                return None;
            }
            std::iter::once(first).chain(rest).collect()
        }
        _ => whole.to_owned(),
    };
    if !digits.chars().all(|c| c.is_ascii_digit()) || (digits.is_empty() && fraction.is_none()) {
        return None;
    }
    let mut out = String::new();
    out.extend(sign);
    out.push_str(if digits.is_empty() { "0" } else { &digits });
    if let Some(fraction) = fraction {
        out.push('.');
        out.push_str(fraction);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(pattern: &str, text: &str) -> Result<IsoDateTime, ValueError> {
        DatePattern::parse(pattern).expect("pattern").read(text)
    }

    #[test]
    fn day_first_is_read_as_day_first_not_as_the_servers_month_first() {
        assert_eq!(
            read("dd/MM/yyyy", "03/04/2024").unwrap().date(),
            "2024-04-03"
        );
        assert_eq!(
            read("MM/dd/yyyy", "03/04/2024").unwrap().date(),
            "2024-03-04"
        );
        assert_eq!(read("d/M/yyyy", "3/4/2024").unwrap().date(), "2024-04-03");
    }

    #[test]
    fn a_zone_in_the_pattern_is_refused_not_matched_and_dropped() {
        // Each of these would match `2024-03-04T10:00:00Z` and emit a zoneless timestamp.
        for pattern in [
            "yyyy-MM-dd'T'HH:mm:ss'Z'",
            "yyyy-MM-dd HH:mm:ss+07:00",
            "yyyy-MM-dd HH:mm:ss-05:00",
            "yyyy-MM-dd HH:mm:ss' UTC'",
            "yyyy-MM-dd HH:mm:ss 'GMT'",
        ] {
            let error = DatePattern::parse(pattern).expect_err(pattern).to_string();
            assert!(error.contains("time zone"), "{pattern}: {error}");
        }
        // A separator is not a zone, and neither is the `T` between date and time.
        assert!(DatePattern::parse("yyyy-MM-dd'T'HH:mm:ss.SSS").is_ok());
        assert!(DatePattern::parse("dd-MM-yyyy HH:mm").is_ok());
    }

    #[test]
    fn a_time_and_a_fraction_come_out_as_iso() {
        let read = read("dd-MM-yyyy HH:mm:ss.SSS", "31-01-2026 07:05:09.250").unwrap();
        assert_eq!(read.timestamp(), "2026-01-31 07:05:09.250");
        assert_eq!(read.date(), "2026-01-31");
    }

    #[test]
    fn a_date_only_pattern_reads_as_midnight_in_a_timestamp_column() {
        assert_eq!(
            read("dd/MM/yyyy", "01/02/2026").unwrap().timestamp(),
            "2026-02-01 00:00:00"
        );
    }

    #[test]
    fn a_twelve_hour_clock_needs_its_marker() {
        assert_eq!(
            read("MM/dd/yyyy hh:mm a", "12/31/2025 12:30 AM")
                .unwrap()
                .timestamp(),
            "2025-12-31 00:30:00"
        );
        assert_eq!(
            read("MM/dd/yyyy hh:mm a", "12/31/2025 01:30 pm")
                .unwrap()
                .timestamp(),
            "2025-12-31 13:30:00"
        );
        assert!(DatePattern::parse("hh:mm yyyy-MM-dd").is_err());
    }

    #[test]
    fn quoted_text_is_literal_and_two_digit_years_pivot() {
        assert_eq!(
            read("yyyy-MM-dd'T'HH:mm", "2026-01-31T08:00")
                .unwrap()
                .timestamp(),
            "2026-01-31 08:00:00"
        );
        assert_eq!(read("dd/MM/yy", "01/02/68").unwrap().date(), "2068-02-01");
        assert_eq!(read("dd/MM/yy", "01/02/69").unwrap().date(), "1969-02-01");
    }

    #[test]
    fn an_impossible_date_is_refused_not_rolled_over() {
        for text in ["31/02/2024", "29/02/2023", "00/01/2024", "01/13/2024"] {
            assert!(read("dd/MM/yyyy", text).is_err(), "{text}");
        }
        assert_eq!(
            read("dd/MM/yyyy", "29/02/2024").unwrap().date(),
            "2024-02-29"
        );
        assert_eq!(
            read("dd/MM/yyyy", "29/02/2000").unwrap().date(),
            "2000-02-29"
        );
        assert!(read("dd/MM/yyyy", "29/02/1900").is_err());
    }

    #[test]
    fn text_that_does_not_fit_is_refused_and_named() {
        for text in ["2024-04-03", "3/4/2024", "03/04/2024 extra", "", "03/04/24"] {
            let error = read("dd/MM/yyyy", text).unwrap_err().to_string();
            assert!(error.contains("DATE_FORMAT dd/MM/yyyy"), "{error}");
        }
    }

    #[test]
    fn a_bad_pattern_is_refused_when_it_is_parsed() {
        for pattern in [
            "dd/MM",
            "yyyy-MM",
            "dd/MM/yyyy EEE",
            "dd/MM/yyy",
            "'dd/MM/yyyy",
        ] {
            assert!(DatePattern::parse(pattern).is_err(), "{pattern}");
        }
    }

    #[test]
    fn indonesian_numbers_read_as_plain_decimals() {
        let id = |text: &str| normalize_number(text, ',', Some('.'));
        assert_eq!(id("1.500,00").as_deref(), Some("1500.00"));
        assert_eq!(id("-1.234.567,5").as_deref(), Some("-1234567.5"));
        assert_eq!(id("1500").as_deref(), Some("1500"));
        assert_eq!(id("0,5").as_deref(), Some("0.5"));
        assert_eq!(id(",5").as_deref(), Some("0.5"));
        assert_eq!(id(" +12 ").as_deref(), Some("+12"));
    }

    #[test]
    fn a_separator_in_the_wrong_place_is_not_a_number() {
        let id = |text: &str| normalize_number(text, ',', Some('.'));
        for text in [
            "1.5", "12.34", "1.5000", "1,5,5", "1.500,", "abc", "", "-", "1e5", "Rp 1.500",
        ] {
            assert_eq!(id(text), None, "{text}");
        }
    }

    #[test]
    fn without_grouping_a_stray_separator_is_refused() {
        assert_eq!(normalize_number("1.500", ',', None), None);
        assert_eq!(
            normalize_number("1500,25", ',', None).as_deref(),
            Some("1500.25")
        );
        assert_eq!(
            normalize_number("1 500", ',', Some(' ')).as_deref(),
            Some("1500")
        );
        assert_eq!(
            normalize_number("1,500.25", '.', Some(',')).as_deref(),
            Some("1500.25")
        );
    }
}
