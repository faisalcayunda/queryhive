import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// App-level state: the saved connections, the object tree built from them, and the open query
/// tabs. One model keeps the wiring simple — every view reads the single environment object —
/// while each tab owns its own SQL, destination and run state, because Navicat lets several
/// queries be in flight at once.
@Observable
final class AppModel {
    static let sqlContentTypes = ["sql", "txt"].compactMap { UTType(filenameExtension: $0) }

    // MARK: Connections

    var connections: [Connection] = []

    // MARK: Object tree

    var tree: [TreeNode] = []
    var selectedNodeID: String?
    var treeFilter = ""

    // MARK: Query tabs

    var tabs: [QueryTab] = []
    var selectedTabID: UUID?

    // MARK: Layout

    var sidebarWidth: CGFloat = 252
    var panelHeight: CGFloat = 232
    var panelCollapsed = false

    // MARK: Sheets and alerts

    var editingConnection: ConnectionEditorTarget?
    var notice: Notice?

    private var tabCounter = 0

    init() {
        let loaded = ConnectionStore.load()
        connections = loaded.connections
        notice = loaded.notice
        rebuildTree()
        newTab()
    }

    // MARK: Derived

    var selectedTab: QueryTab? { tabs.first { $0.id == selectedTabID } }

    var selectedConnection: Connection? {
        guard let id = selectedTab?.connectionID else { return nil }
        return connections.first { $0.id == id }
    }

    func connection(for tab: QueryTab) -> Connection? {
        guard let id = tab.connectionID else { return nil }
        return connections.first { $0.id == id }
    }

    var selectedNode: TreeNode? {
        guard let id = selectedNodeID else { return nil }
        return allNodes().first { $0.id == id }
    }

    /// What the status bar names on the left. With no tab open there is no *active* connection,
    /// which is not the same as having none: saying "No connection" next to a full object tree
    /// reads as a failure.
    var statusConnection: String {
        if let connection = selectedConnection {
            return "\(connection.name) · \(connection.displayTarget)"
        }
        if connections.isEmpty { return "No connections" }
        return selectedTab == nil ? "No query open" : "No connection selected"
    }

    // MARK: Tabs

    func newTab(connectionID: UUID? = nil) {
        tabCounter += 1
        let tab = QueryTab(title: "Query \(tabCounter)")
        tab.connectionID = connectionID ?? selectedTab?.connectionID ?? connections.first?.id
        if let path = UserDefaults.standard.string(forKey: "lastOutputDirectory") {
            tab.outputDirectory = URL(fileURLWithPath: path)
        }
        if let raw = UserDefaults.standard.string(forKey: "lastFormat"), let format = ExportFormat(rawValue: raw) {
            tab.format = format
        }
        tabs.append(tab)
        selectedTabID = tab.id
    }

    func closeTab(_ id: UUID) {
        guard let index = tabs.firstIndex(where: { $0.id == id }) else { return }
        tabs[index].process?.terminate()
        tabs.remove(at: index)
        if selectedTabID == id {
            selectedTabID = tabs.indices.contains(index) ? tabs[index].id : tabs.last?.id
        }
    }

    func closeSelectedTab() {
        guard let id = selectedTabID else { return }
        closeTab(id)
    }

    func selectTab(_ id: UUID) {
        selectedTabID = id
        if let connectionID = tabs.first(where: { $0.id == id })?.connectionID,
           let node = tree.first(where: { $0.connectionID == connectionID }) {
            selectedNodeID = node.id
        }
    }

    func rememberDestination(_ tab: QueryTab) {
        UserDefaults.standard.set(tab.outputDirectory?.path, forKey: "lastOutputDirectory")
        UserDefaults.standard.set(tab.format.rawValue, forKey: "lastFormat")
    }

    // MARK: Connections

    func presentConnectionEditor(_ connectionID: UUID?, startAtURL: Bool = false,
                                 previewTestCount: Int? = nil) {
        editingConnection = ConnectionEditorTarget(connectionID, startAtURL: startAtURL,
                                                   previewTestCount: previewTestCount)
    }

    /// Whether the target popover is open. On the model rather than in the view so the snapshot
    /// tool can open it — it is otherwise unreachable, and it went a long time unseen.
    var targetPopoverOpen = false

