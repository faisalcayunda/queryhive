//! Bringing a `connections.json` from the previous store into this one.
//!
//! The app has, until now, kept its connections in a JSON array at
//! `~/Library/Application Support/QueryHive/connections.json`, with one password per
//! connection in the Keychain under the service name `qh-credentials` still uses. This
//! module reads that array into rows.
//!
//! # The contract is the decoder, not a guess
//!
//! Every default below was read out of `app/Sources/TrinoExporter/Models/Connections.swift`
//! rather than invented: `color` falls back to `blue`, `kind` to `trino`, `port` to the
//! kind's default, `scheme` to `httpScheme` and then to `https`, `verify` to `true`, and
//! `showAllSchemas` to `false`. Two legacy spellings are read and never written
//! (`httpScheme`, `catalog`), and they are read with "first present wins" rather than as
//! aliases, because that is what the decoder does — a file carrying both names is read by
//! the app without complaint, so it has to be read here too.
//!
//! # Four decisions this module makes that the app does not
//!
//! 1. **A row that cannot be read is skipped and reported, not fatal.** The app's decoder
//!    throws on the whole array, so one unreadable row loses every connection and the file
//!    is moved aside. Here the other rows are imported and the bad one is listed in the
//!    report, because a user with nineteen connections and one typo should keep nineteen.
//! 2. **The array's order becomes `sort_order`.** The file has no field for it, and the
//!    order it is written in is the order the sidebar shows. Dropping it would reorder
//!    everyone's sidebar alphabetically.
//! 3. **`secret_ref` is set for every row.** It is the connection's own UUID, which is
//!    exactly the account the app looks a password up under — the reference is derived from
//!    the id, not a fact that has to be discovered by asking the Keychain (which would also
//!    mean prompting).
//! 4. **An empty `host`, `user` or `database` becomes NULL.** SQL has a value for "not
//!    set" and an empty string would be a second way to say the same thing; the app's empty
//!    string and its absent field are already the same thing to it.
//!
//! # What "finished" means
//!
//! The rows are written, then **read back and compared**, and only then is a row written to
//! `legacy_import` saying so. An import that fails verification leaves no marker, so running
//! it again is the retry — and an import that already has a marker returns without writing
//! anything, which is what makes the whole thing safe to run on every launch.
//!
//! Nothing is written before the source file has been copied aside. If the backup cannot be
//! made, the import does not start: the file is the only copy of what is being imported.

use std::path::{Path, PathBuf};

use qh_sync::SyncId;
use rusqlite::{params, OptionalExtension};
use serde_json::{Map, Value};

use crate::connections::ConnectionKind;
use crate::{ConnectionRecord, Storage, StorageError};

/// The file the previous store wrote, inside its directory.
pub const LEGACY_FILE_NAME: &str = "connections.json";

/// The directory the app keeps its files in, inside Application Support.
pub const APPLICATION_SUPPORT_DIRECTORY: &str = "QueryHive";

/// Why an import could not be done.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("could not read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("could not copy {path} to {backup}: {source}")]
    Backup {
        path: PathBuf,
        backup: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{path} does not hold a JSON array of connections: {reason}")]
    Malformed { path: PathBuf, reason: String },

    #[error(
        "the imported rows did not read back the same: {} missing, {} different",
        missing.len(),
        mismatched.len()
    )]
    VerificationFailed {
        missing: Vec<String>,
        mismatched: Vec<String>,
    },

    #[error(transparent)]
    Storage(#[from] StorageError),

    /// A statement the import itself runs — reading the marker, writing it — rather than one
    /// that went through the connection CRUD.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

/// A row in the file that could not become a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedRow {
    /// Its position in the array, counting from zero, so the file can be repaired.
    pub index: usize,
    pub reason: String,
}

/// What an import would write, before anything is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPlan {
    pub source: PathBuf,
    pub connections: Vec<ConnectionRecord>,
    pub skipped: Vec<SkippedRow>,
}

