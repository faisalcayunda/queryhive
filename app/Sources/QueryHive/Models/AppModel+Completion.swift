import AppKit
import QueryHiveFFI
import SwiftUI

extension AppModel {
    /// Candidates for the word under the caret.
    ///
    /// Sources, in the order they are ranked for the position: the children of the node the typed
    /// path names, the objects of this tab's connection, the columns the last run reported, then
    /// SQL keywords. A bare word with no path is offered the objects of **the tab's own
    /// connection**, the ones in its default database and schema first and written bare, the rest
    /// written with the qualifier the server needs to find them; a word under a path is offered
    /// **only that node's children**.
    ///
    /// It used to sweep every node in the tree for every position. So `hive.analytics.` offered
    /// `penerima_manfaat` (correct), `raw_kpm` from the *bronze* schema, and tables from other
    /// connections — a list that looked plausible and was mostly wrong. The fix is to resolve the
    /// path first and list one node's children, and to keep a bare word inside its connection.
    ///
    /// A name matches by its start, by the start of a word inside it (`kpm` finds
    /// `rekap_kpm_harian`) and, from three characters, by any stretch of it; a prefix always
    /// ranks above the others (`SQLSuggestions.match`).
    ///
    /// `scope` is what the text before the word says about the caret (`sqlCaretScope`, asked of
    /// the text up to where the word starts), and `references` is the statement's last analysis
    /// (`EditorDocument.references`, which runs in the background). Both are optional and the
    /// list without them is the one above. With them:
    ///
    /// - The kinds are ranked for the clause: tables before everything at a `FROM` or `JOIN`,
    ///   columns and keywords before tables in a `WHERE` or a select list.
    /// - A table offered in a `FROM` or `JOIN` of a query carries a free alias
    ///   (`SQLSuggestions.tableText`); one named by `UPDATE`, `DELETE FROM` or `INSERT INTO`
    ///   never does. Without `references` the statement's aliases are unknown, so none is made.
    /// - The select list's `AS` names are offered where the driver accepts them
    ///   (`SQLSuggestions.offersSelectAliases`), and where it does not, an output column of the
    ///   last run that is only an alias is not offered either.
    ///
    /// A level of the tree that is not loaded is asked for here and fills in later; the caller
    /// learns of it through `completion.onCatalogLoaded` and asks again. A level that failed to
    /// load is not asked for again until it is invalidated, or this would be a request per key.
    ///
    /// The editor is flushed (`AppModel.flushEditors`) by whoever reads the tab's SQL to make
    /// the `head` this is given; nothing here reads `tab.sql`.
    func suggestions(for tab: QueryTab, prefix: String, path: [String],
                     scope: EditorCaretScope? = nil, references: EditorReferences? = nil,
                     aliasable: Bool = true) -> [SQLSuggestion] {
        let connection = connection(for: tab)
        let driver = connection?.kind
        let clause = scope?.clause
        let tableClause = clause == .queryTable || clause == .writeTable
        let columnClauses: [EditorClause] = [.selectList, .joinCondition, .where, .groupBy, .having,
                                             .orderBy, .setList]
        let columnClause = clause.map(columnClauses.contains) ?? false
        let aliasing = aliasable ? references : nil

        struct Hit {
            let suggestion: SQLSuggestion
            let quality: Int
            /// 0 for a name that can be written bare where the caret is, 1 for one that needs its
            /// qualifier.
            let away: Int
        }
        var hits: [Hit] = []
        var seen = Set<String>()
        func add(_ text: String, _ kind: SQLSuggestion.Kind, name: String? = nil, away: Int = 0) {
            guard !text.isEmpty,
                  let quality = SQLSuggestions.match(name ?? text, typed: prefix, keyword: kind == .keyword),
                  seen.insert("\(kind.rawValue):\(text.lowercased())").inserted else { return }
            hits.append(Hit(suggestion: SQLSuggestion(text: text, kind: kind), quality: quality, away: away))
        }

        /// A table written bare under a path, or with the qualifier its place needs.
        func addTable(_ parts: [String], away: Int) {
            guard let last = parts.last else { return }
            let name = parts.joined(separator: ".")
            add(SQLSuggestions.tableText(name, base: last, in: clause, references: aliasing), .table,
                name: last, away: away)
        }

        func offer(_ node: TreeNode) {
            switch node.kind {
            case .catalog, .database: add(node.title, .catalog)
            case .schema: add(node.title, .schema)
            case .table:
                if let connection {
                    let placed = qualifier(of: node, for: connection.kind, tab: tab)
                    addTable(placed.parts, away: placed.home ? 0 : 1)
                } else {
                    addTable([node.title], away: 0)
                }
            // A group is how the side bar files connections; it is not a name a statement can
            // use, so it is never a suggestion.
            case .group, .connection: break
            }
        }

        if path.isEmpty {
            if let connection, let root = connectionNode(for: connection.id) {
                loadDefaultScope(for: tab, connection: connection, root: root)
                func walk(_ nodes: [TreeNode]) {
                    for node in nodes {
                        offer(node)
                        walk(node.children ?? [])
                    }
                }
                walk(root.children ?? [])
            }
        } else if path.count == 1, let references, let connection,
                  case let named = Self.relations(named: path[0], in: references), !named.isEmpty {
            // `o.` where `o` is an alias or a table of the statement: that relation's columns. An
            // alias the statement gives twice, a subquery and a CTE with no column list name no
            // columns we can vouch for, so they offer none.
            if named.count == 1 {
                for name in columnNames(of: named[0], in: references, tab: tab, connection: connection) {
                    add(name, .column)
                }
            }
        } else if let scope = node(atPath: path, connectionID: tab.connectionID) {
            if scope.kind == .table {
                // `schema.table.`: the table's own columns, read once if the tree has not got them.
                if let columns = scope.columns {
                    for column in columns { add(column.name, .column) }
                } else {
                    loadColumns(of: scope)
                }
            // A node whose children are not loaded yet is asked for them, so `FROM hive.` fills in
            // without the user having to expand that catalog in the sidebar first.
            } else if let children = scope.children {
                for child in children {
                    switch child.kind {
                    case .catalog, .database: add(child.title, .catalog)
                    case .schema: add(child.title, .schema)
                    case .table: addTable([child.title], away: 0)
                    case .group, .connection: break
                    }
                }
            } else {
                requestLoad(of: scope)
            }
        }
        // A path that resolves to nothing offers nothing. Falling back to the whole tree would
        // reproduce the bug this method exists to fix: a wrong table from another schema is worse
        // than no suggestion at all.

        // A column is only in scope where a bare name could be one — never after a path, where the
        // next word is an object and a column name would be noise, and never where a table is
        // being named.
        if path.isEmpty {
            // The statement's own CTEs are tables where a table is named.
            if clause == .queryTable, let references {
                for cte in references.ctes { addTable([cte.name], away: 0) }
            }
            // Where a column is written, the columns of the relations the statement names come
            // first; the last run's columns, which may be from another query, after them.
            var relationColumns = false
            if columnClause, let references, let connection {
                for relation in references.relations.prefix(Self.relationLimit) {
                    for name in columnNames(of: relation, in: references, tab: tab, connection: connection) {
                        add(name, .column)
                        relationColumns = true
                    }
                }
            }
            if !tableClause {
                // The select list's own names, where this clause and driver accept them. Where they
                // are refused, a column of the last run that is one of them is not offered either:
                // `HAVING total > 1` fails on PostgreSQL even though `total` was a column last time.
                let aliases = scope?.selectAliases ?? []
                let accepted = scope.map { SQLSuggestions.offersSelectAliases(in: $0.clause, for: driver) } ?? false
                if accepted {
                    for alias in aliases { add(alias, .column, name: SQLSuggestions.unquoted(alias)) }
                }
                let refused = accepted ? [] : Set(aliases.map { SQLSuggestions.unquoted($0).lowercased() })
                for column in tab.columns where !refused.contains(column.name.lowercased()) {
                    add(column.name, .column, away: relationColumns ? 1 : 0)
                }
            }
            for keyword in SQLSuggestions.keywords { add(keyword, .keyword) }
        }

        /// The order the kinds are shown in for the clause the caret is in.
        func group(_ kind: SQLSuggestion.Kind) -> Int {
            if tableClause {
                switch kind {
                case .table: return 0
                case .schema: return 1
                case .catalog: return 2
                case .keyword: return 3
                case .column: return 4
                }
            }
            if columnClause {
                switch kind {
                case .column: return 0
                case .keyword: return 1
                case .table: return 2
                case .schema: return 3
                case .catalog: return 4
                }
            }
            return kind.rawValue
        }

        return hits
            .sorted { lhs, rhs in
                // A name the typed text starts always beats one it only occurs in.
                let lhsPrefix = lhs.quality == 0 ? 0 : 1, rhsPrefix = rhs.quality == 0 ? 0 : 1
                if lhsPrefix != rhsPrefix { return lhsPrefix < rhsPrefix }
                let lhsGroup = group(lhs.suggestion.kind), rhsGroup = group(rhs.suggestion.kind)
                if lhsGroup != rhsGroup { return lhsGroup < rhsGroup }
                if lhs.away != rhs.away { return lhs.away < rhs.away }
                if lhs.quality != rhs.quality { return lhs.quality < rhs.quality }
                if lhs.suggestion.text.count != rhs.suggestion.text.count {
                    return lhs.suggestion.text.count < rhs.suggestion.text.count
                }
                return lhs.suggestion.text < rhs.suggestion.text
            }
            .prefix(Self.suggestionLimit)
            .map(\.suggestion)
    }

