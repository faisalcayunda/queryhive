//! `import_data`: a file into a table, in one of two families.
//!
//! The source study's most valuable split is here: a **row** file (CSV, XLSX)
//! needs a target table the caller names, and a **statement** file (`.sql`)
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
//! # Statements
//!
//! A `.sql` file is read whole and split by [`qh_sql::statements_with_lines`] —
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

use qh_core::ColumnMeta;
use qh_driver::{DriverKind, ExecuteOptions, Session};
use qh_import::{Format as SourceFormat, Options as ReadOptions, RawRow, RowReader};
use qh_sql::{quote_ident, IdentStyle, SafeMode, StatementKind};
use serde_json::Value as Json;

use crate::commands::{
    connection, expand_user, guard, open, record_kind, safe_mode, statement_timeout,
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

/// The two families a source file belongs to, after the source study: a **row**
/// file that fills a table the caller names, and a **statement** file that brings
/// its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Rows(SourceFormat),
    Statements,
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
    null_text: String,
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
    match source_of(settings, &path)? {
        Source::Rows(format) => import_rows_file(settings, out, engine, cancel, path, format).await,
        Source::Statements => import_statements(settings, out, engine, cancel, path).await,
    }
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
    format: SourceFormat,
) -> Result<(), CliError> {
    // --- everything that can be refused before the network is touched ---
    let mode = mode_of(settings)?;
    let config = connection(settings, engine)?;
    let style = SlotStyle::of(config.kind);
    let safe = safe_mode(settings)?;
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
    let null_text = settings.raw("NULL_TEXT", "");
    let read_options = ReadOptions {
        delimiter: delimiter(settings)?,
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
    let mut reader = RowReader::open(&path, format, &read_options)?;

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
        let message = if in_transaction {
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
    let safe = safe_mode(settings)?;
    let fk = fk_checks(settings, config.kind)?;
    let timeout = statement_timeout(settings)?;

    let text = std::fs::read_to_string(&path).map_err(|error| {
        CliError::Usage(format!(
            "cannot read IMPORT_PATH '{}': {error}",
            path.display()
        ))
    })?;
    let statements = qh_sql::statements_with_lines(&text);
    if statements.is_empty() {
        return Err(CliError::Usage(format!(
            "IMPORT_PATH '{}' has no SQL statements",
            path.display()
        )));
    }
    // The whole script is checked once before the connection opens, so a Safe Mode
    // that refuses one statement refuses the import and names it without touching
    // the server — the same order the row family follows for its DML check.
    guard(safe, &text)?;

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
        safe,
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
        let message = if in_transaction {
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
    safe: SafeMode,
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
        let failure = match guard(safe, statement.text) {
            Ok(()) => run(session, statement.text, timeout).await.map(|_| ()),
            Err(error) => Err(error),
        };
        if let Err(error) = failure {
            outcome.error(statement.line, error.message());
            match mode {
                // A failed statement wrote nothing on its own, so there is nothing
                // to skip over: the next one runs.
                Mode::Skip => {}
                // Keep the prefix and say where it stopped.
                Mode::Commit => {
                    outcome.stopped_at = Some(statement.line);
                    outcome.failed = false;
                    outcome.rollback = false;
                    return Ok(outcome);
                }
                Mode::Stop => {
                    outcome.stopped_at = Some(statement.line);
                    outcome.failed = true;
                    outcome.rollback = in_transaction;
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
}

/// The row loop. Split out so the caller opens and closes the transaction on
/// every path.
#[allow(clippy::too_many_arguments)]
async fn import_rows(
    session: &mut Box<dyn Session>,
    reader: &mut RowReader,
    plan: &Plan,
    mode: Mode,
    batch_size: usize,
    timeout: Option<Duration>,
    in_transaction: bool,
    safe: SafeMode,
    cancel: &CancelFlag,
    out: &mut dyn Emitter,
    progress: &mut Progress,
) -> Result<Outcome, CliError> {
    let mut outcome = Outcome {
        disposition: if in_transaction { "pending" } else { "written" },
        ..Outcome::default()
    };
    let mut batch: Vec<(usize, Vec<String>)> = Vec::new();

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
                if let Some(message) = flush(
                    session,
                    plan,
                    &batch,
                    timeout,
                    safe,
                    &mut outcome,
                    out,
                    progress,
                )
                .await?
                {
                    outcome.error(line_of(&batch), message);
                }
                batch.clear();
                outcome.stopped_at = Some(row.line);
                outcome.failed = mode == Mode::Stop;
                outcome.rollback = mode == Mode::Stop && in_transaction;
                return Ok(outcome);
            }
            continue;
        }

        batch.push((row.line, row.cells));
        if mode == Mode::Skip || batch.len() >= batch_size {
            let line = line_of(&batch);
            if let Some(message) = flush(
                session,
                plan,
                &batch,
                timeout,
                safe,
                &mut outcome,
                out,
                progress,
            )
            .await?
            {
                if mode == Mode::Skip {
                    // The statement is the batch; a failed one writes nothing.
                    outcome.error(line, message);
                } else {
                    outcome.stopped_at = Some(line);
                    outcome.error(line, message);
                    outcome.failed = mode == Mode::Stop;
                    outcome.rollback = mode == Mode::Stop && in_transaction;
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
        if let Some(message) = flush(
            session,
            plan,
            &batch,
            timeout,
            safe,
            &mut outcome,
            out,
            progress,
        )
        .await?
        {
            if mode == Mode::Skip {
                outcome.error(line, message);
            } else {
                outcome.stopped_at = Some(line);
                outcome.error(line, message);
                outcome.failed = mode == Mode::Stop;
                outcome.rollback = mode == Mode::Stop && in_transaction;
            }
        }
    }

    Ok(outcome)
}

/// Send one batch; `Some(message)` when the statement failed, with the count
/// already added on success.
#[allow(clippy::too_many_arguments)]
async fn flush(
    session: &mut Box<dyn Session>,
    plan: &Plan,
    batch: &[(usize, Vec<String>)],
    timeout: Option<Duration>,
    safe: SafeMode,
    outcome: &mut Outcome,
    out: &mut dyn Emitter,
    progress: &mut Progress,
) -> Result<Option<String>, CliError> {
    if batch.is_empty() {
        return Ok(None);
    }
    match send_batch(session, plan, batch, timeout, safe).await {
        Ok(count) => {
            outcome.written += count;
            progress.emit(outcome.written, out)?;
            Ok(None)
        }
        Err((_line, message)) => Ok(Some(message)),
    }
}

/// One `INSERT`, and the server's own count checked against what was sent.
async fn send_batch(
    session: &mut Box<dyn Session>,
    plan: &Plan,
    batch: &[(usize, Vec<String>)],
    timeout: Option<Duration>,
    safe: SafeMode,
) -> Result<u64, (usize, String)> {
    let line = line_of(batch);
    let statement = insert_statement(plan, batch.iter().map(|(_, cells)| cells));
    // Guarded here as well as at the door: this is the statement that runs, and
    // the same rule `to_table` follows.
    if let Err(error) = guard(safe, &statement) {
        return Err((line, error.message()));
    }
    match run(session, &statement, timeout).await {
        Ok(affected) => {
            let sent = batch.len() as u64;
            if let Some(actual) = affected {
                // For an INSERT the server's count is the rows written, and a
                // mismatch is a real disagreement, not a MySQL no-op update.
                if actual != sent {
                    return Err((
                        line,
                        format!("the server wrote {actual} row(s) but {sent} were sent"),
                    ));
                }
            }
            Ok(sent)
        }
        Err(error) => Err((line, error.message())),
    }
}

fn line_of(batch: &[(usize, Vec<String>)]) -> usize {
    batch.first().map(|(line, _)| *line).unwrap_or(0)
}

/// Read one statement to its end, returning the server's affected-row count.
async fn run(
    session: &mut Box<dyn Session>,
    statement: &str,
    timeout: Option<Duration>,
) -> Result<Option<u64>, CliError> {
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
    null_text: String,
) -> Result<Plan, CliError> {
    let fields = match settings.get("COLUMNS") {
        Some(raw) if !raw.trim().is_empty() => parse_columns(raw)?,
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

/// The `COLUMNS` JSON, or the reason it is not usable.
fn parse_columns(raw: &str) -> Result<Vec<(usize, String)>, CliError> {
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
        fields.push((source as usize, target.to_owned()));
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
fn row_is_empty(row: &RawRow, plan: &Plan) -> bool {
    !plan.targets.iter().any(|target| {
        row.cells
            .get(target.source)
            .is_some_and(|cell| !cell.is_empty() && *cell != plan.null_text)
    })
}

/// One `INSERT` for a batch of rows.
fn insert_statement<'a>(plan: &Plan, rows: impl Iterator<Item = &'a Vec<String>>) -> String {
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
                    let text = row.get(target.source).map(String::as_str).unwrap_or("");
                    literal(text, &target.type_name, &plan.null_text, plan.style.style)
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("({values})")
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("INSERT INTO {} ({columns}) VALUES {tuples}", plan.table_sql)
}

/// One value as a SQL literal, by the target column's type.
fn literal(text: &str, type_name: &str, null_text: &str, style: IdentStyle) -> String {
    if text == null_text {
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
            "IMPORT_PATH is required: the CSV, XLSX or SQL file to read".to_owned(),
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
        return SourceFormat::parse(&raw).map(Source::Rows).ok_or_else(|| {
            CliError::Usage(format!(
                "unknown IMPORT_FORMAT '{raw}'; expected csv, xlsx or sql"
            ))
        });
    }
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "csv" | "tsv" => Ok(Source::Rows(SourceFormat::Csv)),
        "xlsx" | "xlsm" => Ok(Source::Rows(SourceFormat::Xlsx)),
        "sql" => Ok(Source::Statements),
        other => Err(CliError::Usage(format!(
            "cannot tell the format from extension '{other}'; set IMPORT_FORMAT to csv, xlsx or sql"
        ))),
    }
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
            null_text: null_text.to_owned(),
            style: SlotStyle::of(qh_driver::DriverKind::Postgres),
        }
    }

    #[test]
    fn a_number_is_bare_and_text_is_quoted() {
        // Bare for numeric target types, because a bare number is what a numeric
        // column takes and quoting it is a type mismatch on a strict server.
        assert_eq!(literal("42", "int8", "", IdentStyle::Ansi), "42");
        assert_eq!(literal("-1.5", "float8", "", IdentStyle::Ansi), "-1.5");
        assert_eq!(literal("hello", "text", "", IdentStyle::Ansi), "'hello'");
        // A non-number in a numeric column is quoted, not written bare: the
        // server's refusal, in a transaction, is a rollback and not broken SQL.
        assert_eq!(literal("abc", "int8", "", IdentStyle::Ansi), "'abc'");
    }

    #[test]
    fn a_quote_is_doubled_and_mysql_also_doubles_a_backslash() {
        assert_eq!(
            literal("O'Brien", "text", "", IdentStyle::Ansi),
            "'O''Brien'"
        );
        // Postgres reads a backslash literally, so it is not doubled there.
        assert_eq!(literal(r"a\b", "text", "", IdentStyle::Ansi), r"'a\b'");
        // MySQL reads it as an escape, so it is.
        assert_eq!(literal(r"a\b", "text", "", IdentStyle::Mysql), r"'a\\b'");
    }

    #[test]
    fn the_null_text_is_null_and_nothing_else_is() {
        assert_eq!(literal("", "text", "", IdentStyle::Ansi), "NULL");
        assert_eq!(literal("NA", "text", "NA", IdentStyle::Ansi), "NULL");
        // The literal string "NA" is data when the null text is something else.
        assert_eq!(literal("NA", "text", "\\N", IdentStyle::Ansi), "'NA'");
    }

    #[test]
    fn booleans_become_keywords_not_numbers() {
        assert_eq!(literal("true", "boolean", "", IdentStyle::Ansi), "TRUE");
        assert_eq!(literal("0", "bool", "", IdentStyle::Ansi), "FALSE");
        // Bare `1` is not a boolean on every server, so it is mapped, not passed.
        assert_eq!(literal("1", "bool", "", IdentStyle::Ansi), "TRUE");
    }

    #[test]
    fn a_batch_becomes_one_statement_with_one_tuple_per_row() {
        let plan = plan("");
        let rows = [
            vec!["1".to_owned(), "a".to_owned()],
            vec!["2".to_owned(), "b".to_owned()],
        ];
        let sql = insert_statement(&plan, rows.iter());
        assert_eq!(
            sql,
            "INSERT INTO \"public\".\"people\" (\"id\", \"name\") VALUES (1, 'a'), (2, 'b')"
        );
    }

    #[test]
    fn a_row_with_no_mapped_value_is_rejected() {
        let plan = plan("");
        let empty = RawRow {
            line: 3,
            cells: vec![String::new(), String::new()],
        };
        assert!(row_is_empty(&empty, &plan));
        let partial = RawRow {
            line: 3,
            cells: vec![String::new(), "x".to_owned()],
        };
        assert!(!row_is_empty(&partial, &plan));
        // A row shorter than the mapping is treated as empty for the missing
        // cells, which is what a short CSV line is.
        let short = RawRow {
            line: 4,
            cells: vec![String::new()],
        };
        assert!(row_is_empty(&short, &plan));
    }

    #[test]
    fn the_column_mapping_parses_and_skips_excluded_fields() {
        let raw = r#"[{"source":0,"target":"id"},{"source":1,"target":"skip","include":false},{"source":2,"target":"name"}]"#;
        let fields = parse_columns(raw).expect("a map");
        assert_eq!(fields, vec![(0, "id".to_owned()), (2, "name".to_owned())]);
        assert!(parse_columns("not json").is_err());
        assert!(parse_columns(r#"[{"target":"id"}]"#).is_err());
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
        assert_eq!(
            source_of(&settings(&[]), Path::new("/tmp/load.CSV")).ok(),
            Some(Source::Rows(SourceFormat::Csv))
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
            Some(Source::Rows(SourceFormat::Xlsx))
        );
        // A spelling nobody knows is refused by name, not defaulted.
        let error = source_of(
            &settings(&[("IMPORT_FORMAT", "parquet")]),
            Path::new("/tmp/load.csv"),
        )
        .unwrap_err();
        assert!(error.message().contains("csv, xlsx or sql"), "{error:?}");
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
}
