//! The execution log: every decision the engine made, chained and credential-free.
//!
//! One test, because the log's sink is process-scoped: installing a second database
//! mid-file would make two tests race over one process. Everything a reader needs to see
//! is asserted in one pass, against a real `qh_storage` database that lives only in memory.

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use qh_core::{EngineError, FailureKind};
use qh_driver::{ConnectionConfig, Driver, DriverKind, Session};
use qh_ffi::events::Capture;
use qh_ffi::{execution_log, run, CancelFlag, Command, Engine, RealEngine, Settings};
use qh_storage::{hash_statement, Storage};

/// An engine that counts connects and always fails them, so "reached the connection" is a
/// fact a test can assert without a server.
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

/// A fresh, migrated database that exists only in memory.
fn log_database() -> Storage {
    let mut storage = Storage::in_memory().expect("in-memory database");
    storage.migrate_at(1_000).expect("migrations");
    storage
}

const CONNECTION: [(&str, &str); 5] = [
    ("DB_KIND", "trino"),
    ("DB_HOST", "coordinator.invalid"),
    ("DB_PORT", "8080"),
    ("DB_USER", "queryhive"),
    ("RETRIES", "0"),
];

async fn drive(engine: &CountingEngine, extra: &[(&str, &str)]) -> Result<(), qh_ffi::CliError> {
    let mut pairs: Vec<(&str, &str)> = CONNECTION.to_vec();
    pairs.extend_from_slice(extra);
    let mut out = Capture::new();
    run(
        Command::Preview,
        &settings(&pairs),
        &mut out,
        engine,
        &CancelFlag::new(),
    )
    .await
}

#[tokio::test]
async fn every_decision_is_chained_hashed_and_free_of_the_statement() {
    execution_log::install(log_database());

    let engine = CountingEngine::new();
    // 1. A refusal decided before the connection is touched.
    let error = drive(
        &engine,
        &[("SAFE_MODE", "read_only"), ("SQL", "DROP TABLE people")],
    )
    .await
    .expect_err("read_only refuses DDL");
    assert!(error.to_string().contains("read-only"), "{error}");
    assert_eq!(engine.connects(), 0);

    // 2. An allow that reaches the connection, which the counting engine fails on purpose.
    drive(
        &engine,
        &[("SAFE_MODE", "read_only"), ("SQL", "SELECT * FROM people")],
    )
    .await
    .expect_err("the counting engine always fails connect");
    assert_eq!(engine.connects(), 1);

    // 3. A `confirm` write is a question until the caller confirms.
    let refusal = drive(
        &engine,
        &[
            ("SAFE_MODE", "confirm"),
            ("SQL", "INSERT INTO people VALUES (1)"),
        ],
    )
    .await
    .expect_err("confirm asks first");
    assert_eq!(engine.connects(), 1, "asking is not running");
    assert!(
        refusal.to_string().contains("requires confirmation"),
        "{refusal}"
    );

    // 4. …and it runs once the caller answers.
    drive(
        &engine,
        &[
            ("SAFE_MODE", "confirm"),
            ("SAFE_MODE_CONFIRMED", "1"),
            ("SQL", "INSERT INTO people VALUES (1)"),
        ],
    )
    .await
    .expect_err("the counting engine always fails connect");
    assert_eq!(engine.connects(), 2, "a confirmed write ran");

    // 5. A floor raises the mode for the run: `full` plus a read-only connection is
    //    read-only, and still refuses the `DROP` before the connection.
    let floored = drive(
        &engine,
        &[
            ("SAFE_MODE", "full"),
            ("DB_READ_ONLY", "1"),
            ("SQL", "DROP TABLE people"),
        ],
    )
    .await
    .expect_err("the floor raises full to read_only");
    assert!(floored.to_string().contains("read-only"), "{floored}");
    assert_eq!(engine.connects(), 2, "refused before connect");

    execution_log::with_storage(|storage| {
        assert_eq!(storage.verify_execution_log().unwrap(), 5, "five decisions");
        // Newest first: the floored refusal, the confirmation question, its answer, the
        // read, the refused DDL.
        let rows = storage.execution_log(10).unwrap();
        let decisions: Vec<&str> = rows.iter().map(|row| row.decision.as_str()).collect();
        assert_eq!(
            decisions,
            vec![
                "refused",
                "confirmed",
                "needs_confirmation",
                "allowed",
                "refused"
            ]
        );
        // The floor's effective mode is what the row records, not the user's `full`.
        assert_eq!(rows[0].safe_mode, "read_only");
        // A confirmation records the mode it actually enforced.
        assert_eq!(rows[1].safe_mode, "confirm");
        assert_eq!(rows[2].safe_mode, "confirm");
        // Each statement appears as its SHA-256 and never as text.
        let expected = [
            hash_statement("DROP TABLE people"),
            hash_statement("INSERT INTO people VALUES (1)"),
            hash_statement("INSERT INTO people VALUES (1)"),
            hash_statement("SELECT * FROM people"),
            hash_statement("DROP TABLE people"),
        ];
        let hashes: Vec<&str> = rows.iter().map(|row| row.statement_hash.as_str()).collect();
        assert_eq!(hashes, expected);
        for hash in &hashes {
            assert_eq!(hash.len(), 64, "a hex SHA-256: {hash}");
            assert!(!hash.contains("DROP"), "not the statement: {hash}");
            assert!(!hash.contains(' '), "a digest has no spaces: {hash}");
        }
        // The reason is the classifier's own sentence, never the SQL.
        assert!(rows[0].reason.as_deref().unwrap().contains("read-only"));
    })
    .expect("a sink is installed");

    execution_log::uninstall();
    assert!(execution_log::with_storage(|_| ()).is_none());
}
