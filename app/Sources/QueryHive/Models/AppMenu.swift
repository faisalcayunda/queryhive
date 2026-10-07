import SwiftUI

enum MenuGroup { case file, query, view, tab }

/// One menu item as data, so the menu, the conflict test and Settings read the same list.
struct MenuSpec: Equatable {
    let id: String
    let title: String
    let shortcut: Shortcut?
    let group: MenuGroup
}

enum AppMenu {
    private static let tabPrefix = "goToTab"

    static func specs(for scheme: ShortcutScheme) -> [MenuSpec] {
        func spec(_ action: ShortcutAction, _ group: MenuGroup, _ title: String? = nil) -> MenuSpec {
            MenuSpec(id: action.rawValue, title: title ?? action.title,
                     shortcut: scheme.shortcut(for: action), group: group)
        }
        var list = [
            spec(.newQuery, .file, "New Query"),
            spec(.closeTab, .file, "Close Query"),
            spec(.openFile, .file, "Load SQL File…"),
            spec(.openQuickly, .file, "Open Quickly…"),
            spec(.run, .query),
            spec(.runScript, .query),
            spec(.explain, .query),
            spec(.exportData, .query, "Export"),
            spec(.countRows, .query),
            spec(.stop, .query),
            spec(.revealOutput, .query),
            spec(.toggleSidebar, .view),
            spec(.toggleResultPanel, .view),
            spec(.focusSidebar, .view),
            spec(.focusEditor, .view),
            spec(.focusResults, .view),
            spec(.peekCell, .view),
            spec(.toggleRecord, .view),
            spec(.fontBigger, .view),
            spec(.fontSmaller, .view),
            spec(.fontReset, .view),
            spec(.nextTab, .tab),
            spec(.previousTab, .tab),
        ]
        for position in 1...9 {
            list.append(MenuSpec(id: tabPrefix + String(position),
                                 title: position == 9 ? "Go to Last Tab" : "Go to Tab \(position)",
                                 shortcut: Shortcut(KeyEquivalent(Character(String(position))), .command),
                                 group: .tab))
        }
        return list
    }

    private static func tabPosition(_ spec: MenuSpec) -> Int? {
        spec.id.hasPrefix(tabPrefix) ? Int(spec.id.dropFirst(tabPrefix.count)) : nil
    }

    /// The one place a menu id becomes an action.
    @MainActor
    static func perform(_ spec: MenuSpec, in model: AppModel) {
        if let position = tabPosition(spec) { model.selectTab(position: position); return }
        let tab = model.selectedTab
        switch ShortcutAction(rawValue: spec.id) {
        case .newQuery: model.newTab()
        case .closeTab: model.closeSelectedTab()
        case .openFile: tab?.loadSQLFromFile()
        case .openQuickly: model.openQuickly()
        case .run: model.runSelectedTab()
        case .runScript: tab.map { model.run($0, from: .all) }
        case .explain: tab.map { model.explain($0) }
        case .exportData: tab.map { model.run($0) }
        case .countRows: tab.map { model.countRows($0) }
        case .stop: model.stopSelectedTab()
        case .revealOutput: tab?.revealFiles()
        case .toggleSidebar: model.toggleSidebar()
        case .toggleResultPanel: model.toggleResultPanel()
        case .focusSidebar: model.focus(.sidebar)
        case .focusEditor: model.focus(.editor)
        case .focusResults: model.focus(.results)
        case .nextTab: model.selectTab(offset: 1)
        case .previousTab: model.selectTab(offset: -1)
        // The responder chain, because the cell lives in the grid and the grid is not the model's:
        // it answers only while it is the first responder, which is when a peek means anything.
        case .peekCell: NSApp.sendAction(#selector(GridTableView.peekCell(_:)), to: nil, from: nil)
        case .toggleRecord:
            guard let tab else { break }
            tab.recordMode.toggle()
            Announcer.post(tab.recordMode ? "Record view shown" : "Record view hidden")
        case .fontBigger: model.adjustFontSize(.bigger)
        case .fontSmaller: model.adjustFontSize(.smaller)
        case .fontReset: model.adjustFontSize(.reset)
        default: break
        }
    }

    @MainActor
    static func isEnabled(_ spec: MenuSpec, in model: AppModel) -> Bool {
        if let position = tabPosition(spec) { return position <= model.tabs.count }
        let tab = model.selectedTab
        let blocked = tab.map { model.runBlockedReason(for: $0) != nil } ?? true
        switch ShortcutAction(rawValue: spec.id) {
        case .openFile, .closeTab, .toggleResultPanel, .focusEditor, .focusResults, .toggleRecord: return tab != nil
        case .run: return tab?.previewing != true
        case .runScript, .explain, .exportData: return !blocked
        case .countRows: return tab?.previewedSQL?.isEmpty == false && tab?.countingRows == false
        case .stop: return tab?.stage == .running
        case .revealOutput: return tab?.files.isEmpty == false
        case .nextTab, .previousTab: return model.tabs.count > 1
        case .peekCell: return tab?.cellCursor != nil
        case .fontBigger: return model.fontSizeChanges(.bigger)
        case .fontSmaller: return model.fontSizeChanges(.smaller)
        case .fontReset: return model.fontSizeChanges(.reset)
        default: return true
        }
    }
}
