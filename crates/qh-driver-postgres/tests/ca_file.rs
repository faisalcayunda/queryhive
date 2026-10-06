//! A connection's own CA file, and the TLS name behind an SSH tunnel, against a server this
//! file stands up.
//!
//! No container: the server here is a socket that does exactly as much of the PostgreSQL
//! protocol as `connect` needs (the SSLRequest answer, a TLS handshake, an
//! `AuthenticationOk`, a `ReadyForQuery`), with a certificate this file generates for the
//! name `db.internal`. That is enough to prove what these tests are about, which is who
//! the client trusts and which name it checks, and nothing more.
//!
//! The tunnel case is the one blueprint W11 §7.3 says was read but never run: the engine
//! points the connection at `127.0.0.1:<local port>` and the certificate says
//! `db.internal`. The test shows the failure first (no `tls_server_name`: the name checked
//! is `127.0.0.1`) and then the fix, so the fix is known to be what changed the outcome.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use qh_core::{EngineError, FailureKind, Value};
use qh_driver::{ConnectionConfig, Driver, DriverKind, ExecuteOptions, Session, TlsCa, TlsMode};
use qh_driver_postgres::PostgresDriver;
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SSL_REQUEST_CODE: u32 = 80_877_103;
const CANCEL_REQUEST_CODE: u32 = 80_877_102;

/// A CA, and a server certificate it signed for `names`, in the shapes the test needs.
struct Pki {
    ca_pem: String,
    server: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

fn pki(names: &[&str]) -> Pki {
    let ca_key = KeyPair::generate().expect("a CA key pair");
    let mut ca_params = CertificateParams::new(Vec::new()).expect("CA parameters");
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = CertifiedIssuer::self_signed(ca_params, ca_key).expect("a CA");

    let server_key = KeyPair::generate().expect("a server key pair");
    let server = CertificateParams::new(
        names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>(),
    )
    .expect("subject alternative names")
    .signed_by(&server_key, &ca)
    .expect("a certificate signed by the CA");

    Pki {
        ca_pem: ca.pem(),
        server: server.der().clone(),
        key: PrivateKeyDer::from(PrivatePkcs8KeyDer::from(server_key.serialize_der())),
    }
}

/// A server that completes a TLS handshake and a PostgreSQL startup.
struct Server {
    address: SocketAddr,
    connections: Arc<AtomicUsize>,
    /// The SNI name each completed handshake carried.
    server_names: Arc<Mutex<Vec<Option<String>>>>,
    /// How each `CancelRequest` arrived: `"tls"` or `"plain"` (the pid and secret key in clear).
    cancels: Arc<Mutex<Vec<&'static str>>>,
    _task: tokio::task::JoinHandle<()>,
}

async fn server(pki: &Pki) -> Server {
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![pki.server.clone()], pki.key.clone_key())
        .expect("a server configuration");
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let address = listener.local_addr().expect("the bound address");
    let connections = Arc::new(AtomicUsize::new(0));
    let server_names = Arc::new(Mutex::new(Vec::new()));
    let cancels = Arc::new(Mutex::new(Vec::new()));

