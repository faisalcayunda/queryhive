//! Deciding what a statement does, and what a connection's Safe Mode does about it.
//!
//! This is the engine's answer to "may this SQL run here", and it lives in `qh-sql`
//! rather than in the app because the engine has three callers and only one of them
//! has a window: the CLI, the MCP server, and the app. A rule kept in a Swift picker
//! would be a rule the other two never see.
//!
//! # The four levels
//!
//! | `SAFE_MODE` | refuses | asks first |
//! |---|---|---|
//! | `full` | nothing | nothing |
//! | `no_ddl` | DDL, and anything the classifier cannot read | nothing |
//! | `confirm` | DDL, and anything the classifier cannot read | every write |
//! | `read_only` | every write, every DDL, and anything the classifier cannot read | nothing |
//!
//! `confirm` is the level between "run it" and "refuse it": a write runs only when the
//! caller has said `SAFE_MODE_CONFIRMED=1` for this run. That is the engine's whole idea
//! of a confirmation — a boolean the caller could only have set after asking its user.
//! Touch ID, a dialog and a keychain prompt are the app's business and are deliberately
//! not modelled here: the CLI and the MCP server have no window to raise, and a level an
//! embedding caller cannot express is a level that would silently become "refuse".
//!
//! The four are a **strictness chain**, which is what lets a floor pick one of them
//! without ever weakening the user's own choice:
//!
//! ```text
//! full < no_ddl < confirm < read_only
//! ```
//!
//! Each step refuses strictly more *unconfirmed* statements than the one before. `confirm`
//! sits above `no_ddl` because it adds a confirmation the user's own level did not demand,
//! and below `read_only` because a confirmed write still runs.
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

use crate::scan::{first_significant, has_significant_text, scan};

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

    /// The stable token a stored record uses for this kind.
    ///
    /// [`label`](Self::label) is a sentence a person reads and may be reworded; this is a
    /// value an execution-log row is compared against, so it is frozen.
    pub const fn token(self) -> &'static str {
        match self {
            StatementKind::ReadOnly => "read_only",
            StatementKind::Dml => "dml",
            StatementKind::Ddl => "ddl",
            StatementKind::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for StatementKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}

/// What a connection refuses, named the way the `SAFE_MODE` setting names it.
///
/// The four variants are ordered by [`strictness`](SafeMode::strictness); that order is
/// what a [`SafeModeFloor`] uses to pick the strictest of several conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafeMode {
    /// Refuses nothing. The engine's default, so the CLI and the golden corpus are
    /// unchanged by this feature's arrival.
    Full,
    /// Allows DML, refuses DDL and anything unclassified.
    NoDdl,
    /// Allows a read; asks for an explicit confirmation before a write; refuses DDL and
    /// anything unclassified.
    Confirm,
    /// Refuses every write, every DDL, and anything unclassified.
    ReadOnly,
}

/// Every spelling the setting accepts, in strictness order, which is also the order a
/// message lists them.
pub const SAFE_MODES: [&str; 4] = ["full", "no_ddl", "confirm", "read_only"];

/// What one mode does about one statement before the caller's confirmation is counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Runs as asked.
    Allow,
    /// Runs when the caller has explicitly confirmed this run, and not otherwise.
    Confirm,
    /// Does not run.
    Refuse,
}

impl Decision {
    /// Whether the statement runs without any further input from the caller.
    pub const fn is_allowed(self) -> bool {
        matches!(self, Decision::Allow)
    }
}

