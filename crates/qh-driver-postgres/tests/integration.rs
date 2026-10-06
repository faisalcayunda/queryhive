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

use std::time::{Duration, Instant};

use qh_core::{EngineError, Value};
use qh_driver::{
    BrowseLevel, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions, ObjectPath,
    Parameter, Session, TlsMode,
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
        // The dev container here is the one from `deploy/dev/up.sh`, which serves
        // no TLS at all, so the tests about queries ask for no TLS either. What the
        // driver does with the other three modes is `tests/tls.rs`'s subject, and
        // the two modes that interact with a server without TLS are checked at the
        // bottom of this file.
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
    // The interval the server sent, in the three parts it sent it as. The Python
    // engine's psycopg folded `1 year 2 mons` into days before anything could
    // render it (`"428 days, 4:05:06"` in the live snapshot); a month is not a fixed
    // number of days, so that fold is the loss `normalize.rs` refuses to make.
    assert_eq!(rendered(4).unwrap(), "14 months, 3 days, 4:05:06");

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
async fn an_array_arrives_as_the_servers_own_literal_and_a_uuid_without_quotes() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let mut cursor = session
        .execute(
            "SELECT '{1,NULL,3}'::int[] AS a, '{a,NULL,c}'::text[] AS b, \
             '{{1,2},{3,NULL}}'::int[] AS c, a_uuid FROM type_zoo",
            &ExecuteOptions::default(),
        )
        .await
        .expect("execute");

    // The name the server reports for an array column is the one a future parser
    // would key on, and it is the element's name with a leading underscore — not the
    // element's. `{{1,2},{3,NULL}}` is why that parser is not written yet: an array
    // has dimensions, braces inside braces, and elements that may contain commas
    // inside quotes.
    let types: Vec<&str> = cursor
        .columns()
        .iter()
        .map(|column| &*column.type_name)
        .collect();
    assert_eq!(types, vec!["_int4", "_text", "_int4", "uuid"]);

    let (rows, _) = drain(&mut cursor, 4).await;
    assert_eq!(
        rows[0],
        vec![
            Value::Text("{1,NULL,3}".into()),
            Value::Text("{a,NULL,c}".into()),
            Value::Text("{{1,2},{3,NULL}}".into()),
            // Bare: the JSON quotes the previous engine wrote came from
            // `json.dumps(default=str)`, not from PostgreSQL.
            Value::Text("550e8400-e29b-41d4-a716-446655440000".into()),
        ],
        "an array or a uuid stopped being the server's own text"
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
        statement_timeout: None,
        bulk: false,
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
        .browse(BrowseLevel::Schema, &ObjectPath::new(), false)
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
        .browse(
            BrowseLevel::Table,
            &ObjectPath::new().schema("public"),
            false,
        )
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
async fn catalogs_lists_databases_even_though_it_is_not_a_tree_level() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    // The tree levels are schema and table, because a connection is bound to one
    // database. But `catalogs` is a question worth answering — it is what a
    // context picker lists — and the Python engine answered it. Refusing here was
    // a regression this test now prevents.
    let databases = session
        .browse(BrowseLevel::Catalog, &ObjectPath::new(), false)
        .await
        .expect("catalogs should list databases, not be refused");
    assert!(databases.contains(&"qh".to_owned()), "{databases:?}");
    // Templates and databases that refuse connections are filtered out by
    // default: offering one would be a choice that cannot be taken.
    assert!(
        !databases.iter().any(|name| name == "template0"),
        "{databases:?}"
    );

    // The same list under the name MySQL gives this level, because that is the other
    // name a caller may bring: `catalogs` is the command, `database` is what the
    // level is called on MySQL, and the one list answers both. A caller that renames
    // the level without this would refuse a command that works.
    let as_database = session
        .browse(BrowseLevel::Database, &ObjectPath::new(), false)
        .await
        .expect("the database level name should reach the same list");
    assert_eq!(as_database, databases);

    // "Show all" drops the filter, and then a template does appear — which is the
    // difference between the two forms of the statement, not a detail.
    let all = session
        .browse(BrowseLevel::Catalog, &ObjectPath::new(), true)
        .await
        .expect("catalogs with everything");
    assert!(all.len() >= databases.len(), "{all:?} vs {databases:?}");
    assert!(all.contains(&"template0".to_owned()), "{all:?}");
}

#[tokio::test]
async fn the_system_schemas_are_hidden_unless_they_are_asked_for() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let visible = session
        .browse(BrowseLevel::Schema, &ObjectPath::new(), false)
        .await
        .unwrap();
    assert!(
        !visible.iter().any(|name| name.starts_with("pg_")),
        "{visible:?}"
    );

    // Someone asking for everything wants `pg_catalog` to appear like any other
    // schema, so the filter is dropped rather than inverted.
    let all = session
        .browse(BrowseLevel::Schema, &ObjectPath::new(), true)
        .await
        .unwrap();
    assert!(all.contains(&"pg_catalog".to_owned()), "{all:?}");
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
async fn prefer_falls_back_to_plaintext_against_a_server_that_does_not_offer_tls() {
    // The one case where a fallback is correct, and it needs a real server to prove
    // it: this container runs with `ssl = off`, so it answers the SSLRequest with
    // "no" and `Prefer` is allowed to continue in clear. `tests/tls.rs` covers the
    // other case — a server that offers TLS and then fails the handshake — where
    // falling back would be the downgrade an attacker triggers.
    let Some(config) = config() else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let mut session = PostgresDriver::new()
        .connect(&config.tls(TlsMode::Prefer))
        .await
        .expect("Prefer should connect to a server that does not offer TLS");

    // Prefer, not a lie: the session really is in clear, which is what the mode
    // allows when the server declined. Asked of the server, so a fallback that
    // quietly did not happen would also be caught.
    let mut cursor = session
        .execute(
            "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
            &ExecuteOptions::default(),
        )
        .await
        .expect("execute");
    let batch = cursor
        .next_batch(1)
        .await
        .expect("next_batch")
        .expect("a row");
    assert_eq!(
        batch.value(0, 0),
        Some(&Value::Bool(false)),
        "this container does not serve TLS; if it now does, this test and the config \\
         above need to move to tests/tls.rs"
    );
    session.close().await.expect("close");
}

#[tokio::test]
async fn require_refuses_a_server_that_does_not_offer_tls() {
    // The opposite decision from the same server: `Require` must not fall back,
    // because the user asked for TLS. The error is the mode working.
    let Some(config) = config() else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let error = PostgresDriver::new()
        .connect(&config.tls(TlsMode::Require))
        .await
        .err()
        .expect("Require connected to a server that does not offer TLS");

    assert!(error.message().contains("TLS"), "{error:?}");
}

#[tokio::test]
async fn a_statement_timeout_is_a_typed_error_that_names_the_limit() {
    // The bound is PostgreSQL's own `statement_timeout`, so the server cancels the
    // statement rather than this process stopping its read. A short bound against a
    // five-second sleep is the discriminating pair: without the mechanism the test
    // would wait five seconds, and without the classification it would come back as a
    // generic `57014` query error.
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let started = Instant::now();
    let mut cursor = session
        .execute(
            "SELECT pg_sleep(5)",
            &ExecuteOptions {
                statement_timeout: Some(Duration::from_millis(500)),
                ..ExecuteOptions::default()
            },
        )
        .await
        .expect("the statement is described and started");
    let error = loop {
        match cursor.next_batch(10).await {
            Ok(Some(_)) => continue,
            Ok(None) => panic!("pg_sleep(5) finished instead of timing out"),
            Err(error) => break error,
        }
    };
    let elapsed = started.elapsed();

    match &error {
        EngineError::Timeout { message, limit_ms } => {
            assert_eq!(*limit_ms, Some(500));
            assert!(message.contains("500 ms"), "{message}");
            // The server's own sentence travels with it.
            assert!(message.contains("statement timeout"), "{message}");
        }
        other => panic!("expected a typed timeout, got {other:?}"),
    }
    // Promptly, not after the sleep. A wide margin over the 500 ms bound: the point is
    // that it did not wait five seconds, and CI is not a latency benchmark.
    assert!(
        elapsed < Duration::from_secs(4),
        "the timeout took {elapsed:?}, which is the sleep and not the bound"
    );
}

#[tokio::test]
async fn a_bound_statement_sends_the_value_out_of_band() {
    // Parameters are the point: a value never becomes SQL text, so the escaping
    // question does not arise. The temp table keeps it from touching anything in the
    // dev database and lets it run repeatedly.
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let mut cursor = session
        .execute(
            "CREATE TEMP TABLE qh_bind (id int4, name text, amount numeric(10,2))",
            &ExecuteOptions::default(),
        )
        .await
        .expect("create temp table");
    drain(&mut cursor, 1_000).await;

    // The quote in the text is data the binder never has to escape.
    let mut cursor = session
        .execute_bound(
            "INSERT INTO qh_bind (id, name, amount) VALUES ($1, $2, $3)",
            &[
                Parameter::Int(1),
                Parameter::Text("O'Brien".to_owned()),
                Parameter::Text("12.34".to_owned()),
            ],
            &ExecuteOptions::default(),
        )
        .await
        .expect("bound insert");
    drain(&mut cursor, 1_000).await;
    assert_eq!(cursor.affected_rows(), Some(1));

    // A NULL parameter is a real NULL, not the four characters.
    let mut cursor = session
        .execute_bound(
            "UPDATE qh_bind SET name = $1 WHERE id = $2",
            &[Parameter::Null, Parameter::Int(1)],
            &ExecuteOptions::default(),
        )
        .await
        .expect("bound update to null");
    drain(&mut cursor, 1_000).await;
    assert_eq!(cursor.affected_rows(), Some(1));

    // A parameterized statement that returns rows is refused by name: the extended
    // protocol returns binary values and this driver's decoder is the text path.
    let refused = session
        .execute_bound(
            "SELECT id FROM qh_bind WHERE name = $1",
            &[Parameter::Text("O'Brien".to_owned())],
            &ExecuteOptions::default(),
        )
        .await;
    match refused {
        Err(EngineError::Usage { .. }) => {}
        Err(other) => panic!("expected a usage refusal, got {other:?}"),
        Ok(_) => panic!("a row-returning bound statement must be refused, but it ran"),
    }

    // Read the row back unparameterized: the quote survived, the NULL is a NULL, and
    // the numeric stayed exact.
    let mut cursor = session
        .execute(
            "SELECT id, name, amount FROM qh_bind",
            &ExecuteOptions::default(),
        )
        .await
        .expect("read back");
    let (rows, _) = drain(&mut cursor, 10).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0][0], Value::Int(1));
    assert_eq!(rows[0][1], Value::Null);
    assert_eq!(
        rows[0][2],
        Value::Decimal {
            unscaled: 1234,
            scale: 2
        }
    );

    session.close().await.expect("close");
}

