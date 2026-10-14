//! Save to Table across connections (W13-T20): read a query on one connection and write the
//! rows as a new table on another.
//!
//! `to_table` hands any run with a `DEST_` setting to [`run`]; a run without one is the
//! `CREATE TABLE AS` it always was. The rows never reach the app: they are read from the
//! source cursor, encoded, and written to the destination in this process, one batch at a
//! time, so the source waits for the destination (the backpressure is the pipe's).
//!
//! - [`types`]: what a source column is ([`types::SqlType`]) and what it is written as
//!   ([`types::map_type`], [`types::plan_table`]): exact, wider, a reported text stand-in, or
//!   refused. Server-supplied type text never reaches SQL.
//! - [`encode`]: one value as a `COPY` text field or a MySQL parameter, or a refusal that
//!   names the rule and never the value.
//! - [`names`]: the one way an identifier is written into SQL, and the names refused.
//! - [`plan`]: the statements, and the decision about each by the destination's own Safe Mode.
//!
//! # The order of a run
//!
//! 1. **No network.** The settings, the source statement (exactly one, a read, allowed by the
//!    source's Safe Mode), the target name, and the destination's decision about the plan.
//!    Every refusal here is a usage error before the first `step`.
//! 2. `step connect_target`: open the destination through [`Engine::for_settings`], set its
//!    session up, and ask whether the table exists (a create over a table fails here, before
//!    the expensive query).
//! 3. `step connect`: open the source, make it read-only on the server, and for PostgreSQL fix
//!    the text settings that make every value printed the same way (condition C-3), then run
//!    the query.
//! 4. `step prepare`, `start`, `step write`: from the first batch the columns and their types
//!    become the table, and the rows flow.
//! 5. `step verify`: the rows read must be the rows the destination acknowledged.
//! 6. `step swap`: the table takes its name. PostgreSQL commits one transaction; MySQL renames.
//!
//! # The live range
//!
//! From the moment the source cursor exists until it answers "no more rows", the cursor is held
//! by a [`LiveSource`], and the **only** way out is [`LiveSource::finish`]: for anything but a
//! finished cursor it asks the server to cancel *before* the cursor is dropped, because a MySQL
//! producer ends when its cursor is dropped and a cancel sent after that has nothing left to
//! name. The functions in that range return a value ([`LiveEnd`]) and never use `?`, so no
//! error can leave the range by dropping the cursor first. A panic is the one exit this cannot
//! cover (R-16).

pub mod encode;
pub mod names;
pub mod plan;
pub mod types;

use std::time::{Duration, Instant};

use qh_core::{ColumnBatch, Value};
use qh_driver::{ConnectionConfig, CopyIn, Cursor, DriverKind, ExecuteOptions, Parameter, Session};
use qh_sql::{statements_dialect, strip_terminator_dialect, Dialect, FloorSource, SafeModeFloor};
use serde_json::{json, Value as Json};

use self::encode::{copy_row, mysql_row, MysqlCell, Violation};
use self::plan::{
    gate, insert_budget, parse_max_allowed_packet, probe, Decider, EngineStatement, Planned,
    ProbeCall, WritePlan,
};
use self::types::{plan_table, PlannedColumn, SqlType, TablePlan, Target};
use crate::commands::{
    cancel_bounded, columns_json, connection, dialect, execute_until_stopped, guard_for, open,
    parse_safe_mode, require_read, safe_mode, safe_mode_floor, source_sql, statement_timeout,
    stop_grace, stop_session, STOP_CEILING,
};
use crate::env::{Destination, Settings, TransferMode};
use crate::events::{event, Emitter};
use crate::progress::PROGRESS_MS_DEFAULT;
use crate::table_swap::{finish_statement, scratch_stamp};
use crate::{CancelFlag, CliError, Engine, ErrorSide};

/// Why the source statement may be read and nothing else, in the log.
const SOURCE_ONLY: &str = "Save to Table across connections reads the source and nothing else";

/// Rows a source batch asks for, a `COPY` window's limits, a MySQL `INSERT`'s width, and how
/// much encoded text waits before it is sent (blueprint §3.4, §3.6).
const FETCH_ROWS: i64 = 1_000;
const COPY_ROWS: i64 = 50_000;
const COPY_BYTES: i64 = 64 * 1024 * 1024;
const BATCH_ROWS: i64 = 1_000;
const SEND_BYTES: usize = 1024 * 1024;
const WINDOW_CEILING: Duration = Duration::from_secs(15);
/// MySQL takes 65,535 placeholders in a prepared statement.
const MYSQL_PLACEHOLDERS: usize = 65_535;

struct Knobs {
    fetch_rows: usize,
    copy_rows: u64,
    copy_bytes: u64,
    batch_rows: usize,
    window: Duration,
    progress_ms: i64,
}

