//! Integration tests against a real MySQL server.
//!
//! ```bash
//! deploy/dev/up.sh mysql
//! QH_TEST_MYSQL=1 cargo test -p qh-driver-mysql --test integration
//! ```
//!
//! Without `QH_TEST_MYSQL=1` each test prints why it is skipping and returns. A
//! silent pass would be worse than a visible skip.
//!
//! What these prove that the unit tests cannot: the unit tests check the parsing
//! rules given a column type, but whether MySQL actually reports those types, and
//! whether `KILL QUERY` really interrupts a statement, are facts about the wire.

use std::time::Instant;

use qh_core::{FailureKind, Value};
use qh_driver::{
    BrowseLevel, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions, ObjectPath, Session,
    TlsMode,
};
use qh_driver_mysql::MysqlDriver;

const SKIP_HINT: &str = "skipped: set QH_TEST_MYSQL=1 with deploy/dev/up.sh mysql running";

/// The dev container's settings, matching deploy/dev/up.sh.
fn config() -> Option<ConnectionConfig> {
    if std::env::var("QH_TEST_MYSQL").as_deref() != Ok("1") {
        return None;
    }
    Some(
        ConnectionConfig::new(
            DriverKind::Mysql,
            std::env::var("QH_MYSQL_HOST").unwrap_or_else(|_| "127.0.0.1".to_owned()),
            std::env::var("QH_MYSQL_PORT")
                .ok()
                .and_then(|port| port.parse().ok())
                .unwrap_or(53306),
            "qh",
        )
        .password("qh-dev-only")
        .database("qh")
        // The driver refuses every other mode until a TLS backend is compiled in,
        // so this is the only mode that exists rather than a shortcut.
        .tls(TlsMode::Disable),
    )
}

async fn connect() -> Option<Box<dyn Session>> {
    let config = config()?;
    Some(
        MysqlDriver::new()
            .connect(&config)
            .await
            .expect("connect to the dev container"),
    )
}

/// Collect every batch, with the per-batch row counts.
async fn drain(cursor: &mut Box<dyn Cursor>, max_rows: usize) -> (Vec<Vec<Value>>, Vec<usize>) {
    let mut rows = Vec::new();
    let mut batch_sizes = Vec::new();
    while let Some(batch) = cursor.next_batch(max_rows).await.expect("next_batch") {
        batch_sizes.push(batch.rows());
        for index in 0..batch.rows() {
            rows.push(
                (0..batch.width())
                    .map(|column| batch.value(index, column).expect("cell").clone())
                    .collect(),
            );
        }
    }
    (rows, batch_sizes)
}

#[tokio::test]
async fn connects_and_reports_its_capabilities_without_asking_the_server() {
    let Some(config) = config() else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let driver = MysqlDriver::new();
    assert_eq!(
        driver.capabilities().levels,
        vec![BrowseLevel::Database, BrowseLevel::Table]
    );

    let session = driver.connect(&config).await.expect("connect");
    assert!(session.capabilities().cancel);
    session.close().await.expect("close");
}

#[tokio::test]
async fn a_column_arrives_with_the_type_the_server_reported() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let cursor = session
        .execute(
            "SELECT CAST(1 AS SIGNED) AS n, 'x' AS t, CAST(1.50 AS DECIMAL(10,2)) AS amount",
            &ExecuteOptions::default(),
        )
        .await
        .expect("execute");

    let names: Vec<&str> = cursor
        .columns()
        .iter()
        .map(|column| &*column.name)
        .collect();
    let types: Vec<&str> = cursor
        .columns()
        .iter()
        .map(|column| &*column.type_name)
        .collect();
    assert_eq!(names, vec!["n", "t", "amount"]);
    // MySQL reports the column type with the result, so unlike PostgreSQL there is
    // no separate describe step — but the names still have to be right.
    assert_eq!(types, vec!["bigint", "varchar", "decimal"]);
}

