//! One export, possibly several files.
//!
//! This is the Rust side of `export_rows` in `exporter/export.py:60-138`. A writer
//! knows how to put rows in a file; this module decides *how many* files and what
//! they are called.
//!
//! # When an export splits
//!
//! At a row ceiling: `rows_per_file` if the caller set one, otherwise the format's
//! own limit ([`Format::max_rows`] — 65535 rows for `xls`, 1048575 for `xlsx`). The
//! smaller of the two wins, so asking for 500-row files does not produce 65535-row
//! spreadsheets.
//!
//! A format with no ceiling and no `rows_per_file` writes one file of any length.
//! That is the common case and it is why the splitting state machine is not in the
//! streaming writers: they have no reason to know a second file might exist.
//!
//! # The first part is renamed, and that is not an accident
//!
//! While an export might still fit in one file it is called `report.csv`, because
//! that is the name the user asked for. The moment a second part is opened, the first
//! file is renamed to `report_part01.csv` and the new one becomes `report_part02.csv`.
//! A user who gets one file gets exactly the name they typed; a user who gets four
//! gets a numbered set with no part missing from the numbering. The alternative —
//! `report.csv`, `report_part02.csv`, `report_part03.csv` — reads as though
//! `part01` went missing.
//!
//! # Cancelling
//!
//! The row source is asked whether to stop before each row, never mid-row, and the
//! part being written is finished either way. So a cancelled export is a valid
//! file with the rows that arrived before the cancel and no half-written row at the
//! end — a partially written CSV with a torn last line would be a worse outcome than
//! the cancel itself.
//!
//! # What is not reproduced
//!
//! The Python engine has a second path that formats rows on worker threads
//! (`export.py:96`). It exists because formatting is pure Python and therefore holds
//! the GIL: without the GIL it does nothing, and `render_workers()` returns 1 on a
//! stock CPython build, so the default path there is the sequential one this module
//! implements. In Rust the same work is already tens of nanoseconds a row, so there
//! is nothing to route around. [`crate::render_rows`] stays public for the day a
//! profile says otherwise.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use qh_core::{ColumnMeta, Value};

use crate::{open, ExportError, ExportOptions, Format, Writer};

/// Rows between two `progress` calls. From the Python engine's `PROGRESS_EVERY`.
pub const PROGRESS_EVERY: u64 = 1_000;

/// Where part `index` goes.
///
/// Part zero is the plain `<basename>.<ext>`; later parts are numbered from one in
/// their name, so `_part02` is the second file. The numbering is not a detail to
/// improve on: it is what the Python engine produced and therefore what a script
/// already globbing `report_part*.csv` expects.
pub fn part_path(out_dir: &Path, basename: &str, extension: &str, index: usize) -> PathBuf {
    if index == 0 {
        out_dir.join(format!("{basename}.{extension}"))
    } else {
        out_dir.join(format!("{basename}_part{:02}.{extension}", index + 1))
    }
}

/// The row ceiling for one part: the smaller of the format's limit and the caller's.
///
/// Zero is treated as "no limit" rather than "one empty file per row", because a
/// `rows_per_file` of 0 arriving from a dialog field someone cleared means "don't
/// split", not a division by zero. The Python engine filters falsy values here for
/// the same reason.
fn row_limit(format: Format, rows_per_file: Option<usize>) -> Option<usize> {
    [format.max_rows(), rows_per_file]
        .into_iter()
        .flatten()
        .filter(|n| *n > 0)
        .min()
}

/// Everything an export needs that is not a row.
///
/// One struct rather than a row of positional arguments, because six of them in a row
/// is six chances to pass the basename where the format was meant. Built with
/// [`ExportSpec::new`] for the common case and struct-update syntax for the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportSpec<'a> {
    pub format: Format,
    /// Where the parts go. Created if it is not there.
    pub out_dir: &'a Path,
    /// The file name without its extension: `report` gives `report.csv`, or
    /// `report_part01.csv` and `report_part02.csv` if the export splits.
    pub basename: &'a str,
    pub columns: Vec<ColumnMeta>,
    pub options: ExportOptions,
    /// Data rows per file. `None` splits on the format's own ceiling alone, and a
    /// `Some(0)` means the same thing — a cleared dialog field is not a request for
    /// one row per file.
    pub rows_per_file: Option<usize>,
}

