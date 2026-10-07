//! `to_table` with `WRITE_MODE=replace` never leaves the user without a table (PF-1).
//!
//! The old table used to be dropped first and the new one created after, so a SELECT that
//! failed, or a Stop in the middle of the build, cost the user the table they had. The
//! command now builds the new table under a temporary name and swaps it in, and drops the
//! old one last.
//!
//! Two halves, because they prove different things:
//!
//! * The recorder tests need no database. They hold the statements the engine sends against
//!   the order the claim needs (nothing touches the target before the build is complete),
//!   for each driver, and drive the failure paths a live server cannot be made to take on
//!   demand: a swap that stops half-way, an undo that fails, a drop that fails.
//! * The live tests run the same claims against a real server: a failing SELECT and a Stop
//!   in the middle of the build must leave the original rows unchanged, and nothing
//!   temporary behind.
//!
//! ```bash
//! QH_TEST_POSTGRES=1 QH_TEST_MYSQL=1 QH_TEST_TRINO=1 \
//!   cargo test -p qh-ffi --test to_table_replace -- --test-threads=1
//! ```
//!
//! Without a flag a live test prints why it is skipping and returns, for the reason
//! `real_server.rs` gives: a green run that tested nothing is worse than a visible skip.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use qh_core::{ColumnBatch, ColumnMeta, EngineError, FailureKind, Value};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions,
    ObjectPath, ObjectsPage, Session,
};
use qh_ffi::events::Capture;
use qh_ffi::{run, CancelFlag, CliError, Command, Engine, RealEngine, Settings};
use serde_json::Value as Json;
use tokio::sync::Notify;

// --------------------------------------------------------------------------- //
// the recorder
// --------------------------------------------------------------------------- //

/// What the fake server does, and what it was asked.
#[derive(Clone, Default)]
struct Server {
    statements: Arc<Mutex<Vec<String>>>,
    /// Whether the target table exists, as the probe reports it.
    exists: bool,
    /// A statement containing any of these fails.
    fail: Vec<&'static str>,
    /// The build never ends on its own: it ends when the session is asked to cancel.
    hang: bool,
    /// Where it hangs: inside `execute`, as MySQL does for a statement with no result set,
    /// or while the cursor is read.
    hang_in_execute: bool,
    released: Arc<Notify>,
}

impl Server {
    fn log(&self) -> Vec<String> {
        self.statements.lock().expect("log").clone()
    }
}

struct FakeEngine {
    kind: DriverKind,
    server: Server,
}

#[async_trait]
impl Engine for FakeEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        vec![self.kind]
    }

    fn driver(&self, _kind: DriverKind) -> &dyn Driver {
        match self.kind {
            DriverKind::Postgres => &FakeDriver(DriverKind::Postgres),
            DriverKind::Mysql => &FakeDriver(DriverKind::Mysql),
            DriverKind::Trino => &FakeDriver(DriverKind::Trino),
        }
    }

    async fn connect(&self, _config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        Ok(Box::new(FakeSession {
            server: self.server.clone(),
        }))
    }
}

struct FakeDriver(DriverKind);

#[async_trait]
impl Driver for FakeDriver {
    fn kind(&self) -> DriverKind {
        self.0
    }

    fn label(&self) -> &'static str {
        "Recording"
    }

    fn default_port(&self) -> u16 {
        1
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
            message: "never asked to connect through here".to_owned(),
            kind: FailureKind::Permanent,
        })
    }
}

struct FakeSession {
    server: Server,
}

#[async_trait]
impl Session for FakeSession {
    fn capabilities(&self) -> Capabilities {
        FakeDriver(DriverKind::Postgres).capabilities()
    }

    fn query_id(&self) -> Option<String> {
        None
    }

