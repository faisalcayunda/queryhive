import Foundation
import XCTest

@testable import QueryHive

/// The parts of `--bench` that need no window, no database and no clock: which scenarios exist and
/// what SQL each one runs, what a Run's stamps turn into, and the fixtures the editor scenarios type
/// into. What cannot be checked here (the timing itself) is run once per scenario by whoever
/// integrates a change to `BenchMode`.
final class BenchScenarioTests: XCTestCase {
    // MARK: Scenarios

    func testEveryScenarioTheDecisionNamesExists() {
        let wanted = [
            "ttfr-s1t-1k", "ttfr-s1t-10k", "ttfr-s2t-500k", "rows-pg-table-500k", "ttfr-s3-rtt30",
            "ttfr-s4-first-run", "rerun-capped", "rerun-capped-rtt30", "rows-lineitem-1m", "mem-5m",
            "mem-mysql-500k", "cancel-mysql-sleep", "cancel-stream-wide", "cancel-trino-heavy",
            "scroll-30x1m-spilled", "type-10k-plan",
            // The regression guard keeps its names.
            "ttfr-s1-1k", "ttfr-s1-10k", "rows-wide-500k", "mem-500k", "rows-mysql-500k", "rows-trino-500k",
            "cancel-pg-sleep", "type-10k", "type-2m", "type-coloured-195k", "open-500x10k",
        ]
        let known = Set(BenchMode.synthetic + BenchMode.database)
        for name in wanted { XCTAssertTrue(known.contains(name), name) }
        XCTAssertEqual(BenchMode.database.count, Set(BenchMode.database).count)
    }

    func testGatedScenariosRunThePlansSQL() throws {
        let caps = ["ttfr-s1t-1k": 1_000, "ttfr-s1t-10k": 10_000, "ttfr-s2t-500k": 500_000,
                    "rows-pg-table-500k": 500_000, "ttfr-s3-rtt30": 1_000, "ttfr-s4-first-run": 1_000]
        for (name, cap) in caps {
            let spec = try XCTUnwrap(BenchMode.cases[name], name)
            XCTAssertEqual(spec.sql(.postgres), "SELECT * FROM wide_500k", name)
            XCTAssertEqual(spec.sqlKind, "table", name)
            XCTAssertEqual(spec.cap, cap, name)
            XCTAssertEqual(spec.kinds, [.postgres], name)
        }
        XCTAssertEqual(BenchMode.cases["rows-mysql-500k"]?.sql(.mysql), "SELECT * FROM wide_500k")
        XCTAssertEqual(BenchMode.cases["rows-lineitem-1m"]?.sql(.trino), "SELECT * FROM tpch.sf1.lineitem LIMIT 1000000")
        XCTAssertEqual(BenchMode.cases["rows-lineitem-1m"]?.cap, 1_000_000)
    }

    func testGeneratedScenariosKeepTheirSQLAndSayTheyAreGenerated() throws {
        let guardRails = [("ttfr-s1-1k", 1_000, 3), ("ttfr-s1-10k", 10_000, 3),
                          ("rows-wide-500k", 500_000, 30), ("mem-500k", 500_000, 30)]
        for (name, rows, columns) in guardRails {
            let spec = try XCTUnwrap(BenchMode.cases[name], name)
            XCTAssertEqual(spec.sqlKind, "generated", name)
            XCTAssertEqual(spec.cap, rows, name)
            XCTAssertEqual(spec.sql(.postgres), BenchMode.generatedSQL(.postgres, rows: rows, columns: columns), name)
            XCTAssertNil(spec.sql(.mysql), name)
        }
        XCTAssertNil(BenchMode.cases["rows-wide-500k"]?.sql(.trino), "Trino's sequence stops at 10,000")
        XCTAssertNotNil(BenchMode.cases["ttfr-s1-1k"]?.sql(.trino))
    }

    func testToxiproxyAndFlows() throws {
        XCTAssertEqual(BenchMode.cases.filter { $0.value.viaToxiproxy }.keys.sorted(),
                       ["rerun-capped-rtt30", "ttfr-s3-rtt30"])
        XCTAssertTrue(try XCTUnwrap(BenchMode.cases["ttfr-s3-rtt30"]).warmUp)
        guard case .firstRun = try XCTUnwrap(BenchMode.cases["ttfr-s4-first-run"]).flow else {
            return XCTFail("S4 is one fresh process per sample")
        }
        XCTAssertFalse(try XCTUnwrap(BenchMode.cases["ttfr-s4-first-run"]).warmUp, "the first Run is the cold one")
        guard case .rerun = try XCTUnwrap(BenchMode.cases["rerun-capped"]).flow else { return XCTFail("rerun") }
        guard case .cancelMidStream(let rows) = try XCTUnwrap(BenchMode.cases["cancel-stream-wide"]).flow else {
            return XCTFail("cancel-stream-wide stops a stream in the middle")
        }
        XCTAssertLessThan(rows, try XCTUnwrap(BenchMode.cases["cancel-stream-wide"]).cap)
    }

