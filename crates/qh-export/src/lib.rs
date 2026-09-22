//! Streaming export writers, one per format.
//!
//! This is the Rust side of `exporter/writers.py`. Every writer takes rows one at a
//! time and writes as it goes, so peak memory stays flat however many rows a query
//! returns — which is the whole reason this is a crate and not a function that
//! builds a `String`.
//!
//! # Which formats exist today
//!
//! All nine are here: `text`, `csv`, `json`, `xml`, `html`, `sql`, `dbf`, `xlsx` and
//! `xls`.
//!
//! The strength of the claim differs by format, and knowing which is which matters more
//! than a single label.
//!
//! `dbf`'s bytes are decided by code — the Python engine wrote it by hand too — so its
//! output is compared **byte for byte** with the Python engine's output, and that
//! comparison is a test.
//!
//! `xlsx` and `xls` are written by `openpyxl` and `xlwt` in the Python engine, so
//! matching their bytes is neither possible nor meaningful. What is claimed there is a
//! workbook the reference library reads back with the same cell values — verified by
//! actually doing that: `openpyxl` for `xlsx` and `xlrd` for `xls`.
//!
//! [`Format::implemented`] is the honest answer to "can I have this one", and it is
//! there so a caller can grey out a format instead of discovering the gap by
//! clicking it.
//!
//! # Two escaping rules that are not the obvious ones
//!
//! 1. **`html.escape` escapes quotes**, and the XML writer uses it too, because the
//!    Python module has a single `from html import escape` at the top. So an XML
//!    element's text comes out with `&quot;` and `&#x27;`, which `xml.sax.saxutils`
//!    would not have produced. Unusual for XML, and preserved: an export that
//!    changed shape between the two engines would be a regression dressed as a fix.
//! 2. **`bytes` are hex in text and `X'..'` in SQL**, not base64. See `qh-core::render`
//!    for the same distinction one layer down.
//!
//! [`plan`] is the layer above: it decides how many files an export becomes and what
//! they are called.
//!
//! # What is deliberately not here yet
//!
//! - `encoding`: the Python engine could write any codec with `errors="replace"`.
//!   This crate writes UTF-8. A writer that silently re-encoded would be worse than
//!   one that cannot, so there is no option pretending otherwise.
//! - `bundle`, which zipped a multi-part export into one download. It needs a
//!   deflate implementation, and a stored-only zip of a 2 GB export is not an
//!   improvement on four files.

mod dbf;
pub mod plan;
mod writers;
mod xls;
mod xlsx;
mod zip;

use std::io;
use std::path::Path;

use qh_core::{ColumnMeta, Value};
use thiserror::Error;

pub use dbf::DbfWriter;
pub use plan::{export_rows, ExportOutcome, ExportSpec, Exporter};
pub use writers::{DelimitedWriter, HtmlWriter, JsonWriter, SqlWriter, XmlWriter};
pub use xls::XlsWriter;
pub use xlsx::XlsxWriter;
pub use zip::bundle;

/// One export format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Format {
    Text,
    Csv,
    Json,
    Xml,
    Html,
    Sql,
    Xlsx,
    Xls,
    Dbf,
}

impl Format {
    /// Every format the Python engine offered, in the order it listed them.
    ///
    /// The order is the Python source's `WRITERS` order, `xls` before `xlsx` included:
    /// it is the order a format menu shows, and a menu that reorders itself between the
    /// two engines is a small thing a user still notices.
    pub const ALL: [Format; 9] = [
        Format::Text,
        Format::Csv,
        Format::Json,
        Format::Xml,
        Format::Html,
        Format::Sql,
        Format::Xls,
        Format::Xlsx,
        Format::Dbf,
    ];

