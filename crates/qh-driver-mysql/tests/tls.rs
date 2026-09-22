//! TLS against real servers, plus the one case a real server here cannot produce.
//!
//! ```bash
//! crates/qh-driver-mysql/tests/mysql-tls-container.sh   # qh-mysql-tls, qh-mysql-plain
//! QH_TEST_MYSQL=1 cargo test -p qh-driver-mysql --test tls
//! ```
//!
//! Without `QH_TEST_MYSQL=1` each test prints why it is skipping and returns, and
//! so does a test whose container is not up: a silent pass would be worse than a
//! visible skip. The reachability probe is a raw TCP connect, never the driver's
//! own `connect`, so a driver that cannot connect is a failure and not a skip.
//!
//! What only a real server can prove:
//!
//! - `Prefer` encrypts a connection it does **not** verify: the server on
//!   53307 serves a certificate signed by a CA this repository generated, no public
//!   root store has heard of it, and the session still connects — with `Ssl_cipher`
//!   naming a real cipher, which is the server's own account of the encryption
//!   rather than the client's silence about an error;
//! - `Require` refuses that same certificate, which is what makes the two modes
//!   different rather than the same configuration under two names;
//! - `Prefer` falls back to plaintext against a server with no TLS at all
//!   (`--tls-version=`, on 53310), and `Require` does not;
//! - `Disable` reaches the TLS server in clear, so "the server refused plaintext"
//!   is never an explanation for a TLS failure here.
//!
//! What a real server cannot produce is a handshake that breaks *after* the server
//! offered TLS: no mode on this side verifies anything for `Prefer`, and a broken
//! handshake is the case the no-downgrade rule exists for. That one is driven by a
//! stub server below, which speaks a MySQL handshake advertising `CLIENT_SSL` and
//! then closes the socket — and counts connections, so a driver that retried in
//! clear would be visible as a second one.

use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use qh_core::{EngineError, FailureKind, Value};
use qh_driver::{ConnectionConfig, Driver, DriverKind, ExecuteOptions, Session, TlsMode};
use qh_driver_mysql::MysqlDriver;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const SKIP_HINT: &str = "skipped: set QH_TEST_MYSQL=1 with mysql-tls-container.sh running";

/// The two servers `tests/mysql-tls-container.sh` owns: TLS on, and TLS absent.
const TLS_PORT: u16 = 53307;
const PLAIN_PORT: u16 = 53310;

/// `CLIENT_SSL` from the MySQL protocol, as `mysql_common` spells it
/// (`constants.rs:166`). The one bit that decides whether the client may start a
/// TLS handshake on this connection.
const CLIENT_SSL: u32 = 0x0000_0800;