    func testKindGuards() throws {
        XCTAssertEqual(BenchMode.cases["cancel-mysql-sleep"]?.kinds, [.mysql])
        XCTAssertEqual(BenchMode.cases["cancel-mysql-sleep"]?.sql(.mysql), "SELECT SLEEP(30)")
        XCTAssertEqual(BenchMode.cases["cancel-pg-sleep"]?.sql(.postgres), "SELECT pg_sleep(30)")
        XCTAssertEqual(BenchMode.cases["cancel-trino-heavy"]?.kinds, [.trino])
        XCTAssertEqual(BenchMode.cases["mem-mysql-500k"]?.kinds, [.mysql])
        let five = try XCTUnwrap(BenchMode.cases["mem-5m"])
        XCTAssertEqual(five.cap, 5_000_000)
        XCTAssertTrue(five.sql(.postgres)?.contains("generate_series(1, 10)") == true)
        XCTAssertEqual(five.sql(.trino), "SELECT * FROM tpch.sf1.lineitem LIMIT 5000000")
        XCTAssertNil(five.sql(.mysql))
    }

    func testRecordNotes() {
        XCTAssertEqual(BenchMode.recordNotes([("sql_kind", "table"), ("via", "toxiproxy:55435"), ("rtt_ms", "30")]),
                       "sql_kind=table; via=toxiproxy:55435; rtt_ms=30")
    }

    // MARK: Stage stamps

    func testStageMetricsTurnStampsIntoOffsetsAndTheFiveHypotheses() {
        let stamps: [String: CFAbsoluteTime] = [
            "run": 100.0, "engine.columns": 100.010, "columns": 100.012, "firstRows": 100.020,
            "engine.done": 100.021, "runDone": 100.0235, "gridAttached": 100.045, "firstDraw": 100.050,
            "firstPaint": 100.056,
        ]
        let metrics = BenchMode.stageMetrics(stamps)
        XCTAssertEqual(metrics["stage_engine_columns_ms"] ?? .nan, 10, accuracy: 1e-6)
        XCTAssertEqual(metrics["stage_columns_ms"] ?? .nan, 12, accuracy: 1e-6)
        XCTAssertEqual(metrics["stage_first_rows_ms"] ?? .nan, 20, accuracy: 1e-6)
        XCTAssertEqual(metrics["stage_engine_done_ms"] ?? .nan, 21, accuracy: 1e-6)
        XCTAssertEqual(metrics["stage_run_done_ms"] ?? .nan, 23.5, accuracy: 1e-6)
        XCTAssertEqual(metrics["stage_grid_attached_ms"] ?? .nan, 45, accuracy: 1e-6)
        XCTAssertEqual(metrics["stage_first_draw_ms"] ?? .nan, 50, accuracy: 1e-6)
        XCTAssertEqual(metrics["stage_first_paint_ms"] ?? .nan, 56, accuracy: 1e-6)
        XCTAssertEqual(metrics["delta_grid_attach_ms"] ?? .nan, 33, accuracy: 1e-6, "H1: gridAttached - columns")
        XCTAssertEqual(metrics["delta_done_hop_ms"] ?? .nan, 2.5, accuracy: 1e-6, "H2: runDone - engine.done")
        XCTAssertEqual(metrics["delta_first_draw_ms"] ?? .nan, 30, accuracy: 1e-6, "H3: firstDraw - firstRows")
        XCTAssertEqual(metrics["delta_flush_ms"] ?? .nan, 6, accuracy: 1e-6, "H4: firstPaint - firstDraw")
        XCTAssertEqual(metrics["first_draw_after_done_count"], 1, "H5: nothing was drawn before the engine finished")
        XCTAssertNil(metrics["stage_run_ms"])
    }

