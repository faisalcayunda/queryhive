import AppKit
import XCTest

@testable import QueryHive

/// The two editor-lane features that are decisions rather than drawing: the find bar's search and
/// the folding ranges.
///
/// Both are checked as logic first. Where a window would be needed to see the result, the test
/// builds the real AppKit view and lays it out offscreen — which is evidence about the view, not
/// about the app putting it on screen.
@MainActor
final class EditorFindAndFoldingTests: XCTestCase {
    // MARK: Find — matching

    func testRangesAreLiteralAndCaseInsensitiveByDefault() {
        // `.` and `*` are searched for as themselves, and case does not matter until asked. A pasted
        // fragment of SQL is full of both, so a regex here would be the wrong default.
        let text = "SELECT a.* FROM t WHERE a.x = 1 -- a.*" as NSString
        let matches = FindReplace.ranges(in: text, needle: "A.*")
        XCTAssertEqual(matches.count, 2)
        XCTAssertEqual(text.substring(with: matches[0]), "a.*")
    }

    func testCaseSensitiveOptionNarrowsTheMatches() {
        let text = "alpha ALPHA Alpha" as NSString
        XCTAssertEqual(FindReplace.ranges(in: text, needle: "alpha").count, 3)
        XCTAssertEqual(FindReplace.ranges(in: text, needle: "alpha", options: .caseSensitive).count, 1)
    }

    func testWholeWordExcludesEmbeddedMatches() {
        // `cat_1` and `concat` contain the letters but not the word, and `_` is a word character.
        let text = "cat category cat_1 concat cat" as NSString
        let matches = FindReplace.ranges(in: text, needle: "cat", options: .wholeWord)
        XCTAssertEqual(matches.count, 2)
        XCTAssertEqual(matches.map { text.substring(with: $0) }, ["cat", "cat"])
    }

    func testAnEmptyNeedleFindsNothingRatherThanEverything() throws {
        XCTAssertTrue(FindReplace.ranges(in: "SELECT 1" as NSString, needle: "").isEmpty)
    }

    // MARK: Find — walking

    func testNextWrapsPastTheEnd() {
        let text = "a b a b a" as NSString   // matches at 0, 4, 8
        XCTAssertEqual(FindReplace.next(in: text, needle: "a", from: 1)?.location, 4)
        XCTAssertEqual(FindReplace.next(in: text, needle: "a", from: 5)?.location, 8)
        // Past the last match the search starts again at the top, the way every find bar does.
        XCTAssertEqual(FindReplace.next(in: text, needle: "a", from: 9)?.location, 0)
        XCTAssertNil(FindReplace.next(in: text, needle: "z", from: 0))
    }

    func testPreviousWrapsPastTheStart() {
        let text = "a b a b a" as NSString
        XCTAssertEqual(FindReplace.previous(in: text, needle: "a", from: 4)?.location, 0)
        XCTAssertEqual(FindReplace.previous(in: text, needle: "a", from: 0)?.location, 8)
    }

    func testCurrentIndexPrefersTheMatchUnderTheCaret() {
        let matches = [NSRange(location: 0, length: 1),
                       NSRange(location: 4, length: 1),
                       NSRange(location: 8, length: 1)]
        XCTAssertEqual(FindReplace.currentIndex(of: matches, selection: NSRange(location: 4, length: 0)), 1)
        // Between two matches the caret belongs to the next one.
        XCTAssertEqual(FindReplace.currentIndex(of: matches, selection: NSRange(location: 5, length: 0)), 2)
        // Past the last match it wraps to the first, matching `next`.
        XCTAssertEqual(FindReplace.currentIndex(of: matches, selection: NSRange(location: 20, length: 0)), 0)
        XCTAssertNil(FindReplace.currentIndex(of: [], selection: NSRange(location: 0, length: 0)))
    }

    // MARK: Find — replacing

    func testReplaceAllWithALongerReplacement() {
        let text = "id, id2, id3" as NSString
        let result = FindReplace.replaceAll(in: text, needle: "id", with: "identifier")
        XCTAssertEqual(result.text, "identifier, identifier2, identifier3")
        XCTAssertEqual(result.count, 3)
    }

