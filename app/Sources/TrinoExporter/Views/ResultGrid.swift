import AppKit
import SwiftUI

/// The rows a Run fetched, as a grid. Navicat's answer to "what did the query return", and the
/// reason this app is a query editor rather than a one-way pipe: Run looks, Export writes.
struct ResultGrid: View {
    @Environment(AppModel.self) private var model
    @Bindable var tab: QueryTab
    @State private var confirmReplace = false

    /// Read through the model so the snapshot tool can open a filter popover; a header funnel
    /// cannot be clicked from a scene.
    private var filteringColumn: Binding<Int?> {
        Binding(get: { model.filterPopoverColumn }, set: { model.filterPopoverColumn = $0 })
    }

    /// The height of every row, fixed rather than sized to its content.
    ///
    /// A drag that crosses rows is turned into a row index by dividing the pointer's travel by this
    /// number, so it has to be the number the rows are actually drawn at — a content-sized row would
    /// make the mapping drift by a row somewhere down a long result. A uniform row height is also
    /// what a data grid wants: rows that breathe by a fraction of a point read as misaligned.
    private let rowHeight: CGFloat = 25
    /// The horizontal padding a cell carries on each side, so a column is drawn at its measured
    /// width plus twice this. The drag's column mapping has to use the same number the cells are
    /// built with, or the selection lands a column off at the far end of a wide result.
    private let cellPadding: CGFloat = 8

    /// Where the drag that is in flight started. `nil` between drags, which is what tells the next
    /// `onChanged` that it is the first of a new selection rather than a continuation.
    @State private var dragAnchor: (row: Int, column: Int)?

    /// The cell the editor is open over, and what has been typed into it. The text is a separate
    /// piece of state so that abandoning the edit — Escape, or a click elsewhere — leaves the queue
    /// untouched.
    @State private var editingCell: CellKey?
    @State private var editingText = ""
    @FocusState private var editorFocused: Bool

    /// The cell whose whole value is open in the reader popover, by its position on screen. Reset
    /// with everything else positional when the rows change, though a popover dismisses itself as
    /// soon as the pointer leaves it, so in practice it is open only while the pointer is inside.
    @State private var viewingCell: CellKey?

    /// Whether the review of the queued changes is open.
    @State private var reviewingChanges = false

    /// Per-column pixel width, computed once per result rather than per cell: at 1000 rows the
    /// per-cell version is O(rows × columns) work on every render pass.
    /// Natural widths, before the viewport has a say.
    private var naturalWidths: [CGFloat] {
        guard let preview = tab.preview else { return [] }
        return preview.columns.enumerated().map { index, column in
            let header = column.name.count
            // Only the head of the result decides the width: measuring every row would make a
            // 1000-row preview pay for its own layout, and one long tail cell would stretch the
            // column to the cap anyway.
            let longest = preview.rows.prefix(200).map { row -> Int in
                guard index < row.count, let value = row[index] else { return 4 }
                return value.count
            }.max() ?? 0
            // The funnel lives in the header cell, so every column pays for it.
            return min(max(CGFloat(max(header, longest)) * 7.2 + 20, 84), 320) + 22
        }
    }

    /// What the grid actually draws. When the columns come to less than the panel is wide, the
    /// slack is shared out between them: a result whose columns stop two thirds of the way across
    /// reads as unfinished, and the empty band beside it is the first thing the eye lands on.
    private func widths(fitting available: CGFloat) -> [CGFloat] {
        let natural = naturalWidths
        let total = natural.reduce(0, +)
        // The row-number gutter is not one of these columns, so it has to come out of the space
        // they share — without this the columns always fell exactly that short of filling.
        let forColumns = available - gutterWidth
        guard total > 0, total < forColumns else { return natural }
        let slack = forColumns - total
        return natural.map { $0 + slack * ($0 / total) }
    }

    private var gutterWidth: CGFloat { 44 + cellPadding * 2 }

    var body: some View {
        VStack(spacing: 0) {
            content
            footer
        }
    }

