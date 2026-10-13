import XCTest

@testable import QueryHive

/// What an import writes for PostgreSQL and MySQL certificate verification.
///
/// The engine turns `verify == false` into `DB_INSECURE=1`, and that turns every mode but `disable`
/// into "encrypt, never check" (`EngineTLSMode`). An import that wrote `verify: false` for a source
/// that asked for `verify-ca` or `verify-full` therefore ran without the certificate check it was
/// asked for, and said nothing. These tests read what the engine would be sent, not only the stored
/// fields.
///
/// The rule under test, for the `sslmode` the import ends on:
/// - a mode that checks the certificate (`verify-ca`, `verify-full`) or `require` gets `verify: true`;
/// - `prefer` keeps `verify: false` as it is today: with `true` the engine would run the real
///   `prefer`, which may fall back to clear text, and an import never loosens (PF-6);
/// - a source that names no mode, or `disable`, changes nothing;
/// - `verify` is only ever written together with a raise of the mode, never to `false`, and a
///   stored connection that already has `verify: false` is not rewritten.
final class ImportTLSVerificationTests: XCTestCase {
    @MainActor
    private func model() -> AppModel {
        isolateConnectionStore()
        _ = useMemorySecretStore()
        return AppModel(persistsSession: false)
    }

    private func imported(_ kind: ConnectionKind, sslmode: String? = nil, ssl: Bool = false,
                          name: String = "Prod") -> ImportedConnection {
        var value = ImportedConnection(name: name, kind: kind, host: "db.internal", port: kind.defaultPort,
                                       user: "ana", database: "app", schema: "", password: nil, remarks: "",
                                       sourceType: "test", sshHost: "")
        value.sslmode = sslmode
        value.ssl = ssl
        return value
    }

    private func stored(_ kind: ConnectionKind, sslmode: String, verify: Bool, name: String = "Prod") -> Connection {
        Connection(id: UUID(), name: name, color: .blue, kind: kind, host: "db.internal", port: kind.defaultPort,
                   sslmode: sslmode, user: "ana", database: "app", schema: "", verify: verify)
    }

    private func environment(_ connection: Connection) -> [String: String] {
        AppModel.connectionEnvironment(connection, secrets: ConnectionSecrets())
    }

    // MARK: The rule itself, per mode

    /// A new connection: (what the source asked, the `sslmode` stored, `verify`, what the engine runs).
    private typealias Row = (asked: String?, sslmode: String, verify: Bool, engine: EngineTLSMode?)

    private let postgresNew: [Row] = [
        (nil, "", false, nil),
        ("disable", "", false, nil),
        // PostgreSQL's own default is `prefer`, so asking for it changes nothing and `verify` stays off.
        ("prefer", "", false, .requireNoVerify),
        ("require", "require", true, .requireNoVerify),
        ("verify-ca", "verify-ca", true, .require),
        ("verify-full", "verify-full", true, .require),
    ]

    private let mysqlNew: [Row] = [
        (nil, "", false, nil),
        ("disable", "", false, nil),
        ("prefer", "prefer", false, .requireNoVerify),
        ("require", "require", true, .requireNoVerify),
    ]

    private func assertNew(_ kind: ConnectionKind, _ rows: [Row], file: StaticString = #filePath, line: UInt = #line) {
        for row in rows {
            var connection = stored(kind, sslmode: "", verify: false)
            AppModel.applyTLS(imported(kind, sslmode: row.asked), to: &connection, isNew: true)
            let label = "\(kind) asked \(row.asked ?? "nothing")"
            XCTAssertEqual(connection.sslmode, row.sslmode, label, file: file, line: line)
            XCTAssertEqual(connection.verify, row.verify, label, file: file, line: line)
            if let engine = row.engine {
                XCTAssertEqual(EngineTLSMode(connection), engine, label, file: file, line: line)
            }
        }
    }

    func testANewPostgreSQLConnectionVerifiesWhenTheSourceAsksForAVerifyingMode() {
        assertNew(.postgres, postgresNew)
    }

