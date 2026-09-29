//! Deciding what a statement does, and what a connection's Safe Mode does about it.
//!
//! This is the engine's answer to "may this SQL run here", and it lives in `qh-sql`
//! rather than in the app because the engine has three callers and only one of them
//! has a window: the CLI, the MCP server, and the app. A rule kept in a Swift picker
//! would be a rule the other two never see.
//!
//! # The three levels
//!
//! | `SAFE_MODE` | refuses |
//! |---|---|
//! | `full` | nothing |
//! | `no_ddl` | DDL, and anything the classifier cannot read |
//! | `read_only` | every write, every DDL, and anything the classifier cannot read |
//!
//! # The rule is conservative, and that is the point
//!
//! **When the classifier cannot tell what a statement is, it treats it as a write and
//! refuses it** under any mode but `full`. The alternative — allowing what it cannot
//! read — is the one failure that cannot be undone: a statement one does not understand
//! is exactly the statement whose effects one cannot predict. So the classifier's job is
//! to *recognise* a read, not to fail to recognise a write.
//!
//! What it recognises is syntax, not semantics. It looks at the leading keyword and then
//! at every bare word in the statement, all of them after [`crate::scan`] has removed
//! literals, quoted identifiers, comments and dollar-quoted bodies — so a `DROP` inside a
//! string is text and a table named `drop2` is not `DROP`. It cannot see a function whose
//! body writes, a `SELECT` that calls one, or a `WITH` clause that reads and a later
//! statement that does not; those are limits of reading text instead of running a
//! server's analyser, and they are stated here rather than hidden. A mode is a guardrail
//! against a wrong statement, not a substitute for the database's own privileges.
//!
//! A statement is **ReadOnly** when its leading keyword is one of the read starters and
//! no write keyword appears anywhere in it. It is **Dml** or **Ddl** when a write keyword
//! does. Anything else — a session control word, a name nobody here knows, a fragment
//! that does not start with a word at all — is **Unknown**, and Unknown is refused by
//! every mode but `full`.
//!
//! One conservative choice worth naming: `SELECT … FOR UPDATE` takes row locks, so the
//! `UPDATE` word inside it makes it a write. It is one syntactically, and a read-only
//! connection that allowed it would be handing out locks.

use thiserror::Error;

use crate::scan::{has_significant_text, scan};

/// What one statement does, as far as reading its text can say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementKind {
    /// Reads, and changes nothing a later statement can see.
    ReadOnly,
    /// Changes rows: `INSERT`, `UPDATE`, `DELETE`, a data-modifying CTE.
    Dml,
    /// Changes structure, or is maintenance a read-only connection has no business
    /// running: `CREATE`, `ALTER`, `DROP`, `TRUNCATE`, `VACUUM`, `SELECT … INTO`.
    Ddl,
    /// The classifier will not claim to know. Treated as a write by every mode but
    /// `full`.
    Unknown,
}

impl StatementKind {
    /// The word the refusal message uses for this kind.
    pub const fn label(self) -> &'static str {
        match self {
            StatementKind::ReadOnly => "read-only",
            StatementKind::Dml => "a write",
            StatementKind::Ddl => "DDL",
            StatementKind::Unknown => "unclassified",
        }
    }
}

impl std::fmt::Display for StatementKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}

/// What a connection refuses, named the way the `SAFE_MODE` setting names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafeMode {
    /// Refuses nothing. The engine's default, so the CLI and the golden corpus are
    /// unchanged by this feature's arrival.
    Full,
    /// Allows DML, refuses DDL and anything unclassified.
    NoDdl,
    /// Refuses every write, every DDL, and anything unclassified.
    ReadOnly,
}

/// Every spelling the setting accepts, in the order a message lists them.
pub const SAFE_MODES: [&str; 3] = ["full", "no_ddl", "read_only"];

impl SafeMode {
    /// Parse a setting value. `None` for a spelling nobody recognises, rather than a
    /// default: a typo in a safety setting is not a decision to make silently.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "full" => Some(SafeMode::Full),
            "no_ddl" => Some(SafeMode::NoDdl),
            "read_only" => Some(SafeMode::ReadOnly),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            SafeMode::Full => "full",
            SafeMode::NoDdl => "no_ddl",
            SafeMode::ReadOnly => "read_only",
        }
    }

    /// Why this mode refuses a statement of `kind`, or `None` when it allows it.
    ///
    /// The wording is the sentence a user reads, so it says what the mode is and what
    /// the statement was, not which internal enum value matched.
    pub fn refusal(self, kind: StatementKind) -> Option<&'static str> {
        match self {
            SafeMode::Full => None,
            SafeMode::NoDdl => match kind {
                StatementKind::ReadOnly | StatementKind::Dml => None,
                StatementKind::Ddl => {
                    Some("DDL is not allowed on a connection whose Safe Mode is no_ddl")
                }
                StatementKind::Unknown => Some(
                    "the classifier could not tell whether this statement is read-only, and a \
                     no_ddl connection treats an unclassified statement as a write",
                ),
            },
            SafeMode::ReadOnly => match kind {
                StatementKind::ReadOnly => None,
                StatementKind::Dml => Some("writing is not allowed on a read-only connection"),
                StatementKind::Ddl => Some("DDL is not allowed on a read-only connection"),
                StatementKind::Unknown => Some(
                    "the classifier could not tell whether this statement is read-only, and a \
                     read-only connection refuses an unclassified statement",
                ),
            },
        }
    }
}

