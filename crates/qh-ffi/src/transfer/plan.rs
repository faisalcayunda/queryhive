//! The statements of a transfer, and the decision about each one (W13-T20 §7.2, B-2, B-3, C-4).
//!
//! The destination is written with its own credentials, so **its** Safe Mode decides, one
//! statement at a time, over the entries of its floor. There are two kinds of statement:
//!
//! - **The engine's own** ([`EngineStatement`]): transaction control, session setup and the
//!   reads that size a write. A closed enum of `&'static str`, never a `format!`, because the
//!   classifier reads `BEGIN`, `COMMIT` and `SET` as `Unknown` and `confirm` refuses `Unknown`
//!   even when confirmed, so they cannot go through it, and because a name from the source
//!   server must have no way into them. They are allowed only if the write plan is allowed,
//!   and they are not logged (the driver's own session statements are not either).
//! - **The written statements** ([`Planned`]): `CREATE`, `COPY`, `INSERT`, the swap and the
//!   clean-up. Each is built here from names that [`super::names`] checked and types that
//!   [`super::types`] rendered, and then, **before it is sent**, read again under the
//!   destination's dialect: it must be exactly one statement, of the kind the plan says
//!   (B-3c). A mismatch is an internal error and nothing is sent. A DDL statement is decided
//!   like `table_op` decides a destructive operation, from the entries; a DML statement like
//!   any write, from the floor's strictest entry. Each decision is logged once per distinct
//!   text, with the hash of the statement that was actually going to run.

use std::collections::HashSet;

use qh_sql::{
    classify_readings, statements_dialect, Decision, Dialect, SafeMode, SafeModeFloor,
    StatementKind,
};

use super::names::{self, NameFault};
use super::types::{TablePlan, Target};
use crate::commands::{
    destructive_mode_of, guard_confirmed, guard_destructive_mode, record_decision,
};
use crate::env::TransferMode;
use crate::CliError;

/// A statement the engine itself sends, as text it owns.
///
/// Each variant reads as exactly one statement under its dialect (a test holds that), and the
/// four source-side `SET`s are condition C-3: the fixed text settings under which the source
/// PostgreSQL prints every value the transfer forwards as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineStatement {
    Begin,
    Commit,
    Rollback,
    /// MySQL: strict mode added to whatever the session has, with no stray comma when
    /// `sql_mode` is empty (condition C-5).
    StrictMode,
    Utf8mb4,
    /// MySQL: how large a packet the server takes.
    MaxPacket,
    /// PostgreSQL source (C-3).
    ExtraFloatDigits,
    DateStyle,
    IntervalStyle,
    ByteaOutput,
    /// MySQL: does a table exist? Bound to the database and the table name.
    ProbeMysql,
}

impl EngineStatement {
    pub const ALL: [EngineStatement; 11] = [
        EngineStatement::Begin,
        EngineStatement::Commit,
        EngineStatement::Rollback,
        EngineStatement::StrictMode,
        EngineStatement::Utf8mb4,
        EngineStatement::MaxPacket,
        EngineStatement::ExtraFloatDigits,
        EngineStatement::DateStyle,
        EngineStatement::IntervalStyle,
        EngineStatement::ByteaOutput,
        EngineStatement::ProbeMysql,
    ];

    /// The four statements a PostgreSQL source runs before its query (C-3).
    pub const SOURCE_READING: [EngineStatement; 4] = [
        EngineStatement::ExtraFloatDigits,
        EngineStatement::DateStyle,
        EngineStatement::IntervalStyle,
        EngineStatement::ByteaOutput,
    ];

