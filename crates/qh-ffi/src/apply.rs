//! `apply_changes`: run a reviewed plan of INSERT/UPDATE/DELETE statements in one
//! transaction, checking each one's affected-row count against what the plan
//! expected.
//!
//! This is the second half of the change-tracking pattern the source study read
//! out of TablePro, and the half QueryHive was missing: the app already builds
//! the statements and shows them (`ChangeReview`), but nothing could run them.
//! The plan is built **once** by the caller and handed here; the review sheet and
//! this command read the same object, so what was approved is what runs.
//!
//! # Why the check is here and not in the app
//!
//! Verifying a write's row count needs the server's own `affected_rows`, and the
//! app never sees one — it is an FFI caller that reads events. The engine has the
//! cursor, so the engine checks. This is also why the check is inside the
//! transaction: an UPDATE whose predicate matched two rows instead of one has
//! already written the second row by the time the count arrives, and the only
//! thing that can still undo it is a rollback.
//!
//! # The rule, and why it is not `!=` for a keyed write
//!
//! A row with a primary key is matched by that key, so the failure that matters
//! is a predicate that matched **more** rows than the one it was built from:
//! `actual > expected`. `actual < expected` is allowed, because MySQL reports
//! zero for an `UPDATE` that sets a column to the value it already held, and that
//! is a save that worked. A keyless write (this app has no primary-key metadata)
//! is matched by every column at its fetched value, so both directions are real:
//! fewer means the row is gone, more means identical rows were all changed. So
//! `actual != expected` there.
//!
//! # Order is the plan's
//!
//! Statements run in the order given. A plan that frees a unique value by deleting
//! a row and then inserts a row that takes it only works if the delete is first;
//! the study records that this is why TablePro stamps a sequence on each change.
//! This command does not reorder anything.
//!
//! # A failure carries a disposition, not just a message
//!
//! When a statement fails (or its count disagrees) the transaction is rolled back
//! and the error says what became of the statements that already ran. That
//! distinction is the study's `DataWritePartialCommitError`: a plan with no
//! transaction, or whose rollback did not take effect, is [`Disposition::Written`]
//! — the rows are in the table, and the message sends the user to the table rather
//! than inviting a retry that would write them twice. A plan that ran inside a
//! transaction and did not commit is [`Disposition::PendingInSessionTransaction`]:
//! the rows were rolled back and the plan is the user's to fix and run again.

use std::time::Duration;

use qh_driver::{ExecuteOptions, Session};
use qh_sql::SafeMode;
use serde_json::Value as Json;

use crate::commands::{connection, guard, open, safe_mode, statement_timeout};
use crate::env::Settings;
use crate::events::{event, Emitter};
use crate::{CancelFlag, CliError, Engine};

/// One statement of the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Change {
    sql: String,
    /// The rows the plan expects this statement to affect. `None` means the plan
    /// made no claim, so nothing is verified.
    expected: Option<i64>,
    /// Whether the predicate is keyed. `true` uses the `actual > expected` rule;
    /// `false` uses `actual != expected`.
    keyed: bool,
}

/// What became of the statements that had already run when a plan failed.
///
/// This is the source study's vocabulary, and the distinction is the whole point:
/// a failure the user can still undo is not the same as one that is already in
/// the table, and telling them apart is what stops a blind retry from writing
/// twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Disposition {
    /// No transaction was open, or the transaction could not be undone cleanly.
    /// The rows are (or may be) in the table, and running the plan again would
    /// write them a second time.
    Written,
    /// The plan ran inside a transaction and did not commit. The engine rolled it
    /// back, so none of the statements are in the table, and the plan is the
    /// caller's to fix and run again.
    ///
    /// QueryHive's engine owns and closes its own transaction, so unlike TablePro
    /// there is no user session transaction left open to hold the statements; the
    /// name is kept because it is the study's token and it still answers the one
    /// question that matters — was anything written?
    PendingInSessionTransaction,
}

impl Disposition {
    fn as_str(self) -> &'static str {
        match self {
            Disposition::Written => "written",
            Disposition::PendingInSessionTransaction => "pendingInSessionTransaction",
        }
    }
}

