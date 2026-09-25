import XCTest

@testable import QueryHive

/// What the editor offers after a `.`.
///
/// The bug these cover: the candidate list ignored the typed qualifier and swept every node in the
/// tree, so `hive.analytics.` offered tables from other schemas and other connections. A list that
/// looks plausible and is mostly wrong is worse than a short one, because the user cannot tell
/// which row is the right one.
@MainActor
final class SuggestionScopeTests: XCTestCase {
    override func setUp() {
        super.setUp()
        // Before any AppModel is built: that constructor reads connections.json, and the actions
        // these tests drive write it.
        isolateConnectionStore()
    }


    private func model() -> AppModel {
        AppModel()
    }

    /// A Trino connection with two catalogs, two schemas in one of them, and tables in each.
    ///
    /// Built by hand rather than by expanding the tree, because expansion is a network round trip
    /// and these tests are about resolution, not fetching.
    @discardableResult
    private func seed(_ model: AppModel) -> (connection: Connection, hive: TreeNode, analytics: TreeNode, bronze: TreeNode) {
        let connection = Connection(id: UUID(), name: "Warehouse", color: .violet, kind: .trino,
                                    host: "trino.internal", port: 8443, sslmode: "prefer",
                                    user: "queryhive", database: "hive", schema: "analytics", verify: true)
        model.connections = [connection]
        model.rebuildTree()

        let root = model.tree[0]
        let hive = TreeNode.catalog("hive", parent: root)
        let tpch = TreeNode.catalog("tpch", parent: root)
        let analytics = TreeNode.schema("analytics", parent: hive)
        let bronze = TreeNode.schema("bronze", parent: hive)
        analytics.children = [TreeNode.table("penerima_manfaat", parent: analytics),
                              TreeNode.table("wilayah", parent: analytics)]
        bronze.children = [TreeNode.table("raw_kpm", parent: bronze)]
        hive.children = [analytics, bronze]
        root.children = [hive, tpch]
        return (connection, hive, analytics, bronze)
    }

    private func tab(_ model: AppModel, connection: Connection) -> QueryTab {
        let tab = QueryTab(title: "q")
        tab.connectionID = connection.id
        model.tabs = [tab]
        model.selectedTabID = tab.id
        return tab
    }

    private func texts(_ suggestions: [SQLSuggestion]) -> [String] {
        suggestions.map(\.text)
    }

    // MARK: Resolution

    func testAPathOfCatalogAndSchemaOffersOnlyThatSchemasTables() {
        let model = model()
        let (connection, _, _, _) = seed(model)
        let tab = tab(model, connection: connection)

        let offered = texts(model.suggestions(for: tab, prefix: "", path: ["hive", "analytics"]))
        XCTAssertEqual(offered.sorted(), ["penerima_manfaat", "wilayah"],
                       "a schema offered a table from another schema, or missed one of its own")
        XCTAssertFalse(offered.contains("raw_kpm"), "the bronze schema's table leaked into analytics")
    }

    func testTheOtherSchemasTablesDoNotAppear() {
        let model = model()
        let (connection, _, _, _) = seed(model)
        let tab = tab(model, connection: connection)

        let offered = texts(model.suggestions(for: tab, prefix: "ra", path: ["hive", "bronze"]))
        XCTAssertEqual(offered, ["raw_kpm"])
    }

    func testACatalogOffersItsSchemasNotItsTables() {
        let model = model()
        let (connection, _, _, _) = seed(model)
        let tab = tab(model, connection: connection)

        let suggestions = model.suggestions(for: tab, prefix: "", path: ["hive"])
        XCTAssertEqual(texts(suggestions).sorted(), ["analytics", "bronze"])
        XCTAssertTrue(suggestions.allSatisfy { $0.kind == .schema },
                      "a catalog offered something that is not a schema")
    }

    func testThePrefixStillFiltersWithinTheScope() {
        let model = model()
        let (connection, _, _, _) = seed(model)
        let tab = tab(model, connection: connection)

        XCTAssertEqual(texts(model.suggestions(for: tab, prefix: "pen", path: ["hive", "analytics"])),
                       ["penerima_manfaat"])
        XCTAssertEqual(texts(model.suggestions(for: tab, prefix: "wil", path: ["hive", "analytics"])),
                       ["wilayah"])
        XCTAssertTrue(model.suggestions(for: tab, prefix: "zzz", path: ["hive", "analytics"]).isEmpty)
    }

