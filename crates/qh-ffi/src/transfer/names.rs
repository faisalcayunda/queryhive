//! The one function every identifier of the transfer goes through (W13-T20 §4.5, B-3b).
//!
//! A table, schema, database, column or scratch name that reaches a statement comes from a
//! server or from a person, and a quote character inside it must not be able to close its
//! own quotes and carry SQL after them. So there is exactly one way to put a name into SQL
//! here: [`ident`], which writes the name bare when it can be and otherwise inside the
//! dialect's quote character with that character doubled. [`check`] runs before it and
//! refuses the names no server would keep as they are written.
//!
//! The rule is the app's own (`SQLIdentifierQuoting.swift`): a plain name is
//! `[a-z_][a-z0-9_]*` and not a reserved word, and everything else is quoted. The reserved
//! words are the *union* over MySQL, Trino and PostgreSQL, one list for every dialect, so a
//! column that moves between databases keeps working. `tests/fixtures/ident_quoting.json`
//! is generated from the Swift list, and both sides read it; `tests/transfer_types.rs`
//! also reads the Swift source itself, so the two lists cannot drift apart unnoticed.
//!
//! Doubling is what [`qh_sql::quote_ident`] does, and it is the implementation used here:
//! the difference is only that it always quotes.

use qh_sql::{quote_ident, IdentStyle};
use thiserror::Error;

use super::types::Target;

/// The reserved words, sorted so a lookup is a binary search.
///
/// Copied from `app/Sources/QueryHive/Models/SQLIdentifierQuoting.swift` (316 words at the
/// blob recorded in `ident_quoting.json`). Never edit it here alone: the drift test fails.
pub const RESERVED_WORDS: [&str; 316] = [
    "accessible",
    "add",
    "all",
    "alter",
    "analyse",
    "analyze",
    "and",
    "any",
    "array",
    "as",
    "asc",
    "asensitive",
    "asymmetric",
    "authorization",
    "auto",
    "before",
    "between",
    "bigint",
    "binary",
    "blob",
    "both",
    "by",
    "call",
    "cascade",
    "case",
    "cast",
    "change",
    "char",
    "character",
    "check",
    "collate",
    "collation",
    "column",
    "concurrently",
    "condition",
    "constraint",
    "continue",
    "convert",
    "create",
    "cross",
    "cube",
    "cume_dist",
    "current_catalog",
    "current_date",
    "current_path",
    "current_role",
    "current_schema",
    "current_time",
    "current_timestamp",
    "current_user",
    "cursor",
    "database",
    "databases",
    "day_hour",
    "day_microsecond",
    "day_minute",
    "day_second",
    "deallocate",
    "dec",
    "decimal",
    "declare",
    "default",
    "deferrable",
    "delayed",
    "delete",
    "dense_rank",
    "desc",
    "describe",
    "deterministic",
    "distinct",
    "distinctrow",
    "div",
    "do",
    "double",
    "drop",
    "dual",
    "each",
    "else",
    "elseif",
    "empty",
    "enclosed",
    "end",
    "escape",
    "escaped",
    "except",
    "execute",
    "exists",
    "exit",
    "explain",
    "extract",
    "false",
    "fetch",
    "first_value",
    "float",
    "float4",
    "float8",
    "for",
    "force",
    "foreign",
    "freeze",
    "from",
    "full",
    "fulltext",
    "function",
    "generated",
    "get",
    "grant",
    "group",
    "grouping",
    "groups",
    "having",
    "high_priority",
    "hour_microsecond",
    "hour_minute",
    "hour_second",
    "if",
    "ignore",
    "ilike",
    "in",
    "index",
    "infile",
    "initially",
    "inner",
    "inout",
    "insensitive",
    "insert",
    "int",
    "int1",
    "int2",
    "int3",
    "int4",
    "int8",
    "integer",
    "intersect",
    "interval",
    "into",
    "io_after_gtids",
    "io_before_gtids",
    "is",
    "isnull",
    "iterate",
    "join",
    "json_array",
    "json_exists",
    "json_object",
    "json_query",
    "json_table",
    "json_value",
    "key",
    "keys",
    "kill",
    "lag",
    "last_value",
    "lateral",
    "lead",
    "leading",
    "leave",
    "left",
    "like",
    "limit",
    "linear",
    "lines",
    "listagg",
    "load",
    "localtime",
    "localtimestamp",
    "lock",
    "long",
    "longblob",
    "longtext",
    "loop",
    "low_priority",
    "master_bind",
    "master_ssl_verify_server_cert",
    "match",
    "maxvalue",
    "mediumblob",
    "mediumint",
    "mediumtext",
    "middleint",
    "minute_microsecond",
    "minute_second",
    "mod",
    "modifies",
    "natural",
    "no_write_to_binlog",
    "normalize",
    "not",
    "notnull",
    "nth_value",
    "ntile",
    "null",
    "numeric",
    "of",
    "offset",
    "on",
    "only",
    "optimize",
    "optimizer_costs",
    "option",
    "optionally",
    "or",
    "order",
    "out",
    "outer",
    "outfile",
    "over",
    "overlaps",
    "partition",
    "percent_rank",
    "placing",
    "precision",
    "prepare",
    "primary",
    "procedure",
    "purge",
    "qualify",
    "range",
    "rank",
    "read",
    "read_write",
    "reads",
    "real",
    "recursive",
    "references",
    "regexp",
    "release",
    "rename",
    "repeat",
    "replace",
    "require",
    "resignal",
    "restrict",
    "return",
    "returning",
    "revoke",
    "right",
    "rlike",
    "rollup",
    "row",
    "row_number",
    "rows",
    "schema",
    "schemas",
    "second_microsecond",
    "select",
    "sensitive",
    "separator",
    "session_user",
    "set",
    "show",
    "signal",
    "similar",
    "skip",
    "smallint",
    "some",
    "spatial",
    "specific",
    "sql",
    "sql_big_result",
    "sql_calc_found_rows",
    "sql_small_result",
    "sqlexception",
    "sqlstate",
    "sqlwarning",
    "ssl",
    "starting",
    "stored",
    "straight_join",
    "symmetric",
    "system",
    "system_user",
    "table",
    "tablesample",
    "terminated",
    "then",
    "tinyblob",
    "tinyint",
    "tinytext",
    "to",
    "trailing",
    "trigger",
    "trim",
    "true",
    "uescape",
    "undo",
    "union",
    "unique",
    "unlock",
    "unnest",
    "unsigned",
    "update",
    "usage",
    "use",
    "user",
    "using",
    "utc_date",
    "utc_time",
    "utc_timestamp",
    "values",
    "varbinary",
    "varchar",
    "varcharacter",
    "variadic",
    "varying",
    "verbose",
    "virtual",
    "when",
    "where",
    "while",
    "window",
    "with",
    "write",
    "xor",
    "year_month",
    "zerofill",
];

