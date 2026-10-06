import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

// Chrome baselines (W9-T0b, blueprint w9-shell-and-a11y §15.1, D-19).
//
// `VisualParityTests` guards the grid and the editor and nothing around them. This file draws the
// rest of what a person sees (the shell, the tree, Open Quickly, the connection sheet, Settings)
// through the real AppKit/SwiftUI view tree and keeps the same two layers the grid gate ends with,
// through the same comparator (`ParityComparator`, `Sidecar`, `Pixels`):
//
//   layout   the picture size and the switches the scene was drawn under, which must be equal
//   pixels   at most 0.1% of the pixels may differ by more than 16/255 in any channel
//
// There are no measured separators, sample points or markers: chrome has no grid to measure.
//
// Record:   QH_RECORD_CHROME=1 swift test --filter ChromeParityTests            (every scene)
//           QH_RECORD_CHROME=empty,fresh-tab swift test --filter ChromeParityTests   (named scenes)
// Compare:  swift test --filter ChromeParityTests
//
// A scene is drawn twice before it is recorded and the two pictures must be identical byte for
// byte, so a baseline is never a coin that happened to land. Recording fails the run after writing,
// so a gate left in record mode cannot report green. The variable is not `QH_RECORD_BASELINES` on
// purpose: that one re-records the grid and the editor, and re-recording needs the owner's approval
// (development-plan §0.5). Re-recording a chrome scene is a declared V-n change in its own commit.
//
// Left out, because they show the clock: `done`, `running` and `files` (a log of timestamps relative
// to now, a running timer, a spinner). The scenes below that carry the seeded tab have their log and
// their start and finish times normalised instead (see `normalise`).
//
// What the scenes pin so a picture is a picture of the app and not of the machine: the appearance and
// fonts, Reduce Motion on (the empty state's hive bobs forever; the grid gate has no such surface),
// every preference a Settings pane shows, and the engine's answers for the breadcrumb (`quiet`).
// What they cannot pin: the General pane prints the main bundle's version, which under `swift test`
// is the test runner's, so a new Xcode moves those few pixels. Dark scenes with an editor show
// identifiers in a dark ink, as the `editor-*-dark` baselines do (the text view's `labelColor` is
// resolved outside the window's appearance in a test process).
//
// Not capturable by `cacheDisplay`, and therefore not here: the window's native toolbar and title
// bar, tooltips, menus, popovers, and a sheet presented over the window. The connection sheet is
// drawn directly for that reason, the way the grid gate draws the cell reader.

@MainActor
final class ChromeParityTests: XCTestCase {
    private enum Look: String, CaseIterable {
        case dark, light
        var theme: AppTheme { self == .dark ? .midnight : .daylight }
        var mode: AppearanceMode { self == .dark ? .dark : .light }
        var scheme: ColorScheme { self == .dark ? .dark : .light }
        var appearance: NSAppearance { NSAppearance(named: self == .dark ? .darkAqua : .aqua)! }
    }

    private enum Surface {
        /// The whole window, as `--snapshot` draws it.
        case shell
        case settings(SettingsView.Pane)
        /// The connection sheet on its own: a sheet is a window of its own and `cacheDisplay` cannot see it.
        case connectionSheet

        var size: CGSize {
            switch self {
            case .shell: CGSize(width: 1240, height: 800)
            case .settings: CGSize(width: 560, height: 640)
            case .connectionSheet: CGSize(width: 620, height: 660)
            }
        }
    }

    /// `name` is the baseline's name (`chrome-<name>-<look>`); `seed` is the `--snapshot` scene it is drawn from.
    private struct Scene {
        let name: String
        let seed: String
        let surface: Surface
    }