    func testAPathThatResolvesToNothingOffersNothing() {
        let model = model()
        let (connection, _, _, _) = seed(model)
        let tab = tab(model, connection: connection)

        // No fallback to the whole tree: a wrong table is worse than no suggestion.
        XCTAssertTrue(model.suggestions(for: tab, prefix: "", path: ["hive", "nosuch"]).isEmpty)
        XCTAssertTrue(model.suggestions(for: tab, prefix: "", path: ["nosuch", "analytics"]).isEmpty)
    }

    func testNoKeywordsOrColumnsUnderAPath() {
        let model = model()
        let (connection, _, _, _) = seed(model)
        let tab = tab(model, connection: connection)
        tab.columns = [Event.Column(name: "kode_wilayah", type: "varchar")]

        let underPath = model.suggestions(for: tab, prefix: "", path: ["hive", "analytics"])
        XCTAssertFalse(underPath.contains { $0.kind == .keyword })
        XCTAssertFalse(underPath.contains { $0.kind == .column })

        // And a bare word still gets them, or the editor would have lost its keyword list.
        let bare = model.suggestions(for: tab, prefix: "ko", path: [])
        XCTAssertTrue(bare.contains { $0.text == "kode_wilayah" })
    }

    func testABareWordStillSeesTheWholeTree() {
        let model = model()
        let (connection, _, _, _) = seed(model)
        let tab = tab(model, connection: connection)

        // `FROM pe…` has no path, so the answer is every match anywhere — that is the behaviour
        // that makes a first table name reachable without expanding anything.
        let offered = texts(model.suggestions(for: tab, prefix: "pe", path: []))
        XCTAssertTrue(offered.contains("penerima_manfaat"))
    }

    // MARK: Shape per driver

    func testAPostgresPathIsSchemaThenTable() {
        let model = model()
        let connection = Connection(id: UUID(), name: "Warehouse", color: .violet, kind: .postgres,
                                    host: "pg.internal", port: 5432, sslmode: "prefer",
                                    user: "analyst", database: "warehouse", schema: "public", verify: true)
        model.connections = [connection]
        model.rebuildTree()
        let root = model.tree[0]
        let publicSchema = TreeNode.schema("public", parent: root)
        let sales = TreeNode.schema("sales", parent: root)
        publicSchema.children = [TreeNode.table("orders", parent: publicSchema)]
        sales.children = [TreeNode.table("invoices", parent: sales)]
        root.children = [publicSchema, sales]
        let tab = tab(model, connection: connection)

        // Postgres has no catalog level: `public.` is a schema, and the database never appears.
        XCTAssertEqual(texts(model.suggestions(for: tab, prefix: "", path: ["public"])), ["orders"])
        XCTAssertEqual(texts(model.suggestions(for: tab, prefix: "", path: ["sales"])), ["invoices"])
    }

    func testAMySQLPathIsDatabaseThenTable() {
        let model = model()
        let connection = Connection(id: UUID(), name: "Reporting", color: .amber, kind: .mysql,
                                    host: "mysql.internal", port: 3306, sslmode: "disable",
                                    user: "analyst", database: "reporting", schema: "", verify: true)
        model.connections = [connection]
        model.rebuildTree()
        let root = model.tree[0]
        let reporting = TreeNode.database("reporting", parent: root)
        reporting.children = [TreeNode.table("kpm", parent: reporting)]
        root.children = [reporting]
        let tab = tab(model, connection: connection)

        XCTAssertEqual(texts(model.suggestions(for: tab, prefix: "", path: ["reporting"])), ["kpm"])
    }

    func testAPathDoesNotCrossConnections() {
        let model = model()
        let (connection, _, _, _) = seed(model)
        // A second Trino connection with its own catalog of the same name and a different table.
        let other = Connection(id: UUID(), name: "Other", color: .green, kind: .trino,
                               host: "other.internal", port: 8443, sslmode: "prefer",
                               user: "queryhive", database: "hive", schema: "analytics", verify: true)
        model.connections = [connection, other]
        model.rebuildTree()
        let otherRoot = model.tree.first { $0.connectionID == other.id }!
        let otherHive = TreeNode.catalog("hive", parent: otherRoot)
        let otherAnalytics = TreeNode.schema("analytics", parent: otherHive)
        otherAnalytics.children = [TreeNode.table("other_table", parent: otherAnalytics)]
        otherHive.children = [otherAnalytics]
        otherRoot.children = [otherHive]

        let tab = tab(model, connection: connection)
        let offered = texts(model.suggestions(for: tab, prefix: "", path: ["hive", "analytics"]))
        XCTAssertFalse(offered.contains("other_table"),
                       "the query runs against one connection, so another's objects are not candidates")
    }

    // MARK: Loading on demand

