//! The engine host and its session pool (W3-T1, blueprint `fase-2-engine-host.md` §13).
//!
//! Two kinds of test live here:
//!
//! * **Offline.** A `Connector` that hands out scripted sessions, so the pool's rules
//!   (reuse, reset at checkin, the 2+1 reservation, credential versions, cancelled sessions,
//!   the read-only reconnect rule, the shared tunnel) are checked without a server.
//! * **Live.** The same lease API over a real server, for what only a server can show: that a
//!   `search_path`, an open transaction, a session variable or a statement bound set by one
//!   Run is gone in the next Run on the *same* backend.
//!
//! ```bash
//! QH_TEST_POSTGRES=1 QH_TEST_MYSQL=1 QH_TEST_TRINO=1 cargo test -p qh-ffi --test host
//! QH_TEST_SSH=1 QH_TEST_POSTGRES=1 cargo test -p qh-ffi --test host ssh_
//! ```
//!
//! A live test without its guard prints why it is skipping and returns, for the reason the
//! driver suites give: a green run that tested nothing is worse than a visible skip.

use std::collections::HashSet;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use qh_core::{ColumnBatch, ColumnMeta, EngineError, FailureKind, Value};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions,
    ObjectPath, ObjectsPage, Session, TlsMode, TunnelAuth, TunnelConfig,
};
use qh_ffi::host::{Connector, EngineHost, Lane, PoolKey, TunnelHandle, CHECKIN_WAIT};
use qh_ffi::retry::{self, RetryPolicy};
use qh_ffi::uniffi_api::{EngineCommand, EventSink, RunCancel, Setting};
use qh_ffi::{CancelFlag, Capture, Command, Engine, Settings};
use serde_json::Value as Json;

// --------------------------------------------------------------------------- //
// helpers
// --------------------------------------------------------------------------- //

fn block<F: Future>(work: F) -> F::Output {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("a test runtime")
        })
        .block_on(work)
}

fn settings(pairs: &[(&str, &str)]) -> Settings {
    Settings::from_pairs(pairs.iter().map(|(key, value)| (*key, *value)))
}

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);

impl Recorder {
    fn events(&self) -> Vec<Json> {
        self.0
            .lock()
            .expect("recorded lines")
            .iter()
            .map(|line| serde_json::from_str(line).expect("an event is JSON"))
            .collect()
    }
}

impl EventSink for Recorder {
    fn on_event(&self, line: String) {
        self.0.lock().expect("recorded lines").push(line);
    }
}

/// A database of the tests' own, for a run that names none. The host writes its Safe Mode decisions
/// to the database the run names, and the default one is the app's: a test must not open it.
const TEST_DATABASE: &str = concat!(env!("CARGO_TARGET_TMPDIR"), "/host-tests.sqlite3");

/// `pairs` as the host takes them, with a `DB_PATH` of the tests' own when the caller set none.
fn host_settings(pairs: &[(&str, &str)]) -> Vec<Setting> {
    let mut settings: Vec<Setting> = pairs
        .iter()
        .map(|(key, value)| Setting {
            key: (*key).to_owned(),
            value: (*value).to_owned(),
        })
        .collect();
    if !pairs.iter().any(|(key, _)| *key == "DB_PATH") {
        settings.push(Setting {
            key: "DB_PATH".to_owned(),
            value: TEST_DATABASE.to_owned(),
        });
    }
    settings
}

/// Run one command through the host and return what its sink was handed.
fn run_host(host: &EngineHost, command: EngineCommand, pairs: &[(&str, &str)]) -> Vec<Json> {
    let sink = Recorder::default();
    host.run(
        command,
        host_settings(pairs),
        Arc::new(sink.clone()),
        RunCancel::new(),
    );
    sink.events()
}

fn last_event<'a>(events: &'a [Json], name: &str) -> &'a Json {
    events
        .iter()
        .rev()
        .find(|event| event["event"] == name)
        .unwrap_or_else(|| panic!("no `{name}` event in {events:?}"))
}

fn without_timing(events: Vec<Json>) -> Vec<Json> {
    events
        .into_iter()
        .map(|mut event| {
            if let Some(object) = event.as_object_mut() {
                object.remove("elapsed_ms");
            }
            event
        })
        .collect()
}

fn pg_config(password: &str) -> ConnectionConfig {
    ConnectionConfig::new(DriverKind::Postgres, "db.example", 5432, "u")
        .password(password)
        .database("d")
        .tls(TlsMode::Disable)
}

// --------------------------------------------------------------------------- //
// the fake server side
// --------------------------------------------------------------------------- //

/// What the fake connector hands out and what happened to it, shared with the test.
#[derive(Default)]
struct World {
    next_id: AtomicUsize,
    connects: AtomicUsize,
    tunnels_opened: AtomicUsize,
    tunnels_dropped: Arc<AtomicUsize>,
    sessions: Mutex<Vec<Arc<Probe>>>,
    tunnels: Mutex<Vec<Arc<TunnelState>>>,
    /// Sessions whose next `execute` fails as a dead socket would, before anything is sent.
    stale: Mutex<HashSet<usize>>,
    /// `(needle, error)`: a statement containing the needle fails at `execute`.
    fail_execute: Mutex<Vec<(String, EngineError)>>,
    /// `(needle, error)`: a statement containing the needle fails after its first batch.
    fail_stream: Mutex<Vec<(String, EngineError)>>,
    connect_delay: Mutex<Duration>,
    reset_delay: Mutex<Duration>,
    /// `(needle, row)`: a statement containing the needle is answered with one row of text cells
    /// instead of the numbers every other statement gets.
    canned: Mutex<Vec<(String, Vec<Option<String>>)>>,
}

#[derive(Default)]
struct Probe {
    id: usize,
    password: Option<String>,
    tunnel_port: Option<u16>,
    executed: Mutex<Vec<String>>,
    resets_started: AtomicUsize,
    resets_done: AtomicUsize,
    cancels: AtomicUsize,
    contexts: Mutex<Vec<(Option<String>, Option<String>)>>,
    closed: AtomicBool,
}

#[derive(Debug)]
struct TunnelState {
    port: u16,
    alive: AtomicBool,
}

#[derive(Debug)]
struct FakeTunnel {
    state: Arc<TunnelState>,
    dropped: Arc<AtomicUsize>,
}

impl TunnelHandle for FakeTunnel {
    fn local_port(&self) -> u16 {
        self.state.port
    }
    fn is_alive(&self) -> bool {
        self.state.alive.load(Ordering::SeqCst)
    }
}

impl Drop for FakeTunnel {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

impl World {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn connects(&self) -> usize {
        self.connects.load(Ordering::SeqCst)
    }

    fn probe(&self, index: usize) -> Arc<Probe> {
        Arc::clone(&self.sessions.lock().expect("sessions")[index])
    }

    fn all_executed(&self) -> Vec<String> {
        self.sessions
            .lock()
            .expect("sessions")
            .iter()
            .flat_map(|probe| probe.executed.lock().expect("executed").clone())
            .collect()
    }

    fn total(&self, count: impl Fn(&Probe) -> usize) -> usize {
        self.sessions
            .lock()
            .expect("sessions")
            .iter()
            .map(|probe| count(probe))
            .sum()
    }
}

struct FakeConnector(Arc<World>);

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
impl Connector for FakeConnector {
    fn driver(&self, kind: DriverKind) -> &dyn Driver {
        real_driver(kind)
    }

    async fn open_tunnel(
        &self,
        _config: &ConnectionConfig,
        _settings: &Settings,
    ) -> Result<Arc<dyn TunnelHandle>, EngineError> {
        let n = self.0.tunnels_opened.fetch_add(1, Ordering::SeqCst);
        let state = Arc::new(TunnelState {
            port: 40_000 + n as u16,
            alive: AtomicBool::new(true),
        });
        self.0
            .tunnels
            .lock()
            .expect("tunnels")
            .push(Arc::clone(&state));
        Ok(Arc::new(FakeTunnel {
            state,
            dropped: Arc::clone(&self.0.tunnels_dropped),
        }))
    }

    async fn connect(
        &self,
        config: &ConnectionConfig,
        tunnel: Option<&Arc<dyn TunnelHandle>>,
    ) -> Result<Box<dyn Session>, EngineError> {
        let delay = *self.0.connect_delay.lock().expect("delay");
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        self.0.connects.fetch_add(1, Ordering::SeqCst);
        let id = self.0.next_id.fetch_add(1, Ordering::SeqCst);
        let probe = Arc::new(Probe {
            id,
            password: config.password.clone(),
            tunnel_port: tunnel.map(|tunnel| tunnel.local_port()),
            ..Probe::default()
        });
        self.0
            .sessions
            .lock()
            .expect("sessions")
            .push(Arc::clone(&probe));
        Ok(Box::new(FakeSession {
            world: Arc::clone(&self.0),
            probe,
            kind: config.kind,
        }))
    }
}

struct FakeSession {
    world: Arc<World>,
    probe: Arc<Probe>,
    kind: DriverKind,
}

fn stale_error() -> EngineError {
    EngineError::Connect {
        message: "connection reset by peer".to_owned(),
        kind: FailureKind::Transient,
    }
}

#[async_trait]
impl Session for FakeSession {
    fn capabilities(&self) -> Capabilities {
        real_driver(self.kind).capabilities()
    }

    fn query_id(&self) -> Option<String> {
        None
    }

    async fn execute(
        &mut self,
        sql: &str,
        options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        if self
            .world
            .stale
            .lock()
            .expect("stale")
            .contains(&self.probe.id)
        {
            return Err(stale_error());
        }
        self.probe
            .executed
            .lock()
            .expect("executed")
            .push(sql.to_owned());
        for (needle, error) in self.world.fail_execute.lock().expect("rules").iter() {
            if sql.contains(needle.as_str()) {
                return Err(error.clone());
            }
        }
        let fail_stream = self
            .world
            .fail_stream
            .lock()
            .expect("rules")
            .iter()
            .find(|(needle, _)| sql.contains(needle.as_str()))
            .map(|(_, error)| error.clone());
        let canned = self
            .world
            .canned
            .lock()
            .expect("canned")
            .iter()
            .find(|(needle, _)| sql.contains(needle.as_str()))
            .map(|(_, row)| row.clone());
        if let Some(row) = canned {
            return Ok(Box::new(FakeCursor {
                columns: (0..row.len())
                    .map(|index| ColumnMeta::new(format!("c{index}"), "text"))
                    .collect(),
                batches: vec![ColumnBatch::new(
                    row.iter()
                        .map(|cell| {
                            vec![cell
                                .as_deref()
                                .map_or(Value::Null, |text| Value::Text(text.into()))]
                        })
                        .collect(),
                )
                .expect("a batch")],
                fail_after_first: None,
                served: 0,
            }));
        }
        let rows = options.row_limit.map_or(3, |limit| limit.min(3));
        Ok(Box::new(FakeCursor {
            columns: vec![ColumnMeta::new("n", "int")],
            batches: vec![
                ColumnBatch::new(vec![(1..=rows as i64).map(Value::Int).collect()])
                    .expect("a batch"),
            ],
            fail_after_first: fail_stream,
            served: 0,
        }))
    }

    async fn browse(
        &mut self,
        _level: BrowseLevel,
        _path: &ObjectPath,
        _include_system: bool,
    ) -> Result<Vec<String>, EngineError> {
        if self
            .world
            .stale
            .lock()
            .expect("stale")
            .contains(&self.probe.id)
        {
            return Err(stale_error());
        }
        Ok(vec!["a".to_owned(), "b".to_owned()])
    }

    async fn objects(&mut self, _path: &ObjectPath) -> Result<ObjectsPage, EngineError> {
        Ok(ObjectsPage {
            columns: real_driver(self.kind)
                .capabilities()
                .objects_columns
                .to_vec(),
            rows: vec![vec!["t".to_owned(); 4]],
        })
    }

    fn explain_statement(&self, sql: &str) -> String {
        format!("EXPLAIN {sql}")
    }

