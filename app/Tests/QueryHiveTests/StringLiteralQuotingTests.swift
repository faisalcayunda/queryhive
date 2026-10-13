import QueryHiveFFI
import XCTest

@testable import QueryHive

/// A value written into SQL text as a string literal: the review text of a write-back (which the grid
/// copies to the clipboard "to run by hand", and which Trino executes as written), the cross-column
/// search, and a `:name` parameter.
///
/// The literal has to read back to exactly the value in every dialect: MySQL treats a backslash as an
/// escape (so `a\' OR 1=1 -- ` would close the literal unless the backslash is doubled), PostgreSQL
/// does when `standard_conforming_strings` is off, Trino never. And a quote followed by a combining
/// mark is one `Character` in Swift and two scalars to a server, so every check here walks scalars.
final class StringLiteralQuotingTests: XCTestCase {
    private let kinds: [ConnectionKind] = [.trino, .postgres, .mysql]

    private let values: [String] = {
        var values = [
            #"a\' OR 1=1 -- "#, #"a\'"#, #"\"#, #"trail\"#, #"a\\'"#, #"a\\"#, #"a\b"#, "O'Brien", "'", "''", "plain",
            "ünïcode 名", "x'; DROP TABLE t; --", "a' OR '1'='1", "line\nbreak", "tab\there",
        ]
        for mark in IdentifierQuotingHardeningTests.marks {
            values += ["a'\(mark) OR 1=1 --", "'\(mark)", "x\\\(mark)", "x\\'\(mark); DROP TABLE t; --",
                       "a\\\(mark)' OR 1=1 --"]
        }
        return values
    }()

    private func dialect(_ kind: ConnectionKind) -> EditorDialect {
        switch kind {
        case .trino: .trino
        case .postgres: .postgres
        case .mysql: .mysql
        }
    }

    private func oneStatement(_ sql: String, _ kind: ConnectionKind, _ label: String,
                              file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(sqlStatements(in: sql, dialect: dialect(kind)).count, 1, "\(label): \(sql)",
                       file: file, line: line)
    }

    /// Reads one string literal off the front of `text`, the way the server does, and returns the value
    /// it holds and what follows it. `nil` when the literal never closes.
    private func readLiteral(_ text: ArraySlice<Unicode.Scalar>, _ kind: ConnectionKind)
        -> (value: String, rest: ArraySlice<Unicode.Scalar>)? {
        var index = text.startIndex
        var backslashEscapes = kind == .mysql
        if kind == .postgres, index < text.endIndex, text[index] == "E" || text[index] == "e" {
            backslashEscapes = true
            index += 1
        }
        guard index < text.endIndex, text[index] == "'" else { return nil }
        index += 1
        var value = String.UnicodeScalarView()
        while index < text.endIndex {
            let scalar = text[index]
            if backslashEscapes, scalar == "\\" {
                guard index + 1 < text.endIndex else { return nil }
                value.append(text[index + 1])
                index += 2
                continue
            }
            if scalar == "'" {
                if index + 1 < text.endIndex, text[index + 1] == "'" {
                    value.append("'")
                    index += 2
                    continue
                }
                return (String(value), text[(index + 1)...])
            }
            value.append(scalar)
            index += 1
        }
        return nil
    }

    private func scalars(_ text: String) -> ArraySlice<Unicode.Scalar> { ArraySlice(Array(text.unicodeScalars)) }

    private func units(_ text: String) -> [UInt32] { text.unicodeScalars.map(\.value) }

    private enum Piece {
        case text(String)
        case literal(String)
    }

    /// The whole text must be exactly these pieces in order: fixed text, and string literals that read
    /// back to the given values. Nothing may be left over.
    private func assertPieces(_ text: String, _ kind: ConnectionKind, _ pieces: [Piece], _ label: String,
                              file: StaticString = #filePath, line: UInt = #line) {
        var rest = scalars(text)
        for piece in pieces {
            switch piece {
            case .text(let fixed):
                let pin = Array(fixed.unicodeScalars)
                guard rest.starts(with: pin) else {
                    return XCTFail("\(label): expected \(fixed) in \(text)", file: file, line: line)
                }
                rest = rest.dropFirst(pin.count)
            case .literal(let value):
                guard let read = readLiteral(rest, kind) else {
                    return XCTFail("\(label): no literal where one was expected in \(text)", file: file, line: line)
                }
                XCTAssertEqual(units(read.value), units(value), "\(label): \(text)", file: file, line: line)
                rest = read.rest
            }
        }
        XCTAssertTrue(rest.isEmpty, "\(label): text left after the last piece: \(text)", file: file, line: line)
    }

