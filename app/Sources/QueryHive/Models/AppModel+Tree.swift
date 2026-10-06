import AppKit
import SwiftUI

extension AppModel {
    var selectedNode: TreeNode? {
        guard let id = selectedNodeID else { return nil }
        return allNodes().first { $0.id == id }
    }

    /// Opens a table the way a database client does: a new tab holding `SELECT * FROM it` with the
    /// query already run, so the rows are on screen before anything has been typed. Double-clicking
    /// a table used to insert its name into whatever editor happened to be in front, which is the
    /// context-menu action; opening it is what the double-click is for.
    func openTable(_ node: TreeNode) {
        guard let name = node.insertableText else { return }
        newTab(connectionID: node.connectionID)
        guard let tab = selectedTab else { return }
        tab.title = node.title
        tab.sql = "SELECT * FROM \(name)"
        // The one place the app knows which table the grid is showing, because this is the one place
        // it wrote the SQL itself. A queued cell edit can be written back only from here.
        tab.sourceTable = name
        preview(tab)
        // Rows take the window. Opening a table is asking to *see* it, and the editor is still
        // there one click away; the previous behaviour showed the rows in a panel under a query the
        // user had not written.
        panelCollapsed = false
        panelExpanded = true
    }

    /// Opens a schema's (or a MySQL database's) objects in a tab of their own.
    ///
    /// One tab per scope: asking for the same schema twice brings the tab you already have to the
    /// front and reloads it, rather than stacking duplicates whose contents drift apart.
    func openObjects(_ node: TreeNode) {
        guard let connectionID = node.connectionID,
              connections.contains(where: { $0.id == connectionID }) else { return }
        let scope = ObjectScope(connectionID: connectionID,
                                catalog: node.database ?? "",
                                schema: node.schema ?? "")
        if let existing = tabs.first(where: { $0.objectScope == scope }) {
            selectedTabID = existing.id
            loadObjects(existing)
            return
        }
        let tab = QueryTab(title: node.title)
        tab.connectionID = node.connectionID
        tab.objectScope = scope
        tabs.append(tab)
        selectedTabID = tab.id
        panelExpanded = false
        loadObjects(tab)
        _ = connection
    }

    /// Runs the `objects` command for one object tab and fills it.
    ///
    /// The environment is built the same way every other browse command builds it, so a connection
    /// that browses in the tree lists objects here without any second set of rules.
    func loadObjects(_ tab: QueryTab) {
        // Cleared first, before anything can return early, because the clear belongs to "this
        // listing is about to be replaced" -- which the caller's request already decided -- and not
        // to whether the connection lookup below succeeds.
        //
        // Clearing at the *end* of the run was a race the user could lose: the rows are painted by
        // the `objects` event, which arrives before the run exits, so a click landing in that window
        // was wiped by the exit handler a moment later. The row stayed highlighted and the inspector
        // never appeared. Clearing here cannot be raced, because no new row is on screen yet.
        clearObjectSelection(tab)

        guard let scope = tab.objectScope,
              let connection = connections.first(where: { $0.id == scope.connectionID }) else {
            // A reason, not a quiet return, for the reason `selectObject` gives: an empty pane
            // claims this schema has no objects, and that is a claim about the server when the
            // truth is that nothing was ever asked. `objectLoading` is cleared with it, so a
            // request that failed before it started cannot leave the reload it replaced, or the
            // pane it belongs to, spinning under a spinner that has nothing left to wait for.
            tab.objectLoading = false
            tab.objectError = "The connection for this schema is gone."
            return
        }
        let env: [String: String]
        do {
            var built = try connectionEnvironment(connection)
            built["RETRIES"] = "2"
            // Blank means "whatever the connection already sets", which is right for both: a Trino
            // schema always names its catalog, and Postgres has no catalog level to name at all.
            if !scope.catalog.isEmpty { built["DB_DATABASE"] = scope.catalog }
            if !scope.schema.isEmpty { built["DB_SCHEMA"] = scope.schema }
            env = built
        } catch {
            tab.objectLoading = false
            tab.objectError = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            return
        }

        let token = UUID()
        tab.objectToken = token
        tab.objectLoading = true
        tab.objectError = nil
        tab.objectProcess = Engine.current.run("objects", env: env, onEvent: { event in
            guard tab.objectToken == token else { return }
            switch event.event {
            case "objects":
                tab.objectColumns = event.objectColumns ?? []
                tab.objectRows = event.data ?? []
                tab.objectLoading = false
            case "error":
                tab.objectError = event.message ?? "Listing the objects failed."
                tab.objectLoading = false
            default:
                break
            }
        }, onExit: { status, log in
            guard tab.objectToken == token else { return }
            tab.objectProcess = nil
            tab.objectLoading = false
            // A non-zero exit with no `error` event still has to say something: a driver that
            // refused the level, or a crash before the first emit, would otherwise leave the pane
            // spinning forever with nothing written to it.
            if status != 0, tab.objectError == nil {
                tab.objectError = log.split(separator: "\n").last.map(String.init)
                    ?? "Listing the objects failed."
            }
        })
    }

