import AppKit
import SwiftUI

/// The rows a Run fetched, as a grid. Navicat's answer to "what did the query return", and the
/// reason this app is a query editor rather than a one-way pipe: Run looks, Export writes.
///
/// What is left here is the SwiftUI around the table: the toolbar, the sort banner, the sheets, the
/// footer and the value reader beside the grid. The grid itself is `ResultGridTable`, an
/// `NSTableView` that draws every pixel of itself — see `GridTableView`. The split is deliberate:
/// the table must not know about `AppModel`, the sort routing or any of these sheets, so it is
/// handed `GridInputs` (what to draw) and `GridCommands` (what to do when the pointer acts).
struct ResultGrid: View {
    @Environment(AppModel.self) private var model
    @Environment(\.colorScheme) private var colorScheme
    @Bindable var tab: QueryTab
    @State private var confirmReplace = false

    /// The reader card's width beside the grid. Fixed rather than resizable, for the same reason the
    /// schema inspector's is: the grid beside it scrolls on both axes, so a wider reader costs it
    /// columns rather than a layout, and one number is one thing to get right.
    private let inspectorWidth: CGFloat = 340

    /// The selection the value reader beside the grid is holding, when the Data pane asks for one.
    ///
    /// Settled when a drag ends rather than followed step by step. The panel's presence — and so the
    /// grid's width — is the same either way, so following the drag would buy nothing but a rebuilt
    /// value, and for a JSON cell a re-parse, on every step of it.
    @State private var inspectedRange: CellRange?

    /// The cell whose whole value is open in the reader popover, keyed by source column. Held here
    /// rather than in the table because a snapshot scene fills it, which the table could not do for
    /// itself.
    @State private var viewingCell: CellKey?

    /// The door from this view's menus into the table's coordinator, which is built after this body
    /// has already run: "Edit Cell…" has to reach the overlay that lives inside the table.

    /// Whether the review of the queued changes is open.
    @State private var reviewingChanges = false

    /// The column being renamed, and the text in the rename sheet. `nil` between renames.
    @State private var renaming: RenameTarget?

    /// The name being typed in the "save filters as a preset" sheet.
    @State private var savingPreset = false
    @State private var presetName = ""

    /// One column rename in flight. An identity of its own so the sheet can be driven by `item:`
    /// without the source index having to be `Identifiable`.
    struct RenameTarget: Identifiable {
        let id = UUID()
        let source: Int
        var name: String
    }

    var body: some View {
        VStack(spacing: 0) {
            if let error = tab.previewError, tab.previewing == false {
                ErrorBanner(message: error,
                            onCopy: {
                                NSPasteboard.general.clearContents()
                                NSPasteboard.general.setString(error, forType: .string)
                            },
                            onShowLog: { tab.panel = .log },
                            onDismiss: { tab.previewError = nil })
            }
            HStack(spacing: 0) {
                content
                // Only while the setting asks for it and a cell is chosen. A panel on an empty
                // selection would be a column of nothing, which reads as a grid that could not be
                // drawn rather than as nothing being selected.
                if tab.recordMode {
                    // Record mode keeps the panel up with or without a chosen cell, and whatever the
                    // Data pane says: leaving it hands the panel back to that setting.
                    Rectangle().fill(Tone.ink.opacity(0.07)).frame(width: 1)
                    RecordPanel(tab: tab, showInGrid: { _ = placeCursor(on: $0) }, openReader: openReader)
                        .frame(width: inspectorWidth)
                } else if let range = inspectorRange {
                    Rectangle().fill(Tone.ink.opacity(0.07)).frame(width: 1)
                    inspector(range).frame(width: inspectorWidth)
                }
            }
            footer
        }
        // Bounded, so the panel is handed the height that is actually there. Without it the reader
        // reports its ideal height, the row grows past the pane, and the note that explains the value
        // is the part that falls off the bottom.
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        // ⌘S (`AppModel.saveFocused`) raises a flag on the tab rather than reaching into this view.
        // Checked on appear as well, because the panel may have been showing the log when it was set.
        .onChange(of: tab.reviewRequested) { _, _ in takeReviewRequest() }
        .onAppear { takeReviewRequest() }
        .onChange(of: DataPreferences.shared.autoShowInspector) { _, on in
            // Switching the panel on with a cell already chosen should show it rather than wait for
            // a click that has already happened.
            if on { inspectedRange = tab.cellSelection }
        }
    }

    private func takeReviewRequest() {
        guard tab.reviewRequested else { return }
        tab.reviewRequested = false
        if model.canSaveFocused { openReview() }
    }

    /// Open the review sheet. The open cell editor is staged first, so the plan the sheet shows and
    /// the one that runs contain what has been typed.
    private func openReview() {
        tab.endCellEdit()
        reviewingChanges = true
    }

    /// The sentence for a run in flight, or `nil` when nothing is running. A preview and an explain
    /// are both runs, and both fill this grid.
    private var loadingLabel: String? {
        GridPlaceholder.inFlight(previewing: tab.previewing, explaining: tab.explaining)
    }

    /// What the body of the grid has to say for itself, or `nil` while it has rows to draw.
    private func placeholder(_ preview: PreviewResult) -> GridPlaceholder? {
        // `fetchedRows` is the one value here SwiftUI watches while a result streams: the store
        // itself is not observable, so the body runs again when this number moves (D-28).
        GridPlaceholder.whenEmpty(shown: tab.result.count, fetched: max(preview.rowCount, tab.fetchedRows),
                                  loading: loadingLabel)
    }

    /// The sentence over the panel while a run is in flight and its columns are not known yet.
    private var awaitingColumns: String? {
        tab.preview == nil ? loadingLabel : nil
    }

    /// What the table is handed until the columns arrive: no columns, no rows.
    private static let noColumnsYet = PreviewResult(columns: [], rowCount: 0, truncated: false,
                                                    queryID: nil, elapsedMS: 0)

