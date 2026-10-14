# QueryHive agent guide

QueryHive is a macOS database client: a SwiftUI app (`app/`) over a Rust engine (`crates/`) bridged by UniFFI (`app/Generated/`). This guide governs any AI agent working here. It is the single source of truth for how work is done; the plans below say what work is done.

## Current work: the perf-parity run

Branch `work/perf-parity` carries a long program to beat TablePro on performance, then close design and feature gaps. It was paused on 2026-10-10 ("PAUSE POINT 7" in the ledger) for another AI to continue. `main` is release 0.2.1 (`74bb9cd`) plus the post-release fixes (build scripts, W11-T6b part 1, shell fix, Export anchor); release 0.2.2 was postponed, so its version bump and note (`docs/releases/v0.2.2.md`) are still uncommitted. Check `git log origin/main..main` before assuming `main` equals `origin/main`. `work/perf-parity` fell behind at `7c8f1b5` (0.2.0), so commit on `main` and fast-forward `work/perf-parity` to it with the push (O-21). All eight former `lane/*` branches are integrated and deleted locally (copies stay on `origin/lane/*`); no worktrees of ours remain.

W11-T6b part 1 is committed but its post-review fixes in `crates/qh-editor/src/refs.rs` are pending review, and the fallback is not wired into `Workspace.swift` yet. Run `git status` before any destructive command: do not `git checkout .`, `git stash`, `git clean` or `git reset` over work you did not make.

Resume in this order:

1. Read `docs/handoff.md` (the working method: lanes, SSD targets, gates, known traps).
2. Read `target/run/keep/ledger.md` "PAUSE POINT 7" (the latest; the ledger now lives in `keep/`): what is in flight, the numbered next steps, and the owner-bound items. Owner decisions O-14 to O-31b and the lane plan O-22 are in the same ledger.
3. Use `docs/architecture/development-plan.md` for task IDs, file ownership (§7), gates (§1) and the definition of done (§9). Do not trust the ledger's list of open tasks: it omits W11-T4, W11-T8, W12-T12 and W12-T2, whose code does not exist. Check each task against the code before calling it landed or open. `target/run/debt-register.md` and `target/run/dbx-adoption-backlog.md` are gone from disk; rebuild the debt list from the plan §5 "Register utang". Then continue through W14 per §5; the FFI chain in §7 is serial, so do not fan it out.

Machine state to check first:

- The SSD `/Volumes/Faisal SSD` must be mounted (Rust target dir `qh-cargo-targets/t1`). The internal disk has about 24 GiB free, so never build Rust into the local `target/`.
- The podman `qh-*` containers and images were deleted on purpose. Live, golden and the ttfr-pg/scroll-1m bench gates cannot run until the owner agrees to recreate them (`deploy/dev/up.sh`). Hold any task whose gate is live (W11-T8, for one).
- `app/build-ffi.sh` and `app/build.sh` honour `CARGO_TARGET_DIR` (an absolute path; a relative one resolves differently in cargo and in the scripts). `CARGO_BUILD_TARGET_DIR` or a cargo config `target-dir` is not honoured.

Review policy in force (owner decision O-20): implementation, docs, bench and cleanup run on cheaper models; only review uses the strongest model, for **one** round. After a blocking finding, fix it, verify with the gates, commit, and record "pending review" in the ledger.

A task is **done** when its files match the task's ownership list, every gate the task names is green, its review tier is satisfied, and it is committed on `main`. Report pending items separately from done ones.

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

- **Git**: commit locally on `main`, one commit per task that passed its gates, Conventional Commits in English, author Faisal Nugraha Cayunda with no co-author trailer. Since owner decision O-21 (2026-10-06), each gate-green task commit is pushed to both `work/perf-parity` and `main` (fast-forward); no pull requests. Lane worktrees are removed after their commit and a clean `git status`, never with `--force`.
- **TablePro** (`/Users/isal/Workspaces/Lab/Experiments/TablePro`, AGPL-3.0) and **dbx** (`/Users/isal/Workspaces/Lab/Experiments/dbx`, Apache-2.0, a Tauri client added by the owner on 2026-10-06) are study material for ideas and numbers; QueryHive (MIT) contains only its own code, assets and strings.
- **Language**: code, comments, logs and commit messages in English; documents under `docs/` and ADRs in Indonesian, matching the files already there.
- **Measure through the dev build only**: run `app/dist/QueryHive.app/Contents/MacOS/QueryHive --bench <scenario>` by path with isolated data. The installed `/Applications/QueryHive.app` and `/Applications/TablePro.app` belong to the owner and share the dev build's bundle id, so address apps by path, never by bundle id or name. `tools/bench/qhbench` drives apps and is for the owner to run.
- **Engine invariants learned in this run**: send a server-side cancel before dropping an unfinished cursor (MySQL `KILL QUERY` needs the connection id); flush the editor (`AppModel.flushEditors`) before anything reads a tab's SQL; keep CLI, MCP and golden output as NDJSON; classify SQL with the connection's dialect (`Dialect::Mysql` understands backslash escapes, `#` comments and `/*! */`).
- **Scratch files** go in `target/run/` (gitignored, survives reboots).
- **Test only what the change touches** (owner rule, 2026-10-14, always in force). Run the targeted tests for the code you changed (`swift test --filter <the classes that cover it>`, `cargo test -p <crate> <test>`) and the one gate the task names. Do not run the full `swift test`, the visual suites (VisualParityTests, ChromeParityTests) or the full `cargo test --workspace` unless the change touches what they cover (a recorded scene, a shared crate); GUI tests open windows on the owner's screen, and unrelated reruns waste time and tokens. The full suites run in CI after the push, so the last full run before a push is the orchestrator's, once. Never repeat a run that already passed on the same tree.
