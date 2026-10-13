//! The table swap behind `to_table`'s `replace` mode: build the new table beside the old
//! one, then move it into place, so a failure or a Stop never costs the user the table
//! they had (PF-1). Moved out of `commands.rs` without a change of behaviour; the plan is
//! made from the names alone, and the statements run on a session `commands::to_table`
//! already opened.

use std::time::Duration;

use qh_core::Value;
use qh_driver::{DriverKind, ExecuteOptions, Session};

use crate::commands::{run_stoppable, Built};
use crate::sql_ident::{qualified, reference, SlotStyle};
use crate::{CancelFlag, CliError};

/// What a `replace` sends: a new table built beside the old one, then swapped in.
///
/// The old table is touched only after the new one is complete, so a SELECT that fails,
/// a Stop in the middle of the build or a name that is already taken leaves the user with
/// the table they had. The swap is the strongest the driver offers: one transaction on
/// PostgreSQL (its DDL is transactional), one atomic `RENAME TABLE` on MySQL, and on Trino
/// two renames with a third that puts the old table back if the second fails. The old table
/// is dropped last, and a drop that fails is a warning that names the table it kept.
pub(crate) struct Replacement {
    /// The table the rows are built into, and where the old one waits, as a user reads them.
    scratch_ref: String,
    backup_ref: String,
    build: String,
    /// A one-row, one-column count of the target table, so the swap knows what to do.
    probe: String,
    drop_scratch: String,
    /// The swap when there is no old table to move aside.
    install: Vec<String>,
    /// The swap when there is one.
    swap: Vec<String>,
    /// Puts the old table back when `swap` stops after moving it aside.
    undo: Option<String>,
    /// Drops the old table, once the new one is in place.
    drop_old: Option<String>,
    /// The swap is one transaction, so a failure is a `ROLLBACK`.
    atomic: bool,
}

impl Replacement {
    pub(crate) fn plan(
        style: SlotStyle,
        catalog: &str,
        schema: &str,
        table: &str,
        body: &str,
    ) -> Self {
        // One stamp names both temporary tables: a collision is an error from the server
        // (neither `CREATE` nor `RENAME` overwrites), never a silent reuse.
        let stamp = scratch_stamp();
        let (scratch_name, backup_name) = (format!("qh_new_{stamp}"), format!("qh_old_{stamp}"));
        let at = |name: &str| qualified(style, catalog, schema, name);
        let plain = |name: &str| reference(style, catalog, schema, name);
        let (target, scratch, backup) = (at(table), at(&scratch_name), at(&backup_name));
        let literal = |text: &str| sql_literal(style.kind, text);
        let probe = match style.kind {
            DriverKind::Postgres => format!(
                "SELECT COUNT(*) FROM information_schema.tables \
                 WHERE table_schema = {} AND table_name = {}",
                literal(schema),
                literal(table)
            ),
            DriverKind::Mysql => format!(
                "SELECT COUNT(*) FROM information_schema.tables \
                 WHERE table_schema = {} AND table_name = {}",
                literal(catalog),
                literal(table)
            ),
            DriverKind::Trino => format!(
                "SELECT COUNT(*) FROM {}.information_schema.tables \
                 WHERE table_schema = {} AND table_name = {}",
                style.quote(catalog),
                literal(schema),
                literal(table)
            ),
        };
        let mut plan = Self {
            build: format!("CREATE TABLE {scratch} AS {body}"),
            drop_scratch: format!("DROP TABLE IF EXISTS {scratch}"),
            scratch_ref: plain(&scratch_name),
            backup_ref: plain(&backup_name),
            probe,
            install: Vec::new(),
            swap: Vec::new(),
            undo: None,
            drop_old: None,
            atomic: false,
        };
        match style.kind {
            DriverKind::Postgres => {
                let moved = vec![
                    "BEGIN".to_owned(),
                    format!("DROP TABLE IF EXISTS {target}"),
                    format!("ALTER TABLE {scratch} RENAME TO {}", style.quote(table)),
                    "COMMIT".to_owned(),
                ];
                plan.install = moved.clone();
                plan.swap = moved;
                plan.atomic = true;
            }
            DriverKind::Mysql => {
                plan.install = vec![format!("RENAME TABLE {scratch} TO {target}")];
                plan.swap = vec![format!(
                    "RENAME TABLE {target} TO {backup}, {scratch} TO {target}"
                )];
                plan.drop_old = Some(format!("DROP TABLE {backup}"));
            }
            DriverKind::Trino => {
                plan.install = vec![format!("ALTER TABLE {scratch} RENAME TO {target}")];
                plan.swap = vec![
                    format!("ALTER TABLE {target} RENAME TO {backup}"),
                    format!("ALTER TABLE {scratch} RENAME TO {target}"),
                ];
                plan.undo = Some(format!("ALTER TABLE {backup} RENAME TO {target}"));
                plan.drop_old = Some(format!("DROP TABLE {backup}"));
            }
        }
        plan
    }

