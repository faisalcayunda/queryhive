//! `import_data`: a file into a table, in one of two families.
//!
//! The source study's most valuable split is here: a **row** file (CSV, XLSX,
//! JSON) needs a target table the caller names, and a **statement** file (`.sql`)
//! carries its own. Two families, one set of error modes and one transaction
//! policy, so a `.sql` file and a `.csv` file fail the same way.
//!
//! # Rows
//!
//! The reverse of `export`, and the same rule applies with the arrow turned
//! around: a slice is read, a slice is sent, and the file is never held whole —
//! for CSV. The XLSX reader holds its worksheet because the format leaves no
//! choice, and this module states that in the `done` event rather than promising
//! a bound it cannot keep (see [`qh_import`]'s module note).
//!
//! # JSON
//!
//! A JSON array of objects, or JSONL / concatenated objects, read through
//! [`qh_import::JsonSource`]. It differs from the text formats in one way that
//! matters here: a cell can be a definite NULL (`null`, or a key the object lacks),
//! which is not the same thing as `""`. So `NULL_TEXT` is optional for JSON and
//! unset by default (an empty string stays an empty string), and `IMPORT_PREVIEW=1`
//! lets the app list the file's keys without connecting. Keys that first appear
//! after the rows that named the columns are not imported, and `done.warnings`
//! says so rather than dropping them silently.
//!
//! # Statements
//!
//! A `.sql` file is read whole (up to `IMPORT_SQL_MAX_BYTES`, so one file cannot
//! take the process down) and split by [`qh_sql::statements_with_lines_dialect`] —
//! the same scanner `qh_sql::check` and the classifier use, never a second
//! splitter that could disagree about where a `;` inside a literal or a
//! dollar-quoted body ends a statement. Statements are sent one at a time, each
//! guarded against the connection's Safe Mode, and the statement's own line is
//! in every error message.
//!
//! There is deliberately no `GO` batch separator. `GO` is a client command in
//! SQL Server, not a statement in any driver this build has (PostgreSQL, MySQL,
//! Trino), and honouring it would mean a second scanner with its own rules. A
//! `GO` line is sent as a statement and fails at the server, which is honest.
//!
//! # Transaction safety, and what happens to a bad row or statement
//!
//! A half-applied import is worse than a failed one, so the default is a real
//! transaction and a rollback: `BEGIN`, the statements, `COMMIT` only if every
//! one succeeded. The three modes are named for what they do, not borrowed:
//!
//! | `ON_ERROR` | on a bad row or statement |
//! |---|---|
//! | `stop` (default) | roll back everything and fail. Nothing lands. |
//! | `commit` | keep the work before it and report where it stopped. |
//! | `skip` | skip it, keep going, report it. **No transaction**, because a |
//! | | rollback cannot "skip" a bad row — the same reason the source study gives. |
//!
//! A driver that cannot open a transaction (Trino answers `transactions: false`)
//! cannot roll back, and rather than pretend, `stop` there reports the work
//! already written and says it could not be rolled back. The vocabulary is the
//! study's: `pending` means a transaction is holding the work, `written` means it
//! is already in the table.
//!
//! `commit` commits the prefix only if the prefix is still there. Some servers end
//! the transaction together with the failed statement: PostgreSQL aborts the
//! block on any error (a later `COMMIT` is a silent `ROLLBACK`), and MySQL rolls
//! the whole transaction back on a deadlock. `txn_after` classifies the error,
//! and when the transaction is gone the import fails and says nothing was written
//! instead of reporting a prefix that no longer exists. A lock wait timeout is
//! only a doubt (InnoDB rolls back the statement alone unless
//! `innodb_rollback_on_timeout` is on), so it commits and adds a warning.
//!
//! A row that has no value at all reaching a mapped column is **rejected**, not
//! counted as inserted — the wording the study takes from TablePro, whose own
//! note says an import that counted those used to overstate what landed.
//!
//! # Foreign-key checks
//!
//! `FOREIGN_KEYS=off` turns the server's foreign-key checks off for the duration
//! of the import and back **on at every exit**, commit and rollback alike — the
//! switch is off again on the two paths out, not only the happy one, because a
//! session left with the checks off is the failure the source study names. The
//! default is `on` (leave the server's checks as they are): disabling them is a
//! privileged operation on PostgreSQL and silently permits orphan rows, so it is
//! something the caller asks for. A driver with no session-level switch (Trino)
//! refuses `FOREIGN_KEYS=off` before connecting. `done` reports `foreign_keys`.
//!
//! # Column mapping
//!
//! `COLUMNS` is a JSON array of `{"source": <index>, "target": "<name>",
//! "include": <bool>}`. Absent, it is derived from the header row: every header
//! cell maps to a column of the same name. A file with no header and no
//! `COLUMNS` is a usage error, because there is nothing to map by.
//!
//! The target's own type decides whether a value is written bare (a number or a
//! boolean) or as a quoted literal, and the target is described by the server
//! (`SELECT * FROM <table> LIMIT 0`), not guessed from the file. A driver whose
//! cursor reports no columns before a row arrives (Trino) falls back to treating
//! every value as text, which the server casts; that is stated, not hidden.

use std::path::{Path, PathBuf};
use std::time::Duration;

use qh_core::{ColumnMeta, EngineError};
use qh_driver::{DriverKind, ExecuteOptions, Session};
use qh_import::{
    Format as SourceFormat, ImportError, JsonRow, JsonSource, Options as ReadOptions, RowReader,
};
use qh_sql::{quote_ident, Dialect, IdentStyle, SafeMode, StatementKind};
use serde_json::{json, Value as Json};

use crate::commands::{
    connection, dialect, expand_user, guard, open, record_kind, safe_mode, statement_timeout,
};
use crate::env::Settings;
use crate::events::{event, Emitter};
use crate::progress::{Progress, PROGRESS_MS_DEFAULT};
use crate::sql_ident::{qualified, reference, SlotStyle};
use crate::{CancelFlag, CliError, Engine};

/// Rows per multi-row `INSERT`, and therefore per round trip.
///
/// The `sql` export writer uses 200 for the same reason: several rows per
/// statement is fast without building one enormous statement. The batch is also
/// the unit a `stop` mode reports its failure against.
pub const IMPORT_BATCH: usize = 200;

/// How many bad-row messages are kept. From TablePro's own cap: a file that is
/// wrong everywhere must not turn its error list into the memory problem.
pub const MAX_ERRORS: usize = 1_000;

/// The largest `.sql` file `import_data` reads into memory, unless the caller
/// sets `IMPORT_SQL_MAX_BYTES`. The file is read whole because splitting needs it
/// whole, so this bound is the process's guard against one huge file (R-8).
pub const IMPORT_SQL_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// How many rows `IMPORT_PREVIEW` shows unless `IMPORT_PREVIEW_ROWS` says
/// otherwise, and the most it will ever show.
pub const IMPORT_PREVIEW_ROWS: usize = 5;
const IMPORT_PREVIEW_MAX: usize = 1_000;

/// How many unmapped JSON keys `qh_import` remembers; a list this long may have
/// been cut, and the warning says so.
const UNMAPPED_KEYS_KEPT: usize = 20;

/// Cells of one data row; `None` is a definite NULL (JSON only).
type Cells = Vec<Option<String>>;

/// A row file's format: CSV or XLSX through [`RowReader`], or JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowFormat {
    Table(SourceFormat),
    Json,
}

impl RowFormat {
    const fn name(self) -> &'static str {
        match self {
            RowFormat::Table(format) => format.name(),
            RowFormat::Json => "json",
        }
    }
}

/// The two families a source file belongs to, after the source study: a **row**
/// file that fills a table the caller names, and a **statement** file that brings
/// its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Rows(RowFormat),
    Statements,
}

/// A row file open for reading, whichever format it is.
enum Rows {
    Table(RowReader),
    Json(JsonSource),
}

impl Rows {
    fn open(path: &Path, format: RowFormat, options: &ReadOptions) -> Result<Self, ImportError> {
        Ok(match format {
            RowFormat::Table(format) => Rows::Table(RowReader::open(path, format, options)?),
            RowFormat::Json => Rows::Json(JsonSource::open(path, options)?),
        })
    }

    fn header(&self) -> Option<&[String]> {
        match self {
            Rows::Table(reader) => reader.header(),
            Rows::Json(source) => source.header(),
        }
    }

    fn next_row(&mut self) -> Result<Option<JsonRow>, ImportError> {
        Ok(match self {
            // A text cell is always a value: whether it means NULL is `NULL_TEXT`'s call.
            Rows::Table(reader) => reader.next_row()?.map(|row| JsonRow {
                line: row.line,
                cells: row.cells.into_iter().map(Some).collect(),
            }),
            Rows::Json(source) => source.next_row()?,
        })
    }

    fn streams(&self) -> bool {
        match self {
            Rows::Table(reader) => reader.streams(),
            Rows::Json(_) => true,
        }
    }

    /// Keys the reader met only after it had named the columns, so never imported.
    fn unmapped_keys(&self) -> &[String] {
        match self {
            Rows::Table(_) => &[],
            Rows::Json(source) => source.unmapped_keys(),
        }
    }
}