// --------------------------------------------------------------------------- //
// server-enforced read-only (W3-T0b): the layer that does not depend on reading the text
// --------------------------------------------------------------------------- //

async fn run_all(session: &mut Box<dyn Session>, sql: &str) -> Result<(), EngineError> {
    let mut cursor = session.execute(sql, &ExecuteOptions::default()).await?;
    while cursor.next_batch(100).await?.is_some() {}
    Ok(())
}

#[tokio::test]
async fn a_read_only_session_refuses_writes_on_the_server_until_it_is_reset() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let table = format!("qh_w3t0b_ro_{}", std::process::id());
    let sequence = format!("{table}_seq");
    run_all(&mut session, &format!("DROP TABLE IF EXISTS {table}"))
        .await
        .expect("drop leftovers");
    run_all(&mut session, &format!("DROP SEQUENCE IF EXISTS {sequence}"))
        .await
        .expect("drop leftovers");
    run_all(&mut session, &format!("CREATE TABLE {table} (id INT)"))
        .await
        .expect("create");
    run_all(&mut session, &format!("CREATE SEQUENCE {sequence}"))
        .await
        .expect("create sequence");
    run_all(&mut session, &format!("INSERT INTO {table} VALUES (1)"))
        .await
        .expect("seed");
    // A function that writes, called from a SELECT: the shape the guard reads as a plain read.
    run_all(
        &mut session,
        &format!(
            "CREATE OR REPLACE FUNCTION {table}_f() RETURNS int LANGUAGE sql AS \
             $$ INSERT INTO {table} VALUES (2) RETURNING id $$"
        ),
    )
    .await
    .expect("create function");

    assert!(session.read_only_statement().is_some());
    session.enforce_read_only().await.expect("switch it on");

    for sql in [
        format!("INSERT INTO {table} VALUES (3)"),
        format!("DELETE FROM {table}"),
        format!("SELECT nextval('{sequence}')"),
        format!("SELECT {table}_f()"),
        format!("CREATE TABLE {table}_x (a int)"),
    ] {
        let error = run_all(&mut session, &sql)
            .await
            .expect_err(&format!("the server must refuse {sql}"));
        assert!(
            error.to_string().contains("read-only transaction"),
            "{sql}: {error}"
        );
    }
    // Reads still work, and a user's `SET` cannot be what turns it off: the guard refuses it,
    // and what the guard would let through (`SELECT`) cannot reach the setting.
    run_all(&mut session, &format!("SELECT count(*) FROM {table}"))
        .await
        .expect("a read still runs");

    // The pool's reset gives the next run a normal session.
    session.reset().await.expect("reset");
    run_all(&mut session, &format!("INSERT INTO {table} VALUES (4)"))
        .await
        .expect("after the reset a write runs again");
    run_all(&mut session, &format!("SELECT nextval('{sequence}')"))
        .await
        .expect("and so does nextval");

    let _ = run_all(&mut session, &format!("DROP FUNCTION {table}_f()")).await;
    let _ = run_all(&mut session, &format!("DROP TABLE {table}")).await;
    let _ = run_all(&mut session, &format!("DROP SEQUENCE {sequence}")).await;
}

