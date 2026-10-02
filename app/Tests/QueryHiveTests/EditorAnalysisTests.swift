import AppKit
import XCTest

@testable import QueryHive

/// G7 for commit A: the tree-sitter analysis behind the FFI, on a headless `SQLTextView`
/// that is never wired to `SQLEditor`. No behaviour changes: the regex path paints
/// nothing here, and every colour lands as a temporary attribute.
@MainActor
final class EditorAnalysisTests: XCTestCase {
    private var analysis: EditorAnalysis!

    private func open(_ sql: String) throws {
        analysis = try EditorAnalysis(text: sql)
    }

    /// The TextKit 1 stack `makeNSView` builds, without a SwiftUI host and without the
    /// coordinator: no regex colours, no folds, no delegate.
    private func view(_ sql: String) -> SQLTextView {
        let storage = NSTextStorage(string: sql)
        let layoutManager = NSLayoutManager()
        storage.addLayoutManager(layoutManager)
        let container = NSTextContainer(size: NSSize(width: 420,
                                                       height: CGFloat.greatestFiniteMagnitude))
        container.widthTracksTextView = false
        layoutManager.addTextContainer(container)
        let textView = SQLTextView(frame: NSRect(x: 0, y: 0, width: 420, height: 400),
                                   textContainer: container)
        textView.font = NSFont.monospacedSystemFont(ofSize: 12.5, weight: .regular)
        return textView
    }

    private func colors() -> [EditorColorClass: NSColor] {
        [.keyword: .systemPurple, .string: .systemGreen, .comment: .systemGray,
         .number: .systemOrange, .parameter: .systemPink, .function: .systemBlue,
         .quotedIdentifier: .systemYellow, .literal: .systemRed, .punctuation: .systemGray]
    }

    private func fonts() -> (regular: NSFont, italic: NSFont) {
        let regular = NSFont.monospacedSystemFont(ofSize: 12.5, weight: .regular)
        let italic = NSFont(descriptor: regular.fontDescriptor
            .withSymbolicTraits(.italic), size: 12.5) ?? regular
        return (regular, italic)
    }

    private func fullPaint() throws -> EditorPaintData {
        try analysis.paint(window: NSRange(location: 0, length: analysis.length),
                           budget: analysis.length)
    }

    // MARK: Paint through the FFI

    func testBlueprintExamplePaintsExactly() throws {
        try open("select :a -- c")
        let paint = try fullPaint()
        XCTAssertEqual(paint.revision, 1)
        XCTAssertEqual(paint.length, 14)
        XCTAssertFalse(paint.inactive)
        XCTAssertFalse(paint.moreInWindow)
        XCTAssertFalse(paint.dirtyElsewhere)
        XCTAssertEqual(paint.ranges, [NSRange(location: 0, length: 14)])
        XCTAssertEqual(paint.runs.map { [$0.range.location, $0.range.length] },
                       [[0, 6], [7, 2], [10, 4]])
        XCTAssertEqual(paint.runs.map { $0.colorClass }, [.keyword, .parameter, .comment])
    }

    func testTypingNarrowsTheRepaint() throws {
        try open("select :a -- c")
        try analysis.markApplied(fullPaint())
        XCTAssertEqual(try analysis.replace(range: NSRange(location: 14, length: 0), with: "x"), 2)
        let paint = try fullPaint()
        XCTAssertEqual(paint.ranges, [NSRange(location: 10, length: 5)])
        XCTAssertEqual(paint.runs.map { $0.colorClass }, [.comment])
        XCTAssertEqual(paint.fonts.map { [$0.range.location, $0.range.length, $0.italic ? 1 : 0] },
                       [[14, 1, 1]])
    }

    func testStalePacketsAreNotApplied() throws {
        try open("select 1")
        let old = try fullPaint()
        _ = try analysis.replace(range: NSRange(location: 8, length: 0), with: " ")
        XCTAssertEqual(analysis.revision, 2)
        XCTAssertFalse(EditorAnalysis.shouldApply(revision: 1, paint: old, hasMarkedText: false,
                                                  storageLength: analysis.length))
        XCTAssertFalse(EditorAnalysis.shouldApply(revision: 2, paint: try fullPaint(),
                                                  hasMarkedText: true, storageLength: 9))
    }

