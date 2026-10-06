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

    /// The pointer settles the tooltip's cell whatever coordinate system AppKit used for `point`: a
    /// tooltip is for the pointer resting over the region, so where the pointer is answers it. This
    /// is what closes the open question about `point` (it documents neither), because the point is
    /// no longer what the answer rests on.
    func testTheTooltipFollowsThePointerWhateverPointAppKitHandsOver() throws {
        let fixture = makeGrid(rowCount: 400)
        fixture.apply()
        GridFixture.pause(for: 0.3)
        fixture.scroll.contentView.scroll(to: NSPoint(x: 0, y: 750))
        fixture.scroll.reflectScrolledClipView(fixture.scroll.contentView)
        GridFixture.pause(for: 0.3)

        let tag = fixture.table.addToolTip(fixture.table.visibleRect, owner: fixture.coordinator, userData: nil)
        fixture.coordinator.pointerOverride = { fixture.windowPoint(inCell: 33, column: 1) }
        // Three different `point`s — the cell in view coordinates, the cell in window coordinates,
        // and nonsense — all answer the cell the pointer is over.
        for point in [fixture.pointInCell(row: 33, column: 1), fixture.windowPoint(inCell: 33, column: 1),
                      NSPoint(x: 5_000, y: -300)] {
            XCTAssertEqual(fixture.coordinator.view(fixture.table, stringForToolTip: tag, point: point,
                                                    userData: nil), "33")
        }

        // A pointer outside the table (it left while the string was being asked for) says nothing,
        // and the point is read the old way.
        fixture.coordinator.pointerOverride = { NSPoint(x: -400, y: -400) }
        XCTAssertEqual(fixture.coordinator.view(fixture.table, stringForToolTip: tag,
                                                point: fixture.pointInCell(row: 33, column: 1), userData: nil),
                       "33")
        // No window on screen, no pointer: the tests above this one.
        fixture.coordinator.pointerOverride = { nil }
        XCTAssertEqual(fixture.coordinator.view(fixture.table, stringForToolTip: tag,
                                                point: fixture.windowPoint(inCell: 34, column: 1), userData: nil),
                       "34")
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

    // MARK: The cell cursor (W10-T1)

    private func keyEvent(_ keyCode: UInt16, _ flags: NSEvent.ModifierFlags = [],
                          in fixture: GridFixture) throws -> NSEvent {
        try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: 0,
                                       windowNumber: fixture.window.windowNumber, context: nil,
                                       characters: "", charactersIgnoringModifiers: "",
                                       isARepeat: false, keyCode: keyCode))
    }

    /// A real arrow-key event through `keyDown`: the cursor and the selection move together, and the
    /// table's own row selection does not (D-15 of Fase 5: it is never drawn, so it must never move).
    func testArrowKeysMoveTheCursorAndTheSelectionAndNotTheTablesRows() throws {
        let fixture = makeGrid()
        fixture.apply()
        fixture.coordinator.press(at: fixture.pointInCell(row: 3, column: 1), clickCount: 1)
        fixture.coordinator.release()

        fixture.table.keyDown(with: try keyEvent(125, [.numericPad, .function], in: fixture))  // down
        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: 4, column: 1))
        XCTAssertEqual(fixture.tab.cellSelection, CellRange(from: (4, 1), to: (4, 1)))

        fixture.table.keyDown(with: try keyEvent(124, [.numericPad, .function], in: fixture))  // right
        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: 4, column: 2))

        fixture.table.keyDown(with: try keyEvent(126, [.option], in: fixture))  // option-up: unmapped
        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: 4, column: 2), "swallowed, not applied")

        XCTAssertEqual(fixture.table.selectedRowIndexes, IndexSet(), "no row of the table was selected")
        XCTAssertEqual(fixture.table.selection, fixture.tab.cellSelection, "the table's copy follows")
        XCTAssertEqual(fixture.table.cursor, fixture.tab.cellCursor)
        XCTAssertGreaterThanOrEqual(fixture.log.selectionChanged, 3)
        XCTAssertGreaterThanOrEqual(fixture.log.settleSelection, 2, "the inspector reads the cursor's cell")
    }

    func testShiftArrowExtendsFromTheAnchorAndEscapeShrinksItBack() throws {
        let fixture = makeGrid()
        fixture.apply()
        fixture.coordinator.press(at: fixture.pointInCell(row: 3, column: 0), clickCount: 1)
        fixture.coordinator.release()

        fixture.table.keyDown(with: try keyEvent(125, [.shift], in: fixture))
        fixture.table.keyDown(with: try keyEvent(125, [.shift], in: fixture))
        fixture.table.keyDown(with: try keyEvent(124, [.shift], in: fixture))
        XCTAssertEqual(fixture.tab.cellCursor?.anchor, CellPos(row: 3, column: 0), "the anchor stays")
        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: 5, column: 1))
        XCTAssertEqual(fixture.tab.cellSelection, CellRange(from: (3, 0), to: (5, 1)))

        // Esc: the block shrinks to the cursor's cell, then the selection goes, the cursor stays.
        fixture.table.keyDown(with: try keyEvent(53, in: fixture))
        XCTAssertEqual(fixture.tab.cellSelection, CellRange(from: (5, 1), to: (5, 1)))
        fixture.table.keyDown(with: try keyEvent(53, in: fixture))
        XCTAssertNil(fixture.tab.cellSelection)
        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: 5, column: 1), "the next arrow starts here")
        XCTAssertNil(fixture.table.selection)
        fixture.table.keyDown(with: try keyEvent(125, in: fixture))
        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: 6, column: 1))
    }

    /// Nothing selected, nothing under the cursor: the first arrow lands on the first cell on screen
    /// instead of doing nothing.
    func testTheFirstArrowLandsOnTheFirstVisibleCell() throws {
        let fixture = makeGrid(rowCount: 400)
        fixture.apply()
        fixture.scroll.contentView.scroll(to: NSPoint(x: 0, y: 750))
        fixture.scroll.reflectScrolledClipView(fixture.scroll.contentView)
        XCTAssertNil(fixture.tab.cellCursor)

        // The first row that is wholly on screen: the header covers the top of the clip view.
        let clip = fixture.scroll.contentView
        let expected = Int(ceil((clip.bounds.minY + clip.contentInsets.top) / fixture.style.rowHeight))
        XCTAssertGreaterThan(expected, 0)

        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.down)))
        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: expected, column: 0))
        XCTAssertEqual(clip.bounds.minY, 750, "it was already in view, so nothing scrolled")
    }

    func testTheCursorIsScrolledIntoViewWithOneStepOfScroll() throws {
        let fixture = makeGrid(rowCount: 400)
        fixture.apply()
        fixture.coordinator.press(at: fixture.pointInCell(row: 0, column: 0), clickCount: 1)
        fixture.coordinator.release()

        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.down, command: true)))
        XCTAssertEqual(fixture.tab.cellCursor?.focus.row, 399)
        let visible = fixture.table.rows(in: fixture.table.visibleRect)
        XCTAssertTrue(NSLocationInRange(399, visible), "the last row is on screen: \(visible)")

        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.up, command: true)))
        let clip = fixture.scroll.contentView
        XCTAssertEqual(clip.bounds.minY, -clip.contentInsets.top, accuracy: 0.5,
                       "and back at the top, with the first row clear of the header")
    }

    func testAPageMovesByTheRowsOnScreenLessOne() throws {
        let fixture = makeGrid(rowCount: 400)
        fixture.apply()
        fixture.coordinator.press(at: fixture.pointInCell(row: 0, column: 0), clickCount: 1)
        fixture.coordinator.release()
        let page = fixture.coordinator.gridBounds.page
        XCTAssertGreaterThan(page, 10)

        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.pageDown)))
        XCTAssertEqual(fixture.tab.cellCursor?.focus.row, page - 1)
        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.pageDown, shift: true)))
        XCTAssertEqual(fixture.tab.cellSelection?.top, page - 1, "extending keeps the anchor")
    }

    /// Tab walks the cells, and past the last one the grid lets go of the keyboard.
    func testTabWalksTheCellsAndLeavesAtTheEnd() throws {
        let fixture = makeGrid(rowCount: 3)
        fixture.apply()
        fixture.coordinator.press(at: fixture.pointInCell(row: 0, column: 2), clickCount: 1)
        fixture.coordinator.release()

        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.tab)))
        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: 1, column: 0), "wrapped onto the next row")
        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.tab, shift: true)))
        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: 0, column: 2))

        fixture.coordinator.press(at: fixture.pointInCell(row: 2, column: 2), clickCount: 1)
        fixture.coordinator.release()
        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.tab)), "the key is taken: it hands the focus on")
        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: 2, column: 2), "and the cursor stays")
    }

    /// A grid with nothing to move to (no rows, or no drawn columns) is not a keyboard trap: the
    /// grid takes the keyboard, so Tab and Shift-Tab have to hand it on instead of being swallowed.
    func testTabLeavesAnEmptyGrid() throws {
        for (label, rowCount, sources) in [("no rows", 0, nil), ("no drawn columns", 3, [Int]())] as [(String, Int, [Int]?)] {
            let fixture = makeGrid(rowCount: rowCount)
            fixture.apply(fixture.inputs(visibleSources: sources))

            // A field either side of the grid, in the key-view loop, for the keyboard to land on.
            let container = NSView(frame: fixture.scroll.frame)
            let before = NSTextField(frame: NSRect(x: 0, y: 0, width: 80, height: 20))
            let after = NSTextField(frame: NSRect(x: 100, y: 0, width: 80, height: 20))
            container.addSubview(fixture.scroll)
            container.addSubview(before)
            container.addSubview(after)
            fixture.window.contentView = container
            before.nextKeyView = fixture.table
            fixture.table.nextKeyView = after
            after.nextKeyView = before

            XCTAssertTrue(fixture.window.makeFirstResponder(fixture.table), label)
            XCTAssertTrue(fixture.window.firstResponder === fixture.table, label)
            fixture.table.keyDown(with: try keyEvent(48, in: fixture))  // tab
            XCTAssertFalse(fixture.window.firstResponder === fixture.table, "\(label): Tab left the grid")
            XCTAssertTrue(after.currentEditor() != nil && fixture.window.firstResponder === after.currentEditor(),
                          "\(label): and landed on the next key view")

            XCTAssertTrue(fixture.window.makeFirstResponder(fixture.table), label)
            fixture.table.keyDown(with: try keyEvent(48, [.shift], in: fixture))  // shift-tab
            XCTAssertTrue(before.currentEditor() != nil && fixture.window.firstResponder === before.currentEditor(),
                          "\(label): Shift-Tab landed on the previous key view")

            // The other motions are still taken and still stop: there is nowhere for them to go.
            XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.down)), label)
            XCTAssertNil(fixture.tab.cellCursor, label)
        }
    }

    /// Return does what a double-click does on the cell: it opens the editor on a plain value.
    func testReturnOpensTheEditorOnTheCursorCell() throws {
        let fixture = makeGrid()
        fixture.apply()
        fixture.coordinator.press(at: fixture.pointInCell(row: 5, column: 0), clickCount: 1)
        fixture.coordinator.release()
        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.returnKey)))
        XCTAssertNotNil(fixture.coordinator.editor)
        XCTAssertEqual(fixture.log.beginEdit, [CellKey(row: 5, column: 0)])

        // The field now has the keyboard: the grid's map answers only Esc, which abandons the edit.
        XCTAssertFalse(fixture.coordinator.handleKey(GridKey(.down)))
        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.escape)))
        XCTAssertNil(fixture.coordinator.editor)
        XCTAssertEqual(fixture.log.cancelEdit, 1)
    }

    // MARK: The ring

    /// The ring is repainted where it was and where it is, and nowhere else.
    func testTheRingInvalidatesTheCellItLeftAndTheCellItReached() {
        let a = CellPos(row: 2, column: 1), b = CellPos(row: 7, column: 3)
        XCTAssertEqual(GridPaintDiff.cursorInvalidations(old: a, new: b), [a, b])
        XCTAssertEqual(GridPaintDiff.cursorInvalidations(old: nil, new: b), [b])
        XCTAssertEqual(GridPaintDiff.cursorInvalidations(old: a, new: nil), [a])
        XCTAssertEqual(GridPaintDiff.cursorInvalidations(old: a, new: a), [], "a cursor that did not move")
        XCTAssertEqual(GridPaintDiff.cursorInvalidations(old: nil, new: nil), [])
    }

    private func cellRect(_ fixture: GridFixture, row: Int, column: Int) -> NSRect {
        let edges = fixture.coordinator.geometry.edges(of: column)
        let rowRect = fixture.table.rect(ofRow: row)
        return NSRect(x: edges.left, y: rowRect.minY, width: edges.right - edges.left, height: rowRect.height)
    }

    func testAKeyMoveRepaintsTheTwoCellsAndNotARowFarAway() throws {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply()
        fixture.coordinator.press(at: fixture.pointInCell(row: 3, column: 1), clickCount: 1)
        fixture.coordinator.release()

        fixture.table.invalidationLog = []
        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.down)))
        let log = try XCTUnwrap(fixture.table.invalidationLog)
        XCTAssertEqual(Set(log), [cellRect(fixture, row: 3, column: 1), cellRect(fixture, row: 4, column: 1)],
                       "the cell the ring left and the cell it reached, and no other")
    }

    /// Gaining or losing the keyboard repaints the cursor's cell only (it goes between 40% and full).
    func testFocusChangedRepaintsOnlyTheCursorCell() throws {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply()
        fixture.coordinator.press(at: fixture.pointInCell(row: 3, column: 1), clickCount: 1)
        fixture.coordinator.release()

        let ring = cellRect(fixture, row: 3, column: 1)
        fixture.table.invalidationLog = []
        fixture.coordinator.focusChanged()
        XCTAssertEqual(fixture.table.invalidationLog, [ring])

        // And through the responder chain, which is what a click on the grid does.
        fixture.table.invalidationLog = []
        XCTAssertTrue(fixture.window.makeFirstResponder(fixture.table))
        XCTAssertTrue(fixture.table.isKeyboardFocused)
        XCTAssertEqual(fixture.table.invalidationLog, [ring])
        XCTAssertEqual(fixture.table.cursorStrength, 0.4, "this window is not key, so the ring stays dim")

        fixture.table.invalidationLog = []
        XCTAssertTrue(fixture.window.makeFirstResponder(nil))
        XCTAssertFalse(fixture.table.isKeyboardFocused)
        XCTAssertEqual(fixture.table.invalidationLog, [ring], "losing the keyboard repaints it too")
    }

    /// The ring draws on the cursor's row and nowhere else, and it is a stroke inside the cell: the
    /// same row without a cursor is the same pixels except for it.
    func testTheRingIsDrawnInsideTheCursorCellOnly() throws {
        let fixture = makeGrid(rowCount: 40)
        fixture.apply()
        let rowRect = fixture.table.rect(ofRow: 5).integral
        let otherRow = fixture.table.rect(ofRow: 8).integral

        let before = try raster(rowRect, in: fixture.table)
        let beforeOther = try raster(otherRow, in: fixture.table)
        fixture.coordinator.press(at: fixture.pointInCell(row: 5, column: 1), clickCount: 1)
        fixture.coordinator.release()
        fixture.tab.cellSelection = nil  // the wash is not what is being looked at
        fixture.tab.cellCursor = GridCursor(anchor: CellPos(row: 5, column: 1), focus: CellPos(row: 5, column: 1))
        fixture.table.selection = nil
        fixture.table.cursor = fixture.tab.cellCursor

        let after = try raster(rowRect, in: fixture.table)
        XCTAssertNotEqual(before, after, "a ring was drawn on the cursor's row")
        XCTAssertEqual(beforeOther, try raster(otherRow, in: fixture.table), "and on no other")

        fixture.table.cursor = nil
        XCTAssertEqual(before, try raster(rowRect, in: fixture.table), "taking the cursor away takes the ring away")
    }

    // MARK: The ring's colour

    /// Four of the five accents are under 3:1 on a light canvas, so the ring picks whichever half of
    /// the accent stands out from the canvas, and falls back to the ink (W9 D-8's rule).
    func testTheRingTakesTheAccentHalfThatStandsOutFromTheCanvas() {
        let white = NSColor(srgbRed: 1, green: 1, blue: 1, alpha: 1)
        let near = NSColor(srgbRed: 0.05, green: 0.06, blue: 0.09, alpha: 1)
        let ice = NSColor(hex: 0x4FD8FF), violet = NSColor(hex: 0x7B61FF)
        let ink = NSColor.black

        XCTAssertLessThan(GridContrast.ratio(ice, white), 3, "the glow is no ring on white")
        XCTAssertEqual(GridContrast.ringColour(candidates: [ice, violet], canvas: white, fallback: ink), violet)
        XCTAssertEqual(GridContrast.ringColour(candidates: [ice, violet], canvas: near, fallback: ink), ice,
                       "on a dark canvas the glow wins")
        let pale = NSColor(hex: 0xEEEEEE)
        XCTAssertEqual(GridContrast.ringColour(candidates: [pale, pale], canvas: white, fallback: ink), ink,
                       "when neither half reaches 3:1, the ink")
        XCTAssertEqual(GridContrast.ratio(NSColor.black, white), 21, accuracy: 0.01)
    }

    func testThePaletteRingIsAlwaysAtLeastThreeToOneAgainstTheCanvas() {
        for dark in [false, true] {
            let appearance = NSAppearance(named: dark ? .darkAqua : .aqua)!
            let palette = GridPalette.resolve(appearance)
            var canvas = NSColor.clear
            appearance.performAsCurrentDrawingAppearance { canvas = NSColor(Tone.canvas) }
            XCTAssertGreaterThanOrEqual(GridContrast.ratio(NSColor(cgColor: palette.cursor) ?? .clear, canvas), 3,
                                        "dark: \(dark)")
        }
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