    private static let scenes: [Scene] = [
        Scene(name: "empty", seed: "empty", surface: .shell),
        Scene(name: "fresh-tab", seed: "fresh-tab", surface: .shell),
        Scene(name: "groups", seed: "groups", surface: .shell),
        Scene(name: "tree-large", seed: "tree-large", surface: .shell),
        Scene(name: "tree-empty-schema", seed: "tree-empty-schema", surface: .shell),
        Scene(name: "tree-databases", seed: "tree-databases", surface: .shell),
        Scene(name: "quickly", seed: "quickly", surface: .shell),
        Scene(name: "cascade-long", seed: "cascade-long", surface: .shell),
        Scene(name: "cascade-postgres", seed: "cascade-postgres", surface: .shell),
        Scene(name: "cascade-mysql", seed: "cascade-mysql", surface: .shell),
        Scene(name: "objects", seed: "objects", surface: .shell),
        Scene(name: "settings", seed: "settings", surface: .settings(.general)),
        Scene(name: "settings-editor", seed: "settings-editor", surface: .settings(.editor)),
        Scene(name: "settings-data", seed: "settings-data", surface: .settings(.data)),
        Scene(name: "settings-keyboard", seed: "settings-keyboard", surface: .settings(.keyboard)),
        Scene(name: "connection", seed: "connection", surface: .connectionSheet),
    ]

    private struct Capture {
        var png: Data
        var sidecar: Sidecar
    }

    /// `nil` compares. `[]` records every scene; a list records those.
    private static let recording: Set<String>? = {
        guard let value = ProcessInfo.processInfo.environment["QH_RECORD_CHROME"], !value.isEmpty else { return nil }
        return value == "1" || value == "all" ? [] : Set(value.split(separator: ",").map(String.init))
    }()

    /// `__Baselines__` beside this file, so the path is the repository's and not the build's.
    private static let baselineDirectory = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().appendingPathComponent("__Baselines__", isDirectory: true)

    /// The keys the model reads when it is built. The Settings panes show them, and another test in
    /// the same process may have left them anywhere, so they are put back to "never written" for the run.
    private static let modelDefaults = ["shortcutScheme", AppModel.restoreTabsKey, "historyLimit",
                                        "recordsHistory", "statementTimeoutMS", "defaultRowLimit"]

    private var savedTheme: (dark: AppTheme, light: AppTheme, accent: AccentChoice, tone: SurfaceTone,
                             mode: AppearanceMode, systemIsDark: Bool, ui: String, code: String)!
    private var savedData: (rows: DataPreferences.RowHeight, alt: Bool, numbers: Bool, inspector: Bool,
                            null: String, viewer: DataPreferences.ViewerMode,
                            sort: GridSort.Direction, font: Int)!
    private var savedEditor: EditorLayout!
    private var savedQueryParameters = true
    private var savedDefaults: [String: Any] = [:]
    private var windows: [NSWindow] = []

    override func setUp() {
        super.setUp()
        isolateConnectionStore()
        let store = ThemeStore.shared
        savedTheme = (store.darkTheme, store.lightTheme, store.accent, store.tone, store.mode,
                      store.systemIsDark, store.uiFontFamily, store.codeFontFamily)
        let data = DataPreferences.shared
        savedData = (data.rowHeight, data.alternateRows, data.showRowNumbers, data.autoShowInspector,
                     data.nullDisplay, data.viewerMode, data.firstSortDirection, data.gridFontSize)
        savedEditor = EditorLayout.current
        savedQueryParameters = EditorPreferences.shared.queryParameters
        for key in Self.modelDefaults {
            if let value = UserDefaults.standard.object(forKey: key) { savedDefaults[key] = value }
            UserDefaults.standard.removeObject(forKey: key)
        }
    }

