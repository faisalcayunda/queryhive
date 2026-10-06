import Foundation
import QueryHiveFFI

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

/// The existing array-backed rows, wrapped in the seam.
final class ArrayRows: ResultRows, @unchecked Sendable {
    private let displayedRows: [[String?]]
    private let sizingRows: [[String?]]
    private let _columns: [Event.Column]
    private var _naturalCharCounts: [Int]?

    init(rows: [[String?]], sizing: [[String?]], columns: [Event.Column]) {
        self.displayedRows = rows
        self.sizingRows = sizing
        self._columns = columns
    }

    var count: Int { displayedRows.count }
    var fetched: Int { sizingRows.count }
    var columns: [Event.Column] { _columns }

    func row(at index: Int) -> [String?]? {
        displayedRows.indices.contains(index) ? displayedRows[index] : nil
    }

    func cell(row: Int, column: Int, format: ColumnFormat) -> CellText {
        // Three answers that must not look alike: a NULL is italic `nullDisplay`, an empty string
        // is `∅`, and a cell a short row does not carry is nothing at all. `valueAt` folds the
        // first and the third into one `nil`, which is how every NULL in the result came to be
        // drawn as an empty string (§8.2; parity item 15, "NULL miring").
        guard displayedRows.indices.contains(row) else { return CellText(text: "", flags: []) }
        let displayedRow = displayedRows[row]
        guard column < displayedRow.count else { return CellText(text: "", flags: []) }
        guard let value = displayedRow[column] else { return CellText(text: "", flags: [.null]) }
        return buildCellText(value: value, format: format, columnIndex: column)
    }
    
    func fullValue(row: Int, column: Int, format: ColumnFormat) -> String? {
        valueAt(row: row, column: column).flatMap { format.render($0, type: _columns[column].type) }
    }

    func rows(in range: Range<Int>, columns: [Int]) -> [[String?]] {
        range.compactMap { rowIndex in
            guard displayedRows.indices.contains(rowIndex) else { return nil }
            let row = displayedRows[rowIndex]
            return columns.compactMap { colIndex in
                colIndex < row.count ? row[colIndex] : nil
            }
        }
    }

    func naturalCharCounts() -> [Int] {
        if let cached = _naturalCharCounts { return cached }
        let counts = _columns.enumerated().map { (sourceIndex, _) -> Int in
            var maxCount = 0
            for row in sizingRows.prefix(200) {
                guard sourceIndex < row.count else { continue }
                if let value = row[sourceIndex] {
                    maxCount = max(maxCount, value.count)
                } else {
                    maxCount = max(maxCount, 4) // NULL counts as 4
                }
            }
            return min(maxCount, 64)
        }
        _naturalCharCounts = counts
        return counts
    }

    func distinctValues(column: Int) async -> DistinctSample {
        // The same rule as `distinct_values` in `view.rs`, with the same limit Swift hands it, so
        // the twin and the store answer alike: stop as soon as the picker could not use the answer.
        let limit = ColumnFilter.valuePickerLimit + 1
        var seen = Set<String>()
        var hasNull = false
        for row in sizingRows {
            if column < row.count, let value = row[column] {
                seen.insert(value.precomposedStringWithCanonicalMapping)
            } else {
                hasNull = true
            }
            if seen.count + (hasNull ? 1 : 0) > limit { return DistinctSample(values: [], more: true) }
        }
        return DistinctSample(values: (hasNull ? [nil] : []) + seen.sorted(), more: false)
    }

    private func valueAt(row: Int, column: Int) -> String? {
        guard displayedRows.indices.contains(row) else { return nil }
        let displayedRow = displayedRows[row]
        guard column < displayedRow.count else { return nil }
        return displayedRow[column]
    }

    private func buildCellText(value: String, format: ColumnFormat, columnIndex: Int) -> CellText {
        // Render with the given format, then take the first line up to prefixLimit UTF-16 units.
        let rendered = format.render(value, type: _columns[columnIndex].type)
        let firstLine = rendered.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: false).first
            .map(String.init) ?? rendered
        let truncated = firstLine.count > CellText.prefixLimit
        let text = String(firstLine.prefix(CellText.prefixLimit))
        var flags = CellFlags()
        if value.isEmpty { flags.insert(.empty) }
        if truncated { flags.insert(.truncated) }
        // numeric: from column type quirk (contains "int")
        let columnType = _columns[columnIndex].type
        if GridMetrics.isNumeric(type: columnType) { flags.insert(.numeric) }
        // openable: first non-whitespace char is { or [
        if let first = value.first(where: { !$0.isWhitespace }) {
            let s = String(first)
            if s == "{" || s == "[" {
                flags.insert(.openable)
            }
        }
        return CellText(text: text, flags: flags)
    }
}