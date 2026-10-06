import AppKit
import XCTest

@testable import QueryHive

/// W10-T9 part 1: column widths by hand, Fit, shift-click and the edge autoscroll (DBX-63, DBX-64,
/// PF-11). The pure parts are checked on their own; the rest drives the coordinator and the header
/// the way `GridParityTests` does.
@MainActor
final class GridColumnResizeTests: XCTestCase {

    private let columns = [Event.Column(name: "kode", type: "text"),
                           Event.Column(name: "jumlah", type: "bigint"),
                           Event.Column(name: "catatan", type: "text"),
                           Event.Column(name: "tanggal", type: "date")]

    override func setUp() {
        super.setUp()
        TestStores.ensureConfigured()
    }

    private func makeGrid(rowCount: Int = 60) -> GridFixture {
        let rows = (0..<rowCount).map { row -> [String?] in
            ["K-\(row)", "\(row)", row == 3 ? String(repeating: "m", count: 90) : "ok", "2026-10-06"]
        }
        return GridFixture(columns: columns, rows: rows)
    }

    private func headerPress(_ fixture: GridFixture, x: CGFloat, clickCount: Int = 1) throws {
        let header = fixture.table.header
        let event = try XCTUnwrap(NSEvent.mouseEvent(
            with: .leftMouseDown, location: header.convert(CGPoint(x: x, y: header.bounds.midY), to: nil),
            modifierFlags: [], timestamp: 0, windowNumber: fixture.window.windowNumber, context: nil,
            eventNumber: 0, clickCount: clickCount, pressure: 1))
        header.mouseDown(with: event)
    }

    private func headerDrag(_ fixture: GridFixture, to x: CGFloat) throws {
        let header = fixture.table.header
        let event = try XCTUnwrap(NSEvent.mouseEvent(
            with: .leftMouseDragged, location: header.convert(CGPoint(x: x, y: header.bounds.midY), to: nil),
            modifierFlags: [], timestamp: 0, windowNumber: fixture.window.windowNumber, context: nil,
            eventNumber: 0, clickCount: 1, pressure: 1))
        header.mouseDragged(with: event)
    }

    // MARK: Pure

    /// With nothing set by hand the widths are the old ones, bit for bit: that is what keeps every
    /// baseline where it was.
    func testNoOverridesIsTheOldFittedPath() {
        let natural: [CGFloat] = [120, 140, 200]
        let old = GridMetrics.fitted(natural, available: 900, gutter: 60).map { $0 + 16 }
        XCTAssertEqual(GridMetrics.drawnWidths(natural: natural, overrides: [nil, nil, nil],
                                               available: 900, gutter: 60), old)
    }

    func testAPinnedColumnKeepsItsWidthAndTheOthersShareWhatIsLeft() {
        let widths = GridMetrics.drawnWidths(natural: [120, 140, 200], overrides: [nil, 300, nil],
                                             available: 1_000, gutter: 60)
        XCTAssertEqual(widths[1], 300, "the pinned column is exactly what was set")
        let free = GridMetrics.fitted([120, 200], available: 700, gutter: 60).map { $0 + 16 }
        XCTAssertEqual([widths[0], widths[2]], free)
    }

    func testAHandSetWidthStaysInsideTheFloorAndTheCeiling() {
        XCTAssertEqual(GridMetrics.clampedColumnWidth(3), GridMetrics.minColumnWidth)
        XCTAssertEqual(GridMetrics.clampedColumnWidth(99_999), GridMetrics.maxColumnWidth)
        XCTAssertEqual(GridMetrics.drawnWidths(natural: [100], overrides: [1], available: 500, gutter: 0),
                       [GridMetrics.minColumnWidth])
    }

    /// A pinned width may be past the ceiling, and the formula then has to draw it as stored and
    /// take it out of the panel at that size.
    func testAPinnedWidthPastTheCeilingIsDrawnAsStoredAndCountedAsStored() {
        let widths = GridMetrics.drawnWidths(natural: [120, 140], overrides: [1_500, nil],
                                             available: 2_000, gutter: 60)
        XCTAssertEqual(widths[0], 1_500)
        XCTAssertEqual(widths[1], GridMetrics.fitted([140], available: 500, gutter: 60)[0] + 16)
    }

