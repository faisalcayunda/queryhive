//! MySQL: streaming batches over a bounded channel, exact decimals, and `KILL QUERY`.
//!
//! Why this driver streams differently from the PostgreSQL one
//! ----------------------------------------------------------
//! `mysql_async`'s `QueryResult<'a, 't, P>` holds a `Connection<'a, 't>` — it
//! **borrows** the connection it is reading from. `tokio-postgres`'s
//! `SimpleQueryStream` owns its own, which is why that driver could hand the
//! result straight to a cursor and be done.
//!
//! Here that is impossible: a `Box<dyn Cursor>` must be `'static`, so it cannot
//! hold something that borrows the session. The result is therefore produced by a
//! **task that owns the connection**, and the cursor reads batches from a bounded
//! channel. That is the backpressure shape the blueprint already called for
//! (section 2.3) rather than a workaround — a producer that ran ahead of the
//! consumer would fill memory with rows nobody had asked for yet.
//!
//! ## One session, one connection
//!
//! A transaction, `SET FOREIGN_KEY_CHECKS`, a user variable and `max_execution_time` all
//! live on the connection, so a session that ran each statement on a new one would run
//! `BEGIN`, its writes and `ROLLBACK` in three different transactions. The producer
//! therefore hands its connection back through a [`Flight`] when the statement has been
//! read to its end (or failed with an answer from the server), and the session runs the
//! next statement on it. A statement that was abandoned instead — the cursor dropped
//! mid-stream, a row limit reached, the socket failing — leaves the connection in an
//! unknown state, so it is dropped as before and the next statement opens a fresh one.
//!
//! `execute` waits for the producer's first message before returning, so
//! `columns()` is valid immediately as the trait requires. Only the column
//! descriptions have to arrive for that, and they come before any row — so this
//! wait does not delay the first row. Where they come from is the `COM_STMT_PREPARE`
//! describe the statement is sent with, except for a capped read (a preview), which skips it
//! and takes them from its own result set: see `run_statement`.
//!
//! ## Cancelling
//!
//! MySQL has no cancel message. It offers `KILL QUERY <id>` instead, run on a
//! **second connection**, which leaves the connection itself alive while stopping
//! the statement. The id is read from the producer's connection and published in a
//! shared cell as soon as that connection exists.
//!
//! ## Statement timeouts, and the write they do not cover
//!
//! `ExecuteOptions::statement_timeout` is put in force as the `max_execution_time`
//! session variable, which MySQL enforces itself: a `SELECT` that overruns is
//! interrupted with error `3024` and the connection stays usable. The bound is set on
//! the same connection the statement runs on, right before it runs.
//!
//! **It does not cover writes.** MySQL applies `max_execution_time` to read-only
//! `SELECT` statements only; an `INSERT`, `UPDATE` or `DELETE` ignores it. So a
//! bounded connection bounds its reads and not its writes, and this note is here
//! because a capability that claimed more would be pretending. The other two drivers'
//! mechanisms cover writes as well, which is a difference the UI would have to state
//! if it ever offered the bound per statement rather than per connection.
//!
//! ## TLS
//!
//! All four `TlsMode`s are implemented, through `mysql_async`'s `rustls`
//! backend:
//!
//! - `Disable` never offers the capability, so nothing can turn the connection
//!   into an encrypted one behind the user's back.
//! - `Prefer` encrypts when the server offers TLS, without checking the
//!   certificate — the same meaning pymysql's `prefer` had, so an internal server
//!   with a self-signed certificate keeps connecting. It falls back to plaintext
//!   for one server answer only — a handshake packet with no `CLIENT_SSL`
//!   capability, which is the server saying it cannot do TLS, decided before a
//!   single TLS byte is sent. A handshake that *fails* is an error: anyone who can
//!   break a handshake would otherwise get a silent downgrade for free, which is
//!   the whole reason the two are told apart, and turning verification off does
//!   not weaken that rule.
//! - `Require` asks for TLS and verifies the certificate, so it fails against a
//!   server it cannot authenticate.
//! - `RequireNoVerify` is `Prefer`'s configuration without its fallback: encrypt,
//!   verify nothing, and only when the user asked for that mode on the connection.
//!
//! A verified connection is checked against the roots `mysql_async` compiles in
//! (the `webpki-roots` bundle) and against the host that was dialled. The platform
//! trust store is **not** reachable from `mysql_async` 0.36's API, so a corporate
//! CA installed in the Keychain is refused rather than trusted; [`tls`] records
//! exactly why and what would close it. That module also holds the mode-to-
//! configuration mapping, public so it can be tested directly.

#![forbid(unsafe_code)]

pub mod metadata;
pub mod normalize;
pub mod tls;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, OptsBuilder, SslOpts};
use qh_core::{ColumnBatch, ColumnMeta, EngineError, FailureKind, Value};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions,
    MetadataSql, ObjectPath, ObjectsPage, Parameter, ParameterStyle, Session,
};
use qh_sql::{
    classify_readings, statements_dialect, strip_terminator_dialect, Dialect, StatementKind,
};
use tokio::sync::mpsc;

/// Whether `sql` is at most one statement under **every** lexer the server might be using.
///
/// The server's `sql_mode` (`NO_BACKSLASH_ESCAPES`, `ANSI_QUOTES`) and version decide where
/// a string or a comment ends, and none of it is knowable here. The lexer that matches the
/// server is one of [`Dialect::readings`], so if every one of them sees at most one
/// statement, the server does too. This is the same reading set the engine's Safe Mode guard
/// classifies under; unlike the guard it does not need the readings to agree on the text of
/// the statement, only that none of them finds a second one, so a trailing `;` after a
/// string the readings scan differently is not refused for that alone.
fn is_single_statement(sql: &str) -> bool {
    Dialect::Mysql
        .readings()
        .iter()
        .all(|lexer| statements_dialect(sql, *lexer).len() <= 1)
}

/// Rows the producer may run ahead by, in batches.
///
/// Four, not four hundred: the point of the bound is that a slow consumer stops
/// the producer rather than letting it read a whole result into memory. Four is
/// enough to keep the socket busy between the consumer's calls.
const BATCH_BACKLOG: usize = 4;

/// Rows the producer puts in one batch when the caller did not say.
///
/// The caller's ceiling is still honoured: the cursor keeps a remainder and slices
/// it, so a producer that read 1024 rows does not hand 1024 to a caller who asked
/// for 100.
const DEFAULT_PRODUCER_BATCH: usize = 1024;

/// How long a session waits for the producer of its previous statement to hand the
/// connection back before it calls the cursor still in use.
///
/// A producer that has read its whole result needs a few microseconds; one blocked on a
/// full channel needs the cursor to be read, which the waiting caller cannot do, so it is
/// refused rather than waited for.
const HAND_BACK_WAIT: Duration = Duration::from_millis(250);

/// How long a reset waits for the previous statement's connection to come back. The pool
/// bounds the whole reset as well; this only stops a producer that never finishes from
/// holding the reset for all of it.
const RESET_WAIT: Duration = Duration::from_secs(2);

/// TCP keepalive interval for every connection, in milliseconds (`OptsBuilder::tcp_keepalive`).
const TCP_KEEPALIVE_MS: u32 = 60_000;

/// What a statement does to the session's transaction, read from its first words.
#[derive(PartialEq, Eq, Clone, Copy)]
enum TxnEffect {
    Begin,
    End,
    Rollback,
    None,
}

fn txn_effect(sql: &str) -> TxnEffect {
    let upper = sql.trim_start().to_ascii_uppercase();
    let mut words = upper
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty());
    match (words.next(), words.next()) {
        (Some("BEGIN"), _) | (Some("START"), Some("TRANSACTION")) => TxnEffect::Begin,
        (Some("ROLLBACK"), Some("TO")) => TxnEffect::None,
        (Some("ROLLBACK"), _) => TxnEffect::Rollback,
        (Some("COMMIT"), _) => TxnEffect::End,
        _ => TxnEffect::None,
    }
}

/// Where a producer hands its connection back to the session that lent it.
struct Flight {
    state: Mutex<FlightState>,
    /// Woken when the producer finishes.
    done: tokio::sync::Notify,
    /// Weak, so holding it does not keep the channel open for a cursor that has been
    /// read to the end. Upgrading tells whether the cursor still exists.
    sender: mpsc::WeakSender<Message>,
}

#[derive(Default)]
struct FlightState {
    /// The connection, when the statement ended in a state the next one can start from.
    conn: Option<Conn>,
    finished: bool,
}

