import AppKit
import SwiftUI

extension AppModel {
    /// Run the statement the prompt was holding, with the values written in.
    func confirmParameters(_ entries: [String: ParameterEntry]) {
        guard let prompt = parameterPrompt else { return }
        parameterPrompt = nil
        guard let tab = tabs.first(where: { $0.id == prompt.tabID }) else { return }
        switch ParameterRender.statement(prompt.template, entries: entries, driver: prompt.kind) {
        case .success(let sql):
            // Kept for this tab alone, so a second run of the same statement is one return press.
            tab.parameterValues = entries
            run(tab, from: prompt.source, substituting: sql)
        case .failure(let error):
            notice = Notice(title: "Couldn't run with those values", message: error.message)
        }
    }

    func cancelParameters() {
        parameterPrompt = nil
    }

    /// Why Run is dim. Only a connection and a statement — Run does not write anything, so a
    /// destination it has not been given yet is none of its business.
    var runBlockedReason: String? {
        if connections.isEmpty { return "Add a connection first" }
        guard let tab = selectedTab else { return nil }
        if connection(for: tab) == nil { return "Choose a connection" }
        if !tab.hasSQL { return "Write a query" }
        return nil
    }

    /// Why Export is dim. Everything Run needs, plus somewhere to put the result.
    func runBlockedReason(for tab: QueryTab) -> String? {
        if connections.isEmpty { return "Add a connection first" }
        if connection(for: tab) == nil { return "Choose a connection" }
        if !tab.hasSQL { return "Write a query" }
        switch tab.destination {
        case .file:
            if tab.trimmedName.isEmpty { return "Name the output file" }
            if tab.outputDirectory == nil { return "Choose an output folder" }
        case .table:
            if tab.trimmedCatalog.isEmpty { return "Name the target catalog" }
            if tab.trimmedSchema.isEmpty { return "Name the target schema" }
            if tab.trimmedTable.isEmpty { return "Name the target table" }
        }
        return nil
    }

    /// Navicat's split, and the reason this app is a query editor rather than a one-way pipe:
    /// **Run looks at the rows; Export writes them.** Run fetches the row limit and stops, so
    /// looking is cheap and reversible; Export streams the whole result to the destination.
    func runSelectedTab() {
        guard let tab = selectedTab else { return }
        preview(tab)
    }

    /// How often the footer's row count follows a streaming run (D-28). The grid itself is polled by
    /// its display link; this is only the one number SwiftUI watches, so it is not re-evaluated per
    /// engine batch.
    static let footerCountInterval: TimeInterval = 0.2

    /// `source` decides what is sent: the selection, the statement under the caret, or everything.
    func preview(_ tab: QueryTab, from source: QuerySource = .selection) {
        Self.flushEditorsNow()
        guard !tab.previewing, tab.stage != .running else { return }
        guard let connection = connection(for: tab) else { return }
        let sql = tab.sql(for: source)
        let env: [String: String]
        do {
            env = try previewEnvironment(for: tab, connection: connection, sql: sql)
        } catch {
            tab.previewError = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            return
        }
        // A Run of a write on a `confirm` connection asks first; the approval is merged into this
        // one run's environment and is never stored.
        if let request = RunConfirmation.request(for: statements(in: sql), command: "preview",
                                                 safeMode: connection.safeMode) {
            awaitConfirmation(request) { [weak self] in
                self?.runPreview(tab, sql: sql, connection: connection,
                                 env: env.merging(RunConfirmation.approvalSettings(true)) { _, new in new },
                                 baseRun: true)
            }
            return
        }
        runPreview(tab, sql: sql, connection: connection, env: env, baseRun: true)
    }

    /// The statements a script holds, split the way the engine splits them, so the confirmation
    /// names the same pieces the engine's own classifier reads.
    func statements(in sql: String) -> [String] {
        sqlStatements(in: sql).map(\.text)
    }

    /// Hold a run until the user answers it, with the action that starts it on approval.
    func awaitConfirmation(_ request: RunConfirmation.Request, run: @escaping () -> Void) {
        pendingConfirmation = PendingConfirmation(request: request, approve: run)
    }

