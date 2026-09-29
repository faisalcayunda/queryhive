import Foundation

/// What the grid says where the rows would go, when there are none to draw.
///
/// A grid with an empty body is three different situations wearing one face. A run that has painted
/// its columns and not its rows yet, a statement that ran to the end and matched nothing, and a
/// filter that hid every row the statement returned all leave the same picture: the header standing
/// over a void. Only the footer told them apart, and the footer is where the eye goes last — so the
/// body says it, and this is the decision of which of the three it is saying.
///
/// The decision is here rather than in `ResultGrid` so it can be checked without a window. The
/// sentences and the spinner belong to the view.
enum GridPlaceholder: Equatable {
    /// A run in flight, with the sentence that goes beside its spinner.
    case loading(String)
    /// The statement ran to the end and the server sent no rows.
    case noRows
    /// Filters hid every fetched row; `hidden` of them are behind them.
    case filteredOut(hidden: Int)

    /// The sentence beside the spinner while a run is in flight, or `nil` when nothing is running.
    ///
    /// Explain is in here because it is a run too — same invocation, same wait, and its result lands
    /// in the same grid. It was missing, and the panel it left behind invited the user to press Run
    /// while they were already waiting for an answer they had asked for.
    static func inFlight(previewing: Bool, explaining: Bool) -> String? {
        if previewing { return "Running…" }
        if explaining { return "Explaining…" }
        return nil
    }

    /// Which of the three a body holding `shown` rows is showing, or `nil` when it holds at least
    /// one row and has nothing to explain.
    ///
    /// A run in flight wins over the other two, and has to: rows arrive a batch at a time, so a body
    /// that is empty *because more are still coming* must say so rather than report a result the
    /// server has not finished sending. `fetched` is what the run has returned so far with no filter
    /// applied, which is what tells an empty result from a filtered-out one.
    static func whenEmpty(shown: Int, fetched: Int, loading: String?) -> GridPlaceholder? {
        guard shown == 0 else { return nil }
        if let loading { return .loading(loading) }
        return fetched == 0 ? .noRows : .filteredOut(hidden: fetched)
    }
}
