import Foundation
import SwiftUI

/// One node of the Navicat-style object tree. Its depth is not fixed: Trino has
/// catalog → schema → table, Postgres schema → table (its database is fixed by the connection),
/// and MySQL database → table. `ConnectionKind.levels` is the contract, and the engine's
/// `db_drivers` command reports the same shape.
///
/// Children are loaded lazily, the first time a node is expanded, by running one engine browse
/// command; `children == nil` means "never loaded", `children == []` means "loaded and empty".
@Observable
final class TreeNode: Identifiable {
    enum Kind {
        case connection, catalog, database, schema, table

        var symbol: String {
            switch self {
            case .connection: "server.rack"
            case .catalog: "bolt.horizontal"
            case .database: "cylinder"
            case .schema: "folder"
            case .table: "tablecells"
            }
        }

        /// What this level is called in the tree's own vocabulary. Used to name a level in an
        /// error or a tooltip without hard-coding "catalog" everywhere.
        var noun: String {
            switch self {
            case .connection: "connection"
            case .catalog: "catalog"
            case .database: "database"
            case .schema: "schema"
            case .table: "table"
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
    /// The driver this node belongs to. Required for quoting: a double-click on a MySQL table
    /// must produce backticks, not the double quotes Trino and Postgres use.
    let connectionKind: ConnectionKind
    var color: ConnectionColor
    /// Trino's catalog, MySQL's database. Postgres has no such level, so it stays nil there.
    let database: String?
    /// Trino's and Postgres's schema. MySQL has none.
    let schema: String?

    var children: [TreeNode]?
    var expanded = false
    var loading = false
    var error: String?

    var isExpandable: Bool { kind.isExpandable }

    private init(kind: Kind, id: String, title: String, connectionID: UUID,
                 connectionKind: ConnectionKind, color: ConnectionColor,
                 database: String? = nil, schema: String? = nil) {
        self.kind = kind
        self.id = id
        self.title = title
        self.connectionID = connectionID
        self.connectionKind = connectionKind
        self.color = color
        self.database = database
        self.schema = schema
    }

    static func connection(_ connection: Connection) -> TreeNode {
        TreeNode(kind: .connection, id: "c:\(connection.id.uuidString)", title: connection.name,
                 connectionID: connection.id, connectionKind: connection.kind, color: connection.color)
    }

    /// A Trino catalog, or a MySQL database. Both are the level directly under the connection
    /// that names *where* the schemas or tables live.
    static func catalog(_ name: String, parent: TreeNode) -> TreeNode {
        TreeNode(kind: .catalog, id: "\(parent.id)/cat:\(name)", title: name,
                 connectionID: parent.connectionID, connectionKind: parent.connectionKind,
                 color: parent.color, database: name)
    }

    static func database(_ name: String, parent: TreeNode) -> TreeNode {
        TreeNode(kind: .database, id: "\(parent.id)/db:\(name)", title: name,
                 connectionID: parent.connectionID, connectionKind: parent.connectionKind,
                 color: parent.color, database: name)
    }

    static func schema(_ name: String, parent: TreeNode) -> TreeNode {
        TreeNode(kind: .schema, id: "\(parent.id)/sch:\(name)", title: name,
                 connectionID: parent.connectionID, connectionKind: parent.connectionKind,
                 color: parent.color, database: parent.database, schema: name)
    }

    static func table(_ name: String, parent: TreeNode) -> TreeNode {
        TreeNode(kind: .table, id: "\(parent.id)/tab:\(name)", title: name,
                 connectionID: parent.connectionID, connectionKind: parent.connectionKind,
                 color: parent.color, database: parent.database, schema: parent.schema)
    }

    /// What a double-click on this node puts into the active query: the table's name, quoted the
    /// way this connection's driver wants it, with exactly the parts that driver has.
    var insertableText: String? {
        guard kind == .table else { return nil }
        switch connectionKind {
        case .trino:
            guard let database, let schema else { return nil }
            return "\"\(database)\".\"\(schema)\".\"\(title)\""
        case .postgres:
            guard let schema else { return nil }
            return "\"\(schema)\".\"\(title)\""
        case .mysql:
            guard let database else { return nil }
            return "`\(database)`.`\(title)`"
        }
    }

    var subtitle: String? {
        switch kind {
        case .connection: connectionKind.label
        case .catalog: "catalog"
        case .database: "database"
        case .schema: "schema"
        case .table: nil
        }
    }

    /// Ids to keep on screen for a filter. The filter can only see what has already been
    /// loaded — browsing is a network round trip per level, so there is nothing to search below
    /// an unexpanded node.
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
    /// Open straight at the URI step, for the sidebar's "Add from URL…" — the sheet owns URL
    /// parsing now, so there is no second implementation of it to drift.
    var startAtURL = false
    /// Snapshot scaffolding. The footer's test state is otherwise unreachable without a server to
    /// test against, and a footer nobody has looked at is a footer that ships broken — which is
    /// exactly how its layout came to be wrong.
    var previewTestCount: Int?

    init(_ connectionID: UUID?, startAtURL: Bool = false, previewTestCount: Int? = nil) {
        self.connectionID = connectionID
        self.startAtURL = startAtURL
        self.previewTestCount = previewTestCount
    }
}
