import AppKit
import SwiftUI

/// The tabbed query workspace: the tab strip, the query toolbar, the SQL editor, and the panel
/// under it. Navicat's query window, dressed in the CleanMyMac palette.
///
/// It runs up under the window's title bar (`RootView` ignores the top safe area): the tab strip is
/// the title bar's own row, the one the traffic lights are in, and `titlebar` is that row's height.
struct Workspace: View {
    @Environment(AppModel.self) private var model
    /// The title bar's height, which is the tab strip's. Zero until the first layout pass reports it.
    var titlebar: CGFloat = 0
    /// The workspace's own height, published by the background reader below. `nil` until the first
    /// layout pass, which is why the ceiling is optional rather than a number.
    @State private var workspaceHeight: CGFloat?

    var body: some View {
        VStack(spacing: 0) {
            // Always there, even with no tab open: it is the window's drag area as well.
            TabStrip(height: max(titlebar, Metrics.tabStrip))
            if let tab = model.selectedTab {
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

/// The strings the empty workspace and the editor show, as values so a test can read them.
enum EmptyCopy {
    static let workspace = "Open a query tab, write SQL, and press Run to see the rows. Export writes a file."

    /// An example that names the shape of a table in the dialect the tab is connected to, rather
    /// than a table of one particular deployment.
    static func placeholder(for kind: ConnectionKind?) -> String {
        switch kind {
        case .trino: "SELECT * FROM catalog.schema.table"
        case .postgres: "SELECT * FROM schema.table"
        case .mysql: "SELECT * FROM database.table"
        case nil: "SELECT * FROM table"
        }
    }

    /// "New Query (⌘T)" with the key of the scheme in force, since DBeaver's is not ⌘T.
    static func help(_ title: String, _ action: ShortcutAction, scheme: ShortcutScheme) -> String {
        guard let key = scheme.shortcut(for: action)?.display else { return title }
        return "\(title) (\(key))"
    }
}

/// Shown when every tab has been closed.
struct EmptyWorkspace: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 14) {
            HiveHero(size: 190)
            Text("No query open").font(.heroTitle)
            Text(EmptyCopy.workspace)
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

/// The tab strip is the window's title bar: it is as tall as the bar, so the tabs sit in the row
/// the traffic lights are in, and empty space in it drags the window like any title bar.
struct TabStrip: View {
    @Environment(AppModel.self) private var model
    /// The title bar's height (`Workspace.titlebar`).
    let height: CGFloat

    var body: some View {
        // The drag area under the tabs, not a background of them: a background takes no clicks.
        ZStack {
            WindowDragArea()
            HStack(spacing: 0) {
                // Hugs the tabs while they fit, so what is left over is empty and falls through to
                // the drag area; a scroller takes the whole width, and nothing in it would.
                ViewThatFits(in: .horizontal) {
                    tabs
                    ScrollView(.horizontal, showsIndicators: false) { tabs }
                }
                Spacer(minLength: 0)
            }
            // With the sidebar hidden the traffic lights and the sidebar toggle are over this column.
            .padding(.leading, model.navigation.sidebarHidden ? Shell.windowControlsWidth : 0)
        }
        .frame(height: height)
        .background(FrameMarker(name: "tab-strip"))
        // No fill: the strip is the workspace's own surface (the theme's canvas and its glow) run
        // up to the top of the window, so there is no band between it and the title bar's area,
        // and no theme in which it is a different colour from what is under it.
        .overlay(alignment: .bottom) { Rectangle().fill(Tone.hairline).frame(height: 1) }
    }

    private var tabs: some View {
        HStack(spacing: 2) {
            ForEach(model.tabs) { tab in
                TabChip(tab: tab)
            }
            // Right after the last tab: a "+" pinned to the far edge reads as belonging to the
            // window, not to the tab strip.
            Button { model.newTab() } label: {
                Image(systemName: "plus")
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(Tone.secondary)
                    .frame(width: 24, height: 24)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(EmptyCopy.help("New Query", .newQuery, scheme: model.shortcutScheme))
        }
        .padding(.horizontal, Metrics.gutter)
        .padding(.vertical, 4)
        .background(FrameMarker(name: "tab-strip.tabs"))
    }
}

/// What a double-click on a title bar does, as System Settings > Desktop & Dock sets it.
enum TitleBarDoubleClick: Equatable {
    case zoom, minimise, nothing

    /// `AppleActionOnDoubleClick` from the global domain: "Maximize" (also what an unset key means),
    /// "Minimize" or "None".
    init(_ setting: String?) {
        switch setting {
        case "Minimize": self = .minimise
        case "None": self = .nothing
        default: self = .zoom
        }
    }

    static var current: TitleBarDoubleClick { .init(UserDefaults.standard.string(forKey: "AppleActionOnDoubleClick")) }

    @MainActor
    func perform(on window: NSWindow) {
        switch self {
        case .zoom: window.performZoom(nil)
        case .minimise: window.performMiniaturize(nil)
        case .nothing: break
        }
    }
}

/// Empty space that moves the window and answers a double-click the way a title bar does. The
/// title bar is transparent and the content runs under it, so nothing of the system's is there to
/// do either. A gesture rather than an `NSView` under the tabs: the hosting view takes every click
/// itself (a hit test in the strip answers with it, whatever `NSView` sits under the tabs), and only
/// SwiftUI's own hit test knows which parts of the strip are empty.
struct WindowDragArea: View {
    var body: some View {
        Color.clear
            .contentShape(Rectangle())
            .background(FrameMarker(name: "tab-strip.drag"))
            .gesture(DragGesture(minimumDistance: 0).onChanged { _ in
                // The press, once: `performDrag` runs until the button is up, so the gesture never
                // sees the release and goes on reporting moves.
                guard let event = NSApp.currentEvent, event.type == .leftMouseDown, let window = event.window
                else { return }
                guard event.clickCount > 1 else { return window.performDrag(with: event) }
                TitleBarDoubleClick.current.perform(on: window)
            })
    }
}

/// What a tab chip says to VoiceOver and what glyph it draws, as a value so a test can read it
/// without a window.
struct TabChipSpec: Equatable {
    enum Glyph: Equatable { case none, spinner, checkmark, exclamation }

    let label: String
    /// "running", "finished" or "failed"; nil for an idle tab.
    let value: String?
    let isSelected: Bool
    let glyph: Glyph
    let actions: [String]

    init(title: String, stage: QueryTab.Stage, isSelected: Bool, badges: [BadgeSpec] = []) {
        label = title
        self.isSelected = isSelected
        actions = ["Close"]
        let stageWord: String?
        switch stage {
        case .idle: stageWord = nil; glyph = .none
        case .running: stageWord = "running"; glyph = .spinner
        case .done: stageWord = "finished"; glyph = .checkmark
        case .failed: stageWord = "failed"; glyph = .exclamation
        }
        // The connection's limits are part of what the tab is, so they are read with it:
        // "running. Environment: production. Safe Mode: Confirm."
        value = badges.isEmpty
            ? stageWord
            : ([stageWord].compactMap { $0 } + badges.map(\.accessibility)).joined(separator: ". ") + "."
    }
}

/// A tab is two sibling buttons, select and close, never one button wrapping the other.
struct TabChip: View {
    @Environment(AppModel.self) private var model
    let tab: QueryTab
    @State private var hovering = false

    private var selected: Bool { model.selectedTabID == tab.id }
    private var badges: [BadgeSpec] { BadgeSpec.badges(for: model.connection(for: tab), in: .tabChip) }
    private var spec: TabChipSpec {
        TabChipSpec(title: tab.title, stage: tab.stage, isSelected: selected, badges: badges)
    }
    private var closeVisible: Bool { hovering || selected }

    var body: some View {
        let spec = spec
        HStack(spacing: 4) {
            Button { model.selectTab(tab.id) } label: {
                HStack(spacing: 6) {
                    glyph(spec.glyph)
                    Text(tab.title)
                        .font(.ui(12, weight: selected ? .semibold : .regular))
                        .lineLimit(1)
                        .foregroundStyle(Tone.ink.opacity(selected ? 1 : 0.75))
                    ConnectionBadges(specs: badges)
                        .accessibilityHidden(true)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel(spec.label)
            .accessibilityValue(spec.value ?? "")
            .accessibilityAddTraits(spec.isSelected ? .isSelected : [])
            // The close button is hidden from VoiceOver while it is invisible, so the action lives
            // here instead.
            .accessibilityAction(named: "Close") { model.closeTab(tab.id) }

            Button { model.closeTab(tab.id) } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 8, weight: .bold))
                    .foregroundStyle(Tone.secondary)
                    .frame(width: 15, height: 15)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .opacity(closeVisible ? 1 : 0)
            .accessibilityHidden(!closeVisible)
            .accessibilityLabel("Close \(tab.title)")
            .help("Close \(tab.title)")
        }
        .padding(.horizontal, 10)
        .frame(height: 27)
        .background(Tone.ink.opacity(selected ? 0.13 : (hovering ? 0.06 : 0)),
                    in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous)
            .strokeBorder(selected ? Tone.accent.opacity(0.35) : .clear))
        .contentShape(RoundedRectangle(cornerRadius: 7, style: .continuous))
        .onHover { hovering = $0 }
        .contextMenu {
            Button("Close") { model.closeTab(tab.id) }
            Button("Close Others") { model.closeOtherTabs(keeping: tab.id) }
                .disabled(model.tabs.count < 2)
            Button("Close Tabs to the Right") { model.closeTabs(after: tab.id) }
                .disabled(model.tabs.last?.id == tab.id)
            Divider()
            Button("Close All") { model.closeAllTabs() }
        }
        .help(tab.summary)
    }

    /// A shape as well as a colour, so the stage does not depend on telling two hues apart. The
    /// running glyph pulses unless Reduce Motion is on, and is then a still dotted circle.
    @ViewBuilder private func glyph(_ glyph: TabChipSpec.Glyph) -> some View {
        switch glyph {
        case .none:
            EmptyView()
        case .spinner:
            Image(systemName: "circle.dotted")
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(Tone.ice)
                .symbolEffect(.pulse, isActive: !ThemeStore.shared.surface.reduceMotion)
                .accessibilityHidden(true)
        case .checkmark:
            Image(systemName: "checkmark")
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(Tone.markMint)
                .accessibilityHidden(true)
        case .exclamation:
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(Tone.markCoral)
                .accessibilityHidden(true)
        }
    }
}

// MARK: Toolbar

/// The row under the tab strip: the breadcrumb on the leading side, and the badges and the Run
/// group on the trailing one. There is none on an object tab (it holds no query to run) and none
/// while the panel fills the window, because the editor those controls belong to is hidden behind it.
struct QueryToolbar: View {
    @Environment(AppModel.self) private var model
    @Bindable var tab: QueryTab

    var body: some View {
        HStack(spacing: 8) {
            // Connection, then the driver's own levels: Navicat's breadcrumb, so a bare
            // `SELECT * FROM wilayah` has somewhere to resolve.
            ContextCascade(tab: tab)
                .background(FrameMarker(name: "query-toolbar.cascade"))
            Spacer(minLength: 12)
            // Run holds the trailing corner, so it is in the same place on every tab regardless of
            // how long the names in the breadcrumb are, which is what the fixed widths buy. A
            // little more inset than the gutter, because the Run capsule draws a coloured glow
            // that runs into the window edge at a symmetric 12 pt.
            QueryActions(tab: tab)
                .background(FrameMarker(name: "query-toolbar.actions"))
                .padding(.trailing, 6)
        }
        .padding(.horizontal, Metrics.gutter)
        .frame(height: Metrics.toolbar)
        .background(FrameMarker(name: "query-toolbar"))
        // No fill, as with the strip above it: the top three rows (title bar, tabs, context) are
        // the workspace's own surface, so no theme has a band in it. (A fill here is a hazard as
        // well as a tint: a `.background(Color)` ignores the safe area by default, ran up through
        // the strip into the title bar's area and tinted it, in every theme.)
        .overlay(alignment: .bottom) { Rectangle().fill(Tone.hairline).frame(height: 1) }
    }
}

/// The trailing group of the query toolbar: badges, then Run with its variants, Stop and Explain.
struct QueryActions: View {
    @Environment(AppModel.self) private var model
    @Bindable var tab: QueryTab

    var body: some View {
        HStack(spacing: 8) {
            BreadcrumbBadges(tab: tab)

            actionButton

            // A zero-size popover anchor, so it costs the row nothing.
            DestinationPopover(tab: tab)
        }
        // At its ideal size, never squeezed: left to compress, Run's label becomes "R…".
        .fixedSize()
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
                .help(model.runBlockedReason ?? EmptyCopy.help("Run the query and show the rows", .run, scheme: model.shortcutScheme))

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
                Text(mode.label).font(.ui(12))
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
    /// What the editor's gutter says the line count is: the editor writes it, so counting the lines
    /// here would be a second pass over the text for every change to it.
    @State private var lines: EditorLineCount

    init(tab: QueryTab) {
        self.tab = tab
        _lines = State(initialValue: EditorLineCount(text: tab.sql))
    }

    /// The number of lines the query has, for the corner readout.
    ///
    /// An empty editor counts **one**, which is what the gutter shows. It used to count zero, so an
    /// empty editor displayed "0 lines" in the corner beside a "1" in the gutter — two answers to
    /// one question, and the wrong one was the corner's: the caret is on line 1. Every editor with
    /// an empty file says the same.
    private var lineCount: Int { lines.count }

    var body: some View {
        // No header row above the editor any more. It carried a "Query · N lines" label, a
        // "Load File…" button and a "Clear" button, and the label earned none of its height: the
        // tab already says which query this is, and a line count is not something anyone acts on.
        //
        // The two buttons were *moved*, not dropped. Clear sits over the top-right corner of the
        // text it clears, and Load SQL File moved to the File menu, which is where ⌘O is looked
        // for — deleting it from here without that would have removed the feature, since this
        // button was its only entry point and the only holder of the shortcut.
        SQLEditor(text: $tab.sql, focused: $focused, caret: $tab.caret, selection: $tab.selection,
                  completion: model.completion,
                  candidates: { prefix, path in
                      model.suggestions(for: tab, prefix: prefix, path: path)
                  },
                  // Read here rather than inside the editor so this body depends on the switches:
                  // that is what makes flipping one in Settings re-apply the editor.
                  layout: EditorLayout.current,
                  onRunStatement: { offset in
                      // The statement's own start, so `run(from: .statement)` reads the same
                      // statement the marker was drawn beside.
                      tab.selection = NSRange(location: offset, length: 0)
                      tab.caret = offset
                      model.run(tab, from: .statement)
                  },
                  lineCount: lines)
            .editorBox(focused: focused)
            // NSTextView has no placeholder of its own, so it is drawn over the text
            // container's own inset (8 wide, 9 tall) plus its line fragment padding.
            .overlay(alignment: .topLeading) {
                if tab.sql.isEmpty {
                    VStack(alignment: .leading, spacing: 6) {
                        Text(EmptyCopy.placeholder(for: model.connection(for: tab)?.kind))
                            .font(.code(13))
                            .foregroundStyle(Tone.ink.opacity(0.26))
                        Text("Suggestions appear as you type · ⌃Space to ask for them")
                            .font(.ui(11.5))
                            .foregroundStyle(Tone.ink.opacity(0.22))
                    }
                    // Starts where the text starts: past the gutter, then past the text view's
                    // own inset. A fixed 13 put the first line *under* the line numbers, so an
                    // empty editor read as "S1ELECT".
                    .padding(.leading, LineNumberRulerView.textOriginX(
                        forLines: lineCount,
                        showsRunMarks: EditorLayout.current.runButtonPerStatement))
                    .padding(.top, 10)
                    .allowsHitTesting(false)
                }
            }
            // Shown only while there is something to clear. It replaced a permanently visible
            // button that spent most of its life disabled; an action that cannot do anything is
            // noise. Nothing reflows when it comes and goes, because it is an overlay rather than
            // a row.
            .overlay(alignment: .topTrailing) {
                if !tab.sql.isEmpty {
                    IconButton(symbol: "xmark", help: "Clear the editor", diameter: 22) {
                        tab.sql = ""
                    }
                    .padding(.trailing, 11)
                    .padding(.top, 8)
                }
            }
            // The line count, in the corner of the text it counts. Faint on purpose: it is a
            // readout, not a control, and the editor scrolls under it — at anything stronger it
            // would compete with the last line of a long query. Monospaced so the number does not
            // shift sideways as it grows from 9 to 10.
            .overlay(alignment: .bottomTrailing) {
                Text(pluralized(lineCount, "line"))
                    .font(.code(10.5))
                    // The same grey as the gutter numbers, so the two read as one readout rather
                    // than as two greys that happen to be nearby.
                    .foregroundStyle(Tone.readout)
                    .padding(.trailing, 12)
                    .padding(.bottom, 7)
                    .allowsHitTesting(false)
            }
            .overlay { SuggestionOverlay(completion: model.completion) }
            .padding(.horizontal, Metrics.gutter)
            .padding(.top, 10)
            .padding(.bottom, 10)
            .frame(maxHeight: .infinity)
            // A list left over from another tab would be pinned to the wrong caret.
            .onChange(of: model.selectedTabID) { _, _ in model.completion.dismiss() }
    }
}

/// Drag handle between the editor and the panel below it. It is also an adjustable control for
/// VoiceOver and Full Keyboard Access: increment makes the panel `step` points taller.
struct PanelResizer: View {
    @Environment(AppModel.self) private var model
    @State private var startHeight: CGFloat?

    static let minHeight: CGFloat = 96
    static let maxHeight: CGFloat = 560
    static let step: CGFloat = 24

    /// The height after one adjustment, kept inside the range the drag gesture keeps it in.
    static func adjusted(_ height: CGFloat, _ direction: AccessibilityAdjustmentDirection) -> CGFloat {
        switch direction {
        case .increment: min(maxHeight, height + step)
        case .decrement: max(minHeight, height - step)
        @unknown default: height
        }
    }

    /// What the accessibility action does: opens a collapsed panel, as dragging the seam does, then steps it.
    static func adjust(_ model: AppModel, _ direction: AccessibilityAdjustmentDirection) {
        model.panelCollapsed = false
        model.panelHeight = adjusted(model.panelHeight, direction)
    }

    var body: some View {
        ZStack {
            Rectangle().fill(Tone.hairline).frame(height: 1)
            Color.clear.contentShape(Rectangle())
        }
        .frame(height: 7)
        .onHover { $0 ? NSCursor.resizeUpDown.set() : NSCursor.arrow.set() }
        .accessibilityElement()
        .accessibilityLabel("Result panel height")
        .accessibilityValue("\(Int(model.panelHeight)) points")
        .accessibilityAdjustableAction { direction in Self.adjust(model, direction) }
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
                    model.panelHeight = min(Self.maxHeight, max(Self.minHeight, base - value.translation.height))
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
                    .font(.code(11, weight: .semibold))
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
                    .font(.ui(11))
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
                .font(.ui(11))
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
    /// The narrowest a column is allowed to be, whatever its contents.
    private let minimumColumnWidth: CGFloat = 90
    /// How wide the first column may grow. Past this the pane simply has more space than a table
    /// name needs, and a name column half the window wide reads as a mistake rather than as room.
    private let maximumFirstColumnWidth: CGFloat = 520
    /// The air between one column and the next. A column is a fixed width and a long name fills
    /// it, so with no gap a truncated name ends flush against the next column's text and the two
    /// read as one string — `kpknl_manaje…kuntabilitasBASE TABLE`. The gap is what keeps a
    /// truncated cell visibly truncated. Shared by the header and the rows so the two stay
    /// aligned.
    private let columnGap: CGFloat = 20
    /// The horizontal inset a row carries, so a cell's text starts this far inside its column. The
    /// header carries the same inset, or every heading sits one inset to the left of its own values.
    private let rowInset: CGFloat = 4
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
                .font(.ui(12, weight: .semibold))
                .foregroundStyle(Tone.ink)
            if tab.objectLoading {
                ProgressView().controlSize(.mini)
            } else if !tab.objectRows.isEmpty {
                Text(pluralized(tab.objectRows.count, "object"))
                    .font(.ui(11))
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
                let widths = widths(fitting: viewport.size.width)
                ScrollView([.horizontal, .vertical]) {
                    VStack(alignment: .leading, spacing: 0) {
                        headerRow(widths)
                        ForEach(Array(tab.objectRows.enumerated()), id: \.offset) { index, row in
                            ObjectRow(tab: tab, index: index, row: row,
                                      widths: widths, columnGap: columnGap, inset: rowInset)
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

    /// Per-column widths for the pane they are drawn in.
    ///
    /// The columns used to be a fixed 170 pt whatever the pane was, and that is how a listing of
    /// `t_spgg_suspect_20260925_1000` came out as `t_spgg_suspe…0260915` with three quarters of the
    /// pane sitting empty beside it. The name column is the one that truncates, and it was the one
    /// column not being given the room that was actually there.
    ///
    /// So the first column takes whatever is left over once the others have their natural width.
    /// The others keep theirs rather than sharing the slack: a type column stretched to half the
    /// window to hold `BASE TABLE` is width that says nothing.
    private func widths(fitting available: CGFloat) -> [CGFloat] {
        let natural = tab.objectColumns.enumerated().map { index, header in
            // Only the head of the listing decides the width: measuring every row would make a long
            // listing pay for its own layout, and one long tail name would stretch the column to the
            // cap anyway.
            let longest = tab.objectRows.prefix(200).map { row -> Int in
                guard index < row.count, let value = row[index] else { return 0 }
                return value.count
            }.max() ?? 0
            return min(max(CGFloat(max(header.count, longest)) * 6.8 + 20, minimumColumnWidth), 420)
        }
        guard let first = natural.first else { return natural }
        let gaps = CGFloat(max(0, natural.count - 1)) * columnGap
        let used = natural.reduce(0, +) + gaps
        guard used < available else { return natural }
        var widened = natural
        widened[0] = min(first + (available - used), maximumFirstColumnWidth)
        return widened
    }

    private func headerRow(_ widths: [CGFloat]) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: columnGap) {
                ForEach(Array(tab.objectColumns.enumerated()), id: \.offset) { index, name in
                    Text(name)
                        .font(.ui(11, weight: .semibold))
                        .foregroundStyle(Tone.secondary)
                        .frame(width: widths.indices.contains(index) ? widths[index] : minimumColumnWidth,
                               alignment: .leading)
                }
            }
            .padding(.horizontal, rowInset)
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
    let widths: [CGFloat]
    let columnGap: CGFloat
    let inset: CGFloat
    @State private var hovering = false

    private var selected: Bool { tab.objectSelection == index }

    var body: some View {
        HStack(spacing: columnGap) {
            // Driven by the headers, not by the row: a driver that answered a short row would
            // otherwise shift every value one column to the left, under the wrong header, which is
            // worse than a blank cell.
            ForEach(tab.objectColumns.indices, id: \.self) { column in
                Text(column < row.count ? (row[column] ?? "") : "")
                    .font(.code(11))
                    .foregroundStyle(Tone.ink)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .frame(width: widths.indices.contains(column) ? widths[column] : 90,
                           alignment: .leading)
            }
        }
        .padding(.vertical, 3)
        .padding(.horizontal, inset)
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
                .font(.ui(13, weight: .semibold))
                .foregroundStyle(Tone.ink)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
            if let scope = tab.objectScope, !scope.title.isEmpty {
                Text(scope.title)
                    .font(.code(11))
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
                    .font(.ui(11))
                    .foregroundStyle(Tone.coral)
                    .fixedSize(horizontal: false, vertical: true)
            } else if tab.objectDetailColumns.isEmpty {
                Text(tab.objectDetailLoading ? "Reading…" : "No columns reported.")
                    .font(.ui(11))
                    .foregroundStyle(Tone.secondary)
            } else {
                ForEach(Array(tab.objectDetailColumns.enumerated()), id: \.offset) { _, column in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(column.name)
                            .font(.code(11))
                            .foregroundStyle(Tone.ink.opacity(0.9))
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .textSelection(.enabled)
                        Spacer(minLength: 4)
                        Text(column.type)
                            .font(.code(11))
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
        DecorativeLabel(text: text, weight: .bold, tracking: 0.8)
    }

    private func field(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(label)
                .font(.ui(11, weight: .semibold))
                .foregroundStyle(Tone.secondary)
            Text(value)
                .font(.code(11))
                .foregroundStyle(Tone.ink.opacity(0.9))
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
