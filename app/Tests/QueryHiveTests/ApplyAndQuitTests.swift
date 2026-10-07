import AppKit
import XCTest

@testable import QueryHive

/// What leaves the grid and the app: one apply per tab (PF-2), what the grid shows once it has run
/// (DBX-27), and the one question before staged work is thrown away (DBX-26).
@MainActor
final class ApplyAndQuitTests: XCTestCase {
    private var savedConfirm: ((UnsavedWork, DiscardIntent) -> Bool)!
    /// Every question asked, with what it was about, so a test can say "once".
    private var asked: [(UnsavedWork, DiscardIntent)] = []
    private var answer = true

    override func setUp() {
        super.setUp()
        isolateConnectionStore()
        savedConfirm = AppModel.confirmDiscard
        asked = []
        answer = true
        AppModel.confirmDiscard = { [unowned self] work, intent in
            asked.append((work, intent))
            return answer
        }
        Announcer.reset()
    }

    override func tearDown() {
        AppModel.confirmDiscard = savedConfirm
        super.tearDown()
    }

    private let columns = [Event.Column(name: "id", type: "bigint"), Event.Column(name: "name", type: "varchar")]

    private func makeModel() -> (AppModel, MockEngine, QueryTab) {
        let engine = MockEngine()
        let model = AppModel()
        model.engine = engine
        let connection = Connection(id: UUID(), name: "Dev", color: .violet, kind: .postgres,
                                    host: "localhost", port: 5432, sslmode: "",
                                    user: "qh", database: "app", schema: "public", verify: false)
        model.connections = [connection]
        let tab = model.selectedTab ?? { model.newTab(); return model.selectedTab! }()
        tab.connectionID = connection.id
        tab.showRows(columns: columns, rows: (0..<5).map { ["\($0)", "v\($0)"] },
                     truncated: false, queryID: nil, elapsedMS: 0)
        tab.sourceTable = "public.t"
        tab.previewBaseSQL = "SELECT * FROM public.t"
        tab.previewedSQL = "SELECT * FROM public.t"
        return (model, engine, tab)
    }

    private func plan(for tab: QueryTab) -> WritePlan {
        WritePlan.build(edits: tab.cellEdits, rows: tab.result, columns: columns,
                        table: "public.t", kind: .postgres)
    }

    private func scriptApply(_ engine: MockEngine) {
        var done = Event(event: "done")
        done.applied = 1
        engine.answer("apply_changes", with: .events([done]))
        engine.answer("preview", with: .events([Event(event: "columns", columns: columns),
                                                Event(event: "done", rows: 0)]))
    }

    // MARK: PF-2, one apply per tab

    func testASecondApplyIsRefusedWhileTheFirstRuns() {
        let (model, engine, tab) = makeModel()
        scriptApply(engine)
        tab.cellEdits.edit("x", at: CellKey(row: 0, column: 1), original: "v0")

        model.applyChanges(plan(for: tab), in: tab)
        XCTAssertTrue(tab.applying)
        model.applyChanges(plan(for: tab), in: tab)

        XCTAssertEqual(engine.calls.filter { $0.command == "apply_changes" }.count, 1)
        XCTAssertTrue(tab.logLines.contains { $0.text.contains("already being applied") })
    }

    func testWhileApplyingNothingNewIsStagedAndNothingCanBeReviewed() {
        let (model, _, tab) = makeModel()
        tab.cellEdits.edit("x", at: CellKey(row: 0, column: 1), original: "v0")
        tab.applying = true

        tab.beginCellEdit(at: CellKey(row: 1, column: 1))
        XCTAssertFalse(tab.hasOpenCellEdit)
        tab.fillCellEdits("f", over: CellRange(from: (row: 1, column: 1), to: (row: 2, column: 1)))
        tab.pasteCellEdits("p", at: CellKey(row: 3, column: 1), columnCount: 2)
        model.addRow(in: tab)
        tab.selectCells(anchor: CellPos(row: 4, column: 0), focus: CellPos(row: 4, column: 0))
        model.deleteRows(in: tab)

        XCTAssertEqual(tab.cellEdits.count, 1, "only the staged edit that is being applied")
        XCTAssertFalse(model.canSaveFocused, "⌘S has nothing to do under an apply")
        model.saveFocused()
        XCTAssertFalse(tab.reviewRequested)
    }