    @ViewBuilder private var content: some View {
        if awaitingColumns != nil || tab.preview?.columns.isEmpty == false {
            // One call site for the table, from the moment a run starts to the moment its result is
            // replaced (W8-F1). The columns arrive with the engine's first event, and until then the
            // table stands built and empty (it draws nothing without columns) under the spinner:
            // building it here is work the main thread does while the engine is busy, instead of
            // work the first rows wait for. A run that follows one (Run again, a server sort or
            // search) finds the table of the run before it and reuses it. Nothing may wrap or
            // un-wrap the table between those states, or SwiftUI moves it out of its window and
            // back (`.opacity` does exactly that), so the spinner is an overlay and the rest of the
            // panel simply is not there.
            let covered = awaitingColumns != nil
            grid(tab.preview ?? Self.noColumnsYet, covered: covered)
                .accessibilityHidden(covered)
                .overlay { if let label = awaitingColumns { status(label, symbol: nil) } }
        } else if tab.preview?.stopped == true {
            // Stop landed before the first page: no columns, so no grid, and it is not "Press Run".
            status("Stopped before any rows arrived.", symbol: "stop.circle")
        } else {
            status("Press Run to see the rows. Run fetches the first \(tab.rowLimit.formatted()) and stops — it writes nothing.",
                   symbol: "play.circle")
        }
    }

