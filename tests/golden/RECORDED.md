# Snapshots recorded against real servers

`index.json` holds two kinds of case. The ones `tools/golden/record.py` writes
drive the engine in-process with a fake cursor, so what they pin is the engine's
own normalisation. The ones this file is about were recorded by
`tools/golden/live_cases.py`, which starts the engine as a child process exactly
as the app does and points it at the containers `deploy/dev/up.sh` starts. Their
case ids end in `_live`.

The difference matters. A fake cursor can hand the engine a `Decimal`, a
`timedelta` or a type name because the case author decided it should; it cannot
decide what psycopg really returns for a `numeric(38,10)`, what PyMySQL really
returns for a `time`, or what Trino really puts on the wire for a
`timestamp(6) with time zone`. Those are facts about the server, and the _live
cases freeze them.

Every case here ran, and every `.ndjson` is that run's stdout after the same
masking `record.py` applies (timings, query ids, temp paths). Nothing was
hand-written. A second `tools/golden/live_cases.py` run reproduced all fourteen
Postgres and MySQL cases and `trino_nation_live` byte for byte; the other five
Trino cases were recorded once, in the one window the container stayed up, and
were then compared against the Rust engine rather than re-recorded. A run that
caught Trino down recorded nothing at all and said so, which is the behaviour a
recorder needs.

## Servers recorded against

| engine | server | how the version was read |
| --- | --- | --- |
| postgres | PostgreSQL 17.11 (aarch64-unknown-linux-musl, Alpine, package 15.2.0) | `podman exec qh-postgres psql -U qh -d qh -tAc 'SELECT version()'` |
| mysql | MySQL Community Server 8.4.11 | `podman exec qh-mysql sh -c 'mysql -uroot -p… -N -e "SELECT VERSION(), @@version_comment"'` |
| trino | Trino 483 | the plan text in `tests/golden/explain/trino_explain_live.ndjson` begins `Trino version: 483` |

Data: the seed from `deploy/dev/seed-postgres.sql` and `seed-mysql.sql`
(`type_zoo`, `wide_500k`, 500,000 rows), plus Trino's built-in `tpch` catalog at
`tpch.tiny`, which needs nothing loaded.

## Coverage, command by engine

Capable means the Python engine implemented the command for that driver when
this was recorded -- read from `exporter/cli.py`, `exporter/drivers.py` and the
`commands` table in `app/engine/queryhive_engine.py`. A dash means the driver
answers with a usage error, so there is nothing to record but the refusal.

**Before** (the twenty-one in-process cases):

| command | trino | postgres | mysql |
| --- | --- | --- | --- |
| `db_drivers` | case (`drivers`) | -- not driver-specific -- | -- not driver-specific -- |
| `test` | case (`test_probe`, `connect_failure`) | capable, uncovered | capable, uncovered |
| `catalogs` | case (`catalogs`) | capable, uncovered | case (`mysql_catalogs`) |
| `schemas` | case (`schemas`) | case (`postgres_schemas`) | **refused** |
| `tables` | case (`tables`) | capable, uncovered | capable, uncovered |
| `objects` | case (`objects_trino`) | case (`postgres_objects`) | case (`mysql_objects`) |
| `export` | case (`export_csv`) | capable, uncovered | capable, uncovered |
| `to_table` | case (`to_table_create`) | capable, uncovered | capable, uncovered |
| `preview` | 5 cases (`type_zoo`, `batching`, `blank_sql`, `limit_truncation`, `no_rows`) | capable, uncovered | capable, uncovered |
| `count` | case (`count`) | capable, uncovered | capable, uncovered |
| `explain` | case (`explain`) | capable, uncovered | capable, uncovered |

**After** (adding the twenty live cases):

