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
        case group, connection, catalog, database, schema, table

        var symbol: String {
            switch self {
            case .group: "folder.badge.gearshape"
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
            case .group: "group"
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
    /// The connection this node belongs to. Optional because a **group** is not part of one: it
    /// holds connections at the top level of the side bar and belongs to none of them. Everything
    /// below a connection inherits its id, so the only nil here is a group.
    let connectionID: UUID?
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

    private init(kind: Kind, id: String, title: String, connectionID: UUID?,
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

    /// A folder holding connections. Its children are assigned by `rebuildTree`, so it never asks
    /// the server for anything — a group is a filing decision, not a place on a server.
    static func group(_ group: ConnectionGroup) -> TreeNode {
        TreeNode(kind: .group, id: "g:\(group.id.uuidString)", title: group.name,
                 connectionID: nil, connectionKind: .trino, color: .gray)
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
        return qualifiedName(database: database, schema: schema, table: title, for: connectionKind)
    }

    /// The group this node *is*, read back out of its id rather than stored a second time — the
    /// same trick `AppModel.connectionID(fromNodeID:)` uses for connections, and for the same
    /// reason: two copies of the same fact can disagree.
    var groupID: UUID? {
        guard kind == .group, id.hasPrefix("g:") else { return nil }
        return UUID(uuidString: String(id.dropFirst(2)))
    }

    var subtitle: String? {
        switch kind {
        case .group: "group"
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

/// A table's fully qualified, quoted name for one driver — the single place that knows Trino and
/// Postgres use double quotes while MySQL uses backticks. The tree's double-click and the editor's
/// object pickers both go through it, so the two cannot produce different SQL for the same table.
func qualifiedName(database: String?, schema: String?, table: String,
                   for kind: ConnectionKind) -> String? {
    switch kind {
    case .trino:
        guard let database, let schema else { return nil }
        return "\"\(database)\".\"\(schema)\".\"\(table)\""
    case .postgres:
        guard let schema else { return nil }
        return "\"\(schema)\".\"\(table)\""
    case .mysql:
        guard let database else { return nil }
        return "`\(database)`.`\(table)`"
    }
}

/// The statement the object screen's inspector runs to learn one table's columns.
///
/// A `SELECT *` under a one-row cap, rather than a metadata query per driver: all three servers
/// describe a result set before they send any row of it, so the engine's `columns` event answers
/// the question, and the cap keeps a wide table from being read to answer a question about its
/// shape. Each driver would otherwise need its own information_schema query, and those three
/// queries are three more places for the answer to disagree with what a `SELECT` actually returns.
///
/// The fallback to the bare name is for a caller whose scope has no name to qualify with. It is
/// deliberately not an error: `qualifiedName` returning nil means this connection cannot qualify
/// the table, and the server will resolve the bare name against the connection's own default
/// rather than refusing to answer at all.
func objectColumnsSQL(database: String?, schema: String?, table: String,
                      for kind: ConnectionKind) -> String {
    let name = qualifiedName(database: database, schema: schema, table: table, for: kind) ?? table
    return "SELECT * FROM \(name)"
}
