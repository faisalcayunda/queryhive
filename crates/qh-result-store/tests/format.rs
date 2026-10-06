//! Format parity: the Rust `ColumnFormat` renderer against the Swift grid's `ColumnFormat.render`
//! (backlog B-21, blueprint section 11.4).
//!
//! `tests/fixtures/format.json` is written by `tests/fixtures/export_format.swift`, which runs the
//! real Swift code over a fixed corpus (the command is in that file). This test builds a one-cell
//! store for every case, renders it through `render_window` with the same format, and requires the
//! same text.
//!
//! Where the two differ, the case is listed in `DIVERGENCES` with the reason. A listed case must
//! still differ, and an unlisted one must match, so a divergence that is fixed or that appears is a
//! failure rather than a quiet change. The list is also the answer to "can `Json` leave
//! `StoreRows.swiftRenderedFormats`": while it holds `json` entries, no.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use qh_core::{ColumnBatch, ColumnMeta, Value};
use qh_result_store::{
    render_window, ColumnFormat, CowCell, Outcome, StoreConfig, StoreRegistry, WindowSpec,
};
use serde_json::Value as Json;

/// `QH_FORMAT_FIXTURE` points the test at another copy, which is how a mutated fixture proves the
/// comparison can fail without touching the committed file.
fn fixture_text() -> String {
    let path = std::env::var("QH_FORMAT_FIXTURE").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/format.json").to_owned()
    });
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// One difference between Swift and Rust. `OPEN` ones are defects of the Rust port.
struct Divergence {
    format: &'static str,
    value: String,
    reason: &'static str,
}

const EMPTY: &str =
    "OPEN: Foundation prints an empty array or object as `[\\n\\n<indent>]`, Rust as `[]`.";
const NUMBER: &str = "OPEN: Foundation prints a double with 17 significant digits (`0.10000000000000001`, \
    `1.4999999999999999e-07`, `1e+21`), an integral double without `.0`, `-0` as `0`, and a number a \
    double cannot hold exactly as written. serde_json prints the shortest double, which loses digits.";
const DUPLICATE: &str = "OPEN: Foundation keeps the first of two equal keys, serde_json the last.";
const ESCAPE: &str =
    "OPEN: Foundation writes `\\b` and `\\f`, Rust writes `\\u0008` and `\\u000c`.";
const KEY_ORDER: &str =
    "OPEN: Foundation orders keys like localizedStandardCompare (case folded, digits by \
    value), Rust by code point.";
const LENIENT: &str =
    "OPEN: Foundation accepts a trailing comma and a leading BOM, serde_json refuses both, \
    so Rust shows the stored text.";
const DEPTH: &str =
    "OPEN: Foundation parses 130 levels of nesting, serde_json stops at 128, so Rust shows \
    the stored text.";

