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

    // MARK: Real rotors (W10-T6b, D-13)

    private func search(_ rotor: NSAccessibilityCustomRotor, forward: Bool,
                        from current: NSAccessibilityCustomRotor.ItemResult? = nil)
        -> NSAccessibilityCustomRotor.ItemResult? {
        let parameters = NSAccessibilityCustomRotor.SearchParameters()
        parameters.searchDirection = forward ? .next : .previous
        parameters.currentItem = current
        return rotor.itemSearchDelegate?.rotor(rotor, resultFor: parameters)
    }

    func testTheEditorVendsTheStatementsAndQueryIssuesRotors() throws {
        let rig = EditorRig("select 1;\nselect 'oops;\n")
        try rig.settle()
        let rotors = rig.view.accessibilityCustomRotors()
        XCTAssertEqual(rotors.map(\.label), ["Statements", "Query issues"])
    }

    func testTheStatementsRotorStepsForwardAndBackAndStopsAtTheEnds() throws {
        let rig = EditorRig("select 1;\nupdate t set a = 2;\ndelete from t")
        try rig.settle()
        let statements = try XCTUnwrap(rig.view.accessibilityCustomRotors().first)
        rig.view.setSelectedRange(NSRange(location: 0, length: 0))
        let first = try XCTUnwrap(search(statements, forward: true))
        XCTAssertEqual(first.customLabel, "1 · SELECT")
        XCTAssertEqual(first.targetRange.location, 0)
        XCTAssertTrue(first.targetElement === rig.view)
        let second = try XCTUnwrap(search(statements, forward: true, from: first))
        XCTAssertEqual(second.customLabel, "2 · UPDATE")
        let third = try XCTUnwrap(search(statements, forward: true, from: second))
        XCTAssertEqual(third.customLabel, "3 · DELETE")
        XCTAssertNil(search(statements, forward: true, from: third), "a rotor does not wrap")
        XCTAssertEqual(search(statements, forward: false, from: third)?.customLabel, "2 · UPDATE")
        XCTAssertNil(search(statements, forward: false, from: first))
    }

    func testWithoutACurrentItemTheSearchStartsAtTheCaret() throws {
        let rig = EditorRig("select 1;\nupdate t set a = 2;\ndelete from t")
        try rig.settle()
        let statements = try XCTUnwrap(rig.view.accessibilityCustomRotors().first)
        rig.view.setSelectedRange(NSRange(location: 14, length: 0))   // inside the UPDATE
        XCTAssertEqual(search(statements, forward: true)?.customLabel, "3 · DELETE")
        XCTAssertEqual(search(statements, forward: false)?.customLabel, "2 · UPDATE")
    }

    func testTheIssuesRotorLandsOnTheUnderlinedRange() throws {
        let text = "selec 1;\nselect 'oops;\n"
        let rig = EditorRig(text)
        try rig.settle()
        rig.coordinator.adoptServerMark(ServerErrorMark(
            sqlSnapshot: text, sent: SentSQL(text: text, documentStart: 0), scalarOffset: 1,
            message: "syntax error at or near \"selec\""))
        let issues = try XCTUnwrap(rig.view.accessibilityCustomRotors().last)
        XCTAssertEqual(issues.label, "Query issues")
        rig.view.setSelectedRange(NSRange(location: 0, length: 0))
        let server = try XCTUnwrap(search(issues, forward: true))
        XCTAssertEqual(server.customLabel, "Server error, line 1: syntax error at or near \"selec\"")
        XCTAssertEqual(server.targetRange, NSRange(location: 0, length: 5))
        let quote = try XCTUnwrap(search(issues, forward: true, from: server))
        XCTAssertEqual(quote.customLabel, "Unclosed quote, line 2")
        XCTAssertEqual(quote.targetRange.location, 16)
    }

    func testAnIssuesRotorWithNothingInItFindsNothing() throws {
        let rig = EditorRig("select 1")
        try rig.settle()
        let issues = try XCTUnwrap(rig.view.accessibilityCustomRotors().last)
        XCTAssertNil(search(issues, forward: true))
    }

    func testTheRotorReadsTheLatestItemsAfterAnEdit() throws {
        let rig = EditorRig("select 1")
        try rig.settle()
        let statements = try XCTUnwrap(rig.view.accessibilityCustomRotors().first)
        rig.view.setSelectedRange(NSRange(location: 8, length: 0))
        rig.view.insertText("; select 2", replacementRange: NSRange(location: NSNotFound, length: 0))
        try rig.settle()
        // The same rotor object, asked again: it reads the view's current sources, not a copy.
        rig.view.setSelectedRange(NSRange(location: 0, length: 0))
        let first = try XCTUnwrap(search(statements, forward: true))
        XCTAssertEqual(search(statements, forward: true, from: first)?.customLabel, "2 · SELECT")
    }

    func testItemSearchWorksOnAPlainList() {
        let items = [EditorRotorItem(label: "a", offset: 0), EditorRotorItem(label: "b", offset: 10),
                     EditorRotorItem(label: "c", offset: 20)]
        XCTAssertEqual(EditorRotorSearch.item(in: items, from: nil, caret: 10, forward: true)?.label, "b")
        XCTAssertEqual(EditorRotorSearch.item(in: items, from: nil, caret: 10, forward: false)?.label, "b")
        XCTAssertEqual(EditorRotorSearch.item(in: items, from: 10, caret: 0, forward: true)?.label, "c")
        XCTAssertEqual(EditorRotorSearch.item(in: items, from: 10, caret: 0, forward: false)?.label, "a")
        XCTAssertNil(EditorRotorSearch.item(in: items, from: 20, caret: 0, forward: true))
        XCTAssertNil(EditorRotorSearch.item(in: [], from: nil, caret: 0, forward: true))
    }
}

