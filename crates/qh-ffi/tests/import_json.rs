//! `import_data` on a JSON or JSONL file: the row family with a reader that can say NULL.
//!
//! The reader itself (framing, BOM, key order, the 64 MiB element cap) is proved in
//! `qh-import`. This file proves what `import_data` does with it:
//!
//! * a JSON `null`, a missing key and `""` stay three different things all the way to the
//!   `INSERT`, and a long number or `1.10` reaches the server as written;
//! * `COLUMNS` entries that name a key are checked against the file, and `IMPORT_PREVIEW`
//!   lists the keys without a connection;
//! * the three `ON_ERROR` policies behave over a JSON source, and a transaction the server
//!   already ended is not reported as written (backlog B-13 N2); and
//! * `IMPORT_SQL_MAX_BYTES` guards the process against a `.sql` file read whole.
//!
//! The session here is a recorder, so none of it needs a database. The last block is the
//! same three policies against a real PostgreSQL:
//!
//! ```bash
//! QH_TEST_POSTGRES=1 cargo test -p qh-ffi --test import_json
//! ```
//!
//! Without `QH_TEST_POSTGRES=1` each live test prints why it is skipping and returns, for
//! the reason `real_server.rs` gives: a green run that tested nothing is worse than a visible
//! skip.

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
use qh_ffi::{run, CancelFlag, CliError, Command, Engine, RealEngine, Settings};
use serde_json::{json, Value as Json};

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

/// The target table, as `SELECT * FROM <table> LIMIT 0` describes it.
fn people_columns() -> Vec<ColumnMeta> {
    vec![
        ColumnMeta::new("id", "int8"),
        ColumnMeta::new("name", "text"),
        ColumnMeta::new("amount", "numeric"),
        ColumnMeta::new("meta", "jsonb"),
        ColumnMeta::new("active", "boolean"),
    ]
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
                columns: people_columns(),
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

fn settings(pairs: &[(&str, &str)]) -> Settings {
    Settings::from_pairs(pairs.iter().map(|(key, value)| (*key, *value)))
}

fn write(directory: &tempfile::TempDir, name: &str, text: &str) -> PathBuf {
    let path = directory.path().join(name);
    std::fs::write(&path, text).expect("write the file");
    path
}

/// Run `import_data` over `path` against the recorder.
async fn import(
    script: &Script,
    kind: DriverKind,
    transactions: bool,
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
        ("TARGET_TABLE", "people"),
    ];
    pairs.push(("IMPORT_PATH", path.to_str().expect("utf-8 path")));
    pairs.extend_from_slice(extra);
    let mut out = Capture::new();
    run(
        Command::ImportData,
        &settings(&pairs),
        &mut out,
        &FakeEngine {
            script: script.clone(),
            driver: FakeDriver { kind, transactions },
        },
        &CancelFlag::new(),
    )
    .await?;
    Ok(out)
}

fn done(events: &Capture) -> Json {
    events
        .lines
        .iter()
        .find(|event| event["event"] == "done")
        .cloned()
        .unwrap_or_else(|| panic!("no done event: {:?}", events.lines()))
}

