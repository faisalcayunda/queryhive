//! What `import_data` does with the values in a file before they reach the `INSERT`
//! (W12-T7a, IMPORT-2 part 1: DBX-31, DBX-32, DBX-7, DBX-72, PF-3, PF-4).
//!
//! The rule under test is one sentence: an import never writes a wrong value without saying
//! so. A day-first date is not read month-first by the server, `NaN` is not a bare word in the
//! SQL, a row with a stray comma does not shift its neighbours into the wrong columns, a
//! connection that dropped is not treated as a bad row, and a file that is not UTF-8 is refused
//! before the first write. The session is a recorder, so none of it needs a database; the
//! same claims against a real server are in `import_live.rs`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use qh_core::{ColumnBatch, ColumnMeta, EngineError, FailureKind};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions,
    ObjectPath, ObjectsPage, Session,
};
use qh_ffi::events::Capture;
use qh_ffi::{run, CancelFlag, CliError, Command, Engine, Settings};
use serde_json::Value as Json;

// ---------------------------------------------------------------------------------------
// The recorder
// ---------------------------------------------------------------------------------------

/// What the fake server saw, and which statements it refuses.
#[derive(Clone, Default)]
struct Script {
    seen: Arc<Mutex<Vec<String>>>,
    /// A statement containing the needle fails with the error.
    failures: Arc<Mutex<Vec<(String, EngineError)>>>,
    connects: Arc<AtomicUsize>,
}

impl Script {
    fn seen(&self) -> Vec<String> {
        self.seen.lock().expect("seen").clone()
    }

    /// The `INSERT`s only: the file's rows as the engine wrote them.
    fn inserts(&self) -> Vec<String> {
        self.seen()
            .into_iter()
            .filter(|sql| sql.starts_with("INSERT"))
            .collect()
    }

    fn fail_on(&self, needle: &str, error: EngineError) {
        self.failures
            .lock()
            .expect("failures")
            .push((needle.to_owned(), error));
    }

    fn connects(&self) -> usize {
        self.connects.load(Ordering::SeqCst)
    }
}

/// A server error as a driver reports it: the message, and the server's own code.
fn server_error(code: Option<&str>) -> EngineError {
    EngineError::Query {
        message: "the server refused the statement".to_owned(),
        code: code.map(str::to_owned),
        position: None,
        kind: FailureKind::Permanent,
    }
}

#[derive(Clone)]
struct FakeDriver {
    kind: DriverKind,
    transactions: bool,
    /// What `SELECT * FROM <table> LIMIT 0` describes. Empty is Trino before a row.
    columns: Vec<ColumnMeta>,
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
        unreachable!("the engine under test connects through `Engine::connect`")
    }
}

struct FakeEngine {
    script: Script,
    driver: FakeDriver,
}

#[async_trait]
impl Engine for FakeEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        vec![self.driver.kind]
    }

    fn driver(&self, _kind: DriverKind) -> &dyn Driver {
        &self.driver
    }

    async fn connect(&self, _config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        self.script.connects.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeSession {
            script: self.script.clone(),
            driver: self.driver.clone(),
        }))
    }
}

struct FakeSession {
    script: Script,
    driver: FakeDriver,
}

