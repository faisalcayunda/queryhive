//! The execution log: every allow/refuse decision, chained so the file can be checked.
//!
//! The schema lives in `migrations/0007_execution_log.sql`, and the rules it exists to
//! keep are three. **The statement is never written down** — only its SHA-256, so the log
//! answers "was this exact statement judged?" without becoming a copy of everything anyone
//! ran. **No credential, host or user is written down either**: the classifier's own fixed
//! sentence is the only free text. And **each row is chained to the one before it**, so a
//! row that was inserted, removed or edited afterwards stops the chain from verifying.
//!
//! # Tamper-evident, not tamper-proof
//!
//! A chain is not a signature. Whoever can write this file can rewrite every row and
//! recompute every hash, and the result verifies. What the chain does give is that a
//! reader who has one earlier `chain_hash` — from a backup, a paste, an external system —
//! can tell whether the rows since then are untouched. That is the honest claim, and it is
//! stated here rather than sold as more.
//!
//! # Why the hash is over canonical JSON
//!
//! The chain hash covers a fixed list of fields. A `\n`-joined string would be
//! ambiguous the moment a field contains a newline, so the fields go through
//! `serde_json::to_vec` as one array: arrays keep their order, the value types are
//! explicit, and nothing in the payload can be read as a separator.

use qh_sync::SyncId;
use rusqlite::{params, OptionalExtension};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{Storage, StorageError};

/// The `prev_hash` of the first row. Not a hash of anything, and spelled so nobody has to
/// guess what an all-zero digest was supposed to mean.
pub const GENESIS: &str = "genesis";

/// One row of `execution_log`, as it is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionRecord {
    /// Insertion order, which is the order the chain verifies in.
    pub seq: i64,
    pub id: SyncId,
    /// Unix milliseconds.
    pub at: i64,
    /// The effective Safe Mode, which may have been raised by a floor.
    pub safe_mode: String,
    /// `allowed` | `confirmed` | `refused` | `needs_confirmation`.
    pub decision: String,
    /// `read_only` | `dml` | `ddl` | `unknown`.
    pub statement_kind: String,
    /// 1-based position of the statement in the script.
    pub statement_index: i64,
    /// The hex SHA-256 of the statement text. Never the text.
    pub statement_hash: String,
    /// The classifier's own sentence, or `None` when the decision was an allow.
    pub reason: Option<String>,
    /// The previous row's `chain_hash`, or [`GENESIS`].
    pub prev_hash: String,
    /// The hex SHA-256 over this row's fields and `prev_hash`.
    pub chain_hash: String,
}

/// The fields a caller supplies; the identity and the chain are computed here.
#[derive(Debug, Clone, Copy)]
pub struct NewExecution<'a> {
    pub safe_mode: &'a str,
    pub decision: &'a str,
    pub statement_kind: &'a str,
    pub statement_index: i64,
    /// The statement text. Only its hash is stored.
    pub statement: &'a str,
    pub reason: Option<&'a str>,
    /// Unix milliseconds, taken by the caller so a batch shares one reading of the clock.
    pub at: i64,
}

/// The hex SHA-256 of a statement's text.
pub fn hash_statement(sql: &str) -> String {
    sha256_hex(sql.as_bytes())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// The fields a row's chain hash covers, in the order it covers them.
///
/// One struct rather than nine arguments, because the order is the whole contract: a
/// hash over the same fields in a different order is a different hash, and a caller
/// passing them positionally could reorder two of the same type without the compiler
/// noticing.
struct ChainFields<'a> {
    id: &'a str,
    at: i64,
    safe_mode: &'a str,
    decision: &'a str,
    statement_kind: &'a str,
    statement_index: i64,
    statement_hash: &'a str,
    reason: Option<&'a str>,
    prev_hash: &'a str,
}

/// The chain hash of one row: its own fields, plus the hash it links back to.
fn chain_hash(fields: ChainFields<'_>) -> String {
    let ChainFields {
        id,
        at,
        safe_mode,
        decision,
        statement_kind,
        statement_index,
        statement_hash,
        reason,
        prev_hash,
    } = fields;
    let payload = json!([
        id,
        at,
        safe_mode,
        decision,
        statement_kind,
        statement_index,
        statement_hash,
        reason,
        prev_hash,
    ]);
    // A tuple of strings, integers and `null` always serialises; an error here would be a
    // defect in this file rather than something a caller could cause, and swallowing it
    // would produce a hash over nothing.
    let bytes = serde_json::to_vec(&payload).expect("an array of scalars serialises");
    sha256_hex(&bytes)
}

