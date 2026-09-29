import XCTest

@testable import QueryHive

/// The grid's server-side sort: the derived-table wrapper the in-memory order escalates to.
///
/// The builder is pure — the statement, the quoting and the NULL placement are all it decides —
/// so it is tested without a server. That the escalation reaches the engine through `preview` is a
/// wiring fact, not something a unit test can run here.
final class ServerSortTests: XCTestCase {
    func testTheStatementIsWrappedNotEdited() throws {
        let statement = try ServerSort.order(sql: "SELECT * FROM hive.analytics.people",
                                             column: "nama",
                                             direction: .ascending,
                                             kind: .postgres)
        // The user's SQL survives byte for byte inside the derived table; the order goes outside.
        XCTAssertTrue(statement.hasPrefix("SELECT * FROM (\nSELECT * FROM hive.analytics.people\n)"))
        XCTAssertTrue(statement.contains("ORDER BY \"nama\" ASC"))
    }

    func testAStatementIsTakenWithoutItsTerminator() throws {
        let statement = try ServerSort.order(sql: "SELECT 1;",
                                             column: "n", direction: .ascending, kind: .trino)
        XCTAssertEqual(statement,
                       "SELECT * FROM (\nSELECT 1\n) AS queryhive_sort ORDER BY \"n\" ASC NULLS LAST")
    }

    func testNullsMirrorTheInMemoryComparatorWhereTheServerCan() throws {
        // Ascending NULLs last, descending first: PostgreSQL's own default, which is the rule
        // `GridSort.compare` follows. Trino spells it too.
        for kind in [ConnectionKind.postgres, .trino] {
            let ascending = try ServerSort.order(sql: "SELECT 1", column: "n",
                                                 direction: .ascending, kind: kind)
            let descending = try ServerSort.order(sql: "SELECT 1", column: "n",
                                                  direction: .descending, kind: kind)
            XCTAssertTrue(ascending.hasSuffix("ORDER BY \"n\" ASC NULLS LAST"), "\(kind) \(ascending)")
            XCTAssertTrue(descending.hasSuffix("ORDER BY \"n\" DESC NULLS FIRST"), "\(kind) \(descending)")
        }
    }

    func testMySQLUsesBackticksAndItsOwnNullPlacement() throws {
        // MySQL has no `NULLS LAST`/`FIRST`, so the clause is left off rather than faked with a
        // second key. The difference is real; the banner names the column and the direction.
        let statement = try ServerSort.order(sql: "SELECT 1", column: "n",
                                             direction: .ascending, kind: .mysql)
        XCTAssertEqual(statement, "SELECT * FROM (\nSELECT 1\n) AS queryhive_sort ORDER BY `n` ASC")
    }

    func testATrailingLineCommentCannotEatTheClosingParenthesis() throws {
        // Without the newlines around the inner statement, a trailing `--` would comment out the
        // wrapper's own tail and the server would refuse the order.
        let statement = try ServerSort.order(sql: "SELECT 1 -- note",
                                             column: "n", direction: .ascending, kind: .trino)
        XCTAssertTrue(statement.contains("-- note\n) AS queryhive_sort"))
    }

    func testAStatementWithMoreThanOnePieceIsRefusedNotGuessed() {
        XCTAssertThrowsError(try ServerSort.order(sql: "SELECT 1; SELECT 2",
                                                  column: "n", direction: .ascending, kind: .trino)) {
            XCTAssertEqual($0 as? ServerSort.Failure, .multipleStatements(2))
        }
    }

    func testABlankColumnIsRefused() {
        XCTAssertThrowsError(try ServerSort.order(sql: "SELECT 1", column: "  ",
                                                  direction: .ascending, kind: .trino)) {
            XCTAssertEqual($0 as? ServerSort.Failure, .noColumn)
        }
    }

    func testTheMarkKeepsTheServersNameNotTheDisplayLabel() {
        // The header can be renamed and columns can move; the order the server was asked for keeps
        // the name it was given.
        let mark = ServerSortMark(column: "nama", direction: .descending)
        XCTAssertEqual(mark, ServerSortMark(column: "nama", direction: .descending))
        XCTAssertNotEqual(mark, ServerSortMark(column: "Nama", direction: .descending))
    }
}
