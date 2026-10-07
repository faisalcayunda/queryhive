import AppKit
import QueryHiveFFI

extension ConnectionKind {
    /// The lexical family the editor reads a tab's SQL under (blueprint w10 §8.6): the quote,
    /// escape and comment rules differ, and a statement boundary depends on all three.
    var editorDialect: EditorDialect {
        switch self {
        case .trino: .trino
        case .postgres: .postgres
        case .mysql: .mysql
        }
    }
}

/// The tree-sitter editor analysis: it converts the FFI's flat arrays into ranges, validates every
/// packet against the blueprint §6 invariants, and applies colours only as temporary attributes.
enum EditorAnalysisError: Error, Equatable {
    case stale
    case outOfBounds
    case splitsCharacter
    case tooLarge
    case malformed
}

/// A token's colour class: the FFI's codes 1 to 9, never renumbered.
enum EditorColorClass: UInt32 {
    case comment = 1
    case string = 2
    case quotedIdentifier = 3
    case number = 4
    case keyword = 5
    case literal = 6
    case function = 7
    case punctuation = 8
    case parameter = 9
}

/// One coloured run: the range and its class.
struct EditorPaintRun: Equatable {
    let range: NSRange
    let colorClass: EditorColorClass
}

/// What to draw for a window, in ranges rather than flat arrays.
struct EditorPaintData: Equatable {
    let revision: UInt64
    let length: Int
    let window: NSRange
    let inactive: Bool
    let moreInWindow: Bool
    let dirtyElsewhere: Bool
    let ranges: [NSRange]
    let runs: [EditorPaintRun]
    let fonts: [(range: NSRange, italic: Bool)]

    static func == (lhs: EditorPaintData, rhs: EditorPaintData) -> Bool {
        lhs.revision == rhs.revision && lhs.length == rhs.length && lhs.window == rhs.window
            && lhs.inactive == rhs.inactive && lhs.moreInWindow == rhs.moreInWindow
            && lhs.dirtyElsewhere == rhs.dirtyElsewhere && lhs.ranges == rhs.ranges
            && lhs.runs == rhs.runs && lhs.fonts.elementsEqual(rhs.fonts) {
                $0.range == $1.range && $0.italic == $1.italic
            }
    }
}

enum EditorFoldKind: Equatable {
    case statement
    case cte
    case subquery
    case body
}

/// One collapsible region, in document offsets.
struct EditorFoldData: Equatable {
    let kind: EditorFoldKind
    let headerLine: Int
    let lastLine: Int
    let header: Int
    let bodyStart: Int
    let bodyEnd: Int
    let summary: String
}

enum EditorIssueKind: Equatable {
    case unclosedQuote
    case unclosedIdentifier
    case unclosedComment
    case unclosedDollar
    case unbalancedParen
    case syntaxError
    case missingToken
}

/// An issue at a document range.
struct EditorIssueData: Equatable {
    let kind: EditorIssueKind
    let range: NSRange
}

/// The slow-moving parts of a document: statements, folds, issues.
struct EditorOutlineData: Equatable {
    let revision: UInt64
    let statements: [NSRange]
    let folds: [EditorFoldData]
    let issues: [EditorIssueData]
}

/// One document's analysis: owns the FFI `EditorDocument` and converts its packets.
///
/// `@unchecked Sendable`: the editor hands it to background queues (the idle pass, the coalesced
/// paint) on purpose. The Rust document serialises its own state under mutexes, `pending` is only
/// touched on `paintQueue`, and `revision` is written on the main thread: a background read that
/// lags behind gets `.stale` back from the Rust side.
final class EditorAnalysis: @unchecked Sendable {
    private let document: EditorDocument
    private let paintQueue = DispatchQueue(label: "queryhive.editor.analysis")
    private var pending: [(window: NSRange, budget: Int,
                           completion: (Result<EditorPaintData, EditorAnalysisError>) -> Void)] = []
    private var workerScheduled = false

    /// The newest revision this object has produced through `replace`.
    private(set) var revision: UInt64

    /// The dialect the document was built under. It is fixed for the document's life, so a tab whose
    /// connection changes gets a new analysis rather than a changed one.
    let dialect: EditorDialect

    init(text: String, dialect: EditorDialect = .generic) throws {
        self.dialect = dialect
        document = try EditorDocument(text: text, dialect: dialect)
        do {
            revision = try document.revision()
        } catch let error as EditorError {
            throw Self.convert(error)
        } catch {
            throw EditorAnalysisError.malformed
        }
    }

