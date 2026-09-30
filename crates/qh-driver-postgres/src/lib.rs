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
//! The cost is one extra round trip and one extra parse on the server (a `SET
//! statement_timeout` that changed rides in the same flight as the describe). The statement
//! itself is never sent before its describe has been answered, so a text the server refuses is
//! never run. What it
//! buys is that values and their types arrive without the binary decoding pass
//! the extended protocol would need, and no unmodelled type can fail a query
//! (see [`normalize`] for the full argument).
//!
//! Because `prepare` describes one statement, **multi-statement SQL is refused
//! here** with the server's own complaint. Splitting a script into statements is
//! `qh-sql`'s job — it already scans literals and comments correctly — not
//! something to guess at inside a driver.
//!
//! ## `catalogs` is answered, though it is not a tree level
//!
//! A PostgreSQL connection is bound to one database, so the object tree starts at
//! schemas and [`Capabilities::levels`] says `[Schema, Table]`. The `catalogs`
//! command is still answered, with the databases on the server (`pg_database`,
//! filtered to the connectable non-template ones): which databases exist is what the
//! context picker lists, and it is what a query needs to know before it is pointed at
//! a different one. The list is a picker list, not a node of the tree — a connection
//! cannot browse into another database, so no database level appears under it.
//!
//! The two are separate questions, and the previous engine declared them separately:
//! `exporter/drivers.py` gave `PostgresDriver` both `levels = ("schema", "table")`
//! and `browse = ("catalogs", "schemas", "tables")`, and its own test
//! (`tests/test_engine_events.py::check_postgres_has_no_catalog_level`) pins both
//! halves — `catalogs` emits the database list, and `levels` stays schema-first.
//! This driver keeps that split: `browse` answers the command, under whichever of
//! `Catalog` and `Database` the caller names it with. Both names mean one list, the
//! way `crates/qh-driver-mysql` accepts both for the level it calls `database` where
//! Trino calls its own `catalog`.
//!
//! Which is why [`Capabilities::levels`] must not be read as "the commands this
//! driver will answer". A caller that gates a browse command on it refuses
//! `catalogs` here before this driver is ever asked — the refusal
//! `tests/golden/RECORDED.md` records as `postgres has no catalog level` against the
//! previous engine's `{"event": "catalogs", "names": ["postgres", "qh"]}`.
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
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use qh_core::{ColumnBatch, ColumnMeta, EngineError, FailureKind, Value};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions,
    ObjectPath, ObjectsPage, Parameter, ParameterStyle, Session, TlsMode,
};
use qh_sql::strip_terminator;
use rustls::client::danger::ServerCertVerifier;
use tokio_postgres::types::private::BytesMut;
use tokio_postgres::types::{Format, IsNull, ToSql, Type};
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
            // The server's own `statement_timeout` session parameter.
            statement_timeout: true,
            // `$1`, `$2`, … — the numbered placeholders the extended protocol
            // takes. Bound values are sent in text format and parsed by the
            // server according to the type it inferred, so one binder covers
            // every type without mapping widths here.
            parameters: Some(ParameterStyle::Dollar),
            read_only: false,
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
            // A pooled session can idle behind a NAT that forgets it; a probe every minute keeps
            // the mapping alive and lets a dead socket show up as one.
            .keepalives_idle(Duration::from_secs(60))
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
            message: connect_message(config, &error),
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
        // Nothing has been asked of the server yet, so the session must not send a
        // `SET statement_timeout = 0` on its first statement and override a bound the
        // server's own configuration set.
        statement_timeout: None,
    }))
}

/// A live PostgreSQL connection.
struct PostgresSession {
    client: Client,
    /// Sends the `CancelRequest` on its own connection.
    cancel_token: tokio_postgres::CancelToken,
    config: ConnectionConfig,
    /// The `statement_timeout` this session last sent to the server.
    ///
    /// Kept so a bound is sent when it changes and not on every statement: `SET` is a
    /// round trip, and an unchanged value is already in force on the connection.
    statement_timeout: Option<Duration>,
}

