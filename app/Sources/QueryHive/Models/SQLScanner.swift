import Foundation

/// Where a position sits in SQL: in code, or inside something that only looks like it.
///
/// One pass over the text, mirroring the states `crates/qh-sql/src/scan.rs` already uses:
/// single-quoted strings, double-quoted and backtick identifiers, line and block comments, and
/// `$tag$ ... $tag$` bodies. Two callers need it and they need the same answer — the `:name`
/// parameter flow, which must not read a colon inside a string, and the keyword pass, which must not
/// rewrite a word inside one — so the scan happens once and both read the result.
///
/// # What it deliberately does not understand
///
/// A backslash escape: PostgreSQL's `E'...'` and MySQL's strings, where `\'` does not end the
/// string. `'it\'s :x'` is therefore read as ending at the second quote, and a `:x` after it would
/// look like a parameter. The engine's own scanner has the same limit, and it is stated here rather
/// than hidden behind a promise the scanner cannot keep.
struct SQLScanner {
    /// One `:name` occurrence, with the range that would be replaced.
    struct Parameter: Equatable {
        let name: String
        let range: NSRange
    }

    /// The ranges that are not code, in the order they were found.
    let opaque: [NSRange]
    /// The parameters, in the order they appear.
    let parameters: [Parameter]

    /// Whether the UTF-16 offset is in code.
    func isCode(at offset: Int) -> Bool {
        !opaque.contains { offset >= $0.location && offset < $0.location + $0.length }
    }

    /// The parameter names, once each, in the order they first appear.
    var parameterNames: [String] {
        var seen = Set<String>()
        return parameters.compactMap { seen.insert($0.name).inserted ? $0.name : nil }
    }

    static func scan(_ sql: String) -> SQLScanner {
        let units = Array(sql.utf16)
        let length = units.count
        var opaque: [NSRange] = []
        var parameters: [Parameter] = []

        /// The index just past `from`, or `length` when it does not occur.
        func find(_ text: [unichar], from: Int, until end: Int) -> Int? {
            guard !text.isEmpty, from + text.count <= end else { return nil }
            var index = from
            while index + text.count <= end {
                if Array(units[index..<(index + text.count)]) == text { return index + text.count }
                index += 1
            }
            return nil
        }

        var index = 0
        // Bracket depth, so `arr[lo:hi]` is a slice and not a parameter named `hi`.
        var depth = 0
        while index < length {
            let unit = units[index]

            if unit == openBracket { depth += 1; index += 1; continue }
            if unit == closeBracket { depth = max(0, depth - 1); index += 1; continue }

            // A line comment: to the end of the line.
            if unit == dash, index + 1 < length, units[index + 1] == dash {
                var end = index
                while end < length, units[end] != newline { end += 1 }
                opaque.append(NSRange(location: index, length: end - index))
                index = end
                continue
            }

            // A block comment: to its close, or to the end of the text.
            if unit == slash, index + 1 < length, units[index + 1] == star {
                let close = find([star, slash], from: index + 2, until: length) ?? length
                opaque.append(NSRange(location: index, length: close - index))
                index = close
                continue
            }

            // A quoted run: a string, or a quoted identifier. Both end at their own quote, and both
            // take a doubled quote as an escape rather than as the end.
            if let quote = quotedRun(startingAt: unit) {
                var end = index + 1
                while end < length {
                    if units[end] == quote {
                        if end + 1 < length, units[end + 1] == quote {
                            end += 2
                            continue
                        }
                        end += 1
                        break
                    }
                    end += 1
                }
                opaque.append(NSRange(location: index, length: end - index))
                index = end
                continue
            }

            // A `$tag$ ... $tag$` body, which is where a function's SQL lives. `$1` is not one:
            // the tag has to close with a second `$`.
            if unit == dollar, let tag = dollarTag(at: index, in: units) {
                let close = find(tag, from: index + tag.count, until: length) ?? length
                opaque.append(NSRange(location: index, length: close - index))
                index = close
                continue
            }

            // `:name`, in code and not after another colon: `a::text` is a cast, and `:=` is an
            // assignment, and neither is a parameter.
            if unit == colon, depth == 0, index + 1 < length,
               !(index > 0 && units[index - 1] == colon),
               isNameStart(units[index + 1]) {
                var end = index + 2
                while end < length, isNameBody(units[end]) { end += 1 }
                let name = String(utf16CodeUnits: Array(units[(index + 1)..<end]), count: end - index - 1)
                parameters.append(Parameter(name: name,
                                            range: NSRange(location: index, length: end - index)))
                index = end
                continue
            }

            index += 1
        }

        return SQLScanner(opaque: opaque, parameters: parameters)
    }

    /// The quote that starts a quoted run at this unit, or `nil`.
    private static func quotedRun(startingAt unit: unichar) -> unichar? {
        if unit == singleQuote || unit == doubleQuote || unit == backtick { return unit }
        return nil
    }

    /// The `$tag$` that opens a dollar body at `index`, including its closing `$`.
    ///
    /// `$1` and `$` alone are not tags: the opening has to be `$`, a name or nothing, and another
    /// `$`. That is what keeps a PostgreSQL parameter placeholder out of this.
    private static func dollarTag(at index: Int, in units: [unichar]) -> [unichar]? {
        var end = index + 1
        while end < units.count, isNameBody(units[end]) { end += 1 }
        guard end < units.count, units[end] == dollar else { return nil }
        return Array(units[index...end])
    }

    private static func isNameStart(_ unit: unichar) -> Bool {
        (unit >= 65 && unit <= 90) || (unit >= 97 && unit <= 122) || unit == underscore
    }

    private static func isNameBody(_ unit: unichar) -> Bool {
        isNameStart(unit) || (unit >= 48 && unit <= 57)
    }

    private static let newline = unichar(10)
    private static let dollar = unichar(36)
    private static let singleQuote = unichar(39)
    private static let doubleQuote = unichar(34)
    private static let backtick = unichar(96)
    private static let dash = unichar(45)
    private static let slash = unichar(47)
    private static let star = unichar(42)
    private static let colon = unichar(58)
    private static let underscore = unichar(95)
    private static let openBracket = unichar(91)
    private static let closeBracket = unichar(93)
}
