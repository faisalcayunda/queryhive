//! TLS, against coordinators the test itself stands up.
//!
//! A real certificate cannot be exercised in CI: the platform trust store holds
//! what the machine's administrator put there, and a test cannot install a root in
//! it. So the coordinator here is a local TLS listener with a certificate this file
//! generated, and the client-building seam is [`client_for`], which takes the root
//! store to verify against instead of the platform's.
//!
//! What that covers:
//!
//! * `Require` verifies: a certificate chaining to the store the client was given
//!   succeeds and the statement is served, a certificate the store does not know
//!   does **not** succeed, and `RequireNoVerify` accepts that same certificate. The
//!   failure is the feature, not a gap.
//! * `Prefer` encrypts and does not verify: pointed at the same self-signed
//!   coordinator it succeeds, and the coordinator's own request log is what shows
//!   the statement arrived *inside the TLS session* rather than over a fallback.
//! * `Prefer` falls back only for a coordinator that does not speak TLS at all, and
//!   refuses to fall back when one that does speak TLS fails the handshake. Both
//!   tests count the connections the server accepted, because a downgrade attempt
//!   is exactly what a second connection would be.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qh_core::EngineError;
use qh_driver::{ConnectionConfig, Driver, DriverKind, ExecuteOptions, TlsMode};
use qh_driver_trino::{client_for, RootCertStore, TrinoDriver};
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// One complete Trino page: a finished statement with one `bigint` row.
///
/// `nextUri` is absent, which is the protocol's own "this is the end", so a
/// statement answered with this needs no page poll — the TLS decision under test is
/// the statement's `POST` and nothing after it.
const PAGE: &str = r#"{"id":"20260922_033254_00000_tlst","columns":[{"name":"n","type":"bigint"}],"data":[[1]],"stats":{"state":"FINISHED"}}"#;

/// A coordinator standing in a test: where it listens, how many connections it
/// accepted, and the request lines it read.
struct Coordinator {
    address: SocketAddr,
    accepted: Arc<AtomicUsize>,
    requests: Arc<Mutex<Vec<String>>>,
    _task: tokio::task::JoinHandle<()>,
}

impl Coordinator {
    fn connections(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    /// The request lines the server read, in the order it read them.
    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("request log").clone()
    }

    fn was_asked_for_the_statement(&self) -> bool {
        self.requests()
            .iter()
            .any(|line| line.starts_with("POST /v1/statement"))
    }
}

/// Read one chunk — a request, or a TLS ClientHello — and record its first line.
///
/// Generic over the stream so the same recording works on a plain socket and on a
/// completed TLS session, which is what lets a test tell the two apart by what the
/// coordinator read.
async fn record<S: tokio::io::AsyncRead + Unpin>(
    stream: &mut S,
    requests: &Arc<Mutex<Vec<String>>>,
) {
    let mut buffer = [0u8; 4096];
    let Ok(Ok(read)) = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buffer)).await
    else {
        return;
    };
    let text = String::from_utf8_lossy(&buffer[..read]);
    if let Some(line) = text.lines().next() {
        requests.lock().expect("request log").push(line.to_owned());
    }
}

// ---------------------------------------------------------------------------
// A coordinator for the test
// ---------------------------------------------------------------------------

/// A self-signed leaf: its own issuer, so no store can chain it anywhere.
///
/// That is what a self-signed keystore on a real coordinator looks like too, and it
/// is the certificate the untrusted cases are about.
fn self_signed_certificate() -> (CertificateDer<'static>, PrivateKeyDer<'static>) {
    let key = KeyPair::generate().expect("a key pair");
    let params = CertificateParams::new(vec!["127.0.0.1".to_owned(), "localhost".to_owned()])
        .expect("subject alternative names");
    let cert = params.self_signed(&key).expect("a self-signed certificate");

    (
        cert.der().clone(),
        PrivateKeyDer::from(PrivatePkcs8KeyDer::from(key.serialize_der())),
    )
}

