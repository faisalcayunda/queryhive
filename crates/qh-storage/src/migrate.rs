//! The migrations, and the rules for running them.
//!
//! One file per version, applied inside a single transaction, with two records of where
//! the database is: SQLite's own `user_version`, which is a number read without a query,
//! and the `schema_migration` table, which says which migration that number means and
//! when it ran. Two records for one fact is a chance for them to disagree, so a database
//! whose marker and history do not match is refused rather than guessed at — the failure
//! mode of guessing is applying a migration twice, and this file's whole purpose is that
//! no migration ever runs twice.

use rusqlite::Connection;

use crate::StorageError;

/// One migration, embedded in the binary.
///
/// Embedded rather than read from disk at run time: the files ship inside the
/// application bundle, and a missing or edited migration file on a user's machine would
/// be a corrupted database with no error message.
pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

/// Every migration, oldest first. Append-only: a version that has shipped is never
/// changed, because some database somewhere has already run it.
pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "init",
        sql: include_str!("../migrations/0001_init.sql"),
    },
    Migration {
        version: 2,
        name: "legacy_import",
        sql: include_str!("../migrations/0002_legacy_import.sql"),
    },
];

/// A migration that has run, as the history table remembers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedMigration {
    pub version: i64,
    pub name: String,
    /// Unix milliseconds.
    pub applied_at: i64,
}

/// The highest version this binary knows how to build.
pub fn latest_version() -> i64 {
    MIGRATIONS.last().map_or(0, |migration| migration.version)
}

/// Bring a database up to date, and report what was applied.
///
/// Returns an empty list for a database that is already current, which is the ordinary
/// case: this runs on every launch.
pub fn run(conn: &mut Connection, at: i64) -> Result<Vec<AppliedMigration>, StorageError> {
    let current = user_version(conn)?;
    let known = latest_version();

    if current > known {
        // A newer build of the app wrote this database. Applying nothing is right, and
        // so is saying so: silently continuing would let this build write rows the other
        // one cannot read.
        return Err(StorageError::NewerDatabase {
            found: current,
            known,
        });
    }

    match current {
        0 => {
            // Either brand new, or a database this engine did not create. A fresh file
            // has no tables at all; anything else is the Python-era database, whose
            // migration is a separate, verified, backed-up operation
            // (`docs/migrations/python-to-rust.md`) rather than something to guess at.
            if let Some(object) = first_user_object(conn)? {
                return Err(StorageError::ForeignDatabase { table: object });
            }
        }
        _ => {
            // A marker with no history table at all is the shape a foreign database takes
            // when it happens to carry a `user_version`: nothing of ours has run here, and
            // `history()` would fail inside SQLite with "no such table", which tells the
            // user nothing. It is the same disagreement as any other, so it is reported as
            // one.
            if !has_table(conn, "schema_migration")? {
                return Err(StorageError::InconsistentHistory {
                    user_version: current,
                    recorded: Vec::new(),
                });
            }
            let applied = history(conn)?;
            let versions: Vec<i64> = applied.iter().map(|row| row.version).collect();
            let expected: Vec<i64> = MIGRATIONS
                .iter()
                .filter(|migration| migration.version <= current)
                .map(|migration| migration.version)
                .collect();
            if versions != expected {
                return Err(StorageError::InconsistentHistory {
                    user_version: current,
                    recorded: versions,
                });
            }
        }
    }

    let mut applied = Vec::new();
    for migration in MIGRATIONS
        .iter()
        .filter(|migration| migration.version > current)
    {
        // One transaction per migration, so a failure leaves the database at the
        // previous version rather than half-way through this one.
        let transaction = conn.transaction()?;
        transaction.execute_batch(migration.sql)?;
        transaction.execute(
            "INSERT INTO schema_migration (version, applied_at, name) VALUES (?1, ?2, ?3)",
            rusqlite::params![migration.version, at, migration.name],
        )?;
        // Inside the transaction, so the marker and the history move together or not at
        // all. `pragma_update` rather than `pragma_query_value`: `user_version` answers
        // with nothing.
        transaction.pragma_update(None, "user_version", migration.version)?;
        transaction.commit()?;

        applied.push(AppliedMigration {
            version: migration.version,
            name: migration.name.to_owned(),
            applied_at: at,
        });
    }

    Ok(applied)
}

/// SQLite's fast marker for which migration last ran.
pub fn user_version(conn: &Connection) -> Result<i64, StorageError> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

/// Everything the history table remembers, oldest first.
pub fn history(conn: &Connection) -> Result<Vec<AppliedMigration>, StorageError> {
    let mut statement = conn
        .prepare("SELECT version, name, applied_at FROM schema_migration ORDER BY version ASC")?;
    let rows = statement.query_map([], |row| {
        Ok(AppliedMigration {
            version: row.get(0)?,
            name: row.get(1)?,
            applied_at: row.get(2)?,
        })
    })?;
    let mut applied = Vec::new();
    for row in rows {
        applied.push(row?);
    }
    Ok(applied)
}

