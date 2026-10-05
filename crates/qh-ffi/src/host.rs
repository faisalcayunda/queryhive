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
//! `route` is one `match` with no wildcard, so a command added later does not compile until
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

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use qh_core::EngineError;
use qh_driver::{ConnectionConfig, Driver, DriverKind, Session};

pub use pool::{
    Lane, PoolKey, PoolStats, SessionPool, CHECKIN_WAIT, EVICT_TICK, IDLE_MAX, IDLE_TTL,
    METADATA_SESSIONS, QUERY_SESSIONS, RESET_BUDGET, TUNNEL_OPEN,
};

use crate::env::Settings;
use crate::events::{event, Emitter, StoreEmitter};
use crate::local::SharedStorage;
use crate::store_api::{
    config_for, guarded, metas_of, panic_text, ColumnWire, ResultHandle, StoreFfiError, StorePhase,
    StoreStats, StoreSweep,
};
use crate::uniffi_api::{
    run_with, runtime, settings_of, EngineCommand, EventSink, RunCancel, Setting, SinkEmitter,
};
use crate::{tunnel, Command, Engine, RealEngine};
use qh_result_store::{Outcome, StoreRegistry};

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

    fn keeps_sessions(&self) -> bool {
        true
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
/// It holds the session pool, the local SQLite handle and the result stores' registry, and nothing else. Creating one does no
/// I/O: the pool is empty, SQLite opens on the first local command, and the runtime is built the
/// first time something needs it.
#[derive(uniffi::Object)]
pub struct EngineHost {
    pool: Arc<SessionPool>,
    storage: SharedStorage,
    /// The result stores' registry: absent until `configure_result_stores`, and there is no
    /// implicit default (blueprint fase-6 section 12.3), so a store made before the app has
    /// said where spill goes fails loudly instead of quietly running without it.
    stores: Mutex<Option<Arc<StoreRegistry>>>,
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

    /// Build the result stores' registry once, and sweep the spill directory (blueprint fase-6
    /// section 9.7). Call it off the main thread, before the first store is made. A second call
    /// is an `InvalidArgument`: the budget and the spill directory are fixed for the process.
    pub fn configure_result_stores(
        &self,
        spill_dir: Option<String>,
        budget_bytes: u64,
    ) -> Result<StoreSweep, StoreFfiError> {
        guarded(|| {
            let budget = usize::try_from(budget_bytes)
                .ok()
                .filter(|budget| *budget > 0)
                .ok_or_else(|| StoreFfiError::InvalidArgument {
                    message: "the result store budget must be a positive number of bytes"
                        .to_owned(),
                })?;
            let mut slot = self.stores.lock().map_err(|_| StoreFfiError::Internal {
                message: "the result store registry lock is poisoned".to_owned(),
            })?;
            if slot.is_some() {
                return Err(StoreFfiError::InvalidArgument {
                    message: "result stores are already configured".to_owned(),
                });
            }
            let (registry, sweep) =
                StoreRegistry::new(config_for(budget, spill_dir.map(std::path::PathBuf::from)));
            *slot = Some(registry);
            Ok(StoreSweep {
                removed: sweep.removed,
                spill_enabled: sweep.spill_enabled,
                reason: sweep.reason,
            })
        })
    }

    /// An empty store for the next run, made before the run so the tab can hold its handle
    /// from the start. The columns arrive with the run.
    pub fn create_result_store(&self) -> Result<Arc<ResultHandle>, StoreFfiError> {
        guarded(|| {
            let registry = self.registry()?;
            Ok(ResultHandle::new(registry.create(), registry))
        })
    }

    /// Run `preview` or `explain` into `store` instead of into `rows` events.
    ///
    /// Like `run`: blocks until the command has ended, a failure is an `error` event on `sink`
    /// and never a thrown error, and it does not belong on the main thread. The rows go to the
    /// store; the sink sees `step`, `columns`, `progress` and `done`. The store ends `Complete`,
    /// `Cancelled` or `Failed`, whichever the run did, so a grid polling `row_count` always sees
    /// a terminal phase. Any other command is a usage `error`, and a store that already holds a
    /// run is refused: one store, one run.
    pub fn run_with_store(
        &self,
        command: EngineCommand,
        settings: Vec<Setting>,
        store: Arc<ResultHandle>,
        sink: Arc<dyn EventSink>,
        cancel: Arc<RunCancel>,
    ) {
        let reporter = SinkEmitter::new(Arc::clone(&sink));
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            self.run_into_store(command, settings, &store, sink, &cancel)
        }));
        if let Err(payload) = outcome {
            // Past the guard inside `run_with`: a panic in this wrapper's own code. The store is
            // closed out as failed and the app hears about it the way it hears about any failure.
            let message = format!("internal error: {}", panic_text(&payload));
            let _ = store.writer().finish(Outcome::Failed {
                detail: message.clone(),
            });
            let mut reporter = reporter;
            let _ = reporter.emit(event("error").field("message", message).build());
        }
    }

    /// A store made from rows already in hand: the sample data, tests, a scene snapshot.
    pub fn store_from_rows(
        &self,
        columns: Vec<ColumnWire>,
        rows: Vec<Vec<Option<String>>>,
    ) -> Result<Arc<ResultHandle>, StoreFfiError> {
        guarded(|| {
            let registry = self.registry()?;
            let handle = registry.from_text_rows(metas_of(columns), rows)?;
            Ok(ResultHandle::new(handle, registry))
        })
    }

    /// The registry's numbers, for the diagnostics bundle.
    pub fn store_stats(&self) -> Result<StoreStats, StoreFfiError> {
        guarded(|| {
            let registry = self.registry()?;
            let stats = registry.stats();
            Ok(StoreStats {
                stores: u32::try_from(stats.stores).unwrap_or(u32::MAX),
                resident_bytes: stats.resident_bytes as u64,
                spilled_bytes: stats.spilled_bytes,
                budget_bytes: stats.budget_bytes as u64,
                spill_enabled: registry.cipher().is_some(),
            })
        })
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
            stores: Mutex::new(None),
        })
    }

    /// A finished store of synthetic rows, for `bench_ffi` and the tests: Rust only, not exported.
    ///
    /// Column `k` is a bigint, a double or a short text by `k % 3`, so the store holds the
    /// encodings a real result does. Deterministic in `seed`.
    pub fn store_synthetic(
        &self,
        rows: u32,
        columns: u32,
        seed: u64,
    ) -> Result<Arc<ResultHandle>, StoreFfiError> {
        use qh_core::{ColumnBatch, ColumnMeta, Value};
        guarded(|| {
            let registry = self.registry()?;
            let handle = registry.create();
            let writer = handle.writer();
            writer.begin(
                (0..columns)
                    .map(|k| {
                        ColumnMeta::new(
                            format!("c{k}"),
                            ["bigint", "double", "text"][(k % 3) as usize],
                        )
                    })
                    .collect(),
            )?;
            let mut state = seed | 1;
            let mut next = move || {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state
            };
            let mut done = 0u32;
            while done < rows {
                let take = (rows - done).min(16_384);
                let batch = (0..columns)
                    .map(|k| {
                        (0..take)
                            .map(|row| match k % 3 {
                                0 => Value::Int((next() >> 1) as i64),
                                1 => Value::Float((next() % 1_000_000) as f64 / 100.0),
                                _ => Value::Text(
                                    format!("r{}-{:x}", done + row, next() % 0xffff_ffff).into(),
                                ),
                            })
                            .collect()
                    })
                    .collect();
                writer.push(&ColumnBatch::new(batch).map_err(|error| {
                    StoreFfiError::InvalidArgument {
                        message: error.to_string(),
                    }
                })?)?;
                done += take;
            }
            writer.finish(Outcome::Complete { truncated: false })?;
            Ok(ResultHandle::new(handle, registry))
        })
    }

    fn registry(&self) -> Result<Arc<StoreRegistry>, StoreFfiError> {
        self.stores
            .lock()
            .map_err(|_| StoreFfiError::Internal {
                message: "the result store registry lock is poisoned".to_owned(),
            })?
            .clone()
            .ok_or_else(|| StoreFfiError::InvalidArgument {
                message: "result stores are not configured".to_owned(),
            })
    }

    /// The body of `run_with_store`, inside its panic guard.
    fn run_into_store(
        &self,
        command: EngineCommand,
        settings: Vec<Setting>,
        store: &ResultHandle,
        sink: Arc<dyn EventSink>,
        cancel: &RunCancel,
    ) {
        let mut reporter = SinkEmitter::new(Arc::clone(&sink));
        let mut refuse = |message: &str| {
            let _ = reporter.emit(event("error").field("message", message).build());
        };
        if !matches!(command, EngineCommand::Preview | EngineCommand::Explain) {
            return refuse("a result store takes only preview and explain");
        }
        match store.phase() {
            Some(StorePhase::Empty) => {}
            Some(_) => return refuse("this result store already holds a run"),
            None => return refuse("the result was closed"),
        }
        // The phase check alone is check-then-act: two callers can both see `Empty`.
        if !store.claim_run() {
            return refuse("this result store already holds a run");
        }
        // The sink is the host's to choose: whatever the caller wrote for it is overwritten.
        let mut settings: Vec<Setting> = settings
            .into_iter()
            .filter(|setting| setting.key != "RESULT_SINK")
            .collect();
        settings.push(Setting {
            key: "RESULT_SINK".to_owned(),
            value: "store".to_owned(),
        });
        let settings = settings_of(settings);
        let command = command.as_command();
        let writer = store.writer();
        let mut out = StoreEmitter::new(SinkEmitter::new(sink), writer.clone());
        let engine = self.engine_for(command, &settings);
        run_with(
            command,
            &settings,
            &mut out,
            &*engine,
            cancel.flag(),
            Some(self.storage.clone()),
        );
        // A run that failed or stopped before its first row never reached the pump, so the
        // store is still `Empty`; close it out so no grid waits on a result that will not come.
        if matches!(
            store.phase(),
            Some(StorePhase::Empty | StorePhase::Streaming)
        ) {
            let _ = writer.finish(if cancel.flag().is_cancelled() {
                Outcome::Cancelled
            } else {
                Outcome::Failed {
                    detail: "the run ended before the result was complete".to_owned(),
                }
            });
        }
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