// --------------------------------------------------------------------------- //
// COPY (<select>) TO STDOUT (W7-T2): the bulk read must be the normal read, faster
// --------------------------------------------------------------------------- //

fn bulk() -> ExecuteOptions {
    ExecuteOptions {
        bulk: true,
        ..ExecuteOptions::default()
    }
}

/// Run `sql` on the normal path and on the bulk path and require the same columns and the same
/// cells. Compared through `Debug` so a `NaN` equals itself.
async fn assert_copy_matches(session: &mut Box<dyn Session>, sql: &str) -> usize {
    let mut normal = session
        .execute(sql, &ExecuteOptions::default())
        .await
        .expect("normal");
    let (expected, _) = drain(&mut normal, 1000).await;
    let expected_columns = format!("{:?}", normal.columns());
    drop(normal);
    let mut copied = session.execute(sql, &bulk()).await.expect("bulk");
    let (actual, _) = drain(&mut copied, 777).await;
    assert_eq!(format!("{:?}", copied.columns()), expected_columns, "{sql}");
    assert_eq!(actual.len(), expected.len(), "row count of {sql}");
    for (index, (left, right)) in actual.iter().zip(&expected).enumerate() {
        assert_eq!(
            format!("{left:?}"),
            format!("{right:?}"),
            "row {index} of {sql}"
        );
    }
    actual.len()
}

