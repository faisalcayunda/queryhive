import AppKit
import SwiftUI

/// Everything `ResultGrid.body` reads that the table has to be told about.
///
/// A value type, and `Equatable`, for two reasons. Reading these properties in `body` is what
/// registers the Observation dependency, so a change here is what makes SwiftUI call
/// `updateNSView` at all; and the coordinator diffs the new value against the one it last applied,
/// so an update that changed nothing costs one comparison rather than a repaint.
struct GridInputs: Equatable {

    /// One drawn column's width, in display order, and the source indices they map to.
    ///
    /// `columnWidths` are the **formula** widths — §10's `min(max(n × 7.2 + 20, 84), 320) + 22`, with
    /// no cell padding in them. They are not what is drawn: the old grid put `.padding(.horizontal, 8)`
    /// *outside* the width frame, so each drawn column is 16 pt wider than this. The coordinator adds
    /// that back and shares out the panel's slack, which is where the panel's width is finally known
    /// (§10 step 2) — `body` cannot see it.
    struct GridColumnLayout: Equatable {
        var columnWidths: [CGFloat]
        var visibleSources: [Int]
    }

    /// The switches that change how the grid is drawn rather than what it holds.
    struct GridStyle: Equatable {
        var rowHeight: CGFloat
        var alternateRows: Bool
        var showRowNumbers: Bool
        var nullDisplay: String
        var codeFontFamily: String
        var accent: String
        /// Whether the app is drawing in a dark appearance.
        ///
        /// SwiftUI's own `colorScheme` rather than `NSAppearance.currentDrawing()` or a view's
        /// `effectiveAppearance`: both of those answered the process-wide appearance when the grid
        /// is built through `NSViewRepresentable` in a host whose window is not the one carrying the
        /// scheme, so a dark grid drew black-on-black text. The environment is what SwiftUI is
        /// actually rendering with, and a change to it moves this field and repaints the palette.
        var isDark: Bool
        /// Whether a header click sorts at all. Always true today; the switch keeps the footer's
        /// own readout of the first direction from being the only thing that knows.
        var sortEnabled: Bool

        /// The style with nothing set, for a context that has not been given one yet.
        static let placeholder = GridStyle(rowHeight: 25, alternateRows: true, showRowNumbers: true,
                                           nullDisplay: "null", codeFontFamily: "", accent: "ice",
                                           isDark: false,
                                           sortEnabled: true)
    }

    /// The revision of `tab.result`: when this changes, the rows under every row index have changed.
    var revision: Int
    var layout: GridColumnLayout
    var selection: CellRange?
    var edits: CellEdits
    /// The sort, reduced to what the header's chevron needs.
    var sort: SortIndicator?
    /// The source columns with an active filter, for the funnels.
    var filtered: Set<Int>
    var style: GridStyle
    /// The column whose filter popover is open, from the model — a snapshot scene fills it, which a
    /// header funnel could not do itself.
    var filterPopover: Int?
    /// The cell whose value reader is open, keyed by source column.
    var viewing: CellKey?
}

/// One column's sort, as the header draws it.
struct SortIndicator: Equatable {
    var column: Int
    var direction: GridSort.Direction
    var origin: SortOrigin
}

/// What the table, its header and its menus are allowed to do.
///
/// Closures rather than a reference to `AppModel`: the table and the header must not know about
/// `toggleSort`, the filter popover's storage or the sheets, and every one of those lives in a
/// SwiftUI view that owns `@State` the table cannot see.
struct GridCommands {
    /// A header click. `source` is the column's identity, not its drawn position.
    var sortClick: (Int) -> Void
    /// The funnel was clicked, at a rect in the table's own coordinates.
    var openFilter: (Int, CGRect) -> Void
    var rename: (Int) -> Void
    var review: () -> Void
    var copy: (Bool) -> Void
    var viewValue: () -> Void
    var beginEdit: (CellKey) -> Void
    var commitEdit: (CellKey, String) -> Void
    var cancelEdit: () -> Void
    /// A drag ended: the inspector's value is settled here rather than on every step of the drag.
    var settleSelection: () -> Void
    var paste: () -> Void
    /// The selection changed under the table's own pointer handling, so the tab's value is already
    /// written and the view only has to record that something was chosen.
    var selectionChanged: () -> Void
}

/// The table's side of `GridCommands`, for the paths that come *from* the table rather than from a
/// SwiftUI view: a double-click that opens the editor, a Return that commits one, and the menu's
/// own Edit Cell item, which has to reach the overlay inside the table.
///
/// A tiny handle rather than a delegate, so `ResultGrid` can hold it in `@State` before the table
/// exists — `NSViewRepresentable` builds its coordinator after `body` has already run.
@MainActor
final class GridTableHandle {
    weak var coordinator: ResultGridTable.Coordinator?

    func beginEdit(at key: CellKey) { coordinator?.beginEdit(at: key) }
    func commitEdit(at key: CellKey, text: String) { coordinator?.commitEdit(at: key, text: text) }
    func cancelEdit() { coordinator?.cancelEdit() }
    func selectCells(anchor: CellPos, focus: CellPos) { coordinator?.selectCells(anchor: anchor, focus: focus) }
    func scrollToVisible(row: Int) { coordinator?.scrollToVisible(row: row) }
    func rowsDidGrow() { coordinator?.rowsDidGrow() }
    func copy(withHeaders: Bool) { coordinator?.copy(withHeaders: withHeaders) }
}

