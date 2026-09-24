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

/// What a Run or an Export actually sends.
///
/// "Run the query" means the selection when there is one, because that is what every SQL client
/// does and what someone who has just highlighted three lines expects.
enum QuerySource {
    case selection
    case statement
    case all
}

/// One column's filter.
///
/// Contains by default, because that is what someone typing a fragment of an id expects. A leading
/// `=`, `>`, `<`, `>=` or `<=` switches to a comparison, which is what a numeric column wants. It
/// filters the rows the preview already holds — it never reaches the server and never touches the
/// statement — so the grid's footer has to say so.
/// One column's filter.
///
/// Two modes, and which one a column gets is decided by its data rather than chosen by the user.
/// A column with few distinct values is better served by picking them from a list — that is what
/// someone filtering a `jenis_kelamin` column actually wants, and it cannot produce a no-match
/// typo. Past `valuePickerLimit` distinct values a list stops being browsable, so the filter
/// becomes free text instead: contains by default, with a leading `=`, `>`, `<`, `>=` or `<=` for
/// a comparison.
///
/// Either way it filters the rows the preview already holds — it never reaches the server and never
/// touches the statement — so the grid's footer has to say so.
enum ColumnFilter: Equatable {
    /// Exact matches against values picked from the column's own distinct list.
    case values(Set<String>)
    /// A comparison or a substring.
    case text(String)

    /// Past this many distinct values the picker becomes a search box.
    static let valuePickerLimit = 10

    /// The distinct values offered for a column, in a stable order: `nil` first because a NULL is
    /// its own state and not a value, then the non-nulls sorted so the list does not reshuffle
    /// between runs. Only the first `valuePickerLimit + 1` are needed to make the decision, but the
    /// full set is cheap over at most a preview's worth of rows.
    static func distinctValues(in rows: [[String?]], column: Int) -> [String?] {
        var seen = Set<String>()
        var hasNull = false
        for row in rows {
            guard column < row.count, let value = row[column] else {
                hasNull = true
                continue
            }
            seen.insert(value)
        }
        return (hasNull ? [nil] : []) + seen.sorted()
    }

    var isEmpty: Bool {
        switch self {
        case .values(let picked): picked.isEmpty
        case .text(let needle): needle.trimmingCharacters(in: .whitespaces).isEmpty
        }
    }

    /// A short description for the header's tooltip.
    var label: String {
        switch self {
        case .values(let picked):
            picked.count == 1 ? (picked.first ?? "") : "\(picked.count) values"
        case .text(let needle):
            needle
        }
    }

    func matches(_ value: String?) -> Bool {
        switch self {
        case .values(let picked):
            // A NULL is represented by a sentinel string, because a Set cannot hold nil and the
            // picker has to be able to offer it: "show me the rows with no value here" is a real
            // question about a column full of them.
            guard let value else { return picked.contains(ColumnFilter.nullToken) }
            return picked.contains(value)
        case .text(let needle):
            return ColumnFilter.matchesText(value, needle)
        }
    }

    /// The picker's stand-in for SQL NULL.
    static let nullToken = "\u{0}null"

    static func matchesText(_ value: String?, _ filter: String) -> Bool {
        let needle = filter.trimmingCharacters(in: .whitespaces)
        guard !needle.isEmpty else { return true }
        // A NULL is not a value that can contain anything, and it is not the empty string either.
        guard let value else { return false }

        for op in [">=", "<=", ">", "<", "="] where needle.hasPrefix(op) {
            let operand = String(needle.dropFirst(op.count)).trimmingCharacters(in: .whitespaces)
            guard !operand.isEmpty else { break }
            if op == "=" { return value.localizedCaseInsensitiveCompare(operand) == .orderedSame }
            // Text ordering when either side is not a number, so a filter on a date column still
            // does something instead of matching nothing.
            if let left = Double(value), let right = Double(operand) {
                switch op {
                case ">=": return left >= right
                case "<=": return left <= right
                case ">": return left > right
                default: return left < right
                }
            }
            switch op {
            case ">=": return value >= operand
            case "<=": return value <= operand
            case ">": return value > operand
            default: return value < operand
            }
        }
        return value.localizedCaseInsensitiveContains(needle)
    }
}

/// The rows a Run fetched, rendered by the grid.
struct PreviewResult {
    var columns: [Event.Column]
    var rows: [[String?]]
    /// The row limit stopped it short, so what is on screen is not the whole result.
    var truncated: Bool
    var queryID: String?
    var elapsedMS: Int

    /// What the grid's footer says it is showing. It must never imply the grid holds everything.
    var summary: String {
        truncated
            ? "First \(rows.count.formatted()) rows · limit reached"
            : pluralized(rows.count, "row")
    }
}

