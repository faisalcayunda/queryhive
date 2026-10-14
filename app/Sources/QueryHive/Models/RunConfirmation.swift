import Foundation

/// The app's half of the engine's `confirm` Safe Mode level, and the DDL/DML split it needs.
///
/// The engine owns the rule. A `confirm` connection runs a write only when the run carries
/// `SAFE_MODE_CONFIRMED=1`, and the engine documents that flag as "a boolean only a caller that
/// asked its user could have set" (`crates/qh-ffi/src/commands.rs`). The engine cannot raise a
/// window, so asking is this app's job: decide when the level demands a question, name what would
/// run, and add the flag to that one run. Nothing here is stored — the flag lives for one run,
/// which is the engine's own model of a confirmation, not an "always allow" switch.
///
/// The classification below mirrors `qh_sql::classify` (`crates/qh-sql/src/classify.rs`), and the
/// direction it errs in is deliberate. A statement it recognises as DML makes the app ask; a
/// statement it recognises as DDL or cannot read at all does **not**, because under `confirm` the
/// engine refuses those whatever the caller says. Offering to approve a statement the engine will
/// refuse anyway would be a question with no good answer, so the app lets the engine's own refusal
/// surface instead.
enum RunConfirmation {
    /// One run the engine would let through only after a confirmation.
    struct Request: Equatable {
        /// The statements the engine is waiting to be approved, spelled as the app knows them.
        var statements: [String]
        /// A heading for the sheet, e.g. "Run this write?" or "Drop table?".
        var title: String
        /// A sentence about what the approval covers, under the statements.
        var note: String
        /// The approve button's label, a named verb ("Run Write", "Drop Table") so the button says
        /// what Return would have done.
        var confirmTitle: String = "Approve and Run"
    }

    /// The confirmation a run of caller SQL needs, or `nil` when nothing would be asked.
    ///
    /// `statements` are the statements the engine will read, already split the way the engine
    /// splits a script. A mode below `confirm` asks nothing; at `confirm` the run asks exactly when
    /// every statement is either a read or a DML write, and at least one is a write. A DDL or an
    /// unclassified statement anywhere in the script short-circuits to `nil`: the engine refuses
    /// the whole script for it, and a confirmation would not change that.
    static func request(for statements: [String], command: String,
                        safeMode: ConnectionSafeMode) -> Request? {
        guard safeMode == .confirm else { return nil }
        let kinds = statements.map(StatementScan.classify)
        guard !kinds.contains(.ddl), !kinds.contains(.unknown) else { return nil }
        let writes = zip(statements, kinds).filter { $0.1 == .dml }.map(\.0)
        guard !writes.isEmpty else { return nil }
        return Request(
            statements: writes,
            title: writes.count == 1 ? "Run this write?" : "Run these \(writes.count) writes?",
            note: "Safe Mode is Confirm on this connection, so the engine runs a write only after "
                + "an explicit approval. This approval covers this run and is not remembered.",
            confirmTitle: writes.count == 1 ? "Run Write" : "Run \(writes.count) Writes")
    }

    /// The confirmation a destructive table operation needs.
    ///
    /// `table_op` is the engine's one exception to "confirm refuses DDL" (ADR-0027): the level
    /// exists to ask about exactly these two operations, so the engine asks instead of refusing.
    /// The app therefore asks at `confirm` even though `StatementScan` calls the statement DDL —
    /// that is the contract, not a hole in the classifier. `full` asks too (blueprint D-11, flagged
    /// for the owner); `no_ddl`/`read_only` return `nil` because the engine refuses before connecting.
    static func destructiveRequest(for statement: String, title: String, confirmTitle: String? = nil,
                                   safeMode: ConnectionSafeMode) -> Request? {
        guard safeMode == .confirm || safeMode == .full else { return nil }
        let verb = confirmTitle ?? (title.hasSuffix("?") ? String(title.dropLast()) : title)
        let isDrop = statement.uppercased().hasPrefix("DROP")
        let note = safeMode == .confirm
            ? "Safe Mode is Confirm on this connection, so the engine runs this only after an "
                + "explicit approval. This approval covers this one operation and is not remembered."
            : isDrop
            ? "This permanently removes the table and its data. It cannot be undone from this app."
            : "This deletes every row in the table. It cannot be undone from this app."
        return Request(statements: [statement], title: title, note: note, confirmTitle: verb)
    }