// ---------------------------------------------------------------------------------------
// What reaches the INSERT
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn null_missing_and_empty_stay_apart_and_numbers_arrive_as_written() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(
        &directory,
        "people.jsonl",
        concat!(
            r#"{"id": 1, "name": "Ada", "amount": 1.10, "meta": {"a": [1, 2]}, "active": true}"#,
            "\n",
            r#"{"id": 2, "name": "", "amount": 123456789012345678901234567890, "meta": null}"#,
            "\n",
            r#"{"id": 3, "name": null}"#,
            "\n",
        ),
    );
    let script = Script::default();
    let events = import(&script, DriverKind::Postgres, true, &path, &[])
        .await
        .expect("the file imports");

    assert_eq!(
        script.inserts(),
        vec![
            "INSERT INTO \"public\".\"people\" (\"id\", \"name\", \"amount\", \"meta\", \"active\") \
             VALUES (1, 'Ada', 1.10, '{\"a\": [1, 2]}', TRUE), \
             (2, '', 123456789012345678901234567890, NULL, NULL), \
             (3, NULL, NULL, NULL, NULL)"
                .to_owned()
        ],
        "null and a missing key are NULL, \"\" is the empty string, 1.10 keeps its zero, and the \
         30-digit number is not squeezed through a float"
    );
    let seen = script.seen();
    assert_eq!(
        seen.first().map(String::as_str),
        Some("SELECT * FROM \"public\".\"people\" LIMIT 0")
    );
    assert_eq!(seen[1], "BEGIN");
    assert_eq!(seen.last().map(String::as_str), Some("COMMIT"));

    let done = done(&events);
    assert_eq!(done["rows"], 3);
    assert_eq!(done["format"], "json");
    assert_eq!(done["streams"], true);
    assert_eq!(done["transaction"], true);
    assert_eq!(done["rejected"], 0);
    assert!(done.get("warnings").is_none(), "{done}");
}

#[tokio::test]
async fn an_array_a_bom_and_pretty_printing_read_the_same_rows() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let variants = [
        (
            "array.json",
            "[{\"id\": 1, \"name\": \"a\"},\n {\"id\": 2, \"name\": \"b\"}]",
        ),
        (
            "bom.jsonl",
            "\u{feff}{\"id\": 1, \"name\": \"a\"}\n{\"id\": 2, \"name\": \"b\"}\n",
        ),
        (
            "pretty.json",
            "{\n  \"id\": 1,\n  \"name\": \"a\"\n}\n{\n  \"id\": 2,\n  \"name\": \"b\"\n}\n",
        ),
    ];
    for (name, text) in variants {
        let path = write(&directory, name, text);
        let script = Script::default();
        import(&script, DriverKind::Postgres, true, &path, &[])
            .await
            .unwrap_or_else(|error| panic!("{name}: {}", error.message()));
        assert_eq!(
            script.inserts(),
            vec![
                "INSERT INTO \"public\".\"people\" (\"id\", \"name\") VALUES (1, 'a'), (2, 'b')"
                    .to_owned()
            ],
            "{name}"
        );
    }
}

#[tokio::test]
async fn null_text_is_off_for_json_until_the_caller_sets_it() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(&directory, "n.jsonl", "{\"id\": 1, \"name\": \"n/a\"}\n");

    let script = Script::default();
    import(&script, DriverKind::Postgres, true, &path, &[])
        .await
        .expect("imports");
    assert!(
        script.inserts()[0].ends_with("VALUES (1, 'n/a')"),
        "{:?}",
        script.inserts()
    );

    let script = Script::default();
    import(
        &script,
        DriverKind::Postgres,
        true,
        &path,
        &[("NULL_TEXT", "n/a")],
    )
    .await
    .expect("imports");
    assert!(
        script.inserts()[0].ends_with("VALUES (1, NULL)"),
        "{:?}",
        script.inserts()
    );
}

#[tokio::test]
async fn a_row_of_nothing_but_null_is_rejected_and_one_empty_string_is_not() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(
        &directory,
        "r.jsonl",
        "{\"id\": 1, \"name\": \"a\"}\n{\"id\": null, \"name\": null}\n",
    );
    let script = Script::default();
    let error = import(&script, DriverKind::Postgres, true, &path, &[])
        .await
        .expect_err("stop refuses the row that carries nothing");
    assert!(
        error.message().contains("stopped at line 2"),
        "{}",
        error.message()
    );
    assert!(error
        .warnings()
        .iter()
        .any(|w| w.contains("no value reached")));

    // `""` is a value in JSON, so a row that holds only that goes in.
    let path = write(&directory, "e.jsonl", "{\"id\": null, \"name\": \"\"}\n");
    let script = Script::default();
    import(&script, DriverKind::Postgres, true, &path, &[])
        .await
        .expect("an empty string is a value");
    assert!(
        script.inserts()[0].ends_with("VALUES (NULL, '')"),
        "{:?}",
        script.inserts()
    );
}

