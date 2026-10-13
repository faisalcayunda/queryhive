import AppKit
import XCTest

@testable import QueryHive

/// The suggestion list as the app builds it: typing into a real editor, the list opening by
/// itself, and a key accepting a row. The seeded model's tab is on a Trino connection whose
/// default scope, hive.analytics, already holds `wilayah`, `penerima_manfaat` and three more.
@MainActor
final class CompletionEditorTests: XCTestCase {
    private var host: EditorTestHost?

    override func setUp() {
        super.setUp()
        isolateConnectionStore()
    }

    override func tearDown() {
        host?.close()
        host = nil
        super.tearDown()
    }

    private func editor(_ sql: String) throws -> EditorTestHost {
        let made = try EditorTestHost(sql: sql, caret: (sql as NSString).length)
        host = made
        made.textView.setSelectedRange(NSRange(location: (sql as NSString).length, length: 0))
        return made
    }

    private func type(_ text: String, into host: EditorTestHost) {
        let end = (host.textView.string as NSString).length
        host.textView.setSelectedRange(NSRange(location: end, length: 0))
        host.textView.insertText(text, replacementRange: NSRange(location: end, length: 0))
    }

    private func key(_ code: UInt16, in host: EditorTestHost) -> Bool {
        let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [], timestamp: 0,
                                     windowNumber: host.window.windowNumber, context: nil,
                                     characters: "", charactersIgnoringModifiers: "", isARepeat: false,
                                     keyCode: code)!
        return host.coordinator.handle(event)
    }

    private func settled(_ host: EditorTestHost) {
        host.pump(for: 0.4)
    }

    func testTypingAfterFromOpensTheListAndTabWritesTheTable() throws {
        let host = try editor("SELECT * FROM ")
        type("wil", into: host)
        XCTAssertTrue(host.pump(until: { host.model.completion.active }), "the list never opened")
        XCTAssertTrue(host.model.completion.items.first?.text.hasPrefix("wilayah") == true)
        XCTAssertTrue(key(48, in: host))
        XCTAssertEqual(host.textView.string, "SELECT * FROM wilayah w", "the table and a free alias")
        XCTAssertFalse(host.model.completion.active)
    }

    func testOneLetterIsEnoughAfterFrom() throws {
        let host = try editor("SELECT * FROM ")
        type("w", into: host)
        XCTAssertTrue(host.pump(until: { host.model.completion.active }))
        XCTAssertTrue(host.model.completion.items.first?.text.hasPrefix("wilayah") == true)
    }

    func testAnApostropheInAnEarlierCommentDoesNotSilenceTheList() throws {
        let host = try editor("-- jangan lupa filter 'tahun\nSELECT * FROM ")
        type("wil", into: host)
        XCTAssertTrue(host.pump(until: { host.model.completion.active }),
                      "an apostrophe in a comment switched suggestions off for the rest of the file")
    }

    func testTheListOpensOnItsOwnAfterFromAndReturnStaysANewline() throws {
        let host = try editor("SELECT * FROM")
        type(" ", into: host)
        XCTAssertTrue(host.pump(until: { host.model.completion.active }),
                      "nothing opened after FROM and a space")
        XCTAssertTrue(host.model.completion.passive)
        XCTAssertFalse(key(36, in: host), "Return was taken by a list nobody asked for")
        XCTAssertFalse(host.model.completion.active)
    }

    func testEscSilencesTheWordUntilTheNextOne() throws {
        let host = try editor("SELECT * FROM ")
        type("wil", into: host)
        XCTAssertTrue(host.pump(until: { host.model.completion.active }))
        XCTAssertTrue(key(53, in: host))
        XCTAssertFalse(host.model.completion.active)
        type("a", into: host)
        settled(host)
        XCTAssertFalse(host.model.completion.active, "the list came back on the next key of the same word")
        type(", pen", into: host)
        XCTAssertTrue(host.pump(until: { host.model.completion.active }), "the next word did not open it")
    }

    func testAnAliasQualifierOffersItsTablesColumnsOnceTheyAreRead() throws {
        let host = try editor("SELECT w FROM wilayah w")
        let engine = MockEngine()
        engine.answer("columns", with: .events([
            Event(event: "table_columns", fields: ["kode_wilayah", "nama"].map {
                Event.ColumnField(name: $0, type: "varchar", nullable: nil, default: nil, extra: "")
            }),
        ]))
        host.model.engine = engine
        host.textView.setSelectedRange(NSRange(location: 8, length: 0))
        host.textView.insertText(".", replacementRange: NSRange(location: 8, length: 0))

        // The statement is read in the background, the table's columns asked for, and the list
        // fills in when they arrive: all without another key.
        XCTAssertTrue(host.pump(until: { host.model.completion.active }),
                      "the columns never showed up after `w.`")
        XCTAssertEqual(host.model.completion.items.map(\.text), ["nama", "kode_wilayah"])
        XCTAssertEqual(engine.calls.filter { $0.command == "columns" }.count, 1)
        XCTAssertTrue(key(48, in: host))
        XCTAssertEqual(host.textView.string, "SELECT w.nama FROM wilayah w")
    }

    func testATableGetsAnAliasUnlessOneIsAlreadyThere() throws {
        let fresh = try editor("SELECT * FROM ")
        type("wil", into: fresh)
        XCTAssertTrue(fresh.pump(until: { fresh.model.completion.active }))
        XCTAssertTrue(fresh.model.completion.items.first?.text.hasPrefix("wilayah ") == true,
                      "\(fresh.model.completion.items.map(\.text))")
        fresh.close()

        let aliased = try editor("SELECT * FROM wi w")
        aliased.textView.setSelectedRange(NSRange(location: 16, length: 0))
        aliased.textView.insertText("l", replacementRange: NSRange(location: 16, length: 0))
        XCTAssertTrue(aliased.pump(until: { aliased.model.completion.active }))
        XCTAssertEqual(aliased.model.completion.items.first?.text, "wilayah",
                       "an alias was written in front of the one the statement already has")
        XCTAssertTrue(key(48, in: aliased))
        XCTAssertEqual(aliased.textView.string, "SELECT * FROM wilayah w")
    }

    // MARK: Names that need quotes

    /// The seeded catalog `hive` with a hyphenated schema added to the ones it lists.
    private func hyphenated(_ sql: String, caret: Int? = nil) throws -> EditorTestHost {
        let made = try editor(sql)
        let hive = try XCTUnwrap(made.model.tree[0].children?.first)
        let hyphen = TreeNode.schema("prod-ops-aktivitas-produksi-harian-ro", parent: hive)
        hyphen.children = [TreeNode.table("kegiatan_harian", parent: hyphen)]
        hive.children?.append(hyphen)
        hive.children?.append(TreeNode.schema("prod_datalake_pii_ro", parent: hive))
        if let caret { made.textView.setSelectedRange(NSRange(location: caret, length: 0)) }
        return made
    }

    func testAHyphenatedNameTypedWithoutAQuoteIsWrittenQuoted() throws {
        let host = try hyphenated("SELECT * FROM hive.")
        type("prod-o", into: host)
        XCTAssertTrue(host.pump(until: { host.model.completion.active }), "a hyphen stopped the list from opening")
        XCTAssertEqual(host.model.completion.items.map(\.text), ["prod-ops-aktivitas-produksi-harian-ro"])
        XCTAssertTrue(key(48, in: host))
        XCTAssertEqual(host.textView.string, "SELECT * FROM hive.\"prod-ops-aktivitas-produksi-harian-ro\"")
    }

    func testAnOpeningQuoteIsNotDoubled() throws {
        let host = try hyphenated("SELECT * FROM hive.\"prod-o")
        host.textView.insertText("p", replacementRange: NSRange(location: 26, length: 0))
        XCTAssertTrue(host.pump(until: { host.model.completion.active }))
        XCTAssertTrue(key(48, in: host))
        XCTAssertEqual(host.textView.string, "SELECT * FROM hive.\"prod-ops-aktivitas-produksi-harian-ro\"")
    }

    func testAClosingQuoteThatIsAlreadyThereIsReplacedToo() throws {
        // `hive."prod-o|"`: the caret sits before a closing quote.
        let sql = "SELECT * FROM hive.\"prod-o\""
        let host = try hyphenated(sql, caret: (sql as NSString).length - 1)
        host.textView.insertText("p", replacementRange: NSRange(location: (sql as NSString).length - 1, length: 0))
        XCTAssertTrue(host.pump(until: { host.model.completion.active }))
        XCTAssertTrue(key(48, in: host))
        XCTAssertEqual(host.textView.string, "SELECT * FROM hive.\"prod-ops-aktivitas-produksi-harian-ro\"")
    }

    func testASnakeCaseSchemaIsWrittenBare() throws {
        let host = try hyphenated("SELECT * FROM hive.")
        type("prod_d", into: host)
        XCTAssertTrue(host.pump(until: { host.model.completion.active }))
        XCTAssertTrue(key(48, in: host))
        XCTAssertEqual(host.textView.string, "SELECT * FROM hive.prod_datalake_pii_ro")
    }

    func testABareWordWithAHyphenFindsTheSchemaToo() throws {
        let host = try hyphenated("SELECT * FROM ")
        type("prod-o", into: host)
        XCTAssertTrue(host.pump(until: { host.model.completion.active }))
        XCTAssertTrue(key(48, in: host))
        XCTAssertEqual(host.textView.string, "SELECT * FROM \"prod-ops-aktivitas-produksi-harian-ro\"")
    }

    func testAMinusBetweenTwoNamesStillCompletesTheSecond() throws {
        let host = try editor("SELECT 1 FROM wilayah w WHERE kode_wilayah-")
        type("wil", into: host)
        // Nothing is called `kode_wilayah-wil`, so the word is `wil` after a minus, and the
        // minus stays where it is.
        XCTAssertTrue(host.pump(until: { host.model.completion.active }))
        XCTAssertTrue(key(48, in: host))
        XCTAssertEqual(host.textView.string, "SELECT 1 FROM wilayah w WHERE kode_wilayah-wilayah")
    }

    func testPickingARowReplacesTheWholeWordTheCaretIsIn() throws {
        let host = try editor("SELECT * FROM wix tail")
        // The caret sits inside the word: "wi|x", and one more letter makes "wil|x".
        host.textView.setSelectedRange(NSRange(location: 16, length: 0))
        host.textView.insertText("l", replacementRange: NSRange(location: 16, length: 0))
        XCTAssertTrue(host.pump(until: { host.model.completion.active }))
        XCTAssertTrue(key(48, in: host))
        XCTAssertEqual(host.textView.string, "SELECT * FROM wilayah tail",
                       "the tail of the word was left behind the inserted name")
    }

    // MARK: A list that no longer matches the text

    func testAListTheCaretHasLeftIsClosedAndTabIsNotWrittenOverTheWrongWord() throws {
        let sql = "SELECT * FROM hive.\"prod-o"
        let host = try hyphenated(sql)
        host.textView.insertText("p", replacementRange: NSRange(location: (sql as NSString).length, length: 0))
        XCTAssertTrue(host.pump(until: { host.model.completion.active }))

        // ← reaches the editor (the list does not take it) and the caret moves off the word.
        XCTAssertFalse(key(123, in: host))
        host.textView.setSelectedRange(NSRange(location: host.textView.selectedRange().location - 1, length: 0))
        XCTAssertFalse(host.model.completion.active, "the list stayed open over a caret that moved")
        let before = host.textView.string
        XCTAssertFalse(key(48, in: host), "Tab was taken by a list that was closed")
        XCTAssertEqual(host.textView.string, before)
    }

    func testTheSameWithoutAQuoteLeavesTheTextAlone() throws {
        let sql = "SELECT * FROM hive.prod-o"
        let host = try hyphenated(sql)
        host.textView.insertText("p", replacementRange: NSRange(location: (sql as NSString).length, length: 0))
        XCTAssertTrue(host.pump(until: { host.model.completion.active }))
        host.textView.setSelectedRange(NSRange(location: host.textView.selectedRange().location - 2, length: 0))
        let before = host.textView.string
        XCTAssertFalse(key(48, in: host))
        XCTAssertEqual(host.textView.string, before)
    }

    func testTabWithinThePauseAfterAKeyDoesNotWriteTheOldRowOverTheNewWord() throws {
        let host = try editor("SELECT * FROM ")
        let analytics = try XCTUnwrap(host.model.tree[0].children?.first?.children?.first)
        analytics.children?.append(TreeNode.table("My Table", parent: analytics))
        type("\"My", into: host)
        XCTAssertTrue(host.pump(until: { host.model.completion.active }))

        type(" T", into: host)          // the list for `"My` is still up: the pause has not passed
        XCTAssertFalse(host.coordinator.listMatchesForTesting)
        XCTAssertFalse(key(48, in: host), "Tab accepted a row that was built for the previous key")
        XCTAssertEqual(host.textView.string, "SELECT * FROM \"My T")
        XCTAssertFalse(host.model.completion.active)
    }

    func testAnAnswerForTextThatHasMovedOnIsDropped() throws {
        let host = try editor("SELECT * FROM wil")
        host.coordinator.refreshForTesting()           // asks the analysis for the statement...
        type("x", into: host)                           // ...and the text moves before it answers
        host.pump(for: 0.05)                            // long enough to answer, too short for the pause
        XCTAssertFalse(host.model.completion.active,
                       "an answer for `wil` opened a list over `wilx`")
    }

    // MARK: A list nobody asked for

    func testAPassiveListLetsTabAndUpThroughAndOnlyDownStepsIntoIt() throws {
        let host = try editor("SELECT * FROM")
        type(" ", into: host)
        XCTAssertTrue(host.pump(until: { host.model.completion.active }))
        XCTAssertTrue(host.model.completion.passive)
        XCTAssertFalse(key(48, in: host), "Tab was taken by a list nobody asked for")
        XCTAssertFalse(key(126, in: host), "↑ was taken by a list nobody asked for")
        XCTAssertTrue(host.model.completion.passive)
        XCTAssertTrue(key(125, in: host))
        XCTAssertFalse(host.model.completion.passive)
    }

    // MARK: The catalog answers while the person is on a row

    func testASelectionSurvivesTheCatalogAnswering() throws {
        let host = try editor("SELECT * FROM ")
        host.model.engine = MockEngine()      // every load fails quietly: no network
        let analytics = try XCTUnwrap(host.model.tree[0].children?.first?.children?.first)
        analytics.children = nil              // the cold connection: tables not read yet
        type("o", into: host)
        XCTAssertTrue(host.pump(until: { host.model.completion.active }))
        XCTAssertTrue(key(125, in: host))
        XCTAssertTrue(key(125, in: host))
        let chosen = host.model.completion.items[host.model.completion.selected].id
        XCTAssertGreaterThan(host.model.completion.selected, 0)

        // The tables level lands and the list is built again, tables first.
        analytics.children = ["orders", "object_log"].map { TreeNode.table($0, parent: analytics) }
        host.model.completion.catalogLoaded()
        XCTAssertTrue(host.pump(until: { host.model.completion.items.contains { $0.kind == .table } }))
        XCTAssertEqual(host.model.completion.items[host.model.completion.selected].id, chosen,
                       "the row the person was on moved when the tables arrived")
        XCTAssertFalse(host.model.completion.passive)
    }

    func testAListSteppedIntoStaysActiveWhenTheCatalogAnswers() throws {
        let host = try editor("SELECT * FROM")
        host.model.engine = MockEngine()
        let analytics = try XCTUnwrap(host.model.tree[0].children?.first?.children?.first)
        analytics.children = nil
        type(" ", into: host)
        XCTAssertTrue(host.pump(until: { host.model.completion.active }))
        XCTAssertTrue(key(125, in: host))
        analytics.children = [TreeNode.table("orders", parent: analytics)]
        host.model.completion.catalogLoaded()
        XCTAssertTrue(host.pump(until: { host.model.completion.items.contains { $0.kind == .table } }))
        XCTAssertFalse(host.model.completion.passive, "Return became a newline again under the person's hand")
    }

    // MARK: Hyphens and quotes around the word

    func testAHyphenatedWordIsReplacedWholeEvenWithItsTailAfterTheCaret() throws {
        let sql = "SELECT * FROM hive.prod-ops-aktivitas-produksi-harian-ro"
        let host = try hyphenated(sql)
        // The caret sits after `prod-o`, and typing `p` makes `prod-op|s-aktivitas-...`.
        host.textView.setSelectedRange(NSRange(location: 25, length: 0))
        host.textView.insertText("p", replacementRange: NSRange(location: 25, length: 0))
        XCTAssertTrue(host.pump(until: { host.model.completion.active }))
        XCTAssertTrue(key(48, in: host))
        XCTAssertEqual(host.textView.string, "SELECT * FROM hive.\"prod-ops-aktivitas-produksi-harian-ro\"")
    }

    func testAnUnquotedHyphenatedSchemaThenADotListsItsTables() throws {
        let host = try hyphenated("SELECT * FROM hive.prod-ops-aktivitas-produksi-harian-ro")
        type(".", into: host)
        XCTAssertTrue(host.pump(until: { host.model.completion.active }),
                      "a hyphen in the qualifier made the path resolve to `ro` and offer nothing")
        XCTAssertEqual(host.model.completion.items.count, 1)
        XCTAssertTrue(host.model.completion.items.first?.text.hasPrefix("kegiatan_harian") == true)
    }

    func testControlSpaceInsideAQuotedNameWithASpaceDoesNotRepeatItsTail() throws {
        let host = try editor("SELECT * FROM \"My Table\"")
        let analytics = try XCTUnwrap(host.model.tree[0].children?.first?.children?.first)
        analytics.children?.append(TreeNode.table("My Table", parent: analytics))
        host.textView.setSelectedRange(NSRange(location: 17, length: 0))      // `"My|` and ` Table"` after
        host.coordinator.refreshForTesting(manual: true)
        XCTAssertTrue(host.pump(until: { host.model.completion.active }))
        XCTAssertTrue(key(48, in: host))
        let written = host.textView.string
        XCTAssertFalse(written.contains("Table\" Table"), written)
        XCTAssertEqual(written.components(separatedBy: "\"My Table\"").count - 1, 1, written)
    }

    func testControlSpaceBeforeAnotherQuotedNameKeepsThatNameApart() throws {
        let host = try editor("SELECT * FROM \"prod-o from \"x\"")
        let analytics = try XCTUnwrap(host.model.tree[0].children?.first?.children?.first)
        analytics.children?.append(TreeNode.table("prod-orders", parent: analytics))
        host.textView.setSelectedRange(NSRange(location: 21, length: 0))       // `"prod-o|`
        host.coordinator.refreshForTesting(manual: true)
        XCTAssertTrue(host.pump(until: { host.model.completion.active }))
        XCTAssertTrue(key(48, in: host))
        XCTAssertTrue(host.textView.string.hasSuffix(" from \"x\""), host.textView.string)
    }
}
