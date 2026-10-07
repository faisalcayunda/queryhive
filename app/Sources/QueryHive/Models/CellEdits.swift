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

    /// Rows the user added that are not in the fetched result.
    ///
    /// Held here rather than in a second queue because the invariants are shared:
    /// deleting a row drops its edits, and a plan must see all three kinds of
    /// change together and in one order. A negative id keeps an inserted row's
    /// identity distinct from every fetched row's index.
    private(set) var inserted: [InsertedRow] = []

    /// Fetched rows the user marked for deletion, in the order they were marked.
    private(set) var deletedRows: [Int] = []

    /// One added row, and where it sits in the queue's own order.
    struct InsertedRow: Equatable {
        let id: Int
        /// The stamp that orders the queue. A removal swaps nothing here (the
        /// arrays are append-only), but the stamp is what keeps the plan's order
        /// from depending on a dictionary's iteration order, which is the failure
        /// the source study records TablePro guarding against.
        let sequence: Int
        var values: [Int: String] = [:]
    }

    private var sequence = 0
    private var nextInsertID = -1

    var isEmpty: Bool { values.isEmpty && inserted.isEmpty && deletedRows.isEmpty }
    var count: Int { values.count + inserted.count + deletedRows.count }

    /// The staged text for a cell, or `nil` when the cell is untouched.
    func value(at key: CellKey) -> String? { values[key] }

    /// The cells that carry a staged edit, as `CellKey`s: row and **source** column, which are the
    /// coordinates the queue is keyed on (`rowText` builds a `CellKey` from the source index it is
    /// iterating).
    ///
    /// Not display positions. The grid's `CellPos`s are display coordinates — the pointer hands
    /// `clampedColumn(atX:)`'s result straight to `selectCells` — so handing these to a diff that
    /// mixes them with a selection would invalidate the wrong rectangle, and a column that is
    /// hidden or dragged out of order would invalidate one that does not exist. Translating to
    /// display is the diff's job: only it can see `visibleSources`.
    var stagedKeys: [CellKey] { Array(values.keys) }

    /// Whether a row carries any staged edit, which is what tells the grid to build its text
    /// instead of taking it from the row cache.
    func hasStagedEdit(row: Int) -> Bool { values.keys.contains { $0.row == row } }

    /// Stage one cell's new text.
    ///
    /// Staging the text the cell already holds *unstages* it: a user who types a value and then
    /// types the original back has changed nothing, and leaving a no-op in the queue would make the
    /// commit button claim work that is not there.
    mutating func edit(_ text: String, at key: CellKey, original: String?) {
        stage(text, at: key, original: original)
    }

    /// Stage one text over every cell of a block — "select these cells and set them all to this".
    ///
    /// The range's columns are **display positions**, and `columns` maps each to the source index a
    /// cell is keyed by; the grid passes its visible columns, so a block dragged over a reordered
    /// grid still edits the cells the user pointed at. Omitting it means the two are the same, which
    /// is the shape the tests use.
    mutating func fill(_ text: String, over range: CellRange, rows: some RowReading,
                       columns: [Int]? = nil) {
        for row in range.top...range.bottom {
            // A row the store cannot read (past the fetched rows, or an added row's table row) has
            // no original to compare with, and an edit keyed to it would name a row that is not
            // there: skipped, the way `paste` skips it (blueprint w10 §5.1).
            guard rows.row(at: row) != nil else { continue }
            for position in range.left...range.right {
                let key = CellKey(row: row, column: Self.source(position, in: columns))
                stage(text, at: key, original: Self.value(in: rows, at: key))
            }
        }
    }

    /// Stage a pasted block, anchored at `origin` — the top-left of the selection, the way a
    /// spreadsheet takes a paste.
    ///
    /// Cells past the edge of the grid are dropped rather than wrapped: a paste that ran off the end
    /// and reappeared on the next row would put values in cells the user never pointed at. As with
    /// `fill`, `origin`'s column and `columnCount` are display positions and `columns` maps them to
    /// the source indices the cells are keyed by.
    mutating func paste(_ text: String, at origin: CellKey, rows: some RowReading, columnCount: Int,
                        columns: [Int]? = nil) {
        paste(lines: Self.parse(text), at: origin, rows: rows, columnCount: columnCount, columns: columns)
    }

    /// The same, from a block already read (`parse`), so a caller that cuts the block at the border
    /// of the fetched rows can hand this the part that is theirs.
    mutating func paste(lines: [[String]], at origin: CellKey, rows: some RowReading, columnCount: Int,
                        columns: [Int]? = nil) {
        for (down, line) in lines.enumerated() {
            for (across, field) in line.enumerated() {
                let position = origin.column + across
                guard position < columnCount else { continue }
                let key = CellKey(row: origin.row + down, column: Self.source(position, in: columns))
                guard rows.row(at: key.row) != nil else { continue }
                stage(field, at: key, original: Self.value(in: rows, at: key))
            }
        }
    }

    /// A display position as the source index a cell is keyed by.
    private static func source(_ position: Int, in columns: [Int]?) -> Int {
        guard let columns, columns.indices.contains(position) else { return position }
        return columns[position]
    }

    mutating func discard() {
        values.removeAll()
        inserted.removeAll()
        deletedRows.removeAll()
        sequence = 0
        nextInsertID = -1
    }

    // MARK: Rows

    /// Add a row and return its id, which is what its cells are keyed by.
    @discardableResult
    mutating func insertRow() -> Int {
        sequence += 1
        let id = nextInsertID
        nextInsertID -= 1
        inserted.append(InsertedRow(id: id, sequence: sequence))
        return id
    }

    /// Mark a fetched row for deletion. Its staged edits go with it: a row that
    /// is being removed cannot also be updated, and keeping the edits would put
    /// two statements for one row into the plan.
    mutating func deleteRow(_ row: Int) {
        if !deletedRows.contains(row) { deletedRows.append(row) }
        values = values.filter { $0.key.row != row }
    }

    /// Mark a block of fetched rows for deletion in one pass. A loop of `deleteRow` is quadratic (each
    /// call scans the marked rows and the staged values), which a select-all delete would pay in full.
    mutating func deleteRows(_ rows: ClosedRange<Int>) {
        let marked = Set(deletedRows)
        deletedRows.append(contentsOf: rows.filter { !marked.contains($0) })
        values = values.filter { !rows.contains($0.key.row) }
    }

    /// Stage one cell of an added row — the insert's counterpart of `edit`.
    mutating func setInserted(_ text: String, row id: Int, column: Int) {
        guard let index = inserted.firstIndex(where: { $0.id == id }) else { return }
        if text.isEmpty {
            inserted[index].values[column] = nil
        } else {
            inserted[index].values[column] = text
        }
    }

    /// Take an added row back out of the queue, with every value typed into it. The id is never
    /// reused: `nextInsertID` only counts down, so an undo that puts the row back finds its own id.
    mutating func removeInserted(row id: Int) {
        inserted.removeAll { $0.id == id }
    }

    /// Let a row marked for deletion go. The edits it held when it was marked are not brought back:
    /// `deleteRow` dropped them, and a restore that resurrected some of them would be a second
    /// guess at what the user meant.
    mutating func restoreRow(_ row: Int) {
        deletedRows.removeAll { $0 == row }
    }

    func isDeleted(_ row: Int) -> Bool { deletedRows.contains(row) }

    /// What is left of the queue once the part a successful apply wrote (`snapshot`) is taken out:
    /// a staged cell goes only if it still holds the text the snapshot held, so a change made while
    /// the apply ran is kept (PF-2). An added row goes when it is in the snapshot with the same
    /// values, a deleted row when the snapshot deleted it too.
    func removing(_ snapshot: CellEdits) -> CellEdits {
        var left = self
        left.values = values.filter { snapshot.values[$0.key] != $0.value }
        // An added row the snapshot carried is in the table now, whatever was typed into it since;
        // keeping it would INSERT it a second time on the next apply.
        left.inserted = inserted.filter { row in !snapshot.inserted.contains { $0.id == row.id } }
        left.deletedRows = deletedRows.filter { !snapshot.deletedRows.contains($0) }
        if left.isEmpty { left.discard() }
        return left
    }

    /// What the grid says about a cell or a row, so the painter, the accessibility label and a
    /// test read one answer (blueprint w10 §4.2, FR-GRID-09).
    enum CellState: Equatable {
        case unchanged, modified, inserted, deleted
    }

    /// One cell's state. A cell of a row marked for deletion is deleted whatever it held; a cell of
    /// an added row is inserted (its row id is negative); otherwise it is modified when it carries a
    /// staged value. Read only: the gestures that add and remove rows arrive with W10-T3.
    func state(of key: CellKey) -> CellState {
        let row = rowState(key.row)
        if row == .deleted || row == .inserted { return row }
        return values[key] != nil ? .modified : .unchanged
    }

    /// One row's state: `deleted` for a fetched row marked for deletion, `inserted` for an added
    /// row (a negative id that is in the queue), `modified` when any of its cells is staged, and
    /// `unchanged` otherwise.
    func rowState(_ row: Int) -> CellState {
        if row < 0 { return inserted.contains { $0.id == row } ? .inserted : .unchanged }
        if isDeleted(row) { return .deleted }
        return hasStagedEdit(row: row) ? .modified : .unchanged
    }

    func insertedValue(row id: Int, column: Int) -> String? {
        inserted.first { $0.id == id }?.values[column]
    }

    private mutating func stage(_ text: String, at key: CellKey, original: String?) {
        // A row marked for deletion takes no edit: `deleteRow` dropped them, and an UPDATE planned
        // after its DELETE would match nothing and roll the whole plan back.
        guard !deletedRows.contains(key.row) else { return }
        if text == original { values[key] = nil } else { values[key] = text }
    }

    private static func value(in rows: some RowReading, at key: CellKey) -> String? {
        guard let row = rows.row(at: key.row) else { return nil }
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
