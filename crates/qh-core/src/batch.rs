//! A batch of rows, held column by column.
//!
//! Column-major rather than row-major because that is the shape the two
//! consumers need: the grid asks for a window of rows and a handful of columns,
//! and an exporter walks one column at a time. A row-major `Vec<Vec<Value>>`
//! makes both of those pointer chases through a value per cell.
//!
//! The invariant is enforced at construction and nowhere else: every column has
//! exactly `rows` values. A malformed batch is a bug in a driver, so it is
//! refused with an error at the boundary rather than tolerated downstream.

use thiserror::Error;

use crate::value::Value;

/// Why a batch could not be built.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BatchError {
    /// No columns at all. A result set always has at least one column, so this
    /// means the caller built the batch wrongly rather than that the query
    /// returned nothing — a query returning no rows still has columns.
    #[error("a batch needs at least one column")]
    NoColumns,

    /// Columns of different lengths. Reported with the two lengths, because
    /// which column is short is the thing that points at the bug.
    #[error("column {index} has {found} values but the first column has {expected}")]
    RaggedColumn {
        index: usize,
        expected: usize,
        found: usize,
    },
}

/// One column's identity, as the server described it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnMeta {
    /// The name the server gave, verbatim. Not normalised to lower case: a
    /// query can return `SELECT 1 AS "Total"` and `... AS "total"` in the same
    /// result, and they are different columns.
    pub name: Box<str>,
    /// The server's own type text, shown as a type chip in the grid. Kept as
    /// reported rather than mapped, because the chip is a hint to the user and
    /// `numeric(38,10)` says more than `Decimal` does.
    pub type_name: Box<str>,
}

impl ColumnMeta {
    pub fn new(name: impl Into<Box<str>>, type_name: impl Into<Box<str>>) -> Self {
        Self {
            name: name.into(),
            type_name: type_name.into(),
        }
    }
}

/// A rectangular block of values, column by column.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnBatch {
    columns: Vec<Vec<Value>>,
}

impl ColumnBatch {
    /// Build a batch, refusing a shape that cannot be a result set.
    pub fn new(columns: Vec<Vec<Value>>) -> Result<Self, BatchError> {
        let Some(first) = columns.first() else {
            return Err(BatchError::NoColumns);
        };
        let expected = first.len();
        for (index, column) in columns.iter().enumerate() {
            if column.len() != expected {
                return Err(BatchError::RaggedColumn {
                    index,
                    expected,
                    found: column.len(),
                });
            }
        }
        Ok(Self { columns })
    }

    /// A batch with no rows: `width` empty columns.
    ///
    /// This is how a cursor reports "here are the columns, there is nothing
    /// yet", which is a distinct state from "finished" — the grid draws headers
    /// for the first and nothing for the second.
    pub fn empty(width: usize) -> Result<Self, BatchError> {
        if width == 0 {
            return Err(BatchError::NoColumns);
        }
        Ok(Self {
            columns: vec![Vec::new(); width],
        })
    }

    /// How many rows every column holds.
    pub fn rows(&self) -> usize {
        self.columns.first().map_or(0, Vec::len)
    }

    pub fn width(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows() == 0
    }

    /// The columns, in the server's order.
    pub fn columns(&self) -> &[Vec<Value>] {
        &self.columns
    }

    /// One cell, addressed as the store addresses it.
    pub fn value(&self, row: usize, column: usize) -> Option<&Value> {
        self.columns.get(column)?.get(row)
    }

    /// Take the storage out, consuming the batch. Used to hand the columns to
    /// the encoder without copying every value.
    pub fn into_columns(self) -> Vec<Vec<Value>> {
        self.columns
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(values: &[i64]) -> Vec<Value> {
        values.iter().copied().map(Value::Int).collect()
    }

    #[test]
    fn a_ragged_batch_is_refused_and_names_the_column() {
        let error = ColumnBatch::new(vec![column(&[1, 2, 3]), column(&[1])]).unwrap_err();
        assert_eq!(
            error,
            BatchError::RaggedColumn {
                index: 1,
                expected: 3,
                found: 1
            }
        );
        // The message has to say which column, or the driver author has to guess.
        assert!(error.to_string().contains("column 1"), "{error}");
    }

    #[test]
    fn a_batch_with_no_columns_is_refused() {
        assert_eq!(ColumnBatch::new(vec![]).unwrap_err(), BatchError::NoColumns);
        assert_eq!(ColumnBatch::empty(0).unwrap_err(), BatchError::NoColumns);
    }

    #[test]
    fn a_batch_with_columns_and_no_rows_is_allowed() {
        // Distinct from "finished": a cursor sends this with its column list.
        let batch = ColumnBatch::empty(3).unwrap();
        assert_eq!(batch.width(), 3);
        assert_eq!(batch.rows(), 0);
        assert!(batch.is_empty());
    }

    #[test]
    fn rows_and_width_read_off_the_shape() {
        let batch = ColumnBatch::new(vec![column(&[1, 2]), column(&[3, 4])]).unwrap();
        assert_eq!(batch.rows(), 2);
        assert_eq!(batch.width(), 2);
        assert!(!batch.is_empty());
    }

    #[test]
    fn cells_are_addressed_by_row_then_column() {
        let batch = ColumnBatch::new(vec![column(&[10, 20]), column(&[30, 40])]).unwrap();
        assert_eq!(batch.value(0, 0), Some(&Value::Int(10)));
        assert_eq!(batch.value(1, 0), Some(&Value::Int(20)));
        assert_eq!(batch.value(0, 1), Some(&Value::Int(30)));
        // Out of range is None, not a panic: the grid can ask for a row that a
        // concurrent clear has removed.
        assert_eq!(batch.value(9, 0), None);
        assert_eq!(batch.value(0, 9), None);
    }

    #[test]
    fn a_single_column_batch_is_normal() {
        let batch = ColumnBatch::new(vec![column(&[1])]).unwrap();
        assert_eq!(batch.width(), 1);
        assert_eq!(batch.rows(), 1);
    }
}
