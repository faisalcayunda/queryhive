import AppKit

/// One multi-line region the editor can collapse.
///
/// Lines are the unit, not characters. The hidden body starts on the line after the header and ends
/// with the last hidden line's newline, so a fold never leaves half a paragraph behind — and a
/// partial paragraph cannot take a collapsed line height, which is the whole mechanism the editor
/// uses to hide it. Everything here is offsets into the text; nothing holds a copy of it.
struct FoldRegion: Equatable {
    enum Kind: Equatable {
        /// A whole statement with more than one line.
        case statement
        /// The parenthesised body of a CTE inside a `WITH`.
        case cte
    }

    let kind: Kind
    /// 0-based line that stays visible.
    let headerLine: Int
    /// 0-based last line hidden while folded.
    let lastLine: Int
    /// UTF-16 offset of the header line's first character. This is the fold's identity: it is what
    /// the editor remembers as folded across a re-scan, and what a gutter click hands back.
    let header: Int
    /// UTF-16 offsets of the hidden body. Always whole lines.
    let bodyStart: Int
    let bodyEnd: Int
    /// The leading keyword, or `CTE`, for the marker's tooltip.
    let summary: String

    var body: NSRange { NSRange(location: bodyStart, length: bodyEnd - bodyStart) }
}

/// Where the editor may fold, and how folded-header offsets survive an edit.
///
/// Pure: it reads a string and returns ranges and offsets, with no text view and no window. The
/// editor owns the actual hiding (`SQLFoldStyler`), which is why this file can be tested directly.
enum SQLFolding {
    /// The largest document this will fold, in UTF-16 units.
    ///
    /// Above it, `regions` answers nothing and the editor shows no fold marks — the text is left
    /// alone, which is the same thing TablePro does above its own limit. The reason this one is
    /// deliberately **lower** than TablePro's 2,000,000: this scanner locates each statement by
    /// re-running the statement scanner and searching for its text, which is not a single pass yet
    /// (plan §12.2.3), so an unbounded document would pay a superlinear cost on every keystroke.
    /// The limit rises when the boundary list comes from `qh-sql`, which already scans once.
    static let foldingSizeLimit = 200_000

    // MARK: Lines

    /// UTF-16 offset of each line's first character.
    ///
    /// The start of the line after the last newline is included, so a text ending in `\n` reports a
    /// final empty line — which is where the caret is, and which the gutter also numbers.
    static func lineStarts(in text: NSString) -> [Int] {
        var starts = [0]
        var index = 0
        while index < text.length {
            if text.character(at: index) == 0x0A { starts.append(index + 1) }
            index += 1
        }
        return starts
    }

    /// The line an offset falls on. Binary search, since the gutter asks this on every visible line.
    static func line(containing offset: Int, lineStarts: [Int]) -> Int {
        var low = 0
        var high = lineStarts.count - 1
        var answer = 0
        while low <= high {
            let mid = (low + high) / 2
            if lineStarts[mid] <= offset {
                answer = mid
                low = mid + 1
            } else {
                high = mid - 1
            }
        }
        return answer
    }

    // MARK: Regions