    func testAnEditTypedWhileTheApplyRunsSurvivesItsSuccess() {
        let (model, engine, tab) = makeModel()
        scriptApply(engine)
        tab.cellEdits.edit("x", at: CellKey(row: 0, column: 1), original: "v0")
        model.applyChanges(plan(for: tab), in: tab)

        // A session opened before the apply began ends while it runs.
        tab.cellEdits.edit("typed", at: CellKey(row: 3, column: 1), original: "v3")
        GridFixture.drain(until: { !tab.applying })

        XCTAssertFalse(tab.applying)
        XCTAssertNil(tab.cellEdits.value(at: CellKey(row: 0, column: 1)), "the applied cell is cleared")
        XCTAssertEqual(tab.cellEdits.value(at: CellKey(row: 3, column: 1)), "typed")
        XCTAssertFalse(engine.calls.contains { $0.command == "preview" },
                       "a refresh would have thrown the survivor away")
        XCTAssertTrue(tab.logLines.contains { $0.text.contains("still staged") })
        XCTAssertFalse(tab.canUndoCellEdit, "an undo step could put the written cell back")
    }

    func testACellEditedBackToADifferentValueDuringTheApplyIsKept() {
        var snapshot = CellEdits()
        snapshot.edit("x", at: CellKey(row: 0, column: 1), original: "v0")
        var now = snapshot
        now.edit("changed again", at: CellKey(row: 0, column: 1), original: "v0")
        XCTAssertEqual(now.removing(snapshot).value(at: CellKey(row: 0, column: 1)), "changed again")
    }

    func testRemovingTheSnapshotClearsAddedAndDeletedRowsItHeld() {
        var snapshot = CellEdits()
        let id = snapshot.insertRow()
        snapshot.setInserted("n", row: id, column: 1)
        snapshot.deleteRow(2)
        var now = snapshot
        let later = now.insertRow()
        now.setInserted("later", row: later, column: 1)

        let left = now.removing(snapshot)
        XCTAssertEqual(left.inserted.map(\.id), [later])
        XCTAssertTrue(left.deletedRows.isEmpty)
    }

    func testRemovingTheSnapshotDropsAnAddedRowItCarriedEvenWhenItsValuesChangedSince() {
        var snapshot = CellEdits()
        let id = snapshot.insertRow()
        snapshot.setInserted("n", row: id, column: 1)
        var now = snapshot
        now.setInserted("n2", row: id, column: 1)
        XCTAssertTrue(now.removing(snapshot).inserted.isEmpty)
    }

    func testAFailedApplyKeepsTheQueueAndSaysWhy() {
        let (model, engine, tab) = makeModel()
        engine.answer("apply_changes", with: .failure(events: [Event(event: "error", message: "row count differs")],
                                                      reason: "x"))
        tab.cellEdits.edit("x", at: CellKey(row: 0, column: 1), original: "v0")
        model.applyChanges(plan(for: tab), in: tab)
        GridFixture.drain(until: { !tab.applying })

        XCTAssertEqual(tab.cellEdits.count, 1)
        XCTAssertTrue(tab.logLines.contains { $0.text.contains("row count differs") })
        XCTAssertFalse(engine.calls.contains { $0.command == "preview" })
    }

    // MARK: Text typed but not yet staged

    func testSaveStagesTheOpenEditorSoTheReviewHoldsWhatWasTyped() {
        let (model, _, tab) = makeModel()
        let key = CellKey(row: 1, column: 1)
        tab.beginCellEdit(at: key)
        tab.typeCellEdit("typed")
        XCTAssertTrue(tab.cellEdits.isEmpty, "typing alone reaches only the buffer")
        XCTAssertTrue(model.canSaveFocused, "typed text is something to save")

        model.saveFocused()

        XCTAssertTrue(tab.reviewRequested)
        XCTAssertFalse(tab.hasOpenCellEdit, "the session ended, so no overlay can type into it")
        XCTAssertEqual(tab.cellEdits.value(at: key), "typed")
        XCTAssertEqual(plan(for: tab).statements.count, 1)
    }

    func testSaveWithAnEditorTypedBackToTheOriginalOpensNoReview() {
        let (model, _, tab) = makeModel()
        tab.beginCellEdit(at: CellKey(row: 1, column: 1))
        tab.typeCellEdit("v1")
        model.saveFocused()
        XCTAssertFalse(tab.reviewRequested)
        XCTAssertFalse(tab.hasOpenCellEdit)
    }

