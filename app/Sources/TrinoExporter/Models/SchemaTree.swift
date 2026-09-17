import Foundation
import SwiftUI

/// One node of the Navicat-style object tree: connection → catalog → schema → table. Children
/// are loaded lazily, the first time a node is expanded, by running one engine browse command;
/// `children == nil` means "never loaded", `children == []` means "loaded and empty".
@Observable
final class TreeNode: Identifiable {
    enum Kind {
        case connection, catalog, schema, table

        var symbol: String {
            switch self {
            case .connection: "server.rack"
            case .catalog: "cylinder.split.1x2"
            case .schema: "folder"
            case .table: "tablecells"
            }
        }

        /// Tables are leaves: no disclosure triangle, nothing to load.
        var isExpandable: Bool { self != .table }
    }

    let id: String
    let kind: Kind
    /// `var`, not `let`: renaming or recolouring a saved connection has to show up on the
    /// node that already exists, and `rebuildTree` reuses nodes by id to keep them expanded.
    var title: String
    let connectionID: UUID
    var color: ConnectionColor
    /// The catalog a `.schema`/`.table` node lives in, and the schema a `.table` node lives in.
    /// `nil` for the levels above.
    let catalog: String?
    let schema: String?

    var children: [TreeNode]?
    var expanded = false
    var loading = false
    var error: String?

    var isExpandable: Bool { kind.isExpandable }

    private init(kind: Kind, id: String, title: String, connectionID: UUID, color: ConnectionColor,
                 catalog: String? = nil, schema: String? = nil) {
        self.kind = kind
        self.id = id
        self.title = title
        self.connectionID = connectionID
        self.color = color
        self.catalog = catalog
        self.schema = schema
    }

    static func connection(_ connection: Connection) -> TreeNode {
        TreeNode(kind: .connection, id: "c:\(connection.id.uuidString)", title: connection.name,
                 connectionID: connection.id, color: connection.color)
    }

    static func catalog(_ name: String, parent: TreeNode) -> TreeNode {
        TreeNode(kind: .catalog, id: "\(parent.id)/cat:\(name)", title: name,
                 connectionID: parent.connectionID, color: parent.color, catalog: name)
    }

    static func schema(_ name: String, parent: TreeNode) -> TreeNode {
        TreeNode(kind: .schema, id: "\(parent.id)/sch:\(name)", title: name,
                 connectionID: parent.connectionID, color: parent.color,
                 catalog: parent.catalog, schema: name)
    }

    static func table(_ name: String, parent: TreeNode) -> TreeNode {
        TreeNode(kind: .table, id: "\(parent.id)/tab:\(name)", title: name,
                 connectionID: parent.connectionID, color: parent.color,
                 catalog: parent.catalog, schema: parent.schema)
    }

    /// What a double-click on this node puts into the active query. A table is quoted the way
    /// Trino wants it, so the tree is useful without typing by hand.
    var insertableText: String? {
        guard kind == .table, let catalog, let schema else { return nil }
        return "\"\(catalog)\".\"\(schema)\".\"\(title)\""
    }

    var subtitle: String? {
        switch kind {
        case .connection: "coordinator"
        case .catalog: "catalog"
        case .schema: "schema"
        case .table: nil
        }
    }

    /// Ids to keep on screen for a filter. The filter can only see what has already been
    /// loaded — browsing a coordinator is a network round trip per level, so there is nothing
    /// to search below an unexpanded node.
    func matchingIDs(_ needle: String) -> Set<String> {
        var visible = Set<String>()
        @discardableResult
        func walk(_ node: TreeNode) -> Bool {
            var keep = node.title.localizedCaseInsensitiveContains(needle)
            for child in node.children ?? [] where walk(child) { keep = true }
            if keep { visible.insert(node.id) }
            return keep
        }
        walk(self)
        return visible
    }
}

/// A connection the editor sheet is open for. `connectionID == nil` means a new, unsaved
/// connection; `id` is the sheet's own identity, so two "new connection" presentations in a row
/// are still two distinct sheets.
struct ConnectionEditorTarget: Identifiable {
    let id = UUID()
    let connectionID: UUID?

    init(_ connectionID: UUID?) { self.connectionID = connectionID }
}
