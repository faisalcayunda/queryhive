# QueryHive repository layout: a proposal with the bill attached

> **Executed 24 Sep 2026, with one correction.** Steps 1–5 of §4's plan are done: `check.js` and
> `assets/trino-mascot.png` are deleted, `.env.example` is un-ignored, `qh-sshd-run.sh` moved into
> `deploy/dev/`, and the sweep ran — the legacy Python unit is gone and `Engine.current` is
> `RustEngine()`. The one thing this document got wrong is step 5's third gate:
> `python3 tools/golden/compare.py` no longer exists, because the sweep that deleted the Python
> engine also retired that harness. Its replacement is
> `/usr/bin/python3 tools/golden/live_cases.py` (normalisation in `tools/golden/normalise.py`),
> alongside `cargo test -p qh-ffi --test golden`. Note also that the 22/22 that harness reported at
> the time of writing is **12/22** against today's binary; the ten red cases are classified, not
> newly broken (`docs/golden-deltas.md`, `tests/golden/RECORDED.md`).
>
> Everything else below is left as written, and every count in it is a count of the tree as it
> stood on 23 Sep, before the sweep. Read it as the argument that was made, not as a description of
> the tree today.

Written 23 Sep 2026. Counted from `git ls-files` (279 files) at commit `d094cb6`, with six other
agents writing into the tree at the time of writing. Nothing in the repository was moved, renamed
or deleted to produce this document.

---

## 0. The verdict first

**Most of the visible mess is scheduled for deletion, and the tidy answer is to let the deletion do
the tidying.**

Of the 13 loose files at the root, six belong to the legacy Python engine (`app.py`, `check.js`,
`build_dmg.sh`, `run_local.sh`, `venv_setup.sh`, `requirements.txt`, `requirements-trino.txt` — that
is seven, plus the Python-side `app/` files below). `PROGRESS.md:604-617` records the decision
("Keputusan sapu bersih") and
`docs/architecture/rust-engine-blueprint.md:928-933` lists the same set by name: `exporter/`,
`app/engine/`, `app/engine-requirements.txt`, `requirements*.txt`, `venv_setup.sh`, `run_local.sh`,
`app.py`, `QueryHive.spec`, `check.js`, and the legacy `tests/*.py` — **all deleted in the same
change that flips `Engine.current = RustEngine()`**.

So there are two categories of untidiness here, and they have opposite answers:

| Category | Size | What to do |
|---|---|---|
| Files that die in the sweep commit | ~30 files: 9 `exporter/`, 6 `tests/*.py`, 2 `app/engine/`, `app/build-engine.sh`, `app/engine-requirements.txt`, 7 root loose files, `QueryHive.spec`, `check.js` | **Nothing.** Moving code that is about to be deleted is work for nobody. Quantified in §4. |
| Files that survive and are genuinely misplaced | 4 items, worth ~2 functional edits total | **Fix now.** §3. |

The structure of the survivors is already close to right. `crates/` (14 crates), `docs/` (16),
`deploy/` (13), `assets/` (6), `app/` (37, of which 3 die) all match their contents. There is no
reorganisation that pays for itself. What follows is a small plan plus a longer list of moves I
measured and rejected, because a plan that only says "yes" is not honest about cost.

---

## 1. Evidence base

Every number in this document comes from one of these, all run against the working tree:

```
git ls-files | wc -l                              279
git ls-files | awk -F/ '{print $1}' | sort | uniq -c     crates/92 tests/90 app/37 docs/16 deploy/13 exporter/9 assets/6 tools/3
grep -rn "tests/golden"  (excl. .git target .venv build dist)   36 lines outside tests/golden/, in 14 files
grep -rn "tools/golden"                                         22 lines outside tools/golden/, in 6 files
grep -rn "assets/"                                              21 lines, in 11 files
grep -rn "crates/"                                             972 lines
grep -rn "check\.js"                                            1 line  (docs/architecture/rust-engine-blueprint.md:931)
grep -rn "mascot"                                               0 lines
git check-ignore -v .env.example        -> .gitignore:30:.env.*
grep -rn "CARGO_MANIFEST_DIR" crates/                           4 sites, all resolving the repo root as ../../ from the crate
```

