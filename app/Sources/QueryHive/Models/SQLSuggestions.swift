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

    /// What the popup shows.
    let text: String
    let kind: Kind
    /// What picking the row writes: `text` unless the name has to be quoted to be read back.
    let insert: String

    init(text: String, kind: Kind, insert: String? = nil) {
        self.text = text
        self.kind = kind
        self.insert = insert ?? text
    }

    var id: String { "\(kind.rawValue):\(text)" }
}

/// What the editor asks the model about the word under the caret.
struct CompletionRequest {
    /// What was typed of the word, without its opening quote.
    var prefix: String
    /// The names the word is qualified by, outermost first, without quotes.
    var path: [String]
    /// What the text before the word says about the caret; nil when it was not read.
    var scope: EditorCaretScope?
    /// The statement's last analysis; nil when it was not asked for or has not landed.
    var references: EditorReferences?
    /// Whether a table written here may get an alias. False when the word is already followed by
    /// one (`FROM us| u`), which would otherwise be written twice.
    var aliasable = true
    /// The word was typed after an identifier quote the user opened. The name picked is then
    /// written quoted even if it did not need to be.
    var quoted = false
}

/// The suggestion list the editor is showing right now.
///
/// It lives on `AppModel` rather than inside the editor view for two reasons: the snapshot tool
/// can open it without typing, and it can be dismissed once when the tab changes instead of
/// being rebuilt by every keystroke.
@Observable
final class EditorCompletion {
    /// How many rows the popup shows at once; the list scrolls past that.
    static let visibleRows = 8

    var items: [SQLSuggestion] = []
    var selected = 0
    /// The first row on screen.
    private(set) var top = 0
    /// The list was opened by itself on an empty word (after `FROM `), so it is there to be read:
    /// Return still inserts a newline until the user steps into it with an arrow key.
    var passive = false
    /// Caret position in the editor container's coordinate space, top-left origin.
    var anchor: CGRect = .zero
    /// Called when the catalog learned something (a level of the tree arrived). The editor that
    /// opened the list sets it to re-ask for it, so a list that was empty because it was waiting
    /// fills in without another keystroke.
    @ObservationIgnored var onCatalogLoaded: (() -> Void)?

    var active: Bool { !items.isEmpty }

    /// The rows on screen.
    var visible: ArraySlice<SQLSuggestion> { items[top..<min(top + Self.visibleRows, items.count)] }

    func present(_ items: [SQLSuggestion], anchor: CGRect, passive: Bool) {
        self.items = items
        self.selected = 0
        self.top = 0
        self.passive = passive
        self.anchor = anchor
    }

    func dismiss() {
        if !items.isEmpty { items = [] }
        selected = 0
        top = 0
        passive = false
    }

    /// Wraps around at both ends, the way every completion list does.
    func move(_ delta: Int) {
        guard !items.isEmpty else { return }
        passive = false
        selected = ((selected + delta) % items.count + items.count) % items.count
        if selected < top { top = selected }
        if selected >= top + Self.visibleRows { top = selected - Self.visibleRows + 1 }
    }

    func catalogLoaded() { onCatalogLoaded?() }
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

    /// How well `name` answers what was typed: 0 for a prefix, 1 for the start of a word inside it
    /// (`kpm` in `rekap_kpm_harian`, `net` in `grossNet`), 2 for any other stretch of three or more
    /// characters; nil for no match. A keyword is only ever matched by its prefix, because a word
    /// from the middle of `GROUP BY` is not what anyone types.
    static func match(_ name: String, typed: String, keyword: Bool = false) -> Int? {
        let needle = Array(typed.lowercased())
        guard !needle.isEmpty else { return 0 }
        let letters = Array(name)
        let lowered = Array(name.lowercased())
        // Case mapping can change the length (a few non-Latin letters); the positions below
        // assume it did not, so such a name is only matched by its prefix.
        func hasPrefix(at start: Int) -> Bool {
            start + needle.count <= lowered.count && Array(lowered[start..<(start + needle.count)]) == needle
        }
        if hasPrefix(at: 0) { return 0 }
        guard !keyword, letters.count == lowered.count else { return nil }
        for index in 1..<max(letters.count, 1) {
            let here = letters[index], before = letters[index - 1]
            let isWord = here.isLetter || here.isNumber
            let startsWord = isWord && (!(before.isLetter || before.isNumber)
                || (before.isLowercase && here.isUppercase))
            if startsWord, hasPrefix(at: index) { return 1 }
        }
        return needle.count >= 3 && String(lowered).contains(String(needle)) ? 2 : nil
    }

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
    ///
    /// `name` is what is written for the table, which may be qualified (`sales.orders`); the alias
    /// is made from `base`, the table's own name, when it differs.
    static func tableText(_ name: String, base: String? = nil, in clause: EditorClause?,
                          references: EditorReferences?) -> String {
        guard clause == .queryTable, let references else { return name }
        return "\(name) \(sqlTableAlias(table: base ?? name, taken: takenNames(references)))"
    }

    /// An alias as the select list spelled it, without its quotes: what a column of the same
    /// name would be called.
    static func unquoted(_ alias: String) -> String {
        guard alias.count >= 2, let first = alias.first, first == "\"" || first == "`",
              alias.last == first else { return alias }
        return String(alias.dropFirst().dropLast())
    }
}
