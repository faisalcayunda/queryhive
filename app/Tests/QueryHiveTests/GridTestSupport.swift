import AppKit
import XCTest

@testable import QueryHive

/// What the `GridCommands` closures recorded.
///
/// The commands are how the table, its header and its menus reach the model, so a test that only
/// looked at the model could pass while the wire behind it was cut — which is what a closure left
/// as a stub looks like. Every closure here writes to its own counter instead.
final class GridCommandLog {
    var selectionChanged = 0
    var settleSelection = 0
    var beginEdit: [CellKey] = []
    var commitEdit: [(CellKey, String)] = []
    var cancelEdit = 0
    var sortClick: [Int] = []
    var openFilter: [(Int, CGRect)] = []
    var rename: [Int] = []
    var copy: [Bool] = []
    var viewValue = 0
    var review = 0
    var paste = 0
}

/// The grid, wired the way `ResultGridTable.makeNSView` wires it, without SwiftUI.
///
/// The blueprint asks for this (§18: "semuanya membangun `GridTableView` di jendela luar layar dan
/// memanggil metode `Coordinator`, tanpa `NSEvent`"), and it is what makes the pointer, the
/// tooltip, the editor overlay and the accessibility tree testable at all: none of them is a
/// SwiftUI view, and all four are reachable through the coordinator.
///
/// The panel goes into a real, off-screen `NSWindow` because two of the things under test are
/// expressed in **window** coordinates — AppKit hands an owner method a point that way, and
/// `convert(_, to: nil)` needs a window to convert against.
@MainActor
final class GridFixture {

    let tab: QueryTab
    let model: AppModel
    let log: GridCommandLog
    let commands: GridCommands
    let coordinator: ResultGridTable.Coordinator
    let table: GridTableView
    let scroll: NSScrollView
    let window: NSWindow
    let style: GridInputs.GridStyle
    /// The columns, in source order, so a test can name one without counting.
    let columns: [Event.Column]
    /// §10's formula widths, before the panel has had its say.
    let naturalWidths: [CGFloat]
    let rowCount: Int

