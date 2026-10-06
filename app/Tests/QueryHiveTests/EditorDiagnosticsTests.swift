import AppKit
import QueryHiveFFI
import XCTest

@testable import QueryHive

/// A coordinator over a real text view and storage, built the way the editor builds them but without
/// a window: what the diagnostics, rotor and dialect tests drive.
@MainActor
struct EditorRig {
    let editor: SQLEditor
    let coordinator: SQLEditor.Coordinator
    let view: SQLTextView
    let layout: NSLayoutManager
    let count: EditorLineCount
    /// Held: a layout manager does not keep its storage alive.
    let storage: NSTextStorage

    /// Hands `coordinator.parent` a copy of the editor for another dialect, as `updateNSView` would.
    func switchDialect(to dialect: EditorDialect) {
        var next = coordinator.parent
        next.dialect = dialect
        coordinator.parent = next
        coordinator.adoptDialect()
    }

    init(_ text: String, dialect: EditorDialect = .generic) {
        storage = NSTextStorage(string: text)
        layout = NSLayoutManager()
        storage.addLayoutManager(layout)
        let box = NSTextContainer(size: NSSize(width: 420, height: 1000))
        layout.addTextContainer(box)
        view = SQLTextView(frame: NSRect(x: 0, y: 0, width: 420, height: 400), textContainer: box)
        count = EditorLineCount(text: text)
        editor = SQLEditor(text: .constant(text), focused: .constant(false), caret: .constant(0),
                           selection: .constant(NSRange(location: 0, length: 0)),
                           completion: EditorCompletion(), candidates: { _, _ in [] },
                           layout: .standard, onRunStatement: nil, lineCount: count, dialect: dialect)
        coordinator = editor.makeCoordinator()
        coordinator.textView = view
        view.delegate = coordinator
        storage.delegate = coordinator
        coordinator.recolour()
    }

    /// Run the analysis and the outline now, as the idle pass would after a pause.
    func settle() throws { try coordinator.syncAnalysisForTesting() }

    func underline(at index: Int) -> (style: Int?, color: NSColor?) {
        (layout.temporaryAttribute(.underlineStyle, atCharacterIndex: index, effectiveRange: nil) as? Int,
         layout.temporaryAttribute(.underlineColor, atCharacterIndex: index, effectiveRange: nil) as? NSColor)
    }

    /// `EditorLineCount` is written on the next main turn; this lets that turn happen.
    static func drainMain() {
        let turn = XCTestExpectation(description: "main turn")
        DispatchQueue.main.async { turn.fulfill() }
        _ = XCTWaiter.wait(for: [turn], timeout: 2)
    }
}

/// Blueprint w10 §8.3: lexical issues and the server's position, merged, labelled and painted.
@MainActor
final class EditorDiagnosticsTests: XCTestCase {
    private func issue(_ kind: QueryHive.EditorIssueKind, _ location: Int, _ length: Int) -> EditorIssueData {
        EditorIssueData(kind: kind, range: NSRange(location: location, length: length))
    }

    func testMergeKeepsLexicalKindsAndDropsTreeSitterSyntaxErrors() {
        let text = "select 'abc; (" as NSString
        let merged = EditorDiagnostics.merge(
            issues: [issue(.syntaxError, 0, 6), issue(.unclosedQuote, 7, 7), issue(.missingToken, 3, 1),
                     issue(.unbalancedParen, 13, 1)],
            server: nil, text: text)
        XCTAssertEqual(merged.map(\.kind), [.lexical(.unclosedQuote), .lexical(.unbalancedParen)])
        XCTAssertEqual(merged.map(\.range), [NSRange(location: 7, length: 7), NSRange(location: 13, length: 1)])
    }

    func testMergeOrdersByPositionAndPutsTheServerErrorAfterALexicalOneAtTheSamePlace() {
        let text = "select 'abc" as NSString
        let server = EditorDiagnostic(kind: .server, range: NSRange(location: 7, length: 4), message: "boom")
        let merged = EditorDiagnostics.merge(issues: [issue(.unclosedQuote, 7, 4)], server: server, text: text)
        XCTAssertEqual(merged.map(\.kind), [.lexical(.unclosedQuote), .server])
        let early = EditorDiagnostic(kind: .server, range: NSRange(location: 0, length: 6), message: "x")
        XCTAssertEqual(EditorDiagnostics.merge(issues: [issue(.unclosedQuote, 7, 4)], server: early, text: text)
            .map(\.kind), [.server, .lexical(.unclosedQuote)])
    }

