import Foundation
import Observation

/// How an import treats a bad row, as the engine's `ON_ERROR` names it.
///
/// The three come from the engine's transaction policy (ADR-0019), and the labels say what the row
/// does rather than borrowing a name: `stop` rolls the whole import back, `commit` keeps what came
/// before the bad row, and `skip` leaves the row out and keeps going **without a transaction**. The
/// sheet states the last part rather than letting "skip" imply a safety it does not have. A `.sql`
/// file is the same policy over statements, so the words change and the rule does not.
enum ImportErrorMode: String, CaseIterable, Identifiable {
    case stop
    case commit
    case skip

    var id: Self { self }

    func title(for format: ImportSourceFormat) -> String {
        switch self {
        case .stop: "Stop and roll back"
        case .commit: "Stop and keep"
        case .skip: format.importsStatements ? "Skip the statement" : "Skip the row"
        }
    }

    func detail(for format: ImportSourceFormat) -> String {
        let (unit, plural) = format.importsStatements ? ("statement", "Statements") : ("row", "Rows")
        switch self {
        case .stop: return "The default. One bad \(unit) rolls the whole import back; nothing lands."
        case .commit: return "\(plural) before a bad \(unit) stay; the import reports the line it stopped at."
        case .skip: return "Bad \(unit)s are left out and the import continues. No transaction."
        }
    }
}

/// The source file's shape, as far as the app needs it to build the engine's settings.
enum ImportSourceFormat: String, CaseIterable, Identifiable {
    case csv
    case tsv
    case xlsx
    /// One JSON array of objects, or JSONL / NDJSON: the engine has one reader for all three.
    case json
    /// A script of statements. It is the other import family: no target table and no fields,
    /// because the file carries its own.
    case sql

    var id: Self { self }

    /// What the engine's `IMPORT_FORMAT` says. A `.tsv` is a CSV with a tab delimiter, because the
    /// engine has one row format with a configurable separator.
    var engineName: String {
        switch self {
        case .csv, .tsv: "csv"
        case .xlsx: "xlsx"
        case .json: "json"
        case .sql: "sql"
        }
    }

    var label: String {
        switch self {
        case .csv: "CSV"
        case .tsv: "TSV"
        case .xlsx: "XLSX"
        case .json: "JSON"
        case .sql: "SQL"
        }
    }

    /// The delimiter a fresh mapping starts with.
    var delimiter: String { self == .tsv ? "\t" : "," }

    /// Whether the format has a field separator at all.
    var hasDelimiter: Bool { self == .csv || self == .tsv }

    /// Whether the first row can be a header. JSON names its columns with its keys and a `.sql`
    /// file has no columns, so neither is asked.
    var hasHeaderRow: Bool { self == .csv || self == .tsv || self == .xlsx }

    /// Whether the file is statements rather than rows.
    var importsStatements: Bool { self == .sql }

    /// Whether the app can read this format's header itself, which the mapping needs.
    ///
    /// CSV and TSV yes: the header is the first record of a text file. XLSX no: its header lives in
    /// a workbook `calamine` parses on the Rust side, and a second reader in Swift would be a
    /// second answer to "what are this file's columns". The sheet says so and runs the import with
    /// the engine's own header mapping instead of faking a picture of the sheet.
    var appCanReadHeader: Bool { self == .csv || self == .tsv }

    /// Whether the engine lists this format's fields for the sheet (`IMPORT_PREVIEW`), which is how
    /// JSON gets a field list without a second JSON parser in Swift.
    var enginePreviewsFields: Bool { self == .json }

    /// Whether the sheet has a field list to draw at all.
    var hasFieldList: Bool { appCanReadHeader || enginePreviewsFields }

