//! Trino: the client protocol spoken by hand, because there is no connection to hold.
//!
//! How a query runs
//! ----------------
//! `POST /v1/statement` starts it and answers with a `nextUri`; each `GET` on that
//! URI returns another *page* — more rows, or more waiting — until a response
//! carries no `nextUri`. Errors arrive on a page too, not only on the `POST`.
//! Cancelling is a `DELETE` on the current page URI. There is no socket, no
//! session state on our side, and nothing to pool: ADR-0006 decided this, and
//! [`Capabilities::persistent_connection`] is `false` to say so out loud.
//!
//! ## Why `columns()` may be empty until the first batch
//!
//! Every other driver here can promise that column metadata is valid the moment
//! `execute` returns. Trino cannot, and the reason is worth stating because it is
//! the same trap that cost the MySQL driver a day:
//!
//! ```text
//! POST /v1/statement   -> stats.state = QUEUED, and there is no `columns` key at all
//! GET  nextUri         -> still QUEUED, still no `columns`
//! GET  nextUri         -> RUNNING, columns, data
//! ```
//!
//! So waiting for the columns inside `execute` would mean waiting for the result
//! set to *begin*, which for a blocking query is when the query *finishes* — and a
//! caller with no cursor has nothing to cancel. [`TrinoCursor`] therefore carries
//! the columns as soon as a page supplies them and reports an empty slice before
//! that, and `execute` returns immediately. Columns are guaranteed present by the
//! time the first batch is returned, which is when a grid can actually use them.
//!
//! ## What the protocol does not carry
//!
//! `timestamp` and `time` lose everything below a millisecond — see [`decode`] for
//! the measurement. That is an upstream edge on this engine's promise of not
//! quietly rounding anything, and it is recorded rather than hidden.
//!
//! ## TLS
//!
//! Only [`TlsMode::Disable`] is accepted, and every other mode is **refused** with
//! a clear error rather than silently connecting in clear. `reqwest` is built with
//! `rustls-tls` so the transport can do it; what is missing is the connector
//! plumbing, and that is tracked separately (K8).

mod decode;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use qh_core::{ColumnBatch, ColumnMeta, EngineError, FailureKind, Value};
use qh_driver::{
    BrowseLevel, Capabilities, ConnectionConfig, Cursor, Driver, DriverKind, ExecuteOptions,
    ObjectPath, ObjectsPage, Session, TlsMode,
};
use serde::Deserialize;
use serde_json::Value as Json;

/// How long to wait between polls of a page URI that is still queued.
///
/// Trino's own clients poll; there is no long-poll. Half a second keeps a
/// hundred-millisecond query from being noticed as slow while not hammering a
/// coordinator that is genuinely still planning.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

// ---------------------------------------------------------------------------
// The wire types
// ---------------------------------------------------------------------------

/// One page of the client protocol.
///
/// Every field is optional because the protocol says so: a `POST` answer has no
/// `columns`, a queued page has no `data`, and the last page has no `nextUri`.
/// Requiring any of them would fail on a response that is perfectly correct.
#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Page {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    next_uri: Option<String>,
    #[serde(default)]
    columns: Option<Vec<WireColumn>>,
    #[serde(default)]
    data: Option<Vec<Vec<Json>>>,
    #[serde(default)]
    error: Option<WireError>,
    /// Rows the statement wrote, when it has a count to report.
    ///
    /// **Top level, not inside `stats`** — measured against Trino 483 rather than
    /// assumed: a `CREATE TABLE AS SELECT` answers with 25, an `INSERT` with 3, and a
    /// `SELECT` or a `DROP` with nothing at all. It is the field the trino client reads
    /// to set `cursor.rowcount`, so reading it is what keeps a write's report the same
    /// number the Python engine would have given.
    #[serde(default)]
    update_count: Option<i64>,
    // `stats` carries the state (QUEUED/RUNNING/FINISHED) and a pile of counters.
    // None of it is read here: whether a query has finished is answered by the
    // absence of `nextUri`, which is the protocol's own rule, and the counters have
    // no home in the Cursor contract. Deserialising it anyway would be a field kept
    // alive by hope, so the extra keys are simply ignored.
}

