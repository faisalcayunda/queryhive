import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// A `DatabaseEngine` whose `preview` and `explain` stream the way the FFI engine does: the rows are in the
/// store before the events that announce them, and every event is its own hop to the main queue
/// (`Sink.onEvent`). `MockEngine` delivers a whole script in one block, which is the one thing
/// these tests cannot use: the question is what the grid does *between* two hops.
final class StreamingEngine: DatabaseEngine, @unchecked Sendable {
    struct Script {
        var columns: [Event.Column] = [Event.Column(name: "n", type: "text")]
        var rows: [[String?]] = (0..<50).map { ["row \($0)"] }
        /// How long the engine thread is busy before it answers: the query's own time.
        var delay: TimeInterval = 0.03
        /// Between `columns`, `progress` and `done`: the pump decoding and pushing the batch.
        var gap: TimeInterval = 0.0005
        /// Answer with `done{cancelled}` instead of `done`.
        var cancelled = false
        /// Answer with an `error` event and a failing exit instead of rows.
        var failure: String?
    }

    private let lock = NSLock()
    private var _script = Script()
    private var _made: [StoreRows] = []
    private var _previews = 0

    var script: Script {
        get { lock.lock(); defer { lock.unlock() }; return _script }
        set { lock.lock(); _script = newValue; lock.unlock() }
    }
    /// Every store `makeResultStore` handed out, in order: the first is the first Run's.
    var stores: [StoreRows] { lock.lock(); defer { lock.unlock() }; return _made }
    /// How many `preview` runs started.
    var previews: Int { lock.lock(); defer { lock.unlock() }; return _previews }

    func makeResultStore() throws -> StoreRows {
        let store = StoreRows(handle: FakeResultHandle())
        lock.lock(); _made.append(store); lock.unlock()
        return store
    }

    func storeFromRows(columns: [Event.Column], rows: [[String?]]) throws -> StoreRows {
        try TestStores.makeStore(columns: columns, rows: rows)
    }

    @discardableResult
    func runIntoStore(_ command: String, env: [String: String], store: StoreRows,
                      onEvent: @escaping (Event) -> Void,
                      onExit: @escaping (_ status: Int32, _ stderr: String) -> Void) -> (any EngineRun)? {
        guard command == "preview" || command == "explain", let fake = store.handle as? FakeResultHandle else {
            return run(command, env: env, onEvent: onEvent, onExit: onExit)
        }
        lock.lock(); _previews += 1; lock.unlock()
        let script = self.script
        let run = MockRun(command: command)
        func main(_ body: @escaping () -> Void) { DispatchQueue.main.async(execute: body) }
        DispatchQueue.global().async {
            Thread.sleep(forTimeInterval: script.delay)
            if let failure = script.failure {
                var error = Event(event: "error")
                error.message = failure
                main { onEvent(error) }
                main { onExit(1, failure) }
                return
            }
            // The pump pushes the first batch into the store, then says so.
            fake.append(script.rows)
            var columns = Event(event: "columns")
            columns.columns = script.columns
            main { onEvent(columns) }
            Thread.sleep(forTimeInterval: script.gap)
            main { onEvent(Event(event: "progress", rows: script.rows.count)) }
            Thread.sleep(forTimeInterval: script.gap)
            fake.setPhase(script.cancelled ? .cancelled : .complete)
            var done = Event(event: "done", rows: script.rows.count)
            done.cancelled = script.cancelled ? true : nil
            main { onEvent(done) }
            main { onExit(0, "") }
        }
        return run
    }

    @discardableResult
    func run(_ command: String, env: [String: String],
             onEvent: @escaping (Event) -> Void,
             onExit: @escaping (_ status: Int32, _ stderr: String) -> Void) -> (any EngineRun)? {
        DispatchQueue.main.async { onExit(0, "") }
        return MockRun(command: command)
    }

    func terminateAll() {}
    func runBlocking(_ command: String, env: [String: String]) {}
}

/// `ResultGrid` in a window, over a tab that a `StreamingEngine` fills: the real SwiftUI tree, the
/// real `NSTableView`, and a main run loop that spins between the engine's hops.
@MainActor
final class HostedGrid {
    let engine: StreamingEngine
    let model: AppModel
    let tab: QueryTab
    let window: NSWindow
    private let host: NSHostingView<AnyView>

