//! The editor analysis surface: tree-sitter colours, folds and issues behind UniFFI.
//!
//! Blueprint `docs/architecture/blueprints/fase-4b-editor-analysis.md` §6, commit A: the
//! FFI exists and is tested, but nothing in the app calls it yet (commit B wires it up).
//!
//! [`EditorDocument`] holds the [`TextBuffer`](qh_editor::TextBuffer) under one lock and
//! the [`Analyzer`](qh_editor::Analyzer) under another, so the main thread never waits
//! for analysis. Lock order is always analysis first, then text. Every method returns a
//! `Result`: bad input is an [`EditorError`], never a panic, and there is no `unwrap`
//! on any input path.
//!
//! [`format_sql`] and [`toggle_line_comment`] (blueprint W12 section 3.4) are free functions,
//! not commands: they take the text and a UTF-16 selection and return one replacement range
//! and the selection to set afterwards.

use std::sync::{Arc, Mutex};

use qh_editor::keywords::is_keyword;
use qh_editor::{
    Analyzer, Dialect, EditError, Fold, FoldKind, Issue, IssueKind, Outline, Paint, TextBuffer,
    CEILING_UTF16,
};
use qh_sql::{CommentError, FormatError, FormatOptions, Indent, Lexer, TokenKind};

/// The lexical family a document's SQL is read under. Only the `"` rule differs between
/// them today; the app sends `.generic` until W10-T6 connects one document per tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EditorDialect {
    Generic,
    Postgres,
    Mysql,
    Trino,
}

impl EditorDialect {
    fn dialect(self) -> Dialect {
        match self {
            EditorDialect::Generic => Dialect::Generic,
            EditorDialect::Postgres => Dialect::Postgres,
            EditorDialect::Mysql => Dialect::Mysql,
            EditorDialect::Trino => Dialect::Trino,
        }
    }
}

/// Why an edit or a request was refused. The messages match [`EditError`]'s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum EditorError {
    /// The caller asked about a revision older than the text's.
    #[error("the revision is out of date")]
    Stale,
    /// The range reaches past the text.
    #[error("the range is outside the text")]
    OutOfBounds,
    /// An end of the range falls between the two halves of a surrogate pair.
    #[error("the range splits a character")]
    SplitsCharacter,
    /// The text would be larger than the editor handles.
    #[error("the text is too large")]
    TooLarge,
    /// The analysis state is unusable: a poisoned lock, or a bad range list.
    #[error("the analysis state is malformed")]
    Malformed,
}

impl From<EditError> for EditorError {
    fn from(error: EditError) -> Self {
        match error {
            EditError::Stale => EditorError::Stale,
            EditError::OutOfBounds => EditorError::OutOfBounds,
            EditError::SplitsCharacter => EditorError::SplitsCharacter,
            EditError::TooLarge => EditorError::TooLarge,
            EditError::Malformed => EditorError::Malformed,
        }
    }
}

/// What to draw for a window: flat `(start, len)` pairs, `(start, len, class)` triples
/// with class 1 to 9, and `(start, len, italic)` triples. All offsets are UTF-16 units,
/// ascending and disjoint, and every run and font lies inside one range.
#[derive(Debug, Clone, uniffi::Record)]
pub struct EditorPaint {
    pub revision: u64,
    pub doc_len_utf16: u32,
    pub window_start: u32,
    pub window_len: u32,
    /// Over the ceiling: no runs, and the ranges go back to the base colour.
    pub inactive: bool,
    /// The budget ran out before the window was done.
    pub more_in_window: bool,
    /// Something outside the window needs repainting.
    pub dirty_elsewhere: bool,
    pub ranges: Vec<u32>,
    pub runs: Vec<u32>,
    pub fonts: Vec<u32>,
}

impl From<Paint> for EditorPaint {
    fn from(paint: Paint) -> Self {
        EditorPaint {
            revision: paint.revision,
            doc_len_utf16: paint.doc_len_utf16,
            window_start: paint.window_start,
            window_len: paint.window_len,
            inactive: paint.inactive,
            more_in_window: paint.more_in_window,
            dirty_elsewhere: paint.dirty_elsewhere,
            ranges: paint.ranges,
            runs: paint.runs,
            fonts: paint.fonts,
        }
    }
}

/// The slow-moving parts of a document: the statements Run splits, the folds, and
/// the issues.
#[derive(Debug, Clone, uniffi::Record)]
pub struct EditorOutline {
    pub revision: u64,
    /// `(start, end)` pairs without their `;`, as `statements_with_lines` gives.
    pub statements: Vec<u32>,
    pub folds: Vec<EditorFold>,
    pub issues: Vec<EditorIssue>,
}

impl From<Outline> for EditorOutline {
    fn from(outline: Outline) -> Self {
        EditorOutline {
            revision: outline.revision,
            statements: outline.statements,
            folds: outline.folds.iter().map(EditorFold::from).collect(),
            issues: outline.issues.iter().map(EditorIssue::from).collect(),
        }
    }
}

/// One collapsible region. Lines are 0-based; offsets are UTF-16 units.
#[derive(Debug, Clone, uniffi::Record)]
pub struct EditorFold {
    pub kind: EditorFoldKind,
    /// The line that stays visible.
    pub header_line: u32,
    /// The last line hidden while folded.
    pub last_line: u32,
    /// Offset of the header line's first character: the fold's identity.
    pub header: u32,
    /// The hidden body, whole lines.
    pub body_start: u32,
    pub body_end: u32,
    pub summary: String,
}

