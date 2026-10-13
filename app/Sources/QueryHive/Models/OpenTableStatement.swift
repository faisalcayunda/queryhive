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
enum OpenTableStatement {
    /// Above this many columns the statement stays `SELECT *`.
    static let maxColumns = 200

    static func sql(table: String, columns: [String], kind: ConnectionKind) -> String {
        guard !columns.isEmpty, columns.count <= maxColumns else { return "SELECT * FROM \(table)" }
        let list = columns.map { SQLIdentifierQuoting.quoted($0, for: kind) }.joined(separator: ", ")
        return "SELECT \(list) FROM \(table)"
    }
}
