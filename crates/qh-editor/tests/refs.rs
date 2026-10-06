//! Table and alias references: what the tree gives, what the lexical fallback gives, and that
//! neither ever answers outside the statement.

mod common;

use common::Rng;
use qh_editor::refs::{RefSource, References, RelationKind};
use qh_editor::{Dialect, Document};

fn refs(sql: &str, dialect: Dialect, at: Option<u32>) -> References {
    let mut doc = Document::new(sql, dialect).unwrap();
    let at = at.unwrap_or(sql.encode_utf16().count() as u32);
    doc.references(doc.revision(), at).unwrap()
}

/// `(catalog, schema, name, alias)` of every relation, as one string each.
fn shown(r: &References) -> Vec<String> {
    r.relations
        .iter()
        .map(|x| {
            let part = |p: &Option<String>| p.clone().unwrap_or_else(|| "-".into());
            format!(
                "{:?} {}.{}.{} as {}{}",
                x.kind,
                part(&x.catalog),
                part(&x.schema),
                part(&x.name),
                part(&x.alias),
                if x.quoted { " quoted" } else { "" }
            )
        })
        .collect()
}

#[test]
fn aliases_with_and_without_as_and_chained_joins() {
    let r = refs(
        "select a.x from public.t as a join \"Db\".s.\"U\" b on a.id = b.id left join v on true",
        Dialect::Postgres,
        None,
    );
    assert_eq!(r.source, RefSource::Tree);
    assert_eq!(
        shown(&r),
        [
            "Table -.public.t as a",
            "Table Db.s.U as b quoted",
            "Table -.-.v as -"
        ]
    );
}

#[test]
fn mysql_backticks_and_comma_lists() {
    let r = refs("select * from `my db`.`t 1` x, u y", Dialect::Mysql, None);
    assert_eq!(
        shown(&r),
        ["Table -.my db.t 1 as x quoted", "Table -.-.u as y"]
    );
}

#[test]
fn ctes_with_and_without_a_column_list() {
    let r = refs(
        "with c(x, y) as (select 1, 2), d as (select 3) select * from c, d, real_t r",
        Dialect::Trino,
        None,
    );
    assert_eq!(r.ctes.len(), 2);
    assert_eq!(
        (r.ctes[0].name.as_str(), r.ctes[0].columns.clone()),
        ("c", vec!["x".into(), "y".into()])
    );
    assert!(r.ctes[1].columns.is_empty());
    let kinds: Vec<_> = r.relations.iter().map(|x| x.kind).collect();
    assert_eq!(
        kinds,
        [RelationKind::Cte, RelationKind::Cte, RelationKind::Table]
    );
}

#[test]
fn write_statements_name_their_table() {
    for (sql, want) in [
        (
            "update t u set a = 1 from v w where true",
            vec!["Table -.-.t as u", "Table -.-.v as w"],
        ),
        ("insert into s.t (a) values (1)", vec!["Table -.s.t as -"]),
        ("delete from t where 1 = 1", vec!["Table -.-.t as -"]),
        ("truncate table s.t", vec!["Table -.s.t as -"]),
    ] {
        let r = refs(sql, Dialect::Postgres, None);
        assert_eq!(r.source, RefSource::Tree, "{sql}");
        assert_eq!(shown(&r), want, "{sql}");
    }
}

#[test]
fn a_subquery_is_a_subquery_and_a_function_is_nothing() {
    let r = refs(
        "select * from (select 1 as x) q join generate_series(1, 2) g on true join t on true",
        Dialect::Postgres,
        None,
    );
    assert_eq!(shown(&r), ["Subquery -.-.- as q", "Table -.-.t as -"]);
    let q = &r.relations[0];
    assert_eq!((q.name_start_utf16, q.name_len_utf16), (14, 15));
}

#[test]
fn an_alias_declared_twice_is_listed_twice() {
    let r = refs(
        "select * from t a where exists (select 1 from u a)",
        Dialect::Postgres,
        None,
    );
    assert_eq!(shown(&r), ["Table -.-.t as a", "Table -.-.u as a"]);
}

#[test]
fn a_statement_with_an_error_falls_back_to_the_lexer() {
    let r = refs(
        "select * from t a join u b on a.id = b.id where",
        Dialect::Postgres,
        None,
    );
    // `where` with nothing after it is an error node in this grammar.
    assert_eq!(r.source, RefSource::Lexical);
    assert_eq!(shown(&r), ["Table -.-.t as a", "Table -.-.u as b"]);
    let r = refs(
        "select * from s.t as x, \"Q\" y, w;\nselect from",
        Dialect::Mysql,
        Some(10),
    );
    assert_eq!(
        shown(&r),
        [
            "Table -.s.t as x",
            "Table -.-.Q as y quoted",
            "Table -.-.w as -"
        ]
    );
}

#[test]
fn the_fallback_matches_the_tree_on_simple_statements() {
    for sql in [
        "select * from t",
        "select * from s.t a join u as b on a.i = b.i",
        "select * from \"A\".\"b\".c x, d",
        "update s.t u set a = 1",
        "insert into s.t (a) values (1)",
        "delete from t where a = 1",
    ] {
        let tree = refs(sql, Dialect::Postgres, None);
        let lexical = qh_editor::refs::lexical_for_test(sql, Dialect::Postgres);
        assert_eq!(shown(&tree), lexical, "{sql}");
    }
}