    /// The text's length and line count, for drift detection against the view.
    var length: Int { (try? get { try Int(document.lenUtf16()) }) ?? -1 }
    var lineCount: Int { (try? get { try Int(document.lineCount()) }) ?? -1 }

    private static func convert(_ error: EditorError) -> EditorAnalysisError {
        switch error {
        case .Stale: return .stale
        case .OutOfBounds: return .outOfBounds
        case .SplitsCharacter: return .splitsCharacter
        case .TooLarge: return .tooLarge
        case .Malformed: return .malformed
        }
    }

    private func get<T>(_ call: () throws -> T) throws -> T {
        do {
            return try call()
        } catch let error as EditorError {
            throw Self.convert(error)
        } catch {
            throw EditorAnalysisError.malformed
        }
    }

    /// Replace `range` with `text`; returns the new revision. Main thread only.
    @discardableResult
    func replace(range: NSRange, with text: String) throws -> UInt64 {
        let next: UInt64 = try get {
            try document.replace(startUtf16: UInt32(range.location),
                                 lenUtf16: UInt32(range.length), text: text)
        }
        revision = next
        return next
    }

    /// What to draw for `window`, at most `budget` units of it. Validated before return.
    func paint(window: NSRange, budget: Int) throws -> EditorPaintData {
        let packet: EditorPaint = try get {
            try document.paint(revision: revision, windowStart: UInt32(window.location),
                               windowLen: UInt32(window.length),
                               budgetUtf16: UInt32(clamping: budget))
        }
        return try Self.convert(packet, length: length)
    }

    /// The UI applied a paint: that part leaves `dirty`.
    func markApplied(_ paint: EditorPaintData) throws {
        var flat: [UInt32] = []
        for range in paint.ranges {
            flat.append(UInt32(range.location))
            flat.append(UInt32(range.length))
        }
        try get { try document.markApplied(revision: paint.revision, ranges: flat) }
    }

    /// `range` must be painted again (the font changed). Main thread only.
    func markDirty(_ range: NSRange) throws {
        try get {
            try document.markDirty(startUtf16: UInt32(range.location),
                                   lenUtf16: UInt32(range.length))
        }
    }

    /// The statements, folds and issues. Background thread, after the debounce.
    func outline() throws -> EditorOutlineData {
        let packet: EditorOutline = try get { try document.outline(revision: revision) }
        return try Self.convert(packet, length: length)
    }

    /// The delimiters touching `offset`, opener first, for the revision this object holds now: two
    /// ranges, or none. Background thread. A result that disagrees with the invariants (an odd
    /// count, a range past the text) is `malformed`, never drawn.
    func bracketPair(at offset: Int) throws -> (revision: UInt64, ranges: [NSRange]) {
        let asked = revision
        let flat: [UInt32] = try get {
            try document.bracketPair(revision: asked, offsetUtf16: UInt32(clamping: offset))
        }
        guard flat.isEmpty || flat.count == 4 else { throw EditorAnalysisError.malformed }
        let ranges = stride(from: 0, to: flat.count, by: 2).map {
            NSRange(location: Int(flat[$0]), length: Int(flat[$0 + 1]))
        }
        let end = length
        guard ranges.allSatisfy({ $0.length > 0 && NSMaxRange($0) <= end }) else {
            throw EditorAnalysisError.malformed
        }
        return (asked, ranges)
    }

    /// Re-parse error trees from scratch, when typing pauses. Background thread.
    func converge() throws {
        try get { try document.converge() }
    }

    /// One coalesced background paint: every request queued while a worker runs
    /// joins it, so a burst of keystrokes paints once and every waiter is answered.
    func requestPaint(window: NSRange, budget: Int,
                      completion: @escaping (Result<EditorPaintData, EditorAnalysisError>) -> Void) {
        paintQueue.async { [weak self] in
            guard let self else { return }
            self.pending.append((window, budget, completion))
            guard !self.workerScheduled else { return }
            self.workerScheduled = true
            self.paintQueue.async {
                self.workerScheduled = false
                let batch = self.pending
                self.pending = []
                guard let newest = batch.last else { return }
                let result: Result<EditorPaintData, EditorAnalysisError>
                do {
                    result = .success(try self.paint(window: newest.window, budget: newest.budget))
                } catch let error as EditorAnalysisError {
                    result = .failure(error)
                } catch {
                    result = .failure(.malformed)
                }
                for waiter in batch {
                    DispatchQueue.main.async { waiter.completion(result) }
                }
            }
        }
    }

