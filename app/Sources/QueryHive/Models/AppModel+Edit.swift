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

    /// Run a reviewed plan of deletes, updates and inserts in one transaction.
    ///
    /// The plan is the same value the review sheet showed — `WritePlan` is built once and
    /// `payload` is the single encoder of its statements — so this cannot send a statement the
    /// user did not read. The engine verifies each statement's affected-row count and rolls the
    /// whole plan back if one disagrees.
    func applyChanges(_ plan: WritePlan, in tab: QueryTab) {
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
        var message: String?
        var applied = 0
        _ = Engine.current.run("apply_changes", env: env, onEvent: { event in
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
        }, onExit: { status, _ in
            if status == 0 {
                // The plan is in the table; the queue it came from has nothing
                // left to say.
                tab.cellEdits.discard()
                tab.note(.success, "Applied \(pluralized(applied, "change")).")
            } else {
                tab.note(.error, message ?? "The change plan failed and was rolled back.")
            }
        })
    }
}
