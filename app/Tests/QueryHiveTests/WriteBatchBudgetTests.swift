import XCTest

@testable import QueryHive

/// The write batch budget: rows, bytes and parameters at once, with a batch
/// closed **before** the row that would cross a bound.
final class WriteBatchBudgetTests: XCTestCase {
    private let one = [Event.Column(name: "a", type: "bigint")]
    private let two = [Event.Column(name: "a", type: "bigint"),
                       Event.Column(name: "b", type: "bigint")]

    /// Three added rows, each providing the given columns with a distinct value.
    private func rows(ofColumnCount count: Int, count rows: Int = 3) -> CellEdits {
        var edits = CellEdits()
        for index in 0..<rows {
            let row = edits.insertRow()
            for column in 0..<count {
                edits.setInserted("\(index)", row: row, column: column)
            }
        }
        return edits
    }

    private func build(_ edits: CellEdits, columns: [Event.Column],
                       budget: WriteBatchBudget) -> WritePlan {
        WritePlan.build(edits: edits, rows: [], columns: columns, table: "t",
                        kind: .postgres, budget: budget)
    }

    func testTheDefaultBudgetStatesItsNumbers() {
        let postgres = WriteBatchBudget.forKind(.postgres)
        XCTAssertEqual(postgres.maxRows, 1_000)
        XCTAssertEqual(postgres.maxBytes, 1 << 20)
        XCTAssertEqual(postgres.maxParameters, 65_535, "PostgreSQL's own statement limit")
        XCTAssertEqual(WriteBatchBudget.forKind(.mysql).maxParameters, 65_535)
        XCTAssertEqual(WriteBatchBudget.forKind(.trino).maxParameters, .max,
                       "Trino's HTTP protocol has no bind parameters")
    }

    func testTheRowBoundClosesTheBatchBeforeTheRowThatWouldCrossIt() {
        let budget = WriteBatchBudget(maxRows: 2, maxBytes: .max, maxParameters: .max)

        let plan = build(rows(ofColumnCount: 1), columns: one, budget: budget)

        XCTAssertEqual(plan.statements.count, 2, "2 + 1 rows, not one batch of 3")
        XCTAssertEqual(plan.statements.map(\.expectedRows), [2, 1])
    }

    func testTheByteBoundClosesTheBatchBeforeTheRowThatWouldCrossIt() {
        // A byte bound smaller than a single statement: every row must still be
        // planned, so the batch is one row at a time rather than a batch that
        // quietly drops rows.
        let budget = WriteBatchBudget(maxRows: .max, maxBytes: 1, maxParameters: .max)

        let plan = build(rows(ofColumnCount: 1), columns: one, budget: budget)

        XCTAssertEqual(plan.statements.count, 3)
        XCTAssertEqual(plan.statements.map(\.expectedRows), [1, 1, 1])
    }

    func testTheParameterBoundClosesTheBatchBeforeTheRowThatWouldCrossIt() {
        // Two values per row, four parameters fit two rows, the third row starts
        // the next batch.
        let budget = WriteBatchBudget(maxRows: .max, maxBytes: .max, maxParameters: 4)

        let plan = build(rows(ofColumnCount: 2), columns: two, budget: budget)

        XCTAssertEqual(plan.statements.count, 2)
        XCTAssertEqual(plan.statements.map(\.expectedRows), [2, 1])
    }

    func testRowsThatWriteDifferentColumnsAreNotMerged() {
        // A multi-row VALUES has one column list, so rows only share a batch when
        // they write the same columns (the study's grouping by column set).
        var edits = CellEdits()
        let first = edits.insertRow()
        edits.setInserted("1", row: first, column: 0)
        let second = edits.insertRow()
        edits.setInserted("2", row: second, column: 1)

        let plan = build(edits, columns: two,
                         budget: WriteBatchBudget(maxRows: 100, maxBytes: .max,
                                                  maxParameters: .max))

        XCTAssertEqual(plan.statements.count, 2)
        XCTAssertTrue(plan.statements[0].sql.contains("(\"a\")"), plan.statements[0].sql)
        XCTAssertTrue(plan.statements[1].sql.contains("(\"b\")"), plan.statements[1].sql)
    }
}
