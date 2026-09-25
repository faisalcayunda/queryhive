import XCTest

@testable import QueryHive

/// Connections filed into groups: the file format, the tree it produces, and what the actions do.
///
/// The file format is the part worth testing hardest. `connections.json` holds a user's servers and
/// every one of them would be lost by a decode that refused the old shape — and the failure would
/// be silent, because `ConnectionStore.load` treats an unreadable file as "no connections" and
/// moves it aside. So the older format is tested explicitly, not assumed to still work.
final class ConnectionGroupTests: XCTestCase {
    override func setUp() {
        super.setUp()
        // Before any AppModel is built: that constructor reads connections.json, and the actions
        // these tests drive write it.
        isolateConnectionStore()
    }


    private func connection(_ name: String, group: UUID? = nil) -> Connection {
        Connection(id: UUID(), name: name, color: .violet, kind: .trino,
                   host: "\(name).internal", port: 8443, sslmode: "prefer",
                   user: "queryhive", database: "hive", schema: "analytics", verify: true,
                   group: group)
    }

    private func encode(_ document: ConnectionsDocument) throws -> Data {
        try JSONEncoder().encode(document)
    }

    // MARK: The file

    func testADocumentRoundTripsThroughTheFile() throws {
        let group = ConnectionGroup(name: "Production")
        let document = ConnectionsDocument(groups: [group],
                                           connections: [connection("a", group: group.id),
                                                         connection("b")])
        let decoded = try JSONDecoder().decode(ConnectionsDocument.self, from: try encode(document))
        XCTAssertEqual(decoded, document)
        XCTAssertEqual(decoded.connections[0].group, group.id)
        XCTAssertNil(decoded.connections[1].group)
    }

    func testAnEmptyGroupSurvivesTheRoundTrip() throws {
        // The reason groups are stored rather than inferred from the connections pointing at them:
        // "New Group" with nothing in it has to still be there next launch.
        let document = ConnectionsDocument(groups: [ConnectionGroup(name: "Empty")], connections: [])
        let decoded = try JSONDecoder().decode(ConnectionsDocument.self, from: try encode(document))
        XCTAssertEqual(decoded.groups.map(\.name), ["Empty"])
        XCTAssertTrue(decoded.connections.isEmpty)
    }

    func testABareArrayFromBeforeGroupsStillLoads() throws {
        // What the file was: `[Connection]`, no envelope. A decode that only knew the envelope would
        // send this to the "corrupt" path, which moves the file aside and starts empty — a user's
        // servers gone with no error shown for it.
        let legacy = try JSONEncoder().encode([connection("old")])
        let decoded = try JSONDecoder().decode(ConnectionsDocument.self, from: legacy)
        XCTAssertEqual(decoded.connections.map(\.name), ["old"])
        XCTAssertTrue(decoded.groups.isEmpty, "a file with no groups read back as having groups")
        XCTAssertNil(decoded.connections[0].group)
    }

    func testAConnectionWithoutTheGroupKeyStillLoads() throws {
        // A connection written before `group` existed has no key for it at all.
        let legacy = """
        [{"id":"\(UUID().uuidString)","name":"old","color":"violet","kind":"trino",
          "host":"h","port":8443,"scheme":"https","sslmode":"prefer","user":"u",
          "database":"d","schema":"s","verify":true}]
        """
        let decoded = try JSONDecoder().decode(ConnectionsDocument.self,
                                               from: Data(legacy.utf8))
        XCTAssertEqual(decoded.connections.count, 1)
        XCTAssertNil(decoded.connections[0].group)
        XCTAssertFalse(decoded.connections[0].showAllSchemas)
    }

    func testAnEnvelopeWithNoGroupsKeyStillLoads() throws {
        // A file written by a build that knew groups but had none.
        let envelope = """
        {"connections":[{"id":"\(UUID().uuidString)","name":"a","color":"violet","kind":"trino",
          "host":"h","port":8443,"scheme":"https","sslmode":"prefer","user":"u",
          "database":"d","schema":"s","verify":true}]}
        """
        let decoded = try JSONDecoder().decode(ConnectionsDocument.self, from: Data(envelope.utf8))
        XCTAssertEqual(decoded.connections.map(\.name), ["a"])
        XCTAssertTrue(decoded.groups.isEmpty)
    }

