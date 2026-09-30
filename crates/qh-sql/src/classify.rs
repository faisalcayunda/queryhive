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

use crate::scan::{first_significant, has_significant_text_dialect, scan_dialect, Dialect, Lexer};

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
    decisions_dialect(mode, sql, Dialect::Generic)
}

/// [`decisions`] under `dialect`'s lexical rules, so a MySQL connection reads a MySQL
/// string escape and a MySQL comment the way its server will.
pub fn decisions_dialect(
    mode: SafeMode,
    sql: &str,
    dialect: impl Into<Lexer>,
) -> Vec<StatementDecision<'_>> {
    let dialect: Lexer = dialect.into();
    statements_dialect(sql, dialect)
        .into_iter()
        .enumerate()
        .map(|(offset, statement)| {
            let kind = classify_dialect(statement, dialect);
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

/// What one statement does, by reading its text (generic lexical rules).
pub fn classify(sql: &str) -> StatementKind {
    classify_dialect(sql, Dialect::Generic)
}

/// [`classify`] under `dialect`'s lexical rules.
pub fn classify_dialect(sql: &str, dialect: impl Into<Lexer>) -> StatementKind {
    let lexer: Lexer = dialect.into();
    let scan = scan_dialect(sql, lexer);
    // The text names something that can change what the next bytes mean: a session setting
    // the classifier's own reading depends on. Unclassifiable, so refused below `full`.
    if lexer.is_postgres() && changes_session_encoding(sql) {
        return StatementKind::Unknown;
    }
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
        "SELECT" | "VALUES" | "TABLE" | "WITH" | "EXPLAIN" => {
            // The strictest word wins, not the first: `DO` (Unknown) before `DELETE` (Dml)
            // must not lower the answer.
            let word_kind = scan
                .keywords
                .iter()
                .filter_map(|word| write_kind(word))
                .max_by_key(|kind| kind_rank(*kind));
            let lock_kind = locking_clause(&scan.keywords).then_some(StatementKind::Dml);
            [word_kind, lock_kind]
                .into_iter()
                .flatten()
                .max_by_key(|kind| kind_rank(*kind))
                .unwrap_or(StatementKind::ReadOnly)
        }
        // A `COPY` that runs a program on the server host is not a row write, it is code
        // execution the classifier will not vouch for.
        "COPY" if scan.keywords.iter().any(|word| word == "PROGRAM") => StatementKind::Unknown,
        // Anything else is judged by its own leading word: a statement that *starts*
        // with `INSERT` is a write even if it contains no other write word.
        other => write_kind(other).unwrap_or(StatementKind::Unknown),
    }
}

/// Whether the words hold a row-locking clause beyond `FOR UPDATE` (which `UPDATE` catches):
/// `FOR SHARE`, `FOR KEY SHARE` and `FOR NO KEY UPDATE` take row locks like `FOR UPDATE` does.
/// A column named `key`, `no` or `share` right after a `FOR` is a false positive, and a refused
/// read is the cheap side of that.
fn locking_clause(words: &[String]) -> bool {
    words
        .windows(2)
        .any(|pair| pair[0] == "FOR" && matches!(pair[1].as_str(), "SHARE" | "KEY" | "NO"))
}

/// Whether PostgreSQL text could switch the session's `client_encoding`, or hides the name of
/// the function that does. In a multibyte client encoding whose second byte can be `\`, a later
/// `E'…'` string ends somewhere no reading here predicts, so text that can move the encoding is
/// not read-only however it looks. Textual and case-insensitive on purpose: `"set_config"` and
/// `pg_catalog.set_config` are the same function, so a keyword scan that skips quoted
/// identifiers is not enough, and a `U&"…"` identifier can spell any name through an escape.
fn changes_session_encoding(sql: &str) -> bool {
    let lower = sql.to_ascii_lowercase();
    lower.contains("set_config") || lower.contains("client_encoding") || lower.contains("u&\"")
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
    if OPAQUE_WORDS.contains(&word) {
        return Some(StatementKind::Unknown);
    }
    None
}

/// Statements that change rows.
///
/// `COPY` and `LOAD` are here because both can write, `CALL` and `DO` because a routine
/// body can do anything, and `REPLACE`/`UPSERT` because two engines spell the write that
/// way. `INTO` is not here: it is DDL, for `SELECT … INTO`.
const DML_WORDS: [&str; 8] = [
    "INSERT", "UPDATE", "DELETE", "MERGE", "REPLACE", "UPSERT", "COPY", "LOAD",
];

/// Words that run code the classifier cannot read, or a statement prepared elsewhere: a `DO`
/// block or a `CALL` runs a routine body that can do anything, and `EXECUTE` runs a statement
/// prepared earlier, so none of them is a row write `no_ddl` should let through.
const OPAQUE_WORDS: [&str; 3] = ["DO", "CALL", "EXECUTE"];

/// Statements that change structure, or that a read-only connection must not run.
const DDL_WORDS: [&str; 18] = [
    "CREATE", "ALTER", "DROP", "TRUNCATE", "GRANT", "REVOKE", "COMMENT", "RENAME", "REINDEX",
    "VACUUM", "CLUSTER", "ANALYZE", "ANALYSE", "REFRESH", "ATTACH", "DETACH", "LOCK", "INTO",
];

/// Check a whole script against a Safe Mode, with no confirmation given.
///
/// The caller that has one — the engine, which reads `SAFE_MODE_CONFIRMED` — uses
/// [`check_confirmed`]. This is the shape every caller that predates the `confirm` level
/// uses, and it is what a `confirm` connection refuses until the caller says otherwise.
pub fn check(mode: SafeMode, sql: &str) -> Result<(), SafeModeError> {
    check_confirmed(mode, false, sql)
}

/// [`check`] under `dialect`'s lexical rules.
pub fn check_dialect(
    mode: SafeMode,
    sql: &str,
    dialect: impl Into<Lexer>,
) -> Result<(), SafeModeError> {
    check_confirmed_dialect(mode, false, sql, dialect)
}

/// Check a whole script against a Safe Mode, with the caller's confirmation.
///
/// Every statement is classified, and the script is refused if **any** of them is not
/// allowed. The errant statement is named by its position and quoted, because "one of your
/// statements is not allowed" is not something a user can act on. A statement a `confirm`
/// mode would run only after a confirmation is refused when `confirmed` is `false`, with
/// [`SafeModeError::NeedsConfirmation`] so the caller can tell the two apart.
pub fn check_confirmed(mode: SafeMode, confirmed: bool, sql: &str) -> Result<(), SafeModeError> {
    check_confirmed_dialect(mode, confirmed, sql, Dialect::Generic)
}

/// [`check_confirmed`] under `dialect`'s lexical rules.
pub fn check_confirmed_dialect(
    mode: SafeMode,
    confirmed: bool,
    sql: &str,
    dialect: impl Into<Lexer>,
) -> Result<(), SafeModeError> {
    decisions_dialect(mode, sql, dialect)
        .into_iter()
        .find_map(|statement| statement.refusal_error(mode, confirmed))
        .map_or(Ok(()), Err)
}

/// [`check_confirmed_dialect`] over several readings at once.
pub fn check_confirmed_readings(
    mode: SafeMode,
    confirmed: bool,
    sql: &str,
    readings: &[Lexer],
) -> Result<(), SafeModeError> {
    decisions_readings(mode, sql, readings)
        .into_iter()
        .find_map(|statement| statement.refusal_error(mode, confirmed))
        .map_or(Ok(()), Err)
}

/// The refusal reason when the lexers a MySQL server might be using read a script
/// differently, worded for the mode that refuses it.
///
/// The one place the refusal names the cause: the same bytes are one statement or two, or
/// a read or a write, depending on a server `sql_mode` the guard cannot see, and a user who
/// only read "could not tell" would not know that rewriting a quote is the way out.
macro_rules! ambiguous_reason {
    ($who:literal) => {
        concat!(
            "the classifier could not tell where this statement ends or what it does, because \
             MySQL string escaping and comments depend on the server's sql_mode (write a quote \
             inside a string as '' instead of \\' and avoid unusual comments), and ",
            $who,
            " refuses an unclassified statement"
        )
    };
}

/// [`ambiguous_reason!`] for PostgreSQL, whose string escaping depends on
/// `standard_conforming_strings` instead.
macro_rules! ambiguous_reason_postgres {
    ($who:literal) => {
        concat!(
            "the classifier could not tell where this statement ends or what it does, because \
             PostgreSQL reads a backslash in a plain string differently depending on \
             standard_conforming_strings (write a quote inside a string as '' instead of \\', \
             or use an E'' string), and ",
            $who,
            " refuses an unclassified statement"
        )
    };
}

/// [`ambiguous_reason!`] for `mode`, or `None` for `full`, which refuses nothing. `postgres`
/// picks the PostgreSQL wording.
fn ambiguous_reason(mode: SafeMode, postgres: bool) -> Option<&'static str> {
    match (mode, postgres) {
        (SafeMode::Full, _) => None,
        (SafeMode::NoDdl, false) => Some(ambiguous_reason!("a no_ddl connection")),
        (SafeMode::Confirm, false) => Some(ambiguous_reason!("a confirm connection")),
        (SafeMode::ReadOnly, false) => Some(ambiguous_reason!("a read-only connection")),
        (SafeMode::NoDdl, true) => Some(ambiguous_reason_postgres!("a no_ddl connection")),
        (SafeMode::Confirm, true) => Some(ambiguous_reason_postgres!("a confirm connection")),
        (SafeMode::ReadOnly, true) => Some(ambiguous_reason_postgres!("a read-only connection")),
    }
}

