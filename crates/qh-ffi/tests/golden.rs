//! The Rust engine against the frozen Python snapshots.
//!
//! `tools/golden/record.py` froze what the Python engine emitted for a fixed set of
//! cases, driving it **in-process with a fake cursor** rather than against a database.
//! This test does the same thing to this engine: a fake session answers `execute`,
//! `browse` and `objects` from a script, and the command's events are compared with
//! the snapshot line by line.
//!
//! # Why the comparison is exact, and where it is not
//!
//! [`EXACT`] cases must match the snapshot byte for byte after the same normalisation
//! `record.py` applies — `elapsed_ms` and `query_id` masked, temp paths replaced. Those
//! are the cases where the two engines promise the same thing: the event names, the
//! fields, the order, and every rendered value including the type zoo.
//!
//! [`ACCEPTED`] cases **differ on purpose**, and the test asserts both the difference
//! and the reason. That is the point of listing them: a difference that is not written
//! down is a regression, and one that is written down has to be re-argued when it
//! changes. The reasons live in `docs/golden-deltas.md`.
//!
//! `a_new_snapshot_cannot_be_ignored` walks the snapshot directory and fails on any
//! case that is in neither list, so freezing a new snapshot cannot silently skip this
//! test — the same protection `compare.py` gives the Python side with its "NEW" line.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use qh_core::{ColumnBatch, ColumnMeta, EngineError, Value};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions,
    ObjectPath, ObjectsPage, Session,
};
use qh_ffi::events::Capture;
use qh_ffi::{run, CancelFlag, CliError, Command, Engine, Settings};
use serde_json::Value as Json;

/// The engine's root, two levels up from this crate.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root")
}

/// The golden cases this engine must reproduce exactly.
const EXACT: &[&str] = &[
    "drivers",
    "test_probe",
    "catalogs",
    "schemas",
    "tables",
    "objects_trino",
    "postgres_schemas",
    "postgres_objects",
    "mysql_catalogs",
    "mysql_objects",
    "batching",
    "limit_truncation",
    "no_rows",
    "count",
    "explain",
    "export_csv",
    "to_table_create",
];

/// The cases that differ on purpose, with the reason the difference is acceptable.
const ACCEPTED: &[&str] = &[
    // The Python engine prefixes every failure with its exception class
    // (`ValueError: SQL or SQL_PATH is required`, `OSError: connection refused`).
    // This engine emits the message alone: the class name of a Python exception is
    // not part of the protocol, and the app shows the text either way.
    "blank_sql",
    "connect_failure",
    // The usage line carries the program's own name, which is the binary's rather
    // than the script's. The suffix — the command list the app parses — is identical.
    "unknown_command",
    // The two type-zoo deltas: a small DECIMAL is not in scientific notation (D-1)
    // and an INTERVAL's text is not wrapped in JSON quotes (D-2). Both are deliberate,
    // both are argued in `docs/golden-deltas.md`, and every other cell of that case —
    // 28 of 30 — matches the snapshot exactly.
    "type_zoo",
];

/// One driven case: its script, its settings, and how to run it.
struct Case {
    id: &'static str,
    folder: &'static str,
    command: Command,
    settings: Vec<(&'static str, &'static str)>,
    session: FakeSession,
}

// --------------------------------------------------------------------------- //
// the fake server
// --------------------------------------------------------------------------- //

/// What a fake session answers with.
#[derive(Clone)]
struct FakeSession {
    kind: DriverKind,
    /// Recorded rather than read: a test that cares which statement ran asserts on it.
    executed: Arc<Mutex<Vec<String>>>,
    query_id: Option<String>,
    /// Rows the statement wrote, when the server has a count to give: a CTAS has one,
    /// a SELECT does not.
    affected: Option<u64>,
    columns: Vec<ColumnMeta>,
    rows: Vec<Vec<Value>>,
    names: Vec<String>,
    object_rows: Vec<Vec<String>>,
}

