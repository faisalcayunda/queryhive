import AppKit
import Foundation

/// One entry of a tree row's context menu, as data: the outline builds an `NSMenu` from it, and a
/// test reads titles, order and ticks without a window.
struct TreeMenuItem {
    var title: String
    var checked = false
    var action: (() -> Void)?
    var submenu: [TreeMenuItem]?
    var isSeparator = false

    static let separator = TreeMenuItem(title: "", isSeparator: true)
}

/// The menus the object tree offers, moved as they were from the SwiftUI rows: same titles, same
/// order, same separators, same Postgres-only switches.
enum TreeMenu {
    static func items(for node: TreeNode, in model: AppModel) -> [TreeMenuItem] {
        typealias Item = TreeMenuItem
        let sep = Item.separator
        func refresh() -> Item { Item(title: "Refresh") { model.refresh(node) } }
        func copy(_ title: String, _ text: String) -> Item {
            Item(title: title) {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(text, forType: .string)
            }
        }
        switch node.kind {
        case .connection:
            // The whole menu is written against `id`: a group is the only node without a
            // connection, and a group is not this case.
            guard let id = node.connectionID else { return [] }
            var items = [Item(title: "Open Connection") { model.expand(node) }]
            // Only Postgres filters system schemas and hides other databases away, so only there do
            // the two switches reveal anything.
            if let connection = model.connections.first(where: { $0.id == id }), connection.kind == .postgres {
                items.append(Item(title: "Show System Schemas", checked: connection.showAllSchemas) {
                    model.toggleShowAllSchemas(id)
                })
                items.append(Item(title: "Show All Databases", checked: connection.showAllDatabases) {
                    model.toggleShowAllDatabases(id)
                })
            }
            items += [sep,
                      Item(title: "Edit Connection…") { model.presentConnectionEditor(id) },
                      Item(title: "Duplicate Connection") { model.duplicateConnection(id) },
                      Item(title: "Delete Connection…") { model.requestDelete(id) },
                      sep, groupMenu(for: id, in: model), sep,
                      Item(title: "New Connection") { model.presentConnectionEditor(nil) },
                      sep,
                      // No key equivalents in here: ⌘R is Run and ⌘T is New Query in the Query menu.
                      Item(title: "New Query") { model.newTab(connectionID: id) },
                      Item(title: "Open SQL File…") { model.runSQLFile(connectionID: id) },
                      sep,
                      Item(title: "Color", submenu: ConnectionColor.allCases.map { color in
                          Item(title: color.rawValue.capitalized, checked: node.color == color) {
                              model.setColor(color, for: id)
                          }
                      }),
                      sep, refresh(),
                      Item(title: "Reveal connections.json") {
                          if let url = try? ConnectionStore.directory() {
                              NSWorkspace.shared.activateFileViewerSelecting(
                                  [url.appendingPathComponent("connections.json")])
                          }
                      }]
            return items
        case .group:
            // Deleting a group does **not** delete its connections.
            return [Item(title: "New Connection in Group") {
                        model.presentConnectionEditor(nil, inGroup: node.groupID)
                    },
                    sep,
                    Item(title: "Rename Group…") { model.presentRenameGroup(node.groupID) },
                    Item(title: "Delete Group") { model.deleteGroup(node.groupID) },
                    sep, refresh()]
        case .catalog, .database, .schema:
            var items = [refresh(), copy("Copy Name", node.title)]
            if let id = node.connectionID {
                items += [sep,
                          Item(title: "New Query") { model.newTab(connectionID: id) },
                          Item(title: "Open SQL File…") { model.runSQLFile(connectionID: id) }]
            }
            return items
        case .table:
            var items = [Item(title: "Insert into Query") { model.insert(node) }]
            if let qualified = node.insertableText { items.append(copy("Copy Qualified Name", qualified)) }
            items += [copy("Copy Name", node.title), sep,
                      Item(title: "Import Data into Table…") { model.presentImport(into: node) },
                      sep,
                      // Whether a question is asked is `RunConfirmation.destructiveRequest`'s call.
                      Item(title: "Truncate Table…") { model.requestTableOperation(.truncate, node: node) },
                      Item(title: "Drop Table…") { model.requestTableOperation(.drop, node: node) },
                      sep, refresh()]
            return items
        }
    }

    /// Which group a connection is filed under: every group that exists, the way out of all of them,
    /// and the way to make a new one.
    private static func groupMenu(for id: UUID, in model: AppModel) -> TreeMenuItem {
        typealias Item = TreeMenuItem
        let filed = model.connections.first(where: { $0.id == id })?.group
        var sub = [Item(title: "No Group", checked: filed == nil) { model.move(id, toGroup: nil) }]
        if !model.groups.isEmpty {
            sub.append(.separator)
            sub += model.groups.map { group in
                Item(title: group.name, checked: filed == group.id) { model.move(id, toGroup: group.id) }
            }
        }
        sub += [.separator, Item(title: "New Group…") { model.presentNewGroup(with: id) }]
        return Item(title: "Group", submenu: sub)
    }
}
