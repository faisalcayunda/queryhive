import CommonCrypto
import XCTest

@testable import QueryHive

/// DBeaver's `data-sources.json` and `credentials-config.json`, written here by the test (never read
/// from the owner's DBeaver): groups, the SSH tunnel, TLS that is never lowered, and credentials that
/// are never overwritten (W13-T16, DBX-73).
final class DBeaverImportTests: XCTestCase {
    // MARK: Fixtures

    private static let key = Data([186, 187, 74, 159, 119, 74, 184, 83, 201, 108, 45, 101, 61, 254, 84, 74])

    /// DBeaver's own file format, run forwards: 16 bytes of IV, then AES-128-CBC of the JSON.
    private func credentialsFile(_ json: String) -> Data {
        let iv = Data((0..<16).map { UInt8($0 &* 7 &+ 3) })
        let input = Data(json.utf8)
        var out = Data(count: input.count + kCCBlockSizeAES128)
        var moved = 0
        let status = out.withUnsafeMutableBytes { outBytes in
            input.withUnsafeBytes { inBytes in
                Self.key.withUnsafeBytes { keyBytes in
                    iv.withUnsafeBytes { ivBytes in
                        CCCrypt(CCOperation(kCCEncrypt), CCAlgorithm(kCCAlgorithmAES), CCOptions(kCCOptionPKCS7Padding),
                                keyBytes.baseAddress, Self.key.count, ivBytes.baseAddress,
                                inBytes.baseAddress, input.count, outBytes.baseAddress, outBytes.count, &moved)
                    }
                }
            }
        }
        XCTAssertEqual(Int(status), Int(kCCSuccess))
        return iv + out.prefix(moved)
    }

    private let credentials = """
    {
      "pg-prod": { "#connection": { "user": "ana", "password": "db-pass" },
                   "network/ssh_tunnel": { "user": "deploy", "password": "ssh-pass" } },
      "pg-key": { "#connection": { "user": "ana" },
                  "network/ssh_tunnel": { "user": "deploy", "password": "key-phrase" } },
      "my-plain": { "#connection": { "user": "root", "password": "my-pass" } }
    }
    """

