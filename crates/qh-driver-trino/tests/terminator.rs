//! A statement's own terminator never reaches Trino.
//!
//! Trino 483 answers a trailing `;` with a syntax error (`line 1:39`), while the editor, PostgreSQL
//! and MySQL all treat it as the end of the statement. What is under test is the body this driver
//! POSTs to `/v1/statement`, seen at a listener that records it. No Trino is needed.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qh_core::EngineError;
use qh_driver::{ConnectionConfig, Driver, DriverKind, ExecuteOptions, TlsMode};
use qh_driver_trino::TrinoDriver;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A coordinator that records the body of every statement and answers `reply`.
struct Coordinator {
    address: SocketAddr,
    bodies: Arc<Mutex<Vec<String>>>,
    _task: tokio::task::JoinHandle<()>,
}

impl Coordinator {
    fn bodies(&self) -> Vec<String> {
        self.bodies.lock().expect("body log").clone()
    }
}

async fn coordinator(reply: &'static str) -> Coordinator {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let address = listener.local_addr().expect("the bound address");
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&bodies);
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let mut data = Vec::new();
                let mut buffer = [0u8; 4096];
                // The head, then as many bytes of body as it announces.
                let (end, announced) = loop {
                    let read = stream.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        return;
                    }
                    data.extend_from_slice(&buffer[..read]);
                    if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&data[..end]).to_ascii_lowercase();
                        let announced = head
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")?.trim().parse().ok()
                            })
                            .unwrap_or(0usize);
                        break (end + 4, announced);
                    }
                };
                while data.len() < end + announced {
                    let read = stream.read(&mut buffer).await.unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    data.extend_from_slice(&buffer[..read]);
                }
                log.lock()
                    .expect("body log")
                    .push(String::from_utf8_lossy(&data[end..]).into_owned());
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    Coordinator {
        address,
        bodies,
        _task: task,
    }
}

async fn session(address: SocketAddr) -> Box<dyn qh_driver::Session> {
    let config = ConnectionConfig::new(DriverKind::Trino, "127.0.0.1", address.port(), "queryhive")
        .database("tpch")
        .schema("tiny")
        .tls(TlsMode::Disable);
    TrinoDriver::new().connect(&config).await.expect("connect")
}

const FINISHED: &str = r#"{"id":"q","stats":{"state":"FINISHED"}}"#;

/// What Trino received for each of `statements`, in order.
async fn received(statements: &[&str]) -> Vec<String> {
    let coordinator = coordinator(FINISHED).await;
    let mut session = session(coordinator.address).await;
    for sql in statements {
        let options = ExecuteOptions::default();
        tokio::time::timeout(Duration::from_secs(5), session.execute(sql, &options))
            .await
            .expect("no hang")
            .unwrap_or_else(|error| panic!("{sql:?}: {error}"));
    }
    coordinator.bodies()
}

#[tokio::test]
async fn a_trailing_terminator_is_not_sent() {
    let sent = received(&[
        "SELECT 1;",
        "SELECT 1;  ",
        "SELECT 1; -- note",
        "SELECT 1;\n",
        "SELECT 1;;",
    ])
    .await;
    assert_eq!(
        sent,
        [
            "SELECT 1",
            "SELECT 1",
            "SELECT 1 -- note",
            "SELECT 1",
            "SELECT 1"
        ]
    );
}

#[tokio::test]
async fn text_that_is_not_a_terminator_is_sent_as_written() {
    let sent = received(&[
        "SELECT 'a;b;'",
        "SELECT 'a;b;';",
        "SELECT 1; SELECT 2",
        "SELECT 1",
    ])
    .await;
    assert_eq!(
        sent,
        [
            "SELECT 'a;b;'",
            "SELECT 'a;b;'",
            "SELECT 1; SELECT 2",
            "SELECT 1"
        ]
    );
}

/// Stripping these would leave an empty body, which Trino answers with an HTTP 400 in another
/// shape than before; they are sent as written so the server answers what it always did.
#[tokio::test]
async fn text_that_is_only_a_terminator_is_sent_as_written() {
    let statements = [";", ";;", "  ;  ", "\n;\n"];
    let sent = received(&statements).await;
    assert_eq!(sent, statements);
}

#[tokio::test]
async fn bound_execution_with_no_values_strips_too() {
    let coordinator = coordinator(FINISHED).await;
    let mut session = session(coordinator.address).await;
    session
        .execute_bound("SELECT 1;", &[], &ExecuteOptions::default())
        .await
        .expect("execute_bound");
    assert_eq!(coordinator.bodies(), ["SELECT 1"]);
}

/// The server's position counts into what it was sent, and the caller reads it against what they
/// wrote: dropping the terminator must not move the start of the text.
#[tokio::test]
async fn an_error_position_still_counts_from_the_callers_first_character() {
    let coordinator = coordinator(
        r#"{"id":"q","stats":{"state":"FAILED"},"error":{"message":"line 2:3: mismatched input","errorName":"SYNTAX_ERROR","errorType":"USER_ERROR","errorLocation":{"lineNumber":2,"columnNumber":3}}}"#,
    )
    .await;
    let mut session = session(coordinator.address).await;
    let outcome = session
        .execute("\n  SELEC 1;", &ExecuteOptions::default())
        .await;
    match outcome {
        Err(EngineError::Query { position, .. }) => assert_eq!(position, Some(4)),
        Err(other) => panic!("{other}"),
        Ok(_) => panic!("the coordinator refused the statement"),
    }
    assert_eq!(coordinator.bodies(), ["\n  SELEC 1"]);
}