    /// The grip is a 5 pt strip over each column's right edge: two points inside it, three outside.
    func testTheGripSitsOverTheSeparator() {
        let geometry = GridColumnGeometry(gutter: 60, widths: [100, 100])
        XCTAssertEqual(geometry.resizeColumn(atX: 160), 0)
        XCTAssertEqual(geometry.resizeColumn(atX: 158), 0)
        XCTAssertEqual(geometry.resizeColumn(atX: 162.9), 0)
        XCTAssertNil(geometry.resizeColumn(atX: 157.9))
        XCTAssertNil(geometry.resizeColumn(atX: 163))
        XCTAssertEqual(geometry.resizeColumn(atX: 260), 1)
        XCTAssertNil(geometry.resizeColumn(atX: 59), "the gutter has no grip")
        XCTAssertNil(geometry.resizeColumn(atX: 300))
    }

    func testTheFunnelStripIsStillTwentyWide() {
        XCTAssertEqual(GridHeaderView.funnelHitArea(columnRight: 200, height: 50),
                       CGRect(x: 180, y: 0, width: 20, height: 50))
    }

    func testAutoscrollSpeedGrowsWithTheOvershootAndIsCapped() {
        let visible = CGRect(x: 0, y: 0, width: 400, height: 300)
        XCTAssertEqual(GridAutoscroll.delta(pointer: CGPoint(x: 100, y: 100), visible: visible), .zero)
        let near = GridAutoscroll.delta(pointer: CGPoint(x: 100, y: 304), visible: visible)
        let far = GridAutoscroll.delta(pointer: CGPoint(x: 100, y: 340), visible: visible)
        XCTAssertEqual(near.width, 0)
        XCTAssertGreaterThan(near.height, 0)
        XCTAssertGreaterThan(far.height, near.height)
        XCTAssertEqual(GridAutoscroll.delta(pointer: CGPoint(x: 100, y: 99_999), visible: visible).height, 30)
        XCTAssertLessThan(GridAutoscroll.delta(pointer: CGPoint(x: -20, y: 100), visible: visible).width, 0)
        XCTAssertLessThan(GridAutoscroll.delta(pointer: CGPoint(x: 100, y: -20), visible: visible).height, 0)
    }

    // MARK: Resize and Fit

    /// Dragging a grip sets that column's width to where the pointer is, and leaves every other
    /// column exactly as wide as it was drawn.
    func testDraggingAGripResizesOneColumnAndPinsTheRest() throws {
        let fixture = makeGrid()
        fixture.apply()
        let before = fixture.coordinator.geometry.widths
        let edge = fixture.coordinator.geometry.edges(of: 1).right

        try headerPress(fixture, x: edge)
        XCTAssertTrue(fixture.log.sortClick.isEmpty, "a press on a grip never sorts")
        XCTAssertTrue(fixture.tab.columnWidthOverrides.isEmpty, "a click that goes nowhere pins nothing")
        try headerDrag(fixture, to: edge + 40)

        let after = fixture.coordinator.geometry.widths
        XCTAssertEqual(after[1], before[1] + 40, accuracy: 0.01)
        for index in [0, 2, 3] { XCTAssertEqual(after[index], before[index], accuracy: 0.01, "column \(index)") }
        XCTAssertEqual(fixture.table.header.geometry.widths, after, "the header reads the same widths")
        XCTAssertEqual(fixture.table.tableColumns.first?.width ?? 0, fixture.coordinator.geometry.totalWidth,
                       accuracy: 0.01, "the document is as wide as the columns")

        try headerDrag(fixture, to: edge - 10_000)
        XCTAssertEqual(fixture.coordinator.geometry.widths[1], GridMetrics.minColumnWidth,
                       "a column cannot be dragged away")
    }

