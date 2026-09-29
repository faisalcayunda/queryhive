//! The eleven driver-facing commands.
//!
//! One function per command, each a transcription of the removed Python engine's own
//! (`queryhive_engine.py:453-935`) rather than a reinterpretation of it: the event
//! names, the fields, and **the order the events go out in** are the contract the
//! app decodes and the snapshots freeze. Where a rule is subtle, the comment says
//! which Python line it comes from and why it is that way, because these are the
//! decisions the migration has to preserve rather than improve.
//!
//! The three local commands — the connection store, its legacy import and the password
//! store — are in [`crate::local`] instead: they open no driver, and the Python engine
//! had no equivalent of any of them.
//!
//! Two rules hold across every command and are worth stating once:
//!
//! 1. **A usage error happens before the network is touched.** `SQL` or `SQL_PATH`
//!    is read, the format is parsed, the write mode is checked — all before
//!    `step connect`. A user who forgot a field is told immediately rather than
//!    after a round trip to a coordinator.
//! 2. **`step connect` is emitted before connecting, and `step write` after the
//!    connection is open.** The app paints a progress log from those two, so their
//!    position is part of the protocol, not decoration.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use qh_core::{ColumnBatch, ColumnMeta, EngineError, Value};
use qh_driver::{
    BrowseLevel, ConnectionConfig, Cursor, DriverKind, ExecuteOptions, ObjectPath, Session,
};
use qh_export::plan::{ExportSpec, Exporter};
use qh_export::{ExportOptions, Format};
use qh_sql::{
    Decision, FloorSource, SafeMode, SafeModeFloor, StatementDecision, StatementKind, SAFE_MODES,
};
use serde_json::{json, Value as Json};

use crate::config;
use crate::env::Settings;
use crate::events::{event, Emitter};
use crate::execution_log::{self, DecisionEntry, LogDecision};
use crate::progress::{Progress, PROGRESS_MS_DEFAULT};
use crate::retry::{self, RetryPolicy};
use crate::sql_ident::{qualified, reference, slots, SlotStyle};
use crate::{CancelFlag, CliError, Engine};

/// Rows per `rows` event of `preview`, and the cursor's own fetch size.
///
/// Small enough that the grid paints before the whole page set has arrived, large
/// enough that a 1000-row preview is five events rather than a thousand.
pub const PREVIEW_BATCH: usize = 200;

/// Rows `preview` returns when `LIMIT` says nothing. The cap is a floor of one, so
/// `LIMIT=0` asks for one row rather than none.
pub const PREVIEW_LIMIT: i64 = 1_000;

/// Rows asked for when the row cap needs to know whether more exists.
///
/// One, because the question is "does the server have another row" and the answer is in
/// the first one. Asking for a page instead would cost `PREVIEW_BATCH` rows of work for
/// every preview whose `LIMIT` is a multiple of the page size, which is the default
/// `LIMIT=1000` against `PREVIEW_BATCH=200` — five pages of which the last was fetched
/// only to be discarded. All three drivers honour the number they are asked for
/// (PostgreSQL and MySQL stop their read, Trino caps its page budget).
pub const VERDICT_FETCH: usize = 1;

/// Recorded when a cancel reached the server mid-write.
pub const CANCEL_WARNING: &str = "stopped before the statement finished";

// --------------------------------------------------------------------------- //
// helpers
// --------------------------------------------------------------------------- //

/// The connection, with the driver's default port filled in.
///
/// Port 0 means "the driver's default" throughout this engine — it is what a
/// missing `DB_PORT` produces — and this is the layer that knows which driver is
/// in play, so this is where it is resolved.
pub(crate) fn connection(
    settings: &Settings,
    engine: &dyn Engine,
) -> Result<ConnectionConfig, CliError> {
    let mut config = config::build(settings)?;
    if config.port == 0 {
        config.port = engine.driver(config.kind).default_port();
    }
    Ok(config)
}

/// The statement bound a run asks for, from `STATEMENT_TIMEOUT_MS`.
///
/// `0` — the engine's default — means no bound, so the CLI and the golden corpus
/// behave exactly as they did before this setting existed. A negative value is
/// refused by name rather than floored: a bound that quietly became no bound would
/// be the opposite of what the caller asked for.
///
/// The bound is a `Duration` here and a server setting one layer down; each driver
/// translates it into its own mechanism, and a driver that cannot enforce one says
/// so through `Capabilities::statement_timeout`.
pub(crate) fn statement_timeout(settings: &Settings) -> Result<Option<Duration>, CliError> {
    let milliseconds = settings.non_negative("STATEMENT_TIMEOUT_MS", 0)?;
    Ok((milliseconds > 0).then(|| Duration::from_millis(milliseconds as u64)))
}

/// Every condition that can raise a run's Safe Mode, and the strictest of them.
///
/// Four conditions exist here. The user's own `SAFE_MODE` is the base; `DB_READ_ONLY` is the
/// connection's own marking, so a connection the app knows is read-only stays read-only whatever
/// level is chosen; `SAFE_MODE_FLOOR` is a minimum pinned from outside the connection — an
/// embedding caller, an external-client gate, a managed policy — which is the one input a
/// configuration profile would set if this project had one; and a driver whose
/// `Capabilities::read_only` is set raises the floor itself. The floor is resolved by
/// [`SafeMode::strictness`](qh_sql::SafeMode::strictness), never by which condition matched first,
/// and it is **never written back**: it is a value for one run, and when the condition goes away
/// the user's own level is what remains.
///
/// The driver condition is read from `engine.driver(kind).capabilities()` **before** connecting,
/// which is why the floor can see it at all. All three drivers report `read_only: false` today, so
/// this changes nothing yet — it is what makes the bit mean something the day one of them cannot
/// write.
pub(crate) fn safe_mode_floor(
    settings: &Settings,
    engine: &dyn Engine,
) -> Result<SafeModeFloor, CliError> {
    let mut floor = SafeModeFloor::new();
    floor.raise(
        FloorSource::User,
        parse_safe_mode(&settings.text("SAFE_MODE", ""), "SAFE_MODE")?,
    );
    if settings.flag("DB_READ_ONLY", false) {
        floor.raise(FloorSource::Connection, SafeMode::ReadOnly);
    }
    // A driver that cannot write raises the floor itself. Read **before connecting**, which is why
    // the floor can see it: `Capabilities` is documented as readable before a session exists, so
    // `driver(kind)` answers without opening anything. Today all three drivers report `false`, so
    // this changes nothing yet — it is the wiring that makes the bit mean something the day one of
    // them is read-only.
    if let Some(kind) = DriverKind::parse(&settings.text("DB_KIND", "")) {
        if engine.driver(kind).capabilities().read_only {
            floor.raise(FloorSource::Driver, SafeMode::ReadOnly);
        }
    }
    let pinned = settings.text("SAFE_MODE_FLOOR", "");
    if !pinned.is_empty() {
        floor.raise(
            FloorSource::Policy,
            parse_safe_mode(&pinned, "SAFE_MODE_FLOOR")?,
        );
    }
    Ok(floor)
}

/// The Safe Mode a run is under, after every floor has been applied.
///
/// Absent `SAFE_MODE` is `full`, which is what every caller that predates this setting
/// sends. A spelling nobody recognises is refused by name: a typo in a safety setting is
/// not a decision to make silently, the same rule `sslmode` follows.
pub(crate) fn safe_mode(settings: &Settings, engine: &dyn Engine) -> Result<SafeMode, CliError> {
    Ok(safe_mode_floor(settings, engine)?
        .resolve()
        .map(|(mode, _source)| mode)
        .unwrap_or(SafeMode::Full))
}

/// Parse one Safe Mode value, refusing an unknown spelling by the setting's own name.
fn parse_safe_mode(raw: &str, key: &str) -> Result<SafeMode, CliError> {
    if raw.trim().is_empty() {
        return Ok(SafeMode::Full);
    }
    SafeMode::parse(raw).ok_or_else(|| {
        CliError::Usage(format!(
            "unknown {key} '{raw}'; expected {}",
            SAFE_MODES.join(", ")
        ))
    })
}

/// Whether this run carries the caller's explicit confirmation.
///
/// `SAFE_MODE_CONFIRMED=1` is the whole of a confirmation as far as the engine can see it:
/// a boolean only a caller that asked its user could have set. The dialog, the biometric
/// prompt and the password fallback are the app's, and are deliberately not modelled here.
pub(crate) fn safe_mode_confirmed(settings: &Settings) -> bool {
    settings.flag("SAFE_MODE_CONFIRMED", false)
}

/// Refuse a script the connection's Safe Mode does not allow, with no confirmation.
///
/// This is the shape every caller that predates the `confirm` level uses, including the
/// bulk `apply_changes` and `import_data` paths: a single confirmation cannot cover a whole
/// plan, so a `confirm` connection refuses them until the caller raises the level.
pub(crate) fn guard(mode: SafeMode, sql: &str) -> Result<(), CliError> {
    guard_confirmed(mode, false, sql)
}

