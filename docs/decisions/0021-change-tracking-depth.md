# 0021 — Change tracking depth: a keyless match policy, a partial-commit vocabulary, a batch budget, and DEFAULT columns

- **Status:** Accepted
- **Date:** 29 Sep 2026 (Phase 5.3 follow-up)
- **Instruction context:** `docs/architecture/tablepro-source-study.md` §2;
  `docs/decisions/0020-apply-changes-in-engine.md`;
  `docs/architecture/tablepro-adoption-plan.md` §7 (5.3)

> This ADR is written in English by instruction, while the ADRs around it are in
> Indonesian. The structure deliberately follows 0020; only the language differs.

## Context

ADR-0020 made a reviewed plan of `DELETE`/`UPDATE`/`INSERT` run inside the engine,
in one transaction, with each statement's affected-row count checked against what
the plan expected. The source study §2 names four things TablePro does that the
plan still did not, and this ADR is those four:

1. **A keyless match policy.** The predicate currently matches *every* original
   column. A keyless `UPDATE` on a table with a `FLOAT` or `JSON` column therefore
   fails to match anything and the plan rolls back — the study's MySQL
   `FLOAT`/`JSON` bug. TablePro answers it with `RowMatchPolicy`: columns that
   cannot be compared safely are excluded, and columns whose grid text is the
   server's own rendering are compared through that rendering (`CONCAT(col)` on
   MySQL).
2. **A partial-commit vocabulary.** A failed plan currently reports the mismatch
   and rolls back. The study's `DataWritePartialCommitError` distinguishes what is
   already in the table (`written`) from what is not, because a blind retry of the
   first writes twice.
3. **A batching budget.** The study's `SQLWriteBatchBudget` bounds a batch by rows,
   bytes and the engine's bind-parameter ceiling at once, and closes a batch
   *before* the row that would cross a bound.
4. **Server-computed and `DEFAULT` columns.** The study records that a generator
   which returned nothing for a row whose every column is server-computed made the
   row vanish while the rest of the batch committed and reported success.

Two limits in this tree bind the work, and are stated rather than worked around:

- The app carries **no primary-key metadata**, so the match policy cannot lean on
  a key and has to say what it cannot compare. ADR-0020 already accepted this.
- The app carries **no generated-column or default metadata** either, so
  "server-computed" cannot be read off the schema here. What it can know is what
  the user *provided*: an inserted cell the user never filled is the signal that
  the server should fill it.

## Options considered

| Decision | Option | Good | Bad |
|---|---|---|---|
| Unmatchable columns | **Exclude and name them** | The predicate matches what it can; the plan says what it dropped | The predicate is weaker, so it can match more than one row |
| | Keep every column | One rule, no exclusions | A `FLOAT`/`JSON`/`BLOB` column makes the whole `UPDATE` match nothing |
| Column with unstable text | **Compare the server's rendering** | The grid text is what is compared, so it matches | One more per-driver spelling to keep |
| | Compare the fetched literal directly | Simple | Fails for `FLOAT`/`JSON`, which is the bug |
| No comparable column at all | **Refuse the statement and say so** | Never widens to `DELETE FROM t` | A change the user staged is not written (visibly) |
| | Drop the `WHERE` | The write happens | It writes every row; the count check catches it only inside a transaction |
| Batching | **Rows + bytes + parameters, close before crossing** | One bound does not exhaust another; keeps a large value from blowing a batch | Three numbers to justify |
| | Rows only | Simpler | A row with a big `TEXT`/`JSON` sends an unbounded request |
| Parameter binding | **Not now** | The plan shape is ready for it | The axis is counted but unused |
| | Implement it now | One pass | Trino's HTTP protocol has no parameters; it is a driver-trait decision, not a plan decision |

## Decision

**The match policy is explicit and named; a failed plan carries a disposition that
names the engine; added rows are batched on three axes at once; and a row the
server fills in entirely still gets a statement instead of disappearing.**

1. **Keyless match policy.** `app/Sources/TrinoExporter/Models/MatchPolicy.swift`
   gives every column one of three rules, from the result's own type name:

   - `match` — compare directly (`col = literal`, `col IS NULL`).
   - `serverText` — compare the server's rendering: `CONCAT(col)` on MySQL (the
     study's spelling), `col::text` on PostgreSQL, `CAST(col AS varchar)` on Trino.
     `FLOAT`/`REAL`/`DOUBLE` and `JSON`/`JSONB`.
   - `excluded(reason)` — binary (`bytea`, `blob`, …), spatial (`geometry`, …) and
     nested (`array`, `map`, `row`, …) values, which cannot be compared as text.

   The rule is enforced in `UpdateStatements.match`, the only place a fetched row's
   `WHERE` is built. Excluded columns are **named**, both in a trailing SQL comment
   on the reviewed statement and in `WritePlan.warnings`. A row whose every column
   is excluded yields **no predicate and no statement** — a `WritePlan.warnings`
   entry instead — because the alternative is a bare `DELETE FROM t` / `UPDATE t
   SET …` that matches the whole table. The engine's keyless count check
   (`actual != expected`) remains the backstop for a predicate weakened by one
   exclusion: a row that matches a duplicate fails and rolls back.

