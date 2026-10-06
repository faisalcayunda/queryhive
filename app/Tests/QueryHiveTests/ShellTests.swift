import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

// The window shell (W9-T8, blueprint w9-shell-and-a11y §11): a native split view around the same
// surfaces, with the tab strip in the title bar's own row. The pictures of it are `ChromeParityTests`
// and `--snapshot`, which draw the window with its title bar; what these tests hold is the structure
// that a picture cannot say: the tab strip is the top row and the traffic lights are in it without
// touching a tab, the query row (breadcrumb and Run group) is under it and fits at the window's
// minimum width, the title is set and not drawn, the empty part of the strip drags the window, the
// sidebar toggle reaches the split view, sheets still attach to the window, and in every theme the
// top row is that theme's own surface (the colour sampled from the window's bitmap), never a system
// grey.
//
// Minimum width (probe P-8c, measured when the breadcrumb and the Run group were the window's
// toolbar, with the widest content the app can draw, scene `shell-narrow`: a Trino connection with
// three 200 pt levels and both badges, sidebar at its ideal 264 pt) is 1400 pt. They are a row in
// the workspace now, which needs less (`testTheQueryRowFitsAtTheMinimumWidth` prints the slack), and
// the number stays: it is a product decision, and the toolbar's overflow menu is gone, not the width.
@MainActor
final class ShellTests: XCTestCase {
    private var windows: [NSWindow] = []

    override func setUp() {
        super.setUp()
        isolateConnectionStore()
        ThemeStore.shared.pin(reduceMotion: true, reduceTransparency: false, increaseContrast: false)
        ThemeStore.shared.pin(theme: .midnight, accent: .ice, tone: .glow, mode: .dark,
                              systemIsDark: true, uiFont: "", codeFont: "")
    }

    override func tearDown() {
        for window in windows {
            window.isReleasedWhenClosed = false
            window.contentView = nil
            window.close()
        }
        windows = []
        ThemeStore.shared.unpinSurface()
        // The themes the tests drew are not what the next test class finds.
        ThemeStore.shared.pin(theme: .midnight, accent: .ice, tone: .glow, mode: .dark,
                              systemIsDark: true, uiFont: "", codeFont: "")
        super.tearDown()
    }

    // MARK: Rig

    /// The window as the app has it: the hosting view hands its toolbar and title to the window
    /// (`sceneBridgingOptions`), and the content runs under the title bar.
    private struct Rig {
        let model: AppModel
        let window: NSWindow
        let host: NSHostingView<AnyView>
    }

    private func rig(scene: String, width: CGFloat = Shell.minWidth, height: CGFloat = 800,
                     theme: AppTheme = .midnight, tone: SurfaceTone = .glow,
                     adjust: (AppModel) -> Void = { _ in }) -> Rig {
        // The mode and the theme go in together: a light theme in a dark mode would be redirected.
        ThemeStore.shared.pin(theme: theme, accent: .ice, tone: tone, mode: theme.isDark ? .dark : .light,
                              systemIsDark: theme.isDark, uiFont: "", codeFont: "")
        let model = Snapshot.seeded(scene: scene)
        // A fixture must not ask a server that is not there: the breadcrumb loads its levels when it
        // appears, and the answer would land at whatever moment the network gives up.
        for connection in model.connections {
            model.catalogOptions[connection.id] = []
            model.schemaOptions["\(connection.id.uuidString)|"] = []
        }
        for tab in model.tabs {
            guard let id = tab.connectionID else { continue }
            model.schemaOptions["\(id.uuidString)|\(model.database(for: tab))"] = []
        }
        adjust(model)
        let scheme: ColorScheme = theme.isDark ? .dark : .light
        let host = NSHostingView(rootView: AnyView(RootView().environment(model)
            .frame(width: width, height: height).preferredColorScheme(scheme)))
        host.sceneBridgingOptions = [.toolbars, .title]
        host.frame = CGRect(x: 0, y: 0, width: width, height: height)
        let window = NSWindow(contentRect: host.frame,
                              styleMask: [.titled, .closable, .resizable, .fullSizeContentView],
                              backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: theme.isDark ? .darkAqua : .aqua)
        window.contentView = host
        window.orderFrontRegardless()
        windows.append(window)
        return Rig(model: model, window: window, host: host)
    }

    private func spin(_ seconds: Double = 0.6) {
        RunLoop.current.run(until: Date().addingTimeInterval(seconds))
    }

