import AppKit
import SwiftUI

/// The panel under the editor: the transaction log, the result's columns, and the files that
/// came out. Navicat's message pane, split into the three things this app actually produces.
struct BottomPanel: View {
    @Environment(AppModel.self) private var model
    @Bindable var tab: QueryTab

    var body: some View {
        VStack(spacing: 0) {
            header
            if !model.panelCollapsed {
                Rectangle().fill(.white.opacity(0.07)).frame(height: 1)
                Group {
                    switch tab.panel {
                    case .log: LogPanel(tab: tab)
                    case .columns: ColumnsPanel(tab: tab)
                    case .files:
                        if tab.destination == .table {
                            TablePanel(tab: tab)
                        } else {
                            FilesPanel(tab: tab)
                        }
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .frame(height: model.panelCollapsed ? Metrics.panelTabs : model.panelHeight)
        .background(Color.black.opacity(0.20))
        .overlay(alignment: .top) { Rectangle().fill(.white.opacity(0.07)).frame(height: 1) }
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
            IconButton(symbol: model.panelCollapsed ? "chevron.up" : "chevron.down",
                       help: model.panelCollapsed ? "Show the panel" : "Hide the panel", diameter: 20) {
                model.panelCollapsed.toggle()
            }
            .padding(.trailing, 4)
        }
        .padding(.leading, Metrics.gutter)
        .padding(.trailing, Metrics.gutter - 6)
        .frame(height: Metrics.panelTabs)
    }

    private func count(_ panel: PanelTab) -> Int? {
        switch panel {
        case .log: tab.logLines.isEmpty ? nil : tab.logLines.count
        case .columns: tab.columns.isEmpty ? nil : tab.columns.count
        case .files: tab.files.isEmpty ? nil : tab.files.count
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
                    .font(.system(size: 11, weight: selected ? .semibold : .regular))
                if let count {
                    Text("\(count)")
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(selected ? Tone.ice : Tone.secondary)
                        .padding(.horizontal, 5)
                        .padding(.vertical, 1)
                        .background(Color.white.opacity(0.08), in: Capsule())
                }
            }
            .foregroundStyle(selected ? .white : Tone.secondary)
            .padding(.horizontal, 9)
            .frame(height: 21)
            .background(Color.white.opacity(selected ? 0.13 : (hovering ? 0.06 : 0)),
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
            .font(.system(size: 11))
            .foregroundStyle(Tone.secondary)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

struct LogRow: View {
    let line: LogLine

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Text(line.at, format: .dateTime.hour().minute().second())
                .font(.system(size: 10.5, design: .monospaced))
                .foregroundStyle(.white.opacity(0.35))
                .frame(width: 56, alignment: .leading)
            Image(systemName: line.kind.symbol)
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(line.kind.tint)
                .frame(width: 12)
                .padding(.top, 2)
            Text(line.text)
                .font(.system(size: 11.5, design: .monospaced))
                .foregroundStyle(line.kind == .info ? .white.opacity(0.82) : line.kind.tint)
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
                    .font(.system(size: 10.5))
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
            .overlay(alignment: .top) { Rectangle().fill(.white.opacity(0.07)).frame(height: 1) }
        }
    }

    private func infoRow(_ label: String, _ value: String, monospaced: Bool = false) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text(label)
                .font(.system(size: 10.5, weight: .semibold))
                .tracking(0.6)
                .foregroundStyle(Tone.secondary)
                .frame(width: 96, alignment: .leading)
            Text(value)
                .font(monospaced ? .system(size: 12, design: .monospaced) : .system(size: 12))
                .foregroundStyle(.white.opacity(0.9))
                .textSelection(.enabled)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 0)
        }
    }
}

// MARK: Columns

/// The columns the coordinator reported, taken from the engine's `start` event — the result set
/// the export is about to write, named before a single row lands.
struct ColumnsPanel: View {
    @Bindable var tab: QueryTab

    var body: some View {
        if tab.columns.isEmpty {
            Text(tab.stage == .running
                 ? "Waiting for the coordinator's first page…"
                 : "Run the query to see its columns.")
                .font(.system(size: 11))
                .foregroundStyle(Tone.secondary)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(Array(tab.columns.enumerated()), id: \.offset) { index, column in
                        HStack(spacing: 10) {
                            Text("\(index + 1)")
                                .font(.mono12)
                                .foregroundStyle(.white.opacity(0.4))
                                .frame(width: 30, alignment: .trailing)
                            Text(column.name)
                                .font(.system(size: 12, design: .monospaced))
                                .lineLimit(1)
                            Spacer(minLength: 8)
                            Chip(text: column.type, tint: typeTint(column.type))
                        }
                        .padding(.vertical, 4)
                        .padding(.horizontal, Metrics.gutter)
                        .background(Color.white.opacity(index % 2 == 0 ? 0.03 : 0), in: Rectangle())
                    }
                }
                .padding(.vertical, 4)
            }
        }
    }

    /// DBAPI type codes, which is all the engine forwards. Grouped rather than named one by one:
    /// the exact code for every Trino type is the coordinator's business, not the UI's.
    private func typeTint(_ type: String) -> Color {
        guard let code = Int(type) else { return .white.opacity(0.6) }
        switch code {
        case 1, 12, -1, -9, -15, -16: return Tone.ice        // strings
        case -5, -6, 4, 5, 8, -7, 6, 7, 3: return Tone.mint  // numbers
        case 16: return Tone.violet                          // boolean
        case 91, 92, 93, -101, -102: return Tone.amber       // date/time
        default: return .white.opacity(0.6)
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
                    .font(.system(size: 11))
                    .foregroundStyle(Tone.secondary)
                if let directory = tab.outputDirectory {
                    Text(directory.path)
                        .font(.system(size: 10.5, design: .monospaced))
                        .foregroundStyle(.white.opacity(0.35))
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
                                    .font(.system(size: 12, design: .monospaced))
                                    .lineLimit(1)
                                    .truncationMode(.middle)
                                    .help(file.path)
                                Spacer(minLength: 8)
                                Text(byteText(file.bytes))
                                    .font(.system(size: 11, design: .monospaced))
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
                            .background(Color.white.opacity(index % 2 == 0 ? 0.03 : 0), in: Rectangle())
                        }
                    }
                    .padding(.vertical, 4)
                }
                HStack(spacing: 8) {
                    Text("\(pluralized(tab.files.count, "file")) · \(byteText(tab.totalBytes))")
                        .font(.system(size: 10.5))
                        .foregroundStyle(Tone.secondary)
                    Spacer()
                    PillButton(title: "Reveal in Finder", symbol: "magnifyingglass", compact: true) { tab.revealFiles() }
                        .keyboardShortcut("r", modifiers: [.command, .shift])
                }
                .padding(.horizontal, Metrics.gutter)
                .padding(.vertical, 6)
                .overlay(alignment: .top) { Rectangle().fill(.white.opacity(0.07)).frame(height: 1) }
            }
        }
    }
}