    // MARK: Packets in and out

    private static func pairs(_ flat: [UInt32]) throws -> [NSRange] {
        guard flat.count % 2 == 0 else { throw EditorAnalysisError.malformed }
        return stride(from: 0, to: flat.count, by: 2).map {
            NSRange(location: Int(flat[$0]), length: Int(flat[$0 + 1]))
        }
    }

    /// Lift a paint packet, checking the §6 result invariants: even pair lists, triples,
    /// ascending disjoint ranges inside the document, every run and font inside one
    /// range, and classes 1 to 9.
    static func convert(_ packet: EditorPaint, length: Int) throws -> EditorPaintData {
        let ranges = try pairs(packet.ranges)
        guard packet.runs.count % 3 == 0, packet.fonts.count % 3 == 0 else {
            throw EditorAnalysisError.malformed
        }
        var previousEnd = 0
        for range in ranges {
            guard range.location >= previousEnd, NSMaxRange(range) <= length else {
                throw EditorAnalysisError.malformed
            }
            previousEnd = NSMaxRange(range)
        }
        func inside(_ range: NSRange) -> Bool {
            ranges.contains { NSLocationInRange(range.location, $0)
                && NSMaxRange(range) <= NSMaxRange($0) }
        }
        var runs: [EditorPaintRun] = []
        for step in stride(from: 0, to: packet.runs.count, by: 3) {
            let range = NSRange(location: Int(packet.runs[step]),
                                length: Int(packet.runs[step + 1]))
            guard let colorClass = EditorColorClass(rawValue: packet.runs[step + 2]),
                  inside(range) else {
                throw EditorAnalysisError.malformed
            }
            runs.append(EditorPaintRun(range: range, colorClass: colorClass))
        }
        var fonts: [(range: NSRange, italic: Bool)] = []
        for step in stride(from: 0, to: packet.fonts.count, by: 3) {
            let range = NSRange(location: Int(packet.fonts[step]),
                                length: Int(packet.fonts[step + 1]))
            let italic = packet.fonts[step + 2]
            guard (italic == 0 || italic == 1), inside(range) else {
                throw EditorAnalysisError.malformed
            }
            fonts.append((range, italic == 1))
        }
        return EditorPaintData(
            revision: packet.revision, length: Int(packet.docLenUtf16),
            window: NSRange(location: Int(packet.windowStart), length: Int(packet.windowLen)),
            inactive: packet.inactive, moreInWindow: packet.moreInWindow,
            dirtyElsewhere: packet.dirtyElsewhere, ranges: ranges, runs: runs, fonts: fonts)
    }

    /// Lift an outline packet. Statements and issues must sit inside the document;
    /// every fold must hide at least one line.
    static func convert(_ packet: EditorOutline, length: Int) throws -> EditorOutlineData {
        // Statements are (start, end) pairs, not (start, len): a length comes from them.
        guard packet.statements.count % 2 == 0 else { throw EditorAnalysisError.malformed }
        let statements = stride(from: 0, to: packet.statements.count, by: 2).map {
            NSRange(location: Int(packet.statements[$0]),
                    length: Int(packet.statements[$0 + 1]) - Int(packet.statements[$0]))
        }
        for range in statements where NSMaxRange(range) > length {
            throw EditorAnalysisError.malformed
        }
        let folds = try packet.folds.map { fold -> EditorFoldData in
            let kind: EditorFoldKind
            switch fold.kind {
            case .statement: kind = .statement
            case .cte: kind = .cte
            case .subquery: kind = .subquery
            case .body: kind = .body
            }
            guard fold.lastLine > fold.headerLine else { throw EditorAnalysisError.malformed }
            return EditorFoldData(kind: kind, headerLine: Int(fold.headerLine),
                                      lastLine: Int(fold.lastLine), header: Int(fold.header),
                                      bodyStart: Int(fold.bodyStart), bodyEnd: Int(fold.bodyEnd),
                                      summary: fold.summary)
        }
        let issues = try packet.issues.map { issue -> EditorIssueData in
            let kind: EditorIssueKind
            switch issue.kind {
            case .unclosedQuote: kind = .unclosedQuote
            case .unclosedIdentifier: kind = .unclosedIdentifier
            case .unclosedComment: kind = .unclosedComment
            case .unclosedDollar: kind = .unclosedDollar
            case .unbalancedParen: kind = .unbalancedParen
            case .syntaxError: kind = .syntaxError
            case .missingToken: kind = .missingToken
            }
            let range = NSRange(location: Int(issue.start), length: Int(issue.len))
            guard NSMaxRange(range) <= length else { throw EditorAnalysisError.malformed }
            return EditorIssueData(kind: kind, range: range)
        }
        return EditorOutlineData(revision: packet.revision, statements: statements,
                                 folds: folds, issues: issues)
    }

