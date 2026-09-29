//! The process's execution log sink: where a Safe Mode decision is written down.
//!
//! The table, the chain and the reasons live in `qh-storage` (`migrations/0007_execution_log.sql`
//! and `qh_storage::ExecutionRecord`). This module is the engine's half: a single place the
//! binaries hand a storage handle to, and a single function [`record`] every module's guard
//! calls. Keeping it process-scoped rather than per-command is what lets `guard` stay a
//! two-argument function — and `apply.rs`, `import.rs` and `mcp.rs`, which all call the same
//! guard, are not touched by the log's arrival.
//!
//! # Why a sink and not a parameter
//!
//! A decision is made in the middle of a command that has no reason to know where the local
//! database is. Threading a storage handle through every command, and every helper those
//! commands call, would put the log's dependency in signatures whose job is something else.
//! The sink is installed once, at the one place a process decides it is the engine rather
//! than a library: the `queryhive-engine` binary's `main`.
//!
//! **A library caller and a test install nothing, and nothing is written.** `run` is called
//! directly by the golden harness and the integration tests, so `cargo test` never opens the
//! user's database — the deliberate alternative to a default-on log that a test run would
//! fill with its own fixtures.
//!
//! # The write is deliberately synchronous
//!
//! `guard` runs on the async path, and the crate's storage rule is that a `rusqlite`
//! connection is never used from a tokio worker. This is the conscious exception: the write
//! is one indexed `INSERT` into a local file, and the alternatives are worse for an audit
//! log. Spawning it would let a decision reach the caller before it reached the log, and a
//! chain that races is a chain that verifies against an order nobody chose. Correctness of
//! the chain wins over a few microseconds on the runtime thread.

use std::sync::{Mutex, OnceLock};

use qh_sql::{SafeMode, StatementKind};
use qh_storage::{NewExecution, Storage, StorageError};

use crate::env::Settings;
use crate::CliError;

/// What the log calls a decision, in the vocabulary the `decision` column stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogDecision {
    /// Ran without asking.
    Allowed,
    /// A `confirm`-mode write the caller had already confirmed for this run.
    Confirmed,
    /// Did not run, and no confirmation would change that.
    Refused,
    /// Did not run because the caller had not confirmed it, and could have.
    NeedsConfirmation,
}

impl LogDecision {
    pub const fn as_str(self) -> &'static str {
        match self {
            LogDecision::Allowed => "allowed",
            LogDecision::Confirmed => "confirmed",
            LogDecision::Refused => "refused",
            LogDecision::NeedsConfirmation => "needs_confirmation",
        }
    }
}

/// One decision, on its way to the log.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DecisionEntry<'a> {
    /// The effective mode, after any floor.
    pub safe_mode: SafeMode,
    /// 1-based position of the statement in the script.
    pub index: usize,
    pub kind: StatementKind,
    /// The statement text. Only its hash is stored.
    pub statement: &'a str,
    pub decision: LogDecision,
    pub reason: Option<&'a str>,
}

/// The installed sink, or nothing. `OnceLock` so a process installs once and reads many
/// times without a lock on the read path beyond the `Mutex` that keeps `rusqlite` honest.
static SINK: OnceLock<Mutex<Option<Storage>>> = OnceLock::new();

fn sink() -> &'static Mutex<Option<Storage>> {
    SINK.get_or_init(|| Mutex::new(None))
}

/// Install a storage handle as this process's decision log.
///
/// Installing again replaces the handle; the binary calls this once. A test calls it with an
/// in-memory database, which is how the write path is proven without touching a real one.
pub fn install(storage: Storage) {
    *sink().lock().expect("the log sink mutex is not poisoned") = Some(storage);
}

/// Forget the installed sink. For tests, and for a caller that wants the engine to stop
/// writing decisions.
pub fn uninstall() {
    *sink().lock().expect("the log sink mutex is not poisoned") = None;
}

/// Open the local database and install it as the decision log.
///
/// The path rule is `crate::local::open_storage`'s — `DB_PATH` when the caller named one,
/// the Application Support file otherwise — so the log lands beside the history and the
/// session rather than in a second place that has to agree about the name.
pub fn install_from_settings(settings: &Settings) -> Result<(), CliError> {
    install(crate::local::open_storage(settings)?);
    Ok(())
}

/// Run `read` against the installed log, or return `None` when none is installed.
///
/// The read half of the sink: a caller that wants the recent decisions, and a test that
/// wants to verify the chain, both go through this rather than reaching for the lock.
pub fn with_storage<R>(read: impl FnOnce(&Storage) -> R) -> Option<R> {
    let guard = sink().lock().expect("the log sink mutex is not poisoned");
    guard.as_ref().map(read)
}

/// Write one decision to the installed log, or do nothing when none is installed.
pub(crate) fn record(entry: &DecisionEntry<'_>) -> Result<(), StorageError> {
    let mut guard = sink().lock().expect("the log sink mutex is not poisoned");
    let Some(storage) = guard.as_mut() else {
        return Ok(());
    };
    storage.append_execution(NewExecution {
        safe_mode: entry.safe_mode.as_str(),
        decision: entry.decision.as_str(),
        statement_kind: entry.kind.token(),
        statement_index: i64::try_from(entry.index).unwrap_or(i64::MAX),
        statement: entry.statement,
        reason: entry.reason,
        at: qh_storage::now_millis(),
    })?;
    Ok(())
}