/// Refuse a script the connection's Safe Mode does not allow, recording every decision.
///
/// The engine's own guard, and deliberately not the UI's: the CLI and the MCP server run
/// this same code, and neither has a picker to enforce anything. A refusal is a usage error
/// decided before the network is touched, so a read-only connection refuses a `DROP`
/// without opening one. Each decision reaches the execution log, and a failed log write is
/// a failure: an audit record that could not be written is not quietly skipped.
pub(crate) fn guard_confirmed(mode: SafeMode, confirmed: bool, sql: &str) -> Result<(), CliError> {
    for statement in qh_sql::decisions(mode, sql) {
        if let Some(error) = record_decision(
            mode,
            confirmed,
            statement.index,
            statement.kind,
            statement.statement,
            statement.decision,
            statement.reason,
        )? {
            return Err(CliError::Usage(error.to_string()));
        }
    }
    Ok(())
}

/// Record one decision and say the error it raises, if any.
///
/// The one place the log's vocabulary ([`LogDecision`]) and the classifier's own error are
/// applied to a decision, shared by [`guard_confirmed`], [`guard_destructive`] and the
/// whole-operation check [`record_kind`]. Keeping it in one function is what makes a
/// confirmation recorded by one path read exactly like one recorded by another.
fn record_decision(
    mode: SafeMode,
    confirmed: bool,
    index: usize,
    kind: StatementKind,
    statement: &str,
    decision: Decision,
    reason: Option<&'static str>,
) -> Result<Option<qh_sql::SafeModeError>, CliError> {
    let evaluated = StatementDecision {
        index,
        kind,
        statement,
        decision,
        reason,
    };
    let error = evaluated.refusal_error(mode, confirmed);
    let logged = match (&error, decision) {
        (None, Decision::Confirm) => LogDecision::Confirmed,
        (None, _) => LogDecision::Allowed,
        (Some(_), Decision::Confirm) => LogDecision::NeedsConfirmation,
        (Some(_), _) => LogDecision::Refused,
    };
    execution_log::record(&DecisionEntry {
        safe_mode: mode,
        index,
        kind,
        statement,
        decision: logged,
        // The reason is why the statement did not run as-is. A row that ran — allowed,
        // or allowed after a confirmation — has no failing reason to record, and the
        // confirmation sentence belongs to the question, not to the answer.
        reason: error.as_ref().and(reason),
    })?;
    Ok(error)
}

/// [`guard_confirmed`] with the confirmation taken from the run's own settings.
///
/// The five single-statement commands use this, so a `confirm` connection runs a write the
/// caller sent `SAFE_MODE_CONFIRMED=1` for and asks otherwise. The bulk paths keep the
/// unconfirmed [`guard`] deliberately: one confirmation does not cover a plan.
pub(crate) fn guard_for(settings: &Settings, mode: SafeMode, sql: &str) -> Result<(), CliError> {
    guard_confirmed(mode, safe_mode_confirmed(settings), sql)
}

/// Why a destructive table operation is a question on a `confirm` connection.
///
/// The classifier refuses DDL under `confirm` (ADR-0026), so the ordinary guard would make
/// `TRUNCATE`/`DROP` impossible on the one level that exists to ask before a write. The plan
/// asks for these two operations "lewat konfirmasi" (§13), so [`guard_destructive`] asks
/// instead. The exception is the command's own; the sentence says what it is asking.
pub(crate) const DESTRUCTIVE_CONFIRM_REASON: &str =
    "a destructive table operation runs on a confirm connection only after an explicit \
     confirmation (SAFE_MODE_CONFIRMED=1)";

/// The Safe Mode gate for a destructive table operation (`table_op`).
///
/// `DROP` and `TRUNCATE` are DDL, so `no_ddl` and `read_only` refuse them exactly as
/// [`guard_for`] refuses any other DDL — with the classifier's own sentence and through the
/// same [`record_decision`], so the log cannot read differently from the path that would have
/// written it. `confirm` is the one difference, and it is deliberate: ADR-0026 makes the
/// classifier refuse DDL there, which would leave the two operations the plan asks for
/// ("truncate/drop tabel lewat konfirmasi") unrunnable at the level built to ask about them.
/// ADR-0027 records the exception; its one recorded cost is closed by [`destructive_mode`].
pub(crate) fn guard_destructive(
    settings: &Settings,
    engine: &dyn Engine,
    statement: &str,
) -> Result<(), CliError> {
    let kind = qh_sql::classify(statement);
    let mode = destructive_mode(settings, engine)?;
    let (decision, reason) = match mode {
        SafeMode::Full => (Decision::Allow, None),
        // Both refusals reuse the classifier's sentence, so a `no_ddl` connection is told
        // exactly what it is told about any other DDL.
        SafeMode::NoDdl | SafeMode::ReadOnly => (Decision::Refuse, mode.refusal(kind)),
        SafeMode::Confirm => (Decision::Confirm, Some(DESTRUCTIVE_CONFIRM_REASON)),
    };
    if let Some(error) = record_decision(
        mode,
        safe_mode_confirmed(settings),
        1,
        kind,
        statement,
        decision,
        reason,
    )? {
        return Err(CliError::Usage(error.to_string()));
    }
    Ok(())
}

/// The Safe Mode a destructive table operation is under.
///
/// The floor's **entries**, not its resolved mode. A total strictness order cannot express
/// "this condition forbids DDL whatever else is pinned", and that is the hole ADR-0027
/// recorded: with `SAFE_MODE=confirm` and `SAFE_MODE_FLOOR=no_ddl` the resolved mode is
/// `confirm` (strictness 2 > 1), so the ordinary guard asked and a confirmed `DROP` ran on a
/// connection whose floor forbids every DDL. Here an entry that refuses DDL decides first,
/// then an entry that asks, and only a floor of `full` allows without a question. A floor may
/// therefore only ever be stricter than the user's own choice, which is what ADR-0026
/// promises.
fn destructive_mode(settings: &Settings, engine: &dyn Engine) -> Result<SafeMode, CliError> {
    let floor = safe_mode_floor(settings, engine)?;
    let forbids_ddl = floor
        .iter()
        .filter(|(_, mode)| matches!(mode, SafeMode::NoDdl | SafeMode::ReadOnly))
        .map(|(_, mode)| mode)
        .max_by_key(|mode| mode.strictness());
    if let Some(mode) = forbids_ddl {
        return Ok(mode);
    }
    if floor.iter().any(|(_, mode)| mode == SafeMode::Confirm) {
        return Ok(SafeMode::Confirm);
    }
    Ok(SafeMode::Full)
}

/// Record the decision a mode makes about a whole operation, before its own error is raised.
///
/// `import_data` refuses a DML import before it opens a connection, so at that point there is
/// no statement to classify. This writes the row [`guard_confirmed`] would write for a
/// one-statement script, which is what keeps the one refusal that path makes from being the
/// engine's only unlogged decision. `confirmed` is deliberately `false`: the bulk paths never
/// carry a confirmation, because one confirmation cannot cover a plan (ADR-0026).
pub(crate) fn record_kind(
    mode: SafeMode,
    kind: StatementKind,
    subject: &str,
) -> Result<(), CliError> {
    let decision = mode.decision(kind);
    let reason = mode.refusal(kind);
    let _ = record_decision(mode, false, 1, kind, subject, decision, reason)?;
    Ok(())
}

/// Where a browse or objects call is aimed.
///
/// The three drivers name their levels differently and the settings follow them:
/// Trino's catalog is the connection's `database`, PostgreSQL's tree starts at
/// schemas, and MySQL's first level is its database — which its driver reads from
/// the path's `schema` slot, because that is the slot that selects which tables are
/// listed.
fn path_for(config: &ConnectionConfig) -> ObjectPath {
    let mut path = ObjectPath::new();
    match config.kind {
        DriverKind::Trino => path.catalog = config.database.clone(),
        DriverKind::Postgres => path.schema = config.schema.clone(),
        DriverKind::Mysql => path.schema = config.database.clone(),
    }
    path.schema = path.schema.or_else(|| config.schema.clone());
    path
}

/// Which level of the tree a browse command lists.
///
/// Two drivers answer the same command under different level names — Trino's
/// `catalogs` is its catalog level, MySQL's is its database level — so the command
/// names the levels it will accept, in order, and the driver's own capabilities
/// pick one. A driver with none of them is a usage error naming the level, decided
/// before anything is opened, exactly as the Python engine refused it.
fn level_for(
    engine: &dyn Engine,
    kind: DriverKind,
    accepted: &[BrowseLevel],
    level_name: &str,
) -> Result<BrowseLevel, CliError> {
    let capabilities = engine.driver(kind).capabilities();
    accepted
        .iter()
        .copied()
        .find(|level| capabilities.levels.contains(level))
        .or_else(|| {
            // A driver answers a listing level it does not put in its tree, and PostgreSQL is
            // the case: its tree starts at schemas, and `catalogs` still answers with the
            // databases on the server. Both halves are deliberate and the previous engine
            // pins both -- its driver declares `levels = ("schema", "table")` and answers
            // `catalogs` -- so the question here is which of the two a *command* asks about,
            // and it is the command. Reading `levels` as "the commands this driver answers"
            // refuses a command the driver implements, before the driver is ever asked.
            //
            // Only a listing level falls through, which keeps the other half intact: a driver
            // that genuinely has no such level is still refused before anything is opened,
            // which is what makes that refusal a usage error rather than a failure.
            accepted
                .first()
                .copied()
                .filter(|level| LISTING_LEVELS.contains(level))
        })
        .ok_or_else(|| {
            CliError::Usage(format!(
                "{kind} has no {level_name} level; {}",
                instead_of(kind, &capabilities.levels)
            ))
        })
}

