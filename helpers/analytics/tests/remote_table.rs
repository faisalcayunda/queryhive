//! A result in the app, queried here (blueprint sections 5.4, 14.4, 14.15).
//!
//! The "app" is `common::StoreSource`: a finished `qh-result-store` store whose chunks are
//! converted to the logical schema exactly as the app converts them before sending.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use crate::common::{collect, engine, texts, StoreSource};
use datafusion::arrow::datatypes::{DataType, TimeUnit};
use qh_analytics::remote_table::RemoteStoreTable;
use qh_analytics::session::Session;
use qh_analytics_proto::{ErrorKind, TableInfo};
use qh_core::{ColumnBatch, ColumnMeta, Value};
use qh_result_store::{logical_schema, Outcome, StoreConfig, StoreHandle, StoreRegistry};

fn finished(chunks: Vec<Vec<Vec<Value>>>, names: &[&str]) -> StoreHandle {
    let registry = StoreRegistry::for_test(StoreConfig {
        spill_dir: None,
        ..StoreConfig::default()
    });
    let handle = registry.create();
    let writer = handle.writer();
    writer
        .begin(names.iter().map(|n| ColumnMeta::new(*n, "text")).collect())
        .unwrap();
    // One push seals one chunk, so `chunks` decides how many the store has.
    for columns in chunks {
        writer.push(&ColumnBatch::new(columns).unwrap()).unwrap();
    }
    writer
        .finish(Outcome::Complete { truncated: false })
        .unwrap();
    handle
}

/// Register `store` as `r` in `session`, served by `source`.
fn register(session: &Session, store: &StoreHandle, source: Arc<StoreSource>) -> TableInfo {
    let table = RemoteStoreTable::new(
        source.schema.schema.clone(),
        u64::from(store.shared().rows()),
        source.chunks.len() as u32,
        source,
    );
    session
        .register("r", Arc::new(table), Some(u64::from(store.shared().rows())))
        .unwrap()
}

fn source_of(store: &StoreHandle) -> Arc<StoreSource> {
    let schema = logical_schema(&store.shared().columns(), &store.shared().stats());
    StoreSource::new(store, schema)
}

fn ts(micros: i64, offset: Option<i32>) -> Value {
    Value::Timestamp {
        micros,
        offset_secs: offset,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_kind_of_column_reaches_sql_with_the_grids_text() {
    // Two chunks, because a chunk is where the physical encoding is chosen: the offsets of
    // `at` are uniform inside each (so each is `tsz`) and differ between them.
    let store = finished(
        vec![
            vec![
                // Mixed variants across the result: the logical type is `Utf8`, and the
                // text is the grid's.
                vec![Value::Int(1), Value::Text("a".into())],
                vec![ts(1_700_000_000_000_000, Some(25_200)), ts(0, Some(25_200))],
                vec![ts(1, Some(25_200)), ts(2, Some(25_200))],
                vec![Value::Int(1), Value::Int(2)],
                vec![Value::Null; 2],
                vec![Value::Bytes(vec![0xde, 0xad]), Value::Bytes(vec![])],
            ],
            vec![
                vec![Value::Float(2.5), Value::Null],
                vec![ts(1_700_000_100_000_000, Some(-10_800)), Value::Null],
                vec![ts(3, Some(25_200)), Value::Null],
                vec![Value::Int(3), Value::Int(4)],
                vec![Value::Null; 2],
                vec![Value::Null, Value::Bytes(vec![1])],
            ],
        ],
        &["mixed", "at", "fixed_at", "n", "blank", "raw"],
    );
    let (engine, _dir) = engine(2);
    let session = engine.session();
    let source = source_of(&store);
    let info = register(&session, &store, source.clone());
    let type_of = |name: &str| {
        info.columns
            .iter()
            .find(|c| c.name == name)
            .unwrap()
            .type_name
            .clone()
    };
    assert_eq!(type_of("mixed"), DataType::Utf8.to_string());
    assert_eq!(
        type_of("at"),
        DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())).to_string()
    );
    assert_eq!(
        type_of("fixed_at"),
        DataType::Timestamp(TimeUnit::Microsecond, Some("+07:00".into())).to_string()
    );
    assert_eq!(type_of("n"), DataType::Int64.to_string());
    assert_eq!(info.rows, Some(4));

    let _lease = engine.grant(64 << 20);
    let rows = texts(
        &collect(&session, "SELECT mixed FROM r ORDER BY n")
            .await
            .unwrap(),
    );
    let expected: Vec<Vec<Option<String>>> = [Some("1"), Some("a"), Some("2.5"), None]
        .iter()
        .map(|cell| vec![cell.map(str::to_owned)])
        .collect();
    assert_eq!(rows, expected);

    // Three of the four instants are there, whatever their zone.
    let rows = texts(&collect(&session, "SELECT count(at) FROM r").await.unwrap());
    assert_eq!(rows, vec![vec![Some("3".to_owned())]]);
    let rows = texts(
        &collect(&session, "SELECT sum(n), count(blank), count(*) FROM r")
            .await
            .unwrap(),
    );
    assert_eq!(
        rows,
        vec![vec![
            Some("10".to_owned()),
            Some("0".to_owned()),
            Some("4".to_owned())
        ]]
    );
    let rows = texts(
        &collect(
            &session,
            "SELECT raw FROM r WHERE raw IS NOT NULL ORDER BY n",
        )
        .await
        .unwrap(),
    );
    assert_eq!(rows.len(), 3);
}