    func testReplaceAllWithAShorterReplacementDoesNotEatTheTail() {
        // The bug a forward loop produces: the second match's range moves once the first shrinks.
        let result = FindReplace.replaceAll(in: "aaaa" as NSString, needle: "aa", with: "b")
        XCTAssertEqual(result.text, "bb")
        XCTAssertEqual(result.count, 2)
    }

    func testReplaceOneLeavesEveryOtherMatchAlone() {
        let text = "x = 1; x = 2" as NSString
        XCTAssertEqual(FindReplace.replace(NSRange(location: 0, length: 1), in: text, with: "y"),
                       "y = 1; x = 2")
    }

    func testSummaryCountsFromOneAndSaysNoMatches() {
        XCTAssertEqual(FindReplace.summary(count: 17, index: 2), "3 of 17")
        XCTAssertEqual(FindReplace.summary(count: 17, index: nil), "1 of 17")
        XCTAssertEqual(FindReplace.summary(count: 0, index: nil), "No matches")
    }

    // MARK: Folding — regions

    func testAStatementSpanningLinesIsFoldable() {
        let sql = "SELECT 1;\nSELECT a,\n  b\nFROM t;\nSELECT 2;"
        let statements = SQLFolding.regions(in: sql).filter { $0.kind == .statement }
        XCTAssertEqual(statements.count, 1, "only the middle statement has more than one line")
        let region = statements[0]
        XCTAssertEqual(region.headerLine, 1)
        XCTAssertEqual(region.lastLine, 3)
        XCTAssertEqual((sql as NSString).substring(with: region.body), "  b\nFROM t;\n",
                       "the hidden body must be whole lines, ending with the last line's newline")
    }

    func testASingleLineStatementIsNotFoldable() {
        XCTAssertTrue(SQLFolding.regions(in: "SELECT 1; SELECT 2;").isEmpty)
        XCTAssertTrue(SQLFolding.regions(in: "SELECT 1").isEmpty)
    }

    func testTwoMultiLineStatementsFoldSeparately() {
        let sql = "SELECT\n  a\nFROM t;\nUPDATE t\nSET a = 1\nWHERE b = 2;"
        let statements = SQLFolding.regions(in: sql).filter { $0.kind == .statement }
        XCTAssertEqual(statements.map(\.headerLine), [0, 3])
        XCTAssertEqual(statements.map(\.lastLine), [2, 5])
    }

    func testIdenticalStatementsAreFoundSeparately() {
        // The reuse-search has to move forward, or the second statement would be located at the
        // first one's offset forever.
        let sql = "SELECT\n  1;\nSELECT\n  1;"
        let ranges = SQLFolding.statementRanges(in: sql)
        XCTAssertEqual(ranges.count, 2)
        XCTAssertNotEqual(ranges[0].location, ranges[1].location)
    }

    func testASemicolonInsideAStringDoesNotSplitAStatement() {
        // The scanner owns this rule; folding must not re-derive it differently.
        let sql = "SELECT 'a;b',\n  c\nFROM t;"
        let statements = SQLFolding.regions(in: sql).filter { $0.kind == .statement }
        XCTAssertEqual(statements.count, 1)
        XCTAssertEqual(statements[0].lastLine, 2)
    }

    func testACTEbodyOnItsOwnLineIsFoldable() {
        // The `WITH` sits on line 0 and the CTE's `(` on line 1, so the statement fold (line 0) and
        // the CTE fold (line 1) are separate regions with separate markers.
        let sql = """
        WITH
            recent AS (
                SELECT id, at
                FROM events
                WHERE at > now() - interval '7' day
            )
        SELECT * FROM recent;
        """
        let regions = SQLFolding.regions(in: sql)
        let ctes = regions.filter { $0.kind == .cte }
        XCTAssertEqual(ctes.count, 1)
        XCTAssertEqual(ctes[0].headerLine, 1)
        XCTAssertEqual(ctes[0].lastLine, 5)
        XCTAssertTrue(regions.contains { $0.kind == .statement && $0.headerLine == 0 && $0.lastLine == 6 })
    }

    func testACTEOnTheStatementLineDoesNotDuplicateTheMarker() {
        // `WITH recent AS (` puts the statement and the CTE on one line. Only one fold can own that
        // header offset, and the statement's is the superset, so the CTE is not a second region.
        let sql = "WITH recent AS (\n    SELECT 1\n)\nSELECT * FROM recent;"
        let regions = SQLFolding.regions(in: sql)
        XCTAssertEqual(regions.filter { $0.headerLine == 0 }.count, 1)
        XCTAssertFalse(regions.contains { $0.kind == .cte })
    }

