import AppKit
import SwiftUI
import UniformTypeIdentifiers

extension AppModel {
    /// How close after the popover closed itself a press still belongs to the same click.
    static let exportWizardSameClick: TimeInterval = 0.5

    /// What the footer's Export and the Run menu's Export… do: open the wizard, or close it when it
    /// is open.
    ///
    /// A bare `exportSettingsOpen = true` got stuck. With the wizard open, a click on Export is a
    /// click outside the popover: the popover closes itself and writes `false`, then the button's
    /// action runs in the same click and writes `true` again, while the popover is on its way out.
    /// The flag stayed true with no wizard on screen (a hosted SDK 27 window showed it again about
    /// 500 ms later; the SDK 14 app is assumed to show nothing), and every later press wrote the
    /// `true` it already held, so the button did nothing. Whatever the order of those two writes,
    /// this never leaves the flag true after a press without a wizard on screen, and no state makes
    /// two presses in a row do nothing.
    func toggleExportWizard(now: TimeInterval = ProcessInfo.processInfo.systemUptime) {
        if exportSettingsOpen, exportWizardShown {
            // Open and on screen: this press is the one that closes it.
            exportSettingsOpen = false
        } else if now - exportWizardDismissedAt < Self.exportWizardSameClick {
            // The same click already closed it (the popover wrote `false` first); leave it closed
            // rather than opening it again under the pointer.
            exportSettingsOpen = false
        } else if exportSettingsOpen {
            // Set, with nothing showing. Writing `true` again changes nothing, and writing `false`
            // then `true` a turn later relies on SwiftUI drawing in between (a menu's tracking loop
            // need not). Clear it only: the next press is a plain open, a change SwiftUI has seen.
            exportSettingsOpen = false
        } else {
            exportSettingsOpen = true
        }
    }

    /// Why the footer's Export cannot open the wizard: there is nothing to export yet. What the
    /// wizard itself sets (a folder, a file name, a target) is not on this list. The wizard is where
    /// those are set, so it has to stay reachable while they are missing; its own Export button
    /// still waits for `runBlockedReason(for:)`.
    func exportWizardBlockedReason(for tab: QueryTab) -> String? {
        if connections.isEmpty { return "Add a connection first" }
        if connection(for: tab) == nil { return "Choose a connection" }
        if !tab.hasSQL { return "Write a query" }
        return nil
    }

    /// The popover's own write to the flag: it closes itself on a click outside it, and that close
    /// is remembered so the press that caused it does not reopen it (`toggleExportWizard`).
    func exportWizardPresented(_ open: Bool, now: TimeInterval = ProcessInfo.processInfo.systemUptime) {
        if !open, exportSettingsOpen { exportWizardDismissedAt = now }
        exportSettingsOpen = open
    }

    func rememberDestination(_ tab: QueryTab) {
        UserDefaults.standard.set(tab.outputDirectory?.path, forKey: "lastOutputDirectory")
        UserDefaults.standard.set(tab.format.rawValue, forKey: "lastFormat")
    }

    /// Opens `.sql` files into fresh query tabs on this connection, so a file never overwrites work in
    /// progress (`openFile`).
    func runSQLFile(connectionID: UUID) {
        openSQLFilesPanel(connectionID: connectionID)
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
        panel.message = "Choose a \(ImportSourceFormat.supportedNames) file to import."
        panel.prompt = "Choose"
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.allowedContentTypes = ImportSourceFormat.panelExtensions.compactMap { UTType(filenameExtension: $0) }
        guard panel.runModal() == .OK, let url = panel.url else { return }
        presentImport(from: url, into: node)
    }

