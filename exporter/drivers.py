"""One connection config, three database drivers: Trino, PostgreSQL, MySQL.

Everything above this module -- QueryStream, to_table, the engine's commands --
works against a single object that knows how to `.connect()`, and one `Driver`
that knows how a name is quoted, how many name parts a statement may carry, and
which browse statement answers a given level of the object tree. Nothing here
imports a client package at module scope: trino, psycopg and pymysql are pulled
in inside `connect()` only, so a checkout (or an app bundle) that has one of the
three can still import this module and use the others.

The three differ in ways the rest of the code must not guess at:

    driver    DBAPI          port   quote        object tree
    trino     trino.dbapi    8080   "x" ""        catalog -> schema -> table
    postgres  psycopg (3.x)  5432   "x" ""        schema -> table
    mysql     pymysql        3306   `x` ``        database -> table

Postgres cannot query across databases (the database is fixed by the
connection, so its catalog level does not exist), while MySQL has no schema
level and *can* query across databases on one connection. `Driver.slots` says
which of (database, schema, table) a driver actually consumes, which is what
`qualified()` drops the others for -- the same three TARGET_* settings mean
different things per driver and a driver simply ignores the part it has no
level for.

Settings come from the environment only, and every DB_* name has a TRINO_*
alias from before this module existed. DB_* wins when both are set; a blank
value means "unset" exactly as it always did.
"""

from __future__ import annotations

import logging
from dataclasses import dataclass, field
from typing import ClassVar
from urllib.parse import parse_qs, unquote, urlparse

log = logging.getLogger(__name__)

KINDS = ("trino", "postgres", "mysql")

# The DB_* name, then the older TRINO_* name it supersedes. `setting_label`
# names both in an error message, because either one may be the setting the
# caller actually used.
ALIASES = {
    "DB_URL": "TRINO_URL",
    "DB_HOST": "TRINO_HOST",
    "DB_PORT": "TRINO_PORT",
    "DB_USER": "TRINO_USER",
    "DB_PASSWORD": "TRINO_PASSWORD",
    "DB_DATABASE": "TRINO_CATALOG",
    "DB_SCHEMA": "TRINO_SCHEMA",
    "DB_INSECURE": "TRINO_INSECURE",
}

# The three TARGET_* settings, one per name slot. Which of them a driver needs
# is `Driver.slots`; a slot whose setting the driver has no level for is
# ignored rather than required.
TARGET_KEYS = {
    "database": "TARGET_CATALOG",
    "schema": "TARGET_SCHEMA",
    "table": "TARGET_TABLE",
}

# The object-tree level each browse command lists, for the "driver has no such
# level" message.
BROWSE_LEVELS = {"catalogs": "catalog", "schemas": "schema", "tables": "table"}

_FALSE = frozenset({"0", "false", "no", "off"})
_TRUE = frozenset({"1", "true", "yes", "on"})


def setting_names(key: str) -> tuple[str, ...]:
    """`key` first, then its TRINO_* alias when it has one."""
    alias = ALIASES.get(key)
    return (key, alias) if alias else (key,)


def setting_label(key: str) -> str:
    """Every spelling of one setting, for a message a user has to act on."""
    names = setting_names(key)
    return f"{names[0]} (or {names[1]})" if len(names) > 1 else names[0]


def _raw(env, key, default=""):
    """The value verbatim: a padding space may be part of a name or a password."""
    value = env.get(key)
    return default if value is None or value == "" else value


def _value(env, key, default=""):
    """The value stripped, with blank meaning "unset" (the app sends blanks)."""
    value = _raw(env, key, "").strip()
    return default if not value else value


def _pick(env, key):
    """(the name that supplied this setting, its value); DB_* wins over the alias.

    The name comes back so a validation error can quote the variable the caller
    actually set instead of one they never used.
    """
    for name in setting_names(key):
        value = _value(env, name)
        if value:
            return name, value
    return setting_names(key)[0], ""


def _flag(raw, default=False):
    """1/true/yes/on and 0/false/no/off, case-insensitive; anything else keeps
    `default`, so only the documented spellings change what the caller asked."""
    low = (raw or "").strip().lower()
    if low in _FALSE:
        return False
    if low in _TRUE:
        return True
    return default