    async fn cancel(&self) -> Result<(), EngineError> {
        self.probe.cancels.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn reset(&mut self) -> Result<(), EngineError> {
        self.probe.resets_started.fetch_add(1, Ordering::SeqCst);
        let delay = *self.world.reset_delay.lock().expect("delay");
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        self.probe.resets_done.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn set_context(&mut self, database: Option<&str>, schema: Option<&str>) {
        self.probe
            .contexts
            .lock()
            .expect("contexts")
            .push((database.map(str::to_owned), schema.map(str::to_owned)));
    }

    async fn close(self: Box<Self>) -> Result<(), EngineError> {
        self.probe.closed.store(true, Ordering::SeqCst);
        Ok(())
    }
}

struct FakeCursor {
    columns: Vec<ColumnMeta>,
    batches: Vec<ColumnBatch>,
    fail_after_first: Option<EngineError>,
    served: usize,
}

#[async_trait]
impl Cursor for FakeCursor {
    fn columns(&self) -> &[ColumnMeta] {
        &self.columns
    }

    async fn next_batch(&mut self, _max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        if self.served == 1 {
            if let Some(error) = self.fail_after_first.take() {
                return Err(error);
            }
        }
        if self.batches.is_empty() {
            return Ok(None);
        }
        self.served += 1;
        Ok(Some(self.batches.remove(0)))
    }
}

/// A host over the fake connector.
fn fake_host() -> (Arc<World>, Arc<EngineHost>) {
    let world = World::new();
    let host = EngineHost::with_connector(Arc::new(FakeConnector(Arc::clone(&world))));
    (world, host)
}

/// The metadata commands run through the pool's metadata lane and answer; they do not fall into
/// a session method a wrapper forgot to forward (blueprint w11 D-1). The fake answers with text
/// rows for the two statements, which is all the pooled path has to carry.
#[test]
fn columns_and_kinds_go_through_the_pooled_session_and_answer() {
    let (world, host) = fake_host();
    world.canned.lock().unwrap().extend([
        (
            "pg_attribute".to_owned(),
            vec![
                Some("id".to_owned()),
                Some("bigint".to_owned()),
                Some("NO".to_owned()),
                None,
                Some("identity".to_owned()),
            ],
        ),
        (
            "FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace WHERE n.nspname"
                .to_owned(),
            vec![Some("v".to_owned()), Some("view".to_owned())],
        ),
    ]);
    let mut pairs = pg_pairs("");
    pairs.extend([
        ("DB_SCHEMA", "public"),
        ("TARGET_SCHEMA", "public"),
        ("TARGET_TABLE", "t"),
    ]);

    let events = run_host(&host, EngineCommand::Columns, &pairs);
    let columns = last_event(&events, "table_columns");
    assert_eq!(columns["fields"][0]["name"], "id", "{events:?}");
    assert_eq!(columns["fields"][0]["extra"], "identity");
    assert_eq!(columns["object"]["schema"], "public");

    pairs.push(("OBJECT_KINDS", "1"));
    let events = run_host(&host, EngineCommand::Tables, &pairs);
    let tables = last_event(&events, "tables");
    assert_eq!(tables["names"][0], "v", "{events:?}");
    assert_eq!(tables["kinds"][0], "view");

    let sent = world.all_executed();
    assert!(
        sent.iter().any(|sql| sql.contains("pg_attribute")),
        "{sent:?}"
    );
    assert_eq!(world.connects(), 1, "both ran on the one metadata session");
}

fn pg_pairs(sql: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![
        ("DB_KIND", "postgres"),
        ("DB_HOST", "db.example"),
        ("DB_PORT", "5432"),
        ("DB_USER", "u"),
        ("DB_PASSWORD", "p1"),
        ("DB_DATABASE", "d"),
        ("DB_SSLMODE", "disable"),
        ("RETRIES", "0"),
        ("SQL", sql),
    ]
}

/// Drain a statement to its end and return the cells as text.
async fn query(
    session: &mut Box<dyn Session>,
    sql: &str,
    options: &ExecuteOptions,
) -> Result<Vec<Vec<String>>, EngineError> {
    let mut cursor = session.execute(sql, options).await?;
    let mut rows = Vec::new();
    while let Some(batch) = cursor.next_batch(100).await? {
        for row in 0..batch.rows() {
            rows.push(
                (0..batch.width())
                    .map(|column| {
                        qh_core::render::to_text(batch.value(row, column).expect("a cell"))
                            .unwrap_or_else(|| "NULL".to_owned())
                    })
                    .collect(),
            );
        }
    }
    Ok(rows)
}

fn no_options() -> ExecuteOptions {
    ExecuteOptions::default()
}

// --------------------------------------------------------------------------- //
// T1 - T10: offline
// --------------------------------------------------------------------------- //

#[test]
fn the_pool_reuses_a_session_and_resets_it_between_runs() {
    let (world, host) = fake_host();
    *world.reset_delay.lock().unwrap() = Duration::from_millis(400);

    let started = Instant::now();
    let first = run_host(&host, EngineCommand::Preview, &pg_pairs("SELECT 1"));
    let took = started.elapsed();
    assert_eq!(last_event(&first, "done")["rows"], 3, "{first:?}");
    // `done` must not wait for the reset: the reset is 400 ms, the run is a fake.
    assert!(
        took < Duration::from_millis(300),
        "the run waited for the reset: {took:?}"
    );
    assert_eq!(world.total(|p| p.resets_done.load(Ordering::SeqCst)), 0);

    // The second Run waits at most CHECKIN_WAIT for the session being reset, which is
    // shorter than the reset, so it opens a new session: this is the "no queue" rule.
    let pool = host.pool();
    block(async { pool.settle().await });
    *world.reset_delay.lock().unwrap() = Duration::ZERO;
    let second = run_host(&host, EngineCommand::Preview, &pg_pairs("SELECT 2"));
    assert_eq!(last_event(&second, "done")["rows"], 3, "{second:?}");
    let pool = host.pool();
    block(async { pool.settle().await });

    assert_eq!(
        world.connects(),
        1,
        "the second run reused the first run's session"
    );
    assert_eq!(
        world.total(|p| p.resets_started.load(Ordering::SeqCst)),
        2,
        "one reset per checkin"
    );
    let stats = host.pool().stats();
    assert_eq!(
        (stats.idle, stats.opened, stats.reused),
        (1, 1, 1),
        "{stats:?}"
    );
}

#[tokio::test]
async fn pool_exhaustion_opens_a_session_outside_the_reservation_instead_of_waiting() {
    let (world, host) = fake_host();
    let pool = host.pool();
    let config = pg_config("p1");
    let query_engine = pool.engine(Lane::Query, settings(&[]));
    let metadata_engine = pool.engine(Lane::Metadata, settings(&[]));

    // Three idle sessions: two query leases and one metadata lease, held together.
    let a = query_engine.connect(&config).await.expect("a");
    let b = query_engine.connect(&config).await.expect("b");
    let c = metadata_engine.connect(&config).await.expect("c");
    for session in [a, b, c] {
        session.close().await.expect("close");
    }
    pool.settle().await;
    assert_eq!(pool.stats().idle, 3);
    assert_eq!(world.connects(), 3);

    // Three previews at once and one more metadata call.
    let started = Instant::now();
    let p1 = query_engine.connect(&config).await.expect("p1");
    let p2 = query_engine.connect(&config).await.expect("p2");
    let p3 = query_engine.connect(&config).await.expect("p3");
    let m1 = metadata_engine.connect(&config).await.expect("m1");
    let m2 = metadata_engine.connect(&config).await.expect("m2");
    assert!(
        started.elapsed() < CHECKIN_WAIT,
        "nothing queued: {:?}",
        started.elapsed()
    );
    // Query cap is 2: p1, p2 and the metadata call reuse idle sessions, p3 and m2 are new.
    assert_eq!(
        world.connects(),
        5,
        "two sessions were opened outside the reservation"
    );

    for session in [p1, p2, p3, m1, m2] {
        session.close().await.expect("close");
    }
    pool.settle().await;
    let stats = pool.stats();
    assert_eq!(stats.idle, 3, "the pool keeps IDLE_MAX sessions: {stats:?}");
    assert_eq!(stats.closed, 2, "the rest are closed: {stats:?}");
    assert_eq!((stats.leased_query, stats.leased_metadata), (0, 0));
}

#[tokio::test]
async fn a_changed_credential_retires_the_old_sessions() {
    let (world, host) = fake_host();
    let pool = host.pool();
    let engine = pool.engine(Lane::Query, settings(&[]));

    let first = engine
        .connect(&pg_config("hunter-one"))
        .await
        .expect("first");
    first.close().await.expect("close");
    pool.settle().await;
    assert_eq!(pool.stats().idle, 1);

    // A lease under the old password, still out when the password changes.
    let stale = engine
        .connect(&pg_config("hunter-one"))
        .await
        .expect("stale");
    let second = engine
        .connect(&pg_config("hunter-two"))
        .await
        .expect("second");
    assert_eq!(world.connects(), 2, "the new password opened a new session");
    assert_eq!(world.probe(1).password.as_deref(), Some("hunter-two"));

    // Returning the old lease discards it rather than pooling a session with old credentials.
    stale.close().await.expect("close");
    second.close().await.expect("close");
    pool.settle().await;
    let stats = pool.stats();
    assert_eq!(
        stats.idle, 1,
        "only the new credential's session is pooled: {stats:?}"
    );
    assert!(
        world.probe(0).closed.load(Ordering::SeqCst),
        "the old session is closed"
    );

    // The key names neither password.
    let one = format!(
        "{:?}",
        PoolKey::of(&pg_config("hunter-one"), &settings(&[]))
    );
    let two = format!(
        "{:?}",
        PoolKey::of(&pg_config("hunter-two"), &settings(&[]))
    );
    for rendered in [&one, &two] {
        assert!(!rendered.contains("hunter"), "{rendered}");
    }
    assert_ne!(
        PoolKey::of(&pg_config("hunter-one"), &settings(&[])),
        PoolKey::of(&pg_config("hunter-two"), &settings(&[])),
        "the credential is part of the key"
    );
}

fn bastion_config() -> ConnectionConfig {
    let mut config = pg_config("p1");
    config.tunnel = Some(TunnelConfig {
        host: "bastion.example".to_owned(),
        port: 22,
        user: "jump".to_owned(),
        auth: TunnelAuth::Agent,
        known_hosts: Some("/u/known_hosts".into()),
        app_known_hosts: Some("/app/known_hosts".into()),
        host_key_accept: None,
        alias: None,
    });
    config
}

fn key_of(config: &ConnectionConfig) -> PoolKey {
    PoolKey::of(config, &settings(&[]))
}

#[test]
fn the_trust_files_of_a_tunnel_are_part_of_its_key_and_a_one_run_pin_is_not() {
    let base = key_of(&bastion_config());

    // A tunnel verified against one set of files is not reused by a run that names another:
    // that would make choosing the file a way around verification.
    let mut other_app_file = bastion_config();
    other_app_file.tunnel.as_mut().unwrap().app_known_hosts = Some("/other/known_hosts".into());
    assert_ne!(base, key_of(&other_app_file), "the app's file");
    let mut no_app_file = bastion_config();
    no_app_file.tunnel.as_mut().unwrap().app_known_hosts = None;
    assert_ne!(base, key_of(&no_app_file), "having no app file");
    let mut other_user_file = bastion_config();
    other_user_file.tunnel.as_mut().unwrap().known_hosts = Some("/elsewhere/known_hosts".into());
    assert_ne!(base, key_of(&other_user_file), "the user's file");

    // What does not decide trust does not split the pool: the pin applies to one run, and a
    // tunnel that was verified and recorded stays valid after it; the alias and the TLS name
    // are derived from a host that is already in the key.
    let mut derived = bastion_config();
    derived.tunnel.as_mut().unwrap().host_key_accept =
        Some("SHA256:ldyiXa1JQakitNU5tErauu8DvWQ1dZ7aXu+rm7KQuog".to_owned());
    derived.tunnel.as_mut().unwrap().alias = Some("prod-bastion".to_owned());
    derived.tls_server_name = Some("db.example".to_owned());
    assert_eq!(base, key_of(&derived));
}

fn fixture_ca(name: &str) -> qh_driver::TlsCa {
    let path = format!(
        "{}/../qh-driver-postgres/tests/tls/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    qh_driver::TlsCa::load(std::path::Path::new(&path)).expect("a fixture CA")
}

#[test]
fn a_jwt_or_a_ca_makes_a_different_connection_and_neither_prints() {
    let trino = || {
        ConnectionConfig::new(DriverKind::Trino, "trino.example", 443, "u").tls(TlsMode::Require)
    };
    let one = trino().bearer("jwt-ONE-SENTINEL");
    let two = trino().bearer("jwt-TWO-SENTINEL");
    assert_ne!(key_of(&one), key_of(&two), "the token is part of the key");
    assert_ne!(key_of(&one), key_of(&trino()), "a token and none differ");
    for rendered in [format!("{:?}", key_of(&one)), format!("{one:?}")] {
        assert!(!rendered.contains("SENTINEL"), "{rendered}");
    }

    // The path is identity and the bytes are hashed: another file is another trust, and so is
    // the same path with other contents.
    let ca = fixture_ca("ca.crt");
    let other = fixture_ca("other-ca.crt");
    let with_ca = trino().tls_ca(ca.clone());
    assert_ne!(key_of(&with_ca), key_of(&trino()), "a CA and none differ");
    assert_ne!(key_of(&with_ca), key_of(&trino().tls_ca(other.clone())));
    let swapped = |pem: &qh_driver::TlsCa| {
        let pem_text = std::fs::read(format!(
            "{}/../qh-driver-postgres/tests/tls/{}",
            env!("CARGO_MANIFEST_DIR"),
            if pem == &ca { "ca.crt" } else { "other-ca.crt" }
        ))
        .unwrap();
        trino().tls_ca(qh_driver::TlsCa::from_pem("/etc/same-path.pem", &pem_text).unwrap())
    };
    assert_ne!(
        key_of(&swapped(&ca)),
        key_of(&swapped(&other)),
        "the same path with other bytes is other trust"
    );
    assert_eq!(key_of(&swapped(&ca)), key_of(&swapped(&ca)));
}

#[tokio::test]
async fn a_cancelled_or_dropped_lease_is_handled() {
    let (world, host) = fake_host();
    let pool = host.pool();
    let engine = pool.engine(Lane::Query, settings(&[]));
    let config = pg_config("p1");

    // A cancelled lease never returns to the pool, so a late cancel cannot hit the next Run.
    let session = engine.connect(&config).await.expect("connect");
    session.cancel().await.expect("cancel");
    session.close().await.expect("close");
    pool.settle().await;
    let stats = pool.stats();
    assert_eq!((stats.idle, stats.closed), (0, 1), "{stats:?}");
    assert_eq!(
        world.total(|p| p.resets_started.load(Ordering::SeqCst)),
        0,
        "not even reset"
    );
    let next = engine.connect(&config).await.expect("next");
    assert_eq!(
        world.connects(),
        2,
        "the next Run got a session that was never cancelled"
    );
    assert_eq!(world.probe(1).cancels.load(Ordering::SeqCst), 0);

    // A lease dropped without `close` still checks in (an early `?` in a command).
    drop(next);
    pool.settle().await;
    assert_eq!(pool.stats().idle, 1);
    assert_eq!(world.total(|p| p.resets_done.load(Ordering::SeqCst)), 1);

    // A checkout future dropped in the middle of a connect gives its lane count back.
    *world.connect_delay.lock().unwrap() = Duration::from_secs(30);
    let other = pg_config("p2");
    let dropped = tokio::time::timeout(Duration::from_millis(50), engine.connect(&other)).await;
    assert!(dropped.is_err(), "the connect was still in flight");
    assert_eq!(pool.stats().leased_query, 0, "{:?}", pool.stats());
}

#[tokio::test]
async fn a_stale_read_reconnects_once_and_a_write_does_not() {
    let (world, host) = fake_host();
    let pool = host.pool();
    let engine = pool.engine(Lane::Query, settings(&[]));
    let config = pg_config("p1");

    // Warm one session, then let its socket die while it sits idle.
    let mut warm = engine.connect(&config).await.expect("warm");
    query(&mut warm, "SELECT 1", &no_options())
        .await
        .expect("first run");
    warm.close().await.expect("close");
    pool.settle().await;
    world.stale.lock().unwrap().insert(0);

    // A read on the reused session: one reconnect, then it runs.
    let mut read = engine.connect(&config).await.expect("read");
    let rows = query(&mut read, "SELECT 2", &no_options())
        .await
        .expect("the read survives");
    assert_eq!(rows.len(), 3);
    assert_eq!(world.connects(), 2, "exactly one reconnect");
    assert!(world
        .probe(1)
        .executed
        .lock()
        .unwrap()
        .contains(&"SELECT 2".to_owned()));
    read.close().await.expect("close");
    pool.settle().await;

    // BEGIN is safe to send again: a transaction on a dead connection died with it.
    world.stale.lock().unwrap().insert(1);
    let mut begin = engine.connect(&config).await.expect("begin");
    query(&mut begin, "BEGIN", &no_options())
        .await
        .expect("BEGIN reconnects");
    assert_eq!(world.connects(), 3);
    begin.close().await.expect("close");
    pool.settle().await;

    // A write is never sent again: the error comes back, and RETRIES does not get a fresh
    // connection for it either.
    world.stale.lock().unwrap().insert(2);
    let mut write = engine.connect(&config).await.expect("write");
    let policy = RetryPolicy::new(2, Duration::from_millis(1));
    let error = match retry::execute(&mut write, &policy, "UPDATE t SET a = 1", &no_options()).await
    {
        Ok(_) => panic!("the write must not run on a stale connection"),
        Err(error) => error,
    };
    assert!(
        !world
            .all_executed()
            .iter()
            .any(|sql| sql.contains("UPDATE")),
        "no session ever saw the UPDATE"
    );
    assert_eq!(
        world.connects(),
        3,
        "no connection was opened for the write: {error:?}"
    );
    write.close().await.expect("close");
    pool.settle().await;
    assert_eq!(
        pool.stats().idle,
        0,
        "a broken lease is discarded, not pooled"
    );
}

#[test]
fn safe_mode_is_decided_per_run_on_a_shared_key() {
    let (world, host) = fake_host();

    let mut a = pg_pairs("DELETE FROM t WHERE a = 1");
    a.push(("SAFE_MODE", "full"));
    let a = run_host(&host, EngineCommand::Preview, &a);
    assert_eq!(last_event(&a, "done")["rows"], 3, "{a:?}");
    let pool = host.pool();
    block(async { pool.settle().await });
    let before = host.pool().stats();

    let mut b = pg_pairs("DELETE FROM t WHERE a = 1");
    b.push(("SAFE_MODE", "read_only"));
    let b = run_host(&host, EngineCommand::Preview, &b);
    assert_eq!(last_event(&b, "error")["event"], "error", "{b:?}");
    let after = host.pool().stats();
    assert_eq!(
        (before.opened, before.reused, before.idle),
        (after.opened, after.reused, after.idle),
        "a refused Run borrows nothing"
    );

    let mut c = pg_pairs("DELETE FROM t WHERE a = 2");
    c.push(("SAFE_MODE", "full"));
    let c = run_host(&host, EngineCommand::Preview, &c);
    assert_eq!(last_event(&c, "done")["rows"], 3, "{c:?}");
    let pool = host.pool();
    block(async { pool.settle().await });
    assert_eq!(world.connects(), 1, "C reused A's session");
}

#[tokio::test]
async fn one_tunnel_serves_every_session_of_a_key() {
    let (world, host) = fake_host();
    let pool = host.pool();
    let env = settings(&[("SSH_PASSWORD", "x")]);
    let mut config = pg_config("p1");
    config.tunnel = Some(TunnelConfig {
        host: "bastion.example".to_owned(),
        port: 22,
        user: "jump".to_owned(),
        auth: TunnelAuth::Password("x".to_owned()),
        known_hosts: None,
        app_known_hosts: None,
        host_key_accept: None,
        alias: None,
    });

    let query_engine = pool.engine(Lane::Query, env.clone());
    let metadata_engine = pool.engine(Lane::Metadata, env.clone());
    let long_engine = pool.long_op_engine(env.clone());

    let q1 = query_engine.connect(&config).await.expect("q1");
    let q2 = query_engine.connect(&config).await.expect("q2");
    let m = metadata_engine.connect(&config).await.expect("m");
    let export = long_engine.connect(&config).await.expect("export");
    assert_eq!(
        world.tunnels_opened.load(Ordering::SeqCst),
        1,
        "one tunnel for the key"
    );
    assert_eq!(pool.stats().tunnels_opened, 1);
    assert_eq!(
        world.probe(0).tunnel_port,
        Some(40_000),
        "the sessions connect through the tunnel's port"
    );
    for session in [q1, q2, m] {
        session.close().await.expect("close");
    }
    pool.settle().await;

    // Everything is idle, but an export is still running: eviction must not close the tunnel.
    pool.evict(Duration::ZERO).await;
    assert_eq!(
        world.tunnels_dropped.load(Ordering::SeqCst),
        0,
        "the export keeps its tunnel"
    );
    export.close().await.expect("close");
    pool.evict(Duration::ZERO).await;
    assert_eq!(
        world.tunnels_dropped.load(Ordering::SeqCst),
        1,
        "released after the last user"
    );

    // A tunnel that has died is opened again, and the sessions that rode on it are closed.
    let s = query_engine.connect(&config).await.expect("s");
    s.close().await.expect("close");
    pool.settle().await;
    assert_eq!(world.tunnels_opened.load(Ordering::SeqCst), 2);
    world.tunnels.lock().unwrap()[1]
        .alive
        .store(false, Ordering::SeqCst);
    let again = query_engine.connect(&config).await.expect("again");
    assert_eq!(
        world.tunnels_opened.load(Ordering::SeqCst),
        3,
        "a dead tunnel is replaced"
    );
    pool.settle().await;
    assert!(
        world.probe(4).closed.load(Ordering::SeqCst),
        "the idle session of the dead tunnel is closed"
    );
    again.close().await.expect("close");
}

#[test]
fn the_host_path_emits_what_the_free_run_emits() {
    struct Plain(FakeConnector);

    #[async_trait]
    impl Engine for Plain {
        fn kinds(&self) -> Vec<DriverKind> {
            DriverKind::ALL.to_vec()
        }
        fn driver(&self, kind: DriverKind) -> &dyn Driver {
            self.0.driver(kind)
        }
        async fn connect(
            &self,
            config: &ConnectionConfig,
        ) -> Result<Box<dyn Session>, EngineError> {
            self.0.connect(config, None).await
        }
    }

    let (_world, host) = fake_host();
    let plain = Plain(FakeConnector(World::new()));
    let cases: [(EngineCommand, Command, &'static str); 5] = [
        (EngineCommand::Preview, Command::Preview, "SELECT 1"),
        (EngineCommand::Explain, Command::Explain, "SELECT 1"),
        (EngineCommand::Count, Command::Count, "SELECT 1"),
        (EngineCommand::Catalogs, Command::Catalogs, "SELECT 1"),
        (EngineCommand::Objects, Command::Objects, "SELECT 1"),
    ];
    for (through_host, direct, sql) in cases {
        let mut pairs = pg_pairs(sql);
        pairs.push(("DB_SCHEMA", "public"));
        let hosted = without_timing(run_host(&host, through_host, &pairs));

        let mut out = Capture::new();
        block(qh_ffi::run(
            direct,
            &settings(&pairs),
            &mut out,
            &plain,
            &CancelFlag::new(),
        ))
        .expect("the free run succeeds");
        assert_eq!(hosted, without_timing(out.lines), "{through_host:?}");
    }

    // A Stop pressed before the first batch: the same events either way.
    let cancel = RunCancel::new();
    cancel.request_cancel();
    let sink = Recorder::default();
    let pairs = pg_pairs("SELECT 1");
    host.run(
        EngineCommand::Preview,
        host_settings(&pairs),
        Arc::new(sink.clone()),
        cancel,
    );
    let flag = CancelFlag::new();
    flag.request();
    let mut out = Capture::new();
    block(qh_ffi::run(
        Command::Preview,
        &settings(&pairs),
        &mut out,
        &plain,
        &flag,
    ))
    .expect("the free run succeeds");
    // The engine polls the work before the stop (a batch that is already ready is kept), so
    // either run may or may not carry the first page. Both must end stopped, and any rows
    // they did carry must be the same rows.
    let (hosted, free) = (without_timing(sink.events()), without_timing(out.lines));
    for events in [&hosted, &free] {
        let done = events.last().expect("a done event");
        assert_eq!(done["event"], "done", "{events:?}");
        assert_eq!(done["cancelled"], true, "{events:?}");
    }
    let rows = |events: &Vec<serde_json::Value>| -> Vec<serde_json::Value> {
        events
            .iter()
            .filter(|e| e["event"] == "rows")
            .cloned()
            .collect()
    };
    if !rows(&hosted).is_empty() && !rows(&free).is_empty() {
        assert_eq!(rows(&hosted), rows(&free));
    }
}

#[test]
fn a_capped_preview_on_the_pool_is_not_cancelled_and_one_on_a_plain_engine_is() {
    let (world, host) = fake_host();
    let mut pairs = pg_pairs("SELECT 1");
    pairs.push(("LIMIT", "2"));

    // The fake answers `min(row_limit, 3)` rows, so a cap of 2 sees its verdict row only if the
    // command asked the driver for cap + 1.
    let events = run_host(&host, EngineCommand::Preview, &pairs);
    let done = last_event(&events, "done");
    assert_eq!(done["truncated"], true, "{events:?}");
    assert_eq!(done["rows"], 2, "{events:?}");
    let pool = host.pool();
    block(async { pool.settle().await });
    assert_eq!(
        world.total(|probe| probe.cancels.load(Ordering::SeqCst)),
        0,
        "a pooled session is never cancelled because of a cap"
    );

    // The same run on an engine with no pool stops the statement, as before.
    let plain_world = World::new();
    let plain = FakeConnector(Arc::clone(&plain_world));
    struct Plain(FakeConnector);
    #[async_trait]
    impl Engine for Plain {
        fn kinds(&self) -> Vec<DriverKind> {
            DriverKind::ALL.to_vec()
        }
        fn driver(&self, kind: DriverKind) -> &dyn Driver {
            self.0.driver(kind)
        }
        async fn connect(
            &self,
            config: &ConnectionConfig,
        ) -> Result<Box<dyn Session>, EngineError> {
            self.0.connect(config, None).await
        }
    }
    let mut out = Capture::new();
    block(qh_ffi::run(
        Command::Preview,
        &settings(&pairs),
        &mut out,
        &Plain(plain),
        &CancelFlag::new(),
    ))
    .expect("the free run succeeds");
    block(async { tokio::time::sleep(Duration::from_millis(100)).await });
    assert_eq!(
        plain_world.total(|probe| probe.cancels.load(Ordering::SeqCst)),
        1
    );
}

#[tokio::test]
async fn a_truncated_cursor_is_abandoned_and_a_drained_one_is_clean() {
    let (world, host) = fake_host();
    let pool = host.pool();
    let engine = pool.engine(Lane::Query, settings(&[]));
    let config = pg_config("p1");

    // Drained to `None`: clean, so the session is pooled.
    let mut s = engine.connect(&config).await.expect("connect");
    query(&mut s, "SELECT 1", &no_options())
        .await
        .expect("drained");
    s.close().await.expect("close");
    pool.settle().await;
    assert_eq!(pool.stats().idle, 1);
    assert_eq!(pool.stats().closed, 0);

    // Dropped after its first batch, still alive at checkin: abandoned, so the backend is
    // stopped by closing the socket, and a replacement is opened in the background.
    let mut s = engine.connect(&config).await.expect("connect");
    let mut cursor = s.execute("SELECT 2", &no_options()).await.expect("execute");
    let _ = cursor.next_batch(10).await.expect("a batch");
    s.close().await.expect("close");
    drop(cursor);
    pool.settle().await;
    let stats = pool.stats();
    assert_eq!(
        (stats.closed, stats.idle, stats.opened),
        (1, 1, 2),
        "{stats:?}"
    );

    // Stopped by the row limit: the limit ended it, not the server. Same fate.
    let mut s = engine.connect(&config).await.expect("connect");
    let limited = ExecuteOptions {
        row_limit: Some(2),
        ..ExecuteOptions::default()
    };
    query(&mut s, "SELECT 3", &limited).await.expect("limited");
    s.close().await.expect("close");
    pool.settle().await;
    let stats = pool.stats();
    assert_eq!(
        (stats.closed, stats.idle, stats.opened),
        (2, 1, 3),
        "a row-limited cursor is abandoned and refilled: {stats:?}"
    );

    // An error that carries the server's code ends the stream on the server: clean.
    world.fail_stream.lock().unwrap().push((
        "with_code".to_owned(),
        EngineError::Query {
            message: "division by zero".to_owned(),
            code: Some("22012".to_owned()),
            position: None,
            kind: FailureKind::Permanent,
        },
    ));
    let mut s = engine.connect(&config).await.expect("connect");
    assert!(query(&mut s, "SELECT with_code", &no_options())
        .await
        .is_err());
    s.close().await.expect("close");
    pool.settle().await;
    let stats = pool.stats();
    assert_eq!(
        (stats.closed, stats.idle, stats.opened),
        (2, 1, 3),
        "a coded error is the server's answer: {stats:?}"
    );

    // An error with no code (a decode failure, a transport error) proves nothing: discarded.
    world.fail_stream.lock().unwrap().push((
        "no_code".to_owned(),
        EngineError::Query {
            message: "cannot decode".to_owned(),
            code: None,
            position: None,
            kind: FailureKind::Permanent,
        },
    ));
    let mut s = engine.connect(&config).await.expect("connect");
    assert!(query(&mut s, "SELECT no_code", &no_options())
        .await
        .is_err());
    s.close().await.expect("close");
    pool.settle().await;
    let stats = pool.stats();
    assert_eq!(
        (stats.closed, stats.idle, stats.opened),
        (3, 1, 4),
        "{stats:?}"
    );
}

#[tokio::test]
async fn a_capped_mysql_or_trino_cursor_is_left_to_the_reset_and_a_cancelled_one_is_not_refilled() {
    let (world, host) = fake_host();
    let pool = host.pool();
    let engine = pool.engine(Lane::Query, settings(&[]));
    let limited = ExecuteOptions {
        row_limit: Some(2),
        ..ExecuteOptions::default()
    };

    // MySQL says by itself whether its connection came back (its reset fails when it did not),
    // and Trino's reset deletes the query: the cursor's own end decides nothing for either, so
    // the session is reset and kept.
    for kind in [DriverKind::Mysql, DriverKind::Trino] {
        let config = ConnectionConfig::new(kind, "db.example", 1234, "u")
            .password("p1")
            .database("d")
            .tls(TlsMode::Disable);
        let mut s = engine.connect(&config).await.expect("connect");
        query(&mut s, "SELECT 3", &limited).await.expect("limited");
        s.close().await.expect("close");
        pool.settle().await;
        let stats = pool.stats();
        assert_eq!(stats.closed, 0, "{kind:?} keeps its session: {stats:?}");
        assert_eq!(stats.idle, if kind == DriverKind::Mysql { 1 } else { 2 });
    }
    assert_eq!(
        world.total(|probe| probe.resets_done.load(Ordering::SeqCst)),
        2
    );

    // A Stop discards, and nothing refills after it: the user asked for the query to end.
    let config = pg_config("p2");
    let mut s = engine.connect(&config).await.expect("connect");
    let _ = s.execute("SELECT 4", &no_options()).await.expect("execute");
    s.cancel().await.expect("cancel");
    s.close().await.expect("close");
    pool.settle().await;
    let stats = pool.stats();
    assert_eq!(stats.closed, 1, "{stats:?}");
    assert_eq!(stats.opened, 3, "no replacement was opened: {stats:?}");
}

#[test]
fn count_drains_its_cursor_so_its_session_goes_back_to_the_pool() {
    let (world, host) = fake_host();
    for _ in 0..2 {
        let events = run_host(&host, EngineCommand::Count, &pg_pairs("SELECT 1"));
        assert_eq!(last_event(&events, "done")["event"], "done", "{events:?}");
        let pool = host.pool();
        block(async { pool.settle().await });
    }
    assert_eq!(
        world.connects(),
        1,
        "count left its cursor unfinished, so its session was thrown away"
    );
}

#[test]
fn the_shared_store_is_opened_and_migrated_once_per_path() {
    let (_world, host) = fake_host();
    let dir = tempfile::tempdir().expect("temp dir");
    let one = dir.path().join("one.db").to_string_lossy().into_owned();
    let two = dir.path().join("two.db").to_string_lossy().into_owned();
    let legacy = dir.path().join("legacy").to_string_lossy().into_owned();

    for sql in ["SELECT 1", "SELECT 2"] {
        let events = run_host(
            &host,
            EngineCommand::HistoryAdd,
            &[("DB_PATH", &one), ("LEGACY_PATH", &legacy), ("SQL", sql)],
        );
        assert_eq!(events[0]["event"], "history_entry", "{events:?}");
    }
    let listed = run_host(
        &host,
        EngineCommand::History,
        &[("DB_PATH", &one), ("LEGACY_PATH", &legacy)],
    );
    assert_eq!(
        listed[0]["entries"].as_array().map(Vec::len),
        Some(2),
        "{listed:?}"
    );
    assert_eq!(host.storage_opens(), 1, "three commands, one open");

    // Another path opens its own database and its own contents.
    let other = run_host(
        &host,
        EngineCommand::History,
        &[("DB_PATH", &two), ("LEGACY_PATH", &legacy)],
    );
    assert_eq!(
        other[0]["entries"].as_array().map(Vec::len),
        Some(0),
        "{other:?}"
    );
    assert_eq!(host.storage_opens(), 2);
}

// --------------------------------------------------------------------------- //
// L1 - L4: PostgreSQL, live
// --------------------------------------------------------------------------- //

const PG_SKIP: &str = "skipped: set QH_TEST_POSTGRES=1 with deploy/dev/up.sh postgres running";

fn live_pg() -> Option<ConnectionConfig> {
    if std::env::var("QH_TEST_POSTGRES").as_deref() != Ok("1") {
        return None;
    }
    Some(
        ConnectionConfig::new(DriverKind::Postgres, "127.0.0.1", 55432, "qh")
            .password("qh-dev-only")
            .database("qh")
            .tls(TlsMode::Disable),
    )
}

async fn lease(engine: &dyn Engine, config: &ConnectionConfig) -> Box<dyn Session> {
    engine
        .connect(config)
        .await
        .expect("connect to the dev container")
}

async fn one(session: &mut Box<dyn Session>, sql: &str) -> String {
    query(session, sql, &no_options())
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error:?}"))
        .first()
        .and_then(|row| row.first().cloned())
        .unwrap_or_else(|| "NULL".to_owned())
}

#[tokio::test]
async fn postgres_search_path_does_not_leak_between_runs() {
    let Some(config) = live_pg() else {
        eprintln!("{PG_SKIP}");
        return;
    };
    let host = EngineHost::new();
    let pool = host.pool();
    let engine = pool.engine(Lane::Query, settings(&[]));

    let mut a = lease(&*engine, &config).await;
    let default_path = one(&mut a, "SHOW search_path").await;
    let pid = one(&mut a, "SELECT pg_backend_pid()").await;
    one(&mut a, "SET search_path = pg_catalog").await;
    one(&mut a, "CREATE TEMP TABLE qh_host_leak (i int)").await;
    one(&mut a, "SELECT pg_advisory_lock(424242)").await;
    assert_eq!(one(&mut a, "SHOW search_path").await, "pg_catalog");
    a.close().await.expect("close");
    pool.settle().await;

    let mut b = lease(&*engine, &config).await;
    assert_eq!(
        one(&mut b, "SELECT pg_backend_pid()").await,
        pid,
        "the session was reused"
    );
    assert_eq!(one(&mut b, "SHOW search_path").await, default_path);
    assert_eq!(
        one(&mut b, "SELECT to_regclass('pg_temp.qh_host_leak')::text").await,
        "NULL"
    );
    assert_eq!(
        one(
            &mut b,
            "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND pid = pg_backend_pid()"
        )
        .await,
        "0"
    );
    b.close().await.expect("close");
    pool.settle().await;
}

/// The server-enforced read-only switch is session state, so the pool's reset has to clear it:
/// a run under `read_only` must not leave the next (writing) run on the same session read-only.
#[tokio::test]
async fn postgres_read_only_does_not_survive_a_checkin() {
    let Some(config) = live_pg() else {
        eprintln!("{PG_SKIP}");
        return;
    };
    let host = EngineHost::new();
    let pool = host.pool();
    let engine = pool.engine(Lane::Query, settings(&[]));
    let table = format!("qh_w3t0b_pool_{}", std::process::id());

    let mut setup = lease(&*engine, &config).await;
    one(&mut setup, &format!("DROP TABLE IF EXISTS {table}")).await;
    one(&mut setup, &format!("CREATE TABLE {table} (i int)")).await;
    setup.close().await.expect("close");
    pool.settle().await;

    let mut a = lease(&*engine, &config).await;
    let pid = one(&mut a, "SELECT pg_backend_pid()").await;
    a.enforce_read_only().await.expect("switch it on");
    let refused = query(
        &mut a,
        &format!("INSERT INTO {table} VALUES (1)"),
        &no_options(),
    )
    .await;
    assert!(
        format!("{refused:?}").contains("read-only transaction"),
        "{refused:?}"
    );
    a.close().await.expect("close");
    pool.settle().await;

    let mut b = lease(&*engine, &config).await;
    assert_eq!(
        one(&mut b, "SELECT pg_backend_pid()").await,
        pid,
        "the session was reused"
    );
    assert_eq!(
        one(&mut b, "SHOW default_transaction_read_only").await,
        "off"
    );
    one(&mut b, &format!("INSERT INTO {table} VALUES (2)")).await;
    one(&mut b, &format!("DROP TABLE {table}")).await;
    b.close().await.expect("close");
    pool.settle().await;
}

#[tokio::test]
async fn postgres_a_failed_transaction_is_rolled_back_before_reuse() {
    let Some(config) = live_pg() else {
        eprintln!("{PG_SKIP}");
        return;
    };
    let host = EngineHost::new();
    let pool = host.pool();
    let engine = pool.engine(Lane::Query, settings(&[]));

    let mut a = lease(&*engine, &config).await;
    let pid = one(&mut a, "SELECT pg_backend_pid()").await;
    one(&mut a, "BEGIN").await;
    assert!(query(&mut a, "SELECT 1/0", &no_options()).await.is_err());
    a.close().await.expect("close");
    pool.settle().await;

    let mut b = lease(&*engine, &config).await;
    assert_eq!(
        one(&mut b, "SELECT pg_backend_pid()").await,
        pid,
        "reused, not replaced"
    );
    assert_eq!(
        one(&mut b, "SELECT 1").await,
        "1",
        "not 25P02, the transaction is gone"
    );
    b.close().await.expect("close");
    pool.settle().await;
}

#[tokio::test]
async fn postgres_timeout_changes_between_runs_are_honoured() {
    let Some(config) = live_pg() else {
        eprintln!("{PG_SKIP}");
        return;
    };
    let host = EngineHost::new();
    let pool = host.pool();
    let engine = pool.engine(Lane::Query, settings(&[]));
    let bounded = ExecuteOptions {
        statement_timeout: Some(Duration::from_millis(100)),
        ..ExecuteOptions::default()
    };

    let mut a = lease(&*engine, &config).await;
    let pid = one(&mut a, "SELECT pg_backend_pid()").await;
    let error = query(&mut a, "SELECT pg_sleep(0.5)", &bounded)
        .await
        .expect_err("times out");
    assert!(matches!(error, EngineError::Timeout { .. }), "{error:?}");
    a.close().await.expect("close");
    pool.settle().await;

    // Run B asks for no bound: the 100 ms of Run A must not still be on the session.
    let mut b = lease(&*engine, &config).await;
    assert_eq!(one(&mut b, "SELECT pg_backend_pid()").await, pid);
    assert_eq!(one(&mut b, "SHOW statement_timeout").await, "0");
    assert!(query(&mut b, "SELECT pg_sleep(0.2)", &no_options())
        .await
        .is_ok());
    // And Run C asks for 100 ms again, which must be sent even though Run A sent it once.
    b.close().await.expect("close");
    pool.settle().await;
    let mut c = lease(&*engine, &config).await;
    let error = query(&mut c, "SELECT pg_sleep(0.5)", &bounded)
        .await
        .expect_err("times out again");
    assert!(matches!(error, EngineError::Timeout { .. }), "{error:?}");
    c.close().await.expect("close");
    pool.settle().await;
}

#[tokio::test]
async fn postgres_reset_keeps_the_clients_own_statements() {
    let Some(config) = live_pg() else {
        eprintln!("{PG_SKIP}");
        return;
    };
    let host = EngineHost::new();
    let pool = host.pool();
    let engine = pool.engine(Lane::Query, settings(&[]));

    let mut setup = lease(&*engine, &config).await;
    for sql in [
        "DROP SCHEMA IF EXISTS qh_host_test CASCADE",
        "CREATE SCHEMA qh_host_test",
        "CREATE TYPE qh_host_test.mood1 AS ENUM ('a', 'b')",
        "CREATE TYPE qh_host_test.mood2 AS ENUM ('c', 'd')",
    ] {
        one(&mut setup, sql).await;
    }
    setup.close().await.expect("close");
    pool.settle().await;

    // Run A meets an enum type the client has to look up, and leaves a SQL-level prepared statement.
    let mut a = lease(&*engine, &config).await;
    let pid = one(&mut a, "SELECT pg_backend_pid()").await;
    assert_eq!(one(&mut a, "SELECT 'a'::qh_host_test.mood1").await, "a");
    one(&mut a, "PREPARE qh_leak AS SELECT 1").await;
    a.close().await.expect("close");
    pool.settle().await;

    // Run B meets a different one after the reset: a `DEALLOCATE ALL` in the reset would
    // have taken the client's own `typeinfo` statements and failed here.
    let mut b = lease(&*engine, &config).await;
    assert_eq!(
        one(&mut b, "SELECT pg_backend_pid()").await,
        pid,
        "the session was reused"
    );
    assert_eq!(one(&mut b, "SELECT 'c'::qh_host_test.mood2").await, "c");
    // The user's own PREPARE is gone, so the name is free again.
    one(&mut b, "PREPARE qh_leak AS SELECT 1").await;
    one(&mut b, "DROP SCHEMA qh_host_test CASCADE").await;
    b.close().await.expect("close");
    pool.settle().await;
}

// --------------------------------------------------------------------------- //
// L6, L7: MySQL, live
// --------------------------------------------------------------------------- //

const MYSQL_SKIP: &str = "skipped: set QH_TEST_MYSQL=1 with deploy/dev/up.sh mysql running";

fn live_mysql() -> Option<ConnectionConfig> {
    if std::env::var("QH_TEST_MYSQL").as_deref() != Ok("1") {
        return None;
    }
    Some(
        ConnectionConfig::new(DriverKind::Mysql, "127.0.0.1", 53306, "qh")
            .password("qh-dev-only")
            .database("qh")
            .tls(TlsMode::Disable),
    )
}

#[tokio::test]
async fn mysql_timeout_changes_between_runs_are_honoured() {
    let Some(config) = live_mysql() else {
        eprintln!("{MYSQL_SKIP}");
        return;
    };
    let host = EngineHost::new();
    let pool = host.pool();
    let engine = pool.engine(Lane::Query, settings(&[]));
    let scan = "SELECT COUNT(*) FROM information_schema.columns a \
                CROSS JOIN information_schema.columns b \
                CROSS JOIN information_schema.columns c";

    let mut a = lease(&*engine, &config).await;
    let id = one(&mut a, "SELECT CONNECTION_ID()").await;
    let bounded = ExecuteOptions {
        statement_timeout: Some(Duration::from_millis(300)),
        ..ExecuteOptions::default()
    };
    let error = query(&mut a, scan, &bounded)
        .await
        .expect_err("the scan is interrupted");
    assert!(matches!(error, EngineError::Timeout { .. }), "{error:?}");
    a.close().await.expect("close");
    pool.settle().await;

    // The next Run asks for no bound, on the same connection: the bound must be gone.
    let mut b = lease(&*engine, &config).await;
    assert_eq!(
        one(&mut b, "SELECT CONNECTION_ID()").await,
        id,
        "the connection was reused"
    );
    assert_eq!(
        one(&mut b, "SELECT @@SESSION.max_execution_time").await,
        "0"
    );
    b.close().await.expect("close");
    pool.settle().await;

    // And asking for it again works: the tracker was reset with the session.
    let mut c = lease(&*engine, &config).await;
    let error = query(&mut c, scan, &bounded)
        .await
        .expect_err("interrupted again");
    assert!(matches!(error, EngineError::Timeout { .. }), "{error:?}");
    c.close().await.expect("close");
    pool.settle().await;
}

#[tokio::test]
async fn mysql_session_state_does_not_leak_between_runs() {
    let Some(config) = live_mysql() else {
        eprintln!("{MYSQL_SKIP}");
        return;
    };
    let host = EngineHost::new();
    let pool = host.pool();
    let engine = pool.engine(Lane::Query, settings(&[]));

    let mut a = lease(&*engine, &config).await;
    let id = one(&mut a, "SELECT CONNECTION_ID()").await;
    let default_mode = one(&mut a, "SELECT @@SESSION.sql_mode").await;
    // What the handshake gave the connection, taken before this run changes anything.
    let charsets = "SELECT @@collation_connection, @@character_set_client, @@character_set_results";
    let default_charsets = query(&mut a, charsets, &no_options())
        .await
        .expect("charsets");
    one(&mut a, "SET NAMES latin1").await;
    one(&mut a, "SET @qh_leak = 7").await;
    one(&mut a, "SET SESSION sql_mode = 'ANSI'").await;
    one(&mut a, "CREATE TEMPORARY TABLE qh_host_leak (i INT)").await;
    one(&mut a, "SELECT GET_LOCK('qh_host_leak', 0)").await;
    one(&mut a, "USE information_schema").await;
    assert_eq!(one(&mut a, "SELECT DATABASE()").await, "information_schema");
    a.close().await.expect("close");
    pool.settle().await;

    let mut b = lease(&*engine, &config).await;
    assert_eq!(
        one(&mut b, "SELECT CONNECTION_ID()").await,
        id,
        "the connection was reused"
    );
    assert_eq!(one(&mut b, "SELECT @qh_leak").await, "NULL");
    // COM_RESET_CONNECTION falls back to the server's globals; the session must not.
    assert_eq!(
        query(&mut b, charsets, &no_options())
            .await
            .expect("charsets"),
        default_charsets,
        "the reused connection compares and encodes text like a fresh one"
    );
    assert_eq!(one(&mut b, "SELECT @@SESSION.sql_mode").await, default_mode);
    assert!(query(&mut b, "SELECT * FROM qh_host_leak", &no_options())
        .await
        .is_err());
    assert_eq!(
        one(&mut b, "SELECT IS_FREE_LOCK('qh_host_leak')").await,
        "1"
    );
    assert_eq!(
        one(&mut b, "SELECT DATABASE()").await,
        "qh",
        "the Run's own database again"
    );
    b.close().await.expect("close");
    pool.settle().await;
}

#[tokio::test]
async fn mysql_a_transaction_left_open_does_not_survive_a_checkin() {
    let Some(config) = live_mysql() else {
        eprintln!("{MYSQL_SKIP}");
        return;
    };
    let host = EngineHost::new();
    let pool = host.pool();
    let engine = pool.engine(Lane::Query, settings(&[]));

    let mut setup = lease(&*engine, &config).await;
    one(&mut setup, "DROP TABLE IF EXISTS qh_host_txn").await;
    one(&mut setup, "CREATE TABLE qh_host_txn (i INT) ENGINE=InnoDB").await;
    setup.close().await.expect("close");
    pool.settle().await;

    let mut a = lease(&*engine, &config).await;
    let id = one(&mut a, "SELECT CONNECTION_ID()").await;
    one(&mut a, "BEGIN").await;
    one(&mut a, "INSERT INTO qh_host_txn VALUES (1)").await;
    assert_eq!(one(&mut a, "SELECT COUNT(*) FROM qh_host_txn").await, "1");
    a.close().await.expect("close");
    pool.settle().await;

    let mut b = lease(&*engine, &config).await;
    assert_eq!(
        one(&mut b, "SELECT CONNECTION_ID()").await,
        id,
        "the connection was reused"
    );
    assert_eq!(
        one(&mut b, "SELECT COUNT(*) FROM qh_host_txn").await,
        "0",
        "rolled back, not committed"
    );
    one(&mut b, "DROP TABLE qh_host_txn").await;
    b.close().await.expect("close");
    pool.settle().await;
}

#[tokio::test]
async fn postgres_set_role_and_custom_settings_do_not_survive_a_checkin() {
    let Some(config) = live_pg() else {
        eprintln!("{PG_SKIP}");
        return;
    };
    let host = EngineHost::new();
    let pool = host.pool();
    let engine = pool.engine(Lane::Query, settings(&[]));

    let mut a = lease(&*engine, &config).await;
    let pid = one(&mut a, "SELECT pg_backend_pid()").await;
    let user = one(&mut a, "SELECT current_user").await;
    one(&mut a, "DROP ROLE IF EXISTS qh_host_role").await;
    one(&mut a, "CREATE ROLE qh_host_role").await;
    one(&mut a, "SET ROLE qh_host_role").await;
    one(&mut a, "SET qh_host.flag = 'leaked'").await;
    assert_eq!(one(&mut a, "SELECT current_user").await, "qh_host_role");
    a.close().await.expect("close");
    pool.settle().await;

    let mut b = lease(&*engine, &config).await;
    assert_eq!(
        one(&mut b, "SELECT pg_backend_pid()").await,
        pid,
        "the session was reused"
    );
    assert_eq!(
        one(&mut b, "SELECT current_user").await,
        user,
        "SET ROLE is gone"
    );
    assert_ne!(
        one(&mut b, "SELECT current_setting('qh_host.flag', true)").await,
        "leaked",
        "the custom setting is gone"
    );
    one(&mut b, "DROP ROLE qh_host_role").await;
    b.close().await.expect("close");
    pool.settle().await;
}

// --------------------------------------------------------------------------- //
// L8: Trino, live
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn trino_context_follows_each_run() {
    if std::env::var("QH_TEST_TRINO").as_deref() != Ok("1") {
        eprintln!("skipped: set QH_TEST_TRINO=1 with deploy/dev/up.sh trino running");
        return;
    }
    let host = EngineHost::new();
    let pool = host.pool();
    let engine = pool.engine(Lane::Query, settings(&[]));
    let config = |catalog: &str, schema: &str| {
        ConnectionConfig::new(DriverKind::Trino, "127.0.0.1", 58080, "queryhive")
            .database(catalog)
            .schema(schema)
            .tls(TlsMode::Disable)
    };

    let mut a = lease(&*engine, &config("tpch", "tiny")).await;
    let seen = query(
        &mut a,
        "SELECT current_catalog, current_schema",
        &no_options(),
    )
    .await
    .expect("run A");
    assert_eq!(seen, vec![vec!["tpch".to_owned(), "tiny".to_owned()]]);
    a.close().await.expect("close");
    pool.settle().await;

    let mut b = lease(&*engine, &config("system", "runtime")).await;
    let seen = query(
        &mut b,
        "SELECT current_catalog, current_schema",
        &no_options(),
    )
    .await
    .expect("run B");
    assert_eq!(seen, vec![vec!["system".to_owned(), "runtime".to_owned()]]);
    b.close().await.expect("close");
    pool.settle().await;
    assert_eq!(
        pool.stats().opened,
        1,
        "one session served both catalogs: {:?}",
        pool.stats()
    );
}

// --------------------------------------------------------------------------- //
// L9: SSH, live
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn ssh_one_tunnel_for_preview_objects_and_export() {
    if std::env::var("QH_TEST_SSH").as_deref() != Ok("1")
        || std::env::var("QH_TEST_POSTGRES").as_deref() != Ok("1")
    {
        eprintln!("skipped: set QH_TEST_SSH=1 and QH_TEST_POSTGRES=1 with qh-sshd-run.sh and up.sh running");
        return;
    }
    let key =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/qh-sshd/id_ed25519");
    let target = std::env::var("QH_TEST_SSH_DB_HOST")
        .unwrap_or_else(|_| "host.containers.internal".to_owned());
    let dir = tempfile::tempdir().expect("temp dir");
    let known_hosts = dir.path().join("known_hosts");

    // Trust the container's host key the way a person would: from its fingerprint, in a file
    // of our own (never `~/.ssh`).
    let probe = qh_tunnel::BastionConfig::new(
        "127.0.0.1",
        52222,
        "qh",
        qh_tunnel::Auth::Key {
            path: key.clone(),
            passphrase: None,
        },
        &known_hosts,
    );
    match qh_tunnel::Tunnel::open(&probe, qh_tunnel::Target::new(target.clone(), 55432)).await {
        Err(qh_tunnel::Error::HostKeyUnknown { key: presented, .. }) => {
            qh_tunnel::known_hosts::append(&known_hosts, "127.0.0.1", 52222, presented.blob())
                .expect("record the host key");
        }
        Ok(_) => {}
        Err(error) => {
            eprintln!("skipped: the sshd container is not usable: {error}");
            return;
        }
    }

    let env = settings(&[
        ("SSH_HOST", "127.0.0.1"),
        ("SSH_PORT", "52222"),
        ("SSH_USER", "qh"),
        ("SSH_AUTH_METHOD", "key"),
        ("SSH_KEY_PATH", &key.to_string_lossy()),
        ("SSH_KNOWN_HOSTS", &known_hosts.to_string_lossy()),
    ]);
    let mut config = ConnectionConfig::new(DriverKind::Postgres, target, 55432, "qh")
        .password("qh-dev-only")
        .database("qh")
        .tls(TlsMode::Disable);
    config.tunnel = Some(TunnelConfig {
        host: "127.0.0.1".to_owned(),
        port: 52222,
        user: "qh".to_owned(),
        auth: TunnelAuth::Key(key),
        known_hosts: Some(known_hosts),
        app_known_hosts: None,
        host_key_accept: None,
        alias: None,
    });

    let host = EngineHost::new();
    let pool = host.pool();
    let query_engine = pool.engine(Lane::Query, env.clone());
    let metadata_engine = pool.engine(Lane::Metadata, env.clone());
    let long_engine = pool.long_op_engine(env);

    let mut preview = match query_engine.connect(&config).await {
        Ok(session) => session,
        Err(error) => {
            eprintln!("skipped: the database is not reachable from the bastion: {error}");
            return;
        }
    };
    assert_eq!(one(&mut preview, "SELECT 1").await, "1");
    let mut objects = metadata_engine.connect(&config).await.expect("metadata");
    assert!(objects
        .browse(BrowseLevel::Schema, &ObjectPath::new(), false)
        .await
        .is_ok());
    let mut export = long_engine.connect(&config).await.expect("export");
    assert_eq!(one(&mut export, "SELECT 2").await, "2");
    assert_eq!(
        pool.stats().tunnels_opened,
        1,
        "one tunnel for preview, objects and export"
    );
    for session in [preview, objects, export] {
        session.close().await.expect("close");
    }
    pool.settle().await;
}

// --------------------------------------------------------------------------- //
// L10 - L14: capped previews and the pipelined read, live (commit 2)
// --------------------------------------------------------------------------- //

const PG_ENV: &[(&str, &str)] = &[
    ("DB_KIND", "postgres"),
    ("DB_HOST", "127.0.0.1"),
    ("DB_PORT", "55432"),
    ("DB_USER", "qh"),
    ("DB_PASSWORD", "qh-dev-only"),
    ("DB_DATABASE", "qh"),
    ("DB_SCHEMA", "public"),
    ("DB_SSLMODE", "disable"),
];

const MYSQL_ENV: &[(&str, &str)] = &[
    ("DB_KIND", "mysql"),
    ("DB_HOST", "127.0.0.1"),
    ("DB_PORT", "53306"),
    ("DB_USER", "qh"),
    ("DB_PASSWORD", "qh-dev-only"),
    ("DB_DATABASE", "qh"),
    ("DB_SSLMODE", "disable"),
];

const TRINO_ENV: &[(&str, &str)] = &[
    ("DB_KIND", "trino"),
    ("DB_HOST", "127.0.0.1"),
    ("DB_PORT", "58080"),
    ("DB_USER", "queryhive"),
    ("DB_DATABASE", "tpch"),
    ("DB_SCHEMA", "sf1"),
];

/// A billion rows from a three-way cross join, streaming from the first second on.
const MYSQL_BIG_FROM: &str = "WITH RECURSIVE n AS (SELECT 1 AS x UNION ALL SELECT x + 1 FROM n \
                              WHERE x < 1000) SELECT {} FROM n a, n b, n c";

/// A preview through the host, blocking. Returns its events.
fn host_preview(host: &EngineHost, env: &[(&str, &str)], sql: &str, limit: &str) -> Vec<Json> {
    let mut pairs = env.to_vec();
    pairs.extend([("RETRIES", "0"), ("SQL", sql), ("LIMIT", limit)]);
    run_host(host, EngineCommand::Preview, &pairs)
}

/// Every row the `rows` events carried, in order, as text.
fn preview_rows(events: &[Json]) -> Vec<Vec<String>> {
    events
        .iter()
        .filter(|event| event["event"] == "rows")
        .flat_map(|event| event["data"].as_array().expect("rows are an array").clone())
        .map(|row| {
            row.as_array()
                .expect("a row is an array")
                .iter()
                .map(|cell| cell.as_str().unwrap_or("NULL").to_owned())
                .collect()
        })
        .collect()
}

fn assert_capped(events: &[Json], rows: u64) {
    let done = last_event(events, "done");
    assert_eq!(done["truncated"], true, "{events:?}");
    assert_eq!(done["rows"], rows, "{events:?}");
    assert!(done.get("error").is_none(), "{events:?}");
}

#[test]
fn postgres_truncated_preview_discards_and_refills() {
    if live_pg().is_none() {
        eprintln!("{PG_SKIP}");
        return;
    }
    let host = EngineHost::new();
    let pool = Arc::clone(host.pool());

    let events = host_preview(
        &host,
        PG_ENV,
        "SELECT pg_backend_pid() AS pid, g FROM generate_series(1, 1000000) AS g",
        "10",
    );
    assert_capped(&events, 10);
    let old_pid = preview_rows(&events)[0][0].clone();
    block(async { pool.settle().await });
    let stats = pool.stats();
    assert_eq!(
        stats.closed, 1,
        "the abandoned session was closed: {stats:?}"
    );
    assert_eq!(stats.idle, 1, "and the pool refilled itself: {stats:?}");

    let next = preview_rows(&host_preview(
        &host,
        PG_ENV,
        "SELECT pg_backend_pid()",
        "10",
    ));
    assert_ne!(next[0][0], old_pid, "the next Run got a different backend");
    assert_eq!(
        pool.stats().opened,
        2,
        "the refill served it, nothing else opened"
    );

    // The old backend stopped when its socket closed.
    let check = format!("SELECT count(*) FROM pg_stat_activity WHERE pid = {old_pid}");
    let waiting = Instant::now();
    loop {
        let events = host_preview(&host, PG_ENV, &check, "10");
        if preview_rows(&events)[0][0] == "0" {
            break;
        }
        assert!(
            waiting.elapsed() < Duration::from_secs(1),
            "the capped backend is still there"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    block(async { pool.settle().await });
}

/// Payloads the generic lexer reads as one statement and the server as two. The statement is
/// described before it is sent, so the server refuses it and nothing after the first `;` runs,
/// whichever way the engine's classifier read it. Run on a *reused* session in `read_only` mode.
#[test]
fn postgres_a_read_only_run_cannot_smuggle_a_delete_on_a_reused_session() {
    let Some(config) = live_pg() else {
        eprintln!("{PG_SKIP}");
        return;
    };
    let host = EngineHost::new();
    let pool = Arc::clone(host.pool());
    let engine = pool.engine(Lane::Query, settings(&[]));
    let table = "qh_smuggle_tmp";
    block(async {
        let mut s = lease(&*engine, &config).await;
        for sql in [
            format!("DROP TABLE IF EXISTS {table}"),
            format!("CREATE TABLE {table} AS SELECT g FROM generate_series(1, 5) g"),
        ] {
            one(&mut s, &sql).await;
        }
        s.close().await.expect("close");
        pool.settle().await;
    });
    let count = || -> String {
        block(async {
            let mut s = lease(&*engine, &config).await;
            let n = one(&mut s, &format!("SELECT count(*) FROM {table}")).await;
            s.close().await.expect("close");
            pool.settle().await;
            n
        })
    };
    assert_eq!(count(), "5");

    let payloads = [
        format!("SELECT E'x\\' AS a, '; DELETE FROM {table}; --'"),
        format!("SELECT 1 /* /* */ ' */; DELETE FROM {table}; --'"),
        format!("SELECT 1 AS a$x$; DELETE FROM {table}; --$x$"),
    ];
    for payload in &payloads {
        let mut pairs = PG_ENV.to_vec();
        pairs.extend([
            ("RETRIES", "0"),
            ("SQL", payload.as_str()),
            ("LIMIT", "10"),
            ("SAFE_MODE", "read_only"),
        ]);
        // Warm the key so the payload runs on a reused session.
        let warm = host_preview(&host, PG_ENV, "SELECT 1", "10");
        assert_eq!(last_event(&warm, "done")["rows"], 1, "{warm:?}");
        block(async { pool.settle().await });
        let events = run_host(&host, EngineCommand::Preview, &pairs);
        block(async { pool.settle().await });
        assert_eq!(count(), "5", "{payload} deleted rows: {events:?}");
    }
    block(async {
        let mut s = lease(&*engine, &config).await;
        one(&mut s, &format!("DROP TABLE {table}")).await;
        s.close().await.expect("close");
        pool.settle().await;
    });
}

#[test]
fn postgres_type_zoo_through_the_host_matches_the_cli() {
    if live_pg().is_none() {
        eprintln!("{PG_SKIP}");
        return;
    }
    let sql = "SELECT * FROM type_zoo ORDER BY 1";
    let mut pairs = PG_ENV.to_vec();
    pairs.extend([("RETRIES", "0"), ("SQL", sql), ("LIMIT", "100")]);

    let mut out = Capture::new();
    block(qh_ffi::run(
        Command::Preview,
        &settings(&pairs),
        &mut out,
        &qh_ffi::RealEngine::with_settings(settings(&pairs)),
        &CancelFlag::new(),
    ))
    .expect("the CLI path succeeds");
    let cli = without_timing(out.lines);

    // The first Run opens the session, the second reuses it.
    let host = EngineHost::new();
    for run in 1..=2 {
        let hosted = without_timing(run_host(&host, EngineCommand::Preview, &pairs));
        assert_eq!(hosted, cli, "run {run} through the host");
        block(async { host.pool().settle().await });
    }
    assert_eq!(
        host.pool().stats().reused,
        1,
        "the second Run reused the session"
    );
}

#[test]
fn mysql_capped_preview_keeps_its_session() {
    if live_mysql().is_none() {
        eprintln!("{MYSQL_SKIP}");
        return;
    }
    let host = EngineHost::new();
    let pool = Arc::clone(host.pool());
    let big = MYSQL_BIG_FROM.replace("{}", "CONNECTION_ID() AS id, a.x AS ax");

    let events = host_preview(&host, MYSQL_ENV, &big, "10");
    assert_capped(&events, 10);
    let id = preview_rows(&events)[0][0].clone();
    block(async { pool.settle().await });
    let stats = pool.stats();
    assert_eq!(
        (stats.idle, stats.closed),
        (1, 0),
        "the session was kept: {stats:?}"
    );

    // The next Run is on the same connection, and without a cap the limit is back to the
    // server's default (a capped read sees its own cap in the variable, so this one is uncapped).
    let engine = pool.engine(Lane::Query, settings(&[]));
    let config = live_mysql().expect("guarded above");
    block(async {
        let mut s = lease(&*engine, &config).await;
        let next = query(
            &mut s,
            "SELECT CONNECTION_ID(), @@SESSION.sql_select_limit",
            &no_options(),
        )
        .await
        .expect("the next run");
        assert_eq!(next[0][0], id, "the connection was reused");
        assert_eq!(
            next[0][1], "18446744073709551615",
            "sql_select_limit is back to its default"
        );
        s.close().await.expect("close");
        pool.settle().await;
    });

    // An explicit LIMIT larger than the cap: the rows are right and nothing fails. The server
    // does not stop at the cap here, so the connection is dropped and the session is not kept.
    let explicit = host_preview(&host, MYSQL_ENV, &format!("{} LIMIT 100000", big), "10");
    assert_capped(&explicit, 10);
    block(async { pool.settle().await });
    assert_eq!(
        pool.stats().closed,
        1,
        "the connection the cap stopped was closed: {:?}",
        pool.stats()
    );

    // A limit on a write's SELECT would cut the rows written: none is applied.
    block(async {
        let mut s = lease(&*engine, &config).await;
        for sql in [
            "DROP TABLE IF EXISTS qh_capped_scratch",
            "CREATE TABLE qh_capped_scratch (x INT)",
        ] {
            one(&mut s, sql).await;
        }
        let capped = ExecuteOptions {
            row_limit: Some(10),
            ..ExecuteOptions::default()
        };
        query(
            &mut s,
            "INSERT INTO qh_capped_scratch \
             WITH RECURSIVE n AS (SELECT 1 AS x UNION ALL SELECT x + 1 FROM n WHERE x < 1000) \
             SELECT x FROM n",
            &capped,
        )
        .await
        .expect("the insert runs");
        assert_eq!(
            one(&mut s, "SELECT COUNT(*) FROM qh_capped_scratch").await,
            "1000"
        );
        // The same connection then reads without a limit: the one set for the read is gone.
        assert_eq!(
            query(
                &mut s,
                "SELECT x FROM qh_capped_scratch",
                &ExecuteOptions::default()
            )
            .await
            .expect("a full read")
            .len(),
            1000
        );
        one(&mut s, "DROP TABLE qh_capped_scratch").await;
        s.close().await.expect("close");
        pool.settle().await;
    });
}

#[test]
fn trino_truncated_preview_deletes_its_query() {
    if std::env::var("QH_TEST_TRINO").as_deref() != Ok("1") {
        eprintln!("skipped: set QH_TEST_TRINO=1 with deploy/dev/up.sh trino running");
        return;
    }
    let host = EngineHost::new();
    let pool = Arc::clone(host.pool());
    let marker = format!(
        "qh-trino-capped-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    );
    let events = host_preview(
        &host,
        TRINO_ENV,
        &format!("SELECT * FROM lineitem /* {marker} */"),
        "10",
    );
    assert_capped(&events, 10);

    let check = format!(
        "SELECT count(*) FROM system.runtime.queries WHERE state = 'RUNNING' \
         AND query LIKE '%{marker}%' AND query NOT LIKE '%system.runtime.queries%'"
    );
    let waiting = Instant::now();
    loop {
        let running = preview_rows(&host_preview(&host, TRINO_ENV, &check, "10"));
        if running[0][0] == "0" {
            break;
        }
        assert!(
            waiting.elapsed() < Duration::from_secs(2),
            "the capped query is still running"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    block(async { pool.settle().await });
    let stats = pool.stats();
    assert_eq!(
        (stats.opened, stats.closed),
        (1, 0),
        "one session served every Run and was kept: {stats:?}"
    );
}

/// A capped statement whose connection dies after its text was sent is not sent again, neither
/// by the pool's reconnect rule nor by the retry policy: a function with a side effect ran once.
#[test]
fn mysql_a_connection_lost_after_the_statement_was_sent_is_not_resent() {
    let Some(config) = live_mysql() else {
        eprintln!("{MYSQL_SKIP}");
        return;
    };
    let host = EngineHost::new();
    let pool = Arc::clone(host.pool());
    let engine = pool.engine(Lane::Query, settings(&[]));
    block(async {
        let mut s = lease(&*engine, &config).await;
        for sql in [
            "DROP TABLE IF EXISTS qh_kill_counter",
            // Not transactional: what the killed statement wrote stays written.
            "CREATE TABLE qh_kill_counter (n INT, s INT) ENGINE=MyISAM",
        ] {
            one(&mut s, sql).await;
        }
        s.close().await.expect("close");
        pool.settle().await;

        // An idle session in the pool, so the next lease is a *reused* one on its first call.
        let mut warm = lease(&*engine, &config).await;
        let id = one(&mut warm, "SELECT CONNECTION_ID()").await;
        warm.close().await.expect("close");
        pool.settle().await;

        let killer_engine = pool.engine(Lane::Metadata, settings(&[]));
        let killer_config = config.clone();
        let killer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            let mut k = lease(&*killer_engine, &killer_config).await;
            let _ = query(&mut k, &format!("KILL {id}"), &no_options()).await;
            k.close().await.expect("close");
        });

        let mut s = lease(&*engine, &config).await;
        let capped = ExecuteOptions {
            row_limit: Some(10),
            ..ExecuteOptions::default()
        };
        let policy = RetryPolicy::new(2, Duration::from_millis(1));
        // 300 rows at 10 ms each: killed after 1.5 s it has written about half of them, and a
        // second run of it would write 300 more.
        let insert = "INSERT INTO qh_kill_counter \
                      WITH RECURSIVE n AS (SELECT 1 AS x UNION ALL SELECT x + 1 FROM n \
                      WHERE x < 300) SELECT x, SLEEP(0.01) FROM n";
        let outcome = retry::execute(&mut s, &policy, insert, &capped).await;
        let error = match outcome {
            Ok(mut cursor) => cursor
                .next_batch(10)
                .await
                .expect_err("the statement was killed"),
            Err(error) => error,
        };
        assert_eq!(error.code(), Some("2013"), "{error:?}");
        killer.await.expect("killer");
        s.close().await.expect("close");
        pool.settle().await;

        let mut check = lease(&*engine, &config).await;
        let written: usize = one(&mut check, "SELECT COUNT(*) FROM qh_kill_counter")
            .await
            .parse()
            .expect("a count");
        assert!(
            written < 300,
            "the statement was cut short and not run again: {written} rows"
        );
        one(&mut check, "DROP TABLE qh_kill_counter").await;
        check.close().await.expect("close");
        pool.settle().await;
    });
}

// --------------------------------------------------------------------------- //
// W11-T1: the metadata commands against each dev server, through the host
// --------------------------------------------------------------------------- //

/// What one live round of the metadata commands saw: the three events it asked for, taken while
/// the objects existed, and what the teardown said after.
struct MetadataRound {
    kinds: Vec<Json>,
    columns: Vec<Json>,
    ddl: Vec<Json>,
}

/// Create objects, ask the three metadata commands about them, and drop them again **before**
/// anything is asserted, so a failed assertion leaves nothing behind on the dev server.
fn metadata_round(
    env: &[(&str, &str)],
    setup: &[&str],
    teardown: &[&str],
    listing: &[(&str, &str)],
    table: &[(&str, &str)],
    view: &[(&str, &str)],
) -> MetadataRound {
    let host = EngineHost::new();
    let sql = |statement: &str| {
        let events = host_preview(&host, env, statement, "10");
        assert!(
            events.iter().all(|event| event["event"] != "error"),
            "{statement}: {events:?}"
        );
    };
    let ask = |command: EngineCommand, extra: &[(&str, &str)]| {
        let mut pairs = env.to_vec();
        pairs.push(("RETRIES", "0"));
        pairs.extend_from_slice(extra);
        run_host(&host, command, &pairs)
    };
    // A leftover from an earlier run that died between setup and teardown is cleared first.
    for statement in teardown {
        let _ = host_preview(&host, env, statement, "10");
    }
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for statement in setup {
            sql(statement);
        }
        let mut with_kinds = listing.to_vec();
        with_kinds.push(("OBJECT_KINDS", "1"));
        MetadataRound {
            kinds: ask(EngineCommand::Tables, &with_kinds),
            columns: ask(EngineCommand::Columns, table),
            ddl: ask(EngineCommand::Ddl, view),
        }
    }));
    for statement in teardown {
        let _ = host_preview(&host, env, statement, "10");
    }
    block(async { host.pool().settle().await });
    outcome.unwrap_or_else(|panic| std::panic::resume_unwind(panic))
}

/// The kind a listing gave one name.
fn kind_of(listing: &[Json], name: &str) -> Option<String> {
    let event = last_event(listing, "tables");
    let position = event["names"]
        .as_array()?
        .iter()
        .position(|candidate| candidate == name)?;
    event["kinds"][position].as_str().map(str::to_owned)
}

#[test]
fn postgres_metadata_names_kinds_columns_and_ddl_through_the_host() {
    if std::env::var("QH_TEST_POSTGRES").as_deref() != Ok("1") {
        eprintln!("{PG_SKIP}");
        return;
    }
    let round = metadata_round(
        PG_ENV,
        &[
            "CREATE SCHEMA qh_w11t1",
            "CREATE TABLE qh_w11t1.t (id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY, \
             label text NOT NULL DEFAULT 'x', doubled bigint GENERATED ALWAYS AS (id * 2) STORED)",
            "CREATE VIEW qh_w11t1.v AS SELECT id FROM qh_w11t1.t",
            "CREATE MATERIALIZED VIEW qh_w11t1.mv AS SELECT id FROM qh_w11t1.t",
        ],
        &["DROP SCHEMA IF EXISTS qh_w11t1 CASCADE"],
        &[("DB_SCHEMA", "qh_w11t1")],
        &[("TARGET_SCHEMA", "qh_w11t1"), ("TARGET_TABLE", "t")],
        &[("TARGET_SCHEMA", "qh_w11t1"), ("TARGET_TABLE", "v")],
    );
    assert_eq!(kind_of(&round.kinds, "t").as_deref(), Some("table"));
    assert_eq!(kind_of(&round.kinds, "v").as_deref(), Some("view"));
    assert_eq!(
        kind_of(&round.kinds, "mv").as_deref(),
        Some("materialized_view"),
        "the kind `information_schema` cannot see at all"
    );

    let fields = &last_event(&round.columns, "table_columns")["fields"];
    assert_eq!(fields[0]["name"], "id");
    assert_eq!(fields[0]["extra"], "identity");
    assert_eq!(fields[0]["nullable"], false);
    assert_eq!(fields[1]["default"], "'x'::text");
    assert_eq!(fields[2]["extra"], "generated");

    let ddl = last_event(&round.ddl, "table_ddl");
    assert_eq!(ddl["object_kind"], "view", "{ddl}");
    assert!(
        ddl["ddl"]
            .as_str()
            .unwrap()
            .contains("CREATE VIEW qh_w11t1.v AS"),
        "{ddl}"
    );
}

#[test]
fn mysql_metadata_names_kinds_columns_and_ddl_through_the_host() {
    if std::env::var("QH_TEST_MYSQL").as_deref() != Ok("1") {
        eprintln!("{MYSQL_SKIP}");
        return;
    }
    let round = metadata_round(
        MYSQL_ENV,
        &[
            "CREATE TABLE qh_w11t1_t (id bigint unsigned NOT NULL AUTO_INCREMENT PRIMARY KEY, \
             label varchar(20) NOT NULL DEFAULT 'x')",
            "CREATE VIEW qh_w11t1_v AS SELECT id FROM qh_w11t1_t",
        ],
        &[
            "DROP VIEW IF EXISTS qh_w11t1_v",
            "DROP TABLE IF EXISTS qh_w11t1_t",
        ],
        &[],
        &[("TARGET_CATALOG", "qh"), ("TARGET_TABLE", "qh_w11t1_t")],
        &[("TARGET_CATALOG", "qh"), ("TARGET_TABLE", "qh_w11t1_v")],
    );
    assert_eq!(
        kind_of(&round.kinds, "qh_w11t1_t").as_deref(),
        Some("table")
    );
    assert_eq!(kind_of(&round.kinds, "qh_w11t1_v").as_deref(), Some("view"));

    let fields = &last_event(&round.columns, "table_columns")["fields"];
    assert_eq!(fields[0]["name"], "id");
    assert_eq!(fields[0]["type"], "bigint unsigned");
    assert_eq!(fields[0]["extra"], "auto_increment");
    assert_eq!(fields[1]["default"], "x");

    let ddl = last_event(&round.ddl, "table_ddl");
    assert_eq!(
        ddl["object_kind"], "view",
        "read from the first column's name: {ddl}"
    );
    assert!(
        ddl["ddl"].as_str().unwrap().contains("VIEW `qh_w11t1_v`"),
        "{ddl}"
    );
}

#[test]
fn trino_metadata_names_kinds_columns_and_ddl_through_the_host() {
    if std::env::var("QH_TEST_TRINO").as_deref() != Ok("1") {
        eprintln!("skipped: set QH_TEST_TRINO=1 with deploy/dev/up.sh trino running");
        return;
    }
    let env: Vec<(&str, &str)> = TRINO_ENV
        .iter()
        .copied()
        .filter(|(key, _)| *key != "DB_DATABASE" && *key != "DB_SCHEMA")
        .chain([("DB_DATABASE", "memory"), ("DB_SCHEMA", "qh_w11t1")])
        .collect();
    let round = metadata_round(
        &env,
        &[
            "CREATE SCHEMA memory.qh_w11t1",
            "CREATE TABLE memory.qh_w11t1.t AS SELECT 1 AS id, 'x' AS label",
            "CREATE VIEW memory.qh_w11t1.v AS SELECT id FROM memory.qh_w11t1.t",
        ],
        &[
            "DROP VIEW IF EXISTS memory.qh_w11t1.v",
            "DROP TABLE IF EXISTS memory.qh_w11t1.t",
            "DROP SCHEMA IF EXISTS memory.qh_w11t1",
        ],
        &[],
        &[
            ("TARGET_CATALOG", "memory"),
            ("TARGET_SCHEMA", "qh_w11t1"),
            ("TARGET_TABLE", "t"),
        ],
        &[
            ("TARGET_CATALOG", "memory"),
            ("TARGET_SCHEMA", "qh_w11t1"),
            ("TARGET_TABLE", "v"),
        ],
    );
    assert_eq!(kind_of(&round.kinds, "t").as_deref(), Some("table"));
    assert_eq!(kind_of(&round.kinds, "v").as_deref(), Some("view"));

    let fields = &last_event(&round.columns, "table_columns")["fields"];
    assert_eq!(fields[0]["name"], "id");
    assert_eq!(fields[0]["extra"], "");

    let ddl = last_event(&round.ddl, "table_ddl");
    assert_eq!(ddl["object_kind"], "view", "{ddl}");
    assert!(
        ddl["ddl"].as_str().unwrap().contains("CREATE VIEW"),
        "{ddl}"
    );
}

/// W11-T2b2, live (L-1 to L-4 through the engine's own settings): a `~/.ssh/config` alias
/// resolves to the container, an unknown host is refused with a structured prompt, the
/// fingerprint a person confirmed is recorded in the app's file and accepted, and a wrong pin
/// is refused and records nothing.
#[tokio::test]
async fn ssh_alias_and_pinned_first_use_through_the_engines_settings() {
    if std::env::var("QH_TEST_SSH").as_deref() != Ok("1") {
        eprintln!("skipped: set QH_TEST_SSH=1 with the qh-sshd-dev container on 52222");
        return;
    }
    let key = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/qh-sshd/id_ed25519")
        .canonicalize()
        .expect("the dev sshd key");
    let dir = tempfile::tempdir().expect("temp dir");
    let config = dir.path().join("ssh_config");
    std::fs::write(
        &config,
        format!(
            "Host qh-dev\n  HostName 127.0.0.1\n  Port 52222\n  User qh\n  IdentityFile {}\n",
            key.display()
        ),
    )
    .expect("write the config");
    let (user_file, app_file) = (
        dir.path().join("user_known_hosts"),
        dir.path().join("app_known_hosts"),
    );
    std::fs::write(&user_file, "").expect("an empty user file");

    let config_text = config.to_string_lossy().into_owned();
    let user_text = user_file.to_string_lossy().into_owned();
    let app_text = app_file.to_string_lossy().into_owned();
    let settings_with = |extra: &[(&str, &str)]| {
        let mut all = vec![
            ("SSH_HOST", "qh-dev"),
            ("SSH_USE_CONFIG", "1"),
            ("SSH_AUTH_METHOD", "key"),
            ("SSH_CONFIG_PATH", config_text.as_str()),
            ("SSH_KNOWN_HOSTS", user_text.as_str()),
            ("SSH_APP_KNOWN_HOSTS", app_text.as_str()),
        ];
        all.extend_from_slice(extra);
        settings(&all)
    };
    let open = |given: Settings| async move {
        let description = qh_ffi::tunnel::settings(&given)
            .expect("the settings describe a bastion")
            .expect("a bastion");
        let target = qh_tunnel::Target::new("127.0.0.1", 5432);
        let opened = qh_ffi::tunnel::open(&description, &given, target).await;
        (description, opened)
    };
    let app_file_is_empty =
        || !app_file.exists() || std::fs::read_to_string(&app_file).unwrap().is_empty();

    // L-1: the alias resolved, and a host nobody recorded is refused with what a sheet needs.
    let (description, refused) = open(settings_with(&[])).await;
    assert_eq!(
        (description.host.as_str(), description.port),
        ("127.0.0.1", 52222)
    );
    assert_eq!(description.alias.as_deref(), Some("qh-dev"));
    assert_eq!(description.auth, TunnelAuth::Key(key.clone()));
    let refused = match refused {
        Err(error) => error,
        Ok(_) => panic!("an unknown host must be refused"),
    };
    let failure = refused.host_key().expect("a structured refusal").clone();
    assert_eq!(failure.state, qh_core::HostKeyState::Unknown);
    assert_eq!(failure.alias.as_deref(), Some("qh-dev"));
    assert_eq!(refused.failure_kind(), FailureKind::Permanent);
    let fingerprint = failure.fingerprint.clone().expect("a fingerprint to show");
    assert!(app_file_is_empty(), "a refusal records nothing");

    // L-2: a pin that is not the server's key is refused, and nothing is recorded.
    let wrong = "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let (_, mismatch) = open(settings_with(&[("SSH_HOST_KEY_ACCEPT", wrong)])).await;
    let mismatch = match mismatch {
        Err(error) => error,
        Ok(_) => panic!("a pin that is not the presented key must not be accepted"),
    };
    let failure = mismatch.host_key().expect("a structured refusal");
    assert_eq!(failure.state, qh_core::HostKeyState::PinMismatch);
    assert_eq!(failure.pinned.as_deref(), Some(wrong));
    assert_eq!(failure.fingerprint.as_deref(), Some(fingerprint.as_str()));
    assert!(app_file_is_empty(), "a refused pin records nothing");

    // L-3: the fingerprint the person was shown is accepted, recorded first, and works.
    let (_, accepted) = open(settings_with(&[("SSH_HOST_KEY_ACCEPT", &fingerprint)])).await;
    let tunnel = accepted.unwrap_or_else(|error| panic!("the confirmed key must open: {error}"));
    std::net::TcpStream::connect(("127.0.0.1", tunnel.local_port()))
        .expect("the forwarded port is listening");
    let recorded = std::fs::read_to_string(&app_file).expect("the app's file was written");
    assert!(recorded.contains("ssh-ed25519"), "{recorded}");
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&app_file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "the app's file is private");
    }
    tunnel.close().await;

    // L-4: no pin needed any more, and the user's own file was never written.
    let (_, again) = open(settings_with(&[])).await;
    again
        .unwrap_or_else(|error| panic!("a recorded host opens without a pin: {error}"))
        .close()
        .await;
    assert_eq!(std::fs::read_to_string(&user_file).unwrap(), "");
}
