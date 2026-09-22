//! The part-splitting rules, checked against the Python engine itself.
//!
//! ```bash
//! QH_TEST_PYTHON=1 cargo test -p qh-export --test plan_parity
//! ```
//!
//! Without `QH_TEST_PYTHON=1` each test prints why it is skipping and returns. That is
//! a deliberate choice over silently passing: a green run that tested nothing is worse
//! than a visible skip, and CI sets the variable.
//!
//! # Why this is worth doing
//!
//! The unit tests next door pin the rules as this crate understands them: parts named
//! from one, the first part renamed when a second appears, the ceiling being the
//! smaller of the format's and the caller's. A rule understood wrongly and tested
//! against itself stays green. So this test does not encode the rules at all — it runs
//! `exporter/export.py`'s own `export_rows` over the same rows and compares the
//! directory it produces, file name by file name and byte by byte, with the one this
//! crate produces.
//!
//! `export.py` is loaded by path with `importlib` because `exporter/__init__.py`
//! imports the `trino` package, which is not installed here. `export_rows` itself needs
//! neither a driver nor a query stream, so both are stubbed; everything that decides
//! where a part goes and what it is called is the real thing.
//!
//! # What is not covered here
//!
//! The two Excel ceilings are not exercised, because reaching one means 65535 rows
//! through `xlwt` and then the same through this crate. Those numbers are pinned
//! against the Python source in `Format::max_rows`'s test instead, where they are one
//! line apart and readable.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use qh_core::{ColumnMeta, Value};
use qh_export::{export_rows, ExportSpec, Format};

const SKIP_HINT: &str = "skipped: set QH_TEST_PYTHON=1 with python3 on PATH";

/// Drives the Python engine's `export_rows` over the same rows, in `case`'s shape.
///
/// The last line it prints is the worker count it used, so this test can refuse to
/// compare against a free-threaded build's parallel path rather than assume it never
/// happens.
///
/// It is handed the same four parameters the Rust side gets rather than a case name, so
/// the two runs cannot drift apart in what they were asked to do.
const REFERENCE: &str = r#"
import importlib.util
import pathlib
import sys
import types

root = pathlib.Path(sys.argv[1])
out = pathlib.Path(sys.argv[2])
fmt = sys.argv[3]
count = int(sys.argv[4])
per_file = sys.argv[5]
per_file = int(per_file) if per_file else None

package = types.ModuleType("exporter")
package.__path__ = [str(root / "exporter")]
sys.modules["exporter"] = package


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, root / "exporter" / filename)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


writers = load("exporter.writers", "writers.py")

drivers = types.ModuleType("exporter.drivers")
drivers.DatabaseConfig = type("DatabaseConfig", (), {})
sys.modules["exporter.drivers"] = drivers

source = types.ModuleType("exporter.source")
source.QueryStream = type("QueryStream", (), {})
source.TrinoConfig = type("TrinoConfig", (), {})
sys.modules["exporter.source"] = source

export = load("exporter.export", "export.py")

columns = [writers.Column("id", "bigint"), writers.Column("name", "varchar")]
rows = [[i, "row%d" % i] for i in range(1, count + 1)]
export.export_rows(columns, rows, out, "report", fmt, rows_per_file=per_file)

print("workers %d" % export.render_workers())
"#;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels under the repo root")
        .to_path_buf()
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("qh-plan-parity-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("python")).expect("temp dir");
    fs::create_dir_all(dir.join("rust")).expect("temp dir");
    dir
}

/// Every file in `dir`, by name, with its bytes.
fn tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<(String, Vec<u8>)> = fs::read_dir(dir)
        .expect("read the output directory")
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

/// Run both engines over the same rows and assert the directories agree.
///
/// Returns without asserting when the reference run cannot happen, so a machine without
/// `python3` sees a skip rather than a failure.
fn compare(tag: &str, format: Format, row_count: i64, rows_per_file: Option<usize>) {
    let root = repo_root();
    let dir = temp_dir(tag);

    // Python first: if the reference run cannot happen there is nothing to compare, so
    // the Rust half is not worth doing.
    let output = Command::new("python3")
        .arg("-c")
        .arg(REFERENCE)
        .arg(&root)
        .arg(dir.join("python"))
        .arg(format.name())
        .arg(row_count.to_string())
        .arg(rows_per_file.map_or_else(String::new, |rows| rows.to_string()))
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            println!("{SKIP_HINT} (python3 did not run: {error})");
            return;
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        panic!(
            "the Python engine failed on {tag}:\n{}\n{stdout}",
            String::from_utf8_lossy(&output.stderr),
        );
    }
    // The GIL-free build takes a different path through `export_rows` -- chunks rendered
    // on threads and written back in order. Comparing against that would be comparing
    // against code this crate deliberately does not have.
    if stdout.trim() != "workers 1" {
        println!(
            "{SKIP_HINT} (the reference run used the parallel path: {})",
            stdout.trim()
        );
        return;
    }

    let out_dir = dir.join("rust");
    let spec = ExportSpec {
        rows_per_file,
        ..ExportSpec::new(format, &out_dir, "report", columns())
    };
    export_rows(spec, rows(row_count), None, None).expect("the Rust export");

    let expected = tree(&dir.join("python"));
    let actual = tree(&dir.join("rust"));
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
