import AppKit
import SwiftUI

extension AppModel {
    /// The saved queries the user keeps within reach, for the sidebar.
    ///
    /// Derived rather than kept as a second list: two lists can disagree after a save or a delete,
    /// and the only thing this one adds is a filter.
    var favouriteQueries: [Event.SavedQuery] {
        savedQueries.filter(\.favourite)
    }

    /// Keep a saved query within reach, or stop keeping it.
    func toggleFavourite(_ query: Event.SavedQuery) {
        let env = [
            "SAVED_ACTION": "favourite",
            "SAVED_ID": query.id,
            "FAVOURITE": query.favourite ? "0" : "1",
        ]
        _ = Engine.current.run("saved_queries", env: Self.localEnvironment(env), onEvent: { event in
            if event.event == "error" { self.savedError = event.message }
        }, onExit: { _, _ in
            // Re-read rather than flipping the flag in place: the engine owns the revision, and both
            // the sidebar and the panel should show what it stored, not what this hoped it stored.
            self.loadSavedQueries()
        })
    }

    /// The display name of the connection a history row belongs to.
    ///
    /// `nil` for a row with no connection and for one whose connection has since been deleted. The
    /// panel draws nothing either way rather than a placeholder, because the row is still true.
    ///
    /// Matched as a UUID string rather than by comparing identities, because that is what the engine
    /// stores on the row: `recordHistory` writes `uuidString.lowercased()`.
    func connectionName(for historyID: String?) -> String? {
        guard let historyID, let id = UUID(uuidString: historyID) else { return nil }
        return connections.first { $0.id == id }?.name
    }

    /// Throw the whole history away, then re-read what is left.
    ///
    /// The engine answers how many rows were living when it ran, which is what the notice says
    /// afterwards: "412 rows removed" is a fact, and "cleared" on its own is not.
    func clearHistory() {
        var cleared: Int?
        _ = Engine.current.run("history_clear", env: Self.localEnvironment([:]),
                               onEvent: { event in
            if event.event == "history_clear" { cleared = event.cleared }
            if event.event == "error" { self.historyError = event.message }
        }, onExit: { _, _ in
            self.loadHistory(search: self.historySearch)
            if let cleared {
                self.notice = Notice(
                    title: "History cleared",
                    message: cleared == 1 ? "1 row removed." : "\(cleared) rows removed.")
            }
        })
    }

    /// Leaves one row in the engine's history for a Run that has finished.
    ///
    /// Run is `preview`, so this is where a Run becomes a fact: the statement, the connection,
    /// when the user asked for it, how long it took, how many rows came back, and how it ended.
    /// Only Run writes history. Explain does not, because a plan is not a result, and Export does
    /// not, because a history that mixed "I looked at this" with "I wrote this somewhere" would
    /// answer neither question.
    ///
    /// Fire and forget. The reply is one line nobody reads, and letting a failed history write
    /// reach the screen would turn a query that worked into a visible failure. `history_add`
    /// dedupes on connection, statement and start, so two finishes of one run stay one row.
    ///
    /// Static because it needs nothing from the model, and an instance method would put `self`
    /// inside a closure that outlives this call for no reason.
    static func recordHistory(connection: Connection, sql: String, startedAt: Date,
                                      outcome: String, elapsedMS: Int?, rowCount: Int?,
                                      error: String?, recording: Bool,
                                      engine: any DatabaseEngine) {
        // The switch in Settings reaches the history here and nowhere else, so there is one place to
        // look when a user asks why a run was not written down. Passed in rather than read from
        // `UserDefaults` here, because the default-inversion that makes it on-by-default is the
        // property's business and not this function's.
        guard recording else { return }
        // No `DB_PATH` and no `DB_*`: `history_add` is a local command that never opens a driver,
        // and without `DB_PATH` the engine writes to the same Application Support database the
        // app's connections already live in.
        var env: [String: String] = [
            "SQL": sql,
            "CONNECTION_ID": connection.id.uuidString.lowercased(),
            "STARTED_AT": String(Int(startedAt.timeIntervalSince1970 * 1000)),
            "OUTCOME": outcome,
        ]
        if let elapsedMS = elapsedMS { env["ELAPSED_MS"] = String(elapsedMS) }
        if let rowCount = rowCount { env["ROW_COUNT"] = String(rowCount) }
        if let error = error { env["ERROR_TEXT"] = error }
        _ = engine.run("history_add", env: Self.localEnvironment(env), onEvent: { _ in }, onExit: { _, _ in })
    }

