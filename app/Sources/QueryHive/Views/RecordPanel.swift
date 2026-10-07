import AppKit
import SwiftUI

/// One column of the cursor's row, without its value.
///
/// The value is read separately and lazily: filtering by name must work for a 500-column result
/// without reading a single cell, and a field the list never scrolls to never formats its text.
struct RecordField: Identifiable, Equatable {
    /// The source column the value is read by and a staged edit is keyed by.
    let source: Int
    /// Where the grid draws it, which is what the cursor counts.
    let position: Int
    /// The drawn label: the user's rename, or the server's name.
    let name: String
    /// The server's own name, which is what a column's saved display format is filed under.
    let columnName: String
    let type: String
    let state: CellEdits.CellState

    var id: Int { source }
}

/// What the Record panel decides, as pure functions so the tests read the same answers the view does.
enum RecordFields {
    /// Rows a value may take before "Show more".
    static let maxLines = 6
    /// A guess at characters per line in the 340 pt panel at 12 pt mono, used only to bound how much
    /// of a huge value is handed to `Text`. It is not what decides "Show more" for a value that
    /// slips under it: word-wrap leaves room unused at the end of each row, so the row measures the
    /// real layout against its `lineLimit` (see `RecordFieldRow`).
    static let charsPerLine = 40
    /// The longest value "Show more" opens in place. Longer ones, and structured or binary ones,
    /// open the Cell reader, which has the mode picker and Copy.
    static let expandLimit = 10_000

    /// A field's value as the panel holds it.
    enum Value: Equatable {
        case text(String)
        case null
        /// A cell of an added row that the user has not filled: the database will choose.
        case defaulted
    }

    /// What the row looks like on screen.
    struct Shown: Equatable {
        enum Kind { case value, null, empty, defaulted

            /// Shape, not tone, tells NULL and DEFAULT from a string that spells the same word, as
            /// the grid does: italic, and "∅" for an empty string.
            var italic: Bool { self == .null || self == .defaulted }
        }
        var text: String
        var kind: Kind
        /// There is more of the value than `text` holds.
        var cut: Bool
    }

    /// The fields of one row, in the order the grid draws them: hidden columns are gone and moved
    /// columns are where they were moved to.
    static func fields(row: Int, columns: [Event.Column], layout: GridColumnLayout,
                       edits: CellEdits) -> [RecordField] {
        let staged = !edits.isEmpty
        return layout.visible.enumerated().map { position, source in
            let column = columns.indices.contains(source) ? columns[source] : nil
            return RecordField(source: source, position: position,
                               name: layout.label(source, original: columns),
                               columnName: column?.name ?? "", type: column?.type ?? "",
                               state: staged ? edits.state(of: CellKey(row: row, column: source)) : .unchanged)
        }
    }

    /// "Find field" and "Edited only". Names only, case-insensitive: nothing here reads a value.
    static func filtered(_ fields: [RecordField], matching query: String, editedOnly: Bool) -> [RecordField] {
        let needle = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard editedOnly || !needle.isEmpty else { return fields }
        return fields.filter { field in
            (!editedOnly || field.state != .unchanged)
                && (needle.isEmpty || field.name.localizedCaseInsensitiveContains(needle))
        }
    }

    /// One field's value: the staged text over the fetched one, or what an added row holds.
    /// `fetched` is the row's values in drawn order, as `readRow` returns them.
    static func value(of field: RecordField, row: Int, fetched: [String?], edits: CellEdits) -> Value {
        if row < 0 {
            guard let text = edits.insertedValue(row: row, column: field.source), !text.isEmpty else {
                return .defaulted
            }
            return .text(text)
        }
        if let staged = edits.value(at: CellKey(row: row, column: field.source)) { return .text(staged) }
        guard fetched.indices.contains(field.position), let text = fetched[field.position] else { return .null }
        return .text(text)
    }

    /// The text to draw for a value, under the column's display format.
    ///
    /// A value is untrusted input of any size. The format is applied only to values the JSON reader
    /// would itself parse, and the clip looks at no more characters than could fit.
    static func shown(_ value: Value, type: String, format: ColumnFormat, nullDisplay: String,
                      expanded: Bool) -> Shown {
        switch value {
        case .null:
            return Shown(text: nullDisplay.isEmpty ? "NULL" : nullDisplay, kind: .null, cut: false)
        case .defaulted:
            return Shown(text: "DEFAULT", kind: .defaulted, cut: false)
        case .text(let raw):
            if raw.isEmpty { return Shown(text: "\u{2205}", kind: .empty, cut: false) }
            let rendered = raw.utf8.count <= GridValue.parseLimit ? format.render(raw, type: type) : raw
            if expanded, !wantsReader(raw, type: type) {
                return Shown(text: rendered, kind: .value, cut: true)
            }
            let clipped = clip(rendered)
            return Shown(text: clipped.text, kind: .value, cut: clipped.cut)
        }
    }

