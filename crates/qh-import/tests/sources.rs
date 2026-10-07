//! The readers against real files: what a ragged CSV hands back, and what a workbook costs.
//!
//! The row-width rule itself (PF-4) lives in `qh-ffi`, where `ON_ERROR` does; what is
//! pinned here is the half this crate owns: that a row with the wrong number of fields
//! arrives with its fields as they were, rather than padded or cut on the way.

use std::path::{Path, PathBuf};

use qh_import::{Format as SourceFormat, ImportError, Options, RowReader};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("qh-import-sources-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir.join(name)
}

fn read_all(path: &Path, format: SourceFormat, options: &Options) -> Vec<(usize, Vec<String>)> {
    let mut reader = RowReader::open(path, format, options).expect("open");
    let mut rows = Vec::new();
    while let Some(row) = reader.next_row().expect("row") {
        rows.push((row.line, row.cells));
    }
    rows
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/offset_sheet.xlsx")
}

fn header() -> Options {
    Options {
        header: true,
        ..Options::default()
    }
}

#[test]
fn a_ragged_csv_row_arrives_with_the_fields_it_has() {
    let path = temp("ragged.csv");
    // Row 2 has a stray unquoted comma, row 3 is short, row 4 has a trailing delimiter.
    std::fs::write(&path, "id,name\n1,Ayu\n2,Budi, Jr\n3\n4,Dewi,\n").unwrap();
    let rows = read_all(&path, SourceFormat::Csv, &header());
    let widths: Vec<usize> = rows.iter().map(|(_, cells)| cells.len()).collect();
    assert_eq!(widths, vec![2, 3, 1, 3], "{rows:?}");
    assert_eq!(rows[1].1, vec!["2", "Budi", " Jr"]);
    assert_eq!(rows[3].1, vec!["4", "Dewi", ""]);
    let _ = std::fs::remove_file(&path);
}

