import AppKit
import QueryHiveFFI
import XCTest

@testable import QueryHive

/// Columns in the editor's suggestions: `alias.` and `table.`, a bare column in a clause that takes
/// one, and the on-demand read that fills them in.
///
/// Everything here runs against `MockEngine`: the real read is a `preview` of `SELECT *` under a
/// one-row cap, and these tests pin what is asked (the command, the statement and the scope it is
/// asked in), that it is asked once, and what is done with the answer. No server is involved, so
/// whether a live Trino or PostgreSQL answers the same way is not shown by them.
@MainActor
final class SuggestionColumnsTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    // MARK: Fixtures

    private struct Fixture {
        let model: AppModel
        let engine: MockEngine
        let tab: QueryTab
        let analytics: TreeNode
    }

    private func fixture(failing: Bool = false) -> Fixture {
        let engine = MockEngine()
        engine.answer("catalogs", with: .events([]))
        engine.answer("schemas", with: .events([]))
        engine.answer("tables", with: .events([]))
        if !failing {
            // One scripted answer per command: the mock does not look at the statement, so every
            // table read answers with these two columns.
            engine.answer("preview", with: .events([
                Event(event: "columns", columns: ["kode_wilayah", "nama"].map { Event.Column(name: $0, type: "varchar") }),
            ]))
        } else {
            engine.answer("preview", with: .failure(reason: "permission denied"))
        }
        let model = AppModel()
        model.engine = engine
        let connection = Connection(id: UUID(), name: "Warehouse", color: .violet, kind: .trino,
                                    host: "trino.internal", port: 8443, sslmode: "prefer",
                                    user: "queryhive", database: "hive", schema: "analytics", verify: true)
        model.connections = [connection]
        model.rebuildTree()
        let root = model.tree[0]
        let hive = TreeNode.catalog("hive", parent: root)
        let analytics = TreeNode.schema("analytics", parent: hive)
        let bronze = TreeNode.schema("bronze", parent: hive)
        analytics.children = ["wilayah", "penerima_manfaat", "program"].map { TreeNode.table($0, parent: analytics) }
        bronze.children = [TreeNode.table("raw_kpm", parent: bronze)]
        hive.children = [analytics, bronze]
        root.children = [hive]
        let tab = QueryTab(title: "q")
        tab.connectionID = connection.id
        model.tabs = [tab]
        model.selectedTabID = tab.id
        return Fixture(model: model, engine: engine, tab: tab, analytics: analytics)
    }

    private func pump(_ seconds: TimeInterval = 0.15) {
        RunLoop.main.run(until: Date().addingTimeInterval(seconds))
    }

    private func relation(_ name: String?, alias: String? = nil, kind: EditorRelationKind = .table,
                          schema: String? = nil, catalog: String? = nil) -> EditorRelation {
        EditorRelation(kind: kind, catalog: catalog, schema: schema, name: name, quoted: false,
                       alias: alias, nameStartUtf16: 0, nameLenUtf16: 0)
    }

    private func references(_ relations: [EditorRelation], ctes: [EditorCte] = []) -> EditorReferences {
        EditorReferences(revision: 1, statementStartUtf16: 0, statementLenUtf16: 0,
                         relations: relations, ctes: ctes, source: .tree)
    }

    private func scope(_ clause: EditorClause) -> EditorCaretScope {
        EditorCaretScope(clause: clause, selectAliases: [])
    }

    private func columns(_ items: [SQLSuggestion]) -> [String] {
        items.filter { $0.kind == .column }.map(\.text)
    }

    // MARK: alias. and table.

    func testAnAliasOffersItsTablesColumnsAfterOneRead() throws {
        let f = fixture()
        let refs = references([relation("wilayah", alias: "w")])

        XCTAssertTrue(columns(f.model.suggestions(for: f.tab, prefix: "", path: ["w"],
                                                  scope: scope(.selectList), references: refs)).isEmpty,
                      "columns that were not read yet must not be guessed")
        pump()
        XCTAssertEqual(columns(f.model.suggestions(for: f.tab, prefix: "", path: ["w"],
                                                   scope: scope(.selectList), references: refs)),
                       ["nama", "kode_wilayah"])

        let reads = f.engine.calls.filter { $0.command == "preview" }
        XCTAssertEqual(reads.count, 1, "the read was repeated")
        let env = try XCTUnwrap(reads.first).env
        XCTAssertEqual(env["SQL"], "SELECT * FROM \"hive\".\"analytics\".\"wilayah\"")
        XCTAssertEqual(env["LIMIT"], "1")
        XCTAssertEqual(env["DB_DATABASE"], "hive")
        XCTAssertEqual(env["DB_SCHEMA"], "analytics")
    }

    func testARequestInFlightIsNotSentTwice() {
        let f = fixture()
        let refs = references([relation("wilayah", alias: "w")])
        for _ in 0..<5 {
            _ = f.model.suggestions(for: f.tab, prefix: "", path: ["w"], scope: scope(.selectList), references: refs)
        }
        pump()
        XCTAssertEqual(f.engine.calls.filter { $0.command == "preview" }.count, 1)
    }

    func testAFailedReadIsNotRepeatedOnEveryKeystroke() {
        let f = fixture(failing: true)
        let refs = references([relation("wilayah", alias: "w")])
        for _ in 0..<4 {
            _ = f.model.suggestions(for: f.tab, prefix: "", path: ["w"], scope: scope(.selectList), references: refs)
            pump()
        }
        XCTAssertEqual(f.engine.calls.filter { $0.command == "preview" }.count, 1)
        XCTAssertNotNil(f.analytics.children?.first { $0.title == "wilayah" }?.columnsError)
    }

    func testATableNameStandsForItselfWhenItHasNoAlias() {
        let f = fixture()
        let refs = references([relation("wilayah")])
        _ = f.model.suggestions(for: f.tab, prefix: "", path: ["wilayah"], scope: scope(.where), references: refs)
        pump()
        XCTAssertEqual(columns(f.model.suggestions(for: f.tab, prefix: "na", path: ["WILAYAH"],
                                                   scope: scope(.where), references: refs)), ["nama"])
    }

    func testAnAliasHidesTheTableNameBehindIt() {
        let f = fixture()
        let refs = references([relation("wilayah", alias: "w")])
        // `wilayah.` is not valid once the table is aliased `w`: no columns, and no read either.
        XCTAssertTrue(f.model.suggestions(for: f.tab, prefix: "", path: ["wilayah"],
                                          scope: scope(.where), references: refs).isEmpty)
        XCTAssertTrue(f.engine.calls.filter { $0.command == "preview" }.isEmpty)
    }

    func testAnAmbiguousAliasOffersNothing() {
        let f = fixture()
        let refs = references([relation("wilayah", alias: "t"), relation("program", alias: "t")])
        XCTAssertTrue(f.model.suggestions(for: f.tab, prefix: "", path: ["t"],
                                          scope: scope(.where), references: refs).isEmpty)
        XCTAssertTrue(f.engine.calls.filter { $0.command == "preview" }.isEmpty,
                      "a read was sent for a name that cannot be resolved")
    }

    func testASubqueryAndACteWithoutAListOfferNoColumns() {
        let f = fixture()
        let refs = references([relation(nil, alias: "q", kind: .subquery),
                               relation("recent", alias: "r", kind: .cte)],
                              ctes: [EditorCte(name: "recent", columns: [])])
        XCTAssertTrue(f.model.suggestions(for: f.tab, prefix: "", path: ["q"],
                                          scope: scope(.where), references: refs).isEmpty)
        XCTAssertTrue(f.model.suggestions(for: f.tab, prefix: "", path: ["r"],
                                          scope: scope(.where), references: refs).isEmpty)
        XCTAssertTrue(f.engine.calls.isEmpty, "nothing in the tree was asked for")
    }

    func testACteWithADeclaredListOffersThatList() {
        let f = fixture()
        let refs = references([relation("recent", alias: "r", kind: .cte)],
                              ctes: [EditorCte(name: "recent", columns: ["id", "total"])])
        XCTAssertEqual(columns(f.model.suggestions(for: f.tab, prefix: "", path: ["r"],
                                                   scope: scope(.where), references: refs)), ["id", "total"])
    }

    func testWithoutTheStatementsAnalysisAnAliasOffersNothing() {
        let f = fixture()
        XCTAssertTrue(f.model.suggestions(for: f.tab, prefix: "", path: ["w"], scope: scope(.where)).isEmpty)
    }

    func testAQualifiedTableOffersItsColumnsWithoutTheStatement() {
        let f = fixture()
        let path = ["hive", "analytics", "wilayah"]
        XCTAssertTrue(f.model.suggestions(for: f.tab, prefix: "", path: path).isEmpty)
        pump()
        XCTAssertEqual(Set(columns(f.model.suggestions(for: f.tab, prefix: "", path: path))),
                       ["kode_wilayah", "nama"])
    }

    func testARelationInAnotherSchemaIsReadWithItsOwnScope() throws {
        let f = fixture()
        let refs = references([relation("raw_kpm", alias: "k", schema: "bronze")])
        _ = f.model.suggestions(for: f.tab, prefix: "", path: ["k"], scope: scope(.where), references: refs)
        let env = try XCTUnwrap(f.engine.calls.first { $0.command == "preview" }).env
        XCTAssertEqual(env["SQL"], "SELECT * FROM \"hive\".\"bronze\".\"raw_kpm\"")
        XCTAssertEqual(env["DB_SCHEMA"], "bronze")
    }

    // MARK: A bare column

    func testABareColumnRanksTheStatementsRelationsBeforeTheLastRun() {
        let f = fixture()
        f.tab.columns = [Event.Column(name: "nama_lama", type: "varchar"),
                         Event.Column(name: "nama", type: "varchar")]
        let refs = references([relation("wilayah", alias: "w")])
        _ = f.model.suggestions(for: f.tab, prefix: "na", path: [], scope: scope(.selectList), references: refs)
        pump()
        let found = f.model.suggestions(for: f.tab, prefix: "na", path: [], scope: scope(.selectList),
                                        references: refs)
        XCTAssertEqual(columns(found), ["nama", "nama_lama"],
                       "the relation's column first, the last run's after it, and `nama` once")
    }

    func testAtMostThreeReadsAreInFlight() {
        let f = fixture()
        let refs = references((0..<6).map { relation("t\($0)", alias: "a\($0)") })
        for index in 0..<6 {
            f.analytics.children?.append(TreeNode.table("t\(index)", parent: f.analytics))
        }
        _ = f.model.suggestions(for: f.tab, prefix: "", path: [], scope: scope(.selectList), references: refs)
        XCTAssertEqual(f.engine.calls.filter { $0.command == "preview" }.count, 3)
        pump()
        _ = f.model.suggestions(for: f.tab, prefix: "", path: [], scope: scope(.selectList), references: refs)
        XCTAssertEqual(f.engine.calls.filter { $0.command == "preview" }.count, 6, "the rest follow, three at a time")
    }

    func testNoColumnsWhereATableIsBeingNamed() {
        let f = fixture()
        f.tab.columns = [Event.Column(name: "nama", type: "varchar")]
        let found = f.model.suggestions(for: f.tab, prefix: "na", path: [], scope: scope(.queryTable),
                                        references: references([]))
        XCTAssertTrue(found.filter { $0.kind == .column }.isEmpty)
    }

    func testTheStatementsCtesAreTablesInAFromList() {
        let f = fixture()
        let refs = references([], ctes: [EditorCte(name: "recent_rows", columns: [])])
        let found = f.model.suggestions(for: f.tab, prefix: "rec", path: [], scope: scope(.queryTable),
                                        references: refs)
        XCTAssertEqual(found.first?.kind, .table)
        XCTAssertTrue(found.first?.text.hasPrefix("recent_rows") == true)
    }

    // MARK: Tables outside the default scope

    func testATableOutsideTheDefaultSchemaIsWrittenWithTheQualifierItNeeds() {
        let f = fixture()
        let found = f.model.suggestions(for: f.tab, prefix: "raw", path: [], scope: scope(.queryTable))
        XCTAssertEqual(found.first?.text, "bronze.raw_kpm",
                       "the tab's schema is hive.analytics, so a bare `raw_kpm` would not resolve")
        // And one in the default schema stays bare, and ranks above one that is away from home.
        let both = f.model.suggestions(for: f.tab, prefix: "", path: [], scope: scope(.queryTable))
            .filter { $0.kind == .table }.map(\.text)
        XCTAssertEqual(both.last, "bronze.raw_kpm")
    }

    // MARK: Invalidation

    func testAReplyThatArrivesAfterInvalidationIsIgnored() throws {
        let f = fixture()
        let refs = references([relation("wilayah", alias: "w")])
        _ = f.model.suggestions(for: f.tab, prefix: "", path: ["w"], scope: scope(.where), references: refs)
        let table = try XCTUnwrap(f.analytics.children?.first { $0.title == "wilayah" })
        XCTAssertTrue(table.columnsLoading)

        f.model.invalidateCompletionCatalog(for: f.tab, object: ["wilayah"])
        pump()
        XCTAssertNil(table.columns, "a reply for the table as it was before the DDL filled it in")
        XCTAssertFalse(table.columnsLoading)
        XCTAssertEqual(f.model.completionColumnReads, 0, "the in-flight count leaked")
    }

    func testAnInvalidatedTableIsReadAgain() throws {
        let f = fixture()
        let refs = references([relation("wilayah", alias: "w")])
        _ = f.model.suggestions(for: f.tab, prefix: "", path: ["w"], scope: scope(.where), references: refs)
        pump()
        let table = try XCTUnwrap(f.analytics.children?.first { $0.title == "wilayah" })
        XCTAssertNotNil(table.columns)

        f.model.invalidateCompletionCatalog(for: f.tab, object: ["wilayah"])
        XCTAssertNil(table.columns)
        XCTAssertNil(f.analytics.children, "the schema's list of tables is stale too")
    }

    func testAReplyForARefreshedNodeIsIgnoredToo() {
        let f = fixture()
        let hive = f.model.tree[0].children![0]
        hive.children = nil
        f.model.requestLoad(of: hive)
        XCTAssertTrue(hive.loading)
        f.model.refresh(hive)   // the sidebar's Refresh, while the first ask is on its way
        pump()
        XCTAssertNil(hive.children, "a reply to an ask made before the refresh filled the node in")
        XCTAssertFalse(hive.loading)
    }

    // MARK: The editor's analysis

    func testTheAnalysisReadsTheStatementAtAnOffset() throws {
        let analysis = try EditorAnalysis(text: "select w.x from wilayah w join program p on true", dialect: .trino)
        let refs = try analysis.references(at: 3)
        XCTAssertEqual(refs.relations.map(\.alias), ["w", "p"])
        XCTAssertEqual(refs.relations.map(\.name), ["wilayah", "program"])
        XCTAssertEqual(refs.revision, analysis.revision)
    }

    func testTheAnalysisAnswersForTheRevisionItHoldsNow() throws {
        let analysis = try EditorAnalysis(text: "select 1", dialect: .trino)
        try analysis.replace(range: NSRange(location: 8, length: 0), with: " from wilayah w")
        let refs = try analysis.references(at: 0)
        XCTAssertEqual(refs.revision, analysis.revision)
        XCTAssertEqual(refs.relations.map(\.alias), ["w"])
    }

    func testTheAnalysisAnswersOffTheMainThread() throws {
        let analysis = try EditorAnalysis(text: "select * from wilayah w", dialect: .trino)
        let answered = expectation(description: "answered")
        analysis.requestReferences(at: 5) { result in
            XCTAssertTrue(Thread.isMainThread)
            XCTAssertEqual((try? result.get())?.relations.first?.alias, "w")
            answered.fulfill()
        }
        wait(for: [answered], timeout: 5)
    }
}
