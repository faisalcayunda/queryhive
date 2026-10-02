import XCTest
import QueryHiveFFI

@testable import QueryHive

/// W4-T2b: Run splits the way the editor splits. Both read `scan.rs` through the
/// commit-A FFI `sql_statement_ranges`, so the band, the run marks and Run never
/// disagree on where a statement ends.
final class StatementSplitTests: XCTestCase {
    /// The FFI's `(start, end)` pairs as trimmed texts, for a dialect that can differ.
    private func texts(_ sql: String, dialect: EditorDialect) throws -> [String] {
        let flat = try sqlStatementRanges(sql: sql, dialect: dialect)
        let text = sql as NSString
        return stride(from: 0, to: flat.count, by: 2).map {
            text.substring(with: NSRange(location: Int(flat[$0]),
                                         length: Int(flat[$0 + 1]) - Int(flat[$0])))
                .trimmingCharacters(in: .whitespacesAndNewlines)
        }
    }

    /// `sqlStatements` agrees with the FFI wrapper on texts and on ranges.
    private func check(_ sql: String, file: StaticString = #filePath, line: UInt = #line) throws {
        let ranges = try editorStatementRanges(sql)
        let statements = sqlStatements(in: sql)
        let text = sql as NSString
        XCTAssertEqual(statements.map(\.text),
                       ranges.map { text.substring(with: $0)
                           .trimmingCharacters(in: .whitespacesAndNewlines) },
                       "texts differ for \(sql.debugDescription)", file: file, line: line)
        XCTAssertEqual(statements.map { NSRange($0.range, in: sql) }, ranges,
                       "ranges differ for \(sql.debugDescription)", file: file, line: line)
    }

    func testStatementRangesMatchSqlStatements() throws {
        for sql in [
            "",
            "   \n  ",
            "-- only a comment",
            "select 1",
            "select 1;",
            "select 1; select 2;",
            "select 1; select 2",
            "select \"a;b\"",
            "select 'it''s; ok'",
            "select $$a;b$$; select 2",
            "select 1; -- a;b\nselect 2",
            "/* ; */ select 1",
            "select 'héllo😀'; select 2",
            "select 1;\r\nselect 2",
            "  select 1  ;  \n  select 2  ",
        ] as [String] {
            try check(sql)
        }
    }

    func testAQuotedSemicolonIsOneStatement() {
        // The W4-T2b bug: the band showed one statement while Run sent `select "a`.
        XCTAssertEqual(sqlStatements(in: "select \"a;b\"").map(\.text), ["select \"a;b\""])
    }

    func testMySQLBackslashEscapeHidesTheFirstQuote() throws {
        // W3-T0: `'\''` is one string in MySQL, so the `DELETE` after it is code.
        let sql = "SELECT '\\''; DELETE FROM t; -- '"
        XCTAssertEqual(try texts(sql, dialect: .mysql), ["SELECT '\\''", "DELETE FROM t"])
        // Generic reads `''` as a doubled quote and never sees a separator.
        XCTAssertEqual(try texts(sql, dialect: .generic).count, 1)
    }

    func testMySQLHashCommentHidesATrailingSeparator() throws {
        // W3-T0: everything after `#` is a comment in MySQL, so the `;` hides too.
        let sql = "SELECT 1 # note; still comment\nSELECT 2"
        XCTAssertEqual(try texts(sql, dialect: .mysql).count, 1)
        // Generic has no `#` comment: the `;` separates.
        XCTAssertEqual(try texts(sql, dialect: .generic),
                       ["SELECT 1 # note", "still comment\nSELECT 2"])
    }
}
