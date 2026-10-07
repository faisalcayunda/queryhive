import AppKit
import XCTest

@testable import QueryHive

/// The accessibility tree (blueprint §11.2, FR-GRID-07).
///
/// The two properties every case here defends are that **nothing is built until a client asks** and
/// **nothing is kept that would go stale**: an element is created by an `accessibility…()` method,
/// and its label, its value and its frame are computed at the moment they are read, so a scroll or
/// a new result cannot leave a client holding a description of a cell that is no longer there.
@MainActor
final class GridAccessibilityTests: XCTestCase {

    /// `announceSelectionForAX` posts through `NSApp`, which a test process does not otherwise
    /// have. Made once, here, rather than in every suite that never posts.
    override func setUp() {
        super.setUp()
        GridFixture.ensureApplication()
    }

    private func makeGrid(rowCount: Int = 5_000) -> GridFixture {
        let columns = [Event.Column(name: "kode", type: "text"),
                       Event.Column(name: "nama", type: "text"),
                       Event.Column(name: "jumlah", type: "bigint"),
                       Event.Column(name: "catatan", type: "text")]
        let rows = (0..<rowCount).map { row in
            ["K-\(row)", "Wilayah \(row)", "\(row)", row.isMultiple(of: 17) ? nil : "catatan \(row)"]
        }
        return GridFixture(columns: columns, rows: rows)
    }

    // MARK: Nothing is built before it is asked for

    /// D-10: with no client watching, AppKit never calls an `accessibility…()` method, so the grid
    /// has no elements, no observer and no notifications to post. The first
    /// `accessibilityChildren` is what arms it, and it arms it exactly once.
    func testNothingIsArmedBeforeAccessibilityChildrenIsAsked() {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply()

        XCTAssertFalse(fixture.coordinator.axClientAttached)
        XCTAssertNil(fixture.coordinator.lastAXAnnouncement)

        let children = fixture.table.accessibilityChildren()
        XCTAssertTrue(fixture.coordinator.axClientAttached)
        XCTAssertFalse(children?.isEmpty ?? true)

        // Idempotent: a second client asking must not re-arm or rebuild.
        let first = fixture.table.accessibilityChildren()
        let second = fixture.table.accessibilityChildren()
        XCTAssertEqual(first?.count, second?.count)
    }

    /// A million-row result must not hand VoiceOver a million rows: the count is the whole result,
    /// the elements are only the rows and columns on screen (§11.2).
    func testOnlyTheRowsAndColumnsOnScreenGetElements() throws {
        let fixture = makeGrid(rowCount: 5_000)
        fixture.apply()
        _ = fixture.table.accessibilityChildren()

        XCTAssertEqual(fixture.coordinator.rowCountForAX, 5_000)
        XCTAssertEqual(fixture.table.accessibilityRowCount(), 5_000)
        XCTAssertEqual(fixture.table.accessibilityColumnCount(), 4)

        let visibleRows = try XCTUnwrap(fixture.coordinator.ax.visibleRows)
        XCTAssertFalse(visibleRows.isEmpty, "the panel is laid out, so something is on screen")
        XCTAssertLessThan(visibleRows.count, 100, "only the viewport's rows get an element")
        XCTAssertEqual(visibleRows.map(\.row), Array(visibleRows.map(\.row).sorted()),
                       "in result order, so reading down the screen reads down the result")

        let columns = try XCTUnwrap(fixture.coordinator.ax.columns)
        XCTAssertEqual(columns.count, 4)
        XCTAssertEqual(columns.map(\.sourceColumn), fixture.coordinator.visibleSources)

        XCTAssertEqual(fixture.table.accessibilityVisibleRows()?.count, visibleRows.count)
        XCTAssertEqual(fixture.table.accessibilityColumns()?.count, 4)

        // A cell off screen is still reachable — VoiceOver navigates to it and scrolls — but it is
        // not in `visibleCells`, which is what the table reports as on screen.
        let last = try XCTUnwrap(fixture.coordinator.ax.cell(row: 4_999, column: 0))
        XCTAssertEqual(last.key.row, 4_999)
        XCTAssertLessThan(try XCTUnwrap(fixture.table.accessibilityVisibleCells()).count, 500)
    }

