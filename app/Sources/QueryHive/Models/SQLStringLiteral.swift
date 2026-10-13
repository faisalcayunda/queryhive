import Foundation

/// A value written into SQL text as a string literal, for the statements this app spells itself: the
/// review text of a write-back (which the grid copies to the clipboard to run by hand, and which Trino
/// runs as written, having no placeholders), the cross-column search, and a `:name` parameter.
///
/// **What closes a literal depends on the dialect.** The quote doubles everywhere. A backslash is an
/// escape in MySQL (so `a\'` would end the literal early unless the backslash is doubled as well), in
/// PostgreSQL when `standard_conforming_strings` is off (the `E'…'` form with a doubled backslash reads
/// the same either way, which is the one spelling that is right in both), and never in Trino.
///
/// **Replacements are made on scalars.** `String.replacingOccurrences(of:with:)` without `.literal`
/// compares whole Swift `Character`s, and a `'` followed by a combining mark (U+0301), a joiner or a
/// variation selector is one Character that is not `'`: it would be left undoubled while the server
/// reads the quote byte and the literal closes. `.literal` compares code units, which is what a server
/// reads. Anything in this app that edits SQL text, a quote character or a comment opener must do the
/// same.
enum SQLStringLiteral {
    /// `'` becomes `''`, wherever it is, whatever follows it.
    static func doubledQuotes(_ text: String) -> String {
        text.replacingOccurrences(of: "'", with: "''", options: .literal)
    }

    /// `\` becomes `\\`, wherever it is, whatever follows it.
    static func doubledBackslashes(_ text: String) -> String {
        text.replacingOccurrences(of: "\\", with: "\\\\", options: .literal)
    }

    /// Whether `scalar` is anywhere in `text`, including under a combining mark that fuses it into a
    /// longer `Character`.
    static func contains(_ scalar: Unicode.Scalar, in text: String) -> Bool {
        text.unicodeScalars.contains(scalar)
    }

    /// The text as a string literal that a server of this kind reads back as exactly the text.
    static func quoted(_ text: String, for kind: ConnectionKind) -> String {
        switch kind {
        case .trino:
            return "'" + doubledQuotes(text) + "'"
        case .mysql:
            return "'" + doubledQuotes(doubledBackslashes(text)) + "'"
        case .postgres:
            guard contains("\\", in: text) else { return "'" + doubledQuotes(text) + "'" }
            return "E'" + doubledQuotes(doubledBackslashes(text)) + "'"
        }
    }
}