#[async_trait]
impl Session for FakeSession {
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
        self.script.seen.lock().expect("seen").push(sql.to_owned());
        let failure = self
            .script
            .failures
            .lock()
            .expect("failures")
            .iter()
            .find(|(needle, _)| sql.contains(needle.as_str()))
            .map(|(_, error)| error.clone());
        if let Some(error) = failure {
            return Err(error);
        }
        if sql.ends_with("LIMIT 0") {
            return Ok(Box::new(FakeCursor {
                columns: self.driver.columns.clone(),
                affected: None,
            }));
        }
        // An INSERT reports the rows it was given, which is what a server does.
        let affected = sql
            .starts_with("INSERT")
            .then(|| sql.matches("), (").count() as u64 + 1);
        Ok(Box::new(FakeCursor {
            columns: Vec::new(),
            affected,
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

struct FakeCursor {
    columns: Vec<ColumnMeta>,
    affected: Option<u64>,
}

#[async_trait]
impl Cursor for FakeCursor {
    fn columns(&self) -> &[ColumnMeta] {
        &self.columns
    }

    async fn next_batch(&mut self, _max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        Ok(None)
    }

    fn affected_rows(&self) -> Option<u64> {
        self.affected
    }
}

// ---------------------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------------------

fn columns(types: &[(&str, &str)]) -> Vec<ColumnMeta> {
    types
        .iter()
        .map(|(name, type_name)| ColumnMeta::new(*name, *type_name))
        .collect()
}

fn write(directory: &tempfile::TempDir, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
    let path = directory.path().join(name);
    std::fs::write(&path, bytes).expect("write the file");
    path
}

/// A server of `kind` that describes the target as `columns`, running `import_data` over `path`.
async fn import_with(
    script: &Script,
    kind: DriverKind,
    transactions: bool,
    columns: Vec<ColumnMeta>,
    path: &Path,
    extra: &[(&str, &str)],
) -> Result<Capture, CliError> {
    let db_kind = match kind {
        DriverKind::Trino => "trino",
        DriverKind::Postgres => "postgres",
        DriverKind::Mysql => "mysql",
    };
    let mut pairs: Vec<(&str, &str)> = vec![
        ("DB_KIND", db_kind),
        ("DB_HOST", "db.invalid"),
        ("DB_USER", "queryhive"),
        ("RETRIES", "0"),
        ("TARGET_SCHEMA", "public"),
        ("TARGET_TABLE", "t"),
        ("PROGRESS_MS", "0"),
    ];
    pairs.push(("IMPORT_PATH", path.to_str().expect("utf-8 path")));
    pairs.extend_from_slice(extra);
    let mut out = Capture::new();
    run(
        Command::ImportData,
        &Settings::from_pairs(pairs.iter().map(|(key, value)| (*key, *value))),
        &mut out,
        &FakeEngine {
            script: script.clone(),
            driver: FakeDriver {
                kind,
                transactions,
                columns,
            },
        },
        &CancelFlag::new(),
    )
    .await?;
    Ok(out)
}

/// PostgreSQL with a transaction, the usual target.
async fn import_pg(
    script: &Script,
    types: &[(&str, &str)],
    path: &Path,
    extra: &[(&str, &str)],
) -> Result<Capture, CliError> {
    import_with(
        script,
        DriverKind::Postgres,
        true,
        columns(types),
        path,
        extra,
    )
    .await
}

fn done(events: &Capture) -> Json {
    events
        .lines
        .iter()
        .find(|event| event["event"] == "done")
        .cloned()
        .unwrap_or_else(|| panic!("no done event: {:?}", events.lines()))
}

/// The error an import ended with, and the per-row messages that came with it.
fn failure(result: Result<Capture, CliError>) -> (String, Vec<String>) {
    match result {
        Err(CliError::Warned { message, warnings }) => (message, warnings),
        Err(other) => panic!("expected a warned failure, got: {}", other.message()),
        Ok(events) => panic!("the import succeeded: {:?}", events.lines()),
    }
}

/// The message of an import that was refused before it started (not one that ran and failed).
fn refused(result: Result<Capture, CliError>) -> String {
    match result {
        Err(CliError::Warned { .. }) => {
            panic!("the import ran and failed; it was meant to be refused")
        }
        Err(other) => other.message(),
        Ok(events) => panic!("the import succeeded: {:?}", events.lines()),
    }
}

fn transaction_marks(script: &Script) -> Vec<String> {
    script
        .seen()
        .into_iter()
        .filter(|sql| matches!(sql.as_str(), "BEGIN" | "COMMIT" | "ROLLBACK"))
        .collect()
}

// ---------------------------------------------------------------------------------------
// DBX-31: dates, NaN, grouping, `Z`
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn nan_and_infinity_are_quoted_so_the_server_reads_them_as_values_not_syntax() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(
        &directory,
        "f.csv",
        "id,x\n1,NaN\n2,-Infinity\n3,1.5\n4,1e3\n",
    );
    let script = Script::default();
    import_pg(&script, &[("id", "int8"), ("x", "float8")], &path, &[])
        .await
        .expect("imports");
    assert_eq!(
        script.inserts(),
        vec!["INSERT INTO \"public\".\"t\" (\"id\", \"x\") VALUES \
             (1, 'NaN'), (2, '-Infinity'), (3, 1.5), (4, 1e3)"
            .to_owned()]
    );
}

#[tokio::test]
async fn a_day_first_timestamp_reaches_postgres_as_the_day_it_was_written() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(&directory, "d.csv", "id,at\n1,03/04/2024 17:30\n");
    let types = [("id", "int8"), ("at", "timestamp")];

    // Without a format the text goes through untouched, which is what a server's DateStyle
    // would read as 4 March: the reason the format exists.
    let script = Script::default();
    import_pg(&script, &types, &path, &[])
        .await
        .expect("imports");
    assert!(
        script.inserts()[0].contains("'03/04/2024 17:30'"),
        "{:?}",
        script.inserts()
    );

    let script = Script::default();
    import_pg(
        &script,
        &types,
        &path,
        &[("DATE_FORMAT", "dd/MM/yyyy HH:mm")],
    )
    .await
    .expect("imports");
    assert!(
        script.inserts()[0].contains("'2024-04-03 17:30:00'"),
        "{:?}",
        script.inserts()
    );

    // A format the value does not fit is a rejected row that names the value and the format,
    // not a guess.
    let script = Script::default();
    let (message, errors) =
        failure(import_pg(&script, &types, &path, &[("DATE_FORMAT", "dd/MM/yyyy")]).await);
    assert!(message.contains("import stopped at line 2"), "{message}");
    assert!(
        errors[0].contains("column at") && errors[0].contains("DATE_FORMAT dd/MM/yyyy"),
        "{errors:?}"
    );

    // A pattern that is not a pattern is refused before connecting.
    let script = Script::default();
    let message =
        refused(import_pg(&script, &types, &path, &[("DATE_FORMAT", "dd/mm/yyyy")]).await);
    assert!(message.contains("DATE_FORMAT"), "{message}");
    assert_eq!(script.connects(), 0);

    // A pattern with a zone in it would match `2024-03-04T10:00:00Z`, drop the `Z`, and let the
    // server read the wall time in its own session zone: refused before connecting.
    for zoned in ["yyyy-MM-dd'T'HH:mm:ss'Z'", "yyyy-MM-dd HH:mm:ss+07:00"] {
        let script = Script::default();
        let message = refused(import_pg(&script, &types, &path, &[("DATE_FORMAT", zoned)]).await);
        assert!(message.contains("time zone"), "{zoned}: {message}");
        assert_eq!(script.connects(), 0);
    }
}

#[tokio::test]
async fn a_date_format_is_applied_per_column_type_and_leaves_text_alone() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(
        &directory,
        "d.csv",
        "id,born,note\n1,03/04/2024,03/04/2024\n",
    );
    let types = [("id", "int8"), ("born", "date"), ("note", "text")];
    let script = Script::default();
    import_pg(&script, &types, &path, &[("DATE_FORMAT", "dd/MM/yyyy")])
        .await
        .expect("imports");
    assert_eq!(
        script.inserts(),
        vec!["INSERT INTO \"public\".\"t\" (\"id\", \"born\", \"note\") VALUES (1, '2024-04-03', '03/04/2024')".to_owned()]
    );
}

#[tokio::test]
async fn a_date_that_does_not_exist_is_a_rejected_row_under_every_policy() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(
        &directory,
        "d.csv",
        "id,born\n1,01/02/2024\n2,31/02/2024\n3,05/06/2024\n",
    );
    let types = [("id", "int8"), ("born", "date")];
    let extra = |mode| [("DATE_FORMAT", "dd/MM/yyyy"), ("ON_ERROR", mode)];