    /// Builds the draft for one file and opens the sheet.
    private func presentImport(from url: URL, into node: TreeNode?) {
        guard let format = ImportSourceFormat.detect(path: url.path) else {
            notice = Notice(title: "Unsupported file",
                            message: "Import reads \(ImportSourceFormat.supportedNames). "
                                + "\(url.lastPathComponent) is none of those.")
            return
        }
        // Broken into steps with explicit types on purpose: as a single `??` chain the type
        // checker walks the whole expression at once and some Swift toolchains give up with a
        // misleading "circular reference" (seen on CI's `macos-latest`, not locally).
        let nodeConnectionID: UUID? = node?.connectionID
        let tabConnectionID: UUID? = selectedTab.flatMap { connection(for: $0)?.id }
        let firstConnectionID: UUID? = connections.first?.id
        let connectionID: UUID? = nodeConnectionID ?? tabConnectionID ?? firstConnectionID
        // Named `targetConnection`, not `connection`: a local `connection` shadows the
        // `connection(for:)` method for the whole brace scope, and Swift 6.1 then resolves the
        // call in the closure above to this not-yet-initialised local ("cannot call value of
        // non-function type 'Connection?'"). Swift 6.4 tolerates it; 6.1 does not.
        let targetConnection = connections.first { $0.id == connectionID }
        var mapping = ImportMapping()
        mapping.path = url.path
        mapping.format = format
        mapping.delimiter = format.delimiter
        if let node, node.kind == .table {
            mapping.targetCatalog = node.database ?? ""
            mapping.targetSchema = node.schema ?? ""
            mapping.targetTable = node.title
        } else {
            mapping.targetCatalog = targetConnection?.database ?? ""
            mapping.targetSchema = targetConnection?.schema ?? ""
            mapping.targetTable = url.deletingPathExtension().lastPathComponent
        }
        importDraft = ImportDraft(mapping: mapping, connectionID: connectionID)
    }

