import XCTest

@testable import QueryHive

/// The footer's Export and the Run menu's Export… (`AppModel.toggleExportWizard`), at the level of
/// the model: no window. The owner's report on 0.2.2 was that Export opened the wizard once and was
/// dead after it closed. With the wizard open, a click on Export is a click outside the popover: the
/// popover writes `false` (it closes itself), then the button wrote `true` in the same click, and the
/// flag stayed true with nothing on screen; every later `= true` changed nothing. Measured in a
/// hosted window with real queued clicks before the fix: the popover's write came first, the
/// button's second, the flag ended `true`.
@MainActor
final class ExportWizardToggleTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    /// The popover on screen, as its panel's `onAppear` leaves it.
    private func shown(_ model: AppModel) {
        model.exportSettingsOpen = true
        model.exportWizardShown = true
    }

    func testAPressOpensAClosedWizard() {
        let model = AppModel()
        model.toggleExportWizard(now: 100)
        XCTAssertTrue(model.exportSettingsOpen)
    }

    // The order measured: the popover closes itself first, then the button's action runs, in the
    // one click. The click closes it, and the next press (a moment later) opens it.
    func testTheClickThatClosedThePopoverLeavesItClosedAndTheNextPressOpensIt() {
        let model = AppModel()
        shown(model)
        model.exportWizardPresented(false, now: 100)   // the popover, on the click outside it
        model.toggleExportWizard(now: 100.1)            // the button, in the same click
        XCTAssertFalse(model.exportSettingsOpen, "the press that closed the wizard opened it again")
        model.exportWizardShown = false                 // the panel's onDisappear
        model.toggleExportWizard(now: 102)
        XCTAssertTrue(model.exportSettingsOpen, "the next press did not open the wizard")
    }

    // The other order: the button's action runs while the popover is still open and still set.
    func testAPressOnAShownWizardClosesItAndTheNextPressOpensIt() {
        let model = AppModel()
        shown(model)
        model.toggleExportWizard(now: 100)
        XCTAssertFalse(model.exportSettingsOpen)
        model.exportWizardPresented(false, now: 100)   // the popover's own write, after: a no-op
        model.exportWizardShown = false
        model.toggleExportWizard(now: 102)
        XCTAssertTrue(model.exportSettingsOpen)
    }

    // The stuck state 0.2.2 reached: set, with nothing on screen. The first press clears it and
    // the second is a plain false-to-true open. This checks the flag only, not that a popover is
    // presented: what the popover does with a false-to-true change needs a window.
    func testAStuckFlagIsClearedByOnePressAndTheNextPressOpens() {
        let model = AppModel()
        model.exportSettingsOpen = true
        model.exportWizardShown = false
        model.toggleExportWizard(now: 100)
        XCTAssertFalse(model.exportSettingsOpen, "writing true over true presents nothing")
        model.toggleExportWizard(now: 101)
        XCTAssertTrue(model.exportSettingsOpen, "the second press did not open the wizard")
    }

    // A panel that never reported its disappearance must not turn every press into a close.
    func testAShownMarkLeftBehindDoesNotStopAPressFromOpening() {
        let model = AppModel()
        model.exportWizardShown = true
        model.exportSettingsOpen = false
        model.toggleExportWizard(now: 100)
        XCTAssertTrue(model.exportSettingsOpen)
    }

    // Esc, Cancel and a click elsewhere close it through the same write; a press after them opens it.
    func testAPressAfterAnyOtherCloseOpensTheWizard() {
        let model = AppModel()
        shown(model)
        model.exportWizardPresented(false, now: 100)
        model.exportWizardShown = false
        model.toggleExportWizard(now: 101)
        XCTAssertTrue(model.exportSettingsOpen)
    }

    // MARK: What dims the footer's Export

    /// One PostgreSQL connection and a tab with a statement, writing to a file.
    private func fixture() -> (AppModel, QueryTab) {
        let model = AppModel()
        model.engine = MockEngine()
        let connection = Connection(id: UUID(), name: "Dev", color: .violet, kind: .postgres, host: "localhost",
                                    port: 5432, sslmode: "", user: "qh", database: "app", schema: "",
                                    verify: false)
        model.connections = [connection]
        let tab = QueryTab(title: "Query 1")
        tab.connectionID = connection.id
        tab.sql = "SELECT 1 AS id"
        tab.destination = .file
        tab.outputName = "export"
        tab.outputDirectory = URL(fileURLWithPath: "/tmp")
        model.tabs = [tab]
        model.selectedTabID = tab.id
        return (model, tab)
    }

    // Closing the wizard with its own settings unfinished used to dim the only button that opens
    // it again (`runBlockedReason`). The opener now waits only for something to export; the
    // wizard's Export still waits for the settings.
    func testUnfinishedWizardSettingsDoNotDimTheButtonThatOpensTheWizard() {
        let (model, tab) = fixture()
        tab.outputDirectory = nil
        XCTAssertEqual(model.runBlockedReason(for: tab), "Choose an output folder", "the wizard's Export")
        XCTAssertNil(model.exportWizardBlockedReason(for: tab), "the footer's Export")
        tab.outputDirectory = URL(fileURLWithPath: "/tmp")
        tab.outputName = " "
        XCTAssertEqual(model.runBlockedReason(for: tab), "Name the output file")
        XCTAssertNil(model.exportWizardBlockedReason(for: tab))
        // Table, on a connection with no default schema: the wizard asks for one.
        tab.destination = .table
        tab.targetTable = "people"
        XCTAssertEqual(model.runBlockedReason(for: tab), "Name the target schema")
        XCTAssertNil(model.exportWizardBlockedReason(for: tab))
    }

    // What still dims the opener: nothing to export yet. Each also dims the wizard's Export.
    func testTheOpenerIsDimOnlyWithNoConnectionOrNoStatement() {
        let (model, tab) = fixture()
        XCTAssertNil(model.exportWizardBlockedReason(for: tab))
        tab.sql = "  "
        XCTAssertEqual(model.exportWizardBlockedReason(for: tab), "Write a query")
        XCTAssertEqual(model.runBlockedReason(for: tab), "Write a query")
        tab.sql = "SELECT 1"
        tab.connectionID = UUID()
        XCTAssertEqual(model.exportWizardBlockedReason(for: tab), "Choose a connection")
        model.connections = []
        XCTAssertEqual(model.exportWizardBlockedReason(for: tab), "Add a connection first")
    }

    // MARK: The call sites

    private static let sources = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("Sources/QueryHive")

    private func source(_ path: String) throws -> String {
        try String(contentsOf: Self.sources.appendingPathComponent(path), encoding: .utf8)
    }

    // The model tests above hold the rule; this holds the wiring. A view that sets the flag
    // straight to true again, or a footer gated on the wizard's settings again, turns this red.
    func testTheOpenersGoThroughTheModelAndTheFooterIsGatedOnlyOnSomethingToExport() throws {
        let files = try XCTUnwrap(FileManager.default.enumerator(at: Self.sources, includingPropertiesForKeys: nil))
        // Snapshot seeds a scene with the wizard open; it draws, it is not a button.
        let allowed: Set = ["AppModel+Export.swift", "Snapshot.swift"]
        var writers: [String] = []
        for case let url as URL in files where url.pathExtension == "swift" && !allowed.contains(url.lastPathComponent) {
            let text = try String(contentsOf: url, encoding: .utf8)
            if text.range(of: #"exportSettingsOpen\s*=\s*true"#, options: .regularExpression) != nil {
                writers.append(url.lastPathComponent)
            }
        }
        XCTAssertEqual(writers, [], "set the wizard open through toggleExportWizard")
        let grid = try source("Views/ResultGrid.swift")
        XCTAssertTrue(grid.contains("model.toggleExportWizard()"), "the footer's Export")
        XCTAssertTrue(grid.contains(".disabled(blocked != nil || tab.stage == .running)")
                      && grid.contains("let blocked = model.exportWizardBlockedReason(for: tab)"),
                      "the footer's Export is dimmed by the wizard's own settings again")
        XCTAssertTrue(try source("Views/Workspace.swift").contains("model.toggleExportWizard()"), "Run menu's Export…")
        XCTAssertTrue(try source("Views/ExportSettings.swift").contains("model.exportWizardPresented($0)"),
                      "the popover's binding")
    }
}
