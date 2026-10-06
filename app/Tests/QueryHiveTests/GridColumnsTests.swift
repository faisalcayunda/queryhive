import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// Hide / reorder / rename, and the reconciliation that keeps a sort and a filter on the column
/// they were set on.
///
/// This is the half of the feature that is easy to get wrong and impossible to see: a moved column
/// draws in a new place, but the sort and the filter are keyed by the **source** index, so they have
/// to keep pointing at the same data. These tests pin that identity, and the one case it cannot
/// cover — hiding the sorted column.
final class GridColumnsTests: XCTestCase {
    override func setUp() {
        super.setUp()
        // Before any QueryTab is built: its constructor reads the stored output directory.
        isolateConnectionStore()
    }

    // MARK: The layout value

    func testTheLayoutStartsAsTheServersOwnOrder() {
        let layout = GridColumnLayout(count: 3)

        XCTAssertEqual(layout.visible, [0, 1, 2])
        XCTAssertEqual(layout.sourceCount, 3)
        XCTAssertEqual(layout.hiddenCount, 0)
    }

    func testHidingKeepsTheColumnSoShowingItRestoresItsOriginalPlace() {
        var layout = GridColumnLayout(count: 3)

        layout.hide(1)
        XCTAssertEqual(layout.visible, [0, 2])
        XCTAssertFalse(layout.isVisible(1))

        layout.show(1)
        XCTAssertEqual(layout.visible, [0, 1, 2], "back where it was, not appended")
    }

    func testMovingReordersTheDrawnColumnsOnly() {
        var layout = GridColumnLayout(count: 3)

        layout.move(from: 0, to: 2)

        XCTAssertEqual(layout.visible, [1, 2, 0])
        XCTAssertEqual(layout.sourceCount, 3, "a move is not a hide")
    }

    func testMovingPastTheEdgesIsClampedRatherThanRefused() {
        var layout = GridColumnLayout(count: 3)

        layout.move(from: 0, to: -5)
        XCTAssertEqual(layout.visible, [0, 1, 2])

        layout.move(from: 0, to: 99)
        XCTAssertEqual(layout.visible, [1, 2, 0])
    }

    func testAHiddenColumnKeepsItsSlotWhileTheDrawnOnesMoveAroundIt() {
        var layout = GridColumnLayout(count: 4)
        layout.hide(1)

        layout.move(from: 0, to: 2)

        // Drawn order is [2, 3, 0]; the hidden source 1 stays in the first slot it occupied, so
        // showing it again puts it back at position 1 rather than at the end.
        XCTAssertEqual(layout.visible, [2, 3, 0])
        layout.show(1)
        XCTAssertEqual(layout.visible, [2, 1, 3, 0])
    }

    func testRenameIsDisplayOnlyAndBlankReverts() {
        var layout = GridColumnLayout(count: 2)
        let columns = [Event.Column(name: "nama", type: "varchar"),
                       Event.Column(name: "nik", type: "varchar")]

        layout.rename(0, to: "  Nama Lengkap  ")
        XCTAssertEqual(layout.label(0, original: columns), "Nama Lengkap")
        XCTAssertTrue(layout.isRenamed(0))
        XCTAssertEqual(layout.label(1, original: columns), "nik", "untouched")

        layout.rename(0, to: "   ")
        XCTAssertFalse(layout.isRenamed(0))
        XCTAssertEqual(layout.label(0, original: columns), "nama")
    }

    func testDisplayPositionAndSourceIndexAreInverses() {
        var layout = GridColumnLayout(count: 3)
        layout.move(from: 0, to: 2)

        XCTAssertEqual(layout.source(at: 0), 1)
        XCTAssertEqual(layout.source(at: 2), 0)
        XCTAssertEqual(layout.position(of: 0), 2)
        XCTAssertNil(layout.position(of: 9), "no such column")
    }

    // MARK: Reconciliation: a sort and a filter follow their column

    private func loadThree(_ tab: QueryTab) {
        tab.showRows(columns: [Event.Column(name: "a", type: "bigint"),
                               Event.Column(name: "b", type: "varchar"),
                               Event.Column(name: "c", type: "varchar")],
                     rows: [["3", "x", "q"], ["1", "y", "r"], ["2", "y", "s"]],
                     truncated: false, queryID: nil, elapsedMS: 0)
    }

