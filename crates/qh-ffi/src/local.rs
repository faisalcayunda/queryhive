//! The local commands: the connection store, its legacy import, the password, the query
//! history, the saved queries, and the session.
//!
//! None of them opens a driver, and none of them is a transcription of the Python engine
//! — it had no local store at all. What they have instead is a contract with the app:
//! `connections` is what the sidebar is painted from, `import_connections` is the
//! one-shot move of the old `connections.json` into the database, and `credential` is the
//! only path by which a saved password leaves the Keychain. `history`, `history_add`,
//! `history_clear` and `saved_queries` are what the History and Saved panels read, and
//! `session` is what brings the tabs back after a relaunch.
//!
//! # Why these are driven by settings and not by arguments
//!
//! Every command here takes its input from the environment, exactly like the
//! driver-facing ones, because `run` is also the FFI surface's entry point and the app
//! sets settings rather than building an argv. Two of them are overrides that exist so
//! the commands can be exercised without touching what the user owns:
//!
//! | Setting | Default |
//! |---|---|
//! | `DB_PATH` | the Application Support database |
//! | `LEGACY_PATH` | the Application Support `connections.json` |
//!
//! # Blocking work runs on a blocking thread
//!
//! SQLite calls through `qh-storage` and Keychain calls through `qh-credentials` both
//! block, and this engine's rule is that neither happens on a tokio worker — the same
//! reason the workspace's `rusqlite` note exists. [`on_blocking`] is where that happens,
//! and the emitter stays on this side: an event is never written from a thread the
//! runtime does not know about.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use qh_credentials::{account_key, KeychainStore, MemoryStore, SecretStore};
use qh_storage::import::{self, ImportReport};
use qh_storage::{ConnectionRecord, Outcome, QueryHistoryRecord, SavedQueryRecord, Storage};
use qh_sync::SyncId;
use secrecy::{ExposeSecret, SecretString};
use serde_json::{json, Value as Json};

use crate::commands::expand_user;
use crate::env::Settings;
use crate::events::{event, Emitter};
use crate::CliError;

/// `connections`: every living connection, as the sidebar shows them.
///
/// Deleted rows are not listed — `Storage::connections` is the living set — and the shape
/// still carries `deleted`, so a decoder written here keeps working when a command that
/// includes tombstones arrives.
pub async fn connections(settings: &Settings, out: &mut dyn Emitter) -> Result<(), CliError> {
    let settings = settings.clone();
    let built = on_blocking(move || {
        let listed = open_storage(&settings)?
            .connections()?
            .iter()
            .map(connection_json)
            .collect::<Vec<_>>();
        Ok(event("connections").field("connections", listed).build())
    })
    .await?;
    out.emit(built)?;
    Ok(())
}

/// `import_connections`: bring the legacy `connections.json` into the database.
///
/// A file that is not there is a normal outcome (`source_found:false`) and not an error:
/// a machine where the app never saved a connection has nothing to import, and a command
/// that failed on every launch would be a command nobody could leave in the startup path.
/// Everything else — an unreadable file, a backup that could not be made, rows that did
/// not read back — is a real failure and arrives as an `error` event.
pub async fn import_connections(
    settings: &Settings,
    out: &mut dyn Emitter,
) -> Result<(), CliError> {
    let settings = settings.clone();
    let built = on_blocking(move || {
        let storage = open_storage(&settings)?;
        let source = legacy_source(&settings);
        let Some(source) = source else {
            // No `LEGACY_PATH` and no Application Support directory to look in — there is
            // no path to report, which is what the `null` says.
            return Ok(import_event(None, false, None));
        };
        if !source.is_file() {
            return Ok(import_event(Some(&source), false, None));
        }
        // One timestamp for the whole import, so the backup's name and the rows' revision
        // are stamped from the same reading of the clock.
        let at = qh_storage::now_millis();
        let plan = import::plan_file(&source, at)?;
        let report = import::import_connections(&storage, &plan, at)?;
        Ok(import_event(Some(&source), true, Some(&report)))
    })
    .await?;
    out.emit(built)?;
    Ok(())
}

