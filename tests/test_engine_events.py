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


def _install_driver_stub(name):
    """The import surface of psycopg / pymysql, for a checkout without them.

    exporter/drivers.py imports each client package inside connect(), so the
    module only has to exist the moment a check reaches that driver -- and every
    check replaces its connect() with a fake before that. The app bundles the
    real packages; a bare checkout may not have them.
    """
    print(f"note: {name} is not installed; using a local stub for the import surface", file=sys.stderr)
    module = types.ModuleType(name)

    def connect(**kwargs):  # pragma: no cover - always replaced by the fake
        raise AssertionError("the fake connect was not installed")

    module.connect = connect
    sys.modules[name] = module
    return module


try:
    import psycopg
except ImportError:
    psycopg = _install_driver_stub("psycopg")

try:
    import pymysql
except ImportError:
    pymysql = _install_driver_stub("pymysql")

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

    def __init__(self, rows=(), columns=None, query_id=QUERY_ID, rowcount=-1, page_limit=None):
        self._rows = [list(row) for row in rows]
        self._pos = 0
        # A real coordinator hands back a page, not the whole result set; a
        # check that watches over-fetching sets this so the fake does too.
        self._page_limit = page_limit
        self._columns = columns
        self._query_id = query_id
        self.description = None  # trino only knows the columns once a page arrived
        self.query_id = None
        self.rowcount = rowcount  # trino: update_count, or -1 when it did not say
        self.arraysize = 0
        self.statements = []
        self.fetches = 0
        self.pages = []  # the rows each fetchmany actually handed back
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
        if self._page_limit is not None:
            size = min(size, self._page_limit)
        page = self._rows[self._pos:self._pos + size]
        self._pos += len(page)
        self.pages.append(page)
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


@contextlib.contextmanager
def fake_dbapi(module, factory):
    """Replace one client package's connect() for the duration of one check.

    drivers.py reaches for `psycopg.connect` / `pymysql.connect` at call time, so
    patching the module attribute is what routes a check down that driver's path
    without a server.
    """
    original = module.connect
    module.connect = factory
    try:
        yield
    finally:
        module.connect = original


