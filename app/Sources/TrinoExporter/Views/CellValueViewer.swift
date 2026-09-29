import AppKit
import SwiftUI

/// One cell's whole value, opened from the grid and shown in full.
///
/// The grid draws a single truncated line per cell; an `ARRAY`, `MAP`, `ROW` or `JSON` cell is a
/// whole structure hidden behind that line. This is the surface that opens it: pretty-printed when
/// the text parses as JSON, and the server's raw text when it does not — the case for a PostgreSQL
/// array literal like `{1,NULL,3}`, which is a real state rather than a fallback bug
/// (`docs/golden-deltas.md` D-9).
///
/// A viewer, not an editor: the value is read-only, and nothing here changes what the cell holds or
/// what an export writes.
///
/// Two limits, both from `GridValue` and both stated rather than silent: a value over
/// `parseLimit` is not parsed, and one over `textLimit` is truncated **with a marker**. A cell is
/// untrusted input, and parsing or laying out a multi-megabyte one is a hang the user cannot
/// cancel. The Copy button always copies the whole value, which is what the limits protect.
struct CellValueViewer: View {
    let value: String
    let column: String
    let type: String

    var body: some View {
        // Computed once here rather than in three computed properties: `prettyPrinted` parses the
        // value, and asking it twice is the same mistake the grid just stopped making.
        let pretty = GridValue.prettyPrinted(value)
        let shown = pretty ?? GridValue.displayText(value)
        let truncated = pretty == nil && (value as NSString).length > GridValue.textLimit

        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Text(column)
                    .font(.code(12, weight: .semibold))
                    .foregroundStyle(Tone.ink)
                    .lineLimit(1)
                Chip(text: type, tint: Tone.violet)
                Spacer(minLength: 8)
                PillButton(title: "Copy", symbol: "doc.on.doc", role: .secondary, compact: true) {
                    copy()
                }
            }

            ScrollView([.horizontal, .vertical]) {
                Text(shown)
                    .font(.mono12)
                    .foregroundStyle(Tone.ink.opacity(0.9))
                    .textSelection(.enabled)
                    .fixedSize(horizontal: true, vertical: true)
            }
            .frame(minHeight: 120, maxHeight: 380)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(10)
            .background(Tone.recess.opacity(0.30),
                        in: RoundedRectangle(cornerRadius: 8, style: .continuous))

            Text(note(pretty: pretty != nil, truncated: truncated))
                .font(.ui(11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(16)
        .frame(width: 560)
    }

    /// What the reader has to say about the value it is showing, and why.
    private func note(pretty: Bool, truncated: Bool) -> String {
        if truncated {
            return "Too large to show in full, so the end is cut off and marked. Copy still takes "
                + "the whole value."
        }
        if pretty {
            return "Shown as formatted JSON. The cell itself is unchanged."
        }
        return "Not JSON, so this is the text the server sent — a PostgreSQL array literal "
            + "arrives this way."
    }

    /// The raw text, not the pretty one: a copy out of a viewer should paste what the server stored,
    /// which is what every other copy in this app does.
    private func copy() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(value, forType: .string)
    }
}
