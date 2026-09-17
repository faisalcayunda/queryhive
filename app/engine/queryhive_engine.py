"""JSON-event command line engine for QueryHive.

    python3 -s -u queryhive_engine.py db_drivers what drivers exist, and their levels
    python3 -s -u queryhive_engine.py test      connect and count the top level
    python3 -s -u queryhive_engine.py catalogs  list the driver's top level
    python3 -s -u queryhive_engine.py schemas   list schemas of DB_DATABASE
    python3 -s -u queryhive_engine.py tables    list tables of the level above
    python3 -s -u queryhive_engine.py export    stream SQL into one of nine formats
    python3 -s -u queryhive_engine.py to_table  let the database write SQL into a table

The desktop app runs this script as a child process and reads the protocol off
its stdout: every stdout line is exactly one compact JSON object and nothing
else, flushed as soon as it is written. Every failure -- a usage error, a bad
setting, a connection that will not open -- is one {"event": "error", ...} line
and exit code 1. Tracebacks and all logging go to stderr, so a caller can parse
stdout without ever filtering it.

Settings are environment variables only; argv is read only for the command
name, so a secret never shows up in a process listing. The events:

    db_drivers drivers                               (kind, label, default_port, levels)
    test     test                                    (ok, catalog_count, host, user)
    catalogs catalogs                                (names)
    schemas  schemas                                 (names)
    tables   tables                                  (names)
    export   step connect, step write, start, progress..., done
             (or a single error event and exit 1)
    to_table step connect, step write, progress..., done
             (or a single error event and exit 1)

Three drivers sit behind one protocol: `trino` (the default), `postgres` and
`mysql`, chosen by DB_KIND and described by exporter/drivers.py. Every DB_* name
has its old TRINO_* alias and DB_* wins when both are set, so an app that sends
only the parts and an older one that still sends TRINO_* both work. `db_drivers`
reports the object-tree levels each driver actually has -- Trino
catalog/schema/table, Postgres schema/table, MySQL database/table -- so the app
never hard-codes that table.

`catalogs`, `schemas` and `tables` feed the app's object tree and each run one
statement the driver builds: SHOW CATALOGS / SHOW SCHEMAS FROM "<database>" /
SHOW TABLES FROM "<database>"."<schema>" on Trino, the information_schema
queries on Postgres, SHOW DATABASES / SHOW TABLES FROM `database` on MySQL --
in the server's own order, which is never re-sorted. All three report the same
`names` array of strings, so one decoder reads them alike. A command the driver
has no level for is a usage error naming the driver rather than an empty
identifier: `catalogs` does not exist on Postgres (the database is set on the
connection) and `schemas` does not exist on MySQL. `test`'s count is
`catalog_count`, never `catalogs`, which is a list here; it runs whichever
top-level probe the driver has. This mirrors exporter/web.py's
/api/connections/{name}/schemas and .../tables endpoints, which name the catalog
explicitly even though the session carries a default.

`export` streams through exporter.run_export: the connection settings mirror
exporter/cli.py's option handling one for one (a URL first, its parts overridden
by the individual variables, `~` expanded in paths, blank means "unset", a
password implies https and the default port moves to 443), and the opts dict
handed to the writers has exactly the keys cli.py builds.

`to_table` is the opposite trade: the database runs the SELECT and commits the
result itself, so no row ever comes back to this process. It never opens a
QueryStream (which rejects DDL/DML by design) and never fetches. SQL/SQL_PATH
resolve exactly as `export`'s do, TARGET_CATALOG/TARGET_SCHEMA/TARGET_TABLE name
the table to write, and WRITE_MODE is `create` (CTAS), `replace` (DROP TABLE IF
EXISTS then CTAS) or `append` (INSERT INTO ... SELECT); any part the driver uses
being blank, or an unknown mode, is a usage error before the network is touched.
The driver decides how many of the three parts it has a level for: Postgres
ignores TARGET_CATALOG, MySQL ignores TARGET_SCHEMA. The `write` step is emitted
once the connection is open and the first statement is next, so a `connect` that
never succeeds leaves exactly one error event behind it and no `write`.
Progress comes from the client's stats callback -- attached to each statement's
cursor, because in trino 0.339.0 that is where the client takes it -- reporting
writtenRows when it reports one, otherwise processedRows, through the same
throttle as `export`, with a final progress event whenever the last emitted
value is not the row count the `done` event carries. psycopg and pymysql have no
stats callback at all, so those runs emit only that one final `progress` line,
carrying whatever `cursor.rowcount` said (`-1` when the server reported none),
and never a tick from a coordinator.
`done` adds `table` (as the driver writes it), `mode`, `query_id` and
`cancelled`; `rows` is cursor.rowcount when the server reported one (>= 0),
otherwise the last progress value, otherwise -1, which is never turned into a
made-up number. A cancelled write is not a failure: the run still emits `done`
with `cancelled: true` and a warning saying the table is now uncertain, and
exits 0. A real failure is one `error` event and exit 1, and it carries a
`warnings` array whenever there is something the caller must still hear -- a
failed `replace` has already dropped the old table by then.

The engine is a sibling of scripts/iceberg_importer/importer.py, which speaks
the same protocol for imports; this follows its structure, its emit() helper
and its exit-code discipline.
"""

