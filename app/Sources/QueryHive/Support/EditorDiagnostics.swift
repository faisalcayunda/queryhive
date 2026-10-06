import AppKit
import SwiftUI

// W10-T6b (blueprint w10 §8.2 to §8.5): what the editor underlines, lists in its rotor and counts in
// its corner. Two sources feed it: the lexical issues the analysis finds in the text (an unclosed
// quote, comment or parenthesis) and the position a server gave for a failed Run. Syntax errors
// from the tree-sitter parse are left out on purpose (P-28): a half-typed query is not an error.

// MARK: What was sent

/// The text a Run actually sent to the server, and where it starts in the document.
///
/// A server error position is an offset into the text it was sent, and that is not always the
/// document: Run sends the selection, or the statement under the caret, or the whole document when
/// nothing is selected. `documentStart` is where the first UTF-16 unit of `text` sits in
/// `QueryTab.sql`, measured after the empty-selection fallback and after the trimming.
struct SentSQL: Equatable {
    let text: String
    let documentStart: Int
}

/// Where the server said a failed Run went wrong, tied to the exact text that was sent.
///
/// It is shown only while the document still holds `sqlSnapshot`: any edit drops it (`QueryTab.sql`
/// does), because after an edit the offset describes text that is gone.
struct ServerErrorMark: Equatable, Identifiable {
    let id = UUID()
    /// `QueryTab.sql` when the Run started.
    let sqlSnapshot: String
    let sent: SentSQL
    /// The server's 1-based offset, in Unicode scalars, into `sent.text`.
    let scalarOffset: Int
    let message: String

    /// The longest underline: a position names a token, and a token is short.
    static let longestUnderline = 64

    /// The UTF-16 range to underline in `text`, which must be `sqlSnapshot`. The word the position
    /// falls on (letters, digits and underscores), at least one unit and at most 64, kept inside the
    /// sent text and its line. `nil` when the offset lies outside the sent text.
    ///
    /// A position at the end of the sent text ("syntax error at end of input") underlines the last
    /// word instead, since there is nothing after it to underline. A MySQL position is the
    /// start of a line, so the blanks that indent it are stepped over.
    func range(in text: NSString) -> NSRange? {
        guard scalarOffset >= 1, sent.documentStart >= 0 else { return nil }
        let sentUnits = (sent.text as NSString).length
        let limit = sent.documentStart + sentUnits
        guard sentUnits > 0, limit <= text.length else { return nil }
        // The scalar offset to a UTF-16 offset inside the sent text. A surrogate pair is one scalar.
        var units = 0
        var scalars = 0
        for scalar in sent.text.unicodeScalars {
            if scalars == scalarOffset - 1 { break }
            units += scalar.value > 0xFFFF ? 2 : 1
            scalars += 1
        }
        guard scalars == scalarOffset - 1 else { return nil }
        var start = sent.documentStart + units
        func character(_ index: Int) -> unichar { text.character(at: index) }
        if start >= limit {
            // One past the end is "at end of input"; anything further is outside.
            guard start == limit else { return nil }
            var last = limit
            while last > sent.documentStart, Self.isBlank(character(last - 1)) { last -= 1 }
            guard last > sent.documentStart else { return nil }
            start = last - 1
            if Self.isWord(character(start)) {
                while start > sent.documentStart, last - start < Self.longestUnderline,
                      Self.isWord(character(start - 1)) { start -= 1 }
                return NSRange(location: start, length: last - start)
            }
            return text.rangeOfComposedCharacterSequence(at: start)
        }
        while start < limit, character(start) == 0x20 || character(start) == 0x09 { start += 1 }
        guard start < limit, character(start) != 0x0A, character(start) != 0x0D else { return nil }
        var end = start
        while end < limit, end - start < Self.longestUnderline, Self.isWord(character(end)) { end += 1 }
        if end == start {
            // Punctuation, or an emoji: one whole character.
            end = min(NSMaxRange(text.rangeOfComposedCharacterSequence(at: start)), limit)
        }
        return NSRange(location: start, length: end - start)
    }

    private static func isBlank(_ unit: unichar) -> Bool {
        unit == 0x20 || unit == 0x09 || unit == 0x0A || unit == 0x0D
    }

    private static func isWord(_ unit: unichar) -> Bool {
        guard let scalar = Unicode.Scalar(unit) else { return false }
        return unit == 0x5F || CharacterSet.alphanumerics.contains(scalar)
    }
}

// MARK: Diagnostics

/// One thing the editor underlines: where, what it is, and the words that describe it.
struct EditorDiagnostic: Equatable {
    enum Kind: Equatable {
        case lexical(EditorIssueKind)
        case server
    }