fn port(setting: &str, default: u16) -> u16 {
    std::env::var(setting)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// Whether something is listening, asked of the OS rather than of the driver.
fn listening(port: u16) -> bool {
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
    TcpStream::connect_timeout(&address.into(), Duration::from_millis(500)).is_ok()
}

fn config(port: u16, tls: TlsMode) -> ConnectionConfig {
    ConnectionConfig::new(DriverKind::Mysql, "127.0.0.1", port, "qh")
        .password("qh-dev-only")
        .database("qh")
        .tls(tls)
}

/// `Some(config)` when this test can run; `None`, having said why, when it cannot.
fn server(port: u16, tls: TlsMode, what: &str) -> Option<ConnectionConfig> {
    if std::env::var("QH_TEST_MYSQL").as_deref() != Ok("1") {
        eprintln!("{SKIP_HINT}");
        return None;
    }
    if !listening(port) {
        eprintln!(
            "skipped: nothing listening on 127.0.0.1:{port} ({what}); start it with \
             crates/qh-driver-mysql/tests/mysql-tls-container.sh"
        );
        return None;
    }
    Some(config(port, tls))
}

async fn connect(config: &ConnectionConfig) -> Result<Box<dyn Session>, EngineError> {
    MysqlDriver::new().connect(config).await
}

/// The one cell of the first row of a statement expected to return one row.
async fn scalar(session: &mut Box<dyn Session>, sql: &str) -> Option<Value> {
    let mut cursor = session
        .execute(sql, &ExecuteOptions::default())
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
    match cursor.next_batch(10).await.expect("next_batch") {
        Some(batch) => batch.value(0, 0).cloned(),
        None => None,
    }
}

/// The cipher the session negotiated, or the empty string when it is in clear.
///
/// `SHOW STATUS LIKE 'Ssl_cipher'` answers with `Variable_name`, `Value` — so the
/// second column is the one that matters, and the first is the literal string
/// `Ssl_cipher` whether or not anything is encrypted. This is the server's own
/// account of the session, which is the point: a client that returned no error
/// would say nothing about whether the bytes were encrypted.
async fn cipher(session: &mut Box<dyn Session>) -> String {
    let mut cursor = session
        .execute("SHOW STATUS LIKE 'Ssl_cipher'", &ExecuteOptions::default())
        .await
        .expect("execute SHOW STATUS");
    let batch = cursor
        .next_batch(10)
        .await
        .expect("next_batch")
        .expect("SHOW STATUS returns a row");
    match batch.value(0, 1) {
        Some(Value::Text(value)) => value.to_string(),
        other => panic!("expected the variable's value, got {other:?}"),
    }
}

/// Assert the error is a connect failure that names the certificate, and that it
/// is classified `Permanent` — a trust store does not change by retrying.
fn assert_rejected_certificate(error: EngineError, mode: TlsMode) {
    match error {
        EngineError::Connect { message, kind } => {
            assert!(
                message.contains("certificate"),
                "{mode:?} failed, but not over the certificate: {message}"
            );
            assert_eq!(
                kind,
                FailureKind::Permanent,
                "{mode:?} would be retried, and a refused certificate never becomes acceptable"
            );
        }
        other => panic!("{mode:?} should fail as a connection error, got {other:?}"),
    }
}

/// The parity rule, and the only test that can prove it: a certificate signed by a
/// CA no root store holds is still accepted by `Prefer`.
///
/// It proves the negative the way the decision requires — by connecting, not by
/// observing the absence of an error — and backs it with the server's own report
/// that the session is encrypted (`Ssl_cipher`). `Require` against the same server
/// is the control in the next test: if that one started passing, this one would no
/// longer be saying anything.
#[tokio::test]
async fn prefer_encrypts_without_verifying_an_unknown_certificate() {
    let Some(config) = server(
        port("QH_MYSQL_TLS_PORT", TLS_PORT),
        TlsMode::Prefer,
        "TLS on",
    ) else {
        return;
    };
    let mut session = connect(&config)
        .await
        .expect("Prefer does not verify, so an internal certificate is not an obstacle");
    let cipher = cipher(&mut session).await;
    assert!(
        !cipher.is_empty(),
        "the connection succeeded but the server reports no cipher, so it was not encrypted"
    );
}

/// The half of the feature that CI cannot exercise with a public certificate, and
/// the reason `Require` is a mode of its own: a certificate no root store signed is
/// refused.
///
/// The container's certificate is signed by `qh-mysql-tls-test-ca`. No public root
/// store contains that CA, so the refusal here is the root store deciding — which
/// is the only thing this driver can currently verify against, and the gap the
/// module notes in `src/tls.rs` describe.
#[tokio::test]
async fn require_refuses_a_certificate_the_public_roots_did_not_sign() {
    let Some(config) = server(
        port("QH_MYSQL_TLS_PORT", TLS_PORT),
        TlsMode::Require,
        "TLS on",
    ) else {
        return;
    };
    let error = connect(&config)
        .await
        .err()
        .expect("the container's certificate is not signed by a public CA");
    assert_rejected_certificate(error, TlsMode::Require);
}

/// The same certificate with `RequireNoVerify`: this is what the mode is for, and
/// the cipher proves the bytes really are encrypted rather than merely accepted.
#[tokio::test]
async fn require_no_verify_reaches_the_same_server_and_encrypts() {
    let Some(config) = server(
        port("QH_MYSQL_TLS_PORT", TLS_PORT),
        TlsMode::RequireNoVerify,
        "TLS on",
    ) else {
        return;
    };
    let mut session = connect(&config)
        .await
        .expect("verification is off for this mode, so the certificate is not the obstacle");
    assert!(
        !cipher(&mut session).await.is_empty(),
        "the handshake completed but the session reports no cipher, so nothing was encrypted"
    );
}

/// And the connection the TLS modes refuse to fall back to, for control: `Disable`
/// reaches the very same server, in clear.
#[tokio::test]
async fn disable_reaches_the_same_server_in_clear() {
    let Some(config) = server(
        port("QH_MYSQL_TLS_PORT", TLS_PORT),
        TlsMode::Disable,
        "TLS on",
    ) else {
        return;
    };
    let mut session = connect(&config)
        .await
        .expect("the container accepts plaintext as well as TLS");
    assert_eq!(
        cipher(&mut session).await,
        "",
        "Disable negotiated a cipher, so it did not connect in clear"
    );
}

/// The one server answer `Prefer` may fall back on: a server that cannot do TLS.
///
/// This is the `--tls-version=` container, whose handshake carries no `CLIENT_SSL`
/// capability. The connection it yields is in clear, which is what the mode asks
/// for here, and it is decided by that capability rather than by a failure.
#[tokio::test]
async fn prefer_falls_back_to_plaintext_only_when_the_server_has_no_tls() {
    let Some(config) = server(
        port("QH_MYSQL_PLAIN_PORT", PLAIN_PORT),
        TlsMode::Prefer,
        "TLS off",
    ) else {
        return;
    };
    let mut session = connect(&config)
        .await
        .expect("a server with no TLS is what Prefer falls back for");
    assert_eq!(cipher(&mut session).await, "");
    // And the fallback produces a working session, not just an open socket.
    assert_eq!(
        scalar(&mut session, "SELECT 1").await,
        Some(Value::Int(1)),
        "the fallback connection is not usable"
    );
}

/// `Require` on that same server: no TLS to require means no connection. Without
/// this, `Prefer`'s fallback could be "any server without TLS" and nothing would
/// notice.
#[tokio::test]
async fn require_connects_to_no_server_that_has_no_tls_at_all() {
    let Some(config) = server(
        port("QH_MYSQL_PLAIN_PORT", PLAIN_PORT),
        TlsMode::Require,
        "TLS off",
    ) else {
        return;
    };
    let error = connect(&config)
        .await
        .err()
        .expect("Require must not accept a server that cannot encrypt");
    assert!(
        matches!(error, EngineError::Connect { .. }),
        "expected a connection error, got {error:?}"
    );
}

/// The no-downgrade rule, which is what keeps `Prefer`'s missing verification from
/// mattering: a server that offers TLS and then breaks the handshake is an error,
/// in every mode, and the connection is never re-made in clear.
///
/// The stub below is the only way to produce that here — it advertises `CLIENT_SSL`
/// in a real handshake packet and then closes the socket — and it counts what it
/// sees per connection, so a fallback would show up as a second connection carrying
/// an authentication packet instead of an `SSLRequest`.
#[tokio::test]
async fn a_handshake_that_breaks_is_an_error_and_never_a_retry_in_clear() {
    for mode in [TlsMode::Prefer, TlsMode::Require] {
        let (port, seen) = stub_offering_tls().await;

        let error = connect(&config(port, mode))
            .await
            .err()
            .unwrap_or_else(|| panic!("{mode:?} accepted a connection whose TLS handshake broke"));

        assert!(
            matches!(error, EngineError::Connect { .. }),
            "{mode:?}: expected a connection error, got {error:?}"
        );

        let seen = seen.lock().expect("the stub's log").clone();
        assert_eq!(
            seen.len(),
            1,
            "{mode:?} opened {} connections: it retried after the handshake failed, which is the \
             downgrade this rule forbids",
            seen.len()
        );
        assert!(
            seen[0].ssl_request,
            "{mode:?} sent a {} byte packet instead of an SSLRequest, so it never tried TLS at all",
            seen[0].packet_length
        );
    }
}

/// What one connection to the stub looked like from the server's side.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Observed {
    /// The length the client declared in the packet header.
    packet_length: usize,
    /// Whether that packet was an `SSLRequest`: 32 bytes with `CLIENT_SSL` set,
    /// which is the client beginning a TLS handshake and nothing else.
    ssl_request: bool,
}

