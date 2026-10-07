//! Reading a file into rows, for `import_data`.
//!
//! The reverse of `qh-export`: where the writers take a row and put it in a
//! file, these take a file and hand back rows. Only the two source formats Phase
//! 5 asks for are here, and the honesty about streaming differs between them,
//! which is the most important thing this crate says.
//!
//! # CSV streams; XLSX streams its rows but must hold its string table
//!
//! The README's rule is that a result set is never held in memory, and the
//! source study checked TablePro against the same rule it puts on QueryHive.
//! TablePro's CSV reader mmaps the file and parses it in ranges, while its XLSX
//! reader loads the workbook whole. This crate is stricter for CSV
//! ( [`csv::Reader`] reads one record at a time and the file is never mapped).
//! For XLSX the rows stream (`calamine`'s cell reader, on its own thread, a
//! bounded queue of rows ahead of the caller) but the format interleaves a sheet
//! with a `sharedStrings` table whose size is unknown until it is parsed, so that
//! table is held. What bounds the memory is therefore a guard, not the reader: a
//! file-size limit and a rows-times-columns limit ([`XLSX_MAX_BYTES`],
//! [`XLSX_MAX_CELLS`]), refused with a [`ImportError::Limit`] before the first
//! write. The engine is in-process, so running out of memory would take the app
//! and its unsaved editor text with it.
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
mod json_source;
mod values;
mod xlsx_source;

pub use csv_source::CsvSource;
pub use json_source::{JsonRow, JsonSource};
pub use qh_export::Codec;
pub use values::{normalize_number, DatePattern, IsoDateTime, ValueError};
pub use xlsx_source::{excel_digits, XlsxSource, XLSX_MAX_BYTES, XLSX_MAX_CELLS};

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
    /// The CSV file's code page. `None` is UTF-8, and a file that is not valid in the
    /// code page is refused before any row is read.
    pub encoding: Option<Codec>,
    /// The largest workbook file read, in bytes. `None` is [`XLSX_MAX_BYTES`].
    pub xlsx_max_bytes: Option<u64>,
    /// The most cells (rows times columns) a worksheet may declare or hold. `None` is
    /// [`XLSX_MAX_CELLS`].
    pub xlsx_max_cells: Option<u64>,
}

/// One data row: its 1-based line in the source, and its cells as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawRow {
    pub line: usize,
    pub cells: Vec<String>,
    /// Which cells the file stores as a number, not as text (XLSX), so their text is already
    /// in `.`-decimal form whatever locale the file's text cells are written in. Empty for a
    /// format with no typed cells (CSV): every cell is text.
    pub numeric: Vec<bool>,
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

    #[error("{path} is not readable as JSON (line {line}, byte offset {offset}): {message}")]
    Json {
        path: String,
        line: usize,
        offset: usize,
        message: String,
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

    /// The file is larger than the caller allowed. A usage problem, not a broken file.
    #[error("{path} is too large to import: {message}")]
    Limit { path: String, message: String },

    /// The file is not valid in the encoding it was read as.
    #[error("{path} line {line}, byte offset {offset}: {message}")]
    Encoding {
        path: String,
        line: usize,
        offset: usize,
        message: String,
    },
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
/// Both arms stream their rows and present the same shape; the XLSX arm also holds its
/// shared-string table, which is why it has size guards (see the module note).
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

    /// Whether the rows are read without holding the file. `true` for both formats;
    /// XLSX still holds its string table, bounded by [`XLSX_MAX_BYTES`].
    pub fn streams(&self) -> bool {
        true
    }

    /// Bytes of the file consumed so far, when the format can say (CSV). XLSX is a zip
    /// read by seeking, so a byte count would not mean "how far through the sheet".
    pub fn bytes_read(&self) -> Option<u64> {
        match self {
            RowReader::Csv(source) => source.bytes_read(),
            RowReader::Xlsx(_) => None,
        }
    }

    /// How many data rows the file says it has, when it says (the XLSX `dimension`).
    pub fn rows_total(&self) -> Option<u64> {
        match self {
            RowReader::Csv(_) => None,
            RowReader::Xlsx(source) => source.rows_total(),
        }
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
    fn a_csv_over_text_streams_and_has_no_byte_count() {
        let csv = RowReader::Csv(CsvSource::from_text(
            "a,b\n1,2\n".to_owned(),
            &Options::default(),
        ));
        assert!(csv.streams());
        assert_eq!(csv.bytes_read(), None);
    }
}