    async fn execute(
        &mut self,
        sql: &str,
        _options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        self.server
            .statements
            .lock()
            .expect("log")
            .push(sql.to_owned());
        if self.server.fail.iter().any(|needle| sql.contains(needle)) {
            return Err(EngineError::Query {
                message: format!("the server refused: {sql}"),
                code: None,
                position: None,
                kind: FailureKind::Permanent,
            });
        }
        let probe = sql.contains("information_schema");
        let build = self.server.hang && sql.starts_with("CREATE TABLE") && sql.contains("qh_new_");
        if build && self.server.hang_in_execute {
            self.server.released.notified().await;
            return Err(EngineError::Query {
                message: "Query execution was interrupted".to_owned(),
                code: None,
                position: None,
                kind: FailureKind::Permanent,
            });
        }
        Ok(Box::new(FakeCursor {
            answer: probe.then_some(i64::from(self.server.exists)),
            hang: build,
            released: self.server.released.clone(),
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
        self.server
            .statements
            .lock()
            .expect("log")
            .push("<cancel>".to_owned());
        self.server.released.notify_one();
        Ok(())
    }

    async fn close(self: Box<Self>) -> Result<(), EngineError> {
        Ok(())
    }
}

struct FakeCursor {
    /// The count a probe answers with; `None` for every other statement.
    answer: Option<i64>,
    hang: bool,
    released: Arc<Notify>,
}

#[async_trait]
impl Cursor for FakeCursor {
    fn columns(&self) -> &[ColumnMeta] {
        &[]
    }

    async fn next_batch(&mut self, _max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        if self.hang {
            // A build that only a cancel ends, and that then fails the way a cancelled
            // statement does.
            self.released.notified().await;
            return Err(EngineError::Query {
                message: "canceling statement due to user request".to_owned(),
                code: None,
                position: None,
                kind: FailureKind::Permanent,
            });
        }
        match self.answer.take() {
            Some(count) => Ok(Some(
                ColumnBatch::new(vec![vec![Value::Int(count)]]).expect("one column, one row"),
            )),
            None => Ok(None),
        }
    }

    fn affected_rows(&self) -> Option<u64> {
        None
    }
}

fn pairs_of(kind: DriverKind, mode: &str) -> Vec<(&'static str, String)> {
    let mut pairs = vec![
        ("DB_HOST", "db.invalid".to_owned()),
        ("DB_USER", "queryhive".to_owned()),
        ("RETRIES", "0".to_owned()),
        ("SQL", "SELECT 1 AS id".to_owned()),
        ("TARGET_TABLE", "people".to_owned()),
        ("WRITE_MODE", mode.to_owned()),
    ];
    match kind {
        DriverKind::Postgres => {
            pairs.push(("DB_KIND", "postgres".to_owned()));
            pairs.push(("TARGET_SCHEMA", "public".to_owned()));
        }
        DriverKind::Mysql => {
            pairs.push(("DB_KIND", "mysql".to_owned()));
            pairs.push(("TARGET_CATALOG", "shop".to_owned()));
        }
        DriverKind::Trino => {
            pairs.push(("DB_KIND", "trino".to_owned()));
            pairs.push(("TARGET_CATALOG", "hive".to_owned()));
            pairs.push(("TARGET_SCHEMA", "default".to_owned()));
        }
    }
    pairs
}

/// Run a `replace` against the fake and return what it answered with and what it was sent.
async fn replace(
    kind: DriverKind,
    server: &Server,
    cancel: &CancelFlag,
) -> (Result<Capture, CliError>, Vec<String>) {
    let settings = Settings::from_pairs(pairs_of(kind, "replace"));
    let mut out = Capture::new();
    let engine = FakeEngine {
        kind,
        server: server.clone(),
    };
    let result = run(Command::ToTable, &settings, &mut out, &engine, cancel)
        .await
        .map(|()| out);
    (result, server.log())
}

fn warnings_of(result: &Result<Capture, CliError>) -> Vec<String> {
    match result {
        Ok(out) => {
            let done = out.lines.last().expect("a done event");
            done["warnings"]
                .as_array()
                .expect("warnings")
                .iter()
                .map(|warning| warning.as_str().unwrap_or_default().to_owned())
                .collect()
        }
        Err(CliError::Warned { warnings, .. }) => warnings.clone(),
        Err(other) => panic!("an error without warnings: {other:?}"),
    }
}

/// The index of the first statement starting with `prefix`.
fn at(log: &[String], prefix: &str) -> usize {
    log.iter()
        .position(|statement| statement.starts_with(prefix))
        .unwrap_or_else(|| panic!("no statement starts with `{prefix}`: {log:#?}"))
}

fn count(log: &[String], prefix: &str) -> usize {
    log.iter()
        .filter(|statement| statement.starts_with(prefix))
        .count()
}

#[tokio::test]
async fn postgres_builds_first_then_swaps_in_one_transaction() {
    let server = Server {
        exists: true,
        ..Server::default()
    };
    let (result, log) = replace(DriverKind::Postgres, &server, &CancelFlag::new()).await;
    let out = result.expect("the replace works");

    let build = at(&log, "CREATE TABLE \"public\".\"qh_new_");
    let begin = at(&log, "BEGIN");
    let drop = at(&log, "DROP TABLE IF EXISTS \"public\".\"people\"");
    let rename = at(&log, "ALTER TABLE \"public\".\"qh_new_");
    let commit = at(&log, "COMMIT");
    assert!(
        build < begin && begin < drop && drop < rename && rename < commit,
        "the old table is dropped only inside the swap transaction: {log:#?}"
    );
    assert!(log[rename].ends_with("RENAME TO \"people\""), "{log:#?}");
    assert_eq!(count(&log, "ROLLBACK"), 0, "{log:#?}");
    assert_eq!(
        warnings_of(&Ok(out)),
        vec!["replaced the existing table public.people".to_owned()]
    );
}

#[tokio::test]
async fn postgres_without_an_old_table_says_nothing_was_replaced() {
    let (result, log) = replace(DriverKind::Postgres, &Server::default(), &CancelFlag::new()).await;
    assert!(
        warnings_of(&result).is_empty(),
        "{:?}",
        warnings_of(&result)
    );
    assert_eq!(count(&log, "COMMIT"), 1, "{log:#?}");
}

#[tokio::test]
async fn a_build_that_fails_never_touches_the_old_table() {
    for kind in [DriverKind::Postgres, DriverKind::Mysql, DriverKind::Trino] {
        let server = Server {
            exists: true,
            fail: vec!["CREATE TABLE"],
            ..Server::default()
        };
        let (result, log) = replace(kind, &server, &CancelFlag::new()).await;
        let warnings = warnings_of(&result);
        assert!(result.is_err(), "{kind:?}");
        let table = match kind {
            DriverKind::Postgres => "public.people",
            DriverKind::Mysql => "shop.people",
            DriverKind::Trino => "hive.default.people",
        };
        assert_eq!(
            warnings,
            vec![format!("the existing table {table} was left as it was")],
            "{kind:?}"
        );
        assert_eq!(
            log.len(),
            2,
            "{kind:?}: the build, then its clean-up: {log:#?}"
        );
        assert!(log[1].starts_with("DROP TABLE IF EXISTS"), "{log:#?}");
        assert!(log[1].contains("qh_new_"), "{log:#?}");
    }
}

#[tokio::test]
async fn mysql_swaps_with_one_atomic_rename_and_drops_the_old_table_last() {
    let server = Server {
        exists: true,
        ..Server::default()
    };
    let (result, log) = replace(DriverKind::Mysql, &server, &CancelFlag::new()).await;
    result.expect("the replace works");

    let build = at(&log, "CREATE TABLE `shop`.`qh_new_");
    let rename = at(&log, "RENAME TABLE `shop`.`people` TO `shop`.`qh_old_");
    let drop = at(&log, "DROP TABLE `shop`.`qh_old_");
    assert!(build < rename && rename < drop, "{log:#?}");
    // One statement moves the old table aside and the new one in, so there is no moment
    // at which `people` does not exist.
    assert!(log[rename].contains(", `shop`.`qh_new_"), "{log:#?}");
    assert!(log[rename].ends_with("TO `shop`.`people`"), "{log:#?}");
    assert_eq!(count(&log, "DROP TABLE IF EXISTS"), 0, "{log:#?}");
}

#[tokio::test]
async fn mysql_without_an_old_table_only_renames_the_new_one_in() {
    let (result, log) = replace(DriverKind::Mysql, &Server::default(), &CancelFlag::new()).await;
    result.expect("the replace works");
    assert!(
        log.iter().any(
            |statement| statement.starts_with("RENAME TABLE `shop`.`qh_new_")
                && statement.ends_with("TO `shop`.`people`")
        ),
        "{log:#?}"
    );
    assert_eq!(count(&log, "DROP TABLE"), 0, "{log:#?}");
}

#[tokio::test]
async fn a_failed_drop_of_the_old_table_is_a_warning_that_names_it() {
    let server = Server {
        exists: true,
        fail: vec!["DROP TABLE `shop`.`qh_old_"],
        ..Server::default()
    };
    let (result, _) = replace(DriverKind::Mysql, &server, &CancelFlag::new()).await;
    let warnings = warnings_of(&result);
    result.expect("the new table is in place, so the run succeeded");
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("the old one is kept as shop.qh_old_")),
        "{warnings:?}"
    );
}

#[tokio::test]
async fn trino_renames_the_old_table_aside_and_puts_it_back_when_the_second_rename_fails() {
    let server = Server {
        exists: true,
        // Only the rename that moves the new table in, not the one that moves the old
        // table aside or back.
        fail: vec!["ALTER TABLE \"hive\".\"default\".\"qh_new_"],
        ..Server::default()
    };
    let (result, log) = replace(DriverKind::Trino, &server, &CancelFlag::new()).await;
    assert!(result.is_err());
    let aside = at(
        &log,
        "ALTER TABLE \"hive\".\"default\".\"people\" RENAME TO",
    );
    let back = at(&log, "ALTER TABLE \"hive\".\"default\".\"qh_old_");
    let discard = at(&log, "DROP TABLE IF EXISTS \"hive\".\"default\".\"qh_new_");
    assert!(aside < back && back < discard, "{log:#?}");
    assert!(
        log[back].ends_with("RENAME TO \"hive\".\"default\".\"people\""),
        "{log:#?}"
    );
    assert_eq!(
        warnings_of(&result),
        vec!["the existing table hive.default.people was left as it was".to_owned()]
    );
}

#[tokio::test]
async fn a_swap_that_cannot_be_undone_names_both_tables_and_keeps_the_new_rows() {
    let server = Server {
        exists: true,
        fail: vec![
            "ALTER TABLE \"hive\".\"default\".\"qh_new_",
            "ALTER TABLE \"hive\".\"default\".\"qh_old_",
        ],
        ..Server::default()
    };
    let (result, log) = replace(DriverKind::Trino, &server, &CancelFlag::new()).await;
    assert!(result.is_err());
    // The new rows are the only copy that is complete: they are not dropped.
    assert_eq!(count(&log, "DROP TABLE"), 0, "{log:#?}");
    let warnings = warnings_of(&result);
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("hive.default.qh_old_")
                && warning.contains("hive.default.qh_new_")
                && warning.contains("could not be renamed back")),
        "{warnings:?}"
    );
}