impl Knobs {
    fn read(settings: &Settings, dest_timeout: Option<Duration>) -> Result<Self, CliError> {
        let positive = |key: &str, default: i64| -> Result<u64, CliError> {
            let value = settings.number(key, default)?;
            u64::try_from(value)
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| CliError::Usage(format!("{key} must be a positive number")))
        };
        // A statement of the destination should not outlive a quarter of its bound, and no
        // window outlives fifteen seconds (the bound being 0, "none", gives the ceiling).
        let window = dest_timeout
            .map_or(WINDOW_CEILING, |bound| (bound / 4).min(WINDOW_CEILING))
            .max(Duration::from_millis(1));
        Ok(Self {
            fetch_rows: positive("TRANSFER_FETCH_ROWS", FETCH_ROWS)? as usize,
            copy_rows: positive("TRANSFER_COPY_ROWS", COPY_ROWS)?,
            copy_bytes: positive("TRANSFER_COPY_BYTES", COPY_BYTES)?,
            batch_rows: positive("TRANSFER_BATCH_ROWS", BATCH_ROWS)? as usize,
            window,
            progress_ms: settings.number("PROGRESS_MS", PROGRESS_MS_DEFAULT)?,
        })
    }
}

/// Everything decided without the network.
struct Setup {
    body: String,
    source_config: ConnectionConfig,
    dest_config: ConnectionConfig,
    target: Target,
    /// `[schema, table]` or `[database, table]`.
    parts: [String; 2],
    dest_dialect: Dialect,
    floor: SafeModeFloor,
    confirmed: bool,
    mode: TransferMode,
    source_timeout: Option<Duration>,
    dest_timeout: Option<Duration>,
    knobs: Knobs,
}

fn on_source(error: impl Into<CliError>) -> CliError {
    CliError::on(ErrorSide::Source, error.into())
}

fn on_target(error: impl Into<CliError>) -> CliError {
    CliError::on(ErrorSide::Target, error.into())
}

/// Step 1: every refusal that needs no network.
fn setup(
    settings: &Settings,
    source: &Settings,
    dest: &Settings,
    mode: TransferMode,
    engine: &dyn Engine,
) -> Result<Setup, CliError> {
    // The source statement: one statement, a read, allowed by the source's own Safe Mode.
    let source_dialect = dialect(source);
    let sql = source_sql(source)?;
    let body = strip_terminator_dialect(sql.trim(), source_dialect);
    let count = statements_dialect(&body, source_dialect).len();
    if count != 1 {
        return Err(CliError::Usage(format!(
            "Save to Table across connections takes exactly one statement, and this has {count}"
        )));
    }
    let source_mode = safe_mode(source, engine).map_err(on_source)?;
    require_read(source, source_mode, &body, SOURCE_ONLY)?;

    // A setting the connection cannot be built from belongs to the side whose setting it is.
    let source_config = connection(source, engine).map_err(on_source)?;
    let dest_config = connection(dest, engine).map_err(on_target)?;
    let Some(target) = Target::of(dest_config.kind) else {
        return Err(CliError::Usage(
            "a table can be saved across connections to PostgreSQL or MySQL; Trino is not a \
             destination"
                .to_owned(),
        ));
    };

    // The name: the parts the destination's kind has, each checked.
    let (namespace_key, namespace_label) = match target {
        Target::Postgres => ("TARGET_SCHEMA", "schema"),
        Target::Mysql => ("TARGET_CATALOG", "database"),
    };
    let namespace = settings.text(namespace_key, "");
    let table = settings.text("TARGET_TABLE", "");
    let mut missing = Vec::new();
    if namespace.is_empty() {
        missing.push(namespace_key);
    }
    if table.is_empty() {
        missing.push("TARGET_TABLE");
    }
    if !missing.is_empty() {
        return Err(CliError::Usage(format!(
            "{} required to write a table",
            missing.join(" and ")
        )));
    }
    for (part, label) in [(&namespace, namespace_label), (&table, "table")] {
        names::check(target, part).map_err(|error| {
            CliError::Usage(format!("the destination {label} name {part:?} {error}"))
        })?;
    }

    // The destination's decision about the plan, from its two floors: its own
    // `DEST_SAFE_MODE_FLOOR` and the top-level `SAFE_MODE_FLOOR`, each an entry.
    let dest_dialect = dialect(dest);
    let mut floor = safe_mode_floor(dest, engine).map_err(on_target)?;
    let pinned = settings.text("SAFE_MODE_FLOOR", "");
    if !pinned.is_empty() {
        floor.raise(
            FloorSource::Policy,
            parse_safe_mode(&pinned, "SAFE_MODE_FLOOR")?,
        );
    }
    let confirmed = dest.flag("SAFE_MODE_CONFIRMED", false);
    let qualified = names::qualified(target, &[&namespace, &table])
        .map_err(|fault| CliError::Usage(format!("the destination name {}", fault.error)))?;
    let subject = match mode {
        TransferMode::Create => format!("CREATE TABLE {qualified}"),
        TransferMode::Replace => format!("DROP TABLE {qualified}"),
    };
    gate(&floor, confirmed, dest_dialect, mode, &subject)?;
    // After the destination's decision: `guard_for` logs an allowance, and a run the
    // destination refuses must not leave a row saying its SELECT was allowed to run.
    guard_for(source, source_mode, &body)?;

    let source_timeout = statement_timeout(source).map_err(on_source)?;
    let dest_timeout = statement_timeout(dest).map_err(on_target)?;
    Ok(Setup {
        body,
        source_config,
        dest_config,
        target,
        parts: [namespace, table],
        dest_dialect,
        floor,
        confirmed,
        mode,
        source_timeout,
        dest_timeout,
        knobs: Knobs::read(settings, dest_timeout)?,
    })
}

// --------------------------------------------------------------------------- //
// the source in its live range
// --------------------------------------------------------------------------- //

