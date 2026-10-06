import AppKit
import XCTest

@testable import QueryHive

/// Where a motion takes the cursor (blueprint W10 §3.1), and the scroll that follows it. Pure.
final class GridCursorMathTests: XCTestCase {

    private let bounds = GridBounds(rows: 100, columns: 5, page: 20)

    private func cursor(_ row: Int, _ column: Int) -> GridCursor {
        GridCursor(anchor: CellPos(row: row, column: column), focus: CellPos(row: row, column: column))
    }

    private func move(_ motion: GridMotion, from: GridCursor, extending: Bool = false,
                      in bounds: GridBounds? = nil) -> GridCursor? {
        GridCursorMath.apply(motion, extending: extending, to: from, in: bounds ?? self.bounds)
    }

    // MARK: One cell

    func testAStepMovesOneCell() {
        XCTAssertEqual(move(.step(rows: 1, columns: 0), from: cursor(5, 2)), cursor(6, 2))
        XCTAssertEqual(move(.step(rows: -1, columns: 0), from: cursor(5, 2)), cursor(4, 2))
        XCTAssertEqual(move(.step(rows: 0, columns: 1), from: cursor(5, 2)), cursor(5, 3))
        XCTAssertEqual(move(.step(rows: 0, columns: -1), from: cursor(5, 2)), cursor(5, 1))
    }

    /// The edge holds the cursor; it does not wrap and does not leave.
    func testAStepIsClampedAtTheEdges() {
        XCTAssertEqual(move(.step(rows: -1, columns: 0), from: cursor(0, 2)), cursor(0, 2))
        XCTAssertEqual(move(.step(rows: 1, columns: 0), from: cursor(99, 2)), cursor(99, 2))
        XCTAssertEqual(move(.step(rows: 0, columns: -1), from: cursor(7, 0)), cursor(7, 0))
        XCTAssertEqual(move(.step(rows: 0, columns: 1), from: cursor(7, 4)), cursor(7, 4))
    }

    // MARK: Collapsing and extending

    func testAPlainMoveCollapsesTheBlockOntoTheNewCell() {
        let block = GridCursor(anchor: CellPos(row: 2, column: 1), focus: CellPos(row: 6, column: 3))
        XCTAssertEqual(move(.step(rows: 1, columns: 0), from: block), cursor(7, 3),
                       "from the focus, which is the end that moves")
    }

    func testExtendingKeepsTheAnchorAndMovesTheFocus() {
        let block = GridCursor(anchor: CellPos(row: 2, column: 1), focus: CellPos(row: 6, column: 3))
        let next = move(.step(rows: 1, columns: 0), from: block, extending: true)
        XCTAssertEqual(next?.anchor, CellPos(row: 2, column: 1))
        XCTAssertEqual(next?.focus, CellPos(row: 7, column: 3))

        // Back past the anchor: the block flips, which `CellRange` normalises.
        let back = move(.toEdge(.top), from: block, extending: true)
        XCTAssertEqual(back?.anchor, CellPos(row: 2, column: 1))
        XCTAssertEqual(back?.focus, CellPos(row: 0, column: 3))
        XCTAssertEqual(back?.range.top, 0)
        XCTAssertEqual(back?.range.bottom, 2)
    }

    // MARK: Edges, pages, rows, corners

    func testToEdgeMovesOneAxisAndLeavesTheOther() {
        XCTAssertEqual(move(.toEdge(.left), from: cursor(5, 3)), cursor(5, 0))
        XCTAssertEqual(move(.toEdge(.right), from: cursor(5, 1)), cursor(5, 4))
        XCTAssertEqual(move(.toEdge(.top), from: cursor(5, 3)), cursor(0, 3))
        XCTAssertEqual(move(.toEdge(.bottom), from: cursor(5, 3)), cursor(99, 3))
    }

    /// A page is the rows on screen less one, so the row that was last is first after the move.
    func testAPageIsTheVisibleRowsLessOne() {
        XCTAssertEqual(move(.page(down: true), from: cursor(10, 2)), cursor(29, 2))
        XCTAssertEqual(move(.page(down: false), from: cursor(30, 2)), cursor(11, 2))
        XCTAssertEqual(move(.page(down: true), from: cursor(90, 2)), cursor(99, 2), "clamped")
        XCTAssertEqual(move(.page(down: false), from: cursor(3, 2)), cursor(0, 2), "clamped")
    }

    func testAPageIsNeverLessThanOneRow() {
        let tiny = GridBounds(rows: 100, columns: 5, page: 1)
        XCTAssertEqual(move(.page(down: true), from: cursor(10, 2), in: tiny), cursor(11, 2))
        let none = GridBounds(rows: 100, columns: 5, page: 0)
        XCTAssertEqual(move(.page(down: false), from: cursor(10, 2), in: none), cursor(9, 2))
    }

    func testRowStartAndEndStayOnTheRow() {
        XCTAssertEqual(move(.rowStart, from: cursor(40, 3)), cursor(40, 0))
        XCTAssertEqual(move(.rowEnd, from: cursor(40, 1)), cursor(40, 4))
    }

    func testTheCornersAreTheFirstAndLastCells() {
        XCTAssertEqual(move(.gridStart, from: cursor(40, 3)), cursor(0, 0))
        XCTAssertEqual(move(.gridEnd, from: cursor(40, 1)), cursor(99, 4))
        let extended = move(.gridEnd, from: cursor(40, 1), extending: true)
        XCTAssertEqual(extended?.anchor, CellPos(row: 40, column: 1))
        XCTAssertEqual(extended?.focus, CellPos(row: 99, column: 4))
    }