    pub const fn text(self) -> &'static str {
        match self {
            EngineStatement::Begin => "BEGIN",
            EngineStatement::Commit => "COMMIT",
            EngineStatement::Rollback => "ROLLBACK",
            EngineStatement::StrictMode => {
                "SET SESSION sql_mode = CONCAT_WS(',', NULLIF(@@sql_mode, ''), 'STRICT_ALL_TABLES')"
            }
            EngineStatement::Utf8mb4 => "SET NAMES utf8mb4",
            EngineStatement::MaxPacket => "SELECT @@max_allowed_packet",
            EngineStatement::ExtraFloatDigits => "SET extra_float_digits = 3",
            EngineStatement::DateStyle => "SET DateStyle = ISO",
            EngineStatement::IntervalStyle => "SET IntervalStyle = postgres",
            EngineStatement::ByteaOutput => "SET bytea_output = hex",
            EngineStatement::ProbeMysql => {
                "SELECT COUNT(*) FROM information_schema.tables \
                 WHERE table_schema = ? AND table_name = ?"
            }
        }
    }

    /// The dialect it is read under, which is the dialect of the server that gets it.
    pub const fn dialect(self) -> Dialect {
        match self {
            EngineStatement::Begin
            | EngineStatement::Commit
            | EngineStatement::Rollback
            | EngineStatement::ExtraFloatDigits
            | EngineStatement::DateStyle
            | EngineStatement::IntervalStyle
            | EngineStatement::ByteaOutput => Dialect::Postgres,
            EngineStatement::StrictMode
            | EngineStatement::Utf8mb4
            | EngineStatement::MaxPacket
            | EngineStatement::ProbeMysql => Dialect::Mysql,
        }
    }

    /// What the classifier reads it as: the reason these are not classified in the run.
    pub const fn kind(self) -> StatementKind {
        match self {
            EngineStatement::MaxPacket | EngineStatement::ProbeMysql => StatementKind::ReadOnly,
            _ => StatementKind::Unknown,
        }
    }
}

/// A statement the transfer writes, with the kind the plan says it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub text: String,
    pub kind: StatementKind,
}

impl Planned {
    fn ddl(text: String) -> Self {
        Planned {
            text,
            kind: StatementKind::Ddl,
        }
    }

    fn dml(text: String) -> Self {
        Planned {
            text,
            kind: StatementKind::Dml,
        }
    }
}

/// `text` as a quoted string literal, for the one place a name goes into a PostgreSQL read.
///
/// Quotes are doubled; a text with a backslash is an `E''` string with the backslash doubled,
/// which reads the same whatever `standard_conforming_strings` says.
fn literal(text: &str) -> String {
    if text.contains('\\') {
        format!("E'{}'", text.replace('\\', "\\\\").replace('\'', "''"))
    } else {
        format!("'{}'", text.replace('\'', "''"))
    }
}

/// The statements one transfer sends to its destination.
///
/// Built from the checked parts of the name, the planned table and the mode, so every name in
/// it went through [`names::ident`] and every type through [`TablePlan`].
#[derive(Debug, Clone)]
pub struct WritePlan {
    pub target: Target,
    pub mode: TransferMode,
    /// Where the rows are written: the table itself for a PostgreSQL create, the scratch table
    /// for everything else.
    pub write_into: String,
    /// The final table, as SQL.
    pub table: String,
    /// The final table as a person reads it (`schema.table`), for the `done` event.
    pub label: String,
    /// The scratch table, as SQL, when there is one.
    pub scratch: Option<String>,
    /// The scratch table as a person reads it, for a warning.
    pub scratch_label: Option<String>,
    columns: String,
    column_count: usize,
    /// `CREATE TABLE`, of the table or of the scratch.
    pub create: Planned,
    /// The `COPY` statement (PostgreSQL).
    pub copy: Option<Planned>,
    /// After the rows are verified, before `COMMIT` (PostgreSQL) or on their own (MySQL).
    pub swap: Vec<Planned>,
    /// MySQL replace: the old table, once the new one is in place.
    pub drop_old: Option<Planned>,
    /// MySQL: removes the scratch table when the run does not get as far as the swap.
    pub cleanup: Option<Planned>,
}

/// What the existence probe is sent as: a read the plan owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeCall {
    /// `SELECT to_regclass('<the quoted name>')`: the PostgreSQL driver binds only statements
    /// that return no rows, so the one name goes in as a literal, escaped, from a name that
    /// was checked. It returns one cell, NULL when there is no such relation.
    Text(Planned),
    /// [`EngineStatement::ProbeMysql`] with these two values bound.
    Bound(EngineStatement, [String; 2]),
}