    /// What the sheet says about this format that the controls do not, or `nil` for the three
    /// that already read like a spreadsheet.
    var note: String? {
        switch self {
        case .csv, .tsv, .xlsx:
            nil
        case .json:
            "A JSON array of objects, or one object per line (JSONL). A JSON null and a key an object "
                + "lacks both become NULL; an empty string stays an empty string. Nested objects and "
                + "arrays are written as their JSON text."
        case .sql:
            "The file's own statements run against the connection, and the file names its own tables, "
                + "so there is no target or field list here. An import is all or nothing by default: "
                + "on a server that can roll back it runs as one transaction, and one bad statement "
                + "leaves nothing behind. Safe Mode checks every statement before the engine connects."
        }
    }

    /// The extensions the file panel offers, which are the ones `detect` maps.
    static let panelExtensions = ["csv", "tsv", "xlsx", "json", "jsonl", "ndjson", "sql"]

    /// How the formats read in a sentence, for the "not a file I open" notices.
    static let supportedNames = "CSV, TSV, XLSX, JSON, JSONL or SQL"

    /// The format an extension names, or `nil` for one the import sheet does not open.
    static func detect(path: String) -> ImportSourceFormat? {
        switch (path as NSString).pathExtension.lowercased() {
        case "csv": .csv
        case "tsv", "tab": .tsv
        case "xlsx", "xlsm": .xlsx
        case "json", "jsonl", "ndjson": .json
        case "sql": .sql
        default: nil
        }
    }
}

/// One source column and what it maps to in the target.
struct ImportField: Identifiable, Equatable {
    /// The column's position in the file's header — its identity in `COLUMNS`.
    var source: Int
    /// The file's own name for it, or a generated one when the file has no header.
    var name: String
    /// Whether the import writes this column at all.
    var include: Bool
    /// The target column's name, or blank when the mapping is by header name.
    var target: String

    var id: Int { source }

    /// The name that goes into `COLUMNS`. A blank target falls back to the file's own name, which
    /// is what the engine's header-derived default would do — so "included, nothing picked" and
    /// "included, named like the header" mean the same thing.
    var effectiveTarget: String { target.isEmpty ? name : target }
}

/// What the import sheet edits, and the engine settings it builds.
///
/// Pure and `Equatable` so the two things worth checking can be checked without a window or a
/// server: the `COLUMNS`/`TARGET_*` settings the engine reads, and what the sheet refuses to claim.
/// The engine owns the actual import; this only says which file, which shape and which policy.
struct ImportMapping: Equatable {
    /// The absolute path of the file being read.
    var path = ""
    var format: ImportSourceFormat = .csv
    var delimiter = ","
    var header = true
    /// One per source column, in the file's own order.
    var fields: [ImportField] = []
    var targetCatalog = ""
    var targetSchema = ""
    var targetTable = ""
    /// The target's own columns, when the app could read them. Empty means "unknown", which is the
    /// XLSX case and the missing-table case; the mapping then goes by header name.
    var targetColumns: [String] = []
    var onError: ImportErrorMode = .stop
    /// What an empty cell in the file means. Blank is the engine's default: an empty cell is NULL.
    var nullText = ""
    /// Whether the server's foreign-key checks stay on. Off is a privileged request the engine
    /// refuses where a driver has no session switch, which is why the default is on.
    var foreignKeys = true
    /// Rows per `INSERT`. 0 leaves the engine's own default (200) alone.
    var batchSize = 0

    var fileName: String { (path as NSString).lastPathComponent }

    var trimmedCatalog: String { targetCatalog.trimmingCharacters(in: .whitespaces) }
    var trimmedSchema: String { targetSchema.trimmingCharacters(in: .whitespaces) }
    var trimmedTable: String { targetTable.trimmingCharacters(in: .whitespaces) }

    /// Whether the mapping is the engine's own header-derived one rather than the sheet's.
    var usesHeaderMapping: Bool { targetColumns.isEmpty }

