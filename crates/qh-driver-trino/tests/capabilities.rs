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
//!
//! The second header the same listener pins is `Authorization`. A coordinator with
//! password-file or LDAP auth answers 401 to a request that carries only
//! `X-Trino-User`, which looks from the connection editor exactly like a wrong
//! password. Every request kind the protocol has is checked here -- the statement's
//! `POST`, the page poll, the `DELETE` that cancels -- because the credential lives
//! at the request builder and a new request path that skipped it would otherwise
//! fail only against a coordinator nobody runs in tests.

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

/// The same connection, with a password and a username that is not the default.
///
/// Both halves differ from `config` on purpose: a credential assembled from the
/// wrong half -- a default user, or a password paired with nobody -- is exactly the
/// bug this pins, and a real coordinator answers it with the same 401 as a wrong
/// password.
fn password_config(address: SocketAddr) -> ConnectionConfig {
    ConnectionConfig::new(
        DriverKind::Trino,
        "127.0.0.1",
        address.port(),
        "analytic_user",
    )
    .database("tpch")
    .schema("tiny")
    .tls(TlsMode::Disable)
    .password("hunter2")
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

/// `Base64("analytic_user:hunter2")`, the value `Basic` auth has to carry.
///
/// Reproduce with `printf 'analytic_user:hunter2' | base64`. Written out rather
/// than computed here so the assertion states the exact bytes on the wire: a
/// helper that encoded the same way the driver does would agree with the driver
/// even when both were wrong.
const BASIC: &str = "Basic YW5hbHl0aWNfdXNlcjpodW50ZXIy";

#[tokio::test]
async fn a_password_reaches_the_coordinator_as_basic_auth_on_the_statement_and_its_page() {
    // The bug this pins: a coordinator with password-file auth answers 401 to every
    // request that carries only `X-Trino-User`, and from the connection editor that
    // is indistinguishable from a wrong password. `X-Trino-User` alone names a user;
    // it authenticates nobody.
    let coordinator = coordinator().await;

    let mut session = TrinoDriver::new()
        .connect(&password_config(coordinator.address))
        .await
        .expect("connect");
    let mut cursor = session
        .execute("SELECT 1", &ExecuteOptions::default())
        .await
        .expect("execute");
    while cursor.next_batch(16).await.expect("next_batch").is_some() {}
    // Drop the cursor before the heads are read: the page poll is a request of its
    // own, and its head has to be in the log by the time it is asserted on.
    drop(cursor);

    let heads = coordinator.heads();
    assert!(heads.len() >= 2, "the statement and a page poll: {heads:?}");
    assert!(heads[0].starts_with("POST /v1/statement"), "{}", heads[0]);
    assert!(
        heads[1].starts_with("GET /v1/statement/next"),
        "the second request should be the page poll: {}",
        heads[1]
    );

    for head in &heads {
        assert_eq!(
            header(head, "Authorization").as_deref(),
            Some(BASIC),
            "the page poll must be authenticated like the statement was:\n{head}"
        );
        assert_eq!(
            header(head, "X-Trino-User").as_deref(),
            Some("analytic_user"),
            "the user half of the credential is the connection's, not the default:\n{head}"
        );
    }
}

#[tokio::test]
async fn the_cancel_request_is_authenticated_too() {
    // Cancel is the one request a coordinator is most likely to answer 401 on,
    // because it is `DELETE` on a URI the coordinator issued and not a statement of
    // its own. An unauthenticated cancel leaves the query running on the server
    // while the app believes it stopped.
    let coordinator = coordinator().await;

    let mut session = TrinoDriver::new()
        .connect(&password_config(coordinator.address))
        .await
        .expect("connect");
    // The listener's first answer is `QUEUED` with a `nextUri`, which is all
    // `execute` needs to leave a `DELETE` target behind.
    let _cursor = session
        .execute("SELECT 1", &ExecuteOptions::default())
        .await
        .expect("execute");
    session.cancel().await.expect("cancel");

    let heads = coordinator.heads();
    assert_eq!(heads.len(), 2, "the statement and the cancel: {heads:?}");
    assert!(heads[0].starts_with("POST /v1/statement"), "{}", heads[0]);
    assert!(heads[1].starts_with("DELETE "), "{}", heads[1]);
    for head in &heads {
        assert_eq!(
            header(head, "Authorization").as_deref(),
            Some(BASIC),
            "the cancel must be authenticated like the statement was:\n{head}"
        );
    }
}

#[tokio::test]
async fn a_connection_with_no_password_sends_no_authorization_header() {
    // The other half of the rule, and the one a careless fix breaks: a coordinator
    // running without auth accepts both, but a connection that was never given a
    // password must not be answered with an empty or garbage credential. A client
    // that sent `Authorization: Basic <user:>` here would be sending a secret it was
    // never given.
    let coordinator = coordinator().await;

    let mut session = TrinoDriver::new()
        .connect(&config(coordinator.address))
        .await
        .expect("connect");
    let mut cursor = session
        .execute("SELECT 1", &ExecuteOptions::default())
        .await
        .expect("execute");
    while cursor.next_batch(16).await.expect("next_batch").is_some() {}
    drop(cursor);

    let heads = coordinator.heads();
    assert!(heads.len() >= 2, "the statement and a page poll: {heads:?}");
    for head in &heads {
        assert_eq!(
            header(head, "Authorization"),
            None,
            "no password was given, so nothing may be sent:\n{head}"
        );
        assert_eq!(
            header(head, "X-Trino-User").as_deref(),
            Some("queryhive"),
            "the user still travels, password or not:\n{head}"
        );
    }
}

#[test]
fn a_credential_never_prints_its_password() {
    // `{:?}` on anything holding a credential is the shortest path from a support
    // request to a secret in a log. `ConnectionConfig` already reports only whether
    // a password is present, and this type has to follow the same rule.
    let with = qh_driver_trino::Credentials::new("analytic_user", Some("hunter2".to_owned()));
    let without = qh_driver_trino::Credentials::new("analytic_user", None);
    let shown = format!("{with:?}");
    assert!(
        !shown.contains("hunter2"),
        "the password was printed: {shown}"
    );
    assert!(shown.contains("analytic_user"), "{shown}");
    assert!(shown.contains("password: present"), "{shown}");
    assert!(format!("{without:?}").contains("password: none"));
}
