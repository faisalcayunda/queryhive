import Foundation

/// The search behind the editor's find bar, as pure functions over a string.
///
/// Kept out of the view because the parts that can be silently wrong — where a match starts, which
/// one "next" means once the caret has moved, what replace-all leaves behind — are the parts worth
/// testing without a window. The editor only wires the bar and the text view to these.
enum FindReplace {
    /// What the search may or may not ignore. An option set rather than loose booleans so a caller
    /// cannot pass a combination that means nothing.
    struct Options: OptionSet, Equatable {
        let rawValue: Int

        /// Match case exactly. Off is case-insensitive, which is what a find bar starts at.
        static let caseSensitive = Options(rawValue: 1 << 0)
        /// Require the match to be bounded by non-word characters on both sides.
        static let wholeWord = Options(rawValue: 1 << 1)
    }

    /// Every non-overlapping occurrence of `needle`, in order.
    ///
    /// `NSString.range(of:)` is used rather than a regular expression, so the needle is literal:
    /// pasting a fragment of SQL searches for the `.` and `*` it contains. A match that a whole-word
    /// filter rejects still advances the cursor, or it would be found again forever.
    static func ranges(in text: NSString, needle: String, options: Options = []) -> [NSRange] {
        guard !needle.isEmpty else { return [] }
        let compare: NSString.CompareOptions = options.contains(.caseSensitive)
            ? [.literal]
            : [.literal, .caseInsensitive]

        var found: [NSRange] = []
        var cursor = 0
        while cursor < text.length {
            let search = NSRange(location: cursor, length: text.length - cursor)
            let match = text.range(of: needle, options: compare, range: search)
            guard match.location != NSNotFound, match.length > 0 else { break }
            if !options.contains(.wholeWord) || isWholeWord(match, in: text) {
                found.append(match)
            }
            cursor = match.location + match.length
        }
        return found
    }

    /// The match at or after `location`, wrapping to the first.
    ///
    /// A caret sitting exactly on a match's first character counts as *next* rather than as one
    /// already passed, which is what makes Return keep advancing after a new search lands on a hit.
    static func next(in text: NSString, needle: String, from location: Int,
                     options: Options = []) -> NSRange? {
        let matches = ranges(in: text, needle: needle, options: options)
        guard !matches.isEmpty else { return nil }
        return matches.first(where: { $0.location >= location }) ?? matches.first
    }

    /// The match ending at or before `location`, wrapping to the last.
    static func previous(in text: NSString, needle: String, from location: Int,
                         options: Options = []) -> NSRange? {
        let matches = ranges(in: text, needle: needle, options: options)
        guard !matches.isEmpty else { return nil }
        return matches.last(where: { NSMaxRange($0) <= location }) ?? matches.last
    }

    /// Which match a caret is on or just before, for the count the bar shows.
    ///
    /// A caret inside a match belongs to it; otherwise the caret is between matches and the next
    /// one is the answer. Never nil unless there are no matches, so the bar can always say where
    /// the users is.
    static func currentIndex(of matches: [NSRange], selection: NSRange) -> Int? {
        guard !matches.isEmpty else { return nil }
        if let hit = matches.firstIndex(where: {
            selection.location >= $0.location && selection.location < NSMaxRange($0)
        }) {
            return hit
        }
        return matches.firstIndex(where: { $0.location >= selection.location }) ?? 0
    }

    /// The whole text with every occurrence replaced, and how many were.
    ///
    /// Applied back to front so replacing one match cannot move the ranges of the ones before it —
    /// which is the bug a forward loop produces when the replacement is a different length.
    static func replaceAll(in text: NSString, needle: String, with replacement: String,
                           options: Options = []) -> (text: String, count: Int) {
        let matches = ranges(in: text, needle: needle, options: options)
        guard !matches.isEmpty else { return (text as String, 0) }
        let result = NSMutableString(string: text)
        for match in matches.reversed() {
            result.replaceCharacters(in: match, with: replacement)
        }
        return (result as String, matches.count)
    }

    /// One replacement on its own, for "Replace" rather than "Replace All".
    static func replace(_ match: NSRange, in text: NSString, with replacement: String) -> String {
        text.replacingCharacters(in: match, with: replacement)
    }

    /// What the bar's count reads: "3 of 17", or "No matches".
    ///
    /// The count is 1-based for a person. A caller with no current index still gets a position, the
    /// first, because "1 of 17" is more useful than a blank when a query has just been typed.
    static func summary(count: Int, index: Int?) -> String {
        guard count > 0 else { return "No matches" }
        return "\((index ?? 0) + 1) of \(count)"
    }

    // MARK: Word boundaries

    static func isWordCharacter(_ character: unichar) -> Bool {
        guard let scalar = Unicode.Scalar(character) else { return false }
        return CharacterSet.alphanumerics.contains(scalar)
            || character == 0x5F   // _
            || character == 0x24   // $, which SQL identifiers may hold
    }

    private static func isWholeWord(_ range: NSRange, in text: NSString) -> Bool {
        if range.location > 0, isWordCharacter(text.character(at: range.location - 1)) { return false }
        let end = NSMaxRange(range)
        if end < text.length, isWordCharacter(text.character(at: end)) { return false }
        return true
    }
}