/// A source session and its open cursor.
struct LiveSource {
    session: Box<dyn Session>,
    cursor: Box<dyn Cursor>,
}

impl LiveSource {
    /// The one way out of the live range.
    ///
    /// A cursor that answered "no more rows" is just closed. Anything else (a Stop, a
    /// destination error, a refused type or value, an internal error) asks the server to
    /// cancel **first**, then drops the cursor, then closes: [`stop_session`] is that order.
    /// Returns the warning to put on the `done` when the server did not confirm the cancel.
    async fn finish(self, completed: bool) -> Option<String> {
        if completed {
            drop(self.cursor);
            let _ = tokio::time::timeout(STOP_CEILING, self.session.close()).await;
            None
        } else {
            stop_session(self.session, Some(self.cursor)).await
        }
    }
}

/// How the live range ended. Built by an explicit `return`, never by `?`.
enum LiveEnd {
    /// The cursor answered "no more rows". `rows_read` is every row it delivered.
    Finished {
        rows_read: u64,
    },
    Stopped {
        rows_read: u64,
    },
    /// An error already placed on its side.
    Failed(CliError),
}

// --------------------------------------------------------------------------- //
// the destination
// --------------------------------------------------------------------------- //

/// What the destination knows once the first batch has named the columns.
struct Prepared {
    plan: WritePlan,
    table_plan: TablePlan,
    decider: Decider,
    /// MySQL: rows per `INSERT`.
    per_insert: usize,
}

/// A `COPY` in progress.
struct Window {
    copy: Box<dyn CopyIn>,
    rows: u64,
    bytes: u64,
    opened: Instant,
}

struct Dest<'a> {
    session: Box<dyn Session>,
    setup: &'a Setup,
    prepared: Option<Prepared>,
    stamp: String,
    existed: bool,
    /// MySQL: bytes an `INSERT` may carry.
    budget: u64,
    window: Option<Window>,
    buffer: Vec<u8>,
    pending: Vec<Parameter>,
    pending_rows: usize,
    pending_bytes: u64,
    /// Rows the destination acknowledged.
    acked: u64,
    /// The table (PostgreSQL create) or scratch (everything else) has been created.
    created: bool,
    /// A PostgreSQL transaction is open.
    transaction: bool,
    /// Columns written as a wider type, for the `done` warnings.
    widenings: Vec<String>,
}

fn options(timeout: Option<Duration>) -> ExecuteOptions {
    ExecuteOptions {
        statement_timeout: timeout,
        ..ExecuteOptions::default()
    }
}

fn parameter(cell: MysqlCell) -> Parameter {
    match cell {
        MysqlCell::Null => Parameter::Null,
        MysqlCell::Int(number) => Parameter::Int(number),
        MysqlCell::UInt(number) => Parameter::UInt(number),
        MysqlCell::Float(number) => Parameter::Float(number),
        MysqlCell::Text(text) => Parameter::Text(text),
        MysqlCell::Bytes(bytes) => Parameter::Bytes(bytes),
    }
}

/// About how many bytes a parameter takes on the wire; high rather than exact.
fn parameter_bytes(parameter: &Parameter) -> u64 {
    match parameter {
        Parameter::Null | Parameter::Bool(_) => 1,
        Parameter::Int(_) | Parameter::UInt(_) | Parameter::Float(_) => 9,
        Parameter::Text(text) => text.len() as u64 + 9,
        Parameter::Bytes(bytes) => bytes.len() as u64 + 9,
    }
}

impl<'a> Dest<'a> {
    fn new(session: Box<dyn Session>, setup: &'a Setup) -> Self {
        Dest {
            session,
            setup,
            prepared: None,
            stamp: scratch_stamp(),
            existed: false,
            budget: insert_budget(None),
            window: None,
            buffer: Vec::new(),
            pending: Vec::new(),
            pending_rows: 0,
            pending_bytes: 0,
            acked: 0,
            created: false,
            transaction: false,
            widenings: Vec::new(),
        }
    }

    /// One of the engine's own statements. Not classified and not logged: the plan that
    /// needs it has already been allowed (see [`plan`]).
    async fn engine(&mut self, statement: EngineStatement) -> Result<(), CliError> {
        finish_statement(&mut self.session, statement.text(), self.setup.dest_timeout)
            .await
            .map(|_| ())
            .map_err(on_target)
    }

    /// A written statement: structure checked, decided (logged once per text), then sent.
    async fn statement(&mut self, planned: &Planned) -> Result<(), CliError> {
        let prepared = self
            .prepared
            .as_mut()
            .ok_or_else(|| on_target(CliError::Internal("no plan to send".to_owned())))?;
        prepared.decider.allow(planned).map_err(on_target)?;
        finish_statement(&mut self.session, &planned.text, self.setup.dest_timeout)
            .await
            .map(|_| ())
            .map_err(on_target)
    }

    /// Session setup and the existence probe, before the source is touched.
    async fn connect_setup(&mut self) -> Result<(), CliError> {
        if self.setup.target == Target::Mysql {
            self.engine(EngineStatement::StrictMode).await?;
            self.engine(EngineStatement::Utf8mb4).await?;
            self.budget = insert_budget(self.read_max_packet().await);
        }
        self.existed = self.probe().await?;
        if self.existed && self.setup.mode == TransferMode::Create {
            return Err(on_target(CliError::Query(format!(
                "the table {}.{} already exists; choose another name, or choose Replace",
                self.setup.parts[0], self.setup.parts[1]
            ))));
        }
        Ok(())
    }

