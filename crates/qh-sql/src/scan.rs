//! One pass over a statement that knows where text is text and where it is
//! syntax.

/// The SQL family whose lexical rules a connection's SQL is read under.
///
/// The scanner has to agree with the server about where a string ends and where a
/// comment starts, because a `;` or a keyword the server sees as *syntax* is what a
/// Safe Mode has to classify. Two families here read the same bytes differently, so a
/// single set of rules would be right for one and wrong — and therefore unsafe — for
/// the other.
///
/// A dialect is a *family*; the concrete rules a scan follows are a [`Lexer`], and
/// [`Dialect::readings`] lists every lexer a guard has to satisfy at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dialect {
    /// The ANSI default, for a caller with no server to read it for: no backslash escapes
    /// inside a string, `--` opens a comment whatever follows it and ends at a newline,
    /// `#` is an operator and not a comment, `/* … */` is always a comment, and
    /// `$tag$ … $tag$` is a dollar-quoted string. No connection reads as this any more:
    /// PostgreSQL, MySQL and Trino each have a dialect of their own, because each lexes
    /// something differently and the difference hid a write from a guard.
    #[default]
    Generic,
    /// MySQL. Its lexical rules depend on the session's `sql_mode`, which is not
    /// knowable before the guard runs, so a MySQL connection is read under every lexer
    /// the server can actually use (see [`Lexer`] and [`Dialect::readings`]) and refused
    /// when any of them sees something the mode forbids or when they disagree.
    Mysql,
    /// PostgreSQL, read the way its lexer (`scan.l`) reads: `E'…'` strings honour backslash
    /// escapes, plain `'…'` honours them only when `standard_conforming_strings` is off,
    /// block comments nest, a `--` comment ends at `\r` as well as `\n`, `$` continues an
    /// identifier, and a dollar quote only starts where a token starts. A setting the guard
    /// cannot see moves where a string ends, so a PostgreSQL connection is read under every
    /// lexer the server can be using ([`Dialect::readings`]), like MySQL.
    Postgres,
    /// Trino: a `--` comment ends at `\r` as well as `\n`, `/* … */` does not nest, there are
    /// no dollar quotes and no backslash escapes (a `$` is a syntax error there, so text after
    /// one is read as code, which can only refuse more), and a backtick is not a quote.
    /// `X'…'` ends at its first quote, which reads the same as a doubled one here because a
    /// plain string has no backslash.
    Trino,
}

/// One concrete set of lexical rules: what a single scan follows.
///
/// A MySQL lexer always uses MySQL's comment rules — `#` opens a line comment, `-- `
/// only opens one when whitespace or a control character follows the dashes, `/* … */`
/// is a comment, `/*M! … */` is an ordinary comment (that spelling is MariaDB's) — and
/// has **no dollar quoting**: `$` is an identifier character there, so `$tag$ ; DELETE
/// … $tag$` is code the server runs, not a string. What varies between MySQL lexers is
/// what the server's `sql_mode` and version decide:
///
/// * `NO_BACKSLASH_ESCAPES` on or off: whether a backslash escapes the next byte inside
///   a string;
/// * `ANSI_QUOTES` on or off: whether `"…"` is a string (with the backslash rule above)
///   or a quoted identifier (where a backslash is an ordinary byte);
/// * an executable comment `/*! … */` or a version-gated `/*!50000 … */` whose body the
///   server either runs (its version is high enough) or skips (it is not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lexer {
    /// MySQL comment rules and no dollar quoting.
    mysql_syntax: bool,
    /// Trino's lexer: generic, with `--` ending at `\r`, no dollar quotes and no backticks.
    trino: bool,
    /// PostgreSQL's lexer: nested block comments, `E'…'`/`U&'…'`/`B'…'` prefixes, `$` in an
    /// identifier, a `--` comment ending at `\r`, string continuation across a newline, and
    /// no backtick quoting.
    postgres: bool,
    /// A backslash escapes the next byte inside a plain `'…'` string (MySQL without
    /// `NO_BACKSLASH_ESCAPES`; PostgreSQL with `standard_conforming_strings = off`).
    backslash_escapes: bool,
    /// PostgreSQL only: `\v` counts as whitespace between the pieces of a continued string
    /// (it does on newer servers, not on older ones).
    vt_is_space: bool,
    /// `"…"` is a quoted identifier, so a backslash inside it is an ordinary byte.
    ansi_quotes: bool,
    /// The body of `/*! … */` is scanned as code rather than skipped as a comment.
    executable_code: bool,
}

impl Lexer {
    /// PostgreSQL, Trino and the ANSI default.
    pub const GENERIC: Lexer = Lexer {
        mysql_syntax: false,
        trino: false,
        postgres: false,
        vt_is_space: false,
        backslash_escapes: false,
        ansi_quotes: false,
        executable_code: false,
    };

    /// MySQL under its default `sql_mode` on a server new enough to run every
    /// executable comment: backslash escapes on, `"` a string, `/*! … */` code.
    pub const MYSQL: Lexer = Lexer::mysql(true, false, true);

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

    /// Trino.
    pub const TRINO: Lexer = Lexer {
        mysql_syntax: false,
        trino: true,
        postgres: false,
        vt_is_space: false,
        backslash_escapes: false,
        ansi_quotes: false,
        executable_code: false,
    };

    /// PostgreSQL with `standard_conforming_strings` on and a server new enough to read `\v`
    /// as whitespace: the reading a current server uses by default.
    pub const POSTGRES: Lexer = Lexer::postgres(false, true);

    /// One PostgreSQL lexer. `backslash_escapes` is `standard_conforming_strings = off`.
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

    /// Whether the body of `/*! … */` is code to this lexer.
    pub(crate) const fn runs_executable_comments(self) -> bool {
        self.executable_code
    }

    /// Whether `$tag$ … $tag$` is a quoted string.
    const fn dollar_quotes(self) -> bool {
        !self.mysql_syntax && !self.trino
    }

    /// Whether `$` continues an identifier (`a$x` is one name) instead of being punctuation.
    const fn ident_dollar(self) -> bool {
        self.mysql_syntax || self.postgres
    }

    /// Whether a byte at or above 0x80 belongs to an identifier.
    const fn unicode_ident(self) -> bool {
        self.postgres
    }

    /// Whether `` ` `` quotes an identifier (it is an operator character in PostgreSQL).
    const fn backtick_quotes(self) -> bool {
        !self.postgres && !self.trino
    }

    /// Whether `/* … */` nests.
    const fn nested_comments(self) -> bool {
        self.postgres
    }

    /// Whether a carriage return ends a `--` comment as a newline does.
    const fn cr_ends_line(self) -> bool {
        self.postgres || self.trino
    }