/// Whether `word` is on the union list. The comparison is exact: a plain name is lower case
/// by definition, so a differently cased spelling is quoted for being upper case anyway.
pub fn is_reserved(word: &str) -> bool {
    RESERVED_WORDS.binary_search(&word).is_ok()
}

/// Lower-case ASCII letters, digits and underscores, starting with a letter or an underscore.
fn is_plain(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first == b'_' || first.is_ascii_lowercase())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Whether `name` has to be quoted to mean what it says.
pub fn needs_quotes(name: &str) -> bool {
    !is_plain(name) || is_reserved(name)
}

fn style(target: Target) -> IdentStyle {
    match target {
        Target::Postgres => IdentStyle::Ansi,
        Target::Mysql => IdentStyle::Mysql,
    }
}

/// The name inside its dialect's quote character, whether it needs it or not, with the
/// quote character doubled where it occurs in the name.
pub fn quote(target: Target, name: &str) -> String {
    quote_ident(style(target), name)
}

/// The name as it is written in a statement: bare when it can be, quoted otherwise.
///
/// Does not judge the name; run [`check`] first, because a name this function quotes
/// correctly can still be one the server would shorten, strip or refuse.
pub fn ident(target: Target, name: &str) -> String {
    if needs_quotes(name) {
        quote(target, name)
    } else {
        name.to_owned()
    }
}