/// `apply_changes`: the reviewed plan, run.
pub async fn apply_changes(
    settings: &Settings,
    out: &mut dyn Emitter,
    engine: &dyn Engine,
    cancel: &CancelFlag,
) -> Result<(), CliError> {
    let changes = parse_changes(settings)?;
    if changes.is_empty() {
        return Err(CliError::Usage(
            "CHANGES is empty: there is nothing to apply".to_owned(),
        ));
    }
    let safe = safe_mode(settings)?;
    // Guarded before the connect step, so a read-only connection refuses the
    // whole plan without opening one — the same order every other write command
    // follows.
    for (index, change) in changes.iter().enumerate() {
        guard(safe, &change.sql).map_err(|error| {
            CliError::Usage(format!(
                "change {} was refused: {}",
                index + 1,
                error.message()
            ))
        })?;
    }
    let timeout = statement_timeout(settings)?;
    let want_transaction = settings.flag("IN_TRANSACTION", true);
    let config = connection(settings, engine)?;
    let engine_name = config.kind.as_str();

    out.emit(event("step").field("step", "connect").build())?;
    let (mut session, _policy) = open(settings, engine, &config).await?;

    let in_transaction = want_transaction && session.capabilities().transactions;
    if in_transaction {
        run(&mut session, "BEGIN", timeout).await?;
    }
    out.emit(event("step").field("step", "write").build())?;

    let mut applied = 0u64;
    let mut results: Vec<Json> = Vec::new();
    for (index, change) in changes.iter().enumerate() {
        if cancel.is_cancelled() {
            // A stop keeps what was applied, and the transaction decides whether
            // that means anything: in a transaction this rollback discards it,
            // and without one it is already written.
            let rolled_back = rollback(&mut session, in_transaction, timeout).await;
            let _ = session.close().await;
            let message = if rolled_back {
                format!(
                    "the change plan on {engine_name} was cancelled before it finished; the \
                     transaction was rolled back, so none of the plan is in the table"
                )
            } else if in_transaction {
                format!(
                    "the change plan on {engine_name} was cancelled before it finished; the \
                     rollback did not take effect, so the {applied} statement(s) already written \
                     may still be in the table — look at the table before running the plan again"
                )
            } else {
                format!(
                    "the change plan on {engine_name} was cancelled before it finished; there is \
                     no transaction to undo them, so the {applied} statement(s) already written \
                     are in the table — look at the table before running the plan again"
                )
            };
            return Err(CliError::Warned {
                message,
                warnings: vec![],
            });
        }
        let affected = match run(&mut session, &change.sql, timeout).await {
            Ok(affected) => affected,
            Err(error) => {
                let rolled_back = rollback(&mut session, in_transaction, timeout).await;
                let _ = session.close().await;
                let cause = format!(
                    "change {} of {} failed: {}",
                    index + 1,
                    changes.len(),
                    error.message()
                );
                return Err(plan_failure(
                    engine_name,
                    in_transaction,
                    rolled_back,
                    applied,
                    changes.len(),
                    &cause,
                ));
            }
        };
        if let Some(actual) = affected {
            if let Some(expected) = change.expected {
                let actual = actual as i64;
                if verification_failed(change.keyed, actual, expected) {
                    let rolled_back = rollback(&mut session, in_transaction, timeout).await;
                    let _ = session.close().await;
                    let cause = format!(
                        "change {} of {} affected {actual} row(s) but the plan expected \
                         {expected} ({}{})",
                        index + 1,
                        changes.len(),
                        if change.keyed { "keyed" } else { "keyless" },
                        if change.keyed {
                            ", and a keyed write must not match more than the row it was \
                             built from"
                        } else {
                            ""
                        }
                    );
                    return Err(plan_failure(
                        engine_name,
                        in_transaction,
                        rolled_back,
                        // The statement completed — it just affected the wrong
                        // number of rows — so it counts among those that ran.
                        applied + 1,
                        changes.len(),
                        &cause,
                    ));
                }
            }
        }
        applied += 1;
        results.push(serde_json::json!({
            "index": index + 1,
            "affected": affected,
            "expected": change.expected,
        }));
        out.emit(event("progress").field("rows", applied).build())?;
    }

    if in_transaction {
        if let Err(error) = run(&mut session, "COMMIT", timeout).await {
            // The statements ran; whether the server kept them depends on where
            // the commit failed, and either way this is the `written` case: the
            // plan must not be run again blind.
            let _ = session.close().await;
            let cause = format!("COMMIT failed: {}", error.message());
            return Err(plan_failure(
                engine_name,
                in_transaction,
                false,
                applied,
                changes.len(),
                &cause,
            ));
        }
    }

    out.emit(
        event("done")
            .field("applied", applied)
            .field("statements", results)
            .field("transaction", in_transaction)
            // Committed (or auto-committed) by the time `done` is emitted, so the
            // plan is in the table whatever the transaction setting was.
            .field("disposition", Disposition::Written.as_str())
            .field("query_id", session.query_id())
            .build(),
    )?;
    let _ = session.close().await;
    Ok(())
}