/// The name of any object this engine did not create, for telling a fresh database from a
/// stranger's.
///
/// Objects rather than tables: a file whose only content is a view is still somebody's
/// database, and creating our tables inside it would be the same silent takeover as doing it
/// beside their tables. Indices and triggers are included for the same reason — none of them
/// can appear in a file we made before the first migration.
fn first_user_object(conn: &Connection) -> Result<Option<String>, StorageError> {
    let mut statement = conn.prepare(
        "SELECT name FROM sqlite_master \
         WHERE type IN ('table', 'view', 'index', 'trigger') \
           AND name NOT LIKE 'sqlite_%' LIMIT 1",
    )?;
    let mut rows = statement.query([])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

/// Whether a table exists, without asking SQLite to run a statement that would fail.
fn has_table(conn: &Connection, name: &str) -> Result<bool, StorageError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        rusqlite::params![name],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        Connection::open_in_memory().expect("in-memory database")
    }

    #[test]
    fn a_fresh_database_is_brought_to_the_latest_version() {
        let mut conn = database();
        assert_eq!(user_version(&conn).unwrap(), 0);
        let applied = run(&mut conn, 1_000).unwrap();
        assert_eq!(applied.len(), MIGRATIONS.len());
        assert_eq!(applied[0].name, "init");
        assert_eq!(applied[0].applied_at, 1_000);
        assert_eq!(user_version(&conn).unwrap(), latest_version());
        assert_eq!(history(&conn).unwrap(), applied);
    }

    #[test]
    fn running_twice_changes_nothing() {
        // This is the launch path, so the second run has to be the dearer case: it must
        // not re-apply anything, and it must not write a second history row.
        let mut conn = database();
        run(&mut conn, 1_000).unwrap();
        let again = run(&mut conn, 2_000).unwrap();
        assert!(again.is_empty(), "nothing to apply");
        let applied = history(&conn).unwrap();
        assert_eq!(applied.len(), MIGRATIONS.len());
        assert_eq!(
            applied[0].applied_at, 1_000,
            "the original timestamp is not rewritten"
        );
    }

    #[test]
    fn a_database_from_a_newer_build_is_left_alone() {
        let mut conn = database();
        conn.pragma_update(None, "user_version", latest_version() + 5)
            .unwrap();
        match run(&mut conn, 1_000) {
            Err(StorageError::NewerDatabase { found, known }) => {
                assert_eq!(found, latest_version() + 5);
                assert_eq!(known, latest_version());
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn tables_this_engine_did_not_create_are_refused_rather_than_migrated_over() {
        // The Python-era database looks like this: tables, and user_version 0. Creating
        // our tables inside it and stamping version 1 would "work" and would be wrong.
        let mut conn = database();
        conn.execute_batch("CREATE TABLE connections (id TEXT PRIMARY KEY, name TEXT);")
            .unwrap();
        match run(&mut conn, 1_000) {
            Err(StorageError::ForeignDatabase { table }) => assert_eq!(table, "connections"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(user_version(&conn).unwrap(), 0, "nothing was stamped");
    }

    #[test]
    fn a_marker_with_no_history_table_is_refused_with_a_reason() {
        // A foreign database that happens to carry a `user_version`. Without this check the
        // failure would come out of SQLite as "no such table: schema_migration", which says
        // nothing about what is actually wrong.
        let mut conn = database();
        conn.execute_batch(
            "CREATE TABLE connections (id TEXT PRIMARY KEY); PRAGMA user_version = 1;",
        )
        .unwrap();
        match run(&mut conn, 1_000) {
            Err(StorageError::InconsistentHistory {
                user_version,
                recorded,
            }) => {
                assert_eq!(user_version, 1);
                assert!(recorded.is_empty());
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[test]
    fn a_database_that_holds_only_a_view_is_still_somebody_elses() {
        // Objects, not just tables: a file whose only content is a view is not ours, and
        // creating our tables inside it is the same silent takeover as doing it beside their
        // tables.
        let mut conn = database();
        conn.execute_batch("CREATE VIEW v AS SELECT 1 AS one;")
            .unwrap();
        match run(&mut conn, 1_000) {
            Err(StorageError::ForeignDatabase { table }) => assert_eq!(table, "v"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert_eq!(user_version(&conn).unwrap(), 0);
    }

    #[test]
    fn a_migration_that_fails_leaves_the_database_at_the_previous_version() {
        // One transaction per version, proven by making the *second* one fail. The starting
        // point is built by hand, because that is the state an interrupted upgrade leaves:
        // version 1 applied and recorded, and an object occupying a name the next migration
        // needs. (A fresh database cannot be used for this: `run` applies every pending
        // migration in one call, so it would never be at 1 when the collision appears.)
        let mut conn = database();
        conn.execute_batch(MIGRATIONS[0].sql).unwrap();
        conn.execute(
            "INSERT INTO schema_migration (version, applied_at, name) VALUES (1, 1000, ?1)",
            rusqlite::params![MIGRATIONS[0].name],
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute_batch("CREATE VIEW legacy_import AS SELECT 1 AS one;")
            .unwrap();
        assert_eq!(user_version(&conn).unwrap(), 1);

        let collision = run(&mut conn, 2_000);
        assert!(
            collision.is_err(),
            "the collision is a failure, not a silent skip: {collision:?}"
        );
        assert_eq!(user_version(&conn).unwrap(), 1, "the marker did not move");
        assert_eq!(
            history(&conn).unwrap().len(),
            1,
            "and neither did the history"
        );
        // What an earlier version did is untouched: rolling back a failed migration must not
        // roll back the ones that already succeeded.
        assert!(has_table(&conn, "connection").unwrap());
    }

    #[test]
    fn a_marker_that_disagrees_with_the_history_is_refused() {
        let mut conn = database();
        run(&mut conn, 1_000).unwrap();
        // Exactly the shape of a bug worth catching: somebody moved the marker without
        // running the migration, or restored half a backup.
        conn.execute_batch("DELETE FROM schema_migration").unwrap();
        match run(&mut conn, 2_000) {
            Err(StorageError::InconsistentHistory {
                user_version,
                recorded,
            }) => {
                assert_eq!(user_version, latest_version());
                assert!(recorded.is_empty());
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }
}
