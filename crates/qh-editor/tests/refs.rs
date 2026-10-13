//! Table and alias references: what the tree gives, what the lexical fallback gives, and that
//! neither ever answers outside the statement.

mod common;

use common::Rng;
use qh_editor::refs::{
    scope_at_end, table_alias, Clause, RefSource, References, RelationKind, Scope,
};
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
        // What the fallback learned for TD-4: function calls, subqueries and CTEs.
        "select extract(year from d) from t u",
        "select coalesce((select 1 from m), 0) from t",
        "select * from generate_series(1, 2) g, t",
        "select * from (select 1 from a) q join b on true",
        "select * from ((select 1 from a) union (select 2 from b)) q, c",
        "select * from a join lateral (select 1 from b where b.x = a.x) l on true",
        "select * from a where x in (select y from b) and exists (select 1 from c z)",
        "with c(x, y) as (select 1, 2), d as materialized (select 3 from e) select * from c, d, t r",
        "with recursive n as (select 1 union all select i + 1 from n) select * from n",
        "insert into t (a) select b from u",
    ] {
        let tree = refs(sql, Dialect::Postgres, None);
        assert_eq!(tree.source, RefSource::Tree, "{sql}");
        let lexical = qh_editor::refs::lexical_for_test(sql, Dialect::Postgres);
        assert_eq!(shown(&tree), lexical, "{sql}");
    }
}

#[test]
fn a_from_inside_a_function_call_is_not_a_table() {
    // `where` with nothing after it is an error node, so these take the lexical path.
    let r = refs(
        "select extract(year from d), substring(s from 2 for 3), trim(both ' ' from x) from t u where",
        Dialect::Postgres,
        None,
    );
    assert_eq!(r.source, RefSource::Lexical);
    assert_eq!(shown(&r), ["Table -.-.t as u"]);
    // `distinct from` compares; it names nothing.
    let r = refs(
        "select * from t where a is distinct from b and",
        Dialect::Postgres,
        None,
    );
    assert_eq!(r.source, RefSource::Lexical);
    assert_eq!(shown(&r), ["Table -.-.t as -"]);
    // The grammar itself cannot parse these two forms, so the fallback is what answers.
    for sql in [
        "select substring(s from 2 for 3) from t u",
        "select trim(both ' ' from x) from t u",
    ] {
        let r = refs(sql, Dialect::Postgres, None);
        assert_eq!(r.source, RefSource::Lexical, "{sql}");
        assert_eq!(shown(&r), ["Table -.-.t as u"], "{sql}");
    }
    // A subquery inside a call is a query again, so its FROM counts.
    let r = refs(
        "select f(a, (select max(d) from m)) from t where",
        Dialect::Postgres,
        None,
    );
    assert_eq!(shown(&r), ["Table -.-.m as -", "Table -.-.t as -"]);
}

#[test]
fn the_fallback_reads_subqueries_and_ctes() {
    let r = refs(
        "with c(x, y) as (select 1, 2), d as (select z from w) select * from c, d, real_t r, (select 1) s where",
        Dialect::Trino,
        None,
    );
    assert_eq!(r.source, RefSource::Lexical);
    assert_eq!(r.ctes.len(), 2);
    assert_eq!(
        (r.ctes[0].name.as_str(), r.ctes[0].columns.clone()),
        ("c", vec!["x".into(), "y".into()])
    );
    assert_eq!(r.ctes[1].name, "d");
    assert!(r.ctes[1].columns.is_empty());
    assert_eq!(
        shown(&r),
        [
            "Table -.-.w as -",
            "Cte -.-.c as -",
            "Cte -.-.d as -",
            "Table -.-.real_t as r",
            "Subquery -.-.- as s",
        ]
    );
    // The subquery's span is the parenthesised text.
    let sql = "select * from (select 1 from a) q where";
    let q = &refs(sql, Dialect::Postgres, None).relations[0];
    assert_eq!(
        (q.kind, q.name_start_utf16, q.name_len_utf16),
        (RelationKind::Subquery, 14, 17)
    );
}

#[test]
fn the_fallback_skips_relations_that_are_not_tables() {
    for (sql, want) in [
        (
            "select * from unnest(a) with ordinality as o(n, i) join t on true",
            vec!["Table -.-.t as -"],
        ),
        (
            "select * from generate_series(1, 2) as g(n), t x where",
            vec!["Table -.-.t as x"],
        ),
        // `UPDATE` as part of a clause names no table.
        (
            "select * from t for update of t where",
            vec!["Table -.-.t as -"],
        ),
        (
            "insert into t (a) values (1) on duplicate key update a = 1 where",
            vec!["Table -.-.t as -"],
        ),
        // A parenthesised join group is read as the list it holds.
        (
            "select * from (a join b on a.x = b.x) join c on true",
            vec!["Table -.-.a as -", "Table -.-.b as -", "Table -.-.c as -"],
        ),
        (
            "select * from a, generate_series(1, 2) g, lateral (select 1 from b) l where",
            vec![
                "Table -.-.a as -",
                "Subquery -.-.- as l",
                "Table -.-.b as -",
            ],
        ),
    ] {
        let r = refs(sql, Dialect::Postgres, None);
        assert_eq!(r.source, RefSource::Lexical, "{sql}");
        assert_eq!(shown(&r), want, "{sql}");
    }
}

