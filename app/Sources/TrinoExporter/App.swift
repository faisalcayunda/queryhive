import AppKit
import SwiftUI

/// `--snapshot <path>` renders the shell to a PNG and exits instead of opening a window; see
/// `Snapshot`. Everything else falls through to the real app.
@main
enum QueryHiveMain {
    @MainActor
    static func main() {
        if let path = AppIconRenderer.requestedSheetPath() {
            AppIconRenderer.runSheet(path: path)
        }
        if let path = AppIconRenderer.requestedPath() {
            AppIconRenderer.run(path: path, compact: AppIconRenderer.requestedCompact())
        }
        if let path = Snapshot.requestedPath() {
            Snapshot.run(path: path, scene: Snapshot.requestedScene(),
                         width: Snapshot.requestedWidth() ?? 1240)
        }
        QueryHiveApp.main()
    }
}

struct QueryHiveApp: App {
    @NSApplicationDelegateAdaptor private var delegate: AppDelegate
    @State private var model = AppModel(persistsSession: true)
    /// Read here, at the top of the scene, so switching the mode in Settings re-evaluates this body
    /// and the whole window takes the new scheme. `AppearanceMode.system` hands back `nil`, which is
    /// what makes SwiftUI inherit macOS's appearance — and keep inheriting it live, so a system
    /// switch repaints the app without any observer of ours.
    @State private var appearance = ThemeStore.shared
    /// Owned by the scene, not by a view: the controller schedules its own checks from the moment
    /// it exists, so it must outlive any window that happens to be closed.
    @StateObject private var updater = Updater()

