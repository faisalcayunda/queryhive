import Foundation

/// Finishing a keyword: whether the word just typed should be replaced by its canonical form.
///
/// A pure decision, so it can be tested without a text view; the coordinator's whole job is to
/// apply what this says. A word counts as finished when the character after it is a delimiter, and
/// it is replaced only when it is a keyword and is not already in the shape it would become — which
/// is what keeps this away from identifiers, from string contents, and from a word the user typed
/// in the case they meant.
enum KeywordCase {
    /// The characters that finish a word. A letter or a digit means the word is still being typed,
    /// and `.` is deliberately absent: `hive.analytics` is a qualified name, and its parts are not
    /// keywords even when they are spelled like one. Quotes are absent for the same reason — a
    /// quote ends a *literal*, not a word.
    static let delimiters = Set(" \n\t\r,;()[]{}<>=+-*/%|&!~^")

    struct Replacement: Equatable {
        let range: NSRange
        let text: String
    }

    /// What to replace at `caret`, or `nil` when nothing should change.
    ///
    /// `text` is the whole document as it stands after the keystroke, so `caret - 1` is the
    /// character just typed.
    static func replacement(in text: NSString, caret: Int) -> Replacement? {
        guard caret > 0, caret <= text.length else { return nil }
        guard let typed = UnicodeScalar(text.character(at: caret - 1)),
              delimiters.contains(Character(typed)) else { return nil }

        var start = caret - 1
        while start > 0, isWordCharacter(text.character(at: start - 1)) { start -= 1 }
        guard start < caret - 1 else { return nil }

        // One part of a qualified name, and its case can be the identifier's own: `hive.from` is a
        // column called `from`, not the keyword.
        if start > 0, UnicodeScalar(text.character(at: start - 1)) == "." { return nil }
        // A word inside a string, a quoted identifier or a comment only looks like a keyword.
        guard SQLScanner.scan(text as String).isCode(at: start) else { return nil }

        let range = NSRange(location: start, length: caret - 1 - start)
        let word = text.substring(with: range)
        let canonical = word.uppercased()
        guard canonical != word, SQLSyntax.keywords.contains(word.lowercased()) else { return nil }
        return Replacement(range: range, text: canonical)
    }

    private static func isWordCharacter(_ character: unichar) -> Bool {
        guard let scalar = UnicodeScalar(character) else { return false }
        return CharacterSet.alphanumerics.contains(scalar) || scalar == "_" || scalar == "$"
    }
}