/// A MySQL server that advertises TLS and then breaks the handshake.
///
/// Returns its port and the log of what it saw. It accepts twice, so a client that
/// fell back to plaintext would be recorded rather than refused at the socket.
/// Everything after the first client packet is dropped unanswered, which is a
/// handshake that fails for a reason no client-side setting can dismiss.
async fn stub_offering_tls() -> (u16, Arc<Mutex<Vec<Observed>>>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .expect("bind a stub server");
    let port = listener.local_addr().expect("the stub's address").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);

    tokio::spawn(async move {
        // Bounded on purpose: a driver that keeps retrying would hang the test
        // rather than be counted, and two accepts is enough to see one retry.
        for _ in 0..2 {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            if socket.write_all(&handshake_offering_tls()).await.is_err() {
                return;
            }
            let mut header = [0u8; 4];
            if socket.read_exact(&mut header).await.is_err() {
                return;
            }
            let length = u32::from_le_bytes([header[0], header[1], header[2], 0]) as usize;
            let mut payload = vec![0u8; length];
            if socket.read_exact(&mut payload).await.is_err() {
                return;
            }
            let ssl_request = length == 32
                && u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]])
                    & CLIENT_SSL
                    != 0;
            recorded.lock().expect("the stub's log").push(Observed {
                packet_length: length,
                ssl_request,
            });
            // Everything from here — a TLS ClientHello, or an authentication
            // response — is dropped. The client's handshake fails either way; what
            // the test reads is whether it came back for a second, plaintext one.
            drop(socket);
        }
    });

    (port, seen)
}

