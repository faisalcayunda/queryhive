import AppKit
import QueryHiveFFI
import SwiftUI
import XCTest

@testable import QueryHive

/// The grid's in-memory sort, and the reader for a structured cell.
///
/// Both are decisions about the rows the grid is already holding rather than about the query: the
/// sort never re-runs anything and the reader never changes what a cell holds. These tests pin the
/// decisions that a reader of the code cannot infer from a type: what a NULL sorts as, when a cell
/// counts as a number, and when a value is worth opening.
final class ResultGridTests: XCTestCase {
    override func setUp() {
        super.setUp()
        // Before any QueryTab is built: its constructor reads the stored output directory.
        isolateConnectionStore()
    }

    // MARK: Ordering

    /// The order Rust gives `rows` over `column`: the sort the grid runs now, asserted against the
    /// store rather than the Swift comparator it replaced (that one lives on as
    /// `SwiftGridReference`, and `SortFixtureExport` holds the two together).
    private func rustOrder(_ rows: [[String?]], column: Int, _ direction: GridSort.Direction) throws -> [[String?]] {
        let width = rows.map(\.count).max() ?? 1
        let store = try TestStores.makeStore(columns: TestStores.textColumns(width), rows: rows)
        defer { store.release() }
        try store.applyBlocking(ViewSpec(sort: SortSpec(column: UInt32(column), descending: direction == .descending),
                                         filters: [], search: nil))
        return store.rows(in: 0..<store.count, columns: Array(0..<width))
    }

    func testNumbersSortByValueNotByText() throws {
        // The bug the comparator exists for: `[String?]` means a naive sort puts "10" before "9".
        let input: [[String?]] = [["10"], ["9"], ["2"], ["100"]]

        XCTAssertEqual(try rustOrder(input, column: 0, .ascending).map { $0[0] },
                       ["2", "9", "10", "100"] as [String?])
        XCTAssertEqual(try rustOrder(input, column: 0, .descending).map { $0[0] },
                       ["100", "10", "9", "2"] as [String?])
    }

    func testNullsSortLastAscendingAndFirstDescending() throws {
        // PostgreSQL's own default, and the reason descending is the exact reverse rather than a
        // second order with its own NULL rule.
        let input: [[String?]] = [["b"], [nil], ["a"], [nil]]

        XCTAssertEqual(try rustOrder(input, column: 0, .ascending).map { $0[0] },
                       ["a", "b", nil, nil] as [String?])
        XCTAssertEqual(try rustOrder(input, column: 0, .descending).map { $0[0] },
                       [nil, nil, "b", "a"] as [String?])
    }

    func testANumberSortsBeforeTextInAMixedColumn() throws {
        let input: [[String?]] = [["apple"], ["10"], ["banana"], ["9"]]

        XCTAssertEqual(try rustOrder(input, column: 0, .ascending).map { $0[0] },
                       ["9", "10", "apple", "banana"] as [String?])
    }

    func testACodeWithDotsIsTextNotANumber() throws {
        // `Decimal(string:)` reads "32.01.01.2001" as 32.01 and a timestamp as its year. The strict
        // parser is what stops a column of codes from being silently reordered as decimals.
        XCTAssertNil(SwiftGridReference.number("32.01.01.2001"))
        XCTAssertNil(SwiftGridReference.number("2026-07-25 15:30:06.233"))

        let input: [[String?]] = [["32.01.01.2002"], ["32.01.01.2001"]]
        XCTAssertEqual(try rustOrder(input, column: 0, .ascending).map { $0[0] },
                       ["32.01.01.2001", "32.01.01.2002"] as [String?])
    }

    func testLargeIntegersKeepTheirLastDigit() throws {
        // The reason the comparator uses `Decimal` rather than `Double`: past 2^53 the two differ.
        let input: [[String?]] = [["9007199254740993"], ["9007199254740992"]]
        XCTAssertEqual(try rustOrder(input, column: 0, .ascending).map { $0[0] },
                       ["9007199254740992", "9007199254740993"] as [String?])
    }

    func testEqualKeysKeepTheOrderTheyArrivedIn() throws {
        // A stable sort: reversing a sort and reversing it back has to give the server's order among
        // ties, which `sorted(by:)` does not promise on its own.
        let input: [[String?]] = [["a", "first"], ["b", "second"], ["a", "third"]]

        XCTAssertEqual(try rustOrder(input, column: 0, .ascending).map { $0[1] },
                       ["first", "third", "second"] as [String?])
    }

