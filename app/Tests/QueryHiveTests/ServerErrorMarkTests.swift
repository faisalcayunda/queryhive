import AppKit
import QueryHiveFFI
import XCTest

@testable import QueryHive

/// Blueprint w10 §8.2: a server position is an offset into the text that was sent, so it is turned
/// into a document range only through the exact text and start that Run recorded.
final class ServerErrorMarkTests: XCTestCase {
    private func mark(_ sent: String, at start: Int = 0, scalar: Int, in snapshot: String? = nil) -> ServerErrorMark {
        ServerErrorMark(sqlSnapshot: snapshot ?? sent, sent: SentSQL(text: sent, documentStart: start),
                        scalarOffset: scalar, message: "boom")
    }

    private func range(_ mark: ServerErrorMark) -> NSRange? {
        mark.range(in: mark.sqlSnapshot as NSString)
    }

    // MARK: Scalar offset to UTF-16 range

    func testAPositionUnderlinesTheWordItFallsOn() {
        XCTAssertEqual(range(mark("selec 1", scalar: 1)), NSRange(location: 0, length: 5))
        XCTAssertEqual(range(mark("select * frm t", scalar: 10)), NSRange(location: 9, length: 3))
        XCTAssertEqual(range(mark("select a_b1 x", scalar: 8)), NSRange(location: 7, length: 4))
    }

    func testAPositionInsideAWordUnderlinesTheRestOfIt() {
        XCTAssertEqual(range(mark("select abcdef", scalar: 10)), NSRange(location: 9, length: 4))
    }

    func testPunctuationIsOneCharacter() {
        XCTAssertEqual(range(mark("select (1", scalar: 8)), NSRange(location: 7, length: 1))
        XCTAssertEqual(range(mark("select 1 ,", scalar: 10)), NSRange(location: 9, length: 1))
    }

    func testEmojiAndCJKCountAsOneScalarEach() {
        // The emoji is one scalar and two UTF-16 units, so scalar 12 is UTF-16 offset 13.
        let sql = "select '😀' selec"
        XCTAssertEqual(range(mark(sql, scalar: 12)), NSRange(location: 12, length: 5))
        let cjk = "select '日本語' selec"
        XCTAssertEqual(range(mark(cjk, scalar: 14)), NSRange(location: 13, length: 5))
    }

    func testAWordIsKeptToSixtyFourUnits() {
        let sql = "select " + String(repeating: "a", count: 100)
        XCTAssertEqual(range(mark(sql, scalar: 8))?.length, 64)
    }

    func testTheUnderlineStaysOnItsLine() {
        XCTAssertEqual(range(mark("select\nselec 1", scalar: 8)), NSRange(location: 7, length: 5))
        // A position on the newline itself has nothing to underline.
        XCTAssertNil(range(mark("select\nx", scalar: 7)))
    }

    func testAnIndentedLineStartIsSteppedOver() {
        // MySQL names the start of a line; the blanks that indent it are not the problem.
        XCTAssertEqual(range(mark("select 1\n    selec 2", scalar: 10)), NSRange(location: 13, length: 5))
    }

    func testEndOfInputUnderlinesTheLastWord() {
        XCTAssertEqual(range(mark("select * from", scalar: 14)), NSRange(location: 9, length: 4))
        XCTAssertEqual(range(mark("select * from  \n", scalar: 17)), NSRange(location: 9, length: 4))
        XCTAssertEqual(range(mark("select (", scalar: 9)), NSRange(location: 7, length: 1))
    }

    func testAnOffsetOutsideTheSentTextHasNoRange() {
        XCTAssertNil(range(mark("select 1", scalar: 10)))
        XCTAssertNil(range(mark("select 1", scalar: 0)))
        XCTAssertNil(range(mark("select 1", scalar: -3)))
    }

    func testTheSentTextMustFitInTheDocument() {
        XCTAssertNil(mark("select 1", at: 5, scalar: 1, in: "select 1").range(in: "select 1" as NSString))
    }

    // MARK: Sent text and its start

