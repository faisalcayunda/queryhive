//! The §5.6 table, one `DataType` at a time: what each Arrow type becomes in the store.
//!
//! Run with `cargo test -p qh-columnar --features from-arrow`; the feature is off in the
//! app's build, so a plain `cargo test --workspace` does not compile this file.
#![cfg(feature = "from-arrow")]

use std::sync::Arc;

use arrow_array::builder::{Int32Builder, ListBuilder, MapBuilder, StringBuilder};
use arrow_array::types::{Int32Type, IntervalMonthDayNano};
use arrow_array::*;
use arrow_buffer::{i256, ScalarBuffer};
use arrow_schema::{
    DataType, Field, Fields, IntervalUnit, Schema, TimeUnit, UnionFields, UnionMode,
};
use qh_columnar::{from_arrow, value_at, ColumnarError, Encoding};
use qh_core::{IntervalValue, Value};

/// Convert a one-column batch; return the encoding and every cell.
fn one(array: ArrayRef) -> (Encoding, Vec<Value>) {
    one_with(Field::new("x", array.data_type().clone(), true), array)
}

fn one_with(field: Field, array: ArrayRef) -> (Encoding, Vec<Value>) {
    let batch = RecordBatch::try_new(Arc::new(Schema::new(vec![field])), vec![array]).unwrap();
    let sealed = from_arrow(&batch).unwrap();
    let enc = sealed.encodings[0];
    let column = sealed.batch.column(0);
    let cells = (0..column.len())
        .map(|row| value_at(column.as_ref(), enc, row).unwrap())
        .collect();
    (enc, cells)
}

fn ints(values: &[Option<i64>]) -> Vec<Value> {
    values
        .iter()
        .map(|v| v.map_or(Value::Null, Value::Int))
        .collect()
}

#[test]
fn small_integers_widen_to_i64_and_u64() {
    let expected = ints(&[Some(-5), None, Some(7)]);
    assert_eq!(
        one(Arc::new(Int8Array::from(vec![Some(-5), None, Some(7)]))),
        (Encoding::I64, expected.clone())
    );
    assert_eq!(
        one(Arc::new(Int16Array::from(vec![Some(-5), None, Some(7)]))),
        (Encoding::I64, expected.clone())
    );
    assert_eq!(
        one(Arc::new(Int32Array::from(vec![Some(-5), None, Some(7)]))),
        (Encoding::I64, expected.clone())
    );
    assert_eq!(
        one(Arc::new(Int64Array::from(vec![Some(-5), None, Some(7)]))),
        (Encoding::I64, expected)
    );

    let unsigned = vec![Value::UInt(0), Value::Null, Value::UInt(200)];
    assert_eq!(
        one(Arc::new(UInt8Array::from(vec![Some(0), None, Some(200)]))),
        (Encoding::U64, unsigned.clone())
    );
    assert_eq!(
        one(Arc::new(UInt16Array::from(vec![Some(0), None, Some(200)]))),
        (Encoding::U64, unsigned.clone())
    );
    assert_eq!(
        one(Arc::new(UInt32Array::from(vec![Some(0), None, Some(200)]))),
        (Encoding::U64, unsigned.clone())
    );
    let (enc, cells) = one(Arc::new(UInt64Array::from(vec![u64::MAX, 1])));
    assert_eq!(
        (enc, cells),
        (Encoding::U64, vec![Value::UInt(u64::MAX), Value::UInt(1)])
    );
}