/// The levels a driver may answer without declaring them in its tree.
///
/// The list of databases a connection can reach is a picker list rather than a node: it is
/// what the user chooses from, not something the tree draws underneath the connection. A
/// driver that lists them is right not to call its tree catalog-first, and the command is
/// still one it answers.
const LISTING_LEVELS: [BrowseLevel; 2] = [BrowseLevel::Catalog, BrowseLevel::Database];

/// What to use instead, for a refusal to name.
///
/// A refusal that only says what failed leaves the user to guess which of the other
/// commands answers their question, and the previous engine did not: it named the two
/// that work. This is built from the levels the driver declared rather than written out
/// per driver, so a driver that gains or loses a level gets the right sentence without
/// anyone remembering to edit one.
///
/// The driver has wording of its own for the same situation (`crates/qh-driver-mysql`'s
/// `browse` arm: "catalogs lists its databases and tables lists their tables"), and this
/// is deliberately a second sentence rather than the same one. The driver knows the nouns
/// -- a database is a database -- and this knows the command names, which is what a user
/// of the command line types. Each says only what it can know, so the two cannot drift
/// into contradicting each other.
fn instead_of(kind: DriverKind, levels: &[BrowseLevel]) -> String {
    let mut commands: Vec<&str> = Vec::new();
    for level in levels {
        // A catalog and a database are the same command: the driver picks which of the two
        // it has, and the user types `catalogs` either way.
        let command = match level {
            BrowseLevel::Catalog | BrowseLevel::Database => "catalogs",
            BrowseLevel::Schema => "schemas",
            BrowseLevel::Table => "tables",
        };
        if !commands.contains(&command) {
            commands.push(command);
        }
    }
    match commands.as_slice() {
        [] => format!("{kind} lists no level at all"),
        [only] => format!("use {only} instead"),
        [first, second] => format!("use {first} or {second} instead"),
        [rest @ .., last] => format!("use {} or {last} instead", rest.join(", ")),
    }
}

/// SQL, or the contents of `SQL_PATH`; neither being set is a usage error.
fn source_sql(settings: &Settings) -> Result<String, CliError> {
    let sql = settings.raw("SQL", "");
    if !sql.trim().is_empty() {
        return Ok(sql);
    }
    let path = settings.text("SQL_PATH", "");
    if path.is_empty() {
        return Err(CliError::Usage("SQL or SQL_PATH is required".to_owned()));
    }
    let path = expand_user(&path);
    std::fs::read_to_string(&path).map_err(|error| {
        CliError::Usage(format!(
            "cannot read SQL_PATH '{}': {error}",
            path.display()
        ))
    })
}

/// `~` as the user's home directory, which is what `Path.expanduser()` did. A
/// `~user` form is left alone: it needs a passwd lookup this engine has no business
/// doing.
///
/// Shared with [`crate::local`], where `DB_PATH` and `LEGACY_PATH` are the other two
/// settings that name a file the user may have written with a `~` in it.
pub(crate) fn expand_user(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => match std::env::var("HOME") {
            Ok(home) if !home.is_empty() => Path::new(&home).join(rest),
            _ => PathBuf::from(path),
        },
        None => PathBuf::from(path),
    }
}

/// The export directory, absolute, because `done.files` reports absolute paths.
fn out_dir(settings: &Settings) -> Result<PathBuf, CliError> {
    let raw = settings.text("OUT_DIR", "");
    let path = if raw.is_empty() {
        std::env::current_dir().map_err(|error| CliError::Internal(error.to_string()))?
    } else {
        expand_user(&raw)
    };
    std::path::absolute(path).map_err(|error| CliError::Internal(error.to_string()))
}

/// The requested format, refused by name before anything is opened.
fn format_of(settings: &Settings) -> Result<Format, CliError> {
    let name = settings.text("FORMAT", "csv").to_ascii_lowercase();
    Format::parse(&name).ok_or_else(|| {
        let known = Format::ALL
            .iter()
            .map(|format| format.name())
            .collect::<Vec<_>>()
            .join(", ");
        CliError::Usage(format!("unknown FORMAT '{name}'; expected one of {known}"))
    })
}

/// Data rows per file, with a blank or non-positive value meaning "never split".
///
/// A cleared dialog field arrives as `0` and means "don't split", not "one row per
/// file" — the Python engine filtered falsy values here for the same reason.
fn rows_per_file(settings: &Settings) -> Result<Option<usize>, CliError> {
    let count = settings.number("ROWS_PER_FILE", 0)?;
    Ok(usize::try_from(count).ok().filter(|count| *count > 0))
}

/// The writers' options, with exactly `format_opts`' keys.
fn format_opts(settings: &Settings, name: &str) -> Result<ExportOptions, CliError> {
    let mut options = ExportOptions::default();

    // A blank delimiter means "the format's own", which is a tab for `txt` and a
    // comma for `csv` — a different thing from a delimiter that is the empty string.
    let delimiter = settings.raw("DELIMITER", "");
    if !delimiter.is_empty() {
        let mut characters = delimiter.chars();
        let character = characters.next().expect("a non-empty string has a char");
        if characters.next().is_some() {
            // The Python engine let `csv` refuse this with "delimiter must be a
            // 1-character string". Saying so here names the setting instead.
            return Err(CliError::Usage(format!(
                "DELIMITER must be a single character, got '{delimiter}'"
            )));
        }
        options.delimiter = Some(character);
    }

    options.header = settings.flag("HEADER", true);
    options.bom = settings.flag("BOM", false);
    options.null_text = settings.raw("NULL_TEXT", "");
    options.jsonl = settings.flag("JSONL", false);
    options.sql_table = optional(settings.raw("SQL_TABLE", ""));
    options.sheet = Some(settings.raw("SHEET", "Sheet1"));
    options.dbf_char_width = usize::try_from(settings.number("DBF_CHAR_WIDTH", 254)?)
        .map_err(|_| CliError::Usage("DBF_CHAR_WIDTH must be a positive number".to_owned()))?;
    options.title = Some(name.to_owned());
    // The two code-page settings the Python engine read here. A blank value keeps the
    // format's own default, the convention `DELIMITER` uses just above: UTF-8 for `txt`
    // and `csv`, cp1252 for `dbf`, which is what `ExportOptions` already carries. An
    // unknown name is refused by name when the writer opens -- before a file exists --
    // the shape `DELIMITER` and `DBF_CHAR_WIDTH` are refused in.
    let encoding = settings.raw("ENCODING", "");
    if !encoding.is_empty() {
        options.encoding = encoding;
    }
    let dbf_encoding = settings.raw("DBF_ENCODING", "");
    if !dbf_encoding.is_empty() {
        options.dbf_encoding = dbf_encoding;
    }
    Ok(options)
}

