import AppKit
import XCTest

@testable import QueryHive

/// The behaviours §13 of the blueprint says a person can do to a grid and that a pixel or a
/// pasteboard string has to prove: the pointer, the editor overlay, the tooltip, the header's
/// strings, the repaint, and what a new result does to the panel (blueprint §18).
///
/// Everything here drives the coordinator's own methods — no `NSEvent` is manufactured, because
/// the seam that matters is the one `GridTableView.mouseDown` forwards to.
@MainActor
final class GridParityTests: XCTestCase {

    private func makeGrid(rowCount: Int = 40) -> GridFixture {
        let columns = [Event.Column(name: "kode", type: "text"),
                       Event.Column(name: "jumlah", type: "bigint"),
                       Event.Column(name: "catatan", type: "text")]
        let long = String(repeating: "w", count: 9_000)
        let rows = (0..<rowCount).map { row -> [String?] in
            if row == 0 { return ["K-0", "7", nil] }               // a NULL
            if row == 1 { return ["K-1", "8", ""] }                // an empty string
            if row == 2 { return ["K-2", "9", long] }              // a value over the tooltip cap
            return ["K-\(row)", "\(row)", "catatan \(row)"]
        }
        return GridFixture(columns: columns, rows: rows)
    }

    // MARK: The pointer

    /// A press selects the cell, a drag extends it, and a pointer past any edge stops at that edge
    /// instead of falling off the result (§13 item 1).
    func testPressAndDragSelectABlockAndClampAtTheEdges() throws {
        let fixture = makeGrid()
        fixture.apply()

        fixture.coordinator.press(at: fixture.pointInCell(row: 1, column: 1), clickCount: 1)
        let single = try XCTUnwrap(fixture.tab.cellSelection)
        XCTAssertEqual(single.top, 1)
        XCTAssertEqual(single.bottom, 1)
        XCTAssertEqual(single.left, 1)
        XCTAssertEqual(single.right, 1)
        XCTAssertEqual(fixture.log.selectionChanged, 1)

        fixture.coordinator.drag(to: fixture.pointInCell(row: 3, column: 2))
        let block = try XCTUnwrap(fixture.tab.cellSelection)
        XCTAssertEqual(block.top, 1)
        XCTAssertEqual(block.bottom, 3)
        XCTAssertEqual(block.left, 1)
        XCTAssertEqual(block.right, 2)
        XCTAssertEqual(block.cellCount, 6)

        // Past the right edge and the last row: clamped, not dropped.
        fixture.coordinator.drag(to: CGPoint(x: 100_000, y: 100_000))
        let clamped = try XCTUnwrap(fixture.tab.cellSelection)
        XCTAssertEqual(clamped.right, 2, "the result draws three columns, and stops there")
        XCTAssertEqual(clamped.bottom, 39, "and forty rows")

        // Into the gutter and above the first row: clamped to the edge of the result.
        fixture.coordinator.drag(to: CGPoint(x: -50, y: -50))
        let back = try XCTUnwrap(fixture.tab.cellSelection)
        XCTAssertEqual(back.left, 0)
        XCTAssertEqual(back.top, 0)
        XCTAssertEqual(fixture.tab.cellCursor?.anchor, CellPos(row: 1, column: 1),
                       "the anchor a drag started from never moves")

        fixture.coordinator.release()
        XCTAssertEqual(fixture.log.settleSelection, 1)
        XCTAssertEqual(fixture.log.selectionChanged, 1, "a drag does not re-run the change command")
    }

    /// The tooltip is the whole formatted value (§8.5), and a NULL or an empty string has none —
    /// which with one region over the whole body has to be expressed by the answer, not by leaving
    /// a cell unregistered.
    func testTheTooltipIsTheFullFormattedValueAndAbsentForNullAndEmpty() throws {
        let fixture = makeGrid()
        fixture.apply()
        // The region is registered by the debounced rebuild, 100 ms after the layout that asked
        // for it; letting it run is part of what is being tested (§8.5).
        GridFixture.pause(for: 0.3)

        let region = fixture.table.visibleRect
        let tag = fixture.table.addToolTip(region, owner: fixture.coordinator, userData: nil)

        func tip(_ row: Int, _ column: Int) -> String {
            // AppKit documents neither the coordinate system of `point` nor a conversion helper,
            // and `GridToolTip.local` settles it from the registered rectangle. In view
            // coordinates — the one the rectangle was registered in — the answer is the cell.
            fixture.coordinator.view(fixture.table, stringForToolTip: tag,
                                     point: fixture.pointInCell(row: row, column: column),
                                     userData: nil)
        }

        XCTAssertEqual(tip(0, 0), "K-0", "the value in the column's display format")
        XCTAssertEqual(tip(3, 1), "3")
        XCTAssertEqual(tip(0, 2), "", "a NULL has no tooltip")
        XCTAssertEqual(tip(1, 2), "", "and neither does an empty string")

        // Over the gutter: no column, so no tooltip — `clampedColumn` must not answer the first.
        let gutter = CGPoint(x: 4, y: fixture.table.rect(ofRow: 3).midY)
        XCTAssertEqual(fixture.coordinator.view(fixture.table, stringForToolTip: tag,
                                                point: gutter, userData: nil), "")

        // A megabyte of JSON does not become a megabyte of tooltip: capped at 8192 UTF-16 units,
        // cut by grapheme so the last character is never half a surrogate pair.
        let overCap = tip(2, 2)
        XCTAssertEqual(overCap.utf16.count, GridToolTip.limit)
        XCTAssertTrue(overCap.hasSuffix("…"))
    }

