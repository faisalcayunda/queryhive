#!/usr/bin/env python3
"""Record golden snapshots of the Python engine's stdout protocol.

    python3 tools/golden/record.py            # (re)write tests/golden/
    python3 tools/golden/compare.py           # record in memory and diff

Why this exists
---------------
The Python engine is about to be deleted. Before it goes, the *decisions* it
makes about rendering -- what a DECIMAL looks like, what a NULL looks like in a
batch, which keys an event carries -- have to be frozen so the Rust engine can
be proved equivalent to them rather than merely similar.

What it records
---------------
The engine's stdout, verbatim, one line per event, for a fixed set of cases.
Nothing else: not stderr, not timing, not the SQL sent to the server (that is
already pinned by tests/test_engine_events.py, which this tool reuses rather
than duplicating).

No database is involved. The cases drive the engine in-process with the fake
cursor from tests/test_engine_events.py, exactly as that file does, so what is
frozen is the engine's own normalisation -- which is precisely the part that has
to survive the migration.

Against a real server
---------------------
This tool never opens a connection, which is its whole point: what it freezes is
normalisation. What a real server *hands* the engine is a different question --
whether a numeric(38,10) really arrives as a Decimal, which type code a driver
reports -- and that one is answered by tools/golden/live_cases.py. It runs the
same engine as a child process against the containers deploy/dev/up.sh starts
and writes its snapshots into the same tree, with the same normalization (see
`normalise`). index.json is rebuilt from every `.meta.json` on disk by
`rebuild_index`, so neither recorder can drop the other's cases.
"""

from __future__ import annotations

import importlib.util
import json
import os
import sys
import tempfile
from dataclasses import dataclass, field
from datetime import date, datetime, time, timedelta, timezone
from decimal import Decimal
from pathlib import Path
from typing import Any, Callable

ROOT = Path(__file__).resolve().parents[2]
GOLDEN_DIR = ROOT / "tests" / "golden"
HARNESS_PATH = ROOT / "tests" / "test_engine_events.py"


def _load_harness():
    """Import tests/test_engine_events.py as a module.

    It is a script, not a package member, and importing it runs its module-level
    work -- installing the trino/psycopg/pymysql import stubs and loading the
    engine -- which is exactly the setup needed here. Its checks only run under
    `__main__`, so importing it does not run the test suite.
    """
    spec = importlib.util.spec_from_file_location("qh_test_harness", HARNESS_PATH)
    module = importlib.util.module_from_spec(spec)
    sys.modules["qh_test_harness"] = module
    spec.loader.exec_module(module)
    return module


harness = _load_harness()

# --------------------------------------------------------------------------- #
# normalisation
# --------------------------------------------------------------------------- #

# Keys whose value cannot be the same twice. Everything else must match exactly:
# an unclassified difference has to fail the comparison, not be smoothed over.
VOLATILE_KEYS = {
    "elapsed_ms": "<TIME>",
    "query_id": "<QUERY_ID>",
}

# A real tmp path differs on every run and would make every diff noisy.
PLACEHOLDERS = ("<TMP>",)


def _normalise_value(key: str, value: Any) -> Any:
    if key in VOLATILE_KEYS:
        return VOLATILE_KEYS[key]
    if key == "files" and isinstance(value, list):
        return [{**entry, "path": _normalise_path(entry.get("path", ""))} for entry in value]
    if isinstance(value, str):
        return _normalise_path(value)
    if isinstance(value, list):
        return [_normalise_value(key, item) for item in value]
    return value


def _normalise_path(text: str) -> str:
    """Replace any tmp path with one token, longest prefix first."""
    for raw in sorted(_TMP_ROOTS, key=len, reverse=True):
        if raw and raw in text:
            text = text.replace(raw, "<TMP>")
    return text


_TMP_ROOTS: set[str] = set()


def add_roots(*paths) -> None:
    """Mask one more root out of every string a case emits.

    `record()` seeds this with the directory its export case writes into;
    `live_cases.py` seeds it with the temp root, because the same normalisation
    has to apply to a run that never passed through this module's own cases.
    """
    _TMP_ROOTS.update(str(path) for path in paths if path)


def normalise(stdout: str) -> list[str]:
    """One normalised JSON line per event, in the engine's own order."""
    lines: list[str] = []
    for line in stdout.splitlines():
        if not line.strip():
            continue
        event = json.loads(line)  # raises if the engine emitted a non-JSON line
        normalised = {key: _normalise_value(key, value) for key, value in event.items()}
        lines.append(json.dumps(normalised, ensure_ascii=False, sort_keys=True))
    return lines


