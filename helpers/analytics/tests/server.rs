//! The protocol loop end to end, over a socket pair, with the test playing the app
//! (blueprint sections 14.3 to 14.6).

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::time::Duration;

use crate::common::StoreSource;
use datafusion::arrow::record_batch::RecordBatch;
use qh_analytics::server::{serve, ServeError, ServerConfig};
use qh_analytics::session::EngineConfig;
use qh_analytics_proto::*;
use qh_core::{ColumnBatch, ColumnMeta, Value};
use qh_result_store::{logical_schema, Outcome, StoreConfig, StoreRegistry};

struct App {
    to_helper: UnixStream,
    frames: mpsc::Receiver<Frame>,
    helper: Option<std::thread::JoinHandle<Result<(), ServeError>>>,
    _runtime: tokio::runtime::Runtime,
}

impl App {
    fn start(spill: Option<std::path::PathBuf>) -> App {
        App::start_capped(spill, MAX_FRAME_LEN as usize)
    }

    /// An app whose helper refuses to send a `Batch` payload above `max_chunk_bytes`.
    fn start_capped(spill: Option<std::path::PathBuf>, max_chunk_bytes: usize) -> App {
        let (app, helper_end) = UnixStream::pair().unwrap();
        let runtime = qh_rt::build_user_initiated("qh-sql-test").unwrap();
        let handle = runtime.handle().clone();
        let reader = helper_end.try_clone().unwrap();
        let mut engine = EngineConfig::new(spill);
        engine.target_partitions = 2;
        let helper = std::thread::spawn(move || {
            serve(
                reader,
                helper_end,
                handle,
                ServerConfig {
                    build: "test-build".into(),
                    engine,
                    max_chunk_bytes,
                    on_write_failure: || {},
                },
            )
        });
        let mut from_helper = app.try_clone().unwrap();
        let (sender, frames) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(Some(frame)) = read_frame(&mut from_helper) {
                if sender.send(frame).is_err() {
                    break;
                }
            }
        });
        App {
            to_helper: app,
            frames,
            helper: Some(helper),
            _runtime: runtime,
        }
    }

    fn send(&mut self, id: u64, message: &AppMessage) {
        write_control(&mut self.to_helper, id, message).unwrap();
        self.to_helper.flush().unwrap();
    }

    fn next(&self) -> Frame {
        self.frames
            .recv_timeout(Duration::from_secs(20))
            .expect("the helper said nothing for 20 s")
    }

    /// The next chunk request, skipping the column list and progress that may come first.
    fn request(&self) -> (u64, HelperMessage) {
        loop {
            let (id, message) = self.control();
            if matches!(message, HelperMessage::ChunkRequest { .. }) {
                return (id, message);
            }
            assert!(
                matches!(
                    message,
                    HelperMessage::ResultColumns { .. } | HelperMessage::Progress { .. }
                ),
                "expected a chunk request, got {message:?}"
            );
        }
    }

    fn control(&self) -> (u64, HelperMessage) {
        let frame = self.next();
        assert_eq!(
            frame.kind,
            Kind::Control,
            "expected a control frame, got {frame:?}"
        );
        (frame.id, decode_control(&frame.payload).unwrap())
    }

    /// Everything the helper says about query `id`, until `Done`. `answer` is called for
    /// each chunk request and decides what the "app" does about it.
    fn query(&mut self, id: u64, answer: &mut dyn FnMut(u64, &HelperMessage) -> Reply) -> Run {
        let mut run = Run::default();
        loop {
            let frame = self.next();
            match frame.kind {
                Kind::Batch => {
                    assert_eq!(frame.id, id);
                    run.chunks.push(decode_batch(frame.payload).unwrap());
                }
                Kind::Control => {
                    let message: HelperMessage = decode_control(&frame.payload).unwrap();
                    match &message {
                        HelperMessage::ChunkRequest { .. } => {
                            match answer(frame.id, &message) {
                                Reply::Batch(batch) => {
                                    write_batch(&mut self.to_helper, frame.id, &batch).unwrap()
                                }
                                Reply::Error(text) => write_control(
                                    &mut self.to_helper,
                                    frame.id,
                                    &AppMessage::ChunkError { message: text },
                                )
                                .unwrap(),
                                Reply::Silence => {}
                            }
                            self.to_helper.flush().unwrap();
                            run.requests += 1;
                        }
                        HelperMessage::ResultColumns { columns } => {
                            assert_eq!(frame.id, id);
                            run.columns = columns.clone();
                        }
                        HelperMessage::Progress { .. } => {}
                        HelperMessage::Done { .. } => {
                            assert_eq!(frame.id, id);
                            run.done = Some(message);
                            return run;
                        }
                        other => panic!("unexpected {other:?}"),
                    }
                }
            }
        }
    }
}