    override func tearDown() {
        closeWindows()
        ThemeStore.shared.unpinSurface()
        let s = savedTheme!
        ThemeStore.shared.pin(theme: s.dark, accent: s.accent, tone: s.tone, mode: s.mode,
                              systemIsDark: s.systemIsDark, uiFont: s.ui, codeFont: s.code)
        ThemeStore.shared.pin(theme: s.light)
        let d = savedData!
        DataPreferences.shared.pin(rowHeight: d.rows, showRowNumbers: d.numbers, autoShowInspector: d.inspector)
        DataPreferences.shared.alternateRows = d.alt
        DataPreferences.shared.nullDisplay = d.null
        DataPreferences.shared.viewerMode = d.viewer
        DataPreferences.shared.firstSortDirection = d.sort
        DataPreferences.shared.gridFontSize = d.font
        setEditor(savedEditor, queryParameters: savedQueryParameters)
        for key in Self.modelDefaults {
            if let value = savedDefaults[key] { UserDefaults.standard.set(value, forKey: key) }
            else { UserDefaults.standard.removeObject(forKey: key) }
        }
        super.tearDown()
    }

    private func closeWindows() {
        for window in windows {
            window.isReleasedWhenClosed = false
            window.contentView = nil
            window.close()
        }
        windows = []
    }

    // MARK: Scene setup

    /// Fixed appearance, fixed fonts, and every preference a pane shows at its default. `pin` never
    /// writes the user's preferences; the editor's setters do, and tearDown puts them back.
    private func applyState(_ look: Look) {
        // Reduce Motion on: the empty state's hive bobs forever and a running tab's glyph pulses, and a
        // picture that moves is not a baseline. The grid gate has no such surface, so it leaves it off.
        ThemeStore.shared.pin(reduceMotion: true, reduceTransparency: false, increaseContrast: false)
        ThemeStore.shared.pin(theme: look.theme, accent: .ice, tone: .glow, mode: look.mode,
                              systemIsDark: look == .dark, uiFont: "", codeFont: "")
        let data = DataPreferences.shared
        data.pin(rowHeight: .normal, showRowNumbers: true, autoShowInspector: false)
        data.alternateRows = true
        data.nullDisplay = "null"
        data.viewerMode = .automatic
        data.firstSortDirection = .ascending
        data.gridFontSize = DataPreferences.standardGridFontSize
        setEditor(.standard, queryParameters: true)
    }

    private func setEditor(_ layout: EditorLayout, queryParameters: Bool) {
        let prefs = EditorPreferences.shared
        prefs.showLineNumbers = layout.showLineNumbers
        prefs.highlightCurrentLine = layout.highlightCurrentLine
        prefs.highlightCurrentStatement = layout.highlightCurrentStatement
        prefs.wordWrap = layout.wordWrap
        prefs.codeFolding = layout.codeFolding
        prefs.showInvisibles = layout.showInvisibles
        prefs.autoUppercaseKeywords = layout.autoUppercaseKeywords
        prefs.runButtonPerStatement = layout.runButtonPerStatement
        prefs.tabWidth = layout.tabWidth
        prefs.fontSize = layout.fontSize
        prefs.queryParameters = queryParameters
    }

    /// The seeded tab carries a log whose lines and finish time are relative to now. The picture
    /// must not be: the lines go, the times become fixed ones, and the panel that showed the log
    /// shows the result instead (the blueprint's `tab.logLines = []`, `tab.panel = .result`).
    private func normalise(_ model: AppModel) {
        let start = Date(timeIntervalSinceReferenceDate: 0)
        for tab in model.tabs {
            tab.logLines = []
            tab.startedAt = start
            tab.finishedAt = start.addingTimeInterval(42)
            if tab.panel == .log { tab.panel = .result }
        }
    }

    /// A picture of a fixture must not depend on a server that is not there. The breadcrumb asks the
    /// engine for the catalogs and schemas when it appears, and the answer (a refused connection)
    /// lands at whatever moment the network gives up, so the loading mark would be in one draw and
    /// not in the next. Both loaders return before the engine when they already hold an answer, so
    /// each connection is given an empty one. The lists only fill menus, which are not drawn.
    private func quiet(_ model: AppModel) {
        for connection in model.connections {
            model.catalogOptions[connection.id] = []
            model.schemaOptions["\(connection.id.uuidString)|"] = []
        }
        for tab in model.tabs {
            guard let id = tab.connectionID else { continue }
            model.schemaOptions["\(id.uuidString)|\(model.database(for: tab))"] = []
        }
    }

