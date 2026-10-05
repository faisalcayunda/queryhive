//! `RESULT_SINK=store` and `ResultHandle` (W5-T2, blueprint `fase-6-data-plane.md` sections 12,
//! 16 and 20).
//!
//! Everything here runs against a fake connector, so no server is needed. The load-bearing test
//! is `the_store_holds_what_the_ndjson_path_sent`: the same scripted results go through
//! `EngineHost::run` (NDJSON `rows` events) and through `run_with_store`, and `rows_text` must
//! equal the NDJSON `data` array cell by cell. That is what keeps the renderer single.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use async_trait::async_trait;
use qh_core::{ColumnBatch, ColumnMeta, EngineError, IntervalValue, Value};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions,
    ObjectPath, ObjectsPage, Session,
};
use qh_ffi::host::{Connector, EngineHost, TunnelHandle};
use qh_ffi::store_api::{
    CellFormat, ColumnWire, FilterSpec, ResultHandle, SortSpec, StoreFfiError, StorePhase, ViewSpec,
};
use qh_ffi::uniffi_api::{EngineCommand, EventSink, RunCancel, Setting};
use qh_ffi::{run, CancelFlag, Capture, Command, Engine, Settings, StoreEmitter};
use qh_result_store::FaultyMedium;
use serde_json::Value as Json;

// --------------------------------------------------------------------------- //
// the fake server
// --------------------------------------------------------------------------- //

/// What the next `execute` hands out.
#[derive(Clone, Default)]
struct Script {
    columns: Vec<ColumnMeta>,
    batches: Vec<ColumnBatch>,
    /// Called with the number of batches served so far, after each one.
    hook: Option<Arc<dyn Fn(usize) + Send + Sync>>,
    /// Panic when this many batches have been served.
    panic_at: Option<usize>,
}

struct World {
    script: Mutex<Script>,
}

struct FakeConnector(Arc<World>);

fn real_driver(kind: DriverKind) -> &'static dyn Driver {
    static TRINO: qh_driver_trino::TrinoDriver = qh_driver_trino::TrinoDriver;
    static POSTGRES: qh_driver_postgres::PostgresDriver = qh_driver_postgres::PostgresDriver;
    static MYSQL: qh_driver_mysql::MysqlDriver = qh_driver_mysql::MysqlDriver;
    match kind {
        DriverKind::Trino => &TRINO,
        DriverKind::Postgres => &POSTGRES,
        DriverKind::Mysql => &MYSQL,
    }
}

#[async_trait]
impl Connector for FakeConnector {
    fn driver(&self, kind: DriverKind) -> &dyn Driver {
        real_driver(kind)
    }

    async fn open_tunnel(
        &self,
        _config: &ConnectionConfig,
        _settings: &qh_ffi::Settings,
    ) -> Result<Arc<dyn TunnelHandle>, EngineError> {
        unreachable!("no test here uses a tunnel")
    }

    async fn connect(
        &self,
        config: &ConnectionConfig,
        _tunnel: Option<&Arc<dyn TunnelHandle>>,
    ) -> Result<Box<dyn Session>, EngineError> {
        Ok(Box::new(FakeSession {
            world: Arc::clone(&self.0),
            kind: config.kind,
        }))
    }
}

struct FakeSession {
    world: Arc<World>,
    kind: DriverKind,
}

#[async_trait]
impl Session for FakeSession {
    fn capabilities(&self) -> Capabilities {
        real_driver(self.kind).capabilities()
    }

    fn query_id(&self) -> Option<String> {
        None
    }

    async fn execute(
        &mut self,
        _sql: &str,
        _options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        let script = self.world.script.lock().unwrap().clone();
        Ok(Box::new(FakeCursor { script, served: 0 }))
    }

    async fn browse(
        &mut self,
        _level: BrowseLevel,
        _path: &ObjectPath,
        _include_system: bool,
    ) -> Result<Vec<String>, EngineError> {
        Ok(Vec::new())
    }

    async fn objects(&mut self, _path: &ObjectPath) -> Result<ObjectsPage, EngineError> {
        Ok(ObjectsPage {
            columns: Vec::new(),
            rows: Vec::new(),
        })
    }

    fn explain_statement(&self, sql: &str) -> String {
        format!("EXPLAIN {sql}")
    }

    async fn cancel(&self) -> Result<(), EngineError> {
        Ok(())
    }

    async fn close(self: Box<Self>) -> Result<(), EngineError> {
        Ok(())
    }
}

struct FakeCursor {
    script: Script,
    served: usize,
}

#[async_trait]
impl Cursor for FakeCursor {
    fn columns(&self) -> &[ColumnMeta] {
        &self.script.columns
    }

    async fn next_batch(&mut self, _max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        if self.script.panic_at == Some(self.served) {
            panic!("the fake cursor panicked on purpose");
        }
        if self.served >= self.script.batches.len() {
            return Ok(None);
        }
        let batch = self.script.batches[self.served].clone();
        self.served += 1;
        if let Some(hook) = &self.script.hook {
            hook(self.served);
        }
        Ok(Some(batch))
    }
}

