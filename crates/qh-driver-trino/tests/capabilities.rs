//! The client capabilities the coordinator is told about, pinned at the request.
//!
//! `X-Trino-Client-Capabilities: PARAMETRIC_DATETIME` is what makes the coordinator
//! encode `timestamp` and `time` at their declared precision. A request without it
//! is answered in milliseconds and with the `(6)` gone from the type name, and
//! nothing reports an error -- a silent downgrade of every value in the result. So
//! what this file pins is the header on the wire: a listener reads the whole request
//! head of every request the driver makes, and a dropped header fails here rather
//! than only against a coordinator that happens to be running.
//!
//! No Trino is needed. What is under test is what this client sends, not what a
//! particular coordinator answers; the live half of the same claim is
//! `the_announced_capability_buys_microseconds_through_the_real_protocol` in
//! `integration.rs`, which runs only with `QH_TEST_TRINO=1`.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qh_driver::{ConnectionConfig, Driver, DriverKind, ExecuteOptions, TlsMode};
use qh_driver_trino::TrinoDriver;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// The poll's answer: the row, and no `nextUri`, which is the protocol's own "this
/// is the end".
const FINISHED: &str = r#"{"id":"q","columns":[{"name":"n","type":"bigint"}],"data":[[1]],"stats":{"state":"FINISHED"}}"#;

/// A coordinator that records every request head and answers a queued statement
/// followed by a finished page.
struct Coordinator {
    address: SocketAddr,
    heads: Arc<Mutex<Vec<String>>>,
    _task: tokio::task::JoinHandle<()>,
}

impl Coordinator {
    /// Every request head it read, in order: request line and headers, as sent.
    fn heads(&self) -> Vec<String> {
        self.heads.lock().expect("request log").clone()
    }
}

async fn coordinator() -> Coordinator {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let address = listener.local_addr().expect("the bound address");
    let heads = Arc::new(Mutex::new(Vec::new()));

    let log = Arc::clone(&heads);
    let task = tokio::spawn(async move {
        let mut served = 0usize;
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let log = Arc::clone(&log);
            let index = served;
            served += 1;
            tokio::spawn(async move {
                if let Some(head) = read_head(&mut stream).await {
                    log.lock().expect("request log").push(head);
                }
                // The statement answers queued with a `nextUri` on this same
                // authority, so the driver's next request is a page poll -- which is
                // the second request this file is here to read.
                let body = if index == 0 {
                    format!(
                        r#"{{"id":"q","nextUri":"http://{address}/v1/statement/next","stats":{{"state":"RUNNING"}}}}"#
                    )
                } else {
                    FINISHED.to_owned()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });

    Coordinator {
        address,
        heads,
        _task: task,
    }
}

/// Read up to the end of the request head, and return it.
///
/// The whole head, not just the request line: the header is the subject, so a
/// helper that read one line would be unable to see it.
async fn read_head<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> Option<String> {
    let mut head = String::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read =
            match tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer)).await {
                Ok(Ok(read)) => read,
                _ => return None,
            };
        if read == 0 {
            return None;
        }
        head.push_str(&String::from_utf8_lossy(&buffer[..read]));
        if head.contains("\r\n\r\n") {
            return Some(head);
        }
    }
}

/// One header's value, case-insensitively, as an HTTP header name is.
fn header(head: &str, name: &str) -> Option<String> {
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().to_owned())
    })
}

fn config(address: SocketAddr) -> ConnectionConfig {
    ConnectionConfig::new(DriverKind::Trino, "127.0.0.1", address.port(), "queryhive")
        .database("tpch")
        .schema("tiny")
        .tls(TlsMode::Disable)
}

#[tokio::test]
async fn the_statement_and_its_pages_all_announce_parametric_datetime() {
    // Both requests the protocol has: the statement's `POST` and the page poll.
    // Measured on 483, the POST is the request that settles the encoding (a poll
    // without the header still came back `timestamp(6)`), but the header is sent on
    // every statement-protocol request -- the reference client sends it on every
    // request too, and a coordinator that read it per response would otherwise
    // downgrade the middle of a result set. So both are pinned: a path that forgets
    // it is a path that gets a silently downgraded answer, and this is the test that
    // notices.
    let coordinator = coordinator().await;

    let mut session = TrinoDriver::new()
        .connect(&config(coordinator.address))
        .await
        .expect("connect");
    let mut cursor = session
        .execute("SELECT 1", &ExecuteOptions::default())
        .await
        .expect("execute");
    let mut rows = 0usize;
    while let Some(batch) = cursor.next_batch(16).await.expect("next_batch") {
        rows += batch.columns().first().map_or(0, Vec::len);
    }
    assert_eq!(rows, 1, "the queued page's row should have arrived");

    let heads = coordinator.heads();
    assert_eq!(
        heads.len(),
        2,
        "the statement and one page poll, nothing else: {heads:?}"
    );
    assert!(heads[0].starts_with("POST /v1/statement"), "{}", heads[0]);
    assert!(
        heads[1].starts_with("GET /v1/statement/next"),
        "the second request should be the page poll: {}",
        heads[1]
    );

    for head in &heads {
        assert_eq!(
            header(head, "X-Trino-Client-Capabilities").as_deref(),
            Some("PARAMETRIC_DATETIME"),
            "the capability that keeps microseconds was not announced in:\n{head}"
        );
        assert_eq!(
            header(head, "X-Trino-User").as_deref(),
            Some("queryhive"),
            "the user travels in the same place and must not be lost with it:\n{head}"
        );
    }
}

#[tokio::test]
async fn the_metadata_requests_announce_it_too() {
    // A browse call reaches the coordinator through the same request builders as a
    // user's statement, which is the point of the header living in one place rather
    // than at each call site: the browse path is the one nobody looks at when they
    // change the query path.
    let coordinator = coordinator().await;

    let mut session = TrinoDriver::new()
        .connect(&config(coordinator.address))
        .await
        .expect("connect");
    // The listener answers every statement with the same queued page, so the
    // browser's own row fetch is not what is under test -- the request it sent is.
    let names = session
        .browse(
            qh_driver::BrowseLevel::Catalog,
            &qh_driver::ObjectPath::new(),
            false,
        )
        .await
        .expect("the listener answers the browse statement like any other");

    let heads = coordinator.heads();
    assert_eq!(heads.len(), 2, "the statement and its page: {heads:?}");
    assert!(heads[0].starts_with("POST /v1/statement"), "{}", heads[0]);
    for head in &heads {
        assert_eq!(
            header(head, "X-Trino-Client-Capabilities").as_deref(),
            Some("PARAMETRIC_DATETIME"),
            "a browse request must carry the same announcement:\n{head}"
        );
    }
    assert_eq!(names, vec!["1".to_owned()], "the listener's own row");
}
