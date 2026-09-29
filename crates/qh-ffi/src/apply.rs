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
            if in_transaction {
                let _ = run(&mut session, "ROLLBACK", timeout).await;
            }
            let _ = session.close().await;
            return Err(CliError::Usage(
                "the change plan was cancelled before it finished".to_owned(),
            ));
        }
        let affected = match run(&mut session, &change.sql, timeout).await {
            Ok(affected) => affected,
            Err(error) => {
                if in_transaction {
                    let _ = run(&mut session, "ROLLBACK", timeout).await;
                }
                let _ = session.close().await;
                return Err(CliError::Warned {
                    message: format!(
                        "change {} of {} failed and the plan was rolled back: {}",
                        index + 1,
                        changes.len(),
                        error.message()
                    ),
                    warnings: vec![],
                });
            }
        };
        if let Some(actual) = affected {
            if let Some(expected) = change.expected {
                let actual = actual as i64;
                let mismatch = verification_failed(change.keyed, actual, expected);
                if mismatch {
                    if in_transaction {
                        let _ = run(&mut session, "ROLLBACK", timeout).await;
                    }
                    let _ = session.close().await;
                    return Err(CliError::Warned {
                        message: format!(
                            "change {} of {} affected {actual} row(s) but the plan expected \
                             {expected} ({}{}); the plan was rolled back",
                            index + 1,
                            changes.len(),
                            if change.keyed { "keyed" } else { "keyless" },
                            if change.keyed {
                                ", and a keyed write must not match more than the row it was \
                                 built from"
                            } else {
                                ""
                            }
                        ),
                        warnings: vec![],
                    });
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
        run(&mut session, "COMMIT", timeout).await?;
    }

    out.emit(
        event("done")
            .field("applied", applied)
            .field("statements", results)
            .field("transaction", in_transaction)
            .field(
                "disposition",
                if in_transaction { "pending" } else { "written" },
            )
            .field("query_id", session.query_id())
            .build(),
    )?;
    let _ = session.close().await;
    Ok(())
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
}
