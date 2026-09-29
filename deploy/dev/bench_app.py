#!/usr/bin/env python3
"""Append app-level benchmark records to deploy/dev/bench-results.jsonl.

    QueryHive --bench ttfr-s1-1k | python3 deploy/dev/bench_app.py --source app
    cargo run --release -p qh-ffi --example bench_ffi -- preview-wide \\
        | python3 deploy/dev/bench_app.py --source ffi --axis 2 --repeat 10
    python3 deploy/dev/bench_app.py --source qhbench --input out.json --app tablepro \\
        --competitor-rev <sha>
    python3 deploy/dev/bench_app.py --report-only        # same as bench_fetch.py
    python3 deploy/dev/bench_app.py --list-scenarios

This is a thin adapter. The producers (`QueryHive --bench`, `bench_ffi`, `qhbench`)
print raw samples; this script groups them per scenario and app, computes the
median and p95, adds the machine facts, and appends one record per group. The
report is rendered by `bench_fetch.py --report-only`, from the same file.

Input contract (what a producer must print)
-------------------------------------------
Standard input or `--input FILE`. Either JSON lines (one object per line) or one
JSON document: an object, an array of objects, or `{"records": [...]}`. One
object is one sample, that is one run. Blank lines and lines that do not start
with `{` are ignored, so a producer may log around its JSON.

    {
      "scenario": "ttfr-s1-1k",          # required unless --scenario is given;
                                         # names live in AXIS_ROWS (--list-scenarios)
      "axis": 1,                         # optional: 1..7 or sort-search |
                                         # introspection | ffi-leak; else --axis
      "app": "queryhive",                # optional: queryhive | tablepro; else --app
      "metrics": {                       # the measurements, name -> number or list
        "ttfr_ms": 12.3,                 #   of numbers (a list is several samples)
        "rows_per_s": 191251
      },
      "app_rev": "88e21bf",              # optional; default: git rev of this tree
      "competitor_rev": "…",             # optional (tablepro); else --competitor-rev
      "display_hz": 120,                 # optional; else --display-hz
      "label": "session-20260929",       # optional; else --label
      "notes": "free text"               # optional
    }

Units are in the metric name and are not converted: `_ms` milliseconds,
`_rows_per_s` rows per second, `_bytes` bytes, `_ms_per_s` milliseconds of hitch
per second of scrolling, `_count` a plain count. A producer may also print the
metrics flat, as top-level keys next to `scenario`, when they end in one of those
suffixes and no `metrics` object is present. `_ratio` (a dimensionless fraction,
e.g. `sink_share_ratio`) is accepted too; the report prints it without a unit.

Axis-2 ceiling rule, as implemented: the target is 575,000 rows/s, unless the
record carries `ceiling_rows_per_s` (the same-day `psql COPY` ceiling) and that
ceiling is below 575,000 / 0.8 = 718,750, in which case the target is 0.8 x the
ceiling.

Metric names the report grades (see AXIS_ROWS in bench_fetch.py):
`ttfr_ms`, `rows_per_s` (+ optional `ceiling_rows_per_s`), `footprint_delta_bytes`
(+ optional `budget_bytes`), `hitch_ms_per_s`, `frame_p99_ms`, `render_ms`,
`keystroke_main_p99_ms`, `input_to_photon_ms`, `cancel_ms`, `launch_ms`, `sort_ms`,
`duration_ms`, `leak_count`. Any other suffixed name is stored and not graded.

A sample that measured nothing says why, instead of `metrics`:

    {"scenario": "scroll-30x1m", "app": "tablepro",
     "status": "tidak diukur (izin OS)"}

`status` is one of "tidak diukur (izin OS)", "tidak diukur (butuh sudo)",
"tidak mendukung", "[belum diukur]".

Output record, one per (scenario, app), appended and never rewritten:

    bench            "app" | "ffi" | "blackbox"   (from --source app|ffi|qhbench)
    axis, scenario, app, app_rev, competitor_rev, label, recorded_at
    hw_model, p_cores, e_cores    sysctl hw.model, hw.perflevel0/1.physicalcpu
    display_hz, server_image      server_image: {container: image@digest} via podman
    metrics          {name: {"median", "p95", "min", "max", "n"}}
    n_expected       --repeat, when given; "short": true when fewer samples arrived
    status           only when nothing was measured

`--repeat N` is the sample count the plan asks for (10, and 20 for TTFR). The
script does not run the producer N times; a producer that prints N lines, or a
shell loop, does. Fewer than N samples is still recorded, flagged `short`, and
warned about on stderr.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import bench_fetch as bf  # noqa: E402

SOURCE_BENCH = {"app": "app", "ffi": "ffi", "qhbench": "blackbox"}
UNIT_SUFFIXES = ("_ms_per_s", "rows_per_s", "_ratio", "_bytes", "_count", "_ms")


def parse_samples(text: str) -> list[dict]:
    """Sample objects from JSON lines or from one JSON document."""
    stripped = text.strip()
    if not stripped:
        return []
    try:
        document = json.loads(stripped)
    except json.JSONDecodeError:
        document = None
    if document is not None:
        if isinstance(document, dict) and isinstance(document.get("records"), list):
            document = document["records"]
        if isinstance(document, dict):
            document = [document]
        if isinstance(document, list):
            return [item for item in document if isinstance(item, dict)]
        raise SystemExit("input JSON must be an object, an array of objects, or {records: [...]}")
    samples = []
    for number, line in enumerate(text.splitlines(), 1):
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            item = json.loads(line)
        except json.JSONDecodeError as error:
            raise SystemExit(f"line {number}: not valid JSON ({error})")
        if isinstance(item, dict):
            samples.append(item)
    return samples


def sample_metrics(sample: dict) -> dict[str, list[float]]:
    """Metric name -> list of numbers, from `metrics` or from flat suffixed keys."""
    raw = sample.get("metrics")
    if not isinstance(raw, dict):
        raw = {k: v for k, v in sample.items() if k.endswith(UNIT_SUFFIXES)}
    found: dict[str, list[float]] = {}
    for name, value in raw.items():
        values = value if isinstance(value, list) else [value]
        numbers = [v for v in values if isinstance(v, (int, float)) and not isinstance(v, bool)]
        if numbers:
            found[name] = numbers
    return found


def build_records(samples: list[dict], *, source: str, axis=None, app: str = "queryhive",
                  scenario: str | None = None, label: str = "", repeat: int | None = None,
                  app_rev: str | None = None, competitor_rev: str | None = None,
                  display_hz: int | None = None, host: dict | None = None) -> list[dict]:
    """Group samples by (scenario, app) into one record each, in first-seen order."""
    groups: dict[tuple, list[dict]] = {}
    for sample in samples:
        name = sample.get("scenario") or scenario
        if not name:
            raise SystemExit("a sample has no `scenario` and --scenario was not given")
        if scenario and sample.get("scenario") and sample["scenario"] != scenario:
            continue
        who = sample.get("app") or app
        if who not in bf.APPS:
            raise SystemExit(f"unknown app {who!r}; expected one of {bf.APPS}")
        status = sample.get("status")
        if status is not None and status not in bf.STATUSES:
            raise SystemExit(f"unknown status {status!r}; expected one of {bf.STATUSES}")
        groups.setdefault((name, who), []).append(sample)

    scenario_axis = {row[1]: row[0] for row in bf.AXIS_ROWS}
    records = []
    for (name, who), members in groups.items():
        values: dict[str, list[float]] = {}
        for sample in members:
            for metric, numbers in sample_metrics(sample).items():
                values.setdefault(metric, []).extend(numbers)
        first = members[0]
        record = {
            "bench": first.get("bench") or SOURCE_BENCH[source],
            "axis": first.get("axis", axis if axis is not None else scenario_axis.get(name)),
            "scenario": name,
            "app": who,
            "app_rev": first.get("app_rev") or (app_rev if who == "queryhive" else None),
            "competitor_rev": first.get("competitor_rev") or competitor_rev,
            "label": first.get("label") or label,
            "recorded_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        }
        record.update(host or {})
        if first.get("display_hz") is not None:
            record["display_hz"] = first["display_hz"]
        elif display_hz is not None:
            record["display_hz"] = display_hz
        if first.get("notes"):
            record["notes"] = first["notes"]
        metrics = {metric: bf.summarize(numbers) for metric, numbers in values.items()}
        metrics = {k: v for k, v in metrics.items() if v}
        if metrics:
            record["metrics"] = metrics
            samples_seen = max(m["n"] for m in metrics.values())
        else:
            statuses = [s.get("status") for s in members if s.get("status")]
            record["status"] = statuses[-1] if statuses else bf.UNMEASURED
            samples_seen = 0
        if repeat is not None:
            record["n_expected"] = repeat
            if metrics and samples_seen < repeat:
                record["short"] = True
        records.append(record)
    return records


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__,
                                     formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--source", choices=sorted(SOURCE_BENCH), default="app",
                        help="app: QueryHive --bench; ffi: bench_ffi; qhbench: black-box harness")
    parser.add_argument("--input", default="-", help="JSON file, or - for standard input")
    parser.add_argument("--axis", default=None,
                        help="default axis for samples without one: 1..7 or a secondary name")
    parser.add_argument("--scenario", default=None,
                        help="default scenario; also drops samples of other scenarios")
    parser.add_argument("--label", default="", help="measurement session label")
    parser.add_argument("--repeat", type=int, default=None,
                        help="samples the plan asks for; fewer is recorded as short")
    parser.add_argument("--app", choices=bf.APPS, default="queryhive")
    parser.add_argument("--competitor-rev", default=None)
    parser.add_argument("--display-hz", type=int, default=None)
    parser.add_argument("--dry-run", action="store_true", help="print the records, append nothing")
    parser.add_argument("--no-report", action="store_true",
                        help="append without rewriting docs/benchmarks.md")
    parser.add_argument("--report-only", action="store_true",
                        help="regenerate docs/benchmarks.md and exit")
    parser.add_argument("--list-scenarios", action="store_true")
    args = parser.parse_args(argv)

    if args.list_scenarios:
        for row in bf.AXIS_ROWS:
            print(f"{row[0]}\t{row[1]}\t{row[2]}\t{row[4]}")
        return 0
    if args.report_only:
        bf.write_report(bf.load_results())
        print(f"report regenerated: {bf.REPORT.relative_to(bf.ROOT)}")
        return 0
    axis = args.axis
    if axis is not None and axis.isdigit():
        axis = int(axis)
    if axis is not None and axis not in bf.AXES:
        raise SystemExit(f"unknown axis {axis!r}; expected one of {list(bf.AXES)}")
    if args.repeat is not None and args.repeat < 1:
        raise SystemExit("--repeat must be at least 1")

    text = sys.stdin.read() if args.input == "-" else pathlib.Path(args.input).read_text("utf-8")
    samples = parse_samples(text)
    if not samples:
        raise SystemExit("no samples on input")

    records = build_records(
        samples, source=args.source, axis=axis, app=args.app, scenario=args.scenario,
        label=args.label, repeat=args.repeat, app_rev=bf.app_revision(),
        competitor_rev=args.competitor_rev, display_hz=args.display_hz,
        host=bf.host_facts(),
    )
    if not records:
        raise SystemExit("no sample matched --scenario")
    for record in records:
        if record["app"] == "tablepro" and record.get("metrics") and not record.get("competitor_rev"):
            print(f"warning: {record['scenario']}/tablepro has metrics but no competitor_rev "
                  "(--competitor-rev)", file=sys.stderr)
        if record.get("short"):
            print(f"warning: {record['scenario']}/{record['app']}: fewer than "
                  f"{record['n_expected']} samples", file=sys.stderr)
        print(json.dumps(record, indent=2, sort_keys=True))
        if not args.dry_run:
            bf.append(record)
    if not args.dry_run and not args.no_report:
        bf.write_report(bf.load_results())
        print(f"report regenerated: {bf.REPORT.relative_to(bf.ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