#[tokio::test]
async fn a_late_key_is_reported_and_not_imported() {
    // The columns are named from the opening rows. A key that first shows up after them
    // cannot be mapped, and the import says so rather than dropping it silently.
    let directory = tempfile::tempdir().expect("a temporary directory");
    let mut text = String::new();
    for id in 1..=1_001 {
        text.push_str(&format!("{{\"id\": {id}}}\n"));
    }
    text.push_str("{\"id\": 1002, \"extra\": \"x\"}\n");
    let path = write(&directory, "late.jsonl", &text);
    let script = Script::default();
    let events = import(&script, DriverKind::Postgres, true, &path, &[])
        .await
        .expect("the import runs");

    let done = done(&events);
    assert_eq!(done["rows"], 1_002);
    let warnings = done["warnings"].as_array().expect("warnings");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].as_str().expect("text").contains("'extra'"),
        "{warnings:?}"
    );
    assert!(
        script.inserts().iter().all(|sql| !sql.contains("extra")),
        "the late key is in no INSERT"
    );
}

#[tokio::test]
async fn an_element_that_is_not_an_object_is_refused_with_its_line_before_connecting() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(&directory, "bad.json", "[{\"id\": 1},\n 5]");
    let script = Script::default();
    let error = import(&script, DriverKind::Postgres, true, &path, &[])
        .await
        .expect_err("a number is not a row");
    assert!(
        error
            .message()
            .contains("element 2 (line 2) is not an object"),
        "{}",
        error.message()
    );
    assert_eq!(
        script.connects(),
        0,
        "the file is read before the connection opens"
    );

    // Nothing to map from: refused by name, not as a missing header.
    let path = write(&directory, "empty.json", "[]");
    let error = import(&script, DriverKind::Postgres, true, &path, &[])
        .await
        .expect_err("an empty array has no keys");
    assert!(
        error.message().contains("no JSON objects"),
        "{}",
        error.message()
    );
    assert_eq!(script.connects(), 0);
}

// ---------------------------------------------------------------------------------------
// COLUMNS and the preview
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn columns_map_json_keys_by_position_and_a_stale_name_is_refused() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(
        &directory,
        "map.jsonl",
        "{\"id\": 1, \"mail\": \"a@b\", \"skip\": 0}\n",
    );

    // `mail` goes to `name`; `skip` is left out.
    let script = Script::default();
    let columns = r#"[
        {"source":0,"target":"id","name":"id"},
        {"source":1,"target":"name","name":"mail"},
        {"source":2,"target":"amount","name":"skip","include":false}
    ]"#;
    import(
        &script,
        DriverKind::Postgres,
        true,
        &path,
        &[("COLUMNS", columns)],
    )
    .await
    .expect("the map is honoured");
    assert_eq!(
        script.inserts(),
        vec!["INSERT INTO \"public\".\"people\" (\"id\", \"name\") VALUES (1, 'a@b')".to_owned()]
    );

    // The sheet saw `email` at column 1, the file now has `mail`: refused, with no connection.
    let script = Script::default();
    let stale =
        r#"[{"source":0,"target":"id","name":"id"},{"source":1,"target":"name","name":"email"}]"#;
    let error = import(
        &script,
        DriverKind::Postgres,
        true,
        &path,
        &[("COLUMNS", stale)],
    )
    .await
    .expect_err("the file changed under the sheet");
    assert_eq!(
        error.message(),
        "COLUMNS[1] names 'email' but the file's column 1 is 'mail'; reload the file in the sheet"
    );
    assert_eq!(script.connects(), 0);
}

