import AppKit
import SwiftUI

/// The tabbed query workspace: the tab strip, the query toolbar, the SQL editor, and the panel
/// under it. Navicat's query window, dressed in the CleanMyMac palette.
struct Workspace: View {
    @Environment(AppModel.self) private var model
    /// The workspace's own height, published by the background reader below. `nil` until the first
    /// layout pass, which is why the ceiling is optional rather than a number.
    @State private var workspaceHeight: CGFloat?

    var body: some View {
        VStack(spacing: 0) {
            if let tab = model.selectedTab {
                TabStrip()
                if tab.isObjects {
                    // No toolbar and no editor: an object tab holds no query to run, and the
                    // toolbar's destination controls would be switches for something that does
                    // not exist on this tab.
                    ObjectsPane(tab: tab)
                } else if model.panelExpanded {
                    // Rows over the whole window. The toolbar, editor and resizer all go with the
                    // editor they belong to -- a resizer under a full-height panel would have
                    // nothing left to resize. The panel's minimise control is the way back.
                    //
                    // `fills` is what makes "over the whole window" true rather than "over 480
                    // points of it": without it the panel kept its own height and the stack around
                    // it centred the remainder, which showed up as a band of nothing above the tab
                    // strip.
                    BottomPanel(tab: tab, ceiling: workspaceHeight, fills: true)
                } else {
                    QueryToolbar(tab: tab)
                    // Keyed by tab: sharing one editor across tabs would carry the previous
                    // query's undo stack and scroll position into the next one.
                    EditorPane(tab: tab).id(tab.id)
                    PanelResizer()
                    BottomPanel(tab: tab, ceiling: workspaceHeight.map { $0 * AppModel.panelShare })
                }
            } else {
                EmptyWorkspace()
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .background(Backdrop(hue: .exporter))
        // As a background, not a wrapper: a GeometryReader around the stack would propose its own
        // (unbounded) size and the layout would collapse. This one only reports.
        .background {
            GeometryReader { geometry in
                Color.clear
                    .onAppear { workspaceHeight = geometry.size.height }
                    .onChange(of: geometry.size.height) { _, height in workspaceHeight = height }
            }
        }
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
                .keyboardShortcut(model.shortcut(for: .newQuery))
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
        .background(Tone.recess.opacity(0.22))
        .overlay(alignment: .bottom) { Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1) }
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
                .foregroundStyle(Tone.ink.opacity(selected ? 1 : 0.75))
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
        .background(Tone.ink.opacity(selected ? 0.13 : (hovering ? 0.06 : 0)),
                    in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous)
            .strokeBorder(selected ? Tone.accent.opacity(0.35) : .clear))
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

    var body: some View {
        HStack(spacing: 8) {
            // Connection, then the driver's own levels: Navicat's breadcrumb, so a bare
            // `SELECT * FROM wilayah` has somewhere to resolve.
            ContextCascade(tab: tab)

            Spacer(minLength: 12)

            // Run holds the trailing corner, so it is in the same place on every tab regardless of
            // how long the names in the breadcrumb are — which is what the fixed widths above buy.
            //
            // A little more inset than the bar's own gutter, because the Run capsule draws a
            // coloured glow: at a symmetric 12pt the halo ran into the window edge and the group
            // read as clipped even though its frame was not.
            actionButton
                .padding(.trailing, 6)

            // A zero-size popover anchor, so it costs the row nothing.
            DestinationPopover(tab: tab)
        }
        .padding(.horizontal, Metrics.gutter)
        .frame(height: Metrics.toolbar)
        .background(Tone.ink.opacity(0.03))
        .overlay(alignment: .bottom) { Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1) }
        .onChange(of: tab.destination) { _, destination in
            if destination == .table { model.prepareTableDestination(tab) }
        }
    }

