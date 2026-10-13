import Foundation

/// When an identifier needs quotes, and how it is quoted, for the statements this app writes
/// itself (the column list of an opened table, a suggestion offered in the editor).
///
/// **Quote only when required.** A plain name is `[a-z_][a-z0-9_]*` and not a reserved word of the
/// dialect. Everything else is quoted: upper case (PostgreSQL and Trino fold an unquoted name to
/// lower case, so `"Name"` and `name` are different columns), a leading digit, a space, a hyphen or
/// any other symbol, a non-ASCII letter, the empty name, and a reserved word. A quote character
/// inside a name is doubled, `"` for Trino and PostgreSQL and `` ` `` for MySQL.
///
/// **Reserved words are a superset on purpose.** Quoting a word that did not need it is harmless
/// and leaving one bare is a syntax error, so each list is the words that any recent version of
/// the dialect refuses as a bare column name, plus the ANSI words all three share. A word that is
/// only non-reserved (`date`, `status`, `type`, `name`) stays bare: every one of these servers
/// accepts it as a column.
///
/// Not the same thing as `quotedIdent`, which always quotes and does not escape: that one is how the
/// grid's write-back spells a column it already knows is safe to quote, and it is left as it is.
enum SQLIdentifierQuoting {
    static func needsQuotes(_ name: String, for kind: ConnectionKind) -> Bool {
        guard isPlain(name) else { return true }
        return reserved(for: kind).contains(name)
    }

    /// The name as it is written in a statement: bare when it can be, quoted with the dialect's
    /// quote character (doubled inside the name) when it cannot.
    static func quoted(_ name: String, for kind: ConnectionKind) -> String {
        guard needsQuotes(name, for: kind) else { return name }
        let mark = kind == .mysql ? "`" : "\""
        return mark + name.replacingOccurrences(of: mark, with: mark + mark) + mark
    }

    /// Lower-case ASCII letters, digits and underscores, starting with a letter or an underscore.
    private static func isPlain(_ name: String) -> Bool {
        guard let first = name.unicodeScalars.first else { return false }
        guard first == "_" || (first >= "a" && first <= "z") else { return false }
        return name.unicodeScalars.allSatisfy { $0 == "_" || ($0 >= "a" && $0 <= "z") || ($0 >= "0" && $0 <= "9") }
    }

    private static func reserved(for kind: ConnectionKind) -> Set<String> {
        switch kind {
        case .trino: ansi.union(trinoExtra)
        case .postgres: ansi.union(postgresExtra)
        case .mysql: ansi.union(mysqlExtra)
        }
    }

    /// The words every one of the three refuses as a bare column name.
    private static let ansi: Set<String> = [
        "all", "alter", "and", "any", "array", "as", "asc", "between", "both", "by", "case", "cast", "check",
        "collate", "column", "constraint", "create", "cross", "current_date", "current_time",
        "current_timestamp", "current_user", "default", "delete", "desc", "describe", "distinct", "do",
        "drop", "else", "end", "escape", "except", "exists", "false", "fetch", "for", "foreign", "from",
        "full", "grant", "group", "having", "in", "inner", "insert", "intersect", "into", "is", "join",
        "leading", "left", "like", "limit", "localtime", "localtimestamp", "natural", "not", "null",
        "offset", "on", "only", "or", "order", "outer", "primary", "references", "right", "select",
        "some", "table", "then", "to", "trailing", "true", "union", "unique", "user", "using", "values",
        "when", "where", "window", "with",
    ]

    private static let trinoExtra: Set<String> = [
        "cube", "deallocate", "execute", "extract", "grouping", "normalize", "of", "prepare", "recursive",
        "rollup", "uescape", "unnest",
    ]

    private static let postgresExtra: Set<String> = [
        "analyse", "analyze", "asymmetric", "authorization", "binary", "collation", "concurrently",
        "current_catalog", "current_role", "current_schema", "deferrable", "freeze", "ilike", "initially",
        "isnull", "lateral", "notnull", "overlaps", "placing", "returning", "session_user", "similar",
        "symmetric", "tablesample", "variadic", "verbose",
    ]

    private static let mysqlExtra: Set<String> = [
        "accessible", "add", "before", "bigint", "blob", "call", "cascade", "change", "char", "character",
        "condition", "continue", "convert", "cume_dist", "database", "databases", "day_hour",
        "day_microsecond", "day_minute", "day_second", "dec", "decimal", "declare", "delayed",
        "dense_rank", "deterministic", "div", "double", "each", "elseif", "empty", "enclosed", "exit",
        "explain", "first_value", "float", "float4", "float8", "force", "fulltext", "function",
        "generated", "get", "groups", "high_priority", "hour_microsecond", "hour_minute", "hour_second",
        "if", "ignore", "index", "infile", "inout", "int", "int1", "int2", "int3", "int4", "int8", "integer",
        "interval", "iterate", "json_table", "key", "keys", "kill", "lag", "last_value", "lead", "leave",
        "lines", "load", "lock", "long", "longblob", "longtext", "loop", "low_priority", "match",
        "mediumblob", "mediumint", "mediumtext", "middleint", "minute_microsecond", "minute_second", "mod",
        "modifies", "no_write_to_binlog", "nth_value", "ntile", "numeric", "of", "optimize", "option",
        "optionally", "out", "outfile", "over", "partition", "percent_rank", "precision", "procedure",
        "purge", "range", "rank", "read", "reads", "real", "recursive", "regexp", "release", "rename",
        "repeat", "replace", "require", "resignal", "restrict", "return", "revoke", "rlike", "row",
        "row_number", "rows", "schema", "schemas", "second_microsecond", "sensitive", "separator", "set",
        "show", "signal", "smallint", "spatial", "specific", "sql", "sqlexception", "sqlstate",
        "sqlwarning", "sql_big_result", "sql_calc_found_rows", "sql_small_result", "ssl", "starting",
        "stored", "straight_join", "system", "terminated", "tinyblob", "tinyint", "tinytext", "trigger",
        "undo", "unlock", "unsigned", "update", "usage", "use", "utc_date", "utc_time", "utc_timestamp",
        "varbinary", "varchar", "varcharacter", "varying", "virtual", "while", "write", "xor",
        "year_month", "zerofill",
    ]
}
