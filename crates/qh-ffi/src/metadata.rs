//! The read-only metadata commands: `columns`, `ddl` and `execution_log`, and the kinds `tables`
//! adds when `OBJECT_KINDS=1` (blueprint w11 section 3).
//!
//! # How a statement reaches the server
//!
//! The SQL belongs to the driver (`qh_driver::MetadataSql`, pure functions) and is **run here**,
//! through `Session::execute` like every other statement. That is the point of the split: the pooled
//! session wrapper, the retry layer and the Safe Mode tests all see these statements without a line
//! of special-casing, and a new method on `Session` that a wrapper forgot to forward cannot quietly
//! answer "unsupported" for every pooled run.
//!
//! [`run_text`] is the one place a metadata statement is sent, and it refuses to send one that is
//! not a single read-only statement under every lexer the dialect might be read with. The drivers
//! never build a statement from a name without quoting it, so this cannot fire for a name; it is the
//! check that stays true if somebody later writes a statement that does.
//!
//! # Events
//!
//! | Command | Event | Keys |
//! |---|---|---|
//! | `tables`, `OBJECT_KINDS=1` | `tables` | `names`, `kinds` (parallel; `null` for a kind the server did not say) |
//! | `columns` | `table_columns` | `object`, `fields`, `truncated` |
//! | `ddl` | `table_ddl` | `object`, `object_kind`, `ddl`, `truncated`, `redacted` |
//! | `execution_log` | `execution_log` | `decisions`, `chain`, `writer` |
//!
//! None of these keys is one `Event` in the app already uses for something else; the decoder
//! reads a key as one type only.

use std::ops::Range;
use std::time::Duration;

use qh_core::{EngineError, Value};
use qh_driver::{
    ColumnInfo, DriverKind, ExecuteOptions, MetadataSql, ObjectDdl, ObjectKind, ObjectPath, Rows,
    Session, Step, TableEntry,
};
use qh_sql::{decisions_readings, Dialect, SafeMode, StatementKind};
use qh_storage::{Storage, StorageError};
use serde_json::{json, Value as Json};

use crate::commands::{self, level_for, path_for};
use crate::env::Settings;
use crate::events::{event, Emitter};
use crate::execution_log;
use crate::local::{self, SharedStorage};
use crate::retry::{self, RetryPolicy};
use crate::sql_ident::{slots, Part, SlotStyle};
use crate::{CliError, Engine};

/// Rows `columns` returns at most. A catalog that big is cut, and the event says so.
const COLUMN_LIMIT: usize = 10_000;

/// Statements one DDL recipe may run.
const DDL_STEPS: usize = 8;

/// Rows one DDL statement may return.
const DDL_ROWS: usize = 5_000;

/// Decisions `execution_log` returns by default, and at most.
const LOG_DEFAULT: i64 = 200;
const LOG_MAX: i64 = 5_000;

fn dialect_of(kind: DriverKind) -> Dialect {
    match kind {
        DriverKind::Mysql => Dialect::Mysql,
        DriverKind::Postgres => Dialect::Postgres,
        DriverKind::Trino => Dialect::Trino,
    }
}

/// The text of a cell: a NULL is `None`, bytes are read as UTF-8 (`SHOW CREATE TABLE` answers in a
/// blob column on some servers), and everything else is the value's canonical text.
fn cell_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Bytes(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        other => qh_core::to_text(other),
    }
}

