import XCTest

@testable import QueryHive

/// The highlight band: the arithmetic that puts it on the line it belongs to.
///
/// Worth its own test because the defect it pins was invisible in review. TextKit reports line
/// fragments from the text container's origin and the band is filled in the view's, so a missing
/// inset moved every band up by nine points — which read as a stray strip of wash above the statement
/// and a bare bottom edge under it, with nothing in the code looking wrong.
final class HighlightBandTests: XCTestCase {
    private let inset = NSSize(width: 8, height: 9)

    func testTheBandIsMovedDownByTheTextContainerInset() {
        let band = NSRect(x: 12, y: 40, width: 200, height: 18)
        let rect = HighlightBand.rect(for: band, boundsWidth: 600, inset: inset)
        XCTAssertEqual(rect.minY, 49)
        // The height is TextKit's and is not this function's to change.
        XCTAssertEqual(rect.height, 18)
    }

    func testTheBandIsAsWideAsTheViewEvenWhenTheLineIsShort() {
        // Full width, or the highlight reads as a highlight of the text rather than as a line of the
        // editor.
        let rect = HighlightBand.rect(for: NSRect(x: 0, y: 0, width: 120, height: 18),
                                      boundsWidth: 600, inset: inset)
        XCTAssertEqual(rect.minX, 0)
        XCTAssertEqual(rect.width, 600)
    }

    func testABandReachingPastTheViewKeepsItsOwnWidth() {
        // A long line in a view scrolled narrower: the band still has to reach the text.
        let rect = HighlightBand.rect(for: NSRect(x: 0, y: 0, width: 900, height: 18),
                                      boundsWidth: 600, inset: inset)
        XCTAssertEqual(rect.width, 900)
    }

    func testNoInsetLeavesTheBandWhereTextKitPutIt() {
        // The one case with nothing to correct, and the value the editor would use if the inset were
        // ever set to zero.
        let band = NSRect(x: 0, y: 40, width: 200, height: 18)
        XCTAssertEqual(HighlightBand.rect(for: band, boundsWidth: 600, inset: .zero).minY, 40)
    }
}