    func testAnUnloadedPathAsksForItsChildren() {
        let model = model()
        let connection = Connection(id: UUID(), name: "Warehouse", color: .violet, kind: .trino,
                                    host: "trino.internal", port: 8443, sslmode: "prefer",
                                    user: "queryhive", database: "hive", schema: "analytics", verify: true)
        model.connections = [connection]
        model.rebuildTree()
        let root = model.tree[0]
        let hive = TreeNode.catalog("hive", parent: root)
        root.children = [hive]          // schemas never loaded
        let tab = tab(model, connection: connection)

        // Nothing to offer yet, and that is correct — but the node must have been asked, or typing
        // `hive.` would never fill in.
        XCTAssertTrue(model.suggestions(for: tab, prefix: "", path: ["hive"]).isEmpty)
        XCTAssertTrue(hive.loading, "an unloaded node was not asked for its children")
    }
}

// MARK: Parsing

/// The qualifier parser, tested on its own.
///
/// This is the part that decides which schema an answer may draw from, and it fails silently: a
/// wrong parse still produces a plausible list. So it gets its own cases, including the shapes the
/// editor can actually be in at a caret — mid-word, after a dot, after a quoted name.
final class WordScopeTests: XCTestCase {
    private func parse(_ text: String, caret: Int? = nil) -> WordScope.Parsed? {
        let ns = text as NSString
        return WordScope.parse(ns, upTo: caret ?? ns.length)
    }

    func testABareWordHasNoPath() {
        let parsed = parse("SELECT pen")
        XCTAssertEqual(parsed?.prefix, "pen")
        XCTAssertEqual(parsed?.path, [])
    }

    func testACatalogAndSchemaBecomeThePath() {
        let parsed = parse("SELECT * FROM hive.analytics.")
        XCTAssertEqual(parsed?.prefix, "")
        XCTAssertEqual(parsed?.path, ["hive", "analytics"])
    }

    func testAPartialWordKeepsThePath() {
        let parsed = parse("SELECT * FROM hive.analytics.pen")
        XCTAssertEqual(parsed?.prefix, "pen")
        XCTAssertEqual(parsed?.path, ["hive", "analytics"])
    }

    func testASingleSegmentIsAPathOfOne() {
        let parsed = parse("SELECT * FROM hive.")
        XCTAssertEqual(parsed?.path, ["hive"])
    }

    func testThreeSegmentsAreKeptInOrder() {
        let parsed = parse("a.b.c.")
        XCTAssertEqual(parsed?.path, ["a", "b", "c"])
    }

    func testAQuotedIdentifierIsOneSegmentEvenWithADotInIt() {
        // The server reads `"a.b"` as a single name, so the parser must not split it.
        let parsed = parse(#"SELECT * FROM hive."analytics.v2".x"#)
        XCTAssertEqual(parsed?.path, ["hive", "analytics.v2"])
        XCTAssertEqual(parsed?.prefix, "x")
    }

    func testAQuotedIdentifierWithASpaceIsOneSegment() {
        let parsed = parse("SELECT * FROM hive.\"my schema\".t")
        XCTAssertEqual(parsed?.path, ["hive", "my schema"])
        XCTAssertEqual(parsed?.prefix, "t")
    }

    func testABacktickNameIsAlsoOneSegment() {
        let parsed = parse("SELECT * FROM `my db`.")
        XCTAssertEqual(parsed?.path, ["my db"])
    }

    func testALeadingDotDoesNotInventASegment() {
        let parsed = parse("SELECT * FROM .")
        XCTAssertEqual(parsed?.path, [])
    }

    func testAWordAfterAnOperatorHasNoPath() {
        // `-` and `+` end a qualifier; a path must not run through them.
        let parsed = parse("SELECT 1 + pen")
        XCTAssertEqual(parsed?.path, [])
        XCTAssertEqual(parsed?.prefix, "pen")
    }

    func testANewlineEndsThePath() {
        let parsed = parse("SELECT *\nFROM hive.")
        XCTAssertEqual(parsed?.path, ["hive"])
    }

    func testTheParseSeesOnlyUpToTheCaret() {
        // Text after the caret must not be read as part of the word being completed. Caret 5 is
        // just past "hive.", so the rest of the line does not exist as far as this is concerned.
        let parsed = parse("hive.analytics.xyz", caret: 5)
        XCTAssertEqual(parsed?.path, ["hive"])
        XCTAssertEqual(parsed?.prefix, "")
    }

    func testAnIdentifierWithAnUnderscoreAndDigitsIsOneWord() {
        let parsed = parse("SELECT * FROM hive.analytics.penerima_manfaat_2")
        XCTAssertEqual(parsed?.prefix, "penerima_manfaat_2")
        XCTAssertEqual(parsed?.path, ["hive", "analytics"])
    }
}
