//! The session pool behind the engine host (blueprint `fase-2-engine-host.md` §4).
//!
//! One `Entry` per connection identity holds up to [`IDLE_MAX`] idle sessions, and counts what is
//! leased per lane. **It is a reservation, not a queue**: a checkout never waits for another Run
//! to finish. The one wait is [`CHECKIN_WAIT`], for a session that is being reset right now,
//! because one round trip of reset is cheaper than a connect. When the lane is full a new session
//! is opened outside the reservation, at the cost every Run paid before there was a pool.
//!
//! Three rules keep a pooled session honest:
//!
//! * a session is reset in the background when it comes back, and one whose reset fails is closed;
//! * a session that was cancelled, broke, was left mid-stream or whose credentials were retired is
//!   closed without a reset, so a late cancel can never land on the next statement;
//! * the credential is part of the pool key (as a keyed hash), so a changed password never finds a
//!   session opened with the old one.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::hash::{BuildHasher, Hash, Hasher, RandomState};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use qh_core::{EngineError, FailureKind};
use qh_driver::{ConnectionConfig, DriverKind, Session, TlsMode, TunnelAuth, TunnelConfig};

use super::lease::{Fate, Held, LeaseState};
use super::{Connector, TunnelHandle};
use crate::env::Settings;
use crate::Engine;

/// Sessions kept per key for query work: preview, explain, count, apply and table operations.
pub const QUERY_SESSIONS: usize = 2;
/// Sessions kept per key for the object tree.
pub const METADATA_SESSIONS: usize = 1;
/// The most idle sessions a key keeps. A checkin beyond it closes the session.
pub const IDLE_MAX: usize = 3;
/// How long a checkout waits for a session that is being reset, before it opens its own.
pub const CHECKIN_WAIT: Duration = Duration::from_millis(250);
/// How long a reset may take before the session is closed instead.
pub const RESET_BUDGET: Duration = Duration::from_secs(2);
/// How often idle sessions are looked at for expiry.
pub const EVICT_TICK: Duration = Duration::from_secs(30);
/// How long a session may sit idle.
pub const IDLE_TTL: Duration = Duration::from_secs(300);
/// How long opening a tunnel may take. `qh-tunnel` has no connect timeout of its own, and a
/// bastion that hangs would otherwise hold every Run of its key until the user pressed Stop.
pub const TUNNEL_OPEN: Duration = Duration::from_secs(30);

/// Which class of work a session was borrowed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lane {
    /// Preview, explain, count, apply and table operations.
    Query,
    /// The object tree.
    Metadata,
}

impl Lane {
    fn index(self) -> usize {
        match self {
            Lane::Query => 0,
            Lane::Metadata => 1,
        }
    }

    fn cap(self) -> usize {
        match self {
            Lane::Query => QUERY_SESSIONS,
            Lane::Metadata => METADATA_SESSIONS,
        }
    }
}

// --------------------------------------------------------------------------- //
// the key
// --------------------------------------------------------------------------- //

/// What separates one pooled connection from another, without the credentials.
///
/// Everything a session is bound to by the server goes here, and nothing a Run can change on a
/// live session does (SQL, limits, timeouts, Safe Mode, retries).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Identity {
    kind: DriverKind,
    host: String,
    port: u16,
    user: String,
    tls: TlsMode,
    insecure: bool,
    database: DatabaseSlot,
    tunnel: Option<TunnelIdentity>,
}

/// How the database name enters the key.
///
/// Only a PostgreSQL connection is bound to its database. MySQL moves with `USE` (so what
/// matters is whether there is a database at all: a session that started without one cannot go
/// back to having none), and a Trino session names its catalog on every request.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum DatabaseSlot {
    Named(String),
    Unset,
    Switchable,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct TunnelIdentity {
    host: String,
    port: u16,
    user: String,
    auth: AuthIdentity,
    known_hosts: Option<std::path::PathBuf>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum AuthIdentity {
    Agent,
    Key(std::path::PathBuf),
    Password,
}

/// The pool key: an [`Identity`] and a keyed hash of every secret that opened the connection.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PoolKey {
    identity: Identity,
    credential: u64,
}