    /// The statements a write run would execute, as far as the app can name them.
    ///
    /// An export streams the caller's statement; a table destination sends the statements the
    /// engine generates for the chosen write mode (`crates/qh-ffi/src/commands.rs`). The generated
    /// ones are spelled from the app's own qualified target, which is what the confirmation sheet
    /// shows; the engine quotes them itself when it runs, and both name the same table.
    func destinationStatements(_ tab: QueryTab, connection: Connection, sql: String) -> [String] {
        guard tab.destination == .table else { return statements(in: sql) }
        let target = tab.target(for: connection.kind)
        let body = sql.trimmingCharacters(in: .whitespacesAndNewlines)
            .trimmingCharacters(in: CharacterSet(charactersIn: ";"))
        switch tab.writeMode {
        case .replace:
            return ["DROP TABLE IF EXISTS \(target)", "CREATE TABLE \(target) AS \(body)"]
        case .append:
            return ["INSERT INTO \(target) \(body)"]
        case .create:
            return ["CREATE TABLE \(target) AS \(body)"]
        }
    }

    /// Escalate the grid's in-memory search: run the same statement again with the term turned into
    /// a cross-column `WHERE`, so the server finds rows the grid never fetched.
    ///
    /// The run is the ordinary `preview` one, so the safety mode, the timeout, the retries and the
    /// streaming are the same as a Run; only the SQL differs. The statement is built by
    /// `SearchStatement`, which wraps the user's SQL inside a derived table rather than editing it,
    /// and is refused rather than guessed when the text holds more than one statement.
    func searchOnServer(_ tab: QueryTab, term: String) {
        guard !tab.previewing, tab.stage != .running else { return }
        if let blocked = serverActionBlockedByEdits(tab) {
            tab.note(.error, blocked)
            tab.panel = .log
            return
        }
        guard let connection = connection(for: tab) else { return }
        guard let original = tab.previewBaseSQL ?? tab.previewedSQL, !original.isEmpty else {
            tab.note(.warning, "Run the query before searching the server.")
            tab.panel = .log
            return
        }
        let statement: String
        do {
            statement = try SearchStatement.crossColumn(sql: original, term: term,
                                                        columns: tab.preview?.columns ?? [],
                                                        kind: connection.kind)
        } catch {
            tab.note(.error, searchFailureMessage(error))
            tab.panel = .log
            return
        }
        let env: [String: String]
        do {
            env = try previewEnvironment(for: tab, connection: connection, sql: statement)
        } catch {
            tab.previewError = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            return
        }
        tab.note(.info, "Searching the server for “\(term)” across every column…")
        runPreview(tab, sql: statement, connection: connection, env: env, clearSearch: false,
                   baseSQL: original, serverSearch: term)
    }

    /// Why an escalation could not be built, in the words the user needs to fix it.
    private func searchFailureMessage(_ error: Error) -> String {
        guard let failure = error as? SearchStatement.Failure else {
            return "Cannot search on the server: \(error.localizedDescription)"
        }
        switch failure {
        case .blank:
            return "Nothing to search for."
        case .multipleStatements(let count):
            return "Searching the server needs one statement, and this result's text has \(count). "
                + "Run the one you mean, then search again."
        case .noColumns:
            return "Cannot search on the server: none of this result's columns can be read as text."
        }
    }

