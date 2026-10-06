import XCTest

@testable import QueryHive

/// The app's half of the engine's `confirm` Safe Mode level.
///
/// The engine is the authority and is tested in Rust (`crates/qh-sql`, `crates/qh-ffi/tests/
/// safe_mode.rs`). What is testable here is the decision that has to exist on this side of the
/// boundary: which statements the app offers to confirm, which it leaves to the engine's own
/// refusal, and the one setting an approved run carries. Nothing in these tests touches an engine.
final class RunConfirmationTests: XCTestCase {
    // MARK: What one statement is

    func testAPlainReadIsReadOnly() {
        for sql in [
            "SELECT 1",
            "select * from people",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "VALUES (1), (2)",
            "TABLE people",
            "SHOW CREATE TABLE people",
            "EXPLAIN SELECT * FROM people",
        ] {
            XCTAssertEqual(StatementScan.classify(sql), .readOnly, sql)
        }
    }

    func testAWriteIsAWriteHoweverItIsHidden() {
        // The same cases the engine's own classifier pins, so the two cannot drift onto opposite
        // answers for the shapes that matter.
        for (sql, kind) in [
            ("INSERT INTO people VALUES (1)", StatementScan.Kind.dml),
            ("update people set a = 1", .dml),
            ("DELETE FROM people", .dml),
            ("MERGE INTO people USING other ON true", .dml),
            ("COPY people FROM STDIN", .dml),
            ("WITH gone AS (DELETE FROM people RETURNING *) SELECT * FROM gone", .dml),
            ("SELECT * FROM people FOR UPDATE", .dml),
            ("SELECT * INTO archive FROM people", .ddl),
            ("DROP TABLE people", .ddl),
            ("CREATE TABLE t (a int)", .ddl),
            ("ALTER TABLE people ADD COLUMN b int", .ddl),
            ("TRUNCATE people", .ddl),
            ("VACUUM people", .ddl),
            ("EXPLAIN ANALYZE SELECT 1", .ddl),
            ("CALL do_something()", .dml),
            ("GRANT SELECT ON people TO reader", .ddl),
        ] {
            XCTAssertEqual(StatementScan.classify(sql), kind, sql)
        }
    }

    func testAKeywordInsideTriviaIsNotAKeyword() {
        for sql in [
            "SELECT 'DROP TABLE people' AS note",
            "SELECT \"delete\" FROM t",
            "SELECT `drop` FROM t",
            "SELECT 1 /* UPDATE t SET a = 1 */",
            "SELECT 1 -- DROP TABLE people",
            // A word that merely contains a keyword is not that keyword.
            "SELECT updated_at, created_at, drop2 FROM t",
            // A PostgreSQL dollar-quoted body is content, not a keyword.
            "SELECT $$UPDATE people SET a = 1$$ AS x",
            "SELECT $tag$DROP TABLE people$tag$ AS x",
        ] {
            XCTAssertEqual(StatementScan.classify(sql), .readOnly, sql)
        }
    }

    func testSomethingTheClassifierCannotReadIsUnknown() {
        for sql in ["", "   ", "-- just a comment", "42", "$1", "`drop`"] {
            XCTAssertEqual(StatementScan.classify(sql), .unknown, sql)
        }
    }

    // MARK: The decision

    func testOnlyConfirmEverAsks() {
        let sql = ["UPDATE people SET a = 1"]
        for mode in [ConnectionSafeMode.full, .noDDL, .readOnly] {
            XCTAssertNil(RunConfirmation.request(for: sql, command: "preview", safeMode: mode),
                         "\(mode.rawValue) never asks")
        }
        let request = RunConfirmation.request(for: sql, command: "preview", safeMode: .confirm)
        XCTAssertEqual(request?.statements, sql)
        XCTAssertEqual(request?.title, "Run this write?")
    }

    func testAReadIsNotAskedAboutEvenAtConfirm() {
        let sql = ["SELECT * FROM people"]
        XCTAssertNil(RunConfirmation.request(for: sql, command: "preview", safeMode: .confirm))
    }