# --------------------------------------------------------------------------- #
# fixtures
# --------------------------------------------------------------------------- #

# The type zoo of blueprint section 1.8, as the Python objects a driver hands the
# engine. Server-side types (NUMERIC(38,10), timestamptz, bytea, ...) arrive here
# as these objects; what is frozen is what the engine does with them.
TYPE_ZOO_ROWS = [
    [
        Decimal("1234567890123456789012345678.1234567890"),  # full i128-range precision
        Decimal("-0.0000000001"),
        Decimal("0"),
        datetime(2026, 1, 31, 12, 0, 0, 123456, tzinfo=timezone(timedelta(hours=7))),
        datetime(2026, 1, 31, 12, 0, 0),  # naive: no zone, must not gain one
        date(2026, 1, 31),
        time(23, 59, 59, 999999),
        timedelta(days=3, hours=4, minutes=5, seconds=6),
        True,
        False,
    ],
    [
        -9223372036854775808,  # i64 min
        18446744073709551615,  # u64 max: MySQL UNSIGNED BIGINT
        1.5,
        -0.0,
        "unicode: \u00e9\u4e2d\U0001f600",
        "tab\tand\nnewline",
        '{"b":1,"a":2}',  # JSON text: key order is the server's, kept as-is
        "[1,null,3]",  # array as text
        b"\x00\x01\xff",  # bytea / BLOB with a NUL and a non-UTF8 byte
        None,
    ],
    [
        "550e8400-e29b-41d4-a716-446655440000",  # UUID as text
        "active",  # enum label, not ordinal
        "(1,2)",  # point/geometry: text if unsupported
        "",  # empty string is not NULL
        "NULL",  # the four characters, not a null
        None,
        None,
        None,
        "  padded  ",
        "\u0000embedded-nul",  # a NUL inside text
    ],
]

TYPE_ZOO_COLUMNS = [
    ("high_precision", "decimal(38,10)"),
    ("tiny_negative", "decimal(38,10)"),
    ("zero_decimal", "decimal(38,10)"),
    ("tz_aware", "timestamp(6) with time zone"),
    ("tz_naive", "timestamp(6)"),
    ("a_date", "date"),
    ("a_time", "time(6)"),
    ("an_interval", "interval day to second"),
    ("a_bool", "boolean"),
    ("another_bool", "boolean"),
]


def _cols(names_and_types):
    """FakeCursor description rows: (name, type, size, size, precision, scale, null_ok)."""
    return [(name, type_name, None, None, None, None, None) for name, type_name in names_and_types]


def _run_trino(rows, columns, command, sql="SELECT * FROM t", rowcount=-1, **env):
    cursor = harness.FakeCursor(rows, _cols(columns) if columns else None, rowcount=rowcount)
    connection = harness.FakeConnection(cursor)
    values = {"TRINO_HOST": "trino.internal", "RETRIES": "0", **env}
    if sql is not None:
        values["SQL"] = sql
    with harness.fake_connect(lambda **kwargs: connection):
        return harness.run_engine(command, **values)


def _run_postgres(rows, columns, command, sql="SELECT * FROM t", **env):
    cursor = harness.FakeCursor(rows, _cols(columns) if columns else None, query_id=None, rowcount=len(rows))
    values = {"DB_KIND": "postgres", "DB_HOST": "pg.internal", "RETRIES": "0", **env}
    if sql is not None:
        values["SQL"] = sql
    with harness.fake_dbapi(harness.psycopg, lambda **kwargs: harness.FakeConnection(cursor)):
        return harness.run_engine(command, **values)


def _run_mysql(rows, columns, command, sql="SELECT * FROM t", **env):
    cursor = harness.FakeCursor(rows, _cols(columns) if columns else None, query_id=None, rowcount=len(rows))
    values = {"DB_KIND": "mysql", "DB_HOST": "mysql.internal", "RETRIES": "0", **env}
    if sql is not None:
        values["SQL"] = sql
    with harness.fake_dbapi(harness.pymysql, lambda **kwargs: harness.FakeConnection(cursor)):
        return harness.run_engine(command, **values)


@dataclass
class Case:
    """One command run, and why it is worth freezing."""

    case_id: str
    command: str
    run: Callable[[], tuple[int, str, str]]
    notes: str
    captures: str = "stdout"
    env: dict = field(default_factory=dict)