    let script = Script::default();
    let events = import_pg(&script, &types, &path, &extra("skip"))
        .await
        .expect("skip imports");
    let done = done(&events);
    assert_eq!(
        (done["rows"].as_u64(), done["rejected"].as_u64()),
        (Some(2), Some(1))
    );
    assert!(
        done["errors"][0]
            .as_str()
            .unwrap()
            .starts_with("line 3: column born: '31/02/2024'"),
        "{done}"
    );
    assert_eq!(
        script.inserts().len(),
        2,
        "each row is its own statement under skip"
    );

    let script = Script::default();
    let (message, errors) = failure(import_pg(&script, &types, &path, &extra("stop")).await);
    assert!(message.contains("import stopped at line 3"), "{message}");
    assert!(errors[0].contains("31/02/2024"), "{errors:?}");
    assert_eq!(transaction_marks(&script), ["BEGIN", "ROLLBACK"]);
}

#[tokio::test]
async fn a_mysql_z_is_sent_as_a_numeric_offset_and_postgres_is_left_alone() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(&directory, "z.csv", "id,at\n1,2024-03-04T10:00:00Z\n");
    let script = Script::default();
    import_with(
        &script,
        DriverKind::Mysql,
        true,
        columns(&[("id", "int"), ("at", "datetime")]),
        &path,
        &[],
    )
    .await
    .expect("imports");
    assert_eq!(
        script.inserts(),
        vec!["INSERT INTO `t` (`id`, `at`) VALUES (1, '2024-03-04T10:00:00+00:00')".to_owned()]
    );

    let script = Script::default();
    import_pg(
        &script,
        &[("id", "int8"), ("at", "timestamptz")],
        &path,
        &[],
    )
    .await
    .expect("imports");
    assert!(
        script.inserts()[0].contains("'2024-03-04T10:00:00Z'"),
        "{:?}",
        script.inserts()
    );
}