impl PoolKey {
    /// The key of the connection `config` describes.
    ///
    /// **Both configs are taken apart without `..`.** A field added to `ConnectionConfig` or
    /// `TunnelConfig` (a per-connection CA, a Trino JWT) stops this function compiling until it
    /// is placed in the identity, in the credential hash, or explicitly ruled out, so a session
    /// opened with old trust or an old secret cannot be reused by accident.
    ///
    /// The password comes from the resolved config (a password inside `DB_URL` lands there), the
    /// bastion password from its tunnel description, and only the key passphrase from settings,
    /// where `tunnel::bastion` reads it too.
    pub fn of(config: &ConnectionConfig, settings: &Settings) -> Self {
        let ConnectionConfig {
            kind,
            host,
            port,
            user,
            password,
            database,
            // The schema is per-Run context (`Session::set_context`), never session state.
            schema: _,
            tls,
            insecure,
            tunnel,
        } = config;

        let database = match kind {
            DriverKind::Postgres => DatabaseSlot::Named(database.clone().unwrap_or_default()),
            DriverKind::Mysql if database.is_some() => DatabaseSlot::Switchable,
            DriverKind::Mysql => DatabaseSlot::Unset,
            DriverKind::Trino => DatabaseSlot::Switchable,
        };

        let mut hasher = credential_hasher();
        hash_secret(&mut hasher, password.as_deref());

        let tunnel = tunnel.as_ref().map(|tunnel| {
            let TunnelConfig {
                host,
                port,
                user,
                auth,
                known_hosts,
            } = tunnel;
            let auth = match auth {
                TunnelAuth::Agent => AuthIdentity::Agent,
                TunnelAuth::Key(path) => {
                    let passphrase = settings.raw("SSH_KEY_PASSPHRASE", "");
                    hash_secret(
                        &mut hasher,
                        Some(passphrase.as_str()).filter(|p| !p.is_empty()),
                    );
                    AuthIdentity::Key(path.clone())
                }
                TunnelAuth::Password(password) => {
                    hash_secret(&mut hasher, Some(password.as_str()));
                    AuthIdentity::Password
                }
            };
            TunnelIdentity {
                host: host.clone(),
                port: *port,
                user: user.clone(),
                auth,
                known_hosts: known_hosts.clone(),
            }
        });

        Self {
            identity: Identity {
                kind: *kind,
                host: host.clone(),
                port: *port,
                user: user.clone(),
                tls: *tls,
                insecure: *insecure,
                database,
                tunnel,
            },
            credential: hasher.finish(),
        }
    }
}

impl fmt::Debug for PoolKey {
    /// The identity in full, the credentials not at all: not even their hash.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PoolKey")
            .field("identity", &self.identity)
            .field("credential", &format_args!("<hashed>"))
            .finish()
    }
}

/// A SipHash keyed once per process, so the hash means something only inside this process and
/// is never comparable across runs or written anywhere.
fn credential_hasher() -> std::hash::DefaultHasher {
    static STATE: OnceLock<RandomState> = OnceLock::new();
    STATE.get_or_init(RandomState::new).build_hasher()
}

/// One secret, as (present, length, bytes), so `("ab", "")` and `("a", "b")` cannot collide.
fn hash_secret(hasher: &mut impl Hasher, secret: Option<&str>) {
    match secret {
        None => hasher.write_u8(0),
        Some(secret) => {
            hasher.write_u8(1);
            hasher.write_usize(secret.len());
            hasher.write(secret.as_bytes());
        }
    }
}

// --------------------------------------------------------------------------- //
// the entry
// --------------------------------------------------------------------------- //

pub(crate) struct Idle {
    session: Box<dyn Session>,
    tunnel: Option<Arc<dyn TunnelHandle>>,
    since: Instant,
}