/// A host over the fake connector, with result stores configured (no spill unless `spill`).
fn host(script: Script, budget: u64, spill: Option<&std::path::Path>) -> Arc<EngineHost> {
    let world = Arc::new(World {
        script: Mutex::new(script),
    });
    let host = EngineHost::with_connector(Arc::new(FakeConnector(world)));
    host.configure_result_stores(spill.map(|dir| dir.to_string_lossy().into_owned()), budget)
        .expect("configure");
    host
}

// --------------------------------------------------------------------------- //
// helpers
// --------------------------------------------------------------------------- //

/// Every event with the instant it arrived.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<(Instant, Json)>>>);

impl Recorder {
    fn events(&self) -> Vec<Json> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|(_, e)| e.clone())
            .collect()
    }

    fn timed(&self) -> Vec<(Instant, Json)> {
        self.0.lock().unwrap().clone()
    }
}

impl EventSink for Recorder {
    fn on_event(&self, line: String) {
        self.0
            .lock()
            .unwrap()
            .push((Instant::now(), serde_json::from_str(&line).expect("JSON")));
    }
}

fn pairs(sql: &str, limit: Option<&str>) -> Vec<Setting> {
    let mut pairs = vec![
        ("DB_KIND", "postgres"),
        ("DB_HOST", "db.example"),
        ("DB_PORT", "5432"),
        ("DB_USER", "u"),
        ("DB_PASSWORD", "p"),
        ("DB_DATABASE", "d"),
        ("DB_SSLMODE", "disable"),
        ("RETRIES", "0"),
        ("SQL", sql),
    ];
    if let Some(limit) = limit {
        pairs.push(("LIMIT", limit));
    }
    pairs
        .into_iter()
        .map(|(key, value)| Setting {
            key: key.to_owned(),
            value: value.to_owned(),
        })
        .collect()
}

fn ndjson(host: &EngineHost, limit: Option<&str>) -> Vec<Json> {
    let sink = Recorder::default();
    host.run(
        EngineCommand::Preview,
        pairs("SELECT 1", limit),
        Arc::new(sink.clone()),
        RunCancel::new(),
    );
    sink.events()
}

fn into_store(host: &EngineHost, limit: Option<&str>) -> (Arc<ResultHandle>, Recorder) {
    let store = host.create_result_store().expect("a store");
    let sink = Recorder::default();
    host.run_with_store(
        EngineCommand::Preview,
        pairs("SELECT 1", limit),
        Arc::clone(&store),
        Arc::new(sink.clone()),
        RunCancel::new(),
    );
    (store, sink)
}

fn last<'a>(events: &'a [Json], name: &str) -> &'a Json {
    events
        .iter()
        .rev()
        .find(|event| event["event"] == name)
        .unwrap_or_else(|| panic!("no `{name}` event in {events:?}"))
}

/// A decoded `QHW1` buffer.
struct Qhw {
    flags: u16,
    first_row: u32,
    rows: usize,
    visible_total: u32,
    source_rows: Vec<u32>,
    cells: Vec<(Option<String>, u8)>,
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}

fn decode(data: &[u8]) -> Qhw {
    assert_eq!(&data[0..4], b"QHW1");
    assert_eq!(u16::from_le_bytes([data[4], data[5]]), 1);
    let rows = u32_at(data, 12) as usize;
    let columns = u32_at(data, 16) as usize;
    let heap_len = u32_at(data, 24) as usize;
    let s0 = 32 + 4 * rows;
    let s1 = s0 + 4 * (rows * columns + 1);
    let s2 = s1 + rows * columns;
    assert_eq!(data.len(), s2 + heap_len, "the total length is S2 + H");
    let offsets: Vec<usize> = (0..=rows * columns)
        .map(|k| u32_at(data, s0 + 4 * k) as usize)
        .collect();
    assert!(
        offsets.windows(2).all(|w| w[0] <= w[1]),
        "offsets are monotone"
    );
    assert!(*offsets.last().unwrap() <= heap_len);
    let cells = (0..rows * columns)
        .map(|k| {
            let flag = data[s1 + k];
            let text = String::from_utf8(data[s2 + offsets[k]..s2 + offsets[k + 1]].to_vec())
                .expect("heap is UTF-8");
            // bit 0 is NULL
            ((flag & 1 == 0).then_some(text), flag)
        })
        .collect();
    Qhw {
        flags: u16::from_le_bytes([data[6], data[7]]),
        first_row: u32_at(data, 8),
        rows,
        visible_total: u32_at(data, 20),
        source_rows: (0..rows).map(|r| u32_at(data, 32 + 4 * r)).collect(),
        cells,
    }
}

// --------------------------------------------------------------------------- //
// the scripted results
// --------------------------------------------------------------------------- //

