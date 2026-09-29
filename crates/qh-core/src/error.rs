//! The error taxonomy, shared by every driver and mapped to Swift at the FFI
//! boundary.
//!
//! The shape follows what the UI has to be able to do with a failure, not what
//! is convenient to construct:
//!
//! * tell the user what went wrong ([`EngineError::message`]),
//! * show the server's own code when there is one (SQLSTATE for Postgres, the
//!   Trino error code, MySQL's errno) so a search for it finds the right page,
//! * point at the offending text in the editor when the server said where it
//!   objected,
//! * and decide whether retrying is allowed at all.
//!
//! That last one is [`FailureKind`], and it is load-bearing: the Python engine
//! classifies errors the same way (`exporter/source.py:57`, `PERMANENT`) so that
//! a syntax error is never retried five times before the user sees it.

use std::time::Duration;

use thiserror::Error;

/// Whether an operation may be tried again.
///
/// Deliberately explicit rather than inferred from the error type: the same
/// class of error can be either, and guessing wrong either hides a real failure
/// behind a retry loop or retries something that can never succeed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Worth retrying: a dropped connection, a 502/503/504 from a coordinator,
    /// a read timeout. These are the cases the Python engine retried with
    /// exponential backoff.
    Transient,
    /// Retrying cannot help: bad credentials, a syntax error, a missing table, a
    /// blank required setting. Surfaced to the user on the first failure.
    Permanent,
    /// The user asked for it to stop. Never retried, and never reported as a
    /// failure the user should act on.
    Cancelled,
}

/// Everything that can go wrong between a UI action and a row of data.
///
/// `equality` is derived so tests can assert on the classification without
/// matching on strings that a message change would break.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EngineError {
    /// The caller's own request was unusable — a blank statement, a missing
    /// setting the driver needs to build its SQL. Produced before the network is
    /// touched, so the user is never left waiting for a round trip to be told
    /// they forgot a field.
    #[error("usage: {message}")]
    Usage { message: String },

    /// The connection did not open, or died and could not be re-established.
    #[error("{message}")]
    Connect { message: String, kind: FailureKind },

    /// The server refused the statement, or the connection failed while it ran.
    ///
    /// `code` and `position` are optional because not every server reports them:
    /// Trino gives a code, Postgres gives a SQLSTATE plus a 1-based character
    /// offset, MySQL gives an errno. They are kept as reported rather than
    /// normalised, so the value in the UI still matches the server's own docs.
    #[error("{message}")]
    Query {
        message: String,
        code: Option<String>,
        /// 1-based character offset into the statement, when the server said so.
        position: Option<u32>,
        kind: FailureKind,
    },

    /// A result handle referred to a store that has been released.
    ///
    /// This is expected rather than exceptional: a grid cell request can arrive
    /// just after the user closed the tab. The UI ignores it. It exists so that
    /// the alternative — a dangling pointer — is representable as an error
    /// instead of as undefined behaviour (blueprint section 2.6).
    #[error("result handle is stale")]
    StaleHandle,

    /// The server stopped the statement because it ran past its own time bound.
    ///
    /// Its own variant rather than a [`EngineError::Query`] because the caller
    /// must be able to tell "this query was slow" apart from "this query was
    /// wrong" without parsing a message: the first is a limit the user can raise,
    /// the second is the server's answer. `limit_ms` is the bound this engine
    /// asked for, when it asked for one; the message always names it, because a
    /// timeout that does not say what it timed out against is not actionable.
    #[error("{message}")]
    Timeout {
        message: String,
        /// The configured statement timeout, in milliseconds, when known.
        limit_ms: Option<u64>,
    },

    /// A bug in this program, including a caught panic at the FFI boundary.
    ///
    /// Never caused by server data. It is separated from the variants above
    /// because it is the only one that should be reported as a defect, and it
    /// carries no retry advice.
    #[error("internal error: {message}")]
    Internal { message: String },
}

impl EngineError {
    /// Whether the operation that produced this error may be attempted again.
    pub fn failure_kind(&self) -> FailureKind {
        match self {
            EngineError::Connect { kind, .. } | EngineError::Query { kind, .. } => *kind,
            EngineError::Usage { .. } | EngineError::Internal { .. } => FailureKind::Permanent,
            // A bound the caller set is not an accident: repeating the statement
            // under the same limit produces the same timeout, so a retry would
            // only make the user wait for it again.
            EngineError::Timeout { .. } => FailureKind::Permanent,
            EngineError::StaleHandle => FailureKind::Cancelled,
        }
    }

