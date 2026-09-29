#!/usr/bin/env python3
"""Tests for the record schema and the per-axis report.

    /usr/bin/python3 deploy/dev/test_bench_report.py
"""

import json
import pathlib
import sys
import tempfile
import unittest

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import bench_app  # noqa: E402
import bench_fetch as bf  # noqa: E402

OLD_RECORD = {
    "columns": 30, "engine": "python", "fetch_ms": 3340.3, "kind": "postgres",
    "label": "baseline-python", "limit": 500000, "peak_rss_bytes": 572309504,
    "peak_rss_mb": 545.8, "rows": 500000, "rows_per_second": 149687,
    "time_to_first_row_after_connect_ms": 721.7, "time_to_first_row_ms": 871.1,
    "total_ms": 4234.5,
}
HOST = {"hw_model": "Mac-test", "p_cores": 4, "e_cores": 6, "server_image": None}


def app_record(scenario, app="queryhive", **metrics):
    sample = {"scenario": scenario, "app": app, "metrics": dict(metrics)}
    return bench_app.build_records([sample], source="app", host=HOST, app_rev="abc1234")[0], sample


def render(results):
    with tempfile.TemporaryDirectory() as tmp:
        original = bf.REPORT
        bf.REPORT = pathlib.Path(tmp) / "benchmarks.md"
        try:
            bf.write_report(results)
            return bf.REPORT.read_text(encoding="utf-8")
        finally:
            bf.REPORT = original


class Percentiles(unittest.TestCase):
    def test_median_and_p95(self):
        summary = bf.summarize(list(range(1, 101)))
        self.assertEqual(summary["median"], 50.5)
        self.assertAlmostEqual(summary["p95"], 95.05)
        self.assertEqual((summary["min"], summary["max"], summary["n"]), (1, 100, 100))

    def test_single_value_and_empty(self):
        self.assertEqual(bf.summarize([7])["p95"], 7)
        self.assertIsNone(bf.summarize([]))
        self.assertIsNone(bf.summarize(["x", True]))


class Aggregation(unittest.TestCase):
    def test_groups_by_scenario_and_app(self):
        samples = [
            {"scenario": "ttfr-s1-1k", "ttfr_ms": v} for v in (10, 20, 30, 40)
        ] + [{"scenario": "ttfr-s1-1k", "app": "tablepro", "metrics": {"ttfr_ms": [100, 120]}}]
        records = bench_app.build_records(samples, source="ffi", axis=1, repeat=5, host=HOST)
        self.assertEqual(len(records), 2)
        mine, theirs = records
        self.assertEqual((mine["bench"], mine["axis"], mine["app"]), ("ffi", 1, "queryhive"))
        self.assertEqual(mine["metrics"]["ttfr_ms"]["median"], 25)
        self.assertTrue(mine["short"])
        self.assertEqual(theirs["metrics"]["ttfr_ms"]["n"], 2)
        self.assertEqual(mine["hw_model"], "Mac-test")

    def test_axis_defaults_from_scenario_and_status_record(self):
        record = bench_app.build_records(
            [{"scenario": "scroll-30x1m", "app": "tablepro", "status": "tidak diukur (izin OS)"}],
            source="qhbench", host=HOST)[0]
        self.assertEqual(record["axis"], 4)
        self.assertEqual(record["bench"], "blackbox")
        self.assertEqual(record["status"], "tidak diukur (izin OS)")
        self.assertNotIn("metrics", record)

    def test_rejects_unknown_status(self):
        with self.assertRaises(SystemExit):
            bench_app.build_records([{"scenario": "x", "status": "kira-kira"}], source="app")

    def test_parse_lines_and_document(self):
        lines = 'log noise\n{"scenario": "a", "ttfr_ms": 1}\n\n{"scenario": "a", "ttfr_ms": 2}\n'
        self.assertEqual(len(bench_app.parse_samples(lines)), 2)
        self.assertEqual(len(bench_app.parse_samples('{"records": [{"scenario": "a"}]}')), 1)
        self.assertEqual(len(bench_app.parse_samples('[{"scenario": "a"}, {"scenario": "b"}]')), 2)


class OldRecords(unittest.TestCase):
    def test_record_without_bench_is_fetch(self):
        self.assertEqual(bf.fetch_records([OLD_RECORD]), [OLD_RECORD])
        record, _ = app_record("ttfr-s1-1k", ttfr_ms=12.0)
        self.assertEqual(bf.fetch_records([OLD_RECORD, record]), [OLD_RECORD])

    def test_report_keeps_fetch_section_and_marks_axes_unmeasured(self):
        text = render([OLD_RECORD])
        self.assertIn("## Baseline engine Python (rekaman beku)", text)
        self.assertIn("149,687 baris/s", text)
        self.assertIn("## Ongkos `panic = \"unwind\"` pada ukuran artefak", text)
        for axis, title in bf.AXES.items():
            self.assertIn(f"## Sumbu {axis}: {title}" if isinstance(axis, int) else f"## {title}",
                          text)
        self.assertIn("| `ttfr-s1-1k` — S1 hangat, cap 1.000 |", text)
        self.assertIn("[belum diukur]", text)
        self.assertNotIn("Memenuhi target absolut", text)

    def test_app_records_do_not_break_fetch_grouping(self):
        record, _ = app_record("ttfr-s1-1k", ttfr_ms=12.0)
        self.assertEqual(render([OLD_RECORD, record]).count("baseline-python"), 3)