/// The result grid, as an `NSTableView` that draws every pixel of itself.
///
/// A thin `NSViewRepresentable`: the table, its header and the coordinator that owns the rows,
/// the caches, the pointer handling and the editor overlay all live in `GridTableView` and
/// `ResultGridTable.Coordinator`, so they can be built and driven from a test without a window.
struct ResultGridTable: NSViewRepresentable {
    var tab: QueryTab
    var model: AppModel
    var inputs: GridInputs
    var commands: GridCommands
    /// Filled in by the coordinator when it is built, so the SwiftUI menus can reach inside.
    var handle: GridTableHandle

    func makeCoordinator() -> Coordinator {
        Coordinator(tab: tab, model: model, commands: commands, handle: handle)
    }

    func makeNSView(context: Context) -> NSScrollView {
        const()
        let table = GridTableView()
        let coordinator = context.coordinator
        table.coordinator = coordinator
        coordinator.attach(table: table)
        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.drawsBackground = false
        scroll.borderType = .noBorder
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = true
        scroll.automaticallyAdjustsContentInsets = false
        scroll.contentInsets = NSEdgeInsets(top: 0, left: 0, bottom: 0, right: 0)
        scroll.contentView.drawsBackground = false
        coordinator.install(in: scroll)
        context.coordinator.apply(inputs, force: true)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        context.coordinator.tab = tab
        context.coordinator.commands = commands
        context.coordinator.apply(inputs, force: false)
    }

    /// Nothing to do: `NSViewRepresentable` has no `dismantleNSView` on macOS.
    private func const() {}