    /// Drops the object screen's selection and whatever detail it had fetched.
    func clearObjectSelection(_ tab: QueryTab) {
        tab.objectSelection = nil
        tab.objectDetailColumns = []
        tab.objectDetailError = nil
        tab.objectDetailTable = nil
        tab.objectDetailLoading = false
        // The token, so a detail fetch still in flight cannot land on the cleared pane.
        tab.objectDetailToken = nil
        tab.objectDetailProcess?.terminate()
        tab.objectDetailProcess = nil
    }

    /// Selects one row of the object screen and fetches that table's columns.
    ///
    /// The click is what asks for the detail, not a timer or a prefetch: this is one round trip
    /// per table on a server that may be far away, and the user asked for exactly one row.
    func selectObject(_ tab: QueryTab, row: Int) {
        guard let scope = tab.objectScope, let name = tab.objectName(at: row) else { return }
        tab.objectSelection = row
        tab.objectDetailColumns = []
        tab.objectDetailError = nil
        tab.objectDetailLoading = false
        tab.objectDetailTable = name

        // Every failure below writes a reason rather than returning quietly. A silent return would
        // leave the inspector saying "No columns reported", which is a claim about the table when
        // the truth is that this app never asked about it.
        guard let connection = connections.first(where: { $0.id == scope.connectionID }) else {
            tab.objectDetailError = "The connection for this schema is gone."
            return
        }
        let env: [String: String]
        do {
            var built = try connectionEnvironment(connection)
            built["RETRIES"] = "2"
            if !scope.catalog.isEmpty { built["DB_DATABASE"] = scope.catalog }
            if !scope.schema.isEmpty { built["DB_SCHEMA"] = scope.schema }
            // A `preview` of `SELECT *` under a one-row cap is how the column list is asked for: the
            // engine describes the result set before it sends rows, so the `columns` event arrives
            // without waiting for data, and `LIMIT=1` keeps a wide table from being read to answer a
            // question about its shape. `count` is deliberately not used: it answers a number, not a
            // shape, and it would run the statement over every row.
            built["SQL"] = objectColumnsSQL(database: scope.catalog.isEmpty ? nil : scope.catalog,
                                            schema: scope.schema.isEmpty ? nil : scope.schema,
                                            table: name, for: connection.kind)
            built["LIMIT"] = "1"
            env = built
        } catch {
            tab.objectDetailError = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            return
        }

        let token = UUID()
        tab.objectDetailToken = token
        tab.objectDetailLoading = true
        tab.objectDetailProcess = Engine.current.run("preview", env: env, onEvent: { event in
            guard tab.objectDetailToken == token else { return }
            switch event.event {
            case "columns":
                tab.objectDetailColumns = event.columns ?? []
                tab.objectDetailLoading = false
            case "error":
                tab.objectDetailError = event.message ?? "Reading the table's columns failed."
                tab.objectDetailLoading = false
            default:
                break
            }
        }, onExit: { status, log in
            guard tab.objectDetailToken == token else { return }
            tab.objectDetailProcess = nil
            tab.objectDetailLoading = false
            if status != 0, tab.objectDetailError == nil {
                tab.objectDetailError = log.split(separator: "\n").last.map(String.init)
                    ?? "Reading the table's columns failed."
            }
        })
    }