    /// Reads the engine's history into `historyEntries`, optionally narrowed to `search`.
    ///
    /// Fire and forget, like the write above, and it keeps the list it already had on a failure:
    /// a read that did not answer is not a reason to blank a panel the user is looking at. Called
    /// when the panel appears, when the search changes, and after a run finishes, which are the
    /// moments the list can have changed.
    ///
    /// The search runs in the engine rather than here. FTS5 is what makes "find the statements
    /// containing these words" answerable at all, and a second implementation over the rows in
    /// memory would be a second answer to the same question.
    func loadHistory(search: String = "") {
        historySearch = search
        historyRead += 1
        let read = historyRead
        var entries: [Event.HistoryEntry]?
        var failure: String?
        var env = ["HISTORY_LIMIT": String(historyLimit)]
        // Left out rather than sent empty when there is nothing to search for: the engine reads a
        // blank as "no search" as well, so this is only about not naming a key with no value.
        if !search.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            env["HISTORY_SEARCH"] = search
        }
        _ = engine.run("history", env: Self.localEnvironment(env), onEvent: { event in
            switch event.event {
            case "history": entries = event.entries ?? []
            case "error": failure = event.message
            default: break
            }
        }, onExit: { status, log in
            // A superseded read says nothing, whichever way it went: its answer describes a search
            // the user has already moved on from, and its failure is not this read's failure.
            guard read == self.historyRead else { return }
            guard status == 0, let entries = entries else {
                self.historyError = failure ?? self.lastLine(of: log)
                    ?? "The engine exited with status \(status)."
                return
            }
            self.historyEntries = entries
            self.historyError = nil
        })
    }

    /// Reads the saved queries into `savedQueries`.
    func loadSavedQueries() {
        var queries: [Event.SavedQuery]?
        var failure: String?
        _ = Engine.current.run("saved_queries", env: Self.localEnvironment(["SAVED_ACTION": "list"]),
                               onEvent: { event in
            switch event.event {
            case "saved_queries": queries = event.queries ?? []
            case "error": failure = event.message
            default: break
            }
        }, onExit: { status, log in
            guard status == 0, let queries = queries else {
                self.savedError = failure ?? self.lastLine(of: log)
                    ?? "The engine exited with status \(status)."
                return
            }
            self.savedQueries = queries
            self.savedError = nil
        })
    }

    /// Saves the tab's statement under `name`, then re-reads the list.
    ///
    /// Answers `false` without running anything when the name or the statement is blank, so the
    /// sheet can refuse an empty name before a command is started. The engine refuses them too,
    /// because a caller that is not this app has no sheet to refuse it in.
    @discardableResult
    func saveQuery(_ tab: QueryTab, named name: String, from source: QuerySource = .all) -> Bool {
        Self.flushEditorsNow()
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        let sql = tab.sql(for: source)
        guard !trimmed.isEmpty,
              !sql.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return false }

        var env: [String: String] = ["SAVED_ACTION": "save", "NAME": trimmed, "SQL": sql]
        // The connection is what the query was written against, and it is worth keeping: the same
        // statement is a different statement against another database.
        if let id = tab.connectionID { env["CONNECTION_ID"] = id.uuidString.lowercased() }
        _ = Engine.current.run("saved_queries", env: Self.localEnvironment(env), onEvent: { event in
            if event.event == "error" { self.savedError = event.message }
        }, onExit: { _, _ in
            self.loadSavedQueries()
        })
        return true
    }

    /// Deletes a saved query, then re-reads the list.
    func deleteSavedQuery(_ id: String) {
        _ = Engine.current.run("saved_queries",
                               env: ["SAVED_ACTION": "delete", "SAVED_ID": id],
                               onEvent: { event in
            if event.event == "error" { self.savedError = event.message }
        }, onExit: { _, _ in
            self.loadSavedQueries()
        })
    }

    /// Puts a statement from the history or the saved list into the tab's editor.
    ///
    /// Replaces the editor's text rather than opening a tab: the list is a way back to something
    /// the user already wrote, and a second tab for the same statement is how a person ends up
    /// with five copies of one query. Undo still brings back what was there.
    func loadIntoEditor(_ sql: String, in tab: QueryTab) {
        tab.sql = sql
        tab.panel = .result
        panelCollapsed = false
    }

    /// The last non-empty line of the engine's log, which is where it puts a failure's reason.
    private func lastLine(of log: String) -> String? {
        log.split(separator: "\n").last.map(String.init)
    }
}
