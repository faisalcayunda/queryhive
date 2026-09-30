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

use std::time::{Duration, Instant};

use mysql_async::consts::{ColumnFlags, ColumnType};
use mysql_async::prelude::Queryable;
use qh_core::{EngineError, FailureKind, Value};
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
        // Everything here is about the protocol, the types and the statements, not
        // about encryption, and the development container's certificate is the one
        // MySQL generates for itself on first start — which no root store trusts.
        // The TLS modes have their own suite in `tests/tls.rs` and their own
        // containers, so this one stays in clear deliberately.
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

/// The three columns MySQL describes with the **same** type code, and the flag that
/// is the only thing telling them apart.
///
/// `ENUM`, `CHAR(36)` and `BINARY(16)` all arrive as `MYSQL_TYPE_STRING` — 254 — so
/// `column_type()` names all three `char`, and the Python engine's DBAPI description
/// names all three `254`, which is why `tests/golden/RECORDED.md` could not tell the
/// enum from the char either. The spelling is in the column's flags instead, and
/// this reads them off the real server rather than from the manual: a column is an
/// enum only because MySQL 8.4.11 says so in `ENUM_FLAG`.
///
/// The raw connection is opened here rather than through the driver because the
/// flags do not survive into `ColumnMeta` — the driver hands the name on, and this
/// is the measurement the name is derived from.
#[tokio::test]
async fn the_flags_tell_an_enum_from_a_char_though_the_type_code_is_the_same() {
    let Some(config) = config() else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let mut conn = mysql_async::Conn::new(
        mysql_async::OptsBuilder::default()
            .ip_or_hostname(config.host.clone())
            .tcp_port(config.port)
            .user(Some(config.user.clone()))
            .pass(config.password.clone())
            .db_name(config.database.clone())
            .prefer_socket(false),
    )
    .await
    .expect("connect to the dev container");

    let result = conn
        .query_iter("SELECT * FROM type_zoo")
        .await
        .expect("describe type_zoo");
    let described: Vec<(String, ColumnType, ColumnFlags, String)> = result
        .columns_ref()
        .iter()
        .map(|column| {
            (
                column.name_str().into_owned(),
                column.column_type(),
                column.flags(),
                qh_driver_mysql::normalize::type_name(column),
            )
        })
        .collect();
    let _ = conn.disconnect().await;

    let column = |name: &str| {
        described
            .iter()
            .find(|(column, ..)| column == name)
            .unwrap_or_else(|| panic!("type_zoo has no column {name}: {described:?}"))
    };

    // The type code on its own cannot name any of the three.
    for name in ["a_enum", "a_char_uuid", "a_binary_uuid"] {
        assert_eq!(
            column(name).1,
            ColumnType::MYSQL_TYPE_STRING,
            "{name} is 254, the code CHAR, ENUM and BINARY all share"
        );
    }

    // The flag is what separates the enum from the two char columns, and the raw
    // bits are pinned: 256 on the enum, 0 on the char, 128 (BINARY_FLAG) on the
    // binary uuid — none of them derived from the type code.
    let (_, _, flags, type_name) = column("a_enum");
    assert!(
        flags.contains(ColumnFlags::ENUM_FLAG),
        "the server did not set ENUM_FLAG on a_enum: {flags:?}"
    );
    assert_eq!(type_name, "enum");
    assert_eq!(flags.bits(), 256, "measured on the dev container");

    for (name, expected_bits) in [("a_char_uuid", 0u16), ("a_binary_uuid", 128u16)] {
        let (_, _, flags, type_name) = column(name);
        assert!(
            !flags.contains(ColumnFlags::ENUM_FLAG),
            "{name} must not look like an enum: {flags:?}"
        );
        assert_eq!(type_name, "char", "{name}");
        assert_eq!(
            flags.bits(),
            expected_bits,
            "{name} measured on the container"
        );
    }
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
        statement_timeout: None,
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
async fn a_write_reports_the_rows_the_server_said_it_wrote() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let _ = session
        .execute(
            "DROP TABLE IF EXISTS qh_affected_probe",
            &ExecuteOptions::default(),
        )
        .await;

    // The count a `CREATE TABLE AS SELECT` reports is the number of rows it wrote, and
    // it is only readable once the cursor has been drained: the OK packet that ends the
    // result set is the one that carries it.
    let mut cursor = session
        .execute(
            "CREATE TABLE qh_affected_probe AS \
             SELECT 1 AS id UNION ALL SELECT 2 UNION ALL SELECT 3",
            &ExecuteOptions::default(),
        )
        .await
        .expect("execute");
    while cursor.next_batch(100).await.expect("next_batch").is_some() {}
    assert_eq!(
        cursor.affected_rows(),
        Some(3),
        "the CREATE reports the three rows it wrote"
    );

    let mut cursor = session
        .execute(
            "INSERT INTO qh_affected_probe SELECT id FROM qh_affected_probe",
            &ExecuteOptions::default(),
        )
        .await
        .expect("execute");
    while cursor.next_batch(100).await.expect("next_batch").is_some() {}
    assert_eq!(
        cursor.affected_rows(),
        Some(3),
        "the INSERT reports the three rows it added"
    );

    // The count is the server's, not this driver's arithmetic: the table really holds
    // six rows now, counted by the server in a statement that reports no count at all.
    let mut cursor = session
        .execute(
            "SELECT COUNT(*) FROM qh_affected_probe",
            &ExecuteOptions::default(),
        )
        .await
        .expect("execute");
    let (rows, _) = drain(&mut cursor, 10).await;
    assert_eq!(rows[0][0].render_text().as_deref(), Some("6"));

    let _ = session
        .execute("DROP TABLE qh_affected_probe", &ExecuteOptions::default())
        .await;
}