from __future__ import annotations

import json
import logging
import os
import signal
import sys
import threading
import time
import traceback
from pathlib import Path

# The app copies this script to <bundle>/engine/queryhive_engine.py and the
# package to <bundle>/engine/exporter/, so `import exporter` needs the script's
# own directory on sys.path; its parent covers a bundle that ships the package
# one level up, and the grandparent covers this checkout, where the script sits
# in app/engine/ and the package in the repository root. Only a directory that
# really holds the package is added, all of them resolved from __file__ (never
# from the working directory), so the script works from anywhere.
_HERE = os.path.dirname(os.path.abspath(__file__))
for _candidate in reversed(
    (_HERE, os.path.dirname(_HERE), os.path.dirname(os.path.dirname(_HERE)))
):
    if os.path.isdir(os.path.join(_candidate, "exporter")) and _candidate not in sys.path:
        sys.path.insert(0, _candidate)


def emit(event, **fields):
    """One compact JSON object per stdout line, flushed as it is written."""
    print(json.dumps({"event": event, **fields}, ensure_ascii=False, default=str), flush=True)


# A bundle that lost its exporter package is still a failure the caller should
# hear about in the protocol, not as a bare traceback on stderr.
try:
    from exporter.drivers import (  # noqa: E402
        DRIVERS,
        KINDS,
        DatabaseConfig,
        driver_of,
    )
    from exporter.export import bundle, run_export  # noqa: E402
    from exporter.source import describe_error  # noqa: E402
    from exporter.to_table import (  # noqa: E402
        CANCEL_WARNING,
        PROGRESS_EVERY as TO_TABLE_PROGRESS_EVERY,
        TableExportError,
        export_to_table,
    )
    from exporter.writers import WRITERS  # noqa: E402
except Exception as _import_error:
    traceback.print_exc()
    emit("error", message=f"{type(_import_error).__name__}: {_import_error}")
    raise SystemExit(1)


PROGRESS_EVERY = 1_000  # rows: no time throttle may swallow a whole 1000 rows

# One rule, two homes: the export path throttles in-process, to_table's lives
# beside the stats reader it floors. They must stay the same number, so a change
# to one that forgets the other fails here instead of quietly changing behaviour.
assert TO_TABLE_PROGRESS_EVERY == PROGRESS_EVERY, "progress floors have drifted apart"

# Set by the SIGTERM/SIGINT handlers below and read by run_export's cancel
# callback. One process runs one command, so one flag is enough.
CANCEL_REQUESTED = threading.Event()


def _request_cancel(signum, frame):
    CANCEL_REQUESTED.set()


