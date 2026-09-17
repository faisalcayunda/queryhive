import AppKit
import SwiftUI

/// The tabbed query workspace: the tab strip, the query toolbar, the SQL editor, and the panel
/// under it. Navicat's query window, dressed in the CleanMyMac palette.
struct Workspace: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 0) {
            if let tab = model.selectedTab {
                TabStrip()
                QueryToolbar(tab: tab)
                // Keyed by tab: sharing one editor across tabs would carry the previous query's
                // undo stack and scroll position into the next one.
                EditorPane(tab: tab).id(tab.id)
                PanelResizer()
                BottomPanel(tab: tab)
            } else {
                EmptyWorkspace()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .background(Backdrop(hue: .exporter))
    }
}

/// Shown when every tab has been closed.
struct EmptyWorkspace: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 14) {
            HiveHero(size: 190)
            Text("No query open").font(.heroTitle)
            Text("Open a query tab, write SQL, and Run streams the result straight to disk.")
                .font(.body13)
                .foregroundStyle(Tone.secondary)
                .multilineTextAlignment(.center)
            HubButton(title: "New Query", symbol: "plus", hue: .exporter) { model.newTab() }
                .keyboardShortcut("t", modifiers: .command)
                .padding(.top, 4)
        }
        .padding(40)
    }
}

// MARK: Tab strip

struct TabStrip: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        HStack(spacing: 0) {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 2) {
                    ForEach(model.tabs) { tab in
                        TabChip(tab: tab)
                    }
                    // Inside the scroller, right after the last tab: a "+" pinned to the far
                    // edge reads as belonging to the window, not to the tab strip.
                    Button { model.newTab() } label: {
                        Image(systemName: "plus")
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(Tone.secondary)
                            .frame(width: 24, height: 24)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .help("New Query (⌘T)")
                }
                .padding(.horizontal, Metrics.gutter)
                .padding(.vertical, 4)
            }
        }
        .frame(height: Metrics.tabStrip)
        .background(Color.black.opacity(0.22))
        .overlay(alignment: .bottom) { Rectangle().fill(.white.opacity(0.07)).frame(height: 1) }
    }
}

struct TabChip: View {
    @Environment(AppModel.self) private var model
    let tab: QueryTab
    @State private var hovering = false

    private var selected: Bool { model.selectedTabID == tab.id }

    var body: some View {
        HStack(spacing: 7) {
            Circle().fill(dot).frame(width: 6, height: 6)
            Text(tab.title)
                .font(.system(size: 12, weight: selected ? .semibold : .regular))
                .lineLimit(1)
                .foregroundStyle(selected ? .white : .white.opacity(0.75))
            Button { model.closeTab(tab.id) } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundStyle(Tone.secondary)
                    .frame(width: 15, height: 15)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .opacity(hovering || selected ? 1 : 0)
            .help("Close \(tab.title)")
        }
        .padding(.horizontal, 10)
        .frame(height: 27)
        .background(Color.white.opacity(selected ? 0.13 : (hovering ? 0.06 : 0)),
                    in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous)
            .strokeBorder(selected ? Tone.ice.opacity(0.35) : .clear))
        .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
        .onTapGesture { model.selectTab(tab.id) }
        .onHover { hovering = $0 }
        .help(tab.summary)
    }

    private var dot: Color {
        switch tab.stage {
        case .idle: Tone.gray.opacity(0.55)
        case .running: Tone.ice
        case .done: Tone.mint
        case .failed: Tone.coral
        }
    }
}

// MARK: Toolbar

struct QueryToolbar: View {
    @Environment(AppModel.self) private var model
    @Bindable var tab: QueryTab
    @State private var confirmReplace = false