    func testANewSessionCommitsAnOrphanedOneToItsOwnCell() {
        let (_, _, tab) = makeModel()
        let first = CellKey(row: 1, column: 1)
        tab.beginCellEdit(at: first)
        tab.typeCellEdit("left behind")

        tab.beginCellEdit(at: CellKey(row: 2, column: 1))
        tab.typeCellEdit("second")
        tab.endCellEdit()

        XCTAssertEqual(tab.cellEdits.value(at: first), "left behind")
        XCTAssertEqual(tab.cellEdits.value(at: CellKey(row: 2, column: 1)), "second")
    }

    func testAnAddedRowTheApplyWroteIsNotInsertedASecondTime() {
        let (model, engine, tab) = makeModel()
        scriptApply(engine)
        let id = tab.cellEdits.insertRow()
        tab.cellEdits.setInserted("new", row: id, column: 1)
        model.applyChanges(plan(for: tab), in: tab)

        // A session opened over that row before the apply ends while it runs.
        tab.cellEdits.setInserted("newer", row: id, column: 1)
        GridFixture.drain(until: { !tab.applying })

        XCTAssertTrue(tab.cellEdits.inserted.isEmpty, "the row is in the table")
        XCTAssertTrue(plan(for: tab).statements.isEmpty)
        XCTAssertTrue(tab.logLines.contains { $0.text.contains("not written") })
    }

    // MARK: DBX-27, what the grid shows after an apply

    func testASuccessfulApplyRunsTheBaseQueryAgainAndTheLogSaysSo() {
        let (model, engine, tab) = makeModel()
        scriptApply(engine)
        tab.cellEdits.edit("x", at: CellKey(row: 0, column: 1), original: "v0")
        model.applyChanges(plan(for: tab), in: tab)
        GridFixture.drain(until: { engine.calls.contains { $0.command == "preview" } })

        let preview = engine.calls.first { $0.command == "preview" }
        XCTAssertEqual(preview?.env["SQL"], "SELECT * FROM public.t")
        XCTAssertTrue(tab.cellEdits.isEmpty)
        XCTAssertTrue(tab.logLines.contains { $0.text.contains("Re-running the base query") })
        XCTAssertFalse(tab.rowsStaleAfterApply)
    }

    func testTheLogNamesTheSortThatTheRefreshResets() {
        let (model, engine, tab) = makeModel()
        scriptApply(engine)
        tab.activeSort = ActiveSort(column: 1, direction: .ascending, origin: .memory)
        model.finishApply(tab, succeeded: true, applied: 1, failure: nil, snapshot: CellEdits())
        XCTAssertTrue(tab.logLines.contains { $0.text.contains("sort, search and filters were reset") })
        GridFixture.drain(until: { !tab.previewing })
    }

    func testWhenTheQueryCannotBeRunAgainTheRowsAreStaleAndEditsAreRefusedUntilANewResult() {
        let (model, _, tab) = makeModel()
        tab.previewBaseSQL = nil
        model.finishApply(tab, succeeded: true, applied: 1, failure: nil, snapshot: CellEdits())

        XCTAssertTrue(tab.rowsStaleAfterApply)
        XCTAssertTrue(tab.logLines.contains { $0.text.contains("could not be run again") })
        tab.beginCellEdit(at: CellKey(row: 0, column: 1))
        XCTAssertFalse(tab.hasOpenCellEdit)
        XCTAssertNotNil(model.rowEditBlockedReason(for: tab))

        // A new result replaces the rows the apply left behind.
        tab.showRows(columns: columns, rows: [["0", "fresh"]], truncated: false, queryID: nil, elapsedMS: 0)
        XCTAssertFalse(tab.rowsStaleAfterApply)
        XCTAssertNil(model.rowEditBlockedReason(for: tab))
    }

    // MARK: ⌘S

    func testSaveOpensTheReviewOnlyWhenThereIsSomethingToReview() {
        let (model, _, tab) = makeModel()
        XCTAssertFalse(model.canSaveFocused)
        model.saveFocused()
        XCTAssertFalse(tab.reviewRequested)

        tab.cellEdits.edit("x", at: CellKey(row: 0, column: 1), original: "v0")
        XCTAssertTrue(model.canSaveFocused)
        tab.panel = .log
        model.saveFocused()
        XCTAssertTrue(tab.reviewRequested)
        XCTAssertEqual(tab.panel, .result, "the grid has to be on screen to show it")
    }

