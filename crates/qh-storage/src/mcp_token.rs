//! MCP tokens: issuing one, hashing it, and deciding whether it still works.
//!
//! The schema lives in `migrations/0005_mcp_token.sql`, and the one rule it exists to
//! keep is that **the token string is never written down**. [`Storage::issue_mcp_token`]
//! returns it once, to be printed by the `issue` subcommand and handed to the client;
//! everything that reads this table afterwards reads a row whose `token_hash` can only
//! be matched by hashing a candidate token, never reversed into one.
//!
//! # Why a plain SHA-256, and not a password hash
//!
//! A password hash is slow on purpose because a password has little entropy and an
//! attacker with the hash can guess. A token from [`generate_token`] is 256 random bits
//! drawn from the OS CSPRNG, so there is nothing to guess and nothing a work factor
//! would buy; what it would cost is the one thing this table is for, which is answering
//! "is this token live?" fast enough to sit in front of every call. The hex digest and
//! the lookup are both O(1) on the token's length, and the `UNIQUE` index does the rest.
//!
//! # What "live" means
//!
//! A row is usable when it is not deleted, not revoked, and its `expires_at` -- when it
//! has one -- is still in the future. The three are separate columns because they are
//! three different answers to "why did this stop working", and a support question about
//! a refused token is answered by which one is set. [`McpTokenRecord::state`] is the one
//! place that reads them together.

use qh_sync::SyncId;
use rusqlite::{params, OptionalExtension};
use serde_json::Value as Json;
use sha2::{Digest, Sha256};

use crate::{Storage, StorageError};

/// The prefix on every token, so a leaked string can be recognised as a QueryHive token
/// in a log or a paste rather than looking like an arbitrary blob.
pub const TOKEN_PREFIX: &str = "qhmcp_";

/// Bytes of randomness in a token, before hexing: 256 bits.
pub const TOKEN_BYTES: usize = 32;

/// Why a token stopped working, or that it has not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenState {
    Live,
    Revoked,
    Expired,
    Deleted,
}

/// One `mcp_token` row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpTokenRecord {
    pub id: SyncId,
    pub name: String,
    /// The SHA-256 hex of the token. Never the token.
    pub token_hash: String,
    /// A JSON array of tool names, kept as text so a new tool needs no migration.
    pub scopes_json: String,
    /// A JSON array of connection ids the token may reach.
    pub connections_json: String,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
    pub version: i64,
}

impl McpTokenRecord {
    /// Whether this token may be used at `now`.
    pub fn state(&self, now: i64) -> TokenState {
        if self.deleted_at.is_some() {
            return TokenState::Deleted;
        }
        if self.revoked_at.is_some() {
            return TokenState::Revoked;
        }
        if self.expires_at.is_some_and(|expires| now >= expires) {
            return TokenState::Expired;
        }
        TokenState::Live
    }

    /// The tools this token may call, parsed.
    pub fn scopes(&self) -> Result<Vec<String>, StorageError> {
        string_array(&self.scopes_json, "scopes_json")
    }

    /// The connections this token may reach, parsed. Empty means none, not all.
    pub fn connections(&self) -> Result<Vec<String>, StorageError> {
        string_array(&self.connections_json, "connections_json")
    }
}

/// A token string from the OS CSPRNG, hexed, with the recognisable prefix.
///
/// `rand::rng()` is a per-thread generator seeded from the operating system, which is
/// the `getrandom` path on macOS; this is not `fastrand` and not a clock. A token that
/// could be guessed would make the whole `token_hash` design theatre.
pub fn generate_token() -> String {
    use rand::RngCore;

    let mut bytes = [0u8; TOKEN_BYTES];
    rand::rng().fill_bytes(&mut bytes);
    format!("{TOKEN_PREFIX}{}", hex::encode(bytes))
}

/// The hex SHA-256 of a token string, which is what the table stores and looks up.
pub fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Read a JSON text column that must be an array of strings.
fn string_array(text: &str, column: &str) -> Result<Vec<String>, StorageError> {
    let parsed: Json =
        serde_json::from_str(text).map_err(|error| StorageError::BadMcpTokenJson {
            column: column.to_owned(),
            reason: error.to_string(),
        })?;
    let items = parsed
        .as_array()
        .ok_or_else(|| StorageError::BadMcpTokenJson {
            column: column.to_owned(),
            reason: "the value is not an array".to_owned(),
        })?;
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| StorageError::BadMcpTokenJson {
                    column: column.to_owned(),
                    reason: "an entry is not a string".to_owned(),
                })
        })
        .collect()
}

const MCP_TOKEN_COLUMNS: &str = "id, name, token_hash, scopes_json, connections_json, \
     expires_at, revoked_at, last_used_at, updated_at, deleted_at, version";