    private func conn(_ driver: String, _ name: String, folder: String? = nil, config: String) -> String {
        let folderPart = folder.map { #""folder": "\#($0)","# } ?? ""
        return #"{ "provider": "x", "driver": "\#(driver)", "name": "\#(name)", \#(folderPart) "configuration": { \#(config) } }"#
    }

    private func dataSources() -> String {
        """
        { "folders": { "Prod": {}, "EU": { "parent": "Prod" } },
          "connections": {
            "pg-prod": \(conn("postgres-jdbc", "Prod PG", folder: "Prod/EU", config: #"""
              "host": "pg.corp", "port": "5433", "database": "app", "type": "prod",
              "url": "jdbc:postgresql://pg.corp:5433/app?sslmode=verify-full",
              "handlers": { "ssh_tunnel": { "type": "tunnel", "enabled": true, "save-password": true,
                  "properties": { "host": "bastion.corp", "port": 2222, "authType": "PASSWORD" } } }
              """#)),
            "pg-key": \(conn("postgres-jdbc", "Key PG", config: #"""
              "host": "k.corp", "port": 5432, "database": "d", "type": "test",
              "handlers": { "ssh_tunnel": { "enabled": true,
                  "properties": { "host": "b2", "authType": "PUBLIC_KEY", "keyPath": "/Users/x/.ssh/id" } } }
              """#)),
            "pg-plain": \(conn("postgres-jdbc", "Plain PG", config: #"""
              "host": "plain.corp", "port": 5432, "database": "d",
              "handlers": { "postgre_ssl": { "enabled": false } }
              """#)),
            "my-plain": \(conn("mysql8", "My Plain", config: #"""
              "host": "my.corp", "port": "3306", "database": "shop",
              "url": "jdbc:mysql://my.corp:3306/shop?useSSL=false"
              """#)),
            "my-verify": \(conn("mysql8", "My Verify", config: #"""
              "host": "v.corp", "port": 3306, "url": "jdbc:mysql://v.corp:3306/shop?sslMode=VERIFY_CA"
              """#)),
            "my-ssl": \(conn("mariaDB", "My Ssl", config: #"""
              "host": "s.corp", "port": 3306,
              "handlers": { "mysql_ssl": { "enabled": true, "properties": {} } }
              """#)),
            "my-ssl-verify": \(conn("mysql8", "My Ssl Verify", config: #"""
              "host": "sv.corp", "port": 3306,
              "handlers": { "mysql_ssl": { "enabled": true, "properties": { "ssl.verify.server": "true" } } }
              """#)),
            "my-ssl-noverify": \(conn("mysql8", "My Ssl NoVerify", config: #"""
              "host": "sn.corp", "port": 3306,
              "handlers": { "mysql_ssl": { "enabled": true, "properties": { "ssl.verify.server": "false" } } }
              """#)),
            "pg-ssl": \(conn("postgres-jdbc", "Pg Ssl", config: #"""
              "host": "ps.corp", "port": 5432, "database": "d",
              "handlers": { "postgre_ssl": { "enabled": true, "properties": {} } }
              """#)),
            "tr": \(conn("trino_jdbc", "Trino TLS", config: #"""
              "url": "jdbc:trino://t.corp:8443/hive/default?SSL=true&SSLVerification=NONE"
              """#)),
            "lite": \(conn("sqlite_jdbc", "A SQLite", config: #""host": "x", "database": "/tmp/a.db""#)),
            "nohost": \(conn("postgres-jdbc", "No Host", config: #""database": "d""#))
          } }
        """
    }

    private func folder(credentials: Bool = true) throws -> URL {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("qh-dbv-\(UUID().uuidString)")
        let dir = root.appendingPathComponent("General/.dbeaver")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try dataSources().write(to: dir.appendingPathComponent("data-sources.json"), atomically: true, encoding: .utf8)
        if credentials {
            try credentialsFile(self.credentials).write(to: dir.appendingPathComponent("credentials-config.json"))
        }
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        return root
    }

    private func read() throws -> ImportBatch {
        try DBeaverImport.read(dataSources: Data(dataSources().utf8), credentials: credentialsFile(credentials))
    }

    private func connection(_ name: String, in batch: ImportBatch) throws -> ImportedConnection {
        try XCTUnwrap(batch.connections.first { $0.name == name }, "\(name) was not imported: \(batch.skipped)")
    }

    // MARK: Reading

    func testTheCredentialsFileDecryptsAndTheSecretsLandOnTheirConnection() throws {
        let pg = try connection("Prod PG", in: read())
        XCTAssertEqual(pg.user, "ana")
        XCTAssertEqual(pg.password, "db-pass")
        XCTAssertEqual(pg.host, "pg.corp")
        XCTAssertEqual(pg.port, 5433)
        XCTAssertEqual(pg.database, "app")
        XCTAssertEqual(pg.environment, .prod)
    }

    func testAFolderPathBecomesOneFlatGroupName() throws {
        XCTAssertEqual(try connection("Prod PG", in: read()).group, "Prod / EU")
        XCTAssertEqual(try connection("Key PG", in: read()).group, "")
    }

    func testAPasswordTunnelCarriesItsHostPortUserAndPassword() throws {
        let pg = try connection("Prod PG", in: read())
        XCTAssertEqual(pg.sshHost, "bastion.corp")
        XCTAssertEqual(pg.sshPort, 2222)
        XCTAssertEqual(pg.sshUser, "deploy")
        XCTAssertEqual(pg.sshAuth, .password)
        XCTAssertEqual(pg.sshPassword, "ssh-pass")
        XCTAssertNil(pg.sshPassphrase)
    }

    func testAKeyTunnelKeepsTheKeyFileAndReadsThePasswordSlotAsThePassphrase() throws {
        let key = try connection("Key PG", in: read())
        XCTAssertEqual(key.sshAuth, .key)
        XCTAssertEqual(key.sshKeyPath, "/Users/x/.ssh/id")
        XCTAssertEqual(key.sshPassphrase, "key-phrase")
        XCTAssertNil(key.sshPassword)
        XCTAssertEqual(key.environment, .staging)
    }

    // MARK: TLS is mapped, never weakened

    func testThePostgresModeInTheURLIsKeptVerbatimAndNeverFoldedIntoRequire() throws {
        XCTAssertEqual(try connection("Prod PG", in: read()).sslmode, "verify-full")
    }

    func testAnEntryThatNamesNoTLSChangesNothingAndADisabledHandlerDoesNotDisableAnything() throws {
        // dbx writes ssl=false for every entry; here silence stays silence.
        let plain = try connection("Plain PG", in: read())
        XCTAssertNil(plain.sslmode)
        XCTAssertEqual(NavicatImport.tlsMode(kind: .postgres, requested: plain.sslmode, current: "verify-full"), "verify-full")
    }

    func testAnEnabledMySQLSSLHandlerWithNoVerifyIsRequire() throws {
        XCTAssertEqual(try connection("My Ssl", in: read()).sslmode, "require")
        XCTAssertEqual(try connection("My Ssl NoVerify", in: read()).sslmode, "require")
    }

    func testAMySQLSSLHandlerThatVerifiesTheServerIsSkippedByName() throws {
        let batch = try read()
        XCTAssertNil(batch.connections.first { $0.name == "My Ssl Verify" })
        let skipped = try XCTUnwrap(batch.skipped.first { $0.name == "My Ssl Verify" })
        XCTAssertTrue(skipped.reason.contains("verifyServerCertificate=true"), skipped.reason)
    }

    func testAnEnabledPostgresSSLHandlerWithNoModeIsVerifyFull() throws {
        XCTAssertEqual(try connection("Pg Ssl", in: read()).sslmode, "verify-full")
    }

    func testUseSSLFalseIsDisableWhichNeverLowersAnything() throws {
        let plain = try connection("My Plain", in: read())
        XCTAssertEqual(plain.sslmode, "disable")
        XCTAssertEqual(NavicatImport.tlsMode(kind: .mysql, requested: "disable", current: "require"), "require")
        XCTAssertEqual(NavicatImport.tlsMode(kind: .mysql, requested: "disable", current: ""), "", "MySQL's own default is already disable")
    }

    func testAMySQLCertificateVerificationThisAppCannotDoIsSkippedByName() throws {
        let batch = try read()
        XCTAssertNil(batch.connections.first { $0.name == "My Verify" })
        let skipped = try XCTUnwrap(batch.skipped.first { $0.name == "My Verify" })
        XCTAssertTrue(skipped.reason.contains("sslMode=VERIFY_CA"), skipped.reason)
    }

    func testTrinoSSLBecomesHttpsAndNoneIsTheOnlyUnverifiedAnswer() throws {
        let trino = try connection("Trino TLS", in: read())
        XCTAssertEqual(trino.kind, .trino)
        XCTAssertEqual(trino.trinoScheme, "https")
        XCTAssertEqual(trino.trinoVerify, false)
        XCTAssertEqual(trino.database, "hive")
        XCTAssertEqual(trino.schema, "default")
        XCTAssertEqual(trino.port, 8443)
    }

    func testTLSWordsOfTheThreeDrivers() throws {
        func tls(_ kind: ConnectionKind, _ query: [String: String], floor: Bool = false) throws -> JDBCImport.TLS {
            try JDBCImport.tls(kind: kind, query: query, port: 5432, floor: floor)
        }
        XCTAssertEqual(try tls(.postgres, ["sslmode": "allow"]).sslmode, "prefer", "never weaker than allow")
        XCTAssertEqual(try tls(.postgres, ["ssl": "true"]).sslmode, "verify-full", "pgjdbc's ssl=true verifies")
        XCTAssertEqual(try tls(.postgres, ["sslmode": "verify_ca"]).sslmode, "verify-ca")
        XCTAssertThrowsError(try tls(.postgres, ["sslmode": "bogus"]))
        XCTAssertEqual(try tls(.mysql, ["sslmode": "REQUIRED"]).sslmode, "require")
        XCTAssertEqual(try tls(.mysql, ["usessl": "true"]).sslmode, "require")
        XCTAssertThrowsError(try tls(.mysql, ["sslmode": "VERIFY_IDENTITY"]))
        XCTAssertThrowsError(try tls(.mysql, ["usessl": "true", "verifyservercertificate": "true"]))
        XCTAssertEqual(try tls(.trino, [:]).scheme, "http")
        XCTAssertEqual(try tls(.trino, ["ssl": "false"]).scheme, "http")
        XCTAssertEqual(try tls(.trino, [:], floor: true).verify, true)
    }

    // MARK: What is left out

    func testAnEntryWithNoDriverHereOrNoHostIsNamedAndNotImported() throws {
        let batch = try read()
        XCTAssertEqual(Set(batch.skipped.map(\.name)), ["A SQLite", "My Verify", "My Ssl Verify", "No Host"])
        XCTAssertTrue(try XCTUnwrap(batch.skipped.first { $0.name == "A SQLite" }).reason.contains("no driver"))
        XCTAssertTrue(try XCTUnwrap(batch.skipped.first { $0.name == "No Host" }).reason.contains("no host"))
    }

    func testAMissingOrDamagedCredentialsFileIsSaidAndNothingIsInvented() throws {
        let without = try DBeaverImport.read(dataSources: Data(dataSources().utf8), credentials: nil)
        XCTAssertNil(try connection("Prod PG", in: without).password)
        XCTAssertTrue(without.notes.joined().contains("No credentials-config.json"))
        let damaged = try DBeaverImport.read(dataSources: Data(dataSources().utf8), credentials: Data(repeating: 7, count: 64))
        XCTAssertNil(try connection("Prod PG", in: damaged).password)
        XCTAssertTrue(damaged.notes.joined().contains("couldn't be decrypted"))
    }

    func testAFileThatIsNotDBeaversIsRefused() {
        XCTAssertThrowsError(try DBeaverImport.read(dataSources: Data("not json".utf8), credentials: nil))
        XCTAssertThrowsError(try DBeaverImport.read(dataSources: Data(#"{"connections":{}}"#.utf8), credentials: nil))
    }

    func testAFolderIsSearchedForDataSources() throws {
        let root = try folder()
        let batch = try DBeaverImport.read(root)
        XCTAssertEqual(try connection("Prod PG", in: batch).password, "db-pass")
    }

    // MARK: Importing into the model

    @MainActor
    private func model() -> (AppModel, MemorySecretStore) {
        isolateConnectionStore()
        return (AppModel(persistsSession: false), useMemorySecretStore())
    }

    @MainActor
    func testAnImportAddsGroupsTunnelsSecretsEnvironmentAndTLS() throws {
        let (model, store) = model()
        model.importDBeaverConnections(from: try folder())

        let prod = try XCTUnwrap(model.connections.first { $0.name == "Prod PG" })
        XCTAssertEqual(prod.kind, .postgres)
        XCTAssertEqual(prod.sslmode, "verify-full")
        XCTAssertEqual(prod.environment, .prod)
        XCTAssertEqual(prod.sshHost, "bastion.corp")
        XCTAssertEqual(prod.sshAuth, .password)
        let group = try XCTUnwrap(model.groups.first { $0.id == prod.group })
        XCTAssertEqual(group.name, "Prod / EU")
        XCTAssertEqual(try store.get(slot: .database, for: prod.id), "db-pass")
        XCTAssertEqual(try store.get(slot: .sshPassword, for: prod.id), "ssh-pass")

        let key = try XCTUnwrap(model.connections.first { $0.name == "Key PG" })
        XCTAssertEqual(try store.get(slot: .sshPassphrase, for: key.id), "key-phrase")

        let trino = try XCTUnwrap(model.connections.first { $0.name == "Trino TLS" })
        XCTAssertEqual(trino.scheme, "https")
        XCTAssertFalse(trino.verify)

        let plain = try XCTUnwrap(model.connections.first { $0.name == "My Plain" })
        XCTAssertEqual(plain.sslmode, "")

        XCTAssertNil(model.connections.first { $0.name == "My Verify" })
        let summary = try XCTUnwrap(model.notice?.message)
        XCTAssertTrue(summary.contains("Skipped:"), summary)
        XCTAssertTrue(summary.contains("My Verify"), summary)
        XCTAssertTrue(summary.contains("New groups: Prod / EU"), summary)
        XCTAssertEqual(model.notice?.title.hasSuffix("from DBeaver"), true)
    }

    @MainActor
    func testReImportingNeverLowersTLSNeverReplacesASavedPasswordAndNeverMovesAFiledConnection() throws {
        let (model, store) = model()
        let mine = ConnectionGroup(name: "Mine")
        let saved = Connection(id: UUID(), name: "Prod PG", color: .blue, kind: .postgres, host: "pg.corp",
                               port: 5433, sslmode: "verify-full", user: "ana", database: "app", schema: "",
                               verify: true, group: mine.id, environment: .dev)
        model.groups = [mine]
        model.connections = [saved]
        try store.set("tightened-since", slot: .database, for: saved.id)

        model.importDBeaverConnections(from: try folder())

        XCTAssertEqual(model.connections.filter { $0.name == "Prod PG" }.count, 1, "refreshed in place")
        let after = try XCTUnwrap(model.connections.first { $0.id == saved.id })
        XCTAssertEqual(after.sslmode, "verify-full")
        XCTAssertEqual(after.group, mine.id, "an import never moves a connection the person filed")
        XCTAssertEqual(after.environment, .dev)
        XCTAssertEqual(try store.get(slot: .database, for: saved.id), "tightened-since")
    }

    @MainActor
    func testAnImportRaisesAWeakerExistingMode() throws {
        let (model, _) = model()
        let saved = Connection(id: UUID(), name: "My Ssl", color: .blue, kind: .mysql, host: "s.corp",
                               port: 3306, sslmode: "disable", user: "", database: "", schema: "", verify: true)
        model.connections = [saved]
        model.importDBeaverConnections(from: try folder())
        XCTAssertEqual(model.connections.first { $0.id == saved.id }?.sslmode, "require")
    }
}
