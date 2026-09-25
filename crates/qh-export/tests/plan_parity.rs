//! The part-splitting rules, checked against what the Python engine wrote.
//!
//! ```bash
//! cargo test -p qh-export --test plan_parity
//! ```
//!
//! # Why this is worth doing
//!
//! The unit tests next door pin the rules as this crate understands them: parts named
//! from one, the first part renamed when a second appears, the ceiling being the
//! smaller of the format's and the caller's. A rule understood wrongly and tested
//! against itself stays green. So this test does not encode the rules at all -- it
//! compares the directory this crate produces against one the Python engine produced
//! from the same rows, file name by file name and byte by byte.
//!
//! # Where the expected output comes from
//!
//! `tests/plan_parity/<tag>/` holds the Python engine's own `export_rows` output for
//! each case, recorded on 23 Sep 2026 while `exporter/export.py` still existed in this
//! tree. It was produced by loading `export.py` by path with `importlib` (its package
//! `__init__` imports `trino`, which is not installed here) and driving it with
//! `writers.Column("id", "bigint")`, `writers.Column("name", "varchar")` and rows
//! `[[i, "row%d" % i] for i in range(1, count + 1)]` -- the same columns and rows
//! [`columns`] and [`rows`] build below.
//!
//! The engine itself is gone in the same change that added these files, so the
//! recordings are what the comparison runs against now. They are deliberately frozen:
//! a rule that changes here should fail loudly, not quietly re-record. To rebuild them
//! the Python engine has to come back from history first, e.g.
//! `git show <commit>:exporter/export.py` and the same for `writers.py`.
//!
//! # What is not covered here
//!
//! The two Excel ceilings are not exercised, because reaching one means 65535 rows
//! through `xlwt` and then the same through this crate. Those numbers are pinned
//! against the Python source in `Format::max_rows`'s test instead, where they are one
//! line apart and readable.

use std::fs;
use std::path::{Path, PathBuf};

use qh_core::{ColumnMeta, Value};
use qh_export::{export_rows, ExportSpec, Format};

/// Where the recorded Python output for one case lives.
fn recorded(tag: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("plan_parity")
        .join(tag)
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("qh-plan-parity-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("rust")).expect("temp dir");
    dir
}

/// Every file in `dir`, by name, with its bytes.
fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("read {}: {error}", dir.display()))
        .map(|entry| {
            let path = entry.expect("directory entry").path();
            let name = path
                .file_name()
                .expect("a file name")
                .to_string_lossy()
                .into_owned();
            (name, fs::read(&path).expect("read the file"))
        })
        .collect();
    files.sort();
    files
}

fn columns() -> Vec<ColumnMeta> {
    vec![
        ColumnMeta::new("id", "bigint"),
        ColumnMeta::new("name", "varchar"),
    ]
}

fn rows(count: i64) -> Vec<Result<Vec<Value>, std::io::Error>> {
    (1..=count)
        .map(|id| {
            Ok(vec![
                Value::Int(id),
                Value::Text(format!("row{id}").into_boxed_str()),
            ])
        })
        .collect()
}

/// Run this crate over the case's rows and assert the directory matches the recording.
fn compare(tag: &str, format: Format, row_count: i64, rows_per_file: Option<usize>) {
    let expected_dir = recorded(tag);
    let expected = tree(&expected_dir);
    // Asserted before the run, so a case whose recording went missing is reported as
    // that rather than as a mysterious difference in what was written.
    assert!(
        !expected.is_empty(),
        "no recording at {}; was it deleted?",
        expected_dir.display()
    );

    let dir = temp_dir(tag);
    let out_dir = dir.join("rust");
    let spec = ExportSpec {
        rows_per_file,
        ..ExportSpec::new(format, &out_dir, "report", columns())
    };
    export_rows(spec, rows(row_count), None, None).expect("the Rust export");

    let actual = tree(&out_dir);
    let names = |files: &[(String, Vec<u8>)]| {
        files
            .iter()
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(&actual),
        names(&expected),
        "the parts are not named the same: {tag}"
    );
    for ((name, ours), (_, theirs)) in actual.iter().zip(expected.iter()) {
        assert_eq!(
            ours,
            theirs,
            "{name} differs from the Python engine's:\n ours: {}\ntheirs: {}",
            String::from_utf8_lossy(ours),
            String::from_utf8_lossy(theirs)
        );
    }
    // Asserted rather than assumed, so a comparison that silently produced nothing
    // cannot pass.
    assert!(!actual.is_empty(), "no files were written for {tag}");
    println!(
        "{tag}: {} part(s) match the Python engine byte for byte",
        actual.len()
    );
}

#[test]
fn a_split_export_matches_the_python_engine_file_for_file() {
    // 23 rows into 4-row parts: 6 parts, so the part-zero rename is exercised, and the
    // last part is a short one.
    compare("csv-split", Format::Csv, 23, Some(4));
}

#[test]
fn a_short_export_keeps_the_python_engines_single_file_name() {
    // The same rows with no ceiling: one file, under the name the user typed, and the
    // rename is never needed.
    compare("csv-short", Format::Csv, 23, None);
}

#[test]
fn an_empty_export_matches_the_python_engine() {
    compare("csv-empty", Format::Csv, 0, None);
}

#[test]
fn another_format_splits_the_same_way() {
    // text and csv share a writer; json and xml do not. This is the one that shows the
    // splitting does not depend on which writer is underneath.
    compare("text-split", Format::Text, 10, Some(3));
}

#[test]
fn a_ceiling_smaller_than_one_row_matches_the_python_engine() {
    // A `rows_per_file` of 1, which is the boundary case for the rename: every part
    // after the first is opened by the roll-over and nothing else.
    compare("csv-per-row", Format::Csv, 4, Some(1));
}
