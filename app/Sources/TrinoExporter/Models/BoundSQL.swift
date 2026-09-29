import Foundation

/// How a driver spells a bound parameter, or that it has none.
///
/// Mirrors the engine's `Capabilities::parameters` (`crates/qh-driver/src/lib.rs`).
/// The engine is the contract — `ParameterStyle::Dollar` there is PostgreSQL's
/// `$1` and `Question` is MySQL's `?` — and this is the app's copy of that
/// declaration, kept beside `ConnectionKind.levels` for the same reason: the FFI
/// does not expose `Capabilities` to Swift, so a statement is built for a driver
/// from the same fact the driver would answer with. A fourth driver means a case
/// here and a variant there, kept in step.
enum ParameterStyle: Equatable {
    /// The driver cannot bind. Values are written into the SQL, which is what
    /// Trino's HTTP protocol requires because it has no placeholders at all.
    case inline
    /// `$1`, `$2`, … — PostgreSQL.
    case dollar
    /// `?`, one per value in order — MySQL.
    case question

    static func forKind(_ kind: ConnectionKind) -> ParameterStyle {
        switch kind {
        case .trino: .inline
        case .postgres: .dollar
        case .mysql: .question
        }
    }

    /// The placeholder for the value at `index` (0-based). Unused when inline.
    func placeholder(_ index: Int) -> String {
        switch self {
        case .inline: ""
        case .dollar: "$\(index + 1)"
        case .question: "?"
        }
    }
}

/// One value the engine binds, typed so the driver can send it natively.
///
/// The cases mirror `qh_driver::Parameter`: a closed set of shapes every client
/// can send. A value outside it (an exact decimal, a timestamp) is kept as text
/// and the server parses it — which is still correct, and still not escaped by
/// this app.
enum BindValue: Equatable {
    case null
    case bool(Bool)
    case int(Int64)
    case uint(UInt64)
    case double(Double)
    case text(String)

    /// The typed value for a cell, from the column's own type name.
    ///
    /// The type comes from the server's column description, the same source
    /// `UpdateStatements.literal` uses, so the two cannot disagree about whether
    /// a value is a number. Exact decimals stay `text` on purpose: binding them
    /// as a `Double` would round a `NUMERIC(38,10)` invisibly, and the server
    /// parses the text into the exact type anyway.
    static func from(text: String, type: String) -> BindValue {
        let name = MatchPolicy.base(type)
        if name == "bool" || name == "boolean" {
            switch text.lowercased() {
            case "true", "t": return .bool(true)
            case "false", "f": return .bool(false)
            default: return .text(text)
            }
        }
        if ["real", "float", "float4", "float8", "double", "double precision"].contains(name) {
            return Double(text).map(BindValue.double) ?? .text(text)
        }
        if UpdateStatements.isNumeric(type) {
            if let value = Int64(text) { return .int(value) }
            if let value = UInt64(text) { return .uint(value) }
            return .text(text)
        }
        return .text(text)
    }

    /// The `{"type": …, "value": …}` object `apply_changes` reads.
    ///
    /// Every value is a string, so a `bigint` and a decimal-looking `VARCHAR` are
    /// not confused by JSON's own number handling; the type tag says which the
    /// engine should parse it as.
    var json: [String: Any] {
        switch self {
        case .null: ["type": "null"]
        case .bool(let value): ["type": "bool", "value": value ? "true" : "false"]
        case .int(let value): ["type": "int", "value": String(value)]
        case .uint(let value): ["type": "uint", "value": String(value)]
        case .double(let value): ["type": "float", "value": String(value)]
        case .text(let value): ["type": "text", "value": value]
        }
    }
}

/// A statement being built: SQL text and the values that fill its holes.
///
/// It writes **two** renderings from one pass, and that is the point: `sql` is
/// what the engine runs (placeholders when the driver binds), `display` is what
/// the review sheet shows (every value written in). They cannot drift, because a
/// value is appended to both in the same call. Trino, which cannot bind, gets
/// `sql == display` and no parameters — exactly the inline statement it always got.
struct BoundSQL: Equatable {
    let style: ParameterStyle
    /// The executable form: placeholders where the driver binds.
    private(set) var sql = ""
    /// The review form: the same values inline, so the user approves the numbers,
    /// not a row of `$1`.
    private(set) var display = ""
    /// The values, in placeholder order. Empty when the driver is inline.
    private(set) var parameters: [BindValue] = []

    init(style: ParameterStyle) {
        self.style = style
    }

    /// SQL syntax: identifiers, operators, keywords, a `DEFAULT`, an `IS NULL`.
    /// Nothing here is a user value, so nothing here is ever bound.
    mutating func text(_ literal: String) {
        sql += literal
        display += literal
    }

    /// One value, bound when the driver can and written into the display always.
    ///
    /// `display` is the literal the value would be if inlined (`UpdateStatements.literal`),
    /// so the review form stays what it was before binding existed.
    mutating func value(_ bound: BindValue, display literal: String) {
        if style != .inline {
            parameters.append(bound)
            sql += style.placeholder(parameters.count - 1)
        } else {
            sql += literal
        }
        display += literal
    }

    /// The `"type":…` objects for the `CHANGES` payload, or `nil` when nothing is
    /// bound — an inline statement carries no `params` at all.
    var parametersJSON: [[String: Any]]? {
        parameters.isEmpty ? nil : parameters.map(\.json)
    }
}
