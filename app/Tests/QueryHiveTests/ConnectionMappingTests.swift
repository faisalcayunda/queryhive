import XCTest

@testable import QueryHive

/// A dictionary standing in for the Keychain, that also counts what was read so a test can show a
/// slot a connection does not use was never asked for.
final class MemorySecretStore: SecretStoring, @unchecked Sendable {
    private let lock = NSLock()
    private var items: [String: String] = [:]
    private(set) var reads: [String] = []

    private func key(_ slot: ConnectionKeychain.Slot, _ id: UUID) -> String { ConnectionKeychain.account(slot, for: id) }

    func contains(slot: ConnectionKeychain.Slot, for id: UUID) -> Bool {
        lock.lock(); defer { lock.unlock() }
        return items[key(slot, id)] != nil
    }

    func get(slot: ConnectionKeychain.Slot, for id: UUID) throws -> String? {
        lock.lock(); defer { lock.unlock() }
        reads.append(key(slot, id))
        return items[key(slot, id)]
    }

    func set(_ secret: String, slot: ConnectionKeychain.Slot, for id: UUID) throws {
        lock.lock(); defer { lock.unlock() }
        items[key(slot, id)] = secret
    }

    func delete(slot: ConnectionKeychain.Slot, for id: UUID) throws {
        lock.lock(); defer { lock.unlock() }
        items[key(slot, id)] = nil
    }

    /// Every account held, for "nothing was written" and "all four were removed".
    var accounts: Set<String> {
        lock.lock(); defer { lock.unlock() }
        return Set(items.keys)
    }
}

extension XCTestCase {
    /// Replaces the app's secret store with a dictionary for one test, and puts the real one back.
    @discardableResult
    func useMemorySecretStore() -> MemorySecretStore {
        let store = MemorySecretStore()
        let previous = AppModel.secretStore
        AppModel.secretStore = store
        addTeardownBlock { AppModel.secretStore = previous }
        return store
    }
}

/// The mapping from a saved connection to the engine's settings (blueprint w11 section 9.1), and the
/// shape of `connections.json` that carries it.
final class ConnectionMappingTests: XCTestCase {
    // MARK: Parity with the MCP server

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

    /// `crates/qh-ffi/tests/fixtures/connection_env.json`, found from this file's own path.
    private func fixture() throws -> Fixture {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        let url = root.appendingPathComponent("crates/qh-ffi/tests/fixtures/connection_env.json")
        let data = try XCTUnwrap(try? Data(contentsOf: url), "the shared fixture is missing: \(url.path)")
        return try JSONDecoder().decode(Fixture.self, from: data)
    }

    /// The app and the MCP server map a stored connection to the same settings. `qh-ffi`'s
    /// `tests/mcp.rs` reads this same file and asserts the same values from the Rust side; a key
    /// missing from an environment counts as empty on both.
    func testTheAppMapsEveryFixtureCaseToTheSameSettingsTheMCPServerDoes() throws {
        let fixture = try fixture()
        XCTAssertGreaterThanOrEqual(fixture.cases.count, 8)
        let knownHosts = try ConnectionStore.directory().appendingPathComponent("known_hosts").path
        for entry in fixture.cases {
            let secrets = ConnectionSecrets(password: entry.secrets["password"],
                                            sshPassword: entry.secrets["sshPassword"],
                                            sshPassphrase: entry.secrets["sshPassphrase"],
                                            jwt: entry.secrets["jwt"])
            let vars = AppModel.connectionEnvironment(entry.connection, secrets: secrets)
            for key in fixture.keys {
                var expected = entry.expect[key] ?? ""
                if expected == "<app-known-hosts>" { expected = knownHosts }
                XCTAssertEqual(vars[key] ?? "", expected, "\(entry.name): \(key)")
            }
        }
    }

