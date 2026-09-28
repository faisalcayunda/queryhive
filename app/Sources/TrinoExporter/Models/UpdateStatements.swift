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
    /// The statements, in row order, or none when there is nothing to write.
    ///
    /// The identity rule, and the reason it has to be written down: this app carries no primary-key
    /// metadata, so a row is identified by **every column the result carries, at the values it was
    /// fetched with**. That is the strongest predicate available without a key — and it is not the
    /// same as being unique. Two rows identical in every column both answer it, and the update
    /// changes both. That is why the statements are meant to be shown before they run: a predicate
    /// matching more than the row it was built from is visible there, and silent everywhere else.
    static func generate(edits: CellEdits, rows: [[String?]], columns: [Event.Column],
                         table: String, kind: ConnectionKind) -> [String] {
        let byRow = Dictionary(grouping: edits.values.keys, by: \.row)
        return byRow.keys.sorted().compactMap { row -> String? in
            guard rows.indices.contains(row) else { return nil }
            let assignments = (byRow[row] ?? [])
                .sorted { $0.column < $1.column }
                .compactMap { key -> String? in
                    guard columns.indices.contains(key.column),
                          let text = edits.value(at: key) else { return nil }
                    let name = quotedIdent(columns[key.column].name, for: kind)
                    return "\(name) = \(literal(text, type: columns[key.column].type))"
                }
            guard !assignments.isEmpty else { return nil }
            let predicate = predicate(for: rows[row], columns: columns, kind: kind)
            return "UPDATE \(table) SET \(assignments.joined(separator: ", "))"
                + (predicate.isEmpty ? "" : " WHERE \(predicate)")
        }
    }

    /// The columns that identify the row, at the values it was fetched with.
    private static func predicate(for row: [String?], columns: [Event.Column],
                                  kind: ConnectionKind) -> String {
        columns.indices.compactMap { index -> String? in
            let name = quotedIdent(columns[index].name, for: kind)
            // A column the row does not carry is NULL, which is what the grid drew for it: `rowView`
            // fills a short row with nil, and nil renders as `null`.
            guard index < row.count, let value = row[index] else { return "\(name) IS NULL" }
            return "\(name) = \(literal(value, type: columns[index].type))"
        }
        .joined(separator: " AND ")
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
        let name = base(type)
        return ["tinyint", "smallint", "int", "integer", "bigint", "int2", "int4", "int8",
                "serial", "bigserial", "smallserial", "real", "double", "double precision",
                "decimal", "numeric", "float", "float4", "float8", "number"].contains(name)
    }

    static func isBoolean(_ type: String) -> Bool {
        ["bool", "boolean"].contains(base(type))
    }

    /// A type name without its width or precision: `decimal(38,10)` becomes `decimal`.
    private static func base(_ type: String) -> String {
        String(type.prefix { $0 != "(" })
            .trimmingCharacters(in: .whitespaces)
            .lowercased()
    }
}