impl Flight {
    fn new(sender: mpsc::WeakSender<Message>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(FlightState::default()),
            done: tokio::sync::Notify::new(),
            sender,
        })
    }

    /// Called by the producer as its last act: `conn` is `None` when it was abandoned.
    fn finish(&self, conn: Option<Conn>) {
        if let Ok(mut state) = self.state.lock() {
            state.conn = conn;
            state.finished = true;
        }
        self.done.notify_one();
    }

    fn take_conn(&self) -> Option<Conn> {
        self.state
            .lock()
            .ok()
            .and_then(|mut state| state.conn.take())
    }

    fn is_finished(&self) -> bool {
        self.state.lock().map_or(true, |state| state.finished)
    }

    /// Whether the producer finished within `limit`.
    async fn wait_finished(&self, limit: Duration) -> bool {
        let wait = async {
            while !self.is_finished() {
                self.done.notified().await;
            }
        };
        tokio::time::timeout(limit, wait).await.is_ok()
    }

    /// Whether the cursor of this statement has been dropped.
    fn cursor_gone(&self) -> bool {
        self.sender
            .upgrade()
            .is_none_or(|sender| sender.is_closed())
    }
}

/// MySQL.
pub struct MysqlDriver;

impl MysqlDriver {
    pub fn new() -> Self {
        Self
    }
}

impl Default for MysqlDriver {
    fn default() -> Self {
        Self::new()
    }
}

/// The object columns this driver reports, carried over unchanged from the Python
/// driver so the grid looks the same after the migration.
const OBJECTS_COLUMNS: [&str; 4] = ["Name", "Engine", "Rows", "Comment"];

#[async_trait]
impl Driver for MysqlDriver {
    fn kind(&self) -> DriverKind {
        DriverKind::Mysql
    }

    fn label(&self) -> &'static str {
        "MySQL"
    }

    fn default_port(&self) -> u16 {
        3306
    }

    fn metadata(&self) -> Option<&dyn MetadataSql> {
        Some(&metadata::MysqlMetadata)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            transactions: true,
            multiple_result_sets: false,
            cancel: true,
            explain: true,
            // MySQL has databases containing tables, and no separate schema level
            // the way PostgreSQL does. Declared rather than hard-coded in the UI.
            levels: vec![BrowseLevel::Database, BrowseLevel::Table],
            objects_columns: OBJECTS_COLUMNS
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            persistent_connection: true,
            // `max_execution_time`, which the server enforces for a read-only
            // `SELECT`. See the module note: it does not cover a write.
            statement_timeout: true,
            // `?`, one per value in order — the positional placeholders the
            // prepared protocol takes. Values go as native `mysql_async::Value`s
            // rather than their text, so the type is the client's too.
            parameters: Some(ParameterStyle::Question),
            read_only: false,
        }
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        // Connect once here rather than lazily, so a bad host or password is
        // reported by `connect` — where the UI shows it — instead of surfacing
        // later as a query that mysteriously returns nothing.
        //
        // The `Opts` that worked is the one the session keeps for its later
        // connections, so a `Prefer` connection that fell back to plaintext does
        // not re-attempt TLS against a server that has already said it cannot do
        // it.
        let mut opts = build_opts(config, tls::ssl_opts(config.tls));
        let conn = match Conn::new(opts.clone()).await {
            Ok(conn) => conn,
            Err(error) => {
                if !tls::may_fall_back(config.tls, &error) {
                    return Err(connect_failure(config, &error));
                }
                // `Prefer`, and the server's handshake said it has no TLS. Ask
                // again without it: connecting in clear is what that mode asks
                // for here, and it is the server's own capability flags that
                // decided it — not a handshake that failed.
                opts = build_opts(config, None);
                Conn::new(opts.clone())
                    .await
                    .map_err(|error| connect_failure(config, &error))?
            }
        };
        let connection_id = Arc::new(AtomicU32::new(conn.id()));
        Ok(Box::new(MysqlSession {
            opts,
            connection_id,
            idle: Some(conn),
            flight: None,
            wanted_db: config.database.clone(),
            current_db: config.database.clone(),
            in_transaction: false,
            transaction_lost: false,
            // Nothing has been asked of the server yet, so the session must not send a
            // `SET SESSION max_execution_time = 0` on its first statement and override a
            // bound the server's configuration set.
            statement_timeout: None,
            select_limit: None,
        }))
    }
}

fn transaction_lost() -> EngineError {
    EngineError::Query {
        message: "the connection that held this session's transaction was lost, so the server \
                  rolled the transaction back; nothing after its BEGIN was kept. Send ROLLBACK \
                  to end it"
            .to_owned(),
        code: None,
        position: None,
        kind: FailureKind::Permanent,
    }
}

fn build_opts(config: &ConnectionConfig, tls: Option<SslOpts>) -> Opts {
    let mut builder = OptsBuilder::default()
        .ip_or_hostname(config.host.clone())
        .tcp_port(config.port)
        .user(Some(config.user.clone()))
        // Connecting over a unix socket would ignore host and port entirely, and
        // in a container the socket usually is not there. Forced off so the
        // configured address is the address used.
        .prefer_socket(false)
        // An idle pooled connection can sit behind a NAT that forgets it; a probe every minute
        // keeps the mapping alive and lets a dead socket show up as one.
        .tcp_keepalive(Some(TCP_KEEPALIVE_MS));
    // Absent means the connection is deliberately in clear: `mysql_async` never
    // offers a capability it was not given options for.
    if let Some(tls) = tls {
        builder = builder.ssl_opts(Some(tls));
    }
    if let Some(password) = &config.password {
        builder = builder.pass(Some(password.clone()));
    }
    if let Some(database) = &config.database {
        builder = builder.db_name(Some(database.clone()));
    }
    builder.into()
}

/// A MySQL session.
struct MysqlSession {
    opts: Opts,
    /// The id `KILL QUERY` needs, published when the session connects and again by whichever
    /// connection runs a statement. It is never cleared when a statement ends: a cursor
    /// dropped mid-stream ends the producer before a Stop can reach the server, and a cancel
    /// that found zero would report "nothing running" for a statement still draining. Zero
    /// only means no connection was ever made.
    connection_id: Arc<AtomicU32>,
    /// The connection kept for the next statement, so an idle session is one
    /// connection rather than a new handshake per query.
    idle: Option<Conn>,
    /// The statement that currently holds the connection, if any.
    flight: Option<Arc<Flight>>,
    /// The database the next run is about, from [`Session::set_context`]. `USE` is sent
    /// before the next statement when it differs from `current_db`, so a pooled session moves
    /// between databases without a new handshake.
    wanted_db: Option<String>,
    /// The database this session's connection is in, as far as the driver knows. A `USE` in
    /// a user's own SQL changes it behind the driver's back, which is why `reset` asks the
    /// server instead of trusting this.
    current_db: Option<String>,
    /// A `BEGIN` was sent and no `COMMIT` or `ROLLBACK` since.
    in_transaction: bool,
    /// The connection that held the open transaction is gone, so the server has already
    /// rolled it back. Everything but `ROLLBACK` is refused until one is sent, because
    /// running on a fresh autocommit connection would let a `COMMIT` succeed over nothing.
    transaction_lost: bool,
    /// The `max_execution_time` this session last sent to the server, so the `SET`
    /// is sent when the bound changes rather than on every statement.
    statement_timeout: Option<Duration>,
    /// The `sql_select_limit` this session last sent, tracked like the timeout. `None` is the
    /// server's own default, which is what a new or freshly reset connection has.
    select_limit: Option<usize>,
}

impl MysqlSession {
    fn capabilities(&self) -> Capabilities {
        MysqlDriver.capabilities()
    }

    /// The connection for the next statement: the one the session already has, taken
    /// back from the previous statement when that one ended cleanly, and a new one only
    /// when there is none to take.
    ///
    /// A previous statement whose cursor is still alive and has not finished is an
    /// error, not a wait and not a second connection: the caller is running two
    /// statements at once on a session that has one connection, and a silent second
    /// connection would put them in different transactions.
    ///
    /// `may_replace` says whether a new connection is acceptable when the old one is gone
    /// in the middle of a transaction (only a `ROLLBACK` is).
    async fn connection(&mut self, may_replace: bool) -> Result<Conn, EngineError> {
        if let Some(conn) = self.idle.take() {
            return Ok(conn);
        }
        if let Some(flight) = self.flight.take() {
            if !flight.cursor_gone() && !flight.wait_finished(HAND_BACK_WAIT).await {
                self.flight = Some(flight);
                return Err(EngineError::Usage {
                    message: "the previous statement of this session is still being read; \
                              read or drop its cursor before running another"
                        .to_owned(),
                });
            }
            if let Some(conn) = flight.take_conn() {
                return Ok(conn);
            }
        }
        if self.in_transaction && !may_replace {
            self.transaction_lost = true;
            return Err(transaction_lost());
        }
        let conn = Conn::new(self.opts.clone())
            .await
            .map_err(|error| EngineError::Connect {
                message: error.to_string(),
                kind: classify_connect_error(&error),
            })?;
        // A new connection starts at the server's own settings, whatever was sent to the
        // one it replaces.
        self.statement_timeout = None;
        self.select_limit = None;
        Ok(conn)
    }