#[derive(Default)]
struct EntryState {
    /// Most recently returned last, so a checkout takes the warmest session.
    idle: Vec<Idle>,
    leased: [usize; 2],
    /// Sessions on their way to idle: being reset, refilled or warmed up.
    pending: usize,
    /// The credential changed under this identity: nothing here is reused any more.
    retired: bool,
}

/// One identity's sessions and its shared tunnel.
pub(crate) struct Entry {
    credential: u64,
    state: Mutex<EntryState>,
    /// Opened at most once, under this lock, so two checkouts at once cannot open two tunnels.
    tunnel: tokio::sync::Mutex<Option<Arc<dyn TunnelHandle>>>,
}

impl Entry {
    fn new(credential: u64) -> Arc<Self> {
        Arc::new(Self {
            credential,
            state: Mutex::new(EntryState::default()),
            tunnel: tokio::sync::Mutex::new(None),
        })
    }

    fn state(&self) -> MutexGuard<'_, EntryState> {
        // A panic while the lock was held cannot leave counters half-updated (each update is one
        // statement), so a poisoned lock is still a usable one.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn is_retired(&self) -> bool {
        self.state().retired
    }
}

/// The lane count of one lease. Dropping it gives the count back, including when the future that
/// was still opening the session is dropped by a Stop.
pub(crate) struct LeaseGuard {
    pool: Arc<SessionPool>,
    entry: Arc<Entry>,
    lane: Lane,
    live: bool,
}

impl LeaseGuard {
    /// The lease ends and its session starts going back, in one step under one lock: a checkout
    /// that looks in between must see a session on its way, not a lane with room and nothing in it.
    fn into_pending(mut self) -> PendingGuard {
        self.live = false;
        {
            let mut state = self.entry.state();
            let lane = self.lane.index();
            state.leased[lane] = state.leased[lane].saturating_sub(1);
            state.pending += 1;
        }
        PendingGuard {
            pool: Arc::clone(&self.pool),
            entry: Arc::clone(&self.entry),
        }
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        if self.live {
            let mut state = self.entry.state();
            let lane = self.lane.index();
            state.leased[lane] = state.leased[lane].saturating_sub(1);
            drop(state);
            self.pool.changed.notify_waiters();
        }
    }
}

/// A session on its way to idle.
pub(crate) struct PendingGuard {
    pool: Arc<SessionPool>,
    entry: Arc<Entry>,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        let mut state = self.entry.state();
        state.pending = state.pending.saturating_sub(1);
        drop(state);
        self.pool.changed.notify_waiters();
    }
}

/// Counts a detached task, so [`SessionPool::settle`] can wait for all of them.
struct Background(Arc<SessionPool>);

impl Drop for Background {
    fn drop(&mut self) {
        self.0.background.fetch_sub(1, Ordering::SeqCst);
        self.0.changed.notify_waiters();
    }
}

// --------------------------------------------------------------------------- //
// the pool
// --------------------------------------------------------------------------- //

/// What a test may observe.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PoolStats {
    pub idle: usize,
    pub leased_query: usize,
    pub leased_metadata: usize,
    pub pending: usize,
    /// Sessions opened, for a lease or a warm-up.
    pub opened: usize,
    /// Leases served by an idle session.
    pub reused: usize,
    /// Sessions the pool closed (discarded, expired, or over the idle cap).
    pub closed: usize,
    pub tunnels_opened: usize,
}

pub struct SessionPool {
    pub(crate) connector: Arc<dyn Connector>,
    entries: Mutex<HashMap<Identity, Arc<Entry>>>,
    /// Signalled whenever a session reaches idle, a lease ends or a background task finishes.
    changed: tokio::sync::Notify,
    background: AtomicUsize,
    opened: AtomicUsize,
    reused: AtomicUsize,
    closed: AtomicUsize,
    tunnels_opened: AtomicUsize,
    evictor: AtomicBool,
}

impl SessionPool {
    pub fn new(connector: Arc<dyn Connector>) -> Arc<Self> {
        Arc::new(Self {
            connector,
            entries: Mutex::new(HashMap::new()),
            changed: tokio::sync::Notify::new(),
            background: AtomicUsize::new(0),
            opened: AtomicUsize::new(0),
            reused: AtomicUsize::new(0),
            closed: AtomicUsize::new(0),
            tunnels_opened: AtomicUsize::new(0),
            evictor: AtomicBool::new(false),
        })
    }