fn optional(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

/// A column list as the app's typed headers: `[{"name": ..., "type": ...}]`.
fn columns_json(columns: &[ColumnMeta]) -> Json {
    Json::Array(
        columns
            .iter()
            .map(|column| json!({ "name": column.name, "type": column.type_name }))
            .collect(),
    )
}

/// One row of a column-major batch as the writers take it.
///
/// Copied rather than borrowed, because a row of a [`ColumnBatch`] is not
/// contiguous: the batch holds columns. It is one small `Vec` per row and it is
/// dropped as soon as it is written, so peak memory stays flat.
fn row_of(batch: &ColumnBatch, index: usize) -> Vec<Value> {
    batch
        .columns()
        .iter()
        .map(|column| column[index].clone())
        .collect()
}

/// List one level and close the session.
///
/// Takes the session rather than borrowing it because closing consumes it — that is
/// the shape of `Session::close`, so that a driver cannot be left half-open. The
/// listing itself goes through [`retry::browse`], which is why the policy comes in.
async fn browse_session(
    policy: &RetryPolicy,
    mut session: Box<dyn Session>,
    level: BrowseLevel,
    path: &ObjectPath,
    include_system: bool,
) -> Result<Vec<String>, CliError> {
    let names = retry::browse(&mut session, policy, level, path, include_system).await?;
    // Best effort: a close that fails after the names are in hand must not turn a
    // successful listing into an error the user sees.
    let _ = session.close().await;
    Ok(names)
}

/// A session, and the retry policy `RETRIES` asked for.
///
/// The policy is returned rather than kept inside the session because a retry is a
/// property of the *calls* a command makes, not of the session: the commands that read
/// pass it to [`retry::execute`], and the one that writes deliberately does not — see
/// [`crate::retry`]. Reading the setting in one place is what keeps `RETRIES` from being
/// read two ways.
pub(crate) async fn open(
    settings: &Settings,
    engine: &dyn Engine,
    config: &ConnectionConfig,
) -> Result<(Box<dyn Session>, RetryPolicy), CliError> {
    let policy = RetryPolicy::from_settings(settings)?;
    let session = retry::connect(engine, config, &policy).await?;
    Ok((session, policy))
}

// --------------------------------------------------------------------------- //
// the commands
// --------------------------------------------------------------------------- //

/// `db_drivers`: every driver, its label, its default port and its tree levels.
///
/// Nothing is opened and no setting is read, so this is the one command a caller
/// can always run first.
pub async fn db_drivers(out: &mut dyn Emitter, engine: &dyn Engine) -> Result<(), CliError> {
    let mut drivers = Vec::new();
    for kind in engine.kinds() {
        let driver = engine.driver(kind);
        let capabilities = driver.capabilities();
        drivers.push(json!({
            "kind": driver.kind().as_str(),
            "label": driver.label(),
            "default_port": driver.default_port(),
            "levels": capabilities
                .levels
                .iter()
                .map(|level| level.as_str())
                .collect::<Vec<_>>(),
        }));
    }
    out.emit(event("drivers").field("drivers", drivers).build())?;
    Ok(())
}

/// `test`: connect and run the driver's own top-level probe.
///
/// The probe is the outermost level the driver lists at all — `SHOW CATALOGS` on
/// Trino, the schema list on PostgreSQL, `SHOW DATABASES` on MySQL — so `test`
/// never needs a catalog or a schema to be configured first. The count is reported
/// as `catalog_count`, which is the key it has always had.
pub async fn test(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
) -> Result<(), CliError> {
    let config = connection(settings, engine)?;
    let (session, policy) = open(settings, engine, &config).await?;
    let level = session
        .capabilities()
        .levels
        .first()
        .copied()
        .ok_or_else(|| CliError::Usage(format!("{} has no top level to list", config.kind)))?;
    let names = browse_session(&policy, session, level, &path_for(&config), false).await?;
    out.emit(
        event("test")
            .field("ok", true)
            .field("catalog_count", names.len())
            .field("host", config.host.clone())
            .field("user", config.user.clone())
            .build(),
    )?;
    Ok(())
}

/// `catalogs`, `schemas` and `tables`: one level of the object tree, as names.
///
/// All three emit the same `names` array — one decoder reads them alike — while the
/// statement behind each, the settings it needs and a driver that has no such level
/// at all are the driver's business.
async fn browse_command(
    command: &str,
    accepted: &[BrowseLevel],
    level_name: &str,
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
) -> Result<(), CliError> {
    let config = connection(settings, engine)?;
    let level = level_for(engine, config.kind, accepted, level_name)?;
    // "Show all" reaches the two levels that ever hide something — PostgreSQL's
    // system schemas, MySQL's own databases — and is a no-op on Trino, whose SHOW
    // statements return everything already. One switch at the command rather than a
    // per-driver special case the caller has to remember.
    let include_system = settings.flag("DB_ALL_SCHEMAS", false);
    let (session, policy) = open(settings, engine, &config).await?;
    let names = browse_session(&policy, session, level, &path_for(&config), include_system).await?;
    out.emit(event(command).field("names", names).build())?;
    Ok(())
}

pub async fn catalogs(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
) -> Result<(), CliError> {
    browse_command(
        "catalogs",
        &[BrowseLevel::Catalog, BrowseLevel::Database],
        "catalog",
        settings,
        out,
        engine,
    )
    .await
}

pub async fn schemas(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
) -> Result<(), CliError> {
    browse_command(
        "schemas",
        &[BrowseLevel::Schema],
        "schema",
        settings,
        out,
        engine,
    )
    .await
}

pub async fn tables(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
) -> Result<(), CliError> {
    // No "show all" here: the Python engine's `tables_sql` never took one, so a
    // switch would be a promise this level cannot keep.
    browse_command(
        "tables",
        &[BrowseLevel::Table],
        "table",
        settings,
        out,
        engine,
    )
    .await
}

/// `objects`: one level's objects, with whatever metadata the driver can answer.
///
/// The two field names are load-bearing and were both wrong the first time:
/// `object_columns` because `columns` is the preview grid's typed headers, and
/// `data` because `rows` is an integer count on `progress` and `done` and one key
/// cannot be two types.
pub async fn objects(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
) -> Result<(), CliError> {
    let config = connection(settings, engine)?;
    let (mut session, policy) = open(settings, engine, &config).await?;
    let page = retry::objects(&mut session, &policy, &path_for(&config)).await?;
    let _ = session.close().await;
    out.emit(
        event("objects")
            .field("object_columns", page.columns)
            .field("data", page.rows)
            .build(),
    )?;
    Ok(())
}

/// `export`: stream one query into the requested format and report the files.
pub async fn export(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
    cancel: &CancelFlag,
) -> Result<(), CliError> {
    let sql = source_sql(settings)?;
    // The Safe Mode is checked before the connect step, so a read-only connection
    // refuses a write without opening one.
    let mode = safe_mode(settings, engine)?;
    guard_for(settings, mode, &sql)?;
    let timeout = statement_timeout(settings)?;
    let format = format_of(settings)?;
    let directory = out_dir(settings)?;
    let name = settings.raw("NAME", "export");
    let rows_per_file = rows_per_file(settings)?;
    let options = format_opts(settings, &name)?;
    let batch_size = usize::try_from(settings.number("BATCH_SIZE", 10_000)?)
        .unwrap_or(1)
        .max(1);
    let zip_result = settings.flag("ZIP", false);
    // Validated before the connect step is announced, so a bad setting is reported
    // without a connection attempt.
    let config = connection(settings, engine)?;

    out.emit(event("step").field("step", "connect").build())?;
    let (mut session, policy) = open(settings, engine, &config).await?;
    let mut cursor = retry::execute(
        &mut session,
        &policy,
        &sql,
        &ExecuteOptions {
            max_batch_rows: Some(batch_size),
            row_limit: None,
            statement_timeout: timeout,
        },
    )
    .await?;

    // The first batch is taken before anything is announced, because the columns are
    // only guaranteed once a batch has arrived: a Trino cursor reports none until
    // then, and the writer needs them to write its header. Python's `on_start` fires
    // at exactly this moment — "once the cursor is open and the first page exists".
    let primed = cursor.next_batch(batch_size).await?;

    out.emit(event("step").field("step", "write").build())?;
    out.emit(
        event("start")
            .field("columns", columns_json(cursor.columns()))
            .field("query_id", session.query_id())
            .build(),
    )?;

    let mut progress = Progress::new(settings.number("PROGRESS_MS", PROGRESS_MS_DEFAULT)?);
    let mut exporter = Exporter::new(ExportSpec {
        format,
        out_dir: &directory,
        basename: &name,
        columns: cursor.columns().to_vec(),
        options,
        rows_per_file,
    })?;
    {
        // The progress callback cannot return an error — the row loop that calls it
        // is infallible there — so a write that fails is reported by the next emit
        // that does return one (`done`, or the correction below). A broken stdout is
        // fatal either way; what matters is that it is not silent.
        let mut report = |rows: u64| {
            let _ = progress.emit(rows, out);
        };
        pump(
            &mut exporter,
            &mut cursor,
            primed,
            batch_size,
            cancel,
            &mut report,
        )
        .await?;
    }
    // A close that failed is the more urgent error: the file on disk is incomplete.
    exporter.finish()?;
    let outcome = exporter.into_outcome();
    if progress.last() != Some(outcome.rows) {
        out.emit(event("progress").field("rows", outcome.rows).build())?;
    }

    let mut files = outcome.files.clone();
    if zip_result && !files.is_empty() {
        files = vec![qh_export::bundle(
            &files,
            &directory.join(format!("{name}.zip")),
        )?];
    }
    let entries: Vec<Json> = files
        .iter()
        .map(|path| {
            json!({
                "path": path.to_string_lossy(),
                // A file this process just wrote and cannot stat is a real problem,
                // but the report is not the place to raise it: 0 says "unknown".
                "bytes": std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0),
            })
        })
        .collect();

    let query_id = session.query_id();
    let mut warnings = outcome.warnings;
    if outcome.cancelled {
        // `pump` stopped mid-result: the statement is still running on the server.
        warnings.extend(stop_session(session, Some(cursor)).await);
    } else {
        let _ = session.close().await;
    }
    out.emit(
        event("done")
            .field("rows", outcome.rows)
            .field("files", entries)
            .field("warnings", warnings)
            .field("query_id", query_id)
            .field("cancelled", outcome.cancelled)
            .build(),
    )?;
    Ok(())
}

