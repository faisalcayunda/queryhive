#!/usr/bin/env python3
"""Check or record golden snapshots of the Python engine against the *real* dev servers.

    <venv>/bin/python tools/golden/live_cases.py                        # check every case
    <venv>/bin/python tools/golden/live_cases.py postgres_count_live    # check one case
    <venv>/bin/python tools/golden/live_cases.py --record <case-id>     # record a new case
    <venv>/bin/python tools/golden/live_cases.py --record --force       # re-record on purpose
    <venv>/bin/python tools/golden/live_cases.py --list

Why this exists
---------------
`record.py` freezes the engine's *normalisation* by driving it in-process with a
fake cursor: the rows a case sees are the ones the case wrote down, so what is
pinned is what the engine does with a value it was handed. That leaves one
question entirely unasked -- does the value the engine is handed on a real
server have the shape the fake cursor pretended it did? A `numeric(38,10)` is a
`Decimal` only because psycopg decoded it that way; a type OID is a number only
because the server sent one. Those are facts about the wire, and the only way to
freeze them is to point the engine at a server that is really running.

So these cases run the engine exactly as the app runs it -- one command name,
settings in the environment, one JSON object per stdout line -- as a child
process against the containers `deploy/dev/up.sh` starts, and freeze its stdout
the same way `record.py` does (and with the same normalisation: see
`normalise`). Nothing is hand-written: a case that cannot run is reported and
recorded not at all.

What it does by default
-----------------------
Nothing is written. Each selected case is run again against the server it was
recorded from and its stdout is diffed, line for line, against the snapshot on
disk (`record.diff_lines`, the same comparison `compare.py` uses for the
in-process cases). Every key must match except the ones `normalise` masks --
`elapsed_ms`, `query_id`, temp paths, and the object identifiers the server itself
assigned (they climb every time `deploy/dev/up.sh` replays its seed, so freezing
them freezes the cluster's history rather than the engine's answer). A case that
cannot run at all (a
container down, a client package not importable) is reported as such rather
than as a difference: "could not ask" is not "the answer changed".

What `--record` writes
----------------------
`tests/golden/<command>/<case>.ndjson` and `<case>.meta.json`, exactly like the
in-process cases, so `crates/qh-ffi/tests/golden.rs` finds them the same way.
`index.json` is rebuilt from every `.meta.json` in the tree, so recording here
never drops the in-process cases and re-recording those never drops these. Case
ids end in `_live` so the two sets stay visible as what they are.

What it will overwrite
----------------------
Only `--record` writes, and only for the cases named (or every case, if none is
named). A snapshot that already exists is *not* replaced by that alone: the run
refuses before touching a server, names the files it would have lost, and exits
2, unless `--force` says the replacement is deliberate. A case that has no
snapshot yet is written with no extra flag -- recording a new case is the
normal use. Recorded snapshots are the only copy of what the previous engine
answered against a real server, which is why a bare invocation cannot destroy
them.

Requirements
------------
The engine's real client packages have to be importable by the interpreter that
runs this script -- trino, psycopg and pymysql -- and the dev servers have to be
up:

    deploy/dev/up.sh postgres mysql   # and `trino` if there is memory for it
    uv venv /tmp/qh-golden-venv && uv pip install --python ... trino psycopg[binary] pymysql

`check_tools()` says exactly what is missing rather than producing a snapshot of
a traceback.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ENGINE = ROOT / "app" / "engine" / "queryhive_engine.py"
GOLDEN_DIR = ROOT / "tests" / "golden"

sys.path.insert(0, str(Path(__file__).resolve().parent))

import record  # noqa: E402  (same directory; reuses its normalisation)

# The dev containers, exactly as `deploy/dev/up.sh` publishes them. The ports are
# off the defaults on purpose (see that script) so a database already listening
# locally is neither shadowed nor accidentally used.
PG = {
    "DB_KIND": "postgres",
    "DB_HOST": "127.0.0.1",
    "DB_PORT": "55432",
    "DB_USER": "qh",
    "DB_PASSWORD": "qh-dev-only",
    "DB_DATABASE": "qh",
    "DB_SCHEMA": "public",
    "DB_SSLMODE": "disable",
}
MYSQL = {
    "DB_KIND": "mysql",
    "DB_HOST": "127.0.0.1",
    "DB_PORT": "53306",
    "DB_USER": "qh",
    "DB_PASSWORD": "qh-dev-only",
    "DB_DATABASE": "qh",
}
# Trino has no seeded catalog: `tpch` is the one the coordinator ships with, and
# `tiny` its smallest generated schema, so the case needs nothing loaded.
TRINO = {
    "DB_KIND": "trino",
    "DB_HOST": "127.0.0.1",
    "DB_PORT": "58080",
    "DB_USER": "queryhive",
    "DB_DATABASE": "tpch",
    "DB_SCHEMA": "tiny",
}

# Every command and level check is a usage error made before the network, so a
# driver that has no such level says so in its own words -- worth freezing for
# the one driver whose missing level the mocked set never exercised.
RETRIES = {"RETRIES": "0"}


@dataclass
class LiveCase:
    """One command run against a real server, and why it is worth freezing."""

    case_id: str
    command: str
    engine: str
    settings: dict
    notes: str
    sql: str | None = None
    env: dict = field(default_factory=dict)
    # True only for the case whose whole point is a refusal the engine makes
    # before the network. Every other case must produce a result, not an error:
    # a server that is down would otherwise freeze a connection failure as if it
    # were the answer, which is the one thing a snapshot must never do.
    expect_error: bool = False

    @property
    def folder(self) -> str:
        return self.command

    def env_map(self) -> dict:
        values = {**self.settings, **RETRIES, **self.env}
        if self.sql is not None:
            values["SQL"] = self.sql
        return values

    def command_line(self) -> str:
        """The exact shell command line the snapshot was recorded with.

        The temp root is printed as `<TMP>`, the same token `normalise` stores,
        because an `export` case's OUT_DIR is a real directory under it and the
        document this feeds has to be the same on every machine.
        """
        parts = [
            f"{key}={_quote_shell(str(value).replace(tempfile.gettempdir(), '<TMP>'))}"
            for key, value in sorted(self.env_map().items())
        ]
        return " ".join(parts + [sys.executable, "-s", "-u", "app/engine/queryhive_engine.py",
                                 self.command])


def _quote_shell(value: str) -> str:
    text = str(value)
    return "'" + text.replace("'", "'\\''") + "'" if any(c in text for c in " '\"$") else text


def _export_dir(case_id: str) -> str:
    """The `export` case's output directory: under the temp root, named for the case.

    Fixed rather than `mkdtemp` because the path is part of the snapshot. The
    recorder masks the temp root into `<TMP>`, so the stored path is
    `<TMP>/qh-golden-<case>/type_zoo.csv` on every run -- while the random
    suffix `mkdtemp` adds would differ every time and make the diff noise. Named
    after the case so two export cases can never overwrite each other's files,
    and under the temp root so nothing lands in the working tree.
    """
    return str(Path(tempfile.gettempdir()) / f"qh-golden-{case_id}")


def cases() -> list[LiveCase]:
    """Every live case, in the order they are recorded."""
    return [
        # -- PostgreSQL ---------------------------------------------------- #
        LiveCase(
            "postgres_type_zoo_live",
            "preview",
            "postgres",
            PG,
            "The wire shape psycopg hands over for every hard Postgres type, as the "
            "engine renders it: a numeric(38,10) with its digits, the scientific "
            "spelling psycopg's Decimal gives a tiny negative, a timestamptz the "
            "server converted to UTC (the seeded +07:00 is *not* what comes back), a "
            "naive timestamp that must not gain a zone, an interval, json and jsonb "
            "text in the server's own key order, arrays, bytea with a NUL and 0xFF, "
            "a uuid, an enum label, a point, a NULL, an empty string and the four "
            "characters NULL. The `columns` type is the type OID psycopg reports; the "
            "identifier of the seeded `mood` enum is masked to `<OID>` because it climbs "
            "every time the seed is replayed.",
            sql="SELECT * FROM type_zoo",
        ),
        LiveCase(
            "postgres_batching_live",
            "preview",
            "postgres",
            PG,
            "250 real rows through PREVIEW_BATCH=200: the batch boundary, the second "
            "batch, and `truncated: true` from the one extra row the cap costs.",
            sql="SELECT id, c01 FROM wide_500k ORDER BY id",
            env={"LIMIT": "250"},
        ),
        LiveCase(
            "postgres_catalogs_live",
            "catalogs",
            "postgres",
            PG,
            "`catalogs` on the driver whose object tree has no catalog level: the "
            "pg_database listing, filtered to connectable non-template databases, in "
            "the query's own ORDER BY.",
        ),
        LiveCase(
            "postgres_tables_live",
            "tables",
            "postgres",
            PG,
            "The information_schema statement Postgres builds for `tables`, against a "
            "schema that really holds the seeded tables -- BASE TABLE only, ordered.",
        ),
        LiveCase(
            "postgres_objects_live",
            "objects",
            "postgres",
            PG,
            "The only driver whose Name/OID/Owner/ACL shape is real: a real OID, the "
            "real owner, and a NULL relacl rendered as an empty string. The OID cell is "
            "masked to `<OID>` for the reason above; the column list that names it is not.",
        ),
        LiveCase(
            "postgres_count_live",
            "count",
            "postgres",
            PG,
            "`count`'s wrapper run by Postgres itself on 500,000 real rows, so the "
            "number is the server's rather than a fake cursor's.",
            sql="SELECT * FROM wide_500k",
        ),
        LiveCase(
            "postgres_explain_live",
            "explain",
            "postgres",
            PG,
            "Postgres answers EXPLAIN with one `QUERY PLAN` text column; this freezes "
            "the real column name, the real plan text and the absence of `truncated`.",
            sql="SELECT * FROM type_zoo WHERE id = 1",
        ),
        LiveCase(
            "postgres_export_live",
            "export",
            "postgres",
            PG,
            "The CSV writer fed by psycopg's real decoding rather than a fake cursor's "
            "rows: the `start` columns carry the server's own type OIDs (the seeded "
            "`mood` enum's masked to `<OID>` for the reason above), the header is "
            "the table's, and `done` reports the real row count and the file's real "
            "byte size. The path is masked to <TMP>; the size is not -- it is a fact "
            "about the bytes the writer produced from the values psycopg handed over.",
            sql="SELECT * FROM type_zoo",
            env={"FORMAT": "csv", "NAME": "type_zoo",
                 "OUT_DIR": _export_dir("postgres_export_live")},
        ),
        # -- MySQL --------------------------------------------------------- #
        LiveCase(
            "mysql_type_zoo_live",
            "preview",
            "mysql",
            MYSQL,
            "The same hard types again, as MySQL decodes them: decimal(38,10), datetime "
            "and timestamp kept distinct, a time PyMySQL hands back as an object, json "
            "re-ordered by the server, a blob with a NUL and 0xFF, an enum label, a "
            "char(36) and a binary(16) uuid, a NULL, an empty string and the four "
            "characters NULL. The `columns` type is MySQL's column type code.",
            sql="SELECT * FROM type_zoo",
        ),
        LiveCase(
            "mysql_batching_live",
            "preview",
            "mysql",
            MYSQL,
            "250 real rows through the 200-row preview batch: the boundary, the second "
            "batch and `truncated: true`.",
            sql="SELECT id, c01 FROM wide_500k ORDER BY id",
            env={"LIMIT": "250"},
        ),
        LiveCase(
            "mysql_tables_live",
            "tables",
            "mysql",
            MYSQL,
            "MySQL's SHOW TABLES FROM `qh`, quoted with backticks, in the server's own "
            "order.",
        ),
        LiveCase(
            "mysql_objects_live",
            "objects",
            "mysql",
            MYSQL,
            "Name/Engine/Rows/Comment from information_schema.TABLES: the columns no "
            "other driver can answer, with a real engine name, a real (estimated) row "
            "count and an empty comment.",
        ),
        LiveCase(
            "mysql_schemas_live",
            "schemas",
            "mysql",
            MYSQL,
            "The one browse command MySQL has no level for: a usage error naming the "
            "driver, raised before the network is touched.",
            expect_error=True,
        ),
        LiveCase(
            "mysql_count_live",
            "count",
            "mysql",
            MYSQL,
            "The count wrapper on MySQL over 500,000 real rows.",
            sql="SELECT * FROM wide_500k",
        ),
        LiveCase(
            "mysql_explain_live",
            "explain",
            "mysql",
            MYSQL,
            "MySQL is the driver whose EXPLAIN is not one text column: a real "
            "multi-column plan table, which is why `explain` emits preview's protocol "
            "rather than a plan shape",
            sql="SELECT * FROM type_zoo WHERE id = 1",
        ),
        LiveCase(
            "mysql_export_live",
            "export",
            "mysql",
            MYSQL,
            "The CSV writer fed by PyMySQL's real decoding: the `start` columns carry "
            "MySQL's own column type codes, and `done` reports the real row count and "
            "the file's real byte size -- a different size from Postgres's for the same "
            "statement, which is the point of freezing both. The path is masked to "
            "<TMP>, the size is not.",
            sql="SELECT * FROM type_zoo",
            env={"FORMAT": "csv", "NAME": "type_zoo",
                 "OUT_DIR": _export_dir("mysql_export_live")},
        ),
        # -- Trino --------------------------------------------------------- #
        LiveCase(
            "trino_nation_live",
            "preview",
            "trino",
            TRINO,
            "The real Trino decoder on tpch.tiny.nation: bigint, varchar, and a NULL "
            "comment that must arrive as JSON null rather than \"None\".",
            sql="SELECT nationkey, name, regionkey, comment FROM tpch.tiny.nation "
                "ORDER BY nationkey",
        ),
        LiveCase(
            "trino_type_zoo_live",
            "preview",
            "trino",
            TRINO,
            "Trino's own decoding of the types that matter, from expressions rather "
            "than a table: a decimal(38,10) with its digits intact, a tiny negative "
            "decimal, a timestamp with a zone, a naive timestamp, a date, a time, an "
            "interval, a NULL and an empty string.",
            sql="SELECT CAST(1234567890123456789012345678.1234567890 AS decimal(38,10)) "
                "AS high_precision, "
                "CAST(-0.0000000001 AS decimal(38,10)) AS tiny_negative, "
                "TIMESTAMP '2026-01-31 12:00:00.123456 +07:00' AS tz_aware, "
                "TIMESTAMP '2026-01-31 12:00:00.123456' AS tz_naive, "
                "DATE '2026-01-31' AS a_date, "
                "TIME '23:59:59.999999' AS a_time, "
                "INTERVAL '3' DAY + INTERVAL '4' HOUR + INTERVAL '5' MINUTE "
                "+ INTERVAL '6' SECOND AS an_interval, "
                "CAST(NULL AS varchar) AS no_value, '' AS empty_text",
        ),
        LiveCase(
            "trino_batching_live",
            "preview",
            "trino",
            TRINO,
            "250 real tpch rows -- a bigint, the tpch connector's `double` totalprice "
            "and a date -- cut by the 200-row batch rule, so the boundary, the second "
            "batch and `truncated: true` are all frozen. tpch's generated orderkeys are "
            "sparse (1..7, then 32..39, ...), so the second batch starts at 801 rather "
            "than at 201: the row *number* is what the cap counts, not the key.",
            sql="SELECT orderkey, totalprice, orderdate FROM tpch.tiny.orders "
                "ORDER BY orderkey",
            env={"LIMIT": "250"},
        ),
        LiveCase(
            "trino_objects_live",
            "objects",
            "trino",
            TRINO,
            "Trino's information_schema objects query: Name/Type only, because Trino "
            "has no OID, owner or ACL to answer with.",
        ),
        LiveCase(
            "trino_count_live",
            "count",
            "trino",
            TRINO,
            "The count wrapper run by the coordinator over tpch.tiny.orders.",
            sql="SELECT * FROM tpch.tiny.orders",
        ),
        LiveCase(
            "trino_explain_live",
            "explain",
            "trino",
            TRINO,
            "A real Trino plan: one varchar column, one row per plan line, fetched "
            "through EXPLAIN with the caller's semicolon stripped.",
            sql="SELECT * FROM tpch.tiny.nation;",
        ),
    ]


LIVE_IDS: tuple[str, ...] = tuple(case.case_id for case in cases())


def check_tools() -> list[str]:
    """What is missing before a live run can mean anything, as plain sentences.

    The interpreter that matters is the one running this script, so each import
    is asked of `sys.executable` -- not of a `python3` found on PATH, which may
    be a different interpreter entirely and whose absence (or whose stubs) would
    otherwise skip the check without saying so.
    """
    problems = []
    for module in ("trino", "psycopg", "pymysql"):
        if subprocess.run(
            [sys.executable, "-c", f"import {module}"], capture_output=True
        ).returncode:
            problems.append(f"{module} is not importable by {sys.executable}")
    if not ENGINE.is_file():
        problems.append(f"the engine is not at {ENGINE}")
    return problems


def run_case(case: LiveCase) -> tuple[int, str, str]:
    """Run one case as the app does; return (exit code, stdout, stderr).

    A child process, not an import: the point is that the real client packages
    open the real connection. `-s` keeps the user's site directory out of it and
    `-u` keeps stdout unbuffered, which is what the app passes too.
    """
    env = {
        key: os.environ[key]
        for key in ("PATH", "HOME", "TMPDIR", "LANG", "LC_ALL")
        if key in os.environ
    }
    env["PYTHONIOENCODING"] = "utf-8"
    env.update(case.env_map())
    completed = subprocess.run(
        [sys.executable, "-s", "-u", str(ENGINE), case.command],
        cwd=str(ROOT), env=env, capture_output=True, text=True, timeout=600,
    )
    return completed.returncode, completed.stdout, completed.stderr


def collect(only: list[str] | None = None) -> tuple[dict[str, dict], list[str]]:
    """Run every case and return ({case_id: {meta, lines}}, failures).

    The mask goes on before the first case runs, not at write time: an `export`
    case really does write into a directory under the temp root, and a check has
    to normalise those paths exactly as the recording that produced the snapshot
    did, or every export case would diff on its own path.
    """
    record.add_roots(tempfile.gettempdir())
    results: dict[str, dict] = {}
    failures: list[str] = []
    for case in cases():
        if only and case.case_id not in only:
            continue
        code, stdout, stderr = run_case(case)
        try:
            lines = record.normalise(stdout)
        except ValueError as exc:
            failures.append(f"{case.case_id}: a stdout line was not JSON ({exc})")
            continue
        if not lines:
            failures.append(f"{case.case_id}: the engine emitted nothing (exit {code})")
            continue
        failed = any(line["event"] == "error" for line in (json.loads(line) for line in lines))
        if failed and not case.expect_error:
            # A server that is down, a container another agent restarted, a wrong
            # password: all of them look like this, and none of them is the
            # answer a snapshot is supposed to freeze. Refuse rather than record.
            failures.append(
                f"{case.case_id}: the engine reported an error (exit {code}): "
                f"{(json.loads(lines[-1]).get('message') or '')[:160]}"
            )
            continue
        if case.expect_error and not failed:
            failures.append(f"{case.case_id}: expected an error event, got a result")
            continue
        results[case.case_id] = {
            "meta": {
                "case": case.case_id,
                "command": case.command,
                "exit_code": code,
                # The same five keys the in-process cases carry, and no more:
                # index.json is one shape for both recorders, and the engine and
                # the command line a case ran under belong in RECORDED.md.
                "notes": case.notes,
                "captures": "stdout",
                "event_lines": len(lines),
            },
            "lines": lines,
            "stderr": stderr,
        }
    return results, failures


def snapshot_paths(case: LiveCase, destination: Path = GOLDEN_DIR) -> tuple[Path, Path]:
    """The two files a recorded case owns: its event lines and its metadata."""
    folder = destination / case.folder
    return folder / f"{case.case_id}.ndjson", folder / f"{case.case_id}.meta.json"


def _show(path: Path) -> str:
    """A path for a message: repo-relative when it is in the repo, absolute when not.

    `destination` is a parameter, so a caller (a test, or a run against a scratch
    tree) can point it outside the repo -- and `relative_to` raises rather than
    saying so.
    """
    try:
        return str(path.relative_to(ROOT))
    except ValueError:
        return str(path)


def record_live(destination: Path = GOLDEN_DIR, only: list[str] | None = None,
                force: bool = False) -> int:
    """Record the selected live cases and rebuild the index from the whole tree.

    A snapshot that already exists is refused, by name and before a server is
    touched, unless `force` was asked for: these files are the only copy of what
    a real server answered, so replacing one has to be a decision rather than
    what happens when `--record` is typed without a case in mind. A case that
    has never been recorded is written with no extra flag -- that is the normal
    use of this tool, and it has nothing to lose.
    """
    selected = [case for case in cases() if only is None or case.case_id in only]
    existing = [
        path
        for case in selected
        for path in snapshot_paths(case, destination)
        if path.exists()
    ]
    if existing and not force:
        for path in existing:
            print(f"refusing to replace {_show(path)}", file=sys.stderr)
        print("a recorded snapshot is the only copy of a real server's answer; "
              "pass --force to overwrite it on purpose", file=sys.stderr)
        return 2
    results, failures = collect(only)
    if failures:
        for failure in failures:
            print(f"FAILED {failure}", file=sys.stderr)
        print("nothing was written; fix the server or the case and run again", file=sys.stderr)
        return 1
    destination.mkdir(parents=True, exist_ok=True)
    for case_id, payload in results.items():
        folder = destination / payload["meta"]["command"]
        folder.mkdir(parents=True, exist_ok=True)
        (folder / f"{case_id}.ndjson").write_text(
            "\n".join(payload["lines"]) + "\n", encoding="utf-8"
        )
        (folder / f"{case_id}.meta.json").write_text(
            json.dumps(payload["meta"], indent=2, ensure_ascii=False, sort_keys=True) + "\n",
            encoding="utf-8",
        )
    record.rebuild_index(destination)
    print(f"recorded {len(results)} case(s): {', '.join(sorted(results))}")
    return 0


def compare_live(destination: Path = GOLDEN_DIR, only: list[str] | None = None) -> int:
    """Re-run the selected cases and diff them against the snapshots on disk.

    Read-only, and the default: nothing here opens a file for writing, so a
    bare invocation cannot lose a snapshot. A case that could not run at all --
    a container down, a client package missing -- is reported as a failure
    rather than as a difference, because "could not ask the server" is not "the
    answer changed", and the two must never be read alike.
    """
    results, failures = collect(only)
    for failure in failures:
        print(f"FAILED {failure}", file=sys.stderr)
    diffs = 0
    missing = 0
    checked = 0
    for case in cases():
        if only is not None and case.case_id not in only:
            continue
        if case.case_id not in results:
            continue  # already reported as a failure; a run that did not happen is not a diff
        checked += 1
        path = snapshot_paths(case, destination)[0]
        if not path.is_file():
            missing += 1
            print(f"MISSING {case.case_id}: no snapshot at {_show(path)}; "
                  f"record it with: live_cases.py --record {case.case_id}")
            continue
        expected = path.read_text(encoding="utf-8").splitlines()
        problems = record.diff_lines(expected, results[case.case_id]["lines"], full=False)
        if problems:
            diffs += 1
            print(f"DIFF {case.case_id}")
            for problem in problems:
                print(f"     {problem}")
        else:
            print(f"ok   {case.case_id}")
    print(f"\n{checked - diffs - missing}/{checked} live cases match"
          + (f" ({len(failures)} could not run)" if failures else ""))
    return 1 if diffs or missing or failures else 0


def markdown() -> int:
    """The per-case section of RECORDED.md, generated from the case table.

    Typed by hand this would drift from what was actually run -- the command
    line in the document has to be the command line the recorder used, so it is
    printed from the same `env_map()` that feeds the child process.
    """
    print("| case | command | engine | event lines | what it freezes |")
    print("| --- | --- | --- | --- | --- |")
    for case in cases():
        print(f"| `{case.case_id}` | `{case.command}` | {case.engine} | "
              f"{_recorded_lines(case.case_id)} | {case.notes} |")
    print()
    for case in cases():
        print(f"### `{case.case_id}`")
        print()
        print(f"    {case.command_line()}")
        print()
    return 0


def _recorded_lines(case_id: str) -> str:
    meta = GOLDEN_DIR / _folder_of(case_id) / f"{case_id}.meta.json"
    if not meta.is_file():
        return "?"
    return str(json.loads(meta.read_text(encoding="utf-8"))["event_lines"])


def _folder_of(case_id: str) -> str:
    for case in cases():
        if case.case_id == case_id:
            return case.folder
    return ""


USAGE = """usage: live_cases.py [--list | --markdown | --record [--force] [case-id ...]]

  (no arguments)   check every case against its snapshot -- reads only, writes nothing
  <case-id> ...    check only those cases

  --record         record the named cases (or all of them) into tests/golden/.
                   Refuses to replace a snapshot that already exists.
  --force          with --record: replace the existing snapshots anyway. This
                   overwrites tests/golden/<command>/<case-id>.ndjson and its
                   .meta.json, and rebuilds index.json -- nothing else is removed.

  --list           print the case table and exit
  --markdown       print the RECORDED.md case section and exit
  -h, --help       print this and exit
