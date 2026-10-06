//! The SQL surface is read-only and names only what the app registered (section 14.7).

use crate::common::{big_table, collect, engine, texts};
use qh_analytics_proto::ErrorKind;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn everything_that_changes_something_is_refused() {
    let (engine, dir) = engine(2);
    let session = engine.session();
    session.register("t", big_table(10, "x"), Some(10)).unwrap();
    let _lease = engine.grant(64 << 20);
    let out_csv = dir.path().join("copied.csv");
    let out_sql = out_csv.display();

    for sql in [
        "CREATE EXTERNAL TABLE e STORED AS CSV LOCATION '/etc/hosts'".to_owned(),
        format!("COPY (SELECT 1) TO '{out_sql}'"),
        format!("COPY t TO '{out_sql}' STORED AS CSV"),
        "INSERT INTO t VALUES (1, 'x')".to_owned(),
        "SET datafusion.execution.batch_size = 1".to_owned(),
        "CREATE TABLE made AS SELECT * FROM t".to_owned(),
        "CREATE VIEW v AS SELECT * FROM t".to_owned(),
        "CREATE SCHEMA other".to_owned(),
        "DROP TABLE t".to_owned(),
        "DELETE FROM t".to_owned(),
        "UPDATE t SET k = 1".to_owned(),
        // Not a path SQL may name: only what the app registered.
        "SELECT * FROM '/etc/hosts'".to_owned(),
        "SELECT * FROM 'file:///etc/hosts'".to_owned(),
        // The catalog is not browsable.
        "SELECT * FROM information_schema.tables".to_owned(),
        "SHOW TABLES".to_owned(),
    ] {
        let result = collect(&session, &sql).await;
        let error = result.expect_err(&format!("this must be refused: {sql}"));
        assert_eq!(error.0.kind, ErrorKind::InvalidArgument, "{sql}: {error}");
    }
    assert!(!out_csv.exists(), "COPY wrote a file");
    // The table survived all of that.
    let rows = texts(&collect(&session, "SELECT count(*) FROM t").await.unwrap());
    assert_eq!(rows, vec![vec![Some("10".to_owned())]]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reading_is_allowed() {
    let (engine, _dir) = engine(2);
    let session = engine.session();
    session.register("t", big_table(5, "x"), Some(5)).unwrap();
    let _lease = engine.grant(64 << 20);
    for sql in [
        "SELECT k FROM t ORDER BY k LIMIT 2",
        "WITH w AS (SELECT k FROM t) SELECT sum(k) FROM w",
        "SELECT a.k, b.k FROM t a JOIN t b ON a.k = b.k ORDER BY 1",
        "EXPLAIN SELECT k FROM t",
        "SELECT qh_natural('item10') < qh_natural('item9')",
        "SELECT * FROM generate_series(1, 3)",
    ] {
        collect(&session, sql)
            .await
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_syntax_error_says_where() {
    let (engine, _dir) = engine(2);
    let session = engine.session();
    let error = collect(&session, "SELECT 1 FROM t\nWHERE )")
        .await
        .unwrap_err();
    assert_eq!(error.0.kind, ErrorKind::InvalidArgument);
    let position = error
        .0
        .position
        .unwrap_or_else(|| panic!("a parse error carries its position: {:?}", error.0));
    assert_eq!(position.line, 2, "{}", error.0.message);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_table_is_the_users_mistake() {
    let (engine, _dir) = engine(2);
    let session = engine.session();
    let error = collect(&session, "SELECT * FROM nosuch").await.unwrap_err();
    assert_eq!(error.0.kind, ErrorKind::InvalidArgument);
    assert!(error.0.message.contains("nosuch"), "{}", error.0.message);
}
