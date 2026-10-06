import CommonCrypto
import XCTest

@testable import QueryHive

/// Navicat's `.ncx`: the tunnel (read from attributes verified against two real exports), TLS that is
/// never lowered, and credentials that are never overwritten (PF-6).
final class NavicatImportTests: XCTestCase {
    /// Navicat's own cipher, run forwards, so a test file can carry a password the importer decrypts.
    private func navicatHex(_ plain: String) -> String {
        let key = Data("libcckeylibcckey".utf8)
        let iv = Data("libcciv libcciv ".utf8)
        let input = Data(plain.utf8)
        var out = Data(count: input.count + kCCBlockSizeAES128)
        var moved = 0
        let status = out.withUnsafeMutableBytes { outBytes in
            input.withUnsafeBytes { inBytes in
                key.withUnsafeBytes { keyBytes in
                    iv.withUnsafeBytes { ivBytes in
                        CCCrypt(CCOperation(kCCEncrypt), CCAlgorithm(kCCAlgorithmAES), CCOptions(kCCOptionPKCS7Padding),
                                keyBytes.baseAddress, key.count, ivBytes.baseAddress,
                                inBytes.baseAddress, input.count, outBytes.baseAddress, outBytes.count, &moved)
                    }
                }
            }
        }
        XCTAssertEqual(Int(status), Int(kCCSuccess))
        return out.prefix(moved).map { String(format: "%02X", $0) }.joined()
    }

    private func export(_ entries: [String]) throws -> URL {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("qh-\(UUID().uuidString).ncx")
        let xml = #"<?xml version="1.0" encoding="UTF-8"?><Connections Ver="1.5">"# + entries.joined() + "</Connections>"
        try xml.write(to: url, atomically: true, encoding: .utf8)
        addTeardownBlock { try? FileManager.default.removeItem(at: url) }
        return url
    }

    private func entry(name: String, type: String = "POSTGRESQL", host: String = "pg.internal", ssl: String = "false",
                       password: String = "db-pass", extra: String = "") -> String {
        #"<Connection ConnectionName="\#(name)" ConnType="\#(type)" Host="\#(host)" Port="5432" Database="app" UserName="ana" Password="\#(navicatHex(password))" SavePassword="true" SSL="\#(ssl)" \#(extra)/>"#
    }

    // MARK: Reading the tunnel

    func testATunnelledEntryCarriesItsHostPortUserMethodAndSecrets() throws {
        let extra = #"SSH="true" SSH_Host="bastion.corp" SSH_Port="4322" SSH_UserName="deploy" SSH_AuthenMethod="PASSWORD" SSH_Password="\#(navicatHex("ssh-pass"))" SSH_SavePassword="true""#
        let read = try NavicatImport.read(export([entry(name: "A", extra: extra)]))
        let imported = try XCTUnwrap(read.connections.first)
        XCTAssertEqual(imported.sshHost, "bastion.corp")
        XCTAssertEqual(imported.sshPort, 4322)
        XCTAssertEqual(imported.sshUser, "deploy")
        XCTAssertEqual(imported.sshAuth, .password)
        XCTAssertEqual(imported.sshPassword, "ssh-pass")
        XCTAssertNil(imported.sshPassphrase)
        XCTAssertEqual(imported.password, "db-pass")
        XCTAssertTrue(imported.notImported.isEmpty)
    }

    func testAKeyEntryCarriesTheKeyFileAndItsPassphrase() throws {
        let extra = #"SSH="true" SSH_Host="b" SSH_AuthenMethod="PUBLICKEY" SSH_PrivateKey="/Users/x/.ssh/id" SSH_Passphrase="\#(navicatHex("phrase"))" SSH_SavePassphrase="true""#
        let imported = try XCTUnwrap(NavicatImport.read(export([entry(name: "A", extra: extra)])).connections.first)
        XCTAssertEqual(imported.sshAuth, .key)
        XCTAssertEqual(imported.sshKeyPath, "/Users/x/.ssh/id")
        XCTAssertEqual(imported.sshPassphrase, "phrase")
        XCTAssertNil(imported.sshPassword, "a password for a key tunnel is not a thing")
    }

    func testATunnelSwitchedOffIsNotATunnelEvenWithAStaleHostInTheFile() throws {
        let extra = #"SSH="false" SSH_Host="stale.example" SSH_UserName="old""#
        let imported = try XCTUnwrap(NavicatImport.read(export([entry(name: "A", extra: extra)])).connections.first)
        XCTAssertEqual(imported.sshHost, "")
        XCTAssertEqual(imported.sshUser, "")
    }