    // MARK: What a cell says

    /// FR-GRID-07's string: "Row r, column c, \<header\>: \<value\>", one-based because it is read
    /// aloud, plus ", changed" for the one thing about a cell its value cannot say.
    func testTheCellLabelNamesTheRowTheColumnAndTheValue() throws {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply()

        let cell = try XCTUnwrap(fixture.coordinator.ax.cell(row: 0, column: 1))
        XCTAssertEqual(cell.accessibilityLabel(),
                       "Row 1, column 2, nama: Wilayah 0")

        // A NULL is read as the word, not as nothing at all.
        let null = try XCTUnwrap(fixture.coordinator.ax.cell(row: 17, column: 3))
        XCTAssertEqual(null.accessibilityLabel(),
                       "Row 18, column 4, catatan: null")

        // A staged edit is what a screen reader has to hear about a cell the table has not written.
        fixture.tab.beginCellEdit(at: CellKey(row: 0, column: 1))
        fixture.tab.typeCellEdit("Wilayah 0 — edited")
        fixture.tab.endCellEdit()
        let staged = try XCTUnwrap(fixture.coordinator.ax.cell(row: 0, column: 1))
        XCTAssertEqual(staged.accessibilityLabel(),
                       "Row 1, column 2, nama: Wilayah 0 — edited, changed")
    }

    // MARK: The header

    /// The chevron and `accessibilitySortDirection` come from the same indicator, so what is read
    /// out and what is drawn cannot disagree (§11.2, item 21).
    func testTheHeaderSortDirectionFollowsTheIndicator() throws {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply(fixture.inputs(sort: SortIndicator(column: 1, direction: .ascending,
                                                         origin: .server)))

        let headers = try XCTUnwrap(fixture.table.accessibilityColumnHeaderUIElements())
        XCTAssertEqual(headers.count, 4)
        let sorted = try XCTUnwrap(headers[1] as? GridAXHeader)
        XCTAssertEqual(sorted.accessibilitySortDirection(), .ascending)
        XCTAssertEqual(sorted.accessibilityLabel(), "nama, text, sorted ascending")
        XCTAssertEqual(sorted.accessibilityValue() as? String, "text")

        let unsorted = try XCTUnwrap(headers[0] as? GridAXHeader)
        XCTAssertEqual(unsorted.accessibilitySortDirection(), .unknown)
        XCTAssertEqual(unsorted.accessibilityLabel(), "kode, text")

        // Pressing the header button runs the same cycle a click does — server-first.
        XCTAssertTrue(sorted.accessibilityPerformPress())
        XCTAssertEqual(fixture.log.sortClick, [1])

        fixture.apply(fixture.inputs(sort: SortIndicator(column: 1, direction: .descending,
                                                         origin: .server)))
        let refreshed = try XCTUnwrap(fixture.table.accessibilityColumnHeaderUIElements())
        let descending = try XCTUnwrap(refreshed[1] as? GridAXHeader)
        XCTAssertEqual(descending.accessibilitySortDirection(), .descending)
        XCTAssertEqual(descending.accessibilityLabel(), "nama, text, sorted descending")
    }

    /// The funnel is a 10-point glyph; "Filter" is the same thing as a named action (FR-GRID-07).
    func testTheHeaderOffersAFilterAction() throws {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply()
        let headers = try XCTUnwrap(fixture.table.accessibilityColumnHeaderUIElements())
        let header = try XCTUnwrap(headers[2] as? GridAXHeader)
        let actions = try XCTUnwrap(header.accessibilityCustomActions())
        XCTAssertEqual(actions.map(\.name), ["Filter"])

        XCTAssertEqual(actions.first?.handler?(), true)
        XCTAssertEqual(fixture.log.openFilter.map(\.0), [2], "the column's source index, as the funnel passes it")
        XCTAssertEqual(fixture.log.openFilter.first?.1, fixture.coordinator.frame(ofHeader: 2))
    }

