//! What the editor can collapse: whole statements, the body of a CTE, a subquery, and the body of
//! a `$tag$ … $tag$` function.
//!
//! A statement fold comes from the scanner's statement range and exists whatever the grammar
//! makes of the text. The other three come from tree nodes, and are kept per statement as byte
//! offsets on the header and last lines, so they stay valid until that statement's text changes.
//!
//! Lines are the unit: the hidden body starts on the line after the header and ends with the
//! last hidden line's newline, so a fold never leaves half a paragraph behind.

use qh_sql::{first_significant, Lexer};
use tree_sitter::Tree;

use crate::text::TextBuffer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldKind {
    Statement,
    Cte,
    Subquery,
    Body,
}

/// One collapsible region. Lines are 0-based; offsets are UTF-16 units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fold {
    pub kind: FoldKind,
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

/// A tree-derived fold: a byte of the header line and a byte of the last line, relative to the
/// statement. Which lines those are depends on the text around, so that is worked out later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelFold {
    pub kind: FoldKind,
    pub head_at: u32,
    pub last_at: u32,
}

/// The CTE bodies, subqueries and dollar-quoted bodies of a tree.
pub fn from_tree(tree: &Tree) -> Vec<RelFold> {
    let mut out = Vec::new();
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        match node.kind() {
            "cte" => {
                let mut walk = node.walk();
                let open = node.children(&mut walk).find(|c| c.kind() == "(");
                let close = node.children(&mut walk).filter(|c| c.kind() == ")").last();
                if let (Some(open), Some(close)) = (open, close) {
                    out.push(RelFold {
                        kind: FoldKind::Cte,
                        head_at: open.start_byte() as u32,
                        last_at: close.start_byte() as u32,
                    });
                }
            }
            "subquery" if node.end_byte() > node.start_byte() => out.push(RelFold {
                kind: FoldKind::Subquery,
                head_at: node.start_byte() as u32,
                last_at: (node.end_byte() - 1) as u32,
            }),
            _ => {}
        }
        // A body sits between two dollar quotes that are children of one node.
        if node.child_count() >= 2 {
            let mut walk = node.walk();
            let quotes: Vec<_> = node
                .children(&mut walk)
                .filter(|c| c.kind() == "dollar_quote")
                .collect();
            if let (Some(open), Some(close)) = (quotes.first(), quotes.last()) {
                if quotes.len() >= 2 && close.start_byte() > open.end_byte() {
                    out.push(RelFold {
                        kind: FoldKind::Body,
                        head_at: (open.end_byte() - 1) as u32,
                        last_at: (close.start_byte() - 1) as u32,
                    });
                }
            }
        }
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return out;
            }
        }
    }
}

/// The name a fold's marker shows.
fn summary(kind: FoldKind) -> &'static str {
    match kind {
        FoldKind::Statement => "statement",
        FoldKind::Cte => "CTE",
        FoldKind::Subquery => "subquery",
        FoldKind::Body => "body",
    }
}

/// A fold from the lines it spans, or `None` when it is one line long.
fn fold(
    buffer: &TextBuffer,
    kind: FoldKind,
    head_byte: usize,
    last_byte: usize,
    summary: String,
) -> Option<Fold> {
    let header_line = buffer.line_of_byte(head_byte);
    let last_line = buffer.line_of_byte(last_byte);
    if last_line <= header_line {
        return None;
    }
    let mut cursor = buffer.cursor();
    Some(Fold {
        kind,
        header_line,
        last_line,
        header: cursor.at(buffer.line_start(header_line)),
        body_start: cursor.at(buffer.line_start(header_line + 1)),
        body_end: cursor.at(buffer.line_start(last_line + 1)),
        summary,
    })
}

/// The fold of a whole statement: its range without the `;`, trimmed of whitespace, header at
/// the first character (a leading comment counts), named by the first word after any comments.
pub fn statement_fold(buffer: &TextBuffer, lexer: Lexer, start: usize, end: usize) -> Option<Fold> {
    let piece = &buffer.as_str()[start..end];
    let trimmed = piece.trim_start();
    let first = start + (piece.len() - trimmed.len());
    let trimmed = trimmed.trim_end();
    if trimmed.is_empty() {
        return None;
    }
    let word = first_significant(piece, lexer).and_then(|at| {
        let rest = &piece[at..];
        let mut chars = rest.char_indices();
        let (_, lead) = chars.next()?;
        if !(lead.is_alphabetic() || lead == '_') {
            return None;
        }
        let end = chars
            .find(|&(_, c)| !(c.is_alphanumeric() || c == '_' || c == '$'))
            .map_or(rest.len(), |(at, _)| at);
        Some(rest[..end].to_uppercase())
    });
    fold(
        buffer,
        FoldKind::Statement,
        first,
        first + trimmed.len() - 1,
        word.unwrap_or_else(|| summary(FoldKind::Statement).to_owned()),
    )
}

/// A tree fold placed in the document.
pub fn placed(buffer: &TextBuffer, stmt_start: usize, rel: &RelFold) -> Option<Fold> {
    fold(
        buffer,
        rel.kind,
        stmt_start + rel.head_at as usize,
        stmt_start + rel.last_at as usize,
        summary(rel.kind).to_owned(),
    )
}

/// Drop a fold whose header another, earlier one already has (a statement fold hides all a CTE
/// on its first line would), and order by header then last line.
pub fn dedupe(mut folds: Vec<Fold>) -> Vec<Fold> {
    let mut headers = std::collections::HashSet::new();
    folds.retain(|fold| headers.insert(fold.header));
    folds.sort_by_key(|fold| (fold.header, fold.last_line));
    folds
}

#[cfg(test)]
mod tests {
    use super::*;
    use qh_sql::Dialect;

    fn statement(text: &str) -> Option<Fold> {
        let buffer = TextBuffer::new(text).unwrap();
        statement_fold(&buffer, Lexer::from(Dialect::Generic), 0, text.len())
    }

    #[test]
    fn a_one_line_statement_has_no_fold_and_a_longer_one_hides_from_its_second_line() {
        assert_eq!(statement("select 1"), None);
        assert_eq!(statement("  \n  "), None);
        let fold = statement("select a\n  from t\n where 1").unwrap();
        assert_eq!((fold.header_line, fold.last_line), (0, 2));
        assert_eq!((fold.header, fold.body_start, fold.body_end), (0, 9, 26));
        assert_eq!(fold.summary, "SELECT");
    }

    #[test]
    fn the_header_is_the_first_character_and_the_name_skips_comments() {
        let fold = statement("\n\n-- note\nwith x as (\nselect 1)\nselect 2").unwrap();
        assert_eq!(fold.header_line, 2);
        assert_eq!(fold.summary, "WITH");
        let fold = statement("'a'\n'b'").unwrap();
        assert_eq!(fold.summary, "statement");
    }
}
