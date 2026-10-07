# QueryHive agent guide

QueryHive is a macOS database client: a SwiftUI app (`app/`) over a Rust engine (`crates/`) bridged by UniFFI (`app/Generated/`). This guide governs any AI agent working here. It is the single source of truth for how work is done; the plans below say what work is done.

## Current work: the perf-parity run

Branch `work/perf-parity` carries a long program to beat TablePro on performance, then close design and feature gaps. It was paused on 2026-10-07 after batches B8 and B8-extra, for another AI to continue. Everything through `1708700` is on `main` and `work/perf-parity` with full gates green; eight `lane/*` branches hold work not yet integrated, and no worktrees remain.

Resume in this order:

1. Read `docs/handoff.md` (the working method: lanes, SSD targets, live lock, gates, known traps, integration order).
2. Read `target/run/ledger.md` "PAUSE POINT 5" (the latest): the status of each lane branch (`lane/w13-t14`, `lane/w12-t11`, `lane/w13-t16`, `lane/w10-t7b`, `lane/w10-t3`, `lane/w12-t7a`, `lane/w12-t9`, `lane/w8-f2`, plus `lane/w7-t1` kept for w8-f2), what each still needs, and the owner-bound items. Owner decisions O-14 to O-31b and the lane plan O-22 are in the same ledger.
3. Use `docs/architecture/development-plan.md` for task IDs, file ownership (§7), gates (§1) and the definition of done (§9); `target/run/debt-register.md` and `target/run/dbx-adoption-backlog.md` list every debt item and the task that owns it. Then continue through W14 per §5, in parallel lanes where §7 allows.

Review policy in force (owner decision O-20): implementation, docs, bench and cleanup run on cheaper models; only review uses the strongest model, for **one** round. After a blocking finding, fix it, verify with the gates, commit, and record "pending review" in the ledger.

A task is **done** when its files match the task's ownership list, every gate the task names is green, its review tier is satisfied, and it is committed on `work/perf-parity`. Report pending items separately from done ones.

## Where the decisions live

- `docs/architecture/prd-performance-and-parity.md`: goals, use cases, requirements, and the owner's decisions O-1 to O-18 (for example tree-sitter for the editor, an Arrow result store, DataFusion only as an optional downloaded helper).
- `docs/architecture/performance-plan.md`: performance targets per axis and phases.
- `docs/architecture/blueprints/*.md`: the approved design for each large rewrite, each ending in an architect-review verdict. Build to the blueprint; record any deviation in the commit message.
- `docs/decisions/`: ADRs. `docs/invariants.md`: rules that have already broken production once.

## Gates

Named sets are defined in `development-plan.md` §1. Run the ones a task names and report the exact result:

- Rust: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
- Swift: `cd app && swift build && swift test`
- Visual parity: `cd app && swift test --filter VisualParityTests` (baselines in `app/Tests/QueryHiveTests/__Baselines__/`; re-record only for a declared visual change V-n, in its own commit)
- Golden: `cargo build --bin queryhive-engine && python3 tools/golden/live_cases.py`; every difference must be classified in `docs/golden-deltas.md`
- App: `./app/build.sh`
- Licences: `cargo deny check licenses`
- UniFFI surface changed: run `./app/build-ffi.sh` and commit `app/Generated/` with the Rust change.

Dev databases run in podman on loopback: PostgreSQL 55432, MySQL 53306, Trino 58080, SSH 52222, toxiproxy (+30 ms) 55435. After a reboot run `podman start qh-postgres qh-mysql qh-trino`, then `deploy/dev/up.sh toxiproxy` and `deploy/dev/qh-sshd-run.sh`.

## Review by risk

- **High** (credentials, SSH trust, spill encryption, Safe Mode classification in `crates/qh-sql`, FFI, any path that can lose user text or data, the engine host, grid and data-plane rewrites): the strongest available model reviews, plus a security or database reviewer where relevant.
- **Medium** (UI features, SQL the app generates): one reviewer.
- **Low** (docs, ADRs, bench tooling, test-only, move-only refactors, cleanup): gates plus one light review.

Every tier gets at most 2 review rounds. Only a blocking finding earns round 2; everything else goes to the backlog and is fixed in the wave's cleanup. After round 2, verify the remaining fixes with the gates and record them as pending review in the ledger instead of starting round 3.

## Working rules

- **Git**: commit locally on `work/perf-parity`, one commit per task that passed its gates, Conventional Commits in English, author Faisal Nugraha Cayunda with no co-author trailer. Since owner decision O-21 (2026-10-06), each gate-green task commit is pushed to both `work/perf-parity` and `main` (fast-forward); no pull requests. Lane worktrees are removed after their commit and a clean `git status`, never with `--force`.
- **TablePro** (`/Users/isal/Workspaces/Lab/Experiments/TablePro`, AGPL-3.0) and **dbx** (`/Users/isal/Workspaces/Lab/Experiments/dbx`, Apache-2.0, a Tauri client added by the owner on 2026-10-06) are study material for ideas and numbers; QueryHive (MIT) contains only its own code, assets and strings.
- **Language**: code, comments, logs and commit messages in English; documents under `docs/` and ADRs in Indonesian, matching the files already there.
- **Measure through the dev build only**: run `app/dist/QueryHive.app/Contents/MacOS/QueryHive --bench <scenario>` by path with isolated data. The installed `/Applications/QueryHive.app` and `/Applications/TablePro.app` belong to the owner and share the dev build's bundle id, so address apps by path, never by bundle id or name. `tools/bench/qhbench` drives apps and is for the owner to run.
- **Engine invariants learned in this run**: send a server-side cancel before dropping an unfinished cursor (MySQL `KILL QUERY` needs the connection id); flush the editor (`AppModel.flushEditors`) before anything reads a tab's SQL; keep CLI, MCP and golden output as NDJSON; classify SQL with the connection's dialect (`Dialect::Mysql` understands backslash escapes, `#` comments and `/*! */`).
- **Scratch files** go in `target/run/` (gitignored, survives reboots).