/// `credential`: read, write or remove the password of one connection.
///
/// `CREDENTIAL_ACTION` names the action and `CONNECTION_ID` names the account — the
/// connection's UUID, which is the Keychain account the app has always used. The event
/// carries the action, the account and whether an item exists; only `get` carries the
/// secret itself, and only because that is what `get` is for.
///
/// `has` and `get` both answer `exists`, and they are two commands rather than one with a
/// flag because the app asks "is a password saved" far more often than it asks for the
/// password — on every sidebar paint, versus once per connection.
pub async fn credential(settings: &Settings, out: &mut dyn Emitter) -> Result<(), CliError> {
    let action = action_of(settings)?;
    let requested = settings.text("CONNECTION_ID", "");
    if requested.is_empty() {
        return Err(CliError::Usage("CONNECTION_ID is required".to_owned()));
    }
    // Read before the task starts: a `set` with no password is the caller's own mistake,
    // and it is refused before anything is written anywhere.
    let secret = match action.as_str() {
        "set" => Some(password_of(settings)?),
        _ => None,
    };

    let settings = settings.clone();
    let built = on_blocking(move || {
        let store = store_for(&settings);
        // The store folds a UUID to one spelling before it names an item; the event
        // echoes the caller's own spelling, which is the one they will send again.
        let key = account_key(&requested);
        let (exists, revealed) = match action.as_str() {
            "set" => {
                // `set` is the one action that carried a secret, and the check above is
                // what put it here.
                store.set(&key, secret.as_ref().expect("a set has a secret"))?;
                (true, None)
            }
            "delete" => {
                store.delete(&key)?;
                (false, None)
            }
            // `contains`, not `get(..).is_some()`: the UI asks this on every sidebar paint,
            // and answering it by loading the password would pull a secret out of the
            // Keychain — through whatever prompt the item's access control raises — only to
            // throw it away. The store's attributes-only query exists for this line.
            "has" => (store.contains(&key)?, None),
            _ => {
                let found = store.get(&key)?;
                // The secret is exposed here and nowhere else, at the one call site whose
                // whole purpose is to hand it to the caller.
                let revealed = found
                    .as_ref()
                    .map(|secret| Json::from(secret.expose_secret()))
                    .unwrap_or(Json::Null);
                (found.is_some(), Some(revealed))
            }
        };

        let mut built = event("credential")
            .field("action", action.as_str())
            .field("account", requested.as_str())
            .field("exists", exists);
        if let Some(revealed) = revealed {
            built = built.field("secret", revealed);
        }
        Ok(built.build())
    })
    .await?;
    out.emit(built)?;
    Ok(())
}

/// `history`: the executions this engine has written down.
///
/// `HISTORY_LIMIT` caps the list and `CONNECTION_ID` narrows it to one connection. Both are
/// optional: the common question is "what did I just run", and it names no connection.
pub async fn history(settings: &Settings, out: &mut dyn Emitter) -> Result<(), CliError> {
    let settings = settings.clone();
    let built = on_blocking(move || {
        let limit = history_limit(&settings)?;
        let filter = connection_filter(&settings)?;
        // Blank means "no search", the same convention `CONNECTION_ID` follows, so the app can set
        // the key unconditionally as it clears the field.
        let search = settings.text("HISTORY_SEARCH", "");
        let storage = open_storage(&settings)?;
        let entries = if search.trim().is_empty() {
            match filter {
                Some(id) => storage.history_for_connection(&id, limit)?,
                None => storage.history(limit)?,
            }
        } else {
            storage.search_history(&search, filter.as_ref(), limit)?
        };
        let listed = entries.iter().map(history_json).collect::<Vec<_>>();
        Ok(event("history").field("entries", listed).build())
    })
    .await?;
    out.emit(built)?;
    Ok(())
}

