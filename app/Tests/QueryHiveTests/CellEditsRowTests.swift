import XCTest

@testable import QueryHive

/// Adding, deleting and restoring rows, and what a block that crosses from the fetched rows into the
/// added ones does (blueprint w10 §5.1 and §5.5).
///
/// The contract these hold: `CellEdits.values` has only keys `0 <= row < fetched`, `deletedRows` only
/// indices below `fetched`, and a negative id lives in `inserted` alone, whatever mutator ran.
final class CellEditsRowTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    private let columns = [Event.Column(name: "id", type: "bigint"), Event.Column(name: "name", type: "varchar")]

    /// Five fetched rows and two added ones: table rows 5 and 6 are the added ones.
    private func tab(added: Int = 2) -> (QueryTab, AppModel) {
        let tab = QueryTab(title: "Q")
        tab.showRows(columns: columns, rows: (0..<5).map { ["\($0)", "v\($0)"] },
                     truncated: false, queryID: nil, elapsedMS: 0)
        tab.sourceTable = "public.t"
        let model = AppModel()
        for _ in 0..<added { model.addRow(in: tab) }
        return (tab, model)
    }

    private func assertTheContract(_ tab: QueryTab, file: StaticString = #filePath, line: UInt = #line) {
        let fetched = tab.result.count
        XCTAssertTrue(tab.cellEdits.values.keys.allSatisfy { 0 <= $0.row && $0.row < fetched },
                      "a staged cell is keyed to a fetched row", file: file, line: line)
        XCTAssertTrue(tab.cellEdits.deletedRows.allSatisfy { 0 <= $0 && $0 < fetched },
                      "a deleted row is a fetched one", file: file, line: line)
        XCTAssertTrue(tab.cellEdits.inserted.allSatisfy { $0.id < 0 }, file: file, line: line)
    }

    // MARK: The queue

    func testInsertRowIdsCountDownAndAreNeverReused() {
        var edits = CellEdits()
        let first = edits.insertRow(), second = edits.insertRow()
        XCTAssertEqual([first, second], [-1, -2])
        edits.removeInserted(row: first)
        XCTAssertEqual(edits.insertRow(), -3, "a removed id is not handed out again")
    }

    func testRemoveInsertedTakesTheRowAndItsValuesOut() {
        var edits = CellEdits()
        let id = edits.insertRow()
        edits.setInserted("x", row: id, column: 1)
        edits.removeInserted(row: id)
        XCTAssertTrue(edits.isEmpty)
        XCTAssertNil(edits.insertedValue(row: id, column: 1))
    }

    func testDeleteRowDropsItsStagedEditsAndRestoreLetsItGoWithoutBringThemBack() {
        var edits = CellEdits()
        edits.edit("new", at: CellKey(row: 2, column: 1), original: "v2")
        edits.deleteRow(2)
        XCTAssertNil(edits.value(at: CellKey(row: 2, column: 1)))
        XCTAssertTrue(edits.isDeleted(2))

        edits.restoreRow(2)
        XCTAssertFalse(edits.isDeleted(2))
        XCTAssertNil(edits.value(at: CellKey(row: 2, column: 1)), "the edit went with the mark")
        XCTAssertTrue(edits.isEmpty)
    }

    func testARowMarkedForDeletionTakesNoEdit() {
        var edits = CellEdits()
        edits.deleteRow(1)
        edits.edit("x", at: CellKey(row: 1, column: 1), original: "v1")
        edits.fill("y", over: CellRange(from: (row: 0, column: 1), to: (row: 2, column: 1)),
                   rows: [["0", "v0"], ["1", "v1"], ["2", "v2"]])
        XCTAssertNil(edits.value(at: CellKey(row: 1, column: 1)))
        XCTAssertEqual(edits.value(at: CellKey(row: 0, column: 1)), "y")
    }

    func testDeleteRowsInOnePassMatchesDeletingThemOneByOne() {
        var bulk = CellEdits(), single = CellEdits()
        bulk.edit("x", at: CellKey(row: 1, column: 0), original: "1")
        single.edit("x", at: CellKey(row: 1, column: 0), original: "1")
        bulk.deleteRow(3)
        single.deleteRow(3)
        bulk.deleteRows(1...4)
        for row in 1...4 { single.deleteRow(row) }
        XCTAssertEqual(bulk.deletedRows.sorted(), single.deletedRows.sorted())
        XCTAssertEqual(bulk.deletedRows.count, 4, "row 3 is marked once")
        XCTAssertTrue(bulk.values.isEmpty)
    }

    func testFillSkipsARowTheStoreCannotRead() {
        var edits = CellEdits()
        edits.fill("z", over: CellRange(from: (row: 1, column: 0), to: (row: 4, column: 0)),
                   rows: [["a"], ["b"], ["c"]])
        XCTAssertEqual(Set(edits.values.keys.map(\.row)), [1, 2], "rows 3 and 4 are not in the rows")
    }

    // MARK: The actions

    func testAddingARowIsOneUndoStepNamedForIt() {
        let (tab, model) = tab(added: 0)
        model.addRow(in: tab)
        XCTAssertEqual(tab.cellEdits.inserted.count, 1)
        XCTAssertEqual(tab.editUndoManager.undoActionName, "Add Row")
        XCTAssertEqual(tab.cellCursor?.focus, CellPos(row: 5, column: 0), "the cursor is on the new row")

        tab.undoCellEdit()
        XCTAssertTrue(tab.cellEdits.isEmpty)
        XCTAssertEqual(tab.editUndoManager.redoActionName, "Add Row")
        tab.redoCellEdit()
        XCTAssertEqual(tab.cellEdits.inserted.count, 1)
    }

    func testDeletingFetchedRowsMarksThemAndOneUndoStepRestoresAll() {
        let (tab, model) = tab(added: 0)
        tab.selectCells(anchor: CellPos(row: 1, column: 0), focus: CellPos(row: 3, column: 1))
        model.deleteRows(in: tab)
        XCTAssertEqual(tab.cellEdits.deletedRows.sorted(), [1, 2, 3])
        XCTAssertEqual(tab.editUndoManager.undoActionName, "Delete Rows")

        tab.undoCellEdit()
        XCTAssertTrue(tab.cellEdits.isEmpty)
    }

    func testDeletingARowIsNamedInTheSingular() {
        let (tab, model) = tab(added: 0)
        tab.selectCells(anchor: CellPos(row: 1, column: 0), focus: CellPos(row: 1, column: 0))
        model.deleteRows(in: tab)
        XCTAssertEqual(tab.editUndoManager.undoActionName, "Delete Row")
    }

    func testRestoreRowsLetsMarkedRowsGoInOneStep() {
        let (tab, model) = tab(added: 0)
        tab.selectCells(anchor: CellPos(row: 1, column: 0), focus: CellPos(row: 2, column: 0))
        model.deleteRows(in: tab)
        model.restoreRows(in: tab)
        XCTAssertTrue(tab.cellEdits.deletedRows.isEmpty)
        XCTAssertEqual(tab.editUndoManager.undoActionName, "Restore Rows")
        tab.undoCellEdit()
        XCTAssertEqual(tab.cellEdits.deletedRows.sorted(), [1, 2])
    }

    func testAnAddedRowThatIsDeletedIsTakenBackAndNeverBecomesADelete() {
        let (tab, model) = tab(added: 1)
        tab.selectCells(anchor: CellPos(row: 5, column: 0), focus: CellPos(row: 5, column: 0))
        model.deleteRows(in: tab)
        XCTAssertTrue(tab.cellEdits.isEmpty)
        assertTheContract(tab)
        tab.undoCellEdit()
        XCTAssertEqual(tab.cellEdits.inserted.count, 1, "undo puts the added row back")
    }

    // MARK: A block that crosses the border (fetched 5, added 2: table rows 5 and 6)

    func testFillOverTheBorderStagesAnUpdateForFetchedRowsAndTheValueForAddedOnes() {
        let (tab, _) = tab()
        tab.fillCellEdits("Z", over: CellRange(from: (row: 3, column: 1), to: (row: 6, column: 1)))

        XCTAssertEqual(Set(tab.cellEdits.values.keys.map(\.row)), [3, 4])
        XCTAssertEqual(tab.cellEdits.insertedValue(row: -1, column: 1), "Z")
        XCTAssertEqual(tab.cellEdits.insertedValue(row: -2, column: 1), "Z")
        assertTheContract(tab)
        XCTAssertEqual(tab.editUndoManager.undoActionName, "Edit Cell")
        tab.undoCellEdit()
        XCTAssertTrue(tab.cellEdits.values.isEmpty)
        XCTAssertEqual(tab.cellEdits.inserted.count, 2, "one step undoes the fill, not the added rows")
        XCTAssertNil(tab.cellEdits.insertedValue(row: -1, column: 1))
    }

    func testFillWithNothingEmptiesTheValueOfAnAddedRow() {
        let (tab, _) = tab()
        tab.fillCellEdits("Z", over: CellRange(from: (row: 5, column: 1), to: (row: 5, column: 1)))
        tab.fillCellEdits("", over: CellRange(from: (row: 5, column: 1), to: (row: 5, column: 1)))
        XCTAssertNil(tab.cellEdits.insertedValue(row: -1, column: 1), "empty text takes the value out")
    }

    func testPasteOverTheBorderPutsOneLineInTheResultAndOneInEachAddedRowAndDropsTheRest() {
        let (tab, _) = tab()
        // Four lines from table row 4: row 4 is fetched, 5 and 6 are added, 7 is past the end.
        tab.pasteCellEdits("p4\np5\np6\np7", at: CellKey(row: 4, column: 1), columnCount: 2)

        XCTAssertEqual(tab.cellEdits.value(at: CellKey(row: 4, column: 1)), "p4")
        XCTAssertEqual(tab.cellEdits.insertedValue(row: -1, column: 1), "p5")
        XCTAssertEqual(tab.cellEdits.insertedValue(row: -2, column: 1), "p6")
        XCTAssertEqual(tab.cellEdits.inserted.count, 2, "a paste adds no rows")
        XCTAssertEqual(tab.rowSpace.count, 7)
        assertTheContract(tab)
    }

    func testPasteStartingInAnAddedRowStaysInTheAddedRows() {
        let (tab, _) = tab()
        tab.pasteCellEdits("a\tb\nc\td", at: CellKey(row: 5, column: 0), columnCount: 2)
        XCTAssertTrue(tab.cellEdits.values.isEmpty)
        XCTAssertEqual(tab.cellEdits.insertedValue(row: -1, column: 0), "a")
        XCTAssertEqual(tab.cellEdits.insertedValue(row: -1, column: 1), "b")
        XCTAssertEqual(tab.cellEdits.insertedValue(row: -2, column: 1), "d")
    }

    func testDeleteOverTheBorderMarksTheFetchedRowsAndDropsTheAddedOnesInOneStep() {
        let (tab, model) = tab()
        tab.selectCells(anchor: CellPos(row: 3, column: 0), focus: CellPos(row: 6, column: 1))
        model.deleteRows(in: tab)

        XCTAssertEqual(tab.cellEdits.deletedRows.sorted(), [3, 4])
        XCTAssertTrue(tab.cellEdits.inserted.isEmpty)
        assertTheContract(tab)

        tab.undoCellEdit()
        XCTAssertTrue(tab.cellEdits.deletedRows.isEmpty)
        XCTAssertEqual(tab.cellEdits.inserted.count, 2, "one undo brings back everything")
    }

    func testCopyOverTheBorderGivesResultValuesThenStagedOnesInTableOrder() throws {
        let (tab, _) = tab()
        tab.cellEdits.setInserted("n5", row: -1, column: 1)
        let text = try GridClipboard.text(result: tab.result,
                                          selection: CellRange(from: (row: 4, column: 0), to: (row: 6, column: 1)),
                                          visible: [0, 1], withHeaders: false,
                                          inserted: tab.cellEdits.inserted)
        XCTAssertEqual(text, "4\tv4\n\tn5\n\t")
    }

    func testThePlanOfABlockThatCrossedTheBorderHasTheExpectedStatementsAndNoWarning() {
        let (tab, model) = tab()
        tab.fillCellEdits("Z", over: CellRange(from: (row: 3, column: 1), to: (row: 6, column: 1)))
        tab.selectCells(anchor: CellPos(row: 0, column: 0), focus: CellPos(row: 0, column: 0))
        model.deleteRows(in: tab)
        tab.cellEdits.setInserted("7", row: -1, column: 0)
        tab.cellEdits.setInserted("7", row: -2, column: 0)

        let plan = WritePlan.build(edits: tab.cellEdits, rows: tab.result, columns: columns,
                                   table: "public.t", kind: .postgres)

        XCTAssertEqual(plan.statements.map(\.kind), [.delete, .update, .update, .insert])
        XCTAssertTrue(plan.warnings.isEmpty, "\(plan.warnings)")
        XCTAssertFalse(plan.warnings.contains { $0.contains("could not be read") })
        assertTheContract(tab)
    }

    func testTheFetchedValueOfAnAddedRowIsNilAndNeverReadsTheStore() {
        let (tab, _) = tab()
        XCTAssertNil(tab.fetchedValue(at: CellKey(row: -1, column: 0)))
        XCTAssertNil(tab.cellValue(at: CellKey(row: -1, column: 0)))
        tab.cellEdits.setInserted("x", row: -1, column: 0)
        XCTAssertEqual(tab.cellValue(at: CellKey(row: -1, column: 0)), "x")
    }

    func testEditingTheCellOfAnAddedRowStagesItsValueAndUndoesAsOneStep() {
        let (tab, _) = tab(added: 1)
        let key = CellKey(row: -1, column: 1)
        tab.beginCellEdit(at: key)
        for text in ["h", "he", "hello"] { tab.typeCellEdit(text) }
        tab.endCellEdit()
        XCTAssertEqual(tab.cellEdits.insertedValue(row: -1, column: 1), "hello")
        XCTAssertTrue(tab.cellEdits.values.isEmpty, "an added row's cell is never an UPDATE")

        tab.undoCellEdit()
        XCTAssertNil(tab.cellEdits.insertedValue(row: -1, column: 1))
        XCTAssertEqual(tab.cellEdits.inserted.count, 1)
    }

    func testDEFAULTTypedIntoAnAddedRowBecomesTheKeyword() throws {
        let (tab, _) = tab(added: 1)
        tab.beginCellEdit(at: CellKey(row: -1, column: 0))
        tab.typeCellEdit("DEFAULT")
        tab.endCellEdit()
        tab.beginCellEdit(at: CellKey(row: -1, column: 1))
        tab.typeCellEdit("x")
        tab.endCellEdit()
        let plan = WritePlan.build(edits: tab.cellEdits, rows: tab.result, columns: columns,
                                   table: "public.t", kind: .postgres)
        let insert = try XCTUnwrap(plan.statements.first)
        XCTAssertTrue(insert.sql.contains("(DEFAULT, 'x')"), insert.sql)
    }

    func testABlockFillLeavesNoEditSessionOpen() {
        let (tab, _) = tab(added: 0)
        tab.beginCellEdit(at: CellKey(row: 0, column: 0))
        XCTAssertTrue(tab.hasOpenCellEdit)
        tab.fillCellEdits("Z", over: CellRange(from: (row: 0, column: 0), to: (row: 1, column: 0)))
        tab.cancelCellEdit()
        XCTAssertFalse(tab.hasOpenCellEdit, "the fill replaces the session; nothing is left open")
    }

    func testARowMarkedForDeletionRefusesAnEditSession() {
        let (tab, model) = tab(added: 0)
        tab.selectCells(anchor: CellPos(row: 2, column: 0), focus: CellPos(row: 2, column: 0))
        model.deleteRows(in: tab)
        tab.beginCellEdit(at: CellKey(row: 2, column: 1))
        XCTAssertFalse(tab.hasOpenCellEdit)
    }
}