    var body: some View {
        HStack(spacing: 8) {
            actionButton

            ToolbarSeparator()

            // Name only: the host and catalog are already on the title strip and the status
            // bar, and repeating them here just truncated the name that identifies the choice.
            ConnectionPickerButton(selection: $tab.connectionID)
                .frame(minWidth: 128, idealWidth: 176, maxWidth: 176)

            ToolbarSeparator()

            Segmented(selection: $tab.destination, options: [Destination.file, .table]) { $0.label }
                .frame(width: 116)
                .help("Write the result to a file, or have Trino write it into a table")

            switch tab.destination {
            case .file: fileControls
            case .table: tableControls
            }

            Spacer(minLength: 8)
        }
        .padding(.horizontal, Metrics.gutter)
        .frame(height: Metrics.toolbar)
        .background(Color.white.opacity(0.03))
        .overlay(alignment: .bottom) { Rectangle().fill(.white.opacity(0.07)).frame(height: 1) }
        .onChange(of: tab.destination) { _, destination in
            if destination == .table { model.prepareTableDestination(tab) }
        }
        .confirmationDialog("Replace \(tab.target(for: model.connection(for: tab)?.kind ?? .trino))?", isPresented: $confirmReplace) {
            Button("Drop and Recreate", role: .destructive) { model.run(tab) }
        } message: {
            Text("The existing table is dropped before the query runs. If the query then fails, the table is already gone.")
        }
    }

    /// Run for a file, Save for a table: a table run can drop something, and the label should not
    /// hide that behind the same word as writing a CSV.
    @ViewBuilder private var actionButton: some View {
        if tab.stage == .running {
            HubButton(title: tab.stopping ? "Stopping…" : "Stop", symbol: "stop.fill", hue: .failure) {
                model.stop(tab)
            }
            .disabled(tab.stopping)
            .keyboardShortcut(".", modifiers: .command)
            .help("Stop the run (⌘.)")
        } else {
            HubButton(title: tab.destination == .table ? "Save" : "Run",
                      symbol: tab.destination == .table ? "square.and.arrow.down" : "play.fill",
                      hue: tab.isDestructive ? .failure : .exporter) {
                if tab.isDestructive { confirmReplace = true } else { model.run(tab) }
            }
            .disabled(model.runBlockedReason(for: tab) != nil)
            .keyboardShortcut("r", modifiers: .command)
            .help(model.runBlockedReason(for: tab) ?? (tab.destination == .table
                                                       ? "Run the query and save it as a table (⌘R)"
                                                       : "Run the query and write the file (⌘R)"))
        }
    }

    @ViewBuilder private var fileControls: some View {
        FormatPickerButton(tab: tab)
        FolderButton(tab: tab)
        HStack(spacing: 6) {
            Image(systemName: "doc.text").font(.system(size: 10)).foregroundStyle(Tone.secondary)
            TextField("export", text: $tab.outputName)
                .toolbarField()
                .frame(minWidth: 104, idealWidth: 132, maxWidth: 132)
        }
        .help("Base file name, without the extension")
    }

    @ViewBuilder private var tableControls: some View {
        // One button instead of four fields. Catalog, schema, table and mode do not fit across a
        // toolbar at the window's minimum width, and the value that would get truncated first is
        // the table name — the one thing the user must read back before Replace drops it.
        TableTargetButton(tab: tab)
            .frame(minWidth: 176, idealWidth: 260, maxWidth: 300)
        WriteModeButton(mode: $tab.writeMode)
    }
}

/// The table destination's target, collapsed into a single toolbar button. Mirrors the file
/// destination's format button: the toolbar names what will happen, the popover is where the
/// detail lives.
struct TableTargetButton: View {
    @Environment(AppModel.self) private var model
    @Bindable var tab: QueryTab
    @State private var showing = false

    private var driverKind: ConnectionKind { model.connection(for: tab)?.kind ?? .trino }

    private var summary: String {
        let target = tab.target(for: driverKind)
        return target.isEmpty ? "Choose a target…" : target
    }

    private var catalogs: [String] {
        model.loadedNames(for: tab.connectionID, kind: driverKind == .mysql ? .database : .catalog)
    }

    private var schemas: [String] {
        model.loadedNames(for: tab.connectionID, kind: .schema, database: tab.trimmedCatalog)
    }

