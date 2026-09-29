import Foundation

/// The `UPDATE` statements the staged edits become.
///
/// One statement per edited row, with every changed cell of that row in a single `SET` list: a row is
/// the unit the user thinks in, and one statement per row gives one affected-row count per row
/// rather than a pile of them.
///
/// Nothing here runs anything. The statements are built, and what to do with them — show them, run
/// them — is the caller's decision; this type exists so the SQL can be read and tested on its own.
enum UpdateStatements {
    /// One row's update, or the reason it has none.
    struct Update: Equatable {
        /// The fetched-row position this was built for, so a refusal can name it.
        let row: Int
        /// The `UPDATE`, or `nil` when no column of the row can be compared safely. A `nil`
        /// statement is a refusal, not an omission: a predicate with no clauses would have to be
        /// dropped, and a bare `UPDATE t SET …` matches every row in the table.
        let sql: String?
        /// The columns left out of the `WHERE`, with why. Non-empty means the predicate is weaker
        /// than the whole row, so it can match more than the row it was built from; the engine's
        /// affected-row check is what keeps that from being a silent over-write.
        let excluded: [String]
    }

    /// The statements, in row order. A row whose predicate cannot be built is reported with a `nil`
    /// `sql` rather than dropped.
    ///
    /// The identity rule, and the reason it has to be written down: this app carries no primary-key
    /// metadata, so a row is identified by **every column that can be compared safely**, at the
    /// values it was fetched with. That is the strongest predicate available without a key — and it
    /// is not the same as being unique. Two rows identical in every comparable column both answer
    /// it, and the update changes both. That is why the statements are meant to be shown before they
    /// run: a predicate matching more than the row it was built from is visible there, and silent
    /// everywhere else. `MatchPolicy` decides which columns may take part.
    static func generate(edits: CellEdits, rows: [[String?]], columns: [Event.Column],
                         table: String, kind: ConnectionKind) -> [Update] {
        let byRow = Dictionary(grouping: edits.values.keys, by: \.row)
        return byRow.keys.sorted().compactMap { row -> Update? in
            guard rows.indices.contains(row) else { return nil }
            let assignments = (byRow[row] ?? [])
                .sorted { $0.column < $1.column }
                .compactMap { key -> String? in
                    guard columns.indices.contains(key.column),
                          let text = edits.value(at: key) else { return nil }
                    let name = quotedIdent(columns[key.column].name, for: kind)
                    // `DEFAULT` is the sentinel that means "use the column's own default", and it
                    // is a keyword, not a string, so it is never quoted.
                    let expression = isDefaultKeyword(text)
                        ? "DEFAULT"
                        : literal(text, type: columns[key.column].type)
                    return "\(name) = \(expression)"
                }
            guard !assignments.isEmpty else { return nil }
            let match = match(for: rows[row], columns: columns, kind: kind)
            guard !match.isEmpty else {
                return Update(row: row, sql: nil, excluded: match.excluded)
            }
            var sql = "UPDATE \(table) SET \(assignments.joined(separator: ", "))"
            sql += " WHERE \(match.sql)"
            if let note = match.note { sql += " \(note)" }
            return Update(row: row, sql: sql, excluded: match.excluded)
        }
    }

    /// The `WHERE` body that identifies one fetched row, and what could not go into it.
    static func match(for row: [String?], columns: [Event.Column],
                      kind: ConnectionKind) -> RowMatch {
        var clauses: [String] = []
        var excluded: [String] = []
        for index in columns.indices {
            let column = columns[index]
            let name = quotedIdent(column.name, for: kind)
            let policy = MatchPolicy.forColumn(type: column.type, kind: kind)
            if case .excluded(let reason) = policy {
                // Named, not silently dropped: a weaker predicate can match more than one row, and
                // the user has to know that is the check they are about to run.
                excluded.append("\"\(column.name)\" (\(reason))")
                continue
            }
            // A column the row does not carry is NULL, which is what the grid drew for it: `rowView`
            // fills a short row with nil, and nil renders as `null`.
            guard index < row.count, let value = row[index] else {
                clauses.append("\(name) IS NULL")
                continue
            }
            if policy == .serverText {
                // The grid's text is the server's rendering of the value, so compare the server's
                // rendering of the column, not the typed value: a `FLOAT` or a `JSON` fetched as
                // text does not always answer `col = '<that text>'`.
                clauses.append("\(serverText(name, for: kind)) = \(literal(value, type: "varchar"))")
            } else {
                clauses.append("\(name) = \(literal(value, type: column.type))")
            }
        }
        return RowMatch(sql: clauses.joined(separator: " AND "), excluded: excluded)
    }

    /// The server's own text rendering of a column, per driver.
    ///
    /// The study names MySQL's `CONCAT(col)`; PostgreSQL spells the same idea `col::text` and Trino
    /// `CAST(col AS varchar)`.
    static func serverText(_ name: String, for kind: ConnectionKind) -> String {
        switch kind {
        case .mysql: "CONCAT(\(name))"
        case .postgres: "\(name)::text"
        case .trino: "CAST(\(name) AS varchar)"
        }
    }

    /// Whether a cell's text is the `DEFAULT` sentinel — the whole text, trimmed and case-folded,
    /// so `default` and `DEFAULT` mean the same keyword. A literal string that is the word
    /// `DEFAULT` is therefore not expressible through the grid yet; that is stated in ADR-0021.
    static func isDefaultKeyword(_ text: String) -> Bool {
        text.trimmingCharacters(in: .whitespacesAndNewlines).uppercased() == "DEFAULT"
    }

    /// A value as a SQL literal.
    ///
    /// Quoted unless the column's own type says the server reads it as a number or a boolean. The
    /// type comes from the result's own column description, so this is the server's answer about the
    /// column rather than a guess made from the text — which matters, because a numeric-looking
    /// string in a `varchar` column must stay quoted.
    static func literal(_ text: String, type: String) -> String {
        if isNumeric(type) || isBoolean(type) { return text }
        return "'" + text.replacingOccurrences(of: "'", with: "''") + "'"
    }

    /// Whether a type name is one whose literals are written without quotes.
    ///
    /// Compared after the width or precision is stripped, because the servers spell it into the name:
    /// `decimal(38,10)`, `character varying(16)`. Exact rather than by prefix, because a prefix would
    /// read `interval` as `int` and write an interval literal unquoted.
    static func isNumeric(_ type: String) -> Bool {
        let name = MatchPolicy.base(type)
        return ["tinyint", "smallint", "int", "integer", "bigint", "int2", "int4", "int8",
                "serial", "bigserial", "smallserial", "real", "double", "double precision",
                "decimal", "numeric", "float", "float4", "float8", "number"].contains(name)
    }

    static func isBoolean(_ type: String) -> Bool {
        ["bool", "boolean"].contains(MatchPolicy.base(type))
    }
}

/// The `WHERE` body that identifies one fetched row, and the columns it could not use.
struct RowMatch: Equatable {
    /// The predicate's text, or empty when no column can be compared safely.
    let sql: String
    /// Columns left out, named with why.
    let excluded: [String]

    var isEmpty: Bool { sql.isEmpty }

    /// A comment naming the columns left out, for the statement text the review sheet shows. It is
    /// part of the *reviewed* SQL on purpose: the predicate is weaker than the whole row, and the
    /// user should see that before approving it.
    var note: String? {
        excluded.isEmpty ? nil : "/* not matched: \(excluded.joined(separator: "; ")) */"
    }
}
