import AppKit
import XCTest

@testable import QueryHive

/// The editor's gutter on every canvas (FR-ED-10, D-15) and what is under it (B-9).
///
/// Read from the rendered editor, not from the colour it was asked to draw: the claim is that a
/// person can see the strip, and that is a difference between pixels.
@MainActor
final class GutterRecessTests: XCTestCase {
    private var host: EditorTestHost?

    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    override func tearDown() {
        host?.close()
        host = nil
        super.tearDown()
    }

    private static let sql = "SELECT 1;\nSELECT 2"

    /// Light: black at 4.5% over a near-white surface is at least 4 of 255 darker than the text area.
    /// Dark: white at 3.5% over a near-black one is at least 4 lighter (7.4 on Midnight).
    func testTheGutterIsAVisibleStripOnAllSevenCanvases() throws {
        XCTAssertEqual(AppTheme.allCases.count, 7)
        for theme in AppTheme.allCases {
            let h = try EditorTestHost(sql: Self.sql, theme: theme)
            defer { h.close() }
            try h.settle()
            let rep = try h.bitmap()
            let size = h.view.bounds.size
            // Below the text, where the gutter has no number or marker and the text area no glyph. In the
            // gutter's own x, and well right of it in the text area.
            let y = size.height - 80
            let gutter = EditorLayout.current.runButtonPerStatement
                ? LineNumberRulerView.gutterWidth(forLines: 2, showsRunMarks: true) : 0
            let strip = NSBitmapImageRep.luma(rep.rgb(atX: 12 + gutter / 2, y: y, viewSize: size))
            let text = NSBitmapImageRep.luma(rep.rgb(atX: 12 + gutter + 150, y: y, viewSize: size))
            // Signed so the direction is checked too: darker on a light canvas, lighter on a dark one.
            let difference = theme.isDark ? strip - text : text - strip
            XCTAssertGreaterThanOrEqual(difference, 4,
                                        "\(theme): the gutter differs from the text area by \(difference)/255 "
                                        + "(gutter \(strip), text \(text))")
        }
    }

    func testTheNumbersAreElevenPointsAndTheGutterFitsFourDigits() throws {
        XCTAssertEqual(LineNumberRulerView.numberPointSize, 11)
        let h = try EditorTestHost(sql: Self.sql)
        host = h
        XCTAssertEqual(h.ruler.numberFont.pointSize, 11)
        let widest = ("8888" as NSString).size(withAttributes: [.font: h.ruler.numberFont]).width
        XCTAssertGreaterThan(LineNumberRulerView.gutterWidth(forLines: 8888, showsRunMarks: true),
                             widest + 8 + LineNumberRulerView.runColumn,
                             "the widest four-digit number does not fit the gutter")
    }

    // MARK: B-9, text under the gutter

    private static let longLine = "SELECT " + (1...80).map { "column_number_\($0)" }.joined(separator: ", ")
        + " FROM t;\nSELECT 2"

    private func noWrap() -> EditorLayout {
        var layout = EditorLayout.standard
        layout.wordWrap = false
        return layout
    }

    /// At rest the text starts to the right of the gutter, not under it: the first glyph's x, in the
    /// view's own space, is past the gutter's edge. This is what the recorded baseline must say.
    func testWithoutWrapTheTextStartsPastTheGutter() throws {
        let h = try EditorTestHost(sql: Self.longLine, layout: noWrap())
        host = h
        try h.settle()
        let layoutManager = try XCTUnwrap(h.textView.layoutManager)
        let first = layoutManager.boundingRect(forGlyphRange: NSRange(location: 0, length: 1),
                                               in: try XCTUnwrap(h.textView.textContainer))
        let inClip = h.scrollView.contentView.convert(NSPoint(x: first.minX + h.textView.textContainerOrigin.x, y: 0),
                                                      from: h.textView)
        // From the clip view's left edge, which is where the gutter starts: its bounds begin at minus the
        // gutter's width, so a point in the text view is shifted by that.
        let fromEdge = inClip.x - h.scrollView.contentView.bounds.minX
        XCTAssertGreaterThanOrEqual(fromEdge, h.ruler.ruleThickness, "the first glyph is drawn under the gutter at rest")
    }

