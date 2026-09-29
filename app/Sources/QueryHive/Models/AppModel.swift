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
    /// The folders connections can be filed under, in the order the user made them.
    ///
    /// Kept beside `connections` rather than inside it, because a group exists on its own: it can
    /// be empty, and it should still be there after a relaunch.
    var groups: [ConnectionGroup] = []

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

    // MARK: Filter presets

    /// Saved filter sets, keyed by connection + table (`FilterPresetStore.identity`). Loaded at
    /// launch and written back on every change, so a preset survives a relaunch. A hand-written
    /// query has no such identity; its presets live on the tab instead and are never written here.
    var filterPresets: [String: [FilterPreset]] = [:]

    /// The key this tab's presets are filed under, or `nil` for a query with no table — where a
    /// preset is tab-local for the reasons `FilterPresetStore` states.
    func presetIdentity(for tab: QueryTab) -> String? {
        FilterPresetStore.identity(connection: connection(for: tab)?.id, table: tab.sourceTable)
    }

    func filterPresets(for tab: QueryTab) -> [FilterPreset] {
        guard let identity = presetIdentity(for: tab) else { return tab.localPresets }
        return filterPresets[identity] ?? []
    }

    /// Save the tab's current filters under a name, replacing a preset of the same name. Returns
    /// `false` when there is nothing to save (no filters, or a blank name).
    @discardableResult
    func saveFilterPreset(named name: String, in tab: QueryTab) -> Bool {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let preset = tab.currentFilterPreset(named: trimmed) else {
            return false
        }
        if let identity = presetIdentity(for: tab) {
            var list = filterPresets[identity] ?? []
            list.removeAll { $0.name == trimmed }
            list.append(preset)
            filterPresets[identity] = list
            FilterPresetStore.save(filterPresets)
        } else {
            tab.localPresets.removeAll { $0.name == trimmed }
            tab.localPresets.append(preset)
        }
        return true
    }

    func deleteFilterPreset(named name: String, in tab: QueryTab) {
        if let identity = presetIdentity(for: tab) {
            var list = filterPresets[identity] ?? []
            list.removeAll { $0.name == name }
            filterPresets[identity] = list.isEmpty ? nil : list
            FilterPresetStore.save(filterPresets)
        } else {
            tab.localPresets.removeAll { $0.name == name }
        }
    }

    // MARK: Query tabs

    var tabs: [QueryTab] = []
    var selectedTabID: UUID?

    /// Whether this model reads and writes the session store.
    ///
    /// Off by default, and the app is the one caller that turns it on. A test that builds a model
    /// should write nothing to the user's session, and a test never relaunches; the app is the only
    /// place the feature has anything to do.
    let persistsSession: Bool

    /// Whether the session has been read yet.
    ///
    /// Saving is refused until it has. `init` makes a placeholder tab before the restore answers,
    /// and a save at that moment would write the placeholder over the very session the restore is
    /// about to read.
    private var sessionReady = false

    /// The termination hook that writes the session one last time.
    private var terminationObserver: NSObjectProtocol?

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

    /// The run waiting for the user to approve it, when the engine's `confirm` Safe Mode level
    /// demands an answer. Held on the model rather than in one view because every run path — Run,
    /// Export, Explain, Count and the tree's truncate/drop — goes through the same question.
    var pendingConfirmation: PendingConfirmation?

    /// The import the sheet is editing, or `nil` when no sheet is open.
    var importDraft: ImportDraft?

    private var tabCounter = 0

    init(persistsSession: Bool = false) {
        self.persistsSession = persistsSession
        let loaded = ConnectionStore.load()
        connections = loaded.document.connections
        groups = loaded.document.groups
        filterPresets = FilterPresetStore.load()
        notice = loaded.notice
        rebuildTree()
        newTab()
        if persistsSession {
            observeTermination()
            restoreSession()
        }
    }

    deinit {
        if let terminationObserver {
            NotificationCenter.default.removeObserver(terminationObserver)
        }
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

    /// The engine's history, newest first, as last read.
    ///
    /// Held here rather than on `QueryTab` because the history is the app's rather than one tab's:
    /// it spans connections and tabs, and the panel showing it shows the same list whichever tab
    /// is in front.
    /// What the History panel's search field holds.
    ///
    /// On the model rather than in the panel's own `@State` for one reason: a finished Run
    /// re-reads the list, and that re-read has to use the same search the user is looking at.
    var historySearch = ""

    /// Which history read is the current one.
    ///
    /// Two can be in flight: the search field starts one per pause in typing, and a finished Run
    /// starts one. Without this, whichever answered last wrote the list, so a superseded search's
    /// results could land on top of the newer ones. `previewToken` guards the same race.
    private var historyRead = 0

    var historyEntries: [Event.HistoryEntry] = []

    /// The saved queries, in name order, as last read.
    var savedQueries: [Event.SavedQuery] = []

    /// The saved queries the user keeps within reach, for the sidebar.
    ///
    /// Derived rather than kept as a second list: two lists can disagree after a save or a delete,
    /// and the only thing this one adds is a filter.
    var favouriteQueries: [Event.SavedQuery] {
        savedQueries.filter(\.favourite)
    }

    /// Keep a saved query within reach, or stop keeping it.
    func toggleFavourite(_ query: Event.SavedQuery) {
        let env = [
            "SAVED_ACTION": "favourite",
            "SAVED_ID": query.id,
            "FAVOURITE": query.favourite ? "0" : "1",
        ]
        _ = Engine.current.run("saved_queries", env: env, onEvent: { event in
            if event.event == "error" { self.savedError = event.message }
        }, onExit: { _, _ in
            // Re-read rather than flipping the flag in place: the engine owns the revision, and both
            // the sidebar and the panel should show what it stored, not what this hoped it stored.
            self.loadSavedQueries()
        })
    }

    /// Why the last read of either list failed.
    ///
    /// Shown inside the panel rather than in an alert: reading the history is never urgent enough
    /// to interrupt what the user is typing, and a panel that silently showed nothing would be
    /// indistinguishable from an empty history.
    /// The last failure on the history side, and on the saved-query side.
    ///
    /// Two fields rather than one shared `libraryError`, which is what this was: one field put a
    /// failed save into the History panel's banner, and let a successful history read clear an error
    /// the Saved panel was still showing. The two panels are separate, so their errors are too.
    var historyError: String?
    var savedError: String?

    func connection(for tab: QueryTab) -> Connection? {
        guard let id = tab.connectionID else { return nil }
        return connections.first { $0.id == id }
    }

    /// The display name of the connection a history row belongs to.
    ///
    /// `nil` for a row with no connection and for one whose connection has since been deleted. The
    /// panel draws nothing either way rather than a placeholder, because the row is still true.
    ///
    /// Matched as a UUID string rather than by comparing identities, because that is what the engine
    /// stores on the row: `recordHistory` writes `uuidString.lowercased()`.
    func connectionName(for historyID: String?) -> String? {
        guard let historyID, let id = UUID(uuidString: historyID) else { return nil }
        return connections.first { $0.id == id }?.name
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
              let root = connectionNode(for: connectionID)
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
        // A new tab has nothing to put in the panel: no rows, no log lines, no files. Opening it at
        // `panelHeight` — 480 points, more than half the window — spent the editor's room on a
        // message about the absence of content. It starts as its header alone, and every path that
        // produces something (Run, Explain, opening a table) already sets `panelCollapsed = false`.
        panelCollapsed = true
        saveSession()
    }

    /// Whether the panel has anything to show for this tab, which is what decides whether it is
    /// open by default.
    ///
    /// A run in progress counts: the panel is where its progress goes. A preview counts even when it
    /// holds zero rows — "0 rows" is an answer, and the grid's column header is how it is read.
    ///
    /// Internal rather than private because `Snapshot` seeds tabs directly and has to reach the same
    /// answer the app would: a scene that filled the grid by hand and then left the panel closed
    /// would photograph a state the app cannot produce.
    func panelHasContent(_ tab: QueryTab) -> Bool {
        tab.stage == .running || tab.preview != nil || !tab.logLines.isEmpty || !tab.files.isEmpty
    }

    /// Open or close the panel for the tab in front, by the rule above. Called wherever the tab in
    /// front changes: a new one, a selected one, and the one left behind by a close.
    ///
    /// `panelExpanded` is cleared here too. It lives on the model rather than the tab, so switching
    /// from a table opened full-window (`openTable` turns it on) to an empty tab would otherwise
    /// leave a full-window panel over "No result yet" — the same mistake as the default, one state
    /// further along.
    func syncPanelToSelectedTab() {
        let hasContent = selectedTab.map(panelHasContent) ?? false
        panelCollapsed = !hasContent
        if !hasContent { panelExpanded = false }
    }

    func closeTab(_ id: UUID) {
        guard let index = tabs.firstIndex(where: { $0.id == id }) else { return }
        tabs[index].process?.terminate()
        tabs.remove(at: index)
        if selectedTabID == id {
            selectedTabID = tabs.indices.contains(index) ? tabs[index].id : tabs.last?.id
            // Including the case where nothing is left: the empty workspace replaces the panel, so
            // leaving it open would be state that contradicts what is on screen.
            syncPanelToSelectedTab()
        }
        saveSession()
    }

    func closeSelectedTab() {
        guard let id = selectedTabID else { return }
        closeTab(id)
    }

    func selectTab(_ id: UUID) {
        selectedTabID = id
        // The panel follows the tab, because an empty tab has nothing to show and a tab with rows
        // should show them without a second click. Without this, switching from a tab with results
        // to an empty one left the panel open at 480 points over "No result yet".
        syncPanelToSelectedTab()
        if let connectionID = tabs.first(where: { $0.id == id })?.connectionID,
           let node = connectionNode(for: connectionID) {
            selectedNodeID = node.id
        }
        saveSession()
    }

    func rememberDestination(_ tab: QueryTab) {
        UserDefaults.standard.set(tab.outputDirectory?.path, forKey: "lastOutputDirectory")
        UserDefaults.standard.set(tab.format.rawValue, forKey: "lastFormat")
    }

    // MARK: Session

    /// Reads the session the last launch wrote, and replaces the placeholder tab with it.
    ///
    /// The placeholder is made first so the model always has a tab to show, and the restore takes
    /// over only a workspace nobody has used yet. If the load is slower than the user, or the
    /// session is empty, the placeholder stays — which is also what a first launch gets.
    ///
    /// `sessionReady` is set when this answers, whichever way it went: from then on the workspace
    /// is real and worth writing down.
    private func restoreSession() {
        var restored: [SessionTab]?
        var active: String?
        Engine.current.run("session", env: sessionEnvironment(["SESSION_ACTION": "load"]),
                           onEvent: { event in
            guard event.event == "session", event.saved == true else { return }
            restored = event.tabs
            active = event.activeTabId
        }, onExit: { [weak self] status, _ in
            guard let self else { return }
            self.sessionReady = true
            guard status == 0, let restored, !restored.isEmpty else { return }
            // Only a workspace nobody has used: one tab, no SQL, no run, not a listing. Otherwise
            // the user is already working and their tab is the one that wins.
            guard self.tabs.count == 1, let only = self.tabs.first,
                  !only.hasSQL, only.stage == .idle, only.preview == nil,
                  only.objectScope == nil else { return }
            self.tabs = restored.map { $0.tab() }
            self.tabCounter = max(self.tabCounter, self.tabs.count)
            if let active, let front = self.tabs.first(where: {
                $0.id.uuidString.caseInsensitiveCompare(active) == .orderedSame
            }) {
                self.selectedTabID = front.id
            } else {
                self.selectedTabID = self.tabs.first?.id
            }
            self.syncPanelToSelectedTab()
        })
    }

    /// Writes the open tabs to the session store.
    ///
    /// Fire and forget, like the history write: the reply is one line nobody reads, and a failed
    /// save has nowhere useful to be shown. Called when the set of tabs changes, when the tab in
    /// front changes, and once more at termination — the last one is what catches SQL typed since
    /// the last tab event.
    func saveSession() {
        guard let env = sessionSaveEnvironment() else { return }
        _ = Engine.current.run("session", env: env, onEvent: { _ in }, onExit: { _, _ in })
    }

    /// Writes the session on the calling thread, for termination.
    ///
    /// The same write as `saveSession`, through the engine's blocking path: at
    /// `applicationWillTerminate` the main queue is the one waiting, so a completion hopped to it
    /// would never run and the last edits would be lost.
    func saveSessionBlocking() {
        guard let env = sessionSaveEnvironment() else { return }
        Engine.current.runBlocking("session", env: env)
    }

    /// The settings a session save runs with, or `nil` when there is nothing to save yet.
    private func sessionSaveEnvironment() -> [String: String]? {
        guard persistsSession, sessionReady else { return nil }
        let snapshot = tabs.map(SessionTab.init)
        guard let data = try? JSONEncoder().encode(snapshot),
              let json = String(data: data, encoding: .utf8) else { return nil }
        var env = sessionEnvironment(["SESSION_ACTION": "save", "TABS_JSON": json])
        if let id = selectedTabID { env["ACTIVE_TAB_ID"] = id.uuidString }
        return env
    }

    /// The settings a session command runs with.
    ///
    /// `DB_PATH` is added only when something redirected the connections store — the test suite.
    /// The app leaves it out so the session lands in the engine's own default database, the same
    /// file the history and the saved queries live in; naming the file here would be a second place
    /// that has to agree with the engine about what it is called.
    private func sessionEnvironment(_ base: [String: String]) -> [String: String] {
        guard let root = ConnectionStore.root else { return base }
        var env = base
        env["DB_PATH"] = root.appendingPathComponent("queryhive.sqlite3").path
        return env
    }

    /// Writes the session once more on the way out, which is what catches SQL typed since the last
    /// tab event.
    private func observeTermination() {
        terminationObserver = NotificationCenter.default.addObserver(
            forName: NSApplication.willTerminateNotification, object: nil, queue: .main
        ) { [weak self] _ in
            self?.saveSessionBlocking()
        }
    }

    // MARK: Connections

    func presentConnectionEditor(_ connectionID: UUID?, startAtURL: Bool = false,
                                 previewTestCount: Int? = nil, inGroup: UUID? = nil) {
        editingConnection = ConnectionEditorTarget(connectionID, startAtURL: startAtURL,
                                                   previewTestCount: previewTestCount)
        // Filing is remembered here rather than passed through the sheet: the sheet's job is to
        // produce a connection, and which folder it belongs in is the tree's business, not a field
        // on the form. Cleared on every other way in, so editing an existing connection from the
        // header cannot silently move it.
        newConnectionGroup = inGroup
    }

    /// The group a connection created from the editor should be filed into. nil when the editor was
    /// opened from anywhere but a group's menu.
    var newConnectionGroup: UUID?

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

    // MARK: Groups

    /// A group name being typed: a new group, or a rename of an existing one.
    ///
    /// On the model rather than in a row, for the same reason as `pendingDeletion`: the sidebar
    /// menu, the connection's own menu and any future entry point all have to open the same prompt.
    struct GroupNaming: Identifiable {
        let id = UUID()
        /// nil while creating; the group being renamed otherwise.
        var groupID: UUID?
        var name: String
        /// The connection to file into the new group, when the prompt was opened from a
        /// connection's own menu. "New Group…" there means "put this one in a new group", not
        /// "make an empty folder somewhere".
        var fileConnectionID: UUID?
    }

    var groupNaming: GroupNaming?

    /// Prompt for a new group; `connectionID` files that connection into it once named.
    func presentNewGroup(with connectionID: UUID? = nil) {
        groupNaming = GroupNaming(groupID: nil, name: "New Group", fileConnectionID: connectionID)
    }

    func presentRenameGroup(_ groupID: UUID?) {
        guard let groupID, let group = groups.first(where: { $0.id == groupID }) else { return }
        groupNaming = GroupNaming(groupID: groupID, name: group.name, fileConnectionID: nil)
    }

    /// Commits the name being typed. A blank name is refused rather than saved: a group with no
    /// name is a row the user cannot identify, and it is one keystroke to avoid.
    func commitGroupNaming() {
        guard let naming = groupNaming else { return }
        groupNaming = nil
        let name = naming.name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { return }

        if let groupID = naming.groupID {
            guard let index = groups.firstIndex(where: { $0.id == groupID }) else { return }
            groups[index].name = name
        } else {
            let group = ConnectionGroup(name: name)
            groups.append(group)
            if let connectionID = naming.fileConnectionID,
               let index = connections.firstIndex(where: { $0.id == connectionID }) {
                connections[index].group = group.id
            }
        }
        persistConnections()
        rebuildTree()
    }

    /// Removes a group, keeping every connection that was in it.
    ///
    /// The connections move to the top level instead of being deleted. A folder that took its
    /// contents with it would be the most dangerous item in the sidebar, and nothing about the
    /// gesture says "and the servers as well".
    func deleteGroup(_ groupID: UUID?) {
        guard let groupID, let index = groups.firstIndex(where: { $0.id == groupID }) else { return }
        groups.remove(at: index)
        for position in connections.indices where connections[position].group == groupID {
            connections[position].group = nil
        }
        persistConnections()
        rebuildTree()
    }

    /// Files a connection under a group, or at the top level when `groupID` is nil.
    func move(_ connectionID: UUID, toGroup groupID: UUID?) {
        guard let index = connections.firstIndex(where: { $0.id == connectionID }) else { return }
        guard connections[index].group != groupID else { return }
        connections[index].group = groupID
        persistConnections()
        rebuildTree()
    }

    /// Writes connections.json. Failures become a notice rather than an error thrown into a menu
    /// action, which has nowhere to put one.
    func persistConnections() {
        do {
            try ConnectionStore.save(ConnectionsDocument(groups: groups, connections: connections))
        } catch {
            notice = Notice(title: "Couldn't save connections", message: error.localizedDescription)
        }
    }

    /// Removes a connection, its Keychain item, and its hold on any open tab. The JSON is written
    /// before the Keychain so a failure never leaves a saved connection whose password is gone.
    func deleteConnection(_ id: UUID) {
        var next = connections
        next.removeAll { $0.id == id }
        do {
            try ConnectionStore.save(ConnectionsDocument(groups: groups, connections: next))
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
            try ConnectionStore.save(ConnectionsDocument(groups: groups, connections: next))
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
            try ConnectionStore.save(ConnectionsDocument(groups: groups, connections: next))
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
            try ConnectionStore.save(ConnectionsDocument(groups: groups, connections: next))
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
        toggleBrowseFlag(id) { $0.showAllSchemas.toggle() }
    }

    /// Flips "show all databases" for a Postgres connection and re-lists its children.
    ///
    /// Same storage story as the schemas flag and the same reason: it is a property of the
    /// connection, so it belongs in `connections.json` beside the rest of them and has to survive
    /// the relaunch that follows. What it changes is the *shape* of the tree under the connection —
    /// a database level, or none — which is why the rebuild is the point rather than a side effect.
    func toggleShowAllDatabases(_ id: UUID) {
        toggleBrowseFlag(id) { $0.showAllDatabases.toggle() }
    }

    /// Saves one per-connection browse flag and rebuilds the tree around it.
    ///
    /// Both toggles are this: a flag that only ever changes what a *fetch* asks for, so nothing is
    /// re-run here — `rebuildTree` drops the cached children and the connection re-lists with the
    /// new flag the next time it is expanded, instead of showing children fetched under the old one.
    private func toggleBrowseFlag(_ id: UUID, _ flip: (inout Connection) -> Void) {
        guard let index = connections.firstIndex(where: { $0.id == id }) else { return }
        var next = connections
        flip(&next[index])
        do {
            try ConnectionStore.save(ConnectionsDocument(groups: groups, connections: next))
        } catch {
            notice = Notice(title: "Couldn't save the setting", message: error.localizedDescription)
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

    // MARK: Editor suggestions

    /// The one completion list on screen. Only the selected tab's editor is visible at a time, so
    /// this is app-level rather than per-tab state.
    var completion = EditorCompletion()

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

    /// How often a preview in flight hands its rows to the grid.
    ///
    /// Handing them over is what gives the buffer a second owner: from that moment the running
    /// preview and the grid share it, and copy-on-write makes the *next* batch copy every row
    /// fetched so far. A million-row preview paid that copy once per batch, and the grid paid a
    /// rebuild of the same size on the same schedule. Five paints a second bounds both, and the
    /// grid still fills in while the query runs. `done` paints the finished set either way, so the
    /// last batch is never the one that got away.
    static let previewPaintInterval: TimeInterval = 0.2

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
        // A Run of a write on a `confirm` connection asks first; the approval is merged into this
        // one run's environment and is never stored.
        if let request = RunConfirmation.request(for: statements(in: sql), command: "preview",
                                                 safeMode: connection.safeMode) {
            awaitConfirmation(request) { [weak self] in
                self?.runPreview(tab, sql: sql, connection: connection,
                                 env: env.merging(RunConfirmation.approvalSettings(true)) { _, new in new })
            }
            return
        }
        runPreview(tab, sql: sql, connection: connection, env: env)
    }

    /// The statements a script holds, split the way the engine splits them, so the confirmation
    /// names the same pieces the engine's own classifier reads.
    private func statements(in sql: String) -> [String] {
        sqlStatements(in: sql).map(\.text)
    }

    /// Hold a run until the user answers it, with the action that starts it on approval.
    private func awaitConfirmation(_ request: RunConfirmation.Request, run: @escaping () -> Void) {
        pendingConfirmation = PendingConfirmation(request: request, approve: run)
    }

    /// The statements a write run would execute, as far as the app can name them.
    ///
    /// An export streams the caller's statement; a table destination sends the statements the
    /// engine generates for the chosen write mode (`crates/qh-ffi/src/commands.rs`). The generated
    /// ones are spelled from the app's own qualified target, which is what the confirmation sheet
    /// shows; the engine quotes them itself when it runs, and both name the same table.
    private func destinationStatements(_ tab: QueryTab, connection: Connection, sql: String) -> [String] {
        guard tab.destination == .table else { return statements(in: sql) }
        let target = tab.target(for: connection.kind)
        let body = sql.trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: ";"))
        switch tab.writeMode {
        case .replace:
            return ["DROP TABLE IF EXISTS \(target)", "CREATE TABLE \(target) AS \(body)"]
        case .append:
            return ["INSERT INTO \(target) \(body)"]
        case .create:
            return ["CREATE TABLE \(target) AS \(body)"]
        }
    }

    // MARK: Table operations

    /// A table's context menu asks for truncate or drop: the engine's `table_op`, through the same
    /// confirmation the run paths use.
    ///
    /// Whether a question is asked is the engine's own contract. At `confirm` these two operations
    /// ask (ADR-0027's one exception to confirm refusing DDL); at `full` they run without asking;
    /// and at `no_ddl`/`read_only` the engine refuses them before opening a connection. The app
    /// does not pre-refuse those two levels — the engine's sentence is the answer, and duplicating
    /// the rule in Swift is how the two would come to disagree.
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

    // MARK: Importing data

    /// Opens the import sheet for a file the user picks, with the active tab's connection as the
    /// target's default.
    func presentImport() {
        presentImport(into: nil)
    }

    /// Opens the import sheet for a table node's context menu, so the target is already named.
    func presentImport(into node: TreeNode?) {
        guard !connections.isEmpty else {
            notice = Notice(title: "No connections",
                            message: "Add a connection before importing data.")
            return
        }
        let panel = NSOpenPanel()
        panel.title = "Import Data from File"
        panel.message = "Choose a CSV, TSV or XLSX file to read rows from."
        panel.prompt = "Choose"
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.allowedContentTypes = ["csv", "tsv", "xlsx"].compactMap { UTType(filenameExtension: $0) }
        guard panel.runModal() == .OK, let url = panel.url else { return }
        presentImport(from: url, into: node)
    }

    /// Builds the draft for one file and opens the sheet.
    private func presentImport(from url: URL, into node: TreeNode?) {
        guard let format = ImportSourceFormat.detect(path: url.path) else {
            notice = Notice(title: "Unsupported file",
                            message: "Import reads CSV, TSV or XLSX. \(url.lastPathComponent) is none of those.")
            return
        }
        let connectionID = node?.connectionID
            ?? selectedTab.flatMap { connection(for: $0)?.id }
            ?? connections.first?.id
        let connection = connections.first { $0.id == connectionID }
        var mapping = ImportMapping()
        mapping.path = url.path
        mapping.format = format
        mapping.delimiter = format.delimiter
        if let node, node.kind == .table {
            mapping.targetCatalog = node.database ?? ""
            mapping.targetSchema = node.schema ?? ""
            mapping.targetTable = node.title
        } else {
            mapping.targetCatalog = connection?.database ?? ""
            mapping.targetSchema = connection?.schema ?? ""
            mapping.targetTable = url.deletingPathExtension().lastPathComponent
        }
        importDraft = ImportDraft(mapping: mapping, connectionID: connectionID)
    }

    /// Points an open draft at another file, keeping the target the user already set.
    func configure(_ draft: ImportDraft, for url: URL) {
        guard let format = ImportSourceFormat.detect(path: url.path) else {
            notice = Notice(title: "Unsupported file",
                            message: "Import reads CSV, TSV or XLSX. \(url.lastPathComponent) is none of those.")
            return
        }
        draft.mapping.path = url.path
        draft.mapping.format = format
        draft.mapping.delimiter = format.delimiter
        // The old file's columns are gone; the target's are still the target's.
        draft.mapping.fields = []
    }

    /// Reads the target table's columns through the engine's `preview`, for the import mapping.
    ///
    /// `SELECT * FROM <qualified> LIMIT 1`, the same shape the object inspector uses: the engine
    /// describes the result set before the rows arrive, so the `columns` event answers the question
    /// without reading the table. A table that does not exist, or a driver that reports nothing
    /// before a row, calls back with `[]`, and the sheet maps by header name instead of pretending.
    func loadImportColumns(connectionID: UUID, qualifiedName: String,
                           completion: @escaping ([String]) -> Void) {
        guard let connection = connections.first(where: { $0.id == connectionID }) else {
            completion([])
            return
        }
        var env: [String: String]
        do {
            env = try connectionEnvironment(connection)
        } catch {
            completion([])
            return
        }
        env["RETRIES"] = "2"
        env["SQL"] = "SELECT * FROM \(qualifiedName)"
        env["LIMIT"] = "1"
        var columns: [String] = []
        _ = Engine.current.run("preview", env: env, onEvent: { event in
            if event.event == "columns" { columns = (event.columns ?? []).map(\.name) }
        }, onExit: { _, _ in completion(columns) })
    }

    /// Runs `import_data` for an import sheet and fills in what the engine answered.
    ///
    /// No confirmation setting is merged: an import is a bulk path, and the engine's own guard
    /// refuses it on a `confirm` connection rather than take one approval for every statement a
    /// file becomes (ADR-0026). The sheet states the refusal before this is reachable.
    func runImport(_ draft: ImportDraft) {
        guard let connectionID = draft.connectionID,
              let connection = connections.first(where: { $0.id == connectionID }) else {
            draft.failure = "Choose a connection first."
            return
        }
        var env: [String: String]
        do {
            env = try connectionEnvironment(connection)
        } catch {
            draft.failure = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            return
        }
        env.merge(draft.mapping.settings()) { _, new in new }
        draft.running = true
        draft.outcome = nil
        draft.failure = nil
        var outcome = ImportOutcome()
        var message: String?
        _ = Engine.current.run("import_data", env: env, onEvent: { event in
            switch event.event {
            case "done":
                outcome.rows = event.rows ?? 0
                outcome.rejected = event.rejected ?? 0
                outcome.errors = event.errors ?? []
                outcome.stoppedAt = event.stoppedAt
                outcome.mode = event.mode ?? ""
                outcome.disposition = event.disposition
                outcome.transaction = event.transaction ?? false
            case "error":
                message = event.message
            default:
                break
            }
        }, onExit: { [weak draft] status, log in
            guard let draft else { return }
            draft.running = false
            guard status == 0 else {
                draft.failure = message ?? log.split(separator: "\n").last.map(String.init)
                    ?? "The engine exited with status \(status)."
                return
            }
            draft.outcome = outcome
        })
    }


    /// Escalate the grid's in-memory search: run the same statement again with the term turned into
    /// a cross-column `WHERE`, so the server finds rows the grid never fetched.
    ///
    /// The run is the ordinary `preview` one, so the safety mode, the timeout, the retries and the
    /// streaming are the same as a Run; only the SQL differs. The statement is built by
    /// `SearchStatement`, which wraps the user's SQL inside a derived table rather than editing it,
    /// and is refused rather than guessed when the text holds more than one statement.
    func searchOnServer(_ tab: QueryTab, term: String) {
        guard !tab.previewing, tab.stage != .running else { return }
        guard let connection = connection(for: tab) else { return }
        guard let original = tab.previewBaseSQL ?? tab.previewedSQL, !original.isEmpty else {
            tab.note(.warning, "Run the query before searching the server.")
            tab.panel = .log
            return
        }
        let statement: String
        do {
            statement = try SearchStatement.crossColumn(sql: original, term: term,
                                                        columns: tab.preview?.columns ?? [],
                                                        kind: connection.kind)
        } catch {
            tab.note(.error, searchFailureMessage(error))
            tab.panel = .log
            return
        }
        let env: [String: String]
        do {
            env = try previewEnvironment(for: tab, connection: connection, sql: statement)
        } catch {
            tab.previewError = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            return
        }
        tab.note(.info, "Searching the server for “\(term)” across every column…")
        runPreview(tab, sql: statement, connection: connection, env: env, clearSearch: false,
                   baseSQL: original)
    }

    /// Why an escalation could not be built, in the words the user needs to fix it.
    private func searchFailureMessage(_ error: Error) -> String {
        guard let failure = error as? SearchStatement.Failure else {
            return "Cannot search on the server: \(error.localizedDescription)"
        }
        switch failure {
        case .blank:
            return "Nothing to search for."
        case .multipleStatements(let count):
            return "Searching the server needs one statement, and this result's text has \(count). "
                + "Run the one you mean, then search again."
        case .noColumns:
            return "Cannot search on the server: none of this result's columns can be read as text."
        }
    }

    /// Escalate the grid's in-memory sort: run the same statement again with an `ORDER BY`, so the
    /// server orders the whole result rather than the rows that were fetched.
    ///
    /// The run is the ordinary `preview` one, so the safety mode, the timeout, the retries and the
    /// streaming are the same as a Run; only the SQL differs. The statement is built by
    /// `ServerSort`, which wraps the user's SQL inside a derived table rather than editing it, and
    /// is refused rather than guessed when the text holds more than one statement.
    func sortOnServer(_ tab: QueryTab, column: Event.Column, direction: GridSort.Direction) {
        guard !tab.previewing, tab.stage != .running else { return }
        guard let connection = connection(for: tab) else { return }
        guard let original = tab.previewBaseSQL ?? tab.previewedSQL, !original.isEmpty else {
            tab.note(.warning, "Run the query before sorting it on the server.")
            tab.panel = .log
            return
        }
        let statement: String
        do {
            statement = try ServerSort.order(sql: original, column: column.name,
                                             direction: direction, kind: connection.kind)
        } catch {
            tab.note(.error, sortFailureMessage(error))
            tab.panel = .log
            return
        }
        let env: [String: String]
        do {
            env = try previewEnvironment(for: tab, connection: connection, sql: statement)
        } catch {
            tab.previewError = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            return
        }
        tab.note(.info, "Sorting on the server by \(column.name) "
                + "\(direction == .ascending ? "↑" : "↓") — the whole result, not only the rows fetched.")
        runPreview(tab, sql: statement, connection: connection, env: env, clearSearch: false,
                   baseSQL: original,
                   serverSort: ServerSortMark(column: column.name, direction: direction))
    }

    /// Why a server sort could not be built, in the words the user needs to fix it.
    private func sortFailureMessage(_ error: Error) -> String {
        guard let failure = error as? ServerSort.Failure else {
            return "Cannot sort on the server: \(error.localizedDescription)"
        }
        switch failure {
        case .noColumn:
            return "Cannot sort on the server: this column has no name to order by."
        case .multipleStatements(let count):
            return "Sorting on the server needs one statement, and this result's text has \(count). "
                + "Run the one you mean, then sort again."
        }
    }

    /// The body of a preview run, shared by Run and the search escalation: put the tab into its
    /// "a new result is arriving" state and start the engine.
    private func runPreview(_ tab: QueryTab, sql: String, connection: Connection,
                            env: [String: String], clearSearch: Bool = true,
                            baseSQL: String? = nil, serverSort: ServerSortMark? = nil) {
        tab.previewing = true
        tab.previewError = nil
        tab.preview = nil
        tab.showingPlan = false
        tab.previewedSQL = sql
        tab.previewBaseSQL = baseSQL ?? sql
        // Set on every run, so an ordinary Run clears the mark left by a server sort — the rows it
        // is about to replace are the ones the mark described.
        tab.serverSort = serverSort

        // The filters described rows that are about to be replaced, and clearing them is also what
        // drops the cell selection — see `columnFilters`' own note. The last total described them
        // too.
        tab.columnFilters = [:]
        // A fresh Run clears the cross-column search with the filters: neither describes the rows
        // that are arriving. The escalated search is the exception — it *is* the search — so its
        // caller keeps the term on screen.
        if clearSearch { tab.gridSearch = "" }
        tab.totalRows = nil
        tab.countError = nil
        tab.panel = .result
        panelCollapsed = false

        let run = UUID()
        tab.previewToken = run
        // When the user asked for it, which is what the history row's `started_at` records. Read
        // here rather than inside the engine because the engine only knows when it started work.
        let startedAt = Date()
        var message: String?
        var columns: [Event.Column] = []
        var rows: [[String?]] = []
        var truncated = false
        var finished = false
        // When the grid last got the rows, so `previewPaintInterval` is measured from a paint
        // rather than from the start of the run.
        var paintedAt = Date.distantPast
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
                // A partial grid while the rest arrives: the point of batching, on a clock rather
                // than once per batch. Every paint costs a copy of the whole buffer here and a
                // rebuild of the whole grid there, so a million rows painted per batch is the one
                // thing this loop cannot afford. See `previewPaintInterval`.
                let now = Date()
                if now.timeIntervalSince(paintedAt) >= Self.previewPaintInterval {
                    paintedAt = now
                    tab.preview = PreviewResult(columns: columns, rows: rows, truncated: false,
                                                queryID: tab.preview?.queryID, elapsedMS: 0)
                }
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
                // Read before `preview` is cleared on the next line. The elapsed time lives on the
                // result, so recording it after the clear wrote `null` for every failed and
                // cancelled run: a failure still knows how long it took to fail.
                let elapsed = tab.preview?.elapsedMS
                tab.previewError = message ?? log.split(separator: "\n").last.map(String.init)
                    ?? "The engine exited with status \(status)."
                tab.preview = nil
                tab.note(.error, tab.previewError ?? "Preview failed")
                tab.panel = .log
                Self.recordHistory(connection: connection, sql: sql, startedAt: startedAt,
                                   outcome: tab.cancelled ? "cancelled" : "error",
                                   elapsedMS: elapsed, rowCount: rows.count,
                                   error: tab.previewError, recording: self.recordsHistory)
                // Re-read, so a History panel that is already on screen counts this run without
                // the user having to switch panels. `onAppear` only fires once per appearance.
                self.loadHistory(search: self.historySearch)
                return
            }
            Self.recordHistory(connection: connection, sql: sql, startedAt: startedAt,
                               outcome: "ok", elapsedMS: tab.preview?.elapsedMS,
                               rowCount: rows.count, error: nil,
                               recording: self.recordsHistory)
            self.loadHistory(search: self.historySearch)
        })
    }

    /// Leaves one row in the engine's history for a Run that has finished.
    ///
    /// Run is `preview`, so this is where a Run becomes a fact: the statement, the connection,
    /// when the user asked for it, how long it took, how many rows came back, and how it ended.
    /// Only Run writes history. Explain does not, because a plan is not a result, and Export does
    /// not, because a history that mixed "I looked at this" with "I wrote this somewhere" would
    /// answer neither question.
    ///
    /// Fire and forget. The reply is one line nobody reads, and letting a failed history write
    /// reach the screen would turn a query that worked into a visible failure. `history_add`
    /// dedupes on connection, statement and start, so two finishes of one run stay one row.
    ///
    /// Static because it needs nothing from the model, and an instance method would put `self`
    /// inside a closure that outlives this call for no reason.
    private static func recordHistory(connection: Connection, sql: String, startedAt: Date,
                                      outcome: String, elapsedMS: Int?, rowCount: Int?,
                                      error: String?, recording: Bool) {
        // The switch in Settings reaches the history here and nowhere else, so there is one place to
        // look when a user asks why a run was not written down. Passed in rather than read from
        // `UserDefaults` here, because the default-inversion that makes it on-by-default is the
        // property's business and not this function's.
        guard recording else { return }
        // No `DB_PATH` and no `DB_*`: `history_add` is a local command that never opens a driver,
        // and without `DB_PATH` the engine writes to the same Application Support database the
        // app's connections already live in.
        var env: [String: String] = [
            "SQL": sql,
            "CONNECTION_ID": connection.id.uuidString.lowercased(),
            "STARTED_AT": String(Int(startedAt.timeIntervalSince1970 * 1000)),
            "OUTCOME": outcome,
        ]
        if let elapsedMS = elapsedMS { env["ELAPSED_MS"] = String(elapsedMS) }
        if let rowCount = rowCount { env["ROW_COUNT"] = String(rowCount) }
        if let error = error { env["ERROR_TEXT"] = error }
        _ = Engine.current.run("history_add", env: env, onEvent: { _ in }, onExit: { _, _ in })
    }

    /// How many history rows one read asks for, and whether a Run is recorded at all.
    ///
    /// Not in `ThemeStore` even though that is where the other preferences live: neither of these is
    /// appearance, and the engine call that records a run has to read the toggle without knowing a
    /// theme exists. The shape is `shortcutScheme`'s above, and for the same reason.
    ///
    /// A cap rather than everything: the panel is a list a person scrolls, and a year of runs is a
    /// list nobody scrolls to the end of. The engine reads newest first, so the cap costs the oldest
    /// entries rather than the recent ones.
    var historyLimit: Int = {
        let stored = UserDefaults.standard.integer(forKey: "historyLimit")
        return stored > 0 ? stored : 200
    }() {
        didSet { UserDefaults.standard.set(historyLimit, forKey: "historyLimit") }
    }

    /// Whether a Run leaves a row in the history.
    ///
    /// On by default, which is why this checks for the stored *object* rather than for the value:
    /// `bool(forKey:)` answers `false` for a key that was never written, so reading the value
    /// directly would make the first launch the one launch that records nothing.
    var recordsHistory: Bool = {
        UserDefaults.standard.object(forKey: "recordsHistory") as? Bool ?? true
    }() {
        didSet { UserDefaults.standard.set(recordsHistory, forKey: "recordsHistory") }
    }

    /// How long a statement may run before the server stops it, in milliseconds.
    ///
    /// The engine's `STATEMENT_TIMEOUT_MS`; `0` means no bound. The default is a minute, which
    /// is the one number here that is a judgement rather than a mechanism: a query that has run
    /// for a minute against a database this app talks to is usually a mistake, and the Stop
    /// button only stops the *reading*, not the work. Sixty seconds is long enough for an
    /// honest report and short enough that a forgotten query does not hold a warehouse slot all
    /// afternoon.
    ///
    /// `object(forKey:)` rather than `integer(forKey:)`: the latter answers 0 for a key that was
    /// never written, and 0 here is a real setting — no bound — not the absence of one.
    var statementTimeoutMS: Int = {
        UserDefaults.standard.object(forKey: "statementTimeoutMS") as? Int ?? 60_000
    }() {
        didSet { UserDefaults.standard.set(statementTimeoutMS, forKey: "statementTimeoutMS") }
    }

    /// Reads the engine's history into `historyEntries`, optionally narrowed to `search`.
    ///
    /// Fire and forget, like the write above, and it keeps the list it already had on a failure:
    /// a read that did not answer is not a reason to blank a panel the user is looking at. Called
    /// when the panel appears, when the search changes, and after a run finishes, which are the
    /// moments the list can have changed.
    ///
    /// The search runs in the engine rather than here. FTS5 is what makes "find the statements
    /// containing these words" answerable at all, and a second implementation over the rows in
    /// memory would be a second answer to the same question.
    func loadHistory(search: String = "") {
        historySearch = search
        historyRead += 1
        let read = historyRead
        var entries: [Event.HistoryEntry]?
        var failure: String?
        var env = ["HISTORY_LIMIT": String(historyLimit)]
        // Left out rather than sent empty when there is nothing to search for: the engine reads a
        // blank as "no search" as well, so this is only about not naming a key with no value.
        if !search.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            env["HISTORY_SEARCH"] = search
        }
        _ = Engine.current.run("history", env: env, onEvent: { event in
            switch event.event {
            case "history": entries = event.entries ?? []
            case "error": failure = event.message
            default: break
            }
        }, onExit: { status, log in
            // A superseded read says nothing, whichever way it went: its answer describes a search
            // the user has already moved on from, and its failure is not this read's failure.
            guard read == self.historyRead else { return }
            guard status == 0, let entries = entries else {
                self.historyError = failure ?? self.lastLine(of: log)
                    ?? "The engine exited with status \(status)."
                return
            }
            self.historyEntries = entries
            self.historyError = nil
        })
    }

    /// Reads the saved queries into `savedQueries`.
    func loadSavedQueries() {
        var queries: [Event.SavedQuery]?
        var failure: String?
        _ = Engine.current.run("saved_queries", env: ["SAVED_ACTION": "list"],
                               onEvent: { event in
            switch event.event {
            case "saved_queries": queries = event.queries ?? []
            case "error": failure = event.message
            default: break
            }
        }, onExit: { status, log in
            guard status == 0, let queries = queries else {
                self.savedError = failure ?? self.lastLine(of: log)
                    ?? "The engine exited with status \(status)."
                return
            }
            self.savedQueries = queries
            self.savedError = nil
        })
    }

    /// Saves the tab's statement under `name`, then re-reads the list.
    ///
    /// Answers `false` without running anything when the name or the statement is blank, so the
    /// sheet can refuse an empty name before a command is started. The engine refuses them too,
    /// because a caller that is not this app has no sheet to refuse it in.
    @discardableResult
    func saveQuery(_ tab: QueryTab, named name: String, from source: QuerySource = .all) -> Bool {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let sql = tab.sql(for: source)
        guard !trimmed.isEmpty,
              !sql.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return false }

        var env: [String: String] = ["SAVED_ACTION": "save", "NAME": trimmed, "SQL": sql]
        // The connection is what the query was written against, and it is worth keeping: the same
        // statement is a different statement against another database.
        if let id = tab.connectionID { env["CONNECTION_ID"] = id.uuidString.lowercased() }
        _ = Engine.current.run("saved_queries", env: env, onEvent: { event in
            if event.event == "error" { self.savedError = event.message }
        }, onExit: { _, _ in
            self.loadSavedQueries()
        })
        return true
    }

    /// Deletes a saved query, then re-reads the list.
    func deleteSavedQuery(_ id: String) {
        _ = Engine.current.run("saved_queries",
                               env: ["SAVED_ACTION": "delete", "SAVED_ID": id],
                               onEvent: { event in
            if event.event == "error" { self.savedError = event.message }
        }, onExit: { _, _ in
            self.loadSavedQueries()
        })
    }

    /// Puts a statement from the history or the saved list into the tab's editor.
    ///
    /// Replaces the editor's text rather than opening a tab: the list is a way back to something
    /// the user already wrote, and a second tab for the same statement is how a person ends up
    /// with five copies of one query. Undo still brings back what was there.
    func loadIntoEditor(_ sql: String, in tab: QueryTab) {
        tab.sql = sql
        tab.panel = .result
        panelCollapsed = false
    }

    /// The last non-empty line of the engine's log, which is where it puts a failure's reason.
    private func lastLine(of log: String) -> String? {
        log.split(separator: "\n").last.map(String.init)
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
    func explain(_ tab: QueryTab, from source: QuerySource = .selection, confirmed: Bool = false) {
        guard !tab.previewing, !tab.explaining, tab.stage != .running else { return }
        guard let connection = connection(for: tab) else { return }
        let sql = tab.sql(for: source)
        if !confirmed,
           let request = RunConfirmation.request(for: statements(in: sql), command: "explain",
                                                 safeMode: connection.safeMode) {
            awaitConfirmation(request) { [weak self] in
                self?.explain(tab, from: source, confirmed: true)
            }
            return
        }
        var env: [String: String]
        do {
            env = try previewEnvironment(for: tab, connection: connection, sql: sql)
            if confirmed { env.merge(RunConfirmation.approvalSettings(true)) { _, new in new } }
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
    func countRows(_ tab: QueryTab, confirmed: Bool = false) {
        guard !tab.countingRows, let sql = tab.previewedSQL, !sql.isEmpty,
              let connection = connection(for: tab) else { return }
        // The engine guards `count` against the caller's own statement, not the `COUNT(*)` wrapper,
        // so counting a write still needs the confirmation at `confirm`.
        if !confirmed,
           let request = RunConfirmation.request(for: statements(in: sql), command: "count",
                                                 safeMode: connection.safeMode) {
            awaitConfirmation(request) { [weak self] in self?.countRows(tab, confirmed: true) }
            return
        }
        let env: [String: String]
        do {
            var built = try connectionEnvironment(connection)
            built["SQL"] = sql
            built["RETRIES"] = String(tab.retries)
            if confirmed { built.merge(RunConfirmation.approvalSettings(true)) { _, new in new } }
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
            self.applyCountEvent(event, to: tab)
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

    /// What one event from a `count` run means for the tab.
    ///
    /// A method of its own so the mapping can be tested without an engine: `Engine.current` is a
    /// `static let`, so the run itself cannot be scripted. It is worth the seam, because this is
    /// exactly where the footer's total went missing — the number arrives in `rows` on the `count`
    /// event, and the reading here asked for `count`, a key neither engine writes, so `totalRows`
    /// stayed nil and the footer kept offering the button as if nothing had been asked.
    ///
    /// Gated on the event name rather than on the field alone: `rows` is also the integer on
    /// `progress` and `done`, so an ungated read would take a preview's row count for the total.
    func applyCountEvent(_ event: Event, to tab: QueryTab) {
        if event.event == "error" { tab.countError = event.message }
        if event.event == "count", let rows = event.rows { tab.totalRows = rows }
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
    func run(_ tab: QueryTab, from source: QuerySource = .selection, confirmed: Bool = false) {
        guard tab.stage != .running else { return }
        guard let connection = connection(for: tab) else { return }
        let sql = tab.sql(for: source)
        let command = tab.destination == .table ? "to_table" : "export"
        // A write run on a `confirm` connection asks first. A table destination's own statements
        // are the generated DROP/CREATE/INSERT rather than the caller's SELECT, so those are what
        // the engine guards — and what the sheet names.
        if !confirmed,
           let request = RunConfirmation.request(
                for: destinationStatements(tab, connection: connection, sql: sql),
                command: command,
                safeMode: connection.safeMode) {
            awaitConfirmation(request) { [weak self] in
                self?.run(tab, from: source, confirmed: true)
            }
            return
        }
        let env: [String: String]
        do {
            env = try overrides(for: tab, connection: connection, sql: sql, confirmed: confirmed)
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

    /// Run a reviewed plan of deletes, updates and inserts in one transaction.
    ///
    /// The plan is the same value the review sheet showed — `WritePlan` is built once and
    /// `payload` is the single encoder of its statements — so this cannot send a statement the
    /// user did not read. The engine verifies each statement's affected-row count and rolls the
    /// whole plan back if one disagrees.
    func applyChanges(_ plan: WritePlan, in tab: QueryTab) {
        // The plan says which rows it could not write, and it must not be a silent
        // partial save: those lines go to the log before anything runs.
        for warning in plan.warnings { tab.note(.warning, warning) }
        guard !plan.isEmpty, let connection = connection(for: tab) else {
            if !plan.warnings.isEmpty { tab.panel = .log }
            return
        }
        let env: [String: String]
        do {
            var built = try connectionEnvironment(connection)
            built["CHANGES"] = plan.payload
            built["IN_TRANSACTION"] = "true"
            env = built
        } catch {
            let message = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            tab.note(.error, message)
            tab.panel = .log
            return
        }
        tab.panel = .log
        tab.note(.info, "Applying \(pluralized(plan.statements.count, "change")) to "
                + "\(plan.table ?? "the table")…")
        var message: String?
        var applied = 0
        _ = Engine.current.run("apply_changes", env: env, onEvent: { event in
            switch event.event {
            case "progress":
                applied = event.rows ?? applied
            case "done":
                applied = event.applied ?? applied
            case "error":
                message = event.message
            default:
                break
            }
        }, onExit: { status, _ in
            if status == 0 {
                // The plan is in the table; the queue it came from has nothing
                // left to say.
                tab.cellEdits.discard()
                tab.note(.success, "Applied \(pluralized(applied, "change")).")
            } else {
                tab.note(.error, message ?? "The change plan failed and was rolled back.")
            }
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
                                      scheme: String, sslmode: String, verify: Bool,
                                      safeMode: String) -> [String: String] {
        let transport = TrinoTransport(stored: scheme)
        return [
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
            //
            // `prefer` is the one mode that needs both, and needs `sslmode` to be the one that
            // names it: a scheme can say "clear" or "encrypt", never "encrypt if you can". So
            // `DB_SCHEME=https` still goes out — it is where the mode starts, and the engine
            // reads it — and `DB_SSLMODE=prefer` decides, because a named sslmode outranks the
            // scheme in `qh_ffi::config`. The scheme word is sent unchanged rather than
            // normalised, so a value the engine would refuse is still refused out loud.
            "DB_SCHEME": kind == .trino ? (transport == .prefer ? "https" : (scheme.isEmpty ? "http" : scheme)) : "",
            // Only `prefer` is named for Trino. A stored Trino connection's `sslmode` is
            // whatever an earlier editor left there (it wrote Postgres's default into it), and
            // sending that for the other transports would outrank the scheme — turning a
            // connection the user set to https into a clear one, silently.
            "DB_SSLMODE": kind == .trino
                ? (transport == .prefer ? TrinoTransport.prefer.rawValue : "")
                : (sslmode.isEmpty ? kind.defaultSSLMode : sslmode),
            // `prefer` decides its own verification, and it never checks: `DB_INSECURE` would
            // otherwise demote it to a required, unverified connection, which is a different
            // mode and not the fallback the user asked for. Every other transport keeps the
            // stored flag, `http` included — the engine ignores it in clear.
            "DB_INSECURE": kind == .trino && transport == .prefer ? "" : (verify ? "" : "1"),
            // What the engine refuses, enforced there and not here: the CLI and the MCP
            // server run the same guard, and neither has this picker. Sent for every
            // driver, because the levels are about SQL and not about a server.
            "SAFE_MODE": safeMode,
        ]
    }

    static func connectionEnvironment(_ connection: Connection, password: String?) -> [String: String] {
        connectionEnvironment(kind: connection.kind, host: connection.host, port: connection.port,
                              user: connection.user, password: password,
                              database: connection.database, schema: connection.schema,
                              scheme: connection.scheme, sslmode: connection.sslmode,
                              verify: connection.verify, safeMode: connection.safeMode.rawValue)
    }

    /// The connection variables for a saved connection, Keychain read included. Shared by `run`
    /// and the target-option fetches so those cannot drift from what a run actually sends.
    ///
    /// The statement timeout is added here rather than at each run path: this is the one
    /// function every path that runs caller SQL goes through, and a bound that reached
    /// `preview` but not `count` would be a bound the user thinks they set.
    private func connectionEnvironment(_ connection: Connection) throws -> [String: String] {
        let password: String?
        do {
            password = try ConnectionKeychain.get(for: connection.id)
        } catch {
            throw EngineLaunchError(message: "Couldn't read the password for \(connection.name) from Keychain: \(error.localizedDescription)")
        }
        var env = Self.connectionEnvironment(connection, password: password)
        env["STATEMENT_TIMEOUT_MS"] = String(statementTimeoutMS)
        return env
    }

    /// Full environment for one export run: the connection plus the query, the destination and
    /// the per-format options. Throws rather than launching with a password the Keychain refused
    /// to hand over, which would silently export as the wrong identity.
    ///
    /// `confirmed` carries the user's approval of a `confirm`-level write into the run's own
    /// environment. It is never stored: the engine's whole model of a confirmation is that one run
    /// carries the flag.
    private func overrides(for tab: QueryTab, connection: Connection,
                           sql: String, confirmed: Bool = false) throws -> [String: String] {
        var env = try connectionEnvironment(connection)
        env.merge(RunConfirmation.approvalSettings(confirmed)) { _, new in new }
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

/// A run waiting for the user to approve it, and the action that starts it once they do.
///
/// The `approve` closure rebuilds and starts the run with `SAFE_MODE_CONFIRMED=1` merged in, so the
/// approval covers exactly that one run. A model-level value rather than per-tab state because the
/// question is about the engine's own level, which belongs to the connection rather than the tab.
struct PendingConfirmation: Identifiable {
    let id = UUID()
    let request: RunConfirmation.Request
    let approve: () -> Void
}
