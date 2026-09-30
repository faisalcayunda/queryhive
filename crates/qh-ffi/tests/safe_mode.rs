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

/// The MySQL Safe Mode bypass W3-T0 found: `SELECT '\''; DELETE …` is one read-only
/// SELECT under a generic scan, so a `read_only` MySQL connection classified it as safe and
/// the server — which always has multi-statements enabled — ran the DELETE. The guard must
/// read a MySQL connection under MySQL's rules and refuse the write before connecting.
///
/// On the code before the fix this test fails: the payload reached `connect`
/// (`connects() == 1`) instead of being refused. After the fix it is a `read-only` usage
/// refusal the server never saw.
#[tokio::test]
async fn read_only_refuses_a_mysql_string_escape_injection_before_connecting() {
    let payloads = [
        "SELECT '\\''; DELETE FROM t; -- '",
        "SELECT '\\''; DELETE FROM t; # '",
        "SELECT \"\\\"\"; DELETE FROM t; # \"",
        // The no-doubling spellings, where the server's `sql_mode` (`NO_BACKSLASH_ESCAPES`
        // or `ANSI_QUOTES`) makes the string close earlier than the MySQL reading thinks,
        // so the generic reading sees the write the MySQL reading misses.
        "SELECT '\\'; DELETE FROM t; -- '",
        "SELECT 1 \"\\\" ; DELETE FROM t ; -- \"",
    ];
    for payload in payloads {
        let engine = CountingEngine::new();
        let error = refuse(
            Command::Preview,
            &engine,
            &[
                ("DB_KIND", "mysql"),
                ("DB_HOST", "mysql.invalid"),
                ("DB_PORT", "3306"),
                ("DB_USER", "queryhive"),
                ("SAFE_MODE", "read_only"),
                ("SQL", payload),
            ],
        )
        .await;
        let message = usage_message(&error);
        assert!(message.contains("read-only"), "{payload:?}: {message}");
        assert_eq!(
            engine.connects(),
            0,
            "the write must be refused before the server is reached: {payload:?}"
        );
    }
}

/// MySQL has no dollar quoting: `$` is an identifier character, so the `;` and the write
/// between two `$tag$` markers are code the server runs. The guard must read them as code on
/// a MySQL connection, in every command that takes SQL, and refuse before connecting.
#[tokio::test]
async fn read_only_refuses_a_mysql_dollar_tag_hiding_a_write_before_connecting() {
    let payloads = [
        "SELECT 1 AS x$a$ ; DELETE FROM t ; $a$",
        "SELECT $a$ ; DELETE FROM t ; $a$",
        "SELECT $$; DROP TABLE t; $$",
        "SELECT $x$ DELETE FROM t $x$",
        "SELECT 1 $tag$; INSERT INTO t VALUES (1); $tag$",
    ];
    let mysql = |sql: &'static str, safe: &'static str| {
        vec![
            ("DB_KIND", "mysql"),
            ("DB_HOST", "mysql.invalid"),
            ("DB_PORT", "3306"),
            ("DB_USER", "queryhive"),
            ("SAFE_MODE", safe),
            ("SQL", sql),
            ("FORMAT", "csv"),
            ("OUT_DIR", "/tmp"),
        ]
    };
    for payload in payloads {
        for command in [
            Command::Preview,
            Command::Count,
            Command::Explain,
            Command::Export,
        ] {
            let engine = CountingEngine::new();
            let error = refuse(command, &engine, &mysql(payload, "read_only")).await;
            let message = usage_message(&error);
            assert!(
                message.contains("read-only") || message.contains("could not tell"),
                "{command:?} {payload:?}: {message}"
            );
            assert_eq!(
                engine.connects(),
                0,
                "{command:?}: the write must be refused before the server is reached: {payload:?}"
            );
        }
    }
}

