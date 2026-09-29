-- The identity this application signs in with, and the profiles that belong to it.
--
-- Local only, and deliberately small: no transport, no server, no session. Every row here
-- follows the sync rule `0001_init.sql` states -- a stable identity, a revision, and a way
-- to say deleted -- so that adding one later needs no migration, and nothing in this file
-- assumes a network exists today.
--
-- Nothing here reaches a database connection. `connection` keeps its own `secret_ref` and
-- its own `SSH_*` settings, and no row of this table is read on the path that opens a
-- connection. This is the identity the *application* runs under, which is what a profile
-- stored for a user needs in order to have an owner at all.
--
-- `app_account` holds exactly one row, and the schema is what says so: the CHECK pins the
-- id to a single value, so a second insert fails loudly rather than creating a second
-- account that some profiles belong to and others do not. The id is still a UUID, so it is
-- a `SyncId` like every other identity in this database -- a singleton needs a stable
-- identity more than anything else here does.
--
-- `provider` and `subject` are what an identity provider told us; `signed_out_at` exists
-- because signing out is a fact and not the absence of one. A user who signs out has not
-- told us they were never here, and a reader asking "who used this" wants the answer that
-- survives the sign-out. Both spellings of "never signed in" (an empty table, and a row
-- whose `signed_in_at` is NULL) are distinguishable and both mean something.
--
-- `profile.owner_id` carries no REFERENCES clause, on purpose and for the same reason
-- `connection_group.parent_id` does not: an account is soft-deleted like every other row,
-- so the owner a profile points at still exists as a tombstone, and a hard delete only
-- happens in a purge that has to decide what else goes with it.
--
-- `profile.kind` is `saved_query | connection | preference`, kept as text rather than a
-- CHECK because that set grows and a CHECK would turn a new kind into a schema change.
-- There is no `secret_ref` column: a kind that has a secret carries it in `payload_json`
-- under the account spelling `qh_credentials::PROFILE_ACCOUNT_PREFIX` documents, and an
-- unused reference column sitting here would invite a secret to be written into the row
-- instead -- which is the one thing `connection`'s table comment forbids outright.
--
-- `payload_json` is the per-kind body and is opaque to SQL for the same reason
-- `connection.options_json` is: a new field in a kind is not a migration.
--
-- Timestamps are unix milliseconds, like every other time in this database.

CREATE TABLE app_account (
  id           TEXT PRIMARY KEY CHECK (id = '00000000-0000-7000-8000-000000000000'),
  provider     TEXT,                          -- google | apple | github | microsoft
  subject      TEXT,                          -- the provider's own user id, as text
  email        TEXT,
  display_name TEXT,
  signed_in_at INTEGER,
  signed_out_at INTEGER,
  updated_at   INTEGER NOT NULL,
  deleted_at   INTEGER,
  version      INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE profile (
  id           TEXT PRIMARY KEY,
  owner_id     TEXT NOT NULL,
  kind         TEXT NOT NULL,                 -- saved_query | connection | preference
  name         TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  updated_at   INTEGER NOT NULL,
  deleted_at   INTEGER,
  version      INTEGER NOT NULL DEFAULT 1
);

-- The query the app actually runs: one owner's living profiles of one kind, in the order it
-- lists them. Partial, like `idx_connection_live`, for the same reason: tombstones accumulate
-- and a listing never asks for them.
CREATE INDEX profile_owner_kind ON profile (owner_id, kind, name) WHERE deleted_at IS NULL;
