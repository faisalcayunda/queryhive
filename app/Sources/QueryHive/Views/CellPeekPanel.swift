import AppKit
import SwiftUI

/// A value cut to what the peek will hold, and what to say about the cut.
struct PeekText: Equatable {
    var text: String
    var note: String?
}

/// The two numbers a peek needs that have nothing to do with AppKit, pure so they are tested as
/// they are: how much of a value it keeps, and where the panel goes.
enum CellPeekLayout {

    /// The value cut to `maxBytes` of UTF-8, with the note that says so.
    ///
    /// Bytes, because a count of bytes is free on a native string and a count of characters is not:
    /// a ten-megabyte cell must not cost a grapheme walk just to be told it is too long. The cut
    /// lands on a scalar boundary, so the last character is whole.
    static func clip(_ value: String, maxBytes: Int = CellPeekPanel.maxBytes) -> PeekText {
        let total = value.utf8.count
        guard total > maxBytes else { return PeekText(text: value, note: nil) }
        let end = value.utf8.index(value.utf8.startIndex, offsetBy: maxBytes)
        let head = String(value[..<end])
        return PeekText(text: head,
                        note: "Showing the first \(size(maxBytes)) of \(size(total)). ⌘C in the grid "
                            + "copies the whole value; the Copy button here copies only what is shown.")
    }

    static func size(_ bytes: Int) -> String {
        let units = ["bytes", "KiB", "MiB", "GiB"]
        var value = Double(bytes)
        var unit = 0
        while value >= 1_024, unit < units.count - 1 { value /= 1_024; unit += 1 }
        if unit == 0 { return "\(bytes) bytes" }
        let shown = value == value.rounded() ? String(Int(value)) : String(format: "%.1f", value)
        return "\(shown) \(units[unit])"
    }

    /// 4 pt below the cell, flipped above it when it does not fit, kept inside `visible`. Screen
    /// coordinates, origin at the bottom left.
    static func frame(size: CGSize, cell: CGRect, visible: CGRect, gap: CGFloat = 4) -> CGRect {
        var x = min(cell.minX, visible.maxX - size.width)
        x = max(x, visible.minX)
        var y = cell.minY - gap - size.height
        if y < visible.minY { y = cell.maxY + gap }
        y = max(min(y, visible.maxY - size.height), visible.minY)
        return CGRect(x: x, y: y, width: size.width, height: size.height)
    }

    /// The panel's size for a value: as wide as the reader, as tall as the reader's own chrome plus
    /// the first lines of the value (and a line of note when there is one), never more than
    /// 592 × 460. A JSON value opens on its tree, which needs more room than its one compact line
    /// suggests. The 170 is what the reader spends above and below its content: a two-row header, its
    /// own note, and the padding between them, measured from a render.
    static func size(for text: String, hasNote: Bool = false) -> CGSize {
        let lines = min(text.utf8.prefix(CellPeekPanel.maxBytes).reduce(1) { $1 == 0x0A ? $0 + 1 : $0 }, 16)
        var content = max(120, CGFloat(lines) * 15 + 24)
        if let first = text.first(where: { !$0.isWhitespace }), first == "{" || first == "[" {
            content = max(content, 200)
        }
        let height = 170 + content + (hasNote ? 44 : 0)
        return CGSize(width: CellPeekPanel.maxSize.width, height: min(height, CellPeekPanel.maxSize.height))
    }
}

/// What the panel is showing right now. Observed by the panel's own SwiftUI content.
@MainActor @Observable
final class CellPeekModel {
    struct Shown: Equatable {
        var column: String
        var type: String
        var connectionID: UUID?
        var table: String?
        var text: String
        var note: String?
        /// A new identity per update, so the reader starts fresh on each cell rather than keeping
        /// the mode and the parse of the one before.
        var token: Int
    }

    enum Phase: Equatable {
        case reading
        case value(Shown)
        case failed(String)
    }

    var phase: Phase = .reading
}

/// The peek (P-17): the value of the cursor's cell, in a floating panel that never takes the
/// keyboard.
///
/// An `NSPanel` and not `QLPreviewPanel`: Quick Look wants a file, and a cell is not one (NFR-S5:
/// nothing is written to disk for this). It hosts the reader the double-click opens, so a JSON cell
/// peeks as its tree and a bytea as its dump. `.nonactivatingPanel` and a borderless style keep the
/// grid the key view, so the arrow keys that move the cursor still reach the table while the peek
/// is up, and the peek follows them the way Quick Look does in Finder.
@MainActor
final class CellPeekPanel: NSPanel {
    /// The most of a value the panel holds, in UTF-8 bytes: 64 KiB.
    nonisolated static let maxBytes = 65_536
    nonisolated static let minSize = CGSize(width: 320, height: 160)
    nonisolated static let maxSize = CGSize(width: 592, height: 460)

    let model: CellPeekModel
    private let host: NSHostingView<CellPeekContent>
    private var token = 0
    /// Whether the panel is up, kept apart from `isVisible` because AppKit hides a
    /// `hidesOnDeactivate` panel without telling us and brings it back on its own.
    private(set) var isOpen = false