    /// Runs the loop until `condition` holds or `timeout` passes; the answer is whether it held.
    @discardableResult
    private func wait(_ timeout: Double = 5, until condition: () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if condition() { return true }
            RunLoop.current.run(until: Date().addingTimeInterval(0.05))
        }
        return condition()
    }

    /// The items this app put on the toolbar, not the sidebar toggle, the separator and the spaces
    /// the system adds (their identifiers start with `NS` or `com.apple`).
    private func appItems(_ window: NSWindow, visibleOnly: Bool = false) -> [NSToolbarItem] {
        let items = (visibleOnly ? window.toolbar?.visibleItems : window.toolbar?.items) ?? []
        return items.filter {
            !($0.itemIdentifier.rawValue.hasPrefix("NS") || $0.itemIdentifier.rawValue.hasPrefix("com.apple"))
        }
    }

    private func descendants<T: NSView>(_ type: T.Type, in view: NSView) -> [T] {
        var found: [T] = []
        if let match = view as? T { found.append(match) }
        for subview in view.subviews { found += descendants(type, in: subview) }
        return found
    }

    /// The window's frame view, which holds the title bar and the toolbar as well as the content.
    private func frameView(_ window: NSWindow) -> NSView { window.contentView!.superview! }

    /// Where a `FrameMarker` landed, in window coordinates (origin at the bottom left); `nil` while
    /// SwiftUI has no such view.
    private func frame(_ name: String, in rig: Rig) -> CGRect? {
        descendants(NSView.self, in: rig.host)
            .first { $0.identifier?.rawValue == name }
            .map { $0.convert($0.bounds, to: nil) }
    }

    private func requireFrame(_ name: String, in rig: Rig, file: StaticString = #filePath, line: UInt = #line) throws -> CGRect {
        XCTAssertTrue(wait { self.frame(name, in: rig) != nil }, "no view named \(name)", file: file, line: line)
        return try XCTUnwrap(frame(name, in: rig), name, file: file, line: line)
    }

    /// The traffic lights, in window coordinates.
    private func windowButtons(_ window: NSWindow) -> [CGRect] {
        [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton].compactMap {
            window.standardWindowButton($0).map { $0.convert($0.bounds, to: nil) }
        }
    }

    /// The system's toolbar items (the sidebar toggle) that have a view, in window coordinates.
    private func toolbarItemFrames(_ window: NSWindow) -> [CGRect] {
        (window.toolbar?.items ?? []).compactMap { $0.view.map { $0.convert($0.bounds, to: nil) } }
    }

    /// The height of the title bar: what the window is taller than its layout rect.
    private func titlebarHeight(_ window: NSWindow) -> CGFloat {
        window.frame.height - window.contentLayoutRect.maxY
    }

    // MARK: Title

    func testTheTitleIsTheTabAndTheSubtitleIsWhereItRuns() {
        let narrow = Snapshot.seeded(scene: "shell-narrow")
        let title = WindowTitle.make(model: narrow)
        XCTAssertEqual(title.title, "penerima_manfaat")
        XCTAssertEqual(title.subtitle,
                       "datawarehouse-main-pusdatin-masked · aktivitas-produksi-harian.sandbox · PROD · Confirm")

        let plain = WindowTitle.make(model: Snapshot.seeded(scene: "done"))
        XCTAssertEqual(plain.subtitle, "Trino production · hive.analytics", "no badge for an unrestricted connection")
    }

    func testAnEmptyWorkspaceIsTitledWithTheApp() {
        let model = Snapshot.seeded(scene: "empty")
        XCTAssertNil(model.selectedTab)
        XCTAssertEqual(WindowTitle.make(model: model), WindowTitle(title: "QueryHive", subtitle: ""))
    }

    func testTheWindowCarriesTheTitleAndTheSubtitle() throws {
        let rig = rig(scene: "done")
        XCTAssertTrue(wait { rig.window.title == "penerima_manfaat" }, "title: \(rig.window.title)")
        XCTAssertEqual(rig.window.subtitle, "Trino production · hive.analytics")
        XCTAssertNotNil(rig.window.toolbar, "sceneBridgingOptions installed no toolbar")
    }

    // The window is found by the view in it, because its title is the tab's name.
    func testTheMainWindowIsTheOneTheShellLivesIn() {
        let rig = rig(scene: "done")
        XCTAssertTrue(wait { MainWindow.isMain(rig.window) })
        let other = NSWindow(contentRect: .init(x: 0, y: 0, width: 100, height: 100), styleMask: [.titled],
                             backing: .buffered, defer: false)
        windows.append(other)
        XCTAssertFalse(MainWindow.isMain(other))
        XCTAssertFalse(MainWindow.isMain(nil))
    }

    // MARK: Title bar

    // The tab strip is the title bar's row: the window's top edge is the strip's, and the traffic
    // lights are vertically inside it.
    func testTheTabStripIsTheTitleBarRow() throws {
        let rig = rig(scene: "done")
        let strip = try requireFrame("tab-strip", in: rig)
        let bar = titlebarHeight(rig.window)
        XCTAssertGreaterThan(bar, 20, "the window has a title bar")
        XCTAssertEqual(rig.window.frame.height - strip.maxY, 0, accuracy: 1, "the strip starts at the window's top edge")
        XCTAssertGreaterThanOrEqual(strip.height + 1, bar, "the strip is at least as tall as the title bar")
        XCTAssertEqual(strip.minX, 264, accuracy: 1, "the strip is the detail column, from the sidebar's edge")
        for button in windowButtons(rig.window) {
            XCTAssertGreaterThanOrEqual(button.minY, strip.minY, "a traffic light sits in the strip's row")
            XCTAssertLessThanOrEqual(button.maxY, strip.maxY)
        }
        // The query row is directly under it.
        let row = try requireFrame("query-toolbar", in: rig)
        XCTAssertEqual(row.maxY, strip.minY, accuracy: 1, "the context row is below the tabs")
        XCTAssertEqual(row.height, Metrics.toolbar, accuracy: 1)
    }

    // No toolbar title or subtitle is drawn (the window keeps both for the Window menu and
    // VoiceOver), and the system's toolbar holds nothing of the app's.
    func testNoTitleIsDrawnAndTheToolbarHoldsNothingOfTheApps() throws {
        let rig = rig(scene: "done")
        XCTAssertTrue(wait { rig.window.title == "penerima_manfaat" })
        XCTAssertEqual(rig.window.subtitle, "Trino production · hive.analytics", "set, for the Window menu and VoiceOver")
        XCTAssertEqual(rig.window.titleVisibility, .hidden)
        spin(0.5)
        // The title bar's own text fields are outside the hosting view; none of them may be showing.
        let drawn = descendants(NSTextField.self, in: frameView(rig.window))
            .filter { field in
                !field.isHidden && !field.stringValue.isEmpty && !field.isDescendant(of: rig.host)
            }
            .map(\.stringValue)
        XCTAssertEqual(drawn, [], "the title bar draws text")
        XCTAssertEqual(appItems(rig.window).count, 0)
    }

    // The traffic lights and the sidebar toggle, with the sidebar hidden, are over the strip's own
    // column: the tabs start after them, however many there are.
    func testTabsStayClearOfTheWindowControls() throws {
        let rig = rig(scene: "done")
        rig.model.navigation.sidebarHidden = true
        for _ in 0..<14 { rig.model.newTab() }
        XCTAssertTrue(wait { self.frame("tab-strip", in: rig)?.minX == 0 }, "the strip did not take the window's width")
        let tabs = try requireFrame("tab-strip.tabs", in: rig)
        let controls = windowButtons(rig.window) + toolbarItemFrames(rig.window)
        XCTAssertEqual(windowButtons(rig.window).count, 3)
        XCTAssertFalse(toolbarItemFrames(rig.window).isEmpty, "the sidebar toggle has a view")
        for control in controls {
            XCTAssertFalse(control.intersects(tabs), "\(control) is over the tabs \(tabs)")
            XCTAssertLessThanOrEqual(control.maxX, tabs.minX)
        }
        // And shown again: the controls are over the sidebar, and the strip is the detail column.
        rig.model.navigation.sidebarHidden = false
        XCTAssertTrue(wait { self.frame("tab-strip", in: rig)?.minX ?? 0 > 200 })
        let strip = try requireFrame("tab-strip", in: rig)
        for control in windowButtons(rig.window) + toolbarItemFrames(rig.window) {
            XCTAssertLessThanOrEqual(control.maxX, strip.minX, "\(control) is over the strip, not the sidebar")
        }
    }

    // The gesture that drags the window and answers a double-click covers the whole strip, and the
    // tabs hug their own width, so there is empty strip for it to answer in, with one tab or many.
    // (The event path itself, `performDrag` and `performZoom` on a mouse down, needs an active app
    // and a real pointer, which a test process does not have.)
    func testTheDragAreaCoversTheStripAndTheTabsLeaveRoomInIt() throws {
        let rig = rig(scene: "done")
        let strip = try requireFrame("tab-strip", in: rig)
        let drag = try requireFrame("tab-strip.drag", in: rig)
        XCTAssertEqual(drag, strip, "the drag area is the whole strip")
        let tabs = try requireFrame("tab-strip.tabs", in: rig)
        XCTAssertGreaterThan(strip.maxX - tabs.maxX, 400, "one tab leaves most of the strip empty")
        rig.model.newTab()
        rig.model.newTab()
        XCTAssertTrue(wait { (self.frame("tab-strip.tabs", in: rig)?.width ?? 0) > tabs.width + 100 })
        let three = try requireFrame("tab-strip.tabs", in: rig)
        XCTAssertGreaterThan(strip.maxX - three.maxX, 100, "three tabs still leave empty strip")
    }

    func testADoubleClickDoesWhatTheSystemSettingSays() {
        XCTAssertEqual(TitleBarDoubleClick(nil), .zoom, "unset means zoom")
        XCTAssertEqual(TitleBarDoubleClick("Maximize"), .zoom)
        XCTAssertEqual(TitleBarDoubleClick("Minimize"), .minimise)
        XCTAssertEqual(TitleBarDoubleClick("None"), .nothing)
    }

    // MARK: Query row

    func testTheQueryRowIsThereForAQueryAndOnlyThen() throws {
        let rig = rig(scene: "done")
        _ = try requireFrame("query-toolbar", in: rig)
        // The editor is hidden behind the panel, so the row it belongs to goes with it.
        rig.model.panelExpanded = true
        XCTAssertTrue(wait { self.frame("query-toolbar", in: rig) == nil }, "the row stayed over a full-height panel")
        rig.model.panelExpanded = false
        _ = try requireFrame("query-toolbar", in: rig)

        let empty = self.rig(scene: "empty")
        spin()
        XCTAssertNil(frame("query-toolbar", in: empty), "no tab, nothing to run")
        XCTAssertNotNil(frame("tab-strip", in: empty), "the strip is there with no tab: it is the title bar")
        // An object tab holds no query to run.
        let objects = self.rig(scene: "objects")
        spin()
        XCTAssertTrue(objects.model.selectedTab?.isObjects == true)
        XCTAssertNil(frame("query-toolbar", in: objects))
    }

    func testTheWindowTitleFollowsTheSelectedTab() {
        let rig = rig(scene: "done")
        XCTAssertTrue(wait { rig.window.title == "penerima_manfaat" })
        rig.model.newTab()
        XCTAssertTrue(wait { rig.window.title == rig.model.selectedTab?.title }, "title: \(rig.window.title)")
    }

    // MARK: Minimum width (P-8c)

    /// The breadcrumb and the Run group, in the row, never touch each other or the row's edge. The
    /// widest content the app can draw: three 200 pt levels and both badges, scene `shell-narrow`.
    func testTheQueryRowFitsAtTheMinimumWidth() throws {
        for title in [nil, String(repeating: "penerima_manfaat_", count: 8)] {
            let rig = rig(scene: "shell-narrow", width: Shell.minWidth) { model in
                if let title { model.selectedTab?.title = title }
            }
            let row = try requireFrame("query-toolbar", in: rig)
            let cascade = try requireFrame("query-toolbar.cascade", in: rig)
            let actions = try requireFrame("query-toolbar.actions", in: rig)
            let slack = actions.minX - cascade.maxX
            print("QueryHive: the query row's slack at \(Shell.minWidth) pt: \(slack) pt")
            XCTAssertGreaterThan(slack, 12, "the breadcrumb runs into the Run group (\(title == nil ? "short" : "long") title)")
            XCTAssertGreaterThanOrEqual(cascade.minX, row.minX)
            XCTAssertLessThanOrEqual(actions.maxX, row.maxX)
            XCTAssertGreaterThan(actions.width, 200, "the Run group is squeezed")
        }
    }

    // MARK: Themes

    /// A colour from the window's bitmap at `x` points from the left and `yTop` points from the top,
    /// as the bitmap holds it. Not converted to sRGB: a bitmap that cannot say which display it was
    /// drawn for is converted with a profile that lifts every dark colour by 10 to 24 levels, so the
    /// expected colours are drawn through the same path (`reference`) and compared as they are.
    private func pixel(_ bitmap: NSBitmapImageRep, _ window: NSWindow, x: CGFloat, yTop: CGFloat) -> [CGFloat] {
        let scale = CGFloat(bitmap.pixelsWide) / window.frame.width
        return components(bitmap.colorAt(x: Int(x * scale), y: Int(yTop * scale))!)
    }

    private func components(_ colour: NSColor) -> [CGFloat] {
        var (r, g, b, a): (CGFloat, CGFloat, CGFloat, CGFloat) = (0, 0, 0, 0)
        colour.getRed(&r, green: &g, blue: &b, alpha: &a)
        return [r, g, b]
    }

    private func capture(_ window: NSWindow) throws -> NSBitmapImageRep {
        let frame = frameView(window)
        let bitmap = try XCTUnwrap(frame.bitmapImageRepForCachingDisplay(in: frame.bounds))
        frame.cacheDisplay(in: frame.bounds, to: bitmap)
        return bitmap
    }

    /// What `content` looks like through the same capture path as the window: drawn alone, in a
    /// window of the theme's appearance, and read from the middle.
    private func reference<V: View>(_ content: V, theme: AppTheme) throws -> [CGFloat] {
        let size = CGSize(width: 64, height: 64)
        let host = NSHostingView(rootView: content.frame(width: size.width, height: size.height)
            .preferredColorScheme(theme.isDark ? .dark : .light))
        host.frame = CGRect(origin: .zero, size: size)
        let window = NSWindow(contentRect: host.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: theme.isDark ? .darkAqua : .aqua)
        window.contentView = host
        window.orderFrontRegardless()
        spin(0.3)
        let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: bitmap)
        let scale = CGFloat(bitmap.pixelsWide) / size.width
        let colour = components(try XCTUnwrap(bitmap.colorAt(x: Int(size.width / 2 * scale), y: Int(size.height / 2 * scale))))
        window.isReleasedWhenClosed = false
        window.contentView = nil
        window.close()
        return colour
    }

    /// The largest channel difference, in 0...1.
    private func distance(_ a: [CGFloat], _ b: [CGFloat]) -> CGFloat {
        zip(a, b).map { abs($0 - $1) }.max() ?? 1
    }

    // The tolerance is a few levels of 255, for the rounding in the compositing; a system toolbar
    // material is 8 to 40 levels off in every theme (F7F7F7 under Daylight, Cloud and Paper alike,
    // 141414 under Ink).
    private static let tolerance: CGFloat = 3.0 / 255

    // Flat tone, so the backdrop has no glow: the top row, the title bar's area above the
    // workspace, is the theme's canvas exactly, and the sidebar's top is the theme's sidebar surface,
    // the same colour as the rest of the sidebar.
    func testEveryThemeDrawsTheTopRowInItsOwnSurface() throws {
        for theme in AppTheme.allCases {
            let rig = rig(scene: "done", theme: theme, tone: .plain)
            let strip = try requireFrame("tab-strip", in: rig)
            _ = try requireFrame("query-toolbar", in: rig)
            spin(0.6)
            let bitmap = try capture(rig.window)
            let width = rig.window.frame.width
            let canvas = try reference(Tone.canvas, theme: theme)
            let sidebar = try reference(ZStack { Tone.canvas; Tone.ink.opacity(Tone.sidebarWash) }, theme: theme)
            XCTAssertGreaterThan(distance(canvas, sidebar), 6.0 / 255, "\(theme): the sidebar cannot be told from the workspace")

            // The title bar's area at the very top, right of the tabs, and the bottom of the strip's
            // row: both are the canvas, and the same colour.
            for x in [width * 0.62, width * 0.8, width - 40] {
                let top = pixel(bitmap, rig.window, x: x, yTop: 3)
                let low = pixel(bitmap, rig.window, x: x, yTop: strip.height - 5)
                XCTAssertLessThan(distance(top, canvas), Self.tolerance, "\(theme): the top of the window is \(top), not the canvas \(canvas)")
                XCTAssertLessThan(distance(low, canvas), Self.tolerance, "\(theme): the strip is \(low), not the canvas \(canvas)")
                XCTAssertLessThan(distance(top, low), Self.tolerance, "\(theme): a seam between the title bar's area and the strip")
            }
            // The context row under the tabs is the same surface: no band between the three rows.
            let row = try requireFrame("query-toolbar", in: rig)
            let gap = (try requireFrame("query-toolbar.cascade", in: rig).maxX + (try requireFrame("query-toolbar.actions", in: rig)).minX) / 2
            let context = pixel(bitmap, rig.window, x: gap, yTop: rig.window.frame.height - row.midY)
            XCTAssertLessThan(distance(context, canvas), Self.tolerance, "\(theme): the context row is \(context), not the canvas \(canvas)")
            // The sidebar: its top (the traffic lights' row) is its surface, and so is its body.
            for x: CGFloat in [150, 200] {
                let top = pixel(bitmap, rig.window, x: x, yTop: 3)
                let body = pixel(bitmap, rig.window, x: x, yTop: rig.window.frame.height - 60)
                XCTAssertLessThan(distance(top, sidebar), Self.tolerance, "\(theme): the sidebar's top is \(top), not its surface \(sidebar)")
                XCTAssertLessThan(distance(top, body), Self.tolerance, "\(theme): the sidebar's top is not its body")
            }
        }
    }

    // With the glow on (the default tone) the strip is the backdrop's own colour: it does not step
    // anywhere between the top of the window and the strip's hairline, in any theme.
    func testTheStripHasNoSeamUnderTheGlowEither() throws {
        for theme in AppTheme.allCases {
            let rig = rig(scene: "done", theme: theme)
            let strip = try requireFrame("tab-strip", in: rig)
            spin(0.6)
            let bitmap = try capture(rig.window)
            for x in [rig.window.frame.width * 0.7, rig.window.frame.width - 40] {
                var previous = pixel(bitmap, rig.window, x: x, yTop: 1)
                for y in stride(from: CGFloat(3), through: strip.height - 4, by: 2) {
                    let next = pixel(bitmap, rig.window, x: x, yTop: y)
                    XCTAssertLessThan(distance(previous, next), 3.0 / 255, "\(theme): a step at y=\(y) of the strip")
                    previous = next
                }
            }
            // `QH_THEME_PNGS=<dir>` also writes the window in each theme (with the sidebar shown and
            // hidden), for a person to look at.
            if let directory = ProcessInfo.processInfo.environment["QH_THEME_PNGS"] {
                try writePNG(bitmap, to: "\(directory)/chrome-\(theme.rawValue).png")
                rig.model.navigation.sidebarHidden = true
                spin(1.0)
                try writePNG(capture(rig.window), to: "\(directory)/chrome-\(theme.rawValue)-sidebar-hidden.png")
            }
        }
    }

    private func writePNG(_ bitmap: NSBitmapImageRep, to path: String) throws {
        try FileManager.default.createDirectory(atPath: (path as NSString).deletingLastPathComponent,
                                                withIntermediateDirectories: true)
        let png = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
        try png.write(to: URL(fileURLWithPath: path))
    }

    // MARK: Sidebar

    func testTheSidebarOpensAtItsIdealWidth() throws {
        let rig = rig(scene: "done")
        XCTAssertTrue(wait { !self.descendants(NSOutlineView.self, in: rig.host).isEmpty })
        spin()
        let outline = try XCTUnwrap(descendants(NSOutlineView.self, in: rig.host).first)
        XCTAssertEqual(outline.frame.width, Shell.sidebarIdeal, accuracy: 1)
    }

    func testHidingTheSidebarInTheModelCollapsesTheColumn() throws {
        let rig = rig(scene: "done")
        XCTAssertTrue(wait { !self.descendants(NSOutlineView.self, in: rig.host).isEmpty })
        spin()
        let outline = try XCTUnwrap(descendants(NSOutlineView.self, in: rig.host).first)
        XCTAssertGreaterThan(outline.visibleRect.width, 0)

        rig.model.toggleSidebar()
        XCTAssertTrue(wait { outline.visibleRect.width == 0 }, "the column did not collapse")
        rig.model.toggleSidebar()
        XCTAssertTrue(wait { outline.visibleRect.width > 0 }, "the column did not come back")
    }

    // `toggleSidebar:` is what the toolbar's own button, the system's View menu item and Full
    // Keyboard Access send down the responder chain. It has to reach the split view, and the model
    // (which the menu and the focus move read) has to hear about the result.
    func testToggleSidebarFromTheResponderChainReachesTheSplitViewAndTheModel() throws {
        let rig = rig(scene: "done")
        XCTAssertTrue(wait { !self.descendants(NSOutlineView.self, in: rig.host).isEmpty })
        spin()
        let outline = try XCTUnwrap(descendants(NSOutlineView.self, in: rig.host).first)
        XCTAssertTrue(rig.window.makeFirstResponder(outline))
        XCTAssertFalse(rig.model.navigation.sidebarHidden)

        XCTAssertTrue(outline.tryToPerform(Selector(("toggleSidebar:")), with: nil),
                      "nothing in the responder chain handles toggleSidebar:")
        XCTAssertTrue(wait { rig.model.navigation.sidebarHidden }, "the model did not follow the split view")
        XCTAssertTrue(wait { outline.visibleRect.width == 0 })
    }

    // MARK: Sheets (P-8d)

    func testASheetStillAttachesToTheWindow() {
        let rig = rig(scene: "done")
        spin()
        XCTAssertNil(rig.window.attachedSheet)
        rig.model.presentConnectionEditor(nil)
        XCTAssertTrue(wait { rig.window.attachedSheet != nil }, "the connection sheet did not attach")
    }

    // MARK: Panel resizer

    func testThePanelResizerStepsAndStaysInItsRange() {
        XCTAssertEqual(PanelResizer.adjusted(480, .increment), 504)
        XCTAssertEqual(PanelResizer.adjusted(480, .decrement), 456)
        XCTAssertEqual(PanelResizer.adjusted(555, .increment), PanelResizer.maxHeight)
        XCTAssertEqual(PanelResizer.adjusted(100, .decrement), PanelResizer.minHeight)
        XCTAssertEqual(PanelResizer.minHeight, 96)
        XCTAssertEqual(PanelResizer.maxHeight, 560)
    }

    func testAdjustingThePanelHeightOpensACollapsedPanel() {
        let model = Snapshot.seeded(scene: "done")
        model.panelHeight = 480
        model.panelCollapsed = true
        PanelResizer.adjust(model, .increment)
        XCTAssertEqual(model.panelHeight, 504)
        XCTAssertFalse(model.panelCollapsed, "the same as dragging the seam")
    }

    // MARK: Scenes

    /// Every scene that draws the shell renders in the bridged window: it has a toolbar, the content
    /// has the size it was asked for, and the picture of the whole window is not empty.
    func testTheChromeScenesRenderInTheBridgedWindow() throws {
        let scenes = ["done", "empty", "fresh-tab", "groups", "tree-large", "tree-empty-schema",
                      "tree-databases", "quickly", "cascade-long", "cascade-postgres", "cascade-mysql",
                      "objects", "badges", "shell-narrow"]
        for scene in scenes {
            let rig = rig(scene: scene, width: Shell.minWidth, height: 800)
            XCTAssertTrue(wait { rig.window.toolbar != nil }, "\(scene): no toolbar")
            spin(0.5)
            let frame = try XCTUnwrap(rig.window.contentView?.superview, "\(scene): no frame view")
            let bitmap = try XCTUnwrap(frame.bitmapImageRepForCachingDisplay(in: frame.bounds))
            frame.cacheDisplay(in: frame.bounds, to: bitmap)
            // The content runs under the title bar, so the window is the content plus the bar.
            XCTAssertEqual(rig.host.bounds.width, Shell.minWidth, scene)
            XCTAssertGreaterThanOrEqual(rig.host.bounds.height, 800, scene)
            // Not one flat colour: a window that drew nothing is one.
            var colours = Set<UInt32>()
            for y in stride(from: 0, to: bitmap.pixelsHigh, by: 37) {
                for x in stride(from: 0, to: bitmap.pixelsWide, by: 41) {
                    guard let colour = bitmap.colorAt(x: x, y: y)?.usingColorSpace(.sRGB) else { continue }
                    colours.insert(UInt32(colour.redComponent * 255) << 16
                                   | UInt32(colour.greenComponent * 255) << 8 | UInt32(colour.blueComponent * 255))
                }
            }
            XCTAssertGreaterThan(colours.count, 8, "\(scene): the window drew nothing")
        }
    }
}