const COLUMNS: &str = "seq, id, at, safe_mode, decision, statement_kind, statement_index, \
     statement_hash, reason, prev_hash, chain_hash";

fn read_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExecutionRecord> {
    Ok(ExecutionRecord {
        seq: row.get(0)?,
        id: crate::history::to_sync_id(row.get::<_, String>(1)?.as_str())?,
        at: row.get(2)?,
        safe_mode: row.get(3)?,
        decision: row.get(4)?,
        statement_kind: row.get(5)?,
        statement_index: row.get(6)?,
        statement_hash: row.get(7)?,
        reason: row.get(8)?,
        prev_hash: row.get(9)?,
        chain_hash: row.get(10)?,
    })
}

impl Storage {
    /// Append one decision, chained to the row before it. Returns the stored row.
    ///
    /// Reading the previous hash and writing the next row happen inside one
    /// `BEGIN IMMEDIATE` transaction, not as two statements. Two engine processes — the app
    /// spawns one per run, and a user can have two tabs in flight — would otherwise both
    /// read the same tail and both chain to it, forking the log into two rows with one
    /// predecessor. The immediate lock makes the second writer wait for the first, so the
    /// chain stays a chain however many processes write to it.
    pub fn append_execution(
        &mut self,
        entry: NewExecution<'_>,
    ) -> Result<ExecutionRecord, StorageError> {
        let transaction = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let prev_hash = transaction
            .query_row(
                "SELECT chain_hash FROM execution_log ORDER BY seq DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or_else(|| GENESIS.to_owned());
        let id = SyncId::now();
        let statement_hash = hash_statement(entry.statement);
        let chain_hash = chain_hash(ChainFields {
            id: id.as_str(),
            at: entry.at,
            safe_mode: entry.safe_mode,
            decision: entry.decision,
            statement_kind: entry.statement_kind,
            statement_index: entry.statement_index,
            statement_hash: &statement_hash,
            reason: entry.reason,
            prev_hash: &prev_hash,
        });
        transaction.execute(
            "INSERT INTO execution_log (id, at, safe_mode, decision, statement_kind, \
              statement_index, statement_hash, reason, prev_hash, chain_hash) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id.as_str(),
                entry.at,
                entry.safe_mode,
                entry.decision,
                entry.statement_kind,
                entry.statement_index,
                statement_hash,
                entry.reason,
                prev_hash,
                chain_hash,
            ],
        )?;
        let seq = transaction.last_insert_rowid();
        transaction.commit()?;
        Ok(ExecutionRecord {
            seq,
            id,
            at: entry.at,
            safe_mode: entry.safe_mode.to_owned(),
            decision: entry.decision.to_owned(),
            statement_kind: entry.statement_kind.to_owned(),
            statement_index: entry.statement_index,
            statement_hash,
            reason: entry.reason.map(str::to_owned),
            prev_hash,
            chain_hash,
        })
    }