/// What an import did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// True when a marker for this source was already there, in which case nothing was
    /// written, no backup was made, and every other field is the state of that earlier run
    /// as far as this one can tell.
    pub already_imported: bool,
    /// The copy of the source file, made before the first row was written.
    pub backup: Option<PathBuf>,
    /// Rows written (including rows that replaced an older revision of themselves).
    pub written: usize,
    /// Rows whose stored revision was already newer, so the stored one was kept.
    pub kept: usize,
    /// The imported rows really are in the database, field for field.
    pub verified: bool,
    pub skipped: Vec<SkippedRow>,
}

impl ImportReport {
    /// A one-line summary for a log or a notice in the UI.
    pub fn summary(&self) -> String {
        if self.already_imported {
            return "connections were imported on an earlier launch".to_owned();
        }
        let mut text = format!("imported {} connection(s)", self.written);
        if self.kept > 0 {
            text.push_str(&format!(", kept {} newer row(s) already stored", self.kept));
        }
        if !self.skipped.is_empty() {
            text.push_str(&format!(", skipped {}", self.skipped.len()));
        }
        text
    }
}

/// Where the app keeps its files: `~/Library/Application Support/QueryHive`.
///
/// Read from `HOME` rather than through a platform framework: this is a plain path, the
/// engine is macOS-only, and pulling in a directories crate for one join would be a
/// dependency to keep current for no behaviour.
pub fn legacy_directory() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join(APPLICATION_SUPPORT_DIRECTORY),
    )
}

/// The file an import would read, if the app has ever saved one.
pub fn default_source() -> Option<PathBuf> {
    Some(legacy_directory()?.join(LEGACY_FILE_NAME))
}

/// Read a file into a plan. Writes nothing.
pub fn plan_file(source: impl AsRef<Path>, at: i64) -> Result<ImportPlan, ImportError> {
    let source = source.as_ref().to_path_buf();
    let text = std::fs::read_to_string(&source).map_err(|error| ImportError::Read {
        path: source.clone(),
        // A missing file is a normal state — the app has never saved a connection — and
        // the caller is expected to treat it as "nothing to import" rather than as a
        // failure. It arrives as `Read` with `NotFound` inside for that reason.
        source: error,
    })?;
    plan_json(&source, &text, at)
}

/// Read the JSON text of a connections file into a plan. Writes nothing.
pub fn plan_json(source: &Path, text: &str, at: i64) -> Result<ImportPlan, ImportError> {
    let parsed: Value = serde_json::from_str(text).map_err(|error| ImportError::Malformed {
        path: source.to_path_buf(),
        reason: error.to_string(),
    })?;
    let rows = parsed.as_array().ok_or_else(|| ImportError::Malformed {
        path: source.to_path_buf(),
        reason: "the top level is not an array".to_owned(),
    })?;

    let mut connections = Vec::new();
    let mut skipped = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        match to_record(row, index, at) {
            Ok(record) => connections.push(record),
            Err(reason) => skipped.push(reason),
        }
    }

    Ok(ImportPlan {
        source: source.to_path_buf(),
        connections,
        skipped,
    })
}