/// The export's row loop.
///
/// Cancellation is asked **before each row**, so a row that arrives while the user
/// is pressing Cancel is not half-written into the file, and the file keeps the
/// rows that were already written. The 1000-row progress floor is not here: it
/// lives in `qh-export::plan`, which the synchronous export path uses too, so both
/// callers report at the same points.
///
/// A transient failure while fetching the next page does not reach this loop: the
/// cursor retries it (see [`crate::retry`]), and what the writer already wrote is never
/// fetched twice — the page that failed is the page that is asked for again.
async fn pump(
    exporter: &mut Exporter,
    cursor: &mut Box<dyn Cursor>,
    primed: Option<ColumnBatch>,
    batch_size: usize,
    cancel: &CancelFlag,
    report: &mut dyn FnMut(u64),
) -> Result<(), CliError> {
    let mut batch = primed;
    loop {
        if let Some(current) = batch.as_ref() {
            for index in 0..current.rows() {
                if cancel.is_cancelled() {
                    exporter.mark_cancelled();
                    exporter.report_progress(report);
                    return Ok(());
                }
                exporter.write_row_with_progress(&row_of(current, index), report)?;
            }
        }
        if cancel.is_cancelled() {
            exporter.mark_cancelled();
            exporter.report_progress(report);
            return Ok(());
        }
        match cursor.next_batch(batch_size).await? {
            Some(next) => batch = Some(next),
            None => break,
        }
    }
    // The closing report, so `done` is never preceded by a stale count.
    exporter.report_progress(report);
    Ok(())
}

