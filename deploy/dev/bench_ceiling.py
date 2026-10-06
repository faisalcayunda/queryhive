#!/usr/bin/env python3
"""Same-day ceilings for axis 2 (rows per second into the grid).

    python3 deploy/dev/bench_ceiling.py pg                       # COPY of `SELECT * FROM wide_500k`
    python3 deploy/dev/bench_ceiling.py pg --sql "SELECT ..." --rows 500000
    python3 deploy/dev/bench_ceiling.py trino --sql "SELECT * FROM tpch.sf1.lineitem LIMIT 1000000" --rows 1000000
    python3 deploy/dev/bench_ceiling.py mysql                    # says there is no valid ceiling

    # the whole axis-2 record: the app's samples in, the same samples with the ceiling out
    QueryHive --bench rows-pg-table-500k --bench-repeat 10 \\
        | python3 deploy/dev/bench_ceiling.py pg --merge \\
        | python3 deploy/dev/bench_app.py --source app --repeat 10

What a ceiling is, and is not (W8-T2 decision, row 6): the ceiling is the rate at which the
*server* can hand over **the exact SQL the app ran**, taken the same day on the same machine.
A ceiling of some other statement is not this scenario's ceiling. The app's old PG scenario ran
a `generate_series` with an md5 per cell, which the server computes at about 81k rows/s, and
the app reached 95% of that: it measured the server, and the table scan (958k rows/s) was
never compared at all. So the kind of ceiling is recorded with it, and `bench_fetch.py` only
lets a ceiling lower the 575,000 rows/s target when the kind is the one the row accepts:

* ``same-sql-copy``: `psql -c "COPY (<sql>) TO STDOUT" > /dev/null`, median of ``--runs``.
  PostgreSQL's own client reading the rows in text and doing nothing with them.
* ``nexturi-drain``: Trino's REST protocol drained from this host: POST the statement, follow
  `nextUri` until it is gone, read every page and decode none. It bounds what the server and
  the HTTP stream can deliver, which is the most a client can ever show.
* ``none``: MySQL. The `mysql` CLI is slower than the app (it formats every cell), so it
  bounds nothing, and the absolute 575,000 rows/s applies.

`--merge` reads the app's sample lines from standard input *first* and measures the ceiling
after, so the ceiling does not compete with the run it describes. It adds
`ceiling_rows_per_s` to every sample's metrics and `ceiling_kind=...` to the first sample's
notes (`bench_app.py` keeps one `notes` string per record, and the report reads the kind from
there). A sample of a scenario this ceiling does not belong to stops the pipeline.
"""

from __future__ import annotations

import argparse
import http.client
import json
import os
import pathlib
import re
import shutil
import statistics
import subprocess
import sys
import time
from urllib.parse import urlsplit

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import bench_fetch as bf  # noqa: E402

PG_KIND, TRINO_KIND, NO_KIND = "same-sql-copy", "nexturi-drain", "none"
NEXT_URI = re.compile(rb'"nextUri"\s*:\s*"([^"]+)"')
FINISHED = re.compile(rb'"state"\s*:\s*"FINISHED"')
# `nextUri` is one of the first fields of a page, before `columns` and `data`, and a page can be
# megabytes: looking only at the head keeps the scan off the rows.
PAGE_HEAD = 4096
RETRY_STATUS = (502, 503, 504)
LIBPQ_PSQL = "/opt/homebrew/opt/libpq/bin/psql"


def psql_binary() -> str:
    found = os.environ.get("PSQL") or shutil.which("psql") or LIBPQ_PSQL
    if not pathlib.Path(found).is_file():
        raise SystemExit(f"psql not found ({found}); install libpq or set PSQL")
    return found


def median(values) -> float:
    return float(statistics.median(values))