/// When this source was imported, if it ever was.
pub fn already_imported(storage: &Storage, source: &Path) -> Result<Option<i64>, ImportError> {
    let imported_at = storage
        .conn
        .query_row(
            "SELECT imported_at FROM legacy_import WHERE source = ?1 AND verified = 1",
            params![source.to_string_lossy()],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    Ok(imported_at)
}

/// Write a plan's rows into the database.
///
/// In order: refuse if this source is already marked done, copy the file aside, merge each
/// row, read them back and compare, and only then mark it done. See the module note.
pub fn import_connections(
    storage: &Storage,
    plan: &ImportPlan,
    at: i64,
) -> Result<ImportReport, ImportError> {
    if already_imported(storage, &plan.source)?.is_some() {
        return Ok(ImportReport {
            already_imported: true,
            backup: None,
            written: 0,
            kept: 0,
            verified: true,
            skipped: plan.skipped.clone(),
        });
    }

    // Before anything else, and before any row: the file is the only copy of what is being
    // imported, and an import is the moment a user is least able to reconstruct it.
    let backup = backup_path(&plan.source, at);
    std::fs::copy(&plan.source, &backup).map_err(|error| ImportError::Backup {
        path: plan.source.clone(),
        backup: backup.clone(),
        source: error,
    })?;

    let mut written_ids = Vec::new();
    let mut written = 0;
    let mut kept = 0;
    for record in &plan.connections {
        match storage.merge_connection(record)? {
            qh_sync::Resolution::TakeTheirs => {
                written += 1;
                written_ids.push(record.meta.id.clone());
            }
            qh_sync::Resolution::KeepOurs => kept += 1,
        }
    }

    // Read back what this import wrote and compare it field for field: a count would not
    // catch a row written with the wrong options, and this does.
    //
    // A row that was **kept** is checked for being present and for nothing else. The merge
    // decided not to write it, so comparing it against the copy from the file would report
    // that decision as a failure — which is exactly what an earlier version of this loop
    // did, on the first run of the test that covers keeping a newer row.
    let mut missing = Vec::new();
    let mut mismatched = Vec::new();
    for record in &plan.connections {
        match storage.connection(&record.meta.id)? {
            None => missing.push(record.meta.id.to_string()),
            Some(stored) if written_ids.contains(&record.meta.id) && stored != *record => {
                mismatched.push(record.meta.id.to_string());
            }
            Some(_) => {}
        }
    }
    if !missing.is_empty() || !mismatched.is_empty() {
        return Err(ImportError::VerificationFailed {
            missing,
            mismatched,
        });
    }

    storage.conn.execute(
        "INSERT INTO legacy_import (source, imported_at, connections, verified) \
         VALUES (?1, ?2, ?3, 1) \
         ON CONFLICT(source) DO UPDATE SET imported_at = excluded.imported_at, \
          connections = excluded.connections, verified = excluded.verified",
        params![
            plan.source.to_string_lossy(),
            at,
            i64::try_from(plan.connections.len()).unwrap_or(i64::MAX)
        ],
    )?;

    Ok(ImportReport {
        already_imported: false,
        backup: Some(backup),
        written,
        kept,
        verified: true,
        skipped: plan.skipped.clone(),
    })
}

/// `connections.json.before-import-<millis>`, beside the file it copies.
///
/// Beside rather than in a temporary directory: a user who has to recover from this by hand
/// should find the copy next to the original, not somewhere they have to be told about.
fn backup_path(source: &Path, at: i64) -> PathBuf {
    let name = source
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| LEGACY_FILE_NAME.to_owned());
    source.with_file_name(format!("{name}.before-import-{at}"))
}

/// One row of the file as a connection, or the reason it could not be one.
fn to_record(row: &Value, index: usize, at: i64) -> Result<ConnectionRecord, SkippedRow> {
    let skipped = |reason: String| SkippedRow { index, reason };
    let Some(fields) = row.as_object() else {
        return Err(skipped("the row is not a JSON object".to_owned()));
    };

    // The two fields the app's decoder requires. Everything else has a default.
    let Some(id_text) = text(fields, "id") else {
        return Err(skipped("no id".to_owned()));
    };
    let Some(name) = text(fields, "name") else {
        return Err(skipped("no name".to_owned()));
    };
    let id = SyncId::parse(&id_text)
        .map_err(|error| skipped(format!("the id is not a UUID: {error}")))?;

    let kind_text = text(fields, "kind").unwrap_or_else(|| "trino".to_owned());
    let kind = ConnectionKind::parse(&kind_text)
        .map_err(|_| skipped(format!("{kind_text:?} is not a kind this engine knows")))?;

    // `database` first, then the name this file used before the app spoke to more than
    // Trino. Both, not either-or: the decoder reads one container and then the other, so a
    // file carrying both is read rather than rejected.
    let database = text(fields, "database").or_else(|| text(fields, "catalog"));
    // Likewise `scheme` and `httpScheme`, with `https` as the last resort.
    let scheme = text(fields, "scheme")
        .or_else(|| text(fields, "httpScheme"))
        .unwrap_or_else(|| "https".to_owned());

    let port = number(fields, "port")
        .and_then(|value| u16::try_from(value).ok())
        .unwrap_or_else(|| default_port(kind));

    let options = options_json(fields, &scheme);

    let mut record = ConnectionRecord::new(name, kind, at);
    record.meta.id = id.clone();
    record.host = non_empty(text(fields, "host"));
    record.port = Some(port);
    record.user_name = non_empty(text(fields, "user"));
    record.database_name = non_empty(database);
    record.options_json = options;
    // The Keychain account is the connection's own identity; it is derived here rather than
    // looked up, so the import never has to ask the Keychain anything. `qh-credentials`
    // folds a UUID account to upper case before use, which is how this lower-case spelling
    // finds the item the app wrote.
    record.secret_ref = Some(id.to_string());
    // The file's order is the sidebar's order, and the file has nowhere else to say so.
    record.sort_order = index as i64;
    Ok(record)
}

