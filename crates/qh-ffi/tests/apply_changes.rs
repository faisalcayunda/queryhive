//! `apply_changes` runs exactly the plan it was handed, in order, and rolls the
//! whole thing back when a write's row count disagrees with the plan.
//!
//! The proof this file exists for is the one the phase asks for: **the statement
//! that was reviewed is the statement that ran.** The session here records every
//! statement it is given, so the recorded list can be held against the `CHANGES`
//! JSON byte for byte — and the fact that the recorded list is what the server
//! would have received is what makes that comparison mean something.
//!
//! No database is needed: the session is a recorder, and the row counts the
//! plan is verified against are the ones it is told to report. `tests/real_server.rs`
//! is where a live PostgreSQL proves the count is the server's own.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use qh_core::{ColumnBatch, ColumnMeta, EngineError};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions,
    ObjectPath, ObjectsPage, Session,
};
use qh_ffi::events::Capture;
use qh_ffi::{run, CancelFlag, Command, Engine, Settings};

/// What the fake session remembers and reports.
#[derive(Clone, Default)]
struct Recording {
    statements: Arc<Mutex<Vec<String>>>,
    /// The affected-row count each `execute` reports, in order. Empty means
    /// `Some(1)` for every statement, which is a plan that agrees with itself.
    counts: Arc<Mutex<VecDeque<Option<u64>>>>,
}

impl Recording {
    fn statements(&self) -> Vec<String> {
        self.statements.lock().expect("recorded statements").clone()
    }
}

struct RecordingEngine {
    recording: Recording,
}

#[async_trait]
impl Engine for RecordingEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        vec![DriverKind::Postgres]
    }

    fn driver(&self, _kind: DriverKind) -> &dyn Driver {
        &FakeDriver
    }

    async fn connect(&self, _config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        Ok(Box::new(RecordingSession {
            recording: self.recording.clone(),
        }))
    }
}

struct FakeDriver;

#[async_trait]
impl Driver for FakeDriver {
    fn kind(&self) -> DriverKind {
        DriverKind::Postgres
    }

    fn label(&self) -> &'static str {
        "Recording"
    }

    fn default_port(&self) -> u16 {
        5432
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            transactions: true,
            multiple_result_sets: false,
            cancel: true,
            explain: true,
            levels: vec![BrowseLevel::Schema, BrowseLevel::Table],
            objects_columns: vec![],
            persistent_connection: true,
            statement_timeout: true,
        }
    }

    async fn connect(&self, _config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        Err(EngineError::Connect {
            message: "the recording driver is never asked to connect through here".to_owned(),
            kind: qh_core::FailureKind::Permanent,
        })
    }
}

struct RecordingSession {
    recording: Recording,
}

#[async_trait]
impl Session for RecordingSession {
    fn capabilities(&self) -> Capabilities {
        FakeDriver.capabilities()
    }

    fn query_id(&self) -> Option<String> {
        None
    }

    async fn execute(
        &mut self,
        sql: &str,
        _options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        self.recording
            .statements
            .lock()
            .expect("record")
            .push(sql.to_owned());
        // Transaction control is not one of the plan's statements and does not
        // consume a scripted count; its affected count is never read.
        let control = matches!(
            sql.trim().to_ascii_uppercase().as_str(),
            "BEGIN" | "COMMIT" | "ROLLBACK"
        );
        let affected = if control {
            Some(0)
        } else {
            self.recording
                .counts
                .lock()
                .expect("counts")
                .pop_front()
                .flatten()
                .or(Some(1))
        };
        Ok(Box::new(RecordingCursor { affected }))
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
        format!("EXPLAIN {sql}")
    }

    async fn cancel(&self) -> Result<(), EngineError> {
        Ok(())
    }

    async fn close(self: Box<Self>) -> Result<(), EngineError> {
        Ok(())
    }
}

struct RecordingCursor {
    affected: Option<u64>,
}

#[async_trait]
impl Cursor for RecordingCursor {
    fn columns(&self) -> &[ColumnMeta] {
        &[]
    }

    async fn next_batch(&mut self, _max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        Ok(None)
    }

    fn affected_rows(&self) -> Option<u64> {
        self.affected
    }
}

fn settings(pairs: &[(&str, &str)]) -> Settings {
    Settings::from_pairs(pairs.iter().map(|(key, value)| (*key, *value)))
}

const CONNECTION: [(&str, &str); 4] = [
    ("DB_KIND", "postgres"),
    ("DB_HOST", "db.invalid"),
    ("DB_USER", "queryhive"),
    ("RETRIES", "0"),
];

async fn apply(recording: &Recording, changes: &str) -> Result<Capture, qh_ffi::CliError> {
    let mut pairs: Vec<(&str, &str)> = CONNECTION.to_vec();
    pairs.push(("CHANGES", changes));
    let mut out = Capture::new();
    run(
        Command::ApplyChanges,
        &settings(&pairs),
        &mut out,
        &RecordingEngine {
            recording: recording.clone(),
        },
        &CancelFlag::new(),
    )
    .await?;
    Ok(out)
}