#[derive(Debug, Deserialize)]
struct WireColumn {
    name: String,
    #[serde(rename = "type")]
    type_text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireError {
    message: String,
    #[serde(default)]
    error_code: Option<i64>,
    #[serde(default)]
    error_name: Option<String>,
    #[serde(default)]
    error_type: Option<String>,
}

/// The running query as far as cancel needs to know it.
#[derive(Debug, Clone)]
struct Running {
    /// The query id, which is also the last resort for cancelling.
    id: String,
    /// The page to `DELETE` to stop it. Trino cancels by deleting the current
    /// page URI, so this is refreshed on every poll.
    next_uri: String,
}

/// Shared so `Session::cancel(&self)` can reach a query the cursor is advancing.
type Shared = Arc<Mutex<Option<Running>>>;

// ---------------------------------------------------------------------------
// The driver
// ---------------------------------------------------------------------------

/// The Trino driver.
///
/// `connect` builds an HTTP client and nothing else: there is no socket to open
/// and therefore nothing that can fail because a server is unreachable. The first
/// statement is where a bad host or a bad port is found, which is why
/// [`TrinoSession::run`] reports the transport failure as a connect failure rather
/// than an empty result.
pub struct TrinoDriver;

impl TrinoDriver {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TrinoDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Driver for TrinoDriver {
    fn kind(&self) -> DriverKind {
        DriverKind::Trino
    }

    fn label(&self) -> &'static str {
        "Trino"
    }

    fn default_port(&self) -> u16 {
        8080
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // Trino has no interactive transaction across statements. Each
            // statement is an HTTP exchange, so there is nothing to hold open.
            transactions: false,
            multiple_result_sets: false,
            // `DELETE` on the page URI, which the server acts on. False would mean
            // cancel only stopped reading locally, and the UI would have to say so.
            cancel: true,
            explain: true,
            levels: vec![
                BrowseLevel::Catalog,
                BrowseLevel::Schema,
                BrowseLevel::Table,
            ],
            // Name and Type, and nothing else, because nothing else is true here:
            // Trino's information_schema exposes catalog/schema/name/type and no
            // OID, owner or ACL. Inventing those columns for Trino would be
            // inventing them, so the list matches the Python engine's driver.
            objects_columns: vec!["Name".to_owned(), "Type".to_owned()],
            // No socket is kept between statements, so anything that assumed one
            // -- an idle timeout, a keepalive, a pool -- must ask first.
            persistent_connection: false,
        }
    }

    async fn connect(&self, config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
        if config.tls != TlsMode::Disable {
            return Err(EngineError::Connect {
                message: format!(
                    "Trino over {:?} is not implemented yet, and this driver will not quietly \
                     connect in clear instead — the transport has TLS but the connector plumbing \
                     does not. Use TLS = disable for now (tracked as K8).",
                    config.tls
                ),
                kind: FailureKind::Permanent,
            });
        }

        // `reqwest::Client` holds the connection pool that makes repeated pages
        // cheap. It is not a *database* connection: nothing here is a session that
        // the server knows about, and the driver still reports
        // `persistent_connection = false`.
        let client = reqwest::Client::builder()
            .build()
            .map_err(|error| EngineError::Connect {
                message: format!("could not build the HTTP client: {error}"),
                kind: FailureKind::Permanent,
            })?;

        Ok(Box::new(TrinoSession {
            client,
            base: format!("{}://{}:{}", "http", config.host, config.port),
            user: non_empty(&config.user).unwrap_or_else(|| "queryhive".to_owned()),
            catalog: config.database.clone().unwrap_or_default(),
            schema: config.schema.clone().unwrap_or_default(),
            running: Arc::new(Mutex::new(None)),
            last_id: None,
        }))
    }
}

fn non_empty(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        None
    } else {
        Some(text.trim().to_owned())
    }
}

// ---------------------------------------------------------------------------
// The session
// ---------------------------------------------------------------------------

struct TrinoSession {
    client: reqwest::Client,
    /// `http://host:port`, with no trailing slash.
    base: String,
    user: String,
    /// Trino's catalog, which is this driver's `database` slot.
    catalog: String,
    schema: String,
    /// What `cancel` needs while a query is in flight, cleared when it ends.
    running: Shared,
    /// The id of the last statement sent, kept after it finishes.
    ///
    /// Separate from `running` because the two answer different questions: `running`
    /// is "is there something to cancel", which stops being true the moment the last
    /// page arrives, and this is "which statement produced the grid the user is
    /// looking at", which is still true afterwards. Trino sends the id with every
    /// statement, so the POST answer is enough to know it.
    last_id: Option<String>,
}

