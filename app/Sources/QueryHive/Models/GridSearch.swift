import Foundation

/// The grid's cross-column search: the in-memory scan, and the server side it can escalate to.
///
/// Two things share the name and do different jobs, which is why the grid has to say which is which:
///
/// * **In memory**, `matches` scans every cell of every fetched row for the term. It is instant, it
///   does not touch the server, and it can only find a row that was fetched — a row outside the row
///   limit is not searched at all.
/// * **Escalated**, `SearchStatement.crossColumn` turns the same term into a `WHERE` over the
///   result's own columns so the server finds rows the grid never fetched. It re-runs the query, so
///   it costs a round trip and reads the table; that is the price of searching rows you have not
///   seen.
enum GridSearch {
    /// Whether any cell of the row contains the term, case-insensitively.
    ///
    /// A nil cell is skipped — a NULL contains nothing — and the term is trimmed first, so a stray
    /// space does not hide every row. Case-insensitive with the locale's own rules, which is what
    /// someone typing a fragment of a name expects; the server escalation folds with `LOWER` for
    /// the same reason.
    static func matches(_ row: [String?], term: String) -> Bool {
        let needle = term.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !needle.isEmpty else { return true }
        for value in row {
            guard let value else { continue }
            if value.localizedCaseInsensitiveContains(needle) { return true }
        }
        return false
    }
    /// Server search policy: one query per pause, not per keystroke.
    static let minimumLength = 3
    static let debounceInterval: TimeInterval = 0.25
    /// The term worth a round trip, or nil below the minimum (then cleared).
    static func searchableTerm(_ term: String) -> String? {
        let needle = term.trimmingCharacters(in: .whitespacesAndNewlines)
        guard needle.count >= minimumLength else { return nil }
        return needle
    }
}

/// The server side of the cross-column search: the statement the term becomes.
enum SearchStatement {
    enum Failure: Error, Equatable {
        case blank
        /// The escalation wraps exactly one statement; the caller must pick one first.
        case multipleStatements(Int)
        /// Every column was one the server cannot render as text, so there is nothing to search.
        case noColumns
    }

    /// The user's statement wrapped so the server searches it by the term across every column.
    ///
    /// Built here rather than by hand in the view for the same reasons `count` wraps its statement:
    /// the user's SQL is kept byte for byte inside a derived table, it is refused rather than
    /// guessed when there is more than one statement, and the text cast is spelled per driver. The
    /// engine runs the result through the ordinary `preview` command, so the safety mode, the
    /// timeout, the retries and the streaming are the same as a Run; only the SQL differs.
    ///
    /// A future engine-side `search_statement` — the sibling of `count_statement` — is the right
    /// home for this builder. It lives in Swift only because the worktree that added the feature is
    /// not allowed to touch `crates/`, and doing it in Swift keeps the search working until then
    /// rather than shipping a button that does nothing.
    static func crossColumn(sql: String, term: String, columns: [Event.Column],
                            kind: ConnectionKind) throws -> String {
        let needle = term.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !needle.isEmpty else { throw Failure.blank }

        let statements = sqlStatements(in: sql)
        guard statements.count == 1, let statement = statements.first else {
            throw Failure.multipleStatements(statements.count)
        }

        // The same rule a write's `WHERE` follows: a nested or binary column is not something the
        // server can cast to text on every engine, and asking Trino to would fail the query rather
        // than simply miss the row. Such a column is left out of the search.
        let usable = columns.filter { column in
            if case .excluded = MatchPolicy.forColumn(type: column.type, kind: kind) { return false }
            return true
        }
        guard !usable.isEmpty else { throw Failure.noColumns }

        // A quote is the only thing that needs escaping: the term is a literal, not a pattern, so
        // there are no wildcards to escape — the position function does the containing.
        let literal = "'" + needle.replacingOccurrences(of: "'", with: "''") + "'"
        let clauses = usable.map { column -> String in
            let text = UpdateStatements.serverText(quotedIdent(column.name, for: kind), for: kind)
            switch kind {
            // MySQL's `LOCATE(haystack, needle)`, Trino's and PostgreSQL's `STRPOS(haystack, needle)`
            // take their arguments in opposite orders; that difference is the whole reason this is
            // per driver.
            case .mysql:
                return "LOCATE(LOWER(\(literal)), LOWER(\(text))) > 0"
            case .trino, .postgres:
                return "STRPOS(LOWER(\(text)), LOWER(\(literal))) > 0"
            }
        }

        // Newlines around the inner statement: a trailing `--` comment would otherwise comment out
        // the closing parenthesis and the alias, and the server would refuse the search rather than
        // run it.
        return "SELECT * FROM (\n\(statement.text)\n) AS queryhive_search"
            + " WHERE \(clauses.joined(separator: " OR "))"
    }
}