#[tokio::test]
async fn the_statements_that_ran_are_the_statements_that_were_reviewed() {
    // The phase's own criterion: what was approved is what ran, byte for byte.
    let recording = Recording::default();
    let reviewed = r#"[
        {"sql":"UPDATE \"public\".\"people\" SET \"name\" = 'z' WHERE \"id\" = 3","expected":1,"keyed":true},
        {"sql":"DELETE FROM \"public\".\"people\" WHERE \"id\" = 4","expected":1,"keyed":false}
    ]"#;
    let events = apply(&recording, reviewed).await.expect("the plan applies");

    let ran = recording.statements();
    assert_eq!(
        ran,
        vec![
            "BEGIN".to_owned(),
            "UPDATE \"public\".\"people\" SET \"name\" = 'z' WHERE \"id\" = 3".to_owned(),
            "DELETE FROM \"public\".\"people\" WHERE \"id\" = 4".to_owned(),
            "COMMIT".to_owned(),
        ],
        "the reviewed statements are the ones the session was given, in order"
    );
    let lines = events.lines();
    let done = lines
        .iter()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON"))
        .find(|event| event["event"] == "done")
        .expect("a done event");
    assert_eq!(done["applied"], 2);
    assert_eq!(done["transaction"], true);
}

#[tokio::test]
async fn a_keyed_write_that_matched_more_than_the_plan_is_rolled_back() {
    let recording = Recording::default();
    // The one statement reports two affected rows where the plan expected one:
    // the predicate matched more than the row it was built from.
    recording.counts.lock().expect("counts").push_back(Some(2));
    let changes = r#"[{"sql":"UPDATE \"public\".\"people\" SET \"name\" = 'z' WHERE \"id\" = 3","expected":1,"keyed":true}]"#;
    let error = apply(&recording, changes).await.expect_err("must fail");

    assert!(
        error
            .message()
            .contains("affected 2 row(s) but the plan expected 1"),
        "{error:?}"
    );
    // The rollback ran, and no commit did.
    let ran = recording.statements();
    assert_eq!(ran.last().map(String::as_str), Some("ROLLBACK"));
    assert!(!ran.iter().any(|sql| sql == "COMMIT"));
}

#[tokio::test]
async fn a_keyed_write_that_matched_fewer_is_allowed() {
    // MySQL reports zero for an update that sets a value it already held. That is
    // a save that worked, so `0 < 1` must not be a failure for a keyed write.
    let recording = Recording::default();
    recording.counts.lock().expect("counts").push_back(Some(0));
    let changes = r#"[{"sql":"UPDATE \"public\".\"people\" SET \"name\" = 'z' WHERE \"id\" = 3","expected":1,"keyed":true}]"#;
    apply(&recording, changes)
        .await
        .expect("a no-op update is not a failure");
    assert_eq!(
        recording.statements().last().map(String::as_str),
        Some("COMMIT")
    );
}

#[tokio::test]
async fn a_keyless_write_that_matched_fewer_is_a_failure() {
    // Without a key the predicate is every column, so a row that is gone is a
    // real disagreement: fewer affected rows than expected.
    let recording = Recording::default();
    recording.counts.lock().expect("counts").push_back(Some(0));
    let changes =
        r#"[{"sql":"DELETE FROM \"people\" WHERE \"id\" = 9","expected":1,"keyed":false}]"#;
    let error = apply(&recording, changes).await.expect_err("must fail");
    assert!(error.message().contains("affected 0 row(s)"), "{error:?}");
}

#[tokio::test]
async fn a_read_only_connection_refuses_the_plan_before_anything_runs() {
    let recording = Recording::default();
    let mut pairs: Vec<(&str, &str)> = CONNECTION.to_vec();
    pairs.push(("CHANGES", r#"[{"sql":"DROP TABLE people"}]"#));
    pairs.push(("SAFE_MODE", "read_only"));
    let mut out = Capture::new();
    let error = run(
        Command::ApplyChanges,
        &settings(&pairs),
        &mut out,
        &RecordingEngine {
            recording: recording.clone(),
        },
        &CancelFlag::new(),
    )
    .await
    .expect_err("the plan must be refused");
    assert!(
        error.message().contains("change 1 was refused"),
        "{error:?}"
    );
    // Nothing reached the session, not even the `BEGIN`.
    assert!(
        recording.statements().is_empty(),
        "{:?}",
        recording.statements()
    );
}

#[tokio::test]
async fn an_unknown_expected_count_is_refused_rather_than_ignored() {
    let recording = Recording::default();
    let error = apply(
        &recording,
        r#"[{"sql":"UPDATE t SET a=1","expected":"one"}]"#,
    )
    .await
    .expect_err("must be refused");
    assert!(
        error.message().contains("expected is not an integer"),
        "{error:?}"
    );
}
