import Foundation

/// How much one batch of rows may carry, on three axes at once.
///
/// The source study's `SQLWriteBatchBudget` bounds a batch by rows, bytes and the engine's
/// bind-parameter ceiling together, and closes the batch **before** the row that would cross a
/// bound, so a batch is never over budget and the row that would have crossed starts the next one.
/// A plan that respected only one axis would still fail the other two: 1.000 tiny rows and 1.000
/// huge ones are not the same request, and a wide row can exhaust a statement's parameters long
/// before it exhausts the row count.
///
/// QueryHive writes literal SQL, not bound parameters (Trino's HTTP protocol has none), so the
/// parameter axis is counted but not yet spent: every value a statement sends as a parameter would
/// spend one, and a value written as a literal spends one too, so the budget keeps its shape when
/// binding arrives. That change is a driver-trait decision for another slice, not this one.
struct WriteBatchBudget: Equatable {
    /// The most rows one batch may carry.
    let maxRows: Int
    /// The most bytes of statement text one batch may carry.
    let maxBytes: Int
    /// The most bind parameters one batch may carry.
    let maxParameters: Int

    /// The budget for a driver.
    ///
    /// The numbers, and why:
    ///
    /// - **rows: 1.000.** The same order the engine reads a result in (`next_batch(1_000)`), and
    ///   small enough that one send is not a long-held lock.
    /// - **bytes: 1 MiB (1.048.576).** Bounds a single request's text independently of the row
    ///   count, which is what stops one row with a large `TEXT`/`JSON` value from blowing up a
    ///   batch of otherwise small rows.
    /// - **parameters: 65.535.** PostgreSQL's and MySQL's own statement limit (both cap at 2^16-1).
    ///   Trino's HTTP protocol has no bind parameters at all, so the axis is unbounded there and the
    ///   driver-trait decision that would change it is recorded, not guessed at.
    static func forKind(_ kind: ConnectionKind) -> WriteBatchBudget {
        switch kind {
        case .postgres, .mysql:
            WriteBatchBudget(maxRows: 1_000, maxBytes: 1 << 20, maxParameters: 65_535)
        case .trino:
            WriteBatchBudget(maxRows: 1_000, maxBytes: 1 << 20, maxParameters: .max)
        }
    }
}
