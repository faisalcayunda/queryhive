//! Round trips for every message, and the ways a frame can be wrong. None of the
//! malformed inputs may panic: the app reads these bytes from a process it does not
//! trust to be correct.

use std::io::Cursor;
use std::sync::Arc;

use arrow_array::{ArrayRef, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use qh_analytics_proto::*;

fn roundtrip<T>(message: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let mut wire = Vec::new();
    write_control(&mut wire, 77, message).unwrap();
    let frame = read_frame(&mut Cursor::new(wire)).unwrap().unwrap();
    assert_eq!(frame.kind, Kind::Control);
    assert_eq!(frame.id, 77);
    let back: T = decode_control(&frame.payload).unwrap();
    assert_eq!(&back, message);
}

fn logical_schema() -> Schema {
    Schema::new(vec![
        Field::new("id", DataType::Int64, true),
        Field::new(
            "at",
            DataType::Timestamp(TimeUnit::Microsecond, Some("+07:00".into())),
            true,
        ),
        Field::new("price", DataType::Decimal128(38, 4), true),
        Field::new("tags", DataType::Utf8, true)
            .with_metadata([("ARROW:extension:name".to_owned(), "arrow.json".to_owned())].into()),
    ])
}

#[test]
fn every_app_message_round_trips() {
    for message in [
        AppMessage::OpenSession { session: 1 },
        AppMessage::CloseSession { session: 1 },
        AppMessage::RegisterResult {
            session: 1,
            name: "r".into(),
            token: "00112233445566778899aabbccddeeff".into(),
            schema: logical_schema(),
            rows: 500_000,
            chunks: 31,
        },
        AppMessage::RegisterFile {
            session: 1,
            name: "f".into(),
            path: "/tmp/a.csv".into(),
            format: FileFormat::Csv {
                has_header: true,
                delimiter: b';',
                quote: b'"',
            },
        },
        AppMessage::RegisterFile {
            session: 1,
            name: "p".into(),
            path: "/tmp/a.parquet".into(),
            format: FileFormat::Parquet,
        },
        AppMessage::Deregister {
            session: 1,
            name: "r".into(),
        },
        AppMessage::RunSql {
            session: 1,
            sql: "select 1".into(),
            lease_bytes: 128 << 20,
            row_cap: 5_000_000,
        },
        AppMessage::Cancel,
        AppMessage::ChunkError {
            message: "the result was closed".into(),
        },
        AppMessage::Shutdown,
    ] {
        roundtrip(&message);
    }
}

#[test]
fn every_helper_message_round_trips() {
    let error = WireError {
        kind: ErrorKind::InvalidArgument,
        message: "no such table".into(),
        position: Some(Position {
            line: 1,
            column: 15,
        }),
    };
    let info = TableInfo {
        name: "r".into(),
        columns: vec![WireColumn {
            name: "id".into(),
            type_name: "Int64".into(),
        }],
        rows: Some(3),
    };
    for message in [
        HelperMessage::Hello {
            protocol: PROTOCOL_VERSION,
            build: "1.2.3".into(),
        },
        HelperMessage::Ack,
        HelperMessage::Failed {
            error: error.clone(),
        },
        HelperMessage::TableInfo { info },
        HelperMessage::ChunkRequest {
            session: 1,
            token: "ab".into(),
            chunk: 7,
            columns: vec![0, 2],
        },
        HelperMessage::ResultColumns { columns: vec![] },
        HelperMessage::Progress { rows: 65_536 },
        HelperMessage::Done {
            rows: 10,
            truncated: false,
            cancelled: true,
            error: Some(WireError {
                kind: ErrorKind::TooLarge,
                message: "budget".into(),
                position: None,
            }),
        },
    ] {
        roundtrip(&message);
    }
}

#[test]
fn a_batch_survives_the_wire() {
    let schema = Arc::new(Schema::new(vec![
        Field::new("a", DataType::Int64, true),
        Field::new("b", DataType::Utf8, true),
    ]));
    let columns: Vec<ArrayRef> = vec![
        Arc::new(Int64Array::from(vec![Some(1), None, Some(3)])),
        Arc::new(StringArray::from(vec![Some("x"), Some("y"), None])),
    ];
    let batch = RecordBatch::try_new(schema, columns).unwrap();
    let mut wire = Vec::new();
    write_batch(&mut wire, 9, &batch).unwrap();
    let frame = read_frame(&mut Cursor::new(wire)).unwrap().unwrap();
    assert_eq!((frame.kind, frame.id), (Kind::Batch, 9));
    assert_eq!(decode_batch(frame.payload).unwrap(), batch);
}

#[test]
fn two_frames_in_a_row_then_a_clean_eof() {
    let mut wire = Vec::new();
    write_frame(&mut wire, Kind::Control, 1, b"{}").unwrap();
    write_frame(&mut wire, Kind::Batch, 2, b"").unwrap();
    let mut reader = Cursor::new(wire);
    assert_eq!(read_frame(&mut reader).unwrap().unwrap().id, 1);
    assert_eq!(read_frame(&mut reader).unwrap().unwrap().id, 2);
    assert!(read_frame(&mut reader).unwrap().is_none());
}

#[test]
fn a_truncated_frame_is_an_error() {
    let mut wire = Vec::new();
    write_frame(&mut wire, Kind::Control, 1, b"{\"type\":\"cancel\"}").unwrap();
    // Every proper prefix that is not empty must fail, inside the header and inside
    // the payload alike.
    for cut in 1..wire.len() {
        let result = read_frame(&mut Cursor::new(&wire[..cut]));
        assert!(
            matches!(result, Err(ProtoError::Truncated)),
            "a frame cut at {cut} of {} gave {result:?}",
            wire.len()
        );
    }
}

#[test]
fn a_length_above_the_limit_is_refused_before_it_is_read() {
    let mut header = vec![0u8; HEADER_LEN];
    header[..4].copy_from_slice(&(MAX_FRAME_LEN + 1).to_le_bytes());
    header[4] = Kind::Control as u8;
    let result = read_frame(&mut Cursor::new(header));
    assert!(
        matches!(result, Err(ProtoError::TooLarge { .. })),
        "{result:?}"
    );

    // At the limit the header is accepted and the missing payload is what fails.
    let mut header = vec![0u8; HEADER_LEN];
    header[..4].copy_from_slice(&MAX_FRAME_LEN.to_le_bytes());
    header[4] = Kind::Control as u8;
    let result = read_frame(&mut Cursor::new(header));
    assert!(matches!(result, Err(ProtoError::Truncated)), "{result:?}");
}

#[test]
fn an_unknown_kind_is_an_error() {
    for kind in [0u8, 3, 4, 255] {
        let mut wire = vec![0u8; HEADER_LEN];
        wire[4] = kind;
        let result = read_frame(&mut Cursor::new(wire));
        assert!(
            matches!(result, Err(ProtoError::UnknownKind(k)) if k == kind),
            "{result:?}"
        );
    }
}

#[test]
fn broken_json_is_an_error() {
    for payload in [
        &b""[..],
        b"{",
        b"[]",
        b"{\"type\":\"nope\"}",
        b"{\"type\":\"run_sql\"}",
        b"\xff\xfe",
    ] {
        assert!(
            decode_control::<AppMessage>(payload).is_err(),
            "{payload:?}"
        );
        assert!(
            decode_control::<HelperMessage>(payload).is_err(),
            "{payload:?}"
        );
    }
}

#[test]
fn a_damaged_batch_is_an_error_not_a_panic() {
    let schema = Arc::new(Schema::new(vec![Field::new("a", DataType::Int64, true)]));
    let batch = RecordBatch::try_new(
        schema,
        vec![Arc::new(Int64Array::from(vec![1, 2, 3])) as ArrayRef],
    )
    .unwrap();
    let good = encode_batch(&batch).unwrap();

    assert!(decode_batch(Vec::new()).is_err());
    assert!(decode_batch(b"not arrow at all".to_vec()).is_err());
    // Truncations, and a flip of every byte in turn: any outcome except a panic is
    // acceptable, because a flipped data byte is still a valid batch.
    for cut in 0..good.len() {
        let _ = decode_batch(good[..cut].to_vec());
    }
    for at in 0..good.len() {
        let mut damaged = good.clone();
        damaged[at] ^= 0xff;
        let _ = decode_batch(damaged);
    }
    // Two batches in one stream are refused: a frame carries exactly one.
    let mut two = Vec::new();
    {
        let mut writer =
            arrow_ipc::writer::StreamWriter::try_new(&mut two, &batch.schema()).unwrap();
        writer.write(&batch).unwrap();
        writer.write(&batch).unwrap();
        writer.finish().unwrap();
    }
    assert!(decode_batch(two).is_err());
}