/// A server certificate signed by a CA this file also returns, so a test can decide
/// to trust it.
fn chain_signed_certificate() -> (
    CertificateDer<'static>,
    PrivateKeyDer<'static>,
    CertificateDer<'static>,
) {
    let ca_key = KeyPair::generate().expect("a CA key pair");
    let mut ca_params = CertificateParams::new(Vec::new()).expect("CA parameters");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = CertifiedIssuer::self_signed(ca_params, ca_key).expect("a self-signed CA");

    let server_key = KeyPair::generate().expect("a server key pair");
    let server_params =
        CertificateParams::new(vec!["127.0.0.1".to_owned(), "localhost".to_owned()])
            .expect("subject alternative names");
    let server = server_params
        .signed_by(&server_key, &ca)
        .expect("a certificate signed by the CA");

    (
        server.der().clone(),
        PrivateKeyDer::from(PrivatePkcs8KeyDer::from(server_key.serialize_der())),
        ca.der().clone(),
    )
}

/// A listener that answers every connection with [`PAGE`], over TLS.
///
/// The connection count and the request log are the evidence for the `Prefer`
/// tests: this listener *only* speaks TLS, so a request that appears in its log was
/// encrypted, and a second connection would be a fallback attempt.
async fn tls_coordinator(
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
) -> Coordinator {
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("a server configuration");
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let address = listener.local_addr().expect("the bound address");
    let accepted = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));

    let counter = Arc::clone(&accepted);
    let log = Arc::clone(&requests);
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            counter.fetch_add(1, Ordering::SeqCst);
            let acceptor = acceptor.clone();
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                let Ok(mut stream) = acceptor.accept(stream).await else {
                    // A handshake the client refused ends here, which is a correct
                    // outcome for this coordinator.
                    return;
                };
                record(&mut stream, &log).await;
                let _ = stream.write_all(response().as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });

    Coordinator {
        address,
        accepted,
        requests,
        _task: task,
    }
}

/// A TLS coordinator whose statement takes two pages, the way a real one does.
///
/// This is what makes "the decision was made once" observable. The queue page
/// carries a `nextUri` for the driver to poll, the poll is a second connection, and
/// this listener only ever completes a TLS handshake — so a regression that
/// re-decided the scheme on the poll would have to reach the port in clear and
/// could not be served.
async fn tls_coordinator_with_a_second_page(
    cert: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
) -> Coordinator {
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("a server configuration");
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let address = listener.local_addr().expect("the bound address");
    let accepted = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let served = Arc::new(AtomicUsize::new(0));

    let counter = Arc::clone(&accepted);
    let log = Arc::clone(&requests);
    let count = Arc::clone(&served);
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            counter.fetch_add(1, Ordering::SeqCst);
            let acceptor = acceptor.clone();
            let log = Arc::clone(&log);
            let index = count.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let Ok(mut stream) = acceptor.accept(stream).await else {
                    return;
                };
                record(&mut stream, &log).await;
                // The statement answers queued with a `nextUri` on this same
                // authority; the poll that follows gets the row and the end.
                let body = if index == 0 {
                    format!(
                        r#"{{"id":"q","nextUri":"https://{address}/v1/statement/next","stats":{{"state":"RUNNING"}}}}"#
                    )
                } else {
                    PAGE.to_owned()
                };
                let _ = stream.write_all(response_for(&body).as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });

    Coordinator {
        address,
        accepted,
        requests,
        _task: task,
    }
}

/// A listener that answers every connection with [`PAGE`], in clear.
///
/// This is the coordinator `Prefer` is allowed to fall back to: it answers the TLS
/// ClientHello with ordinary HTTP bytes, which is what a Trino that never had TLS
/// configured does.
async fn plain_coordinator() -> Coordinator {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let address = listener.local_addr().expect("the bound address");
    let accepted = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));

    let counter = Arc::clone(&accepted);
    let log = Arc::clone(&requests);
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            counter.fetch_add(1, Ordering::SeqCst);
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                record(&mut stream, &log).await;
                let _ = stream.write_all(response().as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });

    Coordinator {
        address,
        accepted,
        requests,
        _task: task,
    }
}

/// A listener that speaks TLS and then refuses the handshake with a fatal alert.
///
/// A peer that offers TLS and fails, as opposed to one that has none: rustls parses
/// the alert, so this is a handshake that *reached* the TLS layer and stopped
/// there. Falling back here is the downgrade an attacker wants, and the connection
/// count is what makes "did not even try" checkable.
async fn tls_alert_coordinator() -> Coordinator {
    // content type 21 (alert), TLS 1.2 record version, length 2, fatal, 40 =
    // handshake_failure.
    const ALERT: [u8; 7] = [0x15, 0x03, 0x03, 0x00, 0x02, 0x02, 0x28];

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let address = listener.local_addr().expect("the bound address");
    let accepted = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));

    let counter = Arc::clone(&accepted);
    let log = Arc::clone(&requests);
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            counter.fetch_add(1, Ordering::SeqCst);
            let log = Arc::clone(&log);
            tokio::spawn(async move {
                record(&mut stream, &log).await;
                let _ = stream.write_all(&ALERT).await;
                let _ = stream.shutdown().await;
            });
        }
    });

    Coordinator {
        address,
        accepted,
        requests,
        _task: task,
    }
}

