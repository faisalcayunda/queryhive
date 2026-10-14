import Foundation
import Observation

/// Work that is built and tested but not yet proven against a real server, behind a switch the owner
/// turns on (ADR-0046 decision 13).
///
/// Every switch here is **off** by default, and off means the feature does not exist: no control
/// shows it, and no run carries a setting it would add. A store of its own, with the same
/// injectable `UserDefaults` and non-persisting `pin` as `DataPreferences`, because a snapshot that
/// draws the feature must not switch it on for the app the owner has installed (the dev build shares
/// its bundle id), and a test must not write to anyone's preferences.
@Observable
final class ExperimentalFeatures {
    static let shared = ExperimentalFeatures()

    static let saveTableAcrossConnectionsKey = "experimentalSaveTableAcrossConnections"

    private let defaults: UserDefaults
    /// Raised by `pin`: the values are for this process only and nothing is written.
    private var isPinned = false
    private var storedSaveTableAcrossConnections: Bool

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        storedSaveTableAcrossConnections = defaults.object(forKey: Self.saveTableAcrossConnectionsKey)
            as? Bool ?? false
    }

    /// Save to Table may name another saved connection as the target (W13-T20). Off until one run
    /// against real servers has passed (the plan's step 12).
    var saveTableAcrossConnections: Bool {
        get { storedSaveTableAcrossConnections }
        set {
            storedSaveTableAcrossConnections = newValue
            guard !isPinned else { return }
            defaults.set(newValue, forKey: Self.saveTableAcrossConnectionsKey)
        }
    }

    /// A render's way to draw a state: the value holds for this process and is never written.
    func pin(saveTableAcrossConnections: Bool) {
        isPinned = true
        storedSaveTableAcrossConnections = saveTableAcrossConnections
    }
}