/// Send one metadata statement and read its answer as text.
///
/// `limit` caps the rows kept; the server is asked for one more, to know whether anything was cut,
/// and the second half of the pair says so. The statement bound is the caller's own
/// (`STATEMENT_TIMEOUT_MS`), so a metadata query cannot hold the one metadata session longer than
/// any other statement could.
async fn run_text(
    session: &mut Box<dyn Session>,
    policy: &RetryPolicy,
    dialect: Dialect,
    sql: &str,
    limit: Option<usize>,
    timeout: Option<Duration>,
) -> Result<(Rows, bool), EngineError> {
    // A single statement that every reading of the dialect calls a read.
    let decisions = decisions_readings(SafeMode::ReadOnly, sql, dialect.readings());
    if decisions.len() != 1 || decisions[0].kind != StatementKind::ReadOnly {
        return Err(EngineError::Internal {
            message:
                "a metadata statement was not a single read-only statement, so it was not sent"
                    .to_owned(),
        });
    }

    let options = ExecuteOptions {
        row_limit: limit.map(|limit| limit + 1),
        statement_timeout: timeout,
        ..ExecuteOptions::default()
    };
    let mut cursor = retry::execute(session, policy, sql, &options).await?;
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let mut truncated = false;
    // Driven to its end rather than dropped: the cursor stops itself at `row_limit`, and a cursor
    // that is left unfinished is one the server may still be sending to.
    while let Some(batch) = cursor.next_batch(1_000).await? {
        for row in 0..batch.rows() {
            if limit.is_some_and(|limit| rows.len() >= limit) {
                truncated = true;
                continue;
            }
            rows.push(
                (0..batch.width())
                    .map(|column| batch.value(row, column).and_then(cell_text))
                    .collect(),
            );
        }
    }
    let columns = cursor
        .columns()
        .iter()
        .map(|column| column.name.to_string())
        .collect();
    Ok((Rows { columns, rows }, truncated))
}

/// What a command names, read as the driver's own slots say.
struct Target {
    /// The path the driver's metadata statements read (MySQL's database is in `schema`, as
    /// `commands::path_for` has it).
    path: ObjectPath,
    /// The three-key `object` of the event: a part the driver has no level for is `null`.
    object: Json,
}

/// `TARGET_CATALOG`, `TARGET_SCHEMA` and `TARGET_TABLE`, the slots this driver uses, refused by name
/// before anything is opened when one is missing.
fn target_of(settings: &Settings, kind: DriverKind) -> Result<Target, CliError> {
    let style = SlotStyle::of(kind);
    let missing: Vec<&str> = slots(style)
        .iter()
        .map(|slot| slot.target)
        .filter(|target| settings.text(target, "").is_empty())
        .collect();
    if !missing.is_empty() {
        return Err(CliError::Usage(format!(
            "{} required to name the object",
            missing.join(" and ")
        )));
    }
    let mut path = ObjectPath::new();
    let (mut catalog, mut schema, mut table) = (Json::Null, Json::Null, Json::Null);
    for slot in slots(style) {
        let value = settings.text(slot.target, "");
        match slot.part {
            Part::Database => {
                catalog = Json::from(value.clone());
                if kind == DriverKind::Mysql {
                    path.schema = Some(value);
                } else {
                    path.catalog = Some(value);
                }
            }
            Part::Schema => {
                schema = Json::from(value.clone());
                path.schema = Some(value);
            }
            Part::Table => {
                table = Json::from(value.clone());
                path.table = Some(value);
            }
        }
    }
    Ok(Target {
        path,
        object: json!({ "catalog": catalog, "schema": schema, "table": table }),
    })
}

/// The driver's metadata statements, or a usage error naming the driver.
fn metadata_of(engine: &dyn Engine, kind: DriverKind) -> Result<&dyn MetadataSql, CliError> {
    engine
        .driver(kind)
        .metadata()
        .ok_or_else(|| CliError::Usage(format!("{kind} cannot describe its objects")))
}

fn kind_json(kind: Option<ObjectKind>) -> Json {
    kind.map_or(Json::Null, |kind| Json::from(kind.token()))
}