| command | trino | postgres | mysql |
| --- | --- | --- | --- |
| `db_drivers` | case | -- | -- |
| `test` | case | capable, **still uncovered** | capable, **still uncovered** |
| `catalogs` | case (in-process only) | **live case** | case + **live case** |
| `schemas` | case (in-process only) | case (in-process only) | **live refusal** |
| `tables` | case (in-process only) | **live case** | **live case** |
| `objects` | case + **live case** | case + **live case** | case + **live case** |
| `export` | case | capable, **still uncovered** | capable, **still uncovered** |
| `to_table` | case | capable, **still uncovered** | capable, **still uncovered** |
| `preview` | 5 cases + **2 live** | **2 live cases** | **2 live cases** |
| `count` | case + **live case** | **live case** | **live case** |
| `explain` | case + **live case** | **live case** | **live case** |

Still missing, in the order worth closing: `export` and `to_table` against
Postgres and MySQL (their writers are engine-side, but the temp paths, the row
counts and the file-size events are all real server facts); `test` against both;
and a live `catalogs`/`schemas`/`tables` for Trino, which today is in-process
only. `db_drivers` needs nothing live: it reads settings, not a server.

## How to re-record

    deploy/dev/up.sh                       # postgres, mysql and trino up
    uv venv --python 3.14 /tmp/qh-golden-venv
    V=/tmp/qh-golden-venv/bin/python
    uv pip install --python $V lz4 python-dateutil pytz 'requests>=2.32.4' \
        tzlocal zstandard 'trino>=0.330' --no-deps
    uv pip install --python $V 'psycopg[binary]' pymysql
    uv pip install --python $V urllib3 certifi idna charset-normalizer six
    $V tools/golden/live_cases.py            # all
    $V tools/golden/live_cases.py --list     # what
    $V tools/golden/live_cases.py <case-id>  # one

Those are the exact install steps that produced these snapshots. `trino` goes in
with `--no-deps` on purpose (see the header of `requirements.txt`: its dependency
list pins `orjson`, which refuses free-threaded Python), so its transitive
dependencies -- `urllib3 certifi idna charset-normalizer six` and the rest --
have to be named explicitly. The engine ships no server-side driver, so
`psycopg[binary]` and `pymysql` are not in `requirements.txt` at all and are
installed here for the first time. `check_tools()` refuses to run and says which
import is missing rather than recording a snapshot of an ImportError.

A case that reports an error is never written: if a container is down the run
exits non-zero and leaves the tree untouched, so a connection failure can never
be frozen as if it were an answer.

## The cases