    init(engine: StreamingEngine = StreamingEngine(), size: CGSize = CGSize(width: 1000, height: 600)) {
        self.engine = engine
        let model = AppModel()
        model.engine = engine
        let connection = Connection(id: UUID(), name: "Dev", color: .violet, kind: .postgres,
                                    host: "127.0.0.1", port: 5432, sslmode: "prefer",
                                    user: "qh", database: "qh", schema: "public", verify: false)
        model.connections = [connection]
        let tab = QueryTab(title: "Query 1")
        tab.connectionID = connection.id
        tab.sql = "SELECT n FROM t"
        model.tabs = [tab]
        model.selectedTabID = tab.id
        self.model = model
        self.tab = tab
        host = NSHostingView(rootView: AnyView(ResultGrid(tab: tab).environment(model)
            .frame(width: size.width, height: size.height)))
        host.frame = CGRect(origin: .zero, size: size)
        window = NSWindow(contentRect: host.frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = host
        window.orderFrontRegardless()
    }

    func close() {
        window.contentView = nil
        window.close()
    }

    /// The grid's table, or `nil` while the panel shows something else (the status sentence).
    var table: GridTableView? { Self.find(GridTableView.self, in: host) }

    /// The first view of this kind, anywhere in the panel, that `matching` accepts.
    func first<T: NSView>(_ type: T.Type, matching accepted: (T) -> Bool = { _ in true }) -> T? {
        func walk(_ view: NSView) -> T? {
            if let hit = view as? T, accepted(hit) { return hit }
            for sub in view.subviews { if let hit = walk(sub) { return hit } }
            return nil
        }
        return walk(host)
    }

    func has<T: NSView>(_ type: T.Type, matching accepted: (T) -> Bool = { _ in true }) -> Bool {
        first(type, matching: accepted) != nil
    }

    /// How visible a view is: the least `alphaValue` on its way up to the window. SwiftUI's
    /// `.opacity` is an `alphaValue` on a view it wraps around what it veils.
    static func visibility(of view: NSView) -> CGFloat {
        var alpha: CGFloat = 1
        var current: NSView? = view
        while let next = current { alpha = min(alpha, next.alphaValue); current = next.superview }
        return alpha
    }

    static func find<T: NSView>(_ type: T.Type, in view: NSView) -> T? {
        if let hit = view as? T { return hit }
        for sub in view.subviews { if let hit = find(type, in: sub) { return hit } }
        return nil
    }

    func spin(_ seconds: TimeInterval) {
        RunLoop.current.run(until: Date().addingTimeInterval(seconds))
    }

    /// Spin the loop until `condition` holds or `timeout` passes; the answer is whether it held.
    @discardableResult
    func wait(_ timeout: TimeInterval = 5, until condition: () -> Bool) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if condition() { return true }
            RunLoop.current.run(until: Date().addingTimeInterval(0.002))
        }
        return condition()
    }

    /// Press Run and wait until the run has ended and its result has been drawn.
    func run() {
        model.preview(tab)
        wait { !tab.previewing && (tab.preview != nil || tab.previewError != nil) }
        spin(0.05)
    }
}

/// The grid is built when a Run starts and lives through the Runs after it (W8-F1).
///
/// The decision behind it (W8-T2 §3): the engine's share of the time to the first row is about 20
/// ms, and the rest was the main thread building the grid twice after the engine had finished.
/// These tests pin the structure that removed the work, not a number: the number is the bench's.
@MainActor
final class FirstPaintTests: XCTestCase {
    private var grids: [HostedGrid] = []

    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    override func tearDown() {
        grids.forEach { $0.close() }
        grids = []
        PerfSignposts.recording = false
        PerfSignposts.clearStages()
        super.tearDown()
    }

    private func hosted(_ script: StreamingEngine.Script = .init()) -> HostedGrid {
        let engine = StreamingEngine()
        engine.script = script
        let grid = HostedGrid(engine: engine)
        grids.append(grid)
        return grid
    }

    func testTheTableIsBuiltWhileTheEngineIsStillWorking() throws {
        var script = StreamingEngine.Script()
        script.delay = 0.5
        let grid = hosted(script)
        XCTAssertNil(grid.table, "nothing has run yet: the panel says Press Run")

        grid.model.preview(grid.tab)
        grid.spin(0.15)

        XCTAssertTrue(grid.tab.previewing, "the engine is still busy")
        XCTAssertNil(grid.tab.preview, "and has sent no columns")
        let table = try XCTUnwrap(grid.table, "the table was not built until the columns arrived")
        XCTAssertNotNil(table.window, "built, and in the window")
        XCTAssertEqual(table.numberOfRows, 0)
        XCTAssertNotNil(grid.tab.pollHook, "the coordinator is wired to the tab before any row exists")
        grid.wait { !grid.tab.previewing }
    }