    func testRubbishStillThrowsSoItIsMovedAside() throws {
        // The corrupt path has to keep working: silently reading a damaged file as "no connections"
        // and then saving over it would destroy the only copy.
        XCTAssertThrowsError(try JSONDecoder().decode(ConnectionsDocument.self, from: Data("{{".utf8)))
    }

    // MARK: The backup

    func testSavingKeepsThePreviousFileBesideIt() throws {
        // The reason this exists: the real connections.json was overwritten once and there was
        // nothing to restore it from. Now every save leaves the file it replaced.
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("qh-backup-\(UUID().uuidString)")
        let previous = ConnectionStore.root
        ConnectionStore.root = root
        defer {
            ConnectionStore.root = previous
            try? FileManager.default.removeItem(at: root)
        }

        try ConnectionStore.save(ConnectionsDocument(connections: [connection("first")]))
        try ConnectionStore.save(ConnectionsDocument(connections: [connection("second")]))

        let url = root.appendingPathComponent("connections.json")
        let backup = root.appendingPathComponent("connections.json.bak")
        XCTAssertTrue(FileManager.default.fileExists(atPath: backup.path),
                      "no backup was written, so the previous state is unrecoverable")

        let current = try JSONDecoder().decode(ConnectionsDocument.self, from: Data(contentsOf: url))
        let kept = try JSONDecoder().decode(ConnectionsDocument.self, from: Data(contentsOf: backup))
        XCTAssertEqual(current.connections.map(\.name), ["second"])
        XCTAssertEqual(kept.connections.map(\.name), ["first"], "the backup is not one save behind")
    }

    // MARK: The tree

    private func model(connections: [Connection], groups: [ConnectionGroup]) -> AppModel {
        let model = AppModel()
        model.connections = connections
        model.groups = groups
        model.rebuildTree()
        return model
    }

    func testGroupsComeFirstAndHoldTheirConnections() {
        let group = ConnectionGroup(name: "Production")
        let a = connection("a", group: group.id)
        let b = connection("b")
        let model = model(connections: [a, b], groups: [group])

        XCTAssertEqual(model.tree.count, 2, "expected one group row and one loose connection")
        let folder = model.tree[0]
        XCTAssertEqual(folder.kind, .group)
        XCTAssertEqual(folder.title, "Production")
        XCTAssertEqual(folder.children?.map(\.title), ["a"])
        XCTAssertEqual(model.tree[1].kind, .connection)
        XCTAssertEqual(model.tree[1].title, "b")
    }

    func testAGroupWithNothingInItIsStillARow() {
        let model = model(connections: [], groups: [ConnectionGroup(name: "Empty")])
        XCTAssertEqual(model.tree.count, 1)
        XCTAssertEqual(model.tree[0].kind, .group)
        XCTAssertEqual(model.tree[0].children?.count, 0)
    }

    func testOnlyUngroupedConnectionsSitAtTheTopLevel() {
        let group = ConnectionGroup(name: "Production")
        let model = model(connections: [connection("a", group: group.id), connection("b")],
                          groups: [group])
        let loose = model.tree.filter { $0.kind == .connection }
        XCTAssertEqual(loose.map(\.title), ["b"])
    }

    func testAConnectionPointingAtAMissingGroupIsShownNotLost() {
        // A hand-edited file, or one written by a build that knew a group this one does not.
        let model = model(connections: [connection("a", group: UUID())], groups: [])
        XCTAssertEqual(model.tree.count, 1, "the connection vanished from the tree")
        XCTAssertEqual(model.tree[0].kind, .connection)
        XCTAssertEqual(model.tree[0].title, "a")
    }

