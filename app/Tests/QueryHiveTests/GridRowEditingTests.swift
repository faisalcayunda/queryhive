import AppKit
import XCTest

@testable import QueryHive

/// The table with rows the user added (blueprint w10 §5): the row space in the coordinator, the
/// gutter click, the keyboard and menu gestures, the accessibility tree and the undo wiring.
///
/// Driven through the coordinator the way `ResultGridTable` wires it, without `NSEvent`.
@MainActor
final class GridRowEditingTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
        GridFixture.ensureApplication()
    }

    private let columns = [Event.Column(name: "kode", type: "text"),
                           Event.Column(name: "nama", type: "text"),
                           Event.Column(name: "jumlah", type: "bigint")]

    /// Five fetched rows, with `added` rows added through the model the way the footer does.
    private func grid(added: Int = 0, showRowNumbers: Bool = true) -> GridFixture {
        var style = GridInputs.GridStyle.placeholder
        style.showRowNumbers = showRowNumbers
        let rows = (0..<5).map { ["K-\($0)", "Nama \($0)", "\($0)"] as [String?] }
        let fixture = GridFixture(columns: columns, rows: rows, style: style)
        fixture.tab.sourceTable = "public.t"
        fixture.apply()
        for _ in 0..<added { fixture.model.addRow(in: fixture.tab) }
        if added > 0 { reapply(fixture) }
        return fixture
    }

    /// What `updateNSView` does after the queue changed.
    private func reapply(_ fixture: GridFixture) {
        fixture.apply(fixture.inputs(), force: false)
    }

    // MARK: Row space in the table

    func testTheTableCountsFetchedAndAddedRows() {
        let fixture = grid(added: 2)
        XCTAssertEqual(fixture.coordinator.tableRowCount, 7)
        XCTAssertEqual(fixture.coordinator.numberOfRows(in: fixture.table), 7)
        XCTAssertEqual(fixture.table.numberOfRows, 7)
        XCTAssertEqual(fixture.coordinator.rowCountForAX, 7)
    }

    func testAnAddedRowIsDrawnAsAnAddedRowAndCarriesWhatWasTypedInIt() {
        let fixture = grid(added: 1)
        fixture.tab.cellEdits.setInserted("Baru", row: -1, column: 1)
        fixture.tab.cellEdits.setInserted("DEFAULT", row: -1, column: 2)

        XCTAssertEqual(fixture.coordinator.rowState(5), .inserted)
        XCTAssertEqual(fixture.coordinator.rowState(0), .unchanged)
        XCTAssertTrue(fixture.coordinator.stagedColumns(5).isEmpty, "an added row is washed whole, with no dots")
        let text = fixture.coordinator.rowText(5, columns: 0..<3)
        XCTAssertEqual(text.cells, ["", "Baru", "DEFAULT"])
        XCTAssertNil(fixture.coordinator.resultRow(forTableRow: 5))
        XCTAssertEqual(fixture.coordinator.resultRow(forTableRow: 3), 3)
    }

    func testAKeyIsMadeForAnAddedRowWithItsNegativeIdAndForNothingPastTheEnd() {
        let fixture = grid(added: 1)
        XCTAssertEqual(fixture.coordinator.validKey(row: 5, source: 1), CellKey(row: -1, column: 1))
        XCTAssertEqual(fixture.coordinator.validKey(row: 4, source: 1), CellKey(row: 4, column: 1))
        XCTAssertNil(fixture.coordinator.validKey(row: 6, source: 1))
        XCTAssertNil(fixture.coordinator.validKey(row: 0, source: 9))
        XCTAssertEqual(fixture.coordinator.tableRow(for: CellKey(row: -1, column: 0)), 5)
    }

    func testPressingAnAddedRowsCellSelectsItsTableRowAndNeverANegativeOne() {
        let fixture = grid(added: 2)
        fixture.coordinator.press(at: fixture.pointInCell(row: 6, column: 1), clickCount: 1)
        fixture.coordinator.release()
        XCTAssertEqual(fixture.tab.cellSelection, CellRange(from: (row: 6, column: 1), to: (row: 6, column: 1)))
        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: 6, column: 1))
    }

    func testADragFromTheResultIntoTheAddedRowsReachesThemInTableRows() {
        let fixture = grid(added: 2)
        fixture.coordinator.press(at: fixture.pointInCell(row: 3, column: 0), clickCount: 1)
        fixture.coordinator.drag(to: fixture.pointInCell(row: 6, column: 2))
        fixture.coordinator.release()
        XCTAssertEqual(fixture.tab.cellSelection, CellRange(from: (row: 3, column: 0), to: (row: 6, column: 2)))
    }

    func testTheCursorMovesDownIntoTheAddedRowsAndStopsAtTheLast() {
        let fixture = grid(added: 2)
        fixture.coordinator.press(at: fixture.pointInCell(row: 4, column: 0), clickCount: 1)
        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.down)))
        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.down)))
        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.down)))
        XCTAssertEqual(fixture.tab.cellCursor?.focus.row, 6)
        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.end, command: true)))
        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: 6, column: 2))
    }

    // MARK: The gutter (DBX-63)

    func testAClickOnTheRowNumbersSelectsTheWholeRow() {
        let fixture = grid()
        let gutter = fixture.coordinator.geometry.gutter
        XCTAssertGreaterThan(gutter, 0)
        let y = fixture.pointInCell(row: 2, column: 0).y
        fixture.coordinator.press(at: CGPoint(x: gutter / 2, y: y), clickCount: 1)
        fixture.coordinator.release()
        XCTAssertEqual(fixture.tab.cellSelection, CellRange(from: (row: 2, column: 0), to: (row: 2, column: 2)))
    }

    func testADragFromTheRowNumbersKeepsChoosingWholeRowsAndAShiftClickExtendsThem() {
        let fixture = grid()
        let gutter = fixture.coordinator.geometry.gutter
        fixture.coordinator.press(at: CGPoint(x: gutter / 2, y: fixture.pointInCell(row: 1, column: 0).y),
                                  clickCount: 1)
        fixture.coordinator.drag(to: fixture.pointInCell(row: 3, column: 0))
        fixture.coordinator.release()
        XCTAssertEqual(fixture.tab.cellSelection, CellRange(from: (row: 1, column: 0), to: (row: 3, column: 2)),
                       "the pointer left the gutter but the selection is still whole rows")

        fixture.coordinator.press(at: CGPoint(x: gutter / 2, y: fixture.pointInCell(row: 4, column: 0).y),
                                  clickCount: 1, extend: true)
        fixture.coordinator.release()
        XCTAssertEqual(fixture.tab.cellSelection, CellRange(from: (row: 1, column: 0), to: (row: 4, column: 2)))
    }

    func testADoubleClickOnTheRowNumbersOpensNoEditor() {
        let fixture = grid()
        let gutter = fixture.coordinator.geometry.gutter
        fixture.coordinator.press(at: CGPoint(x: gutter / 2, y: fixture.pointInCell(row: 2, column: 0).y),
                                  clickCount: 2)
        XCTAssertNil(fixture.coordinator.editor)
        XCTAssertTrue(fixture.log.beginEdit.isEmpty)
    }

    func testWithoutRowNumbersAClickAtTheEdgeIsAnOrdinaryCell() {
        let fixture = grid(showRowNumbers: false)
        XCTAssertEqual(fixture.coordinator.geometry.gutter, 0)
        fixture.coordinator.press(at: fixture.pointInCell(row: 2, column: 1), clickCount: 1)
        XCTAssertEqual(fixture.tab.cellSelection, CellRange(from: (row: 2, column: 1), to: (row: 2, column: 1)))
    }

    // MARK: The keyboard and the menu

    func testDeleteOnASelectedRowMarksItAndAnAddedRowIsTakenBack() {
        let fixture = grid(added: 1)
        fixture.coordinator.press(at: fixture.pointInCell(row: 1, column: 0), clickCount: 1)
        XCTAssertTrue(fixture.coordinator.keyState.canDeleteRows)
        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.deleteBackward)))
        XCTAssertEqual(fixture.tab.cellEdits.deletedRows, [1])

        fixture.coordinator.press(at: fixture.pointInCell(row: 5, column: 0), clickCount: 1)
        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.deleteForward)))
        XCTAssertTrue(fixture.tab.cellEdits.inserted.isEmpty)
        XCTAssertEqual(fixture.tab.cellEdits.deletedRows, [1])
    }

    func testTheKeyIsLeftToAppKitWhenRowsCannotBeDeleted() {
        let fixture = grid()
        fixture.tab.sourceTable = nil
        fixture.coordinator.press(at: fixture.pointInCell(row: 1, column: 0), clickCount: 1)
        XCTAssertFalse(fixture.coordinator.keyState.canDeleteRows)
        XCTAssertFalse(fixture.coordinator.handleKey(GridKey(.deleteBackward)))
        XCTAssertTrue(fixture.tab.cellEdits.isEmpty)
    }

    func testCommandDeleteReachesTheTableAsDeleteToBeginningOfLineAndDeletesTheRows() {
        let fixture = grid()
        fixture.coordinator.press(at: fixture.pointInCell(row: 2, column: 0), clickCount: 1)
        fixture.table.deleteToBeginningOfLine(nil)
        XCTAssertEqual(fixture.tab.cellEdits.deletedRows, [2])
    }

    func testTheMenuOffersTheRowActionsAndSaysWhyOneIsOff() {
        let fixture = grid()
        fixture.coordinator.press(at: fixture.pointInCell(row: 1, column: 0), clickCount: 1)
        fixture.coordinator.drag(to: fixture.pointInCell(row: 3, column: 0))
        let menu = fixture.coordinator.bodyMenu()
        let titles = menu.items.map(\.title)
        XCTAssertTrue(titles.contains("Add Row"))
        XCTAssertTrue(titles.contains("Delete 3 Rows"))
        XCTAssertTrue(titles.contains("Restore Row"))
        XCTAssertEqual(menu.items.first { $0.title == "Add Row" }?.isEnabled, true)
        XCTAssertEqual(menu.items.first { $0.title == "Restore Row" }?.isEnabled, false,
                       "nothing in the selection is marked for deletion")

        fixture.coordinator.deleteRows()
        let after = fixture.coordinator.bodyMenu()
        XCTAssertEqual(after.items.first { $0.title == "Restore Row" }?.isEnabled, true)

        fixture.tab.sourceTable = nil
        let off = fixture.coordinator.bodyMenu()
        XCTAssertEqual(off.items.first { $0.title == "Add Row" }?.isEnabled, false)
        XCTAssertEqual(off.items.first { $0.title == "Add Row" }?.toolTip, "Open a table to add rows")
    }

    // MARK: Editing an added row's cell

    func testTheEditorOverAnAddedRowStagesItsValueAsTheRowsOwn() {
        let fixture = grid(added: 1)
        let key = CellKey(row: -1, column: 1)
        fixture.coordinator.beginEdit(at: key)
        XCTAssertNotNil(fixture.coordinator.editor)
        XCTAssertEqual(fixture.coordinator.editor?.frame.minY ?? 0, fixture.table.rect(ofRow: 5).minY, accuracy: 6)

        fixture.coordinator.commitEdit(at: key, text: "Baru")
        XCTAssertEqual(fixture.tab.cellEdits.insertedValue(row: -1, column: 1), "Baru")
        XCTAssertTrue(fixture.tab.cellEdits.values.isEmpty)
        XCTAssertFalse(fixture.tab.hasOpenCellEdit, "the session ended with the commit")
    }

    func testAFillOverABlockEndsItsSessionAndLeavesNoneOpen() {
        let fixture = grid(added: 1)
        fixture.coordinator.press(at: fixture.pointInCell(row: 3, column: 1), clickCount: 1)
        fixture.coordinator.drag(to: fixture.pointInCell(row: 5, column: 1))
        fixture.coordinator.release()
        fixture.coordinator.beginEdit(at: CellKey(row: 3, column: 1))
        fixture.coordinator.commitEdit(at: CellKey(row: 3, column: 1), text: "Z")
        XCTAssertFalse(fixture.tab.hasOpenCellEdit)
        XCTAssertEqual(Set(fixture.tab.cellEdits.values.keys.map(\.row)), [3, 4])
        XCTAssertEqual(fixture.tab.cellEdits.insertedValue(row: -1, column: 1), "Z")
    }

    func testTheEditorGoesWhenItsAddedRowDoes() {
        let fixture = grid(added: 1)
        let key = CellKey(row: -1, column: 1)
        fixture.coordinator.beginEdit(at: key)
        fixture.tab.undoCellEdit()   // takes the added row back
        reapply(fixture)
        XCTAssertNil(fixture.coordinator.editor)
        XCTAssertFalse(fixture.tab.hasOpenCellEdit)
    }

    func testNoEditorOpensWhenTheTabRefusesTheSession() {
        let fixture = grid()
        fixture.tab.cellEdits.deleteRow(1)
        fixture.coordinator.beginEdit(at: CellKey(row: 1, column: 1))
        XCTAssertNil(fixture.coordinator.editor, "a row marked for deletion takes no edit")
        XCTAssertNil(fixture.coordinator.editingKey)

        fixture.tab.applying = true
        fixture.coordinator.beginEdit(at: CellKey(row: 2, column: 1))
        XCTAssertNil(fixture.coordinator.editor, "nothing new is staged under an apply")

        fixture.tab.applying = false
        fixture.tab.rowsStaleAfterApply = true
        fixture.coordinator.beginEdit(at: CellKey(row: 2, column: 1))
        XCTAssertNil(fixture.coordinator.editor, "stale rows take no edit")
        XCTAssertFalse(fixture.tab.hasOpenCellEdit)
    }

    func testTheEditorGoesWhenItsSessionIsEndedFromOutside() {
        let fixture = grid()
        let key = CellKey(row: 2, column: 1)
        fixture.coordinator.beginEdit(at: key)
        reapply(fixture)   // the update that follows the session opening
        fixture.tab.typeCellEdit("saved by cmd-S")
        fixture.tab.endCellEdit()   // what Save, Review and Apply do
        reapply(fixture)
        XCTAssertNil(fixture.coordinator.editor)
        XCTAssertNil(fixture.coordinator.editingKey)
        XCTAssertEqual(fixture.tab.cellEdits.value(at: key), "saved by cmd-S")
    }

    // MARK: Undo through the responder chain (D-6)

    func testTheTableOwnsTheTabsUndoManagerAndAnswersUndoAndRedo() {
        let fixture = grid()
        XCTAssertTrue(fixture.table.undoManager === fixture.tab.editUndoManager)
        XCTAssertFalse(fixture.table.validateUserInterfaceItem(UndoItem(#selector(GridTableView.undo(_:)))))

        fixture.model.addRow(in: fixture.tab)
        XCTAssertTrue(fixture.table.validateUserInterfaceItem(UndoItem(#selector(GridTableView.undo(_:)))))
        XCTAssertEqual(fixture.tab.editUndoManager.undoActionName, "Add Row")

        fixture.table.undo(nil)
        XCTAssertTrue(fixture.tab.cellEdits.isEmpty)
        XCTAssertTrue(fixture.table.validateUserInterfaceItem(UndoItem(#selector(GridTableView.redo(_:)))))
        fixture.table.redo(nil)
        XCTAssertEqual(fixture.tab.cellEdits.inserted.count, 1)
    }

    private final class UndoItem: NSObject, NSValidatedUserInterfaceItem {
        let action: Selector?
        let tag = 0
        init(_ action: Selector) { self.action = action }
    }

    // MARK: Accessibility

    func testAnAddedRowIsInTheTreeAtItsTableRowAndIsNamedNewRow() throws {
        let fixture = grid(added: 1)
        fixture.tab.cellEdits.setInserted("Baru", row: -1, column: 1)
        _ = fixture.table.accessibilityChildren()

        let cell = try XCTUnwrap(fixture.coordinator.ax.cell(row: 5, column: 1))
        XCTAssertEqual(cell.key, CellKey(row: -1, column: 1))
        XCTAssertEqual(cell.accessibilityLabel(), "Row 6, column 2, nama: Baru, new row")
        XCTAssertEqual(cell.accessibilityRowIndexRange(), NSRange(location: 5, length: 1))
        let blank = try XCTUnwrap(fixture.coordinator.ax.cell(row: 5, column: 2))
        XCTAssertEqual(blank.accessibilityLabel(), "Row 6, column 3, jumlah: default, new row")
        XCTAssertEqual(blank.accessibilityValue() as? String, "default")
        XCTAssertEqual(fixture.table.accessibilityRowCount(), 6)
    }

    func testTheCursorOnAnAddedRowIsTheFocusedElement() throws {
        let fixture = grid(added: 1)
        _ = fixture.table.accessibilityChildren()
        let focused = try XCTUnwrap(fixture.coordinator.ax.focusedCell)
        XCTAssertEqual(focused.key.row, -1, "addRow put the cursor on the new row")
        XCTAssertTrue(fixture.coordinator.isAXFocused(focused.key))
    }

    // MARK: The Record panel and the cursor on an added row

    func testTheRecordPanelPlacesTheCursorOnAnAddedRowsField() {
        let fixture = grid(added: 1)
        let field = RecordField(source: 1, position: 1, name: "nama", columnName: "nama", type: "text",
                                state: .inserted)
        let key = fixture.tab.placeCursor(on: field)
        XCTAssertEqual(key, CellKey(row: -1, column: 1))
        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: 5, column: 1))
    }

    // MARK: Painting

    func testTheTableDrawsWithAnAddedRowWithoutCrashing() throws {
        let fixture = grid(added: 2)
        fixture.table.frame = NSRect(x: 0, y: 0, width: 600, height: CGFloat(fixture.coordinator.tableRowCount) * 25)
        let bitmap = try XCTUnwrap(fixture.table.bitmapImageRepForCachingDisplay(in: fixture.table.bounds))
        fixture.table.cacheDisplay(in: fixture.table.bounds, to: bitmap)
        XCTAssertGreaterThan(bitmap.pixelsHigh, 0)
    }

    func testAddingARowRepaintsItAndGrowsTheTable() {
        let fixture = grid()
        fixture.table.invalidationLog = []
        fixture.model.addRow(in: fixture.tab)
        reapply(fixture)
        XCTAssertEqual(fixture.table.numberOfRows, 6)
        let repainted = fixture.table.invalidationLog ?? []
        let addedRect = fixture.table.rect(ofRow: 5)
        XCTAssertTrue(repainted.contains { $0.intersects(addedRect) }, "the added row's rectangle was repainted")
    }
}