    async fn read_max_packet(&mut self) -> Option<u64> {
        let mut cursor = self
            .session
            .execute(
                EngineStatement::MaxPacket.text(),
                &options(self.setup.dest_timeout),
            )
            .await
            .ok()?;
        let mut packet = None;
        while let Ok(Some(batch)) = cursor.next_batch(10).await {
            packet = packet.or_else(|| {
                batch
                    .columns()
                    .first()
                    .and_then(|column| column.first())
                    .and_then(parse_max_allowed_packet)
            });
        }
        packet
    }

    /// Whether the destination already has a relation of this name.
    async fn probe(&mut self) -> Result<bool, CliError> {
        let parts = [self.setup.parts[0].as_str(), self.setup.parts[1].as_str()];
        let call = probe(self.setup.target, parts).map_err(|fault| {
            on_target(CliError::Usage(format!(
                "the destination name {}",
                fault.error
            )))
        })?;
        let options = options(self.setup.dest_timeout);
        let mut cursor = match call {
            ProbeCall::Text(read) => {
                plan::check_structure(&read, self.setup.dest_dialect).map_err(on_target)?;
                self.session
                    .execute(&read.text, &options)
                    .await
                    .map_err(on_target)?
            }
            ProbeCall::Bound(statement, values) => {
                let bound: Vec<Parameter> = values.into_iter().map(Parameter::Text).collect();
                self.session
                    .execute_bound(statement.text(), &bound, &options)
                    .await
                    .map_err(on_target)?
            }
        };
        let mut found = false;
        while let Some(batch) = cursor.next_batch(10).await.map_err(on_target)? {
            found |= batch
                .columns()
                .first()
                .and_then(|column| column.first())
                .is_some_and(|cell| match cell {
                    Value::Null => false,
                    Value::Int(count) => *count > 0,
                    Value::UInt(count) => *count > 0,
                    // A shape this does not know reads as "there is one": a create then
                    // refuses a name that may be free, where the other guess could replace
                    // a table unwarned.
                    _ => true,
                });
        }
        Ok(found)
    }

    /// Plan the write from the first batch's columns and open the transaction or the scratch.
    async fn begin(&mut self, table_plan: TablePlan) -> Result<(), CliError> {
        let setup = self.setup;
        let parts = [setup.parts[0].as_str(), setup.parts[1].as_str()];
        let plan = WritePlan::build(&table_plan, setup.mode, parts, self.existed, &self.stamp)
            .map_err(|fault| {
                on_target(CliError::Usage(format!(
                    "the destination name {}",
                    fault.error
                )))
            })?;
        let mut decider = Decider::new(&setup.floor, setup.confirmed, setup.dest_dialect);
        for planned in plan.statements() {
            decider.allow(&planned).map_err(on_target)?;
        }
        let per_insert = setup
            .knobs
            .batch_rows
            .min((MYSQL_PLACEHOLDERS / table_plan.columns.len().max(1)).max(1));
        let create = plan.create.clone();
        self.prepared = Some(Prepared {
            plan,
            table_plan,
            decider,
            per_insert,
        });
        if setup.target == Target::Postgres {
            self.engine(EngineStatement::Begin).await?;
            self.transaction = true;
        }
        // Marked first: a create that fails half-way may still have made the table.
        self.created = true;
        self.statement(&create).await
    }

    fn columns(&self) -> &[PlannedColumn] {
        self.prepared
            .as_ref()
            .map_or(&[], |prepared| prepared.table_plan.columns.as_slice())
    }

    /// Write one source row.
    async fn append(&mut self, values: &[&Value], source_row: u64) -> Result<(), CliError> {
        match self.setup.target {
            Target::Postgres => self.append_copy(values, source_row).await,
            Target::Mysql => self.append_insert(values, source_row).await,
        }
    }

    fn refused(&self, violation: Violation, source_row: u64) -> CliError {
        // The rule and the column, never the value.
        on_target(CliError::Query(
            violation.describe(self.columns(), source_row),
        ))
    }

    async fn append_copy(&mut self, values: &[&Value], source_row: u64) -> Result<(), CliError> {
        if self.window.is_none() {
            let copy = self
                .prepared
                .as_ref()
                .and_then(|prepared| prepared.plan.copy.clone())
                .ok_or_else(|| on_target(CliError::Internal("no COPY statement".to_owned())))?;
            let opened = self
                .session
                .copy_in(&copy.text, &options(self.setup.dest_timeout))
                .await
                .map_err(on_target)?;
            self.window = Some(Window {
                copy: opened,
                rows: 0,
                bytes: 0,
                opened: Instant::now(),
            });
        }
        let before = self.buffer.len();
        let result = match self.prepared.as_ref() {
            Some(prepared) => copy_row(
                &prepared.table_plan.columns,
                values.iter().copied(),
                &mut self.buffer,
            ),
            None => Ok(()),
        };
        if let Err(violation) = result {
            return Err(self.refused(violation, source_row));
        }
        let added = (self.buffer.len() - before) as u64;
        let (rows, bytes) = match self.window.as_mut() {
            Some(window) => {
                window.rows += 1;
                window.bytes += added;
                (window.rows, window.bytes)
            }
            None => (0, 0),
        };
        if self.buffer.len() >= SEND_BYTES {
            self.send_buffer().await?;
        }
        if rows >= self.setup.knobs.copy_rows || bytes >= self.setup.knobs.copy_bytes {
            self.close_window().await?;
        }
        Ok(())
    }

