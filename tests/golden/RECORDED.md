# Snapshots recorded against real servers

`index.json` holds two kinds of case. The in-process ones were written by a
recorder that drove the Python engine with a fake cursor, so what they pin is
that engine's own normalisation. That recorder went when the Python engine did
(`tools/golden/record.py` and `compare.py` are both gone); the cases it wrote
stayed. The ones this file is about were recorded by `tools/golden/live_cases.py`,
which starts the engine as a child process exactly as the app does and points it
at the containers `deploy/dev/up.sh` starts. Their case ids end in `_live`.

The difference matters. A fake cursor can hand the engine a `Decimal`, a
`timedelta` or a type name because the case author decided it should; it cannot
decide what psycopg really returns for a `numeric(38,10)`, what PyMySQL really
returns for a `time`, or what Trino really puts on the wire for a
`timestamp(6) with time zone`. Those are facts about the server, and the _live
cases freeze them.

Every case here ran, and every `.ndjson` is that run's stdout after the masking
`tools/golden/normalise.py` applies -- one shared module, so the recorder that
wrote the in-process cases and the reader that checks them here mask the same
things (timings, query ids, temp paths, and the OIDs the server itself assigned
-- see below). Nothing was
hand-written. A second `tools/golden/live_cases.py` run reproduced all fourteen
Postgres and MySQL cases and `trino_nation_live` byte for byte; the other five
Trino cases were recorded once, in the one window the container stayed up, and
were then compared against the Rust engine rather than re-recorded. A run that
caught Trino down recorded nothing at all and said so, which is the behaviour a
recorder needs.

Re-checked on 23 September 2026, when `live_cases.py` gained its read-only
default and the two `export` cases: all twenty-two cases below re-ran against
the same three servers and matched byte for byte (22/22). The two new ones are
the first live case for `export`; "What could not be recorded" says why
`to_table` still has none.

