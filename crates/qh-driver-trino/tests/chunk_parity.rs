//! W7-T1: `value_at(next_chunk)` equals `next_batch`, cell by cell, against a live server.
//!
//! ```bash
//! QH_TEST_TRINO=1 cargo test -p qh-driver-trino --test chunk_parity
//! ```
//!
//! Without `QH_TEST_TRINO=1` each test prints why it is skipping and returns.

use qh_core::Value;
use qh_driver::qh_columnar::value_at;
use qh_driver::{
    ChunkBuilder, ConnectionConfig, Driver, DriverKind, ExecuteOptions, Session, TlsMode,
};
use qh_driver_trino::TrinoDriver;

/// Every cell of a statement through `next_batch`, row-major, as `Debug` text (so NaN
/// compares equal to itself and the variant is part of the comparison).
async fn via_batches(session: &mut Box<dyn Session>, sql: &str) -> Vec<Vec<String>> {
    let mut cursor = session
        .execute(sql, &ExecuteOptions::default())
        .await
        .unwrap_or_else(|error| panic!("execute {sql}: {error:?}"));
    let mut rows = Vec::new();
    while let Some(batch) = cursor.next_batch(1000).await.expect("next_batch") {
        for row in 0..batch.rows() {
            rows.push(
                (0..batch.width())
                    .map(|column| format!("{:?}", batch.value(row, column).expect("cell")))
                    .collect(),
            );
        }
    }
    rows
}

/// The same statement the way the store path reads it: the first fetch through
/// `next_batch` (what the pump primes with, and a Trino cursor only knows its columns after
/// it), then `next_chunk` into fresh builders until it returns 0.
async fn via_chunks(session: &mut Box<dyn Session>, sql: &str, fetch: usize) -> Vec<Vec<String>> {
    let mut cursor = session
        .execute(sql, &ExecuteOptions::default())
        .await
        .unwrap_or_else(|error| panic!("execute {sql}: {error:?}"));
    let mut sealed = Vec::new();
    if let Some(primed) = cursor.next_batch(fetch).await.expect("primed") {
        let mut builder = ChunkBuilder::new(primed.width());
        builder.push_owned(primed).expect("primed rows");
        sealed.push(builder.seal().expect("seal"));
    }
    if !cursor.columns().is_empty() {
        loop {
            let mut builder = ChunkBuilder::new(cursor.columns().len());
            let appended = cursor
                .next_chunk(&mut builder, fetch)
                .await
                .expect("next_chunk");
            if appended == 0 {
                break;
            }
            assert_eq!(builder.rows(), appended);
            sealed.push(builder.seal().expect("seal"));
        }
    }
    let mut rows = Vec::new();
    for chunk in &sealed {
        for row in 0..chunk.batch.num_rows() {
            rows.push(
                (0..chunk.batch.num_columns())
                    .map(|column| {
                        let value: Value = value_at(
                            chunk.batch.column(column).as_ref(),
                            chunk.encodings[column],
                            row,
                        )
                        .expect("value_at");
                        format!("{value:?}")
                    })
                    .collect(),
            );
        }
    }
    rows
}

async fn assert_parity(session: &mut Box<dyn Session>, sql: &str, fetch: usize) {
    let batches = via_batches(session, sql).await;
    let chunks = via_chunks(session, sql, fetch).await;
    assert!(!batches.is_empty(), "{sql} returned no rows");
    assert_eq!(
        batches.len(),
        chunks.len(),
        "{sql} (fetch {fetch}): row count"
    );
    for (row, (a, b)) in batches.iter().zip(&chunks).enumerate() {
        assert_eq!(a, b, "{sql} (fetch {fetch}): row {row}");
    }
}

const SKIP_HINT: &str = "skipped: set QH_TEST_TRINO=1 with deploy/dev/up.sh running";

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

#[tokio::test]
async fn literal_zoo_chunks_equal_batches() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    assert_parity(&mut session, "SELECT CAST('1234567890123456789012345678.1234567890' AS decimal(38,10)) a, CAST('-0.0000000001' AS decimal(38,10)) b, TIMESTAMP '2026-01-31 12:00:00.123456 +07:00' c, TIMESTAMP '2026-01-31 12:00:00.123456' d, DATE '2026-01-31' e, TIME '23:59:59.999999' f, INTERVAL '3 4:05:06' DAY TO SECOND g, CAST(NULL AS varchar) h, '' i, ARRAY[1, 2, NULL] j, CAST(ROW(1, 'x') AS ROW(a integer, b varchar)) k, MAP(ARRAY['a'], ARRAY[1]) l, X'00ff' m, nan() n, infinity() o, true p, CAST(7 AS tinyint) q, CAST(1.5 AS real) r, JSON '{\"a\": 1}' s, UUID '12151fd2-7586-11e9-8f9e-2a86e4085a59' t", 1000).await;
}

#[tokio::test]
async fn many_rows_span_pages_and_chunks() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    assert_parity(
        &mut session,
        "SELECT * FROM tpch.tiny.lineitem ORDER BY orderkey, linenumber",
        16384,
    )
    .await;
    assert_parity(
        &mut session,
        "SELECT * FROM tpch.tiny.orders ORDER BY orderkey",
        500,
    )
    .await;
    assert_parity(
        &mut session,
        "SELECT * FROM tpch.tiny.nation ORDER BY nationkey",
        1000,
    )
    .await;
}