    func testMergeDropsRangesOutsideTheTextAndEmptyOnes() {
        let text = "select" as NSString
        let far = EditorDiagnostic(kind: .server, range: NSRange(location: 4, length: 9), message: "x")
        XCTAssertEqual(EditorDiagnostics.merge(issues: [issue(.unclosedQuote, 3, 9), issue(.unclosedComment, 2, 0)],
                                               server: far, text: text), [])
    }

    func testLabelsNameTheKindAndTheLine() {
        let lexical = EditorDiagnostic(kind: .lexical(.unclosedIdentifier), range: NSRange(location: 0, length: 1),
                                       message: DiagnosticLabels.title(.unclosedIdentifier)!)
        XCTAssertEqual(DiagnosticLabels.label(for: lexical, line: 3), "Unclosed quoted identifier, line 3")
        XCTAssertEqual(DiagnosticLabels.title(.unclosedQuote), "Unclosed quote")
        XCTAssertEqual(DiagnosticLabels.title(.unclosedComment), "Unclosed comment")
        XCTAssertEqual(DiagnosticLabels.title(.unclosedDollar), "Unclosed dollar quote")
        XCTAssertEqual(DiagnosticLabels.title(.unbalancedParen), "Unbalanced parenthesis")
        XCTAssertNil(DiagnosticLabels.title(.syntaxError))
        XCTAssertNil(DiagnosticLabels.title(.missingToken))
    }

    func testAServerLabelKeepsEightyCharactersOfOneLine() {
        let long = String(repeating: "word ", count: 30)
        let server = EditorDiagnostic(kind: .server, range: NSRange(location: 0, length: 1),
                                      message: "syntax error\nat or near \"SELEC\"\n" + long)
        let label = DiagnosticLabels.label(for: server, line: 1)
        XCTAssertTrue(label.hasPrefix("Server error, line 1: syntax error at or near \"SELEC\" word"), label)
        XCTAssertFalse(label.contains("\n"))
        XCTAssertEqual(label.count, "Server error, line 1: ".count + DiagnosticLabels.serverMessageLimit + 1)
        XCTAssertTrue(label.hasSuffix("…"))
    }

    func testTheIssuesRotorLabelsEachStopWithItsLine() {
        let text = "select 1;\nselect 'abc;\nselect (" as NSString
        let diagnostics = EditorDiagnostics.merge(
            issues: [issue(.unclosedQuote, 17, 6), issue(.unbalancedParen, 30, 1)], server: nil, text: text)
        let rotor = IssuesRotorSource(diagnostics: diagnostics, text: text)
        XCTAssertEqual(rotor.title, "Query issues")
        XCTAssertEqual(rotor.items.map(\.label), ["Unclosed quote, line 2", "Unbalanced parenthesis, line 3"])
        XCTAssertEqual(rotor.items.map(\.offset), [17, 30])
        XCTAssertEqual(rotor.items.map(\.length), [6, 1])
    }

    func testTheSummaryCountsAndSaysWhetherTheServerIsOneOfThem() {
        XCTAssertEqual(EditorIssueSummary([]), EditorIssueSummary())
        let lexical = EditorDiagnostic(kind: .lexical(.unclosedQuote), range: NSRange(location: 0, length: 1), message: "a")
        let server = EditorDiagnostic(kind: .server, range: NSRange(location: 2, length: 1), message: "b")
        XCTAssertEqual(EditorIssueSummary([lexical]).count, 1)
        XCTAssertFalse(EditorIssueSummary([lexical]).hasServerError)
        XCTAssertTrue(EditorIssueSummary([lexical, server]).hasServerError)
    }

    // MARK: In the editor