impl TrinoSession {
    /// Resolve a path's catalog and schema, falling back to the connection's own.
    ///
    /// A browse call may carry a catalog even when the connection did not, which
    /// is the whole point of the catalog level existing.
    fn catalog_and_schema(&self, path: &ObjectPath) -> (String, String) {
        let catalog = path
            .catalog
            .clone()
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| self.catalog.clone());
        let schema = path
            .schema
            .clone()
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| self.schema.clone());
        (catalog, schema)
    }

    /// POST one statement and return its first page.
    async fn post(&self, sql: &str) -> Result<Page, EngineError> {
        let mut request = self
            .client
            .post(format!("{}/v1/statement", self.base))
            .header("X-Trino-User", &self.user)
            .header("Content-Type", "text/plain");
        if !self.catalog.is_empty() {
            request = request.header("X-Trino-Catalog", &self.catalog);
        }
        if !self.schema.is_empty() {
            request = request.header("X-Trino-Schema", &self.schema);
        }

        let response =
            request
                .body(sql.to_owned())
                .send()
                .await
                .map_err(|error| EngineError::Connect {
                    message: format!("could not reach {}: {error}", self.base),
                    kind: FailureKind::Transient,
                })?;

        let status = response.status();
        let text = response.text().await.map_err(|error| EngineError::Query {
            message: format!("the response body could not be read: {error}"),
            code: None,
            kind: FailureKind::Transient,
            position: None,
        })?;

        if !status.is_success() {
            // Trino answers 4xx with the same error object as a failed page, so
            // the body is parsed before the status is turned into a message:
            // throwing away a structured error because of a status code loses the
            // reason.
            if let Ok(page) = serde_json::from_str::<Page>(&text) {
                if let Some(error) = page.error {
                    return Err(map_error(error));
                }
            }
            return Err(EngineError::Query {
                message: format!("{status}: {}", text.trim()),
                code: Some(status.as_u16().to_string()),
                kind: FailureKind::Permanent,
                position: None,
            });
        }

        let page: Page = serde_json::from_str(&text).map_err(|error| EngineError::Query {
            message: format!("the response was not a Trino page: {error}"),
            code: None,
            kind: FailureKind::Transient,
            position: None,
        })?;

        if let Some(error) = page.error {
            return Err(map_error(error));
        }
        if let Some(running) = running_from(&page) {
            *self.running.lock().expect("running state") = Some(running);
        }
        Ok(page)
    }

    /// Run a statement to completion and return its rows as text.
    ///
    /// Only for the metadata statements -- `SHOW CATALOGS`, the objects grid --
    /// where the whole result is small and is wanted at once. Anything a user
    /// typed goes through [`Session::execute`] instead, so that it streams and can
    /// be cancelled.
    async fn collect(&mut self, sql: &str) -> Result<Vec<Vec<Value>>, EngineError> {
        let mut cursor = self.execute(sql, &ExecuteOptions::default()).await?;
        let mut rows = Vec::new();
        while let Some(batch) = cursor.next_batch(256).await? {
            let width = batch.columns();
            let height = width.first().map_or(0, Vec::len);
            for index in 0..height {
                rows.push(
                    (0..width.len())
                        .map(|column| width[column][index].clone())
                        .collect(),
                );
            }
        }
        Ok(rows)
    }

    /// The first column of every row, as text.
    ///
    /// `SHOW CATALOGS` and friends return one column, and the tree wants names.
    async fn collect_names(&mut self, sql: &str) -> Result<Vec<String>, EngineError> {
        Ok(self
            .collect(sql)
            .await?
            .into_iter()
            .filter_map(|mut row| {
                if row.is_empty() {
                    return None;
                }
                Some(value_to_text(&row.remove(0)))
            })
            .filter(|name| !name.is_empty())
            .collect())
    }
}

fn running_from(page: &Page) -> Option<Running> {
    let id = page.id.clone()?;
    let next_uri = page.next_uri.clone()?;
    Some(Running { id, next_uri })
}

/// Turn a Trino error into this crate's error.
///
/// `errorType` is what decides whether a retry is worth attempting, and Trino's
/// own vocabulary is used rather than guessed: `USER_ERROR` means the statement or
/// the data is wrong and repeating it will produce the same thing, so it is
/// permanent. Everything else — internal errors, resource exhaustion — is
/// transient, because those are the ones that can pass on a second attempt.
fn map_error(error: WireError) -> EngineError {
    // Checked before `errorType`, because the name is more specific and the two
    // disagree: a cancelled query arrives as `USER_CANCELED` with
    // `errorType = USER_ERROR`. Verified against Trino 483 -- `DELETE` on the page
    // URI produced `{"message":"Query was canceled","errorCode":3,
    // "errorName":"USER_CANCELED","errorType":"USER_ERROR"}`. Classifying that as a
    // generic user error would show a failure to the user who asked for the stop.
    let kind = if error.error_name.as_deref() == Some("USER_CANCELED") {
        FailureKind::Cancelled
    } else {
        match error.error_type.as_deref() {
            Some("USER_ERROR") => FailureKind::Permanent,
            Some(_) => FailureKind::Transient,
            // No type at all: treated as permanent, because the safer mistake is
            // retrying too little rather than looping on something the server will
            // reject identically forever.
            None => FailureKind::Permanent,
        }
    };
    let name = error.error_name.unwrap_or_default();
    let message = if name.is_empty() {
        error.message
    } else {
        format!("{name}: {}", error.message)
    };
    EngineError::Query {
        message,
        code: error.error_code.map(|code| code.to_string()),
        kind,
        position: None,
    }
}