/// Undo this command's own transaction. `true` when a rollback was issued and the
/// server accepted it; `false` when there was no transaction to undo, or the
/// rollback itself failed, in which case the rows may still be in the table.
async fn rollback(
    session: &mut Box<dyn Session>,
    in_transaction: bool,
    timeout: Option<Duration>,
) -> bool {
    in_transaction && run(session, "ROLLBACK", timeout).await.is_ok()
}

/// The error a failed plan returns: how far it got, the engine it ran on, the
/// disposition, and what the user should do next.
///
/// The advice is the study's, and it is deliberately not "try again": a `written`
/// failure means retrying writes twice, so the user is sent to the table instead.
fn plan_failure(
    engine: &str,
    in_transaction: bool,
    rolled_back: bool,
    applied: u64,
    total: usize,
    cause: &str,
) -> CliError {
    let disposition = if in_transaction && rolled_back {
        Disposition::PendingInSessionTransaction
    } else {
        Disposition::Written
    };
    let detail = match disposition {
        Disposition::PendingInSessionTransaction => {
            "the transaction was rolled back, so those rows are not in the table"
        }
        Disposition::Written if in_transaction => {
            "the transaction could not be undone cleanly, so those rows may still be in the table"
        }
        Disposition::Written => "there was no transaction to undo them",
    };
    let advice = match disposition {
        Disposition::PendingInSessionTransaction => {
            "Fix the failing statement and run the plan again when you are ready"
        }
        Disposition::Written => {
            "look at the table before doing anything else, not a retry: running the plan again \
             would write those rows a second time"
        }
    };
    CliError::Warned {
        message: format!(
            "the change plan on {engine}: {cause}. {applied} of {total} statement(s) had completed; \
             {detail} (disposition: {}). {advice}.",
            disposition.as_str()
        ),
        warnings: vec![],
    }
}

/// Read one statement to its end, returning the server's affected-row count.
async fn run(
    session: &mut Box<dyn Session>,
    statement: &str,
    timeout: Option<Duration>,
) -> Result<Option<u64>, CliError> {
    let mut cursor = session
        .execute(
            statement,
            &ExecuteOptions {
                statement_timeout: timeout,
                ..ExecuteOptions::default()
            },
        )
        .await?;
    while cursor.next_batch(1_000).await?.is_some() {}
    Ok(cursor.affected_rows())
}

/// Whether a server's affected-row count disagrees with what the plan expected.
///
/// A keyed write is matched by its key, so only matching **more** than the row
/// it was built from is a failure; `actual < expected` is allowed, because MySQL
/// reports zero for an update that sets a value it already held. A keyless write
/// is matched by every column, so both directions are real and `actual` must
/// equal `expected`.
fn verification_failed(keyed: bool, actual: i64, expected: i64) -> bool {
    if keyed {
        actual > expected
    } else {
        actual != expected
    }
}

/// The `CHANGES` JSON, or the reason it is not usable.
fn parse_changes(settings: &Settings) -> Result<Vec<Change>, CliError> {
    let raw = settings.raw("CHANGES", "");
    if raw.trim().is_empty() {
        return Err(CliError::Usage(
            "CHANGES is required: the JSON plan of statements to apply".to_owned(),
        ));
    }
    let parsed: Json = serde_json::from_str(&raw)
        .map_err(|error| CliError::Usage(format!("CHANGES is not JSON: {error}")))?;
    let rows = parsed
        .as_array()
        .ok_or_else(|| CliError::Usage("CHANGES must be a JSON array".to_owned()))?;
    let mut changes = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let object = row
            .as_object()
            .ok_or_else(|| CliError::Usage(format!("CHANGES[{index}] is not an object")))?;
        let sql = object
            .get("sql")
            .and_then(Json::as_str)
            .filter(|sql| !sql.trim().is_empty())
            .ok_or_else(|| CliError::Usage(format!("CHANGES[{index}] has no sql")))?;
        let expected = match object.get("expected") {
            None | Some(Json::Null) => None,
            Some(value) => Some(value.as_i64().ok_or_else(|| {
                CliError::Usage(format!("CHANGES[{index}].expected is not an integer"))
            })?),
        };
        let keyed = object.get("keyed").and_then(Json::as_bool).unwrap_or(true);
        changes.push(Change {
            sql: sql.to_owned(),
            expected,
            keyed,
        });
    }
    Ok(changes)
}