    // MARK: Tab

    func testTabWalksTheRowAndWrapsOntoTheNext() {
        XCTAssertEqual(move(.next, from: cursor(5, 1)), cursor(5, 2))
        XCTAssertEqual(move(.next, from: cursor(5, 4)), cursor(6, 0), "past the last column: the next row")
        XCTAssertEqual(move(.previous, from: cursor(5, 3)), cursor(5, 2))
        XCTAssertEqual(move(.previous, from: cursor(5, 0)), cursor(4, 4), "before the first: the row above")
    }

    /// Tab off either end is how the grid hands the keyboard on: `nil`, not a clamp.
    func testTabPastEitherEndLeavesTheGrid() {
        XCTAssertNil(move(.next, from: cursor(99, 4)))
        XCTAssertNil(move(.previous, from: cursor(0, 0)))
    }

    func testTabCollapsesABlockAndIgnoresExtending() {
        let block = GridCursor(anchor: CellPos(row: 2, column: 1), focus: CellPos(row: 6, column: 3))
        XCTAssertEqual(move(.next, from: block, extending: true), cursor(6, 4))
    }

    // MARK: Odd bounds

    func testAnEmptyGridHasNowhereToGo() {
        XCTAssertNil(move(.step(rows: 1, columns: 0), from: cursor(0, 0),
                          in: GridBounds(rows: 0, columns: 5, page: 20)))
        XCTAssertNil(move(.next, from: cursor(0, 0), in: GridBounds(rows: 10, columns: 0, page: 20)))
    }

    /// A cursor left over from a longer result is brought back inside before it moves.
    func testACursorOutsideTheBoundsIsClampedFirst() {
        let shrunk = GridBounds(rows: 10, columns: 3, page: 5)
        XCTAssertEqual(move(.step(rows: 0, columns: 0), from: cursor(50, 8), in: shrunk), cursor(9, 2))
        XCTAssertEqual(move(.step(rows: -1, columns: 0), from: cursor(50, 8), in: shrunk), cursor(8, 2))
    }

    /// W10-T3 adds rows the user is inserting below the result; they are rows of the table, so the
    /// bounds count them and the cursor reaches the last of them.
    func testBoundsThatCountInsertedRowsAreReachable() {
        let withInserted = GridBounds(rows: 12, columns: 5, page: 20)  // 10 fetched and 2 inserted
        XCTAssertEqual(move(.toEdge(.bottom), from: cursor(3, 1), in: withInserted), cursor(11, 1))
        XCTAssertEqual(move(.gridEnd, from: cursor(3, 1), in: withInserted), cursor(11, 4))
        XCTAssertNil(move(.next, from: cursor(11, 4), in: withInserted))
    }

    // MARK: Scrolling to a cell

    private let clip = CGRect(x: 0, y: 100, width: 400, height: 300)  // shows y 100…400, x 0…400
    private let noInsets = NSEdgeInsets()

    func testAVisibleCellDoesNotScroll() {
        let cell = CGRect(x: 50, y: 200, width: 100, height: 25)
        XCTAssertEqual(GridScrollMath.origin(revealing: cell, in: clip, insets: noInsets), clip.origin)
    }

    func testACellBelowScrollsJustFarEnoughToShowItsBottom() {
        let cell = CGRect(x: 50, y: 425, width: 100, height: 25)
        XCTAssertEqual(GridScrollMath.origin(revealing: cell, in: clip, insets: noInsets),
                       CGPoint(x: 0, y: 150))
    }

    func testACellAboveScrollsToItsTop() {
        let cell = CGRect(x: 50, y: 60, width: 100, height: 25)
        XCTAssertEqual(GridScrollMath.origin(revealing: cell, in: clip, insets: noInsets),
                       CGPoint(x: 0, y: 60))
    }

    func testACellToTheRightAndToTheLeftScrollHorizontally() {
        let right = CGRect(x: 450, y: 200, width: 100, height: 25)
        XCTAssertEqual(GridScrollMath.origin(revealing: right, in: clip, insets: noInsets).x, 150)
        let scrolled = CGRect(x: 200, y: 100, width: 400, height: 300)
        let left = CGRect(x: 120, y: 200, width: 60, height: 25)
        XCTAssertEqual(GridScrollMath.origin(revealing: left, in: scrolled, insets: noInsets).x, 120)
    }

    /// The header sits over the top of the clip view as an inset: a row under it is not on screen.
    func testTheTopInsetIsNotVisibleRoom() {
        let insets = NSEdgeInsets(top: 40, left: 0, bottom: 0, right: 0)
        let cell = CGRect(x: 50, y: 120, width: 100, height: 25)  // y 100…140 is under the header
        XCTAssertEqual(GridScrollMath.origin(revealing: cell, in: clip, insets: insets).y, 80)
    }

    /// A cell wider than the view shows its left edge, which is the part a person reads.
    func testACellWiderThanTheViewKeepsItsLeftEdge() {
        let wide = CGRect(x: 100, y: 200, width: 900, height: 25)
        XCTAssertEqual(GridScrollMath.origin(revealing: wide, in: clip, insets: noInsets).x, 100)
    }
}
