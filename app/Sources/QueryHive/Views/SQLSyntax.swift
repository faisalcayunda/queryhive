import AppKit
import SwiftUI

/// Colours the SQL editor by what each token *is*.
///
/// The classes come from the tree-sitter analysis (`EditorAnalysis`, Fase 4B): one tree per
/// statement in Rust, applied here as temporary `NSLayoutManager` attributes. This file owns
/// only the palette — the mapping from class to colour — and the comment's italic font. The
/// keyword set stays because auto-uppercase (`KeywordCase`, `SQLEditor`) still reads it.
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

    /// Past this many UTF-16 units the editor stops colouring rather than stalling on every
    /// keystroke. The value is the engine's own ceiling, so the editor and the analysis agree.
    static var ceiling: Int { (try? editorCeiling()) ?? 2_000_000 }

    /// The colour for one analysis class, resolved per appearance at draw time.
    ///
    /// A parameter reads as a value, like `null`, so it wears `literal`'s colour while keeping
    /// its own class across the FFI — a future appearance can split it off without Rust work.
    static func colour(for colorClass: EditorColorClass) -> NSColor {
        let entry: [NSAttributedString.Key: Any]
        switch colorClass {
        case .comment: entry = comment
        case .string: entry = string
        case .quotedIdentifier: entry = quotedIdentifier
        case .number: entry = number
        case .keyword: entry = keyword
        case .literal, .parameter: entry = literal
        case .function: entry = function
        case .punctuation: entry = punctuation
        }
        return entry[.foregroundColor] as? NSColor ?? .labelColor
    }

    /// The palette as hex pairs, one per colour class, for the contrast gate (G8,
    /// `SyntaxPaletteTests`): each half must clear 4.5:1 against its canvas, except `comment`
    /// in the dark, which is meant to recede.
    static func hexPair(for colorClass: EditorColorClass) -> (dark: UInt32, light: UInt32) {
        switch colorClass {
        case .comment: return (0x5A6072, 0x5F6672)
        case .string: return (0x3EE6A8, 0x0A7A52)
        case .quotedIdentifier: return (0xE8C468, 0x7A5C00)
        case .number: return (0xFFB547, 0x9A5B00)
        case .keyword: return (0x9A8DFF, 0x5B3FD6)
        case .literal, .parameter: return (0xFF7A8A, 0xBE2F45)
        case .function: return (0x4FD8FF, 0x0B6E8F)
        case .punctuation: return (0x9AA0B5, 0x565C6B)
        }
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
    /// against the Daylight canvas (`#F4F6FB`), and each dark value clears 4.5:1 against all four
    /// dark canvases — Midnight, Graphite, Nord and Ink. `comment` is deliberately below that in
    /// the dark — it is the one token meant to recede — but is kept legible in the light.
    /// The colours only, with no font in them.
    ///
    /// The font used to be baked in here, and these are `static let`, so the whole attribute
    /// dictionary — font included — was built once, on first use, and then reused for the life of
    /// the process. That is why the code font is resolved per paint in `font(italic:)` instead: a
    /// settings change has to reach the editor without a relaunch, and a cached dictionary cannot
    /// notice one.
    private static func colour(_ dark: UInt32, _ light: UInt32) -> [NSAttributedString.Key: Any] {
        // Both halves resolved once. The provider runs every time AppKit draws or fixes a run, and
        // building a colour through SwiftUI each time was a measurable part of a keystroke.
        let darkColour = NSColor(Color(hex: dark))
        let lightColour = NSColor(Color(hex: light))
        let adaptive = NSColor(name: nil) { appearance in
            appearance.isDark ? darkColour : lightColour
        }
        return [.foregroundColor: adaptive]
    }

    /// Every character carries the default paragraph style explicitly, so a typed character inherits
    /// it through the typing attributes and folding never has to touch attributes.
    static let base = colour(0xE8EAF2, 0x1C1F26).merging([.paragraphStyle: NSParagraphStyle.default]) { _, new in new }
    static let keyword = colour(0x9A8DFF, 0x5B3FD6)
    static let function = colour(0x4FD8FF, 0x0B6E8F)
    static let string = colour(0x3EE6A8, 0x0A7A52)
    static let number = colour(0xFFB547, 0x9A5B00)
    static let quotedIdentifier = colour(0xE8C468, 0x7A5C00)
    static let literal = colour(0xFF7A8A, 0xBE2F45)
    static let comment = colour(0x5A6072, 0x5F6672)
    static let punctuation = colour(0x9AA0B5, 0x565C6B)

    /// The font the highlighter paints in, resolved fresh so it follows the code-font setting.
    ///
    /// Italic is asked for through the descriptor rather than `NSFontManager`: a fixed-pitch face
    /// often has no italic cut for the manager to convert to, and it returns nil. When the chosen
    /// family has no italic either, the descriptor returns nil and the upright font is kept — a
    /// comment that is not slanted is a smaller loss than a comment that does not draw.
    static func font(italic: Bool) -> NSFont {
        let plain = FontChoice.codeNSFont(size: 12.5, weight: .regular)
        guard italic else { return plain }
        return NSFont(descriptor: plain.fontDescriptor.withSymbolicTraits(.italic), size: 12.5) ?? plain
    }
}
