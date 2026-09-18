"""Batched, resumable-ish row streaming from a database.

Nothing here is Trino-specific but the retry classes and the plain-HTTP
auto-upgrade below, which is guarded so a Postgres or MySQL config cannot trip
it: a `DatabaseConfig` for any driver is accepted anywhere a `TrinoConfig` is,
and only its `.connect()` is used.

The single biggest cause of "error fetching results" is pulling the whole
result set into memory with fetchall(). This module never does that: it walks
the cursor with fetchmany(batch_size), so the client holds one batch at a time
and the coordinator hands out pages at the pace we consume them.

On top of that:
  * transient fetch failures (502/503/504, read timeouts, network blips) are
    retried with backoff, because the Trino nextUri page is safe to re-request;
  * a heartbeat keeps long-running queries from being reaped while a slow
    writer (xlsx, dbf) is still chewing on the previous batch.
"""

from __future__ import annotations

import json
try:
    import orjson
    JSON_ERRORS = (json.JSONDecodeError, orjson.JSONDecodeError)
except ImportError:
    JSON_ERRORS = (json.JSONDecodeError,)

import logging
import re
import time
from dataclasses import dataclass, field
from typing import Iterator

import trino
from trino import exceptions as trino_exc

from .drivers import DRIVERS, DatabaseConfig, strip_one_trailing_semicolon
from .writers import Column

log = logging.getLogger(__name__)

RETRYABLE = (
    trino_exc.Http502Error,
    trino_exc.Http503Error,
    trino_exc.Http504Error,
    trino_exc.TrinoExternalError,
    trino_exc.TrinoConnectionError,
    OSError,  # covers requests' ConnectionError / socket timeouts
    *JSON_ERRORS,  # covers proxy returning 200 OK with empty body
)

# TrinoExternalError wraps whatever the connector said. A network blip there is
# worth retrying; "permission denied" is not — retrying it just makes the user
# wait through the full backoff (62s at the default 5 retries) for a verdict
# that will never change.
PERMANENT = re.compile(
    r"permission denied|access denied|not authorized|authentication failed|"
    r"does not exist|not found|syntax error|invalid",
    re.I,
)


def describe_error(exc: BaseException) -> str:
    """A string for `exc` that cannot itself raise.

    trino-python-client 0.339.0 raises

        TrinoUserError("Query has been cancelled", self.query_id)

    at client.py:985 -- a bare string where `TrinoQueryError.__init__` expects an
    error *dict*. Its `__str__` is `repr(self)`, which reads `self.error_type`,
    which is `self._error.get("errorType")`. On that exception `_error` is a
    string, so `str(exc)` raises `AttributeError: 'str' object has no attribute
    'get'`.

    That matters far beyond cosmetics: every cancellation and every failed
    statement goes through an error path that has to name the error, and an
    error path that throws while describing the error replaces the real message
    with a nonsense one -- or, in `main()`, skips the `error` event entirely and
    breaks the protocol the app parses. Nothing in here may escape.
    """
    try:
        text = str(exc)
    except Exception:
        text = ""
    if text:
        return text
    # The client passed the message positionally, so it is still in `args` even
    # when the class cannot stringify itself.
    for arg in getattr(exc, "args", ()) or ():
        if isinstance(arg, str) and arg:
            return arg
    return type(exc).__name__


