//! Seeded round trips: every `Value` variant through seal and back.
//!
//! Checks `value_at(seal(push(v))) == v` and identical `to_text`, for each
//! encoding in the table plus the tagged fallbacks (mixed scales, mixed
//! offsets, second-precision offsets, 39-digit decimals, nested composites,
//! all three `Unknown` forms, NaN payloads, extreme intervals).

use qh_columnar::{value_at, ChunkBuilder, Encoding};
use qh_core::{IntervalValue, Value};

/// Every variant, so one the mapping forgot fails here rather than in grid.
fn every_value() -> Vec<Value> {
    vec![
        Value::Null,
        Value::Bool(true),
        Value::Bool(false),
        Value::Int(0),
        Value::Int(i64::MIN),
        Value::Int(i64::MAX),
        Value::UInt(0),
        Value::UInt(u64::MAX),
        Value::Float(0.0),
        Value::Float(-0.0),
        Value::Float(1.5),
        Value::Float(f64::from_bits(0x7ff0_0000_0000_0001)),
        Value::Float(f64::INFINITY),
        Value::Decimal {
            unscaled: 0,
            scale: 0,
        },
        Value::Decimal {
            unscaled: -1,
            scale: 10,
        },
        Value::Decimal {
            unscaled: 12_345_678_901_234_567_890_123_456_781_234_567_890,
            scale: 10,
        },
        Value::Text("".into()),
        Value::Text("hello".into()),
        Value::Text("é中😀".into()),
        Value::Bytes(vec![]),
        Value::Bytes(vec![0x00, 0x01, 0xff]),
        Value::Timestamp {
            micros: 1_769_835_600_123_456,
            offset_secs: Some(7 * 3600),
        },
        Value::Timestamp {
            micros: 1_769_835_600_000_000,
            offset_secs: None,
        },
        Value::Timestamp {
            micros: 0,
            offset_secs: Some(0),
        },
        Value::Timestamp {
            micros: -1,
            offset_secs: Some(-5 * 3600),
        },
        Value::Date { days: 0 },
        Value::Date { days: -25_509 },
        Value::Time {
            micros: 86_399_999_999,
        },
        Value::Interval(IntervalValue {
            months: 1,
            days: 3,
            micros: 14_706_000_000,
        }),
        Value::Interval(IntervalValue {
            months: -2,
            days: -1,
            micros: -1,
        }),
        Value::Interval(IntervalValue {
            months: i32::MAX,
            days: -2_000_000_000,
            micros: i64::MAX / 1000,
        }),
        Value::Json(r#"{"b":1,"a":2}"#.into()),
        Value::Array(vec![]),
        Value::Array(vec![Value::Int(1), Value::Null, Value::Int(3)]),
        Value::Array(vec![Value::Array(vec![Value::Null])]),
        Value::Row(vec![Value::Int(1), Value::Text("a".into())]),
        Value::Map(vec![]),
        Value::Map(vec![(Value::Text("k".into()), Value::Null)]),
        Value::Map(vec![(
            Value::Int(1),
            Value::Array(vec![Value::Bool(true), Value::Null]),
        )]),
        Value::unknown("point", "(1,2)"),
        Value::unknown("inet", ""),
        Value::unknown_bytes("geometry", vec![0x00, 0xff]),
        // Both forms at once: text for the grid, raw for the export.
        Value::Unknown {
            type_name: "pg_lsn".into(),
            text: Some("0/16B1978".into()),
            raw: Some(vec![0x00, 0x01, 0x6B, 0x19, 0x78]),
        },
        // Neither form: the cell still exists.
        Value::Unknown {
            type_name: "void".into(),
            text: None,
            raw: None,
        },
    ]
}

/// Seal one value per chunk-column and read it back.
fn check_round_trip(values: &[Value]) {
    let mut builder = ChunkBuilder::new(values.len());
    for (column, value) in values.iter().enumerate() {
        builder.push_value(column, value.clone());
    }
    let sealed = builder.seal().expect("seal");
    assert_eq!(sealed.batch.num_rows(), 1);
    for (column, expected) in values.iter().enumerate() {
        let array = sealed.batch.column(column);
        let found = value_at(array, sealed.encodings[column], 0).expect("read");
        assert_values_equal(&found, expected, column);
        assert_eq!(
            found.render_text(),
            expected.render_text(),
            "column {column}"
        );
    }
}

/// NaN never equals itself, so floats compare by bits.
fn assert_values_equal(found: &Value, expected: &Value, column: usize) {
    match (found, expected) {
        (Value::Float(a), Value::Float(b)) => {
            assert_eq!(a.to_bits(), b.to_bits(), "column {column}")
        }
        _ => assert_eq!(found, expected, "column {column}"),
    }
}

#[test]
fn every_value_survives_a_round_trip() {
    check_round_trip(&every_value());
}

#[test]
fn a_float_keeps_its_exact_bits() {
    // Through a NaN and back: == fails for NaN, so bits are the assertion.
    let values = vec![
        Value::Float(f64::NAN),
        Value::Float(f64::INFINITY),
        Value::Float(-0.0),
    ];
    let mut builder = ChunkBuilder::new(1);
    for value in &values {
        builder.push_value(0, value.clone());
    }
    let sealed = builder.seal().expect("seal");
    assert_eq!(sealed.encodings[0], Encoding::F64);
    for (row, expected) in values.iter().enumerate() {
        let Value::Float(bits) =
            value_at(sealed.batch.column(0), Encoding::F64, row).expect("read")
        else {
            panic!("row {row} is not a float");
        };
        let Value::Float(want) = expected else {
            unreachable!()
        };
        assert_eq!(bits.to_bits(), want.to_bits(), "row {row}");
    }
}

#[test]
fn uniform_columns_keep_their_encodings() {
    let mut builder = ChunkBuilder::new(4);
    for value in [Value::Int(1), Value::Null] {
        builder.push_value(0, value);
    }
    for value in [Value::Text("a".into()), Value::Text("b".into())] {
        builder.push_value(1, value);
    }
    for _ in 0..2 {
        builder.push_value(2, Value::Null);
    }
    for value in [
        Value::Decimal {
            unscaled: 150,
            scale: 2,
        },
        Value::Decimal {
            unscaled: -25,
            scale: 2,
        },
    ] {
        builder.push_value(3, value);
    }
    let sealed = builder.seal().expect("seal");
    assert_eq!(
        sealed.encodings,
        vec![Encoding::I64, Encoding::Text, Encoding::Null, Encoding::Dec]
    );
    assert_eq!(
        value_at(sealed.batch.column(0), Encoding::I64, 1).unwrap(),
        Value::Null
    );
}

#[test]
fn mixed_columns_fall_back_to_tagged_without_losing_values() {
    let mixed = vec![Value::Int(1), Value::Text("NaN".into()), Value::Null];
    let scales = vec![
        Value::Decimal {
            unscaled: 150,
            scale: 2,
        },
        Value::Decimal {
            unscaled: 125,
            scale: 3,
        },
    ];
    let zones = vec![
        Value::Timestamp {
            micros: 0,
            offset_secs: Some(3600),
        },
        Value::Timestamp {
            micros: 0,
            offset_secs: Some(7200),
        },
    ];
    let seconds = vec![Value::Timestamp {
        micros: 0,
        offset_secs: Some(25_232),
    }];
    let wide = vec![Value::Decimal {
        unscaled: 10i128.pow(38),
        scale: 0,
    }];
    for values in [&mixed, &scales, &zones, &seconds, &wide] {
        let mut builder = ChunkBuilder::new(1);
        for value in values {
            builder.push_value(0, value.clone());
        }
        let sealed = builder.seal().expect("seal");
        assert_eq!(sealed.encodings, vec![Encoding::Tagged]);
        check_column(values, &sealed.batch, Encoding::Tagged);
    }
}

fn check_column(values: &[Value], batch: &arrow_array::RecordBatch, enc: Encoding) {
    assert_eq!(batch.num_rows(), values.len());
    for (row, expected) in values.iter().enumerate() {
        let found = value_at(batch.column(0), enc, row).expect("read");
        assert_eq!(&found, expected, "row {row}");
        assert_eq!(found.render_text(), expected.render_text(), "row {row}");
    }
}

#[test]
fn unknown_raw_keeps_its_bytes_and_its_hex_text() {
    // B-5: the old codec decoded form 2 (`raw`) to lossy text with
    // `raw: None`, so `to_text` changed from hex to text. The blob must
    // come back with its bytes, and the text must stay hex.
    let raw = Value::unknown_bytes("geometry", vec![0x00, 0xff]);
    let mut builder = ChunkBuilder::new(1);
    builder.push_value(0, raw.clone());
    let sealed = builder.seal().expect("seal");
    assert_eq!(sealed.encodings, vec![Encoding::Tagged]);
    let found = value_at(sealed.batch.column(0), Encoding::Tagged, 0).unwrap();
    assert_eq!(found, raw);
    assert_eq!(found.render_text().as_deref(), Some("00ff"));
}

#[test]
fn deep_nesting_is_an_error_not_a_stack_overflow() {
    // A server (Trino ROW/ARRAY/MAP) can hand us an arbitrarily deep value.
    // Past the cap the encoder must return an error, not recurse to a stack
    // overflow (which aborts the process and no guard can catch).
    let mut value = Value::Int(0);
    for _ in 0..200 {
        value = Value::Array(vec![value]);
    }
    let mut builder = ChunkBuilder::new(1);
    builder.push_value(0, value);
    let result = builder.seal();
    assert!(result.is_err(), "deep nesting should be refused");

    // At the cap it still round-trips.
    let mut ok = Value::Int(7);
    for _ in 0..40 {
        ok = Value::Array(vec![ok]);
    }
    let mut builder = ChunkBuilder::new(1);
    builder.push_value(0, ok.clone());
    let sealed = builder.seal().expect("seal at the cap");
    let found = value_at(sealed.batch.column(0), Encoding::Tagged, 0).unwrap();
    assert_eq!(found, ok);
}

#[test]
fn a_deeply_nested_blob_is_an_error_not_a_stack_overflow() {
    // The decode side has the same cap: a crafted blob of nested TAG_ARRAY
    // must return Corrupt, not overflow the stack.
    let mut blob = Vec::new();
    for _ in 0..200 {
        blob.push(13u8); // TAG_ARRAY
        blob.extend_from_slice(&1u32.to_le_bytes());
    }
    blob.push(2u8); // TAG_INT
    blob.extend_from_slice(&0i64.to_le_bytes());
    let mut reader = qh_columnar::tagged::Reader::new(&blob);
    let result = qh_columnar::tagged::decode_value(&mut reader);
    assert!(result.is_err(), "deep blob should be refused");
}

#[test]
fn split_sealable_bounds_each_piece_and_loses_no_row() {
    let mut builder = ChunkBuilder::new(2);
    let text = "x".repeat(1024);
    for i in 0..5_000i64 {
        builder.push_i64(0, i);
        builder.push_str(1, &text);
    }
    let mut seen = 0i64;
    let mut pieces = 0;
    while !builder.is_empty() {
        let piece = builder.split_sealable();
        let rows = piece.rows();
        assert!(rows * 1049 <= 2 * 1024 * 1024 + 1049, "{rows} rows");
        let sealed = piece.seal().unwrap();
        for row in 0..rows {
            let value = value_at(sealed.batch.column(0).as_ref(), sealed.encodings[0], row);
            assert_eq!(value.unwrap(), Value::Int(seen));
            seen += 1;
        }
        pieces += 1;
    }
    assert_eq!((seen, pieces > 1), (5_000, true));
}

#[test]
fn truncate_takes_the_dropped_rows_out_of_the_size_estimate() {
    let text = "x".repeat(1024);
    let mut builder = ChunkBuilder::new(2);
    for i in 0..3_000i64 {
        builder.push_i64(0, i);
        builder.push_str(1, &text);
    }
    assert!(builder.is_full(), "3,000 KiB rows pass the 2 MiB limit");
    builder.truncate(10);
    assert_eq!(builder.rows(), 10);
    assert!(!builder.is_full(), "ten rows are not a full chunk");
    builder.truncate(50); // past the end: a no-op
    assert_eq!(builder.rows(), 10);
}

#[test]
fn push_owned_into_an_empty_builder_counts_like_pushing_cell_by_cell() {
    let text = "x".repeat(1024);
    let columns = || {
        qh_core::ColumnBatch::new(vec![
            (0..3_000).map(Value::Int).collect(),
            (0..3_000)
                .map(|_| Value::Text(text.as_str().into()))
                .collect(),
        ])
        .unwrap()
    };
    let mut adopted = ChunkBuilder::new(2);
    adopted.push_owned(columns()).unwrap();
    let mut stepped = ChunkBuilder::new(2);
    stepped.push_i64(0, 0);
    stepped.push_str(1, "");
    stepped.push_owned(columns()).unwrap();
    assert_eq!((adopted.rows(), stepped.rows()), (3_000, 3_001));
    assert!(adopted.is_full() && stepped.is_full());
    // Both seal to the same cells.
    let sealed = adopted.seal().unwrap();
    let value = value_at(sealed.batch.column(0).as_ref(), sealed.encodings[0], 2_999);
    assert_eq!(value.unwrap(), Value::Int(2_999));
}