    /// An engine that borrows its sessions from this pool, for the `lane`'s kind of work.
    pub fn engine(self: &Arc<Self>, lane: Lane, settings: Settings) -> Box<dyn Engine> {
        Box::new(super::PooledEngine {
            pool: Arc::clone(self),
            lane,
            settings,
        })
    }

    /// An engine for the long operations (export, `to_table`, import): a session of its own that
    /// is never pooled, on the tunnel the key already shares.
    pub fn long_op_engine(self: &Arc<Self>, settings: Settings) -> Box<dyn Engine> {
        Box::new(super::LongOpEngine {
            pool: Arc::clone(self),
            settings,
        })
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<Identity, Arc<Entry>>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn stats(&self) -> PoolStats {
        let mut stats = PoolStats {
            opened: self.opened.load(Ordering::SeqCst),
            reused: self.reused.load(Ordering::SeqCst),
            closed: self.closed.load(Ordering::SeqCst),
            tunnels_opened: self.tunnels_opened.load(Ordering::SeqCst),
            ..PoolStats::default()
        };
        for entry in self.entries().values() {
            let state = entry.state();
            stats.idle += state.idle.len();
            stats.leased_query += state.leased[Lane::Query.index()];
            stats.leased_metadata += state.leased[Lane::Metadata.index()];
            stats.pending += state.pending;
        }
        stats
    }

    /// Wait until every background task (reset, close, warm-up) has finished.
    pub async fn settle(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.background.load(Ordering::SeqCst) == 0 {
                return;
            }
            // A signal can only be missed between the check and the wait, which `enable` closes;
            // the timeout is a belt for a runtime that dropped a task without running its guard.
            let _ = tokio::time::timeout(Duration::from_millis(50), changed).await;
        }
    }

    /// The entry for this key. A key whose credential differs from the entry's retires it: its
    /// idle sessions are closed now and the sessions still out are closed when they return.
    fn entry_for(self: &Arc<Self>, key: &PoolKey) -> Arc<Entry> {
        let mut retired: Vec<Idle> = Vec::new();
        let entry = {
            let mut entries = self.entries();
            match entries.get(&key.identity) {
                Some(entry) if entry.credential == key.credential => Arc::clone(entry),
                other => {
                    if let Some(old) = other {
                        let mut state = old.state();
                        state.retired = true;
                        retired = std::mem::take(&mut state.idle);
                    }
                    let entry = Entry::new(key.credential);
                    entries.insert(key.identity.clone(), Arc::clone(&entry));
                    entry
                }
            }
        };
        self.close_in_background(retired);
        entry
    }

