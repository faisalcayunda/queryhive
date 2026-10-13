import QueryHiveFFI
import SwiftUI

/// One row of the editor's suggestion popup.
struct SQLSuggestion: Identifiable, Equatable {
    /// Order matters: it is the ranking when several kinds match the same prefix. Objects come
    /// before keywords, because after two characters the user almost always means a table.
    enum Kind: Int {
        case table, column, schema, catalog, keyword

        var label: String {
            switch self {
            case .table: "table"
            case .column: "column"
            case .schema: "schema"
            case .catalog: "catalog"
            case .keyword: "keyword"
            }
        }

        var symbol: String {
            switch self {
            case .table: "tablecells"
            case .column: "textformat.abc"
            case .schema: "folder"
            case .catalog: "cylinder.split.1x2"
            case .keyword: "k.circle"
            }
        }

        var tint: Color {
            switch self {
            case .table: Tone.mint
            case .column: Tone.ice
            case .schema: Tone.amber
            case .catalog: Tone.violet
            case .keyword: Tone.secondary
            }
        }
    }

    let text: String
    let kind: Kind

    var id: String { "\(kind.rawValue):\(text)" }
}

/// The suggestion list the editor is showing right now.
///
/// It lives on `AppModel` rather than inside the editor view for two reasons: the snapshot tool
/// can open it without typing, and it can be dismissed once when the tab changes instead of
/// being rebuilt by every keystroke.
@Observable
final class EditorCompletion {
    var items: [SQLSuggestion] = []
    var selected = 0
    /// Caret position in the editor container's coordinate space, top-left origin.
    var anchor: CGRect = .zero

    var active: Bool { !items.isEmpty }

    func dismiss() {
        if !items.isEmpty { items = [] }
        selected = 0
    }

    /// Wraps around at both ends, the way every completion list does.
    func move(_ delta: Int) {
        guard !items.isEmpty else { return }
        selected = ((selected + delta) % items.count + items.count) % items.count
    }
}

enum SQLSuggestions {
    /// Trino SQL, weighted towards the statements this app actually issues. Multi-word entries are
    /// deliberate: the popup replaces the whole word under the caret, so picking "GROUP BY" after
    /// typing "GRO" leaves "GROUP BY", not "GRO BY".
    static let keywords: [String] = [
        "SELECT", "FROM", "WHERE", "GROUP BY", "ORDER BY", "HAVING", "LIMIT", "OFFSET",
        "JOIN", "INNER JOIN", "LEFT JOIN", "RIGHT JOIN", "FULL JOIN", "CROSS JOIN",
        "ON", "USING", "UNION", "UNION ALL", "INTERSECT", "EXCEPT", "WITH", "AS",
        "AND", "OR", "NOT", "IN", "IS NULL", "IS NOT NULL", "LIKE", "BETWEEN",
        "DISTINCT", "ALL", "ANY", "SOME", "EXISTS", "ASC", "DESC", "NULLS FIRST", "NULLS LAST",
        "CASE", "WHEN", "THEN", "ELSE", "END", "CAST", "TRY_CAST", "COALESCE", "NULLIF",
        "COUNT", "SUM", "AVG", "MIN", "MAX", "ARRAY_AGG", "APPROX_DISTINCT",
        "ROW_NUMBER", "RANK", "DENSE_RANK", "OVER", "PARTITION BY", "UNNEST", "VALUES",
        "INSERT INTO", "CREATE TABLE", "CREATE VIEW", "DROP TABLE", "DROP VIEW",
        "SHOW CATALOGS", "SHOW SCHEMAS", "SHOW TABLES", "SHOW COLUMNS", "DESCRIBE", "EXPLAIN",
        "ANALYZE", "VARCHAR", "BIGINT", "INTEGER", "DOUBLE", "DECIMAL", "BOOLEAN",
        "DATE", "TIMESTAMP", "ARRAY", "MAP", "ROW", "JSON", "INTERVAL", "FILTER",
    ]

    // MARK: Position-dependent policy (blueprint w11 section 12, T6b)

    /// Whether the output names a select list gave (`count(*) AS total`) are valid where the
    /// caret is, for this driver. `ORDER BY` takes them on all three. `GROUP BY` takes them on
    /// PostgreSQL and MySQL but not Trino, and `HAVING` only on MySQL, whose extension it is.
    /// Offering a name a server then refuses is worse than leaving it out, so everywhere else,
    /// and for an unknown driver, the answer is no.
    static func offersSelectAliases(in clause: EditorClause, for kind: ConnectionKind?) -> Bool {
        guard let kind else { return false }
        switch clause {
        case .orderBy: return true
        case .groupBy: return kind != .trino
        case .having: return kind == .mysql
        default: return false
        }
    }

    /// The names a new alias must avoid: every alias and every table name the statement already
    /// uses, and its CTEs. Compared without regard to case by the Rust side.
    static func takenNames(_ references: EditorReferences) -> [String] {
        references.relations.flatMap { [$0.alias, $0.name].compactMap { $0 } }
            + references.ctes.map(\.name)
    }

    /// What picking `table` writes at the caret. In a `FROM` or `JOIN` of a query it is the name
    /// and a short alias that is free in the statement (`order_items oi`); anywhere else, and
    /// wherever the statement is not known, the bare name. A table named by `UPDATE`, `DELETE
    /// FROM` or `INSERT INTO` never gets one: an alias there is not portable (MySQL refuses it in
    /// some forms) and the statement did not ask for it. The alias is made by the Rust side, so a
    /// keyword is never one and what is written reads back as an alias.
    ///
    /// A caller whose caret word is already followed by an alias (`FROM us| u`) passes no scope,
    /// or the alias would be written twice.
    static func tableText(_ table: String, in clause: EditorClause?,
                          references: EditorReferences?) -> String {
        guard clause == .queryTable, let references else { return table }
        return "\(table) \(sqlTableAlias(table: table, taken: takenNames(references)))"
    }

    /// An alias as the select list spelled it, without its quotes: what a column of the same
    /// name would be called.
    static func unquoted(_ alias: String) -> String {
        guard alias.count >= 2, let first = alias.first, first == "\"" || first == "`",
              alias.last == first else { return alias }
        return String(alias.dropFirst().dropLast())
    }
}