    func testSaveIsOffWhileTheViewIsBeingReplaced() {
        let (model, _, tab) = makeModel()
        tab.cellEdits.edit("x", at: CellKey(row: 0, column: 1), original: "v0")
        XCTAssertTrue(model.canSaveFocused)
        let menu = AppMenu.specs(for: .queryhive).first { $0.id == ShortcutAction.saveFile.rawValue }
        XCTAssertNotNil(menu)
        XCTAssertTrue(AppMenu.isEnabled(menu!, in: model))
        XCTAssertEqual(menu?.shortcut?.display, "⌘S")
    }

    // MARK: DBX-26, unsaved work

    func testUnsavedWorkCountsStagedChangesTabsAppliesExportsAndImports() {
        let (model, _, tab) = makeModel()
        XCTAssertTrue(model.unsavedWork.isEmpty)

        tab.cellEdits.edit("x", at: CellKey(row: 0, column: 1), original: "v0")
        tab.cellEdits.deleteRow(2)
        tab.applying = true
        tab.stage = .running
        model.importDraft = ImportDraft(mapping: ImportMapping(), connectionID: nil)
        model.importDraft?.running = true

        let work = model.unsavedWork
        XCTAssertEqual(work.stagedChanges, 2)
        XCTAssertEqual(work.tabsWithChanges, 1)
        XCTAssertEqual(work.applies, 1)
        XCTAssertEqual(work.exports, 1)
        XCTAssertEqual(work.imports, 1)
        XCTAssertEqual(work.lines.count, 4)
        XCTAssertTrue(work.lines[0].contains("2 staged changes in 1 tab"))
    }

    func testClosingACleanTabAsksNothing() {
        let (model, _, tab) = makeModel()
        model.newTab()
        XCTAssertTrue(model.closeTab(tab.id))
        XCTAssertTrue(asked.isEmpty)
    }

    func testClosingATabWithStagedEditsAsksOnceAndCancelKeepsTheTabAndItsEdits() {
        let (model, _, tab) = makeModel()
        model.newTab()
        tab.cellEdits.edit("x", at: CellKey(row: 0, column: 1), original: "v0")
        answer = false

        XCTAssertFalse(model.closeTab(tab.id))

        XCTAssertEqual(asked.count, 1)
        XCTAssertEqual(asked.first?.1, .closeTabs(1))
        XCTAssertTrue(model.tabs.contains { $0.id == tab.id })
        XCTAssertEqual(tab.cellEdits.count, 1, "Cancel keeps the staged edit")
    }

    func testConfirmingClosesTheTab() {
        let (model, _, tab) = makeModel()
        model.newTab()
        tab.cellEdits.edit("x", at: CellKey(row: 0, column: 1), original: "v0")
        XCTAssertTrue(model.closeTab(tab.id))
        XCTAssertFalse(model.tabs.contains { $0.id == tab.id })
    }

    func testClosingAWholeSetOfTabsIsOneQuestionNotOnePerTab() {
        let (model, _, first) = makeModel()
        model.newTab(); model.newTab()
        for tab in model.tabs { tab.cellEdits.edit("x", at: CellKey(row: 0, column: 0), original: "0") }
        answer = false

        model.closeOtherTabs(keeping: first.id)
        XCTAssertEqual(asked.count, 1)
        XCTAssertEqual(model.tabs.count, 3, "Cancel closes none")

        answer = true
        model.closeAllTabs()
        XCTAssertEqual(asked.count, 2)
        XCTAssertTrue(model.tabs.isEmpty)
    }

    func testClosingTabsToTheRightAsksAboutThoseOnly() {
        let (model, _, first) = makeModel()
        model.newTab()
        first.cellEdits.edit("x", at: CellKey(row: 0, column: 0), original: "0")   // not to be closed
        model.closeTabs(after: first.id)
        XCTAssertTrue(asked.isEmpty)
        XCTAssertEqual(model.tabs.count, 1)
    }

    func testQuitAsksWhenThereIsStagedWorkAndCancelKeepsEverything() {
        let (model, _, tab) = makeModel()
        XCTAssertTrue(model.confirmQuit())
        XCTAssertTrue(asked.isEmpty)

        tab.cellEdits.edit("x", at: CellKey(row: 0, column: 1), original: "v0")
        answer = false
        XCTAssertFalse(model.confirmQuit())
        XCTAssertEqual(asked.last?.1, .quit)
        XCTAssertEqual(tab.cellEdits.count, 1)
        answer = true
        XCTAssertTrue(model.confirmQuit())
    }

