//! The session every command drives: [`Held`], the driver's own session plus what has to be
//! looked after around it (blueprint `fase-2-engine-host.md` §4.4 and §6).
//!
//! `Held` is the only session wrapper. It owns the SSH tunnel's lifetime (a plain session reached
//! through a tunnel, as the CLI has), and, when the pool lent it out, the lease: whether the
//! session may go back, and the one rule about reconnecting.
//!
//! # When a statement is sent again on a new connection
//!
//! An idle socket can die without anyone noticing (a NAT that forgot it, a server that
//! restarted). Checkout does not ping. Instead, a reused session that fails on its first
//! statement with an error that proves the server never answered is replaced **once**, and the
//! statement is sent again, but only if sending it twice cannot hurt: a read, or the exact text
//! `BEGIN` / `START TRANSACTION` (a transaction on a dead connection died with it). A write is
//! never sent again, by this layer or through `retry::execute`: a `Connect` error from MySQL can
//! also mean the connection dropped after the server ran the statement.
//!
//! # When a session goes back
//!
//! Only when nothing about it is in doubt. It is closed instead of reset when it was cancelled
//! (a late `KILL QUERY` or `CancelRequest` must never reach the next statement), when a call on it
//! was dropped half-way, when it broke, when a cursor was left before the server ended it, when
//! its tunnel died, or when its credentials were retired. Closing is always safe; a reset of a
//! session in an unknown state is not.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use qh_core::{ColumnBatch, ColumnMeta, EngineError, FailureKind};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, Cursor, DriverKind, ExecuteOptions, ObjectPath,
    ObjectsPage, Parameter, Session,
};
use qh_sql::{Dialect, StatementKind};

use super::pool::{Entry, LeaseGuard, SessionPool};
use super::TunnelHandle;
use crate::env::Settings;
use crate::retry::produced_no_page;

/// Whether a statement may be sent again on a replacement connection.
///
/// Reads, classified under every lexical reading the server could use, and the exact texts the
/// engine's own write paths open a transaction with.
fn rerunnable(sql: &str, kind: DriverKind) -> bool {
    let trimmed = sql.trim();
    if trimmed.eq_ignore_ascii_case("BEGIN") || trimmed.eq_ignore_ascii_case("START TRANSACTION") {
        return true;
    }
    let dialect = if kind == DriverKind::Mysql {
        Dialect::Mysql
    } else {
        Dialect::Generic
    };
    qh_sql::classify_readings(sql, dialect.readings()) == StatementKind::ReadOnly
}

/// Whether an error says the connection was lost rather than the server said no.
fn connection_lost(error: &EngineError) -> bool {
    produced_no_page(error) && error.failure_kind() == FailureKind::Transient
}

/// How the last cursor of a lease ended.
#[derive(Default)]
pub(crate) struct CursorEnd {
    clean: AtomicBool,
}

/// A cursor that notes whether the server saw its result through to the end.
///
/// Clean means the server itself ended the stream: the driver answered `None` before the row
/// limit stopped it, or the server sent an error (an error with a code, or its timeout). An error
/// with no code (a decode failure, a transport error) proves nothing about the stream, and a
/// cursor stopped by the row limit was stopped by us.
struct Tracked {
    inner: Box<dyn Cursor>,
    end: Arc<CursorEnd>,
    row_limit: Option<usize>,
    delivered: usize,
}

#[async_trait]
impl Cursor for Tracked {
    fn columns(&self) -> &[ColumnMeta] {
        self.inner.columns()
    }

    fn affected_rows(&self) -> Option<u64> {
        self.inner.affected_rows()
    }

    async fn next_batch(&mut self, max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        match self.inner.next_batch(max_rows).await {
            Ok(Some(batch)) => {
                self.delivered += batch.rows();
                Ok(Some(batch))
            }
            Ok(None) => {
                if self.row_limit.is_none_or(|limit| self.delivered < limit) {
                    self.end.clean.store(true, Ordering::SeqCst);
                }
                Ok(None)
            }
            Err(error) => {
                if error.code().is_some() || matches!(error, EngineError::Timeout { .. }) {
                    self.end.clean.store(true, Ordering::SeqCst);
                }
                Err(error)
            }
        }
    }
}

