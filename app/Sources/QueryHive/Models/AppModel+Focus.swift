import AppKit
import SwiftUI

enum FocusRegion: String { case sidebar, editor, results }

/// Low-frequency navigation state, one stored property on `AppModel` (blueprint D-2).
struct NavigationState: Equatable {
    var sidebarHidden = false
}

extension AppModel {
    /// Moves the key window's first responder to a region, found by view type so the owners of
    /// the editor, grid and tree need not register. A region with nothing to focus says so.
    func focus(_ region: FocusRegion) {
        if region == .sidebar, navigation.sidebarHidden { navigation.sidebarHidden = false }
        let window = NSApplication.shared.keyWindow
        let root = window?.contentView
        let target: NSView? = root.flatMap {
            switch region {
            case .sidebar: Self.firstDescendant(NSOutlineView.self, in: $0)
            case .editor: Self.firstDescendant(SQLTextView.self, in: $0)
            case .results: Self.firstDescendant(GridTableView.self, in: $0)
            }
        }
        guard let target, window?.makeFirstResponder(target) == true else {
            Announcer.post(region == .results ? "No results to focus" : "Nothing to focus")
            return
        }
        Announcer.post("\(region.rawValue.capitalized) focused")
    }

    /// Which region holds the first responder, asked on demand rather than observed.
    var currentRegion: FocusRegion? {
        var view = NSApplication.shared.keyWindow?.firstResponder as? NSView
        while let current = view {
            if current is SQLTextView { return .editor }
            if current is GridTableView { return .results }
            if current is NSOutlineView { return .sidebar }
            view = current.superview
        }
        return nil
    }

    private static func firstDescendant<T: NSView>(_ type: T.Type, in view: NSView) -> T? {
        if let match = view as? T { return match }
        for sub in view.subviews { if let match = firstDescendant(type, in: sub) { return match } }
        return nil
    }

    func toggleSidebar() {
        navigation.sidebarHidden.toggle()
        Announcer.post(navigation.sidebarHidden ? "Sidebar hidden" : "Sidebar shown")
    }

    func toggleResultPanel() {
        panelCollapsed.toggle()
        Announcer.post(panelCollapsed ? "Results hidden" : "Results shown")
    }

    /// Wraps around, so the last tab's next is the first.
    func selectTab(offset: Int) {
        guard tabs.count > 1, let index = tabs.firstIndex(where: { $0.id == selectedTabID }) else { return }
        let tab = tabs[(index + offset + tabs.count) % tabs.count]
        selectTab(tab.id)
        Announcer.post(tab.title)
    }

    /// 1-based, ⌘1…⌘9; anything past the last tab (⌘9 included) is the last tab.
    func selectTab(position: Int) {
        guard let last = tabs.indices.last, position >= 1 else { return }
        let tab = tabs[position >= 9 ? last : min(position - 1, last)]
        selectTab(tab.id)
        Announcer.post(tab.title)
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
        // The starting row limit, which the grid's own field can then change for this tab alone.
        tab.rowLimit = defaultRowLimit
        tabs.append(tab)
        selectedTabID = tab.id
        // A new tab has nothing to put in the panel: no rows, no log lines, no files. Opening it at
        // `panelHeight` — 480 points, more than half the window — spent the editor's room on a
        // message about the absence of content. It starts as its header alone, and every path that
        // produces something (Run, Explain, opening a table) already sets `panelCollapsed = false`.
        panelCollapsed = true
        saveSession()
        warmUp(for: tab)
    }