#[tokio::test]
async fn copy_returns_type_zoo_cell_for_cell() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let rows = assert_copy_matches(&mut session, "SELECT * FROM type_zoo ORDER BY 1").await;
    assert!(rows > 0);
    // The session time zone changes how timestamptz is written, on both paths alike.
    session
        .execute("SET TIME ZONE 'Asia/Jakarta'", &ExecuteOptions::default())
        .await
        .expect("set");
    assert_copy_matches(&mut session, "SELECT * FROM type_zoo ORDER BY 1").await;
    session.close().await.expect("close");
}

#[tokio::test]
async fn copy_returns_the_first_40k_rows_of_wide_500k_cell_for_cell() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let rows = assert_copy_matches(
        &mut session,
        "SELECT * FROM wide_500k ORDER BY id LIMIT 40000",
    )
    .await;
    assert_eq!(rows, 40_000);
    session.close().await.expect("close");
}

#[tokio::test]
async fn copy_keeps_null_empty_escapes_and_multibyte_text_apart() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    for sql in [
        "SELECT E'a\\tb\\nc\\\\d\\r\\b\\f\\v' AS s, NULL::text AS n, '' AS e, E'\\\\N' AS not_null, \
         'N' AS plain_n, 'Jakarta \u{1F30F} \u{65e5}\u{672c}' AS wide, '\\xdead'::bytea AS b, \
         ARRAY['a\\b', 'c\"d']::text[] AS arr, '{\"k\": \"a\\\\tb\"}'::jsonb AS j",
        "SELECT NULL::int AS only_null",
        "SELECT '' AS only_empty",
        // A trailing `;`, and a trailing line comment that would swallow the closing paren.
        "SELECT 1 AS a;",
        "SELECT 'x;y' AS a -- note",
        "WITH x AS (SELECT generate_series(1, 5) AS n) SELECT n FROM x",
    ] {
        assert_copy_matches(&mut session, sql).await;
    }
    // Zero rows: the cursor reports the columns and ends.
    let mut cursor = session
        .execute("SELECT 1 AS a, 'x' AS b WHERE false", &bulk())
        .await
        .expect("empty");
    assert!(cursor.next_batch(10).await.expect("next").is_none());
    assert_eq!(cursor.columns().len(), 2);
    session.close().await.expect("close");
}