enum Reply {
    Batch(RecordBatch),
    Error(String),
    Silence,
}

#[derive(Default)]
struct Run {
    columns: Vec<WireColumn>,
    chunks: Vec<RecordBatch>,
    requests: usize,
    done: Option<HelperMessage>,
}

impl Run {
    fn done(&self) -> (u64, bool, bool, Option<WireError>) {
        match self.done.as_ref().unwrap() {
            HelperMessage::Done {
                rows,
                truncated,
                cancelled,
                error,
            } => (*rows, *truncated, *cancelled, error.clone()),
            _ => unreachable!(),
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Closing our end is the "app died" signal; the helper must end by itself.
        let _ = self.to_helper.shutdown(std::net::Shutdown::Both);
        if let Some(helper) = self.helper.take() {
            let result = helper.join().expect("the helper loop panicked");
            assert!(result.is_ok() || std::thread::panicking(), "{result:?}");
        }
    }
}

fn hello(app: &App) {
    let (id, message) = app.control();
    assert_eq!(id, 0);
    assert_eq!(
        message,
        HelperMessage::Hello {
            protocol: PROTOCOL_VERSION,
            build: "test-build".into()
        }
    );
}

fn store_of(
    chunks: usize,
    rows: usize,
) -> (qh_result_store::StoreHandle, std::sync::Arc<StoreSource>) {
    let registry = StoreRegistry::for_test(StoreConfig {
        spill_dir: None,
        ..StoreConfig::default()
    });
    let handle = registry.create();
    let writer = handle.writer();
    writer
        .begin(vec![
            ColumnMeta::new("k", "int8"),
            ColumnMeta::new("s", "text"),
        ])
        .unwrap();
    for chunk in 0..chunks {
        writer
            .push(
                &ColumnBatch::new(vec![
                    (0..rows)
                        .map(|r| Value::Int((chunk * rows + r) as i64))
                        .collect(),
                    (0..rows)
                        .map(|r| Value::Text(format!("v{r}").into()))
                        .collect(),
                ])
                .unwrap(),
            )
            .unwrap();
    }
    writer
        .finish(Outcome::Complete { truncated: false })
        .unwrap();
    let schema = logical_schema(&handle.shared().columns(), &handle.shared().stats());
    let source = StoreSource::new(&handle, schema);
    (handle, source)
}

fn register_result(app: &mut App, source: &StoreSource, rows: u64) {
    app.send(
        1,
        &AppMessage::RegisterResult {
            session: 7,
            name: "r".into(),
            token: "0123456789abcdef0123456789abcdef".into(),
            schema: (*source.schema.schema).clone(),
            rows,
            chunks: source.chunks.len() as u32,
        },
    );
}

#[test]
fn hello_then_a_result_queried_chunk_by_chunk() {
    let mut app = App::start(None);
    hello(&app);
    app.send(1, &AppMessage::OpenSession { session: 7 });
    assert_eq!(app.control(), (1, HelperMessage::Ack));

    let (_store, source) = store_of(5, 100);
    register_result(&mut app, &source, 500);
    match app.control() {
        (1, HelperMessage::TableInfo { info }) => {
            assert_eq!(info.name, "r");
            assert_eq!(info.rows, Some(500));
            assert_eq!(info.columns.len(), 2);
        }
        other => panic!("{other:?}"),
    }

    app.send(
        9,
        &AppMessage::RunSql {
            session: 7,
            sql: "SELECT sum(k), count(*) FROM r".into(),
            lease_bytes: 64 << 20,
            row_cap: 1_000,
        },
    );
    let mut requested: HashMap<u32, usize> = HashMap::new();
    let run = app.query(9, &mut |_, message| match message {
        HelperMessage::ChunkRequest {
            session,
            token,
            chunk,
            columns,
        } => {
            assert_eq!(*session, 7);
            assert_eq!(token, "0123456789abcdef0123456789abcdef");
            *requested.entry(*chunk).or_default() += 1;
            Reply::Batch(
                source
                    .chunk_batch(
                        *chunk,
                        &columns.iter().map(|&c| c as usize).collect::<Vec<_>>(),
                    )
                    .unwrap(),
            )
        }
        _ => unreachable!(),
    });
    assert_eq!(run.done(), (1, false, false, None));
    assert_eq!(run.requests, 5);
    assert_eq!(requested.len(), 5, "every chunk once");
    assert_eq!(run.columns.len(), 2);
    // The output chunk is in store encodings: readable with the app's own reader.
    let batch = &run.chunks[0];
    let enc = |i: usize| qh_columnar::encoding::encoding_of(batch.schema().field(i)).unwrap();
    assert_eq!(
        qh_columnar::value_at(batch.column(0).as_ref(), enc(0), 0).unwrap(),
        Value::Int(124_750)
    );
    assert_eq!(
        qh_columnar::value_at(batch.column(1).as_ref(), enc(1), 0).unwrap(),
        Value::Int(500)
    );
}

#[test]
fn a_file_is_registered_and_queried_and_the_row_cap_cuts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("n.csv");
    std::fs::write(&path, "n\n1\n2\n3\n4\n5\n").unwrap();
    let mut app = App::start(None);
    hello(&app);
    app.send(1, &AppMessage::OpenSession { session: 1 });
    app.control();
    app.send(
        2,
        &AppMessage::RegisterFile {
            session: 1,
            name: "f".into(),
            path: path.to_str().unwrap().into(),
            format: FileFormat::Csv {
                has_header: true,
                delimiter: b',',
                quote: b'"',
            },
        },
    );
    match app.control() {
        (2, HelperMessage::TableInfo { info }) => assert_eq!(info.rows, None),
        other => panic!("{other:?}"),
    }
    // 5 rows against a cap of 3: cut, and said so.
    app.send(
        3,
        &AppMessage::RunSql {
            session: 1,
            sql: "SELECT n FROM f ORDER BY n".into(),
            lease_bytes: 64 << 20,
            row_cap: 3,
        },
    );
    let run = app.query(3, &mut |_, _| Reply::Silence);
    assert_eq!(run.done(), (3, true, false, None));
    // Exactly the cap is not a cut.
    app.send(
        4,
        &AppMessage::RunSql {
            session: 1,
            sql: "SELECT n FROM f ORDER BY n".into(),
            lease_bytes: 64 << 20,
            row_cap: 5,
        },
    );
    assert_eq!(
        app.query(4, &mut |_, _| Reply::Silence).done(),
        (5, false, false, None)
    );
}

