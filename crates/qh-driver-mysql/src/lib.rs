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
//! `execute` waits for the producer's first message before returning, so
//! `columns()` is valid immediately as the trait requires. Only the column
//! descriptions have to arrive for that, and they come before any row — so this
//! wait does not delay the first row.
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
    ObjectPath, ObjectsPage, Parameter, ParameterStyle, Session,
};
use qh_sql::strip_terminator;
use tokio::sync::mpsc;

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
            // Nothing has been asked of the server yet, so the session must not send a
            // `SET SESSION max_execution_time = 0` on its first statement and override a
            // bound the server's configuration set.
            statement_timeout: None,
        }))
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
        .prefer_socket(false);
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
    /// The id `KILL QUERY` needs, published by whichever connection is running a
    /// statement. Zero means nothing is running.
    connection_id: Arc<AtomicU32>,
    /// The connection kept for the next statement, so an idle session is one
    /// connection rather than a new handshake per query.
    idle: Option<Conn>,
    /// The `max_execution_time` this session last sent to the server, so the `SET`
    /// is sent when the bound changes rather than on every statement.
    statement_timeout: Option<Duration>,
}

impl MysqlSession {
    fn capabilities(&self) -> Capabilities {
        MysqlDriver.capabilities()
    }

    /// The connection to use for a metadata call, opening one if the idle
    /// connection was handed to a running statement.
    async fn connection(&mut self) -> Result<Conn, EngineError> {
        if let Some(conn) = self.idle.take() {
            return Ok(conn);
        }
        Conn::new(self.opts.clone())
            .await
            .map_err(|error| EngineError::Connect {
                message: error.to_string(),
                kind: classify_connect_error(&error),
            })
    }

    /// Run one of our own statements and read every cell as text.
    ///
    /// Used by browse and objects, which are our statements with a known shape —
    /// unlike a user's query, they do not need the streaming path.
    async fn text_rows(&mut self, sql: &str) -> Result<Vec<Vec<Option<String>>>, EngineError> {
        let mut conn = self.connection().await?;
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
        let mut conn = self.connection().await?;

        // The bound is a server setting, so it reaches the server before the statement
        // does. Applied when it changed: `SET` is a round trip.
        if let Err(error) =
            apply_max_execution_time(&mut conn, self.statement_timeout, options.statement_timeout)
                .await
        {
            // The connection is still good — the bound was refused, the session was not
            // — so it goes back to the session rather than being thrown away.
            self.idle = Some(conn);
            return Err(error);
        }
        self.statement_timeout = options.statement_timeout;

        // Describe the statement **without running it**. `prep` is
        // COM_STMT_PREPARE: it returns the result columns and executes nothing, so
        // it costs one round trip and does no work.
        //
        // This is not a tidy-up, it is what makes cancel reachable at all. MySQL
        // sends a result set's column descriptions when the result set *begins*,
        // and for a blocking query that is when the query finishes. Waiting for
        // them here therefore waited for the whole query. Measured against the dev
        // container: `execute(SELECT SLEEP(2))` returned after 2.002 s, and a long
        // join had not returned after 5 s. During that window `execute` had not
        // handed anything back, so the caller had no cursor to cancel and pressing
        // stop did nothing. Describing that same join first returns in 915 µs.
        let described = conn.prep(sql).await.ok();
        let (columns, column_types, binary) = describe(&described);
        // This describe is not the statement that runs — the producer prepares its
        // own, or runs the text it was given — so it is closed rather than left
        // occupying a slot in the connection's statement cache.
        if let Some(statement) = described {
            let _ = conn.close(statement).await;
        }

        let (sender, mut receiver) = mpsc::channel(BATCH_BACKLOG);
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
            Some(Message::Ready { columns }) => Ok(Box::new(MysqlCursor {
                columns,
                receiver,
                pending: None,
                finished: false,
                affected,
            })),
            Some(Message::Failed(error)) => Err(error),
            Some(Message::Batch(_)) | None => Err(EngineError::Internal {
                message: "the query task ended before describing its result".to_owned(),
            }),
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
        // data — `strip_terminator` is what knows the difference.
        format!("EXPLAIN {}", strip_terminator(sql))
    }