    let kind: Kind
    let range: NSRange
    let message: String

    var isServer: Bool { kind == .server }
}

enum EditorDiagnostics {
    /// The lexical issues worth showing, merged with the server's, in document order.
    ///
    /// `syntaxError` and `missingToken` are dropped (P-28): the tree-sitter parse of a half-typed
    /// query is wrong most of the time, and an underline that is wrong most of the time is noise.
    /// Anything outside `text` is dropped too, so a stale outline cannot underline past the end.
    static func merge(issues: [EditorIssueData], server: EditorDiagnostic?,
                      text: NSString) -> [EditorDiagnostic] {
        var merged: [EditorDiagnostic] = []
        for issue in issues {
            guard let message = DiagnosticLabels.title(issue.kind), issue.range.length > 0,
                  NSMaxRange(issue.range) <= text.length else { continue }
            merged.append(EditorDiagnostic(kind: .lexical(issue.kind), range: issue.range, message: message))
        }
        if let server, server.range.length > 0, NSMaxRange(server.range) <= text.length {
            merged.append(server)
        }
        // Stable: a server error at the same place as a lexical issue keeps its place after it.
        return merged.enumerated().sorted {
            ($0.element.range.location, $0.offset) < ($1.element.range.location, $1.offset)
        }.map(\.element)
    }
}

/// The words for a diagnostic, in the rotor and nowhere else (the underline itself is not read by
/// VoiceOver).
enum DiagnosticLabels {
    /// What a lexical issue is called; `nil` for the kinds that are never shown.
    static func title(_ kind: EditorIssueKind) -> String? {
        switch kind {
        case .unclosedQuote: "Unclosed quote"
        case .unclosedIdentifier: "Unclosed quoted identifier"
        case .unclosedComment: "Unclosed comment"
        case .unclosedDollar: "Unclosed dollar quote"
        case .unbalancedParen: "Unbalanced parenthesis"
        case .syntaxError, .missingToken: nil
        }
    }

    /// The server's message is kept to this many characters in a label.
    static let serverMessageLimit = 80

    /// `line` is 1-based.
    static func label(for diagnostic: EditorDiagnostic, line: Int) -> String {
        switch diagnostic.kind {
        case .lexical:
            return "\(diagnostic.message), line \(line)"
        case .server:
            let flat = diagnostic.message.split(whereSeparator: \.isNewline).joined(separator: " ")
            let short = flat.count > serverMessageLimit ? String(flat.prefix(serverMessageLimit)) + "…" : flat
            return "Server error, line \(line): \(short)"
        }
    }
}

/// How many issues the corner readout reports, and whether one of them is the server's.
struct EditorIssueSummary: Equatable {
    var count = 0
    var hasServerError = false

    init() {}

    init(_ diagnostics: [EditorDiagnostic]) {
        count = diagnostics.count
        hasServerError = diagnostics.contains(where: \.isServer)
    }
}

/// The corner readout's words (blueprint §8.5). Server errors are already announced by the banner;
/// lexical issues are not announced while typing, so this is how they are reached besides the rotor.
enum EditorReadout {
    /// "2 issues", after the line count and its own "·" and glyph; empty when there is nothing to report.
    static func issues(_ summary: EditorIssueSummary) -> String {
        summary.count == 0 ? "" : pluralized(summary.count, "issue")
    }

    static func accessibilityLabel(_ summary: EditorIssueSummary) -> String {
        "\(summary.count.formatted()) query \(summary.count == 1 ? "issue" : "issues")"
    }

    static func tint(_ summary: EditorIssueSummary) -> Color {
        summary.hasServerError ? Tone.markCoral : Tone.markAmber
    }
}

// MARK: Painting

/// Lays the diagnostics onto a layout manager as temporary `.underlineStyle` and `.underlineColor`.
///
/// Those two keys are this painter's alone (blueprint 4B §7.4): it never touches `.foregroundColor`,
/// which the syntax paint owns, or `.backgroundColor`, which the find highlight owns. Lexical issues
/// are dashed amber, a server error is a thick coral line. The colours are resolved per appearance,
/// so a theme switch does not leave a snapshot of the old one behind.
///
/// Temporary attributes are not read by VoiceOver, so the rotor and the readout are the way these
/// are reached without sight.
final class DiagnosticsPainter {
    static let lexicalStyle = NSUnderlineStyle.single.union(.patternDash).rawValue
    static let serverStyle = NSUnderlineStyle.thick.rawValue

    /// Dynamic colours, one instance each: identity is how a test tells which one was applied.
    static let amber = dynamic(Tone.markAmber)
    static let coral = dynamic(Tone.markCoral)

