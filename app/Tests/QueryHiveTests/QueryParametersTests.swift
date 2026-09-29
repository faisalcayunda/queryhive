import XCTest

@testable import QueryHive

/// How a `:name` value becomes SQL text, which is the whole safety story of the feature.
///
/// There is one function that writes a value into a statement, and it answers `nil` rather than a
/// string it cannot represent. These tests are about that answer: what it accepts, what it refuses,
/// and that the refusal is the only alternative to a literal.
final class QueryParametersTests: XCTestCase {
    private func entry(_ kind: ParameterKind, _ text: String) -> ParameterEntry {
        ParameterEntry(kind: kind, text: text)
    }

    private func literal(_ kind: ParameterKind, _ text: String,
                         driver: ConnectionKind = .trino) -> String? {
        ParameterRender.literal(entry(kind, text), driver: driver)
    }

    // MARK: Numbers

    func testANumberIsWrittenBare() {
        XCTAssertEqual(literal(.number, "42"), "42")
        XCTAssertEqual(literal(.number, "1e9"), "1e9")
        XCTAssertEqual(literal(.number, "0.5"), "0.5")
    }

    func testANegativeNumberIsBracketed() {
        // `x - :n` with `n = -5` is otherwise `x --5`, and that is a line comment.
        XCTAssertEqual(literal(.number, "-5"), "(-5)")
    }

    func testWhatIsNotANumberIsRefused() {
        XCTAssertNil(literal(.number, "abc"))
        XCTAssertNil(literal(.number, "1,5"))
        XCTAssertNil(literal(.number, "1 2"))
        XCTAssertNil(literal(.number, ""))
    }

    // MARK: Text

    func testTextIsQuotedAndItsOwnQuoteIsDoubled() {
        XCTAssertEqual(literal(.text, "O'Brien"), "'O''Brien'")
        XCTAssertEqual(literal(.text, "42"), "'42'")
    }

    func testABackslashDependsOnTheDriver() {
        // PostgreSQL gets the one form that reads the same whichever way standard_conforming_strings
        // is set; MySQL is refused because sql_mode decides what a backslash means and this app
        // cannot see it; Trino has no backslash escapes at all.
        XCTAssertEqual(literal(.text, "a\\b", driver: .postgres), "E'a\\\\b'")
        XCTAssertNil(literal(.text, "a\\b", driver: .mysql))
        XCTAssertEqual(literal(.text, "a\\b", driver: .trino), "'a\\b'")
    }

    func testANullCharacterIsRefusedEverywhere() {
        XCTAssertNil(literal(.text, "a\0b", driver: .trino))
        XCTAssertNil(literal(.text, "a\0b", driver: .postgres))
        XCTAssertNil(literal(.text, "a\0b", driver: .mysql))
    }

    // MARK: Booleans, dates and timestamps

    func testABooleanTakesTheWordOrTheDigit() {
        XCTAssertEqual(literal(.boolean, "true"), "TRUE")
        XCTAssertEqual(literal(.boolean, "FALSE"), "FALSE")
        XCTAssertEqual(literal(.boolean, "1"), "TRUE")
        XCTAssertNil(literal(.boolean, "yes"))
    }

    func testADateIsTypedRatherThanAString() {
        // Trino will not compare a date column against a varchar, and every dialect here reads the
        // typed literal.
        XCTAssertEqual(literal(.date, "2026-01-31"), "DATE '2026-01-31'")
        XCTAssertNil(literal(.date, "2026-1-3"))
        XCTAssertNil(literal(.date, "31/01/2026"))
    }

    func testATimestampTakesTheFormsPeopleWrite() {
        XCTAssertEqual(literal(.timestamp, "2026-01-31 10:00"), "TIMESTAMP '2026-01-31 10:00'")
        XCTAssertEqual(literal(.timestamp, "2026-01-31T10:00:05"),
                       "TIMESTAMP '2026-01-31T10:00:05'")
        XCTAssertEqual(literal(.timestamp, "2026-01-31 10:00:05.123"),
                       "TIMESTAMP '2026-01-31 10:00:05.123'")
        XCTAssertNil(literal(.timestamp, "2026-01-31"))
        XCTAssertNil(literal(.timestamp, "2026-01-31 10"))
        XCTAssertNil(literal(.timestamp, "2026-01-31 10:00:05.123.456"))
    }

    func testNullIsTheWord() {
        XCTAssertEqual(literal(.null, "anything"), "NULL")
    }

    // MARK: The statement

    func testEveryParameterIsWrittenIn() {
        let statement = ParameterRender.statement(
            "select * from t where id = :id and name = :name",
            entries: ["id": entry(.number, "7"), "name": entry(.text, "O'Brien")],
            driver: .trino)
        XCTAssertEqual(try? statement.get(),
                       "select * from t where id = 7 and name = 'O''Brien'")
    }

    func testARepeatedNameIsWrittenTwice() {
        let statement = ParameterRender.statement(
            "select :id, :id", entries: ["id": entry(.number, "1")], driver: .trino)
        XCTAssertEqual(try? statement.get(), "select 1, 1")
    }

    func testAParameterInsideAStringIsLeftAlone() {
        let statement = ParameterRender.statement(
            "select ':id' , :id", entries: ["id": entry(.number, "1")], driver: .trino)
        XCTAssertEqual(try? statement.get(), "select ':id' , 1")
    }

    func testOneBadValueStopsTheWholeStatement() {
        // Half-substituted SQL must never reach the server, so the first refusal is the answer.
        let statement = ParameterRender.statement(
            "select * from t where id = :id and n = :n",
            entries: ["id": entry(.number, "7"), "n": entry(.number, "abc")],
            driver: .trino)
        guard case .failure(let error) = statement else {
            return XCTFail("a value that is not a number should refuse the statement")
        }
        XCTAssertTrue(error.message.contains("n is not a number"), error.message)
    }

    func testAStatementWithNoParametersIsItself() {
        let statement = ParameterRender.statement("select 1", entries: [:], driver: .trino)
        XCTAssertEqual(try? statement.get(), "select 1")
    }
}