/// `tables` with `OBJECT_KINDS=1`: the same names, and a `kinds` array beside them.
///
/// A driver with no metadata statements answers the plain listing, with no `kinds` key at all: the
/// app reads a name with no kind as a table, exactly as it always has.
pub(crate) async fn tables(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
) -> Result<(), CliError> {
    let config = commands::connection(settings, engine)?;
    let Some(metadata) = engine.driver(config.kind).metadata() else {
        return commands::browse_tables(settings, out, engine).await;
    };
    // The same refusal a plain `tables` makes for a driver with no table level.
    level_for(
        engine,
        config.kind,
        &[qh_driver::BrowseLevel::Table],
        "table",
    )?;
    let sql = metadata.table_entries(&path_for(&config))?;
    let timeout = commands::statement_timeout(settings)?;
    let (mut session, policy) = commands::open(settings, engine, &config).await?;
    let answered = run_text(
        &mut session,
        &policy,
        dialect_of(config.kind),
        &sql,
        None,
        timeout,
    )
    .await;
    let _ = session.close().await;
    let (rows, _) = answered?;
    let entries: Vec<TableEntry> = metadata.parse_table_entries(&rows)?;
    out.emit(
        event("tables")
            .field(
                "names",
                entries
                    .iter()
                    .map(|entry| entry.name.clone())
                    .collect::<Vec<_>>(),
            )
            .field(
                "kinds",
                entries
                    .iter()
                    .map(|entry| kind_json(entry.kind))
                    .collect::<Vec<_>>(),
            )
            .build(),
    )?;
    Ok(())
}

fn column_json(column: &ColumnInfo) -> Json {
    json!({
        "name": column.name,
        "type": column.data_type,
        "nullable": column.nullable,
        "default": column.default,
        "extra": column.extra,
    })
}

/// `columns`: the columns of one object, with their types, nullability, defaults and extras.
pub async fn columns(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
) -> Result<(), CliError> {
    let config = commands::connection(settings, engine)?;
    let metadata = metadata_of(engine, config.kind)?;
    let target = target_of(settings, config.kind)?;
    let sql = metadata.columns(&target.path)?;
    let timeout = commands::statement_timeout(settings)?;

    let (mut session, policy) = commands::open(settings, engine, &config).await?;
    let answered = run_text(
        &mut session,
        &policy,
        dialect_of(config.kind),
        &sql,
        Some(COLUMN_LIMIT),
        timeout,
    )
    .await;
    let _ = session.close().await;
    let (rows, truncated) = answered?;
    let fields: Vec<Json> = metadata
        .parse_columns(&rows)?
        .iter()
        .map(column_json)
        .collect();
    out.emit(
        event("table_columns")
            .field("object", target.object)
            .field("fields", fields)
            .field("truncated", truncated)
            .build(),
    )?;
    Ok(())
}

/// Run a DDL recipe to its end over one session.
///
/// A recipe that asks for more than [`DDL_STEPS`] statements is a defect in the recipe, not a limit
/// the user hit, and is reported as one. A statement that fails is offered to the recipe first,
/// because one recipe (Trino's) has a second way to ask after one particular refusal.
async fn run_recipe(
    session: &mut Box<dyn Session>,
    policy: &RetryPolicy,
    dialect: Dialect,
    mut recipe: Box<dyn qh_driver::DdlRecipe>,
    timeout: Option<Duration>,
) -> Result<ObjectDdl, EngineError> {
    let mut step = recipe.next(None)?;
    let mut truncated = false;
    for _ in 0..DDL_STEPS {
        let sql = match step {
            Step::Done(mut ddl) => {
                ddl.truncated |= truncated;
                return Ok(ddl);
            }
            Step::Query(sql) => sql,
        };
        step = match run_text(session, policy, dialect, &sql, Some(DDL_ROWS), timeout).await {
            Ok((rows, cut)) => {
                truncated |= cut;
                recipe.next(Some(rows))?
            }
            Err(error) => recipe.recover(&error).ok_or(error)?,
        };
    }
    match step {
        Step::Done(mut ddl) => {
            ddl.truncated |= truncated;
            Ok(ddl)
        }
        Step::Query(_) => Err(EngineError::Internal {
            message: format!("a DDL recipe asked for more than {DDL_STEPS} statements"),
        }),
    }
}

