//! `RETRIES` through a command, end to end.
//!
//! `crates/qh-ffi/src/retry.rs` unit-tests the policy. This file tests the thing that
//! can silently regress while those stay green: whether a command actually *uses* it.
//! The engine here is scripted rather than a database, so a failure happens on a chosen
//! attempt and the number of attempts is counted, not guessed at.
//!
//! What each test is protecting:
//!
//! - `a_transient_fetch_is_retried_and_the_rows_do_not_repeat` — a page that failed
//!   mid-stream is fetched again, and the result is the same rows in the same order as a
//!   run where nothing failed. This is the fix for `docs/golden-deltas.md` D-7.
//! - `retries_zero_fails_where_the_default_would_have_recovered` — `RETRIES=0` really
//!   means one attempt: the same script fails without it.
//! - `a_statement_the_server_refused_reaches_the_user_on_the_first_attempt` — the
//!   syntax-error rule, at the command and not only in the driver.
//! - `a_writing_statement_is_not_retried` — `to_table`'s `DROP`/`CREATE`/`INSERT` is
//!   deliberately outside the retry, so a `CREATE` that may have committed is never
//!   re-issued blind.
//! - `a_connection_that_never_opened_is_retried` — the one case every driver shares.
//!
//! The waits are the policy's real ones (2 s, then 4 s), because a test that shortened
//! them would not be testing what the command line does. Each script fails once, so a
//! test costs one 2 s wait.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use qh_core::{ColumnBatch, ColumnMeta, EngineError, FailureKind, Value};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions,
    ObjectPath, ObjectsPage, Session,
};
use qh_ffi::events::Capture;
use qh_ffi::{run, CancelFlag, CliError, Command, Engine, Settings};
use serde_json::Value as Json;

/// How many of each call the engine was asked to make.
#[derive(Default)]
struct Counts {
    connects: AtomicUsize,
    executes: AtomicUsize,
    fetches: AtomicUsize,
}

/// How a scripted fetch answers.
enum Step {
    Rows(Vec<i64>),
    Fail(EngineError),
    End,
}

/// What the engine does, in order: which connects fail, which statements fail, and what
/// the cursor then hands out.
#[derive(Default)]
struct Script {
    connect_failures: VecDeque<EngineError>,
    execute_failures: VecDeque<EngineError>,
    steps: VecDeque<Step>,
    /// Whether the session claims a cursor that lives inside a connection.
    persistent: bool,
}

/// A failure that is the server's answer: coded, so it is never retried.
fn refused(code: &str) -> EngineError {
    EngineError::Query {
        message: format!("the server refused the statement ({code})"),
        code: Some(code.to_owned()),
        position: None,
        kind: FailureKind::Permanent,
    }
}

/// A request that failed in flight: no page, no server code.
fn transport(message: &str) -> EngineError {
    EngineError::Query {
        message: message.to_owned(),
        code: None,
        position: None,
        kind: FailureKind::Transient,
    }
}

fn unreachable() -> EngineError {
    EngineError::Connect {
        message: "could not reach the coordinator".to_owned(),
        kind: FailureKind::Transient,
    }
}

struct ScriptedCursor {
    steps: VecDeque<Step>,
    counts: Arc<Counts>,
}

#[async_trait]
impl Cursor for ScriptedCursor {
    fn columns(&self) -> &[ColumnMeta] {
        COLUMNS
    }

    async fn next_batch(&mut self, _max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        self.counts.fetches.fetch_add(1, Ordering::SeqCst);
        match self.steps.pop_front() {
            None | Some(Step::End) => Ok(None),
            Some(Step::Rows(rows)) => Ok(Some(
                ColumnBatch::new(vec![rows.into_iter().map(Value::Int).collect()])
                    .expect("a square batch"),
            )),
            Some(Step::Fail(error)) => Err(error),
        }
    }

    fn affected_rows(&self) -> Option<u64> {
        // What Trino reports for a `SELECT`: nothing. `to_table`'s `done` then carries
        // `-1`, which is the value it has always carried.
        None
    }
}

struct ScriptedSession {
    execute_failures: VecDeque<EngineError>,
    steps: VecDeque<Step>,
    counts: Arc<Counts>,
    persistent: bool,
}

#[async_trait]
impl Session for ScriptedSession {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            transactions: false,
            multiple_result_sets: false,
            cancel: true,
            explain: true,
            levels: vec![BrowseLevel::Table],
            objects_columns: Vec::new(),
            persistent_connection: self.persistent,
        }
    }

    fn query_id(&self) -> Option<String> {
        Some("20260923_000000_00000_abcde".to_owned())
    }

    async fn execute(
        &mut self,
        _sql: &str,
        _options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        self.counts.executes.fetch_add(1, Ordering::SeqCst);
        match self.execute_failures.pop_front() {
            Some(error) => Err(error),
            None => Ok(Box::new(ScriptedCursor {
                steps: std::mem::take(&mut self.steps),
                counts: Arc::clone(&self.counts),
            })),
        }
    }

    async fn browse(
        &mut self,
        _level: BrowseLevel,
        _path: &ObjectPath,
        _include_system: bool,
    ) -> Result<Vec<String>, EngineError> {
        Ok(vec!["one".to_owned()])
    }

    async fn objects(&mut self, _path: &ObjectPath) -> Result<ObjectsPage, EngineError> {
        Ok(ObjectsPage::default())
    }

    fn explain_statement(&self, sql: &str) -> String {
        sql.to_owned()
    }

    async fn cancel(&self) -> Result<(), EngineError> {
        Ok(())
    }

    async fn close(self: Box<Self>) -> Result<(), EngineError> {
        Ok(())
    }
}

