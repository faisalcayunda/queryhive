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

    /// Spelled so a test can read what the engine resolved the options to.
    fn explain_statement_with(
        &self,
        sql: &str,
        options: qh_driver::ExplainOptions,
    ) -> Result<String, EngineError> {
        Ok(format!(
            "EXPLAIN<{:?},{}> {sql}",
            options.format, options.analyze
        ))
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

// --------------------------------------------------------------------------- //
// the metadata commands (W11-T1, NFR-S1): read-only whatever the mode, whatever the name
// --------------------------------------------------------------------------- //

/// A session that records every statement and answers the metadata ones with one canned row, so a
/// recipe that asks for several statements asks for all of them.
struct ScriptedSession {
    driver: Box<dyn Driver>,
    statements: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

/// One batch of text cells, column-major as `ColumnBatch` wants them.
struct OneBatch {
    columns: Vec<qh_core::ColumnMeta>,
    batch: Option<qh_core::ColumnBatch>,
}

#[async_trait]
impl qh_driver::Cursor for OneBatch {
    fn columns(&self) -> &[qh_core::ColumnMeta] {
        &self.columns
    }

    async fn next_batch(
        &mut self,
        _max_rows: usize,
    ) -> Result<Option<qh_core::ColumnBatch>, EngineError> {
        Ok(self.batch.take())
    }
}

/// What a statement is answered with: the cells of one row, `None` for a NULL, or no row at all.
fn canned(sql: &str) -> Option<Vec<Option<&'static str>>> {
    if sql.contains("pg_get_viewdef") {
        // The PostgreSQL head: an ordinary table.
        Some(vec![
            Some("r"),
            Some("s"),
            Some("t"),
            None,
            None,
            None,
            None,
        ])
    } else if sql.contains("pg_get_constraintdef") || sql.contains("pg_get_indexdef") {
        None
    } else if sql.contains("information_schema.tables") && sql.contains("table_name =") {
        // Trino's kind lookup.
        Some(vec![Some("BASE TABLE")])
    } else if sql.starts_with("SHOW CREATE") {
        Some(vec![
            Some("CREATE TABLE t (id int)"),
            Some("CREATE TABLE t (id int)"),
        ])
    } else if sql.contains("pg_attribute") {
        Some(vec![Some("id"), Some("bigint"), Some("NO"), None, Some("")])
    } else {
        Some(vec![Some("t"), Some("BASE TABLE")])
    }
}

#[async_trait]
impl Session for ScriptedSession {
    fn capabilities(&self) -> qh_driver::Capabilities {
        self.driver.capabilities()
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
        let Some(row) = canned(sql) else {
            return Ok(Box::new(OneBatch {
                columns: Vec::new(),
                batch: None,
            }));
        };
        let columns = (0..row.len())
            .map(|index| qh_core::ColumnMeta::new(format!("c{index}"), "text"))
            .collect();
        let batch = qh_core::ColumnBatch::new(
            row.iter()
                .map(|cell| {
                    vec![cell.map_or(qh_core::Value::Null, |text| {
                        qh_core::Value::Text(text.into())
                    })]
                })
                .collect(),
        )
        .expect("one row of equal columns");
        Ok(Box::new(OneBatch {
            columns,
            batch: Some(batch),
        }))
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

struct ScriptedEngine {
    inner: RealEngine,
    statements: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

#[async_trait]
impl Engine for ScriptedEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        self.inner.kinds()
    }

    fn driver(&self, kind: DriverKind) -> &dyn Driver {
        self.inner.driver(kind)
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        // The real driver's capabilities, so the session reads as the kind it is standing in for.
        let driver: Box<dyn Driver> = match config.kind {
            DriverKind::Postgres => Box::new(qh_driver_postgres::PostgresDriver::new()),
            DriverKind::Mysql => Box::new(qh_driver_mysql::MysqlDriver::new()),
            DriverKind::Trino => Box::new(qh_driver_trino::TrinoDriver::new()),
        };
        Ok(Box::new(ScriptedSession {
            driver,
            statements: self.statements.clone(),
        }))
    }
}

/// The three kinds, with the connection settings and the `TARGET_*` slots each one names.
fn metadata_connections(name: &str) -> Vec<(DriverKind, Vec<(&str, String)>)> {
    let target = |catalog: bool, schema: bool| {
        let mut slots = vec![("TARGET_TABLE", name.to_owned())];
        if catalog {
            slots.push(("TARGET_CATALOG", name.to_owned()));
        }
        if schema {
            slots.push(("TARGET_SCHEMA", name.to_owned()));
        }
        slots
    };
    let connection = |kind: &'static str, port: &'static str, database: &str, schema: &str| {
        vec![
            ("DB_KIND", kind.to_owned()),
            ("DB_HOST", "db.invalid".to_owned()),
            ("DB_PORT", port.to_owned()),
            ("DB_USER", "queryhive".to_owned()),
            ("DB_DATABASE", database.to_owned()),
            ("DB_SCHEMA", schema.to_owned()),
            ("RETRIES", "0".to_owned()),
        ]
    };
    let join = |mut first: Vec<(&'static str, String)>, second: Vec<(&'static str, String)>| {
        first.extend(second);
        first
    };
    vec![
        (
            DriverKind::Postgres,
            join(
                connection("postgres", "5432", name, name),
                target(false, true),
            ),
        ),
        (
            DriverKind::Mysql,
            join(connection("mysql", "3306", name, ""), target(true, false)),
        ),
        (
            DriverKind::Trino,
            join(connection("trino", "8080", name, name), target(true, true)),
        ),
    ]
}

/// Every statement a metadata command sent, except the guard's own read-only switch.
async fn metadata_statements(
    command: Command,
    kind_settings: &[(&str, String)],
    mode: &str,
    objects: bool,
) -> Vec<String> {
    let statements = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let engine = ScriptedEngine {
        inner: RealEngine::new(),
        statements: statements.clone(),
    };
    let mut pairs: Vec<(&str, &str)> = kind_settings
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    pairs.push(("SAFE_MODE", mode));
    if objects {
        pairs.push(("OBJECT_KINDS", "1"));
    }
    let mut out = Capture::new();
    run(
        command,
        &settings(&pairs),
        &mut out,
        &engine,
        &CancelFlag::new(),
    )
    .await
    .unwrap_or_else(|error| panic!("{command:?} under {mode}: {error:?} {pairs:?}"));
    let sent = statements.lock().unwrap().clone();
    sent.into_iter()
        .filter(|sql| sql != "SET SESSION READ ONLY (test)")
        .collect()
}

/// NFR-S1: `columns`, `ddl` and `tables` with kinds send only single read-only statements, under
/// every Safe Mode including `read_only`, and under every reading of every dialect. The names are
/// the ones a hostile schema would hold: quotes of all three kinds, a semicolon, a comment opener,
/// and a backslash that would swallow a closing quote under one of the readings.
#[tokio::test]
async fn the_metadata_commands_send_only_read_only_statements_under_every_mode_and_name() {
    let names = [
        "people",
        "a'b",
        "a\"b",
        "a`b",
        "x; DROP TABLE y",
        "x -- y",
        "x /* y */",
        "x # y",
        "a\\",
        "a\\'; DROP TABLE y; --",
        "'; DROP TABLE y; --",
    ];
    for name in names {
        for (kind, kind_settings) in metadata_connections(name) {
            let dialect = match kind {
                DriverKind::Postgres => qh_sql::Dialect::Postgres,
                DriverKind::Mysql => qh_sql::Dialect::Mysql,
                DriverKind::Trino => qh_sql::Dialect::Trino,
            };
            for mode in qh_sql::SAFE_MODES {
                for (command, objects) in [
                    (Command::Columns, false),
                    (Command::Ddl, false),
                    (Command::Tables, true),
                ] {
                    let sent = metadata_statements(command, &kind_settings, mode, objects).await;
                    assert!(!sent.is_empty(), "{kind} {command:?} sent nothing");
                    for sql in sent {
                        let decisions = qh_sql::decisions_readings(
                            qh_sql::SafeMode::ReadOnly,
                            &sql,
                            dialect.readings(),
                        );
                        assert_eq!(
                            decisions.len(),
                            1,
                            "{kind} {command:?} {name:?} under {mode}: more than one statement: {sql}"
                        );
                        assert_eq!(
                            decisions[0].kind,
                            qh_sql::StatementKind::ReadOnly,
                            "{kind} {command:?} {name:?} under {mode}: {sql}"
                        );
                    }
                }
            }
        }
    }
}

/// A DDL recipe is asked for every one of its statements, so a write hiding in the third step
/// would show here and not only in the first.
#[tokio::test]
async fn a_postgres_ddl_sends_all_four_of_its_statements_and_all_of_them_read() {
    let (_, settings_of_kind) = metadata_connections("people").remove(0);
    let sent = metadata_statements(Command::Ddl, &settings_of_kind, "read_only", false).await;
    assert_eq!(sent.len(), 4, "{sent:#?}");
    assert!(sent[0].contains("pg_get_viewdef"), "{}", sent[0]);
    assert!(sent[1].contains("pg_attribute"), "{}", sent[1]);
    assert!(sent[2].contains("pg_constraint"), "{}", sent[2]);
    assert!(sent[3].contains("pg_get_indexdef"), "{}", sent[3]);
}

/// A name a driver has no slot for is not needed, and one it has a slot for is: refused by the
/// setting's name, before anything is opened.
#[tokio::test]
async fn a_missing_target_is_refused_by_name_before_the_engine_connects() {
    let engine = CountingEngine::new();
    for (command, setting) in [
        (Command::Columns, "TARGET_CATALOG"),
        (Command::Ddl, "TARGET_CATALOG"),
    ] {
        let error = refuse(command, &engine, &[("TARGET_TABLE", "t")]).await;
        assert!(usage_message(&error).contains(setting), "{error:?}");
    }
    assert_eq!(engine.connects(), 0);
}

/// A driver that cannot describe objects is a usage error naming it, not an empty answer.
#[tokio::test]
async fn a_driver_without_metadata_is_refused_by_name() {
    struct Bare;

    #[async_trait]
    impl Driver for Bare {
        fn kind(&self) -> DriverKind {
            DriverKind::Postgres
        }
        fn label(&self) -> &'static str {
            "Bare"
        }
        fn default_port(&self) -> u16 {
            1
        }
        fn capabilities(&self) -> qh_driver::Capabilities {
            qh_driver_postgres::PostgresDriver::new().capabilities()
        }
        async fn connect(&self, _: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
            unreachable!("refused before connecting")
        }
    }

    struct BareEngine;

    #[async_trait]
    impl Engine for BareEngine {
        fn kinds(&self) -> Vec<DriverKind> {
            vec![DriverKind::Postgres]
        }
        fn driver(&self, _: DriverKind) -> &dyn Driver {
            &Bare
        }
        async fn connect(&self, _: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
            unreachable!("refused before connecting")
        }
    }

    let (_, pairs) = metadata_connections("t").remove(0);
    let pairs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    let error = run(
        Command::Columns,
        &settings(&pairs),
        &mut Capture::new(),
        &BareEngine,
        &CancelFlag::new(),
    )
    .await
    .expect_err("a driver with no metadata cannot describe an object");
    assert!(
        usage_message(&error).contains("postgres cannot describe"),
        "{error:?}"
    );
}

// --------------------------------------------------------------------------- //
// W13-T2: EXPLAIN JSON and ANALYZE, REQUIRE_READ, and functions with an effect
// --------------------------------------------------------------------------- //

/// What a command did on the recording session: how it ended, what it sent, what it emitted.
struct Recorded {
    result: Result<(), CliError>,
    statements: Vec<String>,
    events: Vec<serde_json::Value>,
}

impl Recorded {
    fn done(&self) -> &serde_json::Value {
        self.events
            .iter()
            .rev()
            .find(|event| event["event"] == "done")
            .unwrap_or_else(|| panic!("no done event in {:?}", self.events))
    }
}

async fn record(command: Command, extra: &[(&str, &str)]) -> Recorded {
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
    let result = run(
        command,
        &settings(&pairs),
        &mut out,
        &engine,
        &CancelFlag::new(),
    )
    .await;
    let events = out
        .lines()
        .iter()
        .map(|line| serde_json::from_str(line).expect("an event is JSON"))
        .collect();
    let sent = statements.lock().unwrap().clone();
    Recorded {
        result,
        statements: sent,
        events,
    }
}

const ANALYZE_SENT: [&str; 3] = ["BEGIN READ ONLY", "EXPLAIN<Text,true> SELECT 1", "ROLLBACK"];

/// An ANALYZE of a read runs in every mode, inside a read-only transaction it rolls back.
#[tokio::test]
async fn explain_analyze_of_a_read_is_allowed_in_every_mode() {
    for mode in ["full", "no_ddl", "confirm", "read_only"] {
        let recorded = record(
            Command::Explain,
            &[("EXPLAIN_ANALYZE", "1"), ("SAFE_MODE", mode)],
        )
        .await;
        recorded
            .result
            .as_ref()
            .unwrap_or_else(|error| panic!("{mode}: {error:?}"));
        let mut expected: Vec<&str> = Vec::new();
        if mode == "read_only" {
            expected.push("SET SESSION READ ONLY (test)");
        }
        expected.extend(ANALYZE_SENT);
        assert_eq!(recorded.statements, expected, "{mode}");
    }
    // The plain plan and the JSON plan send nothing around the statement.
    let recorded = record(Command::Explain, &[("EXPLAIN_FORMAT", "json")]).await;
    recorded.result.as_ref().expect("the plan runs");
    assert_eq!(recorded.statements, ["EXPLAIN<Json,false> SELECT 1"]);
    assert!(
        recorded.done().get("warnings").is_none(),
        "{:?}",
        recorded.done()
    );
    let recorded = record(Command::Explain, &[]).await;
    recorded.result.as_ref().expect("the plan runs");
    assert_eq!(recorded.statements, ["EXPLAIN<Text,false> SELECT 1"]);
}

/// The two forms a driver cannot spell are said, not hidden: the plan comes back as text.
#[tokio::test]
async fn explain_format_json_downgrades_with_a_warning() {
    let recorded = record(
        Command::Explain,
        &[("DB_KIND", "mysql"), ("EXPLAIN_FORMAT", "json")],
    )
    .await;
    recorded.result.as_ref().expect("the plan runs");
    assert_eq!(recorded.statements, ["EXPLAIN<Text,false> SELECT 1"]);
    assert_eq!(
        recorded.done()["warnings"],
        serde_json::json!(["MySQL plans are returned as text."])
    );

    // Trino has no JSON form of ANALYZE: ANALYZE wins, and it is run unfenced because Trino has
    // no transaction to put it in.
    let recorded = record(
        Command::Explain,
        &[
            ("DB_KIND", "trino"),
            ("EXPLAIN_FORMAT", "json"),
            ("EXPLAIN_ANALYZE", "1"),
        ],
    )
    .await;
    recorded.result.as_ref().expect("the plan runs");
    assert_eq!(recorded.statements, ["EXPLAIN<Text,true> SELECT 1"]);
    assert_eq!(
        recorded.done()["warnings"],
        serde_json::json!(["Trino has no JSON form of EXPLAIN ANALYZE; the plan is text."])
    );
}

/// The statements an ANALYZE must never run. Each is a write, or something the classifier will
/// not vouch for, and each is refused whatever mode asks for it.
const NOT_FOR_ANALYZE: [&str; 9] = [
    "INSERT INTO w13t2_{} VALUES (1)",
    "UPDATE w13t2_{} SET a = 1",
    "DELETE FROM w13t2_{}",
    "WITH x AS (DELETE FROM w13t2_{} RETURNING 1) SELECT * FROM x",
    "SELECT * FROM w13t2_{} FOR UPDATE",
    "SELECT * INTO w13t2_{}_copy FROM w13t2_{}",
    "EXECUTE w13t2_{}(1)",
    "DO $$ BEGIN PERFORM 1 FROM w13t2_{}; END $$",
    "SELECT pg_terminate_backend(w13t2_{})",
];

fn log_rows_for(statement: &str) -> Vec<(String, String, Option<String>)> {
    let hash = qh_storage::hash_statement(statement);
    qh_ffi::execution_log::with_storage(|storage| {
        storage
            .execution_log(10_000)
            .expect("the log reads")
            .into_iter()
            .filter(|row| row.statement_hash == hash)
            .map(|row| (row.decision, row.safe_mode, row.reason))
            .collect()
    })
    .expect("a sink is installed")
}

/// The log's integrity (Koreksi B-1): a refused ANALYZE leaves exactly one row, `refused`, under
/// the connection's own mode; it never leaves an `allowed` or a `confirmed` for a write that did
/// not run. One test, because the sink is the process's: every statement below is its own, so
/// the other tests' decisions, which land in the same log, are never counted.
#[tokio::test]
async fn explain_analyze_of_a_write_is_refused_before_connecting_and_logged_once() {
    let mut storage = qh_storage::Storage::in_memory().expect("in-memory database");
    storage.migrate_at(1_000).expect("migrations");
    qh_ffi::execution_log::install(storage);

    let mut case = 0;
    for template in NOT_FOR_ANALYZE {
        for (mode, confirmed) in [("full", false), ("confirm", true), ("no_ddl", false)] {
            case += 1;
            let sql = template.replace("{}", &format!("{mode}_{case}"));
            let engine = CountingEngine::new();
            let mut extra = vec![
                ("DB_KIND", "postgres"),
                ("DB_HOST", "postgres.invalid"),
                ("EXPLAIN_ANALYZE", "1"),
                ("SAFE_MODE", mode),
                ("SQL", sql.as_str()),
            ];
            if confirmed {
                extra.push(("SAFE_MODE_CONFIRMED", "1"));
            }
            let message = usage_message(&refuse(Command::Explain, &engine, &extra).await);
            assert!(
                message.contains("EXPLAIN ANALYZE runs the statement"),
                "{mode} {sql}: {message}"
            );
            assert_eq!(engine.connects(), 0, "{mode} {sql}");
            let rows = log_rows_for(&sql);
            assert_eq!(rows.len(), 1, "{mode} {sql}: {rows:?}");
            assert_eq!(rows[0].0, "refused", "{mode} {sql}");
            assert_eq!(rows[0].1, mode, "the connection's own mode: {sql}");
            assert!(
                rows[0]
                    .2
                    .as_deref()
                    .is_some_and(|reason| reason.contains("offered for reads only")),
                "{rows:?}"
            );
        }
    }

    // A floor raises the mode, and the row says the mode the run was under.
    let sql = "INSERT INTO w13t2_floor VALUES (1)";
    let engine = CountingEngine::new();
    refuse(
        Command::Explain,
        &engine,
        &[
            ("DB_KIND", "postgres"),
            ("EXPLAIN_ANALYZE", "1"),
            ("SAFE_MODE", "full"),
            ("SAFE_MODE_FLOOR", "read_only"),
            ("SQL", sql),
        ],
    )
    .await;
    let rows = log_rows_for(sql);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(
        (rows[0].0.as_str(), rows[0].1.as_str()),
        ("refused", "read_only")
    );

    // The readings are the server's: a quote the generic reading closes early hides a write
    // from it, and the ANALYZE would have run that write.
    for (kind, sql) in [
        (
            "postgres",
            "SELECT E'w13t2\\' AS a, '; DELETE FROM w13t2_lex; --'",
        ),
        ("mysql", "SELECT 'w13t2\\''; DELETE FROM w13t2_lex; -- '"),
    ] {
        let engine = CountingEngine::new();
        let message = usage_message(
            &refuse(
                Command::Explain,
                &engine,
                &[
                    ("DB_KIND", kind),
                    ("EXPLAIN_ANALYZE", "1"),
                    ("SAFE_MODE", "full"),
                    ("SQL", sql),
                ],
            )
            .await,
        );
        assert!(
            message.contains("offered for reads only"),
            "{kind}: {message}"
        );
        assert_eq!(engine.connects(), 0, "{kind}");
    }

    // Nothing else leaves a row of its own: an unknown format and a driver with no ANALYZE are
    // refused by the checks that need no log, and a read is allowed once by the guard.
    let sql = "SELECT w13t2_no_row FROM t";
    for (extra, needle) in [
        (
            vec![("EXPLAIN_FORMAT", "yaml")],
            "unknown EXPLAIN_FORMAT 'yaml'",
        ),
        (
            vec![("DB_KIND", "mysql"), ("EXPLAIN_ANALYZE", "1")],
            "EXPLAIN ANALYZE is not available on MySQL",
        ),
    ] {
        let engine = CountingEngine::new();
        let mut pairs = vec![("SQL", sql), ("SAFE_MODE", "full")];
        pairs.extend(extra);
        let message = usage_message(&refuse(Command::Explain, &engine, &pairs).await);
        assert!(message.contains(needle), "{message}");
        assert_eq!(engine.connects(), 0);
        assert!(log_rows_for(sql).is_empty(), "{needle}");
    }
    let recorded = record(
        Command::Explain,
        &[
            ("EXPLAIN_ANALYZE", "1"),
            ("SAFE_MODE", "confirm"),
            ("SQL", "SELECT w13t2_read FROM t"),
        ],
    )
    .await;
    recorded.result.as_ref().expect("a read is analysed");
    let rows = log_rows_for("SELECT w13t2_read FROM t");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].0, "allowed");

    qh_ffi::execution_log::uninstall();
}