impl std::fmt::Display for SafeMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A script a Safe Mode refused, and which statement in it was the reason.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SafeModeError {
    /// One statement in the script, named by its position, was refused.
    ///
    /// The whole script is refused rather than the offending statement being skipped:
    /// a script is one instruction, and running the part before the refused statement
    /// would be running something the user did not get to approve.
    #[error("SAFE_MODE={mode} refuses statement {index} ({kind}): {reason}: {statement}")]
    Refused {
        mode: SafeMode,
        /// 1-based position among the statements the classifier read.
        index: usize,
        kind: StatementKind,
        reason: &'static str,
        /// A flattened, shortened rendering of the refused statement.
        statement: String,
    },
}

/// What one statement does, by reading its text.
pub fn classify(sql: &str) -> StatementKind {
    let scan = scan(sql);
    let Some(leading) = scan.leading_keyword.as_deref() else {
        // No word at all: an empty fragment, a bare operator, a comment. Nothing to
        // recognise, so nothing is claimed.
        return StatementKind::Unknown;
    };
    match leading {
        // These are read-only whatever else they contain. `SHOW CREATE TABLE` holds
        // the word `CREATE` and creates nothing, so scanning their body would refuse a
        // read — the one false positive worth carving out.
        "SHOW" | "DESC" | "DESCRIBE" => StatementKind::ReadOnly,
        // A read starter, but one that can hide a write further in: `WITH x AS (DELETE
        // …) SELECT`, `SELECT … FOR UPDATE`, `SELECT … INTO new_table`, `EXPLAIN
        // ANALYZE …`. Every bare word is checked.
        "SELECT" | "VALUES" | "TABLE" | "WITH" | "EXPLAIN" => scan
            .keywords
            .iter()
            .find_map(|word| write_kind(word))
            .unwrap_or(StatementKind::ReadOnly),
        // Anything else is judged by its own leading word: a statement that *starts*
        // with `INSERT` is a write even if it contains no other write word.
        other => write_kind(other).unwrap_or(StatementKind::Unknown),
    }
}

/// Whether one bare word names a write, and which kind.
///
/// Exact matches only. `UPDATED_AT` is not `UPDATE`, and `DROPPED` is not `DROP`,
/// because [`crate::scan`] keeps digits and underscores inside a word.
fn write_kind(word: &str) -> Option<StatementKind> {
    if DML_WORDS.contains(&word) {
        return Some(StatementKind::Dml);
    }
    if DDL_WORDS.contains(&word) {
        return Some(StatementKind::Ddl);
    }
    None
}

/// Statements that change rows.
///
/// `COPY` and `LOAD` are here because both can write, `CALL` and `DO` because a routine
/// body can do anything, and `REPLACE`/`UPSERT` because two engines spell the write that
/// way. `INTO` is not here: it is DDL, for `SELECT … INTO`.
const DML_WORDS: [&str; 10] = [
    "INSERT", "UPDATE", "DELETE", "MERGE", "REPLACE", "UPSERT", "COPY", "LOAD", "CALL", "DO",
];

/// Statements that change structure, or that a read-only connection must not run.
const DDL_WORDS: [&str; 17] = [
    "CREATE", "ALTER", "DROP", "TRUNCATE", "GRANT", "REVOKE", "COMMENT", "RENAME", "REINDEX",
    "VACUUM", "CLUSTER", "ANALYZE", "REFRESH", "ATTACH", "DETACH", "LOCK", "INTO",
];

/// Check a whole script against a Safe Mode.
///
/// Every statement is classified, and the script is refused if **any** of them is. The
/// errant statement is named by its position and quoted, because "one of your statements
/// is not allowed" is not something a user can act on.
pub fn check(mode: SafeMode, sql: &str) -> Result<(), SafeModeError> {
    if mode == SafeMode::Full {
        return Ok(());
    }
    for (offset, statement) in statements(sql).into_iter().enumerate() {
        let kind = classify(statement);
        if let Some(reason) = mode.refusal(kind) {
            return Err(SafeModeError::Refused {
                mode,
                index: offset + 1,
                kind,
                reason,
                statement: snippet(statement),
            });
        }
    }
    Ok(())
}