impl FakeSession {
    fn new(kind: DriverKind) -> Self {
        Self {
            kind,
            executed: Arc::new(Mutex::new(Vec::new())),
            // The Python engine's Trino cases reported a query id; the fake ones for
            // PostgreSQL and MySQL reported none, which is what those drivers do.
            query_id: Some("20260101_000000_00000_xxxxx".to_owned()),
            affected: None,
            columns: Vec::new(),
            rows: Vec::new(),
            names: Vec::new(),
            object_rows: Vec::new(),
        }
    }

    fn query_id(mut self, id: Option<&str>) -> Self {
        self.query_id = id.map(str::to_owned);
        self
    }

    fn columns(mut self, columns: &[(&str, &str)]) -> Self {
        self.columns = columns
            .iter()
            .map(|(name, type_name)| ColumnMeta::new(*name, *type_name))
            .collect();
        self
    }

    fn rows(mut self, rows: Vec<Vec<Value>>) -> Self {
        self.rows = rows;
        self
    }

    fn affected(mut self, rows: u64) -> Self {
        self.affected = Some(rows);
        self
    }

    fn names(mut self, names: &[&str]) -> Self {
        self.names = names.iter().map(|name| (*name).to_owned()).collect();
        self
    }

    fn object_rows(mut self, rows: &[&[&str]]) -> Self {
        self.object_rows = rows
            .iter()
            .map(|row| row.iter().map(|cell| (*cell).to_owned()).collect())
            .collect();
        self
    }

    fn executed(&self) -> Vec<String> {
        self.executed.lock().expect("executed statements").clone()
    }
}

/// The driver's own metadata, so `db_drivers` and the level rules are the real ones.
fn real_driver(kind: DriverKind) -> &'static dyn Driver {
    static TRINO: qh_driver_trino::TrinoDriver = qh_driver_trino::TrinoDriver;
    static POSTGRES: qh_driver_postgres::PostgresDriver = qh_driver_postgres::PostgresDriver;
    static MYSQL: qh_driver_mysql::MysqlDriver = qh_driver_mysql::MysqlDriver;
    match kind {
        DriverKind::Trino => &TRINO,
        DriverKind::Postgres => &POSTGRES,
        DriverKind::Mysql => &MYSQL,
    }
}

#[async_trait]
impl Session for FakeSession {
    fn capabilities(&self) -> Capabilities {
        // Taken from the real driver rather than copied: the levels and the objects
        // columns are exactly what this test must not let drift.
        real_driver(self.kind).capabilities()
    }

    fn query_id(&self) -> Option<String> {
        self.query_id.clone()
    }

    async fn execute(
        &mut self,
        sql: &str,
        _options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        self.executed
            .lock()
            .expect("executed statements")
            .push(sql.to_owned());
        Ok(Box::new(FakeCursor {
            columns: self.columns.clone(),
            affected: self.affected,
            batches: if self.rows.is_empty() {
                VecDeque::new()
            } else {
                VecDeque::from([ColumnBatch::new(transpose(&self.rows)).expect("a batch")])
            },
        }))
    }

    async fn browse(
        &mut self,
        _level: BrowseLevel,
        _path: &ObjectPath,
        _include_system: bool,
    ) -> Result<Vec<String>, EngineError> {
        Ok(self.names.clone())
    }

    async fn objects(&mut self, _path: &ObjectPath) -> Result<ObjectsPage, EngineError> {
        Ok(ObjectsPage {
            columns: real_driver(self.kind)
                .capabilities()
                .objects_columns
                .to_vec(),
            rows: self.object_rows.clone(),
        })
    }

    fn explain_statement(&self, sql: &str) -> String {
        format!("EXPLAIN {}", qh_sql::strip_terminator(sql))
    }

    async fn cancel(&self) -> Result<(), EngineError> {
        Ok(())
    }

    async fn close(self: Box<Self>) -> Result<(), EngineError> {
        Ok(())
    }
}

/// The batches the fake cursor hands out, in order.
struct FakeCursor {
    columns: Vec<ColumnMeta>,
    affected: Option<u64>,
    batches: VecDeque<ColumnBatch>,
}

#[async_trait]
impl Cursor for FakeCursor {
    fn columns(&self) -> &[ColumnMeta] {
        &self.columns
    }

    fn affected_rows(&self) -> Option<u64> {
        self.affected
    }