/// [`decisions_dialect`] over several readings at once.
///
/// Fails closed: when the readings disagree about where the statements are, a single
/// `Unknown` decision for the whole text is returned, which every mode but `full` refuses;
/// otherwise each statement takes the strictest kind any reading gives it.
pub fn decisions_readings<'a>(
    mode: SafeMode,
    sql: &'a str,
    readings: &[Lexer],
) -> Vec<StatementDecision<'a>> {
    let postgres = readings.iter().any(|lexer| lexer.is_postgres());
    let Some(statements) = statements_agreeing(sql, readings) else {
        return vec![StatementDecision {
            index: 1,
            kind: StatementKind::Unknown,
            statement: sql.trim(),
            decision: mode.decision(StatementKind::Unknown),
            reason: ambiguous_reason(mode, postgres),
        }];
    };
    statements
        .into_iter()
        .enumerate()
        .map(|(offset, statement)| {
            let kind = classify_readings(statement, readings);
            // Unknown because the readings differ (one says read, another cannot say) is
            // the ambiguity case; Unknown because the statement is `SET …` is not.
            let ambiguous = kind == StatementKind::Unknown
                && readings
                    .iter()
                    .any(|lexer| classify_dialect(statement, *lexer) != StatementKind::Unknown);
            StatementDecision {
                index: offset + 1,
                kind,
                statement,
                decision: mode.decision(kind),
                reason: if ambiguous {
                    ambiguous_reason(mode, postgres)
                } else {
                    mode.refusal(kind)
                },
            }
        })
        .collect()
}

/// The refusal rank of a kind, so the strictest reading wins: ReadOnly < Dml < Ddl <
/// Unknown.
fn kind_rank(kind: StatementKind) -> u8 {
    match kind {
        StatementKind::ReadOnly => 0,
        StatementKind::Dml => 1,
        StatementKind::Ddl => 2,
        StatementKind::Unknown => 3,
    }
}

/// The strictest kind any reading gives `sql`. Rank: ReadOnly < Dml < Ddl < Unknown, so
/// a merged kind is always at least as refused as every reading's own. No readings at all
/// is `Unknown`: nothing was checked, so nothing is claimed.
pub fn classify_readings(sql: &str, readings: &[Lexer]) -> StatementKind {
    let mut merged = None;
    for lexer in readings {
        let kind = classify_dialect(sql, *lexer);
        if merged.is_none_or(|current| kind_rank(kind) > kind_rank(current)) {
            merged = Some(kind);
        }
    }
    merged.unwrap_or(StatementKind::Unknown)
}

