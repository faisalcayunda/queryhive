import AppKit

/// A key press as the grid reads it: which key, and which modifiers were down. No `NSEvent`, so the
/// whole map below is a table that a test can walk (blueprint W10 §3.1).
struct GridKey: Equatable {
    enum Code: Equatable {
        case left, right, up, down
        case pageUp, pageDown, home, end
        case returnKey, escape, tab, space
        case deleteBackward, deleteForward
    }

    var code: Code
    var shift = false
    var command = false
    var option = false
    var control = false

    init(_ code: Code, shift: Bool = false, command: Bool = false, option: Bool = false,
         control: Bool = false) {
        self.code = code
        self.shift = shift
        self.command = command
        self.option = option
        self.control = control
    }

    /// The key an event is, by its hardware key code so the layout does not matter. `nil` for every
    /// key the grid has no use for, which is what lets those fall through to AppKit.
    init?(event: NSEvent) {
        guard let code = Self.codes[event.keyCode] else { return nil }
        let flags = event.modifierFlags
        self.init(code, shift: flags.contains(.shift), command: flags.contains(.command),
                  option: flags.contains(.option), control: flags.contains(.control))
    }

    private static let codes: [UInt16: Code] = [
        123: .left, 124: .right, 125: .down, 126: .up,
        116: .pageUp, 121: .pageDown, 115: .home, 119: .end,
        36: .returnKey, 76: .returnKey, 53: .escape, 48: .tab, 49: .space,
        51: .deleteBackward, 117: .deleteForward,
    ]

    /// An arrow key, which the grid always swallows: a key it does not map must still not move the
    /// row selection of the table underneath, which the grid does not draw.
    var isArrow: Bool { [.left, .right, .up, .down].contains(code) }
}

/// What the map needs to know that is not in the key.
struct GridKeyState: Equatable {
    var hasCursor: Bool
    /// There is a selection (a single cell counts).
    var hasSelection: Bool
    /// The selection is more than one cell.
    var selectionIsBlock: Bool
    var peekOpen: Bool
    var editing: Bool
    /// Whether ⌫ may mark the selected rows for deletion. False until W10-T3 draws them.
    var canDeleteRows = false
}

/// What the grid does for a key.
enum GridKeyAction: Equatable {
    case move(GridMotion, extending: Bool)
    /// Return: the same thing a double-click does on the cursor's cell.
    case activate
    case togglePeek
    case cancelEdit
    case closePeek
    /// Esc on a block: the block shrinks to the cursor's cell.
    case collapseSelection
    /// Esc on one cell: nothing is selected, and the cursor stays where it is.
    case clearSelection
    case deleteRows
}

/// The grid's keyboard, as one function (blueprint W10 §3.1).
///
/// Anything not listed is `nil` and goes to AppKit, which is how ⌘C, ⌘A, ⌘Z and the menu's own
/// keys reach the responder chain untouched. Letters and digits are nil on purpose: typing to edit
/// is out of scope, and a key that quietly started an edit would be one a person never asked for.
enum GridKeyMap {
    static func action(for key: GridKey, in state: GridKeyState) -> GridKeyAction? {
        // While a cell is open for editing, the field has the keyboard; the one key the grid still
        // answers is Esc, which abandons the edit.
        if state.editing { return key.code == .escape ? .cancelEdit : nil }

        let plain = !key.option && !key.control
        let bare = plain && !key.command && !key.shift
        switch key.code {
        case .left, .right, .up, .down:
            guard plain else { return nil }
            return .move(arrow(key), extending: key.shift)
        case .pageUp, .pageDown:
            guard plain, !key.command else { return nil }
            return .move(.page(down: key.code == .pageDown), extending: key.shift)
        case .home, .end:
            guard plain else { return nil }
            let start = key.code == .home
            let motion: GridMotion = key.command ? (start ? .gridStart : .gridEnd)
                                                 : (start ? .rowStart : .rowEnd)
            return .move(motion, extending: key.shift)
        case .tab:
            guard plain, !key.command else { return nil }
            return .move(key.shift ? .previous : .next, extending: false)
        case .returnKey:
            return bare && state.hasCursor ? .activate : nil
        case .space:
            return bare && state.hasCursor ? .togglePeek : nil
        case .escape:
            guard bare else { return nil }
            // One step per tap: the peek first, then the block, then the selection.
            if state.peekOpen { return .closePeek }
            if state.selectionIsBlock { return .collapseSelection }
            if state.hasSelection { return .clearSelection }
            return nil
        case .deleteBackward, .deleteForward:
            return bare && state.canDeleteRows && state.hasSelection ? .deleteRows : nil
        }
    }

    /// An arrow as a motion: one cell, or with ⌘ all the way to the edge.
    private static func arrow(_ key: GridKey) -> GridMotion {
        if key.command {
            switch key.code {
            case .left: return .toEdge(.left)
            case .right: return .toEdge(.right)
            case .up: return .toEdge(.top)
            default: return .toEdge(.bottom)
            }
        }
        switch key.code {
        case .left: return .step(rows: 0, columns: -1)
        case .right: return .step(rows: 0, columns: 1)
        case .up: return .step(rows: -1, columns: 0)
        default: return .step(rows: 1, columns: 0)
        }
    }
}

/// Where to scroll so a cell is on screen, pure so the arithmetic is checked without a scroll view.
enum GridScrollMath {
    /// The clip view's new origin: where it is now when the cell is already inside what the insets
    /// leave visible, otherwise the least movement that brings it in. A cell wider than the view
    /// keeps its left edge on screen, which is the part a person reads first.
    static func origin(revealing cell: CGRect, in clip: CGRect, insets: NSEdgeInsets) -> CGPoint {
        var origin = clip.origin
        let top = clip.minY + insets.top, bottom = clip.maxY - insets.bottom
        let left = clip.minX + insets.left, right = clip.maxX - insets.right
        if cell.minY < top {
            origin.y = cell.minY - insets.top
        } else if cell.maxY > bottom {
            origin.y = cell.maxY - (clip.height - insets.bottom)
        }
        if cell.minX < left {
            origin.x = cell.minX - insets.left
        } else if cell.maxX > right {
            origin.x = min(cell.maxX - (clip.width - insets.right), cell.minX - insets.left)
        }
        return origin
    }
}
