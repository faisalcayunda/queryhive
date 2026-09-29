-- The MCP server's tokens: one row per client, and never the token itself.
--
-- `token_hash` holds the SHA-256 hex of the token string, and the token string is
-- shown once -- at `issue` time -- and never stored. A reader who gets this table
-- (a backup, a stolen laptop, a support dump) gets a list of names and hashes, not a
-- set of working credentials. A slow password hash would be better against a
-- determined offline attacker, but a 256-bit random token has no guessing space to
-- slow down, and the operators who have to read this table in a hurry are the ones
-- revoking a leak. The UNIQUE constraint is the lookup: a token is found by hashing
-- it and reading the one row that matches, never by scanning.
--
-- `scopes_json` and `connections_json` are JSON arrays of strings, kept as text for
-- the same reason `connection.options_json` is: a new scope is not a schema change.
-- Stored empty (`[]`) rather than NULL, because "no tools" and "no connections" are
-- choices an operator makes, and NULL would leave a reader guessing which of the two
-- an absent column meant.
--
-- Two ways to stop a token, and both columns exist because they are different facts:
-- `expires_at` is a time the operator set at issue, and it needs no write to take
-- effect. `revoked_at` is an act, and it is a separate timestamp rather than a
-- deletion so that `list` can still say which tokens were revoked and when -- a
-- tombstone would hide the very row an operator wants to audit. `deleted_at` follows
-- the table convention for a future sync purge and is otherwise unused here.
-- Timestamps are unix milliseconds, like every other time in this database.
--
-- No `REFERENCES` to `connection`: a token's allowlist may name a connection that is
-- later deleted, and the right answer then is "the token can reach nothing that still
-- exists", not a write failure on the connection delete. The allowlist is a filter
-- applied to what `connection` returns, not a set of rows that must stay live.

CREATE TABLE mcp_token (
  id               TEXT PRIMARY KEY,
  name             TEXT NOT NULL,
  token_hash       TEXT NOT NULL UNIQUE,
  scopes_json      TEXT NOT NULL,
  connections_json TEXT NOT NULL,
  expires_at       INTEGER,
  revoked_at       INTEGER,
  last_used_at     INTEGER,
  updated_at       INTEGER NOT NULL,
  deleted_at       INTEGER,
  version          INTEGER NOT NULL DEFAULT 1
);
