"""End-to-end checks for the QueryHive engine's stdout JSON-event protocol.

Run: python3 tests/test_engine_events.py

Nothing here talks to Trino: the DBAPI connect that exporter.source reaches for
is replaced with a fake whose cursor pages through a few batches of rows, so the
real QueryStream, retry, export and writer code all run against known data. The
engine is driven in-process exactly as the app drives it out of process -- one
command name, settings in the environment -- and its stdout is parsed as JSON.
"""

from __future__ import annotations

import contextlib
import csv
import importlib.util
import io
import json
import os
import signal
import sys
import tempfile
import traceback
import types
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

QUERY_ID = "20260131_120000_00000_abcde"


def _install_trino_stub():
    """The import-time surface of the trino package, for a checkout without it.

    exporter.source imports trino at module scope (for dbapi.connect, auth and
    the exception classes it retries on). The app bundles the real package into
    its frozen build; a bare checkout may not have it. The tests never speak to
    a server, so a stub keeps the real engine code importable -- and it is the
    stub's dbapi.connect that gets patched below.
    """
    print("note: trino is not installed; using a local stub for the import surface", file=sys.stderr)

    class TrinoError(Exception):
        pass

    class HttpError(TrinoError):
        pass

    exceptions = types.ModuleType("trino.exceptions")
    for name in ("TrinoError", "HttpError", "Http502Error", "Http503Error", "Http504Error",
                 "TrinoExternalError", "TrinoConnectionError", "TrinoUserError"):
        base = HttpError if name.startswith("Http") and name != "HttpError" else TrinoError
        setattr(exceptions, name, type(name, (base,), {}))

    dbapi = types.ModuleType("trino.dbapi")

    def connect(**kwargs):  # pragma: no cover - always replaced by the fake
        raise AssertionError("the fake connect was not installed")

    dbapi.connect = connect

    auth = types.ModuleType("trino.auth")
    auth.BasicAuthentication = lambda user, password: ("BasicAuthentication", user, password)

    module = types.ModuleType("trino")
    module.dbapi, module.auth, module.exceptions = dbapi, auth, exceptions
    sys.modules.update({
        "trino": module, "trino.dbapi": dbapi, "trino.auth": auth, "trino.exceptions": exceptions,
    })
    return module


try:
    import trino
    import trino.dbapi
except ImportError:
    trino = _install_trino_stub()

ENGINE_PATH = ROOT / "app" / "engine" / "queryhive_engine.py"
_spec = importlib.util.spec_from_file_location("queryhive_engine", ENGINE_PATH)
engine = importlib.util.module_from_spec(_spec)
sys.modules["queryhive_engine"] = engine
_spec.loader.exec_module(engine)

# Importing the engine installs its SIGTERM/SIGINT cancel handlers. Hand them
# back to the interpreter so Ctrl-C still interrupts this test run.
try:
    signal.signal(signal.SIGINT, signal.default_int_handler)
    signal.signal(signal.SIGTERM, signal.SIG_DFL)
except ValueError:  # pragma: no cover - only reachable off the main thread
    pass


# --------------------------------------------------------------------------- #
# fakes
# --------------------------------------------------------------------------- #

class FakeCursor:
    """The slice of a DBAPI cursor that QueryStream and the engine touch."""

    def __init__(self, rows=(), columns=None, query_id=QUERY_ID, rowcount=-1):
        self._rows = [list(row) for row in rows]
        self._pos = 0
        self._columns = columns
        self._query_id = query_id
        self.description = None  # trino only knows the columns once a page arrived
        self.query_id = None
        self.rowcount = rowcount  # trino: update_count, or -1 when it did not say
        self.arraysize = 0
        self.statements = []
        self.fetches = 0
        self.on_fetch = None
        self.on_execute = None  # a to_table check drives the client's stats callback
        self.raise_on_execute = None  # the failure SQL, as the coordinator would send it
        self.cancelled = False
        self.cancels = 0
        self.closed = False

    def execute(self, sql):
        self.statements.append(sql)
        self.query_id = self._query_id  # trino knows it as soon as the query is submitted
        if self.on_execute is not None:
            self.on_execute(self)
        if self.raise_on_execute is not None:
            raise self.raise_on_execute

    def fetchmany(self, size):
        self.fetches += 1
        if self.description is None:
            self.description = self._columns  # trino only fills this in with the first page
        if self.on_fetch is not None:
            self.on_fetch(self)
        page = self._rows[self._pos:self._pos + size]
        self._pos += len(page)
        return page

    def fetchall(self):
        return list(self._rows)

    def cancel(self):
        self.cancelled = True
        self.cancels += 1

    def close(self):
        self.closed = True


class FakeConnection:
    """One connection; `source` is either the cursor to hand out or a factory.

    Shaped like the real client (trino 0.339.0), not like whatever the engine
    happens to call: `cursor()` takes `stats_callback` as a keyword and hands it
    to the cursor it builds, while `dbapi.connect()`/`Connection.__init__` take
    no `stats_callback` at all. That is exactly the mismatch the stub used to
    hide -- a keyword the real client rejects was accepted by a permissive
    fake -- so the fake records what it was called with and the checks assert on
    it.

    The export checks want one known cursor, so they pass it directly. A
    to_table run issues one cursor per statement, so those checks pass a factory
    and get a fresh cursor each time -- the only way a `replace` check can see
    the DROP and the CREATE as two statements.
    """

    def __init__(self, source):
        self._source = source
        self.cursors = []
        self.cursor_calls = []
        self.closed = False

    def cursor(self, cursor_style="row", legacy_primitive_types=None, stats_callback=None):
        self.cursor_calls.append({
            "cursor_style": cursor_style,
            "legacy_primitive_types": legacy_primitive_types,
            "stats_callback": stats_callback,
        })
        cursor = self._source() if callable(self._source) else self._source
        cursor.stats_callback = stats_callback  # dbapi forwards it into the cursor
        if cursor not in self.cursors:
            self.cursors.append(cursor)
        return cursor

    def close(self):
        self.closed = True