    func testANewMySQLConnectionVerifiesWhenTheSourceAsksForAVerifyingMode() {
        assertNew(.mysql, mysqlNew)
    }

    func testVerifyFullAndVerifyCAReachTheEngineAsAModeThatChecksTheCertificate() {
        for word in ["verify-ca", "verify-full"] {
            var connection = stored(.postgres, sslmode: "", verify: false)
            AppModel.applyTLS(imported(.postgres, sslmode: word), to: &connection, isNew: true)
            let env = environment(connection)
            XCTAssertEqual(env["DB_SSLMODE"], word)
            XCTAssertEqual(env["DB_INSECURE"], "", "\(word) must not be sent with the flag that switches the check off")
            XCTAssertEqual(EngineTLSMode(environment: env), .require, word)
        }
    }

    func testNavicatsYesNoSSLBecomesRequireWithVerifyOnAndNoSSLChangesNothing() {
        for kind in [ConnectionKind.postgres, .mysql] {
            var on = stored(kind, sslmode: "", verify: false)
            AppModel.applyTLS(imported(kind, ssl: true), to: &on, isNew: true)
            XCTAssertEqual(on.sslmode, "require", "\(kind)")
            XCTAssertTrue(on.verify, "\(kind)")
            XCTAssertEqual(EngineTLSMode(on), .requireNoVerify, "require encrypts and does not check, as libpq's does")

            var off = stored(kind, sslmode: "", verify: false)
            AppModel.applyTLS(imported(kind, ssl: false), to: &off, isNew: true)
            XCTAssertEqual(off.sslmode, "")
            XCTAssertFalse(off.verify, "a source that names no mode keeps what it had")
        }
    }

    // MARK: A merge never lowers, and only raises verify together with the mode

    func testAMergeThatRaisesTheModeToAVerifyingOneTurnsVerifyOn() {
        for (start, startVerify) in [("", false), ("", true), ("prefer", false), ("require", false), ("require", true)] {
            var connection = stored(.postgres, sslmode: start, verify: startVerify)
            AppModel.applyTLS(imported(.postgres, sslmode: "verify-full"), to: &connection, isNew: false)
            XCTAssertEqual(connection.sslmode, "verify-full", "from \(start.isEmpty ? "default" : start)")
            XCTAssertTrue(connection.verify, "from \(start.isEmpty ? "default" : start), verify \(startVerify)")
            XCTAssertEqual(EngineTLSMode(connection), .require)
        }
    }

    func testAMergeNeverLowersTheModeOrTurnsVerifyOff() {
        let asks: [String?] = [nil, "disable", "prefer", "require", "verify-ca", "verify-full"]
        let rank = ["disable": 0, "prefer": 2, "require": 3, "verify-ca": 4, "verify-full": 5]
        for kind in [ConnectionKind.postgres, .mysql] {
            let modes = kind == .postgres ? ["verify-full", "verify-ca", "require"] : ["require"]
            for current in modes {
                for asked in asks {
                    var connection = stored(kind, sslmode: current, verify: true)
                    AppModel.applyTLS(imported(kind, sslmode: asked), to: &connection, isNew: false)
                    let label = "\(kind) \(current) asked \(asked ?? "nothing")"
                    XCTAssertGreaterThanOrEqual(rank[connection.sslmode] ?? -1, rank[current] ?? 99, label)
                    XCTAssertTrue(connection.verify, label)
                }
            }
        }
    }

    func testAStoredUnverifiedConnectionIsVerifiedWhenTheSourceAsksForAVerifyingModeAtOrAboveIt() {
        // Made by an older import: `verify-full` stored with `verify: false`. Re-importing the same
        // source is the owner asking for that check again, so it goes on. A rejected certificate then
        // fails loudly at connect: the drivers name it.
        for (stored, asked) in [("verify-full", "verify-full"), ("verify-ca", "verify-ca"), ("verify-ca", "verify-full")] {
            var connection = self.stored(.postgres, sslmode: stored, verify: false)
            AppModel.applyTLS(imported(.postgres, sslmode: asked), to: &connection, isNew: false)
            XCTAssertEqual(connection.sslmode, asked == "verify-full" ? "verify-full" : stored, "\(stored) asked \(asked)")
            XCTAssertTrue(connection.verify, "\(stored) asked \(asked)")
            XCTAssertEqual(EngineTLSMode(connection), .require, "\(stored) asked \(asked)")
        }
    }

