import XCTest

@testable import QueryHive

/// The rows the table draws: the result's, then the added ones (blueprint w10 D-2, §5.1).
///
/// The property they hold: a negative id appears in a `CellKey` for a table row past the fetched ones
/// and nowhere else, and the selection (which is a rectangle of table rows) never carries one.
final class GridRowSpaceTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    private func space(fetched: Int, added: Int) -> (GridRowSpace, [Int]) {
        var edits = CellEdits()
        let ids = (0..<added).map { _ in edits.insertRow() }
        return (GridRowSpace(fetched: fetched, inserted: edits.inserted), ids)
    }

    func testTheCountIsTheFetchedRowsPlusTheAddedOnes() {
        XCTAssertEqual(space(fetched: 5, added: 0).0.count, 5)
        XCTAssertEqual(space(fetched: 5, added: 2).0.count, 7)
        XCTAssertEqual(space(fetched: 0, added: 1).0.count, 1, "a table with no rows can still take one")
    }

    func testAKindIsFetchedOrInsertedAndNothingOutside() {
        let (space, ids) = space(fetched: 3, added: 2)
        XCTAssertEqual(space.kind(ofTableRow: 0), .fetched(0))
        XCTAssertEqual(space.kind(ofTableRow: 2), .fetched(2))
        XCTAssertEqual(space.kind(ofTableRow: 3), .inserted(id: ids[0], index: 0))
        XCTAssertEqual(space.kind(ofTableRow: 4), .inserted(id: ids[1], index: 1))
        XCTAssertNil(space.kind(ofTableRow: 5))
        XCTAssertNil(space.kind(ofTableRow: -1))
    }

    func testANegativeIdIsInACellKeyIfAndOnlyIfTheRowIsPastTheFetchedOnes() {
        let (space, ids) = space(fetched: 5, added: 2)
        for row in 0..<space.count {
            let key = space.cellKey(forTableRow: row, source: 3)
            XCTAssertEqual(key?.column, 3)
            XCTAssertEqual(key.map { $0.row < 0 }, row >= 5, "row \(row)")
        }
        XCTAssertEqual(space.cellKey(forTableRow: 5, source: 0)?.row, ids[0])
        XCTAssertNil(space.cellKey(forTableRow: 7, source: 0))
    }

    func testTableRowForKeyInvertsCellKey() {
        let (space, _) = space(fetched: 5, added: 2)
        for row in 0..<space.count {
            let key = try! XCTUnwrap(space.cellKey(forTableRow: row, source: 1))
            XCTAssertEqual(space.tableRow(for: key), row)
        }
        XCTAssertNil(space.tableRow(for: CellKey(row: 5, column: 0)), "row 5 is not a fetched row")
        XCTAssertNil(space.tableRow(for: CellKey(row: -99, column: 0)), "an id that is not in the queue")
    }

    func testABlockThatCrossesTheBorderIsAFetchedPrefixAndAnAddedSuffix() {
        let (space, ids) = space(fetched: 5, added: 2)
        let split = space.split(3...6)
        XCTAssertEqual(split.fetched, 3...4)
        XCTAssertEqual(split.inserted.map(\.tableRow), [5, 6])
        XCTAssertEqual(split.inserted.map(\.id), ids)
    }

    func testABlockOnOneSideIsOnlyThatSide() {
        let (space, ids) = space(fetched: 5, added: 2)
        let fetched = space.split(1...2)
        XCTAssertEqual(fetched.fetched, 1...2)
        XCTAssertTrue(fetched.inserted.isEmpty)

        let added = space.split(6...6)
        XCTAssertNil(added.fetched)
        XCTAssertEqual(added.inserted.map(\.id), [ids[1]])
    }

    func testRowsOutsideTheSpaceAreDropped() {
        let (space, _) = space(fetched: 5, added: 1)
        let split = space.split(4...9)
        XCTAssertEqual(split.fetched, 4...4)
        XCTAssertEqual(split.inserted.count, 1)
        XCTAssertNil(space.split(8...9).fetched)
        XCTAssertTrue(space.split(8...9).inserted.isEmpty)
    }

    func testNoCellRangeBuiltFromTableRowsHoldsANegativeValue() {
        let (space, _) = space(fetched: 5, added: 2)
        let range = CellRange(from: (row: 0, column: 0), to: (row: space.count - 1, column: 2))
        XCTAssertGreaterThanOrEqual(range.top, 0)
        XCTAssertEqual(range.rowCount, 7)
        XCTAssertTrue(range.allPositions.allSatisfy { $0.row >= 0 })
    }

    func testClampedKeepsASelectionInsideAndAnEmptySpaceHasNone() {
        let (space, _) = space(fetched: 3, added: 1)
        let clamped = space.clamped(CellRange(from: (row: 2, column: 1), to: (row: 9, column: 2)))
        XCTAssertEqual(clamped, CellRange(from: (row: 2, column: 1), to: (row: 3, column: 2)))
        XCTAssertNil(GridRowSpace(fetched: 0).clamped(CellRange(from: (0, 0), to: (0, 0))))
    }

    // MARK: On a tab

    private func tab(rows: Int = 5) -> QueryTab {
        let tab = QueryTab(title: "Q")
        tab.showRows(columns: [Event.Column(name: "a", type: "bigint"), Event.Column(name: "b", type: "varchar")],
                     rows: (0..<rows).map { ["\($0)", "v\($0)"] },
                     truncated: false, queryID: nil, elapsedMS: 0)
        tab.sourceTable = "public.t"
        return tab
    }

    func testTheTabBuildsItsSpaceFromTheResultAndTheQueue() {
        let tab = tab()
        XCTAssertEqual(tab.rowSpace.count, 5)
        tab.cellEdits.insertRow()
        XCTAssertEqual(tab.rowSpace.count, 6)
        XCTAssertEqual(tab.rowSpace.kind(ofTableRow: 5), .inserted(id: -1, index: 0))
    }

    func testDeletingAnEarlierAddedRowShiftsTheNextOneAndTheSelectionIsClamped() {
        let tab = tab()
        let first = tab.cellEdits.insertRow()
        _ = tab.cellEdits.insertRow()
        let last = CellPos(row: 6, column: 1)
        tab.selectCells(anchor: last, focus: last)

        tab.cellEdits.removeInserted(row: first)
        tab.reconcileSelectionWithRowSpace()

        XCTAssertEqual(tab.rowSpace.count, 6)
        XCTAssertEqual(tab.cellSelection, CellRange(from: (row: 5, column: 1), to: (row: 5, column: 1)))
        XCTAssertEqual(tab.cellCursor?.focus, CellPos(row: 5, column: 1))
    }

    func testTheSelectionGoesWhenThereIsNoRowLeft() {
        let tab = QueryTab(title: "Q")
        tab.sourceTable = "public.t"
        let id = tab.cellEdits.insertRow()
        tab.selectCells(anchor: CellPos(row: 0, column: 0), focus: CellPos(row: 0, column: 0))
        XCTAssertEqual(tab.rowSpace.count, 1)

        tab.cellEdits.removeInserted(row: id)
        tab.reconcileSelectionWithRowSpace()

        XCTAssertNil(tab.cellSelection)
        XCTAssertNil(tab.cellCursor)
    }

    func testUndoRedoAndDiscardKeepTheSelectionInsideTheRows() {
        let tab = tab()
        let model = AppModel()
        model.addRow(in: tab)
        model.addRow(in: tab)
        XCTAssertEqual(tab.rowSpace.count, 7)
        XCTAssertEqual(tab.cellCursor?.focus.row, 6)

        tab.undoCellEdit()
        XCTAssertEqual(tab.rowSpace.count, 6)
        XCTAssertEqual(tab.cellCursor?.focus.row, 5, "the cursor does not rest on a row that is gone")

        tab.redoCellEdit()
        XCTAssertEqual(tab.rowSpace.count, 7)

        tab.discardCellEdits()
        XCTAssertEqual(tab.rowSpace.count, 5)
        XCTAssertEqual(tab.cellCursor?.focus.row, 4)
    }
}
