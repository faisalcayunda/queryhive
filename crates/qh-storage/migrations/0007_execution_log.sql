-- The execution log: one row per allow/refuse decision the engine made about a
-- caller's statement, chained so the file can be checked rather than trusted.
--
-- The statement itself is NEVER here. `statement_hash` is the hex SHA-256 of the
-- statement text, which is enough to answer "was this exact statement judged?" without
-- turning the log into a copy of everything anyone ran. `reason` is the classifier's own
-- fixed sentence, not the statement, so no SQL text and no credential can leak through
-- this table. Hosts, users, passwords and the Keychain reference live nowhere here.
--
-- The chain is what makes deletion or editing visible. `prev_hash` is the `chain_hash`
-- of the row before this one (`genesis` for the first), and `chain_hash` is the SHA-256 of
-- this row's own fields plus `prev_hash`. Recomputing the chain in `seq` order and
-- comparing it with what is stored proves the rows were not inserted, removed or changed
-- after the fact. That is **tamper-evident, not tamper-proof**: whoever holds the file can
-- rewrite the whole chain, and this table is honest about that rather than implying a
-- signature it does not have.
--
-- No `updated_at`/`deleted_at`/`version`: this is an append-only audit record, not a row
-- of the user's workspace. It is never edited, never soft-deleted and never synced, so the
-- sync columns the other tables carry would be three columns that are always the same
-- values. `seq` is the insertion order the chain is verified in.

CREATE TABLE execution_log (
  seq             INTEGER PRIMARY KEY AUTOINCREMENT,
  id              TEXT NOT NULL,
  at              INTEGER NOT NULL,
  safe_mode       TEXT NOT NULL,     -- full | no_ddl | confirm | read_only (the effective one)
  decision        TEXT NOT NULL,     -- allowed | confirmed | refused | needs_confirmation
  statement_kind  TEXT NOT NULL,     -- read_only | dml | ddl | unknown
  statement_index INTEGER NOT NULL,  -- 1-based position of the statement in the script
  statement_hash  TEXT NOT NULL,     -- hex SHA-256 of the statement text; never the text
  reason          TEXT,              -- the classifier's own sentence, or NULL when allowed
  prev_hash       TEXT NOT NULL,     -- chain_hash of the previous row, or 'genesis'
  chain_hash      TEXT NOT NULL      -- hex SHA-256 over this row and prev_hash
);

-- Listing the most recent decisions is the reader's query, and it is always newest-first.
CREATE INDEX idx_execution_log_at ON execution_log(at DESC);