#[tokio::test]
async fn the_preview_lists_keys_and_rows_and_never_connects() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(
        &directory,
        "p.jsonl",
        "{\"id\": 1, \"name\": \"a\"}\n{\"id\": 2}\n{\"id\": 3, \"name\": \"c\"}\n",
    );
    let script = Script::default();
    let path_text = path.to_str().expect("utf-8 path");

    // No connection settings at all: the preview is decided before they are read.
    let run_preview = |extra: &[(&str, &str)]| {
        let mut pairs = vec![("IMPORT_PATH", path_text), ("IMPORT_PREVIEW", "1")];
        pairs.extend_from_slice(extra);
        let engine = FakeEngine {
            script: script.clone(),
            driver: FakeDriver {
                kind: DriverKind::Postgres,
                transactions: true,
            },
        };
        let settings = settings(&pairs);
        async move {
            let mut out = Capture::new();
            run(
                Command::ImportData,
                &settings,
                &mut out,
                &engine,
                &CancelFlag::new(),
            )
            .await
            .map(|()| out)
        }
    };

    let out = run_preview(&[("IMPORT_PREVIEW_ROWS", "2")])
        .await
        .expect("a preview");
    assert_eq!(
        out.lines,
        vec![
            json!({"event": "columns", "columns": [
                {"name": "id", "type": "text"}, {"name": "name", "type": "text"}
            ]}),
            json!({"event": "rows", "data": [["1", "a"], ["2", null]]}),
            json!({"event": "done", "format": "json", "streams": true}),
        ]
    );
    assert_eq!(script.connects(), 0);

    // The default is five rows, so all three of these.
    let out = run_preview(&[]).await.expect("a preview");
    assert_eq!(out.lines[1]["data"].as_array().expect("rows").len(), 3);

    // It is a JSON feature: a CSV or a `.sql` file is refused by name.
    let csv = write(&directory, "p.csv", "id\n1\n");
    let error = run(
        Command::ImportData,
        &settings(&[
            ("IMPORT_PATH", csv.to_str().expect("utf-8 path")),
            ("IMPORT_PREVIEW", "1"),
        ]),
        &mut Capture::new(),
        &FakeEngine {
            script: script.clone(),
            driver: FakeDriver {
                kind: DriverKind::Postgres,
                transactions: true,
            },
        },
        &CancelFlag::new(),
    )
    .await
    .expect_err("only JSON previews");
    assert!(
        error.message().contains("only offered for JSON"),
        "{}",
        error.message()
    );
}

// ---------------------------------------------------------------------------------------
// The three policies, and a transaction the server already ended
// ---------------------------------------------------------------------------------------

/// Five rows, two per batch, with the second batch refused by the server.
fn five_rows(directory: &tempfile::TempDir) -> PathBuf {
    write(
        directory,
        "five.jsonl",
        "{\"id\": 1, \"name\": \"a\"}\n{\"id\": 2, \"name\": \"b\"}\n\
         {\"id\": 3, \"name\": \"boom\"}\n{\"id\": 4, \"name\": \"d\"}\n\
         {\"id\": 5, \"name\": \"e\"}\n",
    )
}

#[tokio::test]
async fn stop_rolls_everything_back_and_names_the_line() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let script = Script::default();
    script.fail_on("boom", server_error(Some("1062")));
    let error = import(
        &script,
        DriverKind::Mysql,
        true,
        &five_rows(&directory),
        &[("IMPORT_BATCH", "2")],
    )
    .await
    .expect_err("stop is the default and it stops");
    assert!(
        error.message().contains("stopped at line 3"),
        "{}",
        error.message()
    );
    assert!(
        error.message().contains("rolled back"),
        "{}",
        error.message()
    );
    let seen = script.seen();
    assert_eq!(seen[1], "BEGIN");
    assert_eq!(seen.last().map(String::as_str), Some("ROLLBACK"));
    assert_eq!(
        script.inserts().len(),
        2,
        "the batch after the failure never runs"
    );
}

#[tokio::test]
async fn commit_keeps_the_prefix_when_the_server_only_undid_the_statement() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let script = Script::default();
    // MySQL 1062 (duplicate key): the statement is undone, the transaction lives on.
    script.fail_on("boom", server_error(Some("1062")));
    let events = import(
        &script,
        DriverKind::Mysql,
        true,
        &five_rows(&directory),
        &[("IMPORT_BATCH", "2"), ("ON_ERROR", "commit")],
    )
    .await
    .expect("commit keeps what worked");
    assert_eq!(script.seen().last().map(String::as_str), Some("COMMIT"));
    let done = done(&events);
    assert_eq!(done["rows"], 2);
    assert_eq!(done["stopped_at"], 3);
    assert_eq!(done["transaction"], true);
    assert!(done.get("warnings").is_none(), "{done}");
}

