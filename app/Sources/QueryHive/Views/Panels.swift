import AppKit
import SwiftUI

/// The panel under the editor: the transaction log, the result's columns, and the files that
/// came out. Navicat's message pane, split into the three things this app actually produces.
struct BottomPanel: View {
    @Environment(AppModel.self) private var model
    @Bindable var tab: QueryTab
    /// The most this panel may take, so a window dragged small cannot squeeze the editor away.
    var ceiling: CGFloat?
    /// Take every point the workspace has left, rather than a fixed share of it.
    ///
    /// Set when the rows are meant to *be* the window (`panelExpanded`, which `openTable` turns on).
    /// Without it the panel was capped at `panelHeight` and the enclosing `VStack` centred that
    /// shorter block in the workspace, which left a band of nothing above the tab strip. Measured:
    /// a 516 pt stack in a 1560 pt workspace put the tab strip at y=295.
    var fills = false

    var body: some View {
        VStack(spacing: 0) {
            header
            if !model.panelCollapsed {
                Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1)
                Group {
                    switch tab.panel {
                    case .log: LogPanel(tab: tab)
                    case .result: ResultGrid(tab: tab)
                    case .files:
                        if tab.destination == .table {
                            TablePanel(tab: tab)
                        } else {
                            FilesPanel(tab: tab)
                        }
                    case .history: HistoryPanel(tab: tab)
                    case .saved: SavedQueriesPanel(tab: tab)
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .frame(height: stackedHeight)
        .frame(maxHeight: fills ? .infinity : nil)
        .background(Tone.recess.opacity(0.20))
        .overlay(alignment: .top) { Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1) }
    }

    /// `nil` when the panel is taking the whole workspace: a fixed height would win over the
    /// flexible frame beside it, because the fixed one is applied first.
    private var stackedHeight: CGFloat? {
        if fills { return nil }
        return model.panelCollapsed ? Metrics.panelTabs : min(model.panelHeight, ceiling ?? .infinity)
    }

    private var header: some View {
        HStack(spacing: 4) {
            ForEach(PanelTab.allCases) { panel in
                PanelTabButton(panel: panel, destination: tab.destination,
                               selected: tab.panel == panel, count: count(panel)) {
                    tab.panel = panel
                    model.panelCollapsed = false
                }
            }
            // The spinner belongs with the tabs it describes, not marooned on the far side of
            // the bar next to the collapse control — those are two unrelated things.
            if tab.stage == .running {
                ProgressView().controlSize(.mini).padding(.leading, 6)
            }
            Spacer()
            // When the rows hold the whole window this is a *minimise*, not a collapse: collapsing
            // a full-height panel would leave an empty window with a header at the bottom, which is
            // not a state anyone asked for. Both controls are never shown together.
            if model.panelExpanded {
                IconButton(symbol: "rectangle.compress.vertical",
                           help: "Minimise, and bring the editor back", diameter: 20) {
                    model.panelExpanded = false
                }
                .padding(.trailing, 4)
            } else {
                IconButton(symbol: model.panelCollapsed ? "chevron.up" : "chevron.down",
                           help: model.panelCollapsed ? "Show the panel" : "Hide the panel", diameter: 20) {
                    model.panelCollapsed.toggle()
                }
                .padding(.trailing, 4)
            }
        }
        .padding(.leading, Metrics.gutter)
        .padding(.trailing, Metrics.gutter - 6)
        .frame(height: Metrics.panelTabs)
    }

    private func count(_ panel: PanelTab) -> Int? {
        switch panel {
        case .log: tab.logLines.isEmpty ? nil : tab.logLines.count
        case .result: tab.preview.map(\.rowCount)
        case .files: tab.files.isEmpty ? nil : tab.files.count
        case .history: model.historyEntries.isEmpty ? nil : model.historyEntries.count
        case .saved: model.savedQueries.isEmpty ? nil : model.savedQueries.count
        }
    }
}

struct PanelTabButton: View {
    let panel: PanelTab
    let destination: Destination
    let selected: Bool
    let count: Int?
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 5) {
                Text(panel.label(for: destination))
                    .font(.ui(11, weight: selected ? .semibold : .regular))
                if let count {
                    Text("\(count)")
                        .font(.ui(10, weight: .semibold))
                        .foregroundStyle(selected ? Tone.accent : Tone.secondary)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .background(Tone.ink.opacity(0.08), in: Capsule())
                }
            }
            .foregroundStyle(selected ? Tone.ink : Tone.secondary)
            .padding(.horizontal, 9)
            .frame(height: 21)
            .background(Tone.ink.opacity(selected ? 0.13 : (hovering ? 0.06 : 0)),
                        in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            .contentShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
    }
}

// MARK: Log

/// The engine's JSON events, already translated into sentences by `AppModel.handle`, plus one
/// line per failure. Reads like a session transcript, which is what Navicat's Messages pane is.
struct LogPanel: View {
    @Bindable var tab: QueryTab