    async fn cancel(&self) -> Result<(), EngineError> {
        let id = self.connection_id.load(Ordering::SeqCst);
        if id == 0 {
            // Nothing has run on this session yet, so there is nothing to stop.
            // Idempotent by design: a user can press stop after the query ended.
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
        outcome
    }

    async fn close(mut self: Box<Self>) -> Result<(), EngineError> {
        if let Some(conn) = self.idle.take() {
            let _ = conn.disconnect().await;
        }
        Ok(())
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
    /// did not work.
    announce_columns: bool,
    sender: mpsc::Sender<Message>,
    connection_id: Arc<AtomicU32>,
    /// Rows the statement wrote, once the server has said.
    affected: Arc<Mutex<Option<u64>>>,
    /// The bound the statement runs under, so a server timeout names it.
    timeout: Option<Duration>,
}

impl Producer {
    async fn run(self) {
        // Captured before `produce` consumes the producer: the sender so a
        // failure can still be reported, the id so it can be cleared once the
        // statement is over.
        let connection_id = Arc::clone(&self.connection_id);
        let sender = self.sender.clone();
        if let Err(error) = self.produce().await {
            // A closed receiver means the cursor was dropped, which is a user
            // cancelling a scroll rather than a failure. Sending is best effort.
            let _ = sender.send(Message::Failed(error)).await;
        }
        connection_id.store(0, Ordering::SeqCst);
        // Dropping the connection closes the socket; `disconnect` is the tidy
        // path but it consumes the value, and there is nothing to report if it
        // fails while the statement is already over.
    }

    /// Read the statement and feed the cursor.
    ///
    /// Consumes the producer so the result set can borrow `conn` for its whole
    /// life while the streaming loop borrows the other fields beside it. A
    /// statement with no values keeps the text protocol it always used; one with
    /// values takes the prepared protocol, which is where `?` exists.
    async fn produce(self) -> Result<(), EngineError> {
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

        if params.is_empty() {
            let mut result = conn
                .query_iter(&sql)
                .await
                .map_err(|error| map_query_error(error, &sql, timeout))?;
            consume(&mut result, &config).await
        } else {
            let mut result = conn
                .exec_iter(&sql, params)
                .await
                .map_err(|error| map_query_error(error, &sql, timeout))?;
            consume(&mut result, &config).await
        }
    }
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
async fn consume<P: mysql_async::prelude::Protocol>(
    result: &mut mysql_async::QueryResult<'_, '_, P>,
    config: &StreamConfig<'_>,
) -> Result<(), EngineError> {
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
            return Ok(());
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
        return Ok(());
    }

    let mut batch = BatchBuilder::new(column_types, binary, config.batch_rows);
    let mut emitted = 0usize;

    while let Some(row) = result
        .next()
        .await
        .map_err(|error| map_query_error(error, config.sql, config.timeout))?
    {
        if let Some(limit) = config.row_limit {
            if emitted + batch.rows >= limit {
                break;
            }
        }
        batch.push_row(&row);
        if batch.rows >= config.batch_rows {
            let full = batch.take()?;
            emitted += full.rows();
            if config.sender.send(Message::Batch(full)).await.is_err() {
                return Ok(());
            }
        }
    }

    if batch.rows > 0 {
        let rest = batch.take()?;
        let _ = config.sender.send(Message::Batch(rest)).await;
    }
    // Read after the stream is exhausted: the OK packet that carries the count is
    // the one that ends it.
    record_affected(config.affected, result);
    Ok(())
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

/// Put `wanted` in force on the server, when it is not there already.
///
/// MySQL's server-side bound is the `max_execution_time` session variable, in
/// milliseconds, and `0` is its own value for "no bound". It is enforced for read-only
/// `SELECT` statements only — the module note says what that leaves uncovered.
async fn apply_max_execution_time(
    conn: &mut Conn,
    current: Option<Duration>,
    wanted: Option<Duration>,
) -> Result<(), EngineError> {
    if current == wanted {
        return Ok(());
    }
    let milliseconds = wanted.map_or(0, |limit| {
        u64::try_from(limit.as_millis()).unwrap_or(u64::MAX)
    });
    let statement = format!("SET SESSION max_execution_time = {milliseconds}");
    conn.query_drop(&statement)
        .await
        .map_err(|error| map_query_error(error, &statement, wanted))
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
        let session = |sql: &str| format!("EXPLAIN {}", strip_terminator(sql));
        assert_eq!(session("SELECT 1"), "EXPLAIN SELECT 1");
        assert_eq!(session("SELECT 1;"), "EXPLAIN SELECT 1");
        // A `;` inside a literal is data and must survive.
        assert_eq!(session("SELECT 'a;b;'"), "EXPLAIN SELECT 'a;b;'");
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