#[test]
fn floats_keep_the_shortest_decimal_text() {
    let (enc, cells) = one(Arc::new(Float32Array::from(vec![0.1f32, 1.5, -0.0])));
    assert_eq!(enc, Encoding::F64);
    // `0.1f32 as f64` would be 0.10000000149011612: the grid would show that.
    assert_eq!(cells[0], Value::Float(0.1));
    assert_eq!(cells[1], Value::Float(1.5));
    assert!(matches!(&cells[2], Value::Float(v) if v.to_bits() == (-0.0f64).to_bits()));

    let half = Float16Array::from(vec![half::f16::from_f32(0.5), half::f16::from_f32(0.1)]);
    let (_, cells) = one(Arc::new(half));
    assert_eq!(cells[0], Value::Float(0.5));
    assert_eq!(cells[1], Value::Float(0.1));

    // A float64 passes through bit for bit, NaN included.
    let nan = f64::from_bits(0x7ff8_0000_0000_1234);
    let (enc, cells) = one(Arc::new(Float64Array::from(vec![nan, f64::INFINITY, -0.0])));
    assert_eq!(enc, Encoding::F64);
    assert!(matches!(&cells[0], Value::Float(v) if v.is_nan()));
    assert!(matches!(&cells[1], Value::Float(v) if *v == f64::INFINITY));
    assert!(matches!(&cells[2], Value::Float(v) if v.to_bits() == (-0.0f64).to_bits()));

    // f32 specials survive the text round trip as the same kind of value.
    let (_, cells) = one(Arc::new(Float32Array::from(vec![
        f32::NAN,
        f32::NEG_INFINITY,
    ])));
    assert!(matches!(&cells[0], Value::Float(v) if v.is_nan()));
    assert_eq!(cells[1], Value::Float(f64::NEG_INFINITY));
}