    var body: some Scene {
        Window("QueryHive", id: "main") {
            RootView()
                .environment(model)
                // 1120 is where the widest toolbar (the table destination: connection, the
                // destination switch, catalog, schema, table name and mode) still fits without
                // clipping. Measured with --snapshot --scene table --width 1120.
                .frame(minWidth: 1120, minHeight: 700)
                .preferredColorScheme(appearance.mode.colorScheme)
        }
        .windowStyle(.hiddenTitleBar)
        .windowResizability(.contentMinSize)
        .defaultSize(width: 1320, height: 880)

        // The standard app-menu entry, so ⌘, works without this app inventing its own key.
        //
        // The updater is handed in so Settings' General pane can run the same check the menu item
        // above does, and observe `canCheckForUpdates` while one is in flight. This scene owns the
        // only `Updater`; a second one would start a second Sparkle controller.
        Settings {
            SettingsView(updater: updater)
                .environment(model)
                .preferredColorScheme(appearance.mode.colorScheme)
        }
        .commands {
            // Where macOS users look for it, and where every other app puts it: directly under
            // About. Everything else about updating is Sparkle's — this is the only entry point
            // the app owns, and the disabled state is the only thing the app has to say about it.
            CommandGroup(after: .appInfo) {
                Button("Check for Updates…") { updater.checkForUpdates() }
                    .disabled(!updater.canCheckForUpdates)
            }
            CommandGroup(replacing: .newItem) {
                Button("New Query") { model.newTab() }
                    .keyboardShortcut(model.shortcut(for: .newQuery))
                Button("Close Query") { model.closeSelectedTab() }
                    .keyboardShortcut(model.shortcut(for: .closeTab))
            }
            // Kept off a key shortcut on purpose: importing writes to the Keychain and to the
            // connections file, so it should not be one keystroke away from an unrelated edit.
            CommandGroup(after: .newItem) {
                // Moved here from the editor's header row, which no longer exists. The shortcut
                // came with it: the button was the only thing holding ⌘O, and deleting the row
                // without re-homing it would have quietly removed the feature.
                Button("Load SQL File…") { model.selectedTab?.loadSQLFromFile() }
                    .keyboardShortcut(model.shortcut(for: .openFile))
                    .disabled(model.selectedTab == nil)
                Button("Import Data from File…") { model.presentImport() }
                    .disabled(model.connections.isEmpty)
                Button("Import Connections from Navicat…") { model.presentNavicatImport() }
            }
            // Every binding here comes from the current scheme, so switching scheme in Settings
            // moves the menu entries too. A `nil` leaves the item with no key at all rather than
            // silently keeping a stale one.
            CommandMenu("Query") {
                Button("Run") { model.runSelectedTab() }
                    .keyboardShortcut(model.shortcut(for: .run))
                    .disabled(model.selectedTab?.previewing == true)
                Button("Run Script") { model.selectedTab.map { model.run($0, from: .all) } }
                    .keyboardShortcut(model.shortcut(for: .runScript))
                    .disabled(model.selectedTab.map { model.runBlockedReason(for: $0) != nil } ?? true)
                Button("Explain") { model.selectedTab.map { model.explain($0) } }
                    .keyboardShortcut(model.shortcut(for: .explain))
                    .disabled(model.selectedTab.map { model.runBlockedReason(for: $0) != nil } ?? true)
                Button("Export") { model.selectedTab.map { model.run($0) } }
                    .keyboardShortcut(model.shortcut(for: .exportData))
                    .disabled(model.selectedTab.map { model.runBlockedReason(for: $0) != nil } ?? true)
                Button("Stop") { model.stopSelectedTab() }
                    .keyboardShortcut(model.shortcut(for: .stop))
                    .disabled(model.selectedTab?.stage != .running)
                Divider()
                Button("Reveal Output in Finder") { model.selectedTab?.revealFiles() }
                    .disabled(model.selectedTab?.files.isEmpty != false)
            }
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    /// KVO on `NSApplication.effectiveAppearance`, which is the documented way to hear about an
    /// appearance change: AppKit publishes no `didChangeEffectiveAppearance` notification, and the
    /// old `NSControlTintDidChangeNotification` is deprecated. This is what keeps `AppearanceMode`
    /// `.system` honest — without it the store's idea of "is the system dark" would be whatever it
    /// was at launch, and the theme would not follow a switch.
    private var appearanceObservation: NSKeyValueObservation?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let app = NSApplication.shared
        ThemeStore.shared.systemIsDark = app.effectiveAppearance.isDark
        appearanceObservation = app.observe(\.effectiveAppearance, options: [.new]) { _, change in
            guard let appearance = change.newValue else { return }
            MainActor.assumeIsolated { ThemeStore.shared.systemIsDark = appearance.isDark }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    // An engine child left running would keep writing after the window is gone.
    func applicationWillTerminate(_ notification: Notification) {
        Engine.current.terminateAll()
    }
}

/// One JSON line printed by queryhive_engine.py. Fields are filled per event type.
struct Event: Decodable {
    struct Column: Decodable {
        let name: String
        let type: String
    }

    struct ExportedFile: Decodable {
        let path: String
        let bytes: Int

        var url: URL { URL(fileURLWithPath: path) }
        var name: String { url.lastPathComponent }
    }

    let event: String
    var step: String?
    var message: String?
    /// `progress` / `done`: the rows the run has sent or wrote. `count`: the total the server
    /// reports for the statement. One key, read per event name — `tests/golden/count/count.ndjson`
    /// is the frozen `{"event": "count", "rows": 4321}`, and neither engine ever writes a `count`
    /// key on it, so a property named `count` would decode a shape that does not exist.
    var rows: Int?
    var columns: [Column]?
    var files: [ExportedFile]?
    var warnings: [String]?
    /// `query_id` on the wire; `convertFromSnakeCase` maps it here.
    var queryId: String?
    var cancelled: Bool?
    /// `test` command.
    var ok: Bool?
    /// `test` command: how many catalogs the coordinator listed. Deliberately not called
    /// `catalogs`, which is the *event name* of the browse command and carries a string array.
    var catalogCount: Int?
    /// `catalogs` / `schemas` / `tables` commands: the listed identifiers, in the coordinator's
    /// own order.
    var names: [String]?
    /// `objects` command: the grid's own headers, in the driver's order. Deliberately a plain
    /// string array under its own key -- `columns` above is `[{"name","type"}]` for the preview
    /// grid, and an object column has no type to give. Reusing that key does not decode.
    var objectColumns: [String]?
    // `preview` command. Deliberately `data`, not `rows`: `rows` is the integer on `progress` and
    // `done`, and one key cannot be two types.
    var data: [[String?]]?
    var truncated: Bool?
    var elapsedMs: Int?
    var host: String?
    var user: String?
    // `to_table` command: what was written, and where.
    var table: String?
    var mode: String?
    /// The coordinator's own state string, carried on `to_table` progress events.
    var state: String?

    /// `history` command: one row per execution, newest first.
    var entries: [HistoryEntry]?
    /// `saved_queries` command, `list` action: the stored statements in name order. Its own key
    /// rather than `entries`, because the two are different record shapes and one key cannot be
    /// two of them.
    var queries: [SavedQuery]?
    /// `history_clear`: how many rows were living when it ran, so a caller can say what it did.
    var cleared: Int?
    /// `apply_changes` reply: how many statements ran and committed. The plan is
    /// rolled back otherwise, so this number is the whole plan or none of it.
    var applied: Int?

    // `import_data`: the row-file family. `rows` above is the count written; these are the rest of
    // its report. `errors` is the bad-row list (line-prefixed), `stopped_at` the line the import
    // gave up on, `disposition`/`transaction` the transaction policy's own words, and `rejected`
    // the rows that carried no value into any mapped column.
    var rejected: Int?
    var errors: [String]?
    var errorsTruncated: Bool?
    var stoppedAt: Int?
    var transaction: Bool?
    var disposition: String?
    var streams: Bool?
    var format: String?
    /// `history_entry`: whether the write landed on a row that was already there. False means the
    /// id in the reply is the one the caller would have chosen.
    var merged: Bool?
    /// `saved_query` reply: which action the engine took.
    var action: String?
    /// `saved_query` reply: the row, for `get` and `save`. Null when a `get` found nothing, which
    /// is a normal answer and not an error.
    var query: SavedQuery?
    /// `saved_query` reply: whether a `rename` actually found a row.
    var renamed: Bool?

    /// `session` command: whether a stored session was found. `false` on a fresh install, which is
    /// a normal answer the app turns into one blank tab, not an error.
    var saved: Bool?
    /// `session` command: the tabs as the app last wrote them, handed back in the same shape.
    var tabs: [SessionTab]?
    /// `session` command: which tab was in front. Absent when none was selected, which is not the
    /// same as an empty string.
    var activeTabId: String?

    /// One execution in the engine's history.
    ///
    /// Three fields are optional because a row can be written before its run has finished: an
    /// entry with no `outcome` yet is not the same as one that ended, and the engine keeps that
    /// difference rather than filling in a zero.
    struct HistoryEntry: Decodable, Identifiable {
        let id: String
        var sql: String
        /// Unix milliseconds, read when the user asked for the run.
        var startedAt: Int
        var elapsedMs: Int?
        var rowCount: Int?
        /// `ok`, `error` or `cancelled`, or nothing yet.
        var outcome: String?
        var error: String?
        /// The connection's own identity, which is also the Keychain account. Absent is a real
        /// value: a run can be recorded before its connection has been saved.
        var connectionId: String?
        var deleted: Bool
        var version: Int
    }

    /// One statement the user chose to keep.
    struct SavedQuery: Decodable, Identifiable {
        let id: String
        var name: String
        var sql: String
        var connectionId: String?
        var folderId: String?
        /// Whether the user keeps this query within reach. Always on the wire, from migration 4
        /// onwards: the engine owns the default, so the app never has to guess what an absent field
        /// would mean.
        var favourite: Bool
        var deleted: Bool
        var version: Int
    }
}

/// A user-facing error surfaced through RootView's alert. Used for problems that happen beside
/// the current work: a broken connections.json, a bad connection URL, a Keychain write that
/// failed on Save.
struct Notice: Identifiable {
    let id = UUID()
    let title: String
    let message: String
}
