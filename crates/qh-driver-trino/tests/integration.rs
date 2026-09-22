//! Integration tests against a real Trino server.
//!
//! ```bash
//! deploy/dev/up.sh trino
//! QH_TEST_TRINO=1 cargo test -p qh-driver-trino --test integration
//! ```
//!
//! Without `QH_TEST_TRINO=1` each test prints why it is skipping and returns. A
//! silent pass would be worse than a visible skip.
//!
//! What these prove that the unit tests cannot: the unit tests check the decoding
//! rules given a type name, but *which* type names and *which* JSON encodings a
//! live coordinator produces are facts about the wire. Every encoding asserted
//! here was read off Trino 483 first and then pinned, and the release is named in
//! the tests because Trino makes no cross-version guarantees.

use std::time::{Duration, Instant};

use qh_core::{FailureKind, Value};
use qh_driver::{
    BrowseLevel, ConnectionConfig, Driver, DriverKind, ExecuteOptions, ObjectPath, Session, TlsMode,
};
use qh_driver_trino::TrinoDriver;

const SKIP_HINT: &str = "skipped: set QH_TEST_TRINO=1 with deploy/dev/up.sh trino running";

/// The dev container's settings, matching deploy/dev/up.sh.
fn config() -> Option<ConnectionConfig> {
    if std::env::var("QH_TEST_TRINO").as_deref() != Ok("1") {
        return None;
    }
    Some(
        ConnectionConfig::new(
            DriverKind::Trino,
            std::env::var("QH_TRINO_HOST").unwrap_or_else(|_| "127.0.0.1".to_owned()),
            std::env::var("QH_TRINO_PORT")
                .ok()
                .and_then(|port| port.parse().ok())
                .unwrap_or(58080),
            // The fourth argument is the *user*, not the database. Trino needs a
            // catalog whenever a schema is set, and it answers 400 with "Schema is
            // set but catalog is not" when one is missing -- which is how this was
            // found rather than assumed.
            "queryhive",
        )
        .database("tpch")
        .schema("tiny")
        .tls(TlsMode::Disable),
    )
}

async fn connect() -> Option<Box<dyn Session>> {
    let config = config()?;
    Some(TrinoDriver::new().connect(&config).await.expect("connect"))
}

/// Run a statement to completion and return its rows.
async fn rows(session: &mut Box<dyn Session>, sql: &str) -> Vec<Vec<Value>> {
    let mut cursor = session
        .execute(sql, &ExecuteOptions::default())
        .await
        .unwrap_or_else(|error| panic!("execute {sql}: {error:?}"));
    let mut out = Vec::new();
    while let Some(batch) = cursor.next_batch(1024).await.expect("next_batch") {
        let columns = batch.columns();
        let height = columns.first().map_or(0, Vec::len);
        for index in 0..height {
            out.push(columns.iter().map(|column| column[index].clone()).collect());
        }
    }
    out
}

