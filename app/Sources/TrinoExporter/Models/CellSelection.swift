import Foundation

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
}

/// Where a pointer position lands in the result grid, as a row and column index.
///
/// A value of its own so the arithmetic that turns a drag into a cell can be tested. It is the part
/// of drag-to-select that is easy to get subtly wrong and impossible to see: a mapping that is one
/// column off at the far end of a wide result looks exactly like a mapping that is right, until
/// somebody counts the cells.
struct GridGeometry {
    /// The row-number column's drawn width, which no data column sits under.
    let gutterWidth: CGFloat
    /// The horizontal padding a cell carries on each side, so a column is drawn at its measured
    /// width plus twice this.
    let cellPadding: CGFloat
    /// The height every row is drawn at.
    let rowHeight: CGFloat

    /// The column a horizontal position falls in, clamped at both ends: the gutter to the left and
    /// the empty space past the last column both mean the nearest column, because a drag that
    /// reaches them is asking to extend the selection to the edge, not to drop it.
    func column(at x: CGFloat, widths: [CGFloat], last: Int) -> Int {
        var edge = gutterWidth
        for (index, width) in widths.enumerated() {
            if x < edge + width + cellPadding * 2 { return index }
            edge += width + cellPadding * 2
        }
        return last
    }

    /// Which cell a pointer position is over, given the row the drag started in.
    ///
    /// The position is in the *starting row's* own space and the gesture keeps reporting after the
    /// pointer has left that row, so the row under the pointer is the starting row plus however many
    /// row-heights it has travelled. Everything is clamped into the grid.
    func cell(at point: CGPoint, inRow row: Int, widths: [CGFloat],
              lastRow: Int, lastColumn: Int) -> (row: Int, column: Int) {
        let travelled = Int(floor(point.y / rowHeight))
        return (row: min(max(row + travelled, 0), lastRow),
                column: column(at: point.x, widths: widths, last: lastColumn))
    }
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
    static func text(rows: [[String?]], headers: [String], selection: CellRange,
                     withHeaders: Bool) -> String {
        var lines: [String] = []
        if withHeaders {
            lines.append((selection.left...selection.right)
                .map { escape(header(at: $0, in: headers)) }
                .joined(separator: "\t"))
        }
        for row in selection.top...selection.bottom {
            let source = rows.indices.contains(row) ? rows[row] : []
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
