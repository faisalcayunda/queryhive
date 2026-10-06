import AppKit
import CoreText
import QuartzCore

/// The result grid: one `NSTableView`, one table column, and every pixel drawn by hand.
///
/// The design decision this file is (blueprint D-1, D-2): a **cell-based** table with a single
/// `NSTableColumn` as wide as the whole result, whose `draw(_:)` paints the gutter, the stripes, the
/// selection and staged washes, the separators and the text of every visible row into one context.
/// Three things fall out of that and none of them would if each row had its own view:
///
///  - the paint order and the one-point overflow Compact cells have over their neighbours are
///    SwiftUI's own, because the rows are painted in order into one context;
///  - a fling allocates nothing, where a row view per row allocates on every frame;
///  - a 500-column result does not have 500 `NSTableColumn` objects whose widths have to be kept in
///    step with the header (the cost TablePro measured as quadratic).
///
/// AppKit is still doing the work that makes a table a table: the clip view, the scrollers, the
/// tile-based invalidation that asks for the rectangle that just appeared, and the header riding
/// above the content. What it is *not* doing is drawing anything.
final class GridTableView: NSTableView {

    /// The rows, the caches and every decision. Weak: the coordinator is owned by SwiftUI.
    weak var coordinator: ResultGridTable.Coordinator?

    /// The header, kept as a strong reference of its own so the coordinator can hand it the columns
    /// and the geometry without reaching through `super`.
    let header = GridHeaderView()

    /// The selection as a block of cells, in **result** coordinates — never table rows, so a
    /// windowed result (blueprint §5.4) would move no selection.
    var selection: CellRange?

    /// The cell cursor (P-24): what the keyboard and a VoiceOver focus move, drawn as a ring on its
    /// focus cell. A copy of `QueryTab.cellCursor`, the way `selection` is one of the selection.
    var cursor: GridCursor?

    /// Whether this table is the first responder, kept by `become`/`resignFirstResponder` rather
    /// than read from the window: during `resignFirstResponder` the window still names this table.
    private(set) var isKeyboardFocused = false

    /// How strongly the ring is drawn: fully while the grid has the keyboard in the key window, and
    /// at 40% otherwise, so the cursor stays findable while the focus is in the peek or elsewhere
    /// (blueprint D-3).
    var cursorStrength: CGFloat {
        isKeyboardFocused && window?.isKeyWindow == true ? 1 : 0.4
    }

    /// Whether the body's own menu has been asked for at least once, which is what tells a plain
    /// right-click from one AppKit synthesised for the header.
    private var lastDirty = CGRect.zero

    // MARK: Polling

    private var displayLink: CADisplayLink?
    private var keyObservers: [NSObjectProtocol] = []

    deinit {
        keyObservers.forEach(NotificationCenter.default.removeObserver)
    }

    /// Whether the rows can still change under the grid, which is what keeps the display link
    /// running. The coordinator turns it on while a result streams or a view is being applied, and
    /// off after one last poll once everything is terminal.
    var isPolling = false {
        didSet { displayLink?.isPaused = !isPolling }
    }