#[tokio::test]
async fn grouping_separators_are_opt_in_and_paired_with_the_decimal_one() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(&directory, "n.csv", "id,price\n1,\"1.500,25\"\n2,\"12\"\n");
    let types = [("id", "int8"), ("price", "numeric")];

    let script = Script::default();
    import_pg(&script, &types, &path, &[])
        .await
        .expect("imports");
    assert!(
        script.inserts()[0].contains("'1.500,25'"),
        "never guessed: {:?}",
        script.inserts()
    );

    let script = Script::default();
    import_pg(
        &script,
        &types,
        &path,
        &[("DECIMAL_SEPARATOR", ","), ("GROUPING_SEPARATOR", ".")],
    )
    .await
    .expect("imports");
    assert!(
        script.inserts()[0].ends_with("VALUES (1, 1500.25), (2, 12)"),
        "{:?}",
        script.inserts()
    );

    let script = Script::default();
    let message = refused(import_pg(&script, &types, &path, &[("GROUPING_SEPARATOR", ".")]).await);
    assert!(message.contains("DECIMAL_SEPARATOR"), "{message}");
    assert_eq!(script.connects(), 0, "refused before connecting");
}

#[tokio::test]
async fn a_fraction_in_an_integer_column_is_rejected_instead_of_rounded_by_the_server() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(&directory, "i.csv", "id,qty\n1,5.5\n2,6\n3,7.0\n");
    let script = Script::default();
    let events = import_pg(
        &script,
        &[("id", "int8"), ("qty", "int4")],
        &path,
        &[("ON_ERROR", "skip")],
    )
    .await
    .expect("imports");
    let done = done(&events);
    assert_eq!(
        (done["rows"].as_u64(), done["rejected"].as_u64()),
        (Some(2), Some(1))
    );
    assert!(
        done["errors"][0]
            .as_str()
            .unwrap()
            .contains("5.5 is not a whole number"),
        "{done}"
    );
    assert_eq!(script.inserts().len(), 2);
    assert!(script.inserts().iter().all(|sql| !sql.contains("5.5")));
}