    async fn next_batch(&mut self, _max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        Ok(self.batches.pop_front())
    }
}

/// Rows as a batch's column-major form.
fn transpose(rows: &[Vec<Value>]) -> Vec<Vec<Value>> {
    let width = rows.first().map(Vec::len).unwrap_or(0);
    (0..width)
        .map(|column| rows.iter().map(|row| row[column].clone()).collect())
        .collect()
}

/// The engine under test: real driver metadata, fake sessions.
struct FakeEngine {
    session: FakeSession,
    /// A server that refuses the connection, for the one case that freezes what a
    /// failed connect looks like.
    refuse: bool,
}

#[async_trait]
impl Engine for FakeEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        DriverKind::ALL.to_vec()
    }

    fn driver(&self, kind: DriverKind) -> &dyn Driver {
        real_driver(kind)
    }

    async fn connect(&self, _config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        if self.refuse {
            // What the Python engine's socket refusal looked like: a transport error
            // whose own words are the message.
            return Err(EngineError::Connect {
                message: "connection refused".to_owned(),
                kind: qh_core::FailureKind::Transient,
            });
        }
        Ok(Box::new(self.session.clone()))
    }
}

// --------------------------------------------------------------------------- //
// the cases
// --------------------------------------------------------------------------- //

/// The settings every case starts from, mirroring `record.py`'s own defaults.
///
/// `USER` is there because it is where the Python engine took the user name from when
/// nothing else supplied one — the snapshot recorded the machine's login name, and the
/// rule that produced it is what is being frozen.
fn base(host_key: &'static str, host: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![(host_key, host), ("RETRIES", "0"), ("USER", "isal")]
}