    /// Borrow a session for `config`, from the idle ones when there is room in the lane.
    pub(crate) async fn checkout(
        self: &Arc<Self>,
        config: &ConnectionConfig,
        settings: &Settings,
        lane: Lane,
    ) -> Result<Box<Held>, EngineError> {
        let key = PoolKey::of(config, settings);
        let entry = self.entry_for(&key);

        enum Take {
            Reuse(Idle),
            Open,
        }
        let mut waited = false;
        let (taken, guard) = loop {
            // Registered before the state is read, so a session that reaches idle in between
            // still wakes the wait below.
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();

            let (decision, dead) = {
                let mut state = entry.state();
                // A session whose tunnel died is not worth handing out.
                let (dead, alive): (Vec<_>, Vec<_>) = std::mem::take(&mut state.idle)
                    .into_iter()
                    .partition(|idle| idle.tunnel.as_ref().is_some_and(|t| !t.is_alive()));
                state.idle = alive;

                let index = lane.index();
                let room = state.leased[index] < lane.cap();
                // `None` is "wait for a session that is being reset".
                let decision = if let Some(idle) = room.then(|| state.idle.pop()).flatten() {
                    state.leased[index] += 1;
                    Some(Take::Reuse(idle))
                } else if room && !waited && state.pending > 0 {
                    None
                } else {
                    state.leased[index] += 1;
                    Some(Take::Open)
                };
                (decision, dead)
            };
            self.close_in_background(dead);

            let Some(taken) = decision else {
                let _ = tokio::time::timeout(CHECKIN_WAIT, changed).await;
                waited = true;
                continue;
            };
            // No await between the count and the guard, so a dropped future cannot leak it.
            let guard = LeaseGuard {
                pool: Arc::clone(self),
                entry: Arc::clone(&entry),
                lane,
                live: true,
            };
            break (taken, guard);
        };

        let (mut session, tunnel, reused) = match taken {
            Take::Reuse(idle) => {
                self.reused.fetch_add(1, Ordering::SeqCst);
                (idle.session, idle.tunnel, true)
            }
            Take::Open => {
                let (session, tunnel) = self.open_session(&entry, config, settings).await?;
                (session, tunnel, false)
            }
        };
        // Always, even with `None`: nothing of the previous Run's context may carry over.
        session.set_context(config.database.as_deref(), config.schema.as_deref());

        Ok(Box::new(Held::leased(
            session,
            tunnel,
            LeaseState::new(
                Arc::clone(self),
                entry,
                guard,
                config.clone(),
                settings.clone(),
                reused,
            ),
        )))
    }

    /// Open a session that is not pooled (export, import, `to_table`): its own connection, on the
    /// tunnel its key shares.
    pub(crate) async fn open_unleased(
        self: &Arc<Self>,
        config: &ConnectionConfig,
        settings: &Settings,
    ) -> Result<Box<Held>, EngineError> {
        let entry = self.entry_for(&PoolKey::of(config, settings));
        let (session, tunnel) = self.open_session(&entry, config, settings).await?;
        Ok(Box::new(Held::new(session, tunnel)))
    }

    /// A new session and the tunnel it rides on, opening the tunnel first when the key has none
    /// that is alive.
    pub(crate) async fn open_session(
        self: &Arc<Self>,
        entry: &Arc<Entry>,
        config: &ConnectionConfig,
        settings: &Settings,
    ) -> Result<(Box<dyn Session>, Option<Arc<dyn TunnelHandle>>), EngineError> {
        let tunnel = self.ensure_tunnel(entry, config, settings).await?;
        let session = self.connector.connect(config, tunnel.as_ref()).await?;
        self.opened.fetch_add(1, Ordering::SeqCst);
        Ok((session, tunnel))
    }

    /// The key's tunnel: the one it has when it is alive, a new one otherwise.
    ///
    /// A dead tunnel takes the sessions that were opened through it with it: their sockets end at
    /// a port nothing listens on any more.
    async fn ensure_tunnel(
        self: &Arc<Self>,
        entry: &Arc<Entry>,
        config: &ConnectionConfig,
        settings: &Settings,
    ) -> Result<Option<Arc<dyn TunnelHandle>>, EngineError> {
        if config.tunnel.is_none() {
            return Ok(None);
        }
        let mut slot = entry.tunnel.lock().await;
        if let Some(tunnel) = slot.as_ref() {
            if tunnel.is_alive() {
                return Ok(Some(Arc::clone(tunnel)));
            }
            *slot = None;
            let idle = std::mem::take(&mut entry.state().idle);
            self.close_in_background(idle);
        }
        let opened =
            tokio::time::timeout(TUNNEL_OPEN, self.connector.open_tunnel(config, settings))
                .await
                .map_err(|_| EngineError::Connect {
                    message: format!(
                        "the SSH tunnel did not open within {} s",
                        TUNNEL_OPEN.as_secs()
                    ),
                    kind: FailureKind::Transient,
                })??;
        self.tunnels_opened.fetch_add(1, Ordering::SeqCst);
        *slot = Some(Arc::clone(&opened));
        Ok(Some(opened))
    }