    /// The column element is a `.column` that groups, not a second description of the header.
    func testTheColumnElementIsAGrouperAndNotADuplicateOfTheHeader() throws {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply(fixture.inputs(sort: SortIndicator(column: 0, direction: .ascending,
                                                         origin: .server)))
        let column = try XCTUnwrap(fixture.coordinator.ax.columns.first)
        XCTAssertEqual(column.accessibilityLabel(), "kode")
        XCTAssertEqual(column.accessibilityValue() as? String, "text")
        XCTAssertEqual(column.accessibilityColumnIndexRange().location, 0)
        // The sort state belongs to the header button: the column does not carry a second one.
        XCTAssertEqual(column.accessibilitySortDirection(), .unknown)
    }

    // MARK: The announcement

    /// "3 × 2 cells selected", posted **on release** and never per drag step: announcing every
    /// mouse-moved pixel would make VoiceOver talk over itself (FR-GRID-07).
    func testTheSelectionIsAnnouncedOnReleaseAndNotDuringTheDrag() {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply()
        _ = fixture.table.accessibilityChildren()

        let anchor = fixture.pointInCell(row: 1, column: 1)
        let focus = fixture.pointInCell(row: 3, column: 2)
        fixture.coordinator.press(at: anchor, clickCount: 1)
        XCTAssertNil(fixture.coordinator.lastAXAnnouncement, "a press does not talk")

        fixture.coordinator.drag(to: focus)
        XCTAssertNil(fixture.coordinator.lastAXAnnouncement, "neither does a step of the drag")

        fixture.coordinator.release()
        XCTAssertEqual(fixture.coordinator.lastAXAnnouncement, "3 × 2 cells selected")
        XCTAssertEqual(fixture.log.settleSelection, 1)
    }

    /// Before a client attaches, the same release posts nothing at all — there is nobody to post to
    /// (D-10).
    func testNothingIsAnnouncedWhileNoClientIsWatching() {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply()
        XCTAssertFalse(fixture.coordinator.axClientAttached)

        fixture.coordinator.press(at: fixture.pointInCell(row: 0, column: 0), clickCount: 1)
        fixture.coordinator.release()
        XCTAssertNil(fixture.coordinator.lastAXAnnouncement)
    }

    /// A release with no selection has nothing to announce, not "0 × 0 cells selected".
    func testReleaseWithoutASelectionAnnouncesNothing() {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply()
        _ = fixture.table.accessibilityChildren()
        XCTAssertTrue(fixture.coordinator.axClientAttached)
        XCTAssertNil(fixture.tab.cellSelection)

        fixture.coordinator.release()
        XCTAssertNil(fixture.coordinator.lastAXAnnouncement)
    }

    // MARK: Focus

    /// VoiceOver's focus is the **cursor**, and reading it must not move the selection: a client
    /// that asks where focus is would otherwise select the cell it asked about.
    ///
    /// The cursor's own model is D-11 (drawn in W10-T1); what is asserted here is that the element
    /// handed to a focus question is that cell and that the answer changes nothing.
    func testTheFocusedElementIsTheCursorsCellAndReadingItChangesNothing() throws {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply()

        fixture.coordinator.press(at: fixture.pointInCell(row: 2, column: 1), clickCount: 1)
        let selection = fixture.tab.cellSelection
        let cursor = try XCTUnwrap(fixture.tab.cellCursor)
        XCTAssertNotNil(selection)

        let focused = try XCTUnwrap(fixture.table.accessibilityFocusedUIElement as? GridAXCell)
        XCTAssertEqual(focused.key.row, cursor.focus.row)
        XCTAssertEqual(focused.display, cursor.focus.column)

        XCTAssertEqual(fixture.tab.cellSelection, selection, "reading focus moved the selection")
    }

    /// FR-GRID-07: a NULL is read as the word, whatever the grid is set to draw it as. A grid that
    /// draws NULL as nothing must not make VoiceOver read a cell with nothing in it.
    func testANullIsReadAsNullEvenWhenTheGridDrawsItAsNothing() throws {
        var style = GridInputs.GridStyle.placeholder
        style.nullDisplay = ""
        let columns = [Event.Column(name: "kode", type: "text"), Event.Column(name: "catatan", type: "text")]
        let fixture = GridFixture(columns: columns, rows: [["K-0", nil], ["K-1", "ada"]], style: style)
        fixture.apply()

        let null = try XCTUnwrap(fixture.coordinator.ax.cell(row: 0, column: 1))
        XCTAssertEqual(null.accessibilityLabel(), "Row 1, column 2, catatan: null")
        XCTAssertEqual(null.accessibilityValue() as? String, "null")
        let value = try XCTUnwrap(fixture.coordinator.ax.cell(row: 1, column: 1))
        XCTAssertEqual(value.accessibilityLabel(), "Row 2, column 2, catatan: ada")
    }

