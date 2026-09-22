//! TLS, against a server that really serves a certificate — and against one that
//! declines to.
//!
//! ```bash
//! crates/qh-driver-postgres/tests/tls/up.sh
//! QH_TEST_POSTGRES=1 cargo test -p qh-driver-postgres --test tls
//! ```
//!
//! Two halves, and they need different servers
//! -------------------------------------------
//! The tests that need a real certificate run against the `qh-pg-tls` container on
//! 127.0.0.1:55433, whose certificate is signed by `tests/tls/ca.crt`. That CA can
//! never be in the machine's Keychain, so those tests go through
//! [`PostgresDriver::connect_with_verifier`] with a root store they build from the
//! file — the same code path as production, with a store the test controls. They are
//! gated on `QH_TEST_POSTGRES=1` and print why they skip when it is unset.
//!
//! The tests that need a server **without** TLS, or one that says "no" to the
//! TLS negotiation, run against a socket this file opens itself, because the
//! interesting bytes are the ones the client sends in the clear — which a real
//! server would answer instead of showing. Those run always, with no container.
//!
//! What each mode must do here
//! ---------------------------
//! - `Require` verifies, and fails against a certificate its store does not hold.
//! - `Prefer` does **not** verify: it is the mode an existing connection to an
//!   internal, self-signed server uses, so a store that holds nothing must not stop
//!   it. It still does not fall back when the server offered TLS and the handshake
//!   failed: that downgrade is the feature under test.
//! - `RequireNoVerify` connects to the very certificate `Require` refused, and it
//!   still refuses to speak in clear.
//! - `Prefer` falls back to plaintext only when the server itself declines TLS.

use std::sync::Arc;
use std::time::Duration;

use qh_core::{FailureKind, Value};
use qh_driver::{ConnectionConfig, Driver, DriverKind, ExecuteOptions, Session, TlsMode};
use qh_driver_postgres::{tls, PostgresDriver};
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::CertificateDer;
use rustls::RootCertStore;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SKIP_HINT: &str =
    "skipped: set QH_TEST_POSTGRES=1 with crates/qh-driver-postgres/tests/tls/up.sh running";

/// The first four bytes of a PostgreSQL message are its length; the next four are
/// its code. These two codes are the whole negotiation:
///
/// - `SSLRequest` is length 8 followed by 80877103, the client's question.
/// - `196608` is protocol version 3.0, which is only ever sent inside a plaintext
///   StartupMessage — so seeing it is proof that what follows is unencrypted.
const SSL_REQUEST_CODE: u32 = 80877103;
const PROTOCOL_VERSION_3: u32 = 196608;

/// The TLS container's settings. Same shape as the plaintext fixture in
/// `tests/integration.rs`, on the port `tests/tls/up.sh` publishes.
fn tls_config(mode: TlsMode) -> Option<ConnectionConfig> {
    if std::env::var("QH_TEST_POSTGRES").as_deref() != Ok("1") {
        return None;
    }
    Some(
        ConnectionConfig::new(
            DriverKind::Postgres,
            std::env::var("QH_PG_TLS_HOST").unwrap_or_else(|_| "127.0.0.1".to_owned()),
            std::env::var("QH_PG_TLS_PORT")
                .ok()
                .and_then(|port| port.parse().ok())
                .unwrap_or(55433),
            "qh",
        )
        .password("qh-dev-only")
        .database("qh")
        .tls(mode),
    )
}

/// A root store holding the CA in `tests/tls/<name>`.
///
/// Note the deliberate absence of a fallback to the platform store: if the file
/// is missing, the test must fail rather than silently verify against the
/// machine's roots, which would make the refusing tests pass for the wrong reason.
fn store_with(name: &str) -> RootCertStore {
    let path = format!("{}/tests/tls/{name}", env!("CARGO_MANIFEST_DIR"));
    let pem = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("{}: {error} — run tests/tls/make-certs.sh", path));
    let mut reader = pem.as_slice();
    let certificates: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<_, _>>()
        .expect("the fixture is a PEM certificate");
    assert!(!certificates.is_empty(), "{path} holds no certificate");

    let mut roots = RootCertStore::empty();
    for certificate in certificates {
        roots.add(certificate).expect("a usable CA certificate");
    }
    roots
}

/// Whether the connection the server sees is encrypted.
///
/// Asked of the server rather than inferred from the absence of an error: a
/// connection that silently fell back to plaintext would also succeed, and
/// `pg_stat_ssl` is the server's own answer about which one it is talking.
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

