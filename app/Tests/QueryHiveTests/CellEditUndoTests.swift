import XCTest

@testable import QueryHive

/// Undo for the grid's queued edits, coalesced per editing session.
///
/// The rule the study paid for: one typed word is one undo, not one per keystroke, and a cell typed
/// back to the value it started from is no undo step at all. These tests drive the session the way
/// the editor does — a keystroke at a time, then an end — and check the queue and the history.
final class CellEditUndoTests: XCTestCase {
    override func setUp() {
        super.setUp()
        // Before any QueryTab is built: its constructor reads the stored output directory.
        isolateConnectionStore()
    }

    private func tab() -> QueryTab {
        let tab = QueryTab(title: "Q")
        tab.showRows(columns: [Event.Column(name: "a", type: "bigint"),
                                              Event.Column(name: "b", type: "varchar")],
                                    rows: [["3", "x"], ["1", "y"]],
                                    truncated: false, queryID: nil, elapsedMS: 0)
        return tab
    }

    private let key = CellKey(row: 0, column: 0)

    func testOneTypedWordIsOneUndoStep() {
        let tab = tab()
        tab.beginCellEdit(at: key)
        for text in ["h", "he", "hel", "hell", "hello"] { tab.typeCellEdit(text) }

        // Nothing has reached the queue while the session is open.
        XCTAssertNil(tab.cellEdits.value(at: key))
        tab.endCellEdit()

        XCTAssertEqual(tab.cellEdits.value(at: key), "hello")
        // A single undo clears the whole word rather than stepping back through it.
        tab.undoCellEdit()
        XCTAssertNil(tab.cellEdits.value(at: key))
        XCTAssertTrue(tab.canRedoCellEdit)
        tab.redoCellEdit()
        XCTAssertEqual(tab.cellEdits.value(at: key), "hello")
    }

    func testTypingBackToTheOriginalValueIsNoUndoStep() {
        let tab = tab()
        tab.beginCellEdit(at: key)
        tab.typeCellEdit("9")
        tab.typeCellEdit("3")   // the value the cell was fetched with
        tab.endCellEdit()

        XCTAssertNil(tab.cellEdits.value(at: key), "a no-op is not a change")
        XCTAssertFalse(tab.canUndoCellEdit, "and an undo that does nothing is worse than no undo")
    }

    func testTypingBackToTheFetchedValueRemovesAnEarlierStagedEdit() {
        let tab = tab()
        // A prior edit is already queued. Editing it back to the fetched value unstages it, and that
        // is a real change — undo has to be able to bring the staged value back.
        tab.cellEdits.edit("first", at: key, original: "3")

        tab.beginCellEdit(at: key)
        tab.typeCellEdit("3")
        tab.endCellEdit()

        XCTAssertTrue(tab.cellEdits.isEmpty)
        XCTAssertTrue(tab.canUndoCellEdit)
        tab.undoCellEdit()
        XCTAssertEqual(tab.cellEdits.value(at: key), "first")
    }

    func testCancellingASessionLeavesTheQueueUntouched() {
        let tab = tab()
        tab.beginCellEdit(at: key)
        tab.typeCellEdit("9")
        tab.cancelCellEdit()

        XCTAssertTrue(tab.cellEdits.isEmpty)
        XCTAssertFalse(tab.canUndoCellEdit)
    }

    func testEachSessionIsItsOwnStep() {
        let tab = tab()
        let other = CellKey(row: 1, column: 0)

        tab.beginCellEdit(at: key)
        tab.typeCellEdit("9")
        tab.endCellEdit()
        tab.beginCellEdit(at: other)
        tab.typeCellEdit("8")
        tab.endCellEdit()

        XCTAssertEqual(tab.cellEdits.value(at: key), "9")
        XCTAssertEqual(tab.cellEdits.value(at: other), "8")

        // The first undo takes back only the second session.
        tab.undoCellEdit()
        XCTAssertEqual(tab.cellEdits.value(at: key), "9")
        XCTAssertNil(tab.cellEdits.value(at: other))
    }

