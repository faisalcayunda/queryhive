//! A password or a token reaches one address, and only that one.
//!
//! Two of the three URLs a statement uses come from the coordinator's own answers (the
//! `nextUri` of each page, and the `DELETE` that cancels), and a redirect can come from
//! anywhere. Each test here stands up a fake coordinator that answers with a URL the
//! driver must not follow with a credential, and a second listener where that URL points,
//! then checks the one thing that matters: **no request that reached the wrong place
//! carried an `Authorization` header**, and the wrong place was not even connected to
//! where the refusal happens before the request is built.
//!
//! Every case runs twice, once with a bearer token and once with a Basic password, because
//! the check is the same function for both (blueprint W11 §7.1 (c), decisions O-27).
//! The coordinator is TLS with a certificate signed by a CA the test makes and hands to the
//! driver as the connection's own CA file, so the CA path is exercised here too.
//!
//! A control comes first in spirit: a plain `reqwest` client with the default redirect
//! policy is pointed at the same fake and **does** leak. Without it, "the listener saw no
//! header" could be true because the fake is blind, not because the driver is careful.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qh_core::{EngineError, FailureKind};
use qh_driver::{ConnectionConfig, Driver, DriverKind, ExecuteOptions, Session, TlsCa, TlsMode};
use qh_driver_trino::TrinoDriver;
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const TOKEN: &str = "eyJSENTINEL.bearer-token_9f3a";
const PASSWORD: &str = "pw-SENTINEL-4711";
/// `queryhive:pw-SENTINEL-4711`, base64, computed outside this file so the test does not
/// agree with the driver by sharing its encoder.
const BASIC: &str = "Basic cXVlcnloaXZlOnB3LVNFTlRJTkVMLTQ3MTE=";

// ---------------------------------------------------------------------------
// The secret, in the two forms the driver takes
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Secret {
    Bearer,
    Basic,
}

impl Secret {
    const ALL: [Secret; 2] = [Secret::Bearer, Secret::Basic];

    fn apply(self, config: ConnectionConfig) -> ConnectionConfig {
        match self {
            Secret::Bearer => config.bearer(TOKEN),
            Secret::Basic => config.password(PASSWORD),
        }
    }

    /// The `Authorization` value a request to the right address must carry.
    fn header(self) -> String {
        match self {
            Secret::Bearer => format!("Bearer {TOKEN}"),
            Secret::Basic => BASIC.to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Certificates
// ---------------------------------------------------------------------------

struct Pki {
    ca_pem: String,
    server: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

fn pki() -> Pki {
    let ca_key = KeyPair::generate().expect("a CA key pair");
    let mut ca_params = CertificateParams::new(Vec::new()).expect("CA parameters");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = CertifiedIssuer::self_signed(ca_params, ca_key).expect("a CA");

    let server_key = KeyPair::generate().expect("a server key pair");
    let server = CertificateParams::new(vec!["127.0.0.1".to_owned(), "localhost".to_owned()])
        .expect("subject alternative names")
        .signed_by(&server_key, &ca)
        .expect("a certificate signed by the CA");

    Pki {
        ca_pem: ca.pem(),
        server: server.der().clone(),
        key: PrivateKeyDer::from(PrivatePkcs8KeyDer::from(server_key.serialize_der())),
    }
}

fn acceptor(pki: &Pki) -> tokio_rustls::TlsAcceptor {
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![pki.server.clone()], pki.key.clone_key())
        .expect("a server configuration");
    tokio_rustls::TlsAcceptor::from(Arc::new(config))
}

// ---------------------------------------------------------------------------
// Fake coordinators
// ---------------------------------------------------------------------------

/// What a handler is asked to answer: the request's index on this listener and its head.
type Handler = Arc<dyn Fn(usize, &str) -> String + Send + Sync>;

/// A listener that records every request it is sent, whole.
struct Listener {
    address: SocketAddr,
    connections: Arc<AtomicUsize>,
    /// The head of every request, `(over_tls, head)`, in arrival order.
    requests: Arc<Mutex<Vec<(bool, String)>>>,
    _task: tokio::task::JoinHandle<()>,
}

impl Listener {
    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    fn requests(&self) -> Vec<(bool, String)> {
        self.requests.lock().expect("request log").clone()
    }

    /// Requests that carried any `Authorization` header at all.
    fn authorized_requests(&self) -> usize {
        self.requests()
            .iter()
            .filter(|(_, head)| head.to_ascii_lowercase().contains("\r\nauthorization:"))
            .count()
    }

    fn plaintext_requests(&self) -> usize {
        self.requests().iter().filter(|(tls, _)| !tls).count()
    }
}

/// Read one request: the head, and as much body as it announces. Reading the body matters:
/// closing a socket with unread bytes can reset the connection and take the response with
/// it, which would make a client fail for a reason that is not the one under test.
async fn read_request<S: tokio::io::AsyncRead + Unpin>(stream: &mut S) -> Option<String> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
            .await
            .ok()?
            .ok()?;
        if read == 0 {
            return None;
        }
        data.extend_from_slice(&buffer[..read]);
        if let Some(end) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&data[..end]).into_owned();
            let announced = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            while data.len() < end + 4 + announced {
                let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
                    .await
                    .ok()?
                    .ok()?;
                if read == 0 {
                    break;
                }
                data.extend_from_slice(&buffer[..read]);
            }
            return Some(head);
        }
    }
}