/// Connect, and check with the server that the session is encrypted and usable.
///
/// Both halves matter: a handshake that succeeded but left an unusable connection
/// is not success, and a connection the server says is in clear is not TLS —
/// which is the failure a mode that quietly downgraded would produce.
async fn connect_and_confirm_encrypted(
    config: &ConnectionConfig,
    verifier: Arc<dyn ServerCertVerifier>,
) {
    let mut session = PostgresDriver::new()
        .connect_with_verifier(config, verifier)
        .await
        .expect("connect");
    assert!(
        server_says_encrypted(&mut session).await,
        "the connection is not encrypted"
    );

    let mut cursor = session
        .execute("SELECT 1::int4", &ExecuteOptions::default())
        .await
        .expect("the encrypted session should be usable");
    let batch = cursor
        .next_batch(1)
        .await
        .expect("next_batch")
        .expect("SELECT 1 returns one row");
    assert_eq!(batch.value(0, 0), Some(&Value::Int(1)));
}

#[tokio::test]
async fn require_verifies_a_certificate_the_named_store_holds() {
    // The "trusts it" half. `tests/tls/server.crt` is signed by `tests/tls/ca.crt`,
    // so a store holding that CA must accept it — and the server must confirm the
    // session really is encrypted, which is the part a fallback would fake.
    let Some(config) = tls_config(TlsMode::Require) else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let verifier = tls::verifier_with_roots(store_with("ca.crt")).expect("a verifier");

    let mut session = PostgresDriver::new()
        .connect_with_verifier(&config, verifier)
        .await
        .expect("the test CA signed this server's certificate");
    assert!(
        server_says_encrypted(&mut session).await,
        "the connection is not encrypted"
    );
    session.close().await.expect("close");
}

#[tokio::test]
async fn require_refuses_a_certificate_the_store_does_not_trust() {
    // The other half, and the reason the first one is meaningful: `other-ca.crt`
    // signed nothing this server presents, so the handshake must fail. This
    // failure is the feature — a client that accepted this certificate would
    // accept anything an attacker on the network put in front of it.
    let Some(config) = tls_config(TlsMode::Require) else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let verifier = tls::verifier_with_roots(store_with("other-ca.crt")).expect("a verifier");

    let error = PostgresDriver::new()
        .connect_with_verifier(&config, verifier)
        .await
        .err()
        .expect("a certificate signed by another CA must not be accepted");

    assert!(
        error.message().contains("TLS"),
        "expected a handshake failure, got {error:?}"
    );
    // Retrying cannot make a certificate trusted.
    assert_eq!(error.failure_kind(), FailureKind::Permanent, "{error:?}");
}

#[tokio::test]
async fn prefer_does_not_downgrade_when_the_server_offers_tls_and_the_handshake_fails() {
    // The distinction the mode exists for: `Prefer` may continue in clear only when the
    // server answers the SSLRequest with "no". Here the server answers "yes" and then hangs
    // up, so the handshake fails for its own reason -- not because a certificate was
    // rejected, which this mode no longer does. A fake server is what makes the test
    // independent of the trust decision: with a real one, the only way to fail the
    // handshake was to present a certificate the store did not hold, and that is now a
    // perfectly successful `Prefer` connection.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a port for the fake server");
    let port = listener.local_addr().expect("a bound address").port();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("one connection");
        let mut opening = [0u8; 8];
        if socket.read_exact(&mut opening).await.is_err() {
            return Vec::new();
        }
        assert_eq!(
            u32::from_be_bytes(opening[4..].try_into().expect("four bytes")),
            SSL_REQUEST_CODE,
            "Prefer must ask the server about TLS before anything else"
        );
        socket.write_all(b"S").await.expect("say yes to TLS");

        // Everything the client sends from here, until it gives up. The socket is dropped
        // after this, which is the failed handshake.
        let mut after = Vec::new();
        let _ = tokio::time::timeout(
            Duration::from_millis(300),
            socket.read_to_end(&mut after),
        )
        .await;
        after
    });

    let config = ConnectionConfig::new(DriverKind::Postgres, "127.0.0.1", port, "qh")
        .password("qh-dev-only")
        .database("qh")
        .tls(TlsMode::Prefer);

    let outcome = PostgresDriver::new().connect(&config).await;
    assert!(
        outcome.is_err(),
        "Prefer downgraded to plaintext after the server offered TLS, which is the attack"
    );

    // And the server's own record of what it received: a TLS record starts with 0x16, while
    // a PostgreSQL StartupMessage starts with a zero byte (the high half of its length). If
    // anything was sent after "yes", it must have been the handshake, never a plaintext
    // session.
    let after = server.await.expect("the fake server");
    if let Some(first) = after.first() {
        assert_eq!(
            *first, 0x16,
            "the client spoke something that is not a TLS record after the server said yes"
        );
    }
}

