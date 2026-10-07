import SwiftUI

/// Everything in the app a key can be bound to.
///
/// Deliberately small: an action belongs here only when the app genuinely performs it, so a scheme
/// can never bind a key to something that does not exist.
enum ShortcutAction: String, CaseIterable, Identifiable {
    case run
    case runScript
    case explain
    case countRows
    case stop
    case newQuery
    case openFile
    case saveFile
    case exportData
    case toggleResultPanel
    case commentLine
    case format
    case closeTab
    case openQuickly
    case revealOutput
    case toggleSidebar
    case focusSidebar
    case focusEditor
    case focusResults
    case nextTab
    case previousTab
    case peekCell
    case toggleRecord
    case fontBigger
    case fontSmaller
    case fontReset
    case addRow

    /// Whether a scheme chooses the key or the macOS convention applies in every scheme.
    enum Scope { case scheme, platform }

    var scope: Scope {
        switch self {
        case .run, .runScript, .explain, .countRows, .newQuery, .openFile, .toggleResultPanel,
             .exportData, .commentLine, .format:
            .scheme
        default:
            .platform
        }
    }

    /// False for an action that has a key reserved but nothing to run yet, so no menu item exists
    /// and nothing may claim to bind it. `saveFile` is Save in the grid since W10-T3 (it reviews the
    /// staged changes) and gains the editor's file save in W12-T2; the other two wait for W12-T2.
    var isAvailable: Bool {
        switch self {
        case .commentLine, .format: false
        default: true
        }
    }

    var id: String { rawValue }

    var title: String {
        switch self {
        case .run: "Run"
        case .runScript: "Run Script"
        case .explain: "Explain"
        case .countRows: "Count Rows"
        case .stop: "Stop"
        case .newQuery: "New Query"
        case .openFile: "Open SQL File"
        case .saveFile: "Save"
        case .exportData: "Export Data"
        case .toggleResultPanel: "Toggle Result Panel"
        case .commentLine: "Comment / Uncomment"
        case .format: "Format SQL"
        case .closeTab: "Close Tab"
        case .openQuickly: "Open Quickly"
        case .revealOutput: "Reveal Output in Finder"
        case .toggleSidebar: "Toggle Sidebar"
        case .focusSidebar: "Focus Sidebar"
        case .focusEditor: "Focus Editor"
        case .focusResults: "Focus Results"
        case .nextTab: "Show Next Tab"
        case .previousTab: "Show Previous Tab"
        case .peekCell: "Peek Cell"
        case .toggleRecord: "Toggle Record View"
        case .fontBigger: "Increase Font Size"
        case .fontSmaller: "Decrease Font Size"
        case .fontReset: "Actual Size"
        case .addRow: "Add Row"
        }
    }
}

/// One key plus its modifiers.
struct Shortcut: Equatable {
    let key: KeyEquivalent
    let modifiers: EventModifiers

    init(_ key: KeyEquivalent, _ modifiers: EventModifiers) {
        self.key = key
        self.modifiers = modifiers
    }

    var keyboard: KeyboardShortcut { KeyboardShortcut(key, modifiers: modifiers) }

    /// `⌘⇧E` style, in the order macOS writes modifier glyphs.
    var display: String {
        var text = ""
        if modifiers.contains(.control) { text += "⌃" }
        if modifiers.contains(.option) { text += "⌥" }
        if modifiers.contains(.shift) { text += "⇧" }
        if modifiers.contains(.command) { text += "⌘" }
        let raw = key.character
        switch raw {
        case "\r": text += "↩"
        case "\u{7F}": text += "⌫"
        case "\u{1B}": text += "⎋"
        case " ": text += "Space"
        default: text += String(raw).uppercased()
        }
        return text
    }
}

/// A named set of bindings, in the spirit of Eclipse's key-binding schemes — which is where
/// DBeaver's own "DBeaver Keyboard Only" scheme comes from.
///
/// Every `dbeaver` entry below is taken from DBeaver's own key-binding declarations, not from
/// memory: the SQL editor's bindings live in
/// `plugins/org.jkiss.dbeaver.ui.editors.sql/plugin.xml` under `<extension point="org.eclipse.ui.bindings">`,
/// and the `cocoa` platform entries are the macOS ones (`COMMAND+Enter` rather than `CTRL+Enter`).
enum ShortcutScheme: String, CaseIterable, Identifiable {
    case dbeaver
    case queryhive

    var id: String { rawValue }

    var title: String {
        switch self {
        case .dbeaver: "DBeaver"
        case .queryhive: "QueryHive"
        }
    }

    var detail: String {
        switch self {
        case .dbeaver:
            "DBeaver's own SQL editor bindings, read from its source. ⌘↩ runs, ⌃⇧E explains."
        case .queryhive:
            "The bindings this app shipped with before schemes existed."
        }
    }

    /// The binding for an action, or `nil` when this scheme leaves it unbound.
    ///
    /// A `nil` is a real answer and the honest one: DBeaver declares several of these commands
    /// without a key of their own — `cancel.query`, `run.count` and `export.data` are declared in
    /// the same file with no `<key>` element — so this scheme does not invent one for them.
    func shortcut(for action: ShortcutAction) -> Shortcut? {
        if action.scope == .platform { return Self.platformTable[action] }
        return switch self {
        case .dbeaver: Self.dbeaverTable[action]
        case .queryhive: Self.queryhiveTable[action]
        }
    }

