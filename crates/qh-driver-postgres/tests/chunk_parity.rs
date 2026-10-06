//! W7-T1: `value_at(next_chunk)` equals `next_batch`, cell by cell, against a live server.
//!
//! ```bash
//! QH_TEST_POSTGRES=1 cargo test -p qh-driver-postgres --test chunk_parity
//! ```
//!
//! Without `QH_TEST_POSTGRES=1` each test prints why it is skipping and returns.

use qh_core::Value;
use qh_driver::qh_columnar::value_at;
use qh_driver::{
    ChunkBuilder, ConnectionConfig, Driver, DriverKind, ExecuteOptions, Session, TlsMode,
};
use qh_driver_postgres::PostgresDriver;

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

const SKIP_HINT: &str = "skipped: set QH_TEST_POSTGRES=1 with deploy/dev/up.sh running";

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
        .tls(TlsMode::Disable),
    )
}

async fn connect() -> Option<Box<dyn Session>> {
    let config = config()?;
    Some(
        PostgresDriver::new()
            .connect(&config)
            .await
            .expect("connect"),
    )
}

#[tokio::test]
async fn type_zoo_chunks_equal_batches() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    assert_parity(&mut session, "SELECT * FROM type_zoo", 1000).await;
}

#[tokio::test]
async fn literal_zoo_chunks_equal_batches() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    assert_parity(&mut session, "SELECT 1::int2 a, 2::int8 b, 1.5::float4 c, 'NaN'::float8 d, 'Infinity'::float8 e, true f, NULL::int g, 12.340::numeric(10,3) h, 'NaN'::numeric i, '\\x00ff'::bytea j, '{\"a\": 1}'::jsonb k, '{1,NULL,3}'::int[] l, '2026-01-31 12:00:00.123456+07'::timestamptz m, '2026-01-31 12:00:00.123456'::timestamp n, '2026-01-31'::date o, '23:59:59.999999'::time p, interval '1 year 2 mons 3 days 04:05:06' q, 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11'::uuid r", 1000).await;
}

#[tokio::test]
async fn many_rows_span_chunks() {
    let Some(mut session) = connect().await else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    assert_parity(
        &mut session,
        "SELECT * FROM wide_500k ORDER BY id LIMIT 40000",
        16384,
    )
    .await;
    assert_parity(
        &mut session,
        "SELECT g, g::text, g::numeric / 7 FROM generate_series(1, 5000) g",
        800,
    )
    .await;
    assert_parity(&mut session, "SELECT CASE WHEN g % 3 = 0 THEN 'NaN'::numeric ELSE g::numeric END FROM generate_series(1, 3000) g", 1000).await;
}

/// No server: every PostgreSQL type the zoo holds, as the text the wire carries, through
/// `ColumnParser::push` and through `from_text`. Includes texts that do not parse, which must
/// keep their text and tag the chunk-column rather than change a cell.
#[test]
fn column_parser_pushes_what_from_text_returns() {
    use qh_driver_postgres::normalize::{from_text, ColumnParser};

    let zoo: &[(&str, &[Option<&str>])] = &[
        (
            "bool",
            &[Some("t"), Some("f"), Some("true"), Some("maybe"), None],
        ),
        (
            "int4",
            &[Some("1"), Some("-9223372036854775808"), Some("x"), None],
        ),
        ("oid", &[Some("42")]),
        (
            "float8",
            &[
                Some("1.5"),
                Some("NaN"),
                Some("Infinity"),
                Some("-Infinity"),
                Some("?"),
            ],
        ),
        (
            "numeric(38,10)",
            &[
                Some("1234567890123456789012345678.1234567890"),
                Some("-0.0000000001"),
                Some("NaN"),
                Some("42"),
                Some("1e3"),
                None,
            ],
        ),
        (
            "bytea",
            &[Some("\\x00ff"), Some("\\xzz"), Some("\\001\\377")],
        ),
        ("date", &[Some("2026-01-31"), Some("infinity"), None]),
        ("time", &[Some("23:59:59.999999"), Some("nope")]),
        (
            "timestamp",
            &[Some("2026-01-31 12:00:00.123456"), Some("-infinity")],
        ),
        (
            "timestamp(6) with time zone",
            &[
                Some("2026-01-31 12:00:00.123456+07"),
                Some("2026-01-31 12:00:00+05:30:15"),
                Some("junk"),
            ],
        ),
        (
            "interval",
            &[Some("1 year 2 mons 3 days 04:05:06"), Some("@ nonsense")],
        ),
        ("jsonb", &[Some("{\"a\": 1}"), Some("")]),
        ("uuid", &[Some("a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11")]),
        ("int4[]", &[Some("{1,NULL,3}")]),
        ("text", &[Some(""), Some("NULL"), Some("x")]),
    ];
    for (type_name, cells) in zoo {
        let parser = ColumnParser::new(type_name);
        let mut builder = ChunkBuilder::new(1);
        for cell in *cells {
            parser.push(&mut builder, 0, *cell);
        }
        let sealed = builder.seal().expect("seal");
        for (row, cell) in cells.iter().enumerate() {
            let got = value_at(sealed.batch.column(0).as_ref(), sealed.encodings[0], row)
                .expect("value_at");
            assert_eq!(
                format!("{got:?}"),
                format!("{:?}", from_text(type_name, *cell)),
                "{type_name} {cell:?}"
            );
        }
    }
}