    /// Whether "Show more" belongs to the Cell reader rather than to the field: a structured or
    /// binary value, JSON in a text column, or text too long to lay out inside a list of fields.
    static func wantsReader(_ raw: String, type: String) -> Bool {
        raw.utf8.count > expandLimit || GridValue.isOpenable(value: raw, type: type)
    }

    /// The first `lines` visual rows of a text, and whether anything was left out.
    static func clip(_ text: String, lines: Int = maxLines, width: Int = charsPerLine) -> (text: String, cut: Bool) {
        // Nothing past this can be on screen, so nothing past it is scanned.
        let head = text.prefix(lines * width + lines)
        var kept: [Substring] = []
        var used = 0
        for line in head.split(separator: "\n", omittingEmptySubsequences: false) {
            let need = max(1, (line.count + width - 1) / width)
            if used + need > lines {
                let room = lines - used
                if room > 0 { kept.append(line.prefix(room * width)) }
                return (kept.joined(separator: "\n"), true)
            }
            kept.append(line)
            used += need
        }
        return (kept.joined(separator: "\n"), head.endIndex != text.endIndex)
    }

    /// What VoiceOver reads for a field: "<column>, <type>: <value>, <status>".
    static func accessibilityLabel(_ field: RecordField, shown: Shown) -> String {
        let value: String = switch shown.kind {
        case .value: shown.text
        case .null: "null"
        case .empty: "empty"
        case .defaulted: "default"
        }
        let status: String? = switch field.state {
        case .unchanged: nil
        case .modified: "edited"
        case .inserted: "added"
        case .deleted: "deleted"
        }
        return ["\(field.name), \(field.type): \(value)", status].compactMap { $0 }.joined(separator: ", ")
    }

    /// What is said when the cursor moves to another row.
    static func announcement(row: Int) -> String {
        row < 0 ? "Record: new row" : "Record: row \((row + 1).formatted())"
    }

    /// One row's values in drawn order, or why they could not be read. Refuses a partial answer: a
    /// field showing NULL because a read failed would be a statement about the data.
    static func readRow(_ rows: any ResultRows, row: Int, sources: [Int]) -> Result<[String?], RecordReadFailure> {
        guard row >= 0, row < rows.count else { return .failure(RecordReadFailure(message: "The row is no longer in the result.")) }
        guard !sources.isEmpty else { return .success([]) }
        do {
            guard let values = try rows.rowsOrThrow(in: row..<row + 1, columns: sources).first,
                  values.count == sources.count else {
                return .failure(RecordReadFailure(message: "The row could not be read."))
            }
            return .success(values)
        } catch {
            return .failure(RecordReadFailure(message: "Could not read this row: \(error.localizedDescription)"))
        }
    }
}

struct RecordReadFailure: Error, Equatable {
    let message: String
}

extension QueryTab {
    /// Put the grid's cursor and selection on one field of the row the Record panel shows, and
    /// return the cell's key (source column; an added row's key carries its negative id). `nil` when
    /// the cursor is on no row of the table, in which case nothing moves.
    @discardableResult
    func placeCursor(on field: RecordField) -> CellKey? {
        guard let row = cellCursor?.focus.row,
              let key = rowSpace.cellKey(forTableRow: row, source: field.source) else { return nil }
        let position = CellPos(row: row, column: field.position)
        selectCells(anchor: position, focus: position)
        return key
    }
}

/// The last row read, kept so typing in "Find field" does not read it again. A class, so the view
/// can fill it from its body without that being a state change.
@MainActor
final class RecordRowCache {
    struct Key: Hashable {
        let row: Int
        let sources: [Int]
        /// Both move when the rows the grid draws are a different set or have grown.
        let revision: Int
        let fetched: Int
    }

    private var key: Key?
    private var stored: Result<[String?], RecordReadFailure> = .success([])

    func values(for key: Key, read: () -> Result<[String?], RecordReadFailure>) -> Result<[String?], RecordReadFailure> {
        if self.key == key { return stored }
        stored = read()
        self.key = key
        return stored
    }
}

/// The cursor's row read as fields: one per visible column, name and type above the value (FR-GRID-13).
///
/// A reader, not an editor: edits stay in the grid, and a click on a field only puts the grid's
/// cursor on that cell. Shown in place of the Cell reader when `QueryTab.recordMode` is on.
struct RecordPanel: View {
    @Bindable var tab: QueryTab
    /// Put the grid's cursor on this field's cell.
    var showInGrid: (RecordField) -> Void
    /// Hand this field's value to the Cell reader.
    var openReader: (RecordField) -> Void

