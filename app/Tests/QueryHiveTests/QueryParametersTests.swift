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

    // MARK: The bracket rule

    /// `crates/qh-editor/tests/fixtures/params/cases.tsv`, the table the Rust copy of the rule
    /// reads too: `dialect<TAB>statement<TAB>names`.
    func testTheSharedCaseTable() throws {
        let path = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("crates/qh-editor/tests/fixtures/params/cases.tsv")
        let table = try String(contentsOf: path, encoding: .utf8)
        var rows = 0
        for line in table.split(separator: "\n") where !line.isEmpty {
            let cells = line.split(separator: "\t", omittingEmptySubsequences: false).map(String.init)
            let driver: ConnectionKind = switch cells[0] {
            case "mysql": .mysql
            case "trino": .trino
            default: .postgres  // `generic` reads like PostgreSQL
            }
            let want = cells.count > 2 ? cells[2].split(separator: ",").map(String.init) : []
            let scan = SQLScanner.scan(cells[1], driver: driver)
            XCTAssertEqual(scan.parameters.map(\.name), want, String(line))
            rows += 1
        }
        XCTAssertGreaterThan(rows, 20)
    }

    func testAParameterInsideABracketIsWrittenInOnMySQLAndTrino() {
        let statement = ParameterRender.statement(
            "select ARRAY[:a, :b]", entries: ["a": entry(.number, "1"), "b": entry(.number, "2")],
            driver: .trino)
        XCTAssertEqual(try? statement.get(), "select ARRAY[1, 2]")
    }

    func testASliceKeepsItsColonOnPostgres() {
        let statement = ParameterRender.statement(
            "select arr[lo:hi], ARRAY[:a] from t where id = :id",
            entries: ["a": entry(.number, "1"), "id": entry(.number, "7")], driver: .postgres)
        XCTAssertEqual(try? statement.get(), "select arr[lo:hi], ARRAY[1] from t where id = 7")
    }

    // MARK: Lists

    private func list(_ element: ParameterKind, _ text: String,
                      driver: ConnectionKind = .postgres) -> Result<String, ParameterError> {
        ParameterRender.statement(
            "select * from t where id in (:ids)",
            entries: ["ids": ParameterEntry(kind: .list, text: text, element: element)],
            driver: driver)
    }

    func testAListFillsAnInList() {
        XCTAssertEqual(try? list(.number, "1, 2,3").get(), "select * from t where id in (1, 2, 3)")
        XCTAssertEqual(try? list(.number, "1\n2\n").get(), "select * from t where id in (1, 2)")
        XCTAssertEqual(try? list(.text, "a, O'Brien").get(),
                       "select * from t where id in ('a', 'O''Brien')")
        XCTAssertEqual(try? list(.text, "'a, b', 'it''s'").get(),
                       "select * from t where id in ('a, b', 'it''s')")
        XCTAssertEqual(try? list(.date, "2026-01-31, 2026-02-01").get(),
                       "select * from t where id in (DATE '2026-01-31', DATE '2026-02-01')")
    }

    func testAListNeverBecomesNull() {
        // dbx writes `(NULL)` for these; here each is an error the person can fix.
        for text in ["", "  \n", "1,,2", "1,2,", ",1", "'abc", "'a'b", "'a' 'b'"] {
            guard case .failure = list(.number, text) else {
                return XCTFail("\(text.debugDescription) should have been refused")
            }
        }
    }

    func testOneBadElementRefusesTheList() {
        guard case .failure(let error) = list(.number, "1, abc, 3") else {
            return XCTFail("an element that is not a number should refuse the list")
        }
        XCTAssertTrue(error.message.contains("element 2 (abc) is not a number"), error.message)
        // The element goes through `literal`, so its driver rules apply to it too.
        guard case .failure = list(.text, "a\\b", driver: .mysql) else {
            return XCTFail("a backslash is refused on MySQL, in a list as alone")
        }
    }

    func testAListIsBoundedAndHasNoNullOrListElements() {
        let many = (0...ParameterRender.listLimit).map(String.init).joined(separator: ",")
        guard case .failure = list(.number, many) else { return XCTFail("too long") }
        guard case .failure = list(.null, "1") else { return XCTFail("NULL is not an element type") }
        guard case .failure = list(.list, "1") else { return XCTFail("a list of lists") }
    }
}
