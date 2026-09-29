//! Query history and saved queries.
//!
//! Two tables the app has always had a schema for and no code against: `query_history` is
//! what a Run leaves behind, and `saved_query` is what a user pins on purpose. Both are
//! written through [`qh_sync::SyncMeta`] like every other row in this database, so the
//! revision and the tombstone follow the same rules the connections do: a change bumps the
//! version, a deletion is a change, and a deleted row stays in the table for a future sync
//! to find.
//!
//! # Why recording is not a plain upsert
//!
//! `0001_init.sql` puts a unique index on `(connection_id, sql_text, started_at)` and says
//! why: two identical queries on one connection at the same millisecond are the same event
//! saved twice, which the autosave path can do when a connection drops mid-query. Two saves
//! of one event carry two different ids, so upserting on `id` alone would not merge them. It
//! would hit that index instead. [`Storage::record_history`] looks the event up first,
//! and the id it returns is the id actually written, which is how a caller can tell the two
//! cases apart without being told.
//!
//! # One timestamp per operation
//!
//! `at` arrives from the caller, exactly as it does in `connections.rs`: a batch of writes
//! shares one reading of the clock, and a test does not depend on how long it took to run.

use qh_sync::{SyncId, SyncMeta, Version};
use rusqlite::{params, OptionalExtension};

use crate::{Storage, StorageError};

/// How an execution ended.
///
/// The column is nullable, so a record carries `Option<Outcome>`: a row written by an older
/// build, or by a process that died before it could say, has no outcome and is not the same
/// as a row whose outcome is `Ok`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    Error,
    Cancelled,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(text: &str) -> Result<Self, StorageError> {
        match text {
            "ok" => Ok(Self::Ok),
            "error" => Ok(Self::Error),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(StorageError::UnknownOutcome {
                text: other.to_owned(),
            }),
        }
    }
}

/// One execution, as it is written down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryHistoryRecord {
    pub meta: SyncMeta,
    /// The connection it ran on. `None` is a real value: a query can be recorded before the
    /// connection it belongs to has been saved.
    pub connection_id: Option<SyncId>,
    pub sql_text: String,
    /// When the statement started, which is not the same as when the row was written: the
    /// dedupe index is on this column, so it has to be the event's own time.
    pub started_at: i64,
    pub elapsed_ms: Option<i64>,
    pub row_count: Option<i64>,
    pub outcome: Option<Outcome>,
    /// What the server or the driver said, when the outcome is not `Ok`.
    pub error_text: Option<String>,
}

/// One statement a user chose to keep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedQueryRecord {
    pub meta: SyncMeta,
    pub name: String,
    pub sql_text: String,
    pub connection_id: Option<SyncId>,
    /// Reserved by the schema and unused by this build. Kept so a folder feature does not
    /// need a migration, and so a row written by one that has it reads back here.
    pub folder_id: Option<SyncId>,
    /// Whether the user keeps this query within reach.
    ///
    /// A property of the row rather than of a group: a query can be favourited and be in a folder at
    /// the same time, and neither implies the other.
    pub favourite: bool,
}

const HISTORY_COLUMNS: &str = "id, connection_id, sql_text, started_at, elapsed_ms, row_count, \
     outcome, error_text, updated_at, deleted_at, version";

const SAVED_QUERY_COLUMNS: &str =
    "id, name, sql_text, connection_id, folder_id, updated_at, deleted_at, version, favourite";

impl Storage {
    /// Write down one execution.
    ///
    /// Returns the id the row actually has, which is the record's own unless an identical
    /// event (same connection, same text, same start) is already stored. In that case the
    /// existing row is updated in place, keeping its identity, because the schema says the
    /// two are one event rather than two. A tombstone loses to the same rule: re-recording
    /// an event that was cleared brings the row back under its old id, which is the honest
    /// reading of "the same event happened again".
    pub fn record_history(&self, record: &QueryHistoryRecord) -> Result<SyncId, StorageError> {
        // `IS` rather than `=`: a null connection is a value the comparison has to match,
        // and `=` never matches null in SQL.
        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM query_history \
                 WHERE connection_id IS ?1 AND sql_text = ?2 AND started_at = ?3",
                params![
                    record.connection_id.as_ref().map(SyncId::as_str),
                    record.sql_text,
                    record.started_at
                ],
                |row| row.get(0),
            )
            .optional()?;