impl SafeMode {
    /// Parse a setting value. `None` for a spelling nobody recognises, rather than a
    /// default: a typo in a safety setting is not a decision to make silently.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "full" => Some(SafeMode::Full),
            "no_ddl" => Some(SafeMode::NoDdl),
            "confirm" => Some(SafeMode::Confirm),
            "read_only" => Some(SafeMode::ReadOnly),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            SafeMode::Full => "full",
            SafeMode::NoDdl => "no_ddl",
            SafeMode::Confirm => "confirm",
            SafeMode::ReadOnly => "read_only",
        }
    }

    /// How restrictive this mode is, as a number a floor can take the maximum of.
    ///
    /// The steps are deliberately total and cumulative: each higher number refuses
    /// strictly more *unconfirmed* statements than the one below it. `no_ddl` (1) lets a
    /// `DELETE` run without asking; `confirm` (2) refuses that `DELETE` until the caller
    /// confirms and still refuses DDL; `read_only` (3) refuses the `DELETE` outright. So a
    /// floor that raises `no_ddl` to `confirm` can never let anything through that the
    /// user's own choice would have refused.
    pub const fn strictness(self) -> u8 {
        match self {
            SafeMode::Full => 0,
            SafeMode::NoDdl => 1,
            SafeMode::Confirm => 2,
            SafeMode::ReadOnly => 3,
        }
    }

    /// The stricter of two modes. Ties return `self`, which is arbitrary and safe: equal
    /// strictness means equal behaviour.
    pub const fn strictest(self, other: Self) -> Self {
        if other.strictness() > self.strictness() {
            other
        } else {
            self
        }
    }

    /// What this mode does about a statement of `kind`, before any confirmation.
    pub const fn decision(self, kind: StatementKind) -> Decision {
        match self {
            SafeMode::Full => Decision::Allow,
            SafeMode::NoDdl => match kind {
                StatementKind::ReadOnly | StatementKind::Dml => Decision::Allow,
                StatementKind::Ddl | StatementKind::Unknown => Decision::Refuse,
            },
            SafeMode::Confirm => match kind {
                StatementKind::ReadOnly => Decision::Allow,
                StatementKind::Dml => Decision::Confirm,
                StatementKind::Ddl | StatementKind::Unknown => Decision::Refuse,
            },
            SafeMode::ReadOnly => match kind {
                StatementKind::ReadOnly => Decision::Allow,
                StatementKind::Dml | StatementKind::Ddl | StatementKind::Unknown => {
                    Decision::Refuse
                }
            },
        }
    }

    /// Why this mode will not run a statement of `kind` as-is, or `None` when it runs
    /// without asking.
    ///
    /// "As-is" is the whole of it: a `Confirm` decision has a reason here because the
    /// statement does not run until the caller confirms, which is exactly the answer
    /// [`crate::check`] needs. The wording is the sentence a user reads, so it says what
    /// the mode is and what the statement was, not which internal enum value matched.
    pub fn refusal(self, kind: StatementKind) -> Option<&'static str> {
        match self.decision(kind) {
            Decision::Allow => None,
            Decision::Confirm => Some(
                "writing runs on a confirm connection only after an explicit confirmation \
                 (SAFE_MODE_CONFIRMED=1)",
            ),
            Decision::Refuse => match self {
                SafeMode::NoDdl => {
                    Some("DDL is not allowed on a connection whose Safe Mode is no_ddl")
                }
                SafeMode::Confirm => match kind {
                    StatementKind::Ddl => {
                        Some("DDL is not allowed on a connection whose Safe Mode is confirm")
                    }
                    _ => Some(
                        "the classifier could not tell whether this statement is read-only, and \
                         a confirm connection refuses an unclassified statement",
                    ),
                },
                SafeMode::ReadOnly => match kind {
                    StatementKind::Dml => Some("writing is not allowed on a read-only connection"),
                    StatementKind::Ddl => Some("DDL is not allowed on a read-only connection"),
                    _ => Some(
                        "the classifier could not tell whether this statement is read-only, and \
                         a read-only connection refuses an unclassified statement",
                    ),
                },
                // `Full` never reaches a `Refuse`, and `Confirm` has no other arm.
                SafeMode::Full => None,
            },
        }
    }
}

impl std::fmt::Display for SafeMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One independent reason a run's mode may not be the level the user chose.
///
/// The order matters and is not cosmetic: it is the tie-break when two sources raise the
/// same level, and the highest source is the one a message should name. A policy outranks
/// a connection, which outranks a driver, which outranks the user's own setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FloorSource {
    /// The user's own `SAFE_MODE`.
    User,
    /// The driver cannot write, whatever the user chose.
    Driver,
    /// The connection is marked read-only, whatever the user chose.
    Connection,
    /// A policy outside the connection pinned a minimum.
    Policy,
}

impl FloorSource {
    /// The words a message uses to say where a floor came from.
    pub const fn as_str(self) -> &'static str {
        match self {
            FloorSource::User => "the connection's own Safe Mode",
            FloorSource::Driver => "the driver, which cannot write",
            FloorSource::Connection => "the connection, which is marked read-only",
            FloorSource::Policy => "a policy pinned outside the connection",
        }
    }
}

