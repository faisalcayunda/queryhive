import AppKit
import QueryHiveFFI
import XCTest

@testable import QueryHive

/// Format SQL as a text operation: what is formatted, what is refused, and what the selection does.
/// The engine's formatter itself is tested in Rust (`crates/qh-sql/tests/format.rs`); these are about
/// the cut the app makes and how its answer is applied.
final class FormatPlanTests: XCTestCase {
    private func apply(_ outcome: SQLFormat.Outcome, to text: String) -> (text: String, selection: NSRange)? {
        guard case let .edit(range, replacement, selection) = outcome else { return nil }
        return ((text as NSString).replacingCharacters(in: range, with: replacement), selection)
    }

    private let ugly = "select id, nama from wilayah where tahun = 2026 and aktif"

    func testTheWholeTextIsFormattedWhenNothingIsSelected() throws {
        let result = try XCTUnwrap(apply(SQLFormat.plan(text: ugly, selection: NSRange(location: 3, length: 0),
                                                        dialect: .postgres), to: ugly))
        XCTAssertEqual(result.text, "SELECT\n    id,\n    nama\nFROM wilayah\nWHERE tahun = 2026\n    AND aktif")
        XCTAssertEqual(result.selection, NSRange(location: 3, length: 0), "the caret stays inside SELECT")
    }

    func testASelectionFormatsOnlyTheStatementsItTouches() throws {
        let text = "select 1 from a;\nselect x, y from b;\nselect 2 from c"
        let inSecond = (text as NSString).range(of: "x, y")
        let result = try XCTUnwrap(apply(SQLFormat.plan(text: text, selection: inSecond, dialect: .trino), to: text))
        XCTAssertEqual(result.text, "select 1 from a;\nSELECT\n    x,\n    y\nFROM b;\nselect 2 from c")
        XCTAssertEqual((result.text as NSString).substring(with: result.selection), "x,\n    y",
                       "the selection still covers what it covered")
    }

    func testASelectionThatStartsInsideAStringFormatsTheWholeStatement() throws {
        // A raw cut from the middle of the literal would hand the formatter half a string.
        let text = "select 'a b c' as x, y from t"
        let selection = NSRange(location: 9, length: 10)
        let result = try XCTUnwrap(apply(SQLFormat.plan(text: text, selection: selection, dialect: .postgres), to: text))
        XCTAssertEqual(result.text, "SELECT\n    'a b c' AS x,\n    y\nFROM t")
    }

    func testARefusalNamesTheLineAndChangesNothing() {
        let text = "select 1;\nselect 'never closed from t"
        let outcome = SQLFormat.plan(text: text, selection: NSRange(location: 0, length: 0), dialect: .postgres)
        guard case .refused(let reason) = outcome else { return XCTFail("\(outcome)") }
        XCTAssertTrue(reason.contains("line 2"), reason)
        XCTAssertTrue(reason.hasSuffix("Nothing was changed."), reason)
    }

    func testARefusalInASelectionCountsLinesFromTheTopOfTheText() {
        let text = "select 1;\n\nselect 'a\\'b' from t"
        let third = (text as NSString).range(of: "from")
        let outcome = SQLFormat.plan(text: text, selection: third, dialect: .mysql)
        guard case .refused(let reason) = outcome else { return XCTFail("\(outcome)") }
        XCTAssertTrue(reason.contains("line 3"), reason)
    }

    func testBlankTextAndASelectionOfNoStatementAreNothingToDo() {
        XCTAssertEqual(SQLFormat.plan(text: "", selection: NSRange(), dialect: .trino), .nothing)
        XCTAssertEqual(SQLFormat.plan(text: "  \n\t", selection: NSRange(location: 1, length: 0), dialect: .trino), .nothing)
        let text = "select 1;\n\n\nselect 2"
        XCTAssertEqual(SQLFormat.plan(text: text, selection: NSRange(location: 10, length: 2), dialect: .trino), .nothing)
    }