#[test]
fn a_failed_request_and_a_failed_query_are_answered_not_dropped() {
    let mut app = App::start(None);
    hello(&app);
    app.send(
        1,
        &AppMessage::RunSql {
            session: 99,
            sql: "SELECT 1".into(),
            lease_bytes: 64 << 20,
            row_cap: 10,
        },
    );
    // No such session: the query ends at once, with an error.
    let run = app.query(1, &mut |_, _| Reply::Silence);
    assert!(matches!(
        run.done(),
        (
            0,
            false,
            false,
            Some(WireError {
                kind: ErrorKind::InvalidArgument,
                ..
            })
        )
    ));

    app.send(2, &AppMessage::OpenSession { session: 5 });
    app.control();
    app.send(
        3,
        &AppMessage::RunSql {
            session: 5,
            sql: "SELEC 1".into(),
            lease_bytes: 64 << 20,
            row_cap: 10,
        },
    );
    let (rows, _, _, error) = app.query(3, &mut |_, _| Reply::Silence).done();
    assert_eq!(rows, 0);
    assert_eq!(error.unwrap().kind, ErrorKind::InvalidArgument);

    app.send(
        4,
        &AppMessage::Deregister {
            session: 5,
            name: "nothing".into(),
        },
    );
    assert!(matches!(app.control(), (4, HelperMessage::Failed { .. })));
}