/// ANALYZE inherits the statement's refusal when the statement is not a read, and a read stays
/// one under the floor: monotone, as ADR-0027 wants.
#[tokio::test]
async fn explain_analyze_is_refused_for_a_function_with_an_effect() {
    for sql in [
        "SELECT nextval('s')",
        "SELECT pg_advisory_lock(1)",
        "SELECT pg_cancel_backend(2)",
    ] {
        let engine = CountingEngine::new();
        let message = usage_message(
            &refuse(
                Command::Explain,
                &engine,
                &[
                    ("DB_KIND", "postgres"),
                    ("EXPLAIN_ANALYZE", "1"),
                    ("SAFE_MODE", "full"),
                    ("SQL", sql),
                ],
            )
            .await,
        );
        assert!(
            message.contains("offered for reads only"),
            "{sql}: {message}"
        );
        assert_eq!(engine.connects(), 0, "{sql}");
    }
}

/// `REQUIRE_READ` is how a re-run of a result says it is a read (AR #1): a statement that is not
/// one is refused before connecting, in `full` too, where nothing else would have stopped it.
#[tokio::test]
async fn require_read_refuses_what_is_not_a_read_in_every_mode() {
    for (sql, why) in [
        ("INSERT INTO w13t2_rr VALUES (1)", "a write"),
        ("SELECT * FROM w13t2_rr FOR UPDATE", "a lock"),
        ("SELECT nextval('w13t2_seq')", "a sequence"),
        ("SELECT pg_terminate_backend(1)", "a signal"),
    ] {
        for mode in ["full", "no_ddl"] {
            // Control: without the setting the mode lets it through to the connection.
            let engine = CountingEngine::new();
            let error = refuse(
                Command::Preview,
                &engine,
                &[("DB_KIND", "postgres"), ("SAFE_MODE", mode), ("SQL", sql)],
            )
            .await;
            assert!(
                matches!(error, CliError::Connect(_)),
                "{why} {mode}: {error:?}"
            );
            assert_eq!(engine.connects(), 1, "{why} {mode}");

            let engine = CountingEngine::new();
            let message = usage_message(
                &refuse(
                    Command::Preview,
                    &engine,
                    &[
                        ("DB_KIND", "postgres"),
                        ("SAFE_MODE", mode),
                        ("REQUIRE_READ", "1"),
                        ("SQL", sql),
                    ],
                )
                .await,
            );
            assert!(message.contains("REQUIRE_READ"), "{why} {mode}: {message}");
            assert_eq!(engine.connects(), 0, "{why} {mode}");
        }
    }
    // A read passes, and so does a run that did not ask.
    let recorded = record(Command::Preview, &[("REQUIRE_READ", "1")]).await;
    recorded.result.as_ref().expect("a read runs");
    assert_eq!(recorded.statements, ["SELECT 1"]);
    // MySQL's own lexical reading applies to the check too.
    let engine = CountingEngine::new();
    let message = usage_message(
        &refuse(
            Command::Preview,
            &engine,
            &[
                ("DB_KIND", "mysql"),
                ("REQUIRE_READ", "1"),
                ("SQL", "SELECT '\\''; DELETE FROM t; -- '"),
            ],
        )
        .await,
    );
    assert!(message.contains("REQUIRE_READ"), "{message}");
    assert_eq!(engine.connects(), 0);
}

