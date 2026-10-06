import XCTest

@testable import QueryHive

/// The editor's switches: what an untouched install gets, that a change is written down, and that
/// the value handed to the editor follows the store.
///
/// Each test builds its own store on a scratch `UserDefaults` suite, so nothing here reads or
/// writes the preferences the user actually has.
final class EditorPreferencesTests: XCTestCase {
    private var suite: UserDefaults!

    override func setUp() {
        super.setUp()
        let suiteName = "qh-editor-prefs-\(UUID().uuidString)"
        suite = UserDefaults(suiteName: suiteName)
        addTeardownBlock {
            UserDefaults.standard.removePersistentDomain(forName: suiteName)
        }
    }

    private func store() -> EditorPreferences {
        EditorPreferences(defaults: suite)
    }

    func testAnUntouchedInstallGetsWhatTheEditorAlreadyDid() {
        // Every default is the behaviour that shipped before these switches existed, so an
        // upgraded install looks the same as a fresh one.
        let prefs = store()
        XCTAssertTrue(prefs.showLineNumbers)
        XCTAssertTrue(prefs.highlightCurrentLine)
        XCTAssertTrue(prefs.highlightCurrentStatement)
        XCTAssertTrue(prefs.wordWrap)
        XCTAssertTrue(prefs.codeFolding)
        XCTAssertFalse(prefs.showInvisibles)
        XCTAssertEqual(prefs.tabWidth, 4)
        // 12.5 is the size the editor was hard-coded to, so an upgrade does not move a pixel.
        XCTAssertEqual(prefs.fontSize, 12.5)
    }

    func testAChangeIsWrittenDownAndReadBack() {
        let prefs = store()
        prefs.showLineNumbers = false
        prefs.wordWrap = false
        prefs.showInvisibles = true
        prefs.tabWidth = 8
        prefs.fontSize = 15.5

        let reopened = EditorPreferences(defaults: suite)
        XCTAssertFalse(reopened.showLineNumbers)
        XCTAssertFalse(reopened.wordWrap)
        XCTAssertTrue(reopened.showInvisibles)
        XCTAssertEqual(reopened.tabWidth, 8)
        XCTAssertEqual(reopened.fontSize, 15.5)
    }

    func testAFontSizeIsClampedToWhatReadsAsCode() {
        let prefs = store()
        prefs.fontSize = 3
        XCTAssertEqual(prefs.fontSize, 10)
        prefs.fontSize = 400
        XCTAssertEqual(prefs.fontSize, 28)
        prefs.fontSize = .nan
        XCTAssertEqual(prefs.fontSize, 12.5, "NaN is not a size, so it is the default")
        prefs.fontSize = 28
        XCTAssertEqual(prefs.fontSize, 28, "the ends of the range are sizes")
    }

    func testAStoredSizeOutsideTheRangeIsHeldInsideItOnLoad() {
        // A hand-edited plist, or a range that was narrower once: what is read is clamped like what
        // is written, and a stored zero is the same as no key at all.
        suite.set(99.0, forKey: "editorFontSize")
        XCTAssertEqual(store().fontSize, 28)
        suite.set(-4.0, forKey: "editorFontSize")
        XCTAssertEqual(store().fontSize, 12.5)
        suite.set(0.0, forKey: "editorFontSize")
        XCTAssertEqual(store().fontSize, 12.5)
    }

    func testATabWidthIsClampedToSomethingUsable() {
        // The value comes from a stepper, but a zero or a negative would put every tab stop on top
        // of the last, and a width nobody can read is not a setting worth honouring.
        let prefs = store()
        prefs.tabWidth = 0
        XCTAssertEqual(prefs.tabWidth, 1)
        prefs.tabWidth = 99
        XCTAssertEqual(prefs.tabWidth, 8)
    }

    func testTheLayoutFollowsTheSharedStore() {
        // What the editor is handed: `EditorLayout.current` reads the store, so this is the one
        // assertion that ties the switches to the value SwiftUI compares.
        let prefs = EditorPreferences.shared
        let before = EditorLayout.current
        addTeardownBlock {
            prefs.showLineNumbers = before.showLineNumbers
            prefs.highlightCurrentLine = before.highlightCurrentLine
            prefs.highlightCurrentStatement = before.highlightCurrentStatement
            prefs.wordWrap = before.wordWrap
            prefs.codeFolding = before.codeFolding
            prefs.showInvisibles = before.showInvisibles
            prefs.tabWidth = before.tabWidth
            prefs.fontSize = before.fontSize
        }

        prefs.showInvisibles = !before.showInvisibles
        XCTAssertNotEqual(EditorLayout.current, before)
        XCTAssertEqual(EditorLayout.current.showInvisibles, !before.showInvisibles)

        // The size is part of the value SwiftUI compares, which is what re-applies the editor.
        prefs.fontSize = before.fontSize + 1
        XCTAssertEqual(EditorLayout.current.fontSize, before.fontSize + 1)
    }

    func testTheHighlighterPaintsInTheChosenSize() {
        // The editor's text, its comment italics and its base attributes all come from
        // `SQLSyntax.font`, so a size that did not reach it would be a setting that did nothing.
        let prefs = EditorPreferences.shared
        let before = prefs.fontSize
        addTeardownBlock { prefs.fontSize = before }

        prefs.fontSize = 12.5
        XCTAssertEqual(SQLSyntax.font(italic: false).pointSize, 12.5)
        prefs.fontSize = 16
        XCTAssertEqual(SQLSyntax.font(italic: false).pointSize, 16)
        XCTAssertEqual(SQLSyntax.font(italic: true).pointSize, 16)
    }

    @MainActor
    func testAnEditorOnScreenFollowsAChangeOfSize() throws {
        // `applyLayout` is what a switch flipped in Settings reaches. A size has to move the text
        // view's font, and the whole document's attributes with it, or the editor would keep the
        // size it opened with until the tab was closed.
        let prefs = EditorPreferences.shared
        let before = prefs.fontSize
        addTeardownBlock { prefs.fontSize = before }
        prefs.fontSize = 12.5

        let storage = NSTextStorage(string: "select 1")
        let layoutManager = NSLayoutManager()
        storage.addLayoutManager(layoutManager)
        let box = NSTextContainer(size: NSSize(width: 420, height: 1000))
        layoutManager.addTextContainer(box)
        let view = SQLTextView(frame: NSRect(x: 0, y: 0, width: 420, height: 400), textContainer: box)
        view.font = SQLSyntax.font(italic: false)
        func editor(_ layout: EditorLayout) -> SQLEditor {
            SQLEditor(text: .constant("select 1"), focused: .constant(false), caret: .constant(0),
                      selection: .constant(NSRange(location: 0, length: 0)),
                      completion: EditorCompletion(), candidates: { _, _ in [] },
                      layout: layout, onRunStatement: nil)
        }
        let coordinator = editor(.current).makeCoordinator()
        coordinator.textView = view
        storage.delegate = coordinator
        view.delegate = coordinator
        coordinator.recolour()
        coordinator.applyLayout()
        XCTAssertEqual(view.font?.pointSize, 12.5)

        prefs.fontSize = 18
        coordinator.parent = editor(.current)
        coordinator.applyLayout()
        XCTAssertEqual(view.font?.pointSize, 18)
        let attributes = storage.attributes(at: 0, effectiveRange: nil)
        XCTAssertEqual((attributes[.font] as? NSFont)?.pointSize, 18,
                       "the text already in the editor is resized, not only what is typed next")
    }
}