#[test]
fn an_error_from_the_app_ends_the_query_with_its_words() {
    let mut app = App::start(None);
    hello(&app);
    app.send(1, &AppMessage::OpenSession { session: 7 });
    app.control();
    let (_store, source) = store_of(4, 50);
    register_result(&mut app, &source, 200);
    app.control();
    app.send(
        2,
        &AppMessage::RunSql {
            session: 7,
            sql: "SELECT count(k) FROM r".into(),
            lease_bytes: 64 << 20,
            row_cap: 10,
        },
    );
    let run = app.query(2, &mut |_, _| Reply::Error("the result was closed".into()));
    let (_, _, cancelled, error) = run.done();
    assert!(!cancelled);
    assert!(error.unwrap().message.contains("the result was closed"));
}

#[test]
fn cancel_ends_a_waiting_query_with_cancelled() {
    let mut app = App::start(None);
    hello(&app);
    app.send(1, &AppMessage::OpenSession { session: 7 });
    app.control();
    let (_store, source) = store_of(4, 50);
    register_result(&mut app, &source, 200);
    app.control();
    app.send(
        2,
        &AppMessage::RunSql {
            session: 7,
            sql: "SELECT count(k) FROM r".into(),
            lease_bytes: 64 << 20,
            row_cap: 10,
        },
    );
    // Wait until the helper is asking for chunks, never answer, then cancel.
    let (_, request) = app.request();
    assert!(matches!(request, HelperMessage::ChunkRequest { .. }));
    app.send(2, &AppMessage::Cancel);
    let run = app.query(2, &mut |_, _| Reply::Silence);
    assert_eq!(run.done(), (0, false, true, None));

    // A late answer to the abandoned request is ignored, and the helper still works.
    app.send(
        3,
        &AppMessage::RunSql {
            session: 7,
            sql: "SELECT 41 + 1".into(),
            lease_bytes: 64 << 20,
            row_cap: 10,
        },
    );
    assert_eq!(
        app.query(3, &mut |_, _| Reply::Silence).done(),
        (1, false, false, None)
    );
}

#[test]
fn the_same_query_id_twice_is_refused() {
    let mut app = App::start(None);
    hello(&app);
    app.send(1, &AppMessage::OpenSession { session: 7 });
    app.control();
    let (_store, source) = store_of(2, 10);
    register_result(&mut app, &source, 20);
    app.control();
    app.send(
        5,
        &AppMessage::RunSql {
            session: 7,
            sql: "SELECT count(k) FROM r".into(),
            lease_bytes: 64 << 20,
            row_cap: 10,
        },
    );
    let (_, first) = app.request();
    assert!(matches!(first, HelperMessage::ChunkRequest { .. }));
    app.send(
        5,
        &AppMessage::RunSql {
            session: 7,
            sql: "SELECT 1".into(),
            lease_bytes: 64 << 20,
            row_cap: 10,
        },
    );
    // The refusal is a `Done` with an error; the running query may still be asking for
    // chunks in between.
    let second = loop {
        let (_, message) = app.control();
        if matches!(message, HelperMessage::Done { .. }) {
            break message;
        }
    };
    assert!(
        matches!(second, HelperMessage::Done { error: Some(_), .. }),
        "{second:?}"
    );
    app.send(5, &AppMessage::Cancel);
}