/// The instant must not move when a zone is relabelled. It does today: `logical.rs`
/// `shift_timestamps` adds the source offset to the micros, so a `tsz` chunk at +07:00
/// shows up in SQL seven hours late, and `CAST(at AS BIGINT)` is wrong. (Arrow timestamps
/// are UTC instants; the zone is only a label, section 5.4 says "relabel".) A chunk that is
/// `tagged` because its offsets differ *within* the chunk does not convert at all: it falls
/// through to `Utf8` and the batch is refused. Both are in `qh-result-store`, which the app
/// side of W13-T8b owns; this test is here for that task to switch on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "qh-result-store logical.rs shifts instants when it relabels a zone (W13-T8b)"]
async fn a_zone_relabel_keeps_the_instant() {
    let store = finished(
        vec![
            vec![
                vec![ts(1_700_000_000_000_000, Some(25_200)), ts(0, Some(25_200))],
                vec![Value::Int(1), Value::Int(2)],
            ],
            vec![
                vec![ts(1_700_000_100_000_000, Some(-10_800)), Value::Null],
                vec![Value::Int(3), Value::Int(4)],
            ],
        ],
        &["at", "n"],
    );
    let (engine, _dir) = engine(2);
    let session = engine.session();
    register(&session, &store, source_of(&store));
    let _lease = engine.grant(64 << 20);
    let rows = texts(
        &collect(&session, "SELECT CAST(at AS BIGINT) FROM r ORDER BY n")
            .await
            .unwrap(),
    );
    assert_eq!(
        rows,
        vec![
            vec![Some("1700000000000000".to_owned())],
            vec![Some("0".to_owned())],
            vec![Some("1700000100000000".to_owned())],
            vec![None],
        ]
    );
}