    /// Move the connection into the database the run asked for, when it is somewhere else.
    ///
    /// `opts` follows, so a connection opened later in this session (after a lost one) starts
    /// in the same database rather than in the one the session first connected to.
    async fn use_wanted_database(&mut self, conn: &mut Conn) -> Result<(), EngineError> {
        let Some(wanted) = self.wanted_db.clone() else {
            return Ok(());
        };
        if self.current_db.as_deref() == Some(wanted.as_str()) {
            return Ok(());
        }
        let statement = format!(
            "USE {}",
            qh_sql::quote_ident(qh_sql::IdentStyle::Mysql, &wanted)
        );
        conn.query_drop(&statement)
            .await
            .map_err(|error| map_query_error(error, &statement, None))?;
        self.current_db = Some(wanted.clone());
        self.opts = OptsBuilder::from_opts(self.opts.clone())
            .db_name(Some(wanted))
            .into();
        Ok(())
    }

    /// Run one of our own statements and read every cell as text.
    ///
    /// Used by browse and objects, which are our statements with a known shape —
    /// unlike a user's query, they do not need the streaming path.
    async fn text_rows(&mut self, sql: &str) -> Result<Vec<Vec<Option<String>>>, EngineError> {
        let mut conn = self.connection(false).await?;
        if let Err(error) = self.use_wanted_database(&mut conn).await {
            self.idle = Some(conn);
            return Err(error);
        }
        // Our own statements are never capped: a select limit left by a capped read on this
        // session would silently shorten a tree level.
        if let Err(error) = apply_session_settings(
            &mut conn,
            (self.statement_timeout, self.select_limit),
            (self.statement_timeout, None),
        )
        .await
        {
            self.idle = Some(conn);
            return Err(error);
        }
        self.select_limit = None;
        let outcome = async {
            let rows: Vec<mysql_async::Row> = conn
                .query(sql)
                .await
                .map_err(|error| map_query_error(error, sql, None))?;
            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                let width = row.len();
                out.push(
                    (0..width)
                        .map(|index| match row.as_ref(index) {
                            Some(mysql_async::Value::NULL) | None => None,
                            Some(mysql_async::Value::Bytes(bytes)) => {
                                Some(String::from_utf8_lossy(bytes).into_owned())
                            }
                            Some(other) => Some(format!("{other:?}")),
                        })
                        .collect(),
                );
            }
            Ok(out)
        }
        .await;
        // The connection goes back to the session even when the query failed:
        // reconnecting after every error would be wasteful and would hide a dead
        // connection until the next call.
        self.idle = Some(conn);
        outcome
    }

    /// Run one statement, with `params` bound when it is not empty.
    ///
    /// The one place a statement is described, dispatched and handed to the
    /// streaming producer; `execute` calls it with no values and `execute_bound`
    /// with them, so the two paths cannot drift.
    async fn run_statement(
        &mut self,
        sql: &str,
        options: &ExecuteOptions,
        params: Vec<mysql_async::Value>,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        // Defence in depth (W3-T0). mysql_async always negotiates
        // `CLIENT_MULTI_STATEMENTS` (opts/mod.rs:1096), so the text protocol below would
        // run every statement in a `SELECT '\''; DELETE …` string that a misread Safe
        // Mode classifier let through — the server, not this driver, splits it. The
        // engine already splits and approves one statement at a time; a text that any
        // lexer the server might be using reads as more than one statement is refused
        // here, before it is sent, so no unapproved second statement ever reaches the
        // server. The bound path (`exec_iter`, COM_STMT_PREPARE) rejects multiples
        // server-side too; this closes the text path.
        if !is_single_statement(sql) {
            return Err(EngineError::Usage {
                message: "this connection runs one statement per request; the text given holds \
                          more than one, or MySQL string escaping and comments could read it as \
                          more than one depending on the server's sql_mode (write a quote \
                          inside a string as '' instead of \\')"
                    .to_owned(),
            });
        }

        let effect = txn_effect(sql);
        if self.transaction_lost && effect != TxnEffect::Rollback {
            return Err(transaction_lost());
        }
        let mut conn = self.connection(effect == TxnEffect::Rollback).await?;
        match effect {
            TxnEffect::Begin => self.in_transaction = true,
            TxnEffect::End | TxnEffect::Rollback => {
                self.in_transaction = false;
                self.transaction_lost = false;
            }
            TxnEffect::None => {}
        }

        if let Err(error) = self.use_wanted_database(&mut conn).await {
            self.idle = Some(conn);
            return Err(error);
        }

        // The bounds are server settings, so they reach the server before the statement does,
        // in one `SET` and only when one of them changed: it is a round trip.
        let select_limit = select_limit_for(sql, options);
        if let Err(error) = apply_session_settings(
            &mut conn,
            (self.statement_timeout, self.select_limit),
            (options.statement_timeout, select_limit),
        )
        .await
        {
            // The connection is still good — the bound was refused, the session was not
            // — so it goes back to the session rather than being thrown away.
            self.idle = Some(conn);
            return Err(error);
        }
        self.statement_timeout = options.statement_timeout;
        self.select_limit = select_limit;

        // Describe the statement **without running it**. `prep` is
        // COM_STMT_PREPARE: it returns the result columns and executes nothing, so
        // it costs one round trip and does no work.
        //
        // This is what makes cancel reachable through a *cursor*. MySQL sends a result
        // set's column descriptions when the result set *begins*, and for a blocking query
        // that is when the query finishes: without the describe, `execute` would return only
        // then (measured: `SELECT SLEEP(2)` took 2.002 s to `execute`), and a caller holding
        // no cursor could not cancel through it. The driver's own tests, and every caller that
        // runs a statement to completion before the next one, rely on `execute` returning
        // while the statement runs.
        //
        // A capped read is the one exception, and it skips the describe. Its only caller is
        // the preview, which races `execute` against a Stop (dropping the call) and reaches the
        // server through the connection id the producer publishes before it sends the query,
        // so it needs no cursor to stop; and the round trip is the one a preview pays for on
        // every Run. The columns then come from the result set's own definitions, as they do
        // for a statement the server declined to describe.
        let described = if options.row_limit.is_some() {
            None
        } else {
            conn.prep(sql).await.ok()
        };
        let (columns, column_types, binary) = describe(&described);
        // This describe is not the statement that runs — the producer prepares its
        // own, or runs the text it was given — so it is closed rather than left
        // occupying a slot in the connection's statement cache.
        if let Some(statement) = described {
            let _ = conn.close(statement).await;
        }

        let (sender, mut receiver) = mpsc::channel(BATCH_BACKLOG);
        let flight = Flight::new(sender.downgrade());
        self.flight = Some(Arc::clone(&flight));
        // Shared rather than sent as a message: the count belongs to the statement,
        // and the cursor may be asked for it before or after the stream is drained.
        let affected = Arc::new(Mutex::new(None));
        let producer = Producer {
            conn,
            sql: sql.to_owned(),
            params,
            row_limit: options.row_limit,
            batch_rows: DEFAULT_PRODUCER_BATCH,
            column_types,
            binary,
            // A statement the server declines to describe still has to run. Its
            // columns are then only known once the result set starts, so the
            // producer announces them and this waits — the old behaviour, kept
            // for the cases that cannot do better.
            announce_columns: columns.is_empty(),
            sender,
            connection_id: Arc::clone(&self.connection_id),
            affected: Arc::clone(&affected),
            timeout: options.statement_timeout,
            flight: Arc::clone(&flight),
        };
        tokio::spawn(producer.run());

        // The fast path: the columns are already known, so there is nothing to
        // wait for and the caller gets a cursor it can cancel immediately.
        if !columns.is_empty() {
            return Ok(Box::new(MysqlCursor {
                columns,
                receiver,
                pending: None,
                finished: false,
                affected,
            }));
        }

        match receiver.recv().await {
            Some(Message::Ready { columns }) => {
                // No columns means nothing will stream: the producer is about to finish,
                // and the next statement (a `COMMIT` after this `INSERT`) needs its
                // connection, so wait for it here rather than make the caller drain a
                // cursor that has nothing in it.
                if columns.is_empty() {
                    flight.wait_finished(HAND_BACK_WAIT).await;
                }
                Ok(Box::new(MysqlCursor {
                    columns,
                    receiver,
                    pending: None,
                    finished: false,
                    affected,
                }))
            }
            Some(Message::Failed(error)) => Err(sent_statement_failure(error)),
            Some(Message::Batch(_)) | None => Err(sent_statement_failure(EngineError::Internal {
                message: "the query task ended before describing its result".to_owned(),
            })),
        }
    }
}

#[async_trait]
impl Session for MysqlSession {
    fn capabilities(&self) -> Capabilities {
        MysqlSession::capabilities(self)
    }

    fn query_id(&self) -> Option<String> {
        let id = self.connection_id.load(Ordering::SeqCst);
        (id != 0).then(|| id.to_string())
    }

    async fn execute(
        &mut self,
        sql: &str,
        options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        self.run_statement(sql, options, Vec::new()).await
    }

    async fn execute_bound(
        &mut self,
        sql: &str,
        parameters: &[Parameter],
        options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        if parameters.is_empty() {
            return self.execute(sql, options).await;
        }
        // Typed rather than stringly: the prepared protocol carries these as
        // MySQL's own value shapes.
        let params: Vec<mysql_async::Value> = parameters.iter().map(mysql_value).collect();
        self.run_statement(sql, options, params).await
    }