def _install_cancel_handlers():
    """Route SIGTERM/SIGINT to the cancel flag instead of killing the process.

    A cancelled export still gets to emit its `done` event and close the files
    it already wrote. signal.signal refuses outside the main thread with
    ValueError, which is swallowed so importing this module can never crash a
    host that loads it from a worker thread.
    """
    for sig in (signal.SIGTERM, signal.SIGINT):
        try:
            signal.signal(sig, _request_cancel)
        except ValueError:
            pass


_install_cancel_handlers()


def settings():
    """Every setting comes from the environment; nothing is read from disk."""
    return os.environ


# --------------------------------------------------------------------------- #
# environment readers
# --------------------------------------------------------------------------- #

_FALSE = frozenset({"0", "false", "no", "off"})
_TRUE = frozenset({"1", "true", "yes", "on"})


def _raw(env, key, default=""):
    """The value verbatim: a padding space may be part of a name or a delimiter."""
    value = env.get(key)
    return default if value is None or value == "" else value


def _value(env, key, default=""):
    """The value stripped, with blank meaning "unset" (the app sends blanks)."""
    value = _raw(env, key, "").strip()
    return default if not value else value


def _int(env, key, default):
    raw = _value(env, key)
    if not raw:
        return default
    try:
        return int(raw)
    except ValueError:
        raise ValueError(f"{key} must be a whole number, got {raw!r}")


def _flag(env, key, default=False):
    """A boolean setting: 1/true/yes/on and 0/false/no/off, case-insensitive.

    A blank value (or anything else unrecognized) keeps `default`, so only the
    documented spellings ever change what the caller asked for.
    """
    raw = _value(env, key)
    if not raw:
        return default
    low = raw.lower()
    if low in _FALSE:
        return False
    if low in _TRUE:
        return True
    return default


# --------------------------------------------------------------------------- #
# connection and query settings
# --------------------------------------------------------------------------- #

def build_config(env):
    """Resolve the connection into a DatabaseConfig, whatever the driver is.

    DB_KIND picks the driver; DB_URL carries the whole connection and any
    individual DB_* variable overrides its part. Every DB_* name has its old
    TRINO_* alias (TRINO_URL, TRINO_HOST, TRINO_CATALOG, ...), DB_* wins when
    both are set, and a blank value means "unset" -- so an app that sends only
    the parts, and an older one that still sends TRINO_*, both work. The app
    sends the parts only; DB_URL exists for the CLI and web paths.
    """
    return DatabaseConfig.from_env(env)


def source_sql(env):
    """SQL, or the contents of SQL_PATH; neither being set is a usage error."""
    sql = _raw(env, "SQL")
    if sql.strip():
        return sql
    path = _value(env, "SQL_PATH")
    if not path:
        raise ValueError("SQL or SQL_PATH is required")
    path = Path(path).expanduser()
    try:
        return path.read_text(encoding="utf-8")
    except OSError as exc:
        raise ValueError(f"cannot read SQL_PATH {str(path)!r}: {exc}")


def format_opts(env, name):
    """The writers' options dict, with exactly cli.py's _format_opts keys."""
    return {
        "delimiter": _raw(env, "DELIMITER") or None,
        "encoding": _value(env, "ENCODING", "utf-8"),
        "header": _flag(env, "HEADER", True),
        "bom": _flag(env, "BOM", False),
        "null_text": _raw(env, "NULL_TEXT"),
        "jsonl": _flag(env, "JSONL", False),
        "sql_table": _raw(env, "SQL_TABLE") or None,
        "sheet": _raw(env, "SHEET", "Sheet1"),
        "dbf_char_width": _int(env, "DBF_CHAR_WIDTH", 254),
        "dbf_encoding": _value(env, "DBF_ENCODING", "cp1252"),
        "title": name,
    }


def make_progress(min_interval_ms):
    """A throttled progress reporter, plus the state it tracks.

    At most one event per `min_interval_ms`, and never fewer than one per whole
    1000 rows: a long query that returns rows faster than the interval would
    otherwise show nothing at all until it finished. The caller emits the total
    once more after run_export returns, so `done` is always preceded by a
    progress event carrying the final count.
    """
    state = {"rows": -1, "at": 0.0}

    def progress(rows):
        now = time.monotonic()
        due = state["rows"] < 0 or (now - state["at"]) * 1000.0 >= min_interval_ms
        if due or rows - state["rows"] >= PROGRESS_EVERY:
            state["rows"], state["at"] = rows, now
            emit("progress", rows=rows)

    return progress, state