#[tokio::test]
async fn the_hard_types_survive_the_round_trip_from_the_server() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let mut cursor = session
        .execute(
            "SELECT exact_money, tiny_negative, a_datetime, a_date, a_time, a_json, \
             raw_bytes, a_enum, a_char_uuid, a_binary_uuid, no_value, empty_text, \
             the_word_null FROM type_zoo",
            &ExecuteOptions::default(),
        )
        .await
        .expect("execute");
    let (rows, _) = drain(&mut cursor, 8).await;
    assert_eq!(rows.len(), 1, "type_zoo should hold exactly one row");

    let rendered = |index: usize| rows[0][index].render_text();

    // The same 38 digits the PostgreSQL driver is tested with: neither database
    // may round this.
    assert_eq!(
        rendered(0).unwrap(),
        "1234567890123456789012345678.1234567890",
        "the exact decimal lost digits on the way through"
    );
    assert_eq!(rendered(1).unwrap(), "-0.0000000001");

    // DATETIME is a wall clock with no zone, so it comes back exactly as stored.
    assert_eq!(rendered(2).unwrap(), "2026-01-31 12:00:00.123456");
    assert_eq!(rendered(3).unwrap(), "2026-01-31");
    assert_eq!(rendered(4).unwrap(), "23:59:59.999999");

    // MySQL returns JSON as text with its own key normalisation, which is kept
    // rather than re-encoded.
    assert!(rendered(5).unwrap().contains("\"a\""), "{:?}", rendered(5));

    // A blob holding a NUL and a non-UTF8 byte.
    assert_eq!(
        rows[0][6],
        Value::Bytes(vec![0x00, 0x01, 0xff]),
        "blob came back wrong"
    );
    // An ENUM is its label, not its ordinal.
    assert_eq!(rows[0][7], Value::Text("ok".into()));
    assert_eq!(rendered(8).unwrap(), "550e8400-e29b-41d4-a716-446655440000");
    // `binary(16)` holds arbitrary bytes; those bytes are what must survive.
    match &rows[0][9] {
        Value::Bytes(bytes) => assert_eq!(bytes.len(), 16, "the binary uuid is the wrong length"),
        other => panic!("binary(16) should arrive as bytes, got {other:?}"),
    }

    // Three ways a cell can look empty, and none of them is another.
    assert_eq!(rows[0][10], Value::Null, "a NULL must stay a NULL");
    assert_eq!(
        rows[0][11],
        Value::Text("".into()),
        "an empty string is not a NULL"
    );
    assert_eq!(
        rows[0][12],
        Value::Text("NULL".into()),
        "the word NULL is not a NULL"
    );
}

#[tokio::test]
async fn the_callers_row_ceiling_is_honoured_though_the_producer_reads_in_its_own_batches() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let mut cursor = session
        .execute(
            "SELECT id FROM wide_500k ORDER BY id",
            &ExecuteOptions::default(),
        )
        .await
        .expect("execute");

    // The producer reads in batches of 1024; asking for 250 must still hand back
    // exactly 250. Getting this wrong is how a grid draws rows it did not ask for.
    let mut sizes = Vec::new();
    let mut seen = 0usize;
    while let Some(batch) = cursor.next_batch(250).await.expect("next_batch") {
        sizes.push(batch.rows());
        seen += batch.rows();
        if seen >= 1_000 {
            break;
        }
    }
    assert_eq!(sizes, vec![250, 250, 250, 250], "sizes were {sizes:?}");
    assert_eq!(seen, 1_000);
}

#[tokio::test]
async fn batches_arrive_in_order_across_the_batch_boundary() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    // The slicing must not shuffle or duplicate anything: the ids have to read
    // 1, 2, 3 ... straight through several producer batches.
    let mut cursor = session
        .execute(
            "SELECT id FROM wide_500k ORDER BY id",
            &ExecuteOptions::default(),
        )
        .await
        .expect("execute");
    let mut expected = 1i64;
    while let Some(batch) = cursor.next_batch(700).await.expect("next_batch") {
        for index in 0..batch.rows() {
            let value = batch.value(index, 0).expect("cell");
            assert_eq!(
                value,
                &Value::Int(expected),
                "row {expected} came back out of order"
            );
            expected += 1;
        }
        if expected > 3_000 {
            break;
        }
    }
    assert!(expected > 3_000, "the scan stopped early");
}