    /// One source column of the rows the grid draws, once the view the tab asked for has landed.
    private func shown(_ tab: QueryTab, _ column: Int) -> [String?] {
        let until = Date().addingTimeInterval(5)
        while tab.viewBusy, Date() < until { RunLoop.main.run(until: Date().addingTimeInterval(0.005)) }
        return tab.result.rows(in: 0..<tab.result.count, columns: [column]).map { $0[0] }
    }

    func testASortFollowsAMovedColumn() {
        let tab = QueryTab(title: "Q")
        loadThree(tab)
        tab.applyMemorySort(GridSort(column: 0, direction: .ascending))
        XCTAssertEqual(shown(tab, 0), ["1", "2", "3"])

        // Move `a` from first to last on screen. Its identity has not changed, so the order must
        // not either; only the chevron's place moves.
        tab.moveColumn(from: 0, to: 2)

        XCTAssertEqual(tab.activeSort, ActiveSort(column: 0, direction: .ascending, origin: .memory))
        XCTAssertEqual(tab.visibleColumnSources, [1, 2, 0])
        XCTAssertEqual(shown(tab, 0), ["1", "2", "3"],
                       "still ordered by source column 0")
    }

    func testAFilterFollowsAMovedColumn() {
        let tab = QueryTab(title: "Q")
        loadThree(tab)
        tab.columnFilters[0] = .text("2")

        tab.moveColumn(from: 0, to: 2)

        XCTAssertEqual(tab.columnFilters[0], .text("2"))
        XCTAssertEqual(shown(tab, 0), ["2"],
                       "the filter still reads the source column it was set on")
    }

    func testMovingAColumnKeepsAFilterAndASortOnDifferentColumns() {
        let tab = QueryTab(title: "Q")
        loadThree(tab)
        tab.columnFilters[1] = .text("y")
        tab.applyMemorySort(GridSort(column: 0, direction: .ascending))

        tab.moveColumn(from: 2, to: 0)

        XCTAssertEqual(tab.columnFilters[1], .text("y"))
        XCTAssertEqual(tab.activeSort, ActiveSort(column: 0, direction: .ascending, origin: .memory))
    }

    func testHidingTheSortedColumnDropsTheSort() {
        let tab = QueryTab(title: "Q")
        loadThree(tab)
        tab.applyMemorySort(GridSort(column: 0, direction: .ascending))

        tab.setColumnHidden(0, true)

        // The order cannot stay: its chevron has nowhere to stand, and an invisible order is a grid
        // that looks unsorted while not being in the server's order.
        XCTAssertNil(tab.activeSort)
        XCTAssertFalse(tab.columnLayout.isVisible(0))
    }

    func testHidingAnotherColumnKeepsTheSort() {
        let tab = QueryTab(title: "Q")
        loadThree(tab)
        tab.applyMemorySort(GridSort(column: 0, direction: .ascending))

        tab.setColumnHidden(1, true)

        XCTAssertEqual(tab.activeSort, ActiveSort(column: 0, direction: .ascending, origin: .memory))
        XCTAssertEqual(shown(tab, 0), ["1", "2", "3"])
    }

    func testAHiddenColumnsFilterStaysActiveRatherThanPointingElsewhere() {
        let tab = QueryTab(title: "Q")
        loadThree(tab)
        tab.columnFilters[1] = .text("y")

        tab.setColumnHidden(1, true)

        // The filter is keyed by the source column, so hiding the column does not move the filter
        // onto a neighbour — it keeps describing the same data, whether or not its funnel is drawn.
        XCTAssertEqual(tab.columnFilters[1], .text("y"))
        XCTAssertEqual(shown(tab, 1), ["y", "y"])
    }

    func testRenamingDoesNotTouchTheSortOrTheFilter() {
        let tab = QueryTab(title: "Q")
        loadThree(tab)
        tab.columnFilters[0] = .text("2")
        tab.applyMemorySort(GridSort(column: 1, direction: .descending))

        tab.renameColumn(0, to: "kode")

        XCTAssertEqual(tab.columnFilters[0], .text("2"))
        XCTAssertEqual(tab.activeSort, ActiveSort(column: 1, direction: .descending, origin: .memory))
    }

