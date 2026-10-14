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

    // The stuck state 0.2.2 reached: set, with nothing on screen. A press still presents it: the
    // flag goes false now and true on the next turn, a change the popover can present.
    func testAPressOnAStuckFlagPresentsTheWizard() {
        let model = AppModel()
        model.exportSettingsOpen = true
        model.exportWizardShown = false
        model.toggleExportWizard(now: 100)
        XCTAssertFalse(model.exportSettingsOpen, "writing true over true presents nothing")
        let turn = expectation(description: "the next main-queue turn")
        DispatchQueue.main.async { turn.fulfill() }
        wait(for: [turn], timeout: 2)
        XCTAssertTrue(model.exportSettingsOpen, "the wizard was not asked for again")
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
}