/// Why a name cannot be used as it is. The messages state the rule and never the name, so a
/// caller that wants to show the name chooses how to escape it (it may hold a newline).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum NameError {
    #[error("is empty")]
    Empty,
    #[error("contains a NUL character")]
    Nul,
    /// PostgreSQL would cut the name silently; MySQL refuses it.
    #[error("is longer than {max} {unit}")]
    TooLong { max: usize, unit: &'static str },
    /// MySQL does not accept a database, table or column name that ends in a space.
    #[error("ends with a space")]
    TrailingSpace,
    /// What PostgreSQL calls a column that has no name of its own.
    #[error("has no name of its own (PostgreSQL names such a column ?column?)")]
    Unnamed,
    /// MySQL stores identifiers as UTF-8 limited to the Basic Multilingual Plane.
    #[error("has a character beyond the Basic Multilingual Plane, which MySQL does not accept in a name")]
    OutsideBmp,
    #[error("appears more than once")]
    Duplicate,
}

/// Which name of a list was refused, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameFault {
    pub index: usize,
    pub error: NameError,
}

/// Refuse the names a server would not keep exactly as written.
///
/// PostgreSQL's limit is 63 **bytes** (it truncates, which would turn two columns into one
/// or a table name into another); MySQL's is 64 **characters**.
pub fn check(target: Target, name: &str) -> Result<(), NameError> {
    if name.is_empty() {
        return Err(NameError::Empty);
    }
    if name.contains('\0') {
        return Err(NameError::Nul);
    }
    match target {
        Target::Postgres => {
            if name.len() > 63 {
                return Err(NameError::TooLong {
                    max: 63,
                    unit: "bytes",
                });
            }
            if name == "?column?" {
                return Err(NameError::Unnamed);
            }
        }
        Target::Mysql => {
            if name.chars().count() > 64 {
                return Err(NameError::TooLong {
                    max: 64,
                    unit: "characters",
                });
            }
            if name.ends_with(' ') {
                return Err(NameError::TrailingSpace);
            }
            if name.chars().any(|character| u32::from(character) > 0xFFFF) {
                return Err(NameError::OutsideBmp);
            }
        }
    }
    Ok(())
}

/// [`check`] then [`ident`].
pub fn checked(target: Target, name: &str) -> Result<String, NameError> {
    check(target, name)?;
    Ok(ident(target, name))
}

/// The parts of a name, each checked and written with [`ident`], joined with dots.
pub fn qualified(target: Target, parts: &[&str]) -> Result<String, NameFault> {
    let mut written: Vec<String> = Vec::with_capacity(parts.len());
    for (index, part) in parts.iter().enumerate() {
        written.push(checked(target, part).map_err(|error| NameFault { index, error })?);
    }
    Ok(written.join("."))
}