    func testAMethodThisImportDoesNotKnowIsNamedAndFallsBackToTheAgent() throws {
        let extra = #"SSH="true" SSH_Host="b" SSH_AuthenMethod="Kerberos""#
        let imported = try XCTUnwrap(NavicatImport.read(export([entry(name: "A", extra: extra)])).connections.first)
        XCTAssertEqual(imported.sshAuth, .agent, "the one method that sends no secret")
        XCTAssertEqual(imported.notImported, ["SSH method Kerberos"])
    }

    func testAPasswordNavicatDidNotSaveIsNotInvented() throws {
        let extra = #"SSH="true" SSH_Host="b" SSH_AuthenMethod="PASSWORD" SSH_Password="\#(navicatHex("x"))" SSH_SavePassword="false""#
        let imported = try XCTUnwrap(NavicatImport.read(export([entry(name: "A", extra: extra)])).connections.first)
        XCTAssertNil(imported.sshPassword)
    }

    func testSSLIsReadFromTheEntry() throws {
        let read = try NavicatImport.read(export([entry(name: "On", ssl: "true"), entry(name: "Off", ssl: "false")]))
        XCTAssertEqual(read.connections.map(\.ssl), [true, false])
    }

    // MARK: TLS only ever goes up

    func testAnEntryWithSSLRaisesToAtLeastRequireAndNeverLowers() {
        XCTAssertEqual(NavicatImport.tlsMode(kind: .postgres, ssl: true, current: ""), "require", "default prefer is below require")
        XCTAssertEqual(NavicatImport.tlsMode(kind: .postgres, ssl: true, current: "prefer"), "require")
        XCTAssertEqual(NavicatImport.tlsMode(kind: .postgres, ssl: true, current: "disable"), "require")
        XCTAssertEqual(NavicatImport.tlsMode(kind: .mysql, ssl: true, current: ""), "require", "MySQL's default is disable")
        for stricter in ["require", "verify-ca", "verify-full"] {
            XCTAssertEqual(NavicatImport.tlsMode(kind: .postgres, ssl: true, current: stricter), stricter)
        }
        XCTAssertEqual(NavicatImport.tlsMode(kind: .postgres, ssl: true, current: "future-word"), "future-word",
                       "a word this build does not know may be a stricter one")
    }

    func testAnEntryWithoutSSLChangesNothing() {
        for current in ["", "disable", "prefer", "require", "verify-full"] {
            XCTAssertEqual(NavicatImport.tlsMode(kind: .postgres, ssl: false, current: current), current)
        }
        XCTAssertEqual(NavicatImport.tlsMode(kind: .trino, ssl: true, current: ""), "", "Trino has a transport, not a mode")
    }

    // MARK: Importing into the model

    @MainActor
    private func model() -> (AppModel, MemorySecretStore) {
        isolateConnectionStore()
        return (AppModel(persistsSession: false), useMemorySecretStore())
    }

    @MainActor
    func testANewTunnelledEntryArrivesWithItsTunnelItsSecretsAndTLS() throws {
        let (model, store) = model()
        let extra = #"SSH="true" SSH_Host="bastion.corp" SSH_Port="22" SSH_UserName="deploy" SSH_AuthenMethod="PASSWORD" SSH_Password="\#(navicatHex("ssh-pass"))""#
        model.importNavicatConnections(from: try export([entry(name: "Prod", ssl: "true", extra: extra)]))

        let added = try XCTUnwrap(model.connections.first { $0.name == "Prod" })
        XCTAssertEqual(added.sshHost, "bastion.corp")
        XCTAssertEqual(added.sshUser, "deploy")
        XCTAssertEqual(added.sshAuth, .password)
        XCTAssertEqual(added.sslmode, "require")
        XCTAssertEqual(try store.get(slot: .database, for: added.id), "db-pass")
        XCTAssertEqual(try store.get(slot: .sshPassword, for: added.id), "ssh-pass")
        let summary = try XCTUnwrap(model.notice?.message)
        XCTAssertTrue(summary.contains("SSH tunnel imported for: Prod"), summary)
        XCTAssertTrue(summary.contains("fingerprint"), summary)
        XCTAssertFalse(summary.contains("doesn't open"), "the old line that said tunnels cannot work is gone")
    }

    @MainActor
    func testReImportingAStaleFileNeverLowersTLSAndNeverReplacesASavedPassword() throws {
        let (model, store) = model()
        let saved = Connection(id: UUID(), name: "Prod", color: .blue, kind: .postgres, host: "pg.internal",
                               port: 5432, sslmode: "verify-full", user: "ana", database: "app", schema: "", verify: true)
        model.connections = [saved]
        try store.set("tightened-since", slot: .database, for: saved.id)

        // The old export: SSL off, and the password the connection had back then.
        model.importNavicatConnections(from: try export([entry(name: "Prod", ssl: "false", password: "stale-password")]))

        let after = try XCTUnwrap(model.connections.first { $0.id == saved.id })
        XCTAssertEqual(after.sslmode, "verify-full", "an import never lowers TLS")
        XCTAssertEqual(try store.get(slot: .database, for: saved.id), "tightened-since",
                       "re-importing a stale file leaves the saved password alone")
        XCTAssertEqual(model.connections.count, 1, "refreshed in place, not duplicated")
        XCTAssertTrue(try XCTUnwrap(model.notice?.message).contains("Kept the secret already saved for: Prod"))
    }