static COLUMNS: &[ColumnMeta] = &[];

/// The driver descriptor, which the commands read for a default port and a level list.
struct FakeDriver;

#[async_trait]
impl Driver for FakeDriver {
    fn kind(&self) -> DriverKind {
        DriverKind::Trino
    }

    fn label(&self) -> &'static str {
        "Trino"
    }

    fn default_port(&self) -> u16 {
        8080
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            transactions: false,
            multiple_result_sets: false,
            cancel: true,
            explain: true,
            levels: vec![BrowseLevel::Table],
            objects_columns: Vec::new(),
            persistent_connection: false,
        }
    }

    async fn connect(&self, _config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        // Never reached: a command asks the `Engine`, which is where the script lives.
        Err(unreachable())
    }
}

/// A scripted engine, and the counters its script touched.
struct FakeEngine {
    script: Mutex<Script>,
    counts: Arc<Counts>,
    driver: FakeDriver,
}

impl FakeEngine {
    fn new(script: Script) -> Self {
        Self {
            script: Mutex::new(script),
            counts: Arc::new(Counts::default()),
            driver: FakeDriver,
        }
    }
}

#[async_trait]
impl Engine for FakeEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        vec![DriverKind::Trino]
    }

    fn driver(&self, _kind: DriverKind) -> &dyn Driver {
        &self.driver
    }

    async fn connect(&self, _config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        self.counts.connects.fetch_add(1, Ordering::SeqCst);
        let mut script = self.script.lock().expect("the script");
        if let Some(error) = script.connect_failures.pop_front() {
            return Err(error);
        }
        Ok(Box::new(ScriptedSession {
            execute_failures: std::mem::take(&mut script.execute_failures),
            steps: std::mem::take(&mut script.steps),
            counts: Arc::clone(&self.counts),
            persistent: script.persistent,
        }))
    }
}

/// One command, with its events captured.
async fn events(
    command: Command,
    engine: &FakeEngine,
    settings: &[(&str, &str)],
) -> Result<Vec<Json>, CliError> {
    let settings = Settings::from_pairs(settings.iter().map(|(key, value)| (*key, *value)));
    let mut out = Capture::new();
    run(command, &settings, &mut out, engine, &CancelFlag::new()).await?;
    Ok(out.lines)
}

/// The rows a `preview` sent, flattened, plus the count `done` reported.
fn rows_of(events: &[Json]) -> (Vec<i64>, i64) {
    let mut rows = Vec::new();
    let mut done = None;
    for event in events {
        match event["event"].as_str() {
            Some("rows") => {
                for row in event["data"].as_array().expect("a data array") {
                    // A cell is the writers' canonical text, so an integer arrives as a
                    // JSON string: `qh-core::render::to_text` is what the grid reads.
                    let cell = row[0]
                        .as_str()
                        .unwrap_or_else(|| panic!("expected a text cell: {row}"));
                    rows.push(cell.parse::<i64>().expect("a number"));
                }
            }
            Some("done") => done = Some(event["rows"].as_i64().expect("a row count")),
            _ => {}
        }
    }
    (rows, done.expect("preview always ends with done"))
}

/// The settings `preview` needs, with the retry count the test is about.
fn preview_settings(retries: &str) -> Vec<(&'static str, &str)> {
    vec![
        ("DB_KIND", "trino"),
        ("DB_HOST", "trino.internal"),
        ("SQL", "SELECT n FROM t"),
        ("RETRIES", retries),
    ]
}

#[tokio::test]
async fn a_transient_fetch_is_retried_and_the_rows_do_not_repeat() {
    let engine = FakeEngine::new(Script {
        steps: vec![
            Step::Rows(vec![1, 2]),
            Step::Fail(transport(
                "could not fetch the next page: connection closed before message completed",
            )),
            Step::Rows(vec![3, 4]),
            Step::End,
        ]
        .into(),
        ..Script::default()
    });

    let events = events(Command::Preview, &engine, &preview_settings("2"))
        .await
        .expect("the retry turns the failure into a complete result");
    let (rows, done) = rows_of(&events);

    // The same rows, once each, in the server's order: a re-fetched page would show up
    // here as a repeat, and a page given up on would show up as a gap.
    assert_eq!(rows, vec![1, 2, 3, 4]);
    assert_eq!(done, 4);
    assert_eq!(
        engine.counts.fetches.load(Ordering::SeqCst),
        4,
        "three answers and one end: the failed fetch was asked again"
    );
}

