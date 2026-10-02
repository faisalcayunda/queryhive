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

use std::sync::{Arc, Mutex};

use qh_editor::{
    Analyzer, Dialect, EditError, Fold, FoldKind, Issue, IssueKind, Outline, Paint, TextBuffer,
    CEILING_UTF16,
};

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
}
