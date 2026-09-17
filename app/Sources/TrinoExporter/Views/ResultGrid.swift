import AppKit
import SwiftUI

/// The rows a Run fetched, as a grid. Navicat's answer to "what did the query return", and the
/// reason this app is a query editor rather than a one-way pipe: Run looks, Export writes.
struct ResultGrid: View {
    @Environment(AppModel.self) private var model
    @Bindable var tab: QueryTab
    @State private var confirmReplace = false

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
            return min(max(CGFloat(max(header, longest)) * 7.2 + 20, 84), 320)
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
                        ForEach(Array(preview.rows.enumerated()), id: \.offset) { index, row in
                            rowView(row, index: index, columns: preview.columns, widths: widths)
                        }
                    } header: {
                        headerRow(preview.columns, widths: widths)
                    }
                }
                .frame(minWidth: geometry.size.width, alignment: .topLeading)
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
                Text(preview.summary)
                    .font(.system(size: 11))
                    .foregroundStyle(preview.truncated ? Tone.amber : Tone.secondary)
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
            PillButton(title: tab.destination == .table ? "Save to Table" : "Export",
                       symbol: tab.destination == .table ? "square.and.arrow.down" : "arrow.down.doc") {
                // Replace drops the old table before the query runs; that is worth a question,
                // and the question belongs beside the button that asks it.
                if tab.isDestructive { confirmReplace = true } else { model.run(tab) }
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