    /// Escalate the grid's in-memory sort: run the same statement again with an `ORDER BY`, so the
    /// server orders the whole result rather than the rows that were fetched.
    ///
    /// The run is the ordinary `preview` one, so the safety mode, the timeout, the retries and the
    /// streaming are the same as a Run; only the SQL differs. The statement is built by
    /// `ServerSort`, which wraps the user's SQL inside a derived table rather than editing it, and
    /// is refused rather than guessed when the text holds more than one statement.
    func sortOnServer(_ tab: QueryTab, column: Event.Column, source: Int, direction: GridSort.Direction) {
        guard !tab.previewing, tab.stage != .running else { return }
        if let blocked = serverActionBlockedByEdits(tab) {
            tab.note(.error, blocked); tab.panel = .log; return
        }
        guard let connection = connection(for: tab) else { return }
        guard let original = tab.previewBaseSQL ?? tab.previewedSQL, !original.isEmpty else {
            tab.note(.warning, "Run the query before sorting it on the server.")
            tab.panel = .log
            return
        }
        let statement: String
        do {
            statement = try ServerSort.order(sql: original, column: column.name,
                                             direction: direction, kind: connection.kind)
        } catch {
            tab.applyMemorySort(GridSort(column: source, direction: direction))
            return
        }
        let env: [String: String]
        do {
            env = try previewEnvironment(for: tab, connection: connection, sql: statement)
        } catch {
            tab.previewError = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            return
        }
        tab.applyServerSort(column: source, direction: direction)
        tab.note(.info, "Sorting on the server by \(column.name) "
                + "\(direction == .ascending ? "↑" : "↓") — the whole result, not only the rows fetched.")
        runPreview(tab, sql: statement, connection: connection, env: env, clearSearch: false,
                   baseSQL: original, activeSort: tab.activeSort)
    }

    /// A header click, server-first: the first click sorts on the server, the
    /// second flips, the third clears. Falls back to memory only for a plan,
    /// an object preview, or a statement the builder refuses to wrap.
    func toggleSort(_ tab: QueryTab, column: Event.Column, source: Int) {
        let current = tab.activeSort.flatMap {
            $0.column == source ? GridSort(column: source, direction: $0.direction) : nil
        }
        let next = GridSort.next(current, clickedColumn: source,
                                 firstDirection: DataPreferences.shared.firstSortDirection)
        setSort(tab, column: column, source: source, sort: next)
    }

    /// One explicit order, routed like a click: server unless a fallback case
    /// holds. A refused builder falls back inside `sortOnServer` itself.
    func setSort(_ tab: QueryTab, column: Event.Column, source: Int, sort: GridSort?) {
        guard let sort else { clearSort(tab); return }
        if SortPolicy.route(builderRefused: false, showingPlan: tab.showingPlan,
                            isObjectResult: tab.isObjects) == .memory {
            // Rust answers `Streaming` to a sort until every row has arrived (§17.3).
            guard !tab.previewing else { return }
            tab.applyMemorySort(sort)
            return
        }
        sortOnServer(tab, column: column, source: source, direction: sort.direction)
    }

    /// The cycle's "off": a memory order just lifts, a server order returns to the base store,
    /// or runs the base statement again when there is none (§18).
    func clearSort(_ tab: QueryTab) {
        guard let sort = tab.activeSort else { return }
        tab.activeSort = nil
        guard sort.origin == .server, tab.serverSearch == nil else {
            if sort.origin == .memory { tab.scheduleViewApply() }
            return
        }
        restoreBase(tab)
    }

    /// Back to the result as the server first sent it. The base store takes over from a server-sorted
    /// or server-searched one, and carries the filters and the in-memory search the tab still shows
    /// (an empty `ViewSpec` would drop filters whose funnels are still lit).
    private func restoreBase(_ tab: QueryTab) {
        guard let base = tab.baseResult else { rerunBaseSQL(tab); return }
        if tab.activeResult !== base.rows { tab.activeResult?.release() }
        tab.activeResult = base.rows
        tab.preview = base.meta
        tab.fetchedRows = base.meta.rowCount
        tab.scheduleViewApply()
    }

    /// The base statement again, for an "off" with no stored base: holding
    /// two large buffers would double memory, so a large base is re-read.
    func rerunBaseSQL(_ tab: QueryTab) {
        guard let sql = tab.previewBaseSQL, !sql.isEmpty,
              let connection = connection(for: tab) else { return }
        guard let env = try? previewEnvironment(for: tab, connection: connection, sql: sql) else {
            tab.previewError = "Couldn't re-run the base query."
            return
        }
        runPreview(tab, sql: sql, connection: connection, env: env, baseRun: true)
    }

