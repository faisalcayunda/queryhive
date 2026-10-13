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
        engine.answer("preview", with: .events([
            Event(event: "columns", columns: ["kode_wilayah", "nama"].map { Event.Column(name: $0, type: "varchar") }),
        ]))
        host.model.engine = engine
        host.textView.setSelectedRange(NSRange(location: 8, length: 0))
        host.textView.insertText(".", replacementRange: NSRange(location: 8, length: 0))

        // The statement is read in the background, the table's columns asked for, and the list
        // fills in when they arrive: all without another key.
        XCTAssertTrue(host.pump(until: { host.model.completion.active }),
                      "the columns never showed up after `w.`")
        XCTAssertEqual(host.model.completion.items.map(\.text), ["nama", "kode_wilayah"])
        XCTAssertEqual(engine.calls.filter { $0.command == "preview" }.count, 1)
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
        hive.children?.append(TreeNode.schema("prod-ops-aktivitas-produksi-harian-ro", parent: hive))
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
}
