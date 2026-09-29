import AppKit
import SwiftUI

/// One cell's whole value, opened from the grid and shown in full.
///
/// The grid draws a single truncated line per cell; an `ARRAY`, `MAP`, `ROW`, `JSON` or binary cell
/// is a whole structure hidden behind that line. This is the surface that opens it, in whichever of
/// its modes the value supports: **Text**, pretty-printed when the text parses as JSON and the
/// server's raw text when it does not — the case for a PostgreSQL array literal like `{1,NULL,3}`,
/// which is a real state rather than a fallback bug (`docs/golden-deltas.md` D-9); **Tree**, the
/// same parsed JSON collapsible and searchable, capped by `JSONTree.nodeLimit`; and **Hex**, the
/// bytes of a `bytea`/`BLOB`/`varbinary` cell as a dump, capped by `HexDump.limit`.
///
/// A viewer, not an editor: the value is read-only, and nothing here — including the per-column
/// display format — changes what the cell holds or what an export writes.
///
/// The limits are from `GridValue`/`JSONTree`/`HexDump` and all are stated rather than silent: a
/// value over `GridValue.parseLimit` is not parsed (Tree is not offered), one over `textLimit` is
/// truncated **with a marker**, and the tree and dump each say where they stopped. A cell is
/// untrusted input, and parsing or laying out a multi-megabyte one is a hang the user cannot cancel.
/// The Copy button always copies the whole value, which is what the limits protect.
struct CellValueViewer: View {
    /// What the reader can show for one value. Only the modes the value supports are offered.
    enum Mode: String, Identifiable, CaseIterable {
        case text, tree, hex

        var id: Self { self }

        var label: String {
            switch self {
            case .text: "Text"
            case .tree: "Tree"
            case .hex: "Hex"
            }
        }
    }

    let value: String
    let column: String
    let type: String
    /// Where the cell came from, so a display format can be filed per connection + table. `nil` for
    /// a hand-written query, and then the format menu is not offered.
    var connectionID: UUID?
    var table: String?

    /// The modes this value supports, in the order the picker shows them.
    let available: [Mode]
    private let pretty: String?
    private let tree: JSONTree.Outcome
    private let identity: String?

    @State private var mode: Mode
    @State private var format: ColumnFormat

