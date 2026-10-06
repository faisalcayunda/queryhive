import AppKit
import XCTest

@testable import QueryHive

/// The peek (blueprint W10 §3.3): what it keeps of a long value, where it sits, and how it follows
/// the cursor — and that it never writes a file (NFR-S5).
@MainActor
final class CellPeekPanelTests: XCTestCase {

    // MARK: The cut

    func testAShortValueIsShownWholeWithoutANote() {
        let clipped = CellPeekLayout.clip("hello")
        XCTAssertEqual(clipped, PeekText(text: "hello", note: nil))
        let edge = String(repeating: "a", count: CellPeekPanel.maxBytes)
        XCTAssertEqual(CellPeekLayout.clip(edge).text, edge, "exactly 64 KiB still fits")
        XCTAssertNil(CellPeekLayout.clip(edge).note)
    }

    func testALongValueIsCutAt64KiBAndTheNoteSaysHowMuchThereWas() {
        let tenMiB = String(repeating: "x", count: 10 * 1_024 * 1_024)
        let clipped = CellPeekLayout.clip(tenMiB)
        XCTAssertEqual(clipped.text.utf8.count, 65_536)
        let note = clipped.note ?? ""
        XCTAssertTrue(note.hasPrefix("Showing the first 64 KiB of 10 MiB."), note)
        XCTAssertTrue(note.contains("copies the whole value"), "the grid's own copy is the way to all of it")
    }

    /// 65,536 is not a multiple of three, so the cut falls inside a euro sign: the last character
    /// that is shown has to be a whole one.
    func testTheCutNeverSplitsACharacter() {
        let euros = String(repeating: "€", count: 30_000)  // 90,000 bytes
        let clipped = CellPeekLayout.clip(euros)
        XCTAssertFalse(clipped.text.contains("\u{FFFD}"), "no replacement character")
        XCTAssertTrue(clipped.text.allSatisfy { $0 == "€" })
        XCTAssertLessThanOrEqual(clipped.text.utf8.count, 65_536)
        XCTAssertGreaterThan(clipped.text.utf8.count, 65_536 - 3)
    }

    func testSizesReadInTheLargestUnitThatFits() {
        XCTAssertEqual(CellPeekLayout.size(512), "512 bytes")
        XCTAssertEqual(CellPeekLayout.size(65_536), "64 KiB")
        XCTAssertEqual(CellPeekLayout.size(1_536), "1.5 KiB")
        XCTAssertEqual(CellPeekLayout.size(10 * 1_024 * 1_024), "10 MiB")
        XCTAssertEqual(CellPeekLayout.size(3 * 1_024 * 1_024 * 1_024), "3 GiB")
    }

    // MARK: Where it sits

    private let screen = CGRect(x: 0, y: 0, width: 1_440, height: 900)
    private let panel = CGSize(width: 592, height: 300)

    func testThePanelSitsFourPointsUnderTheCell() {
        let cell = CGRect(x: 200, y: 500, width: 120, height: 25)
        let frame = CellPeekLayout.frame(size: panel, cell: cell, visible: screen)
        XCTAssertEqual(frame.minX, 200)
        XCTAssertEqual(frame.maxY, 500 - 4, "its top edge is 4 pt under the cell's bottom")
        XCTAssertEqual(frame.size, panel)
    }

    /// Screen coordinates run up, so "below" is a smaller y; with no room there the panel flips
    /// above the cell.
    func testThePanelFlipsAboveWhenItDoesNotFitBelow() {
        let cell = CGRect(x: 200, y: 120, width: 120, height: 25)
        let frame = CellPeekLayout.frame(size: panel, cell: cell, visible: screen)
        XCTAssertEqual(frame.minY, cell.maxY + 4)
    }

    func testThePanelStaysInsideTheScreenSideways() {
        let right = CGRect(x: 1_400, y: 500, width: 100, height: 25)
        XCTAssertEqual(CellPeekLayout.frame(size: panel, cell: right, visible: screen).maxX, 1_440)
        let left = CGRect(x: -50, y: 500, width: 100, height: 25)
        XCTAssertEqual(CellPeekLayout.frame(size: panel, cell: left, visible: screen).minX, 0)
    }

