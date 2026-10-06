//! Read-only metadata: what a driver's own SQL answers about objects, columns and DDL.
//!
//! A driver owns the statements and the reading of their answers, and nothing else. It does
//! not run them: the engine sends each statement through [`crate::Session::execute`], so the
//! pool's session wrapper, the retry layer and the Safe Mode tests see every one of them like
//! any other statement, and a new method on `Session` that a wrapper forgot to forward cannot
//! silently answer "unsupported" (blueprint w11 D-1).
//!
//! Everything here is plain data and pure functions, so a driver's SQL can be pinned by a test
//! without a server. All cells travel as text: the SQL itself turns booleans into `'YES'` and
//! `'NO'` and numbers into text, so reading an answer never depends on how a driver decodes a
//! type.

use qh_core::EngineError;

use crate::ObjectPath;

/// What kind of object a name in the tree is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Table,
    View,
    MaterializedView,
    ForeignTable,
}

impl ObjectKind {
    /// The word the event protocol uses.
    pub const fn token(self) -> &'static str {
        match self {
            ObjectKind::Table => "table",
            ObjectKind::View => "view",
            ObjectKind::MaterializedView => "materialized_view",
            ObjectKind::ForeignTable => "foreign_table",
        }
    }

    /// The kind a token names, or `None` for a word nobody offers.
    pub fn parse(token: &str) -> Option<Self> {
        Some(match token {
            "table" => ObjectKind::Table,
            "view" => ObjectKind::View,
            "materialized_view" => ObjectKind::MaterializedView,
            "foreign_table" => ObjectKind::ForeignTable,
            _ => return None,
        })
    }
}

/// One name of a listing, with its kind when the server said one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableEntry {
    pub name: String,
    pub kind: Option<ObjectKind>,
}

/// One column of an object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    /// `None` when the server did not say.
    pub nullable: Option<bool>,
    /// The default expression as the server prints it, `None` for no default.
    pub default: Option<String>,
    /// `identity`, `generated`, `auto_increment`, or a server's own words; empty for none. Kept
    /// apart from `default` because a generated column's expression is not a default.
    pub extra: String,
}

/// The DDL of one object, as far as the driver could write it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectDdl {
    pub kind: Option<ObjectKind>,
    pub text: String,
    /// Set by whoever ran the statements when a row or step cap cut the answer short.
    pub truncated: bool,
}

/// The answer to one statement: every cell as text, `None` for a SQL NULL.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

impl Rows {
    /// The cell at `row`, `column`, or `None` for a NULL or a cell that is not there.
    pub fn cell(&self, row: usize, column: usize) -> Option<&str> {
        self.rows.get(row)?.get(column)?.as_deref()
    }

    /// A cell that has to be there: a missing or NULL cell is a statement answering in a
    /// shape this driver did not write it for, which is this program's defect to report.
    pub fn required(&self, row: usize, column: usize) -> Result<&str, EngineError> {
        self.cell(row, column).ok_or_else(|| EngineError::Internal {
            message: format!("a metadata answer has no value at row {row}, column {column}"),
        })
    }
}

/// One step of a DDL recipe: another statement to run, or the finished text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Query(String),
    Done(ObjectDdl),
}

/// A DDL written by one or more statements.
///
/// A state machine rather than an async function, so the driver stays free of I/O and a test can
/// feed it canned rows: the engine runs each statement the recipe asks for and hands the rows back.
pub trait DdlRecipe: Send {
    /// The first call passes `None`; every later call passes the rows of the statement the last
    /// call asked for.
    fn next(&mut self, previous: Option<Rows>) -> Result<Step, EngineError>;

    /// The statement the last call asked for failed. A recipe that has a second way to ask returns
    /// the step to take instead; the default has none, and the failure is the answer.
    fn recover(&mut self, _error: &EngineError) -> Option<Step> {
        None
    }
}

/// The metadata statements of one driver.
///
/// `path` is read the way the driver's own `browse` reads it (Trino: catalog and schema;
/// PostgreSQL: schema; MySQL: its database in `schema`), and `target` the same way with the table
/// added. Every name goes into SQL through the dialect's own quoting, never by concatenation.
pub trait MetadataSql: Send + Sync {
    /// The statement that lists the objects of one level with their kind.
    fn table_entries(&self, path: &ObjectPath) -> Result<String, EngineError>;

    fn parse_table_entries(&self, rows: &Rows) -> Result<Vec<TableEntry>, EngineError>;

    /// The statement that lists the columns of `target`.
    fn columns(&self, target: &ObjectPath) -> Result<String, EngineError>;

    fn parse_columns(&self, rows: &Rows) -> Result<Vec<ColumnInfo>, EngineError>;

    /// The statements that write the DDL of `target`.
    fn ddl(&self, target: &ObjectPath) -> Result<Box<dyn DdlRecipe>, EngineError>;
}

/// A part of a name that must be there, refused by naming it.
pub fn required_part<'a>(
    part: &'a Option<String>,
    what: &str,
    context: &str,
) -> Result<&'a str, EngineError> {
    part.as_deref()
        .filter(|text| !text.is_empty())
        .ok_or_else(|| EngineError::Usage {
            message: format!("{what} is required {context}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_round_trips_through_its_token() {
        for kind in [
            ObjectKind::Table,
            ObjectKind::View,
            ObjectKind::MaterializedView,
            ObjectKind::ForeignTable,
        ] {
            assert_eq!(ObjectKind::parse(kind.token()), Some(kind));
        }
        assert_eq!(ObjectKind::parse("sequence"), None);
    }

    #[test]
    fn a_missing_or_null_cell_is_not_a_value() {
        let rows = Rows {
            columns: vec!["a".to_owned()],
            rows: vec![vec![Some("x".to_owned())], vec![None]],
        };
        assert_eq!(rows.cell(0, 0), Some("x"));
        assert_eq!(rows.cell(1, 0), None);
        assert_eq!(rows.cell(2, 0), None);
        assert_eq!(rows.cell(0, 1), None);
        assert!(rows.required(1, 0).is_err());
    }
}