# --------------------------------------------------------------------------- #
# commands
# --------------------------------------------------------------------------- #

def _with_retries(config, env):
    """Apply RETRIES to a bare-cursor command's config.

    build_config does NOT read RETRIES in this checkout: only the export path
    does, and it hands the value to QueryStream's own retry loop while
    max_attempts keeps its default. Commands that use a bare cursor apply the
    documented setting here instead -- unset keeps whatever build_config
    produced, and test/export are left exactly as they were. Floored at 1:
    RETRIES=0 means "never retry", which is still one attempt, and the trino
    client crashes with UnboundLocalError on max_attempts=0.
    """
    config.max_attempts = max(1, _int(env, "RETRIES", config.max_attempts))
    return config


def _fetch_all(config, sql):
    """Run one statement on one connection and return its rows.

    One connection per command, always closed: the same shape run_test_command
    has always had. The driver is asked for the statement, never assumed.
    """
    conn = config.connect()
    try:
        cursor = conn.cursor()
        try:
            cursor.execute(sql)
            return cursor.fetchall()
        finally:
            cursor.close()
    finally:
        conn.close()


def _show_names(config, sql):
    """Each row's first column as a string, in the server's own order.

    Nothing is sorted (the server's order is the answer) and nothing is
    filtered -- a row the server sent, even a column-less one, must not raise
    IndexError.
    """
    return [str(row[0]) if row else "" for row in _fetch_all(config, sql)]


def _browse_config(env):
    """The config plus the driver, or a usage error before the network."""
    config = _with_retries(build_config(env), env)
    return config, driver_of(config)


def run_test_command(env):
    """Connect and run the driver's own top-level probe.

    `catalog_count` is that probe's row count, named as it always was: SHOW
    CATALOGS on Trino, SHOW DATABASES on MySQL, the schema list on Postgres --
    the top level each driver actually has, so `test` never needs a catalog or
    schema to be configured first.
    """
    config = build_config(env)
    driver = driver_of(config)
    rows = _fetch_all(config, driver.probe_sql(config.database, config.schema))
    emit("test", ok=True, catalog_count=len(rows), host=config.host, user=config.user)


def run_browse_command(env, command):
    """One browse command, with the SQL and its level checks owned by the driver.

    `catalogs`, `schemas` and `tables` all emit the same `names` array of
    strings -- one decoder reads them alike -- while the statement behind each
    one, the settings it needs and the driver that has no such level at all are
    the driver's business. A command the driver cannot answer is a usage error
    naming the driver, decided before anything is opened.
    """
    config, driver = _browse_config(env)
    sql = getattr(driver, f"{command}_sql")(config.database, config.schema)
    emit(command, names=_show_names(config, sql))


def run_catalogs_command(env):
    """The driver's top level: catalogs on Trino, databases on MySQL."""
    run_browse_command(env, "catalogs")


def run_schemas_command(env):
    """The schema level, where the driver has one: Trino and Postgres do."""
    run_browse_command(env, "schemas")


def run_tables_command(env):
    """The tables of the driver's parent level(s), each quoted by the driver."""
    run_browse_command(env, "tables")


def run_db_drivers_command(env):
    """Report every driver: kind, label, default port and object-tree levels.

    The app builds its object tree from this instead of hard-coding the table
    this module used to carry, so adding a driver here is enough for the tree to
    know how many levels it has and how they are named. No settings are read and
    nothing is opened.
    """
    emit(
        "drivers",
        drivers=[
            {
                "kind": DRIVERS[kind].kind,
                "label": DRIVERS[kind].label,
                "default_port": DRIVERS[kind].default_port,
                "levels": list(DRIVERS[kind].levels),
            }
            for kind in KINDS
        ],
    )


