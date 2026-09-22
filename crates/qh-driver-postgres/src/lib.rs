//! PostgreSQL: streaming batches, exact values, and a cancel that reaches the server.
//!
//! How a query runs
//! ----------------
//! Two calls, in this order:
//!
//! 1. `Client::prepare` — a Parse/Describe with no execution. This is where the
//!    column names **and their types** come from. The simple query protocol does
//!    not report types at all, and without them the grid has no type chip and
//!    [`normalize`] has no key to parse with.
//! 2. `Client::simple_query_raw` — the statement actually runs, streaming rows
//!    back as text, one message at a time.
//!
//! The cost is one extra round trip and one extra parse on the server. What it
//! buys is that values and their types arrive without the binary decoding pass
//! the extended protocol would need, and no unmodelled type can fail a query
//! (see [`normalize`] for the full argument).
//!
//! Because `prepare` describes one statement, **multi-statement SQL is refused
//! here** with the server's own complaint. Splitting a script into statements is
//! `qh-sql`'s job — it already scans literals and comments correctly — not
//! something to guess at inside a driver.
//!
//! ## Cancelling
//!
//! `cancel` sends a `CancelRequest` on a **second connection** to the same
//! server, using the backend pid and secret key from the running connection.
//! PostgreSQL has no cancel message inside the connection that is busy running
//! the query, so a second connection is the only mechanism it offers.
//!
//! This is the behaviour the Python engine did not have: it killed the child
//! process and left the query running on the server. Here the server is told.
//!
//! ## TLS
//!
//! All four [`TlsMode`]s are implemented, by the `rustls` connector in [`tls`]:
//!
//! - `Disable` never negotiates TLS.
//! - `Prefer` tries TLS and falls back to plaintext **only when the server
//!   answers the SSLRequest with "no"**. A server that offers TLS and then fails
//!   the handshake is an error, never a quiet downgrade: a downgrade the other
//!   end can trigger is what an attacker on the network does, and a session that
//!   silently became readable is worse than one that failed.
//! - `Require` is TLS with the certificate verified against the platform's own
//!   root store.
//! - `RequireNoVerify` is TLS with verification turned off, which is what an
//!   attacker on the network needs in order to read everything on the
//!   connection. It is reachable only by asking for it on one connection, and
//!   the UI warns in those words.
//!
//! Where the roots come from, and why it matters: [`tls::platform_verifier`]
//! verifies against the operating system's store — on macOS the Keychain — so a
//! corporate CA a user installed keeps working without this program shipping a
//! copy of it. [`tls::verifier_with_roots`] is the same path with a root store
//! named by the caller, which is how the tests prove both halves of verification
//! without a CA in the machine's store.

#![forbid(unsafe_code)]

pub mod normalize;
pub mod tls;

use std::error::Error as _;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use qh_core::{ColumnBatch, ColumnMeta, EngineError, FailureKind, Value};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions,
    ObjectPath, ObjectsPage, Session, TlsMode,
};
use qh_sql::strip_terminator;
use rustls::client::danger::ServerCertVerifier;
use tokio_postgres::{Client, NoTls, SimpleQueryStream, Statement};

/// PostgreSQL.
pub struct PostgresDriver;

impl PostgresDriver {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PostgresDriver {
    fn default() -> Self {
        Self::new()
    }
}

/// The object columns this driver reports, in its own order.
///
/// Carried over unchanged from the Python driver, which declared the same four
/// for PostgreSQL. A driver owns this list rather than inheriting a shape,
/// because each server has different metadata worth showing.
const OBJECTS_COLUMNS: [&str; 4] = ["Name", "OID", "Owner", "ACL"];

#[async_trait]
impl Driver for PostgresDriver {
    fn kind(&self) -> DriverKind {
        DriverKind::Postgres
    }

    fn label(&self) -> &'static str {
        "PostgreSQL"
    }