    async fn send_buffer(&mut self) -> Result<(), CliError> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let chunk = std::mem::take(&mut self.buffer);
        match self.window.as_mut() {
            Some(window) => window.copy.send(chunk).await.map_err(on_target),
            None => Ok(()),
        }
    }

    /// End the open `COPY`: what the server loaded must be what was sent.
    async fn close_window(&mut self) -> Result<(), CliError> {
        self.send_buffer().await?;
        let Some(window) = self.window.take() else {
            return Ok(());
        };
        let sent = window.rows;
        let loaded = window.copy.finish().await.map_err(on_target)?;
        if loaded != sent {
            return Err(on_target(CliError::Query(format!(
                "the destination loaded {loaded} of {sent} rows of a window"
            ))));
        }
        self.acked += loaded;
        Ok(())
    }

    /// When the open window must close, whatever the source is doing.
    fn window_deadline(&self) -> Option<tokio::time::Instant> {
        self.window
            .as_ref()
            .map(|window| tokio::time::Instant::from_std(window.opened + self.setup.knobs.window))
    }

    async fn append_insert(&mut self, values: &[&Value], source_row: u64) -> Result<(), CliError> {
        let cells = match self.prepared.as_ref() {
            Some(prepared) => mysql_row(&prepared.table_plan.columns, values.iter().copied()),
            None => Ok(Vec::new()),
        };
        let cells = match cells {
            Ok(cells) => cells,
            Err(violation) => return Err(self.refused(violation, source_row)),
        };
        for cell in cells {
            let parameter = parameter(cell);
            self.pending_bytes += parameter_bytes(&parameter);
            self.pending.push(parameter);
        }
        self.pending_rows += 1;
        let per_insert = self
            .prepared
            .as_ref()
            .map_or(1, |prepared| prepared.per_insert);
        if self.pending_rows >= per_insert || self.pending_bytes >= self.budget {
            self.flush_insert().await?;
        }
        Ok(())
    }

    /// Send the rows waiting as one `INSERT`; the server's count must be the rows sent.
    async fn flush_insert(&mut self) -> Result<(), CliError> {
        if self.pending_rows == 0 {
            return Ok(());
        }
        let rows = self.pending_rows;
        let planned = match self.prepared.as_mut() {
            Some(prepared) => {
                let planned = prepared.plan.insert(rows);
                prepared.decider.allow(&planned).map_err(on_target)?;
                planned
            }
            None => return Ok(()),
        };
        let parameters = std::mem::take(&mut self.pending);
        self.pending_rows = 0;
        self.pending_bytes = 0;
        let mut cursor = self
            .session
            .execute_bound(
                &planned.text,
                &parameters,
                &options(self.setup.dest_timeout),
            )
            .await
            .map_err(on_target)?;
        while cursor.next_batch(1_000).await.map_err(on_target)?.is_some() {}
        let written = cursor.affected_rows().ok_or_else(|| {
            on_target(CliError::Query(
                "the destination did not say how many rows it wrote".to_owned(),
            ))
        })?;
        if written != rows as u64 {
            return Err(on_target(CliError::Query(format!(
                "the destination wrote {written} of {rows} rows of a batch"
            ))));
        }
        self.acked += written;
        Ok(())
    }

    /// The last rows, when the source has no more.
    async fn flush_all(&mut self) -> Result<(), CliError> {
        match self.setup.target {
            Target::Postgres => self.close_window().await,
            Target::Mysql => self.flush_insert().await,
        }
    }

    /// Everything after the verification: the table takes its name.
    async fn swap(&mut self, warnings: &mut Vec<String>) -> Result<(), CliError> {
        let (swap, drop_old) = match self.prepared.as_ref() {
            Some(prepared) => (prepared.plan.swap.clone(), prepared.plan.drop_old.clone()),
            None => return Ok(()),
        };
        for planned in &swap {
            self.statement(planned).await?;
        }
        if self.setup.target == Target::Postgres {
            self.engine(EngineStatement::Commit).await?;
            self.transaction = false;
        }
        self.created = false;
        if let Some(drop_old) = drop_old {
            if let Err(error) = self.statement(&drop_old).await {
                let label = self
                    .prepared
                    .as_ref()
                    .map(|prepared| prepared.plan.backup_label(&self.stamp))
                    .unwrap_or_default();
                warnings.push(format!(
                    "the new table is in place, and the old one is kept as {label}: {}",
                    error.message()
                ));
            }
        }
        Ok(())
    }

    /// Undo what a run that did not finish left on the destination, and close it. Returns the
    /// warnings: what, if anything, was left behind.
    async fn abort(mut self) -> Vec<String> {
        let mut warnings = Vec::new();
        // The statement that may still be running is cancelled first, bounded; only then is the
        // copy dropped, which is what sends `CopyFail`.
        if let Some(window) = self.window.take() {
            let _ = cancel_bounded(STOP_CEILING, self.session.cancel()).await;
            drop(window);
        }
        if self.transaction {
            // Under the same statement bound as everything before it: the PostgreSQL driver sends
            // `SET statement_timeout` ahead of a statement whose bound differs, and on an
            // aborted transaction the server refuses that SET, so the ROLLBACK behind it would
            // never be sent.
            let rolled = tokio::time::timeout(
                STOP_CEILING,
                finish_statement(
                    &mut self.session,
                    EngineStatement::Rollback.text(),
                    self.setup.dest_timeout,
                ),
            )
            .await;
            if !matches!(rolled, Ok(Ok(_))) {
                warnings.push(
                    "the destination transaction could not be rolled back; the server undoes it \
                     when the connection ends"
                        .to_owned(),
                );
            }
        } else if self.created && self.setup.target == Target::Mysql {
            let cleanup = self.prepared.as_mut().and_then(|prepared| {
                let cleanup = prepared.plan.cleanup.clone()?;
                let label = prepared.plan.scratch_label.clone().unwrap_or_default();
                prepared
                    .decider
                    .allow(&cleanup)
                    .ok()
                    .map(|()| (cleanup, label))
            });
            let label = cleanup.as_ref().map(|(_, label)| label.clone());
            let dropped = match cleanup {
                Some((cleanup, _)) => tokio::time::timeout(
                    STOP_CEILING,
                    finish_statement(&mut self.session, &cleanup.text, self.setup.dest_timeout),
                )
                .await
                .map_err(|_| "no answer within the stop budget".to_owned())
                .and_then(|result| result.map(|_| ()).map_err(|error| error.message())),
                None => Err("the clean-up was not allowed".to_owned()),
            };
            if let Err(reason) = dropped {
                warnings.push(format!(
                    "the temporary table {} was left behind: {reason}",
                    label.unwrap_or_default()
                ));
            }
        }
        let _ = tokio::time::timeout(STOP_CEILING, self.session.close()).await;
        warnings
    }

    async fn close(self) {
        let _ = tokio::time::timeout(STOP_CEILING, self.session.close()).await;
    }
}

