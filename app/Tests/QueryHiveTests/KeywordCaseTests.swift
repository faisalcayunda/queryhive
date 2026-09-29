import XCTest

@testable import QueryHive

/// Which word gets rewritten, and which does not.
///
/// The decision is pure, so it is tested without a text view. What matters: a keyword is rewritten
/// only once it is finished, only when it is not already canonical, and never when it is part of a
/// qualified name or sitting inside a string.
final class KeywordCaseTests: XCTestCase {
    private func replacement(_ sql: String, caret: Int? = nil) -> KeywordCase.Replacement? {
        KeywordCase.replacement(in: sql as NSString, caret: caret ?? (sql as NSString).length)
    }

    func testAFinishedKeywordIsRewritten() {
        XCTAssertEqual(replacement("select "),
                       KeywordCase.Replacement(range: NSRange(location: 0, length: 6),
                                               text: "SELECT"))
        XCTAssertEqual(replacement("select\n")?.text, "SELECT")
        XCTAssertEqual(replacement("where(")?.text, "WHERE")
        XCTAssertEqual(replacement("a, from;")?.text, "FROM")
    }

    func testAWordStillBeingTypedIsLeftAlone() {
        XCTAssertNil(replacement("selec"))
        XCTAssertNil(replacement("select"))
    }

    func testAKeywordAlreadyCanonicalIsLeftAlone() {
        XCTAssertNil(replacement("SELECT "))
    }

    func testAnIdentifierIsLeftAlone() {
        XCTAssertNil(replacement("selected "))
        XCTAssertNil(replacement("my_table "))
    }

    func testAQualifiedNameIsNotAKeyword() {
        // `hive.from` is a column called from, and its case may be the identifier's own.
        XCTAssertNil(replacement("hive.from "))
    }

    func testAKeywordInsideAStringIsLeftAlone() {
        // The word is finished by the space, and it is a keyword, and it is not code.
        XCTAssertNil(replacement("where note = 'please select '"))
        XCTAssertNil(replacement("-- select "))
        XCTAssertNil(replacement("/* select "))
        XCTAssertNil(replacement("\"select\" "))
    }

    func testTheDecisionIsOnlyAboutTheWordBeforeTheCaret() {
        let sql = "select id from users where "
        // Past the space, so the character just typed is the delimiter and the word is finished.
        let caret = (sql as NSString).range(of: "where").location + 6
        let found = replacement(sql, caret: caret)
        XCTAssertEqual(found?.text, "WHERE")
        XCTAssertEqual(found.map { (sql as NSString).substring(with: $0.range) }, "where")
    }
}

/// The one pass that both the keyword pass and the parameter flow read.
final class SQLScannerTests: XCTestCase {
    func testAParameterInCodeIsFound() {
        let scan = SQLScanner.scan("select * from t where id = :id and name = :name")
        XCTAssertEqual(scan.parameterNames, ["id", "name"])
        XCTAssertEqual(scan.parameters.first?.range, ("select * from t where id = :id" as NSString).range(of: ":id"))
    }

    func testAParameterInsideAStringOrCommentIsNotOne() {
        XCTAssertTrue(SQLScanner.scan("select ':id'").parameters.isEmpty)
        XCTAssertTrue(SQLScanner.scan("select 'it''s :id'").parameters.isEmpty)
        XCTAssertTrue(SQLScanner.scan("-- :id\nselect 1").parameters.isEmpty)
        XCTAssertTrue(SQLScanner.scan("/* :id */ select 1").parameters.isEmpty)
        XCTAssertTrue(SQLScanner.scan("select \"a:id\"").parameters.isEmpty)
        XCTAssertTrue(SQLScanner.scan("select `a:id`").parameters.isEmpty)
        XCTAssertTrue(SQLScanner.scan("select $tag$ :id $tag$").parameters.isEmpty)
    }

    func testACastAndAnAssignmentAreNotParameters() {
        // `::` is a cast and `:=` is an assignment; a colon next to another colon is never a sigil.
        XCTAssertTrue(SQLScanner.scan("select a::text from t").parameters.isEmpty)
        XCTAssertTrue(SQLScanner.scan("select a := 1").parameters.isEmpty)
        // And a colon that is not followed by a name is not one either.
        XCTAssertTrue(SQLScanner.scan("select a from t limit 1; -- trailing:").parameters.isEmpty)
        XCTAssertTrue(SQLScanner.scan("select ':'").parameters.isEmpty)
    }

    func testARepeatedParameterIsOfferedOnce() {
        let scan = SQLScanner.scan("select :id, :id, :other")
        XCTAssertEqual(scan.parameters.count, 3)
        XCTAssertEqual(scan.parameterNames, ["id", "other"])
    }

    func testIsCodeKnowsWhereAStringEnds() {
        let sql = "select 'abc', id from t"
        let scan = SQLScanner.scan(sql)
        let insideString = (sql as NSString).range(of: "abc").location
        let afterString = (sql as NSString).range(of: "id").location
        XCTAssertFalse(scan.isCode(at: insideString))
        XCTAssertTrue(scan.isCode(at: afterString))
    }
}
