import XCTest

@testable import QueryHive

/// The footer's "count all": what one event from a `count` run does to the tab.
///
/// This exists because the bug it pins was invisible to every other test. The engine sends the
/// total as `{"event": "count", "rows": 4321}` — `tests/golden/count/count.ndjson` has written
/// exactly that since the Python engine — and `EventDecodingTests` already proved the number
/// decodes into `rows`. What nothing checked was the reading: the model asked for a `count`
/// property that no engine writes, so the total stayed nil, the footer never showed it, and the
/// button went on offering to count a result it had already counted.
///
/// The mapping lives in `AppModel.applyCountEvent` rather than inside `countRows` so it can be
/// reached here at all: `Engine.current` is a `static let`, so a run cannot be scripted.
final class CountRowsTests: XCTestCase {
    override func setUp() {
        super.setUp()
        // Before any AppModel is built: its constructor reads connections.json.
        isolateConnectionStore()
    }

    private func tab() -> QueryTab {
        QueryTab(title: "Query 1")
    }

    func testTheCountEventSetsTheTotalFromRows() {
        // The one line this whole file is about. `rows` is where the number arrives.
        let model = AppModel()
        let tab = tab()

        model.applyCountEvent(Event(event: "count", rows: 4321), to: tab)

        XCTAssertEqual(tab.totalRows, 4321)
    }

    func testADoneEventsRowCountIsNotTheTotal() {
        // `rows` is also the integer on `done`, where it is the number a *preview* sent rather
        // than the number that exists. An ungated read would report the fetched count as the
        // total — the exact confusion the separate `count` command was introduced to avoid.
        let model = AppModel()
        let tab = tab()

        model.applyCountEvent(Event(event: "done", rows: 1000), to: tab)

        XCTAssertNil(tab.totalRows)
    }

    func testACountEventWithNoNumberLeavesTheTotalUnset() {
        // A `count` event that carries no `rows` is not a count of zero. Zero is a number the
        // server can report and the footer must show; nil is "nothing came back", and the
        // button has to stay so the user can ask again.
        let model = AppModel()
        let tab = tab()

        model.applyCountEvent(Event(event: "count"), to: tab)

        XCTAssertNil(tab.totalRows)
    }

    func testAnErrorEventBecomesTheCountError() {
        // A failed count has to say why rather than leave the button sitting there as if the
        // click never happened.
        let model = AppModel()
        let tab = tab()

        model.applyCountEvent(Event(event: "error", message: "coordinator refused"), to: tab)

        XCTAssertEqual(tab.countError, "coordinator refused")
        XCTAssertNil(tab.totalRows)
    }
}
