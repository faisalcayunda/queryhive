import Foundation

/// What a `:name` parameter is taken to be.
///
/// Chosen in the prompt rather than inferred: a parameter has no column type behind it, and guessing
/// one would be guessing at what the statement means. The set is the shapes every dialect here
/// spells the same way.
enum ParameterKind: String, CaseIterable, Identifiable {
    case text, number, boolean, date, timestamp, null
    /// Several values for `id IN (:ids)`; the entry's `element` says what each one is.
    case list

    var id: String { rawValue }

    /// What a list's elements can be: any single value that is not itself NULL or a list.
    static let elements: [ParameterKind] = [.text, .number, .boolean, .date, .timestamp]

    var title: String {
        switch self {
        case .text: "Text"
        case .number: "Number"
        case .boolean: "Boolean"
        case .date: "Date"
        case .timestamp: "Timestamp"
        case .null: "NULL"
        case .list: "List"
        }
    }
}

/// One parameter's answer: what it is, and what the user typed.
struct ParameterEntry: Equatable {
    var kind: ParameterKind = .text
    var text: String = ""
    /// What each element of a `.list` is. Unused by every other kind.
    var element: ParameterKind = .text

    /// The value a fresh prompt starts with, so the common case needs no thought.
    static let empty = ParameterEntry(kind: .text, text: "")
}

/// The values a `:name` statement needs, while the prompt is up.
struct ParameterPrompt: Identifiable {
    let id = UUID()
    let tabID: UUID
    let source: QuerySource
    /// The statement as written, with `:name` still in it.
    let template: String
    /// The names, in the order they first appear.
    let names: [String]
    /// The driver, which decides how a text value carrying a backslash is written.
    let kind: ConnectionKind
    /// What the prompt starts with: this tab's last values for these names, or empty.
    let initial: [String: ParameterEntry]

    /// A prompt for this statement, or `nil` when it has no parameters.
    static func request(tab: QueryTab, source: QuerySource, sql: String,
                        kind: ConnectionKind) -> ParameterPrompt? {
        let scan = SQLScanner.scan(sql, driver: kind)
        guard !scan.parameters.isEmpty else { return nil }
        let names = scan.parameterNames
        var initial: [String: ParameterEntry] = [:]
        for name in names { initial[name] = tab.parameterValues[name] ?? .empty }
        return ParameterPrompt(tabID: tab.id, source: source, template: sql, names: names,
                               kind: kind, initial: initial)
    }
}

/// Why a value could not be written, in the words of the person who has to fix it.
struct ParameterError: Error, Equatable {
    let message: String
}

/// Turning `:name` into SQL text.
///
/// **Substitution, not binding.** ADR-0028 gives this job to the caller for a driver that cannot
/// bind, and no driver can bind the statement a user most wants this for: PostgreSQL refuses a bound
/// statement that returns rows, Trino has no placeholders at all, and the `preview` path never
/// reaches `execute_bound`. Binding on MySQL alone would mean different semantics per connection and
/// a history whose text is no longer what ran. So the values are written in, and the statement that
/// results is the statement that runs.
///
/// Every value reaches SQL through [`literal`], and it answers `nil` rather than a string it cannot
/// represent. That is the rule that keeps "substituted" from drifting into "escaped by hand": there
/// is no second path, no raw fragment, and no identifier parameter.
enum ParameterRender {
    /// The literal for one value, or `nil` when it cannot be written safely for this driver.
    static func literal(_ entry: ParameterEntry, driver: ConnectionKind) -> String? {
        switch entry.kind {
        case .null:
            return "NULL"

        // A list is not one literal; `list` writes it, one `literal` per element.
        case .list:
            return nil

        case .boolean:
            switch entry.text.trimmingCharacters(in: .whitespaces).lowercased() {
            case "true", "t", "1": return "TRUE"
            case "false", "f", "0": return "FALSE"
            default: return nil
            }

        case .number:
            guard UpdateStatements.isNumber(entry.text) else { return nil }
            let trimmed = entry.text.trimmingCharacters(in: .whitespaces)
            // A negative goes in brackets: `x - :n` with `n = -5` is otherwise `x --5`, and that is
            // a line comment, not a subtraction.
            return trimmed.hasPrefix("-") ? "(\(trimmed))" : trimmed

        case .date:
            let trimmed = entry.text.trimmingCharacters(in: .whitespaces)
            guard isDate(trimmed) else { return nil }
            // `DATE '...'` rather than a bare string: Trino will not compare a date column against
            // a varchar, and every dialect here reads the typed literal.
            return "DATE '\(trimmed)'"

        case .timestamp:
            let trimmed = entry.text.trimmingCharacters(in: .whitespaces)
            guard isTimestamp(trimmed) else { return nil }
            return "TIMESTAMP '\(trimmed)'"

        case .text:
            guard !entry.text.contains("\0") else { return nil }
            guard entry.text.contains("\\") else { return quoted(entry.text) }
            switch driver {
            case .postgres:
                // `E'...'` with the backslash doubled reads the same whether or not
                // `standard_conforming_strings` is on, which is the one form that is right in both.
                return "E'" + entry.text
                    .replacingOccurrences(of: "\\", with: "\\\\")
                    .replacingOccurrences(of: "'", with: "''") + "'"
            case .mysql:
                // A backslash means whatever `sql_mode` says it means, and this app cannot see that
                // setting. Refused rather than guessed.
                return nil
            case .trino:
                // Trino has no backslash escapes at all, so the text is written as it is.
                return quoted(entry.text)
            }
        }
    }

