"""Write one SELECT's rows into a table from Trino itself.

This is the opposite trade to exporter/source.py. QueryStream pulls pages back
to this process so a writer can serialise them; a CTAS never does -- the
coordinator runs the SELECT and commits the result, and this process only
watches. That is the whole point: a billion-row aggregation lands beside the
data instead of crossing the network twice.

Nothing here touches fetch(). The statements it builds are DDL/DML, which
QueryStream explicitly refuses (a result set is what it is for), and `execute`
already blocks until the statement reaches FINISHED -- there is no page to
walk. What is left to report is what the coordinator says about itself:

  * `connection.cursor(stats_callback=fn)` is where the client takes the
    callback -- NOT `connect()` and not `Connection.__init__`, both of which
    reject the keyword outright in trino 0.339.0. The callback then receives a
    deep copy of the stats dict on every coordinator update. `writtenRows` is
    the truth when the connector reports it (a CTAS does), `processedRows` is
    the fallback; `state` names where the query is. No callback means no
    progress, so every statement here gets a fresh cursor carrying one.
  * `cursor.rowcount` is the Trino `update_count` for CTAS/INSERT/UPDATE/
    DELETE/MERGE after execute returns, or -1 when the coordinator never
    reported one. -1 is passed through as -1: a fabricated 0 would read as
    "wrote nothing" and a fabricated guess as a real count.
  * `cursor.cancel()` asks the coordinator to stop. A cancelled statement may
    surface as trino.exceptions.TrinoUserError whose message says so, or as a
    cursor that simply returns -- both are ordinary outcomes here, not errors.

This module builds no protocol: the caller owns stdout. `on_stats` sees every
coordinator update verbatim, and `on_progress(rows, state)` sees only what
passes the throttle, so the engine decides what a `progress` event looks like
and this module stays testable without capturing stdout.

Failure honesty is the reason TableExportError exists. In `replace` mode the
DROP TABLE IF EXISTS runs first, so by the time the CREATE is submitted the old
table is already gone. If that CREATE then fails, the caller must hear both
things: the error, and that the old table was dropped on the way. Warnings
therefore ride on the exception, not only on the result:

    try:
        result = export_to_table(...)
    except TableExportError as exc:
        emit("error", message=str(exc), warnings=exc.warnings)

The engine (app/engine/queryhive_engine.py, `to_table`) is the caller that
does exactly this.
"""

from __future__ import annotations

import logging
import time
from dataclasses import dataclass, field
from typing import Any, Callable

from trino.exceptions import TrinoUserError

from .drivers import DRIVERS, Driver, driver_of
from .source import TrinoConfig, describe_error

log = logging.getLogger(__name__)

# Rows between progress events, whatever the time throttle says: a long CTAS
# that the coordinator reports in fast ticks must still say something.
PROGRESS_EVERY = 1_000

# Recorded when a cancel reached the coordinator. The old table may or may not
# have been dropped and the new one may or may not exist: the caller cannot
# tell, so it is told so explicitly instead of being handed a clean success.
CANCEL_WARNING = "stopped before the statement finished"

_WRITE_MODES = ("create", "replace", "append")


class TableExportError(Exception):
    """A write that failed, carrying whatever the caller must still hear.

    `warnings` holds what happened before the failure -- most importantly the
    DROP of a `replace`, which reporting the error cannot undo.
    """

    def __init__(self, message: str, warnings: list[str] | None = None):
        super().__init__(message)
        self.warnings: list[str] = list(warnings or [])


@dataclass
class TableResult:
    """What the coordinator said about the write it performed."""

    table: str                       # "catalog.schema.name", as written
    rows: int = -1                   # cursor.rowcount, or -1 when unreported
    query_id: str | None = None
    mode: str = "create"             # "create" | "replace" | "append"
    warnings: list[str] = field(default_factory=list)


def quote_identifier(name: str) -> str:
    """One Trino-quoted SQL identifier, with any embedded `"` doubled.

    Kept as the Trino spelling of `Driver.quote` for callers that predate the
    drivers module; every statement built here now asks the config's own driver
    instead, so `schemas`, `tables` and `to_table` still cannot disagree about
    how a name is quoted -- postgres doubles `"`, mysql doubles a backtick.
    """
    return DRIVERS["trino"].quote(name)


def table_reference(catalog: str, schema: str, table: str, driver: Driver | None = None) -> str:
    """The target as the driver writes it: `catalog.schema.table`, unquoted.

    The parts the driver has no level for are dropped, which is the same three
    settings meaning what the driver can actually use -- postgres reports
    `schema.table`, mysql `database.table`.
    """
    return (driver or DRIVERS["trino"]).reference(catalog, schema, table)