    /// Every statement the plan can send, for the Safe Mode to read before the connect.
    /// The probe is a read and is not among them.
    pub(crate) fn statements(&self) -> Vec<String> {
        let mut all = vec![self.build.clone()];
        all.extend(self.swap.iter().cloned());
        if !self.atomic {
            all.extend(self.install.iter().cloned());
        }
        all.extend(self.undo.clone());
        all.extend(self.drop_old.clone());
        all.push(self.drop_scratch.clone());
        all
    }
}

/// Unique enough for one process and one moment: the clock and a counter, in hex.
fn scratch_stamp() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    // 40 bits of clock and 8 of counter keep both names short of every identifier limit.
    format!(
        "{:010x}{:02x}",
        (nanos as u64) & 0xff_ffff_ffff,
        counter & 0xff
    )
}

/// A string as a SQL literal; MySQL also reads a backslash as an escape.
fn sql_literal(kind: DriverKind, text: &str) -> String {
    let text = if kind == DriverKind::Mysql {
        text.replace('\\', "\\\\")
    } else {
        text.to_owned()
    };
    format!("'{}'", text.replace('\'', "''"))
}

/// Whether the one value a `COUNT(*)` probe returned is more than zero. A shape this does
/// not know reads as "there is a table": the swap then fails on a name that is free, which
/// costs the run and nothing else, where the opposite guess could replace a table unwarned.
fn counts_a_table(value: &Value) -> bool {
    match value {
        Value::Int(count) => *count > 0,
        Value::UInt(count) => *count > 0,
        Value::Decimal { unscaled, .. } => *unscaled > 0,
        Value::Text(count) => count.trim().parse::<u64>().map_or(true, |count| count > 0),
        _ => true,
    }
}

/// Read one statement to its end, returning the server's affected-row count.
async fn finish_statement(
    session: &mut Box<dyn Session>,
    sql: &str,
    timeout: Option<Duration>,
) -> Result<Option<u64>, CliError> {
    let mut cursor = session
        .execute(
            sql,
            &ExecuteOptions {
                statement_timeout: timeout,
                ..ExecuteOptions::default()
            },
        )
        .await?;
    // A DDL statement has no rows to read, but it is not finished when `execute` returns:
    // driving the cursor to its end is what waits for the server, and what the count
    // arrives with.
    while cursor.next_batch(1_000).await?.is_some() {}
    Ok(cursor.affected_rows())
}

/// Whether the probe found the target table.
async fn table_exists(
    session: &mut Box<dyn Session>,
    probe: &str,
    timeout: Option<Duration>,
) -> Result<bool, CliError> {
    let mut cursor = session
        .execute(
            probe,
            &ExecuteOptions {
                statement_timeout: timeout,
                ..ExecuteOptions::default()
            },
        )
        .await?;
    let mut found = false;
    while let Some(batch) = cursor.next_batch(1_000).await? {
        found |= batch
            .columns()
            .first()
            .and_then(|column| column.first())
            .is_some_and(counts_a_table);
    }
    Ok(found)
}