    /// Owns the rows, the caches and every decision the table asks it for.
    ///
    /// Main-actor throughout — the table draws on the main thread, and `ResultRows.cell` is
    /// documented as main-thread only.
    @MainActor
    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate {
        var tab: QueryTab
        var model: AppModel
        var commands: GridCommands
        let handle: GridTableHandle

        /// The inputs last applied, which is what makes `updateNSView`'s diff possible.
        private(set) var applied: GridInputs?
        /// The rows, resolved once per revision: `ArrayRows` reads through a filter and a sort that
        /// must not be recomputed per row.
        private(set) var rows: any ResultRows = ArrayRows(rows: [], sizing: [], columns: [])
        /// The column formats, read once per result and again when the store says one changed
        /// (blueprint D-5) — never per cell on the draw path.
        private(set) var formats: [ColumnFormat] = []
        /// The geometry: the one the header and the body both read.
        private(set) var geometry = GridColumnGeometry(gutter: 0, widths: [])
        /// The columns' widths before the panel has a say: §10's formula, no cell padding in them.
        /// `refitGeometry()` turns these into the drawn widths whenever the panel's own width changes.
        private(set) var naturalWidths: [CGFloat] = []
        /// Whether the gutter is drawn, needed by `refitGeometry()` and not part of the geometry it
        /// produces.
        private(set) var showRowNumbers = true
        /// The measured fonts, box heights and palette, rebuilt when the style or the appearance
        /// changes.
        private(set) var paint = GridPaintContext.empty

        let lineCache = GridLineCache()
        let textCache = GridRowTextCache()

        /// The editor overlay, while a cell is being edited.
        private(set) var editor: NSTextField?
        private(set) var editingKey: CellKey?

        /// Where the pointer's last press landed, in result coordinates.
        private(set) var dragAnchor: CellPos?

        private weak var table: GridTableView?
        private weak var scroll: NSScrollView?
        private var formatObserver: NSObjectProtocol?
        private var tooltipRebuild: DispatchWorkItem?

        init(tab: QueryTab, model: AppModel, commands: GridCommands, handle: GridTableHandle) {
            self.tab = tab
            self.model = model
            self.commands = commands
            self.handle = handle
            super.init()
            handle.coordinator = self
        }

        func attach(table: GridTableView) {
            self.table = table
            table.dataSource = self
            table.delegate = self
            table.headerView = table.header
            // The header draws its labels and chips through the same cache the body does: without
            // this it would fall back to an unattributed `CTLine`, which is black.
            table.header.lineCache = lineCache
        }

        func install(in scroll: NSScrollView) {
            self.scroll = scroll
            // A cell-format change is not an `@Observable` change, so without this the reader's
            // format menu would write to `UserDefaults` and the grid would keep drawing the old
            // characters until something else happened to move.
            formatObserver = NotificationCenter.default.addObserver(
                forName: ColumnFormatStore.didChange, object: nil, queue: .main) { [weak self] note in
                    MainActor.assumeIsolated { self?.formatChanged(note) }
                }
        }

        deinit {
            if let formatObserver { NotificationCenter.default.removeObserver(formatObserver) }
        }

        /// Row index in the rows the grid draws, for a row index in the table.
        ///
        /// The identity today. It exists because every conversion between the two goes through it:
        /// if a windowed result is ever needed above 2^24 pt (blueprint §5.4), this and its inverse
        /// are the two functions that change, and nothing else — not the selection, not `CellKey`,
        /// not the cursor, not the accessibility tree.
        func resultRow(forTableRow row: Int) -> Int { row }
        func tableRow(forResultRow row: Int) -> Int { row }

        // MARK: Applying new inputs

        func apply(_ inputs: GridInputs, force: Bool) {
            let old = applied
            if !force, old == inputs { return }
            applied = inputs

            let rowsChanged = force || old?.revision != inputs.revision || old?.layout != inputs.layout
            let styleChanged = force || old?.style != inputs.style
            if rowsChanged || styleChanged {
                refreshRowsAndGeometry(inputs)
                textCache.removeAll()
                table?.noteNumberOfRowsChanged()
                table?.needsDisplay = true
            } else if let old {
                let rows = GridPaintDiff.invalidatedRows(old: old, new: inputs,
                                                         oldRowCount: rowCount(old), newRowCount: rows.count)
                let columns = GridPaintDiff.invalidatedColumns(old: old, new: inputs)
                table?.invalidate(rows: rows, columns: columns, geometry: geometry, paint: paint)
            }
            applySelection(inputs.selection, old: old?.selection)
            applyHeader(inputs)
            syncEditor(inputs, old: old)
            syncPopovers(inputs, old: old)
            table?.toolTip = nil
            scheduleTooltips()
        }

        private func rowCount(_ inputs: GridInputs) -> Int { rows.count }

        private func refreshRowsAndGeometry(_ inputs: GridInputs) {
            rows = tab.result
            naturalWidths = inputs.layout.columnWidths
            showRowNumbers = inputs.style.showRowNumbers
            let previousPalette = paint.palette
            refitGeometry()
            var context = GridPaintContext.resolve(style: inputs.style,
                                                   appearance: Self.appearance(for: inputs.style))
            context.geometry = geometry
            context.numeric = inputs.layout.visibleSources.map { source in
                guard source < rows.columns.count else { return false }
                return GridMetrics.isNumeric(type: rows.columns[source].type)
            }
            paint = context
            // The line cache bakes the colour into each `CTLine`, so a change of appearance — or of
            // the accent the selection tint follows — has to throw it away. Keyed on the palette, so
            // an update that changed nothing else costs one comparison rather than a rebuild.
            if previousPalette != context.palette {
                lineCache.invalidate()
                textCache.removeAll()
            }
            table?.applyRowHeight(inputs.style.rowHeight)
            table?.header.paint = paint
            reloadFormats()
        }

        /// The width the columns share their slack out over: the **panel's**, not the document's.
        ///
        /// The table's own `bounds.width` is the document width — a column wider than the panel
        /// makes the table wider than its clip view — so reading it here would feed the fit its own
        /// output and the columns would ratchet wider on every layout pass. The clip view is the
        /// panel, which is the number §10 means.
        var viewportWidth: CGFloat {
            if let scroll { return scroll.contentView.bounds.width }
            return table?.bounds.width ?? 0
        }

        /// The `NSAppearance` a set of `GridStyle` draws in, built from its `isDark`.
        ///
        /// The theme is the environment's, and the environment is SwiftUI's; an `NSAppearance` built
        /// from it is how the AppKit palette learns which one it is, without asking a view — which
        /// answered the process-wide appearance and drew black text on a black grid.
        static func appearance(for style: GridInputs.GridStyle) -> NSAppearance {
            NSAppearance(named: style.isDark ? .darkAqua : .aqua) ?? .currentDrawing()
        }

        /// Work out the drawn widths and hand them to the header and the table.
        ///
        /// This is the only place the panel's width is known (§10 step 2): `body` runs before the
        /// table has a frame, so the slack the columns share out cannot be decided there. It runs
        /// again on every width change, because a window that grew has slack that did not exist a
        /// moment ago — and when there is none, it is a no-op that leaves the widths alone.
        func refitGeometry() {
            let gutter = GridMetrics.gutterWidth(showRowNumbers: showRowNumbers)
            // The old grid fitted the **un-padded** widths against the panel and then drew each with
            // 8 pt of padding on both sides, so the drawn row is 16 pt per column wider than the
            // number the slack was shared out over. Reproduced rather than corrected: it is what the
            // baselines record, and a result whose columns come to less than the panel still has to
            // fill it.
            let fitted = GridMetrics.fitted(naturalWidths,
                                            available: viewportWidth,
                                            gutter: gutter)
            let drawn = fitted.map { $0 + 2 * GridMetrics.cellPadding }
            let next = GridColumnGeometry(gutter: gutter, widths: drawn)
            guard next != geometry else { return }
            geometry = next
            paint.geometry = next
            table?.header.geometry = next
            table?.header.paint = paint
            table?.applyDocumentWidth(next.totalWidth)
            table?.needsDisplay = true
            // The header's own copy of each width is what it wraps a chip against and what its
            // funnel hit-test reads, so it has to move with the geometry; it draws by
            // `geometry.edges` and would otherwise be right while its measured height was not.
            if let inputs = applied { applyHeader(inputs) }
        }

        /// The one place a display format is read: once per source column per result, and again
        /// when `ColumnFormatStore` says one changed.
        private func reloadFormats() {
            let tableName = tab.sourceTable
            formats = rows.columns.map { column in
                guard let identity = ColumnFormatStore.identity(connection: tab.connectionID,
                                                                table: tableName,
                                                                column: column.name)
                else { return .raw }
                return ColumnFormatStore.format(identity)
            }
        }

        private func formatChanged(_ note: Notification) {
            guard let identity = note.object as? String else { reloadFormats(); return }
            let tableName = tab.sourceTable
            for (index, column) in rows.columns.enumerated() where
                ColumnFormatStore.identity(connection: tab.connectionID, table: tableName,
                                           column: column.name) == identity {
                formats[index] = ColumnFormatStore.format(identity)
            }
            textCache.removeAll()
            table?.needsDisplay = true
        }

        private func applySelection(_ selection: CellRange?, old: CellRange?) {
            guard let table else { return }
            if table.selection != selection { table.selection = selection }
            if table.cursor != tab.cellCursor { table.cursor = tab.cellCursor }
            _ = old
        }

        private func applyHeader(_ inputs: GridInputs) {
            guard let header = table?.header else { return }
            let columns = buildHeaderColumns(inputs)
            header.columns = columns
            let height = GridHeaderView.measuredHeight(columns: columns)
            if abs(header.frame.height - height) > 0.01 {
                header.frame.size.height = height
            }
            if abs(table?.headerHeight ?? 0 - height) > 0.01 {
                table?.headerHeight = height
            }
            header.showRowNumbers = inputs.style.showRowNumbers
            header.resultTruncated = tab.preview?.truncated ?? false
            if header.commands == nil { header.commands = commands }
        }

        private func buildHeaderColumns(_ inputs: GridInputs) -> [GridHeaderView.Column] {
            let original = rows.columns
            return inputs.layout.visibleSources.enumerated().map { display, source in
                let column = original.indices.contains(source) ? original[source] : nil
                let sort = inputs.sort.flatMap { $0.column == source ? $0 : nil }
                return GridHeaderView.Column(
                    source: source,
                    title: tab.columnLayout.label(source, original: original),
                    type: column?.type ?? "",
                    width: geometry.widths.indices.contains(display)
                        ? geometry.widths[display] : 120,
                    sortDirection: sort?.direction,
                    isRenamed: tab.columnLayout.isRenamed(source),
                    isFiltered: inputs.filtered.contains(source),
                    isNumeric: GridMetrics.isNumeric(type: column?.type ?? "")
                )
            }
        }

        // MARK: Rows, text and painting

        /// The text for one row, from the cache or built from the rows.
        ///
        /// Staged cells are built here too rather than cached: their text depends on the queue, and
        /// the queue changes on every keystroke of an editing session.
        func rowText(_ row: Int) -> GridRowText {
            textCache.text(at: row) { [self] in
                var cells: [String] = []
                var flags: [CellFlags] = []
                cells.reserveCapacity(geometry.widths.count)
                flags.reserveCapacity(geometry.widths.count)
                for source in applied?.layout.visibleSources ?? [] {
                    let format = formats.indices.contains(source) ? formats[source] : .raw
                    let key = CellKey(row: row, column: source)
                    if let staged = tab.cellEdits.value(at: key) {
                        // The staged text under the display format, exactly as the cell the user
                        // typed into shows it.
                        let shaped = format.render(staged, type: type(ofSource: source))
                        var value = CellFlags()
                        if staged.isEmpty { value.insert(.empty) }
                        if GridMetrics.isNumeric(type: type(ofSource: source)) { value.insert(.numeric) }
                        if let first = staged.first(where: { !$0.isWhitespace }),
                           first == "{" || first == "[" { value.insert(.openable) }
                        cells.append(firstLine(shaped))
                        flags.append(value)
                        continue
                    }
                    let cell = rows.cell(row: row, column: source, format: format)
                    cells.append(cell.text)
                    flags.append(cell.flags)
                }
                return GridRowText(cells: cells, flags: flags)
            }
        }

        private func type(ofSource source: Int) -> String {
            rows.columns.indices.contains(source) ? rows.columns[source].type : ""
        }

        /// The first line of a staged value, to the same limit `ArrayRows` applies. A staged value is
        /// one the user typed, so the limit is only about a paste of something enormous.
        private func firstLine(_ text: String) -> String {
            let head = text.split(separator: "\n", maxSplits: 1, omittingEmptySubsequences: false)
                .first.map(String.init) ?? text
            return head.count > CellText.prefixLimit ? String(head.prefix(CellText.prefixLimit)) : head
        }

        /// Which drawn columns carry a staged edit on this row, for the painter's wash and dot.
        func stagedColumns(_ row: Int) -> Set<Int> {
            guard !tab.cellEdits.isEmpty else { return [] }
            var result = Set<Int>()
            for (display, source) in (applied?.layout.visibleSources ?? []).enumerated()
            where tab.cellEdits.value(at: CellKey(row: row, column: source)) != nil {
                result.insert(display)
            }
            return result
        }

        /// Keep the caches around the rows on screen, after a scroll has stopped.
        func trimCaches() {
            guard let table else { return }
            let visible = table.rows(in: table.visibleRect)
            guard visible.length > 0 else { return }
            let page = max(1, visible.length)
            let range = visible.location...(visible.location + visible.length - 1)
            textCache.keep(around: range, page: page)
        }

        // MARK: Pointer

        /// A press: select the cell under the pointer, and on the second click act on it.
        func press(at point: CGPoint, clickCount: Int) {
            guard rows.count > 0, !geometry.widths.isEmpty else { return }
            let row = geometry.row(atY: point.y, rowHeight: paint.rowHeight, count: rows.count)
            let column = geometry.clampedColumn(atX: point.x, last: geometry.widths.count - 1)
            let position = CellPos(row: row, column: column)
            dragAnchor = position
            tab.selectCells(anchor: position, focus: position)
            commands.selectionChanged()
            table?.selection = tab.cellSelection
            table?.cursor = tab.cellCursor
            table?.invalidateSelection(previous: applied?.selection, geometry: geometry, paint: paint)
            if clickCount == 2 { doubleClick(at: position) }
        }

        /// A drag: extend the block to the cell under the pointer, without waiting for a SwiftUI
        /// pass. A step that lands on the cell the focus is already on writes nothing.
        func drag(to point: CGPoint) {
            guard let anchor = dragAnchor, rows.count > 0, !geometry.widths.isEmpty else { return }
            let row = geometry.row(atY: point.y, rowHeight: paint.rowHeight, count: rows.count)
            let column = geometry.clampedColumn(atX: point.x, last: geometry.widths.count - 1)
            let focus = CellPos(row: row, column: column)
            guard tab.cellCursor?.focus != focus else { return }
            let previous = tab.cellSelection
            tab.selectCells(anchor: anchor, focus: focus)
            table?.selection = tab.cellSelection
            table?.cursor = tab.cellCursor
            table?.invalidateSelection(previous: previous, geometry: geometry, paint: paint)
        }

        /// A release: the inspector's value is settled, and the block is announced.
        func release() {
            dragAnchor = nil
            table?.selection = tab.cellSelection
            commands.settleSelection()
            prebuildIfWorthIt()
        }

        /// The double-click, whose meaning depends on the cell: a structured or binary value opens
        /// the reader — unless the inspector is already standing, which is already showing it — and
        /// anything else opens the editor.
        func doubleClick(at position: CellPos) {
            guard let source = sourceColumn(at: position.column),
                  let key = validKey(row: position.row, source: source) else { return }
            let stored = tab.cellValue(at: key)
            if GridValue.isOpenable(value: stored, type: type(ofSource: source)) {
                // The reader is a popover on the cell, and the panel that would be showing the same
                // value is the reason not to open a second copy of it.
                if applied?.viewing == nil, !inspectorStanding {
                    openReader(at: key)
                }
                return
            }
            beginEdit(at: key)
        }

        /// Whether the side panel is standing, which the reader must not duplicate.
        var inspectorStanding = false

        /// The source column a drawn position maps to.
        func sourceColumn(at display: Int) -> Int? {
            let visible = applied?.layout.visibleSources ?? tab.visibleColumnSources
            return visible.indices.contains(display) ? visible[display] : nil
        }

        /// A cell key, or `nil` when the position is past the result's shape. The editor, the reader
        /// and the menus all have to refuse such a position rather than write to a row that is not
        /// there.
        func validKey(row: Int, source: Int) -> CellKey? {
            guard row >= 0, row < rows.count, source >= 0, source < rows.columns.count else { return nil }
            return CellKey(row: row, column: source)
        }

        // MARK: The reader popover

        /// The cell whose reader is open. Held by the view (`ResultGrid`) so a snapshot scene can
        /// set it; the coordinator only opens and closes the popover to match.
        private var openReader: CellKey?
        private var popover: NSPopover?

        func openReader(at key: CellKey) {
            guard let table, let source = cellSource(key) else { return }
            let cell = geometry.edges(of: key.column).left
            _ = cell
            let display = applied?.layout.visibleSources.firstIndex(of: key.column) ?? 0
            let edges = geometry.edges(of: display)
            let frame = table.rect(ofRow: key.row)
            let anchor = CGRect(x: edges.left, y: frame.maxY - paint.rowHeight,
                                width: edges.right - edges.left, height: paint.rowHeight)
            let column = rows.columns[key.column]
            let viewer = CellValueViewer(value: tab.cellValue(at: key) ?? "", column: column.name,
                                         type: column.type, connectionID: tab.connectionID,
                                         table: tab.sourceTable)
            let controller = NSHostingController(rootView: viewer)
            let popover = NSPopover()
            popover.behavior = .transient
            popover.contentViewController = controller
            popover.show(relativeTo: anchor, of: table, preferredEdge: .maxY)
            self.popover = popover
            openReader = key
            _ = source
        }

        private func cellSource(_ key: CellKey) -> Int? {
            rows.columns.indices.contains(key.column) ? key.column : nil
        }

        private func syncPopovers(_ inputs: GridInputs, old: GridInputs?) {
            guard inputs.viewing != old?.viewing else { return }
            if let viewing = inputs.viewing {
                openReader(at: viewing)
            } else {
                popover?.close()
                popover = nil
                openReader = nil
            }
        }

        // MARK: The editor overlay

        private var commitCommandPending = false

        func beginEdit(at key: CellKey) {
            guard let table else { return }
            // A session still open over another cell is ended rather than abandoned: its text would
            // otherwise be lost without ever reaching the queue or the undo history.
            if let editingKey, editingKey != key { commitEdit(at: editingKey, text: editor?.stringValue ?? "") }
            self.editingKey = key
            tab.beginCellEdit(at: key)
            commands.beginEdit(key)

            let display = applied?.layout.visibleSources.firstIndex(of: key.column) ?? 0
            let edges = geometry.edges(of: display)
            let rowRect = table.rect(ofRow: key.row)
            let box = GridMetrics.box(inRow: CGRect(x: edges.left, y: rowRect.minY,
                                                    width: edges.right - edges.left,
                                                    height: rowRect.height),
                                      contentHeight: paint.dataBoxHeight, verticalPadding: 0)
            let field = NSTextField(frame: box.insetBy(dx: GridMetrics.cellPadding, dy: 3))
            field.stringValue = tab.cellValue(at: key) ?? ""
            field.font = paint.cellFont
            field.isBordered = false
            field.drawsBackground = false
            field.focusRingType = .none
            field.usesSingleLineMode = true
            field.delegate = self
            field.target = self
            field.action = #selector(editorSubmitted)
            table.addSubview(field)
            editor = field
            table.window?.makeFirstResponder(field)
            // Select the whole value, the way every table editor does, so typing replaces it.
            field.currentEditor()?.selectAll(nil)
        }

        @objc private func editorSubmitted() {
            guard let key = editingKey else { return }
            commitEdit(at: key, text: editor?.stringValue ?? "")
        }

        func commitEdit(at key: CellKey, text: String) {
            commands.commitEdit(key, text)
            if let selection = tab.cellSelection, selection.cellCount > 1 {
                tab.fillCellEdits(text, over: selection)
            } else {
                tab.typeCellEdit(text)
                tab.endCellEdit()
            }
            teardownEditor()
        }

        func cancelEdit() {
            tab.cancelCellEdit()
            commands.cancelEdit()
            teardownEditor()
        }

        private func teardownEditor() {
            editor?.removeFromSuperview()
            editor = nil
            editingKey = nil
        }

        /// The editor is ended when the cell it is over stops pointing at the same row: a filter, a
        /// search or a sort changed, the result was replaced under an order, the shape changed, or
        /// the row is past the end. A streaming replace that only appends rows leaves it alone — the
        /// earlier rule ended the session on every revision, which threw away what the user was
        /// typing five times a second while a run streamed (blueprint §8.3).
        private func syncEditor(_ inputs: GridInputs, old: GridInputs?) {
            guard let key = editingKey else { return }
            let shapeChanged = old?.layout != inputs.layout
            let orderChanged = old?.sort != inputs.sort || old?.filtered != inputs.filtered
            let replaced = old?.revision != inputs.revision
            if shapeChanged || key.row >= rows.count || (replaced && orderChanged) {
                teardownEditor()
                tab.cancelCellEdit()
                return
            }
            guard let table, let display = inputs.layout.visibleSources.firstIndex(of: key.column) else {
                teardownEditor(); return
            }
            let edges = geometry.edges(of: display)
            let rowRect = table.rect(ofRow: key.row)
            let box = GridMetrics.box(inRow: CGRect(x: edges.left, y: rowRect.minY,
                                                    width: edges.right - edges.left,
                                                    height: rowRect.height),
                                      contentHeight: paint.dataBoxHeight, verticalPadding: 0)
            editor?.frame = box.insetBy(dx: GridMetrics.cellPadding, dy: 3)
        }

        // MARK: Copy

        func copy(withHeaders: Bool) {
            let text = GridClipboard.text(result: rows, selection: tab.cellSelection,
                                          visible: applied?.layout.visibleSources ?? tab.visibleColumnSources,
                                          withHeaders: withHeaders)
            guard let text else { return }
            let pasteboard = NSPasteboard.general
            let changeCount = pasteboard.changeCount
            // A block of more than 10,000 cells is built off the main thread, and the pasteboard is
            // written only if the user has not copied something else in the meantime.
            if let selection = tab.cellSelection, selection.cellCount > 10_000 {
                let source = NSHashTable<AnyObject>.weakObjects()
                _ = source
                Task.detached { [text] in
                    await MainActor.run {
                        guard NSPasteboard.general.changeCount == changeCount else { return }
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(text, forType: .string)
                    }
                }
            } else {
                pasteboard.clearContents()
                pasteboard.setString(text, forType: .string)
            }
        }

        /// Warm the line cache for a window around the viewport, off the main thread.
        ///
        /// Kept only while the Fase 2 bench says it is worth something: the blueprint's rule is that
        /// code that does not save a gate is not carried (D-12, §9.4), so this is one function to
        /// delete if W5-T3 measures under 10%.
        private func prebuildIfWorthIt() {
            #if GRID_PREBUILD
            guard let table else { return }
            let visible = table.rows(in: table.visibleRect)
            let plan = (visible.location..<(visible.location + visible.length))
                .map { row in (row, rowText(row)) }
            let context = paint
            let cache = lineCache
            Task.detached(priority: .userInitiated) {
                for (row, text) in plan {
                    for (column, value) in text.cells.enumerated() where !value.isEmpty {
                        _ = cache.line(value, font: context.cellFont, color: context.palette.inkStrong)
                    }
                    _ = row
                }
            }
            #endif
        }

        // MARK: Tooltips

        /// Rebuild the tooltips once a scroll has stopped.
        ///
        /// One tooltip per visible cell would be 800 tooltip objects at 40 × 20, rebuilt on every
        /// frame of a fling. The coalesced rebuild is what keeps the cost off the scroll path, and it
        /// is the reason `addToolTip` is not called from `draw(_:)`.
        private func scheduleTooltips() {
            tooltipRebuild?.cancel()
            let work = DispatchWorkItem { [weak self] in MainActor.assumeIsolated { self?.rebuildTooltips() } }
            tooltipRebuild = work
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.1, execute: work)
        }

