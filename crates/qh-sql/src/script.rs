//! The script splitter: where each statement of a `.sql` file ends, and what a file asks of
//! its client that no server can run.
//!
//! # Stored-program bodies (MySQL)
//!
//! A `;` ends a statement, except inside the body of a stored program. `CREATE PROCEDURE p()
//! BEGIN DELETE FROM a; DELETE FROM b; END` is one statement to the server; split on its own
//! `;` it is three, and under `ON_ERROR=skip` the second `DELETE` would run on its own the
//! moment the first fragment failed. [`separators`] reads the body the way the server's
//! grammar does (`sql_yacc.yy`: `sp_proc_stmt`) and drops the `;` that belong to it.
//!
//! Only the *opening* of a body is guessed from words; everything inside it is tracked at
//! statement level, because a bare word means different things in different places (`END` closes
//! a block at the start of a statement and closes a `CASE` expression in the middle of one,
//! `IF(` is a function, `BEGIN` alone is a transaction). So:
//!
//! * a body starts after the header of `CREATE [DEFINER = …] {PROCEDURE | FUNCTION | TRIGGER |
//!   EVENT}` (`ALTER EVENT … DO` too), at its first statement, and only when that statement is a
//!   block: `BEGIN`, `IF`, `CASE`, `LOOP`, `WHILE`, `REPEAT`, optionally behind a `label:`. A
//!   simple body (`… RETURN 1;`, `… DELETE FROM t;`) ends at the first `;` as always;
//! * inside it a block opens and closes only at the start of a statement, a statement starts
//!   after `;`, `BEGIN`, `LOOP`, `REPEAT`, `THEN`, `ELSE`, `WHILE … DO` and after the condition list
//!   of `DECLARE … HANDLER FOR`, and a `CASE` met in the middle of a statement is an expression
//!   whose `END` is not a block end;
//! * MariaDB's anonymous `BEGIN NOT ATOMIC … END` is a body too.
//!
//! **Which way it fails.** Every doubt resolves toward the old reading: a header the tracker does
//! not recognise is split on every `;` as before, and a body that never closes swallows the rest
//! of the file into one statement, which the server rejects as a syntax error without running a
//! piece of it. Both are visible failures. What the tracker must not do is end a body early
//! (the fragments after it would run alone), and the tests pin each construct that could.
//!
//! A swallowed remainder cannot slip past Safe Mode either: a statement that opens a body begins
//! with `CREATE`, `ALTER` or `BEGIN`, which every mode below `full` refuses.
//!
//! `walk` and [`crate::scan_dialect`] are untouched: their separators stay the raw `;` positions
//! (the editor's incremental statement list is built from them and cannot carry body state
//! across a window). This module is the one place a *script* is split.
//!
//! # Client directives
//!
//! A file can hold lines that are not SQL at all but instructions for the program that reads
//! it: `DELIMITER` (mysql), `\restrict` and every other backslash command (psql), and `COPY …
//! FROM STDIN` whose rows follow in the file. A server cannot run them, and splitting the file
//! around them sends garbage pieces, so [`client_directive`] names the first one before any
//! connection opens.

use thiserror::Error;

use crate::classify::{line_at, statements_with_lines_dialect};
use crate::scan::{first_significant, scan_dialect, walk, Lexer, OpaqueKind, Visitor};

/// The byte offsets of the `;` that end a statement of `sql` under `lexer`.
///
/// For every lexer but MySQL's this is [`scan_dialect`]'s separators, unchanged.
pub(crate) fn separators(sql: &str, lexer: Lexer) -> Vec<usize> {
    if !lexer.is_mysql() {
        return scan_dialect(sql, lexer).separators;
    }
    let mut splitter = Splitter {
        bytes: sql.as_bytes(),
        pos: 0,
        parens: 0,
        opens: 0,
        at: false,
        top: Top::Start,
        separators: Vec::new(),
    };
    walk(sql.as_bytes(), lexer, &mut splitter);
    splitter.separators
}

fn is(word: &[u8], keyword: &str) -> bool {
    word.eq_ignore_ascii_case(keyword.as_bytes())
}