class StrictConnection:
    """A connection that refuses any keyword the real client would refuse.

    `dbapi.connect()` is a bare `return Connection(*args, **kwargs)`, and
    `Connection.__init__` has no `stats_callback`. This fake has the same
    surface, so passing the callback the wrong way raises here instead of only
    in a live run against the bundled interpreter.
    """

    _ALLOWED = frozenset({
        "host", "port", "user", "source", "catalog", "schema", "session_properties",
        "http_headers", "http_scheme", "auth", "extra_credential", "max_attempts",
        "request_timeout", "isolation_level", "verify", "http_session", "client_tags",
        "legacy_primitive_types", "legacy_prepared_statements", "roles", "timezone",
        "encoding", "heartbeat_interval",
    })
    _CURSOR_ALLOWED = frozenset({"cursor_style", "legacy_primitive_types", "stats_callback"})

    def __init__(self, cursor_factory, **kwargs):
        unknown = set(kwargs) - self._ALLOWED
        if unknown:
            raise TypeError(
                f"Connection.__init__() got an unexpected keyword argument {sorted(unknown)[0]!r}"
            )
        self._factory = cursor_factory
        self.cursors = []
        self.cursor_calls = []
        self.closed = False

    def cursor(self, **kwargs):
        unknown = set(kwargs) - self._CURSOR_ALLOWED
        if unknown:
            raise TypeError(f"cursor() got an unexpected keyword argument {sorted(unknown)[0]!r}")
        cursor = self._factory()
        cursor.stats_callback = kwargs.get("stats_callback")
        self.cursors.append(cursor)
        self.cursor_calls.append(dict(kwargs))
        return cursor

    def close(self):
        self.closed = True


@contextlib.contextmanager
def fake_connect(factory):
    """Replace exporter.source's DBAPI connect for the duration of one check."""
    original = trino.dbapi.connect
    trino.dbapi.connect = factory
    try:
        yield
    finally:
        trino.dbapi.connect = original


ENGINE_KEYS = (
    "TRINO_URL", "TRINO_HOST", "TRINO_PORT", "TRINO_USER", "TRINO_PASSWORD", "TRINO_CATALOG",
    "TRINO_SCHEMA", "TRINO_INSECURE", "SQL", "SQL_PATH", "FORMAT", "OUT_DIR", "NAME", "ZIP",
    "BATCH_SIZE", "ROWS_PER_FILE", "RETRIES", "DELIMITER", "ENCODING", "HEADER", "BOM",
    "NULL_TEXT", "JSONL", "SQL_TABLE", "SHEET", "DBF_CHAR_WIDTH", "DBF_ENCODING", "PROGRESS_MS",
    "TARGET_CATALOG", "TARGET_SCHEMA", "TARGET_TABLE", "WRITE_MODE",
)


@contextlib.contextmanager
def env_only(values):
    """Run with exactly these engine settings, whatever the test environment has."""
    saved = {key: os.environ.get(key) for key in ENGINE_KEYS}
    for key in ENGINE_KEYS:
        os.environ.pop(key, None)
    os.environ.update({key: str(value) for key, value in values.items()})
    try:
        yield
    finally:
        for key in ENGINE_KEYS:
            os.environ.pop(key, None)
        for key, value in saved.items():
            if value is not None:
                os.environ[key] = value


def run_engine(command, **values):
    """Run one command as the app does; return (exit code, stdout, stderr)."""
    out, err = io.StringIO(), io.StringIO()
    with env_only(values), contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        engine.CANCEL_REQUESTED.clear()
        code = engine.main([ENGINE_PATH.name, command])
    return code, out.getvalue(), err.getvalue()


def events_of(stdout):
    return [json.loads(line) for line in stdout.splitlines()]


def event_names(events):
    return [event["event"] for event in events]


def only_event(events, name):
    found = [event for event in events if event["event"] == name]
    assert len(found) == 1, f"expected exactly one {name!r} event, got {event_names(events)}"
    return found[0]


# --------------------------------------------------------------------------- #
# checks
# --------------------------------------------------------------------------- #

COLUMNS = [
    ("id", 23, None, None, None, None, None),
    ("name", 25, None, None, None, None, None),
]
ROWS = [[1, "alice"], [2, "bob, inc"], [3, None]]


def check_empty_result_is_still_a_done_event():
    """An ExportResult with no files at all must still emit a well-formed done."""
    from exporter.export import ExportResult

    original = engine.run_export
    engine.run_export = lambda *args, **kwargs: ExportResult()
    try:
        with tempfile.TemporaryDirectory() as tmp:
            code, out, _err = run_engine(
                "export", TRINO_HOST="trino.internal", SQL="SELECT 1", OUT_DIR=tmp, RETRIES="0",
            )
    finally:
        engine.run_export = original
    assert code == 0, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    assert events[-1] == {
        "event": "done", "rows": 0, "files": [], "warnings": [], "query_id": None,
        "cancelled": False,
    }, events
    assert events[-2] == {"event": "progress", "rows": 0}, events


def check_export_success():
    """A full run: the event order, the row/byte counts and the real file."""
    cursor = FakeCursor(ROWS, COLUMNS)
    with tempfile.TemporaryDirectory() as tmp:
        with fake_connect(lambda **kwargs: FakeConnection(cursor)):
            code, out, _err = run_engine(
                "export", TRINO_HOST="trino.internal", TRINO_USER="analyst",
                SQL="SELECT id, name FROM people", OUT_DIR=tmp, NAME="people",
                FORMAT="csv", BATCH_SIZE="2", RETRIES="0",
            )
        assert code == 0, f"exit {code}, stdout={out!r}"
        events = events_of(out)
        names = event_names(events)

        steps = [event["step"] for event in events if event["event"] == "step"]
        assert steps == ["connect", "write"], steps
        assert names.index("step") < names.index("start"), names
        assert cursor.statements == ["SELECT id, name FROM people"], cursor.statements

        start = only_event(events, "start")
        assert start["columns"] == [{"name": "id", "type": "23"}, {"name": "name", "type": "25"}], start
        assert start["query_id"] == QUERY_ID, start

        done = events[-1]
        assert done["event"] == "done", names
        assert done["rows"] == 3 and done["cancelled"] is False, done
        assert done["warnings"] == [] and done["query_id"] == QUERY_ID, done

        assert len(done["files"]) == 1, done["files"]
        written = Path(done["files"][0]["path"])
        assert os.path.isabs(str(written)), written
        assert written == Path(tmp) / "people.csv", written
        assert done["files"][0]["bytes"] == os.path.getsize(written) > 0, done["files"]

        with written.open(newline="", encoding="utf-8") as handle:
            assert list(csv.reader(handle)) == [
                ["id", "name"], ["1", "alice"], ["2", "bob, inc"], ["3", ""],
            ]