    /// Opens one object-screen row as a query tab, the same way double-clicking the table in the
    /// tree does. Both go through `openTable`, so the two entry points cannot drift into producing
    /// different SQL for the same table.
    ///
    /// Takes the row rather than reading the selection, because a double-click or a right-click can
    /// land on a row the user has not single-clicked first, and a gesture that only worked on an
    /// already-selected row would look broken exactly when the user is moving fastest. Selecting
    /// first is also what makes the inspector agree with the tab that just opened.
    func openObject(_ tab: QueryTab, row: Int) {
        selectObject(tab, row: row)
        guard let node = objectNode(tab, row: row) else { return }
        openTable(node)
    }

    /// Inserts one row's table into the active query, the tree's own context-menu action.
    func insertObject(_ tab: QueryTab, row: Int) {
        selectObject(tab, row: row)
        guard let node = objectNode(tab, row: row) else { return }
        insert(node)
    }

    /// A tree-shaped node for one object-screen row, so the row can reuse the tree's own actions
    /// rather than growing a second copy of "how this driver spells a qualified name".
    private func objectNode(_ tab: QueryTab, row: Int) -> TreeNode? {
        guard let scope = tab.objectScope, let name = tab.objectName(at: row),
              let connection = connections.first(where: { $0.id == scope.connectionID }) else { return nil }
        let connectionNode = TreeNode.connection(connection)
        let parent: TreeNode
        switch connection.kind {
        case .trino:
            let catalog = TreeNode.catalog(scope.catalog, parent: connectionNode)
            parent = TreeNode.schema(scope.schema, parent: catalog)
        case .postgres:
            parent = TreeNode.schema(scope.schema, parent: connectionNode)
        case .mysql:
            parent = TreeNode.database(scope.catalog, parent: connectionNode)
        }
        return TreeNode.table(name, parent: parent)
    }

    func allNodes() -> [TreeNode] {
        var result: [TreeNode] = []
        func walk(_ nodes: [TreeNode]) {
            for node in nodes {
                result.append(node)
                walk(node.children ?? [])
            }
        }
        walk(tree)
        return result
    }

    /// Select a node and open whatever has to open for it to be visible.
    ///
    /// Selecting alone would leave a table inside a collapsed schema, which looks like nothing
    /// happened. The walk is over the same tree the sidebar draws, so a node that is not loaded
    /// yet is simply not found.
    func revealNode(_ id: String) {
        selectedNodeID = id
        func walk(_ nodes: [TreeNode]) -> Bool {
            for node in nodes {
                if node.id == id { return true }
                if let children = node.children, walk(children) {
                    node.expanded = true
                    return true
                }
            }
            return false
        }
        _ = walk(tree)
    }