fn is_any(word: &[u8], keywords: &[&str]) -> bool {
    keywords.iter().any(|keyword| is(word, keyword))
}

/// One thing the walk reported that is neither a comment nor a separator.
#[derive(Clone, Copy)]
enum Ev<'a> {
    Word(&'a [u8]),
    /// A string or a quoted name; what it holds does not matter here.
    Quoted(OpaqueKind),
}

/// What the splitter knows about the byte positions around the event being handled.
struct Ctx {
    /// `(` minus `)` since the statement began, never below zero.
    parens: i32,
    /// How many `(` the statement has had.
    opens: u32,
    /// An `@` lies between this event and the last one (`user@host`).
    at: bool,
    /// The event is followed by a `:` that is not `:=`: it is a label.
    label: bool,
}

/// Where the current top-level statement stands.
enum Top {
    /// No word yet.
    Start,
    /// Nothing to track until the next effective `;`.
    Plain,
    /// A leading `BEGIN`: a transaction, unless `NOT ATOMIC` follows. Holds whether `NOT` came.
    Begin(bool),
    /// In the header of a `CREATE` or `ALTER`, looking for a body.
    Head(Head),
    /// Inside a block body; every `;` is content until it closes.
    Body(Body),
}

enum Step {
    Stay,
    Plain,
    Begin,
    Head(Head),
    Enter(Body),
}

struct Splitter<'a> {
    bytes: &'a [u8],
    /// End of the last event: the bytes up to the next one are punctuation and numbers.
    pos: usize,
    parens: i32,
    opens: u32,
    at: bool,
    top: Top,
    separators: Vec<usize>,
}

impl Splitter<'_> {
    /// Take in the bytes between the last event and `upto`.
    ///
    /// Nothing before the first word of a statement matters, so a plain statement (nearly every
    /// statement of a dump) costs no extra pass.
    fn gap(&mut self, upto: usize) {
        if matches!(self.top, Top::Start | Top::Plain | Top::Begin(_)) {
            return;
        }
        let segment = &self.bytes[self.pos..upto];
        for &byte in segment {
            match byte {
                b'(' => {
                    self.parens += 1;
                    self.opens = self.opens.saturating_add(1);
                }
                b')' => self.parens = (self.parens - 1).max(0),
                b'@' => self.at = true,
                _ => {}
            }
        }
        if let Top::Body(body) = &mut self.top {
            body.gap(segment);
        }
    }

    /// Whether a `:` (and not `:=`) follows `end`, past whitespace.
    fn colon_after(&self, end: usize) -> bool {
        let mut index = end;
        while self.bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
            index += 1;
        }
        self.bytes.get(index) == Some(&b':') && self.bytes.get(index + 1) != Some(&b'=')
    }

    fn reset(&mut self) {
        self.top = Top::Start;
        self.parens = 0;
        self.opens = 0;
        self.at = false;
    }

    fn event(&mut self, event: Ev<'_>) {
        let label = matches!(self.top, Top::Head(_) | Top::Body(_)) && self.colon_after(self.pos);
        let ctx = Ctx {
            parens: self.parens,
            opens: self.opens,
            at: self.at,
            label,
        };
        self.at = false;
        let step = match &mut self.top {
            Top::Plain => return,
            Top::Start => start(event),
            Top::Begin(not_seen) => begin(not_seen, event),
            Top::Head(head) => head.event(&ctx, event),
            Top::Body(body) => {
                if body.event(&ctx, event) {
                    Step::Plain
                } else {
                    Step::Stay
                }
            }
        };
        match step {
            Step::Stay => {}
            Step::Plain => self.top = Top::Plain,
            Step::Begin => self.top = Top::Begin(false),
            Step::Head(head) => self.top = Top::Head(head),
            Step::Enter(body) => self.top = Top::Body(body),
        }
    }
}

