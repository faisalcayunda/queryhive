import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// App-level state: the saved connections, the object tree built from them, and the open query
/// tabs. One model keeps the wiring simple — every view reads the single environment object —
/// while each tab owns its own SQL, destination and run state, because Navicat lets several
/// queries be in flight at once.
@Observable
final class AppModel {
    /// Hands the model whatever the editors have typed but not yet written to `tab.sql`. The editor
    /// writes the model on a pause in typing, so everything that reads the text — a run, an explain,
    /// a save, the session — calls this first. The editor installs it; the model does not know about
    /// view code.
    @MainActor static var flushEditors: @MainActor () -> Void = {}

    /// Called at the top of everything that reads a tab's SQL. The model is not main-actor isolated,
    /// so this asserts what those callers already are: on the main thread. Off it, this traps rather
    /// than reading text the editor has not written.
    static func flushEditorsNow() {
        MainActor.assumeIsolated { flushEditors() }
    }

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

    /// Open Quickly: whether it is showing, what is typed into it, and which result is highlighted.
    ///
    /// On the model rather than in the view because the menu command that opens it lives in the
    /// menu bar, outside the view that draws the palette.
    var openQuicklyOpen = false

    var openQuicklyQuery = ""

    var openQuicklyIndex = 0

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
    var sessionReady = false

    /// The termination hook that writes the session one last time.
    var terminationObserver: NSObjectProtocol?

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

    var tabCounter = 0

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
            // The termination observer is registered whichever way the startup setting goes, and
            // the session is written either way: turning the setting on later has to bring back the
            // workspace you had, not the one from whenever the setting was last on.
            observeTermination()
            if restoreTabsOnLaunch { restoreSession() }
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
    var historyRead = 0

    var historyEntries: [Event.HistoryEntry] = []

    /// The saved queries, in name order, as last read.
    var savedQueries: [Event.SavedQuery] = []

    /// The application's own identity, and the profiles it owns.
    ///
    /// Read from the engine's `account` and `profiles` commands. The account exists from the first
    /// launch, so a profile saved before anyone signs in still has an owner.
    var account: Event.Account?

    var profiles: [Event.Profile] = []

    /// What the Account pane says when a sign-in fails, or when it cannot start for want of a
    /// client id. Shown in the pane rather than as an alert: nothing else is blocked by it.
    var accountNotice: String?

    /// Whether a sign-in is in flight, so the button can say so rather than looking inert.
    var signingIn = false

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

    // MARK: Tabs


    /// The values a `:name` statement is waiting for, while the prompt is up.
    var parameterPrompt: ParameterPrompt?

    /// Whether the workspace comes back the way it was left.
    ///
    /// On by default, which is what this app has always done. Off, a launch starts at one empty tab
    /// and the last session is not read.
    var restoreTabsOnLaunch: Bool = AppModel.storedRestoreTabsOnLaunch() {
        didSet { UserDefaults.standard.set(restoreTabsOnLaunch, forKey: AppModel.restoreTabsKey) }
    }

    // MARK: Connections


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

    // MARK: Groups


    var groupNaming: GroupNaming?

    // MARK: Target options, fetched on demand


    /// Catalogs (MySQL: databases) fetched for a connection, keyed by connection id.
    var catalogOptions: [UUID: [String]] = [:]

    /// Schemas fetched for one catalog, keyed by "connection|catalog". Postgres has no catalog
    /// level, so its key ends in an empty catalog.
    var schemaOptions: [String: [String]] = [:]

    /// Keys with a fetch in flight, so a field can say it is working instead of looking empty.
    var loadingOptions: Set<String> = []

    // MARK: Editor suggestions


    /// The one completion list on screen. Only the selected tab's editor is visible at a time, so
    /// this is app-level rather than per-tab state.
    var completion = EditorCompletion()

    // MARK: Importing data


    /// One pending debounced search per tab: a keystroke cancels the one before it.
    var searchTasks: [UUID: DispatchWorkItem] = [:]

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

    /// The engine Run and History talk to. A property so a test can hand the model a scripted one;
    /// everything else still reads `Engine.current`.
    var engine: any DatabaseEngine = Engine.current

    /// How many rows a Run fetches by default, for a tab that has not chosen its own.
    ///
    /// The engine's `LIMIT`. It is a property of looking rather than of the query, which is why it
    /// lives on the tab and why this is only the starting value: the grid's own field changes one
    /// tab and leaves the others alone, and a restored session keeps whatever each tab was left
    /// with. `object(forKey:)` rather than `integer(forKey:)`, so a stored zero is distinguishable
    /// from a key nobody has written.
    var defaultRowLimit: Int = {
        AppModel.clampedRowLimit(UserDefaults.standard.object(forKey: "defaultRowLimit") as? Int ?? 1000)
    }() {
        didSet {
            let asked = defaultRowLimit
            let clamped = Self.clampedRowLimit(asked)
            if clamped != asked {
                // Re-enters this observer (the property is macro-rewritten), so the note is set
                // after, or the in-range pass would clear it.
                defaultRowLimit = clamped
                // Said, not silent: a typed 5,000,000 that quietly became 200,000 would read as a bug.
                rowLimitClampNote = Self.rowLimitClampMessage(asked)
                return
            }
            rowLimitClampNote = nil
            UserDefaults.standard.set(defaultRowLimit, forKey: "defaultRowLimit")
        }
    }

    /// Set when the last edit to `defaultRowLimit` was out of range; Settings shows it under the field.
    var rowLimitClampNote: String? = {
        // The stored value was above the ceiling when the app started: say so, or Settings shows a
        // number the user never typed with no reason.
        guard let stored = UserDefaults.standard.object(forKey: "defaultRowLimit") as? Int,
              AppModel.clampedRowLimit(stored) != stored else { return nil }
        return AppModel.rowLimitClampMessage(stored)
    }()
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