ENGINE_KEYS = (
    "DB_KIND", "DB_URL", "DB_HOST", "DB_PORT", "DB_USER", "DB_PASSWORD", "DB_DATABASE",
    "DB_SCHEMA", "DB_SCHEME", "DB_SSLMODE", "DB_INSECURE",
    "TRINO_URL", "TRINO_HOST", "TRINO_PORT", "TRINO_USER", "TRINO_PASSWORD", "TRINO_CATALOG",
    "TRINO_SCHEMA", "TRINO_INSECURE", "SQL", "SQL_PATH", "FORMAT", "OUT_DIR", "NAME", "ZIP",
    "BATCH_SIZE", "ROWS_PER_FILE", "RETRIES", "DELIMITER", "ENCODING", "HEADER", "BOM",
    "NULL_TEXT", "JSONL", "SQL_TABLE", "SHEET", "DBF_CHAR_WIDTH", "DBF_ENCODING", "PROGRESS_MS",
    "TARGET_CATALOG", "TARGET_SCHEMA", "TARGET_TABLE", "WRITE_MODE", "LIMIT",
    "DB_ALL_SCHEMAS",
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
    assert message.endswith(
        "test|catalogs|schemas|tables|export|to_table|preview|count|explain"
    ), message


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
# preview: run the caller's SQL and show the first rows, without rewriting it
# --------------------------------------------------------------------------- #

# What the fake coordinator says about the columns. A Trino description is
# (name, type_code, display_size, internal_size, precision, scale, null_ok), so
# the type arrives as the driver's own code/name and the engine stringifies it:
# the grid's type chip is always a string, never null.
PREVIEW_COLUMNS = [
    ("kode_wilayah", "varchar", None, None, None, None, None),
    ("nama", "varchar", None, None, None, None, None),
    ("jumlah", "bigint", None, None, None, None, None),
]
PREVIEW_ROWS = [
    ["32.01", "Jawa Barat", 1234],
    ["32.02", "Jawa Tengah", 5678],
    ["32.03", None, 90],
]


def preview_rows(events):
    """Every `rows` batch's arrays, concatenated in the order they arrived."""
    sent = []
    for event in events:
        if event["event"] == "rows":
            sent.extend(event["data"])
    return sent


def preview_fake(rows, columns=PREVIEW_COLUMNS, **kwargs):
    """(cursor, connect factory) for one preview run, with nothing shared."""
    cursor = FakeCursor(rows, columns, **kwargs)
    return cursor, (lambda **values: FakeConnection(cursor))


def check_preview_reports_columns_batches_and_done():
    """A three-row preview: connect, one columns, one rows batch, one done."""
    cursor, connect = preview_fake(PREVIEW_ROWS)
    with fake_connect(connect):
        code, out, err = run_engine(
            "preview", TRINO_HOST="trino.internal", TRINO_USER="analyst",
            SQL="SELECT kode_wilayah, nama, jumlah FROM wilayah", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}, stderr={err!r}"
    events = events_of(out)

    assert events[0] == {"event": "step", "step": "connect"}, events[0]
    assert event_names(events)[0] == "step", events  # before anything is fetched
    assert events[1] == {
        "event": "columns",
        "columns": [
            {"name": "kode_wilayah", "type": "varchar"},
            {"name": "nama", "type": "varchar"},
            {"name": "jumlah", "type": "bigint"},
        ],
    }, events[1]
    assert all(isinstance(column["type"], str) for column in events[1]["columns"]), events[1]

    batches = [event for event in events if event["event"] == "rows"]
    assert len(batches) == 1, events
    assert batches[0]["data"] == [
        ["32.01", "Jawa Barat", "1234"],
        ["32.02", "Jawa Tengah", "5678"],
        ["32.03", None, "90"],
    ], batches[0]

    done = events[-1]
    assert done["event"] == "done", events
    assert done["rows"] == 3 and done["truncated"] is False, done
    assert done["query_id"] == QUERY_ID, done
    assert isinstance(done["elapsed_ms"], int) and done["elapsed_ms"] >= 0, done


def check_preview_limit_caps_the_rows_without_over_fetching():
    """LIMIT=2 over five fake rows sends two, says truncated, and stops there.

    Deciding `truncated` costs exactly one row past the cap, never a page: the
    fake hands over two-row pages, like a coordinator, and records every row it
    gave away. QueryStream preloads one row to learn the columns (the fake only
    fills `description` in with the first page), so page 1 is that row and page 2
    carries the two the caller asked for plus the one the verdict needs. A third
    page would be the over-fetch this pins down.
    """
    rows = [[f"row {i}"] for i in range(5)]
    cursor, connect = preview_fake(
        rows, columns=[("id", "varchar", None, None, None, None, None)], page_limit=2,
    )
    with fake_connect(connect):
        code, out, _err = run_engine(
            "preview", TRINO_HOST="trino.internal", SQL="SELECT id FROM t", LIMIT="2", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    assert preview_rows(events) == [["row 0"], ["row 1"]], events
    done = events[-1]
    assert done["rows"] == 2 and done["truncated"] is True, done
    assert cursor.fetches == 2, f"a five-row result was paged {cursor.fetches} times for LIMIT=2"
    assert [len(page) for page in cursor.pages] == [1, 2], cursor.pages


def check_preview_truncated_when_the_page_ends_on_the_cap():
    """A page exactly LIMIT long with more behind it is truncated, not "N rows".

    The case that motivated pulling a row past the cap: the fake page ends
    precisely on the cap, so nothing already buffered reveals that the result
    continues. The one-row probe is the only thing that can tell this apart from
    a query that genuinely ended there, and the footer's "limit reached" depends
    on it.
    """
    rows = [[f"row {i}"] for i in range(3)]
    cursor, connect = preview_fake(
        rows, columns=[("id", "varchar", None, None, None, None, None)], page_limit=2,
    )
    with fake_connect(connect):
        code, out, _err = run_engine(
            "preview", TRINO_HOST="trino.internal", SQL="SELECT id FROM t", LIMIT="2", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    assert preview_rows(events) == [["row 0"], ["row 1"]], events
    done = events[-1]
    assert done["rows"] == 2, done  # the number sent, not the number fetched
    assert done["truncated"] is True, done


def check_preview_not_truncated_when_the_page_is_the_whole_result():
    """A page exactly LIMIT long that exhausted the result is not truncated.

    The other half of the pair: same shape, same cap, one row fewer behind it.
    Both must be reported correctly, or "limit reached" means nothing.
    """
    rows = [[f"row {i}"] for i in range(2)]
    cursor, connect = preview_fake(
        rows, columns=[("id", "varchar", None, None, None, None, None)], page_limit=2,
    )
    with fake_connect(connect):
        code, out, _err = run_engine(
            "preview", TRINO_HOST="trino.internal", SQL="SELECT id FROM t", LIMIT="2", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    assert preview_rows(events) == [["row 0"], ["row 1"]], events
    done = events[-1]
    assert done["rows"] == 2, done
    assert done["truncated"] is False, done


def check_preview_limit_defaults_and_floors_at_one():
    """A blank or zero LIMIT falls back to the default; the floor is one row.

    `_value` folds a blank into "unset", so a blank LIMIT is the documented
    default rather than zero rows. LIMIT=0 is the same floor: a preview that
    returned nothing would say nothing about the statement.
    """
    rows = [[str(i)] for i in range(5)]
    columns = [("id", "varchar", None, None, None, None, None)]

    for values, expected, truncated in (
        ({}, 5, False),                       # unset: the documented default
        ({"LIMIT": ""}, 5, False),            # blank: the default, never zero rows
        ({"LIMIT": "0"}, 1, True),            # zero floors at one row
        ({"LIMIT": "-3"}, 1, True),           # and so does anything below it
        ({"LIMIT": "3"}, 3, True),            # a real cap stops short of five
    ):
        cursor, connect = preview_fake(rows, columns=columns)
        with fake_connect(connect):
            code, out, _err = run_engine(
                "preview", TRINO_HOST="trino.internal", SQL="SELECT id FROM t",
                RETRIES="0", **values,
            )
        assert code == 0, f"{values}: exit {code}, stdout={out!r}"
        events = events_of(out)
        done = events[-1]
        assert len(preview_rows(events)) == expected, (values, events)
        assert done["rows"] == expected, (values, done)
        assert done["truncated"] is truncated, (values, done)


def check_preview_batches_more_than_one_page():
    """More than PREVIEW_BATCH rows arrive as several batches, in order."""
    batch = engine.PREVIEW_BATCH
    rows = [[f"row {i}"] for i in range(batch * 2 + 7)]
    columns = [("id", "varchar", None, None, None, None, None)]
    cursor, connect = preview_fake(rows, columns=columns)
    with fake_connect(connect):
        code, out, _err = run_engine(
            "preview", TRINO_HOST="trino.internal", SQL="SELECT id FROM t",
            LIMIT=str(len(rows)), RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    batches = [event for event in events if event["event"] == "rows"]
    assert len(batches) > 1, "a result larger than PREVIEW_BATCH arrived as one event"
    assert all(len(event["data"]) <= batch for event in batches), [len(e["data"]) for e in batches]
    assert preview_rows(events) == [[f"row {i}"] for i in range(len(rows))], "order or content changed"
    done = events[-1]
    assert done["rows"] == len(rows) and done["truncated"] is False, done


def check_preview_null_stays_json_null():
    """A NULL is JSON null: not "None", not an empty string."""
    cursor, connect = preview_fake([["a", None, ""]])
    with fake_connect(connect):
        code, out, _err = run_engine(
            "preview", TRINO_HOST="trino.internal", SQL="SELECT * FROM t", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    lines = out.splitlines()
    rows_lines = [line for line in lines if json.loads(line)["event"] == "rows"]
    assert len(rows_lines) == 1, lines
    assert "null" in rows_lines[0], rows_lines[0]  # the wire form, not just the parsed value
    assert "None" not in rows_lines[0], rows_lines[0]
    assert preview_rows(events_of(out)) == [["a", None, ""]], events_of(out)


def check_preview_exotic_cells_become_strings():
    """A Decimal and a datetime arrive as strings; every cell is str or null."""
    from datetime import datetime
    from decimal import Decimal

    # The fake description only has to be close enough: the type chip is the
    # column's, and the check is about the cells' JSON types.
    columns = [
        ("amount", "decimal(10,2)", None, None, None, None, None),
        ("at", "timestamp(3)", None, None, None, None, None),
        ("flag", "boolean", None, None, None, None, None),
        ("raw", "varbinary", None, None, None, None, None),
    ]
    cursor, connect = preview_fake(
        [[Decimal("12.50"), datetime(2026, 1, 31, 12, 0, 0), True, b"\x01\xff", None]],
        columns=columns,
    )
    with fake_connect(connect):
        code, out, _err = run_engine(
            "preview", TRINO_HOST="trino.internal", SQL="SELECT * FROM t", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    cells = preview_rows(events)[0]
    assert all(cell is None or isinstance(cell, str) for cell in cells), cells
    assert cells[0] == "12.50" and cells[1] == "2026-01-31 12:00:00", cells
    assert cells[2] == "true" and cells[3] == "01ff", cells
    # A native number in the column would need default=str on the way out and
    # the grid would then hold two JSON types for one column.
    paid = [line for line in out.splitlines() if json.loads(line)["event"] == "rows"][0]
    assert '"12.50"' in paid, paid  # a JSON string, never the bare number


def check_preview_does_not_rewrite_the_sql():
    """The statement executed is byte-identical to SQL; nothing is appended.

    LIMIT, wrapping and a trailing semicolon all change what the caller's
    statement means, so the engine adds none of them: the cap is enforced by
    fetching, not by rewriting.
    """
    statements = [
        "SELECT * FROM wilayah ORDER BY kode_wilayah LIMIT 5",
        "SELECT 1",
        "SELECT * FROM t;",
        "SELECT count(*) FROM t",
    ]
    columns = [("id", "varchar", None, None, None, None, None)]
    for sql in statements:
        cursor, connect = preview_fake([["x"]], columns=columns)
        with fake_connect(connect):
            code, out, _err = run_engine(
                "preview", TRINO_HOST="trino.internal", SQL=sql, LIMIT="2", RETRIES="0",
            )
        assert code == 0, f"{sql!r}: exit {code}, stdout={out!r}"
        assert len(cursor.statements) == 1, (sql, cursor.statements)
        executed = cursor.statements[0]
        assert "LIMIT" not in executed or "LIMIT" in sql, (sql, executed)
        assert executed.strip() == sql.strip().rstrip(";"), (sql, executed)
        assert executed.count("SELECT") == sql.count("SELECT"), (sql, executed)


def check_preview_blank_sql_is_usage_error():
    """A blank SQL is a usage error: one error event, exit 1, no network."""
    calls = []
    with fake_connect(lambda **kwargs: calls.append(kwargs)):
        code, out, _err = run_engine(
            "preview", TRINO_HOST="trino.internal", SQL="", SQL_PATH="", LIMIT="5",
        )
    assert code == 1, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    assert event_names(events) == ["error"], events
    assert "SQL" in events[0]["message"], events[0]
    assert not calls, "the engine connected despite a blank SQL"


def check_preview_connection_failure():
    """A connect that will not open: step connect, then one error, exit 1."""

    def refuse(**kwargs):
        raise OSError("connection refused")

    with fake_connect(refuse):
        code, out, err = run_engine(
            "preview", TRINO_HOST="trino.invalid", SQL="SELECT 1", LIMIT="10", RETRIES="0",
        )
    assert code == 1, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    assert event_names(events) == ["step", "error"], events
    assert events[0] == {"event": "step", "step": "connect"}, events
    assert "OSError" in events[-1]["message"] and "connection refused" in events[-1]["message"], events
    assert "done" not in event_names(events), events
    assert "Traceback" in err and "Traceback" not in out, (err, out)


def check_preview_stdout_is_json_only():
    """Every stdout line of a multi-batch preview is one compact JSON object."""
    batch = engine.PREVIEW_BATCH
    cursor, connect = preview_fake(
        [[f"row {i}", i] for i in range(batch + 3)],
        columns=[("id", "varchar", None, None, None, None, None),
                 ("n", "bigint", None, None, None, None, None)],
    )
    with fake_connect(connect):
        code, out, _err = run_engine(
            "preview", TRINO_HOST="trino.internal", SQL="SELECT * FROM t", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    lines = out.splitlines()
    assert lines, "no stdout at all"
    for line in lines:
        assert line.strip() == line and line, f"padding or a blank line in {line!r}"
        payload = json.loads(line)  # raises if any line is not exactly one JSON object
        assert isinstance(payload, dict) and isinstance(payload.get("event"), str), payload
    assert lines[-1].startswith('{"event": "done"'), lines[-1]


def check_preview_batch_key_is_not_the_integer_rows():
    """The batch array lives under `data`; `rows` is always the integer count.

    One key cannot be two types: the Swift `Event` struct decodes `rows` as
    `Int?` on `progress` and `done`, so a batch that reused the name would make
    a decoder fail or mis-read. This is the catalogs-count trap again, and the
    event name stays `rows` while the payload moves to `data`.
    """
    cursor, connect = preview_fake(PREVIEW_ROWS)
    with fake_connect(connect):
        code, out, _err = run_engine(
            "preview", TRINO_HOST="trino.internal", SQL="SELECT * FROM t", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    batches = [event for event in events if event["event"] == "rows"]
    assert batches, events
    for event in batches:
        assert "data" in event, event
        assert "rows" not in event, f"the batch event reused the integer `rows` key: {event}"
        assert isinstance(event["data"], list), event
        assert all(isinstance(row, list) for row in event["data"]), event

    done = only_event(events, "done")
    assert isinstance(done["rows"], int) and not isinstance(done["rows"], bool), done
    assert isinstance(done["truncated"], bool), done


# --------------------------------------------------------------------------- #
# count: the true total of the statement on screen, fetched only when asked
# --------------------------------------------------------------------------- #

COUNT_COLUMNS = [("_col0", "bigint", None, None, None, None, None)]


def count_fake(rows, columns=COUNT_COLUMNS, **kwargs):
    """(cursor, connect factory) for one count run, with nothing shared."""
    cursor = FakeCursor(rows, columns, **kwargs)
    return cursor, (lambda **values: FakeConnection(cursor))


def check_count_reports_one_integer_and_a_done():
    """A SELECT gives step connect, one `count` carrying the total, and a done."""
    cursor, connect = count_fake([[6000]])
    with fake_connect(connect):
        code, out, err = run_engine(
            "count", TRINO_HOST="trino.internal", TRINO_USER="analyst",
            SQL="SELECT * FROM wilayah", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}, stderr={err!r}"
    events = events_of(out)

    assert event_names(events) == ["step", "count", "done"], events
    assert events[0] == {"event": "step", "step": "connect"}, events[0]
    assert "count" in event_names(events[:2]), "the count did not arrive right after the step"

    counted = only_event(events, "count")  # exactly one event with that name
    assert counted["rows"] == 6000, counted

    done = events[-1]
    assert done["event"] == "done", events
    assert isinstance(done["elapsed_ms"], int) and done["elapsed_ms"] >= 0, done
    # One event named `count`, and no `done.rows` for a decoder to mistake it for.
    assert "rows" not in done, done


def check_count_wraps_the_statement_exactly():
    """The statement handed to the cursor is the wrap, byte for byte."""
    cursor, connect = count_fake([[3]])
    with fake_connect(connect):
        code, out, _err = run_engine(
            "count", TRINO_HOST="trino.internal", SQL="SELECT * FROM t", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == [
        "SELECT COUNT(*) FROM (SELECT * FROM t) AS queryhive_count"
    ], cursor.statements


def check_count_strips_one_trailing_semicolon():
    """A trailing semicolon and the whitespace after it are stripped, and only that."""
    statements = [
        ("SELECT 1;", "SELECT COUNT(*) FROM (SELECT 1) AS queryhive_count"),
        ("SELECT 1;  ", "SELECT COUNT(*) FROM (SELECT 1) AS queryhive_count"),
        ("  SELECT 1 ;  \n", "SELECT COUNT(*) FROM (SELECT 1) AS queryhive_count"),
        (  # a semicolon inside the statement is not the one that is stripped
            "SELECT 'a;b' AS s FROM t",
            "SELECT COUNT(*) FROM (SELECT 'a;b' AS s FROM t) AS queryhive_count",
        ),
        (  # nor is a semicolon the caller put in the middle of a comment
            "SELECT 1 -- one; two",
            "SELECT COUNT(*) FROM (SELECT 1 -- one; two) AS queryhive_count",
        ),
        (  # WITH is a SELECT statement too, and stays verbatim inside the parens
            "WITH x AS (SELECT 1) SELECT * FROM x;",
            "SELECT COUNT(*) FROM (WITH x AS (SELECT 1) SELECT * FROM x) AS queryhive_count",
        ),
    ]
    for sql, expected in statements:
        cursor, connect = count_fake([[1]])
        with fake_connect(connect):
            code, out, _err = run_engine(
                "count", TRINO_HOST="trino.internal", SQL=sql, RETRIES="0",
            )
        assert code == 0, f"{sql!r}: exit {code}, stdout={out!r}"
        assert cursor.statements == [expected], (sql, cursor.statements)


def check_count_keeps_the_callers_limit_inside_the_parens():
    """A LIMIT stays inside: the count is of the limited set, which the grid shows."""
    cursor, connect = count_fake([[1000]])
    with fake_connect(connect):
        code, out, _err = run_engine(
            "count", TRINO_HOST="trino.internal", LIMIT="5",
            SQL="SELECT * FROM t LIMIT 1000", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == [
        "SELECT COUNT(*) FROM (SELECT * FROM t LIMIT 1000) AS queryhive_count"
    ], cursor.statements
    assert only_event(events_of(out), "count")["rows"] == 1000, out


def check_count_refuses_a_statement_that_is_not_a_select():
    """An INSERT, an EXPLAIN or a DDL statement is a usage error before the network.

    Wrapping one of those would ask the coordinator to run something that is not
    a query and then report a number for it, so the verdict is made on the
    statement's first keyword before a connection is opened.
    """
    for sql in (
        "INSERT INTO t VALUES (1)",
        "  insert into t values (1)",
        "EXPLAIN SELECT 1",
        "UPDATE t SET a = 1",
        "DELETE FROM t",
        "DROP TABLE t",
        "CREATE TABLE t (a int)",
        "/* a note */ INSERT INTO t VALUES (1)",  # a comment is not a keyword
        "-- a note\nINSERT INTO t VALUES (1)",
    ):
        calls = []
        with fake_connect(lambda **kwargs: calls.append(kwargs)):
            code, out, _err = run_engine(
                "count", TRINO_HOST="trino.invalid", SQL=sql, RETRIES="0",
            )
        assert code == 1, f"{sql!r}: exit {code}, stdout={out!r}"
        events = events_of(out)
        assert event_names(events) == ["error"], (sql, events)
        assert "SELECT" in events[0]["message"], (sql, events[0])
        assert not calls, f"{sql!r}: the engine connected for a statement it cannot count"


def check_count_refuses_two_statements():
    """`SELECT 1; SELECT 2` is a usage error: only one statement may be counted."""
    for sql in ("SELECT 1; SELECT 2", "SELECT 1; SELECT 2;"):
        calls = []
        with fake_connect(lambda **kwargs: calls.append(kwargs)):
            code, out, _err = run_engine(
                "count", TRINO_HOST="trino.invalid", SQL=sql, RETRIES="0",
            )
        assert code == 1, f"{sql!r}: exit {code}, stdout={out!r}"
        events = events_of(out)
        assert event_names(events) == ["error"], (sql, events)
        assert "SELECT" in events[0]["message"], (sql, events[0])
        # A fresh list, never `calls.clear()`: that returns None, so the fake would
        # hand QueryStream a connection it cannot call cursor() on and the check
        # would pass on the wrong failure.
        assert not list(calls), f"{sql!r}: the engine connected for a two-statement file"


def check_count_blank_sql_is_usage_error():
    """A blank SQL is a usage error: error event, exit 1, no network."""
    calls = []
    with fake_connect(lambda **kwargs: calls.append(kwargs)):
        code, out, _err = run_engine(
            "count", TRINO_HOST="trino.internal", SQL="", SQL_PATH="",
        )
    assert code == 1, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    assert event_names(events) == ["error"], events
    assert "SQL" in events[0]["message"], events[0]
    assert not calls, "the engine connected despite a blank SQL"


def check_count_connection_failure():
    """A connect that will not open: step connect, then one error, exit 1."""

    def refuse(**kwargs):
        raise OSError("connection refused")

    with fake_connect(refuse):
        code, out, err = run_engine(
            "count", TRINO_HOST="trino.invalid", SQL="SELECT 1", RETRIES="0",
        )
    assert code == 1, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    assert event_names(events) == ["step", "error"], events
    assert events[0] == {"event": "step", "step": "connect"}, events
    assert "OSError" in events[-1]["message"] and "connection refused" in events[-1]["message"], events
    assert "Traceback" in err and "Traceback" not in out, (err, out)


def check_count_empty_result_is_an_error_not_a_zero():
    """No row, no column and a row that is not an integer are all errors.

    A count that did not come back is not a count of zero, and `rowcount` is not
    a substitute: the fake reports a perfectly good 0 and a -1 while the cursor
    yields nothing, and neither may become the answer.
    """
    cases = [
        ([], 0),          # no rows at all, but rowcount claims zero
        ([], -1),         # trino's "the server did not say"
        ([[]], 0),        # a row with no column
        ([[None]], 0),    # a NULL
        ([["6000"]], 0),  # the number as text
        ([[6000.0]], 0),  # a float
        ([[True]], 0),    # a bool is an int in Python, and is not a count
    ]
    for rows, rowcount in cases:
        cursor, connect = count_fake(rows, rowcount=rowcount)
        with fake_connect(connect):
            code, out, err = run_engine(
                "count", TRINO_HOST="trino.internal", SQL="SELECT * FROM t", RETRIES="0",
            )
        assert code == 1, f"{rows!r} rowcount={rowcount}: exit {code}, stdout={out!r}"
        events = events_of(out)
        assert event_names(events) == ["step", "error"], (rows, events)
        assert "count" not in event_names(events), (rows, events)
        assert "Traceback" in err and "Traceback" not in out, (err, out)


def check_count_emits_a_json_number_not_a_string():
    """The total is a JSON number, and a bigint past 2**31 survives as one."""
    big = 2**31 + 7
    cursor, connect = count_fake([[big]])
    with fake_connect(connect):
        code, out, _err = run_engine(
            "count", TRINO_HOST="trino.internal", SQL="SELECT * FROM t", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    line = [line for line in out.splitlines() if json.loads(line)["event"] == "count"][0]
    assert f'"rows": {big}' in line, line  # the wire form, not just the parsed value
    counted = only_event(events_of(out), "count")
    assert counted["rows"] == big, counted
    assert isinstance(counted["rows"], int) and not isinstance(counted["rows"], bool), counted


def check_count_stdout_is_json_only():
    """Every stdout line of a count run is one compact JSON object."""
    cursor, connect = count_fake([[42]])
    with fake_connect(connect):
        code, out, _err = run_engine(
            "count", TRINO_HOST="trino.internal", SQL="SELECT * FROM t", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    lines = out.splitlines()
    assert lines, "no stdout at all"
    for line in lines:
        assert line.strip() == line and line, f"padding or a blank line in {line!r}"
        payload = json.loads(line)  # raises if any line is not exactly one JSON object
        assert isinstance(payload, dict) and isinstance(payload.get("event"), str), payload
    assert lines[-1].startswith('{"event": "done"'), lines[-1]


# --------------------------------------------------------------------------- #
# explain: the plan the server would use, built by the driver
# --------------------------------------------------------------------------- #

# A Trino plan arrives as one text column, one row per plan line.
EXPLAIN_COLUMNS = [("Query Plan", "varchar", None, None, None, None, None)]
EXPLAIN_ROWS = [
    ["Output[kode_wilayah, nama]"],
    ["  TableScan[table = hive:analytics:wilayah]"],
]

# MySQL is the one driver whose EXPLAIN is a real table rather than a text
# column, so this is the shape that proves every column and every cell position
# survives the trip.
MYSQL_EXPLAIN_COLUMNS = [
    ("id", "bigint", None, None, None, None, None),
    ("select_type", "varchar", None, None, None, None, None),
    ("table", "varchar", None, None, None, None, None),
]


def explain_fake(rows, columns=EXPLAIN_COLUMNS, **kwargs):
    """(cursor, connect factory) for one Trino explain run."""
    cursor = FakeCursor(rows, columns, **kwargs)
    return cursor, (lambda **values: FakeConnection(cursor))


def check_explain_reports_columns_rows_and_done():
    """A SELECT explains as step connect, one columns, the plan rows and a done.

    The protocol is `preview`'s, because all three servers answer EXPLAIN with a
    result set and the grid already knows how to paint one.
    """
    cursor, connect = explain_fake(EXPLAIN_ROWS)
    with fake_connect(connect):
        code, out, err = run_engine(
            "explain", TRINO_HOST="trino.internal", TRINO_USER="analyst",
            SQL="SELECT kode_wilayah, nama FROM wilayah", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}, stderr={err!r}"
    events = events_of(out)

    assert events[0] == {"event": "step", "step": "connect"}, events[0]
    assert event_names(events)[0] == "step", events  # before anything is fetched
    assert events[1] == {
        "event": "columns",
        "columns": [{"name": "Query Plan", "type": "varchar"}],
    }, events[1]
    assert all(isinstance(column["type"], str) for column in events[1]["columns"]), events[1]

    batches = [event for event in events if event["event"] == "rows"]
    assert len(batches) == 1, events
    assert batches[0]["data"] == [
        ["Output[kode_wilayah, nama]"],
        ["  TableScan[table = hive:analytics:wilayah]"],
    ], batches[0]

    done = events[-1]
    assert done["event"] == "done", events
    assert done["rows"] == 2, done
    assert done["query_id"] == QUERY_ID, done
    assert isinstance(done["elapsed_ms"], int) and done["elapsed_ms"] >= 0, done
    # A plan is not capped, so there is no `truncated` at all -- the key's
    # absence is the contract, exactly as `count`'s `done` carries no `rows`.
    assert "truncated" not in done, done
    assert only_event(events, "columns")["columns"], events


def check_wrappers_drop_a_comment_trailing_terminator():
    """Both wrappers lose a `;` that sits at the end of a trailing comment.

    A comment cannot hold the statement separator, but its `;` still ends the
    statement on the wire, so `EXPLAIN SELECT 1 -- note;` and
    `SELECT COUNT(*) FROM (SELECT 1 -- note;)` are syntax errors if it survives.
    The two wrappers reach that by different routes -- the driver peels the
    terminator after finding the separator, `count` strips it before scanning --
    so this pins the *outcome* both share rather than either implementation.
    """
    from exporter.drivers import DRIVERS

    assert DRIVERS["trino"].explain_sql("SELECT 1 -- note;") == "EXPLAIN SELECT 1 -- note", "explain"
    assert DRIVERS["postgres"].explain_sql("SELECT 1 -- note;") == "EXPLAIN SELECT 1 -- note", "pg"
    assert DRIVERS["mysql"].explain_sql("SELECT 1 -- note;") == "EXPLAIN SELECT 1 -- note", "mysql"

    # A statement separator stranded before a trailing comment must go too, or the wrap keeps a
    # `;` in the middle: this is the case that made the first version wrong. The comment itself
    # stays -- it is the caller's text, and dropping it would plan a statement they did not write.
    assert DRIVERS["trino"].explain_sql("SELECT 1; -- trailing;") == (
        "EXPLAIN SELECT 1 -- trailing"
    ), "stranded"
    assert DRIVERS["trino"].explain_sql("SELECT 1; -- note") == "EXPLAIN SELECT 1 -- note", "before"

    # A block comment is part of the statement, not a trailing aside, so it stays.
    assert DRIVERS["trino"].explain_sql("SELECT 1 /* x; */;") == "EXPLAIN SELECT 1 /* x; */", "block"

    # A literal's `;` is untouchable in either direction: it is never the final character, so
    # neither the separator rule nor the terminator peel can reach it.
    assert DRIVERS["trino"].explain_sql("SELECT 'a;b;'") == "EXPLAIN SELECT 'a;b;'", "literal"
    wrapped = engine.count_statement("SELECT 'a;b;'")
    assert wrapped == "SELECT COUNT(*) FROM (SELECT 'a;b;') AS queryhive_count", wrapped

    # And `count` loses a trailing comment's terminator too.
    wrapped = engine.count_statement("SELECT 1 -- note;")
    assert wrapped == "SELECT COUNT(*) FROM (SELECT 1 -- note) AS queryhive_count", wrapped

    # Every terminator goes, because everything after the keyword is one statement.
    assert DRIVERS["trino"].explain_sql("SELECT 1;;") == "EXPLAIN SELECT 1", "double"
    # `count` refuses two separators rather than guessing which statement the grid shows; only
    # EXPLAIN, which wraps whatever it is handed, strips them all.
    try:
        engine.count_statement("SELECT 1;;")
        raise AssertionError("count wrapped a two-statement string")
    except ValueError as error:
        assert "2 statements" in str(error), error


def check_explain_statement_per_driver():
    """Each driver builds its own statement: EXPLAIN, and the SQL byte for byte."""
    cursor, connect = explain_fake(EXPLAIN_ROWS)
    with fake_connect(connect):
        code, out, _err = run_engine(
            "explain", TRINO_HOST="trino.internal", SQL="SELECT 1", RETRIES="0",
        )
    assert code == 0, f"trino: exit {code}, stdout={out!r}"
    assert cursor.statements == ["EXPLAIN SELECT 1"], cursor.statements

    cursor = FakeCursor(EXPLAIN_ROWS, EXPLAIN_COLUMNS)
    with fake_dbapi(psycopg, lambda **kwargs: FakeConnection(cursor)):
        code, out, _err = run_engine(
            "explain", DB_KIND="postgres", DB_HOST="pg.internal", SQL="SELECT 1", RETRIES="0",
        )
    assert code == 0, f"postgres: exit {code}, stdout={out!r}"
    assert cursor.statements == ["EXPLAIN SELECT 1"], cursor.statements

    cursor = FakeCursor(
        [["1", "SIMPLE", "wilayah"]], MYSQL_EXPLAIN_COLUMNS
    )
    with fake_dbapi(pymysql, lambda **kwargs: FakeConnection(cursor)):
        code, out, _err = run_engine(
            "explain", DB_KIND="mysql", DB_HOST="mysql.internal", SQL="SELECT 1", RETRIES="0",
        )
    assert code == 0, f"mysql: exit {code}, stdout={out!r}"
    assert cursor.statements == ["EXPLAIN SELECT 1"], cursor.statements

    # And the driver owns the spelling, not the engine: the base class has no
    # statement and says so by name rather than guessing at one.
    from exporter.drivers import Driver

    try:
        Driver().explain_sql("SELECT 1")
    except ValueError as exc:
        assert "explain" in str(exc), exc
    else:
        raise AssertionError("the base Driver built an explain statement")


def check_explain_statement_comes_from_the_driver():
    """The engine asks the driver; it does not spell EXPLAIN itself.

    Each driver may spell its plan differently -- these three happen to agree,
    which is exactly why a check has to force a disagreement. A driver whose
    `explain_sql` returns something the engine could not have written on its own
    proves which side built the statement: if the engine hard-coded `EXPLAIN`,
    the cursor would receive that instead of this marker.
    """
    from exporter import drivers as drivers_module

    cursor = FakeCursor(EXPLAIN_ROWS, EXPLAIN_COLUMNS)
    real = drivers_module.DRIVERS["trino"]

    class MarkerDriver(type(real)):
        kind = "trino"

        def explain_sql(self, sql):
            # Not something any engine-side f-string could produce, and it keeps
            # the caller's own text so a leak of the raw SQL shows up too.
            return f"SHOW PLAN FOR <<{sql}>>"

    drivers_module.DRIVERS["trino"] = MarkerDriver()
    try:
        with fake_connect(lambda **kwargs: FakeConnection(cursor)):
            code, out, _err = run_engine(
                "explain", TRINO_HOST="trino.internal", SQL="SELECT 1", RETRIES="0",
            )
    finally:
        drivers_module.DRIVERS["trino"] = real
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == ["SHOW PLAN FOR <<SELECT 1>>"], cursor.statements
    assert events_of(out)[0] == {"event": "step", "step": "connect"}, out

    # The engine hands the driver the caller's SQL as written, and the driver
    # applies its own trailing-semicolon rule: stripping is part of building the
    # statement, so a driver that builds it differently is left free to. The
    # engine does not pre-edit the caller's text on the driver's behalf.
    cursor = FakeCursor(EXPLAIN_ROWS, EXPLAIN_COLUMNS)
    drivers_module.DRIVERS["trino"] = MarkerDriver()
    try:
        with fake_connect(lambda **kwargs: FakeConnection(cursor)):
            code, out, _err = run_engine(
                "explain", TRINO_HOST="trino.internal", SQL="SELECT 1;  ", RETRIES="0",
            )
    finally:
        drivers_module.DRIVERS["trino"] = real
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == ["SHOW PLAN FOR <<SELECT 1;  >>"], cursor.statements

    # And the real driver does strip it, because that is what EXPLAIN needs.
    cursor = FakeCursor(EXPLAIN_ROWS, EXPLAIN_COLUMNS)
    with fake_connect(lambda **kwargs: FakeConnection(cursor)):
        code, out, _err = run_engine(
            "explain", TRINO_HOST="trino.internal", SQL="SELECT 1;  ", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == ["EXPLAIN SELECT 1"], cursor.statements


def check_explain_strips_one_trailing_semicolon():
    """`SELECT 1;  ` explains as `EXPLAIN SELECT 1`: EXPLAIN SELECT 1; is invalid.

    Only the final separator goes. The whitespace around and after it goes too,
    so the statement handed to the cursor has no trailing debris.
    """
    for sql, expected in (
        ("SELECT 1", "EXPLAIN SELECT 1"),
        ("SELECT 1;", "EXPLAIN SELECT 1"),
        ("SELECT 1;  ", "EXPLAIN SELECT 1"),
        ("  SELECT 1 ;  \n", "EXPLAIN SELECT 1"),
        ("SELECT * FROM t LIMIT 5;", "EXPLAIN SELECT * FROM t LIMIT 5"),
    ):
        cursor, connect = explain_fake(EXPLAIN_ROWS)
        with fake_connect(connect):
            code, out, _err = run_engine(
                "explain", TRINO_HOST="trino.internal", SQL=sql, RETRIES="0",
            )
        assert code == 0, f"{sql!r}: exit {code}, stdout={out!r}"
        assert cursor.statements == [expected], (sql, cursor.statements)

    # A semicolon in the middle is not the final one, and is left where it is:
    # removing it would be rewriting a statement the caller is asking about.
    cursor, connect = explain_fake(EXPLAIN_ROWS)
    with fake_connect(connect):
        code, out, _err = run_engine(
            "explain", TRINO_HOST="trino.internal", SQL="SELECT 1; SELECT 2", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == ["EXPLAIN SELECT 1; SELECT 2"], cursor.statements


def check_explain_keeps_a_semicolon_inside_a_literal():
    """`SELECT 'a;b'` explains as `EXPLAIN SELECT 'a;b'`: the `;` is text.

    The cases that matter are the ones where a semicolon sits at the very end of
    the statement but is *not* the terminator, because that is where a naive
    `rstrip(";")` and this scanner disagree: it would eat the `;` inside a
    trailing literal and hand the server a statement the caller never wrote.

    A `;` inside a *comment* survives too, for the same reason a literal's does:
    it may be text the caller wrote deliberately, and this function strips only
    what the scanner reports as a real statement separator.

    Note the last two cases. The driver's own rule leaves a comment's trailing
    `;` alone, but `QueryStream` re-applies a naive `sql.strip().rstrip(";")` to
    whatever it is handed (exporter/source.py), so a `;` at the very end of a
    comment is removed one layer below this command regardless. These
    expectations describe what the cursor really receives; the rule the driver
    actually implements is asserted directly in
    `check_strip_one_trailing_semicolon_directly`.
    """
    for sql, expected in (
        ("SELECT 'a;b'", "EXPLAIN SELECT 'a;b'"),
        ("SELECT 'a;b';", "EXPLAIN SELECT 'a;b'"),
        ('SELECT "a;b"', 'EXPLAIN SELECT "a;b"'),
        ("SELECT 'it''s; ok'", "EXPLAIN SELECT 'it''s; ok'"),
        # A literal that ends the statement: its closing `;` is text, not a
        # terminator, so nothing is stripped.
        ("SELECT 'a;b;'", "EXPLAIN SELECT 'a;b;'"),
        ("SELECT 'a;b;' ;", "EXPLAIN SELECT 'a;b;'"),
        # A `;` in a comment that is not final is untouched by either layer.
        ("/* one; two */ SELECT 1", "EXPLAIN /* one; two */ SELECT 1"),
        ("SELECT 1 /* one; two */", "EXPLAIN SELECT 1 /* one; two */"),
        ("SELECT 1 -- one; two", "EXPLAIN SELECT 1 -- one; two"),
        # ... and one that ends the statement: QueryStream's own strip removes
        # it, so the cursor does not see it even though the driver kept it.
        ("SELECT 1 -- one; two;", "EXPLAIN SELECT 1 -- one; two"),
        # The separator before the comment goes, and the comment stays. Its own `;` goes too,
        # because a comment runs to the end of its line and so does the statement.
        ("SELECT 1; -- trailing;", "EXPLAIN SELECT 1 -- trailing"),
    ):
        cursor, connect = explain_fake(EXPLAIN_ROWS)
        with fake_connect(connect):
            code, out, _err = run_engine(
                "explain", TRINO_HOST="trino.internal", SQL=sql, RETRIES="0",
            )
        assert code == 0, f"{sql!r}: exit {code}, stdout={out!r}"
        assert cursor.statements == [expected], (sql, cursor.statements)


def check_strip_one_trailing_semicolon_directly():
    """The stripper's own contract, asserted where it actually lives.

    The end-to-end checks around this one cannot pin it down: `QueryStream`
    re-applies `sql.strip().rstrip(";")` to whatever it is handed
    (exporter/source.py), so by the time a cursor sees the statement the
    driver's more careful strip has been flattened into the same answer for
    every input tried. That makes the driver's version redundant *today* -- but
    it is the one that is correct, and the naive one would corrupt
    `SELECT 'a;b;'` the moment that outer layer changes. So the rule is asserted
    here, on the function, rather than through a path that cannot observe it.
    """
    from exporter.drivers import strip_one_trailing_semicolon as strip

    cases = (
        # An ordinary terminator goes, with the whitespace around it.
        ("SELECT 1", "SELECT 1"),
        ("SELECT 1;", "SELECT 1"),
        ("SELECT 1;  ", "SELECT 1"),
        ("  SELECT 1 ;  \n", "SELECT 1"),
        # A `;` inside a literal is the caller's text and never goes.
        ("SELECT 'a;b'", "SELECT 'a;b'"),
        ("SELECT 'a;b';", "SELECT 'a;b'"),
        ("SELECT 'a;b;'", "SELECT 'a;b;'"),      # ends the statement, still text
        ("SELECT 'a;b;' ;", "SELECT 'a;b;'"),
        ('SELECT "a;b;"', 'SELECT "a;b;"'),
        ("SELECT 'it''s; ok';", "SELECT 'it''s; ok'"),
        # A `;` inside a comment survives, like a literal's: the scanner does
        # not report it as a separator, so it is not the terminator this strips.
        ("SELECT 1 -- note;", "SELECT 1 -- note;"),
        ("SELECT 1; -- note;", "SELECT 1; -- note;"),
        # Only the final separator is touched, never one in the middle.
        ("SELECT 1; SELECT 2", "SELECT 1; SELECT 2"),
        ("SELECT 'a;b';;", "SELECT 'a;b';"),
        ("", ""),
    )
    for sql, expected in cases:
        got = strip(sql)
        assert got == expected, (sql, got, expected)

    # The inputs a naive `rstrip(";")` gets wrong, which is why the scanner
    # exists: it eats a `;` that is comment text rather than a separator, and it
    # eats the caller's second separator rather than only the final terminator.
    for sql in ("SELECT 1 -- note;", "SELECT 'a;b';;", "SELECT 'a;b;' ;"):
        assert sql.strip().rstrip(";") != strip(sql), \
            f"the naive strip and the scanner-aware one agree on {sql!r}"
    assert strip("SELECT 1 -- note;") == "SELECT 1 -- note;", strip("SELECT 1 -- note;")
    assert strip("SELECT 'a;b';;") == "SELECT 'a;b';", strip("SELECT 'a;b';;")
    assert strip("SELECT 'a;b;' ;") == "SELECT 'a;b;'", strip("SELECT 'a;b;' ;")


def check_explain_multi_column_row_arrives_intact():
    """The MySQL shape: every column is reported and a two-column row survives.

    A plan table is wider than one text column, so the engine may not flatten,
    reorder or drop positions -- the grid aligns cells by index.
    """
    rows = [
        ["1", "SIMPLE", "wilayah"],
        ["1", "PRIMARY", None],
    ]
    cursor, connect = explain_fake(rows, MYSQL_EXPLAIN_COLUMNS)
    with fake_dbapi(pymysql, connect):
        code, out, _err = run_engine(
            "explain", DB_KIND="mysql", DB_HOST="mysql.internal",
            SQL="SELECT * FROM wilayah", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    events = events_of(out)

    columns = only_event(events, "columns")["columns"]
    assert [column["name"] for column in columns] == ["id", "select_type", "table"], columns
    assert all(isinstance(column["type"], str) for column in columns), columns

    sent = preview_rows(events)
    assert sent == [["1", "SIMPLE", "wilayah"], ["1", "PRIMARY", None]], sent
    assert len(sent[0]) == 3, sent[0]  # every position, none dropped
    assert sent[0][1] == "SIMPLE" and sent[1][1] == "PRIMARY", sent  # order preserved
    assert events[-1]["rows"] == 2, events[-1]


def check_explain_null_cell_is_json_null():
    """A NULL plan line is JSON null: not "None", not an empty string."""
    cursor, connect = explain_fake([["Output[]"], [None], [""]])
    with fake_connect(connect):
        code, out, _err = run_engine(
            "explain", TRINO_HOST="trino.internal", SQL="SELECT 1", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    lines = out.splitlines()
    rows_lines = [line for line in lines if json.loads(line)["event"] == "rows"]
    assert len(rows_lines) == 1, lines
    assert "null" in rows_lines[0], rows_lines[0]  # the wire form, not just the parsed value
    assert "None" not in rows_lines[0], rows_lines[0]
    assert preview_rows(events_of(out)) == [["Output[]"], [None], [""]], events_of(out)


def check_explain_blank_sql_is_usage_error():
    """A blank SQL is a usage error: one error event, exit 1, no network.

    Whitespace-only counts as blank, and no `step connect` is emitted: the
    verdict is made before the network is touched, exactly as `preview`'s is.
    """
    for sql in ("", "   ", "\n\t "):
        calls = []
        with fake_connect(lambda **kwargs: calls.append(kwargs)):
            code, out, _err = run_engine(
                "explain", TRINO_HOST="trino.internal", SQL=sql, SQL_PATH="",
            )
        assert code == 1, f"{sql!r}: exit {code}, stdout={out!r}"
        events = events_of(out)
        assert event_names(events) == ["error"], (sql, events)
        assert "SQL" in events[0]["message"], (sql, events[0])
        assert not calls, f"{sql!r}: the engine connected despite a blank SQL"

    # And a SQL_PATH that cannot be read is the same kind of usage error.
    calls = []
    with fake_connect(lambda **kwargs: calls.append(kwargs)):
        code, out, _err = run_engine(
            "explain", TRINO_HOST="trino.internal", SQL="",
            SQL_PATH="/nonexistent/queryhive/nope.sql",
        )
    assert code == 1, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    assert event_names(events) == ["error"], events
    assert "SQL_PATH" in events[0]["message"], events[0]
    assert not calls, "the engine connected despite an unreadable SQL_PATH"


def check_explain_connection_failure():
    """A connect that will not open: step connect, then one error, exit 1."""

    def refuse(**kwargs):
        raise OSError("connection refused")

    with fake_connect(refuse):
        code, out, err = run_engine(
            "explain", TRINO_HOST="trino.invalid", SQL="SELECT 1", RETRIES="0",
        )
    assert code == 1, f"exit {code}, stdout={out!r}"
    events = events_of(out)
    assert event_names(events) == ["step", "error"], events
    assert events[0] == {"event": "step", "step": "connect"}, events
    assert "OSError" in events[-1]["message"] and "connection refused" in events[-1]["message"], events
    assert "done" not in event_names(events), events
    assert "columns" not in event_names(events), events
    assert "Traceback" in err and "Traceback" not in out, (err, out)


def check_explain_stdout_is_json_only():
    """Every stdout line of an explain run is one compact JSON object."""
    batch = engine.PREVIEW_BATCH
    cursor, connect = explain_fake(
        [[f"plan line {i}"] for i in range(batch + 3)],
        columns=EXPLAIN_COLUMNS,
    )
    with fake_connect(connect):
        code, out, _err = run_engine(
            "explain", TRINO_HOST="trino.internal", SQL="SELECT * FROM t", RETRIES="0",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    lines = out.splitlines()
    assert lines, "no stdout at all"
    for line in lines:
        assert line.strip() == line and line, f"padding or a blank line in {line!r}"
        payload = json.loads(line)  # raises if any line is not exactly one JSON object
        assert isinstance(payload, dict) and isinstance(payload.get("event"), str), payload
    assert lines[-1].startswith('{"event": "done"'), lines[-1]
    # More than one batch for a plan longer than PREVIEW_BATCH, all of it sent.
    events = events_of(out)
    batches = [event for event in events if event["event"] == "rows"]
    assert len(batches) > 1, "a plan larger than PREVIEW_BATCH arrived as one event"
    assert all(len(event["data"]) <= batch for event in batches), [len(e["data"]) for e in batches]
    assert len(preview_rows(events)) == batch + 3, events[-1]


# --------------------------------------------------------------------------- #
# postgres and mysql: the same protocol, three different drivers
# --------------------------------------------------------------------------- #

MYSQL_DATABASES_SQL = (
    "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA "
    "WHERE SCHEMA_NAME NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys') "
    "ORDER BY 1"
)

MYSQL_ALL_DATABASES_SQL = (
    "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA ORDER BY 1"
)

POSTGRES_DATABASES_SQL = (
    "SELECT datname FROM pg_database "
    "WHERE NOT datistemplate AND datallowconn ORDER BY 1"
)

POSTGRES_SCHEMAS_SQL = (
    "SELECT schema_name FROM information_schema.schemata "
    "WHERE schema_name NOT LIKE 'pg\\_%' "
    "AND schema_name <> 'information_schema' ORDER BY 1"
)


def postgres_tables_sql(schema):
    return (
        "SELECT table_name FROM information_schema.tables "
        f"WHERE table_schema = '{schema}' AND table_type = 'BASE TABLE' ORDER BY 1"
    )


class NoStatsConnection:
    """A connection whose cursor() takes nothing at all.

    The real psycopg and pymysql cursors accept no `stats_callback`, so this
    fake does not either: if to_table ever handed one over, the TypeError the
    bundled interpreter would raise shows up here instead of only in a live run.
    """

    def __init__(self, cursor):
        self._cursor = cursor
        self.cursor_calls = 0
        self.closed = False

    def cursor(self):
        self.cursor_calls += 1
        return self._cursor

    def close(self):
        self.closed = True


def check_postgres_kind_reaches_psycopg():
    """DB_KIND=postgres with DB_HOST opens psycopg, never Trino; aliases still do.

    The two halves of the environment contract in one place: a driver is chosen
    by DB_KIND and its connect path is the client that driver names, while the
    older TRINO_* names keep reaching the Trino path they always did.
    """
    trino_calls = []
    pg_seen = {}
    cursor = FakeCursor([("public",)])

    def pg_connect(**kwargs):
        pg_seen.update(kwargs)
        return FakeConnection(cursor)

    def trino_connect(**kwargs):
        trino_calls.append(kwargs)
        return FakeConnection(FakeCursor())

    with fake_dbapi(psycopg, pg_connect), fake_connect(trino_connect):
        code, out, _err = run_engine(
            "test", DB_KIND="postgres", DB_HOST="pg.internal", DB_PORT="5433",
            DB_USER="analyst", DB_DATABASE="appdb", DB_PASSWORD="secret",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert not trino_calls, "DB_KIND=postgres still went through trino.dbapi.connect"
    assert pg_seen["host"] == "pg.internal" and pg_seen["port"] == 5433, pg_seen
    assert pg_seen["user"] == "analyst" and pg_seen["dbname"] == "appdb", pg_seen
    assert pg_seen["password"] == "secret", pg_seen
    # `test` runs the driver's first available browse statement as its probe, and Postgres now
    # answers `catalogs` -- so the probe is the database list rather than the schema list. The
    # point of this assertion is that the probe is real SQL for the driver chosen, not which SQL.
    assert cursor.statements == [POSTGRES_DATABASES_SQL], cursor.statements  # test's own probe
    assert cursor.closed, "the engine left a handle open"

    trino_seen = {}
    trino_cursor = FakeCursor([("system",), ("hive",)])

    def trino_connect_ok(**kwargs):
        trino_seen.update(kwargs)
        return FakeConnection(trino_cursor)

    with fake_connect(trino_connect_ok):
        code, out, _err = run_engine(
            "test", TRINO_HOST="trino.internal", TRINO_PORT="8081", TRINO_USER="analyst",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert trino_seen["host"] == "trino.internal" and trino_seen["port"] == 8081, trino_seen
    assert trino_seen["user"] == "analyst", trino_seen
    assert trino_cursor.statements == ["SHOW CATALOGS"], trino_cursor.statements
    assert only_event(events_of(out), "test")["catalog_count"] == 2


def check_db_names_beat_trino_aliases():
    """DB_* wins when both it and its TRINO_* alias name the same setting."""
    trino_seen = {}

    def connect(**kwargs):
        trino_seen.update(kwargs)
        return FakeConnection(FakeCursor([("hive",)]))

    with fake_connect(connect):
        code, out, _err = run_engine(
            "test",
            DB_HOST="db.internal", TRINO_HOST="trino.internal",
            DB_PORT="9999", TRINO_PORT="8081",
            DB_USER="dbuser", TRINO_USER="trinuser",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert trino_seen["host"] == "db.internal", trino_seen
    assert trino_seen["port"] == 9999, trino_seen
    assert trino_seen["user"] == "dbuser", trino_seen

    # DB_DATABASE is TRINO_CATALOG's successor, so it names the connection's
    # catalog too -- and, on postgres, the database.
    trino_seen = {}
    with fake_connect(connect):
        code, out, _err = run_engine(
            "test", DB_HOST="trino.internal", DB_DATABASE="hive", TRINO_CATALOG="other",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert trino_seen["catalog"] == "hive", trino_seen

    pg_seen = {}

    def pg_connect(**kwargs):
        pg_seen.update(kwargs)
        return FakeConnection(FakeCursor([("public",)]))

    with fake_dbapi(psycopg, pg_connect):
        code, out, _err = run_engine(
            "test", DB_KIND="postgres", DB_HOST="pg.internal",
            DB_DATABASE="appdb", TRINO_CATALOG="hive",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert pg_seen["dbname"] == "appdb", pg_seen


def check_identifier_quoting_per_driver():
    """Each driver doubles its own quote character and leaves the other's alone."""
    from exporter.drivers import DRIVERS

    assert DRIVERS["trino"].quote('we"ird') == '"we""ird"', DRIVERS["trino"].quote('we"ird')
    assert DRIVERS["postgres"].quote('we"ird') == '"we""ird"', DRIVERS["postgres"].quote('we"ird')
    assert DRIVERS["mysql"].quote("we`ird") == "`we``ird`", DRIVERS["mysql"].quote("we`ird")
    # A name holding the other driver's quote char is not this driver's business.
    assert DRIVERS["mysql"].quote('we"ird') == '`we"ird`', DRIVERS["mysql"].quote('we"ird')
    assert DRIVERS["trino"].quote("we`ird") == '"we`ird"', DRIVERS["trino"].quote("we`ird")
    assert DRIVERS["postgres"].quote("we`ird") == '"we`ird"', DRIVERS["postgres"].quote("we`ird")


def check_qualified_drops_the_level_it_lacks():
    """One target, three statements: the parts a driver has no level for go."""
    from exporter.drivers import DRIVERS

    assert DRIVERS["trino"].qualified("hive", "analytics", "foo") == '"hive"."analytics"."foo"'
    assert DRIVERS["postgres"].qualified("hive", "analytics", "foo") == '"analytics"."foo"'
    assert DRIVERS["mysql"].qualified("hive", "analytics", "foo") == "`hive`.`foo`"
    # Postgres cannot reach another database and MySQL has no schema: each drops
    # the slot it has no place for rather than writing a name that cannot exist.
    assert DRIVERS["postgres"].slots == ("schema", "table")
    assert DRIVERS["mysql"].slots == ("database", "table")


def check_postgres_has_no_catalog_level():
    """Postgres lists its databases but has no catalog *level*; schemas and tables work.

    `catalogs` used to be a usage error here on the grounds that a connection cannot query across
    databases. That is still true of the object tree -- `levels` is unchanged, so a Postgres
    connection is still schema-first -- but the statement is answerable, and it is what lets a query
    be pointed at another database.
    """
    cursor = FakeCursor([("appdb",), ("warehouse",)])

    with fake_dbapi(psycopg, lambda **kwargs: FakeConnection(cursor)):
        code, out, _err = run_engine("catalogs", DB_KIND="postgres", DB_HOST="pg.internal")
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == [POSTGRES_DATABASES_SQL], cursor.statements
    assert only_event(events_of(out), "catalogs")["names"] == ["appdb", "warehouse"]

    # The tree's contract is untouched: no catalog node appears under a Postgres connection.
    from exporter.drivers import DRIVERS
    assert DRIVERS["postgres"].levels == ("schema", "table"), DRIVERS["postgres"].levels

    cases = (
        ("schemas", [("public",), ("analytics",)], {}, POSTGRES_SCHEMAS_SQL),
        ("tables", [("wilayah",)], {"DB_SCHEMA": "analytics"}, postgres_tables_sql("analytics")),
    )
    for command, rows, values, expected in cases:
        cursor = FakeCursor(rows)

        def pg_connect(**kwargs):
            return FakeConnection(cursor)

        with fake_dbapi(psycopg, pg_connect):
            code, out, _err = run_engine(
                command, DB_KIND="postgres", DB_HOST="pg.internal", **values
            )
        assert code == 0, f"{command}: exit {code}, stdout={out!r}"
        assert cursor.statements == [expected], cursor.statements
        assert only_event(events_of(out), command)["names"] == [row[0] for row in rows]

    # A blank DB_SCHEMA is a usage error before the network, named as a setting.
    calls = []
    with fake_dbapi(psycopg, lambda **kwargs: calls.append(kwargs) or FakeConnection(FakeCursor())):
        code, out, _err = run_engine("tables", DB_KIND="postgres", DB_HOST="pg.internal")
    assert code == 1, f"exit {code}, stdout={out!r}"
    message = events_of(out)[0]["message"]
    assert "DB_SCHEMA" in message and "TRINO_SCHEMA" in message, message
    assert not calls, "the engine connected despite a blank schema"


def check_postgres_show_all_schemas():
    """DB_ALL_SCHEMAS=1 drops the system-schema filter; off keeps it."""
    all_schemas_sql = (
        "SELECT schema_name FROM information_schema.schemata ORDER BY 1"
    )

    cursor = FakeCursor([("public",), ("analytics",)])

    def pg_connect(**kwargs):
        return FakeConnection(cursor)

    with fake_dbapi(psycopg, pg_connect):
        code, out, _err = run_engine(
            "schemas", DB_KIND="postgres", DB_HOST="pg.internal", DB_ALL_SCHEMAS="1",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == [all_schemas_sql], cursor.statements

    with fake_dbapi(psycopg, pg_connect):
        code, out, _err = run_engine("schemas", DB_KIND="postgres", DB_HOST="pg.internal")
    assert code == 0, f"exit {code}, stdout={out!r}"
    # The last statement is the default (filtered) one: the flag stays off unless asked.
    assert cursor.statements == [all_schemas_sql, POSTGRES_SCHEMAS_SQL], cursor.statements


def check_show_all_schemas_is_a_noop_on_trino():
    """DB_ALL_SCHEMAS=1 must not crash a driver that already lists every schema.

    Trino's SHOW SCHEMAS returns system schemas as a matter of course, so the
    flag cannot reveal anything it does not already show — and more to the point,
    it must not turn the schemas command into a TypeError for a driver that has
    nothing to filter.
    """
    cursor = FakeCursor([("analytics",), ("information_schema",)])
    connection = FakeConnection(cursor)

    def connect(**kwargs):
        return connection

    with fake_connect(connect):
        code, out, _err = run_engine(
            "schemas", TRINO_HOST="trino.internal", TRINO_CATALOG="hive",
            DB_ALL_SCHEMAS="1",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == ['SHOW SCHEMAS FROM "hive"'], cursor.statements
    # Both the user schema and the system one come back; nothing was hidden to begin with.
    assert only_event(events_of(out), "schemas")["names"] == ["analytics", "information_schema"]


def check_mysql_show_all_databases():
    """DB_ALL_SCHEMAS=1 reveals MySQL's system databases; off keeps them hidden."""
    cursor = FakeCursor([("mydb",)])

    def connect(**kwargs):
        return FakeConnection(cursor)

    with fake_dbapi(pymysql, connect):
        code, out, _err = run_engine(
            "catalogs", DB_KIND="mysql", DB_HOST="mysql.internal", DB_ALL_SCHEMAS="1",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == [MYSQL_ALL_DATABASES_SQL], cursor.statements

    with fake_dbapi(pymysql, connect):
        code, out, _err = run_engine("catalogs", DB_KIND="mysql", DB_HOST="mysql.internal")
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == [MYSQL_ALL_DATABASES_SQL, MYSQL_DATABASES_SQL], cursor.statements


def check_mysql_catalogs_are_databases():
    """MySQL's top level is its databases, and `tables` needs the database level."""
    seen = {}
    cursor = FakeCursor([("information_schema",), ("mydb",)])

    def connect(**kwargs):
        seen.update(kwargs)
        return FakeConnection(cursor)

    with fake_dbapi(pymysql, connect):
        code, out, _err = run_engine("catalogs", DB_KIND="mysql", DB_HOST="mysql.internal",
                                     DB_USER="root")
    assert code == 0, f"exit {code}, stdout={out!r}"
    # information_schema.SCHEMATA, not SHOW DATABASES: same list, but a WHERE clause can filter it
    # and SHOW cannot.
    assert cursor.statements == [MYSQL_DATABASES_SQL], cursor.statements
    assert only_event(events_of(out), "catalogs")["names"] == ["information_schema", "mydb"]
    assert seen["host"] == "mysql.internal" and seen["port"] == 3306, seen
    assert seen["user"] == "root", seen

    cursor = FakeCursor([("orders",), ("users",)])
    seen = {}

    def connect_tables(**kwargs):
        seen.update(kwargs)
        return FakeConnection(cursor)

    with fake_dbapi(pymysql, connect_tables):
        code, out, _err = run_engine("tables", DB_KIND="mysql", DB_HOST="mysql.internal",
                                     DB_DATABASE="mydb", DB_SCHEMA="ignored")
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == ["SHOW TABLES FROM `mydb`"], cursor.statements
    assert seen["database"] == "mydb", seen
    assert only_event(events_of(out), "tables")["names"] == ["orders", "users"]

    # MySQL has no schema level at all.
    calls = []
    with fake_dbapi(pymysql, lambda **kwargs: calls.append(kwargs) or FakeConnection(FakeCursor())):
        code, out, _err = run_engine("schemas", DB_KIND="mysql", DB_HOST="mysql.internal")
    assert code == 1, f"exit {code}, stdout={out!r}"
    message = events_of(out)[0]["message"]
    assert "mysql" in message and "schema" in message, message
    assert not calls, "the engine connected for a command the driver cannot answer"

    # And a blank database is a usage error before the network.
    calls = []
    with fake_dbapi(pymysql, lambda **kwargs: calls.append(kwargs) or FakeConnection(FakeCursor())):
        code, out, _err = run_engine("tables", DB_KIND="mysql", DB_HOST="mysql.internal")
    assert code == 1, f"exit {code}, stdout={out!r}"
    assert "DB_DATABASE" in events_of(out)[0]["message"], events_of(out)[0]
    assert not calls, "the engine connected despite a blank database"


def check_to_table_builds_each_drivers_statement():
    """`create` and `replace` through postgres and mysql, quoted their way."""
    cursor = FakeCursor(rowcount=3)

    def pg_connect(**kwargs):
        return FakeConnection(cursor)

    with fake_dbapi(psycopg, pg_connect):
        code, out, _err = run_engine(
            "to_table", DB_KIND="postgres", DB_HOST="pg.internal", SQL="SELECT 1",
            TARGET_CATALOG="ignored", TARGET_SCHEMA="analytics", TARGET_TABLE="foo",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == ['CREATE TABLE "analytics"."foo" AS SELECT 1'], cursor.statements
    done = events_of(out)[-1]
    assert done["table"] == "analytics.foo", done  # the reference the driver wrote
    assert done["mode"] == "create" and done["rows"] == 3, done

    cursors = []

    def pg_factory():
        fresh = FakeCursor(rowcount=1)
        cursors.append(fresh)
        return fresh

    with fake_dbapi(psycopg, lambda **kwargs: FakeConnection(pg_factory)):
        code, out, _err = run_engine(
            "to_table", DB_KIND="postgres", DB_HOST="pg.internal", SQL="SELECT 1",
            WRITE_MODE="replace", TARGET_SCHEMA="analytics", TARGET_TABLE="foo",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert [c.statements for c in cursors] == [
        ['DROP TABLE IF EXISTS "analytics"."foo"'],
        ['CREATE TABLE "analytics"."foo" AS SELECT 1'],
    ], [c.statements for c in cursors]
    assert events_of(out)[-1]["warnings"], events_of(out)[-1]

    cursor = FakeCursor(rowcount=2)

    def my_connect(**kwargs):
        return FakeConnection(cursor)

    with fake_dbapi(pymysql, my_connect):
        code, out, _err = run_engine(
            "to_table", DB_KIND="mysql", DB_HOST="mysql.internal", SQL="SELECT 1",
            TARGET_CATALOG="mydb", TARGET_SCHEMA="ignored", TARGET_TABLE="foo",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == ["CREATE TABLE `mydb`.`foo` AS SELECT 1"], cursor.statements
    assert events_of(out)[-1]["table"] == "mydb.foo", events_of(out)[-1]

    cursors = []

    def my_factory():
        fresh = FakeCursor(rowcount=1)
        cursors.append(fresh)
        return fresh

    with fake_dbapi(pymysql, lambda **kwargs: FakeConnection(my_factory)):
        code, out, _err = run_engine(
            "to_table", DB_KIND="mysql", DB_HOST="mysql.internal", SQL="SELECT 1",
            WRITE_MODE="replace", TARGET_CATALOG="mydb", TARGET_TABLE="foo",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert [c.statements for c in cursors] == [
        ["DROP TABLE IF EXISTS `mydb`.`foo`"],
        ["CREATE TABLE `mydb`.`foo` AS SELECT 1"],
    ], [c.statements for c in cursors]

    # A quote character inside a target part is doubled by the driver that owns it.
    cursor = FakeCursor(rowcount=1)
    with fake_dbapi(psycopg, lambda **kwargs: FakeConnection(cursor)):
        code, out, _err = run_engine(
            "to_table", DB_KIND="postgres", DB_HOST="pg.internal", SQL="SELECT 1",
            TARGET_SCHEMA='an"alytics', TARGET_TABLE="foo",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == ['CREATE TABLE "an""alytics"."foo" AS SELECT 1'], cursor.statements

    cursor = FakeCursor(rowcount=1)
    with fake_dbapi(pymysql, lambda **kwargs: FakeConnection(cursor)):
        code, out, _err = run_engine(
            "to_table", DB_KIND="mysql", DB_HOST="mysql.internal", SQL="SELECT 1",
            TARGET_CATALOG="my`db", TARGET_TABLE="fo`o",
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == ["CREATE TABLE `my``db`.`fo``o` AS SELECT 1"], cursor.statements


def check_to_table_ignores_the_level_a_driver_lacks():
    """Postgres does not need TARGET_CATALOG, MySQL does not need TARGET_SCHEMA."""
    cursor = FakeCursor(rowcount=1)
    with fake_dbapi(psycopg, lambda **kwargs: FakeConnection(cursor)):
        code, out, _err = run_engine(
            "to_table", DB_KIND="postgres", DB_HOST="pg.internal", SQL="SELECT 1",
            TARGET_SCHEMA="analytics", TARGET_TABLE="foo",  # no TARGET_CATALOG at all
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == ['CREATE TABLE "analytics"."foo" AS SELECT 1'], cursor.statements

    cursor = FakeCursor(rowcount=1)
    with fake_dbapi(pymysql, lambda **kwargs: FakeConnection(cursor)):
        code, out, _err = run_engine(
            "to_table", DB_KIND="mysql", DB_HOST="mysql.internal", SQL="SELECT 1",
            TARGET_CATALOG="mydb", TARGET_TABLE="foo",  # no TARGET_SCHEMA at all
        )
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert cursor.statements == ["CREATE TABLE `mydb`.`foo` AS SELECT 1"], cursor.statements

    # The part that IS required is still a usage error before the network.
    for kind, module, values in (
        ("postgres", psycopg, {"TARGET_SCHEMA": "analytics"}),
        ("mysql", pymysql, {"TARGET_CATALOG": "mydb"}),
    ):
        calls = []
        with fake_dbapi(module, lambda **kwargs: calls.append(kwargs) or FakeConnection(FakeCursor())):
            code, out, _err = run_engine(
                "to_table", DB_KIND=kind, DB_HOST="db.internal", SQL="SELECT 1", **values
            )
        assert code == 1, f"{kind}: exit {code}, stdout={out!r}"
        events = events_of(out)
        assert event_names(events) == ["error"], events
        assert "TARGET_TABLE" in events[0]["message"], events[0]
        assert not calls, f"{kind}: the engine connected despite a blank target"


def check_drivers_without_stats_get_no_stats_callback():
    """psycopg and pymysql cursors take no stats_callback, and none is passed.

    `NoStatsConnection.cursor()` refuses every keyword, which is the real
    signature of both clients: handing one the trino keyword would be a
    TypeError on the first statement, and a driver with no progress support must
    simply report no progress instead.
    """
    for kind, module, values in (
        ("postgres", psycopg, {"TARGET_SCHEMA": "analytics"}),
        ("mysql", pymysql, {"TARGET_CATALOG": "mydb"}),
    ):
        cursor = FakeCursor(rowcount=4)
        connection = NoStatsConnection(cursor)
        seen = {}

        def connect(**kwargs):
            seen.update(kwargs)
            return connection

        with fake_dbapi(module, connect):
            code, out, err = run_engine(
                "to_table", DB_KIND=kind, DB_HOST="db.internal", SQL="SELECT 1",
                WRITE_MODE="replace", TARGET_TABLE="foo", **values
            )
        assert code == 0, f"{kind}: exit {code}, stdout={out!r}, stderr={err!r}"
        assert "unexpected keyword argument" not in err, err
        assert connection.cursor_calls == 2, connection.cursor_calls  # DROP and CREATE
        assert "stats_callback" not in seen, seen
        events = events_of(out)
        assert [event["step"] for event in events if event["event"] == "step"] == ["connect", "write"]
        # One progress event, and it is the runner's final "true total" line --
        # there is no stats callback to tick it, so no intermediate progress.
        progress = [event for event in events if event["event"] == "progress"]
        assert [event["rows"] for event in progress] == [4], events
        assert all(event["state"] is None for event in progress), progress
        done = events[-1]
        assert done["event"] == "done" and done["rows"] == 4, done
        assert done["cancelled"] is False, done
        assert done["warnings"] and "dropped" in done["warnings"][0], done


def check_db_drivers_reports_every_kind():
    """`db_drivers` describes all three: label, default port and tree levels."""
    code, out, _err = run_engine("db_drivers")  # no settings at all, and no network
    assert code == 0, f"exit {code}, stdout={out!r}"
    assert events_of(out) == [{
        "event": "drivers",
        "drivers": [
            {"kind": "trino", "label": "Trino", "default_port": 8080,
             "levels": ["catalog", "schema", "table"]},
            {"kind": "postgres", "label": "PostgreSQL", "default_port": 5432,
             "levels": ["schema", "table"]},
            {"kind": "mysql", "label": "MySQL", "default_port": 3306,
             "levels": ["database", "table"]},
        ],
    }], out


def check_postgres_and_mysql_stdout_is_json_only():
    """Every stdout line of a postgres and of a mysql run is one compact JSON object."""
    runs = (
        ("postgres", psycopg, "schemas", [("public",), ("analytics",)], {}),
        ("postgres", psycopg, "to_table", [], {"TARGET_SCHEMA": "analytics", "TARGET_TABLE": "foo"}),
        ("mysql", pymysql, "catalogs", [("information_schema",), ("mydb",)], {}),
        ("mysql", pymysql, "to_table", [], {"TARGET_CATALOG": "mydb", "TARGET_TABLE": "foo"}),
    )
    for kind, module, command, rows, values in runs:
        cursor = FakeCursor(rows, rowcount=8)
        with fake_dbapi(module, lambda **kwargs: FakeConnection(cursor)):
            code, out, _err = run_engine(
                command, DB_KIND=kind, DB_HOST="db.internal", SQL="SELECT 1", **values
            )
        assert code == 0, f"{kind}/{command}: exit {code}, stdout={out!r}"
        lines = out.splitlines()
        assert lines, f"{kind}/{command}: no stdout at all"
        for line in lines:
            assert line.strip() == line and line, f"{kind}/{command}: padding or a blank line in {line!r}"
            payload = json.loads(line)  # raises if any line is not exactly one JSON object
            assert isinstance(payload, dict) and isinstance(payload.get("event"), str), payload

    # A run that fails is one error line and exit 1, whatever the driver.
    for kind, module in (("postgres", psycopg), ("mysql", pymysql)):
        def refuse(**kwargs):
            raise OSError("connection refused")

        with fake_dbapi(module, refuse):
            code, out, err = run_engine("test", DB_KIND=kind, DB_HOST="db.invalid")
        assert code == 1, f"{kind}: exit {code}, stdout={out!r}"
        lines = out.splitlines()
        assert len(lines) == 1, lines
        event = json.loads(lines[0])
        assert event["event"] == "error", event
        assert "connection refused" in event["message"], event
        assert "Traceback" in err and "Traceback" not in out, (err, out)


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