/// The probe for `[namespace, table]` on `target`: is there already a relation of that name?
///
/// Asked before the source query runs (N-5), so a create over an existing table fails before
/// the expensive part. `to_regclass` also sees a view, which a replace then refuses at its
/// `DROP TABLE` and rolls back.
pub fn probe(target: Target, parts: [&str; 2]) -> Result<ProbeCall, NameFault> {
    let [namespace, table] = parts;
    match target {
        Target::Postgres => {
            let qualified = names::qualified(target, &[namespace, table])?;
            Ok(ProbeCall::Text(Planned {
                text: format!("SELECT to_regclass({})", literal(&qualified)),
                kind: StatementKind::ReadOnly,
            }))
        }
        Target::Mysql => {
            names::qualified(target, &[namespace, table])?;
            Ok(ProbeCall::Bound(
                EngineStatement::ProbeMysql,
                [namespace.to_owned(), table.to_owned()],
            ))
        }
    }
}

impl WritePlan {
    /// Build the plan.
    ///
    /// `parts` is `[schema, table]` for PostgreSQL and `[database, table]` for MySQL;
    /// `existed` is what the probe found (only a MySQL replace reads it); `stamp` makes the
    /// scratch and backup names (`qh_new_<stamp>`, `qh_old_<stamp>`).
    pub fn build(
        table_plan: &TablePlan,
        mode: TransferMode,
        parts: [&str; 2],
        existed: bool,
        stamp: &str,
    ) -> Result<Self, NameFault> {
        let target = table_plan.target;
        let [namespace, name] = parts;
        let table = names::qualified(target, &[namespace, name])?;
        let label = format!("{namespace}.{name}");
        let scratch_name = format!("qh_new_{stamp}");
        let backup_name = format!("qh_old_{stamp}");
        let scratch = names::qualified(target, &[namespace, &scratch_name])?;
        let backup = names::qualified(target, &[namespace, &backup_name])?;
        let columns = table_plan
            .columns
            .iter()
            .map(|column| names::ident(target, &column.name))
            .collect::<Vec<_>>()
            .join(", ");
        let column_count = table_plan.columns.len();

        let direct = target == Target::Postgres && mode == TransferMode::Create;
        let write_into = if direct {
            table.clone()
        } else {
            scratch.clone()
        };
        let create = Planned::ddl(table_plan.create_table_sql(&write_into));

        let mut plan = WritePlan {
            target,
            mode,
            write_into: write_into.clone(),
            table: table.clone(),
            label,
            scratch: (!direct).then(|| scratch.clone()),
            scratch_label: (!direct).then(|| format!("{namespace}.{scratch_name}")),
            columns,
            column_count,
            create,
            copy: None,
            swap: Vec::new(),
            drop_old: None,
            cleanup: None,
        };
        match target {
            Target::Postgres => {
                plan.copy = Some(Planned::dml(format!(
                    "COPY {write_into} ({}) FROM STDIN",
                    plan.columns
                )));
                if mode == TransferMode::Replace {
                    plan.swap = vec![
                        Planned::ddl(format!("DROP TABLE IF EXISTS {table}")),
                        Planned::ddl(format!(
                            "ALTER TABLE {scratch} RENAME TO {}",
                            names::ident(target, name)
                        )),
                    ];
                }
            }
            Target::Mysql => {
                plan.cleanup = Some(Planned::ddl(format!("DROP TABLE IF EXISTS {scratch}")));
                if mode == TransferMode::Replace && existed {
                    plan.swap = vec![Planned::ddl(format!(
                        "RENAME TABLE {table} TO {backup}, {scratch} TO {table}"
                    ))];
                    plan.drop_old = Some(Planned::ddl(format!("DROP TABLE {backup}")));
                } else {
                    plan.swap = vec![Planned::ddl(format!("RENAME TABLE {scratch} TO {table}"))];
                }
            }
        }
        Ok(plan)
    }

