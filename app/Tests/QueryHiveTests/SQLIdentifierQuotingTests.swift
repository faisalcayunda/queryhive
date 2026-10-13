import XCTest

@testable import QueryHive

/// When an identifier is written bare and when it is quoted, per dialect.
final class SQLIdentifierQuotingTests: XCTestCase {
    private let kinds = ConnectionKind.allCases

    /// The reserved words of each project's own documentation (MySQL 8.0 and 8.4's keyword tables,
    /// Trino's reserved keywords, PostgreSQL's key-words appendix): every one must be quoted for every
    /// dialect, because the rule is one list and not one per dialect.
    private let mysqlReserved: [String] = [
        "accessible", "add", "all", "alter", "analyze", "and", "as", "asc", "asensitive", "before",
        "between", "bigint", "binary", "blob", "both", "by", "call", "cascade", "case", "change",
        "char", "character", "check", "collate", "column", "condition", "constraint", "continue",
        "convert", "create", "cross", "cube", "cume_dist", "current_date", "current_time",
        "current_timestamp", "current_user", "cursor", "database", "databases", "day_hour",
        "day_microsecond", "day_minute", "day_second", "dec", "decimal", "declare", "default",
        "delayed", "delete", "dense_rank", "desc", "describe", "deterministic", "distinct",
        "distinctrow", "div", "double", "drop", "dual", "each", "else", "elseif", "empty",
        "enclosed", "escaped", "except", "exists", "exit", "explain", "false", "fetch",
        "first_value", "float", "float4", "float8", "for", "force", "foreign", "from", "fulltext",
        "function", "generated", "get", "grant", "group", "grouping", "groups", "having",
        "high_priority", "hour_microsecond", "hour_minute", "hour_second", "if", "ignore", "in",
        "index", "infile", "inner", "inout", "insensitive", "insert", "int", "int1", "int2",
        "int3", "int4", "int8", "integer", "intersect", "interval", "into", "io_after_gtids",
        "io_before_gtids", "is", "iterate", "join", "json_table", "key", "keys", "kill", "lag",
        "last_value", "lateral", "lead", "leading", "leave", "left", "like", "limit", "linear",
        "lines", "load", "localtime", "localtimestamp", "lock", "long", "longblob", "longtext",
        "loop", "low_priority", "master_bind", "master_ssl_verify_server_cert", "match",
        "maxvalue", "mediumblob", "mediumint", "mediumtext", "middleint", "minute_microsecond",
        "minute_second", "mod", "modifies", "natural", "no_write_to_binlog", "not", "nth_value",
        "ntile", "null", "numeric", "of", "on", "optimize", "optimizer_costs", "option",
        "optionally", "or", "order", "out", "outer", "outfile", "over", "partition",
        "percent_rank", "precision", "primary", "procedure", "purge", "qualify", "range", "rank",
        "read", "read_write", "reads", "real", "recursive", "references", "regexp", "release",
        "rename", "repeat", "replace", "require", "resignal", "restrict", "return", "revoke",
        "right", "rlike", "row", "row_number", "rows", "schema", "schemas", "second_microsecond",
        "select", "sensitive", "separator", "set", "show", "signal", "smallint", "spatial",
        "specific", "sql", "sql_big_result", "sql_calc_found_rows", "sql_small_result",
        "sqlexception", "sqlstate", "sqlwarning", "ssl", "starting", "stored", "straight_join",
        "system", "table", "tablesample", "terminated", "then", "tinyblob", "tinyint", "tinytext",
        "to", "trailing", "trigger", "true", "undo", "union", "unique", "unlock", "unsigned",
        "update", "usage", "use", "using", "utc_date", "utc_time", "utc_timestamp", "values",
        "varbinary", "varchar", "varcharacter", "varying", "virtual", "when", "where", "while",
        "window", "with", "write", "xor", "year_month", "zerofill",
    ]

