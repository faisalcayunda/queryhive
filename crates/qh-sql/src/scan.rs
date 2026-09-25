//! One pass over a statement that knows where text is text and where it is
//! syntax.

/// What a scan of one string found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Scan {
    /// Byte offsets of the `;` characters that really separate statements.
    ///
    /// A `;` inside a literal, a quoted identifier, a comment or a dollar-quoted
    /// block is not included: it is content.
    pub separators: Vec<usize>,
    /// Whether the string ends with a `;` that acts as a terminator.
    ///
    /// True even when that `;` is the last character of a trailing comment.
    /// That looks inconsistent with the line above and is not: a comment cannot
    /// *contain* a separator, but a `;` at the very end of the text still ends
    /// the statement on the wire, so `SELECT 1 -- note;` is a syntax error if it
    /// survives. The Python engine reached the same conclusion, and
    /// `check_wrappers_drop_a_comment_trailing_terminator` pins it.
    pub ends_with_terminator: bool,
    /// The first bare keyword, uppercased, with leading whitespace and comments
    /// skipped. `Some("SELECT")` for `/* c */ select 1`.
    pub leading_keyword: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Normal,
    /// Inside `'...'`. Ends on an unescaped `'`.
    SingleQuoted,
    /// Inside `"..."`, a quoted identifier. Ends on an unescaped `"`.
    DoubleQuoted,
    /// Inside `` `...` ``, MySQL's quoted identifier.
    BacktickQuoted,
    /// Inside `-- ...`, until the end of the line.
    LineComment,
    /// Inside `/* ... */`.
    BlockComment,
    /// Inside `$tag$ ... $tag$`, Postgres' dollar quoting.
    DollarQuoted,
}

/// Scan one possibly multi-statement string.
///
/// The scanner is deliberately permissive: SQL that does not parse still scans.
/// The reason is that one caller is building an error position for the server to
/// object to, so the input is frequently invalid by definition, and refusing to
/// scan it would turn a useful error message into a second error.
pub fn scan(sql: &str) -> Scan {
    let bytes = sql.as_bytes();
    let mut scan = Scan::default();
    let mut state = State::Normal;
    let mut dollar_tag: Vec<u8> = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        let byte = bytes[index];
        match state {
            State::Normal => match byte {
                b'\'' => {
                    state = State::SingleQuoted;
                    index += 1;
                }
                b'"' => {
                    state = State::DoubleQuoted;
                    index += 1;
                }
                b'`' => {
                    state = State::BacktickQuoted;
                    index += 1;
                }
                b'-' if bytes.get(index + 1) == Some(&b'-') => {
                    state = State::LineComment;
                    index += 2;
                }
                b'/' if bytes.get(index + 1) == Some(&b'*') => {
                    state = State::BlockComment;
                    index += 2;
                }
                b';' => {
                    scan.separators.push(index);
                    index += 1;
                }
                b'$' => {
                    if let Some((tag, next)) = read_dollar_tag(bytes, index) {
                        dollar_tag = tag;
                        state = State::DollarQuoted;
                        index = next;
                    } else {
                        index += 1;
                    }
                }
                _ => {
                    index += 1;
                }
            },
            State::SingleQuoted => {
                if byte == b'\'' {
                    // A doubled quote is an escaped quote, not the end.
                    if bytes.get(index + 1) == Some(&b'\'') {
                        index += 2;
                    } else {
                        state = State::Normal;
                        index += 1;
                    }
                } else {
                    index += 1;
                }
            }
            State::DoubleQuoted => {
                if byte == b'"' {
                    if bytes.get(index + 1) == Some(&b'"') {
                        index += 2;
                    } else {
                        state = State::Normal;
                        index += 1;
                    }
                } else {
                    index += 1;
                }
            }
            State::BacktickQuoted => {
                if byte == b'`' {
                    if bytes.get(index + 1) == Some(&b'`') {
                        index += 2;
                    } else {
                        state = State::Normal;
                        index += 1;
                    }
                } else {
                    index += 1;
                }
            }
            State::LineComment => {
                if byte == b'\n' {
                    state = State::Normal;
                }
                index += 1;
            }
            State::BlockComment => {
                // Not nested. Postgres allows nesting and MySQL does not; no
                // driver here needs it, and pretending otherwise would silently
                // mis-scan the MySQL case.
                if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    state = State::Normal;
                    index += 2;
                } else {
                    index += 1;
                }
            }
            State::DollarQuoted => {
                if byte == b'$' && bytes[index..].starts_with(&dollar_tag) {
                    index += dollar_tag.len();
                    state = State::Normal;
                } else {
                    index += 1;
                }
            }
        }
    }

    scan.leading_keyword = leading_keyword(sql);
    scan.ends_with_terminator = sql.trim_end().ends_with(';');
    scan
}