    /// AppKit documents neither the coordinate system of `point` nor a conversion helper, so
    /// `GridToolTip.local` settles it from the rectangle registered with AppKit: whichever reading
    /// lands inside the region actually on screen is where the pointer is. The two readings only
    /// differ once the grid has scrolled — there the answer must not depend on which one arrived.
    func testTheTooltipResolvesBothReadingsOfThePointerOnAScrolledGrid() throws {
        let fixture = makeGrid(rowCount: 400)
        fixture.apply()
        GridFixture.pause(for: 0.3)

        fixture.scroll.contentView.scroll(to: NSPoint(x: 0, y: 750))
        fixture.scroll.reflectScrolledClipView(fixture.scroll.contentView)
        // The region is rebuilt 100 ms after the scroll stops; a window-space point resolved
        // before that has no rectangle to be settled against.
        GridFixture.pause(for: 0.3)

        let tag = fixture.table.addToolTip(fixture.table.visibleRect, owner: fixture.coordinator,
                                           userData: nil)
        func answer(_ point: NSPoint) -> String {
            fixture.coordinator.view(fixture.table, stringForToolTip: tag, point: point,
                                     userData: nil)
        }

        // Row 33 sits at view y 825…850 and at window y 75…100: only one of those readings is the
        // cell, and it has to be the answer either way.
        XCTAssertEqual(answer(fixture.windowPoint(inCell: 33, column: 1)), "33",
                       "the window-space reading resolves to the cell on screen")
        XCTAssertEqual(answer(fixture.pointInCell(row: 33, column: 1)), "33",
                       "and so does the view-space one")
    }

    /// The header's two strings, taken from the grid that shipped rather than from the blueprint's
    /// §12.4 phrasing: a header click routes **server-first** (`AppModel.setSort`), so the wording
    /// about rows already fetched would describe only the fallback path (§13 item 25).
    func testTheHeaderTooltipsAreTheSameStrings() throws {
        let fixture = makeGrid()
        fixture.apply()
        let header = fixture.table.header
        header.scheduleTooltips()
        let tag = header.addToolTip(header.bounds, owner: header, userData: nil)

        let edges = fixture.coordinator.geometry.edges(of: 0)
        let width = edges.right - edges.left
        // The label sits inside the column's content, away from the funnel's own band.
        let label = header.convert(CGPoint(x: edges.left + GridMetrics.cellPadding + (width - 2 * GridMetrics.cellPadding) / 2,
                                           y: header.bounds.midY),
                                   to: nil)
        XCTAssertEqual(header.view(header, stringForToolTip: tag, point: label, userData: nil),
                       "Sort by kode — on the server when possible, over the rows fetched otherwise")

        // The funnel is 10 × 10 in the top-right of the content rect (§12.3).
        let funnel = header.convert(CGPoint(x: edges.right - 18 + 5, y: 4 + 5), to: nil)
        XCTAssertEqual(header.view(header, stringForToolTip: tag, point: funnel, userData: nil),
                       "Filter this column")

        // …and once a filter is on, the same spot says which one.
        fixture.tab.columnFilters[1] = .text("19")
        fixture.apply(fixture.inputs(filtered: [1]))
        let filteredEdges = fixture.coordinator.geometry.edges(of: 1)
        let filteredFunnel = header.convert(CGPoint(x: filteredEdges.right - 18 + 5, y: 4 + 5),
                                            to: nil)
        XCTAssertEqual(header.view(header, stringForToolTip: tag, point: filteredFunnel,
                                   userData: nil),
                       "Filtered by \(fixture.tab.columnFilters[1]?.label ?? "")")
    }

    // MARK: The editor overlay

