import AppKit
import SwiftUI

extension AppModel {
    /// The key this tab's presets are filed under, or `nil` for a query with no table — where a
    /// preset is tab-local for the reasons `FilterPresetStore` states.
    func presetIdentity(for tab: QueryTab) -> String? {
        FilterPresetStore.identity(connection: connection(for: tab)?.id, table: tab.sourceTable)
    }

    func filterPresets(for tab: QueryTab) -> [FilterPreset] {
        guard let identity = presetIdentity(for: tab) else { return tab.localPresets }
        return filterPresets[identity] ?? []
    }

    /// Save the tab's current filters under a name, replacing a preset of the same name. Returns
    /// `false` when there is nothing to save (no filters, or a blank name).
    @discardableResult
    func saveFilterPreset(named name: String, in tab: QueryTab) -> Bool {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let preset = tab.currentFilterPreset(named: trimmed) else {
            return false
        }
        if let identity = presetIdentity(for: tab) {
            var list = filterPresets[identity] ?? []
            list.removeAll { $0.name == trimmed }
            list.append(preset)
            filterPresets[identity] = list
            FilterPresetStore.save(filterPresets)
        } else {
            tab.localPresets.removeAll { $0.name == trimmed }
            tab.localPresets.append(preset)
        }
        return true
    }

    func deleteFilterPreset(named name: String, in tab: QueryTab) {
        if let identity = presetIdentity(for: tab) {
            var list = filterPresets[identity] ?? []
            list.removeAll { $0.name == name }
            filterPresets[identity] = list.isEmpty ? nil : list
            FilterPresetStore.save(filterPresets)
        } else {
            tab.localPresets.removeAll { $0.name == name }
        }
    }

    /// A server round trip re-runs the query and drops staged edits, so it is
    /// refused while any are queued: silent loss is worse than a refusal.
    static let stagedEditsMessage = "Save or discard your changes first."

    func serverActionBlockedByEdits(_ tab: QueryTab) -> String? {
        tab.cellEdits.isEmpty ? nil : Self.stagedEditsMessage
    }

    // MARK: Rows (blueprint w10 §5.2)

    /// Why the grid cannot add or delete rows right now, or `nil` when it can. One answer for the
    /// footer's buttons, the menu, the keyboard and the gestures, so they cannot disagree.
    ///
    /// Safe Mode `read_only` refuses the write the rows would become; `no_ddl` and the rest do not,
    /// because an INSERT and a DELETE are DML.
    func rowEditBlockedReason(for tab: QueryTab) -> String? {
        if tab.applying { return QueryTab.applyingMessage }
        if tab.rowsStaleAfterApply { return QueryTab.staleAfterApplyMessage }
        if tab.previewing || tab.viewBusy { return QueryTab.viewBusyMessage }
        if tab.sourceTable == nil { return "Open a table to add rows" }
        if connection(for: tab)?.safeMode == .readOnly { return "This connection is read-only" }
        if tab.preview == nil || tab.showingPlan { return "Run the table first" }
        return nil
    }

    /// Add one empty row at the end, put the cursor on its first cell, and make it one undo step.
    func addRow(in tab: QueryTab) {
        if let reason = rowEditBlockedReason(for: tab) { refuse(reason, in: tab); return }
        let before = tab.cellEdits
        tab.cellEdits.insertRow()
        tab.registerGridEdit(before, actionName: "Add Row")
        let position = CellPos(row: tab.rowSpace.count - 1, column: 0)
        tab.selectCells(anchor: position, focus: position)
        Announcer.post("Row added. \(pluralized(tab.cellEdits.count, "pending change")).")
    }