/// S-2 / DBX-3: a `SELECT` that calls a function with an effect is a write to every mode that
/// has an opinion about writes. Each of these classified as a read before the denylist, and
/// `read_only` ran them.
#[tokio::test]
async fn a_select_that_calls_a_function_with_an_effect_is_refused_like_a_write() {
    for sql in [
        "SELECT pg_terminate_backend(42)",
        "SELECT pg_cancel_backend(42)",
        "SELECT pg_reload_conf()",
        "SELECT nextval('s')",
        "SELECT setval('s', 1)",
        "SELECT lo_import('/etc/hostname')",
        "SELECT lo_export(1, '/tmp/x')",
        "SELECT pg_advisory_lock(1)",
        "SELECT pg_try_advisory_xact_lock(1)",
    ] {
        // read_only refuses, before the server is reached.
        let engine = CountingEngine::new();
        let message = usage_message(
            &refuse(
                Command::Preview,
                &engine,
                &[
                    ("DB_KIND", "postgres"),
                    ("SAFE_MODE", "read_only"),
                    ("SQL", sql),
                ],
            )
            .await,
        );
        assert!(message.contains("read-only"), "{sql}: {message}");
        assert_eq!(engine.connects(), 0, "{sql}");
        // confirm asks.
        let engine = CountingEngine::new();
        let message = usage_message(
            &refuse(
                Command::Preview,
                &engine,
                &[
                    ("DB_KIND", "postgres"),
                    ("SAFE_MODE", "confirm"),
                    ("SQL", sql),
                ],
            )
            .await,
        );
        assert!(
            message.contains("requires confirmation"),
            "{sql}: {message}"
        );
        assert_eq!(engine.connects(), 0, "{sql}");
        // no_ddl and full let it through to the server, as any other write.
        for mode in ["no_ddl", "full"] {
            let engine = CountingEngine::new();
            let error = refuse(
                Command::Preview,
                &engine,
                &[("DB_KIND", "postgres"), ("SAFE_MODE", mode), ("SQL", sql)],
            )
            .await;
            assert!(
                matches!(error, CliError::Connect(_)),
                "{mode} {sql}: {error:?}"
            );
        }
    }
    for sql in ["SELECT GET_LOCK('a', 1)", "SELECT RELEASE_ALL_LOCKS()"] {
        let engine = CountingEngine::new();
        refuse(
            Command::Preview,
            &engine,
            &[
                ("DB_KIND", "mysql"),
                ("SAFE_MODE", "read_only"),
                ("SQL", sql),
            ],
        )
        .await;
        assert_eq!(engine.connects(), 0, "{sql}");
    }
    // A name that is not a call is still a read.
    let engine = CountingEngine::new();
    let error = refuse(
        Command::Preview,
        &engine,
        &[
            ("DB_KIND", "postgres"),
            ("SAFE_MODE", "read_only"),
            ("SQL", "SELECT nextval FROM t"),
        ],
    )
    .await;
    assert!(matches!(error, CliError::Connect(_)), "{error:?}");
}
