import QueryHiveFFI
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
        let model = AppModel()
        // A bare word asks for its connection's default scope, and a path for an unloaded node. The
        // answers are empty and arrive a turn later, so nothing here reaches a real engine and a
        // test that checks `loading` right after the call still sees it.
        let engine = MockEngine()
        for command in ["catalogs", "schemas", "tables"] { engine.answer(command, with: .events([])) }
        model.engine = engine
        return model
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

// MARK: Position-dependent suggestions (W11-T6b)

extension SuggestionScopeTests {
    private func scope(_ clause: EditorClause, _ aliases: [String] = []) -> EditorCaretScope {
        EditorCaretScope(clause: clause, selectAliases: aliases)
    }

    private func relation(_ name: String, alias: String? = nil) -> EditorRelation {
        EditorRelation(kind: .table, catalog: nil, schema: nil, name: name, quoted: false,
                       alias: alias, nameStartUtf16: 0, nameLenUtf16: 0)
    }

    private func references(_ relations: [EditorRelation] = [],
                            ctes: [EditorCte] = []) -> EditorReferences {
        EditorReferences(revision: 1, statementStartUtf16: 0, statementLenUtf16: 0,
                         relations: relations, ctes: ctes, source: .tree)
    }

    /// A tab on a connection of `kind`, with no tree: enough for what only the driver decides.
    private func driverTab(_ model: AppModel, kind: ConnectionKind) -> QueryTab {
        let connection = Connection(id: UUID(), name: "c", color: .violet, kind: kind,
                                    host: "h.internal", port: 1, sslmode: "prefer",
                                    user: "u", database: "d", schema: "s", verify: true)
        model.connections = [connection]
        model.rebuildTree()
        return tab(model, connection: connection)
    }

    // MARK: Aliases on tables

    func testATableInAFromListCarriesAFreeAlias() {
        let model = model()
        let (connection, _, _, _) = seed(model)
        let tab = tab(model, connection: connection)

        let offered = texts(model.suggestions(for: tab, prefix: "", path: ["hive", "analytics"],
                                              scope: scope(.queryTable), references: references()))
        XCTAssertEqual(offered.sorted(), ["penerima_manfaat pm", "wilayah w"])
        // A bare word reaches every table in the tree, and gets the same treatment.
        let bare = texts(model.suggestions(for: tab, prefix: "wil", path: [],
                                           scope: scope(.queryTable), references: references()))
        XCTAssertEqual(bare, ["wilayah w"])
    }

    func testAnAliasThatTheStatementAlreadyUsesIsNotMadeAgain() {
        let model = model()
        let (connection, _, _, _) = seed(model)
        let tab = tab(model, connection: connection)

        // `w` is taken by an alias, and `wi` is a table name of the statement.
        let taken = references([relation("orders", alias: "w"), relation("wi")])
        let offered = texts(model.suggestions(for: tab, prefix: "wil", path: ["hive", "analytics"],
                                              scope: scope(.queryTable), references: taken))
        XCTAssertEqual(offered, ["wilayah wil"])
        // A CTE name is taken, too.
        let cte = references(ctes: [EditorCte(name: "w", columns: [])])
        XCTAssertEqual(texts(model.suggestions(for: tab, prefix: "wil", path: ["hive", "analytics"],
                                               scope: scope(.queryTable), references: cte)),
                       ["wilayah wi"])
    }

    func testNoAliasWhereTheStatementWritesOrIsNotKnown() {
        let model = model()
        let (connection, _, _, _) = seed(model)
        let tab = tab(model, connection: connection)
        let path = ["hive", "analytics"]

        // A table named by UPDATE, DELETE FROM, INSERT INTO or DDL is written bare.
        for clause in [EditorClause.writeTable, .other, .where, .joinCondition, .selectList] {
            XCTAssertEqual(texts(model.suggestions(for: tab, prefix: "wil", path: path,
                                                   scope: scope(clause), references: references())),
                           ["wilayah"], "\(clause)")
        }
        // No scope, or no analysis to know what is taken: bare, as before this existed.
        XCTAssertEqual(texts(model.suggestions(for: tab, prefix: "wil", path: path)), ["wilayah"])
        XCTAssertEqual(texts(model.suggestions(for: tab, prefix: "wil", path: path,
                                               scope: scope(.queryTable))), ["wilayah"])
        XCTAssertEqual(texts(model.suggestions(for: tab, prefix: "wil", path: path,
                                               references: references())), ["wilayah"])
    }