/// `history_add`: write down one execution.
///
/// `STARTED_AT` is the event's own time and defaults to now. It is the column the dedupe
/// index is on, so a caller replaying a recorded run has to pass the original moment, not
/// the moment of the replay. The reply says whether the write merged into an existing row,
/// because the caller cannot tell from the id alone and it is the one case where the id it
/// gets back is not the id it would have had.
///
/// `STARTED_AT=0` counts as absent rather than as 1970, the same reading a blank value gets: the
/// setting's own default is 0, so 0 is what "the caller did not say" looks like here. No real run
/// happened at the epoch, so nothing is lost by that reading, but it is worth knowing before someone
/// replays a hundred rows through a loop that forgot to pass the key.
pub async fn history_add(settings: &Settings, out: &mut dyn Emitter) -> Result<(), CliError> {
    let settings = settings.clone();
    let built = on_blocking(move || {
        let sql = settings.text("SQL", "");
        if sql.trim().is_empty() {
            return Err(CliError::Usage(
                "SQL is required to record an execution".to_owned(),
            ));
        }
        let started_at = match settings.number("STARTED_AT", 0)? {
            0 => qh_storage::now_millis(),
            given => given,
        };

        let mut record = QueryHistoryRecord::new(connection_filter(&settings)?, sql, started_at);
        record.elapsed_ms = optional_number(&settings, "ELAPSED_MS")?;
        record.row_count = optional_number(&settings, "ROW_COUNT")?;
        record.outcome = outcome_of(&settings)?;
        let error_text = settings.text("ERROR_TEXT", "");
        record.error_text = if error_text.is_empty() {
            None
        } else {
            Some(error_text)
        };

        let storage = open_storage(&settings)?;
        let written = storage.record_history(&record)?;
        Ok(event("history_entry")
            .field("id", written.as_str())
            .field("merged", written != record.meta.id)
            .build())
    })
    .await?;
    out.emit(built)?;
    Ok(())
}

/// `history_clear`: clear the history, all of it or one connection's.
///
/// The count is of rows that were living, so calling it twice reports zero the second time.
/// The rows stay as tombstones, which is what every deletion in this database does.
pub async fn history_clear(settings: &Settings, out: &mut dyn Emitter) -> Result<(), CliError> {
    let settings = settings.clone();
    let built = on_blocking(move || {
        let storage = open_storage(&settings)?;
        let filter = connection_filter(&settings)?;
        let cleared = storage.clear_history(filter.as_ref(), qh_storage::now_millis())?;
        Ok(event("history_clear").field("cleared", cleared).build())
    })
    .await?;
    out.emit(built)?;
    Ok(())
}

/// `saved_queries`: list, read, write, rename or delete one saved query.
///
/// One command with an action rather than five commands, for the same reason `credential`
/// is one: the app sets settings rather than building an argv, and five names would put
/// five entries in the usage line to describe one thing the user thinks of as one thing.
pub async fn saved_queries(settings: &Settings, out: &mut dyn Emitter) -> Result<(), CliError> {
    let settings = settings.clone();
    let built = on_blocking(move || {
        let action = saved_action_of(&settings)?;
        let storage = open_storage(&settings)?;
        let at = qh_storage::now_millis();
        match action.as_str() {
            "list" => Ok(event("saved_queries")
                .field(
                    "queries",
                    storage
                        .saved_queries()?
                        .iter()
                        .map(saved_query_json)
                        .collect::<Vec<_>>(),
                )
                .build()),
            "get" => {
                let id = saved_id(&settings)?;
                let record = storage.saved_query(&id)?;
                Ok(event("saved_query")
                    .field("action", "get")
                    .field("id", id.as_str())
                    .maybe("query", record.as_ref().map(saved_query_json))
                    .build())
            }
            "save" => {
                let name = settings.text("NAME", "");
                if name.trim().is_empty() {
                    return Err(CliError::Usage(
                        "NAME is required to save a query".to_owned(),
                    ));
                }
                let sql = settings.text("SQL", "");
                if sql.trim().is_empty() {
                    return Err(CliError::Usage(
                        "SQL is required to save a query".to_owned(),
                    ));
                }
                let record = SavedQueryRecord::new(name, sql, connection_filter(&settings)?, at);
                storage.save_query(&record)?;
                Ok(event("saved_query")
                    .field("action", "save")
                    .field("query", saved_query_json(&record))
                    .build())
            }
            "rename" => {
                let id = saved_id(&settings)?;
                let name = settings.text("NAME", "");
                if name.trim().is_empty() {
                    return Err(CliError::Usage(
                        "NAME is required to rename a query".to_owned(),
                    ));
                }
                let renamed = storage.rename_saved_query(&id, name, at)?;
                Ok(event("saved_query")
                    .field("action", "rename")
                    .field("id", id.as_str())
                    .field("renamed", renamed)
                    .build())
            }
            "favourite" => {
                let id = saved_id(&settings)?;
                // Read as a value rather than tested for presence, because both answers are
                // legitimate and the absent case still has to mean one of them. Absent means "keep
                // it", so a bare `SAVED_ACTION=favourite SAVED_ID=...` does the obvious thing.
                let favourite = match settings.text("FAVOURITE", "").trim() {
                    "" | "1" | "true" | "yes" => true,
                    "0" | "false" | "no" => false,
                    other => {
                        return Err(CliError::Usage(format!(
                            "unknown FAVOURITE '{other}'; expected 1 or 0"
                        )))
                    }
                };
                let found = storage.set_saved_query_favourite(&id, favourite, at)?;
                Ok(event("saved_query")
                    .field("action", "favourite")
                    .field("id", id.as_str())
                    .field("favourite", favourite)
                    .field("found", found)
                    .build())
            }
            _ => {
                let id = saved_id(&settings)?;
                let deleted = storage.soft_delete_saved_query(&id, at)?;
                Ok(event("saved_query")
                    .field("action", "delete")
                    .field("id", id.as_str())
                    .field("deleted", deleted)
                    .build())
            }
        }
    })
    .await?;
    out.emit(built)?;
    Ok(())
}

