//! The session layer's retry: what this engine does when an accident interrupts a run.
//!
//! `RETRIES` used to be read and ignored (`docs/golden-deltas.md` D-7). This module is
//! the policy that makes it mean something, and it is written down here because a retry
//! that is not described is a retry nobody can argue with.
//!
//! # What is retried
//!
//! Only [`FailureKind::Transient`] — the vocabulary [`qh_core::EngineError`] already
//! carries, and the same classification `exporter/source.py:57` used (`PERMANENT`
//! there). A retry is attempted at three places, and each has its own reason for being
//! safe:
//!
//! 1. **A connection that never opened**, and **a statement that failed before any row
//!    was delivered** — for all three drivers. Nothing has been handed to the caller
//!    yet, so a second attempt cannot repeat anything, and it cannot truncate anything:
//!    there is no partial result to be partial about. This is where a stopped or
//!    restarting coordinator lands, because Trino's `connect` does no I/O and the
//!    transport failure surfaces from the first `POST`.
//! 2. **A page request that failed mid-stream** — Trino only, and only when the request
//!    produced no page at all. A Trino page is a URI, so re-requesting it returns the
//!    page that was never delivered, and the cursor's pending queue is drained only on a
//!    successful fetch (`qh-driver-trino`'s `next_batch` returns the batch it drained and
//!    otherwise leaves the queue untouched), so **a row already handed to the writer
//!    cannot be fetched twice**. The rows already written stay written, and the file
//!    grows by the page that was missing rather than repeating its neighbours.
//! 3. **A listing** (`browse`, `objects`) — all three drivers. A listing is answered
//!    whole or not at all, so a retry either returns the complete answer or a failure;
//!    there is no partial answer to duplicate.
//!
//! # What is not retried, and why
//!
//! - **A statement the server refused** — a syntax error, a missing table, a missing
//!   column, a permission failure, bad credentials. These are *answers*, not accidents:
//!   repeating the statement produces the same answer, and the retry only makes the user
//!   wait for it. The previous engine said the same thing in prose
//!   (`source.py:53-61`) and its test suite pins the classification
//!   (`a_syntax_error_carries_the_servers_code_and_is_not_retried`). Those all arrive as
//!   [`FailureKind::Permanent`], and so does a usage error.
//! - **A cancel** ([`FailureKind::Cancelled`]). The user asked for the stop.
//! - **A page the server answered with an error instead of rows.** This one is not
//!   obvious, so it is spelled out: the driver marks its cursor finished *before* it
//!   reports a page error (`qh-driver-trino`'s `fetch_page` sets `finished = true` and
//!   then returns `map_error(error)`), so asking that cursor again would answer
//!   `Ok(None)` — end of result. Retrying there would end the result early **and report
//!   success**, which is a silent truncation and worse than the failure it hid. So a
//!   mid-stream retry requires that the failure carried no server code
//!   (`produced_no_page` below); a page that was parsed and carried an error always has one
//!   (`errorCode` is part of Trino's error object), and those — `INTERNAL_ERROR`,
//!   `INSUFFICIENT_RESOURCES`, a `404` for a URI whose query the coordinator has
//!   forgotten — are the server's answer to this statement, not an accident.
//! - **A failed fetch on PostgreSQL or MySQL.** Their cursor is a position inside a live
//!   connection, not a URI anything can be re-requested from — both drivers declare
//!   `persistent_connection: true`. When that connection is gone, the rows after the
//!   break cannot be read *by asking again*; the only thing that could is re-running the
//!   statement, and that would deliver the rows already written to the file a second
//!   time. So the failure is reported, the file keeps what it has, and the loss is
//!   visible. `RETRIES` is not silently ignored there: it governs the connect and the
//!   statement (both above), and the promise stops at the first delivered row. That is a
//!   real limit of a cursor that lives inside a connection, and it is written here
//!   rather than papered over.
//! - **A writing statement.** `to_table` runs a `DROP`/`CREATE TABLE AS`/`INSERT`, and a
//!   statement that changes rows is not safe to re-issue blindly — a `CREATE` the
//!   coordinator accepted but could not answer about may well have committed. `to_table`
//!   therefore opens its session with [`connect`] and never calls [`execute`]. The
//!   previous engine did not loop on these either: `to_table.py` never used
//!   `QueryStream`'s retry loop.
//!
//! # How many times, and how long between
//!
//! `RETRIES` is the number of retries, so the number of attempts is `RETRIES + 1` and
//! `RETRIES=0` means one attempt: exactly the previous engine's reading
//! (`for attempt in range(self.retries + 1)`, `exporter/source.py:206`), and its default
//! of 5 (`source.py:187`, `queryhive_engine.py:575`, and `cli.py --retries`). A blank or
//! absent setting keeps 5; `RETRIES` is floored at 0, and a value that is not a whole
//! number is refused by name like every other numeric setting.
//!
//! The wait before attempt *n+1* doubles from 2 s: 2 s, 4 s, 8 s, 16 s, 32 s. That is
//! `retry_backoff = 2.0` with `delay = retry_backoff * 2**attempt` in the previous
//! engine, and the same 62 s total at 5 retries (so the "verdict that never changes"
//! concern in `source.py:53-56` keeps the shape it had).
//!
//! Each retry writes one line to **stderr** in the previous engine's shape — stdout is
//! the protocol the app decodes and the golden harness freezes, and the Python engine
//! logged these through `logging.warning`, which also went to stderr.

