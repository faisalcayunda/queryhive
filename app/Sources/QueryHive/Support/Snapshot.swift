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

    /// `--theme <name>`, `--accent <name>`, `--tone <name>`, `--ui-font <family>` and
    /// `--code-font <family>`: draw the shell in an appearance the user has not chosen. All five go
    /// through `ThemeStore.pin`, so reviewing one never leaves it behind in the user's preferences
    /// — which matters because these flags are how the design is checked, and a check that rewrites
    /// what it is checking is not a check.
    static func requestedAppearance() -> (theme: AppTheme?, accent: AccentChoice?, tone: SurfaceTone?,
                                          mode: AppearanceMode?, systemIsDark: Bool?,
                                          uiFont: String?, codeFont: String?)? {
        let arguments = CommandLine.arguments
        func value(_ flag: String) -> String? {
            guard let index = arguments.firstIndex(of: flag), index + 1 < arguments.count else { return nil }
            return arguments[index + 1]
        }
        let theme = value("--theme").flatMap(AppTheme.init(rawValue:))
        let accent = value("--accent").flatMap(AccentChoice.init(rawValue:))
        let tone = value("--tone").flatMap(SurfaceTone.init(rawValue:))
        let mode = value("--mode").flatMap(AppearanceMode.init(rawValue:))
        // `--system-appearance dark|light` states what the *machine* would report. A snapshot has
        // no real window, so it cannot ask AppKit; this is how `--mode system` is drawn as light on
        // a machine that is currently dark.
        let systemIsDark = value("--system-appearance").map { $0 == "dark" }
        // Font families are taken verbatim: any installed family is valid, so there is nothing to
        // validate against and no reason to reject one this build has not heard of.
        let uiFont = value("--ui-font")
        let codeFont = value("--code-font")
        guard theme != nil || accent != nil || tone != nil || mode != nil || systemIsDark != nil
                || uiFont != nil || codeFont != nil else { return nil }
        return (theme, accent, tone, mode, systemIsDark, uiFont, codeFont)
    }

    @MainActor
    static func run(path: String, scene: String, width: CGFloat = 1240, height: CGFloat = 800) -> Never {
        let requested = requestedAppearance()
        // A snapshot has no window to inherit an appearance from, so the scheme it draws in is
        // decided here: the requested mode when there is one, otherwise the user's stored mode.
        //
        // Everything goes through `pin` in one call. Nothing here may touch a public setter: those
        // persist, and a review is not allowed to change the user's preferences. The order inside
        // `pin` (mode before theme) is what puts a light request in the light slot.
        let store = ThemeStore.shared
        // Always pinned, even with nothing requested: a render must not write to the user's
        // preferences, and a scene that seeds a saved appearance would otherwise keep it for real.
        store.pin(theme: requested?.theme, accent: requested?.accent, tone: requested?.tone,
                  mode: requested?.mode, systemIsDark: requested?.systemIsDark,
                  uiFont: requested?.uiFont, codeFont: requested?.codeFont)
        // A picture must not depend on the machine's Accessibility > Display switches.
        store.pin(reduceMotion: false, reduceTransparency: false, increaseContrast: false)
        let scheme = store.mode.colorScheme
        let app = NSApplication.shared
        // .accessory keeps it out of the Dock and out of the menu bar for the second it lives.
        app.setActivationPolicy(.accessory)

        // A render must not read or write what the user owns. Pointing the connections store at a
        // throwaway directory redirects `connections.json`, and `AppModel`'s local commands take
        // their `DB_PATH` from the same override: without this, a pane that runs a command when it
        // appears — the Account pane's `account` load, which creates the row — would write to the
        // database the user actually uses.
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("qh-snapshot-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        ConnectionStore.root = root

        let model = seeded(scene: scene)
        // Settings is its own scene in the running app, so it needs its own root here or it stays
        // the one screen nobody can look at without launching.
        let hosting: NSHostingView<AnyView>
        if scene.hasPrefix("settings") {
            // The scene suffix is the pane's raw value: `settings` opens General, `settings-editor`
            // opens Editor, and so on. `settings-fonts` is the old name for that pane and still
            // resolves, so an older review script does not silently photograph the wrong pane.
            let name = String(scene.dropFirst("settings-".count))
            let pane = SettingsView.Pane(rawValue: name) ?? (name == "fonts" ? .editor : .general)
            hosting = NSHostingView(rootView: AnyView(SettingsView(pane: pane)
                .environment(model)
                .preferredColorScheme(scheme)))
        } else if scene == "parameters" {
            // The `:name` prompt, rendered directly: a sheet is a window of its own and offscreen
            // capture cannot reach one, which is the same move the grid's JSON reader makes.
            hosting = NSHostingView(rootView: AnyView(ParameterSheet(prompt: Snapshot.parameterPrompt)
                .environment(model)
                .preferredColorScheme(scheme)))
        } else if scene == "grid-json" {
            // The structured-cell reader is a popover in the app, and a popover is a window of its
            // own that offscreen capture cannot reach. Rendering it directly is the same move the
            // Settings scenes make: the surface gets its own root so it can be looked at.
            hosting = NSHostingView(rootView: AnyView(CellValueViewer(
                value: #"{"kabupaten":"Bandung","jumlah_jiwa":48320,"kecamatan":["Sukamaju","Cibadak","Mekarsari"]}"#,
                column: "detail_wilayah",
                type: "map(varchar,json)")
                .frame(width: 592, height: 460)
                .background(Tone.canvas)
                .preferredColorScheme(scheme)))
        } else {
            hosting = NSHostingView(rootView: AnyView(RootView()
                .environment(model)
                .frame(width: width, height: height)
                .preferredColorScheme(scheme)))
        }
        hosting.frame = scene.hasPrefix("settings")
            ? NSRect(x: 0, y: 0, width: 560, height: 640)
            : scene == "grid-json"
                ? NSRect(x: 0, y: 0, width: 592, height: 460)
                : NSRect(x: 0, y: 0, width: width, height: height)

        let window = NSWindow(contentRect: hosting.frame,
                              styleMask: [.titled, .fullSizeContentView],
                              backing: .buffered, defer: false)
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        // The window's own appearance decides what `.ultraThinMaterial` samples and how every
        // dynamic colour resolves, so it has to match the scheme being drawn.
        window.appearance = NSAppearance(named: scheme == .light ? .aqua : .darkAqua)
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
            captureAndExit(window: window, path: path)
        }
        app.run()
        exit(0)
    }

    @MainActor
    private static func captureAndExit(window: NSWindow, path: String) {
        // Sheets and popovers are windows of their own, and a scene that opened one means
        // that one, not the window behind it. Without this the format and target popovers
        // were the only surfaces in the app nobody ever looked at.
        let overlay = NSApp.windows.first { $0.isSheet }
            ?? NSApp.windows.first { String(describing: type(of: $0)).contains("Popover") }
        // A sheet or popover draws its own chrome as a system material, which
        // `cacheDisplay` cannot sample — the capture comes back transparent where the
        // background should be, and white text on it is invisible. Compositing over the
        // canvas colour shows what the eye sees.
        capture(window: overlay ?? window, to: path, over: overlay == nil ? nil : Tone.canvas)
        exit(0)
    }

    @MainActor
    private static func capture(window: NSWindow, to path: String, over background: Color? = nil) {
        guard let view = window.contentView,
              let rep = view.bitmapImageRepForCachingDisplay(in: view.bounds) else {
            FileHandle.standardError.write(Data("snapshot: no drawable content view\n".utf8))
            return
        }
        view.cacheDisplay(in: view.bounds, to: rep)

        var image = NSImage(size: view.bounds.size)
        image.addRepresentation(rep)
        if let background {
            let flattened = NSImage(size: view.bounds.size)
            flattened.lockFocus()
            NSColor(background).setFill()
            NSRect(origin: .zero, size: view.bounds.size).fill()
            image.draw(in: NSRect(origin: .zero, size: view.bounds.size))
            flattened.unlockFocus()
            image = flattened
        }
        guard let tiff = image.tiffRepresentation,
              let bitmap = NSBitmapImageRep(data: tiff),
              let png = bitmap.representation(using: .png, properties: [:]) else {
            FileHandle.standardError.write(Data("snapshot: PNG encoding failed\n".utf8))
            return
        }
        do {
            try png.write(to: URL(fileURLWithPath: path))
            print("snapshot: wrote \(path) (\(bitmap.pixelsWide)x\(bitmap.pixelsHigh), \(png.count) bytes)")
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
    /// The prompt the `parameters` scene draws: one value of each kind that is worth looking at.
    static let parameterPrompt = ParameterPrompt(
        tabID: UUID(),
        source: .statement,
        template: "SELECT * FROM hive.analytics.penerima_manfaat\n"
            + "WHERE tahun = :tahun AND aktif = :aktif AND nama LIKE :wilayah",
        names: ["tahun", "aktif", "wilayah"],
        kind: .trino,
        initial: ["tahun": ParameterEntry(kind: .number, text: "2026"),
                  "aktif": ParameterEntry(kind: .boolean, text: "true"),
                  "wilayah": ParameterEntry(kind: .text, text: "%Sukamaju%")])

    static func seeded(scene: String) -> AppModel {
        // Every scene that draws rows holds them in a store (`QueryTab.showRows`). Spill is off and
        // the budget is small: a fixture is a few dozen rows.
        RustEngine.ensureStoresConfigured(spillDir: nil, budgetBytes: 64 << 20)
        let model = AppModel()
        let primary = Connection(id: UUID(), name: "Trino production", color: .blue, kind: .trino,
                                 host: "trino.internal", port: 8443, scheme: "https",
                                 user: "faisal", database: "hive", schema: "analytics", verify: true)
        let secondary = Connection(id: UUID(), name: "Trino sandbox", color: .amber, kind: .trino,
                                   host: "localhost", port: 8080, scheme: "http",
                                   user: "dev", database: "tpch", schema: "", verify: true)
        // The cascade's levels follow the driver, so each one needs its own connection to show
        // what it does and does not offer.
        let pg = Connection(id: UUID(), name: "Warehouse", color: .violet, kind: .postgres,
                            host: "pg.internal", port: 5432, sslmode: "prefer",
                            user: "analyst", database: "warehouse", schema: "public", verify: true)
        let my = Connection(id: UUID(), name: "Reporting", color: .amber, kind: .mysql,
                            host: "mysql.internal", port: 3306, sslmode: "disable",
                            user: "analyst", database: "reporting", schema: "", verify: true)
        model.connections = [primary, secondary, pg, my]
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
        // A kept appearance and a state that matches neither a preset nor a saved look, so the
        // list's Saved group and its Custom row are in the picture rather than only in the code.
        case "settings-appearance":
            let store = ThemeStore.shared
            store.accent = .violet
            store.saveLook(named: "Night shift")
            store.accent = .mint

        // Open Quickly with something to find: the tree seeded above, two saved queries and a
        // history row, so the palette is reviewable without typing into it.
        case "quickly":
            model.openQuicklyQuery = "pener"
            model.openQuicklyOpen = true
            model.openQuicklyIndex = 0
            model.savedQueries = [
                Event.SavedQuery(
                    id: "s1", name: "penerima per wilayah",
                    sql: "SELECT kode_wilayah, COUNT(*) FROM hive.analytics.penerima_manfaat GROUP BY 1",
                    connectionId: nil, folderId: nil, favourite: true, deleted: false, version: 1),
                Event.SavedQuery(
                    id: "s2", name: "Wilayah terbaru",
                    sql: "SELECT * FROM hive.analytics.wilayah ORDER BY diperbarui DESC",
                    connectionId: nil, folderId: nil, favourite: false, deleted: false, version: 1),
            ]
            model.historyEntries = [
                Event.HistoryEntry(
                    id: "h1", sql: "SELECT COUNT(*) FROM hive.analytics.penerima_manfaat",
                    startedAt: 0, elapsedMs: 812, rowCount: 1, outcome: "ok", error: nil,
                    connectionId: nil, deleted: false, version: 1),
            ]

        // Groups holding the seeded connections, so the folder rows — their mark, their count of
        // connections and the way a connection reads inside one — are reviewable without clicking
        // through the menu that makes them.
        case "groups":
            let production = ConnectionGroup(name: "Production")
            let staging = ConnectionGroup(name: "Staging")
            model.groups = [production, staging]
            model.connections[0].group = production.id
            model.connections[2].group = production.id
            model.connections[3].group = staging.id
            model.rebuildTree()
            // One folder open and one shut, because both states are part of the picture.
            for node in model.tree where node.kind == .group {
                node.expanded = node.title == "Production"
            }
        // A tab that has been opened and not used: no rows, no log lines, no files. This is the
        // state the panel's default is about, and it is what `--scene fresh-tab` is for.
        case "fresh-tab":
            tab.logLines = []
            tab.preview = nil
            tab.files = []
            tab.stage = .idle
            tab.panel = .result
            // The seeded lines above are what the other scenes read; this scene is the absence of
            // them, so the panel's content check has to run the way `newTab` runs it.
            model.panelCollapsed = true
        case "files":
            tab.panel = .files
        case "columns":
            tab.panel = .result
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
        case "objects":
            // What clicking a schema now opens. The columns and rows are seeded exactly as the
            // engine's `objects` event carries them -- Postgres answers all four of these from
            // `pg_class` -- because `openObjects` would otherwise start a real run against a
            // server this fixture does not have.
            let objectsTab = QueryTab(title: "analytics")
            objectsTab.connectionID = primary.id
            objectsTab.objectScope = ObjectScope(connectionID: primary.id,
                                                 catalog: "hive", schema: "analytics")
            objectsTab.objectColumns = ["Name", "OID", "Owner", "ACL"]
            objectsTab.objectRows = [
                ["kasus_kesehatan", "24601", "app_datahub", ""],
                ["penerima_manfaat", "24602", "app_datahub", "{app_datahub=arwdDxt/app_datahub,readonly=r/app_datahub}"],
                ["wilayah", "24603", "postgres", ""],
                ["wilayah_kode", "24604", "app_datahub", ""],
                ["wilayah_replika", "24605", "replication", "{replication=arwdDxt/replication}"],
                ["kasus_harian", "24606", "app_datahub", ""],
                ["referensi_jenis_kelamin", "24607", "postgres", ""],
                ["kasus_kesehatan_2025", "24608", "app_datahub", ""],
            ]
            // One row chosen, so the fixture draws the inspector and its columns. Without a
            // selection the scene would only prove the grid renders, and the pane that reads the
            // table's own columns is the half that has nothing else to check it.
            objectsTab.objectSelection = 1
            objectsTab.objectDetailTable = "penerima_manfaat"
            objectsTab.objectDetailColumns = [
                Event.Column(name: "id", type: "bigint"),
                Event.Column(name: "nik", type: "character varying(16)"),
                Event.Column(name: "nama", type: "text"),
                Event.Column(name: "wilayah_kode", type: "character varying(10)"),
                Event.Column(name: "tanggal_lahir", type: "date"),
                Event.Column(name: "jenis_kelamin", type: "mood"),
                Event.Column(name: "penghasilan_bulanan", type: "numeric(38,10)"),
                Event.Column(name: "terdaftar_pada", type: "timestamp with time zone"),
                Event.Column(name: "dibuat_pada", type: "timestamp without time zone"),
            ]
            model.tabs.append(objectsTab)
            model.selectedTabID = objectsTab.id
        case "objects-trino":
            // The Trino listing, and the reason it needs a scene of its own: two columns whose first
            // holds a long name. The four-column Postgres fixture hides the thing this one exists to
            // show — that the name column has to take the room the pane actually has, or every name
            // truncates beside an empty half-window.
            let objectsTab = QueryTab(title: "snapshot")
            objectsTab.connectionID = primary.id
            objectsTab.objectScope = ObjectScope(connectionID: primary.id,
                                                 catalog: "prod_datalake_pii_rw",
                                                 schema: "pem.pem_kelompok_pm.snapshot")
            objectsTab.objectColumns = ["Name", "Type"]
            objectsTab.objectRows = [
                ["daftar_sppg_per_kelompok_pm_a_2_20260915", "BASE TABLE"],
                ["dashboard_monitoring_pppg_20260911", "BASE TABLE"],
                ["sps2_belum_optimal_pppg_20260915", "BASE TABLE"],
                ["sudah_dicabut_dari_lhs_20260915", "BASE TABLE"],
                ["t_spgg_suspect_20260915_1621", "BASE TABLE"],
                ["t_spgg_suspect_20260917_17_15", "BASE TABLE"],
                ["t_spgg_suspect_20260921_1434", "BASE TABLE"],
                ["t_spgg_suspect_20260923_1431", "BASE TABLE"],
                ["t_spgg_suspect_20260925_1000", "BASE TABLE"],
            ]
            model.tabs.append(objectsTab)
            model.selectedTabID = objectsTab.id
        case "objects-empty", "objects-loading":
            // A schema the coordinator answered with nothing in it. The pane has had an empty
            // state for a while; what it had no scene for is this one, so nothing looked at it.
            let emptyTab = QueryTab(title: "bronze")
            emptyTab.connectionID = primary.id
            emptyTab.objectScope = ObjectScope(connectionID: primary.id,
                                               catalog: "hive", schema: "bronze")
            emptyTab.objectColumns = ["Name", "Type"]
            emptyTab.objectRows = []
            emptyTab.objectLoading = scene == "objects-loading"
            model.tabs.append(emptyTab)
            model.selectedTabID = emptyTab.id
        case "tree-empty-schema":
            // A schema with no tables under it, open. The row below it is the empty state, and the
            // mark it draws is what decides whether that reads as empty or as still loading.
            if let catalog = root.children?.first(where: { $0.title == "hive" }),
               let schema = catalog.children?.first(where: { $0.title == "bronze" }) {
                schema.children = []
                schema.loading = false
                schema.expanded = true
                catalog.expanded = true
            }
        case "tree-databases":
            // A Postgres connection with "show all databases" on. What the flag changes is the shape
            // of what hangs under the connection: the schemas that were one level down now sit under
            // a database, with a sibling for every other database this user may open. So the scene is
            // the tree — the menu entry and the editor's checkbox that flip it are drawn by their own
            // scenes, and neither of those shows what the setting is *for*.
            if let index = model.connections.firstIndex(where: { $0.kind == .postgres }) {
                model.connections[index].showAllDatabases = true
            }
            model.rebuildTree()
            if let warehouse = model.tree.first(where: { $0.connectionKind == .postgres }) {
                warehouse.expanded = true
                warehouse.children = ["postgres", "reporting", "warehouse"].map {
                    TreeNode.database($0, parent: warehouse)
                }
                if let database = warehouse.children?.first(where: { $0.title == "warehouse" }) {
                    database.expanded = true
                    database.children = ["public", "staging"].map {
                        TreeNode.schema($0, parent: database)
                    }
                    if let staging = database.children?.first(where: { $0.title == "staging" }) {
                        staging.expanded = true
                        staging.children = ["kpm_staging", "wilayah_staging"].map {
                            TreeNode.table($0, parent: staging)
                        }
                    }
                }
            }
        case "connection-tested":
            // The footer's success state, which is otherwise unreachable without a server.
            model.presentConnectionEditor(primary.id, previewTestCount: 56)
        case "connection":
            model.presentConnectionEditor(primary.id)
        case "grid", "grid-selection", "grid-edits", "grid-sorted", "grid-columns", "grid-inspector",
             "grid-kinds", "grid-counted":
            // Run's whole point: the rows, before anything is written. Deliberately mixed — a
            // long text column, numbers that must right-align, a NULL, a timestamp, and a result
            // the row limit cut short.
            tab.destination = .file
            tab.sql = "SELECT * FROM hive.analytics.penerima_manfaat"
            tab.rowLimit = 1000
            tab.columns = [
                Event.Column(name: "kode_wilayah", type: "varchar"),
                Event.Column(name: "nama", type: "varchar"),
                Event.Column(name: "jumlah_jiwa", type: "bigint"),
                Event.Column(name: "bobot", type: "double"),
                Event.Column(name: "aktif", type: "boolean"),
                Event.Column(name: "diperbarui", type: "timestamp(6)"),
                Event.Column(name: "catatan", type: "varchar"),
            ]
            // The JSON column and the long cell are set here, before the rows go into a store: a
            // store is read-only, so the scenes that edit a cell cannot do it afterwards.
            var gridRows: [[String?]] = [
                    ["32.01.01.2001", "KPM Sukamaju", "4", "142.857", "true", "2026-07-25 15:30:06.233", nil],
                    ["32.01.01.2002", "KPM Cibadak", "2", "97.321", "true", "2026-07-25 15:30:06.233", "verifikasi lapangan"],
                    ["32.01.01.2003", "KPM Mekarsari", "7", "249.998", "false", "2026-07-25 15:30:06.233", nil],
                    ["32.01.02.1004", "KPM Sukajadi", "1", "12.004", "true", "2026-07-26 08:03:07.567", nil],
                    ["32.01.02.1005", "KPM Cipedes", "9", "318.442", "true", "2026-07-26 08:03:07.567", "pindah domisili"],
                    ["32.01.03.2010", "KPM Rancamanyar", "3", "76.518", "false", "2026-07-27 09:14:22.101", nil],
                    ["32.01.03.2011", "KPM Bojongsoang", "5", "188.930", "true", "2026-07-27 09:14:22.101", nil],
                    ["32.01.04.3001", "KPM Dayeuhkolot", "6", "204.117", "true", "2026-08-01 07:45:59.880", "data ganda"],
                    ["32.01.04.3002", "KPM Citeureup", "2", "66.204", "false", "2026-08-01 07:45:59.880", nil],
                    ["32.01.05.4009", "KPM Cangkuang", "8", "271.555", "true", "2026-08-03 11:22:31.004", nil],
                    ["32.01.05.4010", "KPM Banjaran", "4", "133.870", "true", "2026-08-03 11:22:31.004", nil],
                    ["32.01.06.5001", "KPM Margahayu", "1", "8.412", "false", "2026-08-03 11:22:31.004", "menunggu verifikasi"],
            ]
            if scene == "grid-inspector" {
                tab.columns[6] = Event.Column(name: "catatan", type: "json")
                gridRows[1][6] = #"{"masalah":"verifikasi lapangan","petugas":"BGN-04","selesai":false}"#
            }
            if scene == "grid-kinds" {
                tab.columns[6] = Event.Column(name: "catatan", type: "json")
                gridRows[1][6] = #"{"masalah":"verifikasi lapangan","petugas":"BGN-04"}"#
                gridRows[4][6] = "[1,2,3]"
                gridRows[7][6] = #"{"ganda":true}"#
                gridRows[11][6] = #"{"status":"menunggu"}"#
                gridRows[2][0] = ""
                gridRows[3][1] = "KPM Sukajadi dengan nama yang sangat panjang sehingga tidak muat "
                    + "di dalam satu sel dan harus dipotong di ujung kanannya oleh grid"
            }
            tab.showRows(columns: tab.columns, rows: gridRows, truncated: true,
                         queryID: "20260131_120412_00042_abcde", elapsedMS: 412)
            // The statement that produced what is on screen, so "Count all" has something.
            tab.previewedSQL = tab.sql
            tab.stage = .done
            tab.panel = .result
            // The drag-selected block, drawn only for the scene that exists to show it: the tint
            // over the cells and the footer's own readout are the half of drag-to-select that no
            // unit test can see.
            if scene == "grid-selection" {
                tab.cellSelection = CellRange(from: (row: 1, column: 0), to: (row: 4, column: 1))
            }
            // The staged cell edits: the amber wash and its dot, and the commit pair in the footer.
            // Drawn from the same values the model would hold after a real edit, so the scene shows
            // the queue's own shape rather than a mock of it.
            if scene == "grid-edits" {
                tab.sourceTable = "\"hive\".\"analytics\".\"penerima_manfaat\""
                tab.cellEdits.edit("KPM Cibadak Baru", at: CellKey(row: 1, column: 1),
                                   original: "KPM Cibadak")
                tab.cellEdits.edit("9", at: CellKey(row: 2, column: 2), original: "7")
                tab.cellSelection = CellRange(from: (row: 1, column: 0), to: (row: 2, column: 1))
            }
            // The in-memory sort, and the banner that says the order is the grid's own rather than
            // the server's. Sorted on `jumlah_jiwa`, which is the column the rows below are not
            // already in order by — so the scene shows the sort doing something.
            if scene == "grid-sorted" {
                tab.applyMemorySort(GridSort(column: 2, direction: .ascending))
                tab.applyViewBlocking()
            }
            // The rendering-only column work: one column renamed, one hidden, one moved, and a
            // cross-column search narrowing the rows. Drawn from the same model calls the UI makes,
            // so the scene shows the feature's own state rather than a mock of it.
            if scene == "grid-columns" {
                tab.renameColumn(1, to: "Nama KPM")
                tab.moveColumn(from: 6, to: 1)    // `catatan` up beside the name
                tab.setColumnHidden(4, true)      // `aktif`
                tab.gridSearch = "kpm"
                tab.applyViewBlocking()
            }
            // The value reader standing beside the grid rather than over the cell. `pin` and not the
            // public setter: the setters write, and a render must leave the user's preferences as it
            // found them.
            if scene == "grid-inspector" {
                DataPreferences.shared.pin(autoShowInspector: true)
                tab.sourceTable = "\"hive\".\"analytics\".\"penerima_manfaat\""
                // `catatan` is a JSON column for this scene only, so the reader's mode picker and
                // its two-row header are in the picture. Every column this fixture otherwise carries
                // is a scalar, and a scalar offers one mode, which is no picker at all.
                tab.cellSelection = CellRange(from: (row: 1, column: 6), to: (row: 1, column: 6))
            }
            // Every cell kind, header state and footer state at once, for the visual parity gate: a
            // NULL, an empty string, a long text, JSON and numbers; a sort chevron, an active filter
            // funnel and a type chip on every header; a staged edit, a selected block, and the footer
            // that goes with them. Order matters: the filter clears the sort, and the sort clears the
            // selection and the queue, so they are set in that order. `aktif` and `diperbarui` are
            // hidden so the JSON column and its NULL fit inside the window the gate renders at.
            if scene == "grid-kinds" {
                tab.sourceTable = "\"hive\".\"analytics\".\"penerima_manfaat\""
                tab.setColumnHidden(4, true)
                tab.setColumnHidden(5, true)
                tab.columnFilters[1] = .text("KPM")
                tab.applyMemorySort(GridSort(column: 2, direction: .ascending))
                // The view has to be in before the edits are staged: its landing drops them.
                tab.applyViewBlocking()
                tab.cellEdits.edit("KPM Cibadak Baru", at: CellKey(row: 1, column: 1),
                                   original: "KPM Cibadak")
                tab.cellEdits.edit("9", at: CellKey(row: 2, column: 2), original: "7")
                tab.cellSelection = CellRange(from: (row: 5, column: 3), to: (row: 7, column: 4))
            }
            // The footer once the server has been asked how many rows there really are.
            if scene == "grid-counted" {
                tab.totalRows = 312_480
            }
        case "grid-empty", "grid-loading", "grid-filtered-out":
            // A run whose columns are on screen and whose rows are not. Three states look exactly
            // alike in a blank grid — waiting for the first batch, a statement that matched nothing,
            // and a filter that hid everything it did match — and each scene is one of them. The
            // header is the same in all three; what stands under it is the whole point.
            tab.destination = .file
            tab.sql = "SELECT * FROM hive.analytics.penerima_manfaat WHERE tahun = 2026"
            tab.rowLimit = 1000
            // The shape from the user's own Hive schema, because that is the result that produced
            // this scene: a table whose rows were empty and whose header was all there was to see.
            tab.columns = [
                Event.Column(name: "code", type: "varchar(64)"),
                Event.Column(name: "fullname", type: "varchar(255)"),
                Event.Column(name: "code_beneficiary", type: "varchar(255)"),
                Event.Column(name: "date_of_birth", type: "date"),
                Event.Column(name: "gender", type: "varchar(16)"),
                Event.Column(name: "is_active", type: "boolean"),
                Event.Column(name: "institution_code", type: "varchar(64)"),
                Event.Column(name: "subcategory_code", type: "varchar(64)"),
            ]
            if scene == "grid-filtered-out" {
                // Rows that a filter then hides. The filter is opened on `gender` and given a value
                // none of them carries, which is the state that needs the explanation and the way
                // out; the other two scenes have nothing to hide and no filter to clear.
                tab.showRows(
                    columns: tab.columns, rows: [
                        ["KPM-0001", "KPM Sukamaju", "3201012001", "1987-04-11", "P", "true", "INS-01", "SUB-07"],
                        ["KPM-0002", "KPM Cibadak", "3201012002", "1991-09-02", "L", "false", "INS-01", "SUB-03"],
                        ["KPM-0003", "KPM Mekarsari", "3201012003", "1979-12-30", "P", "true", "INS-02", "SUB-07"],
                    ],
                    truncated: false, queryID: "20260131_120412_00042_abcde", elapsedMS: 383)
                tab.columnFilters[4] = .values(["X"])
                tab.applyViewBlocking()
            } else {
                // Columns and no rows at all. The loading scene keeps `previewing` on, which is the
                // moment the engine has sent the header and not the first batch — the state the
                // footer used to call "0 rows" while it was still counting.
                let loading = scene == "grid-loading"
                tab.showRows(
                    columns: tab.columns, rows: [], truncated: false, queryID: nil,
                    elapsedMS: loading ? 0 : 383)
                tab.previewing = loading
                tab.stage = loading ? .running : .done
            }
            tab.previewedSQL = tab.sql
            tab.panel = .result
        case "explain":
            // The plan lands in the grid, where the rows would go — it is a result set too.
            tab.destination = .file
            tab.sql = "SELECT * FROM hive.analytics.penerima_manfaat"
            tab.columns = [Event.Column(name: "Query Plan", type: "varchar")]
            tab.showRows(
                columns: tab.columns,
                rows: [
                    ["Output[kode_wilayah, nama, jumlah_jiwa, bobot, aktif, diperbarui]"],
                    ["│   Layout: [kode_wilayah:varchar, nama:varchar, jumlah_jiwa:bigint]"],
                    ["│   Estimates: {rows: 48320 (1.21MB), cpu: ?, memory: ?, network: ?}"],
                    ["│   aktif := false"],
                    ["└─ TableScan[hive.analytics.penerima_manfaat]"],
                    ["       Layout: [kode_wilayah:varchar, nama:varchar, jumlah_jiwa:bigint]"],
                    ["       Estimates: {rows: 48320 (1.21MB), cpu: 1.21M, memory: 0B, network: 0B}"],
                    ["       tahun := 2026"],
                    ["       aktif := true"],
                ],
                truncated: false, queryID: "20260131_120412_00042_abcde", elapsedMS: 148)
            tab.previewedSQL = tab.sql
            tab.showingPlan = true
            tab.stage = .done
            tab.panel = .result
        case "tree-large":
            // The shape from a real Trino server: dozens of catalogs, each open, each with
            // schemas and their tables. Built to measure the tree's layout cost rather than to
            // look at, which is why nothing here is named after anything.
            let rootNode = model.tree[0]
            rootNode.children = (0..<40).map { index in
                let catalog = TreeNode.catalog(String(format: "datawarehouse-%02d", index),
                                               parent: rootNode)
                catalog.expanded = true
                catalog.children = (0..<8).map { s in
                    let schema = TreeNode.schema("analytics_\(s)", parent: catalog)
                    schema.expanded = true
                    schema.children = (0..<20).map { t in
                        TreeNode.table(String(format: "table_%02d_%02d_%02d", index, s, t),
                                       parent: schema)
                    }
                    return schema
                }
                return catalog
            }
            rootNode.expanded = true
        case "cascade-long":
            // The case that broke the breadcrumb: a catalog long enough to push Run off the bar.
            tab.connectionID = primary.id
            tab.contextDatabase = "aktivitas-produksi-harian"
            tab.contextSchema = "sandbox"
            tab.sql = "SELECT * FROM wilayah"
        case "cascade-postgres", "cascade-mysql":
            // What the breadcrumb offers per driver: Postgres has no catalog level, MySQL no schema.
            let wanted = scene == "cascade-mysql" ? "Reporting" : "Warehouse"
            if let connection = model.connections.first(where: { $0.name == wanted }) {
                tab.connectionID = connection.id
            }
            tab.sql = "SELECT * FROM wilayah"
        case "syntax":
            // Every class the colourer knows, in SQL that reads like the real thing.
            tab.sql = """
            -- Ringkasan penerima manfaat per wilayah, 2026
            WITH bersih AS (
                SELECT kode_wilayah,
                       UPPER(TRIM(nama)) AS nama,
                       CAST(jumlah_jiwa AS bigint) AS jiwa,
                       COUNT(*) OVER (PARTITION BY kode_wilayah) AS baris
                FROM "hive"."analytics"."penerima_manfaat"
                WHERE tahun = 2026 AND aktif = true AND catatan IS NOT NULL
            )
            /* hanya wilayah yang lolos verifikasi */
            SELECT b.kode_wilayah, b.nama, b.jiwa, b.baris, 'SLHS_TERBIT' AS status
            FROM bersih b
            JOIN `gold`.`dim_wilayah` w ON w.kode = b.kode_wilayah
            WHERE b.jiwa >= 3.5 AND b.nama LIKE '%Sukamaju%'
            ORDER BY b.jiwa DESC
            LIMIT 1000;
            """
            tab.panel = .log
            // The caret in the second statement, so the statement band is visibly on a range other
            // than the first: a render that only ever shows the top of a file cannot tell the two
            // highlights apart.
            let offset = (tab.sql as NSString).range(of: "b.jiwa >= 3.5").location
            if offset != NSNotFound {
                tab.caret = offset
                tab.selection = NSRange(location: offset, length: 0)
            }
        case "table-opened":
            // What double-clicking a table in the tree now does. There is no server here, so the
            // rows are seeded the way the real reply would arrive — the tab, its name, the SQL and
            // the run are all `openTable`'s doing.
            if let node = model.allNodes().first(where: { $0.kind == .table }) {
                model.openTable(node)
                model.selectedTab?.previewing = false
                model.selectedTab?.showRows(
                    columns: [
                        Event.Column(name: "id_sppg", type: "varchar"),
                        Event.Column(name: "status_operasional_sppg", type: "varchar"),
                        Event.Column(name: "memiliki_hari_berhenti_ops", type: "boolean"),
                        Event.Column(name: "status_slhs_terakhir", type: "varchar"),
                    ],
                    rows: [
                        ["0ZDLVY6W", "operasional", "false", "SLHS_TERBIT"],
                        ["1YEQ3SGF", "operasional", "false", "SLHS_TERBIT"],
                        ["2UGE3CG", "operasional", "false", "SLHS_TERBIT"],
                        ["4FYJ5ETO", "operasional", "false", "SLHS_TERBIT"],
                        ["59QMlA7P", "operasional", "false", "SLHS_TERBIT"],
                        ["6JLBLEJ6", "operasional", "false", "SUDAH_DIAJUKAN"],
                        ["6MJZJZMY", "berhenti-ops-sementara", "true", "SLHS_TERBIT"],
                    ],
                    truncated: true, queryID: "20260131_120412_00042_abcde", elapsedMS: 233)
            }
        case "filter-values":
            // A column with few distinct values: the picker lists them, straight from the data.
            tab.destination = .file
            tab.sql = "SELECT * FROM hive.analytics.kasus_kesehatan"
            tab.columns = [
                Event.Column(name: "nama_provinsi", type: "varchar"),
                Event.Column(name: "jenis_kelamin", type: "varchar"),
                Event.Column(name: "jumlah_kasus_baru", type: "bigint"),
            ]
            tab.showRows(
                columns: tab.columns,
                rows: [
                    ["JAWA BARAT", "LAKI-LAKI", "41"],
                    ["JAWA BARAT", "PEREMPUAN", "22"],
                    ["JAWA BARAT", "LAKI LAKI", "150"],
                    ["DKI JAKARTA", "LAKI-LAKI", "88"],
                    ["DKI JAKARTA", "PEREMPUAN", "97"],
                    ["JAWA TIMUR", nil, "12"],
                ],
                truncated: false, queryID: "20260131_120412_00042_abcde", elapsedMS: 96)
            tab.columnFilters = [1: .values(["LAKI-LAKI", "PEREMPUAN"])]
            tab.applyViewBlocking()
            tab.stage = .done
            tab.panel = .result
            model.filterPopoverColumn = 1
        case "filter-search":
            // Past the picker's limit the same column becomes a search box instead — the mode is
            // decided by the data, not by the user.
            tab.destination = .file
            tab.sql = "SELECT * FROM hive.analytics.penerima_manfaat"
            tab.columns = [
                Event.Column(name: "kode_wilayah", type: "varchar"),
                Event.Column(name: "nama", type: "varchar"),
            ]
            tab.showRows(
                columns: tab.columns,
                rows: (1...14).map { ["32.01.\(String(format: "%02d", $0)).2001", "KPM Wilayah \($0)"] },
                truncated: true, queryID: "20260131_120412_00042_abcde", elapsedMS: 210)
            tab.columnFilters = [0: .text("32.01.0")]
            tab.applyViewBlocking()
            tab.stage = .done
            tab.panel = .result
            model.filterPopoverColumn = 0
        case "grid-filtered":
            // A filter narrows what was fetched; the footer has to say so rather than claim the
            // result is this small.
            tab.destination = .file
            tab.sql = "SELECT * FROM hive.analytics.penerima_manfaat"
            tab.columns = [
                Event.Column(name: "kode_wilayah", type: "varchar"),
                Event.Column(name: "nama", type: "varchar"),
                Event.Column(name: "jumlah_jiwa", type: "bigint"),
                Event.Column(name: "aktif", type: "boolean"),
            ]
            tab.showRows(
                columns: tab.columns,
                rows: [
                    ["32.01.01.2001", "KPM Sukamaju", "4", "true"],
                    ["32.01.01.2002", "KPM Cibadak", "2", "true"],
                    ["32.01.02.1004", "KPM Sukajadi", "1", "true"],
                    ["32.01.03.2010", "KPM Rancamanyar", "3", "false"],
                    ["32.02.01.3001", "KPM Dayeuhkolot", "6", "true"],
                ],
                truncated: true, queryID: "20260131_120412_00042_abcde", elapsedMS: 388)
            tab.columnFilters = [0: .text("32.01"), 3: .text("=true")]
            tab.applyViewBlocking()
            tab.stage = .done
            tab.panel = .result
        case "export-settings":
            // The one popover that now holds every destination choice.
            tab.destination = .file
            tab.format = .xlsx
            model.exportSettingsOpen = true
        case "table-target":
            // The target popover, which cannot be reached from a plain snapshot otherwise.
            tab.destination = .table
            tab.targetCatalog = "hive"
            tab.targetSchema = "analytics"
            tab.targetTable = "penerima_manfaat_2026"
            model.targetPopoverOpen = true
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
        case "connection-trino":
            // Trino's form is not the Postgres one with a field renamed: it is the only driver
            // with a transport picker, and the only one whose third transport (`prefer`) is
            // neither a scheme nor a verification answer. Seeded on `prefer` so a review draws
            // the state a snapshot could not otherwise reach.
            let trino = Connection(id: UUID(), name: "Trino prefer", color: .amber, kind: .trino,
                                   host: "localhost", port: 8080, scheme: "prefer",
                                   user: "dev", database: "tpch", schema: "", verify: true)
            model.connections.append(trino)
            model.rebuildTree()
            model.presentConnectionEditor(trino.id)
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
        // The scenes above fill in content by hand, so the panel's own rule has to be applied after
        // them rather than before: it is what the app does when a tab changes, and a render that
        // skipped it would show a state the app cannot reach (a grid full of rows behind a closed
        // panel, or a header open over nothing). `fresh-tab` sets its own state on purpose.
        if scene != "fresh-tab" {
            model.syncPanelToSelectedTab()
        }
        return model
    }
}