#[tokio::test]
async fn retries_zero_fails_where_the_default_would_have_recovered() {
    let script = || Script {
        steps: vec![
            Step::Rows(vec![1, 2]),
            Step::Fail(transport(
                "could not fetch the next page: connection closed",
            )),
            Step::Rows(vec![3, 4]),
            Step::End,
        ]
        .into(),
        ..Script::default()
    };

    // The default (5) recovers, which is the previous engine's behaviour.
    let recovered = FakeEngine::new(script());
    let captured = events(Command::Preview, &recovered, &preview_settings("5"))
        .await
        .expect("five retries recover");
    assert_eq!(rows_of(&captured).0, vec![1, 2, 3, 4]);

    // And `RETRIES=0` does not retry, so the same script fails. The two must differ, or
    // the setting is decoration.
    let none = FakeEngine::new(script());
    let error = events(Command::Preview, &none, &preview_settings("0"))
        .await
        .expect_err("RETRIES=0 does not retry");
    assert!(
        error.message().contains("connection closed"),
        "the server's own wording, not a summary: {error:?}"
    );
    assert_eq!(
        none.counts.fetches.load(Ordering::SeqCst),
        2,
        "one answer, the failure, and no third fetch"
    );
}

#[tokio::test]
async fn a_statement_the_server_refused_reaches_the_user_on_the_first_attempt() {
    // The same classification the Trino suite pins at the driver
    // (`a_syntax_error_carries_the_servers_code_and_is_not_retried`), now at the command:
    // a retry here would make the user wait 62 s to see a typo.
    let engine = FakeEngine::new(Script {
        execute_failures: vec![refused("1")].into(),
        ..Script::default()
    });

    let error = events(Command::Preview, &engine, &preview_settings("5"))
        .await
        .expect_err("a refusal is an answer");
    assert!(error.message().contains("(1)"), "{error:?}");
    assert_eq!(
        engine.counts.executes.load(Ordering::SeqCst),
        1,
        "one attempt, no waiting"
    );
}

#[tokio::test]
async fn a_writing_statement_is_not_retried() {
    // `to_table` runs a `DROP`/`CREATE TABLE AS`/`INSERT`. A `CREATE` the coordinator
    // accepted but could not answer about may have committed, so re-issuing it blind
    // would write the table twice; the command therefore takes the connect retry and
    // leaves its statements alone.
    let engine = FakeEngine::new(Script {
        execute_failures: vec![unreachable()].into(),
        ..Script::default()
    });

    let error = events(
        Command::ToTable,
        &engine,
        &[
            ("DB_KIND", "trino"),
            ("DB_HOST", "trino.internal"),
            ("SQL", "SELECT n FROM t"),
            ("TARGET_CATALOG", "tpch"),
            ("TARGET_SCHEMA", "tiny"),
            ("TARGET_TABLE", "copied"),
            ("RETRIES", "5"),
        ],
    )
    .await
    .expect_err("a failed write is reported, not repeated");

    assert!(error.message().contains("coordinator"), "{error:?}");
    assert_eq!(
        engine.counts.executes.load(Ordering::SeqCst),
        1,
        "a writing statement is never re-issued"
    );
}

#[tokio::test]
async fn a_connection_that_never_opened_is_retried() {
    let engine = FakeEngine::new(Script {
        connect_failures: vec![unreachable()].into(),
        steps: vec![Step::Rows(vec![7]), Step::End].into(),
        ..Script::default()
    });

    let captured = events(Command::Preview, &engine, &preview_settings("2"))
        .await
        .expect("the second connect works");
    assert_eq!(rows_of(&captured).0, vec![7]);
    assert_eq!(engine.counts.connects.load(Ordering::SeqCst), 2);

    // And with no retries it is the first connect's failure that reaches the user.
    let engine = FakeEngine::new(Script {
        connect_failures: vec![unreachable()].into(),
        ..Script::default()
    });
    events(Command::Preview, &engine, &preview_settings("0"))
        .await
        .expect_err("RETRIES=0 does not reconnect");
    assert_eq!(engine.counts.connects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_cursor_inside_a_connection_is_not_retried_mid_stream() {
    // PostgreSQL and MySQL say `persistent_connection: true`. A fetch that failed there
    // cannot be re-issued, and re-running the statement would repeat the rows already
    // written, so the failure reaches the user — `crates/qh-ffi/src/retry.rs` says so.
    let engine = FakeEngine::new(Script {
        steps: vec![
            Step::Rows(vec![1, 2]),
            Step::Fail(transport("the connection to the server was closed")),
            Step::Rows(vec![3, 4]),
            Step::End,
        ]
        .into(),
        persistent: true,
        ..Script::default()
    });

    let error = events(Command::Preview, &engine, &preview_settings("2"))
        .await
        .expect_err("a broken cursor cannot resume");
    assert!(error.message().contains("was closed"), "{error:?}");
    assert_eq!(
        engine.counts.fetches.load(Ordering::SeqCst),
        2,
        "the failure is reported where it happened"
    );
}