    /// Points an open draft at another file, keeping the target the user already set.
    func configure(_ draft: ImportDraft, for url: URL) {
        guard let format = ImportSourceFormat.detect(path: url.path) else {
            notice = Notice(title: "Unsupported file",
                            message: "Import reads \(ImportSourceFormat.supportedNames). "
                                + "\(url.lastPathComponent) is none of those.")
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
        draft.bytesRead = nil
        draft.bytesTotal = nil
        draft.rowsTotal = nil
        var outcome = ImportOutcome()
        var message: String?
        _ = Engine.current.run("import_data", env: env, onEvent: { [weak draft] event in
            switch event.event {
            case "done":
                outcome = ImportOutcome(done: event)
            case "error":
                message = event.message
            case "progress":
                // The engine throttles these, so the numbers move without one event per row. Bytes
                // and their total come for a CSV or JSON file, rows for a sheet (`DBX-32`).
                draft?.bytesRead = event.bytes
                draft?.bytesTotal = event.bytesTotal
                draft?.rowsTotal = event.rowsTotal
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
        // A table on another connection is its own path (W13-T20): it always asks first, on the
        // transfer sheet, whatever either side's Safe Mode is (B-6). A target that cannot be
        // resolved stops here rather than falling back to this tab's own connection.
        if tab.destination == .table, let destination = tableDestination(for: tab) {
            switch destination {
            case .same:
                break
            case .switchedOff, .missing:
                refuseTableRun(tab, reason: destination.blockedReason ?? "Choose the target connection again")
                return
            case .other(let target):
                askTransfer(tab, sql: sql, source: connection, target: target)
                return
            }
        }
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

// MARK: Save to Table across connections (W13-T20)

extension AppModel {
    /// A table run that cannot start, said where a table run reports.
    private func refuseTableRun(_ tab: QueryTab, reason: String) {
        tab.note(.error, reason)
        tab.panel = .log
        panelCollapsed = false
    }

    /// Opens the transfer sheet for a copy to `target`. Nothing runs until it is approved.
    func askTransfer(_ tab: QueryTab, sql: String, source: Connection, target: Connection) {
        if let blocked = runBlockedReason(for: tab) {
            refuseTableRun(tab, reason: blocked)
            return
        }
        let request = RunConfirmation.transferRequest(source: source, target: target, mode: tab.writeMode,
                                                      table: target.kind.targetParts.map(tab.targetValue))
        transfer = TableTransfer(tabID: tab.id, request: request, sql: sql, source: source,
                                 target: target)
    }

    /// The sheet's approve button. The target is resolved again by identity first: the sheet may
    /// have stood open while a connection was edited or deleted, and what runs has to be exactly
    /// what the sheet named (B-6).
    func approveTransfer(_ transfer: TableTransfer) {
        guard transfer.mayApprove, let tab = tabs.first(where: { $0.id == transfer.tabID }),
              tab.stage != .running else { return }
        // Both connections whole, not only the fields the sheet shows: a source that now reads as
        // another user or through another bastion is not the copy that was approved. And the tab
        // must still be a table destination, or the settings below would be an export's.
        guard tab.destination == .table,
              let source = connection(for: tab), source == transfer.source,
              case .other(let target) = tableDestination(for: tab), target == transfer.target,
              runBlockedReason(for: tab) == nil,
              RunConfirmation.transferRequest(source: source, target: target, mode: tab.writeMode,
                                              table: target.kind.targetParts.map(tab.targetValue))
                == transfer.request
        else {
            transfer.phase = .failed
            transfer.failure = "The connections or the target changed while this was open, so nothing "
                + "ran. Close it and save to the table again."
            return
        }
        startTransfer(transfer, tab: tab, source: source, target: target)
    }

    /// Closes the sheet. A copy that is still running goes on, reported in its tab, and Stop still
    /// reaches it from there.
    func dismissTransfer() {
        transfer = nil
    }

    private func startTransfer(_ transfer: TableTransfer, tab: QueryTab, source: Connection,
                               target: Connection) {
        // The statement is the one `run` read (after its own flush) when the sheet opened.
        let env: [String: String]
        do {
            // Both connections' settings, each from its own Keychain items, before anything runs.
            env = try overrides(for: tab, connection: source, sql: transfer.sql, confirmed: true)
            // Defence in depth: settings with no `DEST_` key are the one-connection `to_table`, and
            // with `confirmed` they would carry a confirmation the person gave for something else.
            guard env.keys.contains(where: { $0.hasPrefix(HostKeyGate.destinationPrefix) }) else {
                throw EngineLaunchError(message: "The copy's settings name no target connection, so "
                                            + "nothing ran. Close this and save to the table again.")
            }
        } catch {
            let message = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            transfer.phase = .failed
            transfer.failure = message
            tab.stage = .failed
            tab.failure = message
            tab.failureDetail = ""
            tab.note(.error, message)
            tab.panel = .log
            return
        }
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
        tab.note(.info, "\(source.name) · \(source.displayTarget) → \(target.name) · \(target.displayTarget)")
        tab.note(.info, "\(tab.writeMode.statement) \(transfer.request.table) on \(target.name)")
        transfer.phase = .running

        let run = UUID()
        tab.runToken = run
        var message: String?
        var side: String?
        var done: Event?
        let names = (source: source.name, target: target.name)
        let progress = MainActor.assumeIsolated {
            FileProgress(finderTarget: FileProgress.finderTarget(for: tab),
                         total: FileProgress.knownTotal(for: tab, sql: transfer.sql))
        }
        // `engine` rather than `Engine.current`: the same engine in the app, and one a test can
        // hand a script to.
        tab.process = engine.run("to_table", env: env, onEvent: { event in
            guard tab.runToken == run else { return }
            if event.event == "error" {
                message = event.message
                side = event.side
            }
            if event.event == "done" { done = event }
            if event.event == "progress", let rows = event.rows {
                MainActor.assumeIsolated { progress.update(rows: rows) }
            }
            Self.applyTransferEvent(event, to: tab, transfer: transfer, source: names.source,
                                    target: names.target)
        }, onExit: { [weak self] status, log in
            MainActor.assumeIsolated { progress.finish() }
            guard tab.runToken == run else { return }
            PerfSignposts.cancelEnd()
            tab.process = nil
            tab.stopping = false
            let stopRequested = tab.cancelled
            tab.cancelled = false
            let elapsed = Date.now.timeIntervalSince(tab.startedAt)
            switch TransferOutcome.of(status: status, done: done, stopRequested: stopRequested) {
            case .finished:
                if stopRequested {
                    tab.note(.info, "Stop came after every row was verified, so the table was put in place.")
                }
                tab.finishedAt = .now
                tab.step = "finish"
                tab.stage = .done
                tab.panel = .files
                transfer.phase = .finished
                self?.reportLongRun(.toTable, .done, tab: tab, elapsed: elapsed, rows: tab.rows)
            case .stopped:
                tab.stage = .idle
                tab.note(.warning, transfer.nothingWritten.map { "Stopped. \($0)" }
                         ?? "Stopped. The engine's warnings are above.")
                transfer.phase = .stopped
                self?.reportLongRun(.toTable, .cancelled, tab: tab, elapsed: elapsed, rows: tab.rows)
            case .unknown:
                tab.stage = .idle
                tab.note(.warning, transfer.stopUnconfirmedLine)
                transfer.phase = .stopUnconfirmed
                self?.reportLongRun(.toTable, .cancelled, tab: tab, elapsed: elapsed, rows: tab.rows)
            case .failed:
                let reason = message.map { Self.attributed($0, side: side, source: names.source,
                                                           target: names.target) }
                    ?? (status == 0 ? "The engine exited without reporting completion."
                                    : "The engine exited with status \(status).")
                tab.stage = .failed
                tab.failure = reason
                tab.failureDetail = log
                tab.note(.error, reason)
                tab.panel = .log
                transfer.phase = .failed
                transfer.failure = reason
                self?.reportLongRun(.toTable, .failed, tab: tab, elapsed: elapsed, failure: reason)
            }
        })
    }

    /// An engine message with the connection it belongs to in front, from the `error` event's
    /// `side`: on a copy across connections "connection refused" alone does not say which server.
    static func attributed(_ message: String, side: String?, source: String, target: String) -> String {
        switch side {
        case "source": "\(source): \(message)"
        case "target": "\(target): \(message)"
        default: message
        }
    }

    /// What one event of a copy across connections does to its tab and its sheet. Static, so it can
    /// be tested without an engine, and separate from `handle`: this path's `start` is the table's
    /// shape, not an export's (N-11), and its `rows` are the rows the target acknowledged.
    static func applyTransferEvent(_ event: Event, to tab: QueryTab, transfer: TableTransfer,
                                   source: String, target: String) {
        switch event.event {
        case "error":
            tab.note(.error, attributed(event.message ?? "Engine error", side: event.side,
                                        source: source, target: target))
            // What the failure left on the target rides on the error itself: a scratch table it
            // could not drop, a transaction it could not roll back (review B-1).
            let warnings = event.warnings ?? []
            tab.warnings = warnings
            transfer.warnings = warnings
            for warning in warnings { tab.note(.warning, warning) }
            tab.panel = .log
        case "step":
            tab.step = event.step
            transfer.step = event.step
            if let line = transferStepLine(event.step, source: source, target: target) {
                tab.note(.info, line)
            }
        case "start":
            tab.columns = event.columns ?? []
            tab.queryID = event.queryId
            tab.note(.info, "Query \(event.queryId ?? "(no id)") · \(pluralized(tab.columns.count, "column"))"
                     + " → \(pluralized(event.targetColumns?.count ?? tab.columns.count, "column")) on \(target)")
            for fallback in event.fallbacks ?? [] {
                let line = "Column \(fallback.column) (\(fallback.from)) is written as \(fallback.to): "
                    + "\(target) has no faithful type for it"
                transfer.fallbacks.append(line)
                tab.note(.warning, line)
            }
        case "progress":
            if let rows = event.rows {
                tab.rows = rows
                transfer.rowsWritten = rows
            }
            if let read = event.rowsRead { transfer.rowsRead = read }
        case "done":
            transfer.rowsRead = event.rowsRead ?? transfer.rowsRead
            tab.queryID = event.queryId ?? tab.queryID
            let warnings = event.warnings ?? []
            if event.cancelled == true {
                // Nothing reached the table. The engine's own first line says so in its words; the
                // log says it in the app's when the run exits, with the target's name.
                let rest = warnings.filter { !$0.hasPrefix("stopped; nothing was written") }
                tab.warnings = rest
                transfer.warnings = rest
                for warning in rest { tab.note(.warning, warning) }
                return
            }
            tab.rows = event.rows ?? tab.rows
            transfer.rowsWritten = tab.rows
            tab.writtenTable = event.table
            transfer.table = event.table
            transfer.verified = event.verified ?? false
            tab.warnings = warnings
            transfer.warnings = warnings
            tab.note(.success, transfer.summary)
            for warning in warnings { tab.note(.warning, warning) }
        default:
            break
        }
    }

    /// The log line for each step of a copy, naming the connection the step happens on.
    static func transferStepLine(_ step: String?, source: String, target: String) -> String? {
        switch step {
        case "connect_target": "Connecting to \(target)…"
        case "connect": "Connecting to \(source)…"
        case "write": "Writing rows into \(target)"
        case "verify": "Checking that \(target) holds every row read"
        case "swap": "Putting the table in place on \(target)"
        default: nil
        }
    }
}
