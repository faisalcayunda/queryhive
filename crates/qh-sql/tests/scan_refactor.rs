//! G4: the scanner refactor changes nothing.
//!
//! `walk` was extracted from `scan_dialect`, and `scan_dialect` is the single input of every
//! Safe Mode decision, statement split and terminator strip in the engine. This file holds
//! `frozen`, a copy of the scanner (comments stripped) as it stood before that extraction (commit
//! `a25fa50`, after W3-T0b: MySQL's six lexers, PostgreSQL's four, Trino), and checks that the
//! live `qh_sql` returns exactly what the copy returns:
//!
//! * the whole [`Scan`] (`separators`, `ends_with_terminator`, `leading_keyword`, `keywords`),
//!   `first_significant` and `statement_count_dialect`, for every lexer a connection can use,
//!   over the repository's SQL and over seeded random inputs;
//! * the Safe Mode surface built on top of them (`decisions_readings`, `check_confirmed_readings`,
//!   `statements_with_lines_dialect`, `count_statement_dialect`, ...), as a digest computed with
//!   the pre-refactor code (`SURFACE_DIGEST`). It is a digest and not a second copy of
//!   `classify.rs` because `classify.rs` reads the scanner through `scan_dialect` and
//!   `first_significant` only, and both are compared above.
//!
//! The copy is here and not in `src/` on purpose: nothing may call it. Do not edit it; if the
//! scanner's behaviour is meant to change, that is a Safe Mode change with its own review, and
//! this file is updated in that commit with the reason.
//!
//! Random cases: 10,000 by default; `QH_SCAN_SOAK=1` runs 1,000,000 (use `--release`).

use qh_sql::{
    check_confirmed_readings, classify_readings, count_statement_dialect, decisions_readings,
    first_significant, scan_dialect, statement_count_dialect, statements_agreeing,
    statements_with_lines_dialect, strip_terminator_dialect, Dialect, Lexer, SafeMode, Scan,
    SAFE_MODES,
};

#[allow(dead_code, clippy::all)]
mod frozen {
    use qh_sql::Scan;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Lexer {
        mysql_syntax: bool,
        trino: bool,
        postgres: bool,
        backslash_escapes: bool,
        vt_is_space: bool,
        ansi_quotes: bool,
        executable_code: bool,
    }

    impl Lexer {
        const GENERIC: Lexer = Lexer {
            mysql_syntax: false,
            trino: false,
            postgres: false,
            vt_is_space: false,
            backslash_escapes: false,
            ansi_quotes: false,
            executable_code: false,
        };

        const MYSQL: Lexer = Lexer::mysql(true, false, true);

        const fn mysql(backslash_escapes: bool, ansi_quotes: bool, executable_code: bool) -> Lexer {
            Lexer {
                mysql_syntax: true,
                trino: false,
                postgres: false,
                vt_is_space: false,
                backslash_escapes,
                ansi_quotes,
                executable_code,
            }
        }

        const TRINO: Lexer = Lexer {
            mysql_syntax: false,
            trino: true,
            postgres: false,
            vt_is_space: false,
            backslash_escapes: false,
            ansi_quotes: false,
            executable_code: false,
        };

        const POSTGRES: Lexer = Lexer::postgres(false, true);

        const fn postgres(backslash_escapes: bool, vt_is_space: bool) -> Lexer {
            Lexer {
                mysql_syntax: false,
                trino: false,
                postgres: true,
                vt_is_space,
                backslash_escapes,
                ansi_quotes: false,
                executable_code: false,
            }
        }

        const fn dollar_quotes(self) -> bool {
            !self.mysql_syntax && !self.trino
        }

        const fn ident_dollar(self) -> bool {
            self.mysql_syntax || self.postgres
        }

        const fn unicode_ident(self) -> bool {
            self.postgres
        }

        const fn backtick_quotes(self) -> bool {
            !self.postgres && !self.trino
        }

        const fn nested_comments(self) -> bool {
            self.postgres
        }

        const fn cr_ends_line(self) -> bool {
            self.postgres || self.trino
        }

        const fn is_postgres(self) -> bool {
            self.postgres
        }

        const fn hash_comment(self) -> bool {
            self.mysql_syntax
        }

        const fn dash_needs_space(self) -> bool {
            self.mysql_syntax
        }

        const fn single_backslash(self) -> bool {
            self.backslash_escapes
        }

