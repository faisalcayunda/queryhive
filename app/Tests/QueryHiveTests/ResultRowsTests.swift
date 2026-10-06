import XCTest

@testable import QueryHive

/// The rows the grid draws, as the seam `ResultRows` is (blueprint D-3, D-4).
///
/// Everything here is what a pixel or a statement ends up depending on: what a cell says, which
/// columns a copy takes, how wide a column is measured, and what a write plan matches against.
final class ResultRowsTests: XCTestCase {

    private let columns = [Event.Column(name: "kode", type: "text"),
                           Event.Column(name: "jumlah", type: "bigint"),
                           Event.Column(name: "catatan", type: "text")]

    private func rows(_ values: [[String?]]) -> ArrayRows {
        ArrayRows(rows: values, sizing: values, columns: columns)
    }

    // MARK: One cell

    /// A cell carries its first line only — a row is one line tall — and at most `prefixLimit`
    /// UTF-16 units of it, flagged when the value was longer than that.
    func testACellCutsAtTheFirstLineAndAtThePrefixLimit() {
        let multiline = rows([["one\ntwo", "3", nil]])
        XCTAssertEqual(multiline.cell(row: 0, column: 0, format: .raw).text, "one")
        XCTAssertFalse(multiline.cell(row: 0, column: 0, format: .raw).flags.contains(.truncated))

        let long = String(repeating: "x", count: CellText.prefixLimit + 50)
        let cut = rows([[long, "3", nil]]).cell(row: 0, column: 0, format: .raw)
        XCTAssertEqual(cut.text.count, CellText.prefixLimit)
        XCTAssertTrue(cut.flags.contains(.truncated))
    }

    /// Three answers that must not look alike: a NULL is the word the style gives it, an empty
    /// string is `∅`, and a cell a short row does not carry draws nothing at all.
    ///
    /// This is parity item 15 ("NULL miring, `∅`") and the one a single `nil` collapses: `valueAt`
    /// returns `nil` for both a NULL and a row that ends before the column, so the flag has to be
    /// read off the row's shape rather than off the value.
    func testANullAnEmptyStringAndAMissingCellAreThreeDifferentAnswers() {
        let one = rows([["", nil]])
        XCTAssertEqual(one.cell(row: 0, column: 0, format: .raw).flags, [.empty])
        XCTAssertEqual(one.cell(row: 0, column: 0, format: .raw).text, "")

        XCTAssertEqual(one.cell(row: 0, column: 1, format: .raw).flags, [.null])
        XCTAssertEqual(one.cell(row: 0, column: 1, format: .raw).text, "")

        // Past the end of a short row: no flag at all, so the painter draws neither glyph.
        let short = ArrayRows(rows: [["only"]], sizing: [["only"]], columns: columns)
        XCTAssertEqual(short.cell(row: 0, column: 2, format: .raw).flags, CellFlags())
        XCTAssertEqual(short.cell(row: 9, column: 0, format: .raw).flags, CellFlags())
    }

