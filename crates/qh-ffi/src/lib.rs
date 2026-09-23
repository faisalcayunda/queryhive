//! The engine entry point: the fourteen commands, and the CLI the golden harness runs.
//!
//! This is the Rust side of `app/engine/queryhive_engine.py`. The blueprint puts the
//! CLI's destination at `qh-ffi` ("11 perintah CLI | `queryhive_engine.py:950` |
//! `qh-ffi` (via `DatabaseEngine`)"), and it says the CLI itself stays on as a debug
//! binary for the golden harness once the UI moves to the FFI surface. That is what
//! this crate is today: the commands, their event protocol, and the binary. The
//! uniffi surface lands on top of the same commands, so the app and the harness
//! cannot drift into two different engines.
//!
//! # The protocol is the deliverable
//!
//! Each command emits one compact JSON object per stdout line, named by its `event`
//! field. The app decodes them, and `tests/golden/` freezes what the Python engine
//! emitted for a fixed set of cases. Both mean the same thing for anyone working
//! here: **an event's shape and the order of the events are a contract**, and a
//! change to either is a change to the product.
//!
//! | Command | Events |
//! |---|---|
//! | `db_drivers` | `drivers` |
//! | `connections` | `connections` |
//! | `import_connections` | `import` |
//! | `credential` | `credential` |
//! | `test` | `test` |
//! | `catalogs`, `schemas`, `tables` | `catalogs` / `schemas` / `tables`, each `names` |
//! | `objects` | `objects` (`object_columns`, `data`) |
//! | `export` | `step`, `start`, `progress`, `done` |
//! | `to_table` | `step`, `progress`, `done` |
//! | `preview` | `step`, `columns`, `rows`, `done` |
//! | `explain` | `step`, `columns`, `rows`, `done` |
//! | `count` | `step`, `count`, `done` |
//! | any failure | one `error`, exit 1 |
//!
//! Three of the fourteen open no driver at all: `connections`, `import_connections` and
//! `credential` are the local connection store and the password store, and they live in
//! [`local`] rather than beside the driver-facing commands. The Python engine had none of
//! them, so their events are not frozen by the golden harness — the app is their only
//! other reader.
//!
//! # Two things that are deliberately not in a command
//!
//! - **Retries are not written into a command.** The Python engine wrapped its queries
//!   in `QueryStream`'s retry policy, and in this engine that belongs to the session
//!   layer (`docs/architecture/rust-engine-blueprint.md` section 1.7) — so it is
//!   [`retry`], one wrapper around the session and the cursor, and every command that
//!   reads takes its session from there. What is retried, what is not, and why, is the
//!   module note on [`retry`]; the short version is that `RETRIES` now means what it
//!   meant to the previous engine, and a row already written to a file is never fetched
//!   a second time.
//! - **No `to_table` update count.** `Cursor` has no way to ask the server how many
//!   rows a statement affected, so `to_table`'s `done` reports `-1`, which is the
//!   Python engine's own value for "the coordinator never said".
//!
//! # Testability, and why the shape above was chosen
//!
//! The golden harness drives the Python engine **in-process with a fake cursor**, not
//! against a database. To compare the two engines on the same cases, this one has to
//! be drivable the same way, so a command takes its engine and its event sink as
//! arguments:
//!
//! ```no_run
//! use qh_ffi::{run, CancelFlag, Capture, Command, Engine, RealEngine, Settings};
//!
//! # async fn example() -> Result<(), qh_ffi::CliError> {
//! let settings = Settings::from_pairs([("TRINO_HOST", "trino.internal")]);
//! let engine = RealEngine::new();
//! let mut events = Capture::new();
//! run(
//!     Command::DbDrivers,
//!     &settings,
//!     &mut events,
//!     &engine,
//!     &CancelFlag::new(),
//! )
//! .await?;
//! # Ok(())
//! # }
//! ```
//!
//! Nothing in a command reaches for `std::env` or stdout directly, which is also what
//! makes the FFI surface a wrapper rather than a rewrite.

pub mod commands;
pub mod config;
pub mod env;
pub mod events;
pub mod local;
pub mod progress;
pub mod retry;
pub mod sql_ident;
// The scaffolding has to be generated in the crate root: it defines the `UniFfiTag` the
// other derivations name, and the module path it records is the namespace the bindings
// come out under.
uniffi::setup_scaffolding!();