    func testADdlOrUnknownStatementIsLeftToTheEngine() {
        // Confirm refuses DDL and unclassified statements whatever the caller says, so offering to
        // approve one would be a question with no good answer. The app stays quiet and the engine's
        // own refusal is what the user sees.
        for sql in [["DROP TABLE people"], ["CREATE TABLE t (a int)"], ["gibberish here"]] {
            XCTAssertNil(RunConfirmation.request(for: sql, command: "preview", safeMode: .confirm), "\(sql)")
        }
        // A script that holds both a write and a DDL is refused for the DDL, so the whole script is
        // left to the engine rather than half-confirmed.
        XCTAssertNil(RunConfirmation.request(for: ["UPDATE people SET a = 1", "DROP TABLE other"],
                                             command: "preview", safeMode: .confirm))
    }

    func testAScriptAsksOnlyAboutItsWrites() {
        let request = RunConfirmation.request(
            for: ["SELECT * FROM people", "UPDATE people SET a = 1", "SELECT 2"],
            command: "preview", safeMode: .confirm)
        XCTAssertEqual(request?.statements, ["UPDATE people SET a = 1"])
    }

    func testTheApprovalSettingIsForOneRunAndNotStored() {
        XCTAssertEqual(RunConfirmation.approvalSettings(true), ["SAFE_MODE_CONFIRMED": "1"])
        XCTAssertTrue(RunConfirmation.approvalSettings(false).isEmpty)
    }

    // MARK: Destructive table operations

    func testTheTableOperationsAskEvenAboutDdlAtConfirm() {
        // The engine's own exception: `table_op` asks at confirm rather than refusing, because the
        // level exists to ask about exactly these two operations (ADR-0027).
        for operation in TableOperation.allCases {
            let statement = operation.statement(table: "hive.analytics.people")
            let request = RunConfirmation.destructiveRequest(for: statement,
                                                             title: "\(operation.title)?",
                                                             safeMode: .confirm)
            XCTAssertEqual(request?.statements, [statement])
            XCTAssertEqual(request?.title, "\(operation.title)?")
        }
    }

    func testADestructiveOperationAsksAtConfirmAndFullButNotWhereTheEngineRefuses() {
        let statement = TableOperation.drop.statement(table: "people")
        XCTAssertNil(RunConfirmation.destructiveRequest(for: statement, title: "Drop?", safeMode: .noDDL))
        XCTAssertNil(RunConfirmation.destructiveRequest(for: statement, title: "Drop?", safeMode: .readOnly))
        XCTAssertNotNil(RunConfirmation.destructiveRequest(for: statement, title: "Drop?", safeMode: .confirm))
        let full = RunConfirmation.destructiveRequest(for: statement, title: "Drop?", safeMode: .full)
        XCTAssertTrue(full?.note.contains("cannot be undone") == true)
    }

    func testTheApproveButtonNamesTheVerb() {
        let one = RunConfirmation.request(for: ["UPDATE t SET a = 1"], command: "preview", safeMode: .confirm)
        XCTAssertEqual(one?.confirmTitle, "Run Write")
        let two = RunConfirmation.request(for: ["UPDATE t SET a = 1", "DELETE FROM t"],
                                          command: "preview", safeMode: .confirm)
        XCTAssertEqual(two?.confirmTitle, "Run 2 Writes")
        for operation in TableOperation.allCases {
            let request = RunConfirmation.destructiveRequest(
                for: operation.statement(table: "t"), title: "\(operation.title)?", safeMode: .confirm)
            XCTAssertEqual(request?.confirmTitle, operation.title)
        }
    }

    func testTheOperationNamesTheTableAndTheVerb() {
        XCTAssertEqual(TableOperation.drop.statement(table: "\"public\".\"people\""),
                       "DROP TABLE \"public\".\"people\"")
        XCTAssertEqual(TableOperation.truncate.statement(table: "\"public\".\"people\""),
                       "TRUNCATE TABLE \"public\".\"people\"")
    }

    func testTheTargetPartsAllTravelAndTheDriverDropsWhatItDoesNotHave() {
        // Every part is sent; the engine's own slot rules decide which a driver uses, so a
        // Postgres node's empty catalog is the engine's to ignore rather than the app's to guess.
        XCTAssertEqual(TableOperation.targetSettings(catalog: "hive", schema: "analytics", table: "t"),
                       ["TARGET_CATALOG": "hive", "TARGET_SCHEMA": "analytics", "TARGET_TABLE": "t"])
        XCTAssertEqual(TableOperation.targetSettings(catalog: nil, schema: "public", table: "t"),
                       ["TARGET_CATALOG": "", "TARGET_SCHEMA": "public", "TARGET_TABLE": "t"])
    }
}