    /// What a cell's value cannot say, the label adds: a staged edit, and a row queued for deletion.
    func testTheLabelNamesAChangedCellAndARowMarkedForDeletion() throws {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply()
        fixture.tab.cellEdits.deleteRow(3)

        let marked = try XCTUnwrap(fixture.coordinator.ax.cell(row: 3, column: 0))
        XCTAssertEqual(marked.accessibilityLabel(), "Row 4, column 1, kode: K-3, marked for deletion")
        let other = try XCTUnwrap(fixture.coordinator.ax.cell(row: 4, column: 0))
        XCTAssertEqual(other.accessibilityLabel(), "Row 5, column 1, kode: K-4")

        // A row marked for deletion takes no edit (it would plan an UPDATE after its own DELETE), so
        // "changed" is a different row's.
        fixture.tab.beginCellEdit(at: CellKey(row: 5, column: 0))
        fixture.tab.typeCellEdit("K-5 edited")
        fixture.tab.endCellEdit()
        XCTAssertEqual(try XCTUnwrap(fixture.coordinator.ax.cell(row: 5, column: 0)).accessibilityLabel(),
                       "Row 6, column 1, kode: K-5 edited, changed")
        fixture.tab.beginCellEdit(at: CellKey(row: 3, column: 0))
        fixture.tab.typeCellEdit("K-3 edited")
        fixture.tab.endCellEdit()
        XCTAssertEqual(try XCTUnwrap(fixture.coordinator.ax.cell(row: 3, column: 0)).accessibilityLabel(),
                       "Row 4, column 1, kode: K-3, marked for deletion")
    }

    // MARK: The focus setter (D-11, P-24)