    /// Queue one server search per pause in typing; a keystroke cancels the
    /// one before it. Below the minimum the active server search is cleared.
    func scheduleServerSearch(_ tab: QueryTab) {
        searchTasks[tab.id]?.cancel()
        let term = tab.gridSearch
        let id = tab.id
        let work = DispatchWorkItem { [weak self] in
            self?.searchTasks[id] = nil
            self?.fireServerSearch(tab, term: term)
        }
        searchTasks[tab.id] = work
        DispatchQueue.main.asyncAfter(deadline: .now() + GridSearch.debounceInterval, execute: work)
    }

    /// The debounced fire: no-ops when a run is in flight, when the term is
    /// below the minimum, or when memory already owns the search. A refused
    /// builder stays in memory, which is already narrowing the fetched rows.
    func fireServerSearch(_ tab: QueryTab, term: String) {
        guard !tab.previewing, tab.stage != .running else { return }
        guard let searchable = GridSearch.searchableTerm(term) else {
            if tab.serverSearch != nil { clearSearch(tab) }
            return
        }
        if tab.showingPlan || tab.isObjects { return }
        if tab.serverSearch == searchable { return }
        if !tab.cellEdits.isEmpty { return }
        guard let original = tab.previewBaseSQL ?? tab.previewedSQL, !original.isEmpty,
              let connection = connection(for: tab),
              (try? SearchStatement.crossColumn(sql: original, term: searchable,
                                               columns: tab.preview?.columns ?? [],
                                               kind: connection.kind)) != nil else { return }
        searchOnServer(tab, term: searchable)
    }

    /// Drop the server search: back to the stored base, or re-run it when the
    /// base was too large to keep. The field keeps its text; only the rows go.
    func clearSearch(_ tab: QueryTab) {
        tab.serverSearch = nil
        restoreBase(tab)
    }

