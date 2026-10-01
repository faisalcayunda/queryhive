//! Things that are wrong with a statement, as data for the diagnostics of W10-T6.
//!
//! Two kinds. The lexical ones come from the scanner's own reading of the text (an opener that
//! is never closed, a parenthesis with no partner) and are true whatever the grammar knows.
//! The syntax ones come from ERROR and MISSING nodes in the tree; the grammar flags 14 to 47%
//! of valid MySQL, Trino and PostgreSQL statements, so they are reported here but the app does
//! not draw them by default.

use qh_sql::{walk, EndState, Lexer, OpaqueKind, Visitor};
use tree_sitter::Tree;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueKind {
    UnclosedQuote,
    UnclosedIdentifier,
    UnclosedComment,
    UnclosedDollar,
    UnbalancedParen,
    SyntaxError,
    MissingToken,
}

/// An issue at `[start, start + len)` in UTF-16 units of the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Issue {
    pub kind: IssueKind,
    pub start: u32,
    pub len: u32,
}

/// An issue with byte offsets relative to its statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelIssue {
    pub kind: IssueKind,
    pub start: u32,
    pub len: u32,
}

struct Regions(Vec<(OpaqueKind, usize, usize)>);

impl Visitor for Regions {
    fn opaque(&mut self, kind: OpaqueKind, start: usize, end: usize) {
        self.0.push((kind, start, end));
    }
}

/// The lexical issues of one statement: the opener of a region left open at its end (the
/// delimiter only, not the text under it), and every unmatched parenthesis in code.
pub fn lexical(text: &str, lexer: Lexer) -> Vec<RelIssue> {
    let bytes = text.as_bytes();
    let mut regions = Regions(Vec::new());
    let end = walk(bytes, lexer, &mut regions);
    let regions = regions.0;
    let mut out = Vec::new();

    if let (EndState::Open(kind), Some(&(_, start, stop))) = (end, regions.last()) {
        let region = &bytes[start..stop];
        let found = match kind {
            OpaqueKind::SingleQuote => Some((
                IssueKind::UnclosedQuote,
                region
                    .iter()
                    .position(|&b| b == b'\'')
                    .map_or(1, |at| at + 1),
            )),
            OpaqueKind::DoubleQuote | OpaqueKind::Backtick => {
                Some((IssueKind::UnclosedIdentifier, 1))
            }
            OpaqueKind::BlockComment => Some((IssueKind::UnclosedComment, 2)),
            OpaqueKind::DollarQuote => Some((
                IssueKind::UnclosedDollar,
                region
                    .get(1..)
                    .unwrap_or(&[])
                    .iter()
                    .position(|&b| b == b'$')
                    .map_or(1, |at| at + 2),
            )),
            // A line comment that runs to the end of the text is fine.
            OpaqueKind::LineComment => None,
        };
        if let Some((kind, len)) = found {
            out.push(RelIssue {
                kind,
                start: start as u32,
                len: len as u32,
            });
        }
    }

    let mut open: Vec<usize> = Vec::new();
    let mut unmatched = Vec::new();
    let (mut at, mut region) = (0, 0);
    while at < bytes.len() {
        while region < regions.len() && regions[region].2 <= at {
            region += 1;
        }
        if region < regions.len() && regions[region].1 <= at {
            at = regions[region].2;
            continue;
        }
        match bytes[at] {
            b'(' => open.push(at),
            b')' if open.pop().is_none() => unmatched.push(at),
            _ => {}
        }
        at += 1;
    }
    unmatched.extend(open);
    unmatched.sort_unstable();
    out.extend(unmatched.into_iter().map(|at| RelIssue {
        kind: IssueKind::UnbalancedParen,
        start: at as u32,
        len: 1,
    }));
    out.sort_by_key(|issue| issue.start);
    out
}

/// The ERROR and MISSING nodes of a tree. An ERROR is reported whole and not searched.
pub fn syntax(tree: &Tree) -> Vec<RelIssue> {
    let mut out = Vec::new();
    if !tree.root_node().has_error() {
        return out;
    }
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        let mut descend = node.has_error();
        if node.is_error() {
            out.push(RelIssue {
                kind: IssueKind::SyntaxError,
                start: node.start_byte() as u32,
                len: (node.end_byte() - node.start_byte()) as u32,
            });
            descend = false;
        } else if node.is_missing() {
            out.push(RelIssue {
                kind: IssueKind::MissingToken,
                start: node.start_byte() as u32,
                len: 0,
            });
            descend = false;
        }
        if descend && cursor.goto_first_child() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use qh_sql::Dialect;

    fn lex(text: &str) -> Vec<(IssueKind, usize, usize)> {
        lex_as(text, Dialect::Generic)
    }

    fn lex_as(text: &str, dialect: Dialect) -> Vec<(IssueKind, usize, usize)> {
        lexical(text, Lexer::from(dialect))
            .into_iter()
            .map(|i| (i.kind, i.start as usize, i.len as usize))
            .collect()
    }

    #[test]
    fn an_opener_left_open_is_reported_at_its_delimiter() {
        assert_eq!(lex("select 'abc"), vec![(IssueKind::UnclosedQuote, 7, 1)]);
        assert_eq!(
            lex_as("select E'abc", Dialect::Postgres),
            vec![(IssueKind::UnclosedQuote, 7, 2)]
        );
        assert_eq!(
            lex("select \"a"),
            vec![(IssueKind::UnclosedIdentifier, 7, 1)]
        );
        assert_eq!(
            lex("select 1 /* x"),
            vec![(IssueKind::UnclosedComment, 9, 2)]
        );
        assert_eq!(
            lex("select $tag$ x"),
            vec![(IssueKind::UnclosedDollar, 7, 5)]
        );
        assert_eq!(lex("select 1 -- x"), vec![]);
        assert_eq!(lex("select 1"), vec![]);
    }

    #[test]
    fn parentheses_are_counted_in_code_only() {
        assert_eq!(lex("select (1"), vec![(IssueKind::UnbalancedParen, 7, 1)]);
        assert_eq!(lex("select 1)"), vec![(IssueKind::UnbalancedParen, 8, 1)]);
        assert_eq!(
            lex("select ((1) ')' -- (\n"),
            vec![(IssueKind::UnbalancedParen, 7, 1)]
        );
        assert_eq!(lex("select f(1, (2))"), vec![]);
    }
}
