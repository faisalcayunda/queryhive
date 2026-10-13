import XCTest

@testable import QueryHive

/// Opening a table from the object tree writes its column list, not `SELECT *`.
final class OpenTableStatementTests: XCTestCase {
    func testTheColumnsAreListedInTheTablesOrder() {
        XCTAssertEqual(OpenTableStatement.sql(table: "hive.analytics.penerima_manfaat",
                                              columns: ["id", "nama", "tanggal_lahir"], kind: .trino),
                       "SELECT id, nama, tanggal_lahir FROM hive.analytics.penerima_manfaat")
        XCTAssertEqual(OpenTableStatement.sql(table: "t", columns: ["z", "a", "m"], kind: .postgres),
                       "SELECT z, a, m FROM t", "the table's order, not alphabetical")
    }

    func testAColumnIsQuotedOnlyWhenItNeedsIt() {
        let columns = ["id", "Nama", "tanggal lahir", "order", "1st", "a\"b"]
        XCTAssertEqual(OpenTableStatement.sql(table: "\"public\".\"t\"", columns: columns, kind: .postgres),
                       "SELECT id, \"Nama\", \"tanggal lahir\", \"order\", \"1st\", \"a\"\"b\" FROM \"public\".\"t\"")
        XCTAssertEqual(OpenTableStatement.sql(table: "`db`.`t`", columns: ["id", "key", "a`b"], kind: .mysql),
                       "SELECT id, `key`, `a``b` FROM `db`.`t`")
    }

    func testNoColumnsIsTheOldSelectStar() {
        XCTAssertEqual(OpenTableStatement.sql(table: "t", columns: [], kind: .trino), "SELECT * FROM t")
    }

