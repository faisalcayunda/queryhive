import AppKit
import SwiftUI

/// The rows a Run fetched, as a grid. Navicat's answer to "what did the query return", and the
/// reason this app is a query editor rather than a one-way pipe: Run looks, Export writes.
struct ResultGrid: View {
    @Environment(AppModel.self) private var model
    @Bindable var tab: QueryTab
    @State private var confirmReplace = false
    @State private var filteringColumn: Int?

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
                ColumnFilter.matches(index < row.count ? row[index] : nil, filter)
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
                        .foregroundStyle(.white.opacity(0.92))
                        .lineLimit(1)
                    Chip(text: column.type, tint: typeTint(column.type))
                }
                .frame(width: widths.indices.contains(index) ? widths[index] : 120,
                       alignment: isNumeric(column.type) ? .trailing : .leading)
                .padding(.horizontal, 8)
                .padding(.vertical, 6)
                .overlay(alignment: .topTrailing) { filterButton(index) }
                .overlay(Rectangle().fill(.white.opacity(0.05)).frame(width: 1), alignment: .trailing)
            }
        }
        .background(Color(hex: 0x141726))
        .overlay(Rectangle().fill(.white.opacity(0.12)).frame(height: 1), alignment: .bottom)
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
                    .overlay(Rectangle().fill(.white.opacity(0.05)).frame(width: 1), alignment: .trailing)
            }
        }
        .background(index % 2 == 1 ? Color.white.opacity(0.03) : Color.clear)
    }

    /// The row-number column, shared by the header and every row so they cannot drift apart.
    private func gutter(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 10.5, design: .monospaced))
            .foregroundStyle(.white.opacity(0.35))
            .frame(width: 44, alignment: .trailing)
            .padding(.horizontal, 8)
            .padding(.vertical, 6)
            .overlay(Rectangle().fill(.white.opacity(0.05)).frame(width: 1), alignment: .trailing)
    }

    /// A NULL is not an empty string and must not look like one: it is italic and dim, the same
    /// convention every database client uses.
    @ViewBuilder private func cell(_ value: String?) -> some View {
        if let value {
            if value.isEmpty {
                Text("∅").font(.mono12).foregroundStyle(.white.opacity(0.3))
            } else {
                Text(value)
                    .font(.mono12)
                    .foregroundStyle(.white.opacity(0.9))
                    .lineLimit(1)
                    .truncationMode(.tail)
                    .help(value)
            }
        } else {
            Text("null").italic().font(.mono12).foregroundStyle(.white.opacity(0.3))
        }
    }

    /// What the footer claims. Never "N rows" for a limited result without saying so, and once the
    /// server has been asked, the two numbers appear together.
    private func summaryText(_ preview: PreviewResult) -> String {
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
        if tab.previewedSQL != nil, tab.preview != nil {
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

    /// A funnel per column, always visible and dim until it has something to say: a filter that
    /// only appears on hover is a filter nobody finds.
    private func filterButton(_ index: Int) -> some View {
        let active = !(tab.columnFilters[index] ?? "").isEmpty
        return Button { filteringColumn = index } label: {
            Image(systemName: active ? "line.3.horizontal.decrease.circle.fill"
                                     : "line.3.horizontal.decrease.circle")
                .font(.system(size: 10))
                .foregroundStyle(active ? Tone.ice : .white.opacity(0.30))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.trailing, 6)
        .padding(.top, 4)
        .help(active ? "Filtered by \((tab.columnFilters[index] ?? ""))" : "Filter this column")
        .popover(isPresented: Binding(get: { filteringColumn == index },
                                      set: { if !$0 { filteringColumn = nil } })) {
            filterEditor(index)
        }
    }

    private func filterEditor(_ index: Int) -> some View {
        let column = tab.preview.flatMap { index < $0.columns.count ? $0.columns[index] : nil }
        let binding = Binding<String>(
            get: { tab.columnFilters[index] ?? "" },
            set: { tab.columnFilters[index] = $0.isEmpty ? nil : $0 })
        return VStack(alignment: .leading, spacing: 10) {
            SectionLabel(text: "Filter \(column?.name ?? "column")")
            TextField("contains…", text: binding).field()
            Text("Prefix with =, >, <, >= or <= to compare rather than match. This narrows the "
                 + "\((tab.preview?.rows.count ?? 0).formatted()) rows already fetched — it does "
                 + "not re-run the query, so a row outside the limit is not searched.")
                .font(.system(size: 11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) {
                Spacer()
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

            HStack(spacing: 5) {
                Text("LIMIT").font(.system(size: 10, weight: .semibold)).tracking(0.6)
                    .foregroundStyle(Tone.secondary)
                TextField("1000", value: $tab.rowLimit, format: .number.grouping(.never))
                    .textFieldStyle(.plain)
                    .font(.system(size: 11, design: .monospaced))
                    .frame(width: 52)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 3)
                    .background(Color.black.opacity(0.28), in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 5, style: .continuous).strokeBorder(.white.opacity(0.10)))
            }
            .help("How many rows Run fetches. It does not change the query; the engine stops reading here.")

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
        .overlay(alignment: .top) { Rectangle().fill(.white.opacity(0.07)).frame(height: 1) }
        .confirmationDialog("Replace \(tab.target(for: model.connection(for: tab)?.kind ?? .trino))?",
                            isPresented: $confirmReplace) {
            Button("Drop and Recreate", role: .destructive) { model.run(tab) }
        } message: {
            Text("The existing table is dropped before the query runs. If the query then fails, the table is already gone.")
        }
    }
}
