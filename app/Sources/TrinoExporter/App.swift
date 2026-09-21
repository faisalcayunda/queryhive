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
    @State private var model = AppModel()
    /// Read here, at the top of the scene, so switching the mode in Settings re-evaluates this body
    /// and the whole window takes the new scheme. `AppearanceMode.system` hands back `nil`, which is
    /// what makes SwiftUI inherit macOS's appearance — and keep inheriting it live, so a system
    /// switch repaints the app without any observer of ours.
    @State private var appearance = ThemeStore.shared

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
        Settings {
            SettingsView()
                .environment(model)
                .preferredColorScheme(appearance.mode.colorScheme)
        }
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("New Query") { model.newTab() }
                    .keyboardShortcut(model.shortcut(for: .newQuery))
                Button("Close Query") { model.closeSelectedTab() }
                    .keyboardShortcut(model.shortcut(for: .closeTab))
            }
            // Kept off a key shortcut on purpose: importing writes to the Keychain and to the
            // connections file, so it should not be one keystroke away from an unrelated edit.
            CommandGroup(after: .newItem) {
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

    // A python child left running would keep writing after the window is gone.
    func applicationWillTerminate(_ notification: Notification) {
        Engine.running.forEach { $0.terminate() }
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
    /// The `count` command's answer. Its own event name, so it can never be confused with
    /// `done.rows`, which is the number a preview sent rather than the number that exists.
    var count: Int?
    var elapsedMs: Int?
    var host: String?
    var user: String?
    // `to_table` command: what was written, and where.
    var table: String?
    var mode: String?
    /// The coordinator's own state string, carried on `to_table` progress events.
    var state: String?
}

/// A user-facing error surfaced through RootView's alert. Used for problems that happen beside
/// the current work: a broken connections.json, a bad connection URL, a Keychain write that
/// failed on Save.
struct Notice: Identifiable {
    let id = UUID()
    let title: String
    let message: String
}
