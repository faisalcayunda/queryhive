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
    /// Candidates for a typed prefix. `qualified` is true when the word follows a `.`, which is
    /// the signal to drop keywords and offer objects only.
    let candidates: (_ prefix: String, _ qualified: Bool) -> [SQLSuggestion]

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeNSView(context: Context) -> NSView {
        let container = FlippedContainerView()
        container.autoresizesSubviews = true

        let textView = SQLTextView()
        textView.isRichText = false
        textView.isEditable = true
        textView.isSelectable = true
        textView.allowsUndo = true
        textView.font = .monospacedSystemFont(ofSize: 13, weight: .regular)
        textView.textColor = .white
        textView.insertionPointColor = .white
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

        context.coordinator.textView = textView
        context.coordinator.container = container
        textView.interceptKey = { [weak coordinator = context.coordinator] event in
            coordinator?.handle(event) ?? false
        }
        return container
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        // The coordinator reads bindings and callbacks through `parent`, so it has to be refreshed
        // on every update or it keeps calling into a stale view value.
        context.coordinator.parent = self
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
    }

    // MARK: Coordinator

    @MainActor
    final class Coordinator: NSObject, NSTextViewDelegate {
        var parent: SQLEditor
        weak var textView: SQLTextView?
        weak var container: NSView?

        private var debounce: DispatchWorkItem?
        private var suppressAutoTrigger = false

        init(_ parent: SQLEditor) {
            self.parent = parent
        }

        // MARK: Text changes

        func textDidChange(_ notification: Notification) {
            guard let textView = notification.object as? NSTextView else { return }
            parent.text = textView.string
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
            let items = parent.candidates(context.prefix, context.qualified)
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
            let qualified: Bool
            let minimumPrefix: Int
        }

        private func wordContext(in textView: NSTextView) -> WordContext? {
            let text = textView.string as NSString
            let caret = textView.selectedRange().location
            guard caret <= text.length else { return nil }
            // No SQL suggestions inside a string literal: the user is writing data, not code.
            guard !insideStringLiteral(text, upTo: caret) else { return nil }

            var start = caret
            while start > 0, Self.isWordCharacter(text.character(at: start - 1)) { start -= 1 }
            let prefix = text.substring(with: NSRange(location: start, length: caret - start))
            // A `.` means the word is being qualified, so only objects can be what comes next.
            let qualified = start > 0 && text.character(at: start - 1) == 0x2E
            return WordContext(prefix: prefix, qualified: qualified, minimumPrefix: qualified ? 0 : 2)
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