    async fn browse(
        &mut self,
        level: BrowseLevel,
        path: &ObjectPath,
        include_system: bool,
    ) -> Result<Vec<String>, EngineError> {
        let sql = match level {
            // MySQL's top level is its databases, and the tree level is named
            // "database" where the other two drivers name it "catalog" — both
            // names arrive here and mean the same list.
            BrowseLevel::Catalog | BrowseLevel::Database => catalogs_sql(include_system),
            BrowseLevel::Table => {
                let database = path.schema.as_deref().ok_or_else(|| EngineError::Usage {
                    message: "DB_DATABASE is required to list tables on MySQL".to_owned(),
                })?;
                show_tables_sql(database)
            }
            // The one level this driver genuinely lacks, refused by name rather
            // than answered with an empty list — the wording the Python engine
            // used, hint included, so the message says what to use instead.
            BrowseLevel::Schema => {
                return Err(EngineError::Usage {
                    message: "mysql has no schemas level; catalogs lists its databases and \
                              tables lists their tables"
                        .to_owned(),
                })
            }
        };

        let rows = self.text_rows(&sql).await?;
        Ok(rows
            .into_iter()
            .filter_map(|mut row| row.drain(..).next().flatten())
            .collect())
    }

    async fn objects(&mut self, path: &ObjectPath) -> Result<ObjectsPage, EngineError> {
        // MySQL's first tree level is the database, and it is what selects which
        // tables are listed — so it is the `schema` field here, matching the
        // Python driver's `objects_sql(database, "")`.
        let database = path.schema.as_deref().ok_or_else(|| EngineError::Usage {
            message: "a database is required to list objects on MySQL".to_owned(),
        })?;
        let rows = self.text_rows(&objects_sql(database)).await?;
        Ok(ObjectsPage {
            columns: OBJECTS_COLUMNS
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            // A NULL cell is the empty string here: this grid draws a NULL and an
            // empty cell alike, the same rule the Python engine used.
            rows: rows
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|cell| cell.unwrap_or_default())
                        .collect()
                })
                .collect(),
        })
    }

    fn explain_statement(&self, sql: &str) -> String {
        // MySQL also rejects a trailing terminator, and a `;` inside a literal is
        // data — `strip_terminator` is what knows the difference. Read under MySQL's
        // own rules, so a `;` inside a `'\;'` backslash escape stays data.
        format!("EXPLAIN {}", strip_terminator_dialect(sql, Dialect::Mysql))
    }

    async fn cancel(&self) -> Result<(), EngineError> {
        let id = self.connection_id.load(Ordering::SeqCst);
        if id == 0 {
            // No connection was ever made, so there is nothing to stop.
            return Ok(());
        }

        // `KILL QUERY` and not `KILL CONNECTION`: the first stops the statement and
        // leaves the session usable, which is what the trait promises. The second
        // would drop the connection and take the tab's session with it.
        let mut killer =
            Conn::new(self.opts.clone())
                .await
                .map_err(|error| EngineError::Connect {
                    message: format!("opening a connection to cancel query {id}: {error}"),
                    kind: classify_connect_error(&error),
                })?;
        let statement = format!("KILL QUERY {id}");
        let outcome = killer
            .query_drop(&statement)
            .await
            .map_err(|error| map_query_error(error, &statement, None));
        let _ = killer.disconnect().await;
        match outcome {
            // 1094 is `ER_NO_SUCH_THREAD`: the connection is already gone, so the
            // statement is too. Idempotent by design: a user can press stop after the
            // query ended.
            Err(EngineError::Query {
                code: Some(code), ..
            }) if code == "1094" => Ok(()),
            other => other,
        }
    }

    async fn close(mut self: Box<Self>) -> Result<(), EngineError> {
        let conn = self
            .idle
            .take()
            .or_else(|| self.flight.take().and_then(|flight| flight.take_conn()));
        if let Some(conn) = conn {
            let _ = conn.disconnect().await;
        }
        Ok(())
    }

    /// `COM_RESET_CONNECTION`, then a look at which database the connection is in.
    ///
    /// The connection has to be back with the session: a statement that ended cleanly hands it
    /// back before its cursor says so (module note, "one session, one connection"), and one
    /// that did not (a limit reached, the cursor dropped) never does, so there is nothing to
    /// reset and the pool closes the session. The wait is bounded by the caller.
    ///
    /// A server without `COM_RESET_CONNECTION` (older than 5.7.3, and MariaDB before 10.2.4)
    /// answers `false`, and `COM_CHANGE_USER` does the same job there.
    async fn reset(&mut self) -> Result<(), EngineError> {
        let lost = || EngineError::Connect {
            message: "the connection was not handed back, so it cannot be reset".to_owned(),
            kind: FailureKind::Transient,
        };
        let mut conn = match self.idle.take() {
            Some(conn) => conn,
            None => {
                let flight = self.flight.take().ok_or_else(lost)?;
                if !flight.wait_finished(RESET_WAIT).await {
                    self.flight = Some(flight);
                    return Err(lost());
                }
                flight.take_conn().ok_or_else(lost)?
            }
        };
        let reset = async {
            let reset = conn
                .reset()
                .await
                .map_err(|error| map_query_error(error, "COM_RESET_CONNECTION", None))?;
            if !reset {
                conn.change_user(mysql_async::ChangeUserOpts::default())
                    .await
                    .map_err(|error| map_query_error(error, "COM_CHANGE_USER", None))?;
            }
            // The reset drops the connection's charset and collation back to the server's
            // globals; mysql_async only states them in the handshake. Restated here exactly as
            // the handshake does (utf8mb4 / utf8mb4_general_ci, or utf8 before 5.5.3), so a
            // reused connection compares and encodes text like a fresh one.
            let names = if conn.server_version() < (5, 5, 3) {
                "SET NAMES utf8 COLLATE utf8_general_ci"
            } else {
                "SET NAMES utf8mb4 COLLATE utf8mb4_general_ci"
            };
            conn.query_drop(names)
                .await
                .map_err(|error| map_query_error(error, names, None))?;
            conn.query_first::<Option<String>, _>("SELECT DATABASE()")
                .await
                .map_err(|error| map_query_error(error, "SELECT DATABASE()", None))
        }
        .await;
        let current = match reset {
            Ok(current) => current.flatten(),
            // A connection that failed to reset is dropped, not offered again.
            Err(error) => return Err(error),
        };
        // Whatever the session was connected with is what a session with no `USE` starts in.
        // A connection that ended up in a database the run never asked for (`USE` in user SQL,
        // and a server whose reset keeps it) cannot go back to "none", so it is not reused.
        if self.opts.db_name().is_none() && current.is_some() {
            return Err(EngineError::Internal {
                message: "a session opened without a database cannot leave the one it is in"
                    .to_owned(),
            });
        }
        self.current_db = current;
        self.idle = Some(conn);
        self.in_transaction = false;
        self.transaction_lost = false;
        self.statement_timeout = None;
        self.select_limit = None;
        Ok(())
    }

    fn set_context(&mut self, database: Option<&str>, _schema: Option<&str>) {
        self.wanted_db = database.map(str::to_owned);
    }

    /// Server-side read-only for every later transaction of this session, autocommit statements
    /// included. `COM_RESET_CONNECTION` clears it, and the guard keeps a user's `SET` from
    /// turning it back off.
    fn read_only_statement(&self) -> Option<&'static str> {
        Some("SET SESSION TRANSACTION READ ONLY")
    }
}

// --------------------------------------------------------------------------- //
// the streaming path
// --------------------------------------------------------------------------- //

/// The columns, types and binary flags a described statement reported.
///
/// All three come back empty when the statement could not be described, which is
/// also what a statement with no result set reports — the two are told apart in the
/// producer, by asking the result.
fn describe(
    statement: &Option<mysql_async::Statement>,
) -> (
    Vec<ColumnMeta>,
    Vec<mysql_async::consts::ColumnType>,
    Vec<bool>,
) {
    let Some(statement) = statement else {
        return (Vec::new(), Vec::new(), Vec::new());
    };
    let columns = statement
        .columns()
        .iter()
        .map(|column| ColumnMeta::new(column.name_str().to_string(), normalize::type_name(column)))
        .collect();
    let column_types = statement
        .columns()
        .iter()
        .map(mysql_async::Column::column_type)
        .collect();
    // `TEXT` and `BLOB` are the same protocol type, so the character set is read
    // once per column here rather than guessed at per value.
    let binary = statement
        .columns()
        .iter()
        .map(normalize::is_binary)
        .collect();
    (columns, column_types, binary)
}

/// What the producer sends the cursor.
enum Message {
    /// Column metadata, sent before any row so `execute` can return a cursor
    /// whose `columns()` is already valid.
    Ready {
        columns: Vec<ColumnMeta>,
    },
    Batch(ColumnBatch),
    Failed(EngineError),
}

