//! The delimiter pair next to the caret, for W10-T7's matching-bracket bands.
//!
//! A pure function of one statement's text and the caret in it, so the answer after an edit is
//! the answer for a fresh copy of the same text. Brackets and quotes come from `qh_sql::walk`
//! (the scanner the engine uses), so a bracket inside a string or a comment is never paired,
//! and an opener left open pairs with nothing.

use qh_sql::{walk, EndState, Lexer, OpaqueKind, Visitor};

/// A statement longer than this many bytes gets no pair: the walk is not free and a band on a
/// megabyte of text is not worth a stall.
pub const MAX_STATEMENT_BYTES: usize = 1 << 20;

#[derive(Default)]
struct Scan(Vec<(OpaqueKind, usize, usize)>);

impl Visitor for Scan {
    fn opaque(&mut self, kind: OpaqueKind, start: usize, end: usize) {
        self.0.push((kind, start, end));
    }
}

/// Byte range of one delimiter.
type Span = (usize, usize);

/// The delimiter pair at the caret as `[start, len, start, len]` in UTF-16 units of
/// `statement_text`, opener first; empty when there is none.
///
/// The delimiter ending at the caret is tried before the one starting there. Brackets are
/// `()` and `[]`; quotes are `'…'` (prefix such as `E` included), `"…"`, `` `…` `` and
/// `$tag$…$tag$`, paired only when closed. Comments pair with nothing.
pub fn bracket_pair(
    statement_text: &str,
    caret_in_statement_utf16: u32,
    lexer: impl Into<Lexer>,
) -> Vec<u32> {
    if statement_text.len() > MAX_STATEMENT_BYTES {
        return Vec::new();
    }
    let Some(caret) = byte_of(statement_text, caret_in_statement_utf16) else {
        return Vec::new();
    };
    let bytes = statement_text.as_bytes();
    let mut scan = Scan::default();
    // A region left open is reported last with `end == len`; its tail only looks like a closer
    // (`'it''`, `'a\'`), so it pairs with nothing.
    if let EndState::Open(_) = walk(bytes, lexer, &mut scan) {
        scan.0.pop();
    }

    let mut pairs: Vec<(Span, Span)> = Vec::new();
    let (mut round, mut square): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
    let mut regions = scan.0.iter().peekable();
    let mut at = 0;
    while at < bytes.len() {
        if let Some(&&(kind, start, end)) = regions.peek() {
            if start <= at {
                regions.next();
                at = at.max(end);
                if let Some(pair) = quote_pair(bytes, kind, start, end) {
                    pairs.push(pair);
                }
                continue;
            }
        }
        match bytes[at] {
            b'(' => round.push(at),
            b'[' => square.push(at),
            b')' => pairs.extend(round.pop().map(|open| ((open, open + 1), (at, at + 1)))),
            b']' => pairs.extend(square.pop().map(|open| ((open, open + 1), (at, at + 1)))),
            _ => {}
        }
        at += 1;
    }

    let hit = |touches: &dyn Fn(Span) -> bool| {
        pairs
            .iter()
            .find(|(a, b)| touches(*a) || touches(*b))
            .copied()
    };
    let Some((open, close)) = hit(&|s| s.1 == caret).or_else(|| hit(&|s| s.0 == caret)) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(4);
    for (start, end) in [open, close] {
        out.push(units(&statement_text[..start]));
        out.push(units(&statement_text[start..end]));
    }
    out
}

/// The two delimiters of a closed quote-like region, or `None` for a comment or a region
/// left open at the end of the text.
fn quote_pair(bytes: &[u8], kind: OpaqueKind, start: usize, end: usize) -> Option<(Span, Span)> {
    let region = &bytes[start..end];
    let (open_len, close_len) = match kind {
        OpaqueKind::LineComment | OpaqueKind::BlockComment => return None,
        OpaqueKind::SingleQuote => (region.iter().position(|&b| b == b'\'')? + 1, 1),
        OpaqueKind::DoubleQuote | OpaqueKind::Backtick => (1, 1),
        OpaqueKind::DollarQuote => {
            let tag = region.get(1..)?.iter().position(|&b| b == b'$')? + 2;
            (tag, tag)
        }
    };
    if region.len() < open_len + close_len {
        return None;
    }
    let closer = &region[region.len() - close_len..];
    let closed = match kind {
        OpaqueKind::SingleQuote => closer == b"'",
        OpaqueKind::DoubleQuote => closer == b"\"",
        OpaqueKind::Backtick => closer == b"`",
        _ => closer == &region[..open_len],
    };
    closed.then_some(((start, start + open_len), (end - close_len, end)))
}