Re-checked again later the same day, with all three containers up: 22/22 again,
and this time it took a change to get there. Three Postgres cases had been
failing on nothing but an OID -- `a_mood`'s type code in `type_zoo_live` and
`export_live`, and the table OIDs in `objects_live` -- because
`deploy/dev/up.sh` drops and recreates its containers on every start and replays
the seed (`up.sh:56`), and a server hands out the next identifier each time.
PostgreSQL reserves 1..16383 for its own catalogues and starts client objects at
16384 (`pg_class`'s `FirstNormalObjectId`), so the number is the cluster's
bookkeeping rather than the engine's answer. Both normalisers now replace an
identifier at or above that floor with `<OID>`, in the two places a server puts
one -- a reported type code, and the cell the object browser's own `OID` column
names -- and the four snapshots that held one were rewritten once (the three
live cases plus the in-process `postgres_objects`). What that does *not* touch
is the point of the narrow rule: `MySQL`'s object browser reports a real row
count (`476354` for the seeded table) one column to the right of where Postgres
reports its OID, and a mask keyed on "five digits or more" would have erased it.
`the_server_oid_mask_reaches_the_two_places_an_oid_lives_and_nothing_else` in
`crates/qh-ffi/tests/golden.rs` pins both halves.

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

Capable means the **previous** engine -- the Python one, since deleted -- implemented
the command for that driver when this was recorded, read from `exporter/cli.py`,
`exporter/drivers.py` and its `commands` table. Those files are gone; the tables
below are the record of what they answered. A dash means the driver answered with
a usage error, so there is nothing to record but the refusal.

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

**After** (adding the twenty-two live cases):

| command | trino | postgres | mysql |
| --- | --- | --- | --- |
| `db_drivers` | case | -- | -- |
| `test` | case | capable, **still uncovered** | capable, **still uncovered** |
| `catalogs` | case (in-process only) | **live case** | case (in-process only) |
| `schemas` | case (in-process only) | case (in-process only) | **live refusal** |
| `tables` | case (in-process only) | **live case** | **live case** |
| `objects` | case + **live case** | case + **live case** | case + **live case** |
| `export` | case | **live case** | **live case** |
| `to_table` | case | capable, **still uncovered** | capable, **still uncovered** |
| `preview` | 5 cases + **2 live** | **2 live cases** | **2 live cases** |
| `count` | case + **live case** | **live case** | **live case** |
| `explain` | case + **live case** | **live case** | **live case** |

Still missing, in the order worth closing: `to_table` against Postgres and
MySQL -- and unlike `export`, that hole is now a decision with a reason rather
than an omission, since recording it does measurable damage (see the next
section); `test` against both; and a live `catalogs`/`schemas`/`tables` for
Trino, which today is in-process only. `db_drivers` needs nothing live: it reads
settings, not a server.

## How to check, and how to re-record

There is nothing to install first. The child these cases drive is
`target/debug/queryhive-engine`, the CLI half of the Rust engine, so the only
prerequisites are that binary and the servers:

    cargo build -p qh-ffi --bin queryhive-engine
    deploy/dev/up.sh                                        # postgres, mysql, trino
    /usr/bin/python3 tools/golden/live_cases.py             # check every case -- writes nothing
    /usr/bin/python3 tools/golden/live_cases.py <case-id>   # check one case
    /usr/bin/python3 tools/golden/live_cases.py --list      # what the cases are
    /usr/bin/python3 tools/golden/live_cases.py --record <case-id>   # record one that has no snapshot
    /usr/bin/python3 tools/golden/live_cases.py --record --force     # replace existing snapshots, on purpose

The harness is a plain Python script with no third-party imports, so any
interpreter runs it and `check_tools()` has one thing to check rather than a list
of packages: it refuses to run when the engine binary is missing or not
executable, and says which command builds it. The old recorder needed a venv
because *it* imported the client libraries -- it drove the Python engine in
process. This one starts a child process and reads its stdout, so the client
libraries that open the connections are the Rust driver crates, compiled into
the binary.

A bare invocation writes nothing: it re-runs each case and diffs the stdout
against the snapshot on disk, so a case that already exists cannot be lost by
typing the command without an argument. Recording is `--record`, and even then a
snapshot that already exists is refused by name unless `--force` says the
replacement is deliberate -- a recorded live case is the only copy of what a real
server answered. Recording a case that has no snapshot yet needs no extra flag.

A case that reports an error is never written: if a container is down the run
exits non-zero and leaves the tree untouched, so a connection failure can never
be frozen as if it were an answer.

### What a check says today

Run on 24 September 2026 against all three containers, with
`target/debug/queryhive-engine` built from this branch:

    12/22 live cases match

Ten fail, and none of them is a case that was passing before: every one is a
`columns` type spelling, a value rendering `docs/golden-deltas.md` classifies, or
one message whose wording changed. Two of the four cases this file listed as
"fix first" are green now -- `postgres_catalogs_live` and `trino_explain_live` are
both reproduced exactly -- so the remaining eight failures are:

| case | what differs | classified as |
| --- | --- | --- |
| `postgres_batching_live`, `postgres_explain_live`, `postgres_export_live`, `mysql_batching_live`, `mysql_explain_live`, `mysql_export_live` | `columns.type` only, plus the exported byte size on the two `export` cases | D-8, and the interval fold under D-2 for the size |
| `postgres_type_zoo_live`, `mysql_type_zoo_live` | `columns.type` plus the type-zoo cells | D-8, D-1, D-2, D-9 |
| `trino_type_zoo_live` | two cells: the tiny decimal and the interval | D-1, D-2 |
| `mysql_schemas_live` | the refusal's wording lost its hint | D-4 |

Nothing here is a regression, and recording the ten snapshots again would destroy
the only copy of what the previous engine answered -- which is what they are for.

## The cases

| case | command | engine | event lines | what it freezes |
| --- | --- | --- | --- | --- |
| `postgres_type_zoo_live` | `preview` | postgres | 4 | The wire shape psycopg hands over for every hard Postgres type, as the engine renders it: a numeric(38,10) with its digits, the scientific spelling psycopg's Decimal gives a tiny negative, a timestamptz the server converted to UTC (the seeded +07:00 is *not* what comes back), a naive timestamp that must not gain a zone, an interval, json and jsonb text in the server's own key order, arrays, bytea with a NUL and 0xFF, a uuid, an enum label, a point, a NULL, an empty string and the four characters NULL. The `columns` type is the type OID psycopg reports; the identifier of the seeded `mood` enum is masked to `<OID>` because it climbs every time the seed is replayed. |
| `postgres_batching_live` | `preview` | postgres | 5 | 250 real rows through PREVIEW_BATCH=200: the batch boundary, the second batch, and `truncated: true` from the one extra row the cap costs. |
| `postgres_catalogs_live` | `catalogs` | postgres | 1 | `catalogs` on the driver whose object tree has no catalog level: the pg_database listing, filtered to connectable non-template databases, in the query's own ORDER BY. |
| `postgres_tables_live` | `tables` | postgres | 1 | The information_schema statement Postgres builds for `tables`, against a schema that really holds the seeded tables -- BASE TABLE only, ordered. |
| `postgres_objects_live` | `objects` | postgres | 1 | The only driver whose Name/OID/Owner/ACL shape is real: a real OID, the real owner, and a NULL relacl rendered as an empty string. The OID cell is masked to `<OID>` for the reason above; the column list that names it is not. |
| `postgres_count_live` | `count` | postgres | 3 | `count`'s wrapper run by Postgres itself on 500,000 real rows, so the number is the server's rather than a fake cursor's. |
| `postgres_explain_live` | `explain` | postgres | 4 | Postgres answers EXPLAIN with one `QUERY PLAN` text column; this freezes the real column name, the real plan text and the absence of `truncated`. |
| `postgres_export_live` | `export` | postgres | 5 | The CSV writer fed by psycopg's real decoding rather than a fake cursor's rows: the `start` columns carry the server's own type OIDs (the seeded `mood` enum's masked to `<OID>` for the reason above), the header is the table's, and `done` reports the real row count and the file's real byte size. The path is masked to <TMP>; the size is not -- it is a fact about the bytes the writer produced from the values psycopg handed over. |
| `mysql_type_zoo_live` | `preview` | mysql | 4 | The same hard types again, as MySQL decodes them: decimal(38,10), datetime and timestamp kept distinct, a time PyMySQL hands back as an object, json re-ordered by the server, a blob with a NUL and 0xFF, an enum label, a char(36) and a binary(16) uuid, a NULL, an empty string and the four characters NULL. The `columns` type is MySQL's column type code. |
| `mysql_batching_live` | `preview` | mysql | 5 | 250 real rows through the 200-row preview batch: the boundary, the second batch and `truncated: true`. |
| `mysql_tables_live` | `tables` | mysql | 1 | MySQL's SHOW TABLES FROM `qh`, quoted with backticks, in the server's own order. |
| `mysql_objects_live` | `objects` | mysql | 1 | Name/Engine/Rows/Comment from information_schema.TABLES: the columns no other driver can answer, with a real engine name, a real (estimated) row count and an empty comment. |
| `mysql_schemas_live` | `schemas` | mysql | 1 | The one browse command MySQL has no level for: a usage error naming the driver, raised before the network is touched. |
| `mysql_count_live` | `count` | mysql | 3 | The count wrapper on MySQL over 500,000 real rows. |
| `mysql_explain_live` | `explain` | mysql | 4 | MySQL is the driver whose EXPLAIN is not one text column: a real multi-column plan table, which is why `explain` emits preview's protocol rather than a plan shape |
| `mysql_export_live` | `export` | mysql | 5 | The CSV writer fed by PyMySQL's real decoding: the `start` columns carry MySQL's own column type codes, and `done` reports the real row count and the file's real byte size -- a different size from Postgres's for the same statement, which is the point of freezing both. The path is masked to <TMP>, the size is not. |
| `trino_nation_live` | `preview` | trino | 4 | The real Trino decoder on tpch.tiny.nation: bigint, varchar, and a NULL comment that must arrive as JSON null rather than "None". |
| `trino_type_zoo_live` | `preview` | trino | 4 | Trino's own decoding of the types that matter, from expressions rather than a table: a decimal(38,10) with its digits intact, a tiny negative decimal, a timestamp with a zone, a naive timestamp, a date, a time, an interval, a NULL and an empty string. |
| `trino_batching_live` | `preview` | trino | 5 | 250 real tpch rows -- a bigint, the tpch connector's `double` totalprice and a date -- cut by the 200-row batch rule, so the boundary, the second batch and `truncated: true` are all frozen. tpch's generated orderkeys are sparse (1..7, then 32..39, ...), so the second batch starts at 801 rather than at 201: the row *number* is what the cap counts, not the key. |
| `trino_objects_live` | `objects` | trino | 1 | Trino's information_schema objects query: Name/Type only, because Trino has no OID, owner or ACL to answer with. |
| `trino_count_live` | `count` | trino | 3 | The count wrapper run by the coordinator over tpch.tiny.orders. |
| `trino_explain_live` | `explain` | trino | 4 | A real Trino plan: one varchar column, one row per plan line, fetched through EXPLAIN with the caller's semicolon stripped. |