#[tokio::test]
async fn a_stop_in_the_middle_of_the_build_cancels_it_and_leaves_the_old_table() {
    for kind in [DriverKind::Postgres, DriverKind::Mysql, DriverKind::Trino] {
        for hang_in_execute in [false, true] {
            let server = Server {
                exists: true,
                hang: true,
                hang_in_execute,
                ..Server::default()
            };
            let cancel = CancelFlag::new();
            let stop = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                stop.request();
            });
            let started = Instant::now();
            let (result, log) = replace(kind, &server, &cancel).await;
            let case = format!("{kind:?}, hang in execute: {hang_in_execute}");
            let out = result.expect("a Stop is an answer, not an error");
            assert!(started.elapsed() < Duration::from_secs(5), "{case}");

            let done = out.lines.last().expect("done");
            assert_eq!(done["cancelled"], true, "{case}: {done}");
            assert_eq!(log[1], "<cancel>", "{case}: {log:#?}");
            assert!(
                log[2].starts_with("DROP TABLE IF EXISTS"),
                "{case}: {log:#?}"
            );
            assert_eq!(log.len(), 3, "{case}: nothing touched the target: {log:#?}");
            let warnings = warnings_of(&Ok(out));
            assert!(
                warnings
                    .iter()
                    .any(|warning| warning.contains("was left as it was")),
                "{case}: {warnings:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_collision_with_the_target_is_an_error_and_not_an_overwrite() {
    // The probe said there was no table, and a table appeared before the rename: the
    // server refuses the name, and the run ends with the scratch table dropped.
    let server = Server {
        fail: vec!["RENAME TABLE"],
        ..Server::default()
    };
    let (result, log) = replace(DriverKind::Mysql, &server, &CancelFlag::new()).await;
    assert!(result.is_err());
    assert_eq!(
        count(&log, "DROP TABLE IF EXISTS `shop`.`qh_new_"),
        1,
        "{log:#?}"
    );
    assert_eq!(count(&log, "DROP TABLE `"), 0, "{log:#?}");
}

#[tokio::test]
async fn create_mode_is_still_one_statement() {
    let settings = Settings::from_pairs(pairs_of(DriverKind::Postgres, "create"));
    let server = Server::default();
    let engine = FakeEngine {
        kind: DriverKind::Postgres,
        server: server.clone(),
    };
    let mut out = Capture::new();
    run(
        Command::ToTable,
        &settings,
        &mut out,
        &engine,
        &CancelFlag::new(),
    )
    .await
    .expect("create works");
    assert_eq!(
        server.log(),
        vec!["CREATE TABLE \"public\".\"people\" AS SELECT 1 AS id".to_owned()]
    );
}

// --------------------------------------------------------------------------- //
// a real server
// --------------------------------------------------------------------------- //

/// One live server the claims are proved against.
struct Live {
    flag: &'static str,
    env: &'static [(&'static str, &'static str)],
    /// The settings that name a table in this server's own levels.
    target: &'static [(&'static str, &'static str)],
    /// The statement that lists the tables left behind by a run, as one text column.
    leftovers: &'static str,
    /// A SELECT of one `id` column that runs for much longer than a second.
    slow_select: &'static str,
}

const POSTGRES: Live = Live {
    flag: "QH_TEST_POSTGRES",
    env: &[
        ("DB_KIND", "postgres"),
        ("DB_HOST", "127.0.0.1"),
        ("DB_PORT", "55432"),
        ("DB_USER", "qh"),
        ("DB_PASSWORD", "qh-dev-only"),
        ("DB_DATABASE", "qh"),
        ("DB_SCHEMA", "public"),
        ("DB_SSLMODE", "disable"),
        ("RETRIES", "0"),
    ],
    target: &[("TARGET_SCHEMA", "public")],
    leftovers: "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public' \
                AND substr(table_name, 1, 7) IN ('qh_new_', 'qh_old_')",
    slow_select:
        "SELECT g AS id FROM generate_series(1, 4000) AS g WHERE pg_sleep(0.005) IS NOT NULL",
};

const MYSQL: Live = Live {
    flag: "QH_TEST_MYSQL",
    env: &[
        ("DB_KIND", "mysql"),
        ("DB_HOST", "127.0.0.1"),
        ("DB_PORT", "53306"),
        ("DB_USER", "qh"),
        ("DB_PASSWORD", "qh-dev-only"),
        ("DB_DATABASE", "qh"),
        ("RETRIES", "0"),
    ],
    target: &[("TARGET_CATALOG", "qh")],
    leftovers: "SELECT table_name FROM information_schema.tables WHERE table_schema = 'qh' \
                AND substr(table_name, 1, 7) IN ('qh_new_', 'qh_old_')",
    slow_select:
        "WITH RECURSIVE n AS (SELECT 1 AS id UNION ALL SELECT id + 1 FROM n WHERE id < 1000) \
                  SELECT id FROM n WHERE SLEEP(0.01) = 0",
};

const TRINO: Live = Live {
    flag: "QH_TEST_TRINO",
    env: &[
        ("DB_KIND", "trino"),
        ("DB_HOST", "127.0.0.1"),
        ("DB_PORT", "58080"),
        ("DB_USER", "queryhive"),
        ("DB_DATABASE", "memory"),
        ("DB_SCHEMA", "default"),
        ("RETRIES", "0"),
    ],
    target: &[("TARGET_CATALOG", "memory"), ("TARGET_SCHEMA", "default")],
    leftovers: "SELECT table_name FROM memory.information_schema.tables \
                WHERE table_schema = 'default' \
                AND substr(table_name, 1, 7) IN ('qh_new_', 'qh_old_')",
    slow_select: "SELECT CAST(orderkey AS integer) AS id FROM tpch.sf1.lineitem",
};

impl Live {
    fn enabled(&self) -> bool {
        if std::env::var(self.flag).as_deref() == Ok("1") {
            return true;
        }
        eprintln!("skipped: set {}=1 with deploy/dev/up.sh running", self.flag);
        false
    }

    fn settings(&self, extra: &[(&str, &str)]) -> Settings {
        Settings::from_pairs(
            self.env
                .iter()
                .chain(self.target)
                .chain(extra)
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect::<Vec<_>>(),
        )
    }

    /// Run one statement and return the first column of every row it produced.
    async fn sql(&self, statement: &str) -> Vec<String> {
        let mut out = Capture::new();
        run(
            Command::Preview,
            &self.settings(&[("SQL", statement)]),
            &mut out,
            &RealEngine::new(),
            &CancelFlag::new(),
        )
        .await
        .unwrap_or_else(|error| panic!("`{statement}` failed: {}", error.message()));
        out.lines
            .iter()
            .filter(|event| event["event"] == "rows")
            .flat_map(|event| event["data"].as_array().cloned().unwrap_or_default())
            .map(|row| match &row[0] {
                Json::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect()
    }

    async fn to_table(
        &self,
        table: &str,
        select: &str,
        cancel: &CancelFlag,
    ) -> Result<Capture, CliError> {
        let mut out = Capture::new();
        run(
            Command::ToTable,
            &self.settings(&[
                ("SQL", select),
                ("TARGET_TABLE", table),
                ("WRITE_MODE", "replace"),
            ]),
            &mut out,
            &RealEngine::new(),
            cancel,
        )
        .await
        .map(|()| out)
    }

    /// The table with two rows, `1` and `2`.
    async fn original(&self, table: &str) {
        self.sql(&format!("DROP TABLE IF EXISTS {table}")).await;
        self.sql(&format!("CREATE TABLE {table} (id integer)"))
            .await;
        self.sql(&format!("INSERT INTO {table} VALUES (1), (2)"))
            .await;
    }

    async fn ids(&self, table: &str) -> Vec<String> {
        let mut ids = self.sql(&format!("SELECT id FROM {table}")).await;
        ids.sort();
        ids
    }

    async fn assert_nothing_left_behind(&self) {
        assert_eq!(
            self.sql(self.leftovers).await,
            Vec::<String>::new(),
            "a temporary table survived"
        );
    }
}

/// Drops the test's table, and any temporary one a broken run left, when the test ends
/// however it ends. On its own thread and runtime, because `Drop` cannot await.
struct Cleanup {
    live: &'static Live,
    table: &'static str,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let (live, table) = (self.live, self.table);
        let _ = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async {
                for name in live.sql(live.leftovers).await {
                    live.sql(&format!("DROP TABLE IF EXISTS {name}")).await;
                }
                live.sql(&format!("DROP TABLE IF EXISTS {table}")).await;
            });
        })
        .join();
    }
}