/// The dollar-tag text is one dollar-quoted string on PostgreSQL, so its guard reads it as a
/// single read and it reaches the connect step. Trino has no dollar quotes (a `$` is a syntax
/// error there), so its reading takes what follows as code and refuses the write: this test used
/// to say Trino read it as one string, which was the generic reading's claim and not Trino's.
#[tokio::test]
async fn a_dollar_quoted_string_is_one_read_on_postgres_and_code_on_trino() {
    let engine = CountingEngine::new();
    let error = refuse(
        Command::Preview,
        &engine,
        &[
            ("DB_KIND", "postgres"),
            ("SAFE_MODE", "read_only"),
            ("SQL", "SELECT $a$ ; DELETE FROM t ; $a$"),
        ],
    )
    .await;
    assert!(
        !matches!(error, CliError::Usage(ref message) if message.contains("read-only")),
        "postgres: {error:?}"
    );
    assert_eq!(engine.connects(), 1, "postgres reaches the connect step");

    let engine = CountingEngine::new();
    let error = refuse(
        Command::Preview,
        &engine,
        &[
            ("DB_KIND", "trino"),
            ("SAFE_MODE", "read_only"),
            ("SQL", "SELECT $a$ ; DELETE FROM t ; $a$"),
        ],
    )
    .await;
    assert!(usage_message(&error).contains("read-only"), "{error:?}");
    assert_eq!(engine.connects(), 0, "trino refuses before connecting");
}

/// The PostgreSQL Safe Mode bypasses W3-T0b closed. The generic reading is fooled by three
/// spellings PostgreSQL lexes differently: `E'…'` strings honour a backslash, block comments
/// nest, and `$` continues an identifier so `a$x$` is a name and not a dollar quote. Each is
/// one harmless SELECT to the generic reading and a SELECT plus a `DELETE` to the server, and
/// the only thing that stopped it was the driver's prepare refusing a second statement. The
/// guard must refuse them itself, in every command that takes SQL, before connecting.
const POSTGRES_PAYLOADS: [&str; 6] = [
    "SELECT E'x\\' AS a, '; DELETE FROM t; --'",
    "SELECT 1 /* /* */ ' */; DELETE FROM t; --'",
    "SELECT 1 AS a$x$; DELETE FROM t; --$x$",
    // `standard_conforming_strings` off: a plain string honours the backslash too, and the
    // guard cannot see the setting, so it refuses what the two settings read differently.
    "SELECT 'x\\' AS a, '; DELETE FROM t; --'",
    // A carriage return ends a `--` comment.
    "SELECT 1 -- x\r; DELETE FROM t",
    // A continued E string keeps honouring the backslash.
    "SELECT E'a'\n'x\\'y'; DELETE FROM t; --'",
];

#[tokio::test]
async fn read_only_refuses_a_postgres_lexing_injection_before_connecting() {
    let postgres = |sql: &'static str| {
        vec![
            ("DB_KIND", "postgres"),
            ("DB_HOST", "postgres.invalid"),
            ("DB_PORT", "5432"),
            ("DB_USER", "queryhive"),
            ("SAFE_MODE", "read_only"),
            ("SQL", sql),
            ("FORMAT", "csv"),
            ("OUT_DIR", "/tmp"),
        ]
    };
    for payload in POSTGRES_PAYLOADS {
        for command in [
            Command::Preview,
            Command::Count,
            Command::Explain,
            Command::Export,
        ] {
            let engine = CountingEngine::new();
            let error = refuse(command, &engine, &postgres(payload)).await;
            let message = usage_message(&error);
            assert!(
                message.contains("read-only") || message.contains("could not tell"),
                "{command:?} {payload:?}: {message}"
            );
            assert_eq!(
                engine.connects(),
                0,
                "{command:?}: the write must be refused before the server is reached: {payload:?}"
            );
        }
    }
}

/// A URL-only PostgreSQL connection (no `DB_KIND`) is read as PostgreSQL too, so the scheme
/// is enough to get the PostgreSQL guard, and `confirm` and `no_ddl` refuse the same text.
#[tokio::test]
async fn a_postgres_url_and_the_other_modes_get_the_postgres_guard_too() {
    let engine = CountingEngine::new();
    let error = refuse(
        Command::Preview,
        &engine,
        &[
            ("DB_KIND", ""),
            ("DB_URL", "postgresql://queryhive@postgres.invalid:5432/app"),
            ("SAFE_MODE", "read_only"),
            ("SQL", POSTGRES_PAYLOADS[0]),
        ],
    )
    .await;
    assert!(usage_message(&error).contains("read-only"), "{error:?}");
    assert_eq!(engine.connects(), 0);

    for (mode, ddl) in [("confirm", false), ("no_ddl", true)] {
        let sql = if ddl {
            POSTGRES_PAYLOADS[2].replace("DELETE FROM t", "DROP TABLE t")
        } else {
            POSTGRES_PAYLOADS[2].to_owned()
        };
        let engine = CountingEngine::new();
        let error = refuse(
            Command::Preview,
            &engine,
            &[
                ("DB_KIND", "postgres"),
                ("DB_HOST", "postgres.invalid"),
                ("DB_USER", "queryhive"),
                ("SAFE_MODE", mode),
                ("SQL", &sql),
            ],
        )
        .await;
        let message = usage_message(&error);
        assert!(message.contains(mode), "{mode}: {message}");
        assert_eq!(engine.connects(), 0, "{mode}");
    }
}

