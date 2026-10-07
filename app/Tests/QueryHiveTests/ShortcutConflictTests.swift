import SwiftUI
import XCTest
@testable import QueryHive

final class ShortcutConflictTests: XCTestCase {
    /// Keys the editor, the Open Quickly overlay and the connection sheet handle themselves, plus
    /// the keys W10 reserves in the platform table (rows). ⌘Y (the peek), ⌥⌘I (the Record view) and
    /// ⌘+, ⌘−, ⌘0 (the font size, W10-T7b) are bound now, so the duplicate check sees them as actions.
    private static let localAndReserved: [(String, String)] = [
        ("f", "⌘"), ("f", "⌥⌘"), ("g", "⌘"), ("g", "⇧⌘"), ("[", "⌥⌘"), ("]", "⌥⌘"),
        ("n", "⌥⌘"), ("\u{7F}", "⌘"),
        // ⌘= is the font size's alias (`FontKeyRouter`), taken by the key monitor, not the menu.
        ("=", "⌘"),
    ]

    private static let system: [Shortcut] = [
        Shortcut("q", .command), Shortcut("h", .command), Shortcut("h", [.command, .option]),
        Shortcut("m", .command), Shortcut(",", .command), Shortcut("n", .command),
        Shortcut("p", .command), Shortcut("z", .command), Shortcut("z", [.command, .shift]),
        Shortcut("x", .command), Shortcut("c", .command), Shortcut("v", .command),
        Shortcut("a", .command), Shortcut("`", .command), Shortcut(" ", [.control, .command]),
        Shortcut("f", [.control, .command]), Shortcut("3", [.command, .shift]),
        Shortcut("4", [.command, .shift]), Shortcut("5", [.command, .shift]),
    ]

    private func duplicates(_ scheme: ShortcutScheme) -> [String] {
        var seen: [String: String] = [:]
        var clashes: [String] = []
        for spec in AppMenu.specs(for: scheme) {
            guard let key = spec.shortcut?.display else { continue }
            if let other = seen[key] { clashes.append("\(key): \(other) and \(spec.id)") }
            seen[key] = spec.id
        }
        for (key, mods) in Self.localAndReserved {
            let display = mods + key.uppercased()
            if let other = seen[display] { clashes.append("\(display): \(other) and a local key") }
        }
        return clashes
    }

    func testNoKeyIsBoundTwiceInTheDbeaverScheme() { XCTAssertEqual(duplicates(.dbeaver), []) }
    func testNoKeyIsBoundTwiceInTheQueryhiveScheme() { XCTAssertEqual(duplicates(.queryhive), []) }

    /// The connection sheet is modal, so it is its own scope: it is the key window while it is up and
    /// its key equivalents are matched before the main menu's. ⌘↩ tests the connection there, even in
    /// the DBeaver scheme where the same chord runs a statement behind it, and ⌘T (New Query in the
    /// QueryHive scheme, where it used to clash with Test) is free again.
    func testTheConnectionSheetsTestKeyIsItsOwnScopeAndNotOneOfTheMenusKeys() {
        let key = SheetShortcut.testConnection
        XCTAssertEqual(key.display, "⌘↩")
        XCTAssertFalse(Self.system.contains(key))
        XCTAssertNotEqual(key, Shortcut("t", .command))
        // The queryhive scheme never binds it, so there is nothing to shadow there; the dbeaver
        // scheme's Run does, and is the reason the sheet is modelled as a scope of its own.
        XCTAssertFalse(AppMenu.specs(for: .queryhive).contains { $0.shortcut == key })
        XCTAssertEqual(ShortcutScheme.dbeaver.shortcut(for: .run), key)
        XCTAssertEqual(ShortcutScheme.queryhive.shortcut(for: .newQuery)?.display, "⌘T")
    }

    func testNoActionUsesASystemKey() {
        for scheme in ShortcutScheme.allCases {
            for action in ShortcutAction.allCases {
                guard let key = scheme.shortcut(for: action) else { continue }
                XCTAssertFalse(Self.system.contains(key), "\(action) uses a system key in \(scheme)")
            }
        }
    }