#[tokio::test]
async fn a_format_that_needs_column_types_is_refused_where_the_driver_reports_none() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(&directory, "t.csv", "id,born\n1,03/04/2024\n");
    let script = Script::default();
    let message = refused(
        import_with(
            &script,
            DriverKind::Trino,
            false,
            Vec::new(),
            &path,
            &[("DATE_FORMAT", "dd/MM/yyyy")],
        )
        .await,
    );
    assert!(
        message.contains("need the target's column types"),
        "{message}"
    );
    assert!(script.inserts().is_empty());
}

// ---------------------------------------------------------------------------------------
// PF-4: ragged rows
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_stray_comma_in_row_three_stops_the_import_and_names_the_widths() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(&directory, "r.csv", "id,name\n1,Ayu\n2,Budi, Jr\n3,Citra\n");
    let types = [("id", "int8"), ("name", "text")];

    let script = Script::default();
    let (message, errors) = failure(import_pg(&script, &types, &path, &[]).await);
    assert!(message.contains("import stopped at line 3"), "{message}");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with("line 3: row has 3 fields, header has 2"),
        "{errors:?}"
    );
    assert_eq!(
        transaction_marks(&script),
        ["BEGIN", "ROLLBACK"],
        "stop rolls back"
    );
    assert!(
        script.inserts().iter().all(|sql| !sql.contains("Jr")),
        "the shifted row is never written: {:?}",
        script.inserts()
    );

    let script = Script::default();
    let events = import_pg(&script, &types, &path, &[("ON_ERROR", "skip")])
        .await
        .expect("skip goes on");
    let done = done(&events);
    assert_eq!(
        (done["rows"].as_u64(), done["rejected"].as_u64()),
        (Some(2), Some(1))
    );
    assert!(
        done["errors"][0]
            .as_str()
            .unwrap()
            .starts_with("line 3: row has 3 fields"),
        "{done}"
    );
}

#[tokio::test]
async fn a_trailing_delimiter_is_allowed_and_a_short_row_needs_the_opt_in() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(&directory, "r.csv", "id,name\n1,Ayu,\n2\n3,Citra\n");
    let types = [("id", "int8"), ("name", "text")];

    let script = Script::default();
    let (message, errors) = failure(import_pg(&script, &types, &path, &[]).await);
    assert!(message.contains("import stopped at line 3"), "{message}");
    assert!(
        errors[0].starts_with("line 3: row has 1 fields, header has 2"),
        "{errors:?}"
    );
    assert_eq!(
        script.inserts().len(),
        1,
        "the row before it was sent, then rolled back"
    );

    let script = Script::default();
    import_pg(&script, &types, &path, &[("ALLOW_SHORT_ROWS", "1")])
        .await
        .expect("the opt-in pads it");
    assert!(
        script.inserts()[0].ends_with("VALUES (1, 'Ayu'), (2, NULL), (3, 'Citra')"),
        "{:?}",
        script.inserts()
    );
}

// ---------------------------------------------------------------------------------------
// PF-3: a dead connection is not a bad row
// ---------------------------------------------------------------------------------------

fn dropped() -> EngineError {
    EngineError::Connect {
        message: "the connection failed while the query ran: Connection reset by peer".to_owned(),
        kind: FailureKind::Transient,
    }
}

fn reset_mid_query() -> EngineError {
    EngineError::Query {
        message: "connection reset".to_owned(),
        code: None,
        position: None,
        kind: FailureKind::Transient,
    }
}

