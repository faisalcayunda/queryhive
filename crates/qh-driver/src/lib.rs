//! The contract every database crate implements, and the registry that finds one.
//!
//! Why the shape is what it is
//! ---------------------------
//! Three things had to be true at once, and each rules something out:
//!
//! 1. **A driver must not be assumed to hold a persistent connection.** Trino is
//!    HTTP: a query is a request, a poll of `nextUri`, and a `DELETE` to cancel.
//!    There is no socket to pool. So the trait has no pool in it — a pool lives
//!    inside the PostgreSQL and MySQL drivers as their own business, and Trino
//!    simply does not have one.
//! 2. **Query text comes from the user, so the shape of a result is not known
//!    until runtime.** There is no `query_as::<T>()` here: a cursor hands back
//!    rows of [`qh_core::Value`], and each driver's job is to turn whatever its
//!    server said into that.
//! 3. **Cancelling has to reach the server.** So `cancel` is on the session and
//!    each driver implements its server's own mechanism — a new connection with
//!    a cancel request for PostgreSQL, `KILL QUERY` for MySQL, `DELETE` on the
//!    query URI for Trino. The Python engine's cancel killed the process and left
//!    the query running; that is the behaviour being replaced.
//!
//! Adding a fourth engine means adding a crate that implements these three
//! traits and registering it. It does not mean touching the result store, the
//! grid, the exporters, or the FFI boundary — which is the point of the whole
//! arrangement.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;

use async_trait::async_trait;
use qh_core::{ColumnBatch, ColumnMeta, EngineError, Value};
use thiserror::Error;

/// Which server a driver speaks to.
///
/// A closed enum rather than a string: the UI switches on it to pick icons,
/// labels and tree levels, and a typo in a configuration file should be caught
/// at load time rather than becoming an unknown driver at connect time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DriverKind {
    Trino,
    Postgres,
    Mysql,
}

impl DriverKind {
    /// The name stored in configuration and sent by the app.
    pub const fn as_str(self) -> &'static str {
        match self {
            DriverKind::Trino => "trino",
            DriverKind::Postgres => "postgres",
            DriverKind::Mysql => "mysql",
        }
    }

    /// Parse the name used in configuration.
    ///
    /// Returns `None` rather than a default: silently treating an unknown kind as
    /// Trino would connect somewhere the user did not ask for.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "trino" => Some(DriverKind::Trino),
            "postgres" | "postgresql" => Some(DriverKind::Postgres),
            "mysql" | "mariadb" => Some(DriverKind::Mysql),
            _ => None,
        }
    }

    /// Every kind this build knows, in the order the UI lists them.
    pub const ALL: [DriverKind; 3] = [DriverKind::Trino, DriverKind::Postgres, DriverKind::Mysql];
}

impl fmt::Display for DriverKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A level of the object tree.
///
/// Not every driver has every level: PostgreSQL has no catalog level (the
/// database is set on the connection), MySQL has no schema level. That is
/// declared through [`Capabilities::levels`] rather than hard-coded in the UI,
/// which is how the Python engine did it too (`exporter/drivers.py`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BrowseLevel {
    Catalog,
    Schema,
    Database,
    Table,
}

impl BrowseLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            BrowseLevel::Catalog => "catalog",
            BrowseLevel::Schema => "schema",
            BrowseLevel::Database => "database",
            BrowseLevel::Table => "table",
        }
    }
}

/// Where in the tree a browse or objects call is aimed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ObjectPath {
    pub catalog: Option<String>,
    pub schema: Option<String>,
    pub table: Option<String>,
}

impl ObjectPath {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn catalog(mut self, catalog: impl Into<String>) -> Self {
        self.catalog = Some(catalog.into());
        self
    }

    pub fn schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = Some(schema.into());
        self
    }

    pub fn table(mut self, table: impl Into<String>) -> Self {
        self.table = Some(table.into());
        self
    }
}