impl Storage {
    /// Issue a token and write its hash, returning the plaintext once.
    ///
    /// The return value is the only time the token exists outside the caller's hands:
    /// it is not recoverable from the row, by design.
    pub fn issue_mcp_token(
        &self,
        name: &str,
        scopes: &[String],
        connections: &[String],
        expires_at: Option<i64>,
        at: i64,
    ) -> Result<(String, McpTokenRecord), StorageError> {
        let token = generate_token();
        let record = McpTokenRecord {
            id: SyncId::now(),
            name: name.to_owned(),
            token_hash: hash_token(&token),
            scopes_json: serde_json::to_string(scopes).expect("a list of strings serialises"),
            connections_json: serde_json::to_string(connections)
                .expect("a list of strings serialises"),
            expires_at,
            revoked_at: None,
            last_used_at: None,
            updated_at: at,
            deleted_at: None,
            version: 1,
        };
        self.conn.execute(
            "INSERT INTO mcp_token (id, name, token_hash, scopes_json, connections_json, \
              expires_at, revoked_at, last_used_at, updated_at, deleted_at, version) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                record.id.as_str(),
                record.name,
                record.token_hash,
                record.scopes_json,
                record.connections_json,
                record.expires_at,
                record.revoked_at,
                record.last_used_at,
                record.updated_at,
                record.deleted_at,
                record.version,
            ],
        )?;
        Ok((token, record))
    }

    /// The one row whose hash matches, tombstone included.
    ///
    /// Deleted rows are returned because the caller deciding "this token is not live"
    /// wants to tell "revoked" apart from "never existed", and a lookup that filtered
    /// them out could not.
    pub fn mcp_token_by_hash(&self, hash: &str) -> Result<Option<McpTokenRecord>, StorageError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {MCP_TOKEN_COLUMNS} FROM mcp_token WHERE token_hash = ?1"),
                params![hash],
                read_mcp_token,
            )
            .optional()?)
    }

    /// One token by its row identity, used by `revoke`.
    pub fn mcp_token(&self, id: &SyncId) -> Result<Option<McpTokenRecord>, StorageError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {MCP_TOKEN_COLUMNS} FROM mcp_token WHERE id = ?1"),
                params![id.as_str()],
                read_mcp_token,
            )
            .optional()?)
    }

    /// Every token that has not been deleted, newest first.
    ///
    /// Revoked and expired rows are included: they are exactly the rows an operator
    /// listing tokens wants to see, because "what did I disable, and when" is the
    /// question `list` is asked.
    pub fn mcp_tokens(&self) -> Result<Vec<McpTokenRecord>, StorageError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {MCP_TOKEN_COLUMNS} FROM mcp_token WHERE deleted_at IS NULL \
             ORDER BY updated_at DESC, name ASC"
        ))?;
        let rows = statement.query_map([], read_mcp_token)?;
        let mut tokens = Vec::new();
        for row in rows {
            tokens.push(row?);
        }
        Ok(tokens)
    }

    /// Revoke a token. Returns false when there is no such row.
    ///
    /// Revoking an already-revoked token is a no-op that still answers true: the caller
    /// asked for it to be off, and it is off. The original `revoked_at` is kept, because
    /// "when was it revoked" is the fact `list` reports.
    pub fn revoke_mcp_token(&self, id: &SyncId, at: i64) -> Result<bool, StorageError> {
        let Some(record) = self.mcp_token(id)? else {
            return Ok(false);
        };
        if record.revoked_at.is_some() {
            return Ok(true);
        }
        self.conn.execute(
            "UPDATE mcp_token SET revoked_at = ?1, updated_at = ?1, version = version + 1 \
             WHERE id = ?2",
            params![at, id.as_str()],
        )?;
        Ok(true)
    }

    /// Record that a token was used. Returns false when there is no such row.
    ///
    /// `last_used_at` is written on its own rather than folding into `updated_at` alone:
    /// a use is not an edit to the token, and bumping `version` on every call would make
    /// the revision meaningless as "somebody changed this".
    pub fn touch_mcp_token(&self, id: &SyncId, at: i64) -> Result<bool, StorageError> {
        let changed = self.conn.execute(
            "UPDATE mcp_token SET last_used_at = ?1 WHERE id = ?2",
            params![at, id.as_str()],
        )?;
        Ok(changed > 0)
    }
}