/// What a bad row does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Stop,
    Commit,
    Skip,
}

impl Mode {
    fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "stop" | "stopandrollback" | "rollback" => Some(Mode::Stop),
            "commit" | "stopandcommit" => Some(Mode::Commit),
            "skip" | "skipandcontinue" => Some(Mode::Skip),
            _ => None,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Mode::Stop => "stop",
            Mode::Commit => "commit",
            Mode::Skip => "skip",
        }
    }
}

/// One included field: where it reads from, resolved to its target column.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    /// The index of the source cell it reads.
    source: usize,
    /// The target column's name, as the server has it (or as the caller typed).
    name: String,
    /// The target column's name, quoted for the driver.
    sql_name: String,
    /// The target column's declared type, or empty when the server did not say.
    type_name: String,
}

/// The import's plan: the table, the resolved targets, and how NULL is spelled.
#[derive(Debug, Clone)]
struct Plan {
    table_sql: String,
    table_reference: String,
    targets: Vec<Target>,
    /// The text that means NULL. `Some("")` for CSV and XLSX, where an empty cell
    /// is NULL; `None` for JSON unless the caller set `NULL_TEXT`, because JSON
    /// has a real `null` and `""` is a value.
    null_text: Option<String>,
    /// Whether an empty cell counts as no value at all. True for the text formats,
    /// which have no other way to say "nothing here"; false for JSON.
    blank_is_empty: bool,
    style: SlotStyle,
}

/// `import_data`: read a file into a table, as rows or as statements.
///
/// The family is decided from `IMPORT_FORMAT`, or from the file's extension when
/// that is absent, before anything is opened — so a `.sql` file with a
/// `TARGET_TABLE` is read as statements (and the table is ignored), and a `.csv`
/// file with no `TARGET_TABLE` is refused without a connection.
pub async fn import_data(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
    cancel: &CancelFlag,
) -> Result<(), CliError> {
    let path = source_path(settings)?;
    let source = source_of(settings, &path)?;
    // A preview opens the file and never the connection, so it is decided first: the
    // sheet asks for it before the user has chosen a target.
    if settings.flag("IMPORT_PREVIEW", false) {
        return match source {
            Source::Rows(RowFormat::Json) => preview_json(settings, out, &path),
            _ => Err(CliError::Usage(
                "IMPORT_PREVIEW is only offered for JSON files".to_owned(),
            )),
        };
    }
    match source {
        Source::Rows(format) => import_rows_file(settings, out, engine, cancel, path, format).await,
        Source::Statements => import_statements(settings, out, engine, cancel, path).await,
    }
}

/// `IMPORT_PREVIEW=1` on a JSON file: the keys it names and a few rows, with no
/// connection. `columns` carries the keys in first-seen order (every one is
/// `text`: the target decides the real type), `rows` the first
/// `IMPORT_PREVIEW_ROWS` rows with `null` for NULL, and `done` closes the run.
fn preview_json(settings: &Settings, out: &mut dyn Emitter, path: &Path) -> Result<(), CliError> {
    let wanted =
        usize::try_from(settings.number("IMPORT_PREVIEW_ROWS", IMPORT_PREVIEW_ROWS as i64)?)
            .ok()
            .filter(|rows| *rows > 0)
            .map_or(IMPORT_PREVIEW_ROWS, |rows| rows.min(IMPORT_PREVIEW_MAX));
    let mut source = JsonSource::open(path, &ReadOptions::default())?;
    let columns: Vec<Json> = source
        .header()
        .unwrap_or_default()
        .iter()
        .map(|name| json!({ "name": name, "type": "text" }))
        .collect();
    let mut rows = Vec::new();
    while rows.len() < wanted {
        let Some(row) = source.next_row()? else { break };
        rows.push(Json::Array(
            row.cells
                .into_iter()
                .map(|cell| cell.map_or(Json::Null, Json::String))
                .collect(),
        ));
    }
    out.emit(event("columns").field("columns", columns).build())?;
    if !rows.is_empty() {
        out.emit(event("rows").field("data", rows).build())?;
    }
    out.emit(
        event("done")
            .field("format", RowFormat::Json.name())
            .field("streams", true)
            .build(),
    )?;
    Ok(())
}

/// How an import treats the server's foreign-key checks.
#[derive(Debug, Clone, Copy)]
enum FkChecks {
    /// Do not touch them: the server enforces its own constraints throughout.
    Enforce,
    /// Turn them off for the duration, and back on at the close.
    Disable { off: &'static str, on: &'static str },
}

impl FkChecks {
    /// What `done` reports: the state the import asked for, not what it observed.
    const fn label(self) -> &'static str {
        match self {
            FkChecks::Enforce => "on",
            FkChecks::Disable { .. } => "off",
        }
    }
}

/// The statements that switch a driver's foreign-key checks off and on, or `None`
/// when the driver has no session-level switch.
///
/// PostgreSQL has no foreign-key-only switch: its standard answer is
/// `session_replication_role = replica`, which turns off *all* triggers and rules
/// for the session and needs a superuser or `SET` privilege on the parameter. That
/// is recorded rather than hidden — it is the reason the default is `FOREIGN_KEYS=on`
/// and the reason `off` is refused, not softened, when the server says no.
/// MySQL is `FOREIGN_KEY_CHECKS`; Trino has nothing of the sort.
fn foreign_key_switch(kind: DriverKind) -> Option<(&'static str, &'static str)> {
    match kind {
        DriverKind::Postgres => Some((
            "SET session_replication_role = replica",
            "SET session_replication_role = DEFAULT",
        )),
        DriverKind::Mysql => Some(("SET FOREIGN_KEY_CHECKS = 0", "SET FOREIGN_KEY_CHECKS = 1")),
        DriverKind::Trino => None,
    }
}

/// Resolve `FOREIGN_KEYS` against the driver, before a connection is opened.
///
/// `on` is the default and means "leave the server alone". `off` is a request the
/// caller makes explicitly, and a driver that cannot honour it refuses the import
/// by name rather than running with checks on under a name that said off.
fn fk_checks(settings: &Settings, kind: DriverKind) -> Result<FkChecks, CliError> {
    if settings.flag("FOREIGN_KEYS", true) {
        return Ok(FkChecks::Enforce);
    }
    match foreign_key_switch(kind) {
        Some((off, on)) => Ok(FkChecks::Disable { off, on }),
        None => Err(CliError::Usage(format!(
            "FOREIGN_KEYS=off is not supported by {kind}: it has no session-level foreign-key \
             switch"
        ))),
    }
}

/// What a failed statement did to the transaction it ran in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Txn {
    /// Untouched: the statement never reached the server, or the server undid only that statement.
    Alive,
    /// The server ended the transaction with the statement, so everything before it is gone too.
    Gone,
    /// The server may have ended it, depending on a setting this engine does not read.
    Doubtful,
}

/// Classify a failed statement by what the driver and the server do next.
///
/// * PostgreSQL aborts the whole transaction block on any error the server
///   answers with, and from then on `COMMIT` is a silent `ROLLBACK`. An error
///   with no code never came back from the server (the connection failed), so
///   `COMMIT` will fail loudly and there is nothing to classify.
/// * MySQL rolls the whole transaction back on a deadlock (1213), but only the
///   statement on a lock wait timeout (1205) unless `innodb_rollback_on_timeout`
///   is on, which is why that one is a doubt and not a verdict.
fn txn_after(kind: DriverKind, error: &EngineError) -> Txn {
    match (kind, error) {
        (
            DriverKind::Postgres,
            EngineError::Query { code: Some(_), .. } | EngineError::Timeout { .. },
        ) => Txn::Gone,
        (
            DriverKind::Mysql,
            EngineError::Query {
                code: Some(code), ..
            },
        ) => match code.as_str() {
            "1213" => Txn::Gone,
            "1205" => Txn::Doubtful,
            _ => Txn::Alive,
        },
        _ => Txn::Alive,
    }
}

/// A failed statement: what to tell the user, and what it did to the transaction.
struct Failure {
    message: String,
    after: Txn,
}

impl Failure {
    /// A failure that never reached the server (a guard refusal), so the transaction is as it was.
    fn local(error: &CliError) -> Self {
        Failure {
            message: error.message(),
            after: Txn::Alive,
        }
    }

    fn server(kind: DriverKind, error: &EngineError) -> Self {
        Failure {
            message: error.message().to_owned(),
            after: txn_after(kind, error),
        }
    }
}

/// End an import: close its transaction, then put the checks back.
///
/// One function for both exits and both families, because "turn them back on" is
/// exactly the step that is easy to leave half-done: a `COMMIT` that failed, or a
/// `ROLLBACK` on the `stop` path, must still run the epilogue. The restore is
/// attempted even when the transaction statement failed, and the transaction's own
/// error is the one that wins when it did.
async fn finish(
    session: &mut Box<dyn Session>,
    in_transaction: bool,
    commit: bool,
    fk: FkChecks,
    timeout: Option<Duration>,
) -> Result<(), CliError> {
    let closed = if in_transaction {
        let statement = if commit { "COMMIT" } else { "ROLLBACK" };
        run(session, statement, timeout).await.map(|_| ())
    } else {
        Ok(())
    };
    if let FkChecks::Disable { on, .. } = fk {
        let restored = run(session, on, timeout).await.map(|_| ());
        if closed.is_ok() {
            restored?;
        }
    }
    closed
}

