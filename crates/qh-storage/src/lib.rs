//! The local database: saved connections, query history, saved queries, and the
//! migrations that make them.
//!
//! SQLite through `rusqlite` with the library compiled in (`bundled`), which is what §5
//! of the blueprint chose: the metadata is small, the operations are short, and a
//! self-contained SQLite removes any question about which version the machine has. It is
//! a **blocking** API and it is meant to be called from a blocking thread, never from a
//! tokio worker — a `tokio` worker blocked on a file lock is a runtime that stops
//! answering.
//!
//! # What lives here, and what does not
//!
//! Rows about *the user's own workspace*: which connections exist, what they are called,
//! what happened when they ran queries, what has been saved. Not the passwords — a
//! connection row holds a `secret_ref` naming a Keychain item, and `qh-credentials` is
//! what reads it. Not the results — those are `qh-result-store`'s, which spills to its own
//! files.
//!
//! # Two rules the schema exists to make possible
//!
//! Every row carries [`qh_sync::SyncMeta`]: an identity, a revision, and a tombstone. That
//! is what makes the cloud-sync feature a *transport* to add later rather than a schema
//! migration to schedule now (§5 point 7 and the conflict table in §7.2). The rules for
//! comparing two revisions live in `qh-sync` and are used from here, so there is one
//! answer to "whose edit wins" rather than one per table.
//!
//! Migrations are embedded in the binary and applied one transaction at a time, with
//! SQLite's `user_version` and a `schema_migration` history row moved together inside
//! that transaction. See [`migrate`] for why both records exist and what happens when
//! they disagree.

mod connections;
pub mod import;
pub mod migrate;

pub use connections::{ConnectionGroup, ConnectionKind, ConnectionRecord};
pub use import::{ImportPlan, ImportReport, ImportedSource, SkippedRow};
pub use migrate::{AppliedMigration, Migration, MIGRATIONS};

use std::path::Path;

use rusqlite::Connection;

/// Why a storage operation could not be done.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("the database refused the statement: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// A database written by a newer build of the application.
    ///
    /// Refused rather than opened: this build does not know the shape of rows it did not
    /// write, and writing alongside them risks losing columns it cannot see.
    #[error(
        "the database is at version {found} and this build knows up to {known}; \
         it was written by a newer version of the application"
    )]
    NewerDatabase { found: i64, known: i64 },

    /// A database with tables this engine did not create.
    ///
    /// The Python-era database is the case this exists for. Importing it is a separate
    /// operation that backs up first and verifies afterwards
    /// (`docs/migrations/python-to-rust.md`); creating our tables inside it and stamping
    /// version 1 would look like success and be data loss.
    #[error(
        "the file already holds a table named {table:?} and no migration history; \
         this is not a database this engine created"
    )]
    ForeignDatabase { table: String },

    /// `user_version` and the migration history disagree.
    #[error(
        "the migration marker says {user_version} but the history records {recorded:?}; \
         refusing to guess which one is right"
    )]
    InconsistentHistory {
        user_version: i64,
        recorded: Vec<i64>,
    },

    #[error("{text:?} is not a database kind this engine knows")]
    UnknownKind { text: String },

    #[error("the row holds {text:?} where an identity belongs: {reason}")]
    BadId { text: String, reason: String },

    #[error("HOME is not set, so there is nowhere to keep the database")]
    NoHomeDirectory,

    #[error("could not create {path}: {reason}")]
    Directory { path: std::path::PathBuf, reason: String },
}

/// The database this engine uses when nobody says otherwise.
///
/// Beside `connections.json` in Application Support, which is where the app already keeps
/// what belongs to it: one directory to back up, and one directory for a user to find when
/// they have to recover something by hand.
pub fn default_database_path() -> Option<std::path::PathBuf> {
    Some(import::legacy_directory()?.join(DATABASE_FILE_NAME))
}

/// The file name inside that directory.
pub const DATABASE_FILE_NAME: &str = "queryhive.sqlite3";

/// Open the default database, creating and migrating it if it is not there yet.
///
/// Migrating here rather than leaving it to the caller because this is the launch path and
/// there is nothing else a caller would want to do first: a database that cannot be
/// migrated is one this engine cannot use, and saying so now is better than at the first
/// write.
pub fn open_default() -> Result<Storage, StorageError> {
    let path = default_database_path().ok_or(StorageError::NoHomeDirectory)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| StorageError::Directory {
            path: parent.to_path_buf(),
            reason: error.to_string(),
        })?;
    }
    let mut storage = Storage::open(&path)?;
    storage.migrate()?;
    Ok(storage)
}