#[tokio::test]
async fn connects_and_reports_what_it_is() {
    let Some(session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let capabilities = session.capabilities();
    // No socket is kept between statements: an HTTP exchange, then nothing. This
    // is what ADR-0006 decided, and anything that assumes a connection must ask.
    assert!(!capabilities.persistent_connection);
    assert!(!capabilities.transactions);
    assert!(capabilities.cancel, "DELETE reaches the server");
    assert_eq!(
        capabilities.levels,
        vec![
            BrowseLevel::Catalog,
            BrowseLevel::Schema,
            BrowseLevel::Table
        ]
    );
    assert_eq!(capabilities.objects_columns, vec!["Name", "Type"]);
}

#[tokio::test]
async fn a_query_streams_rows_with_the_types_the_server_reported() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let mut cursor = session
        .execute(
            "SELECT CAST(1 AS BIGINT) AS n, 'x' AS t, true AS b, CAST(NULL AS INTEGER) AS nul",
            &ExecuteOptions::default(),
        )
        .await
        .expect("execute");

    // By design the columns are not known until a page carries them: the first
    // POST answers QUEUED, and waiting for the columns there would mean waiting
    // for the whole query. This asserts the deliberate deviation rather than
    // leaving it to be discovered.
    assert!(
        cursor.columns().is_empty(),
        "a queued query has no columns yet, and pretending otherwise costs a blocked execute"
    );

    let batch = cursor
        .next_batch(64)
        .await
        .expect("next_batch")
        .expect("a batch");

    // ...and they are there by the time rows are, which is when a grid needs them.
    let columns: Vec<(String, String)> = cursor
        .columns()
        .iter()
        .map(|column| (column.name.to_string(), column.type_name.to_string()))
        .collect();
    assert_eq!(
        columns,
        vec![
            ("n".to_owned(), "bigint".to_owned()),
            ("t".to_owned(), "varchar(1)".to_owned()),
            ("b".to_owned(), "boolean".to_owned()),
            ("nul".to_owned(), "integer".to_owned()),
        ]
    );

    assert_eq!(batch.columns()[0], vec![Value::Int(1)]);
    assert_eq!(batch.columns()[1], vec![Value::Text("x".into())]);
    assert_eq!(batch.columns()[2], vec![Value::Bool(true)]);
    assert_eq!(batch.columns()[3], vec![Value::Null]);
}

#[tokio::test]
async fn a_decimal_keeps_every_digit_through_the_real_protocol() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    // 38 significant digits. Trino sends a decimal as a JSON *string*, which is
    // why it survives: a float could not carry this and the engine's promise is
    // that nothing is quietly rounded.
    let rows = rows(
        &mut session,
        "SELECT CAST(1234567890123456789012345678.1234567890 AS DECIMAL(38, 10))",
    )
    .await;
    assert_eq!(
        rows[0][0],
        Value::Decimal {
            unscaled: 12_345_678_901_234_567_890_123_456_781_234_567_890,
            scale: 10,
        }
    );
}

#[tokio::test]
async fn the_protocol_truncates_timestamps_to_milliseconds_and_this_pins_that() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    // The server holds microseconds -- `typeof` says so -- and the JSON does not
    // carry them. Measured here so the limit is a pinned fact about the wire
    // rather than a claim in a comment: if Trino ever starts sending them, this
    // fails and the note in the decoder gets updated instead of going stale.
    let rows = rows(
        &mut session,
        "SELECT CAST('2026-01-31 12:00:00.123456' AS TIMESTAMP(6)), \
         typeof(CAST('2026-01-31 12:00:00.123456' AS TIMESTAMP(6)))",
    )
    .await;

    assert_eq!(
        rows[0][1],
        Value::Text("timestamp(6)".into()),
        "the server's value really is microsecond precision"
    );
    match rows[0][0] {
        Value::Timestamp { micros, .. } => {
            assert_eq!(
                micros % 1_000_000,
                123_000,
                "milliseconds only: the .123456 the server holds arrives as .123"
            );
        }
        ref other => panic!("expected a timestamp, got {other:?}"),
    }
}

#[tokio::test]
async fn varbinary_arrives_base64_and_is_decoded_to_bytes() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    // X'00FF' arrives as "AP8=". A NUL byte is the case that shows whether the
    // decoder was base64 or optimistic text handling.
    let rows = rows(&mut session, "SELECT X'00FF'").await;
    assert_eq!(rows[0][0], Value::Bytes(vec![0x00, 0xff]));
}

#[tokio::test]
async fn browse_walks_the_catalogs_schemas_and_tables_tree() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let catalogs = session
        .browse(BrowseLevel::Catalog, &ObjectPath::new(), false)
        .await
        .expect("catalogs");
    assert!(catalogs.contains(&"tpch".to_owned()), "got {catalogs:?}");

    let schemas = session
        .browse(
            BrowseLevel::Schema,
            &ObjectPath::new().catalog("tpch"),
            false,
        )
        .await
        .expect("schemas");
    assert!(schemas.contains(&"tiny".to_owned()), "got {schemas:?}");

    let tables = session
        .browse(
            BrowseLevel::Table,
            &ObjectPath::new().catalog("tpch").schema("tiny"),
            false,
        )
        .await
        .expect("tables");
    assert!(tables.contains(&"orders".to_owned()), "got {tables:?}");
}