    /// Rebuilds the roots from `connections`, reusing the node that already exists for an id so
    /// everything the user expanded stays expanded after a save, a rename or a delete.
    /// Rebuilds the root of the side bar: one node per group, then the connections that are in no
    /// group.
    ///
    /// Nodes are reused by id, which is what keeps an expanded connection expanded across a rename,
    /// a recolour or a move between groups. Reusing them matters more now that a move rebuilds the
    /// whole tree: without it, filing one connection would collapse every server the user had
    /// opened.
    func rebuildTree() {
        var existing: [String: TreeNode] = [:]
        for node in allNodes() { existing[node.id] = node }

        func node(for connection: Connection) -> TreeNode {
            let id = "c:\(connection.id.uuidString)"
            if let reused = existing[id] {
                reused.title = connection.name
                reused.color = connection.color
                // The driver is editable, and this node is reused across edits: without this a
                // connection switched from Trino to Postgres kept quoting with double quotes and
                // wearing the Trino mark.
                reused.connectionKind = connection.kind
                return reused
            }
            return TreeNode.connection(connection)
        }

        // A connection pointing at a group that is not in the file — a hand-edited
        // connections.json, or one written by a build that knew a group this one does not — is
        // shown at the top level rather than dropped. The tree is the only way to reach a saved
        // connection, so it must not be the place one disappears.
        let orphans = Set(connections.compactMap(\.group)).subtracting(groups.map(\.id))

        var rebuilt: [TreeNode] = groups.map { group in
            let id = "g:\(group.id.uuidString)"
            let node = existing[id] ?? TreeNode.group(group)
            node.title = group.name
            node.children = connections.filter { $0.group == group.id }.map(node(for:))
            return node
        }
        rebuilt += connections
            .filter { $0.group == nil || orphans.contains($0.group!) }
            .map(node(for:))
        tree = rebuilt

        if let selected = selectedNodeID, allNodes().contains(where: { $0.id == selected }) == false {
            selectedNodeID = nil
        }
    }

    /// The tree node for a connection, wherever it is filed.
    ///
    /// Not `allNodes()`: this answers the status bar's dot, which is read on every redraw, and
    /// walking a deep object tree for it would put that cost on the status bar. Groups are one
    /// level, so two levels of search covers the whole space.
    func connectionNode(for connectionID: UUID) -> TreeNode? {
        for node in tree {
            if node.connectionID == connectionID { return node }
            if let children = node.children,
               let found = children.first(where: { $0.connectionID == connectionID }) {
                return found
            }
        }
        return nil
    }

    /// What double-click and Return do on a row: **Open** on a table, **list its objects** on a
    /// schema or a MySQL database, and **expand** on everything else.
    ///
    /// A schema is where the objects are, so asking to see them is what a click there means;
    /// expanding is still one arrow away. A connection expands rather than opening the editor: that
    /// is what the gesture means in every other tree on the platform, and editing a connection is a
    /// deliberate act that lives in the context menu. A Postgres database is not a level whose
    /// objects can be listed (they live under a schema, and the engine refuses the question), so it
    /// expands like a Trino catalog does.
    func openNode(_ node: TreeNode) {
        switch node.kind {
        case .table: openTable(node)
        case .schema: openObjects(node)
        case .database where node.connectionKind != .postgres: openObjects(node)
        default:
            selectedNodeID = node.id
            toggleExpansion(node)
        }
    }

    func toggleExpansion(_ node: TreeNode) {
        node.expanded.toggle()
        if node.expanded { loadChildren(of: node) }
    }

    func expand(_ node: TreeNode) {
        guard !node.expanded else { return }
        node.expanded = true
        loadChildren(of: node)
    }

    /// Drops one node's loaded children so the next expansion fetches them again. A coordinator
    /// can grow a catalog while the window is open, so "Refresh" has to actually re-ask.
    func refresh(_ node: TreeNode) {
        node.children = nil
        node.error = nil
        node.loading = false
        if node.expanded { loadChildren(of: node) }
    }

    /// Collapses the whole tree back to one row per connection and forgets every fetch.
    func reloadTree() {
        for node in allNodes() {
            node.children = nil
            node.error = nil
            node.loading = false
            node.expanded = false
        }
        selectedNodeID = nil
    }

