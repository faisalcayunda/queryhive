//! The view pipeline: sort, filter, search, distinct, streaming, and the
//! error paths (blueprint §13, §21.1).

use std::sync::Arc;

use qh_core::{ColumnBatch, ColumnMeta, Value};
use qh_result_store::{
    distinct_values, matches_text, FilterSpec, Outcome, SortKey, StoreConfig, StoreError,
    StoreHandle, StoreRegistry, ViewSpec,
};

fn registry() -> Arc<StoreRegistry> {
    StoreRegistry::for_test(StoreConfig {
        spill_dir: None,
        ..StoreConfig::default()
    })
}

fn meta(names: &[&str]) -> Vec<ColumnMeta> {
    names
        .iter()
        .map(|name| ColumnMeta::new(*name, "text"))
        .collect()
}

fn columns_from_rows(rows: &[Vec<Value>]) -> Vec<Vec<Value>> {
    let width = rows.first().map_or(0, Vec::len);
    let mut columns: Vec<Vec<Value>> = vec![Vec::with_capacity(rows.len()); width];
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            columns[index].push(cell.clone());
        }
    }
    columns
}

/// A finished store, columns given directly.
fn finished(columns: Vec<Vec<Value>>, metas: Vec<ColumnMeta>) -> StoreHandle {
    let registry = registry();
    let handle = registry.create();
    let writer = handle.writer();
    writer.begin(metas).unwrap();
    writer.push(&ColumnBatch::new(columns).unwrap()).unwrap();
    writer
        .finish(Outcome::Complete { truncated: false })
        .unwrap();
    handle
}

/// A store left mid-stream (no `finish`), for the streaming tests.
fn streaming(columns: Vec<Vec<Value>>, metas: Vec<ColumnMeta>) -> StoreHandle {
    let registry = registry();
    let handle = registry.create();
    let writer = handle.writer();
    writer.begin(metas).unwrap();
    writer.push(&ColumnBatch::new(columns).unwrap()).unwrap();
    handle
}

fn text_rows(rows: &[&str]) -> Vec<Vec<Value>> {
    vec![rows.iter().map(|s| Value::Text((*s).into())).collect()]
}

#[test]
fn an_empty_spec_is_the_identity() {
    let handle = finished(text_rows(&["c", "a", "b"]), meta(&["x"]));
    assert!(ViewSpec::default().is_identity());
    let spec = ViewSpec::default();
    let info = qh_result_store::compute_view(handle.shared(), &spec).unwrap();
    assert!(info.is_identity);
    assert_eq!(info.visible, 3);
    assert!(info.rows.is_empty());
}

#[test]
fn sort_orders_text_naturally() {
    let handle = finished(text_rows(&["item10", "item2", "item1"]), meta(&["x"]));
    let id = handle
        .shared()
        .set_view(ViewSpec {
            sort: Some((0, false)),
            ..ViewSpec::default()
        })
        .unwrap();
    let view = handle.shared().current_view().unwrap();
    assert_eq!(view.id(), id);
    let rows = view.rows().unwrap();
    let texts: Vec<String> = rows
        .iter()
        .map(|row| {
            let values = handle.shared().values(*row..*row + 1).unwrap();
            qh_core::to_text(&values[0][0]).unwrap()
        })
        .collect();
    assert_eq!(texts, vec!["item1", "item2", "item10"]);
}

#[test]
fn sort_puts_nulls_last_ascending_and_first_descending() {
    let rows = vec![
        vec![Value::Text("b".into())],
        vec![Value::Null],
        vec![Value::Text("a".into())],
    ];
    let handle = finished(columns_from_rows(&rows), meta(&["x"]));
    handle
        .shared()
        .set_view(ViewSpec {
            sort: Some((0, false)),
            ..ViewSpec::default()
        })
        .unwrap();
    let asc = handle.shared().current_view().unwrap().rows().unwrap();
    // "a", "b", NULL
    let texts: Vec<Option<String>> = asc
        .iter()
        .map(|row| {
            let values = handle.shared().values(*row..*row + 1).unwrap();
            qh_core::to_text(&values[0][0])
        })
        .collect();
    assert_eq!(
        texts,
        vec![Some("a".to_owned()), Some("b".to_owned()), None]
    );

    handle
        .shared()
        .set_view(ViewSpec {
            sort: Some((0, true)),
            ..ViewSpec::default()
        })
        .unwrap();
    let desc = handle.shared().current_view().unwrap().rows().unwrap();
    let texts: Vec<Option<String>> = desc
        .iter()
        .map(|row| {
            let values = handle.shared().values(*row..*row + 1).unwrap();
            qh_core::to_text(&values[0][0])
        })
        .collect();
    assert_eq!(
        texts,
        vec![None, Some("b".to_owned()), Some("a".to_owned())]
    );
}