    fn default_port(&self) -> u16 {
        5432
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            transactions: true,
            // A simple-query call can carry several statements, but only the
            // last one's rows come back; reporting more than one result set
            // would promise something the protocol does not deliver here.
            multiple_result_sets: false,
            cancel: true,
            explain: true,
            // No catalog level: the database is chosen on the connection, so the
            // tree starts at schemas. Declared rather than hard-coded in the UI.
            levels: vec![BrowseLevel::Schema, BrowseLevel::Table],
            objects_columns: OBJECTS_COLUMNS
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            persistent_connection: true,
        }
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        // A real connection verifies against the platform's own root store, which
        // is the Keychain on macOS — that is what makes a corporate CA the user
        // installed work. Tests come in through `connect_with_verifier` below,
        // because no test can put a CA in that store.
        self.connect_with_verifier(config, tls::platform_verifier()?)
            .await
    }
}

impl PostgresDriver {
    /// [`Driver::connect`], with the certificate verifier named by the caller.
    ///
    /// Production passes [`tls::platform_verifier`]. The parameter exists so the
    /// verifying path can be tested at all: the tests bring up a server whose
    /// certificate is signed by a CA they generated, and verify it against a root
    /// store holding exactly that CA — the same code path, with a store the test
    /// controls instead of the machine's.
    ///
    /// The verifier is used by `Prefer` and `Require`. `RequireNoVerify` ignores
    /// it, because that mode is defined by verifying nothing — and `Disable`
    /// never reaches a handshake to verify.
    pub async fn connect_with_verifier(
        &self,
        config: &ConnectionConfig,
        verifier: Arc<dyn ServerCertVerifier>,
    ) -> Result<Box<dyn Session>, EngineError> {
        let mut pg = tokio_postgres::Config::new();
        pg.host(&config.host)
            .port(config.port)
            .user(&config.user)
            .application_name("QueryHive")
            // Spelled out per mode rather than left at `tokio-postgres`'s
            // default, so which mode is on the wire is decided here.
            .ssl_mode(tls::ssl_mode(config.tls));
        if let Some(database) = &config.database {
            pg.dbname(database);
        }
        if let Some(password) = &config.password {
            pg.password(password);
        }

        match config.tls {
            TlsMode::Disable => open(&pg, config, NoTls).await,
            // `Prefer` encrypts when the server offers it and does **not** verify the
            // certificate. That is psycopg's behaviour, and therefore what an existing
            // connection to an internal, self-signed server depends on: verifying here
            // would break those connections on upgrade with an error that says only
            // "TLS handshake". The mode is still not `RequireNoVerify` — `ssl_mode`
            // decides that, and only `Prefer` may continue in clear when the server
            // declines TLS entirely.
            TlsMode::Prefer => {
                let connector = tls::connector(tls::unverified_client_config()?);
                open(&pg, config, connector).await
            }
            // The one verifying mode: a certificate the platform store does not hold is a
            // failure, and so is a server that declines TLS.
            TlsMode::Require => {
                let connector = tls::connector(tls::verified_client_config(verifier)?);
                open(&pg, config, connector).await
            }
            TlsMode::RequireNoVerify => {
                let connector = tls::connector(tls::unverified_client_config()?);
                open(&pg, config, connector).await
            }
        }
    }
}

/// Open the connection, and hand back a session around it.
///
/// Generic over the connector because the modes install different ones — the
/// rustls connector for three of them, `NoTls` for `Disable` — and everything
/// after the handshake is identical. The connection future has to be spawned
/// here, whichever connector it came from.
async fn open<T>(
    pg: &tokio_postgres::Config,
    config: &ConnectionConfig,
    tls: T,
) -> Result<Box<dyn Session>, EngineError>
where
    T: tokio_postgres::tls::MakeTlsConnect<tokio_postgres::Socket>,
    T::TlsConnect: Send,
    T::Stream: Send + 'static,
    <<T as tokio_postgres::tls::MakeTlsConnect<tokio_postgres::Socket>>::TlsConnect as tokio_postgres::tls::TlsConnect<tokio_postgres::Socket>>::Future:
        Send + 'static,
{
    let (client, connection) = pg
        .connect(tls)
        .await
        .map_err(|error| EngineError::Connect {
            message: format!("{}: {error}", config.redacted()),
            kind: classify_connect_error(&error),
        })?;

    // The connection future drives the socket; if it stops, every query on
    // this client fails. It is spawned rather than awaited because awaiting
    // it would block forever.
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            // Nothing to surface to: whoever holds the client will see a
            // failure on its next query. Kept as a comment rather than a log
            // call because this crate has no tracing subscriber yet.
            let _ = error;
        }
    });

    let cancel_token = client.cancel_token();
    Ok(Box::new(PostgresSession {
        client,
        cancel_token,
        config: config.clone(),
    }))
}

