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

    private var gutterWidth: CGFloat { 44 + 16 }

    var body: some View {
        VStack(spacing: 0) {
            content
            footer
        }
    }

    /// The rows the grid draws: everything fetched, narrowed by whatever filters are set.
    /// Filtering happens here and nowhere else — it never reaches the server and never rewrites the
    /// statement, which is why the footer says how many rows it had to work with.
    private var filteredRows: [[String?]] {
        guard let preview = tab.preview else { return [] }
        guard !tab.columnFilters.isEmpty else { return preview.rows }
        return preview.rows.filter { row in
            tab.columnFilters.allSatisfy { index, filter in
                filter.matches(index < row.count ? row[index] : nil)
            }
        }
    }

    @ViewBuilder private var content: some View {
        if tab.previewing, tab.preview == nil {
            status("Running…", symbol: nil)
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
                .font(.system(size: 11.5))
                .foregroundStyle(tint)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 420)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    private func grid(_ preview: PreviewResult) -> some View {
        // The reader has to be outside the scroller: inside it, `geometry` would report the
        // content's width, which is the thing being decided.
        GeometryReader { geometry in
            let widths = widths(fitting: geometry.size.width)
            ScrollView([.horizontal, .vertical]) {
                LazyVStack(alignment: .leading, spacing: 0, pinnedViews: [.sectionHeaders]) {
                    Section {
                        ForEach(Array(filteredRows.enumerated()), id: \.offset) { index, row in
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
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private func headerRow(_ columns: [Event.Column], widths: [CGFloat]) -> some View {
        HStack(spacing: 0) {
            gutter("#")
            ForEach(Array(columns.enumerated()), id: \.offset) { index, column in
                VStack(alignment: isNumeric(column.type) ? .trailing : .leading, spacing: 3) {
                    Text(column.name)
                        .font(.system(size: 12, weight: .semibold, design: .monospaced))
                        .foregroundStyle(Tone.ink.opacity(0.92))
                        .lineLimit(1)
                    Chip(text: column.type, tint: typeTint(column.type))
                }
                .frame(width: widths.indices.contains(index) ? widths[index] : 120,
                       alignment: isNumeric(column.type) ? .trailing : .leading)
                .padding(.horizontal, 8)
                .padding(.vertical, 6)
                .overlay(alignment: .topTrailing) { filterButton(index) }
                .overlay(Rectangle().fill(Tone.ink.opacity(0.05)).frame(width: 1), alignment: .trailing)
            }
        }
        .background(Color(hex: 0x141726))
        .overlay(Rectangle().fill(Tone.ink.opacity(0.12)).frame(height: 1), alignment: .bottom)
    }

    private func rowView(_ row: [String?], index: Int, columns: [Event.Column], widths: [CGFloat]) -> some View {
        HStack(spacing: 0) {
            gutter("\(index + 1)")
            ForEach(Array(columns.enumerated()), id: \.offset) { columnIndex, column in
                cell(columnIndex < row.count ? row[columnIndex] : nil)
                    .frame(width: widths.indices.contains(columnIndex) ? widths[columnIndex] : 120,
                           alignment: isNumeric(column.type) ? .trailing : .leading)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .overlay(Rectangle().fill(Tone.ink.opacity(0.05)).frame(width: 1), alignment: .trailing)
            }
        }
        .background(index % 2 == 1 ? Tone.ink.opacity(0.03) : Color.clear)
    }

    /// The row-number column, shared by the header and every row so they cannot drift apart.
    private func gutter(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 10.5, design: .monospaced))
            .foregroundStyle(Tone.ink.opacity(0.35))
            .frame(width: 44, alignment: .trailing)
            .padding(.horizontal, 8)
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
        // A plan is not a row count. The grid draws it because a plan *is* a result set, but the
        // footer must not report rows for it, and the total/limit controls below are meaningless.
        if tab.showingPlan {
            return "Query plan · \(pluralized(preview.rows.count, "line"))"
        }
        let fetched = tab.columnFilters.isEmpty
            ? preview.rows.count
            : filteredRows.count
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
                    Text("Counting…").font(.system(size: 11)).foregroundStyle(Tone.secondary)
                }
            } else if let error = tab.countError {
                Text(error)
                    .font(.system(size: 11))
                    .foregroundStyle(Tone.coral)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .frame(maxWidth: 260, alignment: .leading)
                    .help(error)
            } else if tab.totalRows == nil {
                PillButton(title: "Count all", symbol: "number", role: .quiet, compact: true) {
                    model.countRows(tab)
                }
                .help("Ask the server how many rows this statement really returns. "
                      + "Runs a second query over the whole result, so it can be slow.")
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
                .font(.system(size: 11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) {
                if browsable {
                    Text("\(values.count) distinct value\(values.count == 1 ? "" : "s")")
                        .font(.system(size: 10.5))
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
                    .font(.system(size: 11))
                    .foregroundStyle(preview.truncated || !tab.columnFilters.isEmpty ? Tone.amber : Tone.secondary)
                countControl
                if preview.elapsedMS > 0 {
                    Text("· \(preview.elapsedMS) ms").font(.system(size: 11)).foregroundStyle(Tone.secondary)
                }
            } else {
                Text("No result yet").font(.system(size: 11)).foregroundStyle(Tone.secondary)
            }

            Spacer(minLength: 8)

            // Neither control means anything for a plan: there is no limit to set on EXPLAIN, and
            // counting the lines of a plan is not a question anyone has.
            if !tab.showingPlan {
                HStack(spacing: 5) {
                    Text("LIMIT").font(.system(size: 10, weight: .semibold)).tracking(0.6)
                        .foregroundStyle(Tone.secondary)
                    TextField("1000", value: $tab.rowLimit, format: .number.grouping(.never))
                        .textFieldStyle(.plain)
                        .font(.system(size: 11, design: .monospaced))
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
                    .font(.system(size: 11))
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
                    .font(.system(size: 11.5, design: .monospaced))
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
                .font(.system(size: 10.5))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