Two facts that decide several rows below, both verified rather than assumed:

- **The corpus carries no paths.** `tests/golden/*.meta.json`, `tests/golden/index.json` and every
  `.ndjson` are pure JSON/NDJSON with masked values (`<TMP>`). `grep -rln "/Users/\|trino_exporter\|tools/golden" tests/golden/**/*.json`
  returns nothing. So moving `tests/golden/` would not corrupt the record — its cost is in code and
  prose, not data. That distinction matters, and §4 prices it.
- **`deploy/qh-sshd-run.sh` and `crates/qh-tunnel/tests/sshd.rs` share one state directory by
  coincidence of arithmetic**: the script computes the repo root as `$HERE/..` (`:27`), the test
  computes it as `CARGO_MANIFEST_DIR/../../` (`sshd.rs:48`). One of them moves in this plan.

---

## 2. Target tree

Unchanged unless a line says otherwise. Each line: what belongs, and what must never go there.

```
/                          Project-level identity and the two build entry points. Belongs: README.md,
                           PROGRESS.md, LICENSE, .gitignore, .env.example, Cargo.toml, Cargo.lock.
                           Never: anything a single directory owns (no engine scripts, no corpus,
                           no per-language config that already lives under crates/ or app/).

  crates/       [92]       The Rust engine: fourteen qh-* crates, one concern each, each a workspace
                           member in Cargo.toml. Belongs beside every crate: its Cargo.toml, src/,
                           tests/, and fixtures its own tests generate or serve. Never: a crate
                           directory that is only added to the workspace after it holds working code
                           (the stated rule at Cargo.toml:1-6), and never a path deeper than this —
                           four tests resolve the repo root as CARGO_MANIFEST_DIR/../../, so depth is
                           load-bearing.

  tests/        [90]       The golden corpus and nothing else: tests/golden/<command>/<case>.ndjson
                           plus its .meta.json, the generated index.json, RECORDED.md.
                           Never: executable code. The six legacy tests/*.py in this directory today
                           are the last Python files that will ever live here.

  tests/golden/            Frozen stdout of the legacy engine, one folder per command, live cases
                           suffixed _live. Belongs: recorded snapshots only, byte-for-byte, and the
                           prose that explains each accepted difference. Never: hand-written
                           expected output (RECORDED.md: "Nothing was hand-written"), and never a
                           folder that no command in crates/qh-ffi/src/lib.rs COMMANDS can produce.

  tools/golden/ [3]        The Python golden harness: record.py (in-process recorder), live_cases.py
                           (the live-case declaration table, which tests/golden cannot be without),
                           compare.py. Belongs: harness scripts that drive the engine from outside the
                           Rust workspace. Never: anything the Rust side imports, and never corpora —
                           data belongs in tests/golden/, which this directory writes.

  app/          [37]       The macOS app. Belongs: Package.swift, Sources/TrinoExporter/, build.sh,
                           build-engine.sh, engine/, engine-requirements.txt, the two generator
                           scripts, QueryHive.entitlements, DESIGN.md. Never: shared library code —
                           anything both the app and the engine need belongs in a crate behind FFI,
                           which is the whole point of ADR-0004. The three files that die in the sweep
                           leave this directory clean without a move.

  docs/         [16]       Prose that outlives its phase. Belongs: architecture/, decisions/ (0001-0010,
                           the ADRs), compatibility.md, golden-deltas.md, benchmarks.md,
                           dependencies.md, migrations/. Never: generated numbers without their
                           generator named (benchmarks.md:3-4 does this correctly), and never a
                           document quoting a path that the layout plan is about to change.

  deploy/       [13]       Container fixtures for tests and benchmarks. Belongs: dev/ (the container
                           runners, seed SQL, benchmark harness) and, after this plan, nothing at
                           this level. Never: something that runs only on a developer's machine and
                           is not needed to reproduce a test or a benchmark.

  deploy/dev/   [13]       Three jobs in one directory, knowingly: integration-test containers
                           (compose.yaml, up.sh, seed-*.sql, make_seed.py), legacy-engine comparison
                           containers (qh-*-old.sh, qh-trino-tls.sh, qh-mysql-old-seed-prefixed.sql),
                           and the benchmark harness (bench_fetch.py, bench-results.jsonl).
                           Never: a seed file used by exactly one test that could not be regenerated
                           by make_seed.py (it is deterministic on purpose, PROGRESS.md:103).

  assets/       [6]        Hand-authored visual inputs. Belongs: drivers/*.svg and their README,
                           icon.icns. Never: a file nothing draws from — trino-mascot.png is exactly
                           that today (§3, R3) — and never a second copy of a generated artifact whose
                           generator lives in app/.

  (tools/kenari_search.py) Gitignored on purpose, .gitignore:33-41 explains why. Not part of the
                           tracked tree; left alone.
```