/// How many other backends are running a `COPY (...) TO STDOUT` carrying `marker` right now.
async fn count_copies(observer: &mut Box<dyn Session>, marker: &str) -> i64 {
    let mut cursor = observer
        .execute(
            &format!(
                "SELECT count(*) FROM pg_stat_activity WHERE query LIKE 'COPY (%{marker}%TO STDOUT' \
                 AND pid <> pg_backend_pid()"
            ),
            &ExecuteOptions::default(),
        )
        .await
        .expect("observe");
    let (rows, _) = drain(&mut cursor, 10).await;
    match &rows[0][0] {
        Value::Int(count) => *count,
        other => panic!("unexpected count {other:?}"),
    }
}

#[tokio::test]
async fn bulk_really_sends_copy_for_a_plain_select_and_only_for_that() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let mut observer = connect().await.expect("observer");
    let marker = format!("w7t2_marker_{}", std::process::id());
    let mut cursor = session
        .execute(
            &format!("SELECT * FROM wide_500k ORDER BY id /* {marker} */"),
            &bulk(),
        )
        .await
        .expect("bulk");
    assert!(cursor.next_batch(10).await.expect("first").is_some());
    assert_eq!(count_copies(&mut observer, &marker).await, 1);
    session.cancel().await.expect("cancel");
    drop(cursor);
    session.reset().await.ok();

    // Not a plain select: the flag is a hint, and the normal path runs.
    let mut cursor = session
        .execute("SHOW server_version", &bulk())
        .await
        .expect("show");
    assert!(cursor.next_batch(10).await.expect("rows").is_some());
    drop(cursor);
    observer.close().await.expect("close");
}

#[tokio::test]
async fn copy_honours_the_row_limit() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let options = ExecuteOptions {
        row_limit: Some(25),
        bulk: true,
        ..ExecuteOptions::default()
    };
    let mut cursor = session
        .execute("SELECT id FROM wide_500k ORDER BY id", &options)
        .await
        .expect("execute");
    let (rows, _) = drain(&mut cursor, 10).await;
    assert_eq!(rows.len(), 25);
    session.cancel().await.expect("cancel");
    session.close().await.ok();
}

