//! The statements of a text, from the engine's own scanner.
//!
//! A statement is `[start, next.start)`: it runs from just after the previous separator through
//! its own `;`, so the parser sees the terminator, and the last one runs to the end of the text
//! (empty when the text ends with a `;`). The list always partitions the text, which makes the
//! statement holding a byte a binary search.
//!
//! Boundaries come from `qh_sql::walk` under the connection's [`Dialect`], never from a tree:
//! Run, Safe Mode and this list must agree about where a statement ends, and a tree does not
//! (an ERROR node swallows its `;`, and `BEGIN; …; COMMIT` becomes one node).

use qh_sql::{first_significant, walk, Dialect, Lexer, Visitor};
use tree_sitter::Tree;

use crate::classify::Tok;
use crate::folds::RelFold;
use crate::issues::RelIssue;
use crate::text::{EditError, TextBuffer};
use crate::GIANT_STATEMENT_BYTES;

/// One statement and everything cached about it. Offsets inside the caches are relative to
/// `start`, so an edit before the statement moves nothing but `start`.
#[derive(Default)]
pub struct Stmt {
    /// Byte offset of the statement's first byte.
    pub start: usize,
    pub(crate) tree: Option<Tree>,
    /// Edits were applied to `tree` that it has not been re-parsed for.
    pub(crate) tree_stale: bool,
    /// `tree` was re-parsed from an earlier tree, so it may differ from a fresh parse.
    pub(crate) incremental: bool,
    /// Source bytes `tree` is counted as, for the cache budget.
    pub(crate) tree_cost: usize,
    pub(crate) last_use: u64,
    /// The colour tokens. When `tokens_valid` is false they are what the tokens were before
    /// the pending edits, moved through them; the difference to the new ones is what needs
    /// repainting.
    pub(crate) tokens: Option<Vec<Tok>>,
    pub(crate) tokens_valid: bool,
    /// Folds and issues found in the tree and text as they are now.
    pub(crate) outline: Option<(Vec<RelFold>, Vec<RelIssue>)>,
}

pub struct Statements {
    pub(crate) list: Vec<Stmt>,
    lexer: Lexer,
}

/// What [`Statements::apply_edit`] did to the list.
pub struct Change {
    /// The index of the first statement that is new or reused.
    pub first: usize,
    /// How many statements stand where the old ones were.
    pub count: usize,
    /// `count == 1` and the statement is the old one, with its caches, not a fresh one.
    pub reused: bool,
    /// Tree cost of the statements that were dropped.
    pub dropped_tree_cost: usize,
}

struct Separators<'a>(&'a mut Vec<usize>, usize);

impl Visitor for Separators<'_> {
    fn separator(&mut self, at: usize) {
        self.0.push(self.1 + at);
    }
}

