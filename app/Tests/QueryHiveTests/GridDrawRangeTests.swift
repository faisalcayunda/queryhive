import AppKit
import XCTest

@testable import QueryHive

/// Regression for a main-thread crash in `GridTableView.draw(_:)`:
/// "Range requires lowerBound <= upperBound".
///
/// The painter computes the covered rows as `first...last`, where `first` comes from
/// `dirtyRect.minY` and `last` from `dirtyRect.maxY` clamped to the rows that exist. When the dirty
/// rectangle lies wholly **below** the rows — an elastic bounce, or a result that just shrank before
/// the view re-laid itself out — `first` lands past `last` and the closed range traps. Drawing such
/// a rectangle must be a no-op instead of a crash.
///
/// The tests hand `draw(_:)` a real `NSGraphicsContext` so the early `guard ... cgContext else
/// return` is satisfied and the row-range arithmetic actually runs; calling `draw` without a context
/// would return before reaching it and could never catch the bug.
@MainActor
final class GridDrawRangeTests: XCTestCase {

    private func makeTable(rows: Int) -> GridFixture {
        let columns = [Event.Column(name: "a", type: "Text"),
                       Event.Column(name: "b", type: "Text")]
        let body = (0..<rows).map { r -> [String?] in ["r\(r)-a", "r\(r)-b"] }
        let fixture = GridFixture(columns: columns, rows: body)
        fixture.apply()
        return fixture
    }

    /// Run `draw(_:)` against a real graphics context so the guard on `NSGraphicsContext.current`
    /// passes and the row-range arithmetic is exercised. Any Swift trap inside `draw` fails the run.
    private func draw(_ rect: NSRect, on table: GridTableView) {
        let size = CGSize(width: 512, height: 512)
        guard let rep = NSBitmapImageRep(bitmapDataPlanes: nil,
                                         pixelsWide: Int(size.width),
                                         pixelsHigh: Int(size.height),
                                         bitsPerSample: 8,
                                         samplesPerPixel: 4,
                                         hasAlpha: true,
                                         isPlanar: false,
                                         colorSpaceName: .deviceRGB,
                                         bytesPerRow: 0,
                                         bitsPerPixel: 0),
              let ctx = NSGraphicsContext(bitmapImageRep: rep) else {
            XCTFail("could not create an off-screen graphics context")
            return
        }
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = ctx
        table.draw(rect)
        NSGraphicsContext.restoreGraphicsState()
    }

    /// A dirty rectangle wholly below the rows used to trap on `first...last`. It must not.
    func testDrawRectangleBelowTheRowsDoesNotCrash() {
        let fixture = makeTable(rows: 3)
        // Well past the last row (3 rows ≈ a few dozen points): `first` lands past `last`.
        let below = CGRect(x: 0, y: 100_000, width: 400, height: 200)
        draw(below, on: fixture.table)
    }

    /// A rectangle that starts inside the rows but extends past them must still draw the rows it
    /// overlaps, without trapping on the part below the content.
    func testDrawRectangleStraddlingTheBottomRowDoesNotCrash() {
        let fixture = makeTable(rows: 3)
        let lastRowY = fixture.table.rect(ofRow: 2).midY
        let straddle = CGRect(x: 0, y: lastRowY, width: 400, height: 100_000)
        draw(straddle, on: fixture.table)
    }

    /// A normal on-screen rectangle still draws (the guard must not swallow real paints).
    func testDrawRectangleOverTheRowsStillDraws() {
        let fixture = makeTable(rows: 3)
        let over = fixture.table.rect(ofRow: 1)
        draw(over, on: fixture.table)
    }
}