    /// The `COLUMNS` JSON, or `nil` when the engine should derive the mapping from the header.
    ///
    /// Present only when the sheet actually knows the target's columns: with none, there is nothing
    /// to pick from, and the engine's header-derived default is the honest answer. An explicit
    /// empty array is returned when every field is excluded, so the engine refuses with "no columns
    /// to import" rather than falling back to importing all of them.
    ///
    /// For JSON each entry also carries the key's `name`, which the engine checks against the
    /// file's own key at that position: a file that changed since the sheet read it is refused
    /// instead of landing its values in the wrong columns. CSV and XLSX do not send it, because the
    /// app's header and the engine's can differ over a BOM or a quote.
    var columnsJSON: String? {
        guard !format.importsStatements, !fields.isEmpty, !targetColumns.isEmpty else { return nil }
        let rows: [[String: Any]] = fields.map { field in
            var entry: [String: Any] = [
                "source": field.source,
                "target": field.effectiveTarget,
                "include": field.include,
            ]
            if format == .json { entry["name"] = field.name }
            return entry
        }
        guard let data = try? JSONSerialization.data(withJSONObject: rows, options: [.sortedKeys]),
              let json = String(data: data, encoding: .utf8) else { return nil }
        return json
    }

    /// The engine settings this mapping builds. The connection settings are the caller's to add.
    ///
    /// A `.sql` file gets no target and no row settings at all: the engine reads its own statements
    /// and ignores a `TARGET_TABLE`, so sending one would only suggest the table matters.
    func settings() -> [String: String] {
        var env: [String: String] = [
            "IMPORT_PATH": path,
            "IMPORT_FORMAT": format.engineName,
            "ON_ERROR": onError.rawValue,
            "FOREIGN_KEYS": foreignKeys ? "1" : "0",
        ]
        if format.importsStatements { return env }
        env["TARGET_CATALOG"] = trimmedCatalog
        env["TARGET_SCHEMA"] = trimmedSchema
        env["TARGET_TABLE"] = trimmedTable
        // Blank is the engine's own default: CSV and XLSX read an empty cell as NULL, and JSON
        // reads only a real `null` (and a missing key) as NULL, so an empty string stays a value.
        if !nullText.isEmpty { env["NULL_TEXT"] = nullText }
        if format.hasHeaderRow { env["HEADER"] = header ? "1" : "0" }
        if format.hasDelimiter, delimiter != format.delimiter || format == .tsv {
            env["DELIMITER"] = delimiter
        }
        if batchSize > 0 { env["IMPORT_BATCH"] = String(batchSize) }
        if let columns = columnsJSON { env["COLUMNS"] = columns }
        return env
    }

    /// Whether the engine will refuse this import before opening a connection.
    ///
    /// `import_data` is a bulk path checked with the engine's unconfirmed guard, so a `confirm`
    /// connection refuses it outright: one confirmation cannot cover every statement a file turns
    /// into (ADR-0026). The sheet says so, using the engine's reasoning, but the engine is still
    /// the one enforcing it — this is a warning, not the rule.
    static func refusalReason(safeMode: ConnectionSafeMode) -> String? {
        switch safeMode {
        case .full, .noDDL:
            return nil
        case .confirm:
            return "This connection's Safe Mode is Confirm, and an import is a whole script of "
                + "writes: the engine refuses it rather than take one confirmation for all of them. "
                + "Raise the connection to Full or No DDL to import."
        case .readOnly:
            return "This connection's Safe Mode is Read only, so the engine refuses an import."
        }
    }

    /// Whether the sheet has enough to run.
    var ready: Bool {
        guard !path.isEmpty else { return false }
        // A statement file names its own targets, so a path is all it needs.
        if format.importsStatements { return true }
        guard !trimmedTable.isEmpty else { return false }
        // With a real mapping, at least one field must be written; without one the engine's
        // header-derived default covers every column, so there is nothing to check.
        if columnsJSON != nil, !fields.contains(where: \.include) { return false }
        return true
    }