impl From<&Fold> for EditorFold {
    fn from(fold: &Fold) -> Self {
        EditorFold {
            kind: EditorFoldKind::from(fold.kind),
            header_line: fold.header_line,
            last_line: fold.last_line,
            header: fold.header,
            body_start: fold.body_start,
            body_end: fold.body_end,
            summary: fold.summary.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EditorFoldKind {
    Statement,
    Cte,
    Subquery,
    Body,
}

impl From<FoldKind> for EditorFoldKind {
    fn from(kind: FoldKind) -> Self {
        match kind {
            FoldKind::Statement => EditorFoldKind::Statement,
            FoldKind::Cte => EditorFoldKind::Cte,
            FoldKind::Subquery => EditorFoldKind::Subquery,
            FoldKind::Body => EditorFoldKind::Body,
        }
    }
}

/// An issue at `[start, start + len)` in UTF-16 units of the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct EditorIssue {
    pub kind: EditorIssueKind,
    pub start: u32,
    pub len: u32,
}

impl From<&Issue> for EditorIssue {
    fn from(issue: &Issue) -> Self {
        EditorIssue {
            kind: EditorIssueKind::from(issue.kind),
            start: issue.start,
            len: issue.len,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EditorIssueKind {
    UnclosedQuote,
    UnclosedIdentifier,
    UnclosedComment,
    UnclosedDollar,
    UnbalancedParen,
    SyntaxError,
    MissingToken,
}

impl From<IssueKind> for EditorIssueKind {
    fn from(kind: IssueKind) -> Self {
        match kind {
            IssueKind::UnclosedQuote => EditorIssueKind::UnclosedQuote,
            IssueKind::UnclosedIdentifier => EditorIssueKind::UnclosedIdentifier,
            IssueKind::UnclosedComment => EditorIssueKind::UnclosedComment,
            IssueKind::UnclosedDollar => EditorIssueKind::UnclosedDollar,
            IssueKind::UnbalancedParen => EditorIssueKind::UnbalancedParen,
            IssueKind::SyntaxError => EditorIssueKind::SyntaxError,
            IssueKind::MissingToken => EditorIssueKind::MissingToken,
        }
    }
}

/// One document's text and its analysis, behind two locks so the main thread never
/// waits for analysis: `replace`, `mark_applied` and `mark_dirty` touch only the text
/// lock, while `paint`, `outline` and `converge` replay the log onto the analyzer.
#[derive(uniffi::Object)]
pub struct EditorDocument {
    text: Mutex<TextBuffer>,
    analysis: Mutex<Analyzer>,
}

#[uniffi::export]
impl EditorDocument {
    /// A document holding `text`, all of which needs painting.
    #[uniffi::constructor]
    pub fn new(text: String, dialect: EditorDialect) -> Result<Arc<Self>, EditorError> {
        Ok(Arc::new(EditorDocument {
            text: Mutex::new(TextBuffer::new(&text)?),
            analysis: Mutex::new(Analyzer::new(&text, dialect.dialect())?),
        }))
    }

    /// The newest revision: 1 at creation, plus one per `replace`.
    pub fn revision(&self) -> Result<u64, EditorError> {
        Ok(self
            .text
            .lock()
            .map_err(|_| EditorError::Malformed)?
            .revision())
    }

    /// The text's length in UTF-16 units.
    pub fn len_utf16(&self) -> Result<u32, EditorError> {
        Ok(self
            .text
            .lock()
            .map_err(|_| EditorError::Malformed)?
            .len_utf16())
    }

    /// The text's line count, for drift detection against the UI's own count.
    pub fn line_count(&self) -> Result<u32, EditorError> {
        Ok(self
            .text
            .lock()
            .map_err(|_| EditorError::Malformed)?
            .line_count())
    }

    /// Replace `[start_utf16, start_utf16 + len_utf16)` with `text`; returns the new
    /// revision. Main thread: shifts the indexes, logs the edit, never parses.
    pub fn replace(
        &self,
        start_utf16: u32,
        len_utf16: u32,
        text: String,
    ) -> Result<u64, EditorError> {
        Ok(self
            .text
            .lock()
            .map_err(|_| EditorError::Malformed)?
            .replace(start_utf16, len_utf16, &text)?)
    }

    /// What to draw for the window, at most `budget_utf16` units of it. Background
    /// thread: replays the log, resynchronizes the statements, re-parses what changed.
    pub fn paint(
        &self,
        revision: u64,
        window_start: u32,
        window_len: u32,
        budget_utf16: u32,
    ) -> Result<EditorPaint, EditorError> {
        let mut analysis = self.analysis.lock().map_err(|_| EditorError::Malformed)?;
        let log = {
            let mut text = self.text.lock().map_err(|_| EditorError::Malformed)?;
            if revision < text.revision() {
                return Err(EditorError::Stale);
            }
            text.drain_log()
        };
        analysis.sync(log)?;
        Ok(EditorPaint::from(analysis.paint(
            window_start,
            window_len,
            budget_utf16,
        )))
    }

    /// The UI applied a paint of `revision` over `ranges` (`(start, len)` pairs).
    /// Main thread. A stale revision is ignored, like the log does.
    pub fn mark_applied(&self, revision: u64, ranges: Vec<u32>) -> Result<(), EditorError> {
        if ranges.len() % 2 != 0 {
            return Err(EditorError::Malformed);
        }
        self.text
            .lock()
            .map_err(|_| EditorError::Malformed)?
            .mark_applied(revision, ranges);
        Ok(())
    }

    /// `[start_utf16, start_utf16 + len_utf16)` must be painted again. Main thread.
    pub fn mark_dirty(&self, start_utf16: u32, len_utf16: u32) -> Result<(), EditorError> {
        Ok(self
            .text
            .lock()
            .map_err(|_| EditorError::Malformed)?
            .mark_dirty(start_utf16, len_utf16)?)
    }

    /// The statements, folds and issues as they are now. Background thread, after the
    /// debounce: replays the log first, so this never sees an older text than `paint`.
    pub fn outline(&self, revision: u64) -> Result<EditorOutline, EditorError> {
        let mut analysis = self.analysis.lock().map_err(|_| EditorError::Malformed)?;
        let log = {
            let mut text = self.text.lock().map_err(|_| EditorError::Malformed)?;
            if revision < text.revision() {
                return Err(EditorError::Stale);
            }
            text.drain_log()
        };
        analysis.sync(log)?;
        Ok(EditorOutline::from(analysis.outline()))
    }

    /// The delimiter pair touching `offset_utf16`, as `[start, len, start, len]` in UTF-16
    /// units of the document, opener first; empty when there is none (blueprint w10 §9.1).
    /// Background thread: replays the log first, like `outline`. `Stale` unless `revision`
    /// is the newest.
    pub fn bracket_pair(&self, revision: u64, offset_utf16: u32) -> Result<Vec<u32>, EditorError> {
        let mut analysis = self.analysis.lock().map_err(|_| EditorError::Malformed)?;
        let log = {
            let mut text = self.text.lock().map_err(|_| EditorError::Malformed)?;
            if revision != text.revision() {
                return Err(EditorError::Stale);
            }
            text.drain_log()
        };
        analysis.sync(log)?;
        Ok(qh_editor::brackets::pair_in(&analysis, offset_utf16))
    }

    /// Re-parse from scratch every statement whose incremental tree holds an error.
    /// Background thread, when typing pauses. Not in blueprint §6's list, which the
    /// idle queue of §7.2 needs but never names: without it §4.4 cannot run.
    pub fn converge(&self) -> Result<(), EditorError> {
        let mut analysis = self.analysis.lock().map_err(|_| EditorError::Malformed)?;
        let log = {
            let mut text = self.text.lock().map_err(|_| EditorError::Malformed)?;
            text.drain_log()
        };
        analysis.sync(log)?;
        analysis.converge();
        Ok(())
    }
}

/// Above this many UTF-16 units (inclusive) a document gets no colour, no trees and
/// no folds; its statements are still counted.
#[uniffi::export]
pub fn editor_ceiling_utf16() -> Result<u32, EditorError> {
    Ok(CEILING_UTF16)
}

/// The statements of `sql` for Run, as `(start, end)` pairs in UTF-16 units: the
/// pieces between separators that hold more than whitespace, comments and `;`.
#[uniffi::export]
pub fn sql_statement_ranges(sql: String, dialect: EditorDialect) -> Result<Vec<u32>, EditorError> {
    qh_editor::statement_ranges(&sql, dialect.dialect()).map_err(EditorError::from)
}

impl EditorDialect {
    /// Whether upper-casing a keyword leaves the statement meaning what it meant. It does in
    /// PostgreSQL and Trino, which fold unquoted names to lower case, and in ANSI (`Generic`),
    /// which folds them to upper case. It does not in MySQL: table and alias names are case
    /// sensitive when `lower_case_table_names=0` (the Linux default), `user`, `status` and
    /// `type` are keywords and common table names, and an alias keeps the case it was written
    /// in as the column label. Blueprint W12 AR #5; O-28 leaves the final choice to DB and RR,
    /// and this is the one line that changes it.
    fn upper_case_is_neutral(self) -> bool {
        !matches!(self, EditorDialect::Mysql)
    }
}

/// What the formatter does to the case of keywords. A Bool setting in the app today
/// (`autoUppercaseKeywords`), so two values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum KeywordCaseWire {
    Preserve,
    Upper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct FormatOptionsWire {
    /// Spaces per indent level, 1 to 8; 0 is a tab.
    pub indent_width: u8,
    pub keyword_case: KeywordCaseWire,
}

/// One replacement of `[start, start + len)` by `replacement`, and the selection to set once it
/// is applied. All offsets are UTF-16 units. `len == 0` with an empty `replacement` means the
/// text does not change; the selection is still the one to keep. Named `Editor*` like the other
/// records here: the app already has its own `TextEdit` (`SQLEditor.swift`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EditorTextEdit {
    pub start: u32,
    pub len: u32,
    pub replacement: String,
    pub selection_start: u32,
    pub selection_len: u32,
}

/// Why [`format_sql`] changed nothing. Lines are 1-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum FormatFfiError {
    /// The SQL reads differently under different server settings (a backslash or comment form).
    #[error("line {line}: the SQL reads differently under different server settings (a backslash or comment form); write `''` or `E''` explicitly, or pick the tab's dialect")]
    Ambiguous { line: u32 },
    /// The text ends inside a quote, a block comment or a dollar quote.
    #[error("line {line}: a quote, block comment or dollar quote is never closed")]
    Unterminated { line: u32 },
    #[error("the SQL is too large to format")]
    TooLarge,
    /// The result failed the formatter's own check: a bug. The text is untouched.
    #[error("the formatter's self-check failed; the text was not changed")]
    SelfCheck,
    #[error("the selection is outside the text")]
    OutOfBounds,
    #[error("the selection splits a character")]
    SplitsCharacter,
}

/// Why [`toggle_line_comment`] changed nothing. Lines are 1-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum CommentFfiError {
    /// The line starts or ends inside a string, quoted name, block comment or dollar quote.
    #[error("line {line}: it starts or ends inside a string, quoted name, block comment or dollar quote; comment whole statements instead")]
    InsideRegion { line: u32 },
    /// Commenting would change the text next to the lines (a string would join the next one).
    #[error("commenting these lines would change the text around them (a string would join the next one, or a comment would run on); comment whole statements instead")]
    Spills,
    #[error("the text is too large")]
    TooLarge,
    #[error("the selection is outside the text")]
    OutOfBounds,
    #[error("the selection splits a character")]
    SplitsCharacter,
}

impl From<EditError> for FormatFfiError {
    fn from(error: EditError) -> Self {
        match error {
            EditError::SplitsCharacter => FormatFfiError::SplitsCharacter,
            EditError::TooLarge => FormatFfiError::TooLarge,
            _ => FormatFfiError::OutOfBounds,
        }
    }
}

impl From<EditError> for CommentFfiError {
    fn from(error: EditError) -> Self {
        match error {
            EditError::SplitsCharacter => CommentFfiError::SplitsCharacter,
            EditError::TooLarge => CommentFfiError::TooLarge,
            _ => CommentFfiError::OutOfBounds,
        }
    }
}

fn line_u32(line: usize) -> u32 {
    u32::try_from(line).unwrap_or(u32::MAX)
}

impl From<FormatError> for FormatFfiError {
    fn from(error: FormatError) -> Self {
        match error {
            FormatError::Ambiguous { line } => FormatFfiError::Ambiguous {
                line: line_u32(line),
            },
            FormatError::Unterminated { line, .. } => FormatFfiError::Unterminated {
                line: line_u32(line),
            },
            FormatError::TooLarge => FormatFfiError::TooLarge,
            FormatError::SelfCheck => FormatFfiError::SelfCheck,
        }
    }
}

impl From<CommentError> for CommentFfiError {
    fn from(error: CommentError) -> Self {
        match error {
            CommentError::InsideRegion { line } => CommentFfiError::InsideRegion {
                line: line_u32(line),
            },
            CommentError::Spills => CommentFfiError::Spills,
            // The lines come from this text, so this is a bug; the selection is what to blame.
            CommentError::OutOfRange => CommentFfiError::OutOfBounds,
        }
    }
}

/// The selection `[start, start + len)` as byte offsets of `buf`.
fn selection_bytes(buf: &TextBuffer, start: u32, len: u32) -> Result<(usize, usize), EditError> {
    let end = start.checked_add(len).ok_or(EditError::OutOfBounds)?;
    Ok((buf.byte_of(start)?, buf.byte_of(end)?))
}

fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

/// The bytes the formatter treats as space (`qh_sql::format_sql` I-F1).
fn blank(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Upper-case every keyword of `text` that is written in lower case. A word touching a `.` is
/// a name, not a keyword (`hive.from` is a column called `from`), as `KeywordCase.swift` does
/// while typing. Only ASCII letters of words change, so I-K1 holds by construction and is
/// checked all the same.
fn upper_keywords(text: &str, lexer: Lexer) -> Result<String, FormatFfiError> {
    let bytes = text.as_bytes();
    let mut out = bytes.to_vec();
    qh_sql::lex(bytes, 0..bytes.len(), lexer, |token| {
        let dotted = (token.start > 0 && bytes[token.start - 1] == b'.')
            || bytes.get(token.end) == Some(&b'.');
        if token.kind == TokenKind::Word && !dotted && is_keyword(&bytes[token.start..token.end]) {
            out[token.start..token.end].make_ascii_uppercase();
        }
    });
    let only_case = bytes
        .iter()
        .zip(&out)
        .all(|(a, b)| a == b || (a.is_ascii_lowercase() && *b == a.to_ascii_uppercase()));
    String::from_utf8(out)
        .ok()
        .filter(|_| only_case)
        .ok_or(FormatFfiError::SelfCheck)
}

/// The smallest replacement that turns `old` into `new`, as `(start, old_end, new_end)`: bytes
/// outside `[start, end)` are the same in both, and the cuts fall between characters.
fn smallest_edit(old: &str, new: &str) -> (usize, usize, usize) {
    let (a, b) = (old.as_bytes(), new.as_bytes());
    let mut head = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    while !(old.is_char_boundary(head) && new.is_char_boundary(head)) {
        head -= 1;
    }
    let room = a.len().min(b.len()) - head;
    let mut tail = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(room)
        .take_while(|(x, y)| x == y)
        .count();
    while !(old.is_char_boundary(a.len() - tail) && new.is_char_boundary(b.len() - tail)) {
        tail -= 1;
    }
    (head, a.len() - tail, b.len() - tail)
}

/// Where the place `at` of `old` is in `new`, which has the same non-blank bytes in the same
/// order. A caret after a token stays after it; one before a token stays before it; one in a
/// gap on a line of its own goes to the same number of lines below the token above it, or to
/// the end of the gap when the new gap is shorter.
fn same_place(old: &[u8], at: usize, new: &[u8]) -> usize {
    let before = old[..at].iter().filter(|b| !blank(**b)).count();
    let end = if before == 0 {
        0
    } else {
        new.iter()
            .enumerate()
            .filter(|(_, b)| !blank(**b))
            .nth(before - 1)
            .map_or(new.len(), |(i, _)| i + 1)
    };
    let gap = new[end..].iter().take_while(|b| blank(**b)).count();
    if at > 0 && !blank(old[at - 1]) {
        return end;
    }
    if old.get(at).is_some_and(|b| !blank(*b)) {
        return end + gap;
    }
    let above = old[..at]
        .iter()
        .rev()
        .take_while(|b| blank(**b))
        .filter(|b| **b == b'\n')
        .count();
    if above == 0 {
        return end;
    }
    new[end..end + gap]
        .iter()
        .enumerate()
        .filter(|(_, b)| **b == b'\n')
        .nth(above - 1)
        .map_or(end + gap, |(i, _)| end + i + 1)
}

/// Format `sql` (blueprint W12 section 3): whitespace moves, and with
/// `KeywordCaseWire::Upper` keywords written in lower case become upper case (except in MySQL,
/// see [`EditorDialect::upper_case_is_neutral`]); nothing else changes. The selection is the
/// one to keep: the result says where it lands.
#[uniffi::export]
pub fn format_sql(
    sql: String,
    dialect: EditorDialect,
    options: FormatOptionsWire,
    selection_start: u32,
    selection_len: u32,
) -> Result<EditorTextEdit, FormatFfiError> {
    let buf = TextBuffer::new(&sql)?;
    let (from, to) = selection_bytes(&buf, selection_start, selection_len)?;
    let readings = dialect.dialect().readings();
    let indent = match options.indent_width {
        0 => Indent::Tab,
        n => Indent::Spaces(n),
    };
    let mut text = qh_sql::format_sql(&sql, readings, &FormatOptions { indent })?.text;
    if options.keyword_case == KeywordCaseWire::Upper && dialect.upper_case_is_neutral() {
        text = upper_keywords(&text, readings[0])?;
    }

    let (head, old_end, new_end) = smallest_edit(&sql, &text);
    // Outside the replaced range a place keeps its offset from the nearer end.
    let place = |at: usize| {
        if at <= head {
            at
        } else if at >= old_end {
            at - old_end + new_end
        } else {
            same_place(sql.as_bytes(), at, text.as_bytes())
        }
    };
    let (new_from, new_to) = (place(from), place(to));
    Ok(EditorTextEdit {
        start: utf16_len(&sql[..head]),
        len: utf16_len(&sql[head..old_end]),
        replacement: text[head..new_end].to_owned(),
        selection_start: utf16_len(&text[..new_from]),
        selection_len: utf16_len(&text[new_from..new_to.max(new_from)]),
    })
}

/// Where each line of `bytes` starts, ending at `\n`, `\r\n` or a lone `\r` like
/// `qh_sql::toggle_line_comment` counts them.
fn line_starts(bytes: &[u8]) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' || (*b == b'\r' && bytes.get(i + 1) != Some(&b'\n')) {
            starts.push(i + 1);
        }
    }
    starts
}