/// `to_table`: have the server write the query's rows into a table.
///
/// No row crosses this process: the coordinator runs the SELECT and commits the
/// result, and this process only watches. The three modes are the three statements
/// `exporter/to_table.py` builds, and only the parts a driver has a level for are
/// required — PostgreSQL ignores `TARGET_CATALOG` (the database is the connection),
/// MySQL ignores `TARGET_SCHEMA` (it has no schema level).
///
/// Two honest differences from the Python engine are recorded in
/// `docs/golden-deltas.md`: the row count is `-1` because the `Cursor` contract has
/// no update count yet, and there is no mid-flight `state` on `progress` because
/// that came from the trino client's stats callback, which no Rust driver has.
pub async fn to_table(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
    cancel: &CancelFlag,
) -> Result<(), CliError> {
    let sql = source_sql(settings)?;
    let safe = safe_mode(settings, engine)?;
    let timeout = statement_timeout(settings)?;
    let config = connection(settings, engine)?;
    let style = SlotStyle::of(config.kind);

    let targets = [
        ("TARGET_CATALOG", settings.text("TARGET_CATALOG", "")),
        ("TARGET_SCHEMA", settings.text("TARGET_SCHEMA", "")),
        ("TARGET_TABLE", settings.text("TARGET_TABLE", "")),
    ];
    // A part the driver has no level for is not required: the three settings mean
    // what the driver can actually use, and `slots` is that list.
    let target_of = |name: &str| -> String {
        targets
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    let missing: Vec<&str> = slots(style)
        .iter()
        .map(|slot| slot.target)
        .filter(|target| target_of(target).is_empty())
        .collect();
    if !missing.is_empty() {
        return Err(CliError::Usage(format!(
            "{} required to write a table",
            missing.join(" and ")
        )));
    }

    // Read the environment directly rather than through the readers: both of those
    // fold a blank into "unset", and a `WRITE_MODE` the app sent as `""` cannot be
    // defaulted away — a mode this engine cannot name is not the mode it asked for.
    // Only an absent key means create.
    let raw_mode = settings.get("WRITE_MODE");
    let stripped = raw_mode
        .map(|mode| mode.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let mode = if !stripped.is_empty() {
        stripped.clone()
    } else if raw_mode.is_none() {
        "create".to_owned()
    } else {
        String::new()
    };
    if !matches!(mode.as_str(), "create" | "replace" | "append") {
        return Err(CliError::Usage(format!(
            "unknown WRITE_MODE '{stripped}'; expected create, replace or append"
        )));
    }

    let target = qualified(
        style,
        &target_of("TARGET_CATALOG"),
        &target_of("TARGET_SCHEMA"),
        &target_of("TARGET_TABLE"),
    );
    let name = reference(
        style,
        &target_of("TARGET_CATALOG"),
        &target_of("TARGET_SCHEMA"),
        &target_of("TARGET_TABLE"),
    );
    let body = sql.trim().trim_end_matches(';');
    let statements = match mode.as_str() {
        "replace" => vec![
            format!("DROP TABLE IF EXISTS {target}"),
            format!("CREATE TABLE {target} AS {body}"),
        ],
        "append" => vec![format!("INSERT INTO {target} {body}")],
        _ => vec![format!("CREATE TABLE {target} AS {body}")],
    };

    // The Safe Mode is checked against the statements this command will actually
    // send — the generated `DROP`/`CREATE TABLE AS`/`INSERT`, not only the caller's
    // SELECT — because those are what run. Checked here, before the connect step, so
    // a read-only connection refuses `replace` without opening one: `to_table`'s
    // write mode is a decision of the engine and not of the UI that offered it.
    for statement in &statements {
        guard_for(settings, safe, statement)?;
    }

    out.emit(event("step").field("step", "connect").build())?;
    // The connect is retried; the statements below are not, and deliberately so: a
    // `DROP`/`CREATE TABLE AS`/`INSERT` is not safe to re-issue blind, so this command
    // calls `session.execute` and never `retry::execute`. `crate::retry` has the whole
    // argument.
    let (mut session, _policy) = open(settings, engine, &config).await?;

    let mut warnings: Vec<String> = Vec::new();
    let mut cancelled = false;
    let mut query_id = None;
    // Rows the last writing statement reported. A `replace` runs a DROP first, which
    // reports nothing to count, so the value that survives is the CREATE's.
    let mut affected: Option<u64> = None;
    for (index, statement) in statements.iter().enumerate() {
        if index == 0 {
            // The connection is open and this statement is next: exactly what a
            // `write` step promises, and never reached if connect failed.
            out.emit(event("step").field("step", "write").build())?;
        } else if mode == "replace" && index == 1 {
            // The old table is gone from here on, whatever happens next, so the
            // caller is told even if the CREATE fails.
            warnings.push(format!("dropped the existing table {name}"));
        }
        if cancel.is_cancelled() {
            cancelled = true;
            let _ = session.cancel().await;
            break;
        }

        // Both steps, and the reason this is one block: a failure can arrive two
        // ways. `execute` refuses a statement that cannot be planned, and a
        // coordinator that accepted the statement reports a later failure on a page —
        // which is exactly what a CREATE ... AS SELECT against a missing table does.
        // Either way the warnings already earned travel, because nothing the caller
        // does afterwards can undo a DROP that has run.
        let outcome: Result<(), CliError> = async {
            let mut cursor = session
                .execute(
                    statement,
                    &ExecuteOptions {
                        statement_timeout: timeout,
                        ..ExecuteOptions::default()
                    },
                )
                .await?;
            // A DDL statement has no rows to read, but it is not finished when
            // `execute` returns: driving the cursor to its end is what waits for the
            // server to finish the work, and what the count arrives with.
            while cursor.next_batch(1_000).await?.is_some() {}
            if let Some(count) = cursor.affected_rows() {
                affected = Some(count);
            }
            Ok(())
        }
        .await;
        if let Err(error) = outcome {
            return Err(if warnings.is_empty() {
                error
            } else {
                CliError::Warned {
                    message: error.message(),
                    warnings,
                }
            });
        }
        query_id = session.query_id();
    }

    if cancelled && !warnings.contains(&CANCEL_WARNING.to_owned()) {
        warnings.push(CANCEL_WARNING.to_owned());
    }

    // The server's own count when it has one, and `-1` when it does not — which is the
    // value `exporter/to_table.py` used for the same silence, so a caller that has
    // learned to read it sees the same thing from both engines. A fabricated 0 would
    // read as "wrote nothing".
    let rows: i64 = match affected {
        Some(count) => i64::try_from(count).unwrap_or(i64::MAX),
        None => -1,
    };
    let progress = Progress::new(settings.number("PROGRESS_MS", PROGRESS_MS_DEFAULT)?);
    // The Python engine's own condition: it counts backwards from -1, so an
    // unreported write compares equal to itself and stays silent — a `progress`
    // carrying `-1` would read as a count of minus one row. A reported total always
    // gets its event, so `done` is never preceded by a stale one.
    if rows >= 0 && progress.last() != Some(rows as u64) {
        out.emit(
            event("progress")
                .field("rows", rows)
                .field("state", Json::Null)
                .build(),
        )?;
    }

    out.emit(
        event("done")
            .field("rows", rows)
            .field("table", name)
            .field("mode", mode)
            .field("query_id", query_id)
            .field("cancelled", cancelled)
            .field("warnings", warnings)
            .build(),
    )?;
    let _ = session.close().await;
    Ok(())
}

/// `table_op`: drop or truncate one table, through a confirmation.
///
/// The one command whose whole body is a single destructive statement the caller named:
/// `TABLE_OP=drop` sends `DROP TABLE <target>` and `TABLE_OP=truncate` sends
/// `TRUNCATE TABLE <target>`. The target is the same `TARGET_CATALOG` / `TARGET_SCHEMA` /
/// `TARGET_TABLE` triple `to_table` takes, resolved through the same slot rules, so a driver
/// is only asked for the parts it has (PostgreSQL has no catalog slot here, MySQL no schema).
///
/// The Safe Mode is checked **before** the connect step, against the statement this command
/// will actually send, through [`guard_destructive`] — the ordinary guard with one difference,
/// recorded in ADR-0027. A `read_only` or `no_ddl` connection therefore refuses without
/// opening one, and a `confirm` connection asks for `SAFE_MODE_CONFIRMED=1` first.
///
/// `rows` in `done` is the server's own affected count, or `-1` for a server that reported
/// none — never a fabricated zero, the rule [`to_table`] follows.
pub async fn table_op(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
    cancel: &CancelFlag,
) -> Result<(), CliError> {
    // The operation is read and refused by name before anything else: a command whose whole
    // body is one statement cannot guess which one the caller meant.
    let operation = settings.text("TABLE_OP", "");
    let operation = operation.trim().to_ascii_lowercase();
    let verb = match operation.as_str() {
        "drop" => "DROP TABLE",
        "truncate" => "TRUNCATE TABLE",
        "" => {
            return Err(CliError::Usage(
                "TABLE_OP is required: drop or truncate".to_owned(),
            ))
        }
        other => {
            return Err(CliError::Usage(format!(
                "unknown TABLE_OP '{other}'; expected drop or truncate"
            )))
        }
    };
    let timeout = statement_timeout(settings)?;
    let config = connection(settings, engine)?;
    let style = SlotStyle::of(config.kind);

    let targets = [
        ("TARGET_CATALOG", settings.text("TARGET_CATALOG", "")),
        ("TARGET_SCHEMA", settings.text("TARGET_SCHEMA", "")),
        ("TARGET_TABLE", settings.text("TARGET_TABLE", "")),
    ];
    // A part the driver has no level for is not required: `slots` is the list of the parts
    // this driver can actually use, exactly as `to_table` reads it.
    let target_of = |name: &str| -> String {
        targets
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.clone())
            .unwrap_or_default()
    };
    let missing: Vec<&str> = slots(style)
        .iter()
        .map(|slot| slot.target)
        .filter(|target| target_of(target).is_empty())
        .collect();
    if !missing.is_empty() {
        return Err(CliError::Usage(format!(
            "{} required to name the table",
            missing.join(" and ")
        )));
    }

    let target = qualified(
        style,
        &target_of("TARGET_CATALOG"),
        &target_of("TARGET_SCHEMA"),
        &target_of("TARGET_TABLE"),
    );
    let name = reference(
        style,
        &target_of("TARGET_CATALOG"),
        &target_of("TARGET_SCHEMA"),
        &target_of("TARGET_TABLE"),
    );
    let sql = format!("{verb} {target}");

    // The gate reads the statement this command will send, before the connect step, so a
    // read-only connection refuses without opening one.
    guard_destructive(settings, engine, &sql)?;

    out.emit(event("step").field("step", "connect").build())?;
    let (mut session, _policy) = open(settings, engine, &config).await?;
    out.emit(event("step").field("step", "write").build())?;

    let mut cancelled = false;
    let mut affected: Option<u64> = None;
    if cancel.is_cancelled() {
        cancelled = true;
        let _ = session.cancel().await;
    } else {
        // Not `retry::execute`: a `DROP`/`TRUNCATE` is not safe to re-issue blind, the same
        // rule `to_table` follows.
        let mut cursor = session
            .execute(
                &sql,
                &ExecuteOptions {
                    statement_timeout: timeout,
                    ..ExecuteOptions::default()
                },
            )
            .await?;
        // A DDL statement has no rows to read, but it is not finished when `execute`
        // returns: driving the cursor to its end is what waits for the server, and what the
        // count arrives with.
        while cursor.next_batch(1_000).await?.is_some() {}
        if let Some(count) = cursor.affected_rows() {
            affected = Some(count);
        }
    }
    let warnings: Vec<String> = if cancelled {
        vec![CANCEL_WARNING.to_owned()]
    } else {
        Vec::new()
    };

    let rows: i64 = match affected {
        Some(count) => i64::try_from(count).unwrap_or(i64::MAX),
        None => -1,
    };
    let progress = Progress::new(settings.number("PROGRESS_MS", PROGRESS_MS_DEFAULT)?);
    // The same condition `to_table` reports by: an unreported count (`-1`) stays silent
    // rather than becoming a `progress` that reads as a count of minus one row.
    if rows >= 0 && progress.last() != Some(rows as u64) {
        out.emit(
            event("progress")
                .field("rows", rows)
                .field("state", Json::Null)
                .build(),
        )?;
    }

    out.emit(
        event("done")
            .field("rows", rows)
            .field("table", name)
            .field("operation", operation)
            .field("query_id", session.query_id())
            .field("cancelled", cancelled)
            .field("warnings", warnings)
            .build(),
    )?;
    let _ = session.close().await;
    Ok(())
}

/// The two bounds a previewed page carries: how many rows the grid shows, and how long
/// the server may spend producing them.
///
/// One value rather than two arguments because they are always decided together, at the
/// top of `preview`, and both belong to the same question — how much of this statement
/// is worth paying for.
#[derive(Debug, Clone, Copy)]
struct PageBounds {
    limit: Option<u64>,
    timeout: Option<Duration>,
}

/// How long a Stop waits for the server to acknowledge a server-side cancel, and again for the
/// session to close. A server that cannot answer in that time is left to notice the dropped
/// socket on its own.
const STOP_BUDGET: Duration = Duration::from_millis(250);

/// Race `work` against a Stop: `None` when the stop won.
///
/// `work` is polled first, so a call that is already answered is never thrown away for a stop
/// that arrived at the same moment -- the page in hand is kept, as it always was.
async fn until_stopped<T>(
    cancel: &CancelFlag,
    work: impl std::future::Future<Output = T>,
) -> Option<T> {
    tokio::select! {
        biased;
        out = work => Some(out),
        () = cancel.cancelled() => None,
    }
}

/// Run `sql` like [`retry::execute`], but end the wait at a Stop: `None` when the stop won.
///
/// `grace` is how long a stop keeps waiting for the call to finish anyway. Trino's `execute`
/// returns once the coordinator has queued the statement, and only then does the session know
/// the query id its `cancel` needs; dropping the call earlier would orphan a query nothing
/// can name. The other drivers cancel by something they already hold, so they pass zero.
async fn execute_until_stopped(
    cancel: &CancelFlag,
    session: &mut Box<dyn Session>,
    policy: &RetryPolicy,
    sql: &str,
    options: &ExecuteOptions,
    grace: Duration,
) -> Option<Result<Box<dyn Cursor>, EngineError>> {
    let mut work = std::pin::pin!(retry::execute(session, policy, sql, options));
    tokio::select! {
        biased;
        out = &mut work => Some(out),
        () = cancel.cancelled() => {
            if !grace.is_zero() {
                let _ = tokio::time::timeout(grace, &mut work).await;
            }
            None
        }
    }
}

/// The grace a stop gives `execute` on this driver, see [`execute_until_stopped`].
///
/// MySQL needs none: its session publishes the connection id when it connects, the producer
/// publishes it again when a statement starts, and the id is never cleared, so `KILL QUERY`
/// always has an id to name.
fn stop_grace(config: &ConnectionConfig) -> Duration {
    if config.kind == DriverKind::Trino {
        STOP_BUDGET
    } else {
        Duration::ZERO
    }
}

/// What a `done` says when the server did not confirm a stop.
const STOP_UNCONFIRMED: &str =
    "the server did not confirm the stop; the statement may still be running";

/// How long the detached cancel and close may take before they are abandoned.
const STOP_CEILING: Duration = Duration::from_secs(10);

/// Tell the server to stop what this session is running, then close it.
///
/// Every path that leaves a cursor unfinished goes through here rather than through a bare
/// `close`: PostgreSQL's connection task would keep draining the pending response, MySQL's
/// dropped connection reads the rest of the result on the shared runtime, and a Trino query
/// would keep holding the coordinator. The cancel and the close run as a detached task, so a
/// slow `KILL` connection still completes; only the wait before the caller emits its `done` is
/// bounded by [`STOP_BUDGET`]. Returns the warning to put on that `done` when the server did
/// not confirm in time, or the cancel failed.
async fn stop_session(
    session: Box<dyn Session>,
    cursor: Option<Box<dyn Cursor>>,
) -> Option<String> {
    let confirmed = spawn_stop(session, cursor, false);
    let reason = match tokio::time::timeout(STOP_BUDGET, confirmed).await {
        Ok(Ok(Ok(()))) => return None,
        Ok(Ok(Err(error))) => format!("the cancel failed: {error}"),
        Ok(Err(_)) => "the cancel task ended without an answer".to_owned(),
        Err(_) => format!("no answer within {} ms", STOP_BUDGET.as_millis()),
    };
    eprintln!("queryhive-engine: {STOP_UNCONFIRMED} ({reason})");
    Some(STOP_UNCONFIRMED.to_owned())
}

/// [`stop_session`] for a result the cap cut short: nobody asked for a stop, so nothing waits
/// for it. The `done` goes out at once and a failure is only logged, because the app keeps the
/// tab running until the run returns and a cancel round trip would delay every capped Run.
fn stop_session_in_background(session: Box<dyn Session>, cursor: Option<Box<dyn Cursor>>) {
    drop(spawn_stop(session, cursor, true));
}

/// Cancel, then drop the cursor, then close, on a detached task; the receiver gets the cancel's outcome.
fn spawn_stop(
    session: Box<dyn Session>,
    cursor: Option<Box<dyn Cursor>>,
    log_failure: bool,
) -> tokio::sync::oneshot::Receiver<Result<(), String>> {
    let (answer, confirmed) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let cancelled = match tokio::time::timeout(STOP_CEILING, session.cancel()).await {
            Ok(result) => result.map_err(|error| error.to_string()),
            Err(_) => Err(format!("no answer within {} s", STOP_CEILING.as_secs())),
        };
        if let (true, Err(reason)) = (log_failure, &cancelled) {
            eprintln!("queryhive-engine: stopping a capped result failed ({reason})");
        }
        let _ = answer.send(cancelled);
        // Only now: MySQL's producer ends when its cursor is dropped, and a cancel sent after
        // that has nothing left to name.
        drop(cursor);
        let _ = tokio::time::timeout(STOP_CEILING, session.close()).await;
    });
    confirmed
}

