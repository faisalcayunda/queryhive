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
                QueryToolbar(tab: tab)
                // Keyed by tab: sharing one editor across tabs would carry the previous query's
                // undo stack and scroll position into the next one.
                EditorPane(tab: tab).id(tab.id)
                PanelResizer()
                BottomPanel(tab: tab, ceiling: workspaceHeight.map { $0 * AppModel.panelShare })
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
        .background(Color.white.opacity(0.03))
        .overlay(alignment: .bottom) { Rectangle().fill(.white.opacity(0.07)).frame(height: 1) }
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
                    .foregroundStyle(running ? .white.opacity(0.3) : .white.opacity(0.75))
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
                       tint: tab.explaining ? Tone.ice : Tone.secondary,
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
                Text("·").font(.system(size: 10.5)).foregroundStyle(.white.opacity(0.25))
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