    /// The backup table of a MySQL replace, as a person reads it.
    pub fn backup_label(&self, stamp: &str) -> String {
        let namespace = self.label.rsplit_once('.').map_or("", |(left, _)| left);
        format!("{namespace}.qh_old_{stamp}")
    }

    /// One `INSERT` of `rows` rows (MySQL), with a `?` for every value.
    pub fn insert(&self, rows: usize) -> Planned {
        let row = format!("({})", vec!["?"; self.column_count].join(", "));
        let values = vec![row; rows].join(", ");
        Planned::dml(format!(
            "INSERT INTO {} ({}) VALUES {values}",
            self.write_into, self.columns
        ))
    }

    /// Every distinct fixed statement the plan can send to the destination, for the decision
    /// taken before the first row. The `INSERT` is one row wide here; a different width is
    /// decided when it is first built.
    pub fn statements(&self) -> Vec<Planned> {
        let mut all = vec![self.create.clone()];
        all.extend(self.copy.clone());
        if self.target == Target::Mysql {
            all.push(self.insert(1));
        }
        all.extend(self.swap.iter().cloned());
        all.extend(self.drop_old.clone());
        all.extend(self.cleanup.clone());
        all
    }
}

/// Read `text` under `dialect` and check it is the one statement of the kind the plan names.
///
/// The check that stands between a name chosen by a server and a statement sent to another:
/// a name that closed its own quotes and carried a second statement reads as two, and one that
/// changed what the statement does reads as another kind.
pub fn check_structure(planned: &Planned, dialect: Dialect) -> Result<(), CliError> {
    let count = statements_dialect(&planned.text, dialect).len();
    let kind = classify_readings(&planned.text, dialect.readings());
    if count == 1 && kind == planned.kind {
        return Ok(());
    }
    // Never the text: it holds names from a server.
    Err(CliError::Internal(format!(
        "a statement of the copy to another connection read as {count} statement(s) of kind \
         {kind}, and the plan says one of kind {}; nothing was sent",
        planned.kind
    )))
}

/// Decides the written statements of one transfer against its destination's floor.
pub struct Decider {
    ddl_mode: SafeMode,
    dml_mode: SafeMode,
    confirmed: bool,
    dialect: Dialect,
    decided: HashSet<String>,
}

impl Decider {
    /// `floor` is the destination's, with both of its floors raised (`DEST_SAFE_MODE_FLOOR`
    /// and the top-level `SAFE_MODE_FLOOR`). The mode for a DML statement is the floor's
    /// **resolved** strictness, never `safe_mode()` of the destination's settings alone, which
    /// would leave the top-level floor out (C-4).
    pub fn new(floor: &SafeModeFloor, confirmed: bool, dialect: Dialect) -> Self {
        Self {
            ddl_mode: destructive_mode_of(floor),
            dml_mode: floor
                .resolve()
                .map_or(SafeMode::Full, |(mode, _source)| mode),
            confirmed,
            dialect,
            decided: HashSet::new(),
        }
    }

    /// Allow `planned` or refuse it, once per distinct text. The structure is checked every
    /// time, the decision is taken and logged the first.
    pub fn allow(&mut self, planned: &Planned) -> Result<(), CliError> {
        check_structure(planned, self.dialect)?;
        if self.decided.contains(&planned.text) {
            return Ok(());
        }
        match planned.kind {
            StatementKind::Ddl => {
                guard_destructive_mode(self.ddl_mode, self.confirmed, self.dialect, &planned.text)?
            }
            StatementKind::Dml => {
                guard_confirmed(self.dml_mode, self.confirmed, &planned.text, self.dialect)?
            }
            // The plan has no written statement of another kind; a future one must be decided
            // on purpose rather than slip through here.
            other => {
                return Err(CliError::Internal(format!(
                    "a written statement of the copy has the kind {other}, which has no decision"
                )))
            }
        }
        self.decided.insert(planned.text.clone());
        Ok(())
    }
}