#[tokio::test]
async fn the_platform_store_is_what_decides_when_no_verifier_is_named() {
    // `Driver::connect` uses the platform store — the Keychain on macOS — and the
    // TLS container's certificate is not in it, so this must fail. If the driver
    // had stopped verifying, this test would pass a certificate that no store
    // holds, and the failure above would be a test of nothing.
    let Some(config) = tls_config(TlsMode::Require) else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let error = PostgresDriver::new()
        .connect(&config)
        .await
        .err()
        .expect("a test certificate is not in this machine's platform store");

    assert!(
        error.message().contains("TLS"),
        "expected a handshake failure, got {error:?}"
    );
}

#[tokio::test]
async fn require_no_verify_connects_to_the_certificate_require_refused() {
    // The one configuration with no verification at all: it must be reachable only
    // by asking for it, and it must actually work — an implementation that ignored
    // the mode and verified anyway would leave the user with no escape hatch. The
    // verifier passed here is deliberately one that would refuse: the mode ignores
    // it, because the mode is "do not verify".
    let Some(config) = tls_config(TlsMode::RequireNoVerify) else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let refusing_verifier =
        tls::verifier_with_roots(store_with("other-ca.crt")).expect("a verifier");

    connect_and_confirm_encrypted(&config, refusing_verifier).await;
}

#[tokio::test]
async fn prefer_connects_without_verifying_and_still_comes_up_encrypted() {
    // The mode's whole point, and the thing that keeps an upgrade from breaking a working
    // connection: the store holds the *other* CA, so a verifying mode would refuse this
    // certificate -- and `Prefer` must not. The assertion is not "no error was raised" but
    // the server's own report that the session is encrypted, because a silent fallback to
    // plaintext would also raise no error.
    let Some(config) = tls_config(TlsMode::Prefer) else {
        eprintln!("{SKIP_HINT}");
        return;
    };
    let refusing_verifier =
        tls::verifier_with_roots(store_with("other-ca.crt")).expect("a verifier");

    connect_and_confirm_encrypted(&config, refusing_verifier).await;
}

/// A server that answers the TLS negotiation either way, and reports the bytes it
/// saw.
///
/// What it returns is the evidence: the first eight bytes are the client's opening
/// message, so they say whether an SSLRequest was sent at all, and anything that
/// arrives *after* a "no" is a message sent in the clear.
async fn negotiate_against_a_server_that(
    declines: bool,
) -> (u16, tokio::task::JoinHandle<Vec<u8>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a port for the fake server");
    let port = listener.local_addr().expect("a bound address").port();
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("one connection");

        let mut opening = [0u8; 8];
        if socket.read_exact(&mut opening).await.is_err() {
            return Vec::new();
        }

        // Only a real SSLRequest gets an answer; anything else is already a
        // plaintext startup, and this server has nothing to negotiate about.
        if u32::from_be_bytes(opening[4..].try_into().expect("four bytes")) == SSL_REQUEST_CODE
            && declines
        {
            socket.write_all(b"N").await.expect("say no to TLS");
        }

        let mut evidence = opening.to_vec();
        if let Some(message) = read_message(&mut socket).await {
            evidence.extend_from_slice(&message);
        }
        evidence
    });
    (port, handle)
}

/// Read one length-prefixed message, or nothing if the client stopped talking.
async fn read_message(socket: &mut tokio::net::TcpStream) -> Option<Vec<u8>> {
    const MAX: usize = 64 * 1024;
    let mut length = [0u8; 4];
    // A client that refused the plaintext path closes the connection here instead
    // of sending anything, and that EOF is what "it stopped" looks like.
    socket.read_exact(&mut length).await.ok()?;
    let total = u32::from_be_bytes(length) as usize;
    if !(4..=MAX).contains(&total) {
        return None;
    }
    let mut message = Vec::with_capacity(total);
    message.extend_from_slice(&length);
    let mut body = vec![0u8; total - 4];
    socket.read_exact(&mut body).await.ok()?;
    message.extend_from_slice(&body);
    Some(message)
}

