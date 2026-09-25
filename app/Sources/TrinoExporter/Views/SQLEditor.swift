import AppKit
import SwiftUI

/// The SQL editor.
///
/// A `TextEditor` cannot do this job: SwiftUI's version exposes no caret, no selection and no
/// key interception, so there is nowhere to hang a completion list. This wraps an `NSTextView`
/// instead and drives the app's own popup (`SuggestionPopup`), which is why the suggestions can
/// wear the module's colours rather than AppKit's panel.
///
/// AppKit's built-in completion (`completions(forPartialWordRange:…)`, Option-Esc) is
/// deliberately suppressed: two completion lists fighting over the arrow keys is worse than one.
struct SQLEditor: NSViewRepresentable {
    @Binding var text: String
    @Binding var focused: Bool
    /// UTF-16 caret offset and selection, republished on every selection change.
    @Binding var caret: Int
    @Binding var selection: NSRange
    /// Shared with the popup overlay drawn by the parent.
    let completion: EditorCompletion
    /// Candidates for a typed prefix. `path` is the `.`-separated qualifier the word is being
    /// written under — empty for a bare word, `["hive", "analytics"]` for `hive.analytics.` — which
    /// is what decides whether the answer is a table in that schema, a schema in that catalog, or
    /// a catalog in that connection. Without it the list is every object in the tree.
    let candidates: (_ prefix: String, _ path: [String]) -> [SQLSuggestion]

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeNSView(context: Context) -> NSView {
        let container = FlippedContainerView()
        container.autoresizesSubviews = true

        let textView = SQLTextView()
        textView.isRichText = false
        textView.isEditable = true
        textView.isSelectable = true
        textView.allowsUndo = true
        textView.font = FontChoice.codeNSFont(size: 13, weight: .regular)
        // Adaptive, not pinned white: this NSTextView draws on `Tone.canvas`, which is near-black
        // in a dark appearance and off-white in a light one. AppKit resolves both of these per
        // appearance, so the caret and the default text colour follow the canvas with no observer.
        textView.textColor = .labelColor
        textView.insertionPointColor = .labelColor
        textView.drawsBackground = false
        textView.backgroundColor = .clear
        // Every one of these would corrupt SQL: a quote would become curly, "--" an em dash, and
        // "..." a single ellipsis character.
        textView.isAutomaticQuoteSubstitutionEnabled = false
        textView.isAutomaticDashSubstitutionEnabled = false
        textView.isAutomaticTextReplacementEnabled = false
        textView.isAutomaticSpellingCorrectionEnabled = false
        textView.isContinuousSpellCheckingEnabled = false
        textView.isGrammarCheckingEnabled = false
        textView.smartInsertDeleteEnabled = false
        textView.textContainerInset = NSSize(width: 8, height: 9)
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.autoresizingMask = [.width]
        textView.minSize = NSSize(width: 0, height: 0)
        textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        textView.textContainer?.widthTracksTextView = true
        textView.string = text
        textView.delegate = context.coordinator

        let scrollView = NSScrollView()
        scrollView.documentView = textView
        scrollView.hasVerticalScroller = true
        scrollView.drawsBackground = false
        scrollView.borderType = .noBorder
        scrollView.autoresizingMask = [.width, .height]
        scrollView.frame = container.bounds
        container.addSubview(scrollView)

        // The gutter. Created after the document view is set, because the ruler needs a scroll view
        // to attach to.
        let ruler = LineNumberRulerView(textView: textView)
        scrollView.verticalRulerView = ruler
        scrollView.hasVerticalRuler = true
        scrollView.rulersVisible = true

        context.coordinator.textView = textView
        context.coordinator.container = container
        context.coordinator.ruler = ruler
        ruler.update(for: textView.string)
        textView.interceptKey = { [weak coordinator = context.coordinator] event in
            coordinator?.handle(event) ?? false
        }
        // Whatever the tab was holding when it opened — restored SQL, a loaded file, a table just
        // double-clicked — is coloured once here. `updateNSView` cannot do it: it returns early
        // when the string already matches, and re-running the scan on every SwiftUI update would
        // make typing pay for the model's changes.
        context.coordinator.recolour()
        return container
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        // The coordinator reads bindings and callbacks through `parent`, so it has to be refreshed
        // on every update or it keeps calling into a stale view value.
        context.coordinator.parent = self
        // Before the early return below: the gutter's font follows the code-font setting, and a
        // setting change does not alter the text, so it would otherwise never reach the ruler.
        context.coordinator.ruler?.numberFont = FontChoice.codeNSFont(size: 10.5, weight: .regular)
        guard let textView = context.coordinator.textView else { return }
        guard textView.string != text else { return }
        let previous = textView.string
        let caret = textView.selectedRange().location
        textView.string = text
        let length = (text as NSString).length
        // Text appended from outside the editor — double-clicking a table in the tree — should
        // leave the caret at the end of what was just written, not back where it used to be.
        let appended = length > previous.count && text.hasPrefix(previous)
        textView.setSelectedRange(NSRange(location: appended ? length : min(caret, length), length: 0))
        // Text that arrived from outside the editor — opening a table, loading a file — has never
        // been through `textDidChange` and would otherwise stay uncoloured.
        context.coordinator.recolour()
    }