#[tokio::test]
async fn kill_query_reaches_the_server_and_the_session_stays_usable() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    // `execute` must hand back a cursor while the query is still running. If it
    // blocks until the result set begins -- which for a blocking query is when the
    // query ends -- the caller has nothing to cancel, and pressing stop does
    // nothing at all. That was the real defect: measured, this took 2.002 s for
    // `SELECT SLEEP(2)` and over 5 s for a long join, before any cursor existed.
    let submitted = Instant::now();
    let mut cursor = session
        .execute("SELECT SLEEP(30)", &ExecuteOptions::default())
        .await
        .expect("execute");
    let submit_latency = submitted.elapsed();
    assert!(
        submit_latency < std::time::Duration::from_millis(500),
        "execute blocked for {submit_latency:?}; the statement was never handed over, \
         so no cancel could reach it"
    );

    // Let the statement actually start, or the kill arrives before there is
    // anything to kill and the test proves nothing.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    let started = Instant::now();
    session.cancel().await.expect("cancel");
    let outcome = cursor.next_batch(8).await;
    let latency = started.elapsed();
    let total = submitted.elapsed();

    // Section 6 asks for under 500 ms, so it is asserted rather than printed.
    // This is also what proves the kill reached the server: a `SLEEP(30)` that was
    // not interrupted cannot finish in under half a second.
    assert!(
        latency < std::time::Duration::from_millis(500),
        "cancel took {latency:?}, over the 500 ms target"
    );

    // `SLEEP()` reports which happened through its return value, and the two are
    // the opposite way round from what this test first assumed: **1 means it was
    // interrupted, 0 means it slept the full duration and returned normally**. The
    // earlier version asserted 0 and passed -- because `execute` had blocked for the
    // whole thirty seconds, the statement had already finished, and the value it saw
    // was the ordinary completion value. Asserting 1 is what distinguishes an
    // interrupt from a query that simply ran to the end.
    //
    // `total` is what makes this proof rather than coincidence. An earlier version
    // of this test passed while proving nothing: `execute` blocked until the query
    // finished, so by the time `cancel` ran the statement was already over, SLEEP
    // had returned its ordinary 0, and the latency assertion measured a cancel of
    // nothing. The whole suite took 30.28 s -- exactly `SLEEP(30)` running to
    // completion. Asserting the total keeps that from coming back.
    match outcome {
        Ok(Some(batch)) => {
            assert_eq!(batch.rows(), 1, "SLEEP returns exactly one row: {batch:?}");
            assert_eq!(
                batch.value(0, 0),
                Some(&Value::Int(1)),
                "1 is SLEEP reporting the interruption; 0 would mean it slept the full \
                 thirty seconds and nothing stopped it"
            );
        }
        Ok(None) => panic!("SLEEP produced nothing, so it never reported an outcome"),
        Err(error) => panic!("SLEEP reported an error instead of its interrupt value: {error:?}"),
    }

    // The uninterruptible `SLEEP(30)` would have taken thirty seconds; this proves
    // it did not run to completion, which is the difference between an interrupt
    // and a query that simply finished.
    assert!(
        total < std::time::Duration::from_secs(10),
        "the statement ran for {total:?}, so nothing interrupted it"
    );

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
async fn an_interrupted_statement_reports_the_servers_own_code() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    // A join with no usable index: long enough to interrupt, and unlike SLEEP it
    // does not absorb the interrupt as a return value. This test was deleted once
    // because it hung for 91 seconds and only passed after the thread was killed by
    // hand from outside; with `execute` no longer waiting for the result set, the
    // cursor exists while the query runs and cancel can reach it.
    let mut cursor = session
        .execute(
            "SELECT COUNT(*) FROM wide_500k a JOIN wide_500k b ON a.id < b.id",
            &ExecuteOptions::default(),
        )
        .await
        .expect("execute should return while the join is still running");

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    session.cancel().await.expect("cancel");

    // Bounded on purpose: if cancel stops working this fails in ten seconds with a
    // clear message instead of hanging the suite for a minute and a half.
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(10), cursor.next_batch(8))
        .await
        .expect("cancel did not stop the join within ten seconds");

    // MySQL reports an interrupted statement as 1317, "query execution was
    // interrupted". A killed process could not produce this: the server is what
    // stopped the work.
    match outcome {
        Err(error) => assert_eq!(
            error.code(),
            Some("1317"),
            "expected the interrupt code, got {error:?}"
        ),
        Ok(Some(batch)) => panic!(
            "the statement completed instead of being interrupted: {} row(s)",
            batch.rows()
        ),
        Ok(None) => panic!("the statement ended with no rows rather than being interrupted"),
    }

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

