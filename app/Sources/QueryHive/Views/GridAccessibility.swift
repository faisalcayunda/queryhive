import AppKit

/// The grid's accessibility elements, built when AppKit asks for them and not before.
///
/// The table is cell-based with **one** `NSTableColumn`, so AppKit's own tree describes a one-column
/// table: it would tell VoiceOver that a 500-column result has one column, and every index in it
/// would be wrong. The tree therefore has to be replaced, not extended — `GridTableView` overrides
/// the table attributes (`accessibilityRows`, `accessibilityColumns`, `accessibilityCell(forColumn:row:)`
/// and the rest) to answer from here.
///
/// Two properties this file exists to keep:
///
///  - **Nothing is created unless a client is watching.** Every element comes out of an
///    `accessibility…()` method; with VoiceOver off, AppKit never calls one and the grid allocates
///    nothing. That is the effect the design audit asked for ("installed only once an AX client
///    attaches") without trying to detect VoiceOver.
///  - **Nothing is stored that would go stale.** A label and a value are computed at the moment they
///    are asked for, from `ResultRows` and the edit queue, so an element that is cached across an
///    edit cannot announce the value the cell had a second ago. What *is* cached is identity, keyed
///    by revision, result row and source column, so VoiceOver does not lose its place because an
///    element it was on was rebuilt as a different object.
///
/// Coordinates throughout are **result** rows, never table rows (blueprint §5.4). Today the two are
/// the same; a windowed result over 2^24 pt would change `Coordinator.resultRow(forTableRow:)` and
/// nothing here.
@MainActor
final class GridAXTree {

    private unowned let coordinator: ResultGridTable.Coordinator
    /// The revision the cache was built for. A new result throws the elements away: their indices
    /// name rows that no longer exist.
    private var revision: Int?
    private var cells: [CellKey: GridAXCell] = [:]
    private var rows: [Int: GridAXRow] = [:]
    private var headers: [Int: GridAXHeader] = [:]

    init(coordinator: ResultGridTable.Coordinator) {
        self.coordinator = coordinator
    }

    /// The rows on screen, and the columns, as the table layer's children.
    var visibleChildren: [Any] {
        refresh()
        return visibleRows + columns
    }

    /// A header for every drawn column, in display order.
    var columns: [GridAXHeader] {
        refresh()
        return coordinator.visibleSources.enumerated().map { display, source in
            let header = headers[source] ?? {
                let made = GridAXHeader(column: display, sourceColumn: source, coordinator: coordinator)
                headers[source] = made
                return made
            }()
            header.column = display
            return header
        }
    }

    /// A row element for every row on screen, in order.
    var visibleRows: [GridAXRow] {
        refresh()
        let range = coordinator.visibleRowRange()
        return (range.lowerBound..<range.upperBound).map { row in
            let element = rows[row] ?? {
                let made = GridAXRow(row: row, coordinator: coordinator)
                rows[row] = made
                return made
            }()
            return element
        }
    }

    /// One cell, whether or not it is on screen: VoiceOver's table navigation asks for a cell and
    /// then scrolls to it, so refusing a row that is merely off screen would make the grid
    /// unnavigable past the first pageful.
    func cell(row: Int, column display: Int) -> GridAXCell? {
        refresh()
        guard let source = coordinator.sourceColumn(at: display),
              let key = coordinator.validKey(row: row, source: source) else { return nil }
        if let cached = cells[key] { return cached }
        let made = GridAXCell(key: key, display: display, coordinator: coordinator)
        cells[key] = made
        return made
    }

    /// The cells of every row and column on screen.
    var visibleCells: [GridAXCell] {
        let range = coordinator.visibleRowRange()
        var result: [GridAXCell] = []
        for row in range.lowerBound..<range.upperBound {
            for display in 0..<coordinator.visibleSources.count {
                if let cell = cell(row: row, column: display) { result.append(cell) }
            }
        }
        return result
    }

    /// The selection, cut down to what is on screen: an AX element for a row a million rows below
    /// the viewport is an element nobody can reach.
    var selectedCells: [GridAXCell] {
        guard let selection = coordinator.tab.cellSelection else { return [] }
        let range = coordinator.visibleRowRange()
        var result: [GridAXCell] = []
        for row in selection.top...selection.bottom where range.contains(row) {
            for display in selection.left...selection.right {
                if let cell = cell(row: row, column: display) { result.append(cell) }
            }
        }
        return result
    }

    /// The cell the cursor is on, which is what VoiceOver's focus is moved to.
    var focusedCell: GridAXCell? {
        guard let cursor = coordinator.tab.cellCursor else { return nil }
        return cell(row: cursor.focus.row, column: cursor.focus.column)
    }

    /// Throw the elements away because the rows, the columns or the widths under them changed.
    func invalidate() {
        cells.removeAll()
        rows.removeAll()
        headers.removeAll()
        revision = nil
    }

    /// Keep the identity cache to the viewport plus a page, except for the cell the cursor is on:
    /// caching every row a long table has ever shown would grow without bound, and caching none of
    /// them would lose VoiceOver's place on every scroll.
    func trim() {
        let range = coordinator.visibleRowRange()
        let page = max(1, range.upperBound - range.lowerBound)
        let keep = (range.lowerBound - page)..<(range.upperBound + page)
        let cursorRow = coordinator.tab.cellCursor?.focus.row
        cells = cells.filter { key, _ in keep.contains(key.row) || key.row == cursorRow }
        rows = rows.filter { row, _ in keep.contains(row) }
    }