    func testACTASIsNotAFoldableCTE() {
        // `AS (` without a `WITH` in the statement is a create-table-as, not a CTE.
        let sql = "CREATE TABLE t AS (\n  SELECT 1\n);"
        XCTAssertFalse(SQLFolding.regions(in: sql).contains { $0.kind == .cte })
    }

    func testACTESpanningOneLineIsNotFoldable() {
        let sql = "WITH one AS (SELECT 1)\nSELECT * FROM one;"
        XCTAssertFalse(SQLFolding.regions(in: sql).contains { $0.kind == .cte })
    }

    func testAKeywordOrBracketInAStringDoesNotCreateAFold() {
        // `AS (` inside a literal is content, and the CTE matcher has to step over it.
        let sql = "SELECT 'x AS (' ,\n  'y'\nFROM t;"
        XCTAssertFalse(SQLFolding.regions(in: sql).contains { $0.kind == .cte })
    }

    // MARK: Folding — edits

    func testShiftMovesOffsetsAfterAnInsertionAndKeepsOnesBeforeIt() {
        let shifted = SQLFolding.shift([0, 4], from: "abcdef" as NSString, to: "abcXdef" as NSString)
        XCTAssertEqual(shifted, [0, 5])
    }

    func testShiftDropsAnOffsetInsideTheChangedSpan() {
        // The header itself was replaced, so its fold cannot be trusted: it opens.
        let shifted = SQLFolding.shift([0, 4], from: "abcdef" as NSString, to: "abcZZf" as NSString)
        XCTAssertEqual(shifted, [0])
    }

    func testShiftIsANoOpWhenTheTextIsUnchanged() {
        XCTAssertEqual(SQLFolding.shift([3], from: "abc" as NSString, to: "abc" as NSString), [3])
    }

    func testLineStartsAndLookup() {
        let text = "a\nbb\n\nc" as NSString
        XCTAssertEqual(SQLFolding.lineStarts(in: text), [0, 2, 5, 6])
        XCTAssertEqual(SQLFolding.line(containing: 0, lineStarts: [0, 2, 5, 6]), 0)
        XCTAssertEqual(SQLFolding.line(containing: 4, lineStarts: [0, 2, 5, 6]), 1)
        XCTAssertEqual(SQLFolding.line(containing: 6, lineStarts: [0, 2, 5, 6]), 3)
    }

    // MARK: Folding — the AppKit hiding

    /// The hiding itself: the text view's characters are untouched, and the layout really does get
    /// shorter. Height is the evidence, because that is what the mechanism produces.
    func testApplyingAFoldCollapsesTheLayoutAndLeavesTheTextAlone() throws {
        let sql = "SELECT a,\n  b,\n  c\nFROM t;"
        let (textView, layoutManager, container) = editor(sql)
        let expanded = layoutManager.usedRect(for: container).height

        let statement = try XCTUnwrap(SQLFolding.regions(in: sql).first { $0.kind == .statement })
        SQLFoldStyler.apply([statement], to: textView)
        layoutManager.ensureLayout(for: container)
        let folded = layoutManager.usedRect(for: container).height

        XCTAssertLessThan(folded, expanded - 10, "the hidden lines did not collapse")
        XCTAssertEqual(textView.string, sql, "folding must not change a character")
    }

    func testRemovingTheFoldRestoresTheHeight() throws {
        let sql = "SELECT a,\n  b,\n  c\nFROM t;"
        let (textView, layoutManager, container) = editor(sql)
        let expanded = layoutManager.usedRect(for: container).height

        let statement = try XCTUnwrap(SQLFolding.regions(in: sql).first { $0.kind == .statement })
        SQLFoldStyler.apply([statement], to: textView)
        layoutManager.ensureLayout(for: container)
        SQLFoldStyler.apply([], to: textView)
        layoutManager.ensureLayout(for: container)

        XCTAssertEqual(layoutManager.usedRect(for: container).height, expanded, accuracy: 1,
                       "unfolding left the paragraph tall or short")
    }

