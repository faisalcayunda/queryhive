import AppKit
import QueryHiveFFI
import XCTest

@testable import QueryHive

/// Why the suggestion list sometimes did not show up, one test per cause that was real.
///
/// Each test names the cause in its title. They were written against the code as it was, so they
/// failed before the fix and pass after it; the ones about trigger rules that did not exist as a
/// seam (`CompletionTriggerTests`) cover the new decision function instead.
@MainActor
final class SuggestionReachTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    private func trino(_ model: AppModel, name: String = "Warehouse") -> Connection {
        Connection(id: UUID(), name: name, color: .violet, kind: .trino, host: "trino.internal",
                   port: 8443, sslmode: "prefer", user: "queryhive", database: "hive",
                   schema: "analytics", verify: true)
    }

    private func tab(_ model: AppModel, on connection: Connection) -> QueryTab {
        let tab = QueryTab(title: "q")
        tab.connectionID = connection.id
        model.tabs = [tab]
        model.selectedTabID = tab.id
        return tab
    }

    private func texts(_ items: [SQLSuggestion]) -> [String] { items.map(\.text) }

    /// Lets the scripted engine's main-queue deliveries land.
    private func pump(_ seconds: TimeInterval = 0.15) {
        RunLoop.main.run(until: Date().addingTimeInterval(seconds))
    }

    private func names(_ names: [String]) -> Event {
        Event(event: "names", names: names)
    }

    // MARK: A bare word only saw what the sidebar had loaded

    func testABareWordLoadsTheTabsDefaultScopeInsteadOfWaitingForTheTree() {
        let engine = MockEngine()
        engine.answer("catalogs", with: .events([Event(event: "catalogs", names: ["hive"])]))
        engine.answer("schemas", with: .events([Event(event: "schemas", names: ["analytics"])]))
        engine.answer("tables", with: .events([Event(event: "tables", names: ["wilayah", "penerima_manfaat"])]))
        let model = AppModel()
        model.engine = engine
        let connection = trino(model)
        model.connections = [connection]
        model.rebuildTree()
        let tab = tab(model, on: connection)

        var loaded = 0
        model.completion.onCatalogLoaded = { loaded += 1 }

        // Nothing in the tree: the connection was never expanded. Each pass asks for the next
        // level of the default scope, hive.analytics, and the answer lands before the next one.
        for _ in 0..<4 {
            _ = model.suggestions(for: tab, prefix: "wi", path: [])
            pump()
        }
        XCTAssertEqual(engine.calls.map(\.command), ["catalogs", "schemas", "tables"])
        XCTAssertEqual(loaded, 3, "the editor was not told that a level had arrived")
        let tables = model.suggestions(for: tab, prefix: "wi", path: []).filter { $0.kind == .table }
        XCTAssertEqual(texts(tables), ["wilayah"],
                       "`FROM wi` offered nothing until the user expanded the tree by hand")
    }

    func testALoadThatFailedIsNotAskedForAgainOnEveryKeystroke() {
        let engine = MockEngine()
        engine.answer("schemas", with: .failure(reason: "no route to host"))
        let model = AppModel()
        model.engine = engine
        let connection = trino(model)
        model.connections = [connection]
        model.rebuildTree()
        let root = model.tree[0]
        let hive = TreeNode.catalog("hive", parent: root)
        root.children = [hive]
        let tab = tab(model, on: connection)

        for _ in 0..<4 {
            _ = model.suggestions(for: tab, prefix: "", path: ["hive"])
            pump()
        }
        XCTAssertEqual(engine.calls.filter { $0.command == "schemas" }.count, 1,
                       "a node that failed to load was retried on every keystroke")
    }

    // MARK: A bare word reached into other connections

    func testABareWordOnlySeesItsOwnConnection() {
        let model = AppModel()
        let mine = trino(model, name: "Mine")
        let theirs = trino(model, name: "Theirs")
        model.connections = [mine, theirs]
        model.rebuildTree()
        for (connection, table) in [(mine, "pen_mine"), (theirs, "pen_theirs")] {
            let root = model.tree.first { $0.connectionID == connection.id }!
            let hive = TreeNode.catalog("hive", parent: root)
            let analytics = TreeNode.schema("analytics", parent: hive)
            analytics.children = [TreeNode.table(table, parent: analytics)]
            hive.children = [analytics]
            root.children = [hive]
        }
        let tab = tab(model, on: mine)

        XCTAssertEqual(texts(model.suggestions(for: tab, prefix: "pen", path: [])), ["pen_mine"],
                       "the query runs against one connection, but another's table was offered")
    }

    // MARK: Matching and ranking

    private func seeded(tables: [String], columns: [String] = []) -> (AppModel, QueryTab) {
        let model = AppModel()
        let connection = trino(model)
        model.connections = [connection]
        model.rebuildTree()
        let root = model.tree[0]
        let hive = TreeNode.catalog("hive", parent: root)
        let analytics = TreeNode.schema("analytics", parent: hive)
        analytics.children = tables.map { TreeNode.table($0, parent: analytics) }
        hive.children = [analytics]
        root.children = [hive]
        let tab = tab(model, on: connection)
        tab.columns = columns.map { Event.Column(name: $0, type: "varchar") }
        return (model, tab)
    }

    func testAWordInsideTheNameFindsIt() {
        let (model, tab) = seeded(tables: ["penerima_manfaat", "wilayah", "rekap_kpm_harian"])
        XCTAssertTrue(texts(model.suggestions(for: tab, prefix: "manf", path: [])).contains("penerima_manfaat"),
                      "a name is found by the start of any of its words")
        XCTAssertTrue(texts(model.suggestions(for: tab, prefix: "kpm", path: [])).contains("rekap_kpm_harian"))
        // A prefix still beats a word in the middle.
        let ranked = texts(model.suggestions(for: tab, prefix: "wil", path: []))
        XCTAssertEqual(ranked.first, "wilayah")
    }

    func testATableAndAColumnOfTheSameNameAreBothOffered() {
        let (model, tab) = seeded(tables: ["orders"], columns: ["orders"])
        let found = model.suggestions(for: tab, prefix: "ord", path: []).filter { $0.kind != .keyword }
        XCTAssertEqual(Set(found.map(\.kind)), [.table, .column], "one hid the other")
    }

    func testMoreThanEightMatchesAreKept() {
        let tables = (1...12).map { String(format: "t%02d", $0) }
        let (model, tab) = seeded(tables: tables)
        XCTAssertEqual(model.suggestions(for: tab, prefix: "t", path: []).filter { $0.kind == .table }.count, 12,
                       "the list was cut to eight before the keywords could show")
    }

    func testAColumnClauseRanksColumnsAndKeywordsAboveTables() {
        let (model, tab) = seeded(tables: ["extra_rows"], columns: ["exposure"])
        let scope = EditorCaretScope(clause: .where, selectAliases: [])
        let found = model.suggestions(for: tab, prefix: "ex", path: [], scope: scope)
        // EXCEPT, EXISTS and EXPLAIN are the keywords that start with "ex".
        XCTAssertEqual(found.map(\.kind), [.column, .keyword, .keyword, .keyword, .table], "\(texts(found))")
        // At a table position it is the other way round.
        let from = model.suggestions(for: tab, prefix: "ex", path: [],
                                     scope: EditorCaretScope(clause: .queryTable, selectAliases: []))
        XCTAssertEqual(from.first?.kind, .table)
    }

    func testMatchQualityOrdersPrefixThenWordThenStretch() {
        XCTAssertEqual(SQLSuggestions.match("rekap_kpm", typed: "rek"), 0)
        XCTAssertEqual(SQLSuggestions.match("rekap_kpm", typed: "kpm"), 1)
        XCTAssertEqual(SQLSuggestions.match("grossNet", typed: "net"), 1, "a camel-case word start")
        XCTAssertEqual(SQLSuggestions.match("rekap_kpm", typed: "ekap"), 2)
        XCTAssertNil(SQLSuggestions.match("rekap_kpm", typed: "ek"), "two characters inside a word are noise")
        XCTAssertNil(SQLSuggestions.match("GROUP BY", typed: "by", keyword: true))
        XCTAssertEqual(SQLSuggestions.match("anything", typed: ""), 0)
        XCTAssertEqual(SQLSuggestions.match("prod-ops-x", typed: "prod-o"), 0)
        XCTAssertEqual(SQLSuggestions.match("prod-ops-x", typed: "ops"), 1)
    }

    // MARK: The list on screen

    func testTheListShowsEightRowsAndScrollsWithTheSelection() {
        let completion = EditorCompletion()
        completion.present((0..<20).map { SQLSuggestion(text: "t\($0)", kind: .table) },
                           anchor: .zero, passive: false)
        XCTAssertEqual(completion.visible.map(\.text), (0..<8).map { "t\($0)" })
        completion.move(-1)          // wraps to the last row
        XCTAssertEqual(completion.selected, 19)
        XCTAssertEqual(completion.visible.first?.text, "t12")
        XCTAssertEqual(completion.visible.last?.text, "t19")
        completion.move(1)           // and round again
        XCTAssertEqual(completion.selected, 0)
        XCTAssertEqual(completion.visible.first?.text, "t0")
    }

    func testAnArrowKeyMakesAPassiveListActive() {
        let completion = EditorCompletion()
        completion.present([SQLSuggestion(text: "a", kind: .table), SQLSuggestion(text: "b", kind: .table)],
                           anchor: .zero, passive: true)
        XCTAssertTrue(completion.passive)
        completion.move(1)
        XCTAssertFalse(completion.passive, "Return would otherwise still be a newline after the user stepped in")
        completion.dismiss()
        XCTAssertFalse(completion.active)
    }

    // MARK: Where the caret is

    func testAnApostropheInACommentDoesNotSwitchSuggestionsOffForTheRestOfTheFile() {
        let sql = "-- don't forget the filter\nselect * from pe" as NSString
        XCTAssertFalse(SQLEditor.Coordinator.insideStringLiteral(sql, upTo: sql.length),
                       "an apostrophe in a comment was read as the start of a string")
    }

    func testAQuoteInsideAQuotedNameDoesNotOpenAString() {
        let sql = "select \"it's\" from pe" as NSString
        XCTAssertFalse(SQLEditor.Coordinator.insideStringLiteral(sql, upTo: sql.length))
    }

    func testAStringLiteralIsStillAString() {
        let sql = "select * from t where a = 'pe" as NSString
        XCTAssertTrue(SQLEditor.Coordinator.insideStringLiteral(sql, upTo: sql.length))
    }

    func testACommentIsNotCode() {
        for sql in ["select 1 -- pe", "select 1 /* pe"] as [NSString] {
            XCTAssertTrue(SQLEditor.Coordinator.insideStringLiteral(sql, upTo: sql.length),
                          "suggestions opened inside \(sql)")
        }
    }
}