/// Drop the scratch table, or say that it is still there.
async fn discard_scratch(
    session: &mut Box<dyn Session>,
    plan: &Replacement,
    timeout: Option<Duration>,
    reusable: bool,
    warnings: &mut Vec<String>,
) {
    let outcome = if reusable {
        finish_statement(session, &plan.drop_scratch, timeout)
            .await
            .map(|_| ())
    } else {
        Err(CliError::Query(
            "the server did not release the connection".to_owned(),
        ))
    };
    if let Err(error) = outcome {
        warnings.push(format!(
            "the temporary table {} was left behind: {}",
            plan.scratch_ref,
            error.message()
        ));
    }
}

/// `replace`: build, then swap. Returns whether a Stop ended it. Every way out says what
/// became of the existing table, in `warnings`, which travel with an error too.
pub(crate) async fn replace_table(
    session: &mut Box<dyn Session>,
    plan: &Replacement,
    name: &str,
    timeout: Option<Duration>,
    cancel: &CancelFlag,
    affected: &mut Option<u64>,
    warnings: &mut Vec<String>,
) -> Result<bool, CliError> {
    let untouched = format!("the existing table {name} was left as it was");

    match run_stoppable(session, &plan.build, timeout, cancel).await {
        Ok(Built::Finished(count)) => *affected = count,
        Ok(Built::Stopped { reusable }) => {
            discard_scratch(session, plan, timeout, reusable, warnings).await;
            warnings.push(untouched);
            return Ok(true);
        }
        Err(error) => {
            discard_scratch(session, plan, timeout, true, warnings).await;
            warnings.push(untouched);
            return Err(error);
        }
    }
    // The build is done and the swap is a few metadata statements: a Stop that arrived
    // meanwhile discards the build, and a Stop after this point lets the swap finish.
    if cancel.is_cancelled() {
        discard_scratch(session, plan, timeout, true, warnings).await;
        warnings.push(untouched);
        return Ok(true);
    }
    let existed = match table_exists(session, &plan.probe, timeout).await {
        Ok(existed) => existed,
        Err(error) => {
            discard_scratch(session, plan, timeout, true, warnings).await;
            warnings.push(untouched);
            return Err(error);
        }
    };

    // Said before the swap runs, so it travels with any failure that comes after it, and
    // is taken back only where the failure is known to have changed nothing.
    let notice = format!("replaced the existing table {name}");
    if existed {
        warnings.push(notice.clone());
    }
    let steps = if existed { &plan.swap } else { &plan.install };
    for (index, step) in steps.iter().enumerate() {
        let Err(error) = finish_statement(session, step, timeout).await else {
            continue;
        };
        let mut restored = true;
        if plan.atomic {
            let _ = finish_statement(session, "ROLLBACK", timeout).await;
        } else if index > 0 {
            if let Some(undo) = &plan.undo {
                restored = finish_statement(session, undo, timeout).await.is_ok();
            }
        }
        if restored {
            warnings.retain(|warning| *warning != notice);
            discard_scratch(session, plan, timeout, true, warnings).await;
            warnings.push(untouched);
        } else {
            warnings.push(format!(
                "the existing table {name} is now {} and could not be renamed back; \
                 this run's rows are in {}",
                plan.backup_ref, plan.scratch_ref
            ));
        }
        return Err(error);
    }

    if existed {
        if let Some(drop_old) = &plan.drop_old {
            if let Err(error) = finish_statement(session, drop_old, timeout).await {
                warnings.push(format!(
                    "the new table is in place, and the old one is kept as {}: {}",
                    plan.backup_ref,
                    error.message()
                ));
            }
        }
    }
    Ok(false)
}