#[tokio::test]
async fn a_row_limit_stops_reading_without_changing_the_statement() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let options = ExecuteOptions {
        max_batch_rows: None,
        row_limit: Some(10),
    };
    let mut cursor = session
        .execute("SELECT id FROM wide_500k ORDER BY id", &options)
        .await
        .unwrap();
    let (rows, _) = drain(&mut cursor, 4).await;

    // The engine stops reading; it never rewrites the caller's SQL to add a
    // LIMIT, because the grid must not change which query runs.
    assert_eq!(rows.len(), 10);
    assert_eq!(rows[0][0], Value::Int(1));
    assert_eq!(rows[9][0], Value::Int(10));
}

#[tokio::test]
async fn a_statement_with_no_result_set_yields_no_batches() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    // DDL has no columns to fill. It must end cleanly rather than hand back an
    // empty batch that a caller might read as "there is more".
    let mut cursor = session
        .execute(
            "CREATE TEMPORARY TABLE qh_ddl_probe (id int)",
            &ExecuteOptions::default(),
        )
        .await
        .expect("execute");
    assert!(cursor.columns().is_empty());
    assert!(cursor.next_batch(10).await.expect("next_batch").is_none());
}

#[tokio::test]
async fn kill_query_reaches_the_server_and_the_session_stays_usable() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let mut cursor = session
        .execute("SELECT SLEEP(30)", &ExecuteOptions::default())
        .await
        .expect("execute");

    // Let the statement actually start, or the kill arrives before there is
    // anything to kill and the test proves nothing.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    let started = Instant::now();
    session.cancel().await.expect("cancel");
    let outcome = cursor.next_batch(8).await;
    let latency = started.elapsed();

    // Section 6 asks for under 500 ms, so it is asserted rather than printed.
    // This is also what proves the kill reached the server: a `SLEEP(30)` that was
    // not interrupted cannot finish in under half a second.
    assert!(
        latency < std::time::Duration::from_millis(500),
        "cancel took {latency:?}, over the 500 ms target"
    );

    // MySQL's `SLEEP()` reports an interruption by *returning 0* rather than by
    // raising an error, so the value is the signal here and there is no code to
    // check. That is why the latency assertion above is what proves the kill
    // landed: an uninterruptible `SLEEP(30)` cannot finish in under half a second.
    //
    // A second test once tried to prove the error-code path with a long join. It
    // was removed rather than kept: the join kept running for 91 seconds and only
    // stopped when the thread was killed by hand from outside the test, so the
    // driver's cancel did not interrupt it. That is an open defect, recorded as
    // K11 in PROGRESS.md, and a test that hangs for 90 seconds and then passes for
    // the wrong reason is worse than no test.
    match outcome {
        Ok(Some(batch)) => {
            assert_eq!(batch.rows(), 1, "SLEEP returns exactly one row: {batch:?}");
            assert_eq!(
                batch.value(0, 0),
                Some(&Value::Int(0)),
                "SLEEP signals interruption with 0; anything else means it ran to completion"
            );
        }
        Ok(None) => panic!("SLEEP produced nothing, so it never reported an outcome"),
        Err(error) => panic!("SLEEP reported an error instead of its interrupt value: {error:?}"),
    }

    // The session must still work — `KILL QUERY` and not `KILL CONNECTION` is the
    // whole reason this is not a dropped connection.
    let mut after = session
        .execute("SELECT 1", &ExecuteOptions::default())
        .await
        .expect("the session was unusable after a cancel");
    let (rows, _) = drain(&mut after, 4).await;
    assert_eq!(rows[0][0], Value::Int(1));

    session.close().await.expect("close");
}