    func testASelectionIsClearedByAHideOrAMoveButNotByARename() {
        // The selection is a rectangle of display positions, so a column that moves or disappears
        // under it changes what it points at. Edits are keyed by source and survive.
        let tab = QueryTab(title: "Q")
        loadThree(tab)
        tab.cellSelection = CellRange(from: (row: 0, column: 0), to: (row: 1, column: 1))
        tab.cellEdits.edit("z", at: CellKey(row: 0, column: 0), original: "3")

        tab.moveColumn(from: 0, to: 1)
        XCTAssertNil(tab.cellSelection)
        XCTAssertEqual(tab.cellEdits.value(at: CellKey(row: 0, column: 0)), "z")

        tab.cellSelection = CellRange(from: (row: 0, column: 0), to: (row: 0, column: 0))
        tab.renameColumn(0, to: "kode")
        XCTAssertNotNil(tab.cellSelection, "a rename changes no position")
    }

    func testALayoutIsResetWhenTheColumnSetChangesButKeptAcrossARepaint() {
        let tab = QueryTab(title: "Q")
        loadThree(tab)
        tab.renameColumn(1, to: "kode")
        tab.setColumnHidden(2, true)

        // A repaint of the same result — a streaming run paints several times — keeps the layout.
        loadThree(tab)
        XCTAssertEqual(tab.columnLayout.label(1, original: tab.preview!.columns), "kode")
        XCTAssertFalse(tab.columnLayout.isVisible(2))

        // A result with a different number of columns cannot reuse it.
        tab.showRows(columns: [Event.Column(name: "a", type: "bigint"),
                                              Event.Column(name: "b", type: "varchar")],
                                    rows: [], truncated: false, queryID: nil, elapsedMS: 0)
        XCTAssertEqual(tab.columnLayout.sourceCount, 2)
        XCTAssertTrue(tab.columnLayout.isVisible(1), "renames do not survive a different column set")
        XCTAssertEqual(tab.columnLayout.label(1, original: tab.preview!.columns), "b")
    }

    func testShowAllAndResetPutTheGridBack() {
        let tab = QueryTab(title: "Q")
        loadThree(tab)
        tab.setColumnHidden(0, true)
        tab.renameColumn(1, to: "kode")
        tab.moveColumn(from: 0, to: 1)

        tab.showAllColumns()
        XCTAssertEqual(tab.columnLayout.hiddenCount, 0)

        tab.resetColumnLayout()
        XCTAssertEqual(tab.visibleColumnSources, [0, 1, 2])
        XCTAssertEqual(tab.columnLayout.label(1, original: tab.preview!.columns), "b")
    }

    // MARK: Cross-column search

    func testTheInMemorySearchFindsATermInAnyColumn() {
        XCTAssertTrue(SwiftGridReference.matches(["32.01", "KPM Sukamaju", "4"], term: "sukamaju"))
        XCTAssertTrue(SwiftGridReference.matches(["32.01", "KPM Sukamaju", "4"], term: "32.01"))
        XCTAssertFalse(SwiftGridReference.matches(["32.01", "KPM Sukamaju", nil], term: "bandung"))
        // A NULL contains nothing, and a blank term contains everything.
        XCTAssertTrue(SwiftGridReference.matches([nil, nil], term: "   "))
    }

    func testASearchNarrowsTheRowsAndIsResetLikeAFilter() {
        let tab = QueryTab(title: "Q")
        loadThree(tab)
        tab.applyMemorySort(GridSort(column: 0, direction: .ascending))

        tab.gridSearch = "y"

        XCTAssertNil(tab.activeSort, "a search is a claim about a new set of rows")
        XCTAssertEqual(shown(tab, 1).count, 2)
        XCTAssertEqual(shown(tab, 1), ["y", "y"])
        XCTAssertTrue(tab.hasGridSearch)
    }

    func testTheEscalatedStatementWrapsTheUsersSQLAndSearchesEveryColumn() throws {
        let columns = [Event.Column(name: "nama", type: "varchar"),
                       Event.Column(name: "jumlah", type: "bigint")]
        let trino = try SearchStatement.crossColumn(sql: "SELECT * FROM t;", term: "kpm",
                                                    columns: columns, kind: .trino)
        XCTAssertTrue(trino.hasPrefix("SELECT * FROM (\nSELECT * FROM t\n) AS queryhive_search"))
        XCTAssertTrue(trino.contains("STRPOS(LOWER(CAST(\"nama\" AS varchar)), LOWER('kpm')) > 0"))
        XCTAssertTrue(trino.contains("CAST(\"jumlah\" AS varchar)"))

        let my = try SearchStatement.crossColumn(sql: "SELECT * FROM t", term: "kpm",
                                                 columns: columns, kind: .mysql)
        XCTAssertTrue(my.contains("LOCATE(LOWER('kpm'), LOWER(CONCAT(`nama`))) > 0"))

        let pg = try SearchStatement.crossColumn(sql: "SELECT * FROM t", term: "kpm",
                                                 columns: columns, kind: .postgres)
        XCTAssertTrue(pg.contains("STRPOS(LOWER(\"nama\"::text), LOWER('kpm')) > 0"))
    }

