import AppKit
import SwiftUI

/// Colours the SQL editor by what each token *is*.
///
/// One scan, six alternatives, then the words are classified. That is enough for SQL as it is
/// actually written — the hard cases in a real parser (a `--` inside a string, a quote inside a
/// comment) are handled by the alternatives being ordered, not by tracking state, because the
/// regex engine resolves them the same way a reader does.
enum SQLSyntax {
    /// Words that introduce or shape a statement.
    static let keywords: Set<String> = [
        "select", "from", "where", "group", "by", "order", "having", "limit", "offset", "fetch",
        "insert", "into", "values", "update", "set", "delete", "merge", "using", "on", "as",
        "join", "inner", "left", "right", "full", "outer", "cross", "natural", "lateral", "apply",
        "union", "all", "distinct", "except", "intersect", "with", "recursive", "over", "partition",
        "window", "filter", "range", "rows", "preceding", "following", "unbounded", "current", "row",
        "and", "or", "not", "in", "exists", "between", "like", "ilike", "rlike", "regexp", "is",
        "case", "when", "then", "else", "end", "cast", "try_cast", "null", "true", "false",
        "asc", "desc", "nulls", "first", "last", "create", "replace", "table", "view", "schema",
        "database", "catalog", "drop", "alter", "add", "column", "rename", "to", "if", "primary",
        "key", "foreign", "references", "unique", "index", "grant", "revoke", "explain", "analyze",
        "describe", "show", "use", "begin", "commit", "rollback", "transaction", "interval", "at",
        "time", "zone", "escape", "collate", "default", "constraint", "check", "cascade", "restrict",
        "unnest", "generated", "always", "identity", "returning", "conflict", "do", "nothing",
        "engine", "charset", "auto_increment", "unsigned", "zerofill", "straight_join", "force",
    ]

    /// Selected and highlighted as a unit rather than by word.
    private static let pattern = try! NSRegularExpression(
        pattern: #"(--[^\n]*|/\*[\s\S]*?\*/)|('(?:[^']|'')*')|("(?:[^"]|"")*"|`[^`]*`)|(\b\d+(?:\.\d+)?\b)|([A-Za-z_][A-Za-z0-9_$]*)|([-+*/%=<>!|,;().\[\]]+)"#,
        options: [])

    /// Past this the editor stops colouring rather than stalling on every keystroke. Nothing this
    /// app opens is that big; a pasted dump might be.
    private static let ceiling = 200_000

    static func attributes(for sql: String) -> [(NSRange, [NSAttributedString.Key: Any])] {
        guard sql.utf16.count <= ceiling else { return [] }
        let whole = NSRange(location: 0, length: sql.utf16.count)
        var out: [(NSRange, [NSAttributedString.Key: Any])] = []
        let ns = sql as NSString

        for match in pattern.matches(in: sql, options: [], range: whole) {
            let range = match.range
            guard range.location != NSNotFound, range.length > 0 else { continue }
            let text = ns.substring(with: range)

            if match.range(at: 1).location != NSNotFound {
                out.append((range, comment))
            } else if match.range(at: 2).location != NSNotFound {
                out.append((range, string))
            } else if match.range(at: 3).location != NSNotFound {
                // A quoted name is an identifier, whichever quote the driver prefers.
                out.append((range, quotedIdentifier))
            } else if match.range(at: 4).location != NSNotFound {
                out.append((range, number))
            } else if match.range(at: 5).location != NSNotFound {
                let word = text.lowercased()
                if keywords.contains(word) {
                    out.append((range, isLiteral(word) ? literal : keyword))
                } else if isCalled(ns, after: match) {
                    out.append((range, function))
                }
            } else if match.range(at: 6).location != NSNotFound {
                out.append((range, punctuation))
            }
        }
        return out
    }

    static func apply(to textView: NSTextView) {
        guard let storage = textView.textStorage else { return }
        let sql = storage.string
        let whole = NSRange(location: 0, length: (sql as NSString).length)

        storage.beginEditing()
        // Reset first: a word that stopped being a keyword must stop looking like one.
        storage.setAttributes(base, range: whole)
        for (range, attributes) in attributes(for: sql) where NSMaxRange(range) <= whole.length {
            storage.addAttributes(attributes, range: range)
        }
        storage.endEditing()
        // The scan is not cheap and the attributes shift nothing, so put the caret back exactly
        // where it was rather than letting the layout pass move it.
        textView.typingAttributes = base
    }

    // MARK: Classification

    private static func isCalled(_ sql: NSString, after match: NSTextCheckingResult) -> Bool {
        var index = NSMaxRange(match.range)
        while index < sql.length, sql.character(at: index) == 0x20 { index += 1 }
        return index < sql.length && sql.character(at: index) == 0x28   // "("
    }

    /// `null`, `true` and `false` are keywords syntactically and values semantically, and they read
    /// as values.
    private static func isLiteral(_ word: String) -> Bool {
        word == "null" || word == "true" || word == "false"
    }

    // MARK: Palette

    private static func colour(_ hex: UInt32, italic: Bool = false) -> [NSAttributedString.Key: Any] {
        let plain = NSFont.monospacedSystemFont(ofSize: 12.5, weight: .regular)
        // Through the descriptor, not NSFontManager: the monospaced system face has no italic cut
        // for the manager to convert to, and it returns nil.
        let font = italic
            ? (NSFont(descriptor: plain.fontDescriptor.withSymbolicTraits(.italic), size: 12.5) ?? plain)
            : plain
        return [.foregroundColor: NSColor(Color(hex: hex)), .font: font]
    }

    static let base = colour(0xE8EAF2)
    static let keyword = colour(0x8B7BFF)
    static let function = colour(0x4FD8FF)
    static let string = colour(0x3EE6A8)
    static let number = colour(0xFFB547)
    static let quotedIdentifier = colour(0xE8C468)
    static let literal = colour(0xFF7A8A)
    static let comment = colour(0x5A6072, italic: true)
    static let punctuation = colour(0x8A90A6)
}
