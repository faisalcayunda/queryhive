import QueryHiveFFI
import XCTest

@testable import QueryHive

/// When the list opens by itself, and what it is asked about. Pure text in, decision out.
final class CompletionTriggerTests: XCTestCase {
    private func site(_ text: String, dialect: EditorDialect = .postgres,
                      hyphens: Bool = false) -> CompletionSite? {
        let ns = text as NSString
        return CompletionTrigger.site(in: ns, caret: ns.length, dialect: dialect, hyphens: hyphens)
    }

    private func opens(_ text: String, clause: EditorClause? = nil, manual: Bool = false,
                       dialect: EditorDialect = .postgres) -> Bool {
        guard let site = site(text, dialect: dialect) else { return false }
        return CompletionTrigger.shouldOpen(site, clause: clause, manual: manual)
    }

    // MARK: Caret state

    func testNothingOpensInsideAStringOrAComment() {
        for text in ["select 'ab", "select 1 -- ab", "select 1 /* ab", "do $$ ab", "select $f$ ab"] {
            XCTAssertNil(site(text), text)
        }
        XCTAssertNotNil(site("select 'a' || ab"))
        XCTAssertNotNil(site("select 1 /* x */ ab"))
        XCTAssertNotNil(site("-- it's\nselect ab"), "an apostrophe in a comment is not a string")
    }

    func testTheDialectDecidesWhatCountsAsAComment() {
        XCTAssertNil(site("select 1 # ab", dialect: .mysql))
        XCTAssertNotNil(site("select 1 # ab", dialect: .postgres))
        // MySQL: a backslash keeps the string open past an escaped quote.
        XCTAssertNil(site("select 'it\\' || ab", dialect: .mysql))
        XCTAssertNotNil(site("select 'it\\' || ab", dialect: .postgres))
        // MySQL reads "..." as a string; the others as a name being quoted.
        XCTAssertNil(site("select \"ab", dialect: .mysql))
        XCTAssertEqual(site("select \"ab", dialect: .postgres)?.quoted, true)
    }

    func testTheStatementStartIsAfterTheLastSemicolonInCode() {
        XCTAssertEqual(site("select 1; select 2; select a")?.statementStart, 19)
        XCTAssertEqual(site("select ';'; select a")?.statementStart, 11)
        XCTAssertEqual(site("select 1 -- ; \n from a")?.statementStart, 0)
    }

    // MARK: Lexical corners

    func testPostgreSQLBlockCommentsNestAndTheOthersDoNot() {
        let nested = "select 1 /* a /* b */ don't */ select ab"
        XCTAssertNotNil(site(nested, dialect: .postgres), "a nested comment switched suggestions off for the rest")
        XCTAssertNil(site("select 1 /* a /* b */ still comment", dialect: .postgres))
        // Elsewhere the first `*/` ends it, so `don't */ ...` opens a string.
        XCTAssertNil(site(nested, dialect: .trino))
    }

    func testPostgreSQLEscapeStringsTakeABackslashAndPlainOnesDoNot() {
        XCTAssertNil(site("select E'it\\'s ab", dialect: .postgres))
        XCTAssertNotNil(site("select E'it\\'s' || ab", dialect: .postgres))
        XCTAssertNotNil(site("select 'it\\' || ab", dialect: .postgres))
        XCTAssertNotNil(site("select name'it' ab", dialect: .postgres), "an e that ends a longer word is no prefix")
    }

    func testMySQLDashesNeedABlankToBeAComment() {
        XCTAssertNotNil(site("select 1--1 + ab", dialect: .mysql), "`1--1` is arithmetic in MySQL")
        XCTAssertNil(site("select 1-- ab", dialect: .mysql))
        XCTAssertNil(site("select 1 --\tab", dialect: .mysql))
        XCTAssertNil(site("select 1--1 + ab", dialect: .postgres))
    }

    // MARK: The word

    func testAnOpenQuoteIsTheStartOfTheWord() {
        let found = site("select * from \"prod-o")
        XCTAssertEqual(found?.prefix, "prod-o")
        XCTAssertEqual(found?.quoted, true)
        XCTAssertEqual(found?.replaceStart, 14, "the replacement starts at the quote")
        let nested = site("select * from \"prod-ops\".pu")
        XCTAssertEqual(nested?.path, ["prod-ops"])
        XCTAssertEqual(nested?.prefix, "pu")
    }

