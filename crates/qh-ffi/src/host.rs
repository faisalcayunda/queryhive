//! The engine host: the object the app keeps for as long as it runs (blueprint
//! `docs/architecture/blueprints/fase-2-engine-host.md`).
//!
//! Before it, every Run opened its own connection, ran one command and threw the connection away,
//! so the time to first row was a TCP connect, a TLS handshake and an authentication before the
//! statement even started. The host keeps what is expensive across Runs: a small pool of sessions
//! per connection, the SSH tunnel those sessions ride on, and the local SQLite handle.
//!
//! # The pool sits behind the `Engine` trait
//!
//! A command still calls `open()`, then `retry::connect(engine, ..)`, then `session.close()`.
//! What changes is the `engine` the host gives it: its `connect` is a checkout and the `close` of
//! the session it returns is a checkin that resets the session in the background. So
//! `commands.rs`, `apply.rs` and `import.rs` are the code the CLI, the MCP server and the golden
//! harness run, and `crate::run` with a `RealEngine` is exactly what it was.
//!
//! # What goes where
//!
//! [`route`] is one `match` with no wildcard, so a command added later does not compile until
//! someone decides which of these it is:
//!
//! | Route | Commands | Session |
//! |---|---|---|
//! | `Local` | the connection store, history, saved queries, session, account, profiles, `credential` | none; one shared SQLite handle |
//! | `Fresh` | `db_drivers`, `test` | a new one, and a new tunnel: *Test connection* has to prove connect and SSH from nothing |
//! | `Pooled(Metadata)` | the object tree | the pool's metadata lane |
//! | `Pooled(Query)` | preview, explain, count, apply, table operations | the pool's query lanes |
//! | `LongOp` | export, `to_table`, import | one of its own, on the tunnel the key shares |

pub(crate) mod lease;
pub mod pool;

use std::sync::Arc;

use async_trait::async_trait;
use qh_core::EngineError;
use qh_driver::{ConnectionConfig, Driver, DriverKind, Session};

pub use pool::{
    Lane, PoolKey, PoolStats, SessionPool, CHECKIN_WAIT, EVICT_TICK, IDLE_MAX, IDLE_TTL,
    METADATA_SESSIONS, QUERY_SESSIONS, RESET_BUDGET, TUNNEL_OPEN,
};

use crate::env::Settings;
use crate::local::SharedStorage;
use crate::uniffi_api::{
    run_with, runtime, settings_of, EngineCommand, EventSink, RunCancel, Setting, SinkEmitter,
};
use crate::{tunnel, Command, Engine, RealEngine};

/// A tunnel that several sessions ride on.
///
/// A trait so a test can count opens and drops without an SSH server; the real one is
/// [`qh_tunnel::Tunnel`]. Dropping the last `Arc` closes it.
pub trait TunnelHandle: Send + Sync + std::fmt::Debug {
    fn local_port(&self) -> u16;
    /// Whether the bastion connection is still open.
    fn is_alive(&self) -> bool;
}

impl TunnelHandle for qh_tunnel::Tunnel {
    fn local_port(&self) -> u16 {
        qh_tunnel::Tunnel::local_port(self)
    }

    fn is_alive(&self) -> bool {
        qh_tunnel::Tunnel::is_alive(self)
    }
}

/// How the pool opens things: the seam a test replaces to drive the pool without a server.
#[async_trait]
pub trait Connector: Send + Sync {
    /// Metadata for one driver. Must not perform I/O.
    fn driver(&self, kind: DriverKind) -> &dyn Driver;

    /// Open the SSH tunnel `config` describes, towards the database it names.
    async fn open_tunnel(
        &self,
        config: &ConnectionConfig,
        settings: &Settings,
    ) -> Result<Arc<dyn TunnelHandle>, EngineError>;

    /// Open a session. With a `tunnel`, the driver connects to the tunnel's loopback port.
    async fn connect(
        &self,
        config: &ConnectionConfig,
        tunnel: Option<&Arc<dyn TunnelHandle>>,
    ) -> Result<Box<dyn Session>, EngineError>;
}