| case | command | engine | event lines | what it freezes |
| --- | --- | --- | --- | --- |
| `postgres_type_zoo_live` | `preview` | postgres | 4 | The wire shape psycopg hands over for every hard Postgres type, as the engine renders it: a numeric(38,10) with its digits, the scientific spelling psycopg's Decimal gives a tiny negative, a timestamptz the server converted to UTC (the seeded +07:00 is *not* what comes back), a naive timestamp that must not gain a zone, an interval, json and jsonb text in the server's own key order, arrays, bytea with a NUL and 0xFF, a uuid, an enum label, a point, a NULL, an empty string and the four characters NULL. The `columns` type is the type OID psycopg reports. |
| `postgres_batching_live` | `preview` | postgres | 5 | 250 real rows through PREVIEW_BATCH=200: the batch boundary, the second batch, and `truncated: true` from the one extra row the cap costs. |
| `postgres_catalogs_live` | `catalogs` | postgres | 1 | `catalogs` on the driver whose object tree has no catalog level: the pg_database listing, filtered to connectable non-template databases, in the query's own ORDER BY. |
| `postgres_tables_live` | `tables` | postgres | 1 | The information_schema statement Postgres builds for `tables`, against a schema that really holds the seeded tables -- BASE TABLE only, ordered. |
| `postgres_objects_live` | `objects` | postgres | 1 | The only driver whose Name/OID/Owner/ACL shape is real: a real OID, the real owner, and a NULL relacl rendered as an empty string. |
| `postgres_count_live` | `count` | postgres | 3 | `count`'s wrapper run by Postgres itself on 500,000 real rows, so the number is the server's rather than a fake cursor's. |
| `postgres_explain_live` | `explain` | postgres | 4 | Postgres answers EXPLAIN with one `QUERY PLAN` text column; this freezes the real column name, the real plan text and the absence of `truncated`. |
| `mysql_type_zoo_live` | `preview` | mysql | 4 | The same hard types again, as MySQL decodes them: decimal(38,10), datetime and timestamp kept distinct, a time PyMySQL hands back as an object, json re-ordered by the server, a blob with a NUL and 0xFF, an enum label, a char(36) and a binary(16) uuid, a NULL, an empty string and the four characters NULL. The `columns` type is MySQL's column type code. |
| `mysql_batching_live` | `preview` | mysql | 5 | 250 real rows through the 200-row preview batch: the boundary, the second batch and `truncated: true`. |
| `mysql_tables_live` | `tables` | mysql | 1 | MySQL's SHOW TABLES FROM `qh`, quoted with backticks, in the server's own order. |
| `mysql_objects_live` | `objects` | mysql | 1 | Name/Engine/Rows/Comment from information_schema.TABLES: the columns no other driver can answer, with a real engine name, a real (estimated) row count and an empty comment. |
| `mysql_schemas_live` | `schemas` | mysql | 1 | The one browse command MySQL has no level for: a usage error naming the driver, raised before the network is touched. |
| `mysql_count_live` | `count` | mysql | 3 | The count wrapper on MySQL over 500,000 real rows. |
| `mysql_explain_live` | `explain` | mysql | 4 | MySQL is the driver whose EXPLAIN is not one text column: a real multi-column plan table, which is why `explain` emits preview's protocol rather than a plan shape. |
| `trino_nation_live` | `preview` | trino | 4 | The real Trino decoder on tpch.tiny.nation: bigint, varchar, and a NULL comment that must arrive as JSON null rather than "None". |
| `trino_type_zoo_live` | `preview` | trino | 4 | Trino's own decoding of the types that matter, from expressions rather than a table: a decimal(38,10) with its digits intact, a tiny negative decimal, a timestamp with a zone, a naive timestamp, a date, a time, an interval, a NULL and an empty string. |
| `trino_batching_live` | `preview` | trino | 5 | 250 real tpch rows -- a bigint, the tpch connector's `double` totalprice and a date -- cut by the 200-row batch rule, so the boundary, the second batch and `truncated: true` are all frozen. tpch's generated orderkeys are sparse (1..7, then 32..39, ...), so the second batch starts at 801 rather than at 201: the row *number* is what the cap counts, not the key. |
| `trino_objects_live` | `objects` | trino | 1 | Trino's information_schema objects query: Name/Type only, because Trino has no OID, owner or ACL to answer with. |
| `trino_count_live` | `count` | trino | 3 | The count wrapper run by the coordinator over tpch.tiny.orders. |
| `trino_explain_live` | `explain` | trino | 4 | A real Trino plan: one varchar column, one row per plan line, fetched through EXPLAIN with the caller's semicolon stripped. |