    var body: some View {
        if tab.logLines.isEmpty {
            empty("Nothing yet. Write a query and press Run.")
        } else {
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 2) {
                        ForEach(tab.logLines) { line in
                            LogRow(line: line).id(line.id)
                        }
                    }
                    .padding(.horizontal, Metrics.gutter)
                    .padding(.vertical, 8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .onChange(of: tab.logLines.count) { _, _ in
                    guard let last = tab.logLines.last else { return }
                    withAnimation(.easeOut(duration: 0.15)) { proxy.scrollTo(last.id, anchor: .bottom) }
                }
            }
        }
    }

    private func empty(_ text: String) -> some View {
        Text(text)
            .font(.ui(11))
            .foregroundStyle(Tone.secondary)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

struct LogRow: View {
    let line: LogLine

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Text(line.at, format: .dateTime.hour().minute().second())
                .font(.code(10.5))
                .foregroundStyle(Tone.ink.opacity(0.35))
                .frame(width: 56, alignment: .leading)
            Image(systemName: line.kind.symbol)
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(line.kind.tint)
                .frame(width: 12)
                .padding(.top, 2)
            Text(line.text)
                .font(.code(11.5))
                .foregroundStyle(line.kind == .info ? Tone.ink.opacity(0.82) : line.kind.tint)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(.vertical, 1)
    }
}

// MARK: Table destination

/// What a table run produced. A CTAS or INSERT is one statement the coordinator executes; no row
/// ever reaches this app, so there is no file list and no preview — the target, the mode and the
/// row count the coordinator reported are the entire result.
struct TablePanel: View {
    @Environment(AppModel.self) private var model
    @Bindable var tab: QueryTab

    private var driverKind: ConnectionKind { model.connection(for: tab)?.kind ?? .trino }

    private var rowsText: String {
        switch tab.stage {
        case .done: tab.rows.formatted()
        case .running: tab.rows > 0 ? tab.rows.formatted() : "…"
        case .idle, .failed: "—"
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    infoRow("Target", tab.writtenTable ?? tab.target(for: driverKind), monospaced: true)
                    infoRow("Statement", tab.writeMode.statement)
                    infoRow("Rows written", rowsText)
                    if let queryID = tab.queryID {
                        infoRow("Query", queryID, monospaced: true)
                    }
                }
                .padding(.horizontal, Metrics.gutter)
                .padding(.vertical, 10)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            HStack(spacing: 8) {
                Text(tab.isDestructive && tab.stage != .done
                     ? "Replace drops the existing table before the query runs."
                     : "Trino writes the rows itself; none of them travel through this app.")
                    .font(.ui(10.5))
                    .foregroundStyle(tab.isDestructive && tab.stage != .done ? Tone.coral : Tone.secondary)
                    .lineLimit(1)
                Spacer()
                PillButton(title: "Copy Name", symbol: "doc.on.doc", compact: true) {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(tab.writtenTable ?? tab.target(for: driverKind), forType: .string)
                }
                .disabled(tab.trimmedTable.isEmpty)
            }
            .padding(.horizontal, Metrics.gutter)
            .padding(.vertical, 6)
            .overlay(alignment: .top) { Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1) }
        }
    }

    private func infoRow(_ label: String, _ value: String, monospaced: Bool = false) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text(label)
                .font(.ui(10.5, weight: .semibold))
                .tracking(0.6)
                .foregroundStyle(Tone.secondary)
                .frame(width: 96, alignment: .leading)
            Text(value)
                .font(monospaced ? .system(size: 12, design: .monospaced) : .system(size: 12))
                .foregroundStyle(Tone.ink.opacity(0.9))
                .textSelection(.enabled)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 0)
        }
    }
}