    func testSplittingASurrogateThrowsAndWideningFixesIt() throws {
        try open("a😀b")
        XCTAssertThrowsError(try analysis.replace(range: NSRange(location: 1, length: 1),
                                                  with: "")) { error in
            XCTAssertEqual(error as? EditorAnalysisError, .splitsCharacter)
        }
        let widened = EditorAnalysis.widened(NSRange(location: 1, length: 1), in: "a😀b" as NSString)
        XCTAssertEqual(widened, NSRange(location: 1, length: 2))
        _ = try analysis.replace(range: widened, with: "o")
        XCTAssertEqual(analysis.length, 3)
    }

    func testCRLFStatementsSplitAndCount() throws {
        try open("select 1;\r\nselect 2")
        XCTAssertEqual(analysis.lineCount, 2)
        let outline = try analysis.outline()
        XCTAssertEqual(outline.statements.count, 2)
        XCTAssertEqual(outline.statements[0], NSRange(location: 0, length: 8))
        XCTAssertEqual(outline.statements[1], NSRange(location: 9, length: 10))
        XCTAssertFalse(try fullPaint().ranges.isEmpty)
    }

    // MARK: Outlines, drift, undo

    func testOutlineHoldsStatementsAndIssues() throws {
        try open("select 1;\nselect 'oops;\n")
        let outline = try analysis.outline()
        XCTAssertEqual(outline.revision, 1)
        XCTAssertEqual(outline.statements,
                       [NSRange(location: 0, length: 8), NSRange(location: 9, length: 15)])
        XCTAssertTrue(outline.issues.contains { $0.kind == .unclosedQuote })
        XCTAssertTrue(outline.folds.allSatisfy { $0.lastLine > $0.headerLine })
    }

    func testDriftAgainstTheView() throws {
        try open("a\nb\nc")
        XCTAssertEqual(analysis.lineCount, 3)
        let textView = view("a\nb\nc")
        XCTAssertEqual(analysis.length, textView.string.utf16.count)
        textView.string = "a\nb\nc\nd"
        XCTAssertNotEqual(analysis.length, textView.string.utf16.count)
    }

    func testUndoRedoRoundTripIsExact() throws {
        // AppKit undo funnels through the same `replace` typing does (commit B wires the
        // hook); commit A proves the round trip is exact. A bare view vends no undo
        // manager, so the inverse goes through the same call undo would make.
        let sql = "select a from t"
        try open(sql)
        let textView = view(sql)
        let storage = try XCTUnwrap(textView.textStorage)
        let before = try analysis.drainForTesting().paint
        textView.insertText("x", replacementRange: NSRange(location: 7, length: 0))
        _ = try analysis.replace(range: NSRange(location: 7, length: 0), with: "x")
        let typed = try analysis.drainForTesting().paint
        XCTAssertNotEqual(typed, before)
        storage.replaceCharacters(in: NSRange(location: 7, length: 1), with: "")
        _ = try analysis.replace(range: NSRange(location: 7, length: 1), with: "")
        XCTAssertEqual(textView.string as String, sql)
        let undone = try analysis.drainForTesting().paint
        XCTAssertEqual(undone.revision, 3, "undo is a new revision, not time travel")
        XCTAssertEqual(undone.ranges, before.ranges)
        XCTAssertEqual(undone.runs, before.runs)
        textView.insertText("x", replacementRange: NSRange(location: 7, length: 0))
        _ = try analysis.replace(range: NSRange(location: 7, length: 0), with: "x")
        let redone = try analysis.drainForTesting().paint
        XCTAssertEqual(redone.ranges, typed.ranges)
        XCTAssertEqual(redone.runs, typed.runs)
    }

    // MARK: Applying to the view