    /// Every foldable region in the text, outermost/earliest first.
    ///
    /// Statements come from the same scanner "Run Current Statement" uses, so a fold and a run
    /// cannot disagree about where a statement is. CTE bodies are found separately, because the
    /// statement scanner answers "which statement", not "which parentheses".
    static func regions(in sql: String) -> [FoldRegion] {
        let text = sql as NSString
        // Over the limit the editor gets no regions at all, so no fold marks and no cost. The
        // alternative — folding part of a document — would be a fold whose body is cut short.
        guard text.length <= foldingSizeLimit else { return [] }
        let starts = lineStarts(in: text)
        var regions: [FoldRegion] = []

        // One call, shared by the statements and the CTEs below. It is the expensive part, and
        // asking twice was the previous shape.
        let statements = statementRanges(in: sql)

        for range in statements {
            let headerLine = line(containing: range.location, lineStarts: starts)
            let lastLine = line(containing: max(range.location, NSMaxRange(range) - 1), lineStarts: starts)
            guard lastLine > headerLine else { continue }
            regions.append(FoldRegion(
                kind: .statement,
                headerLine: headerLine,
                lastLine: lastLine,
                header: starts[headerLine],
                bodyStart: starts[headerLine + 1],
                bodyEnd: lineEnd(lastLine, starts: starts, length: text.length),
                summary: leadingKeyword(text, from: range.location) ?? "statement"))
        }

        // A fold is identified by its header offset, so two regions must never share one. A CTE
        // whose `(` sits on the statement's own first line — `WITH x AS (` — has exactly the same
        // header as the statement, and the statement's fold already hides everything the CTE's
        // would. Keeping only the first there is what makes the gutter show one marker per line
        // instead of two markers on top of each other that fold different amounts.
        var headers = Set(regions.map(\.header))
        for region in cteRegions(in: sql, statements: statements, text: text, starts: starts) where !headers.contains(region.header) {
            regions.append(region)
            headers.insert(region.header)
        }
        return regions.sorted { ($0.header, $0.lastLine) < ($1.header, $1.lastLine) }
    }

    /// The scanner's statement boundaries as character ranges.
    ///
    /// It reuses `sqlStatement(in:atUTF16Offset:)` rather than re-implementing the literal and
    /// comment rules: the scanner is asked at the start of each statement, and the answer is located
    /// forward from the cursor, which also settles two identical statements in a row. The cursor
    /// only ever moves forward, so the loop terminates even on malformed SQL.
    static func statementRanges(in sql: String) -> [NSRange] {
        let text = sql as NSString
        let length = text.length
        var ranges: [NSRange] = []
        var cursor = 0

        while cursor < length {
            while cursor < length, isTrivia(text.character(at: cursor)) { cursor += 1 }
            guard cursor < length else { break }
            guard let statement = sqlStatement(in: sql, atUTF16Offset: cursor) else { break }
            let search = NSRange(location: cursor, length: length - cursor)
            let found = text.range(of: statement, options: [.literal], range: search)
            guard found.location != NSNotFound, found.length > 0 else { break }
            ranges.append(found)
            cursor = NSMaxRange(found)
        }
        return ranges
    }

    // MARK: CTEs

    /// CTE bodies: an `AS (` inside a statement that also holds a `WITH`.
    ///
    /// Deliberately not a full `WITH` grammar. A real parser would track the CTE list; this tracks
    /// the one shape worth folding — `AS (` with a matching `)` — and gates it on a `WITH` word in
    /// the same statement. That gate is what keeps `CREATE TABLE ... AS (SELECT ...)` out, since a
    /// CTAS has no `WITH` and folding its body would be an odd thing to offer.
    static func cteRegions(in sql: String, statements: [NSRange], text: NSString,
                           starts: [Int]) -> [FoldRegion] {
        var out: [FoldRegion] = []
        for range in statements {
            let statement = text.substring(with: range)
            let ns = statement as NSString
            guard containsWord("WITH", in: ns) else { continue }
            for open in cteOpenParens(in: ns) {
                guard let close = matchingParen(in: ns, from: open) else { continue }
                let headerLine = line(containing: range.location + open, lineStarts: starts)
                let lastLine = line(containing: range.location + close, lineStarts: starts)
                guard lastLine > headerLine else { continue }
                out.append(FoldRegion(
                    kind: .cte,
                    headerLine: headerLine,
                    lastLine: lastLine,
                    header: starts[headerLine],
                    bodyStart: starts[headerLine + 1],
                    bodyEnd: lineEnd(lastLine, starts: starts, length: text.length),
                    summary: "CTE"))
            }
        }
        return out
    }

