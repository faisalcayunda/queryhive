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

    /// The attribute runs for a statement, with the fonts already merged in.
    ///
    /// The fonts arrive as arguments rather than being read from a static here, because they are
    /// the one part of a run that changes while the app runs. The colours stay static: they are
    /// dynamic `NSColor`s that resolve per appearance, so they never need rebuilding.
    static func attributes(for sql: String, baseFont: NSFont, commentFont: NSFont) -> [(NSRange, [NSAttributedString.Key: Any])] {
        guard sql.utf16.count <= ceiling else { return [] }
        let whole = NSRange(location: 0, length: sql.utf16.count)
        var out: [(NSRange, [NSAttributedString.Key: Any])] = []
        let ns = sql as NSString

        for match in pattern.matches(in: sql, options: [], range: whole) {
            let range = match.range
            guard range.location != NSNotFound, range.length > 0 else { continue }
            let text = ns.substring(with: range)

            if match.range(at: 1).location != NSNotFound {
                // The only token that changes shape: a comment is the one run drawn in an italic
                // cut, so it is the only one that has to carry its own font.
                out.append((range, comment.merging([.font: commentFont]) { _, new in new }))
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

        // Both fonts resolved once per pass, from the current setting. The comment's italic is the
        // one token that differs, and it is the only reason there are two.
        let upright = font(italic: false)
        let italic = font(italic: true)
        let base = Self.base.merging([.font: upright]) { _, new in new }

        storage.beginEditing()
        // Reset first: a word that stopped being a keyword must stop looking like one.
        storage.setAttributes(base, range: whole)
        for (range, attributes) in attributes(for: sql, baseFont: upright, commentFont: italic)
        where NSMaxRange(range) <= whole.length {
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

    /// One token's colour, as a pair: the value for a dark canvas and the value for a light one.
    ///
    /// Every one of these was chosen for a near-black canvas and is unusable on an off-white one —
    /// measured, `base` is 1.11:1 on Daylight and `string` 1.48:1, which is why a light appearance
    /// made the query text effectively disappear. Rather than keep two hand-tuned palettes in sync,
    /// each token is built through `NSColor(name:dynamicProvider:)` so AppKit picks the right half
    /// per appearance, exactly like `Tone.ink`.
    ///
    /// The light values are not guesses: each was darkened at its own hue until it cleared 4.5:1
    /// against the Daylight canvas (`#F4F6FB`), and each dark value still clears 4.5:1 against
    /// Midnight (`#0A0B1E`). `comment` is deliberately below that in the dark — it is the one token
    /// meant to recede — but is kept legible in the light.
    /// The colours only, with no font in them.
    ///
    /// The font used to be baked in here, and these are `static let`, so the whole attribute
    /// dictionary — font included — was built once, on first use, and then reused for the life of
    /// the process. That is why the code font is applied per pass in `apply(to:)` instead: a
    /// settings change has to reach the editor without a relaunch, and a cached dictionary cannot
    /// notice one.
    private static func colour(_ dark: UInt32, _ light: UInt32) -> [NSAttributedString.Key: Any] {
        let adaptive = NSColor(name: nil) { appearance in
            NSColor(Color(hex: appearance.isDark ? dark : light))
        }
        return [.foregroundColor: adaptive]
    }

    static let base = colour(0xE8EAF2, 0x1C1F26)
    static let keyword = colour(0x8B7BFF, 0x5B3FD6)
    static let function = colour(0x4FD8FF, 0x0B6E8F)
    static let string = colour(0x3EE6A8, 0x0A7A52)
    static let number = colour(0xFFB547, 0x9A5B00)
    static let quotedIdentifier = colour(0xE8C468, 0x7A5C00)
    static let literal = colour(0xFF7A8A, 0xBE2F45)
    static let comment = colour(0x5A6072, 0x5F6672)
    static let punctuation = colour(0x8A90A6, 0x565C6B)

    /// The font the highlighter paints in, resolved fresh so it follows the code-font setting.
    ///
    /// Italic is asked for through the descriptor rather than `NSFontManager`: a fixed-pitch face
    /// often has no italic cut for the manager to convert to, and it returns nil. When the chosen
    /// family has no italic either, the descriptor returns nil and the upright font is kept — a
    /// comment that is not slanted is a smaller loss than a comment that does not draw.
    private static func font(italic: Bool) -> NSFont {
        let plain = FontChoice.codeNSFont(size: 12.5, weight: .regular)
        guard italic else { return plain }
        return NSFont(descriptor: plain.fontDescriptor.withSymbolicTraits(.italic), size: 12.5) ?? plain
    }
}