fn many_chunks(chunks: usize, rows: usize) -> StoreHandle {
    let batches = (0..chunks)
        .map(|chunk| {
            vec![
                (0..rows)
                    .map(|row| Value::Int((chunk * rows + row) as i64))
                    .collect(),
                (0..rows)
                    .map(|row| Value::Text(format!("c{chunk}r{row}").into()))
                    .collect(),
            ]
        })
        .collect();
    finished(batches, &["k", "s"])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_partition_keeps_at_most_two_requests_open() {
    let store = many_chunks(24, 100);
    let (engine, _dir) = engine(3);
    let session = engine.session();
    let schema = logical_schema(&store.shared().columns(), &store.shared().stats());
    let source = StoreSource::with_delay(&store, schema, Duration::from_millis(15));
    register(&session, &store, source.clone());
    let _lease = engine.grant(64 << 20);

    let rows = texts(
        &collect(&session, "SELECT sum(k), count(*) FROM r")
            .await
            .unwrap(),
    );
    assert_eq!(rows[0][1], Some("2400".to_owned()));
    let peak = source.peak.load(Ordering::SeqCst);
    // 3 partitions x 2 credits. It must also be more than 2, or the test proves nothing
    // about partitions running at the same time.
    assert!(peak <= 6, "{peak} requests were open at once");
    assert!(peak >= 3, "the partitions never overlapped (peak {peak})");
    assert_eq!(
        source.requests.load(Ordering::SeqCst),
        24,
        "every chunk exactly once"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_limit_does_not_pull_the_whole_result() {
    let store = many_chunks(40, 100);
    let (engine, _dir) = engine(2);
    let session = engine.session();
    let source = source_of(&store);
    register(&session, &store, source.clone());
    let _lease = engine.grant(64 << 20);

    let rows = texts(&collect(&session, "SELECT k FROM r LIMIT 5").await.unwrap());
    assert_eq!(rows.len(), 5);
    let requested = source.requests.load(Ordering::SeqCst);
    assert!(requested < 40, "LIMIT 5 asked for {requested} of 40 chunks");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_the_columns_a_query_names_are_requested() {
    let store = many_chunks(4, 10);
    let (engine, _dir) = engine(2);
    let session = engine.session();
    let source = source_of(&store);
    register(&session, &store, source.clone());
    let _lease = engine.grant(64 << 20);

    collect(&session, "SELECT s FROM r").await.unwrap();
    {
        let asked = source.asked.lock().unwrap();
        assert!(!asked.is_empty());
        assert!(asked.iter().all(|columns| columns == &[1]), "{asked:?}");
    }
    // A scan with no columns at all still counts rows: it asks for one and drops it.
    source.asked.lock().unwrap().clear();
    let rows = texts(&collect(&session, "SELECT 1 FROM r").await.unwrap());
    assert_eq!(rows.len(), 40);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_closed_result_is_a_query_error_and_not_a_panic() {
    let store = many_chunks(6, 10);
    let (engine, _dir) = engine(2);
    let session = engine.session();
    let source = source_of(&store);
    register(&session, &store, source.clone());
    let _lease = engine.grant(64 << 20);

    *source.closed.lock().unwrap() = Some("the result was closed".to_owned());
    let error = collect(&session, "SELECT sum(k) FROM r").await.unwrap_err();
    assert_eq!(error.0.kind, ErrorKind::InvalidArgument);
    assert!(
        error.0.message.contains("the result was closed"),
        "{}",
        error.0.message
    );
    // The session is still usable afterwards.
    *source.closed.lock().unwrap() = None;
    let rows = texts(&collect(&session, "SELECT count(*) FROM r").await.unwrap());
    assert_eq!(rows, vec![vec![Some("60".to_owned())]]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_chunk_that_disagrees_with_the_schema_is_refused() {
    let store = many_chunks(2, 10);
    let other = finished(
        vec![vec![
            vec![Value::Text("x".into())],
            vec![Value::Text("y".into())],
        ]],
        &["k", "s"],
    );
    let (engine, _dir) = engine(2);
    let session = engine.session();
    // Registered as the integer table, but the "app" answers from the text one.
    let schema = logical_schema(&store.shared().columns(), &store.shared().stats());
    let wrong = StoreSource::new(
        &other,
        logical_schema(&other.shared().columns(), &other.shared().stats()),
    );
    let table = RemoteStoreTable::new(schema.schema.clone(), 20, 1, wrong);
    session.register("r", Arc::new(table), Some(20)).unwrap();
    let _lease = engine.grant(64 << 20);

    let error = collect(&session, "SELECT k FROM r").await.unwrap_err();
    assert!(
        error.0.message.contains("registered as"),
        "{}",
        error.0.message
    );
}