def check_export_zip():
    """ZIP=1 reports the single archive, never the parts it holds."""
    cursor = FakeCursor(ROWS, COLUMNS)
    with tempfile.TemporaryDirectory() as tmp:
        with fake_connect(lambda **kwargs: FakeConnection(cursor)):
            code, out, _err = run_engine(
                "export", TRINO_HOST="trino.internal", SQL="SELECT 1", OUT_DIR=tmp,
                NAME="bundle", FORMAT="csv", ZIP="1", BATCH_SIZE="2", RETRIES="0",
            )
        assert code == 0, f"exit {code}, stdout={out!r}"
        done = events_of(out)[-1]
        assert done["event"] == "done", done
        assert done["rows"] == 3, done
        assert len(done["files"]) == 1, done["files"]
        archive = Path(done["files"][0]["path"])
        assert archive == Path(tmp) / "bundle.zip", archive
        assert done["files"][0]["bytes"] == os.path.getsize(archive) > 0, done["files"]
        import zipfile

        assert zipfile.ZipFile(archive).namelist() == ["bundle.csv"]


def check_cancel_stops_early():
    """A cancel arriving with the second page still produces a well-formed done."""
    cursor = FakeCursor([[i] for i in range(25)], [("id", 23, None, None, None, None, None)])

    def on_fetch(current):
        if current.fetches == 2:  # the page after the one QueryStream pins first
            engine.CANCEL_REQUESTED.set()

    cursor.on_fetch = on_fetch
    with tempfile.TemporaryDirectory() as tmp:
        with fake_connect(lambda **kwargs: FakeConnection(cursor)):
            code, out, _err = run_engine(
                "export", TRINO_HOST="trino.internal", SQL="SELECT id FROM big", OUT_DIR=tmp,
                NAME="partial", FORMAT="csv", BATCH_SIZE="10", RETRIES="0",
            )
        assert code == 0, f"exit {code}, stdout={out!r}"
        events = events_of(out)
        done = events[-1]
        assert done["event"] == "done" and done["cancelled"] is True, done
        assert 0 <= done["rows"] < 25, done
        assert done["files"] and Path(done["files"][0]["path"]).exists(), done["files"]
        assert done["files"][0]["bytes"] == os.path.getsize(done["files"][0]["path"]), done["files"]


def check_connection_failure():
    """A connection that will not open is one error event and exit 1."""

    def refuse(**kwargs):
        raise OSError("connection refused")

    with tempfile.TemporaryDirectory() as tmp:
        with fake_connect(refuse):
            code, out, err = run_engine(
                "export", TRINO_HOST="trino.invalid", SQL="SELECT 1", OUT_DIR=tmp, RETRIES="0",
            )
    assert code == 1, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    assert events[-1]["event"] == "error", events
    assert "done" not in event_names(events), events
    assert "OSError" in events[-1]["message"] and "connection refused" in events[-1]["message"]
    assert "Traceback" in err, err  # the traceback belongs on stderr, never stdout
    assert "Traceback" not in out, out


def check_blank_sql_is_usage_error():
    """No SQL at all fails before anything touches the network."""
    with tempfile.TemporaryDirectory() as tmp:
        code, out, _err = run_engine(
            "export", TRINO_HOST="trino.invalid", SQL="", SQL_PATH="", OUT_DIR=tmp,
        )
    assert code == 1, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    assert event_names(events) == ["error"], events
    assert "SQL" in events[0]["message"], events[0]


def check_stdout_is_json_only():
    """Every stdout line of a multi-batch run is one compact JSON object."""
    cursor = FakeCursor([[i, f"row {i}"] for i in range(30)], COLUMNS)
    with tempfile.TemporaryDirectory() as tmp:
        with fake_connect(lambda **kwargs: FakeConnection(cursor)):
            code, out, _err = run_engine(
                "export", TRINO_HOST="trino.internal", SQL="SELECT * FROM t", OUT_DIR=tmp,
                NAME="lines", FORMAT="json", BATCH_SIZE="7", RETRIES="0", PROGRESS_MS="0",
            )
    assert code == 0, f"exit {code}, stdout={out!r}"
    lines = out.splitlines()
    assert lines, "no stdout at all"
    for line in lines:
        assert line.strip() == line and line, f"padding or a blank line in {line!r}"
        payload = json.loads(line)  # raises if any line is not exactly one JSON object
        assert isinstance(payload, dict) and isinstance(payload.get("event"), str), payload
    assert lines[-1].startswith('{"event": "done"'), lines[-1]
    assert "progress" in event_names(events_of(out)), out