    /// The statement with every parameter written in, or the first reason it cannot be.
    static func statement(_ template: String,
                          entries: [String: ParameterEntry],
                          driver: ConnectionKind) -> Result<String, ParameterError> {
        let scan = SQLScanner.scan(template, driver: driver)
        guard !scan.parameters.isEmpty else { return .success(template) }

        let text = template as NSString
        var out = ""
        var cursor = 0
        for parameter in scan.parameters {
            let entry = entries[parameter.name] ?? .empty
            let literal: String
            if entry.kind == .list {
                switch list(parameter.name, entry: entry, driver: driver) {
                case .success(let written): literal = written
                case .failure(let error): return .failure(error)
                }
            } else if let written = self.literal(entry, driver: driver) {
                literal = written
            } else {
                return .failure(reason(parameter.name, entry: entry, driver: driver))
            }
            out += text.substring(with: NSRange(location: cursor,
                                                length: parameter.range.location - cursor))
            out += literal
            cursor = parameter.range.location + parameter.range.length
        }
        out += text.substring(from: cursor)
        return .success(out)
    }

    /// The most elements one list takes: a longer one is a pasted table, not a parameter.
    static let listLimit = 10_000

    /// A list's elements written as `a, b, c`, to sit inside the statement's own parentheses.
    ///
    /// Elements are separated by commas or new lines. One that holds a comma or a quote is written
    /// `'like, this'` with `''` for a quote inside. Every element goes through [`literal`], so a list
    /// is exactly as safe as its elements. An empty list, a malformed one (an empty element, an
    /// unclosed quote, text after a closing quote) and a failing element are errors: `IN ()` is a
    /// syntax error on every server here, and `IN (NULL)` would run and quietly match nothing.
    static func list(_ name: String, entry: ParameterEntry,
                     driver: ConnectionKind) -> Result<String, ParameterError> {
        func fail(_ why: String) -> Result<String, ParameterError> {
            .failure(ParameterError(message: "\(name) \(why)"))
        }
        guard ParameterKind.elements.contains(entry.element) else {
            return fail("is a list of a type that cannot be an element.")
        }
        var elements: [String] = []
        let chars = Array(entry.text.trimmingCharacters(in: .whitespacesAndNewlines))
        var index = 0
        while index < chars.count {
            while index < chars.count, chars[index] == " " || chars[index] == "\t" { index += 1 }
            var element = ""
            if index < chars.count, chars[index] == "'" {
                index += 1
                var closed = false
                while index < chars.count {
                    if chars[index] == "'" {
                        if index + 1 < chars.count, chars[index + 1] == "'" {
                            element.append("'")
                            index += 2
                            continue
                        }
                        closed = true
                        index += 1
                        break
                    }
                    element.append(chars[index])
                    index += 1
                }
                guard closed else { return fail("has a quote that is never closed.") }
                while index < chars.count, chars[index] == " " || chars[index] == "\t" { index += 1 }
                guard index == chars.count || chars[index] == "," || chars[index].isNewline else {
                    return fail("has text after a closing quote. Separate elements with commas.")
                }
            } else {
                while index < chars.count, chars[index] != ",", !chars[index].isNewline {
                    element.append(chars[index])
                    index += 1
                }
                element = element.trimmingCharacters(in: .whitespaces)
                guard !element.isEmpty else { return fail("has an empty element.") }
            }
            elements.append(element)
            if index < chars.count {
                // A comma with nothing after it is an empty last element.
                if index + 1 == chars.count { return fail("has an empty element.") }
                index += 1
            }
        }
        guard !elements.isEmpty else { return fail("is an empty list. Give it at least one value.") }
        guard elements.count <= listLimit else {
            return fail("has \(elements.count) elements; the most a list takes is \(listLimit).")
        }
        var written: [String] = []
        for (position, element) in elements.enumerated() {
            let single = ParameterEntry(kind: entry.element, text: element)
            guard let text = literal(single, driver: driver) else {
                let which = "element \(position + 1) (\(element))"
                return .failure(ParameterError(
                    message: "\(name): " + reason(which, entry: single, driver: driver).message))
            }
            written.append(text)
        }
        return .success(written.joined(separator: ", "))
    }

