import Foundation

/// When an identifier needs quotes, and how it is quoted, for the statements this app writes
/// itself (the column list of an opened table; the editor's suggestions can use the same rule).
///
/// **Quote only when required.** A plain name is `[a-z_][a-z0-9_]*` and not a reserved word. Everything
/// else is quoted: upper case (PostgreSQL and Trino fold an unquoted name to lower case, so `"Name"`
/// and `name` are different columns), a leading digit, a space, a hyphen or any other symbol, a
/// non-ASCII letter, the empty name, and a reserved word. A quote character inside a name is doubled,
/// `"` for Trino and PostgreSQL and `` ` `` for MySQL.
///
/// **One list of reserved words, for every dialect.** It is the *union* of the reserved words of
/// MySQL 8.0 and 8.4, Trino and PostgreSQL (every "reserved" category in its key-words appendix,
/// including the ones that can be function or type names, the ones that require `AS`, and the SQL
/// value functions such as `system_user` that parse as a built-in and would silently return the
/// session's value). It is not "what each dialect refuses": a name that any of them reserves is
/// quoted for all of them. Quoting a word that did not need it is harmless, and a bare reserved word
/// is a syntax error or, worse, a different column, so a word reserved by one dialect is also quoted
/// in a statement for another (a column that is moved between databases keeps working).
/// A word that is only non-reserved (`date`, `status`, `type`, `name`, `value`) stays bare: every one
/// of these servers accepts it as a column.
///
/// Not the same thing as `quotedIdent`, which always quotes and does not escape: that one is how the
/// grid's write-back spells a column it already knows is safe to quote, and it is left as it is.
enum SQLIdentifierQuoting {
    static func needsQuotes(_ name: String, for kind: ConnectionKind) -> Bool {
        guard isPlain(name) else { return true }
        return reserved.contains(name)
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

    /// The union described above, from each project's own list of reserved words.
    static let reserved: Set<String> = [
        "accessible", "add", "all", "alter", "analyse", "analyze", "and", "any", "array", "as",
        "asc", "asensitive", "asymmetric", "authorization", "auto", "before", "between", "bigint",
        "binary", "blob", "both", "by", "call", "cascade", "case", "cast", "change", "char",
        "character", "check", "collate", "collation", "column", "concurrently", "condition",
        "constraint", "continue", "convert", "create", "cross", "cube", "cume_dist",
        "current_catalog", "current_date", "current_path", "current_role", "current_schema",
        "current_time", "current_timestamp", "current_user", "cursor", "database", "databases",
        "day_hour", "day_microsecond", "day_minute", "day_second", "deallocate", "dec", "decimal",
        "declare", "default", "deferrable", "delayed", "delete", "dense_rank", "desc", "describe",
        "deterministic", "distinct", "distinctrow", "div", "do", "double", "drop", "dual", "each",
        "else", "elseif", "empty", "enclosed", "end", "escape", "escaped", "except", "execute",
        "exists", "exit", "explain", "extract", "false", "fetch", "first_value", "float", "float4",
        "float8", "for", "force", "foreign", "freeze", "from", "full", "fulltext", "function",
        "generated", "get", "grant", "group", "grouping", "groups", "having", "high_priority",
        "hour_microsecond", "hour_minute", "hour_second", "if", "ignore", "ilike", "in", "index",
        "infile", "initially", "inner", "inout", "insensitive", "insert", "int", "int1", "int2",
        "int3", "int4", "int8", "integer", "intersect", "interval", "into", "io_after_gtids",
        "io_before_gtids", "is", "isnull", "iterate", "join", "json_array", "json_exists",
        "json_object", "json_query", "json_table", "json_value", "key", "keys", "kill", "lag",
        "last_value", "lateral", "lead", "leading", "leave", "left", "like", "limit", "linear",
        "lines", "listagg", "load", "localtime", "localtimestamp", "lock", "long", "longblob",
        "longtext", "loop", "low_priority", "master_bind", "master_ssl_verify_server_cert",
        "match", "maxvalue", "mediumblob", "mediumint", "mediumtext", "middleint",
        "minute_microsecond", "minute_second", "mod", "modifies", "natural", "no_write_to_binlog",
        "normalize", "not", "notnull", "nth_value", "ntile", "null", "numeric", "of", "offset",
        "on", "only", "optimize", "optimizer_costs", "option", "optionally", "or", "order", "out",
        "outer", "outfile", "over", "overlaps", "partition", "percent_rank", "placing",
        "precision", "prepare", "primary", "procedure", "purge", "qualify", "range", "rank",
        "read", "read_write", "reads", "real", "recursive", "references", "regexp", "release",
        "rename", "repeat", "replace", "require", "resignal", "restrict", "return", "returning",
        "revoke", "right", "rlike", "rollup", "row", "row_number", "rows", "schema", "schemas",
        "second_microsecond", "select", "sensitive", "separator", "session_user", "set", "show",
        "signal", "similar", "skip", "smallint", "some", "spatial", "specific", "sql",
        "sql_big_result", "sql_calc_found_rows", "sql_small_result", "sqlexception", "sqlstate",
        "sqlwarning", "ssl", "starting", "stored", "straight_join", "symmetric", "system",
        "system_user", "table", "tablesample", "terminated", "then", "tinyblob", "tinyint",
        "tinytext", "to", "trailing", "trigger", "trim", "true", "uescape", "undo", "union",
        "unique", "unlock", "unnest", "unsigned", "update", "usage", "use", "user", "using",
        "utc_date", "utc_time", "utc_timestamp", "values", "varbinary", "varchar", "varcharacter",
        "variadic", "varying", "verbose", "virtual", "when", "where", "while", "window", "with",
        "write", "xor", "year_month", "zerofill",
    ]
}