    /// How many suggestions are kept. The popup shows `EditorCompletion.visibleRows` of them and
    /// scrolls: cutting the list at eight hid the keywords and the columns behind the tables.
    static let suggestionLimit = 40

    /// The name a table node is written under at the caret: bare when it is in the tab's default
    /// database and schema (where the server looks for a bare name), otherwise with as much
    /// qualifier as it needs.
    func qualifier(of node: TreeNode, for kind: ConnectionKind, tab: QueryTab)
        -> (parts: [String], home: Bool) {
        func same(_ name: String?, _ home: String) -> Bool {
            guard let name, !home.isEmpty else { return false }
            return name.compare(home, options: .caseInsensitive) == .orderedSame
        }
        let title = node.title
        switch kind {
        case .trino:
            let sameCatalog = same(node.database, database(for: tab))
            if sameCatalog, same(node.schema, schema(for: tab)) { return ([title], true) }
            guard let schemaName = node.schema else { return ([title], false) }
            if sameCatalog { return ([schemaName, title], false) }
            return (([node.database, schemaName].compactMap { $0 }) + [title], false)
        case .postgres:
            if same(node.schema, schema(for: tab)) { return ([title], true) }
            return ((node.schema.map { [$0] } ?? []) + [title], false)
        case .mysql:
            if same(node.database, database(for: tab)) { return ([title], true) }
            return ((node.database.map { [$0] } ?? []) + [title], false)
        }
    }