impl Statements {
    pub fn new(text: &str, lexer: Lexer) -> Statements {
        let mut separators = Vec::new();
        walk(text.as_bytes(), lexer, &mut Separators(&mut separators, 0));
        let mut list = vec![Stmt::default()];
        list.extend(separators.iter().map(|&at| Stmt {
            start: at + 1,
            ..Stmt::default()
        }));
        Statements { list, lexer }
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    // Not called anywhere; clippy's `len_without_is_empty` asks for it beside a public `len`.
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Where statement `i` ends (exclusive): the next one's start, or the end of the text.
    pub fn end_of(&self, i: usize, text_len: usize) -> usize {
        self.list.get(i + 1).map_or(text_len, |next| next.start)
    }

    /// Whether statement `i` is too big to parse.
    pub fn is_giant(&self, i: usize, text_len: usize) -> bool {
        self.end_of(i, text_len) - self.list[i].start > GIANT_STATEMENT_BYTES
    }

    /// The index of the statement holding `byte` (the later one, at a boundary).
    pub fn index_of(&self, byte: usize) -> usize {
        self.list.partition_point(|s| s.start <= byte) - 1
    }

    /// Update the list for the edit that replaced `old_len` bytes at `start` with `new_len`
    /// bytes, given the text after the edit.
    ///
    /// The walk restarts at the statement holding `start`, which is in the normal state because
    /// the byte before it is a separator or the text's start. It stops at the first old
    /// separator beyond the edit if the walk finds a separator at that same place (shifted):
    /// from there both texts are the same bytes in the same state, so the old statements from
    /// there on stand. Otherwise (a quote that flipped, say) it runs to the end.
    pub fn apply_edit(
        &mut self,
        text: &str,
        start: usize,
        old_len: usize,
        new_len: usize,
    ) -> Change {
        let delta = new_len as isize - old_len as isize;
        let old_end = start + old_len;
        let first = self.index_of(start);
        let from = self.list[first].start;

        // The last old statement the walk replaces: the one ending at the first separator
        // beyond the edit, or the last one.
        let mut last = first;
        while last + 1 < self.list.len() && self.list[last + 1].start - 1 < old_end {
            last += 1;
        }
        let bytes = text.as_bytes();
        let mut found = Vec::new();
        let mut converged = false;
        if last + 1 < self.list.len() {
            let target = (self.list[last + 1].start as isize + delta) as usize;
            walk(
                &bytes[from..target],
                self.lexer,
                &mut Separators(&mut found, from),
            );
            converged = found.last() == Some(&(target - 1));
        }
        if !converged {
            found.clear();
            walk(
                &bytes[from..],
                self.lexer,
                &mut Separators(&mut found, from),
            );
            last = self.list.len() - 1;
        }

        let count = found.len() + usize::from(!converged);
        let mut old: Vec<Stmt> = self.list.drain(first..=last).collect();
        let reused = old.len() == 1 && count == 1;
        let dropped_tree_cost = if reused {
            0
        } else {
            old.iter().map(|s| s.tree_cost).sum()
        };
        let mut fresh = Vec::with_capacity(count);
        if reused {
            fresh.push(old.pop().expect("one old statement"));
        } else {
            fresh.push(Stmt {
                start: from,
                ..Stmt::default()
            });
            fresh.extend(found.iter().take(count - 1).map(|&at| Stmt {
                start: at + 1,
                ..Stmt::default()
            }));
        }
        // The walk found `count - 1` separators that open a following statement of the new
        // list, plus (when converged) the one that closes the last statement: the old
        // statement after `last` starts right past it.
        self.list.splice(first..first, fresh);
        for stmt in &mut self.list[first + count..] {
            stmt.start = (stmt.start as isize + delta) as usize;
        }
        Change {
            first,
            count,
            reused,
            dropped_tree_cost,
        }
    }

    /// The statements the UI shows, as byte ranges without the `;`: the pieces between
    /// separators that hold more than whitespace, comments and `;`. The same pieces as
    /// `qh_sql::statements_with_lines_dialect`.
    pub fn ranges_for_ui(&self, text: &str) -> Vec<(usize, usize)> {
        let mut out = Vec::with_capacity(self.list.len());
        for (i, stmt) in self.list.iter().enumerate() {
            let end = self
                .list
                .get(i + 1)
                .map_or(text.len(), |next| next.start - 1);
            if first_significant(&text[stmt.start..end], self.lexer).is_some() {
                out.push((stmt.start, end));
            }
        }
        out
    }
}

/// The statements of `sql` for Run, as pairs `(start, end)` in UTF-16 units: what
/// `ranges_for_ui` gives, under `dialect`.
pub fn statement_ranges(sql: &str, dialect: Dialect) -> Result<Vec<u32>, EditError> {
    let buffer = TextBuffer::new(sql)?;
    let statements = Statements::new(sql, Lexer::from(dialect));
    let mut cursor = buffer.cursor();
    let mut out = Vec::new();
    for (start, end) in statements.ranges_for_ui(sql) {
        out.push(cursor.at(start));
        out.push(cursor.at(end));
    }
    Ok(out)
}