/// What the pool needs to know about a borrowed session.
pub(crate) struct LeaseState {
    pool: Arc<SessionPool>,
    entry: Arc<Entry>,
    guard: LeaseGuard,
    config: ConnectionConfig,
    settings: Settings,
    /// The session came from the idle list, so its socket may have died while it waited.
    reused: bool,
    /// A statement has been sent on this session.
    executed: bool,
    /// The connection is known to be gone. The next call replaces it or refuses.
    broken: bool,
    /// A call on the session was started and never finished: dropped by a Stop, most likely.
    unsettled: bool,
    /// An earlier cursor of this lease was left before the server ended it.
    dirty: bool,
    /// `cancel` takes `&self`, hence the atomic.
    cancelled: AtomicBool,
    last_cursor: Option<Arc<CursorEnd>>,
}

impl LeaseState {
    pub(crate) fn new(
        pool: Arc<SessionPool>,
        entry: Arc<Entry>,
        guard: LeaseGuard,
        config: ConnectionConfig,
        settings: Settings,
        reused: bool,
    ) -> Self {
        Self {
            pool,
            entry,
            guard,
            config,
            settings,
            reused,
            executed: false,
            broken: false,
            unsettled: false,
            dirty: false,
            cancelled: AtomicBool::new(false),
            last_cursor: None,
        }
    }

    /// Whether the session must be closed rather than reset.
    fn must_discard(&self) -> bool {
        // A Trino session has no stream to leave behind (its reset stops the query), so an
        // unfinished cursor only disqualifies the two drivers whose result lives on the connection.
        let stream_matters = self.config.kind != DriverKind::Trino;
        let abandoned = stream_matters
            && (self.dirty
                || self
                    .last_cursor
                    .as_ref()
                    .is_some_and(|end| !end.clean.load(Ordering::SeqCst)));
        self.cancelled.load(Ordering::SeqCst) || self.broken || self.unsettled || abandoned
    }
}

