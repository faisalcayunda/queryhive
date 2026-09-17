import AppKit
import SwiftUI

/// Renders the shell to a PNG:
///
///     QueryHive.app/Contents/MacOS/QueryHive --snapshot /tmp/queryhive.png
///
/// Exists because the design is the point of this app and there is otherwise no way to review a
/// change to it without a human sitting in front of the window.
///
/// It captures the app's **own window** with `cacheDisplay(in:to:)` rather than using
/// `ImageRenderer`. That matters: `ImageRenderer` cannot rasterize `TextEditor`, `TextField` or
/// `ScrollView` content and substitutes a "prohibited" placeholder, and `.ultraThinMaterial` has
/// no window to sample behind it, so an ImageRenderer snapshot shows empty panels and flat
/// glass. Drawing the real window into a bitmap has none of those problems and still needs no
/// screen-recording permission, because a process may always draw its own views.
///
/// The scene is seeded from a fixed fixture rather than the real model, so two runs of the same
/// build produce the same image and the connections file and Keychain are never touched.
enum Snapshot {
    static func requestedPath() -> String? {
        let arguments = CommandLine.arguments
        guard let index = arguments.firstIndex(of: "--snapshot"), index + 1 < arguments.count else { return nil }
        return arguments[index + 1]
    }

    /// `--width <pt>`: render at a specific window width. Used to check the toolbar at the
    /// window's minimum, which is where a row of controls actually breaks.
    static func requestedWidth() -> CGFloat? {
        let arguments = CommandLine.arguments
        guard let index = arguments.firstIndex(of: "--width"), index + 1 < arguments.count,
              let value = Double(arguments[index + 1]) else { return nil }
        return CGFloat(value)
    }

    /// `--scene <name>`: which fixture to draw. Default `done`.
    static func requestedScene() -> String {
        let arguments = CommandLine.arguments
        guard let index = arguments.firstIndex(of: "--scene"), index + 1 < arguments.count else { return "done" }
        return arguments[index + 1]
    }

    @MainActor
    static func run(path: String, scene: String, width: CGFloat = 1240, height: CGFloat = 800) -> Never {
        let app = NSApplication.shared
        // .accessory keeps it out of the Dock and out of the menu bar for the second it lives.
        app.setActivationPolicy(.accessory)

        let hosting = NSHostingView(rootView: RootView()
            .environment(seeded(scene: scene))
            .frame(width: width, height: height)
            .preferredColorScheme(.dark))
        hosting.frame = NSRect(x: 0, y: 0, width: width, height: height)

        let window = NSWindow(contentRect: hosting.frame,
                              styleMask: [.titled, .fullSizeContentView],
                              backing: .buffered, defer: false)
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.appearance = NSAppearance(named: .darkAqua)
        window.contentView = hosting
        if let screen = NSScreen.main {
            let frame = screen.visibleFrame
            window.setFrameOrigin(NSPoint(x: frame.minX + 40, y: frame.maxY - height - 40))
        }
        window.orderFrontRegardless()

        // SwiftUI needs a run-loop turn (plus a beat for the glass to composite) before the view
        // has a display list worth caching.
        DispatchQueue.main.asyncAfter(deadline: .now() + (scene == "suggest" ? 0.7 : 1.5)) {
            if scene == "suggest" { typeIntoEditor(in: window) }
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) {
            // A sheet is its own window; capture that when the scene opened one.
            capture(window: NSApp.windows.first { $0.isSheet } ?? window, to: path)
            exit(0)
        }
        app.run()
        exit(0)
    }

    @MainActor
    private static func capture(window: NSWindow, to path: String) {
        guard let view = window.contentView,
              let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else {
            FileHandle.standardError.write(Data("snapshot: no drawable content view\n".utf8))
            return
        }
        view.cacheDisplay(in: view.bounds, to: rep)
        guard let png = rep.representation(using: .png, properties: [:]) else {
            FileHandle.standardError.write(Data("snapshot: PNG encoding failed\n".utf8))
            return
        }
        do {
            try png.write(to: URL(fileURLWithPath: path))
            print("snapshot: wrote \(path) (\(rep.pixelsWide)x\(rep.pixelsHigh), \(png.count) bytes)")
        } catch {
            FileHandle.standardError.write(Data("snapshot: \(error.localizedDescription)\n".utf8))
        }
    }

    /// Types into the SQL editor through the real path — `insertText` fires `textDidChange`,
    /// which is what runs the debounce, the candidate lookup and the popup — so a `suggest`
    /// snapshot proves the whole chain, not just the popup's drawing.
    @MainActor
    private static func typeIntoEditor(in window: NSWindow) {
        guard let root = window.contentView, let textView = findTextView(in: root) else {
            FileHandle.standardError.write(Data("snapshot: no SQL editor found to type into\n".utf8))
            return
        }
        window.makeFirstResponder(textView)
        textView.string = "SELECT kode_wilayah, nama\nFROM "
        let end = (textView.string as NSString).length
        textView.setSelectedRange(NSRange(location: end, length: 0))
        textView.insertText("di", replacementRange: NSRange(location: end, length: 0))
    }

    @MainActor
    private static func findTextView(in view: NSView) -> SQLTextView? {
        if let match = view as? SQLTextView { return match }
        for subview in view.subviews {
            if let found = findTextView(in: subview) { return found }
        }
        return nil
    }

