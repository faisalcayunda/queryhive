import XCTest

@testable import QueryHive

/// Added rows: what an untouched column means, what `DEFAULT` means, and the row
/// the server fills in entirely.
final class InsertStatementsTests: XCTestCase {
    private let columns = [
        Event.Column(name: "id", type: "bigint"),
        Event.Column(name: "nama", type: "varchar"),
    ]

    private func build(_ edits: CellEdits, table: String = "public.penerima",
                       kind: ConnectionKind = .postgres) -> WritePlan {
        WritePlan.build(edits: edits, rows: [], columns: columns, table: table, kind: kind)
    }

    func testAnUnsetColumnIsLeftOutSoTheServerCanDefaultIt() {
        var edits = CellEdits()
        let row = edits.insertRow()
        edits.setInserted("Nia", row: row, column: 1)

        let plan = build(edits)

        // `id` is not named at all: writing `NULL` over a generated or defaulted
        // column is what the study's bug did.
        XCTAssertEqual(plan.statements.map(\.sql),
                       ["INSERT INTO public.penerima (\"nama\") VALUES ('Nia')"])
    }

    func testTheDefaultSentinelIsTheKeywordNotAString() {
        var edits = CellEdits()
        let row = edits.insertRow()
        edits.setInserted("DEFAULT", row: row, column: 0)
        edits.setInserted("Nia", row: row, column: 1)

        let plan = build(edits)

        XCTAssertEqual(plan.statements.first?.sql,
                       "INSERT INTO public.penerima (\"id\", \"nama\") VALUES (DEFAULT, 'Nia')")
    }

    func testAnAllDefaultRowGetsThePostgresSpell() {
        var edits = CellEdits()
        _ = edits.insertRow()

        let plan = build(edits)

        XCTAssertEqual(plan.statements.map(\.sql),
                       ["INSERT INTO public.penerima DEFAULT VALUES"])
        XCTAssertEqual(plan.statements.first?.expectedRows, 1)
        XCTAssertTrue(plan.warnings.isEmpty)
    }

    func testAnAllDefaultRowGetsTheMySQLSpell() {
        var edits = CellEdits()
        _ = edits.insertRow()

        let plan = build(edits, kind: .mysql)

        XCTAssertEqual(plan.statements.map(\.sql),
                       ["INSERT INTO public.penerima () VALUES ()"])
    }

    func testARowOfOnlyDefaultCellsIsSpelledAllDefault() {
        var edits = CellEdits()
        let row = edits.insertRow()
        edits.setInserted("DEFAULT", row: row, column: 0)
        edits.setInserted("default", row: row, column: 1)

        let plan = build(edits)

        XCTAssertEqual(plan.statements.map(\.sql),
                       ["INSERT INTO public.penerima DEFAULT VALUES"])
    }

    func testTrinoHasNoAllDefaultFormSoThePlanSaysTheRowWasNotPlanned() {
        var edits = CellEdits()
        _ = edits.insertRow()

        let plan = build(edits, kind: .trino)

        XCTAssertTrue(plan.isEmpty, "no honest SQL to emit, so nothing is emitted")
        XCTAssertEqual(plan.warnings.count, 1)
        XCTAssertTrue(plan.warnings[0].contains("added row 1"), plan.warnings[0])
        XCTAssertTrue(plan.warnings[0].contains("Trino"), plan.warnings[0])
    }

    func testAnAllDefaultRowDoesNotVanishFromTheBatch() {
        // The study's bug: a generator returning nothing for the default row lets
        // the rest commit and report success. The row keeps its own statement.
        var edits = CellEdits()
        _ = edits.insertRow()
        let normal = edits.insertRow()
        edits.setInserted("Nia", row: normal, column: 1)

        let plan = build(edits)

        XCTAssertEqual(plan.statements.count, 2)
        XCTAssertTrue(plan.statements.contains {
            $0.sql == "INSERT INTO public.penerima DEFAULT VALUES"
        })
        XCTAssertTrue(plan.warnings.isEmpty)
    }

    func testAnUpdateToTheDefaultSentinelIsTheKeyword() throws {
        var edits = CellEdits()
        edits.edit("DEFAULT", at: CellKey(row: 0, column: 1), original: "old")

        let update = try XCTUnwrap(
            UpdateStatements.generate(edits: edits, rows: [["1", "old"]], columns: columns,
                                      table: "t", kind: .postgres).first)
        let sql = try XCTUnwrap(update.sql)
        XCTAssertTrue(sql.contains("\"nama\" = DEFAULT"), sql)
    }

    func testADeleteOfARowWithNoComparableColumnIsRefusedAndSaidSo() {
        let geom = [Event.Column(name: "geom", type: "geometry")]
        var edits = CellEdits()
        edits.deleteRow(0)

        let plan = WritePlan.build(edits: edits, rows: [["POINT(0 0)"]], columns: geom,
                                   table: "t", kind: .postgres)

        XCTAssertTrue(plan.isEmpty, "a bare DELETE FROM t would match every row")
        XCTAssertEqual(plan.warnings.count, 1)
        XCTAssertTrue(plan.warnings[0].contains("no column can be matched safely"),
                      plan.warnings[0])
    }
}
