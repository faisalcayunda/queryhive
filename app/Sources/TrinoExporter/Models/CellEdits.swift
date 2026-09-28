import Foundation

/// One cell of the result grid, by its position in the rows the grid is drawing.
struct CellKey: Hashable {
    let row: Int
    let column: Int
}

/// The cells the user has changed in the grid but not yet committed.
///
/// Staged rather than written: an edit is a statement the user is still composing, and a grid that
/// wrote on every keystroke would turn a typo into a production `UPDATE`. This is the queue that
/// commit and discard act on, and the reason those are buttons rather than the grid writing for you.
///
/// Positions, not identities: a key is a row and a column in the rows *on screen*, which is exactly
/// what the user pointed at. Anything that changes those rows — a new run, a filter — has to throw
/// the queue away, and `QueryTab` does.
struct CellEdits: Equatable {
    private(set) var values: [CellKey: String] = [:]

    var isEmpty: Bool { values.isEmpty }
    var count: Int { values.count }

    /// The staged text for a cell, or `nil` when the cell is untouched.
    func value(at key: CellKey) -> String? { values[key] }

    /// Stage one cell's new text.
    ///
    /// Staging the text the cell already holds *unstages* it: a user who types a value and then
    /// types the original back has changed nothing, and leaving a no-op in the queue would make the
    /// commit button claim work that is not there.
    mutating func edit(_ text: String, at key: CellKey, original: String?) {
        stage(text, at: key, original: original)
    }

    /// Stage one text over every cell of a block — "select these cells and set them all to this".
    mutating func fill(_ text: String, over range: CellRange, rows: [[String?]]) {
        for row in range.top...range.bottom {
            for column in range.left...range.right {
                let key = CellKey(row: row, column: column)
                stage(text, at: key, original: Self.value(in: rows, at: key))
            }
        }
    }

    /// Stage a pasted block, anchored at `origin` — the top-left of the selection, the way a
    /// spreadsheet takes a paste.
    ///
    /// Cells past the edge of the grid are dropped rather than wrapped: a paste that ran off the end
    /// and reappeared on the next row would put values in cells the user never pointed at.
    mutating func paste(_ text: String, at origin: CellKey, rows: [[String?]], columnCount: Int) {
        for (down, line) in Self.parse(text).enumerated() {
            for (across, field) in line.enumerated() {
                let key = CellKey(row: origin.row + down, column: origin.column + across)
                guard key.row < rows.count, key.column < columnCount else { continue }
                stage(field, at: key, original: Self.value(in: rows, at: key))
            }
        }
    }

    mutating func discard() { values.removeAll() }

    private mutating func stage(_ text: String, at key: CellKey, original: String?) {
        if text == original { values[key] = nil } else { values[key] = text }
    }

    private static func value(in rows: [[String?]], at key: CellKey) -> String? {
        guard rows.indices.contains(key.row) else { return nil }
        let row = rows[key.row]
        return row.indices.contains(key.column) ? row[key.column] : nil
    }

    // MARK: Reading a paste

    /// A pasted block back into cells: the inverse of `GridClipboard`'s escaping.
    ///
    /// Rows on newlines and cells on tabs, with a quoted cell unwrapped and its doubled quotes
    /// collapsed. The quoting is what makes the round trip lossless for a value that itself holds a
    /// tab or a newline — the case that would otherwise paste one cell as two.
    ///
    /// A quoted cell holding a newline is *not* handled: the split happens on newlines before any
    /// quoting is considered, so such a value arrives as two rows. Every spreadsheet's own copy
    /// avoids this by quoting, which is what the round trip through this app does; a paste from a
    /// tool that emits a raw newline inside a cell is the case that is out of reach here.
    static func parse(_ text: String) -> [[String]] {
        var normalised = text
            .replacingOccurrences(of: "\r\n", with: "\n")
            .replacingOccurrences(of: "\r", with: "\n")
        // A clipboard block usually ends with the newline that terminated its last row. Keeping it
        // would paste one empty row past the end of what the user copied.
        if normalised.hasSuffix("\n") { normalised.removeLast() }
        return normalised
            .split(separator: "\n", omittingEmptySubsequences: false)
            .map { fields(in: String($0)) }
    }

    /// One line's cells: tabs separate them, except inside a quoted cell.
    private static func fields(in line: String) -> [String] {
        var fields: [String] = []
        var current = ""
        var quoted = false
        var index = line.startIndex
        while index < line.endIndex {
            let character = line[index]
            if quoted {
                if character == "\"" {
                    let next = line.index(after: index)
                    if next < line.endIndex, line[next] == "\"" {
                        current.append("\"")
                        index = next
                    } else {
                        quoted = false
                    }
                } else {
                    current.append(character)
                }
            } else if character == "\"", current.isEmpty {
                // Only a quote at the start of a cell opens one, so `32"` stays the text it looks
                // like rather than swallowing the rest of the line.
                quoted = true
            } else if character == "\t" {
                fields.append(current)
                current = ""
            } else {
                current.append(character)
            }
            index = line.index(after: index)
        }
        fields.append(current)
        return fields
    }
}
