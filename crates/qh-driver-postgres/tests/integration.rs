//! Integration tests against a real PostgreSQL server.
//!
//! ```bash
//! deploy/dev/up.sh postgres
//! QH_TEST_POSTGRES=1 cargo test -p qh-driver-postgres --test integration
//! ```
//!
//! Without `QH_TEST_POSTGRES=1` every test prints why it is skipping and returns.
//! That is a deliberate choice over silently passing: a green run that tested
//! nothing is worse than a visible skip, and CI sets the variable.
//!
//! Why these exist at all, given the unit tests next door
//! -----------------------------------------------------
//! The unit tests prove the parsing rules. They cannot prove that the values
//! PostgreSQL actually sends reach those rules: column type names come from the
//! server, and whether a `numeric(38,10)` arrives as text with its digits intact
//! is a fact about the wire, not about this code. That is what is checked here,
//! against the same `type_zoo` table the Python golden snapshots use.

use std::time::Instant;

use qh_core::Value;
use qh_driver::{
    BrowseLevel, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions, ObjectPath, Session,
    TlsMode,
};
use qh_driver_postgres::PostgresDriver;

const SKIP_HINT: &str = "skipped: set QH_TEST_POSTGRES=1 with deploy/dev/up.sh postgres running";

/// The dev container's settings, matching deploy/dev/up.sh.
fn config() -> Option<ConnectionConfig> {
    if std::env::var("QH_TEST_POSTGRES").as_deref() != Ok("1") {
        return None;
    }
    Some(
        ConnectionConfig::new(
            DriverKind::Postgres,
            std::env::var("QH_PG_HOST").unwrap_or_else(|_| "127.0.0.1".to_owned()),
            std::env::var("QH_PG_PORT")
                .ok()
                .and_then(|port| port.parse().ok())
                .unwrap_or(55432),
            "qh",
        )
        .password("qh-dev-only")
        .database("qh")
        // The driver refuses every other mode until the rustls connector exists,
        // so this is not a shortcut: it is the only mode that is implemented.
        .tls(TlsMode::Disable),
    )
}

async fn connect() -> Option<Box<dyn Session>> {
    let config = config()?;
    Some(
        PostgresDriver::new()
            .connect(&config)
            .await
            .expect("connect to the dev container"),
    )
}

/// Collect every batch of a statement, with the per-batch row counts.
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
    let driver = PostgresDriver::new();

    // Read before connecting, because the connection editor needs it for
    // connections that do not exist yet.
    assert_eq!(
        driver.capabilities().levels,
        vec![BrowseLevel::Schema, BrowseLevel::Table]
    );

    let session = driver.connect(&config).await.expect("connect");
    assert!(session.capabilities().cancel);
    session.close().await.expect("close");
}

