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

    /// The picker's stand-in for SQL NULL.
    static let nullToken = "\u{0}null"
}

/// What a Run produced, apart from the rows: those live in the Rust store (`StoreRows`).
struct PreviewResult {
    var columns: [Event.Column]
    /// Rows the run fetched; written by `done`, and 0 while the result is still arriving.
    var rowCount: Int
    /// The row limit stopped it short, so what is on screen is not the whole result.
    var truncated: Bool
    var queryID: String?
    var elapsedMS: Int
    /// Stop reached the engine (`done.cancelled`): these are the rows that arrived first, possibly none.
    var stopped = false

    /// What the grid's footer says it is showing. It must never imply the grid holds everything.
    var summary: String {
        if stopped {
            return rowCount == 0 ? "Stopped before any rows arrived" : "Stopped · \(pluralized(rowCount, "row"))"
        }
        return truncated
            ? "First \(rowCount.formatted()) rows · limit reached"
            : pluralized(rowCount, "row")
    }
}

/// A finished result and the store that holds its rows: what "off" returns to (blueprint §18).
struct ResultSlot {
    var meta: PreviewResult
    var rows: StoreRows
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

    var sql = "" {
        didSet {
            // The mark says where the server found fault in a text that no longer exists.
            if let mark = errorMark, sql != mark.sqlSnapshot { errorMark = nil }
        }
    }
    var connectionID: UUID?

    /// The dialect the editor reads this tab's SQL under: its connection's, or `.generic` without
    /// one. `EditorPane` keeps it current, and statement boundaries for Run, Export and History are
    /// taken under it, so what the editor draws as one statement is what the engine is sent.
    var dialect: EditorDialect = .generic