    /// The format's own name, which is the key the Python engine filed it under.
    ///
    /// So `Text` is `txt`, not `text`: a saved preference, a command-line flag and a
    /// URL parameter all carry `txt`, and answering with a different word here would
    /// break every one of them. [`Format::parse`] takes `text` as well, because that is
    /// what a person types.
    pub const fn name(self) -> &'static str {
        match self {
            Format::Text => "txt",
            Format::Csv => "csv",
            Format::Json => "json",
            Format::Xml => "xml",
            Format::Html => "html",
            Format::Sql => "sql",
            Format::Xlsx => "xlsx",
            Format::Xls => "xls",
            Format::Dbf => "dbf",
        }
    }

    /// The file extension, which is what the UI shows on the save panel.
    pub const fn extension(self) -> &'static str {
        match self {
            Format::Text => "txt",
            Format::Csv => "csv",
            Format::Json => "json",
            Format::Xml => "xml",
            Format::Html => "html",
            Format::Sql => "sql",
            Format::Xlsx => "xlsx",
            Format::Xls => "xls",
            Format::Dbf => "dbf",
        }
    }

    /// Case-insensitive, because a format arrives from a menu, a URL or a command
    /// line and none of those agree on case.
    ///
    /// `text` is accepted as well as [`Format::name`]'s `txt`: one is what the Python
    /// engine filed the format under and the other is what a person writes, and a
    /// lookup that took only one of them would be a trap for whoever guessed the other.
    pub fn parse(name: &str) -> Option<Self> {
        let name = name.trim().to_ascii_lowercase();
        let name = if name == "text" { "txt" } else { &name };
        Self::ALL.into_iter().find(|format| format.name() == name)
    }

    /// Data rows per file before the export splits into parts.
    ///
    /// `None` for the streaming formats, which have no ceiling — Python's
    /// `Writer.max_rows`. The two Excel writers are the ones that set it, and they set
    /// it to the sheet's row limit **less the header row**: `XLS_MAX_ROWS - 1` and
    /// `XLSX_MAX_ROWS - 1`. Reading the constants instead of the writers would be a row
    /// too generous, and that row is the difference between a clean split and an export
    /// the writer refuses. Both numbers are why a sheet is split at all: BIFF8 has no
    /// streaming mode, so a whole sheet is buffered.
    pub const fn max_rows(self) -> Option<usize> {
        match self {
            Format::Xls => Some(65_536 - 1),
            Format::Xlsx => Some(1_048_576 - 1),
            _ => None,
        }
    }

    /// Whether this crate can write it yet.
    ///
    /// A format can be named in this enum and not be writable: that is a real state,
    /// and saying so lets the UI offer six formats instead of nine rather than
    /// offering nine and failing on three.
    pub const fn implemented(self) -> bool {
        matches!(
            self,
            Format::Text
                | Format::Csv
                | Format::Json
                | Format::Xml
                | Format::Html
                | Format::Sql
                | Format::Dbf
                | Format::Xlsx
                | Format::Xls
        )
    }

    /// Whether rows can be rendered away from the writer.
    ///
    /// True for the formats whose rows are independent of each other, which is what
    /// makes parallel rendering possible at all. `json` and `sql` carry per-row state
    /// — JSON's separators, SQL's `INSERT` batching — so a chunk cannot be rendered
    /// without knowing where it sits, and Python's own writers say `renderable =
    /// False` for exactly those two.
    pub const fn renderable(self) -> bool {
        matches!(
            self,
            Format::Text | Format::Csv | Format::Xml | Format::Html
        )
    }
}

/// What the caller wants for an export.
///
/// One struct rather than per-writer constructors, because these arrive from a UI
/// dialog and a dialog has one set of controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOptions {
    /// Write a header row. Default true, as in the Python engine.
    pub header: bool,
    /// Field separator for `text` and `csv`. `None` keeps the format's own default
    /// (`\t` and `,`), which is different from an empty separator.
    pub delimiter: Option<char>,
    pub quotechar: char,
    /// What a NULL looks like. Empty by default: a CSV cell is empty, and the Python
    /// engine's `null_text` defaults to `""`.
    pub null_text: String,
    /// `\r\n`, which is what the Python engine writes and what Excel expects.
    pub lineterminator: String,
    /// A UTF-8 byte-order mark, so Excel opens a UTF-8 CSV correctly.
    pub bom: bool,
    /// One JSON object per line instead of an array.
    pub jsonl: bool,
    /// Indent for the JSON array form.
    pub indent: usize,
    pub xml_root: String,
    pub xml_record: String,
    /// HTML title. `None` falls back to the file's stem.
    pub title: Option<String>,
    pub sql_ident_quote: char,
    pub sql_rows_per_insert: usize,
    /// SQL table name. `None` falls back to the file's stem.
    pub sql_table: Option<String>,
    /// Width for a `dbf` text column, before the 4000-byte record budget trims it.
    pub dbf_char_width: usize,
    /// Worksheet name for `xlsx` and `xls`. `None` is `Sheet1`, and the format's
    /// 31-character limit is applied rather than refused.
    pub sheet: Option<String>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            header: true,
            delimiter: None,
            quotechar: '"',
            null_text: String::new(),
            lineterminator: "\r\n".to_owned(),
            bom: false,
            jsonl: false,
            indent: 2,
            xml_root: "RECORDS".to_owned(),
            xml_record: "RECORD".to_owned(),
            title: None,
            sql_ident_quote: '"',
            // 200, from the Python engine: several rows per statement keeps a replay
            // fast without building one enormous statement.
            sql_rows_per_insert: 200,
            sql_table: None,
            dbf_char_width: 254,
            sheet: None,
        }
    }
}

