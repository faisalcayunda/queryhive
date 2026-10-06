import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// The window says which pane is open: written from the view because a bar of our own cannot drive
/// the title the way a native toolbar of panes would.
@MainActor
final class SettingsWindowTitleTests: XCTestCase {
    private var windows: [NSWindow] = []

    override func tearDown() {
        for window in windows {
            window.isReleasedWhenClosed = false
            window.contentView = nil
            window.close()
        }
        windows = []
        super.tearDown()
    }

    private func host<Content: View>(_ content: Content) -> (NSHostingView<Content>, NSWindow) {
        let host = NSHostingView(rootView: content)
        host.frame = CGRect(x: 0, y: 0, width: 560, height: 640)
        let window = NSWindow(contentRect: host.frame, styleMask: [.titled], backing: .buffered,
                              defer: false)
        window.title = "Settings"
        window.contentView = host
        windows.append(window)
        return (host, window)
    }

    /// The run loop turns once, which is where the binder's second write lands.
    private func settle() {
        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
    }

    func testEveryPaneInTheBarHasADistinctTitle() {
        let titles = SettingsView.Pane.visible.map(\.title)
        XCTAssertEqual(titles, ["General", "Appearance", "Editor", "Data", "Keyboard"])
        XCTAssertEqual(Set(titles).count, titles.count)
    }

    func testTheWindowIsTitledWithThePaneItOpensOn() {
        let (_, window) = host(SettingsView(pane: .editor))
        settle()
        XCTAssertEqual(window.title, "Editor")
    }

    func testTheTitleFollowsWhenThePaneChanges() {
        let (view, window) = host(WindowTitleBinder(title: "Appearance"))
        settle()
        XCTAssertEqual(window.title, "Appearance")
        view.rootView = WindowTitleBinder(title: "Data")
        settle()
        XCTAssertEqual(window.title, "Data")
    }

    func testAnEmptyTitleLeavesTheWindowAlone() {
        let (_, window) = host(WindowTitleBinder(title: ""))
        settle()
        XCTAssertEqual(window.title, "Settings")
    }
}

/// The question mark is a button: a pointer, a keyboard and VoiceOver all reach it.
///
/// Read off the source, like `AccessibilityLabelTests`: SwiftUI only builds its accessibility tree
/// for a client that is attached, and a unit test run has none, so a hosted view answers no children.
final class HelpHintTests: XCTestCase {
    private func hintSource() throws -> String {
        let theme = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/QueryHive/Support/Theme.swift")
        let text = try String(contentsOf: theme, encoding: .utf8)
        let start = try XCTUnwrap(text.range(of: "struct HelpHint: View"))
        let end = try XCTUnwrap(text.range(of: "// MARK: Illustrations", range: start.upperBound..<text.endIndex))
        return String(text[start.lowerBound..<end.lowerBound])
    }

    func testItIsNamedHelp() {
        XCTAssertEqual(HelpHint.accessibilityName, "Help")
    }

    func testItIsAButtonThatOpensThePopoverAndReadsTheSentenceAsItsHint() throws {
        let source = try hintSource()
        for needle in ["Button {", ".buttonStyle(.plain)", ".help(text)",
                       ".accessibilityLabel(Self.accessibilityName)", ".accessibilityHint(text)",
                       ".popover(isPresented: $showing"] {
            XCTAssertTrue(source.contains(needle), "HelpHint is missing \(needle)")
        }
    }

    func testThePopoverIsNarrowerThanTheSettingsWindow() {
        XCTAssertEqual(HelpHint.popoverWidth, 280)
    }
}
