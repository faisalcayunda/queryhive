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
//! The one other place that installs is the engine host the app keeps ([`ensure_sink`], called
//! by `EngineHost::run` for the commands that decide anything, and for the local commands that
//! open the database anyway, so the first Run does not pay for the open). The app is the engine too, since
//! the engine moved into its process, and a decision made there was written nowhere until then.
//! A host built over a fake connector (`EngineHost::with_connector`, the tests' seam) does not.
//!
//! # The write is deliberately synchronous
//!
//! `guard` runs on the async path, and the crate's storage rule is that a `rusqlite`
//! connection is never used from a tokio worker. This is the conscious exception: the write
//! is one indexed `INSERT` into a local file, and the alternatives are worse for an audit
//! log. Spawning it would let a decision reach the caller before it reached the log, and a
//! chain that races is a chain that verifies against an order nobody chose. Correctness of
//! the chain wins over a few microseconds on the runtime thread.

use std::sync::atomic::{AtomicUsize, Ordering};
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

/// The installed sink, or nothing.
///
/// `key` is the database the handle was opened for, spelled as [`path_key`] spells it. It is
/// `None` for a handle a caller installed itself ([`install`]): that one is the caller's, and
/// [`ensure_sink`] never replaces it. `OnceLock` so a process installs once and reads many
/// times without a lock on the read path beyond the `Mutex` that keeps `rusqlite` honest.
#[derive(Default)]
struct Sink {
    storage: Option<Storage>,
    key: Option<String>,
    /// The database a failed open was for, so a database that stays unopenable is reported once
    /// and not once per run.
    failed: Option<String>,
}

static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();

/// How many databases [`ensure_sink`] has opened, so a test can tell that the same path is not
/// opened twice.
static OPENS: AtomicUsize = AtomicUsize::new(0);

/// The number of databases [`ensure_sink`] has opened in this process.
#[doc(hidden)]
pub fn sink_opens() -> usize {
    OPENS.load(Ordering::SeqCst)
}

fn sink() -> std::sync::MutexGuard<'static, Sink> {
    SINK.get_or_init(|| Mutex::new(Sink::default()))
        .lock()
        .expect("the log sink mutex is not poisoned")
}

/// Install a storage handle as this process's decision log.
///
/// Installing again replaces the handle; the binary calls this once. A test calls it with an
/// in-memory database, which is how the write path is proven without touching a real one. A handle
/// installed here is not tied to a path, so [`ensure_sink`] leaves it alone.
pub fn install(storage: Storage) {
    let mut sink = sink();
    sink.storage = Some(storage);
    sink.key = None;
}

/// Forget the installed sink. For tests, and for a caller that wants the engine to stop
/// writing decisions.
pub fn uninstall() {
    *sink() = Sink::default();
}

/// Which database `settings` name, spelled the way `local::acquire` keys its shared handle:
/// `DB_PATH` with `~` expanded, or the empty string for the default database.
pub fn path_key(settings: &Settings) -> String {
    let raw = settings.text("DB_PATH", "");
    if raw.is_empty() {
        String::new()
    } else {
        crate::commands::expand_user(&raw)
            .to_string_lossy()
            .into_owned()
    }
}

/// Open the local database and install it as the decision log.
///
/// The path rule is `crate::local::open_storage`'s — `DB_PATH` when the caller named one,
/// the Application Support file otherwise — so the log lands beside the history and the
/// session rather than in a second place that has to agree about the name.
pub fn install_from_settings(settings: &Settings) -> Result<(), CliError> {
    let storage = crate::local::open_storage(settings)?;
    let mut sink = sink();
    sink.storage = Some(storage);
    sink.key = Some(path_key(settings));
    sink.failed = None;
    Ok(())
}

