import SwiftUI

/// The collapsible half of the cell reader: a JSON value as a tree, with a search field.
///
/// Everything it draws comes from `JSONTree`, which is where the parse limit and the node cap live;
/// this view only decides what is open and which row a search highlighted. The tree is a reader,
/// not an editor: there is no way to change a value from here, because the value belongs to the
/// server and the grid is the only place this app changes one.
struct JSONTreeView: View {
    let root: JSONTree.Node

    /// The ids of the containers that are open. The root starts open and its members start shut:
    /// a document that opens fully expanded is the wall of braces again, only indented.
    @State private var expanded: Set<Int>
    @State private var query = ""
    @State private var search: JSONTree.Search?

    init(root: JSONTree.Node) {
        self.root = root
        _expanded = State(initialValue: [root.id])
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            field
            ScrollView([.horizontal, .vertical]) {
                LazyVStack(alignment: .leading, spacing: 0) {
                    JSONTreeRow(node: root, depth: 0, expanded: $expanded, search: search)
                }
                .padding(.vertical, 2)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
        .onChange(of: query) { _, _ in refreshSearch() }
    }

    private var field: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 10))
                .foregroundStyle(Tone.secondary)
            TextField("Search keys and values", text: $query)
                .textFieldStyle(.plain)
                .font(.ui(11.5))
            if let search {
                Text(search.hits.isEmpty ? "No matches" : "\(search.hits.count)")
                    .font(.code(10.5))
                    .foregroundStyle(search.hits.isEmpty ? Tone.coral : Tone.secondary)
            }
            if !query.isEmpty {
                Button { query = "" } label: {
                    Image(systemName: "xmark.circle.fill").font(.system(size: 10))
                }
                .buttonStyle(.plain)
                .foregroundStyle(Tone.secondary)
                .help("Clear the search")
            }
        }
        .padding(.horizontal, 8)
        .frame(height: 24)
        .background(Tone.recess.opacity(0.30), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
            .strokeBorder(Tone.ink.opacity(0.10)))
    }

    /// A new query opens every container a hit sits under, so a match deep in an object is visible
    /// rather than merely counted. Clearing the search does not collapse them again: the user's
    /// folds are theirs, and undoing them on a keystroke would be the view fighting back.
    private func refreshSearch() {
        let found = JSONTree.search(in: root, query: query)
        search = found
        if let found { expanded.formUnion(found.expand) }
    }
}

/// One row, and its children when it is open.
///
/// A `struct` rather than a recursive function so a container can draw rows of the same type
/// without an opaque return type recursion to satisfy the compiler.
private struct JSONTreeRow: View {
    let node: JSONTree.Node
    let depth: Int
    @Binding var expanded: Set<Int>
    let search: JSONTree.Search?

    var body: some View {
        if node.isMarker {
            Text(node.value)
                .font(.code(11))
                .foregroundStyle(Tone.secondary)
                .padding(.leading, indent)
                .padding(.vertical, 2)
        } else {
            let isOpen = expanded.contains(node.id)
            VStack(alignment: .leading, spacing: 0) {
                row(isOpen: isOpen)
                if node.isContainer, isOpen {
                    ForEach(node.children) { child in
                        JSONTreeRow(node: child, depth: depth + 1, expanded: $expanded, search: search)
                    }
                }
            }
        }
    }

    private func row(isOpen: Bool) -> some View {
        HStack(spacing: 5) {
            if node.isContainer {
                Button {
                    if isOpen { expanded.remove(node.id) } else { expanded.insert(node.id) }
                } label: {
                    Image(systemName: isOpen ? "chevron.down" : "chevron.right")
                        .font(.system(size: 9, weight: .bold))
                        .frame(width: 12)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(Tone.secondary)
                .help(isOpen ? "Collapse" : "Expand")
            } else {
                Color.clear.frame(width: 12, height: 1)
            }

            if !node.label.isEmpty {
                Text(node.label)
                    .font(.code(11.5, weight: .semibold))
                    .foregroundStyle(node.isContainer ? Tone.ice : Tone.ink)
                    .lineLimit(1)
            }

            Text(JSONTree.display(node))
                .font(.code(11.5))
                .foregroundStyle(highlighted ? Tone.amber : Tone.ink.opacity(0.85))
                .lineLimit(1)
                .truncationMode(.middle)
                .help(node.kind == .string ? node.value : JSONTree.display(node))
        }
        .padding(.leading, indent)
        .padding(.vertical, 2)
        .frame(height: 18, alignment: .leading)
    }

    /// Whether a search hit lands on this row: a hit itself, or a container on the way to one.
    private var highlighted: Bool {
        guard let search else { return false }
        return search.hits.contains(node.id)
    }

    private var indent: CGFloat { CGFloat(depth) * 14 + 4 }
}