/// The development container offers TLS with the certificate MySQL generated for
/// itself when it first initialised — trustworthy to nobody, which is the shape of
/// the deployment this product actually meets.
///
/// The two TLS modes that mean different things are told apart here: `Prefer` uses
/// that certificate without checking who signed it, so an internal server keeps
/// working, and it does so *encrypted* — `Ssl_cipher` is the server's own account of
/// the session, not the client's silence about an error. `Require` refuses the same
/// certificate. This is the driver-level twin of `tests/tls.rs`, which runs the same
/// pair against a certificate this repository signed itself.
#[tokio::test]
async fn prefer_uses_a_certificate_nothing_signed_and_require_refuses_it() {
    let Some(config) = config() else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let mut session = MysqlDriver::new()
        .connect(&config.clone().tls(TlsMode::Prefer))
        .await
        .expect("an internal server with a self-signed certificate must still connect");
    let cipher = cipher_in_use(&mut session).await;
    assert!(
        !cipher.is_empty(),
        "Prefer connected to a server that offers TLS without encrypting"
    );
    let _ = session.close().await;

    let error = MysqlDriver::new()
        .connect(&config.clone().tls(TlsMode::Require))
        .await
        .err()
        .expect("Require verifies, and nothing signed this certificate");
    assert!(error.message().contains("certificate"), "{error:?}");
    assert_eq!(error.failure_kind(), FailureKind::Permanent, "{error:?}");

    // And the plaintext connection `Require` refused to fall back to, so the
    // refusal was the driver's choice rather than the container's.
    let session = MysqlDriver::new()
        .connect(&config)
        .await
        .expect("the container accepts plaintext, so the refusal above was a choice");
    let _ = session.close().await;
}