    private func status(_ text: String, symbol: String?, tint: Color = Tone.secondary) -> some View {
        VStack(spacing: 8) {
            if let symbol {
                Image(systemName: symbol).font(.system(size: 22)).foregroundStyle(tint)
            } else {
                ProgressView().controlSize(.small)
            }
            Text(text)
                .font(.ui(11.5))
                .foregroundStyle(tint)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 420)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// The panel under the header when there is nothing to put under it.
    ///
    /// It keeps the shape `status` already had — a glyph over a sentence — because it is saying the
    /// same kind of thing the untouched grid says before a query runs, and a second visual language
    /// for "there is nothing here" would be one too many. What is new is that the three ways a body
    /// can be empty are now three different sentences, and that the one the user can act on carries
    /// the action: a body emptied by filters is one click from having its rows back.
    @ViewBuilder private func placeholderBody(_ placeholder: GridPlaceholder) -> some View {
        VStack(spacing: 8) {
            switch placeholder {
            case .loading(let label):
                ProgressView().controlSize(.small)
                Text(label).font(.ui(11.5)).foregroundStyle(Tone.secondary)
            case .noRows:
                Image(systemName: "tray")
                    .font(.system(size: 22))
                    .foregroundStyle(Tone.secondary)
                Text(tab.showingPlan
                     ? "No plan. The engine explained the statement and sent nothing back."
                     : tab.preview?.stopped == true
                     ? "Stopped before any rows arrived."
                     : "No rows. The statement ran to the end and matched nothing.")
                    .font(.ui(11.5))
                    .foregroundStyle(Tone.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 420)
            case .filteredOut(let hidden):
                Image(systemName: "line.3.horizontal.decrease.circle")
                    .font(.system(size: 22))
                    .foregroundStyle(Tone.secondary)
                Text("No rows match. \(pluralized(hidden, "fetched row")) hidden by "
                     + "\(hiddenByLabel).")
                    .font(.ui(11.5))
                    .foregroundStyle(Tone.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 420)
                // The same `Clear` the filter popover offers, put where the user is looking. It is
                // one control over the whole dictionary rather than one per column because the
                // question the empty body asks is "what is hiding my rows", not "this column". It
                // clears the cross-column search too: when the search is what emptied the grid,
                // clearing only the filters would leave the button doing nothing.
                PillButton(title: tab.columnFilters.isEmpty ? "Clear Search" : "Clear Filters",
                           symbol: "xmark.circle", role: .quiet) {
                    tab.gridSearch = ""
                    tab.columnFilters = [:]
                }
            }
        }
        .frame(maxWidth: .infinity)
    }

    /// "3 filters" / "1 filter", for the sentence above.
    private var filtersLabel: String {
        "\(tab.columnFilters.count) filter\(tab.columnFilters.count == 1 ? "" : "s")"
    }

    /// What is hiding the rows for the empty body's sentence: the filters, the search, or both.
    private var hiddenByLabel: String {
        var parts: [String] = []
        if !tab.columnFilters.isEmpty { parts.append(filtersLabel) }
        if tab.hasGridSearch { parts.append("the search") }
        return parts.isEmpty ? "nothing" : parts.joined(separator: " and ")
    }

    /// The toolbar, the banner and the table.
    ///
    /// The table is given the header's height rather than the whole pane when there are no rows: a
    /// body with nothing under it still has columns, and that is what keeps a wide result's headings
    /// reachable while the placeholder sits across the panel instead of across the header's width.
    /// The banner stays SwiftUI, above the table (blueprint D-13): drawing a sentence and a button
    /// into an `NSView` buys nothing, and at scroll offset zero the pixels are the same.
    private func grid(_ preview: PreviewResult, covered: Bool) -> some View {
        let inputs = gridInputs(preview)
        let whenEmpty = covered ? nil : placeholder(preview)
        let header = GridMetrics.headerHeight(fontSize: DataPreferences.shared.gridFontSize)
        return VStack(spacing: 0) {
            // Built behind the spinner too: the toolbar does not depend on the columns, and
            // building it when they arrive was a third of what the first rows waited for. Its views
            // are SwiftUI's own, so veiling them moves nothing that matters (the table is the one
            // view that must not be wrapped).
            gridToolbar(preview)
                .opacity(covered ? 0 : 1)
                .allowsHitTesting(!covered)
                .accessibilityHidden(covered)
                .disabled(covered)
            if !covered {
                if let note = failureNote { failureBanner(note) }
                if let sort = tab.activeSort, sort.origin == .memory, preview.truncated {
                    // The only banner left: an in-memory order over a cut-short result is partial,
                    // and one thin row says so. A full memory order and a server order need no
                    // banner — the chevron is the whole story.
                    PartialOrderNote(fetched: tab.result.fetched)
                }
            }
            // The same table with and without a placeholder, so only its height changes when the
            // first rows land. Two call sites, one in each branch of an `if`, were two views to
            // SwiftUI: the first rows threw the table away and built another (W8-F1).
            ResultGridTable(tab: tab, model: model, inputs: inputs, commands: gridCommands(preview))
                .frame(maxWidth: .infinity,
                       minHeight: whenEmpty == nil ? nil : header,
                       maxHeight: whenEmpty == nil ? .infinity : header)
            if let whenEmpty {
                placeholderBody(whenEmpty)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        }
        .sheet(isPresented: $reviewingChanges) {
            ChangeReview(plan: pendingPlan,
                         onApply: { plan in
                             reviewingChanges = false
                             model.applyChanges(plan, in: tab)
                         },
                         onClose: { reviewingChanges = false })
        }
        .sheet(item: $renaming) { target in
            RenameColumnSheet(name: target.name) { name in
                tab.renameColumn(target.source, to: name)
                renaming = nil
            } onCancel: {
                renaming = nil
            }
        }
        .sheet(isPresented: $savingPreset) {
            SavePresetSheet(name: $presetName, canSave: !tab.columnFilters.isEmpty) { name in
                model.saveFilterPreset(named: name, in: tab)
                savingPreset = false
                presetName = ""
            } onCancel: {
                savingPreset = false
                presetName = ""
            }
        }
    }

    /// What went wrong with the store or the view, for a thin strip above the grid: a window the
    /// store could not read, or a view it could not apply. Stale answers never reach here.
    private var failureNote: String? {
        if let error = tab.viewError { return "The grid could not update its rows: \(error)" }
        if let failure = (tab.result as? StoreRows)?.lastFailure {
            return "The grid could not read part of the result: \(failure.detail)"
        }
        return nil
    }

    private func failureBanner(_ text: String) -> some View {
        HStack(spacing: 6) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(Tone.coral)
            Text(text).font(.ui(11)).foregroundStyle(Tone.secondary).lineLimit(2)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
    }

    /// Open the rename sheet over one column, seeded with the label it currently carries.
    private func beginRename(_ source: Int) {
        let original = tab.preview?.columns ?? []
        renaming = RenameTarget(source: source,
                                name: tab.columnLayout.label(source, original: original))
    }

    // MARK: GridInputs / GridCommands

    /// Everything the table has to be told, read here so Observation registers the dependency.
    ///
    /// Reading these properties inside `body` is what makes SwiftUI call `updateNSView` at all: the
    /// table's contents are read by the coordinator, outside any SwiftUI tracking.
    private func gridInputs(_ preview: PreviewResult) -> GridInputs {
        GridInputs(
            revision: tab.gridRevision,
            layout: GridInputs.GridColumnLayout(columnWidths: columnWidths(preview),
                                                visibleSources: tab.visibleColumnSources),
            selection: tab.cellSelection,
            edits: tab.cellEdits,
            sort: tab.activeSort.map {
                SortIndicator(column: $0.column, direction: $0.direction, origin: $0.origin)
            },
            filtered: Set(tab.columnFilters.keys),
            style: GridInputs.GridStyle(
                rowHeight: DataPreferences.shared.rowPoints,
                fontSize: DataPreferences.shared.gridFontSize,
                alternateRows: DataPreferences.shared.alternateRows,
                showRowNumbers: DataPreferences.shared.showRowNumbers,
                nullDisplay: DataPreferences.shared.nullDisplay,
                codeFontFamily: ThemeStore.shared.codeFontFamily,
                accent: ThemeStore.shared.accent.rawValue,
                isDark: colorScheme == .dark,
                // A sort needs every row to have arrived (§17.3).
                sortEnabled: !tab.previewing
            ),
            filterPopover: model.filterPopoverColumn,
            viewing: viewingCell,
            sessionOpen: tab.hasOpenCellEdit
        )
    }

    /// The widths for the columns the grid is drawing, in display order.
    ///
    /// The formula and the sample live in `GridMetrics` and on the seam: a column's natural width is
    /// measured from the first 200 *fetched* rows, in server order, so narrowing the grid with a
    /// filter does not narrow the columns with it. The panel's own width is not known here — this
    /// runs in `body` — so the slack is shared against a nominal width and the table refits on its
    /// first layout pass, which is where the real width arrives.
    private func columnWidths(_ preview: PreviewResult) -> [CGFloat] {
        _ = tab.fetchedRows   // the widths follow the first 200 rows, which arrive while it streams
        let visible = tab.visibleColumnSources
        let counts = tab.result.naturalCharCounts()
        return GridMetrics.naturalWidths(
            headerCounts: visible.map { tab.columnLayout.label($0, original: preview.columns).count },
            sampleCounts: visible.map { counts.indices.contains($0) ? counts[$0] : 0 },
            fontSize: DataPreferences.shared.gridFontSize
        )
    }

    private func gridCommands(_ preview: PreviewResult) -> GridCommands {
        GridCommands(
            sortClick: { source in
                guard preview.columns.indices.contains(source) else { return }
                model.toggleSort(tab, column: preview.columns[source], source: source)
            },
            openFilter: { source, _ in model.filterPopoverColumn = source },
            rename: { source in beginRename(source) },
            review: { openReview() },
            copy: { withHeaders in copySelection(withHeaders: withHeaders) },
            viewValue: { viewSelectedValue() },
            // The editor lives in the table, so these three run over there: every menu action is
            // an `NSMenu` selector on the coordinator (`menu(for:)`, `editCell`). What these
            // closures carry is the table's notification *upward* — `Coordinator.beginEdit` fires
            // `commands.beginEdit` after `tab` has taken the session — and the host adds nothing.
            beginEdit: { _ in },
            commitEdit: { _, _ in },
            cancelEdit: {},
            settleSelection: { inspectedRange = tab.cellSelection },
            paste: { pasteIntoSelection() },
            selectionChanged: {}
        )
    }

    // MARK: Search, columns and presets toolbar

    /// The strip above the header: the cross-column search, and the column and filter-preset menus.
    ///
    /// It sits outside the scroller so it stays put while the columns scroll sideways, and it is the
    /// one place that can say what the two searches do differently — the in-memory one and the
    /// server one are both reachable from here, and neither is obvious from its own control.
    @ViewBuilder private func gridToolbar(_ preview: PreviewResult) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                searchField(preview)
                Spacer(minLength: 8)
                columnsMenu
                presetsMenu
            }
            .padding(.horizontal, Metrics.gutter)
            .padding(.vertical, 5)
            if tab.hasGridSearch, !tab.isServerSearched {
                Text("In memory over the \(tab.result.fetched.formatted()) rows fetched. "
                     + "“Search Server” runs the query again with a WHERE over every column, so it "
                     + "can find rows this grid never fetched.")
                    .font(.ui(11))
                    .foregroundStyle(Tone.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, Metrics.gutter)
                    .padding(.bottom, 4)
            }
        }
        .background(Tone.recess.opacity(0.18))
        .overlay(Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1), alignment: .bottom)
    }

    @ViewBuilder private func searchField(_ preview: PreviewResult) -> some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 11)).foregroundStyle(Tone.secondary)
            TextField("Search all columns", text: $tab.gridSearch)
                .textFieldStyle(.plain)
                .font(.ui(11.5))
                .frame(width: 170)
                .onChange(of: tab.gridSearch) { _, _ in model.scheduleServerSearch(tab) }
            if tab.hasGridSearch {
                Text("\(tab.result.count.formatted()) of \(tab.result.fetched.formatted())")
                    .font(.ui(11)).foregroundStyle(Tone.secondary)
                Button {
                    model.searchOnServer(tab, term: tab.gridSearch)
                } label: {
                    Label("Search Server", systemImage: "arrow.up.right.square")
                        .font(.ui(11))
                }
                .buttonStyle(.plain)
                .foregroundStyle(Tone.accent)
                .help("Re-run the query with this term as a WHERE over every column, so a row "
                      + "outside the row limit is found too. It reads the table again.")
                Button {
                    tab.gridSearch = ""
                } label: {
                    Image(systemName: "xmark.circle.fill").font(.system(size: 11))
                }
                .buttonStyle(.plain)
                .foregroundStyle(Tone.ink.opacity(0.35))
                .help("Clear the search")
            }
        }
    }

    private var columnsMenu: some View {
        let hidden = tab.columnLayout.hiddenCount
        return Menu {
            ForEach(0..<tab.columnLayout.sourceCount, id: \.self) { source in
                Button {
                    tab.toggleColumn(source)
                } label: {
                    Label(tab.columnLayout.label(source, original: tab.preview?.columns ?? []),
                          systemImage: tab.columnLayout.isVisible(source) ? "checkmark" : "circle.dashed")
                }
            }
            Divider()
            Button("Show All Columns") { tab.showAllColumns() }
                .disabled(hidden == 0)
            Button("Reset Column Layout") { tab.resetColumnLayout() }
        } label: {
            Label(hidden == 0 ? "Columns" : "Columns (\(hidden) hidden)", systemImage: "tablecells")
                .font(.ui(11))
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .help("Hide and show the grid's columns. Rendering only: a hidden column is still copied "
              + "and still exported.")
    }

    private var presetsMenu: some View {
        let presets = model.filterPresets(for: tab)
        return Menu {
            Button("Save Current Filters…") {
                presetName = ""
                savingPreset = true
            }
            .disabled(tab.columnFilters.isEmpty)
            if !presets.isEmpty {
                Divider()
                ForEach(presets) { preset in
                    Button(preset.name) { apply(preset) }
                }
                Divider()
                Menu("Delete Preset") {
                    ForEach(presets) { preset in
                        Button(preset.name) { model.deleteFilterPreset(named: preset.name, in: tab) }
                    }
                }
            }
        } label: {
            Label(presets.isEmpty ? "Filters" : "Filters (\(presets.count))",
                  systemImage: "line.3.horizontal.decrease.circle")
                .font(.ui(11))
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
        .help("Save the filters on screen as a preset and reapply one later. Saved per connection "
              + "and table; a hand-written query keeps its presets for this tab only.")
    }

    private func apply(_ preset: FilterPreset) {
        let missing = tab.applyFilterPreset(preset)
        if !missing.isEmpty {
            tab.note(.warning, "Preset “\(preset.name)” names columns this result does not have: "
                     + "\(missing.joined(separator: ", ")).")
        }
    }

    /// Puts the selected block on the clipboard. The path the footer button takes, which is
    /// reachable without the keyboard focus ⌘C wants; ⌘C itself goes through the table's responder
    /// chain and calls the same function in the coordinator.
    ///
    /// The text is built by `GridClipboard.text(result:…)`, which owns the source-column rule: the
    /// copy is the block the user selected, under the server's own column names, regardless of how
    /// the grid has been hidden, reordered or renamed.
    private func copySelection(withHeaders: Bool) {
        let text: String
        do {
            guard let built = try GridClipboard.text(result: tab.result, selection: tab.cellSelection,
                                                     visible: tab.visibleColumnSources,
                                                     withHeaders: withHeaders,
                                                     inserted: tab.cellEdits.inserted) else { return }
            text = built
        } catch {
            if (error as? StoreFailure)?.isStale != true { tab.note(.error, "Copy failed: \(error)") }
            return
        }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    /// "3 Changes", for the footer's readout. One place, so the count and its spelling cannot drift
    /// from the menu's.
    private var changeLabel: String {
        "\(tab.cellEdits.count) Change\(tab.cellEdits.count == 1 ? "" : "s")"
    }

    /// The selected cell when its value is one the reader can open, or `nil` when it is not — or
    /// when nothing is selected.
    ///
    /// The selection is a rectangle of display positions, so its top-left is mapped to the source
    /// column before the reader is opened on it; that is the key the reader's popover is bound to.
    private func openableSelection() -> CellKey? {
        guard let preview = tab.preview, let selection = tab.cellSelection,
              let source = tab.columnLayout.source(at: selection.left),
              preview.columns.indices.contains(source),
              let key = tab.rowSpace.cellKey(forTableRow: selection.top, source: source) else { return nil }
        guard let value = tab.cellValue(at: key),
              GridValue.isOpenable(value: value, type: preview.columns[source].type) else { return nil }
        return key
    }

    /// Open the reader on the selection's top-left cell, the cell its own double-click would open.
    private func viewSelectedValue() {
        guard let key = openableSelection() else { return }
        viewingCell = key
    }

    /// The range the panel holds, or `nil` when there should be no panel at all: the Data pane asks
    /// for one, and something is still chosen.
    ///
    /// The settled range when there is one, and otherwise whatever is chosen — a selection no drag
    /// has settled, which is a scene's or a restored one, is still a selection, and a panel that
    /// showed nothing for it would be the worse answer. The stale case, a settled range whose
    /// selection has gone, answers `nil` here rather than being cleared, because every path that
    /// clears a selection already means the panel goes with it and the next click re-settles it.
    private var inspectorRange: CellRange? {
        guard DataPreferences.shared.autoShowInspector, let chosen = tab.cellSelection else {
            return nil
        }
        return inspectedRange ?? chosen
    }

    /// Put the grid's cursor and selection on one field of the Record panel's row, and hand the
    /// keyboard to the grid. Returns the cell's key (a negative id for an added row), or `nil` when
    /// the cursor is on no row.
    ///
    /// Not scrolled sideways: the row is the cursor's own and already in view, and the table's
    /// scrolling is its coordinator's (`ResultGridTable`).
    private func placeCursor(on field: RecordField) -> CellKey? {
        guard let key = tab.placeCursor(on: field) else { return nil }
        inspectedRange = tab.cellSelection
        model.focus(.results)
        return key
    }

    /// "Show more" on a value the panel cannot hold: leave Record mode so the Cell reader has the
    /// cell, as the beside-the-grid panel when the Data pane asks for one and as the popover when
    /// it does not.
    private func openReader(_ field: RecordField) {
        guard let key = placeCursor(on: field) else { return }
        tab.recordMode = false
        if !DataPreferences.shared.autoShowInspector { viewingCell = key }
    }

    /// The value reader standing beside the grid.
    ///
    /// Drawn for any selection rather than only for a value this reader can open. A panel that came
    /// and went as the selection moved between columns would change the grid's width under the
    /// pointer, and the columns would shift on the very click that was choosing one.
    @ViewBuilder private func inspector(_ range: CellRange) -> some View {
        if range.cellCount > 1 {
            // One value is the reader's unit, so a block has no single value to show. Saying how big
            // it is beats showing the top-left cell as though it stood for the rest of them.
            inspectorNote(icon: "square.grid.3x3",
                          title: "\(range.rowCount) × \(range.columnCount) cells chosen",
                          detail: "The reader shows one cell at a time. Choose a single cell, or use "
                              + "Copy for the block.")
        } else if let cell = inspectorCell(range) {
            // Keyed by the cell. The reader holds its mode and its format in `@State`, and a panel
            // keeps its identity from one selection to the next, so without this the Tree mode of a
            // JSON cell would follow the selection onto a varchar.
            CellValueViewer(value: cell.value ?? "", column: cell.column.name, type: cell.column.type,
                            connectionID: tab.connectionID, table: tab.sourceTable,
                            placement: .panel)
                .id(cell.key)
        } else {
            inspectorNote(icon: "tablecells",
                          title: "Nothing to read",
                          detail: "The chosen cell is past the shape the server reported.")
        }
    }

    private func inspectorNote(icon: String, title: String, detail: String) -> some View {
        VStack(spacing: 8) {
            Image(systemName: icon).font(.system(size: 20)).foregroundStyle(Tone.secondary)
            Text(title)
                .font(.ui(11.5))
                .foregroundStyle(Tone.ink.opacity(0.9))
                .multilineTextAlignment(.center)
            Text(detail)
                .font(.ui(11))
                .foregroundStyle(Tone.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(16)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// The selection's top-left cell, mapped to the source column the grid reads by — the same
    /// mapping `selectedCell()` does, because the selection counts display positions and the reader
    /// is keyed by source.
    private func inspectorCell(_ range: CellRange)
        -> (key: CellKey, column: Event.Column, value: String?)? {
        guard let preview = tab.preview,
              let source = tab.columnLayout.source(at: range.left),
              preview.columns.indices.contains(source),
              let key = tab.rowSpace.cellKey(forTableRow: range.top, source: source) else { return nil }
        return (key, preview.columns[source], tab.cellValue(at: key))
    }

    /// The reader's presentation, bound to one cell's key so only that cell's popover can be open.
    ///
    /// Never while the panel is standing: the value is already on screen beside the rows, and a
    /// popover would be the same value twice, one of them covering the grid it came from.
    private func popoverBinding(for key: CellKey) -> Binding<Bool> {
        Binding(get: { inspectorRange == nil && viewingCell == key },
                set: { if !$0 { viewingCell = nil } })
    }

    /// Put the clipboard's block into the selection, anchored at its top-left corner the way a
    /// spreadsheet takes a paste. The anchor's column is a display position; the tab maps it and the
    /// run of columns after it back to the source cells a paste is keyed by.
    private func pasteIntoSelection() {
        guard let selection = tab.cellSelection, tab.preview != nil,
              let text = NSPasteboard.general.string(forType: .string) else { return }
        tab.pasteCellEdits(text, at: CellKey(row: selection.top, column: selection.left),
                           columnCount: tab.visibleColumnSources.count)
    }

    /// The statements the queued changes would run, or none when the app cannot say which table
    /// to write to — a hand-written query, whose `sourceTable` is nil.
    ///
    /// Built once, and the same value is handed to the review sheet and, from there, to
    /// `apply_changes`: what the sheet shows is literally the plan that runs.
    private var pendingPlan: WritePlan {
        guard let preview = tab.preview, let connection = model.connection(for: tab) else {
            return WritePlan(table: tab.sourceTable, statements: [])
        }
        return WritePlan.build(edits: tab.cellEdits, rows: tab.result,
                               columns: preview.columns, table: tab.sourceTable,
                               kind: connection.kind, viewBusy: tab.viewBusy)
    }

    

    /// What the footer claims. Never "N rows" for a limited result without saying so, and once the
    /// server has been asked, the two numbers appear together.
    private func summaryText(_ preview: PreviewResult) -> String {
        // A run in flight has no count yet, and "0 rows" is exactly what this footer says for a
        // finished, empty result — the one reading it must not give while a result is still
        // arriving. What has landed so far is worth saying, so it is said once there is something.
        if let label = loadingLabel {
            return tab.fetchedRows == 0
                ? label
                : "\(label) · \(pluralized(tab.fetchedRows, "row")) so far"
        }
        // A plan is not a row count. The grid draws it because a plan *is* a result set, but the
        // footer must not report rows for it, and the total/limit controls below are meaningless.
        if tab.showingPlan {
            return "Query plan · \(pluralized(tab.result.fetched, "line"))"
        }
        let narrowed = !tab.columnFilters.isEmpty || tab.hasGridSearch
        let fetched = narrowed ? tab.result.count : tab.result.fetched
        let scope = narrowed ? " of \(tab.result.fetched.formatted())" : ""

        if let total = tab.totalRows {
            return "\(fetched.formatted())\(scope) of \(total.formatted()) rows"
        }
        return narrowed ? "\(fetched.formatted())\(scope) rows" : preview.summary
    }

    /// DBeaver's count, as a button. Only offered once there is a result to count, and only while
    /// the statement on screen is the one that produced it.
    @ViewBuilder private var countControl: some View {
        if tab.previewedSQL != nil, tab.preview != nil, !tab.showingPlan {
            if tab.countingRows {
                HStack(spacing: 5) {
                    ProgressView().controlSize(.mini)
                    Text("Counting…").font(.ui(11)).foregroundStyle(Tone.secondary)
                }
            } else if let error = tab.countError {
                Text(error)
                    .font(.ui(11))
                    .foregroundStyle(Tone.coral)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .frame(maxWidth: 260, alignment: .leading)
                    .help(error)
            } else if tab.totalRows == nil {
                // A glyph rather than a labelled pill: the footer is a dense readout, the sentence
                // beside it already says what the numbers are, and the help carries the one thing
                // the glyph cannot — that this runs a second query over the whole result.
                IconButton(symbol: "number",
                           help: "Count all rows: ask the server how many rows this statement "
                               + "really returns. Runs a second query over the whole result, so "
                               + "it can be slow.",
                           diameter: 22) {
                    model.countRows(tab)
                }
            }
        }
    }

    /// The filter popover's contents, in whichever of its two shapes this column's data calls for.
    ///
    /// Hosted in an `NSPopover` anchored to the funnel, which the table opens: the contents stay
    /// SwiftUI because there is nothing to gain from drawing a picker by hand (blueprint D-14).
    /// The distinct list is asked for asynchronously (the store scans in Rust), and `more` is the
    /// signal to fall back to a search box.
    @ViewBuilder func filterEditor(_ index: Int) -> some View {
        let column = tab.preview.flatMap { index < $0.columns.count ? $0.columns[index] : nil }
        FilterEditorBody(rows: tab.result, column: index) { sample in
            let values = sample.values
            let browsable = !sample.more && values.count <= ColumnFilter.valuePickerLimit
            VStack(alignment: .leading, spacing: 10) {
                SectionLabel(text: "Filter \(column?.name ?? "column")")
                if browsable {
                    ValuePickerList(tab: tab, index: index, values: values)
                } else {
                    SearchFilterField(tab: tab, index: index)
                }
                Text("This narrows the \(tab.result.fetched.formatted()) rows already "
                     + "fetched — it does not re-run the query, so a row outside the limit is not "
                     + "searched.")
                    .font(.ui(11))
                    .foregroundStyle(Tone.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                HStack(spacing: 8) {
                    if browsable {
                        Text("\(values.count) distinct value\(values.count == 1 ? "" : "s")")
                            .font(.ui(11))
                            .foregroundStyle(Tone.secondary)
                    }
                    Spacer(minLength: 0)
                    PillButton(title: "Clear", role: .quiet, compact: true) { tab.columnFilters[index] = nil }
                }
            }
            .padding(14)
            .frame(width: 300)
        }
    }

    /// The grid's own footer: what is on screen, the limit that decided it, and the one action
    /// that turns looking into keeping. Export lives here rather than in the toolbar because it
    /// acts on this result, and because the toolbar has no room left for it.
    private var footer: some View {
        @Bindable var tab = tab
        return HStack(spacing: 10) {
            if let preview = tab.preview {
                // Never "N rows" while a filter is on: that reads as the size of the result
                // rather than the size of what survived the filter.
                // "1.000 rows" was the fetched count wearing a total's clothes. The wording now
                // says which it is, and a fetched total makes the two one sentence.
                Text(summaryText(preview))
                    .font(.ui(11))
                    .foregroundStyle(preview.truncated || preview.stopped || !tab.columnFilters.isEmpty || tab.hasGridSearch
                                     ? Tone.markAmber : Tone.secondary)
                countControl
                // The commit pair, beside the count button: the grid's two ways of asking the server
                // something about what is on screen — how many rows, and "make these changes real".
                // Both appear only when they have something to act on.
                if !tab.cellEdits.isEmpty {
                    // `Tone.markAmber`, not `Tone.amber`: the text is 4.5:1 on the light canvas
                    // where the plain amber is 1.6:1 (W9 §1.8).
                    Text("· \(changeLabel.lowercased())")
                        .font(.ui(11))
                        .foregroundStyle(Tone.markAmber)
                    // No `.keyboardShortcut` here: ⌘S belongs to the menu (blueprint w10 §5.3), and a
                    // second binding for the same key would fire both.
                    PillButton(title: "Review \(pluralized(tab.cellEdits.count, "Change"))…",
                               symbol: "checkmark.circle", compact: true) { openReview() }
                        .disabled(tab.viewBusy || tab.applying)
                        .help(tab.applying ? "\(QueryTab.applyingMessage)…"
                              : tab.viewBusy ? "\(QueryTab.viewBusyMessage)…"
                              : "Review and run the changes (\(model.shortcutScheme.shortcut(for: .saveFile)?.display ?? "⌘S"))")
                    PillButton(title: "Discard", symbol: "arrow.uturn.backward", role: .quiet,
                               compact: true) { tab.discardCellEdits() }
                        .disabled(tab.applying)
                        .help("Throw the \(changeLabel.lowercased()) away")
                }
                if preview.elapsedMS > 0 {
                    Text("· \(preview.elapsedMS) ms").font(.ui(11)).foregroundStyle(Tone.secondary)
                }
                // The selection's own readout, so the block the pointer dragged out is named rather
                // than only tinted — and the button beside it is the copy that always works, with or
                // without the keyboard focus ⌘C wants.
                if let selection = tab.cellSelection {
                    Text("· \(selection.rowCount) × \(selection.columnCount) selected")
                        .font(.ui(11))
                        .foregroundStyle(Tone.accent)
                    IconButton(symbol: "doc.on.doc",
                               help: "Copy the selected cells as a table (⌘C)",
                               diameter: 22) { copySelection(withHeaders: false) }
                }
                // Rows are added and removed only where the app knows the table (blueprint w10
                // §5.2); a button that cannot act says why in its help.
                if tab.sourceTable != nil, !tab.showingPlan {
                    let blocked = model.rowEditBlockedReason(for: tab)
                    IconButton(symbol: "plus", help: blocked ?? "Add a row", label: "Add row",
                               diameter: 22) { model.addRow(in: tab) }
                        .disabled(blocked != nil)
                    IconButton(symbol: "minus",
                               help: blocked ?? (tab.cellSelection == nil ? "Select rows to delete them"
                                                 : "Delete the selected rows"),
                               label: "Delete selected rows", diameter: 22) { model.deleteRows(in: tab) }
                        .disabled(blocked != nil || tab.cellSelection == nil)
                }
            } else {
                Text("No result yet").font(.ui(11)).foregroundStyle(Tone.secondary)
            }

            Spacer(minLength: 8)

            // Neither control means anything for a plan: there is no limit to set on EXPLAIN, and
            // counting the lines of a plan is not a question anyone has.
            if !tab.showingPlan {
                HStack(spacing: 5) {
                    Text("LIMIT").font(.ui(11, weight: .semibold)).tracking(0.6)
                        .foregroundStyle(Tone.secondary)
                    TextField("1000", value: $tab.rowLimit, format: .number.grouping(.never))
                        .textFieldStyle(.plain)
                        .font(.code(11))
                        .frame(width: 52)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 3)
                        .background(Tone.recess.opacity(0.28), in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                        .overlay(RoundedRectangle(cornerRadius: 5, style: .continuous).strokeBorder(Tone.ink.opacity(0.10)))
                }
                .help("How many rows Run fetches. It does not change the query; the engine stops reading here.")
            }

            let reason = model.runBlockedReason(for: tab)
            // Opens the same wizard the Run menu does: one rule for every Export in the app —
            // you see where it is going before it goes.
            PillButton(title: tab.destination == .table ? "Save to Table" : "Export",
                       symbol: tab.destination == .table ? "square.and.arrow.down" : "arrow.down.doc") {
                model.exportSettingsOpen = true
            }
            .disabled(reason != nil || tab.stage == .running)
            .help(reason ?? (tab.destination == .table
                             ? "Run the query again and let Trino write every row into the table"
                             : "Run the query again and stream every row into the file"))
        }
        .padding(.horizontal, Metrics.gutter)
        .padding(.vertical, 6)
        .overlay(alignment: .top) { Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1) }
        .confirmationDialog("Replace \(tab.target(for: model.connection(for: tab)?.kind ?? .trino))?",
                            isPresented: $confirmReplace) {
            Button("Drop and Recreate", role: .destructive) { model.run(tab) }
        } message: {
            Text("The existing table is dropped before the query runs. If the query then fails, the table is already gone.")
        }
    }
}

/// The one thin row for a partial order: an in-memory sort over a cut-short
/// result. Full memory orders and server orders show no banner at all.
struct PartialOrderNote: View {
    let fetched: Int

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "arrow.up.arrow.down")
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(Tone.accent)
            Text("Partial order over the \(fetched.formatted()) rows fetched.")
                .font(.ui(11))
                .foregroundStyle(Tone.secondary)
                .fixedSize()
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
    }
}

/// The rename sheet for one column.
///
/// A sheet rather than an inline edit of the header text: the header is a button that sorts, and a
/// name field inside it would fight the click. Submitting a blank name reverts to the server's own,
/// which is what "clear the rename" has to mean.
private struct RenameColumnSheet: View {
    @State var name: String
    let onCommit: (String) -> Void
    let onCancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Rename Column")
                .font(.ui(13, weight: .semibold))
                .foregroundStyle(Tone.ink)
            TextField("Column name", text: $name)
                .field()
            Text("Display only. The server's name is what the copy and the export use, and the "
                 + "column keeps its own data and filter.")
                .font(.ui(11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) {
                Spacer()
                PillButton(title: "Cancel", role: .quiet, action: onCancel)
                PillButton(title: "Rename") { onCommit(name) }
            }
        }
        .padding(18)
        .frame(width: 340)
    }
}

/// The sheet that names a filter preset before saving it.
private struct SavePresetSheet: View {
    @Binding var name: String
    let canSave: Bool
    let onSave: (String) -> Void
    let onCancel: () -> Void

    private var trimmed: String { name.trimmingCharacters(in: .whitespacesAndNewlines) }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Save Filters as Preset")
                .font(.ui(13, weight: .semibold))
                .foregroundStyle(Tone.ink)
            TextField("Preset name", text: $name)
                .field()
            Text(canSave
                 ? "Saved against this connection and table, so reopening the table offers it again. "
                   + "Columns are matched by name, so it still applies if the result comes back in a "
                   + "different order."
                 : "There are no filters to save yet. Set one from a column's funnel first.")
                .font(.ui(11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) {
                Spacer()
                PillButton(title: "Cancel", role: .quiet, action: onCancel)
                PillButton(title: "Save") { onSave(trimmed) }
                    .disabled(!canSave || trimmed.isEmpty)
            }
        }
        .padding(18)
        .frame(width: 360)
    }
}

/// What the queued cell edits would run, before they run.
///
/// The statements are shown in full, and that is the point of the sheet rather than a nicety. The
/// identity rule these carry is "every column at the value it was fetched with" — this app has no
/// primary key to lean on — so a predicate that matches more than the row it was built from is
/// visible here or nowhere. A commit that wrote without showing this would be asking the user to
/// trust a `WHERE` they never saw.
///
/// Run applies the plan in one transaction (`AppModel.applyChanges`); Copy SQL takes the same
/// statements away to run by hand.
private struct ChangeReview: View {
    let plan: WritePlan
    let onApply: (WritePlan) -> Void
    let onClose: () -> Void

    /// Guards against a double-click sending the plan twice. A plan is not
    /// idempotent — an `INSERT` run twice inserts twice — so the button is spent
    /// once pressed.
    @State private var applying = false

    private var statements: [WriteStatement] { plan.statements }

    private var title: String {
        "\(statements.count) change\(statements.count == 1 ? "" : "s")"
    }

    /// Kept out of `body` on purpose: inlined into `Text(...)` it is a `map` over a closure
    /// concatenating five string literals behind a `??`, and Swift 6.1's type checker gives up
    /// on it ("unable to type-check this expression in reasonable time"). A plain property is
    /// trivial to check and easier to read.
    private var subtitle: String {
        if let table = plan.table {
            return "Against \(table). Each row is matched by every column at the value it "
                + "was fetched with, so two identical rows would both be changed. The "
                + "plan runs in one transaction and is rolled back if a statement "
                + "affects a different number of rows than expected."
        }
        return "This tab does not know which table it is showing, so there is nothing to write "
            + "back to. Open the table from the tree to edit its rows."
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(title)
                .font(.ui(13, weight: .semibold))
                .foregroundStyle(Tone.ink)
            Text(subtitle)
                .font(.ui(11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)

            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(Array(statements.enumerated()), id: \.offset) { _, statement in
                        HStack(alignment: .top, spacing: 8) {
                            Text(statement.kind.rawValue)
                                .font(.code(11, weight: .semibold))
                                .foregroundStyle(Tone.accent)
                                .frame(width: 52, alignment: .leading)
                            Text(statement.sql)
                                .font(.code(11))
                                .foregroundStyle(Tone.ink)
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }
                }
                .padding(10)
            }
            .frame(maxHeight: 300)
            .background(Tone.recess.opacity(0.30), in: RoundedRectangle(cornerRadius: 8, style: .continuous))

            HStack(spacing: 8) {
                PillButton(title: "Copy SQL", symbol: "doc.on.doc") { copyAll() }
                    .disabled(statements.isEmpty)
                Spacer()
                PillButton(title: "Close", role: .quiet, action: onClose)
                PillButton(title: applying ? "Running…" : "Run", symbol: "play.fill", role: .destructive) {
                    guard !applying, !plan.isEmpty else { return }
                    applying = true
                    onApply(plan)
                }
                .disabled(plan.isEmpty || applying)
            }
        }
        .padding(18)
        .frame(width: 640)
    }

    /// The statements as one script, each terminated. The terminator is what makes it pasteable into
    /// a client that expects one; without it the last statement looks truncated.
    private func copyAll() {
        let script = statements.map { $0.sql }
            .map { $0.hasSuffix(";") ? $0 : $0 + ";" }
            .joined(separator: "\n")
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(script, forType: .string)
    }
}

