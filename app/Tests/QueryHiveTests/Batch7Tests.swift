import XCTest

@testable import QueryHive

/// Batch 7 server-first sort and search (performance plan §7).
final class Batch7Tests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    private func makeModel() -> (AppModel, MockEngine, Connection) {
        let engine = MockEngine()
        engine.answer("history_add", with: .events([]))
        engine.answer("history", with: .events([]))
        let model = AppModel()
        model.engine = engine
        let connection = Connection(id: UUID(), name: "Dev", color: .violet, kind: .trino,
                                    host: "localhost", port: 8080, sslmode: "",
                                    user: "qh", database: "hive", schema: "analytics", verify: false)
        model.connections = [connection]
        return (model, engine, connection)
    }

    func testFirstClickSortsThenFlipsThenClearsFromEitherStart() {
        // Ascending start: off, ascending, descending, off.
        XCTAssertEqual(GridSort.next(nil, clickedColumn: 0),
                       GridSort(column: 0, direction: .ascending))
        // Descending start reaches ascending too: the second click flips.
        let started = GridSort.next(nil, clickedColumn: 0, firstDirection: .descending)
        XCTAssertEqual(started, GridSort(column: 0, direction: .descending))
        XCTAssertEqual(GridSort.next(started, clickedColumn: 0, firstDirection: .descending),
                       GridSort(column: 0, direction: .ascending))
        XCTAssertNil(GridSort.next(GridSort(column: 0, direction: .ascending),
                                   clickedColumn: 0, firstDirection: .descending))
    }

    func testApplyingOneOriginClearsTheOther() {
        let tab = QueryTab(title: "Q")
        tab.applyMemorySort(GridSort(column: 1, direction: .ascending))
        XCTAssertEqual(tab.activeSort?.origin, .memory)
        tab.applyServerSort(column: 1, direction: .descending)
        XCTAssertEqual(tab.activeSort,
                       ActiveSort(column: 1, direction: .descending, origin: .server))
        tab.applyMemorySort(GridSort(column: 0, direction: .ascending))
        XCTAssertEqual(tab.activeSort,
                       ActiveSort(column: 0, direction: .ascending, origin: .memory))
    }

    func testTheThreeFallbackCasesRouteToMemory() {
        XCTAssertEqual(SortPolicy.route(builderRefused: true, showingPlan: false,
                                        isObjectResult: false), .memory, "refused builder")
        XCTAssertEqual(SortPolicy.route(builderRefused: false, showingPlan: true,
                                        isObjectResult: false), .memory, "plan on screen")
        XCTAssertEqual(SortPolicy.route(builderRefused: false, showingPlan: false,
                                        isObjectResult: true), .memory, "object preview")
        XCTAssertEqual(SortPolicy.route(builderRefused: false, showingPlan: false,
                                        isObjectResult: false), .server, "otherwise server")
    }

    /// The first source column of what the grid draws, once the view the tab asked for has landed.
    private func shown(_ tab: QueryTab) -> [String?] {
        let until = Date().addingTimeInterval(5)
        while tab.viewBusy, Date() < until { RunLoop.main.run(until: Date().addingTimeInterval(0.005)) }
        return tab.result.rows(in: 0..<tab.result.count, columns: [0]).map { $0[0] }
    }

    func testMemorySortOrdersRowsAndServerSortLeavesThem() {
        let tab = QueryTab(title: "Q")
        tab.showRows(columns: [Event.Column(name: "n", type: "bigint")],
                                    rows: [["3"], ["1"], ["2"]],
                                    truncated: false, queryID: nil, elapsedMS: 0)
        tab.applyMemorySort(GridSort(column: 0, direction: .ascending))
        XCTAssertEqual(shown(tab), ["1", "2", "3"])
        XCTAssertNotNil(tab.viewSpec.sort)
        tab.applyServerSort(column: 0, direction: .ascending)
        XCTAssertNil(tab.viewSpec.sort, "a server order is never asked of the store: it arrives sorted")
    }

    func testSearchNeedsThreeCharactersAndDebouncesAt250ms() {
        XCTAssertEqual(GridSearch.minimumLength, 3)
        XCTAssertEqual(GridSearch.debounceInterval, 0.25, accuracy: 0.001)
        XCTAssertNil(GridSearch.searchableTerm("kp"), "below minimum the search is cleared")
        XCTAssertNil(GridSearch.searchableTerm("   "))
        XCTAssertEqual(GridSearch.searchableTerm("kpm"), "kpm")
        XCTAssertEqual(GridSearch.searchableTerm("  kpm  "), "kpm", "trimmed first")
    }

    func testStagedEditsRejectServerSortAndSearch() {
        let (model, engine, connection) = makeModel()
        let tab = QueryTab(title: "Q")
        tab.connectionID = connection.id
        tab.previewBaseSQL = "SELECT * FROM t"
        tab.previewedSQL = "SELECT * FROM t"
        tab.cellEdits.edit("x", at: CellKey(row: 0, column: 0), original: "y")
        model.sortOnServer(tab, column: Event.Column(name: "n", type: "bigint"),
                           source: 0, direction: .ascending)
        model.searchOnServer(tab, term: "kpm")
        XCTAssertTrue(engine.calls.isEmpty, "rejected before any run")
        XCTAssertNil(tab.activeSort)
        XCTAssertNil(tab.serverSearch)
        XCTAssertEqual(tab.logLines.suffix(2).map(\.text),
                       [AppModel.stagedEditsMessage, AppModel.stagedEditsMessage])
    }

    func testBaseResultIsStoredWhateverItsSizeButNotWhenStopped() throws {
        let columns = [Event.Column(name: "n", type: "bigint")]
        let tab = QueryTab(title: "Q")
        let store = try TestStores.makeStore(columns: columns, rows: [["1"], ["2"]])
        AppModel.applyPreviewDone(Event(event: "done"), columns: columns, rowCount: 2, to: tab,
                                  store: store, storeBase: true)
        XCTAssertTrue(tab.baseResult?.rows === store)
        XCTAssertEqual(tab.baseResult?.meta.rowCount, 2)

        // No 10,000-row rule any more: the base is a store of its own, and spill covers the size.
        let big = QueryTab(title: "Big")
        let bigStore = try TestStores.makeStore(columns: columns, rows: Array(repeating: ["1"], count: 10_001))
        AppModel.applyPreviewDone(Event(event: "done"), columns: columns, rowCount: 10_001, to: big,
                                  store: bigStore, storeBase: true)
        XCTAssertNotNil(big.baseResult)

        // A stopped run is partial, and "off" must not return to a partial result.
        let stopped = QueryTab(title: "Stopped")
        var event = Event(event: "done")
        event.cancelled = true
        AppModel.applyPreviewDone(event, columns: columns, rowCount: 2, to: stopped,
                                  store: store, storeBase: true)
        XCTAssertNil(stopped.baseResult)
    }

    func testOffRestoresTheStoredBaseWithoutAQuery() throws {
        let (model, engine, _) = makeModel()
        let columns = [Event.Column(name: "n", type: "bigint")]
        let tab = QueryTab(title: "Q")
        let base = try TestStores.makeStore(columns: columns, rows: [["1"], ["2"]])
        tab.baseResult = ResultSlot(meta: PreviewResult(columns: columns, rowCount: 2, truncated: false,
                                                        queryID: nil, elapsedMS: 0), rows: base)
        tab.showRows(columns: columns, rows: [["2"], ["1"]],
                                    truncated: false, queryID: nil, elapsedMS: 0)
        let sorted = try XCTUnwrap(tab.activeResult)
        tab.applyServerSort(column: 0, direction: .descending)
        model.clearSort(tab)
        XCTAssertNil(tab.activeSort)
        XCTAssertTrue(tab.activeResult === base, "the base store is what the grid draws again")
        XCTAssertTrue(sorted.isReleased, "the server-sorted store let go")
        XCTAssertEqual(shown(tab), ["1", "2"])
        XCTAssertEqual(tab.preview?.rowCount, 2)
        XCTAssertTrue(engine.calls.isEmpty, "no query for a stored base")
    }

    func testBelowMinimumClearsTheServerSearch() throws {
        let (model, engine, _) = makeModel()
        let columns = [Event.Column(name: "nama", type: "varchar")]
        let tab = QueryTab(title: "Q")
        let store = try TestStores.makeStore(columns: columns, rows: [["KPM Sukamaju"]])
        tab.baseResult = ResultSlot(meta: PreviewResult(columns: columns, rowCount: 1, truncated: false,
                                                        queryID: nil, elapsedMS: 0), rows: store)
        tab.showRows(columns: columns, rows: [["KPM Sukamaju"]],
                                    truncated: false, queryID: nil, elapsedMS: 0)
        tab.serverSearch = "kpm"
        tab.gridSearch = "kp"
        model.fireServerSearch(tab, term: "kp")
        XCTAssertNil(tab.serverSearch)
        XCTAssertEqual(tab.preview?.rowCount, 1)
        XCTAssertTrue(engine.calls.isEmpty, "clearing below minimum runs nothing")
    }

    func testServerErrorIsShownWithoutSilentFallback() async {
        let (model, engine, connection) = makeModel()
        engine.answer("preview", with: .failure(reason: "relation \"t\" does not exist"))
        let tab = QueryTab(title: "Q")
        tab.connectionID = connection.id
        tab.previewBaseSQL = "SELECT * FROM t"
        model.sortOnServer(tab, column: Event.Column(name: "n", type: "bigint"),
                           source: 0, direction: .descending)
        let deadline = Date().addingTimeInterval(5)
        while tab.previewing, Date() < deadline {
            try? await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertFalse(tab.previewing, "the run finished")
        XCTAssertEqual(tab.previewError, "relation \"t\" does not exist")
        XCTAssertEqual(tab.activeSort?.origin, .server, "no silent fallback")
        XCTAssertEqual(tab.panel, .log)
    }

    func testTwoKeystrokesFireOneServerSearch() async {
        let (model, engine, connection) = makeModel()
        var columns = Event(event: "columns")
        columns.columns = [Event.Column(name: "nama", type: "varchar")]
        engine.answer("preview", with: .events([columns, Event(event: "done")]))
        let tab = QueryTab(title: "Q")
        tab.connectionID = connection.id
        tab.previewBaseSQL = "SELECT * FROM t"
        tab.showRows(columns: [Event.Column(name: "nama", type: "varchar")],
                                    rows: [["KPM Sukamaju"], ["KPM Cibadak"]],
                                    truncated: false, queryID: nil, elapsedMS: 0)
        tab.gridSearch = "kp"
        model.scheduleServerSearch(tab)
        tab.gridSearch = "kpm"
        model.scheduleServerSearch(tab)
        let deadline = Date().addingTimeInterval(5)
        while tab.serverSearch == nil, Date() < deadline {
            try? await Task.sleep(for: .milliseconds(10))
        }
        XCTAssertEqual(tab.serverSearch, "kpm", "only the last pause fires")
        XCTAssertEqual(engine.calls.filter { $0.command == "preview" }.count, 1)
        XCTAssertTrue(engine.calls.first { $0.command == "preview" }?.env["SQL"]?
            .contains("queryhive_search") == true)
    }
}
