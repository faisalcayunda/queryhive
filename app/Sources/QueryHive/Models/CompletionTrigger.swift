import Foundation
import QueryHiveFFI

/// The word the suggestion list is being asked about: what was typed, how it is qualified, where
/// picking a suggestion replaces from, and what came before it.
struct CompletionSite: Equatable {
    var prefix: String
    var path: [String]
    /// Where the typed word begins (its opening quote included); the end is the caret, widened
    /// over the rest of the word by whoever replaces it.
    var replaceStart: Int
    /// The word was typed inside an identifier quote that is still open.
    var quoted: Bool
    /// The caret's statement starts here; the text from it to `replaceStart` is the head the
    /// caret scope is read from.
    var statementStart: Int
    /// The word before this one, lower case (`from`, `by`), or nil at the start of a statement.
    var previousWord: String?
}

/// When the suggestion list opens by itself, as a function of the text, so the rules can be
/// tested without an editor.
///
/// - Right after a `.`, and inside an identifier quote that was just opened: always.
/// - After two typed name characters anywhere; after one where a name is expected (a table after
///   `FROM`, a column in a `WHERE`). Not at the start of a statement, where one letter is most
///   likely a keyword being typed out.
/// - On an empty word, after the keywords that are followed by a name: tables after `FROM`,
///   `JOIN`, `INTO`, `UPDATE`, `TABLE`; columns after `WHERE`, `ON`, `AND`, `OR`, `BY`, `HAVING`,
///   `SET`. That list is *passive*: it is there to be read, and Return is still a newline.
/// - Never inside a string, a comment or a `$$` body (the caller does not build a site there).
/// - Never right after Esc closed it, until the next word.
enum CompletionTrigger {
    /// Keywords after which a table is the next thing written.
    static let tableKeywords: Set<String> = ["from", "join", "into", "update", "table"]
    /// Keywords after which a column or an expression is.
    static let columnKeywords: Set<String> = ["where", "on", "and", "or", "by", "having", "set"]

    static func site(in text: NSString, caret: Int, dialect: EditorDialect,
                     hyphens: Bool = false) -> CompletionSite? {
        guard caret <= text.length else { return nil }
        let state = SQLCaretState.scan(text, upTo: caret, dialect: dialect)
        var open: Int?
        switch state.context {
        case .text: return nil
        case .quotedName(let at): open = at
        case .code: open = nil
        }
        guard let parsed = WordScope.parse(text, upTo: caret, hyphens: hyphens, quotedOpen: open) else {
            return nil
        }
        return CompletionSite(prefix: parsed.prefix, path: parsed.path, replaceStart: parsed.start,
                              quoted: parsed.quoted, statementStart: state.statementStart,
                              previousWord: previousWord(in: text, before: parsed.start))
    }

    /// The word that ends before `offset` once blanks are skipped, lower case. A dot or a quote
    /// in front of the word means it is part of a name, so there is no previous word then.
    static func previousWord(in text: NSString, before offset: Int) -> String? {
        var end = min(offset, text.length)
        while end > 0, isBlank(text.character(at: end - 1)) { end -= 1 }
        var start = end
        while start > 0, WordScope.isWordCharacter(text.character(at: start - 1)) { start -= 1 }
        guard start < end else { return nil }
        return text.substring(with: NSRange(location: start, length: end - start)).lowercased()
    }

    static func isBlank(_ unit: unichar) -> Bool {
        unit == 0x20 || unit == 0x09 || unit == 0x0A || unit == 0x0D
    }

    /// Words that can follow a table's name without being its alias.
    private static let afterTable: Set<String> = [
        "where", "join", "inner", "left", "right", "full", "cross", "natural", "outer", "on", "using",
        "group", "order", "having", "limit", "offset", "fetch", "union", "intersect", "except", "select",
        "set", "values", "returning", "window", "qualify", "lateral", "tablesample", "for", "with",
        "and", "or", "when", "then", "else", "end",
    ]

    /// Whether what follows `offset` is already an alias for the table being written: a name that is
    /// not one of the words that follow a table, an `AS`, or a quoted name. A comma, a bracket, the
    /// end of the statement or the end of the text is not.
    static func aliasFollows(in text: NSString, from offset: Int) -> Bool {
        var index = min(offset, text.length)
        while index < text.length, isBlank(text.character(at: index)) { index += 1 }
        guard index < text.length else { return false }
        let first = text.character(at: index)
        if first == 0x22 || first == 0x60 { return true }
        var end = index
        while end < text.length, WordScope.isWordCharacter(text.character(at: end)) { end += 1 }
        guard end > index else { return false }
        return !afterTable.contains(text.substring(with: NSRange(location: index, length: end - index)).lowercased())
    }

    /// Clauses where a single typed letter is already worth a list.
    private static func expectsName(_ clause: EditorClause?) -> Bool {
        guard let clause else { return false }
        return clause != .other
    }

    static func shouldOpen(_ site: CompletionSite, clause: EditorClause?, manual: Bool) -> Bool {
        if manual { return true }
        if !site.path.isEmpty || site.quoted { return true }
        switch site.prefix.count {
        case 0:
            guard let word = site.previousWord else { return false }
            return tableKeywords.contains(word) || columnKeywords.contains(word)
        case 1:
            return expectsName(clause)
        default:
            return true
        }
    }

    /// An empty word without a qualifier that nobody asked for: the list is there to be read, so
    /// Return must still insert a newline.
    static func isPassive(_ site: CompletionSite, manual: Bool) -> Bool {
        !manual && site.prefix.isEmpty && site.path.isEmpty && !site.quoted
    }
}

/// Esc closes the list for the word it was closed on; the next word may open it again.
struct CompletionDismissal: Equatable {
    private(set) var wordStart: Int?

    mutating func dismiss(at wordStart: Int) { self.wordStart = wordStart }

    mutating func reset() { wordStart = nil }

    /// Whether an automatic open for the word is suppressed, the word being any of the readings
    /// that start at `starts` (a word with a hyphen has two). Asking about a different word
    /// forgets the dismissal.
    mutating func suppresses(_ starts: [Int]) -> Bool {
        guard let wordStart else { return false }
        if starts.contains(wordStart) { return true }
        self.wordStart = nil
        return false
    }
}
