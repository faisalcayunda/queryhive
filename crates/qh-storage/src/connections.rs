//! Saved connections and their groups: the rows the app actually edits.
//!
//! Every write goes through [`qh_sync::SyncMeta`] rather than through SQL alone, so the
//! revision and the tombstone follow the same rules everywhere: a change bumps the
//! version, a deletion is a change, and a row that is deleted stays in the table for a
//! future sync to find. The SQL only has to agree with it.
//!
//! A connection row holds **no secret**: `secret_ref` names a Keychain item, and
//! `qh-credentials` is what reads it. The one rule the database can enforce is that the
//! column is called a reference, and the doc comments say so; a password in this file
//! would be a password in every backup.

use qh_sync::{Resolution, SyncId, SyncMeta, Version};
use rusqlite::{params, OptionalExtension};

use crate::{Storage, StorageError};

/// Which server a connection speaks to.
///
/// A closed set, and one that grows by migration: the stored text is the same three
/// words the Python engine wrote, so a file from that era reads back without a mapping
/// table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionKind {
    Trino,
    Postgres,
    Mysql,
}

impl ConnectionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trino => "trino",
            Self::Postgres => "postgres",
            Self::Mysql => "mysql",
        }
    }

    pub fn parse(text: &str) -> Result<Self, StorageError> {
        match text {
            "trino" => Ok(Self::Trino),
            "postgres" => Ok(Self::Postgres),
            "mysql" => Ok(Self::Mysql),
            other => Err(StorageError::UnknownKind {
                text: other.to_owned(),
            }),
        }
    }
}

/// One saved connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionRecord {
    pub meta: SyncMeta,
    pub name: String,
    pub kind: ConnectionKind,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub user_name: Option<String>,
    pub database_name: Option<String>,
    /// `ssl_mode`, `show_system_schemas` and anything else the driver layer reads.
    /// Stored as opaque JSON so a new option does not need a migration.
    pub options_json: String,
    pub is_production: bool,
    pub is_read_only: bool,
    /// The Keychain account, not the password. Never both.
    pub secret_ref: Option<String>,
    pub group_id: Option<SyncId>,
    pub sort_order: i64,
}

/// One folder in the sidebar. Nesting is one level by convention, not by schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionGroup {
    pub meta: SyncMeta,
    pub name: String,
    pub parent_id: Option<SyncId>,
}

const CONNECTION_COLUMNS: &str = "id, name, kind, host, port, user_name, database_name, \
     options_json, is_production, is_read_only, secret_ref, group_id, sort_order, \
     updated_at, deleted_at, version";