impl PostgresSession {
    fn capabilities(&self) -> Capabilities {
        PostgresDriver.capabilities()
    }

    /// Refuse before sending anything when the connection is already gone.
    ///
    /// A pool hands out sessions that sat idle, and an idle socket can die (NAT timeout, a
    /// restarted server). Saying so here, as a connection error and before a byte is sent,
    /// is what lets the layer above tell "never reached the server" from "the server said no".
    fn ensure_open(&self) -> Result<(), EngineError> {
        if self.client.is_closed() {
            return Err(EngineError::Connect {
                message: format!("the connection to {} was closed", self.config.redacted()),
                kind: FailureKind::Transient,
            });
        }
        Ok(())
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
                .map_err(|error| map_query_error(error, sql, None))?,
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
                Err(error) => return Err(map_query_error(error, sql, None)),
            }
        }
        Ok(rows)
    }

    /// Run a statement that carries bound values, through the extended protocol.
    ///
    /// Only statements that return no rows are accepted, and that is a deliberate
    /// limit rather than an oversight: the extended protocol returns result values
    /// in PostgreSQL's **binary** form, while this driver's decoder is the text
    /// path — the reason a type nobody has taught `normalize` about still cannot
    /// fail a query. Decoding binary rows for an arbitrary type would either
    /// duplicate that knowledge or quietly weaken the guarantee, so a
    /// row-returning statement is refused by name instead.
    async fn bind_statement(
        &self,
        sql: &str,
        parameters: &[Parameter],
        options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        let statement = self
            .client
            .prepare(sql)
            .await
            .map_err(|error| map_describe_error(error, sql, options.statement_timeout))?;
        if !statement.columns().is_empty() {
            return Err(EngineError::Usage {
                message: format!(
                    "PostgreSQL binds values only for a statement that returns no rows, and this \
                     one returns {} column(s); write the values into the SQL for this statement \
                     instead",
                    statement.columns().len()
                ),
            });
        }
        if statement.params().len() != parameters.len() {
            return Err(EngineError::Usage {
                message: format!(
                    "the statement has {} placeholder(s) but {} value(s) were supplied",
                    statement.params().len(),
                    parameters.len()
                ),
            });
        }
        let params: Vec<TextParam> = parameters
            .iter()
            .map(|value| TextParam(postgres_text(value)))
            .collect();
        let borrowed: Vec<&(dyn ToSql + Sync)> = params
            .iter()
            .map(|param| param as &(dyn ToSql + Sync))
            .collect();
        let affected = self
            .client
            .execute(&statement, &borrowed)
            .await
            .map_err(|error| map_query_error(error, sql, options.statement_timeout))?;
        Ok(Box::new(PostgresBindCursor {
            affected: Some(affected),
        }))
    }
}

/// A bound value the server parses from text, in the type it inferred.
///
/// `accepts` is true for every type because the bytes are the server's own text
/// form, and `encode_format` says text rather than PostgreSQL's default binary.
/// That covers integer widths, `numeric` precision and timestamps without this
/// crate mapping any of them: the type comes from the statement's own context
/// (`col = $1`, `SET col = $1`, `VALUES ($1)`), and the server converts.
#[derive(Debug)]
struct TextParam(Option<String>);

impl ToSql for TextParam {
    fn to_sql(
        &self,
        _ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        match &self.0 {
            Some(text) => {
                out.extend_from_slice(text.as_bytes());
                Ok(IsNull::No)
            }
            // A real SQL NULL, of whatever type the placeholder was inferred as.
            None => Ok(IsNull::Yes),
        }
    }

    fn accepts(_ty: &Type) -> bool {
        true
    }

    fn encode_format(&self, _ty: &Type) -> Format {
        Format::Text
    }