#[tokio::test]
async fn skip_sends_a_row_per_statement_without_a_transaction() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let script = Script::default();
    script.fail_on("boom", server_error(Some("1062")));
    let events = import(
        &script,
        DriverKind::Mysql,
        true,
        &five_rows(&directory),
        &[("ON_ERROR", "skip")],
    )
    .await
    .expect("skip keeps going");
    assert_eq!(script.inserts().len(), 5, "one INSERT per row");
    assert!(!script
        .seen()
        .iter()
        .any(|sql| sql == "BEGIN" || sql == "COMMIT"));
    let done = done(&events);
    assert_eq!(done["rows"], 4);
    assert_eq!(done["transaction"], false);
    assert_eq!(done["disposition"], "written");
    assert_eq!(done["errors"].as_array().expect("errors").len(), 1);
}

#[tokio::test]
async fn a_deadlock_under_commit_does_not_report_a_prefix_the_server_rolled_back() {
    // B-13 N2. MySQL answers a deadlock by rolling the whole transaction back, so the batch
    // that was already accepted is gone and a `COMMIT` has nothing to commit. The import used
    // to count it as written and send `done`.
    let directory = tempfile::tempdir().expect("a temporary directory");
    let script = Script::default();
    script.fail_on("boom", server_error(Some("1213")));
    let error = import(
        &script,
        DriverKind::Mysql,
        true,
        &five_rows(&directory),
        &[("IMPORT_BATCH", "2"), ("ON_ERROR", "commit")],
    )
    .await
    .expect_err("the prefix is gone, so this is a failure and not a done");
    assert!(
        error.message().contains("stopped at line 3"),
        "{}",
        error.message()
    );
    assert!(
        error.message().contains("no rows were written"),
        "{}",
        error.message()
    );
    assert!(
        error.message().contains("ON_ERROR=skip"),
        "{}",
        error.message()
    );
    let seen = script.seen();
    assert_eq!(
        seen.last().map(String::as_str),
        Some("ROLLBACK"),
        "it closes the transaction the safe way, not with a COMMIT: {seen:?}"
    );
    assert!(!seen.iter().any(|sql| sql == "COMMIT"));
    assert!(
        error
            .warnings()
            .iter()
            .any(|warning| warning.contains("line 3")),
        "{:?}",
        error.warnings()
    );
}

#[tokio::test]
async fn postgresql_ends_the_transaction_on_any_error_so_commit_cannot_keep_a_prefix() {
    // The same loss without a deadlock: a PostgreSQL transaction block is aborted by any
    // error, and the `COMMIT` after it is a `ROLLBACK`. Reporting the prefix would be a lie.
    let directory = tempfile::tempdir().expect("a temporary directory");
    let script = Script::default();
    script.fail_on("boom", server_error(Some("23505")));
    let error = import(
        &script,
        DriverKind::Postgres,
        true,
        &five_rows(&directory),
        &[("IMPORT_BATCH", "2"), ("ON_ERROR", "commit")],
    )
    .await
    .expect_err("nothing was written");
    assert!(
        error.message().contains("no rows were written"),
        "{}",
        error.message()
    );
    assert_eq!(script.seen().last().map(String::as_str), Some("ROLLBACK"));
}