impl Storage {
    /// Write a connection, replacing the row with that identity.
    ///
    /// An upsert rather than an insert-or-update pair: the caller has already decided the
    /// revision it is writing (that is what [`SyncMeta`] is for), and choosing between
    /// two statements here would mean deciding it twice.
    pub fn save_connection(&self, record: &ConnectionRecord) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO connection (id, name, kind, host, port, user_name, database_name, \
              options_json, is_production, is_read_only, secret_ref, group_id, sort_order, \
              updated_at, deleted_at, version) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16) \
             ON CONFLICT(id) DO UPDATE SET \
              name = excluded.name, kind = excluded.kind, host = excluded.host, \
              port = excluded.port, user_name = excluded.user_name, \
              database_name = excluded.database_name, options_json = excluded.options_json, \
              is_production = excluded.is_production, is_read_only = excluded.is_read_only, \
              secret_ref = excluded.secret_ref, group_id = excluded.group_id, \
              sort_order = excluded.sort_order, updated_at = excluded.updated_at, \
              deleted_at = excluded.deleted_at, version = excluded.version",
            params![
                record.meta.id.as_str(),
                record.name,
                record.kind.as_str(),
                record.host,
                record.port.map(i64::from),
                record.user_name,
                record.database_name,
                record.options_json,
                record.is_production,
                record.is_read_only,
                record.secret_ref,
                record.group_id.as_ref().map(SyncId::as_str),
                record.sort_order,
                record.meta.updated_at,
                record.meta.deleted_at,
                record.meta.version.get(),
            ],
        )?;
        Ok(())
    }

    /// One connection by identity, tombstone included.
    ///
    /// Deleted rows are returned because a deletion is a fact about the row, and the
    /// caller asking for an id it wrote is not asking "is this alive" — that question has
    /// its own method.
    pub fn connection(&self, id: &SyncId) -> Result<Option<ConnectionRecord>, StorageError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {CONNECTION_COLUMNS} FROM connection WHERE id = ?1"),
                params![id.as_str()],
                read_connection,
            )
            .optional()?)
    }

    /// Every living connection, in the order the sidebar shows them.
    pub fn connections(&self) -> Result<Vec<ConnectionRecord>, StorageError> {
        self.connections_where("WHERE deleted_at IS NULL ORDER BY sort_order ASC, name ASC")
    }

    /// Every connection the table holds, tombstones included.
    ///
    /// What a sync would send. Ordered by id so two runs agree, which is the smallest
    /// thing that makes a diff possible later.
    pub fn connections_including_deleted(&self) -> Result<Vec<ConnectionRecord>, StorageError> {
        self.connections_where("ORDER BY id ASC")
    }

    /// Delete a connection, keeping the row.
    ///
    /// The revision comes from [`SyncMeta::mark_deleted`] so that "how a deletion is
    /// recorded" has one home. Returns whether there was a row to delete; deleting a
    /// tombstone again is not an error, it is a no-op.
    pub fn soft_delete_connection(&self, id: &SyncId, at: i64) -> Result<bool, StorageError> {
        let Some(mut record) = self.connection(id)? else {
            return Ok(false);
        };
        if record.meta.is_deleted() {
            return Ok(true);
        }
        record.meta.mark_deleted(at);
        self.save_connection(&record)?;
        Ok(true)
    }

    /// Bring a deleted connection back, as a revision like any other.
    pub fn restore_connection(&self, id: &SyncId, at: i64) -> Result<bool, StorageError> {
        let Some(mut record) = self.connection(id)? else {
            return Ok(false);
        };
        if !record.meta.is_deleted() {
            return Ok(true);
        }
        record.meta.undelete(at);
        self.save_connection(&record)?;
        Ok(true)
    }

    /// Merge a connection that arrived from elsewhere.
    ///
    /// The decision is [`SyncMeta::resolve`]'s; this method's whole job is to keep the
    /// rule out of SQL. Returns what it decided, because "your edit was older than ours"
    /// is something a caller may want to report.
    pub fn merge_connection(
        &self,
        incoming: &ConnectionRecord,
    ) -> Result<Resolution, StorageError> {
        let resolution = match self.connection(&incoming.meta.id)? {
            Some(ours) => ours.meta.resolve(&incoming.meta),
            // Nothing stored is the same answer as an older revision: take theirs.
            None => Resolution::TakeTheirs,
        };
        if resolution == Resolution::TakeTheirs {
            self.save_connection(incoming)?;
        }
        Ok(resolution)
    }

    /// Write a group.
    pub fn save_group(&self, group: &ConnectionGroup) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO connection_group (id, name, parent_id, updated_at, deleted_at, version) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, \
              parent_id = excluded.parent_id, updated_at = excluded.updated_at, \
              deleted_at = excluded.deleted_at, version = excluded.version",
            params![
                group.meta.id.as_str(),
                group.name,
                group.parent_id.as_ref().map(SyncId::as_str),
                group.meta.updated_at,
                group.meta.deleted_at,
                group.meta.version.get(),
            ],
        )?;
        Ok(())
    }

    /// Every living group, in name order.
    pub fn groups(&self) -> Result<Vec<ConnectionGroup>, StorageError> {
        let mut statement = self.conn.prepare(
            "SELECT id, name, parent_id, updated_at, deleted_at, version FROM connection_group \
             WHERE deleted_at IS NULL ORDER BY name ASC",
        )?;
        let rows = statement.query_map([], read_group)?;
        let mut groups = Vec::new();
        for row in rows {
            groups.push(row?);
        }
        Ok(groups)
    }

    fn connections_where(&self, clause: &str) -> Result<Vec<ConnectionRecord>, StorageError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {CONNECTION_COLUMNS} FROM connection {clause}"
        ))?;
        let rows = statement.query_map([], read_connection)?;
        let mut connections = Vec::new();
        for row in rows {
            connections.push(row?);
        }
        Ok(connections)
    }
}

impl ConnectionRecord {
    /// A living connection at its first revision, with the defaults a new row gets.
    ///
    /// `now` is passed rather than read from the clock so that a caller building several
    /// rows stamps them all with one moment, and so that tests do not depend on the time
    /// of day.
    pub fn new(name: impl Into<String>, kind: ConnectionKind, at: i64) -> Self {
        Self {
            meta: SyncMeta::new(SyncId::now(), at),
            name: name.into(),
            kind,
            host: None,
            port: None,
            user_name: None,
            database_name: None,
            options_json: "{}".to_owned(),
            is_production: false,
            is_read_only: false,
            secret_ref: None,
            group_id: None,
            sort_order: 0,
        }
    }
}

