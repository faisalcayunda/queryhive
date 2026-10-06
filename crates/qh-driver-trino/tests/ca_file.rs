//! A connection's own CA file, and the TLS name behind an SSH tunnel, against a coordinator
//! this file stands up.
//!
//! The coordinator's certificate is issued for `db.internal` and the driver is handed
//! `127.0.0.1:<port>`, which is what the engine does once a tunnel is open: the connection
//! goes to the tunnel's loopback endpoint and `ConnectionConfig::tls_server_name` keeps the
//! database's own name. The tests show the failure first (no name: `127.0.0.1` is checked
//! against a certificate that does not list it) and then that the name fixes it, so the fix
//! is known to be what changed the outcome (blueprint W11 §7.3).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qh_core::{EngineError, FailureKind};
use qh_driver::{ConnectionConfig, Driver, DriverKind, ExecuteOptions, TlsCa, TlsMode};
use qh_driver_trino::TrinoDriver;
use rcgen::{BasicConstraints, CertificateParams, CertifiedIssuer, IsCa, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct Pki {
    ca_pem: String,
    server: CertificateDer<'static>,
    key: PrivateKeyDer<'static>,
}

/// A CA, and a server certificate it signed for exactly `names`.
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

/// The SNI name and the request head of one completed handshake.
type Seen = (Option<String>, String);

struct Coordinator {
    address: SocketAddr,
    connections: Arc<AtomicUsize>,
    /// The request head and the SNI name of every completed handshake.
    seen: Arc<Mutex<Vec<Seen>>>,
    _task: tokio::task::JoinHandle<()>,
}

/// A TLS coordinator whose `reply` is the body of every answer.
async fn coordinator(pki: &Pki, reply: String) -> Coordinator {
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![pki.server.clone()], pki.key.clone_key())
        .expect("a server configuration");
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let address = listener.local_addr().expect("the bound address");
    let connections = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(Mutex::new(Vec::new()));

    let (count, log) = (Arc::clone(&connections), Arc::clone(&seen));
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            count.fetch_add(1, Ordering::SeqCst);
            let (acceptor, log, reply) = (acceptor.clone(), Arc::clone(&log), reply.clone());
            tokio::spawn(async move {
                let Ok(mut stream) = acceptor.accept(stream).await else {
                    return;
                };
                let name = stream.get_ref().1.server_name().map(str::to_owned);
                let mut buffer = vec![0u8; 8192];
                let read = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or(0);
                let head = String::from_utf8_lossy(&buffer[..read]).into_owned();
                log.lock().expect("log").push((name, head));
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
        connections,
        seen,
        _task: task,
    }
}

const FINISHED: &str = r#"{"id":"q","columns":[{"name":"n","type":"bigint"}],"data":[[1]],"stats":{"state":"FINISHED"}}"#;

fn ca(pem: &str) -> TlsCa {
    TlsCa::from_pem("/tmp/test-ca.pem", pem.as_bytes()).expect("a CA bundle")
}

/// What the engine hands the driver once a tunnel is open.
fn through_the_tunnel(address: SocketAddr, pem: &str) -> ConnectionConfig {
    ConnectionConfig::new(DriverKind::Trino, "127.0.0.1", address.port(), "queryhive")
        .tls(TlsMode::Require)
        .tls_ca(ca(pem))
}

async fn one_row(config: &ConnectionConfig) -> Result<usize, EngineError> {
    let mut session = TrinoDriver::new().connect(config).await?;
    let mut cursor = session
        .execute("SELECT 1", &ExecuteOptions::default())
        .await?;
    let mut rows = 0;
    while let Some(batch) = cursor.next_batch(16).await? {
        rows += batch.columns().first().map_or(0, Vec::len);
    }
    Ok(rows)
}

#[tokio::test]
async fn a_certificate_signed_by_the_named_ca_verifies_for_its_name_through_a_tunnel() {
    let pki = pki(&["db.internal"]);
    let coordinator = coordinator(&pki, FINISHED.to_owned()).await;

    let config =
        through_the_tunnel(coordinator.address, &pki.ca_pem).tls_server_name("db.internal");
    assert_eq!(
        one_row(&config)
            .await
            .expect("verified against the CA file"),
        1
    );

    let seen = coordinator.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "{seen:?}");
    // The name travelled in the handshake (SNI) and in the request, and the socket went
    // to the loopback endpoint: both halves of the split.
    assert_eq!(seen[0].0.as_deref(), Some("db.internal"));
    let expected = format!("host: db.internal:{}", coordinator.address.port());
    assert!(
        seen[0].1.to_ascii_lowercase().contains(&expected),
        "{expected} not in:\n{}",
        seen[0].1
    );
}

