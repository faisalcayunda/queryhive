-- Full-text search over the query history.
--
-- An FTS5 table over `query_history` with `content=` pointing back at it, rather than a copy of
-- the statements. The text then lives in exactly one place: the index holds what a search needs,
-- the table holds the row, and the two cannot disagree about what a statement said. The triggers
-- keep them in step, and the backfill at the end covers rows written before this migration.
--
-- Why not `LIKE '%needle%'`. It answers the same question today and would for a long time -- the
-- history is thousands of rows, not millions -- but it cannot answer the next one. `LIKE` scans
-- every row and cannot rank, so "the statements most like this one" and "the rows that match, by
-- relevance" are both out of reach, and both are the reason a person searches a history at all.
-- The cost is paid once, here, instead of being paid again when the question changes.
--
-- `unicode61` rather than a SQL-aware tokenizer: it keeps `penerima_manfaat` as a single token
-- (underscore is a token character), which is why `Storage::search_history` also turns each term
-- into a prefix query. A search for `penerima` that found nothing would be the wrong answer.

CREATE VIRTUAL TABLE query_history_fts USING fts5(
  sql_text,
  content='query_history',
  content_rowid='rowid',
  tokenize='unicode61'
);

-- The index follows the table rather than the caller following both. `record_history` writes with
-- an upsert, which fires the insert trigger on a new row and the update trigger on an existing
-- one, so a re-recorded event is re-indexed rather than left behind.
--
-- A soft delete is an update of `deleted_at`, so it re-indexes the same text and leaves it
-- findable. That is deliberate and it is why the search query also filters on `deleted_at IS
-- NULL`: the index is not the place that decides what a user may see.

CREATE TRIGGER query_history_fts_insert AFTER INSERT ON query_history BEGIN
  INSERT INTO query_history_fts(rowid, sql_text) VALUES (new.rowid, new.sql_text);
END;

CREATE TRIGGER query_history_fts_delete AFTER DELETE ON query_history BEGIN
  INSERT INTO query_history_fts(query_history_fts, rowid, sql_text)
    VALUES ('delete', old.rowid, old.sql_text);
END;

CREATE TRIGGER query_history_fts_update AFTER UPDATE ON query_history BEGIN
  INSERT INTO query_history_fts(query_history_fts, rowid, sql_text)
    VALUES ('delete', old.rowid, old.sql_text);
  INSERT INTO query_history_fts(rowid, sql_text) VALUES (new.rowid, new.sql_text);
END;

-- Rows that were already there when this migration ran. A database that has been in use for
-- months would otherwise have a search that finds only what happened after the upgrade, which is
-- the one answer worse than an empty one.
INSERT INTO query_history_fts(rowid, sql_text) SELECT rowid, sql_text FROM query_history;