/// What a driver can do, readable **before** connecting.
///
/// Read before the network is touched because the UI needs it to build itself:
/// whether to offer a transaction button, which levels the tree has, which
/// columns the objects grid shows. A capability that could only be discovered
/// after connecting would force the UI to guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub transactions: bool,
    pub multiple_result_sets: bool,
    /// Whether [`Session::cancel`] reaches the server. False would mean cancel
    /// only stops reading locally, which the UI has to say out loud.
    pub cancel: bool,
    pub explain: bool,
    /// Object-tree levels, outermost first.
    pub levels: Vec<BrowseLevel>,
    /// Columns the objects grid reports, in the driver's own order and naming.
    /// Each driver declares its own, the way the Python engine's drivers did.
    pub objects_columns: Vec<String>,
    /// Whether the driver keeps a connection open between statements.
    ///
    /// Trino does not: each query is an HTTP exchange. Anything that assumed a
    /// socket — an idle timeout, a keepalive, a pool — must ask first.
    pub persistent_connection: bool,
}

/// How to run one statement.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExecuteOptions {
    /// Rows per batch handed to the caller. The cursor decides how many rows it
    /// actually fetches per round trip; this is the ceiling on what it returns.
    pub max_batch_rows: Option<usize>,
    /// Stop after this many rows. `None` reads everything.
    ///
    /// The engine stops reading rather than rewriting the statement to add a
    /// `LIMIT`: the row count the grid shows must never change the query that
    /// runs, which is a rule the Python engine kept and the README states.
    pub row_limit: Option<usize>,
}

/// What the objects grid shows for one level.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ObjectsPage {
    /// The driver's own column names, which label the grid.
    pub columns: Vec<String>,
    /// Cells, already as text: this grid is a listing, not data, so every value
    /// is rendered by the driver and a NULL is the empty string.
    pub rows: Vec<Vec<String>>,
}

/// One driver.
///
/// `connect` takes `&self` rather than `&mut self` so a single registered driver
/// can serve many concurrent sessions — which is what a tab per connection needs.
#[async_trait]
pub trait Driver: Send + Sync + 'static {
    fn kind(&self) -> DriverKind;

    /// The name shown in the connection editor and the tree.
    fn label(&self) -> &'static str;

    /// The port to pre-fill when the user has not typed one.
    fn default_port(&self) -> u16;

    /// What this driver can do. Must not perform I/O: the UI calls it for
    /// connections that are not connected.
    fn capabilities(&self) -> Capabilities;

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError>;
}

/// Everything needed to open one connection.
///
/// The password is a plain `String` here for one reason: this type is what a
/// driver receives *after* the secret has been read from the Keychain, and it
/// lives only as long as the connect call. It is never written to a
/// configuration file, a log, or a snapshot. Wrapping it in `secrecy` belongs at
/// this boundary and is on the list for the credentials crate.
///
/// `Debug` is implemented by hand, below, and never prints the password. That is
/// deliberate: `#[derive(Debug)]` would make one `{:?}` in an error path — the
/// easiest leak there is — write a live credential to a log file.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionConfig {
    pub kind: DriverKind,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: Option<String>,
    pub database: Option<String>,
    pub schema: Option<String>,
    pub tls: TlsMode,
    /// Skip certificate verification. Only reachable when the user turned it on
    /// for this connection, and the UI says so.
    pub insecure: bool,
}

impl ConnectionConfig {
    pub fn new(
        kind: DriverKind,
        host: impl Into<String>,
        port: u16,
        user: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            host: host.into(),
            port,
            user: user.into(),
            password: None,
            database: None,
            schema: None,
            tls: TlsMode::Prefer,
            insecure: false,
        }
    }

    pub fn password(mut self, password: impl Into<String>) -> Self {
        self.password = Some(password.into());
        self
    }

    pub fn database(mut self, database: impl Into<String>) -> Self {
        self.database = Some(database.into());
        self
    }

    pub fn schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = Some(schema.into());
        self
    }

    pub fn tls(mut self, tls: TlsMode) -> Self {
        self.tls = tls;
        self
    }

    /// A rendering safe to put in a log or an error message.
    pub fn redacted(&self) -> String {
        format!(
            "{}://{}@{}:{}/{}",
            self.kind,
            self.user,
            self.host,
            self.port,
            self.database.as_deref().unwrap_or("")
        )
    }
}