/// `import_data` for a CSV or XLSX file: rows into the table the caller names.
async fn import_rows_file(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
    cancel: &CancelFlag,
    path: PathBuf,
    format: RowFormat,
) -> Result<(), CliError> {
    let json = format == RowFormat::Json;
    // --- everything that can be refused before the network is touched ---
    let mode = mode_of(settings)?;
    let config = connection(settings, engine)?;
    let style = SlotStyle::of(config.kind);
    let safe = safe_mode(settings, engine)?;
    let dialect = dialect(settings);
    // Importing writes. A mode that refuses a DML statement refuses the whole
    // import, and that is decided here rather than after a connection: a
    // read-only connection must not even open.
    if let Some(reason) = safe.refusal(StatementKind::Dml) {
        // Log the decision before reporting it. This check precedes any `guard`, so without
        // this a refusal that happens before the import ever connects would be the engine's
        // one decision with no row. There is no statement yet — the file has not been read —
        // so the subject is the file being imported, hashed exactly as a statement would be.
        let subject = path.to_string_lossy();
        record_kind(safe, StatementKind::Dml, &subject)?;
        return Err(CliError::Usage(format!(
            "SAFE_MODE={} refuses import_data: {reason}",
            safe.as_str()
        )));
    }
    let fk = fk_checks(settings, config.kind)?;
    let timeout = statement_timeout(settings)?;
    let batch_size = usize::try_from(settings.number("IMPORT_BATCH", IMPORT_BATCH as i64)?)
        .ok()
        .filter(|size| *size > 0)
        .unwrap_or(IMPORT_BATCH);
    // JSON has a real `null`, so for it NULL_TEXT is opt-in; an empty value is unset.
    let null_text = if json {
        optional(settings.raw("NULL_TEXT", ""))
    } else {
        Some(settings.raw("NULL_TEXT", ""))
    };
    let read_options = ReadOptions {
        delimiter: if json { None } else { delimiter(settings)? },
        header: settings.flag("HEADER", true),
        sheet: optional(settings.raw("SHEET", "")),
    };
    let (table_catalog, table_schema, table) = target_parts(settings);
    let table_sql = qualified(style, &table_catalog, &table_schema, &table);
    if table_sql.is_empty() {
        return Err(CliError::Usage(
            "import_data needs a target table: set TARGET_TABLE (and TARGET_SCHEMA where the \
             driver has one)"
                .to_owned(),
        ));
    }
    let table_reference = reference(style, &table_catalog, &table_schema, &table);

    // Opened before the connection so a file that cannot be read — or has no
    // header to map by — is refused without touching the server. The XLSX arm
    // reads its worksheet here, which is the one place this command is not a
    // stream.
    let mut reader = Rows::open(&path, format, &read_options)?;
    if json {
        // The sheet read this file's keys earlier; a `COLUMNS` that no longer matches
        // them means the file changed under it, and that is refused before connecting.
        let Some(header) = reader.header() else {
            return Err(CliError::Usage(format!(
                "'{}' has no JSON objects with keys: there is nothing to import",
                path.display()
            )));
        };
        check_columns_against(settings, header)?;
    }

    out.emit(event("step").field("step", "connect").build())?;
    let (mut session, _policy) = open(settings, engine, &config).await?;

    let columns = target_columns(&mut session, &table_sql, timeout).await?;
    let plan = plan_rows(
        settings,
        reader.header(),
        &columns,
        style,
        &table_sql,
        &table_reference,
        null_text,
        json,
    )?;
    if let Some(missing) = missing_targets(&plan, &columns) {
        let _ = session.close().await;
        return Err(CliError::Usage(missing));
    }

    out.emit(event("step").field("step", "write").build())?;

    // The switch goes off before `BEGIN` so it is not itself rolled back: the
    // study puts the prologue outside the transaction for the same reason.
    if let FkChecks::Disable { off, .. } = fk {
        if let Err(error) = run(&mut session, off, timeout).await {
            let _ = session.close().await;
            return Err(error);
        }
    }

    let transactional = session.capabilities().transactions;
    let in_transaction = transactional && mode != Mode::Skip;
    if in_transaction {
        // `BEGIN` is the one statement after the switch is off that can fail
        // before the loop, so it restores on its own failing path too.
        if let Err(error) = run(&mut session, "BEGIN", timeout).await {
            let _ = finish(&mut session, false, false, fk, timeout).await;
            let _ = session.close().await;
            return Err(error);
        }
    }

    let mut progress = Progress::new(settings.number("PROGRESS_MS", PROGRESS_MS_DEFAULT)?);
    let outcome = match import_rows(
        &mut session,
        &mut reader,
        &plan,
        mode,
        batch_size,
        timeout,
        in_transaction,
        safe,
        dialect,
        cancel,
        out,
        &mut progress,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            // An unexpected failure (a reader that died mid-file, a statement
            // the server refused) leaves the transaction open exactly when there
            // is one; close it the safe way before reporting, checks included.
            let _ = finish(&mut session, in_transaction, false, fk, timeout).await;
            let _ = session.close().await;
            return Err(error);
        }
    };

    if let Err(error) = finish(&mut session, in_transaction, !outcome.rollback, fk, timeout).await {
        let _ = session.close().await;
        return Err(error);
    }

    if outcome.failed {
        // `stop`: the transaction was rolled back above, and this is an error,
        // not a done — a caller must not read a failed import as a success.
        let message = if outcome.lost {
            format!(
                "import stopped at line {}: the server rolled the whole transaction back with \
                 the failed statement, so ON_ERROR=commit could not keep the earlier rows and \
                 no rows were written to {}; ON_ERROR=skip imports the rows that are valid",
                outcome.stopped_at.unwrap_or(0),
                plan.table_reference
            )
        } else if in_transaction {
            format!(
                "import stopped at line {}: no rows were written to {} (the transaction was \
                 rolled back)",
                outcome.stopped_at.unwrap_or(0),
                plan.table_reference
            )
        } else {
            format!(
                "import stopped at line {}: this driver cannot open a transaction, so the {} \
                 rows already written to {} remain",
                outcome.stopped_at.unwrap_or(0),
                outcome.written,
                plan.table_reference
            )
        };
        let _ = session.close().await;
        return Err(CliError::Warned {
            message,
            warnings: outcome.errors,
        });
    }

    if progress.last() != Some(outcome.written) {
        out.emit(event("progress").field("rows", outcome.written).build())?;
    }

    let errors: Vec<Json> = outcome
        .errors
        .iter()
        .map(|message| Json::from(message.clone()))
        .collect();
    let mut warnings = outcome.warnings;
    warnings.extend(unmapped_warning(reader.unmapped_keys()));
    out.emit(
        event("done")
            .field("rows", outcome.written)
            .field("table", plan.table_reference.clone())
            .field("mode", mode.as_str())
            .field("format", format.name())
            .field("streams", reader.streams())
            .field("transaction", in_transaction)
            .field("disposition", outcome.disposition)
            .field("rejected", outcome.rejected)
            .field("errors", errors)
            .field("errors_truncated", outcome.errors_truncated)
            .field("stopped_at", outcome.stopped_at)
            .field("cancelled", outcome.cancelled)
            .field("foreign_keys", fk.label())
            .field("query_id", session.query_id())
            .maybe("warnings", (!warnings.is_empty()).then_some(warnings))
            .build(),
    )?;
    let _ = session.close().await;
    Ok(())
}

