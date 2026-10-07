import Foundation

/// The rows the table draws: the fetched rows, then the rows the user added (blueprint w10 D-2).
///
/// A pure value, so the mapping is checked without a table. The selection and the cursor
/// (`QueryTab.cellSelection`, `cellCursor`) live in **table rows** — indices `0..<count` — because a
/// `CellRange` is a rectangle and only means something in a contiguous space. An added row's identity
/// is a negative id (`CellEdits.InsertedRow.id`), and that id lives in `CellKey.row` only: the one
/// way from a table row to a `CellKey` is `cellKey(forTableRow:source:)`, and it answers a negative
/// id **if and only if** the row is past the fetched ones.
///
/// Added rows always follow every fetched row, so a block that crosses the border is a prefix of
/// fetched rows and a suffix of added ones (`split`).
struct GridRowSpace: Equatable {
    /// Rows the result holds (after filter, search and sort).
    let fetched: Int
    /// The ids of the added rows, in the order they are drawn.
    let insertedIDs: [Int]

    init(fetched: Int, inserted: [CellEdits.InsertedRow]) {
        self.fetched = max(0, fetched)
        self.insertedIDs = inserted.map(\.id)
    }

    init(fetched: Int, insertedIDs: [Int] = []) {
        self.fetched = max(0, fetched)
        self.insertedIDs = insertedIDs
    }

    var count: Int { fetched + insertedIDs.count }

    enum Kind: Equatable {
        case fetched(Int)
        case inserted(id: Int, index: Int)
    }

    /// What a table row is, or `nil` outside `0..<count`.
    func kind(ofTableRow row: Int) -> Kind? {
        guard row >= 0, row < count else { return nil }
        if row < fetched { return .fetched(row) }
        let index = row - fetched
        return .inserted(id: insertedIDs[index], index: index)
    }

    /// The table row of an added row, or `nil` when no such row is in the queue.
    func tableRow(forInsertedID id: Int) -> Int? {
        insertedIDs.firstIndex(of: id).map { fetched + $0 }
    }

    /// The key a cell of a table row is edited under: the row itself for a fetched row, the negative
    /// id for an added one, `nil` outside the space.
    func cellKey(forTableRow row: Int, source: Int) -> CellKey? {
        switch kind(ofTableRow: row) {
        case .fetched(let index): return CellKey(row: index, column: source)
        case .inserted(let id, _): return CellKey(row: id, column: source)
        case nil: return nil
        }
    }

    /// The table row a key names, or `nil` for a row that is not in the space.
    func tableRow(for key: CellKey) -> Int? {
        if key.row < 0 { return tableRow(forInsertedID: key.row) }
        return key.row < fetched ? key.row : nil
    }

    /// A block of table rows cut at the border: the prefix that is fetched rows, and the suffix of
    /// added rows with their ids. Rows outside the space are dropped.
    func split(_ rows: ClosedRange<Int>) -> (fetched: ClosedRange<Int>?, inserted: [(tableRow: Int, id: Int)]) {
        let low = max(rows.lowerBound, 0), high = min(rows.upperBound, count - 1)
        guard low <= high else { return (nil, []) }
        let fetchedPart: ClosedRange<Int>? = low < fetched ? low...min(high, fetched - 1) : nil
        var inserted: [(tableRow: Int, id: Int)] = []
        if high >= fetched {
            for row in max(low, fetched)...high { inserted.append((row, insertedIDs[row - fetched])) }
        }
        return (fetchedPart, inserted)
    }

    /// A selection kept inside the space, or `nil` when the space is empty. The row part only: the
    /// columns are the caller's.
    func clamped(_ range: CellRange) -> CellRange? {
        guard count > 0 else { return nil }
        let top = min(max(range.top, 0), count - 1)
        let bottom = min(max(range.bottom, 0), count - 1)
        return CellRange(from: (top, range.left), to: (bottom, range.right))
    }
}