---

## 3. The moves worth doing

Four items. Two deletions, one path fix, one one-line defect. Total functional edits: **two**.

### R1 — delete `check.js` (root)

| | |
|---|---|
| **From** | `check.js` (944 lines, from the first commit) |
| **To** | — deleted |
| **Why** | It is dead today, not merely redundant. `exporter/static/index.html` inlines its own 1,782-line script (`<script>` at `:685`) and `exporter/web.py:245-246` serves only `static/index.html`; nothing loads a separate file. A tree-wide grep finds **exactly one** inbound mention: `docs/architecture/rust-engine-blueprint.md:931`, which lists `check.js` among the files deleted with the Python engine. Say it plainly: this is the same file's worth of content as a 1,782-line file that supersedes it, kept at the root for 5 weeks after it stopped being reachable. |
| **Dependencies** | None functional. Optionally remove the name from `docs/architecture/rust-engine-blueprint.md:931`, but that line is the deletion note and can stand. |
| **Verify** | `grep -rn "check\.js" . --exclude-dir=.git --exclude-dir=target --exclude-dir=.venv` returns only `blueprint.md:931`. No build gate can notice this file. |

### R2 — `deploy/qh-sshd-run.sh` → `deploy/dev/qh-sshd-run.sh`

| | |
|---|---|
| **From** | `deploy/qh-sshd-run.sh` |
| **To** | `deploy/dev/qh-sshd-run.sh` |
| **Why** | It is the only file at `deploy/` root, and it is the same kind of thing as `deploy/dev/up.sh`: a container runner for tests. Its own header already names `deploy/dev/up.sh` as the owner of the other containers (`:11-13`: *"the dev databases (qh-postgres, qh-mysql, qh-trino …) are run by deploy/dev/up.sh and are left alone even by --down"*). Its sole consumer is `crates/qh-tunnel/tests/sshd.rs`, which cites it three times (`:4`, `:30`, `:33`). One file alone in a top-level directory is the clearest remaining "untidy" in the tree. |
| **Dependencies** | **One functional line.** `deploy/qh-sshd-run.sh:27` today reads `REPO="$(cd "$HERE/.." && pwd)"`; after the move it must read `REPO="$(cd "$HERE/../.." && pwd)"`, otherwise the throwaway client key is written to `deploy/target/qh-sshd/` while `sshd.rs:48` looks in `target/qh-sshd/`. Text-only edits, six lines: the script's own header `:3-4`, `crates/qh-tunnel/tests/sshd.rs:4`, `:30`, `:33`, `PROGRESS.md:477`. No Cargo or Rust change: `sshd.rs:48` uses `CARGO_MANIFEST_DIR/../../target/qh-sshd/id_ed25519`, which still resolves to the repo-root `target/` because `crates/` does not move. |
| **Verify** | `bash -n deploy/dev/qh-sshd-run.sh`; `grep -n 'REPO=' deploy/dev/qh-sshd-run.sh` shows two levels; `ls deploy/target` must **not** exist and `ls target/qh-sshd` must be where the key lands. Then `cargo test -p qh-tunnel` — which passes by *skipping* without `QH_TEST_SSH=1` (`sshd.rs:8`, `:30`), so see §6 for how to tell a wrong move from a right one. |

### R3 — delete `assets/trino-mascot.png`

