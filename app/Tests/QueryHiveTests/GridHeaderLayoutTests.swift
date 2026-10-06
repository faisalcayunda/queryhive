import AppKit
import XCTest

@testable import QueryHive

/// The header band must sit above row 0, not on top of it.
///
/// The owner's screenshot showed row 1 drawn under the translucent header once a result was taller
/// than the viewport. The header is laid out by the enclosing scroll view, so a header that changes
/// height has to tile the scroll view too, not only the table.
@MainActor
final class GridHeaderLayoutTests: XCTestCase {

    private func makeGrid(rowCount: Int) -> GridFixture {
        GridFixture.ensureApplication()
        // The rows live in a result store, which the shared host only has once it is configured;
        // the other grid suites get that from whichever test ran first, this one runs alone too.
        TestStores.ensureConfigured()
        let columns = [Event.Column(name: "kode", type: "text"),
                       Event.Column(name: "jumlah", type: "bigint")]
        let rows = (0..<rowCount).map { ["K-\($0)", "\($0)"] as [String?] }
        return GridFixture(columns: columns, rows: rows, viewport: CGSize(width: 900, height: 300))
    }

    /// Row 0 starts at or below the header's bottom edge, in window coordinates (y points up there,
    /// so "below" is a smaller y), and the clip view is still at the top.
    private func assertHeaderAboveRowZero(_ fixture: GridFixture,
                                          file: StaticString = #filePath, line: UInt = #line) {
        let header = fixture.table.header
        let headerRect = header.convert(header.bounds, to: nil)
        let rowRect = fixture.table.convert(fixture.table.rect(ofRow: 0), to: nil)
        XCTAssertLessThanOrEqual(rowRect.maxY, headerRect.minY + 0.5,
                                 "row 0 (top \(rowRect.maxY)) must not reach into the header (bottom \(headerRect.minY))",
                                 file: file, line: line)
        let clip = fixture.scroll.contentView
        XCTAssertEqual(clip.contentInsets.top, header.frame.height + fixture.scroll.contentInsets.top,
                       accuracy: 0.5, "the clip view is inset by the header", file: file, line: line)
        XCTAssertEqual(clip.bounds.origin.y, -clip.contentInsets.top,
                       accuracy: 0.5, "the clip view stays at the top", file: file, line: line)
        XCTAssertEqual(header.frame.height, fixture.table.headerHeight, accuracy: 0.01, file: file, line: line)
    }

    /// A result taller than the viewport, which is the case that broke.
    func testTheHeaderSitsAboveRowZeroWhenTheResultIsTallerThanTheViewport() {
        let fixture = makeGrid(rowCount: 100)
        fixture.apply()
        assertHeaderAboveRowZero(fixture)
    }

    /// A taller header (a bigger grid font) has to push the clip view down with it.
    func testTheHeaderSitsAboveRowZeroAfterItGrows() {
        let fixture = makeGrid(rowCount: 100)
        fixture.apply()
        let before = fixture.table.headerHeight

        var inputs = fixture.inputs(revision: 2)
        inputs.style.fontSize = 16
        fixture.apply(inputs)

        XCTAssertGreaterThan(fixture.table.headerHeight, before, "a bigger font grows the header")
        assertHeaderAboveRowZero(fixture)
    }

    /// A result that fits the viewport never broke; it stays right.
    func testTheHeaderSitsAboveRowZeroOnAShortResult() {
        let fixture = makeGrid(rowCount: 5)
        fixture.apply()
        assertHeaderAboveRowZero(fixture)
    }
}