/// The fields with no column of their own, at the values the app's model would hold.
///
/// Every one of them carries the decoder's default rather than being omitted: a row that
/// says `"verify": true` is readable without knowing what the app used to do when the key
/// was absent.
fn options_json(fields: &Map<String, Value>, scheme: &str) -> String {
    let mut options = Map::new();
    options.insert(
        "color".to_owned(),
        Value::String(text(fields, "color").unwrap_or_else(|| "blue".to_owned())),
    );
    options.insert("scheme".to_owned(), Value::String(scheme.to_owned()));
    // Stored as the file had it, empty included: the app's per-kind default
    // (`prefer`/`disable`) is computed when the connection is used, not stored.
    options.insert(
        "sslmode".to_owned(),
        Value::String(text(fields, "sslmode").unwrap_or_default()),
    );
    options.insert(
        "schema".to_owned(),
        Value::String(text(fields, "schema").unwrap_or_default()),
    );
    options.insert(
        "verify".to_owned(),
        Value::Bool(flag(fields, "verify").unwrap_or(true)),
    );
    options.insert(
        "showAllSchemas".to_owned(),
        Value::Bool(flag(fields, "showAllSchemas").unwrap_or(false)),
    );
    Value::Object(options).to_string()
}

/// The port the app uses when the file does not say — which depends on the kind, so the
/// kind has to be read first.
fn default_port(kind: ConnectionKind) -> u16 {
    match kind {
        ConnectionKind::Trino => 8080,
        ConnectionKind::Postgres => 5432,
        ConnectionKind::Mysql => 3306,
    }
}

fn text(fields: &Map<String, Value>, key: &str) -> Option<String> {
    match fields.get(key) {
        Some(Value::String(value)) => Some(value.clone()),
        // A number where a string is expected is not silently stringified: the app would
        // have refused the file, and guessing here would invent data.
        _ => None,
    }
}

fn number(fields: &Map<String, Value>, key: &str) -> Option<i64> {
    fields.get(key).and_then(Value::as_i64)
}

fn flag(fields: &Map<String, Value>, key: &str) -> Option<bool> {
    fields.get(key).and_then(Value::as_bool)
}

fn non_empty(text: Option<String>) -> Option<String> {
    text.filter(|value| !value.is_empty())
}

/// The rows a previous import recorded, for a diagnostic that has to answer "did this
/// already happen, and when".
pub fn import_history(storage: &Storage) -> Result<Vec<ImportedSource>, ImportError> {
    let mut statement = storage.conn.prepare(
        "SELECT source, imported_at, connections FROM legacy_import ORDER BY imported_at ASC",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(ImportedSource {
            source: row.get(0)?,
            imported_at: row.get(1)?,
            connections: row.get(2)?,
        })
    })?;
    let mut history = Vec::new();
    for row in rows {
        history.push(row?);
    }
    Ok(history)
}

/// One finished import, as `legacy_import` remembers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedSource {
    pub source: String,
    pub imported_at: i64,
    pub connections: i64,
}
