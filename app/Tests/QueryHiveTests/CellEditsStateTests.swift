import XCTest

@testable import QueryHive

/// What the grid says about a cell and a row (W10-T2, blueprint w10 §4.2): one answer for the
/// painter, the accessibility label and the tests.
final class CellEditsStateTests: XCTestCase {
    func testAnUntouchedEditQueueSaysUnchanged() {
        let edits = CellEdits()
        XCTAssertEqual(edits.state(of: CellKey(row: 0, column: 0)), .unchanged)
        XCTAssertEqual(edits.rowState(0), .unchanged)
        XCTAssertEqual(edits.rowState(-1), .unchanged, "a negative id that is not in the queue is nothing")
    }

    func testAStagedCellIsModifiedAndItsRowSaysModifiedToo() {
        var edits = CellEdits()
        edits.edit("x", at: CellKey(row: 1, column: 1), original: "4")
        XCTAssertEqual(edits.state(of: CellKey(row: 1, column: 1)), .modified)
        XCTAssertEqual(edits.state(of: CellKey(row: 1, column: 0)), .unchanged, "the neighbour is untouched")
        XCTAssertEqual(edits.rowState(1), .modified)
        XCTAssertEqual(edits.rowState(0), .unchanged)
    }

    func testUnstagingTheEditPutsTheStateBack() {
        var edits = CellEdits()
        edits.edit("x", at: CellKey(row: 1, column: 1), original: "4")
        edits.edit("4", at: CellKey(row: 1, column: 1), original: "4")
        XCTAssertEqual(edits.state(of: CellKey(row: 1, column: 1)), .unchanged)
        XCTAssertEqual(edits.rowState(1), .unchanged)
    }

    func testADeletedRowIsDeletedInEveryCellAndAbsorbsItsStagedEdits() {
        var edits = CellEdits()
        edits.edit("x", at: CellKey(row: 2, column: 0), original: "5")
        edits.deleteRow(2)
        XCTAssertEqual(edits.rowState(2), .deleted)
        XCTAssertEqual(edits.state(of: CellKey(row: 2, column: 0)), .deleted, "a deleted row is not also modified")
        XCTAssertEqual(edits.state(of: CellKey(row: 2, column: 1)), .deleted)
        XCTAssertEqual(edits.rowState(0), .unchanged)
    }

    func testAnAddedRowIsInsertedInEveryCellByItsNegativeId() {
        var edits = CellEdits()
        let id = edits.insertRow()
        XCTAssertLessThan(id, 0)
        edits.setInserted("v", row: id, column: 1)
        XCTAssertEqual(edits.rowState(id), .inserted)
        XCTAssertEqual(edits.state(of: CellKey(row: id, column: 0)), .inserted, "an empty cell of an added row too")
        XCTAssertEqual(edits.state(of: CellKey(row: id, column: 1)), .inserted)
        XCTAssertEqual(edits.rowState(id - 1), .unchanged, "another negative id is not in the queue")
        XCTAssertEqual(edits.rowState(0), .unchanged, "fetched rows are not touched by an added one")
    }

    func testEveryKindOfChangeAtOnceKeepsEachRowItsOwn() {
        var edits = CellEdits()
        let added = edits.insertRow()
        edits.edit("x", at: CellKey(row: 0, column: 0), original: "1")
        edits.deleteRow(1)
        XCTAssertEqual([edits.rowState(0), edits.rowState(1), edits.rowState(2), edits.rowState(added)],
                       [.modified, .deleted, .unchanged, .inserted])
    }

    func testDiscardingClearsEveryState() {
        var edits = CellEdits()
        let added = edits.insertRow()
        edits.deleteRow(1)
        edits.edit("x", at: CellKey(row: 0, column: 0), original: "1")
        edits.discard()
        XCTAssertEqual([edits.rowState(0), edits.rowState(1), edits.rowState(added)],
                       [.unchanged, .unchanged, .unchanged])
    }
}