    /// Why a value could not be written.
    static func reason(_ name: String, entry: ParameterEntry,
                       driver: ConnectionKind) -> ParameterError {
        switch entry.kind {
        case .number:
            return ParameterError(message:
                "\(name) is not a number. Write it as one — 42, -3.5, 1e9 — or change its type.")
        case .boolean:
            return ParameterError(message:
                "\(name) is not a boolean. Write true or false, or change its type.")
        case .date:
            return ParameterError(message:
                "\(name) is not a date. Write it as YYYY-MM-DD, or change its type.")
        case .timestamp:
            return ParameterError(message:
                "\(name) is not a timestamp. Write it as YYYY-MM-DD HH:MM[:SS], or change its type.")
        case .text where entry.text.contains("\0"):
            return ParameterError(message:
                "\(name) contains a null character, which no dialect here can carry.")
        case .text where entry.text.contains("\\") && driver == .mysql:
            return ParameterError(message:
                "\(name) contains a backslash, and MySQL's meaning for one depends on its sql_mode, "
                + "which this app cannot see. Remove it, or use PostgreSQL or Trino.")
        case .text:
            return ParameterError(message: "\(name) cannot be written as a literal.")
        case .null:
            return ParameterError(message: "\(name) is NULL, which cannot be written as a literal.")
        case .list:
            return ParameterError(message: "\(name) is a list, which is written element by element.")
        }
    }

    private static func quoted(_ text: String) -> String {
        "'" + text.replacingOccurrences(of: "'", with: "''") + "'"
    }

    /// `YYYY-MM-DD`. The shape only: whether the day exists is the server's to say, and a date that
    /// is the right shape makes a statement the server can answer.
    static func isDate(_ text: String) -> Bool {
        let parts = text.split(separator: "-", omittingEmptySubsequences: false)
        return parts.count == 3
            && parts[0].count == 4 && parts[1].count == 2 && parts[2].count == 2
            && parts.allSatisfy { $0.allSatisfy { $0.isASCII && $0.isNumber } }
    }

    /// `YYYY-MM-DD HH:MM`, `YYYY-MM-DDTHH:MM:SS`, and the fractional seconds that go with either.
    static func isTimestamp(_ text: String) -> Bool {
        let split = text.split(whereSeparator: { $0 == " " || $0 == "T" })
        guard split.count == 2, isDate(String(split[0])) else { return false }
        let clock = split[1].split(separator: ":", omittingEmptySubsequences: false)
        guard clock.count == 2 || clock.count == 3 else { return false }
        guard clock[0].count == 2, clock[1].count == 2,
              clock[0].allSatisfy({ $0.isASCII && $0.isNumber }),
              clock[1].allSatisfy({ $0.isASCII && $0.isNumber }) else { return false }
        if clock.count == 3 {
            let seconds = clock[2].split(separator: ".", omittingEmptySubsequences: false)
            guard let whole = seconds.first, whole.count == 2,
                  whole.allSatisfy({ $0.isASCII && $0.isNumber }) else { return false }
            if seconds.count == 2, seconds[1].isEmpty { return false }
            if seconds.count > 1, !seconds[1].allSatisfy({ $0.isASCII && $0.isNumber }) {
                return false
            }
            if seconds.count > 2 { return false }
        }
        return true
    }
}