/// Reads a statement on a connection it owns, feeding batches to the cursor.
struct Producer {
    conn: Conn,
    sql: String,
    /// Values to bind, in order. Empty takes the text protocol; non-empty takes
    /// the prepared protocol, which is the only one that carries parameters.
    params: Vec<mysql_async::Value>,
    row_limit: Option<usize>,
    batch_rows: usize,
    /// Known up front when the statement could be described.
    column_types: Vec<mysql_async::consts::ColumnType>,
    binary: Vec<bool>,
    /// Whether the columns still have to be announced, because describing up front
    /// did not work or was skipped.
    announce_columns: bool,
    sender: mpsc::Sender<Message>,
    connection_id: Arc<AtomicU32>,
    /// Rows the statement wrote, once the server has said.
    affected: Arc<Mutex<Option<u64>>>,
    /// The bound the statement runs under, so a server timeout names it.
    timeout: Option<Duration>,
    /// Where the connection goes when the statement is over.
    flight: Arc<Flight>,
}

impl Producer {
    async fn run(self) {
        // Captured before `produce` consumes the producer, so a failure can still be
        // reported.
        let sender = self.sender.clone();
        let flight = Arc::clone(&self.flight);
        let (outcome, conn) = self.produce().await;
        // Before the failure is announced, so a caller that sees it finds the connection.
        flight.finish(conn);
        if let Err(error) = outcome {
            // A closed receiver means the cursor was dropped, which is a user
            // cancelling a scroll rather than a failure. Sending is best effort.
            let _ = sender.send(Message::Failed(error)).await;
        }
        // The id is left in place, see `MysqlSession::connection_id`.
        //
        // A connection the session did not take back is dropped with `flight`'s state,
        // which closes the socket; `disconnect` is the tidy path but there is nothing to
        // report if it fails while the statement is already over.
    }

    /// Read the statement and feed the cursor.
    ///
    /// Consumes the producer so the result set can borrow `conn` for its whole
    /// life while the streaming loop borrows the other fields beside it. A
    /// statement with no values keeps the text protocol it always used; one with
    /// values takes the prepared protocol, which is where `?` exists.
    ///
    /// Returns the connection alongside the outcome when the statement ended somewhere
    /// the next one can start from: read to its end, or refused by the server with an
    /// answer. Anything else (a limit reached, the cursor gone, the socket failing)
    /// leaves the protocol mid-result or unknown, so the connection is not offered back.
    async fn produce(self) -> (Result<(), EngineError>, Option<Conn>) {
        let Producer {
            mut conn,
            sql,
            params,
            row_limit,
            batch_rows,
            column_types,
            binary,
            announce_columns,
            sender,
            connection_id,
            affected,
            timeout,
            flight: _,
        } = self;
        // Read before the query, because the result set takes the connection
        // mutably for as long as it lives.
        connection_id.store(conn.id(), Ordering::SeqCst);

        let config = StreamConfig {
            sql: &sql,
            row_limit,
            batch_rows,
            column_types: &column_types,
            binary: &binary,
            announce_columns,
            sender: &sender,
            affected: &affected,
            timeout,
        };

        let (outcome, clean) = if params.is_empty() {
            match conn.query_iter(&sql).await {
                Ok(mut result) => consume(&mut result, &config).await,
                Err(error) => refused(error, &sql, timeout),
            }
        } else {
            match conn.exec_iter(&sql, params).await {
                Ok(mut result) => consume(&mut result, &config).await,
                Err(error) => refused(error, &sql, timeout),
            }
        };
        (outcome, clean.then_some(conn))
    }
}

/// A statement the server (or the socket) refused, and whether the connection can still be
/// used: only an answer from the server says so.
fn refused(
    error: mysql_async::Error,
    sql: &str,
    timeout: Option<Duration>,
) -> (Result<(), EngineError>, bool) {
    let answered = matches!(error, mysql_async::Error::Server(_));
    (Err(map_query_error(error, sql, timeout)), answered)
}

/// The fields the streaming loop reads, borrowed beside the result set.
struct StreamConfig<'a> {
    sql: &'a str,
    row_limit: Option<usize>,
    batch_rows: usize,
    column_types: &'a [mysql_async::consts::ColumnType],
    binary: &'a [bool],
    announce_columns: bool,
    sender: &'a mpsc::Sender<Message>,
    affected: &'a Arc<Mutex<Option<u64>>>,
    timeout: Option<Duration>,
}

/// Drain a result set into batches, announcing columns first when they were not
/// described up front.
///
/// Free rather than a method because the result set holds `conn` mutably for its
/// whole life, and the loop still needs the fields beside it; they are passed as
/// one borrow group instead.
///
/// The `bool` is whether the result was read to its end (or ended in a server answer),
/// so the connection can serve the next statement.
async fn consume<P: mysql_async::prelude::Protocol>(
    result: &mut mysql_async::QueryResult<'_, '_, P>,
    config: &StreamConfig<'_>,
) -> (Result<(), EngineError>, bool) {
    let (column_types, binary) = if config.announce_columns {
        // Not described up front, so ask the result set. This is also where a
        // statement with no result set becomes distinguishable from one that
        // could not be described: an empty list here means there are no rows to
        // come.
        let columns: Vec<ColumnMeta> = result
            .columns_ref()
            .iter()
            .map(|column| {
                ColumnMeta::new(column.name_str().to_string(), normalize::type_name(column))
            })
            .collect();
        let types = result
            .columns_ref()
            .iter()
            .map(mysql_async::Column::column_type)
            .collect();
        let binary = result
            .columns_ref()
            .iter()
            .map(normalize::is_binary)
            .collect();
        if config
            .sender
            .send(Message::Ready { columns })
            .await
            .is_err()
        {
            return (Ok(()), false);
        }
        (types, binary)
    } else {
        (config.column_types.to_vec(), config.binary.to_vec())
    };

    // A statement with no result set — DDL, or an UPDATE — has nothing to
    // stream, and saying so now is better than sending empty batches. It does
    // have a count: `CREATE TABLE ... AS SELECT` reports the rows it wrote in the
    // OK packet the result already read, and so does an `INSERT`.
    if column_types.is_empty() {
        record_affected(config.affected, result);
        return (Ok(()), true);
    }

    let mut batch = BatchBuilder::new(column_types, binary, config.batch_rows);
    let mut emitted = 0usize;
    // Cleared when the loop stops before the server's last packet.
    let mut exhausted = true;

    loop {
        let row = match result.next().await {
            Ok(Some(row)) => row,
            Ok(None) => break,
            Err(error) => return refused(error, config.sql, config.timeout),
        };
        if let Some(limit) = config.row_limit {
            if emitted + batch.rows >= limit {
                exhausted = false;
                break;
            }
        }
        batch.push_row(&row);
        if batch.rows >= config.batch_rows {
            let full = match batch.take() {
                Ok(full) => full,
                Err(error) => return (Err(error), false),
            };
            emitted += full.rows();
            if config.sender.send(Message::Batch(full)).await.is_err() {
                return (Ok(()), false);
            }
        }
    }

    if batch.rows > 0 {
        match batch.take() {
            Ok(rest) => {
                let _ = config.sender.send(Message::Batch(rest)).await;
            }
            Err(error) => return (Err(error), false),
        }
    }
    // Read after the stream is exhausted: the OK packet that carries the count is
    // the one that ends it.
    record_affected(config.affected, result);
    (Ok(()), exhausted)
}

/// One bound value in MySQL's own value shapes.
///
/// Typed rather than stringly: the prepared protocol carries the variant, so a
/// `BIGINT` goes as an integer and a `TEXT` as bytes, not as everything-is-a-string.
fn mysql_value(value: &Parameter) -> mysql_async::Value {
    match value {
        Parameter::Null => mysql_async::Value::NULL,
        // MySQL has no boolean type; a `BOOL` column is `TINYINT(1)`, and its
        // literals are 1 and 0.
        Parameter::Bool(true) => mysql_async::Value::Int(1),
        Parameter::Bool(false) => mysql_async::Value::Int(0),
        Parameter::Int(number) => mysql_async::Value::Int(*number),
        Parameter::UInt(number) => mysql_async::Value::UInt(*number),
        Parameter::Float(number) => mysql_async::Value::Double(*number),
        Parameter::Text(text) => mysql_async::Value::Bytes(text.as_bytes().to_vec()),
    }
}

/// Keep what the server said about rows written.
///
/// A free function rather than a method because the result set holds the connection
/// mutably for as long as it lives, so the only thing that can be borrowed beside it is
/// the shared slot itself.
///
/// MySQL answers `0` when a statement has nothing to report, and that zero is kept
/// rather than turned into "no answer": a `DROP TABLE` really does affect no rows, and
/// pymysql's `rowcount` said `0` for it too.
fn record_affected<P: mysql_async::prelude::Protocol>(
    affected: &Arc<Mutex<Option<u64>>>,
    result: &mysql_async::QueryResult<'_, '_, P>,
) {
    let count = result.affected_rows();
    if let Ok(mut slot) = affected.lock() {
        *slot = Some(count);
    }
}