    @MainActor
    func testAnSSLEntryRaisesAnExistingConnectionFromTheDefaultButKeepsItsPassword() throws {
        let (model, store) = model()
        let saved = Connection(id: UUID(), name: "Prod", color: .blue, kind: .postgres, host: "pg.internal",
                               port: 5432, sslmode: "", user: "ana", database: "app", schema: "", verify: true)
        model.connections = [saved]
        try store.set("mine", slot: .database, for: saved.id)
        model.importNavicatConnections(from: try export([entry(name: "Prod", ssl: "true", password: "theirs")]))
        XCTAssertEqual(model.connections.first?.sslmode, "require")
        XCTAssertEqual(try store.get(slot: .database, for: saved.id), "mine")
    }

    @MainActor
    func testAnExistingConnectionWithNoSavedPasswordGetsTheImportedOne() throws {
        let (model, store) = model()
        let saved = Connection(id: UUID(), name: "Prod", color: .blue, kind: .postgres, host: "pg.internal",
                               port: 5432, user: "ana", database: "app", schema: "", verify: true)
        model.connections = [saved]
        model.importNavicatConnections(from: try export([entry(name: "Prod", password: "from-file")]))
        XCTAssertEqual(try store.get(slot: .database, for: saved.id), "from-file", "an empty slot is not a credential to protect")
    }

    @MainActor
    func testATunnelTheConnectionAlreadyHasIsNotReplaced() throws {
        let (model, store) = model()
        let saved = Connection(id: UUID(), name: "Prod", color: .blue, kind: .postgres, host: "pg.internal",
                               port: 5432, user: "ana", database: "app", schema: "", verify: true,
                               sshHost: "my-own-bastion", sshUser: "me", sshAuth: .key, sshKeyPath: "/k")
        model.connections = [saved]
        let extra = #"SSH="true" SSH_Host="their-bastion" SSH_UserName="them" SSH_AuthenMethod="PASSWORD" SSH_Password="\#(navicatHex("x"))""#
        model.importNavicatConnections(from: try export([entry(name: "Prod", extra: extra)]))
        let after = try XCTUnwrap(model.connections.first)
        XCTAssertEqual(after.sshHost, "my-own-bastion")
        XCTAssertEqual(after.sshAuth, .key)
        XCTAssertNil(try store.get(slot: .sshPassword, for: saved.id))
    }

    // MARK: B-6 and the other slots on duplicate and delete

    @MainActor
    func testDuplicatingUnderBenchWritesNothingToTheKeychain() throws {
        let (model, store) = model()
        AppModel.benchPassword = "bench-stand-in"
        defer { AppModel.benchPassword = nil }
        let original = Connection(id: UUID(), name: "Bench", color: .blue, kind: .postgres, host: "h", port: 5432,
                                  user: "u", database: "d", schema: "", verify: true)
        model.connections = [original]
        model.duplicateConnection(original.id)
        XCTAssertEqual(model.connections.count, 2)
        XCTAssertTrue(store.accounts.isEmpty, "a benchmark's stand-in password must not reach a login keychain: \(store.accounts)")
    }

    @MainActor
    func testDuplicatingCopiesAllFourSlotsAndDeletingRemovesAllFour() throws {
        let (model, store) = model()
        let original = Connection(id: UUID(), name: "Prod", color: .blue, kind: .trino, host: "h", port: 8443,
                                  user: "u", database: "d", schema: "", verify: true)
        model.connections = [original]
        for slot in ConnectionKeychain.Slot.allCases { try store.set("v-\(slot)", slot: slot, for: original.id) }

        model.duplicateConnection(original.id)
        let copy = try XCTUnwrap(model.connections.first { $0.id != original.id })
        for slot in ConnectionKeychain.Slot.allCases {
            XCTAssertEqual(try store.get(slot: slot, for: copy.id), "v-\(slot)", "\(slot)")
        }

        model.deleteConnection(copy.id)
        for slot in ConnectionKeychain.Slot.allCases {
            XCTAssertFalse(store.contains(slot: slot, for: copy.id), "\(slot) was left behind")
            XCTAssertTrue(store.contains(slot: slot, for: original.id), "\(slot) of another connection was touched")
        }
    }
}