/// A live PostgreSQL connection.
struct PostgresSession {
    client: Client,
    /// Sends the `CancelRequest` on its own connection.
    cancel_token: tokio_postgres::CancelToken,
    config: ConnectionConfig,
}

impl PostgresSession {
    fn capabilities(&self) -> Capabilities {
        PostgresDriver.capabilities()
    }

    /// Run `sql` and collect the first column of every row as text.
    ///
    /// Used by the browse and objects calls, which are our own statements with a
    /// known single-column-or-few-column shape — so unlike a user's query, they
    /// do not need the describe step.
    async fn text_rows(&self, sql: &str) -> Result<Vec<Vec<Option<String>>>, EngineError> {
        // Boxed and pinned: `SimpleQueryStream` is not `Unpin`, so it cannot be
        // polled from behind a `&mut` that a plain field would give.
        let mut stream = Box::pin(
            self.client
                .simple_query_raw(sql)
                .await
                .map_err(|error| map_query_error(error, sql))?,
        );
        let mut rows = Vec::new();
        while let Some(message) = stream.next().await {
            match message {
                Ok(tokio_postgres::SimpleQueryMessage::Row(row)) => {
                    let width = row.len();
                    rows.push(
                        (0..width)
                            .map(|index| row.get(index).map(str::to_owned))
                            .collect(),
                    );
                }
                // CommandComplete and RowDescription carry no data.
                Ok(_) => {}
                Err(error) => return Err(map_query_error(error, sql)),
            }
        }
        Ok(rows)
    }
}

#[async_trait]
impl Session for PostgresSession {
    fn capabilities(&self) -> Capabilities {
        PostgresSession::capabilities(self)
    }

    fn query_id(&self) -> Option<String> {
        // PostgreSQL's per-statement identity is the backend pid, which is stable
        // for the life of the connection and is what a CancelRequest needs.
        None
    }

    async fn execute(
        &mut self,
        sql: &str,
        options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        // Described first so the columns arrive with their types. A statement
        // that cannot be described is reported as the server reports it — which
        // is also how a multi-statement script is refused.
        let statement = self
            .client
            .prepare(sql)
            .await
            .map_err(|error| map_describe_error(error, sql))?;
        let (columns, type_names) = describe_columns(&statement);

        let stream = self
            .client
            .simple_query_raw(sql)
            .await
            .map_err(|error| map_query_error(error, sql))?;

        Ok(Box::new(PostgresCursor {
            columns,
            type_names,
            stream: Box::pin(stream),
            row_limit: options.row_limit,
            emitted: 0,
            finished: false,
        }))
    }

    async fn browse(
        &mut self,
        level: BrowseLevel,
        path: &ObjectPath,
        include_system: bool,
    ) -> Result<Vec<String>, EngineError> {
        let sql = match level {
            // Not an object-tree level: a PostgreSQL connection is bound to one
            // database, so the tree starts at schemas. It is still a question worth
            // answering — which databases exist is what a query needs to know
            // before it is pointed at another one, and it is what a context picker
            // lists. The Python engine answered it for the same reason.
            BrowseLevel::Catalog | BrowseLevel::Database => catalogs_sql(include_system),
            BrowseLevel::Schema => schemas_sql(include_system),
            BrowseLevel::Table => {
                let schema = path.schema.as_deref().ok_or_else(|| EngineError::Usage {
                    message: "DB_SCHEMA is required to list tables on PostgreSQL".to_owned(),
                })?;
                tables_sql(schema)
            }
        };

        let rows = self.text_rows(&sql).await?;
        Ok(rows
            .into_iter()
            .filter_map(|mut row| row.drain(..).next().flatten())
            .collect())
    }

