//! `import_data` on a `.sql` file: the statement family.
//!
//! The row family already has its own proof. This file exists for the two things
//! the statement family adds and that a screenshot cannot show:
//!
//! * **the statements that ran are the ones `qh-sql` split**, in file order, with
//!   a `;` inside a literal still text — the same scanner the classifier uses,
//!   never a second splitter; and
//! * **the foreign-key switch goes back on at both exits**, commit and rollback
//!   alike, which is the step the source study says is easy to leave half-done.
//!
//! The session here is a recorder: it keeps every statement it is given and fails
//! the ones a test asks it to. No database is needed, so the error modes, the
//! line numbers and the epilogue ordering are pinned deterministically.
//! `tests/import_live.rs` is where a real PostgreSQL proves the switch itself.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use qh_core::{ColumnBatch, ColumnMeta, EngineError, FailureKind};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions,
    ObjectPath, ObjectsPage, Session,
};
use qh_ffi::events::Capture;
use qh_ffi::{run, CancelFlag, Command, Engine, Settings};

/// What the fake session remembers and which statements it refuses.
#[derive(Clone, Default)]
struct Recording {
    statements: Arc<Mutex<Vec<String>>>,
    /// Substrings that make the statement carrying them fail, as a scripted
    /// server error.
    failures: Arc<Mutex<Vec<String>>>,
}

impl Recording {
    fn statements(&self) -> Vec<String> {
        self.statements.lock().expect("recorded statements").clone()
    }

    fn fail_on(&self, needle: &str) {
        self.failures
            .lock()
            .expect("failures")
            .push(needle.to_owned());
    }
}

#[derive(Clone)]
struct FakeDriver {
    kind: DriverKind,
    transactions: bool,
}

#[async_trait]
impl Driver for FakeDriver {
    fn kind(&self) -> DriverKind {
        self.kind
    }

    fn label(&self) -> &'static str {
        "Recording"
    }

    fn default_port(&self) -> u16 {
        5432
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            transactions: self.transactions,
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
            kind: FailureKind::Permanent,
        })
    }
}

struct RecordingEngine {
    recording: Recording,
    driver: FakeDriver,
}

#[async_trait]
impl Engine for RecordingEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        vec![self.driver.kind]
    }

    fn driver(&self, _kind: DriverKind) -> &dyn Driver {
        &self.driver
    }

    async fn connect(&self, _config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        Ok(Box::new(RecordingSession {
            recording: self.recording.clone(),
            driver: self.driver.clone(),
        }))
    }
}

struct RecordingSession {
    recording: Recording,
    driver: FakeDriver,
}

#[async_trait]
impl Session for RecordingSession {
    fn capabilities(&self) -> Capabilities {
        self.driver.capabilities()
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
        // Transaction control and the foreign-key switch are the import's own
        // bookkeeping; the scripted failures are aimed at the file's statements.
        let control = is_control(sql);
        if !control {
            let failed = self
                .recording
                .failures
                .lock()
                .expect("failures")
                .iter()
                .any(|needle| sql.contains(needle));
            if failed {
                return Err(EngineError::Query {
                    message: format!("the server refused: {sql}"),
                    code: None,
                    position: None,
                    kind: FailureKind::Permanent,
                });
            }
        }
        Ok(Box::new(RecordingCursor {
            affected: Some(if control { 0 } else { 1 }),
        }))
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

/// Transaction control and the FK switch, rather than a statement from the file.
fn is_control(sql: &str) -> bool {
    let upper = sql.trim().to_ascii_uppercase();
    matches!(upper.as_str(), "BEGIN" | "COMMIT" | "ROLLBACK") || upper.starts_with("SET ")
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

/// A `.sql` file in the temp directory, named for the process and the test.
fn script(name: &str, text: &str) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("qh-import-sql-{}-{name}.sql", std::process::id()));
    std::fs::write(&path, text).expect("write the script");
    path
}

/// Run `import_data` over `path` against the recorder.
async fn import(
    recording: &Recording,
    kind: DriverKind,
    transactions: bool,
    path: &Path,
    extra: &[(&str, &str)],
) -> Result<Capture, qh_ffi::CliError> {
    let mut pairs: Vec<(&str, &str)> = vec![
        ("DB_KIND", kind_name(kind)),
        ("DB_HOST", "db.invalid"),
        ("DB_USER", "queryhive"),
        ("RETRIES", "0"),
    ];
    pairs.push(("IMPORT_PATH", path.to_str().expect("utf-8 path")));
    pairs.extend_from_slice(extra);
    let mut out = Capture::new();
    run(
        Command::ImportData,
        &settings(&pairs),
        &mut out,
        &RecordingEngine {
            recording: recording.clone(),
            driver: FakeDriver { kind, transactions },
        },
        &CancelFlag::new(),
    )
    .await?;
    Ok(out)
}

fn kind_name(kind: DriverKind) -> &'static str {
    match kind {
        DriverKind::Trino => "trino",
        DriverKind::Postgres => "postgres",
        DriverKind::Mysql => "mysql",
    }
}

