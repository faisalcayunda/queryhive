import XCTest

@testable import QueryHive

/// Opening a table from the object tree writes its column list, not `SELECT *`, laid out the way
/// Format SQL lays it out: each clause on a line of its own, and two or more columns one per line.
final class OpenTableStatementTests: XCTestCase {
    func testTheColumnsAreListedOnePerLineInTheTablesOrder() {
        XCTAssertEqual(OpenTableStatement.sql(table: "hive.analytics.penerima_manfaat",
                                              columns: ["id", "nama", "tanggal_lahir"], kind: .trino),
                       "SELECT\n    id,\n    nama,\n    tanggal_lahir\nFROM hive.analytics.penerima_manfaat")
        XCTAssertEqual(OpenTableStatement.sql(table: "t", columns: ["z", "a", "m"], kind: .postgres),
                       "SELECT\n    z,\n    a,\n    m\nFROM t", "the table's order, not alphabetical")
    }

    /// The formatter keeps a one-item list on the `SELECT` line, so this does too.
    func testOneColumnStaysOnTheSelectLine() {
        XCTAssertEqual(OpenTableStatement.sql(table: "t", columns: ["id"], kind: .trino), "SELECT id\nFROM t")
    }

    func testAColumnIsQuotedOnlyWhenItNeedsIt() {
        let columns = ["id", "Nama", "tanggal lahir", "order", "1st", "a\"b"]
        XCTAssertEqual(OpenTableStatement.sql(table: "\"public\".\"t\"", columns: columns, kind: .postgres),
                       "SELECT\n    id,\n    \"Nama\",\n    \"tanggal lahir\",\n    \"order\",\n    \"1st\",\n"
                       + "    \"a\"\"b\"\nFROM \"public\".\"t\"")
        XCTAssertEqual(OpenTableStatement.sql(table: "`db`.`t`", columns: ["id", "key", "a`b"], kind: .mysql),
                       "SELECT\n    id,\n    `key`,\n    `a``b`\nFROM `db`.`t`")
    }

    func testNoColumnsIsSelectStarOnTwoLines() {
        XCTAssertEqual(OpenTableStatement.sql(table: "t", columns: [], kind: .trino), "SELECT *\nFROM t")
    }

    func testAVeryWideTableKeepsSelectStar() {
        let wide = (1...OpenTableStatement.maxColumns).map { "c\($0)" }
        XCTAssertTrue(OpenTableStatement.sql(table: "t", columns: wide, kind: .trino).hasPrefix("SELECT\n    c1,\n    c2,\n"),
                      "exactly the limit still lists them")
        XCTAssertEqual(OpenTableStatement.sql(table: "t", columns: wide + ["one_more"], kind: .trino), "SELECT *\nFROM t")
        XCTAssertEqual(OpenTableStatement.maxColumns, 200)
    }

    /// The text a table opens with is already what Format SQL makes of it, in every dialect, so
    /// formatting a freshly opened table changes nothing.
    func testFormatSQLLeavesTheOpenedStatementAsItIs() {
        let tables: [ConnectionKind: String] = [.trino: "prod.public.orders", .postgres: "\"public\".\"Orders\"",
                                                .mysql: "`shop`.`orders`"]
        let lists = [[], ["id"], ["id", "created-at", "Nama", "order", "a\"b", "x`y"]]
        for (kind, table) in tables {
            for columns in lists {
                let sql = OpenTableStatement.sql(table: table, columns: columns, kind: kind)
                XCTAssertEqual(SQLFormat.plan(text: sql, selection: NSRange(location: 0, length: 0),
                                              dialect: kind.editorDialect, options: SQLFormat.options(tabWidth: 4)),
                               .unchanged, "\(kind): \(sql)")
            }
        }
    }
}