impl fmt::Debug for ConnectionConfig {
    /// Never prints the password.
    ///
    /// The presence of a password is shown, because whether one was supplied is
    /// what a support conversation needs; its value is not, and a `{:?}` on a
    /// config is the easiest way to write a live credential into a log file.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "ConnectionConfig({}, tls: {:?}, password: {})",
            self.redacted(),
            self.tls,
            if self.password.is_some() {
                "<redacted>"
            } else {
                "none"
            }
        )
    }
}

/// Whether and how to encrypt the connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TlsMode {
    /// Do not use TLS.
    Disable,
    /// Use TLS if the server offers it, plaintext otherwise. The default,
    /// because it matches what every other database client does and a server
    /// that offers TLS is not one you want to talk to in clear.
    #[default]
    Prefer,
    /// Require TLS, and verify the certificate against the system trust store.
    Require,
    /// Require TLS but do not verify. Only when the user asked for it per
    /// connection, and the UI has to warn: this is what an attacker on the
    /// network needs to read everything.
    RequireNoVerify,
}

/// One connection, and the queries run on it.
#[async_trait]
pub trait Session: Send {
    fn capabilities(&self) -> Capabilities;

    /// The server's identity for the work this session ran, once it has one.
    ///
    /// Trino calls it a query id, MySQL a connection id, and PostgreSQL has no
    /// per-statement identity to offer, so it answers `None` — which is a fact about
    /// the server, not a gap here.
    ///
    /// It is **kept after the statement finishes**. A grid keeps showing which query
    /// produced it, and the engines this one replaces reported an id alongside a
    /// finished result; an id that vanished at completion would take that with it.
    /// `None` only means nothing has been sent yet.
    fn query_id(&self) -> Option<String>;

    async fn execute(
        &mut self,
        sql: &str,
        options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError>;

    /// List the children of one level of the object tree.
    ///
    /// `include_system` is the "show all" switch, and it exists because two
    /// drivers hide something by default that a user may legitimately want:
    /// PostgreSQL hides its system schemas, and MySQL hides the databases it keeps
    /// for itself. It is a parameter here rather than a per-driver special case the
    /// caller has to remember, which is how the Python engine arranged it.
    ///
    /// A level a driver genuinely does not have is a usage error naming the driver,
    /// decided **before** anything is opened — not an empty list, which would look
    /// like "you have none".
    async fn browse(
        &mut self,
        level: BrowseLevel,
        path: &ObjectPath,
        include_system: bool,
    ) -> Result<Vec<String>, EngineError>;

    /// Metadata for the objects at one level, for the objects grid.
    async fn objects(&mut self, path: &ObjectPath) -> Result<ObjectsPage, EngineError>;

    /// The statement that asks the server for the plan of `sql`.
    ///
    /// Each driver spells this itself — all three happen to use `EXPLAIN` today,
    /// but MySQL's is a different grammar and Trino's accepts options, so the
    /// spelling belongs with the driver rather than in shared code that would
    /// have to grow a match on the kind.
    fn explain_statement(&self, sql: &str) -> String;

    /// Ask the server to stop the statement in flight.
    ///
    /// Must be idempotent: calling it with nothing running is success, not an
    /// error, because a user can press stop after the query finished.
    async fn cancel(&self) -> Result<(), EngineError>;

    async fn close(self: Box<Self>) -> Result<(), EngineError>;
}

/// Rows, in batches.
///
/// `next_batch` returning `Ok(None)` means the result is finished. An empty
/// batch is **not** the end: a driver may legally return one while it waits.
///
/// ## When `columns` is valid
///
/// When a statement returns rows, `columns` is populated **by the time
/// `next_batch` returns a batch that contains them**. That is the guarantee
/// callers may rely on, and it is deliberately weaker than "valid as soon as
/// `execute` returns", because the stronger promise is not implementable for every
/// server:
///
/// - PostgreSQL and MySQL describe a result set before it runs, so their cursors
///   fill this in during `execute` and it is never empty.
/// - Trino describes it when the result set *begins*, which for a blocking query is
///   when the query *finishes*. Waiting for that inside `execute` would mean
///   `execute` blocked for the whole query, and a caller holding no cursor has
///   nothing to cancel — a defect that was measured, not hypothesised: 2.002 s to
///   `execute` a `SELECT SLEEP(2)`. So a Trino cursor reports an empty slice until
///   its first batch arrives.
///
/// A caller that needs the column list but has no rows yet — a grid drawing its
/// header before data arrives — must therefore tolerate an empty slice and update
/// when the first batch lands. A statement with no result set at all (`DDL`, an
/// `UPDATE`) ends with `Ok(None)` and may leave this empty throughout.
#[async_trait]
pub trait Cursor: Send {
    fn columns(&self) -> &[ColumnMeta];