async fn replaces_an_existing_table(live: &'static Live, table: &'static str) {
    if !live.enabled() {
        return;
    }
    let _cleanup = Cleanup { live, table };
    live.original(table).await;

    let out = live
        .to_table(table, "SELECT 7 AS id", &CancelFlag::new())
        .await
        .unwrap_or_else(|error| panic!("the replace failed: {}", error.message()));
    let done = out.lines.last().expect("done");
    assert_eq!(done["mode"], "replace", "{done}");
    assert_eq!(done["cancelled"], false, "{done}");
    assert!(
        done["warnings"][0]
            .as_str()
            .is_some_and(|text| text.starts_with("replaced the existing table")),
        "{done}"
    );
    assert_eq!(live.ids(table).await, vec!["7"]);
    live.assert_nothing_left_behind().await;
}

async fn creates_a_table_that_was_not_there(live: &'static Live, table: &'static str) {
    if !live.enabled() {
        return;
    }
    let _cleanup = Cleanup { live, table };
    live.sql(&format!("DROP TABLE IF EXISTS {table}")).await;

    let out = live
        .to_table(table, "SELECT 5 AS id", &CancelFlag::new())
        .await
        .unwrap_or_else(|error| panic!("the replace failed: {}", error.message()));
    let done = out.lines.last().expect("done");
    assert_eq!(done["warnings"], serde_json::json!([]), "{done}");
    assert_eq!(live.ids(table).await, vec!["5"]);
    live.assert_nothing_left_behind().await;
}

