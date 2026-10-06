import Foundation

/// Which engine produced the order the grid is showing: the server over the
/// whole result, or the grid over the rows it had already fetched.
enum SortOrigin: Equatable {
    case server
    case memory
}

/// The one source of truth for the grid's sort: what the chevron follows.
/// Replaces the old `gridSort`/`serverSort` pair, which could disagree.
struct ActiveSort: Equatable {
    /// By position: a result may repeat a name, the grid draws by position.
    var column: Int
    var direction: GridSort.Direction
    var origin: SortOrigin
}

/// Server-first routing: memory is only the fallback for a refused builder,
/// a plan on screen, or an object inspector's preview. Everywhere else the
/// server answers, because fetched rows cannot order the whole result.
enum SortRoute: Equatable {
    case server, memory
}

enum SortPolicy {
    static func route(builderRefused: Bool, showingPlan: Bool,
                      isObjectResult: Bool) -> SortRoute {
        (builderRefused || showingPlan || isObjectResult) ? .memory : .server
    }
}

/// A sort the result grid is applying, as a value the tests can reach.
///
/// The grid holds `[String?]`, so a sort has to settle two questions a column type would have
/// settled for it: what a NULL does, and whether two cells are numbers or text. Both are written
/// down here rather than left to `sorted(by:)`, because a grid that puts "10" before "9" is a grid
/// whose sort nobody trusts the second time.
///
/// This is an **in-memory** order over the rows already fetched. It does not re-run the query and
/// does not touch the statement, which is why the header has to say so: a row outside the limit is
/// not in this order at all.
struct GridSort: Equatable {
    enum Direction: String, Equatable {
        case ascending, descending

        /// The glyph the header draws beside the sorted column.
        var symbol: String { self == .ascending ? "chevron.up" : "chevron.down" }
    }

    /// The column, by position. A result may repeat a name and the grid draws by position.
    var column: Int
    var direction: Direction

    /// What a click on a column header does: an unsorted grid starts ascending, a second click
    /// reverses it, and a third clears back to the server's own order.
    ///
    /// The third state exists because the server's order is the only one that is not the grid's
    /// invention, and a click that could only ever add an order would leave no way back to the rows
    /// as they arrived.
    static func next(_ current: GridSort?, clickedColumn: Int,
                     firstDirection: Direction = .ascending) -> GridSort? {
        guard let current, current.column == clickedColumn else {
            return GridSort(column: clickedColumn, direction: firstDirection)
        }
        // The cycle is: the first direction, then the other, then off. Which one is "first" is the
        // Data pane's, and it also decides where the cycle ends — a fixed `ascending → descending →
        // off` would make a descending start unable to reach ascending at all, because its second
        // click would clear instead of flipping.
        let other: Direction = firstDirection == .ascending ? .descending : .ascending
        return current.direction == firstDirection
            ? GridSort(column: clickedColumn, direction: other)
            : nil
    }
}