    /// Asks the engine to have a database session ready for this tab's connection.
    ///
    /// Sends what a run would send (the same environment, and the tab's own database and schema, so
    /// the engine's connection key matches the run's), and nothing else: no query, no prefetch. The
    /// Keychain is read on a utility queue and never prompts, so selecting a tab neither blocks the
    /// main thread nor raises a dialog; a password that cannot be read that way just means no warm
    /// session, and the run reports whatever is wrong itself.
    private func warmUp(for tab: QueryTab?) {
        guard let tab, let connection = connection(for: tab) else { return }
        let database = database(for: tab)
        let schema = schema(for: tab)
        let timeout = statementTimeoutMS
        DispatchQueue.global(qos: .utility).async {
            // No saved password is a normal answer (an empty one is what a run sends); a Keychain
            // that refuses without a prompt is not, and skips the warm-up.
            let password: String?
            do {
                password = try Self.benchPassword ?? ConnectionKeychain.getWithoutPrompt(for: connection.id)
            } catch {
                return
            }
            var env = Self.connectionEnvironment(connection, password: password)
            env["STATEMENT_TIMEOUT_MS"] = String(timeout)
            env["DB_DATABASE"] = database
            env["DB_SCHEMA"] = schema
            Engine.current.warmUp(env: env)
        }
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
        // Both runs, then the stores (§19): `preview` and `explain` live in `previewProcess`, and a
        // query still waiting on the server would otherwise run on for a tab nobody can see.
        tabs[index].process?.terminate()
        tabs[index].previewProcess?.terminate()
        tabs[index].previewProcess = nil
        searchTasks[id]?.cancel()
        searchTasks[id] = nil
        tabs[index].releaseResults()
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

    /// Close every tab but this one.
    ///
    /// The named tab stays selected, so the one the user was looking at does not move under the
    /// pointer. Every tab goes through `closeTab`, which terminates a running tab's process — the
    /// same thing the single-tab close does, and the reason these do not edit the array themselves.
    func closeOtherTabs(keeping id: UUID) {
        for other in tabs where other.id != id {
            closeTab(other.id)
        }
        selectTab(id)
    }

    /// Close every tab to the right of this one, "right" being the strip's order.
    func closeTabs(after id: UUID) {
        guard let index = tabs.firstIndex(where: { $0.id == id }) else { return }
        for other in tabs.suffix(from: index + 1) {
            closeTab(other.id)
        }
    }

    /// Close every tab, leaving the workspace empty.
    ///
    /// `tabCounter` is deliberately not reset: `newTab` names from it, and going back to "Query 1"
    /// would give two tabs the same name within one session.
    func closeAllTabs() {
        for tab in tabs {
            closeTab(tab.id)
        }
    }

    func selectTab(_ id: UUID) {
        selectedTabID = id
        // A tab in the background keeps its store, which spills first when the budget asks, but not
        // the pages Swift cached from it (NFR-P3).
        for other in tabs where other.id != id {
            other.activeResult?.dropPages()
            other.baseResult?.rows.dropPages()
        }
        // The panel follows the tab, because an empty tab has nothing to show and a tab with rows
        // should show them without a second click. Without this, switching from a tab with results
        // to an empty one left the panel open at 480 points over "No result yet".
        syncPanelToSelectedTab()
        if let connectionID = tabs.first(where: { $0.id == id })?.connectionID,
           let node = connectionNode(for: connectionID) {
            selectedNodeID = node.id
        }
        saveSession()
        warmUp(for: selectedTab)
    }

    /// The results Open Quickly is showing, best first.
    var quickResults: [QuickResult] {
        QuickSearch.results(query: openQuicklyQuery, nodes: allNodes(),
                            savedQueries: savedQueries, history: historyEntries)
    }

    func openQuickly() {
        openQuicklyQuery = ""
        openQuicklyIndex = 0
        openQuicklyOpen = true
    }

    /// Move the highlight by `delta`, staying inside the list.
    func moveQuickHighlight(by delta: Int) {
        let count = quickResults.count
        guard count > 0 else { return }
        openQuicklyIndex = min(max(openQuicklyIndex + delta, 0), count - 1)
    }

    /// Do what a result says, and close the palette.
    func applyQuickResult(_ result: QuickResult) {
        switch result.action {
        case .revealNode(let id):
            revealNode(id)
        case .loadSQL(let sql):
            if let tab = selectedTab { loadIntoEditor(sql, in: tab) }
        }
        openQuicklyOpen = false
    }
}