def _export_case(out_dir: str):
    def run():
        return _run_trino(
            [[1, "alice"], [2, "bob, inc"], [3, None]],
            [("id", "integer"), ("name", "varchar")],
            "export",
            sql="SELECT * FROM people",
            FORMAT="csv",
            OUT_DIR=out_dir,
            NAME="people",
        )

    return run


def _build_cases() -> list[Case]:
    out_dir = tempfile.mkdtemp(prefix="qh-golden-export-")
    _TMP_ROOTS.add(out_dir)
    _TMP_ROOTS.add(tempfile.gettempdir())

    return [
        Case(
            "drivers",
            "db_drivers",
            lambda: harness.run_engine("db_drivers"),
            "The three drivers and the object-tree levels each declares. No network, no settings.",
        ),
        Case(
            "unknown_command",
            "usage",
            lambda: harness.run_engine("no_such_command"),
            "A usage error is one error event and exit 1 -- never a traceback on stdout.",
        ),
        Case(
            "blank_sql",
            "preview",
            lambda: harness.run_engine("preview", TRINO_HOST="trino.internal"),
            "A missing statement is refused before the network is touched.",
        ),
        Case(
            "connect_failure",
            "test",
            lambda: _refuse_connect(),
            "A refused connection is one error event carrying the reason, and stderr keeps the traceback.",
        ),
        Case(
            "test_probe",
            "test",
            lambda: _run_trino([("1",)], None, "test", sql=None),
            "The connection probe. The count key is `catalog_count`, never `catalogs`.",
        ),
        Case(
            "catalogs",
            "catalogs",
            lambda: _run_trino([("hive",), ("system",)], None, "catalogs", sql=None),
            "Top-level names, in the server's own order -- never re-sorted.",
        ),
        Case(
            "schemas",
            "schemas",
            lambda: _run_trino([("analytics",), ("public",)], None, "schemas", sql=None,
                               TRINO_CATALOG="hive"),
            "Schema names under an explicit catalog.",
        ),
        Case(
            "tables",
            "tables",
            lambda: _run_trino([("people",), ("orders",)], None, "tables", sql=None,
                               TRINO_CATALOG="hive", TRINO_SCHEMA="analytics"),
            "Table names under an explicit catalog and schema.",
        ),
        Case(
            "objects_trino",
            "objects",
            lambda: _run_trino(
                [("people", "BASE TABLE"), ("orders", "BASE TABLE"), ("legacy", None)],
                None, "objects", sql=None, TRINO_CATALOG="hive", TRINO_SCHEMA="analytics",
            ),
            "The driver's own columns, and a NULL cell rendered as an empty string.",
        ),
        Case(
            "type_zoo",
            "preview",
            lambda: _run_trino(TYPE_ZOO_ROWS, TYPE_ZOO_COLUMNS, "preview"),
            "Every hard type: DECIMAL precision, tz-aware and naive timestamps, interval, "
            "bytes, NULL, empty string, embedded NUL, and the four characters NULL.",
        ),
        Case(
            "batching",
            "preview",
            lambda: _run_trino(
                [[n, None if n % 3 == 0 else f"name-{n}"] for n in range(1, 26)],
                [("id", "integer"), ("name", "varchar")],
                "preview", LIMIT="25",
            ),
            "More rows than one batch, so the batch boundary itself is frozen.",
        ),
        Case(
            "limit_truncation",
            "preview",
            lambda: _run_trino(
                [[n] for n in range(1, 11)], [("id", "integer")], "preview", LIMIT="3",
            ),
            "A row cap that ends on a page boundary: `truncated` says whether more exists.",
        ),
        Case(
            "no_rows",
            "preview",
            lambda: _run_trino([], [("id", "integer")], "preview"),
            "An empty result still emits columns and done.",
        ),
        Case(
            "count",
            "count",
            lambda: _run_trino([(4321,)], [("_col0", "bigint")], "count"),
            "The true row count from the wrapped statement.",
        ),
        Case(
            "explain",
            "explain",
            lambda: _run_trino(
                [["Fragment 0", "TableScan", "1000", "1500"]],
                [("Fragment", "varchar"), ("Operation", "varchar"),
                 ("rows", "bigint"), ("bytes", "bigint")],
                "explain",
            ),
            "A plan has no `truncated` key at all -- its absence is the contract.",
        ),
        Case(
            "export_csv",
            "export",
            _export_case(out_dir),
            "The CSV writer's bytes: header, quoting, and a NULL rendered as empty.",
        ),
        Case(
            "to_table_create",
            "to_table",
            lambda: _run_trino(
                [], None, "to_table",
                sql="SELECT * FROM people", rowcount=5, TARGET_CATALOG="hive",
                TARGET_SCHEMA="analytics", TARGET_TABLE="people_copy",
            ),
            "CTAS: which statement is built, and what `done` reports.",
        ),
        Case(
            "postgres_schemas",
            "schemas",
            lambda: _run_postgres([("public",), ("analytics",)], None, "schemas", sql=None),
            "Postgres has no catalog level; schema listing is its top level.",
        ),
        Case(
            "postgres_objects",
            "objects",
            lambda: _run_postgres(
                [("people", 16385, "faisal", "")], None, "objects", sql=None,
                DB_SCHEMA="analytics",
            ),
            "Postgres declares four object columns: Name, OID, Owner, ACL.",
        ),
        Case(
            "mysql_catalogs",
            "catalogs",
            lambda: _run_mysql([("information_schema",), ("sips",)], None, "catalogs", sql=None),
            "MySQL calls its top level `catalogs` on the wire, and means databases.",
        ),
        Case(
            "mysql_objects",
            "objects",
            lambda: _run_mysql(
                [("people", "InnoDB", 42, "the people table")], None, "objects",
                sql=None, DB_DATABASE="sips",
            ),
            "MySQL declares four object columns: Name, Engine, Rows, Comment.",
        ),
    ]