/// One row of `mcp_token` as it stands in the database.
fn read_mcp_token(row: &rusqlite::Row<'_>) -> rusqlite::Result<McpTokenRecord> {
    let id: String = row.get(0)?;
    let expires_at: Option<i64> = row.get(5)?;
    let revoked_at: Option<i64> = row.get(6)?;
    let last_used_at: Option<i64> = row.get(7)?;
    let deleted_at: Option<i64> = row.get(9)?;
    Ok(McpTokenRecord {
        id: SyncId::parse(&id).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    error.to_string(),
                )),
            )
        })?,
        name: row.get(1)?,
        token_hash: row.get(2)?,
        scopes_json: row.get(3)?,
        connections_json: row.get(4)?,
        expires_at,
        revoked_at,
        last_used_at,
        updated_at: row.get(8)?,
        deleted_at,
        version: row.get(10)?,
    })
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
    fn a_token_is_found_by_its_hash_and_never_stored_plain() {
        let storage = storage();
        let scopes = vec!["db_drivers".to_owned(), "preview".to_owned()];
        let connections = vec!["11111111-1111-7111-8111-111111111111".to_owned()];
        let (token, record) = storage
            .issue_mcp_token(
                "Claude Desktop",
                &scopes,
                &connections,
                None,
                1_700_000_000_000,
            )
            .expect("issue");

        assert!(token.starts_with(TOKEN_PREFIX));
        // The plaintext is not anywhere in the row.
        assert_ne!(record.token_hash, token);
        assert!(!record.token_hash.contains(&token));
        assert_eq!(record.token_hash, hash_token(&token));

        let found = storage
            .mcp_token_by_hash(&hash_token(&token))
            .expect("lookup")
            .expect("the row");
        assert_eq!(found, record);
        assert_eq!(found.scopes().expect("scopes"), scopes);
        assert_eq!(found.connections().expect("connections"), connections);

        // A different token does not match the row.
        assert!(storage
            .mcp_token_by_hash(&hash_token("qhmcp_not-the-token"))
            .expect("lookup")
            .is_none());
    }

    #[test]
    fn two_tokens_are_not_the_same_string() {
        // The reason generation is CSPRNG: two issues must not collide, and a test with
        // a fixed generator would hide exactly that.
        let storage = storage();
        let (first, _) = storage
            .issue_mcp_token("a", &[], &[], None, 1)
            .expect("issue a");
        let (second, _) = storage
            .issue_mcp_token("b", &[], &[], None, 2)
            .expect("issue b");
        assert_ne!(first, second);
        assert_eq!(first.len(), TOKEN_PREFIX.len() + TOKEN_BYTES * 2);
    }

    #[test]
    fn an_expired_or_revoked_token_says_which_it_is() {
        let storage = storage();
        let (_token, mut record) = storage
            .issue_mcp_token("old", &[], &[], Some(2_000), 1_000)
            .expect("issue");
        // Before the deadline it is live, at it and after it is expired.
        assert_eq!(record.state(1_999), TokenState::Live);
        assert_eq!(record.state(2_000), TokenState::Expired);

        assert!(storage.revoke_mcp_token(&record.id, 3_000).expect("revoke"));
        record = storage
            .mcp_token(&record.id)
            .expect("lookup")
            .expect("the row");
        assert_eq!(record.revoked_at, Some(3_000));
        assert_eq!(record.state(1_999), TokenState::Revoked);

        // Revoking twice keeps the first moment rather than moving it.
        assert!(storage
            .revoke_mcp_token(&record.id, 9_000)
            .expect("revoke again"));
        let again = storage
            .mcp_token(&record.id)
            .expect("lookup")
            .expect("the row");
        assert_eq!(again.revoked_at, Some(3_000));
    }

    #[test]
    fn revoking_an_unknown_id_is_false_not_an_error() {
        let storage = storage();
        let id = SyncId::parse("9db3c0cc-7062-49bb-9d47-62f765275a9b").expect("an id");
        assert!(!storage.revoke_mcp_token(&id, 1).expect("revoke"));
    }

    #[test]
    fn a_use_is_recorded_without_changing_the_revision() {
        let storage = storage();
        let (_token, record) = storage
            .issue_mcp_token("used", &[], &[], None, 1_000)
            .expect("issue");
        assert_eq!(record.last_used_at, None);
        assert!(storage.touch_mcp_token(&record.id, 5_000).expect("touch"));
        let touched = storage
            .mcp_token(&record.id)
            .expect("lookup")
            .expect("the row");
        assert_eq!(touched.last_used_at, Some(5_000));
        assert_eq!(touched.version, record.version, "a use is not an edit");
        assert!(!storage
            .touch_mcp_token(&SyncId::now(), 5_000)
            .expect("touch"));
    }

    #[test]
    fn a_malformed_scope_column_is_refused_by_name() {
        let storage = storage();
        let (_token, record) = storage
            .issue_mcp_token("bad", &[], &[], None, 1)
            .expect("issue");
        storage
            .conn
            .execute(
                "UPDATE mcp_token SET scopes_json = ?1 WHERE id = ?2",
                params!["not json", record.id.as_str()],
            )
            .expect("write the bad value");
        let found = storage
            .mcp_token(&record.id)
            .expect("lookup")
            .expect("the row");
        match found.scopes() {
            Err(StorageError::BadMcpTokenJson { column, .. }) => {
                assert_eq!(column, "scopes_json")
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }
}
