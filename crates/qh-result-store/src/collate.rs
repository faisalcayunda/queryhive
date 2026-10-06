//! Grid collation: the Swift ports and the memcmp-able natural key.
//!
//! Sort, filter, and search semantics are ports of the Swift grid code,
//! not SQL: NULLs sort last ascending, numbers compare by exact decimal
//! value, and text orders by a natural key shared with the `qh_natural`
//! SQL function, so grid and SQL cannot disagree.
//!
//! The key layout is `primary 0x00 secondary 0x00 tertiary 0x00
//! quaternary 0x00 raw-bytes`, compared with `memcmp` and then the source
//! row index.

use std::cmp::Ordering;

use unicode_normalization::UnicodeNormalization;
use unicode_properties::UnicodeGeneralCategory;

/// An exact decimal sort key: sign, adjusted exponent, then significant
/// digits. `-0 == 0` by construction, and floats enter through their
/// shortest text, so float order matches `f64` order and agrees with
/// decimals. 38 significant digits, mirroring Swift `Decimal`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumKey {
    neg: bool,
    adj_exp: i32,
    digits: Vec<u8>,
}

impl NumKey {
    /// -1, 0 or 1. Zero sits between the negatives and the positives.
    fn sign(&self) -> i8 {
        match (self.digits.is_empty(), self.neg) {
            (true, _) => 0,
            (false, true) => -1,
            (false, false) => 1,
        }
    }

    /// Decimal position of the leading digit: `digits * 10^adj_exp` is below
    /// `10^magnitude`. Only meaningful for a non-zero key.
    fn magnitude(&self) -> i64 {
        i64::from(self.adj_exp) + self.digits.len() as i64
    }
}

impl PartialOrd for NumKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for NumKey {
    fn cmp(&self, other: &Self) -> Ordering {
        let sign = self.sign().cmp(&other.sign());
        if sign != Ordering::Equal || self.sign() == 0 {
            return sign;
        }
        // Digits carry no leading or trailing zeros, so the magnitude decides
        // first and a lexicographic digit compare (shorter prefix is smaller)
        // decides the rest.
        let order = self
            .magnitude()
            .cmp(&other.magnitude())
            .then_with(|| self.digits.cmp(&other.digits));
        if self.neg {
            order.reverse()
        } else {
            order
        }
    }
}

/// Port of `GridSort.number`: the exact decimal key for a plain number, or
/// `None`. Trim is Zs plus TAB only (no newlines); at most one dot; the
/// exponent must fit Swift `Decimal` (38 significant digits, adjusted
/// exponent within `[-166, 128]`).
pub fn swift_plain_number(text: &str) -> Option<NumKey> {
    let body = trim_zs_tab(text);
    if body.is_empty() {
        return None;
    }
    let (neg, rest) = match body.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, body.strip_prefix('+').unwrap_or(body)),
    };
    let (mantissa, exp_text) = match rest.find(['e', 'E']) {
        Some(index) => (&rest[..index], Some(&rest[index + 1..])),
        None => (rest, None),
    };
    let exp_given: i64 = match exp_text {
        None => 0,
        Some(text) => {
            let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            // Parse as i64: the digits may exceed i32, and the adjustment
            // below is done in i64 so nothing overflows.
            text.parse::<i64>().ok()?
        }
    };
    let mut parts = mantissa.split('.');
    let int = parts.next().unwrap_or("");
    let frac = parts.next().unwrap_or("");
    if parts.next().is_some() {
        return None;
    }
    if !int.bytes().all(|b| b.is_ascii_digit()) || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let total_digits = int.len() + frac.len();
    if total_digits == 0 || total_digits > 38 {
        return None;
    }
    let mut digits: Vec<u8> = int.bytes().chain(frac.bytes()).collect();
    // Strip leading zeros; they carry no magnitude, so the exponent is untouched.
    let leading = digits.iter().take_while(|&&b| b == b'0').count();
    if leading > 0 {
        digits.drain(..leading);
    }
    // Strip trailing zeros, adding to the exponent for each removed.
    let trailing = digits.iter().rev().take_while(|&&b| b == b'0').count();
    if trailing > 0 {
        digits.truncate(digits.len() - trailing);
    }
    // The exponent arithmetic is i64: an exponent near i32::MIN would
    // overflow in i32 (a debug panic, a silent wrap in release).
    let adj_exp = exp_given - frac.len() as i64 + trailing as i64;
    if !(-166..=128).contains(&adj_exp) {
        return None;
    }
    // Canonical zero, so the derived `Eq` agrees with `Ord`: -0 == 0.
    let zero = digits.is_empty();
    Some(NumKey {
        neg: neg && !zero,
        adj_exp: if zero { 0 } else { adj_exp as i32 },
        digits,
    })
}

