import XCTest

@testable import QueryHive

/// What a preview's `done{cancelled}` does to the tab: a stopped run says so, keeps partial
/// truncated semantics, and surfaces the engine's warnings.
final class StoppedRunTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    private let column = Event.Column(name: "n", type: "int")

    func testAnOrdinaryDoneIsNotStopped() {
        let tab = QueryTab(title: "Query 1")
        let stopped = AppModel.applyPreviewDone(Event(event: "done", rows: 2), columns: [column],
                                                rowCount: 2, to: tab)
        XCTAssertFalse(stopped)
        XCTAssertEqual(tab.preview?.summary, "2 rows")
        XCTAssertEqual(tab.logLines.last?.text, "2 rows returned")
    }

    func testAStopWithRowsSaysStoppedAndCountsAsPartial() {
        let tab = QueryTab(title: "Query 1")
        var event = Event(event: "done", rows: 3)
        event.cancelled = true
        let stopped = AppModel.applyPreviewDone(event, columns: [column],
                                                rowCount: 3, to: tab)
        XCTAssertTrue(stopped)
        XCTAssertEqual(tab.preview?.summary, "Stopped · 3 rows")
        XCTAssertEqual(tab.preview?.truncated, true, "a partial result must keep Sort on Server")
    }

    func testAStopBeforeAnyRowIsNotAnEmptySuccess() {
        let tab = QueryTab(title: "Query 1")
        var event = Event(event: "done", rows: 0)
        event.cancelled = true
        AppModel.applyPreviewDone(event, columns: [], rowCount: 0, to: tab)
        XCTAssertEqual(tab.preview?.stopped, true)
        XCTAssertEqual(tab.preview?.summary, "Stopped before any rows arrived")
        XCTAssertEqual(tab.preview?.truncated, false)
    }

    func testDoneWarningsReachTheLog() {
        let tab = QueryTab(title: "Query 1")
        var event = Event(event: "done", rows: 0)
        event.cancelled = true
        event.warnings = ["the server did not confirm the stop; the statement may still be running"]
        AppModel.applyPreviewDone(event, columns: [], rowCount: 0, to: tab)
        XCTAssertTrue(tab.logLines.contains { $0.text.contains("did not confirm the stop") })
    }
}

/// Stop has to reach the app as the engine's own `done{cancelled}`, not as a dropped stream.
@MainActor
final class StopDeliveryTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    private let doneLine = #"{"event":"done","rows":1,"cancelled":true}"#

    /// `terminate()` sets the stopped flag before the engine answers, so the sink used to drop the
    /// very event that says the run was stopped.
    func testAStoppedPreviewStillDeliversItsDone() {
        let run = RustRun(command: "preview")
        var seen: [Event] = []
        let delivered = expectation(description: "done delivered")
        let sink = Sink(handle: run, onEvent: { seen.append($0); delivered.fulfill() })
        run.terminate()
        sink.onEvent(line: doneLine)
        wait(for: [delivered], timeout: 2)
        XCTAssertEqual(seen.first?.cancelled, true)
    }

    func testAStoppedRunOfAnotherCommandStillDropsItsEvents() {
        let run = RustRun(command: "objects")
        var seen: [Event] = []
        let sink = Sink(handle: run, onEvent: { seen.append($0) })
        run.terminate()
        sink.onEvent(line: doneLine)
        let drained = expectation(description: "main queue drained")
        DispatchQueue.main.async { drained.fulfill() }
        wait(for: [drained], timeout: 2)
        XCTAssertTrue(seen.isEmpty)
    }

    /// The whole Run path with a scripted engine: partial rows survive and history says cancelled.
    func testAStoppedRunKeepsItsRowsAndRecordsCancelled() {
        let key = "recordsHistory"
        let original = UserDefaults.standard.object(forKey: key)
        addTeardownBlock {
            if let original { UserDefaults.standard.set(original, forKey: key) }
            else { UserDefaults.standard.removeObject(forKey: key) }
        }
        let engine = MockEngine()
        var columns = Event(event: "columns")
        columns.columns = [Event.Column(name: "n", type: "int")]
        var rows = Event(event: "rows")
        rows.data = [["1"], ["2"]]
        var done = Event(event: "done", rows: 2)
        done.cancelled = true
        engine.answer("preview", with: .events([columns, rows, done]))
        engine.answer("history_add", with: .events([]))
        engine.answer("history", with: .events([]))

        let model = AppModel()
        model.engine = engine
        model.recordsHistory = true
        let connection = Connection(id: UUID(), name: "Dev", color: .violet, kind: .postgres,
                                    host: "127.0.0.1", port: 5432, sslmode: "prefer",
                                    user: "qh", database: "qh", schema: "public", verify: false)
        model.connections = [connection]
        let tab = QueryTab(title: "Query 1")
        tab.connectionID = connection.id
        tab.sql = "SELECT n FROM t"
        model.tabs = [tab]
        model.selectedTabID = tab.id

        model.preview(tab)
        let finished = expectation(description: "run finished")
        func poll() {
            if !tab.previewing, engine.calls.contains(where: { $0.command == "history_add" }) {
                finished.fulfill()
            } else {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.01) { MainActor.assumeIsolated { poll() } }
            }
        }
        poll()
        wait(for: [finished], timeout: 5)

        XCTAssertEqual(tab.preview?.rowCount, 2, "the partial rows were thrown away")
        XCTAssertEqual(tab.preview?.stopped, true)
        XCTAssertNil(tab.previewError)
        XCTAssertEqual(engine.calls.first { $0.command == "history_add" }?.env["OUTCOME"], "cancelled")
    }
}