fn json(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    )
}

fn redirect(location: &str) -> String {
    format!("HTTP/1.1 307 Temporary Redirect\r\nlocation: {location}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
}

/// The statement's first answer: still queued, and the page to poll.
fn queued(next_uri: &str) -> String {
    json(&format!(
        r#"{{"id":"q","nextUri":"{next_uri}","stats":{{"state":"QUEUED"}}}}"#
    ))
}

/// The last page: one `bigint` row, and no `nextUri`.
fn finished() -> String {
    json(
        r#"{"id":"q","columns":[{"name":"n","type":"bigint"}],"data":[[1]],"stats":{"state":"FINISHED"}}"#,
    )
}

/// A TLS coordinator. Each connection carries one request (the responses close).
async fn tls_listener(pki: &Pki, handler: Handler) -> Listener {
    let acceptor = acceptor(pki);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let address = listener.local_addr().expect("the bound address");
    let connections = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let served = Arc::new(AtomicUsize::new(0));

    let (count, log) = (Arc::clone(&connections), Arc::clone(&requests));
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            count.fetch_add(1, Ordering::SeqCst);
            let (acceptor, log, handler) = (acceptor.clone(), Arc::clone(&log), handler.clone());
            let index = served.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let Ok(mut stream) = acceptor.accept(stream).await else {
                    return;
                };
                let Some(head) = read_request(&mut stream).await else {
                    return;
                };
                log.lock().expect("request log").push((true, head.clone()));
                let _ = stream.write_all(handler(index, &head).as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });

    Listener {
        address,
        connections,
        requests,
        _task: task,
    }
}

/// A plaintext listener that answers every request with `reply`.
///
/// This is where a credential must never arrive. It still answers, so a client that did
/// connect gets a working response and the test sees the leak instead of a timeout.
async fn plain_listener(reply: String) -> Listener {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let address = listener.local_addr().expect("the bound address");
    let connections = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));

    let (count, log) = (Arc::clone(&connections), Arc::clone(&requests));
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            count.fetch_add(1, Ordering::SeqCst);
            let (log, reply) = (Arc::clone(&log), reply.clone());
            tokio::spawn(async move {
                if let Some(head) = read_request(&mut stream).await {
                    log.lock().expect("request log").push((false, head));
                }
                let _ = stream.write_all(reply.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });

    Listener {
        address,
        connections,
        requests,
        _task: task,
    }
}