    init(value: String, column: String, type: String,
         connectionID: UUID? = nil, table: String? = nil) {
        self.value = value
        self.column = column
        self.type = type
        self.connectionID = connectionID
        self.table = table

        // Parsed once here rather than in the body: `prettyPrinted` parses the value, and asking it
        // on every state change is the mistake the grid just stopped making.
        let pretty = GridValue.prettyPrinted(value)
        let tree: JSONTree.Outcome = GridValue.looksLikeJSON(value)
            ? JSONTree.build(from: value)
            : .notJSON
        self.pretty = pretty
        self.tree = tree

        var available: [Mode] = [.text]
        if tree.isTree { available.append(.tree) }
        if GridValue.isBinary(type: type) { available.append(.hex) }
        self.available = available

        let identity = ColumnFormatStore.identity(connection: connectionID, table: table, column: column)
        self.identity = identity
        _format = State(initialValue: identity.map { ColumnFormatStore.format($0) } ?? .raw)
        // A JSON cell opens on its tree and a byte cell on its dump, because that is the mode the
        // cell was opened for. Text is always there to fall back to.
        let initial: Mode = tree.isTree ? .tree : (available.contains(.hex) ? .hex : .text)
        _mode = State(initialValue: initial)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header

            content
                .frame(minHeight: 160, maxHeight: 420)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(10)
                .background(Tone.recess.opacity(0.30),
                            in: RoundedRectangle(cornerRadius: 8, style: .continuous))

            Text(note)
                .font(.ui(11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(16)
        .frame(width: 620)
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 8) {
            Text(column)
                .font(.code(12, weight: .semibold))
                .foregroundStyle(Tone.ink)
                .lineLimit(1)
            Chip(text: type, tint: Tone.violet)
            Spacer(minLength: 8)
            if available.count > 1 { modePicker }
            formatMenu
            PillButton(title: "Copy", symbol: "doc.on.doc", role: .secondary, compact: true) {
                copy()
            }
        }
    }

    private var modePicker: some View {
        Picker("", selection: $mode) {
            ForEach(available) { Text($0.label).tag($0) }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(width: CGFloat(available.count) * 56)
    }

    /// The per-column format menu. Only shown when the cell can be filed somewhere: without a
    /// connection and a table there is no identity to store a choice under, and a menu that silently
    /// forgot the choice would be worse than no menu.
    @ViewBuilder private var formatMenu: some View {
        if identity != nil {
            Menu {
                ForEach(ColumnFormat.allCases) { option in
                    Button {
                        setFormat(option)
                    } label: {
                        if option == format {
                            Label(option.label, systemImage: "checkmark")
                        } else {
                            Text(option.label)
                        }
                    }
                }
            } label: {
                HStack(spacing: 4) {
                    Image(systemName: "textformat").font(.system(size: 10, weight: .semibold))
                    Text(format == .raw ? "Format" : format.label)
                }
                .font(.ui(11.5, weight: .medium))
                .foregroundStyle(Tone.ink.opacity(0.85))
            }
            .menuStyle(.button)
            .buttonStyle(.plain)
            .menuIndicator(.hidden)
            .fixedSize()
            .help("Display format for this column, kept for this connection and table; it changes "
                + "what is shown, not what is stored")
        }
    }

    // MARK: Content

    @ViewBuilder private var content: some View {
        switch mode {
        case .text:
            scrolling(Text(displayedText).font(.mono12))
        case .tree:
            if case .tree(let root) = tree {
                JSONTreeView(root: root)
            } else {
                scrolling(Text(displayedText).font(.mono12))
            }
        case .hex:
            scrolling(Text(HexDump.dump(HexDump.bytes(from: value))).font(.mono12))
        }
    }

    /// The scroll-and-select wrapper every text mode shares, so the three cannot drift apart on
    /// selection or wrapping.
    private func scrolling(_ text: Text) -> some View {
        ScrollView([.horizontal, .vertical]) {
            text
                .foregroundStyle(Tone.ink.opacity(0.9))
                .textSelection(.enabled)
                .fixedSize(horizontal: true, vertical: true)
        }
    }

    /// The text the Text mode shows, after the chosen format and the display truncation.
    ///
    /// `Raw` and `JSON` keep the pretty-printed form when there is one: the grid already has the
    /// raw text, and the reason to open a JSON cell is the layout.
    private var displayedText: String {
        let base: String
        if format == .raw || format == .json, let pretty {
            base = pretty
        } else {
            base = format.render(value, type: type)
        }
        return GridValue.displayText(base)
    }

    // MARK: Note

    /// What the reader has to say about the value it is showing, and why.
    private var note: String {
        switch mode {
        case .tree:
            if case .tree(let root) = tree, root.children.contains(where: { $0.isMarker }) {
                return "Tree view. The value has more nodes than this view builds, so the tail is "
                    + "marked and left out; Text still holds everything."
            }
            return "Tree view, parsed from the cell's own text. The cell is unchanged."
        case .hex:
            return "Hex view of the bytes behind the value, its first \(HexDump.limit.formatted()) "
                + "bytes. Copy still takes the value the server sent."
        case .text:
            if case .tooLarge = tree {
                return "Too large to parse as JSON, so the tree is not offered and this is the "
                    + "server's text. Copy keeps the whole value."
            }
            if format != .raw {
                return "Shown with the \(format.label) display format. The stored value is "
                    + "unchanged, and Copy takes it."
            }
            if (value as NSString).length > GridValue.textLimit {
                return "Too large to show in full, so the end is cut off and marked. Copy still "
                    + "takes the whole value."
            }
            if pretty != nil {
                return "Shown as formatted JSON. The cell itself is unchanged."
            }
            return "Not JSON, so this is the text the server sent — a PostgreSQL array literal "
                + "arrives this way."
        }
    }

    // MARK: Actions

    private func setFormat(_ option: ColumnFormat) {
        format = option
        if let identity { ColumnFormatStore.set(option, for: identity) }
    }

    /// The raw text, not the pretty one: a copy out of a viewer should paste what the server stored,
    /// which is what every other copy in this app does.
    private func copy() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(value, forType: .string)
    }
}