def _literal(value) -> str:
    """One single-quoted SQL string literal, with any embedded `'` doubled."""
    return "'" + str(value).replace("'", "''") + "'"


# --------------------------------------------------------------------------- #
# drivers
# --------------------------------------------------------------------------- #

class Driver:
    """One database kind: its name, its shape, and the SQL it speaks.

    `slots` are the name slots the driver consumes, in order, out of
    (database, schema, table) -- so `qualified()` can drop the parts a driver
    has no level for. `levels` is the same tree as the user sees it, which is
    what `db_drivers` reports: it names the mysql middle level "database" where
    trino's is "catalog", even though both arrive in the `database` slot.
    """

    kind: ClassVar[str] = ""
    label: ClassVar[str] = ""
    default_port: ClassVar[int] = 0
    levels: ClassVar[tuple[str, ...]] = ()
    slots: ClassVar[tuple[str, ...]] = ("database", "schema", "table")
    browse: ClassVar[tuple[str, ...]] = ()
    # Why a browse command this driver cannot answer is missing a level, said
    # in the driver's own terms rather than as "unsupported".
    missing_level: ClassVar[dict[str, str]] = {}
    quote_char: ClassVar[str] = '"'
    # trino's client carries progress stats on the cursor; psycopg and pymysql
    # have none, and handing one a `stats_callback` keyword is a TypeError.
    supports_stats: ClassVar[bool] = False

    # -- names ------------------------------------------------------------- #

    def quote(self, name) -> str:
        """One quoted SQL identifier, with any embedded quote character doubled.

        `"x"` with `""` for trino and postgres, `` `x` `` with ``` `` ``` for
        mysql. The only implementation of identifier quoting: to_table's
        `quote_identifier` and every browse statement route through here.
        """
        text = str(name)
        escaped = text.replace(self.quote_char, self.quote_char * 2)
        return self.quote_char + escaped + self.quote_char

    def _by_slot(self, database, schema, table) -> dict:
        return {"database": database, "schema": schema, "table": table}

    def qualified(self, database="", schema="", table="") -> str:
        """The target as this driver writes it: only the levels it has, quoted.

        trino `"c"."s"."t"`, postgres `"s"."t"` (the database is the connection),
        mysql `` `db`.`t` `` (there is no schema level).
        """
        values = self._by_slot(database, schema, table)
        return ".".join(
            self.quote(values[slot]) for slot in self.slots if values[slot] not in (None, "")
        )

    def reference(self, database="", schema="", table="") -> str:
        """The same target written plainly, for a `done` event or a warning."""
        values = self._by_slot(database, schema, table)
        return ".".join(
            str(values[slot]) for slot in self.slots if values[slot] not in (None, "")
        )

    def required_targets(self) -> tuple[str, ...]:
        """The TARGET_* settings a write genuinely needs from this driver."""
        return tuple(TARGET_KEYS[slot] for slot in self.slots)

    # -- connection -------------------------------------------------------- #

    def connect(self, config, stats_callback=None):
        raise NotImplementedError

    def cursor(self, conn, stats_callback=None):
        """A cursor for `conn`, carrying the stats callback only where it exists.

        The keyword is passed for trino alone. psycopg and pymysql cursors take
        no such argument, so asking them for one would be a TypeError on the
        first statement of a write.
        """
        if self.supports_stats and stats_callback is not None:
            return conn.cursor(stats_callback=stats_callback)
        return conn.cursor()

    # -- browse ------------------------------------------------------------ #

    def _no_level(self, command: str) -> str:
        level = BROWSE_LEVELS[command]
        hint = self.missing_level.get(command)
        message = f"{self.kind} has no {level} level"
        return f"{message}; {hint}" if hint else message

    def catalogs_sql(self, database="", schema="", include_system=False) -> str:
        raise ValueError(self._no_level("catalogs"))

    def schemas_sql(self, database="", schema="", include_system=False) -> str:
        raise ValueError(self._no_level("schemas"))

    def tables_sql(self, database="", schema="") -> str:
        raise ValueError(self._no_level("tables"))

    def probe_sql(self, database="", schema="") -> str:
        """The statement `test` runs: the top level this driver lists at all.

        Never requires a setting, so `test` can report "the connection works"
        without a catalog or schema being configured first.
        """
        for command in ("catalogs", "schemas"):
            if command in self.browse:
                return getattr(self, f"{command}_sql")(database, schema)
        return "SELECT 1"

    # -- DDL/DML ----------------------------------------------------------- #

    def create_sql(self, target: str, body: str) -> str:
        return f"CREATE TABLE {target} AS {body}"

    def drop_sql(self, target: str) -> str:
        return f"DROP TABLE IF EXISTS {target}"

    def append_sql(self, target: str, body: str) -> str:
        return f"INSERT INTO {target} {body}"


