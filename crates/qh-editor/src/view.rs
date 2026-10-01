//! The text the parser sees. Same length as the statement, so every offset carries over.
//!
//! The grammar does not know `:name` parameters: each `:` becomes a one-character error, which
//! makes a parse 5 to 6 times slower and paints false errors. So before parsing, a `:` that
//! starts a parameter is replaced by `_`, turning `:since` into the identifier `_since`.
//!
//! The rule is local to three bytes (the one before, the colon, the one after), which is what
//! lets an edit be widened by one byte on each side and still tell the tree everything that
//! changed ([`widen`]).

use std::borrow::Cow;

/// A `:` followed by a letter or `_`, and not preceded by a word character or another `:`, so
/// `a::text`, `x:a`, `arr[lo:hi]` and `:=` are left alone (the parse of those is what it was).
pub fn parse_bytes(text: &[u8]) -> Cow<'_, [u8]> {
    let mut view: Option<Vec<u8>> = None;
    for (i, &byte) in text.iter().enumerate() {
        if byte != b':' {
            continue;
        }
        let starts_name = text
            .get(i + 1)
            .is_some_and(|next| next.is_ascii_alphabetic() || *next == b'_');
        let after_word =
            i > 0 && (text[i - 1].is_ascii_alphanumeric() || matches!(text[i - 1], b'_' | b':'));
        if starts_name && !after_word {
            view.get_or_insert_with(|| text.to_vec())[i] = b'_';
        }
    }
    match view {
        Some(view) => Cow::Owned(view),
        None => Cow::Borrowed(text),
    }
}

/// The edit `[start, end)` of `text`, widened by one byte each way and rounded out to
/// character boundaries: everything whose view can change when those bytes change.
pub fn widen(text: &str, start: usize, end: usize) -> (usize, usize) {
    let mut start = start.saturating_sub(1);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (end + 1).min(text.len());
    while !text.is_char_boundary(end) {
        end += 1;
    }
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(text: &str) -> String {
        String::from_utf8(parse_bytes(text.as_bytes()).into_owned()).unwrap()
    }

    #[test]
    fn a_parameter_colon_becomes_an_underscore() {
        assert_eq!(view("select :a, :b_1"), "select _a, _b_1");
        assert_eq!(view(":a"), "_a");
    }

    #[test]
    fn casts_slices_assignments_and_glued_colons_are_left_alone() {
        for text in [
            "a::text",
            "x:a",
            "arr[lo:hi]",
            "arr[1:n]",
            "@x := 1",
            "a:: b",
            "1:2",
            ":1",
            ": a",
        ] {
            assert_eq!(view(text), text);
        }
        // `arr[:n]` is a parameter to the view (the colon follows a bracket); the classifier
        // decides what colour it gets.
        assert_eq!(view("arr[:n]"), "arr[_n]");
    }

    #[test]
    fn the_view_has_the_same_length() {
        let text = "select :é, ':a' -- :b\n, :c";
        assert_eq!(parse_bytes(text.as_bytes()).len(), text.len());
    }

    #[test]
    fn widening_rounds_out_to_characters() {
        assert_eq!(widen("abcdef", 2, 4), (1, 5));
        assert_eq!(widen("abcdef", 0, 6), (0, 6));
        // Bytes 1..3 are `é`; widening from 3 lands inside it and rounds down.
        assert_eq!(widen("aéb", 3, 3), (1, 4));
        assert_eq!(widen("aéb", 1, 1), (0, 3));
    }
}