        let mut stored = record.clone();
        if let Some(id) = existing {
            let id = SyncId::parse(&id).map_err(|error| StorageError::BadId {
                text: id,
                reason: error.to_string(),
            })?;
            // Reusing the id is not the whole job. The row already has a revision, and this
            // write is a change to it, so the revision moves forward rather than falling back
            // to the newcomer's first version. Without this, clearing the history and running
            // the same statement again writes version 1 over version 2 and dates the row
            // before its own deletion, which is the opposite of what the module note promises.
            // The tombstone is lifted in the same write, because the event really did happen
            // again.
            if let Some(previous) = self.history_entry(&id)? {
                stored.meta.version = previous.meta.version.next();
                stored.meta.deleted_at = None;
            }
            stored.meta.id = id;
        }
        self.write_history(&stored)?;
        Ok(stored.meta.id)
    }

    /// Write a history row as given, replacing the row with that identity.
    ///
    /// Private on purpose: [`Self::record_history`] is the only way in, so the dedupe rule
    /// cannot be stepped around by calling the write directly.
    fn write_history(&self, record: &QueryHistoryRecord) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO query_history (id, connection_id, sql_text, started_at, elapsed_ms, \
              row_count, outcome, error_text, updated_at, deleted_at, version) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
             ON CONFLICT(id) DO UPDATE SET \
              connection_id = excluded.connection_id, sql_text = excluded.sql_text, \
              started_at = excluded.started_at, elapsed_ms = excluded.elapsed_ms, \
              row_count = excluded.row_count, outcome = excluded.outcome, \
              error_text = excluded.error_text, updated_at = excluded.updated_at, \
              deleted_at = excluded.deleted_at, version = excluded.version",
            params![
                record.meta.id.as_str(),
                record.connection_id.as_ref().map(SyncId::as_str),
                record.sql_text,
                record.started_at,
                record.elapsed_ms,
                record.row_count,
                record.outcome.map(Outcome::as_str),
                record.error_text,
                record.meta.updated_at,
                record.meta.deleted_at,
                record.meta.version.get(),
            ],
        )?;
        Ok(())
    }

    /// One history row by identity, tombstone included.
    pub fn history_entry(&self, id: &SyncId) -> Result<Option<QueryHistoryRecord>, StorageError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {HISTORY_COLUMNS} FROM query_history WHERE id = ?1"),
                params![id.as_str()],
                read_history,
            )
            .optional()?)
    }

    /// The most recent living entries, newest first.
    pub fn history(&self, limit: usize) -> Result<Vec<QueryHistoryRecord>, StorageError> {
        self.history_where(
            "WHERE deleted_at IS NULL ORDER BY started_at DESC, id DESC",
            None,
            limit,
        )
    }

    /// The most recent living entries for one connection, newest first.
    pub fn history_for_connection(
        &self,
        connection_id: &SyncId,
        limit: usize,
    ) -> Result<Vec<QueryHistoryRecord>, StorageError> {
        self.history_where(
            "WHERE deleted_at IS NULL AND connection_id = ?1 ORDER BY started_at DESC, id DESC",
            Some(connection_id),
            limit,
        )
    }

    /// Every history row, tombstones included. What a sync would send.
    pub fn history_including_deleted(&self) -> Result<Vec<QueryHistoryRecord>, StorageError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {HISTORY_COLUMNS} FROM query_history ORDER BY id ASC"
        ))?;
        let rows = statement.query_map([], read_history)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    /// History entries whose statement matches `needle`, newest first.
    ///
    /// The needle becomes an FTS5 query through [`fts_query`] rather than being passed through,
    /// which is where both the safety and the usefulness come from.
    ///
    /// Cleared entries are excluded here rather than by keeping the index clean: a soft delete is
    /// an update, and the update trigger re-indexes the same text, so this filter is the thing
    /// that decides what a search may return.
    pub fn search_history(
        &self,
        needle: &str,
        connection_id: Option<&SyncId>,
        limit: usize,
    ) -> Result<Vec<QueryHistoryRecord>, StorageError> {
        let query = fts_query(needle);
        // Nothing to search for. Worth returning early rather than running: an empty FTS5 query is
        // a syntax error, not an empty result.
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let mut statement = self.conn.prepare(&format!(
            "SELECT {HISTORY_COLUMNS} FROM query_history \
             WHERE deleted_at IS NULL AND rowid IN \
              (SELECT rowid FROM query_history_fts WHERE query_history_fts MATCH ?1){} \
             ORDER BY started_at DESC, id DESC LIMIT ?2",
            if connection_id.is_some() {
                " AND connection_id = ?3"
            } else {
                ""
            }
        ))?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        // Two branches rather than one with a placeholder because SQLite counts the parameters the
        // statement actually names: binding a third on the unfiltered query is a range error, not
        // an ignored argument.
        let rows = match connection_id {
            Some(id) => statement.query_map(params![query, limit, id.as_str()], read_history)?,
            None => statement.query_map(params![query, limit], read_history)?,
        };
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    /// Delete one entry, keeping the row.
    ///
    /// Returns whether there was a row to delete; deleting a tombstone again is a no-op,
    /// not an error, for the same reason it is one in `connections.rs`.
    pub fn soft_delete_history(&self, id: &SyncId, at: i64) -> Result<bool, StorageError> {
        let Some(mut record) = self.history_entry(id)? else {
            return Ok(false);
        };
        if record.meta.is_deleted() {
            return Ok(true);
        }
        record.meta.mark_deleted(at);
        self.write_history(&record)?;
        Ok(true)
    }

    /// Clear the history, all of it or one connection's, and report how many rows that was.
    ///
    /// Soft, like every other deletion here: the rows stay as tombstones so a sync can see
    /// that they went away. The count is of rows that were living, so calling it twice
    /// reports zero the second time.
    pub fn clear_history(
        &self,
        connection_id: Option<&SyncId>,
        at: i64,
    ) -> Result<usize, StorageError> {
        let living = match connection_id {
            Some(id) => self.history_where(
                "WHERE deleted_at IS NULL AND connection_id = ?1 ORDER BY started_at ASC",
                Some(id),
                usize::MAX,
            )?,
            None => self.history_where(
                "WHERE deleted_at IS NULL ORDER BY started_at ASC",
                None,
                usize::MAX,
            )?,
        };
        let mut cleared = 0;
        for mut record in living {
            record.meta.mark_deleted(at);
            self.write_history(&record)?;
            cleared += 1;
        }
        Ok(cleared)
    }

    fn history_where(
        &self,
        clause: &str,
        connection_id: Option<&SyncId>,
        limit: usize,
    ) -> Result<Vec<QueryHistoryRecord>, StorageError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {HISTORY_COLUMNS} FROM query_history {clause} LIMIT ?2"
        ))?;
        // A `usize` larger than `i64` would wrap to a negative limit, which SQLite reads as
        // "no limit", the opposite of what was asked. Clamping keeps the two agreeing.
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = match connection_id {
            Some(id) => statement.query_map(params![id.as_str(), limit], read_history)?,
            None => statement.query_map(params![rusqlite::types::Null, limit], read_history)?,
        };
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    /// Write a saved query, replacing the row with that identity.
    pub fn save_query(&self, record: &SavedQueryRecord) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO saved_query (id, name, sql_text, connection_id, folder_id, \
              updated_at, deleted_at, version, favourite) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, sql_text = excluded.sql_text, \
              connection_id = excluded.connection_id, folder_id = excluded.folder_id, \
              updated_at = excluded.updated_at, deleted_at = excluded.deleted_at, \
              version = excluded.version, favourite = excluded.favourite",
            params![
                record.meta.id.as_str(),
                record.name,
                record.sql_text,
                record.connection_id.as_ref().map(SyncId::as_str),
                record.folder_id.as_ref().map(SyncId::as_str),
                record.meta.updated_at,
                record.meta.deleted_at,
                record.meta.version.get(),
                i64::from(record.favourite),
            ],
        )?;
        Ok(())
    }

    /// One saved query by identity, tombstone included.
    pub fn saved_query(&self, id: &SyncId) -> Result<Option<SavedQueryRecord>, StorageError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {SAVED_QUERY_COLUMNS} FROM saved_query WHERE id = ?1"),
                params![id.as_str()],
                read_saved_query,
            )
            .optional()?)
    }

    /// Every living saved query, in name order.
    pub fn saved_queries(&self) -> Result<Vec<SavedQueryRecord>, StorageError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {SAVED_QUERY_COLUMNS} FROM saved_query \
             WHERE deleted_at IS NULL ORDER BY name ASC"
        ))?;
        let rows = statement.query_map([], read_saved_query)?;
        let mut queries = Vec::new();
        for row in rows {
            queries.push(row?);
        }
        Ok(queries)
    }

    /// Rename a saved query, as a revision like any other edit.
    ///
    /// Returns whether there was a row to rename. Whitelisted by a write of the whole
    /// record rather than an `UPDATE name = ?`: the revision has to move with the name, and
    /// a statement that touched only the name would leave the two disagreeing.
    pub fn rename_saved_query(
        &self,
        id: &SyncId,
        name: impl Into<String>,
        at: i64,
    ) -> Result<bool, StorageError> {
        let Some(mut record) = self.saved_query(id)? else {
            return Ok(false);
        };
        record.name = name.into();
        record.meta.updated_at = at;
        record.meta.version = record.meta.version.next();
        self.save_query(&record)?;
        Ok(true)
    }

    /// Delete a saved query, keeping the row.
    pub fn soft_delete_saved_query(&self, id: &SyncId, at: i64) -> Result<bool, StorageError> {
        let Some(mut record) = self.saved_query(id)? else {
            return Ok(false);
        };
        if record.meta.is_deleted() {
            return Ok(true);
        }
        record.meta.mark_deleted(at);
        self.save_query(&record)?;
        Ok(true)
    }

    /// Keep a saved query within reach, or stop keeping it.
    ///
    /// A revision like any other edit, written the way `rename_saved_query` is: read the whole
    /// record, change one field, write it back. An `UPDATE favourite = ?` would leave the revision
    /// behind, so a second device reconciling this row would hold an older copy of the same edit and
    /// have no reason to prefer the newer one.
    ///
    /// Returns whether there was a row to change. Favouriting a row that is already in that state
    /// returns `true` without writing: bumping a revision to record that nothing changed would make
    /// every reconciliation see an edit where there was none.
    pub fn set_saved_query_favourite(
        &self,
        id: &SyncId,
        favourite: bool,
        at: i64,
    ) -> Result<bool, StorageError> {
        let Some(mut record) = self.saved_query(id)? else {
            return Ok(false);
        };
        if record.favourite == favourite {
            return Ok(true);
        }
        record.favourite = favourite;
        record.meta.updated_at = at;
        record.meta.version = record.meta.version.next();
        self.save_query(&record)?;
        Ok(true)
    }
}