    /// Builds the field list: the file's columns against the target's, matching by name
    /// case-insensitively exactly as the engine's own resolver does.
    ///
    /// With no target columns (XLSX, or a table the app could not describe) every field is included
    /// and carries its own name, which is the mapping the engine would derive by itself.
    static func mappedFields(headers: [String], targetColumns: [String]) -> [ImportField] {
        headers.enumerated().map { index, name in
            let match = targetColumns.first { $0.caseInsensitiveCompare(name) == .orderedSame }
            return ImportField(source: index, name: name,
                               include: targetColumns.isEmpty || match != nil,
                               target: match ?? (targetColumns.isEmpty ? name : ""))
        }
    }

    /// The fields rebuilt against a new set of target columns.
    ///
    /// The first load is the interesting case: the sheet had no target columns and mapped by header
    /// name, and now that the server has described the table the mapping is rebuilt against it.
    /// After that the user's own choices win — a reload only drops a target the new column list no
    /// longer has, so the mapping cannot name a column that is not there.
    func remapped(to columns: [String]) -> ImportMapping {
        var next = self
        let firstLoad = targetColumns.isEmpty
        next.targetColumns = columns
        if firstLoad {
            next.fields = ImportMapping.mappedFields(headers: fields.map(\.name), targetColumns: columns)
            return next
        }
        next.fields = fields.map { field in
            var field = field
            if !field.target.isEmpty,
               !columns.contains(where: { $0.caseInsensitiveCompare(field.target) == .orderedSame }) {
                field.target = ""
                field.include = false
            }
            return field
        }
        return next
    }
}

/// Reads the file's own header row for the mapping sheet.
enum ImportHeaderReader {
    enum Failure: Error, LocalizedError, Equatable {
        case unreadable(String)
        case notText(String)

        var errorDescription: String? {
            switch self {
            case .unreadable(let path): "Couldn't open \(path)."
            case .notText(let path): "\(path) does not look like a text file."
            }
        }
    }

    /// A bounded prefix is enough: the header is the first record, and a file too large to fit the
    /// cap has a header that came well before it.
    private static let prefixLimit = 1 << 20

    /// The first record's fields, or `nil` when the file is not a delimited text file.
    static func readHeaders(path: String, delimiter: Character) throws -> [String] {
        guard let handle = FileHandle(forReadingAtPath: path) else {
            throw Failure.unreadable(path)
        }
        defer { try? handle.close() }
        let prefix = (try? handle.read(upToCount: prefixLimit)) ?? Data()
        guard let text = String(data: prefix, encoding: .utf8) else {
            throw Failure.notText(path)
        }
        return firstRecord(in: text, delimiter: delimiter)
    }

    /// The first record of a CSV/TSV text, RFC 4180 quoting included.
    ///
    /// Quote-aware for the same reason the engine's reader is: a header cell can contain the
    /// delimiter, and splitting on the delimiter alone would turn one column into two. A quoted
    /// newline ends the record for this purpose — a header is one line in every file this app is
    /// likely to meet, and reading further would mean holding more of a file to draw a sheet.
    static func firstRecord(in text: String, delimiter: Character) -> [String] {
        var fields: [String] = []
        var current = ""
        var inQuotes = false
        var index = text.startIndex

        while index < text.endIndex {
            let character = text[index]
            if inQuotes {
                if character == "\"" {
                    let next = text.index(after: index)
                    if next < text.endIndex, text[next] == "\"" {
                        current.append("\"")
                        index = text.index(after: next)
                        continue
                    }
                    inQuotes = false
                } else {
                    current.append(character)
                }
            } else if character == "\"" {
                inQuotes = true
            } else if character == delimiter {
                fields.append(current)
                current = ""
            } else if character == "\n" || character == "\r" {
                fields.append(current)
                return fields
            } else {
                current.append(character)
            }
            index = text.index(after: index)
        }
        fields.append(current)
        return fields
    }
}

