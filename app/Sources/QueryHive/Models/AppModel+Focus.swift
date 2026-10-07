import AppKit
import SwiftUI

enum FocusRegion: String { case sidebar, editor, results }

/// What closing a tab or quitting the app would throw away (DBX-26): staged grid changes, which exist
/// nowhere else, and work that is still running and would be stopped. Pure data, so the sentences
/// are tested without an alert.
struct UnsavedWork: Equatable {
    var stagedChanges = 0
    var tabsWithChanges = 0
    /// Changes being applied to the database right now.
    var applies = 0
    /// Exports and saves-to-table still running.
    var exports = 0
    var imports = 0

    var isEmpty: Bool { stagedChanges == 0 && applies == 0 && exports == 0 && imports == 0 }

    /// One sentence per kind of loss, in the order they matter.
    var lines: [String] {
        var lines: [String] = []
        if stagedChanges > 0 {
            lines.append("\(pluralized(stagedChanges, "staged change")) in "
                         + "\(pluralized(tabsWithChanges, "tab")) will be discarded.")
        }
        if applies > 0 {
            lines.append("Changes are being applied to the database. Stopping now can leave the "
                         + "transaction unfinished; the server rolls it back.")
        }
        if exports > 0 { lines.append("\(pluralized(exports, "export")) still running will be stopped.") }
        if imports > 0 { lines.append("An import is still running and will be stopped.") }
        return lines
    }
}

/// What the confirmation is for, which only changes its words.
enum DiscardIntent: Equatable {
    case quit
    case closeTabs(Int)

    var question: String {
        switch self {
        case .quit: "Quit QueryHive?"
        case .closeTabs(let count): count == 1 ? "Close this query?" : "Close \(count) queries?"
        }
    }

    var confirmTitle: String {
        switch self {
        case .quit: "Quit"
        case .closeTabs: "Close"
        }
    }
}

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

    // MARK: Unsaved work (DBX-26)

    /// What is at stake in `tabs`: staged changes, an apply, an export. An import is the app's and is
    /// counted only by `unsavedWork` below, since it does not die with a tab.
    func unsavedWork(in tabs: [QueryTab]) -> UnsavedWork {
        var work = UnsavedWork()
        for tab in tabs {
            if !tab.cellEdits.isEmpty {
                work.stagedChanges += tab.cellEdits.count
                work.tabsWithChanges += 1
            }
            if tab.applying { work.applies += 1 }
            if tab.stage == .running { work.exports += 1 }
        }
        return work
    }

    /// Everything the app would lose by quitting.
    var unsavedWork: UnsavedWork {
        var work = unsavedWork(in: tabs)
        if importDraft?.running == true { work.imports = 1 }
        return work
    }

    /// Asks, and answers whether to go on. A seam: the app shows an alert, a test answers for it.
    static var confirmDiscard: (UnsavedWork, DiscardIntent) -> Bool = { work, intent in
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = intent.question
        alert.informativeText = work.lines.joined(separator: "\n")
        // Cancel first, so Return keeps the work and the destructive answer is a deliberate click.
        alert.addButton(withTitle: "Cancel")
        let confirm = alert.addButton(withTitle: intent.confirmTitle)
        confirm.hasDestructiveAction = true
        return alert.runModal() == .alertSecondButtonReturn
    }

    /// One question for a set of tabs about to close, and `true` when there is nothing to ask or the
    /// answer is to go on. Asked once however many tabs there are, so "Close Other Tabs" is one
    /// dialog and not one per tab.
    private func confirmClosing(_ doomed: [QueryTab]) -> Bool {
        let work = unsavedWork(in: doomed)
        return work.isEmpty || Self.confirmDiscard(work, .closeTabs(doomed.count))
    }

    /// `true` when the app may quit now: nothing is at stake, or the person said to go on. The one
    /// place ⌘Q, closing the last window and Sparkle's relaunch (which ends in `NSApp.terminate`)
    /// all ask.
    func confirmQuit() -> Bool {
        let work = unsavedWork
        return work.isEmpty || Self.confirmDiscard(work, .quit)
    }

    /// Close one tab, after asking when it holds staged changes or a running export. `confirmed` is
    /// for a caller that has already asked about a set of tabs. Answers whether the tab was closed.
    @discardableResult
    func closeTab(_ id: UUID, confirmed: Bool = false) -> Bool {
        guard let index = tabs.firstIndex(where: { $0.id == id }) else { return false }
        if !confirmed, !confirmClosing([tabs[index]]) { return false }
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
        return true
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
        let others = tabs.filter { $0.id != id }
        guard confirmClosing(others) else { return }
        for other in others { closeTab(other.id, confirmed: true) }
        selectTab(id)
    }

    /// Close every tab to the right of this one, "right" being the strip's order.
    func closeTabs(after id: UUID) {
        guard let index = tabs.firstIndex(where: { $0.id == id }) else { return }
        let after = Array(tabs.suffix(from: index + 1))
        guard confirmClosing(after) else { return }
        for other in after { closeTab(other.id, confirmed: true) }
    }

    /// Close every tab, leaving the workspace empty.
    ///
    /// `tabCounter` is deliberately not reset: `newTab` names from it, and going back to "Query 1"
    /// would give two tabs the same name within one session.
    func closeAllTabs() {
        let all = tabs
        guard confirmClosing(all) else { return }
        for tab in all { closeTab(tab.id, confirmed: true) }
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