/// `preview`: the first `LIMIT` rows of the caller's statement, as written.
///
/// The cap is enforced while pulling and never by rewriting the SQL: the statement
/// the cursor runs is the one the caller handed over, byte for byte, because a
/// rewritten statement can behave differently.
///
/// A Stop lands between rows rather than only at the end of the statement, and the
/// rows already pulled stay on the wire. `cancelled` is added to `done` only when a
/// stop actually arrived, so a run nobody stopped keeps the event shape the frozen
/// corpus recorded.
pub async fn preview(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
    cancel: &CancelFlag,
) -> Result<(), CliError> {
    let sql = source_sql(settings)?;
    // Checked before the connect step: a read-only connection refuses a write
    // without opening one.
    let mode = safe_mode(settings, engine)?;
    guard_for(settings, mode, &sql)?;
    let timeout = statement_timeout(settings)?;
    // Floored at one: a preview that returned no rows at all would tell the caller
    // nothing about the statement.
    let limit = settings.number("LIMIT", PREVIEW_LIMIT)?.max(1) as u64;
    let config = connection(settings, engine)?;
    let started = Instant::now();
    out.emit(event("step").field("step", "connect").build())?;
    let bounds = PageBounds {
        limit: Some(limit),
        timeout,
    };
    let streamed = stream_rows(settings, out, engine, &config, &sql, bounds, cancel).await?;
    let cancelled = streamed.cancelled;
    let done = event("done")
        .field("rows", streamed.rows)
        .field("truncated", streamed.truncated)
        .field("query_id", streamed.query_id)
        .field("elapsed_ms", started.elapsed().as_millis() as u64)
        .maybe("warnings", streamed.warning.map(|text| json!([text])));
    out.emit(if cancelled {
        done.field("cancelled", true).build()
    } else {
        done.build()
    })?;
    Ok(())
}

/// `explain`: the plan the server would use for the caller's statement.
///
/// The statement is built by the driver — `explain_statement`, because EXPLAIN's
/// spelling is per-driver — and emitted exactly as `preview` emits a result set,
/// because all three servers answer EXPLAIN with one.
pub async fn explain(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
    cancel: &CancelFlag,
) -> Result<(), CliError> {
    let sql = source_sql(settings)?;
    // Checked before the connect step: a read-only connection refuses a write
    // without opening one. The statement is the caller's own; the driver's `EXPLAIN`
    // prefix is added later and does not change what was asked.
    let mode = safe_mode(settings, engine)?;
    guard_for(settings, mode, &sql)?;
    let timeout = statement_timeout(settings)?;
    let config = connection(settings, engine)?;
    let started = Instant::now();
    out.emit(event("step").field("step", "connect").build())?;

    let Some(opened) = until_stopped(cancel, open(settings, engine, &config)).await else {
        return stopped_run(out, 0, None, None, None, started);
    };
    let (mut session, policy) = opened?;
    let statement = session.explain_statement(&sql);
    let executed = execute_until_stopped(
        cancel,
        &mut session,
        &policy,
        &statement,
        &ExecuteOptions {
            max_batch_rows: Some(PREVIEW_BATCH),
            row_limit: None,
            statement_timeout: timeout,
        },
        stop_grace(&config),
    )
    .await;
    let Some(cursor) = executed else {
        let query_id = session.query_id();
        let warning = stop_session(session, None).await;
        return stopped_run(out, 0, query_id, None, warning, started);
    };
    let mut cursor = cursor?;
    let Some(primed) = until_stopped(cancel, cursor.next_batch(PREVIEW_BATCH)).await else {
        let query_id = session.query_id();
        let warning = stop_session(session, Some(cursor)).await;
        return stopped_run(out, 0, query_id, None, warning, started);
    };
    let primed = primed?;
    // No row cap, so no `truncated` in `done`: a plan is a handful of rows and is
    // never cut short, and a field that is always false only invites someone to
    // branch on it.
    let (rows, _, cancelled) = emit_batches(out, &mut cursor, primed, None, Some(cancel)).await?;
    let query_id = session.query_id();
    let warning = if cancelled {
        stop_session(session, Some(cursor)).await
    } else {
        drop(cursor);
        let _ = session.close().await;
        None
    };
    let done = event("done")
        .field("rows", rows)
        .field("query_id", query_id)
        .field("elapsed_ms", started.elapsed().as_millis() as u64)
        .maybe("warnings", warning.map(|text| json!([text])));
    out.emit(if cancelled {
        done.field("cancelled", true).build()
    } else {
        done.build()
    })?;
    Ok(())
}

/// The `done` of a run that was stopped before it had anything to show.
fn stopped_run(
    out: &mut dyn Emitter,
    rows: u64,
    query_id: Option<String>,
    truncated: Option<bool>,
    warning: Option<String>,
    started: Instant,
) -> Result<(), CliError> {
    out.emit(
        event("done")
            .field("rows", rows)
            .maybe("truncated", truncated.map(Json::from))
            .field("query_id", query_id)
            .field("elapsed_ms", started.elapsed().as_millis() as u64)
            .maybe("warnings", warning.map(|text| json!([text])))
            .field("cancelled", true)
            .build(),
    )?;
    Ok(())
}

/// A result set as one `columns` event and batched `rows` events.
///
/// `preview` and `explain` share this: both put a database result set on the wire
/// for the grid to paint, and neither may grow a second copy of the batching rule.
/// They differ only in the cap, which is what `limit: None` means.
async fn stream_rows(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
    config: &ConnectionConfig,
    sql: &str,
    bounds: PageBounds,
    cancel: &CancelFlag,
) -> Result<Streamed, CliError> {
    let stopped = |query_id, warning| Streamed {
        rows: 0,
        truncated: false,
        query_id,
        cancelled: true,
        warning,
    };
    let Some(opened) = until_stopped(cancel, open(settings, engine, config)).await else {
        return Ok(stopped(None, None));
    };
    let (mut session, policy) = opened?;
    let executed = execute_until_stopped(
        cancel,
        &mut session,
        &policy,
        sql,
        &ExecuteOptions {
            // The batch size is also the fetch size, so a flush that happens
            // early never asks the coordinator for more rows than the cap shows.
            max_batch_rows: Some(PREVIEW_BATCH),
            row_limit: None,
            statement_timeout: bounds.timeout,
        },
        stop_grace(config),
    )
    .await;
    let Some(cursor) = executed else {
        let query_id = session.query_id();
        return Ok(stopped(query_id, stop_session(session, None).await));
    };
    let mut cursor = cursor?;
    // The first batch is taken before anything is sent, because `columns` is the one
    // event the grid cannot do without and a Trino cursor has none until a page
    // carrying them has arrived. This is the same wait the Python engine did before
    // it emitted the same event, and it is why the primed batch is handed on rather
    // than dropped: those rows are already fetched. A Stop ends this wait too: with
    // no page there is no `columns`, and the run reports only its `done`.
    let Some(primed) = until_stopped(cancel, cursor.next_batch(PREVIEW_BATCH)).await else {
        let query_id = session.query_id();
        return Ok(stopped(query_id, stop_session(session, Some(cursor)).await));
    };
    let primed = primed?;
    let (rows, truncated, cancelled) =
        emit_batches(out, &mut cursor, primed, bounds.limit, Some(cancel)).await?;
    let query_id = session.query_id();
    // A capped or stopped result leaves the statement unfinished on the server, so the
    // session is stopped rather than merely closed, see [`stop_session`].
    let warning = if cancelled {
        stop_session(session, Some(cursor)).await
    } else if truncated {
        stop_session_in_background(session, Some(cursor));
        None
    } else {
        drop(cursor);
        let _ = session.close().await;
        None
    };
    Ok(Streamed {
        rows,
        truncated,
        query_id,
        cancelled,
        warning,
    })
}

/// What [`stream_rows`] reports back for the `done` event.
struct Streamed {
    rows: u64,
    truncated: bool,
    query_id: Option<String>,
    cancelled: bool,
    /// Set when a stop was needed and the server did not confirm it.
    warning: Option<String>,
}