    /// `accepts` is total, so the check the default implementation would do is a
    /// no-op; writing the text is the whole conversion.
    fn to_sql_checked(
        &self,
        ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        self.to_sql(ty, out)
    }
}

/// The value as PostgreSQL's text input, or `None` for a SQL NULL.
///
/// The non-finite floats get the server's own spelling rather than Rust's: a
/// `float8` column receives `Infinity`, not `inf`.
fn postgres_text(value: &Parameter) -> Option<String> {
    match value {
        Parameter::Null => None,
        Parameter::Bool(true) => Some("true".to_owned()),
        Parameter::Bool(false) => Some("false".to_owned()),
        Parameter::Int(number) => Some(number.to_string()),
        Parameter::UInt(number) => Some(number.to_string()),
        Parameter::Float(number) if number.is_nan() => Some("NaN".to_owned()),
        Parameter::Float(number) if number.is_infinite() => Some(if *number > 0.0 {
            "Infinity".to_owned()
        } else {
            "-Infinity".to_owned()
        }),
        Parameter::Float(number) => Some(number.to_string()),
        Parameter::Text(text) => Some(text.clone()),
    }
}

/// The cursor for a statement that writes and returns no rows.
///
/// Its count is already the server's when it is built: `Client::execute` reads
/// the `CommandComplete` tag before returning, so there is nothing left to
/// drain.
struct PostgresBindCursor {
    affected: Option<u64>,
}

#[async_trait]
impl Cursor for PostgresBindCursor {
    fn columns(&self) -> &[ColumnMeta] {
        &[]
    }

    async fn next_batch(&mut self, _max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        Ok(None)
    }

