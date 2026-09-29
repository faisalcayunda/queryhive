import Foundation

/// A sort the result grid is applying, as a value the tests can reach.
///
/// The grid holds `[String?]`, so a sort has to settle two questions a column type would have
/// settled for it: what a NULL does, and whether two cells are numbers or text. Both are written
/// down here rather than left to `sorted(by:)`, because a grid that puts "10" before "9" is a grid
/// whose sort nobody trusts the second time.
///
/// This is an **in-memory** order over the rows already fetched. It does not re-run the query and
/// does not touch the statement, which is why the header has to say so: a row outside the limit is
/// not in this order at all.
struct GridSort: Equatable {
    enum Direction: String, Equatable {
        case ascending, descending

        /// The glyph the header draws beside the sorted column.
        var symbol: String { self == .ascending ? "chevron.up" : "chevron.down" }
    }

    /// The column, by position. A result may repeat a name and the grid draws by position.
    var column: Int
    var direction: Direction

    /// What a click on a column header does: an unsorted grid starts ascending, a second click
    /// reverses it, and a third clears back to the server's own order.
    ///
    /// The third state exists because the server's order is the only one that is not the grid's
    /// invention, and a click that could only ever add an order would leave no way back to the rows
    /// as they arrived.
    static func next(_ current: GridSort?, clickedColumn: Int,
                     firstDirection: Direction = .ascending) -> GridSort? {
        guard let current, current.column == clickedColumn else {
            return GridSort(column: clickedColumn, direction: firstDirection)
        }
        // The cycle is: the first direction, then the other, then off. Which one is "first" is the
        // Data pane's, and it also decides where the cycle ends — a fixed `ascending → descending →
        // off` would make a descending start unable to reach ascending at all, because its second
        // click would clear instead of flipping.
        let other: Direction = firstDirection == .ascending ? .descending : .ascending
        return current.direction == firstDirection
            ? GridSort(column: clickedColumn, direction: other)
            : nil
    }

    /// The rows in the order the grid draws them.
    ///
    /// Stable on purpose: rows whose key compares equal keep the order they arrived in, because
    /// reversing a sort and reversing it back should give the server's order among ties. Swift's
    /// `sorted(by:)` promises no such thing, so the original index is the tiebreak.
    func order(_ rows: [[String?]]) -> [[String?]] {
        rows.enumerated()
            .sorted { left, right in
                let result = GridSort.compare(GridSort.value(in: left.element, at: column),
                                              GridSort.value(in: right.element, at: column))
                if result == .orderedSame { return left.offset < right.offset }
                return (result == .orderedAscending) == (direction == .ascending)
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
    private static func isPlainNumber(_ text: String) -> Bool {
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

    private static func isExponent(_ rest: Substring) -> Bool {
        var digits = rest
        var sawDigit = false
        if digits.first == "+" || digits.first == "-" { digits = digits.dropFirst() }
        for character in digits {
            guard character.isNumber else { return false }
            sawDigit = true
        }
        return sawDigit
    }
}