// MARK: Files

/// What actually landed on disk, with byte counts and a Reveal button per file.
struct FilesPanel: View {
    @Bindable var tab: QueryTab

    var body: some View {
        if tab.files.isEmpty {
            VStack(spacing: 6) {
                Text(tab.stage == .running ? "Writing…" : "No files yet.")
                    .font(.ui(11))
                    .foregroundStyle(Tone.secondary)
                if let directory = tab.outputDirectory {
                    Text(directory.path)
                        .font(.code(10.5))
                        .foregroundStyle(Tone.ink.opacity(0.35))
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            VStack(spacing: 0) {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(Array(tab.files.enumerated()), id: \.offset) { index, file in
                            HStack(spacing: 9) {
                                Image(systemName: "doc.fill")
                                    .font(.system(size: 10))
                                    .foregroundStyle(Tone.mint)
                                    .frame(width: 14)
                                Text(file.name)
                                    .font(.code(12))
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                                    .help(file.path)
                                Spacer(minLength: 8)
                                Text(byteText(file.bytes))
                                    .font(.code(11))
                                    .foregroundStyle(Tone.secondary)
                                Button {
                                    NSWorkspace.shared.activateFileViewerSelecting([file.url])
                                } label: {
                                    Image(systemName: "magnifyingglass.circle")
                                        .font(.system(size: 12))
                                        .foregroundStyle(Tone.secondary)
                                }
                                .buttonStyle(.plain)
                                .help("Reveal \(file.name) in Finder")
                            }
                            .padding(.vertical, 4)
                            .padding(.horizontal, Metrics.gutter)
                            .background(Tone.ink.opacity(index % 2 == 0 ? 0.03 : 0), in: Rectangle())
                        }
                    }
                    .padding(.vertical, 4)
                }
                HStack(spacing: 8) {
                    Text("\(pluralized(tab.files.count, "file")) · \(byteText(tab.totalBytes))")
                        .font(.ui(10.5))
                        .foregroundStyle(Tone.secondary)
                    Spacer()
                    PillButton(title: "Reveal in Finder", symbol: "magnifyingglass", compact: true) { tab.revealFiles() }
                        .keyboardShortcut("r", modifiers: [.command, .shift])
                }
                .padding(.horizontal, Metrics.gutter)
                .padding(.vertical, 6)
                .overlay(alignment: .top) { Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1) }
            }
        }
    }
}

// MARK: History and saved queries

/// A failure line for the two library panels.
///
/// In place rather than in an alert, because reading the history is never urgent enough to
/// interrupt what the user is typing, and a panel that silently showed nothing would look exactly
/// like an empty history.
private func libraryBanner(_ text: String) -> some View {
    HStack(spacing: 6) {
        Image(systemName: "exclamationmark.triangle.fill")
            .font(.system(size: 9, weight: .bold))
            .foregroundStyle(Tone.coral)
        Text(text)
            .font(.ui(10.5))
            .foregroundStyle(Tone.coral)
            .lineLimit(2)
        Spacer()
    }
    .padding(.horizontal, Metrics.gutter)
    .padding(.vertical, 4)
}

private func libraryEmpty(_ text: String) -> some View {
    Text(text)
        .font(.ui(11))
        .foregroundStyle(Tone.secondary)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
}

/// Every execution the engine has written down, newest first.
///
/// The list is the engine's rather than this tab's: it spans connections and tabs, which is why it
/// lives on `AppModel` and why the badge counts the same rows whichever tab is in front. Clicking a
/// row puts its statement back in the editor, which is the whole reason to keep a history.
struct HistoryPanel: View {
    @Environment(AppModel.self) private var model
    var tab: QueryTab

    // The search term lives on `AppModel`, not in `@State` here, and the reason is not tidiness: a
    // finished Run re-reads the list, and that re-read has to carry the term the user is looking
    // at. A term owned by this view could only be handed over from outside, and would arrive
    // stale. The panel still asks the engine rather than filtering the rows it holds: FTS5 is what
    // makes "the statements containing these words" answerable, and a filter written here would be
    // a second answer to the same question, free to disagree with the first.