/// Read a `$tag$` opener at `start`, returning the tag (including both `$`) and
/// the offset just past it.
///
/// The tag must be followed by end-of-input or a non-identifier byte, so that a
/// `$1` placeholder — which Postgres uses for parameters and which is not a
/// dollar quote — is not mistaken for one.
fn read_dollar_tag(bytes: &[u8], start: usize) -> Option<(Vec<u8>, usize)> {
    debug_assert_eq!(bytes.get(start), Some(&b'$'));
    let mut index = start + 1;
    while index < bytes.len() {
        match bytes[index] {
            b'$' => {
                let tag = bytes[start..=index].to_vec();
                return Some((tag, index + 1));
            }
            byte if byte.is_ascii_alphanumeric() || byte == b'_' => index += 1,
            _ => return None,
        }
    }
    None
}

/// The first bare keyword, uppercased.
fn leading_keyword(sql: &str) -> Option<String> {
    let bytes = sql.as_bytes();
    let mut index = 0;
    loop {
        // Skip whitespace.
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            return None;
        }
        // Skip comments, repeatedly: `/* a */ -- b\n SELECT 1` has a keyword
        // after two of them.
        if bytes[index] == b'-' && bytes.get(index + 1) == Some(&b'-') {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index < bytes.len() {
                if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    index += 2;
                    break;
                }
                index += 1;
            }
            continue;
        }
        break;
    }

    let start = index;
    while index < bytes.len() && (bytes[index].is_ascii_alphabetic() || bytes[index] == b'_') {
        index += 1;
    }
    if index == start {
        return None;
    }
    Some(sql[start..index].to_ascii_uppercase())
}

/// Whether anything in `sql` is more than whitespace, a comment, or a `;`.
///
/// Used to tell "no statement at all" from "one statement". A string of only
/// separators and comments is not a statement.
pub fn has_significant_text(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte.is_ascii_whitespace() || byte == b';' {
            index += 1;
            continue;
        }
        if byte == b'-' && bytes.get(index + 1) == Some(&b'-') {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            continue;
        }
        if byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
            index += 2;
            while index < bytes.len() {
                if bytes[index] == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    index += 2;
                    break;
                }
                index += 1;
            }
            continue;
        }
        return true;
    }
    false
}