#[test]
fn the_statement_is_the_one_under_the_offset() {
    let sql = "select * from a;\n  \nselect * from b x;\n\n";
    let first = refs(sql, Dialect::Generic, Some(3));
    assert_eq!(shown(&first), ["Table -.-.a as -"]);
    assert_eq!(
        (first.statement_start_utf16, first.statement_len_utf16),
        (0, 16)
    );
    // In the blank after the first statement: still that statement.
    assert_eq!(
        shown(&refs(sql, Dialect::Generic, Some(19))),
        ["Table -.-.a as -"]
    );
    assert_eq!(
        shown(&refs(sql, Dialect::Generic, Some(20))),
        ["Table -.-.b as x"]
    );
    // In the blank at the end: the last statement.
    assert_eq!(
        shown(&refs(sql, Dialect::Generic, None)),
        ["Table -.-.b as x"]
    );
    // Nothing before the first statement.
    let none = refs("  \n select 1 from t", Dialect::Generic, Some(1));
    assert!(none.relations.is_empty());
    assert_eq!(none.statement_len_utf16, 0);
}

#[test]
fn offsets_are_utf16_and_a_stale_revision_is_refused() {
    let sql = "select '😀' from é.t x";
    let r = refs(sql, Dialect::Postgres, None);
    let rel = &r.relations[0];
    let units: Vec<u16> = sql.encode_utf16().collect();
    let name = String::from_utf16(
        &units[rel.name_start_utf16 as usize..(rel.name_start_utf16 + rel.name_len_utf16) as usize],
    )
    .unwrap();
    assert_eq!(name, "t");
    let mut doc = Document::new(sql, Dialect::Postgres).unwrap();
    let old = doc.revision();
    doc.replace(0, 0, " ").unwrap();
    assert!(doc.references(old, 0).is_err());
    assert!(doc.references(doc.revision(), 9999).is_err());
}

#[test]
fn a_giant_statement_reads_a_window_around_the_offset() {
    let mut sql = String::from("select * from near_t n where x in (");
    sql.push_str(&"1,".repeat(200_000));
    sql.push_str("1) and y in (select 1 from far_t f)");
    let r = refs(&sql, Dialect::Postgres, Some(5));
    assert_eq!(r.source, RefSource::Lexical);
    assert_eq!(shown(&r), ["Table -.-.near_t as n"]);
    let end = sql.encode_utf16().count() as u32;
    let r = refs(&sql, Dialect::Postgres, Some(end));
    assert_eq!(shown(&r), ["Table -.-.far_t as f"]);
}

#[test]
fn a_caret_far_after_a_giant_statement_does_not_panic() {
    let mut sql = String::from("insert into big_t values (");
    sql.push_str(&"1,".repeat(150_000));
    sql.push_str("1);");
    let stmt_end = sql.encode_utf16().count() as u32;
    // A comment longer than the window, caret inside it.
    let mut c = sql.clone();
    c.push_str("/*");
    c.push_str(&" ".repeat(80 * 1024));
    c.push_str("*/");
    let mut doc = Document::new(&c, Dialect::Postgres).unwrap();
    let r = doc
        .references(doc.revision(), stmt_end + 40 * 1024)
        .unwrap();
    assert!(r
        .relations
        .iter()
        .all(|x| x.name_start_utf16 + x.name_len_utf16 <= stmt_end));
    // Trailing blank, caret at the end of the document.
    let mut b = sql.clone();
    b.push_str(&" ".repeat(80 * 1024));
    let mut doc = Document::new(&b, Dialect::Postgres).unwrap();
    let end = b.encode_utf16().count() as u32;
    let r = doc.references(doc.revision(), end).unwrap();
    assert!(r
        .relations
        .iter()
        .all(|x| x.name_start_utf16 + x.name_len_utf16 <= stmt_end));
}

#[test]
fn at_most_64_relations() {
    let sql = format!(
        "select * from {}",
        (0..100)
            .map(|i| format!("t{i}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    assert_eq!(refs(&sql, Dialect::Generic, None).relations.len(), 64);
}

#[test]
fn random_edits_never_panic_or_leave_the_statement() {
    let pieces = [
        "select ",
        "from ",
        "join ",
        "t",
        "u.v",
        " a",
        " as b",
        ",",
        ";",
        "(",
        ")",
        "'",
        "\"",
        "`",
        "with c as (",
        "update ",
        "insert into ",
        "😀",
        " ",
        "\n",
        "--",
        "/*",
        "$$",
        ":p",
        "é",
    ];
    for dialect in [
        Dialect::Generic,
        Dialect::Postgres,
        Dialect::Mysql,
        Dialect::Trino,
    ] {
        let mut rng = Rng(7);
        let mut doc = Document::new("select * from t a;\nselect 1", dialect).unwrap();
        for _ in 0..400 {
            let len = doc.len_utf16();
            let start = rng.below(len as usize + 1) as u32;
            let cut = if rng.below(3) == 0 {
                0
            } else {
                rng.below(3) as u32
            };
            let cut = cut.min(len - start);
            // An edit may split a pair; those are refused, which is fine.
            let _ = doc.replace(start, cut, rng.pick(&pieces));
            let at = rng.below(doc.len_utf16() as usize + 1) as u32;
            let Ok(r) = doc.references(doc.revision(), at) else {
                continue;
            };
            let (s, e) = (
                r.statement_start_utf16,
                r.statement_start_utf16 + r.statement_len_utf16,
            );
            assert!(e <= doc.len_utf16());
            for rel in &r.relations {
                assert!(
                    rel.name_start_utf16 >= s && rel.name_start_utf16 + rel.name_len_utf16 <= e
                );
            }
        }
    }
}