/// `swift_plain_number(text).is_some()` without building the key: the same
/// grammar and range checks, no allocation. Seal and render only need the bit.
pub fn is_swift_plain_number(text: &str) -> bool {
    let body = trim_zs_tab(text);
    // Fast reject: a plain number starts with a digit, sign or dot.
    match body.as_bytes().first() {
        Some(b) if b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.') => {}
        _ => return false,
    }
    let rest = body.strip_prefix(['-', '+']).unwrap_or(body);
    let (mantissa, exp_given) = match rest.find(['e', 'E']) {
        Some(index) => {
            let exp = &rest[index + 1..];
            let digits = exp.strip_prefix(['+', '-']).unwrap_or(exp);
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return false;
            }
            match exp.parse::<i64>() {
                Ok(value) => (&rest[..index], value),
                Err(_) => return false,
            }
        }
        None => (rest, 0),
    };
    let mut parts = mantissa.split('.');
    let int = parts.next().unwrap_or("");
    let frac = parts.next().unwrap_or("");
    if parts.next().is_some()
        || !int.bytes().all(|b| b.is_ascii_digit())
        || !frac.bytes().all(|b| b.is_ascii_digit())
    {
        return false;
    }
    let total = int.len() + frac.len();
    if total == 0 || total > 38 {
        return false;
    }
    let digits = || int.bytes().chain(frac.bytes());
    // All-zero mantissa has no digits left after stripping, so trailing is 0
    // and the exponent range check still applies.
    let trailing = if digits().all(|b| b == b'0') {
        0
    } else {
        digits().rev().take_while(|&b| b == b'0').count()
    };
    (-166..=128).contains(&(exp_given - frac.len() as i64 + trailing as i64))
}

/// Trim only Zs (space separator) and TAB. Newlines are *not* trimmed —
/// `GridSort.number` does not strip them.
///
/// The offsets are byte offsets from `char_indices`, not char counts: a
/// leading NBSP or ideographic space is three bytes, and slicing at a char
/// count would cut inside it.
pub(crate) fn trim_zs_tab(s: &str) -> &str {
    let is_zs_or_tab = |c: char| {
        matches!(
            c,
            '\u{20}' | '\u{a0}' | '\u{1680}' | '\u{2000}'
                ..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\t'
        )
    };
    let start = s
        .char_indices()
        .find(|(_, c)| !is_zs_or_tab(*c))
        .map(|(index, _)| index)
        .unwrap_or(s.len());
    let end = s
        .char_indices()
        .rev()
        .find(|(_, c)| !is_zs_or_tab(*c))
        .map(|(index, c)| index + c.len_utf8())
        .unwrap_or(0);
    if start >= end {
        ""
    } else {
        &s[start..end]
    }
}

/// `swift_double` = `f64::from_str` (decimal, `inf`/`infinity`/`nan` case-insensitive)
/// plus a hand-written hex-float parser (`0x10`, `0x1.8p3`), because Swift's
/// `Double(String)` accepts it with the `p` exponent optional. Like Swift,
/// it does not trim: `"  7 "` is `None`.
pub fn swift_double(text: &str) -> Option<f64> {
    // Fast path: standard decimal float.
    if let Ok(f) = text.parse::<f64>() {
        return Some(f);
    }
    // Hex float: [+-]0x[0-9a-fA-F]*(.[0-9a-fA-F]*)?(p[+-]?[0-9]+)?, at least one hex digit.
    let (neg, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.strip_prefix('+').unwrap_or(text)),
    };
    let rest = rest
        .strip_prefix("0x")
        .or_else(|| rest.strip_prefix("0X"))?;
    let (mantissa, exp) = match rest.find(['p', 'P']) {
        Some(index) => (&rest[..index], rest[index + 1..].parse::<i32>().ok()?),
        None => (rest, 0),
    };
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if int.len() + frac.len() == 0
        || !int.bytes().all(|b| b.is_ascii_hexdigit())
        || !frac.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    let digit = |b: u8| f64::from((b as char).to_digit(16).unwrap_or(0));
    let mut value = int.bytes().fold(0.0, |acc, b| acc * 16.0 + digit(b));
    let mut scale = 1.0 / 16.0;
    for b in frac.bytes() {
        value += digit(b) * scale;
        scale /= 16.0;
    }
    let value = value * 2f64.powi(exp);
    Some(if neg { -value } else { value })
}