    var body: some View {
        VStack(spacing: 0) {
            if let failure = model.historyError { libraryBanner(failure) }
            searchField
            if model.historyEntries.isEmpty {
                libraryEmpty(model.historySearch.isEmpty
                             ? "Nothing yet. Every Run is recorded here."
                             : "Nothing matches \(model.historySearch).")
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 1) {
                        ForEach(model.historyEntries) { entry in
                            HistoryRow(entry: entry,
                                       connection: model.connectionName(for: entry.connectionId)) {
                                model.loadIntoEditor(entry.sql, in: tab)
                            }
                        }
                    }
                    .padding(.vertical, 4)
                }
            }
        }
        // A keystroke cancels the task the one before it started, so typing a word starts one read
        // rather than one per letter. A read already running is not cancelled by that, which is why
        // `loadHistory` carries a token of its own. It also runs on the first appearance, which is
        // why there is no separate `onAppear`.
        .task(id: model.historySearch) {
            try? await Task.sleep(for: .milliseconds(250))
            guard !Task.isCancelled else { return }
            model.loadHistory(search: model.historySearch)
        }
    }

    /// A `Binding` to the model's own field, so the text field writes the one place the term lives.
    private var searchBinding: Binding<String> {
        Binding(get: { model.historySearch }, set: { model.historySearch = $0 })
    }

    private var searchField: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(Tone.secondary)
            TextField("Search statements", text: searchBinding)
                .textFieldStyle(.plain)
                .font(.ui(11))
            if !model.historySearch.isEmpty {
                IconButton(symbol: "xmark.circle.fill", help: "Clear the search", diameter: 18) {
                    model.historySearch = ""
                }
            }
        }
        .padding(.horizontal, Metrics.gutter)
        .padding(.vertical, 5)
        .overlay(alignment: .bottom) { Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1) }
    }
}

/// One execution in the History panel.
///
/// Internal rather than private so a render test can draw it on its own, for the same reason
/// `SavedQueryRow` is: the panel's rows sit in a `LazyVStack` inside a `ScrollView`, and an offscreen
/// render gives that no viewport, so the rows never draw as part of the panel.
struct HistoryRow: View {
    let entry: Event.HistoryEntry
    /// The connection's name, when the row has one the app still knows about.
    let connection: String?
    let load: () -> Void
    @State private var hovering = false

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: symbol)
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(tint)
                .frame(width: 12)
                .padding(.top, 3)
            VStack(alignment: .leading, spacing: 2) {
                Text(entry.sql)
                    .font(.code(11.5))
                    .foregroundStyle(Tone.ink.opacity(0.85))
                    .lineLimit(2)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Text(detail)
                    .font(.ui(10))
                    .foregroundStyle(Tone.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
            Text(started, format: .dateTime.month(.abbreviated).day().hour().minute())
                .font(.code(10))
                .foregroundStyle(Tone.ink.opacity(0.35))
        }
        .padding(.horizontal, Metrics.gutter)
        .padding(.vertical, 4)
        .background(hovering ? Tone.ink.opacity(0.05) : .clear)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .onTapGesture(perform: load)
    }

    private var started: Date {
        Date(timeIntervalSince1970: Double(entry.startedAt) / 1000)
    }

    private var symbol: String {
        switch entry.outcome {
        case "ok": "checkmark.circle.fill"
        case "error": "exclamationmark.circle.fill"
        case "cancelled": "slash.circle.fill"
        default: "clock"
        }
    }

    private var tint: Color {
        switch entry.outcome {
        case "ok": Tone.mint
        case "error": Tone.coral
        case "cancelled": Tone.amber
        default: Tone.gray
        }
    }

    private var detail: String {
        var parts: [String] = []
        // The connection comes first: the same statement against two databases is two different
        // facts, and it is what tells two otherwise identical rows apart.
        if let connection { parts.append(connection) }
        switch entry.outcome {
        case "ok": parts.append("ok")
        case "error": parts.append("error")
        case "cancelled": parts.append("cancelled")
        default: parts.append("running")
        }
        if let rows = entry.rowCount { parts.append(pluralized(rows, "row")) }
        if let elapsed = entry.elapsedMs { parts.append("\(elapsed) ms") }
        if let error = entry.error { parts.append(error) }
        return parts.joined(separator: " · ")
    }
}