/// The value list for a column with few enough distinct values to browse.
///
/// This is the shape that matters: the choices come from the column's own data, so the filter
/// cannot be a typo and the user can see what is actually in there before choosing. `Cari` narrows
/// the *list*, not the grid — it is how you find one value among ten, not another filter.
private struct ValuePickerList: View {
    @Bindable var tab: QueryTab
    let index: Int
    let values: [String?]

    @State private var search = ""

    private var picked: Set<String> {
        if case .values(let set) = tab.columnFilters[index] { return set }
        return []
    }

    private var shown: [String?] {
        let needle = search.trimmingCharacters(in: .whitespaces)
        guard !needle.isEmpty else { return values }
        return values.filter { display($0).localizedCaseInsensitiveContains(needle) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            TextField("Cari", text: $search).field()

            if values.isEmpty {
                Text("No values in the rows fetched.")
                    .font(.ui(11))
                    .foregroundStyle(Tone.secondary)
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 1) {
                        // "Select all" only earns its place once the list is long enough to be
                        // tedious; over three values it is more clutter than help.
                        if values.count > 3, search.isEmpty {
                            row(title: picked.count == values.count ? "Clear" : "Select all",
                                checked: picked.count == values.count) { toggleAll() }
                            Divider().overlay(Tone.ink.opacity(0.08)).padding(.vertical, 3)
                        }
                        ForEach(shown, id: \.self) { value in
                            row(title: display(value), checked: picked.contains(token(value))) {
                                toggle(token(value))
                            }
                        }
                    }
                }
                .frame(maxHeight: 220)
            }
        }
        .onAppear {
            // Opening the picker on a column that was filtered by text converts nothing: the user
            // is choosing values from here on, so the old text is dropped rather than silently
            // combined with a selection it does not describe.
            if case .text = tab.columnFilters[index] { tab.columnFilters[index] = nil }
        }
    }

    private func row(title: String, checked: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Image(systemName: checked ? "checkmark.square.fill" : "square")
                    .font(.system(size: 11))
                    .foregroundStyle(checked ? Tone.accent : Tone.ink.opacity(0.35))
                Text(title)
                    .font(.code(11.5))
                    .foregroundStyle(title == "null" ? Tone.ink.opacity(0.45) : Tone.ink.opacity(0.92))
                    .italic(title == "null")
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 0)
            }
            .padding(.vertical, 3)
            .padding(.horizontal, 4)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .background(checked ? Tone.ink.opacity(0.05) : .clear,
                    in: RoundedRectangle(cornerRadius: 4, style: .continuous))
    }

    private func display(_ value: String?) -> String { value ?? "null" }
    private func token(_ value: String?) -> String { value ?? ColumnFilter.nullToken }

    private func toggle(_ token: String) {
        var set = picked
        if set.contains(token) { set.remove(token) } else { set.insert(token) }
        tab.columnFilters[index] = set.isEmpty ? nil : .values(set)
    }

    private func toggleAll() {
        tab.columnFilters[index] = picked.count == values.count
            ? nil
            : .values(Set(values.map(token)))
    }
}