/// Fold a string for case-insensitive comparison: NFC, full lowercase,
/// then the special folding table.
pub fn fold(s: &str) -> String {
    // NFC then full lowercase.
    let mut folded = s.nfc().collect::<String>().to_lowercase();
    // Special table: ß → ss, ligatures, ς → σ, İ → i̇
    folded = folded
        .replace('ß', "ss")
        .replace('\u{fb00}', "ff")
        .replace('\u{fb01}', "fi")
        .replace('\u{fb02}', "fl")
        .replace('\u{fb03}', "ffi")
        .replace('\u{fb04}', "ffl")
        .replace('\u{fb05}', "ft")
        .replace('\u{fb06}', "st")
        .replace('ς', "σ")
        .replace('İ', "i\u{307}");
    folded
}

/// Case-insensitive contains: `fold(needle)` searched in `fold(haystack)`.
/// ASCII fast path: if both are ASCII, use ASCII case-insensitive search
/// without allocation.
pub fn ci_contains(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle.is_ascii() {
        return haystack
            .to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase());
    }
    fold(haystack).contains(&fold(needle))
}

/// Case-insensitive equality after folding.
pub fn ci_equal(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        return a.eq_ignore_ascii_case(b);
    }
    fold(a) == fold(b)
}

/// Write the 5-level natural sort key for `s` into `out`. The key is a
/// single memcmp-able byte sequence, §13.4:
///
/// `primary 0x00 secondary 0x00 tertiary 0x00 quaternary 0x00 raw-bytes`
///
/// Class bytes are always >= 1, so the `0x00` terminator makes a shorter
/// string sort before a longer one that shares its prefix.
pub fn natural_key(s: &str, out: &mut Vec<u8>) {
    // NFD, split into elements: one base char plus its combining marks.
    let elements = split_elements(s);

    // A digit run is one element per ASCII digit (digits never take marks),
    // so a run is a contiguous slice of `elements`.
    let mut primary = Vec::with_capacity(elements.len() * 2 + 1);
    let mut secondary = Vec::with_capacity(elements.len() + 1);
    let mut tertiary = Vec::with_capacity(elements.len() + 1);
    let mut quaternary = Vec::with_capacity(elements.len() + 1);

    let mut index = 0;
    while index < elements.len() {
        if digit_at(&elements, index) {
            let end = digit_run_end(&elements, index);
            // Leading zeros are dropped from the primary digits and moved to
            // the quaternary level; trailing zeros are kept, because dropping
            // them would put "2" above "10".
            let raw: Vec<u8> = (index..end)
                .map(|at| elements[at].base.unwrap() as u8)
                .collect();
            let leading = raw.iter().take_while(|byte| **byte == b'0').count();
            let significant = &raw[leading..];
            primary.push(0x03);
            primary.push(significant.len().min(255) as u8);
            primary.extend_from_slice(significant);
            quaternary.push(leading.min(255) as u8);
            index = end;
            continue;
        }

        let base = elements[index].base;
        match base {
            Some(base) if base.is_whitespace() => {
                primary.push(0x01);
                quaternary.push(0x00);
            }
            Some(base) if base.is_ascii_lowercase() => {
                primary.push(0x04);
                primary.push(base as u8);
                quaternary.push(0x00);
            }
            // ASCII uppercase folds to its lowercase primary; the tertiary
            // level is what puts it after the lowercase form.
            Some(base) if base.is_ascii_uppercase() => {
                primary.push(0x04);
                primary.push(base.to_ascii_lowercase() as u8);
                quaternary.push(0x00);
            }
            // Letters and digits of other scripts, Katakana folded to
            // Hiragana so the two kana rows interleave by code point.
            Some(base) if base.is_alphabetic() || base.is_numeric() => {
                primary.push(0x05);
                primary.extend_from_slice(&u32_to_be3(other_script_cp(base)));
                quaternary.push(0x00);
            }
            // Punctuation, symbols, emoji: anything left, by code point.
            Some(base) => {
                primary.push(0x02);
                primary.extend_from_slice(&u32_to_be3(base as u32));
                quaternary.push(0x00);
            }
            // A combining mark with no base of its own sorts at the front of
            // its class, ahead of any accented letter.
            None => {
                primary.push(0x02);
                primary.extend_from_slice(&u32_to_be3(0));
                quaternary.push(0x00);
            }
        }

        // Secondary: the accent marks of this element. No marks is 0x00, so
        // an unaccented letter sorts before the accented one.
        let marks = &elements[index].marks;
        if marks.is_empty() {
            secondary.push(0x00);
        } else {
            secondary.push(marks.len().min(255) as u8);
            for mark in marks {
                secondary.extend_from_slice(&u32_to_be3(*mark as u32));
            }
        }

        // Tertiary: lowercase before uppercase.
        tertiary.push(match base {
            Some(base) if base.is_lowercase() => 0x01,
            Some(_) => 0x02,
            None => 0x00,
        });
        index += 1;
    }

    // End of string is class 0, so a prefix sorts before the longer string.
    primary.push(0x00);
    out.extend_from_slice(&primary);
    secondary.push(0x00);
    out.extend_from_slice(&secondary);
    tertiary.push(0x00);
    out.extend_from_slice(&tertiary);
    quaternary.push(0x00);
    out.extend_from_slice(&quaternary);
    out.extend_from_slice(s.as_bytes());
}

