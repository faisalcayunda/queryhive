import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// Blueprint w10 §8.5: the corner readout says how many issues the text has, and the pane keeps its
/// Clear button off the find bar (B-1).
@MainActor
final class EditorReadoutTests: XCTestCase {
    private func summary(_ count: Int, server: Bool = false) -> EditorIssueSummary {
        var summary = EditorIssueSummary()
        summary.count = count
        summary.hasServerError = server
        return summary
    }

    func testTheReadoutIsEmptyWithNothingToReport() {
        XCTAssertEqual(EditorReadout.issues(summary(0)), "")
    }

    func testTheReadoutCountsIssues() {
        XCTAssertEqual(EditorReadout.issues(summary(1)), "1 issue")
        XCTAssertEqual(EditorReadout.issues(summary(3)), "3 issues")
    }

    func testTheAccessibilityLabelNamesQueryIssues() {
        XCTAssertEqual(EditorReadout.accessibilityLabel(summary(2)), "2 query issues")
        XCTAssertEqual(EditorReadout.accessibilityLabel(summary(1)), "1 query issue")
    }

    func testItsGlyphIsCoralForAServerErrorAndAmberOtherwise() {
        XCTAssertEqual(NSColor(EditorReadout.tint(summary(1, server: true))).usingColorSpace(.sRGB),
                       NSColor(Tone.markCoral).usingColorSpace(.sRGB))
        XCTAssertEqual(NSColor(EditorReadout.tint(summary(2))).usingColorSpace(.sRGB),
                       NSColor(Tone.markAmber).usingColorSpace(.sRGB))
    }

    func testTheEditorReportsItsIssuesToThePane() throws {
        let rig = EditorRig("select 1;\nselect 'oops;\n")
        try rig.settle()
        EditorRig.drainMain()
        XCTAssertEqual(rig.count.issues.count, 1)
        XCTAssertFalse(rig.count.issues.hasServerError)
        rig.view.setSelectedRange(NSRange(location: 22, length: 0))
        rig.view.insertText("'", replacementRange: NSRange(location: NSNotFound, length: 0))
        try rig.settle()
        EditorRig.drainMain()
        XCTAssertEqual(rig.count.issues, EditorIssueSummary(), "the readout goes when the issue does")
    }

    func testTheEditorTellsThePaneWhenFindIsOpen() throws {
        let rig = EditorRig("select 1")
        let container = FlippedContainerView()
        let findBar = FlippedContainerView.makeFindBar()
        container.addSubview(findBar)
        container.findBar = findBar
        rig.coordinator.container = container
        rig.coordinator.findBar = findBar
        XCTAssertFalse(rig.count.findOpen)
        rig.coordinator.showFind(replacing: false)
        EditorRig.drainMain()
        XCTAssertTrue(rig.count.findOpen, "the pane hides Clear while the bar covers the corner")
        rig.coordinator.closeFind()
        EditorRig.drainMain()
        XCTAssertFalse(rig.count.findOpen)
    }

    /// B-1: a hidden bar used to be laid out at height 0, where its own 7 + 7 point insets could not be
    /// satisfied. It keeps its fitting height, and takes no room from the text.
    func testAHiddenFindBarStillFitsItsConstraintsAndTakesNoRoom() {
        let container = FlippedContainerView(frame: NSRect(x: 0, y: 0, width: 400, height: 300))
        let findBar = FlippedContainerView.makeFindBar()
        let scroll = NSScrollView()
        container.addSubview(scroll)
        container.addSubview(findBar)
        container.findBar = findBar
        container.layoutSubtreeIfNeeded()
        container.needsLayout = true
        container.layoutSubtreeIfNeeded()
        XCTAssertTrue(findBar.isHidden)
        XCTAssertGreaterThanOrEqual(findBar.frame.height, 14, "the stack's insets need at least this")
        XCTAssertEqual(scroll.frame, NSRect(x: 0, y: 0, width: 400, height: 300), "hidden, it takes no room")
        container.findBarVisible = true
        container.layoutSubtreeIfNeeded()
        XCTAssertFalse(findBar.isHidden)
        XCTAssertEqual(scroll.frame.minY, findBar.frame.height)
        XCTAssertEqual(scroll.frame.height, 300 - findBar.frame.height)
    }
}