class TrinoDriver(Driver):
    kind = "trino"
    label = "Trino"
    default_port = 8080
    levels = ("catalog", "schema", "table")
    slots = ("database", "schema", "table")
    browse = ("catalogs", "schemas", "tables")
    quote_char = '"'
    supports_stats = True

    def connect(self, config, stats_callback=None):
        """trino.dbapi.connect, from either config shape.

        `stats_callback` is accepted and ignored: in trino 0.339.0 it belongs to
        `Connection.cursor()`, and `Connection.__init__` rejects the keyword, so
        it is attached per cursor by `Driver.cursor` below.
        """
        import trino

        host = getattr(config, "host", "") or ""
        if not host:
            raise ValueError("no Trino host configured")
        scheme = getattr(config, "scheme", "") or getattr(config, "http_scheme", "") or "http"
        port = getattr(config, "port", 0) or (443 if scheme == "https" else self.default_port)
        user = getattr(config, "user", "") or ""
        password = getattr(config, "password", None)
        auth = trino.auth.BasicAuthentication(user, password) if password else None
        # DatabaseConfig.database and TrinoConfig.catalog are the same setting.
        database = getattr(config, "database", "") or getattr(config, "catalog", None) or None
        schema = getattr(config, "schema", "") or None
        return trino.dbapi.connect(
            host=host,
            port=port,
            user=user,
            catalog=database,
            schema=schema,
            http_scheme=scheme,
            auth=auth,
            verify=getattr(config, "verify", True),
            source=getattr(config, "source", "queryhive"),
            session_properties=getattr(config, "session_properties", None) or None,
            request_timeout=getattr(config, "request_timeout", 300.0),
            max_attempts=getattr(config, "max_attempts", 5),
            # keeps a long query alive while a slow writer drains the batch
            heartbeat_interval=30.0,
        )

    def schemas_sql(self, database="", schema="", include_system=False):
        # `include_system` is accepted but unused: SHOW SCHEMAS already lists
        # every schema the catalog has, system ones included, so the "show all"
        # switch is a no-op here rather than a broken call.
        if not database:
            raise ValueError(f"{setting_label('DB_DATABASE')} is required to list schemas")
        return f"SHOW SCHEMAS FROM {self.quote(database)}"

    def tables_sql(self, database="", schema=""):
        missing = [
            setting_label(key)
            for key, value in (("DB_DATABASE", database), ("DB_SCHEMA", schema))
            if not value
        ]
        if missing:
            raise ValueError(f"{' and '.join(missing)} required to list tables")
        return f"SHOW TABLES FROM {self.quote(database)}.{self.quote(schema)}"

    def catalogs_sql(self, database="", schema="", include_system=False):
        # `include_system` is accepted but unused: SHOW CATALOGS lists every catalog
        # the coordinator has, and there is no system set to hide.
        return "SHOW CATALOGS"


