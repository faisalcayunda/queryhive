//! A SQL formatter that moves whitespace and nothing else.
//!
//! The contract (blueprint W12 section 3): every non-space byte of the input comes out in the
//! same order, every opaque region (string, quoted identifier, comment, dollar quote) keeps
//! its bytes, and `walk` sees the same regions, words and separators in the output under
//! every reading the caller passes. The formatter holds the original bytes and only rewrites
//! the gaps between atoms, so it cannot change the meaning of a statement by any other route.
//! The result is checked before it is returned; a failed check is [`FormatError::SelfCheck`]
//! and the caller keeps the original text.
//!
//! Keyword case is not this module's business (the keyword list lives in the editor).

use thiserror::Error;

use crate::lex::{lex, TokenKind};
use crate::scan::{walk, EndState, Lexer, OpaqueKind, Visitor};

/// Inputs above this size are refused (a guard for the process, not a quality limit).
pub const FORMAT_MAX_BYTES: usize = 4 << 20;

/// One indent level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indent {
    /// 1 to 8 spaces per level.
    Spaces(u8),
    Tab,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormatOptions {
    pub indent: Indent,
}

impl Default for FormatOptions {
    fn default() -> Self {
        FormatOptions {
            indent: Indent::Spaces(2),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formatted {
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FormatError {
    /// The readings disagree about where a region starts or ends; `line` is the first
    /// differing one (1-based).
    #[error("line {line}: the SQL reads differently under different server settings (a backslash or comment form); write `''` or `E''` explicitly")]
    Ambiguous { line: usize },
    /// The text ends inside a quote, block comment or dollar quote. A line comment with no
    /// newline after it is valid SQL and never lands here.
    #[error("line {line}: {kind:?} is never closed")]
    Unterminated { kind: OpaqueKind, line: usize },
    #[error("the SQL is too large to format")]
    TooLarge,
    /// The result failed its own check: a formatter bug. The original text is untouched.
    #[error("the formatter's self-check failed; the text was not changed")]
    SelfCheck,
}

/// What `walk` saw, kept as ranges so two texts can be compared without copying.
struct Obs {
    regions: Vec<(OpaqueKind, usize, usize)>,
    words: Vec<(usize, usize)>,
    seps: usize,
    end: EndState,
}

impl Visitor for Obs {
    fn separator(&mut self, _at: usize) {
        self.seps += 1;
    }
    fn opaque(&mut self, kind: OpaqueKind, start: usize, end: usize) {
        self.regions.push((kind, start, end));
    }
    fn word(&mut self, start: usize, end: usize) {
        self.words.push((start, end));
    }
}

fn observe(bytes: &[u8], lexer: Lexer) -> Obs {
    let mut obs = Obs {
        regions: Vec::new(),
        words: Vec::new(),
        seps: 0,
        end: EndState::Normal,
    };
    obs.end = walk(bytes, lexer, &mut obs);
    obs
}

/// Same words, regions (kind and bytes), separator count and end state.
fn same(a: &Obs, ab: &[u8], b: &Obs, bb: &[u8]) -> bool {
    a.seps == b.seps
        && a.end == b.end
        && a.regions.len() == b.regions.len()
        && a.words.len() == b.words.len()
        && a.regions
            .iter()
            .zip(&b.regions)
            .all(|(x, y)| x.0 == y.0 && ab[x.1..x.2] == bb[y.1..y.2])
        && a.words
            .iter()
            .zip(&b.words)
            .all(|(x, y)| ab[x.0..x.1] == bb[y.0..y.1])
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn line_of(bytes: &[u8], at: usize) -> usize {
    1 + bytes[..at.min(bytes.len())]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum K {
    Word,
    Opaque(OpaqueKind),
    Comma,
    Semi,
    Open,
    Close,
    /// Numbers, parameters, operator runs and stray bytes.
    Other,
}

struct Atom {
    s: usize,
    e: usize,
    k: K,
    /// Upper-cased text of a word, empty otherwise.
    up: String,
}

impl Atom {
    fn comment(&self) -> bool {
        matches!(
            self.k,
            K::Opaque(OpaqueKind::LineComment | OpaqueKind::BlockComment)
        )
    }
    fn string(&self) -> bool {
        matches!(
            self.k,
            K::Opaque(OpaqueKind::SingleQuote | OpaqueKind::DoubleQuote | OpaqueKind::Backtick)
        )
    }
    fn is(&self, word: &str) -> bool {
        self.k == K::Word && self.up == word
    }
}

fn atoms(bytes: &[u8], lexer: Lexer) -> Vec<Atom> {
    let mut out = Vec::new();
    let plain = |s, e, k| Atom {
        s,
        e,
        k,
        up: String::new(),
    };
    // Bytes no token covers (spaces, controls, high bytes, digits glued to letters): each
    // run of non-space bytes is one atom.
    let stray = |from: usize, to: usize, out: &mut Vec<Atom>| {
        let mut at = from;
        while at < to {
            if is_space(bytes[at]) {
                at += 1;
                continue;
            }
            let start = at;
            while at < to && !is_space(bytes[at]) {
                at += 1;
            }
            out.push(plain(start, at, K::Other));
        }
    };
    let mut pos = 0;
    let mut tokens = Vec::new();
    lex(bytes, 0..bytes.len(), lexer, |t| tokens.push(t));
    for t in tokens {
        if t.start > pos {
            stray(pos, t.start, &mut out);
        }
        match t.kind {
            TokenKind::Opaque(kind) => out.push(plain(t.start, t.end, K::Opaque(kind))),
            TokenKind::Word => out.push(Atom {
                s: t.start,
                e: t.end,
                k: K::Word,
                up: String::from_utf8_lossy(&bytes[t.start..t.end]).to_ascii_uppercase(),
            }),
            TokenKind::Punct => {
                let mut run = t.start;
                for (at, &byte) in bytes.iter().enumerate().take(t.end).skip(t.start) {
                    let k = match byte {
                        b',' => K::Comma,
                        b';' => K::Semi,
                        b'(' => K::Open,
                        b')' => K::Close,
                        _ => continue,
                    };
                    if run < at {
                        out.push(plain(run, at, K::Other));
                    }
                    out.push(plain(at, at + 1, k));
                    run = at + 1;
                }
                if run < t.end {
                    out.push(plain(run, t.end, K::Other));
                }
            }
            TokenKind::Number | TokenKind::Param => out.push(plain(t.start, t.end, K::Other)),
        }
        pos = pos.max(t.end);
    }
    stray(pos, bytes.len(), &mut out);
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Clause {
    Plain,
    /// WHERE, HAVING: `AND`/`OR` start a line.
    Cond,
    /// SELECT, GROUP BY, ORDER BY, SET, WITH: a list of two or more goes one item per line.
    List,
    Join,
    /// `ON` / `USING` after a join, one level in.
    JoinOn,
}

fn next_sig(atoms: &[Atom], from: usize) -> Option<&Atom> {
    atoms.get(from..)?.iter().find(|a| !a.comment())
}

/// Whether atom `i` starts a clause line, given the previous significant atom and whether
/// the frame has seen a join. Only looks at atoms, never at frame state.
fn clause_kind(atoms: &[Atom], i: usize, prev: Option<&Atom>, join: bool) -> Option<Clause> {
    let a = &atoms[i];
    if a.k != K::Word {
        return None;
    }
    let pw = prev
        .filter(|p| p.k == K::Word)
        .map_or("", |p| p.up.as_str());
    let next = next_sig(atoms, i + 1);
    let next_is = |w: &str| next.is_some_and(|n| n.is(w));
    let some = |c, ok: bool| ok.then_some(c);
    match a.up.as_str() {
        "SELECT" => Some(Clause::List),
        "FROM" => some(Clause::Plain, !matches!(pw, "DELETE" | "DISTINCT")),
        "WHERE" | "HAVING" => Some(Clause::Cond),
        "GROUP" | "ORDER" => some(Clause::List, next_is("BY")),
        "WINDOW" | "LIMIT" | "OFFSET" | "FETCH" | "UNION" | "INTERSECT" | "EXCEPT"
        | "RETURNING" | "INSERT" => Some(Clause::Plain),
        "VALUES" => some(Clause::Plain, pw != "DEFAULT"),
        "WITH" => some(
            Clause::List,
            prev.is_none_or(|p| matches!(p.k, K::Semi | K::Open) || p.is("AS")),
        ),
        "UPDATE" => some(Clause::Plain, !matches!(pw, "FOR" | "ON" | "DO" | "KEY")),
        "DELETE" => some(Clause::Plain, pw != "ON"),
        "SET" => some(Clause::List, !matches!(pw, "UPDATE" | "DELETE")),
        "ON" if next_is("CONFLICT") => Some(Clause::Plain),
        "ON" | "USING" => some(Clause::JoinOn, join),
        "NATURAL" | "INNER" | "LEFT" | "RIGHT" | "FULL" | "CROSS" | "JOIN" => {
            let modifier = matches!(
                pw,
                "NATURAL" | "INNER" | "LEFT" | "RIGHT" | "FULL" | "CROSS" | "OUTER"
            );
            let call =
                matches!(a.up.as_str(), "LEFT" | "RIGHT") && next.is_some_and(|n| n.k == K::Open);
            some(Clause::Join, !modifier && !call)
        }
        _ => None,
    }
}

/// Whether the clause that starts at atom `i` has a top-level comma.
///
/// ponytail: the scan stops after `COMMA_SCAN_ATOMS` atoms so unbalanced input stays linear;
/// a list whose first comma lies further out is laid out as a plain run. The decision only
/// reads atoms, never whitespace, so idempotence holds. Raise the cap if it ever matters.
fn has_comma(atoms: &[Atom], i: usize) -> bool {
    const COMMA_SCAN_ATOMS: usize = 2048;
    let mut depth = 0usize;
    let mut prev: Option<&Atom> = Some(&atoms[i]);
    for j in i + 1..atoms.len().min(i + 1 + COMMA_SCAN_ATOMS) {
        let a = &atoms[j];
        match a.k {
            K::Open => depth += 1,
            K::Close if depth == 0 => break,
            K::Close => depth -= 1,
            K::Semi if depth == 0 => break,
            K::Comma if depth == 0 => return true,
            K::Word if depth == 0 && clause_kind(atoms, j, prev, false).is_some() => break,
            _ => {}
        }
        if !a.comment() {
            prev = Some(a);
        }
    }
    false
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sep {
    /// A space, or nothing before `,` `;` `)` and after `(`.
    Soft,
    Newline(usize),
}

#[derive(Default)]
struct Frame {
    /// Starts with SELECT, WITH, VALUES or TABLE: clause words break lines here.
    query: bool,
    base: usize,
    outer: usize,
    and_indent: Option<usize>,
    list: Option<usize>,
    /// The next atom is the first item of a list clause.
    head: bool,
    join: bool,
    between: bool,
    case: u32,
}

fn indent_text(options: &FormatOptions, level: usize) -> String {
    match options.indent {
        Indent::Tab => "\t".repeat(level),
        Indent::Spaces(n) => " ".repeat(usize::from(n.clamp(1, 8)) * level),
    }
}

/// Format `sql`: see the module documentation for what stays fixed.
///
/// `readings` are the lexers the text must be safe under (`Dialect::readings()`; for a
/// generic dialect the caller may pass the union of several). An empty slice means the
/// generic lexer.
pub fn format_sql(
    sql: &str,
    readings: &[Lexer],
    options: &FormatOptions,
) -> Result<Formatted, FormatError> {
    if sql.len() > FORMAT_MAX_BYTES {
        return Err(FormatError::TooLarge);
    }
    let bytes = sql.as_bytes();
    let readings = if readings.is_empty() {
        &[Lexer::GENERIC][..]
    } else {
        readings
    };
    let observed: Vec<Obs> = readings.iter().map(|l| observe(bytes, *l)).collect();
    let first = &observed[0];
    for other in &observed[1..] {
        if other.end != first.end || other.regions.len() != first.regions.len() {
            let at = first.regions.last().map_or(0, |r| r.1);
            return Err(FormatError::Ambiguous {
                line: line_of(bytes, at),
            });
        }
        if let Some((x, _)) = first
            .regions
            .iter()
            .zip(&other.regions)
            .find(|(x, y)| x != y)
        {
            return Err(FormatError::Ambiguous {
                line: line_of(bytes, x.1),
            });
        }
    }
    if let EndState::Open(kind) = first.end {
        if kind != OpaqueKind::LineComment {
            let at = first.regions.last().map_or(0, |r| r.1);
            return Err(FormatError::Unterminated {
                kind,
                line: line_of(bytes, at),
            });
        }
    }

    let atoms = atoms(bytes, readings[0]);
    if atoms.is_empty() {
        return Ok(Formatted {
            text: sql.to_string(),
        });
    }
    let out = lay_out(bytes, &atoms, options).ok_or(FormatError::TooLarge)?;
    let text = String::from_utf8(out).map_err(|_| FormatError::SelfCheck)?;

    // I-F1 to I-F3, and the words under every reading.
    let out_bytes = text.as_bytes();
    let kept = |b: &[u8]| {
        b.iter()
            .copied()
            .filter(|b| !is_space(*b))
            .collect::<Vec<_>>()
    };
    if kept(bytes) != kept(out_bytes) {
        return Err(FormatError::SelfCheck);
    }
    for (lexer, before) in readings.iter().zip(&observed) {
        if !same(before, bytes, &observe(out_bytes, *lexer), out_bytes) {
            return Err(FormatError::SelfCheck);
        }
    }
    Ok(Formatted { text })
}

fn lay_out(bytes: &[u8], atoms: &[Atom], options: &FormatOptions) -> Option<Vec<u8>> {
    // Each query frame adds an indent level, so output grows with depth squared: bound it.
    let limit = (2 * bytes.len()).max(FORMAT_MAX_BYTES);
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() + bytes.len() / 4);
    let mut stack = vec![Frame::default()];
    let mut line_indent = 0usize;
    let mut prev: Option<usize> = None;
    let mut prev_sig: Option<usize> = None;
    for (i, a) in atoms.iter().enumerate() {
        if out.len() > limit {
            return None;
        }
        let f = stack.last().expect("the stack always holds the top frame");
        let ps = prev_sig.map(|p| &atoms[p]);
        let cs = if !a.comment() && (f.query || stack.len() == 1) {
            clause_kind(atoms, i, ps, f.join)
        } else {
            None
        };

        if let Some(p) = prev {
            let gap = &bytes[atoms[p].e..a.s];
            let pa = &atoms[p];
            let gap_newline = gap.contains(&b'\n');
            let mut desired = if a.comment() {
                if gap_newline {
                    Sep::Newline(line_indent)
                } else {
                    Sep::Soft
                }
            } else {
                decide(a, ps, cs, f, &stack)
            };
            // After a line comment the line must end; after a block comment a line break in
            // the input stays one.
            if !a.comment()
                && (pa.k == K::Opaque(OpaqueKind::LineComment) || (pa.comment() && gap_newline))
                && !matches!(desired, Sep::Newline(_))
            {
                desired = Sep::Newline(line_indent);
            }
            if pa.string() && a.string() || gap.contains(&0x0b) {
                out.extend_from_slice(gap);
            } else if gap.is_empty() {
                if matches!(pa.k, K::Comma | K::Semi)
                    && !matches!(a.k, K::Comma | K::Semi | K::Close)
                {
                    match desired {
                        Sep::Soft => out.push(b' '),
                        Sep::Newline(n) => {
                            newline(&mut out, gap, n, options);
                            line_indent = n;
                        }
                    }
                }
            } else {
                match desired {
                    Sep::Newline(n) => {
                        newline(&mut out, gap, n, options);
                        line_indent = n;
                    }
                    Sep::Soft => {
                        let tight = pa.k == K::Open || matches!(a.k, K::Comma | K::Semi | K::Close);
                        if !tight {
                            out.push(b' ');
                        }
                    }
                }
            }
        }
        out.extend_from_slice(&bytes[a.s..a.e]);
        prev = Some(i);
        if a.comment() {
            continue;
        }
        prev_sig = Some(i);

        // Frame state after the atom. Any atom but a list modifier ends the clause head;
        // for `(` that is the outer frame, cleared before the new one is pushed.
        if a.k != K::Word {
            stack.last_mut().expect("top frame").head = false;
        }
        match a.k {
            K::Open => {
                let query = next_sig(atoms, i + 1).is_some_and(|n| {
                    matches!(n.up.as_str(), "SELECT" | "WITH" | "VALUES" | "TABLE")
                });
                stack.push(Frame {
                    query,
                    base: line_indent + 1,
                    outer: line_indent,
                    ..Frame::default()
                });
            }
            K::Close => {
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            K::Semi => {
                stack.truncate(1);
                stack[0] = Frame::default();
            }
            K::Word => {
                let f = stack.last_mut().expect("top frame");
                if f.head && !matches!(a.up.as_str(), "DISTINCT" | "ALL" | "RECURSIVE" | "BY") {
                    f.head = false;
                }
                if let Some(c) = cs {
                    f.and_indent = None;
                    f.list = None;
                    f.head = false;
                    f.between = false;
                    f.case = 0;
                    f.join = matches!(c, Clause::Join | Clause::JoinOn);
                    match c {
                        Clause::Cond => f.and_indent = Some(f.base + 1),
                        Clause::JoinOn => f.and_indent = Some(f.base + 2),
                        Clause::List if has_comma(atoms, i) => {
                            f.list = Some(f.base + 1);
                            f.head = true;
                        }
                        _ => {}
                    }
                }
                match a.up.as_str() {
                    "BETWEEN" => f.between = true,
                    "AND" => f.between = false,
                    "CASE" => f.case += 1,
                    "END" => f.case = f.case.saturating_sub(1),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    // A closing newline in the input stays; a bare end stays bare (`-- note` without one).
    if let Some(l) = atoms.last() {
        if bytes[l.e..].contains(&b'\n') {
            out.push(b'\n');
        }
    }
    Some(out)
}

/// The separator wanted before a non-comment atom.
fn decide(a: &Atom, ps: Option<&Atom>, cs: Option<Clause>, f: &Frame, stack: &[Frame]) -> Sep {
    match a.k {
        K::Comma | K::Semi => return Sep::Soft,
        K::Close => {
            return if stack.len() > 1 && f.query {
                Sep::Newline(f.outer)
            } else {
                Sep::Soft
            }
        }
        _ => {}
    }
    match ps.map(|p| p.k) {
        Some(K::Semi) => return Sep::Newline(0),
        Some(K::Open) => {
            return if f.query {
                Sep::Newline(f.base)
            } else {
                Sep::Soft
            }
        }
        Some(K::Comma) => {
            if let Some(n) = f.list {
                return Sep::Newline(n);
            }
        }
        _ => {}
    }
    if let Some(c) = cs {
        return Sep::Newline(if c == Clause::JoinOn {
            f.base + 1
        } else {
            f.base
        });
    }
    if f.head
        && !matches!(
            a.up.as_str(),
            "DISTINCT" | "ALL" | "RECURSIVE" | "BY" | "ON"
        )
    {
        if let Some(n) = f.list {
            return Sep::Newline(n);
        }
    }
    if a.k == K::Word && matches!(a.up.as_str(), "AND" | "OR") && f.case == 0 {
        if let Some(n) = f.and_indent {
            if !(a.up == "AND" && f.between) {
                return Sep::Newline(n);
            }
        }
    }
    Sep::Soft
}

/// A line break and the indent for level `n`; two or more breaks in the old gap become
/// exactly one blank line.
fn newline(out: &mut Vec<u8>, gap: &[u8], level: usize, options: &FormatOptions) {
    out.push(b'\n');
    if gap.iter().filter(|&&b| b == b'\n').count() >= 2 {
        out.push(b'\n');
    }
    out.extend_from_slice(indent_text(options, level).as_bytes());
}
