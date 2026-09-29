import Foundation

/// What a `:name` parameter is taken to be.
///
/// Chosen in the prompt rather than inferred: a parameter has no column type behind it, and guessing
/// one would be guessing at what the statement means. The set is the shapes every dialect here
/// spells the same way.
enum ParameterKind: String, CaseIterable, Identifiable {
    case text, number, boolean, date, timestamp, null

    var id: String { rawValue }

    var title: String {
        switch self {
        case .text: "Text"
        case .number: "Number"
        case .boolean: "Boolean"
        case .date: "Date"
        case .timestamp: "Timestamp"
        case .null: "NULL"
        }
    }
}

/// One parameter's answer: what it is, and what the user typed.
struct ParameterEntry: Equatable {
    var kind: ParameterKind = .text
    var text: String = ""

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
        let scan = SQLScanner.scan(sql)
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
        let scan = SQLScanner.scan(template)
        guard !scan.parameters.isEmpty else { return .success(template) }

        let text = template as NSString
        var out = ""
        var cursor = 0
        for parameter in scan.parameters {
            let entry = entries[parameter.name] ?? .empty
            guard let literal = literal(entry, driver: driver) else {
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