/// The columns of one table: each name checked, and no two the same.
///
/// A duplicate is refused rather than renamed (OQ-8). PostgreSQL compares names exactly;
/// MySQL ignores case, so `Id` and `id` collide there. (MySQL's comparison also ignores
/// accents, which needs the server's collation tables; a collision this check cannot see
/// still stops at the `CREATE TABLE`, before the first row.)
pub fn check_columns(target: Target, names: &[&str]) -> Result<(), NameFault> {
    let mut seen = std::collections::HashSet::new();
    for (index, name) in names.iter().enumerate() {
        check(target, name).map_err(|error| NameFault { index, error })?;
        let key = match target {
            Target::Postgres => (*name).to_owned(),
            Target::Mysql => name.to_lowercase(),
        };
        if !seen.insert(key) {
            return Err(NameFault {
                index,
                error: NameError::Duplicate,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_union_list_is_sorted_unique_and_the_size_the_app_has() {
        assert_eq!(RESERVED_WORDS.len(), 316);
        assert!(RESERVED_WORDS.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(RESERVED_WORDS.iter().all(|word| is_plain(word)));
    }

    #[test]
    fn a_plain_name_is_bare_and_anything_else_is_quoted() {
        for name in ["id", "_x", "a1", "order_total", "name", "status", "date"] {
            assert_eq!(ident(Target::Postgres, name), name, "{name}");
            assert_eq!(ident(Target::Mysql, name), name, "{name}");
        }
        // Non-reserved words that every server accepts as a column stay bare.
        // Reserved by any dialect: quoted for every dialect.
        for word in ["select", "user", "order", "key", "rank", "array"] {
            assert_eq!(ident(Target::Postgres, word), format!("\"{word}\""));
            assert_eq!(ident(Target::Mysql, word), format!("`{word}`"));
        }
        for name in ["Name", "1st", "a b", "a-b", "é", "", "a.b", "a;b"] {
            assert!(needs_quotes(name), "{name:?}");
        }
    }

    #[test]
    fn the_dialects_own_quote_is_doubled_and_the_other_is_ordinary() {
        assert_eq!(ident(Target::Postgres, "an\"alytics"), "\"an\"\"alytics\"");
        assert_eq!(ident(Target::Postgres, "a`b"), "\"a`b\"");
        assert_eq!(ident(Target::Mysql, "my`db"), "`my``db`");
        assert_eq!(ident(Target::Mysql, "a\"b"), "`a\"b`");
        // A name that tries to close its quote and run a statement stays one name.
        assert_eq!(
            ident(Target::Postgres, "x\"; DROP TABLE y; --"),
            "\"x\"\"; DROP TABLE y; --\""
        );
        assert_eq!(
            ident(Target::Mysql, "x`; DROP TABLE y; --"),
            "`x``; DROP TABLE y; --`"
        );
        assert_eq!(ident(Target::Postgres, "a\nb"), "\"a\nb\"");
    }

    #[test]
    fn the_names_no_server_keeps_are_refused() {
        let long_pg = "a".repeat(64);
        let ok_pg = "a".repeat(63);
        assert!(check(Target::Postgres, &ok_pg).is_ok());
        assert_eq!(
            check(Target::Postgres, &long_pg),
            Err(NameError::TooLong {
                max: 63,
                unit: "bytes"
            })
        );
        // 32 two-byte characters are 64 bytes: too long for PostgreSQL, fine for MySQL.
        let wide = "é".repeat(32);
        assert!(check(Target::Postgres, &wide).is_err());
        assert!(check(Target::Mysql, &wide).is_ok());
        assert!(check(Target::Mysql, &"a".repeat(64)).is_ok());
        assert!(check(Target::Mysql, &"a".repeat(65)).is_err());

        for target in [Target::Postgres, Target::Mysql] {
            assert_eq!(check(target, ""), Err(NameError::Empty));
            assert_eq!(check(target, "a\0b"), Err(NameError::Nul));
        }
        assert_eq!(check(Target::Postgres, "?column?"), Err(NameError::Unnamed));
        assert!(check(Target::Mysql, "?column?").is_ok());
        assert_eq!(check(Target::Mysql, "a "), Err(NameError::TrailingSpace));
        assert!(check(Target::Postgres, "a ").is_ok());
        assert_eq!(
            check(Target::Mysql, "a\u{1F600}"),
            Err(NameError::OutsideBmp)
        );
        assert!(check(Target::Postgres, "a\u{1F600}").is_ok());
        // Trino's generated names and MySQL's expression text are fine and quoted.
        assert_eq!(checked(Target::Postgres, "_col0").as_deref(), Ok("_col0"));
        assert_eq!(
            checked(Target::Mysql, "count(*)").as_deref(),
            Ok("`count(*)`")
        );
    }

    #[test]
    fn duplicate_columns_are_refused_and_mysql_ignores_case() {
        assert!(check_columns(Target::Postgres, &["id", "Id"]).is_ok());
        assert_eq!(
            check_columns(Target::Postgres, &["id", "x", "id"]),
            Err(NameFault {
                index: 2,
                error: NameError::Duplicate
            })
        );
        assert_eq!(
            check_columns(Target::Mysql, &["id", "ID"]),
            Err(NameFault {
                index: 1,
                error: NameError::Duplicate
            })
        );
        // A bad name is reported at its own place, before a later duplicate.
        assert_eq!(
            check_columns(Target::Postgres, &["a", "", "a"]),
            Err(NameFault {
                index: 1,
                error: NameError::Empty
            })
        );
    }

    #[test]
    fn a_qualified_name_names_the_part_that_failed() {
        assert_eq!(
            qualified(Target::Postgres, &["public", "People"]).as_deref(),
            Ok("public.\"People\"")
        );
        assert_eq!(
            qualified(Target::Mysql, &["shop", "order"]).as_deref(),
            Ok("shop.`order`")
        );
        assert_eq!(
            qualified(Target::Postgres, &["public", "a\0"]),
            Err(NameFault {
                index: 1,
                error: NameError::Nul
            })
        );
    }
}