/// Why an export could not be written.
#[derive(Debug, Error)]
pub enum ExportError {
    #[error("{0}")]
    Io(#[from] io::Error),

    /// The format exists in this crate's vocabulary but has no writer yet. Distinct
    /// from a usage error: nothing the caller can change will make it work.
    #[error(
        "{format} export is not implemented yet. Kept for the case of a format being added \
         to `Format::ALL` before its writer exists, which is exactly when this should be \
         said rather than producing a file that is wrong."
    )]
    Unsupported { format: &'static str },

    #[error("{message}")]
    Usage { message: String },

    /// The row source failed partway through.
    ///
    /// Kept apart from [`ExportError::Usage`] because they call for different
    /// responses: a usage error is the caller's to fix, and this one is not — the
    /// query died, or the connection did. It carries the original error rather than
    /// its text so that a caller can still see what kind of failure it was.
    #[error("the row source failed: {0}")]
    Source(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl ExportError {
    /// Wrap a failure from whatever is feeding rows in.
    pub fn source(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        ExportError::Source(Box::new(error))
    }
}

/// One writer, open on a file.
pub trait Writer: Send {
    /// Write one row. `row` is as wide as the columns the writer was opened with.
    fn write_row(&mut self, row: &[Value]) -> Result<(), ExportError>;

    /// Finish the file. Called once, and a writer that needs a trailer writes it
    /// here (`</RECORDS>`, `]`, the last `INSERT`).
    fn finish(&mut self) -> Result<(), ExportError>;

    /// How many values were cut to fit a field, for the formats that can be forced to
    /// cut one. `dbf` fields are fixed width, which is the case this exists for; a
    /// format that never truncates answers zero, as the Python engine's
    /// `getattr(writer, "truncated", 0)` does.
    fn truncated(&self) -> usize {
        0
    }
}

/// Render rows to text without touching a writer, for the parallel path.
///
/// `None` when the format is not [`Format::renderable`], which is a real answer and
/// not a failure: the caller falls back to writing row by row.
///
/// The returned text is what the writer would have written for those rows, so
/// concatenating chunks in order gives the same file as writing them one at a time
/// — which is the property that makes using this from several threads safe, and the
/// one a test pins.
pub fn render_rows(
    format: Format,
    columns: &[ColumnMeta],
    options: &ExportOptions,
    rows: &[Vec<Value>],
) -> Option<String> {
    if !format.renderable() {
        return None;
    }
    match format {
        Format::Text | Format::Csv => Some(writers::render_delimited(format, options, rows)),
        Format::Xml => Some(writers::render_xml(options, columns, rows)),
        Format::Html => Some(writers::render_html(rows)),
        _ => None,
    }
}

/// Open a writer on `path`.
pub fn open(
    format: Format,
    path: &Path,
    columns: &[ColumnMeta],
    options: &ExportOptions,
) -> Result<Box<dyn Writer>, ExportError> {
    if !format.implemented() {
        return Err(ExportError::Unsupported {
            format: format.name(),
        });
    }
    if columns.is_empty() {
        return Err(ExportError::Usage {
            message: "an export needs at least one column: the writers take their header and \
                      their shape from them"
                .to_owned(),
        });
    }

    Ok(match format {
        Format::Text => Box::new(DelimitedWriter::new(path, columns, options, false)?),
        Format::Csv => Box::new(DelimitedWriter::new(path, columns, options, true)?),
        Format::Json => Box::new(JsonWriter::new(path, columns, options)?),
        Format::Xml => Box::new(XmlWriter::new(path, columns, options)?),
        Format::Html => Box::new(HtmlWriter::new(path, columns, options)?),
        Format::Sql => Box::new(SqlWriter::new(path, columns, options)?),
        Format::Dbf => Box::new(DbfWriter::new(path, columns, options)?),
        Format::Xlsx => Box::new(XlsxWriter::new(path, columns, options)?),
        Format::Xls => Box::new(XlsWriter::new(path, columns, options)?),
    })
}

/// Escape text the way `html.escape` does.
///
/// All five of these, because that is the function the Python module imports — and
/// it is the *same* call in the XML writer, which is why an XML element's text can
/// contain `&quot;`.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            // First, or it would double-escape the ampersands introduced below.
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_format_the_python_engine_had_is_named_here() {
        // The Python source's `WRITERS` keys, in `WRITERS` order. Written out here
        // rather than derived, so that renaming one on this side has to be a deliberate
        // edit to this list too.
        let python_keys = [
            "txt", "csv", "json", "xml", "html", "sql", "xls", "xlsx", "dbf",
        ];
        assert_eq!(Format::ALL.len(), python_keys.len());
        let names: Vec<&str> = Format::ALL.iter().map(|format| format.name()).collect();
        assert_eq!(
            names, python_keys,
            "the names and their order are the engine's"
        );
        for name in python_keys {
            assert_eq!(Format::parse(name).map(Format::name), Some(name));
        }
        // `txt` is the engine's key and `text` is what a person writes. A lookup that
        // took only one of them would be a trap for whoever guessed the other.
        assert_eq!(Format::parse("text"), Some(Format::Text));
        // Case does not matter: this arrives from a menu, a URL or a command line.
        assert_eq!(Format::parse("CSV"), Some(Format::Csv));
        assert_eq!(Format::parse("  Json "), Some(Format::Json));
        assert_eq!(Format::parse("parquet"), None);
    }

    #[test]
    fn every_format_has_a_writer() {
        // Stated as a test so that a format added without a writer fails here rather
        // than at the first export.
        assert_eq!(
            Format::ALL
                .iter()
                .filter(|format| format.implemented())
                .count(),
            Format::ALL.len()
        );
        for format in Format::ALL {
            assert!(format.implemented(), "{} has no writer", format.name());
        }
    }

    #[test]
    fn the_row_ceilings_are_the_sheets_own_limits() {
        // From the Python source, which writes `max_rows = XLS_MAX_ROWS - 1` and
        // `XLSX_MAX_ROWS - 1`: the header counts against the sheet, so the data-row
        // ceiling is one less than the sheet limit. They are also why a sheet is split
        // at all -- BIFF8 has no streaming mode, so the whole sheet is buffered.
        assert_eq!(Format::Xls.max_rows(), Some(65_536 - 1));
        assert_eq!(Format::Xlsx.max_rows(), Some(1_048_576 - 1));
        // And one less than the writers' own internal guard, which refuses at the sheet
        // limit: the planner must always split before the writer has to refuse.
        assert!(Format::Xlsx.max_rows().unwrap() < 1_048_576);
        // The streaming formats have no ceiling to record.
        for format in [
            Format::Text,
            Format::Csv,
            Format::Json,
            Format::Xml,
            Format::Html,
            Format::Sql,
        ] {
            assert_eq!(format.max_rows(), None, "{}", format.name());
        }
    }

    #[test]
    fn only_the_formats_whose_rows_stand_alone_are_renderable() {
        // json and sql carry per-row state -- JSON's separators, SQL's INSERT
        // batching -- so a chunk cannot be rendered without knowing where it sits.
        // Python says `renderable = False` for exactly those two.
        for format in [Format::Text, Format::Csv, Format::Xml, Format::Html] {
            assert!(format.renderable(), "{}", format.name());
        }
        for format in [Format::Json, Format::Sql] {
            assert!(!format.renderable(), "{}", format.name());
        }
    }

    #[test]
    fn escaping_is_html_escape_and_not_the_xml_one() {
        // The whole reason this is worth a test: the module imports `html.escape`, so
        // the XML writer escapes quotes too and `&quot;` appears inside XML elements.
        // `xml.sax.saxutils.escape` would have left them alone.
        assert_eq!(escape("a\"b<c>&d'e"), "a&quot;b&lt;c&gt;&amp;d&#x27;e");
        assert_eq!(escape("<&>"), "&lt;&amp;&gt;");
        // The ampersand is replaced first, so the entities above are not escaped
        // again into `&amp;lt;`.
        assert_eq!(escape("&lt;"), "&amp;lt;");
        // Non-ASCII is left as itself.
        assert_eq!(escape("café"), "café");
    }

    #[test]
    fn an_export_with_no_columns_is_refused() {
        let outcome = open(
            Format::Csv,
            Path::new("/tmp/x.csv"),
            &[],
            &ExportOptions::default(),
        );
        match outcome {
            Err(ExportError::Usage { message }) => assert!(message.contains("column"), "{message}"),
            other => panic!("expected a usage error, got {:?}", other.is_ok()),
        }
    }

    #[test]
    fn a_format_that_is_not_renderable_says_none_rather_than_pretending() {
        // A real answer, not a failure: the caller falls back to writing row by row.
        assert_eq!(
            render_rows(
                Format::Sql,
                &[ColumnMeta::new("a", "bigint")],
                &ExportOptions::default(),
                &[]
            ),
            None
        );
    }
}