impl Visitor for Splitter<'_> {
    fn separator(&mut self, at: usize) {
        self.gap(at);
        self.pos = at + 1;
        if let Top::Body(body) = &mut self.top {
            if body.depth > 0 {
                body.restart();
                return;
            }
        }
        self.separators.push(at);
        self.reset();
    }

    fn opaque(&mut self, kind: OpaqueKind, start: usize, end: usize) {
        self.gap(start);
        self.pos = end;
        // A comment is not a token: what lies on either side of it is one gap.
        if !matches!(kind, OpaqueKind::LineComment | OpaqueKind::BlockComment) {
            self.event(Ev::Quoted(kind));
        }
    }

    fn word(&mut self, start: usize, end: usize) {
        self.gap(start);
        self.pos = end;
        let bytes = self.bytes;
        self.event(Ev::Word(&bytes[start..end]));
    }
}

fn start(event: Ev<'_>) -> Step {
    match event {
        Ev::Word(word) if is(word, "CREATE") => Step::Head(Head::new(false)),
        Ev::Word(word) if is(word, "ALTER") => Step::Head(Head::new(true)),
        Ev::Word(word) if is(word, "BEGIN") => Step::Begin,
        _ => Step::Plain,
    }
}

fn begin(not_seen: &mut bool, event: Ev<'_>) -> Step {
    match event {
        Ev::Word(word) if !*not_seen && is(word, "NOT") => {
            *not_seen = true;
            Step::Stay
        }
        Ev::Word(word) if *not_seen && is(word, "ATOMIC") => Step::Enter(Body::block()),
        _ => Step::Plain,
    }
}

// ---------------------------------------------------------------------------------------------
// Header: `CREATE … PROCEDURE | FUNCTION | TRIGGER | EVENT`, up to the first statement of the body.
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// After `CREATE` or `ALTER`: modifiers, then the kind of object.
    Intro,
    /// After `DEFINER`: the user.
    DefinerUser,
    /// The user was read; an `@` before the next event makes it the host.
    DefinerHost,
    /// A procedure or function: the name, up to the `(` of its parameters.
    Name,
    /// Inside the parameter list.
    Params,
    /// Characteristics and `RETURNS`, up to the body.
    After,
    /// The type name after `RETURNS`.
    ReturnType,
    /// What may follow a type name before the body.
    TypeTail,
    /// A trigger: up to `FOR EACH ROW`.
    Trigger,
    /// After `FOR EACH ROW`: `FOLLOWS | PRECEDES name`, or the body.
    Order,
    /// An event: up to `DO`.
    Event,
    /// The next word starts the body.
    BodyFirst,
}

struct Head {
    alter: bool,
    phase: Phase,
    /// The next event belongs to the one before it (`COLLATE x`, `FOLLOWS t`).
    skip: bool,
    for_each: u8,
    /// `opens` when the name began: a `(` after it opens the parameter list.
    opens_at: u32,
}

/// Words that may sit between a routine's parameter list and its body.
const CHARACTERISTICS: [&str; 13] = [
    "COMMENT",
    "LANGUAGE",
    "SQL",
    "NOT",
    "DETERMINISTIC",
    "CONTAINS",
    "NO",
    "READS",
    "MODIFIES",
    "DATA",
    "SECURITY",
    "DEFINER",
    "INVOKER",
];

/// Words that continue a `RETURNS` type name.
const TYPE_TAIL: [&str; 9] = [
    "UNSIGNED",
    "SIGNED",
    "ZEROFILL",
    "PRECISION",
    "VARYING",
    "BINARY",
    "ASCII",
    "UNICODE",
    "NATIONAL",
];

impl Head {
    fn new(alter: bool) -> Head {
        Head {
            alter,
            phase: Phase::Intro,
            skip: false,
            for_each: 0,
            opens_at: 0,
        }
    }