@dataclass
class TrinoConfig:
    """Where to connect. Nothing here is assumed: host, port, scheme and every
    credential come from the caller (CLI flag, env var, or the web form).

    Still the config exporter/cli.py and exporter/web.py build, and unchanged in
    every field they touch. `exporter/drivers.py` owns the connection itself now
    (`DatabaseConfig` is its multi-driver sibling); `kind` is the one addition,
    and it is what `Driver` sees instead of a class check.
    """

    host: str = ""
    port: int = 8080
    user: str = ""
    password: str | None = None
    catalog: str | None = None
    schema: str | None = None
    http_scheme: str = "http"
    verify: bool = True
    source: str = "queryhive"
    session_properties: dict[str, str] = field(default_factory=dict)
    request_timeout: float = 300.0
    max_attempts: int = 5
    kind: str = "trino"

    @classmethod
    def from_url(cls, url: str, **overrides) -> "TrinoConfig":
        """Build a config from a connection URL.

            https://user:secret@trino.internal:8443/hive/analytics

        Scheme, port, credentials, catalog and schema are all optional; the
        path segments map to catalog/schema. Anything passed as a keyword wins.
        """
        from urllib.parse import unquote, urlparse

        parsed = urlparse(url if "//" in url else f"//{url}", scheme="http")
        scheme = parsed.scheme if parsed.scheme in ("http", "https") else "http"
        if not parsed.hostname:
            raise ValueError(f"cannot read a host out of {url!r}")
        parts = [p for p in (parsed.path or "").split("/") if p]
        base = dict(
            host=parsed.hostname,
            port=parsed.port or (443 if scheme == "https" else 8080),
            http_scheme=scheme,
            user=unquote(parsed.username) if parsed.username else "",
            password=unquote(parsed.password) if parsed.password else None,
            catalog=parts[0] if parts else None,
            schema=parts[1] if len(parts) > 1 else None,
        )
        base.update({k: v for k, v in overrides.items() if v not in (None, "")})
        return cls(**base)

    @property
    def url(self) -> str:
        return f"{self.http_scheme}://{self.host}:{self.port}"

    def __post_init__(self):
        # Trino refuses BasicAuth over plaintext, so a password implies TLS.
        # Also auto-upgrade on standard HTTPS ports even without a password.
        if self.http_scheme != "https" and (
            self.password or self.port in (443, 8443)
        ):
            self.http_scheme = "https"

    def connect(self, stats_callback=None):
        """trino.dbapi.connect, built by the Trino driver.

        One implementation for both config shapes: `DatabaseConfig(kind="trino")`
        and this class reach the same keyword arguments. `stats_callback` is
        accepted for signature parity with DatabaseConfig.connect and ignored --
        trino 0.339.0 takes it on `Connection.cursor()`.
        """
        return DRIVERS["trino"].connect(self, stats_callback)