/// The three real drivers and the real tunnel.
pub struct RealConnector {
    trino: qh_driver_trino::TrinoDriver,
    postgres: qh_driver_postgres::PostgresDriver,
    mysql: qh_driver_mysql::MysqlDriver,
}

impl RealConnector {
    pub fn new() -> Self {
        Self {
            trino: qh_driver_trino::TrinoDriver::new(),
            postgres: qh_driver_postgres::PostgresDriver::new(),
            mysql: qh_driver_mysql::MysqlDriver::new(),
        }
    }
}

impl Default for RealConnector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Connector for RealConnector {
    fn driver(&self, kind: DriverKind) -> &dyn Driver {
        match kind {
            DriverKind::Trino => &self.trino,
            DriverKind::Postgres => &self.postgres,
            DriverKind::Mysql => &self.mysql,
        }
    }

    async fn open_tunnel(
        &self,
        config: &ConnectionConfig,
        settings: &Settings,
    ) -> Result<Arc<dyn TunnelHandle>, EngineError> {
        let description = config.tunnel.as_ref().ok_or_else(|| EngineError::Usage {
            message: "this connection has no SSH tunnel to open".to_owned(),
        })?;
        let target = qh_tunnel::Target::new(config.host.clone(), config.port);
        let opened = tunnel::open(description, settings, target).await?;
        Ok(Arc::new(opened))
    }

    async fn connect(
        &self,
        config: &ConnectionConfig,
        tunnel: Option<&Arc<dyn TunnelHandle>>,
    ) -> Result<Box<dyn Session>, EngineError> {
        let Some(tunnel) = tunnel else {
            return self.driver(config.kind).connect(config).await;
        };
        // The tunnel forwards to the database the config named, so the target was read off the
        // config when the tunnel opened; the driver now talks to the loopback endpoint.
        let mut through = config.clone();
        let _ = crate::tunnel::retarget(&mut through, tunnel.local_port());
        self.driver(config.kind).connect(&through).await
    }
}

/// The engine of a pooled command: `connect` is a checkout.
pub(crate) struct PooledEngine {
    pub(crate) pool: Arc<SessionPool>,
    pub(crate) lane: Lane,
    pub(crate) settings: Settings,
}

#[async_trait]
impl Engine for PooledEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        DriverKind::ALL.to_vec()
    }

    fn driver(&self, kind: DriverKind) -> &dyn Driver {
        self.pool.connector.driver(kind)
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        let held = self
            .pool
            .checkout(config, &self.settings, self.lane)
            .await?;
        Ok(held)
    }
}

/// The engine of a long operation: a session of its own, never pooled, on the shared tunnel.
pub(crate) struct LongOpEngine {
    pub(crate) pool: Arc<SessionPool>,
    pub(crate) settings: Settings,
}

#[async_trait]
impl Engine for LongOpEngine {
    fn kinds(&self) -> Vec<DriverKind> {
        DriverKind::ALL.to_vec()
    }

    fn driver(&self, kind: DriverKind) -> &dyn Driver {
        self.pool.connector.driver(kind)
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        let held = self.pool.open_unleased(config, &self.settings).await?;
        Ok(held)
    }
}

/// Where a command runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Local,
    Fresh,
    Pooled(Lane),
    LongOp,
}

/// The route of every command. No wildcard on purpose: a new command fails to compile until it
/// chooses one.
fn route(command: Command) -> Route {
    match command {
        Command::Connections
        | Command::ImportConnections
        | Command::Credential
        | Command::History
        | Command::HistoryAdd
        | Command::HistoryClear
        | Command::SavedQueries
        | Command::Session
        | Command::Account
        | Command::Profiles
        | Command::ProfileSave
        | Command::ProfileDelete => Route::Local,
        Command::DbDrivers | Command::Test => Route::Fresh,
        Command::Catalogs | Command::Schemas | Command::Tables | Command::Objects => {
            Route::Pooled(Lane::Metadata)
        }
        Command::Preview
        | Command::Explain
        | Command::Count
        | Command::ApplyChanges
        | Command::TableOp => Route::Pooled(Lane::Query),
        Command::Export | Command::ToTable | Command::ImportData => Route::LongOp,
    }
}