class PostgresDriver(Driver):
    kind = "postgres"
    label = "PostgreSQL"
    default_port = 5432
    levels = ("schema", "table")
    slots = ("schema", "table")
    browse = ("schemas", "tables")
    missing_level = {"catalogs": "the database is set on the connection"}
    quote_char = '"'

    _SSLMODES = ("disable", "prefer", "require", "verify-ca", "verify-full")

    def connect(self, config, stats_callback=None):
        """psycopg (3.x) connect.

        The database is fixed by the connection, and `DB_SCHEMA` is the
        connection's search_path. `request_timeout` bounds how long the
        connection may take to open; it is not a statement timeout, because a
        CTAS here is expected to run as long as it needs to.
        """
        import psycopg

        host = getattr(config, "host", "") or ""
        if not host:
            raise ValueError("no postgres host configured")
        sslmode = (getattr(config, "sslmode", "") or "").strip().lower()
        if sslmode and sslmode not in self._SSLMODES:
            raise ValueError(
                f"DB_SSLMODE for postgres must be one of {', '.join(self._SSLMODES)}, "
                f"got {sslmode!r}"
            )
        if not getattr(config, "verify", True) and sslmode in ("verify-ca", "verify-full"):
            # DB_INSECURE=1 means "do not verify the certificate": ask libpq to
            # encrypt without validating rather than refuse the connection.
            sslmode = "require"
        timeout = float(getattr(config, "request_timeout", 300.0) or 0)
        kwargs = {
            "host": host,
            "port": getattr(config, "port", 0) or self.default_port,
            "user": getattr(config, "user", "") or None,
            "password": getattr(config, "password", None) or None,
            "dbname": getattr(config, "database", "") or None,
        }
        if timeout > 0:
            kwargs["connect_timeout"] = int(timeout)  # libpq wants whole seconds
        if sslmode:
            kwargs["sslmode"] = sslmode
        schema = getattr(config, "schema", "") or ""
        if schema:
            kwargs["options"] = f"-c search_path={schema}"
        return psycopg.connect(**kwargs)

    def schemas_sql(self, database="", schema="", include_system=False):
        # By default the system schemas are hidden: they are not object-tree
        # levels a user browses, and the backslash escapes the underscore so
        # `pg_%` does not also match, say, `pgx`. "Show all schemas" drops the
        # filter, because someone asking for everything wants `pg_catalog` to
        # appear the same as any user schema.
        where = (
            ""
            if include_system
            else "WHERE schema_name NOT LIKE 'pg\\_%' "
                 "AND schema_name <> 'information_schema' "
        )
        return (
            "SELECT schema_name FROM information_schema.schemata " + where + "ORDER BY 1"
        )

    def tables_sql(self, database="", schema=""):
        if not schema:
            raise ValueError(f"{setting_label('DB_SCHEMA')} is required to list tables")
        return (
            "SELECT table_name FROM information_schema.tables "
            f"WHERE table_schema = {_literal(schema)} "
            "AND table_type = 'BASE TABLE' ORDER BY 1"
        )


class MysqlDriver(Driver):
    kind = "mysql"
    label = "MySQL"
    default_port = 3306
    levels = ("database", "table")
    slots = ("database", "table")
    browse = ("catalogs", "tables")
    missing_level = {"schemas": "catalogs lists its databases and tables lists their tables"}
    quote_char = "`"

    _SSLMODES = ("disable", "require")

    def connect(self, config, stats_callback=None):
        """PyMySQL connect.

        MySQL's middle level is the database, so `DB_DATABASE` is the connection
        database and `DB_SCHEMA` has no meaning here at all.
        """
        import pymysql

        host = getattr(config, "host", "") or ""
        if not host:
            raise ValueError("no mysql host configured")
        sslmode = (getattr(config, "sslmode", "") or "").strip().lower()
        if sslmode and sslmode not in self._SSLMODES:
            raise ValueError(
                f"DB_SSLMODE for mysql must be one of {', '.join(self._SSLMODES)}, "
                f"got {sslmode!r}"
            )
        timeout = float(getattr(config, "request_timeout", 300.0) or 0)
        kwargs = {
            "host": host,
            "port": getattr(config, "port", 0) or self.default_port,
            "user": getattr(config, "user", "") or None,
            "password": getattr(config, "password", None) or None,
            "database": getattr(config, "database", "") or None,
            # utf8mb4, not the server default: a data tool may meet any column.
            "charset": "utf8mb4",
        }
        if timeout > 0:
            kwargs["connect_timeout"] = int(timeout)
        if sslmode and sslmode != "disable":
            # A dict turns TLS on with no CA: encrypted, unverified. `DB_INSECURE`
            # only ever loosens this, never turns TLS off.
            kwargs["ssl"] = {"check_hostname": bool(getattr(config, "verify", True))}
        return pymysql.connect(**kwargs)

    # The databases MySQL keeps for itself. They are real databases and a user with
    # privileges can read them, but they are not what someone browsing a tree is
    # looking for, so they are hidden unless asked for.
    _SYSTEM_DATABASES = ("information_schema", "mysql", "performance_schema", "sys")

    def catalogs_sql(self, database="", schema="", include_system=False):
        # MySQL's catalogs are its databases. `information_schema.SCHEMATA` rather
        # than `SHOW DATABASES` because a WHERE clause can filter it and SHOW
        # cannot -- the two list the same thing, and the filtered form is the
        # reason this is not the one-liner it used to be.
        names = ", ".join(_literal(name) for name in self._SYSTEM_DATABASES)
        where = "" if include_system else f"WHERE SCHEMA_NAME NOT IN ({names}) "
        return "SELECT SCHEMA_NAME FROM information_schema.SCHEMATA " + where + "ORDER BY 1"

    def tables_sql(self, database="", schema=""):
        if not database:
            raise ValueError(f"{setting_label('DB_DATABASE')} is required to list tables")
        return f"SHOW TABLES FROM {self.quote(database)}"