    func testTheSwiftReferenceStillTreatsAShortRowAsNull() {
        // The reference twin keeps the rule the Swift sort had; a store row is always full width.
        XCTAssertNil(SwiftGridReference.value(in: ["only"], at: 3))
    }

    func testTextWithDigitsSortsNaturally() throws {
        let input: [[String?]] = [["KPM 10"], ["KPM 9"], ["KPM 100"]]

        XCTAssertEqual(try rustOrder(input, column: 0, .ascending).map { $0[0] },
                       ["KPM 9", "KPM 10", "KPM 100"] as [String?])
    }

    func testAColumnPastTheEndOfEveryRowIsRefusedNotACrash() throws {
        // Reachable when a sort outlives its columns for an instant: Rust says so instead of
        // guessing, and the grid reports it rather than shuffling what it cannot compare.
        XCTAssertThrowsError(try rustOrder([["a"], ["b"], ["c"]], column: 9, .ascending))
    }

    // MARK: The header's cycle

    func testAHeaderClickCyclesAscendingDescendingOff() {
        let first = GridSort.next(nil, clickedColumn: 2)
        XCTAssertEqual(first, GridSort(column: 2, direction: .ascending))

        let second = GridSort.next(first, clickedColumn: 2)
        XCTAssertEqual(second, GridSort(column: 2, direction: .descending))

        // The third click clears back to the server's own order: the only order that is not the
        // grid's invention, and the state the header's chevron has to be able to leave.
        XCTAssertNil(GridSort.next(second, clickedColumn: 2))
    }

    func testClickingADifferentColumnStartsAscending() {
        let current = GridSort(column: 1, direction: .descending)

        XCTAssertEqual(GridSort.next(current, clickedColumn: 3),
                       GridSort(column: 3, direction: .ascending))
    }

    // MARK: When the order stops describing what is on screen

    func testNewRowsClearTheSort() {
        // Assigning `preview` is what "new rows arrived" means, and an order over the old rows
        // cannot describe the new ones.
        let tab = QueryTab(title: "Query 1")
        tab.applyMemorySort(GridSort(column: 0, direction: .ascending))

        tab.showRows(columns: [Event.Column(name: "a", type: "bigint")],
                                    rows: [["1"]], truncated: false, queryID: nil, elapsedMS: 0)

        XCTAssertNil(tab.activeSort)
    }

    func testAChangedFilterClearsTheSort() {
        // The sort is an order over the rows on screen; a filter changes which rows those are.
        let tab = QueryTab(title: "Query 1")
        tab.applyMemorySort(GridSort(column: 0, direction: .ascending))

        tab.columnFilters[0] = .values(["x"])

        XCTAssertNil(tab.activeSort)
    }

    /// Wait for the view a filter, a search or a sort asked for to land, the way the run loop does.
    @MainActor
    private func settle(_ tab: QueryTab, file: StaticString = #filePath, line: UInt = #line) {
        let until = Date().addingTimeInterval(5)
        while tab.viewBusy, Date() < until { RunLoop.main.run(until: Date().addingTimeInterval(0.005)) }
        XCTAssertFalse(tab.viewBusy, "the view never landed", file: file, line: line)
    }

    private func shown(_ tab: QueryTab) -> [String?] {
        tab.result.rows(in: 0..<tab.result.count, columns: [0]).map { $0[0] }
    }

    @MainActor
    func testTheViewFollowsTheThreeThingsThatChangeIt() {
        // The grid reads the store through a view, and the rows, the sort and the filter are the
        // three things that change what it holds: each must reach the store and bump the revision.
        let tab = QueryTab(title: "Query 1")
        let columns = [Event.Column(name: "n", type: "bigint")]
        tab.showRows(columns: columns, rows: [["3"], ["1"], ["2"]],
                                    truncated: false, queryID: nil, elapsedMS: 0)
        XCTAssertEqual(shown(tab), ["3", "1", "2"], "server order first")

        tab.applyMemorySort(GridSort(column: 0, direction: .ascending))
        settle(tab)
        XCTAssertEqual(shown(tab), ["1", "2", "3"], "the sort is followed")

        tab.applyMemorySort(nil)
        settle(tab)
        XCTAssertEqual(shown(tab), ["3", "1", "2"], "clearing it too")

        tab.showRows(columns: columns, rows: [["9"], ["8"]],
                                    truncated: false, queryID: nil, elapsedMS: 0)
        XCTAssertEqual(shown(tab), ["9", "8"], "and a new set of rows")

        tab.columnFilters[0] = .text("9")
        settle(tab)
        XCTAssertEqual(shown(tab), ["9"], "and a filter")
    }