#[tokio::test]
async fn a_lock_wait_timeout_under_commit_commits_and_says_it_may_not_have_kept_the_prefix() {
    // 1205 undoes only the statement unless `innodb_rollback_on_timeout` is on, and this
    // engine does not read that setting: it commits, as before, and tells the user.
    let directory = tempfile::tempdir().expect("a temporary directory");
    let script = Script::default();
    script.fail_on("boom", server_error(Some("1205")));
    let events = import(
        &script,
        DriverKind::Mysql,
        true,
        &five_rows(&directory),
        &[("IMPORT_BATCH", "2"), ("ON_ERROR", "commit")],
    )
    .await
    .expect("a timeout is not a verdict");
    assert_eq!(script.seen().last().map(String::as_str), Some("COMMIT"));
    let done = done(&events);
    assert_eq!(done["rows"], 2);
    let warnings = done["warnings"].as_array().expect("warnings");
    assert!(
        warnings[0]
            .as_str()
            .expect("text")
            .contains("innodb_rollback_on_timeout"),
        "{warnings:?}"
    );
}

#[tokio::test]
async fn a_failing_commit_is_an_error_and_not_a_done() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let script = Script::default();
    script.fail_on("COMMIT", server_error(Some("1213")));
    let error = import(
        &script,
        DriverKind::Mysql,
        true,
        &directory_file(&directory),
        &[],
    )
    .await
    .expect_err("the commit did not happen, so the import did not");
    assert!(!error.message().is_empty());
}

fn directory_file(directory: &tempfile::TempDir) -> PathBuf {
    write(directory, "one.jsonl", "{\"id\": 1, \"name\": \"a\"}\n")
}

#[tokio::test]
async fn a_read_only_connection_refuses_a_json_import_before_connecting() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let script = Script::default();
    let error = import(
        &script,
        DriverKind::Postgres,
        true,
        &directory_file(&directory),
        &[("SAFE_MODE", "read_only")],
    )
    .await
    .expect_err("read_only refuses a write");
    assert!(error.message().contains("read-only"), "{}", error.message());
    assert_eq!(script.connects(), 0);
}

// ---------------------------------------------------------------------------------------
// IMPORT_SQL_MAX_BYTES
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_sql_file_over_the_limit_is_refused_by_name_before_it_is_read() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(&directory, "big.sql", "SELECT 1;\nSELECT 2;\nSELECT 3;\n");
    let script = Script::default();
    let error = import(
        &script,
        DriverKind::Postgres,
        true,
        &path,
        &[("IMPORT_SQL_MAX_BYTES", "20")],
    )
    .await
    .expect_err("30 bytes do not fit in 20");
    let message = error.message();
    assert!(message.contains("30 bytes"), "{message}");
    assert!(message.contains("20-byte limit"), "{message}");
    assert!(message.contains("IMPORT_SQL_MAX_BYTES"), "{message}");
    assert_eq!(script.connects(), 0);

    // At the limit it runs.
    import(
        &script,
        DriverKind::Postgres,
        true,
        &path,
        &[("IMPORT_SQL_MAX_BYTES", "30")],
    )
    .await
    .expect("a file exactly at the limit is read");
}

#[tokio::test]
async fn an_unknown_format_names_json_among_the_choices() {
    let directory = tempfile::tempdir().expect("a temporary directory");
    let path = write(&directory, "x.txt", "{}");
    let error = import(&Script::default(), DriverKind::Postgres, true, &path, &[])
        .await
        .expect_err("a .txt file has no format");
    assert!(
        error.message().contains("csv, xlsx, json or sql"),
        "{}",
        error.message()
    );
}

// ---------------------------------------------------------------------------------------
// A real PostgreSQL
// ---------------------------------------------------------------------------------------

const SKIP_HINT: &str = "skipped: set QH_TEST_POSTGRES=1 with a throwaway PostgreSQL on port 55432";

