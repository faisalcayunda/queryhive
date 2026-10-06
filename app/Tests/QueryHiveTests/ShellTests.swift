import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

// The window shell (W9-T8, blueprint w9-shell-and-a11y §11): a native split view and toolbar around
// the same surfaces. The pictures of it are `ChromeParityTests` and `--snapshot`, which draw the
// window with its title bar and toolbar; what these tests hold is the structure that a picture
// cannot say: the toolbar is installed and holds the right items, nothing is pushed into its
// overflow menu at the window's minimum width, the sidebar toggle reaches the split view, and
// sheets still attach to the window.
//
// Minimum width (probe P-8c, measured with the widest toolbar the app can draw, scene
// `shell-narrow`: a Trino connection with three 200 pt levels and both badges, sidebar at its ideal
// 264 pt): every item is visible from 1384 pt and one is not at 1376 pt. `Shell.minWidth` is 1400.
// Without badges (`cascade-long`) the same toolbar fits from 1240 pt.
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
                     adjust: (AppModel) -> Void = { _ in }) -> Rig {
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
        let host = NSHostingView(rootView: AnyView(RootView().environment(model)
            .frame(width: width, height: height).preferredColorScheme(.dark)))
        host.sceneBridgingOptions = [.toolbars, .title]
        host.frame = CGRect(x: 0, y: 0, width: width, height: height)
        let window = NSWindow(contentRect: host.frame,
                              styleMask: [.titled, .closable, .resizable, .fullSizeContentView],
                              backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: .darkAqua)
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

    // MARK: Toolbar

    func testTheToolbarHoldsTheBreadcrumbAndTheRunGroup() {
        let rig = rig(scene: "done")
        XCTAssertTrue(wait { self.appItems(rig.window).count == 2 }, "items: \(appItems(rig.window).count)")
        XCTAssertTrue(wait { self.appItems(rig.window, visibleOnly: true).count == 2 },
                      "at the minimum width nothing may be in the overflow menu")
    }

    func testThereIsNoToolbarContentWithoutAQuery() {
        // No tab at all.
        let empty = rig(scene: "empty")
        spin()
        XCTAssertEqual(appItems(empty.window).count, 0, "no tab, nothing to run")
        // An object tab holds no query to run.
        let objects = rig(scene: "objects")
        spin()
        XCTAssertTrue(objects.model.selectedTab?.isObjects == true)
        XCTAssertEqual(appItems(objects.window).count, 0)
    }

    func testTheToolbarGoesWithTheEditorWhenThePanelFillsTheWindow() {
        let rig = rig(scene: "done")
        XCTAssertTrue(wait { self.appItems(rig.window).count == 2 })
        rig.model.panelExpanded = true
        XCTAssertTrue(wait { self.appItems(rig.window).isEmpty }, "the editor is hidden, so its toolbar goes")
        rig.model.panelExpanded = false
        XCTAssertTrue(wait { self.appItems(rig.window).count == 2 })
    }

    func testTheToolbarFollowsTheSelectedTab() {
        let rig = rig(scene: "done")
        XCTAssertTrue(wait { self.appItems(rig.window).count == 2 })
        rig.model.newTab()
        XCTAssertTrue(wait { rig.window.title == rig.model.selectedTab?.title }, "title: \(rig.window.title)")
        XCTAssertEqual(appItems(rig.window).count, 2)
    }

    // MARK: Minimum width (P-8c)

    func testNoItemMovesIntoTheOverflowMenuAtTheMinimumWidth() {
        let rig = rig(scene: "shell-narrow", width: Shell.minWidth)
        XCTAssertTrue(wait { self.appItems(rig.window).count == 2 })
        XCTAssertTrue(wait { self.appItems(rig.window, visibleOnly: true).count == 2 },
                      "the widest toolbar overflows at Shell.minWidth = \(Shell.minWidth)")
    }

    func testALongTabNameDoesNotPushTheToolbarIntoTheOverflowMenu() {
        let rig = rig(scene: "shell-narrow", width: Shell.minWidth) { model in
            model.selectedTab?.title = String(repeating: "penerima_manfaat_", count: 8)
        }
        XCTAssertTrue(wait { self.appItems(rig.window).count == 2 })
        XCTAssertTrue(wait { self.appItems(rig.window, visibleOnly: true).count == 2 },
                      "the title is the part that gives way")
    }

    // The number is a measurement: if the toolbar gets narrower this fails and `Shell.minWidth`
    // should come down with it, rather than keep a margin nobody can account for.
    func testTheMinimumWidthIsNotLargerThanTheToolbarNeeds() {
        let rig = rig(scene: "shell-narrow", width: Shell.minWidth - 40)
        XCTAssertTrue(wait { self.appItems(rig.window).count == 2 })
        spin(1.0)
        XCTAssertLessThan(appItems(rig.window, visibleOnly: true).count, 2,
                          "everything fits 40 pt below Shell.minWidth, so it can be lowered")
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
