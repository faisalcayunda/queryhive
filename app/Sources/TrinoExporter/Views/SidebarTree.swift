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
                            TreeRow(node: node, depth: 0, visible: visibleIDs)
                        }
                    }
                }
                .padding(.horizontal, 6)
                .padding(.vertical, 5)
            }
            hint
        }
        .background {
            Rectangle().fill(.thinMaterial)
            Rectangle().fill(Color.black.opacity(0.18))
        }
        .overlay(alignment: .trailing) { Rectangle().fill(.white.opacity(0.07)).frame(width: 1) }
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
                    .font(.system(size: 10.5, weight: .bold))
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
            .background(Color.black.opacity(0.28), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous).strokeBorder(.white.opacity(0.08)))
        }
        .padding(.horizontal, Metrics.gutter)
        .padding(.top, 12)
        .padding(.bottom, 9)
        .overlay(alignment: .bottom) { Rectangle().fill(.white.opacity(0.07)).frame(height: 1) }
    }

    private var hint: some View {
        Text("Double-click a table to insert it")
            .font(.system(size: 10))
            .foregroundStyle(.white.opacity(0.35))
            .lineLimit(1)
            .padding(.horizontal, Metrics.gutter)
            .padding(.vertical, 7)
            .frame(maxWidth: .infinity, alignment: .leading)
            .overlay(alignment: .top) { Rectangle().fill(.white.opacity(0.07)).frame(height: 1) }
    }

    private var emptyState: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("No connections yet.")
                .font(.system(size: 12, weight: .semibold))
            Text("Add a Trino coordinator to browse its catalogs.")
                .font(.system(size: 11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Button("New Connection") { model.presentConnectionEditor(nil) }
                .buttonStyle(.pill)
            Button("Add from URL…") { model.presentConnectionEditor(nil, startAtURL: true) }
                .buttonStyle(.pill)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .glass(12)
        .padding(.top, 4)
    }

    private var noMatches: some View {
        Text("Nothing loaded matches “\(model.treeFilter)”.")
            .font(.system(size: 11))
            .foregroundStyle(Tone.secondary)
            .fixedSize(horizontal: false, vertical: true)
            .padding(12)
    }
}

/// One tree row plus, when it is open, its children. The recursion goes through `AnyView`: a
/// `View` whose body contains itself is an infinitely sized type, and `AnyView` is what breaks
/// that cycle.
struct TreeRow: View {
    @Environment(AppModel.self) private var model
    @Bindable var node: TreeNode
    let depth: Int
    let visible: Set<String>?
    @State private var hovering = false

    private var isVisible: Bool { visible?.contains(node.id) ?? true }
    /// A filter auto-opens every level it can see, so a deep match is not hidden behind a node
    /// the user would have to expand by hand.
    private var showChildren: Bool { visible != nil || node.expanded }

    var body: some View {
        if isVisible {
            VStack(alignment: .leading, spacing: 1) {
                row
                if node.loading {
                    messageRow(symbol: nil, text: "Loading…", tint: Tone.secondary)
                } else if let error = node.error {
                    messageRow(symbol: "exclamationmark.triangle.fill", text: error, tint: Tone.coral)
                } else if showChildren, let children = node.children {
                    if children.isEmpty {
                        messageRow(symbol: nil, text: "Empty", tint: .white.opacity(0.3))
                    } else {
                        ForEach(children) { child in
                            AnyView(TreeRow(node: child, depth: depth + 1, visible: visible))
                        }
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
                .font(.system(size: 12, weight: node.kind == .connection ? .semibold : .regular))
                .lineLimit(1)
                .truncationMode(.middle)

            Spacer(minLength: 2)
        }
        .padding(.leading, CGFloat(depth) * Metrics.treeIndent + 6)
        .padding(.trailing, 7)
        .frame(height: Metrics.treeRow)
        .background(Color.white.opacity(selected ? 0.13 : (hovering ? 0.06 : 0)),
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
            Text(text).font(.system(size: 10.5)).foregroundStyle(tint).lineLimit(2)
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
            // connection profiles, no server-side "New Database", no groups or sharing. What is
            // left is what the app can actually do.
            Button("Open Connection") { model.expand(node) }
            Divider()
            Button("Edit Connection…") { model.presentConnectionEditor(node.connectionID) }
            Button("Duplicate Connection") { model.duplicateConnection(node.connectionID) }
            Button("Delete Connection…") { model.requestDelete(node.connectionID) }
            Divider()
            Button("New Connection") { model.presentConnectionEditor(nil) }
            Divider()
            // No keyboard shortcuts in here: this app's ⌘R is Run and ⌘T is already New Query
            // from the Query menu, so printing either beside a different action would mislead,
            // and declaring one twice can fire it twice.
            Button("New Query") { model.newTab(connectionID: node.connectionID) }
            Button("Open SQL File…") { model.runSQLFile(connectionID: node.connectionID) }
            Divider()
            Menu("Color") {
                ForEach(ConnectionColor.allCases) { color in
                    Button {
                        model.setColor(color, for: node.connectionID)
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
        case .catalog, .database, .schema:
            Button("Refresh") { model.refresh(node) }
            Button("Copy Name") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(node.title, forType: .string)
            }
            Divider()
            Button("New Query") { model.newTab(connectionID: node.connectionID) }
            Button("Open SQL File…") { model.runSQLFile(connectionID: node.connectionID) }
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

    private func doubleClick() {
        switch node.kind {
        case .table: model.insert(node)
        case .connection: model.presentConnectionEditor(node.connectionID)
        default:
            model.selectedNodeID = node.id
            model.toggleExpansion(node)
        }
    }

    private var iconTint: Color {
        switch node.kind {
        case .connection: node.color.color
        case .catalog: Tone.violet
        case .database: Tone.ice
        case .schema: Tone.amber
        case .table: Tone.secondary
        }
    }

    private var selected: Bool { model.selectedNodeID == node.id }

    private var helpText: String {
        switch node.kind {
        case .connection:
            "\(node.title) · \(node.connectionKind.label) — double-click to edit, or expand to browse"
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
