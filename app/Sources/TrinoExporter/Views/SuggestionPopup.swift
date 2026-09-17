import SwiftUI

/// The suggestion list itself, wearing the module's colours rather than AppKit's completion
/// panel. Rows are keyboard-driven (↑ ↓ ⏎ ⇥ esc) and deliberately not clickable: taking a click
/// would move first responder out of the text view and cost the caret.
struct SuggestionPopup: View {
    let completion: EditorCompletion

    static let width: CGFloat = 268
    static let rowHeight: CGFloat = 23

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(completion.items.enumerated()), id: \.element.id) { index, item in
                HStack(spacing: 8) {
                    Image(systemName: item.kind.symbol)
                        .font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(item.kind.tint)
                        .frame(width: 14)
                    Text(item.text)
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundStyle(index == completion.selected ? .white : .white.opacity(0.85))
                        .lineLimit(1)
                    Spacer(minLength: 10)
                    Text(item.kind.label)
                        .font(.system(size: 10))
                        .foregroundStyle(Tone.secondary)
                }
                .padding(.horizontal, 9)
                .frame(height: Self.rowHeight)
                .background(index == completion.selected ? Color.white.opacity(0.15) : Color.clear,
                            in: RoundedRectangle(cornerRadius: 5, style: .continuous))
            }
        }
        .padding(4)
        .frame(width: Self.width, alignment: .leading)
        .glass(9)
        .shadow(color: .black.opacity(0.55), radius: 18, y: 8)
    }

    static func height(for count: Int) -> CGFloat { CGFloat(count) * rowHeight + 8 }
}

/// Pins the list under the caret, flipping it above when the caret is near the bottom, and keeps
/// it inside the editor horizontally.
struct SuggestionOverlay: View {
    let completion: EditorCompletion

    var body: some View {
        GeometryReader { geometry in
            if completion.active {
                let height = SuggestionPopup.height(for: completion.items.count)
                let below = completion.anchor.maxY + 6
                let y = below + height <= geometry.size.height
                    ? below
                    : max(4, completion.anchor.minY - height - 6)
                let x = min(max(4, completion.anchor.minX),
                            max(4, geometry.size.width - SuggestionPopup.width - 4))
                SuggestionPopup(completion: completion)
                    .offset(x: x, y: y)
            }
        }
        .allowsHitTesting(false)
    }
}
