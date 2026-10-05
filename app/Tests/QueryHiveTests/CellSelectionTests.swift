import XCTest

@testable import QueryHive

/// The block of cells a drag selects, and the text it becomes on the clipboard.
///
/// This is the half of drag-to-select that can be checked without a window: which rectangle a pair
/// of corners describes, and what that rectangle turns into when it is pasted into a spreadsheet.
/// The gesture that produces the corners is the view's, and a snapshot is what shows it.
final class CellSelectionTests: XCTestCase {
    override func setUp() {
        super.setUp()
        // Before any QueryTab is built: its constructor reads the stored output directory.
        isolateConnectionStore()
    }

    private let headers = ["kode_wilayah", "nama", "jumlah_jiwa"]
    private let rows: [[String?]] = [
        ["32.01.01.2001", "KPM Sukamaju", "4"],
        ["32.01.01.2002", "KPM Cibadak", "2"],
        ["32.01.01.2003", "KPM Mekarsari", "7"],
    ]

    // MARK: The rectangle

    func testTheRangeIsTheSameWhicheverCornerTheDragStartedFrom() {
        // A drag up and to the left is the same block as the same drag made down and to the right.
        // The pointer is not asked to respect an order, and nothing downstream should have to ask
        // which corner came first.
        let downRight = CellRange(from: (row: 0, column: 0), to: (row: 2, column: 1))
        let upLeft = CellRange(from: (row: 2, column: 1), to: (row: 0, column: 0))

        XCTAssertEqual(downRight, upLeft)
        XCTAssertEqual(downRight.top, 0)
        XCTAssertEqual(downRight.left, 0)
        XCTAssertEqual(downRight.bottom, 2)
        XCTAssertEqual(downRight.right, 1)
    }

    func testTheRangeCountsItsOwnRowsAndColumns() {
        let range = CellRange(from: (row: 1, column: 0), to: (row: 2, column: 2))

        XCTAssertEqual(range.rowCount, 2)
        XCTAssertEqual(range.columnCount, 3)
        XCTAssertEqual(range.cellCount, 6)
    }

    func testContainsIsTheBlockAndNothingOutsideIt() {
        let range = CellRange(from: (row: 1, column: 1), to: (row: 2, column: 2))

        XCTAssertTrue(range.contains(row: 1, column: 1))
        XCTAssertTrue(range.contains(row: 2, column: 2))
        XCTAssertTrue(range.contains(row: 2, column: 1))
        XCTAssertFalse(range.contains(row: 0, column: 1), "the row above")
        XCTAssertFalse(range.contains(row: 1, column: 0), "the column to the left")
        XCTAssertFalse(range.contains(row: 3, column: 2), "the row below")
    }

    // MARK: The text

    func testTheTextIsTabSeparatedWithOneLinePerRow() {
        let range = CellRange(from: (row: 0, column: 0), to: (row: 1, column: 1))

        XCTAssertEqual(GridClipboard.text(rows: rows, headers: headers, selection: range,
                                          withHeaders: false),
                       "32.01.01.2001\tKPM Sukamaju\n32.01.01.2002\tKPM Cibadak")
    }

    func testHeadersComeFromTheSelectedColumnsOnly() {
        // The block starts at column 1, so the pasted table must carry `nama` and `jumlah_jiwa` and
        // not the name of a column the user did not select — a header that names a column the row
        // does not contain is worse than no header.
        let range = CellRange(from: (row: 0, column: 1), to: (row: 1, column: 2))

        XCTAssertEqual(GridClipboard.text(rows: rows, headers: headers, selection: range,
                                          withHeaders: true),
                       "nama\tjumlah_jiwa\nKPM Sukamaju\t4\nKPM Cibadak\t2")
    }

    func testHeadersAreLeftOutWhenTheyWereNotAskedFor() {
        let range = CellRange(from: (row: 0, column: 0), to: (row: 0, column: 0))

        XCTAssertEqual(GridClipboard.text(rows: rows, headers: headers, selection: range,
                                          withHeaders: false),
                       "32.01.01.2001")
    }

    func testANullIsAnEmptyCellRatherThanTheWordNull() {
        // The grid draws NULL as an italic "null" because an empty cell there would be
        // indistinguishable from an empty string. On the clipboard that is the wrong trade: the
        // paste target has no NULL to receive one, and the four letters would be a claim that the
        // database stored them.
        let rows: [[String?]] = [["a", nil], ["b", ""]]
        let range = CellRange(from: (row: 0, column: 0), to: (row: 1, column: 1))

        XCTAssertEqual(GridClipboard.text(rows: rows, headers: [], selection: range, withHeaders: false),
                       "a\t\nb\t")
    }

    func testAShortRowKeepsItsColumnsRatherThanShiftingThem() {
        // A row with fewer values than the grid has columns is a blank cell, not a shortened row:
        // the grid draws by position, and a copy that skipped the missing cell would move every
        // value after it under the wrong heading.
        let rows: [[String?]] = [["a"]]
        let range = CellRange(from: (row: 0, column: 0), to: (row: 0, column: 2))

        XCTAssertEqual(GridClipboard.text(rows: rows, headers: [], selection: range, withHeaders: false),
                       "a\t\t")
    }