/// Make sure the decision log is the database `settings` name, opening it when it is not.
///
/// What the engine host calls before a command that decides anything, so a decision made through
/// the app is written down like one made through the CLI. Keyed by path because `install` is global
/// to the process: without the key, the first `DB_PATH` to arrive would be the log's for as long as
/// the process lives, and a run for another database (every Swift test has its own) would write to
/// the wrong file while the reader, which opens its own, read another. The same path is one string
/// comparison under a lock and no I/O.
///
/// A failure to open does not stop the command, and is not silent: it is returned (the reader
/// reports `writer: false` on it) and said on stderr once per database.
///
/// Two runs for different databases at the same moment, in one process, take the sink from each
/// other. That is not supported; it can only happen in a test that runs several databases at once,
/// and such a test must not depend on the log.
pub fn ensure_sink(settings: &Settings) -> Result<(), CliError> {
    let key = path_key(settings);
    {
        let sink = sink();
        // A handle a caller installed is the caller's.
        if sink.storage.is_some() && (sink.key.is_none() || sink.key.as_deref() == Some(&key)) {
            return Ok(());
        }
    }
    // Opened outside the lock: it migrates, and a busy file can make it wait.
    match crate::local::open_storage(settings) {
        Ok(storage) => {
            OPENS.fetch_add(1, Ordering::SeqCst);
            let mut sink = sink();
            sink.storage = Some(storage);
            sink.key = Some(key);
            sink.failed = None;
            Ok(())
        }
        Err(error) => {
            let mut sink = sink();
            if sink.failed.as_deref() != Some(&key) {
                eprintln!("the execution log is not being written: {error}");
                sink.failed = Some(key);
            }
            Err(error)
        }
    }
}

/// Run `read` against the installed log, or return `None` when none is installed.
///
/// The read half of the sink: a caller that wants the recent decisions, and a test that
/// wants to verify the chain, both go through this rather than reaching for the lock.
pub fn with_storage<R>(read: impl FnOnce(&Storage) -> R) -> Option<R> {
    sink().storage.as_ref().map(read)
}

/// Write one decision to the installed log, or do nothing when none is installed.
pub(crate) fn record(entry: &DecisionEntry<'_>) -> Result<(), StorageError> {
    let mut guard = sink();
    let Some(storage) = guard.storage.as_mut() else {
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

/// The `decision` a row carries when a Stop could not be confirmed, so it is not one of the four
/// [`LogDecision`]s: no statement was judged, the server just did not say the statement ended.
pub const STOP_UNCONFIRMED_DECISION: &str = "stop_unconfirmed";

/// Write a failed stop to the installed log. `false` when there is no sink (a library caller, a
/// test, a database that would not open) or the write failed, and the caller says it elsewhere.
///
/// A stop that a capped result started in the background has no `done` left to carry its
/// warning, and in the app stderr goes nowhere, so this row is where it can still be found.
/// `reason` must be one of the fixed sentences the engine writes: like every column here it is
/// never an error's own text, which can carry a host.
pub(crate) fn record_stop_unconfirmed(reason: &str) -> bool {
    let mut guard = sink();
    guard
        .storage
        .as_mut()
        .is_some_and(|storage| append_stop_unconfirmed(storage, reason).is_ok())
}

fn append_stop_unconfirmed(storage: &mut Storage, reason: &str) -> Result<(), StorageError> {
    storage.append_execution(NewExecution {
        // Not a Safe Mode outcome, so no mode and no statement: the hash is of the empty text.
        safe_mode: "none",
        decision: STOP_UNCONFIRMED_DECISION,
        statement_kind: "unknown",
        statement_index: 0,
        statement: "",
        reason: Some(reason),
        at: qh_storage::now_millis(),
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_stop_is_a_chained_row_with_no_statement() {
        let mut storage = Storage::in_memory().expect("in-memory database");
        storage.migrate_at(1_000).expect("migrations");
        append_stop_unconfirmed(&mut storage, "the cancel failed").expect("first");
        append_stop_unconfirmed(&mut storage, "no answer to the cancel").expect("second");

        let rows = storage.execution_log(10).expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].decision, STOP_UNCONFIRMED_DECISION);
        assert_eq!(rows[0].reason.as_deref(), Some("no answer to the cancel"));
        assert_eq!(storage.verify_execution_log().expect("chain holds"), 2);
    }
}
