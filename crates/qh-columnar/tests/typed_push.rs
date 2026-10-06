//! The typed `push_*` appends seal exactly like `push_value`, and `push_batch` stops at the seal
//! limit and says how many rows it took. Nothing calls them today; the W8-F2 re-A/B does.

use qh_columnar::{value_at, ChunkBuilder, Encoding, CHUNK_MAX_ROWS};
use qh_core::{ColumnBatch, IntervalValue, Value};

const WIDTH: usize = 13;

/// One row per call: a NULL in every column on row 0, then a typed value in each.
fn typed(builder: &mut ChunkBuilder) {
    for column in 0..WIDTH {
        builder.push_null(column);
    }
    builder.push_bool(0, true);
    builder.push_i64(1, -7);
    builder.push_u64(2, u64::MAX);
    builder.push_f64(3, 1.5);
    builder.push_decimal(4, 12_345, 2);
    builder.push_str(5, "text");
    builder.push_json(6, r#"{"a":1}"#);
    builder.push_bytes(7, &[0, 1, 255]);
    builder.push_date(8, 19_000);
    builder.push_time(9, 3_600_000_000);
    builder.push_timestamp(10, 1_700_000_000_000_000, None);
    builder.push_timestamp(11, 1_700_000_000_000_000, Some(7 * 3600));
    builder.push_interval(
        12,
        IntervalValue {
            months: 1,
            days: 2,
            micros: 3,
        },
    );
}

fn generic(builder: &mut ChunkBuilder) {
    for column in 0..WIDTH {
        builder.push_value(column, Value::Null);
    }
    let row = [
        Value::Bool(true),
        Value::Int(-7),
        Value::UInt(u64::MAX),
        Value::Float(1.5),
        Value::Decimal {
            unscaled: 12_345,
            scale: 2,
        },
        Value::Text("text".into()),
        Value::Json(r#"{"a":1}"#.into()),
        Value::Bytes(vec![0, 1, 255]),
        Value::Date { days: 19_000 },
        Value::Time {
            micros: 3_600_000_000,
        },
        Value::Timestamp {
            micros: 1_700_000_000_000_000,
            offset_secs: None,
        },
        Value::Timestamp {
            micros: 1_700_000_000_000_000,
            offset_secs: Some(7 * 3600),
        },
        Value::Interval(IntervalValue {
            months: 1,
            days: 2,
            micros: 3,
        }),
    ];
    for (column, value) in row.into_iter().enumerate() {
        builder.push_value(column, value);
    }
}

#[test]
fn typed_appends_seal_like_push_value() {
    let (mut by_type, mut by_value) = (ChunkBuilder::new(WIDTH), ChunkBuilder::new(WIDTH));
    typed(&mut by_type);
    generic(&mut by_value);
    assert_eq!(by_type.rows(), 2);
    let (by_type, by_value) = (by_type.seal().unwrap(), by_value.seal().unwrap());

    assert_eq!(by_type.encodings, by_value.encodings);
    assert!(by_type.encodings.iter().all(|e| *e != Encoding::Tagged));
    for column in 0..WIDTH {
        for row in 0..2 {
            let left = value_at(
                by_type.batch.column(column).as_ref(),
                by_type.encodings[column],
                row,
            );
            let right = value_at(
                by_value.batch.column(column).as_ref(),
                by_value.encodings[column],
                row,
            );
            assert_eq!(left.unwrap(), right.unwrap(), "column {column} row {row}");
        }
    }
}

#[test]
fn push_batch_stops_at_the_seal_limit_and_counts_what_it_took() {
    let rows = CHUNK_MAX_ROWS + 10;
    let batch = ColumnBatch::new(vec![(0..rows as i64).map(Value::Int).collect()]).unwrap();
    let mut builder = ChunkBuilder::new(1);
    assert_eq!(builder.push_batch(&batch).unwrap(), CHUNK_MAX_ROWS);
    assert!(builder.is_full());

    let wide = ColumnBatch::new(vec![vec![Value::Int(1)], vec![Value::Int(2)]]).unwrap();
    assert!(
        ChunkBuilder::new(1).push_batch(&wide).is_err(),
        "a width mismatch is an error"
    );
}
