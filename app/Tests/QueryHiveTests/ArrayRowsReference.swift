import Foundation

@testable import QueryHive

/// The array-backed rows the grid read before the Rust store existed, kept as the reference twin
/// (blueprint D-25): `StoreRowsTests` compares the store with it cell by cell, and the ten
/// `ResultRowsTests` keep guarding the rules both must follow. Production never builds one.
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