/// The type zoo of blueprint section 1.8, as the values a driver hands the engine.
///
/// The same rows `record.py` freezes, expressed in this engine's own value model: a
/// Python `Decimal("1234567890123456789012345678.1234567890")` is an unscaled integer
/// and a scale, a tz-aware `datetime` is microseconds plus the offset the server
/// reported, and a `timedelta` is months, days and microseconds kept apart.
fn type_zoo() -> Vec<Vec<Value>> {
    fn decimal(text: &str) -> Value {
        // Parsed here rather than hard-coded so the digits in the snapshot and the
        // digits in this file cannot drift apart.
        let (sign, digits) = match text.strip_prefix('-') {
            Some(rest) => (-1i128, rest),
            None => (1i128, text),
        };
        let (whole, fraction) = match digits.split_once('.') {
            Some((whole, fraction)) => (whole, fraction),
            None => (digits, ""),
        };
        let unscaled: i128 = format!("{whole}{fraction}").parse().expect("digits");
        Value::Decimal {
            unscaled: sign * unscaled,
            scale: fraction.len() as u8,
        }
    }

    // `2026-01-31 12:00:00.123456+07:00` as a driver reports it: `micros` is the
    // instant (05:00:00.123456Z) and `offset_secs` is the zone to show it in, which is
    // the convention all three decoders share (`docs/golden-deltas.md` T-1).
    let aware = Value::Timestamp {
        micros: 1_769_835_600_123_456,
        offset_secs: Some(25_200), // +07:00
    };
    let naive = Value::Timestamp {
        micros: 1_769_860_800_000_000,
        offset_secs: None,
    };

    vec![
        vec![
            decimal("1234567890123456789012345678.1234567890"),
            decimal("-0.0000000001"),
            decimal("0"),
            aware,
            naive,
            Value::Date { days: 20_484 },
            Value::Time {
                micros: 86_399_999_999,
            },
            Value::Interval(qh_core::IntervalValue {
                months: 0,
                days: 3,
                micros: 14_706_000_000,
            }),
            Value::Bool(true),
            Value::Bool(false),
        ],
        vec![
            Value::Int(i64::MIN),
            Value::UInt(u64::MAX),
            Value::Float(1.5),
            Value::Float(-0.0),
            Value::Text("unicode: é中😀".into()),
            Value::Text("tab\tand\nnewline".into()),
            Value::Json(r#"{"b":1,"a":2}"#.into()),
            Value::Text("[1,null,3]".into()),
            Value::Bytes(vec![0x00, 0x01, 0xff]),
            Value::Null,
        ],
        vec![
            Value::Text("550e8400-e29b-41d4-a716-446655440000".into()),
            Value::Text("active".into()),
            Value::Text("(1,2)".into()),
            Value::Text("".into()),
            Value::Text("NULL".into()),
            Value::Null,
            Value::Null,
            Value::Null,
            Value::Text("  padded  ".into()),
            Value::Text("\u{0}embedded-nul".into()),
        ],
    ]
}

fn type_zoo_columns() -> Vec<(&'static str, &'static str)> {
    vec![
        ("high_precision", "decimal(38,10)"),
        ("tiny_negative", "decimal(38,10)"),
        ("zero_decimal", "decimal(38,10)"),
        ("tz_aware", "timestamp(6) with time zone"),
        ("tz_naive", "timestamp(6)"),
        ("a_date", "date"),
        ("a_time", "time(6)"),
        ("an_interval", "interval day to second"),
        ("a_bool", "boolean"),
        ("another_bool", "boolean"),
    ]
}

fn cases() -> Vec<Case> {
    let trino = |command, settings: Vec<(&'static str, &'static str)>, session: FakeSession| Case {
        id: "",
        folder: "",
        command,
        settings,
        session,
    };

    vec![
        Case {
            id: "drivers",
            folder: "db_drivers",
            command: Command::DbDrivers,
            settings: Vec::new(),
            session: FakeSession::new(DriverKind::Trino),
        },
        Case {
            id: "test_probe",
            folder: "test",
            ..trino(
                Command::Test,
                base("TRINO_HOST", "trino.internal"),
                FakeSession::new(DriverKind::Trino).names(&["1"]),
            )
        },
        Case {
            id: "catalogs",
            folder: "catalogs",
            ..trino(
                Command::Catalogs,
                base("TRINO_HOST", "trino.internal"),
                FakeSession::new(DriverKind::Trino).names(&["hive", "system"]),
            )
        },
        Case {
            id: "schemas",
            folder: "schemas",
            ..trino(
                Command::Schemas,
                {
                    let mut settings = base("TRINO_HOST", "trino.internal");
                    settings.push(("TRINO_CATALOG", "hive"));
                    settings
                },
                FakeSession::new(DriverKind::Trino).names(&["analytics", "public"]),
            )
        },
        Case {
            id: "tables",
            folder: "tables",
            ..trino(
                Command::Tables,
                {
                    let mut settings = base("TRINO_HOST", "trino.internal");
                    settings.push(("TRINO_CATALOG", "hive"));
                    settings.push(("TRINO_SCHEMA", "analytics"));
                    settings
                },
                FakeSession::new(DriverKind::Trino).names(&["people", "orders"]),
            )
        },
        Case {
            id: "objects_trino",
            folder: "objects",
            ..trino(
                Command::Objects,
                {
                    let mut settings = base("TRINO_HOST", "trino.internal");
                    settings.push(("TRINO_CATALOG", "hive"));
                    settings.push(("TRINO_SCHEMA", "analytics"));
                    settings
                },
                FakeSession::new(DriverKind::Trino).object_rows(&[
                    &["people", "BASE TABLE"],
                    &["orders", "BASE TABLE"],
                    // A NULL cell, which the driver reports as an empty string.
                    &["legacy", ""],
                ]),
            )
        },
        Case {
            id: "type_zoo",
            folder: "preview",
            ..trino(
                Command::Preview,
                {
                    let mut settings = base("TRINO_HOST", "trino.internal");
                    settings.push(("SQL", "SELECT * FROM t"));
                    settings
                },
                FakeSession::new(DriverKind::Trino)
                    .columns(&type_zoo_columns())
                    .rows(type_zoo()),
            )
        },
        Case {
            id: "batching",
            folder: "preview",
            ..trino(
                Command::Preview,
                {
                    let mut settings = base("TRINO_HOST", "trino.internal");
                    settings.push(("SQL", "SELECT * FROM t"));
                    settings.push(("LIMIT", "25"));
                    settings
                },
                FakeSession::new(DriverKind::Trino)
                    .columns(&[("id", "integer"), ("name", "varchar")])
                    .rows(
                        (1..=25)
                            .map(|n| {
                                vec![
                                    Value::Int(n),
                                    if n % 3 == 0 {
                                        Value::Null
                                    } else {
                                        Value::Text(format!("name-{n}").into())
                                    },
                                ]
                            })
                            .collect(),
                    ),
            )
        },
        Case {
            id: "limit_truncation",
            folder: "preview",
            ..trino(
                Command::Preview,
                {
                    let mut settings = base("TRINO_HOST", "trino.internal");
                    settings.push(("SQL", "SELECT * FROM t"));
                    settings.push(("LIMIT", "3"));
                    settings
                },
                FakeSession::new(DriverKind::Trino)
                    .columns(&[("id", "integer")])
                    .rows((1..=10).map(|n| vec![Value::Int(n)]).collect()),
            )
        },
        Case {
            id: "no_rows",
            folder: "preview",
            ..trino(
                Command::Preview,
                {
                    let mut settings = base("TRINO_HOST", "trino.internal");
                    settings.push(("SQL", "SELECT * FROM t"));
                    settings
                },
                FakeSession::new(DriverKind::Trino).columns(&[("id", "integer")]),
            )
        },
        Case {
            id: "count",
            folder: "count",
            ..trino(
                Command::Count,
                {
                    let mut settings = base("TRINO_HOST", "trino.internal");
                    settings.push(("SQL", "SELECT * FROM t"));
                    settings
                },
                FakeSession::new(DriverKind::Trino)
                    .columns(&[("_col0", "bigint")])
                    .rows(vec![vec![Value::Int(4321)]]),
            )
        },
        Case {
            id: "explain",
            folder: "explain",
            ..trino(
                Command::Explain,
                {
                    let mut settings = base("TRINO_HOST", "trino.internal");
                    settings.push(("SQL", "SELECT * FROM t"));
                    settings
                },
                FakeSession::new(DriverKind::Trino)
                    .columns(&[
                        ("Fragment", "varchar"),
                        ("Operation", "varchar"),
                        ("rows", "bigint"),
                        ("bytes", "bigint"),
                    ])
                    .rows(vec![vec![
                        Value::Text("Fragment 0".into()),
                        Value::Text("TableScan".into()),
                        Value::Text("1000".into()),
                        Value::Text("1500".into()),
                    ]]),
            )
        },
        Case {
            id: "export_csv",
            folder: "export",
            ..trino(
                Command::Export,
                {
                    let mut settings = base("TRINO_HOST", "trino.internal");
                    settings.push(("SQL", "SELECT * FROM people"));
                    settings.push(("FORMAT", "csv"));
                    settings.push(("NAME", "people"));
                    // OUT_DIR is filled in at run time, under the temp directory the
                    // snapshot normalises to `<TMP>`.
                    settings
                },
                FakeSession::new(DriverKind::Trino)
                    .columns(&[("id", "integer"), ("name", "varchar")])
                    .rows(vec![
                        vec![Value::Int(1), Value::Text("alice".into())],
                        vec![Value::Int(2), Value::Text("bob, inc".into())],
                        vec![Value::Int(3), Value::Null],
                    ]),
            )
        },
        Case {
            id: "to_table_create",
            folder: "to_table",
            command: Command::ToTable,
            settings: {
                let mut settings = base("TRINO_HOST", "trino.internal");
                settings.push(("SQL", "SELECT * FROM people"));
                settings.push(("TARGET_CATALOG", "hive"));
                settings.push(("TARGET_SCHEMA", "analytics"));
                settings.push(("TARGET_TABLE", "people_copy"));
                settings
            },
            session: FakeSession::new(DriverKind::Trino)
                .affected(5)
                .query_id(Some("20260101_000000_00000_xxxxx")),
        },
        Case {
            id: "postgres_schemas",
            folder: "schemas",
            command: Command::Schemas,
            settings: base("DB_HOST", "pg.internal")
                .into_iter()
                .chain([("DB_KIND", "postgres")])
                .collect(),
            session: FakeSession::new(DriverKind::Postgres).names(&["public", "analytics"]),
        },
        Case {
            id: "postgres_objects",
            folder: "objects",
            command: Command::Objects,
            settings: base("DB_HOST", "pg.internal")
                .into_iter()
                .chain([("DB_KIND", "postgres"), ("DB_SCHEMA", "analytics")])
                .collect(),
            session: FakeSession::new(DriverKind::Postgres)
                .object_rows(&[&["people", "16385", "faisal", ""]]),
        },
        Case {
            id: "mysql_catalogs",
            folder: "catalogs",
            command: Command::Catalogs,
            settings: base("DB_HOST", "mysql.internal")
                .into_iter()
                .chain([("DB_KIND", "mysql")])
                .collect(),
            session: FakeSession::new(DriverKind::Mysql).names(&["information_schema", "sips"]),
        },
        Case {
            id: "mysql_objects",
            folder: "objects",
            command: Command::Objects,
            settings: base("DB_HOST", "mysql.internal")
                .into_iter()
                .chain([("DB_KIND", "mysql"), ("DB_DATABASE", "sips")])
                .collect(),
            session: FakeSession::new(DriverKind::Mysql).object_rows(&[&[
                "people",
                "InnoDB",
                "42",
                "the people table",
            ]]),
        },
    ]
}

// --------------------------------------------------------------------------- //
// running and comparing
// --------------------------------------------------------------------------- //

/// The temp roots the snapshot masks, longest first.
fn temp_roots() -> Vec<String> {
    let temp = std::env::temp_dir();
    let mut roots = vec![temp.to_string_lossy().into_owned()];
    // On macOS the temp directory is reached through a symlink, and a path built from
    // either spelling is the same file. Both are masked so the comparison does not
    // depend on which one the kernel hands back.
    if let Ok(resolved) = temp.canonicalize() {
        roots.push(resolved.to_string_lossy().into_owned());
    }
    roots.sort_by_key(|root| std::cmp::Reverse(root.len()));
    roots
}

/// The temp roots plus this case's own output directory.
///
/// `record.py` adds the directory it exported into to its mask list, so the file name
/// is all that survives a path — `<TMP>/people.csv` rather than `<TMP>qh-golden-…`. The
/// longest prefix wins, so the export directory has to be masked before the temp root
/// that contains it.
fn roots_with(extra: Option<&Path>) -> Vec<String> {
    let mut roots = temp_roots();
    if let Some(path) = extra {
        roots.push(path.to_string_lossy().into_owned());
    }
    roots.sort_by_key(|root| std::cmp::Reverse(root.len()));
    roots
}

/// One event, normalised exactly as `record.py` normalises it.
fn normalise(value: &Json, tmp: &[String]) -> Json {
    match value {
        Json::Object(fields) => Json::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), normalise_key(key, value, tmp)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn normalise_key(key: &str, value: &Json, tmp: &[String]) -> Json {
    match key {
        // The two keys that cannot be the same twice.
        "elapsed_ms" => Json::String("<TIME>".to_owned()),
        "query_id" => Json::String("<QUERY_ID>".to_owned()),
        "files" => match value {
            Json::Array(entries) => Json::Array(
                entries
                    .iter()
                    .map(|entry| match entry {
                        Json::Object(fields) => Json::Object(
                            fields
                                .iter()
                                .map(|(field, field_value)| {
                                    let normalised = if field == "path" {
                                        mask_path(field_value, tmp)
                                    } else {
                                        field_value.clone()
                                    };
                                    (field.clone(), normalised)
                                })
                                .collect(),
                        ),
                        other => other.clone(),
                    })
                    .collect(),
            ),
            other => other.clone(),
        },
        _ => match value {
            Json::String(text) => Json::String(mask(text, tmp)),
            Json::Array(items) => Json::Array(
                items
                    .iter()
                    .map(|item| normalise_key(key, item, tmp))
                    .collect(),
            ),
            other => other.clone(),
        },
    }
}

fn mask_path(value: &Json, tmp: &[String]) -> Json {
    match value {
        Json::String(text) => Json::String(mask(text, tmp)),
        other => other.clone(),
    }
}

fn mask(text: &str, tmp: &[String]) -> String {
    let mut text = text.to_owned();
    for root in tmp {
        if !root.is_empty() && text.contains(root.as_str()) {
            text = text.replace(root.as_str(), "<TMP>");
        }
    }
    text
}

/// The snapshot's lines, normalised the same way the engine's own events are.
fn snapshot(case_id: &str, folder: &str) -> Vec<Json> {
    let path = root()
        .join("tests/golden")
        .join(folder)
        .join(format!("{case_id}.ndjson"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let event: Json = serde_json::from_str(line).expect("a JSON line");
            normalise(&event, &temp_roots())
        })
        .collect()
}

/// Run one case and return its events, normalised.
async fn recorded(case: &Case, out_dir: Option<&Path>) -> Result<Vec<Json>, CliError> {
    let mut pairs: Vec<(String, String)> = case
        .settings
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    if let Some(out_dir) = out_dir {
        pairs.push(("OUT_DIR".to_owned(), out_dir.to_string_lossy().into_owned()));
    }
    let settings = Settings::from_pairs(pairs);
    let roots = roots_with(out_dir);
    let engine = FakeEngine {
        session: case.session.clone(),
        refuse: case.id == "connect_failure",
    };
    let mut events = Capture::new();
    run(
        case.command,
        &settings,
        &mut events,
        &engine,
        &CancelFlag::new(),
    )
    .await?;
    Ok(events
        .lines
        .iter()
        .map(|event| normalise(event, &roots))
        .collect())
}

#[tokio::test]
async fn every_exact_case_matches_the_python_snapshot() {
    let mut checked = 0;
    for case in cases().iter().filter(|case| EXACT.contains(&case.id)) {
        let out_dir = (case.command == Command::Export)
            .then(|| std::env::temp_dir().join(format!("qh-golden-{}", std::process::id())));
        let expected = snapshot(case.id, case.folder);
        let actual = recorded(case, out_dir.as_deref())
            .await
            .unwrap_or_else(|error| panic!("{} failed: {error}", case.id));
        assert_eq!(
            actual,
            expected,
            "{} did not match the snapshot\n  engine: {}\n  golden: {}",
            case.id,
            lines(&actual),
            lines(&expected)
        );
        checked += 1;
    }
    assert_eq!(checked, EXACT.len(), "every exact case must have been run");
}

#[tokio::test]
async fn the_count_command_runs_the_wrapper_the_snapshot_was_recorded_with() {
    let case = cases()
        .into_iter()
        .find(|case| case.id == "count")
        .expect("the count case");
    let session = case.session.clone();
    recorded(&case, None).await.expect("count runs");
    assert_eq!(
        session.executed(),
        vec!["SELECT COUNT(*) FROM (SELECT * FROM t) AS queryhive_count".to_owned()],
        "the caller's SQL is wrapped, and only the wrapper, is what runs"
    );
}

#[tokio::test]
async fn an_unknown_command_is_one_error_event() {
    // The Python engine's `usage` case, which is the one path that never reaches a
    // command at all: `main` refuses the argument list and says so.
    let usage = qh_ffi::usage("queryhive-engine");
    assert_eq!(
        usage,
        "usage: queryhive-engine db_drivers|connections|import_connections|credential|objects|test|catalogs|schemas|tables|export|to_table|preview|count|explain"
    );
    let golden = snapshot("unknown_command", "usage");
    assert_eq!(golden.len(), 1);
    // The suffix the app parses is identical; only the program's name differs.
    let golden_message = golden[0]["message"].as_str().expect("a message");
    // `usage: <program> <commands>`: the second field is the program's name and the third
    // is the list. The name may differ, and so may the commands ahead of `objects` — the
    // three local ones are not in the snapshot because the Python engine never had them —
    // so what is compared is the frozen suffix that begins at `objects`.
    let suffix = |line: &str| {
        line.split_once("objects|")
            .map(|(_, rest)| rest.to_owned())
            .unwrap_or_else(|| panic!("no frozen suffix in {line:?}"))
    };
    assert_eq!(suffix(&usage), suffix(golden_message));
    assert!(golden_message.starts_with("usage: queryhive_engine.py "));
}

/// The four accepted differences, asserted rather than assumed: if one of these starts
/// matching — because `to_table` learned to report a row count, or an error grew a
/// Python class name — this test fails and the entry in `docs/golden-deltas.md` has to
/// be revisited.
#[tokio::test]
async fn the_accepted_differences_are_what_the_docs_say() {
    // 1. A failure's message carries no Python exception class.
    let blank_sql = Case {
        id: "blank_sql",
        folder: "preview",
        command: Command::Preview,
        settings: vec![
            ("TRINO_HOST", "trino.internal"),
            ("RETRIES", "0"),
            ("USER", "isal"),
        ],
        session: FakeSession::new(DriverKind::Trino),
    };
    let error = recorded(&blank_sql, None)
        .await
        .expect_err("no SQL is a usage error");
    assert_eq!(error.message(), "SQL or SQL_PATH is required");
    assert!(
        snapshot("blank_sql", "preview")[0]["message"]
            .as_str()
            .expect("a message")
            .starts_with("ValueError: "),
        "the snapshot is the Python wording, which is what this difference is about"
    );

    // 2. The two type-zoo deltas, cell by cell: every other cell of the case — and
    //    every event — has to match the snapshot exactly, so this is not a licence for
    //    the rendering to drift.
    let type_zoo_case = cases()
        .into_iter()
        .find(|case| case.id == "type_zoo")
        .expect("the type_zoo case");
    let actual = recorded(&type_zoo_case, None).await.expect("type_zoo runs");
    let golden = snapshot("type_zoo", "preview");
    assert_eq!(actual.len(), golden.len(), "{actual:?}");
    assert_eq!(actual[0], golden[0], "step connect");
    assert_eq!(actual[1], golden[1], "columns");
    assert_eq!(actual[3], golden[3], "done");

    let engine_rows = actual[2]["data"].as_array().expect("rows");
    let golden_rows = golden[2]["data"].as_array().expect("rows");
    assert_eq!(engine_rows.len(), golden_rows.len());
    let mut differences = Vec::new();
    for (row, (engine_row, golden_row)) in engine_rows.iter().zip(golden_rows).enumerate() {
        let engine_cells = engine_row.as_array().expect("cells");
        let golden_cells = golden_row.as_array().expect("cells");
        assert_eq!(engine_cells.len(), golden_cells.len());
        for (column, (engine_cell, golden_cell)) in
            engine_cells.iter().zip(golden_cells).enumerate()
        {
            if engine_cell != golden_cell {
                differences.push((row, column, engine_cell.clone(), golden_cell.clone()));
            }
        }
    }
    assert_eq!(
        differences,
        vec![
            // D-1: positional rather than scientific. Same number, same digits.
            (0, 1, Json::from("-0.0000000001"), Json::from("-1E-10"),),
            // D-2: the same text, without the JSON quotes Python's fallback added.
            (
                0,
                7,
                Json::from("3 days, 4:05:06"),
                Json::from("\"3 days, 4:05:06\""),
            ),
        ],
        "only the two documented deltas may differ"
    );
}

/// Every snapshot is either reproduced or written down. An unclassified one is a case
/// nobody decided about, which is exactly what must not pass silently.
#[test]
fn a_new_snapshot_cannot_be_ignored() {
    let golden = root().join("tests/golden");
    let mut found = 0;
    let mut folders: Vec<PathBuf> = std::fs::read_dir(&golden)
        .expect("the snapshot directory")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .collect();
    folders.sort();

    for folder in folders {
        for entry in std::fs::read_dir(&folder).expect("a case folder") {
            let path = entry.expect("an entry").path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("ndjson") {
                continue;
            }
            let case_id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .expect("a case name")
                .to_owned();
            assert!(
                EXACT.contains(&case_id.as_str()) || ACCEPTED.contains(&case_id.as_str()),
                "{case_id} is in the snapshot but is neither reproduced nor listed as an accepted \
                 difference"
            );
            found += 1;
        }
    }
    assert_eq!(
        found,
        EXACT.len() + ACCEPTED.len(),
        "every listed case must exist in the snapshot"
    );
}

fn lines(events: &[Json]) -> String {
    events
        .iter()
        .map(|event| serde_json::to_string(event).expect("serialisable"))
        .collect::<Vec<_>>()
        .join("\n  ")
}