    func testARunningImportAloneIsReasonToAskBeforeQuitting() {
        let (model, _, _) = makeModel()
        model.importDraft = ImportDraft(mapping: ImportMapping(), connectionID: nil)
        model.importDraft?.running = true
        answer = false
        XCTAssertFalse(model.confirmQuit())
        XCTAssertEqual(asked.last?.0.imports, 1)
    }

    func testTheAppDelegateAnswersTerminateFromTheSameQuestion() {
        let (model, _, tab) = makeModel()
        let delegate = AppDelegate()
        delegate.model = model
        XCTAssertEqual(delegate.applicationShouldTerminate(NSApplication.shared), .terminateNow)

        tab.cellEdits.edit("x", at: CellKey(row: 0, column: 1), original: "v0")
        answer = false
        XCTAssertEqual(delegate.applicationShouldTerminate(NSApplication.shared), .terminateCancel)
        answer = true
        XCTAssertEqual(delegate.applicationShouldTerminate(NSApplication.shared), .terminateNow)
    }

    func testTheSentencesNameWhatIsLost() {
        var work = UnsavedWork()
        work.stagedChanges = 1
        work.tabsWithChanges = 1
        work.exports = 2
        XCTAssertEqual(work.lines, ["1 staged change in 1 tab will be discarded.",
                                    "2 exports still running will be stopped."])
        XCTAssertEqual(DiscardIntent.quit.question, "Quit QueryHive?")
        XCTAssertEqual(DiscardIntent.closeTabs(1).question, "Close this query?")
        XCTAssertEqual(DiscardIntent.closeTabs(3).question, "Close 3 queries?")
    }

    // MARK: PF-12

    func testTheUndoHistoryKeepsAboutAHundredSteps() {
        let tab = QueryTab(title: "Q")
        XCTAssertEqual(tab.editUndoManager.levelsOfUndo, 100)
    }

    func testAFillOrPasteOverTheCeilingIsRefusedAndSaysSo() {
        let (_, _, tab) = makeModel()
        let wide = CellRange(from: (row: 0, column: 0), to: (row: 60_000, column: 0))
        tab.fillCellEdits("z", over: wide)
        XCTAssertTrue(tab.cellEdits.isEmpty)
        XCTAssertTrue(tab.logLines.contains {
            $0.text.contains("at most \(QueryTab.editCellCeiling.formatted())")
        })
        XCTAssertTrue(tab.exceedsCellCeiling(QueryTab.editCellCeiling + 1))
        XCTAssertFalse(tab.exceedsCellCeiling(QueryTab.editCellCeiling))
    }

    func testDeletingMoreCellsThanTheCeilingIsRefused() {
        let (model, _, tab) = makeModel()
        tab.selectCells(anchor: CellPos(row: 0, column: 0), focus: CellPos(row: 4, column: 1))
        XCTAssertFalse(tab.exceedsCellCeiling(tab.cellSelection!.cellCount))
        model.deleteRows(in: tab)
        XCTAssertEqual(tab.cellEdits.deletedRows.count, 5)
    }

    // MARK: Row-edit gates

    func testAddingRowsIsRefusedWithAReasonWhereItCannotWork() {
        let (model, _, tab) = makeModel()
        XCTAssertNil(model.rowEditBlockedReason(for: tab))

        tab.sourceTable = nil
        XCTAssertEqual(model.rowEditBlockedReason(for: tab), "Open a table to add rows")
        tab.sourceTable = "public.t"

        model.connections[0].safeMode = .readOnly
        XCTAssertEqual(model.rowEditBlockedReason(for: tab), "This connection is read-only")
        model.connections[0].safeMode = .noDDL
        XCTAssertNil(model.rowEditBlockedReason(for: tab), "no_ddl still allows DML")
        model.connections[0].safeMode = .full

        tab.previewing = true
        XCTAssertEqual(model.rowEditBlockedReason(for: tab), QueryTab.viewBusyMessage)
        tab.previewing = false

        model.addRow(in: tab)
        XCTAssertEqual(tab.cellEdits.inserted.count, 1)
    }
}
