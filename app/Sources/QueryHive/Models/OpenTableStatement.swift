import Foundation

/// The statement a table opens with: its columns listed one by one, in the table's own order, so the
/// person can remove one or add a `WHERE` without first rewriting a `*`.
///
/// `table` is the already-qualified name (`TreeNode.insertableText`). A column name is quoted only when
/// it has to be (`SQLIdentifierQuoting`).
///
/// Two cases keep `SELECT *`: no columns (the run reported none, or never reported any), and a table
/// of more than `maxColumns`, where a statement of hundreds of names is not something anyone edits,
/// so `*` is the more useful text.
///
/// The layout is the one Format SQL gives the same statement (four-space indent, upper-case
/// keywords), written here rather than asked of the formatter so opening a table never depends on
/// it: `FROM` on a line of its own, and two or more columns one per line. A single column stays on
/// the `SELECT` line, as the formatter keeps it. No trailing `;`: Trino refuses one.
enum OpenTableStatement {
    /// Above this many columns the statement stays `SELECT *`.
    static let maxColumns = 200

    static func sql(table: String, columns: [String], kind: ConnectionKind) -> String {
        guard !columns.isEmpty, columns.count <= maxColumns else { return "SELECT *\nFROM \(table)" }
        let quoted = columns.map { SQLIdentifierQuoting.quoted($0, for: kind) }
        guard quoted.count > 1 else { return "SELECT \(quoted[0])\nFROM \(table)" }
        return "SELECT\n" + quoted.map { "    " + $0 }.joined(separator: ",\n") + "\nFROM \(table)"
    }
}