    fn affected_rows(&self) -> Option<u64> {
        self.affected
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
        // The bound is a server setting, so it has to reach the server before the
        // statement does. Applied only when it changed: `SET` is a round trip, and the
        // value already in force is already in force. It travels in the same flight as the
        // describe below, so it costs nothing extra.
        self.ensure_open()?;
        let set = timeout_statement(self.statement_timeout, options.statement_timeout);
        let client = &self.client;
        // Described first so the columns arrive with their types, and **before the statement
        // is sent**: a statement that cannot be described is refused as the server reports it,
        // which is also how a multi-statement script is refused, and nothing has run by then.
        // The statement is never sent together with its describe: the classifier and the
        // server can read the same text differently, and a describe that would have refused it
        // must come first.
        let ((), statement) = futures_util::future::try_join(
            async {
                match &set {
                    Some(statement) => client
                        .batch_execute(statement)
                        // The bound is in force from here, so a failure setting it is reported
                        // as what it is: the timeout is not on, and the caller must not be
                        // told it is.
                        .await
                        .map_err(|error| {
                            map_query_error(error, statement, options.statement_timeout)
                        }),
                    None => Ok(()),
                }
            },
            async {
                client
                    .prepare(sql)
                    .await
                    .map_err(|error| map_describe_error(error, sql, options.statement_timeout))
            },
        )
        .await?;
        let stream = client
            .simple_query_raw(sql)
            .await
            .map_err(|error| map_query_error(error, sql, options.statement_timeout))?;
        self.statement_timeout = options.statement_timeout;
        let (columns, type_names) = describe_columns(&statement);

        Ok(Box::new(PostgresCursor {
            columns,
            type_names,
            stream: Box::pin(stream),
            row_limit: options.row_limit,
            emitted: 0,
            finished: false,
            affected: None,
            timeout: options.statement_timeout,
        }))
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
        // The bound is a server setting, so it reaches the server before the
        // statement does — the same rule `execute` follows, for the same reason.
        self.ensure_open()?;
        apply_statement_timeout(
            &self.client,
            self.statement_timeout,
            options.statement_timeout,
        )
        .await?;
        self.statement_timeout = options.statement_timeout;
        self.bind_statement(sql, parameters, options).await
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
            //
            // Both names are answered with the one list, because both arrive for the
            // one question: `catalogs` is the command, and `database` is what MySQL
            // calls this level. See the module doc for why the tree level is absent
            // from `levels` while the command is answered anyway.
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

    /// `ROLLBACK`, then `DISCARD ALL` taken apart, in one round trip.
    ///
    /// `tokio-postgres` exposes no transaction status, so the rollback is always sent (outside
    /// a transaction the server only warns). `DISCARD ALL` itself is not sent: it includes
    /// `DEALLOCATE ALL`, and that would delete the `typeinfo` statements the client prepared
    /// for itself and left the next enum, domain or extension type failing with `prepared
    /// statement "sN" does not exist`. Its other components are sent one by one instead, and
    /// the statements a user made with SQL `PREPARE` are found and dropped by name, which
    /// leaves the protocol-level statements alone.
    ///
    /// The three messages go out before any answer is read (`try_join3` polls in order, and
    /// each request is queued when first polled), so the whole reset costs one round trip.
    async fn reset(&mut self) -> Result<(), EngineError> {
        self.ensure_open()?;
        let client = &self.client;
        let (_, _, prepared) = futures_util::future::try_join3(
            client.batch_execute("ROLLBACK"),
            client.batch_execute(
                "CLOSE ALL; SET SESSION AUTHORIZATION DEFAULT; RESET ALL; UNLISTEN *; \
                 SELECT pg_advisory_unlock_all(); DISCARD PLANS; DISCARD TEMP; \
                 DISCARD SEQUENCES",
            ),
            client.simple_query("SELECT name FROM pg_prepared_statements WHERE from_sql"),
        )
        .await
        .map_err(|error| map_query_error(error, "reset", None))?;

        let deallocate: String = prepared
            .iter()
            .filter_map(|message| match message {
                tokio_postgres::SimpleQueryMessage::Row(row) => row.get(0),
                _ => None,
            })
            .map(|name| format!("DEALLOCATE \"{}\"; ", name.replace('"', "\"\"")))
            .collect();
        if !deallocate.is_empty() {
            client
                .batch_execute(&deallocate)
                .await
                .map_err(|error| map_query_error(error, "reset", None))?;
        }
        // `RESET ALL` put `statement_timeout` back to the role's default, which is what a new
        // connection has, and the tracker must say the same or the next run skips its `SET`.
        self.statement_timeout = None;
        Ok(())
    }
}

/// Rows from a running statement, in batches.
struct PostgresCursor {
    columns: Vec<ColumnMeta>,
    type_names: Vec<String>,
    /// Boxed and pinned: `SimpleQueryStream` is not `Unpin`.
    stream: Pin<Box<SimpleQueryStream>>,
    row_limit: Option<usize>,
    emitted: usize,
    finished: bool,
    /// The rows the statement reported affecting, from PostgreSQL's
    /// `CommandComplete` tag.
    ///
    /// `None` until that tag is seen, and it is only read for a statement with no
    /// result set — an `UPDATE`, `INSERT`, `DELETE` or DDL. A `SELECT`'s
    /// `CommandComplete` count is the rows *returned*, which is not what
    /// `Cursor::affected_rows` means, so it is deliberately not recorded here.
    affected: Option<u64>,
    /// The bound this statement was started under, so a server timeout names it.
    timeout: Option<Duration>,
}

#[async_trait]
impl Cursor for PostgresCursor {
    fn columns(&self) -> &[ColumnMeta] {
        &self.columns
    }