    /// Whether this is one of PostgreSQL's lexers.
    pub(crate) const fn is_postgres(self) -> bool {
        self.postgres
    }

    /// Whether `#` opens a line comment.
    const fn hash_comment(self) -> bool {
        self.mysql_syntax
    }

    /// Whether `--` needs a following whitespace/control character to open a comment.
    const fn dash_needs_space(self) -> bool {
        self.mysql_syntax
    }

    /// Whether a backslash escapes the next byte inside `'…'`.
    const fn single_backslash(self) -> bool {
        self.backslash_escapes
    }

    /// Whether a backslash escapes the next byte inside `"…"`.
    const fn double_backslash(self) -> bool {
        self.backslash_escapes && !self.ansi_quotes && !self.postgres
    }
}

impl From<Dialect> for Lexer {
    /// The dialect's default reading: [`Lexer::GENERIC`] or [`Lexer::MYSQL`]. A guard
    /// uses [`Dialect::readings`] instead, which is the whole set.
    fn from(dialect: Dialect) -> Lexer {
        match dialect {
            Dialect::Generic => Lexer::GENERIC,
            Dialect::Mysql => Lexer::MYSQL,
            Dialect::Postgres => Lexer::POSTGRES,
            Dialect::Trino => Lexer::TRINO,
        }
    }
}

/// The one reading a generic connection must satisfy.
static GENERIC_READINGS: [Lexer; 1] = [Lexer::GENERIC];

/// The one reading a Trino connection must satisfy.
static TRINO_READINGS: [Lexer; 1] = [Lexer::TRINO];

/// Every lexer a MySQL server can be using, so a guard can satisfy all of them at once.
///
/// Backslash escapes {on, off} × `"` {string, identifier} × executable-comment body
/// {code, skipped}. With backslash escapes off, `"` as a string and as an identifier scan
/// identically (the only difference between them is the backslash rule), so that
/// combination appears once: six distinct lexers, not eight.
static MYSQL_READINGS: [Lexer; 6] = [
    Lexer::mysql(true, false, true),
    Lexer::mysql(true, true, true),
    Lexer::mysql(false, false, true),
    Lexer::mysql(true, false, false),
    Lexer::mysql(true, true, false),
    Lexer::mysql(false, false, false),
];

/// Every lexer a PostgreSQL server can be using: `standard_conforming_strings` {on, off}
/// (a session can change it, and the guard cannot see it) × `\v` {whitespace, not} (a
/// server-version difference that only matters between the pieces of a continued string).
static POSTGRES_READINGS: [Lexer; 4] = [
    Lexer::postgres(false, true),
    Lexer::postgres(true, true),
    Lexer::postgres(false, false),
    Lexer::postgres(true, false),
];

impl Dialect {
    /// The lexers a guard must satisfy at once for a connection of this dialect.
    /// Generic has one; MySQL has six and PostgreSQL four, because the server's settings
    /// and version are not knowable before the guard runs and they move where a string or
    /// a comment ends.
    pub const fn readings(self) -> &'static [Lexer] {
        match self {
            Dialect::Generic => &GENERIC_READINGS,
            Dialect::Mysql => &MYSQL_READINGS,
            Dialect::Postgres => &POSTGRES_READINGS,
            Dialect::Trino => &TRINO_READINGS,
        }
    }
}

/// Whether a `--` at `index` opens a line comment under `lexer`.
///
/// Generic SQL always treats `--` as a comment. MySQL requires the two dashes to be
/// followed by a whitespace or control character (or the end of input), so `SELECT 1-->2`
/// is arithmetic there and a comment in PostgreSQL.
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

/// Skip an executable-comment opener (`/*!` or a version-gated `/*!50000`) at `index`,
/// returning the offset just past it and any version digits, or `None` when this lexer
/// skips executable comments or this is an ordinary `/* … */` comment (including a
/// `/*+ … */` optimizer hint and MariaDB's `/*M! … */`, which MySQL treats as plain).
///
/// The body of an executable comment is left to be scanned as code — the server runs
/// it — so only the opener is consumed here. The closing `*/` needs no special case:
/// in `Normal` state a `*` and a `/` are punctuation the scanner ignores.
pub(crate) fn executable_comment_opener(bytes: &[u8], index: usize, lexer: Lexer) -> Option<usize> {
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
    /// Every bare word, uppercased, in order.
    ///
    /// A word is a run of letters, digits and `_` that starts with a letter or `_`
    /// and is **outside** a literal, a quoted identifier, a comment or a
    /// dollar-quoted block — the same places [`Scan::separators`] ignores. It is
    /// what lets [`crate::classify`] look for a data-modifying keyword anywhere in
    /// a statement without a second scanner that could disagree with this one.
    ///
    /// Digits and `_` are part of a word, so `updated_at` is one word
    /// (`UPDATED_AT`) and not `UPDATE` followed by junk. That is deliberate: a
    /// keyword match must be exact to be safe, and a column whose name merely
    /// contains a keyword is not that keyword.
    pub keywords: Vec<String>,
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
    /// Inside PostgreSQL's `E'...'`: a backslash escapes the next byte, always.
    EscapeQuoted,
    /// Inside PostgreSQL's `U&'...'`: a doubled quote is a quote, and a backslash is text.
    UnicodeQuoted,
    /// Inside PostgreSQL's `B'...'` or `X'...'`: the first quote ends it, doubled or not.
    BitQuoted,
}

/// Scan one possibly multi-statement string, using generic (ANSI) lexical rules.
///
/// Kept for PostgreSQL, Trino and every caller that predates the dialect split; it is
/// [`scan_dialect`] with [`Dialect::Generic`].
pub fn scan(sql: &str) -> Scan {
    scan_dialect(sql, Dialect::Generic)
}

/// Scan one possibly multi-statement string under one lexer's rules (a [`Dialect`] means
/// its default reading; a guard uses [`Dialect::readings`]).
///
/// The scanner is deliberately permissive: SQL that does not parse still scans.
/// The reason is that one caller is building an error position for the server to
/// object to, so the input is frequently invalid by definition, and refusing to
/// scan it would turn a useful error message into a second error.
pub fn scan_dialect(sql: &str, dialect: impl Into<Lexer>) -> Scan {
    let lexer: Lexer = dialect.into();
    let mut collect = Collect {
        sql,
        scan: Scan::default(),
    };
    walk(sql.as_bytes(), lexer, &mut collect);
    let mut scan = collect.scan;
    scan.leading_keyword = leading_keyword(sql, lexer);
    scan.ends_with_terminator = sql.trim_end().ends_with(';');
    scan
}

/// What [`scan_dialect`] gathers from a [`walk`]: the separators and the bare words.
struct Collect<'a> {
    sql: &'a str,
    scan: Scan,
}