/// The cipher the server says the session is using, or the empty string when the
/// connection is in clear.
///
/// `SHOW STATUS LIKE 'Ssl_cipher'` returns `Variable_name`, `Value`: the second
/// column is the answer, and the first is the literal `Ssl_cipher` either way.
async fn cipher_in_use(session: &mut Box<dyn Session>) -> String {
    let mut cursor = session
        .execute("SHOW STATUS LIKE 'Ssl_cipher'", &ExecuteOptions::default())
        .await
        .expect("execute SHOW STATUS");
    let batch = cursor
        .next_batch(10)
        .await
        .expect("next_batch")
        .expect("SHOW STATUS returns a row");
    match batch.value(0, 1) {
        Some(Value::Text(value)) => value.to_string(),
        other => panic!("expected the variable's value, got {other:?}"),
    }
}

#[tokio::test]
async fn a_statement_timeout_is_a_typed_error_that_names_the_limit() {
    // MySQL's own bound is `max_execution_time`, which it enforces for a read-only
    // `SELECT`. `SELECT SLEEP(3)` is deliberately not used here: MySQL does not apply
    // the bound to it (measured), so the query below is a genuine scan — the
    // Cartesian product of the server's own column catalogue — which the server does
    // interrupt with error 3024.
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let started = Instant::now();
    let mut cursor = session
        .execute(
            "SELECT COUNT(*) FROM information_schema.columns a \
             CROSS JOIN information_schema.columns b \
             CROSS JOIN information_schema.columns c",
            &ExecuteOptions {
                statement_timeout: Some(Duration::from_millis(300)),
                ..ExecuteOptions::default()
            },
        )
        .await
        .expect("execute");
    let error = loop {
        match cursor.next_batch(10).await {
            Ok(Some(_)) => continue,
            Ok(None) => panic!("the scan finished instead of timing out"),
            Err(error) => break error,
        }
    };
    let elapsed = started.elapsed();

    match &error {
        EngineError::Timeout { message, limit_ms } => {
            assert_eq!(*limit_ms, Some(300));
            assert!(message.contains("300 ms"), "{message}");
            // The server's own sentence travels with it.
            assert!(
                message.contains("maximum statement execution time"),
                "{message}"
            );
        }
        other => panic!("expected a typed timeout, got {other:?}"),
    }
    assert!(
        elapsed < Duration::from_secs(5),
        "the timeout took {elapsed:?}"
    );
}

// --------------------------------------------------------------------------- //
// one session keeps one connection (W3-T1 commit F)
// --------------------------------------------------------------------------- //

/// Run one statement to the end and return its first row's first cell, if any.
async fn scalar(session: &mut Box<dyn Session>, sql: &str, options: &ExecuteOptions) -> Value {
    let mut cursor = session.execute(sql, options).await.expect("execute");
    let (rows, _) = drain(&mut cursor, 8).await;
    rows.first().map_or(Value::Null, |row| row[0].clone())
}

/// Run one statement that returns nothing, to the end.
async fn run(session: &mut Box<dyn Session>, sql: &str) {
    let mut cursor = session
        .execute(sql, &ExecuteOptions::default())
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error:?}"));
    drain(&mut cursor, 8).await;
}

#[tokio::test]
async fn every_statement_of_a_session_runs_on_the_same_connection() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let options = ExecuteOptions::default();
    let first = scalar(&mut session, "SELECT CONNECTION_ID()", &options).await;
    let second = scalar(&mut session, "SELECT CONNECTION_ID()", &options).await;
    let third = scalar(&mut session, "SELECT CONNECTION_ID()", &options).await;
    assert_ne!(first, Value::Null, "CONNECTION_ID() came back empty");
    assert_eq!(first, second, "the second statement ran elsewhere");
    assert_eq!(second, third, "the third statement ran elsewhere");
    session.close().await.expect("close");
}

#[tokio::test]
async fn a_rollback_undoes_what_the_same_sessions_transaction_wrote() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let table = "qh_w3t1_rollback";
    run(&mut session, &format!("DROP TABLE IF EXISTS {table}")).await;
    run(
        &mut session,
        &format!("CREATE TABLE {table} (id INT PRIMARY KEY) ENGINE=InnoDB"),
    )
    .await;

    run(&mut session, "BEGIN").await;
    run(&mut session, &format!("INSERT INTO {table} VALUES (1)")).await;
    run(&mut session, "ROLLBACK").await;

    let count = scalar(
        &mut session,
        &format!("SELECT COUNT(*) FROM {table}"),
        &ExecuteOptions::default(),
    )
    .await;
    run(&mut session, &format!("DROP TABLE {table}")).await;
    assert_eq!(count, Value::Int(0), "ROLLBACK undid nothing");
    session.close().await.expect("close");
}