    func testChangingTheSortDropsTheSelectionAndTheQueuedEdits() {
        // The selection and the edits are positions in the rows on screen, and sorting moves those
        // rows. Leaving either behind would attach it to the wrong row, exactly as a filter would.
        let tab = QueryTab(title: "Query 1")
        tab.showRows(columns: [Event.Column(name: "a", type: "bigint")],
                                    rows: [["1"], ["2"]], truncated: false, queryID: nil, elapsedMS: 0)
        tab.cellSelection = CellRange(from: (row: 0, column: 0), to: (row: 1, column: 0))
        tab.cellEdits.edit("9", at: CellKey(row: 0, column: 0), original: "1")

        tab.applyMemorySort(GridSort(column: 0, direction: .ascending))

        XCTAssertNil(tab.cellSelection)
        XCTAssertTrue(tab.cellEdits.isEmpty)
    }

    // MARK: Reading a structured value

    func testAnObjectPrettyPrintsWithStableKeys() throws {
        let formatted = try XCTUnwrap(GridValue.prettyPrinted("{\"b\":1,\"a\":2}"))

        XCTAssertTrue(formatted.contains("\n"), "the pretty form is on more than one line")
        // Keys are sorted, so one value always prints the same way and the reader is not the
        // dictionary's own whim.
        let a = try XCTUnwrap(formatted.range(of: "\"a\""))
        let b = try XCTUnwrap(formatted.range(of: "\"b\""))
        XCTAssertLessThan(a.lowerBound, b.lowerBound)
    }

    func testAValueThatIsNotJSONPrettyPrintsToNil() {
        // A PostgreSQL array literal, not a JSON list (D-9), and ordinary text. Both reach the
        // reader as the raw text the server sent.
        XCTAssertNil(GridValue.prettyPrinted("{1,NULL,3}"))
        XCTAssertNil(GridValue.prettyPrinted("hello"))
        XCTAssertNil(GridValue.prettyPrinted("[1,2"))
    }

    func testAStructuredColumnIsOpenableEvenWhenItsValueIsNotJSON() {
        // The raw PostgreSQL literal has to open too, or the reader would be for the parsed case
        // only — and the parsed case is the one that is already readable.
        XCTAssertTrue(GridValue.isOpenable(value: "{1,NULL,3}", type: "integer[]"))
        XCTAssertTrue(GridValue.isOpenable(value: "{1,NULL,3}", type: "array(bigint)"))
        XCTAssertTrue(GridValue.isOpenable(value: "{\"a\":1}", type: "json"))
    }

    func testJSONInAVarcharColumnIsOpenable() {
        XCTAssertTrue(GridValue.isOpenable(value: "{\"a\":1}", type: "varchar"))
        XCTAssertTrue(GridValue.isOpenable(value: "[1,2,3]", type: "varchar"))
    }

    func testAPlainWordIsNotOpenable() {
        // The container check keeps the reader off cells whose only content is a word the grid
        // already shows: `true` and `42` are valid JSON, but nothing is gained by opening them.
        XCTAssertFalse(GridValue.isOpenable(value: "KPM Sukamaju", type: "varchar"))
        XCTAssertFalse(GridValue.isOpenable(value: "true", type: "boolean"))
        XCTAssertFalse(GridValue.isOpenable(value: "42", type: "bigint"))
        XCTAssertFalse(GridValue.isOpenable(value: nil, type: "varchar"))
        XCTAssertFalse(GridValue.isOpenable(value: "", type: "varchar"))
    }

    func testStructuredTypesAreRecognisedAcrossDrivers() {
        for type in ["array(bigint)", "map(varchar,bigint)", "row(x integer)",
                     "json", "jsonb", "integer[]", "text[]"] {
            XCTAssertTrue(GridValue.isStructured(type: type), type)
        }
        for type in ["varchar", "bigint", "boolean", "timestamp(6)", "double", "numeric(38,10)"] {
            XCTAssertFalse(GridValue.isStructured(type: type), type)
        }
    }

