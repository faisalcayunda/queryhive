import AppKit
import SwiftUI

/// The nine output formats the engine can write, with the display metadata the picker grid and
/// the options panel both read. `fullLabel` matches the engine's own `FORMAT_LABELS`.
enum ExportFormat: String, CaseIterable, Identifiable {
    case txt, csv, json, xml, html, sql, xls, xlsx, dbf

    var id: Self { self }

    var label: String {
        switch self {
        case .txt: "TXT"
        case .csv: "CSV"
        case .json: "JSON"
        case .xml: "XML"
        case .html: "HTML"
        case .sql: "SQL"
        case .xls: "XLS"
        case .xlsx: "XLSX"
        case .dbf: "DBF"
        }
    }

    var fullLabel: String {
        switch self {
        case .txt: "Text file (*.txt)"
        case .csv: "CSV file (*.csv)"
        case .json: "JSON file (*.json)"
        case .xml: "XML file (*.xml)"
        case .html: "HTML file (*.htm;*.html)"
        case .sql: "SQL script file (*.sql)"
        case .xls: "Excel file (*.xls)"
        case .xlsx: "Excel file (2007 or later) (*.xlsx)"
        case .dbf: "DBase file (*.dbf)"
        }
    }

    var symbol: String {
        switch self {
        case .txt: "text.alignleft"
        case .csv: "tablecells"
        case .json: "curlybraces"
        case .xml: "chevron.left.forwardslash.chevron.right"
        case .html: "globe"
        case .sql: "terminal"
        case .xls: "tablecells"
        case .xlsx: "tablecells.fill"
        case .dbf: "cylinder"
        }
    }

    var tint: Color {
        switch self {
        case .txt: Tone.ice
        case .csv: Tone.mint
        case .json: Tone.violet
        case .xml: Tone.amber
        case .html: Tone.blue
        case .sql: Tone.coral
        case .xls, .xlsx: Tone.mint
        case .dbf: Tone.gray
        }
    }

    /// One line under the tile in the picker, so the choice is informed before Run.
    var note: String {
        switch self {
        case .txt: "Tab-separated, one header row"
        case .csv: "RFC 4180 quoting, Excel-safe"
        case .json: "Array of objects, or one per line"
        case .xml: "<RECORDS><RECORD>"
        case .html: "Standalone page, sticky header"
        case .sql: "Multi-row INSERT statements"
        case .xls: "BIFF8, splits at 65,535 rows"
        case .xlsx: "Written streaming, splits at 1,048,576 rows"
        case .dbf: "dBase III+, fixed-width fields"
        }
    }

    /// Formats whose writer splits by itself: the split control is pointless for them.
    var splitsItself: Bool { self == .xls || self == .xlsx }
}

/// Where a run puts its result: a file on disk, or a table on the coordinator. Navicat's two
/// export shapes, and the reason the toolbar swaps its controls when this changes.
enum Destination: String, CaseIterable, Identifiable {
    case file, table

    var id: Self { self }
    var label: String { self == .file ? "File" : "Table" }
}

/// How a table destination treats a table that already exists.
enum WriteMode: String, CaseIterable, Identifiable {
    case create, replace, append

    var id: Self { self }

    var label: String {
        switch self {
        case .create: "Create"
        case .replace: "Replace"
        case .append: "Append"
        }
    }

    var symbol: String {
        switch self {
        case .create: "plus.circle"
        case .replace: "arrow.triangle.2.circlepath"
        case .append: "text.append"
        }
    }

    var tint: Color {
        switch self {
        case .create: Tone.mint
        case .replace: Tone.coral
        case .append: Tone.ice
        }
    }

    /// What actually runs, for the log and the shell prompt.
    var statement: String {
        switch self {
        case .create: "CREATE TABLE"
        case .replace: "DROP + CREATE TABLE"
        case .append: "INSERT INTO"
        }
    }

