import AppKit
import SwiftUI
import XCTest

@testable import QueryHive

/// The editor's per-edit bookkeeping (Fase 4A): the line index, the ranges that follow the text, the
/// debounced model write and the folds that move with an edit. Each of these replaced a pass over
/// the whole document, so each is checked against the pass it replaced.
@MainActor
final class EditorIncrementalTests: XCTestCase {
    /// A seeded generator, so a failure names an edit sequence that can be replayed.
    private struct Lehmer: RandomNumberGenerator {
        var state: UInt64
        mutating func next() -> UInt64 {
            state = state &* 6364136223846793005 &+ 1442695040888963407
            return state
        }
    }

    func testTheLineIndexAfterRandomEditsEqualsAFullRescan() {
        var random = Lehmer(state: 42)
        let text = NSMutableString(string: "SELECT 1;\nFROM t\n\nWHERE x\n")
        var index = LineIndex(text: text)
        let pieces = ["", "a", "\n", "ab\ncd", "\n\n", "é\n", "x"]
        for step in 0..<400 {
            let length = text.length
            let location = Int.random(in: 0...length, using: &random)
            let removed = Int.random(in: 0...min(6, length - location), using: &random)
            let inserted = pieces.randomElement(using: &random)!
            text.replaceCharacters(in: NSRange(location: location, length: removed), with: inserted)
            let delta = (inserted as NSString).length - removed
            index.edit(range: NSRange(location: location, length: (inserted as NSString).length),
                       delta: delta, in: text)
            XCTAssertEqual(index.starts, SQLFolding.lineStarts(in: text), "step \(step)")
            XCTAssertEqual(index.length, text.length, "step \(step)")
        }
    }

    func testARangeFollowsTheTextThroughAnEdit() {
        let statement = NSRange(location: 10, length: 20)
        // Before it: it moves. After it: it stays. Inside it: it stretches.
        XCTAssertEqual(TextEdit(location: 2, oldLength: 0, newLength: 3).grow(statement),
                       NSRange(location: 13, length: 20))
        XCTAssertEqual(TextEdit(location: 40, oldLength: 0, newLength: 3).grow(statement), statement)
        XCTAssertEqual(TextEdit(location: 15, oldLength: 2, newLength: 5).grow(statement),
                       NSRange(location: 10, length: 23))
        var list = [NSRange(location: 0, length: 5), statement, NSRange(location: 40, length: 5)]
        TextEdit(location: 15, oldLength: 0, newLength: 1).apply(to: &list)
        XCTAssertEqual(list, [NSRange(location: 0, length: 5), NSRange(location: 10, length: 21),
                              NSRange(location: 41, length: 5)])
    }

    // MARK: The editor around a text view

    private final class Model {
        var text: String
        var writes = 0
        init(_ text: String) { self.text = text }
    }

    private struct Rig {
        let textView: SQLTextView
        let coordinator: SQLEditor.Coordinator
        let ruler: LineNumberRulerView
        let model: Model
        let layoutManager: NSLayoutManager
        let container: NSTextContainer
        let window: NSWindow
    }

    /// The same stack `makeNSView` builds, without a SwiftUI host: TextKit 1, the coordinator as the
    /// storage's delegate, and a gutter.
    private func rig(_ sql: String) -> Rig {
        let model = Model(sql)
        let storage = NSTextStorage()
        let layoutManager = NSLayoutManager()
        layoutManager.allowsNonContiguousLayout = true
        storage.addLayoutManager(layoutManager)
        let container = NSTextContainer(size: NSSize(width: 420, height: CGFloat.greatestFiniteMagnitude))
        container.widthTracksTextView = false
        layoutManager.addTextContainer(container)
        let textView = SQLTextView(frame: NSRect(x: 0, y: 0, width: 420, height: 400), textContainer: container)
        textView.font = .monospacedSystemFont(ofSize: 13, weight: .regular)
        textView.string = sql
        let editor = SQLEditor(
            text: Binding(get: { model.text }, set: { model.text = $0; model.writes += 1 }),
            focused: .constant(false), caret: .constant(0), selection: .constant(NSRange(location: 0, length: 0)),
            completion: EditorCompletion(), candidates: { _, _ in [] }, layout: .standard, onRunStatement: nil)
        let coordinator = editor.makeCoordinator()
        coordinator.textView = textView
        textView.delegate = coordinator
        storage.delegate = coordinator
        let scroll = NSScrollView(frame: NSRect(x: 0, y: 0, width: 420, height: 400))
        scroll.documentView = textView
        // In a window with the text view focused, so a keystroke is not also "editing ended".
        let window = NSWindow(contentRect: scroll.frame, styleMask: [.titled], backing: .buffered, defer: false)
        window.contentView = scroll
        window.makeFirstResponder(textView)
        let ruler = LineNumberRulerView(textView: textView)
        coordinator.ruler = ruler
        coordinator.recolour()
        return Rig(textView: textView, coordinator: coordinator, ruler: ruler, model: model,
                   layoutManager: layoutManager, container: container, window: window)
    }