    let count = Arc::clone(&connections);
    let names = Arc::clone(&server_names);
    let recorded_cancels = Arc::clone(&cancels);
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            count.fetch_add(1, Ordering::SeqCst);
            let acceptor = acceptor.clone();
            let names = Arc::clone(&names);
            let cancels = Arc::clone(&recorded_cancels);
            tokio::spawn(async move {
                // The SSLRequest: length 8, then its code.
                let mut request = [0u8; 8];
                if stream.read_exact(&mut request).await.is_err() {
                    return;
                }
                match u32::from_be_bytes(request[4..8].try_into().unwrap()) {
                    SSL_REQUEST_CODE => {}
                    // A cancel that skipped TLS: what a `NoTls` cancel from a TLS session is.
                    CANCEL_REQUEST_CODE => {
                        cancels.lock().unwrap().push("plain");
                        return;
                    }
                    _ => return,
                }
                if stream.write_all(b"S").await.is_err() {
                    return;
                }
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    // A handshake the client refused ends here: the outcome under test.
                    return;
                };
                names
                    .lock()
                    .unwrap()
                    .push(tls.get_ref().1.server_name().map(str::to_owned));

                // The StartupMessage: a length that counts itself, then the body.
                let mut length = [0u8; 4];
                if tls.read_exact(&mut length).await.is_err() {
                    return;
                }
                let body = u32::from_be_bytes(length) as usize - 4;
                let mut startup = vec![0u8; body];
                if tls.read_exact(&mut startup).await.is_err() {
                    return;
                }
                if startup.len() == 12
                    && u32::from_be_bytes(startup[..4].try_into().unwrap()) == CANCEL_REQUEST_CODE
                {
                    cancels.lock().unwrap().push("tls");
                    return;
                }
                // AuthenticationOk, BackendKeyData, ReadyForQuery (idle).
                let mut reply = Vec::new();
                reply.extend_from_slice(&[b'R', 0, 0, 0, 8, 0, 0, 0, 0]);
                reply.extend_from_slice(&[b'K', 0, 0, 0, 12, 0, 0, 0, 1, 0, 0, 0, 2]);
                reply.extend_from_slice(&[b'Z', 0, 0, 0, 5, b'I']);
                let _ = tls.write_all(&reply).await;
                // Hold the connection open until the client goes away.
                let mut sink = [0u8; 64];
                while matches!(tls.read(&mut sink).await, Ok(read) if read > 0) {}
            });
        }
    });

    Server {
        address,
        connections,
        server_names,
        cancels,
        _task: task,
    }
}

fn ca(pem: &str) -> TlsCa {
    TlsCa::from_pem("/tmp/test-ca.pem", pem.as_bytes()).expect("a valid CA bundle")
}

/// The loopback connection a tunnel would hand the driver.
fn through_the_tunnel(address: SocketAddr, pem: &str) -> ConnectionConfig {
    ConnectionConfig::new(DriverKind::Postgres, "127.0.0.1", address.port(), "qh")
        .password("not-checked")
        .database("qh")
        .tls(TlsMode::Require)
        .tls_ca(ca(pem))
}

#[tokio::test]
async fn a_certificate_signed_by_the_named_ca_verifies_for_its_name_through_a_tunnel() {
    let pki = pki(&["db.internal"]);
    let server = server(&pki).await;

    let config = through_the_tunnel(server.address, &pki.ca_pem).tls_server_name("db.internal");
    let session = PostgresDriver::new()
        .connect(&config)
        .await
        .expect("the CA file trusts it, and the name checked is db.internal");
    drop(session);

    assert_eq!(server.connections.load(Ordering::SeqCst), 1);
    // The name went out in the handshake too: that is what a virtual-hosting proxy
    // in front of the database would route on.
    assert_eq!(
        server.server_names.lock().unwrap().as_slice(),
        [Some("db.internal".to_owned())]
    );
}