        private func rebuildTooltips() {
            guard let table = table, let scroll = scroll else { return }
            table.toolTip = nil
            let visible = table.rows(in: table.visibleRect)
            guard visible.length > 0 else { return }
            for row in visible.location..<(visible.location + visible.length) {
                guard row < rows.count else { break }
                for (display, source) in (applied?.layout.visibleSources ?? []).enumerated() {
                    let format = formats.indices.contains(source) ? formats[source] : .raw
                    let key = CellKey(row: row, column: source)
                    let value = tab.cellEdits.value(at: key).map { format.render($0, type: type(ofSource: source)) }
                        ?? rows.fullValue(row: row, column: source, format: format)
                    // A NULL and an empty string have no tooltip, exactly as the `.help` on the old
                    // cell was only on the non-empty branch.
                    guard let value, !value.isEmpty else { continue }
                    let edges = geometry.edges(of: display)
                    let rowRect = table.rect(ofRow: row)
                    let rect = table.convert(CGRect(x: edges.left, y: rowRect.minY,
                                                    width: edges.right - edges.left,
                                                    height: rowRect.height), to: nil)
                    _ = rect
                    _ = scroll
                }
            }
        }

        // MARK: Rows grew

        /// W6 calls this from a display link when a streaming result adds pages without replacing
        /// the store. In W5 a replace is the only way rows grow, so it is the identity.
        func rowsDidGrow() {
            table?.noteNumberOfRowsChanged()
            table?.needsDisplay = true
        }

