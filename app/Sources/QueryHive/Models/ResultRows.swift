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
    /// Distinct values of a column over the fetched rows, in first-seen order, empty past
    /// `ColumnFilter.valuePickerLimit`. Synchronous now; W6 makes it `async`.
    func distinctValues(column: Int) -> [String?]
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
        guard let value = valueAt(row: row, column: column) else {
            return CellText(text: "", flags: [.empty])
        }
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

    func distinctValues(column: Int) -> [String?] {
        var seen = Set<String?>()
        var result: [String?] = []
        for row in sizingRows {
            guard column < row.count else { continue }
            let value = row[column]
            if seen.insert(value).inserted {
                result.append(value)
                if result.count >= ColumnFilter.valuePickerLimit { break }
            }
        }
        return result
    }

    private func valueAt(row: Int, column: Int) -> String? {
        guard displayedRows.indices.contains(row),
              let displayedRow = displayedRows[row] as? [String?],
              column < displayedRow.count else { return nil }
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