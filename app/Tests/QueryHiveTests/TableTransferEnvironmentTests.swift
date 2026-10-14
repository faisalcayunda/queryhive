import XCTest

@testable import QueryHive

/// A Keychain that refuses one connection's items, for "one side's credentials fail and nothing
/// runs".
final class RefusingSecretStore: SecretStoring, @unchecked Sendable {
    let refused: UUID
    let inner = MemorySecretStore()

    init(refusing id: UUID) { refused = id }

    struct Denied: LocalizedError {
        var errorDescription: String? { "the user denied access" }
    }

    func contains(slot: ConnectionKeychain.Slot, for id: UUID) -> Bool { inner.contains(slot: slot, for: id) }
    func get(slot: ConnectionKeychain.Slot, for id: UUID) throws -> String? {
        if id == refused { throw Denied() }
        return try inner.get(slot: slot, for: id)
    }
    func set(_ secret: String, slot: ConnectionKeychain.Slot, for id: UUID) throws {
        try inner.set(secret, slot: slot, for: id)
    }
    func delete(slot: ConnectionKeychain.Slot, for id: UUID) throws { try inner.delete(slot: slot, for: id) }
}

/// What a copy across connections sends the engine (W13-T20, blueprint §3.2 and §6.6, ADR-0046
/// decisions 1 and 9): the source's settings as any run sends them, the target's under `DEST_`
/// from its own Keychain items, and a confirmation only where the target's level asks for one.
@MainActor
final class TableTransferEnvironmentTests: XCTestCase {
    private var secrets: MemorySecretStore!

    override func setUp() {
        super.setUp()
        isolateConnectionStore()
        secrets = useMemorySecretStore()
    }

    private func connection(_ name: String, _ kind: ConnectionKind, safeMode: ConnectionSafeMode = .full,
                            timeout: Int? = nil) -> Connection {
        Connection(id: UUID(), name: name, color: .violet, kind: kind, host: "\(name.lowercased()).internal",
                   port: kind.defaultPort, sslmode: "", user: "qh", database: "app", schema: "public",
                   verify: true, safeMode: safeMode, statementTimeoutMS: timeout)
    }

    private func fixture(source: Connection, target: Connection) -> (AppModel, QueryTab) {
        let model = AppModel()
        model.engine = MockEngine()
        model.experiments = scratchExperiments(saveTableAcrossConnections: true)
        model.connections = [source, target]
        let tab = QueryTab(title: "Query 1")
        tab.connectionID = source.id
        tab.sql = "SELECT 1 AS id"
        tab.destination = .table
        tab.targetConnectionID = target.id
        tab.targetSchema = "public"
        tab.targetCatalog = "shop"
        tab.targetTable = "people"
        model.tabs = [tab]
        model.selectedTabID = tab.id
        return (model, tab)
    }

    // MARK: The target's settings, key by key

    private struct Fixture: Decodable {
        struct Case: Decodable {
            let name: String
            let connection: Connection
            let secrets: [String: String]
            let expect: [String: String]
        }
        let keys: [String]
        let cases: [Case]
    }

