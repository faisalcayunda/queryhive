import XCTest

@testable import QueryHive

/// The staged cell edits and the `UPDATE`s they become.
///
/// Both halves are pure, which is why they can be checked here at all: the queue is a dictionary and
/// the statements are a string, so what a drag, a bulk edit and a paste *mean* is a question this
/// file can answer without a window or a server. The gestures and the writing are not.
final class CellEditsTests: XCTestCase {
    private let rows: [[String?]] = [
        ["32.01.01.2001", "KPM Sukamaju", "4"],
        ["32.01.01.2002", "KPM Cibadak", "2"],
        ["32.01.01.2003", "KPM Mekarsari", nil],
    ]
    private let columns = [
        Event.Column(name: "kode_wilayah", type: "varchar"),
        Event.Column(name: "nama", type: "varchar"),
        Event.Column(name: "jumlah_jiwa", type: "bigint"),
    ]

    private func key(_ row: Int, _ column: Int) -> CellKey { CellKey(row: row, column: column) }

    // MARK: The queue

    func testEditingACellStagesItsNewText() {
        var edits = CellEdits()

        edits.edit("KPM Suka Maju", at: key(0, 1), original: "KPM Sukamaju")

        XCTAssertEqual(edits.count, 1)
        XCTAssertEqual(edits.value(at: key(0, 1)), "KPM Suka Maju")
    }

    func testStagingTheValueTheCellAlreadyHoldsUnstagesIt() {
        // A user who types a value and then types the original back has changed nothing. Leaving a
        // no-op in the queue would make the commit button claim work that is not there.
        var edits = CellEdits()
        edits.edit("KPM Suka Maju", at: key(0, 1), original: "KPM Sukamaju")

        edits.edit("KPM Sukamaju", at: key(0, 1), original: "KPM Sukamaju")

        XCTAssertTrue(edits.isEmpty)
        XCTAssertNil(edits.value(at: key(0, 1)))
    }

    func testFillingABlockStagesEveryCell() {
        var edits = CellEdits()

        edits.fill("0", over: CellRange(from: (row: 0, column: 2), to: (row: 2, column: 2)), rows: rows)

        XCTAssertEqual(edits.count, 3)
        XCTAssertEqual(edits.value(at: key(1, 2)), "0")
    }

    func testFillingUnstagesTheCellsTheTextAlreadyMatches() {
        // The same rule as a single edit, applied per cell: a fill over a block where one cell
        // already holds the text leaves that cell alone.
        var edits = CellEdits()

        edits.fill("4", over: CellRange(from: (row: 0, column: 2), to: (row: 1, column: 2)), rows: rows)

        XCTAssertEqual(edits.count, 1, "only the 2 changed")
        XCTAssertEqual(edits.value(at: key(1, 2)), "4")
    }

    func testPastingABlockLandsFromTheOrigin() {
        var edits = CellEdits()

        edits.paste("a\tb\nc\td", at: key(0, 0), rows: rows, columnCount: 3)

        XCTAssertEqual(edits.value(at: key(0, 0)), "a")
        XCTAssertEqual(edits.value(at: key(0, 1)), "b")
        XCTAssertEqual(edits.value(at: key(1, 0)), "c")
        XCTAssertEqual(edits.value(at: key(1, 1)), "d")
        XCTAssertEqual(edits.count, 4)
    }

    func testPastingStopsAtTheEdgeRatherThanWrapping() {
        // The block is anchored at the origin, so its first field lands there and the rest run off
        // the edge. A paste that wrapped to the next row would put values in cells the user never
        // pointed at.
        var edits = CellEdits()

        edits.paste("a\tb\tc", at: key(0, 2), rows: rows, columnCount: 3)

        XCTAssertEqual(edits.count, 1, "only the cell that exists")
        XCTAssertEqual(edits.value(at: key(0, 2)), "a")
    }

    func testPastingUnquotesATabInsideAValue() {
        // The inverse of the grid's own copy: a value that holds a tab arrives quoted, and pasting
        // it back has to land it in one cell rather than two.
        var edits = CellEdits()

        edits.paste("\"a\tb\"\tc", at: key(0, 0), rows: rows, columnCount: 3)

        XCTAssertEqual(edits.value(at: key(0, 0)), "a\tb")
        XCTAssertEqual(edits.value(at: key(0, 1)), "c")
        XCTAssertEqual(edits.count, 2)
    }

    func testPastingHandlesWindowsLineEndingsAndADroppedTrailingNewline() {
        var edits = CellEdits()

        edits.paste("a\tb\r\nc\td\r\n", at: key(0, 0), rows: rows, columnCount: 3)

        XCTAssertEqual(edits.count, 4, "two rows, not three")
        XCTAssertEqual(edits.value(at: key(1, 1)), "d")
    }

    func testDiscardEmptiesTheQueue() {
        var edits = CellEdits()
        edits.fill("0", over: CellRange(from: (row: 0, column: 2), to: (row: 2, column: 2)), rows: rows)

        edits.discard()

        XCTAssertTrue(edits.isEmpty)
    }

    // MARK: The statements