/// `import_data` for a `.sql` file: the statements it holds, one at a time.
///
/// No target table and no column map: the file says what it does. The text is read
/// whole because splitting needs it whole — a statement can span lines and a `;`
/// inside a dollar-quoted body is content — so `done` reports `streams: false`
/// rather than claiming a bound this path cannot keep.
async fn import_statements(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
    cancel: &CancelFlag,
    path: PathBuf,
) -> Result<(), CliError> {
    let mode = mode_of(settings)?;
    let config = connection(settings, engine)?;
    let safe = safe_mode(settings, engine)?;
    // The script is split and classified under the connection's own dialect, so a MySQL
    // `.sql` file's escapes and comments are read the way its server will read them.
    let dialect = dialect(settings);
    let fk = fk_checks(settings, config.kind)?;
    let timeout = statement_timeout(settings)?;

    let unreadable = |error: std::io::Error| {
        CliError::Usage(format!(
            "cannot read IMPORT_PATH '{}': {error}",
            path.display()
        ))
    };
    let limit =
        u64::try_from(settings.number("IMPORT_SQL_MAX_BYTES", IMPORT_SQL_MAX_BYTES as i64)?)
            .ok()
            .filter(|bytes| *bytes > 0)
            .unwrap_or(IMPORT_SQL_MAX_BYTES);
    let size = std::fs::metadata(&path).map_err(unreadable)?.len();
    if size > limit {
        return Err(CliError::Usage(format!(
            "IMPORT_PATH '{}' is {size} bytes, over the {limit}-byte limit (IMPORT_SQL_MAX_BYTES): \
             a .sql file is read whole, so split it or raise the limit",
            path.display()
        )));
    }
    let text = std::fs::read_to_string(&path).map_err(unreadable)?;
    let statements = qh_sql::statements_with_lines_dialect(&text, dialect);
    if statements.is_empty() {
        return Err(CliError::Usage(format!(
            "IMPORT_PATH '{}' has no SQL statements",
            path.display()
        )));
    }
    // The whole script is checked once before the connection opens, so a Safe Mode
    // that refuses one statement refuses the import and names it without touching
    // the server — the same order the row family follows for its DML check.
    guard(safe, &text, dialect)?;

    out.emit(event("step").field("step", "connect").build())?;
    let (mut session, _policy) = open(settings, engine, &config).await?;

    out.emit(event("step").field("step", "write").build())?;

    if let FkChecks::Disable { off, .. } = fk {
        if let Err(error) = run(&mut session, off, timeout).await {
            let _ = session.close().await;
            return Err(error);
        }
    }

    let transactional = session.capabilities().transactions;
    let in_transaction = transactional && mode != Mode::Skip;
    if in_transaction {
        // As in the row family: `BEGIN` can fail after the switch is already off,
        // and the checks must not be left off on that exit either.
        if let Err(error) = run(&mut session, "BEGIN", timeout).await {
            let _ = finish(&mut session, false, false, fk, timeout).await;
            let _ = session.close().await;
            return Err(error);
        }
    }

    let mut progress = Progress::new(settings.number("PROGRESS_MS", PROGRESS_MS_DEFAULT)?);
    let outcome = match import_statements_loop(
        &mut session,
        &statements,
        mode,
        timeout,
        in_transaction,
        config.kind,
        safe,
        dialect,
        cancel,
        out,
        &mut progress,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            let _ = finish(&mut session, in_transaction, false, fk, timeout).await;
            let _ = session.close().await;
            return Err(error);
        }
    };

    if let Err(error) = finish(&mut session, in_transaction, !outcome.rollback, fk, timeout).await {
        let _ = session.close().await;
        return Err(error);
    }

    if outcome.failed {
        let message = if outcome.lost {
            format!(
                "import stopped at line {}: the server rolled the whole transaction back with \
                 the failed statement, so ON_ERROR=commit could not keep the earlier ones and no \
                 statements were applied; ON_ERROR=skip applies the statements that are valid",
                outcome.stopped_at.unwrap_or(0)
            )
        } else if in_transaction {
            format!(
                "import stopped at line {}: no statements were applied (the transaction was \
                 rolled back)",
                outcome.stopped_at.unwrap_or(0)
            )
        } else {
            format!(
                "import stopped at line {}: this driver cannot open a transaction, so the {} \
                 statements already applied remain",
                outcome.stopped_at.unwrap_or(0),
                outcome.written
            )
        };
        let _ = session.close().await;
        return Err(CliError::Warned {
            message,
            warnings: outcome.errors,
        });
    }

    if progress.last() != Some(outcome.written) {
        out.emit(
            event("progress")
                .field("statements", outcome.written)
                .build(),
        )?;
    }

    let errors: Vec<Json> = outcome
        .errors
        .iter()
        .map(|message| Json::from(message.clone()))
        .collect();
    out.emit(
        event("done")
            .field("statements", outcome.written)
            .field("mode", mode.as_str())
            .field("format", "sql")
            .field("streams", false)
            .field("transaction", in_transaction)
            .field("disposition", outcome.disposition)
            .field("errors", errors)
            .field("errors_truncated", outcome.errors_truncated)
            .field("stopped_at", outcome.stopped_at)
            .field("cancelled", outcome.cancelled)
            .field("foreign_keys", fk.label())
            .field("query_id", session.query_id())
            .maybe(
                "warnings",
                (!outcome.warnings.is_empty()).then_some(outcome.warnings),
            )
            .build(),
    )?;
    let _ = session.close().await;
    Ok(())
}

/// The statement loop, with the same three modes and the same running totals the
/// row loop keeps.
#[allow(clippy::too_many_arguments)]
async fn import_statements_loop(
    session: &mut Box<dyn Session>,
    statements: &[qh_sql::ScriptStatement<'_>],
    mode: Mode,
    timeout: Option<Duration>,
    in_transaction: bool,
    kind: DriverKind,
    safe: SafeMode,
    dialect: Dialect,
    cancel: &CancelFlag,
    out: &mut dyn Emitter,
    progress: &mut Progress,
) -> Result<Outcome, CliError> {
    let mut outcome = Outcome {
        disposition: if in_transaction { "pending" } else { "written" },
        ..Outcome::default()
    };

    for statement in statements {
        if cancel.is_cancelled() {
            // A stop keeps what was applied, the same way a cancelled export
            // keeps its file: throwing away finished work is not what Stop means.
            outcome.cancelled = true;
            break;
        }
        // Guarded again here, statement by statement: the door check refused the
        // whole script, and this is the statement that actually runs — the same
        // rule the row family's `send_batch` follows.
        let failure = match guard(safe, statement.text, dialect) {
            Ok(()) => run_engine(session, statement.text, timeout)
                .await
                .map(|_| ())
                .map_err(|error| Failure::server(kind, &error)),
            Err(error) => Err(Failure::local(&error)),
        };
        if let Err(failure) = failure {
            outcome.error(statement.line, failure.message);
            match mode {
                // A failed statement wrote nothing on its own, so there is nothing
                // to skip over: the next one runs.
                Mode::Skip => {}
                // `commit` keeps the prefix and says where it stopped (if it is
                // still there); `stop` rolls it back.
                Mode::Commit | Mode::Stop => {
                    outcome.halt(mode, in_transaction, statement.line, failure.after);
                    return Ok(outcome);
                }
            }
            continue;
        }
        outcome.written += 1;
        progress.emit_count("statements", outcome.written, out)?;
    }

    Ok(outcome)
}

/// The running totals an import reports.
#[derive(Debug, Default)]
struct Outcome {
    written: u64,
    rejected: u64,
    errors: Vec<String>,
    errors_truncated: bool,
    stopped_at: Option<usize>,
    cancelled: bool,
    /// The transaction must be rolled back (`stop`).
    rollback: bool,
    /// The import failed and the caller gets an `error`, not a `done`.
    failed: bool,
    /// `commit` meant to keep the prefix, but the server had already rolled it back.
    lost: bool,
    /// Things the user should hear that do not fail the import.
    warnings: Vec<String>,
    disposition: &'static str,
}

impl Outcome {
    fn error(&mut self, line: usize, message: impl Into<String>) {
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(format!("line {line}: {}", message.into()));
        } else {
            self.errors_truncated = true;
        }
    }

    /// End the import at `line` under `stop` or `commit`.
    ///
    /// `stop` rolls back whatever is open. `commit` keeps the prefix, unless the
    /// failed statement took the transaction down with it: then the prefix is gone,
    /// and the import fails and says so rather than report rows that are not there.
    fn halt(&mut self, mode: Mode, in_transaction: bool, line: usize, after: Txn) {
        let commit = in_transaction && mode == Mode::Commit;
        self.stopped_at = Some(line);
        self.lost = commit && after == Txn::Gone;
        if commit && after == Txn::Doubtful {
            self.warnings.push(format!(
                "line {line}: a lock wait timeout stopped the import; if this server rolls back \
                 the whole transaction on a timeout (innodb_rollback_on_timeout), the rows before \
                 it were not kept"
            ));
        }
        self.failed = mode == Mode::Stop || self.lost;
        self.rollback = in_transaction && self.failed;
    }
}

/// The row loop. Split out so the caller opens and closes the transaction on
/// every path.
#[allow(clippy::too_many_arguments)]
async fn import_rows(
    session: &mut Box<dyn Session>,
    reader: &mut Rows,
    plan: &Plan,
    mode: Mode,
    batch_size: usize,
    timeout: Option<Duration>,
    in_transaction: bool,
    safe: SafeMode,
    dialect: Dialect,
    cancel: &CancelFlag,
    out: &mut dyn Emitter,
    progress: &mut Progress,
) -> Result<Outcome, CliError> {
    let mut outcome = Outcome {
        disposition: if in_transaction { "pending" } else { "written" },
        ..Outcome::default()
    };
    let mut batch: Vec<(usize, Cells)> = Vec::new();

    'read: loop {
        if cancel.is_cancelled() {
            // A stop keeps what was written, the same way a cancelled export
            // keeps its file: throwing away finished work is not what Stop means.
            outcome.cancelled = true;
            break 'read;
        }
        let Some(row) = reader.next_row()? else {
            break 'read;
        };
        if row_is_empty(&row, plan) {
            // A row with nothing in any mapped column is rejected, and what
            // happens next is the mode's decision. The rows already read are sent
            // first, so a `commit` keeps everything before this row.
            outcome.rejected += 1;
            outcome.error(row.line, "no value reached any mapped column");
            if mode != Mode::Skip {
                let mut after = Txn::Alive;
                if let Some(failure) = flush(
                    session,
                    plan,
                    &batch,
                    timeout,
                    safe,
                    dialect,
                    &mut outcome,
                    out,
                    progress,
                )
                .await?
                {
                    outcome.error(line_of(&batch), failure.message);
                    after = failure.after;
                }
                batch.clear();
                outcome.halt(mode, in_transaction, row.line, after);
                return Ok(outcome);
            }
            continue;
        }

        batch.push((row.line, row.cells));
        if mode == Mode::Skip || batch.len() >= batch_size {
            let line = line_of(&batch);
            if let Some(failure) = flush(
                session,
                plan,
                &batch,
                timeout,
                safe,
                dialect,
                &mut outcome,
                out,
                progress,
            )
            .await?
            {
                outcome.error(line, failure.message);
                // In `skip` the statement is the batch; a failed one writes nothing.
                if mode != Mode::Skip {
                    outcome.halt(mode, in_transaction, line, failure.after);
                    return Ok(outcome);
                }
            }
            batch.clear();
        }
    }

    // The tail, and whatever a cancel left behind: both are sent rather than
    // dropped, because a Stop keeps the rows already read.
    if !batch.is_empty() {
        let line = line_of(&batch);
        if let Some(failure) = flush(
            session,
            plan,
            &batch,
            timeout,
            safe,
            dialect,
            &mut outcome,
            out,
            progress,
        )
        .await?
        {
            outcome.error(line, failure.message);
            if mode != Mode::Skip {
                outcome.halt(mode, in_transaction, line, failure.after);
            }
        }
    }

    Ok(outcome)
}

