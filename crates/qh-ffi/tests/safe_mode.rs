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

/// The dollar-tag text is one dollar-quoted string on PostgreSQL and Trino, so their guard
/// is unchanged: it reads as a single read and reaches the connect step.
#[tokio::test]
async fn a_dollar_quoted_string_is_still_one_read_on_postgres_and_trino() {
    for kind in ["postgres", "trino"] {
        let engine = CountingEngine::new();
        let error = refuse(
            Command::Preview,
            &engine,
            &[
                ("DB_KIND", kind),
                ("SAFE_MODE", "read_only"),
                ("SQL", "SELECT $a$ ; DELETE FROM t ; $a$"),
            ],
        )
        .await;
        assert!(
            !matches!(error, CliError::Usage(ref message) if message.contains("read-only")),
            "{kind}: {error:?}"
        );
        assert_eq!(engine.connects(), 1, "{kind} reaches the connect step");
    }
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