/// How many statements `sql` holds.
///
/// The rule, which is the Python engine's own:
///
/// * a `;` at the end of the text is a *terminator*, so it closes a statement
///   rather than starting a new one — `SELECT 1;` is one statement;
/// * a `;` with anything after it *separates*, so it adds a statement —
///   `SELECT 1; SELECT 2` is two;
/// * which means `SELECT 1;;` is two as well: the first `;` separates and the
///   second terminates an empty statement, and `count` refuses it rather than
///   guessing which one the user meant.
///
/// `exporter/drivers.py:365` drew the same line, and
/// `check_wrappers_drop_a_comment_trailing_terminator` pins it.
pub fn statement_count(sql: &str, scan: &Scan) -> usize {
    if !has_significant_text(sql) {
        return 0;
    }
    if scan.separators.is_empty() {
        return 1;
    }
    if scan.ends_with_terminator {
        scan.separators.len()
    } else {
        scan.separators.len() + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn separators(sql: &str) -> Vec<usize> {
        scan(sql).separators
    }

    #[test]
    fn a_terminator_at_the_end_is_one_separator() {
        assert_eq!(separators("SELECT 1"), Vec::<usize>::new());
        assert_eq!(separators("SELECT 1;"), vec![8]);
        assert_eq!(separators("SELECT 1;;"), vec![8, 9]);
    }

    #[test]
    fn a_semicolon_inside_a_literal_is_text() {
        // This is the case a naive `rstrip(";")` gets wrong.
        assert_eq!(separators("SELECT 'a;b;'"), Vec::<usize>::new());
        assert_eq!(separators("SELECT 'a;b;' ;"), vec![14]);
        // A doubled quote is an escaped quote, so the literal does not end there.
        assert_eq!(separators("SELECT 'it''s; ok'"), Vec::<usize>::new());
    }

    #[test]
    fn a_semicolon_inside_a_comment_is_text() {
        assert_eq!(separators("SELECT 1 -- note;"), Vec::<usize>::new());
        assert_eq!(separators("SELECT 1 /* x; */;"), vec![17]);
    }

    #[test]
    fn a_semicolon_inside_a_dollar_quoted_body_is_text() {
        assert_eq!(
            separators("DO $body$ SELECT 1; SELECT 2; $body$"),
            Vec::<usize>::new()
        );
        // A parameter placeholder is not a dollar quote.
        assert_eq!(separators("SELECT $1;"), vec![9]);
    }

    #[test]
    fn a_quoted_identifier_can_hold_a_separator() {
        assert_eq!(separators("SELECT * FROM \"a;b\";"), vec![19]);
        assert_eq!(separators("SELECT * FROM `a;b`;"), vec![19]);
    }

    #[test]
    fn the_leading_keyword_skips_comments_and_whitespace() {
        assert_eq!(
            scan("  select 1").leading_keyword.as_deref(),
            Some("SELECT")
        );
        assert_eq!(
            scan("/* c */ select 1").leading_keyword.as_deref(),
            Some("SELECT")
        );
        assert_eq!(
            scan("-- c\n WITH x AS (SELECT 1) SELECT * FROM x")
                .leading_keyword
                .as_deref(),
            Some("WITH")
        );
        assert_eq!(scan("   ").leading_keyword, None);
    }

    #[test]
    fn a_trailing_terminator_is_seen_even_inside_a_comment() {
        assert!(scan("SELECT 1 -- note;").ends_with_terminator);
        assert!(!scan("SELECT 1 -- note").ends_with_terminator);
        assert!(!scan("SELECT 'a;b;'").ends_with_terminator);
    }

    #[test]
    fn trivia_only_is_not_a_statement() {
        assert!(!has_significant_text("   "));
        assert!(!has_significant_text("-- just a comment"));
        assert!(!has_significant_text("/* just a comment */"));
        assert!(!has_significant_text(";"));
        assert!(has_significant_text("SELECT '--not a comment'"));
    }

    #[test]
    fn statement_counting_follows_the_terminators() {
        for (sql, expected) in [
            ("SELECT 1", 1),
            ("SELECT 1;", 1),
            ("SELECT 1;;", 2),
            ("SELECT 1; SELECT 2", 2),
            ("SELECT 'a;b;'", 1),
            ("SELECT 1 -- note;", 1),
            ("", 0),
            ("   ", 0),
            (";", 0),
        ] {
            let found = statement_count(sql, &scan(sql));
            assert_eq!(found, expected, "{sql:?}");
        }
    }

    #[test]
    fn an_unterminated_literal_does_not_hang_or_panic() {
        // Server data and user typos both produce input like this, and the
        // scanner has to answer anyway: it is used to build error positions.
        assert_eq!(separators("SELECT 'unterminated;"), Vec::<usize>::new());
        assert_eq!(separators("SELECT /* unterminated;"), Vec::<usize>::new());
        assert_eq!(
            separators("SELECT $tag$ unterminated;"),
            Vec::<usize>::new()
        );
    }
}