#[tokio::test]
async fn a_failed_statement_leaves_the_transaction_open_for_the_rollback() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let table = "qh_w3t1_failed";
    run(&mut session, &format!("DROP TABLE IF EXISTS {table}")).await;
    run(
        &mut session,
        &format!("CREATE TABLE {table} (id INT PRIMARY KEY) ENGINE=InnoDB"),
    )
    .await;

    run(&mut session, "BEGIN").await;
    run(&mut session, &format!("INSERT INTO {table} VALUES (1)")).await;
    let duplicate = session
        .execute(
            &format!("INSERT INTO {table} VALUES (1)"),
            &ExecuteOptions::default(),
        )
        .await;
    assert!(duplicate.is_err(), "the duplicate key must be refused");
    drop(duplicate);
    run(&mut session, "ROLLBACK").await;

    let count = scalar(
        &mut session,
        &format!("SELECT COUNT(*) FROM {table}"),
        &ExecuteOptions::default(),
    )
    .await;
    run(&mut session, &format!("DROP TABLE {table}")).await;
    assert_eq!(
        count,
        Value::Int(0),
        "the first insert survived the rollback"
    );
    session.close().await.expect("close");
}

#[tokio::test]
async fn foreign_key_checks_set_in_one_statement_hold_for_the_next() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let (parent, child) = ("qh_w3t1_parent", "qh_w3t1_child");
    run(&mut session, &format!("DROP TABLE IF EXISTS {child}")).await;
    run(&mut session, &format!("DROP TABLE IF EXISTS {parent}")).await;
    run(
        &mut session,
        &format!("CREATE TABLE {parent} (id INT PRIMARY KEY) ENGINE=InnoDB"),
    )
    .await;
    run(
        &mut session,
        &format!(
            "CREATE TABLE {child} (id INT PRIMARY KEY, p INT, \
             FOREIGN KEY (p) REFERENCES {parent}(id)) ENGINE=InnoDB"
        ),
    )
    .await;

    run(&mut session, "SET FOREIGN_KEY_CHECKS = 0").await;
    // The orphan is what the import path writes when it loads children first.
    let orphan = session
        .execute(
            &format!("INSERT INTO {child} VALUES (1, 99)"),
            &ExecuteOptions::default(),
        )
        .await;
    let orphan_ok = orphan.is_ok();
    drop(orphan);
    run(&mut session, "SET FOREIGN_KEY_CHECKS = 1").await;

    run(&mut session, &format!("DROP TABLE IF EXISTS {child}")).await;
    run(&mut session, &format!("DROP TABLE IF EXISTS {parent}")).await;
    assert!(
        orphan_ok,
        "FOREIGN_KEY_CHECKS = 0 did not reach the next statement"
    );
    session.close().await.expect("close");
}

#[tokio::test]
async fn a_session_variable_survives_to_the_next_statement() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    run(&mut session, "SET @qh_w3t1 = 7").await;
    let value = scalar(&mut session, "SELECT @qh_w3t1", &ExecuteOptions::default()).await;
    assert_ne!(
        value,
        Value::Null,
        "the user variable was lost with its connection"
    );
    session.close().await.expect("close");
}

#[tokio::test]
async fn the_statement_timeout_still_holds_on_the_second_statement() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let options = ExecuteOptions {
        statement_timeout: Some(Duration::from_secs(5)),
        ..ExecuteOptions::default()
    };
    // The second statement does not send the `SET` again, because the session
    // believes the bound is already in force. It has to be right.
    scalar(&mut session, "SELECT 1", &options).await;
    let bound = scalar(&mut session, "SELECT @@max_execution_time", &options).await;
    assert_eq!(
        bound,
        Value::Int(5000),
        "the bound was lost with the connection"
    );
    session.close().await.expect("close");
}

