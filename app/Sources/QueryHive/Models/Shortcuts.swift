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
        case .saveFile: "Save SQL File"
        case .exportData: "Export Data"
        case .toggleResultPanel: "Toggle Result Panel"
        case .commentLine: "Comment / Uncomment"
        case .format: "Format SQL"
        case .closeTab: "Close Tab"
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
        switch self {
        case .dbeaver: Self.dbeaverTable[action]
        case .queryhive: Self.queryhiveTable[action]
        }
    }

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
        .stop: Shortcut(".", .command),
        .newQuery: Shortcut("t", .command),
        .openFile: Shortcut("o", .command),
        .saveFile: Shortcut("s", .command),
        .exportData: Shortcut("e", .command),
        .closeTab: Shortcut("w", .command),
        .commentLine: Shortcut("/", .command),
    ]
}