/// Rows being accumulated column by column for the next batch.
struct BatchBuilder {
    column_types: Vec<mysql_async::consts::ColumnType>,
    /// Whether each column holds bytes rather than text, decided once per column.
    binary: Vec<bool>,
    columns: Vec<Vec<Value>>,
    rows: usize,
}

impl BatchBuilder {
    fn new(
        column_types: Vec<mysql_async::consts::ColumnType>,
        binary: Vec<bool>,
        capacity: usize,
    ) -> Self {
        let columns = column_types
            .iter()
            .map(|_| Vec::with_capacity(capacity))
            .collect();
        Self {
            column_types,
            binary,
            columns,
            rows: 0,
        }
    }

    fn push_row(&mut self, row: &mysql_async::Row) {
        for (index, column_type) in self.column_types.iter().enumerate() {
            let value = normalize::from_value(*column_type, row.as_ref(index), self.binary[index]);
            self.columns[index].push(value);
        }
        self.rows += 1;
    }

    /// Take the rows collected so far, leaving the builder empty.
    ///
    /// Returns a `Result` rather than falling back to an empty batch: a shape the
    /// store refuses means a bug in this builder, and silently handing back no
    /// rows would turn that bug into a result that is quietly missing data.
    fn take(&mut self) -> Result<ColumnBatch, EngineError> {
        let columns = std::mem::replace(
            &mut self.columns,
            self.column_types
                .iter()
                .map(|_| Vec::with_capacity(0))
                .collect(),
        );
        self.rows = 0;
        ColumnBatch::new(columns).map_err(|error| EngineError::Internal {
            message: format!("batch shape: {error}"),
        })
    }
}

/// Rows from a running statement, in batches.
struct MysqlCursor {
    columns: Vec<ColumnMeta>,
    receiver: mpsc::Receiver<Message>,
    /// Rows the producer read beyond what the caller asked for.
    pending: Option<ColumnBatch>,
    finished: bool,
    /// Rows the statement wrote, filled in by the producer as it finishes. `None`
    /// means the server has not said yet — not that nothing was written.
    affected: Arc<Mutex<Option<u64>>>,
}

#[async_trait]
impl Cursor for MysqlCursor {
    fn columns(&self) -> &[ColumnMeta] {
        &self.columns
    }

    fn affected_rows(&self) -> Option<u64> {
        self.affected.lock().ok().and_then(|affected| *affected)
    }

    async fn next_batch(&mut self, max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        if self.finished || max_rows == 0 {
            self.finished = true;
            return Ok(None);
        }

        // Serve from the remainder first, so a caller asking for 100 out of a
        // producer batch of 1024 still gets exactly 100.
        if let Some(pending) = self.pending.take() {
            if pending.rows() > max_rows {
                let rest = pending
                    .slice_rows(max_rows..pending.rows())
                    .map_err(|error| EngineError::Internal {
                        message: format!("slicing a batch failed: {error}"),
                    })?;
                let head =
                    pending
                        .slice_rows(0..max_rows)
                        .map_err(|error| EngineError::Internal {
                            message: format!("slicing a batch failed: {error}"),
                        })?;
                self.pending = Some(rest);
                return Ok(Some(head));
            }
            return Ok(Some(pending));
        }

        match self.receiver.recv().await {
            Some(Message::Batch(batch)) => {
                if batch.rows() > max_rows {
                    let rest = batch.slice_rows(max_rows..batch.rows()).map_err(|error| {
                        EngineError::Internal {
                            message: format!("slicing a batch failed: {error}"),
                        }
                    })?;
                    let head =
                        batch
                            .slice_rows(0..max_rows)
                            .map_err(|error| EngineError::Internal {
                                message: format!("slicing a batch failed: {error}"),
                            })?;
                    self.pending = Some(rest);
                    Ok(Some(head))
                } else {
                    Ok(Some(batch))
                }
            }
            Some(Message::Failed(error)) => {
                self.finished = true;
                Err(error)
            }
            // The channel closed with no failure queued: the producer finished.
            Some(Message::Ready { .. }) | None => {
                self.finished = true;
                Ok(None)
            }
        }
    }
}

impl Drop for MysqlCursor {
    fn drop(&mut self) {
        // The producer's next `send` fails, which ends its loop, and the
        // connection it owns is dropped with it. Without this, a cursor dropped
        // mid-scroll would leave the task reading a whole result nobody wants.
        self.receiver.close();
    }
}

// --------------------------------------------------------------------------- //
// statements
// --------------------------------------------------------------------------- //

/// The databases, for the tree's top level and the context picker.
///
/// `information_schema.SCHEMATA` rather than `SHOW DATABASES`, because a `WHERE`
/// clause can filter it and `SHOW` cannot. The two list the same thing; the
/// filtered form is the reason this is not a one-liner.
///
/// The system databases are hidden by default. They are real databases and a user
/// with privileges can read them, but they are not what someone browsing a tree is
/// looking for. Reproduced from `exporter/drivers.py` and pinned by tests.
fn catalogs_sql(include_system: bool) -> String {
    const SYSTEM_DATABASES: [&str; 4] =
        ["information_schema", "mysql", "performance_schema", "sys"];
    let filter = if include_system {
        String::new()
    } else {
        let names = SYSTEM_DATABASES
            .iter()
            .map(|name| quote_literal(name))
            .collect::<Vec<_>>()
            .join(", ");
        format!("WHERE SCHEMA_NAME NOT IN ({names}) ")
    };
    format!("SELECT SCHEMA_NAME FROM information_schema.SCHEMATA {filter}ORDER BY 1")
}

/// A database name is an identifier here, so it is back-quoted with the quote
/// character doubled — `quote_ident` already knows how, and it is the same rule
/// the Python driver used.
fn show_tables_sql(database: &str) -> String {
    format!(
        "SHOW TABLES FROM {}",
        qh_sql::quote_ident(qh_sql::IdentStyle::Mysql, database)
    )
}

/// The objects grid's four columns and statement, unchanged from the Python
/// driver so the grid looks the same after the migration.
fn objects_sql(database: &str) -> String {
    format!(
        "SELECT TABLE_NAME, ENGINE, TABLE_ROWS, TABLE_COMMENT \
         FROM information_schema.TABLES \
         WHERE TABLE_SCHEMA = {} ORDER BY 1",
        quote_literal(database)
    )
}

/// A string literal with its quotes doubled.
///
/// A literal and an identifier are different problems: the schema in a comparison
/// is a literal, so `quote_ident` would be wrong here.
fn quote_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// The error a failed `connect` reports, with what it means for a caller.
///
/// A refused certificate gets its own wording, because the sentence a user needs
/// is that the connection **did not** happen in clear — the same failure an
/// attacker would produce, reported rather than resolved by falling back.
fn connect_failure(config: &ConnectionConfig, error: &mysql_async::Error) -> EngineError {
    let message = match tls::certificate_rejection(error) {
        Some(why) => format!(
            "{}: the server's certificate was not accepted ({why}); the connection was not made \
             in clear",
            config.redacted()
        ),
        None => format!("{}: {error}", config.redacted()),
    };
    EngineError::Connect {
        message,
        kind: classify_connect_error(error),
    }
}

fn classify_connect_error(error: &mysql_async::Error) -> FailureKind {
    match error {
        // The server answered and refused: wrong password, unknown database,
        // host not allowed. Retrying cannot help.
        mysql_async::Error::Server(_) => FailureKind::Permanent,
        // A certificate this client will not accept does not become acceptable
        // by trying again: the trust store or the mode has to change. Reported
        // as permanent so a caller does not burn retries on it.
        error if tls::certificate_rejection(error).is_some() => FailureKind::Permanent,
        _ => FailureKind::Transient,
    }
}

fn map_query_error(error: mysql_async::Error, sql: &str, timeout: Option<Duration>) -> EngineError {
    match &error {
        mysql_async::Error::Server(server) => {
            // 3024 is `ER_QUERY_TIMEOUT`: "Query execution was interrupted, maximum
            // statement execution time exceeded", which is what `max_execution_time`
            // raises. The server's own sentence is kept, because it says the same
            // thing in words the user can search for.
            if server.code == 3024 {
                return EngineError::statement_timeout(timeout, &server.message);
            }
            EngineError::Query {
                message: match sql.is_empty() {
                    true => server.message.clone(),
                    false => format!("{}: {}", server.message, snippet(sql)),
                },
                // MySQL's numeric error code, as a string so it matches the shape
                // PostgreSQL's SQLSTATE takes. 1317 is "query interrupted", which is
                // what a `KILL QUERY` produces.
                code: Some(server.code.to_string()),
                position: None,
                kind: FailureKind::Permanent,
            }
        }
        mysql_async::Error::Io(_) => EngineError::Connect {
            message: format!("the connection failed while the query ran: {error}"),
            kind: FailureKind::Transient,
        },
        _ => EngineError::Query {
            message: error.to_string(),
            code: None,
            position: None,
            kind: FailureKind::Transient,
        },
    }
}