impl std::fmt::Display for FloorSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Several independent conditions, each naming a minimum, resolved to the strictest one.
///
/// This exists so a floor can never be the *first* condition that happened to be true.
/// TablePro learned that the hard way: a first-match chain is only right while the order
/// is right, and adding a fourth condition is what makes it wrong. Resolving by
/// [`SafeMode::strictness`] makes the answer independent of the order the conditions were
/// added in.
///
/// A floor is a value computed for one run and never written back into the user's
/// setting: when the condition goes away, the user's own level is what remains.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SafeModeFloor {
    entries: Vec<(FloorSource, SafeMode)>,
}

impl SafeModeFloor {
    /// A floor with no conditions in it. Its [`resolve`](Self::resolve) is `None`.
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Add one condition's minimum. Repeated conditions stack, and the strictest wins.
    pub fn raise(&mut self, source: FloorSource, mode: SafeMode) {
        self.entries.push((source, mode));
    }

    /// The strictest condition, and which one it was, or `None` for an empty floor.
    ///
    /// Ties are broken by [`FloorSource`]'s own order, so the message names the most
    /// authoritative reason rather than whichever was pushed first.
    pub fn resolve(&self) -> Option<(SafeMode, FloorSource)> {
        self.entries
            .iter()
            .copied()
            .max_by_key(|(source, mode)| (mode.strictness(), *source))
            .map(|(source, mode)| (mode, source))
    }

    /// Whether no condition has been raised.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The conditions, in the order they were raised.
    pub fn iter(&self) -> impl Iterator<Item = (FloorSource, SafeMode)> + '_ {
        self.entries.iter().copied()
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

    /// A statement the mode would run after an explicit confirmation, which this caller
    /// did not give.
    ///
    /// Its own variant rather than [`SafeModeError::Refused`] because the two ask the
    /// caller for different things: a refusal is final, and this one is a question. A CLI
    /// can offer to re-run with `SAFE_MODE_CONFIRMED=1`; an error string that said
    /// "refused" would read as a dead end.
    #[error(
        "SAFE_MODE={mode} requires confirmation for statement {index} ({kind}): {reason}: \
         {statement}"
    )]
    NeedsConfirmation {
        mode: SafeMode,
        /// 1-based position among the statements the classifier read.
        index: usize,
        kind: StatementKind,
        reason: &'static str,
        /// A flattened, shortened rendering of the statement awaiting confirmation.
        statement: String,
    },
}

/// One statement's decision, and enough of the statement to log it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementDecision<'a> {
    /// 1-based position among the statements the classifier read.
    pub index: usize,
    pub kind: StatementKind,
    /// The statement text, trimmed of its surrounding trivia.
    pub statement: &'a str,
    /// What the mode does about it before the caller's confirmation is counted.
    pub decision: Decision,
    /// Why it does not run as-is, matching [`SafeMode::refusal`].
    pub reason: Option<&'static str>,
}

impl StatementDecision<'_> {
    /// The error this statement raises, or `None` when it runs.
    ///
    /// The single place a [`SafeModeError`] is built from a decision, so [`check`],
    /// [`check_confirmed`] and the engine's execution log cannot disagree about what a
    /// decision means.
    pub fn refusal_error(&self, mode: SafeMode, confirmed: bool) -> Option<SafeModeError> {
        let reason = self.reason.unwrap_or("this statement is not allowed here");
        match self.decision {
            Decision::Allow => None,
            Decision::Confirm if confirmed => None,
            Decision::Confirm => Some(SafeModeError::NeedsConfirmation {
                mode,
                index: self.index,
                kind: self.kind,
                reason,
                statement: snippet(self.statement),
            }),
            Decision::Refuse => Some(SafeModeError::Refused {
                mode,
                index: self.index,
                kind: self.kind,
                reason,
                statement: snippet(self.statement),
            }),
        }
    }
}

/// Every statement in a script with the decision this mode makes about it.
///
/// The statements a refusal counts, in the order it counts them. It returns all of them
/// rather than stopping at the first that is not allowed, because the caller that logs
/// decisions (the engine) needs to say which statement stopped the script and would
/// otherwise have to classify a second time to find out.
pub fn decisions(mode: SafeMode, sql: &str) -> Vec<StatementDecision<'_>> {
    statements(sql)
        .into_iter()
        .enumerate()
        .map(|(offset, statement)| {
            let kind = classify(statement);
            StatementDecision {
                index: offset + 1,
                kind,
                statement,
                decision: mode.decision(kind),
                reason: mode.refusal(kind),
            }
        })
        .collect()
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

