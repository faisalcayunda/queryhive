//! Where the server said a statement went wrong, from the driver's `EngineError` to the `error`
//! event the app reads (blueprint w10 section 8.1).
//!
//! Two layers:
//!
//! * **Offline.** A scripted engine fails the statement with and without a position, and the event
//!   is checked for the one rule that keeps CLI, MCP and golden output unchanged: `position` is
//!   there only when the run set `ERROR_POSITION`, and only when the server gave one.
//! * **Live.** The same event from a real server for a syntax error and for an unknown table on a
//!   second line behind an emoji, so the unit is checked against what each server counts (a byte
//!   or UTF-16 count would be off by three).
//!
//! ```bash
//! QH_TEST_POSTGRES=1 QH_TEST_MYSQL=1 QH_TEST_TRINO=1 cargo test -p qh-ffi --test error_position
//! ```
//!
//! A live test without its guard prints why it is skipping and returns, for the reason the driver
//! suites give: a green run that tested nothing is worse than a visible skip.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use qh_core::{EngineError, FailureKind};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions,
    ObjectPath, ObjectsPage, Session,
};
use qh_ffi::events::{error_event, Capture};
use qh_ffi::host::EngineHost;
use qh_ffi::uniffi_api::{EngineCommand, EventSink, RunCancel, Setting};
use qh_ffi::{run, CancelFlag, CliError, Command, Engine, Settings};
use serde_json::Value as Json;

// --------------------------------------------------------------------------- //
// a scripted engine whose statement is refused
// --------------------------------------------------------------------------- //

fn capabilities() -> Capabilities {
    Capabilities {
        transactions: false,
        multiple_result_sets: false,
        cancel: true,
        explain: true,
        levels: vec![BrowseLevel::Table],
        objects_columns: Vec::new(),
        persistent_connection: false,
        statement_timeout: true,
        parameters: None,
        read_only: false,
    }
}

struct RefusingSession(EngineError);

#[async_trait]
impl Session for RefusingSession {
    fn capabilities(&self) -> Capabilities {
        capabilities()
    }

    fn query_id(&self) -> Option<String> {
        None
    }

    async fn execute(
        &mut self,
        _sql: &str,
        _options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        Err(self.0.clone())
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

struct FakeDriver;

#[async_trait]
impl Driver for FakeDriver {
    fn kind(&self) -> DriverKind {
        DriverKind::Postgres
    }

    fn label(&self) -> &'static str {
        "PostgreSQL"
    }

    fn default_port(&self) -> u16 {
        5432
    }

    fn capabilities(&self) -> Capabilities {
        capabilities()
    }

    async fn connect(&self, _config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        Err(EngineError::Internal {
            message: "a command asks the engine, not the driver".to_owned(),
        })
    }
}

struct RefusingEngine {
    error: EngineError,
    driver: FakeDriver,
}

#[async_trait]
impl Engine for RefusingEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        vec![DriverKind::Postgres]
    }

    fn driver(&self, _kind: DriverKind) -> &dyn Driver {
        &self.driver
    }

    async fn connect(&self, _config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        Ok(Box::new(RefusingSession(self.error.clone())))
    }
}

fn settings(pairs: &[(&str, &str)]) -> Settings {
    Settings::from_pairs(pairs.iter().map(|(key, value)| (*key, *value)))
}

fn refused(position: Option<u32>) -> EngineError {
    EngineError::Query {
        message: "syntax error at or near \"SELEC\"".to_owned(),
        code: Some("42601".to_owned()),
        position,
        kind: FailureKind::Permanent,
    }
}

/// A `preview` of `SELEC 1` against the scripted engine, failed.
async fn failed_preview(position: Option<u32>) -> CliError {
    let engine = RefusingEngine {
        error: refused(position),
        driver: FakeDriver,
    };
    let settings = settings(&[
        ("DB_KIND", "postgres"),
        ("DB_HOST", "pg.internal"),
        ("SQL", "SELEC 1"),
        ("RETRIES", "0"),
    ]);
    run(
        Command::Preview,
        &settings,
        &mut Capture::new(),
        &engine,
        &CancelFlag::new(),
    )
    .await
    .expect_err("the server refuses the statement")
}

#[tokio::test]
async fn a_located_failure_reads_like_any_other_and_is_told_apart_only_in_the_event() {
    let located = failed_preview(Some(7)).await;
    let unlocated = failed_preview(None).await;
    assert!(
        matches!(located, CliError::QueryAt { position: 7, .. }),
        "{located:?}"
    );
    assert!(matches!(unlocated, CliError::Query(_)), "{unlocated:?}");
    // Same words either way, so every consumer that prints the error is unchanged.
    assert_eq!(located.to_string(), unlocated.to_string());
    assert_eq!(located.message(), unlocated.message());
    assert_eq!(located.position(), Some(7));
    assert_eq!(unlocated.position(), None);
}

#[tokio::test]
async fn the_event_carries_the_position_only_when_the_run_asked_and_the_server_gave_one() {
    let located = failed_preview(Some(7)).await;
    let unlocated = failed_preview(None).await;

    // Not asked: the CLI, MCP and golden shape, byte for byte what it was.
    let plain = error_event(&located, &settings(&[]));
    assert!(plain.get("position").is_none(), "{plain}");
    assert_eq!(plain["event"], "error");
    assert_eq!(plain["message"], "syntax error at or near \"SELEC\"");
    // A spelling the flag reader does not accept keeps the default, which is off.
    let typo = error_event(&located, &settings(&[("ERROR_POSITION", "yes please")]));
    assert!(typo.get("position").is_none(), "{typo}");

    // Asked: the position joins the same event, after the message.
    let asked = error_event(&located, &settings(&[("ERROR_POSITION", "1")]));
    assert_eq!(asked["position"], 7, "{asked}");
    assert_eq!(asked["message"], plain["message"]);

    // Asked, but the server did not say where: no key rather than a null.
    let nowhere = error_event(&unlocated, &settings(&[("ERROR_POSITION", "1")]));
    assert!(nowhere.get("position").is_none(), "{nowhere}");
}

