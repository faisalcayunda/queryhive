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
use qh_core::{ColumnBatch, ColumnMeta, EngineError, FailureKind};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions,
    ObjectPath, ObjectsPage, Parameter, Session,
};
use qh_ffi::events::Capture;
use qh_ffi::{run, CancelFlag, Command, Engine, Settings};

/// What the fake session remembers and reports.
#[derive(Clone, Default)]
struct Recording {
    statements: Arc<Mutex<Vec<String>>>,
    /// The values each `execute_bound` call carried, in call order. Empty for the
    /// calls that were not parameterized (transaction control).
    parameters: Arc<Mutex<Vec<Vec<Parameter>>>>,
    /// The affected-row count each `execute` reports, in order. Empty means
    /// `Some(1)` for every statement, which is a plan that agrees with itself.
    counts: Arc<Mutex<VecDeque<Option<u64>>>>,
    /// When set, `ROLLBACK` fails the way a server that dropped the connection
    /// would. The statements that ran may already be in the table, which is the
    /// distinction between `written` and `pendingInSessionTransaction`.
    fail_rollback: Arc<Mutex<bool>>,
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
            parameters: None,
            read_only: false,
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
        if sql.trim().eq_ignore_ascii_case("ROLLBACK")
            && *self.recording.fail_rollback.lock().expect("rollback")
        {
            return Err(EngineError::Query {
                message: "the connection went away before the rollback".to_owned(),
                code: None,
                position: None,
                kind: FailureKind::Transient,
            });
        }
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

    /// A driver that records binding, so the test can prove the values reached the
    /// session rather than being re-inlined. It forwards to `execute` after
    /// recording, so the rest of the plan's behaviour is the text-path one.
    async fn execute_bound(
        &mut self,
        sql: &str,
        parameters: &[Parameter],
        options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        if !parameters.is_empty() {
            self.recording
                .parameters
                .lock()
                .expect("record parameters")
                .push(parameters.to_vec());
        }
        self.execute(sql, options).await
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

/// `apply` with extra settings — a plan that is asked for no transaction, most often.
async fn apply_with(
    recording: &Recording,
    changes: &str,
    extra: &[(&str, &str)],
) -> Result<Capture, qh_ffi::CliError> {
    let mut pairs: Vec<(&str, &str)> = CONNECTION.to_vec();
    pairs.push(("CHANGES", changes));
    pairs.extend_from_slice(extra);
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
async fn a_plan_with_bound_values_hands_them_to_the_session_in_order() {
    // The point of the slice: the plan carries placeholders and typed values, the
    // engine forwards them, and nothing re-inlines them on the way.
    let recording = Recording::default();
    let changes = r#"[
        {"sql":"UPDATE \"public\".\"people\" SET \"name\" = $1 WHERE \"id\" = $2",
         "expected":1,"keyed":true,
         "params":[{"type":"text","value":"O'Brien"},{"type":"int","value":"3"}]}
    ]"#;
    apply(&recording, changes).await.expect("the plan applies");

    let parameters = recording.parameters.lock().expect("parameters").clone();
    assert_eq!(parameters.len(), 1, "one parameterized statement");
    assert_eq!(
        parameters[0],
        vec![Parameter::Text("O'Brien".to_owned()), Parameter::Int(3)]
    );
    // The statement the session ran is the bound one; the transaction control
    // statements carried no values and were not recorded.
    let ran = recording.statements();
    assert!(
        ran.iter().any(|sql| sql.contains("$1")),
        "the session ran the bound statement: {ran:?}"
    );
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

#[tokio::test]
async fn a_failure_inside_a_transaction_is_pending_and_names_the_engine() {
    let recording = Recording::default();
    recording.counts.lock().expect("counts").push_back(Some(2));
    let changes = r#"[{"sql":"UPDATE \"public\".\"people\" SET \"name\" = 'z' WHERE \"id\" = 3","expected":1,"keyed":true}]"#;
    let error = apply(&recording, changes).await.expect_err("must fail");

    let message = error.message();
    assert!(message.contains("postgres"), "names the engine: {message}");
    assert!(
        message.contains("(disposition: pendingInSessionTransaction)"),
        "{message}"
    );
    assert!(
        message.contains("those rows are not in the table"),
        "the rollback undid them: {message}"
    );
    // The rollback ran; nothing committed, so there is nothing to inspect.
    let ran = recording.statements();
    assert_eq!(ran.last().map(String::as_str), Some("ROLLBACK"));
    assert!(!ran.iter().any(|sql| sql == "COMMIT"));
}

#[tokio::test]
async fn a_failure_without_a_transaction_is_written_and_says_not_to_retry() {
    // No transaction means every statement auto-committed as it ran: the rows are
    // in the table, and the advice is to look, not to run the plan again.
    let recording = Recording::default();
    recording.counts.lock().expect("counts").push_back(Some(2));
    let changes = r#"[{"sql":"UPDATE \"public\".\"people\" SET \"name\" = 'z' WHERE \"id\" = 3","expected":1,"keyed":true}]"#;
    let error = apply_with(&recording, changes, &[("IN_TRANSACTION", "false")])
        .await
        .expect_err("must fail");

    let message = error.message();
    assert!(message.contains("postgres"), "{message}");
    assert!(message.contains("(disposition: written)"), "{message}");
    assert!(message.contains("there was no transaction"), "{message}");
    assert!(message.contains("not a retry"), "{message}");
    assert!(message.contains("look at the table"), "{message}");
    // The statement completed — it just affected the wrong number of rows — so
    // the count says one of one, not zero.
    assert!(
        message.contains("1 of 1 statement(s) had completed"),
        "{message}"
    );
    let ran = recording.statements();
    assert!(!ran.iter().any(|sql| sql == "BEGIN"), "{ran:?}");
    assert!(!ran.iter().any(|sql| sql == "ROLLBACK"), "{ran:?}");
}

#[tokio::test]
async fn a_rollback_that_failed_is_written() {
    // The rollback is not a formality: when it fails, the rows may still be in
    // the table, and calling that `pending` would invite a retry that writes
    // twice.
    let recording = Recording::default();
    *recording.fail_rollback.lock().expect("rollback") = true;
    recording.counts.lock().expect("counts").push_back(Some(2));
    let changes = r#"[{"sql":"UPDATE t SET a=1 WHERE id=1","expected":1,"keyed":true}]"#;
    let error = apply(&recording, changes).await.expect_err("must fail");

    let message = error.message();
    assert!(message.contains("(disposition: written)"), "{message}");
    assert!(message.contains("could not be undone cleanly"), "{message}");
    assert!(message.contains("may still be in the table"), "{message}");
    let ran = recording.statements();
    assert_eq!(ran.last().map(String::as_str), Some("ROLLBACK"));
}

#[tokio::test]
async fn a_committed_plan_reports_itself_as_written() {
    let recording = Recording::default();
    let events = apply(
        &recording,
        r#"[{"sql":"UPDATE t SET a=1 WHERE id=1","expected":1,"keyed":true}]"#,
    )
    .await
    .expect("the plan applies");
    let done = events
        .lines()
        .iter()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("JSON"))
        .find(|event| event["event"] == "done")
        .expect("a done event");
    // Committed by the time `done` is emitted, so the plan is in the table.
    assert_eq!(done["disposition"], "written");
    assert_eq!(done["transaction"], true);
}