def _sql_body(sql: str) -> str:
    """The SELECT body: surrounding whitespace and one trailing `;` removed.

    Exactly QueryStream's normalisation (`sql.strip().rstrip(";")`), so a
    statement pasted with a semicolon behaves the same whether it is exported
    or handed to CTAS.
    """
    return str(sql).strip().rstrip(";")


def build_statements(mode, catalog, schema, table, sql, driver: Driver | None = None) -> list[str]:
    """The statements to run, in order, for one write mode.

    The target is quoted part by part by the driver -- a DROP must never be able
    to hit a different table than the CREATE after it because one name happened
    to need quotes and the other did not. All three drivers support all three
    modes.
    """
    driver = driver or DRIVERS["trino"]
    target = driver.qualified(catalog, schema, table)
    body = _sql_body(sql)
    if mode == "create":
        return [driver.create_sql(target, body)]
    if mode == "replace":
        return [driver.drop_sql(target), driver.create_sql(target, body)]
    if mode == "append":
        return [driver.append_sql(target, body)]
    raise ValueError(f"unknown WRITE_MODE {mode!r}; expected one of {', '.join(_WRITE_MODES)}")


def _is_cancellation(exc: BaseException) -> bool:
    """Did the client report this query as cancelled underneath us?

    `trino.exceptions.TrinoUserError` is also how a real user error (bad SQL,
    denied permission) arrives, so the class alone is not enough -- only the
    client's own wording counts. Lowercased because the client writes both
    "Query has been cancelled" and "USER_CANCELED".

    The message is read through `describe_error`: the client raises its
    cancellation error in a shape that raises on `str()`, and a detection
    routine that throws is worse than one that returns False.
    """
    if not isinstance(exc, TrinoUserError):
        return False
    # `describe_error`, never `str(exc)`: the client's own cancellation error
    # cannot stringify itself (see its docstring), so the naive version turns
    # every cancel into an AttributeError.
    return "cancel" in describe_error(exc).lower()


def _row_count(stats: Any) -> int | None:
    """The progress count the coordinator reported: written rows, else processed.

    `writtenRows` is the one that means "in the table" for a CTAS or INSERT;
    `processedRows` counts everything the source scanned. Neither key being
    present is not zero -- it is "nothing to report yet" -- so no progress event
    is emitted for that tick rather than inventing a number.
    """
    if not isinstance(stats, dict):
        return None
    for key in ("writtenRows", "processedRows"):
        value = stats.get(key)
        if isinstance(value, bool):
            continue  # a JSON bool is not a count
        if isinstance(value, (int, float)):
            return int(value)
    return None


def make_stats_reporter(min_interval_ms, on_stats=None, on_progress=None):
    """A throttled stats reporter, plus the state it tracks.

    At most one update per `min_interval_ms`, and never fewer than one per whole
    1000 rows. `on_stats` sees every coordinator update unchanged; `on_progress`
    sees only the rows and state that passed the throttle. `state["rows"]`
    starts at -1 meaning "nothing reported yet", which is also how the caller
    knows not to fabricate a total.
    """
    state = {"rows": -1, "at": 0.0, "state": None}

    def report(stats):
        if on_stats is not None:
            on_stats(stats)
        rows = _row_count(stats)
        if rows is None:
            return
        now = time.monotonic()
        due = state["rows"] < 0 or (now - state["at"]) * 1000.0 >= min_interval_ms
        if not due and rows - state["rows"] < PROGRESS_EVERY:
            return
        state["rows"], state["at"] = rows, now
        if isinstance(stats, dict):
            state["state"] = stats.get("state")
        if on_progress is not None:
            on_progress(rows, state["state"])

    return report, state


def _final_rows(cursor, state) -> int:
    """`cursor.rowcount` when the coordinator reported one, else the last progress.

    -1 means "Trino did not say"; the last progress value is a real observation
    and is preferred to it. With neither, -1 reaches the caller unchanged.
    """
    count = getattr(cursor, "rowcount", None) if cursor is not None else None
    if isinstance(count, int) and not isinstance(count, bool) and count >= 0:
        return count
    if state["rows"] >= 0:
        return state["rows"]
    return -1