// --------------------------------------------------------------------------- //
// live: what each server counts
// --------------------------------------------------------------------------- //

/// Where a live test keeps the engine's own database, so no run touches the app's.
const TEST_DATABASE: &str = concat!(env!("CARGO_TARGET_TMPDIR"), "/error-position.sqlite3");

/// Collects the lines the host hands its sink.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);

impl EventSink for Recorder {
    fn on_event(&self, line: String) {
        self.0.lock().expect("recorded lines").push(line);
    }
}

/// One statement through the app's own entry point, [`EngineHost`], and the `error` event it
/// failed with. The host is not the CLI's `run`: it is what the app calls, and its `fail` is the
/// path that must carry the position.
fn live_error(env: &[(&str, &str)], sql: &str) -> Json {
    let sink = Recorder::default();
    let settings = env
        .iter()
        .chain(&[
            ("SQL", sql),
            ("RETRIES", "0"),
            ("ERROR_POSITION", "1"),
            ("DB_PATH", TEST_DATABASE),
        ])
        .map(|(key, value)| Setting {
            key: (*key).to_owned(),
            value: (*value).to_owned(),
        })
        .collect();
    EngineHost::new().run(
        EngineCommand::Preview,
        settings,
        Arc::new(sink.clone()),
        RunCancel::new(),
    );
    let lines = sink.0.lock().expect("recorded lines").clone();
    let last = lines.last().unwrap_or_else(|| panic!("no event at all"));
    serde_json::from_str(last).expect("an event is JSON")
}

fn live(flag: &str, hint: &str) -> bool {
    let on = std::env::var(flag).as_deref() == Ok("1");
    if !on {
        eprintln!("skipped: set {flag}=1 with {hint} running");
    }
    on
}

const POSTGRES_ENV: &[(&str, &str)] = &[
    ("DB_KIND", "postgres"),
    ("DB_HOST", "127.0.0.1"),
    ("DB_PORT", "55432"),
    ("DB_USER", "qh"),
    ("DB_PASSWORD", "qh-dev-only"),
    ("DB_DATABASE", "qh"),
    ("DB_SCHEMA", "public"),
    ("DB_SSLMODE", "disable"),
];

const MYSQL_ENV: &[(&str, &str)] = &[
    ("DB_KIND", "mysql"),
    ("DB_HOST", "127.0.0.1"),
    ("DB_PORT", "53306"),
    ("DB_USER", "qh"),
    ("DB_PASSWORD", "qh-dev-only"),
    ("DB_DATABASE", "qh"),
    ("DB_SSLMODE", "disable"),
];

const TRINO_ENV: &[(&str, &str)] = &[
    ("DB_KIND", "trino"),
    ("DB_HOST", "127.0.0.1"),
    ("DB_PORT", "58080"),
    ("DB_USER", "queryhive"),
    ("DB_DATABASE", "tpch"),
    ("DB_SCHEMA", "sf1"),
];

#[test]
fn postgres_reports_a_character_offset() {
    if !live("QH_TEST_POSTGRES", "deploy/dev/up.sh postgres") {
        return;
    }
    let event = live_error(POSTGRES_ENV, "SELEC 1");
    assert_eq!(event["position"], 1, "{event}");
    // `nope` is the 17th scalar: ten on the first line, its newline, `FROM `. It is the 20th
    // byte and the 18th UTF-16 unit, so a wrong unit fails here.
    let event = live_error(POSTGRES_ENV, "SELECT '\u{1F600}'\nFROM nope");
    assert_eq!(event["position"], 17, "{event}");
}

#[test]
fn mysql_reports_the_start_of_the_line() {
    if !live("QH_TEST_MYSQL", "deploy/dev/up.sh mysql") {
        return;
    }
    let event = live_error(MYSQL_ENV, "SELEC 1");
    assert_eq!(event["position"], 1, "{event}");
    // Only the line is known, so the mark is the first scalar of line 2: the 12th, behind ten
    // scalars and a newline.
    let event = live_error(MYSQL_ENV, "SELECT '\u{1F600}'\nSELEC 2");
    assert_eq!(event["position"], 12, "{event}");
    // The server strips leading whitespace before it counts, so its "line 1" is `SELEC` here: the
    // mark is added back to the 3rd scalar, not left on the first.
    let event = live_error(MYSQL_ENV, "\n\nSELEC 1");
    assert_eq!(event["position"], 3, "{event}");
    let event = live_error(MYSQL_ENV, "  \n  SELECT '\u{1F600}'\nSELEC 2");
    assert_eq!(event["position"], 17, "{event}");
}

#[test]
fn trino_reports_the_line_and_column_it_names() {
    if !live("QH_TEST_TRINO", "deploy/dev/up.sh trino") {
        return;
    }
    let event = live_error(TRINO_ENV, "SELECT * FRM nation");
    assert_eq!(event["position"], 10, "{event}");
    // `line 2:6`, behind a first line of ten scalars.
    let event = live_error(TRINO_ENV, "SELECT '\u{1F600}'\nFROM nope_nope");
    assert_eq!(event["position"], 17, "{event}");
}
