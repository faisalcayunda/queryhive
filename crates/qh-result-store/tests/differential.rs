//! Differential test: the Rust view against the Swift grid (blueprint §13, owner decision O-9).
//!
//! `tests/fixtures/differential.json` is written by the Swift test `SortFixtureExport`, which runs
//! the real `GridSort`, `ColumnFilter` and `GridSearch` over a fixed corpus. This test builds a
//! store from the same corpus, applies the same operations through the view, and requires the same
//! source rows in the same order.
//!
//! Where the two legitimately differ, the case is named in `DIVERGENCES` with the exact rows Rust
//! returns and the reason. A listed case must still differ from Swift and still produce exactly
//! those rows, so a divergence that goes away, or grows, fails the test instead of hiding.

use std::collections::HashMap;
use std::sync::Arc;

use qh_core::{ColumnBatch, ColumnMeta, Value};
use qh_result_store::{FilterSpec, Outcome, StoreConfig, StoreHandle, StoreRegistry, ViewSpec};
use serde_json::Value as Json;

/// `QH_DIFFERENTIAL_FIXTURE` points the test at another copy, which is how a mutated fixture proves
/// the comparison can fail without touching the committed file.
fn fixture_text() -> String {
    let path = std::env::var("QH_DIFFERENTIAL_FIXTURE").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/differential.json"
        )
        .to_owned()
    });
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// One accepted difference between Swift and Rust.
struct Divergence {
    /// The case id, as `describe_*` spells it.
    case: &'static str,
    /// The rows Rust returns for it.
    rust: &'static [usize],
    reason: &'static str,
}

// Every entry is ACCEPTED: a difference the blueprint or owner decision O-9 accepts. A Rust
// deviation from a line-by-line port is a defect to fix, not an entry to add.

const NATURAL: &str =
    "ACCEPTED (O-9): text order is the natural key, not localizedStandardCompare. \
    Digraphs (U+01C5, U+01C6), dotless i, I with dot above, the fi ligature and Hangul \
    jamo order differently from ICU.";

const NUMERAL: &str =
    "ACCEPTED (O-9): Foundation reads Arabic-indic and fullwidth digits as numbers \
    when it orders text, so U+0663 and the fullwidth 12 sort by value among the text cells; the \
    natural key puts them after the ASCII-digit text. Neither is a number to GridSort.number.";

const CLUSTER: &str = "ACCEPTED: ci_contains folds then searches bytes, so 'i' matches inside the \
    folded 'I with dot above' (i + U+0307) and the fi ligature (fi); Foundation matches whole \
    grapheme clusters and does not.";

/// Every accepted difference, one row per fixture case. O-9: the natural key is not
/// `localizedStandardCompare`, and a few Foundation behaviours have no Rust equivalent.
const DIVERGENCES: &[Divergence] = &[
    Divergence {
        case: "sort column=1 descending=false",
        rust: &[
            29, 17, 19, 11, 34, 27, 35, 6, 13, 36, 10, 12, 7, 30, 23, 0, 24, 1, 18, 28, 9, 33, 4,
            22, 2, 25, 8, 31, 14, 5, 39, 20, 37, 16, 3, 26, 38, 21, 15, 32,
        ],
        reason: NUMERAL,
    },
    Divergence {
        case: "sort column=1 descending=true",
        rust: &[
            15, 32, 21, 38, 26, 3, 16, 37, 20, 39, 5, 14, 31, 8, 25, 2, 22, 4, 33, 9, 28, 18, 1, 0,
            24, 23, 7, 30, 10, 12, 13, 36, 6, 35, 27, 11, 34, 19, 17, 29,
        ],
        reason: NUMERAL,
    },
    Divergence {
        case: "sort column=2 descending=false",
        rust: &[
            7, 31, 20, 3, 32, 21, 10, 28, 39, 14, 27, 16, 25, 36, 35, 24, 11, 0, 33, 22, 17, 6, 5,
            1, 30, 13, 26, 15, 19, 8, 37, 2, 4, 29, 12, 23, 9, 38, 34, 18,
        ],
        reason: NATURAL,
    },
    Divergence {
        case: "sort column=2 descending=true",
        rust: &[
            18, 34, 38, 9, 23, 12, 29, 4, 2, 37, 8, 19, 15, 26, 13, 30, 1, 5, 6, 17, 22, 33, 0, 11,
            24, 35, 36, 25, 16, 27, 14, 39, 28, 10, 21, 32, 3, 20, 31, 7,
        ],
        reason: NATURAL,
    },
    Divergence {
        case: "filter column=2 needle=\"i\"",
        rust: &[1, 5, 13, 20, 30, 34],
        reason: CLUSTER,
    },
    Divergence {
        case: "search term=\"i\"",
        rust: &[
            0, 1, 2, 3, 4, 5, 7, 8, 13, 16, 20, 23, 30, 31, 32, 34, 38, 39,
        ],
        reason: CLUSTER,
    },
    Divergence {
        case: "pipeline filter=1:\"<10\" search=\"\" sort=2:true",
        rust: &[
            34, 9, 23, 12, 29, 4, 19, 13, 30, 6, 17, 11, 35, 36, 27, 10, 7,
        ],
        reason: NATURAL,
    },
];

fn cell(json: &Json) -> Option<String> {
    json.as_str().map(str::to_owned)
}

fn indices(json: &Json) -> Vec<usize> {
    json.as_array()
        .expect("an array of row indices")
        .iter()
        .map(|n| n.as_u64().expect("a row index") as usize)
        .collect()
}