    /// The rows the grid draws, in the order it draws them: everything fetched, narrowed by
    /// whatever filters are set, then sorted. Both happen here and nowhere else — neither reaches
    /// the server and neither rewrites the statement, which is why the header says the sort is over
    /// the rows already fetched.
    ///
    /// Sort last, so the order is applied to the set that survives the filter rather than to the
    /// whole fetch and then thrown away. Every positional thing in this view — the selection, the
    /// queued edits, the copy, the paste — indexes *these* rows, because these are what the user
    /// pointed at. `QueryTab.setGridSort` and the filter's own `didSet` clear that state when the
    /// order changes, for exactly that reason.
    /// The rows the grid draws, filtered and sorted — computed once per change and cached by
    /// `QueryTab`.
    ///
    /// The work lives on the tab rather than here because the grid reads this several times per
    /// render (the rows, the placeholder's count, the clipboard), and a sort over a large result is
    /// not free. `QueryTab.gridRevision` is what makes the cache safe to keep.
    private var displayedRows: [[String?]] { tab.displayedRows }

    /// The sentence for a run in flight, or `nil` when nothing is running. A preview and an explain
    /// are both runs, and both fill this grid.
    private var loadingLabel: String? {
        GridPlaceholder.inFlight(previewing: tab.previewing, explaining: tab.explaining)
    }

    /// What the body of the grid has to say for itself, or `nil` while it has rows to draw.
    private func placeholder(_ preview: PreviewResult) -> GridPlaceholder? {
        GridPlaceholder.whenEmpty(shown: displayedRows.count, fetched: preview.rows.count,
                                  loading: loadingLabel)
    }