/// The statements of `sql`, when every reading delimits them identically; `None` when
/// the readings disagree (or there are none). W3-T0.
pub fn statements_agreeing<'a>(sql: &'a str, readings: &[Lexer]) -> Option<Vec<&'a str>> {
    let mut readings = readings.iter();
    let first = readings.next()?;
    let expected = statements_dialect(sql, *first);
    for lexer in readings {
        if statements_dialect(sql, *lexer) != expected {
            return None;
        }
    }
    Some(expected)
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
    statements_with_lines_dialect(sql, Dialect::Generic)
}

/// [`statements_with_lines`] under `dialect`'s lexical rules, so a MySQL import splits on
/// the separators MySQL sees and not the ones a generic scan would.
pub fn statements_with_lines_dialect(
    sql: &str,
    dialect: impl Into<Lexer>,
) -> Vec<ScriptStatement<'_>> {
    let dialect: Lexer = dialect.into();
    if !has_significant_text_dialect(sql, dialect) {
        return Vec::new();
    }
    let scan = scan_dialect(sql, dialect);
    let mut found = Vec::new();
    let mut start = 0;
    for &separator in &scan.separators {
        push_with_line(&mut found, sql, start, separator, dialect);
        start = separator + 1;
    }
    push_with_line(&mut found, sql, start, sql.len(), dialect);
    found
}

/// The statements in a script, with the trivia-only pieces dropped.
///
/// The text-only view of [`statements_with_lines`], kept because most callers
/// classify and never ask where a statement is.
pub fn statements(sql: &str) -> Vec<&str> {
    statements_dialect(sql, Dialect::Generic)
}

/// [`statements`] under `dialect`'s lexical rules.
pub fn statements_dialect(sql: &str, dialect: impl Into<Lexer>) -> Vec<&str> {
    statements_with_lines_dialect(sql, dialect)
        .into_iter()
        .map(|statement| statement.text)
        .collect()
}

