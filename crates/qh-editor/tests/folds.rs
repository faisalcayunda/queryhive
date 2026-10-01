//! Folds and issues, and the statements too big to parse.

mod common;

use common::validate;
use qh_editor::{Class, Dialect, Document, FoldKind, IssueKind};

fn outline(text: &str, dialect: Dialect) -> qh_editor::Outline {
    Document::new(text, dialect).unwrap().outline(1).unwrap()
}

fn kinds(text: &str) -> Vec<(FoldKind, u32, u32)> {
    outline(text, Dialect::Postgres)
        .folds
        .iter()
        .map(|f| (f.kind, f.header_line, f.last_line))
        .collect()
}

#[test]
fn a_statement_fold_hides_from_the_line_after_its_first() {
    let out = outline(
        "select 1;\nselect a,\n       b\n  from t\n;\nselect 3",
        Dialect::Generic,
    );
    assert_eq!(out.folds.len(), 1);
    let fold = &out.folds[0];
    assert_eq!(
        (fold.kind, fold.header_line, fold.last_line),
        (FoldKind::Statement, 1, 3)
    );
    assert_eq!(fold.summary, "SELECT");
    assert_eq!((fold.header, fold.body_start, fold.body_end), (10, 20, 38));
}

#[test]
fn ctes_subqueries_and_dollar_bodies_fold_when_they_start_below_their_statement() {
    let text = "select 1\n;\nwith a as (\n  select 1\n), b as (\n  select 2\n)\nselect * from (\n  select 3\n) s;\n\ncreate function f() returns int language sql as\n$body$\nselect 1;\nselect 2;\n$body$;\n";
    // The `WITH a AS (` CTE starts on its statement's header line, so the statement's fold
    // stands for it; `b`'s body, the subquery and the function body are folds of their own.
    assert_eq!(
        kinds(text),
        [
            (FoldKind::Statement, 2, 9),
            (FoldKind::Cte, 4, 6),
            (FoldKind::Subquery, 7, 9),
            (FoldKind::Statement, 11, 15),
            (FoldKind::Body, 12, 14),
        ]
    );
}

#[test]
fn a_fold_whose_header_line_another_fold_has_is_dropped() {
    // `with x as (` opens on the statement's first line: the statement's fold hides all of it.
    let out = outline(
        "with x as (\n  select 1\n) select * from x",
        Dialect::Generic,
    );
    assert_eq!(
        out.folds.iter().map(|f| f.kind).collect::<Vec<_>>(),
        [FoldKind::Statement]
    );
}

#[test]
fn issues_report_the_opener_left_open_and_the_parenthesis_left_alone() {
    let out = outline("select (1;\nselect 'a\n", Dialect::Generic);
    let found: Vec<_> = out
        .issues
        .iter()
        .filter(|i| !matches!(i.kind, IssueKind::SyntaxError | IssueKind::MissingToken))
        .map(|i| (i.kind, i.start, i.len))
        .collect();
    assert_eq!(
        found,
        [
            (IssueKind::UnbalancedParen, 7, 1),
            (IssueKind::UnclosedQuote, 18, 1)
        ]
    );
}

#[test]
fn the_grammar_flags_an_error_and_says_where() {
    let out = outline("SELEC 1;\nselect 2;", Dialect::Generic);
    assert_eq!(
        out.issues
            .iter()
            .map(|i| (i.kind, i.start))
            .collect::<Vec<_>>(),
        [(IssueKind::SyntaxError, 0)]
    );
    assert_eq!(out.statements, [0, 7, 8, 17]);
}

/// A statement over the limit has no tree, so no tree folds and no syntax issues, but it is
/// still a statement, is coloured by the lexer, and follows edits.
#[test]
fn a_giant_statement_is_lexed_not_parsed() {
    let mut text = String::from("insert into t values\n");
    while text.len() < qh_editor::GIANT_STATEMENT_BYTES + 5_000 {
        text.push_str("  (1, 'a;b', :x, -- note\n   2),\n");
    }
    text.push_str("  (3, 4);\nselect 1");
    let mut doc = Document::new(&text, Dialect::Generic).unwrap();
    let stats = doc.stats();
    assert_eq!(stats.giant, 1);

    let paint = doc.paint(1, 0, 400, u32::MAX).unwrap();
    validate(&paint);
    assert_eq!(doc.stats().trees, 0);
    let classes: Vec<u32> = paint.runs.chunks(3).map(|r| r[2]).collect();
    // `insert`, `into`, (`t`), `values`.
    assert_eq!(classes[..3], [Class::Keyword as u32; 3]);
    assert!(
        classes.contains(&(Class::Parameter as u32)) && classes.contains(&(Class::Comment as u32))
    );

    let outline = doc.outline(1).unwrap();
    assert_eq!(outline.folds.len(), 1);
    assert_eq!(outline.folds[0].kind, FoldKind::Statement);
    assert_eq!(outline.statements.len(), 4);

    // An edit near the start repaints the rest of the statement inside the window, and only that.
    doc.mark_applied(1, paint.ranges.clone());
    let rev = doc.replace(30, 0, "x").unwrap();
    let paint = doc.paint(rev, 0, 400, u32::MAX).unwrap();
    validate(&paint);
    assert_eq!(paint.ranges.len(), 2);
    assert_eq!(paint.ranges[0] + paint.ranges[1], 400);
    assert!(paint.ranges[0] <= 30);
}
