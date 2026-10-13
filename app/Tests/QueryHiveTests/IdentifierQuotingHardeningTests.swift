import QueryHiveFFI
import XCTest

@testable import QueryHive

/// A catalog, schema, table or column name comes from the server, so it is untrusted text. Every
/// statement this app writes from one has to double the quote character inside it (`"` for Trino and
/// PostgreSQL, a backtick for MySQL) or a name can close the quote and carry SQL after it.
///
/// Each path is driven with names that try to: a quote, a backtick, a name that closes the quote and
/// appends `; DROP TABLE x --`, plus the ordinary awkward ones (space, upper case, leading digit,
/// reserved word, unicode). "Stays one identifier" is checked two ways that do not share code with the
/// implementation: a small reader that undoes the quoting and must consume the whole name, and the
/// engine's own statement splitter, which must still see a single statement.
final class IdentifierQuotingHardeningTests: XCTestCase {
    /// Combining and format scalars that Swift fuses with the scalar before them into one `Character`,
    /// so a `"` followed by one is no longer `"` to a Character-level comparison while every server
    /// still reads the quote byte.
    static let marks = ["\u{301}", "\u{200D}", "\u{200C}", "\u{FE0F}", "\u{202E}", "\u{0300}\u{0301}"]

    private let hostile: [String] = {
        var names = [
            #"a"b"#, "a`b", "my table", "UPPER", "1st", "select", "ünïcode_名",
            #"x"; DROP TABLE x --"#, "x`; DROP TABLE x --", "x\"", "x`", "\"", "`", "\"\"", "a.b",
        ]
        for mark in IdentifierQuotingHardeningTests.marks {
            names += ["x\"\(mark); DROP TABLE t; --", "x`\(mark); DROP TABLE t; --",
                      "x\"\(mark) WHERE public.evil() IS NULL --", "\"\(mark)", "`\(mark)", mark, "a\(mark)b"]
        }
        return names
    }()
    private let kinds: [ConnectionKind] = [.trino, .postgres, .mysql]

    private func dialect(_ kind: ConnectionKind) -> EditorDialect {
        switch kind {
        case .trino: .trino
        case .postgres: .postgres
        case .mysql: .mysql
        }
    }

    /// Reads one quoted identifier off the front of `text` the way every server does, byte for byte:
    /// a doubled quote is a quote in the name, a single one ends it. It walks `unicodeScalars`, not
    /// `Character`s, because a quote followed by a combining mark is one Character in Swift and two
    /// scalars to a server. Returns the name and what is left.
    private func readIdentifier(_ text: ArraySlice<Unicode.Scalar>, quote: Unicode.Scalar)
        -> (name: String, rest: ArraySlice<Unicode.Scalar>)? {
        guard text.first == quote else { return nil }
        var name = String.UnicodeScalarView()
        var index = text.startIndex + 1
        while index < text.endIndex {
            let scalar = text[index]
            if scalar == quote {
                if index + 1 < text.endIndex, text[index + 1] == quote {
                    name.append(quote)
                    index += 2
                    continue
                }
                return (String(name), text[(index + 1)...])
            }
            name.append(scalar)
            index += 1
        }
        return nil
    }

    private func scalars(_ text: String) -> ArraySlice<Unicode.Scalar> { ArraySlice(Array(text.unicodeScalars)) }

    /// Splits `a.b.c` where every part is a quoted identifier into its names, or `nil` when anything
    /// but a dot sits between two of them (that is, when a name broke out of its quotes).
    private func readQualified(_ text: String, quote: Unicode.Scalar) -> [String]? {
        var parts: [String] = []
        var rest = scalars(text)
        while true {
            guard let (name, remainder) = readIdentifier(rest, quote: quote) else { return nil }
            parts.append(name)
            if remainder.isEmpty { return parts }
            guard remainder.first == "." else { return nil }
            rest = remainder.dropFirst()
        }
    }

    private func quote(_ kind: ConnectionKind) -> Unicode.Scalar { kind == .mysql ? "`" : "\"" }

    /// The expected spelling of an identifier, built scalar by scalar without the implementation.
    private func ident(_ name: String, _ kind: ConnectionKind) -> String {
        let q = quote(kind)
        var out = String.UnicodeScalarView()
        out.append(q)
        for scalar in name.unicodeScalars {
            out.append(scalar)
            if scalar == q { out.append(q) }
        }
        out.append(q)
        return String(out)
    }