fn push_with_line<'a>(
    found: &mut Vec<ScriptStatement<'a>>,
    sql: &'a str,
    start: usize,
    end: usize,
    dialect: Lexer,
) {
    let piece = &sql[start..end];
    // The line of the first significant byte, so a piece that opens with a header
    // comment is still reported on the line its statement really starts on.
    let Some(leading) = first_significant(piece, dialect) else {
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

    // The Safe Mode bypass W3-T0 found: a MySQL string escape and a MySQL comment hide a
    // write from a generic scan, so a read-only connection ran the write. These pin the
    // fix — the classifier reads the write under `Dialect::Mysql`, and `read_only` refuses
    // it.
    const MYSQL_INJECTIONS: [&str; 4] = [
        // `'\''` is one quote in MySQL, so `; DELETE …; ` after it is code.
        "SELECT '\\''; DELETE FROM t; -- '",
        // The `#` comment variant.
        "SELECT '\\''; DELETE FROM t; # '",
        // Through a double-quoted string, which MySQL's default reads as a string.
        "SELECT \"\\\"\"; DELETE FROM t; # \"",
        // A write hidden inside an executable comment the server runs.
        "SELECT 1 /*! ; DROP TABLE t */",
    ];

    #[test]
    fn a_mysql_injection_is_a_write_under_the_mysql_dialect() {
        for sql in MYSQL_INJECTIONS {
            let kind = classify_dialect(sql, Dialect::Mysql);
            assert!(
                matches!(kind, StatementKind::Dml | StatementKind::Ddl),
                "MySQL must see the write in {sql:?}, saw {kind:?}"
            );
            // And a read-only connection refuses the whole script.
            assert!(
                check_dialect(SafeMode::ReadOnly, sql, Dialect::Mysql).is_err(),
                "read_only must refuse {sql:?}"
            );
        }
        // The generic scan is why it slipped through: it reads the first two as one
        // harmless SELECT. This documents the vulnerability the dialect closes; it is not
        // a behaviour to keep.
        assert_eq!(
            classify(MYSQL_INJECTIONS[0]),
            StatementKind::ReadOnly,
            "generic misreads the injection — the bug the dialect fixes"
        );
    }

    #[test]
    fn a_mysql_connection_is_refused_when_the_readings_disagree() {
        for sql in [
            "SELECT '\\'; DELETE FROM t; -- '",
            "SELECT 1 \"\\\" ; DELETE FROM t ; -- \"",
        ] {
            assert!(
                check_confirmed_readings(SafeMode::ReadOnly, false, sql, Dialect::Mysql.readings())
                    .is_err(),
                "read_only must refuse {sql:?}"
            );
        }
    }

    #[test]
    fn a_mysql_connection_refuses_the_default_mode_injections_too() {
        for sql in MYSQL_INJECTIONS {
            assert!(
                check_confirmed_readings(SafeMode::ReadOnly, false, sql, Dialect::Mysql.readings())
                    .is_err(),
                "read_only must refuse {sql:?}"
            );
        }
    }

    #[test]
    fn a_mysql_connection_accepts_a_plain_read() {
        let agreeing = statements_agreeing("SELECT 1; -- note", Dialect::Mysql.readings());
        assert_eq!(agreeing.as_ref().map(Vec::len), Some(1));
        assert!(statements_agreeing("SELECT 1", Dialect::Mysql.readings()).is_some());
    }

    /// The verdict a `read_only` MySQL connection gives `sql`: the guard's readings, all
    /// of them at once.
    fn mysql_read_only(sql: &str) -> Result<(), SafeModeError> {
        check_confirmed_readings(SafeMode::ReadOnly, false, sql, Dialect::Mysql.readings())
    }

    #[test]
    fn a_mysql_dollar_tag_hides_nothing() {
        // MySQL has no dollar quoting, so what sits between two `$tag$` markers is code the
        // server runs. Every one of these carried a `;` or a write past the old readings.
        for sql in [
            "SELECT 1 AS x$a$ ; DELETE FROM t ; $a$",
            "SELECT $a$ ; DELETE FROM t ; $a$",
            "SELECT $$; DELETE FROM t; $$",
            "SELECT $x$ DELETE FROM t $x$",
            "SELECT $x$ DROP TABLE t $x$",
            "SELECT 1 $tag$; INSERT INTO t VALUES (1); $tag$",
            "SELECT $a$ ; SELECT 2 ; $a$",
            "SELECT 1; $a$ ; DELETE FROM t ; $a$",
        ] {
            assert!(
                mysql_read_only(sql).is_err(),
                "read_only must refuse {sql:?}"
            );
        }
        // The same text on PostgreSQL is one dollar-quoted string: unchanged.
        assert!(check(SafeMode::ReadOnly, "SELECT $a$ ; DELETE FROM t ; $a$").is_ok());
        assert!(check(SafeMode::ReadOnly, "SELECT $x$ DELETE FROM t $x$").is_ok());
        assert!(check_confirmed_readings(
            SafeMode::ReadOnly,
            false,
            "SELECT $a$ ; DELETE FROM t ; $a$",
            Dialect::Generic.readings()
        )
        .is_ok());
    }

    #[test]
    fn a_mysql_hash_comment_is_not_a_write() {
        // The over-refusals of the old Generic stand-in: `#` is a comment on MySQL under
        // every sql_mode, so the words after it are text.
        for sql in [
            "SELECT 1 # plain hash comment",
            "SELECT 1 # remember to update this later",
            "SELECT 1 # drop table t",
            "SELECT 1 -- delete everything\n# and update it",
            "SELECT a FROM t # insert here\nWHERE a = 1",
        ] {
            assert!(mysql_read_only(sql).is_ok(), "a read: {sql:?}");
        }
    }

    #[test]
    fn a_mysql_dash_comment_needs_whitespace() {
        assert!(mysql_read_only("SELECT 1 -- x").is_ok());
        assert!(mysql_read_only("SELECT 1 --\tx").is_ok());
        assert!(mysql_read_only("SELECT 1 --").is_ok());
        assert!(mysql_read_only("SELECT 1; -- trailing").is_ok());
        assert!(mysql_read_only("SELECT 5--2").is_ok());
        assert!(mysql_read_only("SELECT 5 - -2").is_ok());
        // `--x` is not a comment: the `;` after it separates and the write runs.
        assert!(mysql_read_only("SELECT 1 --x; DELETE FROM t").is_err());
        assert!(mysql_read_only("SELECT 1 --x\n; DROP TABLE t").is_err());
        // The `#` line ends at the newline and what follows is code.
        assert!(mysql_read_only("SELECT 1 # x\n; DELETE FROM t").is_err());
    }

    #[test]
    fn a_mysql_executable_comment_is_refused_when_it_holds_a_write() {
        for sql in [
            "SELECT 1 /*! DELETE FROM t */",
            "SELECT 1 /*! ; DROP TABLE t */",
            "SELECT 1 /*!50000 ; DELETE FROM t */",
            "SELECT 1 /*!50000 DELETE FROM t */",
            // A gate above the server's version is skipped by MySQL; the guard cannot know
            // the version, so a lexer that runs it refuses.
            "SELECT 1 /*!99999 ; DELETE FROM t */",
            "/*!50000 DELETE FROM t */",
            // An unbalanced quote inside a body one lexer skips and another runs.
            "SELECT 1 /*!99999 ' */ ; DELETE FROM t ; -- '",
            "SELECT 1 /*!' */ ; DELETE FROM t ; -- '",
            "SELECT 1 /*!99999 \" */ ; DROP TABLE t ; -- \"",
        ] {
            assert!(
                mysql_read_only(sql).is_err(),
                "read_only must refuse {sql:?}"
            );
        }
    }

    #[test]
    fn a_mysql_executable_comment_that_is_only_a_read_passes() {
        for sql in [
            "SELECT /*!40001 SQL_NO_CACHE */ * FROM t",
            "SELECT /*!50000 1 */",
            "SELECT /*+ MAX_EXECUTION_TIME(1000) */ * FROM t",
            "SELECT /*+ NO_INDEX(t) */ 1",
            "SELECT 1 /* update this */",
            // MariaDB's spelling is a plain comment on MySQL, whatever its body says.
            "SELECT 1 /*M! DELETE FROM t */",
            "SELECT 1 /*M! ; DROP TABLE t */",
            "/*M! DELETE FROM t */ SELECT 1",
        ] {
            assert!(mysql_read_only(sql).is_ok(), "a read: {sql:?}");
        }
    }

    #[test]
    fn a_mysql_backslash_or_ansi_quotes_payload_is_refused() {
        for sql in [
            // NO_BACKSLASH_ESCAPES: the string ends at the backslash's quote.
            "SELECT '\\'; DELETE FROM t; -- '",
            "SELECT '\\'; DELETE FROM t; # '",
            "SELECT 'a\\'; DROP TABLE t; -- '",
            // Default mode: `\'` is one quote and the string runs on to the `; DELETE`.
            "SELECT '\\''; DELETE FROM t; -- '",
            "SELECT '\\''; DELETE FROM t; # '",
            // ANSI_QUOTES: `"…"` is an identifier, so a backslash does not escape.
            "SELECT 1 \"\\\" ; DELETE FROM t ; -- \"",
            "SELECT \"\\\"; DELETE FROM t; -- \"",
            "SELECT \"\\\"\"; DELETE FROM t; # \"",
            // Escapes hiding a `;` alone.
            "SELECT '\\'; SELECT 2; -- '",
        ] {
            assert!(
                mysql_read_only(sql).is_err(),
                "read_only must refuse {sql:?}"
            );
        }
    }

    #[test]
    fn ordinary_mysql_reads_still_pass() {
        for sql in [
            "SELECT 1",
            "SELECT 1;",
            "SELECT 'a#b', 'a--b', 'a;b' FROM t",
            "SELECT * FROM t WHERE a LIKE 'a\\_b'",
            "SELECT * FROM t WHERE a LIKE 'a\\_b%' ESCAPE '\\\\'",
            "SELECT JSON_EXTRACT(doc, '$.a.b') FROM t",
            "SELECT doc->>'$.name', doc->'$.tags[0]' FROM t",
            "SELECT 'it''s' AS a, \"say \"\"hi\"\"\" AS b",
            "SELECT `a;b`, `c#d` FROM t",
            "SELECT a$b FROM t$1",
            "SELECT 5--2",
            "SELECT 1; -- trailing",
            "SELECT 1 /* c */ ;",
            "WITH x AS (SELECT 1) SELECT * FROM x # note",
            "SHOW CREATE TABLE t",
            "EXPLAIN SELECT * FROM t",
            "SELECT * FROM t WHERE updated_at > '2020-01-01' -- created_at",
        ] {
            assert!(
                mysql_read_only(sql).is_ok(),
                "a read: {sql:?}: {:?}",
                mysql_read_only(sql)
            );
        }
    }

    #[test]
    fn a_mysql_script_is_refused_when_any_statement_is_a_write_or_unreadable() {
        for sql in [
            "SELECT 1; DELETE FROM t",
            "DELETE FROM t",
            "SET sql_mode = ''",
            "SELECT 1; SET @a = 1",
        ] {
            assert!(
                mysql_read_only(sql).is_err(),
                "read_only must refuse {sql:?}"
            );
        }
        // Whole-script refusal names the ambiguity when the readings disagree.
        let error = mysql_read_only("SELECT '\\'; DELETE FROM t; -- '").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("could not tell"), "{message}");
        assert!(message.contains("MySQL string escaping"), "{message}");
        assert!(message.contains("''"), "{message}");
        // A plain unclassified word is not blamed on escaping.
        let error = mysql_read_only("SET sql_mode = ''").unwrap_err();
        assert!(
            !error.to_string().contains("MySQL string escaping"),
            "{error}"
        );
    }

    #[test]
    fn mysql_readings_fail_closed_on_every_kind_and_every_mode() {
        for sql in [
            "SELECT $a$ ; DELETE FROM t ; $a$",
            "SELECT 1 /*! DELETE FROM t */",
            "SELECT '\\'; DELETE FROM t; -- '",
        ] {
            for mode in [SafeMode::Confirm, SafeMode::ReadOnly] {
                let decisions = decisions_readings(mode, sql, Dialect::Mysql.readings());
                assert!(
                    decisions.iter().any(|d| d.decision != Decision::Allow),
                    "{mode:?} must not allow {sql:?}"
                );
            }
            // `full` refuses nothing, by design.
            assert!(check_confirmed_readings(
                SafeMode::Full,
                false,
                sql,
                Dialect::Mysql.readings()
            )
            .is_ok());
        }
        // The Generic set is byte-identical to the single-dialect path.
        for sql in [
            "SELECT 1; DROP TABLE t",
            "SET x = 1",
            "SELECT $a$ ; $a$",
            "SELECT 1",
        ] {
            assert_eq!(
                decisions_readings(SafeMode::ReadOnly, sql, Dialect::Generic.readings()),
                decisions_dialect(SafeMode::ReadOnly, sql, Dialect::Generic),
                "{sql}"
            );
        }
    }

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
            // block can do anything: neither is a row write the classifier can vouch for,
            // so both are unclassified and `no_ddl` refuses them too (W3-T0b).
            ("CALL do_something()", StatementKind::Unknown),
            (
                "DO $$ BEGIN DELETE FROM people; END $$",
                StatementKind::Unknown,
            ),
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

    // W3-T0b: PostgreSQL. Three payloads a review found hid a write from the generic
    // reading, and the readings a PostgreSQL connection must satisfy now refuse them.
    const POSTGRES_INJECTIONS: [&str; 3] = [
        // `E'…'` honours the backslash, so `\'` does not close the string.
        "SELECT E'x\\' AS a, '; DELETE FROM t; --'",
        // PostgreSQL block comments nest, so `/* /* */ ' */` is one comment.
        "SELECT 1 /* /* */ ' */; DELETE FROM t; --'",
        // `$` continues an identifier, so `a$x$` is a name and not a dollar quote.
        "SELECT 1 AS a$x$; DELETE FROM t; --$x$",
    ];

    fn pg_check(mode: SafeMode, sql: &str) -> Result<(), SafeModeError> {
        check_confirmed_readings(mode, false, sql, Dialect::Postgres.readings())
    }

    fn pg_read_only(sql: &str) -> Result<(), SafeModeError> {
        pg_check(SafeMode::ReadOnly, sql)
    }

    #[test]
    fn a_postgres_injection_is_refused_by_every_mode_that_refuses_a_write() {
        for sql in POSTGRES_INJECTIONS {
            // The generic reading is the bug: each of these is one harmless SELECT to it.
            assert_eq!(classify(sql), StatementKind::ReadOnly, "{sql:?}");
            // The PostgreSQL reading sees the DELETE.
            for lexer in Dialect::Postgres.readings() {
                assert_eq!(
                    classify_dialect(sql, *lexer),
                    StatementKind::Dml,
                    "{lexer:?} {sql:?}"
                );
            }
            assert_eq!(
                classify_readings(sql, Dialect::Postgres.readings()),
                StatementKind::Dml
            );
            assert!(matches!(
                pg_read_only(sql),
                Err(SafeModeError::Refused {
                    kind: StatementKind::Dml,
                    index: 2,
                    ..
                })
            ));
            assert!(pg_check(SafeMode::Confirm, sql).is_err(), "{sql:?}");
            // `no_ddl` lets a write run, so only the DDL variant of the payload is refused.
            assert!(pg_check(SafeMode::NoDdl, sql).is_ok(), "{sql:?}");
            let ddl = sql.replace("DELETE FROM t", "DROP TABLE t");
            assert!(pg_check(SafeMode::NoDdl, &ddl).is_err(), "{ddl:?}");
            assert!(pg_read_only(&ddl).is_err(), "{ddl:?}");
            // `full` refuses nothing, by design.
            assert!(pg_check(SafeMode::Full, sql).is_ok());
            // And the dialect entry points agree with the readings.
            assert!(check_dialect(SafeMode::ReadOnly, sql, Dialect::Postgres).is_err());
        }
    }

    #[test]
    fn more_postgres_payloads_are_refused_under_read_only() {
        for sql in [
            // Lower-case `e`, and a write in the same statement as the E string.
            "SELECT e'x\\' AS a, '; DELETE FROM t; --'",
            "SELECT E'\\'' AS a; UPDATE t SET a = 1; --'",
            // A `--` comment ends at a carriage return.
            "SELECT 1 -- x\r; DELETE FROM t",
            // A backtick is an operator character, not a quote.
            "SELECT ` ; DELETE FROM t ; -- `",
            // A non-ASCII letter is part of an identifier, so `é$x$` is one name.
            "SELECT 1 AS é$x$; DELETE FROM t; --$x$",
            "SELECT 1 AS a1$$; DELETE FROM t; --$$",
            // A dollar quote that starts straight after a number closes where it says.
            "SELECT 1$q$ $q$; DELETE FROM t",
            // Nested comments, deeper.
            "SELECT 1 /* /* /* */ */ */ ; DELETE FROM t; /* ' */",
            "SELECT 1 /* a /* b */ ' c */; INSERT INTO t VALUES (1); --'",
            // A continued E string keeps honouring the backslash in its second piece.
            "SELECT E'a'\n'x\\'y'; DELETE FROM t; --'",
            "SELECT E'a' -- c\n'x\\'y'; DROP TABLE t; --'",
            // A bit string has no backslash, and a doubled quote in it starts a new string.
            "SELECT B'1\\'; DELETE FROM t; --'",
            "SELECT U&'\\'; DELETE FROM t; --'",
            // The write is the first statement, or the second, or after a comment.
            "DELETE FROM t; SELECT 1",
            "/* /* */ */ DELETE FROM t",
            "SELECT 1; /* /* */ */ DELETE FROM t",
            // A write in the dollar-quoted body is text, but a write after it is code.
            "SELECT $a$ x $a$; DELETE FROM t",
            // A write inside a CTE.
            "WITH d AS (DELETE FROM t RETURNING *) SELECT * FROM d",
            "SELECT * FROM t FOR UPDATE",
            "SELECT * INTO u FROM t",
            // A keyword that only *starts* like a read is a name, so it cannot be classified.
            "select$x 1",
        ] {
            assert!(pg_read_only(sql).is_err(), "read_only must refuse {sql:?}");
        }
    }

    #[test]
    fn a_postgres_string_the_two_settings_read_differently_is_refused_and_says_why() {
        // Under `standard_conforming_strings = on` the DELETE is inside a string; under
        // `off` it is code. The server the guard cannot see decides, so the guard refuses.
        let sql = "SELECT 'x\\' AS a, '; DELETE FROM t; --'";
        let error = pg_read_only(sql).expect_err("ambiguous text is refused");
        let SafeModeError::Refused { kind, reason, .. } = error else {
            panic!("expected a refusal");
        };
        assert_eq!(kind, StatementKind::Unknown);
        assert!(reason.contains("standard_conforming_strings"), "{reason}");
        assert!(!reason.contains("sql_mode"), "{reason}");
        // The default reading alone would have let it through: that is the bug this closes.
        assert!(check_dialect(SafeMode::ReadOnly, sql, Dialect::Postgres).is_ok());
        assert!(pg_check(SafeMode::NoDdl, sql).is_err());
        assert!(pg_check(SafeMode::Confirm, sql).is_err());
        // `full` still runs it, and MySQL keeps its own wording.
        assert!(pg_check(SafeMode::Full, sql).is_ok());
        let mysql = check_confirmed_readings(
            SafeMode::ReadOnly,
            false,
            "SELECT '\\'; DELETE FROM t; -- '",
            Dialect::Mysql.readings(),
        );
        assert!(
            matches!(mysql, Err(SafeModeError::Refused { reason, .. }) if reason.contains("sql_mode"))
        );
        // A Windows path that ends in a backslash is the cost: valid with the setting on,
        // and with a second statement after it ambiguous across both settings, so it is
        // refused rather than guessed at.
        assert!(pg_read_only("SELECT 'C:\\dir\\'; SELECT 2").is_err());
        assert!(pg_read_only("SELECT 'C:\\\\dir\\\\'").is_ok());
        assert!(pg_read_only("SELECT E'C:\\\\dir\\\\'").is_ok());
        assert!(pg_read_only("SELECT 'a\\nb'").is_ok());
    }

    #[test]
    fn ordinary_postgres_reads_still_pass() {
        for sql in [
            "SELECT 1",
            "SELECT 1;",
            "SELECT 1; -- trailing",
            "SELECT 1; /* trailing */",
            "SELECT 1 ; ; -- two terminators",
            // Escape strings.
            "SELECT E'\\n'",
            "SELECT E'it\\'s'",
            "select e'a\\\\b', E'tab\\t'",
            "SELECT E'\\\\'",
            // Comments, nested and not, with no payload.
            "SELECT 1 /* a */",
            "SELECT 1 /* a /* b */ c */",
            "SELECT /* /* x */ */ 1 -- note",
            "/* header /* nested */ */ SELECT 1",
            "SELECT 1 /* it's ; not code */",
            "-- a comment; DELETE\nSELECT 1",
            "SELECT 1 -- DELETE FROM t\r\n",
            // Parameters.
            "SELECT $1, $2 FROM t WHERE a = $1",
            "SELECT * FROM t WHERE a = $1 AND b = $2; -- done",
            // Dollar quotes.
            "SELECT $$a;b$$",
            "SELECT $q$ DELETE FROM t; $q$",
            "SELECT $é$x;y$é$, $a1_$z$a1_$",
            "SELECT $a$ $b$a$ ; SELECT 1",
            // Identifiers with a dollar or a non-ASCII letter.
            "SELECT foo$bar FROM t",
            "SELECT a$1, b$$ FROM t",
            "SELECT é, naïve$x FROM t",
            "SELECT \"a;b\", \"c\"\"d\" FROM t",
            "SELECT \"a\\\" FROM t",
            // Strings.
            "SELECT 'a;b'",
            "SELECT 'it''s; ok'",
            "SELECT 'a'\n'b'",
            "SELECT B'101', X'1F', N'x', U&'d\\0061t'",
            "SELECT * FROM t WHERE updated_at > 1 AND dropped = 2 AND created_at IS NULL",
            // Non-SELECT reads.
            "SHOW search_path",
            "EXPLAIN SELECT 1",
            "VALUES (1), (2)",
            "WITH x AS (SELECT 1) SELECT * FROM x",
        ] {
            assert!(
                pg_read_only(sql).is_ok(),
                "read_only must allow {sql:?}: {:?}",
                pg_read_only(sql)
            );
        }
    }

    #[test]
    fn a_postgres_script_is_refused_when_any_statement_is_a_write_or_unreadable() {
        assert!(pg_read_only("SELECT 1; SELECT 2").is_ok());
        assert!(pg_read_only("SELECT 1; SET x = 1").is_err());
        assert!(pg_read_only("SELECT 1; DELETE FROM t").is_err());
        assert!(pg_read_only("SELECT 1; SELECT $$ ; DELETE FROM t $$").is_ok());
        // Unterminated text cannot be classified as a read by a reading that sees a write.
        assert!(pg_read_only("SELECT E'a\\'; DELETE FROM t").is_ok());
        assert!(pg_read_only("SELECT 'a\\'; DELETE FROM t").is_err());
        // Only trivia is nothing to run, and the classifier says so.
        assert!(pg_read_only("-- nothing").is_ok());
        assert!(pg_read_only("/* /* */ */").is_ok());
    }

    #[test]
    fn postgres_readings_fail_closed_on_every_kind_and_every_mode() {
        for sql in [
            "SELECT 1; DROP TABLE t",
            "SET x = 1",
            "SELECT $a$ ; $a$",
            "SELECT 'a\\'; DELETE FROM t; --'",
            "COPY t FROM STDIN",
        ] {
            for mode in [SafeMode::ReadOnly, SafeMode::Confirm] {
                let decisions = decisions_readings(mode, sql, Dialect::Postgres.readings());
                if sql == "SELECT $a$ ; $a$" {
                    assert!(decisions.iter().all(|d| d.decision == Decision::Allow));
                } else {
                    assert!(
                        decisions.iter().any(|d| d.decision != Decision::Allow),
                        "{mode:?} must not allow {sql:?}"
                    );
                }
            }
            assert!(pg_check(SafeMode::Full, sql).is_ok());
        }
    }

    #[test]
    fn postgres_statements_split_where_the_server_splits() {
        let sql = "SELECT E'a;b'; SELECT 1 /* ; /* ; */ ; */; SELECT $$;$$";
        for lexer in Dialect::Postgres.readings() {
            assert_eq!(
                statements_dialect(sql, *lexer),
                vec![
                    "SELECT E'a;b'",
                    "SELECT 1 /* ; /* ; */ ; */",
                    "SELECT $$;$$"
                ],
                "{lexer:?}"
            );
        }
        // The lines follow the same split, past a nested header comment.
        let lines: Vec<usize> = statements_with_lines_dialect(
            "/* a /* b */ */\nSELECT 1;\n-- c\r\nSELECT 2",
            Dialect::Postgres,
        )
        .iter()
        .map(|statement| statement.line)
        .collect();
        assert_eq!(lines, vec![2, 4]);
        // Two readings that split differently are not a script anyone can read.
        assert!(
            statements_agreeing("SELECT 'x\\'; SELECT 1; --'", Dialect::Postgres.readings())
                .is_none()
        );
        assert!(
            statements_agreeing("SELECT 'x'; SELECT 1", Dialect::Postgres.readings()).is_some()
        );
    }

    #[test]
    fn generic_scanning_is_unchanged_for_trino_and_the_other_callers() {
        // Trino keeps the generic reading: it has no dollar quotes and no `E'…'` strings
        // (a `$` is a syntax error there), block comments do not nest, and it runs one
        // statement per request. A regression here would change a connection this task
        // does not touch.
        for sql in POSTGRES_INJECTIONS {
            assert_eq!(
                classify_dialect(sql, Dialect::Generic),
                StatementKind::ReadOnly
            );
        }
        assert_eq!(Dialect::Generic.readings().len(), 1);
        assert_eq!(Dialect::default(), Dialect::Generic);
    }

    // W3-T0b, second pass: what the review's two blocking bypasses and the cheap fixes named.

    fn trino_read_only(sql: &str) -> Result<(), SafeModeError> {
        check_confirmed_readings(SafeMode::ReadOnly, false, sql, Dialect::Trino.readings())
    }

    #[test]
    fn a_trino_carriage_return_ends_a_line_comment() {
        // Live on Trino: `SELECT 1 -- c\r, 2` returns two columns, so the comment ended at the
        // `\r`. The generic reading waits for a newline and hid the write behind it.
        let payload = "EXPLAIN /* x */ -- note\r ANALYZE INSERT INTO t VALUES (1)";
        assert_eq!(
            classify(payload),
            StatementKind::ReadOnly,
            "generic misreads it"
        );
        assert_ne!(
            classify_dialect(payload, Dialect::Trino),
            StatementKind::ReadOnly
        );
        assert!(trino_read_only(payload).is_err());
        assert!(trino_read_only("SELECT 1 -- c\r; DELETE FROM t").is_err());
        assert!(check_dialect(SafeMode::NoDdl, payload, Dialect::Trino).is_err());
    }

    #[test]
    fn trino_reads_still_pass() {
        for sql in [
            "SELECT 1",
            "SELECT 1 -- note\n, 2",
            "SELECT 1 /* a */ , 2 -- DELETE\r\n",
            "SELECT 'it''s; ok', \"a;b\" FROM t",
            "SELECT 'a\\' , 'b'",
            "SELECT X'ab', U&'d\\0061t' FROM t",
            "SHOW CATALOGS",
            "EXPLAIN SELECT 1",
            "SELECT 1; -- trailing\r",
        ] {
            assert!(
                trino_read_only(sql).is_ok(),
                "{sql:?}: {:?}",
                trino_read_only(sql)
            );
        }
        // Trino does not nest block comments (live: `/* /* */ , 2 */` is a syntax error), so
        // the inner `*/` closes the comment and the write after it is code.
        assert!(trino_read_only("SELECT 1 /* /* */ ; DELETE FROM t /* */ */").is_err());
        // No dollar quotes: a `$` is a syntax error there, and what follows it is read as code.
        assert!(trino_read_only("SELECT 1 $x$; DELETE FROM t; $x$").is_err());
        assert_eq!(Dialect::Trino.readings().len(), 1);
        assert_eq!(Dialect::Trino.readings()[0], Lexer::TRINO);
    }

    #[test]
    fn text_that_can_switch_the_postgres_encoding_is_not_a_read() {
        for sql in [
            "SELECT set_config('client_encoding', 'SJIS', false)",
            "SELECT pg_catalog.set_config('client_encoding', 'GBK', false)",
            "SELECT \"set_config\"('a', 'b', false)",
            "SELECT pg_catalog.\"SET_CONFIG\"('a', 'b', false)",
            "SELECT U&\"set\\005fconfig\"('a', 'b', false)",
            "SELECT 'client_encoding'",
            "SELECT 1 /* client_encoding */",
            "SELECT SET_CONFIG ('x', 'y', true)",
        ] {
            for lexer in Dialect::Postgres.readings() {
                assert_eq!(
                    classify_dialect(sql, *lexer),
                    StatementKind::Unknown,
                    "{sql}"
                );
            }
            for mode in [SafeMode::ReadOnly, SafeMode::Confirm, SafeMode::NoDdl] {
                assert!(pg_check(mode, sql).is_err(), "{mode:?} {sql}");
            }
            assert!(pg_check(SafeMode::Full, sql).is_ok());
        }
        // Only PostgreSQL has the function; the other dialects are not made stricter for it.
        assert_eq!(
            classify_dialect("SELECT set_config FROM t", Dialect::Mysql),
            StatementKind::ReadOnly
        );
        // `SET` itself was always refused.
        assert!(pg_read_only("SET client_encoding = 'SJIS'").is_err());
        assert!(pg_read_only("SELECT current_setting('client_encoding')").is_err());
    }

    #[test]
    fn the_cheap_classifier_fixes() {
        for (sql, kind) in [
            // EXECUTE runs a statement prepared elsewhere: anywhere in the text.
            ("EXECUTE p(1)", StatementKind::Unknown),
            ("EXPLAIN ANALYZE EXECUTE p(1)", StatementKind::Unknown),
            ("EXPLAIN EXECUTE p(1)", StatementKind::Unknown),
            (
                "SELECT 1 FROM (SELECT execute FROM t) x",
                StatementKind::Unknown,
            ),
            ("PREPARE p AS DELETE FROM t", StatementKind::Unknown),
            // ANALYSE is ANALYZE.
            ("ANALYSE people", StatementKind::Ddl),
            ("EXPLAIN ANALYSE SELECT 1", StatementKind::Ddl),
            // DO, CALL and COPY … PROGRAM are code the classifier cannot read.
            ("DO $$ BEGIN NULL; END $$", StatementKind::Unknown),
            ("CALL p()", StatementKind::Unknown),
            ("COPY t TO PROGRAM 'id'", StatementKind::Unknown),
            ("COPY (SELECT 1) TO PROGRAM 'id'", StatementKind::Unknown),
            ("COPY t FROM STDIN", StatementKind::Dml),
            ("COPY t TO '/tmp/x'", StatementKind::Dml),
            // A stricter word does not lose to an earlier, milder one.
            (
                "WITH x AS (SELECT 1) SELECT call, delete FROM t",
                StatementKind::Unknown,
            ),
            (
                "WITH x AS (SELECT 1) SELECT delete, call FROM t",
                StatementKind::Unknown,
            ),
            // Every row-locking clause is a write.
            ("SELECT * FROM t FOR UPDATE", StatementKind::Dml),
            ("SELECT * FROM t FOR SHARE", StatementKind::Dml),
            ("SELECT * FROM t FOR KEY SHARE", StatementKind::Dml),
            ("SELECT * FROM t FOR NO KEY UPDATE", StatementKind::Dml),
            (
                "SELECT * FROM t FOR NO KEY UPDATE SKIP LOCKED",
                StatementKind::Dml,
            ),
            (
                "SELECT * FROM t /* c */ FOR /* c */ SHARE NOWAIT",
                StatementKind::Dml,
            ),
            // A plain FOR is not a lock.
            (
                "SELECT substring(a FROM 1 FOR 2) FROM t",
                StatementKind::ReadOnly,
            ),
            (
                "SELECT * FROM t WHERE a = 'FOR SHARE'",
                StatementKind::ReadOnly,
            ),
        ] {
            for dialect in [
                Dialect::Generic,
                Dialect::Postgres,
                Dialect::Mysql,
                Dialect::Trino,
            ] {
                assert_eq!(classify_dialect(sql, dialect), kind, "{dialect:?} {sql}");
            }
        }
        // no_ddl lets a row write through, so it must refuse the opaque ones by name.
        for sql in [
            "DO $$ BEGIN NULL; END $$",
            "CALL p()",
            "EXECUTE p",
            "COPY t TO PROGRAM 'x'",
            "ANALYSE t",
        ] {
            assert!(check(SafeMode::NoDdl, sql).is_err(), "{sql}");
            assert!(check(SafeMode::Confirm, sql).is_err(), "{sql}");
            assert!(check(SafeMode::ReadOnly, sql).is_err(), "{sql}");
            assert!(check(SafeMode::Full, sql).is_ok(), "{sql}");
        }
        assert!(check(SafeMode::NoDdl, "COPY t FROM STDIN").is_ok());
    }
}