    init(columns: [Event.Column],
         rows: [[String?]],
         viewport: CGSize = CGSize(width: 900, height: 600),
         style: GridInputs.GridStyle = .placeholder) {
        let log = GridCommandLog()
        let commands = GridCommands(
            sortClick: { log.sortClick.append($0) },
            openFilter: { log.openFilter.append(($0, $1)) },
            rename: { log.rename.append($0) },
            review: { log.review += 1 },
            copy: { log.copy.append($0) },
            viewValue: { log.viewValue += 1 },
            beginEdit: { log.beginEdit.append($0) },
            commitEdit: { log.commitEdit.append(($0, $1)) },
            cancelEdit: { log.cancelEdit += 1 },
            settleSelection: { log.settleSelection += 1 },
            paste: { log.paste += 1 },
            selectionChanged: { log.selectionChanged += 1 })

        let tab = QueryTab(title: "Grid")
        tab.preview = PreviewResult(columns: columns, rows: rows, truncated: false,
                                    queryID: nil, elapsedMS: 0)
        // The same two numbers `ResultGrid` feeds §10: the header label, and the widest of the
        // first 200 *fetched* rows with NULL counting 4.
        let sampled = ArrayRows(rows: rows, sizing: rows, columns: columns).naturalCharCounts()

        self.tab = tab
        self.model = AppModel()
        self.log = log
        self.commands = commands
        self.style = style
        self.columns = columns
        self.naturalWidths = GridMetrics.naturalWidths(headerCounts: columns.map(\.name.count),
                                                        sampleCounts: sampled)
        self.rowCount = rows.count

        let table = GridTableView()
        let coordinator = ResultGridTable.Coordinator(tab: tab, model: model, commands: commands)
        self.coordinator = coordinator
        self.table = table
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
        self.scroll = scroll

        let window = NSWindow(contentRect: CGRect(origin: .zero, size: viewport),
                              styleMask: [.titled], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.contentView = scroll
        self.window = window

        coordinator.install(in: scroll)
    }

    /// The inputs `ResultGrid.body` would build, with the fields a test wants to move settable.
    ///
    /// `edits` defaults to the tab's own queue rather than to a fresh one, so staging an edit the
    /// way the app does (`tab.beginCellEdit`) shows up in what gets applied.
    func inputs(revision: Int = 1,
                selection: CellRange? = nil,
                sort: SortIndicator? = nil,
                filtered: Set<Int> = [],
                visibleSources: [Int]? = nil,
                widths: [CGFloat]? = nil) -> GridInputs {
        let sources = visibleSources ?? columns.indices.map { $0 }
        return GridInputs(revision: revision,
                   layout: GridInputs.GridColumnLayout(
                       // A moved or hidden grid has fewer drawn columns than the result has
                       // columns, and `refitGeometry` reads this list as the geometry's widths:
                       // handing it the full set would give the table four columns to draw where
                       // two are shown.
                       columnWidths: widths ?? sources.map { naturalWidths[$0] },
                       visibleSources: sources),
                   selection: selection,
                   edits: tab.cellEdits,
                   sort: sort,
                   filtered: filtered,
                   style: style,
                   filterPopover: nil,
                   viewing: nil)
    }

    /// Lay the panel out and apply once.
    ///
    /// Order matters and is the order the app sees: the clip view needs its bounds before
    /// `refitGeometry` can read `viewportWidth` (§10 step 2), and the document needs a height
    /// before `visibleRowRange` can say which rows are on screen.
    func apply(_ inputs: GridInputs? = nil, force: Bool = true) {
        scroll.layoutSubtreeIfNeeded()
        coordinator.apply(inputs ?? self.inputs(), force: force)
        sizeDocument()
        scroll.layoutSubtreeIfNeeded()
        table.layout()
        table.needsDisplay = true
    }

    /// The document's own size: `rows × rowHeight`, which is what AppKit gives the table once it
    /// is laid out inside a scroll view, and the width of the document, which scrolls.
    private func sizeDocument() {
        let height = CGFloat(rowCount) * style.rowHeight
        let width = max(coordinator.geometry.totalWidth, scroll.contentView.bounds.width)
        table.setFrameSize(NSSize(width: width, height: height))
    }

    /// A point in the middle of one drawn cell, in the **table's** coordinates.
    ///
    /// Taken from the coordinator's own geometry rather than from a constant, so a test that
    /// presses cell (2, 1) is pressing cell (2, 1) whatever the panel ends up fitting.
    func pointInCell(row: Int, column: Int) -> CGPoint {
        let edges = coordinator.geometry.edges(of: column)
        let rowRect = table.rect(ofRow: row)
        return CGPoint(x: (edges.left + edges.right) / 2, y: (rowRect.minY + rowRect.maxY) / 2)
    }

    /// The same point in window coordinates — what a callback that AppKit feeds window coordinates
    /// would be handed.
    func windowPoint(inCell row: Int, column: Int) -> CGPoint {
        table.convert(pointInCell(row: row, column: column), to: nil)
    }

    /// AppKit's global application, which `announceSelectionForAX` posts through.
    ///
    /// A test process never launches the app, so the object behind `NSApp` has to exist before
    /// anything posts a notification at it.
    static func ensureApplication() {
        if NSApp == nil { _ = NSApplication.shared }
    }

    /// Spin the main run loop until `condition` holds, or for `timeout`.
    ///
    /// Two things under test arrive a tick after the call that asked for them: a
    /// `NotificationCenter` observer registered with `queue: .main`, and the grid's 100 ms
    /// coalesced tooltip rebuild. Both are delivered by the run loop, and running it is what the
    /// process would do anyway while a person waits for a tooltip to appear.
    static func drain(timeout: TimeInterval = 2, until condition: () -> Bool) {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition(), Date() < deadline {
            _ = RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.05))
        }
    }

    /// Spin the main run loop for a fixed time, for the work whose arrival cannot be watched.
    static func pause(for seconds: TimeInterval) {
        let deadline = Date().addingTimeInterval(seconds)
        while Date() < deadline {
            _ = RunLoop.current.run(mode: .default, before: Date().addingTimeInterval(0.05))
        }
    }
}
