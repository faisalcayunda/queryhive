import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// The plan parser: the rules the blueprint locks (W13 §5.1), on plans captured from the dev
/// servers plus a few small synthetic ones for shapes the dev servers did not produce.
final class QueryPlanTests: XCTestCase {
    private func all(_ nodes: [PlanNode]) -> [PlanNode] {
        nodes.flatMap { [$0] + all($0.children) }
    }

    private func pg(_ plan: String) -> String { "[{\"Plan\": \(plan)}]" }

    // MARK: PostgreSQL

    func testPlainPlanRanksByExclusiveCostAndSharesSumToOne() throws {
        let plan = try XCTUnwrap(QueryPlan.parse(PlanFixtures.pgJoinPlain))
        XCTAssertEqual(plan.dialect, .postgres)
        XCTAssertEqual(plan.hotMetric, .estimatedCost)
        let nodes = all(plan.roots)
        XCTAssertEqual(nodes.count, plan.nodeCount)
        XCTAssertEqual(nodes.map(\.id), Array(0..<nodes.count))
        XCTAssertTrue(nodes.allSatisfy { ($0.cost ?? 0) >= 0 && ($0.share ?? 0) <= 1 })
        XCTAssertEqual(nodes.compactMap(\.share).reduce(0, +), 1, accuracy: 1e-9)
        XCTAssertNil(plan.executionMillis)
    }

    func testLimitOverCostlierChildStaysUnderOne() throws {
        // Limit's Total Cost is below its child's; exclusive is clamped and share stays in 0...1.
        let plan = try XCTUnwrap(QueryPlan.parse(PlanFixtures.pgAnalyzeLimitSubPlan))
        XCTAssertEqual(plan.hotMetric, .selfTime)
        XCTAssertNotNil(plan.executionMillis)
        XCTAssertTrue(all(plan.roots).allSatisfy { ($0.share ?? 0) <= 1 && ($0.selfMillis ?? 0) >= 0 })
    }