/// An open local database.
///
/// Not `Clone` and not shared: SQLite connections are not cheap to build and are not
/// meant to be duplicated, so a pool or a single owning thread is the caller's decision.
pub struct Storage {
    // Visible to the sibling modules that run its statements; not part of the API, so a
    // caller cannot reach past the rules these methods enforce.
    pub(crate) conn: Connection,
}

impl Storage {
    /// Open (or create) the database at a path.
    ///
    /// Does **not** migrate: the caller decides when that happens, and doing it here
    /// would make opening a foreign database look like opening ours.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let conn = Connection::open(path)?;
        let storage = Self { conn };
        storage.prepare()?;
        Ok(storage)
    }

    /// A database that exists only in memory. For tests, and for a caller that has not
    /// decided where the file goes.
    pub fn in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory()?;
        let storage = Self { conn };
        storage.prepare()?;
        Ok(storage)
    }

    /// The per-connection settings, applied on open rather than in a migration.
    ///
    /// `foreign_keys` is off in SQLite by default *per connection* — a table-level
    /// declaration alone does not enforce anything, which is a detail worth getting right
    /// once: without this, `group_id REFERENCES connection_group(id) ON DELETE SET NULL`
    /// is decoration.
    fn prepare(&self) -> Result<(), StorageError> {
        self.conn.pragma_update(None, "foreign_keys", "ON")?;
        // Write-ahead logging, so a reader (the sidebar) does not block the writer (a
        // finished query being recorded). It cannot be set inside a transaction, which is
        // why it is here and not in the migration.
        //
        // Asked through a query rather than `pragma_update`: `journal_mode` answers with
        // the mode it settled on, and an in-memory database answers `memory` — a fact
        // returned rather than an error raised.
        let _mode: String = self
            .conn
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        Ok(())
    }

    /// Bring the database up to date, stamping the history with the current time.
    pub fn migrate(&mut self) -> Result<Vec<AppliedMigration>, StorageError> {
        migrate::run(&mut self.conn, now_millis())
    }

    /// Bring the database up to date, at a stated time. For tests and for an import that
    /// has to be reproducible.
    pub fn migrate_at(&mut self, at: i64) -> Result<Vec<AppliedMigration>, StorageError> {
        migrate::run(&mut self.conn, at)
    }

    /// The version the database is at.
    pub fn schema_version(&self) -> Result<i64, StorageError> {
        migrate::user_version(&self.conn)
    }

    /// Every migration that has run, oldest first.
    pub fn migration_history(&self) -> Result<Vec<AppliedMigration>, StorageError> {
        migrate::history(&self.conn)
    }

    /// Ask SQLite whether the statements it has been given are all there is.
    ///
    /// Used by tests after a write: it is the check that a foreign key was actually
    /// enforced and that an index is not holding a violated constraint.
    #[doc(hidden)]
    pub fn check_integrity(&self) -> Result<(), StorageError> {
        let verdict: String = self
            .conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if verdict == "ok" {
            Ok(())
        } else {
            Err(StorageError::Sqlite(rusqlite::Error::InvalidQuery))
        }
    }
}