def export_to_table(
    config: TrinoConfig,
    sql: str,
    catalog: str,
    schema: str,
    table: str,
    mode: str,
    on_stats: Callable[[dict], None] | None = None,
    is_cancelled: Callable[[], bool] | None = None,
    on_progress: Callable[[int, str | None], None] | None = None,
    progress_ms: int = 250,
    on_write: Callable[[], None] | None = None,
) -> TableResult:
    """Run the write statements for `mode`; return what the coordinator reported.

    `config` may be a TrinoConfig or a DatabaseConfig for any driver; the driver
    decides how the target is quoted and how many of its three parts survive.

    One connection, one statement at a time, one fresh cursor each, each closed
    in a `finally`. The stats callback is attached to each of those cursors --
    trino 0.339.0 takes it on `Connection.cursor()`, not on `connect()` -- and
    only for trino: psycopg and pymysql take no such keyword, so a driver with
    no progress support simply gets a plain cursor and the reporter never fires.
    `on_write` fires once the connection is open and the first statement is
    about to run; cancellation is checked inside the stats callback and asks the
    coordinator to stop exactly once, so a driver with no stats has no cancel
    path either -- a blocking `execute` cannot be interrupted from here anyway.
    A cancelled run returns normally with a warning naming what is now uncertain
    -- the caller decides what that means -- while a real failure raises
    TableExportError with the same warnings.
    """
    driver = driver_of(config)
    statements = build_statements(mode, catalog, schema, table, sql, driver=driver)
    result = TableResult(table=table_reference(catalog, schema, table, driver), mode=mode)
    # The DROP of a `replace` runs first and cannot be undone; remember it the
    # moment it succeeds so no later failure can hide it from the caller.
    drop_index = 0 if mode == "replace" else None
    reporter, state = make_stats_reporter(
        progress_ms, on_stats=on_stats, on_progress=on_progress
    )
    last_cursor = None
    cancel_sent = False

    def on_stats_tick(stats):
        """The client's callback: report progress, then ask to stop exactly once.

        `cancel_sent` is per run, not per tick: a stats callback that fires
        again after the cancel -- which it will, the coordinator keeps
        answering -- must not DELETE the query a second time. The cursor comes
        from the closed-over loop variable, because one connection here runs
        several statements in turn and only the in-flight one may be cancelled.
        """
        nonlocal cancel_sent
        reporter(stats)
        if cancel_sent or is_cancelled is None:
            return
        try:
            if is_cancelled():
                cancel_sent = True
                if last_cursor is not None:
                    cursor = last_cursor
                    cursor.cancel()
        except Exception:  # a cancel that fails must not break the run
            log.debug("cancel failed", exc_info=True)

    def finish(cursor, cancelled: bool) -> TableResult:
        """Settle the counts and, once, the warning that says what is uncertain."""
        result.rows = _final_rows(cursor, state)
        result.query_id = getattr(cursor, "query_id", None)
        if cancelled and CANCEL_WARNING not in result.warnings:
            result.warnings.append(CANCEL_WARNING)
        return result

    # No keyword arguments for connect(): in the bundled client (trino 0.339.0)
    # `stats_callback` belongs to Connection.cursor(), and dbapi.connect()
    # rejects it with a TypeError. The callback is attached per statement below,
    # by the driver, and only for a driver that has progress stats at all.
    conn = driver.connect(config)
    try:
        for index, statement in enumerate(statements):
            cursor = driver.cursor(conn, on_stats_tick)
            last_cursor = cursor
            if index == 0 and on_write is not None:
                # The connection is open and this statement is next: exactly what
                # a `write` step promises, and never reached if connect failed.
                on_write()
            try:
                cursor.execute(statement)
            except Exception as exc:
                # Warnings already earned (the DROP of a `replace`) travel with
                # the failure, because nothing the caller does can undo them.
                warnings = list(result.warnings)
                if _is_cancellation(exc):
                    # Cancelled underneath the client: an ordinary outcome, not
                    # this module's error. The caller decides what it means.
                    warnings.append(CANCEL_WARNING)
                    result.warnings = warnings
                    return finish(cursor, cancelled=True)
                raise TableExportError(f"{type(exc).__name__}: {describe_error(exc)}", warnings) from exc
            finally:
                try:
                    cursor.close()
                except Exception:  # closing must never mask the real error
                    log.debug("ignored error while closing cursor", exc_info=True)

            if drop_index is not None and index == drop_index:
                # The old table is gone from here on, whatever happens next.
                result.warnings.append(f"dropped the existing table {result.table}")
            if cancel_sent and index == len(statements) - 1:
                # The statement returned although we asked it to stop, so what it
                # wrote is not something this process can vouch for.
                return finish(last_cursor, cancelled=True)

        return finish(last_cursor, cancelled=cancel_sent)
    finally:
        try:
            conn.close()
        except Exception:
            log.debug("ignored error while closing connection", exc_info=True)
