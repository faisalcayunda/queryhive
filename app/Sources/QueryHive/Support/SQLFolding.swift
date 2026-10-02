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
        /// A parenthesised subquery spanning more than one line.
        case subquery
        /// The body between two `$tag$` quotes of a function definition.
        case body
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
    // MARK: Lines

    /// UTF-16 offset of each line's first character.
    ///
    /// The start of the line after the last newline is included, so a text ending in `\n` reports a
    /// final empty line — which is where the caret is, and which the gutter also numbers.
    static func lineStarts(in text: NSString) -> [Int] {
        var starts = [0]
        // Read in chunks: `character(at:)` one unit at a time is a call per character, which is
        // milliseconds on a two-megabyte document, and this runs whenever the gutter rebuilds.
        let chunk = 16_384
        var buffer = [unichar](repeating: 0, count: chunk)
        var offset = 0
        while offset < text.length {
            let count = min(chunk, text.length - offset)
            text.getCharacters(&buffer, range: NSRange(location: offset, length: count))
            for i in 0..<count where buffer[i] == 0x0A { starts.append(offset + i + 1) }
            offset += count
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

    /// Every foldable region in the text, outermost/earliest first: statements, CTE bodies,
    /// subqueries and `$tag$` bodies, from one outline of the tree-sitter analysis. Over the
    /// ceiling there are no regions: no fold marks and no cost, never a fold cut short.
    static func regions(in sql: String) -> [FoldRegion] {
        regions(in: sql, statements: nil)
    }
    /// The same, for a caller that already has the statement ranges. The ranges are not reused —
    /// the outline answers statements, folds and issues together — but the parameter stays so
    /// the gutter's call sites keep their shape.
    static func regions(in sql: String, statements known: [NSRange]?) -> [FoldRegion] {
        _ = known
        let text = sql as NSString
        guard text.length <= SQLSyntax.ceiling,
              let analysis = try? EditorAnalysis(text: sql),
              let outline = try? analysis.drainForTesting().outline else { return [] }
        let starts = lineStarts(in: text)
        let sorted = outline.folds.compactMap {
            foldRegion($0, starts: starts, length: text.length)
        }.sorted { ($0.header, $0.lastLine) < ($1.header, $1.lastLine) }
        // One marker per line: a fold opening on its statement's own first line shares the
        // statement's header, and the statement's fold is the superset.
        var seen = Set<Int>()
        return sorted.filter { seen.insert($0.header).inserted }
    }

    /// The statement boundaries as character ranges, without their `;`.
    ///
    /// The same splitter Run uses, over the FFI, so a fold and a run cannot disagree about where
    /// a statement ends — and the boundary list is computed once, not once per statement.
    static func statementRanges(in sql: String) -> [NSRange] {
        (try? editorStatementRanges(sql)) ?? []
    }

    // MARK: CTEs
    /// One outline fold as a `FoldRegion`, its hidden body snapped to whole lines.
    static func foldRegion(_ fold: EditorFoldData, starts: [Int], length: Int) -> FoldRegion? {
        let kind: FoldRegion.Kind
        switch fold.kind {
        case .statement: kind = .statement
        case .cte: kind = .cte
        case .subquery: kind = .subquery
        case .body: kind = .body
        }
        guard fold.lastLine > fold.headerLine, fold.headerLine >= 0,
              fold.lastLine < starts.count else { return nil }
        let bodyStart = fold.headerLine + 1 < starts.count ? starts[fold.headerLine + 1] : length
        let bodyEnd = fold.lastLine + 1 < starts.count ? starts[fold.lastLine + 1] : length
        guard bodyEnd > bodyStart else { return nil }
        return FoldRegion(kind: kind, headerLine: fold.headerLine, lastLine: fold.lastLine,
                          header: starts[fold.headerLine], bodyStart: bodyStart, bodyEnd: bodyEnd,
                          summary: fold.summary)
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
}