class QueryStream:
    """Context manager yielding (columns, row iterator).

    `config` is anything with a `.connect()`: a TrinoConfig, a DatabaseConfig
    for any of the three drivers, or a caller's own object. Nothing here reads a
    driver-specific field -- only the plain-HTTP auto-upgrade below does, and it
    is guarded so a Postgres or MySQL config cannot trip it.
    """

    def __init__(
        self,
        config: TrinoConfig | DatabaseConfig,
        sql: str,
        batch_size: int = 10_000,
        retries: int = 5,
        retry_backoff: float = 2.0,
    ):
        self.config = config
        # Through the shared rule, not `rstrip(";")`. That one stripped *every* trailing
        # semicolon, so `SELECT 1;;` lost both halves of its terminator, and it silently overrode
        # the careful single-strip the drivers do for EXPLAIN and the count wrap -- two rules for
        # one question, the stricter of them dead. There is now one.
        self.sql = strip_one_trailing_semicolon(sql).strip()
        self.batch_size = max(1, int(batch_size))
        self.retries = max(0, int(retries))
        self.retry_backoff = retry_backoff
        self.columns: list[Column] = []
        self.query_id: str | None = None
        self._conn = None
        self._cursor = None

    def __enter__(self) -> "QueryStream":
        last: Exception | None = None
        for attempt in range(self.retries + 1):
            # Close any stale handles before (re)connecting.
            for h in (self._cursor, self._conn):
                try:
                    if h is not None:
                        h.close()
                except Exception:
                    pass
            self._conn = self.config.connect()
            self._cursor = self._conn.cursor()
            self._cursor.arraysize = self.batch_size
            try:
                self._cursor.execute(self.sql)
                self.query_id = getattr(self._cursor, "query_id", None)
                self._pending: list = []
                if self._cursor.description is None:
                    # columns are only known once the coordinator produced its
                    # first page; pulling one row forces that without losing it
                    first = self._fetch(1)
                    self._pending = list(first)
                if self._cursor.description is None:
                    raise RuntimeError("query returned no result set (DDL/DML statement?)")
                self.columns = [Column(c[0], c[1]) for c in self._cursor.description]
                return self
            except RETRYABLE as exc:
                if PERMANENT.search(str(exc)):
                    raise
                last = exc
                if attempt == self.retries:
                    break
                delay = self.retry_backoff * (2 ** attempt)
                log.warning(
                    "execute failed (%s), retry %d/%d in %.1fs",
                    type(exc).__name__, attempt + 1, self.retries, delay,
                )
                time.sleep(delay)
            except trino_exc.HttpError as exc:
                # Only auto-retry the "plain HTTP to HTTPS port" 400 from nginx.
                # Any other HttpError (bad SQL, auth failure) is a real error.
                if "plain HTTP" not in str(exc) and b"plain HTTP" not in getattr(exc, "args", (b"",))[0:1]:
                    raise
                # Trino-only, and guarded: a psycopg or pymysql config has no
                # scheme to upgrade, and `dataclasses.replace(..., scheme=...)`
                # on one would be a TypeError. TrinoConfig names the field
                # `http_scheme`, DatabaseConfig names it `scheme`.
                field_name = (
                    "http_scheme" if hasattr(self.config, "http_scheme")
                    else "scheme" if hasattr(self.config, "scheme")
                    else None
                )
                if field_name is None:
                    raise  # not a Trino connection: nothing to upgrade, no retry
                if getattr(self.config, field_name) != "https":
                    import dataclasses
                    log.warning(
                        "auto-upgrading scheme to https (nginx 400: plain HTTP sent to HTTPS port)"
                    )
                    self.config = dataclasses.replace(self.config, **{field_name: "https"})
                last = exc
                if attempt == self.retries:
                    break
                delay = self.retry_backoff * (2 ** attempt)
                log.warning(
                    "execute failed (plain-HTTP→HTTPS), retry %d/%d in %.1fs",
                    attempt + 1, self.retries, delay,
                )
                time.sleep(delay)
        raise RuntimeError(
            f"giving up after {self.retries} retries on execute: {last}"
        ) from last

    def __exit__(self, *exc):
        for closeable in (self._cursor, self._conn):
            try:
                if closeable is not None:
                    closeable.close()
            except Exception:  # closing must never mask the real error
                log.debug("ignored error while closing trino handle", exc_info=True)

    def _fetch(self, size: int) -> list:
        last: Exception | None = None
        for attempt in range(self.retries + 1):
            try:
                return self._cursor.fetchmany(size)
            except RETRYABLE as exc:
                if PERMANENT.search(str(exc)):
                    raise
                last = exc
                if attempt == self.retries:
                    break
                delay = self.retry_backoff * (2**attempt)
                log.warning(
                    "fetch failed (%s), retry %d/%d in %.1fs",
                    type(exc).__name__, attempt + 1, self.retries, delay,
                )
                time.sleep(delay)
        raise RuntimeError(
            f"giving up fetching results after {self.retries} retries: {last}"
        ) from last

    def batches(self) -> Iterator[list]:
        """One fetched page at a time. Keeping the page intact lets the caller
        overlap fetching with writing (see export._prefetched)."""
        if self._pending:
            yield self._pending
            self._pending = []
        while True:
            batch = self._fetch(self.batch_size)
            if not batch:
                return
            yield batch

    def rows(self) -> Iterator[list]:
        for batch in self.batches():
            yield from batch

    def cancel(self) -> None:
        try:
            if self._cursor is not None:
                self._cursor.cancel()
        except Exception:
            log.debug("cancel failed", exc_info=True)