    static func shouldApply(revision: UInt64, paint: EditorPaintData, hasMarkedText: Bool,
                            storageLength: Int) -> Bool {
        revision == paint.revision && !hasMarkedText && storageLength == paint.length
    }

    /// Widen a range past surrogate boundaries, so an edit never splits a character.
    /// Both ends move outward only.
    static func widened(_ range: NSRange, in text: NSString) -> NSRange {
        var location = range.location
        var end = NSMaxRange(range)
        let length = text.length
        func isLowSurrogate(at index: Int) -> Bool {
            guard index >= 0, index < length else { return false }
            let unit = text.character(at: index)
            return unit >= 0xDC00 && unit <= 0xDFFF
        }
        if isLowSurrogate(at: location) { location -= 1 }
        if isLowSurrogate(at: end) { end += 1 }
        return NSRange(location: max(location, 0), length: max(end - max(location, 0), 0))
    }

    /// Paint a packet onto a text view. Colours go through temporary `.foregroundColor`
    /// attributes only — never `setTemporaryAttributes`, so the find highlight (a
    /// temporary `.backgroundColor`) survives. Comment italics go to storage, because a
    /// temporary attribute cannot carry a font.
    func apply(_ paint: EditorPaintData, to textView: NSTextView,
               colors: [EditorColorClass: NSColor], regularFont: NSFont, italicFont: NSFont) {
        guard let storage = textView.textStorage,
              let layoutManager = textView.layoutManager else { return }
        let length = storage.length
        for range in paint.ranges where NSMaxRange(range) <= length {
            layoutManager.removeTemporaryAttribute(.foregroundColor, forCharacterRange: range)
        }
        for run in paint.runs where NSMaxRange(run.range) <= length {
            if let color = colors[run.colorClass] {
                layoutManager.addTemporaryAttribute(.foregroundColor, value: color,
                                                   forCharacterRange: run.range)
            }
        }
        let fonts = paint.fonts.filter { NSMaxRange($0.range) <= length }
        guard !fonts.isEmpty else { return }
        storage.beginEditing()
        for font in fonts {
            storage.addAttribute(.font, value: font.italic ? italicFont : regularFont,
                                 range: font.range)
        }
        storage.endEditing()
    }

    /// A synchronous full paint plus outline, for tests: converge first so colours,
    /// folds and issues match a fresh document.
    func drainForTesting() throws -> (paint: EditorPaintData, outline: EditorOutlineData) {
        try converge()
        let paint = try paint(window: NSRange(location: 0, length: length), budget: length)
        return (paint, try outline())
    }
}

/// The ceiling: above this many UTF-16 units a document gets no colour or folds.
func editorCeiling() throws -> Int {
    try mapEditorError { try Int(editorCeilingUtf16()) }
}

/// The statements of `sql` for Run, as ranges without their `;`.
func editorStatementRanges(_ sql: String, dialect: EditorDialect = .generic) throws -> [NSRange] {
    let flat: [UInt32] = try mapEditorError { try sqlStatementRanges(sql: sql, dialect: dialect) }
    guard flat.count % 2 == 0 else { throw EditorAnalysisError.malformed }
    return stride(from: 0, to: flat.count, by: 2).map {
        NSRange(location: Int(flat[$0]), length: Int(flat[$0 + 1]) - Int(flat[$0]))
    }
}

private func mapEditorError<T>(_ call: () throws -> T) throws -> T {
    do {
        return try call()
    } catch let error as EditorError {
        switch error {
        case .Stale: throw EditorAnalysisError.stale
        case .OutOfBounds: throw EditorAnalysisError.outOfBounds
        case .SplitsCharacter: throw EditorAnalysisError.splitsCharacter
        case .TooLarge: throw EditorAnalysisError.tooLarge
        case .Malformed: throw EditorAnalysisError.malformed
        }
    } catch {
        throw EditorAnalysisError.malformed
    }
}
