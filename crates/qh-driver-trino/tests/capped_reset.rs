//! What a capped preview leaves behind, and what a failed `DELETE` does about it.
//!
//! A preview stops reading at its cap. The driver itself does not cancel anything on that
//! path (the session is pooled and the pool settles it at check-in): the cancel is the
//! `DELETE` that `reset()` sends when the pool takes the session back. These tests pin both
//! halves of that bargain against a coordinator the test stands up:
//!
//! * the happy half -- a capped preview *is* cancelled, and the coordinator sees the
//!   `DELETE` on the page URI it issued. Without this the claim "the pool stops a Trino
//!   query at check-in" would be a comment, not a fact.
//! * the leak -- when the `DELETE` fails, `reset()` still answers `Ok(())`, the running
//!   query is forgotten on our side, and nothing retries. The coordinator is left holding a
//!   query nobody will ever read. This is the failure the comment in `reset()` waves away
//!   with "Trino abandons the query on its own", and a query that keeps scanning a large
//!   table until its own run-time ceiling is exactly what that comment hides. **This test is
//!   red on purpose**: it demands the correct behaviour, so it fails until `reset()`
//!   propagates a refused cancel instead of swallowing it.
//!
//! The coordinator is a bare HTTP listener, as in `capabilities.rs`: the point is the
//! `DELETE` the client sends, not a real Trino.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qh_driver::{ConnectionConfig, Driver, DriverKind, ExecuteOptions, TlsMode};
use qh_driver_trino::TrinoDriver;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// A page that is *not* the end: it carries one row **and a `nextUri`**, which is the
/// protocol's own "there is more, poll again". A client that reads one batch and stops has
/// left this query unfinished at the coordinator -- exactly a capped preview.
fn running_page(address: SocketAddr) -> String {
    format!(
        r#"{{"id":"capped","nextUri":"http://{address}/v1/statement/page-2","columns":[{{"name":"n","type":"bigint"}}],"data":[[1]],"stats":{{"state":"RUNNING"}}}}"#
    )
}

/// A coordinator for a query that never finishes on its own: `POST` and every `GET` answer a
/// page with another `nextUri`. It records each request line and, for a `DELETE`, whether it
/// was asked to accept or fail it.
struct Coordinator {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
    deletes: Arc<AtomicBool>,
    _task: tokio::task::JoinHandle<()>,
}

