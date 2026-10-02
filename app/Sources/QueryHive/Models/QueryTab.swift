import AppKit
import QueryHiveFFI
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
    /// Stop reached the engine (`done.cancelled`): these are the rows that arrived first, possibly none.
    var stopped = false

    /// What the grid's footer says it is showing. It must never imply the grid holds everything.
    var summary: String {
        if stopped {
            return rows.isEmpty ? "Stopped before any rows arrived" : "Stopped · \(pluralized(rows.count, "row"))"
        }
        return truncated
            ? "First \(rows.count.formatted()) rows · limit reached"
            : pluralized(rows.count, "row")
    }
}

/// The "off" rule before the result store lands: a base result is kept only
/// when holding it beside the active one does not double a large buffer.
enum BaseResultCache {
    static let limit = 10_000
    static func shouldStore(rowCount: Int) -> Bool { rowCount <= limit }
}

/// Which panel is showing under the SQL editor.
///
/// No longer includes Columns: the grid's own header carries every column name and its type chip,
/// so a separate list of them was the same information twice.
enum PanelTab: String, CaseIterable, Identifiable {
    case result, log, files, history, saved

    var id: Self { self }

    /// The third panel shows files for a file run and the written table for a table run; calling
    /// it "Files" while a CTAS is running would just be wrong.
    func label(for destination: Destination) -> String {
        switch self {
        case .result: "Result"
        case .log: "Log"
        case .files: destination == .table ? "Table" : "Files"
        case .history: "History"
        case .saved: "Saved"
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

    /// The tab's identity, and the same one the session store keeps so a relaunch can put the
    /// front tab back in front. Passed in rather than always fresh for that reason: a restored
    /// tab has to come back under the identity `active_tab_id` names.
    let id: UUID
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

    /// The `:name` values this tab last ran with, kept **in memory only**.
    ///
    /// Per tab, because a name means different things in different statements: `:id` is a user in
    /// one and an order in the next, and a value remembered across them is a wrong answer waiting.
    /// Not written to the session either: tabs come back at launch, so a stored value would be a
    /// stale value plus a run that never asked.
    var parameterValues: [String: ParameterEntry] = [:]

    /// Where the caret is, in UTF-16 units, as `NSTextView` reports it. Published by the editor
    /// so "Run Current Statement" knows which one the user is looking at.
    var caret = 0

    var previewing = false
    /// The rows the grid is drawing. Assigning it means a new set of rows exists, so the order the
    /// user asked for — which was an order over the old set — is dropped here rather than in
    /// `AppModel.preview`, because Explain fills the same grid from a run of its own and has to be
    /// covered too.
    var preview: PreviewResult? {
        didSet {
            if activeSort?.origin == .memory { activeSort = nil }
            // A layout describes the column set it was built from: hiding "nama" in one result must
            // not hide whatever column 1 is in the next one. Only a change in the number of columns
            // resets it, so a repaint of the same result — a streaming run paints several times —
            // keeps the user's hidden columns and renames.
            if columnLayout.sourceCount != (preview?.columns.count ?? 0) {
                columnLayout = GridColumnLayout(count: preview?.columns.count ?? 0)
                cellSelection = nil
            }
            clearEditUndo()
            gridRevision += 1
        }
    }
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

    /// The statement the grid's rows actually came from, before any search escalation wrapped it.
    ///
    /// A second escalation must wrap the user's own statement, not the first wrapper: nesting
    /// `queryhive_search` inside itself would still run, but the scan would grow a layer per click.
    /// Set with `previewedSQL`, and equal to it for an ordinary Run.
    var previewBaseSQL: String?

    /// How many rows the statement really returns, once the user has asked. `nil` means "not
    /// asked", which is different from "fewer than the limit".
    var totalRows: Int?
    var countingRows = false
    var countError: String?

    /// Filters by column index: a result may repeat a name and the grid draws by position.
    /// Cleared whenever new rows arrive, because the ones they described are gone.
    ///
    /// A filter change also drops the cell selection and the queued edits: all three are indices
    /// into the rows on screen, so hiding a row moves every index below it and a block — or an edit
    /// — left over from before the filter would point at rows the user never meant.
    var columnFilters: [Int: ColumnFilter] = [:] {
        didSet {
            cellSelection = nil
            cellEdits.discard()
            clearEditUndo()
            if activeSort?.origin == .memory { activeSort = nil }
            gridRevision += 1
        }
    }

    /// A cross-column search over the rows already fetched, and the in-memory counterpart of the
    /// server escalation. Empty means no search. Set through the view's field; like a filter it
    /// narrows the rows on screen and never touches the statement, which is why the grid has to say
    /// so and offers the escalate button beside it.
    ///
    /// A search is a claim about a specific set of rows, so it drops the selection, the queued edits
    /// and the sort, exactly as a filter does.
    var gridSearch = "" {
        didSet {
            guard gridSearch != oldValue else { return }
            cellSelection = nil
            cellEdits.discard()
            clearEditUndo()
            if activeSort?.origin == .memory { activeSort = nil }
            gridRevision += 1
        }
    }

    var hasGridSearch: Bool { !gridSearch.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    /// Whether the rows on screen are the server's answer to the field's term.
    var isServerSearched: Bool {
        guard let server = serverSearch else { return false }
        return server == gridSearch.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// The in-memory order over the fetched rows, or nil when the server owns
    /// the order (or there is none): a server order is never re-applied here.
    private var memorySort: GridSort? {
        guard let sort = activeSort, sort.origin == .memory else { return nil }
        return GridSort(column: sort.column, direction: sort.direction)
    }

    /// The term the grid filters in memory, or nil when the server already
    /// applied the same term: filtering twice would only risk diverging.
    private var effectiveLocalSearch: String? {
        let term = gridSearch.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !term.isEmpty else { return nil }
        if let server = serverSearch, server == term { return nil }
        return term
    }

    /// The grid's column presentation: order, visibility and labels. Rendering only — see
    /// `GridColumnLayout`. Mutated through the methods below so the sort is reconciled against it.
    private(set) var columnLayout = GridColumnLayout(count: 0)

    /// The source indices the grid draws, in display order. A convenience over `columnLayout` for
    /// the view, which needs it on nearly every render pass.
    var visibleColumnSources: [Int] { columnLayout.visible }

    /// Saved filter sets for a tab with no table to file them against. A hand-written query's
    /// identity would change with every run, so a preset made there lives with the tab and is never
    /// written to disk. `FilterPresetStore`'s own note carries the decision.
    var localPresets: [FilterPreset] = []

    /// The order the grid is drawing in, or nil for the server's own order.
    /// The single source of truth: one value, so two indicators cannot differ.
    var activeSort: ActiveSort?

    /// The term the server searched for, or nil when the rows are unsearched.
    /// While it equals the field's term the server owns the search, so the
    /// grid does not filter the same rows a second time in memory.
    var serverSearch: String?

    /// The base result kept for "off": restored without a query when small.
    var baseResult: PreviewResult?

    /// Sort the fetched rows in memory: the fallback when the server route
    /// does not apply. Drops the positional state the new order invalidates.
    func applyMemorySort(_ sort: GridSort?) {
        activeSort = sort.map { ActiveSort(column: $0.column, direction: $0.direction, origin: .memory) }
        cellSelection = nil
        cellEdits.discard()
        clearEditUndo()
        gridRevision += 1
    }

    /// Mark the rows server-ordered: set before the re-run, kept on failure
    /// so a server error is shown, never silently replaced by a fallback.
    func applyServerSort(column: Int, direction: GridSort.Direction) {
        activeSort = ActiveSort(column: column, direction: direction, origin: .server)
        cellSelection = nil
        cellEdits.discard()
        clearEditUndo()
        gridRevision += 1
    }

    // MARK: Columns (rendering only)

    /// Hide a column, or show it again.
    ///
    /// Hiding is not deleting: the column is still fetched, still copied and still exported, and the
    /// queue and the filters are untouched. What it does change is what is on screen, so the block
    /// selection goes — its columns are display positions, and hiding one shifts every position
    /// after it. The sort is reconciled separately, in `reconcileColumns`.
    func setColumnHidden(_ source: Int, _ hidden: Bool) {
        guard source >= 0, source < columnLayout.sourceCount else { return }
        if hidden { columnLayout.hide(source) } else { columnLayout.show(source) }
        cellSelection = nil
        reconcileColumns()
        gridRevision += 1
    }

    func toggleColumn(_ source: Int) {
        setColumnHidden(source, !columnLayout.isVisible(source))
    }

    /// Move the drawn column at `from` to display position `to`. Both are display positions among
    /// the visible columns. This reorders only what is drawn.
    ///
    /// The sort and the filters are keyed by source index, so they follow a moved column on their
    /// own; only the block selection, which is a rectangle of display positions, has to go.
    func moveColumn(from: Int, to: Int) {
        columnLayout.move(from: from, to: to)
        cellSelection = nil
        gridRevision += 1
    }

    /// Rename a column for display. A blank name reverts to the server's. The data, the copy and
    /// the export never see the name, so there is nothing to reconcile here.
    func renameColumn(_ source: Int, to name: String) {
        columnLayout.rename(source, to: name)
        gridRevision += 1
    }

    func showAllColumns() {
        columnLayout.showAll()
        cellSelection = nil
        reconcileColumns()
        gridRevision += 1
    }

    func resetColumnLayout() {
        columnLayout.reset()
        cellSelection = nil
        reconcileColumns()
        gridRevision += 1
    }

    /// The reconciliation the source-index identity cannot do on its own: an order whose column is
    /// no longer drawn has no chevron to say so, so the sort goes with the column. A move or a
    /// rename leaves it alone, because the column it names has not changed.
    private func reconcileColumns() {
        if let sort = activeSort, !columnLayout.isVisible(sort.column) {
            activeSort = nil
            gridRevision += 1
        }
    }

    // MARK: Filter presets

    /// The filters currently set, as a preset keyed by column name, or `nil` when nothing is
    /// filtered. Needs the columns, because a filter is keyed by position and a preset is not.
    func currentFilterPreset(named name: String) -> FilterPreset? {
        FilterPreset.from(name: name, filters: columnFilters, columns: preview?.columns ?? [])
    }

    /// Apply a saved filter set to the rows on screen, and report the column names it carries that
    /// the current result does not have. Setting `columnFilters` drops the selection, the edits and
    /// the sort in its own `didSet`, which is the same treatment a hand-set filter gets.
    @discardableResult
    func applyFilterPreset(_ preset: FilterPreset) -> [String] {
        guard let preview else { return preset.filters.map(\.column) }
        let (filters, missing) = preset.resolve(columns: preview.columns)
        columnFilters = filters
        return missing
    }

    // MARK: Queued-edit undo

    /// The undo history for the grid's queued edits. One editing session becomes one step, so a
    /// typed word undoes as a word rather than a character at a time; see `beginCellEdit`.
    @ObservationIgnored let editUndoManager = UndoManager()

    /// The cell an editor is over and the text typed into it, held here — not in the queue — until
    /// the session ends. Replacing this buffer is the whole cost of a keystroke.
    @ObservationIgnored private var editSession: CellEditSession?

    /// One editing session: the cell, the value it started from, and the text so far.
    private struct CellEditSession {
        let key: CellKey
        /// The value the cell was fetched with. The queue needs this one, not the staged value, to
        /// decide whether the cell has been typed all the way back to where it started.
        let original: String?
        var text: String
    }

    /// Open an editing session over a cell, seeded with what it currently shows.
    func beginCellEdit(at key: CellKey) {
        let seed = cellValue(at: key) ?? ""
        editSession = CellEditSession(key: key, original: fetchedValue(at: key), text: seed)
    }

    /// A keystroke. The buffer is replaced; nothing reaches the queue and nothing is registered with
    /// undo until the session ends. That is the whole point: typing "hello" is one undo, not five.
    func typeCellEdit(_ text: String) {
        guard var session = editSession else { return }
        session.text = text
        editSession = session
    }

    /// The editor was dismissed without committing.
    func cancelCellEdit() { editSession = nil }

    /// End the session and stage its buffer as one queue change and one undo step.
    ///
    /// A buffer typed back to the value the cell was fetched with stages nothing, and because the
    /// queue is unchanged between the two snapshots, no undo step is registered either — an undo
    /// that visibly does nothing is worse than no undo.
    func endCellEdit() {
        guard let session = editSession else { return }
        editSession = nil
        let before = cellEdits
        cellEdits.edit(session.text, at: session.key, original: session.original)
        registerGridEdit(before)
    }

    /// Stage the editor's text over a whole selected block as one undo step.
    func fillCellEdits(_ text: String, over selection: CellRange) {
        let before = cellEdits
        cellEdits.fill(text, over: selection, rows: displayedRows, columns: visibleColumnSources)
        registerGridEdit(before)
    }

    /// Stage a pasted block as one undo step.
    func pasteCellEdits(_ text: String, at origin: CellKey, columnCount: Int) {
        let before = cellEdits
        cellEdits.paste(text, at: origin, rows: displayedRows, columnCount: columnCount,
                        columns: visibleColumnSources)
        registerGridEdit(before)
    }

    /// Empty the queue as one undo step.
    func discardCellEdits() {
        let before = cellEdits
        cellEdits.discard()
        registerGridEdit(before)
    }

    func undoCellEdit() { editUndoManager.undo() }
    func redoCellEdit() { editUndoManager.redo() }
    var canUndoCellEdit: Bool { editUndoManager.canUndo }
    var canRedoCellEdit: Bool { editUndoManager.canRedo }

    /// What one cell shows: its staged text when it has one, the fetched value otherwise.
    func cellValue(at key: CellKey) -> String? {
        if let staged = cellEdits.value(at: key) { return staged }
        return fetchedValue(at: key)
    }

    /// The value the server sent for a cell, before any staged edit.
    func fetchedValue(at key: CellKey) -> String? {
        guard displayedRows.indices.contains(key.row) else { return nil }
        let row = displayedRows[key.row]
        return row.indices.contains(key.column) ? row[key.column] : nil
    }

    /// Throw the queued-edit history away, with the queue it describes.
    ///
    /// Called wherever the grid invalidates the queue itself — a filter, a search, a sort, a new
    /// result — because an undo step in that history names rows and columns on a screen that no
    /// longer exists. Restoring such a step would put edits back onto cells the user never touched.
    private func clearEditUndo() { editUndoManager.removeAllActions() }

    /// Record a queue change as one undo step, with a matching redo. No change means no step.
    ///
    /// Each call is its own group rather than left to the run loop: the session that ends here is
    /// already one user action, and grouping by event would merge two quick actions — a paste and
    /// the next edit — into a single undo.
    private func registerGridEdit(_ before: CellEdits) {
        guard before != cellEdits else { return }
        editUndoManager.beginUndoGrouping()
        editUndoManager.registerUndo(withTarget: self) { tab in
            let after = tab.cellEdits
            tab.cellEdits = before
            tab.editUndoManager.registerUndo(withTarget: tab) { redo in redo.cellEdits = after }
        }
        editUndoManager.setActionName("Edit Cell")
        editUndoManager.endUndoGrouping()
    }

    /// A counter that changes whenever the rows, the filters, the search or the sort change.
    ///
    /// It exists so `displayedRows` can be cached: the grid reads that value several times per
    /// render, and filtering and sorting a large result on each read is work the user pays for on
    /// every hover and selection. Bumped in the places that can change what is on screen —
    /// `preview`, `columnFilters`, `gridSearch` and the sort setters.
    private(set) var gridRevision = 0

    /// The cached answer, and the revision it was computed for.
    @ObservationIgnored private var displayedCache: [[String?]]?
    @ObservationIgnored private var displayedCacheRevision = -1

    /// The rows the grid draws: the fetched rows with the filters and the sort applied.
    ///
    /// Pure in the rows, the filters and the sort — all three revision-stamped — so the answer
    /// cannot change without a bump, which is what makes the cache safe rather than merely fast.
    var displayedRows: [[String?]] {
        if displayedCacheRevision == gridRevision, let displayedCache { return displayedCache }
        let rows: [[String?]]
        if let preview {
            var filtered = columnFilters.isEmpty
                ? preview.rows
                : preview.rows.filter { row in
                    columnFilters.allSatisfy { index, filter in
                        filter.matches(index < row.count ? row[index] : nil)
                    }
                }
            // The cross-column search narrows the same set the filters do, and before the sort for
            // the same reason: the order is over what survives.
            if let term = effectiveLocalSearch {
                filtered = filtered.filter { GridSearch.matches($0, term: term) }
            }
            rows = memorySort.map { $0.order(filtered) } ?? filtered
        } else {
            rows = []
        }
        displayedCache = rows
        displayedCacheRevision = gridRevision
        return rows
    }

    /// The block of cells the pointer has dragged out in the grid, if any. Rows are positions in the
    /// rows the grid is drawing (the filtered ones) and columns are positions among the **drawn**
    /// columns; both are what the pointer pointed at. Cleared whenever new rows arrive, for the same
    /// reason the filters are: the numbers describe rows that no longer exist. A queued edit, by
    /// contrast, is keyed by the source column, because it has to survive a column being moved.
    var cellSelection: CellRange?

    /// The cells the user has changed but not yet written.
    ///
    /// The same positional hazard as the selection, and cleared in the same places: a queued edit
    /// names a row and a column on screen, so a new run or a filter has to drop it rather than let
    /// it point at a row it was never about.
    var cellEdits = CellEdits()

    /// The table a queued edit would be written to, when the app genuinely knows it.
    ///
    /// Set only by the path that wrote the SQL itself — opening a table from the tree, which runs
    /// exactly `SELECT * FROM <name>` (`AppModel.openTable`). A hand-written query leaves this nil
    /// and the grid cannot offer to commit: writing to a table the app guessed at from a `FROM`
    /// clause is worse than not writing at all.
    var sourceTable: String?

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

    init(title: String, id: UUID = UUID()) {
        self.id = id
        self.title = title
        outputDirectory = ConnectionStore.defaultOutputDirectory
        // Each registered edit is already one user action, so the manager must not merge two quick
        // actions into one collapse. `registerGridEdit` opens and closes a group per call.
        editUndoManager.groupsByEvent = false
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

/// Every statement in a script, in one pass, with the range each one occupies.
///
/// The scanner "Run Current Statement" and the editor's folding both need statement boundaries, and
/// this is the one place they come from: the engine's own `scan.rs`, through the commit-A FFI
/// `sql_statement_ranges`. One splitter for the band/run marks and for Run, so `select "a;b"` is
/// one statement in both places instead of Run sending `select "a`.
///
/// A splitter, not a parser: pieces hold more than whitespace, comments and `;`
/// (`statements_with_lines`), so a comment-only stretch is no statement at all. The dialect is
/// `.generic`: this free function sees no tab and therefore no connection to read one from, and
/// per-tab dialect wiring belongs to W10-T6, which already owns the `EditorDocument` dialect.
///
/// The range starts where the previous `;` left off, so it can carry the whitespace between two
/// statements; the text is trimmed. A caller that needs the statement's own first line steps over
/// that whitespace itself (folding does).
///
/// If the FFI refuses the text (past the editor ceiling), the whole script comes back as one
/// piece rather than nothing, so Run still sends the user's SQL and the engine splits it itself.
func sqlStatements(in sql: String) -> [(range: Range<String.Index>, text: String)] {
    let flat: [UInt32]
    do {
        flat = try sqlStatementRanges(sql: sql, dialect: .generic)
    } catch {
        let text = sql.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return [] }
        return [(sql.startIndex..<sql.endIndex, text)]
    }
    // The pairs arrive ascending, so one forward walk maps every UTF-16 offset.
    var utf16Cursor = sql.utf16.startIndex
    var base = 0

    func stringIndex(atUTF16Offset target: Int) -> String.Index? {
        guard target >= base,
              let next = sql.utf16.index(utf16Cursor, offsetBy: target - base,
                                         limitedBy: sql.utf16.endIndex),
              let mapped = String.Index(next, within: sql) else { return nil }
        utf16Cursor = next
        base = target
        return mapped
    }
    var found: [(Range<String.Index>, String)] = []
    found.reserveCapacity(flat.count / 2)
    var pair = 0
    while pair + 1 < flat.count {
        let startOffset = Int(flat[pair])
        let endOffset = Int(flat[pair + 1])
        pair += 2
        guard endOffset >= startOffset,
              let lower = stringIndex(atUTF16Offset: startOffset),
              let upper = stringIndex(atUTF16Offset: endOffset),
              lower <= upper else { continue }
        let text = String(sql[lower..<upper]).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { continue }
        found.append((lower..<upper, text))
    }
    return found
}

/// The statement the caret sits in.
func sqlStatement(in sql: String, atUTF16Offset caret: Int) -> String? {
    let found = sqlStatements(in: sql)
    guard !found.isEmpty else { return nil }

    let caretIndex = String.Index(utf16Offset: min(max(caret, 0), sql.utf16.count), in: sql)
    if let (_, text) = found.first(where: { $0.0.contains(caretIndex) }) { return text }
    // Caret in the whitespace after a statement: that statement is the one being looked at.
    return found.last(where: { $0.0.lowerBound <= caretIndex })?.1 ?? found.first?.1
}