/// Unix milliseconds from the system clock.
///
/// Taken once per operation by the caller rather than read inside the CRUD methods, so
/// that a batch of writes shares one timestamp and a test does not depend on how long it
/// took to run.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_an_in_memory_database_does_not_migrate_it() {
        // Opening and migrating are separate because opening a file that is not ours must
        // not look like opening ours: the migration is what refuses a foreign database.
        let storage = Storage::in_memory().unwrap();
        assert_eq!(storage.schema_version().unwrap(), 0);
        assert!(
            storage.migration_history().is_err() || storage.migration_history().unwrap().is_empty()
        );
    }

    #[test]
    fn migrating_stamps_a_time_that_comes_from_one_clock() {
        let mut storage = Storage::in_memory().unwrap();
        let applied = storage.migrate_at(1_700_000_000_000).unwrap();
        assert_eq!(applied[0].applied_at, 1_700_000_000_000);
        assert!(
            now_millis() > 1_700_000_000_000,
            "the system clock is later than that"
        );
    }

    fn migrated() -> Storage {
        let mut storage = Storage::in_memory().unwrap();
        storage.migrate_at(1_700_000_000_000).unwrap();
        storage
    }

    fn text_column(storage: &Storage, sql: &str) -> String {
        storage
            .conn
            .query_row(sql, [], |row| row.get(0))
            .expect("a readable schema")
    }

    #[test]
    fn a_connection_enforces_its_foreign_keys() {
        // Off by default in SQLite, per connection, whatever the table says. The schema
        // declares `group_id REFERENCES connection_group(id) ON DELETE SET NULL` and that
        // declaration is decoration without this pragma — which is why its test deletes a
        // group for real instead of trusting the SQL to have meant it.
        let storage = migrated();
        let enabled: i64 = storage
            .conn
            .pragma_query_value(None, "foreign_keys", |row| row.get(0))
            .unwrap();
        assert_eq!(enabled, 1);
    }

    #[test]
    fn a_file_database_writes_ahead_and_an_in_memory_one_says_so() {
        // WAL is what keeps the sidebar readable while a finished query is being recorded.
        // It is asked for rather than assumed, and an in-memory database answers `memory`
        // because there is no file to log to — a fact returned, not an error raised.
        let directory = tempfile::tempdir().unwrap();
        let on_disk = Storage::open(directory.path().join("qh.sqlite3")).unwrap();
        assert_eq!(
            text_column(&on_disk, "PRAGMA journal_mode"),
            "wal",
            "a file-backed database logs ahead"
        );
        assert_eq!(
            text_column(&Storage::in_memory().unwrap(), "PRAGMA journal_mode"),
            "memory"
        );
    }

    #[test]
    fn the_schema_is_the_one_the_blueprint_specifies() {
        // Pinned because a released schema is the one thing a migration cannot quietly
        // fix: a missing column needs a backup and a data copy, and a renamed one needs the
        // Python-era import rewritten. Section 3.5 of the blueprint is the reference.
        let storage = migrated();
        let columns = |table: &str| -> Vec<String> {
            let mut statement = storage
                .conn
                .prepare(&format!(
                    "SELECT name FROM pragma_table_info('{table}') ORDER BY cid"
                ))
                .unwrap();
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<String>, _>>()
                .unwrap();
            rows
        };

        assert_eq!(
            columns("connection"),
            vec![
                "id",
                "name",
                "kind",
                "host",
                "port",
                "user_name",
                "database_name",
                "options_json",
                "is_production",
                "is_read_only",
                "secret_ref",
                "group_id",
                "sort_order",
                "updated_at",
                "deleted_at",
                "version",
            ]
        );
        assert_eq!(
            columns("connection_group"),
            vec![
                "id",
                "name",
                "parent_id",
                "updated_at",
                "deleted_at",
                "version"
            ]
        );
        assert_eq!(
            columns("query_history"),
            vec![
                "id",
                "connection_id",
                "sql_text",
                "started_at",
                "elapsed_ms",
                "row_count",
                "outcome",
                "error_text",
                "updated_at",
                "deleted_at",
                "version",
            ]
        );
        assert_eq!(
            columns("saved_query"),
            vec![
                "id",
                "name",
                "sql_text",
                "connection_id",
                "folder_id",
                "updated_at",
                "deleted_at",
                "version",
            ]
        );
        assert_eq!(
            columns("session_restore"),
            vec![
                "id",
                "tab_json",
                "active_tab_id",
                "updated_at",
                "deleted_at",
                "version"
            ]
        );
        assert_eq!(
            columns("schema_migration"),
            vec!["version", "applied_at", "name"]
        );
    }

    #[test]
    fn the_indexes_the_queries_depend_on_exist() {
        let storage = migrated();
        let mut names = {
            let mut statement = storage
                .conn
                .prepare(
                    "SELECT name FROM sqlite_master WHERE type = 'index' \
                     AND name NOT LIKE 'sqlite_%'",
                )
                .unwrap();
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<String>, _>>()
                .unwrap();
            rows
        };
        names.sort();
        assert_eq!(
            names,
            vec![
                "idx_connection_live",
                "idx_history_dedupe",
                "idx_history_started"
            ],
            "the live-connections list, the autosave de-duplication, and the history order"
        );

        // The connection index is partial, which is what keeps it small while tombstones
        // accumulate: it covers living rows only.
        let sql = text_column(
            &storage,
            "SELECT sql FROM sqlite_master WHERE name = 'idx_connection_live'",
        );
        assert!(sql.contains("WHERE deleted_at IS NULL"), "{sql}");
    }
}
