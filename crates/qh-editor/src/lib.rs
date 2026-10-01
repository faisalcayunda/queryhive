//! Editor analysis for QueryHive: which statements a text holds, and for each one a
//! tree-sitter tree, the colour classes of its tokens, its folds and its issues.
//!
//! The design is `docs/architecture/blueprints/fase-4b-editor-analysis.md`. The short version:
//!
//! * Statement boundaries come from `qh_sql::walk`, the scanner the engine and Safe Mode use,
//!   never from a tree ([`statements`]).
//! * Each statement is parsed on its own ([`syntax`]), so a keystroke re-parses one small tree
//!   and an error stays inside its statement.
//! * Colours are a function of the statement's text alone: the tree gives the classes, and
//!   everything the tree does not cover is lexed by `qh_sql::lex` ([`classify`]).
//! * Offsets in and out are UTF-16 code units, the unit `NSString` uses ([`text`]).
//!
//! Everything here is plain single-threaded data. [`Document`] is the synchronous whole: a
//! text buffer for the main thread and an [`Analyzer`] for a background thread that replays the
//! buffer's edit log onto a mirror of its own. `qh-ffi` will split those two across two locks.
//!
//! No `unsafe`: the C parser sits behind `qh-sql-grammar`.

#![forbid(unsafe_code)]

pub mod classify;
pub mod folds;
pub mod issues;
pub mod keywords;
pub mod paint;
pub mod statements;
pub mod syntax;
pub mod text;
pub mod view;

pub use classify::{Class, Tok};
pub use folds::{Fold, FoldKind};
pub use issues::{Issue, IssueKind};
pub use paint::{Analyzer, Document, Outline, Paint, Stats};
pub use qh_sql::Dialect;
pub use statements::statement_ranges;
pub use text::{EditError, LogEntry, TextBuffer};

/// Above this many UTF-16 units (inclusive) a document gets no colour, no trees and no folds;
/// its statements are still counted.
pub const CEILING_UTF16: u32 = 2_000_000;

/// A statement longer than this many bytes gets no tree and its tokens are not cached: the
/// lexer alone colours it. A single 2 MB `INSERT` re-parses in 7 to 17 ms per keystroke;
/// the lexer needs a few milliseconds for the window.
pub const GIANT_STATEMENT_BYTES: usize = 256 * 1024;

/// Source bytes of statements that may hold a tree at once, per document (about 32 MB of
/// trees at 36 to 54 bytes per source byte). Least recently used trees are dropped first;
/// tokens, folds and issues stay cached.
pub const TREE_CACHE_BYTES: usize = 640 * 1024;
