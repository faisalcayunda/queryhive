import Foundation

/// The `UPDATE` statements the staged edits become.
///
/// One statement per edited row, with every changed cell of that row in a single `SET` list: a row is
/// the unit the user thinks in, and one statement per row gives one affected-row count per row
/// rather than a pile of them.
///
/// The statement is built as a `BoundSQL`, so the driver that can bind gets placeholders and the one
/// that cannot gets the literals — from the same pass, so the reviewed statement and the run
/// statement cannot disagree about which values are where.
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
        let bound: BoundSQL?
        /// The columns left out of the `WHERE`, with why. Non-empty means the predicate is weaker
        /// than the whole row, so it can match more than the row it was built from; the engine's
        /// affected-row check is what keeps that from being a silent over-write.
        let excluded: [String]

        /// The review form of the statement, or `nil` for a refusal. The run form
        /// is `bound`; this is what the sheet shows.
        var sql: String? { bound?.display }
    }

    /// The statements, in row order. A row whose predicate cannot be built is reported with a `nil`
    /// statement rather than dropped.
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
        let style = ParameterStyle.forKind(kind)
        let byRow = Dictionary(grouping: edits.values.keys, by: \.row)
        return byRow.keys.sorted().compactMap { row -> Update? in
            guard rows.indices.contains(row) else { return nil }
            // Each assignment is (identifier, value to bind, literal to show). A `DEFAULT` sentinel
            // has no value to bind: it is a keyword, and a keyword is never a parameter.
            let assignments = (byRow[row] ?? [])
                .sorted { $0.column < $1.column }
                .compactMap { key -> (String, BindValue?, String)? in
                    guard columns.indices.contains(key.column),
                          let text = edits.value(at: key) else { return nil }
                    let name = quotedIdent(columns[key.column].name, for: kind)
                    if isDefaultKeyword(text) { return (name, nil, "DEFAULT") }
                    let type = columns[key.column].type
                    return (name, BindValue.from(text: text, type: type), literal(text, type: type))
                }
            guard !assignments.isEmpty else { return nil }

            var bound = BoundSQL(style: style)
            bound.text("UPDATE \(table) SET ")
            for (offset, assignment) in assignments.enumerated() {
                if offset > 0 { bound.text(", ") }
                bound.text("\(assignment.0) = ")
                if let value = assignment.1 {
                    bound.value(value, display: assignment.2)
                } else {
                    // `DEFAULT` is the sentinel that means "use the column's own default", and it
                    // is a keyword, not a string, so it is never quoted and never bound.
                    bound.text(assignment.2)
                }
            }
            bound.text(" WHERE ")
            let match = appendMatch(for: rows[row], columns: columns, kind: kind, into: &bound)
            guard match.clauses > 0 else {
                return Update(row: row, bound: nil, excluded: match.excluded)
            }
            if let note = match.note { bound.text(" \(note)") }
            return Update(row: row, bound: bound, excluded: match.excluded)
        }
    }

    /// Append the `WHERE` body that identifies one fetched row, and report what could not go into it.
    ///
    /// The clauses are appended to `bound`, which already carries the driver's `ParameterStyle`, so
    /// a value in the predicate is a placeholder for a driver that binds and a literal for one that
    /// does not. The number of clauses is returned beside the excluded columns so a caller can tell
    /// "no predicate at all" from "a predicate with exclusions".
    static func appendMatch(for row: [String?], columns: [Event.Column], kind: ConnectionKind,
                            into bound: inout BoundSQL) -> MatchOutcome {
        var clauses = 0
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
            if clauses > 0 { bound.text(" AND ") }
            clauses += 1
            // A column the row does not carry is NULL, which is what the grid drew for it: `rowView`
            // fills a short row with nil, and nil renders as `null`.
            guard index < row.count, let value = row[index] else {
                bound.text("\(name) IS NULL")
                continue
            }
            if policy == .serverText {
                // The grid's text is the server's rendering of the value, so compare the server's
                // rendering of the column, not the typed value: a `FLOAT` or a `JSON` fetched as
                // text does not always answer `col = '<that text>'`.
                bound.text("\(serverText(name, for: kind)) = ")
                bound.value(BindValue.from(text: value, type: "varchar"),
                            display: literal(value, type: "varchar"))
            } else {
                bound.text("\(name) = ")
                bound.value(BindValue.from(text: value, type: column.type),
                            display: literal(value, type: column.type))
            }
        }
        return MatchOutcome(clauses: clauses, excluded: excluded)
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

    /// The predicate for one row, in the review form, plus what was excluded.
    ///
    /// A convenience over `appendMatch` for a caller that only wants to read the predicate text;
    /// the planner appends into the statement's own `BoundSQL` so the values are bound in place.
    static func match(for row: [String?], columns: [Event.Column], kind: ConnectionKind) -> Match {
        var bound = BoundSQL(style: ParameterStyle.forKind(kind))
        let outcome = appendMatch(for: row, columns: columns, kind: kind, into: &bound)
        return Match(sql: bound.display, excluded: outcome.excluded)
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
    ///
    /// This is the **review** form. The executable form for a driver that binds is a placeholder,
    /// and the value is the typed `BindValue` beside it; the two are produced together.
    static func literal(_ text: String, type: String) -> String {
        // A number goes in bare, which is what makes `SET n = 3` a number rather than the string
        // '3'. It goes in bare only when it *is* a number: the inline path writes this straight
        // into the statement, so a value that does not parse would reach the server as a word —
        // `SET n = abc` is either a syntax error or, worse, a reference to a column called `abc`.
        // A value that does not parse is quoted instead, so the server reports a type error against
        // a statement that is at least well formed.
        if isNumeric(type) { return isNumber(text) ? text : quoted(text) }
        if isBoolean(type) { return isBooleanValue(text) ? text : quoted(text) }
        return quoted(text)
    }

    /// A number a server will read as one.
    ///
    /// Deliberately strict: an optional sign, digits, at most one decimal point, and an optional
    /// exponent. `Double(text)` would be the wrong check — it accepts `inf`, `nan` and `1e400`,
    /// none of which is SQL — and so would a locale-aware parse, because `1,5` is two values in
    /// every dialect this app speaks.
    static func isNumber(_ text: String) -> Bool {
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty else { return false }
        var index = trimmed.startIndex
        if trimmed[index] == "-" || trimmed[index] == "+" { index = trimmed.index(after: index) }

        var digits = 0
        while index < trimmed.endIndex, trimmed[index].isNumber, trimmed[index].isASCII {
            digits += 1
            index = trimmed.index(after: index)
        }
        if index < trimmed.endIndex, trimmed[index] == "." {
            index = trimmed.index(after: index)
            while index < trimmed.endIndex, trimmed[index].isNumber, trimmed[index].isASCII {
                digits += 1
                index = trimmed.index(after: index)
            }
        }
        guard digits > 0 else { return false }

        if index < trimmed.endIndex, trimmed[index] == "e" || trimmed[index] == "E" {
            index = trimmed.index(after: index)
            if index < trimmed.endIndex, trimmed[index] == "-" || trimmed[index] == "+" {
                index = trimmed.index(after: index)
            }
            var exponent = 0
            while index < trimmed.endIndex, trimmed[index].isNumber, trimmed[index].isASCII {
                exponent += 1
                index = trimmed.index(after: index)
            }
            guard exponent > 0 else { return false }
        }
        return index == trimmed.endIndex
    }

    /// Whether the text is the word `true` or `false`, which is what a boolean column takes bare.
    static func isBooleanValue(_ text: String) -> Bool {
        ["true", "false"].contains(text.trimmingCharacters(in: .whitespaces).lowercased())
    }

    /// Text as a quoted literal, with the one escape every dialect here agrees on.
    private static func quoted(_ text: String) -> String {
        "'" + text.replacingOccurrences(of: "'", with: "''") + "'"
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

/// What one row's `WHERE` body yielded: how many clauses went in, and which columns could not.
struct MatchOutcome: Equatable {
    /// How many clauses the predicate has. Zero means no column could be compared, which is a
    /// refusal rather than a bare `DELETE`/`UPDATE`.
    let clauses: Int
    /// Columns left out, named with why.
    let excluded: [String]

    var isEmpty: Bool { clauses == 0 }

    /// A comment naming the columns left out, for the statement text the review sheet shows. It is
    /// part of the *reviewed* SQL on purpose: the predicate is weaker than the whole row, and the
    /// user should see that before approving it.
    var note: String? {
        excluded.isEmpty ? nil : "/* not matched: \(excluded.joined(separator: "; ")) */"
    }
}

/// One row's predicate in the review form, and the columns it could not use.
///
/// Returned by [`UpdateStatements.match`] for callers that only want the text. The planner works on
/// a `BoundSQL` instead, so the values in the predicate are placeholders the engine binds.
struct Match: Equatable {
    /// The predicate's text in the review form, or empty when no column can be compared safely.
    let sql: String
    /// Columns left out, named with why.
    let excluded: [String]

    var isEmpty: Bool { sql.isEmpty }

    /// A comment naming the columns left out, for the statement text the review sheet shows.
    var note: String? {
        excluded.isEmpty ? nil : "/* not matched: \(excluded.joined(separator: "; ")) */"
    }
}

