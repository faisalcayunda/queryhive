import Foundation

/// The grid's column presentation: which fetched columns are shown, in what order, under what label.
///
/// Rendering only. The **source index** — a column's position in the server's own column list and in
/// every row — is the identity; order, visibility and label are what the grid does with it. A value,
/// a copy and an export are built from the source columns and never consult this, so hiding a column
/// cannot lose its data and renaming one cannot change what an export writes.
///
/// The layout is a value of its own rather than three loose fields because sort and per-column
/// filters are keyed by source index too. That is what makes them follow a moved column: move
/// `jumlah_jiwa` from third to first and the sort keyed at its source index still names it. The one
/// case identity cannot survive is hiding the sorted column — the chevron that says which order is
/// on screen has nowhere to stand — and `QueryTab` drops the sort for exactly that case.
struct GridColumnLayout: Equatable {
    /// Every source index, in display order. Hidden columns stay in the list, so unhiding restores
    /// one to where it was rather than appending it after the last drawn column.
    private(set) var order: [Int]
    private(set) var hidden: Set<Int>
    /// Source index -> the label drawn instead of the server's name. Absent means the server's name.
    private(set) var renames: [Int: String]

    init(count: Int) {
        order = Array(0..<max(0, count))
        hidden = []
        renames = [:]
    }

    /// How many columns the result the layout was built from has.
    ///
    /// Used to tell a new result from a repaint: a layout only describes the column set it was built
    /// from, so `QueryTab` resets it when the width changes.
    var sourceCount: Int { order.count }

    /// The source indices the grid draws, in display order.
    var visible: [Int] { order.filter { !hidden.contains($0) } }

    var hiddenCount: Int { hidden.count }

    func isVisible(_ source: Int) -> Bool {
        order.contains(source) && !hidden.contains(source)
    }

    func isRenamed(_ source: Int) -> Bool { !(renames[source] ?? "").isEmpty }

    /// The label the grid draws for a source column: the rename when there is one, the server's own
    /// name otherwise. The fallback also covers a source index the column list no longer has, which
    /// can be reached for an instant between two results.
    func label(_ source: Int, original: [Event.Column]) -> String {
        if let renamed = renames[source], !renamed.isEmpty { return renamed }
        return original.indices.contains(source) ? original[source].name : "column \(source + 1)"
    }

    /// The display position of a source column among the drawn ones, or `nil` when it is hidden.
    func position(of source: Int) -> Int? { visible.firstIndex(of: source) }

    /// The source index at a display position, or `nil` past the last drawn column.
    func source(at position: Int) -> Int? {
        let visible = self.visible
        return visible.indices.contains(position) ? visible[position] : nil
    }

    mutating func show(_ source: Int) { hidden.remove(source) }

    mutating func hide(_ source: Int) {
        guard order.contains(source) else { return }
        hidden.insert(source)
    }

    mutating func toggle(_ source: Int) {
        hidden.contains(source) ? show(source) : hide(source)
    }

    mutating func showAll() { hidden.removeAll() }

    /// Rename a column. A blank name means "back to the server's", not an empty header.
    mutating func rename(_ source: Int, to name: String) {
        guard order.contains(source) else { return }
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        renames[source] = trimmed.isEmpty ? nil : trimmed
    }

    /// Move the drawn column at `from` to display position `to`. Both are positions among the drawn
    /// columns; a hidden column keeps its place in the underlying order so unhiding stays
    /// predictable rather than jumping the column to the end.
    mutating func move(from: Int, to: Int) {
        var drawn = visible
        guard drawn.indices.contains(from) else { return }
        let destination = min(max(to, 0), drawn.count - 1)
        guard destination != from else { return }
        let moved = drawn.remove(at: from)
        drawn.insert(moved, at: destination)
        var next = drawn.makeIterator()
        order = order.map { hidden.contains($0) ? $0 : (next.next() ?? $0) }
    }

    /// Back to the server's own order, with every column shown under its own name.
    mutating func reset() {
        hidden.removeAll()
        renames.removeAll()
        order.sort()
    }
}