/// One port that speaks both TLS and plain HTTP, told apart by the first byte (`0x16` is a
/// TLS handshake record). Its TLS answer is a `307` to `http://` on **its own address**, and
/// its plain answer is a finished page.
///
/// The redirect case needs exactly this: reqwest drops `Authorization` on a redirect when
/// the host or the port changes, so only a redirect to the same host and port, differing in
/// scheme alone, can carry it. That means one port that answers both ways.
async fn redirecting_listener(pki: &Pki) -> Listener {
    let acceptor = acceptor(pki);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let address = listener.local_addr().expect("the bound address");
    let connections = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));

    let (count, log) = (Arc::clone(&connections), Arc::clone(&requests));
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            count.fetch_add(1, Ordering::SeqCst);
            let (acceptor, log) = (acceptor.clone(), Arc::clone(&log));
            tokio::spawn(async move {
                if first_byte_is_tls(&stream).await {
                    let Ok(mut stream) = acceptor.accept(stream).await else {
                        return;
                    };
                    if let Some(head) = read_request(&mut stream).await {
                        log.lock().expect("request log").push((true, head));
                    }
                    let to = format!("http://{address}/v1/statement");
                    let _ = stream.write_all(redirect(&to).as_bytes()).await;
                    let _ = stream.shutdown().await;
                } else {
                    let mut stream = stream;
                    if let Some(head) = read_request(&mut stream).await {
                        log.lock().expect("request log").push((false, head));
                    }
                    let _ = stream.write_all(finished().as_bytes()).await;
                    let _ = stream.shutdown().await;
                }
            });
        }
    });

    Listener {
        address,
        connections,
        requests,
        _task: task,
    }
}

async fn first_byte_is_tls(stream: &TcpStream) -> bool {
    let mut first = [0u8; 1];
    matches!(stream.peek(&mut first).await, Ok(1) if first[0] == 0x16)
}

// ---------------------------------------------------------------------------
// Driving the driver
// ---------------------------------------------------------------------------

/// A verifying connection to `127.0.0.1:<port>` that trusts the test's CA file.
fn connection(address: SocketAddr, pki: &Pki, secret: Option<Secret>) -> ConnectionConfig {
    let config = ConnectionConfig::new(DriverKind::Trino, "127.0.0.1", address.port(), "queryhive")
        .database("tpch")
        .schema("tiny")
        .tls(TlsMode::Require)
        .tls_ca(TlsCa::from_pem("/tmp/test-ca.pem", pki.ca_pem.as_bytes()).expect("a CA bundle"));
    match secret {
        Some(secret) => secret.apply(config),
        None => config,
    }
}

async fn start(config: &ConnectionConfig) -> Box<dyn Session> {
    TrinoDriver::new().connect(config).await.expect("connect")
}

/// Run `SELECT 1` to its end and return the rows read, or the first failure.
async fn drain(session: &mut Box<dyn Session>) -> Result<usize, EngineError> {
    let mut cursor = session
        .execute("SELECT 1", &ExecuteOptions::default())
        .await?;
    let mut rows = 0;
    while let Some(batch) = cursor.next_batch(16).await? {
        rows += batch.columns().first().map_or(0, Vec::len);
    }
    Ok(rows)
}

fn header_of(head: &str, name: &str) -> Option<String> {
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().to_owned())
    })
}

/// The facts every refusal shares: named, permanent, both addresses, and no secret.
fn assert_named_refusal(error: &EngineError, session: &str, foreign: &str) {
    assert_eq!(error.failure_kind(), FailureKind::Permanent, "{error:?}");
    let message = error.message();
    assert!(message.contains(session), "{message}");
    assert!(message.contains(foreign), "{message}");
    assert!(message.contains("not sent"), "{message}");
    assert!(
        !message.contains(TOKEN) && !message.contains(PASSWORD),
        "{message}"
    );
}

/// The session listener answered the statement and saw only well-formed, authorized
/// requests; nothing else saw a credential.
fn assert_the_session_got_the_secret(listener: &Listener, secret: Secret, expected: usize) {
    let requests = listener.requests();
    assert_eq!(requests.len(), expected, "{requests:?}");
    for (_, head) in &requests {
        assert_eq!(
            header_of(head, "Authorization").as_deref(),
            Some(secret.header().as_str()),
            "{secret:?}: the address the connection uses must get the credential:\n{head}"
        );
    }
}

// ---------------------------------------------------------------------------
// The control
// ---------------------------------------------------------------------------

