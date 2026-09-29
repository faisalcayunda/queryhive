import XCTest

@testable import QueryHive

/// When the bottom panel is open by default.
///
/// The rule is one line — the panel is open when the tab has something to put in it — but it was
/// wrong in a way that was easy to miss: the panel defaulted to open at 480 points, so a tab nobody
/// had used yet spent more than half the window on "No result yet". These tests pin the rule at
/// each of the three moments it is decided: a new tab, switching tabs, and closing one.
@MainActor
final class PanelDefaultTests: XCTestCase {
    override func setUp() {
        super.setUp()
        // Before any AppModel is built: that constructor reads connections.json, and the actions
        // these tests drive write it.
        isolateConnectionStore()
    }


    /// A model with no connections, so nothing here depends on what is installed on the machine.
    private func model() -> AppModel {
        AppModel()
    }

    /// A one-column preview, so a test can say "this tab has rows" without repeating the shape.
    private func preview(rows: [[String?]]) -> PreviewResult {
        PreviewResult(columns: [Event.Column(name: "a", type: "12")], rows: rows,
                      truncated: false, elapsedMS: 1)
    }

    func testANewTabStartsWithThePanelClosed() {
        // Nothing has been run, so there is nothing to show. The editor gets the whole window.
        let model = model()
        XCTAssertTrue(model.panelCollapsed, "a fresh tab opened the panel over content that does not exist")
    }

    func testANewTabOpenedOverAResultStillStartsClosed() {
        // The rule is about the new tab, not about the model: a second tab is as empty as the first,
        // even when the tab behind it has rows.
        let model = model()
        let first = try? XCTUnwrap(model.selectedTab)
        first?.preview = preview(rows: [["1"]])
        model.newTab()
        XCTAssertTrue(model.panelCollapsed)
    }

    func testSwitchingToATabWithResultsOpensThePanel() {
        let model = model()
        let empty = try? XCTUnwrap(model.selectedTab)
        empty?.preview = preview(rows: [["1"]])
        let withRows = try? XCTUnwrap(model.selectedTab)
        XCTAssertNotNil(withRows)

        model.newTab()
        let fresh = try? XCTUnwrap(model.selectedTab)
        XCTAssertNotNil(fresh)
        XCTAssertTrue(model.panelCollapsed)

        // Back to the tab that has rows: the panel must come back with them, or the rows are hidden
        // behind a click the user did not know they had to make.
        if let id = withRows?.id { model.selectTab(id) }
        XCTAssertFalse(model.panelCollapsed)
    }

    func testSwitchingToAnEmptyTabClosesThePanel() {
        let model = model()
        // A tab with rows, and an empty one after it.
        let withRows = try? XCTUnwrap(model.selectedTab)
        withRows?.preview = preview(rows: [["1"]])
        model.newTab()
        let emptyID = model.selectedTab?.id

        // Switch to the tab with rows: the panel opens for them.
        if let id = withRows?.id { model.selectTab(id) }
        XCTAssertFalse(model.panelCollapsed)

        // And back to the empty one: the panel must close, not stay open over "No result yet".
        if let id = emptyID { model.selectTab(id) }
        XCTAssertTrue(model.panelCollapsed, "an empty tab kept the panel open over nothing")
    }

    func testSwitchingToAnEmptyTabAlsoLeavesFullWindowMode() {
        // `panelExpanded` lives on the model, not the tab, so a tab opened full-window (which is
        // what opening a table does) would otherwise hand that state to the next tab — a
        // full-window panel over "No result yet".
        let model = model()
        let table = try? XCTUnwrap(model.selectedTab)
        table?.preview = preview(rows: [["1"]])
        model.panelCollapsed = false
        model.panelExpanded = true

        model.newTab()
        let emptyID = model.selectedTab?.id

        if let id = table?.id { model.selectTab(id) }
        XCTAssertTrue(model.panelExpanded, "a tab with rows lost its full-window panel")

        if let id = emptyID { model.selectTab(id) }
        XCTAssertFalse(model.panelExpanded, "an empty tab inherited a full-window panel")
        XCTAssertTrue(model.panelCollapsed)
    }

    func testARunningTabCountsAsContent() {
        // The panel is where a run's progress goes, so a run in flight keeps it open even before
        // the first row arrives.
        let model = model()
        let tab = try? XCTUnwrap(model.selectedTab)
        tab?.stage = .running
        tab?.panel = .log
        model.panelCollapsed = false

        model.newTab()
        if let id = tab?.id { model.selectTab(id) }
        XCTAssertFalse(model.panelCollapsed)
    }

    func testAZeroRowPreviewCountsAsContent() {
        // "0 rows" is an answer, and the grid's column header is how it is read. Treating an empty
        // result as no result would hide the columns the query returned.
        let model = model()
        let tab = try? XCTUnwrap(model.selectedTab)
        tab?.preview = preview(rows: [])
        model.panelCollapsed = false

        model.newTab()
        if let id = tab?.id { model.selectTab(id) }
        XCTAssertFalse(model.panelCollapsed)
    }

    func testALogLineCountsAsContent() {
        let model = model()
        let tab = try? XCTUnwrap(model.selectedTab)
        tab?.logLines.append(LogLine(at: .now, kind: .info, text: "Connecting…"))
        model.panelCollapsed = false

        model.newTab()
        if let id = tab?.id { model.selectTab(id) }
        XCTAssertFalse(model.panelCollapsed)
    }

    func testClosingATabHandsThePanelToWhateverIsLeft() {
        let model = model()
        let first = try? XCTUnwrap(model.selectedTab)
        first?.preview = preview(rows: [["1"]])
        model.newTab()
        let second = try? XCTUnwrap(model.selectedTab)
        XCTAssertNotNil(second)

        // Close the empty tab that is in front; the one behind has rows, so the panel opens.
        if let id = second?.id { model.closeTab(id) }
        XCTAssertFalse(model.panelCollapsed)
    }

    func testClosingTheLastTabLeavesNothingSelectedAndThePanelClosed() {
        let model = model()
        let only = try? XCTUnwrap(model.selectedTab)
        model.panelCollapsed = false
        if let id = only?.id { model.closeTab(id) }
        XCTAssertNil(model.selectedTabID)
        XCTAssertTrue(model.panelCollapsed)
    }
}
