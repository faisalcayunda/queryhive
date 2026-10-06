//! `ORDER BY qh_natural(x)` is the order the grid gives (blueprint section 14.10).
//!
//! The corpus is the one the grid's differential test uses
//! (`crates/qh-result-store/tests/fixtures/differential.json`, text with accents, case,
//! digit runs, emoji and NULLs). A text column is sorted twice, once by the store's own
//! `set_view` and once by DataFusion through the UDF, and the row permutations must be the
//! same. A column whose cells the grid reads as numbers is sorted by value there (that is
//! the grid's rule for numbers, not for text), so it is left out of the comparison and the
//! test says which columns it compared.

use std::sync::Arc;

use crate::common::{collect, engine};
use datafusion::arrow::array::{ArrayRef, Int64Array, StringArray};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::datasource::MemTable;
use qh_core::{ColumnBatch, ColumnMeta, Value};
use qh_result_store::{Outcome, StoreConfig, StoreRegistry, ViewSpec};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/qh-result-store/tests/fixtures/differential.json"
);

fn corpus() -> (Vec<String>, Vec<Vec<Option<String>>>) {
    let fixture: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(FIXTURE).unwrap()).unwrap();
    let names = fixture["columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap().to_owned())
        .collect();
    let rows = fixture["rows"].as_array().unwrap();
    let columns = (0..fixture["columns"].as_array().unwrap().len())
        .map(|column| {
            rows.iter()
                .map(|row| row[column].as_str().map(str::to_owned))
                .collect()
        })
        .collect();
    (names, columns)
}

/// The grid's order: the store's own view, sorted on `column`.
fn grid_order(
    names: &[String],
    columns: &[Vec<Option<String>>],
    column: usize,
    descending: bool,
) -> Vec<i64> {
    let registry = StoreRegistry::for_test(StoreConfig {
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
    let values = columns
        .iter()
        .map(|column| {
            column
                .iter()
                .map(|cell| {
                    cell.as_deref()
                        .map_or(Value::Null, |t| Value::Text(t.into()))
                })
                .collect()
        })
        .collect();
    writer.push(&ColumnBatch::new(values).unwrap()).unwrap();
    writer
        .finish(Outcome::Complete { truncated: false })
        .unwrap();
    handle
        .shared()
        .set_view(ViewSpec {
            sort: Some((column, descending)),
            ..ViewSpec::default()
        })
        .unwrap();
    let view = handle.shared().current_view().unwrap();
    view.source_rows(0..view.row_count())
        .unwrap()
        .into_iter()
        .map(i64::from)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sql_and_the_grid_order_text_the_same_way() {
    let (names, columns) = corpus();
    let rows = columns[0].len();
    assert_eq!(rows, 40);

    let mut fields = vec![Field::new("rid", DataType::Int64, false)];
    let mut arrays: Vec<ArrayRef> = vec![Arc::new(Int64Array::from(
        (0..rows as i64).collect::<Vec<_>>(),
    ))];
    for (name, column) in names.iter().zip(&columns) {
        fields.push(Field::new(name, DataType::Utf8, true));
        arrays.push(Arc::new(StringArray::from(column.clone())));
    }
    let schema = Arc::new(Schema::new(fields));
    let batch = RecordBatch::try_new(schema.clone(), arrays).unwrap();
    let (engine, _dir) = engine(4);
    let session = engine.session();
    session
        .register(
            "corpus",
            Arc::new(MemTable::try_new(schema, vec![vec![batch]]).unwrap()),
            Some(rows as u64),
        )
        .unwrap();
    let _lease = engine.grant(64 << 20);

    let mut compared = Vec::new();
    for (index, name) in names.iter().enumerate() {
        // A column the grid would sort by number is not a text-order question.
        let numeric = columns[index]
            .iter()
            .flatten()
            .any(|cell| qh_result_store::swift_double(cell).is_some());
        if numeric {
            continue;
        }
        compared.push(name.clone());
        for descending in [false, true] {
            let direction = if descending { "DESC" } else { "ASC" };
            // DataFusion's default puts NULL last when ascending and first when descending,
            // which is the grid's rule too (NULL is the largest value).
            let sql =
                format!("SELECT rid FROM corpus ORDER BY qh_natural(\"{name}\") {direction}, rid");
            let batches = collect(&session, &sql).await.unwrap();
            let sql_order: Vec<i64> = batches
                .iter()
                .flat_map(|batch| {
                    let ids = batch
                        .column(0)
                        .as_any()
                        .downcast_ref::<Int64Array>()
                        .unwrap();
                    (0..ids.len()).map(|i| ids.value(i)).collect::<Vec<_>>()
                })
                .collect();
            let grid = grid_order(&names, &columns, index, descending);
            assert_eq!(sql_order, grid, "column {name} {direction}");
        }
    }
    assert!(
        compared.len() >= 2,
        "the corpus should have at least two text columns the grid sorts as text, got {compared:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_key_is_the_stores_key_and_null_stays_null() {
    let (engine, _dir) = engine(2);
    let session = engine.session();
    let _lease = engine.grant(1 << 20);
    let batches = collect(&session, "SELECT qh_natural('item10') > qh_natural('item9'), qh_natural(CAST(NULL AS VARCHAR)) IS NULL").await.unwrap();
    let texts = crate::common::texts(&batches);
    assert_eq!(
        texts,
        vec![vec![Some("true".to_owned()), Some("true".to_owned())]]
    );
    let mut key = Vec::new();
    qh_result_store::natural_key("item10", &mut key);
    let batches = collect(&session, "SELECT qh_natural('item10')")
        .await
        .unwrap();
    let column = batches[0]
        .column(0)
        .as_any()
        .downcast_ref::<datafusion::arrow::array::BinaryArray>()
        .unwrap();
    assert_eq!(column.value(0), key.as_slice());
}