    /// The settings a run adds after the user approved it, merged into that run's environment only.
    static func approvalSettings(_ approved: Bool) -> [String: String] {
        approved ? ["SAFE_MODE_CONFIRMED": "1"] : [:]
    }

    // MARK: Save to Table across connections (W13-T20, B-2 and B-6)

    /// One end of a copy across connections, as the transfer sheet names it.
    struct TransferSide: Equatable {
        var name: String
        /// The driver's own name, "PostgreSQL".
        var kind: String
        /// `host:port`, the address the engine dials (before any tunnel).
        var address: String
        /// The connection's database (Trino: its catalog), empty when it names none.
        var database: String
        var environment: ConnectionEnvironment?
        /// The connection's encryption as its editor names it ("require · unverified").
        var tls: String
        /// Whether the engine will not check the certificate on this side.
        var tlsUnverified: Bool
        var safeMode: ConnectionSafeMode
    }

    /// The question every copy across connections asks before anything runs.
    struct TransferRequest: Equatable {
        var source: TransferSide
        var target: TransferSide
        /// The target table, each part quoted for the target's dialect (`SQLIdentifierQuoting`).
        var table: String
        var mode: WriteMode
        var title: String
        /// What the copy does, in a sentence under the two connections.
        var note: String
        var confirmTitle: String
        /// Replace drops a table on the target: the sheet uses the destructive colour.
        var destructive: Bool
        /// The target's Safe Mode is Confirm, so the approval is also the one confirmation its
        /// engine waits for (`DEST_SAFE_MODE_CONFIRMED=1`, this run only).
        var confirmsTargetSafeMode: Bool
        /// The source is labelled prod and the target is not (unlabelled included, OQ-3): the
        /// approval waits for an explicit tick.
        var needsAcknowledgement: Bool
        /// The sentence beside that tick.
        var acknowledgement: String
    }

    /// The transfer sheet for a copy from `source` to `target`. **Never `nil`**: unlike `request`,
    /// which stays quiet below `confirm`, every copy across connections is asked about whatever
    /// either side's Safe Mode is, and there is no "don't ask again" (B-6). Rows leave one server
    /// for another, and the person approving it has to see both by name, address and label first.
    ///
    /// `parts` are the target name's parts the target's driver has (`ConnectionKind.targetParts`),
    /// written with `SQLIdentifierQuoting.quoted` so a name holding the dialect's quote character is
    /// shown as the statement would spell it (B-3d), not as `qualifiedName` does.
    static func transferRequest(source: Connection, target: Connection, mode: WriteMode,
                                table parts: [String]) -> TransferRequest {
        let table = parts.map { SQLIdentifierQuoting.quoted($0, for: target.kind) }.joined(separator: ".")
        let replacing = mode == .replace
        let confirm = target.safeMode == .confirm
        let crossesProduction = source.environment == .prod && target.environment != .prod
        var note = "QueryHive reads the rows from \(source.name) and writes them to \(target.name). "
            + "The table gets the query's columns only: no keys, indexes, defaults, triggers or grants."
        if replacing {
            note += " The new table is built beside the old one and takes its name when every row is in; "
                + "if anything fails first, the old table is left as it was."
        }
        if confirm {
            note += " Safe Mode is Confirm on \(target.name), so its engine creates the table only "
                + "after this approval. The approval covers this one run and is not remembered."
        }
        return TransferRequest(
            source: side(source), target: side(target), table: table, mode: mode,
            title: replacing ? "Replace a table on \(target.name)?" : "Copy rows to \(target.name)?",
            note: note,
            confirmTitle: replacing ? "Replace Table" : "Copy Rows",
            destructive: replacing,
            confirmsTargetSafeMode: confirm,
            needsAcknowledgement: crossesProduction,
            acknowledgement: "I understand rows from \(source.name) will be copied to \(target.name)")
    }

    private static func side(_ connection: Connection) -> TransferSide {
        let tls = ConnectionTLSChoice(kind: connection.kind, scheme: connection.scheme,
                                      verify: connection.verify, sslmode: connection.sslmode)
        return TransferSide(name: connection.name, kind: connection.kind.label,
                            address: "\(connection.host):\(connection.port)",
                            database: connection.database, environment: connection.environment,
                            tls: tls.title, tlsUnverified: tls.title.hasSuffix("· unverified"),
                            safeMode: connection.safeMode)
    }