fn fixture_named(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// `fixtures/many_rows.xlsx`: a header and 10,000 rows of 4 integers, with a declared dimension.
#[test]
fn a_workbook_streams_every_row_with_the_sheets_own_line_numbers() {
    let path = fixture_named("many_rows.xlsx");
    let mut reader = RowReader::open(&path, SourceFormat::Xlsx, &header()).expect("open");
    assert!(reader.streams());
    assert_eq!(reader.header().unwrap(), ["c0", "c1", "c2", "c3"]);
    assert_eq!(reader.rows_total(), Some(10_000));
    let (mut count, mut last) = (0, None);
    while let Some(row) = reader.next_row().expect("row") {
        if count == 0 {
            assert_eq!(row.line, 2, "the header is line 1");
            assert_eq!(row.cells, ["0", "1", "2", "3"]);
        }
        count += 1;
        last = Some(row);
    }
    assert_eq!(count, 10_000);
    let last = last.unwrap();
    assert_eq!(last.line, 10_001);
    assert_eq!(last.cells[3], (10_000 * 4 - 1).to_string());
}

#[test]
fn a_workbook_over_the_size_limit_is_refused_before_a_row_is_read() {
    let options = Options {
        header: true,
        xlsx_max_bytes: Some(100),
        ..Options::default()
    };
    let Err(error) = RowReader::open(
        &fixture_named("many_rows.xlsx"),
        SourceFormat::Xlsx,
        &options,
    ) else {
        panic!("a workbook over the byte limit opened");
    };
    assert!(matches!(error, ImportError::Limit { .. }), "{error}");
    assert!(
        error.to_string().contains("IMPORT_XLSX_MAX_BYTES"),
        "{error}"
    );
}

/// `fixtures/no_dimension.xlsx` has 101 rows of 10 cells and no `dimension` element, as some
/// generators write: the limit can only be met while reading, and fails the stream there.
#[test]
fn a_sheet_with_no_declared_size_is_stopped_by_the_cells_it_actually_has() {
    let options = Options {
        header: true,
        xlsx_max_cells: Some(500),
        ..Options::default()
    };
    let mut reader = RowReader::open(
        &fixture_named("no_dimension.xlsx"),
        SourceFormat::Xlsx,
        &options,
    )
    .expect("open");
    assert_eq!(reader.rows_total(), None);
    let error = loop {
        match reader.next_row() {
            Ok(Some(_)) => {}
            Ok(None) => panic!("the sheet ended under a limit it exceeds"),
            Err(error) => break error,
        }
    };
    assert!(matches!(error, ImportError::Limit { .. }), "{error}");
    assert!(
        error.to_string().contains("IMPORT_XLSX_MAX_CELLS"),
        "{error}"
    );
}

#[test]
fn a_sheet_with_no_declared_size_still_reads_whole_under_the_default_limit() {
    let rows = read_all(
        &fixture_named("no_dimension.xlsx"),
        SourceFormat::Xlsx,
        &header(),
    );
    assert_eq!(rows.len(), 100);
    assert_eq!(rows[0].1.len(), 10);
}

#[test]
fn a_missing_sheet_is_named_and_the_named_sheet_is_read() {
    let path = fixture();
    let options = |sheet: &str| Options {
        header: true,
        sheet: Some(sheet.to_owned()),
        ..Options::default()
    };
    let Err(error) = RowReader::open(&path, SourceFormat::Xlsx, &options("Nope")) else {
        panic!("a sheet that is not there opened");
    };
    assert!(matches!(error, ImportError::MissingSheet { .. }), "{error}");
    assert_eq!(
        read_all(&path, SourceFormat::Xlsx, &options("Data")).len(),
        3
    );
}

/// `fixtures/offset_sheet.xlsx` is written the way Excel writes a sheet whose data starts
/// at C3: a declared `dimension`, a blank row between rows, and a number stored with the
/// digits a float needs (`0.30000000000000004`).
#[test]
fn a_sheet_that_starts_at_c3_reads_from_its_own_first_cell() {
    let path = fixture();
    let mut reader = RowReader::open(&path, SourceFormat::Xlsx, &header()).expect("open");
    assert_eq!(reader.header().unwrap(), ["id", "name", "amount"]);
    assert_eq!(
        reader.rows_total(),
        Some(3),
        "the dimension C3:E6 minus the header"
    );
    let mut rows = Vec::new();
    while let Some(row) = reader.next_row().expect("row") {
        rows.push((row.line, row.cells));
    }
    assert_eq!(
        rows,
        vec![
            (4, vec!["".to_owned(), "".to_owned(), "".to_owned()]),
            (5, vec!["1".to_owned(), "Ayu".to_owned(), "0.3".to_owned()]),
            (6, vec!["2".to_owned(), "Budi".to_owned(), "".to_owned()]),
        ]
    );
}

#[test]
fn a_declared_dimension_over_the_limit_is_refused_at_open() {
    let path = fixture();
    let options = Options {
        header: true,
        xlsx_max_cells: Some(5),
        ..Options::default()
    };
    let Err(error) = RowReader::open(&path, SourceFormat::Xlsx, &options) else {
        panic!("a sheet declaring 12 cells opened under a limit of 5");
    };
    assert!(matches!(error, ImportError::Limit { .. }), "{error}");
}

/// `fixtures/locale_sheet.xlsx`: a number cell, a text cell that looks like a number, and a
/// float with 16 digits. The reader keeps every digit and says which cells were numbers.
#[test]
fn a_sheet_marks_its_number_cells_and_keeps_their_digits() {
    let path = fixture_named("locale_sheet.xlsx");
    let mut reader = RowReader::open(&path, SourceFormat::Xlsx, &header()).expect("open");
    let row = reader.next_row().expect("row").expect("a data row");
    assert_eq!(row.cells, vec!["1.234", "1.234,5", "3.141592653589793"]);
    assert_eq!(row.numeric, vec![true, false, true]);
    assert!(reader.next_row().expect("end").is_none());
}