    func testFormattedTextIsLeftAlone() {
        let text = "SELECT\n    id,\n    nama\nFROM wilayah"
        XCTAssertEqual(SQLFormat.plan(text: text, selection: NSRange(), dialect: .postgres), .unchanged)
    }

    func testTheIndentFollowsTheEditorsTabWidthAndKeywordsAreUpperCase() throws {
        let text = "select a, b from t"
        let two = try XCTUnwrap(apply(SQLFormat.plan(text: text, selection: NSRange(), dialect: .postgres,
                                                     options: SQLFormat.options(tabWidth: 2)), to: text))
        XCTAssertEqual(two.text, "SELECT\n  a,\n  b\nFROM t")
        XCTAssertEqual(SQLFormat.options(tabWidth: 4), FormatOptionsWire(indentWidth: 4, keywordCase: .upper))
        XCTAssertEqual(SQLFormat.options(tabWidth: 40).indentWidth, 8)
    }

    func testMySQLKeepsTheCaseItWasWrittenIn() throws {
        // The engine's rule (O-28): in MySQL a keyword's case can be a table name's.
        let text = "select a, b from t"
        let result = try XCTUnwrap(apply(SQLFormat.plan(text: text, selection: NSRange(), dialect: .mysql), to: text))
        XCTAssertEqual(result.text, "select\n    a,\n    b\nfrom t")
    }

    func testTheTabsDialectComesFromItsConnection() {
        XCTAssertEqual(ConnectionKind.trino.editorDialect, .trino)
        XCTAssertEqual(ConnectionKind.postgres.editorDialect, .postgres)
        XCTAssertEqual(ConnectionKind.mysql.editorDialect, .mysql)
    }
}

/// Format SQL as the editor applies it: one undo step, the model written, a refusal that touches
/// nothing, and the menu's way to the editor in front.
@MainActor
final class FormatEditorTests: XCTestCase {
    private var host: EditorTestHost?
    private var savedFrontEditor: (@MainActor () -> SQLEditor.Coordinator?)?

    override func tearDown() {
        host?.close()
        host = nil
        if let savedFrontEditor { AppModel.frontEditor = savedFrontEditor }
        super.tearDown()
    }

    private func editor(_ sql: String) throws -> EditorTestHost {
        let made = try EditorTestHost(sql: sql)
        host = made
        try made.settle()
        return made
    }

    func testFormatIsOneUndoStepApartFromTheTypingBeforeIt() throws {
        let host = try editor("select id, nama from wilayah")
        let undo = try XCTUnwrap(host.textView.undoManager)
        host.textView.setSelectedRange(NSRange(location: (host.textView.string as NSString).length, length: 0))
        host.textView.insertText(" where aktif", replacementRange: host.textView.selectedRange())
        host.pump(for: 0.1)
        let typed = host.textView.string

        let outcome = host.coordinator.formatSQL()
        guard case .edit = outcome else { return XCTFail("\(outcome)") }
        let formatted = "SELECT\n    id,\n    nama\nFROM wilayah\nWHERE aktif"
        XCTAssertEqual(host.textView.string, formatted)
        XCTAssertEqual(host.tab.sql, formatted, "the model is written at once, not 150 ms later")
        XCTAssertEqual(undo.undoActionName, "Format SQL")
        host.pump(for: 0.1)

        undo.undo()
        XCTAssertEqual(host.textView.string, typed, "one undo takes back the format and only the format")
        undo.undo()
        XCTAssertEqual(host.textView.string, "select id, nama from wilayah", "the typing is its own step")
        undo.redo()
        undo.redo()
        XCTAssertEqual(host.textView.string, formatted)
    }

    func testTheFormattedTextIsAnalysedAgain() throws {
        let host = try editor("select a from t; select b from u")
        _ = host.coordinator.formatSQL()
        try host.settle()
        XCTAssertEqual(host.ruler.lineCount, 4, "the gutter counts the new lines")
        XCTAssertEqual(host.textView.string, "SELECT a\nFROM t;\nSELECT b\nFROM u")
    }