/// The statements in a script, with the trivia-only pieces dropped.
///
/// Split on the separators [`crate::scan`] found, so a `;` inside a literal, a comment or
/// a dollar-quoted body is content and not a statement boundary. A piece that is only
/// whitespace and comments is not a statement and does not get a number — which is why
/// this is the list a refusal counts, and not [`crate::statement_count`].
pub fn statements(sql: &str) -> Vec<&str> {
    if !has_significant_text(sql) {
        return Vec::new();
    }
    let scan = scan(sql);
    let mut found = Vec::new();
    let mut start = 0;
    for &separator in &scan.separators {
        push(&mut found, &sql[start..separator]);
        start = separator + 1;
    }
    push(&mut found, &sql[start..]);
    found
}

fn push<'a>(found: &mut Vec<&'a str>, piece: &'a str) {
    let trimmed = piece.trim();
    if has_significant_text(trimmed) {
        found.push(trimmed);
    }
}

/// A shortened, single-line rendering of a statement for a message.
fn snippet(sql: &str) -> String {
    const LIMIT: usize = 80;
    let flattened = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = flattened.chars();
    let head: String = chars.by_ref().take(LIMIT).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_read_is_read_only() {
        for sql in [
            "SELECT 1",
            "select * from people",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "VALUES (1), (2)",
            "TABLE people",
            "SHOW CREATE TABLE people",
            "EXPLAIN SELECT * FROM people",
        ] {
            assert_eq!(classify(sql), StatementKind::ReadOnly, "{sql}");
        }
    }

    #[test]
    fn a_write_is_a_write_however_it_is_hidden() {
        // The leading keyword is enough for the ordinary spellings.
        for (sql, kind) in [
            ("INSERT INTO people VALUES (1)", StatementKind::Dml),
            ("update people set a = 1", StatementKind::Dml),
            ("DELETE FROM people", StatementKind::Dml),
            ("MERGE INTO people USING other ON true", StatementKind::Dml),
            ("COPY people FROM STDIN", StatementKind::Dml),
            // A data-modifying CTE starts with WITH and writes anyway.
            (
                "WITH gone AS (DELETE FROM people RETURNING *) SELECT * FROM gone",
                StatementKind::Dml,
            ),
            // A read that takes locks is not a read.
            ("SELECT * FROM people FOR UPDATE", StatementKind::Dml),
            // `SELECT … INTO` creates a table.
            ("SELECT * INTO archive FROM people", StatementKind::Ddl),
            ("DROP TABLE people", StatementKind::Ddl),
            ("CREATE TABLE t (a int)", StatementKind::Ddl),
            ("ALTER TABLE people ADD COLUMN b int", StatementKind::Ddl),
            ("TRUNCATE people", StatementKind::Ddl),
            ("VACUUM people", StatementKind::Ddl),
            ("EXPLAIN ANALYZE SELECT 1", StatementKind::Ddl),
            // A routine call can write whatever its body writes, and an anonymous
            // block can do anything.
            ("CALL do_something()", StatementKind::Dml),
            ("DO $$ BEGIN DELETE FROM people; END $$", StatementKind::Dml),
            ("GRANT SELECT ON people TO reader", StatementKind::Ddl),
        ] {
            assert_eq!(classify(sql), kind, "{sql}");
        }
    }

    #[test]
    fn a_keyword_inside_text_is_not_a_keyword() {
        for sql in [
            "SELECT 'DROP TABLE people' AS note",
            "SELECT \"delete\" FROM t",
            "SELECT 1 /* UPDATE t SET a = 1 */",
            "SELECT 1 -- DROP TABLE people",
            // A word that merely contains a keyword is not that keyword.
            "SELECT updated_at, created_at, drop2 FROM t",
        ] {
            assert_eq!(classify(sql), StatementKind::ReadOnly, "{sql}");
        }
    }

    #[test]
    fn something_the_classifier_cannot_read_is_unknown_not_allowed() {
        // Session control and prepared-statement plumbing are neither reads nor the
        // writes this classifier names, so it refuses to claim they are safe.
        for sql in [
            "SET search_path = public",
            "BEGIN",
            "COMMIT",
            "ROLLBACK",
            "USE otherdb",
            "PREPARE p AS SELECT 1",
            "DEALLOCATE p",
        ] {
            assert_eq!(classify(sql), StatementKind::Unknown, "{sql}");
            // And an unclassified statement is refused by the strict modes.
            assert!(SafeMode::ReadOnly.refusal(classify(sql)).is_some(), "{sql}");
            assert!(SafeMode::NoDdl.refusal(classify(sql)).is_some(), "{sql}");
        }
    }

    #[test]
    fn full_allows_everything_no_ddl_allows_writes_but_not_structure() {
        assert_eq!(SafeMode::parse("full"), Some(SafeMode::Full));
        assert_eq!(SafeMode::parse("no_ddl"), Some(SafeMode::NoDdl));
        assert_eq!(SafeMode::parse("read_only"), Some(SafeMode::ReadOnly));
        assert_eq!(SafeMode::parse("READ_ONLY"), Some(SafeMode::ReadOnly));
        assert_eq!(SafeMode::parse("  no_ddl "), Some(SafeMode::NoDdl));
        // A spelling nobody knows is refused rather than defaulted: `maybe` is a bug the
        // caller needs to see, not a quiet `full`.
        assert_eq!(SafeMode::parse("maybe"), None);
        assert_eq!(SafeMode::parse(""), None);

        // Full refuses nothing; that is the property that keeps the CLI unchanged.
        for kind in [
            StatementKind::ReadOnly,
            StatementKind::Dml,
            StatementKind::Ddl,
            StatementKind::Unknown,
        ] {
            assert!(SafeMode::Full.refusal(kind).is_none(), "{kind}");
        }
        // NoDdl allows reads and writes, refuses structure and the unreadable.
        assert!(SafeMode::NoDdl.refusal(StatementKind::ReadOnly).is_none());
        assert!(SafeMode::NoDdl.refusal(StatementKind::Dml).is_none());
        assert!(SafeMode::NoDdl.refusal(StatementKind::Ddl).is_some());
        assert!(SafeMode::NoDdl.refusal(StatementKind::Unknown).is_some());
        // ReadOnly allows only reads.
        assert!(SafeMode::ReadOnly
            .refusal(StatementKind::ReadOnly)
            .is_none());
        assert!(SafeMode::ReadOnly.refusal(StatementKind::Dml).is_some());
        assert!(SafeMode::ReadOnly.refusal(StatementKind::Ddl).is_some());
        assert!(SafeMode::ReadOnly.refusal(StatementKind::Unknown).is_some());
    }

    #[test]
    fn check_refuses_the_write_and_names_it() {
        let error = check(SafeMode::ReadOnly, "SELECT 1; DROP TABLE people").unwrap_err();
        let SafeModeError::Refused {
            index,
            kind,
            statement,
            ..
        } = error;
        assert_eq!(index, 2, "the second statement is the one refused");
        assert_eq!(kind, StatementKind::Ddl);
        assert_eq!(statement, "DROP TABLE people");
    }

    #[test]
    fn a_write_before_a_read_still_refuses_the_whole_script() {
        // The whole script is refused, not the read after the write run anyway.
        let error = check(SafeMode::ReadOnly, "DELETE FROM people; SELECT 1").unwrap_err();
        assert!(error.to_string().contains("statement 1"), "{error}");
    }

    #[test]
    fn check_reads_a_semicolon_inside_text_as_text() {
        // The `;` in the literal does not split, so this is one read-only statement.
        assert!(check(SafeMode::ReadOnly, "SELECT 'a;b;'").is_ok());
        assert!(check(SafeMode::ReadOnly, "SELECT 1;").is_ok());
    }

    #[test]
    fn check_allows_an_empty_script() {
        for sql in ["", "   ", "-- just a comment", ";"] {
            assert!(check(SafeMode::ReadOnly, sql).is_ok(), "{sql:?}");
        }
    }

    #[test]
    fn an_unreadable_statement_is_refused_by_name() {
        let error = check(SafeMode::ReadOnly, "SET search_path = public").unwrap_err();
        assert!(error.to_string().contains("SET"), "{error}");
        assert!(
            error.to_string().contains("could not tell"),
            "the refusal says why it refused: {error}"
        );
    }

    #[test]
    fn statements_are_split_on_real_separators_only() {
        assert_eq!(statements("SELECT 1; SELECT 2").len(), 2);
        assert_eq!(statements("SELECT 'a;b;'").len(), 1);
        assert_eq!(statements("SELECT 1 -- note;").len(), 1);
        // Trivia-only pieces get no number.
        assert_eq!(statements("SELECT 1;; -- x").len(), 1);
    }

    #[test]
    fn a_long_statement_is_shortened_in_the_message() {
        let long = format!("SELECT {}", "x".repeat(200));
        let shortened = snippet(&long);
        assert!(shortened.ends_with('…'));
        assert_eq!(shortened.chars().count(), 81);
    }
}