#[async_trait]
impl Session for TrinoSession {
    fn capabilities(&self) -> Capabilities {
        TrinoDriver.capabilities()
    }

    fn query_id(&self) -> Option<String> {
        self.last_id.clone()
    }

    async fn execute(
        &mut self,
        sql: &str,
        options: &ExecuteOptions,
    ) -> Result<Box<dyn Cursor>, EngineError> {
        // Returns as soon as the POST answers, which is normally while the query
        // is still QUEUED. See the module note: waiting here for the columns would
        // mean waiting for the whole query.
        let page = self.post(sql).await?;
        // Recorded here rather than in the cursor: the id arrives with the POST's
        // answer and belongs to the session, which is what `query_id` is asked of.
        if page.id.is_some() {
            self.last_id = page.id.clone();
        }

        let affected = page
            .update_count
            .and_then(|count| u64::try_from(count).ok());
        let columns = columns_of(&page);
        let pending: VecDeque<Vec<Value>> = match (page.columns.as_deref(), page.data.as_deref()) {
            (Some(wire_columns), Some(rows)) => decode_rows(wire_columns, rows).into(),
            _ => VecDeque::new(),
        };
        let finished = page.next_uri.is_none();

        Ok(Box::new(TrinoCursor {
            client: self.client.clone(),
            user: self.user.clone(),
            catalog: self.catalog.clone(),
            schema: self.schema.clone(),
            running: Arc::clone(&self.running),
            next_uri: page.next_uri,
            columns,
            pending,
            finished,
            affected,
            row_limit: options.row_limit,
            emitted: 0,
            max_batch_rows: options.max_batch_rows,
        }))
    }

    async fn browse(
        &mut self,
        level: BrowseLevel,
        path: &ObjectPath,
        include_system: bool,
    ) -> Result<Vec<String>, EngineError> {
        // `include_system` is accepted and unused, exactly as in the Python
        // engine: `SHOW CATALOGS` and `SHOW SCHEMAS` already list everything the
        // coordinator has, so "show all" is a no-op here rather than a call that
        // quietly does something different.
        let _ = include_system;
        let (catalog, schema) = self.catalog_and_schema(path);

        let sql = match level {
            BrowseLevel::Catalog => catalogs_sql(),
            BrowseLevel::Schema => schemas_sql(&catalog)?,
            BrowseLevel::Table => tables_sql(&catalog, &schema)?,
            // Trino's tree is catalog, schema, table, with no database level of its
            // own -- the Python engine mapped the `database` slot onto the catalog.
            // So this is answered with what to use instead, rather than with an
            // empty list the caller would read as an empty catalog.
            BrowseLevel::Database => {
                return Err(EngineError::Usage {
                    message: "Trino has no database level: its tree is catalog, schema, table. \
                              Browse the catalog level instead."
                        .to_owned(),
                })
            }
        };
        self.collect_names(&sql).await
    }

    async fn objects(&mut self, path: &ObjectPath) -> Result<ObjectsPage, EngineError> {
        let (catalog, schema) = self.catalog_and_schema(path);
        let sql = objects_sql(&catalog, &schema)?;
        let rows = self
            .collect(&sql)
            .await?
            .into_iter()
            .map(|row| row.iter().map(value_to_text).collect())
            .collect();

        Ok(ObjectsPage {
            columns: TrinoDriver.capabilities().objects_columns,
            rows,
        })
    }

    fn explain_statement(&self, sql: &str) -> String {
        explain_sql(sql)
    }

    async fn cancel(&self) -> Result<(), EngineError> {
        let running = self.running.lock().expect("running state").clone();

        // Nothing running is not an error: the button can be pressed after the
        // query finished, and reporting that as a failure would be noise.
        let Some(running) = running else {
            return Ok(());
        };

        // `DELETE` on the page URI is how Trino cancels. A non-2xx answer means the
        // query was already gone, which is the outcome that was wanted.
        let target = if running.next_uri.is_empty() {
            format!("{}/v1/statement/{}", self.base, running.id)
        } else {
            running.next_uri
        };
        let response = self
            .client
            .delete(&target)
            .header("X-Trino-User", &self.user)
            .send()
            .await
            .map_err(|error| EngineError::Query {
                message: format!("could not send the cancel: {error}"),
                code: None,
                kind: FailureKind::Transient,
                position: None,
            })?;

        if response.status().is_success() || response.status().as_u16() == 404 {
            *self.running.lock().expect("running state") = None;
            return Ok(());
        }

        Err(EngineError::Query {
            message: format!("the server refused the cancel: {}", response.status()),
            code: Some(response.status().as_u16().to_string()),
            kind: FailureKind::Transient,
            position: None,
        })
    }