    // MARK: The shared literal

    func testALiteralReadsBackAsExactlyTheValueInEveryDialect() {
        for kind in kinds {
            for value in values {
                let written = UpdateStatements.literal(value, type: "varchar", kind: kind)
                guard let read = readLiteral(scalars(written), kind) else {
                    XCTFail("\(kind) \(units(value)): \(written)")
                    continue
                }
                XCTAssertEqual(units(read.value), units(value), "\(kind): \(written)")
                XCTAssertTrue(read.rest.isEmpty, "\(kind) \(units(value)): \(written)")
            }
        }
    }

    func testTheExactSpellingOfTheOrdinaryAndTheBackslashCases() {
        XCTAssertEqual(UpdateStatements.literal("O'Brien", type: "varchar", kind: .trino), "'O''Brien'")
        XCTAssertEqual(UpdateStatements.literal("O'Brien", type: "varchar", kind: .mysql), "'O''Brien'")
        XCTAssertEqual(UpdateStatements.literal(#"a\' OR 1=1 -- "#, type: "varchar", kind: .mysql),
                       #"'a\\'' OR 1=1 -- '"#)
        XCTAssertEqual(UpdateStatements.literal(#"a\b"#, type: "varchar", kind: .postgres), #"E'a\\b'"#)
        XCTAssertEqual(UpdateStatements.literal(#"a\b"#, type: "varchar", kind: .trino), #"'a\b'"#)
        XCTAssertEqual(UpdateStatements.literal("a'\u{301} OR 1=1", type: "varchar", kind: .trino),
                       "'a''\u{301} OR 1=1'")
        XCTAssertEqual(UpdateStatements.literal("true", type: "boolean", kind: .mysql), "true")
        XCTAssertEqual(UpdateStatements.literal("42", type: "bigint", kind: .postgres), "42")
    }

    // MARK: The statements the grid writes back

    private func plan(_ kind: ConnectionKind, old: String, new: String, other: String) -> WritePlan {
        let columns = [Event.Column(name: "id", type: "bigint"), Event.Column(name: "c", type: "varchar")]
        var edits = CellEdits()
        edits.edit(new, at: CellKey(row: 0, column: 1), original: old)
        edits.deleteRow(1)
        let added = edits.insertRow()
        edits.setInserted("7", row: added, column: 0)
        edits.setInserted(new, row: added, column: 1)
        return WritePlan.build(edits: edits, rows: [["1", old], ["2", other]], columns: columns, table: "t", kind: kind)
    }

    func testWriteBackReviewTextCarriesEveryValueAsOneLiteralInEveryDialect() throws {
        for kind in kinds {
            let c = kind == .mysql ? "`c`" : "\"c\""
            let id = kind == .mysql ? "`id`" : "\"id\""
            for value in values {
                // The changed text and the other row hold the value; the old cell holds it after a prefix.
                let built = plan(kind, old: "old-" + value, new: value, other: value)
                let byKind = Dictionary(uniqueKeysWithValues: built.statements.map { ($0.kind, $0) })
                let label = "\(kind) \(units(value))"

                let update = try XCTUnwrap(byKind[.update])
                assertPieces(update.sql, kind, [.text("UPDATE t SET \(c) = "), .literal(value),
                                                .text(" WHERE \(id) = 1 AND \(c) = "), .literal("old-" + value)],
                             "update \(label)")
                let delete = try XCTUnwrap(byKind[.delete])
                assertPieces(delete.sql, kind, [.text("DELETE FROM t WHERE \(id) = 2 AND \(c) = "), .literal(value)],
                             "delete \(label)")
                let insert = try XCTUnwrap(byKind[.insert])
                assertPieces(insert.sql, kind, [.text("INSERT INTO t (\(id), \(c)) VALUES (7, "), .literal(value),
                                                .text(")")], "insert \(label)")

                for statement in built.statements {
                    oneStatement(statement.sql, kind, "\(statement.kind) \(label)")
                    oneStatement(statement.boundSQL, kind, "bound \(statement.kind) \(label)")
                }
                if kind == .trino {
                    // Trino has no placeholders: the text it runs is the text that was reviewed.
                    XCTAssertEqual(built.statements.map(\.boundSQL), built.statements.map(\.sql), label)
                } else {
                    XCTAssertFalse(built.statements.map(\.boundSQL).joined().contains("DROP TABLE"),
                                   "\(label): bound statements carry placeholders, not values")
                }
            }
        }
    }

    func testTheMySQLReviewTextOfTheReportedDeleteCannotWidenItsPredicate() throws {
        let hostile = #"a\' OR 1=1 -- "#
        let built = plan(.mysql, old: "x", new: "y", other: hostile)
        let delete = try XCTUnwrap(built.statements.first { $0.kind == .delete })
        XCTAssertEqual(delete.sql, #"DELETE FROM t WHERE `id` = 2 AND `c` = 'a\\'' OR 1=1 -- '"#)
    }

    // MARK: The cross-column search

    func testTheSearchTermStaysOneLiteralInEveryDialect() throws {
        for kind in kinds {
            for value in values {
                let term = value.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !term.isEmpty else { continue }
                let sql = try SearchStatement.crossColumn(sql: "SELECT * FROM t", term: term,
                                                          columns: [Event.Column(name: "c", type: "varchar")], kind: kind)
                let c = kind == .mysql ? "`c`" : "\"c\""
                let prefix: String
                let suffix: String
                switch kind {
                case .mysql:
                    prefix = "SELECT * FROM (\nSELECT * FROM t\n) AS queryhive_search WHERE LOCATE(LOWER("
                    suffix = "), LOWER(CONCAT(\(c)))) > 0"
                case .postgres:
                    prefix = "SELECT * FROM (\nSELECT * FROM t\n) AS queryhive_search WHERE STRPOS(LOWER(\(c)::text), LOWER("
                    suffix = ")) > 0"
                case .trino:
                    prefix = "SELECT * FROM (\nSELECT * FROM t\n) AS queryhive_search WHERE STRPOS(LOWER(CAST(\(c) AS varchar)), LOWER("
                    suffix = ")) > 0"
                }
                assertPieces(sql, kind, [.text(prefix), .literal(term), .text(suffix)], "search \(kind) \(units(term))")
                oneStatement(sql, kind, "search \(kind) \(units(term))")
            }
        }
    }

    // MARK: A `:name` parameter

    func testAParameterTextIsAnExactLiteralOrARefusal() {
        for kind in kinds {
            for value in values {
                let written = ParameterRender.literal(ParameterEntry(kind: .text, text: value), driver: kind)
                guard let written else {
                    // The only refusal is MySQL's backslash, whose meaning depends on `sql_mode`.
                    XCTAssertEqual(kind, .mysql, "\(units(value))")
                    XCTAssertTrue(value.unicodeScalars.contains("\\"), "\(units(value))")
                    continue
                }
                guard let read = readLiteral(scalars(written), kind) else {
                    XCTFail("\(kind) \(units(value)): \(written)")
                    continue
                }
                XCTAssertEqual(units(read.value), units(value), "\(kind): \(written)")
                XCTAssertTrue(read.rest.isEmpty, "\(kind) \(units(value)): \(written)")
            }
        }
    }

    // MARK: The confirmation scanner

    func testAQuoteWithAMarkOnItStillOpensAndClosesALiteralForTheConfirmationScanner() {
        // `'x'` followed by a mark is a closed literal to a server, and what follows is a statement the
        // person must be asked about. A scanner that compares whole Characters would take the closing
        // quote for something else and read `DROP` as still inside the literal.
        for mark in IdentifierQuotingHardeningTests.marks {
            for text in ["SELECT 'x'\(mark); DROP TABLE t", "SELECT \"x\"\(mark); DROP TABLE t",
                         "SELECT `x`\(mark); DROP TABLE t", "SELECT '\(mark)x'; DROP TABLE t",
                         "SELECT 'it''\(mark)s'; DROP TABLE t"] {
                XCTAssertTrue(StatementScan.significantWords(in: text).contains("DROP"),
                              "\(text.unicodeScalars.map(\.value))")
            }
        }
        XCTAssertFalse(StatementScan.significantWords(in: "SELECT 'a DROP b'").contains("DROP"),
                       "a word inside a literal is still not a keyword")
    }

    func testAMySQLBackslashFollowedByAMarkIsStillARefusal() {
        let value = "x\\\u{301}"
        XCTAssertNil(ParameterRender.literal(ParameterEntry(kind: .text, text: value), driver: .mysql))
        XCTAssertNil(ParameterRender.literal(ParameterEntry(kind: .text, text: "a\0\u{301}b"), driver: .postgres),
                     "a null character with a mark on it is still a null character")
    }
}