    /// The fixture `ConnectionMappingTests` and `qh-ffi`'s `tests/mcp.rs` both read, found from this
    /// file's own path: whatever a run on a connection sends, a copy to it sends under `DEST_`.
    func testEveryFixtureCaseReachesTheEngineUnderDESTWithTheSameValues() throws {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let url = root.appendingPathComponent("crates/qh-ffi/tests/fixtures/connection_env.json")
        let data = try XCTUnwrap(try? Data(contentsOf: url), "the shared fixture is missing: \(url.path)")
        let shared = try JSONDecoder().decode(Fixture.self, from: data)
        XCTAssertGreaterThanOrEqual(shared.cases.count, 8)
        let knownHosts = try ConnectionStore.directory().appendingPathComponent("known_hosts").path
        let source = connection("Lake", .trino)
        for entry in shared.cases {
            let target = entry.connection
            for (slot, key) in [(ConnectionKeychain.Slot.database, "password"), (.sshPassword, "sshPassword"),
                                (.sshPassphrase, "sshPassphrase"), (.jwt, "jwt")] {
                if let value = entry.secrets[key] { try secrets.set(value, slot: slot, for: target.id) }
            }
            let (model, tab) = fixture(source: source, target: target)
            let env = try model.overrides(for: tab, connection: source, sql: "SELECT 1 AS id")
            for key in shared.keys {
                var expected = entry.expect[key] ?? ""
                if expected == "<app-known-hosts>" { expected = knownHosts }
                XCTAssertEqual(env["DEST_" + key] ?? "", expected, "\(entry.name): DEST_\(key)")
            }
            XCTAssertNil(env["DEST_SSH_HOST_KEY_ACCEPT"], "\(entry.name): no run is born trusting a key")
            XCTAssertEqual(env["DEST_SAFE_MODE"], target.safeMode.rawValue, entry.name)
        }
    }

    func testTheSourceSendsWhatItAlwaysDidAndNoDestinationKeyLeaksIntoIt() throws {
        let source = connection("Lake", .trino)
        let target = connection("Warehouse", .postgres)
        let (model, tab) = fixture(source: source, target: target)
        let env = try model.overrides(for: tab, connection: source, sql: "SELECT 1 AS id")

        var mine = try model.connectionEnvironment(source)
        mine["STATEMENT_TIMEOUT_MS"] = "0"
        for (key, value) in mine {
            XCTAssertEqual(env[key], value, key)
        }
        XCTAssertEqual(env["DB_HOST"], "lake.internal", "the top level is the source")
        XCTAssertEqual(env["DEST_DB_HOST"], "warehouse.internal")
        XCTAssertEqual(env["DEST_DB_KIND"], "postgres")
        XCTAssertEqual(env["WRITE_MODE"], "create")
        XCTAssertEqual(env["TARGET_SCHEMA"], "public")
        XCTAssertEqual(env["TARGET_TABLE"], "people")
    }

    func testTheLocalDatabasePathIsNeverSentForTheTarget() throws {
        let (model, tab) = fixture(source: connection("Lake", .trino), target: connection("Warehouse", .postgres))
        let env = try model.overrides(for: tab, connection: model.connections[0], sql: "SELECT 1 AS id")
        XCTAssertNotNil(env["DB_PATH"], "the test run redirects the store, so the top level names it")
        XCTAssertNil(env["DEST_DB_PATH"], "the engine reads the top-level one only")
    }

    // MARK: Statement bounds (PF-23, OQ-13)

    func testTheInheritedDefaultIsLiftedOnTheSourceAndTheTargetKeepsItsBound() throws {
        let (model, tab) = fixture(source: connection("Lake", .trino), target: connection("Warehouse", .postgres))
        let env = try model.overrides(for: tab, connection: model.connections[0], sql: "SELECT 1 AS id")
        XCTAssertEqual(env["STATEMENT_TIMEOUT_MS"], "0", "the app's own default would cut a slow copy off")
        XCTAssertEqual(env["DEST_STATEMENT_TIMEOUT_MS"], String(model.statementTimeoutMS))
    }

    func testABoundTheSourceSetsForItselfIsKept() throws {
        let source = connection("Lake", .trino, timeout: 30_000)
        let target = connection("Warehouse", .postgres, timeout: 5_000)
        let (model, tab) = fixture(source: source, target: target)
        let env = try model.overrides(for: tab, connection: source, sql: "SELECT 1 AS id")
        XCTAssertEqual(env["STATEMENT_TIMEOUT_MS"], "30000")
        XCTAssertEqual(env["DEST_STATEMENT_TIMEOUT_MS"], "5000")
    }