    @State private var query = ""
    @State private var editedOnly = false
    /// Sources whose value is open in full, for the row on screen.
    @State private var expanded: Set<Int> = []
    @State private var cache = RecordRowCache()

    /// The row the cursor is on, as the key it is edited under: a fetched row's index, or an added
    /// row's negative id. The cursor itself is in table rows (`GridRowSpace`), so it is converted.
    private var row: Int? {
        tab.cellCursor.flatMap { tab.rowSpace.cellKey(forTableRow: $0.focus.row, source: 0)?.row }
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            controls
            Rectangle().fill(Tone.ink.opacity(0.07)).frame(height: 1)
            content
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Record")
        // Debounced by the task's own cancellation: arrowing down twelve rows says the last one.
        .task(id: row) {
            guard let row else { return }
            try? await Task.sleep(for: .milliseconds(250))
            if !Task.isCancelled { Announcer.post(RecordFields.announcement(row: row)) }
        }
        .onChange(of: row) { _, _ in expanded = [] }
    }

    // MARK: Header and controls

    private var header: some View {
        Segmented(selection: $tab.recordMode, options: [false, true]) { $0 ? "Record" : "Cell" }
            .padding(.horizontal, 12)
            .frame(height: 36)
    }

    private var controls: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                Image(systemName: "magnifyingglass").font(.system(size: 11)).foregroundStyle(Tone.secondary)
                    .accessibilityHidden(true)
                TextField("Find field", text: $query)
                    .textFieldStyle(.plain)
                    .font(.ui(11.5))
                    .accessibilityLabel("Find field")
                if !query.isEmpty {
                    Button { query = "" } label: { Image(systemName: "xmark.circle.fill").font(.system(size: 11)) }
                        .buttonStyle(.plain)
                        .foregroundStyle(Tone.ink.opacity(0.35))
                        .accessibilityLabel("Clear the field search")
                }
            }
            .padding(.horizontal, 8)
            .frame(height: 28)
            .background(Tone.recess.opacity(0.28), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            InlineCheckbox(label: "Edited only", isOn: $editedOnly)
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 8)
    }

    // MARK: Fields

    @ViewBuilder private var content: some View {
        if let row, rowExists(row), let preview = tab.preview {
            let fields = RecordFields.fields(row: row, columns: preview.columns,
                                             layout: tab.columnLayout, edits: tab.cellEdits)
            let shown = RecordFields.filtered(fields, matching: query, editedOnly: editedOnly)
            // Only a fetched row has anything to read; an added row holds what the user typed.
            let read: Result<[String?], RecordReadFailure> = row < 0 ? .success([]) : cache.values(
                for: .init(row: row, sources: tab.visibleColumnSources, revision: tab.gridRevision,
                           fetched: tab.fetchedRows),
                read: { RecordFields.readRow(tab.result, row: row, sources: tab.visibleColumnSources) })
            switch read {
            case .failure(let failure):
                note(failure.message)
            case .success(let fetched):
                if shown.isEmpty {
                    note(editedOnly && query.isEmpty ? "No field of this row is edited."
                        : "No field matches.")
                } else {
                    list(shown, row: row, total: fields.count, fetched: fetched)
                }
            }
        } else {
            note("Select a cell to see its row.")
        }
    }

    private func rowExists(_ row: Int) -> Bool {
        row < 0 ? tab.cellEdits.inserted.contains { $0.id == row } : row < tab.result.count
    }

    private func list(_ fields: [RecordField], row: Int, total: Int, fetched: [String?]) -> some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 10) {
                Text(row < 0 ? "New row" : "Row \((row + 1).formatted()) · \(pluralized(total, "field"))")
                    .font(.ui(11))
                    .foregroundStyle(Tone.secondary)
                ForEach(fields) { field in
                    RecordFieldRow(
                        field: field,
                        value: RecordFields.value(of: field, row: row, fetched: fetched, edits: tab.cellEdits),
                        format: ColumnFormatStore.identity(connection: tab.connectionID, table: tab.sourceTable,
                                                           column: field.columnName)
                            .map { ColumnFormatStore.format($0) } ?? .raw,
                        expanded: expanded.contains(field.source),
                        showInGrid: { showInGrid(field) },
                        more: { more(field, fetched: fetched, row: row) })
                }
            }
            .padding(12)
        }
    }

    /// "Show more" and "Show less". A value too big or too structured for the list goes to the
    /// Cell reader instead.
    private func more(_ field: RecordField, fetched: [String?], row: Int) {
        if expanded.contains(field.source) { expanded.remove(field.source); return }
        if case .text(let raw) = RecordFields.value(of: field, row: row, fetched: fetched, edits: tab.cellEdits),
           RecordFields.wantsReader(raw, type: field.type) {
            openReader(field)
        } else {
            expanded.insert(field.source)
        }
    }

    private func note(_ text: String) -> some View {
        Text(text)
            .font(.ui(11.5))
            .foregroundStyle(Tone.secondary)
            .multilineTextAlignment(.center)
            .padding(16)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// One field: the name and type on a line, the value under it.
struct RecordFieldRow: View {
    let field: RecordField
    let value: RecordFields.Value
    let format: ColumnFormat
    let expanded: Bool
    let showInGrid: () -> Void
    let more: () -> Void
    /// Told when the value is drawn with its end cut off by the row limit.
    var overflowChanged: (Bool) -> Void = { _ in }

    /// The value's height with no limit, and with the row limit: where they differ, `lineLimit` is
    /// hiding the end, however the line wrapped.
    @State private var fullHeight: CGFloat = 0
    @State private var limitedHeight: CGFloat = 0
    private var overflows: Bool { fullHeight > limitedHeight + 1 }

    var body: some View {
        let shown = RecordFields.shown(value, type: field.type, format: format,
                                       nullDisplay: DataPreferences.shared.nullDisplay, expanded: expanded)
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 6) {
                mark
                Text(field.name)
                    .font(.code(11.5, weight: .semibold))
                    .foregroundStyle(Tone.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 6)
                Chip(text: field.type, tint: Tone.violet, kind: "Type")
            }
            Text(shown.text)
                .font(.code(12))
                .italic(shown.kind.italic)
                .strikethrough(field.state == .deleted)
                .foregroundStyle(Tone.ink.opacity(ink(shown.kind)))
                .lineLimit(expanded ? nil : RecordFields.maxLines)
                .frame(maxWidth: .infinity, alignment: .leading)
                .onGeometryChange(for: CGFloat.self, of: { $0.size.height }, action: { limitedHeight = $0 })
                .background(alignment: .top) {
                    Text(shown.text)
                        .font(.code(12))
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .hidden()
                        .onGeometryChange(for: CGFloat.self, of: { $0.size.height }, action: { fullHeight = $0 })
                }
            if cut(shown) {
                Button(expanded ? "Show less" : "Show more", action: more)
                    .buttonStyle(.plain)
                    .font(.ui(11, weight: .medium))
                    .foregroundStyle(Tone.accent)
            }
        }
        .padding(6)
        .background(wash, in: RoundedRectangle(cornerRadius: 6, style: .continuous))
        .contentShape(Rectangle())
        .onTapGesture(perform: showInGrid)
        // One element per field; the button inside it is not reachable once its children are
        // ignored, so "Show more" is an action of the element as well.
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(RecordFields.accessibilityLabel(field, shown: shown))
        .accessibilityActions {
            Button("Show in grid", action: showInGrid)
            if cut(shown) { Button(expanded ? "Show less" : "Show more", action: more) }
        }
        .onChange(of: overflows) { _, now in overflowChanged(now) }
    }

    /// "Show more" is owed when the estimate cut the text or the layout did.
    private func cut(_ shown: RecordFields.Shown) -> Bool { shown.cut || overflows }

    /// NULL is the grid's own 0.60; the other quiet states take the staged-text level, which holds
    /// its contrast under the washes.
    private func ink(_ kind: RecordFields.Shown.Kind) -> Double {
        switch kind {
        case .value: 0.9
        case .null: 0.60
        case .empty, .defaulted: GridInk.secondaryText
        }
    }

    /// The staged mark in the grid's own vocabulary (§4.2): a dot, a plus, a minus. Shape first, so
    /// it reads without colour.
    @ViewBuilder private var mark: some View {
        switch field.state {
        case .unchanged:
            EmptyView()
        case .modified:
            Circle().fill(Tone.markAmber).frame(width: 4, height: 4)
        case .inserted:
            Text("+").font(.system(size: 11, weight: .bold)).foregroundStyle(Tone.markMint)
        case .deleted:
            Text("\u{2212}").font(.system(size: 11, weight: .bold)).foregroundStyle(Tone.markCoral)
        }
    }

    private var wash: Color {
        switch field.state {
        case .unchanged: .clear
        case .modified: Tone.markAmber.opacity(GridInk.changedWash)
        case .inserted: Tone.markMint.opacity(GridInk.insertedWash)
        case .deleted: Tone.markCoral.opacity(GridInk.deletedWash)
        }
    }
}