use std::time::Duration;

use async_trait::async_trait;
use qh_core::{ColumnBatch, ColumnMeta, EngineError, FailureKind};
use qh_driver::{
    BrowseLevel, ConnectionConfig, Cursor, ExecuteOptions, ObjectPath, ObjectsPage, Session,
};

use crate::env::{SettingError, Settings};
use crate::Engine;

/// Retries when `RETRIES` says nothing.
///
/// 5 because that is what the previous engine defaulted to on every command that had a
/// retry at all (`exporter/source.py:187` and its `--retries` flag in
/// `exporter/cli.py:80`), and matching it is the point of this engine.
pub const DEFAULT_RETRIES: i64 = 5;

/// The wait before the first retry; each later one doubles it.
///
/// `QueryStream(retry_backoff=2.0)` in `exporter/source.py:188`.
pub const BACKOFF_BASE: Duration = Duration::from_secs(2);

/// How many retries, and how long between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries *after* the first attempt, so the attempt count is this plus one.
    pub retries: u32,
    pub backoff: Duration,
}

impl RetryPolicy {
    pub const fn new(retries: u32, backoff: Duration) -> Self {
        Self { retries, backoff }
    }

    /// The policy `RETRIES` asks for.
    ///
    /// A negative value is floored at 0 rather than refused: `RETRIES=-1` means "do not
    /// retry" in every reading, and a stored connection carrying it should keep working.
    /// A value that is not a number is refused, because that is a setting the caller got
    /// wrong rather than a choice about how many attempts to make.
    pub fn from_settings(settings: &Settings) -> Result<Self, SettingError> {
        let retries = settings.number("RETRIES", DEFAULT_RETRIES)?;
        Ok(Self::new(
            u32::try_from(retries.max(0)).unwrap_or(u32::MAX),
            BACKOFF_BASE,
        ))
    }

    /// Total attempts, including the first one.
    pub const fn attempts(&self) -> u32 {
        self.retries + 1
    }

    /// The wait before the attempt that follows `attempt` (0-based).
    ///
    /// `saturating_mul` with a shift that cannot overflow: `RETRIES` is a user's number,
    /// and a large one must not panic this engine into an `internal error` event.
    fn delay(&self, attempt: u32) -> Duration {
        let factor = 1u32.checked_shl(attempt).unwrap_or(u32::MAX);
        self.backoff.saturating_mul(factor)
    }
}

/// Whether another attempt is allowed.
///
/// The error must be one a retry can help with, and the policy must have attempts left.
fn again(policy: &RetryPolicy, attempt: u32, error: &EngineError) -> bool {
    attempt < policy.retries && error.failure_kind() == FailureKind::Transient
}

/// Whether a failed call produced no page from the server.
///
/// The test the mid-stream retry needs and the statement-level one does not: see the
/// module note on the page the server answers with an error. A server's answer always
/// carries its code ([`EngineError::code`]), and a failure before anything was parsed
/// never does.
fn produced_no_page(error: &EngineError) -> bool {
    match error {
        // The request never reached a coordinator, so no body was read and no page was
        // parsed. Trino's own cursor reports page failures as `Query`, but a driver
        // whose transport fails before a response is written is describable this way,
        // and it is the same fact.
        EngineError::Connect { .. } => true,
        EngineError::Query { code, .. } => code.is_none(),
        _ => false,
    }
}