impl Visitor for Collect<'_> {
    fn separator(&mut self, at: usize) {
        self.scan.separators.push(at);
    }

    fn word(&mut self, start: usize, end: usize) {
        self.scan
            .keywords
            .push(self.sql[start..end].to_ascii_uppercase());
    }
}

/// The kind of text a [`Visitor`] is told is opaque: text that is not syntax, so a `;` or a
/// keyword inside it means nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpaqueKind {
    /// A `'…'` string, including PostgreSQL's `E'…'`, `U&'…'`, `B'…'` and `X'…'` (the prefix is
    /// part of the region) and a string continued across a newline.
    SingleQuote,
    /// A `"…"` string or quoted identifier.
    DoubleQuote,
    /// A `` `…` `` quoted identifier.
    Backtick,
    /// A `-- …` or MySQL `# …` comment, up to but not including its line end.
    LineComment,
    /// A `/* … */` comment. A MySQL executable comment (`/*! … */`) is code, not this.
    BlockComment,
    /// A PostgreSQL `$tag$ … $tag$` string, tags included.
    DollarQuote,
}

/// Where a [`walk`] stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndState {
    /// Outside every opaque region.
    Normal,
    /// The text ended inside this region (a quote never closed, a block comment never
    /// ended, a line comment with no newline after it).
    Open(OpaqueKind),
}

/// What a [`walk`] reports. Every method has an empty default, so a visitor takes only what
/// it needs. Offsets are byte offsets into the slice `walk` was given, and every region is
/// reported once, in order, when it ends (or at the end of the text if it never does).
pub trait Visitor {
    /// A `;` outside every opaque region: a real statement separator.
    fn separator(&mut self, _at: usize) {}
    /// An opaque region `[start, end)`. `end` is the text's length for a region left open.
    fn opaque(&mut self, _kind: OpaqueKind, _start: usize, _end: usize) {}
    /// A bare word `[start, end)`: letters, digits and `_` (and `$`, and bytes at or above
    /// 0x80 where the lexer's identifiers allow them) that start with a letter or `_`,
    /// outside every opaque region.
    fn word(&mut self, _start: usize, _end: usize) {}
}

/// Walk `bytes` once under one lexer's rules, telling `visitor` where the separators, the
/// opaque regions and the bare words are. This is the single pass every scan is made of;
/// [`scan_dialect`] is a [`Visitor`] over it, and the editor's lexer is another.
///
/// Permissive like [`scan_dialect`]: any bytes are accepted, and text that is not valid SQL
/// still walks. It never panics on its own account and holds no state between calls.
pub fn walk(bytes: &[u8], dialect: impl Into<Lexer>, visitor: &mut impl Visitor) -> EndState {
    let lexer: Lexer = dialect.into();
    let mut state = State::Normal;
    // Where the region being read began and what it is, for `Visitor::opaque`. Set on every
    // move out of `Normal`; the initial values are never reported.
    let mut region_start = 0usize;
    let mut region_kind = OpaqueKind::SingleQuote;
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
                    region_start = index;
                    region_kind = OpaqueKind::SingleQuote;
                    index += 1;
                }
                b'"' => {
                    state = State::DoubleQuoted;
                    region_start = index;
                    region_kind = OpaqueKind::DoubleQuote;
                    index += 1;
                }
                b'`' if lexer.backtick_quotes() => {
                    state = State::BacktickQuoted;
                    region_start = index;
                    region_kind = OpaqueKind::Backtick;
                    index += 1;
                }
                // An executable comment `/*! … */` is code the server runs, so its
                // opener is consumed and its body scanned rather than skipped.
                b'/' if executable_comment_opener(bytes, index, lexer).is_some() => {
                    index = executable_comment_opener(bytes, index, lexer).expect("just checked");
                }
                b'-' if dash_opens_comment(bytes, index, lexer) => {
                    state = State::LineComment;
                    region_start = index;
                    region_kind = OpaqueKind::LineComment;
                    index += 2;
                }
                b'#' if lexer.hash_comment() => {
                    state = State::LineComment;
                    region_start = index;
                    region_kind = OpaqueKind::LineComment;
                    index += 1;
                }
                b'/' if bytes.get(index + 1) == Some(&b'*') => {
                    state = State::BlockComment;
                    region_start = index;
                    region_kind = OpaqueKind::BlockComment;
                    comment_depth = 1;
                    index += 2;
                }
                b';' => {
                    visitor.separator(index);
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
                        region_start = index;
                        region_kind = OpaqueKind::DollarQuote;
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
                            region_start = start;
                            region_kind = OpaqueKind::SingleQuote;
                            index += width;
                            continue;
                        }
                    }
                    visitor.word(start, index);
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
                        index += 1;
                        visitor.opaque(region_kind, region_start, index);
                        state = State::Normal;
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
                        index += 1;
                        visitor.opaque(region_kind, region_start, index);
                        state = State::Normal;
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
                        index += 1;
                        visitor.opaque(region_kind, region_start, index);
                        state = State::Normal;
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
                        index += 1;
                        visitor.opaque(region_kind, region_start, index);
                        state = State::Normal;
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
                        index += 1;
                        visitor.opaque(region_kind, region_start, index);
                        state = State::Normal;
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
                        index += 1;
                        visitor.opaque(region_kind, region_start, index);
                        state = State::Normal;
                    }
                } else {
                    index += 1;
                }
            }
            State::LineComment => {
                // The line end is not part of the comment; the next byte is code.
                if ends_line(byte, lexer) {
                    visitor.opaque(region_kind, region_start, index);
                    state = State::Normal;
                }
                index += 1;
            }
            State::BlockComment => {
                // PostgreSQL nests `/* /* */ */`; MySQL and generic SQL do not, and reading
                // one as the other hides or invents a comment end.
                if lexer.nested_comments() && byte == b'/' && bytes.get(index + 1) == Some(&b'*') {
                    comment_depth += 1;
                    index += 2;
                } else if byte == b'*' && bytes.get(index + 1) == Some(&b'/') {
                    comment_depth -= 1;
                    index += 2;
                    if comment_depth == 0 {
                        visitor.opaque(region_kind, region_start, index);
                        state = State::Normal;
                    }
                } else {
                    index += 1;
                }
            }
            State::DollarQuoted => {
                if byte == b'$' && bytes[index..].starts_with(&dollar_tag) {
                    index += dollar_tag.len();
                    visitor.opaque(region_kind, region_start, index);
                    state = State::Normal;
                } else {
                    index += 1;
                }
            }
        }
    }

    if state == State::Normal {
        EndState::Normal
    } else {
        visitor.opaque(region_kind, region_start, bytes.len());
        EndState::Open(region_kind)
    }
}