    func testThePanelIsAsTallAsTheReaderNeedsAndNeverTallerThanTheMaximum() {
        XCTAssertEqual(CellPeekLayout.size(for: "K-1"), CGSize(width: 592, height: 290),
                       "the reader's chrome and its minimum content")
        XCTAssertEqual(CellPeekLayout.size(for: "{\"a\":1}").height, 370, "a JSON value opens on its tree")
        XCTAssertEqual(CellPeekLayout.size(for: "K-1", hasNote: true).height, 334, "a note takes a line more")
        let tall = String(repeating: "line\n", count: 500)
        XCTAssertEqual(CellPeekLayout.size(for: tall).height, 170 + 16 * 15 + 24, "sixteen lines, then it scrolls")
        XCTAssertEqual(CellPeekLayout.size(for: tall, hasNote: true).height, 460, "clamped")
        for text in ["", "x", tall, "[1]"] {
            for note in [false, true] {
                let size = CellPeekLayout.size(for: text, hasNote: note)
                XCTAssertGreaterThanOrEqual(size.height, CellPeekPanel.minSize.height)
                XCTAssertLessThanOrEqual(size.height, CellPeekPanel.maxSize.height)
                XCTAssertLessThanOrEqual(size.width, CellPeekPanel.maxSize.width)
            }
        }
    }

    /// Left alone, a hosting view sizes its window to the content's ideal size, which for a long
    /// value is thousands of points: the panel has to keep the size it was given.
    func testThePanelKeepsTheSizeItWasGivenWhateverTheValue() throws {
        let panel = CellPeekPanel()
        addTeardownBlock { @MainActor in panel.dismiss() }
        let anchor = CGRect(x: 300, y: 500, width: 120, height: 25)
        let long = String(repeating: "abc\n", count: 40_000)
        panel.show(value: long, column: "c", type: "text", connectionID: nil, table: nil, anchor: anchor,
                   appearance: nil)
        GridFixture.pause(for: 0.2)
        XCTAssertEqual(panel.frame.size, CellPeekLayout.size(for: CellPeekLayout.clip(long).text, hasNote: true))
        XCTAssertLessThanOrEqual(panel.frame.height, CellPeekPanel.maxSize.height)

        panel.show(value: "short", column: "c", type: "text", connectionID: nil, table: nil, anchor: anchor,
                   appearance: nil)
        GridFixture.pause(for: 0.2)
        XCTAssertEqual(panel.frame.size, CellPeekLayout.size(for: "short"))
        panel.showReading(anchor: anchor, appearance: nil)
        XCTAssertEqual(panel.frame.size, CellPeekPanel.minSize)
        XCTAssertTrue(panel.isReading)
        panel.dismiss()
        XCTAssertFalse(panel.isOpen)
        XCTAssertFalse(panel.isVisible)
    }

    // MARK: Following the cursor

    private func makeGrid(rowCount: Int = 40, long: String = String(repeating: "w", count: 100_000)) -> GridFixture {
        let columns = [Event.Column(name: "kode", type: "text"),
                       Event.Column(name: "jumlah", type: "bigint"),
                       Event.Column(name: "catatan", type: "text")]
        let rows = (0..<rowCount).map { row -> [String?] in
            if row == 0 { return ["K-0", "7", nil] }
            if row == 2 { return ["K-2", "9", long] }
            return ["K-\(row)", "\(row)", "catatan \(row)"]
        }
        let fixture = GridFixture(columns: columns, rows: rows)
        fixture.apply()
        addTeardownBlock { @MainActor in fixture.coordinator.closePeek() }
        return fixture
    }

    private func press(_ fixture: GridFixture, row: Int, column: Int) {
        fixture.coordinator.press(at: fixture.pointInCell(row: row, column: column), clickCount: 1)
        fixture.coordinator.release()
    }

