import Foundation

/// One statement a [`WritePlan`] will run, and what the plan expects it to affect.
///
/// The SQL here is exactly what the review sheet shows and exactly what the engine
/// is handed — one string, two readers. That is the property the phase asks for:
/// the statement that was reviewed is the statement that ran.
struct WriteStatement: Equatable {
    enum Kind: String {
        case delete = "DELETE"
        case update = "UPDATE"
        case insert = "INSERT"
    }

    let kind: Kind
    let sql: String
    /// The rows the plan expects this statement to affect. `nil` means the plan
    /// makes no claim and the engine verifies nothing.
    let expectedRows: Int?
    /// Whether the predicate is the row's key. This app carries no primary-key
    /// metadata, so every statement it builds is keyless and the engine applies
    /// the two-way rule (`actual != expected`).
    let keyed: Bool
}

/// The queued changes and the statements they become, built **once**.
///
/// `ChangeReview` reads `sql` and `AppModel.applyChanges` reads `payload`; both
/// come from this value, so the sheet and the engine cannot be handed different
/// statements. Building the statements twice — once to show, once to run — is
/// exactly the bug this type exists to make impossible.
struct WritePlan: Equatable {
    /// The table the plan writes to, or `nil` when the tab cannot name one (a
    /// hand-written query has no `sourceTable`), in which case the plan is empty
    /// and the sheet says why.
    let table: String?
    let statements: [WriteStatement]

    var isEmpty: Bool { statements.isEmpty }

    /// The statements as text, in the order they will run.
    var sql: [String] { statements.map(\.sql) }

    /// The `CHANGES` payload `apply_changes` reads — the single encoder.
    var payload: String {
        let items: [[String: Any]] = statements.map { statement in
            var object: [String: Any] = ["sql": statement.sql, "keyed": statement.keyed]
            if let expected = statement.expectedRows { object["expected"] = expected }
            return object
        }
        guard let data = try? JSONSerialization.data(withJSONObject: items, options: [.sortedKeys]) else {
            return "[]"
        }
        return String(data: data, encoding: .utf8) ?? "[]"
    }

    /// Build the plan from the queue, the rows it was built against, and the table.
    ///
    /// The order is deliberate and is the source study's: **deletes, then updates,
    /// then inserts**. A plan that frees a unique value by deleting a row and then
    /// adds a row that takes it only works in that order, which is why the study
    /// records a sequence stamp on every change rather than trusting an array's
    /// order.
    static func build(edits: CellEdits, rows: [[String?]], columns: [Event.Column],
                      table: String?, kind: ConnectionKind) -> WritePlan {
        guard let table, !columns.isEmpty else {
            return WritePlan(table: table, statements: [])
        }
        var statements: [WriteStatement] = []

        for row in edits.deletedRows.sorted() {
            guard rows.indices.contains(row) else { continue }
            let predicate = UpdateStatements.predicate(for: rows[row], columns: columns, kind: kind)
            let sql = predicate.isEmpty
                ? "DELETE FROM \(table)"
                : "DELETE FROM \(table) WHERE \(predicate)"
            // Keyless: the predicate is every column at its fetched value, so a
            // duplicate row would be deleted too, and the engine must see that.
            statements.append(WriteStatement(kind: .delete, sql: sql, expectedRows: 1, keyed: false))
        }

        for sql in UpdateStatements.generate(edits: edits, rows: rows, columns: columns,
                                             table: table, kind: kind) {
            statements.append(WriteStatement(kind: .update, sql: sql, expectedRows: 1, keyed: false))
        }

        for insert in edits.inserted.sorted(by: { $0.sequence < $1.sequence }) {
            let names = columns.map { quotedIdent($0.name, for: kind) }.joined(separator: ", ")
            let values = columns.enumerated().map { index, column -> String in
                // A column the user left blank is NULL, the same reading the grid
                // gives an empty cell.
                guard let text = insert.values[index] else { return "NULL" }
                return UpdateStatements.literal(text, type: column.type)
            }.joined(separator: ", ")
            statements.append(WriteStatement(
                kind: .insert,
                sql: "INSERT INTO \(table) (\(names)) VALUES (\(values))",
                expectedRows: 1,
                keyed: false))
        }

        return WritePlan(table: table, statements: statements)
    }
}