#[tokio::test]
async fn skip_mode_stops_when_the_connection_drops_instead_of_skipping_every_remaining_row() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(&directory, "c.csv", "id,name\n1,a\n2,boom\n3,c\n4,d\n");
    let types = [("id", "int8"), ("name", "text")];
    for error in [dropped(), reset_mid_query()] {
        let script = Script::default();
        script.fail_on("'boom'", error);
        let (message, errors) =
            failure(import_pg(&script, &types, &path, &[("ON_ERROR", "skip")]).await);
        assert!(message.contains("import stopped at line 3"), "{message}");
        assert!(
            message.contains("rows after line 3 were not attempted"),
            "{message}"
        );
        assert!(
            message.contains("the 1 rows already written to public.t remain"),
            "{message}"
        );
        assert_eq!(errors.len(), 1, "{errors:?}");
        // Row 4 and 5 were never sent, and nothing was sent a second time.
        assert_eq!(script.inserts().len(), 2, "{:?}", script.inserts());
        assert!(script
            .inserts()
            .iter()
            .all(|sql| !sql.contains("'c'") && !sql.contains("'d'")));
    }
}

#[tokio::test]
async fn a_server_error_is_still_only_a_bad_row_under_skip() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(&directory, "c.csv", "id,name\n1,a\n2,boom\n3,c\n");
    let script = Script::default();
    script.fail_on("'boom'", server_error(Some("23505")));
    let events = import_pg(
        &script,
        &[("id", "int8"), ("name", "text")],
        &path,
        &[("ON_ERROR", "skip")],
    )
    .await
    .expect("skip goes on");
    assert_eq!(done(&events)["rows"], 2);
}

#[tokio::test]
async fn in_a_transaction_a_dropped_connection_says_nothing_was_written_and_does_not_try_to_roll_back(
) {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(&directory, "c.csv", "id,name\n1,a\n2,boom\n");
    for mode in ["stop", "commit"] {
        let script = Script::default();
        script.fail_on("'boom'", dropped());
        let (message, _) = failure(
            import_pg(
                &script,
                &[("id", "int8"), ("name", "text")],
                &path,
                &[("ON_ERROR", mode)],
            )
            .await,
        );
        assert!(
            message.contains("no rows were written to public.t"),
            "{mode}: {message}"
        );
        assert!(
            message.contains("lines 2-3; rows after line 3 were not attempted"),
            "{mode}: {message}"
        );
        assert_eq!(
            transaction_marks(&script),
            ["BEGIN"],
            "{mode}: a dead connection gets no ROLLBACK"
        );
    }
}

#[tokio::test]
async fn a_statement_file_in_skip_mode_stops_on_a_dropped_connection_too() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(
        &directory,
        "s.sql",
        "INSERT INTO t VALUES (1);\nINSERT INTO t VALUES (2);\nINSERT INTO t VALUES (3);\n",
    );
    let script = Script::default();
    script.fail_on("VALUES (2)", dropped());
    let (message, _) = failure(import_pg(&script, &[], &path, &[("ON_ERROR", "skip")]).await);
    assert!(
        message.contains("statements after line 2 were not attempted"),
        "{message}"
    );
    assert_eq!(
        script.inserts().len(),
        2,
        "the third statement was never sent"
    );
}

// ---------------------------------------------------------------------------------------
// DBX-7: the code page, checked before the first write
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_file_that_is_not_utf8_is_refused_before_connecting_and_cp1252_is_opt_in() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let mut bytes = b"id,name\n1,Ayu\n2,Caf".to_vec();
    bytes.extend([0xE9, b'\n']);
    let path = write(&directory, "cp.csv", bytes);
    let types = [("id", "int8"), ("name", "text")];

    let script = Script::default();
    let message = refused(import_pg(&script, &types, &path, &[("ON_ERROR", "skip")]).await);
    assert!(message.contains("line 3"), "{message}");
    assert!(message.contains("ENCODING=cp1252"), "{message}");
    assert_eq!(
        script.connects(),
        0,
        "refused before the connection, so before any write"
    );

    let script = Script::default();
    import_pg(&script, &types, &path, &[("ENCODING", "cp1252")])
        .await
        .expect("imports");
    assert!(
        script.inserts()[0].ends_with("(2, 'Caf\u{e9}')"),
        "{:?}",
        script.inserts()
    );

    let script = Script::default();
    let message = refused(import_pg(&script, &types, &path, &[("ENCODING", "klingon")]).await);
    assert!(message.contains("unknown ENCODING"), "{message}");
}

