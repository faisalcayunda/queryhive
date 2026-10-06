import AppKit
import SwiftUI
import UniformTypeIdentifiers

extension AppModel {
    func rememberDestination(_ tab: QueryTab) {
        UserDefaults.standard.set(tab.outputDirectory?.path, forKey: "lastOutputDirectory")
        UserDefaults.standard.set(tab.format.rawValue, forKey: "lastFormat")
    }

    /// Opens a `.sql` file into a fresh query tab rather than the one that is already open, so
    /// running a script never overwrites work in progress.
    func runSQLFile(connectionID: UUID) {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = Self.sqlContentTypes
        panel.allowsMultipleSelection = false
        panel.message = "Choose a .sql file to open in a new query"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        newTab(connectionID: connectionID)
        selectedTab?.loadSQL(from: url)
    }

    /// Opens the import sheet for a file the user picks, with the active tab's connection as the
    /// target's default.
    func presentImport() {
        presentImport(into: nil)
    }

    /// Opens the import sheet for a table node's context menu, so the target is already named.
    func presentImport(into node: TreeNode?) {
        guard !connections.isEmpty else {
            notice = Notice(title: "No connections",
                            message: "Add a connection before importing data.")
            return
        }
        let panel = NSOpenPanel()
        panel.title = "Import Data from File"
        panel.message = "Choose a CSV, TSV or XLSX file to read rows from."
        panel.prompt = "Choose"
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.allowedContentTypes = ["csv", "tsv", "xlsx"].compactMap { UTType(filenameExtension: $0) }
        guard panel.runModal() == .OK, let url = panel.url else { return }
        presentImport(from: url, into: node)
    }

    /// Builds the draft for one file and opens the sheet.
    private func presentImport(from url: URL, into node: TreeNode?) {
        guard let format = ImportSourceFormat.detect(path: url.path) else {
            notice = Notice(title: "Unsupported file",
                            message: "Import reads CSV, TSV or XLSX. \(url.lastPathComponent) is none of those.")
            return
        }
        let connectionID = node?.connectionID
            ?? selectedTab.flatMap { connection(for: $0)?.id }
            ?? connections.first?.id
        let connection = connections.first { $0.id == connectionID }
        var mapping = ImportMapping()
        mapping.path = url.path
        mapping.format = format
        mapping.delimiter = format.delimiter
        if let node, node.kind == .table {
            mapping.targetCatalog = node.database ?? ""
            mapping.targetSchema = node.schema ?? ""
            mapping.targetTable = node.title
        } else {
            mapping.targetCatalog = connection?.database ?? ""
            mapping.targetSchema = connection?.schema ?? ""
            mapping.targetTable = url.deletingPathExtension().lastPathComponent
        }
        importDraft = ImportDraft(mapping: mapping, connectionID: connectionID)
    }

    /// Points an open draft at another file, keeping the target the user already set.
    func configure(_ draft: ImportDraft, for url: URL) {
        guard let format = ImportSourceFormat.detect(path: url.path) else {
            notice = Notice(title: "Unsupported file",
                            message: "Import reads CSV, TSV or XLSX. \(url.lastPathComponent) is none of those.")
            return
        }
        draft.mapping.path = url.path
        draft.mapping.format = format
        draft.mapping.delimiter = format.delimiter
        // The old file's columns are gone; the target's are still the target's.
        draft.mapping.fields = []
    }

    /// Reads the target table's columns through the engine's `preview`, for the import mapping.
    ///
    /// `SELECT * FROM <qualified> LIMIT 1`, the same shape the object inspector uses: the engine
    /// describes the result set before the rows arrive, so the `columns` event answers the question
    /// without reading the table. A table that does not exist, or a driver that reports nothing
    /// before a row, calls back with `[]`, and the sheet maps by header name instead of pretending.
    func loadImportColumns(connectionID: UUID, qualifiedName: String,
                           completion: @escaping ([String]) -> Void) {
        guard let connection = connections.first(where: { $0.id == connectionID }) else {
            completion([])
            return
        }
        var env: [String: String]
        do {
            env = try connectionEnvironment(connection)
        } catch {
            completion([])
            return
        }
        env["RETRIES"] = "2"
        env["SQL"] = "SELECT * FROM \(qualifiedName)"
        env["LIMIT"] = "1"
        var columns: [String] = []
        _ = Engine.current.run("preview", env: env, onEvent: { event in
            if event.event == "columns" { columns = (event.columns ?? []).map(\.name) }
        }, onExit: { _, _ in completion(columns) })
    }