    /// Run with its variants on a chevron, and Stop beside it. Navicat's shape, in this app's
    /// toolbar rather than in a strip of its own.
    ///
    /// Stop used to swap into Run's slot, which moved the button out from under the pointer at
    /// exactly the moment someone was reaching for it. It is always here now, disabled when there
    /// is nothing to stop. What is *not* here is Navicat's "Continue on Error": this engine runs
    /// one statement per run, so there is no script to continue.
    @ViewBuilder private var actionButton: some View {
        let running = tab.previewing || tab.stage == .running
        HStack(spacing: 2) {
            HubButton(title: "Run", symbol: "play.fill", hue: .exporter) { model.preview(tab) }
                .disabled(running || model.runBlockedReason != nil)
                .keyboardShortcut(model.shortcut(for: .run))
                .help(model.runBlockedReason ?? "Run the query and show the rows (⌘R)")

            Menu {
                Button("Run") { model.preview(tab) }
                Button("Run Current Statement") { model.preview(tab, from: .statement) }
                Button("Run the Whole Editor") { model.preview(tab, from: .all) }
                Divider()
                // Export lives here rather than in a button of its own: the toolbar had a
                // destination's worth of controls in it, and the one thing that was really a
                // command — write this out — belongs with the other commands.
                // Opens the wizard rather than running blind. Where an export goes is a decision
                // worth seeing every time — it can drop a table — and the settings belong to the
                // moment of exporting, not to a menu item of their own.
                Button(tab.destination == .table ? "Save to Table…" : "Export…") {
                    model.exportSettingsOpen = true
                }
            } label: {
                Image(systemName: "chevron.down")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundStyle(running ? Tone.ink.opacity(0.3) : Tone.ink.opacity(0.75))
                    .frame(width: 18, height: 28)
                    .contentShape(Rectangle())
            }
            .menuStyle(.button)
            .buttonStyle(.plain)
            .menuIndicator(.hidden)
            .frame(width: 18)
            .disabled(running || model.runBlockedReason != nil)
            .help("Run, or run only the statement the caret is in")

            IconButton(symbol: "stop.fill", tint: running ? Tone.coral : Tone.secondary,
                       help: running
                           ? "Stop (\(model.shortcutScheme.shortcut(for: .stop)?.display ?? "no key"))"
                           : "Nothing to stop", diameter: 28) {
                if tab.previewing { model.cancelPreview(tab) } else { model.stop(tab) }
            }
            .disabled(!running)
            .keyboardShortcut(model.shortcut(for: .stop))

            // Explain sits after Stop, where Navicat puts it: the plan is something you reach for
            // while looking at a query, not a peer of Run. It shares Run's blocked reasons, since
            // explaining needs exactly what running needs.
            IconButton(symbol: "list.bullet.rectangle",
                       tint: tab.explaining ? Tone.accent : Tone.secondary,
                       help: tab.explaining ? "Explaining…"
                            : (model.runBlockedReason ?? "Show the query plan without running it"),
                       diameter: 28) {
                model.explain(tab)
            }
            .disabled(running || tab.explaining || model.runBlockedReason != nil)
        }
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
            .background(Tone.recess.opacity(0.30), in: RoundedRectangle(cornerRadius: 7, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous)
                .strokeBorder(mode == .replace ? Tone.coral.opacity(0.5) : Tone.ink.opacity(0.10)))
            .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
        }
        .menuStyle(.button)
        .buttonStyle(.plain)
        .menuIndicator(.hidden)
        .help(mode.help)
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
            // Everything on the left, together. These two act on the text directly below them, so
            // parking them at the far edge of a 1240pt window meant an 800pt mouse journey to
            // reach "Clear" — and left a row whose two ends did not look related to each other.
            // A header that is empty on the right reads as calm; a header with a label at one end
            // and its actions at the other reads as broken.
            HStack(spacing: 8) {
                SectionLabel(text: "Query")
                Text("·").font(.system(size: 10.5)).foregroundStyle(Tone.ink.opacity(0.25))
                Text(pluralized(lineCount, "line"))
                    .font(.system(size: 10.5))
                    .foregroundStyle(Tone.secondary)
                PillButton(title: "Load File…", symbol: "folder", compact: true) { tab.loadSQLFromFile() }
                    .keyboardShortcut(model.shortcut(for: .openFile))
                    .help("Load SQL from a file (⌘O)")
                    .padding(.leading, 6)
                // Quiet: clearing is not a commitment, and giving it the same weight as
                // "Load File…" made the pair read as two equal choices.
                PillButton(title: "Clear", symbol: "xmark", role: .quiet, compact: true) { tab.sql = "" }
                    .disabled(tab.sql.isEmpty)
                Spacer(minLength: 0)
            }

            .padding(.horizontal, Metrics.gutter)
            .frame(height: Metrics.paneHeader)