#[tokio::test]
async fn control_a_default_client_leaks_a_header_across_an_https_to_http_redirect() {
    // The same fake as the redirect case below, against a client with reqwest's default
    // policy. If this did not leak, every "nothing arrived in clear" assertion in this
    // file would prove something about the fake and nothing about the driver.
    let pki = pki();
    let listener = redirecting_listener(&pki).await;

    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .expect("a client");
    let _ = client
        .post(format!("https://{}/v1/statement", listener.address))
        .header("Authorization", "Bearer control-token")
        .body("SELECT 1")
        .send()
        .await;

    assert_eq!(
        listener.plaintext_requests(),
        1,
        "the default policy should have followed the redirect: {:?}",
        listener.requests()
    );
    assert_eq!(
        listener.authorized_requests(),
        2,
        "and carried the header into clear text: {:?}",
        listener.requests()
    );
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_http_next_uri_is_refused_before_it_is_requested() {
    for secret in Secret::ALL {
        let pki = pki();
        let elsewhere = plain_listener(finished()).await;
        let next = format!("http://{}/v1/statement/q/1", elsewhere.address);
        let session = tls_listener(&pki, Arc::new(move |_, _| queued(&next))).await;

        let mut running = start(&connection(session.address, &pki, Some(secret))).await;
        let error = drain(&mut running)
            .await
            .expect_err("a page URL on http must not be followed with a credential");

        assert!(
            matches!(error, EngineError::Query { code: None, .. }),
            "{error:?}"
        );
        assert_named_refusal(
            &error,
            &format!("https://127.0.0.1:{}", session.address.port()),
            &format!("http://127.0.0.1:{}", elsewhere.address.port()),
        );
        assert_eq!(
            elsewhere.connections(),
            0,
            "{secret:?}: nobody dialled the http address"
        );
        assert_eq!(elsewhere.authorized_requests(), 0);
        // The statement itself went out, authorized, to the right place; nothing followed.
        assert_the_session_got_the_secret(&session, secret, 1);
    }
}

#[tokio::test]
async fn a_next_uri_on_another_host_or_port_is_refused() {
    for secret in Secret::ALL {
        let pki = pki();
        let other_port = tls_listener(&pki, Arc::new(|_, _| finished())).await;

        // Another port on the same host.
        let next = format!(
            "https://127.0.0.1:{}/v1/statement/q/1",
            other_port.address.port()
        );
        let session = tls_listener(&pki, Arc::new(move |_, _| queued(&next))).await;
        let mut running = start(&connection(session.address, &pki, Some(secret))).await;
        let error = drain(&mut running).await.expect_err("another port");
        assert_named_refusal(
            &error,
            &format!("https://127.0.0.1:{}", session.address.port()),
            &format!("https://127.0.0.1:{}", other_port.address.port()),
        );
        assert_eq!(other_port.connections(), 0, "{secret:?}");

        // Another host on the same port: `localhost` is not `127.0.0.1` to this check,
        // and the certificate covers both, so a request would have gone through.
        // The poll URL names this listener's own port, by a different host name, so a
        // request that was sent would show up in this listener's log as a second one.
        let own_port = Arc::new(AtomicUsize::new(0));
        let port = Arc::clone(&own_port);
        let session = tls_listener(
            &pki,
            Arc::new(move |_, _| {
                queued(&format!(
                    "https://localhost:{}/v1/statement/q/1",
                    port.load(Ordering::SeqCst)
                ))
            }),
        )
        .await;
        own_port.store(usize::from(session.address.port()), Ordering::SeqCst);
        let mut running = start(&connection(session.address, &pki, Some(secret))).await;
        let error = drain(&mut running).await.expect_err("another host");
        assert_named_refusal(
            &error,
            &format!("https://127.0.0.1:{}", session.address.port()),
            &format!("https://localhost:{}", session.address.port()),
        );
        // Only the statement reached the listener: the poll was never sent.
        assert_the_session_got_the_secret(&session, secret, 1);
    }
}

#[tokio::test]
async fn cancel_and_reset_refuse_a_foreign_next_uri() {
    for secret in Secret::ALL {
        let pki = pki();
        let elsewhere = plain_listener(finished()).await;
        let next = format!("http://{}/v1/statement/q/1", elsewhere.address);
        let session = tls_listener(&pki, Arc::new(move |_, _| queued(&next))).await;

        let mut running = start(&connection(session.address, &pki, Some(secret))).await;
        // `execute` returns on the statement's answer, which is the one that stored the
        // foreign `nextUri` for cancel to use.
        let _cursor = running
            .execute("SELECT 1", &ExecuteOptions::default())
            .await
            .expect("the statement itself is fine");

        let error = running
            .cancel()
            .await
            .expect_err("the cancel has nowhere safe to go");
        assert!(
            matches!(error, EngineError::Query { code: None, .. }),
            "{error:?}"
        );
        assert_named_refusal(
            &error,
            &format!("https://127.0.0.1:{}", session.address.port()),
            &format!("http://127.0.0.1:{}", elsewhere.address.port()),
        );
        // Reset ignores a failed cancel, and still must not send anything.
        running.reset().await.expect("reset");

        assert_eq!(elsewhere.connections(), 0, "{secret:?}");
        assert_the_session_got_the_secret(&session, secret, 1);
    }
}

#[tokio::test]
async fn an_https_to_http_redirect_on_the_same_host_and_port_is_not_followed() {
    for secret in Secret::ALL {
        let pki = pki();
        let listener = redirecting_listener(&pki).await;

        let mut running = start(&connection(listener.address, &pki, Some(secret))).await;
        let error = drain(&mut running)
            .await
            .expect_err("a redirect is an answer, not an instruction to follow");

        assert_eq!(error.failure_kind(), FailureKind::Permanent, "{error:?}");
        assert!(error.message().contains("307"), "{}", error.message());
        assert_eq!(
            listener.plaintext_requests(),
            0,
            "{secret:?}: the credential was carried into clear text: {:?}",
            listener.requests()
        );
        assert_the_session_got_the_secret(&listener, secret, 1);
    }
}

#[tokio::test]
async fn a_session_with_no_secret_still_follows_what_the_coordinator_announces() {
    // Nothing can leak without a credential, and an unauthenticated coordinator that
    // announces another address works today, so the check must not change that.
    let pki = pki();
    let other = tls_listener(&pki, Arc::new(|_, _| finished())).await;
    let next = format!(
        "https://127.0.0.1:{}/v1/statement/q/1",
        other.address.port()
    );
    let session = tls_listener(&pki, Arc::new(move |_, _| queued(&next))).await;

    let mut running = start(&connection(session.address, &pki, None)).await;
    let rows = drain(&mut running)
        .await
        .expect("an open coordinator is followed");

    assert_eq!(rows, 1);
    assert_eq!(other.requests().len(), 1);
    assert_eq!(other.authorized_requests(), 0);
}

#[tokio::test]
async fn the_address_the_connection_uses_gets_the_secret_on_every_request() {
    // The positive half, so a driver that refused everything would not pass the rest.
    for secret in Secret::ALL {
        let pki = pki();
        let session = tls_listener(
            &pki,
            Arc::new(|index, head| {
                let port = header_of(head, "Host").expect("a Host header");
                if index == 0 {
                    queued(&format!("https://{port}/v1/statement/q/1"))
                } else {
                    finished()
                }
            }),
        )
        .await;

        let mut running = start(&connection(session.address, &pki, Some(secret))).await;
        assert_eq!(drain(&mut running).await.expect("a normal statement"), 1);

        assert_the_session_got_the_secret(&session, secret, 2);
        for (_, head) in session.requests() {
            let authorization = header_of(&head, "Authorization").expect("a header");
            // Exactly one scheme: a token is never also sent as Basic, or the reverse.
            match secret {
                Secret::Bearer => assert!(authorization.starts_with("Bearer "), "{authorization}"),
                Secret::Basic => assert!(authorization.starts_with("Basic "), "{authorization}"),
            }
        }
    }
}

#[tokio::test]
async fn prefer_with_a_password_never_falls_back_to_http() {
    // `Prefer` falls back to http for a coordinator that is not speaking TLS. With a
    // password that is the one case where the password would go out in clear, so there is
    // no fallback, and the refusal says why.
    let plain = plain_listener(finished()).await;
    let config = ConnectionConfig::new(
        DriverKind::Trino,
        "127.0.0.1",
        plain.address.port(),
        "queryhive",
    )
    .tls(TlsMode::Prefer)
    .password(PASSWORD);
    let mut running = start(&config).await;

    let error = drain(&mut running)
        .await
        .expect_err("no fallback with a password");

    assert!(matches!(error, EngineError::Connect { .. }), "{error:?}");
    assert_eq!(error.failure_kind(), FailureKind::Permanent, "{error:?}");
    assert!(
        error.message().contains("never falls back"),
        "{}",
        error.message()
    );
    assert!(!error.message().contains(PASSWORD), "{}", error.message());
    assert_eq!(plain.requests().len(), 0, "{:?}", plain.requests());
}
