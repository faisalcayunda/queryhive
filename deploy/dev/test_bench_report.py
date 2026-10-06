#!/usr/bin/env python3
"""Tests for the record schema and the per-axis report.

    /usr/bin/python3 deploy/dev/test_bench_report.py
"""

import http.server
import io
import json
import pathlib
import re
import sys
import tempfile
import threading
import unittest

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import bench_app  # noqa: E402
import bench_ceiling  # noqa: E402
import bench_fetch as bf  # noqa: E402

SWIFT = HERE.parents[1] / "app" / "Sources" / "QueryHive" / "Support" / "BenchMode.swift"

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
        self.assertIn("| `ttfr-s1t-1k` — S1 hangat, cap 1.000, `SELECT * FROM wide_500k` |", text)
        self.assertIn("[belum diukur]", text)
        self.assertNotIn("Memenuhi target absolut", text)

    def test_app_records_do_not_break_fetch_grouping(self):
        record, _ = app_record("ttfr-s1-1k", ttfr_ms=12.0)
        self.assertEqual(render([OLD_RECORD, record]).count("baseline-python"), 3)


TABLE = "sql_kind=table"


def graded(row) -> bool:
    return any(check[3] != bf.INFO for check in row[3])


class AxisReport(unittest.TestCase):
    """One synthetic QueryHive record per axis row, then check its row renders."""

    PASSING = {
        "ttfr-s1t-1k": {"ttfr_ms": [10, 12, 14]},
        "ttfr-s1t-10k": {"ttfr_ms": [10, 12, 14]},
        "ttfr-s2t-500k": {"ttfr_ms": [30, 32]},
        "ttfr-s3-rtt30": {"ttfr_ms": [40, 42]},
        "ttfr-s4-first-run": {"ttfr_ms": [300]},
        "rerun-capped-rtt30": {"rerun_exceeds_rtt_ratio": [0, 0, 1], "rerun_penalty_ms": [5, 8, 40],
                               "rerun_penalty_capped_ms": [4, 6, 30], "rerun_sort_ttfr_ms": [90]},
        "rerun-capped": {"rerun_penalty_ms": [0.4], "rerun_sort_ttfr_ms": [30]},
        "ttfr-s1-1k": {"ttfr_ms": [55]},
        "ttfr-s1-10k": {"ttfr_ms": [70]},
        "rows-pg-table-500k": {"rows_per_s": [600_000, 610_000]},
        "rows-mysql-500k": {"rows_per_s": [600_000]},
        "rows-lineitem-1m": {"rows_per_s": [600_000]},
        "rows-trino-500k": {"rows_per_s": [600_000]},
        "rows-wide-500k": {"rows_per_s": [78_000]},
        "mem-500k": {"peak_footprint_delta_bytes": [50_000_000], "budget_bytes": [100_000_000]},
        "mem-mysql-500k": {"peak_footprint_delta_bytes": [50_000_000], "budget_bytes": [100_000_000]},
        "mem-5m": {"peak_footprint_delta_bytes": [50_000_000], "budget_bytes": [100_000_000]},
        "scroll-30x1m": {"hitch_ms_per_s": [0.5], "frame_cost_p99_ms": [6.0], "frame_p99_ms": [16.7]},
        "scroll-30x1m-spilled": {"hitch_ms_per_s": [0.5], "frame_cost_p99_ms": [9.0]},
        "scroll-500x10k": {"hitch_ms_per_s": [0.5], "frame_cost_p99_ms": [6.0], "frame_p99_ms": [16.7]},
        "open-500x10k": {"render_ms": [20, 22, 24], "render_cold_ms": [44]},
        "type-10k-plan": {"keystroke_main_p99_ms": [3.0], "input_to_photon_ms": [20],
                          "uncoloured_turns_count": [0]},
        "type-2m": {"keystroke_main_p99_ms": [6.0], "input_to_photon_ms": [20],
                    "uncoloured_turns_count": [0]},
        "type-10k": {"keystroke_main_p99_ms": [8.0]},
        "type-coloured-195k": {"keystroke_main_p99_ms": [3.0]},
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

    def sample(self, scenario, metrics, **extra):
        sample = {"scenario": scenario, "metrics": metrics, **extra}
        if scenario in bf.TABLE_SQL:
            sample.setdefault("notes", TABLE)
        return sample

    def line(self, results, scenario):
        return next(l for l in render(results).splitlines() if l.startswith(f"| `{scenario}` "))

    def one(self, scenario, metrics, **extra):
        return self.line(bench_app.build_records([self.sample(scenario, metrics, **extra)],
                                                 source="app", host=HOST), scenario)

    def test_every_scenario_has_a_synthetic_record(self):
        self.assertEqual({row[1] for row in bf.AXIS_ROWS}, set(self.PASSING))
        self.assertEqual({row[0] for row in bf.AXIS_ROWS}, set(bf.AXES))
        self.assertEqual(len({row[1] for row in bf.AXIS_ROWS}), len(bf.AXIS_ROWS), "one row per scenario")

    def test_each_axis_row_renders_its_value_and_a_verdict(self):
        results = [OLD_RECORD]
        for scenario, metrics in self.PASSING.items():
            results += bench_app.build_records([self.sample(scenario, metrics)], source="app", host=HOST)
        text = render(results)
        for row in bf.AXIS_ROWS:
            scenario = row[1]
            line = next(l for l in text.splitlines() if l.startswith(f"| `{scenario}` "))
            if graded(row):
                self.assertIn("Memenuhi target absolut", line, scenario)
            else:  # the plan gives S4 no target, and the generated-SQL rows are a regression guard
                self.assertIn("informasional", line, scenario)
            self.assertIn("(n=", line, scenario)  # the QueryHive cell carries its value and n

    def test_failing_value_is_reported(self):
        self.assertIn("Belum memenuhi", self.one("cancel-pg-sleep", {"cancel_ms": [400, 500]}))

    def test_tablepro_value_ratio_and_status(self):
        # TablePro was recorded under the scenario's older name; it runs the plan's SQL, so it answers.
        results = bench_app.build_records(
            [self.sample("ttfr-s1t-1k", {"ttfr_ms": [10]}),
             {"scenario": "ttfr-s1-1k", "app": "tablepro", "metrics": {"ttfr_ms": [40]}},
             {"scenario": "scroll-30x1m", "app": "tablepro", "status": "tidak diukur (izin OS)"},
             {"scenario": "launch-cold", "app": "tablepro", "status": "tidak diukur (butuh sudo)"},
             {"scenario": "rows-lineitem-1m", "app": "tablepro", "status": "tidak mendukung"}],
            source="qhbench", host=HOST)
        text = render(results)
        row = next(l for l in text.splitlines() if l.startswith("| `ttfr-s1t-1k`"))
        self.assertIn("vs TablePro 0,25×".replace(",", "."), row)
        self.assertIn("lolos", row)
        for status in ("tidak diukur (izin OS)", "tidak diukur (butuh sudo)", "tidak mendukung"):
            self.assertIn(f"| {status} |", text)

    def test_a_tablepro_record_under_the_new_name_wins(self):
        results = bench_app.build_records(
            [self.sample("ttfr-s1t-1k", {"ttfr_ms": [10]}),
             {"scenario": "ttfr-s1-1k", "app": "tablepro", "metrics": {"ttfr_ms": [40]}},
             {"scenario": "ttfr-s1t-1k", "app": "tablepro", "metrics": {"ttfr_ms": [20]}}],
            source="qhbench", host=HOST)
        self.assertIn("vs TablePro 0.50×", self.line(results, "ttfr-s1t-1k"))

    def test_memory_is_the_peak_against_the_budget_plus_64_mb(self):
        self.assertIn("anggaran store belum dicatat",
                      self.one("mem-500k", {"peak_footprint_delta_bytes": 1}))
        within = {"peak_footprint_delta_bytes": [60_000_000, 160_000_000], "budget_bytes": 100_000_000}
        self.assertIn("Memenuhi target absolut", self.one("mem-500k", within))
        over = {"peak_footprint_delta_bytes": [60_000_000, 370_000_000], "budget_bytes": 268_435_456}
        row = self.one("mem-mysql-500k", over)  # a cold peak above 256 MiB + 64 MiB, as MySQL's was
        self.assertIn("Belum memenuhi", row)
        self.assertIn("peak_footprint_delta_bytes max", row)

    def rate_row(self, scenario, rate, ceiling=None, kind=None):
        metrics = {"rows_per_s": rate}
        notes = [TABLE]
        if ceiling is not None:
            metrics["ceiling_rows_per_s"] = ceiling
        if kind:
            notes.append(f"ceiling_kind={kind}")
        return self.one(scenario, metrics, notes="; ".join(notes))

    def test_throughput_uses_the_lower_of_575k_and_80_percent_of_a_ceiling_of_the_right_kind(self):
        # threshold is 0.8 * 400k = 320k
        self.assertIn("Belum memenuhi", self.rate_row("rows-pg-table-500k", 300_000, 400_000, "same-sql-copy"))
        self.assertIn("Memenuhi target absolut", self.rate_row("rows-pg-table-500k", 330_000, 400_000, "same-sql-copy"))
        self.assertIn("Memenuhi target absolut", self.rate_row("rows-lineitem-1m", 330_000, 400_000, "nexturi-drain"))
        row = self.rate_row("rows-pg-table-500k", 330_000, 400_000, "same-sql-copy")
        self.assertIn("plafon same-sql-copy", row)

    def test_a_ceiling_above_the_threshold_keeps_575k(self):
        row = self.rate_row("rows-pg-table-500k", 600_000, 1_000_000, "same-sql-copy")
        self.assertIn("Memenuhi target absolut", row)  # 0.8 * 1M would be 800k, but 575k applies
        self.assertIn("Belum memenuhi", self.rate_row("rows-pg-table-500k", 500_000, 1_000_000, "same-sql-copy"))

    def test_a_ceiling_of_the_wrong_kind_or_no_kind_is_ignored(self):
        for kind in (None, "nexturi-drain", "none"):
            row = self.rate_row("rows-pg-table-500k", 330_000, 400_000, kind)
            self.assertIn("Belum memenuhi", row, kind)  # 575k absolute, not 320k
            self.assertIn("plafon diabaikan", row, kind)
        # MySQL has no valid ceiling at all.
        row = self.rate_row("rows-mysql-500k", 330_000, 400_000, "same-sql-copy")
        self.assertIn("Belum memenuhi", row)
        self.assertIn("plafon diabaikan", row)
        self.assertNotIn("plafon diabaikan", self.rate_row("rows-mysql-500k", 600_000))

    def test_the_generated_pg_scenario_is_informational_whatever_its_ceiling(self):
        row = self.one("rows-wide-500k", {"rows_per_s": 77_000, "ceiling_rows_per_s": 81_000},
                       notes="sql_kind=generated; ceiling_kind=same-sql-copy")
        self.assertIn("informasional", row)
        self.assertNotIn("Memenuhi target absolut", row)
        self.assertNotIn("Belum memenuhi", row)

    def test_gate_rows_need_the_plans_sql(self):
        for notes in ("sql_kind=generated", "ceiling_kind=same-sql-copy", ""):
            results = bench_app.build_records(
                [{"scenario": "ttfr-s1t-1k", "metrics": {"ttfr_ms": [10]}, "notes": notes}],
                source="app", host=HOST)
            row = self.line(results, "ttfr-s1t-1k")
            self.assertIn("[belum diukur] — sql_kind=", row, notes)
            self.assertNotIn("Memenuhi", row, notes)

    def test_old_mysql_and_trino_records_ran_a_table_and_are_still_graded(self):
        # Before W8-F0 the notes had no sql_kind; these two never ran anything else.
        for scenario in ("rows-mysql-500k", "rows-trino-500k"):
            old = bench_app.build_records([{"scenario": scenario, "metrics": {"rows_per_s": [537_000]}}],
                                          source="app", host=HOST)
            row = self.line(old, scenario)
            self.assertIn("Belum memenuhi", row)
            self.assertIn("sql_kind=table", row)
        # The rest cannot say, and stay unmeasured.
        old = bench_app.build_records([{"scenario": "rows-pg-table-500k", "metrics": {"rows_per_s": [600_000]}}],
                                      source="app", host=HOST)
        self.assertIn("[belum diukur] — sql_kind=tidak tercatat", self.line(old, "rows-pg-table-500k"))

    def test_notes_survive_the_record(self):
        record = bench_app.build_records(
            [{"scenario": "ttfr-s1t-1k", "metrics": {"ttfr_ms": 10}, "notes": TABLE + "; warm_up=1"},
             {"scenario": "ttfr-s1t-1k", "metrics": {"ttfr_ms": 12}, "notes": TABLE + "; warm_up=1"}],
            source="app", host=HOST)[0]
        self.assertEqual(bf.note_value(record, "sql_kind"), "table")
        self.assertEqual(bf.note_value(record, "warm_up"), "1")
        self.assertIsNone(bf.note_value(record, "ceiling_kind"))
        self.assertIsNone(bf.note_value(None, "sql_kind"))
        free = {"notes": "ceiling_rows_per_s = same-day psql COPY; sql_kind=table; via=toxiproxy:55435"}
        self.assertEqual(bf.note_value(free, "sql_kind"), "table")
        self.assertEqual(bf.note_value(free, "via"), "toxiproxy:55435")
        self.assertIsNone(bf.note_value(free, "ceiling_rows_per_s"))

    def test_partly_measured_scenario_is_never_memenuhi(self):
        row = self.one("scroll-500x10k", {"hitch_ms_per_s": 0.5})
        self.assertIn("Sebagian terukur — frame_cost_p99_ms [belum diukur]", row)
        self.assertNotIn("Memenuhi", row)

    def test_failure_with_missing_checks_lists_both(self):
        row = self.one("scroll-500x10k", {"hitch_ms_per_s": 5})
        self.assertIn("Belum memenuhi", row)
        self.assertIn("di atas batas", row)
        self.assertIn("sisanya frame_cost_p99_ms [belum diukur]", row)

    def test_frame_time_is_judged_only_on_a_fast_enough_panel(self):
        metrics = {"hitch_ms_per_s": 0.04, "frame_cost_p99_ms": 5.0, "frame_p99_ms": 16.7}
        sixty = self.one("scroll-30x1m", metrics, display_hz=60)
        self.assertIn("Memenuhi target absolut", sixty)
        self.assertIn("frame_p99_ms tidak dinilai (panel 60 Hz < 120 Hz)", sixty)
        fast = self.one("scroll-30x1m", metrics, display_hz=120)
        self.assertIn("Belum memenuhi", fast)  # 16.7 ms does not fit a 120 Hz budget
        self.assertIn("frame_p99_ms median", fast)
        # The proxy is judged on any panel.
        slow = self.one("scroll-30x1m", dict(metrics, frame_cost_p99_ms=9.0), display_hz=60)
        self.assertIn("Belum memenuhi — frame_cost_p99_ms median 9.0 ms", slow)

    def test_spilled_scroll_reports_the_trigger_of_prefetch(self):
        self.assertIn("Memenuhi target absolut", self.one("scroll-30x1m-spilled", {"hitch_ms_per_s": 0.9}))
        self.assertIn("Belum memenuhi", self.one("scroll-30x1m-spilled", {"hitch_ms_per_s": 1.4}))

    def test_open_is_judged_on_warm_opens_and_shows_the_cold_one_apart(self):
        row = self.one("open-500x10k", {"render_ms": [21, 22, 24, 25, 26, 27, 28, 29, 30, 32],
                                        "render_cold_ms": [44]})
        self.assertIn("Memenuhi target absolut", row)  # median 26.5; p95 32 is only shown
        self.assertIn("render_cold_ms median 44.0 ms", row)
        self.assertIn("render_ms p95", row)
        slow = self.one("open-500x10k", {"render_ms": [35, 36, 40], "render_cold_ms": [44]})
        self.assertIn("Belum memenuhi — render_ms median 36.0 ms", slow)

    def test_typing_is_valid_only_when_nothing_ran_uncoloured(self):
        for scenario in ("type-10k-plan", "type-2m"):
            ok = self.one(scenario, {"keystroke_main_p99_ms": 3.0, "uncoloured_turns_count": [0, 0]})
            self.assertIn("Memenuhi target absolut", ok, scenario)
            bad = self.one(scenario, {"keystroke_main_p99_ms": 3.0, "uncoloured_turns_count": [0, 200]})
            self.assertIn("[belum diukur] — 200 giliran ketikan tanpa warna", bad, scenario)
            self.assertNotIn("Memenuhi", bad, scenario)

    def test_an_old_type_2m_record_is_an_over_ceiling_artefact(self):
        row = self.one("type-2m", {"keystroke_main_p99_ms": 4.49, "characters_count": 2_000_000})
        self.assertIn("[belum diukur] — rekaman lama: artefak di atas plafon (over-ceiling artefact)", row)
        self.assertNotIn("Memenuhi", row)
        # The plan's fixture never had the old fixture's problem, so it only says what is missing.
        plan = self.one("type-10k-plan", {"keystroke_main_p99_ms": 3.0})
        self.assertIn("uncoloured_turns_count tidak tercatat", plan)
        self.assertNotIn("over-ceiling", plan)

    def test_e4_is_triggered_when_half_the_samples_pay_more_than_one_rtt(self):
        metrics = {"rerun_penalty_ms": [10, 40], "rerun_exceeds_rtt_ratio": [0, 1]}
        self.assertIn("Belum memenuhi — rerun_exceeds_rtt_ratio median 0.50", self.one("rerun-capped-rtt30", metrics))
        quiet = {"rerun_penalty_ms": [3, 4, 40], "rerun_exceeds_rtt_ratio": [0, 0, 1]}
        self.assertIn("Memenuhi target absolut", self.one("rerun-capped-rtt30", quiet))

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


class RowTables(unittest.TestCase):
    """The tables beside AXIS_ROWS name scenarios; a typo in one would silently grade nothing."""

    def test_the_side_tables_name_rows_that_exist(self):
        rows = {row[1]: row for row in bf.AXIS_ROWS}
        for table in (bf.TABLE_SQL, bf.CEILING_KIND, bf.VALIDITY, bf.OVER_CEILING_ARTEFACT, bf.TABLEPRO_NAME,
                      bf.LEGACY_SQL_KIND):
            self.assertLessEqual(set(table), set(rows))
        for scenario in bf.CEILING_KIND:
            self.assertEqual(rows[scenario][0], 2, scenario)
        self.assertLessEqual({metric for metric in bf.NEEDS_HZ},
                             {check[0] for row in bf.AXIS_ROWS for check in row[3]})

    def test_every_check_has_a_unit_suffix_the_report_and_bench_app_both_know(self):
        for row in bf.AXIS_ROWS:
            for metric, *_ in row[3]:
                self.assertTrue(metric.endswith(bench_app.UNIT_SUFFIXES), metric)

    def test_the_app_and_the_report_agree_on_scenario_and_metric_names(self):
        """BenchMode.swift is where the names come from, so the report is held to its spelling."""
        source = SWIFT.read_text("utf-8")
        in_cases = source[source.index("static let cases"):source.index("}()", source.index("static let cases"))]
        synthetic = re.search(r"static let synthetic = \[(.*?)\]", source, re.S).group(1)
        app_scenarios = set(re.findall(r'"([a-z0-9]+(?:-[a-z0-9]+)+)"', synthetic)) | \
            set(re.findall(r'^\s+"([a-z0-9]+(?:-[a-z0-9]+)+)":', in_cases, re.M))
        reported = {row[1] for row in bf.AXIS_ROWS}
        # What the report grades but the app does not run: the FFI bench, qhbench, and launch-cold.
        elsewhere = {"sort-numeric-500k", "sort-text-500k", "introspect-5000", "ffi-leak"}
        self.assertEqual(reported - app_scenarios - elsewhere, set())
        # What the app runs but the report has no row for (the tab scenarios are a leak check).
        self.assertEqual(app_scenarios - reported, {"tabs-100", "tabs-100-held"})
        for row in bf.AXIS_ROWS:
            if row[1] in elsewhere or row[1] == "launch-cold":
                continue
            for metric, *_ in row[3]:
                self.assertIn(f'"{metric}"', source, f"{row[1]}: {metric} is not a metric BenchMode prints")
        for scenario, (metric, *_) in bf.VALIDITY.items():
            self.assertIn(f'"{metric}"', source, scenario)


class Ceiling(unittest.TestCase):
    SAMPLES = [
        {"scenario": "rows-pg-table-500k", "app": "queryhive", "metrics": {"rows_per_s": 600_000.0},
         "notes": TABLE},
        {"scenario": "rows-pg-table-500k", "app": "queryhive", "metrics": {"rows_per_s": 610_000.0},
         "notes": TABLE},
    ]

    def record(self, kind=bench_ceiling.PG_KIND, rates=(690_282.0, 957_750.0, 1_074_898.0)):
        return bench_ceiling.ceiling_record(kind, list(rates), "SELECT * FROM wide_500k", 500_000)

    def test_record_is_the_median_of_the_runs(self):
        record = self.record()
        self.assertEqual(record["ceiling_rows_per_s"], 957_750.0)
        self.assertEqual(record["rates"], [690_282, 957_750, 1_074_898])
        self.assertEqual(record["ceiling_kind"], "same-sql-copy")

    def test_merge_adds_the_ceiling_to_every_sample_and_the_kind_to_the_first_notes(self):
        merged = bench_ceiling.merge_samples(self.SAMPLES, self.record())
        self.assertEqual([m["metrics"]["ceiling_rows_per_s"] for m in merged], [957_750.0] * 2)
        self.assertEqual(merged[0]["metrics"]["rows_per_s"], 600_000.0)
        self.assertIn("ceiling_kind=same-sql-copy", merged[0]["notes"])
        self.assertTrue(merged[0]["notes"].startswith(TABLE + "; ceiling_kind="))
        self.assertEqual(merged[1]["notes"], TABLE)
        self.assertNotIn("ceiling_rows_per_s", self.SAMPLES[0]["metrics"], "the input is not changed")

    def test_the_merged_samples_make_the_report_use_the_ceiling(self):
        slow = [dict(s, metrics={"rows_per_s": 300_000.0}) for s in self.SAMPLES]
        low = bench_ceiling.ceiling_record(bench_ceiling.PG_KIND, [400_000.0], "SELECT * FROM wide_500k", 500_000)
        records = bench_app.build_records(bench_ceiling.merge_samples(slow, low), source="app", host=HOST)
        row = next(l for l in render(records).splitlines() if l.startswith("| `rows-pg-table-500k`"))
        self.assertIn("Belum memenuhi", row)  # 300k against 0.8 * 400k = 320k
        self.assertIn("plafon same-sql-copy", row)
        fast = [dict(s, metrics={"rows_per_s": 330_000.0}) for s in self.SAMPLES]
        records = bench_app.build_records(bench_ceiling.merge_samples(fast, low), source="app", host=HOST)
        self.assertIn("Memenuhi target absolut",
                      next(l for l in render(records).splitlines() if l.startswith("| `rows-pg-table-500k`")))

    def test_a_ceiling_only_goes_into_the_scenarios_it_belongs_to(self):
        with self.assertRaises(SystemExit):
            bench_ceiling.merge_samples([{"scenario": "rows-lineitem-1m", "metrics": {}}], self.record())
        trino = self.record(bench_ceiling.TRINO_KIND)
        with self.assertRaises(SystemExit):
            bench_ceiling.merge_samples(self.SAMPLES, trino)
        self.assertEqual(len(bench_ceiling.merge_samples(
            [{"scenario": "rows-lineitem-1m", "metrics": {"rows_per_s": 1}}], trino)), 1)

    def test_mysql_has_no_ceiling_and_says_so(self):
        none = bench_ceiling.ceiling_record(bench_ceiling.NO_KIND, [], None, None)
        self.assertIsNone(none["ceiling_rows_per_s"])
        merged = bench_ceiling.merge_samples(
            [{"scenario": "rows-mysql-500k", "metrics": {"rows_per_s": 537_000.0}}], none)
        self.assertNotIn("ceiling_rows_per_s", merged[0]["metrics"])
        self.assertIn("ceiling_kind=none", merged[0]["notes"])
        with self.assertRaises(SystemExit):
            bench_ceiling.merge_samples(self.SAMPLES, none)

    def test_status_samples_pass_through(self):
        samples = [{"scenario": "rows-pg-table-500k", "status": "[belum diukur]", "notes": "no run"}]
        merged = bench_ceiling.merge_samples(samples, self.record())
        self.assertNotIn("metrics", merged[0])
        self.assertTrue(merged[0]["notes"].startswith("no run; ceiling_kind="))

    def test_read_samples_skips_log_lines(self):
        text = 'bench: start x\n{"scenario": "a"}\n\nnot json\n{"scenario": "b"}\n'
        self.assertEqual([s["scenario"] for s in bench_ceiling.read_samples(text)], ["a", "b"])
        with self.assertRaises(SystemExit):
            bench_ceiling.read_samples("{broken")

    def test_main_merges_stdin_into_stdout(self):
        saved = pathlib.Path(tempfile.mkdtemp()) / "ceiling.json"
        saved.write_text(json.dumps(self.record()), "utf-8")
        stdin, stdout = sys.stdin, sys.stdout
        sys.stdin = io.StringIO("\n".join(json.dumps(s) for s in self.SAMPLES) + "\n")
        sys.stdout = io.StringIO()
        try:
            self.assertEqual(bench_ceiling.main(["pg", "--merge", "--from", str(saved)]), 0)
            lines = sys.stdout.getvalue().splitlines()
        finally:
            sys.stdin, sys.stdout = stdin, stdout
        self.assertEqual(len(lines), 2)
        self.assertEqual(json.loads(lines[0])["metrics"]["ceiling_rows_per_s"], 957_750.0)


class FakeTrino(http.server.BaseHTTPRequestHandler):
    """A Trino that answers a statement with `pages` pages, each carrying `data`, chained by nextUri."""

    pages = 5
    data = b'"data":[[1,"aaaaaaaaaaaaaaaa",3.5],[2,"bbbbbbbbbbbbbbbb",4.5]]'
    served = []
    fail_once = set()

    def log_message(self, *args):
        pass

    def reply(self, number):
        port = self.server.server_address[1]
        last = number >= self.pages
        head = b'{"id":"q1","infoUri":"http://x/ui/query.html?q1"'
        if not last:
            head += b',"nextUri":"http://127.0.0.1:%d/v1/statement/executing/q1/%d"' % (port, number + 1)
        state = b"FINISHED" if last else b"RUNNING"
        body = head + b',"columns":[],' + self.data + b',"stats":{"state":"' + state + b'"}}'
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        type(self).served.append(("POST", self.path, self.headers.get("X-Trino-User"),
                                  self.headers.get("X-Trino-Catalog"), self.headers.get("Accept-Encoding")))
        self.reply(1)

    def do_GET(self):
        number = int(self.path.rsplit("/", 1)[1])
        type(self).served.append(("GET", self.path))
        if number in self.fail_once:
            self.fail_once.discard(number)
            self.send_response(503)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        self.reply(number + 0)


class TrinoDrain(unittest.TestCase):
    def setUp(self):
        FakeTrino.served = []
        FakeTrino.fail_once = set()
        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), FakeTrino)
        self.port = self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.addCleanup(self.server.server_close)
        self.addCleanup(self.server.shutdown)

    def test_follows_nexturi_to_the_last_page_and_reads_every_byte(self):
        elapsed, total = bench_ceiling.drain_trino("127.0.0.1", self.port, "qh", "SELECT 1", "tpch", "sf1")
        self.assertGreater(elapsed, 0)
        gets = [s for s in FakeTrino.served if s[0] == "GET"]
        self.assertEqual(len(gets), FakeTrino.pages - 1)
        self.assertGreater(total, FakeTrino.pages * len(FakeTrino.data))
        post = FakeTrino.served[0]
        self.assertEqual(post[1:], ("/v1/statement", "qh", "tpch", "identity"))

    def test_a_503_is_retried_not_taken_for_the_end(self):
        FakeTrino.fail_once = {3}
        bench_ceiling.drain_trino("127.0.0.1", self.port, "qh", "SELECT 1")
        self.assertEqual(sum(1 for s in FakeTrino.served if s[:2] == ("GET", "/v1/statement/executing/q1/3")), 2)

    def test_a_row_that_looks_like_nexturi_is_not_followed(self):
        FakeTrino.pages = 2
        FakeTrino.data = b'"data":[["' + b'"nextUri":"http://127.0.0.1:1/x"'.replace(b'"', b'\\"') + b'"]]'
        self.addCleanup(lambda: setattr(FakeTrino, "pages", 5))
        self.addCleanup(lambda: setattr(FakeTrino, "data", b'"data":[[1,"aaaaaaaaaaaaaaaa",3.5],[2,"bbbbbbbbbbbbbbbb",4.5]]'))
        bench_ceiling.drain_trino("127.0.0.1", self.port, "qh", "SELECT 1")
        self.assertEqual(sum(1 for s in FakeTrino.served if s[0] == "GET"), 1)

    def test_a_drain_that_stops_early_is_refused(self):
        FakeTrino.pages = 1
        self.addCleanup(lambda: setattr(FakeTrino, "pages", 5))
        trino = bf.CONNECTIONS["trino"]
        saved = dict(trino)
        trino.update(DB_HOST="127.0.0.1", DB_PORT=str(self.port))
        self.addCleanup(lambda: trino.update(saved))
        with self.assertRaises(SystemExit):  # one small page cannot be a million rows
            bench_ceiling.drain_rates("SELECT 1", 1_000_000, 1, "tpch", "sf1")


class ResultsFile(unittest.TestCase):
    def test_existing_results_file_still_loads_and_renders(self):
        results = bf.load_results()
        self.assertTrue(results)
        text = render(results)
        self.assertIn("## Engine Rust", text)
        self.assertNotIn("\n\n\n", text)


if __name__ == "__main__":
    unittest.main()