    private func refresh() {
        guard revision != coordinator.currentRevision else { return }
        revision = coordinator.currentRevision
        cells.removeAll()
        rows.removeAll()
    }
}

/// One cell.
@MainActor
final class GridAXCell: NSAccessibilityElement {
    let key: CellKey
    var display: Int
    private unowned let coordinator: ResultGridTable.Coordinator

    init(key: CellKey, display: Int, coordinator: ResultGridTable.Coordinator) {
        self.key = key
        self.display = display
        self.coordinator = coordinator
        super.init()
        setAccessibilityRole(.cell)
    }

    /// "Row r, column c, \<header\>: \<value\>, changed" — the string FR-GRID-07 asks for.
    ///
    /// `r` and `c` are one-based, because they are read out loud and nobody counts from zero. The
    /// value is cut to 256 units with a marker, so a ten-megabyte cell does not become a ten-megabyte
    /// utterance. "changed" appears only for a cell with a staged edit, which is the one thing about
    /// a cell the grid can say that its value cannot.
    override func accessibilityLabel() -> String? {
        let staged = coordinator.stagedValue(at: key)
        let value = staged ?? coordinator.fullValue(at: key) ?? coordinator.nullDisplay
        let changed = staged != nil ? ", changed" : ""
        let shown = (value as NSString).length > 256
            ? (value as NSString).substring(to: 256) + "…"
            : value
        return "Row \(key.row + 1), column \(display + 1), \(coordinator.columnName(at: key.column)): \(shown)\(changed)"
    }

    override func accessibilityValue() -> Any? {
        coordinator.stagedValue(at: key) ?? coordinator.fullValue(at: key) ?? coordinator.nullDisplay
    }

    /// The position in the result, not in the table: `Coordinator.resultRow(forTableRow:)` converts,
    /// and in W5 it is the identity.
    override func accessibilityRowIndexRange() -> NSRange {
        NSRange(location: key.row, length: 1)
    }

    /// The source column, which is the stable identity a moved or hidden column keeps.
    override func accessibilityColumnIndexRange() -> NSRange {
        NSRange(location: key.column, length: 1)
    }

    /// In the table's own coordinates, so AppKit converts it to the screen after a scroll.
    ///
    /// Computed when asked rather than stored: a scroll does not move the element, and a frame
    /// captured at build time would put VoiceOver's highlight a thousand rows above the cell it is
    /// reading.
    override func accessibilityFrameInParentSpace() -> NSRect {
        coordinator.frame(of: key, display: display)
    }

    override func accessibilityRowCount() -> Int { 1 }
    override func accessibilityColumnCount() -> Int { 1 }

    override func accessibilityParent() -> Any? { coordinator.axRowElement(for: key.row) }
}

/// One row, whose children are its visible cells.
@MainActor
final class GridAXRow: NSAccessibilityElement {
    let row: Int
    private unowned let coordinator: ResultGridTable.Coordinator

    init(row: Int, coordinator: ResultGridTable.Coordinator) {
        self.row = row
        self.coordinator = coordinator
        super.init()
        setAccessibilityRole(.row)
    }

    override func accessibilityChildren() -> [Any]? {
        (0..<coordinator.visibleSources.count).compactMap {
            coordinator.axCell(row: row, column: $0)
        }
    }

    override func accessibilityRowIndexRange() -> NSRange {
        NSRange(location: row, length: 1)
    }

    override func accessibilityRowCount() -> Int { 1 }
    override func accessibilityColumnCount() -> Int { coordinator.visibleSources.count }

    override func accessibilityFrameInParentSpace() -> NSRect {
        coordinator.frame(ofRow: row)
    }

    override func accessibilityParent() -> Any? { coordinator.axParentElement() }
}

/// One column header, as a button: pressing it sorts, which is what clicking it does.
@MainActor
final class GridAXHeader: NSAccessibilityElement {
    var column: Int
    let sourceColumn: Int
    private unowned let coordinator: ResultGridTable.Coordinator

    init(column: Int, sourceColumn: Int, coordinator: ResultGridTable.Coordinator) {
        self.column = column
        self.sourceColumn = sourceColumn
        self.coordinator = coordinator
        super.init()
        setAccessibilityRole(.button)
    }

    override func accessibilityLabel() -> String? {
        let name = coordinator.columnName(at: sourceColumn)
        let type = coordinator.columnType(at: sourceColumn)
        switch coordinator.sortDirection(of: sourceColumn) {
        case .ascending: return "\(name), \(type), sorted ascending"
        case .descending: return "\(name), \(type), sorted descending"
        case nil: return "\(name), \(type)"
        }
    }

    /// The type, which is what the chip in the header shows.
    override func accessibilityValue() -> Any? { coordinator.columnType(at: sourceColumn) }

    /// The same indicator the chevron is drawn from, so what is read out and what is drawn cannot
    /// disagree.
    override func accessibilitySortDirection() -> NSAccessibilitySortDirection {
        switch coordinator.sortDirection(of: sourceColumn) {
        case .ascending: return .ascending
        case .descending: return .descending
        case nil: return .unknown
        }
    }

    /// A header press runs the sort cycle — server-first, exactly as a click does.
    override func accessibilityPerformPress() -> Bool {
        coordinator.commands.sortClick(sourceColumn)
        return true
    }

    override func accessibilityFrameInParentSpace() -> NSRect {
        coordinator.frame(ofHeader: column)
    }

    override func accessibilityParent() -> Any? { coordinator.axParentElement() }
}