    async fn next_batch(&mut self, max_rows: usize) -> Result<Option<ColumnBatch>, EngineError>;
}

/// Why a driver could not be found or registered.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RegistryError {
    #[error("no driver is registered for {0}")]
    Unknown(DriverKind),

    #[error("a driver for {kind} is already registered (as {existing:?})")]
    Duplicate { kind: DriverKind, existing: String },
}

/// The drivers this build has, looked up by kind.
///
/// One registry, filled at startup, so the UI never names a driver crate. That
/// is what makes adding SQLite or ClickHouse a matter of registering a crate
/// rather than editing the UI.
#[derive(Default)]
pub struct DriverRegistry {
    drivers: BTreeMap<DriverKind, Box<dyn Driver>>,
}

impl DriverRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a driver. Refuses a second driver for the same kind rather than
    /// letting registration order decide which one runs.
    pub fn register(&mut self, driver: Box<dyn Driver>) -> Result<(), RegistryError> {
        let kind = driver.kind();
        if let Some(existing) = self.drivers.get(&kind) {
            return Err(RegistryError::Duplicate {
                kind,
                existing: existing.label().to_owned(),
            });
        }
        self.drivers.insert(kind, driver);
        Ok(())
    }

    pub fn get(&self, kind: DriverKind) -> Result<&dyn Driver, RegistryError> {
        self.drivers
            .get(&kind)
            .map(AsRef::as_ref)
            .ok_or(RegistryError::Unknown(kind))
    }

    /// Which kinds are available. The connection editor offers exactly these.
    pub fn kinds(&self) -> Vec<DriverKind> {
        self.drivers.keys().copied().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.drivers.is_empty()
    }

    /// The descriptors the UI needs to build the connection editor, without
    /// connecting to anything.
    pub fn descriptors(&self) -> Vec<DriverDescriptor> {
        self.drivers
            .values()
            .map(|driver| DriverDescriptor {
                kind: driver.kind(),
                label: driver.label().to_owned(),
                default_port: driver.default_port(),
                capabilities: driver.capabilities(),
            })
            .collect()
    }
}

/// A driver, described for the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriverDescriptor {
    pub kind: DriverKind,
    pub label: String,
    pub default_port: u16,
    pub capabilities: Capabilities,
}