fn corpus(fixture: &Json) -> StoreHandle {
    let names: Vec<String> = fixture["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_owned())
        .collect();
    let rows = fixture["rows"].as_array().unwrap();
    let columns: Vec<Vec<Value>> = (0..names.len())
        .map(|column| {
            rows.iter()
                .map(|row| match cell(&row[column]) {
                    Some(text) => Value::Text(text.into()),
                    None => Value::Null,
                })
                .collect()
        })
        .collect();
    let registry: Arc<StoreRegistry> = StoreRegistry::for_test(StoreConfig {
        spill_dir: None,
        ..StoreConfig::default()
    });
    let handle = registry.create();
    let writer = handle.writer();
    writer
        .begin(
            names
                .iter()
                .map(|n| ColumnMeta::new(n.as_str(), "text"))
                .collect(),
        )
        .unwrap();
    writer.push(&ColumnBatch::new(columns).unwrap()).unwrap();
    writer
        .finish(Outcome::Complete { truncated: false })
        .unwrap();
    handle
}

fn run(store: &StoreHandle, spec: ViewSpec) -> Vec<usize> {
    store.shared().set_view(spec).unwrap();
    let view = store.shared().current_view().unwrap();
    view.source_rows(0..view.row_count())
        .unwrap()
        .into_iter()
        .map(|row| row as usize)
        .collect()
}

fn filter_spec(case: &Json) -> FilterSpec {
    let column = case["column"].as_u64().unwrap() as usize;
    match case["kind"].as_str().unwrap() {
        "text" => FilterSpec::Text {
            column,
            needle: case["needle"].as_str().unwrap().to_owned(),
        },
        _ => FilterSpec::Values {
            column,
            values: case["values"]
                .as_array()
                .unwrap()
                .iter()
                .map(cell)
                .collect(),
        },
    }
}

/// Every case, with its id, the Swift answer, and the Rust answer.
fn cases() -> Vec<(String, Vec<usize>, Vec<usize>)> {
    let fixture: Json = serde_json::from_str(&fixture_text()).unwrap();
    let store = corpus(&fixture);
    let mut out = Vec::new();

    for case in fixture["sorts"].as_array().unwrap() {
        let column = case["column"].as_u64().unwrap() as usize;
        let descending = case["descending"].as_bool().unwrap();
        let rust = run(
            &store,
            ViewSpec {
                sort: Some((column, descending)),
                ..ViewSpec::default()
            },
        );
        out.push((
            format!("sort column={column} descending={descending}"),
            indices(&case["order"]),
            rust,
        ));
    }
    for case in fixture["filters"].as_array().unwrap() {
        let rust = run(
            &store,
            ViewSpec {
                filters: vec![filter_spec(case)],
                ..ViewSpec::default()
            },
        );
        let id = match case["kind"].as_str().unwrap() {
            "text" => format!(
                "filter column={} needle={:?}",
                case["column"],
                case["needle"].as_str().unwrap()
            ),
            _ => format!("filter column={} values={}", case["column"], case["values"]),
        };
        out.push((id, indices(&case["rows"]), rust));
    }
    for case in fixture["searches"].as_array().unwrap() {
        let term = case["term"].as_str().unwrap();
        let rust = run(
            &store,
            ViewSpec {
                search: term.to_owned(),
                ..ViewSpec::default()
            },
        );
        out.push((
            format!("search term={term:?}"),
            indices(&case["rows"]),
            rust,
        ));
    }
    for case in fixture["pipelines"].as_array().unwrap() {
        let filter = Json::from_iter([
            ("kind".to_owned(), Json::from("text")),
            ("column".to_owned(), case["filter"]["column"].clone()),
            ("needle".to_owned(), case["filter"]["needle"].clone()),
        ]);
        let sort = &case["sort"];
        let sort_column = sort["column"].as_u64().unwrap() as usize;
        let descending = sort["descending"].as_bool().unwrap();
        let term = case["search"].as_str().unwrap();
        let rust = run(
            &store,
            ViewSpec {
                sort: Some((sort_column, descending)),
                filters: vec![filter_spec(&filter)],
                search: term.to_owned(),
            },
        );
        out.push((
            format!(
                "pipeline filter={}:{} search={term:?} sort={sort_column}:{descending}",
                case["filter"]["column"], case["filter"]["needle"]
            ),
            indices(&case["rows"]),
            rust,
        ));
    }
    out
}

#[test]
fn rust_view_matches_the_swift_fixtures() {
    let known: HashMap<&str, &Divergence> = DIVERGENCES.iter().map(|d| (d.case, d)).collect();
    let all = cases();
    assert!(
        all.len() > 200,
        "the fixture is suspiciously small: {}",
        all.len()
    );

    let mut problems = Vec::new();
    let mut seen = 0;
    for (id, swift, rust) in &all {
        match known.get(id.as_str()) {
            None if swift != rust => problems.push(format!(
                "UNLISTED {id}\n  swift {swift:?}\n  rust  {rust:?}"
            )),
            Some(d) => {
                seen += 1;
                if swift == rust {
                    problems.push(format!("STALE (now identical) {id}: {}", d.reason));
                } else if rust.as_slice() != d.rust {
                    problems.push(format!(
                        "CHANGED {id}\n  expected rust {:?}\n  actual rust   {rust:?}",
                        d.rust
                    ));
                }
            }
            None => {}
        }
    }
    if seen != DIVERGENCES.len() {
        problems.push(format!(
            "{} listed divergences name no fixture case",
            DIVERGENCES.len() - seen
        ));
    }
    assert!(problems.is_empty(), "\n{}", problems.join("\n"));
}
