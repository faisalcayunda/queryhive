import Foundation

/// Reading a structured cell — an `ARRAY`, `MAP`, `ROW` or `JSON` value — out of the grid.
///
/// The engine serialises these as JSON text in an export, but the grid receives the server's own
/// text and holds it as one `String?` per cell. `docs/golden-deltas.md` D-9 is the case that proves
/// it: a PostgreSQL array arrives as `{1,NULL,3}`, a server literal rather than a JSON list. So this
/// is a *viewer*, not a decoder: it never changes how a value is stored or exported, it only decides
/// whether a cell can be opened and how to lay the text out once it is.
enum GridValue {
    /// The largest value this will parse as JSON, in UTF-16 units.
    ///
    /// TablePro's number, and the reason it exists: a JSON cell is untrusted input, and parsing a
    /// multi-megabyte one on the main thread is a hang the user cannot cancel. Above it a
    /// structured cell is still openable — the viewer shows the server's raw text instead — so the
    /// limit is stated rather than silent.
    static let parseLimit = 100_000

    /// The largest value the viewer renders in full, in UTF-16 units.
    ///
    /// Above it the text is truncated **with a marker**: a control holding ten megabytes of text
    /// is its own hang. The Copy button still copies the whole value, which is what the limit is
    /// protecting.
    static let textLimit = 500_000

    /// Whether a column's type is one whose cells arrive as structured text.
    ///
    /// Matched loosely because the three drivers spell the same shape differently: Trino says
    /// `array(bigint)`, `map(varchar,bigint)` and `row(x integer)`, PostgreSQL says `integer[]`, and
    /// both say `json`.
    static func isStructured(type: String) -> Bool {
        let lowered = type.lowercased()
        if lowered.hasSuffix("[]") { return true }
        return ["array", "map(", "row(", "json"].contains { lowered.contains($0) }
    }

    /// Whether a column's type is one whose cells are bytes rather than text.
    ///
    /// The engine renders `bytea`, `BLOB` and `varbinary` as lowercase hex
    /// (`crates/qh-core/src/render.rs::hex_encode`), so a cell of one of these can be decoded back
    /// and shown as a hex dump. The match is a substring because the drivers spell the family
    /// differently — PostgreSQL `bytea`, MySQL `blob`/`tinyblob`/`mediumblob`/`longblob`/`binary`,
    /// Trino `varbinary` — and every one of those contains one of these words.
    static func isBinary(type: String) -> Bool {
        let lowered = type.lowercased()
        return ["bytea", "blob", "binary"].contains { lowered.contains($0) }
    }

    /// The pretty-printed form of a value when it parses as JSON, or `nil` when it does not.
    ///
    /// Keys are sorted so one value always prints the same way; the viewer's job is to be read, not
    /// to reproduce the server's key order. `.fragmentsAllowed` lets a top-level scalar through, but
    /// `isOpenable` only offers the viewer for values that look like an object or array, so a
    /// boolean column is never turned into an "openable JSON" cell.
    static func prettyPrinted(_ text: String) -> String? {
        // Before the parse, not after: the point of the limit is not to build the object at all.
        guard (text as NSString).length <= parseLimit,
              let data = text.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data,
                                                             options: [.fragmentsAllowed]),
              let pretty = try? JSONSerialization.data(withJSONObject: object,
                                                       options: [.prettyPrinted, .fragmentsAllowed,
                                                                 .sortedKeys]) else { return nil }
        return String(decoding: pretty, as: UTF8.self)
    }

    /// The raw text the viewer shows, truncated with a marker when it is over `textLimit`.
    ///
    /// The marker is the whole point: a viewer that quietly cut the tail off would be a reader
    /// that lies about the value, which is the one thing a reader must not do.
    static func displayText(_ text: String) -> String {
        let length = (text as NSString).length
        guard length > textLimit else { return text }
        let head = (text as NSString).substring(to: textLimit)
        return head + "\n\n… truncated for display (\(length.formatted()) units); Copy keeps the whole value."
    }

    /// Whether a cell is worth opening: a structured column, a binary column, or text that is
    /// itself a JSON object or array.
    ///
    /// The second half makes JSON stored in a plain `varchar` readable, which is common enough in
    /// this data. The container check keeps it narrow: `true`, `42` and `"x"` parse as JSON too, and
    /// a cell that opens a viewer only to show the same word it already shows is noise. A binary
    /// column always opens, because the hex dump is something the one-line cell cannot show.
    static func isOpenable(value: String?, type: String) -> Bool {
        guard let value, !value.isEmpty else { return false }
        if isStructured(type: type) || isBinary(type: type) { return true }
        return looksLikeJSON(value) && prettyPrinted(value) != nil
    }

    /// A JSON object or array, by its first non-space character. A PostgreSQL array literal is
    /// `{…}` too, so a structured column whose value does not parse still reaches the viewer; it
    /// just shows the server's raw text there.
    static func looksLikeJSON(_ text: String) -> Bool {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.hasPrefix("{") || trimmed.hasPrefix("[")
    }
}
