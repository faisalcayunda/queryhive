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

    var body: some Scene {
        Window("QueryHive", id: "main") {
            RootView()
                .environment(model)
                // 1120 is where the widest toolbar (the table destination: connection, the
                // destination switch, catalog, schema, table name and mode) still fits without
                // clipping. Measured with --snapshot --scene table --width 1120.
                .frame(minWidth: 1120, minHeight: 700)
                .preferredColorScheme(.dark)
        }
        .windowStyle(.hiddenTitleBar)
        .windowResizability(.contentMinSize)
        .defaultSize(width: 1320, height: 880)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("New Query") { model.newTab() }
                    .keyboardShortcut("t", modifiers: .command)
                Button("Close Query") { model.closeSelectedTab() }
                    .keyboardShortcut("w", modifiers: .command)
            }
            CommandMenu("Query") {
                Button("Run") { model.runSelectedTab() }
                    .keyboardShortcut("r", modifiers: .command)
                    .disabled(model.selectedTab?.previewing == true)
                Button("Export") { model.selectedTab.map { model.run($0) } }
                    .keyboardShortcut("e", modifiers: .command)
                    .disabled(model.selectedTab.map { model.runBlockedReason(for: $0) != nil } ?? true)
                Button("Stop") { model.stopSelectedTab() }
                    .keyboardShortcut(".", modifiers: .command)
                    .disabled(model.selectedTab?.stage != .running)
                Divider()
                Button("Reveal Output in Finder") { model.selectedTab?.revealFiles() }
                    .disabled(model.selectedTab?.files.isEmpty != false)
            }
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
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
}

/// A user-facing error surfaced through RootView's alert. Used for problems that happen beside
/// the current work: a broken connections.json, a bad connection URL, a Keychain write that
/// failed on Save.
struct Notice: Identifiable {
    let id = UUID()
    let title: String
    let message: String
}