    /// Runs `import_data` for an import sheet and fills in what the engine answered.
    ///
    /// No confirmation setting is merged: an import is a bulk path, and the engine's own guard
    /// refuses it on a `confirm` connection rather than take one approval for every statement a
    /// file becomes (ADR-0026). The sheet states the refusal before this is reachable.
    func runImport(_ draft: ImportDraft) {
        guard let connectionID = draft.connectionID,
              let connection = connections.first(where: { $0.id == connectionID }) else {
            draft.failure = "Choose a connection first."
            return
        }
        var env: [String: String]
        do {
            env = try connectionEnvironment(connection)
        } catch {
            draft.failure = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            return
        }
        env.merge(draft.mapping.settings()) { _, new in new }
        draft.running = true
        draft.outcome = nil
        draft.failure = nil
        var outcome = ImportOutcome()
        var message: String?
        _ = Engine.current.run("import_data", env: env, onEvent: { event in
            switch event.event {
            case "done":
                outcome.rows = event.rows ?? 0
                outcome.rejected = event.rejected ?? 0
                outcome.errors = event.errors ?? []
                outcome.stoppedAt = event.stoppedAt
                outcome.mode = event.mode ?? ""
                outcome.disposition = event.disposition
                outcome.transaction = event.transaction ?? false
            case "error":
                message = event.message
            default:
                break
            }
        }, onExit: { [weak draft] status, log in
            guard let draft else { return }
            draft.running = false
            guard status == 0 else {
                draft.failure = message ?? log.split(separator: "\n").last.map(String.init)
                    ?? "The engine exited with status \(status)."
                return
            }
            draft.outcome = outcome
        })
    }

    /// Writes the result out. Separate from `preview` so the toolbar can offer both without one
    /// standing in for the other.
    func run(_ tab: QueryTab, from source: QuerySource = .selection, confirmed: Bool = false,
             substituting: String? = nil) {
        Self.flushEditorsNow()
        guard tab.stage != .running else { return }
        guard let connection = connection(for: tab) else { return }
        // A statement with `:name` in it gets its values first, because everything after this reads
        // the text that runs: the classifier, the confirmation sheet and the history all see the
        // substituted statement rather than the template.
        if substituting == nil, EditorPreferences.shared.queryParameters,
           let prompt = ParameterPrompt.request(tab: tab, source: source, sql: tab.sql(for: source),
                                                kind: connection.kind) {
            parameterPrompt = prompt
            return
        }
        let sql = substituting ?? tab.sql(for: source)
        let command = tab.destination == .table ? "to_table" : "export"
        // A write run on a `confirm` connection asks first. A table destination's own statements
        // are the generated DROP/CREATE/INSERT rather than the caller's SELECT, so those are what
        // the engine guards — and what the sheet names.
        if !confirmed,
           let request = RunConfirmation.request(
                for: destinationStatements(tab, connection: connection, sql: sql),
                command: command,
                safeMode: connection.safeMode) {
            awaitConfirmation(request) { [weak self] in
                self?.run(tab, from: source, confirmed: true, substituting: substituting)
            }
            return
        }
        let env: [String: String]
        do {
            env = try overrides(for: tab, connection: connection, sql: sql, confirmed: confirmed)
        } catch {
            let message = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            tab.stage = .failed
            tab.failure = message
            tab.failureDetail = ""
            tab.note(.error, message)
            tab.panel = .log
            return
        }
        rememberDestination(tab)
        // Navicat opens the message pane when a statement runs; a collapsed panel would hide
        // the only place the progress goes.
        panelCollapsed = false
        tab.stage = .running
        tab.step = nil
        tab.rows = 0
        tab.columns = []
        tab.files = []
        tab.warnings = []
        tab.queryID = nil
        tab.failure = nil
        tab.failureDetail = ""
        tab.cancelled = false
        tab.stopping = false
        tab.startedAt = .now
        tab.finishedAt = .now
        tab.panel = .log
        tab.writtenTable = nil
        tab.note(.info, "\(connection.name) · \(connection.displayTarget)")
        tab.note(.info, tab.runSummary(for: connection.kind))

        let directory = tab.outputDirectory
        let run = UUID()
        tab.runToken = run
        var message: String?
        // Finder and the Dock follow the run (FR-RUN-05). Events and exit arrive on the main queue.
        let kind: LongRunKind = tab.destination == .table ? .toTable : .export
        let progress = MainActor.assumeIsolated {
            FileProgress(finderTarget: FileProgress.finderTarget(for: tab),
                         total: FileProgress.knownTotal(for: tab, sql: sql))
        }
        tab.process = Engine.current.run(command, env: env, onEvent: { [weak self] event in
            guard let self, tab.runToken == run else { return }
            if event.event == "error" { message = event.message }
            if event.event == "progress", let rows = event.rows {
                MainActor.assumeIsolated { progress.update(rows: rows) }
            }
            self.handle(event, in: tab)
        }, onExit: { [weak self] status, log in
            // Before the token check: whichever run this was, its Finder and Dock progress ends.
            MainActor.assumeIsolated { progress.finish() }
            guard tab.runToken == run else { return }
            PerfSignposts.cancelEnd()
            tab.process = nil
            tab.stopping = false
            let elapsed = Date.now.timeIntervalSince(tab.startedAt)
            if tab.cancelled {
                tab.cancelled = false
                tab.stage = .idle
                tab.note(.warning, tab.destination == .table
                         ? "Stopped. The coordinator decides what happens to a partly written table."
                         : "Stopped. Partial files, if any, stay in \(directory?.path ?? "the output folder").")
                self?.reportLongRun(kind, .cancelled, tab: tab, elapsed: elapsed, rows: tab.rows)
                return
            }
            guard status == 0 else {
                tab.stage = .failed
                tab.failure = message ?? "The engine exited with status \(status)."
                tab.failureDetail = log
                tab.note(.error, tab.failure ?? "")
                tab.panel = .log
                self?.reportLongRun(kind, .failed, tab: tab, elapsed: elapsed, failure: tab.failure)
                return
            }
            // A table run reports the table it wrote, not a file list, so that is what proves it
            // finished rather than merely exited cleanly.
            let confirmed = tab.destination == .table ? tab.writtenTable != nil
                                                     : (!tab.files.isEmpty || tab.rows > 0)
            guard confirmed else {
                tab.stage = .failed
                tab.failure = "The engine exited without reporting completion."
                tab.failureDetail = log
                tab.note(.error, tab.failure ?? "")
                tab.panel = .log
                self?.reportLongRun(kind, .failed, tab: tab, elapsed: elapsed, failure: tab.failure)
                return
            }
            tab.finishedAt = .now
            tab.step = "finish"
            tab.stage = .done
            tab.panel = tab.destination == .table ? .files : (tab.files.isEmpty ? .log : .files)
            self?.reportLongRun(kind, .done, tab: tab, elapsed: elapsed, rows: tab.rows)
        })
    }

