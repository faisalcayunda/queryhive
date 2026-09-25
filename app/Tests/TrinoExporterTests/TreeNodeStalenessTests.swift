import XCTest

@testable import QueryHive

/// The tree's copy of a connection's own fields, and whether it stays in step when they change.
///
/// `rebuildTree` reuses the node that already exists, which is what keeps an expanded server
/// expanded. The cost of reuse is that everything copied onto the node has to be refreshed when it
/// is rebuilt, and anything missed goes stale **silently** — the row simply keeps saying what it
/// said before. `connectionKind` was missed: changing a connection from Trino to Postgres left the
/// node claiming Trino, which is wrong quoting for every table under it and the wrong mark on its
/// row.
@MainActor
final class TreeNodeStalenessTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    private func connection(_ name: String, kind: ConnectionKind) -> Connection {
        Connection(id: UUID(), name: name, color: .violet, kind: kind,
                   host: "\(name).internal", port: 8443, sslmode: "prefer",
                   user: "queryhive", database: "hive", schema: "analytics", verify: true)
    }

    func testChangingTheDriverReachesTheNode() {
        let model = AppModel()
        var saved = connection("Warehouse", kind: .trino)
        model.connections = [saved]
        model.rebuildTree()
        XCTAssertEqual(model.tree[0].connectionKind, .trino)

        // What the connection editor does: replace the saved connection, then rebuild.
        saved.kind = .postgres
        model.connections = [saved]
        model.rebuildTree()

        XCTAssertEqual(model.tree[0].connectionKind, .postgres,
                       "the node still claims the old driver")
    }

    func testTheDriverDecidesHowATableIsQuoted() {
        // The consequence of the staleness, stated as behaviour: a double-click on a MySQL table has
        // to produce backticks, and a node that still thought it was Trino would produce double
        // quotes — a statement the server rejects.
        let model = AppModel()
        let saved = connection("Reporting", kind: .mysql)
        model.connections = [saved]
        model.rebuildTree()

        let root = model.tree[0]
        let database = TreeNode.database("reporting", parent: root)
        let table = TreeNode.table("kpm", parent: database)
        XCTAssertEqual(table.insertableText, "`reporting`.`kpm`")

        var switched = saved
        switched.kind = .trino
        model.connections = [switched]
        model.rebuildTree()

        // Built through the levels Trino actually has — catalog, then schema, then table. A
        // two-part name is a complete table name for MySQL and an incomplete one for Trino, which
        // is exactly the difference `insertableText` exists to encode.
        let rebuilt = TreeNode.table("kpm", parent: TreeNode.schema(
            "public", parent: TreeNode.catalog("reporting", parent: model.tree[0])))
        XCTAssertEqual(rebuilt.insertableText, "\"reporting\".\"public\".\"kpm\"")
    }

    func testColourAndNameStayInStep() {
        // The two fields that were already refreshed, pinned so a future edit to `rebuildTree`
        // cannot drop them while adding something else.
        let model = AppModel()
        var saved = connection("Warehouse", kind: .trino)
        model.connections = [saved]
        model.rebuildTree()

        saved.name = "Live"
        saved.color = .amber
        model.connections = [saved]
        model.rebuildTree()

        XCTAssertEqual(model.tree[0].title, "Live")
        XCTAssertEqual(model.tree[0].color, .amber)
    }
}