`<TMP>` is the temp root the recorder masks: an `export` case writes into
`<TMP>/qh-golden-<case>/`, and `done.files` carries that masked path with the real
byte size beside it. Everything below the table is printed by
`/usr/bin/python3 tools/golden/live_cases.py --markdown`, from the same case table
the recorder runs, so neither the case list nor a command line can drift from what
was actually executed. Every setting is explicit, and `RETRIES=0` keeps the engine
from reconnecting under a dead server (`deploy/dev/qh-pg-old.sh` and
`qh-mysql-old.sh` are the variants that run real servers instead of containers).

    # postgres_type_zoo_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=postgres DB_PASSWORD=qh-dev-only DB_PORT=55432 DB_SCHEMA=public DB_SSLMODE=disable DB_USER=qh RETRIES=0 SQL='SELECT * FROM type_zoo' target/debug/queryhive-engine preview

    # postgres_batching_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=postgres DB_PASSWORD=qh-dev-only DB_PORT=55432 DB_SCHEMA=public DB_SSLMODE=disable DB_USER=qh LIMIT=250 RETRIES=0 SQL='SELECT id, c01 FROM wide_500k ORDER BY id' target/debug/queryhive-engine preview

    # postgres_catalogs_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=postgres DB_PASSWORD=qh-dev-only DB_PORT=55432 DB_SCHEMA=public DB_SSLMODE=disable DB_USER=qh RETRIES=0 target/debug/queryhive-engine catalogs

    # postgres_tables_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=postgres DB_PASSWORD=qh-dev-only DB_PORT=55432 DB_SCHEMA=public DB_SSLMODE=disable DB_USER=qh RETRIES=0 target/debug/queryhive-engine tables

    # postgres_objects_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=postgres DB_PASSWORD=qh-dev-only DB_PORT=55432 DB_SCHEMA=public DB_SSLMODE=disable DB_USER=qh RETRIES=0 target/debug/queryhive-engine objects

    # postgres_count_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=postgres DB_PASSWORD=qh-dev-only DB_PORT=55432 DB_SCHEMA=public DB_SSLMODE=disable DB_USER=qh RETRIES=0 SQL='SELECT * FROM wide_500k' target/debug/queryhive-engine count

    # postgres_explain_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=postgres DB_PASSWORD=qh-dev-only DB_PORT=55432 DB_SCHEMA=public DB_SSLMODE=disable DB_USER=qh RETRIES=0 SQL='SELECT * FROM type_zoo WHERE id = 1' target/debug/queryhive-engine explain

    # postgres_export_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=postgres DB_PASSWORD=qh-dev-only DB_PORT=55432 DB_SCHEMA=public DB_SSLMODE=disable DB_USER=qh FORMAT=csv NAME=type_zoo OUT_DIR=<TMP>/qh-golden-postgres_export_live RETRIES=0 SQL='SELECT * FROM type_zoo' target/debug/queryhive-engine export

    # mysql_type_zoo_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=mysql DB_PASSWORD=qh-dev-only DB_PORT=53306 DB_USER=qh RETRIES=0 SQL='SELECT * FROM type_zoo' target/debug/queryhive-engine preview

    # mysql_batching_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=mysql DB_PASSWORD=qh-dev-only DB_PORT=53306 DB_USER=qh LIMIT=250 RETRIES=0 SQL='SELECT id, c01 FROM wide_500k ORDER BY id' target/debug/queryhive-engine preview

    # mysql_tables_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=mysql DB_PASSWORD=qh-dev-only DB_PORT=53306 DB_USER=qh RETRIES=0 target/debug/queryhive-engine tables

    # mysql_objects_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=mysql DB_PASSWORD=qh-dev-only DB_PORT=53306 DB_USER=qh RETRIES=0 target/debug/queryhive-engine objects

    # mysql_schemas_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=mysql DB_PASSWORD=qh-dev-only DB_PORT=53306 DB_USER=qh RETRIES=0 target/debug/queryhive-engine schemas

    # mysql_count_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=mysql DB_PASSWORD=qh-dev-only DB_PORT=53306 DB_USER=qh RETRIES=0 SQL='SELECT * FROM wide_500k' target/debug/queryhive-engine count

    # mysql_explain_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=mysql DB_PASSWORD=qh-dev-only DB_PORT=53306 DB_USER=qh RETRIES=0 SQL='SELECT * FROM type_zoo WHERE id = 1' target/debug/queryhive-engine explain

    # mysql_export_live
    DB_DATABASE=qh DB_HOST=127.0.0.1 DB_KIND=mysql DB_PASSWORD=qh-dev-only DB_PORT=53306 DB_USER=qh FORMAT=csv NAME=type_zoo OUT_DIR=<TMP>/qh-golden-mysql_export_live RETRIES=0 SQL='SELECT * FROM type_zoo' target/debug/queryhive-engine export

    # trino_nation_live
    DB_DATABASE=tpch DB_HOST=127.0.0.1 DB_KIND=trino DB_PORT=58080 DB_SCHEMA=tiny DB_USER=queryhive RETRIES=0 SQL='SELECT nationkey, name, regionkey, comment FROM tpch.tiny.nation ORDER BY nationkey' target/debug/queryhive-engine preview

    # trino_type_zoo_live
    DB_DATABASE=tpch DB_HOST=127.0.0.1 DB_KIND=trino DB_PORT=58080 DB_SCHEMA=tiny DB_USER=queryhive RETRIES=0 SQL='SELECT CAST(1234567890123456789012345678.1234567890 AS decimal(38,10)) AS high_precision, CAST(-0.0000000001 AS decimal(38,10)) AS tiny_negative, TIMESTAMP '\''2026-01-31 12:00:00.123456 +07:00'\'' AS tz_aware, TIMESTAMP '\''2026-01-31 12:00:00.123456'\'' AS tz_naive, DATE '\''2026-01-31'\'' AS a_date, TIME '\''23:59:59.999999'\'' AS a_time, INTERVAL '\''3'\'' DAY + INTERVAL '\''4'\'' HOUR + INTERVAL '\''5'\'' MINUTE + INTERVAL '\''6'\'' SECOND AS an_interval, CAST(NULL AS varchar) AS no_value, '\'''\'' AS empty_text' target/debug/queryhive-engine preview

    # trino_batching_live
    DB_DATABASE=tpch DB_HOST=127.0.0.1 DB_KIND=trino DB_PORT=58080 DB_SCHEMA=tiny DB_USER=queryhive LIMIT=250 RETRIES=0 SQL='SELECT orderkey, totalprice, orderdate FROM tpch.tiny.orders ORDER BY orderkey' target/debug/queryhive-engine preview

    # trino_objects_live
    DB_DATABASE=tpch DB_HOST=127.0.0.1 DB_KIND=trino DB_PORT=58080 DB_SCHEMA=tiny DB_USER=queryhive RETRIES=0 target/debug/queryhive-engine objects

    # trino_count_live
    DB_DATABASE=tpch DB_HOST=127.0.0.1 DB_KIND=trino DB_PORT=58080 DB_SCHEMA=tiny DB_USER=queryhive RETRIES=0 SQL='SELECT * FROM tpch.tiny.orders' target/debug/queryhive-engine count

    # trino_explain_live
    DB_DATABASE=tpch DB_HOST=127.0.0.1 DB_KIND=trino DB_PORT=58080 DB_SCHEMA=tiny DB_USER=queryhive RETRIES=0 SQL='SELECT * FROM tpch.tiny.nation;' target/debug/queryhive-engine explain