    /// Offsets (within `statement`) of `(` that open a CTE body: one that follows `AS`, or
    /// `AS MATERIALIZED` / `AS NOT MATERIALIZED`. Strings and comments are skipped, so an `AS`
    /// inside a literal is not one.
    static func cteOpenParens(in statement: NSString) -> [Int] {
        var result: [Int] = []
        var index = 0
        while index < statement.length {
            let (word, next) = nextToken(in: statement, from: index)
            guard let word else { index = next; continue }
            if word == "AS" {
                var cursor = skipTrivia(statement, from: next)
                let (modifier, modifierEnd) = nextToken(in: statement, from: cursor)
                if modifier == "NOT" {
                    cursor = skipTrivia(statement, from: modifierEnd)
                    let (second, secondEnd) = nextToken(in: statement, from: cursor)
                    if second == "MATERIALIZED" { cursor = skipTrivia(statement, from: secondEnd) }
                } else if modifier == "MATERIALIZED" {
                    cursor = skipTrivia(statement, from: modifierEnd)
                }
                if cursor < statement.length, statement.character(at: cursor) == 0x28 {   // (
                    result.append(cursor)
                }
            }
            index = next
        }
        return result
    }

    /// The offset of the `)` matching the `(` at `open`, or nil when it never closes.
    static func matchingParen(in text: NSString, from open: Int) -> Int? {
        var depth = 0
        var index = open
        while index < text.length {
            let character = text.character(at: index)
            if isLiteralStart(character, in: text, at: index) {
                // A quote or a comment: step over the whole thing, so a `)` inside it is content.
                index = nextToken(in: text, from: index).advance
                continue
            }
            if character == 0x28 { depth += 1 }
            if character == 0x29 {
                depth -= 1
                if depth == 0 { return index }
            }
            index += 1
        }
        return nil
    }

    // MARK: Edits

    /// Move folded-header offsets through an edit, dropping the ones the edit landed on.
    ///
    /// The minimal diff — the common prefix and the common suffix — is enough to say whether an
    /// offset is before, inside or after the change. An offset inside the changed span cannot be
    /// trusted, so that fold is dropped and its region simply opens: silently keeping it could hide
    /// the wrong text, which is the one thing a fold must never do.
    static func shift(_ offsets: Set<Int>, from old: NSString, to new: NSString) -> Set<Int> {
        guard !old.isEqual(to: new as String) else { return offsets }
        let oldLength = old.length
        let newLength = new.length
        let limit = min(oldLength, newLength)

        var prefix = 0
        while prefix < limit, old.character(at: prefix) == new.character(at: prefix) { prefix += 1 }
        var suffix = 0
        while suffix < (limit - prefix),
              old.character(at: oldLength - 1 - suffix) == new.character(at: newLength - 1 - suffix) {
            suffix += 1
        }

        let delta = newLength - oldLength
        var shifted: Set<Int> = []
        for offset in offsets {
            if offset < prefix {
                shifted.insert(offset)
            } else if offset >= oldLength - suffix {
                shifted.insert(offset + delta)
            }
            // Otherwise the offset was inside the replaced span: drop it.
        }
        return shifted
    }

    // MARK: Lexing helpers

    private static func isTrivia(_ character: unichar) -> Bool {
        guard let scalar = Unicode.Scalar(character) else { return false }
        return CharacterSet.whitespacesAndNewlines.contains(scalar) || character == 0x3B   // ;
    }

    private static func lineEnd(_ line: Int, starts: [Int], length: Int) -> Int {
        line + 1 < starts.count ? starts[line + 1] : length
    }

    private static func containsWord(_ word: String, in text: NSString) -> Bool {
        var index = 0
        while index < text.length {
            let (token, next) = nextToken(in: text, from: index)
            if token == word { return true }
            index = next
        }
        return false
    }

    private static func leadingKeyword(_ text: NSString, from offset: Int) -> String? {
        let start = skipTrivia(text, from: offset)
        return nextToken(in: text, from: start).word
    }