    /// The link lives only while the table is in a window: a tab in the background has none, so it
    /// does not poll, and a tab that comes back to the front polls once at once (§17.3).
    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        displayLink?.invalidate()
        displayLink = nil
        keyObservers.forEach(NotificationCenter.default.removeObserver)
        keyObservers = []
        guard let window else {
            // Out of a window (a tab switched away, a tab closed): a peek has nothing to sit under.
            coordinator?.closePeek()
            return
        }
        PerfSignposts.stamp(.gridAttached, onlyFirst: true)
        // Whether the window is key is part of how strongly the ring is drawn, and losing it
        // closes the peek (blueprint W10 §3.3).
        keyObservers = [NSWindow.didBecomeKeyNotification, NSWindow.didResignKeyNotification].map { name in
            NotificationCenter.default.addObserver(forName: name, object: window, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.coordinator?.windowKeyChanged() }
            }
        }
        let link = displayLink(target: self, selector: #selector(displayTick(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 30, maximum: 60, preferred: 60)
        link.isPaused = !isPolling
        // `.common`, so it keeps ticking while a scroll is being tracked.
        link.add(to: .main, forMode: .common)
        displayLink = link
        coordinator?.pollRows()
    }

    @objc private func displayTick(_ link: CADisplayLink) {
        coordinator?.pollRows()
    }

    // MARK: Configuration

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        configure()
    }

    required init?(coder: NSCoder) {
        super.init(coder: coder)
        configure()
    }

    private func configure() {
        // The row height is the grid's, not the system's: `rowSizeStyle`'s default would override it.
        rowSizeStyle = .custom
        rowHeight = DataPreferences.shared.rowPoints
        usesAutomaticRowHeights = false
        intercellSpacing = .zero
        gridStyleMask = []
        usesAlternatingRowBackgroundColors = false
        selectionHighlightStyle = .none
        allowsColumnReordering = false
        allowsColumnResizing = false
        allowsColumnSelection = false
        allowsMultipleSelection = false
        allowsTypeSelect = false
        allowsEmptySelection = true
        columnAutoresizingStyle = .noColumnAutoresizing
        focusRingType = .none
        style = .plain
        backgroundColor = .clear
        // No header-styled square in the corner over the scrollers: the legacy scrollers that
        // produced it are gone, and an empty corner is what the grid has always shown.
        cornerView = nil
        // Nothing of AppKit's: the one column below exists only so the table has a column to lay
        // out and a header to hang, and the header draws itself too.
        headerView = header
        // AppKit gives a new header its own default height; start at the grid's so the first
        // `applyHeader` is usually not a change at all.
        header.setFrameSize(NSSize(width: header.frame.width, height: GridMetrics.headerHeight()))

        // Blueprint §11.2. The role is the one AppKit would report anyway; the label is not, and
        // "Result grid" is what VoiceOver says before it starts reading cells.
        setAccessibilityRole(.table)
        setAccessibilityLabel("Result grid")

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("grid"))
        column.minWidth = 1
        // The default 1000 would clamp a 500-column document at 1000 pt and silently stop the
        // horizontal scroll at a fifth of the way across.
        column.maxWidth = .greatestFiniteMagnitude
        column.resizingMask = []
        addTableColumn(column)
    }

    /// Whether this table is flipped. It is: rows run downwards from the top, which is what makes
    /// `rect(ofRow:)` and the painter's own y arithmetic agree.
    override var isFlipped: Bool { true }

    /// The height of the header, which is the columns' own measured height.
    ///
    /// Every caller goes through here. The scroll view is what sits the clip view under the header
    /// (its `contentInsets.top` is the header's height), so a header that grows has to retile the
    /// scroll view and not only the table, or the first rows are drawn under the band. The clip
    /// view keeps its place unless it was at the top, in which case it stays at the top.
    var headerHeight: CGFloat {
        get { header.frame.height }
        set {
            // A layout pass applies the header, so a no-op must stay a no-op.
            guard abs(header.frame.height - newValue) > 0.01 else { return }
            let clip = enclosingScrollView?.contentView
            let wasAtTop = clip.map { abs($0.bounds.origin.y + $0.contentInsets.top) < 0.5 } ?? false
            header.setFrameSize(NSSize(width: header.frame.width, height: newValue))
            tile()
            guard let scroll = enclosingScrollView, let clip else { return }
            scroll.tile()
            if wasAtTop {
                clip.scroll(to: NSPoint(x: clip.bounds.origin.x, y: -clip.contentInsets.top))
                scroll.reflectScrolledClipView(clip)
            }
        }
    }

    /// Set the row height without letting AppKit's own resizing do anything else.
    func applyRowHeight(_ height: CGFloat) {
        guard rowHeight != height else { return }
        rowHeight = height
        noteNumberOfRowsChanged()
        needsDisplay = true
    }

    /// The width of the document: the gutter plus every drawn column.
    func applyDocumentWidth(_ width: CGFloat) {
        let current = tableColumns.first?.width ?? 0
        guard abs(current - width) > 0.01 else { return }
        tableColumns.first?.width = width
    }

    /// A width change is the only thing that can move the columns: the panel's width is what the
    /// slack is shared out against, and it is not known until AppKit has laid the table out
    /// (blueprint §10 step 2). The coordinator recomputes and no-ops when the width is the same, so
    /// a layout pass with nothing behind it costs one comparison.
    override func layout() {
        super.layout()
        coordinator?.refitGeometry()
    }

    /// The palette follows `GridStyle.isDark`, which is set from the environment, so an appearance
    /// flip moves the inputs and repaints the table through the ordinary update path.
    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        needsDisplay = true
    }

    // MARK: Pointer

    /// A press, a drag and a release, converted once and handed to the coordinator (§12.3).
    ///
    /// `super` is not called on any of the three, and that is deliberate: AppKit's own press would
    /// run the delegate's selection, which the coordinator suppresses because the selection here is
    /// a block of *cells*, not a set of table rows, and it would start an edit in the cell-mode
    /// cell this table never draws. The header does the same for the same reason.
    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        coordinator?.press(at: convert(event.locationInWindow, from: nil),
                           clickCount: event.clickCount)
    }

    override func mouseDragged(with event: NSEvent) {
        coordinator?.drag(to: convert(event.locationInWindow, from: nil))
    }

    override func mouseUp(with event: NSEvent) {
        coordinator?.release()
    }

    // MARK: Accessibility

    /// The table's own AX attributes, answered from the tree rather than from AppKit's.
    ///
    /// The overrides exist because the table is cell-based with **one** `NSTableColumn`: AppKit's
    /// default tree honestly describes that column and would tell VoiceOver a 500-column result has
    /// one (blueprint §11.2). Every attribute below is one VoiceOver actually reads, so leaving any
    /// of them to `super` would keep a single lie in an otherwise correct tree.
    ///
    /// Nothing here builds anything until it is called, and the first `accessibilityChildren` is
    /// what arms the notifications (D-10).
    override func accessibilityChildren() -> [Any]? {
        coordinator?.noteAXClientAttached()
        return coordinator?.ax.visibleChildren ?? []
    }

    override func accessibilityRows() -> [any NSAccessibilityRow]? { coordinator?.ax.visibleRows ?? [] }

    override func accessibilityColumns() -> [Any]? { coordinator?.ax.columns ?? [] }

    override func accessibilityVisibleRows() -> [any NSAccessibilityRow]? {
        coordinator?.ax.visibleRows ?? []
    }

    override func accessibilityVisibleCells() -> [Any]? { coordinator?.ax.visibleCells ?? [] }

    override func accessibilityColumnHeaderUIElements() -> [Any]? {
        coordinator?.ax.columnHeaders ?? []
    }

    /// For any position, on screen or not: VoiceOver's table navigation asks for a cell and *then*
    /// scrolls it into view, so refusing one outside the viewport would strand it on page one.
    override func accessibilityCell(forColumn column: Int, row: Int) -> Any? {
        coordinator?.ax.cell(row: row, column: column)
    }

    override func accessibilitySelectedCells() -> [Any]? { coordinator?.ax.selectedCells ?? [] }

    override var accessibilityFocusedUIElement: Any? { coordinator?.ax.focusedCell ?? self }
    /// The **real** row count, not the visible one: `accessibilityRows` deliberately answers only
    /// with the viewport, and a table that claims 40 rows because 40 are on screen would make
    /// VoiceOver announce "row 40 of 40" for a million-row result.
    override func accessibilityRowCount() -> Int { coordinator?.rowCountForAX ?? 0 }

    override func accessibilityColumnCount() -> Int { coordinator?.visibleSources.count ?? 0 }

    // MARK: Drawing

    /// Every pixel of the table, in the order the SwiftUI grid painted them.
    ///
    /// `super` is not called: it would draw the (one) cell-mode cell and the row backgrounds the
    /// grid has switched off anyway, and its paint would land on top of this.
    override func draw(_ dirtyRect: NSRect) {
        guard let coordinator, let cg = NSGraphicsContext.current?.cgContext else { return }
        lastDirty = dirtyRect
        let context = coordinator.paint
        guard !context.geometry.widths.isEmpty else { return }

        cg.saveGState()
        // Flipped view: y runs down, so the text matrix has to point back up or every glyph is
        // mirrored. `textPosition` stays in the view's own coordinates, which is what the painter
        // writes.
        cg.textMatrix = CGAffineTransform(scaleX: 1, y: -1)
        // The palette's alphas are meant to be laid down once. A partial invalidation repaints a
        // rectangle that overlaps one already painted, and `copy` would make the second coat double
        // the first at the edges; the grid's rects are on row boundaries so there is no seam, and the
        // idempotence test is what holds this to it.
        cg.setBlendMode(.normal)

        let rowCount = coordinator.rows.count
        guard rowCount > 0 else { cg.restoreGState(); return }
        PerfSignposts.stamp(.firstDraw, onlyFirst: true)

        // The rows the dirty rectangle covers, plus the neighbours a Compact cell overflows onto.
        let overflow = overflowRows(context)
        let first = max(0, Int(floor(dirtyRect.minY / context.rowHeight)) - overflow)
        let last = min(rowCount - 1, Int(floor(dirtyRect.maxY / context.rowHeight)) + overflow)

        // The columns it covers, plus the gutter when the rectangle reaches the left edge.
        let columns = context.geometry.columns(in: dirtyRect.minX...max(dirtyRect.maxX, dirtyRect.minX))

        let ring = cursor?.focus
        let strength = cursorStrength
        for row in first...last {
            let text = coordinator.rowText(row, columns: columns)
            GridRowPainter.paint(row: row,
                                 columns: columns,
                                 context: context,
                                 text: text,
                                 staged: coordinator.stagedColumns(row),
                                 rowState: coordinator.rowState(row),
                                 selection: selection,
                                 cursorColumn: ring?.row == row ? ring?.column : nil,
                                 cursorStrength: strength,
                                 lines: coordinator.lineCache,
                                 width: bounds.width,
                                 into: cg)
        }

        // The signpost the SwiftUI grid fired from row 0's `onAppear`, moved to the one place that
        // knows the first row has actually been painted.
        if first == 0, dirtyRect.minY < context.rowHeight * 2 {
            PerfSignposts.firstPaint()
        }
        cg.restoreGState()
    }

    /// How many rows either side a cell box can spill onto.
    ///
    /// Zero at every row height but Compact: 23 pt and 25 pt boxes both fit a 25 pt row, while a
    /// 21 pt row is one and two points short of them. The painter's row order is what decides which
    /// row wins where they overlap, exactly as SwiftUI's did.
    private func overflowRows(_ context: GridPaintContext) -> Int {
        let tallest = max(context.dataBoxHeight, context.gutterBoxHeight)
        guard tallest > context.rowHeight else { return 0 }
        return Int(ceil((tallest - context.rowHeight) / 2 / context.rowHeight))
    }

    // MARK: Incremental invalidation

    /// Repaint only the rows and columns a change to the inputs could have altered.
    ///
    /// `rows` is empty to mean "no row changed" and `columns` `nil` to mean "every column of those
    /// rows". A full-width repaint of a few rows is one `setNeedsDisplay` per row rect, which is
    /// what AppKit's dilated invalidation region then merges.
    func invalidate(rows: IndexSet, columns: ClosedRange<Int>?, geometry: GridColumnGeometry,
                    paint: GridPaintContext) {
        guard !rows.isEmpty else { needsDisplay = true; return }
        let overflow = overflowRows(paint)
        for row in rows {
            let top = max(0, CGFloat(row - overflow) * paint.rowHeight)
            let rect: NSRect
            if let columns {
                let first = geometry.edges(of: columns.lowerBound).left
                let last = geometry.edges(of: columns.upperBound).right
                rect = NSRect(x: first, y: top, width: last - first,
                              height: paint.rowHeight * CGFloat(overflow * 2 + 1))
            } else {
                rect = NSRect(x: 0, y: top, width: bounds.width,
                              height: paint.rowHeight * CGFloat(overflow * 2 + 1))
            }
            setNeedsDisplay(rect)
        }
    }

    /// Every rectangle asked to repaint, while a test is recording them. `nil` in the app, where a
    /// fling asks for a great many and nobody reads them; `needsToDraw(_:)` cannot stand in for this
    /// because a window that is not on screen answers `true` for every rectangle.
    var invalidationLog: [NSRect]?

    override func setNeedsDisplay(_ invalidRect: NSRect) {
        invalidationLog?.append(invalidRect)
        super.setNeedsDisplay(invalidRect)
    }

    /// Repaint one cell, which is all a ring that moved from one cell to another has to touch.
    func invalidate(cell: CellPos, geometry: GridColumnGeometry, paint: GridPaintContext) {
        guard cell.column >= 0, cell.column < geometry.widths.count else { return }
        invalidate(rows: IndexSet(integer: cell.row), columns: cell.column...cell.column,
                   geometry: geometry, paint: paint)
    }

    /// Repaint the difference between two selection rectangles, and nothing else.
    ///
    /// The rows in the symmetric difference are repainted whole; a row in both gets only the columns
    /// that entered or left. Without the column part, extending a selection one column to the right
    /// would repaint every row of the block from the left edge on every step of the drag.
    func invalidateSelection(previous: CellRange?, geometry: GridColumnGeometry, paint: GridPaintContext) {
        guard previous != selection else { return }
        var rows = IndexSet()
        var columns = IndexSet()
        if let previous { for position in previous.allPositions { rows.insert(position.row); columns.insert(position.column) } }
        if let selection { for position in selection.allPositions { rows.insert(position.row); columns.insert(position.column) } }
        guard let low = columns.min(), let high = columns.max() else { needsDisplay = true; return }
        invalidate(rows: rows, columns: low...high, geometry: geometry, paint: paint)
    }

    // MARK: Focus and the keyboard

    /// A click makes the table the first responder, which is what puts ⌘C on the responder chain.
    override var acceptsFirstResponder: Bool { true }

    override func becomeFirstResponder() -> Bool {
        isKeyboardFocused = true
        coordinator?.focusChanged()
        return true
    }

    override func resignFirstResponder() -> Bool {
        let resigned = super.resignFirstResponder()
        if resigned {
            isKeyboardFocused = false
            coordinator?.focusChanged()
        }
        return resigned
    }

    /// The grid's own copy. The responder chain sends ⌘C down to whatever implements `copy:`, and
    /// this is the first responder that does — `NSTableView` itself has no such method, so this is
    /// a new one rather than an override. It is only valid when there is a block to copy.
    @objc func copy(_ sender: Any?) {
        coordinator?.copy(withHeaders: false)
    }

    /// ⌘A selects every *row* of the table, which the grid does not draw and does not want: the
    /// accessibility tree would announce a selection nobody can see.
    @objc override func selectAll(_ sender: Any?) {}

    /// Space and ⌘Y: open or close the peek on the cursor's cell (View > Peek Cell).
    @objc func peekCell(_ sender: Any?) {
        coordinator?.togglePeek()
    }

    /// ⌘C is only offered when there is a block to copy, and ⌘A is always refused. Without this the
    /// menu items would be enabled on an empty selection and do nothing when chosen.
    override func validateUserInterfaceItem(_ item: NSValidatedUserInterfaceItem) -> Bool {
        if item.action == #selector(copy(_:)) { return selection != nil }
        if item.action == #selector(selectAll(_:)) { return false }
        if item.action == #selector(peekCell(_:)) { return cursor != nil }
        return super.validateUserInterfaceItem(item)
    }

    /// The grid's keyboard (blueprint W10 §3.1): the key is turned into a `GridKey`, `GridKeyMap`
    /// says what it means, and the coordinator does it.
    ///
    /// An arrow the map has no use for (⌥← and the like) is still swallowed, because
    /// letting it reach `NSTableView` would move the row selection that the grid never draws.
    /// Everything else the map leaves alone goes to AppKit.
    override func keyDown(with event: NSEvent) {
        if let key = GridKey(event: event) {
            if coordinator?.handleKey(key) == true { return }
            if key.isArrow { return }
        }
        super.keyDown(with: event)
    }

    /// The body's menu, from the coordinator. A right click must not move the selection, so nothing
    /// here touches it.
    override func menu(for event: NSEvent) -> NSMenu? {
        coordinator?.bodyMenu()
    }
}