#[test]
fn every_string_flavour_is_text_and_json_follows_the_field() {
    let text = vec![
        Value::Text("a".into()),
        Value::Null,
        Value::Text("é".into()),
    ];
    let values = vec![Some("a"), None, Some("é")];
    assert_eq!(
        one(Arc::new(StringArray::from(values.clone()))),
        (Encoding::Text, text.clone())
    );
    assert_eq!(
        one(Arc::new(LargeStringArray::from(values.clone()))),
        (Encoding::Text, text.clone())
    );
    assert_eq!(
        one(Arc::new(StringViewArray::from(values.clone()))),
        (Encoding::Text, text)
    );

    let json = Field::new("j", DataType::Utf8, true)
        .with_metadata([("ARROW:extension:name".to_owned(), "arrow.json".to_owned())].into());
    let (enc, cells) = one_with(json, Arc::new(StringArray::from(vec![r#"{"a":1}"#])));
    assert_eq!(
        (enc, cells),
        (Encoding::Json, vec![Value::Json(r#"{"a":1}"#.into())])
    );
}

#[test]
fn every_binary_flavour_is_bytes() {
    let bytes = vec![Value::Bytes(vec![0, 255]), Value::Null];
    let (enc, cells) = one(Arc::new(BinaryArray::from(vec![
        Some(&[0u8, 255][..]),
        None,
    ])));
    assert_eq!((enc, cells), (Encoding::Bytes, bytes.clone()));
    let (enc, cells) = one(Arc::new(LargeBinaryArray::from(vec![
        Some(&[0u8, 255][..]),
        None,
    ])));
    assert_eq!((enc, cells), (Encoding::Bytes, bytes.clone()));
    let (enc, cells) = one(Arc::new(BinaryViewArray::from(vec![
        Some(&[0u8, 255][..]),
        None,
    ])));
    assert_eq!((enc, cells), (Encoding::Bytes, bytes.clone()));
    let fixed = FixedSizeBinaryArray::try_from_sparse_iter_with_size(
        vec![Some(&[0u8, 255][..]), None].into_iter(),
        2,
    )
    .unwrap();
    assert_eq!(one(Arc::new(fixed)), (Encoding::Bytes, bytes));
}

fn ts(micros: i64, offset: Option<i32>) -> Value {
    Value::Timestamp {
        micros,
        offset_secs: offset,
    }
}

#[test]
fn timestamps_become_microseconds_and_nanoseconds_floor() {
    let (enc, cells) = one(Arc::new(TimestampSecondArray::from(vec![1, 2])));
    assert_eq!(
        (enc, cells),
        (Encoding::Ts, vec![ts(1_000_000, None), ts(2_000_000, None)])
    );
    let (_, cells) = one(Arc::new(TimestampMillisecondArray::from(vec![1_500])));
    assert_eq!(cells, vec![ts(1_500_000, None)]);
    let (_, cells) = one(Arc::new(TimestampMicrosecondArray::from(vec![7])));
    assert_eq!(cells, vec![ts(7, None)]);
    // 1_999 ns is 1 us; -1 ns is -1 us (floor, towards the past), not 0.
    let (_, cells) = one(Arc::new(TimestampNanosecondArray::from(vec![
        1_999, -1, -1_000, -1_001,
    ])));
    assert_eq!(
        cells,
        vec![ts(1, None), ts(-1, None), ts(-1, None), ts(-2, None)]
    );
}

#[test]
fn a_fixed_offset_zone_is_tsz_and_the_instant_is_kept() {
    let array =
        TimestampMicrosecondArray::from(vec![1_700_000_000_000_000]).with_timezone("+07:00");
    let (enc, cells) = one(Arc::new(array));
    assert_eq!(
        (enc, cells),
        (Encoding::Tsz, vec![ts(1_700_000_000_000_000, Some(25_200))])
    );
    let array = TimestampSecondArray::from(vec![0]).with_timezone("-03:30");
    let (enc, cells) = one(Arc::new(array));
    assert_eq!((enc, cells), (Encoding::Tsz, vec![ts(0, Some(-12_600))]));
}

#[test]
fn a_named_zone_is_resolved_per_value() {
    // Jakarta has no DST, so every value has the same offset and the column is `tsz`.
    let array = TimestampSecondArray::from(vec![0, 1_700_000_000]).with_timezone("Asia/Jakarta");
    let (enc, cells) = one(Arc::new(array));
    assert_eq!(enc, Encoding::Tsz);
    assert_eq!(
        cells,
        vec![ts(0, Some(25_200)), ts(1_700_000_000_000_000, Some(25_200))]
    );

    // Paris moves between +01:00 and +02:00: the offsets differ, so the column is
    // tagged, and each cell still carries its own offset.
    let winter = 1_704_067_200; // 2024-01-01T00:00:00Z
    let summer = 1_719_792_000; // 2024-07-01T00:00:00Z
    let array = TimestampSecondArray::from(vec![winter, summer]).with_timezone("Europe/Paris");
    let (enc, cells) = one(Arc::new(array));
    assert_eq!(enc, Encoding::Tagged);
    assert_eq!(
        cells,
        vec![
            ts(winter * 1_000_000, Some(3_600)),
            ts(summer * 1_000_000, Some(7_200))
        ]
    );
}

#[test]
fn an_unknown_zone_is_an_error_not_a_guess() {
    let array = TimestampSecondArray::from(vec![0]).with_timezone("Mars/Olympus");
    let batch = RecordBatch::try_from_iter(vec![("x", Arc::new(array) as ArrayRef)]).unwrap();
    assert!(matches!(
        from_arrow(&batch),
        Err(ColumnarError::Corrupt { .. })
    ));
}

#[test]
fn dates_times_and_durations() {
    let (enc, cells) = one(Arc::new(Date32Array::from(vec![19_000])));
    assert_eq!(
        (enc, cells),
        (Encoding::Date, vec![Value::Date { days: 19_000 }])
    );
    // Date64 is milliseconds since the epoch: a timestamp without a zone.
    let (enc, cells) = one(Arc::new(Date64Array::from(vec![86_400_000])));
    assert_eq!((enc, cells), (Encoding::Ts, vec![ts(86_400_000_000, None)]));

    let (enc, cells) = one(Arc::new(Time32SecondArray::from(vec![60])));
    assert_eq!(
        (enc, cells),
        (Encoding::Time, vec![Value::Time { micros: 60_000_000 }])
    );
    let (_, cells) = one(Arc::new(Time32MillisecondArray::from(vec![1_500])));
    assert_eq!(cells, vec![Value::Time { micros: 1_500_000 }]);
    let (_, cells) = one(Arc::new(Time64MicrosecondArray::from(vec![42])));
    assert_eq!(cells, vec![Value::Time { micros: 42 }]);
    let (_, cells) = one(Arc::new(Time64NanosecondArray::from(vec![42_999])));
    assert_eq!(cells, vec![Value::Time { micros: 42 }]);

    let interval = |micros| {
        Value::Interval(IntervalValue {
            months: 0,
            days: 0,
            micros,
        })
    };
    let (enc, cells) = one(Arc::new(DurationSecondArray::from(vec![2])));
    assert_eq!(
        (enc, cells),
        (Encoding::Interval, vec![interval(2_000_000)])
    );
    let (_, cells) = one(Arc::new(DurationMillisecondArray::from(vec![2])));
    assert_eq!(cells, vec![interval(2_000)]);
    let (_, cells) = one(Arc::new(DurationMicrosecondArray::from(vec![2])));
    assert_eq!(cells, vec![interval(2)]);
    let (_, cells) = one(Arc::new(DurationNanosecondArray::from(vec![2_500])));
    assert_eq!(cells, vec![interval(2)]);
}

#[test]
fn intervals_keep_their_parts() {
    let part = |months, days, micros| {
        Value::Interval(IntervalValue {
            months,
            days,
            micros,
        })
    };
    let (enc, cells) = one(Arc::new(IntervalYearMonthArray::from(vec![14])));
    assert_eq!((enc, cells), (Encoding::Interval, vec![part(14, 0, 0)]));
    let (_, cells) = one(Arc::new(IntervalDayTimeArray::from(vec![
        arrow_array::types::IntervalDayTime::new(3, 1_500),
    ])));
    assert_eq!(cells, vec![part(0, 3, 1_500_000)]);
    let (_, cells) = one(Arc::new(IntervalMonthDayNanoArray::from(vec![
        IntervalMonthDayNano::new(1, 2, 3_999),
    ])));
    assert_eq!(cells, vec![part(1, 2, 3)]);
}

#[test]
fn decimals_keep_their_digits() {
    let dec = |unscaled, scale| Value::Decimal { unscaled, scale };
    let array = Decimal128Array::from(vec![12_345i128])
        .with_precision_and_scale(10, 2)
        .unwrap();
    assert_eq!(one(Arc::new(array)), (Encoding::Dec, vec![dec(12_345, 2)]));
    let array = Decimal32Array::from(vec![-7i32])
        .with_precision_and_scale(5, 3)
        .unwrap();
    assert_eq!(one(Arc::new(array)).1, vec![dec(-7, 3)]);
    let array = Decimal64Array::from(vec![9i64])
        .with_precision_and_scale(15, 0)
        .unwrap();
    assert_eq!(one(Arc::new(array)).1, vec![dec(9, 0)]);
    // A negative scale is multiplied out: 12 at scale -3 is 12000.
    let array = Decimal128Array::from(vec![12i128])
        .with_precision_and_scale(10, -3)
        .unwrap();
    assert_eq!(one(Arc::new(array)).1, vec![dec(12_000, 0)]);

    // Decimal256 that fits an i128 stays a decimal; one that does not becomes its text.
    let big = i256::from_i128(i128::MAX)
        .checked_mul(i256::from_i128(1_000))
        .unwrap();
    let array = Decimal256Array::from(vec![i256::from_i128(5), big])
        .with_precision_and_scale(60, 2)
        .unwrap();
    let (enc, cells) = one(Arc::new(array));
    assert_eq!(
        enc,
        Encoding::Tagged,
        "a decimal and a text in one column are mixed"
    );
    assert_eq!(cells[0], dec(5, 2));
    assert_eq!(
        cells[1],
        Value::Text("1701411834604692317316873037158841057270.00".into())
    );
    let array = Decimal256Array::from(vec![big])
        .with_precision_and_scale(60, 2)
        .unwrap();
    let (enc, cells) = one(Arc::new(array));
    assert_eq!(enc, Encoding::Text);
    assert_eq!(
        cells[0],
        Value::Text("1701411834604692317316873037158841057270.00".into())
    );
}

#[test]
fn dictionaries_and_run_ends_open_to_their_values() {
    let keys = Int32Array::from(vec![Some(0), Some(1), None, Some(0)]);
    let values = StringArray::from(vec!["a", "b"]);
    let dict = DictionaryArray::<Int32Type>::try_new(keys, Arc::new(values)).unwrap();
    let (enc, cells) = one(Arc::new(dict));
    assert_eq!(enc, Encoding::Text);
    assert_eq!(
        cells,
        vec![
            Value::Text("a".into()),
            Value::Text("b".into()),
            Value::Null,
            Value::Text("a".into())
        ]
    );

    let run_ends = Int32Array::from(vec![2, 3]);
    let values = Int64Array::from(vec![10, 20]);
    let runs = RunArray::<Int32Type>::try_new(&run_ends, &values).unwrap();
    let (enc, cells) = one(Arc::new(runs));
    assert_eq!(enc, Encoding::I64);
    assert_eq!(cells, ints(&[Some(10), Some(10), Some(20)]));
}

#[test]
fn lists_structs_and_maps_are_tagged() {
    let mut builder = ListBuilder::new(Int32Builder::new());
    builder.append_value([Some(1), None]);
    builder.append_null();
    builder.append_value([]);
    let (enc, cells) = one(Arc::new(builder.finish()));
    assert_eq!(enc, Encoding::Tagged);
    assert_eq!(
        cells,
        vec![
            Value::Array(vec![Value::Int(1), Value::Null]),
            Value::Null,
            Value::Array(vec![])
        ]
    );

    let large =
        LargeListArray::from_iter_primitive::<Int32Type, _, _>(vec![Some(vec![Some(4), Some(5)])]);
    assert_eq!(
        one(Arc::new(large)).1,
        vec![Value::Array(vec![Value::Int(4), Value::Int(5)])]
    );

    let fixed = FixedSizeListArray::from_iter_primitive::<Int32Type, _, _>(
        vec![Some(vec![Some(1), Some(2)]), Some(vec![Some(3), Some(4)])],
        2,
    );
    assert_eq!(
        one(Arc::new(fixed)).1,
        vec![
            Value::Array(vec![Value::Int(1), Value::Int(2)]),
            Value::Array(vec![Value::Int(3), Value::Int(4)])
        ]
    );

    let fields = Fields::from(vec![
        Field::new("n", DataType::Int32, true),
        Field::new("s", DataType::Utf8, true),
    ]);
    let record = StructArray::new(
        fields,
        vec![
            Arc::new(Int32Array::from(vec![1, 2])) as ArrayRef,
            Arc::new(StringArray::from(vec![Some("x"), None])),
        ],
        Some(arrow_buffer::NullBuffer::from(vec![true, false])),
    );
    let (enc, cells) = one(Arc::new(record));
    assert_eq!(enc, Encoding::Tagged);
    assert_eq!(
        cells,
        vec![
            Value::Row(vec![Value::Int(1), Value::Text("x".into())]),
            Value::Null
        ]
    );

    let mut builder = MapBuilder::new(None, StringBuilder::new(), Int32Builder::new());
    builder.keys().append_value("a");
    builder.values().append_value(1);
    builder.keys().append_value("b");
    builder.values().append_null();
    builder.append(true).unwrap();
    let (enc, cells) = one(Arc::new(builder.finish()));
    assert_eq!(enc, Encoding::Tagged);
    assert_eq!(
        cells,
        vec![Value::Map(vec![
            (Value::Text("a".into()), Value::Int(1)),
            (Value::Text("b".into()), Value::Null),
        ])]
    );
}

#[test]
fn a_union_cell_is_the_active_childs_value() {
    let fields = UnionFields::try_new(
        vec![0, 1],
        vec![
            Field::new("i", DataType::Int32, true),
            Field::new("s", DataType::Utf8, true),
        ],
    )
    .unwrap();
    let union = UnionArray::try_new(
        fields,
        ScalarBuffer::from(vec![0i8, 1, 0]),
        Some(ScalarBuffer::from(vec![0i32, 0, 1])),
        vec![
            Arc::new(Int32Array::from(vec![10, 20])) as ArrayRef,
            Arc::new(StringArray::from(vec!["x"])),
        ],
    )
    .unwrap();
    assert_eq!(
        union.data_type().clone(),
        DataType::Union(
            UnionFields::try_new(
                vec![0, 1],
                vec![
                    Field::new("i", DataType::Int32, true),
                    Field::new("s", DataType::Utf8, true)
                ]
            )
            .unwrap(),
            UnionMode::Dense,
        )
    );
    let (enc, cells) = one(Arc::new(union));
    // An integer, a string, an integer: mixed, so tagged, and each cell is its own type.
    assert_eq!(enc, Encoding::Tagged);
    assert_eq!(
        cells,
        vec![Value::Int(10), Value::Text("x".into()), Value::Int(20)]
    );
}

#[test]
fn an_all_null_column_and_a_null_array() {
    let (enc, cells) = one(Arc::new(Int64Array::from(vec![None, None])));
    assert_eq!(
        (enc, cells),
        (Encoding::Null, vec![Value::Null, Value::Null])
    );
    let (enc, cells) = one(Arc::new(NullArray::new(3)));
    assert_eq!((enc, cells), (Encoding::Null, vec![Value::Null; 3]));
    let (enc, cells) = one(Arc::new(BooleanArray::from(vec![Some(true), None])));
    assert_eq!(
        (enc, cells),
        (Encoding::Bool, vec![Value::Bool(true), Value::Null])
    );
}

#[test]
fn a_batch_keeps_its_columns_in_order_and_names_them_positionally() {
    let batch = RecordBatch::try_from_iter(vec![
        ("dup", Arc::new(Int32Array::from(vec![1, 2])) as ArrayRef),
        ("dup", Arc::new(StringViewArray::from(vec!["a", "b"]))),
    ])
    .unwrap();
    let sealed = from_arrow(&batch).unwrap();
    assert_eq!(sealed.encodings, vec![Encoding::I64, Encoding::Text]);
    let names: Vec<_> = sealed
        .batch
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    assert_eq!(names, ["c0", "c1"]);
    assert_eq!(sealed.batch.num_rows(), 2);
}

#[test]
fn a_batch_with_no_columns_is_refused() {
    let batch = RecordBatch::try_new_with_options(
        Arc::new(Schema::empty()),
        vec![],
        &RecordBatchOptions::new().with_row_count(Some(3)),
    )
    .unwrap();
    assert!(from_arrow(&batch).is_err());
}

#[test]
fn an_interval_unit_that_the_table_does_not_name_still_has_a_home() {
    // `IntervalUnit` has exactly three variants; this pins that the match covers them all.
    for unit in [
        IntervalUnit::YearMonth,
        IntervalUnit::DayTime,
        IntervalUnit::MonthDayNano,
    ] {
        let array = arrow_array::new_null_array(&DataType::Interval(unit), 2);
        let (enc, cells) = one(array);
        assert_eq!(
            (enc, cells),
            (Encoding::Null, vec![Value::Null, Value::Null])
        );
    }
    // And a unit pair on timestamps: all four convert, none is dropped.
    for unit in [
        TimeUnit::Second,
        TimeUnit::Millisecond,
        TimeUnit::Microsecond,
        TimeUnit::Nanosecond,
    ] {
        let array = arrow_array::new_null_array(&DataType::Timestamp(unit, None), 1);
        assert_eq!(one(array).0, Encoding::Null);
    }
}
