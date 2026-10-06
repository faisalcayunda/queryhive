import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// The object tree as an `NSOutlineView` (W9-T3): what it draws, how it stays in step with the
/// model both ways, and what the keyboard, the drag and the accessibility tree get from it.
///
/// Built the way `SchemaOutline` builds it, without SwiftUI: the coordinator's scroll view goes into
/// an off-screen window, and each test hands it a snapshot the way `updateNSView` does.
@MainActor
final class SchemaOutlineTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    private struct Fixture {
        let model: AppModel
        let coordinator: OutlineCoordinator
        let window: NSWindow
        var outline: OutlineView { coordinator.outline }
        let connection: TreeNode
        let catalog: TreeNode
        let schema: TreeNode

        /// What `updateNSView` does after the SwiftUI body re-ran.
        func refresh(filter: Set<String>? = nil) {
            coordinator.apply(.make(model: model, visible: filter))
            window.layoutIfNeeded()
        }
    }

    /// One Trino connection, open, with one catalog, one schema and `tables` tables under it.
    private func fixture(tables: Int = 3, kind: ConnectionKind = .trino,
                         height: CGFloat = 460) -> Fixture {
        let model = AppModel()
        model.connections = [Connection(id: UUID(), name: "Warehouse", color: .violet, kind: kind,
                                        host: "w.internal", port: 8443, sslmode: "prefer",
                                        user: "queryhive", database: "hive", schema: "analytics",
                                        verify: true)]
        model.rebuildTree()
        let root = model.tree[0]
        let catalog = TreeNode.catalog("hive", parent: root)
        let schema = TreeNode.schema("bronze", parent: catalog)
        schema.children = (0..<tables).map { TreeNode.table("table_\($0)", parent: schema) }
        schema.expanded = true
        catalog.children = [schema]
        catalog.expanded = true
        root.children = [catalog]
        root.expanded = true

        let coordinator = OutlineCoordinator(model: model)
        let scroll = coordinator.makeScrollView()
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 260, height: height),
                              styleMask: [.titled], backing: .buffered, defer: false)
        scroll.frame = NSRect(x: 0, y: 0, width: 260, height: height)
        window.contentView = scroll
        let fixture = Fixture(model: model, coordinator: coordinator, window: window,
                              connection: root, catalog: catalog, schema: schema)
        fixture.refresh()
        return fixture
    }

    private func row(_ f: Fixture, _ id: String) -> Int {
        f.coordinator.rows.firstIndex { $0.id == id }.map { f.outline.row(forItem: f.outline.item(atRow: $0)) } ?? -1
    }

    // MARK: Drawing and sync

    func testTheTreeDrawsOneRowPerVisibleNode() throws {
        let f = fixture()
        XCTAssertEqual(f.outline.numberOfRows, 1 + 1 + 1 + 3, "connection, catalog, schema, three tables")
        XCTAssertEqual(f.outline.level(forRow: 0), 0)
        XCTAssertEqual(f.outline.level(forRow: 5), 3)

        let rep = try XCTUnwrap(f.outline.bitmapImageRepForCachingDisplay(in: f.outline.bounds))
        f.outline.cacheDisplay(in: f.outline.bounds, to: rep)
        var colours = Set<Int>()
        for x in stride(from: 0, to: rep.pixelsWide, by: 3) {
            for y in stride(from: 0, to: rep.pixelsHigh, by: 3) {
                if let c = rep.colorAt(x: x, y: y) { colours.insert(c.hash) }
            }
        }
        XCTAssertGreaterThan(colours.count, 3, "the outline drew one flat colour")
        if let dir = ProcessInfo.processInfo.environment["QH_RENDER_DIR"], !dir.isEmpty,
           let data = rep.representation(using: .png, properties: [:]) {
            try FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            try data.write(to: URL(fileURLWithPath: dir).appendingPathComponent("outline.png"))
        }
    }

    func testRowMetricsMatchTheSwiftUIRows() {
        let f = fixture()
        XCTAssertEqual(f.outline.rect(ofRow: 1).minY - f.outline.rect(ofRow: 0).minY, Metrics.treeRow + 1,
                       "23 pt rows one point apart")
        XCTAssertEqual(f.outline.view(atColumn: 0, row: 0, makeIfNecessary: true)?.frame.height, Metrics.treeRow)
        XCTAssertEqual(f.outline.indentationPerLevel, Metrics.treeIndent)
        // The chevron of level 1 sits one indent right of level 0's, and the cell after it.
        let a = f.outline.frameOfOutlineCell(atRow: 0), b = f.outline.frameOfOutlineCell(atRow: 1)
        XCTAssertEqual(b.minX - a.minX, Metrics.treeIndent)
        XCTAssertEqual(a.minX, OutlineView.gutter)
        XCTAssertEqual(f.outline.frameOfCell(atColumn: 0, row: 0).minX, OutlineView.gutter + 16)
    }

    func testExpansionInTheOutlineWritesTheModelAndBack() {
        let f = fixture()
        let schemaItem = f.outline.item(atRow: 2)
        f.outline.collapseItem(schemaItem)
        XCTAssertFalse(f.schema.expanded, "collapsing the row collapses the node")
        f.refresh()
        XCTAssertEqual(f.outline.numberOfRows, 3)

        f.schema.expanded = true
        f.refresh()
        XCTAssertEqual(f.outline.numberOfRows, 6, "the model opened it, so the outline did")
        XCTAssertTrue(f.outline.isItemExpanded(f.outline.item(atRow: 2)))
    }

    func testSelectionIsInSyncBothWaysWithoutALoop() {
        let f = fixture()
        let table = f.schema.children![1]
        f.model.selectedNodeID = table.id
        f.refresh()
        XCTAssertEqual(f.outline.selectedRow, 4)

        f.outline.selectRowIndexes([3], byExtendingSelection: false)
        XCTAssertEqual(f.model.selectedNodeID, f.schema.children![0].id)

        f.model.selectedNodeID = nil
        f.refresh()
        XCTAssertEqual(f.outline.selectedRow, -1)
    }

    func testAFilterOpensEveryLevelAndClearingItPutsThemBack() {
        let f = fixture()
        f.schema.expanded = false
        f.catalog.expanded = false
        f.refresh()
        XCTAssertEqual(f.outline.numberOfRows, 2, "connection and its collapsed catalog")

        let ids = f.connection.matchingIDs("table_2")
        f.refresh(filter: ids)
        XCTAssertEqual(f.outline.numberOfRows, 4, "the match is shown with the path to it")
        XCTAssertFalse(f.schema.expanded, "a filter does not change what the user opened")

        f.refresh()
        XCTAssertEqual(f.outline.numberOfRows, 2)
    }

    func testAnEmptySchemaAndAFailedLoadAreRowsOfTheTree() {
        let f = fixture(tables: 0)
        XCTAssertEqual(f.coordinator.rows.last?.content, .empty)
        XCTAssertEqual(f.outline.numberOfRows, 4)
        f.schema.error = "permission denied"
        f.refresh()
        XCTAssertEqual(f.coordinator.rows.last?.content, .error)
        XCTAssertEqual(f.coordinator.rows.last?.title, "permission denied")
        // A message is not selectable.
        let item = f.outline.item(atRow: 3)!
        XCTAssertFalse(f.coordinator.outlineView(f.outline, shouldSelectItem: item))
    }

    func testOnlyTheRowsOnScreenGetViews() {
        let model = AppModel()
        model.connections = [Connection(id: UUID(), name: "Big", color: .violet, kind: .trino,
                                        host: "b", port: 1, sslmode: "prefer", user: "u",
                                        database: "d", schema: "s", verify: true)]
        model.rebuildTree()
        let root = model.tree[0]
        root.children = (0..<40).map { c in
            let catalog = TreeNode.catalog("dw-\(c)", parent: root)
            catalog.expanded = true
            catalog.children = (0..<8).map { s in
                let schema = TreeNode.schema("a_\(s)", parent: catalog)
                schema.expanded = true
                schema.children = (0..<20).map { TreeNode.table("t_\(c)_\(s)_\($0)", parent: schema) }
                return schema
            }
            return catalog
        }
        root.expanded = true
        let coordinator = OutlineCoordinator(model: model)
        let scroll = coordinator.makeScrollView()
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 260, height: 460),
                              styleMask: [.titled], backing: .buffered, defer: false)
        scroll.frame = NSRect(x: 0, y: 0, width: 260, height: 460)
        window.contentView = scroll
        coordinator.apply(.make(model: model, visible: nil))
        window.layoutIfNeeded()
        XCTAssertGreaterThan(coordinator.outline.numberOfRows, 6_000)
        let built = coordinator.outline.subviews.filter { $0 is NSTableRowView }.count
        XCTAssertLessThan(built, 60, "row views are made for the visible rows only")
    }

    // MARK: Keyboard

    func testTypeSelectUsesTheTitle() {
        let f = fixture()
        let table = f.outline.item(atRow: 4)!
        XCTAssertEqual(f.coordinator.outlineView(f.outline, typeSelectStringFor: nil, item: table), "table_1")
        XCTAssertTrue(f.outline.allowsTypeSelect)
        f.schema.error = "boom"
        f.refresh()
        let message = f.outline.item(atRow: 3)!
        XCTAssertNil(f.coordinator.outlineView(f.outline, typeSelectStringFor: nil, item: message))
    }

    func testReturnDoesWhatDoubleClickDoes() throws {
        let f = fixture()
        // A catalog's open action is to toggle it. Its children are loaded, so nothing is fetched.
        f.outline.selectRowIndexes([1], byExtendingSelection: false)
        let event = try XCTUnwrap(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0, windowNumber: 0,
            context: nil, characters: "\r", charactersIgnoringModifiers: "\r", isARepeat: false,
            keyCode: 36))
        f.outline.keyDown(with: event)
        XCTAssertFalse(f.catalog.expanded, "Return on an open catalog closed it, like a double-click")
        f.refresh()
        f.outline.keyDown(with: event)
        XCTAssertTrue(f.catalog.expanded)
    }

    // MARK: Disclosure, menu, drag

    func testTheDisclosureButtonIsTheChevronAndItToggles() throws {
        let f = fixture()
        XCTAssertTrue(f.outline.makeView(withIdentifier: NSOutlineView.disclosureButtonIdentifier,
                                         owner: f.outline) is NSButton)
        let rowView = try XCTUnwrap(f.outline.rowView(atRow: 2, makeIfNecessary: true))
        let button = try XCTUnwrap(rowView.subviews.compactMap { $0 as? NSButton }.first,
                                   "the row carries its disclosure button")
        XCTAssertEqual(button.frame.size, NSSize(width: 11, height: 11))
        XCTAssertEqual(button.state, .on)
        button.performClick(nil)
        XCTAssertFalse(f.schema.expanded, "clicking the chevron closed the schema")
    }

    func testTheMenuIsBuiltFromTheDescriptionAndDoesNotMoveTheSelection() {
        let f = fixture()
        f.model.selectedNodeID = f.catalog.id
        f.refresh()
        let menu = f.outline.menuProvider?(4)
        XCTAssertEqual(menu?.items.first?.title, "Insert into Query")
        XCTAssertEqual(f.model.selectedNodeID, f.catalog.id)
        XCTAssertNil(f.outline.menuProvider?(-1))
    }

    func testOnlyATableCanBeDraggedAndItCarriesItsQualifiedName() throws {
        let f = fixture()
        let table = f.outline.item(atRow: 4)!
        let writer = try XCTUnwrap(f.coordinator.outlineView(f.outline, pasteboardWriterForItem: table)
                                   as? NSPasteboardItem)
        XCTAssertEqual(writer.string(forType: .string), "\"hive\".\"bronze\".\"table_1\"")
        XCTAssertEqual(writer.string(forType: .treeNodeID), f.schema.children![1].id)
        XCTAssertNil(f.coordinator.outlineView(f.outline, pasteboardWriterForItem: f.outline.item(atRow: 2)!))

        let mysql = fixture(kind: .mysql)
        let dragged = mysql.coordinator.outlineView(mysql.outline, pasteboardWriterForItem: mysql.outline.item(atRow: 4)!)
        XCTAssertEqual((dragged as? NSPasteboardItem)?.string(forType: .string), "`hive`.`table_1`",
                       "the drag quotes the way the connection's driver does")
    }

    // MARK: Accessibility

    func testTheOutlineAndItsRowsSpeakToVoiceOver() throws {
        let f = fixture()
        XCTAssertEqual(f.outline.accessibilityRole(), .outline)
        XCTAssertEqual(f.outline.accessibilityLabel(), "Object tree")

        let connection = try XCTUnwrap(f.outline.view(atColumn: 0, row: 0, makeIfNecessary: true) as? OutlineCellView)
        XCTAssertEqual(connection.accessibilityLabel(), "Warehouse")
        XCTAssertEqual(connection.accessibilityValue() as? String, "Connected")
        XCTAssertEqual(connection.accessibilityCustomActions()?.map(\.name), ["Open", "Refresh"])
        XCTAssertTrue(connection.accessibilityHelp()?.contains("double-click to expand") == true)

        let table = try XCTUnwrap(f.outline.view(atColumn: 0, row: 4, makeIfNecessary: true) as? OutlineCellView)
        XCTAssertEqual(table.accessibilityLabel(), "table_1")
        XCTAssertEqual(table.accessibilityValue() as? String, "table")

        // Role, level and disclosure of the rows come from AppKit once an accessibility client
        // attaches, which a unit test cannot do; the facts they are built from are checked here and
        // the VoiceOver walk is the owner's smoke test.
        XCTAssertEqual(f.outline.level(forRow: 2), 2)
        XCTAssertTrue(f.outline.isItemExpanded(f.outline.item(atRow: 2)))
        XCTAssertFalse(f.outline.isExpandable(f.outline.item(atRow: 4)))
    }
}