    /// Take a lease's session back: discarded when it cannot be trusted, reset and kept otherwise.
    ///
    /// Returns at once. The reset runs in the background, so the Run that ended never waits for
    /// it and the events it emitted keep their order.
    ///
    /// `refill` names the connection to open in the background when the session is discarded
    /// because its result was left unread (a capped PostgreSQL preview): the next Run should
    /// find a session again instead of paying a connect. It is used only when the key has no
    /// idle session and none on its way.
    pub(crate) fn checkin(
        self: &Arc<Self>,
        entry: &Arc<Entry>,
        guard: LeaseGuard,
        session: Box<dyn Session>,
        tunnel: Option<Arc<dyn TunnelHandle>>,
        fate: Fate,
        refill: Option<(ConnectionConfig, Settings)>,
    ) {
        let pending = guard.into_pending();
        let pool = Arc::clone(self);
        let entry = Arc::clone(entry);
        let work = async move {
            let discard = fate != Fate::Reset
                || entry.is_retired()
                || tunnel.as_ref().is_some_and(|tunnel| !tunnel.is_alive());
            if discard {
                // Released first: a session that is only being closed must not make the next
                // checkout wait for it.
                drop(pending);
                if let Some((config, settings)) = refill {
                    pool.refill(&entry, config, settings);
                }
                pool.close_session(session).await;
                return;
            }
            let mut session = session;
            let reset = tokio::time::timeout(RESET_BUDGET, session.reset()).await;
            if !matches!(reset, Ok(Ok(()))) {
                drop(pending);
                pool.close_session(session).await;
                return;
            }
            pool.park(&entry, session, tunnel);
            drop(pending);
        };
        // With no runtime to run on (a session dropped after shutdown) the work is dropped with
        // its session, which closes the socket, and the guards give the counts back.
        let _ = self.spawn(work);
    }

    /// Put a session in the idle list, or close it when the key has enough.
    fn park(
        self: &Arc<Self>,
        entry: &Arc<Entry>,
        session: Box<dyn Session>,
        tunnel: Option<Arc<dyn TunnelHandle>>,
    ) {
        let leftover = {
            let mut state = entry.state();
            if state.retired || state.idle.len() >= IDLE_MAX {
                Some(Idle {
                    session,
                    tunnel,
                    since: Instant::now(),
                })
            } else {
                state.idle.push(Idle {
                    session,
                    tunnel,
                    since: Instant::now(),
                });
                None
            }
        };
        match leftover {
            Some(idle) => self.close_in_background(vec![idle]),
            None => self.ensure_evictor(),
        }
    }

    /// Open one session for `config` in the background, when its key has none, so the first Run
    /// after selecting a connection finds a warm one. Connects only: it sends no query.
    pub(crate) fn warm(self: &Arc<Self>, config: ConnectionConfig, settings: Settings) {
        let entry = self.entry_for(&PoolKey::of(&config, &settings));
        self.open_idle(&entry, config, settings, true);
    }

    /// [`Self::warm`] for a key that is in use: the replacement for a session that was closed
    /// after a capped preview. Other leases running on the key do not stop it.
    fn refill(self: &Arc<Self>, entry: &Arc<Entry>, config: ConnectionConfig, settings: Settings) {
        self.open_idle(entry, config, settings, false);
    }

    fn open_idle(
        self: &Arc<Self>,
        entry: &Arc<Entry>,
        config: ConnectionConfig,
        settings: Settings,
        only_when_unused: bool,
    ) {
        let pending = {
            let mut state = entry.state();
            if state.retired
                || !state.idle.is_empty()
                || (only_when_unused && state.leased != [0, 0])
                || state.pending > 0
            {
                return;
            }
            state.pending += 1;
            PendingGuard {
                pool: Arc::clone(self),
                entry: Arc::clone(entry),
            }
        };
        let pool = Arc::clone(self);
        let entry = Arc::clone(entry);
        // A failure is dropped: the Run that follows reports the same error on its normal path.
        let _ = self.spawn(async move {
            if let Ok((mut session, tunnel)) = pool.open_session(&entry, &config, &settings).await {
                session.set_context(config.database.as_deref(), config.schema.as_deref());
                pool.park(&entry, session, tunnel);
            }
            drop(pending);
        });
    }

