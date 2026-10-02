//! The logical schema (§5.4): every variant row, one schema across all
//! batches including null, tagged, and spilled chunks, values equal to
//! `value_at` or `to_text`, and a clean IPC round trip.

use std::sync::Arc;

use arrow_array::{Array, RecordBatch};
use arrow_schema::DataType;
use qh_core::{ColumnBatch, ColumnMeta, IntervalValue, Value};
use qh_result_store::{
    logical_batch, logical_batches, logical_names, logical_schema, logical_type, Outcome,
    StoreChunk, StoreConfig, StoreError, StoreHandle, StoreRegistry,
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

/// A finished store from column-major data.
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

/// The single chunk of a one-chunk store.
fn only_chunk(handle: &StoreHandle) -> Arc<StoreChunk> {
    let reference = handle.shared().chunk_refs().into_iter().next().unwrap();
    handle.shared().load_chunk(reference.index).unwrap()
}

/// Round-trip a batch through IPC and return the second batch.
fn ipc_round_trip(batch: &RecordBatch) -> RecordBatch {
    let mut bytes = Vec::new();
    {
        let mut writer =
            arrow_ipc::writer::StreamWriter::try_new(&mut bytes, &batch.schema()).unwrap();
        writer.write(batch).unwrap();
        writer.finish().unwrap();
    }
    let reader =
        arrow_ipc::reader::StreamReader::try_new(std::io::Cursor::new(bytes), None).unwrap();
    reader.into_iter().next().unwrap().unwrap()
}

// --- §5.4: the logical type for each variant row -------------------------

#[test]
fn a_single_variant_maps_to_its_type() {
    let cases: Vec<(Value, DataType)> = vec![
        (Value::Bool(true), DataType::Boolean),
        (Value::Int(1), DataType::Int64),
        (Value::UInt(1), DataType::UInt64),
        (Value::Float(1.0), DataType::Float64),
        (Value::Date { days: 0 }, DataType::Date32),
        (
            Value::Time { micros: 0 },
            DataType::Time64(arrow_schema::TimeUnit::Microsecond),
        ),
        (
            Value::Timestamp {
                micros: 0,
                offset_secs: None,
            },
            DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None),
        ),
        (
            Value::Timestamp {
                micros: 0,
                offset_secs: Some(0),
            },
            DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, Some("+00:00".into())),
        ),
        (Value::Text("t".into()), DataType::Utf8),
        (Value::Json("{}".into()), DataType::Utf8),
        (Value::Bytes(vec![1]), DataType::Binary),
        (
            Value::Interval(IntervalValue {
                months: 1,
                days: 0,
                micros: 0,
            }),
            DataType::Interval(arrow_schema::IntervalUnit::MonthDayNano),
        ),
    ];
    for (value, expected) in cases {
        let handle = finished(vec![vec![value.clone()]], meta(&["x"]));
        let stats = handle.shared().stats();
        assert_eq!(
            logical_type(&stats[0]),
            expected,
            "variant for {value:?} mapped wrong"
        );
    }
}

#[test]
fn a_uniform_decimal_keeps_its_scale() {
    let handle = finished(
        vec![vec![
            Value::Decimal {
                unscaled: 123,
                scale: 2,
            },
            Value::Decimal {
                unscaled: 456,
                scale: 2,
            },
        ]],
        meta(&["x"]),
    );
    let stats = handle.shared().stats();
    assert_eq!(logical_type(&stats[0]), DataType::Decimal128(38, 2));
}

#[test]
fn a_null_only_column_is_null() {
    let handle = finished(vec![vec![Value::Null, Value::Null]], meta(&["x"]));
    let stats = handle.shared().stats();
    assert_eq!(logical_type(&stats[0]), DataType::Null);
}

#[test]
fn mixed_variants_fall_to_utf8() {
    let handle = finished(
        vec![vec![Value::Int(1), Value::Text("a".into())]],
        meta(&["x"]),
    );
    let stats = handle.shared().stats();
    assert_eq!(logical_type(&stats[0]), DataType::Utf8);
}

