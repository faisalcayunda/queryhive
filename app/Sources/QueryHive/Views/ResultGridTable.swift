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
        /// The cell and header-label font, in points. A field of the style so that a change in
        /// Settings is a change of the inputs, which is what makes the grid repaint and re-measure.
        var fontSize = DataPreferences.standardGridFontSize
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

    func makeCoordinator() -> Coordinator {
        Coordinator(tab: tab, model: model, commands: commands)
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
    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate,
        NSViewToolTipOwner {
        var tab: QueryTab
        var model: AppModel
        var commands: GridCommands

        /// The inputs last applied, which is what makes `updateNSView`'s diff possible.
        private(set) var applied: GridInputs?
        /// The rows, resolved once per revision: the tab's store, or the empty stand-in.
        var rows: any ResultRows = EmptyRows()
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
        private var scrollObserver: NSObjectProtocol?
        private var tooltipRebuild: DispatchWorkItem?
        /// The peek (P-17), made when it is first asked for and kept for reuse.
        private(set) var peek: CellPeekPanel?
        /// Which cell the peek was last asked about, and which ask is the latest: a read that comes
        /// back for an older one is dropped, so a fast run of arrow keys cannot land out of order.
        private var peekKey: CellKey?
        private var peekGeneration = 0
        private var peekPending = false
        private var peekReadingTimer: DispatchWorkItem?
        private var announceTimer: DispatchWorkItem?
        /// The registered tooltip region in window coordinates, which is what lets
        /// `GridToolTip.local` say which coordinate system AppKit's point is in.
        private var tooltipWindowRect: NSRect?

        init(tab: QueryTab, model: AppModel, commands: GridCommands) {
            self.tab = tab
            self.model = model
            self.commands = commands
            super.init()
        }

        func attach(table: GridTableView) {
            self.table = table
            table.dataSource = self
            table.delegate = self
            table.headerView = table.header
            // The header draws its labels and chips through the same cache the body does: without
            // this it would fall back to an unattributed `CTLine`, which is black.
            table.header.lineCache = lineCache
            tab.pollHook = { [weak self] in MainActor.assumeIsolated { self?.pollRows() } }
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
            // The tooltip region is registered in window coordinates, so scrolling the content out
            // from under it leaves the pointer outside every region (§8.5: rebuilt 100 ms after the
            // scroll stops, coalesced — never per frame).
            scroll.contentView.postsBoundsChangedNotifications = true
            scrollObserver = NotificationCenter.default.addObserver(
                forName: NSView.boundsDidChangeNotification, object: scroll.contentView, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated {
                    self?.scheduleTooltips()
                    self?.followPeek()
                }
            }
        }

        deinit {
            if let formatObserver { NotificationCenter.default.removeObserver(formatObserver) }
            if let scrollObserver { NotificationCenter.default.removeObserver(scrollObserver) }
            tooltipRebuild?.cancel()
            peekReadingTimer?.cancel()
            announceTimer?.cancel()
            let panel = peek
            DispatchQueue.main.async { MainActor.assumeIsolated { panel?.dismiss() } }
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
            // The cell a peek was showing belongs to rows that are no longer these.
            if rowsChanged, !force { closePeek() }
            if rowsChanged || styleChanged {
                refreshRowsAndGeometry(inputs)
                textCache.removeAll()
                table?.noteNumberOfRowsChanged()
                table?.needsDisplay = true
                // Only if a client is watching: forcing the lazy tree here would allocate it on
                // every result for nobody, which is the thing D-10 forbids.
                if axClientAttached { axTree.invalidate() }
                noteResultChangedForAX()
                table?.isPolling = rows.isLive
            } else if let old {
                let rows = GridPaintDiff.invalidatedRows(old: old, new: inputs,
                                                         oldRowCount: rowCount(old), newRowCount: rows.count)
                let columns = GridPaintDiff.invalidatedColumns(old: old, new: inputs)
                table?.invalidate(rows: rows, columns: columns, geometry: geometry, paint: paint)
            }
            applySelection(inputs.selection, old: old?.selection)
            // Nothing for a peek to be about once the cursor is gone (a new result, a filter).
            if isPeekOpen, tab.cellCursor == nil { closePeek() }
            applyHeader(inputs)
            syncEditor(inputs, old: old)
            syncPopovers(inputs, old: old)
            scheduleTooltips()
        }

        private func rowCount(_ inputs: GridInputs) -> Int { rows.count }

        private func refreshRowsAndGeometry(_ inputs: GridInputs) {
            rows = tab.result
            tab.pollHook = { [weak self] in MainActor.assumeIsolated { self?.pollRows() } }
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
            // The registered regions are in window coordinates, and a refit moves both the columns
            // inside them and the document they sit on. Debounced, so a resize drag is one rebuild
            // 100 ms after it stops rather than one per frame of it.
            scheduleTooltips()
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
            // Before any text is rebuilt: the store drops the pages whose format changed.
            rows.prepare(formats: formats)
        }

        private func formatChanged(_ note: Notification) {
            guard let identity = note.object as? String else { reloadFormats(); return }
            let tableName = tab.sourceTable
            for (index, column) in rows.columns.enumerated() where
                ColumnFormatStore.identity(connection: tab.connectionID, table: tableName,
                                           column: column.name) == identity {
                formats[index] = ColumnFormatStore.format(identity)
            }
            rows.prepare(formats: formats)
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
            let height = GridHeaderView.measuredHeight(columns: columns,
                                                       labelSize: CGFloat(inputs.style.fontSize))
            table?.headerHeight = height
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
                    filterLabel: tab.columnFilters[source]?.label,
                    isNumeric: GridMetrics.isNumeric(type: column?.type ?? "")
                )
            }
        }

        // MARK: Rows, text and painting

        /// The text for one row, from the cache or built from the rows.
        ///
        /// A row with a staged edit is **built, never cached**: its text depends on the queue, and
        /// the queue changes on every keystroke of an editing session, while the cache is only
        /// thrown away by a revision, a style or a palette change. Caching it meant a commit kept
        /// drawing the old value under its new wash, and a discard kept drawing the typed one.
        ///
        /// Only the display columns in `columns` (D-23), widened by a viewport either side so a
        /// short horizontal scroll does not rebuild the row. A 500-column result would otherwise
        /// pay 500 cell reads per row drawn, almost all of them off screen.
        func rowText(_ row: Int, columns: Range<Int>) -> GridRowText {
            let sources = applied?.layout.visibleSources ?? []
            let wanted = widened(columns, count: sources.count)
            let build: (Range<Int>) -> GridRowText = { [self] range in
                var cells: [String] = []
                var flags: [CellFlags] = []
                cells.reserveCapacity(range.count)
                flags.reserveCapacity(range.count)
                for source in sources[range] {
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
                return GridRowText(first: range.lowerBound, cells: cells, flags: flags)
            }
            guard !tab.cellEdits.hasStagedEdit(row: row) else { return build(wanted) }
            return textCache.text(at: row, columns: wanted, build: build)
        }

        /// `columns` plus the columns one viewport's width to its left and right, clamped to what
        /// exists. A request already wider than the viewport (a full-width dirty rect) gains the
        /// same margin, which is cheap next to the 500 it avoids.
        private func widened(_ columns: Range<Int>, count: Int) -> Range<Int> {
            let all = 0..<count
            let asked = columns.clamped(to: all)
            guard !asked.isEmpty, let table else { return asked }
            let margin = max(table.visibleRect.width, 1)
            let left = geometry.edges(of: asked.lowerBound).left
            let right = geometry.edges(of: asked.upperBound - 1).right
            return geometry.columns(in: max(0, left - margin)...(right + margin)).clamped(to: all)
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
            // A click in the grid is the end of a peek (blueprint 3.3).
            closePeek()
            let row = geometry.row(atY: point.y, rowHeight: paint.rowHeight, count: rows.count)
            let column = geometry.clampedColumn(atX: point.x, last: geometry.widths.count - 1)
            let position = CellPos(row: row, column: column)
            dragAnchor = position
            // What is on screen, not what SwiftUI last applied: a key may have moved the selection
            // since, and repainting the older block would leave the newer one painted.
            let previousSelection = tab.cellSelection
            let previousCursor = tab.cellCursor?.focus
            tab.selectCells(anchor: position, focus: position)
            commands.selectionChanged()
            syncTable(previousSelection: previousSelection, previousCursor: previousCursor)
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
            if tab.cellSelection != nil { announceSelectionForAX() }
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
                tab.cancelCellEdit()  // the fill replaces the session; do not leave it open
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
            let text: String
            do {
                guard let built = try GridClipboard.text(result: rows, selection: tab.cellSelection,
                                                         visible: applied?.layout.visibleSources ?? tab.visibleColumnSources,
                                                         withHeaders: withHeaders) else { return }
                text = built
            } catch {
                if (error as? StoreFailure)?.isStale != true { tab.note(.error, "Copy failed: \(error)") }
                return
            }
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
                .map { row in (row, rowText(row, columns: 0..<geometry.widths.count)) }
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

        /// One region over everything the pointer can reach, registered after that 100 ms.
        ///
        /// Not one region per cell: §8.5's alternative (P-6) is a single rectangle and a string
        /// resolved from the point, so 40 × 20 cells cost one registration instead of 800 and a
        /// fling costs nothing at all — the pointer cannot move during the 100 ms because there is
        /// nothing per frame to keep up with.
        private func rebuildTooltips() {
            guard let table else { return }
            table.removeAllToolTips()
            tooltipWindowRect = nil
            table.header.scheduleTooltips()
            let region = table.visibleRect
            guard region.width > 1, region.height > 1 else { return }
            table.addToolTip(region, owner: self, userData: nil)
            tooltipWindowRect = table.convert(region, to: nil)
        }

        /// A stand-in for where the pointer is, in window coordinates, for a test: a window that is
        /// not on screen has no pointer to ask.
        var pointerOverride: (() -> NSPoint?)?

        /// Where the pointer is now, in window coordinates, while the window is on screen.
        private func restingPointer() -> NSPoint? {
            if let pointerOverride { return pointerOverride() }
            guard let window = table?.window, window.isVisible else { return nil }
            return window.mouseLocationOutsideOfEventStream
        }

        /// The tooltip for the cell under the pointer — the whole value in the column's display
        /// format (§8.5), which is what the reader and an export see.
        ///
        /// Empty means *no tooltip*: a NULL and an empty string had no `.help` on the old cell, and
        /// with one region over the whole body the absence has to be expressed by the answer
        /// rather than by not registering the cell.
        func view(_ view: NSView, stringForToolTip tag: NSView.ToolTipTag, point: NSPoint,
                  userData: UnsafeMutableRawPointer?) -> String {
            guard let table, let inputs = applied, rows.count > 0, !geometry.widths.isEmpty
            else { return "" }
            let sources = inputs.layout.visibleSources
            guard !sources.isEmpty else { return "" }
            let local = GridToolTip.local(point: point, in: table, registered: tooltipWindowRect,
                                          pointer: restingPointer())
            // The pointer over the row-number gutter belongs to no column, and `clampedColumn`
            // would silently hand it the first one instead of nothing.
            guard table.bounds.contains(local), local.x >= geometry.gutter else { return "" }
            let display = geometry.clampedColumn(atX: local.x, last: geometry.widths.count - 1)
            guard sources.indices.contains(display) else { return "" }
            let source = sources[display]
            let row = geometry.row(atY: local.y, rowHeight: paint.rowHeight, count: rows.count)
            guard row >= 0, row < rows.count else { return "" }
            let format = formats.indices.contains(source) ? formats[source] : .raw
            let key = CellKey(row: row, column: source)
            let value = tab.cellEdits.value(at: key).map { format.render($0, type: type(ofSource: source)) }
                ?? rows.fullValue(row: row, column: source, format: format)
            guard let value, !value.isEmpty else { return "" }
            return GridToolTip.cap(value)
        }

        // MARK: The keyboard

        /// What the key map needs to know about the grid right now.
        var keyState: GridKeyState {
            let selection = tab.cellSelection
            return GridKeyState(hasCursor: tab.cellCursor != nil,
                                hasSelection: selection != nil,
                                selectionIsBlock: (selection?.cellCount ?? 0) > 1,
                                peekOpen: isPeekOpen,
                                editing: editingKey != nil)
        }

        /// Where the cursor can go: the table's rows, the drawn columns, and a page of rows.
        var gridBounds: GridBounds {
            let visible = visibleRowRange()
            return GridBounds(rows: rows.count, columns: geometry.widths.count,
                              page: visible.upperBound - visible.lowerBound)
        }

        /// A key, from `GridTableView.keyDown`. `true` when the grid took it.
        func handleKey(_ key: GridKey) -> Bool {
            guard let action = GridKeyMap.action(for: key, in: keyState) else { return false }
            switch action {
            case .move(let motion, let extending): moveCursor(motion, extending: extending)
            case .activate:
                if let focus = tab.cellCursor?.focus { doubleClick(at: focus) }
            case .togglePeek: togglePeek()
            case .cancelEdit: cancelEdit()
            case .closePeek: closePeek()
            case .collapseSelection:
                if let focus = tab.cellCursor?.focus {
                    place(GridCursor(anchor: focus, focus: focus), extending: false)
                }
            case .clearSelection: clearSelection()
            case .deleteRows: break  // W10-T3 draws and stages them; the map never offers it before
            }
            return true
        }

        /// One motion of the cursor. With no cursor yet the first key lands on the first cell on
        /// screen, which is where a person who tabbed in is looking.
        private func moveCursor(_ motion: GridMotion, extending: Bool) {
            // No cell to land on (no rows, or no drawn columns): Tab must still leave, because
            // `keyDown` swallows a key the coordinator took and the grid always accepts the keyboard.
            guard rows.count > 0, !geometry.widths.isEmpty else { leaveGrid(motion); return }
            guard let cursor = tab.cellCursor else {
                let first = CellPos(row: firstUsableRow(), column: 0)
                place(GridCursor(anchor: first, focus: first), extending: false)
                return
            }
            guard let next = GridCursorMath.apply(motion, extending: extending, to: cursor, in: gridBounds) else {
                leaveGrid(motion)  // Tab past the first or last cell
                return
            }
            guard next != cursor else { return }
            place(next, extending: extending)
        }

        /// Tab and Shift-Tab hand the keyboard to the next or previous key view; every other motion
        /// stops at the edge.
        private func leaveGrid(_ motion: GridMotion) {
            if motion == .next { table?.window?.selectNextKeyView(table) }
            if motion == .previous { table?.window?.selectPreviousKeyView(table) }
        }

        /// The first row that is wholly on screen and not under the header, which is where a
        /// person who has just arrived is looking.
        private func firstUsableRow() -> Int {
            guard let scroll, paint.rowHeight > 0 else { return 0 }
            let clip = scroll.contentView
            let top = clip.bounds.minY + clip.contentInsets.top
            return min(max(0, Int(ceil(top / paint.rowHeight))), max(0, rows.count - 1))
        }

        /// Put the cursor (and with it the block) somewhere, the way a click does: the tab's own
        /// writer, the table's copies, only the cells that changed repainted, the cell scrolled
        /// into view, and the inspector and the peek told.
        private func place(_ cursor: GridCursor, extending: Bool) {
            let previousSelection = tab.cellSelection
            let previousCursor = tab.cellCursor?.focus
            tab.selectCells(anchor: cursor.anchor, focus: cursor.focus)
            syncTable(previousSelection: previousSelection, previousCursor: previousCursor)
            commands.selectionChanged()
            commands.settleSelection()
            scrollToVisible(cell: cursor.focus)
            if isPeekOpen { showPeek(at: cursor.focus, opening: false) }
            cursorMovedForAX(extending: extending)
        }

        /// Esc on a single cell: nothing is selected any more, and the cursor stays where it was so
        /// the next arrow starts from there rather than from the top.
        private func clearSelection() {
            let previousSelection = tab.cellSelection
            let previousCursor = tab.cellCursor
            tab.cellSelection = nil
            tab.cellCursor = previousCursor
            syncTable(previousSelection: previousSelection, previousCursor: previousCursor?.focus)
            commands.selectionChanged()
            commands.settleSelection()
        }

        /// The table's copies of the selection and the cursor, and a repaint of what changed: the
        /// cells the selection gained or lost, and the one the ring left and the one it reached.
        private func syncTable(previousSelection: CellRange?, previousCursor: CellPos?) {
            table?.selection = tab.cellSelection
            table?.cursor = tab.cellCursor
            table?.invalidateSelection(previous: previousSelection, geometry: geometry, paint: paint)
            for cell in GridPaintDiff.cursorInvalidations(old: previousCursor, new: tab.cellCursor?.focus) {
                table?.invalidate(cell: cell, geometry: geometry, paint: paint)
            }
        }

        /// The table gained or lost the keyboard: the ring goes between full strength and 40%.
        func focusChanged() {
            guard let table, let focus = table.cursor?.focus else { return }
            table.invalidate(cell: focus, geometry: geometry, paint: paint)
        }

        /// The window became or stopped being key: the same repaint, and a peek does not outlive it.
        func windowKeyChanged() {
            focusChanged()
            guard isPeekOpen, let window = table?.window else { return }
            // One turn later: while the key moves from this window to another, nobody is key yet.
            DispatchQueue.main.async { [weak self] in
                MainActor.assumeIsolated {
                    if !window.isKeyWindow { self?.closePeek() }
                }
            }
        }

        // MARK: Scrolling to the cursor

        /// A cell's rectangle in the table's own coordinates. The first column starts at the left
        /// edge, so reaching it brings the row numbers back with it.
        func rect(ofCell position: CellPos) -> CGRect {
            guard let table, position.column >= 0, position.column < geometry.widths.count else { return .zero }
            let edges = geometry.edges(of: position.column)
            let rowRect = table.rect(ofRow: position.row)
            return CGRect(x: position.column == 0 ? 0 : edges.left, y: rowRect.minY,
                          width: edges.right - (position.column == 0 ? 0 : edges.left),
                          height: rowRect.height)
        }

        /// The least scrolling that shows a cell, once per key press and without animation.
        func scrollToVisible(cell position: CellPos) {
            guard let scroll, position.row >= 0, position.row < rows.count else { return }
            let clip = scroll.contentView
            let target = GridScrollMath.origin(revealing: rect(ofCell: position), in: clip.bounds,
                                               insets: clip.contentInsets)
            guard target != clip.bounds.origin else { return }
            let constrained = clip.constrainBoundsRect(NSRect(origin: target, size: clip.bounds.size))
            clip.scroll(to: constrained.origin)
            scroll.reflectScrolledClipView(clip)
        }

        // MARK: The peek

        /// Whether a peek has been asked for and not closed: from the key press, through the read,
        /// to the panel being up. The cursor moving while the first read is still in flight must
        /// carry the peek along, which "the panel is visible" would not.
        var isPeekOpen: Bool { peekKey != nil }

        /// Space and Cmd-Y: open the peek on the cursor's cell, or close it.
        func togglePeek() {
            if isPeekOpen { closePeek(); return }
            guard let focus = tab.cellCursor?.focus else { return }
            showPeek(at: focus, opening: true)
        }

        func closePeek() {
            peekGeneration += 1
            peekReadingTimer?.cancel()
            peekPending = false
            peekKey = nil
            peek?.dismiss()
        }

        /// Read one cell and show it. The read is `rowsOrThrow` for one row and one column, on a
        /// background queue (a ten-megabyte value must not cost a frame), and the panel appears
        /// when the value does, or after 50 ms as "Reading..." if it takes longer.
        private func showPeek(at position: CellPos, opening: Bool) {
            guard table?.window != nil, let source = sourceColumn(at: position.column),
                  let key = validKey(row: position.row, source: source) else { closePeek(); return }
            let panel = peek ?? CellPeekPanel()
            peek = panel
            peekGeneration += 1
            let generation = peekGeneration
            peekKey = key
            peekPending = true
            peekReadingTimer?.cancel()
            let column = rows.columns[source]

            // A staged edit is what the cell shows, and there is nothing to read for it.
            if let staged = tab.cellEdits.value(at: key) {
                peekShow(staged, note: "A staged edit, not written yet.", position: position, column: column,
                         announce: opening)
                return
            }

            let reading = DispatchWorkItem { [weak self] in
                MainActor.assumeIsolated {
                    guard let self, self.peekGeneration == generation, self.peekPending else { return }
                    self.peek?.showReading(anchor: self.screenRect(ofCell: position),
                                           appearance: self.table?.window?.effectiveAppearance)
                }
            }
            peekReadingTimer = reading
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.05, execute: reading)

            let reader = PeekReader(rows: rows)
            Task.detached(priority: .userInitiated) { [weak self] in
                let outcome = Result { try reader.value(row: key.row, column: source) }
                await self?.peekDidRead(outcome, generation: generation, position: position,
                                        column: column, announce: opening)
            }
        }

        private func peekDidRead(_ outcome: Result<String?, Error>, generation: Int, position: CellPos,
                                 column: Event.Column, announce: Bool) {
            guard generation == peekGeneration else { return }
            peekReadingTimer?.cancel()
            switch outcome {
            case .success(let value):
                peekShow(value, note: value == nil ? "This cell is NULL." : nil, position: position,
                         column: column, announce: announce)
            case .failure(let error):
                peekPending = false
                // Said in the panel and not clipped to nothing: a blank peek would pass for an
                // empty value, which is the wrong thing to believe about a cell that failed to read.
                let message = (error as? StoreFailure)?.isStale == true
                    ? "These rows are gone." : "Could not read this cell."
                peek?.showFailure(message, anchor: screenRect(ofCell: position),
                                  appearance: table?.window?.effectiveAppearance)
            }
        }

        private func peekShow(_ value: String?, note: String?, position: CellPos, column: Event.Column,
                              announce: Bool) {
            peekPending = false
            peek?.show(value: value ?? "", column: column.name, type: column.type,
                       connectionID: tab.connectionID, table: tab.sourceTable, extraNote: note,
                       anchor: screenRect(ofCell: position),
                       appearance: table?.window?.effectiveAppearance)
            if announce {
                Announcer.post("Peek: \(column.name), \(String((value ?? "null").prefix(160)))")
            }
        }

        /// A cell's rectangle on screen, which is what the panel hangs under.
        private func screenRect(ofCell position: CellPos) -> CGRect {
            guard let table, let window = table.window else { return .zero }
            return window.convertToScreen(table.convert(rect(ofCell: position), to: nil))
        }

        /// The grid scrolled, or the window moved: the panel follows its cell, and goes if the cell
        /// has left the window.
        func followPeek() {
            guard isPeekOpen, let table, let focus = tab.cellCursor?.focus else { return }
            guard table.visibleRect.intersects(rect(ofCell: focus)) else { closePeek(); return }
            peek?.follow(anchor: screenRect(ofCell: focus))
        }

        // MARK: VoiceOver and the cursor

        /// The last cell a keyboard move told VoiceOver about, for the tests: the real notification
        /// goes to the system and nothing in a test process can read it back.
        private(set) var lastAXFocusPost: CellKey?

        /// VoiceOver put its focus on a cell: the cursor goes there, and the selection stays.
        ///
        /// The second writer of the cursor, besides `QueryTab.selectCells` (blueprint 3.4). A screen
        /// reader's focus is not a choice: reading down a column must not select every cell it
        /// passes, so the block is left alone and the cursor may rest outside it. Nothing is
        /// announced, because VoiceOver is already reading the cell it moved to.
        func focusCellFromAX(_ key: CellKey) {
            guard let display = visibleSources.firstIndex(of: key.column), key.row >= 0,
                  key.row < rows.count else { return }
            let position = CellPos(row: key.row, column: display)
            let previous = tab.cellCursor?.focus
            guard tab.cellCursor != GridCursor(anchor: position, focus: position) else { return }
            tab.cellCursor = GridCursor(anchor: position, focus: position)
            syncTable(previousSelection: tab.cellSelection, previousCursor: previous)
            scrollToVisible(cell: position)
            if isPeekOpen { showPeek(at: position, opening: false) }
        }

        /// Whether a cell is where the cursor is, which is what VoiceOver's focused attribute says.
        func isAXFocused(_ key: CellKey) -> Bool {
            guard let focus = tab.cellCursor?.focus, let display = visibleSources.firstIndex(of: key.column)
            else { return false }
            return focus == CellPos(row: key.row, column: display)
        }

        /// Whether the whole row is marked for deletion, for the label (W10-T3 does the marking).
        func isMarkedForDeletion(row: Int) -> Bool { tab.cellEdits.isDeleted(row) }

        /// A keyboard move: VoiceOver is told its focus moved, and an *extension* is announced once,
        /// 250 ms after the last key that made it, so holding Shift-Down reads one total and not one
        /// per row. A plain move is read by VoiceOver itself, as the new cell.
        private func cursorMovedForAX(extending: Bool) {
            guard axClientAttached else { return }
            if let cell = axTree.focusedCell {
                lastAXFocusPost = cell.key
                NSAccessibility.post(element: cell, notification: .focusedUIElementChanged)
            }
            announceTimer?.cancel()
            guard extending else { return }
            let work = DispatchWorkItem { [weak self] in
                MainActor.assumeIsolated { self?.announceSelectionForAX() }
            }
            announceTimer = work
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.25, execute: work)
        }

        // MARK: Rows grew

        /// One display tick, or a wake-up from the run's first `progress`: ask the store what it
        /// learned, grow the table by the new rows, and stop the link once the rows are final.
        func pollRows() {
            let result = rows.poll()
            if let from = result.grewFrom { rowsDidGrow(from: from, to: rows.count) }
            table?.isPolling = rows.isLive
        }

        /// A streaming result added rows to the view on screen, without replacing it.
        func rowsDidGrow(from: Int, to: Int) {
            table?.noteNumberOfRowsChanged()
            guard let table, from < to else { return }
            // The rows already drawn did not change, so neither the text cache nor the geometry
            // is touched (D-28): only the new rows' rects repaint.
            table.invalidate(rows: IndexSet(integersIn: from..<to), columns: nil,
                             geometry: geometry, paint: paint)
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

        /// Every row there is, for `accessibilityRowCount` — the count must not follow the viewport
        /// that `accessibilityRows` is limited to.
        var rowCountForAX: Int { rows.count }

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

        // MARK: AX notifications

        /// Whether an accessibility client has ever asked for the tree.
        ///
        /// Every notification below is gated on it (blueprint §11.2, D-10): before the first
        /// `accessibilityChildren` there is nothing allocated and nobody listening, so posting then
        /// would be work done for no one — which is the whole property D-10 exists to keep.
        private(set) var axClientAttached = false

        /// Called from the table's first `accessibilityChildren`. Idempotent.
        func noteAXClientAttached() {
            axClientAttached = true
            axTree.trim()
        }

        /// The last selection announcement `release()` made.
        ///
        /// Kept so a test can read it: the real one is posted to the application element, which is
        /// where `NSAccessibilityAnnouncementKey` is documented to go, and nothing in a test process
        /// can observe that.
        private(set) var lastAXAnnouncement: String?

        /// "3 × 2 cells selected" (FR-GRID-07), posted on release and never per drag step: an
        /// announcement on every mouse-moved pixel would make VoiceOver talk over itself.
        func announceSelectionForAX() {
            guard axClientAttached, let table else { return }
            let selection = tab.cellSelection
            let height = selection.map { max(1, $0.bottom - $0.top + 1) } ?? 0
            let width = selection.map { max(1, $0.right - $0.left + 1) } ?? 0
            lastAXAnnouncement = "\(height) × \(width) cells selected"
            NSAccessibility.post(element: NSApp as Any, notification: .selectedCellsChanged)
            NSAccessibility.post(element: table, notification: .selectedCellsChanged)
            NSAccessibility.post(element: NSApp as Any, notification: .announcementRequested,
                                 userInfo: [.announcement: lastAXAnnouncement as Any])
        }

        /// `.layoutChanged` when a new result takes over the rows, again only for a live client.
        func noteResultChangedForAX() {
            guard axClientAttached, let table else { return }
            NSAccessibility.post(element: table, notification: .layoutChanged)
        }

        /// One column's frame across the rows on screen — the `.column` element's own extent, not
        /// the header's. Computed when asked, so a scroll that moves the rows moves it too.
        func frame(ofColumn display: Int) -> NSRect {
            guard let table else { return .zero }
            let edges = geometry.edges(of: display)
            let range = visibleRowRange()
            guard range.lowerBound < range.upperBound else { return .zero }
            let top = table.rect(ofRow: range.lowerBound).minY
            let bottom = table.rect(ofRow: range.upperBound - 1).maxY
            return NSRect(x: edges.left, y: top, width: edges.right - edges.left,
                          height: max(0, bottom - top))
        }

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