impl<'a> ExportSpec<'a> {
    /// The shape most callers want: default options, no caller-imposed split.
    pub fn new(
        format: Format,
        out_dir: &'a Path,
        basename: &'a str,
        columns: Vec<ColumnMeta>,
    ) -> Self {
        Self {
            format,
            out_dir,
            basename,
            columns,
            options: ExportOptions::default(),
            rows_per_file: None,
        }
    }
}

/// An export in progress: the open writer, the part it belongs to, and the count.
///
/// Write rows with [`Exporter::write_row`], then call [`Exporter::finish`]. Dropping
/// without finishing leaves the file missing its trailer, so a caller that can fail
/// between the two should reach for [`export_rows`] instead, which does it for them.
pub struct Exporter {
    format: Format,
    out_dir: PathBuf,
    basename: String,
    options: ExportOptions,
    columns: Vec<ColumnMeta>,
    /// Data rows per part, or `None` for a single file.
    limit: Option<usize>,
    /// The part being written, and its index in `files`.
    part: usize,
    in_part: usize,
    writer: Option<Box<dyn Writer>>,
    /// Every file this export has produced or will produce, in part order. Lazy:
    /// only the parts opened so far are here.
    files: Vec<PathBuf>,
    rows: u64,
    warnings: Vec<String>,
    cancelled: bool,
    finished: bool,
}

impl Exporter {
    /// Open an export, creating `out_dir` and the first part.
    ///
    /// The first part is opened immediately, so an empty result set still leaves a
    /// header-only file — which is what the Python engine does, and a caller that
    /// showed the user a save panel has promised them a file.
    pub fn new(spec: ExportSpec<'_>) -> Result<Self, ExportError> {
        let out_dir = spec.out_dir.to_path_buf();
        fs::create_dir_all(&out_dir)?;

        let mut exporter = Self {
            format: spec.format,
            out_dir,
            basename: spec.basename.to_owned(),
            options: spec.options,
            columns: spec.columns,
            limit: row_limit(spec.format, spec.rows_per_file),
            part: 0,
            in_part: 0,
            writer: None,
            files: Vec::new(),
            rows: 0,
            warnings: Vec::new(),
            cancelled: false,
            finished: false,
        };
        exporter.open_part(0)?;
        Ok(exporter)
    }

    /// The format being written, as a reminder of what the ceiling came from.
    pub fn format(&self) -> Format {
        self.format
    }

    /// Every file written so far, in part order.
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    /// Data rows written across all parts. The header is not a row.
    pub fn rows(&self) -> u64 {
        self.rows
    }

    /// Notice a value was cut to fit its field.
    ///
    /// Collected per export rather than logged, because the only party who can decide
    /// whether a truncated value mattered is the user reading the file.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn cancelled(&self) -> bool {
        self.cancelled
    }

    /// Stop after the current part, keeping what has been written.
    pub fn mark_cancelled(&mut self) {
        self.cancelled = true;
    }

    /// Write one row, opening the next part first if this one is full.
    pub fn write_row(&mut self, row: &[Value]) -> Result<(), ExportError> {
        if self.limit.is_some_and(|limit| self.in_part >= limit) {
            self.roll_over()?;
        }
        match self.writer.as_mut() {
            Some(writer) => writer.write_row(row)?,
            // Unreachable: a part is open from `new` until `finish`.
            None => {
                return Err(ExportError::Usage {
                    message: "this export has already been finished".to_owned(),
                })
            }
        }
        self.in_part += 1;
        self.rows += 1;
        Ok(())
    }

    /// Close the current part. Safe to call twice.
    ///
    /// Nothing opens a new part afterwards, so this is the end of the export.
    pub fn finish(&mut self) -> Result<(), ExportError> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        self.close_part()
    }

    /// Close the part that is open and record anything the writer wants the user to know.
    fn close_part(&mut self) -> Result<(), ExportError> {
        let Some(mut writer) = self.writer.take() else {
            return Ok(());
        };
        writer.finish()?;
        let truncated = writer.truncated();
        if truncated > 0 {
            // The same sentence the Python engine shows, naming the file it happened
            // in, because a warning that does not say where to look is not a warning.
            let name = self
                .files
                .last()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.basename.clone());
            self.warnings.push(format!(
                "{name}: {truncated} value(s) truncated to the dbf field width"
            ));
        }
        Ok(())
    }

    /// Close the full part and start the next, renaming the first on the way.
    fn roll_over(&mut self) -> Result<(), ExportError> {
        // Closed before it is renamed: a rename over an open handle is fine on Unix
        // and not fine on Windows, and this crate does not get to assume which.
        self.close_part()?;
        if self.part == 0 {
            let renamed = part_path(&self.out_dir, &self.basename, self.format.extension(), 1)
                .with_file_name(format!(
                    "{}_part01.{}",
                    self.basename,
                    self.format.extension()
                ));
            fs::rename(&self.files[0], &renamed)?;
            self.files[0] = renamed;
        }
        self.part += 1;
        self.in_part = 0;
        self.open_part(self.part)
    }

    fn open_part(&mut self, index: usize) -> Result<(), ExportError> {
        let path = part_path(
            &self.out_dir,
            &self.basename,
            self.format.extension(),
            index,
        );
        let writer = open(self.format, &path, &self.columns, &self.options)?;
        self.writer = Some(writer);
        self.files.push(path);
        Ok(())
    }

    fn into_outcome(self) -> ExportOutcome {
        ExportOutcome {
            files: self.files,
            rows: self.rows,
            columns: self.columns,
            warnings: self.warnings,
            cancelled: self.cancelled,
        }
    }
}