    /// The body of a preview run, shared by Run and the search escalation: put the tab into its
    /// "a new result is arriving" state and start the engine.
    private func runPreview(_ tab: QueryTab, sql: String, connection: Connection,
                            env: [String: String], clearSearch: Bool = true,
                            baseSQL: String? = nil, activeSort: ActiveSort? = nil,
                            serverSearch: String? = nil, baseRun: Bool = false) {
        let store: StoreRows
        do { store = try engine.makeResultStore() } catch {
            tab.previewError = "Could not open a result store: \(error.localizedDescription)"
            return
        }
        PerfSignposts.runBegin()
        tab.previewing = true
        tab.previewError = nil
        // §18. A fresh Run retires both stores. A sort or search run keeps the base, because "off"
        // must return to the rows they started from, and lets go of the result it replaces.
        if baseRun {
            tab.releaseResults()
        } else {
            if tab.activeResult !== tab.baseResult?.rows { tab.activeResult?.release() }
            tab.activeResult = nil
        }
        tab.preview = nil
        tab.showingPlan = false
        tab.previewedSQL = sql
        tab.previewBaseSQL = baseSQL ?? sql
        // Set on every run, so an ordinary Run clears the order a sort left.
        tab.activeSort = activeSort
        tab.serverSearch = serverSearch

        // The filters described rows that are about to be replaced, and clearing them is also what
        // drops the cell selection — see `columnFilters`' own note. The last total described them
        // too.
        tab.columnFilters = [:]
        // A fresh Run clears the cross-column search with the filters: neither describes the rows
        // that are arriving. The escalated search is the exception — it *is* the search — so its
        // caller keeps the term on screen.
        if clearSearch { tab.gridSearch = "" }
        // After the clearing above, so those two do not ask the new, empty store for a view.
        tab.activeResult = store
        tab.fetchedRows = 0
        tab.totalRows = nil
        tab.countError = nil
        tab.panel = .result
        panelCollapsed = false

        let run = UUID()
        tab.previewToken = run
        // When the user asked for it, which is what the history row's `started_at` records. Read
        // here rather than inside the engine because the engine only knows when it started work.
        let startedAt = Date()
        var message: String?
        var columns: [Event.Column] = []
        var fetched = 0
        var stopped = false
        var finished = false
        var footerAt = Date.distantPast
        var woke = false
        tab.previewProcess = engine.runIntoStore("preview", env: env, store: store, onEvent: { event in
            guard tab.previewToken == run else { return }
            switch event.event {
            case "error":
                message = event.message
            case "columns":
                columns = event.columns ?? []
                store.setColumns(columns)
                // Paint the header as soon as it is known rather than after the first batch.
                tab.preview = PreviewResult(columns: columns, rowCount: 0, truncated: false,
                                            queryID: nil, elapsedMS: 0)
                // A statement that writes returns no columns and so no rows: nothing to hold.
                if columns.isEmpty, tab.activeResult === store {
                    tab.activeResult = nil
                    store.release()
                }
            case "progress":
                // The grid polls the store itself. The footer's count is the only thing handed to
                // SwiftUI, and at most five times a second (D-28); the first progress also wakes
                // the grid at once, so the first rows do not wait for a display tick.
                fetched = event.rows ?? fetched
                if !woke { woke = true; tab.pollHook?() }
                let now = Date()
                if now.timeIntervalSince(footerAt) >= Self.footerCountInterval {
                    footerAt = now
                    tab.fetchedRows = fetched
                }
            case "done":
                PerfSignposts.runDone()
                finished = true
                stopped = Self.applyPreviewDone(event, columns: columns, rowCount: event.rows ?? store.fetched,
                                                to: tab, store: store, storeBase: baseRun)
            default:
                break
            }
        }, onExit: { status, log in
            guard tab.previewToken == run else { return }
            PerfSignposts.cancelEnd()
            tab.previewProcess = nil
            tab.previewing = false
            guard status == 0, finished else {
                // Read before `preview` is cleared on the next line. The elapsed time lives on the
                // result, so recording it after the clear wrote `null` for every failed and
                // cancelled run: a failure still knows how long it took to fail.
                let elapsed = tab.preview?.elapsedMS
                tab.previewError = message ?? log.split(separator: "\n").last.map(String.init)
                    ?? "The engine exited with status \(status)."
                tab.preview = nil
                if tab.activeResult === store { tab.activeResult = nil }
                if tab.baseResult?.rows !== store { store.release() }
                tab.note(.error, tab.previewError ?? "Preview failed")
                tab.panel = .log
                Self.recordHistory(connection: connection, sql: sql, startedAt: startedAt,
                                   outcome: tab.cancelled ? "cancelled" : "error",
                                   elapsedMS: elapsed, rowCount: fetched,
                                   error: tab.previewError, recording: self.recordsHistory, engine: self.engine)
                // Re-read, so a History panel that is already on screen counts this run without
                // the user having to switch panels. `onAppear` only fires once per appearance.
                self.loadHistory(search: self.historySearch)
                return
            }
            Self.recordHistory(connection: connection, sql: sql, startedAt: startedAt,
                               outcome: stopped ? "cancelled" : "ok", elapsedMS: tab.preview?.elapsedMS,
                               rowCount: tab.preview?.rowCount ?? fetched, error: nil,
                               recording: self.recordsHistory, engine: self.engine)
            self.loadHistory(search: self.historySearch)
        })
    }

