import XCTest

@testable import QueryHive

/// When an identifier is written bare and when it is quoted, per dialect.
final class SQLIdentifierQuotingTests: XCTestCase {
    private let kinds = ConnectionKind.allCases

    func testAPlainLowerCaseNameIsWrittenBareInEveryDialect() {
        for kind in kinds {
            for name in ["id", "nama", "tanggal_lahir", "_hidden", "col1", "a1_b2", "created_at", "status", "type", "date"] {
                XCTAssertFalse(SQLIdentifierQuoting.needsQuotes(name, for: kind), "\(name) in \(kind)")
                XCTAssertEqual(SQLIdentifierQuoting.quoted(name, for: kind), name, "\(name) in \(kind)")
            }
        }
    }

    func testUpperCaseIsQuotedBecauseTheServerFoldsABareNameToLowerCase() {
        XCTAssertEqual(SQLIdentifierQuoting.quoted("Name", for: .postgres), "\"Name\"")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("ID", for: .trino), "\"ID\"")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("Name", for: .mysql), "`Name`")
    }

    func testALeadingDigitASpaceAHyphenAndOtherSymbolsAreQuoted() {
        for name in ["1st", "9", "first name", "first-name", "a.b", "a$b", "a/b", "é", "nama_é", "日本", " x", "x "] {
            for kind in kinds {
                XCTAssertTrue(SQLIdentifierQuoting.needsQuotes(name, for: kind), "\(name) in \(kind)")
            }
        }
        XCTAssertEqual(SQLIdentifierQuoting.quoted("first name", for: .postgres), "\"first name\"")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("1st", for: .mysql), "`1st`")
    }

    func testTheEmptyNameIsQuoted() {
        XCTAssertEqual(SQLIdentifierQuoting.quoted("", for: .postgres), "\"\"")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("", for: .mysql), "``")
    }

    func testAQuoteCharacterInsideTheNameIsDoubledForTheDialect() {
        XCTAssertEqual(SQLIdentifierQuoting.quoted("a\"b", for: .postgres), "\"a\"\"b\"")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("a\"b", for: .trino), "\"a\"\"b\"")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("a`b", for: .mysql), "`a``b`")
        // The other dialect's quote is just a symbol: quoted, and not doubled.
        XCTAssertEqual(SQLIdentifierQuoting.quoted("a`b", for: .postgres), "\"a`b\"")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("a\"b", for: .mysql), "`a\"b`")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("\"", for: .postgres), "\"\"\"\"")
    }

    func testReservedWordsAreQuotedWhateverTheirCase() {
        for kind in kinds {
            for word in ["select", "order", "group", "from", "where", "table", "user", "limit", "offset", "union", "null"] {
                XCTAssertTrue(SQLIdentifierQuoting.needsQuotes(word, for: kind), "\(word) in \(kind)")
                // An upper-case spelling is quoted anyway, for being upper case.
                XCTAssertTrue(SQLIdentifierQuoting.needsQuotes(word.uppercased(), for: kind))
            }
        }
        XCTAssertEqual(SQLIdentifierQuoting.quoted("order", for: .trino), "\"order\"")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("group", for: .mysql), "`group`")
    }

    func testAWordOnlyOneDialectReservesIsBareInTheOthers() {
        // MySQL 8 reserves these; PostgreSQL and Trino take them as column names.
        for word in ["rank", "key", "index", "row", "rows", "system", "show", "usage"] {
            XCTAssertTrue(SQLIdentifierQuoting.needsQuotes(word, for: .mysql), word)
            XCTAssertFalse(SQLIdentifierQuoting.needsQuotes(word, for: .postgres), word)
            XCTAssertFalse(SQLIdentifierQuoting.needsQuotes(word, for: .trino), word)
        }
        // PostgreSQL's and Trino's own.
        XCTAssertTrue(SQLIdentifierQuoting.needsQuotes("ilike", for: .postgres))
        XCTAssertFalse(SQLIdentifierQuoting.needsQuotes("ilike", for: .mysql))
        XCTAssertTrue(SQLIdentifierQuoting.needsQuotes("unnest", for: .trino))
        XCTAssertFalse(SQLIdentifierQuoting.needsQuotes("unnest", for: .postgres))
    }

    func testANameThatMerelyContainsAKeywordIsBare() {
        for kind in kinds {
            for name in ["order_id", "group_name", "selected", "from_date", "user_id", "tablename"] {
                XCTAssertFalse(SQLIdentifierQuoting.needsQuotes(name, for: kind), name)
            }
        }
    }
}