#[tokio::test]
async fn a_session_still_answers_after_its_cursor_was_dropped_unread() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let cursor = session
        .execute(
            "SELECT a.id FROM wide_500k a JOIN wide_500k b ON a.id < b.id LIMIT 1000000",
            &ExecuteOptions::default(),
        )
        .await
        .expect("execute");
    session.cancel().await.expect("cancel");
    drop(cursor);
    let one = scalar(&mut session, "SELECT 1", &ExecuteOptions::default()).await;
    assert_eq!(one, Value::Int(1));
    session.close().await.expect("close");
}

const BIG_JOIN: &str = "SELECT a.id FROM wide_500k a JOIN wide_500k b ON a.id < b.id";
/// More rows than the producer's channel holds, and few enough to drain in a moment when
/// the connection is dropped mid-result (the drop reads the rest, unless a KILL came first).
const ABANDONED: &str = "SELECT id FROM wide_500k";

#[tokio::test]
async fn a_second_statement_on_a_live_cursor_is_refused_not_waited_for() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let cursor = session
        .execute(BIG_JOIN, &ExecuteOptions::default())
        .await
        .expect("execute");

    let asked = Instant::now();
    let second = session
        .execute("SELECT 1", &ExecuteOptions::default())
        .await;
    assert!(
        matches!(second, Err(EngineError::Usage { .. })),
        "expected a Usage error, got {:?}",
        second.map(|_| "a cursor")
    );
    assert!(
        asked.elapsed() < Duration::from_secs(1),
        "it waited {:?}",
        asked.elapsed()
    );

    // The refusal must not have cost the session its statement or its connection.
    session.cancel().await.expect("cancel");
    drop(cursor);
    let one = scalar(&mut session, "SELECT 1", &ExecuteOptions::default()).await;
    assert_eq!(one, Value::Int(1));
    session.close().await.expect("close");
}

#[tokio::test]
async fn a_lost_transaction_refuses_commit_and_accepts_rollback() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let table = "qh_w3t1_lost";
    run(&mut session, &format!("DROP TABLE IF EXISTS {table}")).await;
    run(
        &mut session,
        &format!("CREATE TABLE {table} (id INT PRIMARY KEY) ENGINE=InnoDB"),
    )
    .await;

    run(&mut session, "BEGIN").await;
    run(&mut session, &format!("INSERT INTO {table} VALUES (1)")).await;
    // Abandoned mid-stream: the connection cannot be offered back.
    let cursor = session
        .execute(ABANDONED, &ExecuteOptions::default())
        .await
        .expect("execute");
    drop(cursor);

    let commit = session.execute("COMMIT", &ExecuteOptions::default()).await;
    match commit {
        Err(EngineError::Query { kind, .. }) => assert_eq!(kind, FailureKind::Permanent),
        other => panic!(
            "COMMIT over a lost transaction must fail: {:?}",
            other.map(|_| "ok")
        ),
    }
    let select = session
        .execute("SELECT 1", &ExecuteOptions::default())
        .await;
    assert!(
        select.is_err(),
        "the session ran a statement on a fresh connection"
    );
    drop(select);

    run(&mut session, "ROLLBACK").await;
    let count = scalar(
        &mut session,
        &format!("SELECT COUNT(*) FROM {table}"),
        &ExecuteOptions::default(),
    )
    .await;
    run(&mut session, &format!("DROP TABLE {table}")).await;
    assert_eq!(
        count,
        Value::Int(0),
        "the lost transaction's row is in the table"
    );
    // ROLLBACK cleared the state.
    assert_eq!(
        scalar(&mut session, "SELECT 1", &ExecuteOptions::default()).await,
        Value::Int(1)
    );
    session.close().await.expect("close");
}

#[tokio::test]
async fn the_timeout_is_sent_again_when_a_dropped_statement_costs_the_connection() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let options = ExecuteOptions {
        statement_timeout: Some(Duration::from_secs(5)),
        ..ExecuteOptions::default()
    };
    scalar(&mut session, "SELECT 1", &options).await;
    let cursor = session.execute(ABANDONED, &options).await.expect("execute");
    drop(cursor);
    // A new connection starts at the server's 0; the session must not think 5000 is set.
    let bound = scalar(&mut session, "SELECT @@max_execution_time", &options).await;
    assert_eq!(bound, Value::Int(5000));
    session.close().await.expect("close");
}