    /// What a preview's `done` event does to the tab; returns whether the run was stopped.
    ///
    /// A method of its own so it can be tested without an engine (`Engine.current` is a `static
    /// let`). `done.cancelled` is the engine's verdict that Stop reached it: the rows are whatever
    /// arrived first, and an empty one is not "no rows matched". A stopped result with rows is
    /// partial, so it counts as truncated too: Sort on Server and the partial-order banner stay
    /// honest. `done.warnings` (for instance "the server did not confirm the stop") go to the log.
    @discardableResult
    static func applyPreviewDone(_ event: Event, columns: [Event.Column], rowCount: Int,
                                 to tab: QueryTab, store: StoreRows? = nil,
                                 storeBase: Bool = false) -> Bool {
        let stopped = event.cancelled ?? false
        let truncated = (event.truncated ?? false) || (stopped && rowCount > 0)
        tab.preview = PreviewResult(columns: columns, rowCount: rowCount, truncated: truncated,
                                    queryID: event.queryId, elapsedMS: event.elapsedMs ?? 0,
                                    stopped: stopped)
        tab.fetchedRows = rowCount
        // The base run keeps its store for "off". Its own store: holding it beside the active one
        // does not double the resident rows, because a store that is not being read spills first.
        if storeBase, !stopped, let store, let meta = tab.preview {
            tab.baseResult = ResultSlot(meta: meta, rows: store)
        }
        if stopped {
            tab.note(.warning, rowCount == 0 ? "Stopped before any rows arrived"
                                             : "Stopped · \(pluralized(rowCount, "row")) fetched")
        } else {
            tab.note(.success, "\(pluralized(rowCount, "row")) returned\(truncated ? " (limit reached)" : "")")
        }
        for warning in event.warnings ?? [] { tab.note(.warning, warning) }
        return stopped
    }

    /// The rows one Run may fetch. The whole result is held in memory and painted, so an unbounded
    /// field is a way to freeze the app. 200,000 until the streaming grid lands (owner decision
    /// O-12), then 5,000,000.
    static let productRowLimitCeiling = 200_000

    /// The effective ceiling. Only `--bench` writes it (BenchMode raises it so the 500k scenarios
    /// keep measuring 500k, comparable with Fase 0); the product never does.
    nonisolated(unsafe) static var rowLimitCeiling = productRowLimitCeiling

    static var rowLimitRange: ClosedRange<Int> { 1...rowLimitCeiling }

    static func clampedRowLimit(_ value: Int) -> Int {
        min(max(value, rowLimitRange.lowerBound), rowLimitRange.upperBound)
    }

    static func rowLimitClampMessage(_ asked: Int) -> String {
        "\(asked.formatted()) is outside \(rowLimitRange.lowerBound.formatted()) to "
            + "\(rowLimitRange.upperBound.formatted()) rows, so the limit was set to "
            + "\(clampedRowLimit(asked).formatted())."
    }

    func cancelPreview(_ tab: QueryTab) {
        if tab.previewProcess != nil { PerfSignposts.cancelBegin() }
        tab.previewProcess?.terminate()
    }

    /// Explains the query instead of running it, and shows the plan where the rows would go.
    ///
    /// EXPLAIN's spelling belongs to the driver, so the engine owns it; this sends the same
    /// statement a Run would and lets the reply land in the grid. Deliberately the same context
    /// (`database(for:)` / `schema(for:)`) and the same source resolution, because a plan for a
    /// different context than the one the query would run in is worse than no plan.
    func explain(_ tab: QueryTab, from source: QuerySource = .selection, confirmed: Bool = false) {
        Self.flushEditorsNow()
        guard !tab.previewing, !tab.explaining, tab.stage != .running else { return }
        guard let connection = connection(for: tab) else { return }
        let sql = tab.sql(for: source)
        if !confirmed,
           let request = RunConfirmation.request(for: statements(in: sql), command: "explain",
                                                 safeMode: connection.safeMode) {
            awaitConfirmation(request) { [weak self] in
                self?.explain(tab, from: source, confirmed: true)
            }
            return
        }
        var env: [String: String]
        do {
            env = try previewEnvironment(for: tab, connection: connection, sql: sql)
            if confirmed { env.merge(RunConfirmation.approvalSettings(true)) { _, new in new } }
        } catch {
            tab.previewError = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            return
        }
        let store: StoreRows
        do { store = try engine.makeResultStore() } catch {
            tab.previewError = "Could not open a result store: \(error.localizedDescription)"
            return
        }
        tab.explaining = true
        tab.previewError = nil
        // A plan replaces both stores (§18): "off" has no base to return to afterwards.
        tab.releaseResults()
        tab.preview = nil
        tab.activeResult = store
        tab.fetchedRows = 0
        tab.previewedSQL = sql
        tab.activeSort = nil
        tab.serverSearch = nil
        tab.showingPlan = true
        tab.panel = .result
        panelCollapsed = false

        let run = UUID()
        tab.previewToken = run
        var message: String?
        var columns: [Event.Column] = []
        var finished = false
        var woke = false
        tab.previewProcess = engine.runIntoStore("explain", env: env, store: store, onEvent: { event in
            guard tab.previewToken == run else { return }
            switch event.event {
            case "error":
                message = event.message
            case "columns":
                columns = event.columns ?? []
                store.setColumns(columns)
                tab.preview = PreviewResult(columns: columns, rowCount: 0, truncated: false,
                                            queryID: nil, elapsedMS: 0)
            case "progress":
                tab.fetchedRows = event.rows ?? tab.fetchedRows
                if !woke { woke = true; tab.pollHook?() }
            case "done":
                finished = true
                let count = event.rows ?? store.fetched
                if event.cancelled == true {
                    Self.applyPreviewDone(event, columns: columns, rowCount: count, to: tab, store: store)
                    break
                }
                tab.preview = PreviewResult(columns: columns, rowCount: count, truncated: false,
                                            queryID: event.queryId, elapsedMS: event.elapsedMs ?? 0)
                tab.fetchedRows = count
                tab.note(.success, "Plan returned \(pluralized(count, "line"))")
            default:
                break
            }
        }, onExit: { status, log in
            guard tab.previewToken == run else { return }
            tab.previewProcess = nil
            tab.explaining = false
            guard status == 0, finished else {
                tab.previewError = message ?? log.split(separator: "\n").last.map(String.init)
                    ?? "The engine exited with status \(status)."
                tab.preview = nil
                if tab.activeResult === store { tab.activeResult = nil }
                store.release()
                tab.showingPlan = false
                tab.note(.error, tab.previewError ?? "Explain failed")
                tab.panel = .log
                return
            }
        })
    }