#[tokio::test]
async fn a_column_arrives_with_the_type_the_server_named() {
    // This is the whole reason the driver describes the statement first: the
    // simple query protocol carries no type at all, so without the describe step
    // the grid would have no type chip and the parser no key.
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let cursor = session
        .execute(
            "SELECT 1::int4 AS n, 'x'::text AS t, 1.50::numeric(10,2) AS amount",
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
    assert_eq!(types, vec!["int4", "text", "numeric"]);
}

#[tokio::test]
async fn the_hard_types_survive_the_round_trip_from_the_server() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let mut cursor = session
        .execute(
            "SELECT exact_money, tz_aware, tz_naive, a_date, an_interval, raw_bytes, \
             a_uuid, no_value, empty_text, the_word_null FROM type_zoo",
            &ExecuteOptions::default(),
        )
        .await
        .expect("execute");
    let (rows, _) = drain(&mut cursor, 8).await;

    // Prefer the real error over an index panic if the fixture is missing.
    assert_eq!(rows.len(), 1, "type_zoo should hold exactly one row");

    let rendered = |index: usize| rows[0][index].render_text();

    // The value the Python engine produced for the same row, and the one this
    // must never round: a NUMERIC(38,10) has more digits than an f64 holds.
    assert_eq!(
        rendered(0).unwrap(),
        "1234567890123456789012345678.1234567890",
        "the exact decimal lost digits on the way through"
    );
    // PostgreSQL renders `timestamptz` in the **session's** TimeZone, which
    // defaults to UTC — so the `+07:00` the row was written with is not what
    // comes back, and that is the server's behaviour rather than a lost offset.
    // What must hold is that the *instant* survived: 12:00:00.123456+07:00 and
    // 05:00:00.123456+00:00 are the same moment, seven hours apart on the clock.
    assert_eq!(
        rendered(1).unwrap(),
        "2026-01-31 05:00:00.123456+00:00",
        "the instant moved, not just the rendering"
    );
    // The row was written as +07:00, so a client that asks for that zone must get
    // it back — otherwise the offset really would be lost rather than re-rendered.
    let mut zoned = session
        .execute("SET TIME ZONE 'Asia/Jakarta'", &ExecuteOptions::default())
        .await
        .expect("set time zone");
    while zoned.next_batch(1).await.expect("drain").is_some() {}
    let mut again = session
        .execute("SELECT tz_aware FROM type_zoo", &ExecuteOptions::default())
        .await
        .expect("execute");
    let (zoned_rows, _) = drain(&mut again, 1).await;
    assert_eq!(
        zoned_rows[0][0].render_text().unwrap(),
        "2026-01-31 12:00:00.123456+07:00",
        "the same instant renders in the session's zone"
    );

    // ...and the naive column must not have gained one.
    assert!(!rendered(2).unwrap().contains('+'), "{:?}", rendered(2));
    assert_eq!(rendered(3).unwrap(), "2026-01-31");
    assert!(rendered(4).unwrap().contains("3 days"), "{:?}", rendered(4));

    // Bytes that are neither valid UTF-8 nor free of NULs.
    assert_eq!(
        rows[0][5],
        Value::Bytes(vec![0x00, 0x01, 0xff]),
        "bytea came back wrong"
    );
    assert_eq!(rendered(6).unwrap(), "550e8400-e29b-41d4-a716-446655440000");

    // The three ways a cell can look empty, and none of them is the others.
    assert_eq!(rows[0][7], Value::Null, "a NULL must stay a NULL");
    assert_eq!(
        rows[0][8],
        Value::Text("".into()),
        "an empty string is not a NULL"
    );
    assert_eq!(
        rows[0][9],
        Value::Text("NULL".into()),
        "the word NULL is not a NULL"
    );
}

#[tokio::test]
async fn a_large_result_streams_in_batches_rather_than_arriving_at_once() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let mut cursor = session
        .execute(
            "SELECT id, c01 FROM wide_500k ORDER BY id",
            &ExecuteOptions::default(),
        )
        .await
        .expect("execute");

    // 1,000 rows in batches of 250 must arrive as four batches, and the first
    // must be usable long before the last: that is what time-to-first-row means.
    let mut sizes = Vec::new();
    let mut seen = 0usize;
    while let Some(batch) = cursor.next_batch(250).await.expect("next_batch") {
        if sizes.is_empty() {
            assert_eq!(
                batch.rows(),
                250,
                "the first batch was not filled to the asked-for size"
            );
        }
        sizes.push(batch.rows());
        seen += batch.rows();
        if seen >= 1_000 {
            break;
        }
    }
    assert_eq!(sizes.len(), 4, "expected four batches, got {sizes:?}");
    assert_eq!(seen, 1_000);
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
async fn cancel_reaches_the_server_and_the_connection_stays_usable() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let mut cursor = session
        .execute("SELECT pg_sleep(30)", &ExecuteOptions::default())
        .await
        .expect("execute");

    // Let the query actually start, or the cancel arrives before there is
    // anything to cancel and the test proves nothing.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    let started = Instant::now();
    session.cancel().await.expect("cancel");
    let outcome = cursor.next_batch(8).await;
    let latency = started.elapsed();

    // Blueprint section 6 asks for under 500 ms. Asserted rather than printed,
    // because that is the requirement and a lenient bound would hide a failure.
    assert!(
        latency < std::time::Duration::from_millis(500),
        "cancel took {latency:?}, over the 500 ms target"
    );

    // The server cancelled the query, and it said so: SQLSTATE 57014 is
    // query_canceled. A killed process could not produce this.
    match outcome {
        Err(error) => {
            assert_eq!(
                error.code(),
                Some("57014"),
                "expected a cancel, got {error:?}"
            );
        }
        Ok(batch) => panic!("the query kept producing rows after cancel: {batch:?}"),
    }

    // And the connection is still usable, which is the other half of the
    // requirement: a cancel must not leave a poisoned session behind.
    let mut after = session
        .execute("SELECT 1::int4", &ExecuteOptions::default())
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
    // A user can press stop after the query already finished, so this has to be
    // a no-op rather than a failure.
    session.cancel().await.expect("cancel with nothing running");
    session.close().await.expect("close");
}

