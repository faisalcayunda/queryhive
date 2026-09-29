import XCTest

@testable import QueryHive

/// The change queue and the one plan the review sheet and the run both read.
///
/// The phase's criterion is here: **the statement that was reviewed is the statement that ran.**
/// The plan is built once, the sheet renders `plan.sql`, and the engine is handed `plan.payload`;
/// this file proves those two are the same statements, so there is no second generation path that
/// could show one SQL and send another.
final class WritePlanTests: XCTestCase {
    private let rows: [[String?]] = [
        ["1", "KPM Sukamaju", "4"],
        ["2", "KPM Cibadak", "2"],
    ]
    private let columns = [
        Event.Column(name: "id", type: "bigint"),
        Event.Column(name: "nama", type: "varchar"),
        Event.Column(name: "jumlah_jiwa", type: "bigint"),
    ]

    private func build(_ edits: CellEdits) -> WritePlan {
        WritePlan.build(edits: edits, rows: rows, columns: columns,
                        table: "public.penerima", kind: .postgres)
    }

    func testAnEmptyQueueBuildsAnEmptyPlan() {
        XCTAssertTrue(build(CellEdits()).isEmpty)
    }

    func testAPlanWithNoTableIsEmptyAndSaysNothing() {
        // A hand-written query has no `sourceTable`; there is nothing to write back to, and the
        // sheet says so rather than showing a statement against a guessed table.
        let edits = CellEdits()
        let plan = WritePlan.build(edits: edits, rows: rows, columns: columns, table: nil, kind: .postgres)
        XCTAssertNil(plan.table)
        XCTAssertTrue(plan.isEmpty)
    }

    func testDeletesRunBeforeUpdatesAndInsertsLast() {
        // The order the source study records: a delete that frees a unique value has to run before
        // the insert that takes it, so the plan is delete → update → insert.
        var edits = CellEdits()
        edits.edit("KPM Baru", at: CellKey(row: 0, column: 1), original: "KPM Sukamaju")
        edits.deleteRow(1)
        let inserted = edits.insertRow()
        edits.setInserted("9", row: inserted, column: 0)
        edits.setInserted("KPM Mekarsari", row: inserted, column: 1)

        let plan = build(edits)

        XCTAssertEqual(plan.statements.map(\.kind), [.delete, .update, .insert])
    }

    func testTheReviewedStatementsAreTheStatementsTheEngineIsHanded() throws {
        var edits = CellEdits()
        edits.edit("KPM Baru", at: CellKey(row: 0, column: 1), original: "KPM Sukamaju")
        edits.deleteRow(1)
        let inserted = edits.insertRow()
        edits.setInserted("9", row: inserted, column: 0)

        let plan = build(edits)
        let reviewed = plan.sql

        // The payload is the only encoder of the plan, and it is what `apply_changes` reads. If it
        // ever grows a second code path, this fails.
        let data = try XCTUnwrap(plan.payload.data(using: .utf8))
        let decoded = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [[String: Any]])
        let ran = decoded.compactMap { $0["sql"] as? String }

        XCTAssertEqual(ran, reviewed)
        XCTAssertEqual(ran.count, 3)
    }

    func testBuildingFromTheSameQueueTwiceGivesTheSameStatements() {
        // Stability is what makes "reviewed == ran" hold across the moment between showing the
        // sheet and pressing Run: the plan is a value, and rebuilding it does not reorder or change
        // anything.
        var edits = CellEdits()
        edits.edit("A", at: CellKey(row: 0, column: 1), original: "KPM Sukamaju")
        edits.deleteRow(0)
        XCTAssertEqual(build(edits), build(edits))
    }

    func testDeletingARowDropsItsStagedEdits() {
        var edits = CellEdits()
        edits.edit("KPM Baru", at: CellKey(row: 1, column: 1), original: "KPM Cibadak")
        edits.deleteRow(1)

        let plan = build(edits)

        XCTAssertTrue(plan.statements.allSatisfy { $0.kind == .delete })
        XCTAssertEqual(plan.statements.count, 1)
    }

    func testAnInsertQuotesItsIdentifiersAndEscapesItsText() {
        var edits = CellEdits()
        let inserted = edits.insertRow()
        edits.setInserted("O'Brien", row: inserted, column: 1)

        let plan = build(edits)

        let insert = try? XCTUnwrap(plan.statements.first)
        XCTAssertEqual(insert?.kind, .insert)
        XCTAssertTrue(insert?.sql.contains("\"id\"") == true, insert?.sql ?? "")
        XCTAssertTrue(insert?.sql.contains("'O''Brien'") == true, insert?.sql ?? "")
        // A blank cell is NULL, the same reading the grid gives it.
        XCTAssertTrue(insert?.sql.contains("NULL") == true, insert?.sql ?? "")
    }
}