#[tokio::test]
async fn a_code_page_is_refused_for_a_format_that_has_none() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(&directory, "j.jsonl", "{\"id\": 1}\n");
    let script = Script::default();
    let message =
        refused(import_pg(&script, &[("id", "int8")], &path, &[("ENCODING", "cp1252")]).await);
    assert!(message.contains("applies to CSV"), "{message}");
}

// ---------------------------------------------------------------------------------------
// DBX-32: XLSX
// ---------------------------------------------------------------------------------------

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../qh-import/tests/fixtures/offset_sheet.xlsx")
}

#[tokio::test]
async fn a_workbook_streams_into_the_insert_with_every_digit_a_number_cell_stores() {
    let script = Script::default();
    let events = import_pg(
        &script,
        &[("id", "int8"), ("name", "text"), ("amount", "numeric")],
        &fixture(),
        &[("ON_ERROR", "skip")],
    )
    .await
    .expect("imports");
    let done = done(&events);
    assert_eq!(done["format"], "xlsx");
    assert_eq!(done["streams"], true);
    assert_eq!(
        (done["rows"].as_u64(), done["rejected"].as_u64()),
        (Some(2), Some(1))
    );
    assert!(
        done["errors"][0].as_str().unwrap().starts_with("line 4:"),
        "{done}"
    );
    assert_eq!(
        script.inserts(),
        vec![
            "INSERT INTO \"public\".\"t\" (\"id\", \"name\", \"amount\") VALUES (1, 'Ayu', 0.30000000000000004)"
                .to_owned(),
            "INSERT INTO \"public\".\"t\" (\"id\", \"name\", \"amount\") VALUES (2, 'Budi', NULL)"
                .to_owned(),
        ]
    );
    let progress: Vec<&Json> = events
        .lines
        .iter()
        .filter(|e| e["event"] == "progress")
        .collect();
    assert!(
        progress.iter().all(|e| e.get("bytes").is_none()),
        "a sheet has no byte position"
    );
    assert_eq!(progress.last().unwrap()["rows_total"], 3);
}

fn locale_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../qh-import/tests/fixtures/locale_sheet.xlsx")
}

/// `locale_sheet.xlsx` row 2: `a` is the number cell 1.234, `b` the text cell "1.234,5", and
/// `c` the number cell 3.141592653589793.
#[tokio::test]
async fn a_number_cell_is_exact_in_a_number_column_and_shown_in_a_text_column() {
    let script = Script::default();
    import_pg(
        &script,
        &[("a", "numeric"), ("b", "text"), ("c", "float8")],
        &locale_fixture(),
        &[],
    )
    .await
    .expect("imports");
    assert_eq!(
        script.inserts(),
        vec![
            "INSERT INTO \"public\".\"t\" (\"a\", \"b\", \"c\") VALUES (1.234, '1.234,5', 3.141592653589793)"
                .to_owned()
        ]
    );

    // The same float into a text column is the 15 digits Excel shows.
    let script = Script::default();
    import_pg(
        &script,
        &[("a", "numeric"), ("b", "text"), ("c", "text")],
        &locale_fixture(),
        &[],
    )
    .await
    .expect("imports");
    assert_eq!(
        script.inserts(),
        vec![
            "INSERT INTO \"public\".\"t\" (\"a\", \"b\", \"c\") VALUES (1.234, '1.234,5', '3.14159265358979')"
                .to_owned()
        ]
    );
}

