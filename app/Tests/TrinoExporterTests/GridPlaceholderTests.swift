import XCTest

@testable import QueryHive

/// What the grid says when it has no rows to draw.
///
/// A blank grid is three situations wearing one face, and the bug this covers was that two of them —
/// a run still arriving and a run still being explained — were drawn as a result that had finished
/// and found nothing. Cases that reach the engine are covered by the snapshot scenes
/// (`--scene grid-empty`, `grid-loading`, `grid-filtered`); what is checked here is which of the
/// three the body is told it is in, which is pure and does not need a window.
final class GridPlaceholderTests: XCTestCase {

    // MARK: A body with rows in it

    func testAGridWithRowsInItSaysNothing() {
        // The placeholder is what stands *instead* of the rows. One row on screen and there is
        // nothing to explain, whatever else is true.
        XCTAssertNil(GridPlaceholder.whenEmpty(shown: 1, fetched: 1, loading: nil))
        XCTAssertNil(GridPlaceholder.whenEmpty(shown: 250, fetched: 1_000, loading: nil))
        XCTAssertNil(GridPlaceholder.whenEmpty(shown: 250, fetched: 1_000, loading: "Running…"))
    }

    // MARK: The three empty bodies

    func testAStatementThatMatchedNothingSaysSo() {
        // Fetched zero is the server's own answer: the statement ran to the end and returned no
        // rows. Nothing is running and nothing is hidden.
        XCTAssertEqual(GridPlaceholder.whenEmpty(shown: 0, fetched: 0, loading: nil), .noRows)
    }

    func testAFilterThatHidEveryRowNamesHowManyItHid() {
        // The rows are here, under a filter. The count is what makes the difference sayable: a body
        // that only said "no rows" would have the user believing the query, not the filter.
        XCTAssertEqual(GridPlaceholder.whenEmpty(shown: 0, fetched: 12, loading: nil),
                       .filteredOut(hidden: 12))
    }

    // MARK: A run in flight

    func testARunThatHasSentNoRowsYetIsLoadingRatherThanEmpty() {
        // The engine paints the header from its first event and the rows after, so there is a real
        // moment where the grid holds columns and no rows. Reporting that as an empty result is
        // reporting an answer the server has not given.
        XCTAssertEqual(GridPlaceholder.whenEmpty(shown: 0, fetched: 0, loading: "Running…"),
                       .loading("Running…"))
    }

    func testARunInFlightWinsOverAFilterThatHidEverything() {
        // Rows arriving into a body that says "no rows match the filters" is the reading this
        // ordering exists to prevent: the count is of a previous fetch, and the run under way is
        // about to replace the rows those filters were built from.
        XCTAssertEqual(GridPlaceholder.whenEmpty(shown: 0, fetched: 40, loading: "Running…"),
                       .loading("Running…"))
    }

    // MARK: The sentences a run is given

    func testARunningPreviewIsNamedRunning() {
        XCTAssertEqual(GridPlaceholder.inFlight(previewing: true, explaining: false), "Running…")
    }

    func testExplainIsARunToo() {
        // Explain is a run with the same wait and the same grid at the end of it, and it was
        // missing from this list: the panel it left behind said "Press Run to see the rows" to a
        // user who had just asked for something else and was waiting for it.
        XCTAssertEqual(GridPlaceholder.inFlight(previewing: false, explaining: true), "Explaining…")
    }

    func testAnIdleGridHasNoSentenceToShow() {
        // The untouched tab, and the finished one. Both are silent, and what the body says for them
        // is the view's business, not a run's.
        XCTAssertNil(GridPlaceholder.inFlight(previewing: false, explaining: false))
    }

    func testAPreviewBeingExplainedIsStillRunning() {
        // The model cannot start one while the other is in flight, so this is precedence written
        // down rather than a state to expect. A preview is the flow the tokens went to, and if the
        // two ever could overlap, "Running…" is the truthful of the two.
        XCTAssertEqual(GridPlaceholder.inFlight(previewing: true, explaining: true), "Running…")
    }
}