/// Every difference. While a `json` entry is here, `Json` cannot leave
/// `StoreRows.swiftRenderedFormats` (B-21). Text, UUID and Unix timestamp have none.
fn divergences() -> Vec<Divergence> {
    let mut all = Vec::new();
    let mut group = |reason: &'static str, values: &[&str]| {
        all.extend(values.iter().map(|value| Divergence {
            format: "json",
            value: (*value).to_owned(),
            reason,
        }));
    };
    group(
        EMPTY,
        &[
            "[]",
            "{}",
            "[[]]",
            "[{}]",
            r#"{"a":[]}"#,
            r#"{"a":{}}"#,
            r#"[null,true,false,0,"",[],{}]"#,
        ],
    );
    group(
        NUMBER,
        &[
            "-0",
            "0.1",
            "1e3",
            "1E+2",
            "1.0",
            "100000000000000000000",
            "-9223372036854775809",
            "3.14159265358979323846",
            "1.5e-7",
            "1e-400",
            "0.1e1",
            "123456789.123456789",
            "1e-7",
            "5e-324",
            "4.9e-324",
            "1e2",
            "2.5E3",
            "[1.0,2.50,3e0]",
        ],
    );
    group(DUPLICATE, &[r#"{"a":1,"a":2}"#]);
    group(ESCAPE, &[r#""\n\r\t\b\f""#]);
    group(
        KEY_ORDER,
        &[
            r#"{"é":1,"e":2,"z":3,"Z":4,"a":5,"aa":6,"B":7,"b":8}"#,
            r#"{"a":1,"A":2,"_":3,"1":4,"10":5,"2":6,"":7," ":8}"#,
        ],
    );
    group(LENIENT, &["[1,]", r#"{"a":1,}"#, "\u{FEFF}{}"]);
    group(DEPTH, &[&format!("{}{}", "[".repeat(130), "]".repeat(130))]);
    all
}

/// `GridValue.isBinary`: the column types whose cells are bytes.
fn is_binary(type_name: &str) -> bool {
    let lowered = type_name.to_lowercase();
    ["bytea", "blob", "binary"]
        .iter()
        .any(|word| lowered.contains(word))
}

fn format_of(name: &str) -> ColumnFormat {
    match name {
        "raw" => ColumnFormat::Raw,
        other => ColumnFormat::parse(other),
    }
}

/// A binary column holds the bytes the hex stands for, as a driver stores them; anything else holds
/// the text itself.
fn cell_value(type_name: &str, value: &str) -> Value {
    if !is_binary(type_name) {
        return Value::Text(value.into());
    }
    let bytes = (0..value.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&value[at..at + 2], 16).expect("a binary case holds hex"))
        .collect();
    Value::Bytes(bytes)
}

/// What the Rust store shows for one cell under `format`, with no truncation.
fn render(
    registry: &Arc<StoreRegistry>,
    type_name: &str,
    value: &str,
    format: ColumnFormat,
) -> String {
    let handle = registry.create();
    let writer = handle.writer();
    writer
        .begin(vec![ColumnMeta::new("c", type_name)])
        .expect("begin");
    writer
        .push(&ColumnBatch::new(vec![vec![cell_value(type_name, value)]]).expect("a batch"))
        .expect("push");
    writer
        .finish(Outcome::Complete { truncated: false })
        .expect("finish");
    let shared = handle.shared();
    let reference = shared.chunk_refs().into_iter().next().expect("a chunk");
    let chunk = shared.load_chunk(reference.index).expect("load");
    let spec = WindowSpec {
        rows: vec![0],
        columns: vec![0],
        formats: vec![format],
        global_flags: 0,
    };
    let mut scratch = vec![CowCell::default(); 1];
    render_window(&chunk, &spec, &mut scratch, true)
        .expect("render")
        .cell_text(0, 0)
}

/// The case as a short, readable id.
fn describe(format: &str, type_name: &str, value: &str) -> String {
    let shown: String = value.chars().take(60).collect();
    let more = if shown.len() < value.len() {
        format!("... ({} bytes)", value.len())
    } else {
        String::new()
    };
    format!("{format} {type_name} {shown:?}{more}")
}

#[test]
fn rust_formats_match_the_swift_fixture() {
    let fixture: Json = serde_json::from_str(&fixture_text()).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert!(
        cases.len() > 200,
        "the fixture is suspiciously small: {}",
        cases.len()
    );

    let registry = StoreRegistry::for_test(StoreConfig {
        spill_dir: None,
        ..StoreConfig::default()
    });
    let all = divergences();
    let known: HashMap<(&str, &str), &Divergence> = all
        .iter()
        .map(|d| ((d.format, d.value.as_str()), d))
        .collect();

    let mut problems = Vec::new();
    let mut seen = HashSet::new();
    for case in cases {
        let format = case["format"].as_str().unwrap();
        let type_name = case["type"].as_str().unwrap();
        let value = case["value"].as_str().unwrap();
        let swift = case["rendered"].as_str().unwrap();
        let rust = render(&registry, type_name, value, format_of(format));
        let id = describe(format, type_name, value);
        match known.get(&(format, value)) {
            None if swift != rust => problems.push(format!(
                "UNLISTED {id}\n  swift {swift:?}\n  rust  {rust:?}"
            )),
            Some(d) => {
                seen.insert((format, value));
                if swift == rust {
                    problems.push(format!("STALE (now identical) {id}: {}", d.reason));
                }
            }
            None => {}
        }
    }
    if seen.len() != all.len() {
        problems.push(format!(
            "{} listed divergences name no fixture case",
            all.len() - seen.len()
        ));
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}