def run_export_command(env):
    """Stream one query into the requested format and report the files written."""
    sql = source_sql(env)
    fmt = _value(env, "FORMAT", "csv").lower()
    if fmt not in WRITERS:
        raise ValueError(f"unknown FORMAT {fmt!r}; expected one of {', '.join(WRITERS)}")
    out_dir = Path(os.path.expanduser(_value(env, "OUT_DIR", os.getcwd())))
    name = _raw(env, "NAME", "export")
    rows_per_file = _int(env, "ROWS_PER_FILE", None)
    if rows_per_file is not None and rows_per_file <= 0:
        rows_per_file = None  # blank or 0 means "never split"
    opts = format_opts(env, name)
    batch_size = max(1, _int(env, "BATCH_SIZE", 10_000))
    retries = max(0, _int(env, "RETRIES", 5))
    progress, state = make_progress(_int(env, "PROGRESS_MS", 250))
    zip_result = _flag(env, "ZIP", False)
    config = build_config(env)  # validated before the connect step is announced

    def on_start(columns, query_id):
        # Fires once the cursor is open and the first page exists: the columns
        # and the Trino query id are known, and writing begins right after.
        emit("step", step="write")
        emit(
            "start",
            columns=[{"name": c.name, "type": str(c.type)} for c in columns],
            query_id=query_id,
        )

    emit("step", step="connect")
    result = run_export(
        config, sql, out_dir, name, fmt,
        batch_size=batch_size, retries=retries, opts=opts,
        rows_per_file=rows_per_file, progress=progress,
        cancel=CANCEL_REQUESTED.is_set, on_start=on_start,
    )
    if state["rows"] != result.rows:
        emit("progress", rows=result.rows)  # always end on the true total

    files = [Path(p) for p in result.files]
    if zip_result and files:
        files = [bundle(files, out_dir / f"{name}.zip")]
    emit(
        "done",
        rows=result.rows,
        files=[{"path": os.path.abspath(str(p)), "bytes": os.path.getsize(p)} for p in files],
        warnings=list(result.warnings),
        query_id=result.query_id,
        cancelled=bool(result.cancelled),
    )


def run_to_table_command(env):
    """Have Trino write the query's rows into a table; no row reaches this process.

    The events are `export`'s shape with the file report replaced by the table
    report: the two steps, the throttled progress stream, one `done`. Cancelling
    is not failing -- `done` says so and the exit code stays 0 -- while a real
    failure is one `error` event, raised so main() can add the `warnings` the
    write already earned (a failed `replace` has dropped the old table by then).
    """
    sql = source_sql(env)
    # The config names the driver, and the driver says which of the three
    # TARGET_* settings it actually has a level for: postgres ignores
    # TARGET_CATALOG (the database is the connection), mysql ignores
    # TARGET_SCHEMA (it has no schema level). A part the driver does not use is
    # not required, and is dropped from the statement rather than written.
    config = _with_retries(build_config(env), env)  # validated before the connect step
    driver = driver_of(config)
    target = {
        "TARGET_CATALOG": _value(env, "TARGET_CATALOG"),
        "TARGET_SCHEMA": _value(env, "TARGET_SCHEMA"),
        "TARGET_TABLE": _value(env, "TARGET_TABLE"),
    }
    catalog = target["TARGET_CATALOG"]
    schema = target["TARGET_SCHEMA"]
    table = target["TARGET_TABLE"]
    missing = [key for key in driver.required_targets() if not target[key]]
    if missing:
        raise ValueError(f"{' and '.join(missing)} required to write a table")
    # Read the environment directly, not through _raw/_value: both fold a blank
    # into "unset", and a WRITE_MODE the app sent as "" cannot be defaulted
    # away, because a mode it cannot name is not the mode it asked for. Only an
    # absent key means create. Trino is case-insensitive, so CREATE is create.
    raw_mode = env.get("WRITE_MODE")
    stripped_mode = (raw_mode or "").strip().lower()
    mode = stripped_mode or ("create" if raw_mode is None else "")
    if mode not in ("create", "replace", "append"):
        raise ValueError(f"unknown WRITE_MODE {stripped_mode!r}; expected create, replace or append")
    progress_ms = _int(env, "PROGRESS_MS", 250)
    progress_state = {"rows": -1}

    def on_progress(rows, state):
        progress_state["rows"] = rows
        emit("progress", rows=rows, state=state)

    emit("step", step="connect")  # before the network is touched
    try:
        result = export_to_table(
            config, sql, catalog, schema, table, mode,
            is_cancelled=CANCEL_REQUESTED.is_set,
            # The connection is open and the first statement is about to run:
            # this is the moment `write` names, and it is the last thing that
            # happens before the coordinator is asked to do the work.
            on_write=lambda: emit("step", step="write"),
            on_progress=on_progress, progress_ms=progress_ms,
        )
    except TableExportError as exc:
        # TableExportError already carries the warnings main() must report; the
        # wrapper only routes them through main()'s one error path while keeping
        # the original class name and cause in the message.
        raise _WarnedExportError(describe_error(exc), exc.warnings, type(exc).__name__) from exc
    if progress_state["rows"] != result.rows:
        emit("progress", rows=result.rows, state=None)  # always end on the true total

    emit(
        "done",
        rows=result.rows,
        table=result.table,
        mode=result.mode,
        query_id=result.query_id,
        cancelled=CANCEL_WARNING in result.warnings,
        warnings=list(result.warnings),
    )