    func testOneTableCarriesARunFromItsStartToItsEnd() throws {
        let grid = hosted()
        grid.model.preview(grid.tab)
        grid.spin(0.01)
        let first = try XCTUnwrap(grid.table)

        grid.run()

        XCTAssertTrue(grid.table === first, "the first rows threw the table away and built another")
        XCTAssertEqual(first.numberOfRows, 50)
        XCTAssertEqual(grid.tab.preview?.rowCount, 50)
    }

    func testARunAfterARunKeepsTheTable() throws {
        let grid = hosted()
        grid.run()
        let first = try XCTUnwrap(grid.table)
        let firstStore = try XCTUnwrap(grid.tab.activeResult)

        var again = grid.engine.script
        again.rows = (0..<7).map { ["again \($0)"] }
        grid.engine.script = again
        grid.run()

        XCTAssertEqual(grid.engine.previews, 2)
        XCTAssertTrue(grid.table === first, "Run again built a new table")
        XCTAssertTrue(firstStore.isReleased, "the first result's store is let go")
        let second = try XCTUnwrap(grid.tab.activeResult)
        XCTAssertFalse(second === firstStore)
        XCTAssertEqual(first.numberOfRows, 7, "the table is drawing the second result")
        XCTAssertTrue(first.coordinator?.rows === second, "the table still holds the first store")
    }

    func testAnEmptyBodyAndRowsAreOneTableOfTwoHeights() throws {
        // The placeholder used to be a second `ResultGridTable` in the other branch of an `if`.
        var script = StreamingEngine.Script()
        script.delay = 0.05
        let grid = hosted(script)
        grid.model.preview(grid.tab)
        // The columns have come and no row has been taken by the table yet.
        grid.wait { grid.tab.preview != nil }
        let table = try XCTUnwrap(grid.table)
        grid.wait { !grid.tab.previewing }
        grid.spin(0.05)

        XCTAssertTrue(grid.table === table)
        let scroll = try XCTUnwrap(table.enclosingScrollView)
        XCTAssertGreaterThan(scroll.frame.height, 200, "with rows the table fills the panel")
    }

    func testAHeaderOnlyTableIsOneHeaderTall() throws {
        var script = StreamingEngine.Script()
        script.rows = []
        let grid = hosted(script)
        grid.run()

        let table = try XCTUnwrap(grid.table)
        let scroll = try XCTUnwrap(table.enclosingScrollView)
        let header = GridMetrics.headerHeight(fontSize: DataPreferences.shared.gridFontSize)
        XCTAssertEqual(scroll.frame.height, header, accuracy: 1.5,
                       "a result with no rows keeps the header's height and the sentence under it")
    }

    func testExplainBuildsTheTableToo() throws {
        let grid = hosted()
        grid.model.explain(grid.tab)
        grid.spin(0.01)
        let table = try XCTUnwrap(grid.table, "Explain waited for the columns too")

        grid.wait { !grid.tab.explaining && grid.tab.preview != nil }
        grid.spin(0.05)

        XCTAssertTrue(grid.tab.showingPlan)
        XCTAssertTrue(grid.table === table)
        XCTAssertEqual(table.numberOfRows, 50)
    }

    func testAFailedRunLeavesTheErrorAndNoTable() {
        var script = StreamingEngine.Script()
        script.failure = "relation \"t\" does not exist"
        let grid = hosted(script)
        grid.model.preview(grid.tab)
        grid.wait { !grid.tab.previewing && grid.tab.previewError != nil }
        grid.spin(0.05)

        XCTAssertEqual(grid.tab.previewError, "relation \"t\" does not exist")
        XCTAssertNil(grid.table, "a table stood over the error banner")
    }

    func testAStatementThatReturnsNoColumnsLeavesNoTable() {
        var script = StreamingEngine.Script()
        script.columns = []
        script.rows = []
        let grid = hosted(script)
        grid.run()

        XCTAssertNil(grid.table, "a write has nothing to put in a grid")
        XCTAssertNil(grid.tab.activeResult)
    }

    func testTheSpinnerCoversATableThatIsBuiltButEmpty() throws {
        var script = StreamingEngine.Script()
        script.delay = 0.4
        let grid = hosted(script)
        grid.model.preview(grid.tab)
        grid.spin(0.1)

        let table = try XCTUnwrap(grid.table)
        XCTAssertTrue(grid.has(NSProgressIndicator.self), "the spinner is the panel while the engine works")
        // The toolbar is built behind it, and cannot be seen, clicked or reached by VoiceOver.
        let search = try XCTUnwrap(grid.first(NSTextField.self) { $0.placeholderString == "Search all columns" })
        XCTAssertEqual(HostedGrid.visibility(of: search), 0, "a search field shows over a result that is not there")

        grid.wait { !grid.tab.previewing }
        grid.spin(0.05)
        XCTAssertTrue(grid.table === table)
        let after = try XCTUnwrap(grid.first(NSTextField.self) { $0.placeholderString == "Search all columns" })
        XCTAssertEqual(HostedGrid.visibility(of: after), 1, "the result's toolbar is still under its veil")
    }