`<VENV>` below is the interpreter with the client packages; `tools/golden/
live_cases.py --markdown` prints this section again from the case table, so it
cannot drift from what was run. Every setting is explicit and `RETRIES=0` keeps
the engine from reconnecting under a dead server (`deploy/dev/qh-pg-old.sh` and
`qh-mysql-old.sh` are the variants that run the real servers instead of the
containers).

    # postgres_type_zoo_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=postgres DB_PASSWORD=qh-dev-only DB_PORT=55432 DB_SCHEMA=public DB_SSLMODE=disable DB_USER=qh RETRIES=0 SQL='SELECT * FROM type_zoo' <VENV>/bin/python -s -u app/engine/queryhive_engine.py preview

    # postgres_batching_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=postgres DB_PASSWORD=qh-dev-only DB_PORT=55432 DB_SCHEMA=public DB_SSLMODE=disable DB_USER=qh LIMIT=250 RETRIES=0 SQL='SELECT id, c01 FROM wide_500k ORDER BY id' <VENV>/bin/python -s -u app/engine/queryhive_engine.py preview

    # postgres_catalogs_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=postgres DB_PASSWORD=qh-dev-only DB_PORT=55432 DB_SCHEMA=public DB_SSLMODE=disable DB_USER=qh RETRIES=0 <VENV>/bin/python -s -u app/engine/queryhive_engine.py catalogs

    # postgres_tables_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=postgres DB_PASSWORD=qh-dev-only DB_PORT=55432 DB_SCHEMA=public DB_SSLMODE=disable DB_USER=qh RETRIES=0 <VENV>/bin/python -s -u app/engine/queryhive_engine.py tables

    # postgres_objects_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=postgres DB_PASSWORD=qh-dev-only DB_PORT=55432 DB_SCHEMA=public DB_SSLMODE=disable DB_USER=qh RETRIES=0 <VENV>/bin/python -s -u app/engine/queryhive_engine.py objects

    # postgres_count_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=postgres DB_PASSWORD=qh-dev-only DB_PORT=55432 DB_SCHEMA=public DB_SSLMODE=disable DB_USER=qh RETRIES=0 SQL='SELECT * FROM wide_500k' <VENV>/bin/python -s -u app/engine/queryhive_engine.py count

    # postgres_explain_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=postgres DB_PASSWORD=qh-dev-only DB_PORT=55432 DB_SCHEMA=public DB_SSLMODE=disable DB_USER=qh RETRIES=0 SQL='SELECT * FROM type_zoo WHERE id = 1' <VENV>/bin/python -s -u app/engine/queryhive_engine.py explain

    # mysql_type_zoo_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=mysql DB_PASSWORD=qh-dev-only DB_PORT=53306 DB_USER=qh RETRIES=0 SQL='SELECT * FROM type_zoo' <VENV>/bin/python -s -u app/engine/queryhive_engine.py preview

    # mysql_batching_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=mysql DB_PASSWORD=qh-dev-only DB_PORT=53306 DB_USER=qh LIMIT=250 RETRIES=0 SQL='SELECT id, c01 FROM wide_500k ORDER BY id' <VENV>/bin/python -s -u app/engine/queryhive_engine.py preview

    # mysql_tables_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=mysql DB_PASSWORD=qh-dev-only DB_PORT=53306 DB_USER=qh RETRIES=0 <VENV>/bin/python -s -u app/engine/queryhive_engine.py tables

    # mysql_objects_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=mysql DB_PASSWORD=qh-dev-only DB_PORT=53306 DB_USER=qh RETRIES=0 <VENV>/bin/python -s -u app/engine/queryhive_engine.py objects

    # mysql_schemas_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=mysql DB_PASSWORD=qh-dev-only DB_PORT=53306 DB_USER=qh RETRIES=0 <VENV>/bin/python -s -u app/engine/queryhive_engine.py schemas

    # mysql_count_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=mysql DB_PASSWORD=qh-dev-only DB_PORT=53306 DB_USER=qh RETRIES=0 SQL='SELECT * FROM wide_500k' <VENV>/bin/python -s -u app/engine/queryhive_engine.py count

    # mysql_explain_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=mysql DB_PASSWORD=qh-dev-only DB_PORT=53306 DB_USER=qh RETRIES=0 SQL='SELECT * FROM type_zoo WHERE id = 1' <VENV>/bin/python -s -u app/engine/queryhive_engine.py explain

    # trino_nation_live
    DB_DATABASE=tpch DB_HOST=127.0.0.1 DB_KIND=trino DB_PORT=58080 DB_SCHEMA=tiny DB_USER=queryhive RETRIES=0 SQL='SELECT nationkey, name, regionkey, comment FROM tpch.tiny.nation ORDER BY nationkey' <VENV>/bin/python -s -u app/engine/queryhive_engine.py preview

    # trino_type_zoo_live
    DB_DATABASE=tpch DB_HOST=127.0.0.1 DB_KIND=trino DB_PORT=58080 DB_SCHEMA=tiny DB_USER=queryhive RETRIES=0 SQL='SELECT CAST(1234567890123456789012345678.1234567890 AS decimal(38,10)) AS high_precision, CAST(-0.0000000001 AS decimal(38,10)) AS tiny_negative, TIMESTAMP '\''2026-01-31 12:00:00.123456 +07:00'\'' AS tz_aware, TIMESTAMP '\''2026-01-31 12:00:00.123456'\'' AS tz_naive, DATE '\''2026-01-31'\'' AS a_date, TIME '\''23:59:59.999999'\'' AS a_time, INTERVAL '\''3'\'' DAY + INTERVAL '\''4'\'' HOUR + INTERVAL '\''5'\'' MINUTE + INTERVAL '\''6'\'' SECOND AS an_interval, CAST(NULL AS varchar) AS no_value, '\'''\'' AS empty_text' <VENV>/bin/python -s -u app/engine/queryhive_engine.py preview

    # trino_batching_live
    DB_DATABASE=tpch DB_HOST=127.0.0.1 DB_KIND=trino DB_PORT=58080 DB_SCHEMA=tiny DB_USER=queryhive LIMIT=250 RETRIES=0 SQL='SELECT orderkey, totalprice, orderdate FROM tpch.tiny.orders ORDER BY orderkey' <VENV>/bin/python -s -u app/engine/queryhive_engine.py preview

    # trino_objects_live
    DB_DATABASE=tpch DB_HOST=127.0.0.1 DB_KIND=trino DB_PORT=58080 DB_SCHEMA=tiny DB_USER=queryhive RETRIES=0 <VENV>/bin/python -s -u app/engine/queryhive_engine.py objects

    # trino_count_live
    DB_DATABASE=tpch DB_HOST=127.0.0.1 DB_KIND=trino DB_PORT=58080 DB_SCHEMA=tiny DB_USER=queryhive RETRIES=0 SQL='SELECT * FROM tpch.tiny.orders' <VENV>/bin/python -s -u app/engine/queryhive_engine.py count

    # trino_explain_live
    DB_DATABASE=tpch DB_HOST=127.0.0.1 DB_KIND=trino DB_PORT=58080 DB_SCHEMA=tiny DB_USER=queryhive RETRIES=0 SQL='SELECT * FROM tpch.tiny.nation;' <VENV>/bin/python -s -u app/engine/queryhive_engine.py explain