/// Every `Value` variant, one per column, over three rows.
fn zoo() -> (Vec<ColumnMeta>, Vec<Vec<Value>>) {
    let columns = [
        ("a_decimal", "decimal(38,10)"),
        ("a_tstz", "timestamp with time zone"),
        ("a_ts", "timestamp"),
        ("a_date", "date"),
        ("a_time", "time"),
        ("an_interval", "interval"),
        ("a_bool", "boolean"),
        ("a_bigint", "bigint"),
        ("a_ubigint", "bigint unsigned"),
        ("a_double", "double"),
        ("a_text", "text"),
        ("a_json", "json"),
        ("some_bytes", "bytea"),
        ("an_array", "array"),
        ("a_row", "row"),
        ("a_map", "map"),
        ("unknown", "inet"),
    ]
    .iter()
    .map(|(name, type_name)| ColumnMeta::new(*name, *type_name))
    .collect();
    let row = |n: i64| -> Vec<Value> {
        vec![
            Value::Decimal {
                unscaled: 12_345_678_901_234_567_890 * i128::from(n),
                scale: 10,
            },
            Value::Timestamp {
                micros: 1_769_835_600_123_456 + n,
                offset_secs: Some(25_200),
            },
            Value::Timestamp {
                micros: 1_769_860_800_000_000,
                offset_secs: None,
            },
            Value::Date {
                days: 20_484 + n as i32,
            },
            Value::Time {
                micros: 86_399_999_999,
            },
            Value::Interval(IntervalValue {
                months: 1,
                days: 3,
                micros: 14_706_000_000,
            }),
            Value::Bool(n % 2 == 0),
            Value::Int(i64::MIN + n),
            Value::UInt(u64::MAX - n as u64),
            Value::Float(1.5 * n as f64),
            Value::Text(format!("unicode: \u{e9}\u{4e2d}\u{1f600} {n}\ttab\nline").into()),
            Value::Json(r#"{"b":1,"a":[1,2]}"#.into()),
            Value::Bytes(vec![0x00, 0x01, 0xff, n as u8]),
            Value::Array(vec![Value::Int(n), Value::Null, Value::Text("x".into())]),
            Value::Row(vec![Value::Int(n), Value::Text("r".into())]),
            Value::Map(vec![(Value::Text("k".into()), Value::Int(n))]),
            Value::unknown("inet", "10.0.0.1"),
        ]
    };
    let mut rows = vec![row(1), row(2), row(3)];
    // A NULL in every column of one row, and the empty string and the string "NULL" in a text one.
    rows.push(vec![Value::Null; 17]);
    rows[0][10] = Value::Text("".into());
    rows[1][10] = Value::Text("NULL".into());
    (columns, rows)
}

fn batch_of(rows: &[Vec<Value>]) -> ColumnBatch {
    let width = rows[0].len();
    ColumnBatch::new(
        (0..width)
            .map(|c| rows.iter().map(|row| row[c].clone()).collect())
            .collect(),
    )
    .expect("a rectangular batch")
}

fn batches_of(rows: &[Vec<Value>], size: usize) -> Vec<ColumnBatch> {
    rows.chunks(size).map(batch_of).collect()
}

/// `n` rows of three columns (int, text, nullable text), for batching and cap cases.
fn numbered(n: usize) -> (Vec<ColumnMeta>, Vec<Vec<Value>>) {
    let columns = vec![
        ColumnMeta::new("n", "bigint"),
        ColumnMeta::new("label", "text"),
        ColumnMeta::new("maybe", "text"),
    ];
    let rows = (0..n)
        .map(|i| {
            vec![
                Value::Int(i as i64),
                Value::Text(format!("row {i}").into()),
                if i % 5 == 0 {
                    Value::Null
                } else {
                    Value::Text(format!("m{i}").into())
                },
            ]
        })
        .collect();
    (columns, rows)
}

fn script(columns: Vec<ColumnMeta>, rows: &[Vec<Value>], batch: usize) -> Script {
    Script {
        columns,
        batches: if rows.is_empty() {
            Vec::new()
        } else {
            batches_of(rows, batch)
        },
        ..Script::default()
    }
}

/// The NDJSON `data` arrays, concatenated.
fn ndjson_rows(events: &[Json]) -> Vec<Vec<Json>> {
    events
        .iter()
        .filter(|event| event["event"] == "rows")
        .flat_map(|event| event["data"].as_array().unwrap().clone())
        .map(|row| row.as_array().unwrap().clone())
        .collect()
}

// --------------------------------------------------------------------------- //
// the equivalence: one renderer
// --------------------------------------------------------------------------- //

fn assert_same_cells(events: &[Json], store: &ResultHandle) {
    let expected = ndjson_rows(events);
    let count = store.row_count().unwrap();
    assert_eq!(count.fetched as usize, expected.len(), "row count");
    let width = store.columns().unwrap().len();
    let all: Vec<u32> = (0..width as u32).collect();
    let text = decode(&store.rows_text(0, 0, count.fetched, all).unwrap());
    assert_eq!(text.rows, expected.len());
    assert_eq!(text.flags & 1, 0, "rows_text is not cut");
    for (r, row) in expected.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            let (got, flag) = &text.cells[r * width + c];
            assert_eq!(
                got.as_deref(),
                cell.as_str(),
                "row {r} column {c} (flag {flag:#b}, expected {cell:?}): the store and the NDJSON path disagree"
            );
        }
    }
}