    // MARK: Rendering (copied from VisualParityTests, where they are private)

    private func host(_ content: some View, size: CGSize, look: Look) -> NSHostingView<AnyView> {
        let root = AnyView(content
            .frame(width: size.width, height: size.height)
            .background(Tone.canvas)
            .preferredColorScheme(look.scheme))
        let host = NSHostingView(rootView: root)
        host.frame = CGRect(origin: .zero, size: size)
        let window = NSWindow(contentRect: host.frame, styleMask: [.titled], backing: .buffered,
                              defer: false)
        window.appearance = look.appearance
        window.contentView = host
        windows.append(window)
        settle(host)
        return host
    }

    /// The scroller style is a per-machine setting, and a legacy one takes width out of the layout.
    private static func forceOverlayScrollers(_ view: NSView) {
        if let scroll = view as? NSScrollView, scroll.scrollerStyle != .overlay { scroll.scrollerStyle = .overlay }
        for subview in view.subviews { forceOverlayScrollers(subview) }
    }

    /// Wait until the drawn picture is identical for six turns in a row (0.18 s). There is no fixed
    /// sleep to outrun on a slow machine, and the picture compared is the one that is recorded.
    private func settle(_ host: NSView, file: StaticString = #filePath, line: UInt = #line) {
        var previous: Data?
        var stable = 0
        let deadline = Date().addingTimeInterval(20)
        while stable < 6, Date() < deadline {
            Self.forceOverlayScrollers(host)
            RunLoop.current.run(until: Date().addingTimeInterval(0.03))
            host.layoutSubtreeIfNeeded()
            guard let rep = try? bitmap(of: host) else { break }
            let bytes = rep.bitmapData.map { Data(bytes: $0, count: rep.bytesPerRow * rep.pixelsHigh) }
            stable = bytes == previous ? stable + 1 : 0
            previous = bytes
        }
        if stable < 6 { XCTFail("the view did not stop changing within 20 s", file: file, line: line) }
    }

