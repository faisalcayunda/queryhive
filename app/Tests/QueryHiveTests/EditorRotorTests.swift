import AppKit
import XCTest

@testable import QueryHive

/// FR-ED-09: the "Statements" rotor lists every statement with a readable label and its offset.
@MainActor
final class EditorRotorTests: XCTestCase {
    func testLabelsHeadTheLeadingKeyword() {
        let text = "select 1;\n  UPDATE t SET a = 2;" as NSString
        let source = StatementsRotorSource(
            statements: [NSRange(location: 0, length: 9), NSRange(location: 10, length: 22)],
            text: text)
        XCTAssertEqual(source.title, "Statements")
        XCTAssertEqual(source.items.map(\.label), ["1 · SELECT", "2 · UPDATE"])
        XCTAssertEqual(source.items.map(\.offset), [0, 10])
    }

    func testABlankStatementFallsBackToItsPosition() {
        let text = "   ;\nselect 1" as NSString
        let source = StatementsRotorSource(statements: [NSRange(location: 0, length: 3)], text: text)
        XCTAssertEqual(source.items.map(\.label), ["1 · STATEMENT"])
        XCTAssertEqual(source.items.map(\.offset), [0])
    }
    /// The coordinator rebuilds the rotor with every outline: typing a `;` adds a stop.
    func testTheCoordinatorsRotorTracksTheOutline() throws {
        let storage = NSTextStorage(string: "select 1")
        let layout = NSLayoutManager()
        storage.addLayoutManager(layout)
        let box = NSTextContainer(size: NSSize(width: 420, height: 1000))
        layout.addTextContainer(box)
        let view = SQLTextView(frame: NSRect(x: 0, y: 0, width: 420, height: 400), textContainer: box)
        let editor = SQLEditor(text: .constant("select 1"), focused: .constant(false),
                               caret: .constant(0),
                               selection: .constant(NSRange(location: 0, length: 0)),
                               completion: EditorCompletion(), candidates: { _, _ in [] },
                               layout: .standard, onRunStatement: nil)
        let coordinator = editor.makeCoordinator()
        coordinator.textView = view
        view.delegate = coordinator
        storage.delegate = coordinator
        coordinator.recolour()
        try coordinator.syncAnalysisForTesting()
        XCTAssertEqual(coordinator.rotor?.items.map(\.label), ["1 · SELECT"])
        view.setSelectedRange(NSRange(location: 8, length: 0))
        view.insertText("; select 2", replacementRange: NSRange(location: NSNotFound, length: 0))
        try coordinator.syncAnalysisForTesting()
        XCTAssertEqual(coordinator.rotor?.items.map(\.offset), [0, 9])
        XCTAssertEqual(coordinator.rotor?.items.map(\.label), ["1 · SELECT", "2 · SELECT"])
    }
}