    /// The settings an approved transfer sheet adds, to that run only. The source only reads, so it
    /// never gets a confirmation; the target gets one only when its level waits for it.
    static func transferApprovalSettings(_ request: TransferRequest) -> [String: String] {
        request.confirmsTargetSafeMode ? ["DEST_SAFE_MODE_CONFIRMED": "1"] : [:]
    }
}

/// What one statement does, as far as the app needs to tell a write from a read.
///
/// A narrowed mirror of the engine's classifier: the same leading-keyword rule, the same write-word
/// lists, and the same trivia stripping (strings, quoted identifiers, comments and dollar-quoted
/// bodies). It exists so the confirmation sheet can name the statements that need approval; it is
/// not an authority. The engine still classifies the statement itself, and a disagreement in the
/// safe direction — the app asks where the engine would not — costs a question, not a write.
enum StatementScan {
    /// The engine's own four kinds, minus the engine's names for them.
    enum Kind: Equatable {
        case readOnly
        case dml
        case ddl
        case unknown
    }

    /// `INSERT`, `UPDATE`, … — the words that make a statement a write of rows.
    private static let dmlWords: Set<String> = [
        "INSERT", "UPDATE", "DELETE", "MERGE", "REPLACE", "UPSERT", "COPY", "LOAD", "CALL", "DO",
    ]

    /// `CREATE`, `DROP`, … plus `INTO`, which is DDL for `SELECT … INTO`.
    private static let ddlWords: Set<String> = [
        "CREATE", "ALTER", "DROP", "TRUNCATE", "GRANT", "REVOKE", "COMMENT", "RENAME", "REINDEX",
        "VACUUM", "CLUSTER", "ANALYZE", "REFRESH", "ATTACH", "DETACH", "LOCK", "INTO",
    ]

    /// A SQL keyword word, uppercased: letters, digits and underscores.
    private static func writeKind(_ word: String) -> Kind? {
        if dmlWords.contains(word) { return .dml }
        if ddlWords.contains(word) { return .ddl }
        return nil
    }

    /// Classify one statement by its leading keyword and the bare words in its body.
    static func classify(_ sql: String) -> Kind {
        let words = significantWords(in: sql)
        guard let leading = words.first else { return .unknown }
        switch leading {
        // These read whatever else they contain: `SHOW CREATE TABLE` holds `CREATE` and creates
        // nothing, the one false positive worth carving out, exactly as the engine carves it.
        case "SHOW", "DESC", "DESCRIBE":
            return .readOnly
        // A read starter that can hide a write further in: `WITH x AS (DELETE …) SELECT`,
        // `SELECT … FOR UPDATE`, `SELECT … INTO new_table`, `EXPLAIN ANALYZE …`.
        case "SELECT", "VALUES", "TABLE", "WITH", "EXPLAIN":
            for word in words.dropFirst() {
                if let kind = writeKind(word) { return kind }
            }
            return .readOnly
        // Anything else is judged by its own leading word.
        default:
            return writeKind(leading) ?? .unknown
        }
    }

