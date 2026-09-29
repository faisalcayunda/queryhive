import Foundation

/// Reading a structured cell — an `ARRAY`, `MAP`, `ROW` or `JSON` value — out of the grid.
///
/// The engine serialises these as JSON text in an export, but the grid receives the server's own
/// text and holds it as one `String?` per cell. `docs/golden-deltas.md` D-9 is the case that proves
/// it: a PostgreSQL array arrives as `{1,NULL,3}`, a server literal rather than a JSON list. So this
/// is a *viewer*, not a decoder: it never changes how a value is stored or exported, it only decides
/// whether a cell can be opened and how to lay the text out once it is.
enum GridValue {
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

    /// The pretty-printed form of a value when it parses as JSON, or `nil` when it does not.
    ///
    /// Keys are sorted so one value always prints the same way; the viewer's job is to be read, not
    /// to reproduce the server's key order. `.fragmentsAllowed` lets a top-level scalar through, but
    /// `isOpenable` only offers the viewer for values that look like an object or array, so a
    /// boolean column is never turned into an "openable JSON" cell.
    static func prettyPrinted(_ text: String) -> String? {
        guard let data = text.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data,
                                                             options: [.fragmentsAllowed]),
              let pretty = try? JSONSerialization.data(withJSONObject: object,
                                                       options: [.prettyPrinted, .fragmentsAllowed,
                                                                 .sortedKeys]) else { return nil }
        return String(decoding: pretty, as: UTF8.self)
    }

    /// Whether a cell is worth opening: a structured column, or text that is itself a JSON object
    /// or array.
    ///
    /// The second half makes JSON stored in a plain `varchar` readable, which is common enough in
    /// this data. The container check keeps it narrow: `true`, `42` and `"x"` parse as JSON too, and
    /// a cell that opens a viewer only to show the same word it already shows is noise.
    static func isOpenable(value: String?, type: String) -> Bool {
        guard let value, !value.isEmpty else { return false }
        if isStructured(type: type) { return true }
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