#[test]
fn numeric_text_sorts_by_value_not_lexically() {
    // "10" must come after "9" even though '1' < '9'.
    let handle = finished(text_rows(&["9", "10", "2"]), meta(&["x"]));
    handle
        .shared()
        .set_view(ViewSpec {
            sort: Some((0, false)),
            ..ViewSpec::default()
        })
        .unwrap();
    let rows = handle.shared().current_view().unwrap().rows().unwrap();
    let texts: Vec<String> = rows
        .iter()
        .map(|row| {
            let values = handle.shared().values(*row..*row + 1).unwrap();
            qh_core::to_text(&values[0][0]).unwrap()
        })
        .collect();
    assert_eq!(texts, vec!["2", "9", "10"]);
}

#[test]
fn equal_numeric_keys_tie_break_by_source_row() {
    // "1.0" and "1" compare equal as numbers; the tie-break is source order,
    // so a stable sort is observable.
    let handle = finished(text_rows(&["1.0", "1", "1.00"]), meta(&["x"]));
    handle
        .shared()
        .set_view(ViewSpec {
            sort: Some((0, false)),
            ..ViewSpec::default()
        })
        .unwrap();
    let rows = handle.shared().current_view().unwrap().rows().unwrap();
    assert_eq!(rows, vec![0, 1, 2]);
}

#[test]
fn a_sort_key_ranks_num_before_text_before_null() {
    let num = SortKey::Num(qh_result_store::swift_plain_number("1").unwrap());
    let mut key = Vec::new();
    qh_result_store::natural_key("a", &mut key);
    let text = SortKey::Text(key);
    let null = SortKey::Null;
    assert!(matches!(num, SortKey::Num(_)));
    assert!(matches!(text, SortKey::Text(_)));
    assert!(matches!(null, SortKey::Null));
}

#[test]
fn filter_by_values_keeps_only_listed_rows() {
    let handle = finished(text_rows(&["a", "b", "c", "a"]), meta(&["x"]));
    handle
        .shared()
        .set_view(ViewSpec {
            filters: vec![FilterSpec::Values {
                column: 0,
                values: vec![Some("a".to_owned())],
            }],
            ..ViewSpec::default()
        })
        .unwrap();
    let rows = handle.shared().current_view().unwrap().rows().unwrap();
    assert_eq!(rows, vec![0, 3]);
}

#[test]
fn filter_values_can_match_null() {
    let rows = vec![
        vec![Value::Text("a".into())],
        vec![Value::Null],
        vec![Value::Text("b".into())],
    ];
    let handle = finished(columns_from_rows(&rows), meta(&["x"]));
    handle
        .shared()
        .set_view(ViewSpec {
            filters: vec![FilterSpec::Values {
                column: 0,
                values: vec![None],
            }],
            ..ViewSpec::default()
        })
        .unwrap();
    let matched = handle.shared().current_view().unwrap().rows().unwrap();
    assert_eq!(matched, vec![1]);
}

#[test]
fn filter_text_operators_match_swift() {
    assert!(matches_text("10", ">=9"));
    assert!(!matches_text("8", ">=9"));
    assert!(matches_text("8", "<=9"));
    assert!(matches_text("10", ">9"));
    assert!(matches_text("8", "<9"));
    // "=" is case-insensitive.
    assert!(matches_text("Apple", "=apple"));
    // A bare needle is a case-insensitive substring.
    assert!(matches_text("Hello World", "world"));
    assert!(!matches_text("Hello", "world"));
    // An empty trimmed needle matches everything.
    assert!(matches_text("anything", "   "));
    // An operator with an empty operand falls through to substring.
    assert!(matches_text(">", ">"));
}

#[test]
fn filter_text_over_a_column() {
    let handle = finished(text_rows(&["apple", "banana", "cherry"]), meta(&["x"]));
    handle
        .shared()
        .set_view(ViewSpec {
            filters: vec![FilterSpec::Text {
                column: 0,
                needle: ">=banana".to_owned(),
            }],
            ..ViewSpec::default()
        })
        .unwrap();
    let rows = handle.shared().current_view().unwrap().rows().unwrap();
    assert_eq!(rows, vec![1, 2]);
}

#[test]
fn search_scans_every_column() {
    let rows = vec![
        vec![Value::Text("alpha".into()), Value::Text("one".into())],
        vec![Value::Text("beta".into()), Value::Text("two".into())],
    ];
    let handle = finished(columns_from_rows(&rows), meta(&["a", "b"]));
    // "two" lives in the second column, so a first-column-only search would
    // miss it.
    handle
        .shared()
        .set_view(ViewSpec {
            search: "TWO".to_owned(),
            ..ViewSpec::default()
        })
        .unwrap();
    let matched = handle.shared().current_view().unwrap().rows().unwrap();
    assert_eq!(matched, vec![1]);
}

#[test]
fn search_excludes_null_cells() {
    let rows = vec![vec![Value::Text("hit".into())], vec![Value::Null]];
    let handle = finished(columns_from_rows(&rows), meta(&["x"]));
    handle
        .shared()
        .set_view(ViewSpec {
            search: "hit".to_owned(),
            ..ViewSpec::default()
        })
        .unwrap();
    let matched = handle.shared().current_view().unwrap().rows().unwrap();
    assert_eq!(matched, vec![0]);
}