/// A failure that came back from a statement whose text has been sent.
///
/// A server's answer (it carries a code) and a timeout are reported as they are. Anything else
/// (the socket dying, the task ending) does not say whether the server ran the statement, and the
/// layers above resend an `execute` that failed as a transient connection error, which would run
/// a statement with a side effect twice. So it is made permanent and coded like the client
/// library's own "lost connection" (`2013`), and says so.
fn sent_statement_failure(error: EngineError) -> EngineError {
    match error {
        EngineError::Connect { .. }
        | EngineError::Internal { .. }
        | EngineError::Query { code: None, .. } => EngineError::Query {
            message: "the connection was lost after the statement was sent; it was not sent again"
                .to_owned(),
            code: Some("2013".to_owned()),
            position: None,
            kind: FailureKind::Permanent,
        },
        other => other,
    }
}

/// The `sql_select_limit` a statement runs under, when it runs under one.
///
/// A capped preview asks the server to stop producing rows at the cap, so the session's
/// connection ends the statement itself and goes back to the pool instead of being dropped
/// mid-result. That is only sound for a statement whose rows are nothing but a result: for
/// `INSERT … SELECT` or `CREATE TABLE … SELECT` the limit would cut the rows *written*, and for
/// `SELECT … FOR UPDATE` the rows locked. So it is applied to a single statement that reads under
/// **every** lexer the server might use ([`classify_readings`]), which leaves those out because
/// their write or lock words classify them as writes. Anything else keeps the client-side cap
/// alone, and its connection is dropped when the cap stops it. The statement text is never
/// changed; an explicit `LIMIT` in it still wins over the session variable.
fn select_limit_for(sql: &str, options: &ExecuteOptions) -> Option<usize> {
    let limit = options.row_limit?;
    (is_single_statement(sql)
        && classify_readings(sql, Dialect::Mysql.readings()) == StatementKind::ReadOnly)
        .then_some(limit)
}

/// The `SET` that takes the server from `current` to `wanted` (timeout, select limit), or `None`
/// when they agree. One statement for both, so a preview that changes both costs one round trip.
fn settings_statement(
    current: (Option<Duration>, Option<usize>),
    wanted: (Option<Duration>, Option<usize>),
) -> Option<String> {
    let mut parts = Vec::new();
    if current.0 != wanted.0 {
        // `0` is `max_execution_time`'s own value for "no bound".
        let milliseconds = wanted.0.map_or(0, |limit| {
            u64::try_from(limit.as_millis()).unwrap_or(u64::MAX)
        });
        parts.push(format!("SESSION max_execution_time = {milliseconds}"));
    }
    if current.1 != wanted.1 {
        parts.push(match wanted.1 {
            Some(limit) => format!("SESSION sql_select_limit = {limit}"),
            None => "SESSION sql_select_limit = DEFAULT".to_owned(),
        });
    }
    (!parts.is_empty()).then(|| format!("SET {}", parts.join(", ")))
}

/// Put `wanted` in force on the server, when it is not there already.
///
/// MySQL's server-side bound is the `max_execution_time` session variable, in
/// milliseconds. It is enforced for read-only `SELECT` statements only — the module note says
/// what that leaves uncovered. `sql_select_limit` rides in the same `SET`, see
/// [`select_limit_for`].
async fn apply_session_settings(
    conn: &mut Conn,
    current: (Option<Duration>, Option<usize>),
    wanted: (Option<Duration>, Option<usize>),
) -> Result<(), EngineError> {
    let Some(statement) = settings_statement(current, wanted) else {
        return Ok(());
    };
    conn.query_drop(&statement)
        .await
        .map_err(|error| map_query_error(error, &statement, wanted.0))
}