// --------------------------------------------------------------------------- //
// the run
// --------------------------------------------------------------------------- //

fn emit(out: &mut dyn Emitter, value: Json) -> Result<(), CliError> {
    out.emit(value).map_err(CliError::from)
}

fn step(out: &mut dyn Emitter, name: &str) -> Result<(), CliError> {
    emit(out, event("step").field("step", name).build())
}

fn with_warnings(error: CliError, warnings: Vec<String>) -> CliError {
    if warnings.is_empty() {
        return error;
    }
    let side = error.side();
    let warned = CliError::Warned {
        message: error.message(),
        warnings,
    };
    match side {
        Some(side) => CliError::on(side, warned),
        None => warned,
    }
}

/// Copy a query's rows to a table on another connection.
pub(crate) async fn run(
    settings: &Settings,
    destination: Destination,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
    cancel: &CancelFlag,
) -> Result<(), CliError> {
    let Destination {
        source,
        dest,
        mode,
        safe_mode: _,
    } = destination;
    let setup = setup(settings, &source, &dest, mode, engine)?;

    // ---- the destination
    step(out, "connect_target")?;
    let Some(dest_engine) = engine.for_settings(dest.clone()) else {
        return Err(CliError::Internal(
            "this engine cannot open a second connection (Engine::for_settings)".to_owned(),
        ));
    };
    let (dest_session, _) = open(&dest, &*dest_engine, &setup.dest_config)
        .await
        .map_err(on_target)?;
    let mut target = Dest::new(dest_session, &setup);
    if let Err(error) = target.connect_setup().await {
        let warnings = target.abort().await;
        return Err(with_warnings(error, warnings));
    }

    // ---- the source
    step(out, "connect")?;
    let opened = open_source(&source, engine, &setup, cancel).await;
    let (source_session, cursor) = match opened {
        Ok(Some(pair)) => pair,
        Ok(None) => {
            // A Stop before the query had a cursor: nothing was read and nothing written.
            target.close().await;
            return finish_stopped(out, &setup, 0, None, Vec::new());
        }
        Err(error) => {
            let warnings = target.abort().await;
            return Err(with_warnings(error, warnings));
        }
    };

    // ---- the live range
    let mut live = LiveSource {
        session: source_session,
        cursor,
    };
    let end = live_range(&mut live, &mut target, &setup, out, cancel).await;
    let query_id = live.session.query_id();
    match end {
        LiveEnd::Finished { rows_read } => {
            let _ = live.finish(true).await;
            conclude(target, &setup, out, rows_read, query_id).await
        }
        LiveEnd::Stopped { rows_read } => {
            let mut warnings: Vec<String> = live.finish(false).await.into_iter().collect();
            warnings.extend(target.abort().await);
            finish_stopped(out, &setup, rows_read, query_id, warnings)
        }
        LiveEnd::Failed(error) => {
            let mut warnings: Vec<String> = live.finish(false).await.into_iter().collect();
            warnings.extend(target.abort().await);
            Err(with_warnings(error, warnings))
        }
    }
}