    /// Asks for a level of the tree unless it already failed once. `loadChildren` itself skips a
    /// node that is loaded or loading.
    func requestLoad(of node: TreeNode) {
        guard node.error == nil else { return }
        loadChildren(of: node)
    }

    /// Walks down the tab's default database and schema and asks for the first level that is not
    /// loaded, so a bare `FROM wil` finds its tables without the sidebar having been expanded. One
    /// level per call: the answer arrives through `completion.catalogLoaded()`, the editor asks
    /// again, and the next call takes the next step.
    private func loadDefaultScope(for tab: QueryTab, connection: Connection, root: TreeNode) {
        var wanted: [String] = switch connection.kind {
        case .trino: [database(for: tab), schema(for: tab)]
        case .postgres: [schema(for: tab)]
        case .mysql: [database(for: tab)]
        }
        if let blank = wanted.firstIndex(where: \.isEmpty) { wanted.removeSubrange(blank...) }
        if let scope = descend(wanted, from: root, connection: connection) { requestLoad(of: scope) }
    }

    /// The node `path` names below a connection's node, asking for the first level that is not
    /// loaded on the way (so the answer is nil until that arrives and the caller asks again). A
    /// path that names nothing is nil too, and asks for nothing more.
    private func descend(_ path: [String], from root: TreeNode, connection: Connection) -> TreeNode? {
        var current = root
        var rest = path[...]
        while let name = rest.first {
            guard let children = current.children else {
                requestLoad(of: current)
                return nil
            }
            // "Show all databases" puts the database level back under a Postgres connection.
            if connection.kind == .postgres, current.kind == .connection,
               children.contains(where: { $0.kind == .database }) {
                guard let own = children.first(where: {
                    $0.kind == .database
                        && $0.title.compare(connection.database, options: .caseInsensitive) == .orderedSame
                }) else { return nil }
                current = own
                continue
            }
            guard let next = children.first(where: {
                $0.title.compare(name, options: .caseInsensitive) == .orderedSame
            }) else { return nil }
            rest = rest.dropFirst()
            current = next
        }
        return current
    }