    /// Mark the selected fetched rows for deletion and take the selected added rows back out. One
    /// undo step, and an added row is never "deleted": it was never in the table.
    func deleteRows(in tab: QueryTab) {
        guard let selection = tab.cellSelection else { return }
        if let reason = rowEditBlockedReason(for: tab) { refuse(reason, in: tab); return }
        guard !tab.exceedsCellCeiling(selection.cellCount) else { return }
        let parts = tab.rowSpace.split(selection.top...selection.bottom)
        let before = tab.cellEdits
        if let fetched = parts.fetched { tab.cellEdits.deleteRows(fetched) }
        for (_, id) in parts.inserted { tab.cellEdits.removeInserted(row: id) }
        let rows = (parts.fetched?.count ?? 0) + parts.inserted.count
        tab.registerGridEdit(before, actionName: rows == 1 ? "Delete Row" : "Delete Rows")
        Announcer.post("\(pluralized(rows, "row")) deleted. \(pluralized(tab.cellEdits.count, "pending change")).")
    }

    /// Let go of the selected rows that are marked for deletion.
    func restoreRows(in tab: QueryTab) {
        guard let selection = tab.cellSelection else { return }
        if let reason = rowEditBlockedReason(for: tab) { refuse(reason, in: tab); return }
        guard let fetched = tab.rowSpace.split(selection.top...selection.bottom).fetched else { return }
        let before = tab.cellEdits
        for row in fetched where before.isDeleted(row) { tab.cellEdits.restoreRow(row) }
        tab.registerGridEdit(before, actionName: fetched.count == 1 ? "Restore Row" : "Restore Rows")
        if before != tab.cellEdits { Announcer.post("Row restored.") }
    }

    private func refuse(_ reason: String, in tab: QueryTab) {
        tab.note(.warning, reason)
        Announcer.post(reason)
    }

    // MARK: Save (⌘S, blueprint w10 D-6)

    /// Whether ⌘S has something to save: the selected tab holds staged changes and nothing is
    /// replacing its rows or applying them. Read from observed state, not from the focus, so the
    /// menu item is right the moment an edit is staged (the focus is asked, never observed).
    var canSaveFocused: Bool {
        guard let tab = selectedTab else { return false }
        return (!tab.cellEdits.isEmpty || tab.hasOpenCellEdit) && !tab.viewBusy && !tab.applying
    }

    /// ⌘S: open the review of the selected tab's staged changes, which is what saving means for a
    /// grid. The editor's own file save is W12-T2's; until it lands there is nothing else for the
    /// key to mean, so it does not look at the focus.
    func saveFocused() {
        guard canSaveFocused, let tab = selectedTab else { return }
        // What is typed in the open editor is staged first, or the review would not show it.
        tab.endCellEdit()
        guard !tab.cellEdits.isEmpty else { return }
        tab.panel = .result
        panelCollapsed = false
        tab.reviewRequested = true
    }

    /// Run a reviewed plan of deletes, updates and inserts in one transaction.
    ///
    /// The plan is the same value the review sheet showed — `WritePlan` is built once and
    /// `payload` is the single encoder of its statements — so this cannot send a statement the
    /// user did not read. The engine verifies each statement's affected-row count and rolls the
    /// whole plan back if one disagrees.
    ///
    /// One apply per tab at a time (PF-2): `tab.applying` refuses a second, the review sheet, ⌘S and
    /// every new edit until the engine has answered.
    func applyChanges(_ plan: WritePlan, in tab: QueryTab) {
        guard !tab.applying else {
            tab.note(.warning, "The changes are already being applied.")
            return
        }
        // A plan built before a view was replaced names rows of the view that has gone (D-27).
        guard !tab.viewBusy else {
            tab.note(.warning, QueryTab.viewBusyMessage)
            tab.panel = .log
            return
        }
        // The plan says which rows it could not write, and it must not be a silent
        // partial save: those lines go to the log before anything runs.
        for warning in plan.warnings { tab.note(.warning, warning) }
        guard !plan.isEmpty, let connection = connection(for: tab) else {
            if !plan.warnings.isEmpty { tab.panel = .log }
            return
        }
        let env: [String: String]
        do {
            var built = try connectionEnvironment(connection)
            built["CHANGES"] = plan.payload
            built["IN_TRANSACTION"] = "true"
            env = built
        } catch {
            let message = (error as? EngineLaunchError)?.message ?? error.localizedDescription
            tab.note(.error, message)
            tab.panel = .log
            return
        }
        tab.panel = .log
        tab.note(.info, "Applying \(pluralized(plan.statements.count, "change")) to "
                + "\(plan.table ?? "the table")…")
        // What the plan was built from: the success clears exactly this, and nothing typed since.
        let snapshot = tab.cellEdits
        tab.applying = true
        var message: String?
        var applied = 0
        _ = engine.run("apply_changes", env: env, onEvent: { event in
            switch event.event {
            case "progress":
                applied = event.rows ?? applied
            case "done":
                applied = event.applied ?? applied
            case "error":
                message = event.message
            default:
                break
            }
        }, onExit: { [weak self] status, _ in
            self?.finishApply(tab, succeeded: status == 0, applied: applied, failure: message,
                              snapshot: snapshot)
        })
    }