    /// Runs one engine browse command for a node that has never been expanded. `RETRIES` is
    /// dropped to 2: a typo'd catalog name should come back quickly, not after five backoffs.
    func loadChildren(of node: TreeNode) {
        guard node.children == nil, !node.loading else { return }
        guard let connection = connections.first(where: { $0.id == node.connectionID }) else { return }
        let command: String
        var env: [String: String]
        do {
            env = try Self.connectionEnvironment(connection, password: Self.storedPassword(for: connection.id))
        } catch {
            node.error = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            return
        }
        // Which command lists a node's children depends on the driver as much as on the level:
        // Trino's level under the connection is a catalog, MySQL's is a database, and Postgres
        // has neither — its database is fixed by the connection, so its first level is a schema.
        switch (connection.kind, node.kind) {
        case (.trino, .connection):
            command = "catalogs"
        case (.trino, .catalog):
            command = "schemas"
            env["DB_DATABASE"] = node.database ?? ""
        case (.trino, .schema), (.postgres, .schema):
            command = "tables"
            if let database = node.database { env["DB_DATABASE"] = database }
            env["DB_SCHEMA"] = node.schema ?? ""
        case (.postgres, .connection):
            // Postgres draws its schemas here by default, because the database is chosen on the
            // connection and the tree below it is that one database. "Show all databases" puts the
            // level back, so the answer to "which database holds this table" is on screen: the
            // engine's `catalogs` lists what `pg_database` says this user may connect to, and each
            // database node then lists its own schemas over a connection to *that* database.
            command = connection.showAllDatabases ? "catalogs" : "schemas"
            // "Show all schemas" reveals pg_catalog, information_schema and any pg_* schema the
            // engine otherwise hides. Only Postgres filters, so the flag is set here alone — and it
            // is also the flag the *database* list reads, where it means the same shape of thing:
            // the templates and the databases that refuse connections, which `pg_database` holds
            // and the tree leaves out. One switch, two levels, one word for both.
            env["DB_ALL_SCHEMAS"] = connection.showAllSchemas ? "1" : "0"
        case (.postgres, .database):
            // The schemas of the database the node names, not of the one the connection names.
            // `DB_DATABASE` is the whole of that: it is the database the driver opens, so every
            // listing under this node is answered by a connection to this database.
            command = "schemas"
            env["DB_DATABASE"] = node.database ?? ""
            env["DB_ALL_SCHEMAS"] = connection.showAllSchemas ? "1" : "0"
        case (.mysql, .connection):
            // MySQL's information_schema calls a database a CATALOG_NAME, so `catalogs` is its
            // database list. Its system databases are hidden unless the connection asks for them,
            // the same switch Postgres uses for its system schemas.
            command = "catalogs"
            env["DB_ALL_SCHEMAS"] = connection.showAllSchemas ? "1" : "0"
        case (.mysql, .database):
            command = "tables"
            env["DB_DATABASE"] = node.database ?? ""
        default:
            return
        }
        env["RETRIES"] = "2"
        node.loading = true
        node.error = nil
        var message: String?
        // `catalogs` means a catalog for Trino and a database for the other two; the command is
        // shared because it is the same question ("what is directly under the connection?"). A
        // Postgres connection only reaches it through "show all databases", and what it lists is
        // databases, so it belongs on the database side of this the same way MySQL does.
        let catalogNode = connection.kind == .trino ? TreeNode.catalog : TreeNode.database
        Engine.current.run(command, env: env, onEvent: { event in
            switch event.event {
            case "catalogs": node.children = (event.names ?? []).map { catalogNode($0, node) }
            case "schemas": node.children = (event.names ?? []).map { TreeNode.schema($0, parent: node) }
            case "tables": node.children = (event.names ?? []).map { TreeNode.table($0, parent: node) }
            case "error": message = event.message
            default: break
            }
        }, onExit: { status, log in
            node.loading = false
            guard status == 0 else {
                node.error = message ?? log.split(separator: "\n").last.map(String.init) ?? "exit status \(status)"
                return
            }
            if node.children == nil { node.children = [] }
        })
    }

    /// Double-clicking a table drops its quoted name into the active query, the way Navicat
    /// does, so the tree is useful without typing identifiers by hand.
    func insert(_ node: TreeNode) {
        guard let text = node.insertableText else { return }
        if selectedTab == nil { newTab(connectionID: node.connectionID) }
        selectedTab?.insertIntoSQL(text)
    }