    func testAVeryWideTableKeepsSelectStar() {
        let wide = (1...OpenTableStatement.maxColumns).map { "c\($0)" }
        XCTAssertTrue(OpenTableStatement.sql(table: "t", columns: wide, kind: .trino).hasPrefix("SELECT c1, c2"),
                      "exactly the limit still lists them")
        XCTAssertEqual(OpenTableStatement.sql(table: "t", columns: wide + ["one_more"], kind: .trino), "SELECT * FROM t")
        XCTAssertEqual(OpenTableStatement.maxColumns, 200)
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
    private let explicit = "SELECT id, \"Nama\", \"order\" FROM \"public\".\"penerima_manfaat\""

    /// The runs against the database, not the bookkeeping around them (the history write after a run).
    private func previews(_ engine: MockEngine) -> [MockEngine.Invocation] {
        engine.calls.filter { $0.command == "preview" }
    }

    private func scriptColumns(_ engine: MockEngine, _ columns: [Event.Column]) {
        engine.answer("preview", with: .events([Event(event: "columns", columns: columns),
                                                Event(event: "done", rows: 0)]))
    }

    // MARK: Columns already known

    func testAKnownColumnListIsWrittenAndRunAtOnceWithoutAskingTheServer() throws {
        let (model, engine, node, _) = makeModel()
        scriptColumns(engine, columns)
        model.openTable(node, columns: ["id", "Nama", "order"])
        let tab = try XCTUnwrap(model.selectedTab)
        XCTAssertEqual(tab.sql, explicit)
        XCTAssertEqual(tab.sourceTable, qualified)
        XCTAssertTrue(wait { self.previews(engine).count == 1 })
        XCTAssertEqual(previews(engine).first?.env["SQL"], explicit, "the statement that is run is the statement that is shown")
        XCTAssertNotEqual(previews(engine).first?.env["LIMIT"], "1", "this is the real run, not the probe")
    }

    func testAnInspectorThatHasDescribedTheTableSuppliesItsColumns() throws {
        let (model, engine, node, connection) = makeModel()
        scriptColumns(engine, columns)
        let objects = QueryTab(title: "public")
        objects.connectionID = connection.id
        objects.objectScope = ObjectScope(connectionID: connection.id, catalog: "", schema: "public")
        objects.objectDetailTable = "penerima_manfaat"
        objects.objectDetailColumns = columns
        model.tabs.append(objects)
        XCTAssertEqual(model.inspectedColumns(for: node), ["id", "Nama", "order"])
        model.openTable(node)
        XCTAssertEqual(model.selectedTab?.sql, explicit)
        XCTAssertTrue(wait { self.previews(engine).count == 1 }, "no probe: the inspector already knew")
        XCTAssertEqual(previews(engine).count, 1)
    }

    func testAnInspectorStillLoadingOrFailedOrAboutAnotherTableSuppliesNothing() {
        let (model, _, node, connection) = makeModel()
        let objects = QueryTab(title: "public")
        objects.objectScope = ObjectScope(connectionID: connection.id, catalog: "", schema: "public")
        objects.objectDetailTable = "penerima_manfaat"
        objects.objectDetailColumns = columns
        model.tabs.append(objects)
        objects.objectDetailLoading = true
        XCTAssertNil(model.inspectedColumns(for: node))
        objects.objectDetailLoading = false
        objects.objectDetailError = "boom"
        XCTAssertNil(model.inspectedColumns(for: node))
        objects.objectDetailError = nil
        objects.objectDetailTable = "another"
        XCTAssertNil(model.inspectedColumns(for: node))
    }

    // MARK: Columns not known: one run, then the text follows it

    func testUnknownColumnsRunSelectStarOnceAndThenTheTextBecomesTheRunsOwnColumnList() throws {
        let (model, engine, node, _) = makeModel()
        scriptColumns(engine, columns)
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        // At once, as before: the tab is there holding the query it runs.
        XCTAssertEqual(tab.sql, "SELECT * FROM \(qualified)")
        XCTAssertTrue(wait { tab.sql == self.explicit }, "the text did not become the column list: \(tab.sql)")
        XCTAssertTrue(wait { !tab.previewing })
        // One query on the server, and it is the `SELECT *` as it always was, not a one-row probe.
        XCTAssertEqual(previews(engine).count, 1, "a second query was run: \(engine.calls.map(\.command))")
        XCTAssertEqual(previews(engine)[0].env["SQL"], "SELECT * FROM \(qualified)")
        XCTAssertNotEqual(previews(engine)[0].env["LIMIT"], "1")
        // The table the grid writes to, and the text a search, a sort or a count wraps, follow.
        XCTAssertEqual(tab.sourceTable, qualified)
        XCTAssertEqual(tab.previewedSQL, explicit)
        XCTAssertEqual(tab.previewBaseSQL, explicit)
        XCTAssertNil(tab.previewError)
    }

    func testAFailedRunLeavesSelectStarAndSaysNothingAboutColumns() throws {
        let (model, engine, node, _) = makeModel()
        engine.answer("preview", with: .failure(reason: "connection refused"))
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        XCTAssertTrue(wait { !tab.previewing })
        XCTAssertEqual(tab.sql, "SELECT * FROM \(qualified)", "exactly as today")
        XCTAssertEqual(previews(engine).count, 1)
        XCTAssertEqual(tab.previewError, "connection refused", "the run's own error, and nothing else")
    }

    func testARunThatCannotStartLeavesSelectStar() throws {
        let (model, engine, node, _) = makeModel()
        engine.answer("preview", with: .cannotStart(reason: "no engine"))
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        XCTAssertTrue(wait { !tab.previewing })
        XCTAssertEqual(tab.sql, "SELECT * FROM \(qualified)")
    }

    func testATableThatReportsNoColumnsKeepsSelectStar() throws {
        let (model, engine, node, _) = makeModel()
        scriptColumns(engine, [])
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        XCTAssertTrue(wait { !tab.previewing })
        spinLoop()
        XCTAssertEqual(tab.sql, "SELECT * FROM \(qualified)")
        XCTAssertEqual(tab.previewedSQL, "SELECT * FROM \(qualified)")
    }

    func testAVeryWideTableKeepsSelectStarWhateverTheRunReported() throws {
        let (model, engine, node, _) = makeModel()
        scriptColumns(engine, (1...(OpenTableStatement.maxColumns + 1)).map { Event.Column(name: "c\($0)", type: "int") })
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        XCTAssertTrue(wait { !tab.previewing })
        spinLoop()
        XCTAssertEqual(tab.sql, "SELECT * FROM \(qualified)")
    }

    func testTextTypedBeforeTheColumnsArriveIsNeitherOverwrittenNorTouched() throws {
        let (model, engine, node, _) = makeModel()
        scriptColumns(engine, columns)
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        tab.sql = "SELECT 1"   // before the run's answer, which arrives on the main queue
        XCTAssertTrue(wait { !tab.previewing })
        spinLoop()
        XCTAssertEqual(tab.sql, "SELECT 1", "the person's text was written over")
        XCTAssertEqual(tab.previewedSQL, "SELECT * FROM \(qualified)", "and the run's record is still what ran")
    }

    func testATabClosedBeforeTheColumnsArriveIsLeftAlone() throws {
        let (model, engine, node, _) = makeModel()
        scriptColumns(engine, columns)
        model.openTable(node)
        let tab = try XCTUnwrap(model.selectedTab)
        model.tabs.removeAll { $0 === tab }
        spinLoop()
        XCTAssertEqual(tab.sql, "SELECT * FROM \(qualified)")
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

    private func spinLoop() { RunLoop.current.run(until: Date().addingTimeInterval(0.3)) }

    // MARK: What the column list must not change

    func testCellEditsStillWriteBackToTheOpenedTable() throws {
        let (model, engine, node, connection) = makeModel()
        scriptColumns(engine, columns)
        model.openTable(node, columns: ["id", "Nama", "order"])
        let tab = try XCTUnwrap(model.selectedTab)
        XCTAssertEqual(tab.sql, explicit)
        // The table the grid writes to is `sourceTable`, which is the table's name, never a reading
        // of the statement.
        var edits = CellEdits()
        edits.edit("Baru", at: CellKey(row: 0, column: 1), original: "Lama")
        let plan = WritePlan.build(edits: edits, rows: [["1", "Lama", "2"]], columns: columns,
                                   table: tab.sourceTable, kind: connection.kind)
        XCTAssertEqual(plan.table, qualified)
        XCTAssertEqual(plan.statements.count, 1)
        XCTAssertTrue(plan.sql.first?.contains("UPDATE \(qualified)") == true, "\(plan.sql)")
        XCTAssertTrue(plan.sql.first?.contains("\"Nama\"") == true)
        // And what makes the grid editable at all is the table's name being there.
        XCTAssertNotNil(tab.sourceTable)
    }

    func testTheSortAndSearchWrappersStillWrapTheColumnListStatement() throws {
        let (model, engine, node, connection) = makeModel()
        scriptColumns(engine, columns)
        model.openTable(node, columns: ["id", "Nama", "order"])
        let tab = try XCTUnwrap(model.selectedTab)
        let sorted = try ServerSort.order(sql: tab.sql, column: "Nama", direction: .ascending, kind: connection.kind)
        XCTAssertEqual(sorted, "SELECT * FROM (\n\(explicit)\n) AS queryhive_sort ORDER BY \"Nama\" ASC NULLS LAST")
        let searched = try SearchStatement.crossColumn(sql: tab.sql, term: "x", columns: columns, kind: connection.kind)
        XCTAssertTrue(searched.hasPrefix("SELECT * FROM (\n\(explicit)\n) AS queryhive_search WHERE "), searched)
    }
}