/// The free-text filter for a column with too many distinct values to list.
private struct SearchFilterField: View {
    @Bindable var tab: QueryTab
    let index: Int

    var body: some View {
        let binding = Binding<String>(
            get: {
                if case .text(let needle) = tab.columnFilters[index] { return needle }
                return ""
            },
            set: { tab.columnFilters[index] = $0.isEmpty ? nil : .text($0) })

        VStack(alignment: .leading, spacing: 6) {
            TextField("Cari", text: binding).field()
            Text("Too many distinct values to list, so this matches text: contains by default. "
                 + "Prefix with =, >, <, >= or <= to compare instead.")
                .font(.ui(11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// Loads a column's distinct sample, then hands it to `content`; "Loading…" until it arrives.
private struct FilterEditorBody<Content: View>: View {
    let rows: any ResultRows
    let column: Int
    @ViewBuilder let content: (DistinctSample) -> Content
    @State private var sample: DistinctSample?

    var body: some View {
        Group {
            if let sample {
                content(sample)
            } else {
                Text("Loading…")
                    .font(.ui(11))
                    .foregroundStyle(Tone.secondary)
                    .padding(14)
                    .frame(width: 300, alignment: .leading)
            }
        }
        .task(id: column) { sample = await rows.distinctValues(column: column) }
    }
}

/// The inline error strip above the result body: the message can be selected and copied, shown in
/// the log, or dismissed (W9-T5, FR-RUN-06). The body below falls back to its no-result sentence.
struct ErrorBanner: View {
    let message: String
    let onCopy: () -> Void
    let onShowLog: () -> Void
    let onDismiss: () -> Void

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 12, weight: .bold))
                .foregroundStyle(Tone.coral)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text("Query failed").font(.ui(12, weight: .semibold)).foregroundStyle(Tone.ink)
                ScrollView {
                    Text(message)
                        .font(.ui(11.5))
                        .foregroundStyle(Tone.ink)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 120)
                .fixedSize(horizontal: false, vertical: true)
            }
            HStack(spacing: 6) {
                PillButton(title: "Copy", role: .quiet, compact: true, action: onCopy)
                PillButton(title: "Show in Log", role: .quiet, compact: true, action: onShowLog)
                PillButton(title: "Dismiss", role: .quiet, compact: true, action: onDismiss)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(Tone.coral.opacity(0.10))
        .accessibilityElement(children: .contain)
        .task(id: message) { Announcer.post("Query failed: \(message.prefix(160))", priority: .high) }
    }
}