    async fn close(self: Box<Self>) -> Result<(), EngineError> {
        // Nothing to close: no socket is held by us. Cancelling a query that is
        // still running is deliberately NOT done here -- a user who closes a tab
        // while a query runs may well want it to finish and be cached, and
        // silently killing it would be a surprise.
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The cursor
// ---------------------------------------------------------------------------

struct TrinoCursor {
    client: reqwest::Client,
    user: String,
    catalog: String,
    schema: String,
    running: Shared,
    next_uri: Option<String>,
    columns: Vec<ColumnMeta>,
    pending: VecDeque<Vec<Value>>,
    finished: bool,
    /// Rows the statement wrote, when the server has said. The count arrives in the
    /// last page, so it is only meaningful once the cursor has been drained.
    affected: Option<u64>,
    row_limit: Option<usize>,
    emitted: usize,
    max_batch_rows: Option<usize>,
}

impl TrinoCursor {
    /// Fetch one more page, appending its rows.
    async fn fetch_page(&mut self) -> Result<(), EngineError> {
        let Some(uri) = self.next_uri.clone() else {
            self.finished = true;
            return Ok(());
        };

        let response = self
            .client
            .get(&uri)
            .header("X-Trino-User", &self.user)
            .header("X-Trino-Catalog", &self.catalog)
            .header("X-Trino-Schema", &self.schema)
            .send()
            .await
            .map_err(|error| EngineError::Query {
                message: format!("could not fetch the next page: {error}"),
                code: None,
                kind: FailureKind::Transient,
                position: None,
            })?;

        let status = response.status();
        let text = response.text().await.map_err(|error| EngineError::Query {
            message: format!("the page body could not be read: {error}"),
            code: None,
            kind: FailureKind::Transient,
            position: None,
        })?;
        if !status.is_success() {
            return Err(EngineError::Query {
                message: format!("{status}: {}", text.trim()),
                code: Some(status.as_u16().to_string()),
                kind: FailureKind::Transient,
                position: None,
            });
        }

        let mut page: Page = serde_json::from_str(&text).map_err(|error| EngineError::Query {
            message: format!("a page was not a Trino page: {error}"),
            code: None,
            kind: FailureKind::Transient,
            position: None,
        })?;

        if let Some(error) = page.error {
            self.finished = true;
            return Err(map_error(error));
        }

        // The columns arrive with the first page that has rows. Taking them here
        // rather than in `execute` is what keeps `execute` from blocking.
        if self.columns.is_empty() {
            if let Some(wire_columns) = page.columns.as_deref() {
                self.columns = wire_columns
                    .iter()
                    .map(|column| ColumnMeta::new(column.name.clone(), column.type_text.clone()))
                    .collect();
            }
        }

        if let (Some(wire_columns), Some(rows)) = (page.columns.as_deref(), page.data.as_deref()) {
            self.pending.extend(decode_rows(wire_columns, rows));
        }

        // Both readings happen before anything is moved out of the page: asking
        // `running_from(&page)` afterwards would borrow a partially moved value.
        let running = running_from(&page);
        let id = page.id.take();
        // A negative count is the protocol's way of saying "nothing to report" rather
        // than -1 rows, so it is not turned into an unsigned one.
        if let Some(count) = page.update_count {
            self.affected = u64::try_from(count).ok();
        }
        self.next_uri = page.next_uri.take();

        if self.next_uri.is_none() {
            self.finished = true;
            *self.running.lock().expect("running state") = None;
        } else if let Some(running) = running {
            *self.running.lock().expect("running state") = Some(running);
        } else if let Some(id) = id {
            *self.running.lock().expect("running state") = Some(Running {
                id,
                next_uri: self.next_uri.clone().unwrap_or_default(),
            });
        }

        Ok(())
    }

    /// How many rows this call should hand back.
    fn budget(&self, max_rows: usize) -> usize {
        let asked = if max_rows == 0 { usize::MAX } else { max_rows };
        let capped = match self.max_batch_rows {
            Some(ceiling) if ceiling > 0 => asked.min(ceiling),
            _ => asked,
        };
        match self.row_limit {
            // The row limit is the caller's ceiling on the *result*, not on the
            // statement: the engine stops reading rather than rewriting the SQL to
            // add a LIMIT, so the query the user sees is the query that ran.
            Some(limit) => capped.min(limit.saturating_sub(self.emitted)),
            None => capped,
        }
    }
}

#[async_trait]
impl Cursor for TrinoCursor {
    fn columns(&self) -> &[ColumnMeta] {
        &self.columns
    }

    fn affected_rows(&self) -> Option<u64> {
        self.affected
    }

    async fn next_batch(&mut self, max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        loop {
            let budget = self.budget(max_rows);

            if !self.pending.is_empty() && budget > 0 {
                let take = budget.min(self.pending.len());
                let rows: Vec<Vec<Value>> = self.pending.drain(..take).collect();
                self.emitted += take;

                // A page is row-major and a batch is column-major, so the
                // transpose happens here, once per batch.
                let width = self.columns.len().max(rows.first().map_or(0, Vec::len));
                let mut columns: Vec<Vec<Value>> = vec![Vec::with_capacity(rows.len()); width];
                for row in rows {
                    for (index, value) in row.into_iter().enumerate() {
                        if index < columns.len() {
                            columns[index].push(value);
                        }
                    }
                }
                if columns.iter().any(|column| column.is_empty()) {
                    // Fewer values than columns would make a ragged batch, which
                    // ColumnBatch refuses. Padding with NULLs keeps the shape the
                    // server declared.
                    for column in columns.iter_mut() {
                        while column.len() < take {
                            column.push(Value::Null);
                        }
                    }
                }
                return Ok(Some(ColumnBatch::new(columns).map_err(|error| {
                    EngineError::Internal {
                        message: format!("the server's page could not form a batch: {error}"),
                    }
                })?));
            }

            if budget == 0 {
                // The row limit is reached. Stop reading rather than run the rest
                // of the query for rows nobody asked for.
                self.finished = true;
                return Ok(None);
            }

            if self.finished {
                return Ok(None);
            }

            // Nothing buffered and more to come: wait for the next page. A queued
            // page carries no rows, so the loop keeps its promise to return rows or
            // an end, rather than an empty batch the caller has to interpret.
            self.fetch_page().await?;
            if self.pending.is_empty() {
                if self.finished {
                    return Ok(None);
                }
                // Still queued or running with no rows yet. Every poll returns a
                // *new* page URI, so waiting on the URI changing would never wait
                // and a slow query would be polled in a tight loop; the pause is
                // what makes this a poll rather than a spin.
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pages into batches
// ---------------------------------------------------------------------------

fn columns_of(page: &Page) -> Vec<ColumnMeta> {
    page.columns
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|column| ColumnMeta::new(column.name.clone(), column.type_text.clone()))
        .collect()
}

fn decode_rows(columns: &[WireColumn], rows: &[Vec<Json>]) -> Vec<Vec<Value>> {
    rows.iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(index, cell)| match columns.get(index) {
                    Some(column) => decode::decode(&column.type_text, cell),
                    // More values than the page declared columns: keep the value
                    // rather than drop it, because dropping is invisible.
                    None => Value::unknown("unknown", cell.to_string()),
                })
                .collect()
        })
        .collect()
}

/// Cell text for the objects grid and for tree names.
///
/// This grid is a listing, not data, so a NULL is the empty string and every value
/// is rendered rather than carried.
///
/// The rendering itself is `qh-core`'s, which is the point: a `varbinary` is hex
/// here, in the grid, and in an export alike, and there is one place that decides
/// it. An earlier draft of this file had its own copy, along with its own decimal
/// formatter -- a second answer to a question that already had one.
fn value_to_text(value: &Value) -> String {
    qh_core::to_text(value).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The SQL, which is the same SQL the Python engine sent
// ---------------------------------------------------------------------------

/// Quote one name the way Trino does: double quotes, doubled inside.
pub fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn catalogs_sql() -> String {
    // Every catalog the coordinator has, system ones included: there is no system
    // set to hide here, which is why `include_system` is a no-op for Trino.
    "SHOW CATALOGS".to_owned()
}

fn schemas_sql(catalog: &str) -> Result<String, EngineError> {
    if catalog.is_empty() {
        return Err(EngineError::Usage {
            message: "a catalog is required to list schemas; set the connection's database or \
                      pass the catalog level first"
                .to_owned(),
        });
    }
    Ok(format!("SHOW SCHEMAS FROM {}", quote(catalog)))
}

fn tables_sql(catalog: &str, schema: &str) -> Result<String, EngineError> {
    if catalog.is_empty() || schema.is_empty() {
        return Err(EngineError::Usage {
            message: "a catalog and a schema are required to list tables".to_owned(),
        });
    }
    Ok(format!(
        "SHOW TABLES FROM {}.{}",
        quote(catalog),
        quote(schema)
    ))
}

fn objects_sql(catalog: &str, schema: &str) -> Result<String, EngineError> {
    if catalog.is_empty() || schema.is_empty() {
        return Err(EngineError::Usage {
            message: "a catalog and a schema are required to list objects".to_owned(),
        });
    }
    // `information_schema.tables` rather than `SHOW TABLES`: both list the same
    // tables, but only this one carries the object's type, which is what makes the
    // grid worth more than the tree beside it.
    Ok(format!(
        "SELECT table_name, table_type FROM {}.information_schema.tables \
         WHERE table_schema = {} ORDER BY 1",
        quote(catalog),
        literal(schema)
    ))
}

/// `EXPLAIN` in front of the statement.
///
/// Trino's plan comes back as one text column, one row per plan line — unlike
/// Postgres, which returns a single `QUERY PLAN` text column holding the whole
/// plan. A free function so it can be asserted without standing up a session.
fn explain_sql(sql: &str) -> String {
    format!("EXPLAIN {sql}")
}

/// A string literal with single quotes doubled, which is SQL's own escaping.
fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_say_what_this_driver_is() {
        let capabilities = TrinoDriver.capabilities();
        assert!(
            !capabilities.persistent_connection,
            "there is no socket to keep"
        );
        assert!(!capabilities.transactions);
        assert!(capabilities.cancel, "DELETE reaches the server");
        assert_eq!(
            capabilities.levels,
            vec![
                BrowseLevel::Catalog,
                BrowseLevel::Schema,
                BrowseLevel::Table
            ]
        );
        assert_eq!(capabilities.objects_columns, vec!["Name", "Type"]);
    }

    #[test]
    fn the_metadata_sql_matches_the_python_engine() {
        // These strings decide what the tree and grid show, so they are pinned:
        // the Rust engine has to answer with the same shape the Python engine did.
        assert_eq!(catalogs_sql(), "SHOW CATALOGS");
        assert_eq!(schemas_sql("hive").unwrap(), "SHOW SCHEMAS FROM \"hive\"");
        assert_eq!(
            tables_sql("hive", "default").unwrap(),
            "SHOW TABLES FROM \"hive\".\"default\""
        );
        assert_eq!(
            objects_sql("hive", "default").unwrap(),
            "SELECT table_name, table_type FROM \"hive\".information_schema.tables \
             WHERE table_schema = 'default' ORDER BY 1"
        );
    }

    #[test]
    fn a_name_is_quoted_the_way_trino_quotes_it() {
        assert_eq!(quote("orders"), "\"orders\"");
        assert_eq!(
            quote("ORDER"),
            "\"ORDER\"",
            "case is preserved, never folded"
        );
        // A quote inside a name is doubled, not escaped with a backslash.
        assert_eq!(quote("we\"ird"), "\"we\"\"ird\"");
        assert_eq!(literal("o'brien"), "'o''brien'");
    }

    #[test]
    fn listing_without_the_level_above_is_refused_with_a_usable_message() {
        // Not an empty list and not a query that fails on the server: the caller
        // is told which setting to fill in.
        assert!(matches!(schemas_sql(""), Err(EngineError::Usage { .. })));
        assert!(matches!(
            tables_sql("hive", ""),
            Err(EngineError::Usage { .. })
        ));
        assert!(matches!(
            objects_sql("", "default"),
            Err(EngineError::Usage { .. })
        ));
    }

    #[test]
    fn explain_is_the_servers_own_plan() {
        assert_eq!(explain_sql("SELECT 1"), "EXPLAIN SELECT 1");
    }

    #[test]
    fn a_users_error_is_permanent_and_a_resource_error_is_not() {
        // The distinction decides whether a retry loop runs. Retrying a syntax
        // error produces the same syntax error forever.
        let user = map_error(WireError {
            message: "mismatched input".to_owned(),
            error_code: Some(1),
            error_name: Some("SYNTAX_ERROR".to_owned()),
            error_type: Some("USER_ERROR".to_owned()),
        });
        match user {
            EngineError::Query {
                kind,
                code,
                message,
                ..
            } => {
                assert_eq!(kind, FailureKind::Permanent);
                assert_eq!(code.as_deref(), Some("1"));
                assert!(message.contains("SYNTAX_ERROR"), "{message}");
            }
            other => panic!("expected a query error, got {other:?}"),
        }

        for error_type in ["INTERNAL_ERROR", "INSUFFICIENT_RESOURCES"] {
            let error = map_error(WireError {
                message: "busy".to_owned(),
                error_code: Some(65537),
                error_name: None,
                error_type: Some(error_type.to_owned()),
            });
            match error {
                EngineError::Query { kind, .. } => assert_eq!(kind, FailureKind::Transient),
                other => panic!("expected a query error, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_cancelled_query_is_a_cancellation_and_not_a_user_error() {
        // The server's own answer to a successful cancel, quoted exactly as Trino
        // 483 sent it. `errorType` says USER_ERROR, which is why the name is read
        // first: the user asked for this, so the UI must not say it failed.
        let error = map_error(WireError {
            message: "Query was canceled".to_owned(),
            error_code: Some(3),
            error_name: Some("USER_CANCELED".to_owned()),
            error_type: Some("USER_ERROR".to_owned()),
        });
        match error {
            EngineError::Query {
                kind,
                code,
                message,
                ..
            } => {
                assert_eq!(kind, FailureKind::Cancelled);
                assert_eq!(code.as_deref(), Some("3"));
                assert!(message.contains("canceled"), "{message}");
            }
            other => panic!("expected a query error, got {other:?}"),
        }
    }

    #[test]
    fn a_page_with_no_columns_is_not_a_failure_to_decode() {
        // The measured sequence: the first pages are QUEUED and carry no columns
        // and no data at all. Requiring either would reject a correct response.
        let page: Page = serde_json::from_str(
            r#"{"id":"20260922_033254_00000_xc6p3","nextUri":"http://x/next","stats":{"state":"QUEUED"}}"#,
        )
        .expect("a queued page is valid");
        assert!(page.columns.is_none());
        assert!(page.data.is_none());
        assert!(columns_of(&page).is_empty());
        // The page also carried `"stats":{"state":"QUEUED"}`, which is deliberately
        // not modelled. Its being ignored rather than rejected is part of what this
        // test pins: an unmodelled key must not fail a correct response.
        assert!(page.next_uri.is_some());
    }

    #[test]
    fn a_finished_page_has_no_next_uri() {
        let page: Page = serde_json::from_str(
            r#"{"id":"q","columns":[{"name":"n","type":"bigint"}],"data":[[1]],"stats":{"state":"FINISHED"}}"#,
        )
        .expect("a final page is valid");
        assert!(page.next_uri.is_none());
        assert!(running_from(&page).is_none(), "nothing left to cancel");
        assert_eq!(columns_of(&page).len(), 1);
    }

    #[test]
    fn a_grid_cell_is_the_text_form_and_a_null_is_empty() {
        // The grid is a listing, not data: every value is rendered, and a NULL is
        // the empty string rather than a missing cell.
        assert_eq!(value_to_text(&Value::Null), "");
        assert_eq!(value_to_text(&Value::Int(42)), "42");
        // Hex, because that is `qh-core`'s answer for bytes, and it is the same one
        // an export gives.
        assert_eq!(value_to_text(&Value::Bytes(vec![0x00, 0xff])), "00ff");
        assert_eq!(
            value_to_text(&Value::Timestamp {
                micros: 0,
                offset_secs: None
            }),
            "1970-01-01 00:00:00"
        );
    }

    #[test]
    fn a_page_becomes_a_column_major_batch() {
        let columns = vec![
            WireColumn {
                name: "n".to_owned(),
                type_text: "bigint".to_owned(),
            },
            WireColumn {
                name: "t".to_owned(),
                type_text: "varchar(1)".to_owned(),
            },
        ];
        let rows = vec![
            vec![Json::from(1), Json::from("a")],
            vec![Json::from(2), Json::from("b")],
        ];
        let decoded = decode_rows(&columns, &rows);
        assert_eq!(
            decoded,
            vec![
                vec![Value::Int(1), Value::Text("a".into())],
                vec![Value::Int(2), Value::Text("b".into())],
            ]
        );
    }

    #[test]
    fn tls_is_refused_rather_than_quietly_downgraded() {
        // A client that accepts `require` and then talks in clear is worse than one
        // that says it cannot yet, because the user would never find out.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        for mode in [TlsMode::Prefer, TlsMode::Require] {
            let config =
                ConnectionConfig::new(DriverKind::Trino, "127.0.0.1", 58080, "hive").tls(mode);
            let outcome = runtime.block_on(TrinoDriver.connect(&config));
            match outcome {
                Err(EngineError::Connect { message, kind }) => {
                    assert_eq!(kind, FailureKind::Permanent);
                    assert!(
                        message.contains("K8"),
                        "the message should name the tracker: {message}"
                    );
                }
                other => panic!("{mode:?} should be refused, got {:?}", other.is_ok()),
            }
        }
    }
}
