//! The eleven commands.
//!
//! One function per command, each a transcription of the Python engine's own
//! (`queryhive_engine.py:453-935`) rather than a reinterpretation of it: the event
//! names, the fields, and **the order the events go out in** are the contract the
//! app decodes and the snapshots freeze. Where a rule is subtle, the comment says
//! which Python line it comes from and why it is that way, because these are the
//! decisions the migration has to preserve rather than improve.
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
use std::time::Instant;

use qh_core::{ColumnBatch, ColumnMeta, Value};
use qh_driver::{
    BrowseLevel, ConnectionConfig, Cursor, DriverKind, ExecuteOptions, ObjectPath, Session,
};
use qh_export::plan::{ExportSpec, Exporter};
use qh_export::{ExportOptions, Format};
use serde_json::{json, Value as Json};

use crate::config;
use crate::env::Settings;
use crate::events::{event, Emitter};
use crate::progress::{Progress, PROGRESS_MS_DEFAULT};
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
fn connection(settings: &Settings, engine: &dyn Engine) -> Result<ConnectionConfig, CliError> {
    let mut config = config::build(settings)?;
    if config.port == 0 {
        config.port = engine.driver(config.kind).default_port();
    }
    Ok(config)
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
        .ok_or_else(|| CliError::Usage(format!("{kind} has no {level_name} level")))
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
fn expand_user(path: &str) -> PathBuf {
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
    // `ENCODING` and `DBF_ENCODING` are read by the Python engine and are accepted
    // here without effect: every writer in this engine is UTF-8, and the dBase
    // writer's own code page is fixed at cp1252. They are still not *errors*, so a
    // stored connection carrying them keeps working — see docs/golden-deltas.md.
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
/// the shape of `Session::close`, so that a driver cannot be left half-open.
async fn browse_session(
    mut session: Box<dyn Session>,
    level: BrowseLevel,
    path: &ObjectPath,
    include_system: bool,
) -> Result<Vec<String>, CliError> {
    let names = session.browse(level, path, include_system).await?;
    // Best effort: a close that fails after the names are in hand must not turn a
    // successful listing into an error the user sees.
    let _ = session.close().await;
    Ok(names)
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
    let session = engine.connect(&config).await?;
    let level = session
        .capabilities()
        .levels
        .first()
        .copied()
        .ok_or_else(|| CliError::Usage(format!("{} has no top level to list", config.kind)))?;
    let names = browse_session(session, level, &path_for(&config), false).await?;
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
    let session = engine.connect(&config).await?;
    let names = browse_session(session, level, &path_for(&config), include_system).await?;
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
    let mut session = engine.connect(&config).await?;
    let page = session.objects(&path_for(&config)).await?;
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
    let mut session = engine.connect(&config).await?;
    let mut cursor = session
        .execute(
            &sql,
            &ExecuteOptions {
                max_batch_rows: Some(batch_size),
                row_limit: None,
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

    out.emit(
        event("done")
            .field("rows", outcome.rows)
            .field("files", entries)
            .field("warnings", outcome.warnings)
            .field("query_id", session.query_id())
            .field("cancelled", outcome.cancelled)
            .build(),
    )?;
    let _ = session.close().await;
    Ok(())
}

/// The export's row loop.
///
/// Cancellation is asked **before each row**, so a row that arrives while the user
/// is pressing Cancel is not half-written into the file, and the file keeps the
/// rows that were already written. The 1000-row progress floor is not here: it
/// lives in `qh-export::plan`, which the synchronous export path uses too, so both
/// callers report at the same points.
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

    out.emit(event("step").field("step", "connect").build())?;
    let mut session = engine.connect(&config).await?;

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
                .execute(statement, &ExecuteOptions::default())
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

/// `preview`: the first `LIMIT` rows of the caller's statement, as written.
///
/// The cap is enforced while pulling and never by rewriting the SQL: the statement
/// the cursor runs is the one the caller handed over, byte for byte, because a
/// rewritten statement can behave differently.
pub async fn preview(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
) -> Result<(), CliError> {
    let sql = source_sql(settings)?;
    // Floored at one: a preview that returned no rows at all would tell the caller
    // nothing about the statement.
    let limit = settings.number("LIMIT", PREVIEW_LIMIT)?.max(1) as u64;
    let config = connection(settings, engine)?;
    let started = Instant::now();
    out.emit(event("step").field("step", "connect").build())?;
    let (rows, truncated, query_id) = stream_rows(out, engine, &config, &sql, Some(limit)).await?;
    out.emit(
        event("done")
            .field("rows", rows)
            .field("truncated", truncated)
            .field("query_id", query_id)
            .field("elapsed_ms", started.elapsed().as_millis() as u64)
            .build(),
    )?;
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
) -> Result<(), CliError> {
    let sql = source_sql(settings)?;
    let config = connection(settings, engine)?;
    let started = Instant::now();
    out.emit(event("step").field("step", "connect").build())?;

    let mut session = engine.connect(&config).await?;
    let statement = session.explain_statement(&sql);
    let mut cursor = session
        .execute(
            &statement,
            &ExecuteOptions {
                max_batch_rows: Some(PREVIEW_BATCH),
                row_limit: None,
            },
        )
        .await?;
    let primed = cursor.next_batch(PREVIEW_BATCH).await?;
    // No row cap, so no `truncated` in `done`: a plan is a handful of rows and is
    // never cut short, and a field that is always false only invites someone to
    // branch on it.
    let (rows, _) = emit_batches(out, cursor, primed, None).await?;
    out.emit(
        event("done")
            .field("rows", rows)
            .field("query_id", session.query_id())
            .field("elapsed_ms", started.elapsed().as_millis() as u64)
            .build(),
    )?;
    let _ = session.close().await;
    Ok(())
}

/// A result set as one `columns` event and batched `rows` events.
///
/// `preview` and `explain` share this: both put a database result set on the wire
/// for the grid to paint, and neither may grow a second copy of the batching rule.
/// They differ only in the cap, which is what `limit: None` means.
async fn stream_rows(
    out: &mut dyn Emitter,
    engine: &dyn Engine,
    config: &ConnectionConfig,
    sql: &str,
    limit: Option<u64>,
) -> Result<(u64, bool, Option<String>), CliError> {
    let mut session = engine.connect(config).await?;
    let mut cursor = session
        .execute(
            sql,
            &ExecuteOptions {
                // The batch size is also the fetch size, so a flush that happens
                // early never asks the coordinator for more rows than the cap shows.
                max_batch_rows: Some(PREVIEW_BATCH),
                row_limit: None,
            },
        )
        .await?;
    // The first batch is taken before anything is sent, because `columns` is the one
    // event the grid cannot do without and a Trino cursor has none until a page
    // carrying them has arrived. This is the same wait the Python engine did before
    // it emitted the same event, and it is why the primed batch is handed on rather
    // than dropped: those rows are already fetched.
    let primed = cursor.next_batch(PREVIEW_BATCH).await?;
    let (rows, truncated) = emit_batches(out, cursor, primed, limit).await?;
    let query_id = session.query_id();
    let _ = session.close().await;
    Ok((rows, truncated, query_id))
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
/// reached" for one and "N rows" for the other.
async fn emit_batches(
    out: &mut dyn Emitter,
    mut cursor: Box<dyn Cursor>,
    primed: Option<ColumnBatch>,
    limit: Option<u64>,
) -> Result<(u64, bool), CliError> {
    out.emit(
        event("columns")
            .field("columns", columns_json(cursor.columns()))
            .build(),
    )?;

    let mut emitted: u64 = 0;
    let mut truncated = false;
    let mut pending: Vec<Json> = Vec::new();
    let mut next = primed;
    'outer: loop {
        // The Python loop's own condition: it is `emitted` — the flushed count — that
        // gates the next fetch, and a partial batch is not `emitted` yet.
        if let Some(limit) = limit {
            if emitted >= limit {
                break;
            }
        }
        let batch = match next.take() {
            Some(batch) => batch,
            None => match cursor.next_batch(PREVIEW_BATCH).await? {
                Some(batch) => batch,
                None => break,
            },
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
    Ok((emitted, truncated))
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
) -> Result<(), CliError> {
    let sql = source_sql(settings)?;
    let statement = qh_sql::count_statement(&sql)?;
    let config = connection(settings, engine)?;
    let started = Instant::now();
    out.emit(event("step").field("step", "connect").build())?;

    let mut session = engine.connect(&config).await?;
    let mut cursor = session
        .execute(&statement, &ExecuteOptions::default())
        .await?;
    let mut rows: Vec<Value> = Vec::new();
    // The count is one row; reading to the end anyway would be a second statement's
    // worth of waiting for a number that is already in hand.
    if let Some(batch) = cursor.next_batch(PREVIEW_BATCH).await? {
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