    private func type(_ text: String, into rig: Rig) {
        rig.textView.insertText(text, replacementRange: NSRange(location: NSNotFound, length: 0))
    }

    func testTheGutterFollowsTypingWithoutRescanning() {
        let rig = rig("SELECT 1;\nSELECT 2;")
        rig.textView.setSelectedRange(NSRange(location: 9, length: 0))
        type("\n", into: rig)
        XCTAssertEqual(rig.ruler.lineCount, 3)
        XCTAssertEqual(rig.ruler.line(containing: 10), 1)
        type("x", into: rig)
        XCTAssertEqual(rig.ruler.lineCount, 3)
        XCTAssertEqual(rig.ruler.lineCount, SQLFolding.lineStarts(in: rig.textView.string as NSString).count)
    }

    func testTheModelIsWrittenOnAFlushAndNotOnEveryKey() {
        let rig = rig("SELECT 1")
        rig.textView.setSelectedRange(NSRange(location: 8, length: 0))
        for character in [" ", "+", " ", "2"] { type(character, into: rig) }
        XCTAssertEqual(rig.model.text, "SELECT 1", "typing wrote the model before the pause")
        XCTAssertEqual(rig.model.writes, 0)
        XCTAssertTrue(rig.coordinator.hasUnpublishedEdits)

        rig.coordinator.flushToModel()
        XCTAssertEqual(rig.model.text, "SELECT 1 + 2")
        XCTAssertEqual(rig.model.writes, 1)
        XCTAssertFalse(rig.coordinator.hasUnpublishedEdits)

        // A second flush with nothing owed writes nothing.
        rig.coordinator.flushToModel()
        XCTAssertEqual(rig.model.writes, 1)
    }

    func testTheModelsOwnTextIsNotReappliedAndAnExternalChangeIs() {
        let rig = rig("SELECT 1")
        rig.coordinator.adoptModelText("SELECT 1")
        XCTAssertEqual(rig.textView.string, "SELECT 1")

        rig.coordinator.adoptModelText("SELECT 1 FROM t")
        XCTAssertEqual(rig.textView.string, "SELECT 1 FROM t")
        XCTAssertEqual(rig.coordinator.publishedText, "SELECT 1 FROM t")
        XCTAssertFalse(rig.coordinator.hasUnpublishedEdits)
    }

    func testEditsAheadOfTheModelAreNotOverwrittenByTheModelsOlderText() {
        let rig = rig("SELECT 1")
        rig.textView.setSelectedRange(NSRange(location: 8, length: 0))
        type("0", into: rig)
        // SwiftUI updates the editor with the model's text, which is behind the editor's.
        rig.coordinator.adoptModelText("SELECT 1")
        XCTAssertEqual(rig.textView.string, "SELECT 10")
    }

    func testAFoldStaysOnItsLinesWhenTextIsTypedAboveIt() throws {
        let sql = "SELECT 1;\nSELECT a,\n  b\nFROM t;\nSELECT 2;"
        let rig = rig(sql)
        let region = try XCTUnwrap(SQLFolding.regions(in: sql).first { $0.kind == .statement })
        rig.coordinator.toggleFold(headerOffset: region.header)
        rig.layoutManager.ensureLayout(for: rig.container)
        let folded = rig.layoutManager.usedRect(for: rig.container).height

        rig.textView.setSelectedRange(NSRange(location: 0, length: 0))
        type("-- note\n", into: rig)
        rig.layoutManager.ensureLayout(for: rig.container)
        // One line taller for the comment, and the fold still collapses its body.
        let lineHeight = rig.layoutManager.defaultLineHeight(for: rig.textView.font!)
        XCTAssertEqual(rig.layoutManager.usedRect(for: rig.container).height, folded + lineHeight, accuracy: 1,
                       "the fold did not follow the text down")
        let hidden = FoldHider.installed(on: rig.layoutManager).hidden
        XCTAssertEqual(hidden.count, 1)
        XCTAssertEqual(hidden[0].location, region.bodyStart + 8)
    }

    func testTypingInsideAStatementRepaintsItAndNothingElse() throws {
        let sql = "SELECT 1;\nselect 2 from t;\nSELECT 3;"
        let rig = rig(sql)
        rig.textView.setSelectedRange(NSRange(location: 21, length: 0))
        let before = rig.textView.textStorage!.attributes(at: 0, effectiveRange: nil)[.foregroundColor]
        type(" ", into: rig)
        // The edited statement is painted; a keystroke elsewhere in the text leaves the rest as it was.
        XCTAssertNotNil(rig.textView.textStorage!.attributes(at: 12, effectiveRange: nil)[.foregroundColor])
        XCTAssertEqual(String(describing: rig.textView.textStorage!.attributes(at: 0, effectiveRange: nil)[.foregroundColor]),
                       String(describing: before))
    }