fn live_settings() -> Option<Vec<(String, String)>> {
    if std::env::var("QH_TEST_POSTGRES").as_deref() != Ok("1") {
        return None;
    }
    Some(
        [
            ("DB_KIND", "postgres"),
            ("DB_HOST", "127.0.0.1"),
            ("DB_PORT", "55432"),
            ("DB_USER", "qh"),
            // The throwaway container's own password, as `real_server.rs` has it.
            ("DB_PASSWORD", "qh-dev-only"),
            ("DB_DATABASE", "qh"),
            ("DB_SCHEMA", "public"),
            ("DB_SSLMODE", "disable"),
            ("RETRIES", "0"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect(),
    )
}

/// Run `import_data` over `path` against the live server, loading `table`.
async fn live_import(
    path: &Path,
    table: &str,
    extra: &[(&str, &str)],
) -> Result<Capture, CliError> {
    let mut pairs = live_settings().expect("a live target");
    pairs.push(("IMPORT_PATH".to_owned(), path.display().to_string()));
    pairs.push(("TARGET_SCHEMA".to_owned(), "public".to_owned()));
    pairs.push(("TARGET_TABLE".to_owned(), table.to_owned()));
    for (key, value) in extra {
        pairs.push(((*key).to_owned(), (*value).to_owned()));
    }
    let mut out = Capture::new();
    run(
        Command::ImportData,
        &Settings::from_pairs(pairs),
        &mut out,
        &RealEngine::new(),
        &CancelFlag::new(),
    )
    .await?;
    Ok(out)
}

/// Run a read and return every row each `rows` event carried, in order.
async fn live_select(sql: &str) -> Vec<Json> {
    let mut pairs = live_settings().expect("a live target");
    pairs.push(("SQL".to_owned(), sql.to_owned()));
    let mut out = Capture::new();
    run(
        Command::Preview,
        &Settings::from_pairs(pairs),
        &mut out,
        &RealEngine::new(),
        &CancelFlag::new(),
    )
    .await
    .unwrap_or_else(|error| panic!("preview failed: {}", error.message()));
    out.lines
        .iter()
        .filter(|event| event["event"] == "rows")
        .flat_map(|event| match &event["data"] {
            Json::Array(rows) => rows.clone(),
            other => panic!("`data` is an array of rows, got {other}"),
        })
        .collect()
}

/// A fresh `table (id, name, amount, meta, active)`, with `id` the primary key.
async fn live_reset(directory: &tempfile::TempDir, table: &str) {
    let path = write(
        directory,
        &format!("{table}-setup.sql"),
        &format!(
            "DROP TABLE IF EXISTS {table};\n\
             CREATE TABLE {table} (id integer PRIMARY KEY, name text, amount numeric, \
                meta jsonb, active boolean);\n"
        ),
    );
    live_import(&path, table, &[])
        .await
        .expect("the table is created");
}

/// Drop `table` again so a finished test leaves the shared dev database as it found it
/// (the golden `*_tables_live` and `*_objects_live` cases list the `public` schema).
async fn live_drop(directory: &tempfile::TempDir, table: &str) {
    let path = write(
        directory,
        &format!("{table}-drop.sql"),
        &format!("DROP TABLE IF EXISTS {table};\n"),
    );
    live_import(&path, table, &[])
        .await
        .expect("the table is dropped");
}

/// Five rows whose fourth repeats the primary key of the second.
fn live_rows(directory: &tempfile::TempDir, table: &str) -> PathBuf {
    write(
        directory,
        &format!("{table}.jsonl"),
        "{\"id\": 1, \"name\": \"a\"}\n{\"id\": 2, \"name\": \"b\"}\n\
         {\"id\": 3, \"name\": \"c\"}\n{\"id\": 2, \"name\": \"dup\"}\n{\"id\": 5, \"name\": \"e\"}\n",
    )
}

async fn live_ids(table: &str) -> Vec<String> {
    live_select(&format!("SELECT id FROM {table} ORDER BY id"))
        .await
        .into_iter()
        .map(|row| row[0].as_str().expect("an id").to_owned())
        .collect()
}

#[tokio::test]
async fn live_values_land_as_the_file_wrote_them() {
    if live_settings().is_none() {
        eprintln!("{SKIP_HINT}");
        return;
    }
    let directory = tempfile::tempdir().expect("a temporary directory");
    let table = "qh_json_values";
    live_reset(&directory, table).await;
    let path = write(
        &directory,
        "values.json",
        concat!(
            "[",
            r#"{"id": 1, "name": "Ada", "amount": 1.10, "meta": {"a": [1, 2], "b": null}, "active": true},"#,
            r#"{"id": 2, "name": "", "amount": 123456789012345678901234567890, "meta": null},"#,
            r#"{"id": 3, "name": null, "active": false}"#,
            "]"
        ),
    );
    let events = live_import(&path, table, &[])
        .await
        .unwrap_or_else(|error| panic!("{}", error.message()));
    assert_eq!(done(&events)["rows"], 3);

    let rows = live_select(&format!(
        "SELECT id, name, amount::text, meta::text, active::text FROM {table} ORDER BY id"
    ))
    .await;
    assert_eq!(
        rows,
        vec![
            json!(["1", "Ada", "1.10", "{\"a\": [1, 2], \"b\": null}", "true"]),
            json!(["2", "", "123456789012345678901234567890", null, null]),
            json!(["3", null, null, null, "false"]),
        ]
    );
    live_drop(&directory, table).await;
}

#[tokio::test]
async fn live_stop_rolls_the_whole_file_back() {
    if live_settings().is_none() {
        eprintln!("{SKIP_HINT}");
        return;
    }
    let directory = tempfile::tempdir().expect("a temporary directory");
    let table = "qh_json_stop";
    live_reset(&directory, table).await;
    let error = live_import(
        &live_rows(&directory, table),
        table,
        &[("IMPORT_BATCH", "2")],
    )
    .await
    .expect_err("the repeated key stops the import");
    assert!(
        error.message().contains("stopped at line 3"),
        "{}",
        error.message()
    );
    assert!(
        error.message().contains("rolled back"),
        "{}",
        error.message()
    );
    assert!(
        error
            .warnings()
            .iter()
            .any(|warning| warning.contains("duplicate key")),
        "the server's own words are kept: {:?}",
        error.warnings()
    );
    assert_eq!(
        live_ids(table).await,
        Vec::<String>::new(),
        "nothing landed"
    );
    live_drop(&directory, table).await;
}

#[tokio::test]
async fn live_commit_cannot_keep_a_prefix_on_postgresql_and_says_so() {
    if live_settings().is_none() {
        eprintln!("{SKIP_HINT}");
        return;
    }
    let directory = tempfile::tempdir().expect("a temporary directory");
    let table = "qh_json_commit";
    live_reset(&directory, table).await;
    // PostgreSQL aborts the transaction block on the duplicate key, so the first batch
    // is gone with it. What the import reports must match what the table holds.
    let error = live_import(
        &live_rows(&directory, table),
        table,
        &[("IMPORT_BATCH", "2"), ("ON_ERROR", "commit")],
    )
    .await
    .expect_err("there is no prefix to keep, so this is a failure and not a done");
    assert!(
        error.message().contains("stopped at line 3"),
        "{}",
        error.message()
    );
    assert!(
        error.message().contains("no rows were written"),
        "{}",
        error.message()
    );
    assert_eq!(
        live_ids(table).await,
        Vec::<String>::new(),
        "the table agrees"
    );
    live_drop(&directory, table).await;
}

#[tokio::test]
async fn live_skip_keeps_every_valid_row_without_a_transaction() {
    if live_settings().is_none() {
        eprintln!("{SKIP_HINT}");
        return;
    }
    let directory = tempfile::tempdir().expect("a temporary directory");
    let table = "qh_json_skip";
    live_reset(&directory, table).await;
    let events = live_import(
        &live_rows(&directory, table),
        table,
        &[("ON_ERROR", "skip")],
    )
    .await
    .unwrap_or_else(|error| panic!("{}", error.message()));
    let done = done(&events);
    assert_eq!(done["rows"], 4);
    assert_eq!(done["transaction"], false);
    assert_eq!(
        done["errors"].as_array().expect("errors").len(),
        1,
        "{done}"
    );
    assert_eq!(live_ids(table).await, vec!["1", "2", "3", "5"]);
    // The row that was skipped did not overwrite the one that was already there.
    let rows = live_select(&format!("SELECT name FROM {table} WHERE id = 2")).await;
    assert_eq!(rows, vec![json!(["b"])]);
    live_drop(&directory, table).await;
}