/// Connect to the fake server and let it report what it saw.
///
/// The result of the connect is deliberately dropped: what is being tested is the
/// bytes on the wire, and the fake server does not speak enough PostgreSQL for a
/// connection to complete. A timeout keeps a client that waits for an answer this
/// server will never send from hanging the test.
async fn bytes_the_server_saw(mode: TlsMode, declines: bool) -> Vec<u8> {
    let (port, server) = negotiate_against_a_server_that(declines).await;
    let config = ConnectionConfig::new(DriverKind::Postgres, "127.0.0.1", port, "qh")
        .password("qh-dev-only")
        .database("qh")
        .tls(mode);
    let verifier = tls::verifier_with_roots(store_with("ca.crt")).expect("a verifier");

    let driver = PostgresDriver::new();
    let connect = driver.connect_with_verifier(&config, verifier);
    let _ = tokio::time::timeout(Duration::from_secs(3), connect).await;

    tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .expect("the fake server should be done")
        .expect("the fake server should not panic")
}

#[tokio::test]
async fn require_refuses_to_speak_in_clear_when_the_server_declines_tls() {
    // The server says "no TLS". `Require` must stop there, not continue in clear.
    let seen = bytes_the_server_saw(TlsMode::Require, true).await;

    assert_eq!(
        u32::from_be_bytes(seen[..4].try_into().expect("four bytes")),
        8,
        "the client should have opened with the eight-byte SSLRequest"
    );
    assert_eq!(
        u32::from_be_bytes(seen[4..8].try_into().expect("four bytes")),
        SSL_REQUEST_CODE
    );
    assert_eq!(
        seen.len(),
        8,
        "nothing may follow the refused SSLRequest: {:?} went out in the clear",
        String::from_utf8_lossy(&seen[8..])
    );
}

#[tokio::test]
async fn require_no_verify_still_refuses_to_speak_in_clear() {
    // "Do not verify the certificate" is not "do not use TLS". The mode must still
    // require the negotiation, or it would be a plaintext mode wearing a TLS label.
    let seen = bytes_the_server_saw(TlsMode::RequireNoVerify, true).await;

    assert_eq!(
        u32::from_be_bytes(seen[4..8].try_into().expect("four bytes")),
        SSL_REQUEST_CODE,
        "RequireNoVerify must still ask for TLS"
    );
    assert_eq!(
        seen.len(),
        8,
        "RequireNoVerify sent {} bytes in the clear after the refusal",
        seen.len() - 8
    );
}

#[tokio::test]
async fn prefer_continues_in_clear_only_after_the_server_declines_tls() {
    // The one case where a fallback is correct: the server itself said it does not
    // offer TLS. What the fake server saw after that "no" is a plaintext
    // StartupMessage — protocol 3.0, no TLS record — which is the fallback working
    // as intended rather than as a downgrade.
    let seen = bytes_the_server_saw(TlsMode::Prefer, true).await;

    assert_eq!(
        u32::from_be_bytes(seen[4..8].try_into().expect("four bytes")),
        SSL_REQUEST_CODE,
        "Prefer should still ask the server whether it offers TLS"
    );
    assert!(
        seen.len() > 8,
        "Prefer should have continued in clear after the server declined"
    );
    assert_eq!(
        u32::from_be_bytes(seen[8..12].try_into().expect("four bytes")),
        (seen.len() - 8) as u32,
        "the bytes after the refusal should be one length-prefixed message, and its \
         length field should account for all of them"
    );
    assert_eq!(
        u32::from_be_bytes(seen[12..16].try_into().expect("four bytes")),
        PROTOCOL_VERSION_3,
        "the message after the refusal is not a plaintext StartupMessage"
    );
}

#[tokio::test]
async fn disable_does_not_ask_for_tls_at_all() {
    // The control for the tests above: with TLS off there is no SSLRequest, so the
    // eight bytes the server sees first are a StartupMessage. Without this, a driver
    // that always sent an SSLRequest would still pass every assertion above.
    let seen = bytes_the_server_saw(TlsMode::Disable, false).await;

    assert_eq!(
        u32::from_be_bytes(seen[4..8].try_into().expect("four bytes")),
        PROTOCOL_VERSION_3,
        "Disable sent an SSLRequest, which it never should"
    );
}