    func testAStoredUnverifiedConnectionIsLeftAloneWhenTheSourceAsksForLessThanItHas() {
        // The source asks for less than the stored mode, or for a mode that does not check a
        // certificate, or for nothing: nothing about the stored connection changes.
        for asked in [nil, "disable", "prefer", "require", "verify-ca"] as [String?] {
            var connection = stored(.postgres, sslmode: "verify-full", verify: false)
            AppModel.applyTLS(imported(.postgres, sslmode: asked), to: &connection, isNew: false)
            XCTAssertEqual(connection.sslmode, "verify-full", "asked \(asked ?? "nothing")")
            XCTAssertFalse(connection.verify, "asked \(asked ?? "nothing")")
        }
        var requireOnly = stored(.postgres, sslmode: "require", verify: false)
        AppModel.applyTLS(imported(.postgres, sslmode: "require"), to: &requireOnly, isNew: false)
        XCTAssertFalse(requireOnly.verify, "require checks nothing, so there is nothing to turn on")
        let unverified = ConnectionTLSChoice(kind: .postgres, scheme: "", verify: false, sslmode: "verify-full")
        XCTAssertEqual(unverified.title, "verify-full · unverified")
    }

    // MARK: Trino is unchanged

    func testTrinoKeepsItsOwnTransportAndCertificateRules() {
        var plain = ImportedConnection(name: "Lake", kind: .trino, host: "t", port: 8443, user: "u", database: "hive",
                                       schema: "default", password: nil, remarks: "", sourceType: "t", sshHost: "")
        plain.trinoScheme = "https"
        plain.trinoVerify = false
        var fresh = stored(.trino, sslmode: "", verify: false)
        AppModel.applyTLS(plain, to: &fresh, isNew: true)
        XCTAssertEqual(fresh.scheme, "https")
        XCTAssertFalse(fresh.verify, "the source said NONE")

        plain.trinoVerify = nil
        var checked = stored(.trino, sslmode: "", verify: false)
        AppModel.applyTLS(plain, to: &checked, isNew: true)
        XCTAssertTrue(checked.verify, "no answer is a verified HTTPS connection")

        var http = stored(.trino, sslmode: "", verify: true)
        http.scheme = "http"
        AppModel.applyTLS(plain, to: &http, isNew: false)
        XCTAssertEqual(http.scheme, "https", "an HTTP connection is lifted")

        var already = stored(.trino, sslmode: "", verify: false)
        already.scheme = "https"
        plain.trinoVerify = true
        AppModel.applyTLS(plain, to: &already, isNew: false)
        XCTAssertFalse(already.verify, "an HTTPS connection is left alone, as the owner set it")

        // A PostgreSQL mode on a Trino row cannot happen, but the rule must not reach Trino.
        let notTrino = imported(.trino, sslmode: "verify-full")
        var untouched = stored(.trino, sslmode: "", verify: false)
        AppModel.applyTLS(notTrino, to: &untouched, isNew: true)
        XCTAssertFalse(untouched.verify)
        XCTAssertEqual(untouched.sslmode, "")
    }

    // MARK: Each importer, end to end through the model

    private struct Expect {
        var name: String
        var sslmode: String
        var verify: Bool
        var engine: EngineTLSMode?
    }

    @MainActor
    private func check(_ batch: ImportBatch, source: String, _ expected: [Expect],
                       file: StaticString = #filePath, line: UInt = #line) throws {
        let model = model()
        model.applyImport(batch, source: source)
        for want in expected {
            let found = try XCTUnwrap(model.connections.first { $0.name == want.name },
                                      "\(want.name) was not imported: \(batch.skipped)", file: file, line: line)
            XCTAssertEqual(found.sslmode, want.sslmode, want.name, file: file, line: line)
            XCTAssertEqual(found.verify, want.verify, want.name, file: file, line: line)
            let env = environment(found)
            XCTAssertEqual(env["DB_INSECURE"], want.verify ? "" : "1", want.name, file: file, line: line)
            XCTAssertEqual(env["DB_SSLMODE"], found.sslmode.isEmpty ? found.kind.defaultSSLMode : found.sslmode,
                           want.name, file: file, line: line)
            if let engine = want.engine {
                XCTAssertEqual(EngineTLSMode(environment: env), engine, want.name, file: file, line: line)
            }
        }
    }