    var help: String {
        switch self {
        case .create: "Creates the table. Fails if it already exists."
        case .replace: "Drops the existing table first, then recreates it from the query. If the query then fails, the old table is already gone."
        case .append: "Inserts the query's rows into an existing table."
        }
    }
}

/// One line in the bottom panel's Log, the Navicat "Messages" pane. The engine's JSON events
/// arrive here translated, so the panel reads like a session transcript rather than a protocol
/// dump.
struct LogLine: Identifiable {
    enum Kind {
        case info, success, warning, error

        var tint: Color {
            switch self {
            case .info: Tone.secondary
            case .success: Tone.mint
            case .warning: Tone.amber
            case .error: Tone.coral
            }
        }

        var symbol: String {
            switch self {
            case .info: "arrow.right"
            case .success: "checkmark"
            case .warning: "exclamationmark.triangle.fill"
            case .error: "xmark.octagon.fill"
            }
        }
    }

    let id = UUID()
    let at: Date
    let kind: Kind
    let text: String
}

/// Which panel is showing under the SQL editor.
enum PanelTab: String, CaseIterable, Identifiable {
    case log, columns, files

    var id: Self { self }

    /// The third panel shows files for a file run and the written table for a table run; calling
    /// it "Files" while a CTAS is running would just be wrong.
    func label(for destination: Destination) -> String {
        switch self {
        case .log: "Log"
        case .columns: "Columns"
        case .files: destination == .table ? "Table" : "Files"
        }
    }

    /// The title shown on the row of tabs, which does not depend on the destination.
    var label: String { label(for: .file) }
}

/// One query tab: the SQL, where it goes, what happened when it ran, and the bottom panel's
/// contents. Navicat's unit of work, and the reason several exports can be in flight at once.
@Observable
final class QueryTab: Identifiable {
    enum Stage { case idle, running, done, failed }

    let id = UUID()
    var title: String

    // MARK: Query

    var sql = ""
    var connectionID: UUID?

    // MARK: Destination

    /// Which of the two shapes this run takes. Switching it swaps the toolbar's controls.
    var destination: Destination = .file

    var outputName = "export"
    var outputDirectory: URL?
    var format: ExportFormat = .csv {
        didSet { formatChanged(from: oldValue) }
    }
    var zip = false
    var batchSize = 10_000

    // MARK: Table destination

    var targetCatalog = ""
    var targetSchema = ""
    var targetTable = ""
    var writeMode: WriteMode = .create
    /// Filled from the `done` event of a table run.
    var writtenTable: String?
    /// 0 means "one file, however big it gets".
    var splitRows = 0
    var retries = 5

    // MARK: Format options

    var delimiter = ","
    var encoding = "utf-8"
    var header = true
    var bom = false
    var nullText = ""
    var jsonl = false
    var sqlTable = ""
    var sheet = "Sheet1"
    var dbfCharWidth = 254

    // MARK: Run state

    var stage = Stage.idle
    var step: String?
    var rows = 0
    var columns: [Event.Column] = []
    var files: [Event.ExportedFile] = []
    var warnings: [String] = []
    var queryID: String?
    var failure: String?
    var failureDetail = ""
    var logLines: [LogLine] = []
    var panel: PanelTab = .log
    var startedAt = Date.now
    var finishedAt = Date.now

    var process: Process?
    var runToken = UUID()
    var cancelled = false
    /// Set while the debounced... no: set when the user has asked to stop but the engine has
    /// not exited yet, so the toolbar can disable Stop instead of queueing more signals.
    var stopping = false

    init(title: String) {
        self.title = title
        outputDirectory = ConnectionStore.defaultOutputDirectory
    }

    // MARK: Derived

    var trimmedName: String { outputName.trimmingCharacters(in: .whitespaces) }
    var trimmedCatalog: String { targetCatalog.trimmingCharacters(in: .whitespaces) }
    var trimmedSchema: String { targetSchema.trimmingCharacters(in: .whitespaces) }
    var trimmedTable: String { targetTable.trimmingCharacters(in: .whitespaces) }

