//! Safe Mode is the engine's rule, not the app's.
//!
//! Every test here drives `qh_ffi::run` — the same entry point the CLI and the MCP
//! server use — with an engine that **fails the moment it is asked to connect** and
//! counts how many times it was asked. A refusal that happens before the count moves
//! is a refusal the engine made; a refusal that leaves the count at zero could not have
//! come from the server, because the server was never reached.
//!
//! Nothing here needs a database: the classifier runs on the caller's text, and that is
//! the point of it living in the engine. `tests/real_server.rs` is where a live server
//! proves the timeout and the plumbing that needs one.

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use qh_core::{EngineError, FailureKind};
use qh_driver::{ConnectionConfig, Driver, DriverKind, Session};
use qh_ffi::events::Capture;
use qh_ffi::{run, CancelFlag, CliError, Command, Engine, RealEngine, Settings};

/// An engine that counts connects and always fails them.
///
/// The real drivers answer `driver()`, so `db_drivers`, the slot list and the default
/// port are the ones production uses; only `connect` is replaced, and only so that
/// reaching it is visible.
struct CountingEngine {
    inner: RealEngine,
    connects: AtomicUsize,
}

impl CountingEngine {
    fn new() -> Self {
        Self {
            inner: RealEngine::new(),
            connects: AtomicUsize::new(0),
        }
    }

    fn connects(&self) -> usize {
        self.connects.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Engine for CountingEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        self.inner.kinds()
    }

    fn driver(&self, kind: DriverKind) -> &dyn Driver {
        self.inner.driver(kind)
    }

    async fn connect(&self, _config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        Err(EngineError::Connect {
            message: "the engine reached connect".to_owned(),
            kind: FailureKind::Permanent,
        })
    }
}

fn settings(pairs: &[(&str, &str)]) -> Settings {
    Settings::from_pairs(pairs.iter().map(|(key, value)| (*key, *value)))
}

/// A connection that would be usable, so a refusal is about Safe Mode and not about a
/// missing host.
const CONNECTION: [(&str, &str); 5] = [
    ("DB_KIND", "trino"),
    ("DB_HOST", "coordinator.invalid"),
    ("DB_PORT", "8080"),
    ("DB_USER", "queryhive"),
    ("RETRIES", "0"),
];

/// Run one command and return its failure, or panic if it did not fail.
async fn refuse(command: Command, engine: &CountingEngine, extra: &[(&str, &str)]) -> CliError {
    let mut pairs: Vec<(&str, &str)> = CONNECTION.to_vec();
    pairs.extend_from_slice(extra);
    let mut out = Capture::new();
    run(
        command,
        &settings(&pairs),
        &mut out,
        engine,
        &CancelFlag::new(),
    )
    .await
    .expect_err("the command should have been refused")
}

