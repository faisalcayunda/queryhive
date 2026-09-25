import AppKit
import XCTest

@testable import QueryHive

/// The editor's line-number gutter.
///
/// Checked through `numberedLines(in:)` rather than through pixels: what can go wrong here is not
/// the drawing, it is the *decision* of which line fragments are lines. That decision is easy to
/// get subtly wrong and impossible to see — a query whose long line wraps would report more lines
/// than it has, and the count in the corner would contradict the gutter.
@MainActor
final class LineNumberRulerTests: XCTestCase {
    /// A text view laid out at a fixed width, so wrapping is predictable.
    private func editor(_ sql: String, width: CGFloat = 400) -> (NSTextView, NSTextContainer) {
        let storage = NSTextStorage(string: sql)
        let layoutManager = NSLayoutManager()
        storage.addLayoutManager(layoutManager)
        let container = NSTextContainer(size: NSSize(width: width, height: .greatestFiniteMagnitude))
        container.widthTracksTextView = false
        layoutManager.addTextContainer(container)
        let textView = NSTextView(frame: NSRect(x: 0, y: 0, width: width, height: 400),
                                  textContainer: container)
        textView.font = .monospacedSystemFont(ofSize: 13, weight: .regular)
        layoutManager.ensureLayout(for: container)
        return (textView, container)
    }

    private func numbers(_ sql: String, width: CGFloat = 400, viewHeight: CGFloat = 4000) -> [Int] {
        let (textView, _) = editor(sql, width: width)
        let ruler = LineNumberRulerView(textView: textView)
        let visible = NSRect(x: 0, y: 0, width: width, height: viewHeight)
        return ruler.numberedLines(in: visible).map(\.number)
    }

    func testOneLineIsNumberedOne() {
        XCTAssertEqual(numbers("SELECT 1"), [1])
    }

    func testEachLineGetsItsOwnNumber() {
        XCTAssertEqual(numbers("SELECT 1\nFROM t\nWHERE x = 2"), [1, 2, 3])
    }

    func testAnEmptyEditorIsStillOneLine() {
        // The caret sits on line 1 even when there is nothing typed. Numbering it zero, or not at
        // all, would make an empty editor look broken.
        XCTAssertEqual(numbers(""), [1])
    }

    func testATrailingNewlineStartsTheNextLine() {
        // A trailing newline means the caret is on a new line, so the gutter has to show it.
        XCTAssertEqual(numbers("SELECT 1\n"), [1, 2])
    }

    func testABlankLineInTheMiddleIsCounted() {
        XCTAssertEqual(numbers("SELECT 1\n\nFROM t"), [1, 2, 3])
    }

    func testAWrappedLineIsNumberedOnceAndTheNextLineKeepsCounting() throws {
        // The case this exists for. At 400pt a 60-character line wraps; the gutter must still say
        // two lines, and the second must be `2` rather than `3`.
        let sql = String(repeating: "a", count: 200) + "\nsecond"
        let (textView, container) = editor(sql, width: 400)
        let layoutManager = try XCTUnwrap(textView.layoutManager)
        let fragments = layoutManager.lineFragmentRect(forGlyphAt: 0, effectiveRange: nil)
        XCTAssertGreaterThan(fragments.height, 0)

        // Confirm the premise: this really does wrap into more than one fragment.
        var fragmentCount = 0
        layoutManager.enumerateLineFragments(forGlyphRange: layoutManager.glyphRange(
            for: container)) { _, _, _, _, _ in fragmentCount += 1 }
        try XCTSkipUnless(fragmentCount > 2, "the line did not wrap at this width, so this proves nothing")

        let ruler = LineNumberRulerView(textView: textView)
        let numbered = ruler.numberedLines(in: NSRect(x: 0, y: 0, width: 400, height: 4000))
        XCTAssertEqual(numbered.map(\.number), [1, 2],
                       "a wrapped line was numbered per fragment rather than per line")
    }

    func testTheNumbersAreSpreadByLineHeightNotSquashed() {
        // Each number sits on its own fragment, so with no wrapping the y values must strictly
        // increase and be about a line apart. A single shared y would draw all the numbers on top
        // of each other, which a count-only assertion would not catch.
        let (textView, _) = editor("a\nb\nc\nd")
        let ruler = LineNumberRulerView(textView: textView)
        let numbered = ruler.numberedLines(in: NSRect(x: 0, y: 0, width: 400, height: 4000))
        XCTAssertEqual(numbered.count, 4)
        let ys = numbered.map(\.minY)
        XCTAssertEqual(ys, ys.sorted())
        XCTAssertEqual(Set(ys).count, 4, "two lines were given the same y")
        for (a, b) in zip(ys, ys.dropFirst()) {
            XCTAssertGreaterThan(b - a, 5, "lines are stacked on top of each other")
        }
    }

    func testScrollingFarDownKeepsTheOriginalNumbering() {
        // The count starts from the top of the *text*, so a line does not renumber itself just
        // because it scrolled to the top of the view.
        let sql = (1...200).map { "line \($0)" }.joined(separator: "\n")
        let (textView, container) = editor(sql, width: 400)
        let layoutManager = textView.layoutManager!
        layoutManager.ensureLayout(for: container)
        let total = layoutManager.usedRect(for: container).height

        let ruler = LineNumberRulerView(textView: textView)
        // A window onto the bottom of the text.
        let visible = NSRect(x: 0, y: total - 120, width: 400, height: 120)
        let numbered = ruler.numberedLines(in: visible)
        XCTAssertFalse(numbered.isEmpty)
        // The first number in that window must be well past 1, and the numbers must be the plain
        // sequence — counted from the text, not restarted at the window's top.
        let first = numbered.first!.number
        XCTAssertGreaterThan(first, 100, "the count restarted at the visible top instead of the text top")
        XCTAssertEqual(numbered.map(\.number), Array(first...(first + numbered.count - 1)))
    }

    func testTheGutterWidthFollowsTheNumberOfDigits() {
        // Three digits have to fit without the number being clipped, and one digit must not reserve
        // three digits of width — every point here is a point the text does not get.
        let (textView, _) = editor("a")
        let ruler = LineNumberRulerView(textView: textView)
        ruler.update(for: "a")
        let oneDigit = ruler.ruleThickness
        ruler.update(for: (1...1200).map { _ in "x" }.joined(separator: "\n"))
        XCTAssertGreaterThan(ruler.ruleThickness, oneDigit, "the gutter did not widen for four digits")

        // And the width has to actually fit the widest number, or the last digit is clipped.
        var widest: CGFloat = 0
        for n in [1, 12, 120, 1200] {
            let size = ("\(n)" as NSString).size(withAttributes: [.font: ruler.numberFont])
            widest = max(widest, size.width)
        }
        ruler.update(for: (1...1200).map { _ in "x" }.joined(separator: "\n"))
        XCTAssertGreaterThan(ruler.ruleThickness, widest + 8, "the widest number does not fit the gutter")
    }
}
