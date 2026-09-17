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
}