/// The engine, for the app's whole lifetime.
///
/// It holds the session pool and the local SQLite handle, and nothing else. Creating one does no
/// I/O: the pool is empty, SQLite opens on the first local command, and the runtime is built the
/// first time something needs it.
#[derive(uniffi::Object)]
pub struct EngineHost {
    pool: Arc<SessionPool>,
    storage: SharedStorage,
}

#[uniffi::export]
impl EngineHost {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Self::with_connector(Arc::new(RealConnector::new()))
    }

    /// Run one command, sending each event to `sink` as it is produced.
    ///
    /// The contract the free `run` had, kept as it was: blocks until the command has ended, events
    /// go to `sink` on the calling thread, and a failure is one more event (`error`), never a
    /// thrown error. `cancel` is the caller's own handle, made before the call, because the
    /// thread blocked here is not the one that presses Stop. Not for the main thread.
    pub fn run(
        &self,
        command: EngineCommand,
        settings: Vec<Setting>,
        sink: Arc<dyn EventSink>,
        cancel: Arc<RunCancel>,
    ) {
        let settings = settings_of(settings);
        let command = command.as_command();
        let mut out = SinkEmitter::new(sink);
        let engine = self.engine_for(command, &settings);
        run_with(
            command,
            &settings,
            &mut out,
            &*engine,
            cancel.flag(),
            Some(self.storage.clone()),
        );
    }

    /// Open one session in the background for the connection these settings describe, when it has
    /// none, so the first Run finds it warm. Returns at once; a failure is dropped and the Run
    /// that follows reports the same error on its normal path. It connects and sends no query.
    pub fn warm_up(&self, settings: Vec<Setting>) {
        let settings = settings_of(settings);
        let engine = RealEngine::with_settings(settings.clone());
        let Ok(config) = crate::commands::connection(&settings, &engine) else {
            return;
        };
        let Ok(runtime) = runtime() else { return };
        // The pool spawns on the current runtime, and this thread is not on one.
        let _entered = runtime.enter();
        self.pool.warm(config, settings);
    }
}

impl EngineHost {
    /// A host over another way of opening sessions: the seam the tests use.
    pub fn with_connector(connector: Arc<dyn Connector>) -> Arc<Self> {
        Arc::new(Self {
            pool: SessionPool::new(connector),
            storage: SharedStorage::default(),
        })
    }

    /// The pool, for `stats()`, `settle()` and the engines the tests drive directly.
    pub fn pool(&self) -> &Arc<SessionPool> {
        &self.pool
    }

    /// How many times the local database has been opened and migrated through this host.
    #[doc(hidden)]
    pub fn storage_opens(&self) -> usize {
        self.storage.opens()
    }

    fn engine_for(&self, command: Command, settings: &Settings) -> Box<dyn Engine> {
        match route(command) {
            // Local commands open no driver; the engine is only there to satisfy the signature.
            Route::Local | Route::Fresh => Box::new(RealEngine::with_settings(settings.clone())),
            Route::Pooled(lane) => self.pool.engine(lane, settings.clone()),
            Route::LongOp => self.pool.long_op_engine(settings.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_needs_a_fresh_connection_is_exactly_the_probe_and_the_driver_list() {
        // "Test connection" must prove connect and SSH from nothing, so it never borrows a
        // pooled session; everything else that opens a driver does. Written out so a route
        // change is a decision someone made on purpose.
        assert_eq!(route(Command::Test), Route::Fresh);
        assert_eq!(route(Command::DbDrivers), Route::Fresh);
        assert_eq!(route(Command::Preview), Route::Pooled(Lane::Query));
        assert_eq!(route(Command::TableOp), Route::Pooled(Lane::Query));
        assert_eq!(route(Command::Tables), Route::Pooled(Lane::Metadata));
        assert_eq!(route(Command::Export), Route::LongOp);
        assert_eq!(route(Command::History), Route::Local);
    }
}