/// The safe mode is re-checked against each statement before it runs; this is
/// used only by the tests that build a plan directly.
#[allow(dead_code)]
fn allows(mode: SafeMode, sql: &str) -> bool {
    qh_sql::check(mode, sql).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::Settings;

    fn settings(pairs: &[(&str, &str)]) -> Settings {
        Settings::from_pairs(pairs.iter().map(|(k, v)| (*k, *v)))
    }

    #[test]
    fn a_plan_is_read_with_its_expected_counts() {
        let json = r#"[{"sql":"UPDATE t SET a=1 WHERE id=2","expected":1,"keyed":true},
                       {"sql":"DELETE FROM t WHERE id=3","expected":1},
                       {"sql":"INSERT INTO t (a) VALUES (4)"}]"#;
        let changes = parse_changes(&settings(&[("CHANGES", json)])).expect("a plan");
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].expected, Some(1));
        assert!(changes[0].keyed);
        assert_eq!(changes[1].expected, Some(1));
        // `keyed` defaults to true, which is the PK case this app cannot prove
        // and therefore the caller has to say is false when it is.
        assert!(changes[1].keyed);
        assert_eq!(changes[2].expected, None);
    }

    #[test]
    fn an_unusable_plan_is_refused_by_name() {
        assert!(parse_changes(&settings(&[])).is_err());
        assert!(parse_changes(&settings(&[("CHANGES", "not json")])).is_err());
        assert!(parse_changes(&settings(&[("CHANGES", "{}")])).is_err());
        assert!(parse_changes(&settings(&[("CHANGES", r#"[{"expected":1}]"#)])).is_err());
        assert!(parse_changes(&settings(&[(
            "CHANGES",
            r#"[{"sql":"SELECT 1","expected":"x"}]"#
        )]))
        .is_err());
    }

    #[test]
    fn the_verification_rule_is_not_the_same_for_keyed_and_keyless() {
        // Each row: keyed, actual affected rows, expected rows, is it a failure?
        let cases = [
            // Keyed: only matching more than the one row is a failure; MySQL's
            // zero for a no-op update is a save that worked.
            (true, 1, 1, false),
            (true, 2, 1, true),
            (true, 0, 1, false),
            // Keyless: both directions matter.
            (false, 1, 1, false),
            (false, 0, 1, true),
            (false, 2, 1, true),
        ];
        for (keyed, actual, expected, failed) in cases {
            assert_eq!(
                verification_failed(keyed, actual, expected),
                failed,
                "keyed={keyed} actual={actual} expected={expected}"
            );
        }
    }

    #[test]
    fn the_guard_refuses_a_write_on_a_read_only_connection() {
        assert!(!allows(SafeMode::ReadOnly, "UPDATE t SET a=1"));
        assert!(allows(SafeMode::ReadOnly, "SELECT 1"));
    }

    #[test]
    fn a_match_note_comment_does_not_change_how_a_statement_is_classified() {
        // The plan puts the columns it could not match in a trailing comment so
        // the review sheet shows them; the guard must still see an `UPDATE`.
        let sql = "UPDATE t SET a = 1 WHERE \"x\" = 1 /* not matched: \"geom\" (spatial values \
                   are not comparable as text) */";
        assert!(
            allows(SafeMode::Full, sql),
            "a full connection allows the write"
        );
        assert!(
            !allows(SafeMode::ReadOnly, sql),
            "a read-only one refuses it"
        );
    }

    #[test]
    fn a_transactional_failure_is_pending_and_a_bare_one_is_written() {
        let pending = plan_failure("postgres", true, true, 1, 3, "change 2 of 3 failed");
        let message = pending.message();
        assert!(message.contains("postgres"), "{message}");
        assert!(
            message.contains("(disposition: pendingInSessionTransaction)"),
            "{message}"
        );
        assert!(
            message.contains("those rows are not in the table"),
            "{message}"
        );
        assert!(message.contains("run the plan again"), "{message}");

        let written = plan_failure("trino", false, false, 1, 3, "change 2 of 3 failed");
        let message = written.message();
        assert!(message.contains("trino"), "{message}");
        assert!(message.contains("(disposition: written)"), "{message}");
        assert!(
            message.contains("there was no transaction to undo them"),
            "{message}"
        );
        assert!(message.contains("not a retry"), "{message}");
        assert!(message.contains("look at the table"), "{message}");
    }

    #[test]
    fn a_rollback_that_failed_is_written_not_pending() {
        // The distinction the study exists for: a rollback that did not take
        // effect leaves the rows where they are, so this is not the safe case.
        let error = plan_failure("postgres", true, false, 2, 4, "change 3 of 4 failed");
        let message = error.message();
        assert!(message.contains("(disposition: written)"), "{message}");
        assert!(message.contains("could not be undone cleanly"), "{message}");
        assert!(message.contains("may still be in the table"), "{message}");
    }
}