#[test]
fn the_store_holds_what_the_ndjson_path_sent() {
    let (zoo_columns, zoo_rows) = zoo();
    let (numbered_columns, numbered_rows) = numbered(450);
    let cases: Vec<(&str, Script, Option<&str>)> = vec![
        (
            "type zoo, one batch",
            script(zoo_columns.clone(), &zoo_rows, 4),
            None,
        ),
        // One row per batch seals one chunk per row, so every chunk-column is its own encoding.
        (
            "type zoo, one row per batch",
            script(zoo_columns, &zoo_rows, 1),
            None,
        ),
        (
            "450 rows in 200-row batches",
            script(numbered_columns.clone(), &numbered_rows, 200),
            Some("1000"),
        ),
        (
            "a cap inside a batch",
            script(numbered_columns.clone(), &numbered_rows, 200),
            Some("250"),
        ),
        (
            "a cap on a batch boundary",
            script(numbered_columns.clone(), &numbered_rows, 200),
            Some("400"),
        ),
        (
            "a cap that is exactly the result",
            script(numbered_columns.clone(), &numbered_rows, 200),
            Some("450"),
        ),
        ("no rows", script(numbered_columns, &[], 200), None),
    ];
    for (name, script, limit) in cases {
        let host = host(script, 256 * 1024 * 1024, None);
        let events = ndjson(&host, limit);
        assert!(
            events.iter().all(|e| e["event"] != "error"),
            "{name}: {events:?}"
        );
        let (store, sink) = into_store(&host, limit);
        let stored = sink.events();

        // The same `done`, apart from the timing.
        let (a, b) = (last(&events, "done"), last(&stored, "done"));
        for key in ["rows", "truncated", "cancelled"] {
            assert_eq!(a.get(key), b.get(key), "{name}: done.{key}");
        }
        // Same columns event, and no `rows` event at all in store mode.
        assert_eq!(last(&events, "columns"), last(&stored, "columns"), "{name}");
        assert!(
            stored.iter().all(|e| e["event"] != "rows"),
            "{name}: store mode sent a rows event"
        );
        let count = store.row_count().unwrap();
        assert_eq!(
            u64::from(count.fetched),
            b["rows"].as_u64().unwrap(),
            "{name}"
        );
        assert_eq!(count.phase, StorePhase::Complete, "{name}");
        assert_same_cells(&events, &store);
    }
}

/// W4-T3 defect, found by this task and not fixable from `qh-ffi`: a chunk-column that is NULL in
/// every row of its chunk is stored with the `null` encoding (an Arrow `NullArray`), and
/// `render_cell_into` asks `array.is_null(row)`, which is always false for a `NullArray` (it has
/// no validity buffer; `logical_nulls` is what knows). The cell then renders as an empty string
/// with `EMPTY` instead of `NULL`, so the grid would draw the empty-string glyph for a NULL. The fix
/// is one line in `crates/qh-result-store/src/render.rs` (treat `Encoding::Null` as null).
#[test]
fn an_all_null_chunk_column_is_flagged_null() {
    let columns = vec![ColumnMeta::new("a", "text")];
    let rows = vec![vec![Value::Null], vec![Value::Null]];
    let host = host(script(columns, &rows, 2), 256 * 1024 * 1024, None);
    let (store, _) = into_store(&host, None);
    assert_eq!(store.cell_text(0, 0, 0, CellFormat::Raw).unwrap(), None);
}

#[test]
fn a_cell_text_is_the_full_text_and_none_is_null() {
    let (columns, rows) = zoo();
    let host = host(script(columns, &rows, 4), 256 * 1024 * 1024, None);
    let (store, _) = into_store(&host, None);
    // Row 3 is all NULL; row 0 column 10 is the empty string; row 1 column 10 is "NULL".
    assert_eq!(store.cell_text(0, 3, 10, CellFormat::Raw).unwrap(), None);
    assert_eq!(
        store
            .cell_text(0, 0, 10, CellFormat::Raw)
            .unwrap()
            .as_deref(),
        Some("")
    );
    assert_eq!(
        store
            .cell_text(0, 1, 10, CellFormat::Raw)
            .unwrap()
            .as_deref(),
        Some("NULL")
    );
    let err = store.cell_text(0, 99, 0, CellFormat::Raw).unwrap_err();
    assert!(
        matches!(err, StoreFfiError::InvalidArgument { .. }),
        "{err:?}"
    );
}

// --------------------------------------------------------------------------- //
// the events of a store run
// --------------------------------------------------------------------------- //

#[test]
fn a_store_run_reports_progress_at_most_every_16_ms() {
    let (columns, rows) = numbered(20_000);
    let host = host(script(columns, &rows, 200), 256 * 1024 * 1024, None);
    let started = Instant::now();
    let (_, sink) = into_store(&host, Some("1000000"));
    let elapsed_ms = started.elapsed().as_millis() as usize;
    let progress: Vec<(Instant, Json)> = sink
        .timed()
        .into_iter()
        .filter(|(_, event)| event["event"] == "progress")
        .collect();
    assert!(!progress.is_empty(), "the first count always goes out");
    assert!(
        progress.len() <= elapsed_ms / 16 + 1,
        "{} progress events in {elapsed_ms} ms",
        progress.len()
    );
    for pair in progress.windows(2) {
        assert!(pair[1].0 - pair[0].0 >= std::time::Duration::from_millis(15));
        assert!(pair[1].1["rows"].as_u64() > pair[0].1["rows"].as_u64());
    }
}