def _refuse_connect():
    def refuse(**kwargs):
        raise OSError("connection refused")

    with harness.fake_connect(refuse):
        return harness.run_engine("test", TRINO_HOST="trino.invalid", RETRIES="0")


# --------------------------------------------------------------------------- #
# record / collect
# --------------------------------------------------------------------------- #

def collect() -> dict[str, dict]:
    """Run every case and return {case_id: {meta, lines}}; nothing is written."""
    cases = _build_cases()
    results: dict[str, dict] = {}
    for case in cases:
        code, stdout, _stderr = case.run()
        results[case.case_id] = {
            "meta": {
                "case": case.case_id,
                "command": case.command,
                "exit_code": code,
                "notes": case.notes,
                "captures": case.captures,
            },
            "lines": normalise(stdout),
        }
    return results


def rebuild_index(destination: Path = GOLDEN_DIR) -> None:
    """Write index.json from every `.meta.json` in the tree.

    The index is a view of what is on disk rather than of what this run
    happened to produce, so the in-process cases and the ones recorded against
    real servers by `live_cases.py` share one index without either recorder
    knowing about the other's case list.
    """
    index = [
        json.loads(path.read_text(encoding="utf-8"))
        for path in sorted(destination.glob("*/*.meta.json"))
    ]
    (destination / "index.json").write_text(
        json.dumps(sorted(index, key=lambda entry: entry["case"]), indent=2,
                   ensure_ascii=False, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def record(destination: Path = GOLDEN_DIR) -> int:
    """(Re)write this module's cases; leave every other snapshot alone.

    Deliberately not a wipe of `destination`: `live_cases.py` keeps its cases in
    the same tree, and re-recording the fake-cursor cases must not delete a
    snapshot that took a server to produce. A case that has been renamed or
    removed therefore leaves its old `.ndjson` behind -- which is visible rather
    than silent, because the Rust harness fails on any snapshot it cannot
    classify.
    """
    results = collect()
    destination.mkdir(parents=True, exist_ok=True)
    for case_id, payload in results.items():
        folder = destination / payload["meta"]["command"]
        folder.mkdir(parents=True, exist_ok=True)
        (folder / f"{case_id}.ndjson").write_text(
            "\n".join(payload["lines"]) + "\n", encoding="utf-8"
        )
        meta = dict(payload["meta"])
        meta["event_lines"] = len(payload["lines"])
        (folder / f"{case_id}.meta.json").write_text(
            json.dumps(meta, indent=2, ensure_ascii=False, sort_keys=True) + "\n",
            encoding="utf-8",
        )
    rebuild_index(destination)
    return len(results)


def main(argv=None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    destination = Path(argv[0]) if argv else GOLDEN_DIR
    count = record(destination)
    print(f"recorded {count} cases into {destination.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
