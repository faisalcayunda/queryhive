#!/usr/bin/env python3
"""Measure what the engine actually does, so the performance targets have a
baseline to be compared against.

    python3 deploy/dev/bench_fetch.py --kind postgres --label baseline-python
    python3 deploy/dev/bench_fetch.py --engine rust --kind postgres --label rust-release
    python3 deploy/dev/bench_fetch.py --report-only

How to reproduce this
---------------------
Both engines run through this one harness, against the same fixture, with the
same statement, on the same machine. A number exists only if a run produced it.

.. code-block:: bash

    # 1. the fixture databases (postgres 55432, mysql 53306)
    deploy/dev/up.sh all

    # 2. the Python engine, three repeats per database
    python3 deploy/dev/bench_fetch.py --engine python --kind postgres \\
        --label python-<date> --repeat 3
    python3 deploy/dev/bench_fetch.py --engine python --kind mysql \\
        --label python-<date> --repeat 3

    # 3. the Rust engine, the same repeats and the same statement
    cargo build --release --bin queryhive-engine
    python3 deploy/dev/bench_fetch.py --engine rust --kind postgres \\
        --label rust-release-<date> --repeat 3
    python3 deploy/dev/bench_fetch.py --engine rust --kind mysql \\
        --label rust-release-<date> --repeat 3

    # 4. regenerate docs/benchmarks.md from the appended records
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

The comparison is honest by construction: both engines are asked for the same
statement, against the same server, on the same machine. The Python engine is
measured through its own `preview` command; the Rust one through the equivalent
`qh-ffi` CLI, which exists for exactly this reason (blueprint section 4.5). Both
read the same settings from the environment, so one `CONNECTIONS` table drives
both.

Two timing numbers per run, and they are not the same thing:

* the harness's own wall clock, timestamped per stdout line — `total_ms`,
  `fetch_ms`, `time_to_first_row_ms`;
* `elapsed_ms`, which the engine reports in its `done` event. Both engines stamp
  it immediately before emitting `step connect`, so it spans connect + fetch +
  emit and excludes process start. It is the only number that measures the same
  span on both sides without the harness's own scheduling in the middle, which is
  why it is reported beside the wall clock rather than instead of it.

Peak RSS is per child process, read from `/usr/bin/time -l`. A run also records
the machine's load average, because a number taken while four other builds are
running describes the machine, not the engine.
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
ENGINE = ROOT / "app" / "engine" / "queryhive_engine.py"
ENGINE_PYTHON = ROOT / "app" / ".engine" / "python" / "bin" / "python3"
ENGINE_SITE = ROOT / "app" / ".engine" / "site-packages"
TIME_BIN = "/usr/bin/time"

# The Rust engine's CLI, by build profile. Release first: it is the profile the
# app ships and the only fair comparison against a CPython that is itself an
# optimised build. A debug binary is still measurable — the golden harness runs
# one — but it is recorded with its profile so a debug number is never read as
# the engine's speed.
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


def measure(kind: str, sql: str, limit: int, label: str, engine: str = "python",
            binary: str | None = None) -> dict:
    if kind not in CONNECTIONS:
        raise SystemExit(f"unknown kind {kind!r}; expected one of {sorted(CONNECTIONS)}")

    # Both engines read the same names out of the environment, so the settings
    # table above is the single source of truth for either side.
    env = {key: value for key, value in os.environ.items() if not key.startswith("PYTHON")}
    env.update(CONNECTIONS[kind])
    env["SQL"] = sql
    env["LIMIT"] = str(limit)
    # No retries: a baseline that silently retried would report a number no user
    # would ever see, and a real failure should fail the measurement.
    env["RETRIES"] = "0"

    profile = ""
    if engine == "python":
        if not ENGINE_PYTHON.exists():
            raise SystemExit(
                f"the bundled engine is missing ({ENGINE_PYTHON}).\n"
                "Run ./app/build.sh once, or point ENGINE_PYTHON at an interpreter "
                "that has the engine's requirements installed."
            )
        env["PYTHONPATH"] = str(ENGINE_SITE)
        env["PYTHONNOUSERSITE"] = "1"
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        command = [TIME_BIN, "-l", str(ENGINE_PYTHON), "-s", "-u", str(ENGINE), "preview"]
        cwd = ROOT / "app"
    elif engine == "rust":
        profile, path = resolve_rust_binary(binary)
        command = [TIME_BIN, "-l", str(path), "preview"]
        # The Rust binary resolves its own paths; the cwd only has to be inside the
        # workspace so a stray relative write cannot land somewhere unrelated.
        cwd = ROOT
    else:
        raise SystemExit(f"unknown engine {engine!r}; expected 'python' or 'rust'")

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
            # The engine's own number: both engines stamp it right before they
            # emit `step connect`, so it covers connect + fetch + emit on either
            # side. `int()` because the engines report whole milliseconds.
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
        "engine": engine,
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
    if engine == "rust":
        record["build"] = profile
        record["binary"] = str(path.relative_to(ROOT))
    else:
        record["python"] = subprocess.run(
            [str(ENGINE_PYTHON), "-c", "import sys; print(sys.version.split()[0])"],
            capture_output=True, text=True,
        ).stdout.strip()
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


def write_report(results: list[dict]) -> None:
    groups = group_runs(results)

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
        "python3 deploy/dev/bench_fetch.py --engine python --kind postgres --label <sesi> --repeat 3",
        "python3 deploy/dev/bench_fetch.py --engine python --kind mysql    --label <sesi> --repeat 3",
        "cargo build --release --bin queryhive-engine",
        "python3 deploy/dev/bench_fetch.py --engine rust   --kind postgres --label <sesi> --repeat 3",
        "python3 deploy/dev/bench_fetch.py --engine rust   --kind mysql    --label <sesi> --repeat 3",
        "python3 deploy/dev/bench_fetch.py --report-only",
        "```",
        "",
        "Kedua engine dijalankan lewat harness yang sama, membaca nama setelan yang sama dari",
        "environment, dengan perintah `preview` yang setara. Setiap baris stdout-nya diberi cap waktu",
        "saat tiba. Dua angka time-to-first-row dilaporkan karena keduanya berguna dan hanya salah",
        "satunya cocok untuk target §6:",
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
        "**elapsed_ms** adalah angka engine sendiri, diambil dari event `done`. Kedua engine",
        "menstempelnya tepat sebelum mengirim `step connect`, jadi cakupannya sama di kedua sisi",
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
            "## Baseline engine Python",
            "**[belum diukur]** — belum ada hasil. Jalankan harness di atas dengan container yang "
            "sudah menyala.",
        ),
        (
            "rust",
            "## Engine Rust",
            "**[belum diukur]** — belum ada jalannya yang terekam. Bangun `--release` lalu jalankan "
            "harness di atas dengan `--engine rust`.",
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

    # The comparison is against the newest recorded group per engine, so a
    # freshly measured pair is compared rather than an older day's baseline.
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
        lines.append(f"| {metric} | — | — | — | [belum diukur] |")
    lines.append("")

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
    parser.add_argument("--engine", choices=("python", "rust"), default="python")
    parser.add_argument("--kind", choices=sorted(CONNECTIONS), default="postgres")
    parser.add_argument("--sql", default="SELECT * FROM wide_500k")
    parser.add_argument("--limit", type=int, default=WIDE_ROWS)
    parser.add_argument("--label", default="baseline-python")
    parser.add_argument("--binary", default=None,
                        help="the Rust CLI to run; defaults to target/release, then target/debug")
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
                args.kind, args.sql, args.limit, args.label,
                engine=args.engine, binary=args.binary,
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