        const fn double_backslash(self) -> bool {
            self.backslash_escapes && !self.ansi_quotes && !self.postgres
        }
    }

    pub static MYSQL_READINGS: [Lexer; 6] = [
        Lexer::mysql(true, false, true),
        Lexer::mysql(true, true, true),
        Lexer::mysql(false, false, true),
        Lexer::mysql(true, false, false),
        Lexer::mysql(true, true, false),
        Lexer::mysql(false, false, false),
    ];

    pub static POSTGRES_READINGS: [Lexer; 4] = [
        Lexer::postgres(false, true),
        Lexer::postgres(true, true),
        Lexer::postgres(false, false),
        Lexer::postgres(true, false),
    ];

    fn dash_opens_comment(bytes: &[u8], index: usize, lexer: Lexer) -> bool {
        if bytes.get(index) != Some(&b'-') || bytes.get(index + 1) != Some(&b'-') {
            return false;
        }
        if !lexer.dash_needs_space() {
            return true;
        }
        match bytes.get(index + 2) {
            None => true,
            Some(&byte) => byte.is_ascii_whitespace() || byte.is_ascii_control(),
        }
    }

    fn executable_comment_opener(bytes: &[u8], index: usize, lexer: Lexer) -> Option<usize> {
        if !lexer.executable_code {
            return None;
        }
        if bytes.get(index) != Some(&b'/')
            || bytes.get(index + 1) != Some(&b'*')
            || bytes.get(index + 2) != Some(&b'!')
        {
            return None;
        }
        let mut next = index + 3;
        while bytes.get(next).is_some_and(u8::is_ascii_digit) {
            next += 1;
        }
        Some(next)
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum State {
        Normal,
        SingleQuoted,
        DoubleQuoted,
        BacktickQuoted,
        LineComment,
        BlockComment,
        DollarQuoted,
        EscapeQuoted,
        UnicodeQuoted,
        BitQuoted,
    }

    pub fn scan_dialect(sql: &str, lexer: Lexer) -> Scan {
        let bytes = sql.as_bytes();
        let mut scan = Scan::default();
        let mut state = State::Normal;
        let mut dollar_tag: Vec<u8> = Vec::new();
        // How many `/*` are open, for a lexer whose block comments nest.
        let mut comment_depth = 0usize;
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
                    b'`' if lexer.backtick_quotes() => {
                        state = State::BacktickQuoted;
                        index += 1;
                    }
                    // An executable comment `/*! … */` is code the server runs, so its
                    // opener is consumed and its body scanned rather than skipped.
                    b'/' if executable_comment_opener(bytes, index, lexer).is_some() => {
                        index =
                            executable_comment_opener(bytes, index, lexer).expect("just checked");
                    }
                    b'-' if dash_opens_comment(bytes, index, lexer) => {
                        state = State::LineComment;
                        index += 2;
                    }
                    b'#' if lexer.hash_comment() => {
                        state = State::LineComment;
                        index += 1;
                    }
                    b'/' if bytes.get(index + 1) == Some(&b'*') => {
                        state = State::BlockComment;
                        comment_depth = 1;
                        index += 2;
                    }
                    b';' => {
                        scan.separators.push(index);
                        index += 1;
                    }
                    // MySQL has no dollar quoting: `$` is an identifier character there, so
                    // `$tag$ ; DELETE … $tag$` is code the server runs, not a string. In
                    // PostgreSQL a `$` seen here starts a token (an identifier swallows its own
                    // `$`), so it can open a dollar quote.
                    b'$' if lexer.dollar_quotes() => {
                        if let Some((tag, next)) = read_dollar_tag(bytes, index, lexer) {
                            dollar_tag = tag;
                            state = State::DollarQuoted;
                            index = next;
                        } else {
                            index += 1;
                        }
                    }
                    byte if is_ident_start(byte, lexer) => {
                        let start = index;
                        // `$` continues an identifier in MySQL and PostgreSQL (`a$update` is
                        // one name, and so is `a$x$`).
                        while index < bytes.len() && is_ident_cont(bytes[index], lexer) {
                            index += 1;
                        }
                        if lexer.postgres && index - start == 1 {
                            // A one-letter prefix directly followed by a quote is a different
                            // kind of string, not an identifier: `E'…'`, `B'…'`, `X'…'`, and
                            // `U&'…'`. (`N'…'` is `N` and then an ordinary string.)
                            let next = match bytes[start].to_ascii_lowercase() {
                                b'e' if bytes.get(index) == Some(&b'\'') => {
                                    Some((State::EscapeQuoted, 1))
                                }
                                b'b' | b'x' if bytes.get(index) == Some(&b'\'') => {
                                    Some((State::BitQuoted, 1))
                                }
                                b'u' if bytes[index..].starts_with(b"&'") => {
                                    Some((State::UnicodeQuoted, 2))
                                }
                                _ => None,
                            };
                            if let Some((quoted, width)) = next {
                                state = quoted;
                                index += width;
                                continue;
                            }
                        }
                        scan.keywords.push(sql[start..index].to_ascii_uppercase());
                    }
                    _ => {
                        index += 1;
                    }
                },
                State::SingleQuoted => {
                    // A backslash escapes the next byte in MySQL, so `'\''` is one quote
                    // inside the string and not the doubled-quote escape — the difference
                    // that hides `'\''; DELETE …` from a generic scan.
                    if lexer.single_backslash() && byte == b'\\' {
                        index += 2;
                    } else if byte == b'\'' {
                        // A doubled quote is an escaped quote, not the end.
                        if bytes.get(index + 1) == Some(&b'\'') {
                            index += 2;
                        } else if let Some(next) = string_continuation(bytes, index + 1, lexer) {
                            index = next;
                        } else {
                            state = State::Normal;
                            index += 1;
                        }
                    } else {
                        index += 1;
                    }
                }
                State::EscapeQuoted => {
                    if byte == b'\\' {
                        index += 2;
                    } else if byte == b'\'' {
                        if bytes.get(index + 1) == Some(&b'\'') {
                            index += 2;
                        } else if let Some(next) = string_continuation(bytes, index + 1, lexer) {
                            index = next;
                        } else {
                            state = State::Normal;
                            index += 1;
                        }
                    } else {
                        index += 1;
                    }
                }
                State::UnicodeQuoted => {
                    if byte == b'\'' {
                        if bytes.get(index + 1) == Some(&b'\'') {
                            index += 2;
                        } else if let Some(next) = string_continuation(bytes, index + 1, lexer) {
                            index = next;
                        } else {
                            state = State::Normal;
                            index += 1;
                        }
                    } else {
                        index += 1;
                    }
                }
                State::BitQuoted => {
                    // No doubling here: `B'1''0'` is two literals, and the second starts a
                    // plain string under whatever rule plain strings follow.
                    if byte == b'\'' {
                        if let Some(next) = string_continuation(bytes, index + 1, lexer) {
                            index = next;
                        } else {
                            state = State::Normal;
                            index += 1;
                        }
                    } else {
                        index += 1;
                    }
                }
                State::DoubleQuoted => {
                    // MySQL's default `"…"` is a string with the same backslash escapes as
                    // `'…'`; generic SQL's `"…"` is a quoted identifier with neither.
                    if lexer.double_backslash() && byte == b'\\' {
                        index += 2;
                    } else if byte == b'"' {
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
                    if ends_line(byte, lexer) {
                        state = State::Normal;
                    }
                    index += 1;
                }
                State::BlockComment => {
                    // PostgreSQL nests `/* /* */ */`; MySQL and generic SQL do not, and reading
                    // one as the other hides or invents a comment end.
                    if lexer.nested_comments()
                        && byte == b'/'
                        && bytes.get(index + 1) == Some(&b'*')
                    {
                        comment_depth += 1;
                        index += 2;
                    } else if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                        comment_depth -= 1;
                        if comment_depth == 0 {
                            state = State::Normal;
                        }
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

        scan.leading_keyword = leading_keyword(sql, lexer);
        scan.ends_with_terminator = sql.trim_end().ends_with(';');
        scan
    }

    fn is_ident_start(byte: u8, lexer: Lexer) -> bool {
        byte.is_ascii_alphabetic() || byte == b'_' || (lexer.unicode_ident() && byte >= 0x80)
    }

    fn is_ident_cont(byte: u8, lexer: Lexer) -> bool {
        byte.is_ascii_alphanumeric()
            || byte == b'_'
            || (byte == b'$' && lexer.ident_dollar())
            || (lexer.unicode_ident() && byte >= 0x80)
    }

    fn ends_line(byte: u8, lexer: Lexer) -> bool {
        byte == b'\n' || (lexer.cr_ends_line() && byte == b'\r')
    }

    fn string_continuation(bytes: &[u8], after: usize, lexer: Lexer) -> Option<usize> {
        if !lexer.postgres {
            return None;
        }
        let mut index = after;
        let is_newline = |byte: u8| byte == b'\n' || byte == b'\r';
        let skip_comment = |mut at: usize| {
            while at < bytes.len() && !is_newline(bytes[at]) {
                at += 1;
            }
            at
        };
        // Horizontal whitespace and comments, then the mandatory newline.
        loop {
            match bytes.get(index) {
                Some(b' ' | b'\t' | b'\x0c') => index += 1,
                Some(b'\x0b') if lexer.vt_is_space => index += 1,
                Some(b'-') if bytes.get(index + 1) == Some(&b'-') => {
                    index = skip_comment(index + 2)
                }
                _ => break,
            }
        }
        if !bytes.get(index).is_some_and(|byte| is_newline(*byte)) {
            return None;
        }
        index += 1;
        // Whitespace, and comments that end in a newline.
        loop {
            match bytes.get(index) {
                Some(b' ' | b'\t' | b'\n' | b'\r' | b'\x0c') => index += 1,
                Some(b'\x0b') if lexer.vt_is_space => index += 1,
                Some(b'-') if bytes.get(index + 1) == Some(&b'-') => {
                    let end = skip_comment(index + 2);
                    if end >= bytes.len() {
                        return None;
                    }
                    index = end + 1;
                }
                _ => break,
            }
        }
        (bytes.get(index) == Some(&b'\'')).then_some(index + 1)
    }

    fn read_dollar_tag(bytes: &[u8], start: usize, lexer: Lexer) -> Option<(Vec<u8>, usize)> {
        debug_assert_eq!(bytes.get(start), Some(&b'$'));
        let mut index = start + 1;
        while index < bytes.len() {
            match bytes[index] {
                b'$' => {
                    let tag = bytes[start..=index].to_vec();
                    return Some((tag, index + 1));
                }
                byte if lexer.postgres => {
                    let letter = byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80;
                    if letter || (index > start + 1 && byte.is_ascii_digit()) {
                        index += 1;
                    } else {
                        return None;
                    }
                }
                byte if byte.is_ascii_alphanumeric() || byte == b'_' => index += 1,
                _ => return None,
            }
        }
        None
    }

    fn comment_end(bytes: &[u8], index: usize, lexer: Lexer) -> Option<usize> {
        let line_end = |mut at: usize| {
            while at < bytes.len() && !ends_line(bytes[at], lexer) {
                at += 1;
            }
            at
        };
        if dash_opens_comment(bytes, index, lexer) {
            return Some(line_end(index + 2));
        }
        if bytes[index] == b'#' && lexer.hash_comment() {
            return Some(line_end(index + 1));
        }
        if bytes[index] == b'/' && bytes.get(index + 1) == Some(&b'*') {
            let mut depth = 1usize;
            let mut at = index + 2;
            while at < bytes.len() {
                if lexer.nested_comments() && bytes[at] == b'/' && bytes.get(at + 1) == Some(&b'*')
                {
                    depth += 1;
                    at += 2;
                } else if bytes[at] == b'*' && bytes.get(at + 1) == Some(&b'/') {
                    depth -= 1;
                    at += 2;
                    if depth == 0 {
                        return Some(at);
                    }
                } else {
                    at += 1;
                }
            }
            return Some(at);
        }
        None
    }

    fn leading_keyword(sql: &str, lexer: Lexer) -> Option<String> {
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
            // An executable comment is code, so its keyword is the leading one: skip only
            // the `/*!…` opener and let the loop find the word inside it.
            if let Some(next) = executable_comment_opener(bytes, index, lexer) {
                index = next;
                continue;
            }
            // Skip comments, repeatedly: `/* a */ -- b\n SELECT 1` has a keyword
            // after two of them.
            if let Some(next) = comment_end(bytes, index, lexer) {
                index = next;
                continue;
            }
            break;
        }

        let start = index;
        if lexer.postgres {
            // The whole identifier, so `select$x` and `select1` are names and not `SELECT`.
            while index < bytes.len() && is_ident_cont(bytes[index], lexer) {
                index += 1;
            }
            if index == start || !is_ident_start(bytes[start], lexer) {
                return None;
            }
        } else {
            while index < bytes.len()
                && (bytes[index].is_ascii_alphabetic() || bytes[index] == b'_')
            {
                index += 1;
            }
            if index == start {
                return None;
            }
        }
        Some(sql[start..index].to_ascii_uppercase())
    }

    pub fn first_significant(sql: &str, lexer: Lexer) -> Option<usize> {
        let bytes = sql.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte.is_ascii_whitespace() || byte == b';' {
                index += 1;
                continue;
            }
            // An executable comment is code, so its body counts as significant: only its
            // opener is skipped, and the word inside it is the first significant one.
            if let Some(next) = executable_comment_opener(bytes, index, lexer) {
                index = next;
                continue;
            }
            if let Some(next) = comment_end(bytes, index, lexer) {
                index = next;
                continue;
            }
            return Some(index);
        }
        None
    }