    /// Scrolled sideways, no pixel of the gutter's free columns holds text: they are the same as a
    /// blank row's. Before the fix a column of the line read through the translucent gutter.
    func testTextScrolledSidewaysNeverShowsThroughTheGutter() throws {
        let h = try EditorTestHost(sql: Self.longLine, layout: noWrap(), theme: .daylight)
        host = h
        try h.settle()
        let clip = h.scrollView.contentView
        clip.scroll(to: NSPoint(x: 300, y: 0))
        h.scrollView.reflectScrolledClipView(clip)
        h.pump(for: 0.3)
        let rep = try h.bitmap()
        let size = h.view.bounds.size
        // Between the run column and the numbers, in the first line's row and in a row below the text.
        let columns = stride(from: 12 + 15.0, through: 12 + 30.0, by: 1.0)
        let textRow = 9 + 10.0 + 8, blankRow = size.height - 80
        for x in columns {
            let withText = rep.rgb(atX: x, y: textRow, viewSize: size)
            let blank = rep.rgb(atX: x, y: blankRow, viewSize: size)
            XCTAssertEqual(withText.r, blank.r, accuracy: 3, "x=\(x): text shows through the gutter")
            XCTAssertEqual(withText.g, blank.g, accuracy: 3, "x=\(x): text shows through the gutter")
            XCTAssertEqual(withText.b, blank.b, accuracy: 3, "x=\(x): text shows through the gutter")
        }
    }

    /// The caret is brought clear of the gutter, not to the very edge of the view's visible rect, which
    /// starts under it. AppKit does this itself; the test keeps it from regressing.
    func testScrollingToACaretLeavesItClearOfTheGutter() throws {
        let h = try EditorTestHost(sql: Self.longLine, layout: noWrap())
        host = h
        try h.settle()
        let tv = h.textView
        let layoutManager = try XCTUnwrap(tv.layoutManager), container = try XCTUnwrap(tv.textContainer)
        func x(of index: Int) -> CGFloat {
            layoutManager.boundingRect(forGlyphRange: NSRange(location: index, length: 1), in: container).minX
                + tv.textContainerOrigin.x
        }
        let far = (Self.longLine as NSString).range(of: "column_number_70").location
        tv.setSelectedRange(NSRange(location: far, length: 0))
        tv.scrollRangeToVisible(NSRange(location: far, length: 0))
        h.pump(for: 0.2)
        let scrolled = h.scrollView.contentView.bounds.minX
        XCTAssertGreaterThan(scrolled, 100, "the first move did not scroll sideways")
        // A character a little left of the visible area: bringing it into view scrolls left.
        var index = far
        while index > 0, x(of: index) > scrolled - 12 { index -= 1 }
        tv.setSelectedRange(NSRange(location: index, length: 0))
        tv.scrollRangeToVisible(NSRange(location: index, length: 0))
        h.pump(for: 0.2)
        let fromEdge = x(of: index) - h.scrollView.contentView.bounds.minX
        XCTAssertGreaterThanOrEqual(fromEdge, h.ruler.ruleThickness - 0.5, "the caret is under the gutter")
    }

    /// The blinking caret is drawn outside `draw(_:)`, so the clip there does not cover it: one scrolled
    /// under the gutter must not be drawn.
    func testTheCaretIsNotDrawnUnderTheGutter() throws {
        let h = try EditorTestHost(sql: Self.longLine, layout: noWrap(), theme: .daylight)
        host = h
        try h.settle()
        let tv = h.textView
        let clip = h.scrollView.contentView
        clip.scroll(to: NSPoint(x: 300, y: 0))
        h.scrollView.reflectScrolledClipView(clip)
        h.pump(for: 0.2)
        func caretPixels(at x: CGFloat) throws -> Int {
            let rect = NSRect(x: x, y: tv.textContainerOrigin.y, width: 3, height: 15)
            let rep = try XCTUnwrap(NSBitmapImageRep(
                bitmapDataPlanes: nil, pixelsWide: Int(tv.bounds.width.rounded(.up)), pixelsHigh: 40,
                bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0))
            let context = try XCTUnwrap(NSGraphicsContext(bitmapImageRep: rep))
            NSGraphicsContext.saveGraphicsState()
            NSGraphicsContext.current = context
            // The text view is flipped; the bitmap is not, so the context is flipped to match.
            context.cgContext.translateBy(x: 0, y: 40)
            context.cgContext.scaleBy(x: 1, y: -1)
            tv.drawInsertionPoint(in: rect, color: NSColor(red: 1, green: 0, blue: 0, alpha: 1), turnedOn: true)
            NSGraphicsContext.restoreGraphicsState()
            var red = 0
            for px in 0..<rep.pixelsWide { for py in 0..<rep.pixelsHigh {
                if let c = rep.colorAt(x: px, y: py)?.usingColorSpace(.sRGB), c.alphaComponent > 0.5,
                   c.redComponent > 0.9, c.greenComponent < 0.3 { red += 1 }
            } }
            return red
        }
        XCTAssertEqual(try caretPixels(at: 300 + 10), 0, "a caret was drawn under the gutter")
        XCTAssertGreaterThan(try caretPixels(at: 300 + h.ruler.ruleThickness + 10), 0, "and the probe sees a caret elsewhere")
    }
}