/// `ddl`: the DDL of one object, with credentials the server printed into it masked.
pub async fn ddl(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
) -> Result<(), CliError> {
    let config = commands::connection(settings, engine)?;
    let metadata = metadata_of(engine, config.kind)?;
    let target = target_of(settings, config.kind)?;
    let recipe = metadata.ddl(&target.path)?;
    let timeout = commands::statement_timeout(settings)?;

    let (mut session, policy) = commands::open(settings, engine, &config).await?;
    let built = run_recipe(
        &mut session,
        &policy,
        dialect_of(config.kind),
        recipe,
        timeout,
    )
    .await;
    let _ = session.close().await;
    let built = built?;
    let (text, redacted) = redact(&built.text);
    out.emit(
        event("table_ddl")
            .field("object", target.object)
            .field("object_kind", kind_json(built.kind))
            .field("ddl", text)
            .field("truncated", built.truncated)
            .field("redacted", redacted)
            .build(),
    )?;
    Ok(())
}

/// The option names whose values are credentials, in any case.
const SECRET_OPTIONS: [&str; 5] = ["password", "passwd", "secret", "token", "apikey"];

const MASK: &str = "***";

/// Mask the credentials a server prints into a DDL, and say whether any were found.
///
/// Two shapes, because they are the two a server is known to print one in: a MySQL `FEDERATED`
/// table's `CONNECTION='scheme://user:password@host/…'`, and a `password`, `passwd`, `secret`,
/// `token` or `apikey` option in an `OPTIONS (…)` list. Best effort, and said so in the ADR: a view
/// definition can hold a literal its owner wrote, which is that owner's data and not a connection
/// credential, and nothing here tries to find it. It errs towards masking: it does not track string
/// literals, so the words `OPTIONS (password '…')` inside one would be masked too.
pub(crate) fn redact(ddl: &str) -> (String, bool) {
    let bytes = ddl.as_bytes();
    let lower = ddl.to_ascii_lowercase();
    let mut spans: Vec<Range<usize>> = Vec::new();

    for open in keyword_ends(&lower, "connection") {
        let after = skip_space(bytes, open);
        if bytes.get(after) != Some(&b'=') {
            continue;
        }
        let quote = skip_space(bytes, after + 1);
        if !matches!(bytes.get(quote), Some(b'\'' | b'"')) {
            continue;
        }
        let end = literal_end(bytes, quote);
        if let Some(span) = url_password(ddl, quote + 1..end) {
            spans.push(span);
        }
    }

    for open in keyword_ends(&lower, "options") {
        let paren = skip_space(bytes, open);
        if bytes.get(paren) != Some(&b'(') {
            continue;
        }
        let mut at = paren + 1;
        let mut depth = 1;
        while at < bytes.len() && depth > 0 {
            match bytes[at] {
                b'\'' => at = literal_end(bytes, at) + 1,
                b'(' => {
                    depth += 1;
                    at += 1;
                }
                b')' => {
                    depth -= 1;
                    at += 1;
                }
                byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                    let start = at;
                    while at < bytes.len()
                        && (bytes[at].is_ascii_alphanumeric() || bytes[at] == b'_')
                    {
                        at += 1;
                    }
                    let value = skip_space(bytes, at);
                    if SECRET_OPTIONS.contains(&&lower[start..at])
                        && bytes.get(value) == Some(&b'\'')
                    {
                        let end = literal_end(bytes, value);
                        spans.push(value + 1..end);
                        at = end + 1;
                    }
                }
                _ => at += 1,
            }
        }
    }

    if spans.is_empty() {
        return (ddl.to_owned(), false);
    }
    spans.sort_by_key(|span| span.start);
    let mut masked = String::with_capacity(ddl.len());
    let mut from = 0;
    for span in spans {
        if span.start < from {
            continue;
        }
        masked.push_str(&ddl[from..span.start]);
        masked.push_str(MASK);
        from = span.end;
    }
    masked.push_str(&ddl[from..]);
    (masked, true)
}

