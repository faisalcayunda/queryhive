//! `to_table` across two connections, proven with fakes on both sides (W13-T20 step 7).
//!
//! A rig stands in for the whole engine: its `connect` hands out a scripted source session, and
//! its `for_settings` (the second-connection seam) hands out a destination whose every call is
//! written to one shared, ordered log. What the tests assert is therefore the *sequence* the two
//! servers would see, which is what the claims are about:
//!
//! - nothing is sent to the destination that its own Safe Mode does not allow, statement by
//!   statement, and nothing at all when the plan is refused;
//! - the source is read-only, its text settings are fixed first (C-3), and it is asked to cancel
//!   **before** its cursor is dropped on every way out but a finished cursor (B-5);
//! - the destination is left as it was on every failure: rolled back, or the scratch dropped;
//! - the rows read are the rows the destination acknowledged, or nothing is swapped in.
//!
//! The real servers (the `COPY` itself, `CopyFail`, the MySQL protocol) are not here: those are
//! pending live.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use qh_core::{ColumnBatch, ColumnMeta, EngineError, FailureKind, Value};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, CopyIn, Cursor, Driver, DriverKind,
    ExecuteOptions, ObjectPath, ObjectsPage, Parameter, Session, SqlType,
};
use qh_ffi::events::Capture;
use qh_ffi::{run, CancelFlag, CliError, Command, Engine, ErrorSide, RealEngine, Settings};
use serde_json::Value as Json;

type Log = Arc<Mutex<Vec<String>>>;

fn note(log: &Log, line: impl Into<String>) {
    log.lock().expect("log").push(line.into());
}

// --------------------------------------------------------------------------- //
// the source
// --------------------------------------------------------------------------- //

enum Step {
    /// A batch, column by column.
    Rows(Vec<Vec<Value>>),
    Fail(EngineError),
    Pause(u64),
    /// The user presses Stop.
    Stop(CancelFlag),
}

struct Source {
    columns: Vec<ColumnMeta>,
    types: Option<Vec<SqlType>>,
    steps: Mutex<VecDeque<Step>>,
}

struct SourceSession {
    log: Log,
    source: Arc<Source>,
    kind: DriverKind,
}

struct SourceCursor {
    log: Log,
    source: Arc<Source>,
    batches: usize,
}

impl Drop for SourceCursor {
    fn drop(&mut self) {
        note(&self.log, "src:drop-cursor");
    }
}

#[async_trait]
impl Cursor for SourceCursor {
    fn columns(&self) -> &[ColumnMeta] {
        &self.source.columns
    }

    fn column_types(&self) -> Option<&[SqlType]> {
        self.source.types.as_deref()
    }

    async fn next_batch(&mut self, _max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        loop {
            let step = self.source.steps.lock().expect("steps").pop_front();
            match step {
                None => return Ok(None),
                Some(Step::Pause(ms)) => tokio::time::sleep(Duration::from_millis(ms)).await,
                Some(Step::Stop(flag)) => flag.request(),
                Some(Step::Fail(error)) => return Err(error),
                Some(Step::Rows(columns)) => {
                    self.batches += 1;
                    note(&self.log, format!("src:batch {}", self.batches));
                    return Ok(Some(ColumnBatch::new(columns).expect("a square batch")));
                }
            }
        }
    }
}

struct EmptyCursor;

#[async_trait]
impl Cursor for EmptyCursor {
    fn columns(&self) -> &[ColumnMeta] {
        &[]
    }

    async fn next_batch(&mut self, _max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        Ok(None)
    }
}

#[async_trait]
impl Session for SourceSession {
    fn capabilities(&self) -> Capabilities {
        RealEngine::new().driver(self.kind).capabilities()
    }

    fn query_id(&self) -> Option<String> {
        Some("source-query-1".to_owned())
    }