    func testBinaryTypesAreRecognisedAcrossDriversAndOpenTheReader() {
        // The three drivers spell the byte family differently, and every one of them must open the
        // reader: the one-line cell cannot show a hex dump.
        for type in ["bytea", "blob", "tinyblob", "mediumblob", "longblob", "varbinary", "binary(16)"] {
            XCTAssertTrue(GridValue.isBinary(type: type), type)
            XCTAssertTrue(GridValue.isOpenable(value: "00ff10", type: type), type)
        }
        for type in ["varchar", "bigint", "json", "integer[]"] {
            XCTAssertFalse(GridValue.isBinary(type: type), type)
        }
    }

    // MARK: Renders (evidence about the views, not about the app)

    /// Draws the reader and the sort banner through a real AppKit view tree and leaves PNGs behind.
    ///
    /// What it proves: the two views lay out at a real size and draw. What it does not prove: that
    /// the app puts the reader in a popover, or the banner in the header. A rendered view is
    /// evidence about the view.
    ///
    /// `QH_RENDER_DIR=app/.build/render swift test --filter ResultGridTests` writes the PNGs.
    @MainActor
    func testTheReaderAndTheSortBannerRender() throws {
        // A JSON cell opens on its Tree mode and a byte cell on its Hex mode, so these three PNGs
        // are the reader's new surfaces and not just the text one.
        try render(CellValueViewer(value: "{\"b\":1,\"a\":[1,2]}", column: "payload", type: "json")
                .background(Tone.canvas),
                   named: "cell-reader-json.png", size: CGSize(width: 640, height: 340))
        try render(CellValueViewer(value: "{1,NULL,3}", column: "ints", type: "integer[]")
                .background(Tone.canvas),
                   named: "cell-reader-raw.png", size: CGSize(width: 640, height: 260))
        try render(CellValueViewer(value: "89504e470d0a1a0a0000000d494844520000001000000010",
                                   column: "favicon", type: "bytea")
                .background(Tone.canvas),
                   named: "cell-reader-hex.png", size: CGSize(width: 640, height: 300))
        try render(
            PartialOrderNote(fetched: 1000)
                .background(Tone.canvas)
                .frame(width: 620, height: 30),
            named: "grid-sort-banner.png", size: CGSize(width: 620, height: 30)
        )
    }

    // MARK: A Run over a Run

    /// The table outlives the Run that filled it (W8-F1), and with it everything it holds that
    /// belongs to the rows: the scroll position, the selection and the cursor. A table that is not
    /// rebuilt has to be told, and these two are what would go wrong first.
    @MainActor
    func testARunAgainStartsAtTheTopWithNothingSelected() throws {
        var script = StreamingEngine.Script()
        script.rows = (0..<400).map { ["row \($0)"] }
        let grid = HostedGrid(engine: { let e = StreamingEngine(); e.script = script; return e }())
        defer { grid.close() }
        grid.run()
        let table = try XCTUnwrap(grid.table)
        let scroll = try XCTUnwrap(table.enclosingScrollView)
        grid.tab.selectCells(anchor: CellPos(row: 3, column: 0), focus: CellPos(row: 4, column: 0))
        table.scrollRowToVisible(300)
        grid.spin(0.05)
        XCTAssertGreaterThan(scroll.contentView.bounds.origin.y, 1_000, "the fixture did not scroll")
        XCTAssertNotNil(grid.tab.cellSelection)

        script.rows = (0..<400).map { ["again \($0)"] }
        grid.engine.script = script
        grid.run()

        XCTAssertTrue(grid.table === table)
        XCTAssertNil(grid.tab.cellSelection, "the first result's selection followed the table into the second")
        XCTAssertNil(table.selection)
        XCTAssertEqual(scroll.contentView.bounds.origin.y, -scroll.contentView.contentInsets.top, accuracy: 1,
                       "the second result opened where the first one was scrolled to")
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

        let directory = Self.outputDirectory
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let path = directory.appendingPathComponent(name)
        try data.write(to: path)
        print("rendered \(name) to \(path.path)")
    }

    private static var outputDirectory: URL {
        if let asked = ProcessInfo.processInfo.environment["QH_RENDER_DIR"], !asked.isEmpty {
            return URL(fileURLWithPath: asked, isDirectory: true)
        }
        return FileManager.default.temporaryDirectory
            .appendingPathComponent("queryhive-renders", isDirectory: true)
    }
}