    func testSchemasAndCatalogsNeverGetAnAlias() {
        let model = model()
        let (connection, _, _, _) = seed(model)
        let tab = tab(model, connection: connection)

        let suggestions = model.suggestions(for: tab, prefix: "", path: ["hive"],
                                            scope: scope(.queryTable), references: references())
        XCTAssertEqual(texts(suggestions).sorted(), ["analytics", "bronze"])
    }

    // MARK: Select aliases per dialect

    func testSelectAliasesAreOfferedWhereTheDriverAcceptsThem() {
        let model = model()
        let cases: [(ConnectionKind, EditorClause, Bool)] = [
            (.postgres, .orderBy, true), (.mysql, .orderBy, true), (.trino, .orderBy, true),
            (.postgres, .groupBy, true), (.mysql, .groupBy, true), (.trino, .groupBy, false),
            (.postgres, .having, false), (.trino, .having, false), (.mysql, .having, true),
            (.postgres, .where, false), (.mysql, .selectList, false), (.mysql, .queryTable, false),
        ]
        for (kind, clause, expected) in cases {
            let tab = driverTab(model, kind: kind)
            let offered = texts(model.suggestions(for: tab, prefix: "tot", path: [],
                                                  scope: scope(clause, ["total", "region"])))
            XCTAssertEqual(offered.contains("total"), expected, "\(kind) \(clause)")
        }
    }

    func testAnAliasIsNotOfferedAsAColumnWhereTheDriverRefusesIt() {
        let model = model()
        let tab = driverTab(model, kind: .postgres)
        tab.columns = [Event.Column(name: "total", type: "bigint"),
                       Event.Column(name: "region", type: "varchar")]

        // `total` is an output name of this statement, so HAVING on PostgreSQL does not take it;
        // `region` is only a column and stays.
        let having = texts(model.suggestions(for: tab, prefix: "", path: [],
                                             scope: scope(.having, ["total"])))
        XCTAssertFalse(having.contains("total"))
        XCTAssertTrue(having.contains("region"))
        // Where the driver accepts it, it is offered once.
        let order = texts(model.suggestions(for: tab, prefix: "", path: [],
                                            scope: scope(.orderBy, ["total"])))
        XCTAssertEqual(order.filter { $0 == "total" }.count, 1)
        // Without a scope nothing changes.
        XCTAssertTrue(texts(model.suggestions(for: tab, prefix: "", path: [])).contains("total"))
    }

    func testAQuotedAliasIsMatchedByItsNameAndWrittenAsSpelled() {
        let model = model()
        let tab = driverTab(model, kind: .postgres)
        let offered = model.suggestions(for: tab, prefix: "n", path: [],
                                        scope: scope(.orderBy, ["\"Net\"", "`odd`"]))
        XCTAssertTrue(offered.contains { $0.text == "\"Net\"" && $0.kind == .column })
        XCTAssertFalse(offered.contains { $0.text == "`odd`" })
    }

    func testNoAliasesUnderAPath() {
        let model = model()
        let tab = driverTab(model, kind: .postgres)
        XCTAssertTrue(model.suggestions(for: tab, prefix: "", path: ["s"],
                                        scope: scope(.orderBy, ["total"])).isEmpty)
    }

    // MARK: Invalidation

    func testInvalidatingATableDropsItsListingSchemaButNotItsNeighbours() {
        let model = model()
        let (connection, hive, analytics, bronze) = seed(model)
        let tab = tab(model, connection: connection)

        model.invalidateCompletionCatalog(for: tab, object: ["hive", "analytics", "wilayah"])
        XCTAssertNil(analytics.children, "the list holding the table was kept")
        XCTAssertNotNil(bronze.children, "a neighbouring schema was dropped")
        XCTAssertNotNil(hive.children, "the catalog above was dropped")
    }

