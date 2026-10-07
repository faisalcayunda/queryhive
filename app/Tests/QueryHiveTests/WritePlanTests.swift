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

    /// W10-T3: one added row, one deleted and one changed make DELETE, UPDATE, INSERT in that order,
    /// each with the count it expects.
    func testOneAddedOneDeletedAndOneChangedRowPlanInOrderWithTheirExpectedCounts() {
        var edits = CellEdits()
        let added = edits.insertRow()
        edits.setInserted("3", row: added, column: 0)
        edits.setInserted("KPM Baru", row: added, column: 1)
        edits.deleteRow(1)
        edits.edit("KPM Berubah", at: CellKey(row: 0, column: 1), original: "KPM Sukamaju")

        let plan = build(edits)

        XCTAssertEqual(plan.statements.map(\.kind), [.delete, .update, .insert])
        XCTAssertEqual(plan.statements.map(\.expectedRows), [1, 1, 1])
        XCTAssertTrue(plan.warnings.isEmpty)
        XCTAssertTrue(plan.statements[2].sql.hasPrefix("INSERT INTO public.penerima"))
    }

    func testARowThatWasAddedAndThenTakenBackPlansNothing() {
        var edits = CellEdits()
        let added = edits.insertRow()
        edits.setInserted("3", row: added, column: 0)
        edits.removeInserted(row: added)
        XCTAssertTrue(build(edits).isEmpty)
    }

    func testARestoredRowPlansNothingAndADeletedRowTakesNoUpdate() {
        var edits = CellEdits()
        edits.deleteRow(1)
        edits.restoreRow(1)
        XCTAssertTrue(build(edits).isEmpty)

        edits.deleteRow(0)
        edits.edit("late", at: CellKey(row: 0, column: 1), original: "KPM Sukamaju")
        XCTAssertEqual(build(edits).statements.map(\.kind), [.delete], "no UPDATE after its own DELETE")
    }

    func testTheReviewedStatementsAreTheStatementsTheEngineIsHanded() throws {
        var edits = CellEdits()
        edits.edit("KPM Baru", at: CellKey(row: 0, column: 1), original: "KPM Sukamaju")
        edits.deleteRow(1)
        let inserted = edits.insertRow()
        edits.setInserted("9", row: inserted, column: 0)

        let plan = build(edits)
        let reviewed = plan.sql
        let bound = plan.statements.map(\.boundSQL)

        // The payload is the only encoder of the plan, and it is what `apply_changes` reads. The
        // review and the run are two renderings of the one build: the payload carries the bound
        // form, and the placeholders are the only difference from the reviewed literals.
        let data = try XCTUnwrap(plan.payload.data(using: .utf8))
        let decoded = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [[String: Any]])
        let ran = decoded.compactMap { $0["sql"] as? String }

        XCTAssertEqual(ran, bound)
        XCTAssertEqual(ran.count, reviewed.count)
        XCTAssertEqual(ran.count, 3)
        XCTAssertTrue(ran.contains { $0.contains("$1") }, "\(ran)")
    }

    func testBoundValuesTravelInThePayloadWhileTheReviewShowsTheLiterals() throws {
        var edits = CellEdits()
        edits.edit("O'Brien", at: CellKey(row: 0, column: 1), original: "KPM Sukamaju")

        let plan = build(edits)
        let statement = try XCTUnwrap(plan.statements.first)

        // The review shows the value, escaped for a human and a copy-paste; the run
        // binds it, so escaping is no longer anyone's job on the write path.
        XCTAssertTrue(statement.sql.contains("'O''Brien'"), statement.sql)
        XCTAssertTrue(statement.boundSQL.contains("$1"), statement.boundSQL)
        XCTAssertEqual(statement.parameters.first, .text("O'Brien"))

        let data = try XCTUnwrap(plan.payload.data(using: .utf8))
        let decoded = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [[String: Any]])
        let params = decoded.first?["params"] as? [[String: String]]
        XCTAssertEqual(params?.first?["type"], "text")
        XCTAssertEqual(params?.first?["value"], "O'Brien")
    }

    func testTrinoGetsTheInlineStatementBecauseItCannotBind() throws {
        var edits = CellEdits()
        let inserted = edits.insertRow()
        edits.setInserted("Nia", row: inserted, column: 1)

        let plan = WritePlan.build(edits: edits, rows: rows, columns: columns,
                                   table: "public.penerima", kind: .trino)
        let statement = try XCTUnwrap(plan.statements.first)

        // No placeholders exist on Trino's wire, so the statement is the inline one it always was
        // and there is nothing to bind.
        XCTAssertTrue(statement.parameters.isEmpty)
        XCTAssertEqual(statement.boundSQL, statement.sql)
        XCTAssertFalse(statement.boundSQL.contains("?"))
        XCTAssertFalse(statement.boundSQL.contains("$1"))
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
        // Only the column the user provided is named: a column left blank takes
        // the server's own default, which writing `NULL` over would override.
        XCTAssertTrue(insert?.sql.contains("\"nama\"") == true, insert?.sql ?? "")
        XCTAssertFalse(insert?.sql.contains("\"id\"") == true, insert?.sql ?? "")
        XCTAssertTrue(insert?.sql.contains("'O''Brien'") == true, insert?.sql ?? "")
        XCTAssertFalse(insert?.sql.contains("NULL") == true, insert?.sql ?? "")
    }
}