## What the Rust harness needs

Two things for each case, and the second one is easy to forget.

1. The case id in `EXACT` or `ACCEPTED`. Until it is there,
   `a_new_snapshot_cannot_be_ignored` fails.
2. For every id in `EXACT`, a `Case` in `cases()` whose scripted session
   produces those exact bytes. `every_exact_case_matches_the_python_snapshot`
   asserts `checked == EXACT.len()`, so an id in `EXACT` without a `Case` fails
   there instead. The values to script are in the case's `.ndjson`: for the
   Postgres and MySQL cases the `columns` event carries the type code the
   Python engine emits (`"23"`, `"1700"`, `"3"`, `"246"`, ...) and the `rows`
   event carries the decoded values one by one.

### Reproduced today -- put these in `EXACT`

Ten cases came back byte-identical between the Rust binary and the recorded
Python output, apart from the two fields the harness masks:

    "postgres_tables_live",
    "postgres_objects_live",
    "postgres_count_live",
    "mysql_tables_live",
    "mysql_objects_live",
    "mysql_count_live",
    "trino_nation_live",
    "trino_batching_live",
    "trino_objects_live",
    "trino_count_live",

### Not reproduced today

Ten. For each of these the Rust engine and the live Python engine differ. The
ones marked **fix first** are bugs to repair before the case can be `EXACT`; the
rest are the same kind of deliberate difference `docs/golden-deltas.md` already
documents.

    "postgres_catalogs_live",     // fix first: the Rust engine refuses the command
    "postgres_type_zoo_live",     // type codes + interval, arrays, uuid, tiny decimal
    "postgres_batching_live",     // only the columns type spelling differs
    "postgres_explain_live",      // only the columns type spelling differs
    "mysql_type_zoo_live",        // type codes + tiny decimal and a quoted time
    "mysql_batching_live",        // only the columns type spelling differs
    "mysql_explain_live",         // only the columns type spelling differs
    "mysql_schemas_live",         // the message loses its hint
    "trino_type_zoo_live",        // fix first: no Client-Capabilities header
    "trino_explain_live",         // fix first: the trailing ';' is not stripped