2. **Partial-commit vocabulary.** `crates/qh-ffi/src/apply.rs` gains a
   `Disposition` with two values, used whenever a statement fails, a count
   disagrees, a `COMMIT` fails, or a run is cancelled:

   - `written` — no transaction was open, or the transaction could not be undone
     cleanly. The rows are (or may be) in the table.
   - `pendingInSessionTransaction` — the plan ran inside a transaction and did not
     commit; the engine rolled it back, so none of it is in the table.

   The message names the **engine** (`postgres`/`trino`/`mysql`), how many of how
   many statements had run, the disposition, and the next step. `written` sends the
   user to the table and explicitly not to a retry; `pendingInSessionTransaction`
   says the plan is theirs to fix and run again. On success, `done` now reports
   `disposition: written`, because by then the commit has run.

3. **Batching budget.** `app/Sources/TrinoExporter/Models/WriteBatchBudget.swift`
   holds `maxRows`, `maxBytes` and `maxParameters`, and
   `InsertStatements.build` closes a batch **before** the row that would cross any
   of them. The numbers, per driver:

   | Axis | Value | Why |
   |---|---|---|
   | rows | 1.000 | the engine already reads in 1.000-row batches |
   | bytes | 1 MiB (1.048.576) | bounds one request independently of the row count |
   | parameters | 65.535 | PostgreSQL's and MySQL's own statement ceiling; unbounded on Trino, whose HTTP protocol has none |

   Rows only share a multi-row `INSERT` when they write the same column set — the
   study's grouping by column set; a batch is per column set, so one column list is
   never asked to describe two shapes.

4. **Server-computed and `DEFAULT` columns.** `InsertStatements.build`:
   - A column the user **did not provide** is left out of the `INSERT`, so the
     server takes its default or generates it. Writing `NULL` over it either fails
     on a `NOT NULL` key or silently overrides a default.
   - A cell whose whole text is `DEFAULT` is the **sentinel**, rendered as the
     keyword and never quoted, in both `INSERT` and `UPDATE`.
   - A row whose every column is the server's to compute still produces a statement:
     `INSERT INTO t DEFAULT VALUES` (PostgreSQL) or `INSERT INTO t () VALUES ()`
     (MySQL). Trino has no such form, and there the row is reported in
     `WritePlan.warnings` and **not** silently dropped — which is exactly the bug
     the study records.

5. **Parameter binding is deliberately not implemented.** The batch budget counts
   the parameter axis so its shape survives, but the plan still writes literals.
   Trino's HTTP protocol has no bind parameters, so whether to bind at all is a
   driver-trait decision (`Capabilities`), not a change to this plan. It belongs to
   another slice.

## Reasons

1. **A predicate that matches nothing looks like a save that worked.** That is the
   study's bug and the reason the match policy is explicit: an edit that vanished
   after a reload is worse than a refusal.
2. **The plan has to say what it cannot do.** A narrower predicate can match more
   rows; an all-excluded row can match the table. Naming the dropped columns and
   refusing the empty predicate keeps "the plan did less than you asked" visible at
   review time, with the count check as the runtime backstop.
3. **`written` and `pending` are different actions for the user.** A retry after
   `written` doubles the rows; a retry after `pending` is safe. A single "failed"
   message cannot tell them apart, so the disposition is part of the error.
4. **One bound is not a budget.** Rows, bytes and parameters fail independently;
   the study measured all three, and the cost of carrying the third is a number.
5. **A default row that disappears is a data-loss bug that reports success.** The
   per-engine spell keeps the row in the plan, and where no spell exists the plan
   says so instead of committing a prefix and calling it done.

## Consequences

- **`WriteStatement` and `WritePlan` grew.** `WriteStatement.unmatchedColumns` and
  `WritePlan.warnings` carry what the plan could not match or plan; `WritePlan`
  gains an explicit initializer so the existing call sites still compile. The
  `CHANGES` payload is unchanged: these are review-time facts, not engine inputs.
- **`apply_changes` emits `disposition: written` on success**, where it used to say
  `pending` even after a `COMMIT`. Nothing reads the key yet, so this is a
  correction, not a contract change.
- **What is verified and what is not.** The four behaviours are covered by unit and
  integration tests: `MatchPolicyTests`, `InsertStatementsTests`,
  `WriteBatchBudgetTests` (Swift), `apply_changes.rs` and the `apply` module tests
  (Rust). No new live-server test was added; the podman machine is gone.
- **Limits, stated rather than hidden:**
  - There is no primary-key metadata, so every statement is keyless and a table
    with two identical rows still cannot distinguish them. Unchanged from ADR-0020.
  - There is no generated-column or default metadata, so "server-computed" is
    inferred from what the user provided plus the `DEFAULT` sentinel. A column with
    a value the user typed is always written.
  - A literal string that is exactly the word `DEFAULT` cannot be written through
    the grid yet, because the sentinel claims it. A UI escape (or a distinct null
    toggle, which would also make explicit `NULL` expressible) is not built.
  - On Trino, an omitted column becomes `NULL` rather than a default (Trino fills
    unspecified columns with `NULL` and has no `DEFAULT` keyword in `INSERT`), so
    the all-default spell is unavailable there and is reported.
  - The review sheet (`Views/ResultGrid.swift`) still renders `plan.sql` only; the
    match note is in the SQL, and `WritePlan.warnings` are logged by
    `AppModel.applyChanges`. Rendering the warnings in the sheet is deferred because
    that file is owned by another slice this session.