impl QueryHistoryRecord {
    /// A living entry that has not finished yet, at its first revision.
    ///
    /// `outcome`, `elapsed_ms` and `row_count` start empty because that is what is true at
    /// the moment the statement is sent: a record written before the run finishes is not a
    /// record with a wrong answer, it is one without one yet.
    pub fn new(
        connection_id: Option<SyncId>,
        sql_text: impl Into<String>,
        started_at: i64,
    ) -> Self {
        Self {
            meta: SyncMeta::new(SyncId::now(), started_at),
            connection_id,
            sql_text: sql_text.into(),
            started_at,
            elapsed_ms: None,
            row_count: None,
            outcome: None,
            error_text: None,
        }
    }
}

impl SavedQueryRecord {
    /// A living saved query at its first revision.
    pub fn new(
        name: impl Into<String>,
        sql_text: impl Into<String>,
        connection_id: Option<SyncId>,
        at: i64,
    ) -> Self {
        Self {
            meta: SyncMeta::new(SyncId::now(), at),
            name: name.into(),
            sql_text: sql_text.into(),
            connection_id,
            folder_id: None,
            favourite: false,
        }
    }
}

/// The FTS5 query a user's words become.
///
/// Every term is quoted, given a `*`, and the terms are ANDed. Each of those is load-bearing:
///
/// - **Quoted**, because FTS5 reads `*`, `NEAR`, `AND` and a bare `"` as syntax. A user typing
///   `select *` or `don't` would otherwise get a failing search rather than a result, and the
///   failure would reach the panel as a SQL error. Doubling an inner quote is FTS5's own escape
///   inside a quoted phrase.
/// - **Prefixed**, because `unicode61` keeps `penerima_manfaat` as a single token. Without the
///   `*`, a search for `penerima` finds nothing, which is the wrong answer to a fair question.
/// - **ANDed**, because a person typing two words means both of them rather than either.
///
/// One property of the tokenizer is worth stating rather than leaving implicit: `unicode61` defaults
/// to `remove_diacritics=1`, so `cafe` and `café` are the same token and either spelling finds both.
/// For a history written mostly in Indonesian that is the answer a person wants, and it is not
/// something this function can turn off: the tokenizer is fixed in the table definition, and
/// changing it would mean rebuilding the index.
///
/// A term with no token characters in it (`*`, `--`, `(`, `=`) is dropped rather than quoted: an
/// empty quoted phrase is itself a syntax error, and punctuation alone is not a word anyone meant
/// to search for. If every term is dropped the caller gets no results, not an error.
fn fts_query(needle: &str) -> String {
    needle
        .split_whitespace()
        .filter(|term| term.chars().any(|c| c.is_alphanumeric() || c == '_'))
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// One row of `query_history` as it stands in the database.
///
/// Read by index, with the list in `HISTORY_COLUMNS` right above: the two are meant to be
/// checkable against each other by eye, and a mismatch is a programming error rather than
/// user input.
fn read_history(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueryHistoryRecord> {
    let id: String = row.get(0)?;
    let connection_id: Option<String> = row.get(1)?;
    let outcome: Option<String> = row.get(6)?;
    let deleted_at: Option<i64> = row.get(9)?;
    let version: i64 = row.get(10)?;
    Ok(QueryHistoryRecord {
        meta: SyncMeta {
            id: to_sync_id(&id)?,
            updated_at: row.get(8)?,
            deleted_at,
            version: Version::from(version),
        },
        connection_id: match connection_id {
            Some(text) => Some(to_sync_id(&text)?),
            None => None,
        },
        sql_text: row.get(2)?,
        started_at: row.get(3)?,
        elapsed_ms: row.get(4)?,
        row_count: row.get(5)?,
        outcome: match outcome {
            // A row that says something this build does not know fails the listing, the same
            // way `read_connection` fails it. Reporting rather than skipping is deliberate: a
            // row this build cannot read is a fact about the database, and a listing that
            // quietly dropped it would hide the only symptom there is.
            Some(text) => Some(Outcome::parse(&text).map_err(|error| to_sql_error(&error))?),
            None => None,
        },
        error_text: row.get(7)?,
    })
}

fn read_saved_query(row: &rusqlite::Row<'_>) -> rusqlite::Result<SavedQueryRecord> {
    let id: String = row.get(0)?;
    let connection_id: Option<String> = row.get(3)?;
    let folder_id: Option<String> = row.get(4)?;
    let deleted_at: Option<i64> = row.get(6)?;
    let version: i64 = row.get(7)?;
    Ok(SavedQueryRecord {
        meta: SyncMeta {
            id: to_sync_id(&id)?,
            updated_at: row.get(5)?,
            deleted_at,
            version: Version::from(version),
        },
        name: row.get(1)?,
        sql_text: row.get(2)?,
        connection_id: match connection_id {
            Some(text) => Some(to_sync_id(&text)?),
            None => None,
        },
        folder_id: match folder_id {
            Some(text) => Some(to_sync_id(&text)?),
            None => None,
        },
        // Stored as SQLite's 0 or 1, read as a `bool`: the column has an integer affinity, so the
        // number that comes back is whatever a write put there, and `!= 0` is the only reading that
        // stays true for both 1 and a larger value a future writer might choose.
        favourite: row.get::<_, i64>(8)? != 0,
    })
}

pub(crate) fn to_sync_id(text: &str) -> rusqlite::Result<SyncId> {
    SyncId::parse(text).map_err(|error| {
        to_sql_error(&StorageError::BadId {
            text: text.to_owned(),
            reason: error.to_string(),
        })
    })
}

/// A row that is there but says something impossible.
///
/// The same shape `connections.rs` uses: `FromSqlConversionFailure` is rusqlite's own
/// variant for it, and it is what makes the error carry a column index, which is the
/// difference between "one row is corrupt" and "the database is corrupt".
pub(crate) fn to_sql_error(error: &StorageError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            error.to_string(),
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: i64 = 1_700_000_000_000;

    fn storage() -> Storage {
        let mut storage = Storage::in_memory().expect("in-memory database");
        storage.migrate_at(AT).expect("migrate");
        storage
    }

    fn connection() -> SyncId {
        SyncId::now()
    }

    #[test]
    fn an_entry_reads_back_as_it_was_written() {
        let storage = storage();
        let mut record = QueryHistoryRecord::new(Some(connection()), "SELECT 1", AT);
        record.elapsed_ms = Some(3);
        record.row_count = Some(1);
        record.outcome = Some(Outcome::Ok);

        let id = storage.record_history(&record).expect("record");
        let read = storage
            .history_entry(&id)
            .expect("read back")
            .expect("the row is there");
        assert_eq!(read.meta.id, record.meta.id);
        assert_eq!(read.sql_text, "SELECT 1");
        assert_eq!(read.outcome, Some(Outcome::Ok));
        assert_eq!(read.row_count, Some(1));
        assert!(!read.meta.is_deleted());
    }

    #[test]
    fn an_entry_with_no_outcome_yet_is_not_an_error() {
        // A row written before the run finished has no answer, which is a different thing
        // from an answer of `ok`. The column is nullable for this reason.
        let storage = storage();
        let record = QueryHistoryRecord::new(Some(connection()), "SELECT 1", AT);
        let id = storage.record_history(&record).expect("record");
        let read = storage.history_entry(&id).unwrap().unwrap();
        assert_eq!(read.outcome, None);
        assert_eq!(read.elapsed_ms, None);
    }

    #[test]
    fn the_listing_is_newest_first_and_respects_the_limit() {
        let storage = storage();
        let connection = connection();
        for offset in 0..3 {
            let record = QueryHistoryRecord::new(
                Some(connection.clone()),
                format!("SELECT {offset}"),
                AT + offset,
            );
            storage.record_history(&record).expect("record");
        }

        let listed = storage.history(2).expect("list");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].sql_text, "SELECT 2");
        assert_eq!(listed[1].sql_text, "SELECT 1");
    }

    #[test]
    fn one_event_saved_twice_is_one_row() {
        // The reason `idx_history_dedupe` exists. Without the lookup in `record_history`
        // the second save would hit that unique index instead of merging, and the whole
        // point of the index is that the two saves are one event.
        let storage = storage();
        let connection = connection();
        let first = QueryHistoryRecord::new(Some(connection.clone()), "SELECT 1", AT);
        let second = QueryHistoryRecord::new(Some(connection), "SELECT 1", AT);
        assert_ne!(first.meta.id, second.meta.id, "two saves carry two ids");

        let first_id = storage.record_history(&first).expect("first");
        let second_id = storage.record_history(&second).expect("second");
        assert_eq!(first_id, second_id, "the second lands on the first row");
        assert_eq!(storage.history(10).expect("list").len(), 1);
    }

    #[test]
    fn two_events_with_no_connection_are_still_deduped() {
        // `connection_id` is null here, and SQL's `=` would never match it, so this is the
        // test that keeps the `IS` in the lookup from being "simplified" later.
        let storage = storage();
        let first = QueryHistoryRecord::new(None, "SELECT 1", AT);
        let second = QueryHistoryRecord::new(None, "SELECT 1", AT);
        let first_id = storage.record_history(&first).expect("first");
        let second_id = storage.record_history(&second).expect("second");
        assert_eq!(first_id, second_id);
        assert_eq!(storage.history(10).expect("list").len(), 1);
    }

    #[test]
    fn the_same_statement_at_a_different_moment_is_a_second_entry() {
        // The other half of the dedupe rule: running the same query twice is two events,
        // and only the same millisecond makes them one.
        let storage = storage();
        let connection = connection();
        storage
            .record_history(&QueryHistoryRecord::new(
                Some(connection.clone()),
                "SELECT 1",
                AT,
            ))
            .expect("first");
        storage
            .record_history(&QueryHistoryRecord::new(
                Some(connection),
                "SELECT 1",
                AT + 1,
            ))
            .expect("second");
        assert_eq!(storage.history(10).expect("list").len(), 2);
    }

    #[test]
    fn recording_an_event_again_after_a_clear_moves_the_revision_forward() {
        // The revive path, which the reused id alone does not describe: the row already
        // carries the delete's revision, so writing the newcomer's first version would date
        // the row before its own deletion and leave a sync reading a row that went backwards.
        let storage = storage();
        let connection = connection();
        let first = QueryHistoryRecord::new(Some(connection.clone()), "SELECT 1", AT);
        let id = storage.record_history(&first).expect("first");
        assert_eq!(storage.clear_history(None, AT + 10).expect("clear"), 1);
        let after_delete = storage
            .history_entry(&id)
            .expect("read")
            .expect("there")
            .meta
            .version;

        let again = QueryHistoryRecord::new(Some(connection), "SELECT 1", AT);
        let revived = storage.record_history(&again).expect("again");
        assert_eq!(revived, id, "the same event keeps its identity");

        let row = storage.history_entry(&id).expect("read").expect("there");
        assert!(
            row.meta.version.get() > after_delete.get(),
            "the revision kept moving: {} then {}",
            after_delete.get(),
            row.meta.version.get()
        );
        assert!(!row.meta.is_deleted(), "the tombstone is lifted");
        assert_eq!(storage.history(10).expect("list").len(), 1);
    }

    #[test]
    fn a_search_finds_a_statement_by_a_word_in_it() {
        let storage = storage();
        storage
            .record_history(&QueryHistoryRecord::new(
                Some(connection()),
                "SELECT * FROM penerima_manfaat WHERE kabupaten = 'Bandung'",
                AT,
            ))
            .expect("record");
        storage
            .record_history(&QueryHistoryRecord::new(
                None,
                "SELECT * FROM pengguna",
                AT + 1,
            ))
            .expect("record");

        let found = storage.search_history("bandung", None, 10).expect("search");
        assert_eq!(found.len(), 1);
        assert!(found[0].sql_text.contains("penerima_manfaat"));
    }

    #[test]
    fn a_search_matches_a_fragment_inside_an_identifier() {
        // `unicode61` keeps `penerima_manfaat` as one token, so this is the test that keeps the
        // `*` in `fts_query`: without it, a search for `penerima` finds nothing at all.
        let storage = storage();
        storage
            .record_history(&QueryHistoryRecord::new(
                None,
                "SELECT * FROM penerima_manfaat",
                AT,
            ))
            .expect("record");

        assert_eq!(
            storage
                .search_history("penerima", None, 10)
                .expect("search")
                .len(),
            1,
            "a fragment of the token still finds the statement"
        );
    }

    #[test]
    fn a_search_wants_every_word_and_not_either() {
        let storage = storage();
        storage
            .record_history(&QueryHistoryRecord::new(None, "SELECT 1 FROM alpha", AT))
            .expect("record");
        storage
            .record_history(&QueryHistoryRecord::new(None, "SELECT 1 FROM beta", AT + 1))
            .expect("record");

        assert_eq!(
            storage
                .search_history("select alpha", None, 10)
                .expect("both")
                .len(),
            1
        );
        assert_eq!(
            storage
                .search_history("select", None, 10)
                .expect("either")
                .len(),
            2
        );
    }

    #[test]
    fn search_syntax_is_a_phrase_and_not_an_error() {
        // The trap. FTS5 reads `*`, `NEAR`, `AND` and a bare `"` as syntax, so a user typing
        // `select *` or `don't` would get a SQL error where they expected a result.
        let storage = storage();
        storage
            .record_history(&QueryHistoryRecord::new(
                None,
                "SELECT * FROM don't_panic WHERE a NEAR b",
                AT,
            ))
            .expect("record");

        for needle in [
            "select *",
            "don't",
            "\"quoted\"",
            "NEAR",
            "(",
            "=",
            "*",
            "a AND b",
        ] {
            storage
                .search_history(needle, None, 10)
                .unwrap_or_else(|error| {
                    panic!("{needle:?} is a search, not a syntax error: {error}")
                });
        }
        // And the two of those that do carry a word still find the row.
        assert_eq!(
            storage
                .search_history("select *", None, 10)
                .expect("search")
                .len(),
            1
        );
        assert_eq!(
            storage
                .search_history("don't", None, 10)
                .expect("search")
                .len(),
            1
        );
    }

    #[test]
    fn a_search_of_punctuation_only_returns_nothing_rather_than_everything() {
        let storage = storage();
        storage
            .record_history(&QueryHistoryRecord::new(None, "SELECT 1", AT))
            .expect("record");

        for needle in ["", "   ", "(*)", "===", "--"] {
            assert!(
                storage
                    .search_history(needle, None, 10)
                    .expect("search")
                    .is_empty(),
                "{needle:?} carries no word, so it is not a search"
            );
        }
    }

    #[test]
    fn a_search_leaves_out_what_was_cleared() {
        // A clear is an update, so the trigger re-indexes the same text and it stays in the index.
        // The `deleted_at IS NULL` filter is what keeps it out of the answer.
        let storage = storage();
        storage
            .record_history(&QueryHistoryRecord::new(
                None,
                "SELECT * FROM penerima_manfaat",
                AT,
            ))
            .expect("record");
        assert_eq!(
            storage
                .search_history("penerima", None, 10)
                .expect("search")
                .len(),
            1
        );

        storage.clear_history(None, AT + 10).expect("clear");
        assert!(storage
            .search_history("penerima", None, 10)
            .expect("search")
            .is_empty());
    }

    #[test]
    fn a_search_can_be_narrowed_to_one_connection() {
        let storage = storage();
        let mine = connection();
        let theirs = connection();
        storage
            .record_history(&QueryHistoryRecord::new(
                Some(mine.clone()),
                "SELECT * FROM shared",
                AT,
            ))
            .expect("mine");
        storage
            .record_history(&QueryHistoryRecord::new(
                Some(theirs.clone()),
                "SELECT * FROM shared",
                AT + 1,
            ))
            .expect("theirs");

        assert_eq!(
            storage
                .search_history("shared", None, 10)
                .expect("all")
                .len(),
            2
        );
        assert_eq!(
            storage
                .search_history("shared", Some(&mine), 10)
                .expect("mine")
                .len(),
            1
        );
    }

    #[test]
    fn a_search_still_finds_an_entry_that_was_recorded_twice() {
        // The update trigger's real path. Two saves of one event are one row, so the trigger fires
        // and the index has to end up holding the statement rather than losing it.
        let storage = storage();
        let mine = connection();
        for _ in 0..2 {
            storage
                .record_history(&QueryHistoryRecord::new(
                    Some(mine.clone()),
                    "SELECT * FROM penerima_manfaat",
                    AT,
                ))
                .expect("record");
        }

        assert_eq!(
            storage
                .search_history("penerima", None, 10)
                .expect("search")
                .len(),
            1
        );
    }

    #[test]
    fn the_search_migration_reaches_rows_that_were_already_there() {
        // The backfill, which no other test touches: a database that has been in use for months
        // would otherwise get a search that finds only what happened after the upgrade, and that
        // is worse than an empty one because it looks like it worked.
        //
        // The schema is built by running the migration SQL directly rather than through
        // `migrate::run`, because the point here is the SQL's own backfill statement and not the
        // bookkeeping around it. `migrate::run` applies everything pending, so it cannot stop at
        // version 2 to leave a row behind for version 3 to find.
        use crate::migrate::MIGRATIONS;

        let storage = Storage::in_memory().expect("in-memory database");
        storage
            .conn
            .execute_batch(MIGRATIONS[0].sql)
            .expect("schema up to 1");
        storage
            .conn
            .execute_batch(MIGRATIONS[1].sql)
            .expect("schema up to 2");
        storage
            .conn
            .execute(
                "INSERT INTO query_history \
                 (id, connection_id, sql_text, started_at, updated_at, version) \
                 VALUES ('01a0eab3-0000-7000-8000-000000000000', NULL, \
                         'SELECT * FROM penerima_manfaat', 1700000000000, 1700000000000, 1)",
                [],
            )
            .expect("a row from before the index existed");

        assert!(
            storage.search_history("penerima", None, 10).is_err(),
            "there is no index to search before the migration runs"
        );

        storage
            .conn
            .execute_batch(MIGRATIONS[2].sql)
            .expect("the search migration");

        let found = storage
            .search_history("penerima", None, 10)
            .expect("search");
        assert_eq!(
            found.len(),
            1,
            "the backfill reached the row that predates the index"
        );
        assert_eq!(found[0].sql_text, "SELECT * FROM penerima_manfaat");
    }

    #[test]
    fn a_history_entry_outlives_the_connection_that_wrote_it() {
        // The claim the whole feature rests on, and the one an in-memory test cannot make: close the
        // app, open it again, and the history is still there. Every other test in this file shares
        // one connection, so a write that was never committed would pass all of them.
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("qh.sqlite3");

        let mut first = Storage::open(&path).expect("open");
        first.migrate_at(AT).expect("migrate");
        first
            .record_history(&QueryHistoryRecord::new(
                None,
                "SELECT * FROM penerima_manfaat",
                AT,
            ))
            .expect("record");
        drop(first);

        let mut second = Storage::open(&path).expect("reopen");
        second
            .migrate_at(AT + 1)
            .expect("migrate again, which should find nothing to do");
        let entries = second.history(10).expect("read");

        assert_eq!(
            entries.len(),
            1,
            "the row outlived the connection that wrote it"
        );
        assert_eq!(entries[0].sql_text, "SELECT * FROM penerima_manfaat");
        // Findable too, not merely present: the FTS index has to survive the same round trip, and
        // an external-content index is the thing here most able to come back wrong.
        assert_eq!(
            second
                .search_history("penerima", None, 10)
                .expect("search")
                .len(),
            1
        );
    }

    #[test]
    fn favouriting_a_saved_query_is_a_revision_and_repeating_it_is_not() {
        let storage = storage();
        let record =
            SavedQueryRecord::new("Penerima 2026", "SELECT * FROM penerima_manfaat", None, AT);
        storage.save_query(&record).expect("save");
        assert!(
            !storage
                .saved_query(&record.meta.id)
                .expect("read")
                .unwrap()
                .favourite,
            "a new query is not a favourite"
        );

        assert!(storage
            .set_saved_query_favourite(&record.meta.id, true, AT + 1)
            .expect("favourite"));
        let after = storage.saved_query(&record.meta.id).expect("read").unwrap();
        assert!(after.favourite, "it reads back as a favourite");
        let revision = after.meta.version.get();
        assert!(
            revision > record.meta.version.get(),
            "the edit moved the revision"
        );

        // Asking again is not an edit. A second revision for the same state tells a reconciling
        // device that something changed when nothing did.
        assert!(storage
            .set_saved_query_favourite(&record.meta.id, true, AT + 2)
            .expect("favourite again"));
        assert_eq!(
            storage
                .saved_query(&record.meta.id)
                .expect("read")
                .unwrap()
                .meta
                .version
                .get(),
            revision,
            "repeating the same wish is not a new revision"
        );

        // Turning it off is an edit again.
        assert!(storage
            .set_saved_query_favourite(&record.meta.id, false, AT + 3)
            .expect("unfavourite"));
        assert!(
            !storage
                .saved_query(&record.meta.id)
                .expect("read")
                .unwrap()
                .favourite
        );

        // An id nobody has is not an error, it is "nothing to change", the same as `rename`.
        assert!(!storage
            .set_saved_query_favourite(&SyncId::now(), true, AT + 4)
            .expect("an id that is not there"));
    }

    #[test]
    fn renaming_a_saved_query_keeps_it_a_favourite() {
        // `rename_saved_query` reads the whole record and writes it back, so a field added later is
        // exactly the kind of thing it can quietly drop. This is the test that would notice.
        let storage = storage();
        let record = SavedQueryRecord::new("Lama", "SELECT 1", None, AT);
        storage.save_query(&record).expect("save");
        storage
            .set_saved_query_favourite(&record.meta.id, true, AT + 1)
            .expect("favourite");

        storage
            .rename_saved_query(&record.meta.id, "Baru", AT + 2)
            .expect("rename");
        let after = storage.saved_query(&record.meta.id).expect("read").unwrap();
        assert_eq!(after.name, "Baru");
        assert!(
            after.favourite,
            "the rename carried the flag rather than resetting it"
        );
    }

    #[test]
    fn a_vacuum_does_not_move_the_search_index_onto_the_wrong_rows() {
        // The one thing an external-content FTS5 table cannot enforce for itself. The index stores
        // the content table's rowid, `query_history` has a TEXT primary key, so its rowids are
        // implicit and SQLite is free to renumber them on `VACUUM`. If it did, the index would keep
        // answering, and would answer with the wrong statement attached to the right word.
        //
        // Nothing in this workspace runs `VACUUM`, so this is a latent hazard rather than a live bug.
        // It is here so whoever adds a maintenance command finds out from a test instead of from a
        // user noticing that search returns somebody else's statement.
        let directory = tempfile::tempdir().expect("a temporary directory");
        let mut storage = Storage::open(directory.path().join("qh.sqlite3")).expect("open");
        storage.migrate().expect("migrate");

        let statements = ["penerima_manfaat", "pengguna", "bantuan", "jadwal"];
        for (index, text) in statements.iter().enumerate() {
            storage
                .record_history(&QueryHistoryRecord::new(
                    None,
                    format!("SELECT * FROM {text}"),
                    AT + index as i64,
                ))
                .expect("record");
        }

        // A real hole, and a hard delete rather than `soft_delete_history`: that one is an UPDATE, so
        // it leaves every rowid in place and gives `VACUUM` nothing to renumber. This is the shape
        // that makes the hazard reachable. The delete trigger removes the index entry too, so the
        // index is consistent before the vacuum and the only thing under test is the renumbering.
        storage
            .conn
            .execute(
                "DELETE FROM query_history WHERE sql_text LIKE '%bantuan%'",
                [],
            )
            .expect("remove one row, leaving a gap");

        storage.conn.execute_batch("VACUUM").expect("vacuum");

        let found = storage
            .search_history("pengguna", None, 10)
            .expect("search");
        assert_eq!(found.len(), 1, "the word still finds exactly one statement");
        assert_eq!(
            found[0].sql_text, "SELECT * FROM pengguna",
            "and it is still the statement that contains the word"
        );
    }

    #[test]
    fn clearing_empties_the_listing_and_leaves_the_rows() {
        let storage = storage();
        let connection = connection();
        for offset in 0..3 {
            storage
                .record_history(&QueryHistoryRecord::new(
                    Some(connection.clone()),
                    format!("SELECT {offset}"),
                    AT + offset,
                ))
                .expect("record");
        }

        assert_eq!(storage.clear_history(None, AT + 10).expect("clear"), 3);
        assert!(storage.history(10).expect("list").is_empty());
        assert_eq!(
            storage.history_including_deleted().expect("all").len(),
            3,
            "a deletion is a change, not a removal"
        );
        assert_eq!(
            storage.clear_history(None, AT + 11).expect("clear again"),
            0,
            "clearing twice does not count the tombstones again"
        );
    }

    #[test]
    fn clearing_one_connection_leaves_the_others() {
        let storage = storage();
        let mine = connection();
        let theirs = connection();
        storage
            .record_history(&QueryHistoryRecord::new(Some(mine.clone()), "SELECT 1", AT))
            .expect("mine");
        storage
            .record_history(&QueryHistoryRecord::new(
                Some(theirs.clone()),
                "SELECT 2",
                AT,
            ))
            .expect("theirs");

        assert_eq!(
            storage.clear_history(Some(&mine), AT + 10).expect("clear"),
            1
        );
        let left = storage.history(10).expect("list");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].connection_id, Some(theirs));
    }

    #[test]
    fn clearing_one_connection_does_not_touch_the_entries_with_no_connection() {
        // A null connection is not "every connection", which is the mistake the `IS` in the
        // lookup would invite if it were copied here without thought.
        let storage = storage();
        let mine = connection();
        storage
            .record_history(&QueryHistoryRecord::new(Some(mine.clone()), "SELECT 1", AT))
            .expect("mine");
        storage
            .record_history(&QueryHistoryRecord::new(None, "SELECT 2", AT))
            .expect("no connection");

        assert_eq!(
            storage.clear_history(Some(&mine), AT + 10).expect("clear"),
            1
        );
        let left = storage.history(10).expect("list");
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].connection_id, None);
    }

    #[test]
    fn a_saved_query_is_listed_by_name_and_renamed_as_a_revision() {
        let storage = storage();
        let mut second = SavedQueryRecord::new("zeta", "SELECT 2", None, AT);
        second.meta.version = Version::from(1);
        let first = SavedQueryRecord::new("alpha", "SELECT 1", None, AT);
        storage.save_query(&second).expect("save");
        storage.save_query(&first).expect("save");

        let listed = storage.saved_queries().expect("list");
        assert_eq!(
            listed.iter().map(|q| q.name.as_str()).collect::<Vec<_>>(),
            vec!["alpha", "zeta"]
        );

        assert!(storage
            .rename_saved_query(&second.meta.id, "omega", AT + 5)
            .expect("rename"));
        let renamed = storage
            .saved_query(&second.meta.id)
            .expect("read")
            .expect("there");
        assert_eq!(renamed.name, "omega");
        assert_eq!(
            renamed.meta.version.get(),
            2,
            "the name moved with the revision"
        );
        assert_eq!(renamed.meta.updated_at, AT + 5);
        assert!(!listed[0].meta.is_deleted());
    }

    #[test]
    fn renaming_something_that_is_not_there_reports_it_rather_than_failing() {
        let storage = storage();
        let absent = SyncId::now();
        assert!(!storage
            .rename_saved_query(&absent, "x", AT)
            .expect("rename"));
        assert!(!storage
            .soft_delete_saved_query(&absent, AT)
            .expect("delete"));
        assert!(!storage
            .soft_delete_history(&absent, AT)
            .expect("delete history"));
    }

    #[test]
    fn deleting_a_saved_query_keeps_it_out_of_the_listing() {
        let storage = storage();
        let record = SavedQueryRecord::new("keep", "SELECT 1", None, AT);
        storage.save_query(&record).expect("save");
        assert!(storage
            .soft_delete_saved_query(&record.meta.id, AT + 1)
            .expect("delete"));
        assert!(storage.saved_queries().expect("list").is_empty());
        let tombstone = storage
            .saved_query(&record.meta.id)
            .expect("read")
            .expect("there");
        assert!(tombstone.meta.is_deleted());
    }
}