    /// The find bar lays out and clears the second row only when Replace is on, because the editor
    /// gave it no fixed height.
    func testTheFindBarGrowsForTheReplaceRow() {
        let bar = SQLFindBar()
        bar.setQuery("penerima")
        let collapsed = bar.fittingSize.height
        bar.isReplacing = true
        XCTAssertGreaterThan(bar.fittingSize.height, collapsed,
                             "the replace row did not add height to the bar")
    }

    // MARK: Renders

    /// Draws the find bar offscreen and leaves a PNG behind. What it proves: the bar lays out at a
    /// real size and draws marks rather than a blank strip. What it does not prove: that the app
    /// wires it to a tab and shows it.
    func testTheFindBarRenders() throws {
        let bar = SQLFindBar()
        bar.setQuery("penerima")
        bar.setResult(count: 12, current: 4)
        bar.frame = NSRect(x: 0, y: 0, width: 520, height: bar.fittingSize.height)
        try render(bar, named: "find-bar.png")

        bar.isReplacing = true
        bar.frame = NSRect(x: 0, y: 0, width: 520, height: bar.fittingSize.height)
        try render(bar, named: "find-bar-replace.png")

        // The regex toggle on and a pattern that will not compile: the state the bar shows instead
        // of a crash or a fake "No matches".
        bar.isReplacing = false
        bar.setQuery("(")
        bar.setRegularExpression(true)
        bar.setPatternError("Invalid pattern: the value is invalid")
        bar.frame = NSRect(x: 0, y: 0, width: 520, height: bar.fittingSize.height)
        try render(bar, named: "find-bar-invalid.png")
    }

    /// Draws the editor with a statement folded. The PNG is the deliverable; the assertions are a
    /// smoke test that the layout came out at the size asked for and something was drawn.
    func testAFoldedEditorRenders() throws {
        let sql = "SELECT kode_wilayah,\n       nama,\n       jumlah_jiwa\nFROM penerima\nWHERE aktif = true;"
        let (textView, layoutManager, container) = editor(sql)
        let statement = try XCTUnwrap(SQLFolding.regions(in: sql).first { $0.kind == .statement })
        SQLFoldStyler.apply([statement], to: textView)
        layoutManager.ensureLayout(for: container)

        try render(textView, named: "editor-folded.png")
    }

    // MARK: Editor wiring

    /// Drives the coordinator's own find path: seed the query from a selection, walk to the match,
    /// and hand the selection back. This is the code ⌘F reaches, minus the key event.
    func testShowingFindSeedsFromTheSelectionAndSelectsTheMatch() throws {
        let sql = "SELECT alpha,\n  beta\nFROM t;"
        let (textView, _, _) = editor(sql, asSQLTextView: true)
        let editor = makeEditor(sql)
        let coordinator = editor.makeCoordinator()
        coordinator.textView = textView as? SQLTextView
        textView.delegate = coordinator
        let bar = SQLFindBar()
        coordinator.findBar = bar
        coordinator.wire(bar)

        // The user selected a word and pressed ⌘F.
        textView.setSelectedRange(NSRange(location: 7, length: 5))
        coordinator.showFind(replacing: false)

        XCTAssertEqual(bar.query, "alpha")
        XCTAssertFalse(bar.isHidden)
        XCTAssertEqual(textView.selectedRange(), NSRange(location: 7, length: 5))
    }

    /// Drives the real folding path: recolour computes the regions, a toggle folds one, and the
    /// text is unchanged while the layout shrinks.
    func testTogglingAFoldThroughTheCoordinatorCollapsesIt() throws {
        let sql = "SELECT 1;\nSELECT a,\n  b\nFROM t;\nSELECT 2;"
        let (textView, layoutManager, container) = editor(sql, asSQLTextView: true)
        let editor = makeEditor(sql)
        let coordinator = editor.makeCoordinator()
        coordinator.textView = textView as? SQLTextView
        textView.delegate = coordinator
        coordinator.recolour()
        layoutManager.ensureLayout(for: container)

        let region = try XCTUnwrap(SQLFolding.regions(in: sql).first { $0.kind == .statement })
        let expanded = layoutManager.usedRect(for: container).height
        coordinator.toggleFold(headerOffset: region.header)
        layoutManager.ensureLayout(for: container)

        XCTAssertLessThan(layoutManager.usedRect(for: container).height, expanded - 10)
        XCTAssertEqual(textView.string, sql)
    }

