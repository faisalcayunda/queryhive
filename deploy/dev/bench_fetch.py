#!/usr/bin/env python3
"""Measure what the engine actually does, so the performance targets have a
baseline to be compared against.

    python3 deploy/dev/bench_fetch.py --kind postgres --label baseline-python
    python3 deploy/dev/bench_fetch.py --report-only

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
measured through its own `preview` command; the Rust engine will be measured
through the equivalent `qh-ffi` CLI, which exists for exactly this reason
(blueprint section 4.5).
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
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


def measure(kind: str, sql: str, limit: int, label: str) -> dict:
    if not ENGINE_PYTHON.exists():
        raise SystemExit(
            f"the bundled engine is missing ({ENGINE_PYTHON}).\n"
            "Run ./app/build.sh once, or point ENGINE_PYTHON at an interpreter "
            "that has the engine's requirements installed."
        )
    if kind not in CONNECTIONS:
        raise SystemExit(f"unknown kind {kind!r}; expected one of {sorted(CONNECTIONS)}")

    env = {key: value for key, value in os.environ.items() if not key.startswith("PYTHON")}
    env.update(CONNECTIONS[kind])
    env["PYTHONPATH"] = str(ENGINE_SITE)
    env["PYTHONNOUSERSITE"] = "1"
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    env["SQL"] = sql
    env["LIMIT"] = str(limit)
    # No retries: a baseline that silently retried would report a number no user
    # would ever see, and a real failure should fail the measurement.
    env["RETRIES"] = "0"

    command = [
        TIME_BIN, "-l",
        str(ENGINE_PYTHON), "-s", "-u", str(ENGINE),
        "preview",
    ]

    started = time.monotonic()
    process = subprocess.Popen(
        command,
        cwd=str(ROOT / "app"),
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
    return {
        "label": label,
        "engine": "python",
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
        "rows_per_second": round(rows_seen / fetch_seconds) if fetch_seconds > 0 else None,
        "peak_rss_bytes": peak_rss,
        "peak_rss_mb": round(peak_rss / (1024 * 1024), 1) if peak_rss else None,
        "python": subprocess.run(
            [str(ENGINE_PYTHON), "-c", "import sys; print(sys.version.split()[0])"],
            capture_output=True, text=True,
        ).stdout.strip(),
        "recorded_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
    }


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


def write_report(results: list[dict]) -> None:
    python_runs = [row for row in results if row.get("engine") == "python"]
    rust_runs = [row for row in results if row.get("engine") == "rust"]

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
        "python3 deploy/dev/bench_fetch.py --kind postgres --label baseline-python",
        "python3 deploy/dev/bench_fetch.py --kind mysql    --label baseline-python",
        "python3 deploy/dev/bench_fetch.py --report-only",
        "```",
        "",
        "Engine dijalankan sebagai proses anak persis seperti aplikasi menjalankannya, setiap baris",
        "stdout-nya diberi cap waktu saat tiba. Dua angka time-to-first-row dilaporkan karena",
        "keduanya berguna dan hanya salah satunya cocok untuk target §6:",
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
        "**Throughput** dihitung antara baris pertama dan baris terakhir, sehingga waktu start dan",
        "connect tidak ikut dihitung sebagai kecepatan fetch. **Peak RSS** dibaca dari",
        "`/usr/bin/time -l`, yang melaporkan puncak satu proses anak, bukan angka kumulatif.",
        "",
        "Kondisi uji: `SELECT * FROM wide_500k` (30 kolom), tanpa retry, database lokal di container.",
        "",
        "## Baseline engine Python",
        "",
    ]

    if python_runs:
        lines += [
            "| Kind | Baris | Baris pertama (dari connect) | Baris pertama (dari start) | Total | Fetch | Throughput | Peak RSS | Python |",
            "|---|---|---|---|---|---|---|---|---|",
        ]
        for row in python_runs:
            lines.append(
                f"| {row['kind']} | {row['rows']:,} "
                f"| {human_ms(row.get('time_to_first_row_after_connect_ms'))} "
                f"| {human_ms(row['time_to_first_row_ms'])} "
                f"| {human_ms(row['total_ms'])} | {human_ms(row['fetch_ms'])} "
                f"| {human_rate(row['rows_per_second'])} baris/s | {human_bytes(row['peak_rss_bytes'])} "
                f"| {row.get('python', '—')} |"
            )
        lines.append("")
    else:
        lines += [
            "**[belum diukur]** — belum ada hasil. Jalankan harness di atas dengan container yang",
            "sudah menyala.",
            "",
        ]

    lines += ["## Engine Rust", ""]
    if rust_runs:
        lines += [
            "| Kind | Baris | Time-to-first-row | Total | Fetch | Throughput | Peak RSS |",
            "|---|---|---|---|---|---|---|",
        ]
        for row in rust_runs:
            lines.append(
                f"| {row['kind']} | {row['rows']:,} | {human_ms(row['time_to_first_row_ms'])} "
                f"| {human_ms(row['total_ms'])} | {human_ms(row['fetch_ms'])} "
                f"| {human_rate(row['rows_per_second'])} baris/s | {human_bytes(row['peak_rss_bytes'])} |"
            )
    else:
        lines.append(
            "**[belum diukur]** — engine Rust belum punya CLI yang setara `preview`, jadi belum ada "
            "yang bisa diukur."
        )
    lines.append("")

    lines += [
        "## Perbandingan dengan target §6",
        "",
        "| Metrik | Target | Baseline Python | Rust | Status |",
        "|---|---|---|---|---|",
    ]
    pg = next((row for row in python_runs if row["kind"] == "postgres"), None)
    lines.append(
        "| Throughput fetch | ≥ 5× baseline Python | "
        + (f"{human_rate(pg['rows_per_second'])} baris/s" if pg else "—")
        + " | [belum diukur] | Menunggu engine Rust |"
    )
    lines.append(
        "| Time-to-first-row | < 200 ms sejak server mulai mengirim hasil | "
        + (human_ms(pg.get("time_to_first_row_after_connect_ms")) if pg else "—")
        + " | [belum diukur] | Menunggu engine Rust |"
    )
    lines.append(
        "| Memori proses (500k × 30) | < 800 MB | "
        + (human_bytes(pg["peak_rss_bytes"]) if pg else "—")
        + " | [belum diukur] | Menunggu engine Rust |"
    )
    for metric in ("Scroll grid 60 fps", "Cold start < 1 dtk", "Introspeksi 5.000 tabel < 1 dtk",
                   "Pembatalan < 500 ms", "Nol leak lintas FFI"):
        lines.append(f"| {metric} | — | — | — | [belum diukur] |")
    lines.append("")

    lines += ["## Temuan dari baseline", ""]
    findings: list[str] = []
    for row in python_runs:
        kind = row["kind"]
        rate = row.get("rows_per_second")
        first_row = row.get("time_to_first_row_after_connect_ms")
        rss_mb = row.get("peak_rss_mb")

        if rss_mb is not None and rss_mb > 800:
            findings.append(
                f"- **{kind}: memori sudah melewati target §6 sekarang.** {rss_mb:,.0f} MB untuk "
                "500k × 30, sedangkan targetnya < 800 MB. Ini bukan regresi yang diperkenalkan "
                "Rust; ini batas engine Python, dan salah satu alasan store Rust memakai encoding "
                "kolumnar dengan spill."
            )
        if first_row is not None and first_row > 200:
            findings.append(
                f"- **{kind}: baris pertama datang {first_row:,.0f} ms setelah connect**, "
                "sementara targetnya < 200 ms. Engine menunggu halaman pertama utuh sebelum "
                "mengirim apa pun, jadi target ini tidak bisa dicapai tanpa streaming per halaman."
            )
        if rate is not None:
            findings.append(
                f"- {kind}: {rate:,} baris/s adalah **angka dasar yang harus dilampaui 5×** "
                f"menurut §6, jadi target absolutnya sekitar {rate * 5:,} baris/s."
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
    parser.add_argument("--sql", default="SELECT * FROM wide_500k")
    parser.add_argument("--limit", type=int, default=WIDE_ROWS)
    parser.add_argument("--label", default="baseline-python")
    parser.add_argument("--report-only", action="store_true")
    args = parser.parse_args(argv)

    if not args.report_only:
        result = measure(args.kind, args.sql, args.limit, args.label)
        append(result)
        print(json.dumps(result, indent=2, sort_keys=True))

    results = load_results()
    write_report(results)
    print(f"\n{len(results)} run(s) recorded in {RESULTS.relative_to(ROOT)}")
    print(f"report regenerated: {REPORT.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