    // MARK: Model flushes and who wins

    func testTheModelFlushHookWritesWhatWasTypedAndRunCallsIt() {
        let rig = rig("SELECT 1")
        rig.textView.setSelectedRange(NSRange(location: 8, length: 0))
        type("0", into: rig)
        XCTAssertEqual(rig.model.text, "SELECT 1")
        AppModel.flushEditors()
        XCTAssertEqual(rig.model.text, "SELECT 10", "the hook the model calls did not reach the editor")

        var called = 0
        let saved = AppModel.flushEditors
        AppModel.flushEditors = { called += 1 }
        defer { AppModel.flushEditors = saved }
        let model = AppModel()
        if let tab = model.selectedTab { model.run(tab) }
        XCTAssertEqual(called, 1, "run read the tab's SQL without flushing the editors first")
    }

    func testTheFirstCharacterAndTheLastAreWrittenAtOnce() {
        let rig = rig("")
        type("x", into: rig)
        XCTAssertEqual(rig.model.text, "x", "empty -> non-empty waits for the pause")
        type("y", into: rig)
        XCTAssertEqual(rig.model.writes, 1, "a second key wrote the model")
        rig.textView.setSelectedRange(NSRange(location: 0, length: 2))
        rig.textView.insertText("", replacementRange: NSRange(location: NSNotFound, length: 0))
        XCTAssertEqual(rig.model.text, "", "non-empty -> empty waits for the pause")
    }

    func testLocalTypingWinsOverAnOutsideWriteAndNothingIsWrittenDuringTheUpdate() {
        let rig = rig("SELECT 1")
        rig.textView.setSelectedRange(NSRange(location: 8, length: 0))
        type("0", into: rig)
        let writes = rig.model.writes
        rig.coordinator.adoptModelText("SELECT 99")
        XCTAssertEqual(rig.textView.string, "SELECT 10")
        XCTAssertEqual(rig.model.writes, writes)
        rig.coordinator.flushToModel()
        XCTAssertEqual(rig.model.text, "SELECT 10")
    }

    func testAStatementWithADoubleQuotedSemicolonIsColouredAsTheWholeDocumentIs() throws {
        let sql = "SELECT \"a;b\" FROM t;\nSELECT 2;"
        let rig = rig(sql)
        rig.textView.setSelectedRange(NSRange(location: 6, length: 0))
        type(" ", into: rig)
        let text = rig.textView.string
        let expected = SQLSyntax.attributes(for: text, baseFont: .systemFont(ofSize: 12), commentFont: .systemFont(ofSize: 12))
        let index = (text as NSString).range(of: "b\"").location
        let run = try XCTUnwrap(expected.first { NSLocationInRange(index, $0.0) })
        XCTAssertEqual(String(describing: rig.textView.textStorage!.attributes(at: index, effectiveRange: nil)[.foregroundColor]),
                       String(describing: run.1[.foregroundColor]))
    }

    func testAnEditAfterTheSemicolonInsideAQuotedNameIsColouredAsTheWholeDocumentIs() throws {
        let sql = "SELECT \"a;b\" FROM t;\nSELECT 2;"
        let rig = rig(sql)
        rig.textView.setSelectedRange(NSRange(location: 17, length: 0))
        type(" ", into: rig)
        let text = rig.textView.string
        let expected = SQLSyntax.attributes(for: text, baseFont: .systemFont(ofSize: 12), commentFont: .systemFont(ofSize: 12))
        let index = (text as NSString).range(of: "b\"").location
        let run = try XCTUnwrap(expected.first { NSLocationInRange(index, $0.0) })
        XCTAssertEqual(String(describing: rig.textView.textStorage!.attributes(at: index, effectiveRange: nil)[.foregroundColor]),
                       String(describing: run.1[.foregroundColor]))
    }

    /// A quote that never closes must not make a keystroke follow it to the end of the document. The
    /// bound is generous for a debug build; the release cost is a small fraction of it.
    func testALoneQuoteInALargeDocumentDoesNotFreezeTyping() {
        let line = "SELECT id, name FROM public.table_1 WHERE id = 1 AND status = 'open' ORDER BY id;\n"
        let rig = rig(String(repeating: line, count: 2600))
        let middle = (rig.textView.string as NSString).lineRange(for: NSRange(location: 100_000, length: 0)).location
        rig.textView.setSelectedRange(NSRange(location: middle + 7, length: 0))
        rig.textView.scrollRangeToVisible(NSRange(location: middle, length: 0))
        let started = CFAbsoluteTimeGetCurrent()
        type("\"", into: rig)
        for character in "abc de" { type(String(character), into: rig) }
        let elapsed = CFAbsoluteTimeGetCurrent() - started
        XCTAssertLessThan(elapsed, 0.75, "typing an unbalanced quote took \(elapsed) s")
    }
}