    // MARK: Helpers

    private func makeEditor(_ sql: String) -> SQLEditor {
        SQLEditor(text: .constant(sql),
                  focused: .constant(false),
                  caret: .constant(0),
                  selection: .constant(NSRange(location: 0, length: 0)),
                  completion: EditorCompletion(),
                  candidates: { _, _ in [] },
                  layout: .standard, onRunStatement: nil)
    }

    private func editor(_ sql: String, width: CGFloat = 420, asSQLTextView: Bool = false)
        -> (NSTextView, NSLayoutManager, NSTextContainer) {
        let storage = NSTextStorage(string: sql)
        let layoutManager = NSLayoutManager()
        storage.addLayoutManager(layoutManager)
        let container = NSTextContainer(size: NSSize(width: width, height: CGFloat.greatestFiniteMagnitude))
        container.widthTracksTextView = false
        layoutManager.addTextContainer(container)
        let frame = NSRect(x: 0, y: 0, width: width, height: 400)
        let textView: NSTextView = asSQLTextView
            ? SQLTextView(frame: frame, textContainer: container)
            : NSTextView(frame: frame, textContainer: container)
        textView.font = .monospacedSystemFont(ofSize: 13, weight: .regular)
        textView.drawsBackground = true
        textView.backgroundColor = .white
        layoutManager.ensureLayout(for: container)
        return (textView, layoutManager, container)
    }

    /// Draws an AppKit view into a bitmap and writes it out, the same way `SidebarRenderTests`
    /// draws a SwiftUI view through `NSHostingView`. `QH_RENDER_DIR` decides where the PNG goes.
    private func render(_ view: NSView, named name: String) throws {
        if view.fittingSize.height > 0, view.frame.height == 0 {
            view.frame = NSRect(x: 0, y: 0, width: 520, height: view.fittingSize.height)
        }
        view.layoutSubtreeIfNeeded()

        let rep = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds),
                                "\(name): no bitmap to draw into")
        view.cacheDisplay(in: view.bounds, to: rep)
        let data = try XCTUnwrap(rep.representation(using: .png, properties: [:]),
                                 "\(name) could not be encoded as a PNG")

        // A deliberately weak check: one colour means nothing was drawn. It cannot tell a correct
        // bar from a wrong one, only that something is there.
        var colours = Set<Int>()
        for x in stride(from: 0, to: rep.pixelsWide, by: 7) {
            for y in stride(from: 0, to: rep.pixelsHigh, by: 7) {
                if let colour = rep.colorAt(x: x, y: y) { colours.insert(colour.hash) }
            }
        }
        XCTAssertGreaterThan(colours.count, 1, "\(name) drew one flat colour")

        let directory = Self.outputDirectory
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let path = directory.appendingPathComponent(name)
        try data.write(to: path)
        print("rendered \(name) to \(path.path)")
    }

    private static var outputDirectory: URL {
        if let asked = ProcessInfo.processInfo.environment["QH_RENDER_DIR"], !asked.isEmpty {
            return URL(fileURLWithPath: asked, isDirectory: true)
        }
        return URL(fileURLWithPath: NSTemporaryDirectory(), isDirectory: true)
            .appendingPathComponent("queryhive-renders", isDirectory: true)
    }

    // MARK: Size limit

    func testAnOversizedDocumentIsNotFoldedAtAll() {
        // The cap, and the reason it exists: this scanner locates each statement by re-running the
        // statement scanner and searching for its text, which is not one pass yet, so an unbounded
        // document would pay a superlinear cost on every keystroke. Over the limit the editor gets
        // no regions — no fold marks and no cost — rather than a fold whose body is cut short.
        let big = String(repeating: "SELECT 1;\n", count: SQLFolding.foldingSizeLimit / 10 + 1)
        XCTAssertGreaterThan((big as NSString).length, SQLFolding.foldingSizeLimit)
        XCTAssertTrue(SQLFolding.regions(in: big).isEmpty)

        // Just under it still folds, so this is a limit and not a blanket refusal.
        let small = "SELECT a,\n       b\nFROM t;"
        XCTAssertFalse(SQLFolding.regions(in: small).isEmpty)
    }
}
