import XCTest

@testable import QueryHive

/// QueryHive's own connection list as a file: no secret in it, and an import that only adds (DBX-74).
final class ConnectionListTransferTests: XCTestCase {
    @MainActor
    private func model() -> (AppModel, MemorySecretStore) {
        isolateConnectionStore()
        return (AppModel(persistsSession: false), useMemorySecretStore())
    }

    private func tempFile() -> URL {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("qh-list-\(UUID().uuidString).json")
        addTeardownBlock { try? FileManager.default.removeItem(at: url) }
        return url
    }

    @MainActor
    func testTheExportCarriesNoSecretFromAnyOfTheFourSlots() throws {
        let (model, store) = model()
        let group = ConnectionGroup(name: "Prod")
        let unused = ConnectionGroup(name: "Empty")
        let saved = Connection(id: UUID(), name: "Prod PG", color: .red, kind: .postgres, host: "pg.corp", port: 5432,
                               sslmode: "verify-full", user: "ana", database: "app", schema: "", verify: true,
                               group: group.id, safeMode: .readOnly, environment: .prod, sshHost: "bastion",
                               sshAuth: .key, sshKeyPath: "/k")
        model.groups = [group, unused]
        model.connections = [saved]
        for slot in ConnectionKeychain.Slot.allCases { try store.set("SECRET-\(slot)", slot: slot, for: saved.id) }

        let url = tempFile()
        model.exportConnectionList(to: url)
        let text = try String(contentsOf: url, encoding: .utf8)
        XCTAssertFalse(text.contains("SECRET"), text)
        XCTAssertTrue(text.contains("\"format\" : \"queryhive-connections\""), text)
        XCTAssertTrue(text.contains("verify-full"))
        XCTAssertTrue(text.contains("read_only"))
        XCTAssertFalse(text.contains("Empty"), "a group nothing is filed under is not exported")
    }

    @MainActor
    func testARoundTripGivesFreshIdsAndRemapsGroupsAndWritesNothingToTheKeychain() throws {
        let (source, _) = model()
        let group = ConnectionGroup(name: "Prod")
        let saved = Connection(id: UUID(), name: "Prod PG", color: .red, kind: .postgres, host: "pg.corp", port: 5432,
                               sslmode: "verify-full", user: "ana", database: "app", schema: "", verify: true,
                               group: group.id, safeMode: .confirm)
        source.groups = [group]
        source.connections = [saved]
        let url = tempFile()
        source.exportConnectionList(to: url)

        let (target, store) = model()
        let local = ConnectionGroup(name: "Prod")
        target.groups = [local]
        target.importConnectionList(from: url)

        let added = try XCTUnwrap(target.connections.first)
        XCTAssertNotEqual(added.id, saved.id)
        XCTAssertEqual(added.group, local.id, "filed under the group of the same name that was already here")
        XCTAssertEqual(target.groups.count, 1)
        XCTAssertEqual(added.sslmode, "verify-full")
        XCTAssertEqual(added.safeMode, .confirm)
        XCTAssertTrue(store.accounts.isEmpty)
        XCTAssertTrue(try XCTUnwrap(target.notice?.message).contains("no passwords"))
    }

    @MainActor
    func testAnImportNeverChangesAConnectionThatIsAlreadyHere() throws {
        let (model, _) = model()
        let mine = Connection(id: UUID(), name: "Prod PG", color: .red, kind: .postgres, host: "pg.corp", port: 5432,
                              sslmode: "verify-full", user: "ana", database: "app", schema: "", verify: true,
                              safeMode: .readOnly)
        model.connections = [mine]
        // The same connection, as a looser file has it, and a different server under the same name.
        var loose = mine
        loose.sslmode = "disable"
        loose.safeMode = .full
        var other = mine
        other.id = UUID()
        other.host = "pg2.corp"
        let url = tempFile()
        try ConnectionListTransfer.export(groups: [], connections: [loose, other]).write(to: url)

        model.importConnectionList(from: url)

        XCTAssertEqual(model.connections.count, 2)
        XCTAssertEqual(model.connections.first { $0.id == mine.id }, mine, "untouched")
        let added = try XCTUnwrap(model.connections.first { $0.id != mine.id })
        XCTAssertEqual(added.name, "Prod PG 2")
        XCTAssertEqual(added.host, "pg2.corp")
        XCTAssertTrue(try XCTUnwrap(model.notice?.message).contains("Already here, left as they are: Prod PG"))
    }

    func testAFileThatIsNotAListOrIsNewerIsRefusedByName() {
        XCTAssertThrowsError(try ConnectionListTransfer.read(Data("[]".utf8)))
        XCTAssertThrowsError(try ConnectionListTransfer.read(Data(#"{"format":"other","version":1,"connections":[]}"#.utf8)))
        XCTAssertThrowsError(try ConnectionListTransfer.read(Data(#"{"format":"queryhive-connections","version":2,"connections":[]}"#.utf8))) {
            XCTAssertTrue(($0 as? ConnectionListTransfer.Failure)?.message.contains("newer") == true)
        }
        XCTAssertNoThrow(try ConnectionListTransfer.read(Data(#"{"format":"queryhive-connections","version":1,"connections":[]}"#.utf8)))
    }

    @MainActor
    func testKeyAndCAPathsAreFlaggedBecauseTheyAreFromAnotherMac() throws {
        let (model, _) = model()
        let withKey = Connection(id: UUID(), name: "Tunnelled", color: .blue, kind: .postgres, host: "h", port: 5432,
                                 user: "u", database: "d", schema: "", verify: true, sshHost: "b", sshAuth: .key,
                                 sshKeyPath: "/Users/other/.ssh/id")
        let url = tempFile()
        try ConnectionListTransfer.export(groups: [], connections: [withKey]).write(to: url)
        model.importConnectionList(from: url)
        XCTAssertTrue(try XCTUnwrap(model.notice?.message).contains("check the files exist on this Mac: Tunnelled"))
    }
}