    func testAnUnqualifiedNameTakesTheTabsDefaults() {
        let model = model()
        let (connection, _, analytics, bronze) = seed(model)
        let tab = tab(model, connection: connection)

        // The connection's default is hive.analytics.
        model.invalidateCompletionCatalog(for: tab, object: ["wilayah"])
        XCTAssertNil(analytics.children)
        XCTAssertNotNil(bronze.children)
        // The tab's own context wins over the connection's.
        analytics.children = [TreeNode.table("wilayah", parent: analytics)]
        tab.contextSchema = "bronze"
        model.invalidateCompletionCatalog(for: tab, object: ["raw_kpm"])
        XCTAssertNil(bronze.children)
        XCTAssertNotNil(analytics.children)
    }

    func testANewObjectInvalidatesTheListThatWillHoldIt() {
        let model = model()
        let (connection, hive, analytics, _) = seed(model)
        let tab = tab(model, connection: connection)

        // `CREATE TABLE hive.analytics.fresh`: no node yet, but the schema's list is stale.
        model.invalidateCompletionCatalog(for: tab, object: ["hive", "analytics", "fresh"])
        XCTAssertNil(analytics.children)
        // `CREATE SCHEMA hive.silver`: the catalog's list of schemas.
        analytics.children = [TreeNode.table("wilayah", parent: analytics)]
        model.invalidateCompletionCatalog(for: tab, object: ["hive", "silver"], kind: .schema)
        XCTAssertNil(hive.children)
        XCTAssertNotNil(analytics.children)
        // A schema's own tables are dropped with it: `DROP SCHEMA hive.analytics`.
        hive.children = [analytics]
        model.invalidateCompletionCatalog(for: tab, object: ["analytics"], kind: .schema)
        XCTAssertNil(analytics.children)
        XCTAssertNil(hive.children)
        // A level the driver does not have names nothing: Trino has no database level.
        analytics.children = [TreeNode.table("wilayah", parent: analytics)]
        model.invalidateCompletionCatalog(for: tab, object: ["hive"], kind: .database)
        XCTAssertNotNil(analytics.children)
    }

    func testANodeTheSideBarShowsOpenIsLeftToTheTreeRefresh() {
        let model = model()
        let (connection, _, analytics, _) = seed(model)
        let tab = tab(model, connection: connection)
        analytics.expanded = true

        model.invalidateCompletionCatalog(for: tab, object: ["hive", "analytics", "wilayah"])
        XCTAssertNotNil(analytics.children, "blanked rows the user is looking at")
    }

    func testANameThatCannotBeResolvedDropsNothing() {
        let model = model()
        let (connection, hive, analytics, bronze) = seed(model)
        let tab = tab(model, connection: connection)
        func everythingKept() -> Bool {
            hive.children != nil && analytics.children != nil && bronze.children != nil
        }

        model.invalidateCompletionCatalog(for: tab, object: [])
        model.invalidateCompletionCatalog(for: tab, object: ["a", "b", "c", "d"])
        model.invalidateCompletionCatalog(for: tab, object: ["nosuch", "nosuch", "t"])
        XCTAssertTrue(everythingKept())
        // No tab connection at all.
        model.invalidateCompletionCatalog(for: QueryTab(title: "none"), object: ["hive", "analytics", "t"])
        XCTAssertTrue(everythingKept())
    }

    func testPostgresAndMySQLNamesAreQualifiedByTheirOwnLevels() {
        let model = model()
        let pg = Connection(id: UUID(), name: "pg", color: .violet, kind: .postgres,
                            host: "pg.internal", port: 5432, sslmode: "prefer",
                            user: "u", database: "warehouse", schema: "public", verify: true)
        let my = Connection(id: UUID(), name: "my", color: .amber, kind: .mysql,
                            host: "my.internal", port: 3306, sslmode: "disable",
                            user: "u", database: "reporting", schema: "", verify: true)
        model.connections = [pg, my]
        model.rebuildTree()
        let pgRoot = model.tree.first { $0.connectionID == pg.id }!
        let publicSchema = TreeNode.schema("public", parent: pgRoot)
        publicSchema.children = [TreeNode.table("orders", parent: publicSchema)]
        pgRoot.children = [publicSchema]
        let myRoot = model.tree.first { $0.connectionID == my.id }!
        let reporting = TreeNode.database("reporting", parent: myRoot)
        reporting.children = [TreeNode.table("kpm", parent: reporting)]
        myRoot.children = [reporting]

        model.invalidateCompletionCatalog(for: tab(model, connection: pg), object: ["orders"])
        XCTAssertNil(publicSchema.children)
        model.invalidateCompletionCatalog(for: tab(model, connection: my), object: ["kpm"])
        XCTAssertNil(reporting.children)
    }