/// A `HandshakeV10` packet that offers TLS, in the layout `mysql_common` parses
/// (`packets/mod.rs:1605`): a 4-byte header, then protocol version, server version,
/// 31 fixed bytes, the second half of the scramble, and the plugin name.
fn handshake_offering_tls() -> Vec<u8> {
    const CLIENT_PROTOCOL_41: u32 = 0x0000_0200;
    const CLIENT_SECURE_CONNECTION: u32 = 0x0000_8000;
    const CLIENT_PLUGIN_AUTH: u32 = 0x0008_0000;

    let capabilities =
        CLIENT_SSL | CLIENT_PROTOCOL_41 | CLIENT_SECURE_CONNECTION | CLIENT_PLUGIN_AUTH;

    let mut payload = Vec::new();
    payload.push(10u8); // protocol version
    payload.extend_from_slice(b"8.0.0-stub\0");
    payload.extend_from_slice(&42u32.to_le_bytes()); // connection id
    payload.extend_from_slice(&[0x11; 8]); // scramble, first half
    payload.push(0); // filler
    payload.extend_from_slice(&(capabilities as u16).to_le_bytes()); // capabilities, lower
    payload.push(45); // utf8mb4_general_ci
    payload.extend_from_slice(&2u16.to_le_bytes()); // status flags
    payload.extend_from_slice(&((capabilities >> 16) as u16).to_le_bytes()); // capabilities, upper
    payload.push(21); // length of the whole scramble
    payload.extend_from_slice(&[0; 6]); // reserved
    payload.extend_from_slice(&[0; 4]); // MariaDB extended capabilities
    payload.extend_from_slice(&[0x22; 12]); // scramble, second half
    payload.push(0);
    payload.extend_from_slice(b"mysql_native_password\0");

    let mut packet = Vec::with_capacity(payload.len() + 4);
    let length = payload.len() as u32;
    packet.extend_from_slice(&length.to_le_bytes()[..3]);
    packet.push(0); // sequence id
    packet.extend_from_slice(&payload);
    packet
}