    /// DBeaver's "fetch row count": asks the server how many rows the statement on screen really
    /// returns. Deliberately a button and not something the preview does — it is a second query
    /// over the whole result, which can be slow and which the user should choose to pay for.
    func countRows(_ tab: QueryTab, confirmed: Bool = false) {
        guard !tab.countingRows, let sql = tab.previewedSQL, !sql.isEmpty,
              let connection = connection(for: tab) else { return }
        // The engine guards `count` against the caller's own statement, not the `COUNT(*)` wrapper,
        // so counting a write still needs the confirmation at `confirm`.
        if !confirmed,
           let request = RunConfirmation.request(for: statements(in: sql), command: "count",
                                                 safeMode: connection.safeMode) {
            awaitConfirmation(request) { [weak self] in self?.countRows(tab, confirmed: true) }
            return
        }
        let env: [String: String]
        do {
            var built = try connectionEnvironment(connection)
            built["SQL"] = sql
            built["RETRIES"] = String(tab.retries)
            if confirmed { built.merge(RunConfirmation.approvalSettings(true)) { _, new in new } }
            env = built
        } catch {
            tab.countError = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            return
        }
        tab.countingRows = true
        tab.countError = nil
        let run = UUID()
        tab.countToken = run
        tab.countProcess = Engine.current.run("count", env: env, onEvent: { event in
            guard tab.countToken == run else { return }
            self.applyCountEvent(event, to: tab)
        }, onExit: { status, log in
            guard tab.countToken == run else { return }
            tab.countProcess = nil
            tab.countingRows = false
            if status != 0, tab.totalRows == nil {
                tab.countError = tab.countError
                    ?? log.split(separator: "\n").last.map(String.init)
                    ?? "The count failed."
            }
        })
    }

    /// What one event from a `count` run means for the tab.
    ///
    /// A method of its own so the mapping can be tested without an engine: `Engine.current` is a
    /// `static let`, so the run itself cannot be scripted. It is worth the seam, because this is
    /// exactly where the footer's total went missing — the number arrives in `rows` on the `count`
    /// event, and the reading here asked for `count`, a key neither engine writes, so `totalRows`
    /// stayed nil and the footer kept offering the button as if nothing had been asked.
    ///
    /// Gated on the event name rather than on the field alone: `rows` is also the integer on
    /// `progress` and `done`, so an ungated read would take a preview's row count for the total.
    func applyCountEvent(_ event: Event, to tab: QueryTab) {
        if event.event == "error" { tab.countError = event.message }
        if event.event == "count", let rows = event.rows { tab.totalRows = rows }
    }

