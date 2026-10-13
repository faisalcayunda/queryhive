//! A `;` that ends a statement is a terminator, not a mistake: the editor must neither split off
//! an empty statement nor mark an issue for it, whatever follows it.

mod common;

use common::DIALECTS;
use qh_editor::{statement_ranges, Analyzer, IssueKind};

const SHAPES: [&str; 8] = [
    "SELECT 1;",
    "SELECT 1;  ",
    "SELECT 1;\n",
    "SELECT 1;\n\n-- note\n",
    "SELECT 1; -- note",
    "SELECT 1; /* note */",
    "SELECT 1;;",
    "SELECT a, b FROM t WHERE c = 'x;y' ORDER BY a;",
];

#[test]
fn a_trailing_terminator_is_one_statement_and_no_issue_in_every_dialect() {
    let mut problems = Vec::new();
    for dialect in DIALECTS {
        for sql in SHAPES {
            let ranges = statement_ranges(sql, dialect).expect("ranges");
            if ranges.len() != 2 {
                problems.push(format!("{dialect:?} {sql:?}: ranges {ranges:?}"));
            }
            let mut analyzer = Analyzer::new(sql, dialect).expect("analyzer");
            // The tree-sitter ERROR on `;;` is not drawn (P-28, `EditorDiagnostics.swift` drops
            // `syntaxError`), so only the issues the app does show are held to this.
            let shown: Vec<_> = analyzer
                .outline()
                .issues
                .into_iter()
                .filter(|issue| {
                    !matches!(issue.kind, IssueKind::SyntaxError | IssueKind::MissingToken)
                })
                .collect();
            if !shown.is_empty() {
                problems.push(format!("{dialect:?} {sql:?}: {shown:?}"));
            }
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
