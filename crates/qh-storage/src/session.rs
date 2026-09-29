//! Session restore: which query tabs were open, and which one was in front.
//!
//! One row, always, under a fixed identity. The schema names the columns `tab_json` and
//! `active_tab_id`, and one row is the only shape where "which tab was in front" is a single
//! value rather than a copy per tab. The tab order is part of what is restored and a set of
//! rows has no order of its own — there is no sort column here, and adding one would be a
//! migration for an order the JSON already carries. So the tabs live in one blob, in the
//! order the app wrote them, and this module never looks inside it.
//!
//! # The blob is opaque, and that is the point
//!
//! What a tab is made of is the app's business, not this database's. This module stores the
//! exact bytes it was handed and gives them back the same way, so a field added to a tab is
//! one the app round-trips without a migration — the same reason a connection's `options` is
//! kept as text. The engine refuses a save whose blob is not JSON at all, because a session
//! that cannot be parsed is better reported at quit than discovered at the next launch.
//!
//! # Why the revision still moves
//!
//! The row is rewritten whole on every save, so the revision is not load-bearing today. It is
//! kept because it is what every row in this database carries (`qh-sync`), and a future sync
//! that finds a session row should not have to special-case it: a rewrite is an edit, and an
//! edit moves the version forward rather than back to one.

use qh_sync::{SyncMeta, Version};
use rusqlite::{params, OptionalExtension};

use crate::history::to_sync_id;
use crate::{Storage, StorageError};

/// The identity of the one session row.
///
/// A fixed UUID rather than a fresh `SyncId::now()` per write: the session is a singleton, and
/// a new identity each time would leave the previous row behind as an orphan the next read
/// would have to choose between. Spelled as a version-7 UUID so it is the same kind of value
/// as every other id in this database, even though nothing sorts by it.
pub const SESSION_ID: &str = "00000000-0000-7000-8000-000000000001";

/// The stored session: the tabs, and which one was in front.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub meta: SyncMeta,
    /// The tabs exactly as the app wrote them. Opaque here; see the module note.
    pub tabs_json: String,
    /// The tab that was in front, by the identity the app gave it. `None` is a real value: a
    /// session written before a tab was selected has no front tab, which is not the same as
    /// an empty session.
    pub active_tab_id: Option<String>,
}

const SESSION_COLUMNS: &str = "id, tab_json, active_tab_id, updated_at, deleted_at, version";

impl Storage {
    /// Write the session, replacing whatever was there.
    ///
    /// Returns nothing: there is one row, so there is no identity to hand back and nothing
    /// for the caller to choose between.
    pub fn save_session(
        &self,
        tabs_json: &str,
        active_tab_id: Option<&str>,
        at: i64,
    ) -> Result<(), StorageError> {
        let version = self.next_session_version()?;
        self.upsert_session(tabs_json, active_tab_id, at, version, None)
    }

    /// The living session, or `None` when there is none.
    pub fn session(&self) -> Result<Option<SessionRecord>, StorageError> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {SESSION_COLUMNS} FROM session_restore \
                     WHERE id = ?1 AND deleted_at IS NULL"
                ),
                params![SESSION_ID],
                read_session,
            )
            .optional()?)
    }

    /// Forget the session, keeping the row as a tombstone.
    ///
    /// Returns whether there was a living session to forget, so a caller can say what it did.
    pub fn clear_session(&self, at: i64) -> Result<bool, StorageError> {
        let Some(record) = self.session()? else {
            return Ok(false);
        };
        let version = record.meta.version.next();
        self.upsert_session(
            &record.tabs_json,
            record.active_tab_id.as_deref(),
            at,
            version,
            Some(at),
        )?;
        Ok(true)
    }

    /// The revision a rewrite should carry.
    ///
    /// Read including tombstones, so clearing the session and then saving again continues the
    /// count rather than restarting it — the same reason `record_history` reads a deleted row
    /// before reviving it.
    fn next_session_version(&self) -> Result<Version, StorageError> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT version FROM session_restore WHERE id = ?1",
                params![SESSION_ID],
                |row| row.get(0),
            )
            .optional()?;
        Ok(existing.map_or(Version::FIRST, |version| Version::from(version).next()))
    }

    fn upsert_session(
        &self,
        tabs_json: &str,
        active_tab_id: Option<&str>,
        at: i64,
        version: Version,
        deleted_at: Option<i64>,
    ) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO session_restore (id, tab_json, active_tab_id, updated_at, deleted_at, \
              version) VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(id) DO UPDATE SET tab_json = excluded.tab_json, \
              active_tab_id = excluded.active_tab_id, updated_at = excluded.updated_at, \
              deleted_at = excluded.deleted_at, version = excluded.version",
            params![
                SESSION_ID,
                tabs_json,
                active_tab_id,
                at,
                deleted_at,
                version.get(),
            ],
        )?;
        Ok(())
    }
}