/// Whether a mid-stream fetch may be asked for again.
fn again_mid_stream(policy: &RetryPolicy, attempt: u32, error: &EngineError) -> bool {
    again(policy, attempt, error) && produced_no_page(error)
}

/// One line per retry, on stderr: the previous engine's `logging.warning` in one place.
fn log_retry(policy: &RetryPolicy, attempt: u32, what: &str, delay: Duration, error: &EngineError) {
    eprintln!(
        "retry {}/{} on {what} in {:.1}s: {}",
        attempt + 1,
        policy.retries,
        delay.as_secs_f64(),
        error.message()
    );
}

/// Wait out the backoff, saying so first.
async fn wait(policy: &RetryPolicy, attempt: u32, what: &str, error: &EngineError) {
    let delay = policy.delay(attempt);
    log_retry(policy, attempt, what, delay, error);
    tokio::time::sleep(delay).await;
}

/// Open a session, retrying a transient failure.
///
/// A function rather than a session wrapper, and this is why: [`Session::cancel`] takes
/// `&self` and its future has to be `Send`, which needs a session that is `Sync` — and a
/// `Box<dyn Session>`, which is what every driver hands back, is only `Send`. Wrapping it
/// would mean either a lock held across an await or a `Sync` bound the driver crates do
/// not carry. So the calls that may be repeated are functions instead:
/// [`execute`], [`browse`], [`objects`], and [`Cursor`] retries through the cursor
/// [`execute`] returns.
pub async fn connect(
    engine: &dyn Engine,
    config: &ConnectionConfig,
    policy: &RetryPolicy,
) -> Result<Box<dyn Session>, EngineError> {
    let mut attempt = 0;
    loop {
        match engine.connect(config).await {
            Ok(session) => return Ok(session),
            Err(error) => {
                if !again(policy, attempt, &error) {
                    return Err(error);
                }
                wait(policy, attempt, "connect", &error).await;
                attempt += 1;
            }
        }
    }
}

/// Run a statement, retrying a transient failure, and hand back a cursor that retries a
/// failed page request.
///
/// The returned cursor is where a mid-stream failure is handled: a fetch that produced no
/// page is asked for again (see `produced_no_page` below), and nothing that was already
/// delivered is fetched twice.
pub async fn execute(
    session: &mut Box<dyn Session>,
    policy: &RetryPolicy,
    sql: &str,
    options: &ExecuteOptions,
) -> Result<Box<dyn Cursor>, EngineError> {
    let mut attempt = 0;
    loop {
        match session.execute(sql, options).await {
            // Nothing has been delivered at this point, which is what makes a second
            // attempt safe: there is no row to repeat and no partial result to truncate.
            Ok(cursor) => {
                return Ok(Box::new(RetryCursor {
                    inner: cursor,
                    policy: *policy,
                    resumable: !session.capabilities().persistent_connection,
                }))
            }
            Err(error) => {
                if !again(policy, attempt, &error) {
                    return Err(error);
                }
                wait(policy, attempt, "execute", &error).await;
                attempt += 1;
            }
        }
    }
}

/// List one level, retrying a transient failure.
///
/// A listing is answered whole or not at all, so a retry either returns the complete
/// answer or a failure: there is no partial answer to duplicate.
pub async fn browse(
    session: &mut Box<dyn Session>,
    policy: &RetryPolicy,
    level: BrowseLevel,
    path: &ObjectPath,
    include_system: bool,
) -> Result<Vec<String>, EngineError> {
    let mut attempt = 0;
    loop {
        match session.browse(level, path, include_system).await {
            Ok(names) => return Ok(names),
            Err(error) => {
                if !again(policy, attempt, &error) {
                    return Err(error);
                }
                wait(policy, attempt, "list", &error).await;
                attempt += 1;
            }
        }
    }
}

/// One level's objects, retrying a transient failure. The same argument as [`browse`].
pub async fn objects(
    session: &mut Box<dyn Session>,
    policy: &RetryPolicy,
    path: &ObjectPath,
) -> Result<ObjectsPage, EngineError> {
    let mut attempt = 0;
    loop {
        match session.objects(path).await {
            Ok(page) => return Ok(page),
            Err(error) => {
                if !again(policy, attempt, &error) {
                    return Err(error);
                }
                wait(policy, attempt, "objects", &error).await;
                attempt += 1;
            }
        }
    }
}