/// Check a whole script against a Safe Mode, with no confirmation given.
///
/// The caller that has one — the engine, which reads `SAFE_MODE_CONFIRMED` — uses
/// [`check_confirmed`]. This is the shape every caller that predates the `confirm` level
/// uses, and it is what a `confirm` connection refuses until the caller says otherwise.
pub fn check(mode: SafeMode, sql: &str) -> Result<(), SafeModeError> {
    check_confirmed(mode, false, sql)
}

/// Check a whole script against a Safe Mode, with the caller's confirmation.
///
/// Every statement is classified, and the script is refused if **any** of them is not
/// allowed. The errant statement is named by its position and quoted, because "one of your
/// statements is not allowed" is not something a user can act on. A statement a `confirm`
/// mode would run only after a confirmation is refused when `confirmed` is `false`, with
/// [`SafeModeError::NeedsConfirmation`] so the caller can tell the two apart.
pub fn check_confirmed(mode: SafeMode, confirmed: bool, sql: &str) -> Result<(), SafeModeError> {
    decisions(mode, sql)
        .into_iter()
        .find_map(|statement| statement.refusal_error(mode, confirmed))
        .map_or(Ok(()), Err)
}

/// One statement of a script, and the 1-based line it starts on.
///
/// [`statements`] drops the line because its only caller classifies. An importer
/// needs to say *where* a statement failed, and the line is the only handle a user
/// has on a file of ten thousand statements. Both slices come from the same
/// [`scan`], so the count and the positions cannot disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptStatement<'a> {
    /// The 1-based line of the statement's first significant character.
    ///
    /// A leading comment or blank line does not move it: the line is where the
    /// statement really starts, which is the line an error message should name.
    pub line: usize,
    /// The statement text, with surrounding whitespace and comments trimmed.
    pub text: &'a str,
}

/// The statements in a script, with the line each one starts on.
///
/// Split on the separators [`crate::scan`] found, so a `;` inside a literal, a comment
/// or a dollar-quoted body is content and not a statement boundary. A piece that is only
/// whitespace and comments is not a statement and does not get a line — which is why this
/// is the list a refusal counts, and not [`crate::statement_count`].
pub fn statements_with_lines(sql: &str) -> Vec<ScriptStatement<'_>> {
    if !has_significant_text(sql) {
        return Vec::new();
    }
    let scan = scan(sql);
    let mut found = Vec::new();
    let mut start = 0;
    for &separator in &scan.separators {
        push_with_line(&mut found, sql, start, separator);
        start = separator + 1;
    }
    push_with_line(&mut found, sql, start, sql.len());
    found
}

/// The statements in a script, with the trivia-only pieces dropped.
///
/// The text-only view of [`statements_with_lines`], kept because most callers
/// classify and never ask where a statement is.
pub fn statements(sql: &str) -> Vec<&str> {
    statements_with_lines(sql)
        .into_iter()
        .map(|statement| statement.text)
        .collect()
}

fn push_with_line<'a>(
    found: &mut Vec<ScriptStatement<'a>>,
    sql: &'a str,
    start: usize,
    end: usize,
) {
    let piece = &sql[start..end];
    // The line of the first significant byte, so a piece that opens with a header
    // comment is still reported on the line its statement really starts on.
    let Some(leading) = first_significant(piece) else {
        return;
    };
    found.push(ScriptStatement {
        line: line_at(sql, start + leading),
        text: piece.trim(),
    });
}

