-- The local database, as specified in section 3.5 of
-- docs/architecture/rust-engine-blueprint.md.
--
-- "Sync-ready" means every row has a stable identity, a revision and a way to say
-- deleted, so that adding a transport later needs no further migration. The rules
-- themselves live in qh-sync; this file is the shape they take in SQLite.
--
-- Timestamps are unix milliseconds in INTEGER, not SQLite's TEXT dates: the values are
-- compared as numbers (which of two edits came later) and a TEXT date's ordering depends
-- on the format string nobody wrote down.
--
-- Ids are TEXT UUIDs in lower case. Nothing here constrains them further: SQLite has no
-- UUID type, and a CHECK would only duplicate what qh_sync::SyncId already refuses to
-- construct.

-- Saved connections. NEVER a secret: only a reference to a Keychain item, which is what
-- qh-credentials reads under that reference.
CREATE TABLE connection (
  id            TEXT PRIMARY KEY,
  name          TEXT NOT NULL,
  kind          TEXT NOT NULL,              -- trino | postgres | mysql
  host          TEXT, port INTEGER, user_name TEXT, database_name TEXT,
  options_json  TEXT NOT NULL DEFAULT '{}', -- ssl_mode, show_system_schemas, dst.
  is_production INTEGER NOT NULL DEFAULT 0,
  is_read_only  INTEGER NOT NULL DEFAULT 0,
  secret_ref    TEXT,
  group_id      TEXT REFERENCES connection_group(id) ON DELETE SET NULL,
  sort_order    INTEGER NOT NULL DEFAULT 0,
  updated_at    INTEGER NOT NULL,
  deleted_at    INTEGER,
  version       INTEGER NOT NULL DEFAULT 1
);

-- `parent_id` carries no REFERENCES clause, deliberately, and it is not an oversight to
-- be tidied later: groups are soft-deleted like every other row, so the parent a child
-- points at still exists as a tombstone. A hard delete only happens in a purge, and a
-- purge has to walk the tree to decide what else goes anyway — an ON DELETE clause would
-- make it look like that walk is unnecessary.
CREATE TABLE connection_group (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, parent_id TEXT,
  updated_at INTEGER NOT NULL, deleted_at INTEGER, version INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE query_history (
  id TEXT PRIMARY KEY, connection_id TEXT, sql_text TEXT NOT NULL,
  started_at INTEGER NOT NULL, elapsed_ms INTEGER, row_count INTEGER,
  outcome TEXT,                             -- ok | error | cancelled
  error_text TEXT,
  updated_at INTEGER NOT NULL, deleted_at INTEGER, version INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE saved_query (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, sql_text TEXT NOT NULL,
  connection_id TEXT, folder_id TEXT,
  updated_at INTEGER NOT NULL, deleted_at INTEGER, version INTEGER NOT NULL DEFAULT 1
);

-- Session restore: which tabs were open, and what each was showing.
CREATE TABLE session_restore (
  id TEXT PRIMARY KEY, tab_json TEXT NOT NULL, active_tab_id TEXT,
  updated_at INTEGER NOT NULL, deleted_at INTEGER, version INTEGER NOT NULL DEFAULT 1
);

-- One row per migration that has run. `user_version` is the fast marker; this table is
-- the history, so "what version is this database?" can be answered with a name and a
-- date rather than a number.
CREATE TABLE schema_migration (
  version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL, name TEXT NOT NULL
);

-- A partial index on the tombstone: listing live rows is the common query, and a row
-- that is deleted stays in the table forever, so the index stays small as the table
-- grows.
CREATE INDEX idx_connection_live  ON connection(deleted_at) WHERE deleted_at IS NULL;
CREATE INDEX idx_history_started  ON query_history(started_at DESC);
-- Two identical queries on one connection at the same millisecond are the same event
-- saved twice, which the autosave path can do when a connection drops mid-query.
CREATE UNIQUE INDEX idx_history_dedupe ON query_history(connection_id, sql_text, started_at);