/// The engine's answer to one import, as much of it as the sheet shows.
struct ImportOutcome: Equatable {
    var rows: Int = 0
    /// The statements a `.sql` import ran. `nil` for a row import, which reports `rows` instead:
    /// zero statements and no statements are different answers, and only one is a `.sql` file.
    var statements: Int?
    var rejected: Int = 0
    var errors: [String] = []
    /// Whether the engine cut `errors` short, so the list is a beginning and not the whole.
    var errorsTruncated = false
    var stoppedAt: Int?
    var mode: String = "stop"
    var disposition: String?
    var transaction = false
    /// What the engine says the import left out without failing, such as JSON keys that first
    /// appeared after the rows that named the columns.
    var warnings: [String] = []
}

extension ImportOutcome {
    /// The outcome a `done` event carries.
    init(done event: Event) {
        self.init(rows: event.rows ?? 0,
                  statements: event.statements?.count,
                  rejected: event.rejected ?? 0,
                  errors: event.errors ?? [],
                  errorsTruncated: event.errorsTruncated ?? false,
                  stoppedAt: event.stoppedAt,
                  mode: event.mode ?? "",
                  disposition: event.disposition,
                  transaction: event.transaction ?? false,
                  warnings: event.warnings ?? [])
    }
}

/// The keys of a JSON file, read by the engine.
///
/// `IMPORT_PREVIEW=1` opens the file with the importer's own reader and answers with the columns it
/// would map, without connecting. That is the field list for JSON: a second JSON parser in Swift
/// would be a second answer to "what are this file's columns", the same reason XLSX has none.
enum ImportJSONPreview {
    struct Failure: Error, LocalizedError, Equatable {
        var message: String
        var errorDescription: String? { message }
    }

    /// One sample row is all the engine is asked for: only the keys are used, and a larger sample
    /// would only make the answer longer.
    static func environment(path: String) -> [String: String] {
        AppModel.localEnvironment([
            "IMPORT_PATH": path,
            "IMPORT_FORMAT": ImportSourceFormat.json.engineName,
            "IMPORT_PREVIEW": "1",
            "IMPORT_PREVIEW_ROWS": "1",
        ])
    }

    /// Asks the engine for the file's keys, in the order it first saw them. The completion runs on
    /// the main queue, like every engine callback.
    static func load(path: String, engine: any DatabaseEngine,
                     completion: @escaping (Result<[String], Failure>) -> Void) {
        var keys: [String] = []
        var message: String?
        engine.run("import_data", env: environment(path: path), onEvent: { event in
            switch event.event {
            case "columns": keys = (event.columns ?? []).map(\.name)
            case "error": message = event.message
            default: break
            }
        }, onExit: { status, log in
            if status != 0 {
                completion(.failure(Failure(
                    message: message ?? log.split(separator: "\n").last.map(String.init)
                        ?? "The engine exited with status \(status).")))
            } else if keys.isEmpty {
                completion(.failure(Failure(message: "The file has no JSON objects to read keys from.")))
            } else {
                completion(.success(keys))
            }
        })
    }
}

/// One import being set up, and the transient state of loading the target's columns while it is.
///
/// A class because the sheet edits it in place and the model holds it as a sheet item; the mapping
/// inside stays a value so its builders can be tested without the window around them.
@Observable
final class ImportDraft: Identifiable {
    let id = UUID()
    var mapping: ImportMapping
    /// The connection the target belongs to. `nil` only before a file has been chosen.
    var connectionID: UUID?
    /// Set while the app is asking the server for the target's columns.
    var loadingColumns = false
    /// Set while the engine is reading a JSON file's keys for the field list.
    var loadingFields = false
    /// Why the JSON keys could not be read, in a sentence the sheet can show.
    var fieldsError: String?
    /// Why the last column read failed, in a sentence the sheet can show.
    var columnsError: String?
    /// Set while the engine is importing, so the sheet can disable its own button.
    var running = false
    /// What the engine answered, once it has.
    var outcome: ImportOutcome?
    /// Why the import failed, when it did.
    var failure: String?

    init(mapping: ImportMapping, connectionID: UUID?) {
        self.mapping = mapping
        self.connectionID = connectionID
    }
}