    func testARefusalLeavesTheTextAndTheUndoStackAlone() throws {
        let sql = "select 1;\nselect 'never closed from t"
        let host = try editor(sql)
        let undo = try XCTUnwrap(host.textView.undoManager)
        undo.removeAllActions()
        let outcome = host.coordinator.formatSQL()
        guard case .refused(let reason) = outcome else { return XCTFail("\(outcome)") }
        XCTAssertTrue(reason.contains("line 2"), reason)
        XCTAssertEqual(host.textView.string, sql)
        XCTAssertFalse(undo.canUndo)
    }

    func testTheMenuFormatsTheFrontEditorAndLogsARefusalWithoutAnAlert() throws {
        let host = try editor("select 'open")
        savedFrontEditor = AppModel.frontEditor
        AppModel.frontEditor = { host.coordinator }
        host.model.panelExpanded = false
        let spec = try XCTUnwrap(AppMenu.specs(for: .queryhive).first { $0.id == "format" })
        let lines = host.tab.logLines.count
        AppMenu.perform(spec, in: host.model)
        XCTAssertEqual(host.textView.string, "select 'open")
        XCTAssertEqual(host.tab.logLines.count, lines + 1)
        XCTAssertEqual(host.tab.logLines.last?.kind, .warning)
        XCTAssertTrue(host.tab.logLines.last?.text.contains("line 1") == true)
        XCTAssertNil(host.model.notice, "a refusal is not an alert")

        host.textView.selectAll(nil)
        host.textView.insertText("select x, y from t", replacementRange: host.textView.selectedRange())
        host.pump(for: 0.2)
        AppMenu.perform(spec, in: host.model)
        XCTAssertEqual(host.tab.sql, "SELECT\n    x,\n    y\nFROM t")
        XCTAssertEqual(Announcer.last, "SQL formatted")
    }
}

/// Where Format SQL is in the menu, which key it has, and when it can be chosen.
@MainActor
final class FormatMenuTests: XCTestCase {
    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    private func spec(_ scheme: ShortcutScheme = .dbeaver) throws -> MenuSpec {
        try XCTUnwrap(AppMenu.specs(for: scheme).first { $0.id == ShortcutAction.format.rawValue })
    }

    func testFormatSQLIsAnEditMenuItemWithAKeyInBothSchemes() throws {
        XCTAssertEqual(try spec().title, "Format SQL")
        XCTAssertEqual(try spec().group, .edit)
        XCTAssertEqual(try spec(.dbeaver).shortcut?.display, "⌃⇧F", "DBeaver's own binding")
        XCTAssertEqual(try spec(.queryhive).shortcut?.display, "⇧⌘L")
    }

    func testItIsEnabledOnlyForAnEditorWithText() throws {
        let model = AppModel(persistsSession: false)
        model.tabs = []
        XCTAssertFalse(AppMenu.isEnabled(try spec(), in: model), "no tab")
        model.newTab()
        let tab = try XCTUnwrap(model.selectedTab)
        model.panelExpanded = false
        tab.sql = ""
        XCTAssertFalse(AppMenu.isEnabled(try spec(), in: model), "empty")
        tab.sql = " \n\t"
        XCTAssertFalse(AppMenu.isEnabled(try spec(), in: model), "blank")
        tab.sql = "select 1"
        XCTAssertTrue(AppMenu.isEnabled(try spec(), in: model))
        model.panelExpanded = true
        XCTAssertFalse(AppMenu.isEnabled(try spec(), in: model), "the rows fill the window and the editor is gone")
        model.panelExpanded = false
        tab.objectScope = ObjectScope(connectionID: UUID(), catalog: "", schema: "public")
        XCTAssertFalse(AppMenu.isEnabled(try spec(), in: model), "an object tab has no editor")
    }
}
