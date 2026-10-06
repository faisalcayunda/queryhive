import XCTest

@testable import QueryHive

/// The tree's menus as data: the titles, the order and the ticks the SwiftUI menus had.
@MainActor
final class TreeMenuTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    private func model(_ kind: ConnectionKind) -> (AppModel, TreeNode) {
        let model = AppModel()
        model.connections = [Connection(id: UUID(), name: "W", color: .violet, kind: kind, host: "h",
                                        port: 1, sslmode: "prefer", user: "u", database: "d",
                                        schema: "s", verify: true)]
        model.rebuildTree()
        return (model, model.tree[0])
    }

    private func titles(_ items: [TreeMenuItem]) -> [String] {
        items.map { $0.isSeparator ? "-" : $0.title }
    }

    func testAPostgresConnectionHasTheTwoSwitchesAndOthersDoNot() {
        let (pg, pgNode) = model(.postgres)
        let items = TreeMenu.items(for: pgNode, in: pg)
        XCTAssertEqual(titles(items), [
            "Open Connection", "Show System Schemas", "Show All Databases", "-",
            "Edit Connection…", "Duplicate Connection", "Delete Connection…", "-", "Group", "-",
            "New Connection", "-", "New Query", "Open SQL File…", "-", "Color", "-", "Refresh",
            "Reveal connections.json"])
        XCTAssertFalse(items[1].checked)

        let (trino, trinoNode) = model(.trino)
        let other = titles(TreeMenu.items(for: trinoNode, in: trino))
        XCTAssertFalse(other.contains("Show System Schemas"))
        XCTAssertFalse(other.contains("Show All Databases"))
    }

    func testTheSwitchShowsItsStateAndTheColourInUseIsTicked() {
        let (model, node) = model(.postgres)
        model.connections[0].showAllSchemas = true
        let items = TreeMenu.items(for: node, in: model)
        XCTAssertTrue(items.first { $0.title == "Show System Schemas" }!.checked)
        let colours = items.first { $0.title == "Color" }!.submenu!
        XCTAssertEqual(colours.filter(\.checked).map(\.title), ["Violet"])
        let group = items.first { $0.title == "Group" }!.submenu!
        XCTAssertEqual(titles(group), ["No Group", "-", "New Group…"])
        XCTAssertTrue(group[0].checked)
    }

    func testATableOffersTheDestructivePairAfterImport() {
        let (model, root) = model(.trino)
        let table = TreeNode.table("t", parent: TreeNode.schema("s", parent: TreeNode.catalog("c", parent: root)))
        XCTAssertEqual(titles(TreeMenu.items(for: table, in: model)), [
            "Insert into Query", "Copy Qualified Name", "Copy Name", "-", "Import Data into Table…", "-",
            "Truncate Table…", "Drop Table…", "-", "Refresh"])
    }

    func testCatalogsSchemasAndGroupsKeepTheirMenus() {
        let (model, root) = model(.trino)
        let catalog = TreeNode.catalog("c", parent: root)
        XCTAssertEqual(titles(TreeMenu.items(for: catalog, in: model)),
                       ["Refresh", "Copy Name", "-", "New Query", "Open SQL File…"])
        let group = TreeNode.group(ConnectionGroup(name: "Prod"))
        XCTAssertEqual(titles(TreeMenu.items(for: group, in: model)), [
            "New Connection in Group", "-", "Rename Group…", "Delete Group", "-", "Refresh"])
    }

    func testTheMenuBecomesAnNSMenuWithTicksAndSubmenus() {
        let (model, node) = model(.postgres)
        let menu = OutlineCoordinator.menu(from: TreeMenu.items(for: node, in: model))
        XCTAssertEqual(menu.items.first?.title, "Open Connection")
        XCTAssertTrue(menu.items.contains { $0.isSeparatorItem })
        XCTAssertNotNil(menu.items.first { $0.title == "Color" }?.submenu)
    }
}