#[tokio::test]
async fn copy_never_turns_a_read_into_a_write_and_obeys_a_read_only_session() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let table = format!("qh_w7t2_ro_{}", std::process::id());
    run_all(&mut session, &format!("DROP TABLE IF EXISTS {table}"))
        .await
        .expect("drop");
    run_all(&mut session, &format!("CREATE TABLE {table} (id INT)"))
        .await
        .expect("create");
    run_all(&mut session, &format!("INSERT INTO {table} VALUES (1)"))
        .await
        .expect("seed");

    // A data-modifying CTE is not a plain select: it takes the normal path, so it writes
    // exactly when the normal path would, and not at all in a read-only session.
    let writer =
        format!("WITH d AS (INSERT INTO {table} VALUES (2) RETURNING id) SELECT id FROM d");
    run_all(&mut session, "SET default_transaction_read_only = on")
        .await
        .expect("read only");
    let error = match session.execute(&writer, &bulk()).await {
        Err(error) => error.to_string(),
        Ok(mut cursor) => cursor
            .next_batch(10)
            .await
            .expect_err("must be refused")
            .to_string(),
    };
    assert!(error.contains("read-only"), "{error}");

    // A plain read is still a read there, and COPY TO STDOUT does not need a writable session.
    assert_copy_matches(&mut session, &format!("SELECT id FROM {table}")).await;

    run_all(&mut session, "SET default_transaction_read_only = off")
        .await
        .expect("writable");
    let mut cursor = session
        .execute(&format!("SELECT count(*) FROM {table}"), &bulk())
        .await
        .expect("count");
    let (rows, _) = drain(&mut cursor, 10).await;
    assert_eq!(rows[0][0], Value::Int(1), "the CTE must not have written");
    drop(cursor);
    run_all(&mut session, &format!("DROP TABLE {table}"))
        .await
        .expect("drop");
    session.close().await.expect("close");
}

/// The first four bytes of the statement the server says this backend is running. A plain
/// `SELECT` reports `SELE`, and the same statement sent as a bulk read reports `COPY`, so the
/// path that really ran is observable from inside the result.
async fn running_statement_prefix(
    session: &mut Box<dyn Session>,
    options: &ExecuteOptions,
) -> String {
    let mut cursor = session
        .execute(
            "SELECT left(query, 4) FROM pg_stat_activity WHERE pid = pg_backend_pid()",
            options,
        )
        .await
        .expect("probe");
    let (rows, _) = drain(&mut cursor, 10).await;
    match &rows[0][0] {
        Value::Text(text) => text.to_string(),
        other => panic!("unexpected probe {other:?}"),
    }
}

#[tokio::test]
async fn a_bulk_read_is_a_copy_also_in_a_read_only_session() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let normal = ExecuteOptions::default();
    assert_eq!(
        running_statement_prefix(&mut session, &normal).await,
        "SELE"
    );
    assert_eq!(
        running_statement_prefix(&mut session, &bulk()).await,
        "COPY"
    );
    // The server-side read-only mode `Safe Mode` turns on: COPY ... TO STDOUT reads, so it
    // is neither refused nor skipped, and the result is still the normal path's result.
    run_all(
        &mut session,
        "SET SESSION default_transaction_read_only = on",
    )
    .await
    .expect("read only");
    assert_eq!(
        running_statement_prefix(&mut session, &bulk()).await,
        "COPY"
    );
    assert_copy_matches(&mut session, "SELECT * FROM type_zoo ORDER BY 1").await;
    session.close().await.expect("close");
}

/// The server keeps the first bytes of a COPY until it has 8 KB of rows or an end to send, so
/// a bulk `execute` returns then and not when the statement starts. A stop that comes first
/// drops the call and cancels, which is what the commands do.
#[tokio::test]
async fn a_stop_before_the_first_rows_of_a_copy_cancels_it_and_the_connection_stays_usable() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let waiting = tokio::time::timeout(
        Duration::from_millis(250),
        session.execute("SELECT pg_sleep(30)", &bulk()),
    )
    .await;
    assert!(waiting.is_err(), "the sleeping COPY has sent nothing yet");
    drop(waiting);

    let started = Instant::now();
    session.cancel().await.expect("cancel");
    assert_eq!(
        running_statement_prefix(&mut session, &bulk()).await,
        "COPY"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the session waited for the sleep: {:?}",
        started.elapsed()
    );
    session.close().await.expect("close");
}