#[test]
fn a_referential_action_names_no_table() {
    for sql in [
        "create table t (a int references u (id) on update cascade, b int)",
        "create table t (a int references u (id) on update set null, b int)",
        "create table t (a int references u (id) on delete set null on update cascade)",
    ] {
        assert_eq!(
            qh_editor::refs::lexical_for_test(sql, Dialect::Postgres),
            ["Table -.-.t as -"],
            "{sql}"
        );
    }
}

#[test]
fn a_referential_action_does_not_change_the_clause() {
    use Clause::*;
    for (sql, dialect) in [
        (
            "create table t (a int references u (id) on update cascade, |",
            Dialect::Postgres,
        ),
        (
            "create table t (a int references u (id) on update set null, |",
            Dialect::Postgres,
        ),
        (
            "create table t (a timestamp on update current_timestamp, |",
            Dialect::Mysql,
        ),
    ] {
        assert_eq!(clause(sql, dialect), Other, "{sql}");
    }
}

#[test]
fn if_exists_is_not_a_table_name() {
    for (sql, want) in [
        ("create table if not exists t (a int)", "Table -.-.t as -"),
        ("create table if not exists s.t (a int)", "Table -.s.t as -"),
        ("drop table if exists t", "Table -.-.t as -"),
        ("drop table IF EXISTS s.t", "Table -.s.t as -"),
        (
            "alter table if exists t add column a int",
            "Table -.-.t as -",
        ),
    ] {
        assert_eq!(
            qh_editor::refs::lexical_for_test(sql, Dialect::Postgres),
            [want],
            "{sql}"
        );
    }
}

#[test]
fn a_comma_after_a_join_condition_continues_the_from_list() {
    let r = refs(
        "select * from a join b on a.id = b.id, users u join us where",
        Dialect::Postgres,
        None,
    );
    assert_eq!(r.source, RefSource::Lexical);
    assert_eq!(
        shown(&r),
        [
            "Table -.-.a as -",
            "Table -.-.b as -",
            "Table -.-.users as u",
            "Table -.-.us as -",
        ]
    );
    // The scope stays on the safe side: still a join condition after the comma.
    assert_eq!(
        clause(
            "select * from a join b on a.id = b.id, |",
            Dialect::Postgres
        ),
        Clause::JoinCondition
    );
}

#[test]
fn a_comma_outside_the_from_list_names_no_table() {
    for (sql, dialect, want) in [
        (
            "select * from t group by c, d having",
            Dialect::Postgres,
            vec!["Table -.-.t as -"],
        ),
        (
            "select * from t where a = 1 order by c, d limit",
            Dialect::Postgres,
            vec!["Table -.-.t as -"],
        ),
        (
            "select * from t limit 1, 2 where",
            Dialect::Mysql,
            vec!["Table -.-.t as -"],
        ),
        (
            "delete from t returning a, b where",
            Dialect::Postgres,
            vec!["Table -.-.t as -"],
        ),
        (
            "insert into t select * from u on conflict (id) do update set a = 1, b = 2 where",
            Dialect::Postgres,
            vec!["Table -.-.t as -", "Table -.-.u as -"],
        ),
        (
            "insert into t select * from u on duplicate key update a = 1, b = 2 where",
            Dialect::Mysql,
            vec!["Table -.-.t as -", "Table -.-.u as -"],
        ),
        (
            "select a from t union select b, c from u where",
            Dialect::Postgres,
            vec!["Table -.-.t as -", "Table -.-.u as -"],
        ),
        (
            "select * from t; select a, b",
            Dialect::Postgres,
            vec!["Table -.-.t as -"],
        ),
    ] {
        assert_eq!(
            qh_editor::refs::lexical_for_test(sql, dialect),
            want,
            "{sql}"
        );
    }
}

#[test]
fn a_with_that_heads_no_cte_is_left_alone() {
    for sql in [
        "select * from unnest(a) with ordinality as o(n, i), t where",
        "create table t (a int) with (fillfactor = 70);\nselect * from u where",
    ] {
        let r = refs(sql, Dialect::Postgres, None);
        assert!(r.ctes.is_empty(), "{sql}");
    }
}