/// The statements the user chose to keep.
///
/// Saving reads the statement from the editor rather than from the grid's last run, because the
/// thing worth keeping is what is on screen now, not what happened to run last.
struct SavedQueriesPanel: View {
    @Environment(AppModel.self) private var model
    var tab: QueryTab
    /// The inline name field, shown only while saving. A field in the panel rather than a sheet:
    /// the panel is already the thing the user is looking at, and a sheet for one word is a window
    /// to dismiss.
    @State private var naming = false
    @State private var name = ""

    var body: some View {
        VStack(spacing: 0) {
            if let failure = model.savedError { libraryBanner(failure) }
            toolbar
            if model.savedQueries.isEmpty {
                libraryEmpty("Nothing saved yet. Write a query, then Save current.")
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 1) {
                        ForEach(model.savedQueries) { query in
                            SavedQueryRow(query: query,
                                          load: { model.loadIntoEditor(query.sql, in: tab) },
                                          delete: { model.deleteSavedQuery(query.id) },
                                          toggleFavourite: { model.toggleFavourite(query) })
                        }
                    }
                    .padding(.vertical, 4)
                }
            }
        }
        .onAppear { model.loadSavedQueries() }
    }

    private var toolbar: some View {
        HStack(spacing: 8) {
            if naming {
                TextField("Name", text: $name)
                    .textFieldStyle(.plain)
                    .font(.ui(11))
                    .frame(width: 220)
                    .onSubmit(commit)
                PillButton(title: "Save", symbol: "checkmark", compact: true, action: commit)
                    .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                IconButton(symbol: "xmark", help: "Cancel", diameter: 20) {
                    naming = false
                    name = ""
                }
            }
            Spacer()
            PillButton(title: "Save current", symbol: "square.and.arrow.down", compact: true) {
                name = ""
                naming = true
            }
            .help("Save the editor's statement under a name")
        }
        .padding(.horizontal, Metrics.gutter)
        .padding(.vertical, 5)
    }

    private func commit() {
        // A name that is only whitespace is refused here as well as in `saveQuery`, so the button
        // never looks like it did something.
        guard model.saveQuery(tab, named: name) else { return }
        naming = false
        name = ""
    }
}

/// One saved query in the Saved panel.
///
/// Internal rather than private so a render test can draw it on its own. The panel keeps its rows in
/// a `LazyVStack` inside a `ScrollView`, and an offscreen render gives that no viewport, so the rows
/// never draw when the whole panel is rendered. Standing alone this one does, and the star is the
/// part of this panel worth looking at.
struct SavedQueryRow: View {
    let query: Event.SavedQuery
    let load: () -> Void
    let delete: () -> Void
    let toggleFavourite: () -> Void
    @State private var hovering = false

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            // The lead glyph is the star, and it is always drawn. A favourite that only revealed
            // itself under the pointer would be telling nobody anything, and the decorative bookmark
            // this replaced was never doing any work.
            Button(action: toggleFavourite) {
                Image(systemName: query.favourite ? "star.fill" : "star")
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(query.favourite ? Tone.ice : Tone.secondary)
                    .frame(width: 14)
            }
            .buttonStyle(.plain)
            .help(query.favourite ? "Remove from favourites" : "Keep in the sidebar")
            .padding(.top, 2)
            VStack(alignment: .leading, spacing: 2) {
                Text(query.name)
                    .font(.ui(11.5, weight: .semibold))
                    .foregroundStyle(Tone.ink.opacity(0.9))
                Text(query.sql)
                    .font(.code(11))
                    .foregroundStyle(Tone.secondary)
                    .lineLimit(2)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
            // The tap target is the text block, not the whole row. On the row it swallowed the two
            // buttons beside it, so a click on Delete ran `load` as well and deleting a query also
            // put its statement back in the editor. A gesture and a `Button` in one subtree have no
            // priority relationship to appeal to, so the fix is to keep them out of each other's
            // way rather than to order them.
            .contentShape(Rectangle())
            .onTapGesture(perform: load)
            Spacer(minLength: 8)
            if hovering {
                IconButton(symbol: "arrow.up.forward.app", help: "Load into the editor",
                           diameter: 20, action: load)
                IconButton(symbol: "trash", help: "Delete", diameter: 20, action: delete)
            }
        }
        .padding(.horizontal, Metrics.gutter)
        .padding(.vertical, 4)
        .background(hovering ? Tone.ink.opacity(0.05) : .clear)
        .onHover { hovering = $0 }
    }
}