// --------------------------------------------------------------------------- //
// helpers
// --------------------------------------------------------------------------- //

/// `session`: the query tabs that were open, saved and read back as one blob.
///
/// `SESSION_ACTION` names the action. `TABS_JSON` is the app's own JSON — this command does not
/// look inside it, because what a tab is made of is the app's business and a field added to one
/// should not need a migration here. It is refused when it is not JSON at all, which is the one
/// check worth making: a session that cannot be parsed is better reported at quit than
/// discovered at the next launch.
///
/// `load` answers with `saved:false` when there is nothing, rather than an empty list. A fresh
/// install has no session, and that is a normal answer the app turns into one blank tab; an
/// `error` there would make every first launch look like a failure.
pub async fn session(settings: &Settings, out: &mut dyn Emitter) -> Result<(), CliError> {
    let settings = settings.clone();
    let built = on_blocking(move || {
        let action = session_action_of(&settings)?;
        let storage = open_storage(&settings)?;
        match action.as_str() {
            "save" => {
                let tabs = settings.text("TABS_JSON", "");
                if tabs.trim().is_empty() {
                    return Err(CliError::Usage(
                        "TABS_JSON is required to save a session".to_owned(),
                    ));
                }
                if serde_json::from_str::<Json>(&tabs).is_err() {
                    return Err(CliError::Usage(
                        "TABS_JSON must be JSON, and this is not".to_owned(),
                    ));
                }
                // Blank means "no front tab", the convention `CONNECTION_ID` follows.
                let active = settings.text("ACTIVE_TAB_ID", "");
                let active = (!active.trim().is_empty()).then_some(active);
                storage.save_session(&tabs, active.as_deref(), qh_storage::now_millis())?;
                Ok(event("session")
                    .field("action", "save")
                    .field("saved", true)
                    .build())
            }
            "load" => match storage.session()? {
                Some(record) => {
                    // Parsed here so the caller reads an array rather than a string it has to
                    // parse back. A blob that will not parse becomes `null`, which the app reads
                    // as "nothing to restore" — the same reading a broken connection `options`
                    // gets, and for the same reason: one unreadable value is not a reason to
                    // refuse the whole answer.
                    let tabs: Json = serde_json::from_str(&record.tabs_json).unwrap_or(Json::Null);
                    Ok(event("session")
                        .field("action", "load")
                        .field("saved", true)
                        .field("tabs", tabs)
                        .maybe("active_tab_id", record.active_tab_id)
                        .build())
                }
                None => Ok(event("session")
                    .field("action", "load")
                    .field("saved", false)
                    .build()),
            },
            _ => {
                let cleared = storage.clear_session(qh_storage::now_millis())?;
                Ok(event("session")
                    .field("action", "clear")
                    .field("cleared", cleared)
                    .build())
            }
        }
    })
    .await?;
    out.emit(built)?;
    Ok(())
}