    /// A lone column on a wide panel is drawn wider than 1200 pt; its grip must start from that width
    /// and move by the pointer's step, not jump to the ceiling.
    func testAColumnDrawnWiderThanTheCeilingResizesByTheStepAndKeepsItsNeighbours() {
        let wide = GridFixture(columns: [Event.Column(name: "count", type: "bigint")], rows: [["1"]],
                               viewport: CGSize(width: 1_440, height: 600))
        wide.apply()
        let drawn = wide.coordinator.geometry.widths[0]
        XCTAssertGreaterThan(drawn, GridMetrics.maxColumnWidth, "the fixture must draw past the ceiling")
        wide.coordinator.setColumnWidth(display: 0, to: drawn + 1)
        XCTAssertEqual(wide.coordinator.geometry.widths[0], drawn + 1, accuracy: 0.01)
        wide.coordinator.setColumnWidth(display: 0, to: drawn - 1)
        XCTAssertEqual(wide.coordinator.geometry.widths[0], drawn - 1, accuracy: 0.01)

        let fixture = makeGrid()
        fixture.apply()
        fixture.tab.columnWidthOverrides = [0: 1_500, 1: 300]
        fixture.coordinator.setColumnWidth(display: 1, to: 310)
        XCTAssertEqual(fixture.tab.columnWidthOverrides[0], 1_500, "a wide pinned column is not clamped")
        XCTAssertEqual(fixture.tab.columnWidthOverrides[1], 310)
    }

    func testADoubleClickOnAGripFitsTheColumnToItsContent() throws {
        let fixture = makeGrid()
        fixture.apply()
        // `catatan` holds a 90-character value in one row, wider than its formula width.
        let edge = fixture.coordinator.geometry.edges(of: 2).right
        let before = fixture.coordinator.geometry.widths[2]
        try headerPress(fixture, x: edge, clickCount: 2)
        let after = fixture.coordinator.geometry.widths[2]
        XCTAssertGreaterThan(after, before, "the long value is shown whole")
        XCTAssertEqual(after, GridMetrics.fitWidth(headerNeed: 0, cellNeed: after - 16), accuracy: 1.0)
        XCTAssertTrue(fixture.log.sortClick.isEmpty)

        // `kode` is short and the fit makes it narrower than the formula's 84 pt floor allowed.
        fixture.coordinator.fitColumn(display: 0)
        XCTAssertLessThan(fixture.coordinator.geometry.widths[0], before)
    }

    func testFitAllSizesEveryColumnAndResetGivesTheFormulaBack() {
        let fixture = makeGrid()
        fixture.apply()
        let original = fixture.coordinator.geometry.widths
        fixture.coordinator.fitAllColumns()
        XCTAssertEqual(fixture.tab.columnWidthOverrides.count, 4)
        XCTAssertNotEqual(fixture.coordinator.geometry.widths, original)

        fixture.coordinator.resetColumnWidths()
        XCTAssertTrue(fixture.tab.columnWidthOverrides.isEmpty)
        XCTAssertEqual(fixture.coordinator.geometry.widths, original)
    }

    /// The widths are the tab's, so a grid that goes away with its panel and comes back (a new
    /// coordinator) draws them again.
    func testTheWidthsOutliveTheGridThatSetThem() {
        let fixture = makeGrid()
        fixture.apply()
        fixture.coordinator.setColumnWidth(display: 0, to: 200)
        let again = ResultGridTable.Coordinator(tab: fixture.tab, model: fixture.model,
                                                commands: fixture.commands)
        let table = GridTableView()
        table.coordinator = again
        again.attach(table: table)
        again.apply(fixture.inputs(), force: true)
        XCTAssertEqual(again.geometry.widths[0], 200, accuracy: 0.01)
    }