    fn event(&mut self, ctx: &Ctx, event: Ev<'_>) -> Step {
        if self.skip {
            self.skip = false;
            return Step::Stay;
        }
        loop {
            match self.phase {
                Phase::Name | Phase::Params => {
                    if ctx.parens > 0 {
                        self.phase = Phase::Params;
                        return Step::Stay;
                    }
                    if ctx.opens == self.opens_at {
                        return Step::Stay;
                    }
                    self.phase = Phase::After;
                    continue;
                }
                _ if ctx.parens > 0 => return Step::Stay,
                _ => {}
            }
            let word = match event {
                Ev::Word(word) => Some(word),
                Ev::Quoted(_) => None,
            };
            match self.phase {
                Phase::Intro => {
                    let Some(word) = word else {
                        return Step::Plain;
                    };
                    if is_any(word, &["OR", "REPLACE", "AGGREGATE"]) {
                        return Step::Stay;
                    }
                    self.phase = if is(word, "DEFINER") {
                        Phase::DefinerUser
                    } else if is_any(word, &["PROCEDURE", "FUNCTION"]) && !self.alter {
                        self.opens_at = ctx.opens;
                        Phase::Name
                    } else if is(word, "TRIGGER") && !self.alter {
                        Phase::Trigger
                    } else if is(word, "EVENT") {
                        Phase::Event
                    } else {
                        return Step::Plain;
                    };
                    return Step::Stay;
                }
                Phase::DefinerUser => {
                    self.phase = Phase::DefinerHost;
                    return Step::Stay;
                }
                Phase::DefinerHost => {
                    self.phase = Phase::Intro;
                    if ctx.at {
                        return Step::Stay;
                    }
                    continue;
                }
                Phase::Name | Phase::Params => unreachable!("handled above"),
                Phase::After => match word {
                    Some(word) if is(word, "RETURNS") => {
                        self.phase = Phase::ReturnType;
                        return Step::Stay;
                    }
                    Some(word) if is_any(word, &CHARACTERISTICS) => return Step::Stay,
                    // The string after `COMMENT`.
                    None => return Step::Stay,
                    Some(_) => {
                        self.phase = Phase::BodyFirst;
                        continue;
                    }
                },
                Phase::ReturnType => {
                    self.phase = Phase::TypeTail;
                    return Step::Stay;
                }
                Phase::TypeTail => match word {
                    Some(word) if is_any(word, &TYPE_TAIL) || is_any(word, &CHARACTERISTICS) => {
                        return Step::Stay
                    }
                    // `CHARACTER SET x`, `CHARSET x`, `COLLATE x`: the next event is a name.
                    Some(word) if is_any(word, &["CHARACTER", "CHAR"]) => return Step::Stay,
                    Some(word) if is_any(word, &["SET", "CHARSET", "COLLATE"]) => {
                        self.skip = true;
                        return Step::Stay;
                    }
                    None => return Step::Stay,
                    Some(_) => {
                        self.phase = Phase::BodyFirst;
                        continue;
                    }
                },
                Phase::Trigger => {
                    let progress = match (word, self.for_each) {
                        (Some(word), 0) if is(word, "FOR") => 1,
                        (Some(word), 1) if is(word, "EACH") => 2,
                        (Some(word), 2) if is(word, "ROW") => 3,
                        _ => 0,
                    };
                    self.for_each = progress;
                    if progress == 3 {
                        self.phase = Phase::Order;
                    }
                    return Step::Stay;
                }
                Phase::Order => match word {
                    Some(word) if is_any(word, &["FOLLOWS", "PRECEDES"]) => {
                        self.skip = true;
                        return Step::Stay;
                    }
                    _ => {
                        self.phase = Phase::BodyFirst;
                        continue;
                    }
                },
                Phase::Event => {
                    if word.is_some_and(|word| is(word, "DO")) {
                        self.phase = Phase::BodyFirst;
                    }
                    return Step::Stay;
                }
                Phase::BodyFirst => {
                    return match event {
                        // `label: BEGIN`: the label is not the first statement.
                        Ev::Word(_) if ctx.label => Step::Stay,
                        Ev::Quoted(OpaqueKind::Backtick | OpaqueKind::DoubleQuote) if ctx.label => {
                            Step::Stay
                        }
                        Ev::Word(word) => match Body::opened_by(word) {
                            Some(body) => Step::Enter(body),
                            None => Step::Plain,
                        },
                        Ev::Quoted(_) => Step::Plain,
                    };
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Body: statement-level tracking of blocks.
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Opener {
    If,
    Case,
    While,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cond {
    /// An item of the condition list must start.
    Open,
    Not,
    Sqlstate,
    Value,
    /// An item is complete: a `,` makes another, anything else is the handler's action.
    Closed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Declare {
    /// After `DECLARE`; whether `CONTINUE`, `EXIT` or `UNDO` was seen.
    Pre(bool),
    /// `HANDLER` seen: `FOR` starts the condition list.
    Handler,
    Cond(Cond),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stmt {
    /// A statement start with no word read yet.
    Fresh,
    Other,
    Opener(Opener),
    /// `END …`: the rest of the statement says what it closed.
    End,
    /// `UNTIL cond END REPEAT`: here `END` follows with no `;` between.
    Until,
    Declare(Declare),
}

struct Body {
    /// Open blocks. The body is over when it reaches zero.
    depth: u32,
    /// The next word is the first of a statement.
    at_start: bool,
    kind: Stmt,
    /// `CASE` expressions open in the current statement.
    expr_case: u32,
}

impl Body {
    fn block() -> Body {
        Body {
            depth: 1,
            at_start: true,
            kind: Stmt::Fresh,
            expr_case: 0,
        }
    }

    /// The body a first word opens, or `None` for a simple statement.
    fn opened_by(word: &[u8]) -> Option<Body> {
        if is_any(word, &["BEGIN", "LOOP", "REPEAT"]) {
            return Some(Body::block());
        }
        let opener = if is(word, "IF") {
            Opener::If
        } else if is(word, "CASE") {
            Opener::Case
        } else if is(word, "WHILE") {
            Opener::While
        } else {
            return None;
        };
        Some(Body {
            depth: 1,
            at_start: false,
            kind: Stmt::Opener(opener),
            expr_case: 0,
        })
    }

    fn restart(&mut self) {
        self.at_start = true;
        self.kind = Stmt::Fresh;
        self.expr_case = 0;
    }

    /// The bytes between two events matter inside a handler's condition list: a number is an
    /// item, a `,` starts the next one.
    fn gap(&mut self, segment: &[u8]) {
        if let Stmt::Declare(Declare::Cond(cond)) = &mut self.kind {
            for &byte in segment {
                match (*cond, byte) {
                    (Cond::Closed, b',') => *cond = Cond::Open,
                    (Cond::Open, b'0'..=b'9') => *cond = Cond::Closed,
                    _ => {}
                }
            }
        }
    }

    /// Handle one event; `true` when the outermost block just closed.
    fn event(&mut self, ctx: &Ctx, event: Ev<'_>) -> bool {
        loop {
            if let Stmt::Declare(Declare::Cond(cond)) = self.kind {
                // No comma came between the list and this event: the handler's action.
                if cond == Cond::Closed {
                    self.restart();
                    continue;
                }
                let next = match (cond, event) {
                    (Cond::Open, Ev::Word(word)) => {
                        if is_any(word, &["SQLEXCEPTION", "SQLWARNING"]) {
                            Cond::Closed
                        } else if is(word, "NOT") {
                            Cond::Not
                        } else if is(word, "SQLSTATE") {
                            Cond::Sqlstate
                        } else {
                            Cond::Closed
                        }
                    }
                    (Cond::Sqlstate, Ev::Word(word)) if is(word, "VALUE") => Cond::Value,
                    _ => Cond::Closed,
                };
                self.kind = Stmt::Declare(Declare::Cond(next));
                return false;
            }
            return match event {
                Ev::Word(word) if self.at_start => self.first_word(ctx, word),
                Ev::Quoted(kind) if self.at_start => {
                    let label =
                        ctx.label && matches!(kind, OpaqueKind::Backtick | OpaqueKind::DoubleQuote);
                    if !label {
                        self.kind = Stmt::Other;
                        self.at_start = false;
                    }
                    false
                }
                Ev::Word(word) => self.inner_word(word),
                Ev::Quoted(_) => false,
            };
        }
    }

    fn first_word(&mut self, ctx: &Ctx, word: &[u8]) -> bool {
        self.kind = Stmt::Fresh;
        self.expr_case = 0;
        if is(word, "END") {
            self.depth = self.depth.saturating_sub(1);
            self.kind = Stmt::End;
            self.at_start = false;
            return self.depth == 0;
        }
        if ctx.label {
            return false;
        }
        if is_any(word, &["BEGIN", "LOOP", "REPEAT"]) {
            self.depth += 1;
        } else if is(word, "ELSE") {
            // The else branch's first statement follows with no `;` between.
        } else {
            self.at_start = false;
            if is(word, "IF") {
                self.depth += 1;
                self.kind = Stmt::Opener(Opener::If);
            } else if is(word, "CASE") {
                self.depth += 1;
                self.kind = Stmt::Opener(Opener::Case);
            } else if is(word, "WHILE") {
                self.depth += 1;
                self.kind = Stmt::Opener(Opener::While);
            } else if is(word, "UNTIL") {
                self.kind = Stmt::Until;
            } else if is(word, "DECLARE") {
                self.kind = Stmt::Declare(Declare::Pre(false));
            } else {
                self.kind = Stmt::Other;
            }
        }
        false
    }

    fn inner_word(&mut self, word: &[u8]) -> bool {
        match self.kind {
            Stmt::End => {}
            Stmt::Declare(Declare::Pre(seen)) => {
                if is_any(word, &["CONTINUE", "EXIT", "UNDO"]) {
                    self.kind = Stmt::Declare(Declare::Pre(true));
                } else if seen && is(word, "HANDLER") {
                    self.kind = Stmt::Declare(Declare::Handler);
                }
            }
            Stmt::Declare(Declare::Handler) => {
                if is(word, "FOR") {
                    self.kind = Stmt::Declare(Declare::Cond(Cond::Open));
                }
            }
            Stmt::Declare(Declare::Cond(_)) => unreachable!("handled in event"),
            kind => {
                let plain = self.expr_case == 0;
                let starts_statement = is(word, "THEN")
                    || is(word, "ELSE")
                    || (is(word, "DO") && kind == Stmt::Opener(Opener::While));
                if starts_statement && plain {
                    self.at_start = true;
                } else if is(word, "CASE") {
                    self.expr_case += 1;
                } else if is(word, "END") {
                    if self.expr_case > 0 {
                        self.expr_case -= 1;
                    } else if kind == Stmt::Until {
                        self.depth = self.depth.saturating_sub(1);
                        self.kind = Stmt::End;
                        return self.depth == 0;
                    }
                }
            }
        }
        false
    }
}

// ---------------------------------------------------------------------------------------------
// Client directives.
// ---------------------------------------------------------------------------------------------

/// A line of a script that is an instruction to a client program and not SQL.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("line {line}: {reason}")]
pub struct ScriptRefusal {
    /// The 1-based line it is on.
    pub line: usize,
    /// What it is called: `DELIMITER`, `\restrict`, `COPY … FROM STDIN`.
    pub directive: String,
    reason: String,
}

/// The first client directive in `sql` under `dialect`, or `None`.
///
/// * MySQL: a line that starts with `DELIMITER`, the mysql client's command. It is found by line,
///   not by statement: the client reads it after a `USE db` or a `\G` that have no `;`, and then
///   no statement starts with it. Honouring it is a separate decision (ADR-0022 addendum, step 2);
///   until then a routine body that depends on it would be sent in pieces, so the file is refused
///   by name instead.
/// * PostgreSQL: any backslash outside a string, comment or quoted name (a psql meta-command,
///   among them pg_dump's `\restrict` and `\unrestrict`), and `COPY … FROM STDIN`, whose rows
///   are the next lines of the file and cannot be split as SQL.
///
/// Other dialects have none. The caller decides before connecting; this reads text only.
pub fn client_directive(sql: &str, dialect: impl Into<Lexer>) -> Option<ScriptRefusal> {
    let lexer: Lexer = dialect.into();
    if lexer.is_mysql() {
        return first_delimiter_line(sql, lexer).map(|offset| ScriptRefusal {
            line: line_at(sql, offset),
            directive: "DELIMITER".to_owned(),
            reason: "`DELIMITER` is a mysql client command, not SQL. The server rejects it, and \
                     this import does not honour it, so a routine body that relies on it would be cut at its own `;` and sent in pieces. Load the file with \
                     the mysql client, or remove the DELIMITER lines and end each routine with a \
                     single `;`. Any line that starts with the word is refused, a column named \
                     `delimiter` included; write it in backticks"
                .to_owned(),
        });
    }
    if !lexer.is_postgres() {
        return None;
    }
    let backslash = first_backslash(sql, lexer).map(|offset| {
        let line = line_at(sql, offset);
        let name: String = sql[offset..]
            .chars()
            .take_while(|c| !c.is_whitespace())
            .take(32)
            .collect();
        ScriptRefusal {
            line,
            reason: format!(
                "`{name}` is a psql meta-command, not SQL. The server cannot run it, so this \
                 import refuses the file before connecting. Remove the line, or load the file with \
                 psql"
            ),
            directive: name,
        }
    });
    let copy = statements_with_lines_dialect(sql, lexer)
        .into_iter()
        .find(|statement| {
            first_significant(statement.text, lexer).is_some_and(|at| {
                leading_word(&statement.text.as_bytes()[at..]).eq_ignore_ascii_case(b"COPY")
            }) && scan_dialect(statement.text, lexer)
                .keywords
                .windows(2)
                .any(|pair| pair[0] == "FROM" && pair[1] == "STDIN")
        })
        .map(|statement| ScriptRefusal {
            line: statement.line,
            directive: "COPY … FROM STDIN".to_owned(),
            reason: "`COPY … FROM STDIN` reads its rows from the client's input stream, which the \
                     lines after it in a file are not. An import cannot supply them. Use `\\copy` \
                     in psql, or export the table as CSV and import that"
                .to_owned(),
        });
    [backslash, copy]
        .into_iter()
        .flatten()
        .min_by_key(|refusal| refusal.line)
}

fn leading_word(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .position(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
        .unwrap_or(bytes.len());
    &bytes[..end]
}

/// The offset of the first bare word `DELIMITER` that is the first token on its line, outside
/// strings, comments and quoted names.
fn first_delimiter_line(sql: &str, lexer: Lexer) -> Option<usize> {
    struct Delimiter<'a> {
        bytes: &'a [u8],
        found: Option<usize>,
    }
    impl Visitor for Delimiter<'_> {
        fn word(&mut self, start: usize, end: usize) {
            if self.found.is_none()
                && self.bytes[start..end].eq_ignore_ascii_case(b"DELIMITER")
                && self.bytes[..start]
                    .iter()
                    .rev()
                    .take_while(|&&byte| byte != b'\n')
                    .all(u8::is_ascii_whitespace)
            {
                self.found = Some(start);
            }
        }
    }
    let bytes = sql.as_bytes();
    let mut visitor = Delimiter { bytes, found: None };
    walk(bytes, lexer, &mut visitor);
    visitor.found
}

/// The offset of the first `\` that is syntax, not text.
fn first_backslash(sql: &str, lexer: Lexer) -> Option<usize> {
    struct Backslash<'a> {
        bytes: &'a [u8],
        pos: usize,
        found: Option<usize>,
    }
    impl Backslash<'_> {
        fn take(&mut self, upto: usize, resume: usize) {
            if self.found.is_none() {
                self.found = self.bytes[self.pos..upto]
                    .iter()
                    .position(|&byte| byte == b'\\')
                    .map(|at| self.pos + at);
            }
            self.pos = resume;
        }
    }
    impl Visitor for Backslash<'_> {
        fn separator(&mut self, at: usize) {
            self.take(at, at + 1);
        }
        fn opaque(&mut self, _kind: OpaqueKind, start: usize, end: usize) {
            self.take(start, end);
        }
        fn word(&mut self, start: usize, end: usize) {
            self.take(start, end);
        }
    }
    let bytes = sql.as_bytes();
    let mut visitor = Backslash {
        bytes,
        pos: 0,
        found: None,
    };
    walk(bytes, lexer, &mut visitor);
    let end = bytes.len();
    visitor.take(end, end);
    visitor.found
}