/// The control-plane surface the app links against (ADR-0004). Its own module so the
/// command-line and FFI entry points sit beside each other and share one dispatch.
pub mod uniffi_api;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use qh_core::EngineError;
use qh_driver::{ConnectionConfig, Driver, DriverKind, Session};
use thiserror::Error;

pub use env::{SettingError, Settings};
pub use events::{Capture, Emitter, JsonLines};
pub use progress::Progress;

/// Everything a command can fail with.
///
/// The messages are the ones a user sees in the app's error bar, so they carry no
/// Rust type names and no `Debug` rendering — the same rule
/// [`EngineError::message`] follows.
#[derive(Debug, Error)]
pub enum CliError {
    /// The caller's own request was unusable: a blank statement, a format nobody
    /// offers, a write mode that is not one of the three. Always decided before the
    /// network is touched.
    #[error("{0}")]
    Usage(String),

    /// The connection did not open.
    #[error("{0}")]
    Connect(String),

    /// The server refused the statement, or the connection failed while it ran.
    #[error("{0}")]
    Query(String),

    /// A failure that already changed something the user has to hear about.
    ///
    /// A `replace` write drops the old table before it creates the new one, so a
    /// failure after that has two things to say and only one event to say them in.
    #[error("{message}")]
    Warned {
        message: String,
        warnings: Vec<String>,
    },