            SQLEditor(text: $tab.sql, focused: $focused, caret: $tab.caret, selection: $tab.selection,
                      completion: model.completion,
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
                                .foregroundStyle(Tone.ink.opacity(0.26))
                            Text("Suggestions appear as you type · ⌃Space to ask for them")
                                .font(.system(size: 11.5))
                                .foregroundStyle(Tone.ink.opacity(0.22))
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
            Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1)
            Color.clear.contentShape(Rectangle())
        }
        .frame(height: 7)
        .onHover { $0 ? NSCursor.resizeUpDown.set() : NSCursor.arrow.set() }
        .gesture(
            // `.global`, not the default local space. This handle moves as the pane it borders
            // resizes — that is what dragging it does — so a local coordinate space measures the
            // translation against an origin that is itself moving. The value then alternates
            // between the real delta and zero, and the seam shivers under the pointer.
            DragGesture(minimumDistance: 1, coordinateSpace: .global)
                .onChanged { value in
                    if startHeight == nil {
                        startHeight = model.panelHeight
                        model.panelCollapsed = false
                    }
                    let base = startHeight ?? model.panelHeight
                    // Dragging the seam down makes the panel shorter.
                    model.panelHeight = min(560, max(96, base - value.translation.height))
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
            .background(Tone.ink.opacity(selected ? 0.14 : (hovering ? 0.06 : 0.02)),
                        in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(selected ? Tone.accent.opacity(0.5) : Tone.ink.opacity(0.08), lineWidth: selected ? 1.5 : 1))
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

// MARK: Objects

/// A schema's objects, as a grid whose columns are the driver's own.
///
/// There is no fixed four-column shape here on purpose. Postgres answers Name, OID, Owner and ACL
/// — `pg_class` genuinely carries all four — while Trino's information_schema has no OID, owner or
/// ACL to give and answers Name and Type, and MySQL answers Name, Engine, Rows and Comment. A
/// shared shape would put empty cells in three of four columns on two of the three drivers, which
/// is the grid claiming to know something it does not.
struct ObjectsPane: View {
    @Environment(AppModel.self) private var model
    let tab: QueryTab
    /// Wide enough for a schema-qualified name without truncating it to nothing, narrow enough
    /// that four columns still fit the window the editor normally occupies.
    private let columnWidth: CGFloat = 170
    /// The inspector's width. Fixed rather than resizable: the pane beside it scrolls on both
    /// axes, so a wider inspector costs the grid columns rather than a layout, and one number is
    /// one thing to get right.
    private let inspectorWidth: CGFloat = 260

    var body: some View {
        VStack(spacing: 0) {
            header
            Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1)
            HStack(spacing: 0) {
                grid
                // Only while a row is chosen. An inspector on an empty selection would be a panel
                // of blanks, which reads as a table whose shape could not be read rather than as
                // nothing being selected.
                if tab.objectSelection != nil {
                    Rectangle().fill(Tone.ink.opacity(0.07)).frame(width: 1)
                    ObjectInspector(tab: tab).frame(width: inspectorWidth)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "tablecells")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(Tone.secondary)
            Text(tab.objectScope?.title ?? tab.title)
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(Tone.ink)
            if tab.objectLoading {
                ProgressView().controlSize(.mini)
            } else if !tab.objectRows.isEmpty {
                Text(pluralized(tab.objectRows.count, "object"))
                    .font(.system(size: 11))
                    .foregroundStyle(Tone.secondary)
            }
            Spacer()
            IconButton(symbol: "arrow.clockwise", help: "Reload these objects", diameter: 20) {
                model.loadObjects(tab)
            }
        }
        .padding(.horizontal, Metrics.gutter)
        .frame(height: Metrics.panelTabs)
        .background(Tone.recess.opacity(0.22))
    }

    @ViewBuilder
    private var grid: some View {
        if let error = tab.objectError {
            note(error, symbol: "exclamationmark.triangle")
        } else if tab.objectRows.isEmpty {
            // Still "loading" with nothing yet is a spinner rather than an empty state, because
            // "No objects here" while a query is in flight is a claim the app has not earned.
            note(tab.objectLoading ? "Loading…" : "No objects here.",
                 symbol: tab.objectLoading ? "clock" : "tray")
        } else {
            // A ScrollView on *both* axes centres content smaller than its viewport, which parked
            // eight rows in the middle of the window with a screenful of nothing above them.
            //
            // A `.frame(maxHeight: .infinity, alignment: .topLeading)` on the content does not fix
            // it, and it is worth saying why rather than leaving the next reader to rediscover it:
            // a scroll view proposes an *unspecified* size along the axes it scrolls, so that frame
            // collapses to the content's own height and the alignment has nothing to align against.
            // Handing the content the viewport's own size as a minimum leaves no slack to centre.
            GeometryReader { viewport in
                ScrollView([.horizontal, .vertical]) {
                    VStack(alignment: .leading, spacing: 0) {
                        headerRow
                        ForEach(Array(tab.objectRows.enumerated()), id: \.offset) { index, row in
                            ObjectRow(tab: tab, index: index, row: row, columnWidth: columnWidth)
                        }
                    }
                    .padding(.horizontal, Metrics.gutter)
                    .padding(.bottom, 12)
                    .frame(minWidth: viewport.size.width, minHeight: viewport.size.height,
                           alignment: .topLeading)
                }
            }
        }
    }

    private var headerRow: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 0) {
                ForEach(tab.objectColumns, id: \.self) { name in
                    Text(name)
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(Tone.secondary)
                        .frame(width: columnWidth, alignment: .leading)
                }
            }
            .padding(.vertical, 6)
            Rectangle().fill(Tone.ink.opacity(0.09)).frame(height: 1)
        }
    }

    private func note(_ text: String, symbol: String) -> some View {
        VStack(spacing: 8) {
            Image(systemName: symbol).font(.system(size: 22)).foregroundStyle(Tone.secondary)
            Text(text)
                .font(.body13)
                .foregroundStyle(Tone.secondary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(30)
    }
}

/// One object row, and the click that chooses it.
///
/// A row of its own rather than a `ForEach` body inside `ObjectsPane`, for one reason that matters
/// here: the hover state is per row, and a `@State` inside a loop body is shared by every
/// iteration, so hovering one row would light all of them.
private struct ObjectRow: View {
    @Environment(AppModel.self) private var model
    let tab: QueryTab
    let index: Int
    let row: [String?]
    let columnWidth: CGFloat
    @State private var hovering = false

    private var selected: Bool { tab.objectSelection == index }

    var body: some View {
        HStack(spacing: 0) {
            // Driven by the headers, not by the row: a driver that answered a short row would
            // otherwise shift every value one column to the left, under the wrong header, which is
            // worse than a blank cell.
            ForEach(tab.objectColumns.indices, id: \.self) { column in
                Text(column < row.count ? (row[column] ?? "") : "")
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(Tone.ink)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .frame(width: columnWidth, alignment: .leading)
            }
        }
        .padding(.vertical, 3)
        .padding(.horizontal, 4)
        // Full width, so the highlight reads as a *row* rather than as a band behind two cells.
        // Without this the `background` is only as wide as the HStack's own content, which is the
        // sum of the fixed column widths: a two-column Trino listing highlighted about 340 pt of a
        // 1600 pt pane, which looked like a stray rectangle rather than a selection.
        .frame(maxWidth: .infinity, alignment: .leading)
        // The fill spans every column, so the row reads as one thing rather than as a run of
        // separately highlighted cells.
        .background(Tone.accent.opacity(selected ? 0.22 : (hovering ? 0.09 : 0)),
                    in: RoundedRectangle(cornerRadius: 5, style: .continuous))
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        // Double-click first, then single, which is the order the tree already uses and the order
        // SwiftUI needs: a single-tap recogniser attached first claims the first click immediately,
        // so the double-tap gesture never sees its second one. The two actions also compose in this
        // order -- `openObject` selects before it opens, so a double-click leaves the inspector on
        // the table it just opened.
        .onTapGesture(count: 2) { model.openObject(tab, row: index) }
        .onTapGesture { model.selectObject(tab, row: index) }
        .contextMenu { menu }
        .help(tab.objectName(at: index).map { "\($0) — click for its columns, double-click to open" } ?? "")
    }

    @ViewBuilder private var menu: some View {
        Button("Open") { model.openObject(tab, row: index) }
        Button("Insert into Query") { model.insertObject(tab, row: index) }
        if let name = tab.objectName(at: index),
           let scope = tab.objectScope,
           let connection = model.connection(for: tab),
           let qualified = qualifiedName(database: scope.catalog.isEmpty ? nil : scope.catalog,
                                         schema: scope.schema.isEmpty ? nil : scope.schema,
                                         table: name, for: connection.kind) {
            Button("Copy Qualified Name") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(qualified, forType: .string)
            }
        }
        if let name = tab.objectName(at: index) {
            Button("Copy Name") {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(name, forType: .string)
            }
        }
    }
}