    /// Whether the character at `index` starts a string or a comment, so a caller can step over it.
    private static func isLiteralStart(_ character: unichar, in text: NSString, at index: Int) -> Bool {
        if character == 0x27 || character == 0x22 || character == 0x60 { return true }
        if character == 0x2D, index + 1 < text.length, text.character(at: index + 1) == 0x2D { return true }
        if character == 0x2F, index + 1 < text.length, text.character(at: index + 1) == 0x2A { return true }
        return false
    }

    /// The next word or punctuation, skipping the contents of comments and quoted strings, plus the
    /// offset to continue from. A word is uppercased so keywords compare without case folding at
    /// every call site.
    private static func nextToken(in text: NSString, from index: Int) -> (word: String?, advance: Int) {
        guard index >= 0, index < text.length else { return (nil, text.length) }
        let character = text.character(at: index)

        if character == 0x2D, index + 1 < text.length, text.character(at: index + 1) == 0x2D {
            var cursor = index
            while cursor < text.length, text.character(at: cursor) != 0x0A { cursor += 1 }
            return (nil, cursor)
        }
        if character == 0x2F, index + 1 < text.length, text.character(at: index + 1) == 0x2A {
            var cursor = index + 2
            while cursor + 1 < text.length,
                  !(text.character(at: cursor) == 0x2A && text.character(at: cursor + 1) == 0x2F) {
                cursor += 1
            }
            return (nil, min(cursor + 2, text.length))
        }
        if character == 0x27 {
            // A doubled quote is an escaped quote, not the end of the literal.
            var cursor = index + 1
            while cursor < text.length {
                if text.character(at: cursor) == 0x27 {
                    if cursor + 1 < text.length, text.character(at: cursor + 1) == 0x27 {
                        cursor += 2
                        continue
                    }
                    cursor += 1
                    break
                }
                cursor += 1
            }
            return (nil, cursor)
        }
        if character == 0x22 || character == 0x60 {
            var cursor = index + 1
            while cursor < text.length {
                let current = text.character(at: cursor)
                cursor += 1
                if current == character {
                    if cursor < text.length, text.character(at: cursor) == character {
                        cursor += 1
                        continue
                    }
                    break
                }
            }
            return (nil, cursor)
        }
        if isWordStart(character) {
            var cursor = index
            while cursor < text.length, isWordCharacter(text.character(at: cursor)) { cursor += 1 }
            return (text.substring(with: NSRange(location: index, length: cursor - index)).uppercased(), cursor)
        }
        return (nil, index + 1)
    }

    /// Whitespace and comments, from `index` to the first character that is neither.
    private static func skipTrivia(_ text: NSString, from index: Int) -> Int {
        var cursor = index
        while cursor < text.length {
            let character = text.character(at: cursor)
            if let scalar = Unicode.Scalar(character), CharacterSet.whitespacesAndNewlines.contains(scalar) {
                cursor += 1
                continue
            }
            if character == 0x2D, cursor + 1 < text.length, text.character(at: cursor + 1) == 0x2D {
                cursor = nextToken(in: text, from: cursor).advance
                continue
            }
            if character == 0x2F, cursor + 1 < text.length, text.character(at: cursor + 1) == 0x2A {
                cursor = nextToken(in: text, from: cursor).advance
                continue
            }
            break
        }
        return cursor
    }

    static func isWordStart(_ character: unichar) -> Bool {
        guard let scalar = Unicode.Scalar(character) else { return false }
        return CharacterSet.letters.contains(scalar) || character == 0x5F
    }

    static func isWordCharacter(_ character: unichar) -> Bool {
        guard let scalar = Unicode.Scalar(character) else { return false }
        return CharacterSet.alphanumerics.contains(scalar)
            || character == 0x5F   // _
            || character == 0x24   // $
    }
}