    /// The most recent decisions, newest first, capped at `limit`.
    pub fn execution_log(&self, limit: usize) -> Result<Vec<ExecutionRecord>, StorageError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM execution_log ORDER BY seq DESC LIMIT ?1"
        ))?;
        let rows = statement.query_map(params![limit as i64], read_record)?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row?);
        }
        Ok(records)
    }

    /// Recompute the chain from the first row and return how many rows it verified.
    ///
    /// This is the whole point of the table: a caller that got here without an error is
    /// holding a log where no row after the first has been inserted, removed or edited
    /// since the chain was written — up to the first `prev_hash` the caller has from
    /// somewhere else, which is the limit stated in the module note.
    pub fn verify_execution_log(&self) -> Result<usize, StorageError> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM execution_log ORDER BY seq ASC"
        ))?;
        let rows = statement.query_map([], read_record)?;
        let mut expected_prev = GENESIS.to_owned();
        let mut verified = 0usize;
        for row in rows {
            let record = row?;
            if record.prev_hash != expected_prev {
                return Err(StorageError::BrokenExecutionChain {
                    seq: record.seq,
                    expected: expected_prev,
                    found: record.prev_hash,
                });
            }
            let expected = chain_hash(ChainFields {
                id: record.id.as_str(),
                at: record.at,
                safe_mode: &record.safe_mode,
                decision: &record.decision,
                statement_kind: &record.statement_kind,
                statement_index: record.statement_index,
                statement_hash: &record.statement_hash,
                reason: record.reason.as_deref(),
                prev_hash: &record.prev_hash,
            });
            if expected != record.chain_hash {
                return Err(StorageError::BrokenExecutionChain {
                    seq: record.seq,
                    expected,
                    found: record.chain_hash,
                });
            }
            expected_prev = record.chain_hash;
            verified += 1;
        }
        Ok(verified)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage() -> Storage {
        let mut storage = Storage::in_memory().expect("in-memory database");
        storage.migrate_at(1_000).expect("migrations");
        storage
    }

    fn entry<'a>(decision: &'a str, statement: &'a str) -> NewExecution<'a> {
        NewExecution {
            safe_mode: "read_only",
            decision,
            statement_kind: "ddl",
            statement_index: 1,
            statement,
            reason: Some("DDL is not allowed on a read-only connection"),
            at: 2_000,
        }
    }

    #[test]
    fn the_statement_is_hashed_and_never_stored() {
        let mut storage = storage();
        let record = storage
            .append_execution(entry("refused", "DROP TABLE people"))
            .unwrap();
        assert_eq!(record.statement_hash, hash_statement("DROP TABLE people"));
        assert_eq!(record.statement_hash.len(), 64, "a hex SHA-256");
        assert_eq!(record.statement_kind, "ddl");
        // The schema is the machine-checkable half of "never the text": there is no column
        // whose name is `statement`, and there is one for the hash.
        let mut statement = storage
            .conn
            .prepare("SELECT name FROM pragma_table_info('execution_log')")
            .unwrap();
        let columns: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(
            !columns.iter().any(|name| name == "statement"),
            "no column holds the statement text: {columns:?}"
        );
        assert!(
            columns.iter().any(|name| name == "statement_hash"),
            "{columns:?}"
        );
        // And the digest is the only thing a reader can get back.
        let stored: String = storage
            .conn
            .query_row("SELECT statement_hash FROM execution_log", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(stored, hash_statement("DROP TABLE people"));
        assert!(!stored.contains("DROP"), "a hash is not the text: {stored}");
    }

    #[test]
    fn a_row_links_back_to_the_one_before_it() {
        let mut storage = storage();
        let first = storage
            .append_execution(entry("allowed", "SELECT 1"))
            .unwrap();
        let second = storage
            .append_execution(entry("refused", "DROP TABLE people"))
            .unwrap();
        assert_eq!(first.prev_hash, GENESIS);
        assert_eq!(second.prev_hash, first.chain_hash);
        assert_ne!(first.chain_hash, second.chain_hash);
        assert_eq!(storage.verify_execution_log().unwrap(), 2);
    }

    #[test]
    fn the_chain_notices_an_edited_row() {
        let mut storage = storage();
        storage
            .append_execution(entry("allowed", "SELECT 1"))
            .unwrap();
        storage
            .append_execution(entry("refused", "DROP TABLE people"))
            .unwrap();
        // Editing a stored field without recomputing the chain is exactly the tamper the
        // chain exists to make visible.
        storage
            .conn
            .execute(
                "UPDATE execution_log SET decision = 'allowed' WHERE seq = 2",
                [],
            )
            .unwrap();
        let broken = storage.verify_execution_log().unwrap_err();
        assert!(
            matches!(broken, StorageError::BrokenExecutionChain { seq: 2, .. }),
            "{broken:?}"
        );
    }

    #[test]
    fn the_chain_notices_a_deleted_row() {
        let mut storage = storage();
        storage
            .append_execution(entry("allowed", "SELECT 1"))
            .unwrap();
        storage
            .append_execution(entry("refused", "DROP TABLE people"))
            .unwrap();
        storage
            .conn
            .execute("DELETE FROM execution_log WHERE seq = 1", [])
            .unwrap();
        let broken = storage.verify_execution_log().unwrap_err();
        assert!(
            matches!(broken, StorageError::BrokenExecutionChain { seq: 2, .. }),
            "the second row still points at the first: {broken:?}"
        );
    }

    #[test]
    fn an_empty_log_verifies_as_empty() {
        assert_eq!(storage().verify_execution_log().unwrap(), 0);
    }

    #[test]
    fn the_log_reads_back_newest_first() {
        let mut storage = storage();
        let first = storage
            .append_execution(entry("allowed", "SELECT 1"))
            .unwrap();
        let second = storage
            .append_execution(entry("refused", "DROP TABLE people"))
            .unwrap();
        assert_eq!(storage.execution_log(10).unwrap(), vec![second, first]);
        // The cap keeps the newest, not the oldest.
        assert_eq!(storage.execution_log(1).unwrap().len(), 1);
    }
}