    private func ncx(_ entries: [(String, String, String)]) throws -> ImportBatch {
        let rows = entries.map { name, type, ssl in
            #"<Connection ConnectionName="\#(name)" ConnType="\#(type)" Host="h-\#(name).corp" Port="5432" Database="app" UserName="ana" SavePassword="false" SSL="\#(ssl)"/>"#
        }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("qh-tls-\(UUID().uuidString).ncx")
        try (#"<?xml version="1.0" encoding="UTF-8"?><Connections Ver="1.5">"# + rows.joined() + "</Connections>")
            .write(to: url, atomically: true, encoding: .utf8)
        addTeardownBlock { try? FileManager.default.removeItem(at: url) }
        let export = try NavicatImport.read(url)
        return ImportBatch(connections: export.connections, skipped: export.skipped)
    }

    @MainActor
    func testNavicatImportsKeepTheCertificateCheckTheirSSLFlagImplies() throws {
        let batch = try ncx([("pg-on", "POSTGRESQL", "true"), ("pg-off", "POSTGRESQL", "false"),
                             ("my-on", "MYSQL", "true"), ("my-off", "MYSQL", "false")])
        try check(batch, source: "Navicat", [
            Expect(name: "pg-on", sslmode: "require", verify: true, engine: .requireNoVerify),
            Expect(name: "pg-off", sslmode: "", verify: false),
            Expect(name: "my-on", sslmode: "require", verify: true, engine: .requireNoVerify),
            Expect(name: "my-off", sslmode: "", verify: false),
        ])
    }

    private func dbeaver(_ entries: [(name: String, driver: String, url: String)]) throws -> ImportBatch {
        let connections = entries.enumerated().map { index, entry in
            #""c\#(index)": { "provider": "x", "driver": "\#(entry.driver)", "name": "\#(entry.name)", "configuration": { "host": "h\#(index).corp", "port": 5432, "database": "d", "url": "\#(entry.url)" } }"#
        }
        let json = "{ \"connections\": { " + connections.joined(separator: ", ") + " } }"
        return try DBeaverImport.read(dataSources: Data(json.utf8), credentials: nil)
    }

    @MainActor
    func testDBeaverImportsKeepTheCertificateCheckTheSourceAskedFor() throws {
        let pg = "jdbc:postgresql://h.corp:5432/d"
        let my = "jdbc:mysql://h.corp:3306/d"
        let batch = try dbeaver([
            ("pg-full", "postgres-jdbc", pg + "?sslmode=verify-full"),
            ("pg-ca", "postgres-jdbc", pg + "?sslmode=verify-ca"),
            ("pg-require", "postgres-jdbc", pg + "?sslmode=require"),
            ("pg-prefer", "postgres-jdbc", pg + "?sslmode=prefer"),
            ("pg-disable", "postgres-jdbc", pg + "?sslmode=disable"),
            ("pg-none", "postgres-jdbc", pg),
            ("pg-ssl-true", "postgres-jdbc", pg + "?ssl=true"),
            ("my-require", "mysql8", my + "?sslMode=REQUIRED"),
            ("my-prefer", "mysql8", my + "?sslMode=PREFERRED"),
            ("my-disable", "mysql8", my + "?sslMode=DISABLED"),
            ("my-none", "mysql8", my),
            ("my-verify", "mysql8", my + "?sslMode=VERIFY_CA"),
        ])
        XCTAssertNotNil(batch.skipped.first { $0.name == "my-verify" },
                        "MySQL verification this app cannot do is skipped by name, not imported unverified")
        try check(batch, source: "DBeaver", [
            Expect(name: "pg-full", sslmode: "verify-full", verify: true, engine: .require),
            Expect(name: "pg-ca", sslmode: "verify-ca", verify: true, engine: .require),
            Expect(name: "pg-require", sslmode: "require", verify: true, engine: .requireNoVerify),
            Expect(name: "pg-prefer", sslmode: "", verify: false, engine: .requireNoVerify),
            Expect(name: "pg-disable", sslmode: "", verify: false),
            Expect(name: "pg-none", sslmode: "", verify: false),
            Expect(name: "pg-ssl-true", sslmode: "verify-full", verify: true, engine: .require),
            Expect(name: "my-require", sslmode: "require", verify: true, engine: .requireNoVerify),
            Expect(name: "my-prefer", sslmode: "prefer", verify: false, engine: .requireNoVerify),
            Expect(name: "my-disable", sslmode: "", verify: false),
            Expect(name: "my-none", sslmode: "", verify: false),
        ])
    }

    @MainActor
    func testDataGripImportsKeepTheCertificateCheckTheSourceAskedFor() throws {
        func source(_ name: String, _ ref: String, _ url: String) -> String {
            "<data-source source=\"LOCAL\" name=\"\(name)\" uuid=\"u-\(name)\"><driver-ref>\(ref)</driver-ref><jdbc-url>\(url)</jdbc-url></data-source>"
        }
        let pg = "jdbc:postgresql://h.corp:5432/d"
        let my = "jdbc:mysql://h.corp:3306/d"
        let xml = """
        <?xml version="1.0" encoding="UTF-8"?>
        <project version="4"><component name="DataSourceManagerImpl" format="xml" multifile-model="true">
        \(source("pg-full", "postgresql", pg + "?sslmode=verify-full"))
        \(source("pg-ca", "postgresql", pg + "?sslmode=verify-ca"))
        \(source("pg-require", "postgresql", pg + "?sslmode=require"))
        \(source("pg-prefer", "postgresql", pg + "?sslmode=prefer"))
        \(source("pg-disable", "postgresql", pg + "?sslmode=disable"))
        \(source("pg-none", "postgresql", pg))
        \(source("my-require", "mysql.8", my + "?sslMode=REQUIRED"))
        \(source("my-prefer", "mysql.8", my + "?sslMode=PREFERRED"))
        \(source("my-none", "mysql.8", my))
        </component></project>
        """
        let batch = try DataGripImport.read(dataSources: Data(xml.utf8), local: nil, forest: nil, keychain: nil)
        try check(batch, source: "DataGrip", [
            Expect(name: "pg-full", sslmode: "verify-full", verify: true, engine: .require),
            Expect(name: "pg-ca", sslmode: "verify-ca", verify: true, engine: .require),
            Expect(name: "pg-require", sslmode: "require", verify: true, engine: .requireNoVerify),
            Expect(name: "pg-prefer", sslmode: "", verify: false, engine: .requireNoVerify),
            Expect(name: "pg-disable", sslmode: "", verify: false),
            Expect(name: "pg-none", sslmode: "", verify: false),
            Expect(name: "my-require", sslmode: "require", verify: true, engine: .requireNoVerify),
            Expect(name: "my-prefer", sslmode: "prefer", verify: false, engine: .requireNoVerify),
            Expect(name: "my-none", sslmode: "", verify: false),
        ])
    }

    @MainActor
    func testReImportingIntoAnExistingConnectionRaisesVerifyWithTheModeAndNeverLowersIt() throws {
        let model = model()
        // `pg-full`: an old import left `require` + unverified. `pg-require`: already tight.
        // `pg-ca`: an old import left `verify-full` + unverified, and this file does not raise it.
        let seeds = [stored(.postgres, sslmode: "require", verify: false, name: "pg-full"),
                     stored(.postgres, sslmode: "verify-full", verify: true, name: "pg-require"),
                     stored(.postgres, sslmode: "verify-full", verify: false, name: "pg-ca")]
        let batch = try dbeaver([
            ("pg-full", "postgres-jdbc", "jdbc:postgresql://h.corp:5432/d?sslmode=verify-full"),
            ("pg-require", "postgres-jdbc", "jdbc:postgresql://h.corp:5432/d?sslmode=require"),
            ("pg-ca", "postgres-jdbc", "jdbc:postgresql://h.corp:5432/d?sslmode=verify-ca"),
        ])
        // The importer matches an existing connection on name, host and kind, so give the seeds the
        // hosts and ports the fixture entries carry.
        for var seed in seeds {
            let entry = try XCTUnwrap(batch.connections.first { $0.name == seed.name })
            seed.host = entry.host
            seed.port = entry.port
            model.connections.append(seed)
        }
        model.applyImport(batch, source: "DBeaver")

        let after = Dictionary(uniqueKeysWithValues: model.connections.map { ($0.name, $0) })
        XCTAssertEqual(model.connections.count, 3, "refreshed in place")
        XCTAssertEqual(after["pg-full"]?.sslmode, "verify-full")
        XCTAssertEqual(after["pg-full"]?.verify, true, "raised to a verifying mode, so the check is on")
        XCTAssertEqual(after["pg-require"]?.sslmode, "verify-full", "a looser file never lowers it")
        XCTAssertEqual(after["pg-require"]?.verify, true)
        XCTAssertEqual(after["pg-ca"]?.sslmode, "verify-full", "verify-ca is below what it has")
        XCTAssertEqual(after["pg-ca"]?.verify, false, "asking for less than the stored mode changes nothing")
    }

    @MainActor
    func testTheSummaryWarnsThatAVerifyingImportBringsNoCAFile() throws {
        let model = model()
        let batch = try dbeaver([
            ("pg-full", "postgres-jdbc", "jdbc:postgresql://h.corp:5432/d?sslmode=verify-full"),
            ("pg-ca", "postgres-jdbc", "jdbc:postgresql://h.corp:5432/d?sslmode=verify-ca"),
            ("pg-require", "postgres-jdbc", "jdbc:postgresql://h.corp:5432/d?sslmode=require"),
            ("pg-none", "postgres-jdbc", "jdbc:postgresql://h.corp:5432/d"),
        ])
        model.applyImport(batch, source: "DBeaver")
        let message = try XCTUnwrap(model.notice?.message)
        let line = try XCTUnwrap(message.components(separatedBy: "\n\n").first { $0.hasPrefix("Certificate check on for: ") }, message)
        XCTAssertTrue(line.contains("pg-full"), line)
        XCTAssertTrue(line.contains("pg-ca"), line)
        XCTAssertFalse(line.contains("pg-require"), line)
        XCTAssertFalse(line.contains("pg-none"), line)
        XCTAssertTrue(line.contains("host name"), line)
        XCTAssertTrue(line.contains("CA file"), line)

        let quiet = self.model()
        quiet.applyImport(try dbeaver([("pg-require", "postgres-jdbc", "jdbc:postgresql://h.corp:5432/d?sslmode=require")]),
                          source: "DBeaver")
        XCTAssertFalse(try XCTUnwrap(quiet.notice?.message).contains("Certificate check on"))
    }

    // MARK: QueryHive's own list carries `verify` verbatim

    @MainActor
    func testTheConnectionListImportCarriesVerifyAsExported() throws {
        let source = model()
        var unchecked = stored(.postgres, sslmode: "verify-full", verify: false, name: "unchecked")
        unchecked.host = "other.internal"
        source.connections = [stored(.postgres, sslmode: "verify-full", verify: true, name: "checked"), unchecked]
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("qh-list-\(UUID().uuidString).json")
        addTeardownBlock { try? FileManager.default.removeItem(at: url) }
        source.exportConnectionList(to: url)

        let target = model()
        target.importConnectionList(from: url)
        let byName = Dictionary(uniqueKeysWithValues: target.connections.map { ($0.name, $0) })
        XCTAssertEqual(byName["checked"]?.verify, true)
        XCTAssertEqual(byName["unchecked"]?.verify, false, "written as it was exported, not rewritten")
        XCTAssertEqual(byName["checked"]?.sslmode, "verify-full")
    }
}