    func testSpaceOpensThePeekOnTheCursorCellAndShowsItsValue() throws {
        let fixture = makeGrid()
        press(fixture, row: 1, column: 0)
        XCTAssertFalse(fixture.coordinator.isPeekOpen)

        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.space)))
        GridFixture.drain(until: { fixture.coordinator.peek?.shownText == "K-1" })
        let peek = try XCTUnwrap(fixture.coordinator.peek)
        XCTAssertEqual(peek.shownText, "K-1")
        XCTAssertTrue(peek.isOpen)
        XCTAssertTrue(peek.isVisible)
        XCTAssertEqual(Announcer.last, "Peek: kode, K-1")
        XCTAssertFalse(peek.canBecomeKey, "the grid keeps the keyboard")
    }

    /// The arrows still reach the table with the peek up, and the peek follows (Quick Look in Finder).
    func testTheArrowKeysMoveTheCursorAndThePeekFollows() throws {
        let fixture = makeGrid()
        press(fixture, row: 1, column: 0)
        _ = fixture.coordinator.handleKey(GridKey(.space))
        GridFixture.drain(until: { fixture.coordinator.peek?.shownText == "K-1" })

        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.down)))
        GridFixture.drain(until: { fixture.coordinator.peek?.shownText?.hasPrefix("K-2") == true })
        XCTAssertEqual(fixture.coordinator.peek?.shownText, "K-2")
        XCTAssertEqual(fixture.tab.cellCursor?.focus, CellPos(row: 2, column: 0))

        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.right)))
        GridFixture.drain(until: { fixture.coordinator.peek?.shownText == "9" })
        XCTAssertEqual(fixture.coordinator.peek?.shownText, "9", "row 2, column `jumlah`")
        XCTAssertTrue(fixture.coordinator.isPeekOpen)
    }

    /// A value past 64 KiB is read whole and shown cut, with the note.
    func testALongCellIsShownCutWithTheNote() throws {
        let fixture = makeGrid()
        press(fixture, row: 2, column: 2)
        _ = fixture.coordinator.handleKey(GridKey(.space))
        GridFixture.drain(until: { fixture.coordinator.peek?.note != nil })
        let peek = try XCTUnwrap(fixture.coordinator.peek)
        XCTAssertEqual(peek.shownText?.utf8.count, 65_536)
        XCTAssertTrue(peek.note?.hasPrefix("Showing the first 64 KiB of 97.7 KiB.") == true, peek.note ?? "")
    }

    func testANullCellIsSaidToBeNull() throws {
        let fixture = makeGrid()
        press(fixture, row: 0, column: 2)
        _ = fixture.coordinator.handleKey(GridKey(.space))
        GridFixture.drain(until: { fixture.coordinator.peek?.note != nil })
        XCTAssertEqual(fixture.coordinator.peek?.note, "This cell is NULL.")
        XCTAssertEqual(Announcer.last, "Peek: catatan, null")
    }

    func testAStagedEditIsWhatThePeekShows() throws {
        let fixture = makeGrid()
        fixture.tab.beginCellEdit(at: CellKey(row: 3, column: 0))
        fixture.tab.typeCellEdit("K-3 edited")
        fixture.tab.endCellEdit()
        press(fixture, row: 3, column: 0)
        _ = fixture.coordinator.handleKey(GridKey(.space))
        XCTAssertEqual(fixture.coordinator.peek?.shownText, "K-3 edited", "no read needed, so no wait")
    }

    // MARK: Closing

    func testEscapeClosesThePeekBeforeItTouchesTheSelection() throws {
        let fixture = makeGrid()
        press(fixture, row: 1, column: 0)
        _ = fixture.coordinator.handleKey(GridKey(.space))
        GridFixture.drain(until: { fixture.coordinator.peek?.shownText == "K-1" })

        XCTAssertTrue(fixture.coordinator.handleKey(GridKey(.escape)))
        XCTAssertFalse(fixture.coordinator.isPeekOpen)
        XCTAssertFalse(try XCTUnwrap(fixture.coordinator.peek).isVisible)
        XCTAssertNotNil(fixture.tab.cellSelection, "the first Esc closed the peek and nothing else")
    }

    func testSpaceAgainAndAClickAndANewResultCloseIt() throws {
        let fixture = makeGrid()
        press(fixture, row: 1, column: 0)

        _ = fixture.coordinator.handleKey(GridKey(.space))
        GridFixture.drain(until: { fixture.coordinator.peek?.shownText == "K-1" })
        _ = fixture.coordinator.handleKey(GridKey(.space))
        XCTAssertFalse(fixture.coordinator.isPeekOpen, "Space again")

        _ = fixture.coordinator.handleKey(GridKey(.space))
        GridFixture.drain(until: { fixture.coordinator.peek?.shownText == "K-1" })
        fixture.coordinator.press(at: fixture.pointInCell(row: 4, column: 1), clickCount: 1)
        XCTAssertFalse(fixture.coordinator.isPeekOpen, "a click in the grid")

        _ = fixture.coordinator.handleKey(GridKey(.space))
        GridFixture.drain(until: { fixture.coordinator.peek?.shownText == "4" })
        fixture.apply(fixture.inputs(revision: 2), force: false)
        XCTAssertFalse(fixture.coordinator.isPeekOpen, "a new result: the cell it showed is gone")
    }

    /// With nothing under the cursor there is nothing to peek at.
    func testSpaceWithoutACursorDoesNothing() {
        let fixture = makeGrid()
        XCTAssertFalse(fixture.coordinator.handleKey(GridKey(.space)))
        XCTAssertFalse(fixture.coordinator.isPeekOpen)
    }

    /// A read that comes back for a cell the cursor has already left is dropped, so a quick run of
    /// arrow keys cannot leave the panel on a cell the cursor is not on.
    func testAStaleReadIsDropped() throws {
        let fixture = makeGrid()
        press(fixture, row: 3, column: 0)
        _ = fixture.coordinator.handleKey(GridKey(.space))
        for _ in 0..<5 { _ = fixture.coordinator.handleKey(GridKey(.down)) }
        GridFixture.drain(until: { fixture.coordinator.peek?.shownText == "K-8" })
        GridFixture.pause(for: 0.2)
        XCTAssertEqual(fixture.coordinator.peek?.shownText, "K-8")
        XCTAssertEqual(fixture.tab.cellCursor?.focus.row, 8)
    }

    // MARK: No files (NFR-S5)

    /// The peek reads a value into memory and nothing else. The value carries a marker no other
    /// process has, and every file written to the temporary directory while the peek was up is
    /// searched for it: a copy of the value on disk would be one (a Quick Look preview of a cell
    /// would have written exactly that).
    func testThePeekWritesNoFileHoldingTheValue() throws {
        let marker = "peek-marker-\(UUID().uuidString)"
        let started = Date().addingTimeInterval(-1)
        let fixture = makeGrid(long: marker + String(repeating: "w", count: 100_000))
        press(fixture, row: 2, column: 2)
        _ = fixture.coordinator.handleKey(GridKey(.space))
        GridFixture.drain(until: { fixture.coordinator.peek?.note != nil })
        XCTAssertTrue(fixture.coordinator.peek?.shownText?.hasPrefix(marker) == true, "the peek did read it")
        _ = fixture.coordinator.handleKey(GridKey(.down))
        GridFixture.pause(for: 0.3)
        fixture.coordinator.closePeek()

        let needle = try XCTUnwrap(marker.data(using: .utf8))
        let keys: [URLResourceKey] = [.contentModificationDateKey, .isRegularFileKey, .fileSizeKey]
        let root = URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
        var found: [String] = []
        let walker = try XCTUnwrap(FileManager.default.enumerator(at: root, includingPropertiesForKeys: keys,
                                                                   options: [.skipsPackageDescendants]))
        for case let url as URL in walker {
            guard let values = try? url.resourceValues(forKeys: Set(keys)), values.isRegularFile == true,
                  (values.contentModificationDate ?? .distantPast) >= started,
                  (values.fileSize ?? 0) <= 8 << 20,
                  let data = try? Data(contentsOf: url, options: .mappedIfSafe) else { continue }
            if data.range(of: needle) != nil { found.append(url.path) }
        }
        XCTAssertEqual(found, [], "the peek wrote the value to disk")
    }
}
