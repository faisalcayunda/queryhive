//! `import_data` on a `.sql` file: what the splitter hands the importer, and what is refused
//! before a connection opens (W12-T7b, ADR-0022 addendum).
//!
//! * a MySQL stored-program body reaches the server as one statement, so a failure under
//!   `ON_ERROR=skip` cannot leave its second `DELETE` to run alone;
//! * a `DELIMITER` line, a psql backslash line (pg_dump's `\restrict`) and `COPY … FROM STDIN`
//!   are refused by name, with their line, and nothing connects;
//! * a UTF-8 or UTF-16 byte-order mark is read past, and a file that is not text says where.
//!
//! The session is the recorder `import_sql.rs` uses: no database is needed.

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
    /// How many times the importer asked for a session.
    connects: Arc<Mutex<u32>>,
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
        *self.recording.connects.lock().expect("connects") += 1;
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
    script_bytes(name, text.as_bytes())
}

fn script_bytes(name: &str, bytes: &[u8]) -> PathBuf {
    let path =
        std::env::temp_dir().join(format!("qh-import-split-{}-{name}.sql", std::process::id()));
    std::fs::write(&path, bytes).expect("write the script");
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

const DUMP_ROUTINE: &str = "CREATE PROCEDURE purge() BEGIN DELETE FROM a; DELETE FROM b; END;";

#[tokio::test]
async fn a_routine_body_is_one_statement_in_a_transaction() {
    let recording = Recording::default();
    let path = script(
        "routine",
        &format!("SELECT 1;\n{DUMP_ROUTINE}\nSELECT 2;\n"),
    );
    let events = import(&recording, DriverKind::Mysql, true, &path, &[])
        .await
        .expect("the script imports");
    assert_eq!(
        recording.statements(),
        vec![
            "BEGIN".to_owned(),
            "SELECT 1".to_owned(),
            DUMP_ROUTINE.trim_end_matches(';').to_owned(),
            "SELECT 2".to_owned(),
            "COMMIT".to_owned(),
        ]
    );
    assert_eq!(done(&events)["statements"], 3);
}

#[tokio::test]
async fn a_failed_routine_never_leaves_its_tail_to_run_alone_under_skip() {
    // The data-loss case: with every `;` a separator, the fragment `DELETE FROM b` is a statement
    // of its own that succeeds after `CREATE PROCEDURE … DELETE FROM a` failed.
    let recording = Recording::default();
    recording.fail_on("CREATE PROCEDURE");
    let path = script("skip", &format!("{DUMP_ROUTINE}\nSELECT 2;\n"));
    let events = import(
        &recording,
        DriverKind::Mysql,
        true,
        &path,
        &[("ON_ERROR", "skip")],
    )
    .await
    .expect("skip keeps going");
    let sent = recording.statements();
    assert_eq!(
        sent,
        vec![
            DUMP_ROUTINE.trim_end_matches(';').to_owned(),
            "SELECT 2".to_owned()
        ],
        "no fragment of the routine ran by itself: {sent:?}"
    );
    let done = done(&events);
    assert_eq!(done["errors"].as_array().expect("errors").len(), 1);
    assert_eq!(done["statements"], 1);
}

async fn refused_before_connecting(
    name: &str,
    kind: DriverKind,
    text: &str,
    needles: &[&str],
) -> String {
    let recording = Recording::default();
    let path = script(name, text);
    let error = import(&recording, kind, true, &path, &[])
        .await
        .expect_err("a client directive is refused");
    let message = error.message();
    for needle in needles {
        assert!(message.contains(needle), "{needle:?} not in {message}");
    }
    assert_eq!(
        *recording.connects.lock().unwrap(),
        0,
        "nothing connected: {message}"
    );
    assert!(recording.statements().is_empty());
    message
}

#[tokio::test]
async fn a_delimiter_line_is_refused_by_name_before_connecting() {
    let dump = format!(
        "SET NAMES utf8mb4;\nDROP TABLE IF EXISTS t;\nDELIMITER ;;\n{DUMP_ROUTINE}\nDELIMITER ;\n"
    );
    refused_before_connecting(
        "delimiter",
        DriverKind::Mysql,
        &dump,
        &["line 3", "`DELIMITER`", "mysql client command"],
    )
    .await;
    // Even a file whose first statement would have passed every other check.
    refused_before_connecting(
        "delimiter-lower",
        DriverKind::Mysql,
        "delimiter //\nSELECT 1//\n",
        &["line 1", "`DELIMITER`"],
    )
    .await;
}

#[tokio::test]
async fn a_pg_dump_restrict_line_is_refused_not_glued_to_the_first_statement() {
    let dump = "--\n-- PostgreSQL database dump\n--\n\\restrict Zk3xQ\n\nSET statement_timeout = 0;\nSELECT 1;\n\\unrestrict Zk3xQ\n";
    let message = refused_before_connecting(
        "restrict",
        DriverKind::Postgres,
        dump,
        &["line 4", "`\\restrict`", "psql meta-command"],
    )
    .await;
    assert!(!message.contains("syntax"), "{message}");
}

#[tokio::test]
async fn copy_from_stdin_is_refused_by_name_before_connecting() {
    refused_before_connecting(
        "copy",
        DriverKind::Postgres,
        "CREATE TABLE t (a int);\nCOPY public.t (a) FROM stdin;\n1\n2\n\\.\n",
        &["line 2", "COPY … FROM STDIN", "\\copy"],
    )
    .await;
}

#[tokio::test]
async fn a_utf8_byte_order_mark_is_not_part_of_the_first_statement() {
    // With the mark left in, the first statement is not `SELECT` to the classifier, and a
    // read-only connection refuses a file that holds nothing but a read.
    let recording = Recording::default();
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(b"SELECT 1;\nSELECT 2;\n");
    let path = script_bytes("bom8", &bytes);
    import(
        &recording,
        DriverKind::Postgres,
        true,
        &path,
        &[("SAFE_MODE", "read_only")],
    )
    .await
    .expect("a read-only file imports on a read-only connection");
    assert_eq!(
        recording.statements(),
        vec![
            "BEGIN".to_owned(),
            "SELECT 1".to_owned(),
            "SELECT 2".to_owned(),
            "COMMIT".to_owned()
        ]
    );
}

#[tokio::test]
async fn a_utf16_file_is_decoded() {
    for big_endian in [false, true] {
        let recording = Recording::default();
        let text = "SELECT 'é;😀';\nSELECT 2;\n";
        let mut bytes = if big_endian {
            vec![0xFE, 0xFF]
        } else {
            vec![0xFF, 0xFE]
        };
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&if big_endian {
                unit.to_be_bytes()
            } else {
                unit.to_le_bytes()
            });
        }
        let path = script_bytes(if big_endian { "u16be" } else { "u16le" }, &bytes);
        import(&recording, DriverKind::Postgres, true, &path, &[])
            .await
            .expect("a UTF-16 script imports");
        assert_eq!(
            recording.statements()[1..3],
            ["SELECT 'é;😀'".to_owned(), "SELECT 2".to_owned()],
            "big_endian={big_endian}"
        );
    }
}

#[tokio::test]
async fn a_file_that_is_not_utf8_says_where_it_stops() {
    let recording = Recording::default();
    let path = script_bytes("latin1", b"SELECT 1;\nINSERT INTO t VALUES ('caf\xe9');\n");
    let error = import(&recording, DriverKind::Mysql, true, &path, &[])
        .await
        .expect_err("not UTF-8");
    let message = error.message();
    assert!(message.contains("not valid UTF-8"), "{message}");
    assert!(message.contains("line 2"), "{message}");
    assert!(message.contains("--hex-blob"), "{message}");
    assert_eq!(*recording.connects.lock().unwrap(), 0);
}