## How a case is classified

`crates/qh-ffi/tests/golden.rs` holds three lists, and every case in
`tests/golden/index.json` has to be in one of them -- `a_new_snapshot_cannot_be_ignored`
walks the directory and fails by name on any case that is in none, so freezing a
new snapshot cannot silently skip the test. Where each list stands today:

| list | what it means | count |
| --- | --- | --- |
| `EXACT` | the fake-session cases this engine reproduces byte for byte, after the same normalisation | 17 |
| `LIVE` | recorded from a real server; compared against it by `tools/golden/live_cases.py`, not by this test file | 22 |
| `ACCEPTED` | in-process cases that differ **on purpose**, each with its reason in the comment beside it | 4 |

`LIVE` is not a place to park a snapshot. Being there only asserts that the id is
declared in `tools/golden/live_cases.py`, which is the table naming the command,
the settings and the container that produced it -- so the comparison can be run
again by anyone. The ids are matched exactly rather than as substrings, which
`a_declared_case_id_is_matched_exactly_and_not_as_a_substring` pins.

Adding a case is then two steps, and the second is the one that gets forgotten:
put the id in a list, and for an `EXACT` id script a `Case` in `cases()` whose
session produces those bytes -- `every_exact_case_matches_the_python_snapshot`
asserts `checked == EXACT.len()`, so an id in `EXACT` with no `Case` fails there
instead. The values to script are in the case's `.ndjson`, cell by cell.