/// Whether `byte` starts an identifier under `lexer`.
fn is_ident_start(byte: u8, lexer: Lexer) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || (lexer.unicode_ident() && byte >= 0x80)
}

/// Whether `byte` continues an identifier under `lexer`.
fn is_ident_cont(byte: u8, lexer: Lexer) -> bool {
    byte.is_ascii_alphanumeric()
        || byte == b'_'
        || (byte == b'$' && lexer.ident_dollar())
        || (lexer.unicode_ident() && byte >= 0x80)
}

/// Whether `byte` ends a `--`/`#` line comment under `lexer`.
fn ends_line(byte: u8, lexer: Lexer) -> bool {
    byte == b'\n' || (lexer.cr_ends_line() && byte == b'\r')
}

/// PostgreSQL's string continuation: `'a'` + whitespace containing a newline + `'b'` is one
/// string, and the second piece is read under the *first* piece's rules (so an `E'…'`
/// string continues as an `E` string whatever the plain-string setting is). `after` is the
/// offset just past the closing quote; the result is the offset just past the continuing
/// quote. Only PostgreSQL's lexers continue a string this way.
///
/// The whitespace is `scan.l`'s `quotecontinue`: horizontal whitespace or `--` comments, a
/// newline, then any whitespace and comments that each end in a newline. Measured against a
/// PostgreSQL 17 server, where `\v` counts as whitespace on both sides of the newline.
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
            Some(b'-') if bytes.get(index + 1) == Some(&b'-') => index = skip_comment(index + 2),
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

/// Read a `$tag$` opener at `start`, returning the tag (including both `$`) and
/// the offset just past it.
///
/// The tag must be followed by end-of-input or a non-identifier byte, so that a
/// `$1` placeholder — which Postgres uses for parameters and which is not a
/// dollar quote — is not mistaken for one. PostgreSQL's own rule is stricter about the
/// tag: it may not start with a digit, and a byte at or above 0x80 is a letter.
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

/// If a comment starts at `index`, the offset just past it (a line comment stops at its
/// newline, which is left for the caller).
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
            if lexer.nested_comments() && bytes[at] == b'/' && bytes.get(at + 1) == Some(&b'*') {
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

/// The first bare keyword, uppercased.
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
        while index < bytes.len() && (bytes[index].is_ascii_alphabetic() || bytes[index] == b'_') {
            index += 1;
        }
        if index == start {
            return None;
        }
    }
    Some(sql[start..index].to_ascii_uppercase())
}

/// Whether anything in `sql` is more than whitespace, a comment, or a `;`.
///
/// Used to tell "no statement at all" from "one statement". A string of only
/// separators and comments is not a statement.
pub fn has_significant_text(sql: &str) -> bool {
    first_significant(sql, Dialect::Generic).is_some()
}

/// [`has_significant_text`] under `dialect`'s comment rules.
pub(crate) fn has_significant_text_dialect(sql: &str, dialect: impl Into<Lexer>) -> bool {
    first_significant(sql, dialect).is_some()
}

/// The byte offset of the first significant byte — not whitespace, not a
/// comment, not a `;` — or `None` when there is none.
///
/// The offset is what lets a caller name the *line* a statement starts on even
/// when the piece it was split into opens with a header comment. Sharing the
/// skip logic with [`has_significant_text`] is the point: the two cannot
/// disagree about where the real text begins.
pub fn first_significant(sql: &str, dialect: impl Into<Lexer>) -> Option<usize> {
    let lexer: Lexer = dialect.into();
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
    statement_count_dialect(sql, scan, Dialect::Generic)
}