/// NFD, split into one element per base char with its combining marks. A
/// mark with no base yet becomes its own element, so no element ever owns a
/// mark it did not decompose with.
fn split_elements(s: &str) -> Vec<Element> {
    let mut elements: Vec<Element> = Vec::new();
    for c in s.nfd() {
        if c.general_category() == unicode_properties::GeneralCategory::NonspacingMark {
            if let Some(last) = elements.last_mut() {
                if last.base.is_some() {
                    last.marks.push(c);
                    continue;
                }
            }
            elements.push(Element {
                base: None,
                marks: vec![c],
            });
        } else {
            elements.push(Element {
                base: Some(c),
                marks: Vec::new(),
            });
        }
    }
    elements
}

fn digit_at(elements: &[Element], index: usize) -> bool {
    elements[index]
        .base
        .is_some_and(|base| base.is_ascii_digit())
}

fn digit_run_end(elements: &[Element], start: usize) -> usize {
    let mut end = start;
    while end < elements.len() && digit_at(elements, end) {
        end += 1;
    }
    end
}

/// Katakana U+30A1..U+30F6 folds to the matching Hiragana U+3041..U+3096;
/// every other code point keeps its own.
fn other_script_cp(base: char) -> u32 {
    let cp = base as u32;
    if (0x30A1..=0x30F6).contains(&cp) {
        cp - 0x60
    } else {
        cp
    }
}

#[derive(Debug)]
struct Element {
    base: Option<char>,
    marks: Vec<char>,
}

