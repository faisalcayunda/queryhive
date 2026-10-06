import Foundation

@testable import QueryHive

/// The Swift sort, filter, search and distinct-list implementations the grid ran before the Rust
/// view existed (blueprint D-25). Production no longer carries them; `SortFixtureExport` still
/// drives them to regenerate the differential fixture, and `StoreRowsTests` uses them as the twin
/// the Rust view must agree with.
enum SwiftGridReference {
    // MARK: Sort

    static func order(_ sort: GridSort, _ rows: [[String?]]) -> [[String?]] {
        rows.enumerated()
            .sorted { left, right in
                let result = compare(value(in: left.element, at: sort.column),
                                     value(in: right.element, at: sort.column))
                if result == .orderedSame { return left.offset < right.offset }
                return (result == .orderedAscending) == (sort.direction == .ascending)
            }
            .map(\.element)
    }

    /// The cell a row holds in a column, or `nil` when the row is shorter than the grid's columns.
    ///
    /// A short row reads as NULL here for the same reason the grid draws it blank: there is no
    /// value, and a sort that treated it as the empty string would put it among the real empties
    /// instead of with the nulls.
    static func value(in row: [String?], at column: Int) -> String? {
        row.indices.contains(column) ? row[column] : nil
    }

    /// How two cells compare, before the direction is applied.
    ///
    /// **NULLs** sort last ascending and first descending — the mirror of PostgreSQL's own default
    /// (`NULLS LAST` for `ASC`, `NULLS FIRST` for `DESC`). That choice makes descending exactly the
    /// reverse of ascending rather than a second order with its own rules, which is what a click
    /// on the same header should mean.
    ///
    /// **Numbers** compare numerically only when *both* cells are plain numbers, so "10" comes
    /// after "9". A cell that is not a plain number falls back to a locale-aware text compare, and
    /// a number sorts before text when a column mixes the two. The strict "is it a plain number"
    /// check matters: `Decimal(string:)` alone reads `32.01.01.2001` as `32.01` and a timestamp as
    /// its year, which would silently reorder a column nobody asked to reorder.
    static func compare(_ left: String?, _ right: String?) -> ComparisonResult {
        switch (left, right) {
        case (nil, nil):
            return .orderedSame
        case (nil, _):
            return .orderedDescending
        case (_, nil):
            return .orderedAscending
        case let (left?, right?):
            switch (number(left), number(right)) {
            case let (leftNumber?, rightNumber?):
                if leftNumber == rightNumber { return .orderedSame }
                return leftNumber < rightNumber ? .orderedAscending : .orderedDescending
            case (_?, nil):
                return .orderedAscending
            case (nil, _?):
                return .orderedDescending
            case (nil, nil):
                // Locale-aware, and natural for text that holds digits: "KPM 9" sorts before
                // "KPM 10", the same correction the numeric branch makes for whole cells.
                return left.localizedStandardCompare(right)
            }
        }
    }

    /// A cell read as a number, or `nil` when it is not one.
    static func number(_ text: String) -> Decimal? {
        let trimmed = text.trimmingCharacters(in: .whitespaces)
        guard !trimmed.isEmpty, isPlainNumber(trimmed) else { return nil }
        // POSIX, so a decimal point is a dot whatever the user's locale says: these values came
        // from a server, not from the locale.
        return Decimal(string: trimmed, locale: Locale(identifier: "en_US_POSIX"))
    }

    /// A sign, digits with at most one dot, and an optional exponent — and nothing else.
    static func isPlainNumber(_ text: String) -> Bool {
        var sawDigit = false
        var sawDot = false
        var index = text.startIndex
        if text[index] == "+" || text[index] == "-" { index = text.index(after: index) }
        while index < text.endIndex {
            let character = text[index]
            if character.isNumber {
                sawDigit = true
            } else if character == "." {
                guard !sawDot else { return false }
                sawDot = true
            } else if character == "e" || character == "E" {
                return sawDigit && isExponent(text[text.index(after: index)...])
            } else {
                return false
            }
            index = text.index(after: index)
        }
        return sawDigit
    }

    static func isExponent(_ rest: Substring) -> Bool {
        var digits = rest
        var sawDigit = false
        if digits.first == "+" || digits.first == "-" { digits = digits.dropFirst() }
        for character in digits {
            guard character.isNumber else { return false }
            sawDigit = true
        }
        return sawDigit
    }

    // MARK: Search

    /// Whether any cell of the row contains the term, case-insensitively.
    ///
    /// A nil cell is skipped — a NULL contains nothing — and the term is trimmed first, so a stray
    /// space does not hide every row. Case-insensitive with the locale's own rules, which is what
    /// someone typing a fragment of a name expects; the server escalation folds with `LOWER` for
    /// the same reason.
    static func matches(_ row: [String?], term: String) -> Bool {
        let needle = term.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !needle.isEmpty else { return true }
        for value in row {
            guard let value else { continue }
            if value.localizedCaseInsensitiveContains(needle) { return true }
        }
        return false
    }

    // MARK: Filters

    /// The distinct values offered for a column, in a stable order: `nil` first because a NULL is
    /// its own state and not a value, then the non-nulls sorted so the list does not reshuffle
    /// between runs. Only the first `valuePickerLimit + 1` are needed to make the decision, but the
    /// full set is cheap over at most a preview's worth of rows.
    static func distinctValues(in rows: [[String?]], column: Int) -> [String?] {
        var seen = Set<String>()
        var hasNull = false
        for row in rows {
            guard column < row.count, let value = row[column] else {
                hasNull = true
                continue
            }
            seen.insert(value)
        }
        return (hasNull ? [nil] : []) + seen.sorted()
    }

    static func matches(_ filter: ColumnFilter, _ value: String?) -> Bool {
        switch filter {
        case .values(let picked):
            guard let value else { return picked.contains(ColumnFilter.nullToken) }
            return picked.contains(value)
        case .text(let needle):
            return matchesText(value, needle)
        }
    }

    static func matchesText(_ value: String?, _ filter: String) -> Bool {
        let needle = filter.trimmingCharacters(in: .whitespaces)
        guard !needle.isEmpty else { return true }
        // A NULL is not a value that can contain anything, and it is not the empty string either.
        guard let value else { return false }

        for op in [">=", "<=", ">", "<", "="] where needle.hasPrefix(op) {
            let operand = String(needle.dropFirst(op.count)).trimmingCharacters(in: .whitespaces)
            guard !operand.isEmpty else { break }
            if op == "=" { return value.localizedCaseInsensitiveCompare(operand) == .orderedSame }
            // Text ordering when either side is not a number, so a filter on a date column still
            // does something instead of matching nothing.
            if let left = Double(value), let right = Double(operand) {
                switch op {
                case ">=": return left >= right
                case "<=": return left <= right
                case ">": return left > right
                default: return left < right
                }
            }
            switch op {
            case ">=": return value >= operand
            case "<=": return value <= operand
            case ">": return value > operand
            default: return value < operand
            }
        }
        return value.localizedCaseInsensitiveContains(needle)
    }
}