    func testAnUnclosedQuoteIsUnderlinedDashedAmberAndCounted() throws {
        let rig = EditorRig("select 1;\nselect 'oops;\n")
        try rig.settle()
        XCTAssertEqual(rig.coordinator.diagnostics.map(\.kind), [.lexical(.unclosedQuote)])
        let range = try XCTUnwrap(rig.coordinator.diagnostics.first?.range)
        let mark = rig.underline(at: range.location)
        XCTAssertEqual(mark.style, DiagnosticsPainter.lexicalStyle)
        XCTAssertTrue(mark.color === DiagnosticsPainter.amber)
        XCTAssertNil(rig.underline(at: 0).style, "text outside the issue is not underlined")
        EditorRig.drainMain()
        XCTAssertEqual(rig.count.issues, EditorIssueSummary(rig.coordinator.diagnostics))
        XCTAssertEqual(rig.count.issues.count, 1)
    }

    func testAServerMarkIsUnderlinedThickCoralAndListedBeforeLaterIssues() throws {
        let text = "selec 1;\nselect 'oops;\n"
        let rig = EditorRig(text)
        try rig.settle()
        let mark = ServerErrorMark(sqlSnapshot: text, sent: SentSQL(text: text, documentStart: 0),
                                   scalarOffset: 1, message: "syntax error at or near \"selec\"")
        rig.coordinator.adoptServerMark(mark)
        XCTAssertEqual(rig.coordinator.diagnostics.map(\.kind), [.server, .lexical(.unclosedQuote)])
        XCTAssertEqual(rig.coordinator.diagnostics.first?.range, NSRange(location: 0, length: 5))
        let server = rig.underline(at: 2)
        XCTAssertEqual(server.style, DiagnosticsPainter.serverStyle)
        XCTAssertTrue(server.color === DiagnosticsPainter.coral)
        XCTAssertEqual(rig.coordinator.issuesRotor?.items.map(\.label).first,
                       "Server error, line 1: syntax error at or near \"selec\"")
        EditorRig.drainMain()
        XCTAssertTrue(rig.count.issues.hasServerError)
    }

    func testTheFirstEditDropsTheServerMarkAndItDoesNotComeBack() throws {
        let text = "selec 1"
        let rig = EditorRig(text)
        try rig.settle()
        let mark = ServerErrorMark(sqlSnapshot: text, sent: SentSQL(text: text, documentStart: 0),
                                   scalarOffset: 1, message: "boom")
        rig.coordinator.adoptServerMark(mark)
        XCTAssertNotNil(rig.underline(at: 0).style)
        rig.view.setSelectedRange(NSRange(location: 7, length: 0))
        rig.view.insertText(" ", replacementRange: NSRange(location: NSNotFound, length: 0))
        // The model still holds the mark for up to 150 ms; SwiftUI hands it over again.
        rig.coordinator.adoptServerMark(mark)
        try rig.settle()
        XCTAssertEqual(rig.coordinator.diagnostics, [])
        XCTAssertNil(rig.underline(at: 0).style)
    }

    func testAMarkForOtherTextIsNotWorn() throws {
        let rig = EditorRig("select 1")
        try rig.settle()
        let other = "selec 1"
        rig.coordinator.adoptServerMark(ServerErrorMark(
            sqlSnapshot: other, sent: SentSQL(text: other, documentStart: 0), scalarOffset: 1, message: "boom"))
        XCTAssertEqual(rig.coordinator.diagnostics, [])
        XCTAssertNil(rig.underline(at: 0).style)
    }

    func testTheUnderlineIsGoneOnceTheQuoteIsClosed() throws {
        let rig = EditorRig("select 'oops")
        try rig.settle()
        XCTAssertEqual(rig.coordinator.diagnostics.count, 1)
        rig.view.setSelectedRange(NSRange(location: 12, length: 0))
        rig.view.insertText("'", replacementRange: NSRange(location: NSNotFound, length: 0))
        try rig.settle()
        XCTAssertEqual(rig.coordinator.diagnostics, [])
        XCTAssertNil(rig.underline(at: 8).style)
    }

    func testTheSyntaxPaintStillOwnsTheForegroundColour() throws {
        let rig = EditorRig("select 'oops")
        try rig.settle()
        XCTAssertNotNil(rig.layout.temporaryAttribute(.foregroundColor, atCharacterIndex: 0, effectiveRange: nil),
                        "colouring a keyword must survive the underline pass")
    }
}