    /// The notifier the app uses. A test replaces it with one built on fakes.
    @MainActor static var longRunNotifier = LongRunNotifier.system()

    /// Announces the end of a run and, for a long one that finished in the background, notifies.
    /// Permission is asked inside `finished`, at the first notification, never at launch.
    func reportLongRun(_ kind: LongRunKind, _ outcome: LongRunOutcome, tab: QueryTab,
                       elapsed: TimeInterval, rows: Int? = nil, failure: String? = nil) {
        let report = LongRunReport(kind: kind, outcome: outcome, elapsed: elapsed, rows: rows,
                                   failure: failure)
        let id = tab.id
        Task { @MainActor [weak self, weak tab] in
            let notifier = Self.longRunNotifier
            notifier.onSelectTab = { [weak self] id in self?.selectedTabID = id }
            if await notifier.finished(report, tab: id) == .attention {
                tab?.note(.info, "Notifications are off for QueryHive, so the Dock icon bounced instead. "
                          + "Turn them on in System Settings > Notifications.")
            }
        }
    }

    private func handle(_ event: Event, in tab: QueryTab) {
        switch event.event {
        case "error":
            tab.note(.error, event.message ?? "Engine error")
            tab.panel = .log
        case "step":
            tab.step = event.step
            if event.step == "connect" { tab.note(.info, "Connecting…") }
            if event.step == "write" { tab.note(.info, "Streaming rows into the writer") }
        case "start":
            tab.columns = event.columns ?? []
            tab.queryID = event.queryId
            tab.note(.info, "Query \(event.queryId ?? "(no id)") · \(pluralized(tab.columns.count, "column"))")
        case "progress":
            if let total = event.rows { tab.rows = total }
        case "done":
            tab.rows = event.rows ?? tab.rows
            tab.files = event.files ?? []
            tab.writtenTable = event.table
            tab.warnings = event.warnings ?? []
            tab.queryID = event.queryId ?? tab.queryID
            if let table = event.table {
                tab.note(.success, "\(pluralized(tab.rows, "row")) written to \(table)")
            } else {
                tab.note(.success, "\(pluralized(tab.rows, "row")) written to \(pluralized(tab.files.count, "file"))")
            }
            for warning in tab.warnings { tab.note(.warning, warning) }
        default:
            break
        }
    }
}