    func testNoSavedConnectionEverYieldsAHostKeyPinAndASecretIsOnlySentWhereItsModeUsesIt() {
        let secrets = ConnectionSecrets(password: "SENTINEL-db", sshPassword: "SENTINEL-ssh",
                                        sshPassphrase: "SENTINEL-phrase", jwt: "SENTINEL-jwt")
        XCTAssertFalse("\(secrets)".contains("SENTINEL"), "a secret must not print itself")
        XCTAssertFalse(String(reflecting: secrets).contains("SENTINEL"))
        for kind in ConnectionKind.allCases {
            for host in ["", "bastion"] {
                for auth in SSHAuthMethod.allCases {
                    for dbAuth in DatabaseAuth.allCases {
                        for useConfig in [false, true] {
                            let connection = Connection(id: UUID(), name: "c", color: .blue, kind: kind,
                                                        host: "h", port: 1, user: "u", database: "d",
                                                        schema: "", verify: true, sshHost: host,
                                                        sshUseConfig: useConfig, sshPort: 22, sshUser: "u",
                                                        sshAuth: auth, sshKeyPath: "/k", dbAuth: dbAuth,
                                                        caFile: "/ca.pem")
                            let vars = AppModel.connectionEnvironment(connection, secrets: secrets)
                            let label = "\(kind) \(host) \(auth) \(dbAuth)"
                            XCTAssertNil(vars["SSH_HOST_KEY_ACCEPT"], label)
                            XCTAssertNil(vars["SSH_HOST_KEY_DETAIL"], "the gate adds it, not the mapping: \(label)")
                            XCTAssertEqual(vars["SSH_APP_KNOWN_HOSTS"] != nil, !host.isEmpty, label)
                            XCTAssertEqual(vars["SSH_PASSWORD"] != nil, !host.isEmpty && auth == .password, label)
                            XCTAssertEqual(vars["SSH_KEY_PASSPHRASE"] != nil, !host.isEmpty && auth == .key, label)
                            let jwt = kind == .trino && dbAuth == .jwt
                            XCTAssertEqual(vars["DB_JWT"] != nil, jwt, label)
                            XCTAssertEqual(vars["DB_PASSWORD"], jwt ? "" : "SENTINEL-db", label)
                        }
                    }
                }
            }
        }
    }

    // MARK: Which secrets a run reads

    @MainActor
    func testARunReadsOnlyTheSlotsTheConnectionUsesAndAddsItsOwnTimeout() throws {
        isolateConnectionStore()
        let store = useMemorySecretStore()
        let id = UUID()
        for slot in ConnectionKeychain.Slot.allCases { try store.set("v-\(slot)", slot: slot, for: id) }
        let connection = Connection(id: id, name: "c", color: .blue, kind: .postgres, host: "h", port: 5432,
                                    user: "u", database: "d", schema: "", verify: true,
                                    sshHost: "bastion", sshUser: "deploy", sshAuth: .key, sshKeyPath: "/k",
                                    statementTimeoutMS: 15_000)
        let model = AppModel(persistsSession: false)
        let vars = try model.connectionEnvironment(connection)
        XCTAssertEqual(vars["DB_PASSWORD"], "v-database")
        XCTAssertEqual(vars["SSH_KEY_PASSPHRASE"], "v-sshPassphrase")
        XCTAssertNil(vars["SSH_PASSWORD"])
        XCTAssertNil(vars["DB_JWT"])
        XCTAssertEqual(vars["STATEMENT_TIMEOUT_MS"], "15000", "the connection's own bound")
        let read = Set(store.reads)
        XCTAssertEqual(read, [ConnectionKeychain.account(.database, for: id),
                              ConnectionKeychain.account(.sshPassphrase, for: id)],
                       "a slot the connection does not use is never asked for, so it never prompts")
    }

    @MainActor
    func testAConnectionWithoutABoundInheritsTheAppSettingAndFollowsIt() {
        isolateConnectionStore()
        let model = AppModel(persistsSession: false)
        var connection = Connection(id: UUID(), name: "c", color: .blue, kind: .mysql, host: "h", port: 3306,
                                    user: "u", database: "d", schema: "", verify: true)
        // The setting is written through to UserDefaults, which a test run must leave as it found it.
        let before = model.statementTimeoutMS
        defer { model.statementTimeoutMS = before }
        model.statementTimeoutMS = 60_000
        XCTAssertEqual(model.effectiveStatementTimeoutMS(connection), 60_000)
        model.statementTimeoutMS = 90_000
        XCTAssertEqual(model.effectiveStatementTimeoutMS(connection), 90_000, "nil follows the setting")
        connection.statementTimeoutMS = 0
        XCTAssertEqual(model.effectiveStatementTimeoutMS(connection), 0, "an explicit 0 is its own answer: no bound")
        connection.statementTimeoutMS = 5_000
        XCTAssertEqual(model.effectiveStatementTimeoutMS(connection), 5_000)
    }