    func testAConnectionIsFoundWhereverItIsFiled() {
        // The status bar's dot and the breadcrumb read this, and both used to search the root only.
        let group = ConnectionGroup(name: "Production")
        let a = connection("a", group: group.id)
        let model = model(connections: [a], groups: [group])

        XCTAssertNotNil(model.connectionNode(for: a.id),
                        "a connection inside a group was invisible to the status bar")
        XCTAssertNil(model.connectionNode(for: UUID()))
    }

    func testMovingAConnectionKeepsItsExpandedNode() {
        // Nodes are reused by id, so filing one connection must not collapse the servers the user
        // already opened. This matters more now that a move rebuilds the whole tree.
        let group = ConnectionGroup(name: "Production")
        let a = connection("a")
        let model = model(connections: [a], groups: [group])
        let node = try? XCTUnwrap(model.connectionNode(for: a.id))
        node?.expanded = true

        model.move(a.id, toGroup: group.id)
        XCTAssertTrue(model.connectionNode(for: a.id)?.expanded ?? false,
                      "moving a connection collapsed it")
    }

    // MARK: The actions

    @MainActor
    func testDeletingAGroupKeepsItsConnections() {
        let group = ConnectionGroup(name: "Production")
        let a = connection("a", group: group.id)
        let model = model(connections: [a], groups: [group])

        model.deleteGroup(group.id)
        XCTAssertTrue(model.groups.isEmpty)
        XCTAssertEqual(model.connections.count, 1, "the connection was deleted with its group")
        XCTAssertNil(model.connections[0].group)
        XCTAssertEqual(model.tree.count, 1)
        XCTAssertEqual(model.tree[0].kind, .connection)
    }

    @MainActor
    func testRenamingAGroupKeepsItsConnections() {
        let group = ConnectionGroup(name: "Production")
        let a = connection("a", group: group.id)
        let model = model(connections: [a], groups: [group])

        model.presentRenameGroup(group.id)
        model.groupNaming?.name = "Live"
        model.commitGroupNaming()

        XCTAssertEqual(model.groups.map(\.name), ["Live"])
        XCTAssertEqual(model.connections[0].group, group.id,
                       "renaming orphaned the connections: they are filed by id, not by name")
        XCTAssertEqual(model.tree[0].title, "Live")
    }

    @MainActor
    func testCreatingAGroupFromAConnectionFilesThatConnection() {
        let a = connection("a")
        let model = model(connections: [a], groups: [])

        model.presentNewGroup(with: a.id)
        model.groupNaming?.name = "Production"
        model.commitGroupNaming()

        XCTAssertEqual(model.groups.count, 1)
        XCTAssertEqual(model.connections[0].group, model.groups[0].id,
                       "the connection was not filed into the group it created")
    }

    @MainActor
    func testABlankNameIsRefused() {
        // A group with no name is a row the user cannot identify. Creating one is not the lesser
        // evil; refusing is.
        let model = model(connections: [], groups: [])
        model.presentNewGroup()
        model.groupNaming?.name = "   "
        model.commitGroupNaming()
        XCTAssertTrue(model.groups.isEmpty, "a group was created with a blank name")
    }

    @MainActor
    func testMovingOutOfAGroupPutsAConnectionBackAtTheTopLevel() {
        let group = ConnectionGroup(name: "Production")
        let a = connection("a", group: group.id)
        let model = model(connections: [a], groups: [group])

        model.move(a.id, toGroup: nil)
        XCTAssertNil(model.connections[0].group)
        XCTAssertEqual(model.tree.count, 2)
        XCTAssertEqual(model.tree.filter { $0.kind == .connection }.map(\.title), ["a"])
    }

    @MainActor
    func testTheGroupsInTheTreeFollowTheOrderTheyWereMade() {
        // The menu and the tree both read this order, and a list that reordered itself when a
        // connection moved would make the menu hard to use.
        let first = ConnectionGroup(name: "First")
        let second = ConnectionGroup(name: "Second")
        let a = connection("a")
        let model = model(connections: [a], groups: [first, second])

        model.move(a.id, toGroup: second.id)
        XCTAssertEqual(model.tree.map(\.title), ["First", "Second"])
        XCTAssertEqual(model.groups.map(\.name), ["First", "Second"])
    }
}