    private static func dynamic(_ color: Color) -> NSColor {
        NSColor(name: nil) { appearance in
            var resolved = NSColor.clear
            appearance.performAsCurrentDrawingAppearance { resolved = NSColor(color) }
            return resolved
        }
    }

    private(set) var painted: [EditorDiagnostic] = []
    private var paintedLength = -1

    /// Paint `diagnostics`. Nothing is written when they are what is already painted for a text of
    /// this length, which is the common case on a pause in typing.
    func apply(_ diagnostics: [EditorDiagnostic], to layoutManager: NSLayoutManager, length: Int) {
        let wanted = diagnostics.filter { $0.range.length > 0 && NSMaxRange($0.range) <= length }
        guard wanted != painted || length != paintedLength else { return }
        // The whole document, not the old ranges: an edit since the last paint moved them, and a
        // removal over where they used to be would leave a line behind.
        if !painted.isEmpty { Self.clearAttributes(in: layoutManager, length: length) }
        for diagnostic in wanted {
            layoutManager.addTemporaryAttributes(
                [.underlineStyle: diagnostic.isServer ? Self.serverStyle : Self.lexicalStyle,
                 .underlineColor: diagnostic.isServer ? Self.coral : Self.amber],
                forCharacterRange: diagnostic.range)
        }
        painted = wanted
        paintedLength = length
    }

    /// Forget what was painted and remove it: for a text that was replaced wholesale.
    func reset(_ layoutManager: NSLayoutManager, length: Int) {
        Self.clearAttributes(in: layoutManager, length: length)
        painted = []
        paintedLength = -1
    }

    private static func clearAttributes(in layoutManager: NSLayoutManager, length: Int) {
        let whole = NSRange(location: 0, length: length)
        layoutManager.removeTemporaryAttribute(.underlineStyle, forCharacterRange: whole)
        layoutManager.removeTemporaryAttribute(.underlineColor, forCharacterRange: whole)
    }
}

// MARK: Rotor

/// The diagnostics as rotor stops: "Unclosed quote, line 3", "Server error, line 1: syntax error…".
struct IssuesRotorSource: EditorRotorSource, Equatable {
    let title = "Query issues"
    var items: [EditorRotorItem]

    init(diagnostics: [EditorDiagnostic], text: NSString) {
        guard !diagnostics.isEmpty else { items = []; return }
        let starts = SQLFolding.lineStarts(in: text)
        items = diagnostics.map {
            let line = SQLFolding.line(containing: $0.range.location, lineStarts: starts) + 1
            return EditorRotorItem(label: DiagnosticLabels.label(for: $0, line: line),
                                   offset: $0.range.location, length: $0.range.length)
        }
    }
}

enum EditorRotorSearch {
    /// The stop after (or before) `reference`. With a current item the search starts strictly past
    /// it; without one it starts at the caret and includes a stop that begins there. `nil` at the
    /// ends: a rotor does not wrap.
    static func item(in items: [EditorRotorItem], from current: Int?, caret: Int,
                     forward: Bool) -> EditorRotorItem? {
        let sorted = items.sorted { $0.offset < $1.offset }
        if forward {
            return sorted.first { item in current.map { item.offset > $0 } ?? (item.offset >= caret) }
        }
        return sorted.last { item in current.map { item.offset < $0 } ?? (item.offset <= caret) }
    }
}

/// Answers VoiceOver's rotor searches from the editor's latest items. The editor owns the delegates
/// (a rotor holds its delegate weakly) and swaps the items it reads with every outline.
final class EditorRotorDelegate: NSObject, NSAccessibilityCustomRotorItemSearchDelegate {
    private let title: String
    private weak var textView: SQLTextView?

    init(title: String, textView: SQLTextView) {
        self.title = title
        self.textView = textView
    }

    func rotor(_ rotor: NSAccessibilityCustomRotor,
               resultFor parameters: NSAccessibilityCustomRotor.SearchParameters)
        -> NSAccessibilityCustomRotor.ItemResult? {
        guard let textView, let source = textView.rotorSources.first(where: { $0.title == title }) else {
            return nil
        }
        let current = parameters.currentItem.flatMap { item -> Int? in
            item.targetRange.location == NSNotFound ? nil : item.targetRange.location
        }
        guard let found = EditorRotorSearch.item(
            in: source.items, from: current, caret: textView.selectedRange().location,
            forward: parameters.searchDirection == .next) else { return nil }
        let result = NSAccessibilityCustomRotor.ItemResult(targetElement: textView)
        let length = (textView.string as NSString).length
        let start = min(found.offset, length)
        result.targetRange = NSRange(location: start, length: min(max(found.length, 0), length - start))
        result.customLabel = found.label
        return result
    }
}
