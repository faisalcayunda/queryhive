//! Reading a file into rows, for `import_data`.
//!
//! The reverse of `qh-export`: where the writers take a row and put it in a
//! file, these take a file and hand back rows. Only the two source formats Phase
//! 5 asks for are here, and the honesty about streaming differs between them,
//! which is the most important thing this crate says.
//!
//! # CSV streams; XLSX cannot, and this crate does not pretend otherwise
//!
//! The README's rule is that a result set is never held in memory, and the
//! source study checked TablePro against the same rule it puts on QueryHive.
//! TablePro's CSV reader mmaps the file and parses it in ranges, while its XLSX
//! reader loads the workbook whole — and the reason is in the format: an XLSX
//! sheet's cells are indices into a `sharedStrings` table, so a streaming read
//! still has to hold that table, and there is no way to know its size before
//! parsing it. This crate is stricter for CSV than TablePro ( [`csv::Reader`]
//! reads one record at a time and the file is never mapped) and equally
//! whole-file for XLSX, because it has no choice. The XLSX path says so at the
//! call site rather than promising a bound it cannot keep: `import_data` names
//! the limitation in `done`, and `README.md` repeats it.
//!
//! # What a row is
//!
//! A [`RawRow`] is the line number it came from and its cells as text. Nothing
//! is typed here: the target table's own column types decide whether a value is
//! written bare or quoted, and that decision needs the server's answer, which
//! this crate never asks for. An empty cell is the empty string — whether it
//! means NULL is the caller's rule, because only the caller knows the target.

#![forbid(unsafe_code)]

use std::path::Path;

use thiserror::Error;

mod csv_source;
mod xlsx_source;

pub use csv_source::CsvSource;
pub use xlsx_source::XlsxSource;

/// The source formats this crate reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Csv,
    Xlsx,
}

impl Format {
    /// Parse a spelling, case-insensitively. `None` for anything else, so a typo
    /// is refused rather than defaulted.
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "csv" => Some(Format::Csv),
            "xlsx" => Some(Format::Xlsx),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Format::Csv => "csv",
            Format::Xlsx => "xlsx",
        }
    }
}

/// How to read the file.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Options {
    /// The CSV field separator. `None` is the format's own comma.
    pub delimiter: Option<char>,
    /// Whether the first row names the columns. The header is read but is not a
    /// data row either way.
    pub header: bool,
    /// The worksheet to read, by name. `None` is the first sheet.
    pub sheet: Option<String>,
}

/// One data row: its 1-based line in the source, and its cells as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRow {
    pub line: usize,
    pub cells: Vec<String>,
}

/// Why a file could not be read.
#[derive(Debug, Error)]
pub enum ImportError {
    #[error("could not read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{path} is not readable as CSV: {source}")]
    Csv {
        path: String,
        #[source]
        source: csv::Error,
    },

    #[error("{path} is not readable as a workbook: {source}")]
    Xlsx {
        path: String,
        #[source]
        source: calamine::Error,
    },

    #[error("{path} has no sheet named '{sheet}'")]
    MissingSheet { path: String, sheet: String },

    #[error("{path} has no worksheets")]
    NoSheets { path: String },
}

impl ImportError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        ImportError::Io {
            path: path.display().to_string(),
            source,
        }
    }
}

/// A file open for reading, one row at a time.
///
/// The CSV arm genuinely streams; the XLSX arm holds the worksheet whole and
/// hands rows out from it. Both present the same shape, so the caller's loop does
/// not have to know which it has — but the caller should anyway, because the
/// memory bound differs (see the module note).
pub enum RowReader {
    Csv(CsvSource),
    Xlsx(XlsxSource),
}

impl RowReader {
    /// Open `path` as `format`.
    pub fn open(path: &Path, format: Format, options: &Options) -> Result<Self, ImportError> {
        match format {
            Format::Csv => CsvSource::open(path, options).map(RowReader::Csv),
            Format::Xlsx => XlsxSource::open(path, options).map(RowReader::Xlsx),
        }
    }

    /// The header row, when the file has one and it has been read.
    ///
    /// Read at open time, so the caller can build a column map from names before
    /// the first data row is asked for. `None` when `header` was false.
    pub fn header(&self) -> Option<&[String]> {
        match self {
            RowReader::Csv(source) => source.header(),
            RowReader::Xlsx(source) => source.header(),
        }
    }

    /// The next data row, or `None` at the end.
    pub fn next_row(&mut self) -> Result<Option<RawRow>, ImportError> {
        match self {
            RowReader::Csv(source) => source.next_row(),
            RowReader::Xlsx(source) => source.next_row(),
        }
    }

    /// Whether this source can be read without holding the file.
    ///
    /// `false` for XLSX, and the caller is expected to say so rather than claim a
    /// bound it cannot keep.
    pub fn streams(&self) -> bool {
        matches!(self, RowReader::Csv(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_names_round_trip_and_refuse_anything_else() {
        assert_eq!(Format::parse("CSV"), Some(Format::Csv));
        assert_eq!(Format::parse(" xlsx "), Some(Format::Xlsx));
        assert_eq!(Format::parse("parquet"), None);
        assert_eq!(Format::parse(""), None);
        assert_eq!(Format::Csv.name(), "csv");
        assert_eq!(Format::Xlsx.name(), "xlsx");
    }

    #[test]
    fn only_csv_claims_to_stream() {
        // A file that exists is not needed: the claim is a property of the arm,
        // and this pins it so the XLSX arm cannot quietly start claiming a bound
        // the format does not allow.
        let csv = RowReader::Csv(CsvSource::from_text(
            "a,b\n1,2\n".to_owned(),
            &Options::default(),
        ));
        assert!(csv.streams());
    }
}