#[tokio::test]
async fn cancelling_a_streaming_copy_stops_it_and_the_connection_stays_usable() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    // Far more rows than can be read before the cancel lands. The set-returning function is in
    // the select list because in FROM the server would spool every row to a temp file first.
    let mut cursor = session
        .execute("SELECT generate_series(1, 2000000000) AS g", &bulk())
        .await
        .expect("execute");
    assert!(cursor.next_batch(100).await.expect("first").is_some());

    let started = Instant::now();
    session.cancel().await.expect("cancel");
    drop(cursor);
    // The same session, no reset in between: the rest of the copy is discarded as it arrives,
    // the server's cancel ends it, and the next statement runs.
    assert_eq!(
        running_statement_prefix(&mut session, &bulk()).await,
        "COPY"
    );
    assert_copy_matches(&mut session, "SELECT * FROM type_zoo ORDER BY 1").await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the copy was not cancelled: {:?}",
        started.elapsed()
    );
    session.close().await.expect("close");
}

#[tokio::test]
async fn a_statement_timeout_inside_a_copy_is_the_same_typed_error() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let started = Instant::now();
    let mut cursor = session
        .execute(
            "SELECT pg_sleep(5)",
            &ExecuteOptions {
                statement_timeout: Some(Duration::from_millis(500)),
                bulk: true,
                ..ExecuteOptions::default()
            },
        )
        .await
        .expect("the statement is described and started");
    let error = cursor.next_batch(10).await.expect_err("must time out");
    match &error {
        EngineError::Timeout { message, limit_ms } => {
            assert_eq!(*limit_ms, Some(500));
            assert!(message.contains("statement timeout"), "{message}");
        }
        other => panic!("expected a typed timeout, got {other:?}"),
    }
    // Once, not twice: a statement that timed out is not started again by a fallback.
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    drop(cursor);
    session.close().await.expect("close");
}

/// Plain selects whose text could break the wrapper: a terminator, a comment that runs to the
/// end of the line, strings and dollar quotes holding parentheses and `COPY` words. Each ends
/// in a column that names the statement the server was running, which must be the COPY.
#[tokio::test]
async fn awkward_but_plain_selects_are_wrapped_and_still_return_the_same_rows() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let path = "(SELECT left(query, 4) FROM pg_stat_activity WHERE pid = pg_backend_pid())";
    for sql in [
        format!("SELECT 1 AS a, {path} AS path"),
        format!("SELECT 1 AS a, {path} AS path;"),
        format!("SELECT 1 AS a, {path} AS path ; -- done"),
        format!("SELECT 1 AS a, {path} AS path -- done"),
        format!("-- lead\nSELECT 1 AS a, {path} AS path /* tail ) */"),
        format!("SELECT ') TO STDOUT; --' AS a, $q$) ; COPY x TO$q$ AS b, {path} AS path"),
        format!("WITH x AS (SELECT 2 AS n) SELECT n, {path} AS path FROM x"),
        format!("  \n SELECT 'é日' AS a, {path} AS path  \n"),
    ] {
        let mut cursor = session.execute(&sql, &bulk()).await.expect(&sql);
        let (rows, _) = drain(&mut cursor, 10).await;
        assert_eq!(rows.len(), 1, "{sql}");
        let last = rows[0].last().expect("a path column");
        assert_eq!(*last, Value::Text("COPY".into()), "{sql}");
        drop(cursor);
        // Cell for cell against the normal path, ignoring the path column that differs by design.
        let mut normal = session
            .execute(&sql, &ExecuteOptions::default())
            .await
            .expect(&sql);
        let (expected, _) = drain(&mut normal, 10).await;
        let width = rows[0].len() - 1;
        assert_eq!(rows[0][..width], expected[0][..width], "{sql}");
    }
    session.close().await.expect("close");
}