#[test]
fn the_fallback_survives_deep_nesting() {
    let sql = format!(
        "select * from {}select 1 from t{} where",
        "(".repeat(50_000),
        ")".repeat(50_000)
    );
    let lexical = qh_editor::refs::lexical_for_test(&sql, Dialect::Postgres);
    assert!(lexical.len() <= 1, "{lexical:?}");
    // Unbalanced, too.
    let sql = format!("select * from {}select 1 from t where", "(".repeat(50_000));
    let _ = qh_editor::refs::lexical_for_test(&sql, Dialect::Postgres);
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

/// The clause at the `|` in `marked`: the text before the bar is all the reader is given.
fn clause(marked: &str, dialect: Dialect) -> Clause {
    scope(marked, dialect).clause
}

fn scope(marked: &str, dialect: Dialect) -> Scope {
    let at = marked.find('|').expect("a caret marker");
    scope_at_end(&marked[..at], dialect)
}

#[test]
fn the_clause_at_the_caret_in_a_query() {
    use Clause::*;
    for (sql, want) in [
        ("select |", SelectList),
        ("select a, count(|", SelectList),
        ("select * from |", QueryTable),
        ("select * from t a, |", QueryTable),
        ("select * from t a join |", QueryTable),
        ("select * from t a left outer join |", QueryTable),
        ("select * from t a join u b on |", JoinCondition),
        (
            "select * from t a join u b on a.x = b.x and |",
            JoinCondition,
        ),
        ("select * from t a join u b using (|", JoinCondition),
        ("select * from t a join u b on a.x = b.x join |", QueryTable),
        ("select * from (a join |", QueryTable),
        ("select * from t where |", Where),
        ("select * from t where a in (1, |", Where),
        ("select * from t where a is distinct from |", Where),
        ("select * from t group by |", GroupBy),
        ("select * from t group by a having |", Having),
        ("select * from t order by |", OrderBy),
        ("select * from t limit |", Other),
        ("select * from t for update |", Other),
        ("select extract(year from |", SelectList),
        ("select * from generate_series(|", Other),
        ("select * from t where a in (select x from |", QueryTable),
        ("select * from t where a in (select x from u) and |", Where),
        ("select a from t union select |", SelectList),
        ("with x as (select 1) select * from |", QueryTable),
        ("with x as (select * from |", QueryTable),
        ("select 1;\nselect * from |", QueryTable),
        ("select 1;\n|", Other),
        ("|", Other),
        ("  \n |select 1", Other),
    ] {
        assert_eq!(clause(sql, Dialect::Postgres), want, "{sql}");
    }
}

#[test]
fn the_clause_at_the_caret_in_a_write() {
    use Clause::*;
    for (sql, want) in [
        ("update |", WriteTable),
        ("update only |", WriteTable),
        ("update t set |", SetList),
        ("update t u set a = 1, |", SetList),
        ("update t set a = 1 from |", QueryTable),
        ("update t set a = 1 where |", Where),
        ("delete from |", WriteTable),
        ("delete from t using |", QueryTable),
        ("delete from t where |", Where),
        ("delete from t where a in (select x from |", QueryTable),
        ("insert into |", WriteTable),
        ("insert into t (|", Other),
        ("insert into t (a, b) values (|", Other),
        ("insert into t select * from |", QueryTable),
        ("insert into t (a) select * from u on conflict (|", Other),
        ("truncate |", WriteTable),
        ("truncate table |", WriteTable),
        ("create table |", WriteTable),
        ("create table if not exists |", WriteTable),
        ("drop table if exists |", WriteTable),
        ("alter table |", WriteTable),
        ("create table t (a int references |", Other),
    ] {
        assert_eq!(clause(sql, Dialect::Postgres), want, "{sql}");
    }
    // MySQL's `ON DUPLICATE KEY UPDATE` is not an UPDATE statement.
    assert_eq!(
        clause(
            "insert into t (a) values (1) on duplicate key update |",
            Dialect::Mysql
        ),
        Clause::Other
    );
}

#[test]
fn the_select_aliases_the_caret_can_see() {
    let aliases = |sql: &str| scope(sql, Dialect::Postgres).select_aliases;
    // `AS` names an output; a cast, an implicit alias and a table alias do not.
    assert_eq!(
        aliases("select a as x, count(*) as \"N\", cast(b as int), c d, e::int as z from t as u order by |"),
        ["x", "\"N\"", "z"]
    );
    // A subquery's outputs are its own.
    assert_eq!(
        aliases(
            "select a as x from (select 1 as y) q where z in (select 2 as w from u) group by |"
        ),
        ["x"]
    );
    assert_eq!(
        aliases("select a as x from (select 1 as y from t order by |"),
        ["y"]
    );
    // `ORDER BY` after a `UNION` answers to the first select.
    assert_eq!(
        aliases("select a as x from t union select b as y from u order by |"),
        ["x"]
    );
    // Another statement starts clean.
    assert!(aliases("select 1 as x; select * from t order by |").is_empty());
    // MySQL quotes with backticks.
    assert_eq!(
        scope("select a as `my col` from t order by |", Dialect::Mysql).select_aliases,
        ["`my col`"]
    );
    assert!(aliases("select * from t where a as |").is_empty());
}

#[test]
fn the_clause_survives_any_text() {
    let pieces = [
        "select ",
        "from ",
        "join ",
        "on ",
        "(",
        ")",
        ")), ",
        "((",
        ";",
        "'",
        "\"",
        "`",
        "--",
        "/*",
        "$$",
        "update ",
        "set ",
        "delete ",
        "with c as (",
        "t",
        "😀",
        "\n",
    ];
    for dialect in [
        Dialect::Generic,
        Dialect::Postgres,
        Dialect::Mysql,
        Dialect::Trino,
    ] {
        let mut rng = Rng(11);
        let mut text = String::new();
        for _ in 0..400 {
            text.push_str(rng.pick(&pieces));
            let _ = scope_at_end(&text, dialect);
        }
    }
    // Brackets only, a long way down, and an unclosed run of them.
    for n in [100_000, 3] {
        let _ = scope_at_end(&format!("select {}", "(".repeat(n)), Dialect::Postgres);
        let _ = scope_at_end(&format!("select {}", ")".repeat(n)), Dialect::Postgres);
    }
    // A statement past the giant size is not read.
    let giant = format!("select * from t where a in ({}", "1,".repeat(200_000));
    assert_eq!(scope_at_end(&giant, Dialect::Postgres), Scope::default());
}

#[test]
fn an_alias_is_short_free_and_never_a_keyword() {
    let none: [&str; 0] = [];
    for (table, taken, want) in [
        ("users", vec![], "u"),
        ("order_items", vec![], "oi"),
        ("OrderItems", vec![], "oi"),
        ("user_roles", vec!["ur"], "u"),
        ("users", vec!["u"], "us"),
        // `or` is a keyword, so the next try is `ord`.
        ("orders", vec!["o"], "ord"),
        ("t", vec![], "t2"),
        ("2fa_codes", vec![], "fc"),
        ("users", vec!["u", "us", "use"], "u2"),
        ("users", vec!["U"], "us"),
        ("__", vec![], "t"),
        // Reserved in practice though the grammar has no keyword for them.
        ("x_o_r", vec![], "x"),
        ("q_u_a_l_i_f_y", vec![], "q"),
        ("s_a_m_p_l_e", vec![], "s"),
        ("models", vec!["m", "mo"], "m2"),
        ("divisions", vec!["d", "di"], "d2"),
        ("topics", vec!["t", "to"], "t2"),
    ] {
        assert_eq!(table_alias(table, &taken), want, "{table} {taken:?}");
    }
    assert_eq!(table_alias("users", &none), "u");
}

#[test]
fn a_generated_alias_reads_back_as_an_alias() {
    let names = [
        "users",
        "order",
        "group",
        "select",
        "from",
        "as",
        "in",
        "on",
        "by",
        "is",
        "or",
        "if",
        "do",
        "table",
        "user",
        "ab",
        "a",
        "x_y_z",
        "a_b",
        "ÉTÉ",
        "日本",
        "t1",
        "1t",
        "_",
        "",
        "order_by",
        "in_on",
        "limit_offset",
        "left_join",
    ];
    let taken_sets: [Vec<&str>; 3] = [vec![], vec!["o", "or", "ord"], vec!["u", "us", "use", "u2"]];
    for name in names {
        for taken in &taken_sets {
            let alias = table_alias(name, taken);
            assert!(!alias.is_empty(), "{name}");
            assert!(
                alias
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    && !alias.as_bytes()[0].is_ascii_digit(),
                "{name} -> {alias}"
            );
            assert!(
                !qh_editor::keywords::is_keyword(alias.as_bytes()),
                "{name} -> {alias}"
            );
            assert!(
                !taken.iter().any(|t| t.eq_ignore_ascii_case(&alias)),
                "{name} -> {alias} is taken"
            );
            // The reader takes it for the alias of the table it was made for.
            let sql = format!("select * from {name} {alias} where");
            if name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                && !name.is_empty()
                && !qh_editor::keywords::is_keyword(name.as_bytes())
                && !name.as_bytes()[0].is_ascii_digit()
            {
                let r = refs(&sql, Dialect::Postgres, None);
                assert_eq!(
                    r.relations[0].alias.as_deref(),
                    Some(alias.as_str()),
                    "{sql}"
                );
            }
        }
    }
}