#[test]
fn shutdown_is_acknowledged_and_ends_the_loop() {
    let mut app = App::start(None);
    hello(&app);
    app.send(4, &AppMessage::Shutdown);
    assert_eq!(app.control(), (4, HelperMessage::Ack));
    let result = app.helper.take().unwrap().join().unwrap();
    assert!(result.is_ok(), "{result:?}");
}

#[test]
fn a_frame_the_helper_cannot_read_is_a_violation_not_a_hang() {
    let mut app = App::start(None);
    hello(&app);
    // An unknown frame kind.
    let mut header = vec![0u8; HEADER_LEN];
    header[4] = 9;
    app.to_helper.write_all(&header).unwrap();
    let result = app.helper.take().unwrap().join().unwrap();
    assert!(
        matches!(
            result,
            Err(ServeError::Protocol(ProtoError::UnknownKind(9)))
        ),
        "{result:?}"
    );

    // A control frame that is not JSON of ours.
    let mut app = App::start(None);
    hello(&app);
    write_frame(&mut app.to_helper, Kind::Control, 1, b"{\"type\":\"nope\"}").unwrap();
    let result = app.helper.take().unwrap().join().unwrap();
    assert!(
        matches!(result, Err(ServeError::Protocol(ProtoError::BadControl(_)))),
        "{result:?}"
    );
}

#[test]
fn the_helper_ends_when_the_app_does() {
    // `Drop for App` closes the socket and requires the loop to return Ok.
    let app = App::start(None);
    hello(&app);
    drop(app);
}

#[test]
fn a_cell_too_big_for_a_frame_fails_that_query_and_the_helper_keeps_serving() {
    // 64 KiB stands in for the protocol's 256 MiB: one row cannot be cut, so a single
    // 200 KB cell makes a payload the helper cannot send.
    let mut app = App::start_capped(None, 64 * 1024);
    hello(&app);
    app.send(1, &AppMessage::OpenSession { session: 7 });
    assert_eq!(app.control(), (1, HelperMessage::Ack));
    app.send(
        2,
        &AppMessage::RunSql {
            session: 7,
            sql: "SELECT repeat('x', 200000) AS big".into(),
            lease_bytes: 64 << 20,
            row_cap: 10,
        },
    );
    let run = app.query(2, &mut |_, _| Reply::Silence);
    let (rows, _, _, error) = run.done();
    assert_eq!(rows, 0);
    let error = error.expect("Done must carry the error, not silence");
    assert_eq!(error.kind, ErrorKind::TooLarge);
    assert!(run.chunks.is_empty(), "nothing partial was sent");

    // Still alive: a control request is answered and a small query runs.
    app.send(3, &AppMessage::OpenSession { session: 8 });
    assert_eq!(app.control(), (3, HelperMessage::Ack));
    app.send(
        4,
        &AppMessage::RunSql {
            session: 8,
            sql: "SELECT 1 AS one".into(),
            lease_bytes: 64 << 20,
            row_cap: 10,
        },
    );
    let run = app.query(4, &mut |_, _| Reply::Silence);
    assert_eq!(run.done(), (1, false, false, None));
}

#[test]
fn a_failed_write_is_reported_to_the_process_instead_of_leaving_it_half_alive() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static FAILED: AtomicBool = AtomicBool::new(false);
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("the pipe is gone"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let (app, helper_end) = UnixStream::pair().unwrap();
    let runtime = qh_rt::build_user_initiated("qh-sql-test").unwrap();
    let handle = runtime.handle().clone();
    let helper = std::thread::spawn(move || {
        serve(
            helper_end,
            Broken,
            handle,
            ServerConfig {
                build: "test-build".into(),
                engine: EngineConfig::new(None),
                max_chunk_bytes: MAX_FRAME_LEN as usize,
                on_write_failure: || FAILED.store(true, Ordering::SeqCst),
            },
        )
    });
    // The Hello is the first thing written, and it fails.
    for _ in 0..500 {
        if FAILED.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(FAILED.load(Ordering::SeqCst), "nobody was told");
    drop(app);
    helper.join().unwrap().unwrap();
}