    /// The message shown to the user, without the enum variant's own wording.
    ///
    /// The FFI boundary hands the UI this string plus [`EngineError::code`] and
    /// [`EngineError::position`], rather than a `Debug` rendering of the enum:
    /// users should never see Rust type names.
    pub fn message(&self) -> &str {
        match self {
            EngineError::Usage { message }
            | EngineError::Connect { message, .. }
            | EngineError::Query { message, .. }
            | EngineError::Timeout { message, .. }
            | EngineError::Internal { message } => message,
            EngineError::StaleHandle => "result handle is stale",
        }
    }

    /// The server's own error code, when one was reported.
    pub fn code(&self) -> Option<&str> {
        match self {
            EngineError::Query { code, .. } => code.as_deref(),
            _ => None,
        }
    }

    /// The 1-based offset into the statement the server objected to.
    pub fn position(&self) -> Option<u32> {
        match self {
            EngineError::Query { position, .. } => *position,
            _ => None,
        }
    }

    /// The statement timeout this engine asked for, when one was in force.
    pub fn limit_ms(&self) -> Option<u64> {
        match self {
            EngineError::Timeout { limit_ms, .. } => *limit_ms,
            _ => None,
        }
    }

    /// The error a driver reports when the server stopped a statement for
    /// running past `limit`.
    ///
    /// One constructor so all three drivers word it the same way, and so the
    /// limit is always named when it is known — the whole point of giving this
    /// its own variant. `server_message` is the server's own sentence and is
    /// appended rather than dropped: it is where a coordinator names the bound it
    /// actually enforced.
    pub fn statement_timeout(limit: Option<Duration>, server_message: &str) -> Self {
        let message = match limit {
            Some(limit) => format!(
                "the statement exceeded the {} ms statement timeout and was cancelled by the server",
                limit.as_millis()
            ),
            None => {
                "the statement was cancelled by the server because it exceeded a statement timeout"
                    .to_owned()
            }
        };
        let message = if server_message.trim().is_empty() {
            message
        } else {
            format!("{message}: {}", server_message.trim())
        };
        EngineError::Timeout {
            message,
            limit_ms: limit.and_then(|limit| u64::try_from(limit.as_millis()).ok()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_syntax_error_is_never_retried() {
        // The Python engine classifies these as PERMANENT (exporter/source.py:57);
        // retrying would make the user wait five round trips to see a typo.
        let error = EngineError::Query {
            message: "line 1:1: Column 'nope' cannot be resolved".into(),
            code: Some("47".into()),
            position: Some(15),
            kind: FailureKind::Permanent,
        };
        assert_eq!(error.failure_kind(), FailureKind::Permanent);
        assert_eq!(
            error.message(),
            "line 1:1: Column 'nope' cannot be resolved"
        );
        assert_eq!(error.code(), Some("47"));
        assert_eq!(error.position(), Some(15));
    }

    #[test]
    fn a_dropped_connection_may_be_retried() {
        let error = EngineError::Connect {
            message: "connection reset by peer".into(),
            kind: FailureKind::Transient,
        };
        assert_eq!(error.failure_kind(), FailureKind::Transient);
        assert_eq!(error.code(), None);
    }

    #[test]
    fn a_stale_handle_is_not_a_failure_to_show() {
        // Closing a tab while rows are still arriving must not produce an error
        // dialog: the grid simply drops the answer.
        assert_eq!(
            EngineError::StaleHandle.failure_kind(),
            FailureKind::Cancelled
        );
        assert_eq!(EngineError::StaleHandle.position(), None);
    }

    #[test]
    fn usage_errors_carry_no_retry_advice() {
        let error = EngineError::Usage {
            message: "SQL is required".into(),
        };
        assert_eq!(error.failure_kind(), FailureKind::Permanent);
        assert_eq!(error.to_string(), "usage: SQL is required");
    }

    #[test]
    fn a_statement_timeout_names_its_limit_and_is_not_retried() {
        // The bound is the caller's own, so repeating the statement under the same
        // limit would only make the user wait for the same timeout again.
        let error = EngineError::statement_timeout(
            Some(Duration::from_millis(1_500)),
            "canceling statement due to statement timeout",
        );
        assert_eq!(error.failure_kind(), FailureKind::Permanent);
        assert_eq!(error.limit_ms(), Some(1_500));
        assert!(error.message().contains("1500 ms"), "{}", error.message());
        assert!(
            error.message().contains("statement timeout"),
            "{}",
            error.message()
        );
        // The server's own sentence is kept: it is where the real bound is named.
        assert!(
            error.message().contains("canceling statement"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn a_statement_timeout_without_a_known_limit_still_says_what_it_is() {
        let error = EngineError::statement_timeout(None, "");
        assert_eq!(error.limit_ms(), None);
        assert!(
            error.message().contains("statement timeout"),
            "{}",
            error.message()
        );
    }
}