    // MARK: Coordinator

    @MainActor
    final class Coordinator: NSObject, NSTextViewDelegate {
        var parent: SQLEditor
        weak var textView: SQLTextView?
        weak var container: NSView?
        weak var ruler: LineNumberRulerView?

        private var debounce: DispatchWorkItem?
        private var suppressAutoTrigger = false
        private var isColouring = false

        init(_ parent: SQLEditor) {
            self.parent = parent
        }

        // MARK: Text changes

        func textDidChange(_ notification: Notification) {
            guard let textView = notification.object as? NSTextView else { return }
            parent.text = textView.string
            colour(textView)
            if suppressAutoTrigger {
                // The change we just made was accepting a suggestion; re-opening the list over
                // the word it just inserted would be maddening.
                suppressAutoTrigger = false
                parent.completion.dismiss()
                return
            }
            debounce?.cancel()
            let work = DispatchWorkItem { [weak self] in self?.refresh(manual: false) }
            debounce = work
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.08, execute: work)
        }

        /// Attributes only — the string is never touched, so this cannot loop back through
        /// `textDidChange`, and the binding keeps whatever the user typed.
        func recolour() {
            guard let textView else { return }
            colour(textView)
        }

        private func colour(_ textView: NSTextView) {
            guard !isColouring else { return }
            isColouring = true
            SQLSyntax.apply(to: textView)
            isColouring = false
            // The gutter is numbered from the text, so it has to be told when the text changed.
            // `NSRulerView` handles scrolling on its own; it cannot know about typing.
            ruler?.update(for: textView.string)
        }

        func textDidBeginEditing(_ notification: Notification) {
            parent.focused = true
        }

        func textDidEndEditing(_ notification: Notification) {
            parent.focused = false
            parent.completion.dismiss()
        }

        func textViewDidChangeSelection(_ notification: Notification) {
            guard let textView else { return }
            // Run needs both: which statement the caret is in, and what is highlighted.
            parent.caret = textView.selectedRange().location
            parent.selection = textView.selectedRange()
            // Clicking somewhere else with the list open should close it, not leave it pinned to
            // the old caret. Typing keeps it open because that path re-runs `refresh`.
            guard parent.completion.active else { return }
            if (textView.string as NSString).length == 0 { parent.completion.dismiss() }
        }

        // MARK: Key handling

        /// Returns true when the editor consumed the event.
        func handle(_ event: NSEvent) -> Bool {
            // ⌃Space opens the list on demand, the way every code editor does it. Option-Esc is
            // AppKit's own "complete:" binding; taking it keeps `SQLTextView`'s word-based
            // completion panel from appearing behind ours.
            if event.modifierFlags.contains(.control), event.charactersIgnoringModifiers == " " {
                refresh(manual: true)
                return true
            }
            if event.keyCode == 53, event.modifierFlags.contains(.option) {
                refresh(manual: true)
                return true
            }
            guard parent.completion.active else { return false }
            switch event.keyCode {
            case 53:                                    // esc
                parent.completion.dismiss()
                return true
            case 125 where !event.modifierFlags.contains(.command):   // ↓
                parent.completion.move(1)
                return true
            case 126 where !event.modifierFlags.contains(.command):   // ↑
                parent.completion.move(-1)
                return true
            case 36, 76, 48:                            // return, keypad enter, tab
                accept()
                return true
            default:
                return false
            }
        }

        private func accept() {
            guard let textView, parent.completion.active else { return }
            let item = parent.completion.items[parent.completion.selected]
            suppressAutoTrigger = true
            textView.insertText(item.text, replacementRange: wordRange(in: textView))
            parent.completion.dismiss()
        }

        // MARK: Candidates