    /// Fetches the catalogs a connection can see.
    ///
    /// The target fields used to offer only what the object tree had already loaded, which meant
    /// a user who had never expanded the connection got a plain text field where a dropdown
    /// belongs — the chevron only appears when there is something behind it. Asking the server
    /// when the popover opens is one round trip and makes the field what it looks like.
    func loadCatalogs(for connectionID: UUID?) {
        guard let connectionID, let connection = connections.first(where: { $0.id == connectionID }),
              connection.kind != .postgres   // Postgres has no catalog level; it would be a usage error
        else { return }
        let key = optionKey(connectionID, "")
        guard catalogOptions[connectionID] == nil, !loadingOptions.contains(key) else { return }
        guard var env = try? connectionEnvironment(connection) else { return }
        env["RETRIES"] = "2"
        loadingOptions.insert(key)
        Engine.current.run("catalogs", env: env, onEvent: { [weak self] event in
            guard event.event == "catalogs" else { return }
            self?.catalogOptions[connectionID] = event.names ?? []
        }, onExit: { [weak self] _, _ in
            // Left nil on failure on purpose: reopening the popover then retries, which is what
            // someone staring at an empty dropdown will do.
            self?.loadingOptions.remove(key)
        })
    }

    func loadSchemas(for connectionID: UUID?, catalog: String) {
        guard let connectionID, let connection = connections.first(where: { $0.id == connectionID }) else { return }
        let key = optionKey(connectionID, catalog)
        guard schemaOptions[key] == nil, !loadingOptions.contains(key) else { return }
        guard var env = try? connectionEnvironment(connection) else { return }
        env["RETRIES"] = "2"
        // Blank means "the connection's own database", which is the Postgres case.
        if !catalog.isEmpty { env["DB_DATABASE"] = catalog }
        loadingOptions.insert(key)
        Engine.current.run("schemas", env: env, onEvent: { [weak self] event in
            guard event.event == "schemas" else { return }
            self?.schemaOptions[key] = event.names ?? []
        }, onExit: { [weak self] _, _ in
            self?.loadingOptions.remove(key)
        })
    }

    private func optionKey(_ connectionID: UUID, _ catalog: String) -> String {
        "\(connectionID.uuidString)|\(catalog)"
    }

    func isLoadingOptions(for connectionID: UUID?, catalog: String = "") -> Bool {
        guard let connectionID else { return false }
        return loadingOptions.contains(optionKey(connectionID, catalog))
    }

    /// What a target field offers: everything the tree has loaded **plus** everything fetched on
    /// demand. Neither source alone is enough — the tree can be untouched, and a fetch can fail.
    func targetChoices(for connectionID: UUID?, kind: TreeNode.Kind, database: String = "") -> [String] {
        var names = Set(loadedNames(for: connectionID, kind: kind, database: database))
        if let connectionID {
            switch kind {
            case .catalog, .database: names.formUnion(catalogOptions[connectionID] ?? [])
            case .schema: names.formUnion(schemaOptions[optionKey(connectionID, database)] ?? [])
            case .group, .connection, .table: break
            }
        }
        return names.sorted()
    }

    /// Names the tree has already loaded at one level. These are the options behind the target
    /// fields' chevron; empty until that connection has been expanded, which is why the field is
    /// a combo box rather than a menu. `database` narrows to one parent where the level has one.
    ///
    /// Only the one connection's subtree is walked. This is read from the toolbar's computed
    /// properties, twice per redraw, and walking every connection's every node each time is what
    /// made the breadcrumb expensive on a server with thirty catalogs.
    func loadedNames(for connectionID: UUID?, kind: TreeNode.Kind, database: String = "") -> [String] {
        guard let connectionID,
              let root = connectionNode(for: connectionID) else { return [] }
        var names: [String] = []
        func walk(_ nodes: [TreeNode]) {
            for node in nodes {
                if node.kind == kind, database.isEmpty || node.database == database {
                    names.append(node.title)
                }
                // A node of the wanted kind has no children of that kind below it, so a match is
                // a leaf for this purpose and the descent can stop there.
                if node.kind != kind { walk(node.children ?? []) }
            }
        }
        walk(root.children ?? [])
        return names.sorted()
    }