/// Written out rather than derived: the open writer is a `dyn Writer` and is not
/// `Debug`, and a caller printing an export in progress wants its file names and
/// count, not whatever a writer would say about itself.
impl std::fmt::Debug for Exporter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Exporter")
            .field("format", &self.format)
            .field("files", &self.files)
            .field("columns", &self.columns.len())
            .field("rows", &self.rows)
            .field("limit", &self.limit)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

/// What an export produced.
#[derive(Debug, Clone, PartialEq)]
pub struct ExportOutcome {
    /// The files, in part order. One entry unless the export split.
    pub files: Vec<PathBuf>,
    /// Data rows written, summed over the parts.
    pub rows: u64,
    /// The columns every part was written with.
    pub columns: Vec<ColumnMeta>,
    /// Things the user should be told, such as a value cut to a `dbf` field width.
    pub warnings: Vec<String>,
    /// Whether the row source was cut short. `files` holds valid files either way.
    pub cancelled: bool,
}

/// Write rows into one or more files, finishing them however that goes.
///
/// `rows` yields one row at a time and may fail partway — a query that dies after
/// 900,000 rows leaves a finished part holding 900,000 rows and returns the error,
/// rather than a file whose trailer was never written.
///
/// `cancel` is asked before each row is taken, and a `true` ends the export with
/// [`ExportOutcome::cancelled`] set and the rows so far kept.
pub fn export_rows<E, I>(
    spec: ExportSpec<'_>,
    rows: I,
    progress: Option<&mut dyn FnMut(u64)>,
    cancel: Option<&dyn Fn() -> bool>,
) -> Result<ExportOutcome, ExportError>
where
    I: IntoIterator<Item = Result<Vec<Value>, E>>,
    E: Error + Send + Sync + 'static,
{
    let mut exporter = Exporter::new(spec)?;
    let written = feed(&mut exporter, rows, progress, cancel);
    let finished = exporter.finish();
    // A close that failed is the more urgent error: the loop's error has already
    // stopped the export, and this one means the file on disk is not complete.
    finished?;
    written?;
    Ok(exporter.into_outcome())
}