    /// Typing a cell's whole value is **one** undo step: a queue written on every keystroke would
    /// turn a typo into a production `UPDATE` (§13 item 4).
    func testTypingInTheOverlayIsOneUndoStep() throws {
        let fixture = makeGrid()
        fixture.apply()
        let key = CellKey(row: 5, column: 0)

        fixture.coordinator.beginEdit(at: key)
        let field = try XCTUnwrap(fixture.coordinator.editor, "the overlay is a field over the cell")
        XCTAssertEqual(field.superview, fixture.table)
        XCTAssertEqual(field.stringValue, "K-5", "seeded with the value the cell holds")
        XCTAssertEqual(fixture.log.beginEdit, [key])

        field.stringValue = "K-5 edited"
        fixture.coordinator.commitEdit(at: key, text: field.stringValue)

        XCTAssertNil(fixture.coordinator.editor, "the overlay goes away with the session")
        XCTAssertTrue(fixture.table.subviews.filter { $0 is NSTextField }.isEmpty)
        XCTAssertEqual(fixture.tab.cellEdits.value(at: key), "K-5 edited")
        XCTAssertEqual(fixture.log.commitEdit.count, 1)

        fixture.tab.undoCellEdit()
        XCTAssertNil(fixture.tab.cellEdits.value(at: key),
                     "one undo takes the whole session back, not the last keystroke")
    }

    /// Return over a block fills every cell of it and stages the block as one step (§13 item 4).
    func testReturnOverABlockFillsIt() throws {
        let fixture = makeGrid()
        fixture.apply()

        fixture.coordinator.press(at: fixture.pointInCell(row: 1, column: 1), clickCount: 1)
        fixture.coordinator.drag(to: fixture.pointInCell(row: 3, column: 2))
        let selection = try XCTUnwrap(fixture.tab.cellSelection)
        XCTAssertEqual(selection.cellCount, 6)

        let key = CellKey(row: 1, column: 1)
        fixture.coordinator.beginEdit(at: key)
        fixture.coordinator.commitEdit(at: key, text: "filled")

        XCTAssertEqual(fixture.tab.cellEdits.count, 6)
        for row in 1...3 {
            for source in 1...2 {
                XCTAssertEqual(fixture.tab.cellEdits.value(at: CellKey(row: row, column: source)),
                               "filled")
            }
        }
        XCTAssertEqual(fixture.log.commitEdit.count, 1)

        // The block is one undo step, not six.
        fixture.tab.undoCellEdit()
        XCTAssertTrue(fixture.tab.cellEdits.isEmpty)
    }

    /// The block fill replaces the editing session, so a stray keystroke afterwards must not be
    /// staged into the cell the closed editor was over.
    func testBlockFillLeavesNoEditSessionOpen() throws {
        let fixture = makeGrid()
        fixture.apply()
        fixture.coordinator.press(at: fixture.pointInCell(row: 1, column: 1), clickCount: 1)
        fixture.coordinator.drag(to: fixture.pointInCell(row: 3, column: 2))
        let key = CellKey(row: 1, column: 1)
        fixture.coordinator.beginEdit(at: key)
        fixture.coordinator.commitEdit(at: key, text: "filled")

        fixture.tab.typeCellEdit("stale")
        fixture.tab.endCellEdit()
        XCTAssertEqual(fixture.tab.cellEdits.value(at: key), "filled")
    }

    /// Escape ends the overlay and leaves the queue exactly as it was: what was already staged
    /// stays staged, and nothing new appears (§13 item 4).
    func testEscLeavesTheQueueUntouched() throws {
        let fixture = makeGrid()
        fixture.apply()

        let staged = CellKey(row: 0, column: 0)
        fixture.coordinator.beginEdit(at: staged)
        fixture.coordinator.commitEdit(at: staged, text: "already staged")
        XCTAssertEqual(fixture.tab.cellEdits.count, 1)

        fixture.coordinator.beginEdit(at: CellKey(row: 4, column: 2))
        XCTAssertNotNil(fixture.coordinator.editor)

        fixture.coordinator.cancelEdit()
        XCTAssertNil(fixture.coordinator.editor)
        XCTAssertEqual(fixture.log.cancelEdit, 1)
        XCTAssertEqual(fixture.tab.cellEdits.count, 1, "Escape added nothing and dropped nothing")
        XCTAssertEqual(fixture.tab.cellEdits.value(at: staged), "already staged")
    }

    // MARK: Painting

    /// The painter lays the palette down once; a partial invalidation repaints a rectangle that
    /// overlaps one already painted, so the second coat has to be the same pixels as the first or
    /// the edges double (§18, and the comment on `GridTableView.invalidate`).
    func testRepaintingARowTwiceGivesTheSamePixels() throws {
        let fixture = makeGrid()
        fixture.apply()

        let visible = fixture.table.visibleRect
        let row = fixture.table.rect(ofRow: 5).intersection(visible).integral
        XCTAssertFalse(row.isEmpty, "row 5 is on screen in a 600 pt panel")

        let first = try raster(row, in: fixture.table)
        XCTAssertFalse(first.allSatisfy { $0 == 0 }, "nothing was drawn, so this proves nothing")
        let second = try raster(row, in: fixture.table)
        XCTAssertEqual(first, second, "the same rectangle painted twice must be identical")
    }

