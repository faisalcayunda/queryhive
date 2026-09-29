import Foundation

/// How one column may be used to identify a row when the table has no usable key.
///
/// QueryHive carries no primary-key metadata, so a row is identified by **every
/// column at the value it was fetched with** (`UpdateStatements`). The source
/// study paid for that rule twice, and this type is where both holes are closed,
/// not worked around:
///
/// 1. **Some types cannot be compared by value at all.** A `bytea`, a geometry, a
///    nested type: including it in the predicate makes the `WHERE` match nothing.
///    That looks exactly like a successful save whose edit is gone — the study's
///    MySQL `FLOAT`/`JSON` bug, generalised. Those columns are `excluded`, and
///    the plan says which, rather than emitting a predicate that silently runs
///    against a value the server cannot compare.
/// 2. **For some types the text the grid read is the server's own rendering**,
///    not the stored value, so a direct comparison can fail. Those columns are
///    compared through the server's rendering of the column instead of the
///    literal: `CONCAT(col)` on MySQL (the spelling the study names), `col::text`
///    on PostgreSQL, `CAST(col AS varchar)` on Trino.
///
/// The rule is enforced in `UpdateStatements.match`, which is the only place a
/// `WHERE` for a fetched row is built. Nothing here widens or narrows a predicate
/// quietly: an excluded column is reported with the statement, and a row whose
/// every column is excluded yields no predicate at all rather than a bare
/// `DELETE`/`UPDATE` that would match the whole table.
enum MatchPolicy: Equatable {
    /// Compare the column directly: `col = <literal>`.
    case match
    /// Compare the server's own text rendering of the column, so the text the
    /// grid read is what is compared.
    case serverText
    /// The column cannot be compared safely. It is left out of the predicate, and
    /// named in the plan.
    case excluded(String)

    /// The policy for one column, from the result's own type name.
    ///
    /// The type comes from the server's column description, not from guessing at
    /// the text, the same rule `UpdateStatements.literal` follows.
    static func forColumn(type: String, kind: ConnectionKind) -> MatchPolicy {
        let name = base(type)
        if let reason = excludedTypes[name] {
            return .excluded(reason)
        }
        if serverTextTypes.contains(name) {
            return .serverText
        }
        return .match
    }

    /// Types whose value cannot be reproduced as text and compared. A comparison
    /// either fails outright or is meaningless (two blobs, two geometries), so
    /// the column is left out of the predicate entirely.
    private static let excludedTypes: [String: String] = {
        var types: [String: String] = [:]
        for name in ["bytea", "blob", "binary", "varbinary", "tinyblob", "mediumblob",
                     "longblob", "image"] {
            types[name] = "binary values are not comparable as text"
        }
        for name in ["geometry", "geography", "point", "linestring", "polygon", "multipoint",
                     "multilinestring", "multipolygon", "geometrycollection"] {
            types[name] = "spatial values are not comparable as text"
        }
        for name in ["array", "list", "map", "row", "struct", "record"] {
            types[name] = "nested values are not comparable as text"
        }
        return types
    }()

    /// Types the grid reads as the server's rendering; the rendering is what is
    /// compared, not the typed value. `FLOAT`/`JSON` are the study's own examples.
    private static let serverTextTypes: Set<String> = [
        "real", "float", "float4", "float8", "double", "double precision",
        "json", "jsonb",
    ]

    /// A type name without its width or precision, lowercased.
    ///
    /// The servers spell the width into the name (`decimal(38,10)`,
    /// `character varying(16)`), and the policy is about the type, not the width.
    static func base(_ type: String) -> String {
        String(type.prefix { $0 != "(" })
            .trimmingCharacters(in: .whitespaces)
            .lowercased()
    }
}
