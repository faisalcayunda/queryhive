import AppKit
import QueryHiveFFI
import XCTest

@testable import QueryHive

/// What completion must not do while somebody types: stay cheap on a big catalog, stay quiet (no
/// sheet, no Keychain dialog, no message), stay bounded on the server, and stop what it started
/// when the answer is no longer wanted.
@MainActor
final class CompletionHardeningTests: XCTestCase {
    private var savedStore: (any SecretStoring)?

    override func setUp() {
        super.setUp()
        isolateConnectionStore()
        savedStore = AppModel.secretStore
    }

    override func tearDown() {
        if let savedStore { AppModel.secretStore = savedStore }
        SQLSuggestions.makeAlias = { sqlTableAlias(table: $0, taken: $1) }
        super.tearDown()
    }

    // MARK: Rig

    private func connection(tunnel: Bool = false) -> Connection {
        Connection(id: UUID(), name: "Warehouse", color: .violet, kind: .trino, host: "trino.internal",
                   port: 8443, sslmode: "prefer", user: "queryhive", database: "hive",
                   schema: "analytics", verify: true, sshHost: tunnel ? "bastion.corp" : "")
    }

    private func rig(engine: any DatabaseEngine, tunnel: Bool = false, loaded: Bool = false)
        -> (model: AppModel, tab: QueryTab, root: TreeNode, analytics: TreeNode, connection: Connection) {
        let model = AppModel()
        model.engine = engine
        let connection = connection(tunnel: tunnel)
        model.connections = [connection]
        model.rebuildTree()
        let root = model.tree[0]
        let hive = TreeNode.catalog("hive", parent: root)
        let analytics = TreeNode.schema("analytics", parent: hive)
        if loaded {
            analytics.children = ["wilayah", "program"].map { TreeNode.table($0, parent: analytics) }
            hive.children = [analytics]
            root.children = [hive]
        }
        let tab = QueryTab(title: "q")
        tab.connectionID = connection.id
        model.tabs = [tab]
        model.selectedTabID = tab.id
        return (model, tab, root, analytics, connection)
    }

    private func pump(_ seconds: TimeInterval = 0.15) {
        RunLoop.main.run(until: Date().addingTimeInterval(seconds))
    }

    private func relation(_ name: String, alias: String? = nil) -> EditorRelation {
        EditorRelation(kind: .table, catalog: nil, schema: nil, name: name, quoted: false, alias: alias,
                       nameStartUtf16: 0, nameLenUtf16: 0)
    }

    private func references(_ relations: [EditorRelation]) -> EditorReferences {
        EditorReferences(revision: 1, statementStartUtf16: 0, statementLenUtf16: 0, relations: relations,
                         ctes: [], source: .tree)
    }

    // MARK: The cost of a big catalog

    func testAnAliasIsMadeOnlyForTheRowsThatAreKept() {
        let r = rig(engine: MockEngine())
        let hive = r.root.children?.first ?? TreeNode.catalog("hive", parent: r.root)
        let analytics = TreeNode.schema("analytics", parent: hive)
        analytics.children = (0..<5_000).map { TreeNode.table(String(format: "t%04d", $0), parent: analytics) }
        hive.children = [analytics]
        r.root.children = [hive]

        var made = 0
        SQLSuggestions.makeAlias = { _, _ in made += 1; return "a\(made)" }
        let refs = references([relation("other", alias: "x")])
        let scope = EditorCaretScope(clause: .queryTable, selectAliases: [])

        let found = r.model.suggestions(for: r.tab, prefix: "t", path: [], scope: scope, references: refs)
        XCTAssertEqual(found.count, AppModel.suggestionLimit)
        XCTAssertLessThanOrEqual(made, AppModel.suggestionLimit,
                                 "an alias was made for \(made) of 5000 tables, not for the rows kept")
        XCTAssertTrue(found.allSatisfy { $0.insert.split(separator: " ").count == 2 })

        made = 0
        XCTAssertTrue(r.model.suggestions(for: r.tab, prefix: "zzz", path: [], scope: scope, references: refs)
            .filter { $0.kind == .table }.isEmpty)
        XCTAssertEqual(made, 0, "a table that does not match was given an alias")
    }

    // MARK: A read that is no longer wanted is stopped

    private func columnsEngine() -> MockEngine {
        let engine = MockEngine()
        engine.answer("columns", with: .events([
            Event(event: "table_columns", fields: [Event.ColumnField(name: "kode", type: "varchar",
                                                                     nullable: nil, default: nil, extra: "")]),
        ]))
        for command in ["catalogs", "schemas", "tables"] { engine.answer(command, with: .events([])) }
        return engine
    }