    /// A display format changes the characters and never the value, and it is applied to the cell
    /// and to the whole value alike.
    func testTheDisplayFormatIsAppliedToTheCellAndToTheFullValue() {
        let uuid = ArrayRows(rows: [["550e8400e29b41d4a716446655440000"]],
                             sizing: [["550e8400e29b41d4a716446655440000"]],
                             columns: [Event.Column(name: "id", type: "uuid")])
        XCTAssertEqual(uuid.cell(row: 0, column: 0, format: .uuid).text,
                       "550e8400-e29b-41d4-a716-446655440000")
        XCTAssertEqual(uuid.fullValue(row: 0, column: 0, format: .uuid),
                       "550e8400-e29b-41d4-a716-446655440000")

        let binary = ArrayRows(rows: [["6869"]], sizing: [["6869"]],
                               columns: [Event.Column(name: "payload", type: "bytea")])
        XCTAssertEqual(binary.cell(row: 0, column: 0, format: .text).text, "hi")

        let stamp = ArrayRows(rows: [["1700000000"]], sizing: [["1700000000"]],
                              columns: [Event.Column(name: "at", type: "bigint")])
        XCTAssertEqual(stamp.cell(row: 0, column: 0, format: .unixTimestamp).text,
                       "2023-11-14 22:13:20")

        let json = ArrayRows(rows: [[#"{"b":2,"a":1}"#]], sizing: [[#"{"b":2,"a":1}"#]],
                             columns: [Event.Column(name: "doc", type: "jsonb")])
        let drawn = json.cell(row: 0, column: 0, format: .json).text
        XCTAssertFalse(drawn.contains("\n"), "a grid row must not be broken by a pretty object")
        XCTAssertTrue(drawn.contains("\"a\""))
    }

    /// `.raw` is what an edit, a copy and an export see: the value as the engine sent it.
    func testFullValueRawReturnsWhatTheEngineSent() {
        let value = "550e8400e29b41d4a716446655440000"
        let one = ArrayRows(rows: [[value]], sizing: [[value]],
                            columns: [Event.Column(name: "id", type: "uuid")])
        XCTAssertEqual(one.fullValue(row: 0, column: 0, format: .raw), value)
        XCTAssertEqual(one.cell(row: 0, column: 0, format: .raw).text, value)

        // A NULL has no raw value to read, which is what keeps it out of a tooltip.
        let withNull = rows([["a", "1", nil]])
        XCTAssertNil(withNull.fullValue(row: 0, column: 2, format: .raw))
        XCTAssertNil(withNull.fullValue(row: 9, column: 0, format: .raw))
    }

    /// Flagged cells: numeric by the column's type (the `contains "int"` quirk included), and
    /// openable when the value starts with `{` or `[` — the cheap scan D-4 moved into the rows.
    func testTheFlagsDescribeWhatTheCellHolds() {
        let values = [["1", "{ \"a\": 1 }", nil]]
        let one = ArrayRows(rows: values, sizing: values,
                            columns: [Event.Column(name: "n", type: "bigint"),
                                      Event.Column(name: "doc", type: "jsonb"),
                                      Event.Column(name: "note", type: "text")])
        // Numeric is the *column's* type, not a parse of the value: the grid tints the column.
        XCTAssertTrue(one.cell(row: 0, column: 0, format: .raw).flags.contains(.numeric))
        XCTAssertTrue(one.cell(row: 0, column: 1, format: .raw).flags.contains(.openable))
        XCTAssertFalse(one.cell(row: 0, column: 2, format: .raw).flags.contains(.openable),
                       "a NULL is never openable")
        XCTAssertFalse(one.cell(row: 0, column: 2, format: .raw).flags.contains(.numeric))

        // The `contains "int"` quirk: `interval` reads as a number and has always been drawn
        // flush right, so the flag follows the same rule `GridMetrics.isNumeric` lays down.
        let quirky = ArrayRows(rows: [["0 seconds"]], sizing: [["0 seconds"]],
                               columns: [Event.Column(name: "d", type: "interval")])
        XCTAssertTrue(quirky.cell(row: 0, column: 0, format: .raw).flags.contains(.numeric))
    }

    // MARK: Columns out

    /// Copy and export take the columns a test asks for, in the order it asks for them — which is
    /// what keeps a hidden or moved column from taking its neighbour's values.
    func testRowsInColumnsReturnsTheRequestedSourceColumns() {
        let one = rows([["a", "7", "note"], ["b", "8", nil]])
        XCTAssertEqual(one.rows(in: 0..<1, columns: [2, 0]), [["note", "a"]])
        XCTAssertEqual(one.rows(in: 0..<2, columns: [1]),
                       [["7"], ["8"]])
        // A row that ends early yields nils rather than shifting the rest of the row left.
        XCTAssertEqual(one.rows(in: 1..<2, columns: [0, 1, 2]), [["b", "8", nil]])
        XCTAssertEqual(one.rows(in: 0..<0, columns: [0]), [])
    }

    func testRowsInColumnsKeepsServerOrderWhenDisplayIsFiltered() {
        let fetched = [["a", "1"], ["b", "2"], ["c", "3"]]
        let shown = ArrayRows(rows: [fetched[2]], sizing: fetched, columns: columns)
        XCTAssertEqual(shown.count, 1)
        XCTAssertEqual(shown.fetched, 3)
        XCTAssertEqual(shown.rows(in: 0..<shown.count, columns: [0, 1]), [["c", "3"]])
    }

    // MARK: Width

    /// The old formula, kept: the widest of the first 200 **fetched** rows, NULL counting 4,
    /// capped at 64 — and read from the server's order, so a filter that hid the longest row
    /// cannot make a column narrower than the data it was measured against.
    func testNaturalCharCountsMatchTheOldFormula() {
        let long = String(repeating: "w", count: 100)
        let fetched: [[String?]] = [[nil, "9", long],
                                    ["ab", "1234567890", nil]]
        // The display shows only the second row, as a filter over the first would.
        let rows = ArrayRows(rows: [fetched[1]], sizing: fetched, columns: columns)
        XCTAssertEqual(rows.naturalCharCounts(), [4, 10, 64])
        // Cached: a second ask must give the same answer without a second pass.
        XCTAssertEqual(rows.naturalCharCounts(), [4, 10, 64])

        // With no NULL and nothing over the cap, the counts are the plain widths.
        let plain = ArrayRows(rows: [["abc", "12", "xy"]],
                              sizing: [["abc", "12", "xy"]], columns: columns)
        XCTAssertEqual(plain.naturalCharCounts(), [3, 2, 2])
    }

    // MARK: Both shapes

    /// `RowReading` is what `WritePlan` matches against, and it has to answer the same way for a
    /// plain array of rows and for the seam that wraps one.
    func testRowReadingIsAvailableOnBothShapes() {
        let array: [[String?]] = [["a", "1"], ["b", nil]]
        XCTAssertEqual(array.row(at: 1), ["b", nil])
        XCTAssertNil(array.row(at: 5))
        XCTAssertNil(array.row(at: -1))

        let rows = self.rows(array)
        XCTAssertEqual(rows.row(at: 0), ["a", "1"])
        XCTAssertEqual(rows.row(at: 1), ["b", nil])
        XCTAssertNil(rows.row(at: 5))
    }

    /// The write plan reads the rows through `RowReading`, so both shapes have to produce the
    /// same statements — the WHERE clause is matched against `row(at:)`, and a wrapper that
    /// answered differently would change which row the UPDATE lands on.
    func testWritePlanBuildsTheSameStatementsFromEitherShape() {
        let columns = [Event.Column(name: "kode", type: "text"),
                       Event.Column(name: "jumlah", type: "bigint")]
        let array: [[String?]] = [["A-1", "7"], ["A-2", "8"]]
        var edits = CellEdits()
        edits.edit("42", at: CellKey(row: 0, column: 1), original: "7")

        let fromArray = WritePlan.build(edits: edits, rows: array, columns: columns,
                                        table: "public.penerima", kind: .postgres)
        let fromSeam = WritePlan.build(edits: edits,
                                       rows: ArrayRows(rows: array, sizing: array, columns: columns),
                                       columns: columns, table: "public.penerima", kind: .postgres)
        XCTAssertFalse(fromArray.statements.isEmpty, "the fixture has to stage a real edit")
        XCTAssertEqual(fromArray.statements, fromSeam.statements)
        XCTAssertEqual(fromArray.warnings, fromSeam.warnings)
    }

    // MARK: Distinct values (TM-3)

    private func distinctRows(_ values: [String?]) -> ArrayRows {
        let rows = values.map { [$0] }
        return ArrayRows(rows: rows, sizing: rows, columns: [Event.Column(name: "k", type: "text")])
    }

    /// NULL first, the rest sorted, each once, as the whole set rather than the first ten seen.
    func testDistinctValuesAreNullFirstThenSortedAndComplete() async {
        let sample = await distinctRows(["b", nil, "a", "b", "c", nil]).distinctValues(column: 0)
        XCTAssertEqual(sample, DistinctSample(values: [nil, "a", "b", "c"], more: false))
    }

    /// Eleven distinct values are a full answer that the picker still refuses (more than ten); a
    /// twelfth is `more`, with no values, which is what `distinct_values(limit: 11)` answers.
    func testDistinctValuesStopAtTheLimitWithMoreAndNoValues() async {
        let eleven = (0..<11).map { Optional("v\($0)") }
        let full = await distinctRows(eleven).distinctValues(column: 0)
        XCTAssertFalse(full.more)
        XCTAssertEqual(full.values.count, 11)
        XCTAssertGreaterThan(full.values.count, ColumnFilter.valuePickerLimit)

        let twelve = await distinctRows(eleven + ["v11"]).distinctValues(column: 0)
        XCTAssertEqual(twelve, DistinctSample(values: [], more: true))

        let nullCounts = await distinctRows((0..<11).map { Optional("v\($0)") } + [nil]).distinctValues(column: 0)
        XCTAssertTrue(nullCounts.more, "a NULL is one of the entries the limit counts")
    }

    /// A key typed two ways (precomposed and decomposed) is one entry, in the composed form.
    func testDistinctValuesNormaliseToNFC() async {
        let sample = await distinctRows(["e\u{301}", "\u{e9}"]).distinctValues(column: 0)
        XCTAssertEqual(sample.values.count, 1)
        XCTAssertEqual(sample.values.first??.unicodeScalars.count, 1)
    }

    /// Rows that never reach the column count as NULL, as `ColumnFilter.distinctValues` has it.
    func testDistinctValuesTreatAShortRowAsNull() async {
        let rows: [[String?]] = [["x"], []]
        let sample = await ArrayRows(rows: rows, sizing: rows,
                                     columns: [Event.Column(name: "k", type: "text")])
            .distinctValues(column: 0)
        XCTAssertEqual(sample.values, [nil, "x"])
    }

    /// The rows' defaults for the parts only a live store has a use for.
    func testAnArrayHasNothingToPollAndNoFormatToDrop() {
        let rows = distinctRows(["a"])
        XCTAssertEqual(rows.poll(), PollResult(grewFrom: nil, finished: true))
        XCTAssertFalse(rows.isLive)
        XCTAssertTrue(rows.prepare(formats: [.raw]).isEmpty)
        rows.release()
    }
}