DRIVERS: dict[str, Driver] = {
    driver.kind: driver for driver in (TrinoDriver(), PostgresDriver(), MysqlDriver())
}
assert tuple(DRIVERS) == KINDS, "KINDS and DRIVERS have drifted apart"


def driver_of(config) -> Driver:
    """The Driver a config asks for; anything without a kind is Trino.

    `TrinoConfig` carries `kind = "trino"`, but a caller that hands over some
    other object with only `.connect()` still gets the driver it always had.
    """
    kind = getattr(config, "kind", None) or "trino"
    try:
        return DRIVERS[kind]
    except KeyError:
        raise ValueError(
            f"unknown database kind {kind!r}; expected one of {', '.join(KINDS)}"
        ) from None


# --------------------------------------------------------------------------- #
# the config
# --------------------------------------------------------------------------- #

@dataclass
class DatabaseConfig:
    """Where to connect, whatever the database is.

    A drop-in for TrinoConfig wherever only `.connect()` is needed (QueryStream,
    to_table); `kind` picks the driver and `port = 0` means "that driver's
    default". TrinoConfig is untouched and still used by exporter/cli.py and
    exporter/web.py, so `driver_of` treats a config without `kind` as Trino.
    """

    kind: str = "trino"
    host: str = ""
    port: int = 0                 # 0 means "driver default"
    user: str = ""
    password: str | None = None
    database: str = ""            # trino: catalog; postgres: dbname; mysql: database
    schema: str = ""              # trino: schema; postgres: search_path; mysql: unused
    scheme: str = ""              # trino: "http"/"https"
    sslmode: str = ""             # postgres and mysql
    verify: bool = True
    request_timeout: float = 300.0
    max_attempts: int = 5
    session_properties: dict = field(default_factory=dict)

    def __post_init__(self):
        self.kind = (self.kind or "trino").strip().lower()
        if self.kind not in DRIVERS:
            raise ValueError(
                f"unknown DB_KIND {self.kind!r}; expected one of {', '.join(KINDS)}"
            )
        if self.kind == "trino":
            self.scheme = (self.scheme or "http").strip().lower()
            if self.scheme not in ("http", "https"):
                raise ValueError(f"DB_SCHEME must be http or https, got {self.scheme!r}")
            # Same rule as TrinoConfig: Trino refuses BasicAuth over plaintext,
            # so a password implies TLS, as do the standard HTTPS ports.
            if self.scheme != "https" and (self.password or self.port in (443, 8443)):
                self.scheme = "https"
        else:
            self.scheme = (self.scheme or "").strip().lower()
        self.sslmode = (self.sslmode or "").strip().lower()

    def connect(self, stats_callback=None):
        """Open one connection; `stats_callback` is attached where it exists."""
        return DRIVERS[self.kind].connect(self, stats_callback)

    # -- from the environment ---------------------------------------------- #

    @classmethod
    def from_env(cls, env) -> "DatabaseConfig":
        """Every setting from `env`, DB_* first, then its TRINO_* alias.

        `env` is an os.environ-shaped mapping. A URL is optional: the app sends
        only the parts, and those alone must build a working config.
        """
        kind_key, kind = _pick(env, "DB_KIND")
        kind = (kind or "trino").strip().lower()
        if kind not in DRIVERS:
            raise ValueError(
                f"unknown {kind_key} {kind!r}; expected one of {', '.join(KINDS)}"
            )
        _, url = _pick(env, "DB_URL")
        overrides: dict = {"kind": kind}
        for name, key in (
            ("host", "DB_HOST"),
            ("user", "DB_USER"),
            ("password", "DB_PASSWORD"),
            ("database", "DB_DATABASE"),
            ("schema", "DB_SCHEMA"),
            ("scheme", "DB_SCHEME"),
            ("sslmode", "DB_SSLMODE"),
        ):
            _, value = _pick(env, key)
            overrides[name] = value or None
        port_key, port_raw = _pick(env, "DB_PORT")
        port = None
        if port_raw:
            try:
                port = int(port_raw)
            except ValueError:
                raise ValueError(f"{port_key} must be a whole number, got {port_raw!r}")
        overrides["port"] = port
        _, insecure = _pick(env, "DB_INSECURE")
        overrides["verify"] = not _flag(insecure, False)

        if url:
            config = cls.from_url(url, **overrides)
        elif overrides["host"]:
            config = cls(**{k: v for k, v in overrides.items() if v is not None})
        else:
            raise ValueError("need DB_URL or DB_HOST (or TRINO_URL / TRINO_HOST)")
        if not config.user:
            config.user = _value(env, "USER") or ("trino" if kind == "trino" else "")
        if kind == "trino" and config.password and not (port or url):
            config.port = 443  # a password implies https, so the default port moves too
        return config

    # -- from a URL --------------------------------------------------------- #

    _SCHEME_KINDS = {
        "trino": "trino", "http": "trino", "https": "trino",
        "postgres": "postgres", "postgresql": "postgres",
        "mysql": "mysql",
    }

    @classmethod
    def from_url(cls, url: str, **overrides) -> "DatabaseConfig":
        """Build a config from a connection URL; keywords win over its parts.

            https://user:secret@trino.internal:8443/hive/analytics
            postgresql://user:secret@pg.internal/appdb?sslmode=require
            mysql://user:secret@mysql.internal:3306/shop

        The scheme picks the driver unless a `kind` override says otherwise.
        Path segments map to the driver's levels from the top: trino
        catalog/schema, postgres and mysql the database.
        """
        overrides = {k: v for k, v in overrides.items() if v not in (None, "")}
        parsed = urlparse(url if "//" in url else f"//{url}", scheme="http")
        url_scheme = (parsed.scheme or "").lower()
        kind = overrides.get("kind") or cls._SCHEME_KINDS.get(url_scheme) or "trino"
        if kind not in DRIVERS:
            raise ValueError(f"unknown kind {kind!r}; expected one of {', '.join(KINDS)}")
        if not parsed.hostname:
            raise ValueError(f"cannot read a host out of {url!r}")
        parts = [p for p in (parsed.path or "").split("/") if p]
        base = dict(
            kind=kind,
            host=parsed.hostname,
            user=unquote(parsed.username) if parsed.username else "",
            password=unquote(parsed.password) if parsed.password else None,
        )
        if kind == "trino":
            scheme = url_scheme if url_scheme in ("http", "https") else "http"
            base["scheme"] = scheme
            base["port"] = parsed.port or (443 if scheme == "https" else 8080)
            base["database"] = unquote(parts[0]) if parts else ""
            base["schema"] = unquote(parts[1]) if len(parts) > 1 else ""
        else:
            base["port"] = parsed.port or 0
            base["database"] = unquote(parts[0]) if parts else ""
            base["sslmode"] = parse_qs(parsed.query).get("sslmode", [""])[0]
        base.update(overrides)
        return cls(**base)