fn units(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

/// The byte offset of a UTF-16 offset, `None` past the end or inside a surrogate pair.
fn byte_of(text: &str, target: u32) -> Option<usize> {
    let mut seen = 0u32;
    for (byte, ch) in text.char_indices() {
        if seen == target {
            return Some(byte);
        }
        seen += ch.len_utf16() as u32;
        if seen > target {
            return None;
        }
    }
    (seen == target).then_some(text.len())
}

#[cfg(test)]
mod bracket_pair_tests {
    use super::*;
    use qh_sql::Dialect;

    fn pair(text: &str, caret: u32) -> Vec<u32> {
        bracket_pair(text, caret, Dialect::Generic)
    }

    #[test]
    fn unclosed_quotes_ending_in_an_escaped_or_doubled_quote_pair_nothing() {
        let none = Vec::<u32>::new();
        assert_eq!(pair("x 'abc''", 2), none);
        assert_eq!(pair("x '''", 2), none);
        assert_eq!(pair("x \"a\"\"", 2), none);
        assert_eq!(bracket_pair("x 'ab\\'", 2, Dialect::Mysql), none);
    }

    #[test]
    fn nested_parentheses_and_brackets_pair_by_depth() {
        let text = "f(a, (b), c[1])";
        assert_eq!(pair(text, 2), vec![1, 1, 14, 1]);
        assert_eq!(pair(text, 6), vec![5, 1, 7, 1]);
        assert_eq!(pair(text, 14), vec![11, 1, 13, 1]);
        assert_eq!(pair(text, 15), vec![1, 1, 14, 1]);
    }

    #[test]
    fn the_character_before_the_caret_wins_over_the_one_after() {
        assert_eq!(pair("(a)(b)", 3), vec![0, 1, 2, 1]);
        assert_eq!(pair("(a)(b)", 4), vec![3, 1, 5, 1]);
        assert_eq!(pair("a(b)", 0), Vec::<u32>::new());
        assert_eq!(pair("(a) b", 4), Vec::<u32>::new());
    }

    #[test]
    fn brackets_inside_strings_and_comments_never_pair() {
        assert_eq!(pair("a(')')", 2), vec![1, 1, 5, 1]);
        // The `)` inside the string is not a bracket; the quote beside the caret is.
        assert_eq!(pair("a(')')", 4), vec![2, 1, 4, 1]);
        assert_eq!(pair("( /* ) */ )", 1), vec![0, 1, 10, 1]);
        assert_eq!(pair("( -- )\n)", 1), vec![0, 1, 7, 1]);
        assert_eq!(pair("select ( $$ ) $$ )", 8), vec![7, 1, 17, 1]);
    }

    #[test]
    fn quotes_pair_when_the_caret_touches_either_end() {
        assert_eq!(pair("x 'ab' y", 2), vec![2, 1, 5, 1]);
        assert_eq!(pair("x 'ab' y", 6), vec![2, 1, 5, 1]);
        assert_eq!(pair("x 'ab' y", 4), Vec::<u32>::new());
        assert_eq!(pair("x \"ab\"", 3), vec![2, 1, 5, 1]);
        assert_eq!(pair("x 'it''s'", 3), vec![2, 1, 8, 1]);
    }

    #[test]
    fn dollar_tags_and_prefixes_are_whole_delimiters() {
        let pg = |t: &str, c| bracket_pair(t, c, Dialect::Postgres);
        assert_eq!(pg("x $a$ b $a$", 2), vec![2, 3, 8, 3]);
        assert_eq!(pg("x $a$ b $a$", 11), vec![2, 3, 8, 3]);
        assert_eq!(pg("x E'b'", 4), vec![2, 2, 5, 1]);
    }

    #[test]
    fn backticks_pair_in_mysql() {
        assert_eq!(
            bracket_pair("select `a b`", 7, Dialect::Mysql),
            vec![7, 1, 11, 1]
        );
    }

    #[test]
    fn nothing_pairs_when_unclosed() {
        assert_eq!(pair("a(b", 2), Vec::<u32>::new());
        assert_eq!(pair("a)b", 2), Vec::<u32>::new());
        assert_eq!(pair("x 'ab", 2), Vec::<u32>::new());
        assert_eq!(pair("x /* a", 2), Vec::<u32>::new());
        assert_eq!(pair("(]", 1), Vec::<u32>::new());
    }

    #[test]
    fn offsets_are_utf16_units() {
        // U+1F600 is two units and four bytes; 'é' is one unit and two bytes.
        let text = "é('😀')";
        assert_eq!(pair(text, 2), vec![1, 1, 6, 1]);
        assert_eq!(pair(text, 3), vec![2, 1, 5, 1]);
        // Inside the surrogate pair, and past the end.
        assert_eq!(pair(text, 4), Vec::<u32>::new());
        assert_eq!(pair(text, 99), Vec::<u32>::new());
    }

    #[test]
    fn a_huge_statement_gets_nothing() {
        let text = format!("({})", "a".repeat(MAX_STATEMENT_BYTES));
        assert_eq!(pair(&text, 1), Vec::<u32>::new());
    }

    #[test]
    fn an_edit_gives_the_answer_of_a_fresh_copy() {
        use crate::TextBuffer;
        let mut buffer = TextBuffer::new("select f(1)").unwrap();
        buffer.edit(9, 0, "(0)+").unwrap();
        assert_eq!(buffer.as_str(), "select f((0)+1)");
        assert_eq!(pair(buffer.as_str(), 9), vec![8, 1, 14, 1]);
        assert_eq!(pair(buffer.as_str(), 10), vec![9, 1, 11, 1]);
    }
}