class AxisReport(unittest.TestCase):
    """One synthetic QueryHive record per axis row, then check its row renders."""

    PASSING = {
        "ttfr-s1-1k": {"ttfr_ms": [10, 12, 14]},
        "ttfr-s1-10k": {"ttfr_ms": [10, 12, 14]},
        "ttfr-s2-500k": {"ttfr_ms": [30, 32]},
        "ttfr-s3-rtt30": {"ttfr_ms": [40, 42]},
        "ttfr-s4-first-run": {"ttfr_ms": [300]},
        "rows-wide-500k": {"rows_per_s": [600_000, 610_000]},
        "rows-lineitem-1m": {"rows_per_s": [600_000]},
        "mem-500k": {"footprint_delta_bytes": [50_000_000], "budget_bytes": [100_000_000]},
        "mem-5m": {"footprint_delta_bytes": [50_000_000], "budget_bytes": [100_000_000]},
        "scroll-30x1m": {"hitch_ms_per_s": [0.5], "frame_p99_ms": [8.0]},
        "scroll-500x10k": {"hitch_ms_per_s": [0.5], "frame_p99_ms": [8.0], "render_ms": [20]},
        "type-10k": {"keystroke_main_p99_ms": [3.0], "input_to_photon_ms": [20]},
        "type-2m": {"keystroke_main_p99_ms": [6.0], "input_to_photon_ms": [20]},
        "cancel-pg-sleep": {"cancel_ms": [40, 60]},
        "cancel-mysql-sleep": {"cancel_ms": [40, 60]},
        "cancel-stream-wide": {"cancel_ms": [40, 60]},
        "cancel-trino-heavy": {"cancel_ms": [200]},
        "launch-warm": {"launch_ms": [300]},
        "launch-cold": {"launch_ms": [900]},
        "sort-numeric-500k": {"sort_ms": [80]},
        "sort-text-500k": {"sort_ms": [250]},
        "introspect-5000": {"duration_ms": [800]},
        "ffi-leak": {"leak_count": [0]},
    }

    def test_every_scenario_has_a_synthetic_record(self):
        self.assertEqual({row[1] for row in bf.AXIS_ROWS}, set(self.PASSING))
        self.assertEqual({row[0] for row in bf.AXIS_ROWS}, set(bf.AXES))

    def test_each_axis_row_renders_its_value_and_a_verdict(self):
        results = [OLD_RECORD]
        for scenario, metrics in self.PASSING.items():
            sample = {"scenario": scenario, "metrics": metrics}
            results += bench_app.build_records([sample], source="app", host=HOST)
        text = render(results)
        for scenario in self.PASSING:
            row = next(l for l in text.splitlines() if l.startswith(f"| `{scenario}` "))
            if scenario != "ttfr-s4-first-run":  # the plan gives S4 no absolute target
                self.assertIn("Memenuhi target absolut", row, scenario)
            self.assertIn("(n=", row, scenario)  # the QueryHive cell carries its value and n

    def test_failing_value_is_reported(self):
        record = bench_app.build_records(
            [{"scenario": "cancel-pg-sleep", "cancel_ms": [400, 500]}], source="app", host=HOST)
        row = next(l for l in render(record).splitlines() if l.startswith("| `cancel-pg-sleep`"))
        self.assertIn("Belum memenuhi", row)

    def test_tablepro_value_ratio_and_status(self):
        results = bench_app.build_records(
            [{"scenario": "ttfr-s1-1k", "metrics": {"ttfr_ms": [10]}},
             {"scenario": "ttfr-s1-1k", "app": "tablepro", "metrics": {"ttfr_ms": [40]}},
             {"scenario": "scroll-30x1m", "app": "tablepro", "status": "tidak diukur (izin OS)"},
             {"scenario": "launch-cold", "app": "tablepro", "status": "tidak diukur (butuh sudo)"},
             {"scenario": "rows-lineitem-1m", "app": "tablepro", "status": "tidak mendukung"}],
            source="qhbench", host=HOST)
        text = render(results)
        row = next(l for l in text.splitlines() if l.startswith("| `ttfr-s1-1k`"))
        self.assertIn("vs TablePro 0,25×".replace(",", "."), row)
        self.assertIn("lolos", row)
        for status in ("tidak diukur (izin OS)", "tidak diukur (butuh sudo)", "tidak mendukung"):
            self.assertIn(f"| {status} |", text)

    def test_memory_without_budget_is_not_graded(self):
        record = bench_app.build_records(
            [{"scenario": "mem-500k", "footprint_delta_bytes": 1}], source="app", host=HOST)
        row = next(l for l in render(record).splitlines() if l.startswith("| `mem-500k`"))
        self.assertIn("anggaran store belum dicatat", row)

    def test_throughput_uses_the_lower_of_3x_and_80_percent_of_ceiling(self):
        def row_for(rate):
            record = bench_app.build_records(
                [{"scenario": "rows-wide-500k",
                  "metrics": {"rows_per_s": rate, "ceiling_rows_per_s": 400_000}}],
                source="app", host=HOST)
            return next(l for l in render(record).splitlines() if l.startswith("| `rows-wide-500k`"))
        self.assertIn("Belum memenuhi", row_for(300_000))  # threshold is 0.8 * 400k = 320k
        self.assertIn("Memenuhi target absolut", row_for(330_000))

    def test_ceiling_above_threshold_keeps_575k(self):
        record = bench_app.build_records(
            [{"scenario": "rows-wide-500k",
              "metrics": {"rows_per_s": 600_000, "ceiling_rows_per_s": 1_000_000}}],
            source="app", host=HOST)
        row = next(l for l in render(record).splitlines() if l.startswith("| `rows-wide-500k`"))
        self.assertIn("Memenuhi target absolut", row)  # 0.8 * 1M would be 800k, but 575k applies

    def test_partly_measured_scenario_is_never_memenuhi(self):
        record = bench_app.build_records(
            [{"scenario": "scroll-500x10k", "metrics": {"hitch_ms_per_s": 0.5}}],
            source="app", host=HOST)
        row = next(l for l in render(record).splitlines() if l.startswith("| `scroll-500x10k`"))
        self.assertIn("Sebagian terukur — frame_p99_ms, render_ms [belum diukur]", row)
        self.assertNotIn("Memenuhi", row)

    def test_failure_with_missing_checks_lists_both(self):
        record = bench_app.build_records(
            [{"scenario": "scroll-500x10k", "metrics": {"hitch_ms_per_s": 5}}],
            source="app", host=HOST)
        row = next(l for l in render(record).splitlines() if l.startswith("| `scroll-500x10k`"))
        self.assertIn("Belum memenuhi", row)
        self.assertIn("di atas batas", row)
        self.assertIn("sisanya frame_p99_ms, render_ms [belum diukur]", row)

    def test_comparison_metric_is_shown_in_both_cells(self):
        results = bench_app.build_records(
            [{"scenario": "type-10k",
              "metrics": {"keystroke_main_p99_ms": 3, "input_to_photon_ms": 20}},
             {"scenario": "type-10k", "app": "tablepro",
              "metrics": {"keystroke_main_p99_ms": 5, "input_to_photon_ms": 40}}],
            source="qhbench", host=HOST)
        row = next(l for l in render(results).splitlines() if l.startswith("| `type-10k`"))
        cells = [c.strip() for c in row.split(" | ")]
        self.assertIn("input_to_photon_ms p95 20.0 ms", cells[2])
        self.assertIn("input_to_photon_ms p95 40.0 ms", cells[3])
        self.assertIn("vs TablePro 0.50×", row)

    def test_ratio_suffix_is_a_metric(self):
        record = bench_app.build_records(
            [{"scenario": "preview-wide", "sink_share_ratio": 0.4, "call_p50_ms": 2}],
            source="ffi", host=HOST)[0]
        self.assertEqual(set(record["metrics"]), {"sink_share_ratio", "call_p50_ms"})

    def test_section6_rows_fill_from_data(self):
        text = render([OLD_RECORD])
        self.assertIn("| Cold start < 1 dtk | — | — | — | [belum diukur] |", text)
        record = bench_app.build_records(
            [{"scenario": "launch-cold", "launch_ms": 900},
             {"scenario": "ffi-leak", "leak_count": 3}], source="app", host=HOST)
        text = render([OLD_RECORD] + record)
        self.assertIn("| Cold start < 1 dtk | < 1 dtk | — | 900.0 ms | Memenuhi — 900.0 ms |", text)
        self.assertIn("| Nol leak lintas FFI | nol leak | — | 3 | Belum memenuhi — 3 |", text)
        self.assertIn("| Pembatalan < 500 ms | — | — | — | [belum diukur] |", text)


class ResultsFile(unittest.TestCase):
    def test_existing_results_file_still_loads_and_renders(self):
        results = bf.load_results()
        self.assertTrue(results)
        text = render(results)
        self.assertIn("## Engine Rust", text)
        self.assertNotIn("\n\n\n", text)


if __name__ == "__main__":
    unittest.main()