    func testASupersededColumnReadIsStoppedAndGivesItsPlaceBack() throws {
        let engine = columnsEngine()
        let r = rig(engine: engine, loaded: true)
        let refs = references([relation("wilayah", alias: "w")])
        _ = r.model.suggestions(for: r.tab, prefix: "", path: ["w"],
                                scope: EditorCaretScope(clause: .where, selectAliases: []), references: refs)
        let table = try XCTUnwrap(r.analytics.children?.first { $0.title == "wilayah" })
        XCTAssertEqual(r.model.completionColumnReads, 1)
        let handle = try XCTUnwrap(engine.runs.last)
        XCTAssertFalse(handle.terminated)

        r.model.invalidateCompletionCatalog(for: r.tab, object: ["wilayah"])
        XCTAssertTrue(handle.terminated, "the read was only ignored, not stopped")
        XCTAssertEqual(r.model.completionColumnReads, 0, "it kept its place among the reads in flight")
        XCTAssertFalse(table.columnsLoading)
        pump()
        XCTAssertNil(table.columns)
        XCTAssertEqual(r.model.completionColumnReads, 0, "the late exit gave the place back twice")
    }

    func testRefreshingTheNodeStopsItsReadToo() throws {
        let engine = columnsEngine()
        let r = rig(engine: engine, loaded: true)
        let refs = references([relation("wilayah", alias: "w")])
        _ = r.model.suggestions(for: r.tab, prefix: "", path: ["w"],
                                scope: EditorCaretScope(clause: .where, selectAliases: []), references: refs)
        let table = try XCTUnwrap(r.analytics.children?.first { $0.title == "wilayah" })
        XCTAssertTrue(table.columnsLoading)
        r.model.refresh(table)
        XCTAssertFalse(table.columnsLoading, "refresh left the node waiting for a read it dropped")
        XCTAssertTrue(try XCTUnwrap(engine.runs.last).terminated)
        r.model.reloadTree()
        XCTAssertEqual(r.model.completionColumnReads, 0)
    }

    // MARK: Quiet

    /// An engine behind the host key gate whose every browse command is refused for its key.
    private func refusedKeyRig() throws -> (RecordingEngine, HostKeyCenter, HostKeyGate) {
        let line = #"{"event":"error","message":"the bastion's host key is not trusted","host_key":{"state":"unknown","host":"bastion.corp","port":22,"alias":"prod","key_type":"ssh-ed25519","fingerprint":"SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","app_known_hosts":"/x/known_hosts","ca_covered":false,"recorded":[],"pinned":null}}"#
        let event = try XCTUnwrap(EngineWire.event(in: Data(line.utf8)))
        let recording = RecordingEngine()
        for command in ["catalogs", "schemas", "tables", "columns"] { recording.script[command] = ([event], 1) }
        let center = HostKeyCenter()
        return (recording, center, HostKeyGate(wrapping: recording, center: center))
    }

    func testACompletionLoadThatMeetsARefusedHostKeyRaisesNoSheetAndIsNotRepeated() throws {
        let (recording, center, gate) = try refusedKeyRig()
        let r = rig(engine: gate, tunnel: true)
        r.model.noteConnected(r.connection.id)   // the person has connected through it before

        for _ in 0..<3 { _ = r.model.suggestions(for: r.tab, prefix: "wi", path: []) }
        XCTAssertEqual(recording.runs.map(\.command), ["catalogs"], "asked again after it failed")
        XCTAssertNil(center.prompt, "a sheet came up in the middle of a word")
        XCTAssertNil(r.root.error, "the failure was said in the side bar")
        XCTAssertTrue(r.root.completionFailed)
        XCTAssertNil(recording.runs.first?.env["SSH_HOST_KEY_DETAIL"], "the detail was asked for")

        // Control: the same refusal met by a person expanding the node does reach the sheet, so the
        // rig above could have shown one.
        r.model.loadChildren(of: r.root)
        XCTAssertNotNil(center.prompt)
        XCTAssertEqual(recording.runs.count, 2)
    }

    func testATunnelledConnectionIsLeftAloneUntilThePersonHasConnected() throws {
        let (recording, center, gate) = try refusedKeyRig()
        let r = rig(engine: gate, tunnel: true)
        _ = r.model.suggestions(for: r.tab, prefix: "wi", path: [])
        _ = r.model.suggestions(for: r.tab, prefix: "", path: ["hive"])
        XCTAssertTrue(recording.runs.isEmpty, "a tunnel was opened by typing")
        XCTAssertNil(center.prompt)

        // Anything that carried a statement to its end through it counts.
        r.model.noteConnected(r.connection.id)
        _ = r.model.suggestions(for: r.tab, prefix: "wi", path: [])
        XCTAssertEqual(recording.runs.count, 1)
    }

    /// A Keychain whose `get` would show the approval dialog and whose non-prompting read says so.
    private final class PromptingStore: SecretStoring {
        var prompts = 0
        func contains(slot: ConnectionKeychain.Slot, for id: UUID) -> Bool { true }
        func get(slot: ConnectionKeychain.Slot, for id: UUID) throws -> String? {
            prompts += 1
            return "secret"
        }
        func getWithoutPrompt(slot: ConnectionKeychain.Slot, for id: UUID) throws -> String? {
            throw ConnectionKeychain.KeychainError(status: errSecInteractionNotAllowed)
        }
        func set(_ secret: String, slot: ConnectionKeychain.Slot, for id: UUID) throws {}
        func delete(slot: ConnectionKeychain.Slot, for id: UUID) throws {}
    }