    private let trinoReserved: [String] = [
        "alter", "and", "as", "auto", "between", "by", "case", "cast", "constraint", "create",
        "cross", "cube", "current_catalog", "current_date", "current_path", "current_role",
        "current_schema", "current_time", "current_timestamp", "current_user", "deallocate",
        "delete", "describe", "distinct", "drop", "else", "end", "escape", "except", "exists",
        "extract", "false", "for", "from", "full", "group", "grouping", "having", "in", "inner",
        "insert", "intersect", "into", "is", "join", "json_array", "json_exists", "json_object",
        "json_query", "json_table", "json_value", "left", "like", "listagg", "localtime",
        "localtimestamp", "natural", "normalize", "not", "null", "on", "or", "order", "outer",
        "overlaps", "prepare", "recursive", "right", "rollup", "select", "skip", "table", "then",
        "trim", "true", "uescape", "union", "unnest", "using", "values", "when", "where", "with",
    ]

    private let postgresReserved: [String] = [
        "all", "analyse", "analyze", "and", "any", "array", "as", "asc", "asymmetric",
        "authorization", "binary", "both", "case", "cast", "check", "collate", "collation",
        "column", "concurrently", "constraint", "create", "cross", "current_catalog",
        "current_date", "current_role", "current_schema", "current_time", "current_timestamp",
        "current_user", "default", "deferrable", "desc", "distinct", "do", "else", "end", "except",
        "false", "fetch", "for", "foreign", "freeze", "from", "full", "grant", "group", "having",
        "ilike", "in", "initially", "inner", "intersect", "into", "is", "isnull", "join",
        "lateral", "leading", "left", "like", "limit", "localtime", "localtimestamp", "natural",
        "not", "notnull", "null", "offset", "on", "only", "or", "order", "outer", "overlaps",
        "placing", "primary", "references", "returning", "right", "select", "session_user",
        "similar", "some", "symmetric", "system_user", "table", "tablesample", "then", "to",
        "trailing", "true", "union", "unique", "user", "using", "variadic", "verbose", "when",
        "where", "window", "with",
    ]

    func testAPlainLowerCaseNameIsWrittenBareInEveryDialect() {
        for kind in kinds {
            for name in ["id", "nama", "tanggal_lahir", "_hidden", "col1", "a1_b2", "created_at", "status", "type",
                         "date", "name", "value", "comment", "count", "level", "data", "text", "label", "amount"] {
                XCTAssertFalse(SQLIdentifierQuoting.needsQuotes(name, for: kind), "\(name) in \(kind)")
                XCTAssertEqual(SQLIdentifierQuoting.quoted(name, for: kind), name, "\(name) in \(kind)")
            }
        }
    }