    func testStageMetricsLeaveOutWhatNobodyStamped() {
        XCTAssertEqual(Set(BenchMode.stageMetrics(["run": 5, "firstPaint": 5.03]).keys), ["stage_first_paint_ms"])
        XCTAssertEqual(BenchMode.stageMetrics(["firstPaint": 5.03]), [:], "no run, no offsets")
        let early = BenchMode.stageMetrics(["run": 0, "engine.done": 0.050, "firstDraw": 0.020])
        XCTAssertEqual(early["first_draw_after_done_count"], 0, "H5: a paint came before the engine finished")
    }

    func testStageMetricNames() {
        XCTAssertEqual(PerfSignposts.Stage.allCases.map(\.metric),
                       ["run", "engine_columns", "engine_progress", "columns", "grid_attached", "first_rows",
                        "first_draw", "engine_done", "run_done", "first_paint"])
        XCTAssertEqual(PerfSignposts.snakeCase("replaceAndRuler"), "replace_and_ruler")
        XCTAssertEqual(PerfSignposts.snakeCase("apply.layout"), "apply_layout")
        XCTAssertEqual(PerfSignposts.snakeCase("tempAttrAdd"), "temp_attr_add")
    }

    /// The stamp table is process-wide: switch recording on for the test and put it back.
    func testClearStagesDropsEveryStageAndTheEnginesButNotTheCancelStamps() {
        PerfSignposts.recording = true
        defer { PerfSignposts.recording = false; PerfSignposts.clearStages(); PerfSignposts.clear("cancel", "cancelEnd") }
        for stage in PerfSignposts.Stage.allCases { PerfSignposts.stamp(stage) }
        PerfSignposts.stamp("engine.rows")
        PerfSignposts.stamp("cancel")
        XCTAssertNotNil(PerfSignposts.time(of: .engineColumns))
        PerfSignposts.clearStages()
        for stage in PerfSignposts.Stage.allCases { XCTAssertNil(PerfSignposts.time(of: stage), stage.rawValue) }
        XCTAssertNil(PerfSignposts.time(of: "engine.rows"))
        XCTAssertNotNil(PerfSignposts.time(of: "cancel"), "a cancel is not a stage of a Run")
    }

    func testOnlyFirstStampSurvivesUntilItIsCleared() {
        PerfSignposts.recording = true
        defer { PerfSignposts.recording = false; PerfSignposts.clearStages() }
        PerfSignposts.stamp(.engineProgress, onlyFirst: true)
        let first = PerfSignposts.time(of: .engineProgress)
        PerfSignposts.stamp(.engineProgress, onlyFirst: true)
        XCTAssertEqual(PerfSignposts.time(of: .engineProgress), first)
        PerfSignposts.clearStages()
        XCTAssertNil(PerfSignposts.time(of: .engineProgress), "the next repeat starts empty")
    }

    // MARK: Parts

    func testPartTimesItsBodyAndCountsOnlyWhileRecording() {
        PerfSignposts.recording = false
        PerfSignposts.partsReset()
        XCTAssertEqual(PerfSignposts.part("layout") { 7 }, 7)
        PerfSignposts.count("tempAttrAdd")
        XCTAssertTrue(PerfSignposts.partsSnapshot().isEmpty, "nothing is kept without --bench")

        PerfSignposts.recording = true
        defer { PerfSignposts.recording = false; PerfSignposts.partsReset() }
        XCTAssertEqual(PerfSignposts.part(PerfSignposts.Part.layout) { 9 }, 9)
        PerfSignposts.part(PerfSignposts.Part.layout) { Thread.sleep(forTimeInterval: 0.002) }
        PerfSignposts.count(PerfSignposts.Part.tempAttrAdd, by: 3)
        let parts = PerfSignposts.partsSnapshot()
        XCTAssertEqual(parts["layout"]?.count, 2)
        XCTAssertGreaterThanOrEqual(parts["layout"]?.ms ?? 0, 1.5)
        XCTAssertEqual(parts["tempAttrAdd"], PerfSignposts.PartTotal(ms: 0, count: 3))
        PerfSignposts.partsReset()
        XCTAssertTrue(PerfSignposts.partsSnapshot().isEmpty)
    }