    pub fn statement_count_dialect(sql: &str, scan: &Scan, lexer: Lexer) -> usize {
        if first_significant(sql, lexer).is_none() {
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
    /// The frozen lexer that matches a public one. The public lexers are the two constants and
    /// the readings of MySQL and PostgreSQL (their fields are private), so the pairing is by
    /// position in `Dialect::readings()`, which `readings_pair_up` checks against the constants.
    pub fn lexer_for(lexer: qh_sql::Lexer) -> Lexer {
        use qh_sql::Dialect;
        if lexer == qh_sql::Lexer::GENERIC {
            return Lexer::GENERIC;
        }
        if lexer == qh_sql::Lexer::TRINO {
            return Lexer::TRINO;
        }
        if let Some(at) = Dialect::Mysql.readings().iter().position(|l| *l == lexer) {
            return MYSQL_READINGS[at];
        }
        if let Some(at) = Dialect::Postgres
            .readings()
            .iter()
            .position(|l| *l == lexer)
        {
            return POSTGRES_READINGS[at];
        }
        panic!("a lexer this file does not know: {lexer:?}");
    }
}

/// Every lexer a connection can be read under: Generic, Trino, MySQL's six, PostgreSQL's four.
fn all_lexers() -> Vec<Lexer> {
    let mut lexers = vec![Lexer::GENERIC, Lexer::TRINO];
    lexers.extend_from_slice(Dialect::Mysql.readings());
    lexers.extend_from_slice(Dialect::Postgres.readings());
    lexers
}

/// SplitMix64: seeded, no dependency, and the same stream on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// The pieces random inputs are built from: every byte sequence a lexer decides something
/// about, plus words the classifier looks for and multi-byte characters.
const ALPHABET: &[&str] = &[
    "'", "''", "\"", "\"\"", "`", "--", "--x", "-- ", "/*", "*/", "/*!", "/*!50000", "/*M!", "/*+",
    "$", "$$", "$tag$", "$x$", "$1", ";", ";", "\n", "\r", "\r\n", "\\", "\\'", "\\\"", "#", "_",
    "a", "e", "E", "E'", "b", "B'", "x", "X'", "u&'", "U&'", "N'", "select", "SELECT", "delete",
    "DROP", "from", "t", "1", "42", "é", "😀", " ", " ", " ", "\t", "\x0b", "\x0c", "&", "-", "*",
    "/", "!", "(", ")", ",", "=",
];

fn random_sql(rng: &mut Rng) -> String {
    let pieces = rng.below(41);
    let mut sql = String::new();
    for _ in 0..pieces {
        sql.push_str(ALPHABET[rng.below(ALPHABET.len())]);
    }
    sql
}

/// Fixed payloads: the ones the scanner's own tests and W3-T0/W3-T0b were written around.
const PAYLOADS: &[&str] = &[
    "SELECT '\\''; DELETE FROM t; -- '",
    "SELECT '\\''; DELETE FROM t; # '",
    "SELECT \"\\\"\"; DELETE FROM t; # \"",
    "SELECT 1 # '\n; DELETE FROM t; -- '",
    "SELECT 1 /*! ; DROP TABLE t */",
    "SELECT 1 /*!50000 ; DELETE FROM t */",
    "SELECT 1 /*M! ; DROP TABLE t */",
    "SELECT $$; DELETE FROM t; $$",
    "SELECT $x$ DELETE FROM t $x$; select 1",
    "DO $body$ SELECT 1; SELECT 2; $body$",
    "SELECT $1; SELECT $2",
    "SELECT a$b; DELETE FROM t; $b$",
    "SELECT E'\\'' ; DELETE FROM t; --'",
    "SELECT 'a'\n'b'; DELETE FROM t",
    "SELECT 'a' -- c\n -- d\n 'b'; DELETE FROM t",
    "SELECT 'a'\x0b\n'b'; DELETE FROM t",
    "SELECT U&'\\'; DELETE FROM t; --'",
    "SELECT B'1''0'; DELETE FROM t",
    "SELECT X'1F'; SELECT 2",
    "SELECT /* a /* b */ ; DELETE FROM t; */ 1",
    "SELECT 1 -- x\r; DELETE FROM t",
    "SELECT 1 --x; DELETE FROM t",
    "SELECT `a;b`; SELECT \"a;b\"",
    "SELECT 'unterminated; DELETE FROM t",
    "SELECT 'a\\",
    "/* unterminated ; DELETE FROM t",
    "",
    ";",
    "  ;  ;  ",
    "-- only a comment",
    "# only a comment",
    "é; SELECT 😀; \u{a0};",
];

/// The repository's own SQL: dev seeds and the golden corpus's cases.
const FILES: &[&str] = &[
    include_str!("../../../deploy/dev/seed-mysql.sql"),
    include_str!("../../../deploy/dev/seed-postgres.sql"),
    include_str!("../../../deploy/dev/qh-mysql-old-seed-prefixed.sql"),
    include_str!("../../../tools/golden/live_cases.py"),
];

/// One input, every lexer: the live scanner and the frozen one must agree on everything the
/// scanner exposes.
fn assert_same(sql: &str, lexers: &[Lexer]) {
    for &lexer in lexers {
        let old_lexer = frozen::lexer_for(lexer);
        let old = frozen::scan_dialect(sql, old_lexer);
        let new = scan_dialect(sql, lexer);
        assert_eq!(new, old, "Scan differs for {sql:?} under {lexer:?}");
        assert_eq!(
            first_significant(sql, lexer),
            frozen::first_significant(sql, old_lexer),
            "first_significant differs for {sql:?} under {lexer:?}"
        );
        assert_eq!(
            statement_count_dialect(sql, &new, lexer),
            frozen::statement_count_dialect(sql, &old, old_lexer),
            "statement_count differs for {sql:?} under {lexer:?}"
        );
    }
}

#[test]
fn readings_pair_up() {
    assert_eq!(
        Dialect::Mysql.readings().len(),
        frozen::MYSQL_READINGS.len()
    );
    assert_eq!(
        Dialect::Postgres.readings().len(),
        frozen::POSTGRES_READINGS.len()
    );
    // Same position, same fields: the two types print alike when they describe one lexer.
    for (live, old) in Dialect::Mysql
        .readings()
        .iter()
        .zip(frozen::MYSQL_READINGS.iter())
    {
        assert_eq!(format!("{live:?}"), format!("{old:?}"));
    }
    for (live, old) in Dialect::Postgres
        .readings()
        .iter()
        .zip(frozen::POSTGRES_READINGS.iter())
    {
        assert_eq!(format!("{live:?}"), format!("{old:?}"));
    }
    assert_eq!(all_lexers().len(), 12);
    // The pairing must reach a frozen lexer for every public one without panicking.
    for lexer in all_lexers() {
        frozen::lexer_for(lexer);
    }
}

#[test]
fn payloads_and_repository_sql_scan_identically() {
    let lexers = all_lexers();
    for sql in PAYLOADS {
        assert_same(sql, &lexers);
    }
    for file in FILES {
        assert_same(file, &lexers);
        // Whole file, each line, and each `;`-terminated chunk: a quote opened in the
        // middle of a file behaves differently from the same text at the start of one.
        for line in file.lines() {
            assert_same(line, &lexers);
        }
        for chunk in file.split_inclusive(';') {
            assert_same(chunk, &lexers);
        }
    }
}

#[test]
fn random_inputs_scan_identically() {
    let cases = if std::env::var_os("QH_SCAN_SOAK").is_some() {
        1_000_000
    } else {
        10_000
    };
    let lexers = all_lexers();
    let mut rng = Rng(0x5CA9_0A11_C0DE);
    for _ in 0..cases {
        assert_same(&random_sql(&mut rng), &lexers);
    }
}

/// FNV-1a, folded over the debug rendering of every result.
struct Digest(u64);

impl Digest {
    fn add(&mut self, text: &str) {
        for byte in text.bytes().chain([0xff]) {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

fn add_surface(digest: &mut Digest, sql: &str) {
    for dialect in [
        Dialect::Generic,
        Dialect::Mysql,
        Dialect::Postgres,
        Dialect::Trino,
    ] {
        let readings = dialect.readings();
        for mode in [
            SafeMode::Full,
            SafeMode::NoDdl,
            SafeMode::Confirm,
            SafeMode::ReadOnly,
        ] {
            digest.add(&format!("{:?}", decisions_readings(mode, sql, readings)));
            for confirmed in [false, true] {
                digest.add(&format!(
                    "{:?}",
                    check_confirmed_readings(mode, confirmed, sql, readings)
                ));
            }
        }
        digest.add(&format!("{:?}", classify_readings(sql, readings)));
        digest.add(&format!("{:?}", statements_agreeing(sql, readings)));
        for &lexer in readings {
            digest.add(&format!("{:?}", statements_with_lines_dialect(sql, lexer)));
            digest.add(&format!("{:?}", count_statement_dialect(sql, lexer)));
            digest.add(&strip_terminator_dialect(sql, lexer));
        }
    }
}

/// The digest of the Safe Mode surface over `PAYLOADS`, `FILES` and 3,000 seeded inputs,
/// computed with the scanner before the `walk` extraction (a25fa50). `SAFE_MODES` is folded
/// in so a renamed mode does not slip through as an unrelated change.
///
/// `FILES` includes `tools/golden/live_cases.py`, so the digest has to be taken again whenever
/// that file changes: it was, on 6 Oct 2026 (W11-T1, nine metadata cases) and again the same day
/// (the harness gets a throwaway `DB_PATH`, six lines of Python and no SQL). Nothing the scanner
/// or the classifier does moved with either: the payloads, the seeds and the random inputs are the
/// same, `payloads_and_repository_sql_scan_identically` reads the new text of that file under
/// every lexer and finds the frozen scanner agreeing, and `qh-sql`'s sources are untouched.
const SURFACE_DIGEST: u64 = 0x6e16_c1bd_0d82_fdfb;

#[test]
fn the_safe_mode_surface_is_unchanged() {
    let mut digest = Digest(0xcbf2_9ce4_8422_2325);
    digest.add(&SAFE_MODES.join(","));
    for sql in PAYLOADS {
        add_surface(&mut digest, sql);
    }
    for file in FILES {
        add_surface(&mut digest, file);
    }
    let mut rng = Rng(0x00DD_BA11);
    for _ in 0..3_000 {
        add_surface(&mut digest, &random_sql(&mut rng));
    }
    assert_eq!(
        digest.0, SURFACE_DIGEST,
        "the Safe Mode surface changed: got {:#018x}",
        digest.0
    );
}

/// `cargo test --release -p qh-sql --test scan_refactor -- --ignored --nocapture`: the cost of
/// the refactor on the big bench corpora (the blueprint allows at most 10% on `scan()`).
/// Reads the corpora from `target/run/ts-bench/corpus` when they exist.
#[test]
#[ignore = "timing, run by hand"]
fn scan_is_not_slower_than_the_frozen_scanner() {
    use std::time::Instant;
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/run/ts-bench/corpus");
    for name in ["chars-2m.sql", "bench-2m.sql", "dump-2m.sql"] {
        let Ok(sql) = std::fs::read_to_string(dir.join(name)) else {
            eprintln!("{name}: corpus missing, skipped");
            continue;
        };
        for lexer in [Lexer::GENERIC, Lexer::MYSQL, Lexer::POSTGRES] {
            let old_lexer = frozen::lexer_for(lexer);
            let best = |mut run: Box<dyn FnMut() -> Scan>| {
                (0..15)
                    .map(|_| {
                        let start = Instant::now();
                        std::hint::black_box(run());
                        start.elapsed().as_secs_f64() * 1e3
                    })
                    .fold(f64::MAX, f64::min)
            };
            let old_ms = best(Box::new(|| frozen::scan_dialect(&sql, old_lexer)));
            let new_ms = best(Box::new(|| scan_dialect(&sql, lexer)));
            eprintln!(
                "{name} {lexer:?}: frozen {old_ms:.2} ms, live {new_ms:.2} ms ({:+.1}%)",
                (new_ms / old_ms - 1.0) * 100.0
            );
        }
    }
}