    func testTheMenusOfferFitAllAndResetOnlyWhenTheyCanAct() {
        let fixture = makeGrid()
        fixture.apply()
        let body = fixture.coordinator.bodyMenu()
        let reset = body.items.first { $0.title == "Reset Column Widths" }
        XCTAssertNotNil(body.items.first { $0.title == "Fit All Columns" })
        XCTAssertEqual(reset?.isEnabled, false)
        fixture.coordinator.fitAllColumns()
        XCTAssertEqual(fixture.coordinator.bodyMenu().items.first { $0.title == "Reset Column Widths" }?.isEnabled, true)
        XCTAssertEqual(fixture.coordinator.widthsMenu().items.map(\.title), ["Fit All Columns", "Reset Column Widths"])
    }

    // MARK: PF-11

    /// Two different four-column results are not the same columns: the layout and the widths that
    /// described the first must not carry onto the second, but a repaint of the same result keeps them.
    func testAResultWithOtherNamesOrTypesDoesNotInheritTheLayout() {
        let tab = QueryTab(title: "T")
        tab.showRows(columns: columns, rows: [["a", "1", "x", "d"]], truncated: false, queryID: nil, elapsedMS: 0)
        tab.setColumnHidden(1, true)
        tab.columnWidthOverrides = [0: 200]

        tab.showRows(columns: columns, rows: [["a", "2", "y", "d"]], truncated: false, queryID: nil, elapsedMS: 0)
        XCTAssertEqual(tab.visibleColumnSources, [0, 2, 3], "the same columns keep their layout")
        XCTAssertEqual(tab.columnWidthOverrides, [0: 200])

        var renamed = columns
        renamed[2] = Event.Column(name: "keterangan", type: "text")
        tab.showRows(columns: renamed, rows: [["a", "2", "y", "d"]], truncated: false, queryID: nil, elapsedMS: 0)
        XCTAssertEqual(tab.visibleColumnSources, [0, 1, 2, 3], "another name is another column set")
        XCTAssertTrue(tab.columnWidthOverrides.isEmpty)

        tab.setColumnHidden(0, true)
        tab.columnWidthOverrides = [1: 90]
        var retyped = renamed
        retyped[1] = Event.Column(name: "jumlah", type: "text")
        tab.showRows(columns: retyped, rows: [["a", "2", "y", "d"]], truncated: false, queryID: nil, elapsedMS: 0)
        XCTAssertEqual(tab.visibleColumnSources, [0, 1, 2, 3], "and so is another type")
        XCTAssertTrue(tab.columnWidthOverrides.isEmpty)
    }

    func testAReRunThroughNilKeepsTheWidthsOfTheSameColumns() {
        let tab = QueryTab(title: "T")
        tab.showRows(columns: columns, rows: [["a", "1", "x", "d"]], truncated: false, queryID: nil, elapsedMS: 0)
        tab.columnWidthOverrides = [2: 180]
        tab.preview = nil
        tab.showRows(columns: columns, rows: [["a", "1", "x", "d"]], truncated: false, queryID: nil, elapsedMS: 0)
        XCTAssertEqual(tab.columnWidthOverrides, [2: 180], "a sort re-run is the same columns")
    }

    func testResetColumnLayoutAlsoForgetsTheWidths() {
        let tab = QueryTab(title: "T")
        tab.showRows(columns: columns, rows: [["a", "1", "x", "d"]], truncated: false, queryID: nil, elapsedMS: 0)
        tab.columnWidthOverrides = [2: 180]
        tab.resetColumnLayout()
        XCTAssertTrue(tab.columnWidthOverrides.isEmpty)
    }

    // MARK: Shift-click