    fn read_only_statement(&self) -> Option<&'static str> {
        Some("SET SESSION CHARACTERISTICS AS READ ONLY")
    }

    async fn execute(
        &mut self,
        sql: &str,
        _options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        note(&self.log, format!("src:execute {sql}"));
        if sql.starts_with("SET ") {
            return Ok(Box::new(EmptyCursor));
        }
        Ok(Box::new(SourceCursor {
            log: self.log.clone(),
            source: self.source.clone(),
            batches: 0,
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
        note(&self.log, "src:cancel");
        Ok(())
    }

    async fn close(self: Box<Self>) -> Result<(), EngineError> {
        note(&self.log, "src:close");
        Ok(())
    }
}

// --------------------------------------------------------------------------- //
// the destination
// --------------------------------------------------------------------------- //

#[derive(Default)]
struct DestScript {
    /// Whether the table already exists.
    exists: bool,
    /// A statement containing any of these fails.
    fail_on: Vec<&'static str>,
    /// Every send of a `COPY` fails.
    fail_send: bool,
    /// What a finished `COPY` claims to have loaded, instead of the lines it was sent.
    copy_claims: Option<u64>,
    /// What an `INSERT` claims to have written, instead of its rows.
    insert_claims: Option<u64>,
    /// Each `COPY`: its statement and everything sent to it.
    copied: Mutex<Vec<(String, String)>>,
    /// The statement bound every call to the destination carried, with the statement's start.
    bounds: Mutex<Vec<(String, Option<Duration>)>>,
}

struct DestSession {
    log: Log,
    script: Arc<DestScript>,
    kind: DriverKind,
}

struct DestCursor {
    cell: Option<Value>,
    affected: Option<u64>,
}

#[async_trait]
impl Cursor for DestCursor {
    fn columns(&self) -> &[ColumnMeta] {
        static ONE: std::sync::OnceLock<Vec<ColumnMeta>> = std::sync::OnceLock::new();
        ONE.get_or_init(|| vec![ColumnMeta::new("c", "text")])
    }

    async fn next_batch(&mut self, _max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        Ok(self
            .cell
            .take()
            .map(|cell| ColumnBatch::new(vec![vec![cell]]).expect("one cell")))
    }

    fn affected_rows(&self) -> Option<u64> {
        self.affected
    }
}

struct DestCopy {
    log: Log,
    script: Arc<DestScript>,
    statement: String,
    data: Vec<u8>,
    finished: bool,
}

#[async_trait]
impl CopyIn for DestCopy {
    async fn send(&mut self, data: Vec<u8>) -> Result<(), EngineError> {
        if self.script.fail_send {
            return Err(refused("the copy was refused"));
        }
        note(&self.log, format!("dst:copy-send {} bytes", data.len()));
        self.data.extend(data);
        Ok(())
    }

    async fn finish(mut self: Box<Self>) -> Result<u64, EngineError> {
        self.finished = true;
        note(&self.log, "dst:copy-finish");
        let text = String::from_utf8(std::mem::take(&mut self.data)).expect("text");
        let lines = text.matches('\n').count() as u64;
        self.script
            .copied
            .lock()
            .expect("copied")
            .push((self.statement.clone(), text));
        Ok(self.script.copy_claims.unwrap_or(lines))
    }
}

impl Drop for DestCopy {
    fn drop(&mut self) {
        if !self.finished {
            note(&self.log, "dst:copy-fail");
        }
    }
}

fn refused(message: &str) -> EngineError {
    EngineError::Query {
        message: message.to_owned(),
        code: Some("42501".to_owned()),
        position: None,
        kind: FailureKind::Permanent,
    }
}

impl DestSession {
    fn bound(&self, sql: &str, options: &ExecuteOptions) {
        self.script
            .bounds
            .lock()
            .expect("bounds")
            .push((sql.to_owned(), options.statement_timeout));
    }

    fn check(&self, sql: &str) -> Result<(), EngineError> {
        if self
            .script
            .fail_on
            .iter()
            .any(|needle| sql.contains(needle))
        {
            return Err(refused("the destination refused the statement"));
        }
        Ok(())
    }
}

#[async_trait]
impl Session for DestSession {
    fn capabilities(&self) -> Capabilities {
        RealEngine::new().driver(self.kind).capabilities()
    }

    fn query_id(&self) -> Option<String> {
        None
    }

    async fn execute(
        &mut self,
        sql: &str,
        options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        note(&self.log, format!("dst:execute {sql}"));
        self.bound(sql, options);
        self.check(sql)?;
        if sql.starts_with("SELECT to_regclass") {
            let cell = if self.script.exists {
                Value::Text("public.people".into())
            } else {
                Value::Null
            };
            return Ok(Box::new(DestCursor {
                cell: Some(cell),
                affected: None,
            }));
        }
        if sql.starts_with("SELECT @@max_allowed_packet") {
            return Ok(Box::new(DestCursor {
                cell: Some(Value::Int(67_108_864)),
                affected: None,
            }));
        }
        Ok(Box::new(DestCursor {
            cell: None,
            affected: None,
        }))
    }

    async fn execute_bound(
        &mut self,
        sql: &str,
        parameters: &[Parameter],
        options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        note(&self.log, format!("dst:bound {sql} {parameters:?}"));
        self.bound(sql, options);
        self.check(sql)?;
        if sql.starts_with("SELECT COUNT(*) FROM information_schema.tables") {
            return Ok(Box::new(DestCursor {
                cell: Some(Value::Int(i64::from(self.script.exists))),
                affected: None,
            }));
        }
        let rows = sql.matches("(?").count() as u64;
        Ok(Box::new(DestCursor {
            cell: None,
            affected: Some(self.script.insert_claims.unwrap_or(rows)),
        }))
    }

    async fn copy_in(
        &mut self,
        statement: &str,
        options: &ExecuteOptions,
    ) -> Result<Box<dyn CopyIn>, EngineError> {
        note(&self.log, format!("dst:copy-in {statement}"));
        self.bound(statement, options);
        self.check(statement)?;
        Ok(Box::new(DestCopy {
            log: self.log.clone(),
            script: self.script.clone(),
            statement: statement.to_owned(),
            data: Vec::new(),
            finished: false,
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
        note(&self.log, "dst:cancel");
        Ok(())
    }

    async fn close(self: Box<Self>) -> Result<(), EngineError> {
        note(&self.log, "dst:close");
        Ok(())
    }
}

// --------------------------------------------------------------------------- //
// the engine
// --------------------------------------------------------------------------- //

struct Rig {
    inner: RealEngine,
    log: Log,
    source: Arc<Source>,
    dest: Arc<DestScript>,
    seam: bool,
    /// The settings `connect` saw for the source side, and `for_settings` for the other.
    source_configs: Mutex<Vec<ConnectionConfig>>,
    dest_settings: Mutex<Vec<Settings>>,
}

struct DestEngine {
    inner: RealEngine,
    log: Log,
    script: Arc<DestScript>,
}

#[async_trait]
impl Engine for Rig {
    fn kinds(&self) -> Vec<DriverKind> {
        self.inner.kinds()
    }

    fn driver(&self, kind: DriverKind) -> &dyn Driver {
        self.inner.driver(kind)
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        note(&self.log, "src:connect");
        self.source_configs
            .lock()
            .expect("configs")
            .push(config.clone());
        Ok(Box::new(SourceSession {
            log: self.log.clone(),
            source: self.source.clone(),
            kind: config.kind,
        }))
    }

    fn for_settings(&self, settings: Settings) -> Option<Box<dyn Engine>> {
        if !self.seam {
            return None;
        }
        self.dest_settings.lock().expect("settings").push(settings);
        Some(Box::new(DestEngine {
            inner: RealEngine::new(),
            log: self.log.clone(),
            script: self.dest.clone(),
        }))
    }
}

#[async_trait]
impl Engine for DestEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        self.inner.kinds()
    }

    fn driver(&self, kind: DriverKind) -> &dyn Driver {
        self.inner.driver(kind)
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        note(&self.log, "dst:connect");
        Ok(Box::new(DestSession {
            log: self.log.clone(),
            script: self.script.clone(),
            kind: config.kind,
        }))
    }
}

// --------------------------------------------------------------------------- //
// the harness
// --------------------------------------------------------------------------- //

fn int_column(values: &[i64]) -> Vec<Value> {
    values.iter().map(|value| Value::Int(*value)).collect()
}

fn text_column(values: &[&str]) -> Vec<Value> {
    values
        .iter()
        .map(|value| Value::Text((*value).into()))
        .collect()
}

/// The ordinary source: two columns, `id bigint` and `name text`.
fn people(steps: Vec<Step>) -> Source {
    Source {
        columns: vec![
            ColumnMeta::new("id", "bigint"),
            ColumnMeta::new("name", "varchar"),
        ],
        types: Some(vec![
            SqlType::of(qh_driver::Base::Int64),
            SqlType::of(qh_driver::Base::Text),
        ]),
        steps: Mutex::new(steps.into()),
    }
}

fn two_people() -> Vec<Step> {
    vec![Step::Rows(vec![
        int_column(&[1, 2]),
        text_column(&["ann", "bob"]),
    ])]
}

struct Outcome {
    result: Result<(), CliError>,
    events: Vec<Json>,
    log: Vec<String>,
    rig: Rig,
}

impl Outcome {
    fn position(&self, prefix: &str) -> usize {
        self.log
            .iter()
            .position(|line| line.starts_with(prefix))
            .unwrap_or_else(|| panic!("no `{prefix}` in {:#?}", self.log))
    }

    fn has(&self, prefix: &str) -> bool {
        self.log.iter().any(|line| line.starts_with(prefix))
    }

    fn count(&self, prefix: &str) -> usize {
        self.log
            .iter()
            .filter(|line| line.starts_with(prefix))
            .count()
    }

    fn lines(&self, prefix: &str) -> Vec<&str> {
        self.log
            .iter()
            .filter(|line| line.starts_with(prefix))
            .map(String::as_str)
            .collect()
    }

    fn event(&self, name: &str) -> &Json {
        self.events
            .iter()
            .find(|event| event["event"] == name)
            .unwrap_or_else(|| panic!("no `{name}` event in {:#?}", self.events))
    }

    fn steps(&self) -> Vec<String> {
        self.events
            .iter()
            .filter(|event| event["event"] == "step")
            .map(|event| event["step"].as_str().unwrap().to_owned())
            .collect()
    }

    fn untouched(&self) -> bool {
        self.log.is_empty() && self.events.is_empty()
    }
}

fn settings(pairs: &[(&str, &str)]) -> Settings {
    Settings::from_pairs(pairs.iter().map(|(key, value)| (*key, *value)))
}

/// The source side: a Trino connection, with a password the destination must never see.
const SOURCE: [(&str, &str); 6] = [
    ("DB_KIND", "trino"),
    ("DB_HOST", "source.invalid"),
    ("DB_USER", "ann"),
    ("DB_PASSWORD", "source-password"),
    ("RETRIES", "0"),
    ("SQL", "SELECT id, name FROM people;"),
];

/// A PostgreSQL destination, `full`, writing `public.people`.
const TO_POSTGRES: [(&str, &str); 7] = [
    ("DEST_DB_KIND", "postgres"),
    ("DEST_DB_HOST", "target.invalid"),
    ("DEST_DB_PASSWORD", "dest-password"),
    ("DEST_SAFE_MODE", "full"),
    ("TARGET_SCHEMA", "public"),
    ("TARGET_TABLE", "people"),
    ("DEST_RETRIES", "0"),
];

const TO_MYSQL: [(&str, &str); 7] = [
    ("DEST_DB_KIND", "mysql"),
    ("DEST_DB_HOST", "target.invalid"),
    ("DEST_DB_PASSWORD", "dest-password"),
    ("DEST_SAFE_MODE", "full"),
    ("TARGET_CATALOG", "shop"),
    ("TARGET_TABLE", "people"),
    ("DEST_RETRIES", "0"),
];

/// Replace or add keys.
fn with(
    base: &[(&'static str, &'static str)],
    extra: &[(&'static str, &'static str)],
) -> Vec<(&'static str, &'static str)> {
    let mut all: Vec<(&str, &str)> = base
        .iter()
        .filter(|(key, _)| !extra.iter().any(|(other, _)| other == key))
        .copied()
        .collect();
    all.extend_from_slice(extra);
    all
}

async fn transfer_with(
    pairs: &[(&str, &str)],
    source: Source,
    dest: DestScript,
    seam: bool,
    cancel: CancelFlag,
) -> Outcome {
    let log: Log = Arc::default();
    let rig = Rig {
        inner: RealEngine::new(),
        log: log.clone(),
        source: Arc::new(source),
        dest: Arc::new(dest),
        seam,
        source_configs: Mutex::default(),
        dest_settings: Mutex::default(),
    };
    let mut out = Capture::new();
    let result = run(Command::ToTable, &settings(pairs), &mut out, &rig, &cancel).await;
    let log = log.lock().expect("log").clone();
    Outcome {
        result,
        events: out.lines,
        log,
        rig,
    }
}

async fn transfer(pairs: &[(&str, &str)], source: Source, dest: DestScript) -> Outcome {
    transfer_with(pairs, source, dest, true, CancelFlag::new()).await
}

fn sided(result: &Result<(), CliError>) -> (ErrorSide, String) {
    match result {
        Err(error) => (error.side().expect("a placed error"), error.message()),
        Ok(()) => panic!("expected an error"),
    }
}

fn all(
    base: &[(&'static str, &'static str)],
    extra: &[(&'static str, &'static str)],
) -> Vec<(&'static str, &'static str)> {
    let mut pairs = SOURCE.to_vec();
    pairs.extend(with(base, extra));
    pairs
}

// --------------------------------------------------------------------------- //
// the happy paths
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn a_postgres_create_is_one_transaction_with_the_rows_in_a_copy() {
    let pairs = all(&TO_POSTGRES, &[]);
    let out = transfer(&pairs, people(two_people()), DestScript::default()).await;
    out.result.as_ref().expect("the transfer ran");

    // The destination saw exactly this, in this order, and nothing else.
    let dst: Vec<&str> = out
        .log
        .iter()
        .filter(|line| line.starts_with("dst:"))
        .map(String::as_str)
        .collect();
    assert_eq!(
        dst,
        [
            "dst:connect",
            // An unquoted NULL in an array literal is a NULL, not the string 'NULL'.
            "dst:execute SET array_nulls = on",
            "dst:execute SELECT to_regclass('public.people')",
            "dst:execute BEGIN",
            "dst:execute CREATE TABLE public.people (id bigint, name text)",
            "dst:copy-in COPY public.people (id, name) FROM STDIN",
            "dst:copy-send 12 bytes",
            "dst:copy-finish",
            "dst:execute COMMIT",
            "dst:close",
        ]
    );
    let copied = out.rig.dest.copied.lock().unwrap().clone();
    assert_eq!(copied[0].1, "1\tann\n2\tbob\n");

    // The source was only read, on a session the server made read-only, and its trailing
    // semicolon is gone.
    let src = out.lines("src:execute");
    assert_eq!(
        src,
        [
            "src:execute SET SESSION CHARACTERISTICS AS READ ONLY",
            "src:execute SELECT id, name FROM people"
        ]
    );
    assert!(!out.has("src:cancel"), "a finished cursor is not cancelled");
    assert!(out.has("src:close"));

    assert_eq!(
        out.steps(),
        [
            "connect_target",
            "connect",
            "prepare",
            "write",
            "verify",
            "swap"
        ]
    );
    let start = out.event("start");
    assert_eq!(start["query_id"], "source-query-1");
    assert_eq!(start["target_columns"][1]["type"], "text");
    assert_eq!(start["fallbacks"].as_array().unwrap().len(), 0);
    let done = out.event("done");
    assert_eq!(done["rows"], 2);
    assert_eq!(done["rows_read"], 2);
    assert_eq!(done["verified"], true);
    assert_eq!(done["cancelled"], false);
    assert_eq!(done["table"], "public.people");
    assert_eq!(done["mode"], "create");
    assert_eq!(done["target_kind"], "postgres");
}

#[tokio::test]
async fn a_postgres_source_has_its_text_settings_fixed_before_the_query() {
    // C-3: the four SETs, after the read-only statement and before the query.
    let mut pairs = all(&TO_POSTGRES, &[]);
    pairs.retain(|(key, _)| !key.starts_with("DB_"));
    pairs.extend([("DB_KIND", "postgres"), ("DB_HOST", "source.invalid")]);
    let out = transfer(&pairs, people(two_people()), DestScript::default()).await;
    out.result.as_ref().expect("ran");
    let src = out.lines("src:execute");
    assert_eq!(
        src,
        [
            "src:execute SET SESSION CHARACTERISTICS AS READ ONLY",
            "src:execute SET extra_float_digits = 3",
            "src:execute SET DateStyle = ISO",
            "src:execute SET IntervalStyle = postgres",
            "src:execute SET bytea_output = hex",
            "src:execute SELECT id, name FROM people",
        ]
    );
    // A Trino or MySQL source gets none of them.
    let out = transfer(
        &all(&TO_POSTGRES, &[]),
        people(two_people()),
        DestScript::default(),
    )
    .await;
    assert!(!out.has("src:execute SET DateStyle"));
}

#[tokio::test]
async fn a_postgres_replace_builds_beside_the_table_and_swaps_inside_the_transaction() {
    let pairs = all(&TO_POSTGRES, &[("WRITE_MODE", "replace")]);
    let dest = DestScript {
        exists: true,
        ..DestScript::default()
    };
    let out = transfer(&pairs, people(two_people()), dest).await;
    out.result.as_ref().expect("ran");
    let dst = out.lines("dst:execute");
    assert!(dst[2].starts_with("dst:execute BEGIN"), "{dst:#?}");
    assert!(dst[3].contains("CREATE TABLE public.qh_new_"), "{dst:#?}");
    assert!(
        dst[4].starts_with("dst:execute DROP TABLE IF EXISTS public.people"),
        "{dst:#?}"
    );
    assert!(dst[5].contains("ALTER TABLE public.qh_new_"), "{dst:#?}");
    assert!(dst[5].ends_with("RENAME TO people"), "{dst:#?}");
    assert_eq!(dst[6], "dst:execute COMMIT");
    // The old table is touched only after the rows are all in.
    assert!(
        out.position("dst:copy-finish")
            < out.position("dst:execute DROP TABLE IF EXISTS public.people")
    );
    assert_eq!(out.event("done")["mode"], "replace");
    assert!(!out.has("dst:execute ROLLBACK"));
}

#[tokio::test]
async fn a_mysql_create_builds_a_scratch_table_and_renames_it_in() {
    let pairs = all(&TO_MYSQL, &[]);
    let out = transfer(&pairs, people(two_people()), DestScript::default()).await;
    out.result.as_ref().expect("ran");
    let dst: Vec<String> = out
        .log
        .iter()
        .filter(|line| line.starts_with("dst:"))
        .map(|line| line.split(" [").next().unwrap().to_owned())
        .collect();
    assert_eq!(dst[0], "dst:connect");
    // The session first: strict mode with no stray comma, utf8mb4, the packet size.
    assert!(dst[1].starts_with("dst:execute SET SESSION sql_mode = CONCAT_WS(',', NULLIF(@@sql_mode, ''), 'STRICT_ALL_TABLES')"), "{dst:#?}");
    assert_eq!(dst[2], "dst:execute SET NAMES utf8mb4");
    assert_eq!(dst[3], "dst:execute SELECT @@max_allowed_packet");
    assert!(
        dst[4].starts_with("dst:bound SELECT COUNT(*) FROM information_schema.tables"),
        "{dst:#?}"
    );
    assert!(
        dst[5].contains("CREATE TABLE shop.qh_new_") && dst[5].ends_with("DEFAULT CHARSET=utf8mb4"),
        "{dst:#?}"
    );
    assert!(
        dst[6].contains("INSERT INTO shop.qh_new_") && dst[6].contains("VALUES (?, ?), (?, ?)"),
        "{dst:#?}"
    );
    assert!(
        dst[7].contains("RENAME TABLE shop.qh_new_") && dst[7].contains("TO shop.people"),
        "{dst:#?}"
    );
    assert_eq!(dst[8], "dst:close");
    // The values go as bound parameters, not as text of the statement.
    let insert = out.lines("dst:bound INSERT")[0];
    assert!(
        insert.ends_with("[Int(1), Text(\"ann\"), Int(2), Text(\"bob\")]"),
        "{insert}"
    );
    assert_eq!(out.event("done")["rows"], 2);
    assert_eq!(out.event("done")["target_kind"], "mysql");
    assert!(!out.has("dst:copy-in"), "MySQL has no bulk copy");
}

#[tokio::test]
async fn a_mysql_replace_moves_the_old_table_aside_and_drops_it_last() {
    let pairs = all(&TO_MYSQL, &[("WRITE_MODE", "replace")]);
    let dest = DestScript {
        exists: true,
        ..DestScript::default()
    };
    let out = transfer(&pairs, people(two_people()), dest).await;
    out.result.as_ref().expect("ran");
    let executes = out.lines("dst:execute");
    let rename = executes
        .iter()
        .position(|line| line.contains("RENAME TABLE shop.people TO shop.qh_old_"))
        .expect("one atomic rename");
    let drop = executes
        .iter()
        .position(|line| line.contains("DROP TABLE shop.qh_old_"))
        .expect("the old table dropped");
    assert!(rename < drop, "{executes:#?}");
    // The only warning is the widening of `name` (text of no length is a longtext); nothing was
    // kept or left behind.
    let warnings = out.event("done")["warnings"].to_string();
    assert!(
        warnings.contains("longtext") && !warnings.contains("kept as"),
        "{warnings}"
    );

    // A drop that fails is a warning that names the table kept, and the new one is in place.
    let dest = DestScript {
        exists: true,
        fail_on: vec!["DROP TABLE shop.qh_old_"],
        ..DestScript::default()
    };
    let out = transfer(&pairs, people(two_people()), dest).await;
    out.result.as_ref().expect("the swap is done");
    let warnings = out.event("done")["warnings"].to_string();
    assert!(
        warnings.contains("the old one is kept as shop.qh_old_"),
        "{warnings}"
    );
}

#[tokio::test]
async fn a_query_with_no_rows_makes_an_empty_table_and_still_verifies() {
    for (pairs, no_write) in [
        (all(&TO_POSTGRES, &[]), "dst:copy-in"),
        (all(&TO_MYSQL, &[]), "dst:bound INSERT"),
    ] {
        let out = transfer(&pairs, people(Vec::new()), DestScript::default()).await;
        out.result.as_ref().expect("an empty result is a result");
        assert!(!out.has(no_write), "{:#?}", out.log);
        let done = out.event("done");
        assert_eq!(done["rows"], 0);
        assert_eq!(done["rows_read"], 0);
        assert_eq!(done["verified"], true);
        assert!(out.has("dst:execute CREATE TABLE"));
    }
}

// --------------------------------------------------------------------------- //
// windows and batches
// --------------------------------------------------------------------------- //

fn five_people() -> Vec<Step> {
    vec![Step::Rows(vec![
        int_column(&[1, 2, 3, 4, 5]),
        text_column(&["a", "b", "c", "d", "e"]),
    ])]
}

#[tokio::test]
async fn a_copy_window_closes_on_rows_and_on_bytes_and_every_row_is_acknowledged() {
    let by_rows = all(&TO_POSTGRES, &[("TRANSFER_COPY_ROWS", "2")]);
    let out = transfer(&by_rows, people(five_people()), DestScript::default()).await;
    out.result.as_ref().expect("ran");
    let copied = out.rig.dest.copied.lock().unwrap().clone();
    let sizes: Vec<usize> = copied
        .iter()
        .map(|(_, data)| data.matches('\n').count())
        .collect();
    assert_eq!(sizes, [2, 2, 1]);
    assert_eq!(out.count("dst:copy-in"), 3);
    assert_eq!(
        out.count("dst:execute BEGIN"),
        1,
        "one transaction around every window"
    );
    assert_eq!(out.count("dst:execute COMMIT"), 1);
    assert_eq!(out.event("done")["rows"], 5);

    // 10 bytes: two 4-byte rows reach it.
    let by_bytes = all(&TO_POSTGRES, &[("TRANSFER_COPY_BYTES", "7")]);
    let out = transfer(&by_bytes, people(five_people()), DestScript::default()).await;
    out.result.as_ref().expect("ran");
    let copied = out.rig.dest.copied.lock().unwrap().clone();
    assert!(copied.len() >= 3, "{copied:?}");
    let total: usize = copied
        .iter()
        .map(|(_, data)| data.matches('\n').count())
        .sum();
    assert_eq!(total, 5);
}

#[tokio::test]
async fn a_slow_source_does_not_hold_a_copy_window_open_past_its_timer() {
    // DEST_STATEMENT_TIMEOUT_MS=40 gives a 10 ms window. The source pauses for 80 ms after its
    // first rows, so the window must close while the next batch is still on its way (N-4).
    let pairs = all(&TO_POSTGRES, &[("DEST_STATEMENT_TIMEOUT_MS", "40")]);
    let steps = vec![
        Step::Rows(vec![int_column(&[1, 2]), text_column(&["a", "b"])]),
        Step::Pause(80),
        Step::Rows(vec![int_column(&[3]), text_column(&["c"])]),
    ];
    let out = transfer(&pairs, people(steps), DestScript::default()).await;
    out.result.as_ref().expect("ran");
    assert!(
        out.position("dst:copy-finish") < out.position("src:batch 2"),
        "{:#?}",
        out.log
    );
    assert_eq!(
        out.count("dst:copy-in"),
        2,
        "a new window for the rows after the pause"
    );
    assert_eq!(out.event("done")["rows"], 3);
}

#[tokio::test]
async fn mysql_inserts_in_batches_with_bound_values() {
    let pairs = all(&TO_MYSQL, &[("TRANSFER_BATCH_ROWS", "2")]);
    let out = transfer(&pairs, people(five_people()), DestScript::default()).await;
    out.result.as_ref().expect("ran");
    let inserts = out.lines("dst:bound INSERT");
    assert_eq!(inserts.len(), 3);
    let widths: Vec<usize> = inserts
        .iter()
        .map(|line| line.matches("(?").count())
        .collect();
    assert_eq!(widths, [2, 2, 1]);
    assert_eq!(out.event("done")["rows"], 5);
}

// --------------------------------------------------------------------------- //
// verification
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn rows_read_must_be_the_rows_acknowledged_or_nothing_is_swapped_in() {
    // PostgreSQL: the server says it loaded fewer rows than were sent.
    let pairs = all(&TO_POSTGRES, &[]);
    let dest = DestScript {
        copy_claims: Some(1),
        ..DestScript::default()
    };
    let out = transfer(&pairs, people(two_people()), dest).await;
    let (side, message) = sided(&out.result);
    assert_eq!(side, ErrorSide::Target);
    assert!(message.contains("loaded 1 of 2"), "{message}");
    assert!(out.has("dst:execute ROLLBACK"));
    assert!(!out.has("dst:execute COMMIT"));

    // MySQL: the count of a batch disagrees; the scratch table is dropped, never renamed in.
    let pairs = all(&TO_MYSQL, &[]);
    let dest = DestScript {
        insert_claims: Some(1),
        ..DestScript::default()
    };
    let out = transfer(&pairs, people(two_people()), dest).await;
    let (side, message) = sided(&out.result);
    assert_eq!(side, ErrorSide::Target);
    assert!(message.contains("wrote 1 of 2"), "{message}");
    assert!(out.has("dst:execute DROP TABLE IF EXISTS shop.qh_new_"));
    assert!(!out.has("dst:execute RENAME TABLE"));
    assert!(out.events.iter().all(|event| event["event"] != "done"));
}

// --------------------------------------------------------------------------- //
// B-5: cancel before drop, and the destination as it was
// --------------------------------------------------------------------------- //

fn cancel_before_drop(out: &Outcome) {
    assert!(
        out.position("src:cancel") < out.position("src:drop-cursor"),
        "the cursor was dropped before the server was asked to cancel: {:#?}",
        out.log
    );
}

#[tokio::test]
async fn a_stop_cancels_the_source_before_dropping_its_cursor_and_writes_nothing() {
    let cancel = CancelFlag::new();
    let steps = vec![
        Step::Rows(vec![int_column(&[1, 2]), text_column(&["a", "b"])]),
        Step::Stop(cancel.clone()),
        Step::Rows(vec![int_column(&[3]), text_column(&["c"])]),
    ];
    let pairs = all(&TO_POSTGRES, &[]);
    let out = transfer_with(&pairs, people(steps), DestScript::default(), true, cancel).await;
    out.result.as_ref().expect("a Stop is a done");
    cancel_before_drop(&out);
    assert!(out.has("dst:copy-fail"), "the open copy is aborted");
    // On the destination too: the statement still running is cancelled, and only then is the
    // copy dropped (which is what sends `CopyFail`).
    assert!(
        out.position("dst:cancel") < out.position("dst:copy-fail"),
        "{:#?}",
        out.log
    );
    assert!(out.has("dst:execute ROLLBACK"));
    assert!(!out.has("dst:execute COMMIT"));
    let done = out.event("done");
    assert_eq!(done["cancelled"], true);
    assert_eq!(done["rows"], 0);
    assert_eq!(done["verified"], false);
    assert!(
        done["warnings"].to_string().contains("nothing was written"),
        "{done}"
    );

    // MySQL: the scratch table goes.
    let cancel = CancelFlag::new();
    let steps = vec![
        Step::Rows(vec![int_column(&[1, 2]), text_column(&["a", "b"])]),
        Step::Stop(cancel.clone()),
        Step::Rows(vec![int_column(&[3]), text_column(&["c"])]),
    ];
    let pairs = all(&TO_MYSQL, &[]);
    let out = transfer_with(&pairs, people(steps), DestScript::default(), true, cancel).await;
    out.result.as_ref().expect("a Stop is a done");
    cancel_before_drop(&out);
    assert!(out.has("dst:execute DROP TABLE IF EXISTS shop.qh_new_"));
    assert!(!out.has("dst:execute RENAME TABLE"));
}

#[tokio::test]
async fn a_stop_before_the_destination_writes_anything_leaves_it_alone() {
    let cancel = CancelFlag::new();
    cancel.request();
    let pairs = all(&TO_POSTGRES, &[]);
    let out = transfer_with(
        &pairs,
        people(two_people()),
        DestScript::default(),
        true,
        cancel,
    )
    .await;
    out.result.as_ref().expect("a done");
    assert_eq!(out.event("done")["cancelled"], true);
    assert!(!out.has("dst:execute BEGIN"));
    assert!(!out.has("dst:execute CREATE"));
}

#[tokio::test]
async fn a_destination_failure_mid_stream_cancels_the_source_first_and_rolls_back() {
    // The send of the first window fails while the source still has rows to give.
    let pairs = all(&TO_POSTGRES, &[("TRANSFER_COPY_ROWS", "2")]);
    let dest = DestScript {
        fail_send: true,
        ..DestScript::default()
    };
    let out = transfer(&pairs, people(five_people()), dest).await;
    let (side, _) = sided(&out.result);
    assert_eq!(side, ErrorSide::Target);
    cancel_before_drop(&out);
    assert!(out.has("dst:execute ROLLBACK"));
    assert!(!out.has("dst:execute COMMIT"));

    // MySQL: an INSERT refused by the server.
    let pairs = all(&TO_MYSQL, &[("TRANSFER_BATCH_ROWS", "2")]);
    let dest = DestScript {
        fail_on: vec!["INSERT INTO"],
        ..DestScript::default()
    };
    let out = transfer(&pairs, people(five_people()), dest).await;
    let (side, message) = sided(&out.result);
    assert_eq!(side, ErrorSide::Target);
    assert!(message.contains("refused"), "{message}");
    cancel_before_drop(&out);
    assert!(out.has("dst:execute DROP TABLE IF EXISTS shop.qh_new_"));
}

#[tokio::test]
async fn every_call_to_the_destination_carries_its_statement_bound_the_clean_up_included() {
    // The PostgreSQL driver sends `SET statement_timeout` ahead of a statement whenever the
    // bound changes, in the same flight. On an aborted transaction the server refuses that SET
    // and the statement behind it never goes out, so a ROLLBACK (or the MySQL scratch DROP)
    // that asked for a different bound than the run's would silently not run.
    let bound = Some(Duration::from_secs(60));
    let steps = || {
        vec![
            Step::Rows(vec![int_column(&[1, 2]), text_column(&["a", "b"])]),
            Step::Rows(vec![int_column(&[3]), text_column(&["c"])]),
        ]
    };
    #[allow(clippy::type_complexity)]
    let cases: Vec<(Vec<(&str, &str)>, DestScript, &str)> = vec![
        // PostgreSQL: the CREATE fails, then the transaction is rolled back.
        (
            all(&TO_POSTGRES, &[("DEST_STATEMENT_TIMEOUT_MS", "60000")]),
            DestScript {
                fail_on: vec!["CREATE TABLE"],
                ..DestScript::default()
            },
            "ROLLBACK",
        ),
        // PostgreSQL: a copy is open when the send fails.
        (
            all(
                &TO_POSTGRES,
                &[
                    ("DEST_STATEMENT_TIMEOUT_MS", "60000"),
                    ("TRANSFER_COPY_ROWS", "2"),
                ],
            ),
            DestScript {
                fail_send: true,
                ..DestScript::default()
            },
            "ROLLBACK",
        ),
        // MySQL: an INSERT fails, then the scratch table is dropped.
        (
            all(&TO_MYSQL, &[("DEST_STATEMENT_TIMEOUT_MS", "60000")]),
            DestScript {
                fail_on: vec!["INSERT INTO"],
                ..DestScript::default()
            },
            "DROP TABLE IF EXISTS",
        ),
    ];
    for (pairs, dest, cleanup) in cases {
        let out = transfer(&pairs, people(steps()), dest).await;
        assert!(out.result.is_err(), "{cleanup}");
        let bounds = out.rig.dest.bounds.lock().unwrap().clone();
        assert!(
            bounds.iter().any(|(sql, _)| sql.starts_with(cleanup)),
            "{cleanup} was never sent: {bounds:#?}"
        );
        for (sql, seen) in &bounds {
            assert_eq!(*seen, bound, "{sql} ran under another bound than the run's");
        }
        // And nothing is reported as left behind when the clean-up ran.
        let message = out.result.as_ref().unwrap_err().to_string();
        assert!(!message.contains("could not be rolled back"), "{message}");
        assert!(!out
            .result
            .as_ref()
            .unwrap_err()
            .warnings()
            .iter()
            .any(|w| w.contains("left behind")));
    }
}

#[tokio::test]
async fn a_failed_create_cancels_the_source_first_and_leaves_the_destination_untouched() {
    for pairs in [all(&TO_POSTGRES, &[]), all(&TO_MYSQL, &[])] {
        let dest = DestScript {
            fail_on: vec!["CREATE TABLE"],
            ..DestScript::default()
        };
        let out = transfer(&pairs, people(two_people()), dest).await;
        let (side, _) = sided(&out.result);
        assert_eq!(side, ErrorSide::Target);
        cancel_before_drop(&out);
        assert!(!out.has("dst:copy-in") && !out.has("dst:bound INSERT"));
        assert!(!out.has("dst:execute COMMIT") && !out.has("dst:execute RENAME"));
    }
}

#[tokio::test]
async fn a_value_the_encoder_refuses_stops_the_transfer_without_naming_the_value() {
    // The second row's id is a text: the driver gave up on parsing a number.
    let steps = vec![Step::Rows(vec![
        vec![Value::Int(1), Value::Text("secret-value-9".into())],
        text_column(&["a", "b"]),
    ])];
    let pairs = all(&TO_POSTGRES, &[]);
    let out = transfer(&pairs, people(steps), DestScript::default()).await;
    let (side, message) = sided(&out.result);
    assert_eq!(side, ErrorSide::Target);
    assert!(message.contains("column \"id\", source row 2"), "{message}");
    assert!(!message.contains("secret-value-9"), "{message}");
    cancel_before_drop(&out);
    assert!(out.has("dst:execute ROLLBACK"));
}

#[tokio::test]
async fn a_type_with_no_faithful_target_is_refused_after_the_first_batch_with_the_cursor_cancelled()
{
    let mut source = people(two_people());
    source.types = Some(vec![
        SqlType::of(qh_driver::Base::Int64),
        SqlType::of(qh_driver::Base::Other).with_native("geometry"),
    ]);
    let pairs = all(&TO_POSTGRES, &[]);
    let out = transfer(&pairs, source, DestScript::default()).await;
    let (side, message) = sided(&out.result);
    assert_eq!(side, ErrorSide::Target);
    assert!(
        message.contains("\"name\"") && message.contains("geometry"),
        "{message}"
    );
    cancel_before_drop(&out);
    assert!(
        !out.has("dst:execute BEGIN"),
        "nothing was started on the destination"
    );
    assert!(!out.has("dst:execute CREATE"));
}

#[tokio::test]
async fn a_source_that_fails_mid_stream_is_the_sources_error_and_the_destination_is_rolled_back() {
    let steps = vec![
        Step::Rows(vec![int_column(&[1, 2]), text_column(&["a", "b"])]),
        Step::Fail(EngineError::Query {
            message: "the source dropped the query".to_owned(),
            code: None,
            position: None,
            kind: FailureKind::Permanent,
        }),
    ];
    let pairs = all(&TO_POSTGRES, &[]);
    let out = transfer(&pairs, people(steps), DestScript::default()).await;
    let (side, message) = sided(&out.result);
    assert_eq!(side, ErrorSide::Source);
    assert!(message.contains("dropped the query"));
    assert!(out.has("dst:execute ROLLBACK"));
    assert!(!out.has("dst:execute COMMIT"));
}

#[tokio::test]
async fn a_source_without_columns_or_without_a_description_is_refused() {
    let mut source = people(Vec::new());
    source.columns = Vec::new();
    source.types = Some(Vec::new());
    let out = transfer(&all(&TO_POSTGRES, &[]), source, DestScript::default()).await;
    let (side, message) = sided(&out.result);
    assert_eq!(side, ErrorSide::Source);
    assert!(message.contains("no columns"), "{message}");

    let mut source = people(two_people());
    source.types = None;
    let out = transfer(&all(&TO_POSTGRES, &[]), source, DestScript::default()).await;
    let (side, message) = sided(&out.result);
    assert_eq!(side, ErrorSide::Target);
    assert!(
        message.contains("did not describe its column types"),
        "{message}"
    );
    cancel_before_drop(&out);
}

#[tokio::test]
async fn a_create_over_an_existing_table_fails_before_the_source_query_runs() {
    let dest = DestScript {
        exists: true,
        ..DestScript::default()
    };
    for pairs in [all(&TO_POSTGRES, &[]), all(&TO_MYSQL, &[])] {
        let out = transfer(&pairs, people(two_people()), dest_like(&dest)).await;
        let (side, message) = sided(&out.result);
        assert_eq!(side, ErrorSide::Target);
        assert!(message.contains("already exists"), "{message}");
        assert!(
            !out.has("src:connect"),
            "the expensive query was never started"
        );
        assert_eq!(out.steps(), ["connect_target"]);
    }
}

fn dest_like(script: &DestScript) -> DestScript {
    DestScript {
        exists: script.exists,
        ..DestScript::default()
    }
}

// --------------------------------------------------------------------------- //
// B-1 and the two connections' identities
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn a_dest_setting_never_reaches_the_source_and_back() {
    let pairs = all(&TO_POSTGRES, &[]);
    let out = transfer(&pairs, people(two_people()), DestScript::default()).await;
    out.result.as_ref().expect("ran");

    // The source connected with its own password and host, whatever the destination sent.
    let configs = out.rig.source_configs.lock().unwrap().clone();
    assert_eq!(configs[0].host, "source.invalid");
    assert_eq!(configs[0].password.as_deref(), Some("source-password"));

    // The destination engine was built from the destination's settings alone.
    let seen = out.rig.dest_settings.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    let dest = &seen[0];
    assert_eq!(dest.get("DB_HOST"), Some("target.invalid"));
    assert_eq!(dest.get("DB_PASSWORD"), Some("dest-password"));
    assert_eq!(dest.get("SAFE_MODE"), Some("full"));
    assert!(!dest.has_prefixed("DEST_"));
    assert_eq!(
        dest.get("DB_USER"),
        None,
        "the source's user is not inherited"
    );
    assert!(
        !format!("{dest:?}").contains("dest-password"),
        "redacted in Debug"
    );
}

#[tokio::test]
async fn two_safe_mode_floors_are_both_entries_in_either_order() {
    // confirm asks; a floor of no_ddl from either source forbids DDL whatever else says.
    for extra in [
        [
            ("DEST_SAFE_MODE_FLOOR", "confirm"),
            ("SAFE_MODE_FLOOR", "no_ddl"),
        ],
        [
            ("DEST_SAFE_MODE_FLOOR", "no_ddl"),
            ("SAFE_MODE_FLOOR", "confirm"),
        ],
    ] {
        let mut pairs = all(
            &TO_POSTGRES,
            &[
                ("DEST_SAFE_MODE", "confirm"),
                ("DEST_SAFE_MODE_CONFIRMED", "1"),
            ],
        );
        pairs.extend(extra);
        let out = transfer(&pairs, people(two_people()), DestScript::default()).await;
        assert!(
            matches!(out.result, Err(CliError::Usage(_))),
            "{extra:?}: {:?}",
            out.result
        );
        assert!(out.untouched(), "{extra:?}: {:#?}", out.log);
    }
}

// --------------------------------------------------------------------------- //
// B-2: the destination's Safe Mode decides, statement by statement
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn the_destinations_safe_mode_decides_create_and_replace_and_a_refusal_sends_nothing() {
    struct Case {
        label: &'static str,
        extra: Vec<(&'static str, &'static str)>,
        runs: bool,
    }
    let case = |label, extra: &[(&'static str, &'static str)], runs| Case {
        label,
        extra: extra.to_vec(),
        runs,
    };
    let create = ("WRITE_MODE", "create");
    let replace = ("WRITE_MODE", "replace");
    let cases = vec![
        case("full create", &[create], true),
        case("full replace", &[replace], true),
        case(
            "no_ddl create",
            &[create, ("DEST_SAFE_MODE", "no_ddl")],
            false,
        ),
        case(
            "no_ddl replace",
            &[replace, ("DEST_SAFE_MODE", "no_ddl")],
            false,
        ),
        case(
            "read_only create",
            &[create, ("DEST_SAFE_MODE", "read_only")],
            false,
        ),
        case(
            "read-only mark create",
            &[create, ("DEST_DB_READ_ONLY", "1")],
            false,
        ),
        case(
            "confirm create, unconfirmed",
            &[create, ("DEST_SAFE_MODE", "confirm")],
            false,
        ),
        case(
            "confirm create, confirmed",
            &[
                create,
                ("DEST_SAFE_MODE", "confirm"),
                ("DEST_SAFE_MODE_CONFIRMED", "1"),
            ],
            true,
        ),
        case(
            "confirm replace, confirmed",
            &[
                replace,
                ("DEST_SAFE_MODE", "confirm"),
                ("DEST_SAFE_MODE_CONFIRMED", "1"),
            ],
            false,
        ),
        case(
            "confirm create, confirmed, with a no_ddl floor",
            &[
                create,
                ("DEST_SAFE_MODE", "confirm"),
                ("DEST_SAFE_MODE_CONFIRMED", "1"),
                ("DEST_SAFE_MODE_FLOOR", "no_ddl"),
            ],
            false,
        ),
        case(
            "full create under a top-level confirm floor, confirmed",
            &[
                create,
                ("SAFE_MODE_FLOOR", "confirm"),
                ("DEST_SAFE_MODE_CONFIRMED", "1"),
            ],
            true,
        ),
        case(
            "full create under a top-level confirm floor, unconfirmed",
            &[create, ("SAFE_MODE_FLOOR", "confirm")],
            false,
        ),
        case(
            "full create under a top-level read_only floor",
            &[create, ("SAFE_MODE_FLOOR", "read_only")],
            false,
        ),
    ];
    for (target, base) in [("postgres", &TO_POSTGRES), ("mysql", &TO_MYSQL)] {
        for case in &cases {
            let pairs = all(base, &case.extra);
            let dest = DestScript {
                exists: case.extra.contains(&replace),
                ..DestScript::default()
            };
            let out = transfer(&pairs, people(two_people()), dest).await;
            let label = format!("{target}: {}", case.label);
            if case.runs {
                out.result
                    .as_ref()
                    .unwrap_or_else(|error| panic!("{label}: {error}"));
                if target == "postgres" {
                    // The engine's own transaction control reaches a destination that asks,
                    // which the classifier alone would refuse (it reads BEGIN as Unknown).
                    assert!(
                        out.has("dst:execute BEGIN") && out.has("dst:execute COMMIT"),
                        "{label}"
                    );
                }
            } else {
                let error = out.result.as_ref().expect_err(&label);
                assert!(matches!(error, CliError::Usage(_)), "{label}: {error:?}");
                // Nothing was opened, sent or announced.
                assert!(out.untouched(), "{label}: {:#?}", out.log);
            }
        }
    }
}

#[tokio::test]
async fn the_source_must_be_one_read_and_the_sources_safe_mode_still_applies() {
    let two = with(&SOURCE, &[("SQL", "SELECT 1; SELECT 2")]);
    let mut pairs = two.clone();
    pairs.extend(TO_POSTGRES);
    let out = transfer(&pairs, people(two_people()), DestScript::default()).await;
    assert!(
        matches!(&out.result, Err(CliError::Usage(m)) if m.contains("exactly one")),
        "{:?}",
        out.result
    );
    assert!(out.untouched());

    // A write is not read, in any Safe Mode: this reads the source and nothing else.
    for sql in [
        "INSERT INTO people VALUES (1)",
        "DELETE FROM people",
        "CREATE TABLE x (a int)",
    ] {
        let mut pairs = with(&SOURCE, &[("SQL", sql)]);
        pairs.extend(TO_POSTGRES);
        let out = transfer(&pairs, people(two_people()), DestScript::default()).await;
        assert!(
            matches!(out.result, Err(CliError::Usage(_))),
            "{sql}: {:?}",
            out.result
        );
        assert!(out.untouched(), "{sql}");
    }
    // The source's own Safe Mode, here read_only, still lets a read through.
    let mut pairs = with(&SOURCE, &[("SAFE_MODE", "read_only")]);
    pairs.extend(TO_POSTGRES);
    let out = transfer(&pairs, people(two_people()), DestScript::default()).await;
    out.result.as_ref().expect("a read is a read");
}

#[tokio::test]
async fn a_bad_connection_url_is_placed_on_its_side_and_never_echoes_its_password() {
    // The destination's URL has a password and no host.
    let mut pairs = SOURCE.to_vec();
    pairs.extend(with(
        &TO_POSTGRES,
        &[
            ("DEST_DB_URL", "postgres://alice:s3cret-pw@/db"),
            ("DEST_DB_HOST", ""),
        ],
    ));
    let out = transfer(&pairs, people(two_people()), DestScript::default()).await;
    let (side, message) = sided(&out.result);
    assert_eq!(side, ErrorSide::Target);
    assert!(!message.contains("s3cret-pw"), "{message}");
    assert!(message.contains("host"), "{message}");
    assert!(out.untouched());

    // The same on the source side.
    let mut pairs = with(
        &SOURCE,
        &[("DB_URL", "trino://bob:other-pw@/hive"), ("DB_HOST", "")],
    );
    pairs.extend(TO_POSTGRES);
    let out = transfer(&pairs, people(two_people()), DestScript::default()).await;
    let (side, message) = sided(&out.result);
    assert_eq!(side, ErrorSide::Source);
    assert!(!message.contains("other-pw"), "{message}");
    assert!(out.untouched());
}

#[tokio::test]
async fn trino_is_not_a_destination() {
    let mut pairs = SOURCE.to_vec();
    pairs.extend(with(&TO_POSTGRES, &[("DEST_DB_KIND", "trino")]));
    let out = transfer(&pairs, people(two_people()), DestScript::default()).await;
    assert!(
        matches!(&out.result, Err(CliError::Usage(m)) if m.contains("Trino")),
        "{:?}",
        out.result
    );
    assert!(out.untouched());
}

#[tokio::test]
async fn hostile_names_from_the_server_reach_the_destination_only_quoted() {
    let mut source = people(Vec::new());
    source.columns = vec![
        ColumnMeta::new("id\"); DROP TABLE people; --", "bigint"),
        ColumnMeta::new("select", "varchar"),
    ];
    source.steps = Mutex::new(vec![Step::Rows(vec![int_column(&[1]), text_column(&["a"])])].into());
    let pairs = all(&TO_POSTGRES, &[]);
    let out = transfer(&pairs, source, DestScript::default()).await;
    out.result.as_ref().expect("ran");
    let create = out.lines("dst:execute CREATE")[0];
    assert_eq!(
        create,
        "dst:execute CREATE TABLE public.people (\"id\"\"); DROP TABLE people; --\" bigint, \"select\" text)"
    );
    // Every statement the destination got is one statement.
    for line in out.lines("dst:execute") {
        assert!(!line.contains("\n"), "{line}");
    }
}

// --------------------------------------------------------------------------- //
// the execution log
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn each_written_statement_is_logged_once_by_the_hash_of_what_was_going_to_run() {
    use qh_storage::{hash_statement, Storage};
    let mut storage = Storage::in_memory().expect("in-memory database");
    storage.migrate_at(1_000).expect("migrations");
    qh_ffi::execution_log::install(storage);

    // A table and a source query no other test uses, so the rows below are this test's alone.
    let source = with(&SOURCE, &[("SQL", "SELECT id, name FROM logged_people")]);
    let base = with(
        &TO_POSTGRES,
        &[
            ("TARGET_TABLE", "logged_table"),
            ("DEST_SAFE_MODE", "confirm"),
            ("TRANSFER_COPY_ROWS", "2"),
        ],
    );
    let select = hash_statement("SELECT id, name FROM logged_people");
    let create = hash_statement("CREATE TABLE public.logged_table (id bigint, name text)");
    let copy = hash_statement("COPY public.logged_table (id, name) FROM STDIN");
    let subject = hash_statement("CREATE TABLE public.logged_table");

    // Asked, not confirmed: refused before a connection, logged with the plan's subject.
    let mut asked = source.clone();
    asked.extend(base.clone());
    let out = transfer(&asked, people(five_people()), DestScript::default()).await;
    assert!(matches!(out.result, Err(CliError::Usage(_))));
    assert!(out.untouched());

    // Confirmed: three COPY windows of one statement text, logged once; the engine's own
    // BEGIN and COMMIT are not decisions and are not in the log.
    let mut pairs = source.clone();
    pairs.extend(with(&base, &[("DEST_SAFE_MODE_CONFIRMED", "1")]));
    let out = transfer(&pairs, people(five_people()), DestScript::default()).await;
    out.result.as_ref().expect("ran once confirmed");
    assert_eq!(out.count("dst:copy-in"), 3);

    qh_ffi::execution_log::with_storage(|storage| {
        assert!(storage.verify_execution_log().unwrap() >= 3);
        let rows = storage.execution_log(1000).unwrap();
        let decisions = |hash: &str| -> Vec<String> {
            rows.iter()
                .filter(|row| row.statement_hash == hash)
                .map(|row| row.decision.clone())
                .collect()
        };
        assert_eq!(decisions(&subject), ["needs_confirmation"]);
        // The source's SELECT was allowed once, by the run that ran: the refused run, which
        // the destination turned away before anything started, left no row for it.
        assert_eq!(decisions(&select), ["allowed"]);
        assert_eq!(decisions(&create), ["confirmed"]);
        assert_eq!(decisions(&copy), ["confirmed"], "one row for three windows");
        for engine_statement in ["BEGIN", "COMMIT", "ROLLBACK"] {
            assert!(decisions(&hash_statement(engine_statement)).is_empty());
        }
        // The log holds hashes and never statements.
        assert!(rows
            .iter()
            .all(|row| !row.statement_hash.contains("logged_table")));
    })
    .expect("a sink is installed");
    qh_ffi::execution_log::uninstall();
}
