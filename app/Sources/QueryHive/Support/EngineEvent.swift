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
    /// `import_data` on a `.sql` file: the statements it ran (`progress` and `done`), as a count.
    /// The key is also on `apply_changes`' `done`, where it is the array of per-statement results,
    /// so it decodes to either shape instead of failing the whole event (`StatementCount`).
    var statements: StatementCount?
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

    // The metadata events (W11-T1, `crates/qh-ffi/src/metadata.rs`). Each key is one this struct
    // did not use for anything else, because one key cannot be two types: `names` stays the
    // `tables` listing and `kinds` rides beside it, `fields` is not `columns` (the preview
    // grid's typed headers) and `object` is not `table` (a `to_table` name).

    /// `tables` with `OBJECT_KINDS=1`: one kind per name, in the same order. `table`, `view`,
    /// `materialized_view` or `foreign_table`, and `null` where the server did not say. Absent for
    /// a driver that has no kinds, and a name with no kind is a table.
    var kinds: [String?]?
    /// `table_columns` and `table_ddl`: what was asked about, by slot. A part the driver has no
    /// level for is `null` (MySQL has no schema, PostgreSQL no catalog).
    var object: ObjectName?
    /// `table_columns`: the columns, in the table's order.
    var fields: [ColumnField]?
    /// `table_ddl`: what the object is, when the server said. `object_kind` on the wire.
    var objectKind: String?
    /// `table_ddl`: the DDL text. A reconstruction for PostgreSQL, whose first line says so.
    var ddl: String?
    /// `table_ddl`: whether a credential the server printed into the DDL was masked.
    var redacted: Bool?
    /// `execution_log`: the recent Safe Mode decisions, newest first.
    var decisions: [LogDecision]?
    /// `execution_log`: whether the chain over the rows still verifies.
    var chain: LogChain?
    /// `execution_log`: whether this engine is writing decisions to the log it reads.
    var writer: Bool?
    /// `error`: the host key behind a failed SSH connection, when `SSH_HOST_KEY_DETAIL=1` asked
    /// for it (blueprint w11 §5.8). The key is read here once so the connection work that decides
    /// what to do with it adds no new key to this struct. `host_key` on the wire.
    var hostKey: HostKeyDetail?

    /// The three parts of an object's name, one per slot the driver has.
    struct ObjectName: Decodable, Equatable {
        var catalog: String?
        var schema: String?
        var table: String?
    }

    /// One column of `table_columns`.
    struct ColumnField: Decodable, Equatable {
        var name: String
        var type: String
        /// `nil` when the server did not say.
        var nullable: Bool?
        /// The default expression as the server prints it. For a generated column this is its
        /// expression, which is why `extra` has to be read with it.
        var `default`: String?
        /// `identity`, `generated`, `auto_increment`, or the server's own words; empty for none.
        var extra: String
    }

    /// One decision of the execution log. The statement is never in it, only its hash.
    struct LogDecision: Decodable, Equatable, Identifiable {
        let seq: Int
        let id: String
        /// Unix milliseconds.
        var at: Int
        var safeMode: String
        /// `allowed`, `confirmed`, `refused` or `needs_confirmation`.
        var decision: String
        /// `read_only`, `dml`, `ddl` or `unknown`.
        var statementKind: String
        var statementIndex: Int
        var statementHash: String
        var reason: String?
    }

    /// What `execution_log` says about its own chain. `verified` is `nil` when the caller asked
    /// not to verify, `false` with `seq` and `detail` when a row was changed.
    struct LogChain: Decodable, Equatable {
        var verified: Bool?
        var rows: Int?
        var seq: Int?
        var detail: String?
    }

    /// The host key of a bastion, and what is already recorded for it.
    struct HostKeyDetail: Decodable, Equatable {
        /// `unknown`, `changed`, `revoked`, `certificate`, `certificate_expected`, `pin_mismatch`,
        /// `record_failed` or `store_unsafe`.
        var state: String
        var host: String
        var port: Int?
        var alias: String?
        var keyType: String?
        var fingerprint: String?
        var appKnownHosts: String?
        var caCovered: Bool?
        var recorded: [RecordedKey]?
        /// Only on `pin_mismatch`: the fingerprint the caller pinned.
        var pinned: String?

        struct RecordedKey: Decodable, Equatable {
            var fingerprint: String?
            var keyType: String?
            /// `app`, `user` or `system`.
            var source: String?
            var path: String?
            var line: Int?
        }
    }

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

    /// The `statements` key as a number of statements, whichever shape the engine wrote it in.
    ///
    /// `import_data` writes an integer and `apply_changes` writes one object per statement it ran.
    /// One key cannot be two types, and a property typed for one would throw on the other and drop
    /// the entire event, so this reads an integer as it is and an array as its length.
    struct StatementCount: Decodable, Equatable {
        var count: Int

        init(from decoder: Decoder) throws {
            let value = try decoder.singleValueContainer()
            if let number = try? value.decode(Int.self) {
                count = number
            } else {
                count = try value.decode([Anything].self).count
            }
        }

        /// Any JSON value, read and thrown away.
        private struct Anything: Decodable {
            init(from decoder: Decoder) throws {}
        }
    }
}