async fn a_failing_select_leaves_the_original_rows(live: &'static Live, table: &'static str) {
    if !live.enabled() {
        return;
    }
    let _cleanup = Cleanup { live, table };
    live.original(table).await;

    let error = live
        .to_table(
            table,
            "SELECT id FROM qh_no_such_table_for_to_table",
            &CancelFlag::new(),
        )
        .await
        .expect_err("the SELECT fails");
    let CliError::Warned { warnings, .. } = &error else {
        panic!("the error carries what became of the table: {error:?}");
    };
    assert!(
        warnings
            .iter()
            .any(|warning| warning.contains("was left as it was")),
        "{warnings:?}"
    );
    assert_eq!(live.ids(table).await, vec!["1", "2"], "the original rows");
    live.assert_nothing_left_behind().await;
}

async fn a_stop_in_the_middle_of_the_build_leaves_the_original_rows(
    live: &'static Live,
    table: &'static str,
) {
    if !live.enabled() {
        return;
    }
    let _cleanup = Cleanup { live, table };
    live.original(table).await;

    let cancel = CancelFlag::new();
    let stop = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        stop.request();
    });
    let started = Instant::now();
    let out = live
        .to_table(table, live.slow_select, &cancel)
        .await
        .unwrap_or_else(|error| panic!("a Stop is an answer: {}", error.message()));
    let took = started.elapsed();
    let done = out.lines.last().expect("done");
    assert_eq!(done["cancelled"], true, "{done}");
    assert!(took < Duration::from_secs(8), "the Stop took {took:?}");
    assert_eq!(live.ids(table).await, vec!["1", "2"], "the original rows");
    live.assert_nothing_left_behind().await;
}

async fn all_four(live: &'static Live, prefix: &'static str) {
    // One test per server, in order, so the four claims share one set of tables and two
    // servers never wait on each other's DDL.
    let tables: [&'static str; 4] = match prefix {
        "pg" => ["qh_tt_pg_a", "qh_tt_pg_b", "qh_tt_pg_c", "qh_tt_pg_d"],
        "my" => ["qh_tt_my_a", "qh_tt_my_b", "qh_tt_my_c", "qh_tt_my_d"],
        _ => ["qh_tt_tr_a", "qh_tt_tr_b", "qh_tt_tr_c", "qh_tt_tr_d"],
    };
    replaces_an_existing_table(live, tables[0]).await;
    creates_a_table_that_was_not_there(live, tables[1]).await;
    a_failing_select_leaves_the_original_rows(live, tables[2]).await;
    a_stop_in_the_middle_of_the_build_leaves_the_original_rows(live, tables[3]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_postgres_replace_never_leaves_the_user_without_a_table() {
    all_four(&POSTGRES, "pg").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_mysql_replace_never_leaves_the_user_without_a_table() {
    all_four(&MYSQL, "my").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_trino_replace_never_leaves_the_user_without_a_table() {
    all_four(&TRINO, "tr").await;
}