/// One call a lease forwards, described as data so the reconnect rule can run it twice.
#[derive(Clone, Copy)]
enum Op<'a> {
    Execute(&'a str, &'a ExecuteOptions),
    Bound(&'a str, &'a [Parameter], &'a ExecuteOptions),
    Browse(BrowseLevel, &'a ObjectPath, bool),
    Objects(&'a ObjectPath),
}

enum Reply {
    Cursor(Box<dyn Cursor>),
    Names(Vec<String>),
    Page(ObjectsPage),
}

/// The session the commands drive.
///
/// The session sits behind a tokio mutex for one mechanical reason: `Session::cancel` takes
/// `&self`, and an `async` `&self` method's future must be `Send`, which needs the wrapper to be
/// `Sync` — and `Box<dyn Session>` is only `Send`. The lock never serialises anything in
/// practice, because safe Rust already forbids holding `&mut self` (an `execute`) and `&self` (a
/// `cancel`) on one session at the same time; a driver's cancel reaches the server through a
/// *separate* connection for exactly that reason (see the module note in `retry.rs`).
pub(crate) struct Held {
    inner: tokio::sync::Mutex<Option<Box<dyn Session>>>,
    // Mirrored at construction: `capabilities` is documented on the trait as readable before
    // connecting, so it cannot change with session state, and the `&self` accessor cannot lock.
    // `query_id` is the one accessor that genuinely moves, and it is mirrored after every
    // `execute` -- the only method that changes it.
    capabilities: Capabilities,
    query_id: Option<String>,
    /// Dropped after `inner`, so no forward is asked of a bastion connection that is already gone.
    tunnel: Option<Arc<dyn TunnelHandle>>,
    lease: Option<LeaseState>,
}

impl Held {
    /// A session with no lease: the CLI's, or a long operation's. It owns its tunnel share and
    /// nothing else.
    pub(crate) fn new(inner: Box<dyn Session>, tunnel: Option<Arc<dyn TunnelHandle>>) -> Self {
        Self {
            capabilities: inner.capabilities(),
            query_id: None,
            inner: tokio::sync::Mutex::new(Some(inner)),
            tunnel,
            lease: None,
        }
    }

    pub(crate) fn leased(
        inner: Box<dyn Session>,
        tunnel: Option<Arc<dyn TunnelHandle>>,
        lease: LeaseState,
    ) -> Self {
        Self {
            capabilities: inner.capabilities(),
            query_id: None,
            inner: tokio::sync::Mutex::new(Some(inner)),
            tunnel,
            lease: Some(lease),
        }
    }

    fn session(&mut self) -> &mut Box<dyn Session> {
        self.inner
            .get_mut()
            .as_mut()
            .expect("a session is present until it is closed")
    }

    /// Whether `sql` may be sent again on a replacement connection. Only a lease can reconnect,
    /// so a session with none skips the classification altogether.
    fn rerunnable(&self, sql: &str) -> bool {
        self.lease
            .as_ref()
            .is_some_and(|lease| rerunnable(sql, lease.config.kind))
    }

    /// Replace a lost connection with a new one on the same key and tunnel.
    async fn reconnect(&mut self) -> Result<(), EngineError> {
        let Some(lease) = self.lease.as_mut() else {
            return Ok(());
        };
        // Before the first await: a future dropped mid-connect leaves this set, and the session is
        // closed at checkin instead of going back half-replaced.
        lease.broken = true;
        let (session, tunnel) = lease
            .pool
            .reconnect(&lease.entry, &lease.config, &lease.settings)
            .await?;
        // The old session is dropped, not closed: its socket is what is gone.
        drop(self.inner.get_mut().replace(session));
        self.tunnel = tunnel;
        lease.broken = false;
        lease.reused = false;
        lease.executed = false;
        lease.unsettled = false;
        lease.dirty = false;
        lease.last_cursor = None;
        Ok(())
    }

    /// One call on the session, with the reconnect rule around it.
    async fn call(&mut self, rerunnable: bool, op: Op<'_>) -> Result<Reply, EngineError> {
        if self.lease.as_ref().is_some_and(|lease| lease.broken) {
            if !rerunnable {
                return Err(EngineError::Connect {
                    message: "the pooled connection was lost; the statement was not sent again"
                        .to_owned(),
                    kind: FailureKind::Permanent,
                });
            }
            self.reconnect().await?;
        }
        let first = self
            .lease
            .as_ref()
            .is_some_and(|lease| lease.reused && !lease.executed);

        let mut outcome = self.attempt(op).await;
        if first && rerunnable {
            if let Err(error) = &outcome {
                if produced_no_page(error) {
                    self.reconnect().await?;
                    outcome = self.attempt(op).await;
                }
            }
        }
        if let Some(lease) = self.lease.as_mut() {
            lease.executed = true;
            if outcome.as_ref().err().is_some_and(connection_lost) {
                lease.broken = true;
            }
        }
        outcome
    }

    async fn attempt(&mut self, op: Op<'_>) -> Result<Reply, EngineError> {
        if let Some(lease) = self.lease.as_mut() {
            // A left-over cursor from an earlier statement, still open when the next one starts.
            if lease
                .last_cursor
                .as_ref()
                .is_some_and(|end| !end.clean.load(Ordering::SeqCst))
            {
                lease.dirty = true;
            }
            // Cleared only when the call returns: a call dropped in the middle leaves it set.
            lease.unsettled = true;
        }
        let session = &mut **self.session();
        let outcome = match op {
            Op::Execute(sql, options) => session.execute(sql, options).await.map(Reply::Cursor),
            Op::Bound(sql, parameters, options) => session
                .execute_bound(sql, parameters, options)
                .await
                .map(Reply::Cursor),
            Op::Browse(level, path, include_system) => session
                .browse(level, path, include_system)
                .await
                .map(Reply::Names),
            Op::Objects(path) => session.objects(path).await.map(Reply::Page),
        };
        if let Some(lease) = self.lease.as_mut() {
            lease.unsettled = false;
        }
        outcome
    }

    fn track(&mut self, cursor: Box<dyn Cursor>, options: &ExecuteOptions) -> Box<dyn Cursor> {
        let Some(lease) = self.lease.as_mut() else {
            return cursor;
        };
        let end = Arc::new(CursorEnd::default());
        lease.last_cursor = Some(Arc::clone(&end));
        Box::new(Tracked {
            inner: cursor,
            end,
            row_limit: options.row_limit,
            delivered: 0,
        })
    }

    /// Give the session back to the pool (or drop it), whichever way this wrapper is finished.
    fn release(&mut self) {
        let session = self.inner.get_mut().take();
        let tunnel = self.tunnel.take();
        match (self.lease.take(), session) {
            (Some(lease), Some(session)) => {
                let discard = lease.must_discard();
                lease
                    .pool
                    .checkin(&lease.entry, lease.guard, session, tunnel, discard);
            }
            (_, session) => {
                // The session first, then the tunnel it rides on.
                drop(session);
                drop(tunnel);
            }
        }
    }
}

impl Drop for Held {
    /// A session dropped without `close` (a command left through `?`) still goes back.
    fn drop(&mut self) {
        self.release();
    }
}

#[async_trait]
impl Session for Held {
    fn capabilities(&self) -> Capabilities {
        self.capabilities.clone()
    }

    fn query_id(&self) -> Option<String> {
        self.query_id.clone()
    }

    async fn execute(
        &mut self,
        sql: &str,
        options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        let again = self.rerunnable(sql);
        let Reply::Cursor(cursor) = self.call(again, Op::Execute(sql, options)).await? else {
            unreachable!("an execute answers with a cursor")
        };
        // The id arrived with the statement: mirror it so `query_id` stays lock-free for the
        // events that report it.
        self.query_id = self.session().query_id();
        Ok(self.track(cursor, options))
    }

    /// Forwarded, not inherited.
    ///
    /// The trait's default refuses a non-empty parameter list, which is the right default for a
    /// driver that cannot bind — but a wrapper is not a driver. Without this, every bound
    /// statement through a wrapper is refused, and `apply_changes` stops working for no reason
    /// the caller can see.
    async fn execute_bound(
        &mut self,
        sql: &str,
        parameters: &[Parameter],
        options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        let again = self.rerunnable(sql);
        let Reply::Cursor(cursor) = self
            .call(again, Op::Bound(sql, parameters, options))
            .await?
        else {
            unreachable!("an execute answers with a cursor")
        };
        self.query_id = self.session().query_id();
        Ok(self.track(cursor, options))
    }

    async fn browse(
        &mut self,
        level: BrowseLevel,
        path: &ObjectPath,
        include_system: bool,
    ) -> Result<Vec<String>, EngineError> {
        match self
            .call(true, Op::Browse(level, path, include_system))
            .await?
        {
            Reply::Names(names) => Ok(names),
            _ => unreachable!("a browse answers with names"),
        }
    }

    async fn objects(&mut self, path: &ObjectPath) -> Result<ObjectsPage, EngineError> {
        match self.call(true, Op::Objects(path)).await? {
            Reply::Page(page) => Ok(page),
            _ => unreachable!("an objects call answers with a page"),
        }
    }

    fn explain_statement(&self, sql: &str) -> String {
        match self.inner.try_lock() {
            Ok(session) => match session.as_ref() {
                Some(session) => session.explain_statement(sql),
                None => format!("EXPLAIN {sql}"),
            },
            // Contended is unreachable in practice: a caller cannot hold `&mut self` (an `execute`
            // in flight) and `&self` (this call) on one session at the same time. If that ever
            // changed, the statement is still spelled the way all three drivers spell it rather
            // than not at all — and the contended case is the one to revisit, not the spelling.
            Err(_) => format!("EXPLAIN {sql}"),
        }
    }

    async fn cancel(&self) -> Result<(), EngineError> {
        // Marked first: whatever the cancel does or fails to do, this session never serves
        // another statement, so a cancel that lands late has nothing to land on.
        if let Some(lease) = &self.lease {
            lease.cancelled.store(true, Ordering::SeqCst);
        }
        match self.inner.lock().await.as_ref() {
            Some(session) => session.cancel().await,
            None => Ok(()),
        }
    }

    async fn close(mut self: Box<Self>) -> Result<(), EngineError> {
        if self.lease.is_some() {
            // Back to the pool, which resets it in the background: `close` returns at once.
            self.release();
            return Ok(());
        }
        let session = self.inner.get_mut().take();
        match session {
            Some(session) => session.close().await,
            None => Ok(()),
        }
    }

    async fn reset(&mut self) -> Result<(), EngineError> {
        self.session().reset().await
    }

    fn set_context(&mut self, database: Option<&str>, schema: Option<&str>) {
        self.session().set_context(database, schema);
    }
}