    private func tab(_ sql: String, caret: Int, selecting length: Int = 0,
                     dialect: EditorDialect = .generic) -> QueryTab {
        let tab = QueryTab(title: "Query 1")
        tab.sql = sql
        tab.dialect = dialect
        tab.selection = NSRange(location: caret, length: length)
        return tab
    }

    func testAnEmptySelectionSendsTheWholeDocumentFromItsStartNotFromTheCaret() throws {
        let sql = "select 1;\nselec 2;\nselect 3"
        let tab = tab(sql, caret: 14)
        let sent = tab.sent(for: .selection)
        XCTAssertEqual(sent, SentSQL(text: sql, documentStart: 0))
        // Position 11 is the 11th scalar of the document, not caret + 11.
        let mark = ServerErrorMark(sqlSnapshot: sql, sent: sent, scalarOffset: 11, message: "boom")
        XCTAssertEqual(mark.range(in: sql as NSString), NSRange(location: 10, length: 5))
    }

    func testASelectionStartsWhereTheSelectionStarts() throws {
        let sql = "select 1;\nselec 2;\nselect 3"
        let tab = tab(sql, caret: 10, selecting: 8)
        let sent = tab.sent(for: .selection)
        XCTAssertEqual(sent, SentSQL(text: "selec 2;", documentStart: 10))
        let mark = ServerErrorMark(sqlSnapshot: sql, sent: sent, scalarOffset: 1, message: "boom")
        XCTAssertEqual(mark.range(in: sql as NSString), NSRange(location: 10, length: 5))
    }

    func testASelectionOutsideTheDocumentFallsBackToTheWholeText() {
        let tab = tab("select 1", caret: 4, selecting: 40)
        XCTAssertEqual(tab.sent(for: .selection), SentSQL(text: "select 1", documentStart: 0))
    }

    func testAStatementWithLeadingBlanksStartsAtItsFirstCharacter() {
        let sql = "select 1;\n\n   selec 2"
        let tab = tab(sql, caret: 18)
        let sent = tab.sent(for: .statement)
        XCTAssertEqual(sent.text, "selec 2")
        XCTAssertEqual(sent.documentStart, 14)
        XCTAssertEqual(ServerErrorMark(sqlSnapshot: sql, sent: sent, scalarOffset: 1, message: "x")
            .range(in: sql as NSString), NSRange(location: 14, length: 5))
    }

    func testAStatementAfterAnEmojiCountsItInUTF16() {
        // The emoji is two UTF-16 units and one scalar: the start is in units, the position in scalars.
        let sql = "select '😀';\n\n   selec 2"
        let tab = tab(sql, caret: (sql as NSString).length - 2)
        let sent = tab.sent(for: .statement)
        XCTAssertEqual(sent.text, "selec 2")
        XCTAssertEqual(sent.documentStart, (sql as NSString).length - 7)
        XCTAssertEqual(ServerErrorMark(sqlSnapshot: sql, sent: sent, scalarOffset: 1, message: "x")
            .range(in: sql as NSString), NSRange(location: sent.documentStart, length: 5))
    }

    func testNoStatementFallsBackToTheWholeDocument() {
        XCTAssertEqual(tab("-- only a comment", caret: 3).sent(for: .statement),
                       SentSQL(text: "-- only a comment", documentStart: 0))
    }

    func testSentTextIsWhatSqlForReturnsForEverySource() {
        let sql = "select 1;\n\n   selec 2;\nselect 'é😀'"
        for caret in [0, 5, 14, 22, (sql as NSString).length] {
            for length in [0, 4] {
                let tab = tab(sql, caret: caret, selecting: min(length, (sql as NSString).length - caret))
                for source in [QuerySource.all, .selection, .statement] {
                    XCTAssertEqual(tab.sent(for: source).text, tab.sql(for: source),
                                   "caret \(caret), length \(length), \(source)")
                }
            }
        }
    }