@MainActor
final class OpenTableModelTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
        useMemorySecretStore()
    }

    private let columns = [Event.Column(name: "id", type: "bigint"), Event.Column(name: "Nama", type: "varchar"),
                           Event.Column(name: "order", type: "int")]

    private func makeModel() -> (AppModel, MockEngine, TreeNode, Connection) {
        let engine = MockEngine()
        let model = AppModel()
        model.engine = engine
        let connection = Connection(id: UUID(), name: "Dev", color: .violet, kind: .postgres,
                                    host: "localhost", port: 5432, sslmode: "", user: "qh", database: "app",
                                    schema: "public", verify: false)
        model.connections = [connection]
        let schema = TreeNode.schema("public", parent: TreeNode.connection(connection))
        return (model, engine, TreeNode.table("penerima_manfaat", parent: schema), connection)
    }

    private func wait(_ timeout: Double = 5, until condition: () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if condition() { return true }
            RunLoop.current.run(until: Date().addingTimeInterval(0.02))
        }
        return condition()
    }

    private let qualified = "\"public\".\"penerima_manfaat\""
    private let explicit = "SELECT\n    id,\n    \"Nama\",\n    \"order\"\nFROM \"public\".\"penerima_manfaat\""
    private var star: String { "SELECT *\nFROM \(qualified)" }

    /// The runs against the database, not the bookkeeping around them (the history write after a run).
    private func previews(_ engine: MockEngine) -> [MockEngine.Invocation] {
        engine.calls.filter { $0.command == "preview" }
    }

    private func scriptColumns(_ engine: MockEngine, _ columns: [Event.Column]) {
        engine.answer("preview", with: .events([Event(event: "columns", columns: columns),
                                                Event(event: "done", rows: 0)]))
    }

    private func done(cancelled: Bool = false) -> Event {
        var event = Event(event: "done", rows: 0)
        if cancelled { event.cancelled = true }
        return event
    }

    private func waitForTheRunToEnd(_ tab: QueryTab) {
        XCTAssertTrue(wait { !tab.previewing && tab.stage != .running })
        RunLoop.current.run(until: Date().addingTimeInterval(0.2))
    }

    // MARK: One run, then the text follows it

    func testTheTabRunsSelectStarOnceAndThenTheTextBecomesTheRunsOwnColumnList() throws {
        let (model, engine, node, _) = makeModel()
        scriptColumns(engine, columns)
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        // At once, as before: the tab is there holding the query it runs.
        XCTAssertEqual(tab.sql, star)
        XCTAssertTrue(wait { tab.sql == self.explicit }, "the text did not become the column list: \(tab.sql)")
        waitForTheRunToEnd(tab)
        // One query on the server, and it is the `SELECT *` as it always was, not a one-row probe.
        XCTAssertEqual(previews(engine).count, 1, "a second query was run: \(engine.calls.map(\.command))")
        XCTAssertEqual(previews(engine)[0].env["SQL"], star)
        XCTAssertNotEqual(previews(engine)[0].env["LIMIT"], "1")
        // The table the grid writes to, and the text a search, a sort or a count wraps, follow.
        XCTAssertEqual(tab.sourceTable, qualified)
        XCTAssertEqual(tab.previewedSQL, explicit)
        XCTAssertEqual(tab.previewBaseSQL, explicit)
        XCTAssertNil(tab.previewError)
    }

    func testARunThatFailsBeforeAnyColumnsLeavesSelectStarAndSaysNothingAboutColumns() throws {
        let (model, engine, node, _) = makeModel()
        engine.answer("preview", with: .failure(reason: "connection refused"))
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        waitForTheRunToEnd(tab)
        XCTAssertEqual(tab.sql, star, "exactly as today")
        XCTAssertEqual(previews(engine).count, 1)
        XCTAssertEqual(tab.previewError, "connection refused", "the run's own error, and nothing else")
    }

    func testARunThatFailsAfterTheColumnsLeavesSelectStarAndTheServersErrorMark() throws {
        let (model, engine, node, _) = makeModel()
        var error = Event(event: "error")
        error.message = "division by zero"
        error.position = 8
        engine.answer("preview", with: .failure(events: [Event(event: "columns", columns: columns), error],
                                                reason: "division by zero"))
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        waitForTheRunToEnd(tab)
        XCTAssertEqual(tab.sql, star, "the header arrived and the run still failed")
        XCTAssertEqual(tab.previewedSQL, star)
        let mark = try XCTUnwrap(tab.errorMark, "the server's error position was dropped")
        XCTAssertEqual(mark.sqlSnapshot, star)
    }

    func testAStoppedRunLeavesSelectStar() throws {
        let (model, engine, node, _) = makeModel()
        engine.answer("preview", with: .events([Event(event: "columns", columns: columns), done(cancelled: true)]))
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        waitForTheRunToEnd(tab)
        XCTAssertEqual(tab.preview?.stopped, true)
        XCTAssertEqual(tab.sql, star)
    }

    func testARunThatCannotStartLeavesSelectStar() throws {
        let (model, engine, node, _) = makeModel()
        engine.answer("preview", with: .cannotStart(reason: "no engine"))
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        waitForTheRunToEnd(tab)
        XCTAssertEqual(tab.sql, star)
    }

    func testATableThatReportsNoColumnsKeepsSelectStar() throws {
        let (model, engine, node, _) = makeModel()
        scriptColumns(engine, [])
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        waitForTheRunToEnd(tab)
        XCTAssertEqual(tab.sql, star)
        XCTAssertEqual(tab.previewedSQL, star)
    }

    func testTheLimitIsTwoHundredColumnsAsTheModelSeesIt() throws {
        for count in [OpenTableStatement.maxColumns, OpenTableStatement.maxColumns + 1] {
            let (model, engine, node, _) = makeModel()
            scriptColumns(engine, (1...count).map { Event.Column(name: "c\($0)", type: "int") })
            model.openTable(node)
            let tab = try XCTUnwrap(model.selectedTab)
            waitForTheRunToEnd(tab)
            if count == OpenTableStatement.maxColumns {
                XCTAssertTrue(tab.sql.hasPrefix("SELECT\n    c1,\n    c2,\n"), "\(count) columns are listed: \(tab.sql.prefix(40))")
                XCTAssertTrue(tab.sql.hasSuffix(",\n    c200\nFROM \(qualified)"))
            } else {
                XCTAssertEqual(tab.sql, star, "\(count) columns keep the star")
            }
        }
    }

    func testTextTypedBeforeTheRunFinishesIsNeitherOverwrittenNorTouched() throws {
        let (model, engine, node, _) = makeModel()
        scriptColumns(engine, columns)
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        tab.sql = "SELECT 1"   // before the run's answer, which arrives on the main queue
        waitForTheRunToEnd(tab)
        XCTAssertEqual(tab.sql, "SELECT 1", "the person's text was written over")
        XCTAssertEqual(tab.previewedSQL, star, "and the run's record is still what ran")
    }

    /// Typing the editor has not written back yet is handed to the model by `flushEditors`, and the
    /// rewrite has to ask for it first: without the flush it would see the untouched `SELECT *` and
    /// write over the person's text.
    func testTheRewriteFlushesTheEditorBeforeItLooksAtTheText() throws {
        let (model, engine, node, _) = makeModel()
        scriptColumns(engine, columns)
        let saved = AppModel.flushEditors
        defer { AppModel.flushEditors = saved }
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        // An editor with typing in it that it has not published: the flush is what publishes it.
        var pending: String? = "SELECT 1 -- typed, not yet written back"
        AppModel.flushEditors = { if let text = pending { tab.sql = text; pending = nil } }
        waitForTheRunToEnd(tab)
        XCTAssertNil(pending, "the rewrite never asked the editor for its text")
        XCTAssertEqual(tab.sql, "SELECT 1 -- typed, not yet written back")
    }

    func testATabClosedBeforeTheRunFinishesIsLeftAlone() throws {
        let (model, engine, node, _) = makeModel()
        scriptColumns(engine, columns)
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        model.tabs.removeAll { $0 === tab }
        spinLoop()
        XCTAssertEqual(tab.sql, star)
    }

    func testTheSecondRunOfTheColumnListIsAnOrdinaryRunOfWhatTheEditorShows() throws {
        let (model, engine, node, _) = makeModel()
        scriptColumns(engine, columns)
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        XCTAssertTrue(wait { tab.sql == self.explicit && !tab.previewing })
        model.preview(tab)
        XCTAssertTrue(wait { self.previews(engine).count == 2 })
        XCTAssertEqual(previews(engine)[1].env["SQL"], explicit)
    }

    // MARK: What the column list must not change

    func testACellEditStillWritesBackToTheOpenedTableAfterTheRewrite() throws {
        let (model, engine, node, connection) = makeModel()
        scriptColumns(engine, columns)
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        XCTAssertTrue(wait { tab.sql == self.explicit })
        waitForTheRunToEnd(tab)
        // Everything the plan reads comes from the tab as it is after the rewrite: the table the
        // grid writes to, and the columns the run reported.
        let result = try XCTUnwrap(tab.preview?.columns)
        XCTAssertEqual(result.map(\.name), ["id", "Nama", "order"])
        var edits = CellEdits()
        edits.edit("Baru", at: CellKey(row: 0, column: 1), original: "Lama")
        let plan = WritePlan.build(edits: edits, rows: [["1", "Lama", "2"]], columns: result,
                                   table: tab.sourceTable, kind: connection.kind)
        XCTAssertEqual(plan.table, qualified)
        XCTAssertEqual(plan.statements.count, 1)
        XCTAssertTrue(plan.sql.first?.contains("UPDATE \(qualified)") == true, "\(plan.sql)")
        XCTAssertTrue(plan.sql.first?.contains("\"Nama\"") == true)
        // The grid offers to commit because the table's name is there, not because of the statement.
        XCTAssertNotNil(tab.sourceTable)
    }

    func testServerSortAndSearchWrapTheRewrittenStatement() throws {
        let (model, engine, node, connection) = makeModel()
        scriptColumns(engine, columns)
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        XCTAssertTrue(wait { tab.previewBaseSQL == self.explicit })
        waitForTheRunToEnd(tab)
        // The model's own server sort, which wraps `previewBaseSQL`.
        model.sortOnServer(tab, column: Event.Column(name: "Nama", type: "varchar"), source: 1, direction: .ascending)
        XCTAssertTrue(wait { self.previews(engine).count == 2 }, "the sort was not sent")
        XCTAssertEqual(previews(engine)[1].env["SQL"],
                       "SELECT * FROM (\n\(explicit)\n) AS queryhive_sort ORDER BY \"Nama\" ASC NULLS LAST")
        let searched = try SearchStatement.crossColumn(sql: try XCTUnwrap(tab.previewBaseSQL), term: "x",
                                                       columns: columns, kind: connection.kind)
        XCTAssertTrue(searched.hasPrefix("SELECT * FROM (\n\(explicit)\n) AS queryhive_search WHERE "), searched)
    }

    private func spinLoop() { RunLoop.current.run(until: Date().addingTimeInterval(0.3)) }
}