/// One cursor, with the page-request retry around it.
struct RetryCursor {
    inner: Box<dyn Cursor>,
    policy: RetryPolicy,
    resumable: bool,
}

#[async_trait]
impl Cursor for RetryCursor {
    fn columns(&self) -> &[ColumnMeta] {
        self.inner.columns()
    }

    fn affected_rows(&self) -> Option<u64> {
        self.inner.affected_rows()
    }

    async fn next_batch(&mut self, max_rows: usize) -> Result<Option<ColumnBatch>, EngineError> {
        let mut attempt = 0;
        loop {
            match self.inner.next_batch(max_rows).await {
                Ok(batch) => return Ok(batch),
                Err(error) => {
                    if !self.resumable || !again_mid_stream(&self.policy, attempt, &error) {
                        return Err(error);
                    }
                    wait(&self.policy, attempt, "fetch", &error).await;
                    attempt += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use qh_core::Value;
    use qh_driver::Capabilities;

    use super::*;

    /// How a scripted call answers: a result, a failure, or the end of the result.
    enum Step {
        Rows(Vec<i64>),
        Fail(EngineError),
        End,
    }

    fn transient(message: &str) -> EngineError {
        EngineError::Connect {
            message: message.to_owned(),
            kind: FailureKind::Transient,
        }
    }

    /// A failure with no server code: a request that produced no page.
    fn transport(message: &str) -> EngineError {
        EngineError::Query {
            message: message.to_owned(),
            code: None,
            position: None,
            kind: FailureKind::Transient,
        }
    }

    /// A failure the server answered with, code and all — the shape a syntax error and a
    /// page carrying an error both have.
    fn answered(code: &str, kind: FailureKind) -> EngineError {
        EngineError::Query {
            message: format!("the server said {code}"),
            code: Some(code.to_owned()),
            position: None,
            kind,
        }
    }

    struct ScriptedCursor {
        steps: VecDeque<Step>,
        columns: Vec<ColumnMeta>,
        /// Shared so a test can count how many times the cursor was asked.
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Cursor for ScriptedCursor {
        fn columns(&self) -> &[ColumnMeta] {
            &self.columns
        }

        async fn next_batch(
            &mut self,
            _max_rows: usize,
        ) -> Result<Option<ColumnBatch>, EngineError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match self.steps.pop_front() {
                None | Some(Step::End) => Ok(None),
                Some(Step::Rows(rows)) => Ok(Some(
                    ColumnBatch::new(vec![rows.into_iter().map(Value::Int).collect()])
                        .expect("a square batch"),
                )),
                Some(Step::Fail(error)) => Err(error),
            }
        }
    }

    struct ScriptedSession {
        /// One entry per `execute` call, in order; past the end the cursor is returned.
        failures: VecDeque<EngineError>,
        cursor: Option<ScriptedCursor>,
        /// How many times `execute` was called.
        executes: Arc<AtomicUsize>,
        persistent: bool,
    }

    impl ScriptedSession {
        fn new(failures: Vec<EngineError>, steps: Vec<Step>) -> Self {
            Self {
                failures: failures.into(),
                cursor: Some(ScriptedCursor {
                    steps: steps.into(),
                    columns: vec![ColumnMeta::new("n".to_owned(), "bigint".to_owned())],
                    calls: Arc::new(AtomicUsize::new(0)),
                }),
                executes: Arc::new(AtomicUsize::new(0)),
                persistent: false,
            }
        }
    }

    #[async_trait]
    impl Session for ScriptedSession {
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                transactions: false,
                multiple_result_sets: false,
                cancel: true,
                explain: true,
                levels: vec![BrowseLevel::Table],
                objects_columns: vec!["Name".to_owned()],
                persistent_connection: self.persistent,
            }
        }

        fn query_id(&self) -> Option<String> {
            None
        }

        async fn execute(
            &mut self,
            _sql: &str,
            _options: &ExecuteOptions,
        ) -> Result<Box<dyn Cursor>, EngineError> {
            self.executes.fetch_add(1, Ordering::SeqCst);
            match self.failures.pop_front() {
                Some(error) => Err(error),
                None => Ok(Box::new(
                    self.cursor.take().expect("the cursor is handed out once"),
                )),
            }
        }

        async fn browse(
            &mut self,
            _level: BrowseLevel,
            _path: &ObjectPath,
            _include_system: bool,
        ) -> Result<Vec<String>, EngineError> {
            Ok(Vec::new())
        }

        async fn objects(&mut self, _path: &ObjectPath) -> Result<ObjectsPage, EngineError> {
            Ok(ObjectsPage::default())
        }

        fn explain_statement(&self, sql: &str) -> String {
            sql.to_owned()
        }

        async fn cancel(&self) -> Result<(), EngineError> {
            Ok(())
        }

        async fn close(self: Box<Self>) -> Result<(), EngineError> {
            Ok(())
        }
    }

    /// A policy that never sleeps: the waits are covered separately.
    fn instant(retries: u32) -> RetryPolicy {
        RetryPolicy::new(retries, Duration::ZERO)
    }

    /// Every row a cursor hands out, in order, through the retry layer.
    async fn drain(
        session: &mut Box<dyn Session>,
        policy: &RetryPolicy,
    ) -> Result<Vec<i64>, EngineError> {
        let mut cursor = execute(session, policy, "SELECT n", &ExecuteOptions::default()).await?;
        let mut rows = Vec::new();
        while let Some(batch) = cursor.next_batch(100).await? {
            for value in &batch.columns()[0] {
                match value {
                    Value::Int(number) => rows.push(*number),
                    other => panic!("expected an integer, got {other:?}"),
                }
            }
        }
        Ok(rows)
    }

    #[tokio::test]
    async fn a_transient_execute_failure_is_retried_until_it_works() {
        let session = ScriptedSession::new(
            vec![
                transient("connection reset"),
                transport("could not reach the coordinator"),
            ],
            vec![Step::Rows(vec![1, 2]), Step::End],
        );
        let executes = Arc::clone(&session.executes);
        let mut session: Box<dyn Session> = Box::new(session);

        assert_eq!(drain(&mut session, &instant(5)).await.unwrap(), vec![1, 2]);
        assert_eq!(
            executes.load(Ordering::SeqCst),
            3,
            "two failures, then the attempt that worked"
        );
    }

    #[tokio::test]
    async fn retries_zero_means_one_attempt_and_the_failure_is_reported() {
        let session = ScriptedSession::new(vec![transient("connection refused")], Vec::new());
        let executes = Arc::clone(&session.executes);
        let mut session: Box<dyn Session> = Box::new(session);

        let error = drain(&mut session, &instant(0))
            .await
            .expect_err("RETRIES=0 does not retry");
        assert!(error.message().contains("connection refused"), "{error:?}");
        assert_eq!(executes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_statement_the_server_refused_is_never_retried() {
        // The exact shape `a_syntax_error_carries_the_servers_code_and_is_not_retried`
        // pins in the Trino suite: code 1, USER_ERROR, Permanent.
        let session = ScriptedSession::new(
            vec![answered("1", FailureKind::Permanent)],
            vec![Step::Rows(vec![1]), Step::End],
        );
        let executes = Arc::clone(&session.executes);
        let mut session: Box<dyn Session> = Box::new(session);

        let error = drain(&mut session, &instant(5))
            .await
            .expect_err("a refusal is an answer");
        assert_eq!(error.failure_kind(), FailureKind::Permanent);
        assert_eq!(
            executes.load(Ordering::SeqCst),
            1,
            "one attempt, no waiting"
        );
    }

    #[tokio::test]
    async fn a_cancel_is_never_retried() {
        let session = ScriptedSession::new(
            vec![answered("3", FailureKind::Cancelled)],
            vec![Step::Rows(vec![1]), Step::End],
        );
        let executes = Arc::clone(&session.executes);
        let mut session: Box<dyn Session> = Box::new(session);

        let error = drain(&mut session, &instant(5))
            .await
            .expect_err("the user asked for the stop");
        assert_eq!(error.failure_kind(), FailureKind::Cancelled);
        assert_eq!(executes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_transient_page_request_is_retried_without_repeating_a_delivered_row() {
        // Two rows, then a page that never arrived, then the rest of the result: the
        // sequence must read 1,2,3,4 — the page is re-requested, the rows already
        // delivered are not.
        let session = ScriptedSession::new(
            Vec::new(),
            vec![
                Step::Rows(vec![1, 2]),
                Step::Fail(transport(
                    "could not fetch the next page: connection closed",
                )),
                Step::Rows(vec![3, 4]),
                Step::End,
            ],
        );
        let calls = Arc::clone(&session.cursor.as_ref().expect("a cursor").calls);
        let mut session: Box<dyn Session> = Box::new(session);

        assert_eq!(
            drain(&mut session, &instant(3)).await.unwrap(),
            vec![1, 2, 3, 4]
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            4,
            "four fetches: the failed one was asked again"
        );
    }

    #[tokio::test]
    async fn a_page_the_server_answered_is_not_retried_so_the_result_cannot_truncate() {
        // The driver sets `finished` before it reports a page error, so asking again
        // would answer `Ok(None)` — an early end reported as success. The failure is
        // reported instead, and the test names the truncation it prevents.
        let session = ScriptedSession::new(
            Vec::new(),
            vec![
                Step::Rows(vec![1]),
                Step::Fail(answered("99", FailureKind::Transient)),
                Step::End,
            ],
        );
        let calls = Arc::clone(&session.cursor.as_ref().expect("a cursor").calls);
        let mut session: Box<dyn Session> = Box::new(session);

        let error = drain(&mut session, &instant(3))
            .await
            .expect_err("an answered page is not an accident");
        assert_eq!(error.code(), Some("99"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the cursor was asked once more, not three times"
        );
    }

    #[tokio::test]
    async fn a_cursor_inside_a_connection_is_not_retried_mid_stream() {
        // PostgreSQL and MySQL declare `persistent_connection`. A fetch that failed
        // there cannot be re-issued, and re-running the statement would repeat the rows
        // already written, so the failure is reported on the first attempt.
        let mut scripted = ScriptedSession::new(
            Vec::new(),
            vec![
                Step::Rows(vec![1]),
                Step::Fail(transport("the connection to the server was closed")),
                Step::Rows(vec![2]),
                Step::End,
            ],
        );
        scripted.persistent = true;
        let calls = Arc::clone(&scripted.cursor.as_ref().expect("a cursor").calls);
        let mut session: Box<dyn Session> = Box::new(scripted);

        let error = drain(&mut session, &instant(3))
            .await
            .expect_err("a broken cursor cannot resume");
        assert!(error.message().contains("was closed"), "{error:?}");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the second failure was not asked again"
        );
    }

    #[test]
    fn the_policy_reads_retries_and_defaults_to_the_previous_engines_five() {
        let read = |pairs: &[(&str, &str)]| {
            let settings = Settings::from_pairs(pairs.iter().map(|(k, v)| (*k, *v)));
            RetryPolicy::from_settings(&settings)
        };
        assert_eq!(
            read(&[]).unwrap().retries,
            5,
            "the previous engine's default"
        );
        assert_eq!(read(&[("RETRIES", "2")]).unwrap().retries, 2);
        assert_eq!(read(&[("RETRIES", "0")]).unwrap().retries, 0);
        assert_eq!(
            read(&[("RETRIES", "")]).unwrap().retries,
            5,
            "blank is unset"
        );
        assert_eq!(
            read(&[("RETRIES", "-3")]).unwrap().retries,
            0,
            "floored at no retry"
        );
        assert_eq!(read(&[("RETRIES", "2")]).unwrap().attempts(), 3);
        let error = read(&[("RETRIES", "many")]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "RETRIES must be a whole number, got 'many'"
        );
    }

    #[test]
    fn the_wait_doubles_from_two_seconds_and_a_huge_retry_count_does_not_overflow() {
        let policy = RetryPolicy::new(5, BACKOFF_BASE);
        assert_eq!(policy.delay(0), Duration::from_secs(2));
        assert_eq!(policy.delay(1), Duration::from_secs(4));
        assert_eq!(policy.delay(2), Duration::from_secs(8));
        assert_eq!(
            policy.delay(4),
            Duration::from_secs(32),
            "62 s in total at 5 retries"
        );
        // A user's number must not panic this engine into an `internal error`.
        let generous = RetryPolicy::new(1_000, BACKOFF_BASE);
        assert!(generous.delay(999) <= Duration::MAX);
    }
}