    /// How many times `needle` occurs in `text`, compared scalar by scalar.
    private func occurrences(of needle: String, in text: String) -> Int {
        let hay = Array(text.unicodeScalars)
        let pin = Array(needle.unicodeScalars)
        guard !pin.isEmpty, hay.count >= pin.count else { return 0 }
        return (0...(hay.count - pin.count)).filter { Array(hay[$0..<($0 + pin.count)]) == pin }.count
    }

    private func count(of scalar: Unicode.Scalar, in text: String) -> Int {
        text.unicodeScalars.filter { $0 == scalar }.count
    }

    private func assertOneStatement(_ sql: String, _ kind: ConnectionKind, _ label: String,
                                    file: StaticString = #filePath, line: UInt = #line) {
        let statements = sqlStatements(in: sql, dialect: dialect(kind))
        XCTAssertEqual(statements.count, 1, "\(label): \(sql)", file: file, line: line)
    }

    // MARK: The shared helpers

    func testAnEmbeddedQuoteIsDoubledForTheDialectsOwnQuote() {
        XCTAssertEqual(quotedIdent(#"a"b"#, for: .postgres), #""a""b""#)
        XCTAssertEqual(quotedIdent(#"a"b"#, for: .trino), #""a""b""#)
        XCTAssertEqual(quotedIdent("a`b", for: .mysql), "`a``b`")
        // The other dialect's quote is just a character.
        XCTAssertEqual(quotedIdent("a`b", for: .postgres), "\"a`b\"")
        XCTAssertEqual(quotedIdent(#"a"b"#, for: .mysql), "`a\"b`")
        XCTAssertEqual(quotedIdent(#"x"; DROP TABLE x --"#, for: .postgres), #""x""; DROP TABLE x --""#)
        XCTAssertEqual(quotedIdent("x`; DROP TABLE x --", for: .mysql), "`x``; DROP TABLE x --`")
    }

    func testOrdinaryNamesKeepTheirExactBytes() {
        for kind in kinds {
            let m = String(Character(quote(kind)))
            XCTAssertEqual(quotedIdent("nama", for: kind), m + "nama" + m)
            XCTAssertEqual(quotedIdent("Nama Lengkap", for: kind), m + "Nama Lengkap" + m)
        }
        XCTAssertEqual(qualifiedName(database: "hive", schema: "analytics", table: "wilayah", for: .trino),
                       #""hive"."analytics"."wilayah""#)
        XCTAssertEqual(qualifiedName(database: nil, schema: "public", table: "wilayah", for: .postgres),
                       #""public"."wilayah""#)
        XCTAssertEqual(qualifiedName(database: "qh", schema: nil, table: "wilayah", for: .mysql),
                       "`qh`.`wilayah`")
    }

    func testEveryNameStaysOneIdentifierInEveryPartOfAQualifiedName() throws {
        for kind in kinds {
            for name in hostile {
                let q = quote(kind)
                let written = quotedIdent(name, for: kind)
                // Built from the scalars, not from the implementation: each quote scalar in the name
                // appears twice, and the two wrapping quotes are the only others.
                var independent = [q]
                for scalar in name.unicodeScalars { independent += scalar == q ? [q, q] : [scalar] }
                independent.append(q)
                XCTAssertEqual(Array(written.unicodeScalars), independent, "\(kind) \(name.unicodeScalars.map(\.value))")
                XCTAssertEqual(count(of: q, in: written), 2 + 2 * count(of: q, in: name), "\(kind)")
                let single = try XCTUnwrap(readIdentifier(scalars(written), quote: q))
                XCTAssertEqual(Array(single.name.unicodeScalars), Array(name.unicodeScalars), "\(kind)")
                XCTAssertTrue(single.rest.isEmpty, "\(kind) \(name)")

                // The hostile name in each slot the driver has, the others ordinary.
                let slots: [(String?, String?, String)] = [(name, "s", "t"), ("c", name, "t"), ("c", "s", name)]
                for (database, schema, table) in slots {
                    guard let text = qualifiedName(database: database, schema: schema, table: table, for: kind)
                    else { continue }
                    let expected: [String] = switch kind {
                    case .trino: [database!, schema!, table]
                    case .postgres: [schema!, table]
                    case .mysql: [database!, table]
                    }
                    XCTAssertEqual(readQualified(text, quote: q), expected, "\(kind): \(text)")
                    assertOneStatement("SELECT * FROM \(text)", kind, "\(kind) \(name)")
                }
            }
        }
    }

    // MARK: The tree, the inspector and the opened table

    private func tableNode(_ kind: ConnectionKind, catalog: String, schema: String, table: String) -> TreeNode {
        let connection = TreeNode.connection(Connection(id: UUID(), name: "c", color: .blue, kind: kind, host: "h",
                                                        port: 1, sslmode: "", user: "u", database: "", schema: "",
                                                        verify: true))
        switch kind {
        case .trino:
            return .table(table, parent: .schema(schema, parent: .catalog(catalog, parent: connection)))
        case .postgres:
            return .table(table, parent: .schema(schema, parent: connection))
        case .mysql:
            return .table(table, parent: .database(catalog, parent: connection))
        }
    }

    func testTheTreesInsertableTextAndTheTableItOpensKeepAHostileNameInItsSlot() throws {
        for kind in kinds {
            for name in hostile {
                let node = tableNode(kind, catalog: name, schema: name, table: name)
                let text = try XCTUnwrap(node.insertableText)
                let parts = try XCTUnwrap(readQualified(text, quote: quote(kind)), "\(kind): \(text)")
                XCTAssertEqual(parts, Array(repeating: name, count: kind == .trino ? 3 : 2), "\(kind)")

                // `openTable` writes `SELECT *` first and a column list after; both hold the table.
                let star = OpenTableStatement.sql(table: text, columns: [], kind: kind)
                assertOneStatement(star, kind, "open \(kind) \(name)")
                let list = OpenTableStatement.sql(table: text, columns: [name, "id"], kind: kind)
                assertOneStatement(list, kind, "open list \(kind) \(name)")
            }
        }
    }

    func testTheInspectorsColumnReadQuotesTheTableAndItsFallbackToo() throws {
        for kind in kinds {
            for name in hostile {
                let sql = objectColumnsSQL(database: "c", schema: "s", table: name, for: kind)
                assertOneStatement(sql, kind, "inspector \(kind) \(name)")
                XCTAssertTrue(sql.hasPrefix("SELECT * FROM "))
                let target = String(sql.dropFirst("SELECT * FROM ".count))
                let parts = try XCTUnwrap(readQualified(target, quote: quote(kind)), "\(kind): \(sql)")
                XCTAssertEqual(parts.last, name)
            }
        }
        // No catalog on Trino means the name cannot be qualified; the bare name is the server's to
        // resolve, but it still goes in as one identifier. A plain name keeps its bytes.
        XCTAssertEqual(objectColumnsSQL(database: nil, schema: "s", table: "wilayah", for: .trino),
                       "SELECT * FROM wilayah")
        XCTAssertEqual(objectColumnsSQL(database: nil, schema: "s", table: #"x"; DROP TABLE x --"#, for: .trino),
                       #"SELECT * FROM "x""; DROP TABLE x --""#)
        XCTAssertEqual(objectColumnsSQL(database: nil, schema: nil, table: "x`; DROP TABLE x --", for: .mysql),
                       "SELECT * FROM `x``; DROP TABLE x --`")
    }

    // MARK: Statements built from a result's column names

    private func columns(_ name: String) -> [Event.Column] {
        [Event.Column(name: "id", type: "bigint"), Event.Column(name: name, type: "varchar")]
    }

    func testTheGridsServerSortAndSearchQuoteAHostileColumn() throws {
        for kind in kinds {
            for name in hostile where name.trimmingCharacters(in: .whitespaces) == name {
                let sorted = try ServerSort.order(sql: "SELECT * FROM t", column: name, direction: .ascending, kind: kind)
                assertOneStatement(sorted, kind, "sort \(kind) \(name)")
                let tail = try XCTUnwrap(sorted.components(separatedBy: " ORDER BY ").last)
                let read = try XCTUnwrap(readIdentifier(scalars(tail), quote: quote(kind)), "\(kind): \(sorted)")
                XCTAssertEqual(Array(read.name.unicodeScalars), Array(name.unicodeScalars), "\(kind)")
                let nulls = kind == .mysql ? "" : " NULLS LAST"
                XCTAssertEqual(sorted, "SELECT * FROM (\nSELECT * FROM t\n) AS queryhive_sort ORDER BY \(self.ident(name, kind)) ASC\(nulls)")

                let search = try SearchStatement.crossColumn(sql: "SELECT * FROM t", term: "abc",
                                                             columns: columns(name), kind: kind)
                assertOneStatement(search, kind, "search \(kind) \(name)")
                let clauses = ["id", name].map { column -> String in
                    let q = self.ident(column, kind)
                    switch kind {
                    case .mysql: return "LOCATE(LOWER('abc'), LOWER(CONCAT(\(q)))) > 0"
                    case .postgres: return "STRPOS(LOWER(\(q)::text), LOWER('abc')) > 0"
                    case .trino: return "STRPOS(LOWER(CAST(\(q) AS varchar)), LOWER('abc')) > 0"
                    }
                }
                XCTAssertEqual(search, "SELECT * FROM (\nSELECT * FROM t\n) AS queryhive_search WHERE "
                               + clauses.joined(separator: " OR "))
            }
        }
    }

    private func plan(_ kind: ConnectionKind, column name: String, table: String) -> WritePlan {
        var edits = CellEdits()
        edits.edit("changed", at: CellKey(row: 0, column: 1), original: "old")
        edits.deleteRow(1)
        let added = edits.insertRow()
        edits.setInserted("7", row: added, column: 0)
        edits.setInserted("new", row: added, column: 1)
        return WritePlan.build(edits: edits, rows: [["1", "old"], ["2", "other"]], columns: columns(name),
                               table: table, kind: kind)
    }

    func testWriteBackStatementsQuoteAHostileColumnAndStayOneStatementEach() throws {
        for kind in kinds {
            for name in hostile {
                let node = tableNode(kind, catalog: "c", schema: "s", table: "t")
                let table = try XCTUnwrap(node.insertableText)
                let built = plan(kind, column: name, table: table)
                XCTAssertEqual(built.statements.map(\.kind), [.delete, .update, .insert], "\(kind) \(name)")
                let named = ident(name, kind)
                let id = ident("id", kind)
                for statement in built.statements {
                    assertOneStatement(statement.sql, kind, "\(statement.kind) \(kind) \(name)")
                    assertOneStatement(statement.boundSQL, kind, "bound \(statement.kind) \(kind) \(name)")
                }
                let byKind = Dictionary(uniqueKeysWithValues: built.statements.map { ($0.kind, $0.sql) })
                XCTAssertEqual(byKind[.update],
                               "UPDATE \(table) SET \(named) = 'changed' WHERE \(id) = 1 AND \(named) = 'old'",
                               "\(kind) \(name.unicodeScalars.map(\.value))")
                XCTAssertEqual(byKind[.delete],
                               "DELETE FROM \(table) WHERE \(id) = 2 AND \(named) = 'other'",
                               "\(kind) \(name.unicodeScalars.map(\.value))")
                XCTAssertEqual(byKind[.insert],
                               "INSERT INTO \(table) (\(id), \(named)) VALUES (7, 'new')",
                               "\(kind) \(name.unicodeScalars.map(\.value))")
            }
        }
    }

    func testTheNoteNamingAnUnmatchedColumnCannotCloseItsComment() throws {
        // A column the predicate cannot compare (binary here) is named in a `/* … */` note that is part
        // of the statement. A name holding `*/` would end the comment and run what follows.
        var evil = ["x */; DROP TABLE t; /*", "*/ DELETE FROM t; --", "/*", "*/", "a*/*b"]
        for mark in Self.marks { evil += ["x */\(mark); DROP TABLE t; --", "x /*\(mark) y", "*\(mark)/", "/\(mark)*", "*/\(mark)", "\(mark)*/"] }
        for kind in kinds {
            for name in evil {
                let cols = [Event.Column(name: "id", type: "bigint"), Event.Column(name: "nama", type: "varchar"),
                            Event.Column(name: name, type: "bytea")]
                var edits = CellEdits()
                edits.edit("changed", at: CellKey(row: 0, column: 1), original: "old")
                edits.deleteRow(1)
                let built = WritePlan.build(edits: edits, rows: [["1", "old", nil], ["2", "other", nil]], columns: cols,
                                            table: "t", kind: kind)
                XCTAssertEqual(built.statements.map(\.kind), [.delete, .update], "\(kind) \(name)")
                for statement in built.statements {
                    for text in [statement.sql, statement.boundSQL] {
                        let label = "\(statement.kind) \(kind) \(name)"
                        XCTAssertTrue(text.contains("not matched"), "\(label): \(text)")
                        let bytes = name.unicodeScalars.map(\.value)
                        XCTAssertEqual(occurrences(of: "*/", in: text), 1,
                                       "\(label) \(bytes): the comment closes once, at its end: \(text)")
                        XCTAssertEqual(occurrences(of: "/*", in: text), 1,
                                       "\(label) \(bytes): no comment opens inside the note: \(text)")
                        XCTAssertTrue(text.hasSuffix("*/"), "\(label): \(text)")
                        assertOneStatement(text, kind, label)
                    }
                }
            }
        }
        // An ordinary name is shown as it is.
        let plain = UpdateStatements.match(for: ["1", nil], columns: [Event.Column(name: "id", type: "bigint"),
                                                                        Event.Column(name: "blob col", type: "bytea")],
                                           kind: .postgres)
        XCTAssertEqual(plain.note, #"/* not matched: "blob col" (binary values are not comparable as text) */"#)
    }
}