/// Open the source, make it read-only on the server, fix its text settings, run the query.
/// `Ok(None)` is a Stop that arrived before the cursor did.
async fn open_source(
    source: &Settings,
    engine: &dyn Engine,
    setup: &Setup,
    cancel: &CancelFlag,
) -> Result<Option<(Box<dyn Session>, Box<dyn Cursor>)>, CliError> {
    let (mut session, policy) = open(source, engine, &setup.source_config)
        .await
        .map_err(on_source)?;
    // Always, whatever the source's Safe Mode: this run reads and nothing else. A session that
    // cannot be made read-only is not used. Trino has no such switch.
    if let Err(error) = session.enforce_read_only().await {
        let _ = session.close().await;
        return Err(on_source(CliError::from(error)));
    }
    if setup.source_config.kind == DriverKind::Postgres {
        // C-3: every value that is forwarded as the server printed it is printed the same way.
        for statement in EngineStatement::SOURCE_READING {
            if let Err(error) = finish_statement(&mut session, statement.text(), None).await {
                let _ = session.close().await;
                return Err(on_source(error));
            }
        }
    }
    let query = ExecuteOptions {
        max_batch_rows: Some(setup.knobs.fetch_rows),
        row_limit: None,
        statement_timeout: setup.source_timeout,
    };
    let started = execute_until_stopped(
        cancel,
        &mut session,
        &policy,
        &setup.body,
        &query,
        stop_grace(&setup.source_config),
    )
    .await;
    match started {
        Some(Ok(cursor)) => Ok(Some((session, cursor))),
        Some(Err(error)) => {
            let _ = session.close().await;
            Err(on_source(CliError::from(error)))
        }
        None => {
            let _ = cancel_bounded(STOP_CEILING, session.cancel()).await;
            let _ = tokio::time::timeout(STOP_CEILING, session.close()).await;
            Ok(None)
        }
    }
}

/// How an attempt to take the next batch ended.
enum Fetched {
    Batch(Option<ColumnBatch>),
    Stopped,
    Failed(CliError),
}

/// The next batch, with the destination's `COPY` window closing on its own timer meanwhile.
///
/// The fetch is pinned and survives the timer: a slow source must not hold a window open past
/// its bound (N-4), and dropping a half-done fetch would lose a page.
async fn fetch(
    cursor: &mut Box<dyn Cursor>,
    rows: usize,
    dest: &mut Dest<'_>,
    cancel: &CancelFlag,
) -> Fetched {
    let mut next = std::pin::pin!(cursor.next_batch(rows));
    loop {
        let deadline = dest.window_deadline();
        tokio::select! {
            biased;
            () = cancel.cancelled() => return Fetched::Stopped,
            batch = &mut next => {
                return match batch {
                    Ok(batch) => Fetched::Batch(batch),
                    Err(error) => Fetched::Failed(on_source(CliError::from(error))),
                };
            }
            () = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                if let Err(error) = dest.close_window().await {
                    return Fetched::Failed(error);
                }
            }
        }
    }
}

/// From the first batch to the cursor's end. **No `?` in this function**: every way out is a
/// [`LiveEnd`], so the caller can finish the source in the one order that cancels first.
async fn live_range(
    live: &mut LiveSource,
    dest: &mut Dest<'_>,
    setup: &Setup,
    out: &mut dyn Emitter,
    cancel: &CancelFlag,
) -> LiveEnd {
    let knobs = &setup.knobs;
    if let Err(error) = step(out, "prepare") {
        return LiveEnd::Failed(error);
    }
    // The first batch comes before anything is announced: a Trino cursor has no columns until
    // it arrives.
    let mut next = match fetch(&mut live.cursor, knobs.fetch_rows, dest, cancel).await {
        Fetched::Batch(batch) => batch,
        Fetched::Stopped => return LiveEnd::Stopped { rows_read: 0 },
        Fetched::Failed(error) => return LiveEnd::Failed(error),
    };

    let columns = live.cursor.columns().to_vec();
    if columns.is_empty() {
        return LiveEnd::Failed(on_source(CliError::Query(
            "the query returned no columns, so there is no table to make from it".to_owned(),
        )));
    }
    let Some(types) = live.cursor.column_types().map(<[SqlType]>::to_vec) else {
        return LiveEnd::Failed(on_target(CliError::Query(
            "the source driver did not describe its column types".to_owned(),
        )));
    };
    if types.len() != columns.len() {
        return LiveEnd::Failed(on_target(CliError::Internal(
            "the source described a different number of column types than columns".to_owned(),
        )));
    }
    let pairs: Vec<(&str, &SqlType)> = columns
        .iter()
        .zip(&types)
        .map(|(column, ty)| (&*column.name, ty))
        .collect();
    let table_plan = match plan_table(setup.source_config.kind, setup.target, &pairs) {
        Ok(plan) => plan,
        Err(refusal) => {
            return LiveEnd::Failed(on_target(CliError::Query(refusal.to_string())));
        }
    };
    let target_columns: Vec<Json> = table_plan
        .columns
        .iter()
        .map(|column| json!({ "name": column.name, "type": column.resolved.ddl }))
        .collect();
    let fallbacks: Vec<Json> = table_plan
        .fallbacks()
        .into_iter()
        .map(|change| json!({ "column": change.column, "from": change.from, "to": change.to }))
        .collect();
    let widenings: Vec<String> = table_plan
        .widenings()
        .into_iter()
        .map(|change| {
            format!(
                "column {:?} ({}) is written as {}: {}",
                change.column, change.from, change.to, change.note
            )
        })
        .collect();

    if let Err(error) = dest.begin(table_plan).await {
        return LiveEnd::Failed(error);
    }
    dest.widenings = widenings;
    if let Err(error) = step(out, "write") {
        return LiveEnd::Failed(error);
    }
    let started = event("start")
        .field("columns", columns_json(&columns))
        .field("query_id", live.session.query_id())
        .field("target_columns", target_columns)
        .field("fallbacks", fallbacks)
        .build();
    if let Err(error) = emit(out, started) {
        return LiveEnd::Failed(error);
    }

    let mut rows_read: u64 = 0;
    let mut reported = Instant::now();
    // The first fetch already answered "no more rows": asking a finished cursor again is not
    // something every driver promises to take well.
    let mut ended = next.is_none();
    loop {
        if let Some(batch) = next.take() {
            for index in 0..batch.rows() {
                if cancel.is_cancelled() {
                    return LiveEnd::Stopped { rows_read };
                }
                let values: Vec<&Value> = (0..batch.width())
                    .filter_map(|column| batch.value(index, column))
                    .collect();
                if values.len() != batch.width() {
                    return LiveEnd::Failed(on_source(CliError::Internal(
                        "a batch had a missing cell".to_owned(),
                    )));
                }
                if let Err(error) = dest.append(&values, rows_read + 1).await {
                    return LiveEnd::Failed(error);
                }
                rows_read += 1;
                if reported.elapsed() >= Duration::from_millis(knobs.progress_ms.max(0) as u64) {
                    reported = Instant::now();
                    let progress = event("progress")
                        .field("rows", dest.acked)
                        .field("rows_read", rows_read)
                        .build();
                    if let Err(error) = emit(out, progress) {
                        return LiveEnd::Failed(error);
                    }
                }
            }
        }
        if ended {
            break;
        }
        if cancel.is_cancelled() {
            return LiveEnd::Stopped { rows_read };
        }
        match fetch(&mut live.cursor, knobs.fetch_rows, dest, cancel).await {
            Fetched::Batch(Some(batch)) => next = Some(batch),
            Fetched::Batch(None) => ended = true,
            Fetched::Stopped => return LiveEnd::Stopped { rows_read },
            Fetched::Failed(error) => return LiveEnd::Failed(error),
        }
    }
    if let Err(error) = dest.flush_all().await {
        return LiveEnd::Failed(error);
    }
    LiveEnd::Finished { rows_read }
}