    private func statements(_ edits: CellEdits, table: String = "\"hive\".\"analytics\".\"people\"",
                            kind: ConnectionKind = .trino) -> [String] {
        UpdateStatements.generate(edits: edits, rows: rows, columns: columns, table: table, kind: kind)
            .compactMap(\.sql)
    }

    func testOneStatementPerRowWithEveryChangedCellInOneSet() {
        var edits = CellEdits()
        edits.edit("KPM Suka Maju", at: key(0, 1), original: "KPM Sukamaju")
        edits.edit("9", at: key(0, 2), original: "4")

        let statements = statements(edits)

        XCTAssertEqual(statements.count, 1, "one row, one statement")
        XCTAssertTrue(statements[0].hasPrefix("UPDATE \"hive\".\"analytics\".\"people\" SET "),
                      statements[0])
        XCTAssertTrue(statements[0].contains("\"nama\" = 'KPM Suka Maju'"), statements[0])
        XCTAssertTrue(statements[0].contains("\"jumlah_jiwa\" = 9"), statements[0])
    }

    func testTheWhereMatchesTheRowAtTheValuesItWasFetchedWith() {
        // The identity rule, stated as a test: with no primary key, every column of the row at the
        // value it arrived with is what identifies it.
        var edits = CellEdits()
        edits.edit("9", at: key(1, 2), original: "2")

        let statement = statements(edits)[0]

        XCTAssertTrue(statement.contains("WHERE \"kode_wilayah\" = '32.01.01.2002'"), statement)
        XCTAssertTrue(statement.contains("\"nama\" = 'KPM Cibadak'"), statement)
        XCTAssertTrue(statement.contains("\"jumlah_jiwa\" = 2"), statement)
    }

    func testANullInTheRowBecomesIsNull() {
        // `= NULL` is never true in SQL, so a NULL has to be `IS NULL` or the statement matches
        // nothing at all — the failure that looks like a successful no-op.
        var edits = CellEdits()
        edits.edit("7", at: key(2, 2), original: nil)

        let statement = statements(edits)[0]

        XCTAssertTrue(statement.contains("\"jumlah_jiwa\" IS NULL"), statement)
    }

    func testANumericColumnIsUnquotedAndAStringColumnIsNot() {
        // The type comes from the result's own column description, so a numeric-looking string in a
        // varchar column stays quoted.
        var edits = CellEdits()
        edits.edit("32.01.01.9999", at: key(0, 0), original: "32.01.01.2001")
        edits.edit("10", at: key(0, 2), original: "4")

        let statement = statements(edits)[0]

        XCTAssertTrue(statement.contains("\"kode_wilayah\" = '32.01.01.9999'"), statement)
        XCTAssertTrue(statement.contains("\"jumlah_jiwa\" = 10"), statement)
    }

    func testABooleanIsUnquoted() {
        // Trino is strict about comparing a boolean to a varchar, so a boolean literal has to be
        // written as one.
        XCTAssertEqual(UpdateStatements.literal("true", type: "boolean"), "true")
        XCTAssertEqual(UpdateStatements.literal("false", type: "bool"), "false")
    }

    func testAnIntervalIsNotReadAsAnInteger() {
        // `interval` starts with `int`. A prefix match would write an interval literal unquoted,
        // which is the kind of bug that only shows up on the one table that has an interval column.
        XCTAssertFalse(UpdateStatements.isNumeric("interval day to second"))
        XCTAssertEqual(UpdateStatements.literal("1 day", type: "interval day to second"), "'1 day'")
    }

    func testATypeIsReadWithoutItsWidth() {
        XCTAssertTrue(UpdateStatements.isNumeric("decimal(38,10)"))
        XCTAssertTrue(UpdateStatements.isNumeric("numeric(10, 2)"))
        XCTAssertFalse(UpdateStatements.isNumeric("character varying(16)"))
    }

    func testAQuoteInAValueIsDoubled() {
        var edits = CellEdits()
        edits.edit("O'Brien", at: key(0, 1), original: "KPM Sukamaju")

        let statement = statements(edits)[0]

        XCTAssertTrue(statement.contains("'O''Brien'"), statement)
    }

    func testTheIdentifierIsQuotedTheWayTheDriverQuotesIt() {
        var edits = CellEdits()
        edits.edit("x", at: key(0, 1), original: "KPM Sukamaju")

        let mysql = statements(edits, table: "`qh`.`people`", kind: .mysql)[0]

        XCTAssertTrue(mysql.hasPrefix("UPDATE `qh`.`people` SET `nama` = 'x'"), mysql)
    }

    func testNothingStagedIsNoStatements() {
        XCTAssertTrue(statements(CellEdits()).isEmpty)
    }

    func testAnEditOnARowThatIsGoneProducesNothingRatherThanACrash() {
        // Reachable: the queue names positions, and the rows can be replaced under it. A statement
        // built from a row that is no longer there would carry an empty `WHERE` — which updates
        // every row in the table.
        var edits = CellEdits()
        edits.edit("x", at: key(9, 1), original: "KPM Sukamaju")

        XCTAssertTrue(statements(edits).isEmpty)
    }
}
