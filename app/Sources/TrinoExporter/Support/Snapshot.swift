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
        app.run()
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
    private static func seeded(scene: String) -> AppModel {
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
        case "connection-tested":
            // The footer's success state, which is otherwise unreachable without a server.
            model.presentConnectionEditor(primary.id, previewTestCount: 56)
        case "connection":
            model.presentConnectionEditor(primary.id)
        case "grid":
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
            tab.preview = PreviewResult(
                columns: tab.columns,
                rows: [
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
                ],
                truncated: true,
                queryID: "20260131_120412_00042_abcde",
                elapsedMS: 412)
            // The statement that produced what is on screen, so "Count all" has something.
            tab.previewedSQL = tab.sql
            tab.stage = .done
            tab.panel = .result
        case "explain":
            // The plan lands in the grid, where the rows would go — it is a result set too.
            tab.destination = .file
            tab.sql = "SELECT * FROM hive.analytics.penerima_manfaat"
            tab.columns = [Event.Column(name: "Query Plan", type: "varchar")]
            tab.preview = PreviewResult(
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
        case "table-opened":
            // What double-clicking a table in the tree now does. There is no server here, so the
            // rows are seeded the way the real reply would arrive — the tab, its name, the SQL and
            // the run are all `openTable`'s doing.
            if let node = model.allNodes().first(where: { $0.kind == .table }) {
                model.openTable(node)
                model.selectedTab?.previewing = false
                model.selectedTab?.preview = PreviewResult(
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
            tab.preview = PreviewResult(
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
            tab.preview = PreviewResult(
                columns: tab.columns,
                rows: (1...14).map { ["32.01.\(String(format: "%02d", $0)).2001", "KPM Wilayah \($0)"] },
                truncated: true, queryID: "20260131_120412_00042_abcde", elapsedMS: 210)
            tab.columnFilters = [0: .text("32.01.0")]
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
            tab.preview = PreviewResult(
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
