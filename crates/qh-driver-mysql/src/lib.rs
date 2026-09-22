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
//! ## TLS
//!
//! Only [`TlsMode::Disable`] is implemented. Every other mode is **refused with a
//! clear error** rather than quietly falling back to plaintext — the same rule as
//! the PostgreSQL driver, and for the same reason.

#![forbid(unsafe_code)]

pub mod normalize;

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use mysql_async::prelude::Queryable;
use mysql_async::{Conn, Opts, OptsBuilder};
use qh_core::{ColumnBatch, ColumnMeta, EngineError, FailureKind, Value};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions,
    ObjectPath, ObjectsPage, Session, TlsMode,
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
        }
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        if config.tls != TlsMode::Disable {
            return Err(EngineError::Usage {
                message: format!(
                    "MySQL TLS is not implemented yet, and {:?} would otherwise connect in clear. \
                     Set this connection to disable TLS, or wait for the TLS backend.",
                    config.tls
                ),
            });
        }

        let opts = build_opts(config);
        // Connect once here rather than lazily, so a bad host or password is
        // reported by `connect` — where the UI shows it — instead of surfacing
        // later as a query that mysteriously returns nothing.
        let conn = Conn::new(opts.clone())
            .await
            .map_err(|error| EngineError::Connect {
                message: format!("{}: {error}", config.redacted()),
                kind: classify_connect_error(&error),
            })?;
        let connection_id = Arc::new(AtomicU32::new(conn.id()));
        Ok(Box::new(MysqlSession {
            opts,
            connection_id,
            idle: Some(conn),
        }))
    }
}

fn build_opts(config: &ConnectionConfig) -> Opts {
    let mut builder = OptsBuilder::default()
        .ip_or_hostname(config.host.clone())
        .tcp_port(config.port)
        .user(Some(config.user.clone()))
        // Connecting over a unix socket would ignore host and port entirely, and
        // in a container the socket usually is not there. Forced off so the
        // configured address is the address used.
        .prefer_socket(false);
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
                .map_err(|error| map_query_error(error, sql))?;
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
        let conn = self.connection().await?;
        let (sender, mut receiver) = mpsc::channel(BATCH_BACKLOG);
        let producer = Producer {
            conn,
            sql: sql.to_owned(),
            row_limit: options.row_limit,
            batch_rows: DEFAULT_PRODUCER_BATCH,
            sender,
            connection_id: Arc::clone(&self.connection_id),
        };
        tokio::spawn(producer.run());

        // Wait for the column descriptions. They arrive before any row, so this
        // does not delay the first row — it is what lets `columns()` be valid as
        // soon as `execute` returns, which the trait requires.
        match receiver.recv().await {
            Some(Message::Ready { columns }) => Ok(Box::new(MysqlCursor {
                columns,
                receiver,
                pending: None,
                finished: false,
            })),
            Some(Message::Failed(error)) => Err(error),
            Some(Message::Batch(_)) | None => Err(EngineError::Internal {
                message: "the query task ended before describing its result".to_owned(),
            }),
        }
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
            .map_err(|error| map_query_error(error, &statement));
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
    row_limit: Option<usize>,
    batch_rows: usize,
    sender: mpsc::Sender<Message>,
    connection_id: Arc<AtomicU32>,
}

impl Producer {
    async fn run(mut self) {
        if let Err(error) = self.produce().await {
            // A closed receiver means the cursor was dropped, which is a user
            // cancelling a scroll rather than a failure. Sending is best effort.
            let _ = self.sender.send(Message::Failed(error)).await;
        }
        self.connection_id.store(0, Ordering::SeqCst);
        // Dropping the connection closes the socket; `disconnect` is the tidy
        // path but it consumes the value, and there is nothing to report if it
        // fails while the statement is already over.
    }

    async fn produce(&mut self) -> Result<(), EngineError> {
        // Read before the query, because `query_iter` takes the connection
        // mutably for as long as the result lives.
        self.connection_id.store(self.conn.id(), Ordering::SeqCst);

        let mut result = self
            .conn
            .query_iter(&self.sql)
            .await
            .map_err(|error| map_query_error(error, &self.sql))?;

        let columns: Vec<ColumnMeta> = result
            .columns_ref()
            .iter()
            .map(|column| {
                ColumnMeta::new(column.name_str().to_string(), normalize::type_name(column))
            })
            .collect();
        let column_types: Vec<mysql_async::consts::ColumnType> = result
            .columns_ref()
            .iter()
            .map(mysql_async::Column::column_type)
            .collect();
        // `TEXT` and `BLOB` are the same protocol type, so the character set is
        // read once per column here rather than guessed at per value.
        let binary: Vec<bool> = result
            .columns_ref()
            .iter()
            .map(normalize::is_binary)
            .collect();

        if self.sender.send(Message::Ready { columns }).await.is_err() {
            return Ok(());
        }
        // A statement with no result set — DDL, or an UPDATE — has nothing to
        // stream, and saying so now is better than sending empty batches.
        if column_types.is_empty() {
            return Ok(());
        }

        let mut batch = BatchBuilder::new(column_types, binary, self.batch_rows);
        let mut emitted = 0usize;

        while let Some(row) = result
            .next()
            .await
            .map_err(|error| map_query_error(error, &self.sql))?
        {
            if let Some(limit) = self.row_limit {
                if emitted + batch.rows >= limit {
                    break;
                }
            }
            batch.push_row(&row);
            if batch.rows >= self.batch_rows {
                let full = batch.take()?;
                emitted += full.rows();
                if self.sender.send(Message::Batch(full)).await.is_err() {
                    return Ok(());
                }
            }
        }

        if batch.rows > 0 {
            let rest = batch.take()?;
            let _ = self.sender.send(Message::Batch(rest)).await;
        }
        Ok(())
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
}

#[async_trait]
impl Cursor for MysqlCursor {
    fn columns(&self) -> &[ColumnMeta] {
        &self.columns
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

fn classify_connect_error(error: &mysql_async::Error) -> FailureKind {
    match error {
        // The server answered and refused: wrong password, unknown database,
        // host not allowed. Retrying cannot help.
        mysql_async::Error::Server(_) => FailureKind::Permanent,
        _ => FailureKind::Transient,
    }
}

fn map_query_error(error: mysql_async::Error, sql: &str) -> EngineError {
    match &error {
        mysql_async::Error::Server(server) => EngineError::Query {
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
        },
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

    // `classify_connect_error` and `map_query_error` are not unit-tested here:
    // `mysql_async::Error` cannot be constructed outside the crate. Both are
    // exercised against a live server in `tests/integration.rs`, which produces a
    // real authentication failure, a real syntax error, and a real KILL QUERY.
}