#[tokio::test]
async fn the_objects_grid_carries_name_and_type_and_invents_nothing() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let page = session
        .objects(&ObjectPath::new().catalog("tpch").schema("tiny"))
        .await
        .expect("objects");

    // Two columns, because two are true here. Trino's information_schema exposes
    // no OID, owner or ACL, so a wider grid would be a wider invention.
    assert_eq!(page.columns, vec!["Name".to_owned(), "Type".to_owned()]);
    assert!(
        page.rows.iter().any(|row| row[0] == "orders"),
        "got {:?}",
        page.rows
    );
    assert!(
        page.rows
            .iter()
            .all(|row| row[1] == "BASE TABLE" || row[1] == "VIEW"),
        "every row should carry the server's own type: {:?}",
        page.rows
    );
    assert!(
        page.rows.iter().all(|row| row.len() == 2),
        "a ragged grid row would be a bug in the driver, not in the data"
    );
}

#[tokio::test]
async fn a_syntax_error_carries_the_servers_code_and_is_not_retried() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    // The POST itself succeeds: Trino answers 200 with a `nextUri`, and the error
    // arrives on a page when that URI is fetched. Measured, not assumed -- so the
    // driver maps errors in *both* places, and this test would pass either way but
    // only be honest about one.
    let mut cursor = session
        .execute("SELECT nope FROM", &ExecuteOptions::default())
        .await
        .expect("the POST answers with a page; the error comes with the next one");

    let error = cursor
        .next_batch(8)
        .await
        .expect_err("a syntax error should surface when the page is fetched");

    match error {
        qh_core::EngineError::Query {
            kind,
            code,
            message,
            ..
        } => {
            // Trino's own code for a syntax error, and `USER_ERROR`, which means
            // repeating the statement produces the same thing forever: a retry
            // loop here would spin.
            assert_eq!(kind, FailureKind::Permanent, "{message}");
            assert_eq!(code.as_deref(), Some("1"), "{message}");
            assert!(message.contains("SYNTAX_ERROR"), "{message}");
        }
        other => panic!("expected a query error, got {other:?}"),
    }
}

#[tokio::test]
async fn cancel_stops_a_running_query_and_is_reported_as_cancelled() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    // Long enough to interrupt, and it is genuinely still running when the cursor
    // comes back -- which is the point: a caller that has to wait for the result
    // set to begin has nothing to cancel.
    let started = Instant::now();
    let mut cursor = session
        .execute(
            "SELECT count(*) FROM tpch.tiny.lineitem a \
             CROSS JOIN tpch.tiny.lineitem b CROSS JOIN tpch.tiny.lineitem c",
            &ExecuteOptions::default(),
        )
        .await
        .expect("execute should return while the query is still queued");
    let submit = started.elapsed();
    assert!(
        submit < Duration::from_secs(5),
        "execute took {submit:?}; a caller without a cursor cannot cancel anything"
    );
    assert!(
        session.query_id().is_some(),
        "the running query should be known"
    );

    tokio::time::sleep(Duration::from_millis(300)).await;
    session.cancel().await.expect("cancel");

    // Bounded, so a regression fails with a message instead of hanging the suite.
    let outcome = tokio::time::timeout(Duration::from_secs(20), cursor.next_batch(8))
        .await
        .expect("cancel did not stop the query within twenty seconds");

    match outcome {
        Err(qh_core::EngineError::Query {
            kind,
            code,
            message,
            ..
        }) => {
            // Trino answers a successful cancel with `USER_CANCELED` carrying
            // `errorType = USER_ERROR`. Reporting that as a plain user error would
            // tell the user their stop button was a failure.
            assert_eq!(kind, FailureKind::Cancelled, "{message}");
            assert_eq!(code.as_deref(), Some("3"), "{message}");
        }
        Err(other) => panic!("expected a cancellation, got {other:?}"),
        Ok(batch) => panic!("the query was not cancelled: got {batch:?}"),
    }
}

