import Foundation
import QueryHiveFFI

/// A minimal row accessor for write plans, edit staging, and clipboard.
protocol RowReading {
    func row(at index: Int) -> [String?]?
}

extension Array: RowReading where Element == [String?] {
    func row(at index: Int) -> [String?]? {
        indices.contains(index) ? self[index] : nil
    }
}

/// A block of cells in the result grid, as a rectangle of row and column indices.
///
/// Normalised on construction, so a drag that went up and to the left describes the same block as
/// the same drag made down and to the right. The pointer is not asked to respect an order, and a
/// caller never has to check which corner it started from.
///
/// Indices are positions in the rows the grid is *drawing* — the filtered rows, not the fetched
/// ones — because that is what the user pointed at. A copy therefore reproduces what is on screen.
struct CellRange: Equatable {
    let top: Int
    let left: Int
    let bottom: Int
    let right: Int

    init(from anchor: (row: Int, column: Int), to focus: (row: Int, column: Int)) {
        top = min(anchor.row, focus.row)
        bottom = max(anchor.row, focus.row)
        left = min(anchor.column, focus.column)
        right = max(anchor.column, focus.column)
    }

    var rowCount: Int { bottom - top + 1 }
    var columnCount: Int { right - left + 1 }
    var cellCount: Int { rowCount * columnCount }

    func contains(row: Int, column: Int) -> Bool {
        row >= top && row <= bottom && column >= left && column <= right
    }

    /// Every position the block covers, row by row.
    ///
    /// Used by the row cache to work out which rows a selection change can have repainted: the
    /// union of the old block's positions and the new one's, minus the ones both agree on, is the
    /// set that has to be redrawn and nothing more.
    var allPositions: [CellPos] {
        var result: [CellPos] = []
        result.reserveCapacity(cellCount)
        for r in top...bottom {
            for c in left...right {
                result.append(CellPos(row: r, column: c))
            }
        }
        return result
    }
}

/// A single cell position in display coordinates.
struct CellPos: Equatable, Hashable { var row: Int; var column: Int }

/// The cell cursor: anchor and focus (P-24). W10-T1 makes this authoritative and draws the ring.
struct GridCursor: Equatable {
    var anchor: CellPos
    var focus: CellPos
    var range: CellRange { CellRange(from: (anchor.row, anchor.column), to: (focus.row, focus.column)) }
}

/// The column a horizontal position falls in, clamped at both ends: the gutter to the left and
/// the empty space past the last column both mean the nearest column, because a drag that
/// reaches them is asking to extend the selection to the edge, not to drop it.
func column(at x: CGFloat, geometry: GridColumnGeometry, last: Int) -> Int {
    geometry.clampedColumn(atX: x, last: last)
}

/// Which cell a pointer position is over.
func cell(at point: CGPoint, geometry: GridColumnGeometry, rowHeight: CGFloat, lastRow: Int, lastColumn: Int) -> (row: Int, column: Int) {
    let row = geometry.row(atY: point.y, rowHeight: rowHeight, count: lastRow + 1)
    let col = geometry.clampedColumn(atX: point.x, last: lastColumn)
    return (row: row, column: col)
}

/// The selected block as the text a spreadsheet reads back as a table.
///
/// Tabs between cells and one newline between rows: that is the shape Excel, Numbers and Sheets
/// parse into a grid when it is pasted, which is the whole point of copying out of the result grid.
/// A CSV would need a delimiter choice and would not paste into a spreadsheet as columns without an
/// import step; this needs neither.
enum GridClipboard {
    /// `rows` are the rows on screen, `headers` the column names, and `selection` the block to take.
    ///
    /// `withHeaders` prepends the selected columns' names, so the pasted table carries its own
    /// labels. It is off by default: a user who selected a rectangle of data asked for that
    /// rectangle, and a header they did not select is a row they did not ask for.
    static func text(rows: some RowReading, headers: [String], selection: CellRange,
                     withHeaders: Bool) -> String {
        var lines: [String] = []
        if withHeaders {
            lines.append((selection.left...selection.right)
                .map { escape(header(at: $0, in: headers)) }
                .joined(separator: "\t"))
        }
        for row in selection.top...selection.bottom {
            let source = rows.row(at: row) ?? []
            lines.append((selection.left...selection.right)
                .map { column -> String in
                    // A short row is a blank cell, not a shifted one: the grid draws by position,
                    // and a copy that skipped the missing cell would move every value after it
                    // under the wrong heading.
                    guard column < source.count, let value = source[column] else { return "" }
                    return escape(value)
                }
                .joined(separator: "\t"))
        }
        return lines.joined(separator: "\n")
    }

    private static func header(at index: Int, in headers: [String]) -> String {
        headers.indices.contains(index) ? headers[index] : ""
    }

    /// The selected block as text, straight off the seam, with the source-column rule applied.
    ///
    /// This is the grid's own copy path, and the reason it lives here rather than in the view: a
    /// copy is built from the **source** columns the selection covers, in the server's own order and
    /// under the server's own names. Hiding, reordering and renaming are rendering-only, so a
    /// reordered grid can put two columns that are not adjacent in the source between the block's
    /// corners; taking the source indices and sorting them is what keeps the copy honest about which
    /// values it holds, rather than copying a source range that reaches through a column the user
    /// never selected.
    ///
    /// `nil` when nothing is selected or the block covers no source column — the caller has nothing
    /// to put on the pasteboard and should not clear it.
    static func text(result: any ResultRows, selection: CellRange?, visible: [Int],
                     withHeaders: Bool) -> String? {
        guard let selection else { return nil }
        let sources = (selection.left...selection.right)
            .compactMap { visible.indices.contains($0) ? visible[$0] : nil }
            .sorted()
        guard !sources.isEmpty else { return nil }
        let headers = sources.map { source in
            result.columns.indices.contains(source) ? result.columns[source].name : ""
        }
        let rows = result.rows(in: selection.top..<selection.bottom + 1, columns: sources)
        guard !rows.isEmpty else { return withHeaders ? headers.joined(separator: "\t") : nil }
        // The projected rows are already narrowed to the selected columns, so the block this reads
        // is a rectangle from the origin: the range's own offsets would index past every row.
        let block = CellRange(from: (row: 0, column: 0),
                              to: (row: rows.count - 1, column: sources.count - 1))
        return text(rows: rows, headers: headers, selection: block, withHeaders: withHeaders)
    }

    /// A value that itself holds a tab, a newline or a quote is wrapped in quotes and its own quotes
    /// are doubled — the rule every spreadsheet reads. Without it one such value silently becomes
    /// two columns and shifts the rest of the row, which is worse than the quoting it costs.
    ///
    /// A NULL is the empty string: the paste target has no NULL to receive one, and a literal
    /// "null" would be a claim that the database stored those four letters.
    private static func escape(_ value: String) -> String {
        let needsQuoting = value.contains("\t") || value.contains("\n")
            || value.contains("\r") || value.contains("\"")
        guard needsQuoting else { return value }
        return "\"" + value.replacingOccurrences(of: "\"", with: "\"\"") + "\""
    }
}