Measured on 24 September 2026:

    $ cargo test -p qh-ffi --test golden
    test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

## Divergences against real servers

Found by running `target/debug/queryhive-engine` and the Python engine with the
same settings on the same server and diffing the events (masks applied as
usual). This is the part of the run that the fake-cursor cases structurally
cannot produce: they decide the value, the decoder never runs.

The findings below were made against a `target/debug/queryhive-engine` built on
22 September 2026. They were re-measured on **24 September 2026** with a binary
built from this branch, and the verdicts moved: three of the four defects this
file used to call "fix first" are closed, and one that did not look like a defect
turned out to be one. Each subsection says where it stands now, and the failing
set is in "What a check says today" above. Where a subsection has not been
re-measured since the fix, it says so instead of assuming.

### Trino: the driver does not advertise `PARAMETRIC_DATETIME`, so the server downgrades every value

**Closed.** The driver now sends the header:
`crates/qh-driver-trino/src/lib.rs:131` holds
`const CLIENT_CAPABILITIES: &str = "PARAMETRIC_DATETIME"` and `:482` attaches it to
every request as `X-Trino-Client-Capabilities`. Re-measured on 24 September 2026,
`trino_type_zoo_live` matches the snapshot on `columns` and on eight of its nine
cells -- the three rows of the table below are all correct now. The test that used
to pin the truncation as if it were the contract is gone; its replacement,
`the_announced_capability_buys_microseconds_through_the_real_protocol`
(`crates/qh-driver-trino/tests/integration.rs:231`), asserts the opposite.

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
(`trino/client.py` 0.339). That was the one-line fix this file predicted, and it
is the fix that landed.

The paragraph that used to sit here flagged
`crates/qh-driver-trino/tests/integration.rs:173` for having a test that pinned
the symptom as the contract. That file has been rewritten since: the test named
there no longer exists, and what replaced it asserts the announced capability
rather than the truncation. Nothing is left to settle.