    func testAQuoteInTheTermIsEscapedAndNestedColumnsAreSkipped() throws {
        let columns = [Event.Column(name: "payload", type: "array(bigint)"),
                       Event.Column(name: "nama", type: "varchar")]
        let sql = try SearchStatement.crossColumn(sql: "SELECT * FROM t", term: "O'Brien",
                                                  columns: columns, kind: .trino)
        XCTAssertTrue(sql.contains("'O''Brien'"))
        XCTAssertFalse(sql.contains("\"payload\""), "a nested column cannot be cast to text")
    }

    func testTheEscalationRefusesWhatItCannotWrap() {
        let columns = [Event.Column(name: "nama", type: "varchar")]

        XCTAssertThrowsError(try SearchStatement.crossColumn(sql: "SELECT 1", term: " ",
                                                             columns: columns, kind: .trino))
        XCTAssertThrowsError(try SearchStatement.crossColumn(sql: "SELECT 1; SELECT 2", term: "x",
                                                             columns: columns, kind: .trino))
        XCTAssertThrowsError(try SearchStatement.crossColumn(
            sql: "SELECT 1", term: "x",
            columns: [Event.Column(name: "p", type: "array(bigint)")], kind: .trino))
    }

    // MARK: A render (evidence about the view, not about the app)

    /// Draws the grid with a hidden column, a moved column, a rename and an active search, through a
    /// real AppKit view tree, and leaves a PNG behind.
    ///
    /// What it proves: the surface lays out and draws with all four features on at once. What it
    /// does not prove: that the app reaches them through the menus this file cannot click.
    /// `QH_RENDER_DIR=app/.build/render swift test --filter GridColumnsTests` writes the PNG.
    @MainActor
    func testTheGridRendersWithHiddenMovedRenamedColumnsAndASearch() throws {
        let model = AppModel()
        let tab = QueryTab(title: "Q")
        tab.showRows(
            columns: [Event.Column(name: "kode_wilayah", type: "varchar"),
                      Event.Column(name: "nama", type: "varchar"),
                      Event.Column(name: "jumlah_jiwa", type: "bigint"),
                      Event.Column(name: "aktif", type: "boolean"),
                      Event.Column(name: "catatan", type: "varchar")],
            rows: [["32.01.01.2001", "KPM Sukamaju", "4", "true", nil],
                   ["32.01.01.2002", "KPM Cibadak", "2", "true", "verifikasi"],
                   ["32.01.02.1004", "KPM Sukajadi", "1", "true", nil]],
            truncated: false, queryID: "20260131_120412_00042_abcde", elapsedMS: 210)
        tab.renameColumn(1, to: "Nama KPM")
        tab.setColumnHidden(3, true)          // `aktif` hidden
        tab.moveColumn(from: 0, to: 3)        // `kode_wilayah` moved to the end
        tab.gridSearch = "kpm"
        _ = shown(tab, 0)   // the view must land first: it drops the selection
        tab.cellSelection = CellRange(from: (row: 0, column: 0), to: (row: 1, column: 1))

        let view = ResultGrid(tab: tab)
            .environment(model)
            .background(Tone.canvas)
        try render(view, named: "grid-columns-search.png", size: CGSize(width: 900, height: 320))
    }

    @MainActor
    private func render(_ content: some View, named name: String, size: CGSize) throws {
        let host = NSHostingView(rootView: content)
        host.frame = CGRect(origin: .zero, size: size)
        host.layoutSubtreeIfNeeded()

        let rep = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds),
                                "\(name): no bitmap to draw into")
        host.cacheDisplay(in: host.bounds, to: rep)
        let data = try XCTUnwrap(rep.representation(using: .png, properties: [:]),
                                 "\(name) could not be encoded as a PNG")
        let directory = Self.renderDirectory
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try data.write(to: directory.appendingPathComponent(name))
        print("rendered \(name) to \(directory.path)")
    }

    private static var renderDirectory: URL {
        if let asked = ProcessInfo.processInfo.environment["QH_RENDER_DIR"], !asked.isEmpty {
            return URL(fileURLWithPath: asked, isDirectory: true)
        }
        return FileManager.default.temporaryDirectory
            .appendingPathComponent("queryhive-renders", isDirectory: true)
    }
}
