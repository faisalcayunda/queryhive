-- The record of a finished import from the previous store.
--
-- One row per source file. The row is written **only after the imported connections have
-- been read back and found to match what was written** (see `src/import.rs`), which is
-- what makes "did the import finish?" a question with an answer rather than a guess. An
-- import that failed verification leaves no row, so running it again is the retry.
--
-- `source` is the path, because that is what identifies an import: a user with two
-- accounts on one machine has two Application Support directories, and a database knows
-- which one it already took.
CREATE TABLE legacy_import (
  source      TEXT PRIMARY KEY,
  imported_at INTEGER NOT NULL,
  connections INTEGER NOT NULL,
  -- 1 once verification passed. Kept as a column rather than implied by the row's
  -- existence so that a future import that wants to record a partial outcome has somewhere
  -- to say so without a migration.
  verified    INTEGER NOT NULL
);
