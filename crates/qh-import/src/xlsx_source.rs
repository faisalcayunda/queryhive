//! The XLSX source: the whole worksheet, held, because the format leaves no
//! choice.
//!
//! Read with `calamine`, which is MIT and handles the two things that make a
//! streaming XLSX reader impossible to promise: cells are indices into a
//! `sharedStrings` part, and the sheet does not say how many strings there are
//! until the part has been parsed. TablePro's own reader loads the workbook
//! whole for the same reason. So this source reads the range once, into rows,
//! and hands them out from a vector.
//!
//! Cells are rendered as text. A date or time cell is rendered as an ISO-8601
//! timestamp rather than Excel's serial number, so it imports as a date into a
//! date column; a number keeps its digits; a formula cell carries its cached
//! value, which is what calamine reports. An empty cell is the empty string, the
//! same as an empty CSV field — the caller decides whether that is NULL.

use std::path::Path;

use calamine::{open_workbook_auto, Data, Reader};

use crate::{ImportError, Options, RawRow};

/// A worksheet read whole, yielding its rows.
pub struct XlsxSource {
    rows: std::vec::IntoIter<Vec<String>>,
    header: Option<Vec<String>>,
    /// 1-based line of the row `rows` will yield next, so a message can name it.
    line: usize,
}

impl XlsxSource {
    /// Open `path` and read the requested sheet's range.
    pub fn open(path: &Path, options: &Options) -> Result<Self, ImportError> {
        let path_text = path.display().to_string();
        let mut workbook = open_workbook_auto(path).map_err(|error| ImportError::Xlsx {
            path: path_text.clone(),
            source: error,
        })?;

        let names = workbook.sheet_names();
        let sheet = match &options.sheet {
            Some(wanted) => {
                if !names.iter().any(|name| name == wanted) {
                    return Err(ImportError::MissingSheet {
                        path: path_text.clone(),
                        sheet: wanted.clone(),
                    });
                }
                wanted.clone()
            }
            None => names
                .into_iter()
                .next()
                .ok_or_else(|| ImportError::NoSheets {
                    path: path_text.clone(),
                })?,
        };

        let range = workbook
            .worksheet_range(&sheet)
            .map_err(|error| ImportError::Xlsx {
                path: path_text.clone(),
                source: error,
            })?;

        let mut rows: Vec<Vec<String>> = range
            .rows()
            .map(|row| row.iter().map(cell_text).collect())
            .collect();
        let header = if options.header && !rows.is_empty() {
            Some(rows.remove(0))
        } else {
            None
        };
        Ok(Self {
            rows: rows.into_iter(),
            header,
            line: 1,
        })
    }

    pub fn header(&self) -> Option<&[String]> {
        self.header.as_deref().filter(|header| !header.is_empty())
    }

    pub fn next_row(&mut self) -> Result<Option<RawRow>, ImportError> {
        // Empty rows are data here, not skipped: an XLSX row that exists but is
        // blank is a row the user can see, and dropping it would make the line
        // numbers in an error message not match the sheet. The caller counts a
        // row with no mapped value as rejected, which is where that decision
        // lives.
        let Some(cells) = self.rows.next() else {
            return Ok(None);
        };
        let line = self.line;
        self.line += 1;
        Ok(Some(RawRow { line, cells }))
    }
}

/// One cell as text.
fn cell_text(cell: &Data) -> String {
    match cell {
        // A date cell arrives as a serial number unless it is asked for as a
        // datetime, which the `dates`+`chrono` features make possible. An ISO
        // timestamp imports into a date or timestamp column; the serial number
        // would not.
        Data::DateTime(value) => match value.as_datetime() {
            Some(stamp) => stamp.to_string(),
            None => value.to_string(),
        },
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_date_cell_becomes_an_iso_timestamp_not_a_serial_number() {
        // 2026-01-31 as an Excel serial, the number a date cell holds on disk.
        let cell = Data::DateTime(calamine::ExcelDateTime::new(
            46_053.0,
            calamine::ExcelDateTimeType::DateTime,
            false,
        ));
        let text = cell_text(&cell);
        assert!(text.starts_with("2026-01-31"), "{text}");
        assert!(
            !text.starts_with("46053"),
            "a serial number would import as a number, not a date"
        );
    }

    #[test]
    fn an_empty_cell_is_the_empty_string() {
        assert_eq!(cell_text(&Data::Empty), "");
    }

    #[test]
    fn a_string_and_a_number_keep_their_text() {
        assert_eq!(cell_text(&Data::String("hello".to_owned())), "hello");
        assert_eq!(cell_text(&Data::Int(42)), "42");
        assert_eq!(cell_text(&Data::Float(1.5)), "1.5");
        assert_eq!(cell_text(&Data::Bool(true)), "true");
    }
}
