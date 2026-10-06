import Foundation

struct CellFlags: OptionSet, Sendable {
    let rawValue: UInt8
    static let `null` = CellFlags(rawValue: 1)
    static let empty = CellFlags(rawValue: 2)
    static let truncated = CellFlags(rawValue: 4)
    static let numeric = CellFlags(rawValue: 8)
    static let openable = CellFlags(rawValue: 16)
}

struct CellText: Equatable, Sendable {
    /// First line of the display text (format applied), at most `prefixLimit` UTF-16 units.
    var text: String
    var flags: CellFlags
    static let prefixLimit = 1_024
}

/// The rows the grid draws: after filter, search and sort.
protocol ResultRows: AnyObject, RowReading {
    /// Rows the grid draws: after filter, search and sort.
    var count: Int { get }
    /// Rows fetched before filter and search (the footer's "12 of 40").
    var fetched: Int { get }
    /// Source order, as the engine sent them.
    var columns: [Event.Column] { get }

    /// Display text of one cell. Main thread only: StoreRows fills pages from here.
    func cell(row: Int, column: Int, format: ColumnFormat) -> CellText
    /// The whole value under `format`; `.raw` gives what an edit, a copy and an export see.
    func fullValue(row: Int, column: Int, format: ColumnFormat) -> String?
    /// Raw values for copy and write plans, in the order of `columns` (source indices).
    func rows(in range: Range<Int>, columns: [Int]) -> [[String?]]
    /// The same, but a failure to read is thrown instead of answered with nothing: the copy path and
    /// the write-plan builder refuse a partial answer.
    func rowsOrThrow(in range: Range<Int>, columns: [Int]) throws -> [[String?]]
    /// Per source column, the widest of the first 200 *fetched* rows in Characters, capped at 64
    /// (a count of 42 or more already hits the 320 pt clamp), NULL counting 4.
    func naturalCharCounts() -> [Int]
    /// The distinct values of a column over the fetched rows (§13.9): NULL first, then the rest
    /// sorted by NFC bytes. Past `ColumnFilter.valuePickerLimit + 1` of them the scan stops with
    /// `more` set and no values. The picker is browsable when `!more && values.count <= 10`.
    func distinctValues(column: Int) async -> DistinctSample

    /// Main thread, once per display tick while `isLive`. What the rows learned since the last
    /// call; an array answers "nothing, finished".
    func poll() -> PollResult
    /// Whether the rows can still change under the grid (streaming, or a view being applied).
    var isLive: Bool { get }
    /// The format of every column, in source order, before the grid rebuilds any text. Returns the
    /// source columns whose rendered text a reader may have cached under the old format.
    @discardableResult
    func prepare(formats: [ColumnFormat]) -> IndexSet
    /// Give back whatever the rows hold outside the process heap. Idempotent.
    func release()
}

extension ResultRows {
    func rowsOrThrow(in range: Range<Int>, columns: [Int]) throws -> [[String?]] { rows(in: range, columns: columns) }
    func poll() -> PollResult { PollResult(grewFrom: nil, finished: true) }
    var isLive: Bool { false }
    @discardableResult
    func prepare(formats: [ColumnFormat]) -> IndexSet { IndexSet() }
    func release() {}
}

/// What `poll()` found: the first row of a growth in the view the grid draws, and whether the
/// rows have stopped changing.
struct PollResult: Equatable {
    var grewFrom: Int?
    var finished: Bool
}

/// The answer to `distinctValues(column:)`: `values` is empty when `more`.
struct DistinctSample: Equatable {
    var values: [String?]
    var more: Bool
}