#[test]
fn the_sink_is_the_hosts_to_choose() {
    let (columns, rows) = numbered(3);
    let host = host(script(columns, &rows, 200), 256 * 1024 * 1024, None);

    // `host.run` carries no store, so a caller asking for one is refused, before the network.
    let mut settings = pairs("SELECT 1", None);
    settings.push(Setting {
        key: "RESULT_SINK".to_owned(),
        value: "store".to_owned(),
    });
    let sink = Recorder::default();
    host.run(
        EngineCommand::Preview,
        settings.clone(),
        Arc::new(sink.clone()),
        RunCancel::new(),
    );
    let events = sink.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(
        events[0]["message"],
        "RESULT_SINK=store needs a result store, which only the app's engine host attaches"
    );

    // The same through the library entry the CLI uses: a plain emitter never carries a store.
    let engine = NoEngine;
    let mut capture = Capture::new();
    let settings = Settings::from_pairs([
        ("DB_KIND", "postgres"),
        ("DB_HOST", "h"),
        ("DB_USER", "u"),
        ("SQL", "SELECT 1"),
        ("RESULT_SINK", "store"),
    ]);
    let outcome = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run(
            Command::Preview,
            &settings,
            &mut capture,
            &engine,
            &CancelFlag::new(),
        ));
    assert!(
        matches!(outcome, Err(qh_ffi::CliError::Usage(_))),
        "{outcome:?}"
    );
    assert!(
        capture.lines.is_empty(),
        "nothing is sent before the refusal"
    );

    // `run_with_store` overwrites whatever the caller wrote for the sink.
    let mut with_ndjson = pairs("SELECT 1", None);
    with_ndjson.push(Setting {
        key: "RESULT_SINK".to_owned(),
        value: "ndjson".to_owned(),
    });
    let store = host.create_result_store().unwrap();
    let sink = Recorder::default();
    host.run_with_store(
        EngineCommand::Preview,
        with_ndjson,
        Arc::clone(&store),
        Arc::new(sink.clone()),
        RunCancel::new(),
    );
    assert!(sink.events().iter().all(|e| e["event"] != "rows"));
    assert_eq!(store.row_count().unwrap().fetched, 3);
}

/// An engine that must never be asked for anything.
struct NoEngine;

#[async_trait]
impl Engine for NoEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        DriverKind::ALL.to_vec()
    }

    fn driver(&self, kind: DriverKind) -> &dyn Driver {
        real_driver(kind)
    }

    async fn connect(&self, _config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        panic!("the usage error comes before the network");
    }
}

#[test]
fn a_store_emitter_hands_the_writer_to_the_command() {
    // The library-level seam `run_with_store` is built on, driven without a host.
    let (columns, rows) = numbered(5);
    let world = Arc::new(World {
        script: Mutex::new(script(columns, &rows, 2)),
    });
    let engine = FakeEngine(world);
    let registry_host = host(Script::default(), 1 << 20, None);
    let store = registry_host.create_result_store().unwrap();
    let writer = store.writer();
    let mut out = StoreEmitter::new(Capture::new(), writer);
    let settings = Settings::from_pairs([
        ("DB_KIND", "postgres"),
        ("DB_HOST", "h"),
        ("DB_USER", "u"),
        ("SQL", "SELECT 1"),
        ("RESULT_SINK", "store"),
        ("RETRIES", "0"),
    ]);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run(
            Command::Preview,
            &settings,
            &mut out,
            &engine,
            &CancelFlag::new(),
        ))
        .expect("a store run");
    let events = out.into_inner().lines;
    assert!(events.iter().all(|e| e["event"] != "rows"));
    assert_eq!(store.row_count().unwrap().fetched, 5);
}

struct FakeEngine(Arc<World>);

#[async_trait]
impl Engine for FakeEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        DriverKind::ALL.to_vec()
    }

    fn driver(&self, kind: DriverKind) -> &dyn Driver {
        real_driver(kind)
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        Ok(Box::new(FakeSession {
            world: Arc::clone(&self.0),
            kind: config.kind,
        }))
    }
}

// --------------------------------------------------------------------------- //
// the run lifecycle
// --------------------------------------------------------------------------- //

