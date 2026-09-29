import Foundation
import Observation

/// What the SQL editor does with what it is given: the switches the Editor pane owns.
///
/// A store of its own rather than more fields on `ThemeStore`. These are not appearance — they
/// change how the editor lays text out and what it draws over it — and `ThemeStore` exists so that
/// `Tone` can read a palette from a static member. Nothing here is read that way, so it does not
/// belong there.
///
/// Every default is what the editor already did before these existed, so a fresh install and an
/// upgraded one behave the same: line numbers, the caret's line and its statement highlighted,
/// folding and wrapping on, four-space tabs, and invisible characters off.
@Observable
final class EditorPreferences {
    static let shared = EditorPreferences()

    private static let showLineNumbersKey = "editorShowLineNumbers"
    private static let highlightLineKey = "editorHighlightCurrentLine"
    private static let highlightStatementKey = "editorHighlightCurrentStatement"
    private static let wordWrapKey = "editorWordWrap"
    private static let codeFoldingKey = "editorCodeFolding"
    private static let showInvisiblesKey = "editorShowInvisibles"
    private static let autoUppercaseKey = "editorAutoUppercaseKeywords"
    private static let runButtonKey = "editorRunButtonPerStatement"
    private static let tabWidthKey = "editorTabWidth"

    /// The store these switches are read from and written to.
    ///
    /// A parameter rather than `.standard` written in, so a test can hand in a scratch suite: this
    /// is a singleton whose setters persist, and a test that wrote to the real preferences would be
    /// a test that changes what the user sees.
    private let defaults: UserDefaults

    private var storedShowLineNumbers: Bool
    private var storedHighlightLine: Bool
    private var storedHighlightStatement: Bool
    private var storedWordWrap: Bool
    private var storedCodeFolding: Bool
    private var storedShowInvisibles: Bool
    private var storedAutoUppercase: Bool
    private var storedRunButton: Bool
    private var storedTabWidth: Int

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
        storedShowLineNumbers = Self.on(defaults, Self.showLineNumbersKey, default: true)
        storedHighlightLine = Self.on(defaults, Self.highlightLineKey, default: true)
        storedHighlightStatement = Self.on(defaults, Self.highlightStatementKey, default: true)
        storedWordWrap = Self.on(defaults, Self.wordWrapKey, default: true)
        storedCodeFolding = Self.on(defaults, Self.codeFoldingKey, default: true)
        storedShowInvisibles = Self.on(defaults, Self.showInvisiblesKey, default: false)
        storedAutoUppercase = Self.on(defaults, Self.autoUppercaseKey, default: false)
        storedRunButton = Self.on(defaults, Self.runButtonKey, default: true)
        let tabWidth = defaults.integer(forKey: Self.tabWidthKey)
        storedTabWidth = tabWidth > 0 ? tabWidth : 4
    }

    /// Whether a switch nobody has touched is on.
    ///
    /// `object(forKey:)` rather than `bool(forKey:)`: the latter answers `false` for a key that was
    /// never written, which would make the first launch the one launch that hid the line numbers.
    private static func on(_ defaults: UserDefaults, _ key: String, default fallback: Bool) -> Bool {
        defaults.object(forKey: key) as? Bool ?? fallback
    }

    /// The gutter. Off leaves the text its full width and the editor without a margin.
    var showLineNumbers: Bool {
        get { storedShowLineNumbers }
        set { storedShowLineNumbers = newValue; persist() }
    }

    /// A faint band behind the line the caret is on, which is how an eye finds the caret after a
    /// scroll without hunting for the insertion point.
    var highlightCurrentLine: Bool {
        get { storedHighlightLine }
        set { storedHighlightLine = newValue; persist() }
    }

    /// The same, for the whole statement the caret is in. This is the one that matters in a file of
    /// several statements: it says which one Run will take.
    var highlightCurrentStatement: Bool {
        get { storedHighlightStatement }
        set { storedHighlightStatement = newValue; persist() }
    }

    /// Whether a long line wraps or runs off the right edge behind a horizontal scroller.
    var wordWrap: Bool {
        get { storedWordWrap }
        set { storedWordWrap = newValue; persist() }
    }

    /// Whether the gutter offers fold markers and collapsed bodies.
    var codeFolding: Bool {
        get { storedCodeFolding }
        set { storedCodeFolding = newValue; persist() }
    }

    /// Spaces and tabs drawn as marks, so an indent of three spaces and one that is a tab stop stop
    /// looking the same.
    var showInvisibles: Bool {
        get { storedShowInvisibles }
        set { storedShowInvisibles = newValue; persist() }
    }

    /// Whether a keyword becomes upper case as soon as it is finished. Off by default: it rewrites
    /// what was typed, and a setting that does that should be chosen rather than discovered.
    var autoUppercaseKeywords: Bool {
        get { storedAutoUppercase }
        set { storedAutoUppercase = newValue; persist() }
    }

    /// Whether the gutter offers a run marker beside each statement. On by default: it is the one
    /// control here that adds something rather than changing what is drawn.
    var runButtonPerStatement: Bool {
        get { storedRunButton }
        set { storedRunButton = newValue; persist() }
    }

    /// How wide a tab is, in spaces. Clamped, because the value comes from a control and a zero
    /// would put every tab stop on top of the last.
    var tabWidth: Int {
        get { storedTabWidth }
        set { storedTabWidth = min(max(newValue, 1), 8); persist() }
    }

    private func persist() {
        defaults.set(storedShowLineNumbers, forKey: Self.showLineNumbersKey)
        defaults.set(storedHighlightLine, forKey: Self.highlightLineKey)
        defaults.set(storedHighlightStatement, forKey: Self.highlightStatementKey)
        defaults.set(storedWordWrap, forKey: Self.wordWrapKey)
        defaults.set(storedCodeFolding, forKey: Self.codeFoldingKey)
        defaults.set(storedShowInvisibles, forKey: Self.showInvisiblesKey)
        defaults.set(storedAutoUppercase, forKey: Self.autoUppercaseKey)
        defaults.set(storedRunButton, forKey: Self.runButtonKey)
        defaults.set(storedTabWidth, forKey: Self.tabWidthKey)
    }
}