/// Send one batch; `Some(failure)` when the statement failed, with the count
/// already added on success.
#[allow(clippy::too_many_arguments)]
async fn flush(
    session: &mut Box<dyn Session>,
    plan: &Plan,
    batch: &[(usize, Cells)],
    timeout: Option<Duration>,
    safe: SafeMode,
    dialect: Dialect,
    outcome: &mut Outcome,
    out: &mut dyn Emitter,
    progress: &mut Progress,
) -> Result<Option<Failure>, CliError> {
    if batch.is_empty() {
        return Ok(None);
    }
    match send_batch(session, plan, batch, timeout, safe, dialect).await {
        Ok(count) => {
            outcome.written += count;
            progress.emit(outcome.written, out)?;
            Ok(None)
        }
        Err(failure) => Ok(Some(failure)),
    }
}

/// One `INSERT`, and the server's own count checked against what was sent.
async fn send_batch(
    session: &mut Box<dyn Session>,
    plan: &Plan,
    batch: &[(usize, Cells)],
    timeout: Option<Duration>,
    safe: SafeMode,
    dialect: Dialect,
) -> Result<u64, Failure> {
    let statement = insert_statement(plan, batch.iter().map(|(_, cells)| cells));
    // Guarded here as well as at the door: this is the statement that runs, and
    // the same rule `to_table` follows.
    if let Err(error) = guard(safe, &statement, dialect) {
        return Err(Failure::local(&error));
    }
    match run_engine(session, &statement, timeout).await {
        Ok(affected) => {
            let sent = batch.len() as u64;
            if let Some(actual) = affected {
                // For an INSERT the server's count is the rows written, and a
                // mismatch is a real disagreement, not a MySQL no-op update.
                if actual != sent {
                    return Err(Failure {
                        message: format!("the server wrote {actual} row(s) but {sent} were sent"),
                        after: Txn::Alive,
                    });
                }
            }
            Ok(sent)
        }
        Err(error) => Err(Failure::server(plan.style.kind, &error)),
    }
}

fn line_of(batch: &[(usize, Cells)]) -> usize {
    batch.first().map(|(line, _)| *line).unwrap_or(0)
}

/// Read one statement to its end, returning the server's affected-row count.
async fn run(
    session: &mut Box<dyn Session>,
    statement: &str,
    timeout: Option<Duration>,
) -> Result<Option<u64>, CliError> {
    Ok(run_engine(session, statement, timeout).await?)
}

/// [`run`], keeping the driver's own error: its code says what the transaction became.
async fn run_engine(
    session: &mut Box<dyn Session>,
    statement: &str,
    timeout: Option<Duration>,
) -> Result<Option<u64>, EngineError> {
    let mut cursor = session
        .execute(
            statement,
            &ExecuteOptions {
                statement_timeout: timeout,
                ..ExecuteOptions::default()
            },
        )
        .await?;
    while cursor.next_batch(1_000).await?.is_some() {}
    Ok(cursor.affected_rows())
}

/// The target table's columns, as the server describes them.
///
/// `LIMIT 0` asks for the shape and no rows. A driver that reports columns only
/// when a row arrives (Trino) answers nothing here and the caller falls back to
/// text; the module note says so.
async fn target_columns(
    session: &mut Box<dyn Session>,
    table_sql: &str,
    timeout: Option<Duration>,
) -> Result<Vec<ColumnMeta>, CliError> {
    let sql = format!("SELECT * FROM {table_sql} LIMIT 0");
    let mut cursor = session
        .execute(
            &sql,
            &ExecuteOptions {
                statement_timeout: timeout,
                ..ExecuteOptions::default()
            },
        )
        .await?;
    // Driven to its end so a cursor that fills columns on the first batch gets
    // the chance; `LIMIT 0` means the batch is empty and the columns stay empty,
    // which is the Trino case.
    let _ = cursor.next_batch(1).await?;
    Ok(cursor.columns().to_vec())
}

/// The plan, from the reader's header and the caller's mapping.
#[allow(clippy::too_many_arguments)]
fn plan_rows(
    settings: &Settings,
    header: Option<&[String]>,
    columns: &[ColumnMeta],
    style: SlotStyle,
    table_sql: &str,
    table_reference: &str,
    null_text: Option<String>,
    json: bool,
) -> Result<Plan, CliError> {
    let fields = match settings.get("COLUMNS") {
        Some(raw) if !raw.trim().is_empty() => parse_columns(raw)?
            .into_iter()
            .map(|field| (field.source, field.target))
            .collect::<Vec<_>>(),
        _ => {
            let Some(header) = header else {
                return Err(CliError::Usage(
                    "COLUMNS is required when the file has no header row: there is nothing to \
                     map a column name from"
                        .to_owned(),
                ));
            };
            header
                .iter()
                .enumerate()
                .map(|(index, name)| (index, name.clone()))
                .collect()
        }
    };
    if fields.is_empty() {
        return Err(CliError::Usage(
            "no columns to import: COLUMNS is empty and the header has no names".to_owned(),
        ));
    }
    let targets = fields
        .into_iter()
        .map(|(source, name)| resolve(source, &name, style, columns))
        .collect();
    Ok(Plan {
        table_sql: table_sql.to_owned(),
        table_reference: table_reference.to_owned(),
        targets,
        null_text,
        blank_is_empty: !json,
        style,
    })
}

/// One field, with its quoted name and type looked up in the server's answer.
fn resolve(source: usize, name: &str, style: SlotStyle, columns: &[ColumnMeta]) -> Target {
    // Matched case-insensitively, as TablePro matches a field to a column: a
    // header `ID` reaching a column `id` is the same column.
    let type_name = columns
        .iter()
        .find(|column| column.name.eq_ignore_ascii_case(name))
        .map(|column| column.type_name.to_string())
        .unwrap_or_default();
    Target {
        source,
        name: name.to_owned(),
        sql_name: quote_ident(style.style, name),
        type_name,
    }
}

/// One `COLUMNS` entry that is included in the import.
#[derive(Debug, PartialEq, Eq)]
struct Field {
    /// Its position in `COLUMNS`, for messages.
    entry: usize,
    source: usize,
    target: String,
    /// The file's own name for the column, as the sheet saw it. Only JSON checks it.
    name: Option<String>,
}

/// For a JSON file, refuse a `COLUMNS` the file no longer matches.
///
/// The sheet built `COLUMNS` from a preview of this file's keys. If a field names
/// the key it read and the file's column at that position is another one, or the
/// position is past the file's last column, the file changed under the sheet and
/// importing by position would put one key's values in another's column. CSV and
/// XLSX are not checked: the Swift and the engine headers can differ in BOM and
/// quoting without the file having changed.
fn check_columns_against(settings: &Settings, header: &[String]) -> Result<(), CliError> {
    let Some(raw) = settings.get("COLUMNS").filter(|raw| !raw.trim().is_empty()) else {
        return Ok(());
    };
    for field in parse_columns(raw)? {
        match (header.get(field.source), &field.name) {
            (None, _) => {
                return Err(CliError::Usage(format!(
                    "COLUMNS[{}] reads column {} but the file has {} column(s); reload the file \
                     in the sheet",
                    field.entry,
                    field.source,
                    header.len()
                )))
            }
            (Some(seen), Some(name)) if seen != name => {
                return Err(CliError::Usage(format!(
                    "COLUMNS[{}] names '{name}' but the file's column {} is '{seen}'; reload the \
                     file in the sheet",
                    field.entry, field.source
                )))
            }
            _ => {}
        }
    }
    Ok(())
}

