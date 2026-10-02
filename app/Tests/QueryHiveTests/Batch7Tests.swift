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

    func testMemorySortOrdersRowsAndServerSortLeavesThem() {
        let tab = QueryTab(title: "Q")
        tab.preview = PreviewResult(columns: [Event.Column(name: "n", type: "bigint")],
                                    rows: [["3"], ["1"], ["2"]],
                                    truncated: false, queryID: nil, elapsedMS: 0)
        tab.applyMemorySort(GridSort(column: 0, direction: .ascending))
        XCTAssertEqual(tab.displayedRows.map { $0[0] }, ["1", "2", "3"])
        tab.applyServerSort(column: 0, direction: .ascending)
        XCTAssertEqual(tab.displayedRows.map { $0[0] }, ["3", "1", "2"],
                       "a server order is never re-applied in memory")
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

    func testBaseResultIsStoredOnlyWhenSmall() {
        let columns = [Event.Column(name: "n", type: "bigint")]
        let tab = QueryTab(title: "Q")
        AppModel.applyPreviewDone(Event(event: "done"), columns: columns,
                                  rows: [["1"], ["2"]], to: tab, storeBase: true)
        XCTAssertEqual(tab.baseResult?.rows.count, 2)
        let big = QueryTab(title: "Big")
        AppModel.applyPreviewDone(Event(event: "done"), columns: columns,
                                  rows: Array(repeating: ["1"], count: 10_001),
                                  to: big, storeBase: true)
        XCTAssertNil(big.baseResult, "a large base is re-run, not held twice")
        XCTAssertTrue(BaseResultCache.shouldStore(rowCount: 10_000))
        XCTAssertFalse(BaseResultCache.shouldStore(rowCount: 10_001))
    }

    func testOffRestoresTheStoredBaseWithoutAQuery() {
        let (model, engine, _) = makeModel()
        let columns = [Event.Column(name: "n", type: "bigint")]
        let tab = QueryTab(title: "Q")
        tab.baseResult = PreviewResult(columns: columns, rows: [["1"], ["2"]],
                                       truncated: false, queryID: nil, elapsedMS: 0)
        tab.preview = PreviewResult(columns: columns, rows: [["2"], ["1"]],
                                    truncated: false, queryID: nil, elapsedMS: 0)
        tab.applyServerSort(column: 0, direction: .descending)
        model.clearSort(tab)
        XCTAssertNil(tab.activeSort)
        XCTAssertEqual(tab.preview?.rows.map { $0[0] }, ["1", "2"])
        XCTAssertTrue(engine.calls.isEmpty, "no query for a stored base")
    }

    func testBelowMinimumClearsTheServerSearch() {
        let (model, engine, _) = makeModel()
        let columns = [Event.Column(name: "nama", type: "varchar")]
        let base = PreviewResult(columns: columns, rows: [["KPM Sukamaju"]],
                                 truncated: false, queryID: nil, elapsedMS: 0)
        let tab = QueryTab(title: "Q")
        tab.baseResult = base
        tab.preview = PreviewResult(columns: columns, rows: [["KPM Sukamaju"]],
                                    truncated: false, queryID: nil, elapsedMS: 0)
        tab.serverSearch = "kpm"
        tab.gridSearch = "kp"
        model.fireServerSearch(tab, term: "kp")
        XCTAssertNil(tab.serverSearch)
        XCTAssertEqual(tab.preview?.rows.count, 1)
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
        tab.preview = PreviewResult(columns: [Event.Column(name: "nama", type: "varchar")],
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
