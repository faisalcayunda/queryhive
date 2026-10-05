import Foundation

/// One statement a [`WritePlan`] will run, and what the plan expects it to affect.
///
/// Two renderings live here, and they come from **one** build: `sql` is the review
/// form (every value written in, so the user approves the numbers rather than a row
/// of `$1`), and `boundSQL` + `parameters` is the run form (placeholders for a driver
/// that binds, and the typed values beside them). They cannot disagree, because
/// `BoundSQL` writes both from the same value call. For a driver that cannot bind —
/// Trino — `boundSQL == sql` and `parameters` is empty, which is exactly the inline
/// statement the plan always emitted.
struct WriteStatement: Equatable {
    enum Kind: String {
        case delete = "DELETE"
        case update = "UPDATE"
        case insert = "INSERT"
    }

    let kind: Kind
    /// The statement as the review sheet shows it, with every value inlined.
    let sql: String
    /// The statement as the engine runs it: placeholders when the driver binds.
    let boundSQL: String
    /// The values bound into `boundSQL`, in order. Empty for an inlined statement.
    let parameters: [BindValue]
    /// The rows the plan expects this statement to affect. `nil` means the plan
    /// makes no claim and the engine verifies nothing.
    let expectedRows: Int?
    /// Whether the predicate is the row's key. This app carries no primary-key
    /// metadata, so every statement it builds is keyless and the engine applies
    /// the two-way rule (`actual != expected`).
    let keyed: Bool
    /// Columns the predicate could not compare safely (empty for the common case
    /// and for every `INSERT`). Non-empty means the match is weaker than the whole
    /// row, so it can affect more rows than expected — the engine's count check is
    /// what turns that into a rollback instead of a silent over-write.
    let unmatchedColumns: [String]

    /// Build from a `BoundSQL`, which already knows the driver and has the two
    /// renderings plus the values.
    init(kind: Kind, bound: BoundSQL, expectedRows: Int?, keyed: Bool,
         unmatchedColumns: [String]) {
        self.kind = kind
        self.sql = bound.display
        self.boundSQL = bound.sql
        self.parameters = bound.parameters
        self.expectedRows = expectedRows
        self.keyed = keyed
        self.unmatchedColumns = unmatchedColumns
    }
}

/// The queued changes and the statements they become, built **once**.
///
/// `ChangeReview` reads `sql` and `AppModel.applyChanges` reads `payload`; both
/// come from this value, so the sheet and the engine cannot be handed different
/// statements. Building the statements twice — once to show, once to run — is
/// exactly the bug this type exists to make impossible. Binding is where that
/// matters most: the review inlines the values (`sql`) and the run binds them
/// (`payload`), and both are rendered from the one `BoundSQL`.
struct WritePlan: Equatable {
    /// The table the plan writes to, or `nil` when the tab cannot name one (a
    /// hand-written query has no `sourceTable`), in which case the plan is empty
    /// and the sheet says why.
    let table: String?
    let statements: [WriteStatement]
    /// Rows the plan could not write, named with why. Empty is the ordinary case;
    /// non-empty means the plan would do less than the queue shows, and it says so
    /// instead of dropping the change quietly.
    let warnings: [String]

    init(table: String?, statements: [WriteStatement], warnings: [String] = []) {
        self.table = table
        self.statements = statements
        self.warnings = warnings
    }

    var isEmpty: Bool { statements.isEmpty }

    /// The statements as text for the review sheet, in the order they will run.
    ///
    /// This is the **display** form: the values are written in, so a `WHERE` is
    /// legible and a copied script runs. The engine is handed `payload` instead.
    var sql: [String] { statements.map(\.sql) }

    /// The `CHANGES` payload `apply_changes` reads — the single encoder.
    ///
    /// `sql` is the bound form (placeholders for a driver that binds) and `params`
    /// the values in placeholder order; `apply_changes` binds them or, for an
    /// inlined plan, finds no `params` and runs the SQL as written. The `params`
    /// key is additive: a plan with none is the plan this engine ran before
    /// binding existed.
    var payload: String {
        let items: [[String: Any]] = statements.map { statement in
            var object: [String: Any] = ["sql": statement.boundSQL, "keyed": statement.keyed]
            if let expected = statement.expectedRows { object["expected"] = expected }
            if !statement.parameters.isEmpty {
                object["params"] = statement.parameters.map(\.json)
            }
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
    ///
    /// The identity rule — every column that can be compared safely, at the value
    /// it was fetched with — lives in `MatchPolicy` and `UpdateStatements.appendMatch`.
    /// A row whose every column is excluded from the predicate yields no statement
    /// and a warning, never a bare `DELETE`/`UPDATE` that would match the table.
    /// Added rows are grouped and bounded by `WriteBatchBudget`.
    static func build(edits: CellEdits, rows: some RowReading, columns: [Event.Column],
                      table: String?, kind: ConnectionKind,
                      budget: WriteBatchBudget? = nil) -> WritePlan {
        guard let table, !columns.isEmpty else {
            return WritePlan(table: table, statements: [])
        }
        let budget = budget ?? WriteBatchBudget.forKind(kind)
        let style = ParameterStyle.forKind(kind)
        var statements: [WriteStatement] = []
        var warnings: [String] = []

        for row in edits.deletedRows.sorted() {
            guard let rowData = rows.row(at: row) else { continue }
            var bound = BoundSQL(style: style)
            bound.text("DELETE FROM \(table) WHERE ")
            let match = UpdateStatements.appendMatch(for: rowData, columns: columns, kind: kind,
                                                     into: &bound)
            guard match.clauses > 0 else {
                warnings.append(refusal(action: "deleted", row: row, excluded: match.excluded))
                continue
            }
            if let note = match.note { bound.text(" \(note)") }
            // Keyless: the predicate is the comparable columns at their fetched
            // values, so a duplicate row would be deleted too, and the engine must
            // see that.
            statements.append(WriteStatement(kind: .delete, bound: bound, expectedRows: 1,
                                             keyed: false, unmatchedColumns: match.excluded))
        }

        for update in UpdateStatements.generate(edits: edits, rows: rows, columns: columns,
                                                table: table, kind: kind) {
            guard let bound = update.bound else {
                warnings.append(refusal(action: "updated", row: update.row,
                                        excluded: update.excluded))
                continue
            }
            statements.append(WriteStatement(kind: .update, bound: bound, expectedRows: 1,
                                             keyed: false, unmatchedColumns: update.excluded))
        }

        let inserts = InsertStatements.build(edits: edits, columns: columns, table: table,
                                             kind: kind, budget: budget)
        statements.append(contentsOf: inserts.statements)
        warnings.append(contentsOf: inserts.warnings)

        return WritePlan(table: table, statements: statements, warnings: warnings)
    }

    /// The wording for a row the match policy could not identify, shared by the
    /// delete and update paths so the two cannot drift.
    private static func refusal(action: String, row: Int, excluded: [String]) -> String {
        "row \(row + 1) was not \(action): no column can be matched safely "
            + "(\(excluded.joined(separator: "; ")))"
    }
}