    /// Set while a delete is waiting for the user to confirm. Held on the model rather than in a
    /// row so the tree's context menu and the editor's Delete button ask the same question.
    var pendingDeletion: UUID?

    var pendingDeletionName: String {
        guard let id = pendingDeletion else { return "this connection" }
        return connections.first { $0.id == id }?.name ?? "this connection"
    }

    func requestDelete(_ id: UUID) {
        pendingDeletion = id
    }

    /// Removes a connection, its Keychain item, and its hold on any open tab. The JSON is written
    /// before the Keychain so a failure never leaves a saved connection whose password is gone.
    func deleteConnection(_ id: UUID) {
        var next = connections
        next.removeAll { $0.id == id }
        do {
            try ConnectionStore.save(next)
        } catch {
            notice = Notice(title: "Couldn't delete connection", message: error.localizedDescription)
            return
        }
        connections = next
        do {
            try ConnectionKeychain.delete(for: id)
        } catch {
            notice = Notice(title: "Couldn't delete the Keychain password", message: error.localizedDescription)
        }
        for tab in tabs where tab.connectionID == id {
            tab.connectionID = connections.first?.id
        }
        rebuildTree()
    }

    /// Copies a connection under a new name. The password comes along: a duplicate that silently
    /// connects as nobody would be a worse outcome than not duplicating at all.
    func duplicateConnection(_ id: UUID) {
        guard let original = connections.first(where: { $0.id == id }) else { return }
        var copy = original
        copy.id = UUID()
        copy.name = "\(original.name) copy"
        var next = connections
        next.append(copy)
        do {
            try ConnectionStore.save(next)
        } catch {
            notice = Notice(title: "Couldn't duplicate the connection", message: error.localizedDescription)
            return
        }
        connections = next
        if let password = try? ConnectionKeychain.get(for: id), !password.isEmpty {
            do {
                try ConnectionKeychain.set(password, for: copy.id)
            } catch {
                notice = Notice(title: "Duplicated, but without the password",
                                 message: "Couldn't write the password for \(copy.name) to Keychain: \(error.localizedDescription). Set it in the connection editor.")
            }
        }
        rebuildTree()
    }

    /// Recolours a saved connection. The colour is the user's own tag — it is what the sidebar
    /// row and the tree tile are painted with.
    func setColor(_ color: ConnectionColor, for id: UUID) {
        guard let index = connections.firstIndex(where: { $0.id == id }) else { return }
        var next = connections
        next[index].color = color
        do {
            try ConnectionStore.save(next)
        } catch {
            notice = Notice(title: "Couldn't save the colour", message: error.localizedDescription)
            return
        }
        connections = next
        rebuildTree()
    }

    /// Opens a `.sql` file into a fresh query tab rather than the one that is already open, so
    /// running a script never overwrites work in progress.
    func runSQLFile(connectionID: UUID) {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = Self.sqlContentTypes
        panel.allowsMultipleSelection = false
        panel.message = "Choose a .sql file to open in a new query"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        newTab(connectionID: connectionID)
        selectedTab?.loadSQL(from: url)
    }

    func connectionName(id: UUID) -> String {
        connections.first { $0.id == id }?.name ?? "connection"
    }

    // MARK: Object tree