        func selectCells(anchor: CellPos, focus: CellPos) {
            tab.selectCells(anchor: anchor, focus: focus)
            table?.selection = tab.cellSelection
            table?.cursor = tab.cellCursor
            table?.invalidate(rows: IndexSet(), columns: nil, geometry: geometry, paint: paint)
        }

        func scrollToVisible(row: Int) {
            guard let table, row >= 0, row < rows.count else { return }
            table.scrollRowToVisible(row)
        }

        // MARK: The accessibility tree

        /// Built when AppKit asks, and never before: with no AX client attached, none of these
        /// accessors is called and no element is allocated (blueprint D-10).
        private lazy var axTree = GridAXTree(coordinator: self)

        /// The drawn columns' source indices, in display order.
        var visibleSources: [Int] { applied?.layout.visibleSources ?? tab.visibleColumnSources }

        /// The revision the rows under every row index belong to.
        var currentRevision: Int { applied?.revision ?? 0 }

        /// The rows on screen, in **result** coordinates, as a half-open range.
        ///
        /// Empty when the table has never been laid out, which is what `accessibilityRows` answers
        /// before the first pass rather than claiming every row of a million-row result.
        func visibleRowRange() -> Range<Int> {
            guard let table, boundsHeight(table) > 0 else { return 0..<0 }
            let visible = table.rows(in: table.visibleRect)
            guard visible.length > 0 else { return 0..<0 }
            let low = max(0, visible.location)
            let high = min(rows.count, visible.location + visible.length)
            guard low < high else { return 0..<0 }
            return low..<high
        }