/// One session row as it stands in the database.
fn read_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    let id: String = row.get(0)?;
    let deleted_at: Option<i64> = row.get(4)?;
    let version: i64 = row.get(5)?;
    Ok(SessionRecord {
        meta: SyncMeta {
            id: to_sync_id(&id)?,
            updated_at: row.get(3)?,
            deleted_at,
            version: Version::from(version),
        },
        tabs_json: row.get(1)?,
        active_tab_id: row.get(2)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: i64 = 1_700_000_000_000;
    const TABS: &str = r#"[{"id":"a","sql":"SELECT 1"}]"#;

    fn storage() -> Storage {
        let mut storage = Storage::in_memory().expect("in-memory database");
        storage.migrate_at(AT).expect("migrate");
        storage
    }

    #[test]
    fn a_session_reads_back_as_it_was_written() {
        let storage = storage();
        storage
            .save_session(TABS, Some("a"), AT)
            .expect("save the session");

        let read = storage.session().expect("read").expect("the row is there");
        assert_eq!(read.tabs_json, TABS);
        assert_eq!(read.active_tab_id.as_deref(), Some("a"));
        assert!(!read.meta.is_deleted());
    }

    #[test]
    fn a_save_replaces_the_previous_session_rather_than_adding_one() {
        // The singleton rule. A second save with a fresh identity would leave the first row
        // behind, and the next read would have to pick between two sessions.
        let storage = storage();
        storage.save_session(TABS, Some("a"), AT).expect("first");
        storage
            .save_session(r#"[{"id":"b","sql":"SELECT 2"}]"#, Some("b"), AT + 1)
            .expect("second");

        let read = storage.session().expect("read").expect("there");
        assert_eq!(read.tabs_json, r#"[{"id":"b","sql":"SELECT 2"}]"#);
        assert_eq!(read.active_tab_id.as_deref(), Some("b"));
        assert!(
            read.meta.version.get() > 1,
            "a rewrite is an edit: the revision moved"
        );
    }

    #[test]
    fn a_session_with_no_front_tab_keeps_that_as_none() {
        // `None` and `Some("")` are different answers, and only the first is what "no tab was
        // selected" means.
        let storage = storage();
        storage.save_session(TABS, None, AT).expect("save");
        assert_eq!(
            storage
                .session()
                .expect("read")
                .expect("there")
                .active_tab_id,
            None
        );
    }

    #[test]
    fn clearing_forgets_the_session_and_reports_whether_there_was_one() {
        let storage = storage();
        assert!(!storage.clear_session(AT).expect("nothing to clear"));
        storage.save_session(TABS, Some("a"), AT).expect("save");
        assert!(storage.clear_session(AT + 1).expect("clear"));
        assert!(storage.session().expect("read").is_none());
        assert!(
            !storage.clear_session(AT + 2).expect("clear again"),
            "clearing twice does not report the tombstone"
        );
    }

    #[test]
    fn saving_after_a_clear_moves_the_revision_forward_rather_than_back() {
        // The revive path, and the only reason `next_session_version` reads tombstones: writing
        // the newcomer's first revision over the deleted row's would date the session before its
        // own deletion.
        let storage = storage();
        storage.save_session(TABS, Some("a"), AT).expect("save");
        let before = storage.session().unwrap().unwrap().meta.version;
        storage.clear_session(AT + 1).expect("clear");
        storage
            .save_session(TABS, None, AT + 2)
            .expect("save again");

        let read = storage.session().expect("read").expect("revived");
        assert!(read.meta.version.get() > before.get());
        assert!(!read.meta.is_deleted());
    }

    #[test]
    fn a_session_outlives_the_connection_that_wrote_it() {
        // The claim the whole feature rests on: quit, launch, and the tabs are still there.
        // Every other test in this file shares one connection, so a write that never committed
        // would pass all of them.
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("qh.sqlite3");

        let mut first = Storage::open(&path).expect("open");
        first.migrate_at(AT).expect("migrate");
        first
            .save_session(TABS, Some("a"), AT)
            .expect("save the session");
        drop(first);

        let mut second = Storage::open(&path).expect("reopen");
        second
            .migrate_at(AT + 1)
            .expect("migrate again, nothing to do");
        let read = second.session().expect("read").expect("still there");
        assert_eq!(read.tabs_json, TABS);
        assert_eq!(read.active_tab_id.as_deref(), Some("a"));
    }
}
