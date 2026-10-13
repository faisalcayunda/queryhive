import Foundation
import QueryHiveFFI

/// What the caret sits in, read from the start of the document: code, text that is not SQL (a
/// string, a comment, a `$$` body), or a quoted name that has been opened and not closed yet.
///
/// One forward pass, and the same pass finds where the caret's statement starts (after the last `;`
/// that was code), which is the head the caret-scope reader needs. The rules follow the dialect the
/// editor lexes under: MySQL has `#` comments and backslash escapes in strings, and reads `"` as a
/// string, not a name; PostgreSQL has `$tag$` bodies.
///
/// The editor used to answer this with a count of apostrophes, which an apostrophe in a comment
/// (`-- don't`) turned into "inside a string" for the rest of the file, with suggestions off.
struct SQLCaretState: Equatable {
    enum Context: Equatable {
        case code
        /// A string literal, a comment or a dollar-quoted body: nothing to complete.
        case text
        /// A quoted name opened at this offset (the quote itself) and not closed before the caret.
        case quotedName(open: Int)
    }

    let context: Context
    /// Where the statement holding the caret starts: 0, or the offset after the last `;` in code.
    let statementStart: Int

    var isText: Bool { context == .text }

    static func scan(_ text: NSString, upTo caret: Int, dialect: EditorDialect) -> SQLCaretState {
        let end = min(max(caret, 0), text.length)
        guard end > 0 else { return SQLCaretState(context: .code, statementStart: 0) }
        // One bulk copy and a plain loop: reading a unit at a time through an `NSString` is a call
        // per character, and this runs on every pause in typing.
        var units = [unichar](repeating: 0, count: end)
        text.getCharacters(&units, range: NSRange(location: 0, length: end))
        let mysql = dialect == .mysql
        // ponytail: the whole text before the caret on every pause; a cached checkpoint per
        // statement if a very large script ever makes this show in the `type-10k` bench.
        var statementStart = 0
        var index = 0
        while index < end {
            let unit = units[index]
            switch unit {
            case 0x3B:                                           // ;
                statementStart = index + 1
                index += 1
            case 0x2D where index + 1 < end && units[index + 1] == 0x2D
                    && (!mysql || Self.endsDashes(at: index, in: text)),   // --
                 0x23 where mysql:                                         // # (MySQL)
                var stop = index
                while stop < end, units[stop] != 0x0A { stop += 1 }
                if stop >= end { return SQLCaretState(context: .text, statementStart: statementStart) }
                index = stop + 1
            case 0x2F where index + 1 < end && units[index + 1] == 0x2A:   // /*
                // PostgreSQL nests block comments; the others end at the first `*/`.
                var depth = 1
                var stop = index + 2
                while stop + 1 < end, depth > 0 {
                    if units[stop] == 0x2A, units[stop + 1] == 0x2F {
                        depth -= 1
                        stop += 2
                    } else if dialect == .postgres, units[stop] == 0x2F, units[stop + 1] == 0x2A {
                        depth += 1
                        stop += 2
                    } else {
                        stop += 1
                    }
                }
                if depth > 0 { return SQLCaretState(context: .text, statementStart: statementStart) }
                index = stop
            case 0x27, 0x22 where mysql:                          // ' (and " as a string in MySQL)
                // PostgreSQL's `E'...'` takes a backslash as an escape; MySQL's strings always do.
                let escapes = mysql || (dialect == .postgres && index > 0
                    && (units[index - 1] | 0x20) == 0x65
                    && (index < 2 || !isTagBody(units[index - 2])))
                guard let after = closing(of: unit, from: index + 1, in: units, end: end,
                                          backslash: escapes) else {
                    return SQLCaretState(context: .text, statementStart: statementStart)
                }
                index = after
            case 0x22, 0x60:                                      // " and ` as quoted names
                guard let after = closing(of: unit, from: index + 1, in: units, end: end,
                                          backslash: false) else {
                    return SQLCaretState(context: .quotedName(open: index), statementStart: statementStart)
                }
                index = after
            case 0x24 where dialect == .postgres || dialect == .generic:   // $tag$ ... $tag$
                guard let tag = dollarTag(at: index, in: units, end: end) else {
                    index += 1
                    continue
                }
                guard let after = find(tag, from: index + tag.count, in: units, end: end) else {
                    return SQLCaretState(context: .text, statementStart: statementStart)
                }
                index = after
            default:
                index += 1
            }
        }
        return SQLCaretState(context: .code, statementStart: statementStart)
    }

    /// MySQL reads `--` as a comment only when a blank or a control character follows it (or the
    /// text ends), so `1--1` is arithmetic. The text after the caret decides, which is why this
    /// looks at the whole text and not at the units copied.
    private static func endsDashes(at index: Int, in text: NSString) -> Bool {
        guard index + 2 < text.length else { return true }
        return text.character(at: index + 2) <= 0x20
    }

    /// The index just past the quote that closes a run opened before `from`, or nil when the run is
    /// still open at `end`. A doubled quote is an escaped one, and a backslash escapes in MySQL.
    private static func closing(of quote: unichar, from: Int, in units: [unichar], end: Int,
                                backslash: Bool) -> Int? {
        var index = from
        while index < end {
            if backslash, units[index] == 0x5C { index += 2; continue }
            if units[index] == quote {
                if index + 1 < end, units[index + 1] == quote { index += 2; continue }
                return index + 1
            }
            index += 1
        }
        return nil
    }

    /// `$tag$` including both dollars; `$1` is a placeholder, not a tag.
    private static func dollarTag(at index: Int, in units: [unichar], end: Int) -> [unichar]? {
        var stop = index + 1
        while stop < end, isTagBody(units[stop]) { stop += 1 }
        guard stop < end, units[stop] == 0x24 else { return nil }
        if stop > index + 1, units[index + 1] >= 0x30, units[index + 1] <= 0x39 { return nil }
        return Array(units[index...stop])
    }

    private static func isTagBody(_ unit: unichar) -> Bool {
        (unit >= 0x41 && unit <= 0x5A) || (unit >= 0x61 && unit <= 0x7A) || unit == 0x5F
            || (unit >= 0x30 && unit <= 0x39)
    }

    private static func find(_ pattern: [unichar], from: Int, in units: [unichar], end: Int) -> Int? {
        guard from + pattern.count <= end else { return nil }
        var index = from
        outer: while index + pattern.count <= end {
            for offset in 0..<pattern.count where units[index + offset] != pattern[offset] {
                index += 1
                continue outer
            }
            return index + pattern.count
        }
        return nil
    }
}