    func testAfterInvalidationTheNextSuggestionAsksTheEngineAgain() {
        let model = model()
        let (connection, _, analytics, _) = seed(model)
        let tab = tab(model, connection: connection)
        XCTAssertEqual(texts(model.suggestions(for: tab, prefix: "wil", path: ["hive", "analytics"])),
                       ["wilayah"])

        model.invalidateCompletionCatalog(for: tab, object: ["hive", "analytics", "wilayah"])
        XCTAssertTrue(model.suggestions(for: tab, prefix: "wil", path: ["hive", "analytics"]).isEmpty)
        XCTAssertTrue(analytics.loading, "the schema was not asked for its tables again")
    }
}

/// The policy behind the suggestions, and the Rust functions it stands on.
final class SuggestionPolicyTests: XCTestCase {
    func testTheBindingsReadTheStatementAtTheCaret() throws {
        let sql = "select * from public.t as a join (select 1) q on true"
        let document = try EditorDocument(text: sql, dialect: .postgres)
        let references = try document.references(revision: try document.revision(), offsetUtf16: 3)
        XCTAssertEqual(references.source, .tree)
        XCTAssertEqual(references.relations.map(\.alias), ["a", "q"])
        XCTAssertEqual(references.relations.map(\.kind), [.table, .subquery])
        XCTAssertEqual(references.relations[0].schema, "public")
        XCTAssertEqual(references.relations[0].name, "t")
        XCTAssertEqual(SQLSuggestions.takenNames(references), ["a", "t", "q"])
    }

    func testTheBindingsRefuseAStaleRevision() throws {
        let document = try EditorDocument(text: "select 1", dialect: .generic)
        let old = try document.revision()
        _ = try document.replace(startUtf16: 0, lenUtf16: 0, text: " ")
        XCTAssertThrowsError(try document.references(revision: old, offsetUtf16: 0)) {
            XCTAssertEqual($0 as? EditorError, EditorError.Stale)
        }
    }

    func testTheCaretScopeIsReadFromTheTextBeforeIt() {
        XCTAssertEqual(sqlCaretScope(head: "select * from ", dialect: .postgres).clause, .queryTable)
        XCTAssertEqual(sqlCaretScope(head: "update ", dialect: .mysql).clause, .writeTable)
        XCTAssertEqual(sqlCaretScope(head: "delete from ", dialect: .trino).clause, .writeTable)
        let scope = sqlCaretScope(head: "select count(*) as n from t group by a having ",
                                  dialect: .postgres)
        XCTAssertEqual(scope.clause, .having)
        XCTAssertEqual(scope.selectAliases, ["n"])
    }

    func testAnAliasIsFreeAndNeverAKeyword() {
        XCTAssertEqual(sqlTableAlias(table: "order_items", taken: []), "oi")
        XCTAssertEqual(sqlTableAlias(table: "users", taken: ["u"]), "us")
        // `or` is a keyword, `o` is taken: the next form is used.
        XCTAssertEqual(sqlTableAlias(table: "orders", taken: ["o"]), "ord")
    }

    func testTableTextIsTheBareNameUnlessAFromListOfAKnownStatement() {
        let known = EditorReferences(revision: 1, statementStartUtf16: 0, statementLenUtf16: 0,
                                     relations: [], ctes: [], source: .tree)
        XCTAssertEqual(SQLSuggestions.tableText("users", in: .queryTable, references: known), "users u")
        XCTAssertEqual(SQLSuggestions.tableText("users", in: .writeTable, references: known), "users")
        XCTAssertEqual(SQLSuggestions.tableText("users", in: nil, references: known), "users")
        XCTAssertEqual(SQLSuggestions.tableText("users", in: .queryTable, references: nil), "users")
    }

    func testUnquotingStripsOnePairOfMatchingQuotes() {
        XCTAssertEqual(SQLSuggestions.unquoted("\"Net\""), "Net")
        XCTAssertEqual(SQLSuggestions.unquoted("`odd`"), "odd")
        XCTAssertEqual(SQLSuggestions.unquoted("plain"), "plain")
        XCTAssertEqual(SQLSuggestions.unquoted("\"half"), "\"half")
        XCTAssertEqual(SQLSuggestions.unquoted("\""), "\"")
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