    func testAWholeBlockFillIsOneStep() {
        let tab = tab()
        tab.fillCellEdits("Z", over: CellRange(from: (row: 0, column: 0), to: (row: 1, column: 0)))

        XCTAssertEqual(tab.cellEdits.values.count, 2)
        tab.undoCellEdit()
        XCTAssertTrue(tab.cellEdits.isEmpty)
    }

    func testDiscardingTheQueueIsOneStepAndCanBeUndone() {
        let tab = tab()
        tab.cellEdits.edit("9", at: key, original: "3")
        tab.cellEdits.edit("8", at: CellKey(row: 1, column: 0), original: "1")

        tab.discardCellEdits()

        XCTAssertTrue(tab.cellEdits.isEmpty)
        tab.undoCellEdit()
        XCTAssertEqual(tab.cellEdits.value(at: key), "9")
        XCTAssertEqual(tab.cellEdits.value(at: CellKey(row: 1, column: 0)), "8")
    }

    func testAFilterDropsTheUndoHistoryWithTheQueue() {
        let tab = tab()
        tab.beginCellEdit(at: key)
        tab.typeCellEdit("9")
        tab.endCellEdit()
        XCTAssertTrue(tab.canUndoCellEdit)

        // The filter throws the queue away, so the undo step that named those cells goes too: an
        // undo would otherwise put an edit back onto a row the filter hid.
        tab.columnFilters[0] = .text("3")
        XCTAssertFalse(tab.canUndoCellEdit)
    }

    func testEachRowGestureIsOneStepWithItsOwnNameAndARedo() {
        let tab = tab()
        tab.sourceTable = "public.t"
        let model = AppModel()

        model.addRow(in: tab)
        XCTAssertEqual(tab.editUndoManager.undoActionName, "Add Row")
        tab.selectCells(anchor: CellPos(row: 0, column: 0), focus: CellPos(row: 1, column: 0))
        model.deleteRows(in: tab)
        XCTAssertEqual(tab.editUndoManager.undoActionName, "Delete Rows")
        model.restoreRows(in: tab)
        XCTAssertEqual(tab.editUndoManager.undoActionName, "Restore Rows")

        tab.undoCellEdit()
        XCTAssertEqual(tab.cellEdits.deletedRows.sorted(), [0, 1])
        XCTAssertEqual(tab.editUndoManager.redoActionName, "Restore Rows")
        tab.undoCellEdit()
        XCTAssertTrue(tab.cellEdits.deletedRows.isEmpty)
        XCTAssertEqual(tab.cellEdits.inserted.count, 1, "one undo per gesture, so the added row is still there")
        tab.undoCellEdit()
        XCTAssertTrue(tab.cellEdits.isEmpty)
        XCTAssertFalse(tab.canUndoCellEdit)

        tab.redoCellEdit()
        tab.redoCellEdit()
        tab.redoCellEdit()
        XCTAssertEqual(tab.cellEdits.inserted.count, 1)
        XCTAssertTrue(tab.cellEdits.deletedRows.isEmpty)
    }

    func testDiscardingIsNamedAndUndoable() {
        let tab = tab()
        tab.cellEdits.edit("9", at: key, original: "3")
        tab.discardCellEdits()
        XCTAssertEqual(tab.editUndoManager.undoActionName, "Discard Changes")
    }

    func testABlockFillMapsDisplayPositionsToSourceColumns() {
        // The fill's columns are display positions and `columns` maps them to the source indices a
        // cell is keyed by, so a block filled over a reordered grid edits the columns it covers.
        let tab = tab()
        tab.moveColumn(from: 0, to: 1)   // drawn order is now b, a
        tab.fillCellEdits("Z", over: CellRange(from: (row: 0, column: 0), to: (row: 0, column: 0)))

        // The first drawn column is source 1.
        XCTAssertEqual(tab.cellEdits.value(at: CellKey(row: 0, column: 1)), "Z")
        XCTAssertNil(tab.cellEdits.value(at: CellKey(row: 0, column: 0)))
    }
}