        private func refresh(manual: Bool) {
            guard let textView else { return }
            guard let context = wordContext(in: textView) else {
                parent.completion.dismiss()
                return
            }
            // Two characters is the floor for typing; ⌃Space and the word after a dot are
            // explicit enough to offer the whole list.
            if !manual, context.prefix.count < context.minimumPrefix {
                parent.completion.dismiss()
                return
            }
            let items = parent.candidates(context.prefix, context.path)
            guard !items.isEmpty else {
                parent.completion.dismiss()
                return
            }
            parent.completion.items = items
            parent.completion.selected = 0
            parent.completion.anchor = caretRect(in: textView)
        }

        private struct WordContext {
            let prefix: String
            /// The `.`-separated segments the word is being qualified *by*, outermost first. Typing
            /// `hive.analytics.` gives `["hive", "analytics"]` with an empty prefix; typing
            /// `hive.analytics.pen` gives the same path with the prefix `pen`. Empty when the word
            /// stands alone.
            let path: [String]
            let minimumPrefix: Int
        }

        private func wordContext(in textView: NSTextView) -> WordContext? {
            let text = textView.string as NSString
            let caret = textView.selectedRange().location
            guard caret <= text.length else { return nil }
            // No SQL suggestions inside a string literal: the user is writing data, not code.
            guard !insideStringLiteral(text, upTo: caret) else { return nil }
            guard let parsed = WordScope.parse(text, upTo: caret) else { return nil }
            return WordContext(prefix: parsed.prefix, path: parsed.path,
                               minimumPrefix: parsed.path.isEmpty ? 2 : 0)
        }

        private func wordRange(in textView: NSTextView) -> NSRange {
            let text = textView.string as NSString
            let caret = textView.selectedRange().location
            var start = caret
            while start > 0, Self.isWordCharacter(text.character(at: start - 1)) { start -= 1 }
            return NSRange(location: start, length: caret - start)
        }

        private static func isWordCharacter(_ character: unichar) -> Bool {
            guard let scalar = Unicode.Scalar(character) else { return false }
            return CharacterSet.alphanumerics.contains(scalar) || character == 0x5F  // _
        }

        /// A `.` between two name segments.
        private static func isQualifierSeparator(_ character: unichar) -> Bool {
            character == 0x2E
        }

        /// The quote characters the three drivers use for identifiers: `"` for Trino and Postgres,
        /// a backtick for MySQL.
        private static func isQuote(_ character: unichar) -> Bool {
            character == 0x22 || character == 0x60
        }

        /// An odd number of unescaped quotes before the caret means the caret is inside one.
        private func insideStringLiteral(_ text: NSString, upTo caret: Int) -> Bool {
            var open = false
            var index = 0
            while index < caret {
                if text.character(at: index) == 0x27 {       // '
                    if index + 1 < caret, text.character(at: index + 1) == 0x27 {
                        index += 2                            // '' is an escaped quote
                        continue
                    }
                    open.toggle()
                }
                index += 1
            }
            return open
        }

        /// Where the caret is, in the container's coordinate space — which is flipped, so the
        /// rect can be handed straight to a SwiftUI `.offset`.
        private func caretRect(in textView: NSTextView) -> CGRect {
            guard let container, let window = textView.window else { return .zero }
            let screen = textView.firstRect(forCharacterRange: textView.selectedRange(), actualRange: nil)
            let inWindow = window.convertFromScreen(screen)
            return container.convert(inWindow, from: nil)
        }
    }
}

/// SwiftUI lays its overlays out from the top-left; `NSView` defaults to the bottom-left, which
/// would put the popup at the wrong end of the editor.
final class FlippedContainerView: NSView {
    override var isFlipped: Bool { true }
}

/// `NSTextView` that lets the coordinator see keys before AppKit does, and that refuses to show
/// AppKit's own completion list.
final class SQLTextView: NSTextView {
    var interceptKey: ((NSEvent) -> Bool)?

    override func keyDown(with event: NSEvent) {
        if let interceptKey, interceptKey(event) { return }
        super.keyDown(with: event)
    }
}

/// The word under the caret and the qualifier it sits under, parsed out of the text.
///
/// Split out of the editor's coordinator as a pure function so it can be tested directly: this is
/// the part that decides *which* schema's tables an answer may contain, and getting it wrong is
/// silent — the list still looks plausible, it is just mostly wrong. The coordinator only has the
/// `NSTextView` wiring around it.
enum WordScope {
    struct Parsed: Equatable {
        /// The partial word being typed, which the candidate list filters by.
        let prefix: String
        /// The `.`-separated names the word is qualified by, outermost first and without quotes.
        /// `hive.analytics.pen` gives `["hive", "analytics"]` and the prefix `pen`.
        let path: [String]
    }