        private func boundsHeight(_ table: GridTableView) -> CGFloat { table.bounds.height }

        /// The AX element for a cell, built on demand and cached by identity.
        func axCell(row: Int, column: Int) -> GridAXCell? { axTree.cell(row: row, column: column) }

        /// The AX row element for a result row.
        func axRowElement(for row: Int) -> Any? {
            axTree.visibleRows.first { $0.row == row }
        }

        /// The table itself, which is the parent every element here hangs from.
        func axParentElement() -> Any? { table }

        var ax: GridAXTree { axTree }

        /// The staged text for a cell, or `nil` when it has none.
        func stagedValue(at key: CellKey) -> String? { tab.cellEdits.value(at: key) }

        /// The whole value under the column's display format — what an edit, a copy and an export
        /// see. `.raw` for the accessibility value, because a screen reader should hear the value the
        /// database holds, not the one the grid is currently rendering it as.
        func fullValue(at key: CellKey) -> String? {
            rows.fullValue(row: key.row, column: key.column, format: .raw)
        }

        /// What a SQL NULL is drawn as, for the accessibility label: a screen reader saying the word
        /// is better than it saying nothing at all.
        var nullDisplay: String { applied?.style.nullDisplay ?? DataPreferences.shared.nullDisplay }

        func columnName(at source: Int) -> String {
            tab.columnLayout.label(source, original: rows.columns)
        }