/// [`statement_count`] under `dialect`'s comment rules.
///
/// `scan` must be a [`scan_dialect`] of `sql` under the same `dialect`, so the count of
/// separators and the "is there any statement at all" check agree about what is a
/// comment.
pub fn statement_count_dialect(sql: &str, scan: &Scan, dialect: impl Into<Lexer>) -> usize {
    if !has_significant_text_dialect(sql, dialect) {
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
    fn bare_words_are_collected_from_syntax_but_not_from_text() {
        // The words a classifier may act on: exact, uppercased, and only where they
        // are syntax.
        assert_eq!(
            scan("select * from t where updated_at > 1").keywords,
            vec!["SELECT", "FROM", "T", "WHERE", "UPDATED_AT"]
        );
        // A `;` and a keyword inside a literal, a quoted identifier, a comment and a
        // dollar-quoted body are all text, so none of them is a word.
        assert_eq!(
            scan("SELECT 'DROP TABLE', \"delete\", `insert` /* update */ -- merge\n").keywords,
            vec!["SELECT"]
        );
        assert_eq!(scan("DO $body$ DELETE FROM t; $body$").keywords, vec!["DO"]);
        // Digits and underscores stay inside a word, so a column named `drop2` and a
        // table named `created_at` are not `DROP` or `CREATE`.
        assert_eq!(
            scan("select drop2, created_at").keywords,
            vec!["SELECT", "DROP2", "CREATED_AT"]
        );
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

    fn separators_my(sql: &str) -> Vec<usize> {
        scan_dialect(sql, Dialect::Mysql).separators
    }

    #[test]
    fn a_mysql_backslash_escapes_the_quote_that_generic_reads_as_a_terminator() {
        // The injection: `'\''` is one quote inside the string in MySQL, so the string
        // ends at the third quote and the `; DELETE …; ` after it is code. A generic
        // scan reads `''` as a doubled quote, keeps the string open, and never sees the
        // separators or the DELETE — which is the bug.
        let payload = "SELECT '\\''; DELETE FROM t; -- '";
        assert_eq!(scan(payload).separators, Vec::<usize>::new());
        assert!(!scan(payload).keywords.contains(&"DELETE".to_owned()));
        assert_eq!(separators_my(payload).len(), 2);
        assert!(scan_dialect(payload, Dialect::Mysql)
            .keywords
            .contains(&"DELETE".to_owned()));
        // The same through a double-quoted string, which MySQL's default reads as a
        // string too.
        let payload = "SELECT \"\\\"\"; DELETE FROM t; -- \"";
        assert_eq!(separators_my(payload).len(), 2);
        assert!(scan_dialect(payload, Dialect::Mysql)
            .keywords
            .contains(&"DELETE".to_owned()));
    }

    #[test]
    fn a_mysql_hash_comment_hides_a_trailing_separator() {
        // The `#` variant of the same injection: everything after `#` is a comment in
        // MySQL, so the DELETE before it is exposed as code.
        let payload = "SELECT '\\''; DELETE FROM t; # '";
        assert_eq!(separators_my(payload).len(), 2);
        assert!(scan_dialect(payload, Dialect::Mysql)
            .keywords
            .contains(&"DELETE".to_owned()));
        // `#` is an ordinary byte in generic SQL, not a comment.
        assert_eq!(scan("SELECT 1 # 2").keywords, vec!["SELECT"]);
        assert_eq!(
            scan_dialect("SELECT 1 # DROP TABLE t\nSELECT 2", Dialect::Mysql).keywords,
            vec!["SELECT", "SELECT"]
        );
    }

    #[test]
    fn mysql_dash_comment_needs_a_space_after_the_dashes() {
        // MySQL only treats `--` as a comment when whitespace or a control char follows,
        // so `--x` is an operator and the `;` after it still separates.
        assert_eq!(
            scan_dialect("SELECT 1 -- note", Dialect::Mysql).keywords,
            vec!["SELECT"]
        );
        assert!(
            scan_dialect("SELECT 1 --note; DELETE FROM t", Dialect::Mysql)
                .keywords
                .contains(&"DELETE".to_owned())
        );
        // Generic treats `--note` as a comment regardless.
        assert_eq!(
            scan("SELECT 1 --note; DELETE FROM t").keywords,
            vec!["SELECT"]
        );
    }

    #[test]
    fn a_mysql_executable_comment_is_code_not_trivia() {
        // `/*! … */` runs on the server, so its body must be scanned. A DROP hidden in
        // one is a DROP.
        let sql = "SELECT 1 /*! ; DROP TABLE t */";
        assert!(scan_dialect(sql, Dialect::Mysql)
            .keywords
            .contains(&"DROP".to_owned()));
        assert_eq!(separators_my(sql).len(), 1);
        // A version gate is skipped the same way.
        assert!(
            scan_dialect("SELECT 1 /*!50000 ; DELETE FROM t */", Dialect::Mysql)
                .keywords
                .contains(&"DELETE".to_owned())
        );
        // A `/*+ … */` optimizer hint is an ordinary comment, and so is the same text in
        // generic SQL.
        assert_eq!(
            scan_dialect("SELECT /*+ NO_INDEX */ 1", Dialect::Mysql).keywords,
            vec!["SELECT"]
        );
        assert_eq!(
            scan("SELECT 1 /*! DROP TABLE t */").keywords,
            vec!["SELECT"]
        );
    }

    #[test]
    fn a_mariadb_executable_comment_is_a_plain_comment_on_mysql() {
        // `/*M! … */` is MariaDB's spelling; MySQL reads it as an ordinary comment, so
        // under every MySQL lexer its body is text and hides nothing.
        for lexer in mysql_lexers() {
            let hidden = scan_dialect("SELECT 1 /*M! ; DROP TABLE t */", lexer);
            assert_eq!(hidden.keywords, vec!["SELECT"], "{lexer:?}");
            assert!(hidden.separators.is_empty(), "{lexer:?}");
            assert_eq!(
                scan_dialect("/*M comment */", lexer).keywords,
                Vec::<String>::new(),
                "{lexer:?}"
            );
            assert_eq!(
                scan_dialect("/*M! DELETE FROM t */ SELECT 1", lexer)
                    .leading_keyword
                    .as_deref(),
                Some("SELECT"),
                "{lexer:?}"
            );
        }
    }

    fn mysql_lexers() -> impl Iterator<Item = Lexer> {
        Dialect::Mysql.readings().iter().copied()
    }

    fn words(sql: &str, lexer: Lexer) -> Vec<String> {
        scan_dialect(sql, lexer).keywords
    }

    fn sees(sql: &str, lexer: Lexer, word: &str) -> bool {
        words(sql, lexer).iter().any(|found| found == word)
    }

    #[test]
    fn the_mysql_readings_are_the_six_distinct_lexers() {
        let readings = Dialect::Mysql.readings();
        assert_eq!(readings.len(), 6);
        for (index, lexer) in readings.iter().enumerate() {
            for other in &readings[index + 1..] {
                assert_ne!(lexer, other);
            }
        }
        // The default reading leads, and Generic is not among them: a MySQL server
        // never reads `$tag$` as a string or `#` as an operator.
        assert_eq!(readings[0], Lexer::MYSQL);
        assert!(!readings.contains(&Lexer::GENERIC));
        assert_eq!(Dialect::Generic.readings(), &[Lexer::GENERIC]);
    }

    #[test]
    fn mysql_has_no_dollar_quoting_under_any_lexer() {
        // `$` is an identifier character in MySQL, so what sits between two `$a$`
        // markers is code: the `;` separates and the DELETE is a bare word.
        let sql = "SELECT $a$ ; DELETE FROM t ; $a$";
        // The spelling the server runs as two statements: `x$a$` is one identifier.
        for lexer in mysql_lexers() {
            let valid = "SELECT 1 AS x$a$ ; DELETE FROM t ; $a$";
            assert_eq!(scan_dialect(valid, lexer).separators.len(), 2, "{lexer:?}");
            assert!(sees(valid, lexer, "DELETE"), "{lexer:?}");
        }
        assert!(scan("SELECT 1 AS x$a$ ; DELETE FROM t ; $a$")
            .separators
            .is_empty());
        for lexer in mysql_lexers() {
            assert_eq!(scan_dialect(sql, lexer).separators.len(), 2, "{lexer:?}");
            assert!(sees(sql, lexer, "DELETE"), "{lexer:?}");
        }
        // A write with no `;` at all is hidden the same way.
        for lexer in mysql_lexers() {
            assert!(sees("SELECT $x$ DELETE FROM t $x$", lexer, "DELETE"));
            assert!(sees("SELECT $$ DROP TABLE t $$", lexer, "DROP"));
            // The empty tag too, with the write behind a `;`.
            assert_eq!(
                scan_dialect("SELECT $$; DELETE FROM t; $$", lexer)
                    .separators
                    .len(),
                2
            );
        }
        // PostgreSQL and Trino keep dollar quoting, byte for byte.
        assert!(scan(sql).separators.is_empty());
        assert_eq!(
            scan("SELECT $x$ DELETE FROM t $x$").keywords,
            vec!["SELECT"]
        );
    }

    #[test]
    fn a_dollar_inside_a_mysql_identifier_stays_in_the_word() {
        for lexer in mysql_lexers() {
            assert_eq!(
                words("SELECT a$update, b$ FROM t$drop", lexer),
                vec!["SELECT", "A$UPDATE", "B$", "FROM", "T$DROP"],
                "{lexer:?}"
            );
        }
    }

    #[test]
    fn a_mysql_hash_comment_is_a_comment_under_every_lexer() {
        for lexer in mysql_lexers() {
            assert_eq!(
                words("SELECT 1 # plain hash comment", lexer),
                vec!["SELECT"],
                "{lexer:?}"
            );
            assert_eq!(
                words("SELECT 1 # remember to update this later", lexer),
                vec!["SELECT"],
                "{lexer:?}"
            );
            // The comment ends at the line, and what follows is code.
            assert!(sees("SELECT 1 # x\n; DELETE FROM t", lexer, "DELETE"));
            assert_eq!(
                scan_dialect("SELECT 1 # x; y", lexer).separators,
                Vec::<usize>::new()
            );
            // A `#` inside a literal or a quoted identifier is text.
            assert_eq!(
                words("SELECT 'a#b', \"c#d\", `e#f` FROM t", lexer),
                vec!["SELECT", "FROM", "T"]
            );
        }
    }

    #[test]
    fn a_mysql_double_dash_needs_whitespace_under_every_lexer() {
        for lexer in mysql_lexers() {
            // `-- ` and `--` at the end of the line or the text are comments.
            assert_eq!(words("SELECT 1 -- delete", lexer), vec!["SELECT"]);
            assert_eq!(words("SELECT 1 --\tdelete", lexer), vec!["SELECT"]);
            assert_eq!(
                words("SELECT 1 --\ndelete", lexer),
                vec!["SELECT", "DELETE"]
            );
            assert_eq!(words("SELECT 1 --", lexer), vec!["SELECT"]);
            // `--x` is arithmetic, so the `;` after it separates.
            assert!(sees("SELECT 1 --x; DELETE FROM t", lexer, "DELETE"));
            assert_eq!(
                scan_dialect("SELECT 1 --x; DELETE FROM t", lexer)
                    .separators
                    .len(),
                1
            );
            assert_eq!(words("SELECT 5--2", lexer), vec!["SELECT"]);
            assert_eq!(scan_dialect("SELECT 5--2", lexer).separators.len(), 0);
        }
    }

    #[test]
    fn an_executable_comment_body_is_code_or_skipped_depending_on_the_lexer() {
        let mut code = 0;
        let mut skipped = 0;
        for lexer in mysql_lexers() {
            for sql in [
                "SELECT 1 /*! DELETE FROM t */",
                "SELECT 1 /*!50000 DELETE FROM t */",
                "SELECT 1 /*!99999 DELETE FROM t */",
            ] {
                if sees(sql, lexer, "DELETE") {
                    code += 1;
                } else {
                    skipped += 1;
                    assert_eq!(words(sql, lexer), vec!["SELECT"], "{sql} {lexer:?}");
                }
            }
        }
        // Three spellings, three lexers that run the body and three that skip it.
        assert_eq!((code, skipped), (9, 9));
        // A `;` inside the body is a separator where the body is code, and text where it
        // is skipped, and the leading word follows the same rule.
        let with_gate = "/*!50000 SELECT */ 1";
        assert_eq!(
            scan_dialect(with_gate, Lexer::MYSQL)
                .leading_keyword
                .as_deref(),
            Some("SELECT")
        );
        // A `/*+ … */` hint is an ordinary comment under every lexer.
        for lexer in mysql_lexers() {
            assert_eq!(
                words("SELECT /*+ NO_INDEX(t) */ 1", lexer),
                vec!["SELECT"],
                "{lexer:?}"
            );
        }
    }

    #[test]
    fn backslash_and_ansi_quotes_move_where_a_string_ends() {
        // `'\'; DELETE …; -- '`: with backslash escapes the string swallows the `;`; without
        // them the string is `'\'` and the DELETE is code.
        let single = "SELECT '\\'; DELETE FROM t; -- '";
        // `"\"; …`: a string under default MySQL, an identifier under ANSI_QUOTES.
        let double = "SELECT \"\\\"; DELETE FROM t; -- \"";
        let (mut hides_single, mut shows_single) = (0, 0);
        let (mut hides_double, mut shows_double) = (0, 0);
        for lexer in mysql_lexers() {
            if sees(single, lexer, "DELETE") {
                shows_single += 1;
            } else {
                hides_single += 1;
            }
            if sees(double, lexer, "DELETE") {
                shows_double += 1;
            } else {
                hides_double += 1;
            }
        }
        assert!(hides_single > 0 && shows_single > 0);
        assert!(hides_double > 0 && shows_double > 0);
        // Generic never reads a backslash as an escape.
        assert!(scan(single).keywords.contains(&"DELETE".to_owned()));
    }

    #[test]
    fn generic_scanning_is_unchanged_by_the_dialect_split() {
        // The default dialect must behave exactly as before, so PostgreSQL and Trino,
        // and every generic caller, are untouched. A backslash is an ordinary byte.
        assert_eq!(
            scan("SELECT 'a\\'"),
            scan_dialect("SELECT 'a\\'", Dialect::Generic)
        );
        assert_eq!(separators("SELECT 'it''s; ok'"), Vec::<usize>::new());
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

    // W3-T0b: PostgreSQL is read the way `scan.l` lexes it. Each expectation below was
    // checked against a live PostgreSQL 17 server (`SELECT` the text, or run it with a
    // marker `INSERT` after it and see whether the row appeared), not read off the manual.

    fn pg(sql: &str) -> Scan {
        scan_dialect(sql, Lexer::POSTGRES)
    }

    /// `standard_conforming_strings = off`.
    fn pg_off(sql: &str) -> Scan {
        scan_dialect(sql, Dialect::Postgres.readings()[1])
    }

    fn has(scan: &Scan, word: &str) -> bool {
        scan.keywords.iter().any(|found| found == word)
    }

    #[test]
    fn the_postgres_readings_are_four_distinct_lexers_and_the_default_is_first() {
        let readings = Dialect::Postgres.readings();
        assert_eq!(readings.len(), 4);
        for (index, lexer) in readings.iter().enumerate() {
            assert!(lexer.is_postgres());
            for other in &readings[index + 1..] {
                assert_ne!(lexer, other);
            }
        }
        assert_eq!(readings[0], Lexer::POSTGRES);
        assert_eq!(Lexer::from(Dialect::Postgres), Lexer::POSTGRES);
        assert!(!readings.contains(&Lexer::GENERIC));
        assert!(!Lexer::GENERIC.is_postgres() && !Lexer::MYSQL.is_postgres());
    }

    #[test]
    fn an_e_string_honours_backslash_escapes_under_every_postgres_reading() {
        // `E'x\' AS a, '` is one string, so the `;` after it is a separator and the
        // DELETE is code. Generic reads `\'` as a backslash and a closing quote.
        let payload = "SELECT E'x\\' AS a, '; DELETE FROM t; --'";
        assert!(!has(&scan(payload), "DELETE"), "generic misreads it");
        for lexer in Dialect::Postgres.readings() {
            let scanned = scan_dialect(payload, *lexer);
            assert!(has(&scanned, "DELETE"), "{lexer:?}");
            assert_eq!(scanned.separators.len(), 2, "{lexer:?}");
        }
        // Lower case, and inside a longer statement.
        assert!(has(
            &pg("SELECT e'x\\' AS a, '; DELETE FROM t; --'"),
            "DELETE"
        ));
        // `E` only prefixes a string when it is a whole token: `xE'…'` is the name `xE`
        // followed by an ordinary string, where a backslash is text.
        assert!(has(&pg("SELECT xE'\\'; DELETE FROM t; --'"), "DELETE"));
        assert_eq!(pg("SELECT xE'a\\'").keywords, vec!["SELECT", "XE"]);
        // A backslash pair keeps the quote after it live.
        assert_eq!(pg("SELECT E'a\\\\'; SELECT 2").separators.len(), 1);
    }

    #[test]
    fn a_plain_string_honours_a_backslash_only_when_standard_conforming_strings_is_off() {
        let payload = "SELECT 'x\\' AS a, '; DELETE FROM t; --'";
        // on: `\` is a byte, so `'x\'` is a string, ` AS a, ` is code, and the tail is a
        // second string: the DELETE is hidden.
        assert!(!has(&pg(payload), "DELETE"));
        assert!(pg(payload).separators.is_empty());
        // off: `\'` is an escaped quote, `'x\' AS a, '` is one string, and the DELETE is
        // code the server runs. This is why a guard has to read both.
        assert!(has(&pg_off(payload), "DELETE"));
        assert_eq!(pg_off(payload).separators.len(), 2);
        // Both agree when no backslash precedes a quote.
        assert_eq!(
            pg("SELECT 'a\\nb'").keywords,
            pg_off("SELECT 'a\\nb'").keywords
        );
    }

    #[test]
    fn a_unicode_string_never_honours_a_backslash_escape_of_the_quote() {
        // `U&'…'` is lexed as its own string kind: the `\` is the unicode escape
        // character, so `'\'` is complete even where a plain string would run on.
        let sql = "SELECT U&'\\'; DELETE FROM t; --'";
        for lexer in Dialect::Postgres.readings() {
            assert!(has(&scan_dialect(sql, *lexer), "DELETE"), "{lexer:?}");
        }
        // A doubled quote is still a quote, and a lower-case prefix works.
        assert!(scan_dialect("SELECT u&'it''s; ok'", Lexer::POSTGRES)
            .separators
            .is_empty());
        // `U&"…"` is a quoted identifier.
        assert!(pg("SELECT U&\"a;b\" FROM t").separators.is_empty());
        // A bare `U` and `&` are not a string prefix.
        assert!(has(&pg("SELECT u & 'a'; DELETE FROM t"), "DELETE"));
    }

    #[test]
    fn a_bit_string_ends_at_its_first_quote_and_a_doubled_one_starts_another_string() {
        // `B'1''0'` is two literals on the server (a syntax error, but one that reads the
        // second as a plain string). A `\` in the bit string is text.
        assert_eq!(pg("SELECT B'1\\'; DELETE FROM t; --'").separators.len(), 2);
        assert!(has(&pg("SELECT x'1F\\'; DELETE FROM t; --'"), "DELETE"));
        // After the doubled quote the new string follows the plain-string rule.
        let sql = "SELECT B'1''\\'; DELETE FROM t; --'";
        assert!(has(&pg(sql), "DELETE"));
        assert!(!has(&pg_off(sql), "DELETE"));
    }

    #[test]
    fn a_string_continues_across_a_newline_under_the_rules_of_its_first_piece() {
        // `'a'` + newline + `'b'` is one string, and an E string stays an E string: the
        // second piece honours the backslash even where a plain string would not.
        let sql = "SELECT E'a'\n'x\\'y'; DELETE FROM t; --'";
        for lexer in Dialect::Postgres.readings() {
            let scanned = scan_dialect(sql, *lexer);
            assert!(has(&scanned, "DELETE"), "{lexer:?}");
            assert_eq!(scanned.separators.len(), 2, "{lexer:?}");
        }
        // The mirror: on the server this is one string to the very last quote.
        let mirror = "SELECT E'a'\n'\\'; DELETE FROM t; --'";
        assert!(!has(&pg(mirror), "DELETE"));
        assert!(pg(mirror).separators.is_empty());
        // Comments and `\r` count as the newline's company; a missing newline is no
        // continuation.
        for joined in [
            "SELECT E'a' -- c\n'\\'; DELETE FROM t; --'",
            "SELECT E'a'\r'\\'; DELETE FROM t; --'",
            "SELECT E'a' \n -- c\n\t'\\'; DELETE FROM t; --'",
        ] {
            assert!(!has(&pg(joined), "DELETE"), "{joined:?}");
        }
        let apart = "SELECT E'a' '\\'; DELETE FROM t; --'";
        assert!(has(&pg(apart), "DELETE"));
        // `\v` is whitespace on a server that reads it so, and not on one that does not.
        let vt = "SELECT E'a'\n\x0b'\\'; DELETE FROM t; --'";
        assert!(!has(
            &scan_dialect(vt, Dialect::Postgres.readings()[0]),
            "DELETE"
        ));
        assert!(has(
            &scan_dialect(vt, Dialect::Postgres.readings()[2]),
            "DELETE"
        ));
    }

    #[test]
    fn a_postgres_block_comment_nests() {
        let payload = "SELECT 1 /* /* */ ' */; DELETE FROM t; --'";
        assert!(!has(&scan(payload), "DELETE"), "generic does not nest");
        for lexer in Dialect::Postgres.readings() {
            let scanned = scan_dialect(payload, *lexer);
            assert!(has(&scanned, "DELETE"), "{lexer:?}");
            assert_eq!(scanned.separators.len(), 2, "{lexer:?}");
        }
        // Balanced nesting hides everything inside it; an unclosed level hides the rest.
        let closed = "SELECT 1 /* a /* b; */ c; */ ;";
        assert_eq!(pg(closed).separators, vec![closed.len() - 1]);
        assert!(pg("SELECT 1 /* a /* b */ ; DELETE FROM t")
            .separators
            .is_empty());
        // `/*/` does not close, `/**/` is empty, and `*/` first does not open one.
        assert!(pg("SELECT 1 /*/ ; */ ;").separators == vec![18]);
        assert!(pg("SELECT 1 /**/ ;").separators == vec![14]);
        assert!(pg("SELECT 1 /* /*/ */ ; */ ;").separators == vec![24]);
        // The leading keyword and the first significant byte skip a nested comment too.
        assert_eq!(
            pg("/* a /* b */ c */ select 1").leading_keyword.as_deref(),
            Some("SELECT")
        );
        assert_eq!(
            first_significant("/* a /* b */ c */ x", Lexer::POSTGRES),
            Some(18)
        );
    }

    #[test]
    fn a_postgres_line_comment_ends_at_a_carriage_return() {
        let payload = "SELECT 1 -- x\r; DELETE FROM t";
        assert!(has(&pg(payload), "DELETE"));
        assert_eq!(pg(payload).separators.len(), 1);
        assert!(
            !has(&scan(payload), "DELETE"),
            "generic waits for a newline"
        );
        assert_eq!(
            pg("-- c\r select 1").leading_keyword.as_deref(),
            Some("SELECT")
        );
        assert_eq!(first_significant("-- c\r x", Lexer::POSTGRES), Some(6));
        // `--` is a comment wherever it follows, unlike MySQL.
        assert!(pg("SELECT 1--x\n;").separators == vec![12]);
    }

    #[test]
    fn a_dollar_continues_a_postgres_identifier_so_it_cannot_open_a_quote() {
        let payload = "SELECT 1 AS a$x$; DELETE FROM t; --$x$";
        assert!(
            !has(&scan(payload), "DELETE"),
            "generic reads a$x$ as a dollar quote"
        );
        for lexer in Dialect::Postgres.readings() {
            let scanned = scan_dialect(payload, *lexer);
            assert!(has(&scanned, "DELETE"), "{lexer:?}");
            assert!(has(&scanned, "A$X$"), "{lexer:?}");
        }
        assert_eq!(
            pg("SELECT foo$bar, a$1").keywords,
            vec!["SELECT", "FOO$BAR", "A$1"]
        );
        // A non-ASCII letter is an identifier byte too, as are digits after one.
        let unicode = "SELECT 1 AS é$x$; DELETE FROM t; --$x$";
        assert!(has(&pg(unicode), "DELETE"));
        assert_eq!(pg("SELECT éa1$b").keywords, vec!["SELECT", "éA1$B"]);
        // A number is not an identifier, so a dollar quote can start straight after one,
        // after a string, a parameter, a quoted identifier and an operator.
        for opener in ["1", "'a'", "$1", "\"a\"", "+", ")"] {
            let sql = format!("SELECT {opener}$q$; DELETE FROM t; $q$");
            assert!(!has(&pg(&sql), "DELETE"), "{opener}");
        }
    }

    #[test]
    fn a_postgres_dollar_quote_follows_the_servers_tag_rules() {
        // `$$`, `$tag$`, tags with digits after the first letter, `_`, non-ASCII letters.
        for sql in [
            "SELECT $$a;b$$",
            "SELECT $a$ ; DELETE FROM t $a$",
            "SELECT $a1_$ ; DELETE FROM t $a1_$",
            "SELECT $é$ ; DELETE FROM t $é$",
            "SELECT $_$ ; DELETE FROM t $_$",
        ] {
            let scanned = pg(sql);
            assert!(scanned.separators.is_empty(), "{sql}");
            assert_eq!(scanned.keywords, vec!["SELECT"], "{sql}");
        }
        // A tag is case sensitive, and a longer delimiter that merely ends in the tag
        // does not close it: the last `$` of `$b$a$` is where `$a$` starts.
        assert!(pg("SELECT $a$ x $A$ ; $a$ y").separators.is_empty());
        assert_eq!(pg("SELECT $a$ $b$a$ ; y").separators.len(), 1);
        // A tag cannot start with a digit: `$1$` is the parameter `$1` and a lone `$`.
        assert!(has(&pg("SELECT $1$ ; DELETE FROM t ; $1$"), "DELETE"));
        assert_eq!(pg("SELECT $1, $2").separators.len(), 0);
        assert_eq!(pg("SELECT $1; SELECT $2").separators.len(), 1);
        // No closing `$` means no quote: `$abc` is a `$` and a word.
        assert!(has(&pg("SELECT $abc ; DELETE FROM t"), "DELETE"));
        // A space inside the would-be tag ends the attempt.
        assert!(has(&pg("SELECT $a b$ ; DELETE FROM t"), "DELETE"));
    }

    #[test]
    fn a_backtick_is_an_operator_in_postgres_not_a_quote() {
        let sql = "SELECT ` ; DELETE FROM t ; -- `";
        assert!(
            !has(&scan(sql), "DELETE"),
            "generic quotes an identifier with it"
        );
        for lexer in Dialect::Postgres.readings() {
            assert!(has(&scan_dialect(sql, *lexer), "DELETE"), "{lexer:?}");
        }
    }

    #[test]
    fn a_postgres_quoted_identifier_holds_anything_and_doubles_its_quote() {
        assert!(pg("SELECT \"a;b\" FROM t").separators.is_empty());
        assert!(pg("SELECT \"a\"\";b\" FROM t").separators.is_empty());
        // A backslash is an ordinary byte in an identifier, under both settings.
        for lexer in Dialect::Postgres.readings() {
            assert!(has(
                &scan_dialect("SELECT \"a\\\"; DELETE FROM t", *lexer),
                "DELETE"
            ));
        }
        // `N'…'` is `N` and then an ordinary string, so it follows the plain rule.
        assert!(has(&pg("SELECT N'\\'; DELETE FROM t; --'"), "DELETE"));
        assert!(!has(&pg_off("SELECT N'\\'; DELETE FROM t; --'"), "DELETE"));
    }

    #[test]
    fn a_postgres_leading_keyword_is_a_whole_identifier() {
        assert_eq!(
            pg("select$x 1").leading_keyword.as_deref(),
            Some("SELECT$X")
        );
        assert_eq!(pg("select1").leading_keyword.as_deref(), Some("SELECT1"));
        assert_eq!(pg("éx").leading_keyword.as_deref(), Some("éX"));
        assert_eq!(pg("SELECT 1").leading_keyword.as_deref(), Some("SELECT"));
        // Generic and MySQL are untouched.
        assert_eq!(scan("select1").leading_keyword.as_deref(), Some("SELECT"));
    }

    #[test]
    fn postgres_scanning_never_panics_or_hangs_on_a_broken_string() {
        for sql in [
            "SELECT E'",
            "SELECT E'\\",
            "SELECT U&'",
            "SELECT B'",
            "SELECT 'a'\n",
            "SELECT 'a'\n--",
            "SELECT 'a' --\n",
            "SELECT /*",
            "SELECT /* /*",
            "SELECT $",
            "SELECT $a",
            "SELECT $a$",
            "SELECT é",
            "É",
            "\u{feff}SELECT 1",
            "SELECT '\u{e9}\\\u{e9}'",
            "SELECT E'\u{e9}\\\u{e9}' ; DROP TABLE t",
        ] {
            for lexer in Dialect::Postgres.readings() {
                let _ = scan_dialect(sql, *lexer);
            }
        }
    }
}