    init() {
        let model = CellPeekModel()
        self.model = model
        let host = NSHostingView(rootView: CellPeekContent(model: model))
        // The panel's size is `CellPeekLayout.size`, set by `present`. Left to itself the hosting
        // view sizes its window to the content's ideal size, which for a 64 KiB value is thousands
        // of points tall.
        host.sizingOptions = []
        self.host = host
        super.init(contentRect: CGRect(origin: .zero, size: Self.maxSize),
                   styleMask: [.nonactivatingPanel, .utilityWindow, .borderless],
                   backing: .buffered, defer: true)
        contentView = host
        isFloatingPanel = true
        becomesKeyOnlyIfNeeded = true
        hidesOnDeactivate = true
        level = .floating
        isReleasedWhenClosed = false
        isOpaque = false
        backgroundColor = .clear
        hasShadow = true
        collectionBehavior = [.transient, .ignoresCycle]
        // Read aloud as one thing; the reader inside is plain static text to VoiceOver.
        setAccessibilityLabel("Cell peek")
    }

    /// A borderless panel does not take the key by default; said out loud because the whole design
    /// leans on it.
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    /// The text on screen, for the tests and for what is announced.
    var shownText: String? {
        if case .value(let shown) = model.phase { return shown.text }
        return nil
    }

    var note: String? {
        if case .value(let shown) = model.phase { return shown.note }
        return nil
    }

    var isReading: Bool { model.phase == .reading }

    /// Show a value. `anchor` is the cell's rectangle in screen coordinates.
    func show(value: String, column: String, type: String, connectionID: UUID?, table: String?,
              extraNote: String? = nil, anchor: CGRect, appearance: NSAppearance?) {
        let clipped = CellPeekLayout.clip(value)
        token += 1
        let note = [extraNote, clipped.note].compactMap { $0 }.joined(separator: " ")
        model.phase = .value(.init(column: column, type: type, connectionID: connectionID, table: table,
                                   text: clipped.text, note: note.isEmpty ? nil : note, token: token))
        present(size: CellPeekLayout.size(for: clipped.text, hasNote: !note.isEmpty), anchor: anchor,
                appearance: appearance)
    }

    func showReading(anchor: CGRect, appearance: NSAppearance?) {
        model.phase = .reading
        present(size: Self.minSize, anchor: anchor, appearance: appearance)
    }

    func showFailure(_ message: String, anchor: CGRect, appearance: NSAppearance?) {
        model.phase = .failed(message)
        present(size: Self.minSize, anchor: anchor, appearance: appearance)
    }

    /// Move to sit under a cell again, because the cursor moved or the grid scrolled.
    func follow(anchor: CGRect) {
        guard isOpen else { return }
        setFrame(Self.frame(size: frame.size, anchor: anchor, on: screen), display: true)
    }

    func dismiss() {
        guard isOpen else { return }
        isOpen = false
        orderOut(nil)
    }

    private func present(size: CGSize, anchor: CGRect, appearance: NSAppearance?) {
        self.appearance = appearance
        let target = Self.frame(size: size, anchor: anchor, on: screen)
        let wasOpen = isOpen
        setFrame(target, display: true)
        isOpen = true
        if !wasOpen {
            // A 0.12 s fade, and no fade at all under Reduce Motion.
            if ThemeStore.shared.surface.reduceMotion {
                alphaValue = 1
                orderFront(nil)
            } else {
                alphaValue = 0
                orderFront(nil)
                NSAnimationContext.runAnimationGroup { context in
                    context.duration = 0.12
                    animator().alphaValue = 1
                }
            }
        } else {
            orderFront(nil)
        }
    }

    private static func frame(size: CGSize, anchor: CGRect, on screen: NSScreen?) -> CGRect {
        let visible = (screen ?? NSScreen.screens.first(where: { $0.frame.intersects(anchor) })
            ?? NSScreen.main)?.visibleFrame ?? CGRect(x: 0, y: 0, width: 1_440, height: 900)
        return CellPeekLayout.frame(size: size, cell: anchor, visible: visible)
    }
}

/// The panel's content: the reader, a note about what was cut, or a word about what is happening.
struct CellPeekContent: View {
    var model: CellPeekModel

    var body: some View {
        Group {
            switch model.phase {
            case .reading:
                status("Reading…")
            case .failed(let message):
                status(message)
            case .value(let shown):
                VStack(spacing: 0) {
                    CellValueViewer(value: shown.text, column: shown.column, type: shown.type,
                                    connectionID: shown.connectionID, table: shown.table,
                                    placement: .panel)
                    if let note = shown.note {
                        Text(note)
                            .font(.ui(11))
                            .foregroundStyle(Tone.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding(.horizontal, 12)
                            .padding(.bottom, 10)
                    }
                }
                .id(shown.token)
            }
        }
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Tone.outline))
    }

    private func status(_ text: String) -> some View {
        Text(text)
            .font(.ui(12))
            .foregroundStyle(Tone.secondary)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

/// The one read a peek makes, in a box a background queue may hold.
///
/// `StoreRows` is safe off the main thread (its reads go through its own lock, and `rowsOrThrow`
/// is documented for it), but the `ResultRows` protocol does not say so, so the box does.
struct PeekReader: @unchecked Sendable {
    let rows: any ResultRows

    /// One cell, by result row and source column. `nil` is a NULL; a failure throws, so a cell that
    /// could not be read is never shown as one that is empty.
    func value(row: Int, column: Int) throws -> String? {
        try rows.rowsOrThrow(in: row..<row + 1, columns: [column]).first?.first ?? nil
    }
}