/// The bulk paths guard with the same PostgreSQL reading: a plan and a script that hide a
/// DDL statement from the generic reading are refused, and no statement of them runs.
#[tokio::test]
async fn a_postgres_plan_and_a_postgres_script_are_guarded_by_the_postgres_reading() {
    let ddl = "SELECT 1 AS a$x$; DROP TABLE t; --$x$";
    let engine = CountingEngine::new();
    let changes = format!(r#"[{{"sql":{}}}]"#, serde_json::to_string(ddl).unwrap());
    let error = refuse(
        Command::ApplyChanges,
        &engine,
        &[
            ("DB_KIND", "postgres"),
            ("DB_HOST", "postgres.invalid"),
            ("DB_USER", "queryhive"),
            ("SAFE_MODE", "no_ddl"),
            ("CHANGES", &changes),
        ],
    )
    .await;
    let message = usage_message(&error);
    assert!(message.contains("change 1 was refused"), "{message}");
    assert!(message.contains("DDL"), "{message}");
    assert_eq!(engine.connects(), 0);

    let path = std::env::temp_dir().join(format!("qh-safe-mode-pg-{}.sql", std::process::id()));
    std::fs::write(&path, format!("SELECT 1;\n{ddl}\n")).expect("write the script");
    let engine = CountingEngine::new();
    let error = refuse(
        Command::ImportData,
        &engine,
        &[
            ("DB_KIND", "postgres"),
            ("DB_HOST", "postgres.invalid"),
            ("DB_USER", "queryhive"),
            ("SAFE_MODE", "no_ddl"),
            ("IMPORT_PATH", path.to_str().expect("utf-8 path")),
        ],
    )
    .await;
    let _ = std::fs::remove_file(&path);
    let message = usage_message(&error);
    assert!(message.contains("no_ddl"), "{message}");
    assert_eq!(engine.connects(), 0);
}

/// What a PostgreSQL read looks like must still reach the server: escape strings, nested
/// comments with nothing behind them, parameters, dollar-quoted text, and identifiers with a
/// `$`.
#[tokio::test]
async fn ordinary_postgres_reads_reach_the_connect_step_under_read_only() {
    for sql in [
        "SELECT E'\\n', E'it\\'s'",
        "SELECT 1 /* a /* b */ c */ -- tail",
        "SELECT $1::int, $2::int",
        "SELECT $$a;b$$",
        "SELECT foo$bar FROM t; -- trailing",
    ] {
        let engine = CountingEngine::new();
        let error = refuse(
            Command::Preview,
            &engine,
            &[
                ("DB_KIND", "postgres"),
                ("DB_HOST", "postgres.invalid"),
                ("DB_USER", "queryhive"),
                ("SAFE_MODE", "read_only"),
                ("SQL", sql),
            ],
        )
        .await;
        assert!(
            !matches!(error, CliError::Usage(ref message) if message.contains("read-only")
                || message.contains("could not tell")),
            "{sql:?}: {error:?}"
        );
        assert_eq!(engine.connects(), 1, "{sql:?} reaches the connect step");
    }
}

/// Trino has its own reading: a `--` comment ends at a carriage return there (live:
/// `SELECT 1 -- c\r, 2` returns two columns), where the generic reading waits for a newline. So
/// `EXPLAIN -- note\r ANALYZE INSERT …` was one harmless EXPLAIN to the guard and a write to the
/// server, and a `read_only` Trino connection wrote a row. The guard must refuse it, in every
/// command that takes SQL, before connecting.
#[tokio::test]
async fn read_only_refuses_a_trino_carriage_return_comment_before_connecting() {
    let payloads = [
        "EXPLAIN /* n */ -- note\r ANALYZE INSERT INTO memory.default.t VALUES (1)",
        "SELECT 1 -- note\r; DELETE FROM memory.default.t",
    ];
    for payload in payloads {
        for command in [
            Command::Preview,
            Command::Count,
            Command::Explain,
            Command::Export,
        ] {
            let engine = CountingEngine::new();
            let error = refuse(
                command,
                &engine,
                &[
                    ("SAFE_MODE", "read_only"),
                    ("SQL", payload),
                    ("FORMAT", "csv"),
                    ("OUT_DIR", "/tmp"),
                ],
            )
            .await;
            let message = usage_message(&error);
            assert!(
                message.contains("read-only") || message.contains("could not tell"),
                "{command:?} {payload:?}: {message}"
            );
            assert_eq!(engine.connects(), 0, "{command:?} {payload:?}");
        }
    }
}

/// What Trino reads as one harmless statement still reaches the server: its block comments do
/// not nest, and a newline still ends a `--` comment.
#[tokio::test]
async fn ordinary_trino_reads_reach_the_connect_step_under_read_only() {
    for sql in [
        "SELECT 1 /* a */ , 2 -- DELETE\r\n",
        "SELECT 1 -- note\n, 2",
        "SELECT 'a\\' , 'b'",
        "SHOW CATALOGS",
    ] {
        let engine = CountingEngine::new();
        let error = refuse(
            Command::Preview,
            &engine,
            &[("SAFE_MODE", "read_only"), ("SQL", sql)],
        )
        .await;
        assert!(
            !matches!(error, CliError::Usage(ref message) if message.contains("read-only")
                || message.contains("could not tell")),
            "{sql:?}: {error:?}"
        );
        assert_eq!(engine.connects(), 1, "{sql:?}");
    }
}

/// Text that can switch the PostgreSQL session's `client_encoding` is refused below `full`: in a
/// multibyte encoding whose second byte can be a backslash, a later `E'…'` string ends somewhere
/// no reading predicts, and a script runs on one session.
#[tokio::test]
async fn a_postgres_encoding_switch_is_refused_before_connecting() {
    let payloads = [
        "SELECT set_config('client_encoding', 'SJIS', false)",
        "SELECT pg_catalog.\"set_config\"('client_encoding', 'GBK', false)",
        "SELECT U&\"set\\005fconfig\"('a', 'b', false)",
    ];
    for payload in payloads {
        for mode in ["read_only", "confirm", "no_ddl"] {
            let engine = CountingEngine::new();
            let error = refuse(
                Command::Preview,
                &engine,
                &[
                    ("DB_KIND", "postgres"),
                    ("DB_HOST", "postgres.invalid"),
                    ("DB_USER", "queryhive"),
                    ("SAFE_MODE", mode),
                    ("SQL", payload),
                ],
            )
            .await;
            assert!(
                matches!(error, CliError::Usage(_)),
                "{mode} {payload:?}: {error:?}"
            );
            assert_eq!(engine.connects(), 0, "{mode} {payload:?}");
        }
    }
    // The import path the review used: a script runs on one session.
    let path = std::env::temp_dir().join(format!("qh-safe-mode-enc-{}.sql", std::process::id()));
    std::fs::write(
        &path,
        "SELECT set_config('client_encoding', 'SJIS', false);\nSELECT E'\\x83\\'; DELETE FROM t; --';\n",
    )
    .expect("write the script");
    let engine = CountingEngine::new();
    let error = refuse(
        Command::ImportData,
        &engine,
        &[
            ("DB_KIND", "postgres"),
            ("DB_HOST", "postgres.invalid"),
            ("DB_USER", "queryhive"),
            ("SAFE_MODE", "read_only"),
            ("IMPORT_PATH", path.to_str().expect("utf-8 path")),
        ],
    )
    .await;
    let _ = std::fs::remove_file(&path);
    assert!(
        usage_message(&error).contains("could not tell"),
        "{error:?}"
    );
    assert_eq!(engine.connects(), 0);
    // `full` refuses nothing.
    let engine = CountingEngine::new();
    let error = refuse(
        Command::Preview,
        &engine,
        &[
            ("DB_KIND", "postgres"),
            ("DB_HOST", "postgres.invalid"),
            ("DB_USER", "queryhive"),
            ("SQL", payloads[0]),
        ],
    )
    .await;
    assert!(matches!(error, CliError::Connect(_)), "{error:?}");
}

/// `EXECUTE`, `DO`, `CALL` and `COPY … PROGRAM` are unclassified, so `no_ddl` refuses them too.
#[tokio::test]
async fn no_ddl_refuses_the_opaque_statements() {
    for sql in [
        "EXECUTE p(1)",
        "DO $$ BEGIN NULL; END $$",
        "CALL p()",
        "COPY t TO PROGRAM 'id'",
        "ANALYSE t",
        "SELECT * FROM t FOR SHARE",
    ] {
        let engine = CountingEngine::new();
        let mode = if sql.contains("FOR SHARE") {
            "read_only"
        } else {
            "no_ddl"
        };
        let error = refuse(
            Command::Preview,
            &engine,
            &[("DB_KIND", "postgres"), ("SAFE_MODE", mode), ("SQL", sql)],
        )
        .await;
        assert!(
            matches!(error, CliError::Usage(_)),
            "{mode} {sql:?}: {error:?}"
        );
        assert_eq!(engine.connects(), 0, "{sql:?}");
    }
}

// --------------------------------------------------------------------------- //
// the server-enforced read-only layer: `open()` asks the session to refuse writes
// --------------------------------------------------------------------------- //

/// A session that records every statement, and whose read-only switch is a statement it can
/// be seen sending.
struct RecordingSession {
    inner: Box<dyn Driver>,
    statements: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

struct EmptyCursor;

#[async_trait]
impl qh_driver::Cursor for EmptyCursor {
    fn columns(&self) -> &[qh_core::ColumnMeta] {
        &[]
    }

    async fn next_batch(
        &mut self,
        _max_rows: usize,
    ) -> Result<Option<qh_core::ColumnBatch>, EngineError> {
        Ok(None)
    }

    fn affected_rows(&self) -> Option<u64> {
        None
    }
}

#[async_trait]
impl Session for RecordingSession {
    fn capabilities(&self) -> qh_driver::Capabilities {
        self.inner.capabilities()
    }

    fn query_id(&self) -> Option<String> {
        None
    }

    async fn execute(
        &mut self,
        sql: &str,
        _options: &qh_driver::ExecuteOptions,
    ) -> Result<Box<dyn qh_driver::Cursor>, EngineError> {
        self.statements.lock().unwrap().push(sql.to_owned());
        Ok(Box::new(EmptyCursor))
    }

    async fn browse(
        &mut self,
        _level: qh_driver::BrowseLevel,
        _path: &qh_driver::ObjectPath,
        _include_system: bool,
    ) -> Result<Vec<String>, EngineError> {
        Ok(Vec::new())
    }

    async fn objects(
        &mut self,
        _path: &qh_driver::ObjectPath,
    ) -> Result<qh_driver::ObjectsPage, EngineError> {
        Ok(qh_driver::ObjectsPage::default())
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

    fn read_only_statement(&self) -> Option<&'static str> {
        Some("SET SESSION READ ONLY (test)")
    }
}

struct RecordingEngine {
    inner: RealEngine,
    statements: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl Engine for RecordingEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        self.inner.kinds()
    }

    fn driver(&self, kind: DriverKind) -> &dyn Driver {
        self.inner.driver(kind)
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        let _ = config;
        Ok(Box::new(RecordingSession {
            inner: Box::new(qh_driver_postgres::PostgresDriver::new()),
            statements: self.statements.clone(),
        }))
    }
}

async fn statements_sent(extra: &[(&str, &str)]) -> Vec<String> {
    let statements = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let engine = RecordingEngine {
        inner: RealEngine::new(),
        statements: statements.clone(),
    };
    let mut pairs: Vec<(&str, &str)> = vec![
        ("DB_KIND", "postgres"),
        ("DB_HOST", "postgres.invalid"),
        ("DB_USER", "queryhive"),
        ("SQL", "SELECT 1"),
    ];
    pairs.extend_from_slice(extra);
    let mut out = Capture::new();
    run(
        Command::Preview,
        &settings(&pairs),
        &mut out,
        &engine,
        &CancelFlag::new(),
    )
    .await
    .expect("the preview runs on the recording session");
    let sent = statements.lock().unwrap().clone();
    sent
}

#[tokio::test]
async fn a_read_only_run_asks_the_session_to_refuse_writes_before_its_own_statement() {
    // Under `read_only`, and under a read-only connection floor, the switch goes first.
    for extra in [
        &[("SAFE_MODE", "read_only")][..],
        &[("DB_READ_ONLY", "1")][..],
        &[("SAFE_MODE", "no_ddl"), ("SAFE_MODE_FLOOR", "read_only")][..],
    ] {
        assert_eq!(
            statements_sent(extra).await,
            vec!["SET SESSION READ ONLY (test)", "SELECT 1"],
            "{extra:?}"
        );
    }
    // No other level asks: `full`, `no_ddl` and `confirm` write, so the server may too.
    for mode in ["full", "no_ddl", "confirm"] {
        assert_eq!(
            statements_sent(&[("SAFE_MODE", mode)]).await,
            vec!["SELECT 1"],
            "{mode}"
        );
    }
    assert_eq!(statements_sent(&[]).await, vec!["SELECT 1"]);
}

/// A `#` comment holding words that look like writes is a comment on MySQL under every
/// sql_mode, so a read-only connection lets the read through to the server.
#[tokio::test]
async fn read_only_lets_a_mysql_hash_comment_read_through() {
    for sql in [
        "SELECT 1 # plain hash comment",
        "SELECT 1 # remember to update this later",
        "SELECT 'a#b', 'a--b' -- delete me",
    ] {
        let engine = CountingEngine::new();
        let error = refuse(
            Command::Preview,
            &engine,
            &[
                ("DB_KIND", "mysql"),
                ("DB_HOST", "mysql.invalid"),
                ("DB_PORT", "3306"),
                ("DB_USER", "queryhive"),
                ("SAFE_MODE", "read_only"),
                ("SQL", sql),
            ],
        )
        .await;
        assert!(
            !matches!(error, CliError::Usage(ref message) if message.contains("read-only")
                || message.contains("could not tell")),
            "{sql:?}: {error:?}"
        );
        assert_eq!(engine.connects(), 1, "{sql:?} reaches the connect step");
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

#[tokio::test]
async fn table_op_is_gated_like_every_other_write() {
    // The two operations the plan asks for "lewat konfirmasi" (§13). `TRUNCATE`/`DROP` are
    // DDL, so `no_ddl` and `read_only` refuse them before the connection is touched, exactly
    // as any other DDL; `confirm` asks for the caller's confirmation, which is ADR-0027's one
    // deliberate difference from the ordinary guard.
    let targets = [
        ("TARGET_CATALOG", "hive"),
        ("TARGET_SCHEMA", "default"),
        ("TARGET_TABLE", "people"),
    ];

    // `read_only` refuses before the connection is touched.
    let engine = CountingEngine::new();
    let mut pairs: Vec<(&str, &str)> = vec![("SAFE_MODE", "read_only"), ("TABLE_OP", "drop")];
    pairs.extend_from_slice(&targets);
    let error = refuse(Command::TableOp, &engine, &pairs).await;
    let message = usage_message(&error);
    assert!(message.contains("read-only"), "{message}");
    assert!(message.contains("DDL"), "{message}");
    assert_eq!(engine.connects(), 0);

    // It is DDL, so `no_ddl` refuses it too.
    let engine = CountingEngine::new();
    let mut pairs: Vec<(&str, &str)> = vec![("SAFE_MODE", "no_ddl"), ("TABLE_OP", "truncate")];
    pairs.extend_from_slice(&targets);
    let error = refuse(Command::TableOp, &engine, &pairs).await;
    assert!(usage_message(&error).contains("no_ddl"), "{error:?}");
    assert_eq!(engine.connects(), 0);

    // `confirm` asks first...
    let engine = CountingEngine::new();
    let mut pairs: Vec<(&str, &str)> = vec![("SAFE_MODE", "confirm"), ("TABLE_OP", "drop")];
    pairs.extend_from_slice(&targets);
    let error = refuse(Command::TableOp, &engine, &pairs).await;
    assert!(
        usage_message(&error).contains("requires confirmation"),
        "{error:?}"
    );
    assert_eq!(engine.connects(), 0);

    // ...and runs once the caller answers. Reaching `connect` is the proof: this engine fails
    // it on purpose, so only a run the guard let through gets there.
    let mut pairs: Vec<(&str, &str)> = vec![
        ("SAFE_MODE", "confirm"),
        ("SAFE_MODE_CONFIRMED", "1"),
        ("TABLE_OP", "drop"),
    ];
    pairs.extend_from_slice(&targets);
    let error = refuse(Command::TableOp, &engine, &pairs).await;
    assert!(
        matches!(error, CliError::Connect(_)),
        "a confirmed drop should reach the connection, got {error:?}"
    );
    assert_eq!(engine.connects(), 1);

    // `full` runs it without asking.
    let engine = CountingEngine::new();
    let mut pairs: Vec<(&str, &str)> = vec![("TABLE_OP", "truncate")];
    pairs.extend_from_slice(&targets);
    let error = refuse(Command::TableOp, &engine, &pairs).await;
    assert!(matches!(error, CliError::Connect(_)), "{error:?}");
    assert_eq!(engine.connects(), 1);

    // An operation nobody offers, and a missing one, are refused by name before any guard.
    let engine = CountingEngine::new();
    let error = refuse(Command::TableOp, &engine, &[("TABLE_OP", "vanish")]).await;
    assert_eq!(
        usage_message(&error),
        "unknown TABLE_OP 'vanish'; expected drop or truncate"
    );
    let error = refuse(Command::TableOp, &engine, &[]).await;
    assert_eq!(
        usage_message(&error),
        "TABLE_OP is required: drop or truncate"
    );
    assert_eq!(engine.connects(), 0);
}

#[tokio::test]
async fn a_no_ddl_floor_refuses_a_confirmed_drop() {
    // ADR-0027 recorded this as its one cost: a total strictness order cannot say "this
    // floor forbids DDL whatever else is pinned". With `SAFE_MODE=confirm` and
    // `SAFE_MODE_FLOOR=no_ddl`, the resolved mode is `confirm` (2 > 1), so the ordinary
    // guard asked and a confirmed `DROP` ran on a connection whose floor forbids all DDL.
    // The floor's own condition is what decides now.
    let engine = CountingEngine::new();
    let error = refuse(
        Command::TableOp,
        &engine,
        &[
            ("SAFE_MODE", "confirm"),
            ("SAFE_MODE_FLOOR", "no_ddl"),
            ("SAFE_MODE_CONFIRMED", "1"),
            ("TABLE_OP", "drop"),
            ("TARGET_CATALOG", "hive"),
            ("TARGET_SCHEMA", "default"),
            ("TARGET_TABLE", "people"),
        ],
    )
    .await;
    let message = usage_message(&error);
    assert!(message.contains("no_ddl"), "{message}");
    assert!(
        message.contains("DDL"),
        "the refusal should be the classifier's own DDL sentence: {message}"
    );
    assert_eq!(
        engine.connects(),
        0,
        "a floor that forbids DDL must refuse before connecting"
    );
}

#[tokio::test]
async fn a_users_own_no_ddl_refuses_a_confirm_floor() {
    // The same hole from the other side, and the one a managed profile would create: the
    // user's connection is `no_ddl` while the pinned floor is only `confirm`. Strictness
    // alone resolves to `confirm`, but the user's own level refuses DDL and a floor may
    // only ever be stricter, so the answer is the refusal.
    let engine = CountingEngine::new();
    let error = refuse(
        Command::TableOp,
        &engine,
        &[
            ("SAFE_MODE", "no_ddl"),
            ("SAFE_MODE_FLOOR", "confirm"),
            ("SAFE_MODE_CONFIRMED", "1"),
            ("TABLE_OP", "truncate"),
            ("TARGET_CATALOG", "hive"),
            ("TARGET_SCHEMA", "default"),
            ("TARGET_TABLE", "people"),
        ],
    )
    .await;
    assert!(usage_message(&error).contains("no_ddl"), "{error:?}");
    assert_eq!(engine.connects(), 0);
}