    private func raster(_ rect: NSRect, in table: GridTableView) throws -> Data {
        let rep = try XCTUnwrap(NSBitmapImageRep(
            bitmapDataPlanes: nil, pixelsWide: Int(rect.width), pixelsHigh: Int(rect.height),
            bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
            colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0))
        rep.size = NSSize(width: rect.width, height: rect.height)
        table.cacheDisplay(in: rect, to: rep)
        let bytes = try XCTUnwrap(rep.bitmapData)
        return Data(bytes: bytes, count: rep.bytesPerRow * rep.pixelsHigh)
    }

    // MARK: A column's format

    /// The format menu writes to `UserDefaults`, which is not observable, so the grid re-reads its
    /// formats on `ColumnFormatStore.didChange` and repaints (D-5). Without that, a format change
    /// would draw old characters until something else happened to move (§13 item 13).
    func testAFormatChangeRepaintsTheColumnThroughTheNotification() throws {
        let fixture = makeGrid()
        fixture.apply()

        let defaults = UserDefaults.standard
        let key = ColumnFormatStore.defaultsKey
        let saved = defaults.dictionary(forKey: key)
        defer {
            if let saved { defaults.set(saved, forKey: key) }
            else { defaults.removeObject(forKey: key) }
        }

        // Only a path that wrote the SQL itself knows where a column came from; the fixture
        // states the two halves so the identity can be filed.
        fixture.tab.connectionID = UUID()
        fixture.tab.sourceTable = "penerima"
        let identity = try XCTUnwrap(ColumnFormatStore.identity(connection: fixture.tab.connectionID,
                                                                table: "penerima", column: "kode"))
        XCTAssertEqual(fixture.coordinator.formats[0], .raw)

        fixture.table.needsDisplay = false
        ColumnFormatStore.set(.uuid, for: identity)
        GridFixture.drain(until: { fixture.coordinator.formats[0] == .uuid })

        XCTAssertEqual(fixture.coordinator.formats[0], .uuid,
                       "the grid read its formats again on the notification")
        XCTAssertTrue(fixture.table.needsDisplay, "and repainted with them")
    }

    // MARK: A new result

    /// Replacing the result under the same shape keeps where the user was reading: the offset is
    /// the panel's, not the rows' (§13 item 18).
    func testReplacingTheResultKeepsTheScrollOffset() throws {
        let fixture = makeGrid(rowCount: 400)
        fixture.apply()

        fixture.scroll.contentView.scroll(to: NSPoint(x: 0, y: 750))
        fixture.scroll.reflectScrolledClipView(fixture.scroll.contentView)
        let before = fixture.scroll.contentView.bounds.origin.y
        XCTAssertGreaterThan(before, 0, "the fixture scrolled, or this asserts nothing")

        fixture.apply(fixture.inputs(revision: 2))
        XCTAssertEqual(fixture.scroll.contentView.bounds.origin.y, before)
    }

    // MARK: Copy

    /// A copy is built from the **source** columns the block covers, sorted into source order and
    /// under the server's own names: hiding and reordering are rendering only, and a reordered grid
    /// must not copy a source range that runs through a column the user never selected
    /// (§13 item 3).
    func testCopyIsBuiltFromSourceColumnsInSourceOrder() throws {
        let fixture = makeGrid()
        // Source 2 drawn first, source 1 hidden: the display order and the source order disagree.
        fixture.apply(fixture.inputs(visibleSources: [2, 0]))

        fixture.coordinator.press(at: fixture.pointInCell(row: 3, column: 0), clickCount: 1)
        fixture.coordinator.drag(to: fixture.pointInCell(row: 4, column: 1))
        XCTAssertEqual(fixture.tab.cellSelection?.left, 0)
        XCTAssertEqual(fixture.tab.cellSelection?.right, 1)

        // The menu's command is logged; the coordinator is what writes the pasteboard.
        fixture.commands.copy(true)
        XCTAssertEqual(fixture.log.copy, [true])
        fixture.coordinator.copy(withHeaders: true)

        let text = try XCTUnwrap(NSPasteboard.general.string(forType: .string))
        // Display 0…1 covers sources 2 and 0, sorted to 0 and 2: `kode` then `catatan`.
        XCTAssertEqual(text, "kode\tcatatan\nK-3\tcatatan 3\nK-4\tcatatan 4")
    }
}