    func testTotalIsAveragePerLoopTimesLoops() throws {
        let text = pg(#"""
            {"Node Type":"Nested Loop","Plan Rows":1,"Actual Rows":4,"Actual Loops":1,"Actual Total Time":10.0,
             "Plans":[{"Node Type":"Seq Scan","Parent Relationship":"Inner","Relation Name":"t","Plan Rows":2,
                       "Actual Rows":2,"Actual Loops":3,"Actual Total Time":2.0}]}
            """#)
        let root = try XCTUnwrap(QueryPlan.parse(text)).roots[0]
        XCTAssertEqual(root.children[0].actualRows, 6)
        XCTAssertEqual(root.children[0].selfMillis ?? -1, 6, accuracy: 1e-9)
        XCTAssertEqual(root.selfMillis ?? -1, 4, accuracy: 1e-9)
        XCTAssertEqual(root.children[0].subtitle, "t")
    }

    func testExclusiveTimeIsNeverNegative() throws {
        let text = pg(#"""
            {"Node Type":"Sort","Actual Loops":1,"Actual Total Time":1.0,
             "Plans":[{"Node Type":"Seq Scan","Actual Loops":1,"Actual Total Time":5.0}]}
            """#)
        XCTAssertEqual(try XCTUnwrap(QueryPlan.parse(text)).roots[0].selfMillis, 0)
    }

    func testInitPlanIsNotSubtractedButSubPlanIs() throws {
        func parent(_ relationship: String) throws -> Double {
            let text = pg(#"""
                {"Node Type":"Result","Actual Loops":1,"Actual Total Time":10.0,
                 "Plans":[{"Node Type":"Aggregate","Parent Relationship":"\#(relationship)","Actual Loops":1,"Actual Total Time":4.0}]}
                """#)
            return try XCTUnwrap(QueryPlan.parse(text)).roots[0].selfMillis ?? -1
        }
        XCTAssertEqual(try parent("InitPlan"), 10, accuracy: 1e-9)
        XCTAssertEqual(try parent("SubPlan"), 6, accuracy: 1e-9)
    }

    func testParallelWorkersAreNotSummed() throws {
        // Three processes ran the scan side by side for 4 ms each: the Gather's wall time is
        // 5 ms, so the scan is 4 ms and not 12.
        let text = pg(#"""
            {"Node Type":"Gather","Workers Launched":2,"Actual Loops":1,"Actual Total Time":5.0,
             "Plans":[{"Node Type":"Seq Scan","Parallel Aware":true,"Actual Loops":3,"Actual Total Time":4.0}]}
            """#)
        let root = try XCTUnwrap(QueryPlan.parse(text)).roots[0]
        XCTAssertEqual(root.children[0].selfMillis ?? -1, 4, accuracy: 1e-9)
        XCTAssertEqual(root.selfMillis ?? -1, 1, accuracy: 1e-9)
    }

    func testNonParallelAwareNodesUnderGatherAreNotSummed() throws {
        let text = pg(#"""
            {"Node Type":"Gather","Workers Launched":2,"Actual Loops":1,"Actual Total Time":12.284,
             "Plans":[{"Node Type":"Aggregate","Parallel Aware":false,"Actual Loops":3,"Actual Total Time":4.4,
               "Plans":[{"Node Type":"Nested Loop","Parallel Aware":false,"Actual Loops":3,"Actual Total Time":4.111,
                 "Plans":[{"Node Type":"Seq Scan","Parallel Aware":true,"Actual Loops":3,"Actual Total Time":2.449}]}]}]}
            """#)
        let root = try XCTUnwrap(QueryPlan.parse(text)).roots[0]
        XCTAssertEqual(root.selfMillis ?? -1, 12.284 - 4.4, accuracy: 1e-9)
        XCTAssertEqual(root.children[0].children[0].selfMillis ?? -1, 4.111 - 2.449, accuracy: 1e-9)
    }

    func testInitPlanCostIsSubtractedFromTheParent() throws {
        let text = pg(#"""
            {"Node Type":"Limit","Total Cost":5094.02,
             "Plans":[{"Node Type":"Aggregate","Parent Relationship":"InitPlan","Total Cost":5094.01,
               "Plans":[{"Node Type":"Seq Scan","Parent Relationship":"Outer","Total Cost":4344.00}]}]}
            """#)
        let root = try XCTUnwrap(QueryPlan.parse(text)).roots[0]
        XCTAssertEqual(root.cost ?? -1, 0.01, accuracy: 1e-6)  // not 750
    }

    func testNothingIsMarkedWhenNoNodeReachesTwentyPercent() throws {
        func leaf(_ cost: Int) -> String { #"{"Node Type":"Seq Scan","Total Cost":\#(cost)}"# }
        // Ten sibling scans of equal cost under a free parent: 10% each.
        let kids = (0..<10).map { _ in leaf(10) }.joined(separator: ",")
        let text = pg(#"{"Node Type":"Append","Total Cost":100,"Plans":[\#(kids)]}"#)
        let plan = try XCTUnwrap(QueryPlan.parse(text))
        XCTAssertNil(plan.hottest)
    }

    func testTheHottestNodeIsTheLargestExclusive() throws {
        let text = pg(#"""
            {"Node Type":"Hash Join","Total Cost":100,
             "Plans":[{"Node Type":"Seq Scan","Total Cost":10},{"Node Type":"Seq Scan","Total Cost":80}]}
            """#)
        let plan = try XCTUnwrap(QueryPlan.parse(text))
        let hot = try XCTUnwrap(all(plan.roots).first { $0.id == plan.hottest })
        XCTAssertEqual(hot.cost, 80)
    }

    // MARK: Trino

    func testTrinoFragmentsAreLinkedThroughRemoteSource() throws {
        let plan = try XCTUnwrap(QueryPlan.parse(PlanFixtures.trinoDistributed))
        XCTAssertEqual(plan.dialect, .trino)
        XCTAssertEqual(plan.hotMetric, .estimatedCPU)
        XCTAssertEqual(plan.roots.count, 1)
        let nodes = all(plan.roots)
        XCTAssertEqual(nodes.count, plan.nodeCount)
        let remote = nodes.filter { $0.title.hasPrefix("Remote") }
        XCTAssertFalse(remote.isEmpty)
        XCTAssertTrue(remote.contains { !$0.children.isEmpty }, "a RemoteSource pulls its fragment in")
    }

    func testTrinoNaNAndEmptyEstimatesAreDropped() throws {
        let text = #"""
            {"0":{"id":"1","name":"Output","descriptor":{},"details":[],"estimates":[],"children":[
              {"id":"2","name":"TableScan","descriptor":{"table":"c:s:t"},"details":["d"],
               "estimates":[{"outputRowCount":"NaN","outputSizeInBytes":"NaN","cpuCost":"NaN","memoryCost":"NaN","networkCost":"NaN"}],
               "children":[]}]}}
            """#
        let plan = try XCTUnwrap(QueryPlan.parse(text))
        let nodes = all(plan.roots)
        XCTAssertTrue(nodes.allSatisfy { $0.estimatedRows == nil && $0.cost == nil && $0.share == nil })
        XCTAssertNil(plan.hottest)
        XCTAssertEqual(nodes[1].subtitle, "c:s:t")
    }

    func testTrinoHottestIsHighestCPUWithRowsAsTieBreak() throws {
        func scan(_ name: String, rows: Int) -> String {
            #"{"id":"\#(name)","name":"\#(name)","descriptor":{},"details":[],"estimates":[{"outputRowCount":\#(rows),"cpuCost":50.0}],"children":[]}"#
        }
        let text = #"{"0":{"id":"r","name":"Join","descriptor":{},"details":[],"estimates":[{"outputRowCount":1,"cpuCost":0.0}],"children":[\#(scan("A", rows: 5)),\#(scan("B", rows: 9))]}}"#
        let plan = try XCTUnwrap(QueryPlan.parse(text))
        let hot = try XCTUnwrap(all(plan.roots).first { $0.id == plan.hottest })
        XCTAssertEqual(hot.title, "B")
    }

    func testFragmentCycleTerminates() throws {
        let text = #"""
            {"0":{"id":"a","name":"RemoteSource","descriptor":{"sourceFragmentIds":"[1]"},"details":[],"estimates":[],"children":[]},
             "1":{"id":"b","name":"RemoteSource","descriptor":{"sourceFragmentIds":"[0, 1]"},"details":[],"estimates":[],"children":[]}}
            """#
        let plan = try XCTUnwrap(QueryPlan.parse(text))
        XCTAssertEqual(plan.nodeCount, 2)
    }

    func testLogicalPlanParses() throws {
        let plan = try XCTUnwrap(QueryPlan.parse(PlanFixtures.trinoLogical))
        XCTAssertGreaterThan(plan.nodeCount, 0)
    }

    // MARK: Untrusted input

    func testBrokenInputIsNilWithoutCrashing() {
        let truncated = String(PlanFixtures.pgJoinPlain.prefix(120))
        for text in ["", "null", "[]", "{}", "42", #"{"foreign":1}"#, #"[{"Plan":5}]"#, truncated, "Seq Scan on t"] {
            XCTAssertNil(QueryPlan.parse(text), text)
        }
    }

    func testDepthBeyondTheCapIsNil() {
        func nest(_ depth: Int) -> String {
            String(repeating: #"{"Node Type":"Result","Plans":["#, count: depth)
                + #"{"Node Type":"Result"}"# + String(repeating: "]}", count: depth)
        }
        XCTAssertNotNil(QueryPlan.parse(pg(nest(100))))
        XCTAssertNil(QueryPlan.parse(pg(nest(300))))
    }

    func testNodeCountBeyondTheCapIsNil() {
        let kids = [String](repeating: #"{"Node Type":"Result"}"#, count: QueryPlan.nodeLimit + 1)
        XCTAssertNil(QueryPlan.parse(pg(#"{"Node Type":"Append","Plans":[\#(kids.joined(separator: ","))]}"#)))
    }

    func testFiveThousandNodesParseQuickly() throws {
        let kids = [String](repeating: #"{"Node Type":"Seq Scan","Total Cost":1.0,"Plan Rows":1}"#, count: 5_000)
        let text = pg(#"{"Node Type":"Append","Total Cost":5000.0,"Plans":[\#(kids.joined(separator: ","))]}"#)
        let start = Date()
        let plan = try XCTUnwrap(QueryPlan.parse(text))
        XCTAssertEqual(plan.nodeCount, 5_001)
        // The blueprint's target is 50 ms; the bound here is loose so a busy machine does not flake.
        XCTAssertLessThan(Date().timeIntervalSince(start), 0.5)
    }

    // MARK: Words

    func testSpokenLabelNamesTheHottestNode() throws {
        let plan = try XCTUnwrap(QueryPlan.parse(PlanFixtures.pgAnalyzeJoin))
        let nodes = all(plan.roots)
        if let hot = nodes.first(where: { $0.id == plan.hottest }) {
            XCTAssertTrue(PlanText.spoken(hot, plan, hot: true).hasSuffix("hottest node"))
        }
        XCTAssertFalse(PlanText.describe(plan).isEmpty)
    }

    /// The view lays out for both dialects and exposes the plan to accessibility (drawn, no baseline).
    @MainActor func testTheTreeViewLaysOut() throws {
        for text in [PlanFixtures.pgAnalyzeJoin, PlanFixtures.trinoDistributed] {
            let plan = try XCTUnwrap(QueryPlan.parse(text))
            let host = NSHostingView(rootView: PlanTreeView(plan: plan, raw: text))
            host.frame = NSRect(x: 0, y: 0, width: 640, height: 360)
            host.layoutSubtreeIfNeeded()
            let rep = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
            host.cacheDisplay(in: host.bounds, to: rep)
        }
    }
}