    func testAHyphenatedWordIsOneWordOnlyWhenAsked() {
        XCTAssertEqual(site("select * from prod-o")?.prefix, "o")
        let joined = site("select * from prod-o", hyphens: true)
        XCTAssertEqual(joined?.prefix, "prod-o")
        XCTAssertEqual(joined?.replaceStart, 14)
        // A leading minus is not part of the word, and a space ends it.
        XCTAssertEqual(site("select a -b", hyphens: true)?.prefix, "b")
        XCTAssertEqual(site("select a - b", hyphens: true)?.prefix, "b")
        XCTAssertEqual(site("select * from hive.prod-o", hyphens: true)?.path, ["hive"])
    }

    func testThePreviousWordIsTheKeywordBeforeBlanks() {
        XCTAssertEqual(site("select * from ")?.previousWord, "from")
        XCTAssertEqual(site("select * from\n  ")?.previousWord, "from")
        XCTAssertEqual(site("select * from ord")?.previousWord, "from")
        XCTAssertEqual(site("select a, ")?.previousWord, nil)
    }

    // MARK: Opening

    func testItOpensAfterTwoCharactersAndAfterADot() {
        XCTAssertTrue(opens("select * from or"))
        XCTAssertTrue(opens("select * from hive."))
        XCTAssertTrue(opens("select * from \""))
        XCTAssertFalse(opens("select * from a, "), "an empty word with no keyword in front of it")
    }

    func testOneCharacterIsEnoughWhereANameIsExpected() {
        XCTAssertTrue(opens("select * from o", clause: .queryTable))
        XCTAssertTrue(opens("select * from t where a", clause: .where))
        XCTAssertFalse(opens("s", clause: .other), "one letter at the start of a statement is a keyword")
        XCTAssertFalse(opens("s", clause: nil))
    }

    func testItOpensOnAnEmptyWordAfterTheKeywordsThatTakeAName() {
        for keyword in ["from", "join", "into", "update", "table", "where", "on", "and", "or", "by",
                        "having", "set"] {
            XCTAssertTrue(opens("select 1 \(keyword) "), keyword)
        }
        XCTAssertFalse(opens("select 1 limit "))
        XCTAssertFalse(opens("select "), "no relation is known before a FROM; the list would be keywords")
    }

    func testAnEmptyWordListIsPassiveAndOthersAreNot() throws {
        let empty = try XCTUnwrap(site("select * from "))
        XCTAssertTrue(CompletionTrigger.isPassive(empty, manual: false))
        XCTAssertFalse(CompletionTrigger.isPassive(empty, manual: true))
        XCTAssertFalse(CompletionTrigger.isPassive(try XCTUnwrap(site("select * from o")), manual: false))
        XCTAssertFalse(CompletionTrigger.isPassive(try XCTUnwrap(site("select * from hive.")), manual: false))
    }

    func testManualAlwaysOpens() {
        XCTAssertTrue(opens("s", clause: .other, manual: true))
        XCTAssertTrue(opens("select ", manual: true))
    }

    // MARK: A table that already has an alias

    func testAnAliasThatFollowsTheWordIsRecognised() {
        func follows(_ text: String) -> Bool {
            CompletionTrigger.aliasFollows(in: text as NSString, from: 0)
        }
        XCTAssertTrue(follows(" u where x"))
        XCTAssertTrue(follows(" AS u"))
        XCTAssertTrue(follows("\n  \"my alias\""))
        XCTAssertFalse(follows(""))
        XCTAssertFalse(follows(" where x = 1"))
        XCTAssertFalse(follows(" JOIN b ON true"))
        XCTAssertFalse(follows(", b"))
        XCTAssertFalse(follows(") x"))
        XCTAssertFalse(follows(";"))
        XCTAssertFalse(follows("\ngroup by a"))
    }

    // MARK: Esc

    func testEscSilencesTheWordItClosedUntilTheNextWord() {
        var dismissal = CompletionDismissal()
        XCTAssertFalse(dismissal.suppresses([14]))
        dismissal.dismiss(at: 14)
        XCTAssertTrue(dismissal.suppresses([14]), "the same word, one more character")
        XCTAssertTrue(dismissal.suppresses([14]))
        XCTAssertFalse(dismissal.suppresses([20]), "a new word")
        XCTAssertFalse(dismissal.suppresses([14]), "and the old dismissal is forgotten")
    }
}
