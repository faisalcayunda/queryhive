import Foundation

/// How a column's stored text is *displayed* — never how it is stored or exported.
///
/// TablePro keeps a display format per connection + table, and so does `ColumnFormatStore`. The set
/// here is the subset that means something for a client whose cells arrive as strings: no PHP
/// serializer, because the engine has none, and no separate "raw bytes" spelling, because the one
/// byte view lives in the cell reader's Hex mode where it can be looked at properly.
///
/// Every case is rendering-only. The value the server sent is what the grid copies, what an export
/// writes and what the cell viewer's Copy button takes; a format changes the characters on screen
/// and nothing else.
enum ColumnFormat: String, CaseIterable, Identifiable, Codable {
    case raw
    case text
    case uuid
    case unixTimestamp = "unix_timestamp"
    case json

    var id: Self { self }

    var label: String {
        switch self {
        case .raw: "Raw"
        case .text: "Text"
        case .uuid: "UUID"
        case .unixTimestamp: "Unix timestamp"
        case .json: "JSON"
        }
    }

    var help: String {
        switch self {
        case .raw: "The text the server sent, unchanged."
        case .text: "Bytes decoded as UTF-8 text; text is left as it is."
        case .uuid: "A 32-digit hex UUID, written 8-4-4-4-12."
        case .unixTimestamp: "Seconds (or milliseconds) since 1970, as a UTC date and time."
        case .json: "Parsed and re-indented when the value is JSON."
        }
    }

    /// The one-line text a cell shows under this format. Rendering only.
    ///
    /// `type` decides what `.text` means: for a binary column it decodes the hex the engine sent
    /// back into text, and for everything else it is the raw value, because there is nothing to
    /// decode. A value this cannot recognise comes back unchanged rather than carrying an error —
    /// a display format that rejected values would be a second way for the grid to be wrong.
    func render(_ value: String, type: String = "") -> String {
        switch self {
        case .raw:
            return value
        case .text:
            guard GridValue.isBinary(type: type) else { return value }
            return String(decoding: HexDump.bytes(from: value), as: UTF8.self)
        case .uuid:
            return Self.canonicalUUID(value) ?? value
        case .unixTimestamp:
            return Self.timestamp(value) ?? value
        case .json:
            // Collapsed to one line: a pretty-printed object has no business breaking a grid row,
            // and the viewer's own JSON mode is where the indentation is worth having.
            guard let pretty = GridValue.prettyPrinted(value) else { return value }
            return pretty.replacingOccurrences(of: "\n", with: " ")
        }
    }

    /// `32` hex digits, with or without the dashes already in place, as the canonical spelling.
    static func canonicalUUID(_ value: String) -> String? {
        guard value.allSatisfy({ $0.isHexDigit || $0 == "-" }) else { return nil }
        let digits = Array(value.lowercased().filter(\.isHexDigit))
        guard digits.count == 32 else { return nil }
        func part(_ range: Range<Int>) -> String { String(digits[range]) }
        return "\(part(0..<8))-\(part(8..<12))-\(part(12..<16))-\(part(16..<20))-\(part(20..<32))"
    }

    /// A Unix timestamp as `yyyy-MM-dd HH:mm:ss` in UTC.
    ///
    /// A 13-digit value is read as milliseconds, because that is the other plausible unit and a
    /// seconds reading would land in the year 50,000. Everything else is seconds, fractions
    /// included. UTC rather than the local zone because a rendered grid must not move when the
    /// machine's timezone setting does. Not a number: `nil`, and the caller shows the stored text.
    static func timestamp(_ value: String) -> String? {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let number = Double(trimmed), number.isFinite else { return nil }
        let seconds = abs(number) >= 1e11 ? number / 1000 : number
        let date = Date(timeIntervalSince1970: seconds)
        // `DateFormatter` is not thread-safe, so it is built per call rather than shared. This runs
        // once per cell per render on a small preview, which is cheap enough to keep it simple.
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = TimeZone(identifier: "UTC")
        formatter.dateFormat = "yyyy-MM-dd HH:mm:ss"
        return formatter.string(from: date)
    }
}

/// Where per-column display formats live between launches.
///
/// The app's other small preferences go straight into `UserDefaults` as strings, and this follows
/// that shape with one key holding every choice. A format belongs to a column, and a column name
/// means different things on two servers, so the identity is connection + table + column — the
/// same unit TablePro files it under, with the column added because two columns of one table can
/// want different formats.
///
/// `raw` is stored as the absence of an entry rather than as a value, so an untouched grid writes
/// nothing and "clear every format" is one `removeObject`.
enum ColumnFormatStore {
    static let defaultsKey = "columnFormats"

    /// The key a choice is filed under, or `nil` when the app does not know where the cell came
    /// from — a hand-written query — and then there is nowhere to file it.
    static func identity(connection: UUID?, table: String?, column: String) -> String? {
        guard let connection, let table, !table.isEmpty, !column.isEmpty else { return nil }
        return "\(connection.uuidString)|\(table)|\(column)"
    }

    static func format(_ identity: String, in defaults: UserDefaults = .standard) -> ColumnFormat {
        let stored = defaults.dictionary(forKey: defaultsKey) as? [String: String]
        return stored?[identity].flatMap(ColumnFormat.init(rawValue:)) ?? .raw
    }

    static func set(_ format: ColumnFormat, for identity: String, in defaults: UserDefaults = .standard) {
        var stored = (defaults.dictionary(forKey: defaultsKey) as? [String: String]) ?? [:]
        if format == .raw {
            stored.removeValue(forKey: identity)
        } else {
            stored[identity] = format.rawValue
        }
        if stored.isEmpty {
            defaults.removeObject(forKey: defaultsKey)
        } else {
            defaults.set(stored, forKey: defaultsKey)
        }
    }
}
