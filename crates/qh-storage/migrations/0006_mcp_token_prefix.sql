-- A short, non-secret prefix of each token, so a token list is readable by a person.
--
-- The token string itself is never stored (see the module note in `mcp_token.rs`): this column
-- holds the first few characters after `qhmcp_`, which is enough to tell two tokens apart in a
-- list and not enough to be useful to an attacker, because the rest of the string is 256 random
-- bits. It exists because "which token is this?" is the question an operator asks before revoking
-- one, and a list of names with no way to match them to a string they are holding is a list nobody
-- can act on.
--
-- Rows that predate this column get an empty prefix rather than a guess: their tokens are
-- unrecoverable by design, so there is nothing to fill in, and an empty prefix is displayed as
-- such. `DEFAULT ''` is what lets the ALTER run on a table that already has rows.

ALTER TABLE mcp_token ADD COLUMN token_prefix TEXT NOT NULL DEFAULT '';
