//! The XLSX source: rows streamed from the worksheet, a guard on how big a workbook may be.
//!
//! Read with `calamine`, which is MIT. For `.xlsx` and `.xlsm` its `worksheet_cells_reader`
//! walks the sheet's XML one cell at a time, so the sheet is never copied into a dense
//! range the way `worksheet_range` does (one `Data` per cell, then a second copy as text).
//! What cannot stream is the `sharedStrings` part: a cell holds an index into it, and its
//! size is unknown until it has been parsed. It is held, which is why a file-size limit
//! ([`XLSX_MAX_BYTES`]) and a rows-times-columns limit ([`XLSX_MAX_CELLS`]) are checked
//! before the first row is handed out. A limit is an [`ImportError::Limit`] naming the
//! setting that raises it: the engine runs inside the app, so an out-of-memory kill would
//! take its unsaved editor text too. `.xls`, `.xlsb` and `.ods` have no cell reader and
//! are read whole through `open_workbook_auto`, under the same two limits.
//!
//! The streaming reader borrows the workbook, so it lives on its own thread and hands
//! rows over a bounded queue. A queue that is never drained stops the thread; a thread
//! that dies without saying so is an error here, never a short file.
//!
//! # What a row is
//!
//! A row's line is its row number in the sheet. Leading empty rows are skipped (the first
//! row with a cell is the header), and an empty row between data rows is a row, so a line
//! number in a message is the one the user sees. Columns are counted from the first column
//! of the sheet's own `dimension`, and every row is padded to its width.
//!
//! # Cells as text
//!
//! A date or time cell is rendered as an ISO-8601 timestamp rather than Excel's serial
//! number, so it imports as a date into a date column. A formula cell carries its cached
//! value. An empty cell is the empty string, the same as an empty CSV field; the caller
//! decides whether that is NULL. A number cell keeps every digit the file stores (the
//! shortest text that reads back as the same `f64`), and [`RawRow::numeric`] marks it, so the
//! caller can tell it from a text cell that merely looks like a number: a text cell is read
//! in the file's locale, a number cell never is. A number landing in a text column is the
//! one place the digits are cut, to the 15 an Excel user sees ([`excel_digits`]): `0.1 + 0.2`
//! is `0.3` there and `0.30000000000000004` in a `float8`.

use std::path::Path;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};

use calamine::{open_workbook, open_workbook_auto, Data, Reader, Xlsx};

use crate::{ImportError, Options, RawRow};

/// The largest workbook file read, unless the caller says otherwise: 128 MiB.
pub const XLSX_MAX_BYTES: u64 = 128 * 1024 * 1024;
/// The most cells (rows times columns) a worksheet may declare or hold: 50 million.
pub const XLSX_MAX_CELLS: u64 = 50_000_000;

/// Rows the reader thread may be ahead of the importer.
const QUEUE_ROWS: usize = 512;

enum Message {
    /// The sheet is open: how many rows it declares, header included.
    Opened(Option<u64>),
    Row(RawRow),
    End,
    Failed(ImportError),
}

/// A worksheet read row by row.
pub struct XlsxSource {
    rows: Receiver<Message>,
    header: Option<Vec<String>>,
    rows_total: Option<u64>,
    /// The first row, when it was not the header: handed out before the queue.
    first: Option<RawRow>,
    finished: bool,
    path: String,
}

impl XlsxSource {
    /// Open `path`, check the limits, and read the requested sheet's first row.
    pub fn open(path: &Path, options: &Options) -> Result<Self, ImportError> {
        let path_text = path.display().to_string();
        let max_bytes = options.xlsx_max_bytes.unwrap_or(XLSX_MAX_BYTES);
        let max_cells = options.xlsx_max_cells.unwrap_or(XLSX_MAX_CELLS);
        let size = std::fs::metadata(path)
            .map_err(|error| ImportError::io(path, error))?
            .len();
        if size > max_bytes {
            return Err(ImportError::Limit {
                path: path_text,
                message: format!(
                    "{size} bytes against a limit of {max_bytes}; split the workbook, export \
                     the sheet as CSV, or raise IMPORT_XLSX_MAX_BYTES"
                ),
            });
        }

        let (sender, rows) = sync_channel(QUEUE_ROWS);
        let (path_owned, sheet) = (path.to_path_buf(), options.sheet.clone());
        std::thread::Builder::new()
            .name("qh-xlsx-rows".to_owned())
            .spawn(move || {
                if let Err(error) = pump(&path_owned, sheet.as_deref(), max_cells, &sender) {
                    let _ = sender.send(Message::Failed(error));
                }
            })
            .map_err(|error| ImportError::io(path, error))?;

        let mut source = Self {
            rows,
            header: None,
            rows_total: None,
            first: None,
            finished: false,
            path: path_text,
        };
        match source.receive()? {
            Message::Opened(total) => source.rows_total = total,
            Message::Failed(error) => return Err(error),
            Message::Row(_) | Message::End => {
                unreachable!("the reader thread opens the sheet before it sends a row")
            }
        }
        if let Some(first) = source.read_message()? {
            if options.header {
                source.header = Some(first.cells);
                source.rows_total = source.rows_total.map(|total| total.saturating_sub(1));
            } else {
                source.first = Some(first);
            }
        }
        Ok(source)
    }

