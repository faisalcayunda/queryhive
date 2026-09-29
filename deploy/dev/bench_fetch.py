#!/usr/bin/env python3
"""Measure what the engine actually does, so the performance targets have a
baseline to be compared against.

    cargo build --release --bin queryhive-engine
    python3 deploy/dev/bench_fetch.py --kind postgres --label rust-release
    python3 deploy/dev/bench_fetch.py --report-only

How to reproduce this
---------------------
The Rust engine runs through this harness, against the fixture databases, with
the same statement, on the same machine. A number exists only if a run produced
it. There is no second arm any more: the Python engine this harness was built to
compare against has been deleted from the tree, so only its already-recorded
rows survive, as the frozen baseline the report compares against.

.. code-block:: bash

    # 1. the fixture databases (postgres 55432, mysql 53306)
    deploy/dev/up.sh all

    # 2. the engine, three repeats per database
    cargo build --release --bin queryhive-engine
    python3 deploy/dev/bench_fetch.py --kind postgres \\
        --label rust-release-<date> --repeat 3
    python3 deploy/dev/bench_fetch.py --kind mysql \\
        --label rust-release-<date> --repeat 3

    # 3. regenerate docs/benchmarks.md from the appended records
    python3 deploy/dev/bench_fetch.py --report-only

Use a fresh ``--label`` per measurement session: the report groups by
engine + kind + label + build, so reusing a label merges today's runs with an
older day's and the spread then describes two machines instead of one.

``--no-report`` appends the run without rewriting ``docs/benchmarks.md``. Use it
while another writer owns that file, then regenerate it once for the session.

Trino is not measured: the memory catalog in the dev container has no
``wide_500k``, so a Trino number would be a different workload. The recorded
Python baseline has no Trino run either.

Why a separate harness
---------------------
The targets in the blueprint (section 6) are stated as numbers — time to first
row under 200 ms, a 5x throughput improvement, memory under 800 MB. A number
without a measurement method is a claim, so this file is the method.

What it measures, and how
-------------------------
The engine is run as a child process, exactly as the app runs it, and every line
it writes to stdout is timestamped as it arrives. That yields:

* **time to first row** — process start to the first `rows` event. This is the
  number the user feels, and it is why the engine streams rather than buffering.
* **time to last row** — process start to `done`.
* **throughput** — rows divided by the time between the first and last row, so
  process startup is not counted as fetch speed.
* **peak RSS** — read from `/usr/bin/time -l`, which reports one child's peak
  rather than a cumulative figure that a previous run could inflate.

Results append to `deploy/dev/bench-results.jsonl` one JSON object per run, and
`--report-only` regenerates `docs/benchmarks.md` from that file. Nothing is ever
edited by hand into the report, so a number in the docs always has a run behind
it.

The comparison is honest by construction: the recorded Python rows and a fresh
Rust run were asked for the same statement, against the same server, on the same
machine. The engine is driven through its own `preview` command -- the `qh-ffi`
CLI, which exists for exactly this reason (blueprint section 4.5) -- and reads
its settings from the environment, so one `CONNECTIONS` table drives it.

Two timing numbers per run, and they are not the same thing:

* the harness's own wall clock, timestamped per stdout line — `total_ms`,
  `fetch_ms`, `time_to_first_row_ms`;
* `elapsed_ms`, which the engine reports in its `done` event. The engine stamps
  it immediately before emitting `step connect`, so it spans connect + fetch +
  emit and excludes process start. It is the only number that measures that
  span without the harness's own scheduling in the middle, which is why it is
  reported beside the wall clock rather than instead of it.

Peak RSS is per child process, read from `/usr/bin/time -l`. A run also records
the machine's load average, because a number taken while four other builds are
running describes the machine, not the engine.

One schema for every axis
-------------------------
The records of the other axes (`deploy/dev/bench_app.py`) share this file and
this report. Each record carries `bench` (fetch, app, ffi, blackbox; a record
without it is a fetch record), `axis`, `scenario`, `app`, `app_rev`,
`competitor_rev`, `hw_model`, `p_cores`, `e_cores`, `display_hz` and
`server_image`. `--report-only` renders the fetch section as before and one
section per axis: target, QueryHive, TablePro, verdict. A number that nobody
measured reads `[belum diukur]`; a TablePro number that cannot be had reads
`tidak diukur (izin OS)`, `tidak diukur (butuh sudo)` or `tidak mendukung`.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import statistics
import subprocess
import sys
import time

ROOT = pathlib.Path(__file__).resolve().parents[2]
RESULTS = ROOT / "deploy" / "dev" / "bench-results.jsonl"
REPORT = ROOT / "docs" / "benchmarks.md"
TIME_BIN = "/usr/bin/time"

# The Rust engine's CLI, by build profile. Release first: it is the profile the
# app ships and the one the recorded baseline was compared against. A debug
# binary is still measurable -- the golden harness runs one -- but it is
# recorded with its profile so a debug number is never read as the engine's
# speed.
RUST_BINARIES = (
    ("release", ROOT / "target" / "release" / "queryhive-engine"),
    ("debug", ROOT / "target" / "debug" / "queryhive-engine"),
)

# Connection settings matching deploy/dev/up.sh. Throwaway local fixtures.
CONNECTIONS = {
    "postgres": {
        "DB_KIND": "postgres",
        "DB_HOST": "127.0.0.1",
        "DB_PORT": "55432",
        "DB_USER": "qh",
        "DB_PASSWORD": "qh-dev-only",
        "DB_DATABASE": "qh",
    },
    "mysql": {
        "DB_KIND": "mysql",
        "DB_HOST": "127.0.0.1",
        "DB_PORT": "53306",
        "DB_USER": "qh",
        "DB_PASSWORD": "qh-dev-only",
        "DB_DATABASE": "qh",
    },
    "trino": {
        "DB_KIND": "trino",
        "DB_HOST": "127.0.0.1",
        "DB_PORT": "58080",
        "DB_USER": "qh",
        "DB_DATABASE": "memory",
        "DB_SCHEMA": "default",
    },
}

# Every setting the engine reads, so nothing leaks in from the shell.
ENGINE_KEYS = (
    "DB_KIND", "DB_URL", "DB_HOST", "DB_PORT", "DB_USER", "DB_PASSWORD", "DB_DATABASE",
    "DB_SCHEMA", "DB_SCHEME", "DB_SSLMODE", "DB_INSECURE", "DB_ALL_SCHEMAS",
    "TRINO_URL", "TRINO_HOST", "TRINO_PORT", "TRINO_USER", "TRINO_PASSWORD",
    "TRINO_CATALOG", "TRINO_SCHEMA", "TRINO_INSECURE",
    "SQL", "SQL_PATH", "FORMAT", "OUT_DIR", "NAME", "ZIP", "LIMIT", "BATCH_SIZE",
    "ROWS_PER_FILE", "RETRIES", "PROGRESS_MS", "DELIMITER", "ENCODING", "HEADER", "BOM",
    "NULL_TEXT", "JSONL", "SQL_TABLE", "SHEET", "DBF_CHAR_WIDTH", "DBF_ENCODING",
    "TARGET_CATALOG", "TARGET_SCHEMA", "TARGET_TABLE", "WRITE_MODE",
)

COLUMN_COUNT = 30
WIDE_ROWS = 500_000
DEFAULT_SQL = "SELECT * FROM wide_500k"


def parse_peak_rss(text: str) -> int | None:
    """Peak resident set size in bytes from `/usr/bin/time -l` output.

    macOS writes the number first and the label after it:

        1234567890  maximum resident set size

    so the label is matched at the end of the line, not the start.
    """
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.endswith("maximum resident set size"):
            fields = stripped.split()
            if fields:
                try:
                    return int(fields[0])
                except ValueError:
                    return None
    return None


def resolve_rust_binary(override: str | None) -> tuple[str, pathlib.Path]:
    """The Rust CLI to run, and the build profile it came from.

    An explicit `--binary` is reported by whatever its path says it is, so a
    hand-pointed build is never silently labelled release.
    """
    if override:
        path = pathlib.Path(override).resolve()
        if not path.is_file():
            raise SystemExit(f"--binary {override} is not a file")
        profile = "release" if "/release/" in str(path) else "debug"
        return profile, path
    for profile, path in RUST_BINARIES:
        if path.is_file():
            return profile, path
    raise SystemExit(
        "no Rust engine binary found. Run:\n"
        "  cargo build --release --bin queryhive-engine"
    )


def measure(kind: str, sql: str, limit: int, label: str,
            binary: str | None = None) -> dict:
    if kind not in CONNECTIONS:
        raise SystemExit(f"unknown kind {kind!r}; expected one of {sorted(CONNECTIONS)}")

    # The engine reads the settings below out of the environment, so the table
    # above is the single source of truth for what it connects to.
    env = dict(os.environ)
    env.update(CONNECTIONS[kind])
    env["SQL"] = sql
    env["LIMIT"] = str(limit)
    # No retries: a baseline that silently retried would report a number no user
    # would ever see, and a real failure should fail the measurement.
    env["RETRIES"] = "0"

    profile, path = resolve_rust_binary(binary)
    command = [TIME_BIN, "-l", str(path), "preview"]
    # The binary resolves its own paths; the cwd only has to be inside the
    # workspace so a stray relative write cannot land somewhere unrelated.
    cwd = ROOT

    load_before = os.getloadavg()

    started = time.monotonic()
    process = subprocess.Popen(
        command,
        cwd=str(cwd),
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
    )

    first_row_at: float | None = None
    connect_emitted_at: float | None = None
    rows_seen = 0
    done_at: float | None = None
    engine_elapsed_ms: int | None = None
    error_message: str | None = None

    assert process.stdout is not None
    for line in process.stdout:
        line = line.strip()
        if not line:
            continue
        now = time.monotonic()
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        name = event.get("event")
        if name == "step" and event.get("step") == "connect":
            # The engine emits this before it touches the network, so the gap
            # from here to the first `rows` is connect + submit + first page.
            # Anchoring on `columns` instead would measure almost nothing: that
            # event only fires once the first page already exists.
            if connect_emitted_at is None:
                connect_emitted_at = now
        elif name == "rows":
            if first_row_at is None:
                first_row_at = now
            rows_seen += len(event.get("data") or [])
        elif name == "done":
            done_at = now
            # The engine's own number: it stamps this right before it emits
            # `step connect`, so it covers connect + fetch + emit. `int()`
            # because the engine reports whole milliseconds.
            reported = event.get("elapsed_ms")
            if isinstance(reported, (int, float)):
                engine_elapsed_ms = int(reported)
        elif name == "error":
            error_message = event.get("message")

    stderr_text = process.stderr.read() if process.stderr else ""
    process.wait()
    finished = time.monotonic()
    peak_rss = parse_peak_rss(stderr_text)

    if process.returncode != 0 or error_message:
        raise SystemExit(
            f"{kind}: the engine failed (exit {process.returncode})\n"
            f"message: {error_message}\n"
            f"stderr tail:\n" + "\n".join(stderr_text.splitlines()[-15:])
        )

    # Throughput is measured between the first and the last row, so process and
    # connect time are not credited as fetch speed.
    fetch_seconds = (done_at - first_row_at) if (first_row_at and done_at) else 0.0
    load_after = os.getloadavg()
    record = {
        "label": label,
        "engine": "rust",
        "kind": kind,
        "sql": sql,
        "limit": limit,
        "rows": rows_seen,
        "columns": COLUMN_COUNT,
        "time_to_first_row_ms": round((first_row_at - started) * 1000, 1) if first_row_at else None,
        "time_to_first_row_after_connect_ms": (
            round((first_row_at - connect_emitted_at) * 1000, 1)
            if (first_row_at and connect_emitted_at)
            else None
        ),
        "total_ms": round((finished - started) * 1000, 1),
        "fetch_ms": round(fetch_seconds * 1000, 1),
        "elapsed_ms": engine_elapsed_ms,
        "rows_per_second": round(rows_seen / fetch_seconds) if fetch_seconds > 0 else None,
        "peak_rss_bytes": peak_rss,
        "peak_rss_mb": round(peak_rss / (1024 * 1024), 1) if peak_rss else None,
        # The load average at the time, because a contended machine reports its
        # own busyness as the engine's latency otherwise. Recorded before and
        # after so a run that started quiet and ended busy is visible as such.
        "load_avg_1m_before": round(load_before[0], 2),
        "load_avg_5m_before": round(load_before[1], 2),
        "load_avg_1m_after": round(load_after[0], 2),
        "load_avg_5m_after": round(load_after[1], 2),
        "ncpu": os.cpu_count(),
        "recorded_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
    }
    record["build"] = profile
    record["binary"] = str(path.relative_to(ROOT))
    # The shared schema: this harness measures the engine CLI, which is the fetch
    # family and the row-throughput axis.
    is_default = sql == DEFAULT_SQL and limit == WIDE_ROWS
    record.update({
        "bench": FETCH_BENCH,
        # The named scenario only for the default workload; any override is a
        # different workload and must not be read as the wide_500k number.
        "axis": 2 if is_default else None,
        "scenario": f"fetch-{kind}-wide-500k" if is_default else f"fetch-{kind}-custom",
        "app": "queryhive",
        "app_rev": app_revision(),
        "competitor_rev": None,
    })
    record.update(host_facts())
    return record


def append(result: dict) -> None:
    RESULTS.parent.mkdir(parents=True, exist_ok=True)
    with RESULTS.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(result, sort_keys=True) + "\n")


def load_results() -> list[dict]:
    if not RESULTS.exists():
        return []
    return [json.loads(line) for line in RESULTS.read_text(encoding="utf-8").splitlines() if line.strip()]


def human_bytes(value: int | None) -> str:
    if value is None:
        return "—"
    return f"{value / (1024 * 1024):,.0f} MB"


def human_ms(value: float | None) -> str:
    return "—" if value is None else f"{value:,.0f} ms"


def human_rate(value: int | None) -> str:
    return "—" if value is None else f"{value:,}"


def group_runs(results: list[dict]) -> dict[tuple, list[dict]]:
    """Group records by engine + kind + label + build, in the order appended.

    Grouping by label is what keeps two measurement sessions apart: the same
    engine measured on a quiet machine and on a busy one are different
    populations, and pooling them would make the spread describe the machine.
    """
    groups: dict[tuple, list[dict]] = {}
    for row in results:
        key = (
            # A record written before the engines were labelled has no `engine`
            # key; every one of those came from the Python arm, so that is the
            # default rather than a guess.
            row.get("engine", "python"),
            row["kind"],
            row.get("label", ""),
            row.get("build", ""),
        )
        groups.setdefault(key, []).append(row)
    return groups


def spread(values) -> tuple[float | None, float | None, float | None]:
    """Median, minimum and maximum of the numbers actually present."""
    numbers = sorted(value for value in values if isinstance(value, (int, float)))
    if not numbers:
        return None, None, None
    return statistics.median(numbers), numbers[0], numbers[-1]


def cell(values, fmt) -> str:
    """`median [min–max]`, or the bare median when every repeat agrees."""
    median, low, high = spread(values)
    if median is None:
        return "—"
    if low == high:
        return fmt(median)
    return f"{fmt(median)} [{fmt(low)}–{fmt(high)}]"


def newest_group(groups: dict[tuple, list[dict]], engine: str, kind: str):
    """The most recently appended group for one engine and one database."""
    matches = [
        (key, rows) for key, rows in groups.items() if key[0] == engine and key[1] == kind
    ]
    return matches[-1] if matches else None


# ---------------------------------------------------------------------------
# One record schema for every axis (performance-plan section 2 and 4.0.6)
# ---------------------------------------------------------------------------
#
# Every line of bench-results.jsonl is one record. The fetch harness above writes
# `bench = "fetch"` records; `bench_app.py` writes the others. A record written
# before the schema existed has no `bench` key and is read as "fetch": nothing in
# the file is ever rewritten.
#
#   bench            family: fetch | app | ffi | blackbox
#   axis             1..7, or a secondary name (sort-search, introspection, ffi-leak)
#   scenario         a name from AXIS_ROWS below, or a free name for a probe
#   app              queryhive | tablepro
#   app_rev          git revision of the app under test
#   competitor_rev   pinned TablePro revision (tablepro records)
#   hw_model, p_cores, e_cores, display_hz
#   server_image     {container: image digest}, when podman could tell
#   metrics          {name: {"median", "p95", "min", "max", "n"}}   (bench_app.py)
#   status           one of STATUSES, instead of metrics, when nothing was measured

FETCH_BENCH = "fetch"
APP_BENCHES = ("app", "ffi", "blackbox")
APPS = ("queryhive", "tablepro")
UNMEASURED = "[belum diukur]"
STATUSES = (
    UNMEASURED,
    "tidak diukur (izin OS)",
    "tidak diukur (butuh sudo)",
    "tidak mendukung",
)

# Axis key -> section title. Numbers are the axes of performance-plan section 2.
AXES = {
    1: "TTFR sampai baris pertama tergambar",
    2: "Baris/s ke grid",
    3: "Memori puncak",
    4: "Frame saat scroll",
    5: "Latensi ketikan",
    6: "Latensi cancel",
    7: "Cold start",
    "sort-search": "Sekunder: sort dan search",
    "introspection": "Sekunder: introspeksi 5.000 tabel",
    "ffi-leak": "Sekunder: leak lintas FFI",
}

# One row per scenario. `checks` are the absolute targets as
# (metric, stat, op, threshold); `rel` is the target against TablePro as
# (metric, stat, op, ratio) on QueryHive / TablePro, or None when the plan gives
# no TablePro number. Metric names carry their unit as a suffix: _ms, _bytes,
# _rows_per_s, _ms_per_s, _count.
AXIS_ROWS = (
    (1, "ttfr-s1-1k", "S1 hangat, cap 1.000",
     (("ttfr_ms", "median", "<=", 25), ("ttfr_ms", "p95", "<=", 40)),
     "p50 ≤ 25 ms, p95 ≤ 40 ms", ("ttfr_ms", "median", "<=", 0.5), "≤ 0,5×"),
    (1, "ttfr-s1-10k", "S1 hangat, cap 10.000",
     (("ttfr_ms", "median", "<=", 25), ("ttfr_ms", "p95", "<=", 40)),
     "p50 ≤ 25 ms, p95 ≤ 40 ms", ("ttfr_ms", "median", "<=", 0.5), "≤ 0,5×"),
    (1, "ttfr-s2-500k", "S2 cap 500.000",
     (("ttfr_ms", "p95", "<=", 50),),
     "p95 ≤ 50 ms (progresif)", ("ttfr_ms", "median", "<=", 0.1), "≤ 0,1×"),
    (1, "ttfr-s3-rtt30", "S3 RTT 30 ms (toxiproxy)",
     (("ttfr_ms", "median", "<=", 50),),
     "hangat ≤ 1 RTT + 20 ms (50 ms)", ("ttfr_ms", "median", "<=", 1.0), "≤ 1,0×"),
    (1, "ttfr-s4-first-run", "S4 Run pertama setelah app dibuka",
     (), "—", ("ttfr_ms", "median", "<=", 1.0), "≤ 1,0×"),
    (2, "rows-wide-500k", "`wide_500k` tanpa cap",
     (("rows_per_s", "median", ">=", 575_000),),
     "≥ 575.000 baris/s, atau ≥ 80% plafon bila lebih rendah",
     ("rows_per_s", "median", ">=", 1.5), "≥ 1,5×"),
    (2, "rows-lineitem-1m", "Trino `tpch.sf1.lineitem`, cap 1M",
     (("rows_per_s", "median", ">=", 575_000),),
     "≥ 575.000 baris/s, atau ≥ 80% plafon bila lebih rendah",
     ("rows_per_s", "median", ">=", 1.5), "≥ 1,5×"),
    (3, "mem-500k", "500k × 30",
     (("footprint_delta_bytes", "median", "<=", None),),
     "≤ anggaran store + 64 MB", ("footprint_delta_bytes", "median", "<=", 0.5), "≤ 0,5×"),
    (3, "mem-5m", "5M baris",
     (("footprint_delta_bytes", "median", "<=", None),),
     "≤ anggaran store + 64 MB", ("footprint_delta_bytes", "median", "<=", 0.5), "≤ 0,5×"),
    (4, "scroll-30x1m", "30 kolom × 1M baris, fling vertikal",
     (("hitch_ms_per_s", "median", "<=", 1), ("frame_p99_ms", "median", "<=", 8.3)),
     "hitch ≤ 1 ms/s; p99 frame ≤ 8,3 ms", ("hitch_ms_per_s", "median", "<=", 1.0), "hitch ≤ 1,0×"),
    (4, "scroll-500x10k", "500 kolom × 10k baris, horizontal + vertikal",
     (("hitch_ms_per_s", "median", "<=", 1), ("frame_p99_ms", "median", "<=", 8.3),
      ("render_ms", "median", "<=", 30)),
     "hitch ≤ 1 ms/s; p99 frame ≤ 8,3 ms; tergambar ≤ 30 ms",
     ("hitch_ms_per_s", "median", "<=", 1.0), "hitch ≤ 1,0×"),
    (5, "type-10k", "berkas 10k baris, mengetik di tengah",
     (("keystroke_main_p99_ms", "median", "<=", 4),),
     "main thread p99 ≤ 4 ms", ("input_to_photon_ms", "p95", "<=", 1.0), "photon p95 ≤ 1,0×"),
    (5, "type-2m", "berkas 2M karakter, mengetik di tengah",
     (("keystroke_main_p99_ms", "median", "<=", 8),),
     "main thread p99 ≤ 8 ms", ("input_to_photon_ms", "p95", "<=", 1.0), "photon p95 ≤ 1,0×"),
    (6, "cancel-pg-sleep", "`pg_sleep(30)`",
     (("cancel_ms", "p95", "<=", 100),), "p95 ≤ 100 ms", ("cancel_ms", "p95", "<=", 1.0), "≤ 1,0×"),
    (6, "cancel-mysql-sleep", "`SLEEP(30)`",
     (("cancel_ms", "p95", "<=", 100),), "p95 ≤ 100 ms", ("cancel_ms", "p95", "<=", 1.0), "≤ 1,0×"),
    (6, "cancel-stream-wide", "stream `wide_500k` di tengah",
     (("cancel_ms", "p95", "<=", 100),), "p95 ≤ 100 ms", ("cancel_ms", "p95", "<=", 1.0), "≤ 1,0×"),
    (6, "cancel-trino-heavy", "agregasi berat Trino",
     (("cancel_ms", "p95", "<=", 300),), "p95 ≤ 300 ms", ("cancel_ms", "p95", "<=", 1.0), "≤ 1,0×"),
    (7, "launch-warm", "launch sampai frame interaktif, hangat",
     (("launch_ms", "median", "<=", 400),), "≤ 400 ms", ("launch_ms", "median", "<=", 1.0), "≤ 1,0×"),
    (7, "launch-cold", "launch sampai frame interaktif, dingin (setelah `purge`)",
     (("launch_ms", "median", "<=", 1000),), "≤ 1 dtk", ("launch_ms", "median", "<=", 1.0), "≤ 1,0×"),
    ("sort-search", "sort-numeric-500k", "sort numerik in-memory, 500k",
     (("sort_ms", "median", "<=", 100),), "≤ 100 ms, off-main", None, "—"),
    ("sort-search", "sort-text-500k", "sort teks in-memory, 500k",
     (("sort_ms", "median", "<=", 300),), "≤ 300 ms, off-main", None, "—"),
    ("introspection", "introspect-5000", "introspeksi 5.000 tabel",
     (("duration_ms", "median", "<=", 1000),), "< 1 dtk", None, "—"),
    ("ffi-leak", "ffi-leak", "100× buka/tutup tab, `leaks`",
     (("leak_count", "median", "<=", 0),), "nol leak", None, "—"),
)

# The "Perbandingan dengan target section 6" rows that were "[belum diukur]": the
# label as the report prints it, the scenario that answers it, and its target.
SECTION6_ROWS = (
    ("Scroll grid 60 fps", "scroll-30x1m", "frame_p99_ms", "median", "<=", 16.7,
     "p99 frame ≤ 16,7 ms (60 fps)"),
    ("Cold start < 1 dtk", "launch-cold", "launch_ms", "median", "<=", 1000, "< 1 dtk"),
    ("Introspeksi 5.000 tabel < 1 dtk", "introspect-5000", "duration_ms", "median", "<=", 1000,
     "< 1 dtk"),
    ("Pembatalan < 500 ms", "cancel-pg-sleep", "cancel_ms", "p95", "<=", 500, "p95 < 500 ms"),
    ("Nol leak lintas FFI", "ffi-leak", "leak_count", "median", "<=", 0, "nol leak"),
)


def percentile(values, fraction: float) -> float | None:
    """Linear-interpolation percentile (numpy's default), `fraction` in 0..1."""
    numbers = sorted(v for v in values if isinstance(v, (int, float)) and not isinstance(v, bool))
    if not numbers:
        return None
    position = fraction * (len(numbers) - 1)
    low = int(position)
    high = min(low + 1, len(numbers) - 1)
    return numbers[low] + (numbers[high] - numbers[low]) * (position - low)


def summarize(values) -> dict | None:
    """The stored shape of one metric: median, p95, min, max and n."""
    numbers = [v for v in values if isinstance(v, (int, float)) and not isinstance(v, bool)]
    if not numbers:
        return None
    return {
        "median": percentile(numbers, 0.5),
        "p95": percentile(numbers, 0.95),
        "min": min(numbers),
        "max": max(numbers),
        "n": len(numbers),
    }


def _sysctl(name: str) -> str | None:
    try:
        done = subprocess.run(["sysctl", "-n", name], capture_output=True, text=True, timeout=5)
    except (OSError, subprocess.SubprocessError):
        return None
    return done.stdout.strip() or None if done.returncode == 0 else None


def _sysctl_int(name: str) -> int | None:
    text = _sysctl(name)
    return int(text) if text and text.isdigit() else None


def server_images() -> dict | None:
    """`{container: image digest}` for the dev containers podman knows about."""
    try:
        done = subprocess.run(
            ["podman", "inspect", "qh-postgres", "qh-mysql", "qh-trino",
             "--format", "{{.Name}} {{.ImageName}} {{.ImageDigest}}"],
            capture_output=True, text=True, timeout=15,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    images = {}
    for line in done.stdout.splitlines():
        parts = line.split()
        if len(parts) == 3:
            images[parts[0]] = f"{parts[1]}@{parts[2]}"
    return images or None


def app_revision() -> str | None:
    """Short git revision of this tree, `-dirty` when it has uncommitted changes."""
    try:
        rev = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=str(ROOT),
                             capture_output=True, text=True, timeout=10)
        dirty = subprocess.run(["git", "status", "--porcelain"], cwd=str(ROOT),
                               capture_output=True, text=True, timeout=30)
    except (OSError, subprocess.SubprocessError):
        return None
    if rev.returncode != 0 or not rev.stdout.strip():
        return None
    return rev.stdout.strip() + ("-dirty" if dirty.stdout.strip() else "")


def host_facts(display_hz: int | None = None) -> dict:
    """The machine facts every record carries; None where the machine will not say."""
    return {
        "hw_model": _sysctl("hw.model"),
        "p_cores": _sysctl_int("hw.perflevel0.physicalcpu"),
        "e_cores": _sysctl_int("hw.perflevel1.physicalcpu"),
        "display_hz": display_hz,
        "server_image": server_images(),
    }


def fetch_records(results: list[dict]) -> list[dict]:
    """The records the fetch section is built from; old ones have no `bench`."""
    return [r for r in results if r.get("bench", FETCH_BENCH) == FETCH_BENCH]


def latest_app_record(results: list[dict], scenario: str, app: str) -> dict | None:
    """Newest non-fetch record for one scenario and app, in the order appended."""
    match = None
    for row in results:
        if (row.get("bench", FETCH_BENCH) in APP_BENCHES
                and row.get("scenario") == scenario and row.get("app", "queryhive") == app):
            match = row
    return match


def fmt_metric(name: str, value: float) -> str:
    """A number with the unit its metric name carries."""
    if name.endswith("_bytes"):
        return f"{value / (1024 * 1024):,.1f} MB"
    if name.endswith("rows_per_s"):
        return f"{value:,.0f} baris/s"
    if name.endswith("_ms_per_s"):
        return f"{value:,.2f} ms/s"
    if name.endswith("_ms"):
        return f"{value:,.1f} ms"
    if name.endswith("_count"):
        return f"{value:,.0f}"
    return f"{value:,.2f}"


def metric_stat(record: dict | None, metric: str, stat: str) -> float | None:
    if not record:
        return None
    entry = (record.get("metrics") or {}).get(metric)
    value = entry.get(stat) if isinstance(entry, dict) else None
    return value if isinstance(value, (int, float)) else None


def _holds(op: str, value: float, threshold: float) -> bool:
    return value <= threshold if op == "<=" else value >= threshold


def side_cell(record: dict | None, checks) -> str:
    """One app's cell: its headline numbers, or the status it was recorded with."""
    if record is None:
        return UNMEASURED
    shown = []
    for metric, stat, _op, _threshold in checks:
        value = metric_stat(record, metric, stat)
        if value is not None:
            n = record["metrics"][metric].get("n")
            shown.append(f"{metric} {stat} {fmt_metric(metric, value)}"
                         + (f" (n={n})" if n else ""))
    if shown:
        return "; ".join(shown)
    status = record.get("status")
    return status if status in STATUSES else UNMEASURED


def _threshold(record: dict, metric: str, op: str, threshold, axis) -> float | None:
    """The absolute threshold, where it depends on something the record carries."""
    if axis == 2:
        # Plan section 2: the target is 575k rows/s, or 80% of the same-day COPY
        # ceiling when that ceiling is lower, i.e. when ceiling < 575k / 0.8.
        ceiling = metric_stat(record, "ceiling_rows_per_s", "median")
        if ceiling is not None and ceiling < threshold / 0.8:
            return 0.8 * ceiling
        return threshold
    if metric == "footprint_delta_bytes" and threshold is None:
        budget = metric_stat(record, "budget_bytes", "median")
        return budget + 64 * 1024 * 1024 if budget is not None else None
    return threshold


def shown_checks(row) -> tuple:
    """What a scenario's cells display: its absolute checks, and the metric the
    TablePro comparison uses when that is not one of them, so a verdict never
    rests on a number neither cell shows."""
    _axis, _scenario, _label, checks, _target, rel, _rel_text = row
    shown = tuple(checks)
    if rel and not any(c[0] == rel[0] and c[1] == rel[1] for c in shown):
        shown += (rel,)
    return shown


def axis_verdict(row, qh: dict | None, tp: dict | None) -> str:
    axis, _scenario, _label, checks, _target, rel, _rel_text = row
    if not any(metric_stat(qh, c[0], c[1]) is not None for c in shown_checks(row)):
        return UNMEASURED
    parts = []
    if checks:
        problems, missing, ungraded = [], [], False
        for metric, stat, op, threshold in checks:
            value = metric_stat(qh, metric, stat)
            if value is None:
                missing.append(metric)
                continue
            limit = _threshold(qh, metric, op, threshold, axis)
            if limit is None:
                ungraded = True
            elif not _holds(op, value, limit):
                side = "di atas batas ≤" if op == "<=" else "di bawah batas ≥"
                problems.append(f"{metric} {stat} {fmt_metric(metric, value)} "
                                f"{side} {fmt_metric(metric, limit)}")
        if problems:
            parts.append("Belum memenuhi — " + "; ".join(problems))
            if missing:
                parts.append(f"sisanya {', '.join(missing)} {UNMEASURED}")
        elif missing:
            # Never "Memenuhi" while a check is unmeasured.
            parts.append(f"Sebagian terukur — {', '.join(missing)} {UNMEASURED}")
        elif ungraded:
            parts.append("Terukur — anggaran store belum dicatat (`budget_bytes`)")
        else:
            parts.append("Memenuhi target absolut")
    if rel:
        metric, stat, op, ratio = rel
        mine, theirs = metric_stat(qh, metric, stat), metric_stat(tp, metric, stat)
        if mine is not None and theirs:
            got = mine / theirs
            parts.append(f"vs TablePro {got:,.2f}× "
                         f"({'lolos' if _holds(op, got, ratio) else 'gagal'} {op} {ratio:g}×)")
        else:
            parts.append("vs TablePro " + (UNMEASURED if tp is None else "tanpa pembanding"))
    return "; ".join(parts)


def axis_sections(results: list[dict]) -> list[str]:
    """One section per axis: target, QueryHive, TablePro, verdict."""
    lines: list[str] = []
    for axis, title in AXES.items():
        heading = f"## Sumbu {axis}: {title}" if isinstance(axis, int) else f"## {title}"
        lines += [heading, ""]
        lines += ["| Skenario | Target | QueryHive | TablePro | Verdict |", "|---|---|---|---|---|"]
        for row in (r for r in AXIS_ROWS if r[0] == axis):
            _axis, scenario, label, _checks, target, rel, rel_text = row
            qh = latest_app_record(results, scenario, "queryhive")
            tp = latest_app_record(results, scenario, "tablepro")
            full_target = "; ".join(
                part for part in (target if target != "—" else "",
                                  f"vs TablePro {rel_text}" if rel else "") if part) or "—"
            cells = shown_checks(row)
            tp_cell = side_cell(tp, cells) if rel else "—"
            lines.append(
                f"| `{scenario}` — {label} | {full_target} | {side_cell(qh, cells)} "
                f"| {tp_cell} | {axis_verdict(row, qh, tp)} |"
            )
        lines.append("")
    return lines


def section6_cells(results: list[dict], label: str):
    """(target, value, status) for a section 6 row, or None when nothing is recorded."""
    for name, scenario, metric, stat, op, threshold, text in SECTION6_ROWS:
        if name != label:
            continue
        value = metric_stat(latest_app_record(results, scenario, "queryhive"), metric, stat)
        if value is None:
            return None
        ok = _holds(op, value, threshold)
        return text, fmt_metric(metric, value), ("Memenuhi" if ok else "Belum memenuhi") \
            + f" — {fmt_metric(metric, value)}"
    return None


# Prose that is measured once by hand-run builds, not by this harness. Kept here so a
# regenerated report keeps it instead of dropping it.
PANIC_UNWIND_SECTION = '''\
## Ongkos `panic = "unwind"` pada ukuran artefak

ADR-0009 memilih `panic = "unwind"` supaya panic dari data server tidak menjatuhkan aplikasi, dan
konsekuensinya dijanjikan dicatat sebagai angka. Ini angkanya, diukur 24 Sep 2026 dengan
membangun `qh-ffi` dua kali dan mengganti satu baris di `Cargo.toml` (`[profile.release]`).
Profil lain identik: `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`.

| Artefak | `panic = "unwind"` | `panic = "abort"` | Selisih |
|---|---|---|---|
| `libqh_ffi.a` (arsip yang ditautkan app) | 131,52 MiB | 121,59 MiB | **9,93 MiB** |
| `libqh_ffi.dylib` (dibaca generator) | 9,54 MiB | 8,15 MiB | **1,39 MiB** |
| `QueryHive` (binary app yang dikirim) | 16,32 MiB | 14,63 MiB | **1,69 MiB** |

Yang menentukan bagi pengguna adalah baris terakhir: **unwinding table berbiaya 1,69 MiB**, sekitar
10% dari binary app. Baris arsipnya jauh lebih besar dan sama sekali tidak relevan untuk distribusi,
sebab app tidak pernah mengirim arsip itu — kode yang dipakai disalin ke binary app, dan sisanya
dibuang. Angka 9,93 MiB itu adalah selisih blob mentah, bukan ukuran yang sampai ke siapa pun.

Catatan cara mengukurnya: tiga angka di atas diambil dari build yang sama sekali berbeda (bukan
inkremental) untuk kedua nilai, karena `panic` mengubah seluruh graf. Setelah pengukuran, `Cargo.toml`
dikembalikan ke `unwind` dan `app/build.sh` dijalankan ulang, jadi binary di `app/dist/` cocok
dengan profil yang berlaku.
'''


def write_report(results: list[dict]) -> None:
    groups = group_runs(fetch_records(results))

    lines = [
        "# Benchmarks",
        "",
        "> Setiap angka di berkas ini dihasilkan oleh `deploy/dev/bench_fetch.py`; tidak ada yang",
        "> ditulis dengan tangan. Hasil mentah ada di `deploy/dev/bench-results.jsonl`.",
        "> Kebijakan: angka yang belum diukur ditulis sebagai **[belum diukur]**, bukan diperkirakan.",
        "",
        "## Cara mengukur",
        "",
        "```bash",
        "deploy/dev/up.sh all",
        "cargo build --release --bin queryhive-engine",
        "python3 deploy/dev/bench_fetch.py --kind postgres --label <sesi> --repeat 3",
        "python3 deploy/dev/bench_fetch.py --kind mysql    --label <sesi> --repeat 3",
        "python3 deploy/dev/bench_fetch.py --report-only",
        "```",
        "",
        "Baseline Python di bawah ini adalah **rekaman beku**: barisnya sudah ada di",
        "`deploy/dev/bench-results.jsonl` sebelum mesin Python dihapus dari pohon, dan tidak bisa",
        "direkam ulang. Harness hanya bisa menjalankan engine Rust.",
        "",
        "Harness menjalankan engine lewat satu jalur: perintah `preview` pada CLI `qh-ffi`, dengan",
        "nama setelan yang sama dibaca dari environment. Setiap baris stdout-nya diberi cap waktu saat",
        "tiba. Dua angka time-to-first-row dilaporkan karena keduanya berguna dan hanya salah satunya",
        "cocok untuk target §6:",
        "",
        "- **dari connect** — jarak dari event `step connect` ke event `rows` pertama. Engine",
        "  mengirim `step connect` sebelum menyentuh jaringan, jadi ini connect + submit + halaman",
        "  pertama. Inilah yang dibandingkan dengan target \u00a76 (< 200 ms sejak server mulai",
        "  mengirim hasil).",
        "- **dari start** — jarak dari proses dijalankan. Ini yang dirasakan pengguna, termasuk",
        "  waktu start interpreter.",
        "",
        "Menganchor pada event `columns` akan salah: event itu baru muncul setelah halaman pertama",
        "sudah ada, sehingga selisihnya hampir nol dan menyembunyikan seluruh waktu tunggu.",
        "",
        "**elapsed_ms** adalah angka engine sendiri, diambil dari event `done`. Engine menstempelnya",
        "tepat sebelum mengirim `step connect`, jadi cakupannya sama di setiap baris rekaman",
        "(connect + fetch + emit) dan tidak memuat waktu start proses — inilah satu-satunya angka",
        "yang mengukur rentang identik tanpa penjadwalan harness di tengahnya. **Throughput**",
        "dihitung antara baris pertama dan baris terakhir, sehingga waktu start dan connect tidak",
        "ikut dihitung sebagai kecepatan fetch. **Peak RSS** dibaca dari `/usr/bin/time -l`, yang",
        "melaporkan puncak satu proses anak, bukan angka kumulatif.",
        "",
        "**Rata-rata beban mesin** dicatat pada setiap run (`load_avg_1m_before`/`_after`): angka",
        "yang diambil saat mesin sibuk menggambarkan mesinnya, bukan engine-nya.",
        "",
        "Kondisi uji: `SELECT * FROM wide_500k` (30 kolom), tanpa retry, database lokal di container.",
        "Trino tidak diukur: katalog `memory` di container dev tidak punya `wide_500k`.",
        "",
    ]

    headers = (
        "| Kind | Label | Build | n | Baris | Baris pertama (dari connect) | Baris pertama (dari start) "
        "| Total (proses) | elapsed_ms (engine) | Fetch | Throughput | Peak RSS | load 1m |",
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|",
    )

    for engine, title, missing in (
        (
            "python",
            "## Baseline engine Python (rekaman beku)",
            "**[belum diukur]** — tidak ada baris baseline Python di "
            "`deploy/dev/bench-results.jsonl`. Baris seperti itu tidak bisa direkam ulang: mesin "
            "Python sudah dihapus dari pohon, jadi ini satu-satunya cara angka baseline ada.",
        ),
        (
            "rust",
            "## Engine Rust",
            "**[belum diukur]** — belum ada jalannya yang terekam. Bangun `--release` lalu jalankan "
            "harness di atas.",
        ),
    ):
        lines += [title, ""]
        members = [(key, rows) for key, rows in groups.items() if key[0] == engine]
        if not members:
            lines += [missing, ""]
            continue
        lines += list(headers)
        for (_, kind, label, profile), rows in members:
            lines.append(
                f"| {kind} | {label} | {profile or '—'} | {len(rows)} | {rows[0]['rows']:,} "
                f"| {cell((r.get('time_to_first_row_after_connect_ms') for r in rows), human_ms)} "
                f"| {cell((r.get('time_to_first_row_ms') for r in rows), human_ms)} "
                f"| {cell((r.get('total_ms') for r in rows), human_ms)} "
                f"| {cell((r.get('elapsed_ms') for r in rows), human_ms)} "
                f"| {cell((r.get('fetch_ms') for r in rows), human_ms)} "
                f"| {cell((r.get('rows_per_second') for r in rows), human_rate)} baris/s "
                f"| {cell((r.get('peak_rss_bytes') for r in rows), human_bytes)} "
                f"| {cell((r.get('load_avg_1m_before') for r in rows), lambda v: f'{v:.2f}')} |"
            )
        lines.append("")
        lines.append(
            "Setiap sel adalah **median [min–max]** dari n repeat; satu angka saja berarti semua "
            "repeat sepakat. Statistik yang dipakai median, bukan yang tercepat: pada mesin yang "
            "dipakai bersama, satu run yang kebetulan sepi bukan kecepatan engine."
        )
        lines.append("")

    lines += [
        "## Perbandingan dengan target §6",
        "",
        "| Metrik | Target | Baseline Python | Rust | Status |",
        "|---|---|---|---|---|",
    ]
    def stat(entry, key, fmt) -> tuple[str, float | None]:
        """The grouped cell for one metric, and the median behind it."""
        if entry is None:
            return "—", None
        median, _, _ = spread(row.get(key) for row in entry[1])
        return (cell((row.get(key) for row in entry[1]), fmt), median)

    # The comparison is against the newest recorded Python baseline and the newest
    # Rust group: the same frozen baseline every time, so a number compared here on
    # one day means what it meant the day before.
    py_pg = newest_group(groups, "python", "postgres")
    rs_pg = newest_group(groups, "rust", "postgres")

    rate_cell, py_rate = stat(py_pg, "rows_per_second", human_rate)
    _, rs_rate = stat(rs_pg, "rows_per_second", human_rate)
    ttfr_cell, _ = stat(py_pg, "time_to_first_row_after_connect_ms", human_ms)
    _, rs_ttfr = stat(rs_pg, "time_to_first_row_after_connect_ms", human_ms)
    rss_cell, _ = stat(py_pg, "peak_rss_bytes", human_bytes)
    _, rs_rss = stat(rs_pg, "peak_rss_bytes", human_bytes)

    def verdict(target_met: bool | None, detail: str) -> str:
        if target_met is None:
            return "[belum diukur] — menunggu kedua sisi terukur"
        return f"{'Memenuhi' if target_met else 'Belum memenuhi'} — {detail}"

    rate_verdict = None
    if py_rate and rs_rate:
        rate_verdict = rs_rate >= py_rate * 5
    else:
        rs_rate = None
    ttfr_verdict = None if rs_ttfr is None else rs_ttfr < 200
    rss_verdict = None if rs_rss is None else rs_rss < 800 * 1024 * 1024

    lines.append(
        "| Throughput fetch | ≥ 5× baseline Python | "
        + (f"{rate_cell} baris/s" if py_rate else "—")
        + " | "
        + (f"{rs_rate:,.0f} baris/s (median)" if rs_rate else "[belum diukur]")
        + " | "
        + verdict(
            rate_verdict,
            f"{rs_rate / py_rate:.2f}× baseline Python"
            if (rate_verdict is not None and py_rate)
            else "tanpa basis pembanding",
        )
        + " |"
    )
    lines.append(
        "| Time-to-first-row | < 200 ms sejak server mulai mengirim hasil | "
        + ttfr_cell
        + " | "
        + (human_ms(rs_ttfr) if rs_ttfr is not None else "[belum diukur]")
        + " | "
        + verdict(ttfr_verdict, f"{rs_ttfr:,.0f} ms" if rs_ttfr is not None else "—")
        + " |"
    )
    lines.append(
        "| Memori proses (500k × 30) | < 800 MB | "
        + rss_cell
        + " | "
        + (human_bytes(rs_rss) if rs_rss is not None else "[belum diukur]")
        + " | "
        + verdict(rss_verdict, human_bytes(rs_rss) if rs_rss is not None else "—")
        + " |"
    )
    for metric in ("Scroll grid 60 fps", "Cold start < 1 dtk", "Introspeksi 5.000 tabel < 1 dtk",
                   "Pembatalan < 500 ms", "Nol leak lintas FFI"):
        filled = section6_cells(results, metric)
        if filled is None:
            lines.append(f"| {metric} | — | — | — | [belum diukur] |")
        else:
            target, value, status = filled
            lines.append(f"| {metric} | {target} | — | {value} | {status} |")
    lines.append("")

    lines += axis_sections(results)
    lines += PANIC_UNWIND_SECTION.split("\n")

    lines += ["## Temuan", ""]
    findings: list[str] = []
    for (engine, kind, label, profile), rows in groups.items():
        # One finding per group, off the median: per-repeat findings would say the
        # same thing three times and invite reading a single run as the engine.
        rate, _, _ = spread(row.get("rows_per_second") for row in rows)
        first_row, _, _ = spread(
            row.get("time_to_first_row_after_connect_ms") for row in rows
        )
        rss_mb, _, _ = spread(row.get("peak_rss_mb") for row in rows)
        where = f"{engine}/{kind} ({label}{', ' + profile if profile else ''}, n={len(rows)})"

        if rss_mb is not None and rss_mb > 800:
            findings.append(
                f"- **{where}: memori melewati target §6.** {rss_mb:,.0f} MB untuk 500k × 30, "
                "sedangkan targetnya < 800 MB."
            )
        if first_row is not None and first_row > 200:
            findings.append(
                f"- **{where}: baris pertama {first_row:,.0f} ms setelah connect**, sementara "
                "targetnya < 200 ms."
            )
        if rate is not None:
            findings.append(
                f"- {where}: {rate:,.0f} baris/s (median); target §6 berarti "
                f"≥ {rate * 5:,.0f} baris/s."
            )
    if not findings:
        findings.append("- Belum ada hasil untuk dianalisis.")
    lines += findings
    lines.append("")

    REPORT.parent.mkdir(parents=True, exist_ok=True)
    REPORT.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kind", choices=sorted(CONNECTIONS), default="postgres")
    parser.add_argument("--sql", default=DEFAULT_SQL)
    parser.add_argument("--limit", type=int, default=WIDE_ROWS)
    parser.add_argument("--label", default="rust-release")
    parser.add_argument("--binary", default=None,
                        help="the CLI to run; defaults to target/release, then target/debug")
    parser.add_argument("--repeat", type=int, default=1,
                        help="how many times to repeat the measurement, one record per run")
    parser.add_argument("--report-only", action="store_true")
    parser.add_argument("--no-report", action="store_true",
                        help="append the record without rewriting docs/benchmarks.md")
    args = parser.parse_args(argv)

    if args.report_only and args.no_report:
        raise SystemExit("--report-only and --no-report are mutually exclusive")
    if args.repeat < 1:
        raise SystemExit("--repeat must be at least 1")

    if not args.report_only:
        for index in range(args.repeat):
            result = measure(
                args.kind, args.sql, args.limit, args.label, binary=args.binary,
            )
            append(result)
            print(json.dumps(result, indent=2, sort_keys=True))
            if index + 1 < args.repeat:
                print(f"--- repeat {index + 2} of {args.repeat} ---")

    results = load_results()
    if not args.no_report:
        write_report(results)
        print(f"report regenerated: {REPORT.relative_to(ROOT)}")
    print(f"{len(results)} run(s) recorded in {RESULTS.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
