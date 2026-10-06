//! Which `:name` colours as a parameter. The case table is `fixtures/params/cases.tsv`, shared
//! with `QueryParametersTests` in the app, because the rule lives twice (here and in
//! `SQLScanner.swift`) and the two must not drift. Each row is `dialect<TAB>statement<TAB>names`.

use std::fs;
use std::path::Path;

use qh_editor::classify::lex_statement;
use qh_editor::{Class, Dialect};

fn parameters(sql: &str, dialect: Dialect) -> Vec<String> {
    lex_statement(sql.as_bytes(), dialect)
        .iter()
        .filter(|t| t.class == Class::Parameter)
        .map(|t| sql[t.start as usize + 1..t.end as usize].to_string())
        .collect()
}

#[test]
fn the_shared_case_table() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/params/cases.tsv");
    let table = fs::read_to_string(path).unwrap();
    let mut rows = 0;
    for line in table.lines().filter(|l| !l.is_empty()) {
        let mut cells = line.split('\t');
        let dialect = match cells.next().unwrap() {
            "pg" => Dialect::Postgres,
            "mysql" => Dialect::Mysql,
            "trino" => Dialect::Trino,
            _ => Dialect::Generic,
        };
        let sql = cells.next().unwrap();
        let want: Vec<&str> = cells
            .next()
            .unwrap_or("")
            .split(',')
            .filter(|n| !n.is_empty())
            .collect();
        assert_eq!(parameters(sql, dialect), want, "{line}");
        rows += 1;
    }
    assert!(rows > 20);
}
