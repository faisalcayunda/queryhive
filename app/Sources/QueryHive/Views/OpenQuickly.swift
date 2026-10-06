import SwiftUI

/// Open Quickly: one field, and everything the app already knows about.
///
/// It searches what is in memory — the object tree's loaded nodes, the saved queries and the
/// history — rather than the server, so typing never opens a connection. Picking a node reveals it
/// in the tree; picking a statement puts it in the tab in front. `QuickSearch` is the ranking, and
/// it is a pure function so the order can be tested without this view.
struct OpenQuickly: View {
    @Environment(AppModel.self) private var model
    @FocusState private var fieldFocused: Bool
    @State private var scope = QuickScope.all

    /// Computed here, not through `AppModel.quickResults`, so the scope can filter it without a
    /// model change. ponytail: fold into `AppModel+Focus` (`quicklyScope`) when the split lands.
    private var results: [QuickResult] {
        QuickSearch.results(query: model.openQuicklyQuery, nodes: model.allNodes(),
                            savedQueries: model.savedQueries, history: model.historyEntries,
                            scope: scope)
    }

    private func moveHighlight(by delta: Int, in results: [QuickResult]) {
        guard !results.isEmpty else { return }
        model.openQuicklyIndex = min(max(model.openQuicklyIndex + delta, 0), results.count - 1)
    }

    var body: some View {
        @Bindable var model = model
        let results = results
        VStack(spacing: 0) {
            HStack(spacing: 9) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(Tone.secondary)
                TextField("Search objects, saved queries and history",
                          text: $model.openQuicklyQuery)
                    .textFieldStyle(.plain)
                    .font(.ui(14))
                    .foregroundStyle(Tone.ink)
                    .focused($fieldFocused)
                    .onSubmit { pick(results) }
                    .onKeyPress(.upArrow) { moveHighlight(by: -1, in: results); return .handled }
                    .onKeyPress(.downArrow) { moveHighlight(by: 1, in: results); return .handled }
                    .onKeyPress(keys: [.tab]) { press in
                        scope = scope.rotated(by: press.modifiers.contains(.shift) ? -1 : 1)
                        return .handled
                    }
                    .accessibilityLabel("Open Quickly")
                    .onKeyPress(.escape) { model.openQuicklyOpen = false; return .handled }
                Text("Tab changes scope")
                    .font(.ui(11))
                    .foregroundStyle(Tone.secondary)
                Text("\(results.count)")
                    .font(.code(11))
                    .foregroundStyle(Tone.secondary)
            }
            .padding(.horizontal, 14)
            .frame(height: 46)

            Segmented(selection: $scope, options: QuickScope.allCases, label: \.title)
                .padding(.horizontal, 10)
                .padding(.bottom, 8)

            Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1)

            if results.isEmpty {
                Text("Nothing matches. Open Quickly searches the object tree you have loaded, your "
                     + "saved queries and the history, not the server.")
                    .font(.ui(11.5))
                    .foregroundStyle(Tone.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(14)
            } else {
                ScrollViewReader { proxy in
                    ScrollView {
                        LazyVStack(spacing: 1) {
                            ForEach(Array(results.enumerated()), id: \.element.id) { index, result in
                                row(result, selected: index == model.openQuicklyIndex)
                                    .id(result.id)
                                    .onTapGesture { model.applyQuickResult(result) }
                                    .accessibilityElement(children: .combine)
                                    .accessibilityValue("\(index + 1) of \(results.count)")
                                    .accessibilityAddTraits(index == model.openQuicklyIndex
                                                            ? .isSelected : [])
                            }
                        }
                        .padding(6)
                    }
                    .accessibilityLabel("Results")
                    // No anchor: the list scrolls the least it can to bring the row into view.
                    .onChange(of: model.openQuicklyIndex) {
                        if results.indices.contains(model.openQuicklyIndex) {
                            proxy.scrollTo(results[model.openQuicklyIndex].id)
                        }
                    }
                }
                .frame(maxHeight: 320)
            }
        }
        .frame(width: 560)
        .background(Tone.canvas, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous)
            .strokeBorder(Tone.ink.opacity(0.16)))
        .shadow(color: .black.opacity(0.35), radius: 26, y: 14)
        .onAppear { fieldFocused = true }
        // The highlight is an index into the results, so a new query starts it over rather than
        // leaving it past the end of a shorter list.
        .onChange(of: model.openQuicklyQuery) { model.openQuicklyIndex = 0 }
        .onChange(of: scope) { model.openQuicklyIndex = 0 }
    }

    private func pick(_ results: [QuickResult]) {
        guard results.indices.contains(model.openQuicklyIndex) else { return }
        model.applyQuickResult(results[model.openQuicklyIndex])
    }

    private func row(_ result: QuickResult, selected: Bool) -> some View {
        HStack(spacing: 10) {
            Image(systemName: result.symbol)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(selected ? Tone.ink : Tone.secondary)
                .frame(width: 18)
            VStack(alignment: .leading, spacing: 1) {
                Text(result.title)
                    .font(.ui(12.5, weight: selected ? .semibold : .regular))
                    .foregroundStyle(Tone.ink)
                    .lineLimit(1)
                Text(result.subtitle)
                    .font(.code(11))
                    .foregroundStyle(Tone.ink.opacity(0.6))
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(selected ? Tone.accent.opacity(0.16) : .clear,
                    in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        .contentShape(Rectangle())
    }
}