/// Which panel is showing under the SQL editor.
///
/// No longer includes Columns: the grid's own header carries every column name and its type chip,
/// so a separate list of them was the same information twice.
enum PanelTab: String, CaseIterable, Identifiable {
    case result, log, files

    var id: Self { self }

    /// The third panel shows files for a file run and the written table for a table run; calling
    /// it "Files" while a CTAS is running would just be wrong.
    func label(for destination: Destination) -> String {
        switch self {
        case .result: "Result"
        case .log: "Log"
        case .files: destination == .table ? "Table" : "Files"
        }
    }

    /// The title shown on the row of tabs, which does not depend on the destination.
    var label: String { label(for: .file) }
}

/// One query tab: the SQL, where it goes, what happened when it ran, and the bottom panel's
/// contents. Navicat's unit of work, and the reason several exports can be in flight at once.
/// Which objects a tab is showing.
///
/// One value rather than three loose fields, so "is this scope already open" is an equality check
/// and so the two engine settings always travel with the connection they belong to.
struct ObjectScope: Equatable {
    var connectionID: UUID
    /// Trino's catalog, MySQL's database; empty for Postgres, whose database is fixed by the
    /// connection and already in the environment it builds.
    var catalog: String
    /// Trino's and Postgres's schema; empty for MySQL, which has no such level.
    var schema: String

    /// What the pane's header names, and nothing when neither level has a name.
    var title: String {
        [catalog, schema].filter { !$0.isEmpty }.joined(separator: ".")
    }
}

@Observable
final class QueryTab: Identifiable {
    enum Stage { case idle, running, done, failed }

    let id = UUID()
    var title: String

    // MARK: Objects

    /// Set when this tab lists a schema's objects instead of holding a query. A tab is one or the
    /// other for its whole life: the pane, the toolbar and the run controls all key off this, and
    /// a tab that could be both would need every one of them to ask twice.
    var objectScope: ObjectScope?
    var objectColumns: [String] = []
    var objectRows: [[String?]] = []
    var objectLoading = false
    var objectError: String?
    /// Guards a stale reply: a second load updates the token, and the first run's late events are
    /// then ignored instead of overwriting the newer list.
    var objectToken: UUID?
    var objectProcess: (any EngineRun)?

    /// The row the user has clicked, by index into `objectRows`.
    ///
    /// An index rather than the name, because the grid has to keep the same row highlighted while
    /// a reload is in flight and the driver's own ordering is the only order there is. A reload
    /// that changes the list clears it, since the index would then point at a different table.
    var objectSelection: Int?

    /// The selected table's own columns, fetched when the row is clicked.
    ///
    /// Separate from `objectColumns`, which is the *listing's* shape — Name/Type on Trino,
    /// Name/OID/Owner/ACL on Postgres. This is the table's schema, and it is a different question
    /// with a different answer, so it gets its own fields rather than sharing a list whose meaning
    /// depends on which one was loaded last.
    var objectDetailColumns: [Event.Column] = []
    var objectDetailLoading = false
    var objectDetailError: String?
    /// Guards a stale reply, exactly as `objectToken` does for the listing.
    var objectDetailToken: UUID?
    var objectDetailProcess: (any EngineRun)?

    /// The table the detail fields describe, so the inspector can name it while it loads and can
    /// tell that a stale answer belongs to a row the user has already left.
    var objectDetailTable: String?

    var isObjects: Bool { objectScope != nil }

    /// The `Name` cell of one row, which is the table name every driver puts first.
    ///
    /// Found by header rather than by position: the three drivers agree that the first column is
    /// the name, but the *rest* of the columns differ, so the index has to come from the header
    /// the driver actually sent rather than from a constant that only holds for two of them.
    func objectName(at row: Int) -> String? {
        guard objectRows.indices.contains(row),
              let column = objectColumns.firstIndex(where: { $0.caseInsensitiveCompare("Name") == .orderedSame }),
              objectRows[row].indices.contains(column) else { return nil }
        let name = objectRows[row][column]?.trimmingCharacters(in: .whitespaces) ?? ""
        return name.isEmpty ? nil : name
    }

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

    // MARK: Preview

    /// How many rows a Run fetches. Navicat calls this the row limit and keeps it with the grid;
    /// it is a property of looking, not of the query, so it is not part of the statement.
    var rowLimit = 1000

    /// Where the caret is, in UTF-16 units, as `NSTextView` reports it. Published by the editor
    /// so "Run Current Statement" knows which one the user is looking at.
    var caret = 0

    var previewing = false
    var preview: PreviewResult?
    var previewError: String?

    /// The plan Explain last fetched, and whether the grid is showing it instead of rows.
    ///
    /// The plan lands in the same grid because it *is* a result set — one text column on Trino and
    /// Postgres, a table on MySQL — so the grid already renders it correctly and there is no second
    /// view to keep in step. `showingPlan` is what tells the footer not to quote row counts for it.
    var explaining = false
    var showingPlan = false

