-- A favourite is a saved query the user wants within reach.
--
-- A column on `saved_query` rather than a table of its own, because it is one bit about a row that
-- already exists. A favourites table would be a second identity for the same query, free to
-- disagree with the first about whether that row is still alive, and it would need its own
-- tombstone rules to say what happens when a favourite is deleted while favourited.
--
-- `folder_id` is the column the schema already reserved for grouping, and this is the flatter
-- question: keep this where I can see it, regardless of which folder it belongs to. Both can be
-- true at once and neither is derived from the other, so they stay separate columns.
--
-- `ALTER TABLE ... ADD COLUMN` with a default rather than a table rebuild. The column is additive,
-- every row that exists is by definition not a favourite, and a rebuild would rewrite a whole table
-- to answer one bit. SQLite refuses a non-constant default here, which is why this is a literal 0
-- rather than anything computed.

ALTER TABLE saved_query ADD COLUMN favourite INTEGER NOT NULL DEFAULT 0;