    func testTheSameConnectionPathKeepsTheInheritedBound() throws {
        let source = connection("Warehouse", .postgres)
        let (model, tab) = fixture(source: source, target: connection("Shop", .mysql))
        tab.targetConnectionID = nil
        let env = try model.overrides(for: tab, connection: source, sql: "SELECT 1 AS id")
        XCTAssertEqual(env["STATEMENT_TIMEOUT_MS"], String(model.statementTimeoutMS))
    }

    // MARK: The confirmation goes to the target, and only when its level asks

    func testAnApprovedCopyConfirmsATargetAtConfirmAndNeverTheSource() throws {
        let source = connection("Lake", .trino, safeMode: .confirm)
        let target = connection("Warehouse", .postgres, safeMode: .confirm)
        let (model, tab) = fixture(source: source, target: target)
        let approved = try model.overrides(for: tab, connection: source, sql: "SELECT 1 AS id", confirmed: true)
        XCTAssertEqual(approved["DEST_SAFE_MODE_CONFIRMED"], "1")
        XCTAssertNil(approved["SAFE_MODE_CONFIRMED"], "the source only reads; a confirmation there would be unused authority")
        let unapproved = try model.overrides(for: tab, connection: source, sql: "SELECT 1 AS id", confirmed: false)
        XCTAssertNil(unapproved["DEST_SAFE_MODE_CONFIRMED"])
    }

    func testAFullTargetGetsNoConfirmationItDoesNotNeed() throws {
        let (model, tab) = fixture(source: connection("Lake", .trino), target: connection("Warehouse", .postgres))
        tab.writeMode = .replace
        let env = try model.overrides(for: tab, connection: model.connections[0], sql: "SELECT 1 AS id",
                                      confirmed: true)
        XCTAssertNil(env["DEST_SAFE_MODE_CONFIRMED"])
        XCTAssertNil(env["SAFE_MODE_CONFIRMED"])
        XCTAssertEqual(env["DEST_SAFE_MODE"], "full")
        XCTAssertEqual(env["WRITE_MODE"], "replace")
    }

    // MARK: Credentials

    func testEachSideReadsItsOwnKeychainItems() throws {
        let source = connection("Lake", .trino)
        let target = connection("Warehouse", .postgres)
        try secrets.set("lake-secret", slot: .database, for: source.id)
        try secrets.set("warehouse-secret", slot: .database, for: target.id)
        let (model, tab) = fixture(source: source, target: target)
        let env = try model.overrides(for: tab, connection: source, sql: "SELECT 1 AS id")
        XCTAssertEqual(env["DB_PASSWORD"], "lake-secret")
        XCTAssertEqual(env["DEST_DB_PASSWORD"], "warehouse-secret")
    }

    func testATargetWhoseKeychainRefusesStopsTheCopyAndIsNamed() throws {
        let source = connection("Lake", .trino)
        let target = connection("Warehouse", .postgres)
        let store = RefusingSecretStore(refusing: target.id)
        let previous = AppModel.secretStore
        AppModel.secretStore = store
        defer { AppModel.secretStore = previous }
        let (model, tab) = fixture(source: source, target: target)
        XCTAssertThrowsError(try model.overrides(for: tab, connection: source, sql: "SELECT 1 AS id")) { error in
            let message = (error as? EngineLaunchError)?.message ?? ""
            XCTAssertTrue(message.contains("Warehouse"), message)
        }

        // Through the sheet: nothing is handed to the engine.
        let engine = MockEngine()
        model.engine = engine
        model.run(tab)
        let transfer = try XCTUnwrap(model.transfer)
        model.approveTransfer(transfer)
        XCTAssertEqual(transfer.phase, .failed)
        XCTAssertTrue(transfer.failure?.contains("Warehouse") == true, transfer.failure ?? "")
        XCTAssertTrue(engine.calls.isEmpty)
        XCTAssertEqual(tab.stage, .failed)
    }
}