#[tokio::test]
async fn cancelling_with_nothing_running_is_not_an_error() {
    let Some(session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    // The button can be pressed after the query finished. Calling that a failure
    // would put an error in front of the user for doing the right thing.
    session.cancel().await.expect("cancel with nothing running");
}

#[tokio::test]
async fn a_level_trino_does_not_have_is_refused_with_a_usable_message() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    // Trino's tree is catalog, schema, table. Asking for a database level gets an
    // explanation rather than an empty list that looks like an empty catalog.
    let error = session
        .browse(BrowseLevel::Database, &ObjectPath::new(), false)
        .await
        .expect_err("Trino has no database level");
    match error {
        qh_core::EngineError::Usage { message } => {
            assert!(message.contains("catalog"), "{message}")
        }
        other => panic!("expected a usage error, got {other:?}"),
    }
}

#[tokio::test]
async fn a_browse_call_uses_the_connections_own_catalog_and_schema() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    // An empty path is not an error: the connection's own catalog and schema are
    // the fallback, which is what makes the tree open on something sensible without
    // the UI having to pass the whole path down. The refusal path -- a *missing*
    // catalog with nothing to fall back to -- is covered by the unit tests, where
    // it can be provoked without a server.
    let tables = session
        .browse(BrowseLevel::Table, &ObjectPath::new(), false)
        .await
        .expect("the connection's catalog and schema should be used");
    assert!(tables.contains(&"orders".to_owned()), "got {tables:?}");
}

#[tokio::test]
async fn the_callers_row_ceiling_is_honoured_across_pages() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let mut cursor = session
        .execute(
            "SELECT * FROM tpch.tiny.orders",
            &ExecuteOptions {
                max_batch_rows: None,
                row_limit: Some(7),
            },
        )
        .await
        .expect("execute");

    let mut total = 0usize;
    while let Some(batch) = cursor.next_batch(3).await.expect("next_batch") {
        let rows = batch.columns().first().map_or(0, Vec::len);
        assert!(rows <= 3, "a batch of {rows} exceeds what was asked for");
        total += rows;
    }
    // The row limit is the caller's ceiling, not a rewrite of the statement: the
    // query the user sees is the query that ran.
    assert_eq!(total, 7);
}

#[tokio::test]
async fn explain_returns_the_servers_own_plan() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let statement = session.explain_statement("SELECT 1");
    assert_eq!(statement, "EXPLAIN SELECT 1");
    let rows = rows(&mut session, &statement).await;
    // Trino's plan is one text column, one row per plan line.
    assert!(!rows.is_empty(), "a plan should have at least one line");
}

#[tokio::test]
async fn tls_is_refused_rather_than_silently_downgraded() {
    // No server needed: the refusal happens before any request is made, which is
    // the whole point -- a client that accepts `require` and then talks in clear is
    // worse than one that says it cannot yet.
    let Some(_) = config() else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let base = config().expect("config");
    for mode in [TlsMode::Prefer, TlsMode::Require] {
        let with_tls = ConnectionConfig::new(DriverKind::Trino, "127.0.0.1", 58080, "queryhive")
            .database("tpch")
            .schema("tiny")
            .tls(mode);
        match TrinoDriver::new().connect(&with_tls).await {
            Err(qh_core::EngineError::Connect { kind, message }) => {
                assert_eq!(kind, FailureKind::Permanent);
                assert!(message.contains("K8"), "{message}");
            }
            other => panic!("{mode:?} should be refused, got {:?}", other.is_ok()),
        }
    }
    // And the plaintext config still works, so the refusal is about TLS and not
    // about the driver being unable to connect at all.
    TrinoDriver::new()
        .connect(&base)
        .await
        .expect("plaintext still connects");
}