| | |
|---|---|
| **From** | `assets/trino-mascot.png` (23,773 bytes, dated 20 Aug) |
| **To** | — deleted; or `docs/`, if the owner says it is a design reference he browses |
| **Why** | Zero inbound references. `grep -rn "mascot"` over the tree returns nothing, and every `.png` mentioned anywhere is a temporary file produced at build time (`app/build-dmg.sh:163-165` renders `proof.png`, `app/make-icon.sh:23-45` renders icon-set PNGs). The app's mark does not come from this file: `AppIcon.swift:117,177` rasterises Swift drawing code, which is what `make-icon.sh` bakes into `assets/icon.icns`. So the file is inert — it neither builds anything nor explains anything. |
| **Dependencies** | None. It is tracked, so `git log -- assets/trino-mascot.png` can recover it if the call is wrong. |
| **Verify** | The grep above. No build gate can notice a PNG that no script reads — `swift build` is worth running for form, but it cannot fail here. |
| **Honest caveat** | "No reference" is not "no value". Prefer the named 18 Aug-equivalent reference image, if it is one, over a guess. If in doubt, move it to `docs/` rather than delete: that keeps it out of `assets/`, which should only contain files a build step reads. Do not add a `docs/` citation for it unless it is actually used there. |

### R4 — un-ignore `.env.example` (a real defect, not tidiness)

| | |
|---|---|
| **From** | `.env.example` — on disk, **not tracked**, and hidden by the pattern meant for secrets |
| **To** | same path, tracked |
| **Why** | `git check-ignore -v .env.example` → `.gitignore:30:.env.*`. The file's own first two lines say *"Copy to .env and fill in. .env is gitignored; this example file is not."* — that claim is false. A fresh clone therefore has no `.env.example` at all, and the documented recreation path for the local search helper (`tools/kenari_search.py`, described at `.gitignore:33-41`) has nothing to copy from. This is the one finding in this document that is a defect rather than untidiness. |
| **Dependencies** | `.gitignore:30`: keep `.env.*` for the real secret file and add an exact-name negation after it. Then `git add .env.example`. The negation must be the exact string, so no `.env.<something>` a developer creates locally becomes tracked. |
| **Verify** | `git check-ignore -v .env.example` must now exit **1** (nothing ignored) and `git ls-files .env.example` must print the path. Re-run `git check-ignore -v .env` and confirm it still exits 0 — that is the secret, and `.gitignore`'s own comment calls `git check-ignore -v .env` "the test that it stayed covered". No build gate. |

---

## 4. Moves measured and rejected

Each of these was costed before being dropped. The cost column is the reason, not an aside.

### 4.1 The legacy Python unit → `legacy/` — **reject**

The whole unit leaves the tree as one change, in the same commit that flips `Engine.current`
(`PROGRESS.md:604-617`; the replacement rule at `:616-617` is explicit: the deletion happens *in the
same change* so that the app is never without an engine). A relocation would have to edit, and then
delete:

| Where | Lines that would need editing |
|---|---|
| `run_local.sh` | `:6` `source ./venv_setup.sh`, `:7` `exec .venv/bin/python app.py` |
| `venv_setup.sh` | `:28` `-r requirements.txt`, `:30` `-r requirements-trino.txt` |
| `build_dmg.sh` (root) | `:13` `exporter/__init__.py`, `:28` three `tests/test_*.py`, `:39` `assets/icon.icns`, `:41` `exporter/static` |
| `app/build.sh` | `:7`, `:36`, `:41-43`, `:68-69` |
| `app/build-dmg.sh` | `:24` |
| `QueryHive.spec` | `:14` `['app.py']`, `:41`, `:55` `assets/icon.icns` |
| `tests/*.py` | 6 × `sys.path.insert` |
| `tools/golden/record.py` | `:53` `HARNESS_PATH = ROOT / "tests" / "test_engine_events.py"` |
| `tools/golden/live_cases.py` | `:81` `ENGINE = ROOT / "app" / "engine" / "queryhive_engine.py"` |
| `deploy/dev/bench_fetch.py` | `:52-54` `app/engine/…`, `app/.engine/…` |
| `.gitignore` | `:20-21` root-anchored `/test_mac.py`, `/test_menu.py` |
| Docs | ~30 quoting lines (README's Layout tree at `:265-303`, `docs/architecture/rust-engine-blueprint.md:928-933`, `docs/migrations/python-to-rust.md`, `PROGRESS.md:610`) |

**15 functional edits plus ~30 prose edits, on files that are then deleted.** The move's only
lasting output is a directory that exists for a few days under a new name. Not doing it costs
nothing; doing it costs a day of path-chasing and one more chance to break the golden harness
(`record.py:53` is the sharp edge: the *live* recorder imports a test file, so moving the tests
silently breaks recording of the whole `_live` half of the corpus).

### 4.2 The six `tests/*.py` → `tests/legacy/`, to make `tests/` corpus-only — **reject**

The tidiest version of this idea — `tests/` holds only `golden/` — is real, and it is also the
cheapest-looking version:

- 6 × `sys.path.insert(0, …parents[1])` / `.parent.parent`: `bench_writers.py:25`,
  `test_engine_events.py:28`, `test_parallel_render.py:16`, `test_prefetch.py:11`,
  `test_retry.py:10`, `test_writers.py:15` — all of them count on the repo root being exactly one
  level up, so each one changes.
- `tools/golden/record.py:53` (imports `tests/test_engine_events.py` as a module),
  `build_dmg.sh:28` (runs three of them as a pre-DMG gate), `README.md:256`, `.gitignore:20-21`.

**Six functional edits and four text edits, to move files that die next.** The directory only
*stays* tidy if the sweep happens; if it does, the sweep already tidied it.

### 4.3 `tools/golden/` → `tests/harness/` — **reject**

This is the most tempting structural move, because it removes a top-level directory that holds
three files, and the corpus/harness split is real: the corpus lives in `tests/` and its generator
lives in `tools/`, and `tests/golden/RECORDED.md` has to explain the relationship in its first
paragraph.

Cost, counted:

- Functional: `crates/qh-ffi/tests/golden.rs:1088` (`root().join("tools/golden/live_cases.py")` —
  the Rust test *opens the Python file as its declaration table*); `tools/golden/record.py:51,52,53`
  (`parents[2]` becomes `parents[1]`); `tools/golden/live_cases.py:80,81,82` (same); `compare.py:24`
  (its `sys.path.insert` of its own directory) and `:28` (`ROOT = record.ROOT`) follow changes in
  those two.
- Prose: 22 lines outside the directory, in 6 files — `tests/golden/RECORDED.md` ×11,
  `crates/qh-ffi/tests/golden.rs` ×5 (one of them inside an assertion message a developer reads on
  failure), `docs/architecture/rust-engine-blueprint.md` ×2, `PROGRESS.md` ×2,
  `docs/golden-deltas.md` ×1, `crates/qh-ffi/Cargo.toml` ×1.

**30 edits to delete one name from the top level.** The assertion message is the decisive one: a
path that appears in a failure message is a path nobody will update by accident, and a stale one
sends the next reader to a directory that no longer exists.

### 4.4 `tests/golden/` → anywhere else — **reject**

In *code* this is cheap: two edits, `crates/qh-ffi/tests/golden.rs:854` and `:1045`, both
`root().join("tests/golden")`. Verified that the recorded content itself contains no paths, so a
move would not corrupt a single snapshot. That is not the argument that decides it.

What decides it is that this directory is quoted in **36 lines across 14 files outside itself** —
6 in `docs/golden-deltas.md`, and nine doc-comments in five different crates (`qh-core/src/value.rs`,
`qh-driver-postgres/src/normalize.rs` ×7, `qh-driver-postgres/src/lib.rs`, `qh-driver-mysql` ×2,
`qh-ffi/src/lib.rs`, `qh-ffi/tests/golden.rs`). Those comments are how a maintainer knows *why* a
decoding rule exists: `normalize.rs:442` says "the value `tests/golden/preview/type_zoo.ndjson`
holds for NUMERIC(38,10)". Repath 36 citations to shave one directory name off a path, and you spend
the corpus's credibility to buy nothing. It is also the frozen record of an engine that is about to
be deleted; re-pathing evidence is the last thing to do to it.

**Leave `tests/golden/` where it is, permanently.**

### 4.5 Splitting `deploy/dev/` by lifetime — **reject for now, revisit after the sweep**

Three jobs share this directory: test containers, legacy-engine comparison containers
(`qh-*-old.sh`, `qh-trino-tls.sh`, `qh-mysql-old-seed-prefixed.sql`), and the benchmark harness
(`bench_fetch.py`, `bench-results.jsonl`). The third is a different lifetime from the first two —
it dies with the Python engine, because `bench_fetch.py:52-54` drives `app/engine/queryhive_engine.py`
and `app/.engine/python/bin/python3`.

Cost of splitting today: `up.sh` alone carries 34 inbound references; `bench_fetch.py:49-54`
hardcodes `deploy/dev/bench-results.jsonl`, `docs/benchmarks.md` and the `app/engine` paths;
`docs/compatibility.md` quotes `deploy/dev/qh-*-old.sh` in 8 lines. **Reject now** — the third job
disappears on its own in the sweep, and re-evaluating then costs nothing because the directory will
be down to its surviving two jobs.

### 4.6 `crates/` to a deeper path — **reject, and record the invariant**

972 quoting lines, and four places compute the repo root as `CARGO_MANIFEST_DIR/../../`:
`crates/qh-ffi/tests/golden.rs:41-42`, `crates/qh-export/tests/plan_parity.rs:97-101`,
`crates/qh-tunnel/tests/sshd.rs:48`, `crates/qh-driver-postgres/tests/tls.rs:117`. Renaming `crates/`
at the same depth breaks nothing functionally; moving it one level deeper breaks all four. **Write
the invariant down: crates stay exactly two levels under the repo root.**

### 4.7 Root-loose survivors: `PROGRESS.md`, `Cargo.toml`, `LICENSE`, `.gitignore` — **reject**

`PROGRESS.md` is a status/decision log at the root, referenced from `.gitignore:39`,
`docs/architecture/rust-engine-blueprint.md:4`, `:967`, `:1064` and
`crates/qh-credentials/tests/keychain.rs:24`. It is the file the sweep decision itself is recorded
in. Conventional location, referenced comfortably, zero gain from moving. The same reasoning covers
`Cargo.toml`/`Cargo.lock` (workspace root by definition) and `LICENSE` (ADR-0002 pins MIT; a licence
file at the root is what licence detection reads).

---

## 5. Order of operations

No step depends on another, so the order is chosen for *auditability*: cheapest and least risky
first, so a failed step cannot be confused with an earlier one. The tree is never in a broken state —
no step removes something another step is about to use.

| # | Step | Gate (what proves it worked) |
|---|---|---|
| 1 | Delete `check.js` | `grep -rn "check\.js" . --exclude-dir=.git --exclude-dir=target` → only `blueprint.md:931`. No build gate can observe this file, and saying otherwise would be dishonest. |
| 2 | Un-ignore `.env.example` (`.gitignore:30` + `git add .env.example`) | `git check-ignore -v .env.example` exits 1; `git check-ignore -v .env` exits 0; `git ls-files .env.example` prints the path. No build gate. |
| 3 | Delete (or relocate) `assets/trino-mascot.png` | `grep -rn "mascot" . --exclude-dir=.git --exclude-dir=target` → nothing. Run `swift build` for form only; it cannot fail on this. |
| 4 | Move `deploy/qh-sshd-run.sh` → `deploy/dev/`, edit `:27`, fix the six text lines | `bash -n deploy/dev/qh-sshd-run.sh`; `ls deploy/target` → not found; `cargo test --workspace`; then **`swift build`** (nothing in this step touches Swift, so a failure means the tree was already broken) |
| 5 | *Owner's step, not this plan's:* the sweep — delete the legacy unit and flip `Engine.current = RustEngine()` in the same commit | `cargo test --workspace`, `swift build`, and the golden harness. **All three.** This is the step whose verification is genuinely three-way: `cargo test --workspace` runs `crates/qh-ffi/tests/golden.rs` against `tests/golden/`, `swift build` proves the app links without `PythonEngine`, and `python3 tools/golden/compare.py` proves the corpus still agrees with what it froze. |

Per-step verification summary, since the ask is explicit about it:

| Step | `cargo test --workspace` | `swift build` | golden harness |
|---|---|---|---|
| 1 `check.js` | not needed | not needed | not needed |
| 2 `.env.example` | not needed | not needed | not needed |
| 3 `trino-mascot.png` | not needed | form only | not needed |
| 4 `qh-sshd-run.sh` | **yes** (proves the crate still compiles and the tunnel tests skip cleanly) | **yes** (cheap, catches nothing in this step) | not needed |
| 5 the sweep | **yes** | **yes** | **yes** |

---

## 6. The riskiest single step

**Step 4 — moving `deploy/qh-sshd-run.sh` one directory down.** Not because it is large, but because
its failure mode is silence.

**What would break.** `deploy/qh-sshd-run.sh:27` computes the repo root as `$HERE/..`, and
`crates/qh-tunnel/tests/sshd.rs:48` computes the same root as `CARGO_MANIFEST_DIR/../../`. Move the
script without changing `:27` and the two disagree: the script writes its throwaway client key to
`deploy/target/qh-sshd/id_ed25519`, the test reads `target/qh-sshd/id_ed25519`, and nothing
reconciles them.

**How you would fail to notice.** This is the part that makes it the riskiest step rather than
merely the sharpest. `sshd.rs` is written to *skip loudly rather than fail loudly*:

```
sshd.rs:8    "Without `QH_TEST_SSH=1` every test prints why it is skipping and returns."
sshd.rs:30   "skipped: set QH_TEST_SSH=1 with deploy/qh-sshd-run.sh running (container qh-sshd-dev)"
```

There is no test today that asserts the key path exists — `QH_SSH_KEY` merely defaults to it and is
never required to be present (`sshd.rs:46-49` reads the env var, falling back to the path without a
check). So `cargo test -p qh-tunnel` exits **green** after a wrong move: every tunnel test skipped,
none failed. A plan that stops at "cargo test passed" ships this defect.

**How to tell.** Three checks, in order:

1. `ls deploy/target` → must be **not found**. `ls target/qh-sshd` → must exist after one script run.
   This is the direct, offline proof; it needs no container.
2. Run `cargo test -p qh-tunnel` and read the output: a *skip* line naming `QH_TEST_SSH=1` is the
   signal that nothing ran. Test counts must be non-zero only when the container is up, and the
   skip text must quote the new path (`deploy/dev/qh-sshd-run.sh:30`), so a stale message is visible
   in the same line.
3. When the owner next runs the container anyway: `QH_TEST_SSH=1 cargo test -p qh-tunnel` with
   `deploy/dev/qh-sshd-run.sh` up must show the tunnel tests **executing**, not skipping.

**Second-order note.** This step and step 3 are the only two that change anything a build reads, and
neither touches the Rust or Swift code. The sweep (step 5) is the high-blast-radius step, but it is
the owner's recorded decision with three named verifications, not a step this plan invents. Between
the two, step 4 is the one this plan could get wrong quietly.

---

## 7. What I would leave alone, and why

- **`tests/golden/`** — permanent. 2 code edits, 36 prose citations, and it is the shared contract
  between a Rust test and the Python harness. §4.4.
- **`tools/golden/`** — leave, despite being a 3-file top-level directory. 30 edits for one name.
  §4.3.
- **`deploy/dev/`** — leave the three-lifetimes shape until the sweep removes the third. §4.5.
- **`crates/`** — leave, and treat "two levels under the root" as an invariant. §4.6.
- **`crates/qh-driver-postgres/tests/tls/*.key`** — three committed private keys. This looks
  alarming and is deliberately fine: `tests/tls/make-certs.sh:7-10` states that the files are
  committed rather than generated per run *because* the container's certificate must be signed by
  the same CA the test trusts, and `:19-20` says plainly that the keys *"protect nothing: they belong
  to a throwaway container on 127.0.0.1"*. A documented test fixture with a stated threat model is
  not misplaced. Leave it, and do not "fix" it.
- **`app/engine/`, `app/build-engine.sh`, `app/engine-requirements.txt`** — they are inside the
  Python unit and die with it; `app/` cleans itself. §0.
- **`.spec` artifacts, `build/`, `dist/`, `app/.engine/`, `target/`, `.venv/`, `backup.zip`,
  `.qh-patch.py`** — untracked build output and one deliberate archive (`PROGRESS.md:619-624` keeps
  `backup.zip` on purpose). Ignored, regenerable, not part of the tracked tree.

The principle: **tidying earns its keep when someone browses the directory.** A directory with
3 files and 22 inbound references is not the problem; a top-level mixed bag whose contents are about
to be deleted is not a problem either, because it will not be there. Four items were worth doing.
Everything else on the list was measured, priced, and dropped.

---

## 8. Misplaced-but-not-untidy, with the evidence

Three items are the wrong *thing* rather than the wrong *place*. Each with the proof it is unused or
mis-declared:

1. **`check.js` (root, 944 lines) — dead, not just loose.** Evidence: one inbound reference in the
   entire tree (`docs/architecture/rust-engine-blueprint.md:931`, the deletion list).
   `exporter/static/index.html:685` opens its own `<script>`; `exporter/web.py:245-246` returns only
   `static/index.html`; there is no `<script src=` in the served HTML. It is a 944-line predecessor
   of the 1,782-line file that replaced it, still sitting at the root. **Recommend: delete (§3, R1).**

2. **`assets/trino-mascot.png` (23,773 bytes) — orphaned.** Evidence: `grep -rn "mascot"` over the
   tree returns nothing; the only `.png` references anywhere are temporary outputs of
   `app/build-dmg.sh:163-165` and `app/make-icon.sh:23-45`; the app's icon is drawn in Swift
   (`AppIcon.swift:117,177`). Nothing reads it and no document cites it. **Recommend: delete, or move
   to `docs/` if it is a design reference (§3, R3).**

3. **`.env.example` — a file whose own text is false.** Evidence: `git check-ignore -v .env.example`
   → `.gitignore:30:.env.*`; the file begins *".env is gitignored; this example file is not."* It is
   the intended onboarding artifact, it is on disk, and it is invisible to git. **Recommend: fix the
   ignore rule and track it (§3, R4).** This is the only finding here that is a defect.

Two things that *look* misplaced and are not, so nobody spends effort on them: the committed TLS
private keys (§7) and `QueryHive.spec` at the root (untracked, gitignored by `*.spec` at
`.gitignore:6`, regenerated by `build_dmg.sh`, and deleted by the sweep).

Duplicate-purpose pairs I checked and would not touch:

- **Root `build_dmg.sh` vs `app/build-dmg.sh`** — both produce `dist/QueryHive-<version>-arm64.dmg`,
  but the first wraps the legacy pywebview app in PyInstaller and the second is the Swift app
  (`app/build-dmg.sh:2`). Genuinely two build paths for two engines; the first dies in the sweep,
  which resolves the duplication for free.
- **`requirements.txt` + `requirements-trino.txt` (root) vs `app/engine-requirements.txt`** — dev
  venv versus the bundled, fully-pinned lockfile that `app/build-engine.sh:47` reuses instead of
  re-resolving. Different consumers, both explained in their own headers and at
  `README.md:158-161`. Both die in the sweep.
- **`crates/qh-driver-postgres/tests/tls/` vs `crates/qh-driver-mysql/tests/mysql-tls-container.sh`**
  — one uses committed certs, the other a container script, each for a reason stated in its header.
  Different drivers, different TLS stories. Leave.

### One more thing on the deleted `exporter/` (asked explicitly)

The question was whether any part of the legacy unit is worth moving *now*. Answer: no, and the
asymmetry is worth stating numerically. Moving it costs 15 functional edits plus ~30 prose edits
(§4.1) and pays off only in the window between now and the sweep. Its three most valuable files —
`exporter/writers.py`, `exporter/source.py`, `exporter/export.py` — already exist in Rust
(`crates/qh-export/src/{writers,plan,lib}.rs`, `qh-driver*`), and the parity work is being tracked
against the corpus, not against the Python source. The one part that must not be lost —
`exporter/static/index.html` and `app/engine/queryhive_engine.py` — must be *superseded*, not moved:
the app's engine is `crates/qh-ffi` and the UI is SwiftUI. So the correct disposition is the one
already recorded, and this plan deliberately adds nothing to it. The cheapest thing you can do for a
directory you are about to delete is leave it findable by the names the deletion plan already uses.