    /// Every bare word a statement holds, uppercased, after the trivia is removed.
    ///
    /// The stripping is the part that matters: a `DROP` inside a string, a quoted identifier or a
    /// comment is text, not a keyword, and reading it as one would make the app ask about a plain
    /// `SELECT`. Dollar-quoted bodies are PostgreSQL's, so only that one spelling is skipped here;
    /// a body the app fails to recognise can only make it ask more, never less.
    static func significantWords(in sql: String) -> [String] {
        var words: [String] = []
        var current = ""
        // One element per scalar, not per grapheme: a quote with a combining mark on it is a quote to
        // the engine, and a scanner that took the pair for one other character would read a closing
        // quote as part of the literal and miss the statement after it.
        let characters = sql.unicodeScalars.map(Character.init)
        var index = 0

        func flush() {
            if !current.isEmpty {
                words.append(current.uppercased())
                current = ""
            }
        }

        while index < characters.count {
            let character = characters[index]

            // `-- line comment`
            if character == "-", index + 1 < characters.count, characters[index + 1] == "-" {
                flush()
                while index < characters.count, characters[index] != "\n" { index += 1 }
                continue
            }
            // `/* block comment */`, not nested, matching the engine's scanner.
            if character == "/", index + 1 < characters.count, characters[index + 1] == "*" {
                flush()
                index += 2
                while index + 1 < characters.count,
                      !(characters[index] == "*" && characters[index + 1] == "/") {
                    index += 1
                }
                index = min(index + 2, characters.count)
                continue
            }
            // `'string literal'`, with `''` as an escaped quote.
            if character == "'" {
                flush()
                index += 1
                while index < characters.count {
                    if characters[index] == "'" {
                        if index + 1 < characters.count, characters[index + 1] == "'" {
                            index += 2
                            continue
                        }
                        index += 1
                        break
                    }
                    index += 1
                }
                continue
            }
            // `"quoted identifier"`, with `""` as an escaped quote.
            if character == "\"" {
                flush()
                index += 1
                while index < characters.count {
                    if characters[index] == "\"" {
                        if index + 1 < characters.count, characters[index + 1] == "\"" {
                            index += 2
                            continue
                        }
                        index += 1
                        break
                    }
                    index += 1
                }
                continue
            }
            // `` `quoted identifier` ``, MySQL's spelling.
            if character == "`" {
                flush()
                index += 1
                while index < characters.count {
                    if characters[index] == "`" {
                        if index + 1 < characters.count, characters[index + 1] == "`" {
                            index += 2
                            continue
                        }
                        index += 1
                        break
                    }
                    index += 1
                }
                continue
            }
            // `$tag$ dollar-quoted body $tag$`.
            if character == "$", let tag = dollarTag(characters, from: index) {
                flush()
                if let closed = closingDollar(characters, tag: tag.tag, from: tag.next) {
                    index = closed
                } else {
                    // No closing tag: the opening tag is not a word either way.
                    index = tag.next
                }
                continue
            }
            if character.isLetter || character.isNumber || character == "_" {
                current.append(character)
                index += 1
                continue
            }
            flush()
            index += 1
        }
        flush()
        return words
    }

    /// A `$tag$` opener at `start`, or `nil` when the `$` starts something else.
    private static func dollarTag(_ characters: [Character], from start: Int)
        -> (tag: String, next: Int)? {
        guard characters[start] == "$" else { return nil }
        var index = start + 1
        while index < characters.count {
            let character = characters[index]
            if character == "$" {
                return (String(characters[start...index]), index + 1)
            }
            // A tag is an identifier: letters, digits and underscores, and cannot start with one
            // of the digits (`$1` is a parameter, not a tag).
            if character.isLetter || character == "_"
                || (character.isNumber && index > start + 1) {
                index += 1
                continue
            }
            return nil
        }
        return nil
    }

    /// The index just past the closing `tag`, or `nil` when there is none.
    private static func closingDollar(_ characters: [Character], tag: String,
                                      from start: Int) -> Int? {
        let closing = Array(tag)
        guard !closing.isEmpty, start + closing.count <= characters.count else { return nil }
        var index = start
        while index + closing.count <= characters.count {
            if Array(characters[index..<(index + closing.count)]) == closing {
                return index + closing.count
            }
            index += 1
        }
        return nil
    }
}

/// A destructive operation the tree can run: truncate or drop, through the engine's `table_op`.
///
/// The engine reads `TABLE_OP=drop|truncate` and the same `TARGET_*` triple `to_table` takes
/// (`crates/qh-ffi/src/commands.rs`). The statement named here is what the confirmation sheet and
/// the log show the user; it is the operation and its target, spelled the way the app spells a
/// qualified name, not a byte-for-byte copy of what the engine sends.
enum TableOperation: String, CaseIterable, Identifiable {
    case truncate
    case drop

    var id: Self { self }

    var title: String {
        switch self {
        case .truncate: "Truncate Table"
        case .drop: "Drop Table"
        }
    }

    var verb: String {
        switch self {
        case .truncate: "TRUNCATE TABLE"
        case .drop: "DROP TABLE"
        }
    }

    var symbol: String {
        switch self {
        case .truncate: "eraser"
        case .drop: "trash"
        }
    }

    /// The one line the confirmation names and the log records.
    func statement(table: String) -> String { "\(verb) \(table)" }

    /// The `TARGET_*` parts a tree node names, in the shape the engine asks a driver for.
    ///
    /// Every part is sent; the engine's own `slots` decides which ones a driver has, so a Postgres
    /// node's empty catalog is ignored there rather than guessed at here.
    static func targetSettings(catalog: String?, schema: String?, table: String) -> [String: String] {
        [
            "TARGET_CATALOG": catalog ?? "",
            "TARGET_SCHEMA": schema ?? "",
            "TARGET_TABLE": table,
        ]
    }
}
