import AppKit
import QueryHiveFFI
import XCTest

@testable import QueryHive

/// Matching brackets and quotes (FR-ED-08, D-14): the pair comes from Rust a moment after the caret
/// stops, is worn only if nothing moved since it was asked for, and is drawn behind the text.
@MainActor
final class EditorBracketsTests: XCTestCase {
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

    // `(` at 8 closes at 15; the inner `(` at 12 closes at 14.
    private static let sql = "SELECT f(a, (b)) FROM t WHERE x = 'q'"

    private func editor(_ sql: String = sql, theme: AppTheme = .daylight) throws -> EditorTestHost {
        let h = try EditorTestHost(sql: sql, theme: theme)
        host = h
        try h.settle()
        return h
    }

    private func caret(_ h: EditorTestHost, at offset: Int) {
        h.textView.setSelectedRange(NSRange(location: offset, length: 0))
    }

    func testThePairAppearsOnceTheCaretHasSettledNextToABracket() throws {
        let h = try editor()
        XCTAssertEqual(h.textView.bracketRanges, [], "nothing is drawn before the caret is next to a delimiter")
        caret(h, at: 9)
        // Not at once: the ask waits for the caret to stop, so a held arrow key pays for nothing.
        XCTAssertEqual(h.textView.bracketRanges, [])
        XCTAssertTrue(h.pump(until: { !h.textView.bracketRanges.isEmpty }), "the pair never arrived")
        XCTAssertEqual(h.textView.bracketRanges, [NSRange(location: 8, length: 1), NSRange(location: 15, length: 1)])
        caret(h, at: 13)
        XCTAssertTrue(h.pump(until: { h.textView.bracketRanges.first?.location == 12 }))
        XCTAssertEqual(h.textView.bracketRanges, [NSRange(location: 12, length: 1), NSRange(location: 14, length: 1)])
        caret(h, at: 0)
        XCTAssertTrue(h.pump(until: { h.textView.bracketRanges.isEmpty }), "the bands stayed after the caret left")
    }

    func testQuotesPairToo() throws {
        let h = try editor()
        let quote = (Self.sql as NSString).range(of: "'q'").location
        caret(h, at: quote + 1)
        XCTAssertTrue(h.pump(until: { !h.textView.bracketRanges.isEmpty }))
        XCTAssertEqual(h.textView.bracketRanges,
                       [NSRange(location: quote, length: 1), NSRange(location: quote + 2, length: 1)])
    }

    func testASelectionClearsThePair() throws {
        let h = try editor()
        caret(h, at: 9)
        XCTAssertTrue(h.pump(until: { !h.textView.bracketRanges.isEmpty }))
        h.textView.setSelectedRange(NSRange(location: 9, length: 3))
        XCTAssertEqual(h.textView.bracketRanges, [], "a selection has no pair")
        h.pump(for: 0.2)
        XCTAssertEqual(h.textView.bracketRanges, [], "and none comes back for it")
    }

    /// An edit between the ask and the answer: the bands go with the text they were computed for, and
    /// the pair that comes after is the edited text's.
    func testAnEditDropsTheOldPairAndTheNextOneIsTheNewTexts() throws {
        let h = try editor()
        caret(h, at: 9)
        XCTAssertTrue(h.pump(until: { !h.textView.bracketRanges.isEmpty }))
        h.textView.insertText("zz", replacementRange: NSRange(location: 0, length: 0))
        XCTAssertEqual(h.textView.bracketRanges, [], "the old pair outlived the edit")
        h.pump(for: 0.3)
        // The caret moved with the typed text, so it is no longer next to the `(`.
        let open = (h.textView.string as NSString).range(of: "(").location
        caret(h, at: open + 1)
        XCTAssertTrue(h.pump(until: { !h.textView.bracketRanges.isEmpty }))
        XCTAssertEqual(h.textView.bracketRanges.first, NSRange(location: open, length: 1))
    }