/// Where each whole-word occurrence of `word` ends in `lower`.
fn keyword_ends(lower: &str, word: &str) -> Vec<usize> {
    let bytes = lower.as_bytes();
    let boundary = |index: Option<usize>| {
        index
            .and_then(|index| bytes.get(index))
            .is_none_or(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
    };
    lower
        .match_indices(word)
        .filter(|(start, _)| boundary(start.checked_sub(1)) && boundary(Some(start + word.len())))
        .map(|(start, _)| start + word.len())
        .collect()
}

fn skip_space(bytes: &[u8], mut at: usize) -> usize {
    while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

/// The index of the quote that closes the literal opened at `open`, a doubled quote being an
/// escaped one. The end of the text when it never closes.
fn literal_end(bytes: &[u8], open: usize) -> usize {
    let quote = bytes[open];
    let mut at = open + 1;
    while at < bytes.len() {
        if bytes[at] == quote {
            if bytes.get(at + 1) == Some(&quote) {
                at += 2;
                continue;
            }
            return at;
        }
        at += 1;
    }
    bytes.len()
}

/// The password inside a `scheme://user:password@host/…` URL, as a span of `text`.
fn url_password(text: &str, within: Range<usize>) -> Option<Range<usize>> {
    let url = &text[within.clone()];
    let authority_start = url.find("://")? + 3;
    let authority_end = url[authority_start..]
        .find('/')
        .map_or(url.len(), |slash| authority_start + slash);
    let authority = &url[authority_start..authority_end];
    let userinfo = &authority[..authority.rfind('@')?];
    let colon = userinfo.find(':')?;
    let start = within.start + authority_start + colon + 1;
    let end = within.start + authority_start + userinfo.len();
    (end > start).then_some(start..end)
}

/// `execution_log`: the recent Safe Mode decisions, and whether the chain still verifies.
///
/// Read through the sink the engine writes with ([`execution_log::ensure_sink`]), so the reader and
/// the writer are one database and one handle. Only when the sink cannot be opened does it open the
/// database for itself, and then it says `writer: false`: what is shown is what was written
/// earlier, and nothing being decided now is being added to it.
///
/// A chain that does not verify is reported and the rows are **still** returned, because seeing the
/// rows is how it gets diagnosed.
pub async fn execution_log(
    settings: &Settings,
    out: &mut dyn Emitter,
    shared: Option<SharedStorage>,
) -> Result<(), CliError> {
    let limit = settings.number("EXECUTION_LOG_LIMIT", LOG_DEFAULT)?;
    if !(1..=LOG_MAX).contains(&limit) {
        return Err(CliError::Usage(format!(
            "EXECUTION_LOG_LIMIT must be between 1 and {LOG_MAX}"
        )));
    }
    let limit = usize::try_from(limit).unwrap_or(1);
    let verify = settings.flag("EXECUTION_LOG_VERIFY", true);

    let settings = settings.clone();
    let built = tokio::task::spawn_blocking(move || -> Result<Json, CliError> {
        let written = if execution_log::ensure_sink(&settings).is_ok() {
            execution_log::with_storage(|storage| read_log(storage, limit, verify))
        } else {
            // `ensure_sink` has said on stderr why it could not open the log. Opening the same
            // database to read it is a second attempt at the same thing, and is the answer only
            // for a failure that was not the file's own.
            None
        };
        let (log, writer) = match written {
            Some(log) => (log, true),
            None => {
                let storage = local::acquire(&settings, shared.as_ref())?;
                (read_log(&storage, limit, verify), false)
            }
        };
        let (decisions, chain) = log?;
        Ok(event("execution_log")
            .field("decisions", decisions)
            .field("chain", chain)
            .field("writer", writer)
            .build())
    })
    .await
    .map_err(|error| CliError::Internal(error.to_string()))??;
    out.emit(built)?;
    Ok(())
}

/// The decisions as JSON, newest first, and the chain's verdict.
fn read_log(storage: &Storage, limit: usize, verify: bool) -> Result<(Vec<Json>, Json), CliError> {
    let decisions = storage
        .execution_log(limit)?
        .iter()
        .map(|record| {
            json!({
                "seq": record.seq,
                "id": record.id.as_str(),
                "at": record.at,
                "safe_mode": record.safe_mode,
                "decision": record.decision,
                "statement_kind": record.statement_kind,
                "statement_index": record.statement_index,
                "statement_hash": record.statement_hash,
                "reason": record.reason,
            })
        })
        .collect();
    let chain = if !verify {
        json!({ "verified": null })
    } else {
        match storage.verify_execution_log() {
            Ok(rows) => json!({ "verified": true, "rows": rows }),
            Err(error @ StorageError::BrokenExecutionChain { seq, .. }) => {
                json!({ "verified": false, "seq": seq, "detail": error.to_string() })
            }
            Err(other) => return Err(other.into()),
        }
    };
    Ok((decisions, chain))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_federated_table_loses_the_password_in_its_connection_url() {
        let ddl = "CREATE TABLE `t` (\n  `id` int\n) ENGINE=FEDERATED DEFAULT CHARSET=utf8mb4 \
                   CONNECTION='mysql://app:s3cret@db.internal:3306/shop/orders'";
        let (masked, redacted) = redact(ddl);
        assert!(redacted);
        assert!(
            masked.contains("CONNECTION='mysql://app:***@db.internal:3306/shop/orders'"),
            "{masked}"
        );
        assert!(!masked.contains("s3cret"));
    }

    #[test]
    fn a_connection_url_without_a_password_is_left_alone() {
        for url in [
            "mysql://app@db/shop/t",
            "mysql://db/shop/t",
            "mysql://app:@db/shop/t",
        ] {
            let ddl = format!("CREATE TABLE t (id int) CONNECTION='{url}'");
            assert_eq!(redact(&ddl), (ddl.clone(), false), "{url}");
        }
    }

    #[test]
    fn foreign_table_options_lose_a_secret_under_any_spelling() {
        let ddl = "CREATE FOREIGN TABLE f (id int) SERVER s OPTIONS (schema_name 'public', \
                   Password 'p1', PASSWD 'p2', secret 'p3', token 'p4', apikey 'p5', table_name 'it''s')";
        let (masked, redacted) = redact(ddl);
        assert!(redacted);
        for leaked in ["p1", "p2", "p3", "p4", "p5"] {
            assert!(!masked.contains(leaked), "{leaked} in {masked}");
        }
        assert!(masked.contains("schema_name 'public'"), "{masked}");
        assert!(masked.contains("table_name 'it''s'"), "{masked}");
        assert_eq!(masked.matches("***").count(), 5);
    }

    #[test]
    fn a_column_called_password_is_not_a_credential() {
        let ddl =
            "CREATE TABLE users (\n    password text NOT NULL,\n    token text DEFAULT 'abc'\n);\n";
        assert_eq!(redact(ddl), (ddl.to_owned(), false));
    }

    #[test]
    fn an_unterminated_literal_does_not_hang_or_panic() {
        for ddl in [
            "CONNECTION='mysql://a:b",
            "OPTIONS (password 'abc",
            "OPTIONS ('",
            "CONNECTION=",
        ] {
            let _ = redact(ddl);
        }
    }

    #[test]
    fn bytes_that_are_text_read_as_text() {
        assert_eq!(
            cell_text(&Value::Bytes(b"CREATE TABLE t".to_vec())).as_deref(),
            Some("CREATE TABLE t")
        );
        assert_eq!(cell_text(&Value::Null), None);
        assert_eq!(cell_text(&Value::Int(7)).as_deref(), Some("7"));
    }
}
