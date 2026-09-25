//! Identifier quoting, one implementation instead of one per driver.
//!
//! The Python engine kept this in one place too (`exporter/drivers.py:240`),
//! which is why the three drivers cannot drift apart on it.

/// Which quoting a dialect wants around an identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentStyle {
    /// `"name"`, with `"` doubled inside. Postgres, Trino, and the SQL standard.
    Ansi,
    /// `` `name` ``, with `` ` `` doubled inside. MySQL and MariaDB.
    Mysql,
}

/// Quote one identifier, escaping the quote character by doubling it.
///
/// Doubling is the rule every dialect here uses: `an"alytics` becomes
/// `"an""alytics"` and `` my`db `` becomes `` `my``db` ``. Getting this wrong
/// produces SQL that does not parse for a table the user can see in the tree,
/// which is a confusing way to fail.
pub fn quote_ident(style: IdentStyle, ident: &str) -> String {
    match style {
        IdentStyle::Ansi => format!("\"{}\"", ident.replace('"', "\"\"")),
        IdentStyle::Mysql => format!("`{}`", ident.replace('`', "``")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_quoting_doubles_the_quote_character() {
        // Pinned by check_to_table_quotes_identifiers in tests/test_engine_events.py.
        assert_eq!(quote_ident(IdentStyle::Ansi, "analytics"), "\"analytics\"");
        assert_eq!(
            quote_ident(IdentStyle::Ansi, "an\"alytics"),
            "\"an\"\"alytics\""
        );
    }

    #[test]
    fn mysql_quoting_doubles_the_backtick() {
        assert_eq!(quote_ident(IdentStyle::Mysql, "mydb"), "`mydb`");
        assert_eq!(quote_ident(IdentStyle::Mysql, "my`db"), "`my``db`");
    }

    #[test]
    fn a_quote_of_the_other_dialect_is_left_alone() {
        // A backtick is an ordinary character in ANSI quoting, and vice versa.
        assert_eq!(quote_ident(IdentStyle::Ansi, "a`b"), "\"a`b\"");
        assert_eq!(quote_ident(IdentStyle::Mysql, "a\"b"), "`a\"b`");
    }

    #[test]
    fn an_empty_identifier_still_quotes() {
        // Not rejected here: the caller decides whether an empty part is a usage
        // error, and it needs a quoted rendering for the message either way.
        assert_eq!(quote_ident(IdentStyle::Ansi, ""), "\"\"");
        assert_eq!(quote_ident(IdentStyle::Mysql, ""), "``");
    }
}
