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
    private var columnElems: [Int: GridAXColumn] = [:]

    init(coordinator: ResultGridTable.Coordinator) {
        self.coordinator = coordinator
    }

    /// The rows on screen, and the columns, as the table layer's children.
    var visibleChildren: [Any] {
        refresh()
        return visibleRows + columns
    }

    /// A header element for every drawn column, in display order: what
    /// `accessibilityColumnHeaderUIElements` answers with.
    ///
    /// Deliberately a different set from `columns`. A header is a `.button` that runs the sort
    /// cycle; a column is a `.column` that groups cells. VoiceOver reads them through different
    /// attributes, and handing it buttons where it asked for columns is how a table ends up with
    /// no column headers at all.
    var columnHeaders: [GridAXHeader] {
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

    /// One lightweight `.column` element per drawn column, in display order.
    ///
    /// Cached by source column and re-indexed on every question, the way the headers are: a column
    /// dragged or hidden must not leave VoiceOver holding an element whose position it no longer
    /// has, and rebuilding all of them on every call would hand a client a different object each
    /// time and make it lose its place.
    var columns: [GridAXColumn] {
        refresh()
        let visible = coordinator.visibleSources
        for source in columnElems.keys where !visible.contains(source) { columnElems[source] = nil }
        return visible.enumerated().map { display, source in
            let element = columnElems[source] ?? {
                let made = GridAXColumn(column: display, sourceColumn: source, coordinator: coordinator)
                columnElems[source] = made
                return made
            }()
            element.column = display
            return element
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
        columnElems.removeAll()
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
        // Blueprint §11.2: elements are thrown away when the result, the columns or the visible
        // columns change. Cells and rows are keyed by revision; the column elements go for the same
        // reason — a client that held one must not be left holding an element for a column that is
        // no longer there, and re-indexing on the next question reuses whatever still is.
        headers.removeAll()
        columnElems.removeAll()
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
    /// utterance. What follows the value is the one thing about a cell that its value cannot say:
    /// "changed" for a staged edit, "marked for deletion" for a row queued to be deleted.
    ///
    /// A NULL is the word "null" whatever the grid is set to draw it as. `nullDisplay` is a choice
    /// about the picture, and an empty one would read as a cell with nothing in it.
    override func accessibilityLabel() -> String? {
        let staged = coordinator.stagedValue(at: key)
        let value = staged ?? coordinator.fullValue(at: key) ?? Self.nullWord
        var status = ""
        if staged != nil { status += ", changed" }
        if coordinator.isMarkedForDeletion(row: key.row) { status += ", marked for deletion" }
        let shown = (value as NSString).length > 256
            ? (value as NSString).substring(to: 256) + "…"
            : value
        return "Row \(key.row + 1), column \(display + 1), \(coordinator.columnName(at: key.column)): \(shown)\(status)"
    }

    /// What VoiceOver says for a NULL.
    static let nullWord = "null"

    override func accessibilityValue() -> Any? {
        coordinator.stagedValue(at: key) ?? coordinator.fullValue(at: key) ?? Self.nullWord
    }

    /// Whether the cursor is on this cell.
    override func isAccessibilityFocused() -> Bool { coordinator.isAXFocused(key) }

    /// VoiceOver moved its focus here: the cursor follows it, the selection stays where it is, and
    /// nothing is announced (blueprint 3.4, D-11/P-24). Taking focus *away* needs no action: the
    /// cell that receives it is the one that writes.
    override func setAccessibilityFocused(_ accessibilityFocused: Bool) {
        guard accessibilityFocused else { return }
        coordinator.focusCellFromAX(key)
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
///
/// `NSAccessibilityRow` rather than just an element: `accessibilityRows` is typed to return rows,
/// and AppKit's own row protocol is what carries `accessibilityIndex` — the row's place in the
/// **result**, in the same coordinates the cells report (§5.4).
/// `@preconcurrency` because the AppKit row protocol is nonisolated and this class is not: AppKit
/// only ever reaches it from the main thread, which is where the rest of this file already runs.
@MainActor
final class GridAXRow: NSAccessibilityElement, @preconcurrency NSAccessibilityRow {
    let row: Int
    private unowned let coordinator: ResultGridTable.Coordinator

    init(row: Int, coordinator: ResultGridTable.Coordinator) {
        self.row = row
        self.coordinator = coordinator
        super.init()
        setAccessibilityRole(.row)
    }

    /// The result row index, one place VoiceOver uses to say "row 12 of 40 000".
    override func accessibilityIndex() -> Int { row }

    /// `NSAccessibilityElementProtocol` requires a non-optional identifier, where the class this
    /// inherits from answers an optional one through `NSAccessibility`'s nullable property. Both
    /// are spelled the same in Swift, so the row spells its own: a stable one for a row that
    /// otherwise has no identity of its own to hang focus on.
    override func accessibilityIdentifier() -> String { "grid.row.\(row)" }

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

    /// "Filter": what the funnel does, offered as an action because a 10-point glyph in the corner
    /// of a header is not a target a screen-reader user can be asked to find (FR-GRID-07).
    override func accessibilityCustomActions() -> [NSAccessibilityCustomAction]? {
        let source = sourceColumn
        return [NSAccessibilityCustomAction(name: "Filter") { [weak self] in
            MainActor.assumeIsolated {
                guard let self else { return false }
                self.coordinator.commands.openFilter(source, self.coordinator.frame(ofHeader: self.column))
                return true
            }
        }]
    }

    override func accessibilityFrameInParentSpace() -> NSRect {
        coordinator.frame(ofHeader: column)
    }

    override func accessibilityParent() -> Any? { coordinator.axParentElement() }
}

/// One drawn column, as a `.column`.
///
/// Lightweight on purpose (blueprint §11.2): it groups, it does not describe. The name, the type
/// and the sort state all belong to the header button that sits above it, and duplicating them here
/// would give VoiceOver two things to read for one column. What it needs is where the column is and
/// which one it is — the **source** index, the identity a moved or hidden column keeps, which is
/// what the cells inside it report too.
@MainActor
final class GridAXColumn: NSAccessibilityElement {
    var column: Int
    let sourceColumn: Int
    private unowned let coordinator: ResultGridTable.Coordinator

    init(column: Int, sourceColumn: Int, coordinator: ResultGridTable.Coordinator) {
        self.column = column
        self.sourceColumn = sourceColumn
        self.coordinator = coordinator
        super.init()
        setAccessibilityRole(.column)
    }

    override func accessibilityLabel() -> String? { coordinator.columnName(at: sourceColumn) }

    override func accessibilityValue() -> Any? { coordinator.columnType(at: sourceColumn) }

    override func accessibilityColumnIndexRange() -> NSRange {
        NSRange(location: sourceColumn, length: 1)
    }

    /// The column's own extent over the rows on screen, computed when asked: a scroll moves it.
    override func accessibilityFrameInParentSpace() -> NSRect {
        coordinator.frame(ofColumn: column)
    }

    override func accessibilityParent() -> Any? { coordinator.axParentElement() }
}
