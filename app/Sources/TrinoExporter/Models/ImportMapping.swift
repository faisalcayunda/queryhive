import Foundation
import Observation

/// How an import treats a bad row, as the engine's `ON_ERROR` names it.
///
/// The three come from the engine's transaction policy (ADR-0019), and the labels say what the row
/// does rather than borrowing a name: `stop` rolls the whole import back, `commit` keeps what came
/// before the bad row, and `skip` leaves the row out and keeps going **without a transaction**. The
/// sheet states the last part rather than letting "skip" imply a safety it does not have.
enum ImportErrorMode: String, CaseIterable, Identifiable {
    case stop
    case commit
    case skip

    var id: Self { self }

    var title: String {
        switch self {
        case .stop: "Stop and roll back"
        case .commit: "Stop and keep"
        case .skip: "Skip the row"
        }
    }

    var detail: String {
        switch self {
        case .stop: "The default. One bad row rolls the whole import back; nothing lands."
        case .commit: "Rows before a bad row stay; the import reports the line it stopped at."
        case .skip: "Bad rows are left out and the import continues. No transaction."
        }
    }
}

/// The source file's shape, as far as the app needs it to build the engine's settings.
enum ImportSourceFormat: String, CaseIterable, Identifiable {
    case csv
    case tsv
    case xlsx

    var id: Self { self }

    /// What the engine's `IMPORT_FORMAT` says. A `.tsv` is a CSV with a tab delimiter, because the
    /// engine has one row format with a configurable separator.
    var engineName: String { self == .xlsx ? "xlsx" : "csv" }

    var label: String {
        switch self {
        case .csv: "CSV"
        case .tsv: "TSV"
        case .xlsx: "XLSX"
        }
    }

    /// The delimiter a fresh mapping starts with.
    var delimiter: String { self == .tsv ? "\t" : "," }

    /// Whether the app can read this format's header itself, which the mapping needs.
    ///
    /// CSV and TSV yes: the header is the first record of a text file. XLSX no: its header lives in
    /// a workbook `calamine` parses on the Rust side, and a second reader in Swift would be a
    /// second answer to "what are this file's columns". The sheet says so and runs the import with
    /// the engine's own header mapping instead of faking a picture of the sheet.
    var appCanReadHeader: Bool { self != .xlsx }

    /// The format an extension names, or `nil` for one the import sheet does not open.
    static func detect(path: String) -> ImportSourceFormat? {
        switch (path as NSString).pathExtension.lowercased() {
        case "csv": .csv
        case "tsv", "tab": .tsv
        case "xlsx", "xlsm": .xlsx
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
    var columnsJSON: String? {
        guard !fields.isEmpty, !targetColumns.isEmpty else { return nil }
        let rows: [[String: Any]] = fields.map { field in
            [
                "source": field.source,
                "target": field.effectiveTarget,
                "include": field.include,
            ]
        }
        guard let data = try? JSONSerialization.data(withJSONObject: rows, options: [.sortedKeys]),
              let json = String(data: data, encoding: .utf8) else { return nil }
        return json
    }

    /// The engine settings this mapping builds. The connection settings are the caller's to add.
    func settings() -> [String: String] {
        var env: [String: String] = [
            "IMPORT_PATH": path,
            "IMPORT_FORMAT": format.engineName,
            "TARGET_CATALOG": trimmedCatalog,
            "TARGET_SCHEMA": trimmedSchema,
            "TARGET_TABLE": trimmedTable,
            "ON_ERROR": onError.rawValue,
            "HEADER": header ? "1" : "0",
            "FOREIGN_KEYS": foreignKeys ? "1" : "0",
        ]
        if !nullText.isEmpty { env["NULL_TEXT"] = nullText }
        if delimiter != format.delimiter || format == .tsv { env["DELIMITER"] = delimiter }
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
        guard !path.isEmpty, !trimmedTable.isEmpty else { return false }
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
    var rejected: Int = 0
    var errors: [String] = []
    var stoppedAt: Int?
    var mode: String = "stop"
    var disposition: String?
    var transaction = false
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