/// The switches as one value, which is what makes them reach the editor.
///
/// `SQLEditor` is an `NSViewRepresentable`: SwiftUI re-applies it when the value it was given
/// changes. Passing the store itself would not do that, because the reference never changes, so the
/// layout is passed as a value and the host reads `current` while its body runs. That read is also
/// what registers the dependency, so a switch flipped in Settings re-applies the editor.
struct EditorLayout: Equatable {
    var showLineNumbers: Bool
    var highlightCurrentLine: Bool
    var highlightCurrentStatement: Bool
    var wordWrap: Bool
    var codeFolding: Bool
    var showInvisibles: Bool
    var autoUppercaseKeywords: Bool
    var runButtonPerStatement: Bool
    var tabWidth: Int

    /// What an untouched install has, for anything that needs a layout without a store behind it.
    static let standard = EditorLayout(showLineNumbers: true, highlightCurrentLine: true,
                                       highlightCurrentStatement: true, wordWrap: true,
                                       codeFolding: true, showInvisibles: false,
                                       autoUppercaseKeywords: false, runButtonPerStatement: true,
                                       tabWidth: 4)

    static var current: EditorLayout {
        let prefs = EditorPreferences.shared
        return EditorLayout(showLineNumbers: prefs.showLineNumbers,
                            highlightCurrentLine: prefs.highlightCurrentLine,
                            highlightCurrentStatement: prefs.highlightCurrentStatement,
                            wordWrap: prefs.wordWrap,
                            codeFolding: prefs.codeFolding,
                            showInvisibles: prefs.showInvisibles,
                            autoUppercaseKeywords: prefs.autoUppercaseKeywords,
                            runButtonPerStatement: prefs.runButtonPerStatement,
                            tabWidth: prefs.tabWidth)
    }
}