Four of them (`postgres_batching_live`, `postgres_explain_live`,
`mysql_batching_live`, `mysql_explain_live`) differ from Python *only* in the
`columns` event's `type` spelling -- the driver's type code (`"23"`, `"8"`)
against a type name (`"int4"`, `"bigint"`). That is a protocol decision rather
than a value-decoding bug: the fake session declares both strings itself, so
which list they belong in is a choice about what the protocol promises. Every
other byte of those four -- 250 rows, the batch boundary, the plan table the
server sent -- the Rust engine reproduces exactly, so if the promise is the type
name they move to `EXACT` as soon as the `Case` scripts the name, and if the
promise is the code they stay `ACCEPTED` and the mapping runs the other way.

### Expected failure until then

`cargo test -p qh-ffi --test golden` fails with one panic, naming whichever
unclassified case it reaches first:

    ---- a_new_snapshot_cannot_be_ignored stdout ----
    thread 'a_new_snapshot_cannot_be_ignored' (445018) panicked at crates/qh-ffi/tests/golden.rs:1021:13:
    postgres_catalogs_live is in the snapshot but is neither reproduced nor listed as an accepted difference
    note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

    test result: FAILED. 4 passed; 1 failed; 0 ignored; 0 measured

`every_exact_case_matches_the_python_snapshot` passes today and will keep
passing: it counts only the ids in `EXACT`, and the existing twenty-one cases are
untouched. The other three tests in that file pass.

## Divergences against real servers

Found by running `target/debug/queryhive-engine` and the Python engine with the
same settings on the same server and diffing the events (masks applied as
usual). This is the part of the run that the fake-cursor cases structurally
cannot produce: they decide the value, the decoder never runs.

Every case in this section was re-run against `target/debug/queryhive-engine`
built at 23:56 on 22 September 2026 -- the working tree as another agent had
left it by then, which is why the Trino findings below have to be read together
with the note about `crates/qh-driver-trino/tests/integration.rs`. The
`EXACT`/`ACCEPTED` split above is that build's, not an older one's.

### Trino: the driver does not advertise `PARAMETRIC_DATETIME`, so the server downgrades every value

One cause, four observed differences. `trino_type_zoo_live` asks Trino 483 for
`TIMESTAMP '2026-01-31 12:00:00.123456 +07:00'`, `TIMESTAMP '…123456'`,
`TIME '23:59:59.999999'` and a few more literals:

| event | python | rust |
| --- | --- | --- |
| `columns` | `timestamp(6) with time zone`, `timestamp(6)`, `time(6)` | `timestamp with time zone`, `timestamp`, `time` |
| row 3, `tz_aware` | `2026-01-31 12:00:00.123456+07:00` | `2026-01-31 12:00:00.123000+07:00` |
| row 4, `tz_naive` | `2026-01-31 12:00:00.123456` | `2026-01-31 12:00:00.123000` |
| row 6, `a_time` | `23:59:59.999999` | `00:00:00` |

The protocol is not truncating anything. The server sends different bytes to a
client that has not asked for parametric datetimes. Same server, same query, two
raw requests that differ only in one header:

    without X-Trino-Client-Capabilities:
      ([('tz_aware', 'timestamp with time zone'), ('a_time', 'time')],
       [['2026-01-31 12:00:00.123 +07:00', '00:00:00.000']])

    with X-Trino-Client-Capabilities: NUMBER,PARAMETRIC_DATETIME,SESSION_AUTHORIZATION
      ([('tz_aware', 'timestamp(6) with time zone'), ('a_time', 'time(6)')],
       [['2026-01-31 12:00:00.123456 +07:00', '23:59:59.999999']])