impl Coordinator {
    /// A coordinator whose `DELETE` answers `delete_status`.
    async fn new(delete_status: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
        let address = listener.local_addr().expect("the bound address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let deletes = Arc::new(AtomicBool::new(false));

        let log = Arc::clone(&requests);
        let saw_delete = Arc::clone(&deletes);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let log = Arc::clone(&log);
                let saw_delete = Arc::clone(&saw_delete);
                tokio::spawn(async move {
                    let Some(head) = read_head(&mut stream).await else {
                        return;
                    };
                    let line = head.lines().next().unwrap_or_default().to_owned();
                    log.lock().expect("request log").push(line.clone());

                    // The two requests the protocol has, plus the cancel:
                    //   POST /v1/statement   -> the statement, answers a running page
                    //   GET  /v1/statement/… -> a page poll, answers another running page
                    //   DELETE /v1/statement/… -> the cancel reset() sends
                    let (status, body) = if line.starts_with("DELETE ") {
                        saw_delete.store(true, Ordering::SeqCst);
                        // A cancel the coordinator refuses: a 500, the shape of a DELETE that
                        // did not land (a proxy, a coordinator restart, an overloaded server).
                        if delete_status == "200 OK" {
                            ("200 OK", String::new())
                        } else {
                            (delete_status, "cancel rejected".to_owned())
                        }
                    } else {
                        // The statement (`POST`) and every page poll (`GET`) answer a page that
                        // still has a `nextUri`: the query never finishes on its own.
                        ("200 OK", running_page(address))
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
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
            deletes,
            _task: task,
        }
    }

    fn request_lines(&self) -> Vec<String> {
        self.requests.lock().expect("request log").clone()
    }

    fn deletes(&self) -> Vec<String> {
        self.request_lines()
            .into_iter()
            .filter(|line| line.starts_with("DELETE "))
            .collect()
    }

    fn saw_delete(&self) -> bool {
        self.deletes.load(Ordering::SeqCst)
    }
}

/// Read up to the end of the request head.
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

fn config(address: SocketAddr) -> ConnectionConfig {
    ConnectionConfig::new(DriverKind::Trino, "127.0.0.1", address.port(), "queryhive")
        .database("tpch")
        .schema("tiny")
        .tls(TlsMode::Disable)
}

/// Read a cursor's first batch and stop, the way a preview does under its cap.
async fn read_one_batch_then_stop(session: &mut Box<dyn qh_driver::Session>) {
    let mut cursor = session
        .execute("SELECT n FROM wide", &ExecuteOptions::default())
        .await
        .expect("execute");
    let batch = cursor.next_batch(16).await.expect("next_batch");
    assert!(batch.is_some(), "the first page should have a row");
    // Drop the cursor without draining it: a capped preview.
    drop(cursor);
}

// ---------------------------------------------------------------------------
// The happy half: a capped preview is cancelled when the pool resets the session.
// ---------------------------------------------------------------------------

/// The `DELETE` the comment promises. A session whose preview stopped at its cap is `reset`
/// when it goes back, and that reset is a `DELETE` on the page URI the coordinator issued.
/// If this ever fails, the pool's whole "settle it at check-in" story for Trino is dead.
#[tokio::test]
async fn a_capped_preview_is_cancelled_when_the_session_is_reset() {
    let coordinator = Coordinator::new("200 OK").await;
    let mut session = TrinoDriver::new()
        .connect(&config(coordinator.address))
        .await
        .expect("connect");

    read_one_batch_then_stop(&mut session).await;

    // This is what the pool does at check-in for a Trino session (pool.rs: checkin -> reset).
    // The coordinator accepted the cancel, so the reset is a clean success.
    session
        .reset()
        .await
        .expect("an accepted cancel resets clean");

    assert!(
        coordinator.saw_delete(),
        "the capped preview left a running query, and reset() never sent the DELETE that \
         stops it. Requests seen:\n{:#?}",
        coordinator.request_lines()
    );
    let deletes = coordinator.deletes();
    assert_eq!(deletes.len(), 1, "exactly one cancel: {deletes:?}");
    assert!(
        deletes[0].contains("page-2"),
        "the DELETE must name the page URI the coordinator issued, so it cancels THIS query: {}",
        deletes[0]
    );
}

// ---------------------------------------------------------------------------
// The leak: a DELETE that fails is forgotten, and nothing retries.
// ---------------------------------------------------------------------------

/// **RED -- this test fails until the leak is fixed.**
///
/// A capped preview whose cancel the coordinator *refuses*. Today `reset()` swallows the
/// failure (`let _ = self.delete_running()`), answers `Ok(())`, and forgets the running query;
/// nothing retries. The coordinator keeps scanning a table nobody will ever read, and the
/// driver hands the session back as if it were clean. That is the leak, and the comment in
/// `reset()` ("Trino abandons the query on its own") is what hides it -- a Trino query is not
/// abandoned because a client stopped listening; it runs to its own ceiling.
///
/// The test asks for the *correct* behaviour, so it is red on purpose:
///
/// * a reset whose cancel the coordinator refuses must NOT be reported as success -- it must
///   return the failure (or otherwise refuse to hand the session back), so the pool can
///   discard it and log it, instead of quietly reusing a session with a live straggler.
///
/// This is deliberately `#[ignore]`-free and expected to FAIL: the failing assertion is the
/// proof. When the fix lands, it goes green and no longer needs to be touched.
#[tokio::test]
async fn a_refused_cancel_must_not_be_reported_as_a_clean_reset() {
    let coordinator = Coordinator::new("500 Internal Server Error").await;
    let mut session = TrinoDriver::new()
        .connect(&config(coordinator.address))
        .await
        .expect("connect");

    read_one_batch_then_stop(&mut session).await;

    let reset = session.reset().await;

    // The proof the cancel was at least attempted: the leak is the *silence*, not a skipped
    // request. If this fires, the driver stopped trying to cancel at all -- a different bug.
    assert!(
        coordinator.saw_delete(),
        "reset() never sent the DELETE for a query it left running:\n{:#?}",
        coordinator.request_lines()
    );

    // THE RED ASSERTION. A cancel the coordinator refused means the query is still running
    // there; reporting the reset as a success is the leak. Until `reset()` propagates the
    // failure (or refuses the session) this assertion fails -- which is the point of this test.
    assert!(
        reset.is_err(),
        "a cancel the coordinator refused (500) was reported as a clean reset. The query is \
         still running at the coordinator and this session is handed back as reusable. \
         Deletes seen: {:#?}",
        coordinator.deletes()
    );

    // If reset() ever learns to fail here, the pool discards the session and this second
    // statement must never reach a server that is still holding the first query.
    if reset.is_err() {
        assert_eq!(
            coordinator.deletes().len(),
            1,
            "a failed reset must not leave a second statement on the same session: {:#?}",
            coordinator.deletes()
        );
    }
}