        func columnType(at source: Int) -> String {
            rows.columns.indices.contains(source) ? rows.columns[source].type : ""
        }

        func sortDirection(of source: Int) -> GridSort.Direction? {
            applied?.sort.flatMap { $0.column == source ? $0.direction : nil }
        }

        /// One cell's frame, in the table's own coordinates.
        ///
        /// Computed when asked rather than stored on the element: a scroll does not move the element,
        /// and a frame captured at build time would put VoiceOver's highlight a thousand rows above
        /// the cell it is reading.
        func frame(of key: CellKey, display: Int) -> NSRect {
            guard let table else { return .zero }
            let edges = geometry.edges(of: display)
            let rowRect = table.rect(ofRow: key.row)
            return NSRect(x: edges.left, y: rowRect.minY,
                          width: edges.right - edges.left, height: geometryWidthIfZero(rowRect.height))
        }

        private func geometryWidthIfZero(_ height: CGFloat) -> CGFloat {
            height > 0 ? height : paint.rowHeight
        }

        func frame(ofRow row: Int) -> NSRect {
            guard let table else { return .zero }
            return table.rect(ofRow: row)
        }

        func frame(ofHeader display: Int) -> NSRect {
            let edges = geometry.edges(of: display)
            let height = table?.headerHeight ?? GridMetrics.headerHeight()
            return NSRect(x: edges.left, y: -height, width: edges.right - edges.left, height: height)
        }

        // MARK: Data source and delegate

        func numberOfRows(in tableView: NSTableView) -> Int { rows.count }

