import AppKit
import SwiftUI

extension AppModel {
    /// Candidates for the word under the caret.
    ///
    /// Sources, in the order they are ranked: the children of the node the typed path names, the
    /// columns the last run reported, then SQL keywords. A bare word with no path is offered
    /// objects from the whole tree, which is right for `FROM ta…`; a word under a path is offered
    /// **only that node's children**, which is the part that was missing.
    ///
    /// It used to ignore the path entirely and sweep every node in the tree for every position. So
    /// `hive.analytics.` offered `penerima_manfaat` (correct), `raw_kpm` from the *bronze* schema,
    /// and tables from other connections — a list that looked plausible and was mostly wrong. The
    /// fix is to resolve the path first and list one node's children.
    func suggestions(for tab: QueryTab, prefix: String, path: [String]) -> [SQLSuggestion] {
        let needle = prefix.lowercased()
        func matches(_ name: String) -> Bool {
            needle.isEmpty || name.lowercased().hasPrefix(needle)
        }

        var found: [SQLSuggestion] = []
        var seen = Set<String>()
        func add(_ text: String, _ kind: SQLSuggestion.Kind) {
            guard !text.isEmpty, seen.insert(text.lowercased()).inserted else { return }
            found.append(SQLSuggestion(text: text, kind: kind))
        }

        // A node whose children are not loaded yet is asked for them, so `FROM hive.` fills in
        // without the user having to expand that catalog in the sidebar first. The list cannot be
        // patched when the answer arrives — the popup is not holding a reference to this array —
        // so the next keystroke re-runs this and finds them. That is the same bargain the tree
        // itself makes, and it beats blocking a keystroke on a network round trip.
        func offerChildren(of node: TreeNode) {
            guard let children = node.children else {
                loadChildren(of: node)
                return
            }
            for child in children where matches(child.title) {
                switch child.kind {
                case .catalog, .database: add(child.title, .catalog)
                case .schema: add(child.title, .schema)
                case .table: add(child.title, .table)
                case .group, .connection: break
                }
            }
        }

        if path.isEmpty {
            for node in allNodes() where matches(node.title) {
                switch node.kind {
                case .catalog, .database: add(node.title, .catalog)
                case .schema: add(node.title, .schema)
                case .table: add(node.title, .table)
                // A group is how the side bar files connections; it is not a name a statement can
                // use, so it is never a suggestion.
                case .group, .connection: break
                }
            }
        } else if let scope = node(atPath: path, connectionID: tab.connectionID) {
            offerChildren(of: scope)
        }
        // A path that resolves to nothing offers nothing. Falling back to the whole tree would
        // reproduce the bug this method exists to fix: a wrong table from another schema is worse
        // than no suggestion at all.

        // A column is only in scope where a bare name could be one — never after a path, where the
        // next word is an object and a column name would be noise.
        if path.isEmpty {
            for column in tab.columns where matches(column.name) {
                add(column.name, .column)
            }
            for keyword in SQLSuggestions.keywords where matches(keyword) {
                add(keyword, .keyword)
            }
        }

        return found
            .sorted { lhs, rhs in
                if lhs.kind != rhs.kind { return lhs.kind.rawValue < rhs.kind.rawValue }
                if lhs.text.count != rhs.text.count { return lhs.text.count < rhs.text.count }
                return lhs.text < rhs.text
            }
            .prefix(8)
            .map { $0 }
    }

    /// The node a typed path names, or nil when the tree cannot answer for it.
    ///
    /// The path is matched **against the shape the driver actually has**, not against fixed
    /// positions: `["hive", "analytics"]` is catalog-then-schema on Trino, database-then-schema on
    /// Postgres (where the database is fixed by the connection and never appears in a name), and
    /// database-only on MySQL. `ConnectionKind.levels` is that contract, and the search follows it
    /// so a path cannot resolve to a node of the wrong kind — `hive.analytics` must not land on a
    /// table called `analytics` just because one exists somewhere.
    func node(atPath path: [String], connectionID: UUID?) -> TreeNode? {
        guard !path.isEmpty else { return nil }
        // The tab's own connection first: two connections can both hold a `hive` catalog, and the
        // query runs against one of them. Falling back to any connection would offer objects the
        // statement cannot reach.
        let roots: [TreeNode]
        if let connectionID, let own = connectionNode(for: connectionID) {
            roots = [own]
        } else {
            roots = tree
        }

        /// One level down, by name, case-insensitively — SQL identifiers are not case sensitive in
        /// the places this is used, and the tree stores them as the server spelled them.
        func child(_ node: TreeNode, named name: String, of kind: TreeNode.Kind) -> TreeNode? {
            node.children?.first {
                $0.kind == kind && $0.title.compare(name, options: .caseInsensitive) == .orderedSame
            }
        }

        /// The levels this driver puts in a name, outermost first, mapped to the node kind each
        /// one is.
        func levels(for kind: ConnectionKind) -> [(TreeNode.Kind, String)] {
            switch kind {
            case .trino: [(.catalog, "catalog"), (.schema, "schema"), (.table, "table")]
            case .postgres: [(.schema, "schema"), (.table, "table")]
            case .mysql: [(.database, "database"), (.table, "table")]
            }
        }

        for root in roots {
            let shape = levels(for: root.connectionKind)
            guard path.count <= shape.count else { continue }
            var current = root
            var matched = true
            for (index, segment) in path.enumerated() {
                let (nodeKind, _) = shape[index]
                guard let next = child(current, named: segment, of: nodeKind) else {
                    matched = false
                    break
                }
                current = next
            }
            if matched { return current }
        }
        return nil
    }
}