    /// The macOS and Xcode conventions, the same in both schemes: where DBeaver declares no key
    /// (Stop, Close Tab, Open Quickly) a scheme that left it unbound would leave the default user
    /// with no key for it.
    static let platformTable: [ShortcutAction: Shortcut] = [
        .stop: Shortcut(".", .command),
        .saveFile: Shortcut("s", .command),
        .closeTab: Shortcut("w", .command),
        // ⇧⌘O, one shift away from Open File (⌘O).
        .openQuickly: Shortcut("o", [.command, .shift]),
        .revealOutput: Shortcut("r", [.command, .shift]),
        .toggleSidebar: Shortcut("s", [.control, .command]),
        // Left, middle, bottom; ⌘1…⌘9 already belong to the tabs.
        .focusSidebar: Shortcut("1", [.command, .option]),
        .focusEditor: Shortcut("2", [.command, .option]),
        .focusResults: Shortcut("3", [.command, .option]),
        .nextTab: Shortcut("]", [.command, .shift]),
        .previousTab: Shortcut("[", [.command, .shift]),
        // Space does the same inside the grid; this is the menu's way to it.
        .peekCell: Shortcut("y", .command),
        // Xcode's Inspectors key, and the one W9 reserved for this panel.
        .toggleRecord: Shortcut("i", [.command, .option]),
        // The size of the text in the region that has the keyboard (W10-T7, D-16). ⌘= is the same
        // key as ⌘+ without the shift, which `FontKeyRouter` answers on its own
        // (`EditorFontSizeTests.testEqualsIsAnAliasOfPlusAndNothingElseIs`).
        .fontBigger: Shortcut("+", .command),
        .fontSmaller: Shortcut("-", .command),
        .fontReset: Shortcut("0", .command),
        // The key W10 reserved for adding a row to the grid.
        .addRow: Shortcut("n", [.command, .option]),
    ]

    private static let dbeaverTable: [ShortcutAction: Shortcut] = [
        // COMMAND+Enter on cocoa for ui.editors.sql.run.statement.
        .run: Shortcut(.return, .command),
        // ALT+X for ui.editors.sql.run.script.
        .runScript: Shortcut("x", .option),
        // CTRL+SHIFT+E for ui.editors.sql.run.explain.
        .explain: Shortcut("e", [.control, .shift]),
        // CTRL+] for core.sql.editor.create. DBeaver also binds CTRL+F3 to the same command, but
        // `KeyEquivalent` has no function-key cases, and ] is a binding of the same command from
        // the same declaration rather than a quieter substitute for it.
        .newQuery: Shortcut("]", .control),
        // CTRL+ALT+SHIFT+O for ui.editors.sql.open.file.
        .openFile: Shortcut("o", [.control, .option, .shift]),
        // CTRL+T and CTRL+SHIFT+T for the result panel toggles.
        .toggleResultPanel: Shortcut("t", .control),
        // CTRL+/ for ui.editors.sql.comment.single.
        .commentLine: Shortcut("/", .control),
        // CTRL+SHIFT+F for ui.editors.text.content.format.
        .format: Shortcut("f", [.control, .shift]),
    ]

    private static let queryhiveTable: [ShortcutAction: Shortcut] = [
        .run: Shortcut("r", .command),
        // No `runScript`: the app never bound one, and ⌘⇧R is already Reveal in Finder.
        .explain: Shortcut("e", .command),
        .countRows: Shortcut("k", [.command, .shift]),
        .newQuery: Shortcut("t", .command),
        .openFile: Shortcut("o", .command),
        // ⌘E has run Explain since before schemes existed, so Export moved (⇧⌘E), not Explain.
        .exportData: Shortcut("e", [.command, .shift]),
        // ⇧⌘Y is Xcode's binding for the bottom area.
        .toggleResultPanel: Shortcut("y", [.command, .shift]),
        .commentLine: Shortcut("/", .command),
    ]
}

/// What a Control-Tab style key press asks for. Pure, so it is tested without an `NSEvent`.
enum TabKeyAction: Equatable { case next, previous }

enum TabKeyRouter {
    static let tabKeyCode: UInt16 = 48

    /// ⌃Tab and ⌃⇧Tab alias the menu's ⇧⌘] and ⇧⌘[, which SwiftUI cannot bind a second key to.
    /// A bare Tab (a text field, the editor's indent) never routes.
    static func route(keyCode: UInt16, control: Bool, shift: Bool, command: Bool, option: Bool) -> TabKeyAction? {
        guard keyCode == tabKeyCode, control, !command, !option else { return nil }
        return shift ? .previous : .next
    }
}

/// ⌘= as an alias of Increase Font Size (⌘+). SwiftUI binds one key per menu item, and on a US keyboard
/// ⌘+ is ⇧⌘=, which a person pressing the key they see (=) does not do. Pure, so it is tested without an
/// `NSEvent`.
enum FontKeyRouter {
    static func route(charactersIgnoringModifiers: String?, command: Bool, control: Bool,
                      option: Bool) -> AppModel.FontStep? {
        // Shift is allowed: ⇧⌘= is what ⌘+ is, and the menu cannot be relied on to answer it.
        guard command, !control, !option, charactersIgnoringModifiers == "=" else { return nil }
        return .bigger
    }
}

/// Keys a modal sheet binds for itself, apart from the menu's schemes.
///
/// A sheet is the key window while it is up, so its own key equivalents are matched before the main
/// menu's: ⌘↩ tests the connection there even in the DBeaver scheme, where the same chord runs a
/// statement in the editor behind it. That is why this is a table of its own and not an entry in
/// `ShortcutAction` (which would put it into the duplicate check against the menu), and why
/// `ShortcutConflictTests` models the sheet as its own scope.
enum SheetShortcut {
    /// Test Connection in the connection sheet. It was ⌘T, which is New Query in the QueryHive
    /// scheme (W11-T3 closes that conflict).
    static let testConnection = Shortcut(.return, .command)
}
