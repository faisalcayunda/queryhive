//! Peeling a trailing statement terminator, and the `count` wrapper.
//!
//! The behaviour here is not invented: it is the behaviour of
//! `exporter/drivers.py:365` as pinned by
//! `check_wrappers_drop_a_comment_trailing_terminator` in
//! `tests/test_engine_events.py`, which is a check written specifically because
//! this area kept regressing. Those expectations are reproduced here as this
//! module's tests.

use thiserror::Error;

use crate::scan::{
    has_significant_text_dialect, scan_dialect, statement_count_dialect, Dialect, Lexer,
};

/// Why a statement could not be wrapped.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SqlError {
    /// There is no statement to work with.
    #[error("SQL is required")]
    Blank,

    /// More than one statement, and the caller asked about exactly one.
    ///
    /// The message keeps the shape the Python engine used (`2 statements`), so a
    /// user or a script matching on it keeps working.
    #[error("{count} statements; the grid counts one at a time")]
    MultipleStatements { count: usize },

    /// A statement the count wrapper cannot place inside a `FROM (…)` clause.
    #[error("{keyword} cannot be counted; use SELECT or WITH")]
    NotASelect { keyword: String },
}

/// Remove the terminator that ends a statement, and tidy what surrounded it.
///
/// Three cases, in the order they are decided:
///
/// 1. **No real separator.** Only a `;` at the very end can still be a
///    terminator — including the last character of a trailing comment, because
///    that `;` ends the statement on the wire even though a comment cannot hold
///    a separator. `SELECT 1 -- note;` becomes `SELECT 1 -- note`.
/// 2. **A separator with only trivia after it.** The head is kept and the trivia
///    is re-attached: `SELECT 1; -- trailing;` becomes `SELECT 1 -- trailing`,
///    and `SELECT 1;;` becomes `SELECT 1`.
/// 3. **A separator with another statement after it.** Nothing is peeled and the
///    string is returned as written, because the caller asked about all of it.
///    `SELECT 1; SELECT 2` stays `SELECT 1; SELECT 2`.
///
/// A `;` inside a literal is never touched: `SELECT 'a;b;'` is returned
/// unchanged, because those characters are data.
pub fn strip_terminator(sql: &str) -> String {
    strip_terminator_dialect(sql, Dialect::Generic)
}

/// [`strip_terminator`] under `dialect`'s lexical rules, so a MySQL `EXPLAIN` prefix is
/// added to a statement whose `;` inside a backslash-escaped string stays data.
pub fn strip_terminator_dialect(sql: &str, dialect: impl Into<Lexer>) -> String {
    let dialect: Lexer = dialect.into();
    let trimmed = sql.trim();
    let scan = scan_dialect(trimmed, dialect);

    let Some(&first_separator) = scan.separators.first() else {
        return if scan.ends_with_terminator {
            drop_trailing_terminator(trimmed)
        } else {
            trimmed.to_owned()
        };
    };

    let head = &trimmed[..first_separator];
    let rest = &trimmed[first_separator + 1..];

    // Another statement follows: leave the caller's text alone rather than
    // guessing which statement they meant.
    if has_significant_text_dialect(rest, dialect) {
        return trimmed.to_owned();
    }

    let rest = drop_trailing_terminator(rest);
    let head = head.trim_end();
    let rest = rest.trim();
    if rest.is_empty() {
        head.to_owned()
    } else {
        // One space, so a removed separator leaves no double gap behind.
        format!("{head} {rest}")
    }
}

/// Trim, and drop one trailing `;` if there is one.
fn drop_trailing_terminator(text: &str) -> String {
    let trimmed = text.trim();
    match trimmed.strip_suffix(';') {
        Some(without) => without.trim_end().to_owned(),
        None => trimmed.to_owned(),
    }
}

/// Wrap a `SELECT`/`WITH` statement so the server returns its true row count.
///
/// The count is the server's, not a count of rows the grid happened to receive:
/// `SELECT COUNT(*)` over the caller's statement is the only way to answer "how
/// many rows does this return" without reading them all, which is what makes the
/// row count cheap for a query the user is only looking at.
///
/// Refused rather than guessed in two cases: a statement that is not `SELECT` or
/// `WITH` (wrapping it would change what runs), and more than one statement (the
/// caller asked about one).
pub fn count_statement(sql: &str) -> Result<String, SqlError> {
    count_statement_dialect(sql, Dialect::Generic)
}