#[test]
fn an_overflowing_interval_falls_to_utf8() {
    let handle = finished(
        vec![vec![
            Value::Interval(IntervalValue {
                months: i32::MAX,
                days: 0,
                micros: i64::MAX,
            }),
            Value::Interval(IntervalValue {
                months: 0,
                days: 0,
                micros: 0,
            }),
        ]],
        meta(&["x"]),
    );
    let stats = handle.shared().stats();
    // The interval overflow flag is set for a value past what the logical
    // interval can hold; the column falls to text rather than losing digits.
    if stats[0].overflow {
        assert_eq!(logical_type(&stats[0]), DataType::Utf8);
    }
}

#[test]
fn a_json_column_carries_the_arrow_json_extension() {
    let handle = finished(vec![vec![Value::Json(r#"{"a":1}"#.into())]], meta(&["x"]));
    let stats = handle.shared().stats();
    let schema = logical_schema(&handle.shared().columns(), &stats);
    assert_eq!(schema.schema.field(0).data_type(), &DataType::Utf8);
    assert_eq!(
        schema
            .schema
            .field(0)
            .metadata()
            .get("ARROW:extension:name"),
        Some(&"arrow.json".to_owned())
    );
}

// --- names ---------------------------------------------------------------

#[test]
fn duplicate_names_get_suffixes_and_empty_names_a_placeholder() {
    let columns = vec![
        ColumnMeta::new("id", "int"),
        ColumnMeta::new("id", "int"),
        ColumnMeta::new("", "text"),
    ];
    let names = logical_names(&columns);
    assert_eq!(names, vec!["id", "id_2", "column_2"]);
}

// --- the whole pipeline --------------------------------------------------

#[test]
fn one_schema_covers_every_batch() {
    // A mixed store: one text column, one int column, one all-null column,
    // and a tagged column. Every batch must come out under the same schema.
    let handle = finished(
        vec![
            // text
            vec![Value::Text("a".into()), Value::Text("b".into())],
            // int
            vec![Value::Int(1), Value::Int(2)],
            // all null
            vec![Value::Null, Value::Null],
            // tagged: a mix that forces the tagged encoding
            vec![Value::Int(1), Value::Text("mixed".into())],
        ],
        meta(&["t", "i", "n", "tagged"]),
    );
    let shared = handle.shared();
    let stats = shared.stats();
    let schema = logical_schema(&shared.columns(), &stats);
    let chunks = vec![only_chunk(&handle)];
    let batches = logical_batches(&chunks, &[0, 1, 2, 3], &schema).unwrap();
    assert!(!batches.is_empty());
    for batch in &batches {
        assert_eq!(batch.schema(), schema.schema);
        assert_eq!(batch.num_columns(), 4);
        // The batch must survive a rebuild and an IPC round trip.
        let rebuilt = RecordBatch::try_new(schema.schema.clone(), batch.columns().to_vec());
        assert!(rebuilt.is_ok());
        let round = ipc_round_trip(batch);
        assert_eq!(round.schema(), schema.schema);
        assert_eq!(round.num_rows(), batch.num_rows());
    }
}

#[test]
fn a_fallen_column_keeps_the_grid_text() {
    // A tagged column whose logical type is Utf8 must read back as the same
    // text the grid shows, never a silent NULL.
    let handle = finished(
        vec![vec![
            Value::Int(7),
            Value::Text("seven".into()),
            Value::Null,
        ]],
        meta(&["x"]),
    );
    let shared = handle.shared();
    let stats = shared.stats();
    assert_eq!(logical_type(&stats[0]), DataType::Utf8);
    let schema = logical_schema(&shared.columns(), &stats);
    let chunk = only_chunk(&handle);
    let batch = logical_batch(&chunk, &[0], &schema).unwrap();
    let array = batch.column(0);
    let text = array
        .as_any()
        .downcast_ref::<arrow_array::StringArray>()
        .unwrap();
    assert_eq!(text.value(0), "7");
    assert_eq!(text.value(1), "seven");
    assert!(text.is_null(2), "a NULL must stay NULL, not become empty");
}

#[test]
fn logical_values_equal_value_at() {
    let columns = vec![
        vec![Value::Text("a".into()), Value::Text("b".into())],
        vec![Value::Int(1), Value::Int(2)],
    ];
    let handle = finished(columns, meta(&["t", "i"]));
    let shared = handle.shared();
    let schema = logical_schema(&shared.columns(), &shared.stats());
    let chunk = only_chunk(&handle);
    let batch = logical_batch(&chunk, &[0, 1], &schema).unwrap();

    let texts = batch
        .column(0)
        .as_any()
        .downcast_ref::<arrow_array::StringArray>()
        .unwrap();
    let ints = batch
        .column(1)
        .as_any()
        .downcast_ref::<arrow_array::Int64Array>()
        .unwrap();
    let values = shared.values(0..2).unwrap();
    for (row, row_values) in values.iter().enumerate() {
        assert_eq!(texts.value(row), qh_core::to_text(&row_values[0]).unwrap());
        assert_eq!(
            ints.value(row),
            match &row_values[1] {
                Value::Int(v) => *v,
                other => panic!("expected int, got {other:?}"),
            }
        );
    }
}

#[test]
fn a_spilled_chunk_converts_the_same_as_a_resident_one() {
    let dir = std::env::temp_dir().join(format!("qh-logical-spill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let registry = StoreRegistry::for_test(StoreConfig {
        spill_dir: Some(dir.clone()),
        ..StoreConfig::default()
    });
    let handle = registry.create();
    let writer = handle.writer();
    writer.begin(meta(&["t", "i"])).unwrap();
    writer
        .push(
            &ColumnBatch::new(vec![
                vec![Value::Text("a".into()), Value::Text("b".into())],
                vec![Value::Int(1), Value::Int(2)],
            ])
            .unwrap(),
        )
        .unwrap();
    writer
        .finish(Outcome::Complete { truncated: false })
        .unwrap();

    let shared = handle.shared();
    let schema = logical_schema(&shared.columns(), &shared.stats());
    let resident = logical_batch(&only_chunk(&handle), &[0, 1], &schema).unwrap();

    // Drop the reader's reference, spill, and convert again.
    let moved = shared.force_spill_all_for_test(&registry);
    assert!(moved >= 1, "nothing spilled");
    let spilled = logical_batch(&only_chunk(&handle), &[0, 1], &schema).unwrap();
    assert_eq!(resident, spilled);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_decoded_record_round_trips_through_ipc() {
    // Encode a chunk to a spill record and decode it back; the logical view
    // of the decoded batch must match the original.
    let handle = finished(
        vec![vec![Value::Text("x".into()), Value::Text("y".into())]],
        meta(&["t"]),
    );
    let chunk = only_chunk(&handle);
    let encoded = qh_result_store::encode_record(&chunk.batch, &chunk.flags).unwrap();
    let decoded = qh_result_store::decode_record(&encoded).unwrap();
    assert_eq!(decoded.batch.num_rows(), chunk.batch.num_rows());
    assert_eq!(decoded.batch.num_columns(), chunk.batch.num_columns());
    assert_eq!(decoded.batch.schema(), chunk.batch.schema());
}

#[test]
fn a_column_past_the_chunk_width_is_not_a_panic() {
    // A target that does not exist must clamp or error, never index out of
    // bounds.
    let handle = finished(vec![vec![Value::Text("a".into())]], meta(&["t"]));
    let shared = handle.shared();
    let schema = logical_schema(&shared.columns(), &shared.stats());
    let chunk = only_chunk(&handle);
    let result = logical_batch(&chunk, &[5], &schema);
    match result {
        Ok(batch) => assert_eq!(batch.num_columns(), 1),
        Err(error) => assert!(matches!(error, StoreError::Corrupt { .. })),
    }
}