    func testARunDoesNotMoveTheTableOutOfItsWindow() throws {
        let grid = hosted()
        grid.run()
        let table = try XCTUnwrap(grid.table)

        PerfSignposts.recording = true
        grid.run()

        XCTAssertTrue(grid.table === table)
        // `runBegin` clears the stamp, and the table stamps it whenever it enters a window.
        XCTAssertNil(PerfSignposts.time(of: .gridAttached),
                     "the table left its window and came back: SwiftUI re-parented it")
    }

    // MARK: A released store

    func testAReleasedStoreHasNoRowsToDraw() throws {
        let store = try TestStores.makeStore(columns: [Event.Column(name: "n", type: "text")],
                                             rows: [["a"], ["b"]])
        XCTAssertEqual(store.count, 2)
        store.release()
        XCTAssertEqual(store.count, 0)
        XCTAssertFalse(store.isLive)
    }

    func testNoRowIsDrawnFromAStoreThatWasReleasedUnderTheTable() throws {
        let grid = hosted()
        grid.run()
        let table = try XCTUnwrap(grid.table)
        let store = try XCTUnwrap(grid.tab.activeResult)

        PerfSignposts.recording = true
        PerfSignposts.clearStages()
        table.needsDisplay = true
        table.displayIfNeeded()
        XCTAssertNotNil(PerfSignposts.time(of: .firstDraw), "a store with rows is drawn (the control)")

        PerfSignposts.clearStages()
        store.release()
        table.noteNumberOfRowsChanged()
        table.needsDisplay = true
        table.displayIfNeeded()
        XCTAssertNil(PerfSignposts.time(of: .firstDraw), "a released store was drawn")
        XCTAssertEqual(table.numberOfRows, 0)
    }

    // MARK: The stamps

    func testARunLeavesTheStagesInTheOrderTheyHappen() throws {
        PerfSignposts.recording = true
        let grid = hosted()
        grid.run()
        grid.wait { PerfSignposts.time(of: .firstPaint) != nil }

        let stamps = PerfSignposts.stampSnapshot()
        func at(_ stage: PerfSignposts.Stage) throws -> CFAbsoluteTime {
            try XCTUnwrap(stamps[stage.rawValue], "\(stage.rawValue) was never stamped")
        }
        let run = try at(.run), attached = try at(.gridAttached), columns = try at(.columns)
        let firstRows = try at(.firstRows), firstDraw = try at(.firstDraw), paint = try at(.firstPaint)
        XCTAssertLessThanOrEqual(run, attached, "the table is attached after the Run starts, before its columns")
        XCTAssertLessThanOrEqual(attached, columns)
        XCTAssertLessThanOrEqual(columns, firstDraw)
        XCTAssertLessThanOrEqual(firstRows, firstDraw)
        XCTAssertLessThanOrEqual(firstDraw, paint)
    }

    // MARK: The cost, for whoever wants to see it