    func testASelectionPastTheLastRowIsEmptyCellsRatherThanACrash() {
        // Reachable: the selection is an index, and rows can be replaced under it. The copy has to
        // answer with blanks rather than reach out of the array.
        let range = CellRange(from: (row: 0, column: 0), to: (row: 2, column: 1))

        XCTAssertEqual(GridClipboard.text(rows: [], headers: [], selection: range, withHeaders: false),
                       "\t\n\t\n\t")
    }

    func testAValueHoldingATabOrNewlineIsQuoted() {
        // Without the quoting one such value silently becomes two columns and shifts the rest of the
        // row, which is worse than the quotes it costs.
        let rows: [[String?]] = [["a\tb", "c\nd"]]
        let range = CellRange(from: (row: 0, column: 0), to: (row: 0, column: 1))

        XCTAssertEqual(GridClipboard.text(rows: rows, headers: [], selection: range, withHeaders: false),
                       "\"a\tb\"\t\"c\nd\"")
    }

    func testAQuoteInsideAValueIsDoubled() {
        let rows: [[String?]] = [["he said \"no\""]]
        let range = CellRange(from: (row: 0, column: 0), to: (row: 0, column: 0))

        XCTAssertEqual(GridClipboard.text(rows: rows, headers: [], selection: range, withHeaders: false),
                       "\"he said \"\"no\"\"\"")
    }

    func testAPlainValueIsLeftAlone() {
        // The quoting is only for values that would break the table. Everything else is written
        // exactly as the server returned it.
        let rows: [[String?]] = [["KPM Sukamaju", "32.01.01.2001"]]
        let range = CellRange(from: (row: 0, column: 0), to: (row: 0, column: 1))

        XCTAssertEqual(GridClipboard.text(rows: rows, headers: [], selection: range, withHeaders: false),
                       "KPM Sukamaju\t32.01.01.2001")
    }

    // MARK: When the selection stops describing what is on screen

    func testChangingAFilterDropsTheSelection() {
        // The selection and the filters are both indices into the rows on screen. Hiding a row moves
        // every index below it, so a block left over from before the filter would highlight — and
        // copy — rows the user never pointed at.
        let tab = QueryTab(title: "Query 1")
        tab.cellSelection = CellRange(from: (row: 0, column: 0), to: (row: 2, column: 1))

        tab.columnFilters[0] = .values(["32.01.01.2001"])

        XCTAssertNil(tab.cellSelection)
    }

    // MARK: Where the pointer lands

    /// The three numbers the grid draws with, in the values it actually uses.
    private let geometry = GridColumnGeometry(gutter: 60, widths: [116, 216, 66])

    func testTheGutterIsTheFirstColumnRatherThanNothing() {
        // A drag that reaches left into the row-number gutter is asking to extend the selection to
        // the first column, not to drop the selection.
        XCTAssertEqual(geometry.clampedColumn(atX: 0, last: 2), 0)
        XCTAssertEqual(geometry.clampedColumn(atX: 59, last: 2), 0)
        XCTAssertEqual(geometry.clampedColumn(atX: 60, last: 2), 0)
    }

    func testEachColumnStartsWhereTheLastOneEnds() {
        // The cell's own padding is part of the column's drawn width, so the boundary is the
        // measured width plus both paddings — the off-by-a-padding this test exists to catch.
        XCTAssertEqual(geometry.clampedColumn(atX: 175, last: 2), 0)
        XCTAssertEqual(geometry.clampedColumn(atX: 176, last: 2), 1)
        XCTAssertEqual(geometry.clampedColumn(atX: 391, last: 2), 1)
        XCTAssertEqual(geometry.clampedColumn(atX: 392, last: 2), 2)
    }

    func testPastTheLastColumnIsTheLastColumn() {
        // The space to the right of a short result is still the rightmost column: the pointer has
        // nowhere else to be, and a drag out there means "to the edge".
        XCTAssertEqual(geometry.clampedColumn(atX: 10_000, last: 2), 2)
    }

    func testTheRowIsComputedFromYCoordinate() {
        // The row is floor(y / rowHeight), clamped to valid range.
        XCTAssertEqual(geometry.row(atY: 0, rowHeight: 25, count: 12), 0)
        XCTAssertEqual(geometry.row(atY: 24.9, rowHeight: 25, count: 12), 0)
        XCTAssertEqual(geometry.row(atY: 25, rowHeight: 25, count: 12), 1)
        XCTAssertEqual(geometry.row(atY: 74, rowHeight: 25, count: 12), 2)
    }

    func testADragThatLeavesTheGridKeepsSelectingItsEdge() {
        // Up past the first row and down past the last both clamp. Without this a drag off the top
        // would build a range with a negative index, and the highlight and the copy would disagree
        // about which rows they meant.
        XCTAssertEqual(geometry.row(atY: -1, rowHeight: 25, count: 12), 0)
        XCTAssertEqual(geometry.row(atY: -100, rowHeight: 25, count: 12), 0)
        XCTAssertEqual(geometry.row(atY: 1_000, rowHeight: 25, count: 12), 11)
    }

    func testTheCellCarriesBothAxesAtOnce() {
        let col = geometry.clampedColumn(atX: 400, last: 2)
        let row = geometry.row(atY: 50, rowHeight: 25, count: 12)

        XCTAssertEqual(row, 2, "two row-heights down")
        XCTAssertEqual(col, 2, "past the second column's edge at 392")
    }
}