/// The `COLUMNS` JSON, or the reason it is not usable.
fn parse_columns(raw: &str) -> Result<Vec<Field>, CliError> {
    let parsed: Json = serde_json::from_str(raw)
        .map_err(|error| CliError::Usage(format!("COLUMNS is not JSON: {error}")))?;
    let rows = parsed
        .as_array()
        .ok_or_else(|| CliError::Usage("COLUMNS must be a JSON array".to_owned()))?;
    let mut fields = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let object = row
            .as_object()
            .ok_or_else(|| CliError::Usage(format!("COLUMNS[{index}] is not an object")))?;
        if object.get("include").and_then(Json::as_bool) == Some(false) {
            continue;
        }
        let source = object
            .get("source")
            .and_then(Json::as_u64)
            .ok_or_else(|| CliError::Usage(format!("COLUMNS[{index}] has no source index")))?;
        let target = object
            .get("target")
            .and_then(Json::as_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| CliError::Usage(format!("COLUMNS[{index}] has no target name")))?;
        let name = match object.get("name") {
            None | Some(Json::Null) => None,
            Some(Json::String(name)) => Some(name.clone()),
            Some(_) => {
                return Err(CliError::Usage(format!(
                    "COLUMNS[{index}] name must be a string"
                )))
            }
        };
        fields.push(Field {
            entry: index,
            source: source as usize,
            target: target.to_owned(),
            name,
        });
    }
    Ok(fields)
}

/// The mapped targets the server does not have, as a usage error naming them.
///
/// `None` when the server answered nothing at all (Trino before a row), because
/// then there is nothing to check against.
fn missing_targets(plan: &Plan, columns: &[ColumnMeta]) -> Option<String> {
    if columns.is_empty() {
        return None;
    }
    let missing: Vec<&str> = plan
        .targets
        .iter()
        .filter(|target| {
            !columns
                .iter()
                .any(|column| column.name.eq_ignore_ascii_case(&target.name))
        })
        .map(|target| target.name.as_str())
        .collect();
    (!missing.is_empty()).then(|| {
        format!(
            "the target table has no column named {}",
            missing.join(", ")
        )
    })
}

/// Whether a row carries no value for any mapped field.
///
/// A `None` cell is a NULL and never a value; so is the caller's `NULL_TEXT`; and
/// for the text formats so is an empty cell.
fn row_is_empty(row: &JsonRow, plan: &Plan) -> bool {
    !plan.targets.iter().any(|target| {
        row.cells
            .get(target.source)
            .and_then(Option::as_deref)
            .is_some_and(|cell| {
                !(plan.blank_is_empty && cell.is_empty()) && Some(cell) != plan.null_text.as_deref()
            })
    })
}