    func testCompletionNeverShowsTheKeychainDialogAndSaysNothingWhenItCannotRead() {
        let store = PromptingStore()
        AppModel.secretStore = store
        let engine = MockEngine()
        let r = rig(engine: engine, loaded: false)

        _ = r.model.suggestions(for: r.tab, prefix: "wi", path: [])
        XCTAssertEqual(store.prompts, 0, "the Keychain was asked in a way that can show the dialog")
        XCTAssertTrue(engine.calls.isEmpty, "a load was sent without a password it was allowed to read")
        XCTAssertTrue(r.root.completionFailed)
        XCTAssertNil(r.root.error)

        // Control: the sidebar, which a person drives, reads the way it always did.
        r.model.loadChildren(of: r.root)
        XCTAssertEqual(store.prompts, 1)
    }

    func testAColumnReadAlsoReadsThePasswordWithoutTheDialog() throws {
        let store = PromptingStore()
        AppModel.secretStore = store
        let engine = columnsEngine()
        let r = rig(engine: engine, loaded: true)
        let table = try XCTUnwrap(r.analytics.children?.first)
        r.model.loadColumns(of: table)
        XCTAssertEqual(store.prompts, 0)
        XCTAssertTrue(engine.calls.isEmpty)
        XCTAssertTrue(table.completionFailed)
        XCTAssertNil(table.columnsRun)
        // And it is not retried with every key.
        r.model.loadColumns(of: table)
        XCTAssertEqual(store.prompts, 0)
    }

    func testAColumnReadForATunnelledConnectionWaitsToo() throws {
        let engine = columnsEngine()
        let r = rig(engine: engine, tunnel: true, loaded: true)
        let table = try XCTUnwrap(r.analytics.children?.first)
        r.model.loadColumns(of: table)
        XCTAssertTrue(engine.calls.isEmpty)
        r.model.noteConnected(r.connection.id)
        r.model.loadColumns(of: table)
        XCTAssertEqual(engine.calls.map(\.command), ["columns"])
    }

    func testRefreshingANodeForgetsAQuietFailureSoAPersonCanTryAgain() throws {
        let r = rig(engine: MockEngine(), loaded: true)
        let table = try XCTUnwrap(r.analytics.children?.first)
        table.completionFailed = true
        table.columnsLoading = true
        r.model.refresh(table)
        XCTAssertFalse(table.completionFailed)
        XCTAssertFalse(table.columnsLoading)
    }

    // MARK: A word that is the statement's own name

    func testAWordThatIsAnAliasOfTheStatementOffersNoColumnsToTurnItInto() throws {
        let engine = columnsEngine()
        let r = rig(engine: engine, loaded: true)
        let table = try XCTUnwrap(r.analytics.children?.first { $0.title == "wilayah" })
        table.columns = [Event.Column(name: "order_id", type: "bigint"), Event.Column(name: "o_x", type: "bigint")]
        let refs = references([relation("wilayah", alias: "o")])
        let scope = EditorCaretScope(clause: .joinCondition, selectAliases: [])

        XCTAssertTrue(r.model.suggestions(for: r.tab, prefix: "o", path: [], scope: scope, references: refs).isEmpty,
                      "Return would have replaced the alias `o` with a column")
        XCTAssertTrue(r.model.suggestions(for: r.tab, prefix: "O", path: [], scope: scope, references: refs).isEmpty)
        // One more character and it is a column being written.
        XCTAssertEqual(r.model.suggestions(for: r.tab, prefix: "or", path: [], scope: scope, references: refs)
            .filter { $0.kind == .column }.map(\.text), ["order_id"])
        // And where the word is not one of the statement's names, one letter still opens columns.
        let others = r.model.suggestions(for: r.tab, prefix: "o", path: [], scope: scope,
                                         references: references([relation("wilayah", alias: "w")]))
        XCTAssertFalse(others.isEmpty)
    }

    // MARK: The analysis

    func testTheAnalysisRefusesAnAnswerForARevisionThatMovedOn() throws {
        let analysis = try EditorAnalysis(text: "select 1", dialect: .trino)
        let asked = analysis.revision
        try analysis.replace(range: NSRange(location: 8, length: 0), with: " ")
        XCTAssertThrowsError(try analysis.references(at: 0, revision: asked)) {
            XCTAssertEqual($0 as? EditorAnalysisError, .stale)
        }
        XCTAssertNoThrow(try analysis.references(at: 0, revision: analysis.revision))
    }

    func testTheAnalysisIsAskedInTheBackgroundAndAnsweredOnTheMainThread() throws {
        let analysis = try EditorAnalysis(text: "select * from wilayah w", dialect: .trino)
        let answered = expectation(description: "answered")
        var delivered = false
        analysis.requestReferences(at: 5) { result in
            XCTAssertTrue(Thread.isMainThread)
            delivered = true
            XCTAssertEqual((try? result.get())?.relations.first?.alias, "w")
            answered.fulfill()
        }
        XCTAssertFalse(delivered, "the answer was given before the call returned: nothing was asynchronous")
        wait(for: [answered], timeout: 5)
    }
}