    /// Parse the word that would be completed at `caret`.
    ///
    /// Returns nil when there is nothing to complete at that position.
    static func parse(_ text: NSString, upTo caret: Int) -> Parsed? {
        guard caret <= text.length else { return nil }
        var start = caret
        while start > 0, isWordCharacter(text.character(at: start - 1)) { start -= 1 }
        let prefix = text.substring(with: NSRange(location: start, length: caret - start))

        // Walk back over the qualifier. This is what tells the candidate list which schema's tables
        // to offer: without it every table in the tree is a candidate for every position, so
        // `hive.analytics.` offered tables from other schemas and other connections entirely.
        var path: [String] = []
        var cursor = start
        while cursor > 0, isSeparator(text.character(at: cursor - 1)) {
            let end = cursor - 1
            // A quoted identifier is one segment even though it can contain spaces and dots:
            // `"my schema"` and `"a.b"` are each a single name to the server.
            if end > 0, isQuote(text.character(at: end - 1)) {
                let quote = text.character(at: end - 1)
                var begin = end - 1
                while begin > 0, text.character(at: begin - 1) != quote { begin -= 1 }
                guard begin > 0 else { break }
                path.insert(text.substring(with: NSRange(location: begin, length: end - 1 - begin)), at: 0)
                cursor = begin - 1
                continue
            }
            var begin = end
            while begin > 0, isWordCharacter(text.character(at: begin - 1)) { begin -= 1 }
            // A `.` with no name before it — `t.` where `t` is not a name, or a leading dot —
            // ends the walk rather than inventing an empty segment.
            guard begin < end else { break }
            path.insert(text.substring(with: NSRange(location: begin, length: end - begin)), at: 0)
            cursor = begin
        }
        return Parsed(prefix: prefix, path: path)
    }

    static func isWordCharacter(_ character: unichar) -> Bool {
        guard let scalar = Unicode.Scalar(character) else { return false }
        return CharacterSet.alphanumerics.contains(scalar) || character == 0x5F  // _
    }

    /// A `.` between two name segments.
    static func isSeparator(_ character: unichar) -> Bool { character == 0x2E }

    /// The identifier quotes the three drivers use: `"` for Trino and Postgres, a backtick for
    /// MySQL.
    static func isQuote(_ character: unichar) -> Bool { character == 0x22 || character == 0x60 }
}

/// The line-number gutter down the left of the editor.
///
/// An `NSRulerView` rather than a SwiftUI column beside the editor. The ruler is part of the scroll
/// view, so AppKit keeps it aligned with the text as it scrolls and as lines wrap; a column drawn
/// next to the editor would have to re-derive the scroll offset every frame and would drift the
/// first time a long line wrapped.
///
/// It numbers **logical** lines — the ones the query has — not the visual fragments wrapping
/// produces, which is also what the corner readout counts. A wrapped continuation gets no number,
/// so the two cannot disagree about how many lines the query is.
final class LineNumberRulerView: NSRulerView {
    private weak var textView: NSTextView?

    /// The font the numbers are drawn in. Set from the same family as the code, so a font chosen in
    /// Settings reaches the gutter instead of leaving it in the system's monospaced face.
    var numberFont: NSFont = .monospacedSystemFont(ofSize: 10.5, weight: .regular) {
        didSet { needsDisplay = true }
    }

    /// Lines in the text, kept by the coordinator on every change. Used only for the gutter's width,
    /// so it is not recomputed per draw.
    private var lineCount = 1

    init(textView: NSTextView) {
        self.textView = textView
        super.init(scrollView: textView.enclosingScrollView, orientation: .verticalRuler)
        clientView = textView
        ruleThickness = Self.width(for: 1)
    }

    required init(coder: NSCoder) {
        fatalError("LineNumberRulerView is only created in code")
    }

    /// The text changed: recount for the width, and repaint.
    func update(for text: String) {
        let lines = text.isEmpty ? 1 : text.reduce(into: 1) { count, character in
            if character == "\n" { count += 1 }
        }
        lineCount = lines
        let wanted = Self.width(for: lines)
        if abs(wanted - ruleThickness) > 0.5 { ruleThickness = wanted }
        needsDisplay = true
    }

    /// Wide enough for the number it will have to show, so the gutter does not jump sideways when
    /// the hundredth line arrives — and no wider, because every point here is a point the text
    /// does not get.
    private static func width(for lines: Int) -> CGFloat {
        let digits = CGFloat(max(2, String(max(lines, 1)).count))
        return digits * 7.5 + 20
    }