/// Why a replace is never asked about, only refused, when the destination asks before it
/// writes: it deletes a table the user has.
const REPLACE_ON_CONFIRM: &str =
    "a replace deletes the existing table, so a destination that asks before it writes does not \
     offer it";

/// The decision that needs no network: can this plan be written to a destination under this
/// floor at all? A refusal is logged with the plan's subject and returned as a usage error,
/// before a connection is opened. An allowance is not logged here (the statements themselves
/// are, when they are decided).
///
/// | destination | create | replace |
/// |---|---|---|
/// | `full` | yes | yes |
/// | asks (`confirm`) | yes, once confirmed | no |
/// | `no_ddl`, `read_only`, or any entry that forbids DDL | no | no |
pub fn gate(
    floor: &SafeModeFloor,
    confirmed: bool,
    dialect: Dialect,
    mode: TransferMode,
    subject: &str,
) -> Result<(), CliError> {
    let ddl_mode = destructive_mode_of(floor);
    if mode == TransferMode::Replace && ddl_mode == SafeMode::Confirm {
        let refused = record_decision(
            ddl_mode,
            confirmed,
            1,
            StatementKind::Ddl,
            subject,
            Decision::Refuse,
            Some(REPLACE_ON_CONFIRM),
        )?;
        let _ = refused;
        return Err(CliError::Usage(format!(
            "SAFE_MODE={ddl_mode} refuses the replace of the destination table: {REPLACE_ON_CONFIRM}"
        )));
    }
    match ddl_mode {
        SafeMode::Full => Ok(()),
        SafeMode::Confirm if confirmed => Ok(()),
        // The refusal, or the question, in the words `table_op` uses, and in the log.
        _ => guard_destructive_mode(ddl_mode, confirmed, dialect, subject),
    }
}