#[tokio::test]
async fn cancelling_with_nothing_running_is_not_an_error() {
    let Some(session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    // A user can press stop after the query already finished.
    session.cancel().await.expect("cancel with nothing running");
    session.close().await.expect("close");
}

#[tokio::test]
async fn the_tree_lists_databases_with_the_system_ones_hidden_by_default() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let databases = session
        .browse(BrowseLevel::Database, &ObjectPath::new(), false)
        .await
        .expect("databases");
    assert!(databases.contains(&"qh".to_owned()), "{databases:?}");
    // A user browsing a tree is not looking for the server's own databases.
    for hidden in ["information_schema", "mysql", "performance_schema", "sys"] {
        assert!(
            !databases.contains(&hidden.to_owned()),
            "{hidden} should be hidden: {databases:?}"
        );
    }

    // Someone asking for everything gets everything, and then they do appear.
    let all = session
        .browse(BrowseLevel::Catalog, &ObjectPath::new(), true)
        .await
        .expect("databases with everything");
    assert!(all.contains(&"information_schema".to_owned()), "{all:?}");
}

#[tokio::test]
async fn the_schema_level_is_refused_with_a_hint_naming_what_to_use() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    // MySQL has no schema level — its middle level is the database. Answering with
    // an empty list would look like "you have no schemas" rather than "there is no
    // such level here", so the message has to say what to use instead.
    let error = session
        .browse(BrowseLevel::Schema, &ObjectPath::new(), false)
        .await
        .expect_err("a schema browse should be refused");
    assert!(error.message().contains("schemas"), "{error:?}");
    assert!(
        error.message().contains("catalogs lists its databases"),
        "{error:?}"
    );
}

#[tokio::test]
async fn the_table_list_and_objects_grid_read_real_metadata() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let tables = session
        .browse(BrowseLevel::Table, &ObjectPath::new().schema("qh"), false)
        .await
        .expect("tables");
    assert!(tables.contains(&"wide_500k".to_owned()), "{tables:?}");
    assert!(tables.contains(&"type_zoo".to_owned()), "{tables:?}");

    let page = session
        .objects(&ObjectPath::new().schema("qh"))
        .await
        .expect("objects");
    // The four columns the Python driver declared, unchanged.
    assert_eq!(page.columns, vec!["Name", "Engine", "Rows", "Comment"]);
    let wide = page
        .rows
        .iter()
        .find(|row| row[0] == "wide_500k")
        .expect("wide_500k is missing");
    assert_eq!(wide.len(), 4);
    assert_eq!(
        wide[1], "InnoDB",
        "the engine column should name the storage engine: {wide:?}"
    );
    // A NULL in the comment column is the empty string here: this grid draws a
    // NULL and an empty cell alike.
    assert_eq!(wide[3], "");
}

#[tokio::test]
async fn a_syntax_error_carries_the_server_code_and_is_not_retried() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    // Matched rather than `expect_err`, because the success type is a trait object.
    let error = match session
        .execute("SELECT nope FROM", &ExecuteOptions::default())
        .await
    {
        Ok(_) => panic!("a syntax error was accepted"),
        Err(error) => error,
    };

    assert!(error.code().is_some(), "no error code on {error:?}");
    assert_eq!(
        error.failure_kind(),
        FailureKind::Permanent,
        "a syntax error must not retry"
    );
    assert!(error.message().contains("SELECT nope FROM"), "{error:?}");
}

#[tokio::test]
async fn a_bad_password_is_a_permanent_failure_not_a_retry_loop() {
    let Some(config) = config() else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let wrong = config.password("definitely-not-the-password");

    let error = MysqlDriver::new()
        .connect(&wrong)
        .await
        .err()
        .expect("connecting with the wrong password should fail");

    assert_eq!(error.failure_kind(), FailureKind::Permanent, "{error:?}");
    assert!(
        !error.message().contains("definitely-not-the-password"),
        "{error:?}"
    );
}

#[tokio::test]
async fn tls_is_refused_rather_than_silently_downgraded() {
    let Some(config) = config() else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    for mode in [TlsMode::Prefer, TlsMode::Require, TlsMode::RequireNoVerify] {
        let error = MysqlDriver::new()
            .connect(&config.clone().tls(mode))
            .await
            .err()
            .unwrap_or_else(|| panic!("{mode:?} was accepted, which would connect in clear"));
        assert!(error.message().contains("TLS"), "{mode:?}: {error:?}");
    }
}