    func testApplyUsesOnlyTemporaryAttributes() throws {
        let sql = "select :a -- c"
        try open(sql)
        let textView = view(sql)
        let storage = try XCTUnwrap(textView.textStorage)
        let layoutManager = try XCTUnwrap(textView.layoutManager)
        // A find highlight, as the find bar leaves it: temporary `.backgroundColor`.
        let match = NSRange(location: 7, length: 2)
        layoutManager.addTemporaryAttribute(.backgroundColor, value: NSColor.systemYellow,
                                            forCharacterRange: match)
        let paint = try analysis.drainForTesting().paint
        try analysis.markApplied(paint)
        let (regular, italic) = fonts()
        analysis.apply(paint, to: textView, colors: colors(),
                       regularFont: regular, italicFont: italic)
        var storageHasColor = false
        storage.enumerateAttribute(.foregroundColor,
                                   in: NSRange(location: 0, length: storage.length)) { value, _, stop in
            if value != nil { storageHasColor = true; stop.pointee = true }
        }
        XCTAssertFalse(storageHasColor, "colours never reach storage")
        XCTAssertEqual(layoutManager.temporaryAttribute(.backgroundColor, atCharacterIndex: 7,
                                                        effectiveRange: nil) as? NSColor,
                       .systemYellow, "the find highlight survives the paint")
        XCTAssertEqual(layoutManager.temporaryAttribute(.foregroundColor, atCharacterIndex: 0,
                                                        effectiveRange: nil) as? NSColor,
                       .systemPurple, "the keyword run lands as a temporary attribute")
        let commentFont = storage.attribute(.font, at: 10,
                                                  effectiveRange: nil) as? NSFont
        XCTAssertTrue(commentFont?.fontDescriptor.symbolicTraits.contains(.italic) ?? false,
                      "the comment range keeps its italic in storage")
        let codeFont = storage.attribute(.font, at: 0, effectiveRange: nil) as? NSFont
        XCTAssertFalse(codeFont?.fontDescriptor.symbolicTraits.contains(.italic) ?? true,
                       "code keeps the upright font in storage")
        XCTAssertEqual(codeFont?.pointSize, 12.5)
    }

    func testMarkAppliedClearsAndMarkDirtyReturns() throws {
        try open("select 1")
        XCTAssertFalse(try fullPaint().ranges.isEmpty)
        try analysis.markApplied(fullPaint())
        XCTAssertTrue(try fullPaint().ranges.isEmpty)
        try analysis.markDirty(NSRange(location: 0, length: 3))
        XCTAssertEqual(try fullPaint().ranges, [NSRange(location: 0, length: 6)])
    }

    func testCoalescedPaintAnswersEveryWaiter() throws {
        try open("select 1; select 2;")
        let window = NSRange(location: 0, length: analysis.length)
        let first = expectation(description: "first waiter")
        let second = expectation(description: "second waiter")
        var revisions: [UInt64] = []
        analysis.requestPaint(window: window, budget: 1024) { result in
            if case .success(let paint) = result { revisions.append(paint.revision) }
            first.fulfill()
        }
        analysis.requestPaint(window: window, budget: 1024) { result in
            if case .success(let paint) = result { revisions.append(paint.revision) }
            second.fulfill()
        }
        wait(for: [first, second], timeout: 10)
        XCTAssertEqual(revisions, [1, 1])
    }

    func testStatementRangesAndCeiling() throws {
        XCTAssertEqual(try editorStatementRanges("select 1; select 2;"),
                       [NSRange(location: 0, length: 8), NSRange(location: 9, length: 9)])
        XCTAssertEqual(try editorStatementRanges("-- only a comment"), [])
        XCTAssertEqual(try editorCeiling(), 2_000_000)
    }

    func testDrainIsDeterministic() throws {
        try open("with c as (select 1) select * from c;\nselect 'x';\n")
        let first = try analysis.drainForTesting()
        let second = try analysis.drainForTesting()
        XCTAssertEqual(first.paint, second.paint)
        XCTAssertEqual(first.outline, second.outline)
        XCTAssertEqual(first.outline.statements.count, 2)
    }
}
