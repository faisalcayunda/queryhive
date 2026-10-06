import Foundation

/// One JSON line printed by queryhive_engine.py. Fields are filled per event type.
struct Event: Decodable {
    struct Column: Decodable {
        let name: String
        let type: String
    }

    struct ExportedFile: Decodable {
        let path: String
        let bytes: Int

        var url: URL { URL(fileURLWithPath: path) }
        var name: String { url.lastPathComponent }
    }

    let event: String
    var step: String?
    var message: String?
    /// `progress` / `done`: the rows the run has sent or wrote. `count`: the total the server
    /// reports for the statement. One key, read per event name — `tests/golden/count/count.ndjson`
    /// is the frozen `{"event": "count", "rows": 4321}`, and neither engine ever writes a `count`
    /// key on it, so a property named `count` would decode a shape that does not exist.
    var rows: Int?
    var columns: [Column]?
    var files: [ExportedFile]?
    var warnings: [String]?
    /// `query_id` on the wire; `convertFromSnakeCase` maps it here.
    var queryId: String?
    var cancelled: Bool?
    /// `test` command.
    var ok: Bool?
    /// `test` command: how many catalogs the coordinator listed. Deliberately not called
    /// `catalogs`, which is the *event name* of the browse command and carries a string array.
    var catalogCount: Int?
    /// `catalogs` / `schemas` / `tables` commands: the listed identifiers, in the coordinator's
    /// own order.
    var names: [String]?
    /// `objects` command: the grid's own headers, in the driver's order. Deliberately a plain
    /// string array under its own key -- `columns` above is `[{"name","type"}]` for the preview
    /// grid, and an object column has no type to give. Reusing that key does not decode.
    var objectColumns: [String]?
    // `preview` command. Deliberately `data`, not `rows`: `rows` is the integer on `progress` and
    // `done`, and one key cannot be two types.
    var data: [[String?]]?
    var truncated: Bool?
    var elapsedMs: Int?
    var host: String?
    var user: String?
    // `to_table` command: what was written, and where.
    var table: String?
    var mode: String?
    /// The coordinator's own state string, carried on `to_table` progress events.
    var state: String?

    /// `history` command: one row per execution, newest first.
    var entries: [HistoryEntry]?
    /// `saved_queries` command, `list` action: the stored statements in name order. Its own key
    /// rather than `entries`, because the two are different record shapes and one key cannot be
    /// two of them.
    var queries: [SavedQuery]?
    /// `history_clear`: how many rows were living when it ran, so a caller can say what it did.
    var cleared: Int?
    /// `apply_changes` reply: how many statements ran and committed. The plan is
    /// rolled back otherwise, so this number is the whole plan or none of it.
    var applied: Int?

    // `import_data`: the row-file family. `rows` above is the count written; these are the rest of
    // its report. `errors` is the bad-row list (line-prefixed), `stopped_at` the line the import
    // gave up on, `disposition`/`transaction` the transaction policy's own words, and `rejected`
    // the rows that carried no value into any mapped column.
    var rejected: Int?
    var errors: [String]?
    var errorsTruncated: Bool?
    var stoppedAt: Int?
    var transaction: Bool?
    var disposition: String?
    var streams: Bool?
    var format: String?
    /// `history_entry`: whether the write landed on a row that was already there. False means the
    /// id in the reply is the one the caller would have chosen.
    var merged: Bool?
    /// `saved_query` reply: which action the engine took.
    var action: String?
    /// `saved_query` reply: the row, for `get` and `save`. Null when a `get` found nothing, which
    /// is a normal answer and not an error.
    var query: SavedQuery?
    /// `saved_query` reply: whether a `rename` actually found a row.
    var renamed: Bool?

    /// `session` command: whether a stored session was found. `false` on a fresh install, which is
    /// a normal answer the app turns into one blank tab, not an error.
    var saved: Bool?
    /// `session` command: the tabs as the app last wrote them, handed back in the same shape.
    var tabs: [SessionTab]?
    /// `session` command: which tab was in front. Absent when none was selected, which is not the
    /// same as an empty string.
    var activeTabId: String?

    /// `account` command: the application's own identity.
    var account: Account?
    /// `profiles` command, `list`: the living profiles. Its own key rather than reusing `queries`,
    /// which is a different record shape.
    var profiles: [Profile]?
    /// `profile` replies: the one row a `get` or `save` names.
    var profile: Profile?
    /// `profile_delete`: whether there was a row to delete. False is a no-op, not a failure.
    var deleted: Bool?

    /// One execution in the engine's history.
    ///
    /// Three fields are optional because a row can be written before its run has finished: an
    /// entry with no `outcome` yet is not the same as one that ended, and the engine keeps that
    /// difference rather than filling in a zero.
    struct HistoryEntry: Decodable, Identifiable {
        let id: String
        var sql: String
        /// Unix milliseconds, read when the user asked for the run.
        var startedAt: Int
        var elapsedMs: Int?
        var rowCount: Int?
        /// `ok`, `error` or `cancelled`, or nothing yet.
        var outcome: String?
        var error: String?
        /// The connection's own identity, which is also the Keychain account. Absent is a real
        /// value: a run can be recorded before its connection has been saved.
        var connectionId: String?
        var deleted: Bool
        var version: Int
    }

    /// One statement the user chose to keep.
    struct SavedQuery: Decodable, Identifiable {
        let id: String
        var name: String
        var sql: String
        var connectionId: String?
        var folderId: String?
        /// Whether the user keeps this query within reach. Always on the wire, from migration 4
        /// onwards: the engine owns the default, so the app never has to guess what an absent field
        /// would mean.
        var favourite: Bool
        var deleted: Bool
        var version: Int
    }

    /// One account, as the engine's `account` event carries it.
    ///
    /// A row with no provider is a real state rather than an error: the account exists from the
    /// first launch so that a profile saved before anyone has signed in still has an owner.
    struct Account: Decodable, Equatable {
        let id: String
        var provider: String?
        var subject: String?
        var email: String?
        var displayName: String?
        var signedInAt: Int?
        var signedOutAt: Int?
    }

    /// One thing saved for an account.
    ///
    /// The `payload` the engine carries is deliberately not decoded here: it is the kind's own
    /// body, and a pane that showed it would be showing a blob. Whatever owns a kind will decode
    /// it when it exists.
    struct Profile: Decodable, Identifiable, Equatable {
        let id: String
        var ownerId: String
        var kind: String
        var name: String
    }
}