    /// The main thread's share of a Run, in the test host: not a gate, a way to repeat the numbers
    /// in `docs/architecture/blueprints/w8-first-paint.md` §3. Off unless `QH_FIRST_PAINT_PROBE=1`.
    ///
    /// Two clocks per Run: wall time from Run to the first paint, and the CPU the main thread spent
    /// from the `columns` hop to it. The second is what a loaded machine cannot move, so it is the
    /// one to compare between two builds. A debug build in a test host is slower than the app, and
    /// the engine here is a sleep, so the numbers say how much work moved, not what TTFR is.
    func testTheMainThreadCostOfARun() throws {
        guard ProcessInfo.processInfo.environment["QH_FIRST_PAINT_PROBE"] == "1" else {
            throw XCTSkip("set QH_FIRST_PAINT_PROBE=1 to print the main thread's cost of a Run")
        }
        PerfSignposts.recording = true
        func median(_ values: [Double]) -> Double { values.sorted()[values.count / 2] }
        let cases: [(again: Bool, rows: Int, columns: Int, engine: TimeInterval)] = [
            (false, 1_000, 10, 0.020), (false, 10_000, 10, 0.020), (false, 1_000, 100, 0.020),
            (true, 1_000, 10, 0.020), (true, 1_000, 10, 0.005),
        ]
        for item in cases {
            var script = StreamingEngine.Script()
            script.columns = (0..<item.columns).map { Event.Column(name: "col\($0)", type: "text") }
            script.rows = (0..<item.rows).map { row in (0..<item.columns).map { "v\(row)_\($0)" } }
            script.delay = item.engine
            let probe = CPUProbeEngine(StreamingEngine())
            probe.inner.script = script
            let grid = HostedGrid(engine: probe.inner)
            grid.model.engine = probe
            var wall: [Double] = [], cpu: [Double] = []
            var stages: [String: [Double]] = [:]
            for sample in 0..<22 {
                if !item.again || sample == 0 {
                    grid.tab.releaseResults()
                    grid.tab.preview = nil
                    grid.spin(0.2)
                }
                PerfSignposts.clearStages()
                grid.model.preview(grid.tab)
                grid.wait(8) { PerfSignposts.time(of: .firstPaint) != nil && !grid.tab.previewing }
                let end = mainThreadCPUSeconds()
                let stamps = PerfSignposts.stampSnapshot()
                if sample >= 2, let run = stamps["run"], let paint = stamps["firstPaint"] {
                    wall.append((paint - run) * 1000)
                    cpu.append((end - probe.cpuAtColumns) * 1000)
                    for stage in PerfSignposts.Stage.allCases where stage != .run {
                        if let at = stamps[stage.rawValue] { stages[stage.metric, default: []].append((at - run) * 1000) }
                    }
                }
                grid.spin(0.15)
            }
            grid.close()
            // Each stage, in ms after Run, in the order they usually come (the engine's own are the
            // double's, so `engine_*` is absent): the decomposition W8-T2 §3 asks for.
            let order = stages.sorted { median($0.value) < median($1.value) }
                .map { String(format: "%@ %.1f", $0.key, median($0.value)) }.joined(separator: "  ")
            print(String(format: "PROBE again=%@ %dx%d engine=%dms  wall run->paint p50 %.1f ms  main CPU columns->paint p50 %.1f ms  |  ",
                         item.again ? "yes" : "no", item.rows, item.columns, Int(item.engine * 1000),
                         median(wall), median(cpu)) + order)
        }
    }
}

/// The main thread's CPU seconds, from the kernel: what a loaded machine cannot move.
func mainThreadCPUSeconds() -> Double {
    var info = thread_basic_info()
    var count = mach_msg_type_number_t(MemoryLayout<thread_basic_info>.size / MemoryLayout<integer_t>.size)
    let port = mach_thread_self()
    defer { mach_port_deallocate(mach_task_self_, port) }
    let status = withUnsafeMutablePointer(to: &info) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
            thread_info(port, thread_flavor_t(THREAD_BASIC_INFO), $0, &count)
        }
    }
    guard status == KERN_SUCCESS else { return .nan }
    return Double(info.user_time.seconds) + Double(info.user_time.microseconds) / 1e6
        + Double(info.system_time.seconds) + Double(info.system_time.microseconds) / 1e6
}

/// A `StreamingEngine` that notes the main thread's CPU clock right before the `columns` event.
final class CPUProbeEngine: DatabaseEngine, @unchecked Sendable {
    let inner: StreamingEngine
    nonisolated(unsafe) private(set) var cpuAtColumns = 0.0
    init(_ inner: StreamingEngine) { self.inner = inner }

    func run(_ command: String, env: [String: String], onEvent: @escaping (Event) -> Void,
             onExit: @escaping (Int32, String) -> Void) -> (any EngineRun)? {
        inner.run(command, env: env, onEvent: onEvent, onExit: onExit)
    }
    func terminateAll() {}
    func runBlocking(_ command: String, env: [String: String]) {}
    func makeResultStore() throws -> StoreRows { try inner.makeResultStore() }
    func storeFromRows(columns: [Event.Column], rows: [[String?]]) throws -> StoreRows {
        try inner.storeFromRows(columns: columns, rows: rows)
    }
    func runIntoStore(_ command: String, env: [String: String], store: StoreRows,
                      onEvent: @escaping (Event) -> Void,
                      onExit: @escaping (Int32, String) -> Void) -> (any EngineRun)? {
        inner.runIntoStore(command, env: env, store: store, onEvent: { [self] event in
            if event.event == "columns" { cpuAtColumns = mainThreadCPUSeconds() }
            onEvent(event)
        }, onExit: onExit)
    }
}