    /// How many of a statement's relations are asked about for a bare column.
    static let relationLimit = 8

    /// The relations an `o.` can mean: those aliased `o`, or else the ones with no alias called
    /// `o`. More than one is an ambiguity, and the caller offers nothing for it.
    static func relations(named qualifier: String, in references: EditorReferences) -> [EditorRelation] {
        func same(_ a: String?, _ b: String) -> Bool {
            a?.compare(b, options: .caseInsensitive) == .orderedSame
        }
        let aliased = references.relations.filter { same($0.alias, qualifier) }
        if !aliased.isEmpty { return aliased }
        return references.relations.filter { $0.alias == nil && same($0.name, qualifier) }
    }

    /// The tree path a relation of a statement names, with the tab's database and schema filling
    /// in the leading parts the statement left out, the way the server would read it. Nil when a
    /// part is missing and the tab has no default for it.
    func treePath(of relation: EditorRelation, tab: QueryTab, kind: ConnectionKind) -> [String]? {
        guard let name = relation.name else { return nil }
        let depth = kind.levels.count
        let written = [relation.catalog, relation.schema].compactMap { $0 } + [name]
        let parts = Array(written.suffix(depth))
        let defaults: [String] = switch kind {
        case .trino: [database(for: tab), schema(for: tab)]
        case .postgres: [schema(for: tab)]
        case .mysql: [database(for: tab)]
        }
        let leading = Array(defaults.prefix(depth - parts.count))
        guard leading.allSatisfy({ !$0.isEmpty }) else { return nil }
        return leading + parts
    }

    /// The columns of one relation of the statement, when they can be vouched for. A table's come
    /// from its node in the tree and are read once if the node has none yet (the answer lands
    /// through `completion.catalogLoaded()`); a CTE's from the column list it declares. A subquery
    /// and a CTE with no list have none, which is better than a guess.
    func columnNames(of relation: EditorRelation, in references: EditorReferences, tab: QueryTab,
                     connection: Connection) -> [String] {
        func cte(_ name: String) -> EditorCte? {
            references.ctes.first { $0.name.compare(name, options: .caseInsensitive) == .orderedSame }
        }
        switch relation.kind {
        case .subquery:
            return []
        case .cte:
            return relation.name.flatMap(cte)?.columns ?? []
        case .table:
            guard let name = relation.name else { return [] }
            if relation.catalog == nil, relation.schema == nil, let own = cte(name) { return own.columns }
            guard let root = connectionNode(for: connection.id),
                  let path = treePath(of: relation, tab: tab, kind: connection.kind),
                  let table = descend(path, from: root, connection: connection),
                  table.kind == .table else { return [] }
            if let columns = table.columns { return columns.map(\.name) }
            loadColumns(of: table)
            return []
        }
    }

