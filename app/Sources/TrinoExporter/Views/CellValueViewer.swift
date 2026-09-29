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
struct CellValueViewer: View {
    let value: String
    let column: String
    let type: String

    /// The pretty form when there is one, the raw text otherwise.
    private var shown: String { GridValue.prettyPrinted(value) ?? value }
    private var isJSON: Bool { GridValue.prettyPrinted(value) != nil }

    var body: some View {
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

            Text(isJSON
                 ? "Shown as formatted JSON. The cell itself is unchanged."
                 : "Not JSON, so this is the text the server sent — a PostgreSQL array literal "
                   + "arrives this way.")
                .font(.ui(11))
                .foregroundStyle(Tone.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(16)
        .frame(width: 560)
    }

    /// The raw text, not the pretty one: a copy out of a viewer should paste what the server stored,
    /// which is what every other copy in this app does.
    private func copy() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(value, forType: .string)
    }
}