    override func drawHashMarksAndLabels(in rect: NSRect) {
        guard let textView, let scrollView else { return }
        // The text view's origin in the ruler's own coordinates. `convert` accounts for one being
        // flipped and the other not, which is the whole reason this is not arithmetic on offsets.
        let origin = convert(NSPoint.zero, from: textView)
        let inset = textView.textContainerInset

        let attributes: [NSAttributedString.Key: Any] = [
            .font: numberFont,
            .foregroundColor: NSColor.secondaryLabelColor,
        ]

        for entry in numberedLines(in: scrollView.contentView.bounds) {
            let label = "\(entry.number)" as NSString
            let size = label.size(withAttributes: attributes)
            label.draw(at: NSPoint(x: ruleThickness - size.width - 8,
                                   y: origin.y + inset.height + entry.minY
                                      + (entry.height - size.height) / 2),
                       withAttributes: attributes)
        }
    }

    /// One number to draw: which line it is, and where its line fragment sits in the text
    /// container's coordinates.
    struct NumberedLine: Equatable {
        let number: Int
        let minY: CGFloat
        let height: CGFloat
    }

    /// Which line numbers to draw for a visible region, and where.
    ///
    /// Split out of the drawing so the decision can be tested without a screen. The decision that
    /// matters is which fragments are *lines*: a query line long enough to wrap produces several
    /// fragments and only the first is a line of the query, so numbering every fragment would
    /// report more lines than the query has — and disagree with the count in the corner, which
    /// counts newlines.
    func numberedLines(in visibleRect: NSRect) -> [NumberedLine] {
        guard let textView,
              let layoutManager = textView.layoutManager,
              let container = textView.textContainer
        else { return [] }

        let string = textView.string as NSString

        // An empty editor still has a line 1, where the caret is. Left to `enumerateLineFragments`
        // it would produce nothing, and the gutter would go blank the moment the last character was
        // deleted.
        if string.length == 0 {
            let height = layoutManager.defaultLineHeight(for: textView.font ?? .systemFont(ofSize: 13))
            return [NumberedLine(number: 1, minY: 0, height: height)]
        }

        let glyphRange = layoutManager.glyphRange(forBoundingRect: visibleRect, in: container)
        let firstCharacter = layoutManager.characterIndexForGlyph(at: glyphRange.location)

        // Counting starts from the top of the text, not from the top of the view, so scrolling
        // does not renumber the query. The count then advances with the fragments in order, which
        // enumerates them front to back.
        var line = 1 + newlines(in: string, before: firstCharacter)
        var cursor = firstCharacter
        var seen = -1
        var out: [NumberedLine] = []

        layoutManager.enumerateLineFragments(forGlyphRange: glyphRange) { fragmentRect, _, _, fragmentGlyphs, _ in
            let index = layoutManager.characterIndexForGlyph(at: fragmentGlyphs.location)
            while cursor < index {
                if string.character(at: cursor) == 0x0A { line += 1 }
                cursor += 1
            }
            // Not the start of a line: a wrapped continuation, which gets no number.
            guard index == 0 || string.character(at: index - 1) == 0x0A else { return }
            // A fragment on the boundary of the visible range can be enumerated twice.
            guard index != seen else { return }
            seen = index
            out.append(NumberedLine(number: line, minY: fragmentRect.minY, height: fragmentRect.height))
        }

        // A trailing newline puts the caret on a line the layout manager has no glyph for, and on
        // some paths it does not create a fragment for it either. The query does have that line —
        // the caret is sitting on it — so it is added here rather than being left to chance.
        if string.character(at: string.length - 1) == 0x0A {
            let height = layoutManager.defaultLineHeight(for: textView.font ?? .systemFont(ofSize: 13))
            let last = out.last
            out.append(NumberedLine(number: line + 1,
                                    minY: last.map { $0.minY + $0.height } ?? 0,
                                    height: height))
        }
        return out
    }

    /// How many newlines the text holds before `index`.
    ///
    /// Searched with `range(of:)` rather than by reading character by character: the call is a scan
    /// in C, and this runs when the view is scrolled, where the text above can be long.
    private func newlines(in string: NSString, before index: Int) -> Int {
        let limit = min(index, string.length)
        guard limit > 0 else { return 0 }
        var count = 0
        var cursor = 0
        while cursor < limit {
            let found = string.range(of: "\n", options: [],
                                     range: NSRange(location: cursor, length: limit - cursor))
            guard found.location != NSNotFound else { break }
            count += 1
            cursor = found.location + 1
        }
        return count
    }
}