    func testPartMetricsCountAMissingKeyAsZero() {
        typealias Total = PerfSignposts.PartTotal
        let turns: [[String: Total]] = [
            ["layout": Total(ms: 2, count: 1)],
            ["layout": Total(ms: 4, count: 1), "tempAttrAdd": Total(ms: 0, count: 3)],
            [:],
        ]
        let metrics = BenchMode.partMetrics(turns)
        XCTAssertEqual(metrics["part_layout_p50_ms"], 2)
        XCTAssertEqual(metrics["part_layout_p99_ms"], 4)
        XCTAssertEqual(metrics["part_layout_count"] ?? .nan, 2.0 / 3.0, accuracy: 1e-9)
        XCTAssertEqual(metrics["part_temp_attr_add_count"], 1)
        XCTAssertNil(metrics["part_temp_attr_add_p50_ms"], "a part that only counts has no times")
        XCTAssertEqual(BenchMode.partMetrics([]), [:])
    }

    // MARK: Re-run

    func testRerunPenaltyIsJudgedAgainstOneRoundTrip() {
        let slow = BenchMode.rerunMetrics(uncapped: 40, warm: 42, capped: 75, sort: 120, rttMS: 30)
        XCTAssertEqual(slow["rerun_penalty_ms"], 35)
        XCTAssertEqual(slow["rerun_penalty_capped_ms"], 33)
        XCTAssertEqual(slow["rerun_exceeds_rtt_ratio"], 1)
        XCTAssertEqual(slow["rerun_sort_ttfr_ms"], 120)
        let fine = BenchMode.rerunMetrics(uncapped: 40, warm: 42, capped: 60, sort: 90, rttMS: 30)
        XCTAssertEqual(fine["rerun_exceeds_rtt_ratio"], 0)
        // On loopback one round trip is taken as 1 ms.
        XCTAssertEqual(BenchMode.rerunMetrics(uncapped: 20, warm: 20, capped: 20.5, sort: 30, rttMS: 0)["rerun_exceeds_rtt_ratio"], 0)
        XCTAssertEqual(BenchMode.rerunMetrics(uncapped: 20, warm: 20, capped: 22, sort: 30, rttMS: 0)["rerun_exceeds_rtt_ratio"], 1)
    }

    // MARK: Editor fixtures

    func testPlanDocumentIsTenThousandShortLines() {
        let document = BenchMode.planDocument(lines: 10_000)
        let length = (document as NSString).length
        XCTAssertTrue((380_000...420_000).contains(length), "about 400k characters, got \(length)")
        let lines = document.split(separator: "\n", omittingEmptySubsequences: false)
        XCTAssertEqual(lines.count, 10_001, "10,000 lines and the empty tail after the last newline")
        XCTAssertTrue(lines.dropLast().allSatisfy { $0.hasPrefix("SELECT") && $0.hasSuffix(";") })
        // The dense fixture is what type-10k has always typed into, and stays so.
        XCTAssertGreaterThan((BenchMode.sqlDocument(lines: 10_000, minimumCharacters: 0) as NSString).length, 900_000)
    }

    func testFitToCeilingLeavesRoomForTheTypedCharacters() {
        let ceiling = 2_000_000
        let long = BenchMode.sqlDocument(lines: 0, minimumCharacters: ceiling)
        XCTAssertGreaterThan((long as NSString).length, ceiling - BenchMode.ceilingMargin)
        XCTAssertEqual((BenchMode.fitToCeiling(long, ceiling: ceiling) as NSString).length, ceiling - 4_096)
        let short = BenchMode.planDocument(lines: 100)
        XCTAssertEqual(BenchMode.fitToCeiling(short, ceiling: ceiling), short)
        XCTAssertEqual(BenchMode.fitToCeiling("abc", ceiling: 10), "")
    }

    func testProbeIsTheLineBeforeTheOneTypedOn() {
        let text = "SELECT 1;\nSELECT 2;\nSELECT 3;\n" as NSString
        XCTAssertEqual(BenchMode.probeIndex(in: text, before: 20), 10, "start of line 3 -> line 2")
        XCTAssertEqual(BenchMode.probeIndex(in: text, before: 25), 10, "inside line 3 -> line 2")
        XCTAssertEqual(BenchMode.probeIndex(in: text, before: 10), 0)
        XCTAssertNil(BenchMode.probeIndex(in: text, before: 3), "nothing before the first line")
        XCTAssertNil(BenchMode.probeIndex(in: "" as NSString, before: 0))
    }

    func testPercentileMatchesWhatTheRecordsWereBuiltWith() {
        XCTAssertEqual(BenchMode.percentile([4, 1, 3, 2], 0.5), 2)
        XCTAssertEqual(BenchMode.percentile([1, 2, 3, 4], 0.99), 4)
        XCTAssertTrue(BenchMode.percentile([], 0.5).isNaN)
    }
}