    var body: some View {
        Button { showing.toggle() } label: {
            HStack(spacing: 7) {
                Image(systemName: "tablecells")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(Tone.mint)
                Text(summary)
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(tab.trimmedTable.isEmpty ? Tone.secondary : .white)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 4)
                Image(systemName: "chevron.down")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundStyle(Tone.secondary)
            }
            .padding(.horizontal, 9)
            .frame(maxWidth: .infinity)
            .frame(height: Metrics.control)
            .background(Color.black.opacity(0.30), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).strokeBorder(.white.opacity(0.10)))
            .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .buttonStyle(.plain)
        .help("Where \(driverKind.label) writes the query's rows")
        .popover(isPresented: $showing, arrowEdge: .bottom) { panel }
    }

    private var panel: some View {
        VStack(alignment: .leading, spacing: 12) {
            SectionLabel(text: "Target table")
            // Only the fields this driver has. Postgres writes inside the database its connection
            // already opened, so a catalog field would be a lie; MySQL has no schema level.
            switch driverKind {
            case .trino:
                LabeledField("Catalog") {
                    ComboField(placeholder: "hive", text: $tab.targetCatalog, options: catalogs, width: 332)
                }
                LabeledField("Schema") {
                    ComboField(placeholder: "analytics", text: $tab.targetSchema, options: schemas, width: 332)
                }
            case .postgres:
                LabeledField("Schema") {
                    ComboField(placeholder: "public", text: $tab.targetSchema, options: schemas, width: 332)
                }
            case .mysql:
                LabeledField("Database") {
                    ComboField(placeholder: "mydb", text: $tab.targetCatalog, options: catalogs, width: 332)
                }
            }
            LabeledField("Table") {
                TextField("penerima_manfaat_2026", text: $tab.targetTable).field()
            }
            Divider().overlay(.white.opacity(0.08))
            SectionLabel(text: "If the table already exists")
            ForEach(WriteMode.allCases) { mode in
                Button { tab.writeMode = mode } label: {
                    HStack(alignment: .top, spacing: 9) {
                        Image(systemName: tab.writeMode == mode ? "checkmark.circle.fill" : "circle")
                            .font(.system(size: 12))
                            .foregroundStyle(tab.writeMode == mode ? mode.tint : .white.opacity(0.3))
                            .padding(.top, 1)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(mode.statement)
                                .font(.system(size: 11.5, weight: .semibold, design: .monospaced))
                                .foregroundStyle(mode == .replace ? Tone.coral : .white)
                            Text(mode.help)
                                .font(.system(size: 11))
                                .foregroundStyle(Tone.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        Spacer(minLength: 0)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .padding(14)
        .frame(width: 360)
    }
}

/// Create / Replace / Append as a menu button. Replace is drawn in coral, because it is the one
/// mode that can destroy something the user already had.
struct WriteModeButton: View {
    @Binding var mode: WriteMode

    var body: some View {
        Menu {
            ForEach(WriteMode.allCases) { option in
                Button { mode = option } label: {
                    if option == mode {
                        Label(option.label, systemImage: "checkmark")
                    } else {
                        Text(option.label)
                    }
                }
            }
        } label: {
            HStack(spacing: 6) {
                Image(systemName: mode.symbol)
                    .font(.system(size: 10, weight: .semibold))
                    .foregroundStyle(mode.tint)
                Text(mode.label).font(.system(size: 12))
                Image(systemName: "chevron.down")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundStyle(Tone.secondary)
            }
            .padding(.horizontal, 9)
            .frame(height: Metrics.control)
            .background(Color.black.opacity(0.30), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous)
                .strokeBorder(mode == .replace ? Tone.coral.opacity(0.5) : .white.opacity(0.10)))
            .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .help(mode.help)
    }
}

/// The format button opens a popover rather than a menu: the nine writers want a tile grid, and
/// each one's own options belong right next to the choice, the way Navicat's export wizard puts
/// them.
struct FormatPickerButton: View {
    @Bindable var tab: QueryTab
    @State private var showing = false

    var body: some View {
        Button { showing.toggle() } label: {
            HStack(spacing: 7) {
                Image(systemName: tab.format.symbol)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(tab.format.tint)
                Text(tab.format.label)
                    .font(.system(size: 12, weight: .semibold, design: .monospaced))
                Image(systemName: "chevron.down")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundStyle(Tone.secondary)
            }
            .padding(.horizontal, 9)
            .frame(height: 28)
            .background(Color.black.opacity(0.30), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous).strokeBorder(.white.opacity(0.10)))
            .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .buttonStyle(.plain)
        .help(tab.format.fullLabel)
        .popover(isPresented: $showing, arrowEdge: .bottom) {
            VStack(alignment: .leading, spacing: 12) {
                SectionLabel(text: "Format")
                FormatGrid(selection: $tab.format)
                Divider().overlay(.white.opacity(0.08))
                FormatOptionsPanel(tab: tab)
                Divider().overlay(.white.opacity(0.08))
                StreamingOptions(tab: tab)
            }
            .padding(14)
            .frame(width: 372)
        }
    }
}

struct FolderButton: View {
    @Bindable var tab: QueryTab
    @State private var hovering = false

    var body: some View {
        Button { choose() } label: {
            HStack(spacing: 6) {
                Image(systemName: "folder.fill").font(.system(size: 10)).foregroundStyle(Tone.amber)
                Text(tab.outputDirectory?.lastPathComponent ?? "Choose folder")
                    .font(.system(size: 12))
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            .padding(.horizontal, 9)
            .frame(height: 28)
            .frame(maxWidth: 150)
            .background(Color.black.opacity(0.30), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous)
                .strokeBorder(.white.opacity(hovering ? 0.18 : 0.10)))
            .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help(tab.outputDirectory?.path ?? "Choose an output folder")
    }

    private func choose() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.prompt = "Choose"
        if let directory = tab.outputDirectory { panel.directoryURL = directory }
        if panel.runModal() == .OK, let url = panel.url { tab.outputDirectory = url }
    }
}

// MARK: Editor

struct EditorPane: View {
    @Environment(AppModel.self) private var model
    @Bindable var tab: QueryTab
    /// `@FocusState` cannot see inside an `NSViewRepresentable`; the editor reports focus itself
    /// through `textDidBeginEditing` / `textDidEndEditing`.
    @State private var focused = false

    private var lineCount: Int {
        tab.sql.isEmpty ? 0 : tab.sql.split(whereSeparator: \.isNewline).count
    }

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                SectionLabel(text: "Query")
                Text("·").font(.system(size: 10.5)).foregroundStyle(.white.opacity(0.25))
                Text(pluralized(lineCount, "line"))
                    .font(.system(size: 10.5))
                    .foregroundStyle(Tone.secondary)
                Spacer()
                Button("Load File…") { tab.loadSQLFromFile() }
                    .buttonStyle(.pill)
                    .keyboardShortcut("o", modifiers: .command)
                    .help("Load SQL from a file (⌘O)")
                Button("Clear") { tab.sql = "" }
                    .buttonStyle(.pill)
                    .disabled(tab.sql.isEmpty)
            }
            .padding(.horizontal, Metrics.gutter)
            .frame(height: Metrics.paneHeader)

            SQLEditor(text: $tab.sql, focused: $focused, completion: model.completion,
                      candidates: { prefix, qualified in
                          model.suggestions(for: tab, prefix: prefix, qualified: qualified)
                      })
                .editorBox(focused: focused)
                // NSTextView has no placeholder of its own, so it is drawn over the text
                // container's own inset (8 wide, 9 tall) plus its line fragment padding.
                .overlay(alignment: .topLeading) {
                    if tab.sql.isEmpty {
                        VStack(alignment: .leading, spacing: 6) {
                            Text("SELECT * FROM hive.analytics.penerima_manfaat")
                                .font(.system(size: 13, design: .monospaced))
                                .foregroundStyle(.white.opacity(0.26))
                            Text("Suggestions appear as you type · ⌃Space to ask for them")
                                .font(.system(size: 11.5))
                                .foregroundStyle(.white.opacity(0.22))
                        }
                        .padding(.leading, 13)
                        .padding(.top, 10)
                        .allowsHitTesting(false)
                    }
                }
                .overlay { SuggestionOverlay(completion: model.completion) }
                .padding(.horizontal, Metrics.gutter)
                .padding(.bottom, 10)
        }
        .frame(maxHeight: .infinity)
        // A list left over from another tab would be pinned to the wrong caret.
        .onChange(of: model.selectedTabID) { _, _ in model.completion.dismiss() }
    }
}

/// Drag handle between the editor and the panel below it.
struct PanelResizer: View {
    @Environment(AppModel.self) private var model
    @State private var startHeight: CGFloat?

    var body: some View {
        ZStack {
            Rectangle().fill(.white.opacity(0.07)).frame(height: 1)
            Color.clear.contentShape(Rectangle())
        }
        .frame(height: 7)
        .onHover { $0 ? NSCursor.resizeUpDown.set() : NSCursor.arrow.set() }
        .gesture(
            DragGesture(minimumDistance: 1)
                .onChanged { value in
                    if startHeight == nil { startHeight = model.panelHeight }
                    let base = startHeight ?? model.panelHeight
                    // Dragging the seam down makes the panel shorter.
                    model.panelHeight = min(560, max(96, base - value.translation.height))
                    model.panelCollapsed = false
                }
                .onEnded { _ in startHeight = nil }
        )
    }
}

// MARK: Format controls (shared by the toolbar popover)

struct FormatGrid: View {
    @Binding var selection: ExportFormat
    private let columns = Array(repeating: GridItem(.flexible(), spacing: 7), count: 3)

    var body: some View {
        LazyVGrid(columns: columns, spacing: 7) {
            ForEach(ExportFormat.allCases) { format in
                FormatTile(format: format, selected: selection == format) {
                    selection = format
                }
            }
        }
    }
}

struct FormatTile: View {
    let format: ExportFormat
    let selected: Bool
    let action: () -> Void
    @State private var hovering = false

    var body: some View {
        Button(action: action) {
            VStack(spacing: 5) {
                Image(systemName: format.symbol)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(selected ? .white : format.tint)
                    .frame(width: 28, height: 28)
                    .background {
                        RoundedRectangle(cornerRadius: 8, style: .continuous)
                            .fill(selected ? AnyShapeStyle(Hue.exporter.gradient)
                                           : AnyShapeStyle(format.tint.opacity(0.14)))
                    }
                Text(format.label)
                    .font(.system(size: 10.5, weight: .semibold, design: .monospaced))
                    .foregroundStyle(selected ? .white : Tone.secondary)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 8)
            .background(Color.white.opacity(selected ? 0.14 : (hovering ? 0.06 : 0.02)),
                        in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(selected ? Tone.ice.opacity(0.5) : .white.opacity(0.08), lineWidth: selected ? 1.5 : 1))
            .contentShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        }
        .buttonStyle(.plain)
        .onHover { hovering = $0 }
        .help("\(format.fullLabel) — \(format.note)")
    }
}

/// Only the options the picked writer actually reads, so the panel never shows a field that
/// would be silently ignored.
struct FormatOptionsPanel: View {
    @Bindable var tab: QueryTab

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            SectionLabel(text: "\(tab.format.label) options")
            switch tab.format {
            case .txt, .csv:
                LabeledField("Delimiter") {
                    Segmented(selection: $tab.delimiter, options: [",", ";", "|", "\t"]) {
                        [",": "Comma", ";": "Semicolon", "|": "Pipe", "\t": "Tab"][$0] ?? $0
                    }
                }
                HStack(alignment: .top, spacing: 10) {
                    LabeledField("NULL as") { TextField("blank", text: $tab.nullText).field() }
                    LabeledField("Encoding") { TextField("utf-8", text: $tab.encoding).field() }
                }
                ChipToggle(label: "Header row", isOn: $tab.header)
                if tab.format == .csv {
                    ChipToggle(label: "UTF-8 BOM (stops Excel mangling it)", isOn: $tab.bom)
                }
            case .json:
                ChipToggle(label: "Newline-delimited (jsonl)", isOn: $tab.jsonl)
            case .sql:
                LabeledField("Target table") {
                    TextField("catalog.schema.table", text: $tab.sqlTable).field()
                }
            case .xls, .xlsx:
                LabeledField("Sheet name") { TextField("Sheet1", text: $tab.sheet).field() }
            case .dbf:
                LabeledField("Max char width") {
                    TextField("254", value: $tab.dbfCharWidth, format: .number.grouping(.never)).field()
                }
            case .xml, .html:
                Text("This format takes no options.")
                    .font(.system(size: 11))
                    .foregroundStyle(Tone.secondary)
            }
        }
    }
}

struct StreamingOptions: View {
    @Bindable var tab: QueryTab

    var body: some View {
        VStack(alignment: .leading, spacing: 9) {
            SectionLabel(text: "Streaming")
            HStack(alignment: .top, spacing: 10) {
                LabeledField("Rows per fetch") {
                    TextField("10000", value: $tab.batchSize, format: .number.grouping(.never)).field()
                }
                LabeledField("Retries") {
                    TextField("5", value: $tab.retries, format: .number.grouping(.never)).field()
                }
            }
            LabeledField("Split every N rows") {
                TextField("one file", value: $tab.splitRows, format: .number.grouping(.never))
                    .field()
                    .disabled(tab.format.splitsItself)
            }
            Text(tab.format.splitsItself
                 ? "\(tab.format.label) splits by its own row ceiling, so no split is needed."
                 : "0 writes one file however big it gets.")
                .font(.system(size: 11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
            ChipToggle(label: "Zip the result files", isOn: $tab.zip)
        }
    }
}