/// A value a driver could not turn into a [`Value`].
///
/// Kept as a helper rather than a trait method so every driver reports an
/// unmodelled type the same way, and none of them panics on one.
pub fn unsupported_column_value(type_name: &str, bytes: &[u8]) -> Value {
    qh_core::unsupported(type_name, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qh_core::BatchError;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // A driver that does nothing, used to prove the traits are usable and the
    // registry dispatches. It is deliberately tiny: a fake that grows behaviour
    // stops being a contract test and becomes a second implementation.
    struct FakeDriver {
        kind: DriverKind,
        connects: Arc<AtomicUsize>,
    }

    struct FakeSession {
        cancelled: bool,
    }

    struct FakeCursor {
        columns: Vec<ColumnMeta>,
        batches: Vec<ColumnBatch>,
        served: usize,
    }

    #[async_trait]
    impl Driver for FakeDriver {
        fn kind(&self) -> DriverKind {
            self.kind
        }

        fn label(&self) -> &'static str {
            "Fake"
        }

        fn default_port(&self) -> u16 {
            1234
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities {
                transactions: true,
                multiple_result_sets: false,
                cancel: true,
                explain: true,
                levels: vec![BrowseLevel::Schema, BrowseLevel::Table],
                objects_columns: vec!["Name".to_owned()],
                persistent_connection: true,
            }
        }

        async fn connect(
            &self,
            _config: &ConnectionConfig,
        ) -> Result<Box<dyn Session>, EngineError> {
            self.connects.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeSession { cancelled: false }))
        }
    }

    #[async_trait]
    impl Session for FakeSession {
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                transactions: true,
                multiple_result_sets: false,
                cancel: true,
                explain: true,
                levels: vec![BrowseLevel::Schema, BrowseLevel::Table],
                objects_columns: vec!["Name".to_owned()],
                persistent_connection: true,
            }
        }

        fn query_id(&self) -> Option<String> {
            Some("fake-1".to_owned())
        }

        async fn execute(
            &mut self,
            _sql: &str,
            _options: &ExecuteOptions,
        ) -> Result<Box<dyn Cursor>, EngineError> {
            Ok(Box::new(FakeCursor {
                columns: vec![ColumnMeta::new("id", "integer")],
                batches: vec![ColumnBatch::new(vec![vec![Value::Int(1)]]).unwrap()],
                served: 0,
            }))
        }

        async fn browse(
            &mut self,
            _level: BrowseLevel,
            _path: &ObjectPath,
            _include_system: bool,
        ) -> Result<Vec<String>, EngineError> {
            Ok(vec!["public".to_owned()])
        }

        async fn objects(&mut self, _path: &ObjectPath) -> Result<ObjectsPage, EngineError> {
            Ok(ObjectsPage {
                columns: vec!["Name".to_owned()],
                rows: vec![vec!["people".to_owned()]],
            })
        }

        fn explain_statement(&self, sql: &str) -> String {
            format!("EXPLAIN {sql}")
        }

        async fn cancel(&self) -> Result<(), EngineError> {
            Ok(())
        }

        async fn close(self: Box<Self>) -> Result<(), EngineError> {
            let _ = self.cancelled;
            Ok(())
        }
    }

    #[async_trait]
    impl Cursor for FakeCursor {
        fn columns(&self) -> &[ColumnMeta] {
            &self.columns
        }

        async fn next_batch(
            &mut self,
            _max_rows: usize,
        ) -> Result<Option<ColumnBatch>, EngineError> {
            if self.served >= self.batches.len() {
                return Ok(None);
            }
            let batch = self.batches[self.served].clone();
            self.served += 1;
            Ok(Some(batch))
        }
    }

    #[test]
    fn a_kind_round_trips_through_its_name() {
        for kind in DriverKind::ALL {
            assert_eq!(DriverKind::parse(kind.as_str()), Some(kind), "{kind}");
        }
        // The aliases a user might type, and the two MySQL spellings.
        assert_eq!(DriverKind::parse("PostgreSQL"), Some(DriverKind::Postgres));
        assert_eq!(DriverKind::parse(" mariadb "), Some(DriverKind::Mysql));
    }

    #[test]
    fn an_unknown_kind_is_refused_rather_than_defaulted() {
        // Defaulting to Trino would connect somewhere the user did not ask for.
        assert_eq!(DriverKind::parse("oracle"), None);
        assert_eq!(DriverKind::parse(""), None);
    }

    #[test]
    fn the_registry_refuses_a_second_driver_for_one_kind() {
        let mut registry = DriverRegistry::new();
        registry
            .register(Box::new(FakeDriver {
                kind: DriverKind::Postgres,
                connects: Arc::new(AtomicUsize::new(0)),
            }))
            .unwrap();

        let error = registry
            .register(Box::new(FakeDriver {
                kind: DriverKind::Postgres,
                connects: Arc::new(AtomicUsize::new(0)),
            }))
            .unwrap_err();
        // Which driver runs must not depend on registration order.
        assert!(matches!(error, RegistryError::Duplicate { .. }), "{error}");
    }

    #[test]
    fn an_unregistered_kind_is_reported_not_guessed() {
        let registry = DriverRegistry::new();
        assert!(registry.is_empty());
        // Matched rather than `unwrap_err`, because the success type is a trait
        // object and so has no `Debug` for the panic message to use.
        match registry.get(DriverKind::Trino) {
            Ok(driver) => panic!(
                "an empty registry returned {driver_label}",
                driver_label = driver.label()
            ),
            Err(error) => assert_eq!(error, RegistryError::Unknown(DriverKind::Trino)),
        }
    }

    #[test]
    fn descriptors_are_readable_without_connecting() {
        // The connection editor builds itself from these, for connections that
        // do not exist yet — so this must not need a server.
        let mut registry = DriverRegistry::new();
        registry
            .register(Box::new(FakeDriver {
                kind: DriverKind::Trino,
                connects: Arc::new(AtomicUsize::new(0)),
            }))
            .unwrap();

        let descriptors = registry.descriptors();
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].kind, DriverKind::Trino);
        assert_eq!(descriptors[0].default_port, 1234);
        assert_eq!(
            descriptors[0].capabilities.levels,
            vec![BrowseLevel::Schema, BrowseLevel::Table]
        );
    }

    #[tokio::test]
    async fn a_session_streams_batches_until_it_says_it_is_finished() {
        let driver = FakeDriver {
            kind: DriverKind::Mysql,
            connects: Arc::new(AtomicUsize::new(0)),
        };
        let config = ConnectionConfig::new(DriverKind::Mysql, "127.0.0.1", 3306, "qh");
        let mut session = driver.connect(&config).await.unwrap();
        // The driver's own connect ran, not some shortcut in the registry.
        assert_eq!(driver.connects.load(Ordering::SeqCst), 1);

        let mut cursor = session
            .execute("SELECT 1", &ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(cursor.columns().len(), 1);

        let first = cursor.next_batch(10).await.unwrap();
        assert!(first.is_some(), "the first batch was withheld");
        // Then the end, reported as None rather than as an empty batch.
        assert!(cursor.next_batch(10).await.unwrap().is_none());

        session.close().await.unwrap();
    }

    #[test]
    fn a_connection_config_never_prints_its_password() {
        // A `{:?}` on a config is the easiest way to leak a password into a log,
        // so Debug is implemented by hand rather than derived.
        let config = ConnectionConfig::new(DriverKind::Postgres, "db.internal", 5432, "qh")
            .password("hunter2")
            .database("qhd");

        let printed = config.redacted();
        assert!(!printed.contains("hunter2"), "{printed}");
        assert_eq!(printed, "postgres://qh@db.internal:5432/qhd");

        let debugged = format!("{config:?}");
        assert!(!debugged.contains("hunter2"), "{debugged}");
        // That a password exists is still visible: it is what a support
        // conversation needs, and it is not the secret itself.
        assert!(debugged.contains("<redacted>"), "{debugged}");

        let without = ConnectionConfig::new(DriverKind::Postgres, "db.internal", 5432, "qh");
        assert!(format!("{without:?}").contains("password: none"));
    }

    #[test]
    fn tls_defaults_to_prefer_not_to_plaintext() {
        // Defaulting to Disable would silently talk in clear to a server that
        // offered TLS.
        assert_eq!(TlsMode::default(), TlsMode::Prefer);
    }

    #[test]
    fn an_objects_page_is_text_by_contract() {
        // This grid lists objects, not data: a NULL is the empty string, because
        // the grid draws a NULL cell and an empty cell alike.
        let page = ObjectsPage {
            columns: vec!["Name".to_owned(), "Owner".to_owned()],
            rows: vec![vec!["people".to_owned(), String::new()]],
        };
        assert_eq!(page.rows[0].len(), page.columns.len());
    }

    #[test]
    fn a_ragged_batch_is_refused_before_a_driver_can_emit_one() {
        let error = ColumnBatch::new(vec![vec![Value::Int(1)], vec![]]).unwrap_err();
        assert!(matches!(error, BatchError::RaggedColumn { .. }));
    }
}