    /// The catalog/schema (Trino) or database/schema (Postgres, MySQL) this tab runs *in*, picked
    /// from the toolbar cascade. Blank means "whatever the connection says", which is the state a
    /// new tab starts in — so the connection stays the single place a default is set, and the tab
    /// only records a deliberate deviation from it.
    var contextDatabase = ""
    var contextSchema = ""

    /// The editor's selection, in UTF-16 units, republished whenever it changes. Length 0 means
    /// nothing is highlighted.
    var selection = NSRange(location: 0, length: 0)

    /// The SQL the grid is actually showing. A count must be of *this*, not of whatever the editor
    /// holds now — the user may have typed since the run, and a total for a different statement
    /// would be a number that looks authoritative and is not.
    var previewedSQL: String?

    /// How many rows the statement really returns, once the user has asked. `nil` means "not
    /// asked", which is different from "fewer than the limit".
    var totalRows: Int?
    var countingRows = false
    var countError: String?

    /// Filters by column index: a result may repeat a name and the grid draws by position.
    /// Cleared whenever new rows arrive, because the ones they described are gone.
    var columnFilters: [Int: ColumnFilter] = [:]

    /// The SQL one source resolves to. Every path out of the editor goes through this, so
    /// "the selected query" means the same thing to Run and to Export.
    func sql(for source: QuerySource) -> String {
        switch source {
        case .all:
            return sql
        case .selection:
            let text = sql as NSString
            guard selection.length > 0, selection.location >= 0,
                  NSMaxRange(selection) <= text.length else { return sql }
            return text.substring(with: selection)
        case .statement:
            return sqlStatement(in: sql, atUTF16Offset: selection.location) ?? sql
        }
    }

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

    var process: (any EngineRun)?
    var runToken = UUID()
    /// A preview is its own run and its own token: pressing Run while an export is in flight
    /// must not be able to cancel the export, or vice versa.
    var previewProcess: (any EngineRun)?
    var previewToken = UUID()
    /// The count is its own run and token too: asking for a total must not disturb the run it
    /// is asking about.
    var countProcess: (any EngineRun)?
    var countToken = UUID()
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
        loadSQL(from: url)
    }

    func loadSQL(from url: URL) {
        do {
            sql = try String(contentsOf: url, encoding: .utf8)
            if title.hasPrefix("Query ") { title = url.deletingPathExtension().lastPathComponent }
        } catch {
            note(.error, "Couldn't read \(url.lastPathComponent): \(error.localizedDescription)")
        }
    }
}

/// The statement the caret sits in.
///
/// A scanner, not a parser: it splits on a `;` that is outside a single-quoted string and outside
/// a `--` or `/* */` comment, which is what a file of ordinary statements needs. A semicolon
/// inside a Postgres dollar-quoted body would fool it, and that is a deliberate trade — such a
/// script is rare, and refusing to guess beats splitting wrongly and running half a statement.
func sqlStatement(in sql: String, atUTF16Offset caret: Int) -> String? {
    var found: [(Range<String.Index>, String)] = []
    var start = sql.startIndex
    var index = sql.startIndex
    var inString = false
    var inLineComment = false
    var inBlockComment = false

    func peek() -> Character? {
        let next = sql.index(after: index)
        return next < sql.endIndex ? sql[next] : nil
    }
    func take() {
        let text = sql[start..<index].trimmingCharacters(in: .whitespacesAndNewlines)
        if !text.isEmpty { found.append((start..<index, text)) }
        start = sql.index(after: index)
    }

    while index < sql.endIndex {
        let character = sql[index]
        if inLineComment {
            if character == "\n" { inLineComment = false }
        } else if inBlockComment {
            if character == "*", peek() == "/" { inBlockComment = false; index = sql.index(after: index) }
        } else if inString {
            if character == "'" { inString = false }
        } else if character == "'" {
            inString = true
        } else if character == "-", peek() == "-" {
            inLineComment = true
            index = sql.index(after: index)
        } else if character == "/", peek() == "*" {
            inBlockComment = true
            index = sql.index(after: index)
        } else if character == ";" {
            take()
        }
        index = sql.index(after: index)
    }
    let tail = sql[start...].trimmingCharacters(in: .whitespacesAndNewlines)
    if !tail.isEmpty { found.append((start..<sql.endIndex, tail)) }
    guard !found.isEmpty else { return nil }

    let caretIndex = String.Index(utf16Offset: min(max(caret, 0), sql.utf16.count), in: sql)
    if let (_, text) = found.first(where: { $0.0.contains(caretIndex) }) { return text }
    // Caret in the whitespace after a statement: that statement is the one being looked at.
    return found.last(where: { $0.0.lowerBound <= caretIndex })?.1 ?? found.first?.1
}