    pub fn header(&self) -> Option<&[String]> {
        self.header.as_deref().filter(|header| !header.is_empty())
    }

    /// The data rows the sheet's `dimension` declares, when it declares one.
    pub fn rows_total(&self) -> Option<u64> {
        self.rows_total
    }

    pub fn next_row(&mut self) -> Result<Option<RawRow>, ImportError> {
        // Empty rows are data here, not skipped: an XLSX row that exists but is
        // blank is a row the user can see, and dropping it would make the line
        // numbers in an error message not match the sheet. The caller counts a
        // row with no mapped value as rejected, which is where that decision
        // lives.
        if let Some(first) = self.first.take() {
            return Ok(Some(first));
        }
        self.read_message()
    }

    fn receive(&mut self) -> Result<Message, ImportError> {
        self.rows.recv().map_err(|_| ImportError::Io {
            path: self.path.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "the worksheet reader stopped before the end of the sheet",
            ),
        })
    }

    fn read_message(&mut self) -> Result<Option<RawRow>, ImportError> {
        if self.finished {
            return Ok(None);
        }
        match self.receive()? {
            Message::Row(row) => Ok(Some(row)),
            Message::End => {
                self.finished = true;
                Ok(None)
            }
            Message::Failed(error) => {
                self.finished = true;
                Err(error)
            }
            Message::Opened(_) => unreachable!("a sheet is opened once"),
        }
    }
}

/// Open the workbook and push its rows into `sender`. Runs on the reader thread.
fn pump(
    path: &Path,
    wanted: Option<&str>,
    max_cells: u64,
    sender: &SyncSender<Message>,
) -> Result<(), ImportError> {
    let path_text = path.display().to_string();
    let xlsx_error = |error: calamine::XlsxError| ImportError::Xlsx {
        path: path_text.clone(),
        source: error.into(),
    };
    let too_big = |cells: u64| ImportError::Limit {
        path: path_text.clone(),
        message: format!(
            "the sheet has {cells} cells against a limit of {max_cells}; import part of it, \
             or raise IMPORT_XLSX_MAX_CELLS"
        ),
    };
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    // Whether the send worked: a dropped receiver means nobody wants the rest.
    let send = |message: Message| sender.send(message).is_ok();

    if matches!(extension.as_deref(), Some("xlsx" | "xlsm")) {
        let mut workbook: Xlsx<_> = open_workbook(path).map_err(xlsx_error)?;
        let sheet = pick_sheet(&workbook.sheet_names(), wanted, &path_text)?;
        let mut cells = workbook
            .worksheet_cells_reader(&sheet)
            .map_err(xlsx_error)?;
        let dimensions = cells.dimensions();
        let (rows, columns) = (
            u64::from(dimensions.end.0.saturating_sub(dimensions.start.0)) + 1,
            u64::from(dimensions.end.1.saturating_sub(dimensions.start.1)) + 1,
        );
        let declared = dimensions != calamine::Dimensions::default();
        if declared && rows.saturating_mul(columns) > max_cells {
            return Err(too_big(rows.saturating_mul(columns)));
        }
        if !send(Message::Opened(declared.then_some(rows))) {
            return Ok(());
        }

        let first_column = if declared { dimensions.start.1 } else { 0 };
        let mut width = if declared { columns as usize } else { 0 };
        let (mut current, mut line_cells): (Option<u32>, Vec<(usize, String, bool)>) =
            (None, Vec::new());
        let mut seen: u64 = 0;
        let flush = |row: u32, line_cells: &mut Vec<(usize, String, bool)>, width: &mut usize| {
            *width = (*width).max(line_cells.iter().map(|(at, ..)| at + 1).max().unwrap_or(0));
            let mut padded = vec![String::new(); *width];
            let mut numeric = vec![false; *width];
            for (at, text, number) in line_cells.drain(..) {
                padded[at] = text;
                numeric[at] = number;
            }
            send(Message::Row(RawRow {
                line: row as usize + 1,
                cells: padded,
                numeric,
            }))
        };
        while let Some(cell) = cells.next_cell().map_err(xlsx_error)? {
            let (row, column) = cell.get_position();
            let value = Data::from(cell.get_value().clone());
            let text = cell_text(&value);
            if text.is_empty() {
                continue;
            }
            seen += 1;
            if seen > max_cells {
                return Err(too_big(seen));
            }
            if current != Some(row) {
                if let Some(done) = current {
                    if !flush(done, &mut line_cells, &mut width) {
                        return Ok(());
                    }
                    // Empty rows between this one and the last are rows.
                    for blank in done + 1..row {
                        if !flush(blank, &mut line_cells, &mut width) {
                            return Ok(());
                        }
                    }
                }
                current = Some(row);
            }
            line_cells.push((
                column.saturating_sub(first_column) as usize,
                text,
                is_number(&value),
            ));
        }
        if let Some(done) = current {
            flush(done, &mut line_cells, &mut width);
        }
    } else {
        // `.xls`, `.xlsb`, `.ods`: no cell reader, so the range is read whole.
        let mut workbook = open_workbook_auto(path).map_err(|error| ImportError::Xlsx {
            path: path_text.clone(),
            source: error,
        })?;
        let sheet = pick_sheet(&workbook.sheet_names(), wanted, &path_text)?;
        let range = workbook
            .worksheet_range(&sheet)
            .map_err(|error| ImportError::Xlsx {
                path: path_text.clone(),
                source: error,
            })?;
        let (rows, columns) = range.get_size();
        if (rows as u64).saturating_mul(columns as u64) > max_cells {
            return Err(too_big((rows as u64).saturating_mul(columns as u64)));
        }
        if !send(Message::Opened(Some(rows as u64))) {
            return Ok(());
        }
        for (index, row) in range.rows().enumerate() {
            let cells = row.iter().map(cell_text).collect();
            let numeric = row.iter().map(is_number).collect();
            if !send(Message::Row(RawRow {
                line: index + 1,
                cells,
                numeric,
            })) {
                return Ok(());
            }
        }
    }
    send(Message::End);
    Ok(())
}

