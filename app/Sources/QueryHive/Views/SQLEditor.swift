import AppKit
import Observation
import QueryHiveFFI
import SwiftUI

/// What the editor already knows and the pane around it needs, handed to SwiftUI: the line count,
/// how many issues the text has, and whether the find bar is open.
///
/// The corner readout used to split the whole query into lines on every body evaluation, which is a
/// pass over the text per model change. The ruler keeps a line index for its own drawing, so the
/// readout reads that instead; the coordinator writes here when the count moves.
@Observable
final class EditorLineCount {
    /// What the editor's gutter reports; nil until it has.
    var value: Int?
    /// The underlined issues (W10-T6b), for the readout's "· N issues".
    var issues = EditorIssueSummary()
    /// Whether the find bar is showing. The pane hides its Clear button then: the button sits where
    /// the bar's close button is (B-1).
    var findOpen = false
    /// The text the pane was built for. Until the editor reports, the count comes from this, once per
    /// read — and holding it costs a reference, where counting in `init` cost a pass per re-render.
    let seed: String

    init(text: String = "") { seed = text }

    var count: Int { value ?? (1 + seed.utf8.reduce(0) { $1 == 0x0A ? $0 + 1 : $0 }) }
}

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
    /// Where the editor reports how many lines the text has. Optional, so a host that does not show
    /// the count does not have to make one.
    var lineCount: EditorLineCount? = nil
    /// The dialect the tab's connection speaks. The analysis is built under it and rebuilt when it
    /// changes, so statement boundaries and quoting follow the connection (blueprint w10 §8.6).
    var dialect: EditorDialect = .generic
    /// Where the server said the last Run went wrong; underlined while the text is the one that Run
    /// sent. The editor drops it on the first edit, ahead of the model.
    var errorMark: ServerErrorMark? = nil

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    /// Write whatever the editors have typed but not yet handed to the model.
    ///
    /// The model is written at most every 150 ms while typing, so a Run, a Save or a session save
    /// that reads `tab.sql` in between would read text that is a keystroke or two old. The editor
    /// already flushes on any click, any command-key and any menu; a caller that reads the text
    /// without a user event in front of it — a scripted run, a timer — calls this first.
    @MainActor
    static func flushPendingEdits() {
        for coordinator in Coordinator.live.allObjects { coordinator.flushToModel() }
    }

    func makeNSView(context: Context) -> NSView {
        let container = FlippedContainerView()
        container.autoresizesSubviews = true

        // TextKit 1, chosen here rather than reached by accident. `NSTextView()` builds a TextKit 2
        // view and drops to TextKit 1 the first time anything reads `layoutManager`, which the editor
        // does in a dozen places; the mode then depended on which of them ran first. Building the
        // stack by hand makes it TextKit 1 from creation, and non-contiguous layout is what lets a
        // large document lay out only what is on screen instead of everything above the edit.
        let storage = NSTextStorage()
        let layoutManager = NSLayoutManager()
        layoutManager.allowsNonContiguousLayout = true
        storage.addLayoutManager(layoutManager)
        let textContainer = NSTextContainer(size: NSSize(width: 0, height: CGFloat.greatestFiniteMagnitude))
        layoutManager.addTextContainer(textContainer)
        let textView = SQLTextView(frame: .zero, textContainer: textContainer)
        PerfSignposts.watchTextKit(textView)
        textView.isRichText = false
        textView.isEditable = true
        textView.isSelectable = true
        textView.allowsUndo = true
        textView.font = SQLSyntax.font(italic: false)
        // Adaptive, not pinned white: this NSTextView draws on `Tone.canvas`, which is near-black
        // in a dark appearance and off-white in a light one. AppKit resolves both of these per
        // appearance, so the caret and the default text colour follow the canvas with no observer.
        // The text colour is the one `baseAttributes` keeps in the storage.
        textView.textColor = SQLSyntax.baseColour
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
        textView.isAutomaticLinkDetectionEnabled = false
        textView.isAutomaticDataDetectionEnabled = false
        textView.isAutomaticTextCompletionEnabled = false
        textView.inlinePredictionType = .no
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
        // The coordinator hears every character edit from the storage itself: that is what keeps the
        // gutter's line index, the cached statement ranges and the folds in step with the text one
        // edit at a time, instead of each being recomputed from the whole string.
        storage.delegate = context.coordinator
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
        let findBar = FlippedContainerView.makeFindBar()
        container.addSubview(findBar)
        container.findBar = findBar
        context.coordinator.findBar = findBar
        context.coordinator.wire(findBar)

        context.coordinator.textView = textView
        context.coordinator.container = container
        context.coordinator.ruler = ruler
        context.coordinator.scrollView = scrollView
        context.coordinator.observe(scrollView)
        // The gutter is sized before the layout switches apply. Changing its width after the scroll
        // view has tiled shifts the content by the difference, which a scrolling (no-wrap) editor
        // shows as a horizontal offset it never had.
        ruler.rebuildIndex()
        // The layout switches, before anything is drawn: the gutter's visibility, the wrapping, the
        // tab stops and the invisible characters are all properties of the text view.
        context.coordinator.applyLayout()
        PerfSignposts.logTextKit(textView)
        ruler.onToggleFold = { [weak coordinator = context.coordinator] offset in
            coordinator?.toggleFold(headerOffset: offset)
        }
        ruler.onRun = { [weak coordinator = context.coordinator] offset in
            coordinator?.runStatement(at: offset)
        }
        ruler.refreshMarks = { [weak coordinator = context.coordinator] in
            coordinator?.refreshMarksIfStale()
        }
        textView.interceptKey = { [weak coordinator = context.coordinator] event in
            coordinator?.handle(event) ?? false
        }
        // A click in the text closes the suggestion list rather than leaving it pinned to the caret
        // it was built for. The list is deliberately not clickable, so a click is the one gesture
        // that can only mean "I am looking somewhere else now".
        textView.onClick = { [weak coordinator = context.coordinator] in
            coordinator?.dismissCompletionOnClick()
        }
        // Whatever the tab was holding when it opened — restored SQL, a loaded file, a table just
        // double-clicked — is coloured once here. `updateNSView` cannot do it: it returns early
        // when the string already matches, and re-running the scan on every SwiftUI update would
        // make typing pay for the model's changes.
        context.coordinator.recolour()
        return container
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        let coordinator = context.coordinator
        // The coordinator reads bindings and callbacks through `parent`, so it has to be refreshed
        // on every update or it keeps calling into a stale view value.
        coordinator.parent = self
        // Before the early return below: the gutter's font follows the code-font setting, and a
        // setting change does not alter the text, so it would otherwise never reach the ruler.
        coordinator.ruler?.numberFont = FontChoice.codeNSFont(size: 10.5, weight: .regular)
        // The same reason: a switch flipped in Settings changes nothing about the text, so the
        // editor has to be told to re-read the layout rather than waiting for an edit.
        coordinator.applyLayout()
        // Not `textView.string != text`: that compared two whole documents on every update. The
        // editor remembers the value the model holds, and the model's string is the very one it
        // wrote, so the common case is one identity check.
        coordinator.adoptModelText(text)
        // After the text, so the mark is compared with the text it is meant for.
        coordinator.adoptDialect()
        coordinator.adoptServerMark(errorMark)
    }

    static func dismantleNSView(_ nsView: NSView, coordinator: Coordinator) {
        coordinator.flushToModel()
    }

    // MARK: Coordinator

    @MainActor
    final class Coordinator: NSObject, NSTextViewDelegate, @preconcurrency NSTextStorageDelegate {
        /// Every live editor, so `SQLEditor.flushPendingEdits()` can reach them.
        static let live = NSHashTable<Coordinator>.weakObjects()

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
        /// The statement ranges of the current text, for the caret's own highlight, the gutter's run
        /// marks and the statement a keystroke repaints. Replaced by each analysis, and moved through
        /// every edit in between, so they are approximately right the moment after a keystroke and
        /// exactly right once the analysis lands.
        private var statementBounds: [NSRange] = []

        private var debounce: DispatchWorkItem?
        private var suppressAutoTrigger = false
        /// True while the uppercase pass is replacing text, so the `textDidChange` that replacement
        /// raises does not start another one.
        private var isUppercasing = false

        // MARK: Revisions and the model

        /// Bumped by every edit to the characters. Everything computed off the main thread carries
        /// the revision it was computed for and is dropped if the text has moved on.
        private var revision = 0
        /// The revision the model was last written for.
        private var publishedRevision = 0
        /// What the model holds, as far as this editor knows: the string it last wrote, or the one it
        /// last took from the model. `updateNSView` compares the model's text against this instead of
        /// against the whole document; it is the same string object, so the comparison is an identity
        /// check.
        private(set) var publishedText: String {
            didSet { publishedBlank = Self.isBlank(publishedText as NSString) }
        }
        /// Whether the model's text is empty or only whitespace. The placeholder, the Clear button and
        /// the menus that need a query all read the model, so that flip is written at once.
        private var publishedBlank: Bool

        /// Whitespace only, by the same definition `QueryTab.hasSQL` uses.
        private static func isBlank(_ text: NSString) -> Bool {
            var index = 0
            while index < text.length {
                let c = text.character(at: index)
                if c == 0x20 || c == 0x09 || c == 0x0A || c == 0x0D { index += 1; continue }
                if c < 0x80 { return false }
                guard let scalar = Unicode.Scalar(c), CharacterSet.whitespacesAndNewlines.contains(scalar) else { return false }
                index += 1
            }
            return true
        }
        private var syncItem: DispatchWorkItem?
        private static let syncInterval: TimeInterval = 0.15
        private static let syncCeiling: TimeInterval = 5
        private var unpublishedSince: CFAbsoluteTime?
        /// The caret moved while text was owed, so the caret is owed too: it goes to the model with
        /// the text, since a Run reads both.
        private var selectionOwed = false
        private let observers = ObserverBag()

        /// Set while the editor itself replaces the whole text, so that replacement is not tracked as
        /// an edit: it is followed by a full rebuild of everything the tracking would have kept.
        private var isReplacingText = false

        // MARK: Analysis

        /// The tree-sitter analysis of this document (Fase 4B). Rebuilt when text arrives from
        /// outside; keystrokes flow through `replace`, never through a rebuild.
        private var analysis: EditorAnalysis?
        /// The outline revision the gutter is wearing: statements, folds, run marks and rotor.
        private var outlineRevision: UInt64 = 0
        /// The paint revision last applied to the layout manager. Tests poll this.
        private(set) var appliedRevision: UInt64 = 0
        /// How many paints have reached `applyPaint`, stale ones included. A test that watches a
        /// quiet editor for this to stop moving is how a paint loop that never ends is noticed.
        private(set) var paintsAppliedForTesting = 0
        /// The revision keystrokes have reached; tests wait for `appliedRevision` to catch up.
        var pendingRevisionForTesting: UInt64 { analysis?.revision ?? appliedRevision }
        /// The edit's range, for the neighbour-colour inheritance in `textDidChange`.
        private var lastEditRange = NSRange(location: 0, length: 0)
        private var visibleScheduled = false
        private var idleItem: DispatchWorkItem?
        private static let idleDelay: TimeInterval = 0.5
        private static let idleQueue = DispatchQueue(label: "qh.editor.idle", qos: .utility)
        /// This size or smaller paints synchronously, so a tab opens coloured; anything bigger
        /// paints off the main thread, visible window first.
        private static let synchronousPaintLimit = 256_000
        /// The analysis window budget per turn, in UTF-16 units.
        private static let visibleBudget = 131_072

        // MARK: Folds

        /// The foldable regions the current text holds, from the last analysis and moved through the
        /// edits since.
        private var foldRegions: [FoldRegion] = []
        /// What the user has folded: the header's offset, and the body that is hidden. Offsets, not
        /// line numbers — a line number belongs to a text, and the text is the thing that keeps
        /// changing — and moved through every edit, so a fold stays on the lines it was made on.
        private var folded: [Int: NSRange] = [:]

        /// The statements as a rotor source (FR-ED-09): the list assistive navigation jumps
        /// through, rebuilt with every outline. "Query issues" is the second source.
        private(set) var rotor: StatementsRotorSource?
        /// "Query issues": the underlined diagnostics as rotor stops, rebuilt with every outline.
        private(set) var issuesRotor: IssuesRotorSource?

        // MARK: Diagnostics (W10-T6b)

        /// The lexical issues of the last outline. They describe the text as the outline saw it, so
        /// they are only merged and painted on an outline turn or while that outline is current.
        private var lexicalIssues: [EditorIssueData] = []
        /// What is underlined now: the lexical issues and the server's mark, merged.
        private(set) var diagnostics: [EditorDiagnostic] = []
        private let diagnosticsPainter = DiagnosticsPainter()
        /// The server mark being worn, by the model mark's id. It is dropped by the first edit.
        private var serverMark: (id: UUID, diagnostic: EditorDiagnostic)?
        /// A mark that must not come back: the one an edit dropped, and one that never matched the
        /// text. SwiftUI hands the same model mark on every update until the model drops it.
        private var dismissedMarkID: UUID?

        private var findVisible = false
        private var findMatches: [NSRange] = []
        private var findIndex: Int?

        init(_ parent: SQLEditor) {
            self.parent = parent
            self.publishedText = parent.text
            self.publishedBlank = Self.isBlank(parent.text as NSString)
            super.init()
            Self.live.add(self)
            AppModel.flushEditors = { SQLEditor.flushPendingEdits() }
            installFlushTriggers()
        }

        /// The document's characters as an `NSString`, without copying them. `textView.string` hands
        /// back a bridged `String`, and bridging a mutable string copies it — on a megabyte of SQL
        /// that is the whole cost of a keystroke. The storage's own string is live; it is only ever
        /// read here.
        private var nsText: NSString {
            (textView?.textStorage?.mutableString ?? NSMutableString()) as NSString
        }

        // MARK: Model sync

        var hasUnpublishedEdits: Bool { publishedRevision != revision }

        /// Write the model when typing pauses for 150 ms, and never more often than that. Every edit
        /// pushes the write back, so a burst of typing costs the model — and every SwiftUI view that
        /// reads the text — one update instead of one per key; on a large document that update is
        /// hundreds of milliseconds of string comparisons, which is the whole cost of a keystroke.
        /// A burst that never pauses is cut off after `syncCeiling`, so the model is never further
        /// behind than that even without a click or a shortcut to force it.
        private func scheduleSync() {
            let now = CFAbsoluteTimeGetCurrent()
            let owedSince = unpublishedSince ?? now
            unpublishedSince = owedSince
            syncItem?.cancel()
            let wait = min(Self.syncInterval, max(0, owedSince + Self.syncCeiling - now))
            let item = DispatchWorkItem { [weak self] in self?.flushToModel() }
            syncItem = item
            DispatchQueue.main.asyncAfter(deadline: .now() + wait, execute: item)
            // While text is owed, the next click or command-key writes it first: that is what puts
            // the model in step before a Run button, a Save shortcut or a tab switch reads it.
            guard observers.monitor == nil else { return }
            observers.monitor = NSEvent.addLocalMonitorForEvents(
                matching: [.keyDown, .leftMouseDown, .rightMouseDown, .otherMouseDown]
            ) { [weak self] event in
                MainActor.assumeIsolated {
                    let shortcut = event.type == .keyDown
                        && !event.modifierFlags.intersection([.command, .control]).isEmpty
                    if event.type != .keyDown || shortcut { self?.flushToModel() }
                }
                return event
            }
        }

        /// Hand the model what has been typed, now.
        func flushToModel() {
            syncItem?.cancel()
            syncItem = nil
            if let monitor = observers.monitor {
                NSEvent.removeMonitor(monitor)
                observers.monitor = nil
            }
            unpublishedSince = nil
            guard let textView, hasUnpublishedEdits || selectionOwed else { return }
            if hasUnpublishedEdits {
                let text = textView.string
                publishedRevision = revision
                publishedText = text
                parent.text = text
            }
            if selectionOwed {
                selectionOwed = false
                publishSelection(textView)
            }
        }

        private func publishSelection(_ textView: NSTextView) {
            parent.caret = textView.selectedRange().location
            parent.selection = textView.selectedRange()
        }

        private func installFlushTriggers() {
            let center = NotificationCenter.default
            let names: [Notification.Name] = [
                NSMenu.didBeginTrackingNotification,
                NSApplication.willResignActiveNotification,
                NSApplication.willTerminateNotification,
                NSWindow.willCloseNotification,
            ]
            for name in names {
                observers.tokens.append(center.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                    MainActor.assumeIsolated { self?.flushToModel() }
                })
            }
        }

        /// The model's text changed, or did not. Called on every SwiftUI update.
        func adoptModelText(_ text: String) {
            // The common case, and the cheap one: the model holds what this editor last wrote. Edits
            // the editor has not written yet make it ahead of the model, not behind it.
            guard text != publishedText, let textView else { return }
            // Who wins when the model changes underneath text the editor has not written yet: the local
            // typing. An outside write that lands in the 150 ms after a keystroke is the rarer thing and
            // the one that can be redone; the typing cannot, and writing it back from here would be a
            // model write during a view update. The pending flush then overwrites the outside text.
            // A load into an editor the user has not touched since the last write is applied.
            guard !hasUnpublishedEdits else { return }
            syncItem?.cancel()
            syncItem = nil
            let previous = textView.string
            let caret = textView.selectedRange().location
            // Folds are keyed by header offset, so a change from outside has to say how far the text
            // moved. The diff walks both strings, which is fine here and would not be per keystroke.
            let shifted = folded.isEmpty ? [:] : foldedThrough(from: previous, to: text)
            isReplacingText = true
            textView.string = text
            isReplacingText = false
            publishedText = text
            publishedRevision = revision
            let length = (text as NSString).length
            // Text appended from outside the editor — double-clicking a table in the tree — should
            // leave the caret at the end of what was just written, not back where it used to be.
            let appended = length > (previous as NSString).length && text.hasPrefix(previous)
            textView.setSelectedRange(NSRange(location: appended ? length : min(caret, length), length: 0))
            folded = shifted
            // Text that arrived from outside the editor — opening a table, loading a file — has never
            // been through `textDidChange` and would otherwise stay uncoloured.
            recolour()
        }

        private func foldedThrough(from old: String, to new: String) -> [Int: NSRange] {
            let headers = SQLFolding.shift(Set(folded.keys), from: old as NSString, to: new as NSString)
            // Only the header offsets survive the diff; the bodies come back with the next analysis,
            // which `recolour` runs straight away.
            return Dictionary(uniqueKeysWithValues: headers.map { ($0, NSRange(location: $0, length: 0)) })
        }

        // MARK: Storage edits

        /// Every edit to the characters passes through here, once, before anything reads the text.
        func textStorage(_ storage: NSTextStorage, didProcessEditing editedMask: NSTextStorageEditActions,
                         range editedRange: NSRange, changeInLength delta: Int) {
            guard editedMask.contains(.editedCharacters) else { return }
            revision += 1
            guard !isReplacingText else { return }
            // The server's position described the text before this edit. The underline goes with
            // the next outline; the mark must not come back from the model in the meantime.
            if let mark = serverMark {
                dismissedMarkID = mark.id
                serverMark = nil
            }
            PerfSignposts.part(PerfSignposts.Part.replaceAndRuler) {
                let edit = TextEdit(location: editedRange.location,
                                    oldLength: editedRange.length - delta, newLength: editedRange.length)
                let linesBefore = ruler?.knownLineCount
                ruler?.textEdited(range: editedRange, delta: delta)
                edit.apply(to: &statementBounds)
                moveFolds(through: edit)
                lastEditRange = NSRange(location: editedRange.location, length: editedRange.length)
                forward(edit: edit, in: storage)
                publishLineCount()
                if let linesBefore, linesBefore != ruler?.lineCount {
                    // A line was added or removed: everything drawn against a line number below this
                    // point moved, and the analysis that would say so is a debounce away.
                    updateRunMarks()
                    updateFoldMarks()
                }
            }
        }

        private func moveFolds(through edit: TextEdit) {
            if !folded.isEmpty {
                var moved: [Int: NSRange] = [:]
                for (header, body) in folded {
                    if edit.oldEnd <= header {
                        moved[header + edit.delta] = NSRange(location: body.location + edit.delta, length: body.length)
                    } else if edit.location >= NSMaxRange(body) {
                        moved[header] = body
                    } else if edit.location >= header, edit.oldEnd <= body.location {
                        moved[header] = NSRange(location: body.location + edit.delta, length: body.length)
                    }
                    // Otherwise the edit landed in the body, or straddled it: that fold opens, the same
                    // way a fold whose offset sat inside a changed span always has.
                }
                folded = moved
                hideFolded(invalidating: false)
            }
            guard !foldRegions.isEmpty else { return }
            foldRegions = foldRegions.compactMap { region in
                var header = region.header, start = region.bodyStart, end = region.bodyEnd
                if edit.oldEnd <= header {
                    header += edit.delta; start += edit.delta; end += edit.delta
                } else if edit.location >= end {
                    // Untouched.
                } else if edit.location >= header, edit.oldEnd <= start {
                    start += edit.delta; end += edit.delta
                } else {
                    return nil
                }
                guard let ruler else { return region }
                return FoldRegion(kind: region.kind, headerLine: ruler.line(containing: header),
                                  lastLine: ruler.line(containing: max(start, end - 1)),
                                  header: header, bodyStart: start, bodyEnd: end, summary: region.summary)
            }
        }

        /// Hand the edit to the analysis, widened past surrogates; a failure rebuilds it.
        private func forward(edit: TextEdit, in storage: NSTextStorage) {
            guard let analysis else { return }
            let text = storage.mutableString as NSString
            do {
                let widened = EditorAnalysis.widened(
                    NSRange(location: edit.location, length: edit.newLength), in: text)
                let oldLength = max(0, widened.length - edit.delta)
                let replacement = widened.length > 0 ? text.substring(with: widened) : ""
                try PerfSignposts.part("ffiReplace") {
                    try analysis.replace(
                        range: NSRange(location: widened.location, length: oldLength), with: replacement)
                }
            } catch {
                rebuildAnalysis()
            }
        }

        // MARK: Layout switches

        /// Uppercase the keyword the keystroke just finished, and say whether it did.
        ///
        /// The edit goes through `shouldChangeText`/`didChangeText` rather than through the text
        /// storage alone, so it is one undoable step and the delegate is told — which is what makes
        /// the syntax pass repaint the word it changed.
        @discardableResult
        private func uppercaseFinishedKeyword(_ textView: NSTextView) -> Bool {
            let text = nsText
            let caret = textView.selectedRange().location
            guard !isUppercasing, Self.mightFinishKeyword(in: text, caret: caret),
                  let replacement = keywordReplacement(in: text, caret: caret)
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

        /// `KeywordCase.replacement` on the text from the caret's statement to the caret, not on the
        /// document. It scans what it is given to decide whether the word is code, and a statement
        /// starts in code, so what comes after the statement's start is all it can need; scanning the
        /// whole document for every finished keyword cost more than the keystroke did.
        private func keywordReplacement(in text: NSString, caret: Int) -> KeywordCase.Replacement? {
            var low = 0, high = statementBounds.count
            while low < high {
                let mid = (low + high) / 2
                if statementBounds[mid].location <= caret { low = mid + 1 } else { high = mid }
            }
            let start = low > 0 ? statementBounds[low - 1].location : 0
            guard start > 0, start < caret else { return KeywordCase.replacement(in: text, caret: caret) }
            let window = text.substring(with: NSRange(location: start, length: caret - start)) as NSString
            guard let found = KeywordCase.replacement(in: window, caret: caret - start) else { return nil }
            return KeywordCase.Replacement(range: NSRange(location: found.range.location + start,
                                                          length: found.range.length), text: found.text)
        }

        /// The cheap half of `KeywordCase.replacement`, which scans the whole document for "is this
        /// code" before it looks at the word. Almost every space typed follows a word that is not a
        /// keyword, so asking the word first spares the scan; it says yes only where
        /// `replacement` could, so the outcome is the same.
        private static func mightFinishKeyword(in text: NSString, caret: Int) -> Bool {
            guard caret > 1, caret <= text.length,
                  let typed = UnicodeScalar(text.character(at: caret - 1)),
                  KeywordCase.delimiters.contains(Character(typed)) else { return false }
            var start = caret - 1
            while start > 0, isKeywordCharacter(text.character(at: start - 1)) { start -= 1 }
            guard start < caret - 1 else { return false }
            let word = text.substring(with: NSRange(location: start, length: caret - 1 - start))
            return word != word.uppercased() && SQLSyntax.keywords.contains(word.lowercased())
        }

        private static func isKeywordCharacter(_ character: unichar) -> Bool {
            guard let scalar = UnicodeScalar(character) else { return false }
            return CharacterSet.alphanumerics.contains(scalar) || scalar == "_" || scalar == "$"
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
            // A size is the one switch that changes the font the tab stops are measured in, so it
            // goes first and the tab pass below re-applies the width and repaints the whole text.
            let resized = previous != nil && previous?.fontSize != layout.fontSize
            if resized {
                textView.font = SQLSyntax.font(italic: false)
                ruler?.needsDisplay = true
            }
            if resized || previous?.tabWidth != layout.tabWidth { applyTabWidth(layout) }
            // The first pass has no regions to refresh: `recolour` follows it and does the analysis.
            if let previous, previous.codeFolding != layout.codeFolding { runIdle() }
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
            ruler.runMarks = statementBounds.map { range in
                LineNumberRulerView.RunMark(headerLine: ruler.line(containing: range.location),
                                            headerOffset: range.location)
            }
        }

        /// Run the statement that starts at `offset`.
        ///
        /// The offset is put where a run reads it — the tab's own selection — and then the host is
        /// asked to run, so the gutter and the Run menu take the same path through the model.
        func runStatement(at offset: Int) {
            guard let textView else { return }
            let length = nsText.length
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
            let whole = NSRange(location: 0, length: nsText.length)
            textView.textStorage?.addAttribute(.paragraphStyle, value: style, range: whole)
            try? analysis?.markDirty(whole)
            requestVisible()
        }

        /// The bands behind the text: the caret's statement first, then its line over it.
        private func updateHighlight(_ textView: SQLTextView) {
            let layout = parent.layout
            guard layout.highlightCurrentLine || layout.highlightCurrentStatement else {
                textView.highlightRanges = []
                return
            }
            let text = nsText
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
            let started = PerfSignposts.didChangeBegin()
            defer { PerfSignposts.didChangeEnd(started: started) }
            guard let textView = notification.object as? NSTextView else { return }
            (textView as? SQLTextView)?.applyTurnOpen = false
            // Before anything reads the text: this rewrites the word just finished, and the fold,
            // colour and completion passes should see the text that will actually be sent.
            if parent.layout.autoUppercaseKeywords, uppercaseFinishedKeyword(textView) {
                // The replacement raised `textDidChange` again, and that pass has already done the
                // rest of this work. Doing it here as well would scan the document twice per word.
                return
            }
            scheduleSync()
            // The placeholder reads emptiness and the menus read blankness; either flip is written now.
            if Self.isBlank(nsText) != publishedBlank || (nsText.length == 0) != publishedText.isEmpty { flushToModel() }
            inheritNeighborColor()
            requestVisible()
            scheduleIdle()
            // The gutter changes with a line added or removed, which `textEdited` already redraws for,
            // and with wrapping, where a typed character can push a line onto another fragment.
            if parent.layout.wordWrap, editMovedTheLinesBelow() { ruler?.needsDisplay = true }
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

        /// The paragraph the last key edited, as `(start, height)`, for `editMovedTheLinesBelow`.
        private var editedParagraph: (start: Int, height: CGFloat)?
        /// Past these the gutter is redrawn without looking: laying out a paragraph to measure it is
        /// only cheap while the paragraph and the edit are short.
        private static let measuredParagraphLimit = 4_096
        private static let measuredEditLimit = 256

        /// Whether the edit that just happened can have moved the lines under it, which with wrapping
        /// is what the gutter has to be redrawn for (a line added or removed is `textEdited`'s: it
        /// redraws for that itself). Typing inside one paragraph moves what is below it only when the
        /// paragraph's height changes: a fragment wrapped or unwrapped, or a character from a taller
        /// fallback font. The gutter used to be redrawn on every key, and in a profile of the
        /// coloured scenario that redraw cost more than the text view's own.
        ///
        /// Yes without looking for anything that is not one short edit in one short paragraph, and for
        /// the first key in a paragraph, whose previous height is not known.
        private func editMovedTheLinesBelow() -> Bool {
            let previous = editedParagraph
            editedParagraph = nil
            guard let layoutManager = textView?.layoutManager else { return true }
            let text = nsText
            let edit = lastEditRange
            guard edit.length <= Self.measuredEditLimit, NSMaxRange(edit) <= text.length,
                  text.rangeOfCharacter(from: .newlines, options: [], range: edit).location == NSNotFound
            else { return true }
            let paragraph = text.paragraphRange(for: NSRange(location: edit.location, length: 0))
            guard paragraph.length > 0, paragraph.length <= Self.measuredParagraphLimit else { return true }
            let glyphs = layoutManager.glyphRange(forCharacterRange: paragraph, actualCharacterRange: nil)
            guard glyphs.length > 0 else { return true }
            let first = layoutManager.lineFragmentRect(forGlyphAt: glyphs.location, effectiveRange: nil)
            let last = layoutManager.lineFragmentRect(forGlyphAt: NSMaxRange(glyphs) - 1, effectiveRange: nil)
            let height = last.maxY - first.minY
            editedParagraph = (paragraph.location, height)
            guard let previous, previous.start == paragraph.location else { return true }
            return abs(previous.height - height) > 0.01
        }

        /// Everything derived from the text, rebuilt from scratch: for text that arrived from outside
        /// the editor, and for the first paint. Attributes only — the string is never touched, so
        /// this cannot loop back through `textDidChange`, and the binding keeps whatever the user
        /// typed.
        func recolour() {
            guard textView != nil else { return }
            ruler?.rebuildIndex()
            publishLineCount()
            statementBounds = []
            foldRegions = []
            rebuildAnalysis()
        }
        /// A new analysis for the text as it stands: a load, an outside replacement, or recovery
        /// from a failed edit. Storage is reset to base attributes — colours live on the layout
        /// manager now — and the visible window paints, synchronously when the document is small.
        private func rebuildAnalysis() {
            guard let textView, let storage = textView.textStorage,
                  let layoutManager = textView.layoutManager else { return }
            analysis = try? EditorAnalysis(text: textView.string, dialect: parent.dialect)
            outlineRevision = 0
            let whole = NSRange(location: 0, length: storage.length)
            storage.setAttributes(Self.baseAttributes(textView: textView), range: whole)
            layoutManager.removeTemporaryAttribute(.foregroundColor, forCharacterRange: whole)
            // A new analysis means a text the old issues and the old mark were not written for.
            lexicalIssues = []
            serverMark = nil
            diagnostics = []
            diagnosticsPainter.reset(layoutManager, length: storage.length)
            publishIssues()
            textView.typingAttributes = Self.baseAttributes(textView: textView)
            ruler?.needsDisplay = true
            if storage.length <= Self.synchronousPaintLimit, !textView.hasMarkedText() {
                syncVisiblePaint()
            } else {
                requestVisible()
            }
            scheduleIdle()
        }

        /// Storage carries base attributes only: the code font, the paragraph style and the colour of
        /// plain text. Token colours are temporary attributes on top of it, and comment italics
        /// arrive through the paint's fonts. The base colour has to be here: text with no
        /// `.foregroundColor` is drawn black, and the paint only ever colours tokens.
        private static func baseAttributes(textView: NSTextView) -> [NSAttributedString.Key: Any] {
            [.font: SQLSyntax.font(italic: false),
             .foregroundColor: SQLSyntax.baseColour,
             .paragraphStyle: textView.defaultParagraphStyle ?? NSParagraphStyle.default]
        }

        // MARK: Painting from the analysis

        /// Paint the visible window now, on this thread. Only for small documents: the analysis
        /// parses the visible statements synchronously, which costs milliseconds there.
        private func syncVisiblePaint() {
            guard let textView, let analysis = analysis, !textView.hasMarkedText() else { return }
            let paint = try? analysis.paint(window: visibleCharacters(textView),
                                            budget: Self.visibleBudget)
            guard let paint else { return }
            applyPaint(paint)
        }

        /// Paint the visible window off the main thread, coalesced: a burst of keystrokes
        /// paints once, and a scroll schedules at most one paint per frame.
        private func requestVisible() {
            guard analysis != nil, textView != nil, !visibleScheduled else { return }
            visibleScheduled = true
            DispatchQueue.main.async { [weak self] in
                guard let self, let textView = self.textView else {
                    self?.visibleScheduled = false
                    return
                }
                self.visibleScheduled = false
                guard let analysis = self.analysis else { return }
                let window = self.visibleCharacters(textView)
                analysis.requestPaint(window: window, budget: Self.visibleBudget) {
                    [weak self] result in
                    guard let paint = try? result.get() else { return }
                    self?.applyPaint(paint)
                }
            }
        }

        /// The characters on screen, and a screen either side of them: the margin is what keeps a
        /// scroll from showing one frame of stale colour before this catches up.
        private func visibleCharacters(_ textView: NSTextView) -> NSRange {
            let whole = NSRange(location: 0, length: nsText.length)
            guard let layoutManager = textView.layoutManager, let container = textView.textContainer else { return whole }
            let origin = textView.textContainerOrigin
            var rect = textView.visibleRect
            rect = rect.insetBy(dx: 0, dy: -rect.height).offsetBy(dx: -origin.x, dy: -origin.y)
            let glyphs = layoutManager.glyphRange(forBoundingRect: rect, in: container)
            return NSIntersectionRange(layoutManager.characterRange(forGlyphRange: glyphs, actualGlyphRange: nil), whole)
        }

        /// Lay a paint onto the layout manager. Ranges with nothing to change are left alone —
        /// each write invalidates its range, and redrawing one coloured line costs milliseconds.
        private func applyPaint(_ paint: EditorPaintData) {
            PerfSignposts.part("applyPaint") { applyPaintBody(paint) }
            paintsAppliedForTesting += 1
        }

        private func applyPaintBody(_ paint: EditorPaintData) {
            guard let textView, let storage = textView.textStorage,
                  let layoutManager = textView.layoutManager, let analysis = analysis,
                  analysis.revision == paint.revision, !textView.hasMarkedText(),
                  storage.length == paint.length else {
                // Lengths disagree: the analysis is not describing this text anymore. A stale
                // revision alone just returns — its own request is already on the way.
                if let length = textView?.textStorage?.length, analysis?.length != length {
                    rebuildAnalysis()
                }
                return
            }
            PerfSignposts.applyBegin()
            textView.applyTurnOpen = true
            let length = storage.length
            if paint.inactive {
                let window = NSIntersectionRange(paint.window, NSRange(location: 0, length: length))
                layoutManager.removeTemporaryAttribute(.foregroundColor, forCharacterRange: window)
                try? analysis.markApplied(paint)
                return
            }
            let whole = NSRange(location: 0, length: length)
            for range in paint.ranges {
                let clipped = NSIntersectionRange(range, whole)
                guard clipped.length > 0 else { continue }
                let runs = paint.runs.filter { NSIntersectionRange($0.range, clipped).length > 0 }
                guard rangeNeedsPaint(clipped, runs: runs, in: layoutManager, length: length) else { continue }
                PerfSignposts.part(PerfSignposts.Part.tempAttrRemove) {
                    layoutManager.removeTemporaryAttribute(.foregroundColor, forCharacterRange: clipped)
                }
                for run in runs {
                    let at = NSIntersectionRange(run.range, clipped)
                    if let color = Self.paintColors[run.colorClass], at.length > 0 {
                        PerfSignposts.part(PerfSignposts.Part.tempAttrAdd) {
                            layoutManager.addTemporaryAttribute(.foregroundColor, value: color, forCharacterRange: at)
                        }
                    }
                }
            }
            // Apply the paint's fonts to storage: comment italics live here, because a
            // temporary attribute cannot carry a font. Only where the font differs: a paint carries
            // a font entry for every edited range, and writing the font the text already has still
            // reaches the layout manager as an attribute edit, which lays the range out again and
            // redisplays the whole visible rect. That was most of what a keystroke cost.
            let fonts = paint.fonts.filter { NSMaxRange($0.range) <= length }
            if !fonts.isEmpty {
                let upright = SQLSyntax.font(italic: false)
                let italic = SQLSyntax.font(italic: true)
                let changes = fonts.flatMap { entry -> [(range: NSRange, font: NSFont)] in
                    let wanted = entry.italic ? italic : upright
                    return Self.ranges(of: storage, within: entry.range, notUsing: wanted).map { ($0, wanted) }
                }
                if !changes.isEmpty {
                    storage.beginEditing()
                    for change in changes { storage.addAttribute(.font, value: change.font, range: change.range) }
                    storage.endEditing()
                }
            }
            try? analysis.markApplied(paint)
            appliedRevision = paint.revision
            // The window's own leftovers go round again. What is dirty *outside* the window does
            // not: the next request would ask for the same window and get nothing back, so the
            // loop never ended while anything off screen was dirty, and every key then paid for
            // dozens of empty paints. A scroll paints the new window (`observe`) and the idle
            // pass paints the rest.
            if paint.moreInWindow { requestVisible() }
        }

        /// Whether `range` needs touching: a run whose colour differs, or a gap between runs
        /// that still carries a colour from a run that has since shrunk or gone.
        private func rangeNeedsPaint(_ range: NSRange, runs: [EditorPaintRun],
                                     in layoutManager: NSLayoutManager, length: Int) -> Bool {
            guard length > 0 else { return true }
            let whole = NSRange(location: 0, length: length)
            for run in runs {
                var effective = NSRange()
                let at = min(run.range.location, length - 1)
                let current = layoutManager.temporaryAttribute(.foregroundColor, atCharacterIndex: at,
                    longestEffectiveRange: &effective, in: whole) as? NSColor
                if current !== Self.paintColors[run.colorClass] { return true }
            }
            var cursor = range.location
            for run in runs.sorted(by: { $0.range.location < $1.range.location }) {
                if run.range.location > cursor, staleColor(at: cursor, in: layoutManager, length: length) { return true }
                cursor = max(cursor, NSMaxRange(run.range))
            }
            return cursor < NSMaxRange(range) && staleColor(at: cursor, in: layoutManager, length: length)
        }

        /// The runs of `range` whose font is not `font`.
        private static func ranges(of storage: NSTextStorage, within range: NSRange, notUsing font: NSFont) -> [NSRange] {
            var stale: [NSRange] = []
            storage.enumerateAttribute(.font, in: range, options: []) { value, run, _ in
                guard (value as? NSFont) != font else { return }
                if let last = stale.last, NSMaxRange(last) == run.location {
                    stale[stale.count - 1] = NSUnionRange(last, run)
                } else {
                    stale.append(run)
                }
            }
            return stale
        }

        /// The one colour instance per class: identity (`===`) is how a paint decides a run is
        /// already right, so every paint must use these and nothing else.
        static let paintColors: [EditorColorClass: NSColor] = [
            .comment: SQLSyntax.colour(for: .comment),
            .string: SQLSyntax.colour(for: .string),
            .quotedIdentifier: SQLSyntax.colour(for: .quotedIdentifier),
            .number: SQLSyntax.colour(for: .number),
            .keyword: SQLSyntax.colour(for: .keyword),
            .literal: SQLSyntax.colour(for: .literal),
            .function: SQLSyntax.colour(for: .function),
            .punctuation: SQLSyntax.colour(for: .punctuation),
            .parameter: SQLSyntax.colour(for: .parameter),
        ]

        /// A temporary colour back to its class, by identity. `literal` and `parameter` share a
        /// colour; neither inherits, so the overlap does not matter.
        private static func classOf(_ color: NSColor) -> EditorColorClass? {
            paintColors.first { $0.value === color }?.key
        }

        /// Whether a gap between runs still carries a colour: a run shrank or went, and its tail
        /// was left behind.
        private func staleColor(at index: Int, in layoutManager: NSLayoutManager, length: Int) -> Bool {
            guard index < length else { return false }
            return layoutManager.temporaryAttribute(.foregroundColor, atCharacterIndex: index,
                longestEffectiveRange: nil, in: NSRange(location: 0, length: length)) != nil
        }

        /// Tint what was just typed with the colour before it, so a keystroke inside a string, a
        /// comment or a word does not flash uncoloured for the turn the analysis takes. Anything
        /// wrong here lasts one analysis turn: the paint corrects it.
        private func inheritNeighborColor() {
            PerfSignposts.part(PerfSignposts.Part.inherit) { inheritNeighborColorBody() }
        }

        private func inheritNeighborColorBody() {
            guard let layoutManager = textView?.layoutManager else { return }
            let inserted = lastEditRange
            guard inserted.length > 0, inserted.location > 0 else { return }
            let text = nsText
            guard NSMaxRange(inserted) <= text.length else { return }
            var effective = NSRange()
            guard let color = layoutManager.temporaryAttribute(.foregroundColor,
                atCharacterIndex: inserted.location - 1, longestEffectiveRange: &effective,
                in: NSRange(location: 0, length: text.length)) as? NSColor,
                  let cls = Self.classOf(color) else { return }
            if cls == .string || cls == .comment {
                PerfSignposts.part(PerfSignposts.Part.tempAttrAdd) {
                    layoutManager.addTemporaryAttribute(.foregroundColor, value: color, forCharacterRange: inserted)
                }
            } else if let previous = UnicodeScalar(text.character(at: inserted.location - 1)),
                      !CharacterSet.whitespacesAndNewlines.contains(previous),
                      text.substring(with: inserted).rangeOfCharacter(from: .whitespacesAndNewlines) == nil {
                PerfSignposts.part(PerfSignposts.Part.tempAttrAdd) {
                    layoutManager.addTemporaryAttribute(.foregroundColor, value: color, forCharacterRange: inserted)
                }
            }
        }

        func observe(_ scrollView: NSScrollView) {
            scrollView.contentView.postsBoundsChangedNotifications = true
            observers.tokens.append(NotificationCenter.default.addObserver(
                forName: NSView.boundsDidChangeNotification, object: scrollView.contentView, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.requestVisible() }
            })
        }

        // MARK: Analysis
        /// The slow-moving parts — statements, folds, run marks, rotor — after a pause in typing.
        private func scheduleIdle() {
            idleItem?.cancel()
            let item = DispatchWorkItem { [weak self] in self?.runIdle() }
            idleItem = item
            DispatchQueue.main.asyncAfter(deadline: .now() + Self.idleDelay, execute: item)
        }
        /// One idle pass: converge the error trees, then read the outline.
        private func runIdle() {
            guard let textView, let analysis = analysis, !textView.hasMarkedText() else { return }
            Self.idleQueue.async { [weak self] in
                guard let outcome = try? analysis.drainForTesting() else { return }
                DispatchQueue.main.async { [weak self] in
                    self?.applyPaint(outcome.paint)
                    self?.updateOutline(outcome.outline)
                }
            }
        }

        /// A command that needs the regions as they are now — fold, unfold, a click on a
        /// marker — outlines synchronously when the document is small, not after the idle pass.
        private func ensureFreshAnalysis() {
            guard let textView, let analysis = analysis else { return }
            guard outlineRevision != analysis.revision else { return }
            guard nsText.length <= Self.synchronousPaintLimit, !textView.hasMarkedText() else { return }
            if let outline = try? analysis.outline() { updateOutline(outline) }
        }

        /// The gutter asks before it resolves a click, so a marker's offset is never the offset of a
        /// text that has since moved.
        func refreshMarksIfStale() {
            ensureFreshAnalysis()
        }

        /// A synchronous paint plus outline, for tests: converge first so colours, folds and
        /// issues match a fresh document.
        func syncAnalysisForTesting() throws {
            guard let analysis = analysis else { return }
            let outcome = try analysis.drainForTesting()
            applyPaint(outcome.paint)
            updateOutline(outcome.outline)
        }

        /// Wear an outline: statements for the band, the run marks and the rotor; folds for the
        /// gutter. Folds the user closed stay closed while their header is still a region's header.
        private func updateOutline(_ outline: EditorOutlineData) {
            PerfSignposts.part(PerfSignposts.Part.outlineApply) { updateOutlineBody(outline) }
        }

        private func updateOutlineBody(_ outline: EditorOutlineData) {
            guard let textView else { return }
            // A result for text that has since changed is worth nothing: the edit that changed it
            // scheduled its own idle pass.
            guard outline.revision == analysis?.revision else { return }
            outlineRevision = outline.revision
            statementBounds = outline.statements
            if parent.layout.codeFolding {
                let starts = SQLFolding.lineStarts(in: nsText)
                let length = nsText.length
                foldRegions = outline.folds.compactMap {
                    SQLFolding.foldRegion($0, starts: starts, length: length)
                }.sorted { ($0.header, $0.lastLine) < ($1.header, $1.lastLine) }
                let byHeader = Dictionary(foldRegions.map { ($0.header, $0) }, uniquingKeysWith: { first, _ in first })
                // Only a header that is still a foldable region's header keeps its fold: a region
                // that stopped being multi-line must not stay collapsed.
                folded = folded.reduce(into: [:]) { kept, entry in
                    if let region = byHeader[entry.key] { kept[entry.key] = region.body }
                }
                unfoldRegions(containing: textView.selectedRange().location)
            } else {
                // Folding off means no regions and no markers. The offsets are dropped with them: a
                // fold collapsed when the switch went off would otherwise come back somewhere else.
                foldRegions = []
                folded = [:]
            }
            rotor = StatementsRotorSource(statements: statementBounds, text: nsText)
            lexicalIssues = outline.issues
            refreshDiagnostics()
            hideFolded()
            updateFoldMarks()
            updateRunMarks()
            updateHighlight(textView)
        }

        // MARK: Diagnostics

        /// Merge the lexical issues with the server's mark, underline them, and refresh the rotor and
        /// the readout. Only called while the outline is the text's own.
        private func refreshDiagnostics() {
            guard let textView, let layoutManager = textView.layoutManager else { return }
            let text = nsText
            diagnostics = EditorDiagnostics.merge(issues: lexicalIssues, server: serverMark?.diagnostic,
                                                  text: text)
            diagnosticsPainter.apply(diagnostics, to: layoutManager, length: text.length)
            issuesRotor = IssuesRotorSource(diagnostics: diagnostics, text: text)
            var sources: [any EditorRotorSource] = []
            if let rotor { sources.append(rotor) }
            if let issuesRotor { sources.append(issuesRotor) }
            textView.rotorSources = sources
            publishIssues()
        }

        private func publishIssues() {
            guard let box = parent.lineCount else { return }
            let summary = EditorIssueSummary(diagnostics)
            guard box.issues != summary else { return }
            DispatchQueue.main.async { if box.issues != summary { box.issues = summary } }
        }

        /// A tab whose connection changed reads its SQL under another dialect; the analysis is fixed
        /// to the one it was built with, so it is built again.
        func adoptDialect() {
            guard let analysis, analysis.dialect != parent.dialect else { return }
            rebuildAnalysis()
        }

        /// Wear the model's server mark, if it is for the text on screen. Called on every SwiftUI update.
        func adoptServerMark(_ mark: ServerErrorMark?) {
            let wanted = mark?.id == dismissedMarkID ? nil : mark
            guard wanted?.id != serverMark?.id else { return }
            if let wanted, nsText.isEqual(to: wanted.sqlSnapshot),
               let range = wanted.range(in: nsText) {
                serverMark = (wanted.id, EditorDiagnostic(kind: .server, range: range, message: wanted.message))
            } else {
                // Not this text, or a position outside it: never retried for this mark.
                dismissedMarkID = wanted?.id ?? dismissedMarkID
                serverMark = nil
            }
            // The lexical issues are the outline's, so the merge waits for an outline that is current.
            ensureFreshAnalysis()
            if outlineRevision == analysis?.revision { refreshDiagnostics() } else { scheduleIdle() }
        }

        func textDidBeginEditing(_ notification: Notification) {
            parent.focused = true
        }

        func textDidEndEditing(_ notification: Notification) {
            parent.focused = false
            parent.completion.dismiss()
            flushToModel()
        }

        func textViewDidChangeSelection(_ notification: Notification) {
            guard let textView else { return }
            // Run needs both: which statement the caret is in, and what is highlighted. While text is
            // owed the caret is owed with it: typing moves it on every key, and each write is a
            // SwiftUI update for whatever reads it.
            if hasUnpublishedEdits {
                selectionOwed = true
            } else {
                publishSelection(textView)
            }
            // The bands follow the caret, which is the whole point of them.
            updateHighlight(textView)
            // A caret moving into a collapsed body would be invisible, and the next keystroke would
            // land somewhere the user cannot see. Opening the fold is the honest answer.
            if !folded.isEmpty {
                let caret = textView.selectedRange().location
                if folded.values.contains(where: { NSLocationInRange(caret, $0) }) {
                    unfoldRegions(containing: caret)
                    hideFolded()
                    updateFoldMarks()
                }
            }
            // The emptied document is the one case the caret can settle on its own: there is no word
            // left to complete, so the list cannot outlive the deletion that emptied it. Every other
            // way the caret moves is handled where it happens — a click closes the list through
            // `SQLTextView.onClick`, and a keystroke rebuilds it against the new caret in `refresh`.
            guard parent.completion.active else { return }
            if nsText.length == 0 { parent.completion.dismiss() }
        }

        /// Tell SwiftUI how many lines there are, when that changed. Deferred a turn: this is also
        /// called from `makeNSView` and `updateNSView`, where writing state is not allowed.
        /// Tell the pane whether the find bar is open, so its Clear button stays off the bar's close
        /// button (B-1).
        private func publishFindOpen() {
            guard let box = parent.lineCount else { return }
            let open = findVisible
            DispatchQueue.main.async { if box.findOpen != open { box.findOpen = open } }
        }

        private func publishLineCount() {
            guard let box = parent.lineCount, let ruler else { return }
            let lines = ruler.lineCount
            guard box.value != lines else { return }
            DispatchQueue.main.async { if box.value != lines { box.value = lines } }
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
            publishFindOpen()
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
            publishFindOpen()
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

        private func unfoldRegions(containing offset: Int) {
            folded = folded.filter { !NSLocationInRange(offset, $0.value) }
        }

        /// Give the layout manager the bodies that are folded now. Only the ranges that changed since
        /// last time are invalidated; a fold or an unfold touches its own lines and nothing else.
        private func hideFolded(invalidating: Bool = true) {
            guard let textView else { return }
            SQLFoldStyler.hide(folded.values.filter { $0.length > 0 }, in: textView, invalidating: invalidating)
        }

        func toggleFold(headerOffset: Int) {
            ensureFreshAnalysis()
            guard let textView,
                  let region = foldRegions.first(where: { $0.header == headerOffset }) else { return }
            if folded[headerOffset] != nil {
                folded[headerOffset] = nil
            } else {
                folded[headerOffset] = region.body
                // Put the caret on the visible header rather than leaving it inside a body that is
                // about to disappear.
                textView.setSelectedRange(NSRange(location: region.header, length: 0))
            }
            hideFolded()
            updateFoldMarks()
        }

        private func foldAtCaret() {
            ensureFreshAnalysis()
            guard let textView else { return }
            let caret = textView.selectedRange().location
            let candidates = foldRegions.filter { caret >= $0.header && caret < $0.bodyEnd }
            // Innermost: when a statement fold and a CTE fold overlap, the one whose header is
            // nearest the caret wins, so folding inside a `WITH` folds the part being looked at.
            guard let region = candidates.max(by: { $0.header < $1.header }) else { return }
            folded[region.header] = region.body
            hideFolded()
            updateFoldMarks()
        }

        private func unfoldAtCaret() {
            ensureFreshAnalysis()
            guard let textView else { return }
            let caret = textView.selectedRange().location
            let candidates = foldRegions.filter { folded[$0.header] != nil && caret >= $0.header && caret < $0.bodyEnd }
            guard let region = candidates.max(by: { $0.header < $1.header }) else { return }
            folded[region.header] = nil
            hideFolded()
            updateFoldMarks()
        }

        private func updateFoldMarks() {
            guard let ruler else { return }
            ruler.foldMarks = foldRegions.map { region in
                LineNumberRulerView.FoldMark(headerLine: region.headerLine,
                                             headerOffset: region.header,
                                             folded: folded[region.header] != nil,
                                             summary: region.summary)
            }
        }

        /// A click moved the caret: close the list rather than leave it pinned to the old one.
        func dismissCompletionOnClick() {
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
            let text = nsText
            let caret = textView.selectedRange().location
            guard caret <= text.length else { return nil }
            // No SQL suggestions inside a string literal: the user is writing data, not code.
            guard !insideStringLiteral(text, upTo: caret) else { return nil }
            guard let parsed = WordScope.parse(text, upTo: caret) else { return nil }
            return WordContext(prefix: parsed.prefix, path: parsed.path,
                               minimumPrefix: parsed.path.isEmpty ? 2 : 0)
        }

        private func wordRange(in textView: NSTextView) -> NSRange {
            let text = nsText
            let caret = textView.selectedRange().location
            var start = caret
            while start > 0, Self.isWordCharacter(text.character(at: start - 1)) { start -= 1 }
            return NSRange(location: start, length: caret - start)
        }

        private static func isWordCharacter(_ character: unichar) -> Bool {
            guard let scalar = Unicode.Scalar(character) else { return false }
            return CharacterSet.alphanumerics.contains(scalar) || character == 0x5F  // _
        }

        /// The quote characters the three drivers use for identifiers: `"` for Trino and Postgres,
        /// a backtick for MySQL.
        private static func isQuote(_ character: unichar) -> Bool {
            character == 0x22 || character == 0x60
        }

        /// An odd number of unescaped quotes before the caret means the caret is inside one.
        private func insideStringLiteral(_ text: NSString, upTo caret: Int) -> Bool {
            guard caret > 0 else { return false }
            // One bulk copy and a plain loop: reading up to the caret a unit at a time through an
            // `NSString` is a call per character, and this runs on every pause in typing.
            var units = [unichar](repeating: 0, count: caret)
            text.getCharacters(&units, range: NSRange(location: 0, length: caret))
            var open = false
            var index = 0
            while index < caret {
                if units[index] == 0x27 {                    // '
                    if index + 1 < caret, units[index + 1] == 0x27 {
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

/// One stop in the editor's statement rotor: what it is called, and where it starts.
struct EditorRotorItem: Equatable {
    let label: String
    let offset: Int
    /// How much text the stop covers, for the range VoiceOver moves to; 0 is a point.
    var length = 0
}

/// Something the editor can step through: the "Statements" rotor and the "Query issues" rotor.
/// The coordinator rebuilds the lists with every outline.
protocol EditorRotorSource {
    var title: String { get }
    var items: [EditorRotorItem] { get }
}

/// The statements as rotor stops: "SELECT" reads better than "statement 2", so the leading
/// keyword heads the label and the fallback keeps the position.
struct StatementsRotorSource: EditorRotorSource, Equatable {
    let title = "Statements"
    var items: [EditorRotorItem]

    init(statements: [NSRange], text: NSString) {
        items = statements.enumerated().map { index, range in
            EditorRotorItem(label: Self.label(index: index, range: range, text: text),
                            offset: range.location, length: range.length)
        }
    }
    static func label(index: Int, range: NSRange, text: NSString) -> String {
        let limit = min(NSMaxRange(range), text.length)
        var start = range.location
        while start < limit, let scalar = UnicodeScalar(text.character(at: start)),
              CharacterSet.whitespacesAndNewlines.contains(scalar) { start += 1 }
        var end = start
        while end < limit, Self.isWord(text.character(at: end)) { end += 1 }
        let word = end > start
            ? text.substring(with: NSRange(location: start, length: end - start)).uppercased()
            : "STATEMENT"
        return "\(index + 1) · \(word)"
    }

    private static func isWord(_ character: unichar) -> Bool {
        guard let scalar = Unicode.Scalar(character) else { return false }
        return CharacterSet.letters.contains(scalar) || character == 0x5F
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

    /// A hidden find bar, born at its fitting size and not at zero: its own frame is a required
    /// constraint, and a zero one cannot hold its stack's insets, so the first layout pass logged a
    /// constraint conflict (B-1).
    static func makeFindBar() -> SQLFindBar {
        let bar = SQLFindBar()
        bar.frame = NSRect(origin: .zero, size: bar.fittingSize)
        bar.isHidden = true
        return bar
    }

    override func layout() {
        super.layout()
        guard let findBar else { return }
        findBar.isHidden = !findBarVisible
        // `fittingSize` rather than a constant: the bar grows a second row when Replace is shown,
        // and a fixed height would clip it.
        let fitting = findBar.fittingSize
        let barHeight = findBarVisible ? fitting.height : 0
        // A hidden bar keeps its fitting size. At height 0, or in the zero-width container SwiftUI
        // builds before it sizes it, its stack's 10 + 10 and 7 + 7 point insets could not be satisfied
        // and Auto Layout logged a constraint conflict on every layout (B-1). A hidden view takes no
        // room and draws nothing, so the frame is only what its constraints need.
        findBar.frame = NSRect(x: 0, y: 0, width: max(bounds.width, fitting.width), height: fitting.height)
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

/// The AppKit half of code folding: it hides and shows the collapsed lines.
///
/// Folding is a **layout** change, never a character change and no longer an attribute change. The
/// text the editor holds — and the model behind the binding — is exactly what the user typed, which
/// is what makes editing around a fold safe. The body is hidden by the layout manager's delegate:
/// the glyphs of a folded line are generated as `.null`, so they have no width and draw nothing, and
/// each folded line's fragment is squashed to `collapsedLineHeight`. Nothing is written into the text
/// storage, so a fold cannot be undone by a repaint and cannot outlive the text it was made for, and
/// a fold or an unfold invalidates the lines it covers instead of the whole document.
enum SQLFoldStyler {
    /// The height a folded line is squashed to.
    ///
    /// Not literally zero: a fragment that is zero tall is skipped by some of what walks the layout —
    /// measured, the first prototype saved 0pt — so the smallest positive value is used instead.
    static let collapsedLineHeight: CGFloat = 0.1

    /// Hide exactly these regions' bodies, and show whatever was hidden before.
    static func apply(_ regions: [FoldRegion], to textView: NSTextView) {
        hide(regions.map(\.body).filter { $0.length > 0 }, in: textView)
    }

    /// `invalidating: false` moves the hidden ranges without touching the layout: for an edit, where
    /// the layout manager has already invalidated what changed and the glyphs it kept carry their
    /// hidden property with them.
    static func hide(_ bodies: [NSRange], in textView: NSTextView, invalidating: Bool = true) {
        guard let layoutManager = textView.layoutManager else { return }
        FoldHider.installed(on: layoutManager).replace(bodies, in: layoutManager,
                                                       redisplaying: invalidating ? textView : nil)
    }
}

/// The layout manager's delegate for folding. One per layout manager, kept alive by it.
final class FoldHider: NSObject, NSLayoutManagerDelegate {
    private(set) var hidden: [NSRange] = []
    private static var associationKey = 0

    static func installed(on layoutManager: NSLayoutManager) -> FoldHider {
        if let existing = objc_getAssociatedObject(layoutManager, &associationKey) as? FoldHider {
            if layoutManager.delegate !== existing { layoutManager.delegate = existing }
            return existing
        }
        let hider = FoldHider()
        objc_setAssociatedObject(layoutManager, &associationKey, hider, .OBJC_ASSOCIATION_RETAIN_NONATOMIC)
        layoutManager.delegate = hider
        return hider
    }

    func replace(_ bodies: [NSRange], in layoutManager: NSLayoutManager, redisplaying textView: NSTextView?) {
        let updated = Self.normalised(bodies)
        guard updated != hidden else { return }
        let changed = Self.minus(hidden, updated) + Self.minus(updated, hidden)
        hidden = updated
        guard let textView, let storage = layoutManager.textStorage else { return }
        let text = storage.mutableString as NSString
        for range in changed {
            let clamped = NSIntersectionRange(range, NSRange(location: 0, length: text.length))
            guard clamped.length > 0 else { continue }
            // From the line before: the hidden glyphs of the first folded line are laid out into the
            // header's own fragment, so the header is part of what changed.
            let start = text.lineRange(for: NSRange(location: max(0, clamped.location - 1), length: 0)).location
            let span = NSRange(location: start, length: NSMaxRange(clamped) - start)
            layoutManager.invalidateGlyphs(forCharacterRange: span, changeInLength: 0, actualCharacterRange: nil)
            layoutManager.invalidateLayout(forCharacterRange: span, actualCharacterRange: nil)
        }
        textView.needsDisplay = true
    }

    private func isHidden(_ index: Int) -> Bool {
        hidden.contains { NSLocationInRange(index, $0) }
    }

    /// Sorted, non-empty and non-overlapping: a CTE inside a folded statement is one hidden range.
    private static func normalised(_ ranges: [NSRange]) -> [NSRange] {
        var merged: [NSRange] = []
        for range in ranges.filter({ $0.length > 0 }).sorted(by: { $0.location < $1.location }) {
            if let last = merged.last, range.location <= NSMaxRange(last) {
                merged[merged.count - 1] = NSUnionRange(last, range)
            } else {
                merged.append(range)
            }
        }
        return merged
    }

    private static func minus(_ from: [NSRange], _ cuts: [NSRange]) -> [NSRange] {
        var out: [NSRange] = []
        for range in from {
            var pieces = [range]
            for cut in cuts {
                pieces = pieces.flatMap { piece -> [NSRange] in
                    let overlap = NSIntersectionRange(piece, cut)
                    guard overlap.length > 0 else { return [piece] }
                    return [NSRange(location: piece.location, length: overlap.location - piece.location),
                            NSRange(location: NSMaxRange(overlap), length: NSMaxRange(piece) - NSMaxRange(overlap))]
                        .filter { $0.length > 0 }
                }
            }
            out += pieces
        }
        return out
    }

    // MARK: NSLayoutManagerDelegate

    /// A folded character becomes a null glyph. Newlines are left alone: a null newline would not
    /// end its line, and the line ending is what gives each folded line a fragment of its own to
    /// squash. Everything else the layout manager decided is passed through unchanged.
    func layoutManager(_ layoutManager: NSLayoutManager, shouldGenerateGlyphs glyphs: UnsafePointer<CGGlyph>,
                       properties props: UnsafePointer<NSLayoutManager.GlyphProperty>,
                       characterIndexes: UnsafePointer<Int>, font: NSFont,
                       forGlyphRange glyphRange: NSRange) -> Int {
        guard !hidden.isEmpty else { return 0 }
        var updated = Array(UnsafeBufferPointer(start: props, count: glyphRange.length))
        var changed = false
        for i in 0..<glyphRange.length where updated[i] != .controlCharacter && isHidden(characterIndexes[i]) {
            updated[i] = .null
            changed = true
        }
        guard changed else { return 0 }
        updated.withUnsafeBufferPointer {
            layoutManager.setGlyphs(glyphs, properties: $0.baseAddress!, characterIndexes: characterIndexes,
                                    font: font, forGlyphRange: glyphRange)
        }
        return glyphRange.length
    }

    /// A fragment that starts in a folded line is the same height as it ever was: squashed.
    func layoutManager(_ layoutManager: NSLayoutManager,
                       shouldSetLineFragmentRect lineFragmentRect: UnsafeMutablePointer<NSRect>,
                       lineFragmentUsedRect: UnsafeMutablePointer<NSRect>,
                       baselineOffset: UnsafeMutablePointer<CGFloat>, in textContainer: NSTextContainer,
                       forGlyphRange glyphRange: NSRange) -> Bool {
        guard !hidden.isEmpty, isHidden(layoutManager.characterIndexForGlyph(at: glyphRange.location)) else {
            return false
        }
        lineFragmentRect.pointee.size.height = SQLFoldStyler.collapsedLineHeight
        lineFragmentUsedRect.pointee.size.height = SQLFoldStyler.collapsedLineHeight
        baselineOffset.pointee = 0
        return true
    }
}

/// One change to the characters: where, what it replaced, and what it left.
struct TextEdit {
    let location: Int
    let oldLength: Int
    let newLength: Int

    var oldEnd: Int { location + oldLength }
    var delta: Int { newLength - oldLength }

    /// A range that follows the text: an edit before it moves it, an edit inside it stretches it, an
    /// edit after it leaves it alone.
    func grow(_ range: NSRange) -> NSRange {
        if NSMaxRange(range) <= location { return range }
        if range.location >= oldEnd { return NSRange(location: range.location + delta, length: range.length) }
        let start = min(range.location, location)
        let end = max(NSMaxRange(range), oldEnd) + delta
        return NSRange(location: start, length: max(0, end - start))
    }

    /// The same for a sorted list: the ones before the edit are not visited.
    func apply(to ranges: inout [NSRange]) {
        var low = 0, high = ranges.count
        while low < high {
            let mid = (low + high) / 2
            if NSMaxRange(ranges[mid]) <= location { low = mid + 1 } else { high = mid }
        }
        var index = low
        while index < ranges.count {
            ranges[index] = grow(ranges[index])
            index += 1
        }
    }
}

/// Notification tokens and the event monitor a coordinator holds, removed when it goes.
private final class ObserverBag {
    var tokens: [NSObjectProtocol] = []
    var monitor: Any?

    deinit {
        for token in tokens { NotificationCenter.default.removeObserver(token) }
        if let monitor { NSEvent.removeMonitor(monitor) }
    }
}

/// `NSTextView` that lets the coordinator see keys before AppKit does, and that refuses to show
/// AppKit's own completion list.
final class SQLTextView: NSTextView {
    var interceptKey: ((NSEvent) -> Bool)?

    /// The lists VoiceOver's rotor steps through ("Statements", "Query issues"), swapped by the
    /// coordinator with every outline. The rotors themselves are made on demand and read this.
    var rotorSources: [any EditorRotorSource] = []
    /// A rotor holds its delegate weakly, so the view keeps them.
    private var rotorDelegates: [EditorRotorDelegate] = []

    override func accessibilityCustomRotors() -> [NSAccessibilityCustomRotor] {
        rotorDelegates = rotorSources.map { EditorRotorDelegate(title: $0.title, textView: self) }
        return zip(rotorSources, rotorDelegates).map {
            NSAccessibilityCustomRotor(label: $0.title, itemSearchDelegate: $1)
        }
    }

    /// Called before a click in the text is handled. A click moves the caret, so a suggestion list
    /// still anchored to the old one is pointing at the wrong word; closing it is this hook's job.
    var onClick: (() -> Void)?

    /// The bands drawn behind the text, painted in order: the caret's statement first, its line
    /// over it. Behind the text rather than over it, which is what keeps the selection, the syntax
    /// colours and the caret readable through them.
    var highlightRanges: [NSRange] = [] {
        didSet {
            guard highlightRanges != oldValue else { return }
            invalidateBands()
        }
    }

    /// Where the bands were last laid out, in this view's coordinates.
    private var paintedBands: [NSRect] = []

    /// The colour those bands are painted in. From the palette rather than a constant, so a light
    /// canvas does not get a light band on it.
    var highlightColour: NSColor = .clear

    /// Repaint the bands that moved, not the view. A band changes with nearly every keystroke — the
    /// statement's range grows by a character — and asking for the whole view to be redrawn made each
    /// key re-rasterize every visible glyph, which was most of what a keystroke cost. The old
    /// geometry is kept rather than recomputed, because the ranges it came from describe a text that
    /// has since changed.
    private func invalidateBands() {
        if layoutManager == nil { needsDisplay = true }
        let updated = bandRects(for: highlightRanges)
        guard updated != paintedBands else { return }
        for rect in paintedBands + updated { setNeedsDisplay(rect) }
        paintedBands = updated
    }

    /// The band a highlighted range is painted as: one rectangle from its first line to its last, in
    /// this view's coordinates. Asking the layout manager for every line's rectangle laid out all of a
    /// long statement to draw what is one continuous wash.
    private func bandRects(for ranges: [NSRange]) -> [NSRect] {
        guard let layoutManager, let textContainer else { return [] }
        let length = textStorage?.length ?? 0
        var rects: [NSRect] = []
        for range in ranges {
            let clamped = NSIntersectionRange(range, NSRange(location: 0, length: length))
            guard clamped.length > 0 else { continue }
            let glyphs = layoutManager.glyphRange(forCharacterRange: clamped, actualCharacterRange: nil)
            guard glyphs.length > 0 else { continue }
            let first = layoutManager.lineFragmentUsedRect(forGlyphAt: glyphs.location, effectiveRange: nil)
            let last = layoutManager.lineFragmentUsedRect(forGlyphAt: NSMaxRange(glyphs) - 1, effectiveRange: nil)
            let band = NSRect(x: 0, y: first.minY, width: max(first.maxX, last.maxX),
                              height: last.maxY - first.minY)
            _ = textContainer
            rects.append(HighlightBand.rect(for: band, boundsWidth: bounds.width, inset: textContainerInset))
        }
        return rects
    }

    /// A click in the text is a caret move, so the suggestion list closes before the caret lands
    /// rather than staying anchored to the word it was built for.
    override func mouseDown(with event: NSEvent) {
        onClick?()
        super.mouseDown(with: event)
    }

    override func keyDown(with event: NSEvent) {
        PerfSignposts.keystrokeBegin()
        if let interceptKey, interceptKey(event) { return }
        super.keyDown(with: event)
    }

    /// True from the moment a paint is laid onto the layout manager until the next edit, so a
    /// `--bench` run can tell the draw an analysis repaint caused from the one a key caused.
    var applyTurnOpen = false

    /// Under `--bench` only: the time `layout` and `draw` took, split by the turn that asked for
    /// them, and how much of the view was dirty. Layout is forced first so `draw` is the drawing
    /// alone; the layout manager would have done the same work inside `super`.
    override func draw(_ dirtyRect: NSRect) {
        guard PerfSignposts.recording else { return super.draw(dirtyRect) }
        let prefix = applyTurnOpen ? "apply." : ""
        if let layoutManager, let textContainer {
            let origin = textContainerOrigin
            PerfSignposts.part(prefix + PerfSignposts.Part.layout) {
                layoutManager.ensureLayout(forBoundingRect: dirtyRect.offsetBy(dx: -origin.x, dy: -origin.y), in: textContainer)
            }
        }
        var rects: UnsafePointer<NSRect>?
        var count = 0
        getRectsBeingDrawn(&rects, count: &count)
        PerfSignposts.count(prefix + "dirtyRects", by: count)
        PerfSignposts.count(prefix + "dirtyPoints", by: Int(dirtyRect.height.rounded()))
        PerfSignposts.part(prefix + PerfSignposts.Part.draw) { super.draw(dirtyRect) }
    }

    override func drawBackground(in rect: NSRect) {
        super.drawBackground(in: rect)
        guard !highlightRanges.isEmpty else { return }
        highlightColour.setFill()
        for band in bandRects(for: highlightRanges) where band.intersects(rect) {
            NSBezierPath(rect: band).fill()
        }
    }
}

/// The band one line of a highlight is painted as.
///
/// A value of its own because this arithmetic was wrong in a way nothing could see: TextKit reports
/// the line fragments in the *text container's* space and the fill happens in the view's, so leaving
/// the inset out put every band one inset height above the line it belonged to — a stray strip of
/// wash above the statement and a bare bottom edge under it. `drawHashMarksAndLabels` had always
/// added the same inset for the same reason; only this one had forgotten.
struct HighlightBand {
    /// `band` is a line fragment as TextKit reports it and `boundsWidth` is the view's own width.
    ///
    /// Full width rather than the glyphs' own, so a band reads as a line of the editor and not as a
    /// highlight of the text that happens to sit on it.
    static func rect(for band: NSRect, boundsWidth: CGFloat, inset: NSSize) -> NSRect {
        NSRect(x: 0, y: band.minY + inset.height,
               width: max(boundsWidth, band.maxX), height: band.height)
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

/// Where each line of a text starts, kept in step with the text one edit at a time.
///
/// The gutter needs "which line is this offset on" for every visible line and "how many lines are
/// there" for its width. Both used to be answered by walking the whole document, which is a pass
/// over the text per keystroke; this holds the answer and moves it through each edit instead, in
/// time proportional to the lines after the edit rather than the characters in the document.
struct LineIndex {
    /// UTF-16 offset of each line's first character; `starts[0]` is 0, and a text ending in a
    /// newline has a final, empty line.
    private(set) var starts: [Int]
    /// The length of the text this describes, which is how a stale index is noticed.
    private(set) var length: Int

    init(text: NSString) {
        starts = SQLFolding.lineStarts(in: text)
        length = text.length
    }

    var count: Int { starts.count }

    func line(containing offset: Int) -> Int {
        SQLFolding.line(containing: offset, lineStarts: starts)
    }

    /// `range` is where the edit left its text, and `delta` how much longer the text got.
    mutating func edit(range: NSRange, delta: Int, in text: NSString) {
        // A line starts after each newline, so the starts that belonged to the replaced text are the
        // ones in (location, oldEnd]; everything after that only moves.
        let oldEnd = range.location + range.length - delta
        let low = firstStart(after: range.location)
        let high = max(low, firstStart(after: oldEnd))
        var added: [Int] = []
        if range.length > 0 {
            var units = [unichar](repeating: 0, count: range.length)
            text.getCharacters(&units, range: range)
            for i in 0..<range.length where units[i] == 0x0A { added.append(range.location + i + 1) }
        }
        starts.replaceSubrange(low..<high, with: added)
        if delta != 0 {
            var i = low + added.count
            while i < starts.count {
                starts[i] += delta
                i += 1
            }
        }
        length += delta
    }

    /// The index of the first start greater than `offset`.
    private func firstStart(after offset: Int) -> Int {
        var low = 0, high = starts.count
        while low < high {
            let mid = (low + high) / 2
            if starts[mid] <= offset { low = mid + 1 } else { high = mid }
        }
        return low
    }
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
        didSet {
            labelSizes = [:]
            needsDisplay = true
        }
    }

    /// The width of a line number in the current font, by its digit count. Measuring a string is
    /// most of what drawing a number costs, and every number of one length measures the same.
    private var labelSizes: [Int: NSSize] = [:]

    /// The line index, built on first use and moved through each edit after that.
    private var index: LineIndex?

    /// The storage's own string: reading it does not copy the document the way `textView.string` does.
    private var text: NSString {
        (textView?.textStorage?.mutableString ?? NSMutableString()) as NSString
    }

    /// The index, rebuilt if the text has changed length behind its back — a text view that was
    /// filled without telling the gutter, as the tests do.
    private func currentIndex() -> LineIndex {
        let text = self.text
        if let index, index.length == text.length { return index }
        let rebuilt = LineIndex(text: text)
        index = rebuilt
        return rebuilt
    }

    /// Lines in the text. What the corner readout shows and what the gutter's width is sized from.
    var lineCount: Int { currentIndex().count }

    /// The count as of the last edit the index was told about, without checking it against the text.
    /// For the moment inside `didProcessEditing`, when the text is already new and the index is not.
    var knownLineCount: Int? { index?.count }

    /// The 0-based line an offset falls on.
    func line(containing offset: Int) -> Int { currentIndex().line(containing: offset) }

    /// The text was replaced or first loaded: index it from scratch.
    func rebuildIndex() {
        index = LineIndex(text: text)
        updateWidth()
        needsDisplay = true
    }

    /// One edit, from the text storage: move the index through it. `range` is where the new text
    /// sits and `delta` how much the text grew.
    func textEdited(range: NSRange, delta: Int) {
        let text = self.text
        guard var current = index, current.length + delta == text.length else {
            index = LineIndex(text: text)
            updateWidth()
            return
        }
        let before = current.count
        current.edit(range: range, delta: delta, in: text)
        index = current
        if current.count != before {
            updateWidth()
            needsDisplay = true
        }
    }

    /// Called just before a click is resolved to a marker, so the marks it reads are current.
    var refreshMarks: (() -> Void)?

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
        index = LineIndex(text: text as NSString)
        updateWidth()
        needsDisplay = true
    }

    /// The width the current line count and the run column ask for.
    private func updateWidth() {
        let wanted = Self.gutterWidth(forLines: index?.count ?? 1, showsRunMarks: showsRunMarks)
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
        PerfSignposts.part("rulerDraw") { drawLabels(in: rect) }
    }

    private func drawLabels(in rect: NSRect) {
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
            let hasRun = Self.mark(in: runMarks, at: line, key: \.headerLine) != nil
            // The run marker, at the gutter's leading edge. The fold marker moves over when both are
            // on this line, because a statement's first line is often also a fold header.
            if hasRun { drawRunMarker(midY: midY) }
            if let mark = Self.mark(in: foldMarks, at: line, key: \.headerLine) {
                // Always in the fold column when the run column is reserved, whether or not this
                // line has a run marker: a marker drawn inside the run column would be read as a
                // click on the other control and could not be hit at all.
                drawFoldMarker(mark, midY: midY,
                               x: showsRunMarks ? Self.foldMarkerShiftedX : Self.foldMarkerX)
            }

            let label = "\(entry.number)" as NSString
            let size: NSSize
            if let known = labelSizes[label.length] {
                size = known
            } else {
                size = label.size(withAttributes: attributes)
                labelSizes[label.length] = size
            }
            let point = NSPoint(x: ruleThickness - size.width - 8,
                                y: origin.y + inset.height + entry.minY + (entry.height - size.height) / 2)
            guard NSRect(origin: point, size: size).intersects(rect) else { continue }
            label.draw(at: point, withAttributes: attributes)
        }
    }

    /// The mark on a line. The coordinator hands the marks over in text order, so this is a binary
    /// search: with a mark per statement, a scan per visible line per draw is thousands of
    /// comparisons on every keystroke.
    private static func mark<Mark>(in marks: [Mark], at line: Int, key: KeyPath<Mark, Int>) -> Mark? {
        var low = 0, high = marks.count
        while low < high {
            let mid = (low + high) / 2
            if marks[mid][keyPath: key] < line { low = mid + 1 } else { high = mid }
        }
        return low < marks.count && marks[low][keyPath: key] == line ? marks[low] : nil
    }

    /// A right-pointing triangle: the mark that this statement can be run from here.
    ///
    /// Sized to be aimed at rather than merely seen. At nine by six and a half it read as a speck
    /// beside the fold marker's nine by nine, and it is the one control in the gutter that does
    /// something to the server. Eleven by eight still fits the 14-point run column.
    private func drawRunMarker(midY: CGFloat) {
        let path = NSBezierPath()
        let x = Self.runMarkerX
        path.move(to: NSPoint(x: x, y: midY - 5.5))
        path.line(to: NSPoint(x: x, y: midY + 5.5))
        path.line(to: NSPoint(x: x + 8, y: midY))
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
        refreshMarks?()
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
        return Self.mark(in: runMarks, at: line, key: \.headerLine)?.headerOffset
    }

    private func foldHeader(at point: NSPoint) -> Int? {
        // With the run column reserved, the leading strip belongs to the run marker.
        if showsRunMarks, point.x < Self.runColumn { return nil }
        guard let line = line(at: point) else { return nil }
        return Self.mark(in: foldMarks, at: line, key: \.headerLine)?.headerOffset
    }

    /// The 0-based line under a point in the ruler.
    private func line(at point: NSPoint) -> Int? {
        let string = text
        guard let textView,
              let layoutManager = textView.layoutManager,
              let container = textView.textContainer,
              string.length > 0
        else { return nil }
        let origin = convert(NSPoint.zero, from: textView)
        let containerPoint = NSPoint(x: 0, y: point.y - origin.y - textView.textContainerInset.height)
        let character = min(max(0, layoutManager.characterIndex(for: containerPoint, in: container,
                                                                fractionOfDistanceBetweenInsertionPoints: nil)),
                            string.length - 1)
        return line(containing: character)
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

        let string = text

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
        var line = 1 + currentIndex().line(containing: firstCharacter)
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
}
