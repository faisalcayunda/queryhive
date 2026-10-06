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
        VStack(spacing: 0) {
            header
            // Favourites sit above the objects on purpose: the tree is a catalogue of what the
            // server has, and a favourite is a statement the user decided to keep.
            if !model.favouriteQueries.isEmpty {
                if model.favouriteQueries.count > 6 {
                    ScrollView { favourites.padding(.horizontal, 6) }.frame(height: 170)
                } else {
                    favourites.padding(.horizontal, 6)
                }
            }
            if model.tree.isEmpty {
                emptyState.padding(.horizontal, 6).padding(.vertical, 5)
                Spacer(minLength: 0)
            } else if let ids = visibleIDs, ids.isEmpty {
                noMatches.padding(.horizontal, 6).padding(.vertical, 5)
                Spacer(minLength: 0)
            } else {
                SchemaOutline(visible: visibleIDs)
            }
        }
        // The theme's canvas and a wash of it, and no system material: a material resolves to the
        // same grey in every theme. It runs up under the title bar (the traffic lights' row is
        // this surface) because the split view's sidebar is as tall as the window. The divider on
        // its right is the split view's own.
        .background {
            Tone.canvas.ignoresSafeArea()
            Rectangle().fill(Tone.ink.opacity(Tone.sidebarWash)).ignoresSafeArea()
        }
        // The sidebar shows favourites and the panel that reads them is not part of the launch path,
        // so the sidebar is what asks for them. Without this the section stays empty until the user
        // visits Saved, which is the opposite of within reach.
        .onAppear { model.loadSavedQueries() }
    }

    /// The saved queries the user keeps within reach.
    private var favourites: some View {
        FavouritesSection(queries: model.favouriteQueries) { query in
            // A favourite with no tab open has nowhere to go. The row is still drawn, so the list does
            // not change shape with the editor's state, but it does nothing.
            if let tab = model.selectedTab {
                model.loadIntoEditor(query.sql, in: tab)
            }
        }
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
                    .font(.ui(10, weight: .bold))
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
        .overlay(alignment: .bottom) { Rectangle().fill(Tone.hairline).frame(height: 1) }
    }

    private var emptyState: some View {
        VStack(alignment: .center, spacing: 10) {
            Text("No connections yet.")
                .font(.ui(12, weight: .semibold))
            Text("Add a connection to browse its catalogs, schemas and tables.")
                .font(.ui(11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
            PillButton(title: "New Connection", symbol: "plus") { model.presentConnectionEditor(nil) }
            PillButton(title: "Add from URL…", symbol: "link", role: .quiet) { model.presentConnectionEditor(nil, startAtURL: true) }
        }
        // Centered as a unit: the description wraps to several lines in a narrow column, and
        // `multilineTextAlignment` is what centers those lines, not the stack's alignment.
        .multilineTextAlignment(.center)
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .center)
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

/// The favourites section of the sidebar, on its own.
///
/// A view rather than a computed property on `SidebarTree` so it can be drawn without the tree. The
/// object rows are an `NSOutlineView`, which `ImageRenderer` cannot draw, so this is the part of the
/// sidebar an offscreen SwiftUI render can show.
struct FavouritesSection: View {
    let queries: [Event.SavedQuery]
    let load: (Event.SavedQuery) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            Text("FAVOURITES")
                .font(.ui(10, weight: .bold))
                .tracking(0.8)
                .foregroundStyle(Tone.secondary)
                .padding(.horizontal, 6)
                .padding(.bottom, 2)

            ForEach(queries) { query in
                Button { load(query) } label: {
                    HStack(spacing: 6) {
                        Image(systemName: "star.fill")
                            .font(.system(size: 8, weight: .bold))
                            .foregroundStyle(Tone.ice)
                        Text(query.name)
                            .font(.ui(11.5))
                            .foregroundStyle(Tone.ink.opacity(0.9))
                            .lineLimit(1)
                        Spacer(minLength: 4)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help(query.sql)
                .padding(.horizontal, 6)
                .padding(.vertical, 3)
            }

            Rectangle()
                .fill(Tone.ink.opacity(0.08))
                .frame(height: 1)
                .padding(.horizontal, 4)
                .padding(.vertical, 4)
        }
        .padding(.vertical, 4)
    }
}