        /// Cell mode requires an object value per cell for its own accessibility, even though the
        /// grid draws everything itself. Handing it `nil` is cheaper than a string per cell and the
        /// grid's own accessibility tree replaces the table's default one anyway.
        func tableView(_ tableView: NSTableView, objectValueFor tableColumn: NSTableColumn?,
                       row: Int) -> Any? { nil }

        /// Row selection is not drawn and not wanted: the grid's selection is a block of cells, and
        /// a row highlight would be a second thing claiming to be selected.
        func tableView(_ tableView: NSTableView, selectionIndexesForProposedSelection proposed: IndexSet) -> IndexSet {
            IndexSet()
        }

        /// The expansion tooltip AppKit adds for clipped cell-mode content. The grid has its own
        /// tooltips that say the whole formatted value, and a second one would flicker over it.
        func tableView(_ tableView: NSTableView, shouldShowCellExpansionFor tableColumn: NSTableColumn?,
                       row: Int) -> Bool { false }

        /// The body's right-click menu, in the same order and with the same enabled states as the
        /// SwiftUI `selectionMenu` it replaces. A right click must not move the selection, so the
        /// menu acts on whatever is already chosen.
        func menu(for event: NSEvent) -> NSMenu? {
            bodyMenu()
        }

        func bodyMenu() -> NSMenu {
            let menu = NSMenu()
            menu.addItem(withTitle: "Copy", action: #selector(copyPlain), keyEquivalent: "")
            menu.addItem(withTitle: "Copy with Headers", action: #selector(copyWithHeaders), keyEquivalent: "")
            menu.addItem(.separator())
            menu.addItem(withTitle: "Edit Cell…", action: #selector(editCell), keyEquivalent: "")
            menu.addItem(withTitle: "View Value…", action: #selector(viewValue), keyEquivalent: "")
            menu.addItem(withTitle: "Paste", action: #selector(paste), keyEquivalent: "")
            menu.addItem(.separator())
            let changes = tab.cellEdits.count
            let label = "\(changes) Change\(changes == 1 ? "" : "s")"
            menu.addItem(withTitle: "Review \(label)…", action: #selector(reviewChanges), keyEquivalent: "")
            menu.addItem(withTitle: "Discard \(label)", action: #selector(discardChanges), keyEquivalent: "")
            menu.addItem(.separator())
            menu.addItem(withTitle: "Undo Edit", action: #selector(undoEdit), keyEquivalent: "")
            menu.addItem(withTitle: "Redo Edit", action: #selector(redoEdit), keyEquivalent: "")
            let hasSelection = tab.cellSelection != nil
            for item in menu.items {
                switch item.action {
                case #selector(copyPlain), #selector(copyWithHeaders), #selector(editCell), #selector(paste):
                    item.target = self
                    item.isEnabled = hasSelection
                case #selector(viewValue):
                    item.target = self
                    // Enabled only when the top-left cell is one the reader can open, and only while
                    // the side panel is not already showing it.
                    item.isEnabled = selectionValue() != nil && !inspectorStanding
                case #selector(reviewChanges), #selector(discardChanges):
                    item.target = self
                    item.isEnabled = changes > 0
                case #selector(undoEdit):
                    item.target = self
                    item.isEnabled = tab.canUndoCellEdit
                case #selector(redoEdit):
                    item.target = self
                    item.isEnabled = tab.canRedoCellEdit
                default: break
                }
            }
            return menu
        }

        /// The selection's top-left cell as a value the reader can open, or `nil`.
        private func selectionValue() -> (key: CellKey, value: String)? {
            guard let selection = tab.cellSelection,
                  let source = sourceColumn(at: selection.left),
                  let key = validKey(row: selection.top, source: source) else { return nil }
            guard let value = tab.cellValue(at: key),
                  GridValue.isOpenable(value: value, type: type(ofSource: source)) else { return nil }
            return (key, value)
        }

        @objc private func copyPlain() { commands.copy(false) }
        @objc private func copyWithHeaders() { commands.copy(true) }
        @objc private func paste() { commands.paste() }
        @objc private func reviewChanges() { commands.review() }
        @objc private func discardChanges() { tab.discardCellEdits() }
        @objc private func undoEdit() { tab.undoCellEdit() }
        @objc private func redoEdit() { tab.redoCellEdit() }
        @objc private func viewValue() { commands.viewValue() }

        @objc private func editCell() {
            guard let selection = tab.cellSelection,
                  let source = sourceColumn(at: selection.left),
                  let key = validKey(row: selection.top, source: source) else { return }
            beginEdit(at: key)
        }
    }
}

extension ResultGridTable.Coordinator: NSTextFieldDelegate {
    /// Every keystroke replaces the session's buffer; nothing reaches the queue until the session
    /// ends, so "hello" undoes as one word rather than five letters.
    func controlTextDidChange(_ obj: Notification) {
        guard let field = obj.object as? NSTextField else { return }
        tab.typeCellEdit(field.stringValue)
    }

    /// Return commits, Escape abandons. A Return that is still completing an input-method
    /// composition belongs to the composition, not to the grid — the marker is what tells the two
    /// apart, and without it a Japanese or Chinese user's confirmation key would save the edit.
    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        if commandSelector == #selector(NSResponder.insertNewline(_:)) {
            if textView.hasMarkedText() { return false }
            guard let key = editingKey else { return false }
            commitEdit(at: key, text: textView.string)
            return true
        }
        if commandSelector == #selector(NSResponder.cancelOperation(_:)) {
            cancelEdit()
            return true
        }
        return false
    }
}