    func testUpperCaseIsQuotedBecauseTheServerFoldsABareNameToLowerCase() {
        XCTAssertEqual(SQLIdentifierQuoting.quoted("Name", for: .postgres), "\"Name\"")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("ID", for: .trino), "\"ID\"")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("Name", for: .mysql), "`Name`")
    }

    func testALeadingDigitASpaceAHyphenAndOtherSymbolsAreQuoted() {
        for name in ["1st", "9", "first name", "first-name", "a.b", "a$b", "a/b", "\u{E9}", "nama_\u{E9}", "\u{65E5}\u{672C}", " x", "x "] {
            for kind in kinds {
                XCTAssertTrue(SQLIdentifierQuoting.needsQuotes(name, for: kind), "\(name) in \(kind)")
            }
        }
        XCTAssertEqual(SQLIdentifierQuoting.quoted("first name", for: .postgres), "\"first name\"")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("1st", for: .mysql), "`1st`")
    }

    func testTheEmptyNameIsQuoted() {
        XCTAssertEqual(SQLIdentifierQuoting.quoted("", for: .postgres), "\"\"")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("", for: .mysql), "``")
    }

    func testAQuoteCharacterInsideTheNameIsDoubledForTheDialect() {
        XCTAssertEqual(SQLIdentifierQuoting.quoted("a\"b", for: .postgres), "\"a\"\"b\"")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("a\"b", for: .trino), "\"a\"\"b\"")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("a`b", for: .mysql), "`a``b`")
        // The other dialect's quote is just a symbol: quoted, and not doubled.
        XCTAssertEqual(SQLIdentifierQuoting.quoted("a`b", for: .postgres), "\"a`b\"")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("a\"b", for: .mysql), "`a\"b`")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("\"", for: .postgres), "\"\"\"\"")
    }

    func testReservedWordsAreQuotedWhateverTheirCase() {
        for kind in kinds {
            for word in ["select", "order", "group", "from", "where", "table", "user", "limit", "offset", "union", "null"] {
                XCTAssertTrue(SQLIdentifierQuoting.needsQuotes(word, for: kind), "\(word) in \(kind)")
                // An upper-case spelling is quoted anyway, for being upper case.
                XCTAssertTrue(SQLIdentifierQuoting.needsQuotes(word.uppercased(), for: kind))
            }
        }
        XCTAssertEqual(SQLIdentifierQuoting.quoted("order", for: .trino), "\"order\"")
        XCTAssertEqual(SQLIdentifierQuoting.quoted("group", for: .mysql), "`group`")
    }

    /// The reviewer's words, and every other word one dialect reserves: all of them are quoted for
    /// all three, so a word reserved by another database does not protect this one by accident.
    func testEveryWordAnyDialectReservesIsQuotedForAllThree() {
        for (dialect, list) in [("MySQL", mysqlReserved), ("Trino", trinoReserved), ("PostgreSQL", postgresReserved)] {
            XCTAssertGreaterThan(list.count, 60, "\(dialect)'s list was not pinned")
            for word in list {
                for kind in kinds {
                    XCTAssertTrue(SQLIdentifierQuoting.needsQuotes(word, for: kind), "\(dialect) reserves \(word); bare in \(kind)")
                }
            }
        }
    }

    func testTheWordsOnlyOneDialectReservesEachProtectTheOthers() {
        // MySQL 8.0/8.4 only.
        for word in ["analyze", "binary", "cube", "grouping", "lateral", "cursor", "dual", "distinctrow", "escaped",
                     "linear", "maxvalue", "asensitive", "insensitive", "read_write", "optimizer_costs",
                     "io_after_gtids", "io_before_gtids", "master_bind", "master_ssl_verify_server_cert",
                     "tablesample", "qualify", "rank", "key", "index", "row", "system"] {
            for kind in kinds { XCTAssertTrue(SQLIdentifierQuoting.needsQuotes(word, for: kind), "\(word) in \(kind)") }
        }
        // Trino only.
        for word in ["trim", "skip", "listagg", "json_array", "json_exists", "json_object", "json_query", "json_table",
                     "json_value", "current_catalog", "current_schema", "current_role", "current_path", "auto", "unnest"] {
            for kind in kinds { XCTAssertTrue(SQLIdentifierQuoting.needsQuotes(word, for: kind), "\(word) in \(kind)") }
        }
        // PostgreSQL only; `system_user` parses as a built-in, so a column of that name silently returns
        // the session's value.
        for word in ["system_user", "session_user", "ilike", "verbose", "freeze", "variadic", "collation", "concurrently"] {
            for kind in kinds { XCTAssertTrue(SQLIdentifierQuoting.needsQuotes(word, for: kind), "\(word) in \(kind)") }
        }
    }

    func testANameThatMerelyContainsAKeywordIsBare() {
        for kind in kinds {
            for name in ["order_id", "group_name", "selected", "from_date", "user_id", "tablename", "keyword", "ranking"] {
                XCTAssertFalse(SQLIdentifierQuoting.needsQuotes(name, for: kind), name)
            }
        }
    }
}