    /// A scene that exercises every surface: two connections, an expanded catalog with schemas
    /// and tables, and a finished query with columns, files and a log.
    @MainActor
    private static func seeded(scene: String) -> AppModel {
        let model = AppModel()
        let primary = Connection(id: UUID(), name: "Trino production", color: .blue, kind: .trino,
                                 host: "trino.internal", port: 8443, scheme: "https",
                                 user: "faisal", database: "hive", schema: "analytics", verify: true)
        let secondary = Connection(id: UUID(), name: "Trino sandbox", color: .amber, kind: .trino,
                                   host: "localhost", port: 8080, scheme: "http",
                                   user: "dev", database: "tpch", schema: "", verify: true)
        model.connections = [primary, secondary]
        model.rebuildTree()

        let root = model.tree[0]
        let catalog = TreeNode.catalog("hive", parent: root)
        let analytics = TreeNode.schema("analytics", parent: catalog)
        let bronze = TreeNode.schema("bronze", parent: catalog)
        analytics.children = ["penerima_manfaat", "wilayah", "program_bantuan", "dim_wilayah", "distribusi_bantuan"].map { TreeNode.table($0, parent: analytics) }
        bronze.children = [TreeNode.table("raw_kpm", parent: bronze)]
        catalog.children = [analytics, bronze]
        root.children = [catalog, TreeNode.catalog("tpch", parent: root)]
        root.expanded = true
        catalog.expanded = true
        analytics.expanded = true
        model.selectedNodeID = analytics.children?.first?.id

        guard let tab = model.selectedTab else { return model }
        tab.title = "penerima_manfaat"
        tab.connectionID = primary.id
        tab.sql = """
        SELECT kode_wilayah, nama, jumlah_jiwa, diperbarui
        FROM hive.analytics.penerima_manfaat
        WHERE tahun = 2026
        ORDER BY kode_wilayah
        """
        tab.format = .xlsx
        tab.outputName = "penerima_2026"
        tab.outputDirectory = ConnectionStore.defaultOutputDirectory
        tab.stage = .done
        tab.rows = 312_480
        tab.queryID = "20260131_120412_00042_abcde"
        tab.columns = [
            Event.Column(name: "kode_wilayah", type: "12"),
            Event.Column(name: "nama", type: "12"),
            Event.Column(name: "jumlah_jiwa", type: "4"),
            Event.Column(name: "diperbarui", type: "93"),
        ]
        tab.files = [
            Event.ExportedFile(path: "/tmp/penerima_2026.xlsx", bytes: 4_182_004),
            Event.ExportedFile(path: "/tmp/penerima_2026_part02.xlsx", bytes: 39_418),
        ]
        tab.startedAt = Date(timeIntervalSinceNow: -42)
        tab.finishedAt = .now
        tab.panel = .log
        let stamps = [-41.0, -40.5, -40.0, -12.0, -0.4, -0.2]
        let texts: [(LogLine.Kind, String)] = [
            (.info, "Trino production · trino.internal:8443/hive.analytics"),
            (.info, "XLSX → ~/Downloads/penerima_2026"),
            (.info, "Connecting…"),
            (.info, "Query 20260131_120412_00042_abcde · 4 columns"),
            (.success, "312,480 rows written to 2 files"),
            (.warning, "part02: 3 value(s) truncated to the dbf field width"),
        ]
        for (index, entry) in texts.enumerated() {
            tab.logLines.append(LogLine(at: Date(timeIntervalSinceNow: stamps[index]), kind: entry.0, text: entry.1))
        }

        switch scene {
        case "files":
            tab.panel = .files
        case "columns":
            tab.panel = .columns
        case "empty":
            model.tabs = []
            model.selectedTabID = nil
        case "running":
            tab.stage = .running
            tab.panel = .log
            tab.rows = 148_320
            tab.files = []
            tab.logLines = Array(tab.logLines.prefix(4))
            tab.startedAt = Date(timeIntervalSinceNow: -18)
        case "connection":
            model.presentConnectionEditor(primary.id)
        case "disabled":
            // No connection and no SQL: the primary action in its "not yet" state, which is what
            // a user sees the moment the app opens.
            tab.connectionID = nil
            tab.sql = ""
        case "new-connection":
            // The new-connection flow starts at the type grid, not at a form.
            model.presentConnectionEditor(nil)
        case "connection-uri":
            model.presentConnectionEditor(nil)
        case "connection-postgres", "connection-mysql":
            // The editor is not the same form three times over: Postgres has no catalog and
            // swaps the TLS toggle for an SSL mode, MySQL has no schema at all.
            let kind: ConnectionKind = scene == "connection-mysql" ? .mysql : .postgres
            let extra = Connection(id: UUID(),
                                   name: kind == .mysql ? "Reporting" : "Warehouse",
                                   color: kind == .mysql ? .amber : .violet, kind: kind,
                                   host: kind == .mysql ? "mysql.internal" : "pg.internal",
                                   port: kind.defaultPort,
                                   sslmode: kind.defaultSSLMode,
                                   user: "analyst",
                                   database: kind == .mysql ? "reporting" : "warehouse",
                                   schema: "public", verify: true)
            model.connections.append(extra)
            model.rebuildTree()
            model.presentConnectionEditor(extra.id)
        case "table":
            // The table destination: same tab, different toolbar, and a panel that reports the
            // statement Trino ran rather than a file list.
            tab.destination = .table
            tab.targetCatalog = "hive"
            tab.targetSchema = "analytics"
            tab.targetTable = "penerima_manfaat_2026"
            tab.writeMode = .create
            tab.writtenTable = "hive.analytics.penerima_manfaat_2026"
            tab.files = []
            tab.panel = .files
            tab.logLines[1] = LogLine(at: tab.logLines[1].at, kind: .info,
                                      text: "CREATE TABLE hive.analytics.penerima_manfaat_2026")
            tab.logLines[4] = LogLine(at: tab.logLines[4].at, kind: .success,
                                      text: "312,480 rows written to hive.analytics.penerima_manfaat_2026")
        default:
            break
        }
        return model
    }
}