    /// Switching a tab to a table destination starts from the connection's own catalog/schema and
    /// the output name, so the common case needs no typing.
    func prepareTableDestination(_ tab: QueryTab) {
        guard let connection = connection(for: tab) else { return }
        // The three drivers do not share a target shape: Postgres writes inside the database it
        // is already connected to, so its catalog field is meaningless, and MySQL has no schema
        // at all. Only the fields the driver actually has get a default.
        switch connection.kind {
        case .trino:
            if tab.trimmedCatalog.isEmpty { tab.targetCatalog = connection.database }
            if tab.trimmedSchema.isEmpty { tab.targetSchema = connection.schema }
        case .postgres:
            if tab.trimmedSchema.isEmpty { tab.targetSchema = connection.schema }
        case .mysql:
            if tab.trimmedCatalog.isEmpty { tab.targetCatalog = connection.database }
        }
        if tab.trimmedTable.isEmpty { tab.targetTable = tab.trimmedName }
    }

    /// A table's context menu asks for truncate or drop: the engine's `table_op`, through the same
    /// confirmation the run paths use.
    ///
    /// Whether a question is asked is `RunConfirmation.destructiveRequest`'s call: it asks at
    /// `confirm` (ADR-0027's one exception to confirm refusing DDL) and at `full` (D-11), and
    /// returns `nil` at `no_ddl`/`read_only`, where the engine refuses them before opening a
    /// connection. The app does not pre-refuse those two levels: the engine's sentence is the
    /// answer, and duplicating the rule in Swift is how the two would come to disagree.
    func requestTableOperation(_ operation: TableOperation, node: TreeNode) {
        guard let connection = connections.first(where: { $0.id == node.connectionID }) else { return }
        let statement = operation.statement(table: node.insertableText ?? node.title)
        if let request = RunConfirmation.destructiveRequest(for: statement,
                                                            title: "\(operation.title)?",
                                                            safeMode: connection.safeMode) {
            awaitConfirmation(request) { [weak self] in
                self?.runTableOperation(operation, node: node, confirmed: true)
            }
            return
        }
        runTableOperation(operation, node: node, confirmed: false)
    }

    /// Runs `table_op` for one node and reports what the server said.
    private func runTableOperation(_ operation: TableOperation, node: TreeNode, confirmed: Bool) {
        guard let connection = connections.first(where: { $0.id == node.connectionID }) else { return }
        let name = node.insertableText ?? node.title
        var env: [String: String]
        do {
            env = try connectionEnvironment(connection)
        } catch {
            notice = Notice(title: "\(operation.title) failed",
                            message: (error as? EngineLaunchError)?.message ?? error.localizedDescription)
            return
        }
        env["TABLE_OP"] = operation.rawValue
        env.merge(TableOperation.targetSettings(catalog: node.database, schema: node.schema,
                                                table: node.title)) { _, new in new }
        env.merge(RunConfirmation.approvalSettings(confirmed)) { _, new in new }

        var message: String?
        var rows: Int?
        _ = Engine.current.run("table_op", env: env, onEvent: { event in
            switch event.event {
            case "error": message = event.message
            case "done": rows = event.rows
            default: break
            }
        }, onExit: { [weak self] status, log in
            guard let self else { return }
            guard status == 0 else {
                let reason = message ?? log.split(separator: "\n").last.map(String.init)
                    ?? "The engine exited with status \(status)."
                self.notice = Notice(title: "\(operation.title) did not run", message: reason)
                return
            }
            var text = name
            if let rows, rows >= 0 { text += " · \(pluralized(rows, "row"))" }
            self.notice = Notice(title: "\(operation.title) done", message: text)
            // The table is gone or empty, so the sibling list the node was drawn from is stale.
            if let parent = self.parent(of: node) { self.refresh(parent) }
        })
    }

    /// The node's parent in the tree, or `nil` for a root. A walk, used only after a table
    /// operation rather than on any drawing path.
    func parent(of node: TreeNode) -> TreeNode? {
        allNodes().first { $0.children?.contains(where: { $0 === node }) == true }
    }
}