    /// Close and forget idle sessions older than `ttl`, then entries that hold nothing.
    ///
    /// An entry stays while anything still uses its tunnel (an export in progress holds a clone),
    /// so a tunnel never closes under a running export.
    pub async fn evict(self: &Arc<Self>, ttl: Duration) {
        let mut expired: Vec<Idle> = Vec::new();
        for entry in self.entries().values() {
            // Held by the map alone: anything else (a lease, a warm-up, a long operation) still
            // owns the entry, and its idle sessions and tunnel are left where they are.
            if Arc::strong_count(entry) > 1 {
                continue;
            }
            let mut state = entry.state();
            let (old, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut state.idle)
                .into_iter()
                .partition(|idle| idle.since.elapsed() >= ttl);
            state.idle = keep;
            expired.extend(old);
        }
        // Closed before the second look: an idle session holds a clone of its tunnel.
        for idle in expired {
            self.close_session(idle.session).await;
        }
        self.entries().retain(|_, entry| {
            if Arc::strong_count(entry) > 1 {
                return true;
            }
            let state = entry.state();
            let empty = state.idle.is_empty() && state.leased == [0, 0] && state.pending == 0;
            drop(state);
            if !empty {
                return true;
            }
            match entry.tunnel.try_lock() {
                Ok(slot) => slot
                    .as_ref()
                    .is_some_and(|tunnel| Arc::strong_count(tunnel) > 1),
                // Someone is opening it right now.
                Err(_) => true,
            }
        });
    }

    /// One task per pool, started when the first session goes idle. It holds the pool weakly, so
    /// a pool nobody uses any more is not kept alive by its own janitor.
    fn ensure_evictor(self: &Arc<Self>) {
        if self.evictor.swap(true, Ordering::SeqCst) {
            return;
        }
        let pool: Weak<SessionPool> = Arc::downgrade(self);
        let started = tokio::runtime::Handle::try_current().map(|runtime| {
            runtime.spawn(async move {
                loop {
                    tokio::time::sleep(EVICT_TICK).await;
                    let Some(pool) = pool.upgrade() else { break };
                    pool.evict(IDLE_TTL).await;
                }
            })
        });
        if started.is_err() {
            self.evictor.store(false, Ordering::SeqCst);
        }
    }

    /// Close a session and count it.
    async fn close_session(&self, session: Box<dyn Session>) {
        self.closed.fetch_add(1, Ordering::SeqCst);
        let _ = tokio::time::timeout(RESET_BUDGET, session.close()).await;
    }

    fn close_in_background(self: &Arc<Self>, idle: Vec<Idle>) {
        if idle.is_empty() {
            return;
        }
        let pool = Arc::clone(self);
        let _ = self.spawn(async move {
            for idle in idle {
                pool.close_session(idle.session).await;
            }
        });
    }

    /// Run `work` on the current runtime, counted so [`Self::settle`] can wait for it. `false`
    /// when there is no runtime, in which case `work` is dropped unrun.
    fn spawn(self: &Arc<Self>, work: impl Future<Output = ()> + Send + 'static) -> bool {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return false;
        };
        self.background.fetch_add(1, Ordering::SeqCst);
        let counted = Background(Arc::clone(self));
        runtime.spawn(async move {
            let _counted = counted;
            work.await;
        });
        true
    }

    /// A fresh session for a lease whose own is broken: the same key, the same tunnel.
    pub(crate) async fn reconnect(
        self: &Arc<Self>,
        entry: &Arc<Entry>,
        config: &ConnectionConfig,
        settings: &Settings,
    ) -> Result<(Box<dyn Session>, Option<Arc<dyn TunnelHandle>>), EngineError> {
        let (mut session, tunnel) = self.open_session(entry, config, settings).await?;
        session.set_context(config.database.as_deref(), config.schema.as_deref());
        Ok((session, tunnel))
    }
}
