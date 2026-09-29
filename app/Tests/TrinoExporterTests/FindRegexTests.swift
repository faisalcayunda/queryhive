import AppKit
import XCTest

@testable import QueryHive

/// The find bar's regular-expression mode.
///
/// The literal path is the default and stays literal; this is the opt-in beside it. Two things are
/// pinned here that are easy to get wrong and silent when wrong: an invalid pattern is a state the
/// bar shows rather than a crash or an empty result dressed up as "No matches", and replace-all in
/// regex mode treats the replacement as a template (`$1`), which is the one thing a regex find is
/// for.
@MainActor
final class FindRegexTests: XCTestCase {
    // MARK: Matching

    func testLiteralSearchStaysLiteral() {
        // The default did not move: `a.c` is still those three characters, not a pattern.
        let text = "abc a.c" as NSString
        XCTAssertEqual(FindReplace.ranges(in: text, needle: "a.c").count, 1)
    }

    func testRegexMatchesAPatternRatherThanCharacters() {
        let text = "abc ac abbc" as NSString
        let matches = FindReplace.ranges(in: text, needle: "ab+c", options: .regularExpression)
        XCTAssertEqual(matches.count, 2)
        XCTAssertEqual(text.substring(with: matches[0]), "abc")
        XCTAssertEqual(text.substring(with: matches[1]), "abbc")
    }

    func testRegexIsCaseInsensitiveUnlessAsked() {
        let text = "Foo foo" as NSString
        XCTAssertEqual(FindReplace.ranges(in: text, needle: "foo", options: .regularExpression).count, 2)
        XCTAssertEqual(FindReplace.ranges(in: text, needle: "foo",
                                          options: [.regularExpression, .caseSensitive]).count, 1)
    }

    func testRegexWholeWordUsesBoundaries() {
        // `cat` in `concat` and `cat_1` is not the word, and `_` is a word character.
        let text = "cat concat cat_1 cat" as NSString
        let matches = FindReplace.ranges(in: text, needle: "cat",
                                         options: [.regularExpression, .wholeWord])
        XCTAssertEqual(matches.count, 2)
    }

    func testRegexNextWrapsPastTheEnd() {
        let text = "x1 y2 z3" as NSString
        XCTAssertEqual(FindReplace.next(in: text, needle: "\\d", from: 3,
                                        options: .regularExpression)?.location, 4)
        XCTAssertEqual(FindReplace.next(in: text, needle: "\\d", from: 8,
                                        options: .regularExpression)?.location, 1)
    }

    // MARK: Invalid patterns

    func testAnInvalidPatternIsAStateNotACrashOrAnEmptyResult() {
        let text = "SELECT 1" as NSString
        XCTAssertTrue(FindReplace.ranges(in: text, needle: "(", options: .regularExpression).isEmpty)

        let error = FindReplace.patternError(needle: "(", options: .regularExpression)
        XCTAssertNotNil(error)
        XCTAssertTrue(error?.hasPrefix("Invalid pattern") == true, error ?? "no error")
    }

    func testAValidPatternHasNoErrorAndAPatternWithoutTheModeIsNeverAnError() {
        XCTAssertNil(FindReplace.patternError(needle: "a.c", options: .regularExpression))
        // `(` is only invalid as a regex; as literal text it is fine.
        XCTAssertNil(FindReplace.patternError(needle: "(", options: []))
        XCTAssertNil(FindReplace.patternError(needle: "", options: .regularExpression))
    }

    // MARK: Replacing

    func testRegexReplaceAllUsesCaptureTemplates() {
        let text = "a1 b2 c3" as NSString
        let result = FindReplace.replaceAll(in: text, needle: "([a-z])(\\d)", with: "$2$1",
                                            options: .regularExpression)
        XCTAssertEqual(result.text, "1a 2b 3c")
        XCTAssertEqual(result.count, 3)
    }

    func testLiteralReplaceAllKeepsTheDollarLiteral() {
        // `$1` is a template only in regex mode; a plain search must not start substituting.
        let result = FindReplace.replaceAll(in: "x x" as NSString, needle: "x", with: "$1")
        XCTAssertEqual(result.text, "$1 $1")
    }

    func testAnInvalidPatternReplacesNothing() {
        let result = FindReplace.replaceAll(in: "a(b" as NSString, needle: "(", with: "",
                                            options: .regularExpression)
        XCTAssertEqual(result.text, "a(b")
        XCTAssertEqual(result.count, 0)
    }

    // MARK: The bar

    func testTheFindBarShowsAnInvalidPatternThenClearsIt() {
        let bar = SQLFindBar()
        bar.setQuery("(")
        bar.setPatternError("Invalid pattern: the value is invalid")
        XCTAssertEqual(bar.patternError, "Invalid pattern: the value is invalid")

        // The next successful search clears it, so a stale error cannot sit beside a real count.
        bar.setResult(count: 2, current: 0)
        XCTAssertNil(bar.patternError)
        XCTAssertEqual(bar.patternError, nil)
    }

    func testTheFindBarTurnsTheRegexOptionOn() {
        let bar = SQLFindBar()
        XCTAssertFalse(bar.options.contains(.regularExpression))
        bar.setRegularExpression(true)
        XCTAssertTrue(bar.options.contains(.regularExpression))
        bar.setRegularExpression(false)
        XCTAssertFalse(bar.options.contains(.regularExpression))
    }

    /// Drives the coordinator's own find path: a regex query walks to the pattern's match and hands
    /// the selection back. This is the code the `.` toggle reaches, minus the button press.
    func testTheEditorCanFindByRegexThroughTheCoordinator() throws {
        let sql = "SELECT alpha,\n  beta\nFROM t;"
        let (textView, _, _) = editor(sql)
        let coordinator = SQLFindBarTestsEditor.makeCoordinator(for: sql)
        coordinator.textView = textView as? SQLTextView
        textView.delegate = coordinator
        let bar = SQLFindBar()
        coordinator.findBar = bar
        coordinator.wire(bar)

        bar.setQuery("a\\w+")
        bar.setRegularExpression(true)
        coordinator.showFind(replacing: false)

        XCTAssertEqual(textView.selectedRange(), NSRange(location: 7, length: 5), "alpha")
    }

    // MARK: Helpers

    private func editor(_ sql: String) -> (NSTextView, NSLayoutManager, NSTextContainer) {
        let storage = NSTextStorage(string: sql)
        let layoutManager = NSLayoutManager()
        storage.addLayoutManager(layoutManager)
        let container = NSTextContainer(size: NSSize(width: 420, height: CGFloat.greatestFiniteMagnitude))
        container.widthTracksTextView = false
        layoutManager.addTextContainer(container)
        let frame = NSRect(x: 0, y: 0, width: 420, height: 400)
        let textView = SQLTextView(frame: frame, textContainer: container)
        textView.font = NSFont.monospacedSystemFont(ofSize: 13, weight: .regular)
        layoutManager.ensureLayout(for: container)
        return (textView, layoutManager, container)
    }
}

/// Builds a real `SQLEditor` coordinator without a SwiftUI host, the same way
/// `EditorFindAndFoldingTests` does for its wiring test.
private enum SQLFindBarTestsEditor {
    @MainActor
    static func makeCoordinator(for sql: String) -> SQLEditor.Coordinator {
        let editor = SQLEditor(text: .constant(sql),
                               focused: .constant(false),
                               caret: .constant(0),
                               selection: .constant(NSRange(location: 0, length: 0)),
                               completion: EditorCompletion(),
                               candidates: { _, _ in [] })
        return editor.makeCoordinator()
    }
}