    /// The target's dotted name with exactly the parts this driver has. Postgres writes inside
    /// the database its connection already opened, so it has no catalog part; MySQL has no schema.
    func target(for kind: ConnectionKind) -> String {
        switch kind {
        case .trino: "\(trimmedCatalog).\(trimmedSchema).\(trimmedTable)"
        case .postgres: "\(trimmedSchema).\(trimmedTable)"
        case .mysql: "\(trimmedCatalog).\(trimmedTable)"
        }
    }

    /// True when a run would drop a table: the toolbar turns coral and the orb asks first.
    var isDestructive: Bool { destination == .table && writeMode == .replace }

    /// One line naming what the run will do, used by the log and the status bar.
    func runSummary(for kind: ConnectionKind) -> String {
        switch destination {
        case .file:
            return "\(format.label) → \(outputDirectory?.path ?? "no folder")/\(trimmedName)"
        case .table:
            return "\(writeMode.statement) \(target(for: kind))"
        }
    }

    var hasSQL: Bool { !sql.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    var totalBytes: Int { files.reduce(0) { $0 + $1.bytes } }

    var elapsedText: String { elapsed(from: startedAt, to: stage == .running ? .now : finishedAt) }

    /// The one line the status bar shows for this tab.
    var summary: String {
        switch stage {
        case .idle:
            return hasSQL ? "Ready" : "No query"
        case .running:
            return "Running · \(rows.formatted()) rows written"
        case .done:
            if destination == .table {
                // `writtenTable` comes from the engine and is already in the driver's own
                // shape; the fallback is only reached if a table run reported nothing.
                return "Query OK · \(pluralized(rows, "row")) → \(writtenTable ?? trimmedTable) · \(elapsedText)"
            }
            return "Query OK · \(pluralized(rows, "row")) · \(files.count == 1 ? "1 file" : "\(files.count) files") · \(elapsedText)"
        case .failed:
            return failure ?? "Failed"
        }
    }

    // MARK: Log

    func note(_ kind: LogLine.Kind, _ text: String) {
        logLines.append(LogLine(at: .now, kind: kind, text: text))
    }

    // MARK: Format defaults

    private func formatChanged(from old: ExportFormat) {
        // The delimiter only follows the format while the user hasn't diverged from the
        // previous format's own default: switching to CSV after typing ";" keeps the ";".
        if format == .csv, delimiter == "\t" { delimiter = "," }
        if format == .txt, delimiter == "," { delimiter = "\t" }
        if format == .sql, sqlTable.isEmpty { sqlTable = trimmedName }
        if (format == .xls || format == .xlsx), sheet.isEmpty { sheet = "Sheet1" }
        // The two Excel writers split by themselves; a hand-set split would be a second
        // ceiling the user never asked for.
        if format.splitsItself { splitRows = 0 }
        _ = old
    }

    /// Called by the table context menu and the tree's double-click: put a name in the editor
    /// without destroying what is already there.
    func insertIntoSQL(_ text: String) {
        if sql.isEmpty || sql.hasSuffix("\n") || sql.hasSuffix(" ") {
            sql += text
        } else {
            sql += " " + text
        }
    }

    func revealFiles() {
        let existing = files.map(\.url).filter { FileManager.default.fileExists(atPath: $0.path) }
        if !existing.isEmpty {
            NSWorkspace.shared.activateFileViewerSelecting(existing)
        } else if let directory = outputDirectory, FileManager.default.fileExists(atPath: directory.path) {
            NSWorkspace.shared.open(directory)
        }
    }

    func loadSQLFromFile() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = AppModel.sqlContentTypes
        panel.allowsMultipleSelection = false
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            sql = try String(contentsOf: url, encoding: .utf8)
            if title.hasPrefix("Query ") { title = url.deletingPathExtension().lastPathComponent }
        } catch {
            note(.error, "Couldn't read \(url.lastPathComponent): \(error.localizedDescription)")
        }
    }
}