fn usage_message(error: &CliError) -> String {
    match error {
        CliError::Usage(message) => message.clone(),
        other => panic!("expected a usage refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn read_only_refuses_a_drop_before_the_engine_connects() {
    let engine = CountingEngine::new();
    let error = refuse(
        Command::Preview,
        &engine,
        &[("SAFE_MODE", "read_only"), ("SQL", "DROP TABLE people")],
    )
    .await;
    let message = usage_message(&error);
    assert!(message.contains("read-only"), "{message}");
    assert!(message.contains("statement 1"), "{message}");
    assert!(message.contains("DROP TABLE people"), "{message}");
    assert_eq!(
        engine.connects(),
        0,
        "a refusal the server never saw is the engine's own"
    );
}

#[tokio::test]
async fn read_only_refuses_to_tables_replace_in_the_engine() {
    // The mode the app offers is not what enforces this: `to_table`'s `replace` runs a
    // `DROP` before its query, and the generated statements are what the guard reads.
    let engine = CountingEngine::new();
    let error = refuse(
        Command::ToTable,
        &engine,
        &[
            ("SAFE_MODE", "read_only"),
            ("SQL", "SELECT 1"),
            ("TARGET_CATALOG", "hive"),
            ("TARGET_SCHEMA", "default"),
            ("TARGET_TABLE", "people"),
            ("WRITE_MODE", "replace"),
        ],
    )
    .await;
    let message = usage_message(&error);
    assert!(message.contains("read-only"), "{message}");
    assert!(message.contains("DDL"), "{message}");
    assert_eq!(engine.connects(), 0);
}

#[tokio::test]
async fn no_ddl_refuses_ddl_but_lets_a_write_reach_the_connection() {
    let engine = CountingEngine::new();
    // DDL is refused without connecting.
    let error = refuse(
        Command::Preview,
        &engine,
        &[("SAFE_MODE", "no_ddl"), ("SQL", "DROP TABLE people")],
    )
    .await;
    assert!(usage_message(&error).contains("no_ddl"), "{error:?}");
    assert_eq!(engine.connects(), 0);

    // A write is allowed by `no_ddl`, so the run gets as far as `connect` — which this
    // engine fails on purpose. Reaching it is the proof.
    let error = refuse(
        Command::Preview,
        &engine,
        &[
            ("SAFE_MODE", "no_ddl"),
            ("SQL", "INSERT INTO people VALUES (1)"),
        ],
    )
    .await;
    assert!(
        matches!(error, CliError::Connect(_)),
        "no_ddl should let a write through to the connection, got {error:?}"
    );
    assert_eq!(engine.connects(), 1);
}

#[tokio::test]
async fn read_only_lets_a_read_reach_the_connection() {
    let engine = CountingEngine::new();
    let error = refuse(
        Command::Preview,
        &engine,
        &[("SAFE_MODE", "read_only"), ("SQL", "SELECT * FROM people")],
    )
    .await;
    assert!(matches!(error, CliError::Connect(_)), "{error:?}");
    assert_eq!(engine.connects(), 1);
}

#[tokio::test]
async fn full_is_the_default_and_refuses_nothing() {
    // The engine's default, which is what keeps the CLI and the golden corpus
    // unchanged: no `SAFE_MODE` means `full`.
    let engine = CountingEngine::new();
    let error = refuse(Command::Preview, &engine, &[("SQL", "DROP TABLE people")]).await;
    assert!(matches!(error, CliError::Connect(_)), "{error:?}");
    assert_eq!(engine.connects(), 1);
}

#[tokio::test]
async fn an_unknown_safe_mode_is_refused_by_name() {
    let engine = CountingEngine::new();
    let error = refuse(
        Command::Preview,
        &engine,
        &[("SAFE_MODE", "maybe"), ("SQL", "SELECT 1")],
    )
    .await;
    assert_eq!(
        usage_message(&error),
        "unknown SAFE_MODE 'maybe'; expected full, no_ddl, confirm, read_only"
    );
    assert_eq!(engine.connects(), 0);
}

#[tokio::test]
async fn confirm_refuses_a_write_until_the_caller_confirms_it() {
    let engine = CountingEngine::new();
    // A write is a question, and the question is asked before the connection.
    let error = refuse(
        Command::Preview,
        &engine,
        &[
            ("SAFE_MODE", "confirm"),
            ("SQL", "INSERT INTO people VALUES (1)"),
        ],
    )
    .await;
    let message = usage_message(&error);
    assert!(message.contains("requires confirmation"), "{message}");
    assert!(message.contains("statement 1"), "{message}");
    assert_eq!(engine.connects(), 0);

    // The same run with the caller's confirmation reaches the connection — which this
    // engine fails on purpose, so reaching it is the proof.
    let error = refuse(
        Command::Preview,
        &engine,
        &[
            ("SAFE_MODE", "confirm"),
            ("SAFE_MODE_CONFIRMED", "1"),
            ("SQL", "INSERT INTO people VALUES (1)"),
        ],
    )
    .await;
    assert!(matches!(error, CliError::Connect(_)), "{error:?}");
    assert_eq!(engine.connects(), 1);

    // DDL and the unreadable stay refused even with a confirmation.
    let engine = CountingEngine::new();
    let error = refuse(
        Command::Preview,
        &engine,
        &[
            ("SAFE_MODE", "confirm"),
            ("SAFE_MODE_CONFIRMED", "1"),
            ("SQL", "DROP TABLE people"),
        ],
    )
    .await;
    assert!(usage_message(&error).contains("confirm"), "{error:?}");
    assert_eq!(engine.connects(), 0);
}

#[tokio::test]
async fn a_read_only_connection_floor_raises_a_full_setting() {
    // The user chose `full`; the connection is marked read-only. The run is read-only, and
    // the refusal happens before the connection is opened. Nothing is written back: the
    // same settings without `DB_READ_ONLY` run the write.
    let engine = CountingEngine::new();
    let error = refuse(
        Command::Preview,
        &engine,
        &[
            ("SAFE_MODE", "full"),
            ("DB_READ_ONLY", "1"),
            ("SQL", "DROP TABLE people"),
        ],
    )
    .await;
    assert!(usage_message(&error).contains("read-only"), "{error:?}");
    assert_eq!(engine.connects(), 0);

    let error = refuse(
        Command::Preview,
        &engine,
        &[("SAFE_MODE", "full"), ("SQL", "DROP TABLE people")],
    )
    .await;
    assert!(
        matches!(error, CliError::Connect(_)),
        "without the floor, full still runs it: {error:?}"
    );
    assert_eq!(engine.connects(), 1);
}

#[tokio::test]
async fn a_policy_floor_is_the_stricter_of_two_conditions() {
    // The user chose `no_ddl` and a policy pinned `read_only`. The floor must pick the
    // strictest, not the first condition, so a write is refused outright rather than
    // reaching the connection.
    let engine = CountingEngine::new();
    let error = refuse(
        Command::Preview,
        &engine,
        &[
            ("SAFE_MODE", "no_ddl"),
            ("SAFE_MODE_FLOOR", "read_only"),
            ("SQL", "INSERT INTO people VALUES (1)"),
        ],
    )
    .await;
    assert!(usage_message(&error).contains("read-only"), "{error:?}");
    assert_eq!(engine.connects(), 0);

    // A floor spelling nobody knows is refused by its own setting's name.
    let error = refuse(
        Command::Preview,
        &engine,
        &[("SAFE_MODE_FLOOR", "maybe"), ("SQL", "SELECT 1")],
    )
    .await;
    assert_eq!(
        usage_message(&error),
        "unknown SAFE_MODE_FLOOR 'maybe'; expected full, no_ddl, confirm, read_only"
    );
    assert_eq!(engine.connects(), 0);
}

#[tokio::test]
async fn count_is_gated_on_the_callers_statement_not_the_wrapper() {
    // `count` wraps the statement in `SELECT COUNT(*)`, which would happily run a
    // data-modifying CTE. The guard reads what the caller asked for.
    let engine = CountingEngine::new();
    let error = refuse(
        Command::Count,
        &engine,
        &[
            ("SAFE_MODE", "read_only"),
            (
                "SQL",
                "WITH gone AS (DELETE FROM people RETURNING *) SELECT * FROM gone",
            ),
        ],
    )
    .await;
    assert!(usage_message(&error).contains("read-only"), "{error:?}");
    assert_eq!(engine.connects(), 0);
}

#[tokio::test]
async fn export_and_explain_are_gated_too() {
    let engine = CountingEngine::new();
    let error = refuse(
        Command::Explain,
        &engine,
        &[("SAFE_MODE", "read_only"), ("SQL", "DROP TABLE people")],
    )
    .await;
    assert!(usage_message(&error).contains("read-only"), "{error:?}");
    assert_eq!(engine.connects(), 0);

    let engine = CountingEngine::new();
    let error = refuse(
        Command::Export,
        &engine,
        &[
            ("SAFE_MODE", "read_only"),
            ("SQL", "DROP TABLE people"),
            ("FORMAT", "csv"),
            ("OUT_DIR", "/tmp"),
        ],
    )
    .await;
    assert!(usage_message(&error).contains("read-only"), "{error:?}");
    assert_eq!(engine.connects(), 0);
}

#[tokio::test]
async fn a_negative_statement_timeout_is_refused_by_name() {
    let engine = CountingEngine::new();
    let error = refuse(
        Command::Preview,
        &engine,
        &[("STATEMENT_TIMEOUT_MS", "-1"), ("SQL", "SELECT 1")],
    )
    .await;
    assert!(error.message().contains("cannot be negative"), "{error:?}");
    assert_eq!(engine.connects(), 0);
}

#[tokio::test]
async fn an_unparseable_statement_timeout_is_refused_by_name() {
    let engine = CountingEngine::new();
    let error = refuse(
        Command::Preview,
        &engine,
        &[("STATEMENT_TIMEOUT_MS", "soon"), ("SQL", "SELECT 1")],
    )
    .await;
    let message = error.to_string();
    assert!(
        message.contains("STATEMENT_TIMEOUT_MS must be a whole number"),
        "{message}"
    );
    assert_eq!(engine.connects(), 0);
}

#[tokio::test]
async fn a_multi_statement_script_names_the_statement_it_refuses() {
    let engine = CountingEngine::new();
    let error = refuse(
        Command::Preview,
        &engine,
        &[
            ("SAFE_MODE", "read_only"),
            ("SQL", "SELECT 1; DROP TABLE people; SELECT 2"),
        ],
    )
    .await;
    let message = usage_message(&error);
    assert!(message.contains("statement 2"), "{message}");
    assert!(message.contains("DROP TABLE people"), "{message}");
    assert_eq!(engine.connects(), 0);
}