The interval text in the same case (`"3 days, 4:05:06"` against `3 04:05:06.000`)
was a separate difference, and for a while a bigger one than the delta
`docs/golden-deltas.md` describes for the in-process case: the body differed too,
not only the quotes, because Trino's interval had no decoder and the wire text was
passed through. That is closed -- see the section at the end of this file. The body
now agrees and only the quotes remain, which is exactly D-2.

### Trino: `explain` does not strip the caller's semicolon

**Closed.** `crates/qh-driver-trino/src/lib.rs:1188` builds its statement as
`format!("EXPLAIN {}", strip_terminator(sql))`, so the terminator is handled in
the driver rather than by a test double. Re-measured on 24 September 2026,
`trino_explain_live` is one of the twelve cases that match the snapshot exactly.

The snapshot below is the failure as it was, kept because it is the reason the
driver strips the terminator at all:

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
side (columns, the plan row, done), so this case could not be listed at all until
the terminator was handled.

The in-process `explain` case could not see this: its fake session implements
`explain_statement` by calling `qh_sql::strip_terminator` itself, so the
stripping the real session is supposed to do was done by the test double. That is
why a live case was the only place this could show up, and why it is worth
keeping the snapshot even now that the two agree.

### Postgres: `catalogs` is refused

**Closed.** The refusal turned out not to be in the driver at all: PostgreSQL's
driver answers `catalogs`, and what refused first was a gate in
`crates/qh-ffi/src/commands.rs` reading `capabilities().levels` -- the
description of the **tree** -- as "the commands this driver answers". Those are
two different questions, and the previous engine pinned them separately
(`catalogs` returned the database list while `levels` stayed schema-first).
Re-measured on 24 September 2026, `postgres_catalogs_live` matches the snapshot
exactly: `{"event":"catalogs","names":["postgres","qh"]}`.

Kept because it is the reason the two questions are separate:

`postgres_catalogs_live` was recorded from a server that answers
`pg_database` with two databases:

    python: {"event": "catalogs", "names": ["postgres", "qh"]}                      (exit 0)
    rust:   {"event": "error", "message": "postgres has no catalog level"}           (exit 1)

The Python engine's Postgres browse list included the catalog level -- that is
how the case has an event at all -- while the Rust engine's did not, so the app's
catalog picker lost both databases.

### Postgres: interval, arrays and uuid are decoded differently

**Still open.** Re-measured on 24 September 2026, `postgres_type_zoo_live` differs
in three value cells plus the type spelling above. `rows` event:

| column | python | rust |
| --- | --- | --- |
| `an_interval` | `"428 days, 4:05:06"` | `14 months, 3 days, 4:05:06` |
| `ints` / `texts` / `nested` | `[1, null, 3]` / `["a", null, "c"]` / `[[1, 2], [3, null]]` | `{1,NULL,3}` / `{a,NULL,c}` / `{{1,2},{3,NULL}}` |
| `a_uuid` | `"550e8400-e29b-41d4-a716-446655440000"` (quotes included) | `550e8400-e29b-41d4-a716-446655440000` |

The interval is not just spelled differently: psycopg folds the seeded
`1 year 2 mons 3 days 04:05:06` into a `timedelta` of 428 days, while the Rust
side keeps `months: 14, days: 3`. Whichever side is right, this case is the one
that pins it.

The arrays are the newest of the three and the only one that is a real
disagreement about what the protocol should carry: the Rust side keeps
PostgreSQL's own array literal (`crates/qh-driver-postgres/src/normalize.rs:47`
argues it -- parsing `{1,NULL,3}` into elements means writing an array-literal
parser for nested braces, quoted elements and escapes, to reformat a string
nobody parses), while the previous engine sent a JSON array because psycopg had
already decoded one. Both are strings in the cell either way; what differs is
whether the cell holds JSON or PostgreSQL syntax. It is recorded as D-9 in
`docs/golden-deltas.md`.

The uuid is D-2's family: the quotes were `json.dumps(default=str)` wrapping a
`uuid.UUID`, never part of the value, and the Rust side renders the bare UUID.
`crates/qh-driver-postgres/src/normalize.rs:723` pins it.

One consequence worth knowing before anyone reads the two `export` snapshots:
these renderings change the exported bytes. The same `SELECT * FROM type_zoo`
that wrote 508 bytes of Postgres CSV writes **499** now, and MySQL's 398 became
**399**. So `postgres_export_live` and `mysql_export_live` fail on `files.bytes`
in the `done` event -- and their `files.bytes` line is the only place in the
corpus where the size of a rendering decision is visible as a number.

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