/// One row of `connection` as it stands in the database.
fn read_connection(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConnectionRecord> {
    // Column order follows CONNECTION_COLUMNS, and a mismatch is a programming error
    // rather than user input, so a wrong type here is a panic in a test and an error in
    // the field. Reading by index keeps the two lists adjacent, which is what makes it
    // checkable by eye.
    let id: String = row.get(0)?;
    let kind: String = row.get(2)?;
    let port: Option<i64> = row.get(4)?;
    let group_id: Option<String> = row.get(11)?;
    let deleted_at: Option<i64> = row.get(14)?;
    let version: i64 = row.get(15)?;
    Ok(ConnectionRecord {
        meta: SyncMeta {
            id: to_sync_id(&id)?,
            updated_at: row.get(13)?,
            deleted_at,
            version: Version::from(version),
        },
        name: row.get(1)?,
        kind: ConnectionKind::parse(&kind).map_err(|error| to_sql_error(&error))?,
        host: row.get(3)?,
        port: port.and_then(|value| u16::try_from(value).ok()),
        user_name: row.get(5)?,
        database_name: row.get(6)?,
        options_json: row.get(7)?,
        // SQLite has no boolean type; the column is an INTEGER with 0/1, and anything
        // else is a value this engine did not write.
        is_production: row.get::<_, i64>(8)? != 0,
        is_read_only: row.get::<_, i64>(9)? != 0,
        secret_ref: row.get(10)?,
        group_id: match group_id {
            Some(text) => Some(to_sync_id(&text)?),
            None => None,
        },
        sort_order: row.get(12)?,
    })
}

fn read_group(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConnectionGroup> {
    let id: String = row.get(0)?;
    let parent_id: Option<String> = row.get(2)?;
    let deleted_at: Option<i64> = row.get(4)?;
    let version: i64 = row.get(5)?;
    Ok(ConnectionGroup {
        meta: SyncMeta {
            id: to_sync_id(&id)?,
            updated_at: row.get(3)?,
            deleted_at,
            version: Version::from(version),
        },
        name: row.get(1)?,
        parent_id: match parent_id {
            Some(text) => Some(to_sync_id(&text)?),
            None => None,
        },
    })
}

fn to_sync_id(text: &str) -> rusqlite::Result<SyncId> {
    SyncId::parse(text).map_err(|error| {
        to_sql_error(&StorageError::BadId {
            text: text.to_owned(),
            reason: error.to_string(),
        })
    })
}

/// A row that is there but says something impossible.
///
/// `FromSqlConversionFailure` is rusqlite's own variant for it, and it is what makes the
/// row-level error carry a column index — which is the difference between "one row is
/// corrupt" and "the database is corrupt".
fn to_sql_error(error: &StorageError) -> rusqlite::Error {
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

    fn storage() -> Storage {
        let mut storage = Storage::in_memory().expect("in-memory database");
        storage.migrate_at(1_700_000_000_000).expect("migrate");
        storage
    }

    #[test]
    fn deleting_a_group_leaves_its_connections_without_one() {
        // `group_id REFERENCES connection_group(id) ON DELETE SET NULL` is a table-level
        // declaration, and a declaration enforces nothing unless `foreign_keys` is on for
        // the connection — which is why the pragma is set on open and why this test
        // deletes the row for real instead of soft-deleting it.
        let storage = storage();
        let group = ConnectionGroup {
            meta: SyncMeta::new(SyncId::now(), 1_700_000_000_000),
            name: "Production".to_owned(),
            parent_id: None,
        };
        storage.save_group(&group).expect("save the group");

        let mut record = ConnectionRecord::new("Inside", ConnectionKind::Trino, 1_700_000_000_000);
        record.group_id = Some(group.meta.id.clone());
        storage
            .save_connection(&record)
            .expect("save the connection");
        assert_eq!(storage.groups().expect("groups"), vec![group.clone()]);

        storage
            .conn
            .execute(
                "DELETE FROM connection_group WHERE id = ?1",
                params![group.meta.id.as_str()],
            )
            .expect("a hard delete");

        let orphaned = storage
            .connection(&record.meta.id)
            .expect("read back")
            .expect("the connection survives its group");
        assert_eq!(orphaned.group_id, None);
        assert!(storage.groups().expect("groups").is_empty());
    }
}