#[tokio::test]
async fn a_cancel_from_a_tls_session_is_itself_encrypted() {
    // S-1. The `CancelRequest` carries the backend's pid and secret key on a second
    // connection; it used to be sent with `NoTls` whatever the session negotiated, so a
    // `Prefer` session cancelled in clear and a verifying one could not cancel at all.
    let pki = pki(&["db.internal"]);

    // `Require` with the connection's own CA, through a tunnel-shaped address.
    let server_a = server(&pki).await;
    let config = through_the_tunnel(server_a.address, &pki.ca_pem).tls_server_name("db.internal");
    let session = PostgresDriver::new()
        .connect(&config)
        .await
        .expect("connects");
    session.cancel().await.expect("the cancel speaks TLS too");
    // The fake closes right after reading the request, so give it a moment to record.
    wait_for(&server_a.cancels, 1).await;
    assert_eq!(server_a.cancels.lock().unwrap().as_slice(), ["tls"]);

    // `Prefer`: the mode where the cleartext cancel was reachable.
    let server_b = server(&pki).await;
    let config = ConnectionConfig::new(
        DriverKind::Postgres,
        "127.0.0.1",
        server_b.address.port(),
        "qh",
    )
    .password("not-checked")
    .database("qh")
    .tls(TlsMode::Prefer);
    let session = PostgresDriver::new()
        .connect(&config)
        .await
        .expect("connects");
    session.cancel().await.expect("the cancel speaks TLS too");
    wait_for(&server_b.cancels, 1).await;
    assert_eq!(
        server_b.cancels.lock().unwrap().as_slice(),
        ["tls"],
        "a TLS session must never cancel in clear"
    );
}