/// `SELECT @@max_allowed_packet` as the number of bytes it says, or `None` for a cell that is
/// not one.
pub fn parse_max_allowed_packet(value: &qh_core::Value) -> Option<u64> {
    match value {
        qh_core::Value::Int(number) => u64::try_from(*number).ok(),
        qh_core::Value::UInt(number) => Some(*number),
        qh_core::Value::Text(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// The MySQL packet size to plan inserts under: half of what the server takes, at most
/// 16 MiB, and a megabyte when the server's answer could not be read.
pub fn insert_budget(max_allowed_packet: Option<u64>) -> u64 {
    const CEILING: u64 = 16 * 1024 * 1024;
    const FALLBACK: u64 = 1024 * 1024;
    match max_allowed_packet {
        Some(packet) => (packet / 2).clamp(1024, CEILING),
        None => FALLBACK,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::types::{plan_table, Base, SqlType};
    use qh_driver::DriverKind;
    use qh_sql::FloorSource;

    fn floor(entries: &[(FloorSource, SafeMode)]) -> SafeModeFloor {
        let mut floor = SafeModeFloor::new();
        for (source, mode) in entries {
            floor.raise(*source, *mode);
        }
        floor
    }

    fn table_plan(target: Target, columns: &[&str]) -> TablePlan {
        let int = SqlType::of(Base::Int64);
        let columns: Vec<(&str, &SqlType)> = columns.iter().map(|name| (*name, &int)).collect();
        plan_table(DriverKind::Postgres, target, &columns).expect("a plan")
    }

    #[test]
    fn every_engine_statement_reads_as_one_statement_of_its_kind_under_its_dialect() {
        for statement in EngineStatement::ALL {
            let text = statement.text();
            let dialect = statement.dialect();
            assert_eq!(statements_dialect(text, dialect).len(), 1, "{text}");
            assert_eq!(
                classify_readings(text, dialect.readings()),
                statement.kind(),
                "{text}"
            );
            // Owned text: no placeholder but the two bound ones, no name of any server.
            assert!(!text.contains('{'), "{text}");
        }
        assert_eq!(EngineStatement::ALL.len(), 11);
    }

    #[test]
    fn the_strict_mode_statement_has_no_stray_comma_for_an_empty_sql_mode() {
        // C-5: CONCAT_WS skips a NULL, and NULLIF turns the empty mode into NULL.
        let text = EngineStatement::StrictMode.text();
        assert!(text.contains("CONCAT_WS(','"), "{text}");
        assert!(text.contains("NULLIF(@@sql_mode, '')"), "{text}");
        assert!(!text.contains("CONCAT(@@sql_mode"), "{text}");
    }

    #[test]
    fn the_source_reading_settings_are_the_four_of_condition_c3() {
        let texts: Vec<&str> = EngineStatement::SOURCE_READING
            .iter()
            .map(|statement| statement.text())
            .collect();
        assert_eq!(
            texts,
            [
                "SET extra_float_digits = 3",
                "SET DateStyle = ISO",
                "SET IntervalStyle = postgres",
                "SET bytea_output = hex"
            ]
        );
    }

    #[test]
    fn a_postgres_create_writes_straight_into_the_table_and_a_replace_into_a_scratch() {
        let columns = table_plan(Target::Postgres, &["id", "Order"]);
        let create = WritePlan::build(
            &columns,
            TransferMode::Create,
            ["public", "People"],
            false,
            "abc",
        )
        .unwrap();
        assert_eq!(create.write_into, "public.\"People\"");
        assert!(create.scratch.is_none());
        assert_eq!(
            create.create.text,
            "CREATE TABLE public.\"People\" (id bigint, \"Order\" bigint)"
        );
        assert_eq!(
            create.copy.as_ref().unwrap().text,
            "COPY public.\"People\" (id, \"Order\") FROM STDIN"
        );
        assert!(create.swap.is_empty());

        let replace = WritePlan::build(
            &columns,
            TransferMode::Replace,
            ["public", "People"],
            true,
            "abc",
        )
        .unwrap();
        assert_eq!(replace.write_into, "public.qh_new_abc");
        assert_eq!(
            replace
                .swap
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>(),
            [
                "DROP TABLE IF EXISTS public.\"People\"",
                "ALTER TABLE public.qh_new_abc RENAME TO \"People\""
            ]
        );
    }

    #[test]
    fn a_mysql_transfer_builds_a_scratch_and_renames_it_in() {
        let columns = table_plan(Target::Mysql, &["id", "v"]);
        let create = WritePlan::build(
            &columns,
            TransferMode::Create,
            ["shop", "people"],
            false,
            "abc",
        )
        .unwrap();
        assert_eq!(
            create.create.text,
            "CREATE TABLE shop.qh_new_abc (id bigint, v bigint) DEFAULT CHARSET=utf8mb4"
        );
        assert_eq!(
            create.insert(2).text,
            "INSERT INTO shop.qh_new_abc (id, v) VALUES (?, ?), (?, ?)"
        );
        assert_eq!(
            create.swap[0].text,
            "RENAME TABLE shop.qh_new_abc TO shop.people"
        );
        assert_eq!(
            create.cleanup.as_ref().unwrap().text,
            "DROP TABLE IF EXISTS shop.qh_new_abc"
        );
        assert!(create.drop_old.is_none());

        let replace = WritePlan::build(
            &columns,
            TransferMode::Replace,
            ["shop", "people"],
            true,
            "abc",
        )
        .unwrap();
        assert_eq!(
            replace.swap[0].text,
            "RENAME TABLE shop.people TO shop.qh_old_abc, shop.qh_new_abc TO shop.people"
        );
        assert_eq!(
            replace.drop_old.as_ref().unwrap().text,
            "DROP TABLE shop.qh_old_abc"
        );
        // With no old table to move aside it is the install, and nothing is dropped after.
        let fresh = WritePlan::build(
            &columns,
            TransferMode::Replace,
            ["shop", "people"],
            false,
            "abc",
        )
        .unwrap();
        assert_eq!(
            fresh.swap[0].text,
            "RENAME TABLE shop.qh_new_abc TO shop.people"
        );
        assert!(fresh.drop_old.is_none());
    }

    #[test]
    fn every_written_statement_passes_the_structure_check_even_with_hostile_names() {
        let hostile = [
            "a\"; DROP TABLE x; --",
            "b`; DROP TABLE x; --",
            "c'; DELETE FROM y; --",
            "d\\",
            "e\nf",
            "select",
            "g;h",
        ];
        for target in Target::ALL {
            let dialect = match target {
                Target::Postgres => Dialect::Postgres,
                Target::Mysql => Dialect::Mysql,
            };
            let columns = table_plan(target, &hostile);
            for mode in [TransferMode::Create, TransferMode::Replace] {
                for existed in [false, true] {
                    let plan = WritePlan::build(
                        &columns,
                        mode,
                        ["sch\"ema", "ta`ble; DROP TABLE z"],
                        existed,
                        "abc",
                    );
                    // A name MySQL refuses (a quote in a database name is fine) must refuse here,
                    // not build; the ones that build must read as one statement of their kind.
                    let plan = plan.expect("these names are quotable");
                    let mut statements = plan.statements();
                    statements.push(plan.insert(3));
                    for planned in &statements {
                        check_structure(planned, dialect)
                            .unwrap_or_else(|error| panic!("{} -> {error}", planned.text));
                    }
                    if let ProbeCall::Text(read) =
                        probe(target, ["sch\"ema", "ta`ble; DROP TABLE z"]).unwrap()
                    {
                        check_structure(&read, dialect).expect("the probe reads as a read");
                    }
                }
            }
        }
    }

    #[test]
    fn a_statement_that_reads_as_two_or_as_another_kind_is_an_internal_error_with_no_text() {
        let two = Planned::ddl("CREATE TABLE a (x int); DROP TABLE b".to_owned());
        let error = check_structure(&two, Dialect::Postgres).unwrap_err();
        assert!(matches!(error, CliError::Internal(_)), "{error}");
        assert!(!error.to_string().contains("DROP TABLE b"), "{error}");
        let wrong_kind = Planned::dml("DROP TABLE a".to_owned());
        assert!(check_structure(&wrong_kind, Dialect::Mysql).is_err());
        // A forged second statement behind a comment is still two for the dialect.
        let comment = Planned::ddl("CREATE TABLE a (x int) -- \n; DROP TABLE b".to_owned());
        assert!(check_structure(&comment, Dialect::Mysql).is_err());
    }

    #[test]
    fn the_probe_names_the_table_as_a_literal_or_as_bound_values() {
        let ProbeCall::Text(read) =
            probe(Target::Postgres, ["public", "it's \"a\" table\\"]).unwrap()
        else {
            panic!("a PostgreSQL probe is text");
        };
        assert_eq!(
            read.text,
            "SELECT to_regclass(E'public.\"it''s \"\"a\"\" table\\\\\"')"
        );
        assert_eq!(read.kind, StatementKind::ReadOnly);
        assert_eq!(
            probe(Target::Mysql, ["shop", "t"]).unwrap(),
            ProbeCall::Bound(
                EngineStatement::ProbeMysql,
                ["shop".to_owned(), "t".to_owned()]
            )
        );
    }

    #[test]
    fn the_gate_matches_the_table_of_the_decision_record() {
        use FloorSource::{Connection, Policy, User};
        let pg = Dialect::Postgres;
        let decide = |entries: &[(FloorSource, SafeMode)], confirmed, mode| {
            gate(
                &floor(entries),
                confirmed,
                pg,
                mode,
                "CREATE TABLE t (a int)",
            )
        };
        let create = TransferMode::Create;
        let replace = TransferMode::Replace;
        // full
        assert!(decide(&[(User, SafeMode::Full)], false, create).is_ok());
        assert!(decide(&[(User, SafeMode::Full)], false, replace).is_ok());
        // no_ddl and read_only refuse both
        for mode in [SafeMode::NoDdl, SafeMode::ReadOnly] {
            assert!(decide(&[(User, mode)], true, create).is_err(), "{mode}");
            assert!(decide(&[(User, mode)], true, replace).is_err(), "{mode}");
        }
        // confirm asks for create, once, and never offers replace
        assert!(decide(&[(User, SafeMode::Confirm)], false, create).is_err());
        assert!(decide(&[(User, SafeMode::Confirm)], true, create).is_ok());
        assert!(decide(&[(User, SafeMode::Confirm)], true, replace).is_err());
        // a floor entry that forbids DDL wins over an entry that asks, in either order
        for entries in [
            [(User, SafeMode::Confirm), (Policy, SafeMode::NoDdl)],
            [(Policy, SafeMode::NoDdl), (User, SafeMode::Confirm)],
        ] {
            assert!(decide(&entries, true, create).is_err(), "{entries:?}");
        }
        // a read-only connection mark refuses whatever the user chose
        assert!(decide(
            &[(User, SafeMode::Full), (Connection, SafeMode::ReadOnly)],
            true,
            create
        )
        .is_err());
        let refusal = decide(&[(User, SafeMode::Confirm)], true, replace).unwrap_err();
        assert!(matches!(refusal, CliError::Usage(_)), "{refusal}");
        assert!(refusal.to_string().contains("replace"), "{refusal}");
    }

    #[test]
    fn the_decider_decides_each_text_once_and_with_the_floors_two_entries() {
        use FloorSource::{Policy, User};
        let columns = table_plan(Target::Postgres, &["id"]);
        let plan = WritePlan::build(
            &columns,
            TransferMode::Create,
            ["public", "t"],
            false,
            "abc",
        )
        .unwrap();
        let copy = plan.copy.clone().unwrap();

        // full: everything passes.
        let mut decider = Decider::new(&floor(&[(User, SafeMode::Full)]), false, Dialect::Postgres);
        for planned in plan.statements() {
            decider.allow(&planned).expect(&planned.text);
        }
        // confirm, unconfirmed: the DML asks (and the DDL does).
        let mut decider = Decider::new(
            &floor(&[(User, SafeMode::Confirm)]),
            false,
            Dialect::Postgres,
        );
        assert!(decider.allow(&plan.create).is_err());
        assert!(decider.allow(&copy).is_err());
        // confirm, confirmed: both pass. The top-level policy floor is an entry like the other.
        let mut decider = Decider::new(
            &floor(&[(User, SafeMode::Confirm)]),
            true,
            Dialect::Postgres,
        );
        assert!(decider.allow(&plan.create).is_ok());
        assert!(decider.allow(&copy).is_ok());
        let mut decider = Decider::new(
            &floor(&[(User, SafeMode::Full), (Policy, SafeMode::Confirm)]),
            false,
            Dialect::Postgres,
        );
        assert!(
            decider.allow(&copy).is_err(),
            "the policy floor asks for a write too"
        );
        // A statement of another kind than the plan says is not decided at all.
        let forged = Planned::dml(plan.create.text.clone());
        assert!(matches!(decider.allow(&forged), Err(CliError::Internal(_))));
    }

    #[test]
    fn the_packet_budget_is_half_the_servers_at_most_sixteen_mebibytes() {
        use qh_core::Value;
        assert_eq!(
            parse_max_allowed_packet(&Value::Int(67_108_864)),
            Some(67_108_864)
        );
        assert_eq!(
            parse_max_allowed_packet(&Value::UInt(4_194_304)),
            Some(4_194_304)
        );
        assert_eq!(
            parse_max_allowed_packet(&Value::Text(" 1048576 ".into())),
            Some(1_048_576)
        );
        assert_eq!(parse_max_allowed_packet(&Value::Int(-1)), None);
        assert_eq!(parse_max_allowed_packet(&Value::Null), None);
        assert_eq!(insert_budget(Some(67_108_864)), 16 * 1024 * 1024);
        assert_eq!(insert_budget(Some(4_194_304)), 2_097_152);
        assert_eq!(insert_budget(Some(100)), 1024);
        assert_eq!(insert_budget(None), 1024 * 1024);
    }
}