fn u32_to_be3(v: u32) -> [u8; 3] {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

/// Prefix key for the 16-byte fast path: first 16 bytes of the full
/// natural key. Used when the full key arena doesn't fit in the budget.
pub fn natural_key_prefix(s: &str) -> u128 {
    let mut key = Vec::new();
    natural_key(s, &mut key);
    key.resize(16, 0);
    u128::from_le_bytes(key[..16].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_swift_plain_number_agrees_with_the_key_parser() {
        let cases = [
            "",
            " ",
            "0",
            "-0",
            "+0",
            "00.00",
            "0e999",
            "0e-200",
            "0.0e129",
            "1",
            "-1",
            "+1.5",
            "1.",
            ".5",
            ".",
            "-",
            "+",
            "1e3",
            "1E-3",
            "1e",
            "1e+",
            "1e+5x",
            "1.2.3",
            "abc",
            "1a",
            " 42\t",
            "\u{a0}7\u{3000}",
            "4\n",
            "1e128",
            "1e129",
            "1e-166",
            "1e-167",
            "100e-168",
            "12345678901234567890123456789012345678",
            "123456789012345678901234567890123456789",
            "0.000000000000000000000000000000000000001e-120",
            "1e99999999999999999999",
            "1e-99999999999999999999",
            "٣",
            "1_000",
            "NaN",
            "inf",
            "0x10",
            "-.5e2",
            "5.e2",
            "1e0001",
            "0.0e5",
            "-0.0",
        ];
        for text in cases {
            assert_eq!(
                is_swift_plain_number(text),
                swift_plain_number(text).is_some(),
                "{text:?}"
            );
        }
    }

    #[test]
    fn swift_plain_number_accepts_integers() {
        assert_eq!(swift_plain_number("42").unwrap().digits, b"42");
        assert!(swift_plain_number("-42").unwrap().neg);
        assert!(!swift_plain_number("+42").unwrap().neg);
    }

    #[test]
    fn swift_plain_number_accepts_decimals() {
        assert_eq!(swift_plain_number("1.5").unwrap().adj_exp, -1);
        assert_eq!(swift_plain_number("1.50").unwrap().adj_exp, -1);
        assert_eq!(
            swift_plain_number("1.5").unwrap().digits,
            swift_plain_number("1.50").unwrap().digits
        );
    }

    #[test]
    fn swift_plain_number_accepts_exponent() {
        assert_eq!(swift_plain_number("1e3").unwrap().adj_exp, 3);
        assert_eq!(swift_plain_number("1e-3").unwrap().adj_exp, -3);
    }

    #[test]
    fn swift_plain_number_rejects_malformed() {
        assert!(swift_plain_number("").is_none());
        assert!(swift_plain_number("1.2.3").is_none());
        assert!(swift_plain_number("1e").is_none());
        assert!(swift_plain_number("1e+").is_none());
        assert!(swift_plain_number("abc").is_none());
    }

    #[test]
    fn swift_double_parses_decimal() {
        assert_eq!(swift_double("1.5"), Some(1.5));
        assert_eq!(swift_double("-0.0"), Some(-0.0));
    }

    #[test]
    fn swift_double_parses_hex() {
        assert_eq!(swift_double("0x1p0"), Some(1.0));
        assert_eq!(swift_double("0x1.8p3"), Some(12.0));
    }

    #[test]
    fn fold_handles_special() {
        assert_eq!(fold("ß"), "ss");
        assert_eq!(fold("ﬁ"), "fi");
        assert_eq!(fold("İ"), "i\u{307}");
    }

    #[test]
    fn ci_contains_ascii_fast_path() {
        assert!(ci_contains("Hello World", "world"));
        assert!(ci_contains("HELLO", "hello"));
        assert!(!ci_contains("Hello", "world"));
    }

    #[test]
    fn ci_contains_unicode() {
        assert!(ci_contains("café", "CAFÉ"));
        assert!(ci_contains("Straße", "STRASSE"));
    }

    #[test]
    fn ci_equal_ascii() {
        assert!(ci_equal("hello", "HELLO"));
        assert!(!ci_equal("hello", "world"));
    }

    #[test]
    fn ci_equal_unicode() {
        assert!(ci_equal("café", "CAFÉ"));
        assert!(ci_equal("Straße", "STRASSE"));
    }

    #[test]
    fn natural_key_is_memcmp_able() {
        let mut a = Vec::new();
        let mut b = Vec::new();
        natural_key("a", &mut a);
        natural_key("b", &mut b);
        assert!(a < b);
    }

    #[test]
    fn natural_key_nulls_last() {
        let mut empty = Vec::new();
        let mut nonempty = Vec::new();
        natural_key("", &mut empty);
        natural_key("a", &mut nonempty);
        assert!(empty < nonempty);
    }

    #[test]
    fn natural_key_digit_runs_order_by_value() {
        let mut k1 = Vec::new();
        let mut k2 = Vec::new();
        natural_key("1", &mut k1);
        natural_key("2", &mut k2);
        assert!(k1 < k2);
        let mut k10 = Vec::new();
        natural_key("10", &mut k10);
        assert!(k2 < k10);
    }

    #[test]
    fn num_key_orders_zero_between_negatives_and_positives() {
        let key = |t: &str| swift_plain_number(t).unwrap();
        assert!(key("-3") < key("-0"));
        assert!(key("-0") < key("3"));
        assert_eq!(key("-0").cmp(&key("0")), Ordering::Equal);
        assert_eq!(key("-0"), key("0"));
    }

    #[test]
    fn num_key_ignores_leading_zeros() {
        let key = |t: &str| swift_plain_number(t).unwrap();
        assert_eq!(key("007").cmp(&key("7")), Ordering::Equal);
        assert!(key("007") < key("10"));
        assert!(key("0.1") < key("1"));
        assert!(key("0.1") > key("0.05"));
    }

    #[test]
    fn num_key_compares_magnitude_before_digits() {
        let key = |t: &str| swift_plain_number(t).unwrap();
        assert!(key(".5") < key("1.5"));
        assert!(key("1.5") < key("15"));
        assert!(key("-1.5") > key("-15"));
        assert!(key("1") < key("1.5"));
    }

    #[test]
    fn swift_double_accepts_hex_without_exponent() {
        assert_eq!(swift_double("0x10"), Some(16.0));
        assert_eq!(swift_double("-0x1.8"), Some(-1.5));
        assert_eq!(swift_double("0x"), None);
        assert_eq!(swift_double("0xg"), None);
    }

    #[test]
    fn swift_double_does_not_trim() {
        assert_eq!(swift_double("  7 "), None);
        assert_eq!(swift_double("7 "), None);
        assert_eq!(swift_double("\n7"), None);
        assert_eq!(swift_double("7"), Some(7.0));
    }
}