    func testShiftClickExtendsFromTheAnchorAndADragKeepsIt() throws {
        let fixture = makeGrid()
        fixture.apply()
        let coordinator = fixture.coordinator
        coordinator.press(at: fixture.pointInCell(row: 2, column: 1), clickCount: 1)
        coordinator.release()

        coordinator.press(at: fixture.pointInCell(row: 5, column: 3), clickCount: 1, extend: true)
        let block = try XCTUnwrap(fixture.tab.cellSelection)
        XCTAssertEqual([block.top, block.left, block.bottom, block.right], [2, 1, 5, 3])
        XCTAssertEqual(fixture.tab.cellCursor?.anchor, CellPos(row: 2, column: 1))
        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: 5, column: 3))

        coordinator.drag(to: fixture.pointInCell(row: 8, column: 2))
        let dragged = try XCTUnwrap(fixture.tab.cellSelection)
        XCTAssertEqual([dragged.top, dragged.left, dragged.bottom, dragged.right], [2, 1, 8, 2],
                       "the drag after a shift-click still grows from the first anchor")
        coordinator.release()

        // A plain press collapses it again.
        coordinator.press(at: fixture.pointInCell(row: 4, column: 0), clickCount: 1)
        XCTAssertEqual(fixture.tab.cellSelection?.cellCount, 1)
    }

    func testShiftClickWithNoCursorIsAPlainClickAndNeverOpensTheEditor() throws {
        let fixture = makeGrid()
        fixture.apply()
        fixture.coordinator.press(at: fixture.pointInCell(row: 3, column: 2), clickCount: 1, extend: true)
        XCTAssertEqual(fixture.tab.cellSelection?.cellCount, 1)

        fixture.coordinator.press(at: fixture.pointInCell(row: 6, column: 2), clickCount: 2, extend: true)
        XCTAssertTrue(fixture.log.beginEdit.isEmpty, "a shift double-click is an extension, not an edit")
        XCTAssertGreaterThan(try XCTUnwrap(fixture.tab.cellSelection).cellCount, 1)
    }

    // MARK: Autoscroll

    func testADragHeldBelowTheRowsScrollsThemAndKeepsExtendingTheSelection() throws {
        let fixture = makeGrid(rowCount: 200)
        fixture.apply()
        let clip = fixture.scroll.contentView
        fixture.coordinator.press(at: fixture.pointInCell(row: 2, column: 1), clickCount: 1)

        let below = CGPoint(x: 100, y: clip.bounds.maxY + 40)
        let windowPoint = fixture.table.convert(below, to: nil)
        let startY = clip.bounds.origin.y
        let startBottom = try XCTUnwrap(fixture.tab.cellSelection).bottom

        XCTAssertTrue(fixture.table.autoscrollOnce(windowPoint: windowPoint))
        XCTAssertGreaterThan(clip.bounds.origin.y, startY, "the rows moved up under the pointer")
        let first = try XCTUnwrap(fixture.tab.cellSelection).bottom
        XCTAssertGreaterThan(first, startBottom, "and the selection reached the rows that came into view")

        for _ in 0..<5 { fixture.table.autoscrollOnce(windowPoint: windowPoint) }
        XCTAssertGreaterThan(try XCTUnwrap(fixture.tab.cellSelection).bottom, first)
        XCTAssertEqual(fixture.tab.cellCursor?.anchor, CellPos(row: 2, column: 1))
    }

    func testAPointerInsideTheRowsDoesNotScroll() {
        let fixture = makeGrid(rowCount: 200)
        fixture.apply()
        let clip = fixture.scroll.contentView
        let start = clip.bounds.origin
        let inside = fixture.table.convert(CGPoint(x: 100, y: clip.bounds.midY), to: nil)
        XCTAssertFalse(fixture.table.autoscrollOnce(windowPoint: inside))
        XCTAssertEqual(clip.bounds.origin, start)
    }

    func testTheScrollStopsAtTheLastRow() {
        let fixture = makeGrid(rowCount: 60)
        fixture.apply()
        let clip = fixture.scroll.contentView
        let below = fixture.table.convert(CGPoint(x: 100, y: clip.bounds.maxY + 500), to: nil)
        var steps = 0
        while fixture.table.autoscrollOnce(windowPoint: below), steps < 500 { steps += 1 }
        XCTAssertLessThan(steps, 500, "it comes to rest rather than scrolling for ever")
        XCTAssertFalse(fixture.table.autoscrollOnce(windowPoint: below))
    }
}
