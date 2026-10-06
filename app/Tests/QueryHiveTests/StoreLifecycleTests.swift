import XCTest
import QueryHiveFFI

@testable import QueryHive

/// What a tab does with its stores: the view guard for staged edits (D-27), the generation that
/// decides which apply may end `viewBusy`, the close that lets go of both stores and stops both
/// runs (§19, TM-4), and the engine refusing what it cannot run (§17.1).
@MainActor
final class StoreLifecycleTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    private let columns = [Event.Column(name: "id", type: "bigint"), Event.Column(name: "nama", type: "varchar")]
    private let rows: [[String?]] = [["1", "a"], ["2", "b"], ["3", "c"]]

    private func spin(until condition: () -> Bool, seconds: Double = 5) {
        let deadline = Date().addingTimeInterval(seconds)
        while !condition(), Date() < deadline { RunLoop.main.run(until: Date().addingTimeInterval(0.005)) }
    }

    /// A tab over a fake store whose `set_view` answers are controlled by the test: each call waits
    /// on its own semaphore, so the order in which applies finish is the test's.
    private final class Gate {
        let handle = FakeResultHandle(rows: [["1", "a"], ["2", "b"], ["3", "c"]], columns: 2, phase: .complete)
        var semaphores: [DispatchSemaphore] = []
        let lock = NSLock()
        var started = 0
        init() {
            handle.nextView = { [unowned self] _ in
                self.lock.lock()
                let index = self.started
                self.started += 1
                self.lock.unlock()
                let semaphore = DispatchSemaphore(value: 0)
                self.lock.lock(); self.semaphores.append(semaphore); self.lock.unlock()
                semaphore.wait()
                self.handle.serve(view: UInt64(index + 1))
                return ViewInfo(viewId: UInt64(index + 1), visible: 3, fetched: 3)
            }
        }
        var begun: Int { lock.lock(); defer { lock.unlock() }; return started }
        func finish(_ index: Int) { lock.lock(); let s = semaphores[index]; lock.unlock(); s.signal() }
    }

    private func gatedTab() -> (QueryTab, Gate) {
        let gate = Gate()
        let tab = QueryTab(title: "Q")
        let store = StoreRows(handle: gate.handle, columns: columns)
        tab.activeResult = store
        tab.showRowsPreviewMetadata(columns: columns, rowCount: 3)
        return (tab, gate)
    }

    // MARK: viewBusy (D-27)

    func testOnlyTheLatestApplyEndsViewBusy() {
        let (tab, gate) = gatedTab()
        tab.columnFilters = [0: .text("1")]
        spin { gate.begun == 1 }
        XCTAssertTrue(tab.viewBusy)
        let first = tab.gridRevision

        tab.columnFilters = [0: .text("2")]
        spin { gate.begun == 2 }
        XCTAssertEqual(gate.begun, 1, "applies reach the store one at a time, in order")
        XCTAssertTrue(tab.viewBusy)

        // The older apply finishes first. Its view is installed, so the grid is told; but a newer one
        // is queued, so the guard stays up.
        gate.finish(0)
        spin { tab.gridRevision > first }
        XCTAssertGreaterThan(tab.gridRevision, first, "the grid is told its rows moved")
        XCTAssertTrue(tab.viewBusy, "an older apply must not lower the guard")

        spin { gate.begun == 2 }
        gate.finish(1)
        spin { !tab.viewBusy }
        XCTAssertFalse(tab.viewBusy)
    }

    func testTheStoreEndsOnTheLatestViewEvenWhenTheHopsComeBackOutOfOrder() throws {
        // The second ask is answered with an older view id than the first, as a hop that lands late
        // would be: the store keeps the newer view and the count that goes with it.
        let handle = FakeResultHandle(rows: rows, columns: 2, phase: .complete)
        var answers: [ViewInfo] = [ViewInfo(viewId: 5, visible: 3, fetched: 3), ViewInfo(viewId: 3, visible: 1, fetched: 3)]
        handle.nextView = { _ in answers.removeFirst() }
        let store = StoreRows(handle: handle, columns: columns)
        try store.applyBlocking(ViewSpec(sort: nil, filters: [], search: nil))
        try store.applyBlocking(ViewSpec(sort: nil, filters: [], search: nil))
        XCTAssertEqual(store.viewID, 5)
        XCTAssertEqual(store.count, 3)
    }

    func testTwoAppliesReachRustInCallOrderSoTheLaterSpecIsTheOneInstalled() async throws {
        let handle = FakeResultHandle(rows: rows, columns: 2, phase: .complete)
        let lock = NSLock()
        var specs: [String?] = []
        handle.nextView = { spec in
            lock.lock(); specs.append(spec.search); let id = UInt64(specs.count); lock.unlock()
            if id == 1 { Thread.sleep(forTimeInterval: 0.2) }   // the first ask is the slow one
            handle.serve(view: id)
            return ViewInfo(viewId: id, visible: 3, fetched: 3)
        }
        let store = StoreRows(handle: handle, columns: columns)
        async let slow = store.apply(ViewSpec(sort: nil, filters: [], search: "one"))
        try await Task.sleep(nanoseconds: 50_000_000)
        async let fast = store.apply(ViewSpec(sort: nil, filters: [], search: "two"))
        _ = try await (slow, fast)
        XCTAssertEqual(specs, ["one", "two"])
        XCTAssertEqual(store.viewID, 2, "the later ask is the view on screen")
    }

    func testAServerRunThatReplacesTheStoreLowersTheGuardOfAnApplyStillInFlight() {
        let (tab, gate) = gatedTab()
        tab.columnFilters = [0: .text("1")]
        spin { gate.begun == 1 }
        XCTAssertTrue(tab.viewBusy)
        // What a non-base `runPreview` does: swap the store without going through `releaseResults`.
        let old = tab.activeResult
        tab.activeResult = nil
        tab.activeResult = StoreRows(handle: FakeResultHandle(rows: rows, columns: 2, phase: .complete), columns: columns)
        XCTAssertFalse(tab.viewBusy)
        gate.finish(0)
        RunLoop.main.run(until: Date().addingTimeInterval(0.1))
        XCTAssertFalse(tab.viewBusy, "the old apply's hop lands on a store that is no longer shown")
        _ = old
        tab.showRowsPreviewMetadata(columns: columns, rowCount: 3)
        tab.fillCellEdits("f", over: CellRange(from: (row: 0, column: 1), to: (row: 0, column: 1)))
        XCTAssertFalse(tab.cellEdits.isEmpty, "staging works on the new store")
    }

    func testAnApplyThatFailsWithAnythingButAViewErrorStillLowersTheGuard() {
        let (tab, gate) = gatedTab()
        gate.handle.nextView = { _ in throw StoreFfiError.StaleHandle }
        tab.columnFilters = [0: .text("1")]
        spin { !tab.viewBusy }
        XCTAssertFalse(tab.viewBusy)
    }

    func testAnApplyThatWasSupersededBeforeItStartedNeverReachesTheStore() {
        let (tab, gate) = gatedTab()
        tab.columnFilters = [0: .text("1")]
        tab.columnFilters = [0: .text("2")]     // same run-loop turn: the first never starts
        spin { gate.begun == 1 }
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        XCTAssertEqual(gate.begun, 1)
        gate.finish(0)
        spin { !tab.viewBusy }
        XCTAssertFalse(tab.viewBusy)
    }

    func testStagingFillingPastingAndCommittingAreRefusedWhileTheViewIsBeingReplaced() {
        let (tab, gate) = gatedTab()
        tab.columnFilters = [0: .text("1")]
        spin { gate.begun == 1 }
        XCTAssertTrue(tab.viewBusy)

        tab.beginCellEdit(at: CellKey(row: 0, column: 1))
        tab.typeCellEdit("x")
        tab.endCellEdit()
        tab.fillCellEdits("f", over: CellRange(from: (row: 0, column: 0), to: (row: 1, column: 1)))
        tab.pasteCellEdits("p", at: CellKey(row: 0, column: 0), columnCount: 2)
        XCTAssertTrue(tab.cellEdits.isEmpty, "nothing was queued against rows that are about to move")
        XCTAssertTrue(tab.logLines.contains { $0.text.contains(QueryTab.viewBusyMessage) })

        gate.finish(0)
        spin { !tab.viewBusy }
        tab.fillCellEdits("f", over: CellRange(from: (row: 0, column: 1), to: (row: 1, column: 1)))
        XCTAssertFalse(tab.cellEdits.isEmpty, "and staging works again afterwards")
    }

    func testTheViewLandingDropsAnEditThatGotInFirstAndTheSelection() {
        let (tab, gate) = gatedTab()
        tab.columnFilters = [0: .text("1")]
        spin { gate.begun == 1 }
        // An edit that reached the queue before the guard came on, as a keystroke already in flight
        // would: it is keyed by an index of the old view.
        tab.cellEdits.edit("x", at: CellKey(row: 0, column: 1), original: "a")
        tab.cellSelection = CellRange(from: (row: 0, column: 0), to: (row: 0, column: 0))
        gate.finish(0)
        spin { !tab.viewBusy }
        XCTAssertTrue(tab.cellEdits.isEmpty)
        XCTAssertNil(tab.cellSelection)
    }

    func testAWritePlanIsNotBuiltFromRowsOfAViewThatIsBeingReplaced() {
        var edits = CellEdits()
        edits.edit("x", at: CellKey(row: 0, column: 1), original: "a")
        edits.deleteRow(1)
        let plan = WritePlan.build(edits: edits, rows: rows, columns: columns, table: "public.t",
                                   kind: .postgres, viewBusy: true)
        XCTAssertTrue(plan.statements.isEmpty, "no UPDATE and no DELETE from rows that may have moved")
        XCTAssertTrue(plan.warnings.contains { $0.contains(QueryTab.viewBusyMessage) })
        // The same queue, the same rows, a settled view: a plan.
        let settled = WritePlan.build(edits: edits, rows: rows, columns: columns, table: "public.t", kind: .postgres)
        XCTAssertEqual(settled.statements.map(\.kind), [.delete, .update])
    }

    func testAWritePlanWarnsForARowItCouldNotReadInsteadOfSkippingIt() {
        let handle = FakeResultHandle(rows: rows, columns: 2, phase: .complete)
        let store = StoreRows(handle: handle, columns: columns)
        handle.serve(view: 7)    // view 0 is stale: every read answers StaleView
        var edits = CellEdits()
        edits.edit("x", at: CellKey(row: 0, column: 1), original: "a")
        edits.deleteRow(1)
        let plan = WritePlan.build(edits: edits, rows: store, columns: columns, table: "public.t", kind: .postgres)
        XCTAssertTrue(plan.statements.isEmpty, "a row that cannot be read is not guessed at")
        XCTAssertEqual(plan.warnings.count, 2)
        XCTAssertTrue(plan.warnings.contains { $0.contains("row 2 was not deleted") })
        XCTAssertTrue(plan.warnings.contains { $0.contains("row 1 was not updated") })
    }

    func testApplyChangesRefusesAPlanWhileTheViewIsBusy() {
        let (tab, gate) = gatedTab()
        let model = AppModel()
        let engine = MockEngine()
        model.engine = engine
        tab.columnFilters = [0: .text("1")]
        spin { gate.begun == 1 }
        let plan = WritePlan(table: "public.t", statements: [])
        model.applyChanges(plan, in: tab)
        XCTAssertTrue(engine.calls.isEmpty)
        XCTAssertTrue(tab.logLines.contains { $0.text == QueryTab.viewBusyMessage })
        gate.finish(0)
        spin { !tab.viewBusy }
    }

    // MARK: Closing a tab (§19, TM-4)

    func testClosingATabStopsBothRunsAndLetsGoOfBothStores() throws {
        let model = AppModel()
        let tab = QueryTab(title: "Q")
        model.tabs = [tab]
        model.selectedTabID = tab.id
        let before = TestStores.liveStores
        tab.showRows(columns: columns, rows: rows)
        let active = try XCTUnwrap(tab.activeResult)
        let base = try TestStores.makeStore(columns: columns, rows: rows)
        tab.baseResult = ResultSlot(meta: PreviewResult(columns: columns, rowCount: 3, truncated: false,
                                                        queryID: nil, elapsedMS: 0), rows: base)
        let preview = MockRun(command: "preview")
        let export = MockRun(command: "export")
        tab.previewProcess = preview
        tab.process = export
        let token = tab.previewToken
        XCTAssertEqual(TestStores.liveStores, before + 2)

        model.closeTab(tab.id)

        XCTAssertTrue(preview.terminated, "a preview or explain still waiting on the server is stopped")
        XCTAssertTrue(export.terminated)
        XCTAssertTrue(active.isReleased)
        XCTAssertTrue(base.isReleased)
        XCTAssertNil(tab.activeResult)
        XCTAssertNil(tab.baseResult)
        XCTAssertFalse(tab.viewBusy)
        XCTAssertNotEqual(tab.previewToken, token, "events from the run that is winding down are dropped")
        XCTAssertEqual(TestStores.liveStores, before, "the engine's own count is back where it was")
    }

    func testClosingATabMidApplyLeavesNoGuardBehind() {
        let (tab, gate) = gatedTab()
        let model = AppModel()
        model.tabs = [tab]
        tab.columnFilters = [0: .text("1")]
        spin { gate.begun == 1 }
        XCTAssertTrue(tab.viewBusy)
        model.closeTab(tab.id)
        XCTAssertFalse(tab.viewBusy)
        gate.finish(0)
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        XCTAssertFalse(tab.viewBusy)
        XCTAssertNil(tab.activeResult)
    }

    func testSelectingAnotherTabGivesTheBackgroundTabsPagesBack() {
        let model = AppModel()
        model.newTab()
        let first = model.tabs[0], second = model.tabs[1]
        let handle = FakeResultHandle(rows: (0..<10).map { ["r\($0)"] }, columns: 1, phase: .complete)
        first.activeResult = StoreRows(handle: handle, columns: [Event.Column(name: "c", type: "text")])
        _ = first.activeResult?.cell(row: 0, column: 0, format: .raw)
        _ = first.activeResult?.cell(row: 0, column: 0, format: .raw)
        XCTAssertEqual(handle.calls.count, 1, "cached")
        model.selectTab(second.id)
        _ = first.activeResult?.cell(row: 0, column: 0, format: .raw)
        XCTAssertEqual(handle.calls.count, 2, "dropped when the tab went to the background")
    }

    // MARK: The engine

    func testAStoreThatIsNotTheEnginesOwnIsRefusedOutLoud() {
        let fake = StoreRows(handle: FakeResultHandle(), columns: [])
        var exit: (Int32, String)?
        let run = RustEngine().runIntoStore("preview", env: [:], store: fake, onEvent: { _ in },
                                            onExit: { exit = ($0, $1) })
        XCTAssertNil(run)
        XCTAssertEqual(exit?.0, -1)
        XCTAssertTrue(exit?.1.contains("not backed by the engine") == true, exit?.1 ?? "")
    }

    func testOnlyPreviewAndExplainRunIntoAStore() throws {
        let store = try RustEngine().makeResultStore()
        defer { store.release() }
        for command in ["export", "count", "objects"] {
            var exit: (Int32, String)?
            XCTAssertNil(RustEngine().runIntoStore(command, env: [:], store: store, onEvent: { _ in },
                                                   onExit: { exit = ($0, $1) }))
            XCTAssertEqual(exit?.0, -1, command)
        }
    }

    func testTheSharedHostIsConfiguredOnceAndTheSecondCallerGetsNothing() {
        TestStores.ensureConfigured()
        XCTAssertNil(RustEngine.ensureStoresConfigured(spillDir: nil, budgetBytes: 1 << 20))
        XCTAssertNotNil(RustEngine.storeStats())
        XCTAssertEqual(RustEngine.storeStats()?.spillEnabled, false, "the shared test host cannot spill")
    }

    func testTheSoftFileLimitIsRaisedToTheHardLimitOrFourThousandNinetySix() {
        var before = rlimit()
        getrlimit(RLIMIT_NOFILE, &before)
        let after = RustEngine.raiseFileLimit()
        XCTAssertGreaterThanOrEqual(after.rlim_cur, min(after.rlim_max, 4_096))
        XCTAssertGreaterThanOrEqual(after.rlim_cur, before.rlim_cur, "never lowered")
    }

    func testAStoreFromRowsThroughTheEngineHoldsTheRowsAndCountsAsLive() throws {
        let before = TestStores.liveStores
        let store = try RustEngine().storeFromRows(columns: columns, rows: rows)
        XCTAssertEqual(store.count, 3)
        XCTAssertEqual(store.fetched, 3)
        XCTAssertFalse(store.isLive, "a finished result has nothing to poll")
        XCTAssertEqual(TestStores.liveStores, before + 1)
        store.release()
        XCTAssertEqual(TestStores.liveStores, before)
    }
}

extension QueryTab {
    /// The metadata a `columns` event gives a tab, for tests that bring their own store.
    func showRowsPreviewMetadata(columns: [Event.Column], rowCount: Int) {
        preview = PreviewResult(columns: columns, rowCount: rowCount, truncated: false, queryID: nil, elapsedMS: 0)
    }
}