fn response_for(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn response() -> String {
    response_for(PAGE)
}

fn config(address: SocketAddr, tls: TlsMode) -> ConnectionConfig {
    ConnectionConfig::new(DriverKind::Trino, "127.0.0.1", address.port(), "queryhive")
        .database("tpch")
        .schema("tiny")
        .tls(tls)
}

/// The single row this file's coordinator serves, through the real driver.
async fn one_row(config: &ConnectionConfig) -> Result<Vec<Vec<qh_core::Value>>, EngineError> {
    let mut session = TrinoDriver::new().connect(config).await?;
    let mut cursor = session
        .execute("SELECT 1", &ExecuteOptions::default())
        .await?;
    let mut rows = Vec::new();
    while let Some(batch) = cursor.next_batch(16).await? {
        let columns = batch.columns();
        let height = columns.first().map_or(0, Vec::len);
        for index in 0..height {
            rows.push(columns.iter().map(|column| column[index].clone()).collect());
        }
    }
    Ok(rows)
}

fn assert_connect_failure(error: EngineError) {
    match error {
        EngineError::Connect { message, .. } => assert!(message.contains("https://"), "{message}"),
        other => panic!("expected a connect failure, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Require: the mode that verifies
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_certificate_chaining_to_the_configured_store_verifies() {
    // The store the caller supplies is the one the certificate is checked against.
    // Nothing here reaches the platform store, which is the point: the verifying
    // path is real code and this exercises it.
    let (cert, key, ca) = chain_signed_certificate();
    let coordinator = tls_coordinator(cert, key).await;

    let mut roots = RootCertStore::empty();
    roots.add(ca).expect("the CA goes into the store");
    let client = client_for(TlsMode::Require, Some(roots)).expect("a client");

    // The same request the driver sends, over a handshake that verified against the
    // store above.
    let response = client
        .post(format!("https://{}/v1/statement", coordinator.address))
        .header("X-Trino-User", "queryhive")
        .header("Content-Type", "text/plain")
        .body("SELECT 1")
        .send()
        .await
        .expect("a certificate chaining to the given store must verify");

    assert_eq!(response.status(), 200);
    let body = response.text().await.expect("the page body");
    assert!(body.contains(r#""data":[[1]]"#), "{body}");
    assert_eq!(coordinator.connections(), 1);
}

#[tokio::test]
async fn require_refuses_a_certificate_it_does_not_trust() {
    // The failure is the feature. `Require` means the certificate is checked, and a
    // certificate no store knows must stop the connection rather than be waved
    // through.
    let (cert, key) = self_signed_certificate();
    let coordinator = tls_coordinator(cert, key).await;

    let error = one_row(&config(coordinator.address, TlsMode::Require))
        .await
        .expect_err("a self-signed certificate must not verify");

    assert_connect_failure(error);
    // One connection: it did not try to find a way around the refusal.
    assert_eq!(
        coordinator.connections(),
        1,
        "a refused certificate must not be retried"
    );
}

#[tokio::test]
async fn require_no_verify_accepts_the_certificate_require_refuses() {
    // The app's `verify: false`, reachable only when the user asked for it on that
    // connection. It is the same coordinator and the same certificate as the test
    // above, so the two together say exactly what is checked and what is not.
    let (cert, key) = self_signed_certificate();
    let coordinator = tls_coordinator(cert, key).await;

    let rows = one_row(&config(coordinator.address, TlsMode::RequireNoVerify))
        .await
        .expect("an unverified but encrypted connection is what this mode is");
    assert_eq!(rows, vec![vec![qh_core::Value::Int(1)]]);
    assert_eq!(coordinator.connections(), 1);
    assert!(coordinator.was_asked_for_the_statement());
}

#[tokio::test]
async fn disable_still_talks_in_clear() {
    // The one mode that never touches TLS, kept honest so the other three are
    // visibly a choice rather than the only path that works.
    let coordinator = plain_coordinator().await;
    let rows = one_row(&config(coordinator.address, TlsMode::Disable))
        .await
        .expect("plaintext is what this mode is");
    assert_eq!(rows, vec![vec![qh_core::Value::Int(1)]]);
}

// ---------------------------------------------------------------------------
// Prefer: encrypt without verifying, and downgrade only for a peer with no TLS
// ---------------------------------------------------------------------------

#[tokio::test]
async fn prefer_encrypts_without_verifying_the_certificate() {
    // `Prefer` does not check the certificate, deliberately: a self-signed
    // coordinator is ordinary in this product's deployments, and the Python
    // engine's `prefer` did not check either. So this must *succeed*, and the
    // evidence that it succeeded over TLS rather than by falling back is the
    // coordinator itself -- this listener only ever completes a TLS handshake, and
    // only a TLS session could have delivered the statement it logged.
    let (cert, key) = self_signed_certificate();
    let coordinator = tls_coordinator(cert, key).await;

    let rows = one_row(&config(coordinator.address, TlsMode::Prefer))
        .await
        .expect("Prefer must not verify: a self-signed certificate is what it encrypts over");

    assert_eq!(rows, vec![vec![qh_core::Value::Int(1)]]);
    assert_eq!(
        coordinator.connections(),
        1,
        "one connection: nothing was retried in clear"
    );
    assert!(
        coordinator.was_asked_for_the_statement(),
        "the coordinator should have read the POST inside the TLS session, got {:?}",
        coordinator.requests()
    );
}

#[tokio::test]
async fn prefer_keeps_the_tls_decision_for_every_page() {
    // The statement answer is a queued page carrying a `nextUri`, so the driver
    // makes a second request -- and that is the one a downgrade would have to happen
    // on. Both requests go to this listener, which only speaks TLS, and the second
    // connection's request line is the evidence it was served over TLS rather than
    // re-decided. A regression that re-ran the choice per request would fail here
    // and nowhere else.
    let (cert, key) = self_signed_certificate();
    let coordinator = tls_coordinator_with_a_second_page(cert, key).await;

    let rows = one_row(&config(coordinator.address, TlsMode::Prefer))
        .await
        .expect("a failed handshake is not a reason to talk in clear");

    assert_eq!(rows, vec![vec![qh_core::Value::Int(1)]]);
    assert_eq!(
        coordinator.connections(),
        2,
        "the statement and its page, and nothing in clear"
    );
    let requests = coordinator.requests();
    assert!(
        requests
            .first()
            .is_some_and(|line| line.starts_with("POST /v1/statement")),
        "{requests:?}"
    );
    assert!(
        requests
            .get(1)
            .is_some_and(|line| line.starts_with("GET /v1/statement/next")),
        "the page poll should have gone to the same coordinator over TLS: {requests:?}"
    );
}

#[tokio::test]
async fn prefer_falls_back_when_the_coordinator_does_not_speak_tls() {
    // A coordinator with no TLS answers the ClientHello with an HTTP response, and
    // that is the only signal allowed to downgrade. Two connections: the HTTPS
    // attempt, then the HTTP one.
    let coordinator = plain_coordinator().await;

    let rows = one_row(&config(coordinator.address, TlsMode::Prefer))
        .await
        .expect("a coordinator without TLS is what the fallback exists for");
    assert_eq!(rows, vec![vec![qh_core::Value::Int(1)]]);
    assert_eq!(
        coordinator.connections(),
        2,
        "one HTTPS attempt, then one HTTP retry"
    );
}

#[tokio::test]
async fn prefer_does_not_downgrade_a_handshake_that_failed() {
    // The coordinator *does* speak TLS and refuses the handshake with an alert.
    // Falling back here would hand an attacker who can break the handshake a
    // plaintext connection, so this must fail instead -- and the connection count
    // is what makes "did not even try" checkable rather than asserted.
    let coordinator = tls_alert_coordinator().await;

    let error = one_row(&config(coordinator.address, TlsMode::Prefer))
        .await
        .expect_err("a failed handshake is not a reason to talk in clear");

    assert_connect_failure(error);
    assert_eq!(
        coordinator.connections(),
        1,
        "a server that speaks TLS and fails the handshake must not be downgraded"
    );
}