/// The 1-based line `offset` sits on.
fn line_at(sql: &str, offset: usize) -> usize {
    sql.as_bytes()[..offset]
        .iter()
        .filter(|&&byte| byte == b'\n')
        .count()
        + 1
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
        } = error
        else {
            panic!("a read_only refusal is final, not a question");
        };
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
    fn statements_carry_the_line_their_first_word_is_on() {
        let found = statements_with_lines("SELECT 1;\n\n  SELECT 2;\n");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].line, 1);
        assert_eq!(found[0].text, "SELECT 1");
        // The blank line and the leading spaces are skipped: the line is where the
        // statement really starts, not where the previous `;` left off.
        assert_eq!(found[1].line, 3);
        assert_eq!(found[1].text, "SELECT 2");
        // Leading comments and whitespace before the first statement do not move it
        // off the line its first word is on.
        let lead = statements_with_lines("-- header\nSELECT 1");
        assert_eq!(lead[0].line, 2);
        // The two views agree on the text.
        assert_eq!(
            statements("SELECT 1;\nSELECT 2")
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            statements_with_lines("SELECT 1;\nSELECT 2")
                .into_iter()
                .map(|statement| statement.text.to_owned())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_long_statement_is_shortened_in_the_message() {
        let long = format!("SELECT {}", "x".repeat(200));
        let shortened = snippet(&long);
        assert!(shortened.ends_with('…'));
        assert_eq!(shortened.chars().count(), 81);
    }

    #[test]
    fn confirm_runs_a_write_only_after_a_confirmation() {
        assert_eq!(SafeMode::parse("confirm"), Some(SafeMode::Confirm));
        assert_eq!(SafeMode::Confirm.as_str(), "confirm");

        // A read needs no confirmation.
        assert!(check(SafeMode::Confirm, "SELECT 1").is_ok());
        assert!(check_confirmed(SafeMode::Confirm, true, "SELECT 1").is_ok());
        // A write is a question, not a dead end: the refusal says so and names the
        // statement, and the same script passes once the caller confirms.
        let error = check(SafeMode::Confirm, "INSERT INTO people VALUES (1)").unwrap_err();
        assert!(
            matches!(error, SafeModeError::NeedsConfirmation { .. }),
            "a write on a confirm connection is a question: {error:?}"
        );
        assert!(
            error.to_string().contains("requires confirmation"),
            "{error}"
        );
        assert!(check_confirmed(SafeMode::Confirm, true, "INSERT INTO people VALUES (1)").is_ok());
        // DDL and the unreadable stay refused even with a confirmation: confirming a
        // statement whose effects the classifier cannot name is not a safety decision.
        assert!(check_confirmed(SafeMode::Confirm, true, "DROP TABLE people").is_err());
        assert!(check_confirmed(SafeMode::Confirm, true, "SET search_path = public").is_err());
    }

    #[test]
    fn the_strictness_order_is_full_no_ddl_confirm_read_only() {
        let order = [
            SafeMode::Full,
            SafeMode::NoDdl,
            SafeMode::Confirm,
            SafeMode::ReadOnly,
        ];
        for pair in order.windows(2) {
            assert!(
                pair[0].strictness() < pair[1].strictness(),
                "{:?} must be stricter than {:?}",
                pair[1],
                pair[0]
            );
            assert_eq!(pair[0].strictest(pair[1]), pair[1]);
            assert_eq!(pair[1].strictest(pair[0]), pair[1]);
        }
    }

    #[test]
    fn a_floor_picks_the_strictest_condition_not_the_first() {
        let mut floor = SafeModeFloor::new();
        assert!(floor.is_empty());
        assert_eq!(floor.resolve(), None);
        // The first condition is the loosest; a first-match chain would return `no_ddl`.
        floor.raise(FloorSource::User, SafeMode::Full);
        floor.raise(FloorSource::Connection, SafeMode::NoDdl);
        floor.raise(FloorSource::Policy, SafeMode::ReadOnly);
        assert_eq!(
            floor.resolve(),
            Some((SafeMode::ReadOnly, FloorSource::Policy))
        );
        // Adding a looser condition after the strict one does not lower the floor.
        floor.raise(FloorSource::Driver, SafeMode::Confirm);
        assert_eq!(
            floor.resolve(),
            Some((SafeMode::ReadOnly, FloorSource::Policy))
        );
    }

    #[test]
    fn a_floor_tie_names_the_most_authoritative_source() {
        // Two sources at the same level: the message should name the policy, not the
        // first one pushed. Strictness alone cannot choose, so the source must.
        let mut floor = SafeModeFloor::new();
        floor.raise(FloorSource::User, SafeMode::ReadOnly);
        floor.raise(FloorSource::Policy, SafeMode::ReadOnly);
        assert_eq!(
            floor.resolve(),
            Some((SafeMode::ReadOnly, FloorSource::Policy))
        );
    }
}