def copy_rates(sql: str, rows: int, runs: int) -> list[float]:
    """Rows per second of `COPY (<sql>) TO STDOUT` to /dev/null, one value per run."""
    pg = bf.CONNECTIONS["postgres"]
    env = dict(os.environ, PGPASSWORD=pg["DB_PASSWORD"])
    statement = f"COPY ({sql.strip().rstrip(';')}) TO STDOUT"
    rates = []
    for _ in range(runs):
        started = time.perf_counter()
        done = subprocess.run(
            [psql_binary(), "-h", pg["DB_HOST"], "-p", pg["DB_PORT"], "-U", pg["DB_USER"],
             "-d", pg["DB_DATABASE"], "-q", "-c", statement],
            env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        elapsed = time.perf_counter() - started
        if done.returncode:
            raise SystemExit(f"psql failed: {done.stderr.decode(errors='replace')[:300]}")
        rates.append(rows / elapsed)
    return rates


def _page(connection: http.client.HTTPConnection, method: str, target: str, headers: dict,
          body: bytes | None = None) -> bytes:
    """One page of the Trino protocol. The server asks a client to come back later with 502, 503
    or 504, and a client that does not would stop at the first of them."""
    for _ in range(100):
        connection.request(method, target, body=body, headers=headers)
        response = connection.getresponse()
        data = response.read()
        if response.status in RETRY_STATUS:
            time.sleep(0.05)
            continue
        if response.status != 200:
            raise SystemExit(f"Trino answered {response.status} to {method} {target}: {data[:200]!r}")
        return data
    raise SystemExit(f"Trino kept answering {RETRY_STATUS} to {method} {target}")


def drain_trino(host: str, port: int, user: str, sql: str, catalog: str = "tpch",
                schema: str = "sf1", timeout: float = 600) -> tuple[float, int]:
    """Seconds from posting `sql` to the last page, and the bytes read. Nothing is decoded: a
    client that parses every row is measuring its parser, and the ceiling is what the server and
    the stream can deliver."""
    headers = {"X-Trino-User": user, "X-Trino-Catalog": catalog, "X-Trino-Schema": schema,
               "Accept-Encoding": "identity"}
    connection = http.client.HTTPConnection(host, port, timeout=timeout)
    started = time.perf_counter()
    body = _page(connection, "POST", "/v1/statement", headers, sql.encode())
    total = 0
    while True:
        total += len(body)
        found = NEXT_URI.search(body[:PAGE_HEAD])
        if not found:
            break
        target = urlsplit(found.group(1).decode())
        body = _page(connection, "GET", target.path + (f"?{target.query}" if target.query else ""), headers)
    elapsed = time.perf_counter() - started
    connection.close()
    if not FINISHED.search(body):
        raise SystemExit(f"the Trino drain ended on a page that is not FINISHED: {body[:300]!r}")
    return elapsed, total


def drain_rates(sql: str, rows: int, runs: int, catalog: str, schema: str) -> list[float]:
    trino = bf.CONNECTIONS["trino"]
    rates = []
    for _ in range(runs):
        elapsed, total = drain_trino(trino["DB_HOST"], int(trino["DB_PORT"]), trino["DB_USER"], sql,
                                     catalog, schema)
        # A drain that read next to nothing stopped early; its rate would be a number of a kind.
        if total < rows * 8:
            raise SystemExit(f"the drain read {total} bytes for {rows} rows; it stopped early")
        rates.append(rows / elapsed)
    return rates


def ceiling_record(kind: str, rates: list[float], sql: str | None, rows: int | None) -> dict:
    """What a measured ceiling says: the median and the spread it came from."""
    ordered = sorted(rates)
    return {
        "ceiling_kind": kind,
        "ceiling_rows_per_s": median(ordered) if ordered else None,
        "rates": [round(r) for r in ordered],
        "sql": sql,
        "rows": rows,
        "recorded_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
    }


def note_for(record: dict) -> str:
    """The notes a sample carries so the report can tell which ceiling this is (`key=value; text`)."""
    kind = record["ceiling_kind"]
    if kind == NO_KIND:
        return f"ceiling_kind={kind}; MySQL has no valid ceiling, so the absolute 575000 rows/s applies"
    rates = record["rates"]
    how = {PG_KIND: "same-day psql COPY of the exact SQL this scenario runs",
           TRINO_KIND: "same-day host-side drain of the nextUri chain, nothing decoded"}[kind]
    return (f"ceiling_kind={kind}; ceiling_rows_per_s is a {how}, median of {len(rates)} "
            f"({rates[0]}-{rates[-1]} rows/s)")


# Which scenarios a ceiling of each kind may be merged into (bench_fetch.CEILING_KIND, plus MySQL).
def allowed(kind: str, scenario: str) -> bool:
    if kind == NO_KIND:
        return "mysql" in scenario
    return bf.CEILING_KIND.get(scenario) == kind


def merge_samples(samples: list[dict], record: dict) -> list[dict]:
    """The samples with the ceiling in their metrics and its kind in the first one's notes."""
    for sample in samples:
        scenario = sample.get("scenario", "")
        if not allowed(record["ceiling_kind"], scenario):
            raise SystemExit(f"a {record['ceiling_kind']} ceiling does not belong to {scenario!r}")
    merged = []
    for index, sample in enumerate(samples):
        sample = dict(sample)
        if record["ceiling_rows_per_s"] is not None and isinstance(sample.get("metrics"), dict):
            sample["metrics"] = dict(sample["metrics"], ceiling_rows_per_s=record["ceiling_rows_per_s"])
        if index == 0:
            sample["notes"] = "; ".join(part for part in (sample.get("notes"), note_for(record)) if part)
        merged.append(sample)
    return merged


def measure(args) -> dict:
    if args.kind == "mysql":
        return ceiling_record(NO_KIND, [], None, None)
    sql = args.sql or bf.DEFAULT_SQL
    rows = args.rows or bf.WIDE_ROWS
    if args.kind == "pg":
        return ceiling_record(PG_KIND, copy_rates(sql, rows, args.runs), sql, rows)
    if not args.sql or not args.rows:
        raise SystemExit("trino needs --sql and --rows (the LIMIT in the statement)")
    return ceiling_record(TRINO_KIND, drain_rates(sql, rows, args.runs, args.catalog, args.schema), sql, rows)


def read_samples(text: str) -> list[dict]:
    """The sample objects on the app's standard output: JSON lines, anything else is a log."""
    samples = []
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("{"):
            try:
                item = json.loads(line)
            except json.JSONDecodeError as error:
                raise SystemExit(f"not valid JSON ({error}): {line[:80]}")
            if isinstance(item, dict):
                samples.append(item)
    return samples


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("kind", choices=("pg", "trino", "mysql"))
    parser.add_argument("--sql", help="the exact statement the app ran (pg default: SELECT * FROM wide_500k)")
    parser.add_argument("--rows", type=int, help="rows that statement returns (pg default: 500000)")
    parser.add_argument("--runs", type=int, default=5, help="repetitions; the ceiling is their median")
    parser.add_argument("--catalog", default="tpch", help="trino: X-Trino-Catalog")
    parser.add_argument("--schema", default="sf1", help="trino: X-Trino-Schema")
    parser.add_argument("--merge", action="store_true",
                        help="read the app's samples from stdin, print them with the ceiling added")
    parser.add_argument("--from", dest="saved", metavar="FILE",
                        help="with --merge: use a ceiling saved by --out instead of measuring")
    parser.add_argument("--out", metavar="FILE", help="also save the ceiling record to FILE")
    args = parser.parse_args(argv)
    if args.runs < 1:
        raise SystemExit("--runs must be at least 1")

    samples = read_samples(sys.stdin.read()) if args.merge else []
    if args.merge and not samples:
        raise SystemExit("--merge: no samples on standard input")
    record = json.loads(pathlib.Path(args.saved).read_text("utf-8")) if args.saved else measure(args)
    if args.out:
        pathlib.Path(args.out).write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", "utf-8")
    if args.merge:
        for sample in merge_samples(samples, record):
            print(json.dumps(sample, sort_keys=True))
    else:
        print(json.dumps(record, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