#[test]
fn a_store_that_is_not_configured_says_so_and_configuring_twice_is_refused() {
    let world = Arc::new(World {
        script: Mutex::new(Script::default()),
    });
    let host = EngineHost::with_connector(Arc::new(FakeConnector(world)));
    for result in [
        host.create_result_store().map(|_| ()),
        host.store_from_rows(vec![], vec![]).map(|_| ()),
        host.store_stats().map(|_| ()),
    ] {
        match result {
            Err(StoreFfiError::InvalidArgument { message }) => {
                assert_eq!(message, "result stores are not configured")
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }
    assert!(matches!(
        host.configure_result_stores(None, 0),
        Err(StoreFfiError::InvalidArgument { .. })
    ));
    let sweep = host.configure_result_stores(None, 1 << 20).unwrap();
    assert!(!sweep.spill_enabled);
    assert!(sweep.reason.is_some());
    assert!(matches!(
        host.configure_result_stores(None, 1 << 20),
        Err(StoreFfiError::InvalidArgument { .. })
    ));
    assert!(host.create_result_store().is_ok());
}

#[test]
fn a_store_takes_one_run_and_only_preview_or_explain() {
    let (columns, rows) = numbered(3);
    let host = host(script(columns, &rows, 200), 1 << 24, None);
    let store = host.create_result_store().unwrap();
    let run = |command, store: &Arc<ResultHandle>| {
        let sink = Recorder::default();
        host.run_with_store(
            command,
            pairs("SELECT 1", None),
            Arc::clone(store),
            Arc::new(sink.clone()),
            RunCancel::new(),
        );
        sink.events()
    };
    let refused = run(EngineCommand::Count, &store);
    assert_eq!(refused.len(), 1, "{refused:?}");
    assert_eq!(refused[0]["event"], "error");
    assert_eq!(store.row_count().unwrap().phase, StorePhase::Empty);

    let first = run(EngineCommand::Preview, &store);
    assert_eq!(last(&first, "done")["rows"], 3);
    assert_eq!(store.row_count().unwrap().phase, StorePhase::Complete);

    let second = run(EngineCommand::Preview, &store);
    assert_eq!(
        second[0]["message"],
        "this result store already holds a run"
    );
    assert_eq!(
        store.row_count().unwrap().fetched,
        3,
        "the first result is untouched"
    );
}

#[test]
fn a_run_that_fails_before_its_rows_leaves_a_failed_store_not_an_empty_one() {
    // A statement the guard refuses never reaches the pump.
    let (columns, rows) = numbered(3);
    let host = host(script(columns, &rows, 200), 1 << 24, None);
    let store = host.create_result_store().unwrap();
    let sink = Recorder::default();
    let mut settings = pairs("DELETE FROM t", None);
    settings.push(Setting {
        key: "SAFE_MODE".to_owned(),
        value: "read_only".to_owned(),
    });
    host.run_with_store(
        EngineCommand::Preview,
        settings,
        Arc::clone(&store),
        Arc::new(sink.clone()),
        RunCancel::new(),
    );
    assert_eq!(last(&sink.events(), "error")["event"], "error");
    assert_eq!(store.row_count().unwrap().phase, StorePhase::Failed);
}

#[test]
fn closing_the_tab_mid_stream_ends_the_run_cancelled_and_not_failed() {
    let (columns, rows) = numbered(2_000);
    let slot: Arc<Mutex<Option<Arc<ResultHandle>>>> = Arc::default();
    let hook_slot = Arc::clone(&slot);
    let mut script = script(columns, &rows, 200);
    script.hook = Some(Arc::new(move |served| {
        if served == 3 {
            if let Some(store) = hook_slot.lock().unwrap().as_ref() {
                store.release().unwrap();
            }
        }
    }));
    let host = host(script, 1 << 24, None);
    let store = host.create_result_store().unwrap();
    *slot.lock().unwrap() = Some(Arc::clone(&store));
    let sink = Recorder::default();
    host.run_with_store(
        EngineCommand::Preview,
        pairs("SELECT 1", Some("100000")),
        Arc::clone(&store),
        Arc::new(sink.clone()),
        RunCancel::new(),
    );
    let events = sink.events();
    assert!(events.iter().all(|e| e["event"] != "error"), "{events:?}");
    assert_eq!(last(&events, "done")["cancelled"], true);
    assert!(matches!(store.row_count(), Err(StoreFfiError::StaleHandle)));
}

#[test]
fn a_panic_in_a_run_is_an_error_event_and_a_failed_store() {
    let (columns, rows) = numbered(1_000);
    let mut script = script(columns, &rows, 200);
    script.panic_at = Some(2);
    let host = host(script, 1 << 24, None);
    let (store, sink) = into_store(&host, Some("100000"));
    let events = sink.events();
    let message = last(&events, "error")["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(message.contains("internal error"), "{message}");
    assert!(message.contains("panicked on purpose"), "{message}");
    assert_eq!(store.row_count().unwrap().phase, StorePhase::Failed);
    // The handle still works: a panic in the run did not poison the store.
    assert_eq!(
        store.row_count().unwrap().fetched,
        400,
        "two batches were served"
    );
    assert!(store
        .window(0, 0, 10, vec![0, 1], vec![CellFormat::Raw; 2])
        .is_ok());
}

#[test]
fn disk_full_mid_stream_is_an_error_event_and_other_stores_stay_readable() {
    let dir = tempfile::tempdir().unwrap();
    let spill = dir.path().join("spill");
    // 1 MiB: a few batches fit, then the writer has to evict.
    let (columns, rows) = {
        let (columns, _) = numbered(0);
        let rows: Vec<Vec<Value>> = (0..80_000)
            .map(|i| {
                vec![
                    Value::Int(i),
                    Value::Text(format!("a fairly long label to take some room {i}").into()),
                    Value::Text(format!("another long cell to take some room {i}").into()),
                ]
            })
            .collect();
        (columns, rows)
    };
    let host = host(script(columns, &rows, 2_000), 1 << 20, Some(&spill));

    // Store A, finished before the trouble starts.
    let a_rows: Vec<Vec<Option<String>>> = (0..200)
        .map(|i| {
            vec![
                Some(format!("a-{i}")),
                Some("x".repeat(400)),
                (i % 3 != 0).then(|| format!("n{i}")),
            ]
        })
        .collect();
    let a = host
        .store_from_rows(
            ["c0", "c1", "c2"]
                .iter()
                .map(|name| ColumnWire {
                    name: (*name).to_owned(),
                    type_name: "text".to_owned(),
                })
                .collect(),
            a_rows.clone(),
        )
        .unwrap();

    // Store B's disk is full.
    let b = host.create_result_store().unwrap();
    *b.shared_for_test().faulty_medium.lock().unwrap() = Some(Box::new(FaultyMedium));
    let sink = Recorder::default();
    host.run_with_store(
        EngineCommand::Preview,
        pairs("SELECT 1", Some("1000000")),
        Arc::clone(&b),
        Arc::new(sink.clone()),
        RunCancel::new(),
    );
    let events = sink.events();
    let message = last(&events, "error")["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(message.contains("disk is full"), "{message}");
    assert!(
        events.iter().all(|e| e["event"] != "done"),
        "an error ends the run: {events:?}"
    );
    assert_eq!(b.row_count().unwrap().phase, StorePhase::Failed);

    // A reads back exactly what it was given, whether it stayed resident or was spilled.
    let window = decode(&a.rows_text(0, 0, 200, vec![0, 1, 2]).unwrap());
    assert_eq!(window.rows, 200);
    for (r, row) in a_rows.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            assert_eq!(window.cells[r * 3 + c].0, *cell, "A row {r} column {c}");
        }
    }
}

// --------------------------------------------------------------------------- //
// the handle
// --------------------------------------------------------------------------- //

#[test]
fn a_released_handle_answers_stale_handle_everywhere_and_release_is_idempotent() {
    let host = host(Script::default(), 1 << 24, None);
    let store = host
        .store_from_rows(
            vec![ColumnWire {
                name: "a".to_owned(),
                type_name: "text".to_owned(),
            }],
            vec![vec![Some("x".to_owned())]],
        )
        .unwrap();
    assert_eq!(host.store_stats().unwrap().stores, 1);
    store.release().unwrap();
    store.release().unwrap();
    assert_eq!(host.store_stats().unwrap().stores, 0);
    let stale = |result: Result<(), StoreFfiError>| {
        assert!(
            matches!(result, Err(StoreFfiError::StaleHandle)),
            "{result:?}"
        )
    };
    stale(store.row_count().map(|_| ()));
    stale(store.columns().map(|_| ()));
    stale(
        store
            .window(0, 0, 1, vec![0], vec![CellFormat::Raw])
            .map(|_| ()),
    );
    stale(store.rows_text(0, 0, 1, vec![0]).map(|_| ()));
    stale(store.cell_text(0, 0, 0, CellFormat::Raw).map(|_| ()));
    stale(store.column_widths().map(|_| ()));
    stale(store.distinct_values(0, 10).map(|_| ()));
    stale(
        store
            .set_view(ViewSpec {
                sort: None,
                filters: vec![],
                search: None,
            })
            .map(|_| ()),
    );
}

#[test]
fn dropping_the_handle_frees_the_store() {
    let host = host(Script::default(), 1 << 24, None);
    let store = host.store_synthetic(1_000, 3, 1).unwrap();
    assert_eq!(host.store_stats().unwrap().stores, 1);
    assert!(host.store_stats().unwrap().resident_bytes > 0);
    drop(store);
    let stats = host.store_stats().unwrap();
    assert_eq!((stats.stores, stats.resident_bytes), (0, 0));
}

#[test]
fn a_bad_window_request_is_a_typed_error_and_never_a_panic() {
    let host = host(Script::default(), 1 << 26, None);
    let store = host.store_synthetic(100, 3, 1).unwrap();
    let invalid = |result: Result<Vec<u8>, StoreFfiError>, what: &str| {
        assert!(
            matches!(result, Err(StoreFfiError::InvalidArgument { .. })),
            "{what}: {result:?}"
        )
    };
    invalid(
        store.window(0, 0, 10, vec![3], vec![CellFormat::Raw]),
        "column past the end",
    );
    invalid(
        store.window(0, 0, 10, vec![u32::MAX], vec![CellFormat::Raw]),
        "huge column",
    );
    invalid(
        store.window(0, 0, 10, vec![0, 1], vec![CellFormat::Raw]),
        "formats mismatch",
    );
    invalid(
        store.window(0, 0, 4_097, vec![0], vec![CellFormat::Raw]),
        "too many rows",
    );
    invalid(
        store.window(0, 0, 4_096, vec![0; 65], vec![CellFormat::Raw; 65]),
        "too many cells",
    );
    invalid(
        store.window(0, 0, 1, vec![0; 1_025], vec![CellFormat::Raw; 1_025]),
        "too many columns",
    );
    // Rows past the end are clamped, not an error: the grid asks for what it thinks it has.
    let past = decode(
        &store
            .window(0, 1_000, 10, vec![0], vec![CellFormat::Raw])
            .unwrap(),
    );
    assert_eq!(
        (past.rows, past.first_row, past.visible_total),
        (0, 100, 100)
    );
    let tail = decode(
        &store
            .window(0, 95, 10, vec![0], vec![CellFormat::Raw])
            .unwrap(),
    );
    assert_eq!(tail.rows, 5);
    assert_eq!(tail.source_rows, vec![95, 96, 97, 98, 99]);
    // A view id that is not the current one is `StaleView`, naming the current one.
    match store.window(7, 0, 1, vec![0], vec![CellFormat::Raw]) {
        Err(StoreFfiError::StaleView { current }) => assert_eq!(current, 0),
        other => panic!("expected StaleView, got {other:?}"),
    }
    // A view over a column that does not exist.
    let bad = store.set_view(ViewSpec {
        sort: Some(SortSpec {
            column: 9,
            descending: false,
        }),
        filters: vec![],
        search: None,
    });
    assert!(
        matches!(bad, Err(StoreFfiError::InvalidArgument { .. })),
        "{bad:?}"
    );
}

#[test]
fn windows_cross_chunk_boundaries_and_follow_a_view() {
    let host = host(Script::default(), 1 << 29, None);
    // 70,000 rows seal at 65,536, so there are two chunks. The same seed gives the same rows,
    // so `plain` is the ground truth that the viewed store is checked against.
    let store = host.store_synthetic(70_000, 3, 7).unwrap();
    let plain = host.store_synthetic(70_000, 3, 7).unwrap();
    let all = vec![0u32, 1, 2];
    let formats = vec![CellFormat::Raw; 3];
    let cell = |handle: &ResultHandle, view: u64, row: u32, column: u32| {
        handle
            .cell_text(view, row, column, CellFormat::Raw)
            .unwrap()
            .unwrap()
    };

    let across = decode(
        &store
            .window(0, 65_530, 12, all.clone(), formats.clone())
            .unwrap(),
    );
    assert_eq!(across.source_rows, (65_530..65_542).collect::<Vec<u32>>());
    assert_eq!(
        (across.first_row, across.visible_total, across.rows),
        (65_530, 70_000, 12)
    );
    assert_ne!(across.flags & 0b11, 0, "CUT and COMPLETE");
    for (r, row) in (65_530..65_542u32).enumerate() {
        for c in 0..3 {
            assert_eq!(
                across.cells[r * 3 + c].0.as_deref(),
                Some(cell(&plain, 0, row, c as u32).as_str())
            );
        }
    }

    // Sorted, the top rows come from anywhere in the store.
    let view = store
        .set_view(ViewSpec {
            sort: Some(SortSpec {
                column: 0,
                descending: true,
            }),
            filters: vec![],
            search: None,
        })
        .unwrap();
    assert_eq!((view.visible, view.fetched), (70_000, 70_000));
    let top = decode(
        &store
            .window(view.view_id, 0, 30, all.clone(), formats.clone())
            .unwrap(),
    );
    assert_ne!(top.flags & 0b100, 0, "VIEWED");
    let keys: Vec<i64> = (0..30)
        .map(|r| top.cells[r * 3].0.as_ref().unwrap().parse().unwrap())
        .collect();
    assert!(
        keys.windows(2).all(|w| w[0] >= w[1]),
        "descending: {keys:?}"
    );
    let mut spans = [false; 2];
    for (r, source) in top.source_rows.iter().enumerate() {
        spans[usize::from(*source >= 65_536)] = true;
        for c in 0..3 {
            assert_eq!(
                top.cells[r * 3 + c].0.as_deref(),
                Some(cell(&plain, 0, *source, c as u32).as_str()),
                "sorted row {r} column {c}"
            );
        }
    }
    // The old view id is now stale, and says which one is current.
    match store.window(0, 0, 1, all.clone(), formats.clone()) {
        Err(StoreFfiError::StaleView { current }) => assert_eq!(current, view.view_id),
        other => panic!("expected StaleView, got {other:?}"),
    }
    let count = store.row_count().unwrap();
    assert_eq!(
        (count.view_id, count.visible, count.fetched),
        (view.view_id, 70_000, 70_000)
    );
    let _ = spans;

    // A filter: the rows whose text column holds "r6999" (row 6999 and 69990..69999).
    let filtered = store
        .set_view(ViewSpec {
            sort: None,
            filters: vec![FilterSpec::Text {
                column: 2,
                needle: "r6999".to_owned(),
            }],
            search: None,
        })
        .unwrap();
    // Hex digits follow the row number, so "r6999" also starts the texts of 69990..69999.
    let found = decode(
        &store
            .window(filtered.view_id, 0, 100, all.clone(), formats.clone())
            .unwrap(),
    );
    assert_eq!(filtered.visible as usize, found.rows);
    assert!(
        found.source_rows.windows(2).all(|w| w[0] < w[1]),
        "source order"
    );
    assert!(found.source_rows.contains(&6_999));
    assert!(found.source_rows.contains(&69_995));
    for (r, source) in found.source_rows.iter().enumerate() {
        assert!(found.cells[r * 3 + 2]
            .0
            .as_ref()
            .unwrap()
            .starts_with("r6999"));
        assert_eq!(
            found.cells[r * 3 + 2].0.as_deref(),
            Some(cell(&plain, 0, *source, 2).as_str())
        );
    }

    // The distinct picker reads store order, whatever the view.
    let distinct = store.distinct_values(1, 10).unwrap();
    assert!(distinct.more && distinct.values.is_empty());
}