/// Run the blocking half of a local command off the runtime's worker threads.
///
/// The closure returns the whole event, so nothing but owned JSON crosses back and the
/// emitter is only ever touched on this side of the boundary.
async fn on_blocking<F>(work: F) -> Result<Json, CliError>
where
    F: FnOnce() -> Result<Json, CliError> + Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(built) => built,
        // A panic inside the task is a defect in this program like any other, and the
        // binary's own `catch_unwind` cannot see it: it happened on another thread.
        Err(error) => Err(CliError::Internal(error.to_string())),
    }
}

/// The database the commands work on: `DB_PATH` when the caller named one, the
/// Application Support file otherwise.
///
/// A named path is migrated on open for the same reason [`qh_storage::open_default`]
/// migrates: every command that reaches this function wants a usable schema, so a caller
/// who pointed `DB_PATH` at a fresh file meant "make it usable", not "tell me it has no
/// tables".
fn open_storage(settings: &Settings) -> Result<Storage, CliError> {
    let raw = settings.text("DB_PATH", "");
    if raw.is_empty() {
        return Ok(qh_storage::open_default()?);
    }
    let mut storage = Storage::open(expand_user(&raw))?;
    storage.migrate()?;
    Ok(storage)
}

/// The `connections.json` an import would read: `LEGACY_PATH` when the caller named one,
/// the Application Support file the app wrote otherwise.
fn legacy_source(settings: &Settings) -> Option<PathBuf> {
    let raw = settings.text("LEGACY_PATH", "");
    if raw.is_empty() {
        import::default_source()
    } else {
        Some(expand_user(&raw))
    }
}

/// One connection as the `connections` event carries it.
///
/// The nulls are the point: `host`, `port`, `user`, `database` and `secret_ref` are
/// genuinely absent for a connection that does not use them, and an empty string would be
/// a second value the app had to know means "missing".
fn connection_json(record: &ConnectionRecord) -> Json {
    // The stored options are opaque JSON — they are kept as text so a new option needs no
    // migration — and they are handed over parsed, because a caller that reads a `string`
    // here would have to parse it back. A row that will not parse becomes `null`: one
    // unreadable options bag is not a reason to refuse the whole list.
    let options: Json = serde_json::from_str(&record.options_json).unwrap_or(Json::Null);
    json!({
        "id": record.meta.id.as_str(),
        "name": record.name,
        "kind": record.kind.as_str(),
        "host": record.host,
        "port": record.port,
        "user": record.user_name,
        "database": record.database_name,
        "options": options,
        "secret_ref": record.secret_ref,
        "is_production": record.is_production,
        "is_read_only": record.is_read_only,
        "deleted": record.meta.is_deleted(),
        "version": record.meta.version.get(),
        "sort_order": record.sort_order,
    })
}