    /// The environment for a preview: the connection plus the statement and the row cap. Deliberately
    /// *not* the destination — looking at rows must not depend on having picked a file or a table.
    private func previewEnvironment(for tab: QueryTab, connection: Connection,
                                    sql: String) throws -> [String: String] {
        var env = try connectionEnvironment(connection)
        // The cascade overrides the connection's own database/schema. Trino resolves an
        // unqualified table against these, which is the whole point of picking them.
        env["DB_DATABASE"] = database(for: tab)
        env["DB_SCHEMA"] = schema(for: tab)
        env["SQL"] = sql
        let limit = Self.clampedRowLimit(tab.rowLimit)
        if limit != tab.rowLimit {
            tab.note(.warning, Self.rowLimitClampMessage(tab.rowLimit))
            tab.rowLimit = limit
        }
        env["LIMIT"] = String(limit)
        env["RETRIES"] = String(tab.retries)
        return env
    }

    func stopSelectedTab() {
        guard let tab = selectedTab else { return }
        stop(tab)
    }

    func stop(_ tab: QueryTab) {
        guard tab.stage == .running, !tab.stopping else { return }
        tab.stopping = true
        tab.cancelled = true
        tab.note(.warning, "Stopping…")
        if tab.process != nil { PerfSignposts.cancelBegin() }
        tab.process?.terminate()
    }

    /// Full environment for one export run: the connection plus the query, the destination and
    /// the per-format options. Throws rather than launching with a password the Keychain refused
    /// to hand over, which would silently export as the wrong identity.
    ///
    /// `confirmed` carries the user's approval of a `confirm`-level write into the run's own
    /// environment. It is never stored: the engine's whole model of a confirmation is that one run
    /// carries the flag.
    func overrides(for tab: QueryTab, connection: Connection,
                           sql: String, confirmed: Bool = false) throws -> [String: String] {
        var env = try connectionEnvironment(connection)
        env.merge(RunConfirmation.approvalSettings(confirmed)) { _, new in new }
        // Same context rule as a preview: an export runs where the cascade says it runs.
        env["DB_DATABASE"] = database(for: tab)
        env["DB_SCHEMA"] = schema(for: tab)
        env["SQL"] = sql
        env["RETRIES"] = String(tab.retries)
        switch tab.destination {
        case .file:
            guard let directory = tab.outputDirectory else {
                throw EngineLaunchError(message: "Choose an output folder before running.")
            }
            env["FORMAT"] = tab.format.rawValue
            env["OUT_DIR"] = directory.path
            env["NAME"] = tab.trimmedName
            env["ZIP"] = tab.zip ? "1" : ""
            env["BATCH_SIZE"] = String(tab.batchSize)
            env["ROWS_PER_FILE"] = tab.splitRows > 0 ? String(tab.splitRows) : ""
            env["DELIMITER"] = tab.delimiter
            env["ENCODING"] = tab.encoding
            env["HEADER"] = tab.header ? "1" : "0"
            env["BOM"] = tab.bom ? "1" : ""
            env["NULL_TEXT"] = tab.nullText
            env["JSONL"] = tab.jsonl ? "1" : ""
            env["SQL_TABLE"] = tab.sqlTable
            env["SHEET"] = tab.sheet
            env["DBF_CHAR_WIDTH"] = String(tab.dbfCharWidth)
        case .table:
            // Kept separate from the session catalog: a query may well read from one catalog and
            // write into another.
            env["TARGET_CATALOG"] = tab.trimmedCatalog
            env["TARGET_SCHEMA"] = tab.trimmedSchema
            env["TARGET_TABLE"] = tab.trimmedTable
            env["WRITE_MODE"] = tab.writeMode.rawValue
        }
        return env
    }
}