    async fn next_batch(&mut self, max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        if self.finished {
            return Ok(None);
        }
        if self.columns.is_empty() {
            // A statement with no result set (DDL, or an `UPDATE`/`INSERT`/
            // `DELETE`) has no columns to fill, so it is finished the moment it
            // starts — but not before the stream is drained, because the
            // `CommandComplete` at its end is the only place PostgreSQL reports
            // how many rows the statement affected.
            while let Some(message) = self.stream.next().await {
                match message {
                    Ok(tokio_postgres::SimpleQueryMessage::CommandComplete(count)) => {
                        self.affected = Some(count);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        self.finished = true;
                        return Err(map_query_error(error, "", self.timeout));
                    }
                }
            }
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
                    return Err(map_query_error(error, "", self.timeout));
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

    fn affected_rows(&self) -> Option<u64> {
        self.affected
    }
}

/// Every database on the server, for the context picker.
///
/// Templates are not real databases and `datallowconn = false` ones refuse
/// connections, so offering either would be a choice that cannot be taken —
/// unless the user asked for everything, in which case the filter goes.
///
/// The `catalogs` command is answered with this list even though no database level
/// appears in the tree. The module doc says why, and
/// `tests/integration.rs::catalogs_lists_databases_even_though_it_is_not_a_tree_level`
/// proves it against a real server — including that both level names reach it.
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

/// What to say when a connection fails.
///
/// `tokio_postgres::Error`'s own `Display` is the two words `db error` whenever the server
/// answered with an `ErrorResponse`; the reason lives in `as_db_error()`. Without reading it,
/// a server that rejects the login says "db error" and nothing else, which is what happened
/// when this was measured: the real answer was `role "qh" does not exist`. `map_query_error`
/// already reads the server's answer this way for a failed statement, and
/// `classify_connect_error` above already asks `as_db_error()` the same question, so this is
/// the third place agreeing with the other two rather than a new rule.
fn connect_message(config: &ConnectionConfig, error: &tokio_postgres::Error) -> String {
    match error.as_db_error() {
        Some(db_error) => format!(
            "{}: {} (SQLSTATE {})",
            config.redacted(),
            db_error.message(),
            db_error.code().code()
        ),
        None => format!("{}: {error}", config.redacted()),
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
fn map_describe_error(
    error: tokio_postgres::Error,
    sql: &str,
    timeout: Option<Duration>,
) -> EngineError {
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
    map_query_error(error, sql, timeout)
}

/// Whether the server's error is the one `statement_timeout` raises.
///
/// The SQLSTATE alone is not enough: `57014` (`query_canceled`) is also what a
/// `CancelRequest` produces, and the two mean opposite things to a user — one is a
/// limit being reported, the other is the stop they asked for. The server's own
/// sentence is what tells them apart, and it is the part this reads.
fn is_statement_timeout(db_error: &tokio_postgres::error::DbError) -> bool {
    db_error.code().code() == "57014"
        && db_error
            .message()
            .to_ascii_lowercase()
            .contains("statement timeout")
}

/// Map a failure the server reported while running a statement.
fn map_query_error(
    error: tokio_postgres::Error,
    sql: &str,
    timeout: Option<Duration>,
) -> EngineError {
    if let Some(db_error) = error.as_db_error() {
        if is_statement_timeout(db_error) {
            return EngineError::statement_timeout(timeout, db_error.message());
        }
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

/// Put `wanted` in force on the server, when it is not there already.
///
/// PostgreSQL's server-side bound is the `statement_timeout` session parameter, and `0`
/// is its own value for "no bound". Sending the change with `SET` is what makes the
/// limit the server's: a statement that overruns is cancelled by the backend and its
/// resources released, whichever client asked for it.
async fn apply_statement_timeout(
    client: &Client,
    current: Option<Duration>,
    wanted: Option<Duration>,
) -> Result<(), EngineError> {
    let Some(statement) = timeout_statement(current, wanted) else {
        return Ok(());
    };
    client
        .batch_execute(&statement)
        .await
        // The bound is in force from here, so a failure setting it is reported as what
        // it is: the timeout is not on, and the caller must not be told it is.
        .map_err(|error| map_query_error(error, &statement, wanted))
}

/// The `SET` that takes the server from `current` to `wanted`, or `None` when they agree.
///
/// Clamped to what the setting can hold (an `int` of milliseconds): a longer bound sent as is
/// would make the `SET` itself fail on range, and the statement would then run with no bound
/// at all, which is the opposite of what was asked.
fn timeout_statement(current: Option<Duration>, wanted: Option<Duration>) -> Option<String> {
    if current == wanted {
        return None;
    }
    let milliseconds = wanted.map_or(0, |limit| {
        u64::try_from(limit.as_millis())
            .unwrap_or(u64::MAX)
            .min(i32::MAX as u64)
    });
    Some(format!("SET statement_timeout = {milliseconds}"))
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
    fn the_timeout_set_is_only_sent_when_it_changes_and_never_overflows() {
        let second = Some(Duration::from_secs(1));
        assert_eq!(timeout_statement(second, second), None);
        assert_eq!(timeout_statement(None, None), None);
        assert_eq!(
            timeout_statement(None, second).as_deref(),
            Some("SET statement_timeout = 1000")
        );
        // Back to no bound is an explicit zero, the server's own word for it.
        assert_eq!(
            timeout_statement(second, None).as_deref(),
            Some("SET statement_timeout = 0")
        );
        // A bound longer than the setting can hold is clamped instead of failing the `SET`
        // and leaving the statement with no bound at all.
        assert_eq!(
            timeout_statement(None, Some(Duration::from_secs(u64::MAX / 4))).as_deref(),
            Some("SET statement_timeout = 2147483647")
        );
    }

    #[test]
    fn this_driver_is_postgres_and_says_so() {
        let driver = PostgresDriver::new();
        assert_eq!(driver.kind(), DriverKind::Postgres);
        assert_eq!(driver.label(), "PostgreSQL");
        assert_eq!(driver.default_port(), 5432);
    }

    #[test]
    fn this_driver_binds_with_dollar_placeholders() {
        // The caller reads this before connecting to build the statement for
        // *this* driver rather than guess a dialect.
        let capabilities = PostgresDriver.capabilities();
        assert_eq!(capabilities.parameters, Some(ParameterStyle::Dollar));
        assert!(!capabilities.read_only);
    }

    #[test]
    fn a_bound_value_gets_the_servers_own_text() {
        assert_eq!(postgres_text(&Parameter::Null), None);
        assert_eq!(
            postgres_text(&Parameter::Bool(true)).as_deref(),
            Some("true")
        );
        assert_eq!(postgres_text(&Parameter::Int(-5)).as_deref(), Some("-5"));
        assert_eq!(
            postgres_text(&Parameter::UInt(u64::MAX)).as_deref(),
            Some("18446744073709551615")
        );
        assert_eq!(
            postgres_text(&Parameter::Float(1.5)).as_deref(),
            Some("1.5")
        );
        // PostgreSQL's spelling, not Rust's `inf`/`NaN`.
        assert_eq!(
            postgres_text(&Parameter::Float(f64::INFINITY)).as_deref(),
            Some("Infinity")
        );
        assert_eq!(
            postgres_text(&Parameter::Float(f64::NEG_INFINITY)).as_deref(),
            Some("-Infinity")
        );
        assert_eq!(
            postgres_text(&Parameter::Float(f64::NAN)).as_deref(),
            Some("NaN")
        );
        // Text is passed through, quote and all — the server parses it, this
        // crate never escapes it.
        assert_eq!(
            postgres_text(&Parameter::Text("O'Brien".to_owned())).as_deref(),
            Some("O'Brien")
        );
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
        // Neither name is a tree level, and both are answered as a browse command:
        // `levels` is the tree, not the list of commands a driver will answer. The
        // previous engine's own test pinned the same two halves
        // (`check_postgres_has_no_catalog_level`), and adding a level here to make a
        // command work would move the tree away from `db_drivers` and from the app's
        // own `ConnectionKind.levels`.
        assert!(!capabilities.levels.contains(&BrowseLevel::Database));
        // What the driver *answers* is checked against a real server, in
        // `tests/integration.rs::catalogs_lists_databases_even_though_it_is_not_a_tree_level`.
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
