//! CSV and Parquet as tables, and what comes out of them (blueprint sections 14.11, 5.6).

use std::fs::File;
use std::sync::Arc;

use crate::common::{engine, run};
use datafusion::arrow::array::builder::Int32Builder;
use datafusion::arrow::array::{
    ArrayRef, BinaryArray, BooleanArray, Date32Array, Decimal128Array, Float32Array, Int32Array,
    Int64Array, ListBuilder, StringArray, TimestampMillisecondArray, TimestampNanosecondArray,
};
use datafusion::arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::parquet::arrow::ArrowWriter;
use datafusion::parquet::basic::{Compression, ZstdLevel};
use datafusion::parquet::file::properties::WriterProperties;
use qh_analytics_proto::{ErrorKind, FileFormat};
use qh_core::Value;

fn csv(has_header: bool, delimiter: u8) -> FileFormat {
    FileFormat::Csv {
        has_header,
        delimiter,
        quote: b'"',
    }
}

fn text(value: &str) -> Value {
    Value::Text(value.into())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_csv_file_is_a_table_and_its_output_is_in_store_encodings() {
    let (engine, dir) = engine(2);
    let path = dir.path().join("people.csv");
    std::fs::write(
        &path,
        "id;name;score;born\n1;alpha;1.5;2024-01-02\n2;beta;;2024-02-03\n3;\"gam;ma\";3.25;\n",
    )
    .unwrap();
    let session = engine.session();
    let info = session
        .register_file("people", path.to_str().unwrap(), &csv(true, b';'))
        .await
        .unwrap();
    assert_eq!(info.rows, None);
    let names: Vec<_> = info.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["id", "name", "score", "born"]);

    let _lease = engine.grant(64 << 20);
    let (out, outcome) = run(&session, "SELECT * FROM people ORDER BY id", 1_000).await;
    assert_eq!(outcome.error, None);
    assert_eq!(outcome.rows, 3);
    assert_eq!(out.encodings(), ["i64", "text", "f64", "date"]);
    let cells = out.cells();
    assert_eq!(
        cells[0],
        vec![
            Value::Int(1),
            text("alpha"),
            Value::Float(1.5),
            Value::Date { days: 19_724 }
        ]
    );
    assert_eq!(cells[1][2], Value::Null);
    assert_eq!(cells[2][1], text("gam;ma"));
    assert_eq!(cells[2][3], Value::Null);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_is_read_whatever_it_is_called_and_a_bracket_is_not_a_glob() {
    let (engine, dir) = engine(2);
    let session = engine.session();
    let _lease = engine.grant(64 << 20);
    for name in ["data[1].tsv", "notes.txt", "no extension"] {
        let path = dir.path().join(name);
        std::fs::write(&path, "a\tb\n1\tx\n2\ty\n").unwrap();
        let info = session
            .register_file("f", path.to_str().unwrap(), &csv(true, b'\t'))
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(info.columns.len(), 2, "{name}");
        let (out, outcome) = run(&session, "SELECT count(*) FROM f", 10).await;
        assert_eq!(outcome.error, None, "{name}");
        assert_eq!(out.cells(), vec![vec![Value::Int(2)]], "{name}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_directory_reads_the_files_that_match_and_no_others() {
    let (engine, dir) = engine(2);
    let folder = dir.path().join("parts");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("a.csv"), "n\n1\n2\n").unwrap();
    std::fs::write(folder.join("b.csv"), "n\n3\n").unwrap();
    std::fs::write(folder.join("readme.txt"), "not,data\nat,all\n").unwrap();
    let session = engine.session();
    session
        .register_file("parts", folder.to_str().unwrap(), &csv(true, b','))
        .await
        .unwrap();
    let _lease = engine.grant(64 << 20);
    let (out, outcome) = run(&session, "SELECT sum(n) FROM parts", 10).await;
    assert_eq!(outcome.error, None);
    assert_eq!(out.cells(), vec![vec![Value::Int(6)]]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_paths_and_compressed_csv_are_refused_in_plain_words() {
    let (engine, dir) = engine(2);
    let session = engine.session();
    let gz = dir.path().join("big.csv.gz");
    std::fs::write(&gz, [0x1f, 0x8b, 0, 0]).unwrap();
    let error = session
        .register_file("z", gz.to_str().unwrap(), &csv(true, b','))
        .await
        .unwrap_err();
    assert_eq!(error.0.kind, ErrorKind::InvalidArgument);
    assert!(
        error.0.message.contains("Compressed CSV"),
        "{}",
        error.0.message
    );

    for path in ["/nonexistent/file.csv", "relative/file.csv"] {
        let error = session
            .register_file("n", path, &csv(true, b','))
            .await
            .unwrap_err();
        assert_eq!(error.0.kind, ErrorKind::InvalidArgument, "{path}");
    }
    assert!(
        session.tables().is_empty(),
        "a refused file leaves no table behind"
    );
}

fn parquet_fixture(path: &std::path::Path, compression: Compression) {
    let mut lists = ListBuilder::new(Int32Builder::new());
    lists.append_value([Some(1), Some(2)]);
    lists.append_null();
    lists.append_value([]);
    let columns: Vec<(&str, ArrayRef)> = vec![
        (
            "i32",
            Arc::new(Int32Array::from(vec![Some(3), None, Some(1)])),
        ),
        (
            "f32",
            Arc::new(Float32Array::from(vec![Some(0.1f32), None, Some(2.5)])),
        ),
        (
            "s",
            Arc::new(StringArray::from(vec![Some("é"), None, Some("a")])),
        ),
        (
            "d",
            Arc::new(Date32Array::from(vec![Some(19_000), None, Some(0)])),
        ),
        (
            "ts_ms",
            Arc::new(
                TimestampMillisecondArray::from(vec![Some(1_700_000_000_123), None, Some(0)])
                    .with_timezone("UTC"),
            ),
        ),
        (
            "ts_ns",
            Arc::new(TimestampNanosecondArray::from(vec![
                Some(1_999),
                None,
                Some(-1),
            ])),
        ),
        (
            "dec",
            Arc::new(
                Decimal128Array::from(vec![Some(12_345i128), None, Some(-5)])
                    .with_precision_and_scale(10, 2)
                    .unwrap(),
            ),
        ),
        (
            "b",
            Arc::new(BooleanArray::from(vec![Some(true), None, Some(false)])),
        ),
        (
            "bin",
            Arc::new(BinaryArray::from(vec![
                Some(&[0u8, 255][..]),
                None,
                Some(&[][..]),
            ])),
        ),
        ("l", Arc::new(lists.finish())),
        (
            "i64",
            Arc::new(Int64Array::from(vec![Some(1), Some(2), Some(3)])),
        ),
    ];
    let batch = RecordBatch::try_from_iter_with_nullable(
        columns.into_iter().map(|(name, array)| (name, array, true)),
    )
    .unwrap();
    let properties = WriterProperties::builder()
        .set_compression(compression)
        .build();
    let mut writer = ArrowWriter::try_new(
        File::create(path).unwrap(),
        batch.schema(),
        Some(properties),
    )
    .unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parquet_in_every_codec_normalises_every_type() {
    let (engine, dir) = engine(2);
    let session = engine.session();
    let _lease = engine.grant(64 << 20);
    for (name, compression) in [
        ("plain", Compression::UNCOMPRESSED),
        ("snappy", Compression::SNAPPY),
        ("zstd", Compression::ZSTD(ZstdLevel::default())),
    ] {
        let path = dir.path().join(format!("{name}.parquet"));
        parquet_fixture(&path, compression);
        session
            .register_file(name, path.to_str().unwrap(), &FileFormat::Parquet)
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let sql = format!("SELECT * FROM {name} ORDER BY i64");
        let (out, outcome) = run(&session, &sql, 100).await;
        assert_eq!(outcome.error, None, "{name}");
        assert_eq!(outcome.rows, 3, "{name}");
        assert_eq!(
            out.encodings(),
            ["i64", "f64", "text", "date", "tsz", "ts", "dec", "bool", "bytes", "tagged", "i64"],
            "{name}"
        );
        let cells = out.cells();
        // Row 0: int32 -> i64, float32 -> shortest text, strings, a date, a UTC timestamp
        // in microseconds, a nanosecond one floored to microseconds, a decimal, a list.
        assert_eq!(cells[0][0], Value::Int(3), "{name}");
        assert_eq!(cells[0][1], Value::Float(0.1), "{name}");
        assert_eq!(cells[0][2], text("é"), "{name}");
        assert_eq!(cells[0][3], Value::Date { days: 19_000 }, "{name}");
        assert_eq!(
            cells[0][4],
            Value::Timestamp {
                micros: 1_700_000_000_123_000,
                offset_secs: Some(0)
            },
            "{name}"
        );
        assert_eq!(
            cells[0][5],
            Value::Timestamp {
                micros: 1,
                offset_secs: None
            },
            "{name}"
        );
        assert_eq!(
            cells[0][6],
            Value::Decimal {
                unscaled: 12_345,
                scale: 2
            },
            "{name}"
        );
        assert_eq!(cells[0][7], Value::Bool(true), "{name}");
        assert_eq!(cells[0][8], Value::Bytes(vec![0, 255]), "{name}");
        assert_eq!(
            cells[0][9],
            Value::Array(vec![Value::Int(1), Value::Int(2)]),
            "{name}"
        );
        // Row 1 is NULL in every column but the key.
        assert!(
            cells[1][..10].iter().all(|cell| *cell == Value::Null),
            "{name}: {:?}",
            cells[1]
        );
        // Row 2: -1 ns floors to -1 us, and an empty list is an empty array.
        assert_eq!(
            cells[2][5],
            Value::Timestamp {
                micros: -1,
                offset_secs: None
            },
            "{name}"
        );
        assert_eq!(cells[2][9], Value::Array(vec![]), "{name}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_column_types_of_a_parquet_file_are_reported_to_the_app() {
    let (engine, dir) = engine(2);
    let session = engine.session();
    let path = dir.path().join("t.parquet");
    parquet_fixture(&path, Compression::SNAPPY);
    let info = session
        .register_file("t", path.to_str().unwrap(), &FileFormat::Parquet)
        .await
        .unwrap();
    let find = |name: &str| {
        info.columns
            .iter()
            .find(|c| c.name == name)
            .unwrap()
            .type_name
            .clone()
    };
    assert_eq!(find("i32"), DataType::Int32.to_string());
    assert_eq!(
        find("ts_ns"),
        DataType::Timestamp(TimeUnit::Nanosecond, None).to_string()
    );
    let _ = Field::new("x", DataType::Null, true);
    let _ = Schema::empty();
}