`23:59:59.999999` becoming `00:00:00.000` is the same rounding: 999.999 ms
rounds up into the next second, where 123.456 ms rounds down to 123. So the
renderer is innocent -- it faithfully shows what the server sent -- and the
driver never asks for the better encoding:

    $ grep -rniE '"x-trino[^"]*"|capabilit' crates/qh-driver-trino/src/*.rs
    X-Trino-User, X-Trino-Catalog, X-Trino-Schema ... and no Client-Capabilities

The Python client sends `NUMBER,PARAMETRIC_DATETIME,SESSION_AUTHORIZATION`
(`trino/client.py` 0.339). Adding that header to the Rust driver is a one-line
fix that should close all three rows of the table above at once.

Worth flagging to whoever owns `crates/qh-driver-trino` before this case is
listed: `crates/qh-driver-trino/tests/integration.rs:173` has
`the_protocol_truncates_timestamps_to_milliseconds_and_this_pins_that`, whose
own comment says "milliseconds only: the .123456 the server holds arrives as
.123". The server does not hold `.123` -- it holds `.123456` and hands the
downgrade to clients that do not declare the capability. That test pins the
symptom as if it were the contract, and if it survives, this case has to be
`ACCEPTED` for a reason that is not true. It needs to be settled before the id
is listed either way.

The interval text in the same case (`"3 days, 4:05:06"` against
`3 04:05:06.000`) is a separate difference and the same shape as the delta
`docs/golden-deltas.md` already describes for the in-process case, except that
the body differs too, not only the quotes.

### Trino: `explain` does not strip the caller's semicolon

`trino_explain_live` was recorded with `SQL='SELECT * FROM tpch.tiny.nation;'`
-- the terminator is deliberate, because the Python engine strips it. The Rust
engine sent it through:

    python: {"columns": [{"name": "Query Plan", "type": "varchar(721)"}], "event": "columns"}
    rust:   {"event": "error", "message": "SYNTAX_ERROR: line 1:39: mismatched input
             ';'. Expecting: ',', '.', 'AS', 'CROSS', 'EXCEPT', 'FETCH', 'FOR',
             'FULL', 'GROUP', 'HAVING', 'INNER', 'INTERSECT', 'JOIN', 'LEFT',
             'LIMIT', 'MATCH_RECOGNIZE', 'NATURAL', 'OFFSET', 'ORDER', 'PIVOT',
             'RIGHT', 'TABLESAMPLE', 'UNION', 'WHERE', 'WINDOW', <EOF>,
             <identifier>"}

The live run produced two events on the Rust side against four on the Python
side (columns, the plan row, done), so this case cannot be listed at all until
the terminator is handled.

The in-process `explain` case cannot see this: its fake session implements
`explain_statement` by calling `qh_sql::strip_terminator` itself, so the
stripping the real session is supposed to do is done by the test double.

### Postgres: `catalogs` is refused

`postgres_catalogs_live`, recorded from a server that answers
`pg_database` with two databases:

    python: {"event": "catalogs", "names": ["postgres", "qh"]}                      (exit 0)
    rust:   {"event": "error", "message": "postgres has no catalog level"}           (exit 1)

The Python engine's Postgres browse list includes the catalog level -- that is
how the case has an event at all -- while the Rust engine's does not, so the
app's catalog picker loses both databases.

### Postgres: interval, arrays and uuid are decoded differently

`postgres_type_zoo_live`, `rows` event:

| column | python | rust |
| --- | --- | --- |
| `an_interval` | `"428 days, 4:05:06"` | `14 months, 3 days, 4:05:06` |
| `ints` / `texts` / `nested` | `[1, null, 3]` / `["a", null, "c"]` / `[[1, 2], [3, null]]` | `{1,NULL,3}` / `{a,NULL,c}` / `{{1,2},{3,NULL}}` |
| `a_uuid` | `"550e8400-e29b-41d4-a716-446655440000"` (quotes included) | `550e8400-e29b-41d4-a716-446655440000` |

The interval is not just spelled differently: psycopg folds the seeded
`1 year 2 mons 3 days 04:05:06` into a `timedelta` of 428 days, while the Rust
side keeps `months: 14, days: 3`. Whichever side is right, this case is the one
that pins it.

### Postgres and MySQL: `columns.type` is a type code, not a type name

    postgres  python: {"name": "id", "type": "23"}        rust: {"name": "id", "type": "int4"}
    postgres  python: {"name": "a_time", "type": "1083"}  rust: {"name": "a_time", "type": "time"}
    mysql     python: {"name": "id", "type": "3"}         rust: {"name": "id", "type": "int"}
    mysql     python: {"name": "a_enum", "type": "254"}   rust: {"name": "a_enum", "type": "char"}
    mysql     python: {"name": "filtered", "type": "5"}   rust: {"name": "filtered", "type": "double"}

The Python engine emits whatever the DBAPI description gives, which for psycopg
and PyMySQL is the numeric type code; the Rust engine maps the code to a name.
Two of those mappings are worth a second look before the four
type-spelling-only cases are listed: MySQL's `254` is both `CHAR` and `ENUM`,
and the Rust side calls it `char` where the Python side leaves the code as `254`
-- so neither spelling says `enum`. This is a protocol decision either way, not
a decoding bug; the `EXACT` section above says what it implies for the harness.

### MySQL: a quoted time and the tiny decimal

`mysql_type_zoo_live`, `rows` event, the columns that differ after the type
spelling is set aside:

| column | python | rust |
| --- | --- | --- |
| `tiny_negative` | `-1E-10` | `-0.0000000001` |
| `a_time` | `"23:59:59.999999"` | `23:59:59.999999` |

The first is the decimal spelling `docs/golden-deltas.md` already describes (the
Python side is `str(Decimal)`, which goes scientific for a small exponent). The
second is a difference in kind: PyMySQL hands the engine a `datetime.time`
object, which the Python `to_text` fallback renders through `json.dumps`, hence
the quotes around a time of day. The Rust engine decodes MySQL's `TIME` into its
own time value and renders it bare. Everything else in the row matches byte for
byte, including the `binary(16)` uuid, the blob with a NUL and 0xFF, and the
json the server re-ordered.

### MySQL: the missing level message loses its hint

`mysql_schemas_live`:

    python: {"event": "error", "message": "ValueError: mysql has no schema level;
             catalogs lists its databases and tables lists their tables"}   (exit 1)
    rust:   {"event": "error", "message": "mysql has no schema level"}      (exit 1)

Both refuse before the network is touched; the difference is the hint, and the
fact that the Python engine's error event carries the exception class. The
hint is the sentence the app shows the user, so this case is the record of what
that sentence is today.

## What could not be recorded

- **`export` and `to_table` on Postgres and MySQL.** Both engines implement
  them for both drivers, and both were left out on purpose: their stdout carries
  paths and sizes, not values, so a live case would freeze little that the
  in-process `export_csv` and `to_table_create` do not already. They are the
  biggest remaining hole in the matrix if the parent wants it closed.
- **`test` on Postgres and MySQL.** The in-process `test_probe` covers the
  event; a live run would add only the real `catalog_count`.
- **Live Trino `catalogs`, `schemas` and `tables`.** These stay in-process only.
  The Trino container was taken down and restarted by other agents repeatedly
  through this work -- when one re-record attempt caught it down, the recorder
  wrote nothing rather than freeze a connection failure -- so the six Trino
  cases were recorded in one healthy window and the whole set was re-verified
  against the Rust engine once the container came back. Adding the three missing
  browse commands needs another healthy window, not more code.
- **The `mysql_*` comparison against the Rust engine, on the first attempt.**
  `target/debug/queryhive-engine` could not reach `127.0.0.1:53306` while other
  agents were editing `crates/qh-driver-mysql`: `invalid peer certificate:
  UnknownIssuer`, from a `Prefer` default that refused the container's
  self-signed certificate. That work landed while this was being written -- the
  same binary and settings connect now, and all seven `mysql_*` cases have been
  compared (results above and in the list). Nothing about the golden cases
  depended on it, since `golden.rs` drives a `FakeSession` and never opens a
  socket.
- **A clean `QH_TEST_TRINO=1 QH_TEST_POSTGRES=1 QH_TEST_MYSQL=1 cargo test
  --workspace` run.** It was run with `--no-fail-fast` and every test binary
  passed except three: `qh-ffi --test golden` (4 passed, 1 failed -- the
  expected unclassified-case panic above), `qh-driver-trino --test integration`
  (6 passed, 13 failed) and `qh-driver-mysql --test integration` (16 passed, 1
  failed). The last two are the dev containers: another agent was restarting
  Trino throughout this work, and both suites pass when re-run on their own --
  trino 19/19, postgres 14/14, mysql 17/17. Nothing outside `qh-ffi --test
  golden` failed for a reason that has anything to do with these snapshots.