/// The row loop, split out so that the caller above can finish the file whether this
/// returns an error or not.
///
/// `'p` is written out rather than elided on purpose: left implicit, the trait object
/// would borrow for as long as the caller's own `&mut`, which then cannot be reborrowed
/// for the length of the loop.
fn feed<'p, E, I>(
    exporter: &mut Exporter,
    rows: I,
    mut progress: Option<&mut (dyn FnMut(u64) + 'p)>,
    cancel: Option<&dyn Fn() -> bool>,
) -> Result<(), ExportError>
where
    I: IntoIterator<Item = Result<Vec<Value>, E>>,
    E: Error + Send + Sync + 'static,
{
    let mut rows = rows.into_iter();
    loop {
        // Asked before the next row is taken, so the row that arrives while the user
        // is clicking Cancel is not half-written into the file.
        if cancel.is_some_and(|cancel| cancel()) {
            exporter.mark_cancelled();
            break;
        }
        let Some(row) = rows.next() else { break };
        let row = row.map_err(ExportError::source)?;
        exporter.write_row(&row)?;
        if let Some(report) = progress.as_mut() {
            if exporter.rows() % PROGRESS_EVERY == 0 {
                report(exporter.rows());
            }
        }
    }
    if let Some(report) = progress.as_mut() {
        report(exporter.rows());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use qh_core::Value;

    /// A temp directory per test, so parallel tests cannot truncate each other's
    /// files. The name is unique per process and per call; nothing is deleted, which
    /// is what lets a failure be inspected afterwards.
    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("qh-plan-{}-{tag}-{seq}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn columns() -> Vec<ColumnMeta> {
        vec![
            ColumnMeta::new("id", "bigint"),
            ColumnMeta::new("name", "varchar"),
        ]
    }

    /// The default options, csv, and no caller-imposed split: the shape most of these
    /// tests vary one field of.
    fn spec<'a>(dir: &'a Path, basename: &'a str) -> ExportSpec<'a> {
        ExportSpec::new(Format::Csv, dir, basename, columns())
    }

    fn row(id: i64) -> Vec<Value> {
        vec![
            Value::Int(id),
            Value::Text(format!("row{id}").into_boxed_str()),
        ]
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn names(outcome: &ExportOutcome) -> Vec<String> {
        outcome
            .files
            .iter()
            .map(|path| {
                path.file_name()
                    .expect("a file name")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    /// The rows of one part, header excluded.
    fn data_lines(text: &str) -> Vec<&str> {
        let mut lines = text.lines().filter(|line| !line.is_empty());
        lines.next(); // header
        lines.collect()
    }

    #[test]
    fn part_names_are_the_python_engines() {
        let dir = Path::new("/out");
        // Part zero is the name the user typed, before it is known a split happens.
        assert_eq!(
            part_path(dir, "report", "csv", 0),
            PathBuf::from("/out/report.csv")
        );
        // Numbered from one, two digits wide.
        assert_eq!(
            part_path(dir, "report", "csv", 1),
            PathBuf::from("/out/report_part02.csv")
        );
        assert_eq!(
            part_path(dir, "report", "csv", 98),
            PathBuf::from("/out/report_part99.csv")
        );
        // Past two digits it grows rather than wrapping, because `{:02}` is a
        // minimum width and not a mask.
        assert_eq!(
            part_path(dir, "report", "csv", 99),
            PathBuf::from("/out/report_part100.csv")
        );
    }

    #[test]
    fn the_ceiling_is_the_smaller_of_the_format_and_the_caller() {
        // A format's own limit stands on its own.
        assert_eq!(row_limit(Format::Xls, None), Some(65_535));
        assert_eq!(row_limit(Format::Xlsx, None), Some(1_048_575));
        // A format with no ceiling splits only when asked to.
        assert_eq!(row_limit(Format::Csv, None), None);
        assert_eq!(row_limit(Format::Csv, Some(500)), Some(500));
        // The smaller wins, so a request for 500-row files does not produce
        // 65535-row spreadsheets.
        assert_eq!(row_limit(Format::Xls, Some(500)), Some(500));
        assert_eq!(row_limit(Format::Xls, Some(70_000)), Some(65_535));
        // Zero from a cleared dialog field means "do not split", not "one row each".
        assert_eq!(row_limit(Format::Csv, Some(0)), None);
        assert_eq!(row_limit(Format::Xls, Some(0)), Some(65_535));
    }

    #[test]
    fn one_file_keeps_the_name_the_user_typed() {
        let dir = temp_dir("one-file");
        let outcome = export_rows(
            ExportSpec {
                rows_per_file: Some(5),
                ..spec(&dir, "report")
            },
            (1..=3).map(|id| Ok::<_, std::io::Error>(row(id))),
            None,
            None,
        )
        .expect("export");

        assert_eq!(outcome.rows, 3);
        assert_eq!(names(&outcome), ["report.csv"]);
        assert!(!outcome.cancelled);
        assert!(outcome.warnings.is_empty());
        // And no `_part01` file was left behind by the rename.
        assert!(!dir.join("report_part01.csv").exists());
    }

    #[test]
    fn a_second_part_renames_the_first_and_numbers_from_one() {
        let dir = temp_dir("split");
        let outcome = export_rows(
            ExportSpec {
                rows_per_file: Some(3),
                ..spec(&dir, "report")
            },
            (1..=10).map(|id| Ok::<_, std::io::Error>(row(id))),
            None,
            None,
        )
        .expect("export");

        assert_eq!(outcome.rows, 10);
        assert_eq!(
            names(&outcome),
            [
                "report_part01.csv",
                "report_part02.csv",
                "report_part03.csv",
                "report_part04.csv"
            ]
        );
        // The un-suffixed name is gone: it is the same file, renamed, not a fifth one.
        assert!(!dir.join("report.csv").exists());

        // 3 + 3 + 3 + 1, in order, with each part carrying its own header.
        let counts: Vec<usize> = outcome
            .files
            .iter()
            .map(|path| data_lines(&read(path)).len())
            .collect();
        assert_eq!(counts, [3, 3, 3, 1]);
        assert_eq!(
            data_lines(&read(&outcome.files[0])),
            ["1,row1", "2,row2", "3,row3"]
        );
        assert_eq!(data_lines(&read(&outcome.files[3])), ["10,row10"]);
    }

    #[test]
    fn a_format_with_no_ceiling_writes_one_file_however_many_rows() {
        let dir = temp_dir("no-ceiling");
        let outcome = export_rows(
            ExportSpec {
                rows_per_file: None,
                ..spec(&dir, "big")
            },
            (1..=2_500).map(|id| Ok::<_, std::io::Error>(row(id))),
            None,
            None,
        )
        .expect("export");

        assert_eq!(outcome.rows, 2_500);
        assert_eq!(names(&outcome), ["big.csv"]);
        assert_eq!(data_lines(&read(&outcome.files[0])).len(), 2_500);
    }

    #[test]
    fn an_empty_result_still_leaves_a_header_only_file() {
        // The save panel already promised the user a file.
        let dir = temp_dir("empty");
        let outcome = export_rows(
            ExportSpec {
                rows_per_file: None,
                ..spec(&dir, "report")
            },
            std::iter::empty::<Result<Vec<Value>, std::io::Error>>(),
            None,
            None,
        )
        .expect("export");

        assert_eq!(outcome.rows, 0);
        assert_eq!(names(&outcome), ["report.csv"]);
        assert_eq!(read(&outcome.files[0]), "id,name\r\n");
    }

    #[test]
    fn a_cancel_keeps_the_rows_already_written_and_closes_the_file() {
        let dir = temp_dir("cancel");
        let cancelled_after = std::cell::Cell::new(0u32);
        let cancel = || {
            let seen = cancelled_after.get();
            cancelled_after.set(seen + 1);
            // False for the first four asks (four rows written), then true. The check
            // happens before each row is taken, so this writes exactly four rows.
            seen >= 4
        };

        let outcome = export_rows(
            ExportSpec {
                rows_per_file: None,
                ..spec(&dir, "report")
            },
            (1..=10).map(|id| Ok::<_, std::io::Error>(row(id))),
            None,
            Some(&cancel),
        )
        .expect("export");

        assert!(outcome.cancelled);
        assert_eq!(outcome.rows, 4);
        // Closed, not abandoned: the last line is complete and there is no fifth row.
        assert_eq!(
            read(&outcome.files[0]),
            "id,name\r\n1,row1\r\n2,row2\r\n3,row3\r\n4,row4\r\n"
        );
    }

    #[test]
    fn a_cancel_at_a_part_boundary_opens_no_fifth_file() {
        let dir = temp_dir("cancel-boundary");
        let asks = std::cell::Cell::new(0u32);
        let cancel = || {
            let seen = asks.get();
            asks.set(seen + 1);
            seen >= 6 // six rows written: two full parts of three
        };

        let outcome = export_rows(
            ExportSpec {
                rows_per_file: Some(3),
                ..spec(&dir, "report")
            },
            (1..=10).map(|id| Ok::<_, std::io::Error>(row(id))),
            None,
            Some(&cancel),
        )
        .expect("export");

        assert!(outcome.cancelled);
        assert_eq!(outcome.rows, 6);
        // The cancel is seen before the roll-over check, so no empty part03 is opened.
        assert_eq!(names(&outcome), ["report_part01.csv", "report_part02.csv"]);
        assert_eq!(
            data_lines(&read(&outcome.files[1])),
            ["4,row4", "5,row5", "6,row6"]
        );
    }

    #[test]
    fn a_source_that_dies_mid_stream_still_leaves_a_finished_file() {
        // A torn file is the failure mode worth a test: the rows that did arrive have
        // to be readable, and the error still has to come back.
        let dir = temp_dir("source-error");
        let rows: Vec<Result<Vec<Value>, std::io::Error>> = vec![
            Ok(row(1)),
            Ok(row(2)),
            Err(std::io::Error::other("connection reset")),
            Ok(row(4)),
        ];

        let outcome = export_rows(
            ExportSpec {
                rows_per_file: None,
                ..spec(&dir, "report")
            },
            rows,
            None,
            None,
        );

        let error = outcome.expect_err("the failure must come back, not be swallowed");
        assert!(error.to_string().contains("connection reset"), "{error}");
        // The rows before the failure, and the file closed properly behind them.
        assert_eq!(
            read(&dir.join("report.csv")),
            "id,name\r\n1,row1\r\n2,row2\r\n"
        );
    }

    #[test]
    fn progress_is_reported_every_thousand_rows_and_once_at_the_end() {
        let dir = temp_dir("progress");
        let mut seen: Vec<u64> = Vec::new();
        let mut report = |rows: u64| seen.push(rows);

        export_rows(
            ExportSpec {
                rows_per_file: None,
                ..spec(&dir, "report")
            },
            (1..=2_500).map(|id| Ok::<_, std::io::Error>(row(id))),
            Some(&mut report),
            None,
        )
        .expect("export");

        // Every thousand, then the final count — which is not on a boundary and would
        // otherwise never be reported.
        assert_eq!(seen, [1_000, 2_000, 2_500]);
    }

    #[test]
    fn a_truncating_writer_reports_it_against_the_file_it_happened_in() {
        // dbf text fields are fixed width. Two values longer than the field, and the
        // message has to name the file and count them, as the Python engine does.
        let dir = temp_dir("truncation");
        let options = ExportOptions {
            dbf_char_width: 4,
            ..ExportOptions::default()
        };
        let rows = vec![
            vec![Value::Int(1), Value::Text("far too long".into())],
            vec![Value::Int(2), Value::Text("also too long".into())],
        ];

        let outcome = export_rows(
            ExportSpec {
                format: Format::Dbf,
                options,
                ..spec(&dir, "report")
            },
            rows.into_iter().map(Ok::<_, std::io::Error>),
            None,
            None,
        )
        .expect("export");

        assert_eq!(
            outcome.warnings,
            ["report.dbf: 2 value(s) truncated to the dbf field width"]
        );
    }

    #[test]
    fn writing_after_finishing_is_refused_rather_than_silently_dropped() {
        let dir = temp_dir("after-finish");
        let mut exporter = Exporter::new(spec(&dir, "report")).expect("open");
        exporter.write_row(&row(1)).expect("first row");
        exporter.finish().expect("finish");
        // Twice is not an error: the caller may be in a `finally`-shaped path.
        exporter.finish().expect("finish is idempotent");

        let error = exporter.write_row(&row(2)).expect_err("must be refused");
        assert!(
            error.to_string().contains("already been finished"),
            "{error}"
        );
        assert_eq!(read(&dir.join("report.csv")), "id,name\r\n1,row1\r\n");
    }

    #[test]
    fn a_format_without_a_writer_fails_before_any_file_is_created() {
        // The guard in `open`, reached through this module so the ordering is pinned:
        // no empty file is left behind for a format that cannot be written.
        let dir = temp_dir("unsupported");
        let error = Exporter::new(ExportSpec::new(Format::Csv, &dir, "report", Vec::new()))
            .expect_err("an export with no columns has no shape");
        assert!(error.to_string().contains("column"), "{error}");
        assert_eq!(fs::read_dir(&dir).expect("dir").count(), 0);
    }
}