class _WarnedExportError(Exception):
    """An error the caller must also report warnings for.

    main() has one error path for every command, so a failure with more to say
    than a message says it by carrying `warnings` through that path. It reports
    itself under `reported_as` -- the class whose message it is quoting -- rather
    than under this internal wrapper's name.
    """

    def __init__(self, message, warnings, reported_as="TableExportError"):
        super().__init__(message)
        self.warnings = list(warnings)
        self.reported_as = reported_as

    def __str__(self):
        return f"{self.reported_as}: {super().__str__()}"


def main(argv=None):
    argv = list(sys.argv if argv is None else argv)
    commands = {
        # db_drivers leads the usage line on purpose: `main`'s usage message has
        # always ended with the browse-and-write commands in this order, and an
        # app that parses that suffix must keep seeing it. It also costs nothing
        # to ask, so it is the one command a caller can always run first.
        "db_drivers": run_db_drivers_command,
        "test": run_test_command,
        "catalogs": run_catalogs_command,
        "schemas": run_schemas_command,
        "tables": run_tables_command,
        "export": run_export_command,
        "to_table": run_to_table_command,
    }
    if len(argv) != 2 or argv[1] not in commands:
        emit("error", message=f"usage: {argv[0] if argv else __file__} {'|'.join(commands)}")
        return 1
    # Library warnings (retry backoff, cancel, close failures) must never land
    # on stdout, which carries the protocol.
    logging.basicConfig(level=logging.INFO, stream=sys.stderr, format="%(levelname)s %(message)s")
    try:
        commands[argv[1]](settings())
    except Exception as exc:
        traceback.print_exc()
        fields = {}
        warnings = getattr(exc, "warnings", None)
        if warnings:
            # A failure that already changed something out of sight (the DROP of
            # a `replace`) says so here; an error with nothing to add stays the
            # plain one-error shape it always was.
            fields["warnings"] = list(warnings)
        # A _WarnedExportError already quotes the class whose message it is, so
        # its own internal name is never shown; every other exception keeps the
        # "TypeName: message" shape it has always had.
        # `str(exc)` would raise for the client's own cancellation error (see
        # `describe_error`), and an exception thrown here would skip the `error`
        # event altogether -- no JSON on stdout, and a protocol the app cannot
        # parse. This is the last line of defence, so it may not throw.
        message = str(exc) if isinstance(exc, _WarnedExportError) else f"{type(exc).__name__}: {describe_error(exc)}"
        emit("error", message=message, **fields)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