"""
# The flags. Anything starting with `-` and not in here is a mistake worth
# naming rather than silently ignoring: a typo'd `--recordd` that recorded
# nothing is exactly the kind of accident this tool no longer allows.
FLAGS = ("--record", "--force", "--list", "--markdown", "-h", "--help")


def main(argv=None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if "-h" in argv or "--help" in argv:
        print(USAGE, end="")
        return 0
    if "--list" in argv:
        for case in cases():
            print(f"{case.case_id:28s} {case.command:8s} {case.engine}")
        return 0
    if "--markdown" in argv:
        return markdown()

    known = {case.case_id for case in cases()}
    selected = [arg for arg in argv if not arg.startswith("-")]
    unknown = [arg for arg in argv if arg.startswith("-") and arg not in FLAGS]
    if unknown:
        print(f"unknown option(s): {' '.join(unknown)}", file=sys.stderr)
        print(USAGE, end="", file=sys.stderr)
        return 2
    missing = [case_id for case_id in selected if case_id not in known]
    if missing:
        print(f"no such case: {', '.join(missing)}", file=sys.stderr)
        print(USAGE, end="", file=sys.stderr)
        return 2
    if "--force" in argv and "--record" not in argv:
        print("--force only means something with --record", file=sys.stderr)
        print(USAGE, end="", file=sys.stderr)
        return 2

    problems = check_tools()
    if problems:
        for problem in problems:
            print(f"cannot run: {problem}", file=sys.stderr)
        return 2

    only = selected or None
    if "--record" in argv:
        return record_live(only=only, force="--force" in argv)
    return compare_live(only=only)


if __name__ == "__main__":
    raise SystemExit(main())