/// One `INSERT` for a batch of rows.
fn insert_statement<'a>(plan: &Plan, rows: impl Iterator<Item = &'a Cells>) -> String {
    let columns = plan
        .targets
        .iter()
        .map(|target| target.sql_name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let tuples = rows
        .map(|row| {
            let values = plan
                .targets
                .iter()
                .map(|target| {
                    // A short text row has no cell there, which reads as blank.
                    let cell = row.get(target.source).map_or(Some(""), Option::as_deref);
                    literal(
                        cell,
                        &target.type_name,
                        plan.null_text.as_deref(),
                        plan.style.style,
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("({values})")
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("INSERT INTO {} ({columns}) VALUES {tuples}", plan.table_sql)
}

/// One value as a SQL literal, by the target column's type. `None` is always NULL.
fn literal(
    cell: Option<&str>,
    type_name: &str,
    null_text: Option<&str>,
    style: IdentStyle,
) -> String {
    let Some(text) = cell else {
        return "NULL".to_owned();
    };
    if Some(text) == null_text {
        return "NULL".to_owned();
    }
    if type_name.is_empty() {
        // The server said nothing about the column (Trino before a row): quote
        // it and let the server cast, which is the only honest answer left.
        return quoted(text, style);
    }
    if is_numeric(type_name) {
        return if is_number(text) {
            text.to_owned()
        } else {
            quoted(text, style)
        };
    }
    if is_boolean(type_name) {
        return match text.trim().to_ascii_lowercase().as_str() {
            "true" | "t" | "1" | "yes" | "y" => "TRUE".to_owned(),
            "false" | "f" | "0" | "no" | "n" => "FALSE".to_owned(),
            _ => quoted(text, style),
        };
    }
    quoted(text, style)
}

/// A quoted string literal, escaped the way the dialect reads it.
fn quoted(text: &str, style: IdentStyle) -> String {
    match style {
        // The SQL standard, which Postgres and Trino follow with
        // `standard_conforming_strings` on: only the quote doubles.
        IdentStyle::Ansi => format!("'{}'", text.replace('\'', "''")),
        // MySQL reads a backslash as an escape, so a backslash that is data has
        // to be doubled as well, before the quote.
        IdentStyle::Mysql => {
            let escaped = text.replace('\\', "\\\\").replace('\'', "''");
            format!("'{escaped}'")
        }
    }
}

/// Whether a type name is one whose literals are written bare.
fn is_numeric(type_name: &str) -> bool {
    let base = base_type(type_name);
    [
        "tinyint",
        "smallint",
        "int",
        "integer",
        "bigint",
        "int2",
        "int4",
        "int8",
        "serial",
        "bigserial",
        "smallserial",
        "real",
        "float",
        "float4",
        "float8",
        "double",
        "double precision",
        "decimal",
        "numeric",
        "number",
    ]
    .contains(&base.as_str())
}

fn is_boolean(type_name: &str) -> bool {
    matches!(base_type(type_name).as_str(), "bool" | "boolean")
}

/// A type name without its width or precision, lower-cased.
fn base_type(type_name: &str) -> String {
    type_name
        .split('(')
        .next()
        .unwrap_or(type_name)
        .trim()
        .to_ascii_lowercase()
}

/// Whether text is an integer or a float, so a numeric column can take it bare.
fn is_number(text: &str) -> bool {
    let trimmed = text.trim();
    !trimmed.is_empty() && (trimmed.parse::<i64>().is_ok() || trimmed.parse::<f64>().is_ok())
}

/// `IMPORT_PATH` or `IMPORT_FILE`, expanded and absolute.
fn source_path(settings: &Settings) -> Result<PathBuf, CliError> {
    let raw = settings.text("IMPORT_PATH", "");
    let raw = if raw.is_empty() {
        settings.text("IMPORT_FILE", "")
    } else {
        raw
    };
    if raw.is_empty() {
        return Err(CliError::Usage(
            "IMPORT_PATH is required: the CSV, XLSX, JSON or SQL file to read".to_owned(),
        ));
    }
    let path = expand_user(&raw);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|error| CliError::Internal(error.to_string()))?
            .join(path)
    };
    if !path.exists() {
        return Err(CliError::Usage(format!(
            "IMPORT_PATH '{}' does not exist",
            path.display()
        )));
    }
    Ok(path)
}

/// The import family and, for rows, the format: `IMPORT_FORMAT` first, the file's
/// extension when it is unset.
///
/// `sql` is its own family rather than a third row format, because a `.sql` file
/// is statements and needs no target table. A spelling nobody knows is refused by
/// name rather than defaulted.
fn source_of(settings: &Settings, path: &Path) -> Result<Source, CliError> {
    let raw = settings.text("IMPORT_FORMAT", "");
    if !raw.trim().is_empty() {
        if raw.trim().eq_ignore_ascii_case("sql") {
            return Ok(Source::Statements);
        }
        if is_json_name(&raw) {
            return Ok(Source::Rows(RowFormat::Json));
        }
        return SourceFormat::parse(&raw)
            .map(|format| Source::Rows(RowFormat::Table(format)))
            .ok_or_else(|| {
                CliError::Usage(format!(
                    "unknown IMPORT_FORMAT '{raw}'; expected csv, xlsx, json or sql"
                ))
            });
    }
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "csv" | "tsv" => Ok(Source::Rows(RowFormat::Table(SourceFormat::Csv))),
        "xlsx" | "xlsm" => Ok(Source::Rows(RowFormat::Table(SourceFormat::Xlsx))),
        "json" | "jsonl" | "ndjson" => Ok(Source::Rows(RowFormat::Json)),
        "sql" => Ok(Source::Statements),
        other => Err(CliError::Usage(format!(
            "cannot tell the format from extension '{other}'; set IMPORT_FORMAT to csv, xlsx, \
             json or sql"
        ))),
    }
}

/// The spellings of JSON: one array of objects, or one object per line.
fn is_json_name(name: &str) -> bool {
    matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "json" | "jsonl" | "ndjson"
    )
}

/// The warning for keys that first appeared after the rows that named the columns.
fn unmapped_warning(keys: &[String]) -> Option<String> {
    if keys.is_empty() {
        return None;
    }
    // A key can be as long as the file allows an element to be; a name that fits a
    // message is enough to find it by.
    let names = keys
        .iter()
        .map(|key| match key.char_indices().nth(80) {
            Some((end, _)) => format!("'{}...'", &key[..end]),
            None => format!("'{key}'"),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let cut = if keys.len() >= UNMAPPED_KEYS_KEPT {
        format!(" (only the first {UNMAPPED_KEYS_KEPT} are listed)")
    } else {
        String::new()
    };
    Some(format!(
        "keys first seen after the rows that named the columns were not imported: {names}{cut}"
    ))
}

/// `ON_ERROR`, refused by name when it is not one of the three.
fn mode_of(settings: &Settings) -> Result<Mode, CliError> {
    let raw = settings.text("ON_ERROR", "stop");
    Mode::parse(&raw).ok_or_else(|| {
        CliError::Usage(format!(
            "unknown ON_ERROR '{raw}'; expected stop, commit or skip"
        ))
    })
}

/// `DELIMITER`, or `\t` for a `.tsv` file.
fn delimiter(settings: &Settings) -> Result<Option<char>, CliError> {
    let raw = settings.raw("DELIMITER", "");
    if raw.is_empty() {
        return Ok(None);
    }
    let mut characters = raw.chars();
    let character = characters
        .next()
        .ok_or_else(|| CliError::Usage("DELIMITER must be one character".to_owned()))?;
    if characters.next().is_some() {
        return Err(CliError::Usage(format!(
            "DELIMITER must be a single character, got '{raw}'"
        )));
    }
    Ok(Some(character))
}

/// The three `TARGET_*` parts, read straight from the settings.
fn target_parts(settings: &Settings) -> (String, String, String) {
    (
        settings.text("TARGET_CATALOG", ""),
        settings.text("TARGET_SCHEMA", ""),
        settings.text("TARGET_TABLE", ""),
    )
}

fn optional(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qh_core::ColumnMeta;

    fn plan(null_text: &str) -> Plan {
        // The text formats: an empty cell is NULL, and NULL_TEXT defaults to it.
        plan_with(Some(null_text), true)
    }

    fn plan_with(null_text: Option<&str>, blank_is_empty: bool) -> Plan {
        Plan {
            table_sql: "\"public\".\"people\"".to_owned(),
            table_reference: "public.people".to_owned(),
            targets: vec![
                Target {
                    source: 0,
                    name: "id".to_owned(),
                    sql_name: "\"id\"".to_owned(),
                    type_name: "int8".to_owned(),
                },
                Target {
                    source: 1,
                    name: "name".to_owned(),
                    sql_name: "\"name\"".to_owned(),
                    type_name: "text".to_owned(),
                },
            ],
            null_text: null_text.map(str::to_owned),
            blank_is_empty,
            style: SlotStyle::of(qh_driver::DriverKind::Postgres),
        }
    }

    /// A text-format cell: always a value, and `null_text` is always set.
    fn text(cell: &str, type_name: &str, null_text: &str, style: IdentStyle) -> String {
        literal(Some(cell), type_name, Some(null_text), style)
    }

    #[test]
    fn a_number_is_bare_and_text_is_quoted() {
        // Bare for numeric target types, because a bare number is what a numeric
        // column takes and quoting it is a type mismatch on a strict server.
        assert_eq!(text("42", "int8", "", IdentStyle::Ansi), "42");
        assert_eq!(text("-1.5", "float8", "", IdentStyle::Ansi), "-1.5");
        assert_eq!(text("hello", "text", "", IdentStyle::Ansi), "'hello'");
        // A non-number in a numeric column is quoted, not written bare: the
        // server's refusal, in a transaction, is a rollback and not broken SQL.
        assert_eq!(text("abc", "int8", "", IdentStyle::Ansi), "'abc'");
    }

    #[test]
    fn a_quote_is_doubled_and_mysql_also_doubles_a_backslash() {
        assert_eq!(text("O'Brien", "text", "", IdentStyle::Ansi), "'O''Brien'");
        // Postgres reads a backslash literally, so it is not doubled there.
        assert_eq!(text(r"a\b", "text", "", IdentStyle::Ansi), r"'a\b'");
        // MySQL reads it as an escape, so it is.
        assert_eq!(text(r"a\b", "text", "", IdentStyle::Mysql), r"'a\\b'");
    }

    #[test]
    fn the_null_text_is_null_and_nothing_else_is() {
        assert_eq!(text("", "text", "", IdentStyle::Ansi), "NULL");
        assert_eq!(text("NA", "text", "NA", IdentStyle::Ansi), "NULL");
        // The literal string "NA" is data when the null text is something else.
        assert_eq!(text("NA", "text", "\\N", IdentStyle::Ansi), "'NA'");
    }

    #[test]
    fn a_json_null_is_null_and_an_empty_string_is_a_value() {
        // No `NULL_TEXT`: `null` (a `None` cell) is NULL, and `""` is the empty string.
        assert_eq!(literal(None, "text", None, IdentStyle::Ansi), "NULL");
        assert_eq!(literal(Some(""), "text", None, IdentStyle::Ansi), "''");
        // A `None` cell is NULL even when the null text is empty, and never a literal.
        assert_eq!(literal(None, "int8", Some(""), IdentStyle::Ansi), "NULL");
        // An explicit `NULL_TEXT` turns that string into NULL for JSON too.
        assert_eq!(
            literal(Some("n/a"), "text", Some("n/a"), IdentStyle::Ansi),
            "NULL"
        );
        // A long number and a trailing zero reach the server as written.
        let long = "123456789012345678901234567890";
        assert_eq!(literal(Some(long), "numeric", None, IdentStyle::Ansi), long);
        assert_eq!(
            literal(Some("1.10"), "numeric", None, IdentStyle::Ansi),
            "1.10"
        );
    }

    #[test]
    fn booleans_become_keywords_not_numbers() {
        assert_eq!(text("true", "boolean", "", IdentStyle::Ansi), "TRUE");
        assert_eq!(text("0", "bool", "", IdentStyle::Ansi), "FALSE");
        // Bare `1` is not a boolean on every server, so it is mapped, not passed.
        assert_eq!(text("1", "bool", "", IdentStyle::Ansi), "TRUE");
    }

    #[test]
    fn a_batch_becomes_one_statement_with_one_tuple_per_row() {
        let plan = plan("");
        let rows = [
            vec![Some("1".to_owned()), Some("a".to_owned())],
            vec![Some("2".to_owned()), None],
        ];
        let sql = insert_statement(&plan, rows.iter());
        assert_eq!(
            sql,
            "INSERT INTO \"public\".\"people\" (\"id\", \"name\") VALUES (1, 'a'), (2, NULL)"
        );
    }

    #[test]
    fn a_row_with_no_mapped_value_is_rejected() {
        let plan = plan("");
        let row = |cells: Vec<Option<&str>>| JsonRow {
            line: 3,
            cells: cells.into_iter().map(|c| c.map(str::to_owned)).collect(),
        };
        assert!(row_is_empty(&row(vec![Some(""), Some("")]), &plan));
        assert!(!row_is_empty(&row(vec![Some(""), Some("x")]), &plan));
        // A row shorter than the mapping is treated as empty for the missing
        // cells, which is what a short CSV line is.
        assert!(row_is_empty(&row(vec![Some("")]), &plan));
    }

    #[test]
    fn a_json_row_is_empty_only_when_every_mapped_cell_is_null() {
        let plan = plan_with(None, false);
        let row = |cells: Vec<Option<&str>>| JsonRow {
            line: 1,
            cells: cells.into_iter().map(|c| c.map(str::to_owned)).collect(),
        };
        assert!(row_is_empty(&row(vec![None, None]), &plan));
        // `""` is a value in JSON, so a row that holds only that is not rejected.
        assert!(!row_is_empty(&row(vec![None, Some("")]), &plan));
        // An explicit NULL_TEXT makes that string a NULL, so it is not a value either.
        let marked = plan_with(Some("n/a"), false);
        assert!(row_is_empty(&row(vec![Some("n/a"), None]), &marked));
        assert!(!row_is_empty(&row(vec![Some("n/a"), Some("x")]), &marked));
    }

    #[test]
    fn the_column_mapping_parses_and_skips_excluded_fields() {
        let raw = r#"[{"source":0,"target":"id"},{"source":1,"target":"skip","include":false},{"source":2,"target":"name"}]"#;
        let fields = parse_columns(raw).expect("a map");
        let pairs: Vec<_> = fields
            .iter()
            .map(|field| (field.source, field.target.as_str()))
            .collect();
        assert_eq!(pairs, vec![(0, "id"), (2, "name")]);
        // The position in COLUMNS survives an excluded entry, for messages.
        assert_eq!(fields[1].entry, 2);
        assert!(parse_columns("not json").is_err());
        assert!(parse_columns(r#"[{"target":"id"}]"#).is_err());
        assert!(parse_columns(r#"[{"source":0,"target":"id","name":3}]"#).is_err());
    }

    #[test]
    fn a_json_columns_map_is_checked_against_the_files_own_keys() {
        let settings = |columns: &str| Settings::from_pairs([("COLUMNS", columns)]);
        let header = ["id".to_owned(), "mail".to_owned()];
        // Names that match, and entries with no name, pass.
        let fine = r#"[{"source":0,"target":"id","name":"id"},{"source":1,"target":"email"}]"#;
        assert!(check_columns_against(&settings(fine), &header).is_ok());
        // A name that is not the file's key at that position is refused, by entry.
        let stale = r#"[{"source":0,"target":"id","name":"id"},{"source":1,"target":"email","name":"email"}]"#;
        let error = check_columns_against(&settings(stale), &header).unwrap_err();
        assert_eq!(
            error.message(),
            "COLUMNS[1] names 'email' but the file's column 1 is 'mail'; reload the file in the sheet"
        );
        // A position past the last key is refused whether or not it carries a name.
        let past = r#"[{"source":2,"target":"x"}]"#;
        let error = check_columns_against(&settings(past), &header).unwrap_err();
        assert!(error.message().contains("column 2"), "{error:?}");
        // An excluded entry is not part of the import, so it is not checked.
        let excluded = r#"[{"source":9,"target":"x","include":false}]"#;
        assert!(check_columns_against(&settings(excluded), &header).is_ok());
        // No COLUMNS: the plan comes from the header itself, so there is nothing to check.
        assert!(check_columns_against(&Settings::from_pairs([("X", "y")]), &header).is_ok());
    }

    #[test]
    fn a_mapped_target_the_server_does_not_have_is_named() {
        let plan = plan("");
        let columns = vec![ColumnMeta::new("id", "int8")];
        let message = missing_targets(&plan, &columns).expect("name must be reported");
        assert!(message.contains("name"), "{message}");
        // Nothing to check against when the driver has not answered yet.
        assert_eq!(missing_targets(&plan, &[]), None);
    }

    #[test]
    fn the_three_modes_parse_and_anything_else_is_refused() {
        assert_eq!(Mode::parse("stop"), Some(Mode::Stop));
        assert_eq!(Mode::parse("stopAndRollback"), Some(Mode::Stop));
        assert_eq!(Mode::parse("commit"), Some(Mode::Commit));
        assert_eq!(Mode::parse("skipAndContinue"), Some(Mode::Skip));
        assert_eq!(Mode::parse("maybe"), None);
    }

    #[test]
    fn a_sql_extension_is_its_own_family_and_format_wins_over_it() {
        let settings = |pairs: &[(&str, &str)]| Settings::from_pairs(pairs.iter().copied());
        assert_eq!(
            source_of(&settings(&[]), Path::new("/tmp/load.sql")).ok(),
            Some(Source::Statements)
        );
        let csv = Source::Rows(RowFormat::Table(SourceFormat::Csv));
        assert_eq!(
            source_of(&settings(&[]), Path::new("/tmp/load.CSV")).ok(),
            Some(csv)
        );
        // `IMPORT_FORMAT` decides when it is set, even against the extension.
        assert_eq!(
            source_of(
                &settings(&[("IMPORT_FORMAT", "sql")]),
                Path::new("/tmp/rows.csv")
            )
            .ok(),
            Some(Source::Statements)
        );
        assert_eq!(
            source_of(
                &settings(&[("IMPORT_FORMAT", " XLSX ")]),
                Path::new("/tmp/load.sql")
            )
            .ok(),
            Some(Source::Rows(RowFormat::Table(SourceFormat::Xlsx)))
        );
        // JSON by extension (all three spellings) and by `IMPORT_FORMAT`, which wins.
        let json = Some(Source::Rows(RowFormat::Json));
        for file in ["/tmp/a.json", "/tmp/a.JSONL", "/tmp/a.ndjson"] {
            assert_eq!(source_of(&settings(&[]), Path::new(file)).ok(), json);
        }
        assert_eq!(
            source_of(
                &settings(&[("IMPORT_FORMAT", "jsonl")]),
                Path::new("/tmp/a.txt")
            )
            .ok(),
            json
        );
        // A spelling nobody knows is refused by name, not defaulted.
        let error = source_of(
            &settings(&[("IMPORT_FORMAT", "parquet")]),
            Path::new("/tmp/load.csv"),
        )
        .unwrap_err();
        assert!(
            error.message().contains("csv, xlsx, json or sql"),
            "{error:?}"
        );
        let unknown = source_of(&settings(&[]), Path::new("/tmp/load.txt")).unwrap_err();
        assert!(unknown.message().contains("extension"), "{unknown:?}");
    }

    #[test]
    fn foreign_key_checks_follow_the_driver_and_the_setting() {
        let settings = |pairs: &[(&str, &str)]| Settings::from_pairs(pairs.iter().copied());
        // Default is on: the server is left alone.
        assert!(matches!(
            fk_checks(&settings(&[]), DriverKind::Postgres),
            Ok(FkChecks::Enforce)
        ));
        // `off` on a driver with a switch becomes the pair that turns it off and
        // back on.
        match fk_checks(&settings(&[("FOREIGN_KEYS", "off")]), DriverKind::Postgres) {
            Ok(FkChecks::Disable { off, on }) => {
                assert!(off.contains("replica"), "{off}");
                assert!(on.contains("DEFAULT"), "{on}");
            }
            other => panic!("expected a disable policy, got {other:?}"),
        }
        match fk_checks(&settings(&[("FOREIGN_KEYS", "off")]), DriverKind::Mysql) {
            Ok(FkChecks::Disable { off, on }) => {
                assert_eq!(off, "SET FOREIGN_KEY_CHECKS = 0");
                assert_eq!(on, "SET FOREIGN_KEY_CHECKS = 1");
            }
            other => panic!("expected a disable policy, got {other:?}"),
        }
        // A driver with no switch refuses rather than running with checks on under
        // a name that said off.
        let refused = fk_checks(&settings(&[("FOREIGN_KEYS", "off")]), DriverKind::Trino);
        assert!(refused.is_err(), "{refused:?}");
        // The label is the state the caller asked for.
        assert_eq!(FkChecks::Enforce.label(), "on");
        assert_eq!(FkChecks::Disable { off: "x", on: "y" }.label(), "off");
    }
    fn query_error(code: Option<&str>) -> EngineError {
        EngineError::Query {
            message: "the server said no".to_owned(),
            code: code.map(str::to_owned),
            position: None,
            kind: qh_core::FailureKind::Permanent,
        }
    }

    #[test]
    fn a_failed_statement_is_classified_by_what_the_server_does_to_the_transaction() {
        let pg = DriverKind::Postgres;
        let my = DriverKind::Mysql;
        // PostgreSQL aborts the block on any error the server answers with.
        assert_eq!(txn_after(pg, &query_error(Some("23505"))), Txn::Gone);
        assert_eq!(
            txn_after(
                pg,
                &EngineError::Timeout {
                    message: "late".to_owned(),
                    limit_ms: None
                }
            ),
            Txn::Gone
        );
        // No code: the connection failed, `COMMIT` will say so, nothing to classify.
        assert_eq!(txn_after(pg, &query_error(None)), Txn::Alive);
        // MySQL: a deadlock takes the whole transaction, a duplicate key only the statement.
        assert_eq!(txn_after(my, &query_error(Some("1213"))), Txn::Gone);
        assert_eq!(txn_after(my, &query_error(Some("1062"))), Txn::Alive);
        // A lock wait timeout depends on `innodb_rollback_on_timeout`.
        assert_eq!(txn_after(my, &query_error(Some("1205"))), Txn::Doubtful);
        // Trino has no transaction to lose.
        assert_eq!(
            txn_after(DriverKind::Trino, &query_error(Some("1"))),
            Txn::Alive
        );
    }

    #[test]
    fn commit_keeps_the_prefix_unless_the_server_took_it_back() {
        let mut kept = Outcome::default();
        kept.halt(Mode::Commit, true, 7, Txn::Alive);
        assert!(
            (kept.stopped_at, kept.failed, kept.rollback, kept.lost)
                == (Some(7), false, false, false)
        );

        // The deadlock case (B-13 N2): the import fails and rolls back, it does not report a prefix.
        let mut gone = Outcome::default();
        gone.halt(Mode::Commit, true, 7, Txn::Gone);
        assert!((gone.failed, gone.rollback, gone.lost) == (true, true, true));

        // A doubt commits, as before, and says so.
        let mut doubtful = Outcome::default();
        doubtful.halt(Mode::Commit, true, 7, Txn::Doubtful);
        assert!(!doubtful.failed && !doubtful.lost);
        assert_eq!(doubtful.warnings.len(), 1);

        // `stop` always rolls back; without a transaction (Trino) it fails with nothing to roll back.
        let mut stop = Outcome::default();
        stop.halt(Mode::Stop, true, 7, Txn::Alive);
        assert!((stop.failed, stop.rollback, stop.lost) == (true, true, false));
        let mut bare = Outcome::default();
        bare.halt(Mode::Stop, false, 7, Txn::Gone);
        assert!((bare.failed, bare.rollback, bare.lost) == (true, false, false));
        // `commit` with no transaction has no prefix to lose.
        let mut free = Outcome::default();
        free.halt(Mode::Commit, false, 7, Txn::Gone);
        assert!((free.failed, free.lost) == (false, false));
    }

    #[test]
    fn unmapped_keys_are_named_and_a_long_key_is_cut() {
        assert_eq!(unmapped_warning(&[]), None);
        let one = unmapped_warning(&["late".to_owned()]).expect("a warning");
        assert!(one.ends_with("not imported: 'late'"), "{one}");
        let long = "k".repeat(200);
        let cut = unmapped_warning(&[long]).expect("a warning");
        assert!(cut.len() < 200, "{cut}");
        assert!(cut.contains("...'"), "{cut}");
        // A list at the reader's cap may have been cut, and says so.
        let many: Vec<String> = (0..UNMAPPED_KEYS_KEPT).map(|i| format!("k{i}")).collect();
        let capped = unmapped_warning(&many).expect("a warning");
        assert!(capped.contains("only the first 20"), "{capped}");
    }
}