/// The sheet to read: the one named, or the first.
fn pick_sheet(names: &[String], wanted: Option<&str>, path: &str) -> Result<String, ImportError> {
    match wanted {
        Some(wanted) if names.iter().any(|name| name == wanted) => Ok(wanted.to_owned()),
        Some(wanted) => Err(ImportError::MissingSheet {
            path: path.to_owned(),
            sheet: wanted.to_owned(),
        }),
        None => names.first().cloned().ok_or_else(|| ImportError::NoSheets {
            path: path.to_owned(),
        }),
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

/// Whether the cell holds a number, as opposed to text that may look like one.
fn is_number(cell: &Data) -> bool {
    matches!(cell, Data::Int(_) | Data::Float(_))
}

/// A number cell's text the way Excel shows what it stored: a whole number in full, anything
/// else rounded to 15 significant digits with the trailing zeros gone. For a text column,
/// which would otherwise receive `0.30000000000000004`; text that is not a finite number is
/// returned as it is.
pub fn excel_digits(exact: &str) -> String {
    match exact.parse::<f64>() {
        Ok(value) if value.is_finite() && value.fract() != 0.0 => {
            // `{:e}` with 14 decimals is 15 significant digits, in a form that parses back.
            let rounded: f64 = format!("{value:.14e}").parse().unwrap_or(value);
            rounded.to_string()
        }
        _ => exact.to_owned(),
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

    #[test]
    fn a_float_cell_keeps_every_digit_the_file_stores() {
        assert_eq!(cell_text(&Data::Float(0.1 + 0.2)), "0.30000000000000004");
        assert_eq!(
            cell_text(&Data::Float(3.141592653589793)),
            "3.141592653589793"
        );
        assert_eq!(cell_text(&Data::Float(5.0)), "5");
        assert_eq!(cell_text(&Data::Float(-1234.5)), "-1234.5");
        assert_eq!(
            cell_text(&Data::Float(3_171_234_567_890_123.0)),
            "3171234567890123"
        );
        assert_eq!(cell_text(&Data::Float(1e300)).len(), 301);
    }

    #[test]
    fn a_text_column_gets_the_digits_excel_shows() {
        assert_eq!(excel_digits("0.30000000000000004"), "0.3");
        assert_eq!(excel_digits("3.141592653589793"), "3.14159265358979");
        assert_eq!(excel_digits("-1234.5"), "-1234.5");
        assert_eq!(excel_digits("5"), "5");
        assert_eq!(excel_digits("3171234567890123"), "3171234567890123");
        assert_eq!(excel_digits("hello"), "hello");
    }

    #[test]
    fn only_int_and_float_cells_are_numbers() {
        assert!(is_number(&Data::Int(1)));
        assert!(is_number(&Data::Float(1.5)));
        assert!(!is_number(&Data::String("1.5".to_owned())));
        assert!(!is_number(&Data::Empty));
        assert!(!is_number(&Data::Bool(true)));
    }
}