fn done(events: &Capture) -> serde_json::Value {
    events
        .lines
        .iter()
        .find(|event| event["event"] == "done")
        .cloned()
        .unwrap_or_else(|| panic!("no done event: {:?}", events.lines()))
}

#[tokio::test]
async fn the_statements_that_ran_are_the_ones_qh_sql_split() {
    // The `;` inside the literal is text, so the file is two statements, not
    // three. The scanner is qh-sql's, and this pins that the importer uses it.
    let recording = Recording::default();
    let path = script("split", "SELECT 'a;b' AS note;\nSELECT 2;\n");
    let events = import(&recording, DriverKind::Postgres, true, &path, &[])
        .await
        .expect("the script imports");

    assert_eq!(
        recording.statements(),
        vec![
            "BEGIN".to_owned(),
            "SELECT 'a;b' AS note".to_owned(),
            "SELECT 2".to_owned(),
            "COMMIT".to_owned(),
        ]
    );
    let done = done(&events);
    assert_eq!(done["statements"], 2);
    assert_eq!(done["transaction"], true);
    assert_eq!(done["format"], "sql");
    assert_eq!(done["foreign_keys"], "on");
}

#[tokio::test]
async fn a_go_line_is_not_a_separator_because_it_is_not_sql() {
    // The decision, pinned: qh-sql splits on `;` alone. A `GO` line is part of the
    // statement around it and reaches the server, which is honest — no second
    // splitter was written to honour a client command none of these drivers has.
    let recording = Recording::default();
    let path = script("go", "SELECT 1\nGO\n");
    import(&recording, DriverKind::Postgres, true, &path, &[])
        .await
        .expect("the script imports");
    assert_eq!(
        recording.statements(),
        vec![
            "BEGIN".to_owned(),
            "SELECT 1\nGO".to_owned(),
            "COMMIT".to_owned()
        ]
    );
}

#[tokio::test]
async fn a_stop_mode_failure_rolls_back_and_names_the_line() {
    let recording = Recording::default();
    recording.fail_on("boom");
    let path = script("stop", "SELECT 1;\nSELECT 'boom';\nSELECT 3;\n");
    let error = import(&recording, DriverKind::Postgres, true, &path, &[])
        .await
        .expect_err("stop is the default and it stops");

    assert!(
        error.message().contains("stopped at line 2"),
        "the line is named: {}",
        error.message()
    );
    assert!(
        error.message().contains("rolled back"),
        "{}",
        error.message()
    );
    let ran = recording.statements();
    assert_eq!(
        ran,
        vec![
            "BEGIN".to_owned(),
            "SELECT 1".to_owned(),
            "SELECT 'boom'".to_owned(),
            "ROLLBACK".to_owned(),
        ],
        "the third statement never ran, and the rollback is the last thing"
    );
}

#[tokio::test]
async fn a_commit_mode_keeps_the_prefix_and_does_not_roll_back() {
    let recording = Recording::default();
    recording.fail_on("boom");
    let path = script("commit", "SELECT 1;\nSELECT 'boom';\nSELECT 3;\n");
    let events = import(
        &recording,
        DriverKind::Postgres,
        true,
        &path,
        &[("ON_ERROR", "commit")],
    )
    .await
    .expect("commit keeps what worked");

    let ran = recording.statements();
    assert_eq!(
        ran,
        vec![
            "BEGIN".to_owned(),
            "SELECT 1".to_owned(),
            "SELECT 'boom'".to_owned(),
            "COMMIT".to_owned(),
        ],
        "the failing statement is attempted, nothing after it runs, and the prefix commits"
    );
    let done = done(&events);
    assert_eq!(done["statements"], 1);
    assert_eq!(done["stopped_at"], 2);
    let errors = done["errors"].as_array().expect("errors");
    assert!(
        errors[0].as_str().expect("message").contains("line 2"),
        "{errors:?}"
    );
}

#[tokio::test]
async fn a_skip_mode_uses_no_transaction_and_skips_only_the_bad_statement() {
    let recording = Recording::default();
    recording.fail_on("boom");
    let path = script("skip", "SELECT 1;\nSELECT 'boom';\nSELECT 3;\n");
    let events = import(
        &recording,
        DriverKind::Postgres,
        true,
        &path,
        &[("ON_ERROR", "skip")],
    )
    .await
    .expect("skip keeps going");

    let ran = recording.statements();
    assert_eq!(
        ran,
        vec![
            "SELECT 1".to_owned(),
            "SELECT 'boom'".to_owned(),
            "SELECT 3".to_owned(),
        ],
        "no BEGIN/COMMIT/ROLLBACK, and the statement after the bad one still runs"
    );
    let done = done(&events);
    assert_eq!(done["statements"], 2);
    assert_eq!(done["transaction"], false);
    assert_eq!(done["disposition"], "written");
    assert_eq!(done["stopped_at"], serde_json::Value::Null);
}