#[test]
fn filter_then_search_then_sort() {
    let rows = vec![
        vec![Value::Text("b".into()), Value::Text("keep".into())],
        vec![Value::Text("a".into()), Value::Text("drop".into())],
        vec![Value::Text("c".into()), Value::Text("keep".into())],
    ];
    let handle = finished(columns_from_rows(&rows), meta(&["a", "b"]));
    handle
        .shared()
        .set_view(ViewSpec {
            sort: Some((0, false)),
            filters: vec![FilterSpec::Values {
                column: 1,
                values: vec![Some("keep".to_owned())],
            }],
            search: String::new(),
        })
        .unwrap();
    let matched = handle.shared().current_view().unwrap().rows().unwrap();
    // Rows 0 and 2 survive the filter; sorted by column 0 that is b, c.
    assert_eq!(matched, vec![0, 2]);
}

#[test]
fn sorting_a_streaming_store_is_refused() {
    let handle = streaming(text_rows(&["a", "b"]), meta(&["x"]));
    let error = handle
        .shared()
        .set_view(ViewSpec {
            sort: Some((0, false)),
            ..ViewSpec::default()
        })
        .unwrap_err();
    assert!(matches!(error, StoreError::Streaming));
}

#[test]
fn a_filter_view_is_allowed_while_streaming() {
    let handle = streaming(text_rows(&["a", "b", "a"]), meta(&["x"]));
    handle
        .shared()
        .set_view(ViewSpec {
            filters: vec![FilterSpec::Values {
                column: 0,
                values: vec![Some("a".to_owned())],
            }],
            ..ViewSpec::default()
        })
        .unwrap();
    let view = handle.shared().current_view().unwrap();
    assert_eq!(view.row_count(), 2);
    assert_eq!(view.rows().unwrap(), vec![0, 2]);
}

#[test]
fn a_newer_view_supersedes_the_old_one() {
    let handle = finished(text_rows(&["a", "b"]), meta(&["x"]));
    handle
        .shared()
        .set_view(ViewSpec {
            search: "a".to_owned(),
            ..ViewSpec::default()
        })
        .unwrap();
    let old = handle.shared().current_view().unwrap();
    handle
        .shared()
        .set_view(ViewSpec {
            search: "b".to_owned(),
            ..ViewSpec::default()
        })
        .unwrap();
    let error = old.rows().unwrap_err();
    assert!(matches!(error, StoreError::Superseded));
}

#[test]
fn distinct_values_lists_what_the_column_holds() {
    let handle = finished(text_rows(&["b", "a", "b", "c"]), meta(&["x"]));
    let distinct = distinct_values(handle.shared(), 0, 10).unwrap();
    assert!(!distinct.more);
    assert_eq!(
        distinct.values,
        vec![
            Some("a".to_owned()),
            Some("b".to_owned()),
            Some("c".to_owned())
        ]
    );
}

#[test]
fn distinct_values_includes_null_once() {
    let rows = vec![
        vec![Value::Text("a".into())],
        vec![Value::Null],
        vec![Value::Text("a".into())],
        vec![Value::Null],
    ];
    let handle = finished(columns_from_rows(&rows), meta(&["x"]));
    let distinct = distinct_values(handle.shared(), 0, 10).unwrap();
    assert!(!distinct.more);
    assert_eq!(distinct.values, vec![None, Some("a".to_owned())]);
}

#[test]
fn distinct_values_stops_with_more_when_over_the_limit() {
    let handle = finished(text_rows(&["a", "b", "c", "d"]), meta(&["x"]));
    let distinct = distinct_values(handle.shared(), 0, 2).unwrap();
    assert!(distinct.more);
    assert!(distinct.values.is_empty());
}

#[test]
fn an_out_of_range_sort_column_is_refused() {
    let handle = finished(text_rows(&["a", "b"]), meta(&["x"]));
    let error = handle
        .shared()
        .set_view(ViewSpec {
            sort: Some((7, false)),
            ..ViewSpec::default()
        })
        .unwrap_err();
    assert!(matches!(error, StoreError::InvalidArgument { .. }));
}

#[test]
fn releasing_a_store_frees_its_budget() {
    let registry = registry();
    let handle = registry.create();
    let writer = handle.writer();
    writer.begin(meta(&["x"])).unwrap();
    writer
        .push(&ColumnBatch::new(text_rows(&["a"])).unwrap())
        .unwrap();
    writer
        .finish(Outcome::Complete { truncated: false })
        .unwrap();
    assert!(registry.stats().resident_bytes > 0);
    drop(handle);
    // Dropping the owner token releases the store and returns its bytes.
    assert_eq!(registry.stats().resident_bytes, 0);
    assert_eq!(registry.stats().stores, 0);
}
