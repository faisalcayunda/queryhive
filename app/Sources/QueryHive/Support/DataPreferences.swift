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

        /// The row's height once the cell font is `fontSize` rather than the standard one: two
        /// points for each point of font above the standard, so the standard size is exactly
        /// `points` and a larger font never overfills its row.
        ///
        /// Below the standard the row stays where it is. A line of text loses about one point per
        /// point of font, not two, so shrinking the row at the same rate would push Compact's box
        /// further past its row than it already goes at the standard size; the denser rows are the
        /// Compact preset's job.
        func points(atFontSize fontSize: Int) -> CGFloat {
            points + 2 * CGFloat(max(0, fontSize - DataPreferences.standardGridFontSize))
        }
    }

    /// What the cells were drawn in before the size was a setting, which is why it is the default.
    static let standardGridFontSize = 12
    /// The smallest and largest cell font the grid offers, in points.
    static let gridFontSizeRange = 11...16

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
    private static let autoInspectorKey = "gridAutoShowInspector"
    private static let fontSizeKey = "gridFontSize"

    /// The store these are read from and written to. A parameter so a test can hand in a scratch
    /// suite: this is a singleton whose setters persist, and a test that wrote to the real
    /// preferences would change what the user sees.
    private let defaults: UserDefaults

    /// Raised by `pin`, which is what a snapshot render uses instead of the public setters.
    ///
    /// The setters persist, so a caller that only wants to *draw* a state has to be able to hand its
    /// values to something that never writes. `ThemeStore` carries the same flag for the same
    /// reason: a `--snapshot` run once wrote the appearance it was reviewing straight into the
    /// user's preferences, which is the exact thing a review must not do.
    private var isPinned = false

    private var storedRowHeight: RowHeight
    private var storedNullDisplay: String
    private var storedAlternateRows: Bool
    private var storedRowNumbers: Bool
    private var storedViewerMode: ViewerMode
    private var storedFirstSort: GridSort.Direction
    private var storedAutoInspector: Bool
    private var storedFontSize: Int

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
        // The one switch on this pane that changes the grid's shape rather than its drawing, and the
        // only one that defaults off: an untouched install gets the whole pane for the grid, which
        // is what it has always had.
        storedAutoInspector = Self.on(defaults, Self.autoInspectorKey, default: false)
        // An absent key reads 0, which is out of the range, so it clamps to the smallest size
        // rather than the default; the explicit check is what keeps an upgrade at 12.
        let fontSize = defaults.integer(forKey: Self.fontSizeKey)
        storedFontSize = fontSize == 0 ? Self.standardGridFontSize : Self.clampedFontSize(fontSize)
    }

    static func clampedFontSize(_ size: Int) -> Int {
        min(max(size, gridFontSizeRange.lowerBound), gridFontSizeRange.upperBound)
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

    /// Whether the value reader stands beside the grid whenever a cell is chosen.
    ///
    /// The reader is otherwise a popover on the cell, which is the right shape for looking at one
    /// value and going back to the rows. This is for reading against the grid instead: it publishes
    /// the choice to a panel, so a click does not have to be spent dismissing a popover before the
    /// next cell can be chosen.
    ///
    /// Off by default, because it is the one switch here that changes the pane's shape rather than
    /// the grid's drawing: on, the result pane is a grid and a reader side by side; off, the grid
    /// has the whole width, which is what it has always had.
    var autoShowInspector: Bool {
        get { storedAutoInspector }
        set { storedAutoInspector = newValue; persist() }
    }

    /// Which way the **first** click on a column header sorts. The second click is always the other
    /// way and the third clears it, so this only decides where the cycle starts.
    var firstSortDirection: GridSort.Direction {
        get { storedFirstSort }
        set { storedFirstSort = newValue; persist() }
    }

    /// The cell and header-label font, in points. Clamped, because the value comes from a control.
    /// The row-number gutter and the type chip keep their own sizes.
    var gridFontSize: Int {
        get { storedFontSize }
        set { storedFontSize = Self.clampedFontSize(newValue); persist() }
    }

    /// The row's height in points: the preset's, moved by the font size.
    var rowPoints: CGFloat { rowHeight.points(atFontSize: gridFontSize) }

    private func persist() {
        // Pinned means a render: the values are for this process only and the file is left alone.
        guard !isPinned else { return }
        defaults.set(storedRowHeight.rawValue, forKey: Self.rowHeightKey)
        defaults.set(storedNullDisplay, forKey: Self.nullDisplayKey)
        defaults.set(storedAlternateRows, forKey: Self.alternateRowsKey)
        defaults.set(storedRowNumbers, forKey: Self.rowNumbersKey)
        defaults.set(storedViewerMode.rawValue, forKey: Self.viewerModeKey)
        defaults.set(storedFirstSort.rawValue, forKey: Self.firstSortKey)
        defaults.set(storedAutoInspector, forKey: Self.autoInspectorKey)
        defaults.set(storedFontSize, forKey: Self.fontSizeKey)
    }

    /// Set what the grid should draw without writing any of it down.
    ///
    /// Only the three a snapshot scene needs. `isPinned` is raised first, before anything is
    /// assigned: that ordering is the whole point, because it is what keeps the value out of the
    /// file.
    func pin(rowHeight: RowHeight? = nil, showRowNumbers: Bool? = nil,
             autoShowInspector: Bool? = nil) {
        isPinned = true
        if let rowHeight { storedRowHeight = rowHeight }
        if let showRowNumbers { storedRowNumbers = showRowNumbers }
        if let autoShowInspector { storedAutoInspector = autoShowInspector }
    }
}