    /// An sRGB bitmap of the view at 1x, whatever the machine's display is.
    private func bitmap(of host: NSView) throws -> NSBitmapImageRep {
        let w = Int(host.bounds.width), h = Int(host.bounds.height)
        let device = try XCTUnwrap(NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: w, pixelsHigh: h, bitsPerSample: 8,
            samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
            bytesPerRow: 0, bitsPerPixel: 0))
        // Tagged before anything is drawn, so AppKit converts into sRGB instead of leaving the
        // machine's device space in the file.
        let rep = try XCTUnwrap(device.retagging(with: .sRGB))
        XCTAssertEqual(rep.colorSpace, NSColorSpace.sRGB)
        rep.size = host.bounds.size
        host.cacheDisplay(in: host.bounds, to: rep)
        return rep
    }

    private func png(of host: NSView) throws -> Data {
        try XCTUnwrap(bitmap(of: host).representation(using: .png, properties: [:]))
    }

    // MARK: Capture

    private func draw(_ scene: Scene, look: Look) throws -> Capture {
        applyState(look)
        let model = Snapshot.seeded(scene: scene.seed)
        normalise(model)
        quiet(model)
        let content: AnyView
        switch scene.surface {
        case .shell:
            content = AnyView(RootView().environment(model))
        case .settings(let pane):
            content = AnyView(SettingsView(pane: pane).environment(model))
        case .connectionSheet:
            let target = try XCTUnwrap(model.editingConnection, "\(scene.name): the scene opens no editor")
            // The sheet is drawn on its own, so the model must not also present it over a shell.
            model.editingConnection = nil
            content = AnyView(ConnectionEditorSheet(target: target).environment(model))
        }
        let seededSaved = model.savedQueries
        let view = host(content, size: scene.surface.size, look: look)
        if !seededSaved.isEmpty {
            // The sidebar asks the local engine for the saved queries when it appears, and the
            // answer (none, in an empty store) replaces the scene's. Wait for it, then put the
            // scene's back: what the scene means to show is the palette with them.
            let ids = seededSaved.map(\.id)
            let landed = Date().addingTimeInterval(10)
            while model.savedQueries.map(\.id) == ids, Date() < landed {
                RunLoop.current.run(until: Date().addingTimeInterval(0.02))
            }
            model.savedQueries = seededSaved
            settle(view)
        }
        let data = try png(of: view)
        let pixels = try XCTUnwrap(Pixels(png: data))
        XCTAssertEqual([pixels.width, pixels.height], [Int(scene.surface.size.width), Int(scene.surface.size.height)],
                       "\(scene.name): the window is not the size the scene asks for")
        return Capture(png: data, sidecar: Sidecar(
            scene: "chrome-\(scene.name)-\(look.rawValue)", pixelSize: [pixels.width, pixels.height],
            facts: ["scene": scene.seed, "look": look.rawValue, "reduceMotion": "true",
                    "size": "\(pixels.width)x\(pixels.height)"],
            metrics: [:], samples: []))
    }

    // MARK: Record / compare

    func testChromeScenesMatchTheirBaselines() throws {
        let directory = Self.baselineDirectory
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        var recorded = 0
        for scene in Self.scenes {
            if let wanted = Self.recording, !wanted.isEmpty, !wanted.contains(scene.name) { continue }
            for look in Look.allCases {
                let name = "chrome-\(scene.name)-\(look.rawValue)"
                let pngURL = directory.appendingPathComponent("\(name).png")
                let jsonURL = directory.appendingPathComponent("\(name).json")
                let capture = try draw(scene, look: look)
                closeWindows()

                if Self.recording != nil {
                    // Twice, and identical: a scene that cannot repeat itself has no baseline to give.
                    let again = try draw(scene, look: look)
                    closeWindows()
                    guard again.png == capture.png else {
                        XCTFail("[\(name)] two draws of the same scene differ, so it is not recorded")
                        continue
                    }
                    try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
                    try capture.png.write(to: pngURL)
                    try encoder.encode(capture.sidecar).write(to: jsonURL)
                    recorded += 1
                    continue
                }

                guard let basePNG = try? Data(contentsOf: pngURL), let baseJSON = try? Data(contentsOf: jsonURL),
                      let baseline = try? JSONDecoder().decode(Sidecar.self, from: baseJSON),
                      let basePixels = Pixels(png: basePNG) else {
                    XCTFail("[\(name)] no baseline. Record with QH_RECORD_CHROME=1 (a new scene, or a declared V-n change)")
                    continue
                }
                guard let actualPixels = Pixels(png: capture.png) else {
                    XCTFail("[\(name)] the fresh render could not be decoded")
                    continue
                }
                let failures = ParityComparator.compare(baseline: baseline, baselinePixels: basePixels,
                                                        actual: capture.sidecar, actualPixels: actualPixels)
                for failure in failures { XCTFail("[\(name)] \(failure)") }
                if !failures.isEmpty, let dump = ProcessInfo.processInfo.environment["QH_RENDER_DIR"], !dump.isEmpty {
                    // The picture that failed, so it can be looked at.
                    let out = URL(fileURLWithPath: dump, isDirectory: true)
                    try? FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
                    try? capture.png.write(to: out.appendingPathComponent("\(name).actual.png"))
                }
            }
        }
        if Self.recording != nil {
            print("chrome parity: recorded \(recorded) scenes into \(directory.path)")
            XCTFail("recorded \(recorded) scenes; rerun without QH_RECORD_CHROME")
        }
    }
}
