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
