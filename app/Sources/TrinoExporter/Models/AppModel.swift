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

    /// The key-binding scheme, remembered across launches.
    ///
    /// Every binding in the app is read through `shortcut(for:)`, so switching this takes effect
    /// everywhere at once rather than only where someone remembered to look.
    var shortcutScheme: ShortcutScheme = {
        let raw = UserDefaults.standard.string(forKey: "shortcutScheme") ?? ""
        return ShortcutScheme(rawValue: raw) ?? .dbeaver
    }() {
        didSet { UserDefaults.standard.set(shortcutScheme.rawValue, forKey: "shortcutScheme") }
    }

    /// The binding for an action under the current scheme, or `nil` where this scheme leaves it
    /// unbound — in which case the view must not attach a shortcut at all.
    func shortcut(for action: ShortcutAction) -> KeyboardShortcut? {
        shortcutScheme.shortcut(for: action)?.keyboard
    }

    // MARK: Query tabs

    var tabs: [QueryTab] = []
    var selectedTabID: UUID?

    // MARK: Layout

    var sidebarWidth: CGFloat = 264
    /// Taller than it was. Run now puts rows in the panel rather than writing a file, so the
    /// panel is the main event instead of a message strip — 232pt showed four rows of it.
    /// The panel is the result grid now, so it starts at a bit over half the window and the editor
    /// keeps the rest — the proportion a query editor with a real result set wants, rather than the
    /// message strip this used to be.
    var panelHeight: CGFloat = 480

    /// But never more than this share of the window, so the editor cannot be squeezed to nothing
    /// when the window is small. Applied at layout time, not stored: a height the user dragged to
    /// on a large display must survive being restored on a small one.
    static let panelShare: CGFloat = 0.55
    var panelCollapsed = false

    /// The panel holding the whole workspace with the editor hidden behind it. This is what opening
    /// a table gives you -- rows, not a form you have to dismiss -- and the panel's minimise control
    /// is what puts the editor back. Unlike `panelCollapsed` it is not persisted: it is a view of the
    /// current tab's work, and reopening the app into a hidden editor would be a surprise.
    var panelExpanded = false

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

    /// What this tab should run against: its own cascade choice when it has one, otherwise the
    /// connection's configured default. Every run path goes through these, so the toolbar cascade
    /// and the export environment can never disagree about the context.
    func database(for tab: QueryTab) -> String {
        let picked = tab.contextDatabase.trimmingCharacters(in: .whitespaces)
        return picked.isEmpty ? (connection(for: tab)?.database ?? "") : picked
    }

    func schema(for tab: QueryTab) -> String {
        let picked = tab.contextSchema.trimmingCharacters(in: .whitespaces)
        return picked.isEmpty ? (connection(for: tab)?.schema ?? "") : picked
    }

    func connection(for tab: QueryTab) -> Connection? {
        guard let id = tab.connectionID else { return nil }
        return connections.first { $0.id == id }
    }

    var selectedNode: TreeNode? {
        guard let id = selectedNodeID else { return nil }
        return allNodes().first { $0.id == id }
    }

    /// What the status bar names on the left, and what the dot beside it reports.
    ///
    /// This follows the connection the user is *looking at*, which is not the same as the one the
    /// active tab would run against. It used to read `selectedTab?.connectionID`, so clicking a
    /// connection in the tree changed nothing until a query tab happened to point at it — and with
    /// no tab open it reported "No query open" over a tree full of connections. Selecting a row is
    /// the user saying which connection they mean; the status bar should agree with the tree they
    /// are looking at.
    ///
    /// The tree selection wins, and the active tab is the fallback for when nothing is selected —
    /// so a tab opened from the menu (which selects its connection's root, see `selectTab`) and a
    /// row clicked by hand both land here, and there is no state where the bar names one connection
    /// while the tree highlights another.
    var statusConnection: String {
        if let connection = statusConnectionTarget {
            // Name and whether it is answering, not where it lives: the host and port were already
            // on the title strip, and the status bar is the one place that has to answer "is this
            // thing talking to the server?" at a glance.
            return "\(connection.name) · \(connectionState(for: connection.id).label)"
        }
        if connections.isEmpty { return "No connections" }
        return "No connection selected"
    }

    /// Which connection the status bar is about: the tree's selection when there is one, the
    /// active tab's otherwise.
    var statusConnectionTarget: Connection? {
        if let id = selectedNodeID,
           let parsed = connectionID(fromNodeID: id),
           let connection = connections.first(where: { $0.id == parsed }) {
            return connection
        }
        return selectedConnection
    }

    /// The connection a node id belongs to, read from the id itself rather than by walking.
    ///
    /// Every id is built from its parent's: a connection is `c:<uuid>`, and each level below appends
    /// `/cat:`, `/db:`, `/sch:` or `/tab:`. The owning connection is therefore the leading
    /// `c:<uuid>` segment, so this is a string parse instead of `allNodes()` — a full recursive
    /// walk. That matters because the status bar asks for this twice per redraw (label and dot),
    /// and DESIGN.md already records that flattening the whole tree on every redraw is one of the
    /// three mistakes that made this window slow once.
    private func connectionID(fromNodeID id: String) -> UUID? {
        guard id.hasPrefix("c:") else { return nil }
        return UUID(uuidString: String(id.dropFirst(2).prefix { $0 != "/" }))
    }

    /// Whether the app is talking to a server right now.
    ///
    /// Derived from the connection's root node rather than tracked in a flag of its own: a browse
    /// command in flight *is* `node.loading`, a failed one leaves `node.error`, and a node with
    /// children has answered a browse at least once. A second flag could disagree with the tree the
    /// user is looking at; this cannot.
    func connectionState(for connectionID: UUID?) -> ConnectionState {
        guard let connectionID,
              let root = tree.first(where: { $0.connectionID == connectionID })
        else { return .disconnected }
        if root.loading { return .connecting }
        if root.error != nil { return .disconnected }
        // Not yet expanded is **idle**, not broken. Reporting `.disconnected` here announced a
        // failure that had not happened: a connection the user had simply not opened yet was
        // labelled the same as one that had just refused a connection, and on first launch that was
        // every connection in the tree.
        return root.children == nil ? .idle : .connected
    }

    // MARK: Tabs

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
        guard connections.contains(where: { $0.id == node.connectionID }) else { return }
        let scope = ObjectScope(connectionID: node.connectionID,
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
        guard let scope = tab.objectScope,
              let connection = connections.first(where: { $0.id == scope.connectionID }) else { return }
        guard var env = try? connectionEnvironment(connection) else { return }
        env["RETRIES"] = "2"
        // Blank means "whatever the connection already sets", which is right for both: a Trino
        // schema always names its catalog, and Postgres has no catalog level to name at all.
        if !scope.catalog.isEmpty { env["DB_DATABASE"] = scope.catalog }
        if !scope.schema.isEmpty { env["DB_SCHEMA"] = scope.schema }

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

    /// The export settings popover, on the model for the same reason.
    var exportSettingsOpen = false

    /// Which column's filter popover is open, by column index. On the model for the same reason as
    /// the two above: a snapshot has no way to click a header funnel.
    var filterPopoverColumn: Int?

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

    // MARK: Importing from another client

    /// Asks for Navicat's exported `.ncx` and adds what is in it.
    ///
    /// A panel rather than a remembered path: the file is wherever the user's own export put it,
    /// and naming a location from here would be a guess about their disk.
    func presentNavicatImport() {
        let panel = NSOpenPanel()
        panel.title = "Import Connections from Navicat"
        panel.message = "Choose the .ncx file Navicat wrote for File ▸ Export Connections."
        panel.prompt = "Import"
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        // A filter is applied only if the system actually has a type registered for `.ncx`. A panel
        // whose content types resolve to nothing would allow nothing, which is worse than allowing
        // too much — the parse either recognises the file or says so.
        if let ncx = UTType(filenameExtension: "ncx") { panel.allowedContentTypes = [ncx] }
        guard panel.runModal() == .OK, let url = panel.url else { return }
        importNavicatConnections(from: url)
    }

    /// Adds every connection in a Navicat export that this app has a driver for, with the password
    /// Navicat saved alongside it.
    ///
    /// **Adds, and never replaces.** An import is a bulk edit to a hand-curated list, so an
    /// existing connection is never touched: a name already in use gets a numeric suffix and the
    /// original keeps its host and its stored password. Matching by name and updating would
    /// silently rewrite a connection the user is working in — the host and the credential — which
    /// is a far worse outcome than a duplicate row they can delete.
    ///
    /// Keychain is written before the JSON, the same order the connection editor uses: if a write
    /// fails, the saved list never claims a password that is not there.
    func importNavicatConnections(from url: URL) {
        let export: NavicatImport.Export
        do {
            export = try NavicatImport.read(url)
        } catch {
            notice = Notice(title: "Couldn't read the Navicat export", message: error.localizedDescription)
            return
        }

        var next = connections
        var taken = Set(next.map(\.name))
        var added: [String] = []
        var refreshed: [String] = []
        var renamed: [String] = []
        var noPassword: [String] = []
        var noDatabase: [String] = []
        var tunnelled: [String] = []
        var keychainFailures: [String] = []

        for imported in export.connections {
            // Same name **and** same host means this is the very connection the export describes,
            // so it is refreshed in place instead of duplicated. Without this, re-importing — which
            // is exactly what someone does when the first import turns out to have got something
            // wrong — would append forty near-copies of connections they already have, and the only
            // way back would be deleting forty rows by hand.
            //
            // Matching on the host as well is what keeps that safe: a same-named entry pointing at a
            // different server is a different connection, and it still gets a suffix rather than
            // having its host rewritten underneath it.
            if let index = next.firstIndex(where: {
                $0.name == imported.name && $0.host == imported.host
            }) {
                let id = next[index].id
                next[index].kind = imported.kind
                next[index].port = imported.port
                next[index].user = imported.user
                next[index].database = imported.database
                // `schema` is deliberately not refreshed. The export has no schema to refresh it
                // with — see `NavicatImport` — so the only thing this could do is overwrite a value
                // the user set by hand with nothing.
                refreshed.append(imported.name)
                if imported.database.isEmpty { noDatabase.append(imported.name) }
                if !imported.sshHost.isEmpty { tunnelled.append(imported.name) }
                if let password = imported.password, !password.isEmpty {
                    do {
                        try ConnectionKeychain.set(password, for: id)
                    } catch {
                        keychainFailures.append(imported.name)
                    }
                } else {
                    noPassword.append(imported.name)
                }
                continue
            }

            // A name that is already here on a *different* host gets " 2", " 3", … rather than
            // overwriting. The loop is bounded by the list itself, so a file full of one repeated
            // name cannot spin.
            var name = imported.name
            if taken.contains(name) {
                var suffix = 2
                while taken.contains("\(name) \(suffix)") { suffix += 1 }
                let unique = "\(name) \(suffix)"
                renamed.append("\(name) → \(unique)")
                name = unique
            }
            taken.insert(name)

            let id = UUID()
            let connection = Connection(
                id: id,
                name: name,
                // The colour is this app's own tag, not something Navicat has. Cycling the palette
                // keeps a freshly imported list visually separable instead of uniformly grey.
                color: ConnectionColor.allCases[next.count % ConnectionColor.allCases.count],
                kind: imported.kind,
                host: imported.host,
                port: imported.port,
                scheme: imported.kind == .trino ? "https" : "https",
                // The export carries encryption settings this app spells differently, and guessing
                // a mapping would be worse than leaving the default: an sslmode that is wrong is a
                // connection failure, not a silent one.
                sslmode: "",
                user: imported.user,
                database: imported.database,
                schema: imported.schema,
                verify: false,
                showAllSchemas: false
            )

            if let password = imported.password, !password.isEmpty {
                do {
                    try ConnectionKeychain.set(password, for: id)
                } catch {
                    keychainFailures.append(name)
                }
            } else {
                noPassword.append(name)
            }
            if imported.database.isEmpty { noDatabase.append(name) }
            if !imported.sshHost.isEmpty { tunnelled.append(name) }
            added.append(name)
            next.append(connection)
        }

        do {
            try ConnectionStore.save(next)
        } catch {
            notice = Notice(title: "Couldn't save the imported connections",
                            message: error.localizedDescription)
            return
        }
        connections = next
        rebuildTree()
        notice = Notice(title: noticeTitle(added: added.count, refreshed: refreshed.count,
                                            skipped: export.skipped.count),
                        message: importSummary(added: added, refreshed: refreshed, renamed: renamed,
                                               noPassword: noPassword,
                                               noDatabase: noDatabase, tunnelled: tunnelled,
                                               keychainFailures: keychainFailures,
                                               skipped: export.skipped))
    }

    private func noticeTitle(added: Int, refreshed: Int, skipped: Int) -> String {
        var parts: [String] = []
        if added > 0 { parts.append("Imported \(added) \(added == 1 ? "connection" : "connections")") }
        if refreshed > 0 { parts.append("refreshed \(refreshed)") }
        if parts.isEmpty { parts.append("Nothing new") }
        var title = parts.joined(separator: ", ")
        if skipped > 0 { title += ", skipped \(skipped)" }
        return title + " from Navicat"
    }

    /// Every line here exists because silence about it would be a lie of omission — a connection
    /// that arrived without its password, or with a tunnel this app cannot open, will not connect,
    /// and the user would otherwise be left to find that out one failed test at a time.
    private func importSummary(added: [String], refreshed: [String], renamed: [String],
                               noPassword: [String], noDatabase: [String], tunnelled: [String],
                               keychainFailures: [String],
                               skipped: [NavicatImport.Skipped]) -> String {
        var lines: [String] = []
        if !added.isEmpty { lines.append("Added: " + added.joined(separator: ", ") + ".") }
        if !refreshed.isEmpty {
            lines.append("Updated in place, because the name and host already matched: "
                         + refreshed.joined(separator: ", ") + ".")
        }
        if lines.isEmpty { lines.append("Nothing was added or updated.") }
        if !renamed.isEmpty {
            lines.append("Renamed, because the name was already in use: " + renamed.joined(separator: ", ") + ".")
        }
        if !noPassword.isEmpty {
            lines.append("No password in the export for: " + noPassword.joined(separator: ", ") + ". Set one in the connection editor.")
        }
        if !keychainFailures.isEmpty {
            lines.append("Password couldn't be saved to Keychain for: " + keychainFailures.joined(separator: ", ") + ".")
        }
        if !noDatabase.isEmpty {
            lines.append("No database in the export for: " + noDatabase.joined(separator: ", ") + ". Pick one in the connection editor before browsing.")
        }
        if !tunnelled.isEmpty {
            lines.append("These use an SSH tunnel, which this app doesn't open: " + tunnelled.joined(separator: ", ") + ".")
        }
        if !skipped.isEmpty {
            lines.append("Skipped: " + skipped.map { "\($0.name) (\($0.reason))" }.joined(separator: "; ") + ".")
        }
        return lines.joined(separator: "\n\n")
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

    /// Flips "show all schemas" for a connection and re-lists its children so the change is visible
    /// immediately rather than after the next expansion. Only Postgres has anything to reveal, but
    /// the toggle is stored per-connection so it survives a relaunch like every other field.
    func toggleShowAllSchemas(_ id: UUID) {
        guard let index = connections.firstIndex(where: { $0.id == id }) else { return }
        var next = connections
        next[index].showAllSchemas.toggle()
        do {
            try ConnectionStore.save(next)
        } catch {
            notice = Notice(title: "Couldn't save the setting", message: error.localizedDescription)
            return
        }
        connections = next
        // Rebuild drops the cached children, so the connection re-lists — with the flag — the next
        // time it is expanded instead of showing the schemas it fetched under the old setting.
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
            // "Show all schemas" reveals pg_catalog, information_schema and any pg_* schema the
            // engine otherwise hides. Only Postgres filters, so the flag is set here alone.
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
        // `catalogs` means a catalog for Trino and a database for MySQL; the command is shared
        // because it is the same question ("what is directly under the connection?").
        let catalogNode = connection.kind == .mysql ? TreeNode.database : TreeNode.catalog
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
            case .connection, .table: break
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
              let root = tree.first(where: { $0.connectionID == connectionID }) else { return [] }
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

    /// Why Run is dim. Only a connection and a statement — Run does not write anything, so a
    /// destination it has not been given yet is none of its business.
    var runBlockedReason: String? {
        if connections.isEmpty { return "Add a connection first" }
        guard let tab = selectedTab else { return nil }
        if connection(for: tab) == nil { return "Choose a connection" }
        if !tab.hasSQL { return "Write a query" }
        return nil
    }

    /// Why Export is dim. Everything Run needs, plus somewhere to put the result.
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

    /// Navicat's split, and the reason this app is a query editor rather than a one-way pipe:
    /// **Run looks at the rows; Export writes them.** Run fetches the row limit and stops, so
    /// looking is cheap and reversible; Export streams the whole result to the destination.
    func runSelectedTab() {
        guard let tab = selectedTab else { return }
        preview(tab)
    }

    /// `source` decides what is sent: the selection, the statement under the caret, or everything.
    func preview(_ tab: QueryTab, from source: QuerySource = .selection) {
        guard !tab.previewing, tab.stage != .running else { return }
        guard let connection = connection(for: tab) else { return }
        let sql = tab.sql(for: source)
        let env: [String: String]
        do {
            env = try previewEnvironment(for: tab, connection: connection, sql: sql)
        } catch {
            tab.previewError = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            return
        }
        tab.previewing = true
        tab.previewError = nil
        tab.preview = nil
        tab.showingPlan = false
        tab.previewedSQL = sql
        // The filters and the last total described rows that are about to be replaced.
        tab.columnFilters = [:]
        tab.totalRows = nil
        tab.countError = nil
        tab.panel = .result
        panelCollapsed = false

        let run = UUID()
        tab.previewToken = run
        var message: String?
        var columns: [Event.Column] = []
        var rows: [[String?]] = []
        var truncated = false
        var finished = false
        tab.previewProcess = Engine.current.run("preview", env: env, onEvent: { event in
            guard tab.previewToken == run else { return }
            switch event.event {
            case "error":
                message = event.message
            case "columns":
                columns = event.columns ?? []
                // Paint the header as soon as it is known rather than after the first batch.
                tab.preview = PreviewResult(columns: columns, rows: [], truncated: false,
                                            queryID: nil, elapsedMS: 0)
            case "rows":
                rows.append(contentsOf: event.data ?? [])
                // A partial grid while the rest arrives: the point of batching.
                tab.preview = PreviewResult(columns: columns, rows: rows, truncated: false,
                                            queryID: tab.preview?.queryID, elapsedMS: 0)
            case "done":
                truncated = event.truncated ?? false
                finished = true
                tab.preview = PreviewResult(columns: columns, rows: rows, truncated: truncated,
                                            queryID: event.queryId, elapsedMS: event.elapsedMs ?? 0)
                tab.note(.success, "\(pluralized(rows.count, "row")) returned\(truncated ? " (limit reached)" : "")")
            default:
                break
            }
        }, onExit: { status, log in
            guard tab.previewToken == run else { return }
            tab.previewProcess = nil
            tab.previewing = false
            guard status == 0, finished else {
                tab.previewError = message ?? log.split(separator: "\n").last.map(String.init)
                    ?? "The engine exited with status \(status)."
                tab.preview = nil
                tab.note(.error, tab.previewError ?? "Preview failed")
                tab.panel = .log
                return
            }
        })
    }

    func cancelPreview(_ tab: QueryTab) {
        tab.previewProcess?.terminate()
    }

    /// Explains the query instead of running it, and shows the plan where the rows would go.
    ///
    /// EXPLAIN's spelling belongs to the driver, so the engine owns it; this sends the same
    /// statement a Run would and lets the reply land in the grid. Deliberately the same context
    /// (`database(for:)` / `schema(for:)`) and the same source resolution, because a plan for a
    /// different context than the one the query would run in is worse than no plan.
    func explain(_ tab: QueryTab, from source: QuerySource = .selection) {
        guard !tab.previewing, !tab.explaining, tab.stage != .running else { return }
        guard let connection = connection(for: tab) else { return }
        let sql = tab.sql(for: source)
        let env: [String: String]
        do {
            env = try previewEnvironment(for: tab, connection: connection, sql: sql)
        } catch {
            tab.previewError = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            return
        }
        tab.explaining = true
        tab.previewError = nil
        tab.preview = nil
        tab.previewedSQL = sql
        tab.showingPlan = true
        tab.panel = .result
        panelCollapsed = false

        let run = UUID()
        tab.previewToken = run
        var message: String?
        var columns: [Event.Column] = []
        var rows: [[String?]] = []
        var finished = false
        tab.previewProcess = Engine.current.run("explain", env: env, onEvent: { event in
            guard tab.previewToken == run else { return }
            switch event.event {
            case "error":
                message = event.message
            case "columns":
                columns = event.columns ?? []
                tab.preview = PreviewResult(columns: columns, rows: [], truncated: false,
                                            queryID: nil, elapsedMS: 0)
            case "rows":
                rows.append(contentsOf: event.data ?? [])
                tab.preview = PreviewResult(columns: columns, rows: rows, truncated: false,
                                            queryID: tab.preview?.queryID, elapsedMS: 0)
            case "done":
                finished = true
                tab.preview = PreviewResult(columns: columns, rows: rows, truncated: false,
                                            queryID: event.queryId, elapsedMS: event.elapsedMs ?? 0)
                tab.note(.success, "Plan returned \(pluralized(rows.count, "line"))")
            default:
                break
            }
        }, onExit: { status, log in
            guard tab.previewToken == run else { return }
            tab.previewProcess = nil
            tab.explaining = false
            guard status == 0, finished else {
                tab.previewError = message ?? log.split(separator: "\n").last.map(String.init)
                    ?? "The engine exited with status \(status)."
                tab.preview = nil
                tab.showingPlan = false
                tab.note(.error, tab.previewError ?? "Explain failed")
                tab.panel = .log
                return
            }
        })
    }

    /// DBeaver's "fetch row count": asks the server how many rows the statement on screen really
    /// returns. Deliberately a button and not something the preview does — it is a second query
    /// over the whole result, which can be slow and which the user should choose to pay for.
    func countRows(_ tab: QueryTab) {
        guard !tab.countingRows, let sql = tab.previewedSQL, !sql.isEmpty,
              let connection = connection(for: tab) else { return }
        let env: [String: String]
        do {
            var built = try connectionEnvironment(connection)
            built["SQL"] = sql
            built["RETRIES"] = String(tab.retries)
            env = built
        } catch {
            tab.countError = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            return
        }
        tab.countingRows = true
        tab.countError = nil
        let run = UUID()
        tab.countToken = run
        tab.countProcess = Engine.current.run("count", env: env, onEvent: { event in
            guard tab.countToken == run else { return }
            if event.event == "error" { tab.countError = event.message }
            if let count = event.count { tab.totalRows = count }
        }, onExit: { status, log in
            guard tab.countToken == run else { return }
            tab.countProcess = nil
            tab.countingRows = false
            if status != 0, tab.totalRows == nil {
                tab.countError = tab.countError
                    ?? log.split(separator: "\n").last.map(String.init)
                    ?? "The count failed."
            }
        })
    }

    /// The environment for a preview: the connection plus the statement and the row cap. Deliberately
    /// *not* the destination — looking at rows must not depend on having picked a file or a table.
    private func previewEnvironment(for tab: QueryTab, connection: Connection,
                                    sql: String) throws -> [String: String] {
        var env = try connectionEnvironment(connection)
        // The cascade overrides the connection's own database/schema. Trino resolves an
        // unqualified table against these, which is the whole point of picking them.
        env["DB_DATABASE"] = database(for: tab)
        env["DB_SCHEMA"] = schema(for: tab)
        env["SQL"] = sql
        env["LIMIT"] = String(max(1, tab.rowLimit))
        env["RETRIES"] = String(tab.retries)
        return env
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

    /// Writes the result out. Separate from `preview` so the toolbar can offer both without one
    /// standing in for the other.
    func run(_ tab: QueryTab, from source: QuerySource = .selection) {
        guard tab.stage != .running else { return }
        guard let connection = connection(for: tab) else { return }
        let env: [String: String]
        do {
            env = try overrides(for: tab, connection: connection, sql: tab.sql(for: source))
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
        tab.process = Engine.current.run(command, env: env, onEvent: { [weak self] event in
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
    private func overrides(for tab: QueryTab, connection: Connection,
                           sql: String) throws -> [String: String] {
        var env = try connectionEnvironment(connection)
        // Same context rule as a preview: an export runs where the cascade says it runs.
        env["DB_DATABASE"] = database(for: tab)
        env["DB_SCHEMA"] = schema(for: tab)
        env["SQL"] = sql
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