#[tokio::test]
async fn foreign_keys_go_off_before_the_transaction_and_back_on_at_commit() {
    let recording = Recording::default();
    let path = script("fk-commit", "SELECT 1;\n");
    import(
        &recording,
        DriverKind::Postgres,
        true,
        &path,
        &[("FOREIGN_KEYS", "off")],
    )
    .await
    .expect("the import runs");

    assert_eq!(
        recording.statements(),
        vec![
            "SET session_replication_role = replica".to_owned(),
            "BEGIN".to_owned(),
            "SELECT 1".to_owned(),
            "COMMIT".to_owned(),
            "SET session_replication_role = DEFAULT".to_owned(),
        ],
        "off outside the transaction, and on again after the commit"
    );
}

#[tokio::test]
async fn foreign_keys_go_back_on_after_a_rollback_too() {
    // The step the study says is easy to leave half-done: the `stop` path rolls
    // back and must still restore the checks.
    let recording = Recording::default();
    recording.fail_on("boom");
    let path = script("fk-rollback", "SELECT 'boom';\n");
    let error = import(
        &recording,
        DriverKind::Postgres,
        true,
        &path,
        &[("FOREIGN_KEYS", "off")],
    )
    .await
    .expect_err("stop stops");

    assert!(
        error.message().contains("rolled back"),
        "{}",
        error.message()
    );
    assert_eq!(
        recording.statements(),
        vec![
            "SET session_replication_role = replica".to_owned(),
            "BEGIN".to_owned(),
            "SELECT 'boom'".to_owned(),
            "ROLLBACK".to_owned(),
            "SET session_replication_role = DEFAULT".to_owned(),
        ],
        "the epilogue runs on the rollback exit as well"
    );
}

#[tokio::test]
async fn foreign_keys_off_is_refused_on_a_driver_with_no_switch() {
    let recording = Recording::default();
    let path = script("fk-trino", "SELECT 1;\n");
    let error = import(
        &recording,
        DriverKind::Trino,
        true,
        &path,
        &[("FOREIGN_KEYS", "off")],
    )
    .await
    .expect_err("Trino has no switch and must not pretend");

    assert!(
        error.message().contains("not supported by trino"),
        "{}",
        error.message()
    );
    assert!(
        recording.statements().is_empty(),
        "nothing reached a session: {:?}",
        recording.statements()
    );
}

#[tokio::test]
async fn a_driver_without_transactions_reports_written_not_pending() {
    // Trino answers `transactions: false`. `stop` there cannot roll back, and the
    // message says what already landed rather than claiming a rollback.
    let recording = Recording::default();
    recording.fail_on("boom");
    let path = script("no-tx", "SELECT 1;\nSELECT 'boom';\n");
    let error = import(&recording, DriverKind::Trino, false, &path, &[])
        .await
        .expect_err("stop stops");

    assert!(
        error.message().contains("already applied remain"),
        "{}",
        error.message()
    );
    assert!(
        !recording.statements().iter().any(|sql| sql == "BEGIN"),
        "a driver without transactions is never sent one: {:?}",
        recording.statements()
    );
}

#[tokio::test]
async fn a_read_only_connection_refuses_the_script_before_connecting() {
    let recording = Recording::default();
    let path = script("read-only", "DROP TABLE people;\n");
    let error = import(
        &recording,
        DriverKind::Postgres,
        true,
        &path,
        &[("SAFE_MODE", "read_only")],
    )
    .await
    .expect_err("a read-only connection refuses DDL");

    assert!(
        error.message().contains("SAFE_MODE=read_only"),
        "{}",
        error.message()
    );
    assert!(
        recording.statements().is_empty(),
        "nothing reached a session: {:?}",
        recording.statements()
    );
}

#[tokio::test]
async fn the_error_cap_holds_for_statements_too() {
    // A file that is wrong everywhere must not turn its error list into the
    // memory problem. One past the cap proves the truncation flag, not just the
    // length.
    let recording = Recording::default();
    recording.fail_on("boom");
    let mut text = String::new();
    for index in 0..1_005 {
        text.push_str(&format!("SELECT 'boom-{index}';\n"));
    }
    let path = script("cap", &text);
    let events = import(
        &recording,
        DriverKind::Postgres,
        true,
        &path,
        &[("ON_ERROR", "skip")],
    )
    .await
    .expect("skip keeps going");

    let done = done(&events);
    let errors = done["errors"].as_array().expect("errors");
    assert_eq!(errors.len(), 1_000, "the cap is the study's 1.000");
    assert_eq!(done["errors_truncated"], true);
}

#[tokio::test]
async fn a_script_with_no_statements_is_refused_by_name() {
    let recording = Recording::default();
    let path = script("empty", "-- just a comment\n");
    let error = import(&recording, DriverKind::Postgres, true, &path, &[])
        .await
        .expect_err("nothing to import is a usage error");
    assert!(
        error.message().contains("has no SQL statements"),
        "{}",
        error.message()
    );
    assert!(recording.statements().is_empty());
}