/// With the Indonesian number format, the number cell 1.234 stays 1.234 (it is not a text
/// spelling of 1234) and only the text cell "1.234,5" is read as 1234.5.
#[tokio::test]
async fn a_locale_reads_text_cells_and_leaves_number_cells_alone() {
    let script = Script::default();
    import_pg(
        &script,
        &[("a", "numeric"), ("b", "numeric"), ("c", "numeric")],
        &locale_fixture(),
        &[("DECIMAL_SEPARATOR", ","), ("GROUPING_SEPARATOR", ".")],
    )
    .await
    .expect("imports");
    assert_eq!(
        script.inserts(),
        vec![
            "INSERT INTO \"public\".\"t\" (\"a\", \"b\", \"c\") VALUES (1.234, 1234.5, 3.141592653589793)"
                .to_owned()
        ]
    );
}

/// A JSON number is `.`-decimal already, and the reader does not tell it from a string, so a
/// locale is refused rather than multiplying 1.234 by a thousand.
#[tokio::test]
async fn a_number_format_is_refused_for_json_before_connecting() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(&directory, "n.jsonl", "{\"n\": 1.234}\n");
    for settings in [
        vec![("DECIMAL_SEPARATOR", ","), ("GROUPING_SEPARATOR", ".")],
        vec![("DECIMAL_SEPARATOR", ",")],
    ] {
        let script = Script::default();
        let message = refused(import_pg(&script, &[("n", "numeric")], &path, &settings).await);
        assert!(message.contains("JSON"), "{message}");
        assert_eq!(script.connects(), 0);
        assert!(script.inserts().is_empty());
    }
}

#[tokio::test]
async fn a_workbook_over_a_limit_is_a_usage_error_before_connecting() {
    let script = Script::default();
    let message = refused(
        import_pg(
            &script,
            &[("id", "int8")],
            &fixture(),
            &[("IMPORT_XLSX_MAX_BYTES", "100")],
        )
        .await,
    );
    assert!(message.contains("IMPORT_XLSX_MAX_BYTES"), "{message}");
    assert_eq!(script.connects(), 0);

    let script = Script::default();
    let message = refused(
        import_pg(
            &script,
            &[("id", "int8")],
            &fixture(),
            &[("IMPORT_XLSX_MAX_CELLS", "5")],
        )
        .await,
    );
    assert!(message.contains("IMPORT_XLSX_MAX_CELLS"), "{message}");
    assert_eq!(script.connects(), 0);
}

// ---------------------------------------------------------------------------------------
// DBX-72: progress by bytes
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn progress_carries_the_bytes_read_and_the_file_size() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let mut text = String::from("id,name\n");
    for row in 0..50 {
        text.push_str(&format!("{row},name{row}\n"));
    }
    let path = write(&directory, "p.csv", &text);
    let script = Script::default();
    let events = import_pg(
        &script,
        &[("id", "int8"), ("name", "text")],
        &path,
        &[("ON_ERROR", "skip")],
    )
    .await
    .expect("imports");
    let progress: Vec<&Json> = events
        .lines
        .iter()
        .filter(|e| e["event"] == "progress")
        .collect();
    assert!(
        progress.len() > 1,
        "skip sends a row at a time and PROGRESS_MS=0 reports each"
    );
    let total = text.len() as u64;
    assert!(
        progress.iter().all(|e| e["bytes_total"] == total),
        "{progress:?}"
    );
    let bytes: Vec<u64> = progress
        .iter()
        .map(|e| e["bytes"].as_u64().unwrap())
        .collect();
    assert!(bytes.windows(2).all(|pair| pair[0] <= pair[1]), "{bytes:?}");
    assert_eq!(
        *bytes.last().unwrap(),
        total,
        "the last event says the whole file was read"
    );
}
