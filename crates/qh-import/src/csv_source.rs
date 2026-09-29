//! The CSV source: a real stream, one record at a time.
//!
//! `csv::Reader` over the file handle. `has_headers(false)` because the header is
//! read here, once, so it can be named before a data row is asked for —
//! `has_headers(true)` would hide the first row from the record loop without
//! handing it back in a form this crate can carry.
//!
//! `flexible(true)` because a ragged CSV file is a normal file: a row with too
//! few or too many fields is padded or truncated against the column map at the
//! point where that decision belongs, not made a parse failure here.

use std::io::Read;
use std::path::Path;

use crate::{ImportError, Options, RawRow};

/// An open CSV file, or an in-memory CSV for a test.
pub struct CsvSource {
    reader: csv::Reader<Box<dyn Read + Send>>,
    header: Option<Vec<String>>,
    path: String,
    want_header: bool,
}

impl CsvSource {
    /// Open `path` as CSV.
    pub fn open(path: &Path, options: &Options) -> Result<Self, ImportError> {
        let file = std::fs::File::open(path).map_err(|error| ImportError::io(path, error))?;
        let mut source = Self::build(Box::new(file), path.display().to_string(), options);
        source.detect_header()?;
        Ok(source)
    }

    /// A source over text already in hand, for a test.
    pub fn from_text(text: String, options: &Options) -> Self {
        let mut source = Self::build(
            Box::new(std::io::Cursor::new(text.into_bytes())),
            String::from("<memory>"),
            options,
        );
        let _ = source.detect_header();
        source
    }

    fn build(reader: Box<dyn Read + Send>, path: String, options: &Options) -> Self {
        let mut builder = csv::ReaderBuilder::new();
        builder.has_headers(false).flexible(true);
        if let Some(delimiter) = options.delimiter {
            // `csv` takes a byte. A delimiter that is not one ASCII byte is
            // refused by the caller before this is reached.
            builder.delimiter(delimiter as u8);
        }
        Self {
            reader: builder.from_reader(reader),
            header: None,
            path,
            want_header: options.header,
        }
    }

    /// Read the first record as the header, when the caller asked for one. The
    /// header is consumed here so `next_row` never sees it.
    fn detect_header(&mut self) -> Result<(), ImportError> {
        if !self.want_header {
            return Ok(());
        }
        let mut record = csv::StringRecord::new();
        let read = self
            .reader
            .read_record(&mut record)
            .map_err(|error| ImportError::Csv {
                path: self.path.clone(),
                source: error,
            })?;
        if read {
            self.header = Some(record.iter().map(str::to_owned).collect());
        }
        Ok(())
    }

    /// The header row, when the file has one and it has been read.
    pub fn header(&self) -> Option<&[String]> {
        self.header.as_deref().filter(|header| !header.is_empty())
    }

    pub fn next_row(&mut self) -> Result<Option<RawRow>, ImportError> {
        let mut record = csv::StringRecord::new();
        let read = self
            .reader
            .read_record(&mut record)
            .map_err(|error| ImportError::Csv {
                path: self.path.clone(),
                source: error,
            })?;
        if !read {
            return Ok(None);
        }
        let line = record
            .position()
            .map(|position| position.line() as usize)
            .unwrap_or(0);
        let cells = record.iter().map(str::to_owned).collect();
        Ok(Some(RawRow { line, cells }))
    }
}
