//! How often a running query is polled, pinned at the coordinator.
//!
//! Trino has no long poll: a poll for a query that is still planning comes back
//! with a fresh page URI and no rows. The client sets the pace, and the pace is
//! what decides how long a page which has just become ready waits to be noticed.
//!
//! A flat interval is the trap. An empty page does not mean the query is slow: a
//! client that asks faster than the workers fill the output buffer sees empty
//! pages throughout a healthy query, so a flat half second caps every such query
//! at two pages a second, which on a thousand-row page is two thousand rows a
//! second however fast the cluster was reading. The pause therefore starts small,
//! grows only while the pages stay empty, and is reset by the first one that
//! carries rows.
//!
//! No Trino is needed. What is under test is the pace this client sets, which is
//! visible at a listener that never finishes its query.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use qh_driver::{ConnectionConfig, Driver, DriverKind, ExecuteOptions, TlsMode};
use qh_driver_trino::TrinoDriver;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// How long the poll's pace is observed for.
const WINDOW: Duration = Duration::from_millis(1200);

/// The floor the pace is held to: the flat half second this replaced managed
/// three requests in this window (the statement and two polls), and a pace that
/// stayed there is the regression this file exists to catch.
const FEWEST_REQUESTS: usize = 7;

/// The ceiling the pace is held under: a client that stopped pausing would ask
/// hundreds of times a second on a local coordinator, and the pause exists so it
/// does not.
const MOST_REQUESTS: usize = 30;

/// A coordinator whose query never finishes and never produces a row.
struct Coordinator {
    address: SocketAddr,
    requests: Arc<AtomicUsize>,
    _task: tokio::task::JoinHandle<()>,
}

impl Coordinator {
    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

async fn coordinator() -> Coordinator {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let address = listener.local_addr().expect("the bound address");
    let requests = Arc::new(AtomicUsize::new(0));

    let counter = Arc::clone(&requests);
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer).await;
                // Still running, no `data`, and a `nextUri` that points back here:
                // the shape a Hive query has while its first split is being read.
                let body = format!(
                    r#"{{"id":"q","nextUri":"http://{address}/v1/statement/next","stats":{{"state":"RUNNING"}}}}"#
                );
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
        requests,
        _task: task,
    }
}

fn config(address: SocketAddr) -> ConnectionConfig {
    ConnectionConfig::new(DriverKind::Trino, "127.0.0.1", address.port(), "queryhive")
        .database("tpch")
        .schema("tiny")
        .tls(TlsMode::Disable)
}

#[tokio::test]
async fn a_query_with_no_rows_yet_is_polled_faster_than_the_old_flat_half_second() {
    let coordinator = coordinator().await;

    let mut session = TrinoDriver::new()
        .connect(&config(coordinator.address))
        .await
        .expect("connect");
    let mut cursor = session
        .execute("SELECT count(*) FROM wide", &ExecuteOptions::default())
        .await
        .expect("execute");

    // `next_batch` keeps asking while a page carries no rows, so the call is the
    // window: the timeout drops the future and the poll with it.
    let started = Instant::now();
    let outcome = tokio::time::timeout(WINDOW, cursor.next_batch(16)).await;
    let elapsed = started.elapsed();

    assert!(
        outcome.is_err(),
        "the query never finished, so this cannot have returned: {outcome:?} at {elapsed:?}"
    );

    let requests = coordinator.requests();
    assert!(
        requests >= FEWEST_REQUESTS,
        "{requests} requests in {elapsed:?} is the flat half second this replaced: the pause \
         grows while the pages stay empty, so a query that is producing must not be held to \
         the rate of one that is still planning"
    );
    assert!(
        requests <= MOST_REQUESTS,
        "{requests} requests in {elapsed:?} is a spin, not a poll"
    );
}