    private func allNodes() -> [TreeNode] {
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

    /// Rebuilds the roots from `connections`, reusing the node that already exists for an id so
    /// everything the user expanded stays expanded after a save, a rename or a delete.
    func rebuildTree() {
        var existing: [String: TreeNode] = [:]
        for node in allNodes() { existing[node.id] = node }
        tree = connections.map { connection in
            let id = "c:\(connection.id.uuidString)"
            if let node = existing[id] {
                node.title = connection.name
                node.color = connection.color
                return node
            }
            return TreeNode.connection(connection)
        }
        if let selected = selectedNodeID, allNodes().contains(where: { $0.id == selected }) == false {
            selectedNodeID = nil
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
            env = try Self.connectionEnvironment(connection, password: ConnectionKeychain.get(for: connection.id))
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
            command = "schemas"
        case (.mysql, .connection):
            // MySQL's information_schema calls a database a CATALOG_NAME, so `catalogs` is
            // exactly `SHOW DATABASES`.
            command = "catalogs"
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
        // `catalogs` means a catalog for Trino and a database for MySQL; the command is shared
        // because it is the same question ("what is directly under the connection?").
        let catalogNode = connection.kind == .mysql ? TreeNode.database : TreeNode.catalog
        Engine.run(command, env: env, onEvent: { event in
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

    // MARK: Target options, fetched on demand

    /// Catalogs (MySQL: databases) fetched for a connection, keyed by connection id.
    private var catalogOptions: [UUID: [String]] = [:]
    /// Schemas fetched for one catalog, keyed by "connection|catalog". Postgres has no catalog
    /// level, so its key ends in an empty catalog.
    private var schemaOptions: [String: [String]] = [:]
    /// Keys with a fetch in flight, so a field can say it is working instead of looking empty.
    private(set) var loadingOptions: Set<String> = []

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
        Engine.run("catalogs", env: env, onEvent: { [weak self] event in
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
        Engine.run("schemas", env: env, onEvent: { [weak self] event in
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
            case .connection, .table: break
            }
        }
        return names.sorted()
    }

    /// Names the tree has already loaded at one level. These are the options behind the target
    /// fields' chevron; empty until that connection has been expanded, which is why the field is
    /// a combo box rather than a menu. `database` narrows to one parent where the level has one.
    func loadedNames(for connectionID: UUID?, kind: TreeNode.Kind, database: String = "") -> [String] {
        guard let connectionID else { return [] }
        return allNodes()
            .filter { node in
                node.connectionID == connectionID && node.kind == kind
                    && (database.isEmpty || node.database == database)
            }
            .map(\.title)
            .sorted()
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

    // MARK: Editor suggestions

    /// The one completion list on screen. Only the selected tab's editor is visible at a time, so
    /// this is app-level rather than per-tab state.
    var completion = EditorCompletion()

    /// Candidates for the word under the caret.
    ///
    /// Sources, in the order they are ranked: object names already loaded in the tree, the columns
    /// the last run reported, then SQL keywords. Objects come from the tree, so a completely
    /// collapsed tree offers keywords only — browsing is a network round trip per level and
    /// blocking a keystroke on one would be worse than a short list.
    func suggestions(for tab: QueryTab, prefix: String, qualified: Bool) -> [SQLSuggestion] {
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

        for node in allNodes() where matches(node.title) {
            switch node.kind {
            case .catalog, .database: add(node.title, .catalog)
            case .schema: add(node.title, .schema)
            case .table: add(node.title, .table)
            case .connection: break
            }
        }
        for column in tab.columns where matches(column.name) {
            add(column.name, .column)
        }
        // After a `.` the next word can only be an object, so a keyword would just be noise.
        if !qualified {
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

    // MARK: Running a tab

    func runBlockedReason(for tab: QueryTab) -> String? {
        if connections.isEmpty { return "Add a connection first" }
        if connection(for: tab) == nil { return "Choose a connection" }
        if !tab.hasSQL { return "Write a query" }
        switch tab.destination {
        case .file:
            if tab.trimmedName.isEmpty { return "Name the output file" }
            if tab.outputDirectory == nil { return "Choose an output folder" }
        case .table:
            if tab.trimmedCatalog.isEmpty { return "Name the target catalog" }
            if tab.trimmedSchema.isEmpty { return "Name the target schema" }
            if tab.trimmedTable.isEmpty { return "Name the target table" }
        }
        return nil
    }

    func runSelectedTab() {
        guard let tab = selectedTab else { return }
        run(tab)
    }

    func stopSelectedTab() {
        guard let tab = selectedTab else { return }
        stop(tab)
    }

    func stop(_ tab: QueryTab) {
        guard tab.stage == .running, !tab.stopping else { return }
        tab.stopping = true
        tab.cancelled = true
        tab.note(.warning, "Stopping…")
        tab.process?.terminate()
    }

    func run(_ tab: QueryTab) {
        guard tab.stage != .running else { return }
        guard let connection = connection(for: tab) else { return }
        let env: [String: String]
        do {
            env = try overrides(for: tab, connection: connection)
        } catch {
            let message = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            tab.stage = .failed
            tab.failure = message
            tab.failureDetail = ""
            tab.note(.error, message)
            tab.panel = .log
            return
        }
        rememberDestination(tab)
        // Navicat opens the message pane when a statement runs; a collapsed panel would hide
        // the only place the progress goes.
        panelCollapsed = false
        tab.stage = .running
        tab.step = nil
        tab.rows = 0
        tab.columns = []
        tab.files = []
        tab.warnings = []
        tab.queryID = nil
        tab.failure = nil
        tab.failureDetail = ""
        tab.cancelled = false
        tab.stopping = false
        tab.startedAt = .now
        tab.finishedAt = .now
        tab.panel = .log
        tab.writtenTable = nil
        tab.note(.info, "\(connection.name) · \(connection.displayTarget)")
        tab.note(.info, tab.runSummary(for: connection.kind))

        let directory = tab.outputDirectory
        let command = tab.destination == .table ? "to_table" : "export"
        let run = UUID()
        tab.runToken = run
        var message: String?
        tab.process = Engine.run(command, env: env, onEvent: { [weak self] event in
            guard let self, tab.runToken == run else { return }
            if event.event == "error" { message = event.message }
            self.handle(event, in: tab)
        }, onExit: { status, log in
            guard tab.runToken == run else { return }
            tab.process = nil
            tab.stopping = false
            if tab.cancelled {
                tab.cancelled = false
                tab.stage = .idle
                tab.note(.warning, tab.destination == .table
                         ? "Stopped. The coordinator decides what happens to a partly written table."
                         : "Stopped. Partial files, if any, stay in \(directory?.path ?? "the output folder").")
                return
            }
            guard status == 0 else {
                tab.stage = .failed
                tab.failure = message ?? "The engine exited with status \(status)."
                tab.failureDetail = log
                tab.note(.error, tab.failure ?? "")
                tab.panel = .log
                return
            }
            // A table run reports the table it wrote, not a file list, so that is what proves it
            // finished rather than merely exited cleanly.
            let confirmed = tab.destination == .table ? tab.writtenTable != nil
                                                     : (!tab.files.isEmpty || tab.rows > 0)
            guard confirmed else {
                tab.stage = .failed
                tab.failure = "The engine exited without reporting completion."
                tab.failureDetail = log
                tab.note(.error, tab.failure ?? "")
                tab.panel = .log
                return
            }
            tab.finishedAt = .now
            tab.step = "finish"
            tab.stage = .done
            tab.panel = tab.destination == .table ? .files : (tab.files.isEmpty ? .log : .files)
        })
    }

    private func handle(_ event: Event, in tab: QueryTab) {
        switch event.event {
        case "error":
            tab.note(.error, event.message ?? "Engine error")
            tab.panel = .log
        case "step":
            tab.step = event.step
            if event.step == "connect" { tab.note(.info, "Connecting…") }
            if event.step == "write" { tab.note(.info, "Streaming rows into the writer") }
        case "start":
            tab.columns = event.columns ?? []
            tab.queryID = event.queryId
            tab.note(.info, "Query \(event.queryId ?? "(no id)") · \(pluralized(tab.columns.count, "column"))")
        case "progress":
            if let total = event.rows { tab.rows = total }
        case "done":
            tab.rows = event.rows ?? tab.rows
            tab.files = event.files ?? []
            tab.writtenTable = event.table
            tab.warnings = event.warnings ?? []
            tab.queryID = event.queryId ?? tab.queryID
            if let table = event.table {
                tab.note(.success, "\(pluralized(tab.rows, "row")) written to \(table)")
            } else {
                tab.note(.success, "\(pluralized(tab.rows, "row")) written to \(pluralized(tab.files.count, "file"))")
            }
            for warning in tab.warnings { tab.note(.warning, warning) }
        default:
            break
        }
    }

    // MARK: Environment

    /// The connection variables plus the scheme, shared by the export flow, the object tree and
    /// the connection editor's Test button, so the engine contract lives in exactly one place.
    /// The password travels in its own variable and never inside `TRINO_URL`, so a URL echoed
    /// back in an error message can never carry the secret.
    static func connectionEnvironment(kind: ConnectionKind, host: String, port: Int, user: String,
                                      password: String?, database: String, schema: String,
                                      scheme: String, sslmode: String, verify: Bool) -> [String: String] {
        [
            "DB_KIND": kind.rawValue,
            "DB_HOST": host,
            "DB_PORT": String(port),
            "DB_USER": user,
            "DB_PASSWORD": password ?? "",
            "DB_DATABASE": database,
            "DB_SCHEMA": schema,
            // Trino's transport is a scheme; the other two express encryption through sslmode.
            // Sending the wrong one is harmless — the engine ignores what a driver has no use
            // for — but sending both would be confusing to read in a bug report.
            "DB_SCHEME": kind == .trino ? (scheme.isEmpty ? "http" : scheme) : "",
            "DB_SSLMODE": kind.hasSSLModes ? (sslmode.isEmpty ? kind.defaultSSLMode : sslmode) : "",
            "DB_INSECURE": verify ? "" : "1",
        ]
    }

    static func connectionEnvironment(_ connection: Connection, password: String?) -> [String: String] {
        connectionEnvironment(kind: connection.kind, host: connection.host, port: connection.port,
                              user: connection.user, password: password,
                              database: connection.database, schema: connection.schema,
                              scheme: connection.scheme, sslmode: connection.sslmode,
                              verify: connection.verify)
    }

    /// The connection variables for a saved connection, Keychain read included. Shared by `run`
    /// and the target-option fetches so those cannot drift from what a run actually sends.
    private func connectionEnvironment(_ connection: Connection) throws -> [String: String] {
        let password: String?
        do {
            password = try ConnectionKeychain.get(for: connection.id)
        } catch {
            throw EngineLaunchError(message: "Couldn't read the password for \(connection.name) from Keychain: \(error.localizedDescription)")
        }
        return Self.connectionEnvironment(connection, password: password)
    }

    /// Full environment for one export run: the connection plus the query, the destination and
    /// the per-format options. Throws rather than launching with a password the Keychain refused
    /// to hand over, which would silently export as the wrong identity.
    private func overrides(for tab: QueryTab, connection: Connection) throws -> [String: String] {
        let password: String?
        do {
            password = try ConnectionKeychain.get(for: connection.id)
        } catch {
            throw EngineLaunchError(message: "Couldn't read the password for \(connection.name) from Keychain: \(error.localizedDescription)")
        }
        var env = Self.connectionEnvironment(connection, password: password)
        env["SQL"] = tab.sql
        env["RETRIES"] = String(tab.retries)
        switch tab.destination {
        case .file:
            guard let directory = tab.outputDirectory else {
                throw EngineLaunchError(message: "Choose an output folder before running.")
            }
            env["FORMAT"] = tab.format.rawValue
            env["OUT_DIR"] = directory.path
            env["NAME"] = tab.trimmedName
            env["ZIP"] = tab.zip ? "1" : ""
            env["BATCH_SIZE"] = String(tab.batchSize)
            env["ROWS_PER_FILE"] = tab.splitRows > 0 ? String(tab.splitRows) : ""
            env["DELIMITER"] = tab.delimiter
            env["ENCODING"] = tab.encoding
            env["HEADER"] = tab.header ? "1" : "0"
            env["BOM"] = tab.bom ? "1" : ""
            env["NULL_TEXT"] = tab.nullText
            env["JSONL"] = tab.jsonl ? "1" : ""
            env["SQL_TABLE"] = tab.sqlTable
            env["SHEET"] = tab.sheet
            env["DBF_CHAR_WIDTH"] = String(tab.dbfCharWidth)
        case .table:
            // Kept separate from the session catalog: a query may well read from one catalog and
            // write into another.
            env["TARGET_CATALOG"] = tab.trimmedCatalog
            env["TARGET_SCHEMA"] = tab.trimmedSchema
            env["TARGET_TABLE"] = tab.trimmedTable
            env["WRITE_MODE"] = tab.writeMode.rawValue
        }
        return env
    }
}

/// Carries a ready-to-display message about why an engine launch never happened. `run` and the
/// connection editor catch this and surface `message` verbatim.
struct EngineLaunchError: Error {
    let message: String
}