    /// Reads one table's columns for completion, once: nothing while it is loading or failed
    /// before, and at most `completionColumnReadLimit` at a time.
    ///
    /// The engine has no columns command, so this asks the way the object inspector does: a
    /// `preview` of `SELECT *` under a one-row cap, which the server describes before it sends a
    /// row (`objectColumnsSQL`). It is a real query, which is why it is only sent for a table the
    /// statement names, and never twice for the same one.
    func loadColumns(of table: TreeNode) {
        guard table.kind == .table, table.columns == nil, !table.columnsLoading,
              table.columnsError == nil,
              completionColumnReads < Self.completionColumnReadLimit,
              let connection = connections.first(where: { $0.id == table.connectionID }) else { return }
        var env: [String: String]
        do {
            env = try Self.connectionEnvironment(connection, password: Self.storedPassword(for: connection.id))
        } catch {
            table.columnsError = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            return
        }
        if let database = table.database { env["DB_DATABASE"] = database }
        if let schema = table.schema { env["DB_SCHEMA"] = schema }
        env["SQL"] = objectColumnsSQL(database: table.database, schema: table.schema,
                                      table: table.title, for: connection.kind)
        env["LIMIT"] = "1"
        env["RETRIES"] = "2"
        table.columnsLoading = true
        let token = UUID()
        table.loadToken = token
        completionColumnReads += 1
        engine.run("preview", env: env, onEvent: { event in
            guard table.loadToken == token else { return }
            switch event.event {
            case "columns": table.columns = event.columns ?? []
            case "error": table.columnsError = event.message ?? "Reading the table's columns failed."
            default: break
            }
        }, onExit: { [weak self] status, log in
            self?.completionColumnReads -= 1
            guard table.loadToken == token else { return }
            table.columnsLoading = false
            if table.columns == nil, table.columnsError == nil {
                table.columnsError = status == 0 ? "The server reported no columns."
                    : log.split(separator: "\n").last.map(String.init) ?? "exit status \(status)"
            }
            self?.completion.catalogLoaded()
        })
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

    /// Forgets what completion loaded about one object, so the next keystroke asks the engine
    /// again. Called after DDL (W12-T4), with the name as the statement wrote it: one to three
    /// parts, a missing leading part taken from the tab's own database and schema, the way the
    /// server would read it. `kind` is the level the name is at, a table or view by default;
    /// `CREATE SCHEMA hive.silver` names a `.schema`, and MySQL's `CREATE DATABASE` a `.database`.
    ///
    /// Two nodes are dropped: the object's own (a table's columns, once the tree holds them, or
    /// a schema's tables) and the one that lists it, because `CREATE` and `DROP` change that
    /// list. There is no second cache to clear: completion reads the object tree, so this
    /// resets `children` to "never loaded". A node the side bar shows open is left to the
    /// tree's own refresh, which reloads it and keeps its rows until the reply; clearing it here
    /// would blank rows the user is looking at.
    ///
    /// A name with more parts than its level has, a level the driver does not have, or a name
    /// that cannot be completed because the tab has no default for a missing part drops nothing.
    ///
    /// A reply still on its way for a dropped node is ignored: the node's load token changes, so
    /// what was asked before the DDL cannot repopulate it after.
    @MainActor
    func invalidateCompletionCatalog(for tab: QueryTab, object parts: [String],
                                     kind: TreeNode.Kind = .table) {
        guard let connection = connection(for: tab),
              let depth = connection.kind.levels.firstIndex(of: kind).map({ $0 + 1 }),
              (1...depth).contains(parts.count) else { return }
        let defaults: [String] = switch connection.kind {
        case .trino: [database(for: tab), schema(for: tab)]
        case .postgres: [schema(for: tab)]
        case .mysql: [database(for: tab)]
        }
        let leading = Array(defaults.prefix(depth - parts.count))
        guard leading.allSatisfy({ !$0.isEmpty }) else { return }
        let path = leading + parts

        var dropped: [TreeNode] = []
        if let own = node(atPath: path, connectionID: tab.connectionID) { dropped.append(own) }
        if path.count > 1 {
            if let parent = node(atPath: Array(path.dropLast()), connectionID: tab.connectionID) {
                dropped.append(parent)
            }
        } else if let root = connectionNode(for: connection.id) {
            dropped.append(root)
        }
        for node in dropped where !node.expanded {
            node.children = nil
            node.error = nil
            node.loading = false
            node.loadToken = nil
            node.columns = nil
            node.columnsError = nil
            node.columnsLoading = false
        }
    }
}