/// Send `columns`, then `rows` in `PREVIEW_BATCH` batches, honouring an optional cap.
///
/// The values are the writers' own canonical text (`qh-core::render::to_text`), so a
/// cell is always a string or `null`: `null` is how the grid renders a NULL, and a
/// decimal or a timestamp never becomes a native JSON number or a second JSON type
/// in the same column. The batches go out under `data`, never under `rows` — `rows`
/// is the integer count `done` carries, and one key cannot be two types.
///
/// The cap costs one row past it, pulled for the verdict and then dropped: a page
/// that ends exactly on the cap with more behind it is not the same result as one
/// that ends there because the query was done, and the grid's footer says "limit
/// reached" for one and "N rows" for the other. Asking for that row is
/// [`VERDICT_FETCH`] rows wide, never a page, so the verdict costs the same whether the
/// cap lands mid-page or exactly on the page boundary.
///
/// A stop is asked **before each fetch**, which is the same place `pump` asks it and
/// the only place it can cost the server anything: pages already received are handed
/// on, exactly as an export keeps the rows it already wrote. Asking per row instead
/// would drop a page that had already arrived, which is what a Stop pressed while the
/// first page was in flight would look like, and it buys nothing — formatting a row
/// this process already holds is not the expensive part of the wait.
async fn emit_batches(
    out: &mut dyn Emitter,
    cursor: &mut Box<dyn Cursor>,
    primed: Option<ColumnBatch>,
    limit: Option<u64>,
    cancel: Option<&CancelFlag>,
) -> Result<(u64, bool, bool), CliError> {
    out.emit(
        event("columns")
            .field("columns", columns_json(cursor.columns()))
            .build(),
    )?;

    let mut emitted: u64 = 0;
    let mut truncated = false;
    let mut cancelled = false;
    let mut pending: Vec<Json> = Vec::new();
    let mut next = primed;
    'outer: loop {
        // The Python loop's own condition: it is `emitted` — the flushed count — that
        // gates the next fetch, and a partial batch is not `emitted` yet.
        if let Some(limit) = limit {
            if emitted >= limit {
                // The cap was reached without a row to spare, so whether more exists has
                // to be asked: the grid's footer claims the whole result when nothing is
                // truncated, and a result that is exactly the cap is not the same as one
                // the cap cut short. One row, not a page: a cap that is a multiple of the
                // page size — the default `LIMIT=1000` against `PREVIEW_BATCH=200` — would
                // otherwise fetch a whole page to discard all but this row.
                truncated = match cancel {
                    Some(cancel) => {
                        match until_stopped(cancel, cursor.next_batch(VERDICT_FETCH)).await {
                            Some(fetched) => matches!(fetched?, Some(batch) if batch.rows() > 0),
                            // The probe for one more row was cut short after the cap was
                            // reached: the result was not shown to its end, so it is capped.
                            None => {
                                cancelled = true;
                                true
                            }
                        }
                    }
                    None => {
                        matches!(
                            cursor.next_batch(VERDICT_FETCH).await?,
                            Some(batch) if batch.rows() > 0
                        )
                    }
                };
                break;
            }
        }
        let batch = match next.take() {
            // The primed batch is handed on rather than dropped: it was fetched before
            // this loop existed, so a stop that arrived while it was in flight has
            // already been paid for.
            Some(batch) => batch,
            None => {
                // Asked before the fetch rather than before each row: this is the point
                // where waiting has a cost, and the batch already in hand is not dropped
                // for a stop that arrived while it was being fetched.
                if let Some(cancel) = cancel {
                    if cancel.is_cancelled() {
                        cancelled = true;
                        break 'outer;
                    }
                }
                let fetched = match cancel {
                    Some(cancel) => {
                        match until_stopped(cancel, cursor.next_batch(PREVIEW_BATCH)).await {
                            Some(fetched) => fetched?,
                            None => {
                                cancelled = true;
                                break 'outer;
                            }
                        }
                    }
                    None => cursor.next_batch(PREVIEW_BATCH).await?,
                };
                match fetched {
                    Some(batch) => batch,
                    None => break,
                }
            }
        };
        if batch.rows() == 0 {
            // An empty batch is not the end: a driver may legally return one while
            // it waits for rows.
            continue;
        }
        for index in 0..batch.rows() {
            if let Some(limit) = limit {
                if emitted + pending.len() as u64 >= limit {
                    // One row past the cap: the query had more to give, so the cap
                    // is what stopped this, not the end of the result. The row is
                    // the whole point of the fetch and is then discarded.
                    truncated = true;
                    break 'outer;
                }
            }
            pending.push(Json::Array(
                row_of(&batch, index)
                    .iter()
                    .map(|value| match qh_core::render::to_text(value) {
                        Some(text) => Json::String(text),
                        None => Json::Null,
                    })
                    .collect(),
            ));
            if pending.len() >= PREVIEW_BATCH {
                out.emit(
                    event("rows")
                        .field("data", Json::Array(std::mem::take(&mut pending)))
                        .build(),
                )?;
                emitted += PREVIEW_BATCH as u64;
            }
        }
    }
    if !pending.is_empty() {
        emitted += pending.len() as u64;
        out.emit(event("rows").field("data", Json::Array(pending)).build())?;
    }
    Ok((emitted, truncated, cancelled))
}

/// `count`: how many rows the caller's statement really returns.
///
/// The deliberate counterpart to `preview`'s page: the statement is wrapped in
/// `SELECT COUNT(*) FROM ( … ) AS queryhive_count`, so the grid's footer can say
/// "6000 rows" instead of "the first 1.000". That rewrite is the point of the
/// command and happens nowhere else — `preview` and `export` still run the caller's
/// SQL byte for byte.
pub async fn count(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
    cancel: &CancelFlag,
) -> Result<(), CliError> {
    let sql = source_sql(settings)?;
    // Checked before the connect step. The caller's statement is the one classified:
    // `count` wraps a `WITH … DELETE` in a `SELECT COUNT(*)` that would run it, so the
    // gate has to look at what was asked and not at the wrapper.
    let mode = safe_mode(settings, engine)?;
    guard_for(settings, mode, &sql)?;
    let timeout = statement_timeout(settings)?;
    let statement = qh_sql::count_statement(&sql)?;
    let config = connection(settings, engine)?;
    let started = Instant::now();
    out.emit(event("step").field("step", "connect").build())?;

    let stopped = |out: &mut dyn Emitter, warning: Option<String>| -> Result<(), CliError> {
        out.emit(
            event("done")
                .field("elapsed_ms", started.elapsed().as_millis() as u64)
                .maybe("warnings", warning.map(|text| json!([text])))
                .field("cancelled", true)
                .build(),
        )?;
        Ok(())
    };
    let Some(opened) = until_stopped(cancel, open(settings, engine, &config)).await else {
        return stopped(out, None);
    };
    let (mut session, policy) = opened?;
    let executed = execute_until_stopped(
        cancel,
        &mut session,
        &policy,
        &statement,
        &ExecuteOptions {
            statement_timeout: timeout,
            ..ExecuteOptions::default()
        },
        stop_grace(&config),
    )
    .await;
    let Some(cursor) = executed else {
        return stopped(out, stop_session(session, None).await);
    };
    let mut cursor = cursor?;
    let mut rows: Vec<Value> = Vec::new();
    // The count is one row; reading to the end anyway would be a second statement's
    // worth of waiting for a number that is already in hand.
    //
    // A timeout here is reported, not estimated. No driver in this workspace can give
    // a cheap approximate count — PostgreSQL's `reltuples`, MySQL's
    // `information_schema.TABLES.TABLE_ROWS` and Trino's `$partitions` all answer for
    // a *table* and not for an arbitrary statement, and this command counts a
    // statement — so the honest answer is that an estimate is not available and the
    // run failed. A fabricated number would be read as the count.
    let Some(first) = until_stopped(cancel, cursor.next_batch(PREVIEW_BATCH)).await else {
        return stopped(out, stop_session(session, Some(cursor)).await);
    };
    if let Some(batch) = first? {
        if batch.rows() > 0 {
            rows = row_of(&batch, 0);
        }
    }
    let _ = session.close().await;

    out.emit(event("count").field("rows", count_value(&rows)?).build())?;
    out.emit(
        event("done")
            .field("elapsed_ms", started.elapsed().as_millis() as u64)
            .build(),
    )?;
    Ok(())
}

/// The integer in the count cursor's first row, or a failure.
///
/// A count that did not come back is not a zero: an empty result set, a row with no
/// column, or a value that is not an integer is an error, so the caller is never
/// handed a number this process did not actually get. `cursor.rowcount` is never
/// consulted — it is `-1` on Trino, and this codebase has been careful never to turn
/// that into a count.
fn count_value(row: &[Value]) -> Result<i64, CliError> {
    match row.first() {
        None => Err(CliError::Query(
            "count query returned a row with no value".to_owned(),
        )),
        Some(Value::Int(count)) => Ok(*count),
        // MySQL's `COUNT(*)` is a signed bigint, but a server that answered with an
        // unsigned one answered with an integer all the same.
        Some(Value::UInt(count)) => i64::try_from(*count).map_err(|_| {
            CliError::Query(format!(
                "count query returned {count} rows, which is more than this engine can report"
            ))
        }),
        Some(other) => Err(CliError::Query(format!(
            "count query returned {}, not an integer",
            qh_core::render::to_text(other).unwrap_or_else(|| "NULL".to_owned())
        ))),
    }
}