/// What the chosen table is: the row's own values, and the columns the server reports for it.
///
/// Navicat's Objects tab shows the listing; the detail belongs beside it, not instead of it, which
/// is why this is a pane and not a sheet. The two halves answer different questions: the listing
/// columns are whatever the driver chose to say about *all* the tables (Trino: Name and Type), and
/// the columns below are what one table actually has.
private struct ObjectInspector: View {
    @Environment(AppModel.self) private var model
    let tab: QueryTab

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    summary
                    listing
                    columns
                }
                .padding(.horizontal, Metrics.gutter)
                .padding(.vertical, 12)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            footer
        }
        .background(Tone.recess.opacity(0.14))
    }

    private var summary: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(tab.objectDetailTable ?? "Table")
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(Tone.ink)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            if let scope = tab.objectScope, !scope.title.isEmpty {
                Text(scope.title)
                    .font(.system(size: 10.5, design: .monospaced))
                    .foregroundStyle(Tone.secondary)
                    .textSelection(.enabled)
            }
        }
    }

    /// Every cell of the selected row, under the driver's own header for it. This is what makes
    /// the inspector driver-independent: Postgres contributes OID, Owner and ACL here, Trino its
    /// Type, MySQL its Engine and Rows, without this view naming any of them.
    @ViewBuilder private var listing: some View {
        if let row = tab.objectSelection, tab.objectRows.indices.contains(row) {
            VStack(alignment: .leading, spacing: 6) {
                sectionTitle("Listing")
                ForEach(tab.objectColumns.indices, id: \.self) { column in
                    let cell = tab.objectRows[row].indices.contains(column)
                        ? (tab.objectRows[row][column] ?? "") : ""
                    if !cell.isEmpty {
                        field(tab.objectColumns[column], cell)
                    }
                }
            }
        }
    }

    @ViewBuilder private var columns: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                sectionTitle("Columns")
                if tab.objectDetailLoading { ProgressView().controlSize(.mini) }
                Spacer(minLength: 0)
            }
            if let error = tab.objectDetailError {
                Text(error)
                    .font(.system(size: 10.5))
                    .foregroundStyle(Tone.coral)
                    .fixedSize(horizontal: false, vertical: true)
            } else if tab.objectDetailColumns.isEmpty {
                Text(tab.objectDetailLoading ? "Reading…" : "No columns reported.")
                    .font(.system(size: 10.5))
                    .foregroundStyle(Tone.secondary)
            } else {
                ForEach(Array(tab.objectDetailColumns.enumerated()), id: \.offset) { _, column in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(column.name)
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundStyle(Tone.ink.opacity(0.9))
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .textSelection(.enabled)
                        Spacer(minLength: 4)
                        Text(column.type)
                            .font(.system(size: 10, design: .monospaced))
                            .foregroundStyle(Tone.secondary)
                            .lineLimit(1)
                    }
                }
            }
        }
    }

    private var footer: some View {
        HStack(spacing: 8) {
            // The inspector only exists while a row is chosen, so the selection is what these act
            // on; there is no row index in scope here and there does not need to be one.
            PillButton(title: "Open", symbol: "arrow.up.forward.app", compact: true) {
                guard let row = tab.objectSelection else { return }
                model.openObject(tab, row: row)
            }
            PillButton(title: "Insert", symbol: "text.insert", role: .quiet, compact: true) {
                guard let row = tab.objectSelection else { return }
                model.insertObject(tab, row: row)
            }
            Spacer(minLength: 0)
        }
        .padding(.horizontal, Metrics.gutter)
        .padding(.vertical, 6)
        .overlay(alignment: .top) { Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1) }
    }

    private func sectionTitle(_ text: String) -> some View {
        Text(text.uppercased())
            .font(.system(size: 9.5, weight: .bold))
            .tracking(0.8)
            .foregroundStyle(Tone.secondary)
    }

    private func field(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(label)
                .font(.system(size: 9.5, weight: .semibold))
                .foregroundStyle(Tone.secondary)
            Text(value)
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(Tone.ink.opacity(0.9))
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
