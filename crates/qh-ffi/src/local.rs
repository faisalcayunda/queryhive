//! The three local commands: the connection store, its legacy import, and the password.
//!
//! None of them opens a driver, and none of them is a transcription of the Python engine
//! — it had no local store at all. What they have instead is a contract with the app:
//! `connections` is what the sidebar is painted from, `import_connections` is the
//! one-shot move of the old `connections.json` into the database, and `credential` is the
//! only path by which a saved password leaves the Keychain.
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
use qh_storage::{ConnectionRecord, Storage};
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
            "has" => (store.get(&key)?.is_some(), None),
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

// --------------------------------------------------------------------------- //
// helpers
// --------------------------------------------------------------------------- //

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