    @MainActor
    func testTheDatabaseOnlyShimStillCarriesTheTunnelAndLeavesOutWhatItCouldNotReadSilently() {
        // The tree's browse and the warm-up hold only the database password. The rest of the mapping
        // must still be there, and under `--bench` no Keychain is touched for the other slots.
        AppModel.benchPassword = "bench"
        defer { AppModel.benchPassword = nil }
        let connection = Connection(id: UUID(), name: "c", color: .blue, kind: .postgres, host: "h", port: 5432,
                                    user: "u", database: "d", schema: "", verify: true,
                                    sshHost: "bastion", sshUser: "deploy", sshAuth: .password, caFile: "/ca.pem")
        let vars = AppModel.connectionEnvironment(connection, password: "pw")
        XCTAssertEqual(vars["SSH_HOST"], "bastion")
        XCTAssertEqual(vars["DB_CA_FILE"], "/ca.pem")
        XCTAssertEqual(vars["DB_PASSWORD"], "pw")
        XCTAssertNil(vars["SSH_PASSWORD"], "not read under --bench")
    }

    // MARK: connections.json

    private let base = #""id":"00000000-0000-0000-0000-000000000001","name":"wh""#

    private func decode(_ json: String) throws -> Connection {
        try JSONDecoder().decode(Connection.self, from: Data(json.utf8))
    }

    func testAFileWrittenBeforeW11LoadsWithTheDefaultsItAlreadyBehavedAs() throws {
        let connection = try decode("{\(base)}")
        XCTAssertEqual(connection.sshHost, "")
        XCTAssertFalse(connection.usesTunnel)
        XCTAssertFalse(connection.sshUseConfig)
        XCTAssertEqual(connection.sshPort, 0)
        XCTAssertEqual(connection.sshUser, "")
        XCTAssertEqual(connection.sshAuth, .agent)
        XCTAssertEqual(connection.sshKeyPath, "")
        XCTAssertEqual(connection.dbAuth, .password)
        XCTAssertEqual(connection.caFile, "")
        XCTAssertNil(connection.statementTimeoutMS)
    }

    func testTheW11FieldsRoundTripAndABoundOfNilIsNotWritten() throws {
        let original = Connection(id: UUID(), name: "c", color: .blue, kind: .trino, host: "h", port: 8443,
                                  user: "u", database: "d", schema: "", verify: true,
                                  sshHost: "prod-bastion", sshUseConfig: true, sshPort: 2222, sshUser: "deploy",
                                  sshAuth: .key, sshKeyPath: "/k", dbAuth: .jwt, caFile: "/ca.pem",
                                  statementTimeoutMS: 15_000)
        let data = try JSONEncoder().encode(original)
        XCTAssertEqual(try JSONDecoder().decode(Connection.self, from: data), original)

        var inherit = original
        inherit.statementTimeoutMS = nil
        let text = String(decoding: try JSONEncoder().encode(inherit), as: UTF8.self)
        XCTAssertFalse(text.contains("statementTimeoutMS"), "inherit is the absence of a value, not a value")
        XCTAssertNil(try JSONDecoder().decode(Connection.self, from: Data(text.utf8)).statementTimeoutMS)
        // No secret field exists to be written: not one key of the object names one.
        let keys = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any]).keys
        for key in keys {
            for word in ["password", "passphrase", "token"] {
                XCTAssertFalse(key.lowercased().contains(word), key)
            }
            XCTAssertNotEqual(key.lowercased(), "jwt")
        }
    }

    func testAWordALaterBuildWroteDoesNotMoveTheFileAside() throws {
        let connection = try decode(#"{\#(base),"sshAuth":"kerberos","dbAuth":"saml","statementTimeoutMS":"soon"}"#)
        XCTAssertEqual(connection.sshAuth, .agent)
        XCTAssertEqual(connection.dbAuth, .password)
        XCTAssertNil(connection.statementTimeoutMS)
    }
}
