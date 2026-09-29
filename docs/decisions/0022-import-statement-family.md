# 0022 — The statement family of import, and foreign keys switched back on

- **Status:** Accepted
- **Date:** 29 Sep 2026 (Fase 5.1, second slice)
- **Instruction context:** `docs/architecture/tablepro-adoption-plan.md` §7 (5.1);
  `docs/architecture/tablepro-source-study.md` §3

## Context

ADR-0019 reasoned about `import_data` and recorded what it deliberately left
undone: the **row** family landed (CSV and XLSX into a named table), the
**statement** family did not. The source study's most valuable design decision is
exactly that split — a plugin carries `requiresTargetTable`, and the two answers
are two families with one runner, one set of error modes and one transaction
policy. A `.sql` file belongs to the family that brings its own statements.

Two boundaries in this tree bind the choice. The engine has three callers (the
app, the CLI, the MCP server), so the split has to live in `qh-ffi` where all
three reach it, not in a Swift sheet. And `qh-sql` already owns the one correct
way to find the end of a statement — `scan` knows that a `;` inside a literal, a
comment or a Postgres dollar-quoted body is text — so a second splitter in the
import path is the failure mode to avoid, not the implementation.

The study names one thing in this area that is easy to leave half-done: foreign-key
checks are turned off for a bulk load and must be turned **on again on both exits**,
the commit and the rollback.

## Options considered

| Decision | Option | Pro | Con |
|---|---|---|---|
| Surface | **Extend `import_data` with a source kind** | Invariant #11 untouched; `.csv` and `.sql` fail the same way | The `done` shape branches by family |
| | A second command `import_sql` | Two crisp commands | A new command means the four hand-kept lists; more surface for one shared policy |
| Splitter | **`qh_sql::statements_with_lines`** | Same scanner as `check` and the classifier | None; it is the existing scanner with a line number |
| `GO` | **Not a separator** | One splitter, no client-command grammar | A `GO` file fails at the server |
| FK default | **`FOREIGN_KEYS=on`** | No privilege needed, no silent orphan rows | Bulk loads that need checks off must ask |
| | `off` by default | Matches TablePro's bulk-load behaviour | `session_replication_role` needs a superuser on PostgreSQL |
| FK restore | **A single `finish` for both exits** | The epilogue cannot be forgotten on one path | — |

## Decision

**`import_data` gains a statement family, chosen from `IMPORT_FORMAT` or the
file's extension, and both families run under one `ON_ERROR` policy and one
transaction policy. The splitter is `qh-sql`'s. `FOREIGN_KEYS=off` turns the
server's checks off for the import and back on at commit and at rollback alike.**

Binding details:

1. **No new command.** `IMPORT_FORMAT=sql`, or a `.sql` extension, selects the
   statement family; `TARGET_TABLE`, `COLUMNS`, the delimiter and the sheet are
   ignored. The four lists of invariant #11 are untouched and the FFI surface is
   unchanged — the statement family is a source kind, not a command.
2. **One splitter.** Statements come from `qh_sql::statements_with_lines`, which
   shares its scan with `qh_sql::statements` and `qh_sql::check`. A statement is
   sent on its own, guarded against Safe Mode a second time at send (the row
   family guards its `INSERT` the same way). If a `;` splits differently here than
   in the SQL editor, that is a bug in one scanner, not a disagreement between
   two.
3. **No `GO`.** `GO` is a client command in SQL Server and not a statement in
   PostgreSQL, MySQL or Trino. Honouring it would be a second scanner with its own
   rules; a `GO` line is sent as part of its statement and the server refuses it.
4. **Same policy as rows.** `stop` (default) rolls back, `commit` keeps the prefix
   and reports where it stopped, `skip` skips the one bad statement and uses no
   transaction — the reasons are ADR-0019's and are not repeated. A driver without
   transactions reports `disposition: "written"` instead of claiming a rollback.
5. **Line numbers and the error cap.** Every message names the statement's 1-based
   line, from the same scan that split it, and the `MAX_ERRORS` cap of 1.000
   truncates a file that is wrong everywhere. Both already existed for rows; this
   is the statement family matching.
6. **Foreign keys.** `FOREIGN_KEYS` is `on` by default, which leaves the server
   alone. `off` runs a prologue before `BEGIN` and an epilogue after the
   transaction ends, from a single `finish` called on the error path, the commit
   path and the rollback path. PostgreSQL is `SET session_replication_role =
   replica` / `DEFAULT`; MySQL is `FOREIGN_KEY_CHECKS = 0` / `1`; Trino has no
   switch and `FOREIGN_KEYS=off` is refused by name before a connection opens.
   `done` reports `foreign_keys`.
7. **The statement `done` shape.** `statements` (the count applied), `format:
   "sql"`, `streams: false` (the file is read whole because splitting needs it
   whole), plus the same `mode`, `transaction`, `disposition`, `errors`,
   `errors_truncated`, `stopped_at`, `cancelled`, `foreign_keys` and `query_id`
   keys the row family has. It omits `rows` and `rejected`, which have no meaning
   for a script; an absent key is not a null, and the app's decoder is not built
   for either family yet.

## Reasons

1. **One policy, two families is the study's own lesson.** The interesting part is
   not the plugin boundary but that the runner owns the transaction and the modes,
   and both families get it. A second command would have duplicated the policy or
   split it.
2. **A second splitter is a correctness bug waiting to happen.** `qh-sql` exists
   because a naive `rstrip(";")` deletes a character the user typed. A `.sql`
   import that split with `split(';')` would hit the same defect on a dollar-quoted
   function body or a `';'` in a value.
3. **The line is the only handle on a big file.** "Statement 8,412 failed" is what
   makes a ten-thousand-statement load fixable; the row family already knows this.
4. **The restore is the part that is easy to leave half-done.** The study calls it
   out, and the failure is silent: an import that turns checks off and returns on
   the error path leaves the session unguarded. Funnelling every exit through one
   `finish` is the fix, and both exits are tested.
5. **Defaulting `off` would change behaviour and need a superuser.** On PostgreSQL
   the switch is `session_replication_role`, which needs a superuser or `SET`
   privilege, and it disables more than foreign keys (every trigger and rule).
   Silently permitting orphan rows is not a default to pick for someone. `off` is
   a request the caller makes.

## Consequences

- **Surface unchanged.** `import_data` remains the twenty-first command; no list of
  invariant #11 moves, and `./app/build-ffi.sh` was not needed because no exported
  signature changed. The `done` event gained keys (`statements`, `foreign_keys`)
  and, for `.sql` files, reports `format: "sql"`; the event protocol is additive
  per the MCP stability note, and no frozen golden case covers `import_data`.
- **`qh-sql` grew one public function.** `statements_with_lines` and the
  `ScriptStatement` value, with `statements` now delegating to them so the two
  cannot drift.
- **A live PostgreSQL proves the switch, not just its order.** A child row loads
  before its parent with `FOREIGN_KEYS=off`, and the checks are enforced again
  after both a commit and a rollback; `crates/qh-ffi/tests/import_live.rs` is that
  proof, gated on `QH_TEST_POSTGRES=1`.
- **Still not built.** The app's import mapping sheet; MySQL is not tested live
  for this (no MySQL container in this session), so its `FOREIGN_KEY_CHECKS`
  spelling is pinned by unit test and code review only. Trino's refusal is a unit
  test; it needs no server.