async fn wait_for(cancels: &Mutex<Vec<&'static str>>, count: usize) {
    for _ in 0..100 {
        if cancels.lock().unwrap().len() >= count {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn without_the_tls_name_the_tunnel_address_is_what_gets_checked_and_it_fails() {
    // The gap, shown before the fix: `127.0.0.1` is not in a certificate issued for
    // `db.internal`, so every verifying mode failed through a tunnel.
    let pki = pki(&["db.internal"]);
    let server = server(&pki).await;

    let error = PostgresDriver::new()
        .connect(&through_the_tunnel(server.address, &pki.ca_pem))
        .await
        .err()
        .expect("127.0.0.1 is not the name on this certificate");

    assert!(matches!(error, EngineError::Connect { .. }), "{error:?}");
    // A name that does not match will not match on the next try either.
    assert_eq!(error.failure_kind(), FailureKind::Permanent, "{error:?}");
    assert!(server.server_names.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_certificate_the_ca_file_does_not_cover_is_refused() {
    // The platform store is not consulted as well: the bundle is the whole of what is
    // trusted, so another CA's certificate fails even though it is well formed.
    let served = pki(&["db.internal"]);
    let other = pki(&["db.internal"]);
    let server = server(&served).await;

    let config = through_the_tunnel(server.address, &other.ca_pem).tls_server_name("db.internal");
    let error = PostgresDriver::new()
        .connect(&config)
        .await
        .err()
        .expect("a certificate from a CA outside the bundle must not verify");

    assert!(matches!(error, EngineError::Connect { .. }), "{error:?}");
    assert_eq!(error.failure_kind(), FailureKind::Permanent, "{error:?}");
}

#[tokio::test]
async fn a_ca_file_outside_the_verifying_mode_is_refused_before_a_socket_opens() {
    // `Prefer` and `RequireNoVerify` would ignore the bundle, and a connection that looks
    // pinned and is not is the failure this guards against.
    let pki = pki(&["db.internal"]);
    let server = server(&pki).await;

    for mode in [TlsMode::Prefer, TlsMode::RequireNoVerify, TlsMode::Disable] {
        let config = through_the_tunnel(server.address, &pki.ca_pem).tls(mode);
        let error = PostgresDriver::new()
            .connect(&config)
            .await
            .err()
            .expect("refused");
        assert!(
            matches!(error, EngineError::Usage { .. }),
            "{mode:?}: {error:?}"
        );
    }
    assert_eq!(server.connections.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_tls_name_needs_an_address_to_connect_to() {
    // `host` is where the socket goes, so with a name to verify it has to be an address.
    let config = ConnectionConfig::new(DriverKind::Postgres, "not-an-ip", 5432, "qh")
        .tls(TlsMode::Prefer)
        .tls_server_name("db.internal");
    let error = PostgresDriver::new()
        .connect(&config)
        .await
        .err()
        .expect("refused");
    assert!(matches!(error, EngineError::Usage { .. }), "{error:?}");
}

// ---------------------------------------------------------------------------
// Live: the `qh-pg-tls` container, against the committed fixture CA
// ---------------------------------------------------------------------------
//
//   crates/qh-driver-postgres/tests/tls/up.sh
//   QH_TEST_POSTGRES=1 cargo test -p qh-driver-postgres --test ca_file
//
// Gated the same way `tests/tls.rs` is, and for the same reason: a suite that went red
// because a container is not up would teach a reader to ignore red. The fake-server tests
// above prove the logic; these prove it against a real PostgreSQL serving a certificate
// signed by `tests/tls/ca.crt`, through the real file loader.

fn live() -> Option<ConnectionConfig> {
    if std::env::var("QH_TEST_POSTGRES").as_deref() != Ok("1") {
        eprintln!("skipped: set QH_TEST_POSTGRES=1 with tests/tls/up.sh running on 55433");
        return None;
    }
    let port = std::env::var("QH_PG_TLS_PORT")
        .ok()
        .and_then(|port| port.parse().ok())
        .unwrap_or(55433);
    let reachable = std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(300),
    )
    .is_ok();
    if !reachable {
        eprintln!("skipped: nothing listening on 127.0.0.1:{port}; start tests/tls/up.sh");
        return None;
    }
    Some(
        ConnectionConfig::new(DriverKind::Postgres, "127.0.0.1", port, "qh")
            .password("qh-dev-only")
            .database("qh")
            .tls(TlsMode::Require),
    )
}

fn fixture(name: &str) -> TlsCa {
    let path = format!("{}/tests/tls/{name}", env!("CARGO_MANIFEST_DIR"));
    TlsCa::load(std::path::Path::new(&path)).unwrap_or_else(|error| panic!("{error}"))
}

async fn server_says_encrypted(session: &mut Box<dyn Session>) -> bool {
    let mut cursor = session
        .execute(
            "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
            &ExecuteOptions::default(),
        )
        .await
        .expect("pg_stat_ssl should answer");
    let batch = cursor
        .next_batch(1)
        .await
        .expect("next_batch")
        .expect("one row");
    matches!(batch.value(0, 0), Some(Value::Bool(true)))
}

#[tokio::test]
async fn live_the_ca_file_verifies_the_servers_own_certificate() {
    let Some(config) = live() else { return };
    let mut session = PostgresDriver::new()
        .connect(&config.tls_ca(fixture("ca.crt")))
        .await
        .expect("the fixture CA signed the server certificate");
    assert!(server_says_encrypted(&mut session).await);
}

#[tokio::test]
async fn live_another_ca_file_does_not() {
    // A well-formed CA that signed nothing here: the bundle is all that is trusted, and
    // the machine's own store, which has no copy of the fixture CA either way, is not
    // consulted to make up the difference.
    let Some(config) = live() else { return };
    let error = PostgresDriver::new()
        .connect(&config.tls_ca(fixture("other-ca.crt")))
        .await
        .err()
        .expect("a certificate from a CA outside the bundle must not verify");
    assert!(matches!(error, EngineError::Connect { .. }), "{error:?}");
}

#[tokio::test]
async fn live_the_name_checked_may_differ_from_the_address_dialled() {
    // `localhost` is on the certificate and `127.0.0.1` is where the socket goes: the
    // `host` and `hostaddr` split a tunnel relies on, against a real server.
    let Some(config) = live() else { return };
    let mut session = PostgresDriver::new()
        .connect(
            &config
                .clone()
                .tls_ca(fixture("ca.crt"))
                .tls_server_name("localhost"),
        )
        .await
        .expect("localhost is a name on the certificate");
    assert!(server_says_encrypted(&mut session).await);

    let error = PostgresDriver::new()
        .connect(
            &config
                .tls_ca(fixture("ca.crt"))
                .tls_server_name("db.internal"),
        )
        .await
        .err()
        .expect("a name the certificate does not carry must not verify");
    assert!(matches!(error, EngineError::Connect { .. }), "{error:?}");
}