#[tokio::test]
async fn the_object_tree_and_objects_grid_read_real_metadata() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let schemas = session
        .browse(BrowseLevel::Schema, &ObjectPath::new())
        .await
        .expect("schemas");
    assert!(schemas.contains(&"public".to_owned()), "{schemas:?}");
    // The server's own catalogues are not the user's schemas.
    assert!(
        !schemas.iter().any(|name| name.starts_with("pg_")),
        "{schemas:?}"
    );
    assert!(
        !schemas.contains(&"information_schema".to_owned()),
        "{schemas:?}"
    );

    let tables = session
        .browse(BrowseLevel::Table, &ObjectPath::new().schema("public"))
        .await
        .expect("tables");
    assert!(tables.contains(&"wide_500k".to_owned()), "{tables:?}");
    assert!(tables.contains(&"type_zoo".to_owned()), "{tables:?}");

    let page = session
        .objects(&ObjectPath::new().schema("public"))
        .await
        .expect("objects");
    // The four columns the Python driver declared, unchanged.
    assert_eq!(page.columns, vec!["Name", "OID", "Owner", "ACL"]);
    let wide = page
        .rows
        .iter()
        .find(|row| row[0] == "wide_500k")
        .expect("wide_500k is missing");
    assert_eq!(wide.len(), 4);
    assert!(
        wide[1].parse::<u32>().is_ok(),
        "the OID column is not a number: {wide:?}"
    );
    // A NULL in the ACL column is the empty string here: this grid draws a NULL
    // and an empty cell alike.
    assert_eq!(wide[3], "");
}

#[tokio::test]
async fn a_level_the_driver_does_not_have_is_refused_by_name() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    // PostgreSQL has no catalog level. Answering with an empty list would look
    // like "you have no catalogs" instead of "this driver has no such level".
    let error = session
        .browse(BrowseLevel::Catalog, &ObjectPath::new())
        .await
        .expect_err("a catalog browse should be refused");
    assert!(error.message().contains("catalog"), "{error:?}");
    assert!(error.message().contains("schema/table"), "{error:?}");
}

#[tokio::test]
async fn a_syntax_error_carries_the_sqlstate_and_is_not_retried() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    // Matched rather than `expect_err`, because the success type is a trait
    // object and so has no `Debug` for the panic message to use.
    let error = match session
        .execute("SELECT nope FROM", &ExecuteOptions::default())
        .await
    {
        Ok(_) => panic!("a syntax error was accepted"),
        Err(error) => error,
    };

    // SQLSTATE, kept as the server spells it so a search finds PostgreSQL's own
    // documentation.
    assert!(error.code().is_some(), "no SQLSTATE on {error:?}");
    assert_eq!(
        error.failure_kind(),
        qh_core::FailureKind::Permanent,
        "a syntax error must not retry"
    );
    // And the statement is included so the user can see which one failed.
    assert!(error.message().contains("SELECT nope FROM"), "{error:?}");
}

#[tokio::test]
async fn a_bad_password_is_a_permanent_failure_not_a_retry_loop() {
    let Some(config) = config() else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let wrong = config.password("definitely-not-the-password");

    let error = PostgresDriver::new()
        .connect(&wrong)
        .await
        .err()
        .expect("connecting with the wrong password should fail");

    // Retrying cannot help, so the UI must show it instead of looping.
    assert_eq!(
        error.failure_kind(),
        qh_core::FailureKind::Permanent,
        "{error:?}"
    );
    // And the message must not contain the password it was given.
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
        let error = PostgresDriver::new()
            .connect(&config.clone().tls(mode))
            .await
            .err()
            .unwrap_or_else(|| panic!("{mode:?} was accepted, which would connect in clear"));
        assert!(error.message().contains("TLS"), "{mode:?}: {error:?}");
    }
}