def check_test_command():
    """`test` connects, runs SHOW CATALOGS and reports the catalog count."""
    cursor = FakeCursor([("system",), ("hive",), ("memory",)])
    connection = FakeConnection(cursor)
    with fake_connect(lambda **kwargs: connection):
        code, out, _err = run_engine(
            "test", TRINO_HOST="trino.internal", TRINO_USER="analyst", TRINO_INSECURE="1",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert events_of(out) == [{
        "event": "test", "ok": True, "catalog_count": 3, "host": "trino.internal",
        "user": "analyst",
    }], out
    assert cursor.statements == ["SHOW CATALOGS"], cursor.statements
    assert cursor.closed and connection.closed, "the engine left a handle open"


def check_test_event_reports_catalog_count():
    """`test` names its count catalog_count; a list key never shares that name.

    `catalogs` is a list on the browse event, so a decoder must never meet an
    integer under the same key: the collision this pins down.
    """
    cursor = FakeCursor([("system",), ("hive",)])
    with fake_connect(lambda **kwargs: FakeConnection(cursor)):
        code, out, _err = run_engine("test", TRINO_HOST="trino.internal")
    assert code == 0, f"exit {code}, stdout={out!r}"
    event = only_event(events_of(out), "test")
    assert "catalog_count" in event, event
    assert event["catalog_count"] == 2, event
    assert "catalogs" not in event, event


def check_catalogs_command():
    """`catalogs` emits the coordinator's names, unsorted, and closes up."""
    cursor = FakeCursor([("tpch",), ("hive",), ("system",)])  # deliberately not alphabetical
    connection = FakeConnection(cursor)
    with fake_connect(lambda **kwargs: connection):
        code, out, _err = run_engine("catalogs", TRINO_HOST="trino.internal")
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert events_of(out) == [{
        "event": "catalogs", "names": ["tpch", "hive", "system"],
    }], out
    assert cursor.statements == ["SHOW CATALOGS"], cursor.statements
    assert cursor.closed and connection.closed, "the engine left a handle open"


def check_schemas_command():
    """`schemas` runs SHOW SCHEMAS FROM the configured catalog."""
    cursor = FakeCursor([("analytics",), ("bronze",), ("information_schema",)])
    connection = FakeConnection(cursor)
    seen = {}

    def connect(**kwargs):
        seen.update(kwargs)
        return connection

    with fake_connect(connect):
        code, out, _err = run_engine(
            "schemas", TRINO_HOST="trino.internal", TRINO_CATALOG="hive", RETRIES="7",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert events_of(out) == [{
        "event": "schemas", "names": ["analytics", "bronze", "information_schema"],
    }], out
    assert cursor.statements == ['SHOW SCHEMAS FROM "hive"'], cursor.statements
    assert seen.get("catalog") == "hive", seen  # the connection carries the default too
    assert seen.get("max_attempts") == 7, seen  # RETRIES reaches the connector
    assert cursor.closed and connection.closed, "the engine left a handle open"


def check_tables_command():
    """`tables` runs SHOW TABLES FROM catalog.schema, quoting both parts."""
    cursor = FakeCursor([("penerima_manfaat",), ("wilayah",)])
    connection = FakeConnection(cursor)
    seen = {}

    def connect(**kwargs):
        seen.update(kwargs)
        return connection

    with fake_connect(connect):
        code, out, _err = run_engine(
            "tables", TRINO_HOST="trino.internal",
            TRINO_CATALOG="hive", TRINO_SCHEMA="analytics",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert events_of(out) == [{
        "event": "tables", "names": ["penerima_manfaat", "wilayah"],
    }], out
    assert cursor.statements == ['SHOW TABLES FROM "hive"."analytics"'], cursor.statements
    assert seen.get("catalog") == "hive" and seen.get("schema") == "analytics", seen
    assert cursor.closed and connection.closed, "the engine left a handle open"


def check_retries_reaches_the_connector():
    """RETRIES lands on TrinoConfig.max_attempts, and never as 0.

    The trino client raises UnboundLocalError on max_attempts=0, so RETRIES=0
    ("never retry") must still hand it one attempt.
    """
    for retries, expected in (("7", 7), ("0", 1), ("1", 1)):
        cursor = FakeCursor([("hive",)])
        seen = {}

        def connect(**kwargs):
            seen.update(kwargs)
            return FakeConnection(cursor)

        with fake_connect(connect):
            code, out, _err = run_engine(
                "catalogs", TRINO_HOST="trino.internal", RETRIES=retries,
            )
        assert code == 0, f"RETRIES={retries}: exit {code}, stdout={out!r}"
        assert seen.get("max_attempts") == expected, (retries, seen)

    cursor = FakeCursor([("hive",)])
    seen = {}

    def connect_default(**kwargs):
        seen.update(kwargs)
        return FakeConnection(cursor)

    with fake_connect(connect_default):  # unset keeps build_config's own default
        code, out, _err = run_engine("catalogs", TRINO_HOST="trino.internal")
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert seen.get("max_attempts") == 5, seen


def check_quote_in_identifier_is_escaped():
    """A `"` in a name is doubled inside the identifier, never left to break it."""
    cursor = FakeCursor([])
    with fake_connect(lambda **kwargs: FakeConnection(cursor)):
        code, out, _err = run_engine(
            "schemas", TRINO_HOST="trino.internal", TRINO_CATALOG='we"ird',
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == ['SHOW SCHEMAS FROM "we""ird"'], cursor.statements

    cursor = FakeCursor([])
    with fake_connect(lambda **kwargs: FakeConnection(cursor)):
        code, out, _err = run_engine(
            "tables", TRINO_HOST="trino.internal",
            TRINO_CATALOG='we"ird', TRINO_SCHEMA='sch"ema',
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == [
        'SHOW TABLES FROM "we""ird"."sch""ema"'
    ], cursor.statements


def check_blank_catalog_is_usage_error():
    """`schemas` with a blank or absent TRINO_CATALOG fails before the network."""
    for values in ({"TRINO_CATALOG": ""}, {}):
        calls = []
        with fake_connect(lambda **kwargs: calls.append(kwargs)):
            code, out, _err = run_engine("schemas", TRINO_HOST="trino.internal", **values)
        assert code == 1, f"{values}: exit {code}, stdout={out!r}"
        events = events_of(out)
        assert event_names(events) == ["error"], events
        assert "TRINO_CATALOG" in events[0]["message"], events[0]
        assert not calls, "the engine connected despite a blank catalog"


def check_blank_schema_is_usage_error():
    """`tables` with a blank TRINO_SCHEMA fails before the network."""
    for values in ({"TRINO_CATALOG": "hive", "TRINO_SCHEMA": ""}, {"TRINO_CATALOG": "hive"}):
        calls = []
        with fake_connect(lambda **kwargs: calls.append(kwargs)):
            code, out, _err = run_engine("tables", TRINO_HOST="trino.internal", **values)
        assert code == 1, f"{values}: exit {code}, stdout={out!r}"
        events = events_of(out)
        assert event_names(events) == ["error"], events
        assert "TRINO_SCHEMA" in events[0]["message"], events[0]
        assert not calls, "the engine connected despite a blank schema"


def check_browse_connection_failure():
    """A browse command whose connect fails is one error event and exit 1."""
    def refuse(**kwargs):
        raise OSError("connection refused")

    with fake_connect(refuse):
        code, out, err = run_engine(
            "tables", TRINO_HOST="trino.invalid", TRINO_CATALOG="hive",
            TRINO_SCHEMA="analytics", RETRIES="0",
        )
    assert code == 1, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    assert event_names(events) == ["error"], events
    assert "OSError" in events[0]["message"] and "connection refused" in events[0]["message"], events
    assert "Traceback" in err and "Traceback" not in out, (err, out)


def check_browse_names_are_string_arrays():
    """All three browse events carry `names` as a JSON array of strings."""
    cases = [
        ("catalogs", [("hive",), ("tpch",)], {}),
        ("schemas", [("analytics",), ("bronze",)], {"TRINO_CATALOG": "hive"}),
        ("tables", [("wilayah",)], {"TRINO_CATALOG": "hive", "TRINO_SCHEMA": "analytics"}),
    ]
    for command, rows, values in cases:
        cursor = FakeCursor(rows)
        with fake_connect(lambda **kwargs: FakeConnection(cursor)):
            code, out, _err = run_engine(command, TRINO_HOST="trino.internal", **values)
        assert code == 0, f"{command}: exit {code}, stdout={out!r}"
        lines = out.splitlines()
        assert lines, f"{command}: no stdout at all"
        for line in lines:  # every stdout line is exactly one compact JSON object
            assert line.strip() == line and line, f"{command}: padding or blank line in {line!r}"
            payload = json.loads(line)
            assert isinstance(payload, dict) and isinstance(payload.get("event"), str), payload
        event = only_event(events_of(out), command)
        assert isinstance(event.get("names"), list), event
        assert all(isinstance(name, str) for name in event["names"]), event
        assert event["names"] == [row[0] for row in rows], event


def check_empty_row_is_not_filtered():
    """A column-less row is a real coordinator answer: it must not raise."""
    cursor = FakeCursor([("hive",), (), ("tpch",)])
    with fake_connect(lambda **kwargs: FakeConnection(cursor)):
        code, out, _err = run_engine("catalogs", TRINO_HOST="trino.internal")
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert events_of(out) == [{"event": "catalogs", "names": ["hive", "", "tpch"]}], out


def check_unknown_command_is_usage_error():
    """A missing or unknown command is a usage error on stdout, exit 1."""
    for argv in ([ENGINE_PATH.name], [ENGINE_PATH.name, "import"]):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = engine.main(argv)
        assert code == 1, f"{argv}: exit {code}"
        events = events_of(out.getvalue())
        assert event_names(events) == ["error"], events
        assert "usage" in events[0]["message"], events[0]


# --------------------------------------------------------------------------- #
# to_table: Trino writes the rows, nothing comes back to this process
# --------------------------------------------------------------------------- #

TARGET = {"TARGET_CATALOG": "hive", "TARGET_SCHEMA": "analytics", "TARGET_TABLE": "foo"}


def stats_cursor(cursor, updates):
    """Wire the client's stats callback into a fake cursor's execute.

    The real trino client calls `stats_callback(deepcopy(stats))` on every
    coordinator update, from inside `cursor.execute`. The fake does the same, so
    the engine's progress and cancel paths run against a live callback rather
    than a mock of one.
    """

    def on_execute(current):
        callback = current.stats_callback
        for stats in updates:
            if current.cancelled or current.raise_on_execute is not None:
                break
            callback(dict(stats))

    cursor.on_execute = on_execute
    return cursor


def to_table_connection(cursor_factory, seen=None):
    """A connect factory that records its kwargs and wires each cursor's callback.

    The callback is not read from the connect kwargs: the real client takes it
    on `Connection.cursor()`, and `FakeConnection` mirrors that, so a callback
    passed the wrong way simply would not arrive.
    """
    seen = {} if seen is None else seen

    def connect(**kwargs):
        seen.update(kwargs)
        return FakeConnection(cursor_factory)

    return connect


def check_to_table_write_step_follows_a_live_connection():
    """A connect that fails emits no `write`: nothing is running to write with.

    `step connect` is announced before the network, `step write` only once the
    connection is open and the first statement is next. Emitting both up front
    would tell the app a write was under way when nothing ever ran.
    """

    def refuse(**kwargs):
        raise OSError("connection refused")

    with fake_connect(refuse):
        code, out, err = run_engine("to_table", TRINO_HOST="trino.invalid", SQL="SELECT 1", **TARGET)
    assert code == 1, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    assert event_names(events) == ["step", "error"], events
    assert events[0] == {"event": "step", "step": "connect"}, events
    assert "write" not in [event.get("step") for event in events], events
    assert "connection refused" in events[-1]["message"], events[-1]
    assert "Traceback" in err and "Traceback" not in out, (err, out)


def check_to_table_progress_ends_on_the_final_count():
    """The last progress before `done` carries the same rows `done` reports.

    The throttle may have swallowed the final ticks, so the runner emits once
    more when its last value differs -- and does not repeat itself when the
    coordinator already agreed.
    """
    cursor = stats_cursor(FakeCursor(rowcount=5000), [{"state": "RUNNING", "writtenRows": 5000}])
    with fake_connect(to_table_connection(lambda: cursor)):
        code, out, _err = run_engine(
            "to_table", TRINO_HOST="trino.internal", SQL="SELECT 1", PROGRESS_MS="600000", **TARGET
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    progress = [event["rows"] for event in events if event["event"] == "progress"]
    assert progress == [5000], progress  # no duplicate when the value already matches
    assert events[-1]["event"] == "done" and events[-1]["rows"] == 5000, events[-1]

    # rowcount -1 with progress 7: `done` takes the last observation, and the
    # progress stream is not repeated because that value is already the answer.
    cursor = stats_cursor(FakeCursor(rowcount=-1), [{"state": "RUNNING", "writtenRows": 7}])
    with fake_connect(to_table_connection(lambda: cursor)):
        code, out, _err = run_engine(
            "to_table", TRINO_HOST="trino.internal", SQL="SELECT 1", PROGRESS_MS="600000", **TARGET
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert [event["rows"] for event in events_of(out) if event["event"] == "progress"] == [7], out
    assert events_of(out)[-1]["rows"] == 7, out


def check_to_table_uses_the_real_client_contract():
    """The connection setup must match the real client's shape, keyword for keyword.

    This is the check that would have caught the bug the permissive stub hid:
    `Connection.__init__` takes no `stats_callback`, so handing one to
    `connect()` is a TypeError against the bundled trino 0.339.0, while
    `Connection.cursor(stats_callback=...)` is where it belongs. StrictConnection
    has the real surface -- it refuses any other keyword on either call -- so
    the engine's own choices are what is being tested, not the fake's tolerance.
    """
    connect_calls = []
    connections = []
    cursor = stats_cursor(FakeCursor(rowcount=9), [{"state": "RUNNING", "writtenRows": 9}])

    def connect(**kwargs):
        connect_calls.append(kwargs)
        connection = StrictConnection(lambda: cursor, **kwargs)  # raises on a bad keyword
        connections.append(connection)
        return connection

    with fake_connect(connect):
        code, out, _err = run_engine("to_table", TRINO_HOST="trino.internal", SQL="SELECT 1", **TARGET)
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert len(connect_calls) == 1, connect_calls
    assert "stats_callback" not in connect_calls[0], connect_calls[0]
    assert len(connections) == 1 and connections[0].closed, "the connection was left open"

    calls = connections[0].cursor_calls
    assert len(calls) == 1, calls
    assert calls[0]["stats_callback"] is not None, "cursor() was not given the stats callback"
    assert calls[0]["stats_callback"].__name__ == "on_stats_tick", calls[0]
    done = events_of(out)[-1]
    assert done["rows"] == 9 and done["cancelled"] is False, done


def check_to_table_stats_callback_is_per_cursor():
    """Every statement gets its own cursor, and each one carries the callback.

    A connection does not own the callback; the cursor does. A `replace` opens
    two cursors, so both must be wired or the CREATE's progress goes missing.
    """
    cursors = []

    def factory():
        cursor = stats_cursor(FakeCursor(rowcount=4), [{"state": "RUNNING", "writtenRows": 4}])
        cursors.append(cursor)
        return cursor

    connection = FakeConnection(factory)
    seen = {}

    def connect(**kwargs):
        seen.update(kwargs)
        return connection

    with fake_connect(connect):
        code, out, _err = run_engine(
            "to_table", TRINO_HOST="trino.internal", SQL="SELECT 1", WRITE_MODE="replace", **TARGET
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert len(connection.cursor_calls) == 2, connection.cursor_calls
    assert all(call["stats_callback"] is not None for call in connection.cursor_calls), \
        connection.cursor_calls
    assert all(cursor.statements for cursor in cursors), [c.statements for c in cursors]
    assert "stats_callback" not in seen, seen


def check_to_table_create():
    """`create` sends one CTAS and reports the rowcount the coordinator gave."""
    cursor = stats_cursor(FakeCursor(rowcount=42), [{"state": "RUNNING", "writtenRows": 42}])
    seen = {}
    with fake_connect(to_table_connection(lambda: cursor, seen)):
        code, out, _err = run_engine("to_table", TRINO_HOST="trino.internal", SQL="SELECT 1", **TARGET)
    assert code == 0, f"exit {code}, stdout={out!r}"
    events = events_of(out)

    steps = [event["step"] for event in events if event["event"] == "step"]
    assert steps == ["connect", "write"], steps
    assert events[0] == {"event": "step", "step": "connect"}, events[0]
    assert events[1] == {"event": "step", "step": "write"}, events[1]
    assert cursor.statements == ['CREATE TABLE "hive"."analytics"."foo" AS SELECT 1'], cursor.statements
    # The connection is opened with no stats_callback at all: the bundled client
    # takes it on cursor(), and connect() rejects the keyword.
    assert "stats_callback" not in seen, f"connect() was handed stats_callback: {seen}"
    assert seen.get("host") == "trino.internal" and seen.get("port") == 8080, seen

    done = events[-1]
    assert done == {
        "event": "done", "rows": 42, "table": "hive.analytics.foo", "mode": "create",
        "query_id": QUERY_ID, "cancelled": False, "warnings": [],
    }, done
    assert cursor.closed, "the engine left a cursor open"
    assert any(event["event"] == "progress" for event in events), events


def check_to_table_append():
    """`append` is one INSERT INTO ... SELECT against the quoted target."""
    cursor = stats_cursor(FakeCursor(rowcount=7), [{"state": "RUNNING", "processedRows": 7}])
    with fake_connect(to_table_connection(lambda: cursor)):
        code, out, _err = run_engine(
            "to_table", TRINO_HOST="trino.internal", SQL="SELECT 1", WRITE_MODE="append", **TARGET
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == ['INSERT INTO "hive"."analytics"."foo" SELECT 1'], cursor.statements
    done = events_of(out)[-1]
    assert done["mode"] == "append" and done["rows"] == 7, done


def check_to_table_replace_drops_first():
    """`replace` runs the DROP and then the CREATE, in that order."""
    cursors = []

    def factory():
        cursor = stats_cursor(FakeCursor(rowcount=5), [{"state": "RUNNING", "writtenRows": 5}])
        cursors.append(cursor)
        return cursor

    with fake_connect(to_table_connection(factory)):
        code, out, _err = run_engine(
            "to_table", TRINO_HOST="trino.internal", SQL="SELECT 1", WRITE_MODE="replace", **TARGET
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert [c.statements for c in cursors] == [
        ['DROP TABLE IF EXISTS "hive"."analytics"."foo"'],
        ['CREATE TABLE "hive"."analytics"."foo" AS SELECT 1'],
    ], [c.statements for c in cursors]
    assert all(c.closed for c in cursors), "a cursor was left open"
    done = events_of(out)[-1]
    assert done["mode"] == "replace" and done["rows"] == 5, done
    assert done["warnings"] and "dropped" in done["warnings"][0], done


def check_to_table_replace_create_failure_keeps_drop_warning():
    """A CREATE that fails after the DROP still tells the caller the old table went."""
    drop = stats_cursor(FakeCursor(), [{"state": "FINISHED"}])
    create = FakeCursor()
    create.raise_on_execute = trino.exceptions.TrinoUserError(
        "line 1:1: Table 'hive.analytics.foo' already exists"
    )

    with fake_connect(to_table_connection(lambda: drop if not drop.statements else create)):
        code, out, err = run_engine(
            "to_table", TRINO_HOST="trino.internal", SQL="SELECT 1", WRITE_MODE="replace", **TARGET
        )
    assert code == 1, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    error = only_event(events, "error")
    assert "already exists" in error["message"], error
    assert isinstance(error.get("warnings"), list) and error["warnings"], error
    assert any("dropped" in warning for warning in error["warnings"]), error
    assert "done" not in event_names(events), events
    assert "Traceback" in err and "Traceback" not in out, (err, out)


def check_to_table_quotes_identifiers():
    """A `"` anywhere in catalog, schema or table is doubled, part by part."""
    for values, expected in (
        (
            {"TARGET_CATALOG": 'hi"ve', "TARGET_SCHEMA": "analytics", "TARGET_TABLE": "foo"},
            'CREATE TABLE "hi""ve"."analytics"."foo" AS SELECT 1',
        ),
        (
            {"TARGET_CATALOG": "hive", "TARGET_SCHEMA": 'ana"lytics', "TARGET_TABLE": "foo"},
            'CREATE TABLE "hive"."ana""lytics"."foo" AS SELECT 1',
        ),
        (
            {"TARGET_CATALOG": "hive", "TARGET_SCHEMA": "analytics", "TARGET_TABLE": 'f"oo'},
            'CREATE TABLE "hive"."analytics"."f""oo" AS SELECT 1',
        ),
    ):
        cursor = stats_cursor(FakeCursor(rowcount=1), [{"writtenRows": 1}])
        with fake_connect(to_table_connection(lambda: cursor)):
            code, out, _err = run_engine("to_table", TRINO_HOST="trino.internal", SQL="SELECT 1", **values)
        assert code == 0, f"{values}: exit {code}, stdout={out!r}"
        assert cursor.statements == [expected], cursor.statements
        assert events_of(out)[-1]["table"] == f"{values['TARGET_CATALOG']}.{values['TARGET_SCHEMA']}.{values['TARGET_TABLE']}"


def check_to_table_blank_target_is_usage_error():
    """A blank TARGET_TABLE, and separately a blank TARGET_CATALOG, never connect."""
    cases = [
        {"TARGET_CATALOG": "hive", "TARGET_SCHEMA": "analytics", "TARGET_TABLE": ""},
        {"TARGET_CATALOG": "hive", "TARGET_SCHEMA": "analytics"},  # absent
        {"TARGET_CATALOG": "", "TARGET_SCHEMA": "analytics", "TARGET_TABLE": "foo"},
        {"TARGET_CATALOG": "hive", "TARGET_SCHEMA": "", "TARGET_TABLE": "foo"},
    ]
    for values in cases:
        calls = []

        def connect(**kwargs):
            calls.append(kwargs)
            return FakeConnection(FakeCursor())

        with fake_connect(connect):
            code, out, _err = run_engine("to_table", TRINO_HOST="trino.internal", SQL="SELECT 1", **values)
        assert code == 1, f"{values}: exit {code}, stdout={out!r}"
        events = events_of(out)
        assert event_names(events) == ["error"], events
        assert "required" in events[0]["message"] and "TARGET_" in events[0]["message"], events[0]
        assert not calls, f"{values}: the engine connected despite a blank target"


def check_to_table_invalid_write_mode_is_usage_error():
    """A bad WRITE_MODE is a usage error, decided before the network.

    A blank value counts: `_value` would fold it into "unset" and quietly use
    the default, which would run a CTAS the caller never asked for. An absent
    key is the only thing that means create.
    """
    for mode in ("upsert", "", "merge", "insert"):
        calls = []

        def connect(**kwargs):
            calls.append(kwargs)
            return FakeConnection(FakeCursor())

        with fake_connect(connect):
            code, out, _err = run_engine(
                "to_table", TRINO_HOST="trino.internal", SQL="SELECT 1", WRITE_MODE=mode, **TARGET
            )
        assert code == 1, f"{mode!r}: exit {code}, stdout={out!r}"
        events = events_of(out)
        assert event_names(events) == ["error"], events
        assert "WRITE_MODE" in events[0]["message"], events[0]
        assert not calls, f"{mode!r}: the engine connected despite a bad mode"

    cursor = stats_cursor(FakeCursor(rowcount=1), [{"writtenRows": 1}])
    with fake_connect(to_table_connection(lambda: cursor)):  # unset defaults to create
        code, out, _err = run_engine("to_table", TRINO_HOST="trino.internal", SQL="SELECT 1", **TARGET)
    assert code == 0, f"unset: exit {code}, stdout={out!r}"
    assert cursor.statements == ['CREATE TABLE "hive"."analytics"."foo" AS SELECT 1'], cursor.statements
    assert events_of(out)[-1]["mode"] == "create", out


def check_to_table_unknown_rowcount_is_not_a_number():
    """rowcount -1 with no rows reported stays -1; with progress, it is the progress."""
    cursor = stats_cursor(FakeCursor(rowcount=-1), [{"state": "RUNNING"}])
    with fake_connect(to_table_connection(lambda: cursor)):
        code, out, _err = run_engine("to_table", TRINO_HOST="trino.internal", SQL="SELECT 1", **TARGET)
    assert code == 0, f"exit {code}, stdout={out!r}"
    done = events_of(out)[-1]
    assert done["rows"] == -1, done  # not 0, and not a guess

    cursor = stats_cursor(FakeCursor(rowcount=-1), [{"state": "RUNNING", "writtenRows": 3}])
    with fake_connect(to_table_connection(lambda: cursor)):
        code, out, _err = run_engine("to_table", TRINO_HOST="trino.internal", SQL="SELECT 1", **TARGET)
    assert code == 0, f"exit {code}, stdout={out!r}"
    done = events_of(out)[-1]
    assert done["rows"] == 3, done  # the last real observation, not a fabrication


def check_to_table_progress_follows_the_stats():
    """Progress is driven by stats: writtenRows first, and its `state` rides along."""
    cursor = stats_cursor(
        FakeCursor(rowcount=-1),
        [
            {"state": "QUEUED", "queued": True},                                  # no counts: no event
            {"state": "RUNNING", "processedRows": 1200, "writtenRows": 400},      # writtenRows wins
            {"state": "RUNNING", "processedRows": 9000},                          # writtenRows gone
        ],
    )
    with fake_connect(to_table_connection(lambda: cursor)):
        code, out, _err = run_engine(
            "to_table", TRINO_HOST="trino.internal", SQL="SELECT 1", PROGRESS_MS="0", **TARGET
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    progress = [event for event in events if event["event"] == "progress"]
    assert [event["rows"] for event in progress] == [400, 9000], progress
    assert [event["state"] for event in progress] == ["RUNNING", "RUNNING"], progress
    assert events[-1]["rows"] == 9000, events[-1]


def check_to_table_cancel_is_not_a_failure():
    """A cancel is not a failure: exit 0, `cancelled: true`, and what it cost.

    The fake raises the client's own cancellation error the moment a cancel
    lands, which is what trino 0.339.0 does when the query goes away underneath
    `execute`. `done` still has to be well formed, and its `rows` must be the
    last value the coordinator actually reported rather than a made-up total.
    """
    ticks = [
        {"state": "RUNNING", "writtenRows": 10},
        {"state": "RUNNING", "writtenRows": 20},
    ]
    cancel_fired = {"count": 0}

    def is_cancelled():
        cancel_fired["count"] += 1
        return cancel_fired["count"] >= 2  # the second tick asks it to stop

    cursors = []

    def factory():
        cursor = FakeCursor(rowcount=-1)
        cursors.append(cursor)

        def on_execute(current):
            for stats in ticks:
                current.stats_callback(stats)
                if current.cancelled:
                    # what the real client raises when a query is cancelled under it
                    raise trino.exceptions.TrinoUserError("Query has been cancelled", QUERY_ID)

        cursor.on_execute = on_execute
        return cursor

    original = engine.CANCEL_REQUESTED.is_set
    engine.CANCEL_REQUESTED.is_set = is_cancelled
    try:
        with fake_connect(to_table_connection(factory)):
            code, out, _err = run_engine("to_table", TRINO_HOST="trino.internal", SQL="SELECT 1", **TARGET)
    finally:
        engine.CANCEL_REQUESTED.is_set = original
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert sum(cursor.cancels for cursor in cursors) == 1, \
        f"cancel() ran {sum(c.cancels for c in cursors)} times over {len(cursors)} cursor(s)"
    events = events_of(out)
    assert "error" not in event_names(events), events
    done = events[-1]
    assert done["event"] == "done" and done["cancelled"] is True, done
    assert done["warnings"] and "stopped" in done["warnings"][0], done
    assert done["rows"] == 10, done  # the last value reported before the cancel
    assert [event["rows"] for event in events if event["event"] == "progress"] == [10], events


def check_to_table_cancel_is_sent_once():
    """Every later stats tick must not send another cancel.

    `cursor.cancel()` is a DELETE against the query; on the real client the
    second one fails, turns into an error, and a perfectly ordinary cancel
    becomes an `error` event. The fake records every call, and `is_cancelled`
    keeps answering True after the first, so a missing guard shows up as
    cancels > 1 rather than as a flaky success.
    """
    ticks = [{"state": "RUNNING", "writtenRows": n} for n in (10, 20, 30, 40)]
    cursor = FakeCursor(rowcount=-1)
    cursor.on_execute = lambda current: [current.stats_callback(stats) for stats in ticks]
    cancel_calls = []

    original = engine.CANCEL_REQUESTED.is_set
    engine.CANCEL_REQUESTED.is_set = lambda: True
    try:
        with fake_connect(to_table_connection(lambda: cursor)):
            code, out, _err = run_engine("to_table", TRINO_HOST="trino.internal", SQL="SELECT 1", **TARGET)
    finally:
        engine.CANCEL_REQUESTED.is_set = original
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.cancels == 1, f"cancel() ran {cursor.cancels} times for {len(ticks)} stats ticks"
    done = events_of(out)[-1]
    assert done["cancelled"] is True, done
    assert "error" not in event_names(events_of(out)), out


def check_to_table_stdout_is_json_only():
    """Every stdout line of a to_table run is one compact JSON object."""
    cursor = stats_cursor(
        FakeCursor(rowcount=12),
        [
            {"state": "RUNNING", "writtenRows": 1000},
            {"state": "RUNNING", "writtenRows": 2000},
            {"state": "FINISHED", "writtenRows": 2500},
        ],
    )
    with fake_connect(to_table_connection(lambda: cursor)):
        code, out, _err = run_engine(
            "to_table", TRINO_HOST="trino.internal", SQL="SELECT * FROM t", PROGRESS_MS="0", **TARGET
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    lines = out.splitlines()
    assert lines, "no stdout at all"
    for line in lines:
        assert line.strip() == line and line, f"padding or a blank line in {line!r}"
        payload = json.loads(line)  # raises if any line is not exactly one JSON object
        assert isinstance(payload, dict) and isinstance(payload.get("event"), str), payload
    assert lines[-1].startswith('{"event": "done"'), lines[-1]
    assert "progress" in event_names(events_of(out)), out


def check_to_table_usage_line_lists_the_command():
    """The usage line main() prints names every command, to_table included."""
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        code = engine.main([ENGINE_PATH.name])
    assert code == 1, f"exit {code}"
    message = events_of(out.getvalue())[0]["message"]
    assert message.endswith("test|catalogs|schemas|tables|export|to_table"), message


def check_describe_error_survives_a_client_error_that_cannot_stringify():
    """An error path may not throw while describing the error.

    trino-python-client 0.339.0 raises, at client.py:985,

        TrinoUserError("Query has been cancelled", self.query_id)

    a bare string where `TrinoQueryError.__init__` expects an error dict, so its
    `__str__` -> `__repr__` -> `self.error_type` -> `self._error.get(...)` raises
    `AttributeError: 'str' object has no attribute 'get'`.

    With the real package installed, `check_to_table_cancel_is_not_a_failure`
    builds exactly that object and covers this. This check exists because the
    local trino stub defines a well-behaved `TrinoUserError`, so on a checkout
    with no trino installed the real shape is otherwise never exercised -- and
    that is how a cancellation once came out as "AttributeError: 'str' object
    has no attribute 'get'" from a run that had actually been cancelled.
    """
    from exporter.source import describe_error
    from exporter.to_table import _is_cancellation

    class Unstringable(trino.exceptions.TrinoUserError):
        def __str__(self):
            raise AttributeError("'str' object has no attribute 'get'")

        __repr__ = __str__

    exc = Unstringable("Query has been cancelled", QUERY_ID)
    assert describe_error(exc) == "Query has been cancelled", describe_error(exc)
    assert _is_cancellation(exc) is True

    # And a genuinely different failure must not be mistaken for a cancel.
    other = Unstringable("line 1:1: Table 'hive.analytics.foo' already exists", QUERY_ID)
    assert _is_cancellation(other) is False


def check_error_event_survives_an_unstringable_failure():
    """main() must always emit its one `error` event.

    stdout carries the protocol. If the handler throws while formatting the
    message, nothing is written at all and the app parses an empty stream -- a
    worse outcome than the original failure, and one no caller can diagnose.
    """
    class Unstringable(Exception):
        def __str__(self):
            raise AttributeError("nope")

        __repr__ = __str__

    original = engine.build_config

    def boom(_env):
        raise Unstringable("the real message")

    engine.build_config = boom
    try:
        code, out, _err = run_engine("catalogs", TRINO_HOST="trino.internal")
    finally:
        engine.build_config = original

    assert code == 1, code
    events = events_of(out)
    assert event_names(events) == ["error"], events
    assert "the real message" in events[0]["message"], events


# --------------------------------------------------------------------------- #

def main():
    checks = [value for name, value in sorted(globals().items()) if name.startswith("check_")]
    failed = 0
    for check in checks:
        try:
            check()
        except Exception:
            failed += 1
            traceback.print_exc()
            print(f"FAIL {check.__name__}")
        else:
            print(f"ok   {check.__name__}")
    print(f"\n{len(checks) - failed}/{len(checks)} checks passed")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