/// Toggle `-- ` on the lines the selection touches (a selection that ends exactly at the start
/// of a line does not touch it). The selection keeps its text: a place at or after a marker
/// moves with it.
#[uniffi::export]
pub fn toggle_line_comment(
    sql: String,
    dialect: EditorDialect,
    selection_start: u32,
    selection_len: u32,
) -> Result<EditorTextEdit, CommentFfiError> {
    let buf = TextBuffer::new(&sql)?;
    let (from, to) = selection_bytes(&buf, selection_start, selection_len)?;
    let bytes = sql.as_bytes();
    let starts = line_starts(bytes);
    let line_at = |at: usize| starts.partition_point(|&s| s <= at);
    let first = line_at(from);
    let mut last = line_at(to);
    if to > from && starts[last - 1] == to {
        last -= 1;
    }
    let edit = qh_sql::toggle_line_comment(&sql, dialect.dialect().readings(), first, last)?;
    let start = buf.utf16_of(edit.start);
    let removed = edit.end - edit.start;
    if edit.replacement.is_empty() && removed == 0 {
        return Ok(EditorTextEdit {
            start,
            len: 0,
            replacement: String::new(),
            selection_start,
            selection_len,
        });
    }

    // One marker per non-blank line: where it sits and how many bytes it cuts. They are
    // ASCII, so a byte shift is a UTF-16 shift.
    let uncomment = edit.replacement.len() < removed;
    let body = (first..=last).filter_map(|n| {
        let at = starts[n - 1];
        let end = bytes[at..]
            .iter()
            .position(|b| matches!(b, b'\n' | b'\r'))
            .map_or(bytes.len(), |p| at + p);
        (!bytes[at..end].iter().all(|b| blank(*b))).then_some((at, end))
    });
    let shared = body.clone().next().map_or(0, |(at, _)| edit.start - at);
    let markers: Vec<(usize, usize)> = body
        .map(|(at, end)| {
            if uncomment {
                let indent = at
                    + bytes[at..end]
                        .iter()
                        .take_while(|b| matches!(b, b' ' | b'\t'))
                        .count();
                (
                    indent,
                    2 + usize::from(bytes.get(indent + 2) == Some(&b' ')),
                )
            } else {
                (at + shared, 0)
            }
        })
        .collect();
    let added = if uncomment { 0 } else { "-- ".len() };
    let moved = |at: usize| {
        let hit = markers.iter().filter(|m| at >= m.0);
        let cut: usize = hit.clone().map(|m| m.1.min(at - m.0)).sum();
        (added * hit.count()) as i64 - cut as i64
    };
    let new_from = i64::from(selection_start) + moved(from);
    let new_len = i64::from(selection_len) + moved(to) - moved(from);
    Ok(EditorTextEdit {
        start,
        len: buf.utf16_of(edit.end) - start,
        replacement: edit.replacement,
        selection_start: new_from.max(0) as u32,
        selection_len: new_len.max(0) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paint_full(text: &str, revision: u64) -> EditorPaint {
        let doc = EditorDocument::new(text.to_owned(), EditorDialect::Generic)
            .expect("a small document opens");
        let len = doc.len_utf16().expect("length reads");
        doc.paint(revision, 0, len, u32::MAX)
            .expect("a fresh revision paints")
    }

    #[test]
    fn the_blueprint_example_paints_exactly() {
        // Blueprint §6's locked example: `new("select :a -- c", Generic)`.
        let paint = paint_full("select :a -- c", 1);
        assert_eq!(paint.revision, 1);
        assert_eq!(paint.doc_len_utf16, 14);
        assert!(!paint.inactive);
        assert!(!paint.more_in_window);
        assert!(!paint.dirty_elsewhere);
        assert_eq!(paint.ranges, vec![0, 14]);
        assert_eq!(paint.runs, vec![0, 6, 5, 7, 2, 9, 10, 4, 1]);
        assert_eq!(paint.fonts, vec![0, 10, 0, 10, 4, 1]);

        // After applying and appending one character, only the longer comment
        // comes back, with its italic.
        let doc = EditorDocument::new("select :a -- c".to_owned(), EditorDialect::Generic)
            .expect("a small document opens");
        doc.mark_applied(1, vec![0, 14])
            .expect("an even range applies");
        assert_eq!(
            doc.replace(14, 0, "x".to_owned()).expect("appending works"),
            2
        );
        let paint = doc
            .paint(2, 0, 15, u32::MAX)
            .expect("the new revision paints");
        assert_eq!(paint.ranges, vec![10, 5]);
        assert_eq!(paint.runs, vec![10, 5, 1]);
        assert_eq!(paint.fonts, vec![14, 1, 1]);
    }

    #[test]
    fn an_old_revision_is_stale_and_bad_ranges_are_refused() {
        let doc = EditorDocument::new("select 1".to_owned(), EditorDialect::Generic)
            .expect("a small document opens");
        assert_eq!(
            doc.paint(0, 0, 8, u32::MAX).unwrap_err(),
            EditorError::Stale
        );
        assert_eq!(doc.outline(0).unwrap_err(), EditorError::Stale);
        // A face is two UTF-16 units; ending a range between them splits it.
        let doc = EditorDocument::new("a😀b".to_owned(), EditorDialect::Generic)
            .expect("a small document opens");
        assert_eq!(
            doc.replace(1, 1, "".to_owned()).unwrap_err(),
            EditorError::SplitsCharacter
        );
        assert_eq!(
            doc.replace(0, 99, "".to_owned()).unwrap_err(),
            EditorError::OutOfBounds
        );
        assert_eq!(
            doc.mark_applied(1, vec![0]).unwrap_err(),
            EditorError::Malformed
        );
    }

    #[test]
    fn a_bracket_pair_follows_the_edits_and_refuses_a_stale_revision() {
        let doc = EditorDocument::new("select f(1)".to_owned(), EditorDialect::Generic)
            .expect("a small document opens");
        assert_eq!(doc.bracket_pair(1, 9).unwrap(), vec![8, 1, 10, 1]);
        let revision = doc.replace(9, 0, "(0)+".to_owned()).expect("an edit lands");
        // The old revision is refused, the new one pairs against the edited text.
        assert_eq!(doc.bracket_pair(1, 9).unwrap_err(), EditorError::Stale);
        assert_eq!(doc.bracket_pair(revision, 10).unwrap(), vec![9, 1, 11, 1]);
        // Nothing to pair, past the end, and inside a surrogate pair.
        assert_eq!(doc.bracket_pair(revision, 0).unwrap(), Vec::<u32>::new());
        assert_eq!(doc.bracket_pair(revision, 99).unwrap(), Vec::<u32>::new());
        let face = EditorDocument::new("('😀')".to_owned(), EditorDialect::Generic).unwrap();
        assert_eq!(face.bracket_pair(1, 1).unwrap(), vec![0, 1, 5, 1]);
        assert_eq!(face.bracket_pair(1, 3).unwrap(), Vec::<u32>::new());
    }

    #[test]
    fn statement_ranges_match_the_ui_pieces() {
        assert_eq!(
            sql_statement_ranges("select 1; select 2;".to_owned(), EditorDialect::Generic)
                .expect("two statements split"),
            vec![0, 8, 9, 18]
        );
        assert_eq!(
            sql_statement_ranges("-- only a comment".to_owned(), EditorDialect::Generic)
                .expect("a comment splits"),
            Vec::<u32>::new()
        );
    }

    #[test]
    fn the_ceiling_is_the_crate_constant() {
        assert_eq!(
            editor_ceiling_utf16().expect("the ceiling reads"),
            qh_editor::CEILING_UTF16
        );
    }

    #[test]
    fn an_outline_holds_statements_folds_and_issues() {
        let doc = EditorDocument::new(
            "select 1;\nselect 'oops;\n".to_owned(),
            EditorDialect::Generic,
        )
        .expect("a small document opens");
        let outline = doc.outline(1).expect("a fresh revision outlines");
        assert_eq!(outline.revision, 1);
        assert_eq!(outline.statements, vec![0, 8, 9, 24]);
        assert!(
            outline
                .issues
                .iter()
                .any(|issue| issue.kind == EditorIssueKind::UnclosedQuote),
            "the unclosed quote is reported: {:?}",
            outline.issues
        );
        assert!(
            outline
                .folds
                .iter()
                .all(|fold| fold.last_line > fold.header_line),
            "every fold hides a line: {:?}",
            outline.folds
        );
    }

    // ---- format_sql and toggle_line_comment ----

    fn upper() -> FormatOptionsWire {
        FormatOptionsWire {
            indent_width: 2,
            keyword_case: KeywordCaseWire::Upper,
        }
    }

    fn keep() -> FormatOptionsWire {
        FormatOptionsWire {
            indent_width: 2,
            keyword_case: KeywordCaseWire::Preserve,
        }
    }

    /// Apply `edit` to `text` the way the editor does, in UTF-16 units.
    fn apply(text: &str, edit: &EditorTextEdit) -> String {
        let mut units: Vec<u16> = text.encode_utf16().collect();
        let (start, end) = (edit.start as usize, (edit.start + edit.len) as usize);
        units.splice(start..end, edit.replacement.encode_utf16());
        String::from_utf16(&units).expect("the edit keeps the text valid")
    }

    /// The text the selection covers, or `|` marks the caret, after `edit` is applied.
    fn marked(text: &str, edit: &EditorTextEdit) -> String {
        let mut units: Vec<u16> = apply(text, edit).encode_utf16().collect();
        let (a, b) = (
            edit.selection_start as usize,
            (edit.selection_start + edit.selection_len) as usize,
        );
        units.splice(b..b, "|".encode_utf16());
        if a != b {
            units.splice(a..a, "|".encode_utf16());
        }
        String::from_utf16(&units).expect("the marks keep the text valid")
    }

    fn fmt(sql: &str, dialect: EditorDialect, options: FormatOptionsWire) -> String {
        let edit = format_sql(sql.to_owned(), dialect, options, 0, 0).expect("the SQL formats");
        apply(sql, &edit)
    }

    #[test]
    fn format_moves_whitespace_and_upper_cases_keywords() {
        let sql = "select a,b from t where x=1 and y=2";
        let kept = fmt(sql, EditorDialect::Postgres, keep());
        assert_eq!(kept, "select\n  a,\n  b\nfrom t\nwhere x=1\n  and y=2");
        let shouted = fmt(sql, EditorDialect::Postgres, upper());
        assert_eq!(shouted, "SELECT\n  a,\n  b\nFROM t\nWHERE x=1\n  AND y=2");
        // A tab, and the indent width.
        let tabbed = fmt(
            sql,
            EditorDialect::Postgres,
            FormatOptionsWire {
                indent_width: 0,
                keyword_case: KeywordCaseWire::Preserve,
            },
        );
        assert!(tabbed.contains("\n\tb"), "{tabbed:?}");
    }

    #[test]
    fn upper_case_leaves_strings_comments_and_qualified_names_alone() {
        let sql = "select o.status, 'from where' as s, \"select\" from orders o -- where\nwhere o.type = 1";
        let out = fmt(sql, EditorDialect::Postgres, upper());
        assert_eq!(
            out,
            "SELECT\n  o.status,\n  'from where' AS s,\n  \"select\"\nFROM orders o -- where\nWHERE o.type = 1"
        );
        // Same bytes apart from the case of the changed words (I-K1), and the same kind of
        // statement for Safe Mode.
        assert!(out.eq_ignore_ascii_case(&fmt(sql, EditorDialect::Postgres, keep())));
        for write in ["delete from t where a = 1", "drop table t", "select 1"] {
            let readings = EditorDialect::Postgres.dialect().readings();
            let shouted = fmt(write, EditorDialect::Postgres, upper());
            assert_eq!(
                qh_sql::classify_readings(write, readings),
                qh_sql::classify_readings(&shouted, readings),
                "{write}"
            );
        }
    }

    #[test]
    fn mysql_keeps_the_case_of_every_word() {
        let sql = "select 1 from user";
        assert_eq!(
            fmt(sql, EditorDialect::Mysql, upper()),
            fmt(sql, EditorDialect::Mysql, keep())
        );
        assert_eq!(
            fmt(sql, EditorDialect::Mysql, upper()),
            "select 1\nfrom user"
        );
        // The other dialects do change it.
        for dialect in [
            EditorDialect::Generic,
            EditorDialect::Postgres,
            EditorDialect::Trino,
        ] {
            assert_eq!(
                fmt(sql, dialect, upper()),
                "SELECT 1\nFROM USER",
                "{dialect:?}"
            );
        }
    }

    #[test]
    fn format_refuses_what_it_cannot_read_and_names_the_line() {
        // MySQL reads `\'` as an escaped quote or as a backslash and a closing quote.
        let ambiguous = "select 1;\nselect 'a\\', 'b'";
        assert_eq!(
            format_sql(ambiguous.to_owned(), EditorDialect::Mysql, keep(), 0, 0).unwrap_err(),
            FormatFfiError::Ambiguous { line: 2 }
        );
        assert_eq!(
            format_sql(
                "select 1\n, 'oops".to_owned(),
                EditorDialect::Generic,
                keep(),
                0,
                0
            )
            .unwrap_err(),
            FormatFfiError::Unterminated { line: 2 }
        );
        let huge = "a".repeat(qh_sql::FORMAT_MAX_BYTES + 1);
        assert_eq!(
            format_sql(huge, EditorDialect::Generic, keep(), 0, 0).unwrap_err(),
            FormatFfiError::TooLarge
        );
    }

    #[test]
    fn formatting_twice_changes_nothing_and_keeps_the_selection() {
        let sql = "select a,b from t";
        let once = fmt(sql, EditorDialect::Postgres, upper());
        // The caret sits in the gap before `t`, which the formatter does not touch.
        let at = once.len() as u32 - 1;
        let edit = format_sql(once.clone(), EditorDialect::Postgres, upper(), at, 1).unwrap();
        assert_eq!(
            edit,
            EditorTextEdit {
                start: edit.start,
                len: 0,
                replacement: String::new(),
                selection_start: at,
                selection_len: 1,
            }
        );
    }

    #[test]
    fn format_offsets_are_utf16_units() {
        // Two surrogate pairs ahead of the part that changes: 4 UTF-16 units, 8 bytes.
        let sql = "-- 😀😀\nselect a,b from t";
        let edit = format_sql(sql.to_owned(), EditorDialect::Postgres, keep(), 0, 0).unwrap();
        assert_eq!(apply(sql, &edit), "-- 😀😀\nselect\n  a,\n  b\nfrom t");
        // The edit starts where the first change does: right after `select`.
        assert_eq!(edit.start, "-- 😀😀\nselect".encode_utf16().count() as u32);
        // A caret between the two halves of a pair is refused, one past the end too.
        let split = "-- 😀".encode_utf16().count() as u32 - 1;
        assert_eq!(
            format_sql(sql.to_owned(), EditorDialect::Postgres, keep(), split, 0).unwrap_err(),
            FormatFfiError::SplitsCharacter
        );
        let past = sql.encode_utf16().count() as u32 + 1;
        assert_eq!(
            format_sql(sql.to_owned(), EditorDialect::Postgres, keep(), past, 0).unwrap_err(),
            FormatFfiError::OutOfBounds
        );
        assert_eq!(
            format_sql(sql.to_owned(), EditorDialect::Postgres, keep(), u32::MAX, 5).unwrap_err(),
            FormatFfiError::OutOfBounds
        );
    }

    #[test]
    fn the_selection_follows_its_token_when_whitespace_moves() {
        let sql = "select   a  ,  b   from   t";
        // Before `from`, after `b`, inside `from`, and the whole `b   from`.
        let before_from = sql.find("from").unwrap() as u32;
        let cases = [
            (before_from, 0, "select\n  a,\n  b\n|from t"),
            (
                sql.find("b").unwrap() as u32 + 1,
                0,
                "select\n  a,\n  b|\nfrom t",
            ),
            (before_from + 2, 0, "select\n  a,\n  b\nfr|om t"),
            (
                sql.find("b").unwrap() as u32,
                8,
                "select\n  a,\n  |b\nfrom| t",
            ),
        ];
        for (start, len, want) in cases {
            let edit =
                format_sql(sql.to_owned(), EditorDialect::Postgres, keep(), start, len).unwrap();
            assert_eq!(marked(sql, &edit), want, "selection {start}+{len}");
        }
    }

    #[test]
    fn a_caret_on_a_line_of_its_own_stays_on_one() {
        let sql = "select 1;\n\n\n\nselect 2;";
        // On the second of three blank lines: the formatter keeps one blank line, so the
        // caret lands on it.
        let at = "select 1;\n\n".len() as u32;
        let edit = format_sql(sql.to_owned(), EditorDialect::Postgres, keep(), at, 0).unwrap();
        assert_eq!(marked(sql, &edit), "select 1;\n\n|select 2;");
    }

    fn toggle(sql: &str, start: u32, len: u32) -> Result<EditorTextEdit, CommentFfiError> {
        toggle_line_comment(sql.to_owned(), EditorDialect::Postgres, start, len)
    }

    #[test]
    fn toggling_comments_and_uncomments_the_touched_lines() {
        let sql = "select 1,\n  2\nfrom t;\nselect 3;";
        // The first two lines: the selection ends mid-line 2.
        let edit = toggle(sql, 0, "select 1,\n  2".len() as u32).unwrap();
        let commented = apply(sql, &edit);
        assert_eq!(commented, "-- select 1,\n--   2\nfrom t;\nselect 3;");
        assert_eq!(
            marked(sql, &edit),
            "-- |select 1,\n--   2|\nfrom t;\nselect 3;"
        );
        // And back: the selection covers the same text again.
        let back = toggle(&commented, edit.selection_start, edit.selection_len).unwrap();
        assert_eq!(apply(&commented, &back), sql);
        assert_eq!(
            marked(&commented, &back),
            "|select 1,\n  2|\nfrom t;\nselect 3;"
        );
    }

    #[test]
    fn a_selection_ending_at_a_line_start_leaves_that_line_alone() {
        let sql = "a\nb\nc";
        let edit = toggle(sql, 0, 4).unwrap(); // "a\nb\n"
        assert_eq!(apply(sql, &edit), "-- a\n-- b\nc");
        // A caret on a line comments that line, even at its very start.
        let edit = toggle(sql, 4, 0).unwrap();
        assert_eq!(apply(sql, &edit), "a\nb\n-- c");
        assert_eq!(marked(sql, &edit), "a\nb\n-- |c");
    }

    #[test]
    fn a_marker_goes_in_at_the_smallest_indent_and_blank_lines_stay_blank() {
        let sql = "    a\n\n  b\n      c";
        let edit = toggle(sql, 0, sql.len() as u32).unwrap();
        assert_eq!(apply(sql, &edit), "  --   a\n\n  -- b\n  --     c");
    }

    #[test]
    fn comment_offsets_are_utf16_and_a_blank_selection_changes_nothing() {
        // A face before the line: the edit sits after 2 UTF-16 units of it.
        let sql = "'😀' ,\nx";
        let edit = toggle(sql, 7, 0).unwrap();
        assert_eq!(apply(sql, &edit), "'😀' ,\n-- x");
        assert_eq!(edit.start, "'😀' ,\n".encode_utf16().count() as u32);
        // CRLF and a lone CR end a line like LF does.
        let edit = toggle("a\r\nb\r\nc", 3, 0).unwrap();
        assert_eq!(apply("a\r\nb\r\nc", &edit), "a\r\n-- b\r\nc");
        let edit = toggle("a\rb\rc", 2, 0).unwrap();
        assert_eq!(apply("a\rb\rc", &edit), "a\r-- b\rc");
        let blank = "a\n\nb";
        let edit = toggle(blank, 2, 0).unwrap();
        assert_eq!((edit.len, edit.replacement.as_str()), (0, ""));
        assert_eq!((edit.selection_start, edit.selection_len), (2, 0));
    }

    #[test]
    fn comment_refuses_inside_a_string_and_bad_selections() {
        let sql = "select 'a\nb', 1";
        // Line 1 ends inside the string, line 2 starts inside it.
        assert_eq!(
            toggle(sql, 0, 0).unwrap_err(),
            CommentFfiError::InsideRegion { line: 1 }
        );
        assert_eq!(
            toggle(sql, 12, 0).unwrap_err(),
            CommentFfiError::InsideRegion { line: 2 }
        );
        let split = "'😀".encode_utf16().count() as u32 - 1;
        assert_eq!(
            toggle("'😀' a", split, 0).unwrap_err(),
            CommentFfiError::SplitsCharacter
        );
        assert_eq!(toggle("a", 2, 0).unwrap_err(), CommentFfiError::OutOfBounds);
    }
}