`mysql_schemas_live`, snapshot against the engine as it is on 24 September 2026:

    golden: {"event": "error", "message": "ValueError: mysql has no schema level;
             catalogs lists its databases and tables lists their tables"}   (exit 1)
    now:    {"event": "error", "message": "mysql has no schema level; use
             catalogs or tables instead"}                                    (exit 1)

Both refuse before the network is touched, and both name the alternative, so the
case does not fail for the reason this section was written about. It fails on
wording: the hint was put back (`mysql has no schema level; use catalogs or
tables instead`) but it is not the sentence the snapshot holds, and the
exception-class prefix is gone by design (D-4).

The lesson is in this row rather than in the fix: a snapshot of an error message
fails when the message is improved, and that is the correct behaviour for a
golden corpus -- "the sentence changed" is exactly what someone should have to
say out loud. What it does not mean is that the message is worse. The hint
concatenated here is the sentence the app shows the user, and
`a_level_trino_does_not_have_is_refused_with_a_usable_message`
(`crates/qh-driver-trino/tests/integration.rs:502`) is the test that guards that
shape for Trino.

### Postgres: `to_table` reports a write the server never commits

Found while deciding whether a live `to_table` case was worth recording. The
engine reports success; the table is not there:

    $ DB_KIND=postgres DB_HOST=127.0.0.1 DB_PORT=55432 DB_USER=qh DB_PASSWORD=qh-dev-only \
        DB_DATABASE=qh DB_SCHEMA=public DB_SSLMODE=disable RETRIES=0 SQL='SELECT * FROM type_zoo' \
        TARGET_SCHEMA=public TARGET_TABLE=golden_postgres_to_table_live WRITE_MODE=replace \
        app/engine/queryhive_engine.py to_table
    {"event": "step", "step": "connect"}
    {"event": "step", "step": "write"}
    {"event": "done", "rows": -1, "table": "public.golden_postgres_to_table_live", "mode": "replace",
     "query_id": null, "cancelled": false,
     "warnings": ["dropped the existing table public.golden_postgres_to_table_live"]}

    $ podman exec qh-postgres psql -U qh -d qh -tAc \
        "SELECT tablename FROM pg_tables WHERE schemaname='public' ORDER BY tablename"
    ['type_zoo', 'wide_500k']

`exporter/drivers.py`'s Postgres `connect()` builds `psycopg.connect(...)` with
no `autocommit` and `exporter/to_table.py` never calls `commit()`, so the
connection closes at the end of the run and the server rolls the CTAS back. The
`-1` is the visible half of the same fact: psycopg's `rowcount` for a CTAS is
`-1`, so the count is lost along with the table. MySQL is the opposite on both
points -- its DDL commits implicitly and its CTAS reports the rows written -- and
the contrast is what a live case would have pinned. The engine's own docstring
("the database runs the SELECT and commits the result itself") is not true for
this driver.

## What could not be recorded

- **`export` on Postgres and MySQL -- closed on 23 September 2026.** The reason
  given here before ("their stdout carries paths and sizes, not values") was
  wrong. `done` carries the real row count and the file's real byte size, and
  `start` carries the driver's own column type codes -- none of which the
  in-process `export_csv` can produce, because it hands the writer rows a case
  wrote down. `postgres_export_live` and `mysql_export_live` are recorded in
  `tests/golden/export/`: the same `SELECT * FROM type_zoo` writes 508 bytes of
  Postgres CSV against 398 of MySQL CSV, and the two `start` events disagree on
  every type on the wire.
- **`to_table` on Postgres and MySQL -- re-examined, and still not recorded.**
  This one was tried against both servers. Each attempt failed for a reason that
  writing the file anyway would not fix:
  - *MySQL.* The write commits -- MySQL's DDL is implicitly committed -- so
    `golden_mysql_to_table_live` stayed in `qh` and changed two snapshots that
    already existed: `mysql_tables_live` (SHOW TABLES) and `mysql_objects_live`
    (information_schema.TABLES) both went from `ok` to `DIFF` on the next check.
    A case whose execution rewrites another case's expected output is worse than
    a hole, and re-recording those two to expect the artifact would make them
    depend on whether this case had run first.
  - *Postgres.* The write does **not** commit. `done` reports
    `"table": "public.golden_postgres_to_table_live"` and exit 0, and `pg_tables`
    afterwards lists only `type_zoo` and `wide_500k`. Freezing that event would
    enshrine a success the server contradicts -- the mistake the recorder
    already refuses to make for a connection failure. The defect and its cause
    are in the divergences above.
  What a `to_table` case would pin -- the driver-built statement, the
  `public.<table>` / `qh.<table>` spelling, and the count the server reports
  (`-1` on Postgres against the written count on MySQL) -- is pinned by nothing
  today. Closing it needs the Postgres commit defect and the MySQL shared-table
  problem solved first.
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

## Setelah perbaikan (23 Sep 2026)

Empat cacat yang dicatat di atas sudah dikerjakan, dan yang berubah diukur ulang terhadap
server yang sama dengan perintah yang sama. Baris di bawah menggantikan verdict di atas untuk
kasus yang disebut; sisanya belum berubah.

| kasus | sebelum | sesudah |
| --- | --- | --- |
| `trino_explain_live` | 2 event, `SYNTAX_ERROR` pada `;` | **4 event, identik** dengan snapshot |
| `trino_type_zoo_live` | tipe kolom turun menjadi `timestamp with time zone`/`time`, nilai `.123000` dan `00:00:00` | **tipe kolom identik**; sisa dua sel nilai |
| `mysql_type_zoo_live` | `a_enum` bertipe `char` | `a_enum` bertipe **`enum`** — flag `ENUM_FLAG` dibaca, bukan kode `254` yang dipakai bersama `CHAR` |
| `mysql_schemas_live` | `mysql has no schema level` | `mysql has no schema level; use catalogs or tables instead` |
| `postgres_catalogs_live` | ditolak (`postgres has no catalog level`) | **identik** dengan snapshot: `{"event":"catalogs","names":["postgres","qh"]}` |

Dua sel yang masih berbeda di `trino_type_zoo_live`, keduanya sudah punya sebab:

- `tiny_negative`: `-1E-10` melawan `-0.0000000001` — D-1, perbaikan disengaja.
- `an_interval`: `"3 days, 4:05:06"` melawan `3 days, 4:05:06` -- **hanya kutipnya**, yaitu D-2.
  Sebelum ini badannya juga berbeda (`3 04:05:06.000`), karena Trino `INTERVAL DAY TO SECOND` belum
  punya decoder sehingga teks wire diteruskan apa adanya. Decoder itu sekarang ada: kedua jenis
  interval Trino (`DAY TO SECOND` dan `YEAR TO MONTH`) masuk ke model nilai dan memakai perender
  yang sama dengan PostgreSQL, jadi selisihnya tinggal kutip -- sama seperti kasus Postgres.

Penolakan `catalogs` itu ternyata **bukan** di driver. Driver PostgreSQL sudah menjawabnya, dan
yang menolak lebih dulu adalah gate di `crates/qh-ffi/src/commands.rs`, yang membaca
`capabilities().levels` -- deskripsi **pohon** -- sebagai "perintah yang dijawab driver". Kedua
hal itu memang berbeda: daftar database yang bisa dihubungi adalah daftar **pilihan**, bukan
simpul yang digambar di bawah koneksi, dan mesin lama memaku keduanya sekaligus dalam satu uji
(`catalogs` menjawab daftar database, sementara `levels` tetap schema-first). Sekarang level
semacam itu diteruskan ke driver; MySQL tetap menolak `schemas`-nya sebelum menyentuh jaringan.

Dua keputusan yang diambil dari sini, beserta tempatnya:

- `columns.type` adalah **nama tipe, bukan kode DBAPI** — `docs/golden-deltas.md` D-8. Kasus yang
  hanya berbeda di medan ini diklasifikasikan oleh entri itu, bukan ditimbang ulang satu per satu.
- Kasus live tetap di daftar `LIVE` di `crates/qh-ffi/tests/golden.rs`, dengan gigi bahwa setiap id
  wajib dideklarasikan di `tools/golden/live_cases.py`. Kedua puluh dua sudah di sana, termasuk dua
  kasus `export`, dan `cargo test -p qh-ffi --test golden` hijau (9 lulus) pada 24 Sep 2026.

### Diukur ulang 24 Sep 2026

Angka "22/22 cocok" di atas berlaku untuk build 22–23 Sep. Terhadap
`target/debug/queryhive-engine` yang dibangun 24 Sep dari branch ini, hasilnya
**12/22**, dan yang berubah bukan hanya angka: enam kasus yang dulu merah sekarang hanya berbeda di
`columns.type` (D-8), tiga cacat yang dulu "fix first" sudah tertutup
(`postgres_catalogs_live`, `trino_explain_live`, dan capability header Trino), sementara
`mysql_schemas_live` masih merah karena kata-katanya berbeda dari snapshot. Delapan sisanya selisih
nilai yang sudah diklasifikasi (D-1, D-2, D-9) atau hanya medan `columns.type`.

Dua kasus `export` live menyusul 23 Sep 2026 -- `postgres_export_live` dan `mysql_export_live`, satu
per server untuk `SELECT * FROM type_zoo` yang sama -- sehingga celah `export` di matriks di atas
tertutup; keduanya sudah diverifikasi ulang (22/22 kasus cocok). Keduanya **belum** ada di daftar
`LIVE`, jadi `a_new_snapshot_cannot_be_ignored` gagal dan menyebut `mysql_export_live` lebih dulu
sampai dua id itu ditambahkan. `to_table` untuk Postgres dan MySQL **tidak** direkam; dua sebabnya
terukur dan ada di "What could not be recorded" (MySQL meninggalkan tabel yang langsung mengubah dua
snapshot lain; Postgres tidak pernah commit, jadi snapshot-nya akan memakukan keberhasilan yang
dibantah server).