    @ViewBuilder private var content: some View {
        if let label = loadingLabel, tab.preview == nil {
            // Nothing to draw yet. The columns arrive with the engine's first event and the header
            // arrives with them, so until then there is no table to stand a spinner inside of and
            // the spinner is the whole panel.
            status(label, symbol: nil)
        } else if let error = tab.previewError {
            status(error, symbol: "exclamationmark.triangle.fill", tint: Tone.coral)
        } else if let preview = tab.preview, !preview.columns.isEmpty {
            grid(preview)
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
                     : "No rows. The statement ran to the end and matched nothing.")
                    .font(.ui(11.5))
                    .foregroundStyle(Tone.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 420)
            case .filteredOut(let hidden):
                Image(systemName: "line.3.horizontal.decrease.circle")
                    .font(.system(size: 22))
                    .foregroundStyle(Tone.secondary)
                Text("No rows match the filters. \(pluralized(hidden, "fetched row")) hidden by "
                     + "\(filtersLabel).")
                    .font(.ui(11.5))
                    .foregroundStyle(Tone.secondary)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 420)
                // The same `Clear` the filter popover offers, put where the user is looking. It is
                // one control over the whole dictionary rather than one per column because the
                // question the empty body asks is "what is hiding my rows", not "this column".
                PillButton(title: "Clear Filters", symbol: "xmark.circle", role: .quiet) {
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

    private func grid(_ preview: PreviewResult) -> some View {
        // The reader has to be outside the scroller: inside it, `geometry` would report the
        // content's width, which is the thing being decided.
        GeometryReader { geometry in
            let widths = widths(fitting: geometry.size.width)
            if let placeholder = placeholder(preview) {
                // No rows to scroll through, so the two halves are separate views rather than one
                // scroller. The header still scrolls sideways, which is what keeps a wide result's
                // columns reachable in a narrow panel, but with no rows under it there is nothing it
                // has to stay in step with. The body is then laid out across the *panel*, not across
                // the header: one scroller around both centred the sentence on the header's width,
                // so in a narrow window with eight columns it was drawn off the right edge.
                VStack(spacing: 0) {
                    ScrollView(.horizontal) { headerRow(preview.columns, widths: widths) }
                    placeholderBody(placeholder)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            } else {
                ScrollView([.horizontal, .vertical]) {
                    LazyVStack(alignment: .leading, spacing: 0, pinnedViews: [.sectionHeaders]) {
                        Section {
                            ForEach(Array(displayedRows.enumerated()), id: \.offset) { index, row in
                                rowView(row, index: index, columns: preview.columns, widths: widths)
                            }
                        } header: {
                            headerRow(preview.columns, widths: widths)
                        }
                    }
                    // maxHeight as well as minWidth: a short result was centred in the scroller and
                    // floated in the middle of the panel instead of sitting under its header.
                    .frame(minWidth: geometry.size.width, maxHeight: .infinity, alignment: .topLeading)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        // ⌘C copies the selected block; the context menu is the same action for a pointer that has
        // not found the key, and the footer's own button is the third door to it. Three, because
        // copying a table out is the reason the grid exists and it should not need discovering.
        //
        // The two paths differ in how the text leaves: this one hands the system an item provider,
        // which is what a copy command is for, while the menu and the button write the pasteboard
        // directly. Same text either way.
        .onCopyCommand {
            guard let text = selectionText(withHeaders: false) else { return [] }
            return [NSItemProvider(object: text as NSString)]
        }
        .contextMenu { selectionMenu }
        .sheet(isPresented: $reviewingChanges) {
            ChangeReview(plan: pendingPlan,
                         onApply: { plan in
                             reviewingChanges = false
                             model.applyChanges(plan, in: tab)
                         },
                         onClose: { reviewingChanges = false })
        }
    }

    /// The selected block as the tab-separated text a spreadsheet reads back as a table, or `nil`
    /// when nothing is selected.
    ///
    /// `displayedRows`, not the fetched rows: the user pointed at what is on screen, so what they
    /// copy is what they saw. A filter that hid a row must not put it back in the paste.
    private func selectionText(withHeaders: Bool) -> String? {
        guard let preview = tab.preview, let selection = tab.cellSelection else { return nil }
        return GridClipboard.text(rows: displayedRows, headers: preview.columns.map(\.name),
                                  selection: selection, withHeaders: withHeaders)
    }

    /// Puts the selected block on the clipboard. The path the context menu and the footer button
    /// take, both of which are reachable without the keyboard focus ⌘C wants.
    private func copySelection(withHeaders: Bool) {
        guard let text = selectionText(withHeaders: withHeaders) else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(text, forType: .string)
    }

    @ViewBuilder private var selectionMenu: some View {
        Button("Copy") { copySelection(withHeaders: false) }
            .disabled(tab.cellSelection == nil)
        Button("Copy with Headers") { copySelection(withHeaders: true) }
            .disabled(tab.cellSelection == nil)
        Divider()
        Button("Edit Cell…") { beginEditingSelection() }
            .disabled(tab.cellSelection == nil)
        Button("View Value…") { viewSelectedValue() }
            .disabled(selectedCell() == nil)
        Button("Paste") { pasteIntoSelection() }
            .disabled(tab.cellSelection == nil)
        Divider()
        Button("Review \(changeLabel)…") { reviewingChanges = true }
            .disabled(tab.cellEdits.isEmpty)
        Button("Discard \(changeLabel)") { tab.cellEdits.discard() }
            .disabled(tab.cellEdits.isEmpty)
    }

    /// "3 Changes", for the menu items and the footer. One place, so the two cannot disagree about
    /// how many there are or how to spell it.
    private var changeLabel: String {
        "\(tab.cellEdits.count) Change\(tab.cellEdits.count == 1 ? "" : "s")"
    }

    private func headerRow(_ columns: [Event.Column], widths: [CGFloat]) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            // The sort's own sentence, above the names. A chevron beside a column says *which* column
            // and which way; it cannot say that this is an in-memory order over the rows already
            // fetched. That is the part the plan asked to be written down, and it matters: a sorted
            // grid looks exactly like a sorted result.
            if let sort = tab.gridSort {
                GridSortBanner(column: columnName(sort.column, in: columns),
                               direction: sort.direction,
                               fetched: tab.preview?.rows.count ?? 0,
                               onClear: { tab.setGridSort(nil) })
            }
            HStack(spacing: 0) {
                gutter("#")
                ForEach(Array(columns.enumerated()), id: \.offset) { index, column in
                    headerCell(index, column,
                               width: widths.indices.contains(index) ? widths[index] : 120)
                }
            }
        }
        // `Tone.recess` rather than a literal dark navy. This was `Color(hex: 0x141726)`, the one
        // hard-coded colour left in the views, and it is why the grid's header stayed near-black in
        // light mode while every other surface followed the appearance: a literal has no appearance
        // to resolve against. The tint is applied to `recess` so the band still reads as a header
        // rather than as another row -- `recess` is black on dark and white on light, so the same
        // expression deepens the dark appearance and lightens the light one.
        //
        // The opacity is 0.30, the value the rest of the chrome uses for a recess (see `Tone`'s own
        // note on it). It was 0.85, and at that strength the band stopped reading as part of the
        // surface: on the dark theme it went to near-black, a slab of a different colour laid over
        // the panel rather than a shade *of* it. The family the app already settled on is the fix —
        // the band is a recess, and every other recess in the window is 0.24–0.34.
        .background(Tone.recess.opacity(0.30))
        .overlay(Rectangle().fill(Tone.ink.opacity(0.12)).frame(height: 1), alignment: .bottom)
    }

    /// One column's header cell, and the door to the sort: clicking it cycles ascending, descending,
    /// and off. The whole cell is the target rather than a small glyph, because a header that only
    /// sorts from one 10pt corner is a header nobody sorts from.
    private func headerCell(_ index: Int, _ column: Event.Column, width: CGFloat) -> some View {
        let numeric = isNumeric(column.type)
        let sort = tab.gridSort?.column == index ? tab.gridSort : nil
        return Button {
            tab.setGridSort(GridSort.next(tab.gridSort, clickedColumn: index))
        } label: {
            VStack(alignment: numeric ? .trailing : .leading, spacing: 3) {
                HStack(spacing: 3) {
                    Text(column.name)
                        .font(.code(12, weight: .semibold))
                        .foregroundStyle(Tone.ink.opacity(0.92))
                        .lineLimit(1)
                    if let sort {
                        Image(systemName: sort.direction.symbol)
                            .font(.system(size: 8, weight: .bold))
                            .foregroundStyle(Tone.accent)
                    }
                }
                Chip(text: column.type, tint: typeTint(column.type))
            }
            .frame(width: width, alignment: numeric ? .trailing : .leading)
            .padding(.horizontal, cellPadding)
            .padding(.vertical, 6)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .overlay(alignment: .topTrailing) { filterButton(index) }
        .overlay(Rectangle().fill(Tone.ink.opacity(0.05)).frame(width: 1), alignment: .trailing)
        .help("Sort by \(column.name) — over the rows already fetched, not the whole result")
    }

    /// A column's name by position, for the sort banner. Falls back to the position when the sort
    /// and the columns disagree, which can happen for the instant between a new result's columns
    /// landing and its sort being cleared.
    private func columnName(_ index: Int, in columns: [Event.Column]) -> String {
        columns.indices.contains(index) ? columns[index].name : "column \(index + 1)"
    }

    private func rowView(_ row: [String?], index: Int, columns: [Event.Column], widths: [CGFloat]) -> some View {
        HStack(spacing: 0) {
            gutter("\(index + 1)")
            ForEach(Array(columns.enumerated()), id: \.offset) { columnIndex, column in
                let key = CellKey(row: index, column: columnIndex)
                let staged = tab.cellEdits.value(at: key) != nil
                cellView(key: key, original: columnIndex < row.count ? row[columnIndex] : nil,
                         column: column)
                    .frame(width: widths.indices.contains(columnIndex) ? widths[columnIndex] : 120,
                           alignment: isNumeric(column.type) ? .trailing : .leading)
                    .padding(.horizontal, cellPadding)
                    .padding(.vertical, 4)
                    // Under the cell rather than behind the text: the highlight has to cover the
                    // padding too, or a selected block reads as a run of text highlights with the
                    // column separators cutting through it.
                    .background(cellBackground(row: index, column: columnIndex, staged: staged))
                    // A dot as well as the wash: a cell can be both selected and changed, and one
                    // tint cannot say which of the two it is.
                    .overlay(alignment: .topTrailing) {
                        if staged {
                            Circle().fill(Tone.amber).frame(width: 4, height: 4).padding(3)
                        }
                    }
                    .overlay(Rectangle().fill(Tone.ink.opacity(0.05)).frame(width: 1), alignment: .trailing)
            }
        }
        .frame(height: rowHeight)
        .background(index % 2 == 1 ? Tone.ink.opacity(0.03) : Color.clear)
        .contentShape(Rectangle())
        .gesture(selectionDrag(row: index, widths: widths))
    }

    /// The wash behind one cell. A staged change wins over the selection, because what the user has
    /// changed is the thing they most need to see.
    private func cellBackground(row: Int, column: Int, staged: Bool) -> Color {
        if staged { return Tone.amber.opacity(0.20) }
        if tab.cellSelection?.contains(row: row, column: column) == true {
            return Tone.accent.opacity(0.20)
        }
        return .clear
    }

    /// One cell: the editor while it is being edited, the staged text when it has one, and the value
    /// the server sent otherwise.
    @ViewBuilder
    private func cellView(key: CellKey, original: String?, column: Event.Column) -> some View {
        // `stored` is what an edit, a copy and an export see; `shown` is only what is drawn. The
        // display format is rendering-only, so it is applied here and nowhere else.
        let stored = tab.cellEdits.value(at: key) ?? original
        // A hand-written query has no table, so there is nowhere to file a per-column format and
        // the cell shows the server's own text. The format is read from the store rather than held
        // as state, which is why closing the reader — a state change — is what repaints the cell.
        let identity = ColumnFormatStore.identity(connection: tab.connectionID,
                                                  table: tab.sourceTable,
                                                  column: column.name)
        let format = identity.map { ColumnFormatStore.format($0) } ?? .raw
        let shown = stored.map { format.render($0, type: column.type) }
        if editingCell == key {
            TextField("", text: $editingText)
                .textFieldStyle(.plain)
                .font(.mono12)
                .foregroundStyle(Tone.ink)
                .focused($editorFocused)
                .onSubmit { commitEdit(at: key) }
                .onExitCommand { cancelEdit() }
                .onAppear { editorFocused = true }
        } else if GridValue.isOpenable(value: stored, type: column.type) {
            // A structured value — ARRAY, MAP, ROW, JSON, or JSON that happens to live in a varchar
            // — opens its whole self instead of standing in for it with one truncated line. The
            // double-click is the one editing uses; the difference is that for a value the reader
            // can show, reading it is the more useful default, and the context menu still offers
            // Edit Cell… for the other one.
            cell(shown)
                .contentShape(Rectangle())
                .onTapGesture(count: 2) { viewingCell = key }
                .popover(isPresented: popoverBinding(for: key), arrowEdge: .bottom) {
                    // The reader gets the stored value and the cell's origin, so its format menu
                    // can file a choice under the same identity the grid reads it from.
                    CellValueViewer(value: stored ?? "", column: column.name, type: column.type,
                                    connectionID: tab.connectionID, table: tab.sourceTable)
                }
        } else {
            cell(shown)
                .contentShape(Rectangle())
                // Double-click, the gesture every table editor uses. The context menu carries the
                // same action, because a drag gesture sits on the row and which of the two claims a
                // click is not something a snapshot can prove.
                .onTapGesture(count: 2) { beginEdit(at: key) }
        }
    }

    /// The text a cell currently shows: the staged edit when there is one, the server's value
    /// otherwise. The same rule `cellView` uses, so the menu and the reader cannot disagree with
    /// the cell about what it holds.
    private func cellValue(at key: CellKey) -> String? {
        tab.cellEdits.value(at: key) ?? originalText(at: key)
    }

    /// The selected cell when its value is one the reader can open, or `nil` when it is not — or
    /// when nothing is selected. Drives the context menu item's enabled state.
    private func selectedCell() -> (value: String, column: Event.Column)? {
        guard let preview = tab.preview, let selection = tab.cellSelection,
              preview.columns.indices.contains(selection.left) else { return nil }
        let column = preview.columns[selection.left]
        let key = CellKey(row: selection.top, column: selection.left)
        guard let value = cellValue(at: key),
              GridValue.isOpenable(value: value, type: column.type) else { return nil }
        return (value, column)
    }

    /// Open the reader on the selection's top-left cell, the cell its own double-click would open.
    private func viewSelectedValue() {
        guard let selection = tab.cellSelection, selectedCell() != nil else { return }
        viewingCell = CellKey(row: selection.top, column: selection.left)
    }

    /// The reader's presentation, bound to one cell's key so only that cell's popover can be open.
    private func popoverBinding(for key: CellKey) -> Binding<Bool> {
        Binding(get: { viewingCell == key }, set: { if !$0 { viewingCell = nil } })
    }

    /// Open the editor over one cell, seeded with what the cell currently shows.
    private func beginEdit(at key: CellKey) {
        editingCell = key
        editingText = tab.cellEdits.value(at: key) ?? originalText(at: key) ?? ""
    }

    /// Take the editor's text into the queue.
    ///
    /// Over the whole selection when one covers more than the edited cell: "select these cells and
    /// set them all to this" is one gesture, not N, and it is the bulk edit the selection was drawn
    /// for.
    private func commitEdit(at key: CellKey) {
        if let selection = tab.cellSelection, selection.cellCount > 1 {
            tab.cellEdits.fill(editingText, over: selection, rows: displayedRows)
        } else {
            tab.cellEdits.edit(editingText, at: key, original: originalText(at: key))
        }
        cancelEdit()
    }

    private func cancelEdit() {
        editingCell = nil
        editingText = ""
    }

    /// The value the server sent for a cell, before any staged edit.
    private func originalText(at key: CellKey) -> String? {
        guard displayedRows.indices.contains(key.row) else { return nil }
        let row = displayedRows[key.row]
        return row.indices.contains(key.column) ? row[key.column] : nil
    }

    /// Open the editor from the menu, over the selection's top-left cell.
    private func beginEditingSelection() {
        guard let selection = tab.cellSelection else { return }
        beginEdit(at: CellKey(row: selection.top, column: selection.left))
    }

    /// Put the clipboard's block into the selection, anchored at its top-left corner the way a
    /// spreadsheet takes a paste.
    private func pasteIntoSelection() {
        guard let selection = tab.cellSelection, let preview = tab.preview,
              let text = NSPasteboard.general.string(forType: .string) else { return }
        tab.cellEdits.paste(text, at: CellKey(row: selection.top, column: selection.left),
                            rows: displayedRows, columnCount: preview.columns.count)
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
        return WritePlan.build(edits: tab.cellEdits, rows: displayedRows,
                               columns: preview.columns, table: tab.sourceTable,
                               kind: connection.kind)
    }

    /// The drag that selects a block of cells.
    ///
    /// `minimumDistance: 0` so a plain click selects the one cell under the pointer: a selection
    /// that only appears after a two-cell drag is a selection nobody finds.
    private func selectionDrag(row: Int, widths: [CGFloat]) -> some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { value in
                guard let preview = tab.preview, !preview.columns.isEmpty, !displayedRows.isEmpty
                else { return }
                let target = geometry.cell(at: value.location, inRow: row, widths: widths,
                                           lastRow: displayedRows.count - 1,
                                           lastColumn: preview.columns.count - 1)
                if dragAnchor == nil { dragAnchor = target }
                guard let anchor = dragAnchor else { return }
                tab.cellSelection = CellRange(from: anchor, to: target)
            }
            .onEnded { _ in dragAnchor = nil }
    }

    /// The arithmetic that turns a drag into a cell, as a value the tests can reach. Built from the
    /// same three numbers the rows and columns are drawn with, so the two cannot drift.
    private var geometry: GridGeometry {
        GridGeometry(gutterWidth: gutterWidth, cellPadding: cellPadding, rowHeight: rowHeight)
    }

    /// The row-number column, shared by the header and every row so they cannot drift apart.
    private func gutter(_ text: String) -> some View {
        Text(text)
            .font(.code(10.5))
            .foregroundStyle(Tone.ink.opacity(0.35))
            .frame(width: 44, alignment: .trailing)
            .padding(.horizontal, cellPadding)
            .padding(.vertical, 6)
            .overlay(Rectangle().fill(Tone.ink.opacity(0.05)).frame(width: 1), alignment: .trailing)
    }

    /// A NULL is not an empty string and must not look like one: it is italic and dim, the same
    /// convention every database client uses.
    @ViewBuilder private func cell(_ value: String?) -> some View {
        if let value {
            if value.isEmpty {
                Text("∅").font(.mono12).foregroundStyle(Tone.ink.opacity(0.3))
            } else {
                Text(value)
                    .font(.mono12)
                    .foregroundStyle(Tone.ink.opacity(0.9))
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .help(value)
            }
        } else {
            Text("null").italic().font(.mono12).foregroundStyle(Tone.ink.opacity(0.3))
        }
    }

    /// What the footer claims. Never "N rows" for a limited result without saying so, and once the
    /// server has been asked, the two numbers appear together.
    private func summaryText(_ preview: PreviewResult) -> String {
        // A run in flight has no count yet, and "0 rows" is exactly what this footer says for a
        // finished, empty result — the one reading it must not give while a result is still
        // arriving. What has landed so far is worth saying, so it is said once there is something.
        if let label = loadingLabel {
            return preview.rows.isEmpty
                ? label
                : "\(label) · \(pluralized(preview.rows.count, "row")) so far"
        }
        // A plan is not a row count. The grid draws it because a plan *is* a result set, but the
        // footer must not report rows for it, and the total/limit controls below are meaningless.
        if tab.showingPlan {
            return "Query plan · \(pluralized(preview.rows.count, "line"))"
        }
        let fetched = tab.columnFilters.isEmpty
            ? preview.rows.count
            : displayedRows.count
        let scope = tab.columnFilters.isEmpty ? "" : " of \(preview.rows.count.formatted())"

        if let total = tab.totalRows {
            return "\(fetched.formatted())\(scope) of \(total.formatted()) rows"
        }
        return tab.columnFilters.isEmpty
            ? preview.summary
            : "\(fetched.formatted())\(scope) rows"
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

    /// The distinct values a column actually holds in the fetched rows. Empty when the column has
    /// too many to browse — that is the signal to fall back to a search box.
    private func distinctValues(_ index: Int) -> [String?] {
        guard let preview = tab.preview else { return [] }
        return ColumnFilter.distinctValues(in: preview.rows, column: index)
    }

    /// A funnel per column, always visible and dim until it has something to say: a filter that
    /// only appears on hover is a filter nobody finds.
    private func filterButton(_ index: Int) -> some View {
        let filter = tab.columnFilters[index]
        let active = !(filter?.isEmpty ?? true)
        return Button { filteringColumn.wrappedValue = index } label: {
            // A filled funnel means something is filtered; a half-filled one means the picker has a
            // selection but the popover is closed. Both read as "this column is not showing
            // everything", which is the only thing the header has to communicate.
            Image(systemName: active ? "line.3.horizontal.decrease.circle.fill"
                                     : "line.3.horizontal.decrease.circle")
                .font(.system(size: 10))
                .foregroundStyle(active ? Tone.accent : Tone.ink.opacity(0.30))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.trailing, 6)
        .padding(.top, 4)
        .help(active ? "Filtered by \(filter?.label ?? "")" : "Filter this column")
        .popover(isPresented: Binding(get: { filteringColumn.wrappedValue == index },
                                      set: { if !$0 { filteringColumn.wrappedValue = nil } })) {
            filterEditor(index)
        }
    }

    /// The filter popover, in whichever of its two shapes this column's data calls for.
    @ViewBuilder private func filterEditor(_ index: Int) -> some View {
        let column = tab.preview.flatMap { index < $0.columns.count ? $0.columns[index] : nil }
        let values = distinctValues(index)
        let browsable = values.count <= ColumnFilter.valuePickerLimit

        VStack(alignment: .leading, spacing: 10) {
            SectionLabel(text: "Filter \(column?.name ?? "column")")
            if browsable {
                ValuePickerList(tab: tab, index: index, values: values)
            } else {
                SearchFilterField(tab: tab, index: index)
            }
            Text("This narrows the \((tab.preview?.rows.count ?? 0).formatted()) rows already "
                 + "fetched — it does not re-run the query, so a row outside the limit is not "
                 + "searched.")
                .font(.ui(11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) {
                if browsable {
                    Text("\(values.count) distinct value\(values.count == 1 ? "" : "s")")
                        .font(.ui(10.5))
                        .foregroundStyle(Tone.secondary)
                }
                Spacer(minLength: 0)
                PillButton(title: "Clear", role: .quiet, compact: true) { tab.columnFilters[index] = nil }
            }
        }
        .padding(14)
        .frame(width: 300)
    }

    private func isNumeric(_ type: String) -> Bool {
        let lowered = type.lowercased()
        return ["int", "long", "double", "decimal", "real", "bigint", "smallint", "tinyint", "numeric", "float"]
            .contains { lowered.contains($0) }
    }

    private func typeTint(_ type: String) -> Color {
        if isNumeric(type) { return Tone.mint }
        let lowered = type.lowercased()
        if lowered.contains("bool") { return Tone.violet }
        if lowered.contains("date") || lowered.contains("time") { return Tone.amber }
        return Tone.ice
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
                    .foregroundStyle(preview.truncated || !tab.columnFilters.isEmpty ? Tone.amber : Tone.secondary)
                countControl
                // The commit pair, beside the count button: the grid's two ways of asking the server
                // something about what is on screen — how many rows, and "make these changes real".
                // Both appear only when they have something to act on.
                if !tab.cellEdits.isEmpty {
                    Text("· \(changeLabel.lowercased())")
                        .font(.ui(11))
                        .foregroundStyle(Tone.amber)
                    IconButton(symbol: "checkmark.circle",
                               help: "Review and commit the \(changeLabel.lowercased())",
                               diameter: 22) { reviewingChanges = true }
                    IconButton(symbol: "arrow.uturn.backward",
                               help: "Discard the \(changeLabel.lowercased())",
                               diameter: 22) { tab.cellEdits.discard() }
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
            } else {
                Text("No result yet").font(.ui(11)).foregroundStyle(Tone.secondary)
            }

            Spacer(minLength: 8)

            // Neither control means anything for a plan: there is no limit to set on EXPLAIN, and
            // counting the lines of a plan is not a question anyone has.
            if !tab.showingPlan {
                HStack(spacing: 5) {
                    Text("LIMIT").font(.ui(10, weight: .semibold)).tracking(0.6)
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

/// The header's own sentence about the sort.
///
/// A chevron on a column says *which* column and which way; it cannot say that this is an order over
/// the rows already fetched. That is the part the plan asked to be written down, because a sorted
/// grid looks exactly like a sorted result and the difference matters: the rows outside the limit are
/// not in this order at all.
///
/// A view of its own rather than four lines inside `headerRow` so the sentence can be rendered and
/// looked at without standing up the whole grid.
struct GridSortBanner: View {
    let column: String
    let direction: GridSort.Direction
    let fetched: Int
    let onClear: () -> Void

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "arrow.up.arrow.down")
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(Tone.accent)
            Text("Sorted by \(column) \(direction == .ascending ? "↑" : "↓") · in memory over the "
                 + "\(pluralized(fetched, "row")) fetched — not the whole result.")
                .font(.ui(10.5))
                .foregroundStyle(Tone.secondary)
                .fixedSize()
            Button(action: onClear) {
                Image(systemName: "xmark.circle.fill")
                    .font(.system(size: 10))
                    .foregroundStyle(Tone.ink.opacity(0.35))
            }
            .buttonStyle(.plain)
            .help("Clear the sort and go back to the server's own order")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 4)
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
/// Running the statements is deliberately not wired here yet: it needs a write command in the engine
/// (the FFI's command list is a contract, so that is its own change). Until then the statements are
/// the user's to read and take away, which is the safe half of the feature rather than a stub.
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

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(title)
                .font(.ui(13, weight: .semibold))
                .foregroundStyle(Tone.ink)
            Text(plan.table.map { "Against \($0). Each row is matched by every column at the value it "
                              + "was fetched with, so two identical rows would both be changed. The "
                              + "plan runs in one transaction and is rolled back if a statement "
                              + "affects a different number of rows than expected." }
                 ?? "This tab does not know which table it is showing, so there is nothing to write "
                    + "back to. Open the table from the tree to edit its rows.")
                .font(.ui(11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)

            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(Array(statements.enumerated()), id: \.offset) { _, statement in
                        HStack(alignment: .top, spacing: 8) {
                            Text(statement.kind.rawValue)
                                .font(.code(9, weight: .semibold))
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
                .font(.ui(10.5))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