    func testPlatformActionsHaveTheSameKeyInBothSchemes() {
        for action in ShortcutAction.allCases where action.scope == .platform {
            XCTAssertEqual(ShortcutScheme.dbeaver.shortcut(for: action), ShortcutScheme.queryhive.shortcut(for: action))
        }
    }

    func testExportAndExplainDifferInTheQueryhiveScheme() {
        XCTAssertEqual(ShortcutScheme.queryhive.shortcut(for: .explain)?.display, "⌘E")
        XCTAssertEqual(ShortcutScheme.queryhive.shortcut(for: .exportData)?.display, "⇧⌘E")
    }

    func testEveryAvailableActionHasAMenuItemAndEveryKeyedItemIsAnAction() {
        for scheme in ShortcutScheme.allCases {
            let specs = AppMenu.specs(for: scheme)
            let ids = Set(specs.map(\.id))
            for action in ShortcutAction.allCases where action.isAvailable {
                XCTAssertTrue(ids.contains(action.rawValue), "\(action) has no menu item")
            }
            for spec in specs where spec.shortcut != nil && !spec.id.hasPrefix("goToTab") {
                XCTAssertNotNil(ShortcutAction(rawValue: spec.id), "\(spec.id) is not an action")
            }
        }
    }

    func testGoToTabCoversOneToNine() {
        let tabs = AppMenu.specs(for: .dbeaver).filter { $0.id.hasPrefix("goToTab") }
        XCTAssertEqual(tabs.compactMap { $0.shortcut?.display }, (1...9).map { "⌘\($0)" })
    }

    func testNoViewHardCodesAKeyboardShortcut() throws {
        // The one literal key that remains is Panels' Reveal button (the menu now owns ⇧⌘R). The
        // connection sheet's Test took its key from `SheetShortcut` (W11-T3), so it is not here.
        let allowed: Set<String> = ["Panels.swift"]
        let dir = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Sources/QueryHive")
        let files = try XCTUnwrap(FileManager.default.enumerator(at: dir, includingPropertiesForKeys: nil))
        for case let url as URL in files where url.pathExtension == "swift" && !allowed.contains(url.lastPathComponent) {
            let text = try String(contentsOf: url, encoding: .utf8)
            XCTAssertFalse(text.contains(".keyboardShortcut(\""), "\(url.lastPathComponent) hard-codes a key")
        }
    }

    func testTabKeyRouterMapsControlTabAndShiftControlTab() {
        XCTAssertEqual(TabKeyRouter.route(keyCode: 48, control: true, shift: false, command: false, option: false), .next)
        XCTAssertEqual(TabKeyRouter.route(keyCode: 48, control: true, shift: true, command: false, option: false), .previous)
    }

    func testTabKeyRouterIgnoresPlainTab() {
        XCTAssertNil(TabKeyRouter.route(keyCode: 48, control: false, shift: false, command: false, option: false))
        XCTAssertNil(TabKeyRouter.route(keyCode: 48, control: false, shift: true, command: false, option: false))
        XCTAssertNil(TabKeyRouter.route(keyCode: 48, control: true, shift: false, command: true, option: false))
    }

    @MainActor
    func testTabNavigationWrapsAndGoToTabClamps() {
        let model = AppModel(persistsSession: false)
        model.tabs = []
        model.newTab(); model.newTab(); model.newTab()
        model.selectTab(position: 1)
        model.selectTab(offset: -1)
        XCTAssertEqual(model.selectedTabID, model.tabs.last?.id)
        model.selectTab(offset: 1)
        XCTAssertEqual(model.selectedTabID, model.tabs.first?.id)
        model.selectTab(position: 9)
        XCTAssertEqual(model.selectedTabID, model.tabs.last?.id)
        XCTAssertEqual(Announcer.last, model.tabs.last?.title)
    }

    @MainActor
    func testFocusWithoutAGridAnnouncesIt() {
        let model = AppModel(persistsSession: false)
        Announcer.reset()
        model.focus(.results)
        XCTAssertEqual(Announcer.last, "No results to focus")
    }
}
