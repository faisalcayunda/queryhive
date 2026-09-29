import XCTest

@testable import QueryHive

/// How a value becomes SQL text, which matters most on the path that writes it straight in.
///
/// Trino cannot bind, so for that driver the literal *is* the statement. A number column used to
/// take whatever was typed without looking at it, which turned a typed `abc` into a bare word in
/// the SQL that ran.
final class UpdateLiteralTests: XCTestCase {
    func testANumberColumnTakesANumberBare() {
        XCTAssertEqual(UpdateStatements.literal("42", type: "int"), "42")
        XCTAssertEqual(UpdateStatements.literal("-3.5", type: "double"), "-3.5")
        XCTAssertEqual(UpdateStatements.literal("1e9", type: "bigint"), "1e9")
        XCTAssertEqual(UpdateStatements.literal("+7", type: "numeric"), "+7")
    }

    func testANumberColumnQuotesWhatIsNotANumber() {
        // The whole point of the change: this used to reach the server as a word, which is either a
        // syntax error or a reference to a column called `abc`.
        XCTAssertEqual(UpdateStatements.literal("abc", type: "int"), "'abc'")
        XCTAssertEqual(UpdateStatements.literal("1,5", type: "decimal"), "'1,5'")
        XCTAssertEqual(UpdateStatements.literal("", type: "int"), "''")
        XCTAssertEqual(UpdateStatements.literal("1 2", type: "int"), "'1 2'")
        XCTAssertEqual(UpdateStatements.literal("1'; DROP TABLE t --", type: "int"),
                       "'1''; DROP TABLE t --'")
    }

    func testABooleanColumnTakesTrueOrFalseBare() {
        XCTAssertEqual(UpdateStatements.literal("true", type: "boolean"), "true")
        XCTAssertEqual(UpdateStatements.literal("FALSE", type: "bool"), "FALSE")
        XCTAssertEqual(UpdateStatements.literal("yes", type: "boolean"), "'yes'")
    }

    func testTextIsQuotedAndItsOwnQuotesAreDoubled() {
        XCTAssertEqual(UpdateStatements.literal("O'Brien", type: "varchar"), "'O''Brien'")
        XCTAssertEqual(UpdateStatements.literal("42", type: "varchar"), "'42'")
    }

    func testTheNumberGrammarRefusesWhatADoubleParseWouldAccept() {
        // `Double("inf")`, `Double("nan")` and `Double("1e400")` all succeed, and none of them is a
        // number any of these dialects will read as one.
        XCTAssertFalse(UpdateStatements.isNumber("inf"))
        XCTAssertFalse(UpdateStatements.isNumber("nan"))
        XCTAssertFalse(UpdateStatements.isNumber("1e"))
        XCTAssertFalse(UpdateStatements.isNumber("--1"))
        XCTAssertFalse(UpdateStatements.isNumber("1 2"))
        XCTAssertFalse(UpdateStatements.isNumber(""))
        XCTAssertFalse(UpdateStatements.isNumber("0x10"))
        // And the forms that are numbers stay numbers.
        XCTAssertTrue(UpdateStatements.isNumber("0"))
        XCTAssertTrue(UpdateStatements.isNumber("1."))
        XCTAssertTrue(UpdateStatements.isNumber(".5"))
        XCTAssertTrue(UpdateStatements.isNumber("-1.5e-3"))
    }
}
