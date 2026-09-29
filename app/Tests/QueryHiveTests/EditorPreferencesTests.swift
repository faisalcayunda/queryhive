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
    }

    func testAChangeIsWrittenDownAndReadBack() {
        let prefs = store()
        prefs.showLineNumbers = false
        prefs.wordWrap = false
        prefs.showInvisibles = true
        prefs.tabWidth = 8

        let reopened = EditorPreferences(defaults: suite)
        XCTAssertFalse(reopened.showLineNumbers)
        XCTAssertFalse(reopened.wordWrap)
        XCTAssertTrue(reopened.showInvisibles)
        XCTAssertEqual(reopened.tabWidth, 8)
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
        }

        prefs.showInvisibles = !before.showInvisibles
        XCTAssertNotEqual(EditorLayout.current, before)
        XCTAssertEqual(EditorLayout.current.showInvisibles, !before.showInvisibles)
    }
}