    /// A pair asked for when the caret was elsewhere is not worn: the generation moved on.
    func testAnAnswerForACaretThatMovedIsIgnored() throws {
        let h = try editor()
        caret(h, at: 9)
        caret(h, at: 0)
        h.pump(for: 0.3)
        XCTAssertEqual(h.textView.bracketRanges, [], "the first ask's answer was worn after the caret moved")
    }

    func testTheRevisionTheFFIWasAskedAtDecidesWhetherItIsStale() throws {
        let document = try EditorDocument(text: Self.sql, dialect: .generic)
        XCTAssertEqual(try document.bracketPair(revision: 1, offsetUtf16: 9), [8, 1, 15, 1])
        let next = try document.replace(startUtf16: 0, lenUtf16: 0, text: "zz")
        XCTAssertThrowsError(try document.bracketPair(revision: 1, offsetUtf16: 9)) { error in
            XCTAssertEqual(error as? EditorError, .Stale)
        }
        XCTAssertEqual(try document.bracketPair(revision: next, offsetUtf16: 11), [10, 1, 17, 1])
    }

    func testTheWrapperRefusesAnImpossiblePacketAndLiftsAGoodOne() throws {
        let analysis = try EditorAnalysis(text: Self.sql)
        let found = try analysis.bracketPair(at: 9)
        XCTAssertEqual(found.revision, analysis.revision)
        XCTAssertEqual(found.ranges, [NSRange(location: 8, length: 1), NSRange(location: 15, length: 1)])
        XCTAssertEqual(try analysis.bracketPair(at: 0).ranges, [])
        // Past the end is no pair, not an error.
        XCTAssertEqual(try analysis.bracketPair(at: 10_000).ranges, [])
    }

    // MARK: Drawing

    /// The bands sit on the delimiters: the rectangles are the glyphs' own, and the outline's pixels
    /// differ from the same row one character away.
    func testTheBandsAreDrawnOnTheDelimiters() throws {
        let h = try editor()
        let tv = h.textView
        caret(h, at: 9)
        XCTAssertTrue(h.pump(until: { !tv.bracketRanges.isEmpty }))
        h.pump(for: 0.2)
        let rects = tv.bracketRects(for: tv.bracketRanges)
        XCTAssertEqual(rects.count, 2)
        let layoutManager = try XCTUnwrap(tv.layoutManager), container = try XCTUnwrap(tv.textContainer)
        for (rect, range) in zip(rects, tv.bracketRanges) {
            var expected = layoutManager.boundingRect(
                forGlyphRange: layoutManager.glyphRange(forCharacterRange: range, actualCharacterRange: nil), in: container)
            expected.origin.x += tv.textContainerOrigin.x
            expected.origin.y += tv.textContainerOrigin.y
            XCTAssertEqual(rect, expected)
        }
        let rep = try h.bitmap()
        let size = h.view.bounds.size
        for rect in rects {
            let inHost = tv.convert(rect, to: h.view)
            let y = h.view.isFlipped ? inHost.midY : size.height - inHost.midY
            // The left edge of the band is its outline; the cell to its left is the plain row.
            let edge = rep.rgb(atX: inHost.minX + 0.5, y: y, viewSize: size)
            let beside = rep.rgb(atX: inHost.minX - inHost.width, y: y, viewSize: size)
            let distance = abs(edge.r - beside.r) + abs(edge.g - beside.g) + abs(edge.b - beside.b)
            XCTAssertGreaterThan(distance, 40, "no band outline at \(inHost)")
        }
        // And with the pair gone, the pixels go back to the plain row.
        caret(h, at: 0)
        XCTAssertTrue(h.pump(until: { tv.bracketRanges.isEmpty }))
        h.pump(for: 0.2)
        let after = try h.bitmap()
        let inHost = tv.convert(rects[0], to: h.view)
        let y = h.view.isFlipped ? inHost.midY : size.height - inHost.midY
        let edge = after.rgb(atX: inHost.minX + 0.5, y: y, viewSize: size)
        let beside = after.rgb(atX: inHost.minX - inHost.width, y: y, viewSize: size)
        XCTAssertLessThan(abs(edge.r - beside.r) + abs(edge.g - beside.g) + abs(edge.b - beside.b), 12)
    }
}