    func testTheStatementCutFollowsTheTabsDialect() {
        let sql = "SELECT '\\''; DELETE FROM t; -- '"
        let mysql = tab(sql, caret: 20, dialect: .mysql)
        XCTAssertEqual(mysql.sent(for: .statement).text, "DELETE FROM t")
        // Generic reads `''` as a doubled quote and sees one statement.
        let generic = tab(sql, caret: 20, dialect: .generic)
        XCTAssertNotEqual(generic.sent(for: .statement).text, "DELETE FROM t")
    }

    // MARK: The mark on the tab

    func testAnEditDropsTheMark() {
        let tab = tab("selec 1", caret: 0)
        tab.errorMark = mark("selec 1", scalar: 1)
        tab.sql = "selec 1"
        XCTAssertNotNil(tab.errorMark, "writing the same text is not an edit")
        tab.sql = "select 1"
        XCTAssertNil(tab.errorMark)
    }

    func testAMarkIsNotEqualToAnotherRunsMark() {
        XCTAssertNotEqual(mark("selec 1", scalar: 1), mark("selec 1", scalar: 1), "each Run's mark has its own identity")
    }
}

/// A Run asks for the position, and keeps it only for the exact text it sent.
@MainActor
final class ServerErrorRunTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    private func rig(_ sql: String, caret: Int = 0, kind: ConnectionKind = .postgres)
        -> (AppModel, QueryTab, MockEngine) {
        let engine = MockEngine()
        let model = AppModel()
        model.engine = engine
        model.recordsHistory = false
        let connection = Connection(id: UUID(), name: "Dev", color: .violet, kind: kind,
                                    host: "127.0.0.1", port: 5432, sslmode: "prefer",
                                    user: "qh", database: "qh", schema: "public", verify: false)
        model.connections = [connection]
        let tab = QueryTab(title: "Query 1")
        tab.connectionID = connection.id
        tab.sql = sql
        tab.selection = NSRange(location: caret, length: 0)
        model.tabs = [tab]
        model.selectedTabID = tab.id
        return (model, tab, engine)
    }

    private func failure(_ message: String, position: Int?) -> Event {
        var event = Event(event: "error")
        event.message = message
        event.position = position
        return event
    }

    private func waitForRun(_ tab: QueryTab) {
        let finished = expectation(description: "run finished")
        func poll() {
            if !tab.previewing { finished.fulfill() } else {
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.01) { MainActor.assumeIsolated { poll() } }
            }
        }
        // One turn first: the scripted engine answers on the next main turn.
        DispatchQueue.main.async { MainActor.assumeIsolated { poll() } }
        wait(for: [finished], timeout: 5)
    }

    func testARunAsksForThePositionAndKeepsItForTheTextItSent() {
        let sql = "select 1;\n\n   selec 2"
        let (model, tab, engine) = rig(sql, caret: 18)
        engine.answer("preview", with: .failure(events: [failure("syntax error at or near \"selec\"", position: 1)],
                                                reason: "exit"))
        model.preview(tab, from: .statement)
        waitForRun(tab)
        XCTAssertEqual(engine.calls.first { $0.command == "preview" }?.env["ERROR_POSITION"], "1")
        let mark = tab.errorMark
        XCTAssertEqual(mark?.sent, SentSQL(text: "selec 2", documentStart: 14))
        XCTAssertEqual(mark?.sqlSnapshot, sql)
        XCTAssertEqual(mark?.range(in: sql as NSString), NSRange(location: 14, length: 5))
        XCTAssertEqual(tab.previewError, "syntax error at or near \"selec\"")
    }

    func testAnEmptySelectionRunMeasuresFromTheDocumentStart() {
        let sql = "select 1;\nselec 2;\nselect 3"
        let (model, tab, engine) = rig(sql, caret: 14)
        engine.answer("preview", with: .failure(events: [failure("syntax error", position: 11)], reason: "exit"))
        model.preview(tab)
        waitForRun(tab)
        XCTAssertEqual(tab.errorMark?.range(in: sql as NSString), NSRange(location: 10, length: 5))
    }

    func testNoPositionNoMark() {
        let (model, tab, engine) = rig("select * from nope")
        engine.answer("preview", with: .failure(events: [failure("relation \"nope\" does not exist", position: nil)],
                                                reason: "exit"))
        model.preview(tab)
        waitForRun(tab)
        XCTAssertNil(tab.errorMark)
        XCTAssertNotNil(tab.previewError)
    }

    func testAnEditWhileTheRunWasInFlightLeavesNoMark() {
        let (model, tab, engine) = rig("selec 1")
        engine.answer("preview", with: .failure(events: [failure("syntax error", position: 1)], reason: "exit"))
        model.preview(tab)
        tab.sql = "selec 1 "
        waitForRun(tab)
        XCTAssertNil(tab.errorMark, "the offset describes text that is no longer in the document")
    }

    func testANewRunRetiresTheLastMark() {
        let (model, tab, engine) = rig("selec 1")
        engine.answer("preview", with: .failure(events: [failure("syntax error", position: 1)], reason: "exit"))
        model.preview(tab)
        waitForRun(tab)
        XCTAssertNotNil(tab.errorMark)
        var done = Event(event: "done")
        done.rows = 0
        engine.answer("preview", with: .events([done]))
        model.preview(tab)
        XCTAssertNil(tab.errorMark, "the mark goes the moment the next Run starts")
        waitForRun(tab)
        XCTAssertNil(tab.errorMark)
    }

    func testOnlyRunAsksForThePosition() {
        let (model, tab, engine) = rig("select 1")
        engine.answer("preview", with: .failure(events: [failure("syntax error", position: 1)], reason: "exit"))
        engine.answer("explain", with: .failure(events: [failure("syntax error", position: 9)], reason: "exit"))
        // The base statement run again, as clearing a server sort does: the document may have changed.
        tab.previewBaseSQL = "select 1"
        model.rerunBaseSQL(tab)
        waitForRun(tab)
        XCTAssertNil(engine.calls.last { $0.command == "preview" }?.env["ERROR_POSITION"])
        XCTAssertNil(tab.errorMark)
        // Explain wraps the text, so its positions are 8 units off.
        model.explain(tab)
        let explained = expectation(description: "explain answered")
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { explained.fulfill() }
        wait(for: [explained], timeout: 2)
        XCTAssertNil(engine.calls.first { $0.command == "explain" }?.env["ERROR_POSITION"])
        XCTAssertNil(tab.errorMark)
    }

    /// A sort or a search wraps the user's text in a derived table, so an offset into it names no place
    /// in the document: neither asks for a position, and neither leaves a mark.
    func testASortAndASearchOnTheServerDoNotAskForAPosition() {
        let (model, tab, engine) = rig("select a from t")
        let column = Event.Column(name: "a", type: "text")
        func showAResult() {
            tab.previewedSQL = "select a from t"
            tab.previewBaseSQL = "select a from t"
            tab.preview = PreviewResult(columns: [column], rowCount: 1, truncated: false, queryID: nil, elapsedMS: 0)
        }
        showAResult()
        engine.answer("preview", with: .failure(events: [failure("syntax error", position: 20)], reason: "exit"))
        model.sortOnServer(tab, column: column, source: 0, direction: .ascending)
        waitForRun(tab)
        XCTAssertNil(engine.calls.last { $0.command == "preview" }?.env["ERROR_POSITION"])
        XCTAssertNil(tab.errorMark)
        showAResult()   // a failed run clears the result it replaced
        model.searchOnServer(tab, term: "needle")
        waitForRun(tab)
        XCTAssertEqual(engine.calls.filter { $0.command == "preview" }.count, 2)
        XCTAssertNil(engine.calls.last { $0.command == "preview" }?.env["ERROR_POSITION"])
        XCTAssertNil(tab.errorMark)
    }

    func testRunReadsThisConnectionsDialect() {
        let (model, tab, engine) = rig("select 1", kind: .mysql)
        engine.answer("preview", with: .events([Event(event: "done", rows: 0)]))
        XCTAssertEqual(tab.dialect, .generic)
        model.preview(tab)
        waitForRun(tab)
        XCTAssertEqual(tab.dialect, .mysql)
    }
}
