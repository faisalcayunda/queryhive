import Foundation

/// The grid's server-side sort: the statement the in-memory order escalates to.
///
/// The grid sorts the rows it has, and says so. When the result was cut short by the row limit, an
/// in-memory order is no longer "the order of the result" — the rows outside the limit are not in it
/// at all — so the header offers to send the order to the server instead. The shape is the same one
/// the cross-column search escalation uses (`SearchStatement`): the user's statement kept byte for
/// byte inside a derived table, with an `ORDER BY` on the outside. It re-runs the query through the
/// ordinary `preview` command, so Safe Mode, the timeout, the retries and the streaming are the same
/// as a Run; only the SQL differs.
enum ServerSort {
    enum Failure: Error, Equatable {
        /// The column the order names has no name to send.
        case noColumn
        /// The escalation wraps exactly one statement; the caller must pick one first.
        case multipleStatements(Int)
    }

    /// The user's statement wrapped so the server orders it, or the reason it cannot be built.
    ///
    /// `column` is a source column's server name, quoted per driver. The direction is the grid's own
    /// (the same `GridSort.Direction` the header click cycles), so a server sort and the chevron on
    /// the header can never disagree about which way the rows go.
    static func order(sql: String, column: String, direction: GridSort.Direction,
                      kind: ConnectionKind) throws -> String {
        let statements = sqlStatements(in: sql)
        guard statements.count == 1, let statement = statements.first else {
            throw Failure.multipleStatements(statements.count)
        }
        let name = column.trimmingCharacters(in: .whitespaces)
        guard !name.isEmpty else { throw Failure.noColumn }

        let identifier = quotedIdent(name, for: kind)
        // NULL placement mirrors the in-memory comparator, which mirrors PostgreSQL's default:
        // NULLs last ascending, first descending. MySQL has no `NULLS LAST`/`FIRST` syntax, and its
        // own default puts them first ascending, so the clause is left off there rather than faked
        // with a second key. That difference is real and worth knowing; the banner names the column
        // and the direction, and the grid still marks the order it is showing.
        let nulls: String
        switch kind {
        case .postgres, .trino:
            nulls = direction == .ascending ? " NULLS LAST" : " NULLS FIRST"
        case .mysql:
            nulls = ""
        }

        // Newlines around the inner statement: a trailing `--` comment would otherwise comment out
        // the closing parenthesis, and the server would refuse the order rather than run it.
        return "SELECT * FROM (\n\(statement.text)\n) AS queryhive_sort"
            + " ORDER BY \(identifier) \(direction == .ascending ? "ASC" : "DESC")\(nulls)"
    }
}