/// The source is finished and closed: check the count, swap, report.
async fn conclude(
    mut dest: Dest<'_>,
    setup: &Setup,
    out: &mut dyn Emitter,
    rows_read: u64,
    query_id: Option<String>,
) -> Result<(), CliError> {
    let progress = event("progress")
        .field("rows", dest.acked)
        .field("rows_read", rows_read)
        .build();
    if let Err(error) = step(out, "verify").and_then(|()| emit(out, progress)) {
        let _ = dest.abort().await;
        return Err(error);
    }
    if dest.acked != rows_read {
        let message = format!(
            "verification failed: {rows_read} rows were read from the source and the destination \
             acknowledged {}; nothing was swapped in",
            dest.acked
        );
        let warnings = dest.abort().await;
        return Err(with_warnings(on_target(CliError::Query(message)), warnings));
    }
    if let Err(error) = step(out, "swap") {
        let _ = dest.abort().await;
        return Err(error);
    }
    // A Stop from here lets the swap finish: the rows are verified and the swap is a few
    // metadata statements.
    let mut warnings = std::mem::take(&mut dest.widenings);
    if let Err(error) = dest.swap(&mut warnings).await {
        let mut left = dest.abort().await;
        warnings.append(&mut left);
        return Err(with_warnings(error, warnings));
    }
    let label = dest
        .prepared
        .as_ref()
        .map(|prepared| prepared.plan.label.clone())
        .unwrap_or_default();
    let acked = dest.acked;
    dest.close().await;
    let mode = match setup.mode {
        TransferMode::Create => "create",
        TransferMode::Replace => "replace",
    };
    emit(
        out,
        event("done")
            .field("rows", acked)
            .field("rows_read", rows_read)
            .field("table", label)
            .field("mode", mode)
            .field("query_id", query_id)
            .field("cancelled", false)
            .field("warnings", warnings)
            .field("verified", true)
            .field("target_kind", setup.dest_config.kind.as_str())
            .build(),
    )
}

/// The `done` of a run a Stop ended: nothing was written to the destination.
fn finish_stopped(
    out: &mut dyn Emitter,
    setup: &Setup,
    rows_read: u64,
    query_id: Option<String>,
    mut warnings: Vec<String>,
) -> Result<(), CliError> {
    warnings.insert(
        0,
        format!(
            "stopped; nothing was written to {}.{}",
            setup.parts[0], setup.parts[1]
        ),
    );
    let mode = match setup.mode {
        TransferMode::Create => "create",
        TransferMode::Replace => "replace",
    };
    emit(
        out,
        event("done")
            .field("rows", 0)
            .field("rows_read", rows_read)
            .field("table", format!("{}.{}", setup.parts[0], setup.parts[1]))
            .field("mode", mode)
            .field("query_id", query_id)
            .field("cancelled", true)
            .field("warnings", warnings)
            .field("verified", false)
            .field("target_kind", setup.dest_config.kind.as_str())
            .build(),
    )
}
