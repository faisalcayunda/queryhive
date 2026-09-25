import AppKit
import SwiftUI

/// The object tree down the left: connection → catalog → schema → table, each level fetched the
/// first time it is opened. Navicat's spine, wearing the CleanMyMac surfaces.
struct SidebarTree: View {
    @Environment(AppModel.self) private var model

    /// Ids to keep on screen while the filter box has text. `nil` means "no filter".
    private var visibleIDs: Set<String>? {
        let needle = model.treeFilter.trimmingCharacters(in: .whitespaces)
        guard !needle.isEmpty else { return nil }
        var ids = Set<String>()
        for node in model.tree { ids.formUnion(node.matchingIDs(needle)) }
        return ids
    }

    var body: some View {
        @Bindable var model = model
        VStack(spacing: 0) {
            header
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 1) {
                    if model.tree.isEmpty {
                        emptyState
                    } else if let ids = visibleIDs, ids.isEmpty {
                        noMatches
                    } else {
                        ForEach(model.tree) { node in
                            TreeRow(node: node, depth: 0, visible: visibleIDs,
                                    selectedID: model.selectedNodeID)
                        }
                    }
                }
                .padding(.horizontal, 6)
                .padding(.vertical, 5)
            }
        }
        .background {
            Rectangle().fill(.thinMaterial)
            Rectangle().fill(Tone.recess.opacity(0.18))
        }
        .overlay(alignment: .trailing) { Rectangle().fill(Tone.ink.opacity(0.07)).frame(width: 1) }
    }

    private var header: some View {
        @Bindable var model = model
        return VStack(spacing: 8) {
            // The app's identity, at the top of the column that is the app's own panel. It
            // replaced a bare "OBJECTS" label: the tree below is self-evidently objects, and a
            // panel header earns its place better by naming the thing it belongs to.
            HStack(spacing: 7) {
                HiveMark(size: 15)
                Text("QUERYHIVE")
                    .font(.ui(10.5, weight: .bold))
                    .tracking(1.1)
                    .foregroundStyle(Tone.secondary)
                Spacer()
                Menu {
                    Button("New Connection") { model.presentConnectionEditor(nil) }
                    Button("Add from URL…") { model.presentConnectionEditor(nil, startAtURL: true) }
                } label: {
                    Image(systemName: "plus.circle.fill")
                        .font(.system(size: 13))
                        .foregroundStyle(Tone.secondary)
                        .frame(width: 20, height: 20)
                }
                .menuStyle(.button)
                .buttonStyle(.plain)
                .menuIndicator(.hidden)
                .frame(width: 20, height: 20)
                .help("New connection, or add one from a Trino URL")

                IconButton(symbol: "arrow.clockwise", help: "Collapse and reload every connection", diameter: 20) {
                    model.reloadTree()
                }
            }
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").font(.system(size: 10)).foregroundStyle(Tone.secondary)
                TextField("Filter loaded objects", text: $model.treeFilter)
                    .textFieldStyle(.plain)
                    .font(.system(size: 11))
                if !model.treeFilter.isEmpty {
                    Button { model.treeFilter = "" } label: {
                        Image(systemName: "xmark.circle.fill")
                            .font(.system(size: 10))
                            .foregroundStyle(Tone.secondary)
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 8)
            .frame(height: 24)
            .background(Tone.recess.opacity(0.28), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).strokeBorder(Tone.ink.opacity(0.08)))
        }
        .padding(.horizontal, Metrics.gutter)
        .padding(.top, 12)
        .padding(.bottom, 9)
        .overlay(alignment: .bottom) { Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1) }
    }

    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("No connections yet.")
                .font(.ui(12, weight: .semibold))
            Text("Add a Trino coordinator to browse its catalogs.")
                .font(.ui(11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
            PillButton(title: "New Connection", symbol: "plus") { model.presentConnectionEditor(nil) }
            PillButton(title: "Add from URL…", symbol: "link", role: .quiet) { model.presentConnectionEditor(nil, startAtURL: true) }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .glass(12)
        .padding(.top, 4)
    }

    private var noMatches: some View {
        Text("Nothing loaded matches “\(model.treeFilter)”.")
            .font(.ui(11))
            .foregroundStyle(Tone.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .padding(12)
    }
}

/// One tree row plus, when it is open, its children.
///
/// The recursion goes through `@ViewBuilder` functions rather than a `View` whose body contains
/// itself: a self-containing `View` is an infinitely sized type, and the old code broke that cycle
/// with `AnyView` — which erases the type, so SwiftUI cannot diff child rows at all and rebuilds
/// the whole subtree whenever the parent redraws. With a tree this size that is what made
/// expanding and scrolling feel sluggish. A function can call itself and still return a concrete
/// `some View`, so every row keeps its identity and only what changed is redrawn.
struct TreeRow: View {
    @Environment(AppModel.self) private var model
    @Bindable var node: TreeNode
    let depth: Int
    let visible: Set<String>?
    /// The selected row's id, passed down from the root rather than read from the model here.
    ///
    /// Reading `model.selectedNodeID` inside this body subscribed **every visible row** to the
    /// selection, so one click re-ran every row's body instead of the two whose highlight moved
    /// — with a few hundred rows on screen that is the click that felt heavy. As a plain `let`,
    /// SwiftUI compares it per row and skips every body whose value did not change. The same
    /// reason `visible` above is a value, and the reason the comment on the old `selected` flag
    /// gave for passing it in.
    let selectedID: String?
    @State private var hovering = false

    /// Whether this row is the selected one. Computed from the passed-in id, so the body still
    /// reads nothing from the model.
    private var selected: Bool { node.id == selectedID }

    private var isVisible: Bool { visible?.contains(node.id) ?? true }
    /// A filter auto-opens every level it can see, so a deep match is not hidden behind a node
    /// the user would have to expand by hand.
    private var showChildren: Bool { visible != nil || node.expanded }

    var body: some View {
        if isVisible {
            VStack(alignment: .leading, spacing: 1) {
                row
                children
            }
        }
    }

    /// The children, built only when this node is actually open. A collapsed catalog contributes
    /// one line to the layout instead of a row per schema it has never fetched.
    @ViewBuilder private var children: some View {
        if node.loading {
            messageRow(symbol: nil, text: "Loading…", tint: Tone.secondary)
        } else if let error = node.error {
            messageRow(symbol: "exclamationmark.triangle.fill", text: error, tint: Tone.coral)
        } else if showChildren, let children = node.children {
            if children.isEmpty {
                messageRow(symbol: nil, text: "Empty", tint: Tone.ink.opacity(0.3))
            } else {
                // Lazy, so a wide catalog builds only the rows on screen. The recursion is a
                // method call on the child, not a nested `TreeRow`, which is what keeps the type
                // concrete.
                LazyVStack(alignment: .leading, spacing: 1) {
                    ForEach(children) { child in
                        TreeRow(node: child, depth: depth + 1, visible: visible,
                                selectedID: selectedID)
                    }
                }
            }
        }
    }

    private var row: some View {
        HStack(spacing: 5) {
            Button {
                if node.isExpandable { model.toggleExpansion(node) }
            } label: {
                Image(systemName: "chevron.right")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundStyle(Tone.secondary)
                    .rotationEffect(.degrees(showChildren ? 90 : 0))
                    .frame(width: 11, height: 11)
                    .opacity(node.isExpandable ? 1 : 0)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            Image(systemName: node.kind.symbol)
                .font(.system(size: 10.5, weight: .medium))
                .foregroundStyle(iconTint)
                .frame(width: 15)

            Text(node.title)
                .font(.ui(12, weight: node.kind == .connection ? .semibold : .regular))
                .lineLimit(1)
                .truncationMode(.middle)

            Spacer(minLength: 2)
        }
        .padding(.leading, CGFloat(depth) * Metrics.treeIndent + 6)
        .padding(.trailing, 7)
        .frame(height: Metrics.treeRow)
        .background(Tone.ink.opacity(selected ? 0.13 : (hovering ? 0.06 : 0)),
                    in: RoundedRectangle(cornerRadius: 6, style: .continuous))
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .onTapGesture(count: 2) { doubleClick() }
        .onTapGesture { model.selectedNodeID = node.id }
        .contextMenu { menu }
        .help(helpText)
    }

    private func messageRow(symbol: String?, text: String, tint: Color) -> some View {
        HStack(spacing: 5) {
            if let symbol {
                Image(systemName: symbol).font(.system(size: 9)).foregroundStyle(tint)
            } else {
                ProgressView().controlSize(.mini)
            }
            Text(text).font(.ui(10.5)).foregroundStyle(tint).lineLimit(2)
            Spacer(minLength: 2)
        }
        .padding(.leading, CGFloat(depth + 1) * Metrics.treeIndent + 6)
        .padding(.trailing, 7)
        .padding(.vertical, 2)
    }

    @ViewBuilder private var menu: some View {
        switch node.kind {
        case .connection:
            // Navicat's connection menu, minus the entries this app has nothing behind: no
            // connection profiles, no server-side "New Database", no sharing. What is left is what
            // the app can actually do — plus groups, which is where a connection is filed.
            //
            // The whole menu is written once against `id`, the one thing on this branch that is
            // certain: a group is the only node without a connection, and a group is not this case.
            if let id = node.connectionID {
                Button("Open Connection") { model.expand(node) }
                // Only Postgres filters system schemas away, so only there does "show all" reveal
                // anything. MySQL and Trino already list everything `SHOW DATABASES` / `SHOW SCHEMAS`
                // return, so offering the switch there would be a control with nothing behind it.
                if let connection = model.connections.first(where: { $0.id == id }),
                   connection.kind == .postgres {
                    Button {
                        model.toggleShowAllSchemas(id)
                    } label: {
                        if connection.showAllSchemas {
                            Label("Show System Schemas", systemImage: "checkmark")
                        } else {
                            Text("Show System Schemas")
                        }
                    }
                }
                Divider()
                Button("Edit Connection…") { model.presentConnectionEditor(id) }
                Button("Duplicate Connection") { model.duplicateConnection(id) }
                Button("Delete Connection…") { model.requestDelete(id) }
                Divider()
                groupMenu(for: id)
                Divider()
                Button("New Connection") { model.presentConnectionEditor(nil) }
                Divider()
                // No keyboard shortcuts in here: this app's ⌘R is Run and ⌘T is already New Query
                // from the Query menu, so printing either beside a different action would mislead,
                // and declaring one twice can fire it twice.
                Button("New Query") { model.newTab(connectionID: id) }
                Button("Open SQL File…") { model.runSQLFile(connectionID: id) }
                Divider()
                Menu("Color") {
                    ForEach(ConnectionColor.allCases) { color in
                        Button {
                            model.setColor(color, for: id)
                        } label: {
                            // The colour's own name, with a tick on the one in use. A context menu
                            // cannot draw swatches, and inventing a row of coloured dots that only
                            // works in one menu would be worse than saying the colour.
                            if node.color == color {
                                Label(color.rawValue.capitalized, systemImage: "checkmark")
                            } else {
                                Text(color.rawValue.capitalized)
                            }
                        }
                    }
                }
                Divider()
                Button("Refresh") { model.refresh(node) }
                Button("Reveal connections.json") {
                    if let url = try? ConnectionStore.directory() {
                        NSWorkspace.shared.activateFileViewerSelecting([url.appendingPathComponent("connections.json")])
                    }
                }
            }
        case .group:
            // A group holds connections and nothing else, so its menu is about the filing: what to
            // call it, what goes in it, and how to get rid of it without losing what is inside.
            Button("New Connection in Group") {
                model.presentConnectionEditor(nil, inGroup: node.groupID)
            }
            Divider()
            Button("Rename Group…") { model.presentRenameGroup(node.groupID) }
            // Deleting a group does **not** delete its connections. A folder that took its contents
            // with it would make this the most dangerous item in the side bar, and there is nothing
            // in the gesture that says "and the servers too".
            Button("Delete Group") { model.deleteGroup(node.groupID) }
            Divider()
            Button("Refresh") { model.refresh(node) }
        case .catalog, .database, .schema:
            Button("Refresh") { model.refresh(node) }
            Button("Copy Name") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(node.title, forType: .string)
            }
            // A catalog, database or schema always belongs to a connection — only a group has none —
            // so the unwrap is a formality the compiler needs and the tree can always satisfy.
            if let id = node.connectionID {
                Divider()
                Button("New Query") { model.newTab(connectionID: id) }
                Button("Open SQL File…") { model.runSQLFile(connectionID: id) }
            }
        case .table:
            Button("Insert into Query") { model.insert(node) }
            if let qualified = node.insertableText {
                Button("Copy Qualified Name") {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(qualified, forType: .string)
                }
            }
            Button("Copy Name") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(node.title, forType: .string)
            }
            Divider()
            Button("Refresh") { model.refresh(node) }
        }
    }

    /// Which group a connection is filed under: the list of groups that exist, the way out of all
    /// of them, and the way to make a new one. Every answer to "where should this live" is in this
    /// menu, including "nowhere", which is a real answer.
    @ViewBuilder
    private func groupMenu(for id: UUID) -> some View {
        let filed = model.connections.first(where: { $0.id == id })?.group
        Menu("Group") {
            Button {
                model.move(id, toGroup: nil)
            } label: {
                if filed == nil { Label("No Group", systemImage: "checkmark") } else { Text("No Group") }
            }
            if !model.groups.isEmpty {
                Divider()
                ForEach(model.groups) { group in
                    Button {
                        model.move(id, toGroup: group.id)
                    } label: {
                        if filed == group.id {
                            Label(group.name, systemImage: "checkmark")
                        } else {
                            Text(group.name)
                        }
                    }
                }
            }
            Divider()
            Button("New Group…") { model.presentNewGroup(with: id) }
        }
    }

    /// The tree's double-click: **Open** on a table, **list its objects** on a schema or a MySQL
    /// database, and **expand** on everything else.
    ///
    /// A schema used to expand here like a catalog does. That is the right gesture for a catalog --
    /// you are walking down to the level you want -- but a schema is where the objects actually
    /// are, and asking to see them is what a click there means. Expanding is still one click away
    /// on the disclosure triangle, which is where the platform puts it anyway.
    ///
    /// A connection used to open the editor here, which made the one row you double-click most
    /// often — the root of the tree, the thing you expand to start browsing — the one row that
    /// refused to expand. Expand/collapse is what the gesture means in every other tree on the
    /// platform, and it is what this tree does for catalogs, schemas and databases; a connection
    /// was the single exception, and the exception was the wrong way round. Editing a connection
    /// is a deliberate act with a form and a Save button, so it belongs in the context menu, where
    /// it already is.
    private func doubleClick() {
        switch node.kind {
        case .table: model.openTable(node)
        case .schema, .database: model.openObjects(node)
        default:
            model.selectedNodeID = node.id
            model.toggleExpansion(node)
        }
    }

    private var iconTint: Color {
        switch node.kind {
        case .group: Tone.secondary
        case .connection: node.color.color
        case .catalog: Tone.violet
        case .database: Tone.ice
        case .schema: Tone.amber
        case .table: Tone.secondary
        }
    }


    private var helpText: String {
        switch node.kind {
        case .group:
            "Group \(node.title) — holds connections. Right-click to rename or delete it."
        case .connection:
            "\(node.title) · \(node.connectionKind.label) — double-click to expand. Right-click to edit."
        case .catalog:
            "Catalog \(node.title) — expand to list schemas"
        case .database:
            "Database \(node.title) — expand to list tables"
        case .schema:
            "Schema \(node.title) — expand to list tables"
        case .table:
            node.insertableText?.appending(" — double-click to insert") ?? node.title
        }
    }
}