    /// VoiceOver puts its focus on a cell: the cursor goes there, the block stays, and nothing is
    /// announced. Reading down a column must not select every cell it passes.
    func testTheFocusSetterMovesTheCursorAndLeavesTheSelectionAlone() throws {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply()
        _ = fixture.table.accessibilityChildren()
        fixture.coordinator.press(at: fixture.pointInCell(row: 2, column: 1), clickCount: 1)
        fixture.coordinator.drag(to: fixture.pointInCell(row: 4, column: 2))
        fixture.coordinator.release()
        let selection = fixture.tab.cellSelection
        let announced = fixture.coordinator.lastAXAnnouncement

        let cell = try XCTUnwrap(fixture.coordinator.ax.cell(row: 20, column: 3))
        XCTAssertFalse(cell.isAccessibilityFocused())
        cell.setAccessibilityFocused(true)

        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: 20, column: 3))
        XCTAssertEqual(fixture.tab.cellCursor?.anchor, CellPos(row: 20, column: 3))
        XCTAssertEqual(fixture.tab.cellSelection, selection, "the block did not move")
        XCTAssertEqual(fixture.coordinator.lastAXAnnouncement, announced, "and nothing was announced")
        XCTAssertTrue(cell.isAccessibilityFocused())
        XCTAssertTrue((fixture.table.accessibilityFocusedUIElement as? GridAXCell) === cell)
        XCTAssertEqual(fixture.table.cursor, fixture.tab.cellCursor, "the ring follows it")

        // Taking focus away is not a move; the cell that gets it is what writes.
        cell.setAccessibilityFocused(false)
        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: 20, column: 3))
    }

    /// The cell is scrolled into view: VoiceOver's focus is on a cell a person can see.
    func testTheFocusSetterScrollsTheCellIntoView() throws {
        let fixture = makeGrid(rowCount: 5_000)
        fixture.apply()
        _ = fixture.table.accessibilityChildren()
        let cell = try XCTUnwrap(fixture.coordinator.ax.cell(row: 3_000, column: 0))
        cell.setAccessibilityFocused(true)
        XCTAssertTrue(NSLocationInRange(3_000, fixture.table.rows(in: fixture.table.visibleRect)))
    }

    func testTheFocusSetterUsesTheDisplayColumnOfAMovedGrid() throws {
        let fixture = makeGrid(rowCount: 40)
        // Source 2 is drawn first, source 0 second, and the other two are hidden.
        fixture.apply(fixture.inputs(visibleSources: [2, 0]))
        _ = fixture.table.accessibilityChildren()
        let cell = try XCTUnwrap(fixture.coordinator.ax.cell(row: 5, column: 1))  // source 0
        cell.setAccessibilityFocused(true)
        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: 5, column: 1))
        XCTAssertTrue(cell.isAccessibilityFocused())
    }

    // MARK: Keyboard moves and VoiceOver

    /// A move of the cursor tells VoiceOver where its focus went, once a client is attached; with
    /// none attached it posts nothing (D-10).
    func testAKeyboardMoveTellsVoiceOverOnlyWhenAClientIsAttached() throws {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply()
        fixture.coordinator.press(at: fixture.pointInCell(row: 2, column: 1), clickCount: 1)
        fixture.coordinator.release()

        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.down)))
        XCTAssertNil(fixture.coordinator.lastAXFocusPost, "nobody is listening")

        _ = fixture.table.accessibilityChildren()
        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.down)))
        XCTAssertEqual(fixture.coordinator.lastAXFocusPost, CellKey(row: 4, column: 1))
    }

    /// Holding Shift-Down reads one total, 250 ms after the last key, and not one per row.
    func testAnExtensionIsAnnouncedOnceAfterTheLastKey() throws {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply()
        fixture.coordinator.press(at: fixture.pointInCell(row: 2, column: 1), clickCount: 1)
        fixture.coordinator.release()
        _ = fixture.table.accessibilityChildren()
        XCTAssertNil(fixture.coordinator.lastAXAnnouncement)

        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.down, shift: true)))
        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.down, shift: true)))
        XCTAssertNil(fixture.coordinator.lastAXAnnouncement, "not per key press")

        GridFixture.drain(until: { fixture.coordinator.lastAXAnnouncement != nil })
        XCTAssertEqual(fixture.coordinator.lastAXAnnouncement, "3 × 1 cells selected")
    }

    /// A plain move is read by VoiceOver as the cell it lands on, so no total is announced.
    func testAPlainMoveAnnouncesNoTotal() throws {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply()
        fixture.coordinator.press(at: fixture.pointInCell(row: 2, column: 1), clickCount: 1)
        fixture.coordinator.release()
        _ = fixture.table.accessibilityChildren()

        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.down)))
        GridFixture.pause(for: 0.4)
        XCTAssertNil(fixture.coordinator.lastAXAnnouncement)
    }

    // MARK: A new result

    /// The elements are thrown away when the result changes: their indices name rows that no longer
    /// exist, and a client that held one would read yesterday's value off today's row.
    func testTheElementsAreDiscardedWhenAResultReplacesTheRows() throws {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply()
        _ = fixture.table.accessibilityChildren()

        let before = try XCTUnwrap(fixture.coordinator.ax.cell(row: 0, column: 0))
        XCTAssertEqual(before.accessibilityLabel(), "Row 1, column 1, kode: K-0")

        fixture.apply(fixture.inputs(revision: 2))
        XCTAssertNotEqual(fixture.coordinator.currentRevision, 1)

        let after = try XCTUnwrap(fixture.coordinator.ax.cell(row: 0, column: 0))
        XCTAssertTrue(before !== after, "the old element must not survive its own row")
        XCTAssertEqual(after.accessibilityLabel(), "Row 1, column 1, kode: K-0")
    }

    /// `rowCountForAX` is the whole result, not the viewport: the count has to survive a scroll and
    /// a pageful of rows arriving.
    func testTheRowCountIsTheResultAndNotTheViewport() {
        let fixture = makeGrid(rowCount: 5_000)
        fixture.apply()
        _ = fixture.table.accessibilityChildren()
        XCTAssertEqual(fixture.coordinator.rowCountForAX, 5_000)
        XCTAssertLessThan(fixture.coordinator.ax.visibleRows.count, 5_000)
    }
}