fn snippet(sql: &str) -> String {
    const LIMIT: usize = 60;
    let flattened = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    if flattened.chars().count() <= LIMIT {
        return flattened;
    }
    let head: String = flattened.chars().take(LIMIT).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capped(rows: usize) -> ExecuteOptions {
        ExecuteOptions {
            row_limit: Some(rows),
            ..ExecuteOptions::default()
        }
    }

    #[test]
    fn a_select_limit_is_asked_only_for_a_single_read_with_a_row_limit() {
        assert_eq!(select_limit_for("SELECT * FROM t", &capped(11)), Some(11));
        assert_eq!(
            select_limit_for("WITH x AS (SELECT 1) SELECT * FROM x;", &capped(5)),
            Some(5)
        );
        // No cap, no limit.
        assert_eq!(
            select_limit_for("SELECT 1", &ExecuteOptions::default()),
            None
        );
        // A limit on these would cut the rows written or locked.
        for sql in [
            "INSERT INTO t SELECT * FROM s",
            "CREATE TABLE t2 AS SELECT * FROM t",
            "SELECT * FROM t INTO OUTFILE '/tmp/x'",
            "SELECT * FROM t FOR UPDATE",
            "SELECT * FROM t LOCK IN SHARE MODE",
            "CALL p()",
            "REPLACE INTO t SELECT * FROM s",
        ] {
            assert_eq!(select_limit_for(sql, &capped(11)), None, "{sql}");
        }
        // More than one statement is refused before it is sent, and never limited.
        assert_eq!(select_limit_for("SELECT 1; SELECT 2", &capped(11)), None);
    }

    #[test]
    fn one_set_carries_the_timeout_and_the_select_limit() {
        let second = Duration::from_secs(1);
        assert_eq!(settings_statement((None, None), (None, None)), None);
        assert_eq!(
            settings_statement((None, None), (Some(second), Some(11))).as_deref(),
            Some("SET SESSION max_execution_time = 1000, SESSION sql_select_limit = 11")
        );
        // Only what changed is sent, and a limit going away is the server's own default.
        assert_eq!(
            settings_statement((Some(second), Some(11)), (Some(second), None)).as_deref(),
            Some("SET SESSION sql_select_limit = DEFAULT")
        );
        assert_eq!(
            settings_statement((Some(second), None), (None, None)).as_deref(),
            Some("SET SESSION max_execution_time = 0")
        );
    }

    #[test]
    fn this_driver_is_mysql_and_says_so() {
        let driver = MysqlDriver::new();
        assert_eq!(driver.kind(), DriverKind::Mysql);
        assert_eq!(driver.label(), "MySQL");
        assert_eq!(driver.default_port(), 3306);
    }

    #[test]
    fn the_tree_starts_at_databases_not_catalogs_or_schemas() {
        let capabilities = MysqlDriver.capabilities();
        assert_eq!(
            capabilities.levels,
            vec![BrowseLevel::Database, BrowseLevel::Table]
        );
        assert!(!capabilities.levels.contains(&BrowseLevel::Schema));
    }

    #[test]
    fn cancel_and_persistence_are_declared() {
        let capabilities = MysqlDriver.capabilities();
        assert!(capabilities.cancel);
        assert!(capabilities.persistent_connection);
    }

    #[test]
    fn a_statement_bound_is_declared_as_the_servers_own() {
        // `max_execution_time`, which MySQL enforces itself. A driver that could not
        // would say `false` rather than pretend.
        assert!(MysqlDriver.capabilities().statement_timeout);
    }

    #[test]
    fn this_driver_binds_with_question_marks() {
        let capabilities = MysqlDriver.capabilities();
        assert_eq!(capabilities.parameters, Some(ParameterStyle::Question));
        assert!(!capabilities.read_only);
    }

    #[test]
    fn a_bound_value_keeps_its_native_shape() {
        // The prepared protocol carries the variant, so this is typed rather than
        // every-value-as-text.
        assert_eq!(mysql_value(&Parameter::Null), mysql_async::Value::NULL);
        assert_eq!(
            mysql_value(&Parameter::Bool(true)),
            mysql_async::Value::Int(1)
        );
        assert_eq!(
            mysql_value(&Parameter::Bool(false)),
            mysql_async::Value::Int(0)
        );
        assert_eq!(
            mysql_value(&Parameter::Int(-7)),
            mysql_async::Value::Int(-7)
        );
        assert_eq!(
            mysql_value(&Parameter::UInt(u64::MAX)),
            mysql_async::Value::UInt(u64::MAX)
        );
        assert_eq!(
            mysql_value(&Parameter::Float(1.5)),
            mysql_async::Value::Double(1.5)
        );
        assert_eq!(
            mysql_value(&Parameter::Text("O'Brien".to_owned())),
            mysql_async::Value::Bytes(b"O'Brien".to_vec())
        );
    }

    #[test]
    fn error_3024_becomes_a_typed_timeout_that_names_the_limit() {
        let error = map_query_error(
            mysql_async::Error::Server(mysql_async::ServerError {
                code: 3024,
                message: "Query execution was interrupted, maximum statement execution \
                          time exceeded"
                    .to_owned(),
                state: "HY000".to_owned(),
            }),
            "SELECT SLEEP(3)",
            Some(Duration::from_millis(500)),
        );
        match &error {
            EngineError::Timeout { message, limit_ms } => {
                assert_eq!(*limit_ms, Some(500));
                assert!(message.contains("500 ms"), "{message}");
                assert!(
                    message.contains("maximum statement execution time"),
                    "the server's own sentence is kept: {message}"
                );
            }
            other => panic!("expected a typed timeout, got {other:?}"),
        }
        // A bound is the caller's, so a retry would only wait for it again.
        assert_eq!(error.failure_kind(), FailureKind::Permanent);
    }

    #[test]
    fn an_ordinary_server_error_is_not_mistaken_for_a_timeout() {
        // 1317 is "query interrupted", which is what a `KILL QUERY` produces, and it
        // must stay a query error: the user asked for the stop.
        let error = map_query_error(
            mysql_async::Error::Server(mysql_async::ServerError {
                code: 1317,
                message: "Query execution was interrupted".to_owned(),
                state: "70100".to_owned(),
            }),
            "SELECT 1",
            Some(Duration::from_millis(500)),
        );
        assert!(matches!(error, EngineError::Query { .. }), "{error:?}");
    }

    #[test]
    fn the_objects_grid_reports_the_same_four_columns_as_before() {
        assert_eq!(
            MysqlDriver.capabilities().objects_columns,
            vec![
                "Name".to_owned(),
                "Engine".to_owned(),
                "Rows".to_owned(),
                "Comment".to_owned(),
            ]
        );
    }

    /// The statement and columns the Python driver used, pinned so the grid looks
    /// the same after the migration. The columns are what label each cell, so a
    /// SELECT list that drifted out of step would put every value under the wrong
    /// header — silently, because both are strings.
    #[test]
    fn the_objects_statement_matches_the_one_it_replaces() {
        assert_eq!(
            objects_sql("sips"),
            "SELECT TABLE_NAME, ENGINE, TABLE_ROWS, TABLE_COMMENT \
             FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = 'sips' ORDER BY 1"
        );
    }

    /// The statement the Python driver built, pinned verbatim.
    ///
    /// Expectation taken from running `exporter/drivers.py`, not retyped from
    /// reading it — the system-database list and the ordering are both easy to get
    /// subtly wrong from prose.
    #[test]
    fn the_catalogs_statement_matches_the_one_it_replaces() {
        assert_eq!(
            catalogs_sql(false),
            "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA \
             WHERE SCHEMA_NAME NOT IN ('information_schema', 'mysql', \
             'performance_schema', 'sys') ORDER BY 1"
        );
        assert_eq!(
            catalogs_sql(true),
            "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA ORDER BY 1"
        );
    }

    #[test]
    fn the_system_databases_are_hidden_by_default_and_only_by_name() {
        // Nothing is hidden when the user asks for everything, and the default
        // hides exactly the four MySQL keeps for itself — not a `LIKE 'inf%'`
        // that would also swallow a user database called `influencer`.
        let filtered = catalogs_sql(false);
        for name in ["information_schema", "mysql", "performance_schema", "sys"] {
            assert!(
                filtered.contains(&format!("'{name}'")),
                "{name} missing from {filtered}"
            );
        }
        assert_eq!(
            catalogs_sql(true),
            "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA ORDER BY 1"
        );
    }

    #[test]
    fn a_database_name_cannot_break_out_of_its_literal_or_its_backticks() {
        // Two different escapes, for two different syntactic positions.
        assert_eq!(quote_literal("sips"), "'sips'");
        assert_eq!(quote_literal("a'b"), "'a''b'");
        assert_eq!(show_tables_sql("my`db"), "SHOW TABLES FROM `my``db`");
        assert_eq!(show_tables_sql("sips"), "SHOW TABLES FROM `sips`");
    }

    #[test]
    fn explain_drops_a_terminator_but_not_a_semicolon_inside_text() {
        // The same expression `explain_statement` builds.
        let session =
            |sql: &str| format!("EXPLAIN {}", strip_terminator_dialect(sql, Dialect::Mysql));
        assert_eq!(session("SELECT 1"), "EXPLAIN SELECT 1");
        assert_eq!(session("SELECT 1;"), "EXPLAIN SELECT 1");
        // A `;` inside a literal is data and must survive.
        assert_eq!(session("SELECT 'a;b;'"), "EXPLAIN SELECT 'a;b;'");
        // Read under MySQL's rules: a `;` after a backslash-escaped quote is inside the
        // string, so it stays data rather than becoming a terminator to peel.
        assert_eq!(session("SELECT '\\';'"), "EXPLAIN SELECT '\\';'");
    }

    #[test]
    fn a_multi_statement_request_is_rejected_before_it_is_sent() {
        // The backstop `run_statement` applies (W3-T0): a text any lexer the server might
        // use reads as more than one statement never reaches the always-multi-statement
        // text protocol.
        assert!(is_single_statement("SELECT 1"));
        assert!(is_single_statement("SELECT 1;"));
        assert!(is_single_statement("SELECT 'a;b'"));
        // The engine allows a single read with a trailing comment; so does the backstop.
        assert!(is_single_statement("SELECT 1; -- note"));
        assert!(is_single_statement("SELECT 1 # note; more"));
        assert!(is_single_statement("SELECT 5--2"));
        assert!(is_single_statement("SELECT /*+ NO_INDEX(t) */ 1"));
        // A backslash-escaped quote and a trailing `;` is one statement under every reading.
        assert!(is_single_statement("INSERT INTO t VALUES ('it\\'s');"));
        assert!(!is_single_statement("SELECT '\\''; DELETE FROM t; -- '"));
        assert!(!is_single_statement("SELECT '\\''; DELETE FROM t; # '"));
        assert!(!is_single_statement("SELECT '\\'; DELETE FROM t; -- '"));
        assert!(!is_single_statement(
            "SELECT 1 \"\\\" ; DELETE FROM t ; -- \""
        ));
        assert!(!is_single_statement("SELECT 1; SELECT 2"));
        assert!(!is_single_statement("SELECT 1 --x; DELETE FROM t"));
        assert!(!is_single_statement("SELECT 1 /*! ; DROP TABLE t */"));
        assert!(!is_single_statement("SELECT 1 /*!50000 ; DROP TABLE t */"));
        assert!(!is_single_statement(
            "SELECT 1 /*!99999 ' */ ; DROP TABLE t ; -- '"
        ));
    }

    #[test]
    fn a_dollar_tag_does_not_hide_a_statement_from_the_backstop() {
        // MySQL has no dollar quoting: `$` is an identifier character, so the `;` between
        // two `$tag$` markers separates statements the server runs. A reading that treated
        // the markers as a string would call these one statement.
        assert!(!is_single_statement(
            "SELECT 1 AS x$a$ ; DELETE FROM t ; $a$"
        ));
        assert!(!is_single_statement("SELECT $a$ ; DELETE FROM t ; $a$"));
        assert!(!is_single_statement("SELECT $$; DELETE FROM t; $$"));
        assert!(!is_single_statement("SELECT 1 $tag$; DROP TABLE t; $tag$"));
        assert!(!is_single_statement("SELECT $a$ ; SELECT 2 ; $a$"));
        // A `$` inside an identifier or a string is nothing special.
        assert!(is_single_statement("SELECT a$b FROM t$1"));
        assert!(is_single_statement(
            "SELECT JSON_EXTRACT(doc, '$.a') FROM t"
        ));
    }

    #[test]
    fn a_batch_is_sliced_to_the_ceiling_the_caller_asked_for() {
        // The producer reads in its own batches; the caller's ceiling still holds.
        let batch = ColumnBatch::new(vec![
            (1..=10).map(Value::Int).collect(),
            (11..=20).map(Value::Int).collect(),
        ])
        .unwrap();
        let head = batch.slice_rows(0..3).unwrap();
        let rest = batch.slice_rows(3..10).unwrap();
        assert_eq!(head.rows(), 3);
        assert_eq!(rest.rows(), 7);
        // And the columns stay aligned across the cut.
        assert_eq!(
            head.columns()[1],
            vec![Value::Int(11), Value::Int(12), Value::Int(13)]
        );
        assert_eq!(rest.columns()[1][0], Value::Int(14));
    }

    #[test]
    fn an_error_message_shows_a_readable_snippet_of_the_statement() {
        assert_eq!(snippet("SELECT 1"), "SELECT 1");
        assert_eq!(snippet("SELECT\n  1"), "SELECT 1");
        assert!(snippet(&"x".repeat(200)).ends_with('…'));
    }

    // `map_query_error` is unit-tested above for the one variant that can be built
    // from outside the crate (`Error::Server`, which is what carries error 3024). It is
    // exercised against a live server in `tests/integration.rs` too, which produces a
    // real authentication failure, a real syntax error, a real `KILL QUERY`, and a real
    // `max_execution_time` timeout.
    //
    // `classify_connect_error` and `connect_failure` are pinned in `src/tls.rs`
    // for the one variant that *can* be built from outside (a rejected
    // certificate) and against a live server in `tests/tls.rs` for the rest.
}
