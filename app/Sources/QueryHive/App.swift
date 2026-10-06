import AppKit
import SwiftUI

/// `--snapshot <path>` renders the shell to a PNG and exits instead of opening a window; see
/// `Snapshot`. Everything else falls through to the real app.
@main
enum QueryHiveMain {
    @MainActor
    static func main() {
        PerfSignposts.launchBegin()
        // A result that has spilled holds one file descriptor for as long as its tab lives (D-26).
        RustEngine.raiseFileLimit()
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
        // `--bench <scenario>` runs a measurement and exits; only `launch` comes back, to let the
        // real app start and report its own first frame.
        if CommandLine.arguments.contains("--bench") { BenchMode.run() }
        // Once, synchronously, before `AppModel` restores any tab (§17.6): the sweep of leftover
        // spill files is over before the first Run can make a store. `--snapshot` and `--bench`
        // configured their own budgets above, and the first caller wins.
        let spill = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first?
            .appendingPathComponent("QueryHive/spill").path
        RustEngine.ensureStoresConfigured(spillDir: spill, budgetBytes: 256 << 20)
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
                .onAppear { delegate.installTabKeys(for: model) }
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
            // Items and keys come from `AppMenu`, so the menu, the conflict test and Settings read
            // one list. A `nil` shortcut leaves the item with no key at all.
            CommandGroup(replacing: .newItem) { items(.file, limit: 2) }
            // Kept off a key shortcut on purpose: importing writes to the Keychain and to the
            // connections file, so it should not be one keystroke away from an unrelated edit.
            CommandGroup(after: .newItem) {
                items(.file, skip: 2)
                Button("Import Data from File…") { model.presentImport() }
                    .disabled(model.connections.isEmpty)
                Button("Import Connections from Navicat…") { model.presentNavicatImport() }
            }
            CommandMenu("Query") { items(.query) }
            CommandGroup(after: .sidebar) { items(.view) }
            CommandGroup(after: .windowArrangement) { items(.tab) }
        }
    }

    @ViewBuilder
    private func items(_ group: MenuGroup, limit: Int = .max, skip: Int = 0) -> some View {
        let specs = AppMenu.specs(for: model.shortcutScheme).filter { $0.group == group }
        ForEach(Array(specs.dropFirst(skip).prefix(limit)), id: \.id) { spec in
            Button(spec.title) { AppMenu.perform(spec, in: model) }
                .keyboardShortcut(spec.shortcut?.keyboard)
                .disabled(!AppMenu.isEnabled(spec, in: model))
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
        PerfSignposts.watchFirstFrame()
        ThemeStore.shared.systemIsDark = app.effectiveAppearance.isDark
        appearanceObservation = app.observe(\.effectiveAppearance, options: [.new]) { _, change in
            guard let appearance = change.newValue else { return }
            MainActor.assumeIsolated { ThemeStore.shared.systemIsDark = appearance.isDark }
        }
    }

    private var tabKeyMonitor: Any?

    /// ⌃Tab and ⌃⇧Tab, the aliases SwiftUI's one-key menu items cannot carry. Only when the main
    /// window is key with no sheet over it, so Settings and the sheets keep their own Tab.
    @MainActor
    func installTabKeys(for model: AppModel) {
        guard tabKeyMonitor == nil else { return }
        tabKeyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            let flags = event.modifierFlags
            guard let action = TabKeyRouter.route(keyCode: event.keyCode, control: flags.contains(.control),
                                                  shift: flags.contains(.shift), command: flags.contains(.command),
                                                  option: flags.contains(.option)),
                  let window = NSApp.keyWindow, window.title == "QueryHive", window.attachedSheet == nil
            else { return event }
            MainActor.assumeIsolated { model.selectTab(offset: action == .next ? 1 : -1) }
            return nil
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    /// Under `--bench` a quit request is refused and logged: a benchmark that dies silently
    /// because something asked the app to quit is a lost measurement, not a result.
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        PerfSignposts.recording ? BenchMode.refuseTermination() : .terminateNow
    }

    // An engine child left running would keep writing after the window is gone.
    func applicationWillTerminate(_ notification: Notification) {
        Engine.current.terminateAll()
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