#[tokio::test]
async fn without_the_tls_name_the_tunnel_address_is_what_gets_checked_and_it_fails() {
    // The gap, shown before the fix: a certificate for `db.internal` does not name
    // `127.0.0.1`, so every verifying mode failed through a tunnel.
    let pki = pki(&["db.internal"]);
    let coordinator = coordinator(&pki, FINISHED.to_owned()).await;

    let error = one_row(&through_the_tunnel(coordinator.address, &pki.ca_pem))
        .await
        .expect_err("127.0.0.1 is not the name on this certificate");

    assert!(matches!(error, EngineError::Connect { .. }), "{error:?}");
    assert!(coordinator.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_certificate_the_ca_file_does_not_cover_is_refused() {
    // The platform store is not consulted as well: the bundle is all that is trusted.
    let served = pki(&["db.internal"]);
    let other = pki(&["db.internal"]);
    let coordinator = coordinator(&served, FINISHED.to_owned()).await;

    let config =
        through_the_tunnel(coordinator.address, &other.ca_pem).tls_server_name("db.internal");
    let error = one_row(&config)
        .await
        .expect_err("another CA's bundle must not verify");

    assert!(matches!(error, EngineError::Connect { .. }), "{error:?}");
    assert!(coordinator.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_ca_file_outside_the_verifying_mode_is_refused_before_a_socket_opens() {
    let pki = pki(&["db.internal"]);
    let coordinator = coordinator(&pki, FINISHED.to_owned()).await;

    for mode in [TlsMode::Prefer, TlsMode::RequireNoVerify, TlsMode::Disable] {
        let config = through_the_tunnel(coordinator.address, &pki.ca_pem)
            .tls(mode)
            .tls_server_name("db.internal");
        let error = TrinoDriver::new()
            .connect(&config)
            .await
            .err()
            .expect("refused");
        assert!(
            matches!(error, EngineError::Usage { .. }),
            "{mode:?}: {error:?}"
        );
    }
    assert_eq!(coordinator.connections.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_next_uri_with_the_coordinators_real_port_is_refused_not_dialled_on_loopback() {
    // Behind a tunnel the name is mapped to loopback, so a `nextUri` naming the
    // coordinator's own port (not the tunnel's) would be dialled on whatever listens on
    // that loopback port, with the credential in the header. The origin check turns it
    // into a named refusal first, which the second listener's silence proves.
    let pki = pki(&["db.internal"]);
    let elsewhere = coordinator(&pki, FINISHED.to_owned()).await;
    let reply = format!(
        r#"{{"id":"q","nextUri":"https://db.internal:{}/v1/statement/q/1","stats":{{"state":"QUEUED"}}}}"#,
        elsewhere.address.port()
    );
    let tunnel = coordinator(&pki, reply).await;

    let config = through_the_tunnel(tunnel.address, &pki.ca_pem)
        .tls_server_name("db.internal")
        .bearer("eyJSENTINEL.bearer-token_9f3a");
    let error = one_row(&config)
        .await
        .expect_err("the poll must be refused");

    assert_eq!(error.failure_kind(), FailureKind::Permanent, "{error:?}");
    let message = error.message();
    assert!(
        message.contains(&format!("https://db.internal:{}", tunnel.address.port())),
        "{message}"
    );
    assert!(
        message.contains(&format!("https://db.internal:{}", elsewhere.address.port())),
        "{message}"
    );
    assert_eq!(elsewhere.connections.load(Ordering::SeqCst), 0);
}