    /// What an apply's end does to the tab. A method of its own so it is tested without an engine.
    ///
    /// On success the plan is in the table, so the queue it came from is taken out of the tab — but
    /// only the cells that still equal the snapshot (`CellEdits.removing`), because an edit typed
    /// while the engine worked is not in the table. Then the grid must stop showing the values from
    /// before (DBX-27): the base query is run again, so an update shows its new value, an inserted
    /// row is a fetched one and a deleted row is gone, and a second edit of the same row matches
    /// what the table holds now instead of what it held then. The log says which was done.
    func finishApply(_ tab: QueryTab, succeeded: Bool, applied: Int, failure: String?,
                     snapshot: CellEdits) {
        tab.applying = false
        guard succeeded else {
            tab.note(.error, failure ?? "The change plan failed and was rolled back.")
            return
        }
        tab.note(.success, "Applied \(pluralized(applied, "change")).")
        // An added row the plan wrote is in the table, so text typed into it while the engine ran
        // has no row to go to: it is dropped (not queued as a second INSERT), and the log says so.
        let written = Dictionary(uniqueKeysWithValues: snapshot.inserted.map { ($0.id, $0.values) })
        let lost = tab.cellEdits.inserted.filter { row in written[row.id].map { $0 != row.values } ?? false }
        let remaining = tab.cellEdits.removing(snapshot)
        if !lost.isEmpty {
            tab.note(.warning, "\(pluralized(lost.count, "row")) added by this apply "
                     + "\(lost.count == 1 ? "was" : "were") changed while it ran; those later values were "
                     + "not written. Edit the row again once the grid has refreshed.")
        }
        tab.cellEdits = remaining
        // A step in this history would put a cell that is now in the table back into the queue.
        tab.clearEditUndo()
        tab.reconcileSelectionWithRowSpace()
        guard remaining.isEmpty else {
            tab.note(.warning, "\(pluralized(remaining.count, "change")) made while the apply ran "
                     + "\(remaining.count == 1 ? "is" : "are") still staged, so the grid was not "
                     + "refreshed and still shows the values from before. Review them, then run the "
                     + "query again.")
            return
        }
        refreshAfterApply(tab)
    }

    private func refreshAfterApply(_ tab: QueryTab) {
        let narrowed = tab.activeSort != nil || tab.hasGridSearch || !tab.columnFilters.isEmpty
        if tab.previewBaseSQL?.isEmpty == false, connection(for: tab) != nil {
            rerunBaseSQL(tab)
            if tab.previewing {
                tab.note(.info, "Re-running the base query to show the committed values"
                         + (narrowed ? "; the sort, search and filters were reset." : "."))
                return
            }
        }
        tab.rowsStaleAfterApply = true
        tab.note(.warning, "The query could not be run again, so the grid still shows the values "
                 + "from before and is read-only until you run it. The committed values were not "
                 + "overlaid.")
    }
}