    /// Where the server said the last Run went wrong, while the document is still the text that Run
    /// sent (blueprint w10 §8.2). Set only by a Run whose text is a piece of this document.
    var errorMark: ServerErrorMark?

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
            // not hide whatever column 1 is in the next one. A change in the number of columns
            // resets it, and so does a change in any column's name or type (PF-11): two four-column
            // results are not the same columns. A repaint of the same result — a streaming run paints
            // several times — keeps the user's hidden columns and renames.
            let columns = preview?.columns ?? []
            let signature = columns.map { "\($0.name)\u{1F}\($0.type)" }
            let sameColumns = signature == columnSignature
            if columnLayout.sourceCount != columns.count || (!columns.isEmpty && !sameColumns) {
                columnLayout = GridColumnLayout(count: columns.count)
                cellSelection = nil
            }
            // The widths follow the column set, not the run: a sort re-run passes through `nil` and
            // back with the same columns, and the user's widths ride across it.
            if !columns.isEmpty {
                if !sameColumns { columnWidthOverrides = [:] }
                columnSignature = signature
            }
            clearEditUndo()
            gridRevision += 1
        }
    }
    var previewError: String?

    /// Column widths the user has set by hand or by Fit, keyed by source column, in drawn points
    /// (DBX-64). Session only: it lives with the tab and is never written to the session file.
    /// Dropped when the column set changes, by `preview`'s `didSet`, and by Reset Column Layout.
    @ObservationIgnored var columnWidthOverrides: [Int: CGFloat] = [:]
    /// Name and type of every column of the last result that had any, which is what tells a new
    /// column set from a repaint of the old one.
    @ObservationIgnored private var columnSignature: [String] = []

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
            scheduleViewApply()
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
            scheduleViewApply()
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
    var memorySort: GridSort? {
        guard let sort = activeSort, sort.origin == .memory else { return nil }
        return GridSort(column: sort.column, direction: sort.direction)
    }

    /// The term the grid filters in memory, or nil when the server already
    /// applied the same term: filtering twice would only risk diverging.
    var effectiveLocalSearch: String? {
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

    /// The base result kept for "off": the rows in server order, held by their own store, which
    /// spills before it doubles memory (§18). `nil` for a restored tab, a stopped Run or a failed one.
    var baseResult: ResultSlot?

    /// Sort the fetched rows in memory: the fallback when the server route
    /// does not apply. Drops the positional state the new order invalidates.
    func applyMemorySort(_ sort: GridSort?) {
        activeSort = sort.map { ActiveSort(column: $0.column, direction: $0.direction, origin: .memory) }
        cellSelection = nil
        cellEdits.discard()
        clearEditUndo()
        scheduleViewApply()
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
        columnWidthOverrides = [:]
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
    @ObservationIgnored private var editSession: CellEditSession? {
        // Mirrored into an observed flag only when it flips, never per keystroke.
        didSet { if (editSession != nil) != hasOpenCellEdit { hasOpenCellEdit = editSession != nil } }
    }

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
        // A session left open by a grid that has since gone (a panel switch) is committed to its own
        // cell, not overwritten: its text is the user's, and the new editor would stage it on the
        // wrong cell.
        endCellEdit()
        guard !refusedWhileBusy() else { return }
        if cellEdits.isDeleted(key.row) {
            note(.warning, "This row is marked for deletion. Restore it to edit it.")
            return
        }
        let seed = cellValue(at: key) ?? ""
        editSession = CellEditSession(key: key, original: fetchedValue(at: key), text: seed)
    }

    /// Whether a session is open over a cell. Observed, so the grid drops its editor overlay when a
    /// session is ended from outside (Save, Review, Apply) and the menu sees text that is typed but
    /// not yet staged. Also the test hook for the leak a fill used to leave behind.
    private(set) var hasOpenCellEdit = false

    /// The cell the open session is over, so the editor overlay is only shown when `beginCellEdit`
    /// really took the session (it refuses a deleted row, an apply in flight and stale rows).
    var editingCellKey: CellKey? { editSession?.key }

    /// A keystroke. The buffer is replaced; nothing reaches the queue and nothing is registered with
    /// undo until the session ends. That is the whole point: typing "hello" is one undo, not five.
    func typeCellEdit(_ text: String) {
        guard !viewBusy, var session = editSession else { return }
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
    ///
    /// Only the view being replaced refuses it (D-27), not an apply in flight: the text was typed
    /// before the apply began, and dropping it would lose what the person wrote. It lands in the
    /// queue beside the plan being run, and the apply's success keeps it (`CellEdits.removing`, PF-2).
    func endCellEdit() {
        guard let session = editSession else { return }
        editSession = nil
        guard !viewBusyRefusal() else { return }
        let before = cellEdits
        if session.key.row < 0 {
            // An added row's cell: empty text takes the value out (the column is then left out of
            // the INSERT), and `DEFAULT` is the keyword.
            cellEdits.setInserted(session.text, row: session.key.row, column: session.key.column)
        } else {
            cellEdits.edit(session.text, at: session.key, original: session.original)
        }
        registerGridEdit(before)
    }

    /// Stage the editor's text over a whole selected block as one undo step.
    ///
    /// The selection is in table rows, so the block is cut at the border (`GridRowSpace.split`): the
    /// fetched rows stage UPDATE edits and the added rows take the text as their own values. Nothing
    /// is ever keyed to a table row that is past the fetched ones.
    func fillCellEdits(_ text: String, over selection: CellRange) {
        guard !refusedWhileBusy(), !exceedsCellCeiling(selection.cellCount) else { return }
        let before = cellEdits
        let parts = rowSpace.split(selection.top...selection.bottom)
        if let fetched = parts.fetched {
            cellEdits.fill(text, over: CellRange(from: (fetched.lowerBound, selection.left),
                                                 to: (fetched.upperBound, selection.right)),
                           rows: result, columns: visibleColumnSources)
        }
        for (_, id) in parts.inserted {
            for position in selection.left...selection.right {
                guard let source = columnLayout.source(at: position) else { continue }
                cellEdits.setInserted(text, row: id, column: source)
            }
        }
        registerGridEdit(before)
    }

    /// Stage a pasted block as one undo step. `origin` is a table row and a display column; a line
    /// that lands past the last row is dropped, since a paste adds no rows.
    func pasteCellEdits(_ text: String, at origin: CellKey, columnCount: Int) {
        guard !refusedWhileBusy() else { return }
        let lines = CellEdits.parse(text)
        guard !exceedsCellCeiling(lines.reduce(0) { $0 + $1.count }) else { return }
        let before = cellEdits
        let space = rowSpace
        let fetchedLines = origin.row < space.fetched ? min(lines.count, space.fetched - origin.row) : 0
        cellEdits.paste(lines: Array(lines.prefix(fetchedLines)), at: origin, rows: result,
                        columnCount: columnCount, columns: visibleColumnSources)
        for (down, line) in lines.enumerated().dropFirst(fetchedLines) {
            guard case .inserted(let id, _) = space.kind(ofTableRow: origin.row + down) else { continue }
            for (across, field) in line.enumerated() where origin.column + across < columnCount {
                guard let source = columnLayout.source(at: origin.column + across) else { continue }
                cellEdits.setInserted(field, row: id, column: source)
            }
        }
        registerGridEdit(before)
    }

    static let viewBusyMessage = "The grid is updating its rows"
    static let applyingMessage = "The changes are being applied"
    static let staleAfterApplyMessage = "These rows changed on the server. Run the query again before editing them"

    /// Edits name rows of the view on screen, and a view is on its way out (D-27): refuse, and say so.
    private func viewBusyRefusal() -> Bool {
        guard viewBusy else { return false }
        note(.warning, Self.viewBusyMessage)
        return true
    }

    /// Everything that stops a new edit: a view being replaced, an apply in flight (PF-2: one apply
    /// per tab, and nothing new is staged under it), or rows the last apply left behind.
    private func refusedWhileBusy() -> Bool {
        if viewBusyRefusal() { return true }
        if applying { note(.warning, Self.applyingMessage + "…"); return true }
        if rowsStaleAfterApply { note(.warning, Self.staleAfterApplyMessage + "."); return true }
        return false
    }

    /// The most cells one fill, paste or delete may stage (PF-12). Every undo step keeps the queue it
    /// came from, so an unbounded block would multiply into a hundred copies of itself.
    static let editCellCeiling = 50_000
    /// How many steps the undo history keeps (PF-12).
    static let undoLevels = 100

    func exceedsCellCeiling(_ cells: Int) -> Bool {
        guard cells > Self.editCellCeiling else { return false }
        note(.warning, "That is \(cells.formatted()) cells, and the grid stages at most "
             + "\(Self.editCellCeiling.formatted()) at a time. Select fewer, or change them with a statement.")
        return true
    }

    /// An apply is running for this tab (PF-2): a second one, ⌘S, the review sheet and a new edit are
    /// all refused until it ends.
    var applying = false

    /// The last apply wrote rows the grid still shows as they were, and the query could not be run
    /// again to show them (DBX-27's fallback). Edits are refused until a new result replaces them,
    /// because a plan built from the old values would match nothing.
    var rowsStaleAfterApply = false

    /// Raised by ⌘S: the grid opens its review sheet and puts this down. A flag rather than a
    /// counter, so a grid that is not on screen yet (the panel shows the log, or is collapsed) still
    /// finds the request when it appears.
    var reviewRequested = false

    /// Empty the queue as one undo step.
    func discardCellEdits() {
        let before = cellEdits
        cellEdits.discard()
        registerGridEdit(before, actionName: "Discard Changes")
    }

    /// Set the selection and cursor from the coordinator.
    func selectCells(anchor: CellPos, focus: CellPos) {
        cellSelection = CellRange(from: (anchor.row, anchor.column), to: (focus.row, focus.column))
        cellCursor = GridCursor(anchor: anchor, focus: focus)
    }

    func undoCellEdit() { editUndoManager.undo() }
    func redoCellEdit() { editUndoManager.redo() }
    var canUndoCellEdit: Bool { editUndoManager.canUndo }
    var canRedoCellEdit: Bool { editUndoManager.canRedo }

    /// What one cell shows: its staged text when it has one, the fetched value otherwise.
    func cellValue(at key: CellKey) -> String? {
        // An added row has no fetched value: what it holds is what the user typed into it.
        if key.row < 0 { return cellEdits.insertedValue(row: key.row, column: key.column) }
        if let staged = cellEdits.value(at: key) { return staged }
        return fetchedValue(at: key)
    }

    /// The value the server sent for a cell, before any staged edit. Never reads the store for an
    /// added row, whose key is a negative id and not a row of the result.
    func fetchedValue(at key: CellKey) -> String? {
        guard key.row >= 0 else { return nil }
        return result.fullValue(row: key.row, column: key.column, format: .raw)
    }

    /// The rows the table draws: the result's, then the added ones.
    var rowSpace: GridRowSpace { GridRowSpace(fetched: result.count, inserted: cellEdits.inserted) }

    /// Keep the selection and the cursor inside the rows that are there. Every change of
    /// `cellEdits` that moves `inserted.count` calls this, because a table row past the fetched ones
    /// is an added row and deleting an earlier one shifts the rest (blueprint w10 §5.1).
    func reconcileSelectionWithRowSpace() {
        let space = rowSpace
        guard space.count > 0 else {
            if cellSelection != nil { cellSelection = nil }
            cellCursor = nil
            return
        }
        guard cellSelection != nil || cellCursor != nil else { return }
        func clamp(_ position: CellPos) -> CellPos {
            CellPos(row: min(max(position.row, 0), space.count - 1), column: position.column)
        }
        let selection = cellSelection
        let anchor = clamp(cellCursor?.anchor ?? CellPos(row: selection?.top ?? 0, column: selection?.left ?? 0))
        let focus = clamp(cellCursor?.focus ?? CellPos(row: selection?.bottom ?? 0, column: selection?.right ?? 0))
        let inside = selection.flatMap { space.clamped($0) }
        guard cellCursor?.anchor != anchor || cellCursor?.focus != focus || selection != inside else { return }
        selectCells(anchor: anchor, focus: focus)
    }

    /// Throw the queued-edit history away, with the queue it describes.
    ///
    /// Called wherever the grid invalidates the queue itself — a filter, a search, a sort, a new
    /// result — because an undo step in that history names rows and columns on a screen that no
    /// longer exists. Restoring such a step would put edits back onto cells the user never touched.
    func clearEditUndo() { editUndoManager.removeAllActions() }

    /// Record a queue change as one undo step, with a matching redo. No change means no step.
    ///
    /// Each call is its own group rather than left to the run loop: the session that ends here is
    /// already one user action, and grouping by event would merge two quick actions — a paste and
    /// the next edit — into a single undo.
    ///
    /// `actionName` is what Edit > Undo says ("Undo Add Row"). An undo and a redo both put a queue
    /// back, so both reconcile the selection with the rows that queue draws.
    func registerGridEdit(_ before: CellEdits, actionName: String = "Edit Cell") {
        guard before != cellEdits else { return }
        editUndoManager.beginUndoGrouping()
        editUndoManager.registerUndo(withTarget: self) { tab in
            let after = tab.cellEdits
            tab.cellEdits = before
            tab.reconcileSelectionWithRowSpace()
            tab.editUndoManager.registerUndo(withTarget: tab) { redo in
                redo.cellEdits = after
                redo.reconcileSelectionWithRowSpace()
            }
            tab.editUndoManager.setActionName(actionName)
        }
        editUndoManager.setActionName(actionName)
        editUndoManager.endUndoGrouping()
        reconcileSelectionWithRowSpace()
    }

    /// A counter that changes whenever the rows the grid draws are a different set: a new result,
    /// a view that has just been installed, a layout change. The grid redraws from it. Growth of a
    /// streaming result does not bump it (`StoreRows.poll` and `rowsDidGrow` carry that, D-28).
    private(set) var gridRevision = 0

    /// The rows the grid draws. Written when a Run starts, and released with the tab (§19).
    var activeResult: StoreRows? {
        // A different store means an apply still in flight is for rows that are gone: its hop must
        // neither land nor be waited for, so the guard goes down with the store (D-27).
        didSet {
            guard oldValue !== activeResult else { return }
            viewGeneration += 1
            viewBusy = false
            rowsStaleAfterApply = false
        }
    }

    /// Rows fetched so far, at most five times a second while streaming (D-28): the one number
    /// SwiftUI watches, because the store itself is not observable.
    var fetchedRows = 0

    /// A view is being applied to the store: edits, fills, pastes and write plans are refused until
    /// it lands, because their row indices belong to the view being replaced (D-27).
    private(set) var viewBusy = false
    /// Which `scheduleViewApply` call is the latest. Only its hop may clear `viewBusy`.
    @ObservationIgnored private(set) var viewGeneration = 0
    /// Why the last view could not be applied, for the grid's banner.
    var viewError: String?

    @ObservationIgnored private static let noRows = EmptyRows()

    /// What the grid draws: the store, or an empty stand-in before any Run.
    var result: any ResultRows { activeResult ?? Self.noRows }

    /// Ask the store for the view the filters, search and in-memory sort describe. The grid keeps
    /// showing the old view until the new one is installed.
    func scheduleViewApply() {
        guard let store = activeResult else { gridRevision += 1; return }
        viewGeneration += 1
        let generation = viewGeneration
        viewBusy = true
        viewError = nil
        let spec = viewSpec
        Task { @MainActor [weak self] in
            // A newer ask (or `applyViewBlocking`) came before this one ran: it is the one to apply.
            guard self?.viewGeneration == generation else { return }
            var failure: StoreFfiError?
            do { _ = try await store.apply(spec) } catch let error as StoreFfiError {
                switch error {
                case .Superseded, .StaleHandle: self?.endViewApply(generation)
                    return
                case .StaleView: break
                default: failure = error
                }
            } catch { self?.endViewApply(generation); return }
            self?.viewApplied(on: store, generation: generation, failure: failure)
        }
    }

    /// Apply the view now, on this thread. For fixtures, snapshots and tests that set a filter, a
    /// search or a sort and need the rows to be those straight away, before they stage anything.
    func applyViewBlocking() {
        guard let store = activeResult else { gridRevision += 1; return }
        viewGeneration += 1
        do { try store.applyBlocking(viewSpec); viewError = nil } catch {
            viewError = error.localizedDescription
        }
        viewBusy = false
        gridRevision += 1
    }

    /// An apply that installed nothing: lower the guard if it was the latest ask.
    private func endViewApply(_ generation: Int) {
        if generation == viewGeneration { viewBusy = false }
    }

    /// The hop that follows an `apply`. Every completed apply moves the store's view, so the grid
    /// is told (its cached rows belong to the one before); only the latest one ends `viewBusy`.
    func viewApplied(on store: StoreRows, generation: Int, failure: StoreFfiError? = nil) {
        guard activeResult === store else { return }
        gridRevision += 1
        guard generation == viewGeneration else { return }
        viewBusy = false
        if let failure { viewError = failure.localizedDescription }
        // An edit that got in before `viewBusy` came on names a row of the view that has gone.
        cellSelection = nil
        if !cellEdits.isEmpty { cellEdits.discard() }
        clearEditUndo()
    }

    /// Installed by the grid's coordinator: asks it to poll the store now. The first `progress` of
    /// a run calls it, so the first rows do not wait for a display tick.
    @ObservationIgnored var pollHook: (() -> Void)?

    /// Let go of both stores and forget the metadata that described them (§19). Replaces the token,
    /// so events from a run that is still winding down are dropped by the guard that checks it.
    func releaseResults() {
        previewToken = UUID()
        let held = [activeResult, baseResult?.rows]
        activeResult = nil
        baseResult = nil
        viewBusy = false
        viewGeneration += 1
        for store in held { store?.release() }
    }

    /// A finished result from rows already in hand: fixtures, snapshots and benches. Builds the store
    /// through the engine and installs it the way a `done` would.
    func showRows(columns: [Event.Column], rows: [[String?]], truncated: Bool = false,
                  queryID: String? = nil, elapsedMS: Int = 0, stopped: Bool = false) {
        do {
            let store = try Engine.current.storeFromRows(columns: columns, rows: rows)
            activeResult?.release()
            activeResult = store
            fetchedRows = store.fetched
            preview = PreviewResult(columns: columns, rowCount: store.fetched, truncated: truncated,
                                    queryID: queryID, elapsedMS: elapsedMS, stopped: stopped)
        } catch {
            previewError = "Could not hold the rows: \(error.localizedDescription)"
        }
    }

    /// The block of cells the pointer has dragged out in the grid, if any. Rows are positions in the
    /// rows the grid is drawing (the filtered ones) and columns are positions among the **drawn**
    /// columns; both are what the pointer pointed at. Cleared whenever new rows arrive, for the same
    /// reason the filters are: the numbers describe rows that no longer exist. A queued edit, by
    /// contrast, is keyed by the source column, because it has to survive a column being moved.
    var cellSelection: CellRange? {
        didSet {
            guard let selection = cellSelection else { cellCursor = nil; return }
            // Keep the cursor inside the selection.
            if let cursor = cellCursor, !selection.contains(row: cursor.focus.row, column: cursor.focus.column) {
                cellCursor = GridCursor(anchor: CellPos(row: selection.top, column: selection.left), focus: CellPos(row: selection.top, column: selection.left))
            } else if cellCursor == nil {
                cellCursor = GridCursor(anchor: CellPos(row: selection.top, column: selection.left), focus: CellPos(row: selection.top, column: selection.left))
            }
        }
    }
    
    /// The cell cursor (P-24). W10-T1 makes this authoritative and draws the ring.
    var cellCursor: GridCursor?

    /// The panel beside the grid reads the cursor's whole row as fields instead of one cell
    /// (W10-T5). Per tab and not saved: it is a way of looking, not part of the session.
    var recordMode = false

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
    func sql(for source: QuerySource) -> String { sent(for: source).text }

    /// What `sql(for:)` returns, and where it starts in the document. The start is measured after the
    /// empty-selection fallback and after the trimming, because a server error position counts from
    /// the first character that was sent.
    func sent(for source: QuerySource) -> SentSQL {
        switch source {
        case .all:
            return SentSQL(text: sql, documentStart: 0)
        case .selection:
            let text = sql as NSString
            guard selection.length > 0, selection.location >= 0,
                  NSMaxRange(selection) <= text.length else { return SentSQL(text: sql, documentStart: 0) }
            return SentSQL(text: text.substring(with: selection), documentStart: selection.location)
        case .statement:
            guard let found = sqlStatementLocated(in: sql, atUTF16Offset: selection.location,
                                                  dialect: dialect) else {
                return SentSQL(text: sql, documentStart: 0)
            }
            return SentSQL(text: found.text, documentStart: found.start)
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
        editUndoManager.levelsOfUndo = Self.undoLevels
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
/// (`statements_with_lines`), so a comment-only stretch is no statement at all. The dialect is the
/// caller's (a tab's own, `QueryTab.dialect`); a caller with no connection to read one from leaves
/// it `.generic`.
///
/// The range starts where the previous `;` left off, so it can carry the whitespace between two
/// statements; the text is trimmed. A caller that needs the statement's own first line steps over
/// that whitespace itself (folding does).
///
/// If the FFI refuses the text (past the editor ceiling), the whole script comes back as one
/// piece rather than nothing, so Run still sends the user's SQL and the engine splits it itself.
func sqlStatements(in sql: String,
                   dialect: EditorDialect = .generic) -> [(range: Range<String.Index>, text: String)] {
    let flat: [UInt32]
    do {
        flat = try sqlStatementRanges(sql: sql, dialect: dialect)
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
func sqlStatement(in sql: String, atUTF16Offset caret: Int, dialect: EditorDialect = .generic) -> String? {
    sqlStatementLocated(in: sql, atUTF16Offset: caret, dialect: dialect)?.text
}

/// The statement the caret sits in, with the UTF-16 offset in `sql` of the first character of its
/// trimmed text. The range `sqlStatements` returns starts where the previous `;` ended, so the
/// whitespace the trimming removed from the front is added back to find where the text begins.
func sqlStatementLocated(in sql: String, atUTF16Offset caret: Int,
                         dialect: EditorDialect = .generic) -> (text: String, start: Int)? {
    let found = sqlStatements(in: sql, dialect: dialect)
    guard !found.isEmpty else { return nil }

    let caretIndex = String.Index(utf16Offset: min(max(caret, 0), sql.utf16.count), in: sql)
    // Caret in the whitespace after a statement: that statement is the one being looked at.
    guard let (range, text) = found.first(where: { $0.0.contains(caretIndex) })
            ?? found.last(where: { $0.0.lowerBound <= caretIndex }) ?? found.first else { return nil }
    let leading = sql[range].unicodeScalars.prefix { CharacterSet.whitespacesAndNewlines.contains($0) }
        .reduce(0) { $0 + $1.utf16.count }
    return (text, sql.utf16.distance(from: sql.utf16.startIndex, to: range.lowerBound) + leading)
}