    #[error(transparent)]
    Config(#[from] config::ConfigError),

    #[error(transparent)]
    Setting(#[from] SettingError),

    #[error(transparent)]
    Sql(#[from] qh_sql::SqlError),

    #[error(transparent)]
    Export(#[from] qh_export::ExportError),

    /// The local database refused an operation: a migration it will not apply, a file
    /// that is not this engine's, a statement SQLite rejected.
    #[error(transparent)]
    Storage(#[from] qh_storage::StorageError),

    /// The legacy `connections.json` could not be read into rows, copied aside, or
    /// verified.
    #[error(transparent)]
    Import(#[from] qh_storage::import::ImportError),

    /// The password store refused an operation.
    #[error(transparent)]
    Credential(#[from] qh_credentials::CredentialError),

    /// The event stream could not be written — stdout closed, most likely. Not
    /// recoverable here, and not something a retry would fix.
    #[error("{0}")]
    Io(#[from] std::io::Error),

    /// A defect in this program, including a caught panic.
    #[error("internal error: {0}")]
    Internal(String),
}

impl CliError {
    /// What the error event must also report, if anything.
    pub fn warnings(&self) -> &[String] {
        match self {
            CliError::Warned { warnings, .. } => warnings,
            _ => &[],
        }
    }

    /// The message, without this enum's own wording.
    pub fn message(&self) -> String {
        match self {
            CliError::Warned { message, .. } => message.clone(),
            other => other.to_string(),
        }
    }
}

impl From<EngineError> for CliError {
    fn from(error: EngineError) -> Self {
        // The driver's own wording, kept. A UI that shows "connection refused"
        // should show what the server or the socket said, not this crate's summary.
        let message = error.message().to_owned();
        match error {
            EngineError::Usage { .. } => CliError::Usage(message),
            EngineError::Connect { .. } => CliError::Connect(message),
            EngineError::Query { .. } => CliError::Query(message),
            EngineError::StaleHandle => CliError::Internal(message),
            EngineError::Internal { .. } => CliError::Internal(message),
        }
    }
}

/// Set when the user asked the run to stop.
///
/// One process runs one command, so one flag is enough. It is shared with the signal
/// handlers, which is why it is behind an `Arc` rather than a plain `AtomicBool`.
#[derive(Debug, Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }

    /// Route SIGTERM or SIGINT here instead of killing the process: a cancelled
    /// export still gets to emit its `done` and close the files it already wrote.
    pub fn request(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// The drivers this build has, and the way to open one.
///
/// A command never names a driver crate: it asks for the driver of the configured
/// kind. That is what keeps adding SQLite or ClickHouse a matter of registering a
/// crate rather than editing eleven commands — the same reason
/// [`qh_driver::DriverRegistry`] exists one layer down.
#[async_trait]
pub trait Engine: Send + Sync {
    /// The kinds on offer, in the order the UI lists them.
    fn kinds(&self) -> Vec<DriverKind>;

    /// Metadata for one driver. Must not perform I/O.
    fn driver(&self, kind: DriverKind) -> &dyn Driver;

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError>;
}

/// The three drivers, wired together.
pub struct RealEngine {
    trino: qh_driver_trino::TrinoDriver,
    postgres: qh_driver_postgres::PostgresDriver,
    mysql: qh_driver_mysql::MysqlDriver,
}

impl RealEngine {
    pub fn new() -> Self {
        Self {
            trino: qh_driver_trino::TrinoDriver::new(),
            postgres: qh_driver_postgres::PostgresDriver::new(),
            mysql: qh_driver_mysql::MysqlDriver::new(),
        }
    }
}

impl Default for RealEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Engine for RealEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        DriverKind::ALL.to_vec()
    }

    fn driver(&self, kind: DriverKind) -> &dyn Driver {
        match kind {
            DriverKind::Trino => &self.trino,
            DriverKind::Postgres => &self.postgres,
            DriverKind::Mysql => &self.mysql,
        }
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        self.driver(config.kind).connect(config).await
    }
}

/// One command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    DbDrivers,
    Connections,
    ImportConnections,
    Credential,
    Objects,
    Test,
    Catalogs,
    Schemas,
    Tables,
    Export,
    ToTable,
    Preview,
    Count,
    Explain,
}

/// Every command, in the order the usage line lists them.
///
/// The order is a contract, not a convenience: the usage message ends with the
/// browse-and-write commands in a fixed order, and a caller that parses that suffix
/// must keep seeing it. A new command can only be added ahead of `objects` — which is
/// why `objects` sits near the head, ahead of the frozen suffix, rather than beside the
/// other browse commands where it belongs semantically. The three local commands are
/// ahead of it for the same reason.
pub const COMMANDS: [&str; 14] = [
    "db_drivers",
    "connections",
    "import_connections",
    "credential",
    "objects",
    "test",
    "catalogs",
    "schemas",
    "tables",
    "export",
    "to_table",
    "preview",
    "count",
    "explain",
];

impl Command {
    /// The command a name selects, or `None` for a name nobody offers.
    pub fn parse(name: &str) -> Option<Self> {
        let command = match name {
            "db_drivers" => Command::DbDrivers,
            "connections" => Command::Connections,
            "import_connections" => Command::ImportConnections,
            "credential" => Command::Credential,
            "objects" => Command::Objects,
            "test" => Command::Test,
            "catalogs" => Command::Catalogs,
            "schemas" => Command::Schemas,
            "tables" => Command::Tables,
            "export" => Command::Export,
            "to_table" => Command::ToTable,
            "preview" => Command::Preview,
            "count" => Command::Count,
            "explain" => Command::Explain,
            _ => return None,
        };
        Some(command)
    }

    pub fn name(self) -> &'static str {
        COMMANDS[self as usize]
    }
}

/// The one-line usage message, with the suffix the app parses kept verbatim.
pub fn usage(program: &str) -> String {
    format!("usage: {program} {}", COMMANDS.join("|"))
}

/// Run one command, sending its events to `out`.
pub async fn run(
    command: Command,
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
    cancel: &CancelFlag,
) -> Result<(), CliError> {
    match command {
        Command::DbDrivers => commands::db_drivers(out, engine).await,
        Command::Connections => local::connections(settings, out).await,
        Command::ImportConnections => local::import_connections(settings, out).await,
        Command::Credential => local::credential(settings, out).await,
        Command::Objects => commands::objects(settings, out, engine).await,
        Command::Test => commands::test(settings, out, engine).await,
        Command::Catalogs => commands::catalogs(settings, out, engine).await,
        Command::Schemas => commands::schemas(settings, out, engine).await,
        Command::Tables => commands::tables(settings, out, engine).await,
        Command::Export => commands::export(settings, out, engine, cancel).await,
        Command::ToTable => commands::to_table(settings, out, engine, cancel).await,
        Command::Preview => commands::preview(settings, out, engine).await,
        Command::Count => commands::count(settings, out, engine).await,
        Command::Explain => commands::explain(settings, out, engine).await,
    }
}
