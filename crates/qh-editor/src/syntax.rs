//! One tree-sitter parser, and the trees of a document's statements.
//!
//! A statement's tree is edited in place when its text changes and re-parsed from the old tree
//! the next time something needs it. An incremental tree can differ from a fresh one where the
//! grammar recovered from an error (measured: 14 to 16% of random edits, always in trees with
//! an ERROR), so [`Analyzer::converge`](crate::Analyzer::converge) re-parses those from scratch
//! once typing pauses.

use tree_sitter::{InputEdit, Parser, Point};

use crate::statements::Stmt;
use crate::view;

pub struct Syntax {
    parser: Parser,
}

impl Syntax {
    pub fn new() -> Syntax {
        let mut parser = Parser::new();
        parser
            .set_language(&qh_sql_grammar::LANGUAGE.into())
            .expect("the vendored grammar's ABI is one the runtime supports");
        Syntax { parser }
    }

    /// Make `stmt.tree` match `text` (the statement's text): parse it if it has no tree, and
    /// re-parse from the old tree if edits were applied to it. A statement with no tree after
    /// this (the parser gave up) is coloured by the lexer alone.
    ///
    /// ponytail: no cancellation. A parse of a statement under the giant limit takes well under
    /// a millisecond typically; add a progress callback if a pathological one shows up.
    pub fn ensure(&mut self, stmt: &mut Stmt, text: &str) {
        if stmt.tree.is_some() && !stmt.tree_stale {
            return;
        }
        let bytes = view::parse_bytes(text.as_bytes());
        let old = if stmt.tree_stale {
            stmt.tree.as_ref()
        } else {
            None
        };
        stmt.incremental = old.is_some();
        stmt.tree = self.parser.parse(&*bytes, old);
        stmt.tree_stale = false;
        stmt.tree_cost = if stmt.tree.is_some() { text.len() } else { 0 };
    }

    /// Parse `text` from scratch, whatever the statement holds.
    pub fn parse_fresh(&mut self, stmt: &mut Stmt, text: &str) {
        stmt.tree = None;
        stmt.tree_stale = false;
        self.ensure(stmt, text);
        stmt.incremental = false;
    }
}

impl Default for Syntax {
    fn default() -> Self {
        Syntax::new()
    }
}

/// The point (row, byte column) of `byte` in `text`.
fn point_at(text: &[u8], byte: usize) -> Point {
    advance(Point { row: 0, column: 0 }, &text[..byte])
}

/// The point reached from `from` by reading `bytes`.
fn advance(from: Point, bytes: &[u8]) -> Point {
    let rows = bytes.iter().filter(|&&b| b == b'\n').count();
    if rows == 0 {
        return Point {
            row: from.row,
            column: from.column + bytes.len(),
        };
    }
    let last_line = bytes
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |at| at + 1);
    Point {
        row: from.row + rows,
        column: bytes.len() - last_line,
    }
}

/// The edit that turns `old` (a statement's text) at `[start, end)` into `new`, as the tree
/// wants it: widened by one byte each way for the `:name` view ([`view::widen`]), and with
/// points, which tree-sitter uses to keep reused subtrees' extents right.
pub fn input_edit(old: &str, start: usize, end: usize, new: &str) -> InputEdit {
    let (from, to) = view::widen(old, start, end);
    let bytes = old.as_bytes();
    let start_position = point_at(bytes, from);
    let old_end_position = advance(start_position, &bytes[from..to]);
    let mut new_end_position = advance(start_position, &bytes[from..start]);
    new_end_position = advance(new_end_position, new.as_bytes());
    new_end_position = advance(new_end_position, &bytes[end..to]);
    InputEdit {
        start_byte: from,
        old_end_byte: to,
        new_end_byte: from + (start - from) + new.len() + (to - end),
        start_position,
        old_end_position,
        new_end_position,
    }
}

/// Drop least recently used trees until the source bytes they cover fit in `budget`.
/// `total` is the caller's running sum of `tree_cost`.
pub fn evict(stmts: &mut [Stmt], total: &mut usize, budget: usize) {
    if *total <= budget {
        return;
    }
    let mut order: Vec<usize> = (0..stmts.len())
        .filter(|&i| stmts[i].tree.is_some())
        .collect();
    order.sort_unstable_by_key(|&i| stmts[i].last_use);
    for i in order {
        if *total <= budget {
            break;
        }
        let stmt = &mut stmts[i];
        *total -= stmt.tree_cost;
        stmt.tree_cost = 0;
        stmt.tree = None;
        stmt.tree_stale = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_count_rows_and_byte_columns() {
        assert_eq!(point_at(b"ab\ncd", 4), Point { row: 1, column: 1 });
        assert_eq!(point_at(b"ab\ncd", 2), Point { row: 0, column: 2 });
        assert_eq!(point_at("é\nx".as_bytes(), 2), Point { row: 0, column: 2 });
        assert_eq!(
            advance(Point { row: 3, column: 5 }, b"x\n\ny"),
            Point { row: 5, column: 1 }
        );
        assert_eq!(
            advance(Point { row: 3, column: 5 }, b"xy"),
            Point { row: 3, column: 7 }
        );
    }

    #[test]
    fn an_input_edit_is_widened_and_consistent_with_the_new_text() {
        let old = "select a,\n  b from t;";
        let new_text = "select a,\n  bc from t;";
        let edit = input_edit(old, 13, 13, "c");
        assert_eq!(
            (edit.start_byte, edit.old_end_byte, edit.new_end_byte),
            (12, 14, 15)
        );
        assert_eq!(edit.start_position, Point { row: 1, column: 2 });
        assert_eq!(edit.new_end_position, point_at(new_text.as_bytes(), 15));
        assert_eq!(edit.old_end_position, point_at(old.as_bytes(), 14));
    }
}