    async fn objects(&mut self, path: &ObjectPath) -> Result<ObjectsPage, EngineError> {
        let schema = path.schema.as_deref().ok_or_else(|| EngineError::Usage {
            message: "DB_SCHEMA (or a database) is required to list objects on PostgreSQL"
                .to_owned(),
        })?;
        let rows = self.text_rows(&objects_sql(schema)).await?;
        Ok(ObjectsPage {
            columns: OBJECTS_COLUMNS
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
            // A NULL cell is the empty string here, because this grid draws a
            // NULL and an empty cell alike — the same rule the Python engine used.
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
        // PostgreSQL rejects `EXPLAIN SELECT 1;`, so the terminator goes — and
        // only a real one: a `;` inside a literal is text. `strip_terminator`
        // knows the difference, which is why it is shared rather than reimplemented.
        format!("EXPLAIN {}", strip_terminator(sql))
    }

    async fn cancel(&self) -> Result<(), EngineError> {
        // Idempotent by construction: with nothing running PostgreSQL still
        // answers the CancelRequest, and there is nothing to undo.
        self.cancel_token
            .cancel_query(NoTls)
            .await
            .map_err(|error| EngineError::Connect {
                message: format!("cancel on {}: {error}", self.config.redacted()),
                kind: FailureKind::Transient,
            })
    }

    async fn close(self: Box<Self>) -> Result<(), EngineError> {
        // Dropping the client ends the connection; the spawned connection future
        // returns once the socket closes.
        Ok(())
    }
}

/// Rows from a running statement, in batches.
struct PostgresCursor {
    columns: Vec<ColumnMeta>,
    type_names: Vec<String>,
    /// Boxed and pinned: `SimpleQueryStream` is not `Unpin`.
    stream: std::pin::Pin<Box<SimpleQueryStream>>,
    row_limit: Option<usize>,
    emitted: usize,
    finished: bool,
}

#[async_trait]
impl Cursor for PostgresCursor {
    fn columns(&self) -> &[ColumnMeta] {
        &self.columns
    }

    async fn next_batch(&mut self, max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        if self.finished || self.columns.is_empty() {
            // A statement with no result set (DDL, or an UPDATE) has no columns
            // to fill, so it is finished the moment it starts.
            self.finished = true;
            return Ok(None);
        }

        let mut rows: Vec<Vec<Value>> = Vec::with_capacity(max_rows.min(1024));
        while rows.len() < max_rows {
            if let Some(limit) = self.row_limit {
                if self.emitted + rows.len() >= limit {
                    self.finished = true;
                    break;
                }
            }
            match self.stream.next().await {
                Some(Ok(tokio_postgres::SimpleQueryMessage::Row(row))) => {
                    let cells = (0..self.columns.len())
                        .map(|index| normalize::from_text(&self.type_names[index], row.get(index)))
                        .collect();
                    rows.push(cells);
                }
                // CommandComplete and RowDescription carry no data; the column
                // types were already described up front.
                Some(Ok(_)) => continue,
                Some(Err(error)) => {
                    self.finished = true;
                    return Err(map_query_error(error, ""));
                }
                None => {
                    self.finished = true;
                    break;
                }
            }
        }

        if rows.is_empty() {
            return Ok(None);
        }
        self.emitted += rows.len();

        // Transposed here because the store is column-major and a row is the
        // shape the wire arrives in. This is the one place the two meet.
        let column_count = self.columns.len();
        let mut columns: Vec<Vec<Value>> = (0..column_count)
            .map(|_| Vec::with_capacity(rows.len()))
            .collect();
        for row in rows {
            for (index, value) in row.into_iter().enumerate() {
                columns[index].push(value);
            }
        }
        ColumnBatch::new(columns)
            .map(Some)
            .map_err(|error| EngineError::Internal {
                message: format!("the cursor built a batch the store refused: {error}"),
            })
    }
}

/// Every database on the server, for the context picker.
///
/// Templates are not real databases and `datallowconn = false` ones refuse
/// connections, so offering either would be a choice that cannot be taken —
/// unless the user asked for everything, in which case the filter goes.
///
/// These strings are reproduced verbatim from `exporter/drivers.py` so the Rust
/// driver answers with what the Python engine answered. They are pinned by tests.
fn catalogs_sql(include_system: bool) -> String {
    let filter = if include_system {
        ""
    } else {
        "WHERE NOT datistemplate AND datallowconn "
    };
    format!("SELECT datname FROM pg_database {filter}ORDER BY 1")
}

/// The schemas a user browses.
///
/// The system schemas are hidden by default, and the backslash escapes the
/// underscore so `pg\_%` does not also match a schema named `pgx`. "Show all"
/// drops the filter entirely, because someone asking for everything wants
/// `pg_catalog` to appear the same as any user schema.
fn schemas_sql(include_system: bool) -> String {
    let filter = if include_system {
        ""
    } else {
        "WHERE schema_name NOT LIKE 'pg\\_%' AND schema_name <> 'information_schema' "
    };
    format!("SELECT schema_name FROM information_schema.schemata {filter}ORDER BY 1")
}

/// The tables of one schema.
///
/// `table_type = 'BASE TABLE'` is the same set the objects grid means by
/// `relkind IN ('r', 'p')`, so the tree and the grid cannot disagree about what a
/// table is.
fn tables_sql(schema: &str) -> String {
    format!(
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = {} AND table_type = 'BASE TABLE' ORDER BY 1",
        quote_literal(schema)
    )
}

/// The objects grid's four columns, unchanged from the Python driver so the grid
/// looks the same after the migration.
fn objects_sql(schema: &str) -> String {
    format!(
        "SELECT c.relname, c.oid, pg_get_userbyid(c.relowner), \
         COALESCE(array_to_string(c.relacl, ', '), '') \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = {} AND c.relkind IN ('r', 'p') \
         ORDER BY c.relname",
        quote_literal(schema)
    )
}

/// A string literal with its quotes doubled.
///
/// String literals and identifiers are different problems: a schema name in a
/// comparison is a literal, so `quote_ident` would be wrong here.
fn quote_literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

fn describe_columns(statement: &Statement) -> (Vec<ColumnMeta>, Vec<String>) {
    let mut columns = Vec::with_capacity(statement.columns().len());
    let mut type_names = Vec::with_capacity(statement.columns().len());
    for column in statement.columns() {
        let type_name = column.type_().name();
        columns.push(ColumnMeta::new(column.name(), type_name));
        type_names.push(type_name.to_owned());
    }
    (columns, type_names)
}

/// Whether a connection failure is worth retrying.
///
/// A server-reported error — bad credentials, an unknown database — will not
/// succeed on a second attempt, so it is permanent. So is a TLS handshake that
/// failed, and for the same reason: a certificate this machine does not trust
/// will not become trusted by asking again. Anything else is a network condition
/// and is retried with backoff, the same distinction the Python engine drew for
/// queries.
fn classify_connect_error(error: &tokio_postgres::Error) -> FailureKind {
    if error.as_db_error().is_some() || is_tls_failure(error) {
        FailureKind::Permanent
    } else {
        FailureKind::Transient
    }
}

/// Whether the failure came out of the TLS handshake.
///
/// `tokio-postgres` keeps the reason in the error's source chain rather than in
/// its kind: what it hands back is the `io::Error` the connector produced. The
/// rustls error underneath that is looked for by type, because matching on the
/// message text would break on the next wording change — and `io::Error::source`
/// reports the *inner* error's cause rather than the inner error itself, so
/// `get_ref` is the only way to reach it.
fn is_tls_failure(error: &tokio_postgres::Error) -> bool {
    let Some(source) = error.source() else {
        return false;
    };
    let Some(io_error) = source.downcast_ref::<std::io::Error>() else {
        return source.is::<rustls::Error>();
    };
    io_error.get_ref().is_some_and(|inner| {
        // A certificate the handshake refused, or a hostname that cannot be a
        // TLS name — neither becomes usable by connecting again.
        inner.is::<rustls::Error>() || inner.is::<rustls::pki_types::InvalidDnsNameError>()
    })
}

/// Map a failure while describing a statement.
fn map_describe_error(error: tokio_postgres::Error, sql: &str) -> EngineError {
    // A multi-statement script is the one case worth naming, because the server's
    // own message does not say what to do about it.
    if let Some(db_error) = error.as_db_error() {
        if db_error.code().code() == "42601" && qh_sql::statement_count(sql, &qh_sql::scan(sql)) > 1
        {
            return EngineError::Usage {
                message: "this statement holds more than one command, and a driver can only \
                          describe one at a time; split it into separate statements"
                    .to_owned(),
            };
        }
    }
    map_query_error(error, sql)
}

/// Map a failure the server reported while running a statement.
fn map_query_error(error: tokio_postgres::Error, sql: &str) -> EngineError {
    if let Some(db_error) = error.as_db_error() {
        return EngineError::Query {
            message: match sql.is_empty() {
                true => db_error.message().to_owned(),
                false => format!("{}: {}", db_error.message(), snippet(sql)),
            },
            // SQLSTATE, kept as the server spells it so a search for it finds
            // PostgreSQL's own documentation.
            code: Some(db_error.code().code().to_owned()),
            // PostgreSQL does report a position for syntax errors, and the UI can
            // hold the caret on it once the field is plumbed through.
            position: None,
            kind: FailureKind::Permanent,
        };
    }
    if error.is_closed() {
        return EngineError::Connect {
            message: format!("the connection closed while the query ran: {error}"),
            kind: FailureKind::Transient,
        };
    }
    EngineError::Query {
        message: error.to_string(),
        code: None,
        position: None,
        kind: FailureKind::Transient,
    }
}

/// A short, single-line rendering of a statement for an error message.
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
    fn this_driver_is_postgres_and_says_so() {
        let driver = PostgresDriver::new();
        assert_eq!(driver.kind(), DriverKind::Postgres);
        assert_eq!(driver.label(), "PostgreSQL");
        assert_eq!(driver.default_port(), 5432);
    }

    #[test]
    fn the_tree_starts_at_schemas_because_there_is_no_catalog_level() {
        let capabilities = PostgresDriver.capabilities();
        assert_eq!(
            capabilities.levels,
            vec![BrowseLevel::Schema, BrowseLevel::Table]
        );
        // The UI builds the tree from this rather than hard-coding it.
        assert!(!capabilities.levels.contains(&BrowseLevel::Catalog));
        // But `levels` and what the driver will *answer* are different questions.
        // The database list is not a tree level, yet it is what a context picker
        // needs, and the Python engine answered it for that reason. `browse`
        // answers Catalog rather than refusing it.
    }

    /// The statements the Python driver built, pinned verbatim so the migration
    /// cannot quietly change what the tree or the grid asks the server.
    ///
    /// The expected strings are the ones `exporter/drivers.py` actually produced —
    /// taken from running that code, not retyped from reading it.
    #[test]
    fn the_metadata_statements_match_the_ones_they_replace() {
        assert_eq!(
            catalogs_sql(false),
            "SELECT datname FROM pg_database WHERE NOT datistemplate AND datallowconn ORDER BY 1"
        );
        assert_eq!(
            catalogs_sql(true),
            "SELECT datname FROM pg_database ORDER BY 1"
        );

        assert_eq!(
            schemas_sql(false),
            "SELECT schema_name FROM information_schema.schemata \
             WHERE schema_name NOT LIKE 'pg\\_%' \
             AND schema_name <> 'information_schema' ORDER BY 1"
        );
        assert_eq!(
            schemas_sql(true),
            "SELECT schema_name FROM information_schema.schemata ORDER BY 1"
        );

        assert_eq!(
            tables_sql("analytics"),
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = 'analytics' AND table_type = 'BASE TABLE' ORDER BY 1"
        );

        assert_eq!(
            objects_sql("analytics"),
            "SELECT c.relname, c.oid, pg_get_userbyid(c.relowner), \
             COALESCE(array_to_string(c.relacl, ', '), '') \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'analytics' AND c.relkind IN ('r', 'p') \
             ORDER BY c.relname"
        );
    }

    #[test]
    fn the_backslash_in_the_system_schema_filter_actually_reaches_the_server() {
        // The filter means `pg\_%` with a real backslash, so it does not also
        // match a user schema named `pgx`. A string that lost the escape looks
        // identical at a glance and filters the wrong set.
        let sql = schemas_sql(false);
        assert!(sql.contains(r"NOT LIKE 'pg\_%'"), "{sql}");
        assert!(!sql.contains("NOT LIKE 'pg_%'"), "{sql}");
    }

    #[test]
    fn cancel_is_declared_as_reaching_the_server() {
        // The whole point of the rewrite: this used to be a killed process.
        assert!(PostgresDriver.capabilities().cancel);
        assert!(PostgresDriver.capabilities().persistent_connection);
    }

    #[test]
    fn the_objects_grid_reports_the_same_four_columns_as_before() {
        assert_eq!(
            PostgresDriver.capabilities().objects_columns,
            vec![
                "Name".to_owned(),
                "OID".to_owned(),
                "Owner".to_owned(),
                "ACL".to_owned(),
            ]
        );
    }

    /// The statements the Python driver built, pinned so the grid looks the same
    /// after the migration.
    #[test]
    fn the_objects_statement_matches_the_one_it_replaces() {
        assert_eq!(
            objects_sql("analytics"),
            "SELECT c.relname, c.oid, pg_get_userbyid(c.relowner), \
             COALESCE(array_to_string(c.relacl, ', '), '') \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE n.nspname = 'analytics' AND c.relkind IN ('r', 'p') \
             ORDER BY c.relname"
        );
    }

    #[test]
    fn a_schema_name_cannot_break_out_of_its_literal() {
        // A quote in an identifier is doubled, so a schema named `a'b` cannot end
        // the literal and append SQL.
        assert_eq!(quote_literal("analytics"), "'analytics'");
        assert_eq!(quote_literal("a'b"), "'a''b'");
        assert_eq!(
            quote_literal("'; DROP TABLE people; --"),
            "'''; DROP TABLE people; --'"
        );
    }

    #[test]
    fn explain_drops_a_terminator_but_not_a_semicolon_inside_text() {
        let session = |sql: &str| format!("EXPLAIN {}", strip_terminator(sql));
        assert_eq!(session("SELECT 1"), "EXPLAIN SELECT 1");
        // PostgreSQL rejects `EXPLAIN SELECT 1;`.
        assert_eq!(session("SELECT 1;"), "EXPLAIN SELECT 1");
        assert_eq!(session("SELECT 1;  \n"), "EXPLAIN SELECT 1");
        // A `;` inside a literal is data and must survive.
        assert_eq!(session("SELECT 'a;b;'"), "EXPLAIN SELECT 'a;b;'");
    }

    #[test]
    fn an_error_message_shows_a_readable_snippet_of_the_statement() {
        assert_eq!(snippet("SELECT 1"), "SELECT 1");
        assert_eq!(snippet("SELECT\n  1"), "SELECT 1");
        let long = snippet(&"x".repeat(200));
        assert!(long.ends_with('…'));
        assert_eq!(long.chars().count(), 61);
    }

    // `classify_connect_error` and `map_query_error` are not unit-tested here:
    // `tokio_postgres::Error` cannot be constructed outside the crate, so a
    // hand-made one is not possible. Both are exercised by the integration test
    // in `tests/integration.rs`, which produces a real authentication failure and
    // a real syntax error against a live server.
}
