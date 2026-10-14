import Foundation
import QueryHiveFFI

/// What Format SQL does to a text, decided without a text view: the engine's formatter
/// (`formatSql`, blueprint W12 §3) on the whole text, or on the statements a selection touches.
///
/// The engine formats everything it is given and only maps the selection through it, so the
/// "statements the selection touches" cut (D-6) is made here: the slice runs from the first such
/// statement's start to the last one's end, which is what keeps a selection that begins inside a
/// literal from handing the formatter half a string. Whatever lies outside the slice, the `;`
/// after it included, is not touched.
///
/// Formatting is pure text: nothing here talks to a server.
enum SQLFormat {
    /// What the formatter would do.
    enum Outcome: Equatable {
        /// Replace `range` (UTF-16, of the original text) with `replacement`, then select `selection`.
        case edit(range: NSRange, replacement: String, selection: NSRange)
        /// The text is already formatted.
        case unchanged
        /// Nothing to format: a blank text, or a selection that touches no statement.
        case nothing
        /// The formatter would not touch the text; the reason is ready to show.
        case refused(String)
    }

    /// The options the editor's preferences ask for: the editor's tab width as the indent, and
    /// keywords in upper case (the engine leaves MySQL's alone, where case can change meaning).
    static func options(tabWidth: Int = EditorPreferences.shared.tabWidth) -> FormatOptionsWire {
        FormatOptionsWire(indentWidth: UInt8(min(max(tabWidth, 1), 8)), keywordCase: .upper)
    }

    static func plan(text: String, selection: NSRange, dialect: EditorDialect,
                     options: FormatOptionsWire = options()) -> Outcome {
        let ns = text as NSString
        guard ns.length > 0, text.contains(where: { !$0.isWhitespace }) else { return .nothing }
        let whole = NSRange(location: 0, length: ns.length)
        let location = min(max(selection.location, 0), ns.length)
        let selection = NSRange(location: location, length: min(max(selection.length, 0), ns.length - location))

        let slice: NSRange
        if selection.length == 0 {
            slice = whole
        } else {
            switch statements(touching: selection, in: text, dialect: dialect) {
            case .unreadable(let error): return .refused(error)
            case .none: return .nothing
            case .span(let span): slice = span
            }
        }

        let part = slice == whole ? text : ns.substring(with: slice)
        // The selection the engine maps is the part of it inside the slice.
        let inStart = min(max(selection.location, slice.location), NSMaxRange(slice))
        let inEnd = min(max(NSMaxRange(selection), inStart), NSMaxRange(slice))
        let edit: EditorTextEdit
        do {
            edit = try formatSql(sql: part, dialect: dialect, options: options,
                                 selectionStart: UInt32(inStart - slice.location),
                                 selectionLen: UInt32(inEnd - inStart))
        } catch let error as FormatFfiError {
            let firstLine = 1 + lineBreaks(in: ns, before: slice.location)
            return .refused(reason(for: error, firstLine: firstLine))
        } catch {
            return .refused("Couldn't format the SQL. Nothing was changed.")
        }
        guard edit.len > 0 || !edit.replacement.isEmpty else { return .unchanged }

        let range = NSRange(location: slice.location + Int(edit.start), length: Int(edit.len))
        let delta = (edit.replacement as NSString).length - Int(edit.len)
        // Ends outside the slice keep their place in the text around it.
        let start = selection.location < slice.location ? selection.location
            : slice.location + Int(edit.selectionStart)
        let end = NSMaxRange(selection) > NSMaxRange(slice) ? NSMaxRange(selection) + delta
            : slice.location + Int(edit.selectionStart) + Int(edit.selectionLen)
        return .edit(range: range, replacement: edit.replacement,
                     selection: NSRange(location: start, length: max(0, end - start)))
    }

    private enum Touched { case span(NSRange), none, unreadable(String) }

    /// The span from the first statement the selection touches to the end of the last.
    private static func statements(touching selection: NSRange, in text: String,
                                   dialect: EditorDialect) -> Touched {
        let ranges: [NSRange]
        do {
            ranges = try editorStatementRanges(text, dialect: dialect)
        } catch EditorAnalysisError.tooLarge {
            return .unreadable(tooLarge)
        } catch {
            return .unreadable("Couldn't format: the statements in the selection could not be read. Nothing was changed.")
        }
        // A statement's range starts right after the `;` before it, so it carries the line breaks
        // between the two; those are not the statement's, and formatting them would pull it up
        // onto the previous line.
        let ns = text as NSString
        let touched = ranges.map { trimmed($0, in: ns) }
            .filter { $0.length > 0 && NSIntersectionRange($0, selection).length > 0 }
        guard let first = touched.first, let last = touched.last else { return .none }
        return .span(NSRange(location: first.location, length: NSMaxRange(last) - first.location))
    }

    private static func trimmed(_ range: NSRange, in text: NSString) -> NSRange {
        func blank(_ index: Int) -> Bool {
            guard let scalar = Unicode.Scalar(text.character(at: index)) else { return false }
            return CharacterSet.whitespacesAndNewlines.contains(scalar)
        }
        var start = range.location, end = NSMaxRange(range)
        while start < end, blank(start) { start += 1 }
        while end > start, blank(end - 1) { end -= 1 }
        return NSRange(location: start, length: end - start)
    }

    private static func lineBreaks(in text: NSString, before offset: Int) -> Int {
        var count = 0
        var index = 0
        while index < offset {
            if text.character(at: index) == 0x0A { count += 1 }
            index += 1
        }
        return count
    }

    /// What the notice says. Every refusal ends the same way, because the point of it is that the
    /// text the person wrote is still the text in the editor.
    static func reason(for error: FormatFfiError, firstLine: Int = 1) -> String {
        let unchanged = "Nothing was changed."
        switch error {
        case .Ambiguous(let line):
            return "Couldn't format: line \(Int(line) + firstLine - 1) reads differently under different "
                + "server settings (a backslash in a string, or a comment form). \(unchanged)"
        case .Unterminated(let line):
            return "Couldn't format: the quote or comment that opens on line \(Int(line) + firstLine - 1) "
                + "is never closed. \(unchanged)"
        case .TooLarge:
            return tooLarge
        case .SelfCheck:
            return "Couldn't format: the result failed the formatter's own check. \(unchanged)"
        case .OutOfBounds, .SplitsCharacter:
            return "Couldn't format this selection. \(unchanged)"
        }
    }

    private static let tooLarge = "Couldn't format: the SQL is larger than the formatter's 4 MB limit. Nothing was changed."
}
