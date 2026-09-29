import Foundation
import Observation

/// How the result grid draws: the switches the Data pane owns.
///
/// A store of its own, for the same reason the editor has one — these are neither appearance nor the
/// editor's business — and every default is what the grid already did, so a fresh install and an
/// upgraded one look the same.
@Observable
final class DataPreferences {
    static let shared = DataPreferences()

    /// How tall a row is. The reference app offers three; the middle one is the height this grid
    /// has always used, which is why it is the default.
    enum RowHeight: String, CaseIterable, Identifiable {
        case compact, normal, tall

        var id: String { rawValue }

        var title: String {
            switch self {
            case .compact: "Compact"
            case .normal: "Normal"
            case .tall: "Tall"
            }
        }

        var points: CGFloat {
            switch self {
            case .compact: 21
            case .normal: 25
            case .tall: 30
            }
        }
    }

    /// What a cell reader opens on.
    ///
    /// `automatic` is what the reader has always done — a JSON value opens on its tree, a byte value
    /// on its dump, anything else on its text — and it is the default because a fixed choice would
    /// take that away for the values it was chosen for.
    enum ViewerMode: String, CaseIterable, Identifiable {
        case automatic, text, tree, hex

        var id: String { rawValue }

        var title: String {
            switch self {
            case .automatic: "Automatic"
            case .text: "Text"
            case .tree: "Tree"
            case .hex: "Hex"
            }
        }
    }

    private static let rowHeightKey = "gridRowHeight"
    private static let nullDisplayKey = "gridNullDisplay"
    private static let alternateRowsKey = "gridAlternateRows"
    private static let rowNumbersKey = "gridRowNumbers"
    private static let viewerModeKey = "jsonViewerMode"
    private static let firstSortKey = "gridFirstSortDirection"

    /// The store these are read from and written to. A parameter so a test can hand in a scratch
    /// suite: this is a singleton whose setters persist, and a test that wrote to the real
    /// preferences would change what the user sees.
    private let defaults: UserDefaults

    private var storedRowHeight: RowHeight
    private var storedNullDisplay: String
    private var storedAlternateRows: Bool
    private var storedRowNumbers: Bool
    private var storedViewerMode: ViewerMode
    private var storedFirstSort: GridSort.Direction

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        storedRowHeight = RowHeight(rawValue: defaults.string(forKey: Self.rowHeightKey) ?? "")
            ?? .normal
        // Empty is a real answer — a cell that shows nothing for a NULL — so the absent key is what
        // falls back, not an empty string.
        storedNullDisplay = defaults.object(forKey: Self.nullDisplayKey) as? String ?? "null"
        storedAlternateRows = Self.on(defaults, Self.alternateRowsKey, default: true)
        storedRowNumbers = Self.on(defaults, Self.rowNumbersKey, default: true)
        storedViewerMode = ViewerMode(rawValue: defaults.string(forKey: Self.viewerModeKey) ?? "")
            ?? .automatic
        storedFirstSort = GridSort.Direction(rawValue: defaults.string(forKey: Self.firstSortKey) ?? "")
            ?? .ascending
    }

    private static func on(_ defaults: UserDefaults, _ key: String, default fallback: Bool) -> Bool {
        defaults.object(forKey: key) as? Bool ?? fallback
    }

    var rowHeight: RowHeight {
        get { storedRowHeight }
        set { storedRowHeight = newValue; persist() }
    }

    /// What a SQL NULL is drawn as. Empty is allowed: some people want the cell blank, and a blank
    /// cell is a choice rather than a missing setting.
    var nullDisplay: String {
        get { storedNullDisplay }
        set { storedNullDisplay = newValue; persist() }
    }

    /// The faint band on every other row. It was always drawn; this is the switch for it.
    var alternateRows: Bool {
        get { storedAlternateRows }
        set { storedAlternateRows = newValue; persist() }
    }

    /// The row-number gutter. Off gives the columns its width.
    var showRowNumbers: Bool {
        get { storedRowNumbers }
        set { storedRowNumbers = newValue; persist() }
    }

    var viewerMode: ViewerMode {
        get { storedViewerMode }
        set { storedViewerMode = newValue; persist() }
    }

    /// Which way the **first** click on a column header sorts. The second click is always the other
    /// way and the third clears it, so this only decides where the cycle starts.
    var firstSortDirection: GridSort.Direction {
        get { storedFirstSort }
        set { storedFirstSort = newValue; persist() }
    }

    private func persist() {
        defaults.set(storedRowHeight.rawValue, forKey: Self.rowHeightKey)
        defaults.set(storedNullDisplay, forKey: Self.nullDisplayKey)
        defaults.set(storedAlternateRows, forKey: Self.alternateRowsKey)
        defaults.set(storedRowNumbers, forKey: Self.rowNumbersKey)
        defaults.set(storedViewerMode.rawValue, forKey: Self.viewerModeKey)
        defaults.set(storedFirstSort.rawValue, forKey: Self.firstSortKey)
    }
}