/// [`count_statement`] under `dialect`'s lexical rules, so a MySQL count reads a MySQL
/// escape and refuses a second statement MySQL would run.
pub fn count_statement_dialect(sql: &str, dialect: impl Into<Lexer>) -> Result<String, SqlError> {
    let dialect: Lexer = dialect.into();
    let trimmed = sql.trim();
    let scan = scan_dialect(trimmed, dialect);
    let count = statement_count_dialect(trimmed, &scan, dialect);

    if count == 0 {
        return Err(SqlError::Blank);
    }
    if count > 1 {
        return Err(SqlError::MultipleStatements { count });
    }
    match scan.leading_keyword.as_deref() {
        Some("SELECT" | "WITH") => {}
        Some(keyword) => {
            return Err(SqlError::NotASelect {
                keyword: keyword.to_owned(),
            })
        }
        None => return Err(SqlError::Blank),
    }

    Ok(format!(
        "SELECT COUNT(*) FROM ({}) AS queryhive_count",
        strip_terminator_dialect(trimmed, dialect)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decisions_readings, Decision, SafeMode, StatementKind};

    /// The expectations from `check_explain_strips_one_trailing_semicolon`,
    /// `check_explain_keeps_a_semicolon_inside_a_literal` and
    /// `check_wrappers_drop_a_comment_trailing_terminator`, unchanged.
    #[test]
    fn a_single_terminator_goes() {
        assert_eq!(strip_terminator("SELECT 1"), "SELECT 1");
        assert_eq!(strip_terminator("SELECT 1;"), "SELECT 1");
        assert_eq!(strip_terminator("SELECT 1;  "), "SELECT 1");
        assert_eq!(strip_terminator("  SELECT 1 ;  \n"), "SELECT 1");
        assert_eq!(
            strip_terminator("SELECT * FROM t LIMIT 5;"),
            "SELECT * FROM t LIMIT 5"
        );
        assert_eq!(strip_terminator("SELECT 1;;"), "SELECT 1");
    }

    #[test]
    fn a_semicolon_inside_a_literal_is_never_touched() {
        assert_eq!(strip_terminator("SELECT 'a;b;'"), "SELECT 'a;b;'");
        assert_eq!(strip_terminator("SELECT 'a;b;' ;"), "SELECT 'a;b;'");
        assert_eq!(strip_terminator("SELECT \"a;b\""), "SELECT \"a;b\"");
        assert_eq!(strip_terminator("SELECT 'it''s; ok'"), "SELECT 'it''s; ok'");
    }

    #[test]
    fn a_comments_trailing_semicolon_still_goes() {
        // A comment cannot hold a separator, but its `;` ends the statement on
        // the wire, so this is a syntax error if it survives.
        assert_eq!(strip_terminator("SELECT 1 -- note;"), "SELECT 1 -- note");
        assert_eq!(strip_terminator("SELECT 1 -- note"), "SELECT 1 -- note");
    }

    #[test]
    fn a_stranded_separator_before_a_trailing_comment_goes_too() {
        // The case that made the first version of this wrong.
        assert_eq!(
            strip_terminator("SELECT 1; -- trailing;"),
            "SELECT 1 -- trailing"
        );
        assert_eq!(strip_terminator("SELECT 1; -- note"), "SELECT 1 -- note");
    }

    #[test]
    fn a_block_comment_is_part_of_the_statement() {
        assert_eq!(strip_terminator("SELECT 1 /* x; */;"), "SELECT 1 /* x; */");
    }

    /// The text the Trino driver sends for `sql`: the terminator stripped and the leading blanks
    /// put back (the driver's `wire_sql`, which this crate must not depend on).
    fn sent_to_trino(sql: &str) -> String {
        let lead = sql.len() - sql.trim_start().len();
        format!(
            "{}{}",
            &sql[..lead],
            strip_terminator_dialect(sql, Dialect::Trino)
        )
    }

    const SAFE_MODES: [SafeMode; 4] = [
        SafeMode::Full,
        SafeMode::NoDdl,
        SafeMode::Confirm,
        SafeMode::ReadOnly,
    ];

    /// What the Safe Mode gate decides, statement by statement, as the engine host asks it
    /// (`decisions_readings` over the connection's readings). The statement text is left out:
    /// it differs by the terminator by design.
    fn gate(
        mode: SafeMode,
        sql: &str,
    ) -> Vec<(usize, StatementKind, Decision, Option<&'static str>)> {
        decisions_readings(mode, sql, Dialect::Trino.readings())
            .into_iter()
            .map(|d| (d.index, d.kind, d.decision, d.reason))
            .collect()
    }

    fn refusal_rank(decision: Decision) -> u8 {
        match decision {
            Decision::Allow => 0,
            Decision::Confirm => 1,
            Decision::Refuse => 2,
        }
    }

    /// Safe Mode decides on the text the caller wrote and the server runs the stripped text, so
    /// the two must get the same decision for every statement in every mode.
    #[test]
    fn stripping_the_terminator_never_changes_the_safe_mode_decisions() {
        for sql in [
            "SELECT 1;",
            "SELECT 1; -- note",
            "DELETE FROM t;",
            "DELETE FROM t; -- SELECT 1",
            "DROP TABLE t;;",
            "SELECT 1; DELETE FROM t",
            "SELECT 1; DELETE FROM t;",
            "SELECT 'a;b;'; ",
            "INSERT INTO t VALUES (';');",
            "SELECT 1; -- ; DROP TABLE x",
            "SELECT 1 --x\r; DELETE FROM t",
            "SELECT 1 /* a /* b */ ; DELETE FROM t",
            "  \n SELECT 1 ; ",
        ] {
            let sent = sent_to_trino(sql);
            for mode in SAFE_MODES {
                assert_eq!(gate(mode, sql), gate(mode, &sent), "{mode:?} {sql:?}");
            }
        }
    }

    /// A non-breaking space is not a blank to the statement splitter but is one to `trim`, so the
    /// original text reads one statement more than what is sent. Equality does not hold here;
    /// what must hold is that the gate, which reads the original, is never less strict than the
    /// statements the server runs: it may refuse more, never let through more.
    #[test]
    fn stripping_the_terminator_never_lets_through_more_than_the_gate_read() {
        for sql in ["SELECT 1;\u{a0}", "\u{a0}DELETE FROM t;\u{a0}"] {
            let sent = sent_to_trino(sql);
            for mode in SAFE_MODES {
                let strictest = |text: &str| {
                    gate(mode, text)
                        .into_iter()
                        .map(|(_, _, decision, _)| refusal_rank(decision))
                        .max()
                };
                assert!(
                    strictest(sql) >= strictest(&sent),
                    "{mode:?} {sql:?}: gate read {:?}, but {sent:?} reads {:?}",
                    gate(mode, sql),
                    gate(mode, &sent)
                );
            }
        }
    }

    #[test]
    fn a_second_statement_means_nothing_is_peeled() {
        // The caller asked about all of it; stripping would change the question.
        assert_eq!(strip_terminator("SELECT 1; SELECT 2"), "SELECT 1; SELECT 2");
    }

    #[test]
    fn the_count_wrapper_keeps_the_callers_text() {
        // Pinned by check_wrappers_drop_a_comment_trailing_terminator.
        assert_eq!(
            count_statement("SELECT 'a;b;'").unwrap(),
            "SELECT COUNT(*) FROM (SELECT 'a;b;') AS queryhive_count"
        );
        assert_eq!(
            count_statement("SELECT 1 -- note;").unwrap(),
            "SELECT COUNT(*) FROM (SELECT 1 -- note) AS queryhive_count"
        );
        assert_eq!(
            count_statement("SELECT * FROM people;").unwrap(),
            "SELECT COUNT(*) FROM (SELECT * FROM people) AS queryhive_count"
        );
    }

    #[test]
    fn a_cte_is_countable() {
        let wrapped = count_statement("WITH x AS (SELECT 1) SELECT * FROM x").unwrap();
        assert!(
            wrapped.starts_with("SELECT COUNT(*) FROM (WITH x AS (SELECT 1)"),
            "{wrapped}"
        );
        assert!(wrapped.ends_with(") AS queryhive_count"), "{wrapped}");
    }

    #[test]
    fn two_statements_are_refused_by_name() {
        let error = count_statement("SELECT 1;;").unwrap_err();
        assert_eq!(error, SqlError::MultipleStatements { count: 2 });
        // The message says how many, in the shape the Python engine used.
        assert!(error.to_string().contains("2 statements"), "{error}");
    }

    #[test]
    fn a_non_select_is_refused_rather_than_wrapped() {
        // Wrapping would change what runs, which is the one thing the row count
        // must never do.
        for (sql, keyword) in [
            ("DELETE FROM people", "DELETE"),
            ("UPDATE people SET a = 1", "UPDATE"),
            ("DROP TABLE people", "DROP"),
        ] {
            assert_eq!(
                count_statement(sql).unwrap_err(),
                SqlError::NotASelect {
                    keyword: keyword.to_owned()
                },
                "{sql}"
            );
        }
    }

    #[test]
    fn blank_input_is_refused() {
        assert_eq!(count_statement("   ").unwrap_err(), SqlError::Blank);
        assert_eq!(
            count_statement("-- nothing here").unwrap_err(),
            SqlError::Blank
        );
        assert_eq!(count_statement(";").unwrap_err(), SqlError::Blank);
    }
}