/// The `import` event, from what the import did or from how far it got before there was
/// nothing to do.
fn import_event(source: Option<&Path>, source_found: bool, report: Option<&ImportReport>) -> Json {
    event("import")
        .field(
            "source",
            source.map(|path| path.to_string_lossy().into_owned()),
        )
        .field("source_found", source_found)
        .field(
            "already_imported",
            report.is_some_and(|report| report.already_imported),
        )
        .field("written", report.map_or(0usize, |report| report.written))
        .field("kept", report.map_or(0usize, |report| report.kept))
        // Nothing was verified when there was nothing to verify, which is what `false`
        // says; this is only ever `true` when rows were read back and compared.
        .field("verified", report.is_some_and(|report| report.verified))
        .field(
            "backup",
            report
                .and_then(|report| report.backup.as_ref())
                .map(|path| path.to_string_lossy().into_owned()),
        )
        .field(
            "skipped",
            report
                .map(|report| {
                    report
                        .skipped
                        .iter()
                        .map(|row| json!({"index": row.index, "reason": row.reason}))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        )
        .build()
}

/// How many history entries a listing may return.
fn history_limit(settings: &Settings) -> Result<usize, CliError> {
    let limit = settings.number("HISTORY_LIMIT", 100)?;
    // A negative limit reaches SQLite as "no limit", which is the opposite of what a caller
    // that asked for a number meant.
    Ok(usize::try_from(limit.max(0)).unwrap_or(0))
}

/// The connection a command is scoped to, when the caller named one.
///
/// An empty value means "no filter", not "the connection whose identity is the empty
/// string": the app sets every setting it knows about, and one it does not want arrives
/// blank.
fn connection_filter(settings: &Settings) -> Result<Option<SyncId>, CliError> {
    let raw = settings.text("CONNECTION_ID", "");
    if raw.is_empty() {
        return Ok(None);
    }
    SyncId::parse(&raw).map(Some).map_err(|error| {
        CliError::Usage(format!("CONNECTION_ID {raw:?} is not an identity: {error}"))
    })
}

/// The saved query a `get`, `rename` or `delete` names.
///
/// `SAVED_ID`, not `CONNECTION_ID`: a saved query has its own identity and also carries the
/// connection it belongs to, and one key for both would make `rename` ambiguous.
fn saved_id(settings: &Settings) -> Result<SyncId, CliError> {
    let raw = settings.text("SAVED_ID", "");
    if raw.is_empty() {
        return Err(CliError::Usage(
            "SAVED_ID is required for get, rename and delete".to_owned(),
        ));
    }
    SyncId::parse(&raw)
        .map_err(|error| CliError::Usage(format!("SAVED_ID {raw:?} is not an identity: {error}")))
}

/// An optional number, where unset and zero are different answers.
///
/// Blank counts as unset, the same convention [`connection_filter`] follows. `is_set` alone
/// would not do: it is true whenever the key exists, and the app sets the keys it knows
/// about. Reading a blank as `Some(0)` would turn "no elapsed time recorded" into "it took
/// no time", which is a different fact about a different query.
fn optional_number(settings: &Settings, key: &str) -> Result<Option<i64>, CliError> {
    if settings.text(key, "").is_empty() {
        return Ok(None);
    }
    Ok(Some(settings.number(key, 0)?))
}

/// The outcome a caller is recording, refused by name when it is not one of the three.
fn outcome_of(settings: &Settings) -> Result<Option<Outcome>, CliError> {
    let raw = settings.text("OUTCOME", "");
    if raw.is_empty() {
        return Ok(None);
    }
    Outcome::parse(&raw).map(Some).map_err(|_| {
        CliError::Usage(format!(
            "unknown OUTCOME '{raw}'; expected ok, error or cancelled"
        ))
    })
}

/// The `saved_queries` action, refused by name when it is not one of the five.
fn saved_action_of(settings: &Settings) -> Result<String, CliError> {
    let raw = settings.text("SAVED_ACTION", "");
    if raw.is_empty() {
        return Err(CliError::Usage(
            "SAVED_ACTION is required; expected list, get, save, rename or delete".to_owned(),
        ));
    }
    let action = raw.to_ascii_lowercase();
    if !matches!(
        action.as_str(),
        "list" | "get" | "save" | "rename" | "delete" | "favourite"
    ) {
        return Err(CliError::Usage(format!(
            "unknown SAVED_ACTION '{raw}'; expected list, get, save, rename, favourite or delete"
        )));
    }
    Ok(action)
}

/// The `session` action, refused by name when it is not one of the three.
fn session_action_of(settings: &Settings) -> Result<String, CliError> {
    let raw = settings.text("SESSION_ACTION", "");
    if raw.is_empty() {
        return Err(CliError::Usage(
            "SESSION_ACTION is required; expected save, load or clear".to_owned(),
        ));
    }
    let action = raw.to_ascii_lowercase();
    if !matches!(action.as_str(), "save" | "load" | "clear") {
        return Err(CliError::Usage(format!(
            "unknown SESSION_ACTION '{raw}'; expected save, load or clear"
        )));
    }
    Ok(action)
}

/// One history row as the `history` event carries it.
///
/// The nulls are the point, exactly as they are in [`connection_json`]: an execution that
/// has not finished has no elapsed time and no outcome, and an empty string would be a
/// second value the app had to know means "not yet".
fn history_json(record: &QueryHistoryRecord) -> Json {
    json!({
        "id": record.meta.id.as_str(),
        "connection_id": record.connection_id.as_ref().map(SyncId::as_str),
        "sql": record.sql_text,
        "started_at": record.started_at,
        "elapsed_ms": record.elapsed_ms,
        "row_count": record.row_count,
        "outcome": record.outcome.map(Outcome::as_str),
        "error": record.error_text,
        "deleted": record.meta.is_deleted(),
        "version": record.meta.version.get(),
    })
}

/// One saved query as the `saved_queries` event carries it.
fn saved_query_json(record: &SavedQueryRecord) -> Json {
    json!({
        "id": record.meta.id.as_str(),
        "name": record.name,
        "sql": record.sql_text,
        "connection_id": record.connection_id.as_ref().map(SyncId::as_str),
        "folder_id": record.folder_id.as_ref().map(SyncId::as_str),
        "favourite": record.favourite,
        "deleted": record.meta.is_deleted(),
        "version": record.meta.version.get(),
    })
}

/// The `credential` action, refused by name when it is not one of the four.
fn action_of(settings: &Settings) -> Result<String, CliError> {
    let raw = settings.text("CREDENTIAL_ACTION", "");
    if raw.is_empty() {
        return Err(CliError::Usage(
            "CREDENTIAL_ACTION is required; expected has, get, set or delete".to_owned(),
        ));
    }
    let action = raw.to_ascii_lowercase();
    if !matches!(action.as_str(), "has" | "get" | "set" | "delete") {
        return Err(CliError::Usage(format!(
            "unknown CREDENTIAL_ACTION '{raw}'; expected has, get, set or delete"
        )));
    }
    Ok(action)
}

/// The secret a `set` carries.
///
/// `DB_PASSWORD` is read verbatim rather than through [`Settings::text`], because a
/// password's padding is part of it — the same reason that reader does not strip. An
/// absent key is refused rather than stored as an empty password: clearing a password is
/// what `delete` is for.
fn password_of(settings: &Settings) -> Result<SecretString, CliError> {
    if !settings.is_set("DB_PASSWORD") {
        return Err(CliError::Usage(
            "DB_PASSWORD is required to set a credential".to_owned(),
        ));
    }
    Ok(SecretString::new(
        settings.raw("DB_PASSWORD", "").into_boxed_str(),
    ))
}

/// The store the `credential` command works against.
///
/// [`KeychainStore`] is the real one: one generic-password item per connection, under the
/// service name the app has always used. `CREDENTIAL_STORE=memory` swaps in
/// [`MemoryStore`], which is **the test and CI seam** — an unattended run that touches the
/// login Keychain raises a permission dialog on the machine running it, and a suite that
/// waits for a human to answer is a suite that hangs. Anything other than `memory` gets
/// the Keychain, including an unrecognised value.
///
/// The memory store is one static rather than one per call because the actions are run as
/// separate commands, exactly as the app runs them: a fresh map per `run` would make a
/// `set` and the `has` that follows it disagree, which is not how the Keychain behaves.
fn store_for(settings: &Settings) -> &'static dyn SecretStore {
    static KEYCHAIN: KeychainStore = KeychainStore;
    if settings
        .text("CREDENTIAL_STORE", "")
        .eq_ignore_ascii_case("memory")
    {
        return memory_store();
    }
    &KEYCHAIN
}

/// The process-wide memory store, created on first use.
fn memory_store() -> &'static MemoryStore {
    static MEMORY: OnceLock<MemoryStore> = OnceLock::new();
    MEMORY.get_or_init(MemoryStore::new)
}
