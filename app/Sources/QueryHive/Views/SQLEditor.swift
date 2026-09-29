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
    /// The editor's own switches, passed as a value: SwiftUI re-applies a representable when the
    /// value it was given changes, and a reference to the store would never change.
    let layout: EditorLayout
    /// Called with a statement's first offset when its run marker is clicked in the gutter. The
    /// host decides what running means; the editor only knows where the statement starts.
    let onRunStatement: ((Int) -> Void)?

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
        textView.textContainerInset = LineNumberRulerView.textInset
        textView.isVerticallyResizable = true
        textView.isHorizontallyResizable = false
        textView.autoresizingMask = [.width]
        textView.minSize = NSSize(width: 0, height: 0)
        textView.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        textView.textContainer?.widthTracksTextView = true
        textView.string = text
        textView.delegate = context.coordinator
        // The caret and selection the tab is already holding. A tab keeps them for Run Current
        // Statement, so an editor built for a tab the user was in the middle of should start where
        // they left off rather than at the top of the file.
        let textLength = (text as NSString).length
        if selection.location <= textLength {
            let location = selection.location
            let selected = min(selection.length, textLength - location)
            textView.setSelectedRange(NSRange(location: location, length: selected))
        }

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

        // The find bar lives in the container, above the scroll view, and starts hidden. The
        // container lays the text view out below it rather than over it, so a find bar open over a
        // query never hides the line the query is on.
        let findBar = SQLFindBar()
        findBar.isHidden = true
        container.addSubview(findBar)
        container.findBar = findBar
        context.coordinator.findBar = findBar
        context.coordinator.wire(findBar)

        context.coordinator.textView = textView
        context.coordinator.container = container
        context.coordinator.ruler = ruler
        context.coordinator.scrollView = scrollView
        ruler.update(for: textView.string)
        // The layout switches, before anything is drawn: the gutter's visibility, the wrapping, the
        // tab stops and the invisible characters are all properties of the text view.
        context.coordinator.applyLayout()
        ruler.onToggleFold = { [weak coordinator = context.coordinator] offset in
            coordinator?.toggleFold(headerOffset: offset)
        }
        ruler.onRun = { [weak coordinator = context.coordinator] offset in
            coordinator?.runStatement(at: offset)
        }
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
        // The same reason: a switch flipped in Settings changes nothing about the text, so the
        // editor has to be told to re-read the layout rather than waiting for an edit.
        context.coordinator.applyLayout()
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
        weak var container: FlippedContainerView?
        weak var ruler: LineNumberRulerView?
        weak var findBar: SQLFindBar?
        weak var scrollView: NSScrollView?

        /// The layout the text view is currently wearing, so a switch is applied when it moves and
        /// not on every SwiftUI update. Re-setting the tab stops rewrites every attribute in the
        /// text storage, which is not something to do on every keystroke.
        private var appliedLayout: EditorLayout?
        /// The statement ranges of the current text, for the caret's own highlight. Recomputed when
        /// the text changes, never when the caret moves.
        private var statementBounds: [NSRange] = []

        private var debounce: DispatchWorkItem?
        private var suppressAutoTrigger = false
        private var isColouring = false
        /// True while the uppercase pass is replacing text, so the `textDidChange` that replacement
        /// raises does not start another one.
        private var isUppercasing = false

        /// The string the last fold-shift was measured against. Folding is keyed by header offset,
        /// so an edit has to be told which text it came from to know how far the offsets moved.
        private var lastString = ""
        /// The foldable regions the current text holds, recomputed whenever it changes.
        private var foldRegions: [FoldRegion] = []
        /// Header offsets the user has folded. Offsets, not line numbers: a line number belongs to
        /// a text, and the text is the thing that keeps changing.
        private var foldedHeaders: Set<Int> = []

        private var findVisible = false
        private var findMatches: [NSRange] = []
        private var findIndex: Int?

        init(_ parent: SQLEditor) {
            self.parent = parent
        }

        // MARK: Layout switches

        /// Uppercase the keyword the keystroke just finished, and say whether it did.
        ///
        /// The edit goes through `shouldChangeText`/`didChangeText` rather than through the text
        /// storage alone, so it is one undoable step and the delegate is told — which is what makes
        /// the syntax pass repaint the word it changed.
        @discardableResult
        private func uppercaseFinishedKeyword(_ textView: NSTextView) -> Bool {
            guard !isUppercasing,
                  let replacement = KeywordCase.replacement(in: textView.string as NSString,
                                                            caret: textView.selectedRange().location)
            else { return false }
            isUppercasing = true
            defer { isUppercasing = false }

            let selected = textView.selectedRange()
            guard textView.shouldChangeText(in: replacement.range,
                                            replacementString: replacement.text) else { return false }
            textView.textStorage?.replaceCharacters(in: replacement.range, with: replacement.text)
            textView.didChangeText()
            // The caret sat after the delimiter, and the word before it got longer or shorter.
            let delta = (replacement.text as NSString).length - replacement.range.length
            textView.setSelectedRange(NSRange(location: selected.location + delta,
                                              length: selected.length))
            return true
        }

        /// Put the editor's switches into the text view, and only the ones that moved.
        ///
        /// Called from both `makeNSView` and `updateNSView`, because a switch flipped in Settings
        /// changes nothing about the text and would otherwise never reach the editor.
        func applyLayout() {
            guard let textView else { return }
            let layout = parent.layout
            guard layout != appliedLayout else {
                updateHighlight(textView)
                return
            }
            let previous = appliedLayout
            appliedLayout = layout
            scrollView?.hasVerticalRuler = layout.showLineNumbers
            scrollView?.rulersVisible = layout.showLineNumbers
            applyWrap(layout)
            textView.layoutManager?.showsInvisibleCharacters = layout.showInvisibles
            if previous?.tabWidth != layout.tabWidth { applyTabWidth(layout) }
            if previous?.codeFolding != layout.codeFolding {
                refreshFolds(previous: textView.string)
            }
            updateRunMarks()
            updateHighlight(textView)
        }

        /// The gutter's run markers, from the statement ranges the text already gave us.
        private func updateRunMarks() {
            guard let ruler else { return }
            ruler.showsRunMarks = parent.layout.runButtonPerStatement
            guard parent.layout.runButtonPerStatement else {
                ruler.runMarks = []
                return
            }
            let lineStarts = SQLFolding.lineStarts(in: (textView?.string ?? "") as NSString)
            ruler.runMarks = statementBounds.compactMap { range in
                let line = SQLFolding.line(containing: range.location, lineStarts: lineStarts)
                return LineNumberRulerView.RunMark(headerLine: line, headerOffset: range.location)
            }
        }

        /// Run the statement that starts at `offset`.
        ///
        /// The offset is put where a run reads it — the tab's own selection — and then the host is
        /// asked to run, so the gutter and the Run menu take the same path through the model.
        func runStatement(at offset: Int) {
            guard let textView else { return }
            let length = (textView.string as NSString).length
            guard offset >= 0, offset <= length else { return }
            let selection = NSRange(location: offset, length: 0)
            textView.setSelectedRange(selection)
            parent.selection = selection
            parent.caret = offset
            parent.onRunStatement?(offset)
        }

        /// Wrap, or run off the right edge. In AppKit these are one decision: a container that
        /// tracks the text view's width wraps, and one with an unbounded width scrolls.
        private func applyWrap(_ layout: EditorLayout) {
            guard let textView, let container = textView.textContainer else { return }
            textView.isHorizontallyResizable = !layout.wordWrap
            container.widthTracksTextView = layout.wordWrap
            container.containerSize = NSSize(width: layout.wordWrap ? 0 : CGFloat.greatestFiniteMagnitude,
                                             height: CGFloat.greatestFiniteMagnitude)
            textView.autoresizingMask = layout.wordWrap ? [.width] : []
            scrollView?.hasHorizontalScroller = !layout.wordWrap
        }

        /// Tab stops at the chosen width. A paragraph style is an attribute, so the text already
        /// there has to be given it as well as the default for what comes next — and the syntax
        /// pass has to run again, because it is what re-applies the colours the attribute change
        /// would otherwise leave stale.
        private func applyTabWidth(_ layout: EditorLayout) {
            guard let textView else { return }
            let font = textView.font ?? FontChoice.codeNSFont(size: 13, weight: .regular)
            let space = (" " as NSString).size(withAttributes: [.font: font]).width
            let style = NSMutableParagraphStyle()
            style.defaultTabInterval = space * CGFloat(layout.tabWidth)
            style.tabStops = []
            textView.defaultParagraphStyle = style
            let whole = NSRange(location: 0, length: (textView.string as NSString).length)
            textView.textStorage?.addAttribute(.paragraphStyle, value: style, range: whole)
            colour(textView)
        }

        /// The bands behind the text: the caret's statement first, then its line over it.
        private func updateHighlight(_ textView: SQLTextView) {
            let layout = parent.layout
            guard layout.highlightCurrentLine || layout.highlightCurrentStatement else {
                textView.highlightRanges = []
                return
            }
            let text = textView.string as NSString
            let caret = min(textView.selectedRange().location, text.length)
            var ranges: [NSRange] = []
            if layout.highlightCurrentStatement, text.length > 0,
               let statement = statementBounds.first(where: { NSLocationInRange(caret, $0) }) {
                ranges.append(statement)
            }
            if layout.highlightCurrentLine, text.length > 0 {
                ranges.append(text.lineRange(for: NSRange(location: caret, length: 0)))
            }
            textView.highlightColour = NSColor(Tone.accent.opacity(0.15))
            textView.highlightRanges = ranges
        }

        // MARK: Text changes

        func textDidChange(_ notification: Notification) {
            guard let textView = notification.object as? NSTextView else { return }
            // Before anything reads the text: this rewrites the word just finished, and the fold,
            // colour and completion passes should see the text that will actually be sent.
            if parent.layout.autoUppercaseKeywords, uppercaseFinishedKeyword(textView) {
                // The replacement raised `textDidChange` again, and that pass has already done the
                // rest of this work. Doing it here as well would scan the document twice per word.
                return
            }
            parent.text = textView.string
            // Folds are recomputed before the syntax pass, because the syntax pass is what applies
            // them: the other order would paint the previous text's folds onto this one.
            refreshFolds(previous: lastString)
            lastString = textView.string
            colour(textView)
            if findVisible { findQueryChanged(findBar?.query ?? "") }
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
            refreshFolds(previous: lastString)
            lastString = textView.string
            colour(textView)
        }

        private func colour(_ textView: NSTextView) {
            guard !isColouring else { return }
            isColouring = true
            SQLSyntax.apply(to: textView)
            // The syntax pass resets every attribute, so the folds have to be re-applied after it
            // rather than once at fold time: otherwise the first keystroke anywhere would unfold
            // every collapsed region by repainting over it.
            SQLFoldStyler.apply(folded, to: textView)
            isColouring = false
            // The gutter is numbered from the text, so it has to be told when the text changed.
            // `NSRulerView` handles scrolling on its own; it cannot know about typing.
            ruler?.update(for: textView.string)
            updateFoldMarks()
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
            // The bands follow the caret, which is the whole point of them.
            updateHighlight(textView)
            // A caret moving into a collapsed body would be invisible, and the next keystroke would
            // land somewhere the user cannot see. Opening the fold is the honest answer.
            if !foldedHeaders.isEmpty {
                let caret = textView.selectedRange().location
                let inside = foldRegions.contains { region in
                    region.body.length > 0 && NSLocationInRange(caret, region.body)
                }
                if inside {
                    unfoldRegions(containing: caret)
                    colour(textView)
                }
            }
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
            // Find and folding come before the completion guard: a request to find something or to
            // collapse a statement means the list is finished with, not that the key is theirs.
            if let shortcut = EditorShortcut(event: event) {
                switch shortcut {
                case .find:
                    showFind(replacing: false)
                    return true
                case .replace:
                    showFind(replacing: true)
                    return true
                case .findNext:
                    guard findVisible else { return false }
                    findNext()
                    return true
                case .findPrevious:
                    guard findVisible else { return false }
                    findPrevious()
                    return true
                case .fold:
                    foldAtCaret()
                    return true
                case .unfold:
                    unfoldAtCaret()
                    return true
                case .dismiss:
                    guard findVisible else { return false }
                    closeFind()
                    return true
                }
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

        // MARK: Find

        func wire(_ bar: SQLFindBar) {
            bar.onQueryChanged = { [weak self] query in self?.findQueryChanged(query) }
            bar.onFindNext = { [weak self] in self?.findNext() }
            bar.onFindPrevious = { [weak self] in self?.findPrevious() }
            bar.onReplace = { [weak self] in self?.replaceCurrent() }
            bar.onReplaceAll = { [weak self] in self?.replaceAllMatches() }
            bar.onClose = { [weak self] in self?.closeFind() }
        }

        func showFind(replacing: Bool) {
            guard let textView else { return }
            let wasVisible = findVisible
            findBar?.isReplacing = replacing
            findVisible = true
            findBar?.isHidden = false
            container?.findBarVisible = true
            // Seeding from a selection is what every editor does: select a word, press ⌘F, and it
            // is the query. A multi-line selection is not a search term, and a bar that is already
            // open must not overwrite what the user typed with whatever is now selected in the text.
            if !wasVisible {
                let selection = textView.selectedRange()
                if selection.length > 0, selection.length <= 200 {
                    let selected = (textView.string as NSString).substring(with: selection)
                    if selected.rangeOfCharacter(from: .newlines) == nil {
                        findBar?.setQuery(selected)
                    }
                }
            }
            findBar?.focusSearch()
            findQueryChanged(findBar?.query ?? "")
        }

        func closeFind() {
            findVisible = false
            findMatches = []
            findIndex = nil
            clearFindHighlight()
            findBar?.isHidden = true
            container?.findBarVisible = false
            if let textView { textView.window?.makeFirstResponder(textView) }
        }

        private func findQueryChanged(_ query: String) {
            guard let textView else { return }
            let options = findBar?.options ?? []
            // An invalid pattern is a state the bar shows, not a search: running it would find
            // nothing and read as "No matches", which is a different and wrong answer.
            if let error = FindReplace.patternError(needle: query, options: options) {
                findMatches = []
                findIndex = nil
                updateFindHighlight()
                findBar?.setPatternError(error)
                return
            }
            findMatches = FindReplace.ranges(in: textView.string as NSString, needle: query,
                                             options: options)
            findIndex = FindReplace.currentIndex(of: findMatches, selection: textView.selectedRange())
            updateFindHighlight()
            findBar?.setResult(count: findMatches.count, current: findIndex)
            if let index = findIndex, findMatches.indices.contains(index) {
                select(match: findMatches[index])
            }
        }

        private func findNext() {
            guard !findMatches.isEmpty else {
                findQueryChanged(findBar?.query ?? "")
                return
            }
            guard let index = findIndex else { return }
            let next = (index + 1) % findMatches.count
            findIndex = next
            findMatch(at: next)
        }

        private func findPrevious() {
            guard !findMatches.isEmpty else {
                findQueryChanged(findBar?.query ?? "")
                return
            }
            guard let index = findIndex else { return }
            let previous = (index - 1 + findMatches.count) % findMatches.count
            findIndex = previous
            findMatch(at: previous)
        }

        private func findMatch(at index: Int) {
            updateFindHighlight()
            findBar?.setResult(count: findMatches.count, current: index)
            select(match: findMatches[index])
        }

        private func select(match: NSRange) {
            guard let textView else { return }
            textView.setSelectedRange(match)
            textView.scrollRangeToVisible(match)
        }

        /// Highlights are **temporary** attributes: they live on the layout manager, never on the
        /// text storage, so a search cannot change a byte of the query and closing the bar cannot
        /// leave a mark behind in the undo stack.
        private func updateFindHighlight() {
            guard let textView, let layoutManager = textView.layoutManager else { return }
            clearFindHighlight()
            guard findVisible, !findMatches.isEmpty else { return }
            let length = (textView.string as NSString).length
            for (index, match) in findMatches.enumerated() where NSMaxRange(match) <= length {
                layoutManager.addTemporaryAttribute(
                    .backgroundColor,
                    value: Tone.inkNS(index == findIndex ? 0.30 : 0.12),
                    forCharacterRange: match)
            }
        }

        private func clearFindHighlight() {
            guard let textView, let layoutManager = textView.layoutManager else { return }
            let whole = NSRange(location: 0, length: (textView.string as NSString).length)
            layoutManager.removeTemporaryAttribute(.backgroundColor, forCharacterRange: whole)
        }

        private func replaceCurrent() {
            guard let textView, let storage = textView.textStorage else { return }
            guard let index = findIndex, findMatches.indices.contains(index) else {
                findNext()
                return
            }
            let match = findMatches[index]
            let replacement = findBar?.replacement ?? ""
            guard textView.shouldChangeText(in: match, replacementString: replacement) else { return }
            storage.replaceCharacters(in: match, with: replacement)
            textView.didChangeText()
            // Leave the caret after the replacement, which is where the next Replace would start.
            let caret = match.location + (replacement as NSString).length
            textView.setSelectedRange(NSRange(location: caret, length: 0))
            findQueryChanged(findBar?.query ?? "")
        }

        private func replaceAllMatches() {
            guard let textView, let storage = textView.textStorage else { return }
            let query = findBar?.query ?? ""
            guard !query.isEmpty else { return }
            let text = textView.string as NSString
            let result = FindReplace.replaceAll(in: text, needle: query,
                                                with: findBar?.replacement ?? "",
                                                options: findBar?.options ?? [])
            guard result.count > 0 else {
                findBar?.setResult(count: 0, current: nil)
                return
            }
            let whole = NSRange(location: 0, length: text.length)
            guard textView.shouldChangeText(in: whole, replacementString: result.text) else { return }
            storage.replaceCharacters(in: whole, with: result.text)
            textView.didChangeText()
            findQueryChanged(query)
        }

        // MARK: Folding

        /// The regions the user has folded right now.
        private var folded: [FoldRegion] {
            foldRegions.filter { foldedHeaders.contains($0.header) }
        }

        private func refreshFolds(previous: String) {
            guard let textView else { return }
            let text = textView.string
            // Both the caret's band and the gutter's run markers read these, so they are computed
            // when either wants them, and never on a caret move.
            let wantsStatements = parent.layout.highlightCurrentStatement
                || parent.layout.runButtonPerStatement
            statementBounds = wantsStatements ? SQLFolding.statementRanges(in: text) : []
            defer { updateRunMarks() }
            // Folding off means no regions and no markers. The offsets are dropped with them: a
            // fold that was collapsed when the switch went off would otherwise come back somewhere
            // else when it went on again, because the text may have changed in between.
            guard parent.layout.codeFolding else {
                foldRegions = []
                foldedHeaders = []
                updateFoldMarks()
                return
            }
            foldedHeaders = SQLFolding.shift(foldedHeaders,
                                             from: previous as NSString, to: text as NSString)
            foldRegions = SQLFolding.regions(in: text)
            // Only a header that is still a foldable region's header keeps its fold: a region that
            // stopped being multi-line — its body deleted, say — must not stay collapsed.
            foldedHeaders.formIntersection(Set(foldRegions.map(\.header)))
            unfoldRegions(containing: textView.selectedRange().location)
        }

        private func unfoldRegions(containing offset: Int) {
            let doomed = foldRegions.filter { region in
                region.body.length > 0 && NSLocationInRange(offset, region.body)
            }.map(\.header)
            guard !doomed.isEmpty else { return }
            foldedHeaders.subtract(doomed)
        }

        func toggleFold(headerOffset: Int) {
            guard let textView,
                  let region = foldRegions.first(where: { $0.header == headerOffset }) else { return }
            if foldedHeaders.contains(headerOffset) {
                foldedHeaders.remove(headerOffset)
            } else {
                foldedHeaders.insert(headerOffset)
                // Put the caret on the visible header rather than leaving it inside a body that is
                // about to disappear.
                textView.setSelectedRange(NSRange(location: region.header, length: 0))
            }
            colour(textView)
        }

        private func foldAtCaret() {
            guard let textView else { return }
            let caret = textView.selectedRange().location
            let candidates = foldRegions.filter { caret >= $0.header && caret < $0.bodyEnd }
            // Innermost: when a statement fold and a CTE fold overlap, the one whose header is
            // nearest the caret wins, so folding inside a `WITH` folds the part being looked at.
            guard let region = candidates.max(by: { $0.header < $1.header }) else { return }
            foldedHeaders.insert(region.header)
            colour(textView)
        }

        private func unfoldAtCaret() {
            guard let textView else { return }
            let caret = textView.selectedRange().location
            let candidates = folded.filter { caret >= $0.header && caret < $0.bodyEnd }
            guard let region = candidates.max(by: { $0.header < $1.header }) else { return }
            foldedHeaders.remove(region.header)
            colour(textView)
        }

        private func updateFoldMarks() {
            guard let ruler else { return }
            ruler.foldMarks = foldRegions.map { region in
                LineNumberRulerView.FoldMark(headerLine: region.headerLine,
                                             headerOffset: region.header,
                                             folded: foldedHeaders.contains(region.header),
                                             summary: region.summary)
            }
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
///
/// It also owns the find bar's place: the bar is a subview pinned to the top and the text view is
/// laid out below it, so opening Find pushes the query down instead of covering the first lines.
/// That is why this override exists rather than the bar being added as a SwiftUI overlay — an
/// overlay cannot make room for itself.
final class FlippedContainerView: NSView {
    weak var findBar: NSView?
    /// Set by the coordinator. Flipping it re-lays the text view out, so the bar makes room.
    var findBarVisible = false {
        didSet { needsLayout = true }
    }

    override var isFlipped: Bool { true }

    override func layout() {
        super.layout()
        guard let findBar else { return }
        findBar.isHidden = !findBarVisible
        // `fittingSize` rather than a constant: the bar grows a second row when Replace is shown,
        // and a fixed height would clip it.
        let barHeight = findBarVisible ? findBar.fittingSize.height : 0
        findBar.frame = NSRect(x: 0, y: 0, width: bounds.width, height: barHeight)
        for case let scroll as NSScrollView in subviews {
            scroll.frame = NSRect(x: 0, y: barHeight, width: bounds.width,
                                  height: max(0, bounds.height - barHeight))
        }
    }
}

/// The keys the editor takes before AppKit sees them.
///
/// Modifiers are checked one by one rather than as a set, because `modifierFlags` also carries
/// caps-lock, function and numeric-pad bits that have nothing to do with what was pressed.
enum EditorShortcut {
    case find, replace, findNext, findPrevious, fold, unfold, dismiss

    init?(event: NSEvent) {
        let flags = event.modifierFlags
        let command = flags.contains(.command)
        let option = flags.contains(.option)
        let shift = flags.contains(.shift)
        let control = flags.contains(.control)
        let key = event.charactersIgnoringModifiers?.lowercased()

        if command, !option, !control, key == "f" { self = .find; return }
        if command, option, !control, key == "f" { self = .replace; return }
        if command, !option, !control, !shift, key == "g" { self = .findNext; return }
        if command, !option, !control, shift, key == "g" { self = .findPrevious; return }
        if command, option, !control, !shift, key == "[" { self = .fold; return }
        if command, option, !control, !shift, key == "]" { self = .unfold; return }
        if event.keyCode == 53, !command, !option { self = .dismiss; return }
        return nil
    }
}

/// The AppKit half of code folding: it applies and removes the collapsed-line attributes.
///
/// Folding is an **attribute** change, never a character change. The text the editor holds — and
/// the model behind the binding — is exactly what the user typed, which is what makes editing
/// around a fold safe and what this lane's brief asks to keep honest. The body is hidden by giving
/// each hidden line a near-zero paragraph height and a clear foreground; `SQLSyntax.apply` resets
/// every attribute on each pass, so a fold can never outlive the text it was made for.
enum SQLFoldStyler {
    /// The height a folded line is squashed to.
    ///
    /// Not literally zero: a paragraph style whose maximum line height is 0 is read as "no maximum"
    /// and keeps full height — measured, the first prototype saved 0pt — so the smallest positive
    /// value is used instead. At this height the glyphs overlap the line below, which is why the
    /// foreground is cleared too.
    static let collapsedLineHeight: CGFloat = 0.1

    static func apply(_ regions: [FoldRegion], to textView: NSTextView) {
        guard let storage = textView.textStorage else { return }
        let whole = NSRange(location: 0, length: storage.length)
        let collapsed = NSMutableParagraphStyle()
        collapsed.minimumLineHeight = collapsedLineHeight
        collapsed.maximumLineHeight = collapsedLineHeight

        storage.beginEditing()
        // Detached first, so a region that is no longer folded gets its height back even when this
        // runs without a syntax pass in front of it.
        storage.addAttribute(.paragraphStyle, value: NSParagraphStyle.default, range: whole)
        for region in regions {
            let body = region.body
            guard body.length > 0, NSMaxRange(body) <= whole.length else { continue }
            storage.addAttribute(.paragraphStyle, value: collapsed, range: body)
            storage.addAttribute(.foregroundColor, value: NSColor.clear, range: body)
        }
        storage.endEditing()
        // Explicit, because hiding has to actually happen: an attribute edit usually invalidates
        // layout on its own, but if it ever did not, the fold would be remembered and invisible.
        textView.layoutManager?.invalidateLayout(forCharacterRange: whole, actualCharacterRange: nil)
    }
}

/// `NSTextView` that lets the coordinator see keys before AppKit does, and that refuses to show
/// AppKit's own completion list.
final class SQLTextView: NSTextView {
    var interceptKey: ((NSEvent) -> Bool)?

    /// The bands drawn behind the text, painted in order: the caret's statement first, its line
    /// over it. Behind the text rather than over it, which is what keeps the selection, the syntax
    /// colours and the caret readable through them.
    var highlightRanges: [NSRange] = [] {
        didSet {
            if highlightRanges != oldValue { needsDisplay = true }
        }
    }

    /// The colour those bands are painted in. From the palette rather than a constant, so a light
    /// canvas does not get a light band on it.
    var highlightColour: NSColor = .clear

    override func keyDown(with event: NSEvent) {
        if let interceptKey, interceptKey(event) { return }
        super.keyDown(with: event)
    }

    override func drawBackground(in rect: NSRect) {
        super.drawBackground(in: rect)
        guard !highlightRanges.isEmpty, let layoutManager, let textContainer else { return }
        let length = (string as NSString).length
        highlightColour.setFill()
        for range in highlightRanges {
            let clamped = NSIntersectionRange(range, NSRange(location: 0, length: length))
            guard clamped.length > 0 else { continue }
            let glyphs = layoutManager.glyphRange(forCharacterRange: clamped,
                                                  actualCharacterRange: nil)
            layoutManager.enumerateEnclosingRects(
                forGlyphRange: glyphs,
                withinSelectedGlyphRange: NSRange(location: NSNotFound, length: 0),
                in: textContainer
            ) { band, _ in
                // Full width rather than the glyphs' own, so the band reads as a line of the editor
                // and not as a highlight of the text that happens to be on it.
                let line = NSRect(x: 0, y: band.minY,
                                  width: max(self.bounds.width, band.maxX), height: band.height)
                NSBezierPath(rect: line).fill()
            }
        }
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

    /// One foldable header the gutter can draw a marker for. The coordinator supplies these; the
    /// ruler only decides whether the header is visible.
    struct FoldMark: Equatable {
        /// 0-based line the marker sits on.
        let headerLine: Int
        /// UTF-16 offset of that line's first character, handed back when the marker is clicked.
        let headerOffset: Int
        let folded: Bool
        /// The leading keyword or `CTE`, for the marker's tooltip.
        let summary: String
    }

    /// Markers for the foldable lines. Empty by default, so the gutter draws exactly what it drew
    /// before folding existed when nothing is foldable.
    var foldMarks: [FoldMark] = [] {
        didSet { needsDisplay = true }
    }

    /// Called with a mark's `headerOffset` when its marker is clicked.
    var onToggleFold: ((Int) -> Void)?

    /// One run marker: the first line of a statement, and the offset a click hands back.
    struct RunMark: Equatable {
        /// 0-based line the marker sits on.
        let headerLine: Int
        /// UTF-16 offset of the statement's first character.
        let headerOffset: Int
    }

    /// Markers for the statements that can be run. Empty when the switch is off.
    var runMarks: [RunMark] = [] {
        didSet { needsDisplay = true }
    }

    /// Whether the run column is reserved at all, which is what decides the gutter's width.
    var showsRunMarks = false {
        didSet {
            guard showsRunMarks != oldValue else { return }
            updateWidth()
        }
    }

    /// Called with a mark's `headerOffset` when its run marker is clicked.
    var onRun: ((Int) -> Void)?

    init(textView: NSTextView) {
        self.textView = textView
        super.init(scrollView: textView.enclosingScrollView, orientation: .verticalRuler)
        clientView = textView
        ruleThickness = Self.gutterWidth(forLines: 1)
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
        updateWidth()
        needsDisplay = true
    }

    /// The width the current line count and the run column ask for.
    private func updateWidth() {
        let wanted = Self.gutterWidth(forLines: lineCount, showsRunMarks: showsRunMarks)
        if abs(wanted - ruleThickness) > 0.5 { ruleThickness = wanted }
    }

    /// The text's own inset inside the text view. Named because the placeholder overlay in
    /// `EditorPane` has to start at the same x the text does, or it draws under the gutter.
    static let textInset = NSSize(width: 8, height: 9)

    /// Where the text starts, measured from the editor box's left edge: the gutter, then the text
    /// view's own inset.
    ///
    /// An overlay that wants to sit exactly where the text sits has to add both, and this is that
    /// sum in one place. Guessing it produced a placeholder drawn *under* the line numbers.
    static func textOriginX(forLines lines: Int, showsRunMarks: Bool = false) -> CGFloat {
        gutterWidth(forLines: lines, showsRunMarks: showsRunMarks) + textInset.width
    }

    /// Wide enough for the number it will have to show, so the gutter does not jump sideways when
    /// the hundredth line arrives — and no wider, because every point here is a point the text
    /// does not get.
    ///
    /// Internal rather than private so the placeholder can be told where the text starts.
    static func gutterWidth(forLines lines: Int, showsRunMarks: Bool = false) -> CGFloat {
        let digits = CGFloat(max(2, String(max(lines, 1)).count))
        return digits * 7.5 + 20 + (showsRunMarks ? runColumn : 0)
    }

    /// The extra width the run column takes, and where each marker sits inside the gutter.
    ///
    /// Two columns rather than one because a statement's first line is often also a fold header —
    /// `WITH x AS (` is both — and a marker that hid the other would take away a control the user
    /// can see.
    static let runColumn: CGFloat = 14
    private static let runMarkerX: CGFloat = 5
    private static let foldMarkerX: CGFloat = 5
    private static let foldMarkerShiftedX: CGFloat = 19

    /// The gutter draws **no background of its own**.
    ///
    /// `NSRulerView`'s default fill is a system control grey. On a themed window that is a strip of
    /// somebody else's palette down the side of the editor: measured on the midnight theme, the
    /// gutter band sat at luminance 25 while the editor's own surface was 12 and the text area 7 —
    /// the one part of the editor that did not follow the theme. Leaving it unfilled lets the
    /// editor box's surface show through, which is the same fill that sits behind the text.
    ///
    /// Overridden here rather than painted over `super`: `super.draw` would fill its grey first and
    /// then the marks on top of it, so the call to draw the marks has to be the whole body.
    override func draw(_ dirtyRect: NSRect) {
        // A touch brighter than the surface behind it, so the gutter reads as its own strip without
        // becoming a strip of somebody else's palette — which is what the default fill was.
        //
        // White at 3.5% *over* whatever is behind, rather than an absolute colour: the result is the
        // editor's own themed surface lifted a step, so a theme change moves it too. A fixed grey
        // was the bug this replaces.
        //
        // Only one direction can brighten, and that is white. On a light theme the surface is
        // already near-white, so the step is necessarily tiny there (measured: 248.1 -> 248.1,
        // which is to say invisible); on a dark theme it is plain (7.9 -> 15.3). If the strip needs
        // to be visible on light themes as well, the direction has to flip there — say the word and
        // it becomes a recess instead of a lift.
        NSColor.white.withAlphaComponent(0.035).setFill()
        bounds.fill()

        drawHashMarksAndLabels(in: dirtyRect)

        // And one hairline down its trailing edge, because with only a 3.5% lift the gutter would
        // otherwise run into the text with no seam at all. The default ruler draws a separator as
        // part of its background, so replacing that background took the separator with it.
        //
        // `Tone.ink` at 7%, the same hairline the panes use between each other, so the seam belongs
        // to the theme rather than to the system.
        Tone.inkNS(0.07).setFill()
        NSRect(x: bounds.maxX - 1, y: bounds.minY, width: 1, height: bounds.height).fill()
    }

    override func drawHashMarksAndLabels(in rect: NSRect) {
        guard let textView, let scrollView else { return }
        // The text view's origin in the ruler's own coordinates. `convert` accounts for one being
        // flipped and the other not, which is the whole reason this is not arithmetic on offsets.
        let origin = convert(NSPoint.zero, from: textView)
        let inset = textView.textContainerInset

        let attributes: [NSAttributedString.Key: Any] = [
            .font: numberFont,
            // The app's own readout grey. `secondaryLabelColor` is the system's, which is a different
            // colour from `Tone.secondary` on every theme — and it resolved per appearance, so a
            // theme switch left the gutter in the system's grey while everything around it moved.
            .foregroundColor: Tone.readoutNS,
        ]

        for entry in numberedLines(in: scrollView.contentView.bounds) {
            let midY = origin.y + inset.height + entry.minY + entry.height / 2
            let line = entry.number - 1
            let hasRun = runMarks.contains { $0.headerLine == line }
            // The run marker, at the gutter's leading edge. The fold marker moves over when both are
            // on this line, because a statement's first line is often also a fold header.
            if hasRun { drawRunMarker(midY: midY) }
            if let mark = foldMarks.first(where: { $0.headerLine == line }) {
                // Always in the fold column when the run column is reserved, whether or not this
                // line has a run marker: a marker drawn inside the run column would be read as a
                // click on the other control and could not be hit at all.
                drawFoldMarker(mark, midY: midY,
                               x: showsRunMarks ? Self.foldMarkerShiftedX : Self.foldMarkerX)
            }

            let label = "\(entry.number)" as NSString
            let size = label.size(withAttributes: attributes)
            label.draw(at: NSPoint(x: ruleThickness - size.width - 8,
                                   y: origin.y + inset.height + entry.minY
                                      + (entry.height - size.height) / 2),
                       withAttributes: attributes)
        }
    }

    /// A small right-pointing triangle: the mark that this statement can be run from here.
    private func drawRunMarker(midY: CGFloat) {
        let path = NSBezierPath()
        let x = Self.runMarkerX
        path.move(to: NSPoint(x: x, y: midY - 4.5))
        path.line(to: NSPoint(x: x, y: midY + 4.5))
        path.line(to: NSPoint(x: x + 6.5, y: midY))
        path.close()
        NSColor(Tone.accent).setFill()
        path.fill()
    }

    /// A small triangle: pointing right when the region is folded, down when it is open.
    private func drawFoldMarker(_ mark: FoldMark, midY: CGFloat, x: CGFloat) {
        let size: CGFloat = 4.5
        let path = NSBezierPath()
        if mark.folded {
            path.move(to: NSPoint(x: x, y: midY - size))
            path.line(to: NSPoint(x: x, y: midY + size))
            path.line(to: NSPoint(x: x + size * 1.5, y: midY))
        } else {
            path.move(to: NSPoint(x: x, y: midY + size))
            path.line(to: NSPoint(x: x + size * 2, y: midY + size))
            path.line(to: NSPoint(x: x + size, y: midY - size))
        }
        path.close()
        (mark.folded ? Tone.inkNS(0.80) : Tone.readoutNS).setFill()
        path.fill()
    }

    /// Clicking a marker runs the statement, or folds or unfolds its region.
    ///
    /// A ruler has no notion of rows, so the click is mapped through the layout manager to a
    /// character and then to a line. The run column is the leading strip and the fold column is
    /// everything after it; a click anywhere on a line's own column counts, because the markers are
    /// small and a user aiming at one should not have to hit it exactly.
    override func mouseDown(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        if let offset = runHeader(at: point) {
            onRun?(offset)
            return
        }
        if let offset = foldHeader(at: point) {
            onToggleFold?(offset)
            return
        }
        super.mouseDown(with: event)
    }

    /// The statement a click in the run column names, or nil.
    private func runHeader(at point: NSPoint) -> Int? {
        guard showsRunMarks, point.x < Self.runColumn, let line = line(at: point) else { return nil }
        return runMarks.first { $0.headerLine == line }?.headerOffset
    }

    private func foldHeader(at point: NSPoint) -> Int? {
        // With the run column reserved, the leading strip belongs to the run marker.
        if showsRunMarks, point.x < Self.runColumn { return nil }
        guard let line = line(at: point) else { return nil }
        return foldMarks.first { $0.headerLine == line }?.headerOffset
    }

    /// The 0-based line under a point in the ruler.
    private func line(at point: NSPoint) -> Int? {
        guard let textView,
              let layoutManager = textView.layoutManager,
              let container = textView.textContainer,
              (textView.string as NSString).length > 0
        else { return nil }
        let origin = convert(NSPoint.zero, from: textView)
        let containerPoint = NSPoint(x: 0, y: point.y - origin.y - textView.textContainerInset.height)
        let string = textView.string as NSString
        let index = min(max(0, layoutManager.characterIndex(for: containerPoint, in: container,
                                                            fractionOfDistanceBetweenInsertionPoints: nil)),
                         string.length - 1)
        return SQLFolding.line(containing: index, lineStarts: SQLFolding.lineStarts(in: string))
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
