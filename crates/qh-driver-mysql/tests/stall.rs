//! A peer that stops answering ends the connect at the bound, and the error says where.
//!
//! No database and no container: every test opens a socket of its own and plays the server up
//! to the stage it wants to get stuck in. `mysql_async` has no connect timeout at all, so
//! before `CONNECT_TIMEOUT` a server that accepted the socket and went quiet, or one that
//! offered TLS and never handshook, held the caller until the user pressed Stop.

use std::time::{Duration, Instant};

use qh_core::{EngineError, FailureKind};
use qh_driver::{ConnectionConfig, DriverKind, TlsMode};
use qh_driver_mysql::MysqlDriver;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

const LIMIT: Duration = Duration::from_millis(300);

/// Listen on a loopback port and run `play` on every connection, forever.
async fn peer<F, Fut>(play: F) -> u16
where
    F: Fn(TcpStream) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("address").port();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(play(socket));
        }
    });
    port
}

async fn hold(socket: TcpStream) {
    let _socket = socket;
    std::future::pending::<()>().await;
}

/// A protocol-10 greeting, with or without the `CLIENT_SSL` capability.
fn greeting(offer_tls: bool) -> Vec<u8> {
    // PROTOCOL_41 | SECURE_CONNECTION (| SSL) in the low word, PLUGIN_AUTH in the high one.
    let low: u16 = 0x0200 | 0x8000 | if offer_tls { 0x0800 } else { 0 };
    let high: u16 = 0x0008;
    let mut body = vec![10];
    body.extend_from_slice(b"8.4.0\0");
    body.extend_from_slice(&1u32.to_le_bytes());
    body.extend_from_slice(b"abcdefgh\0");
    body.extend_from_slice(&low.to_le_bytes());
    body.extend_from_slice(&[0xff, 0x02, 0x00]); // utf8mb4, autocommit
    body.extend_from_slice(&high.to_le_bytes());
    body.push(21); // auth data length
    body.extend_from_slice(&[0; 10]);
    body.extend_from_slice(b"ijklmnopqrst\0");
    body.extend_from_slice(b"mysql_native_password\0");
    let mut packet = (body.len() as u32).to_le_bytes()[..3].to_vec();
    packet.push(0); // sequence id
    packet.extend(body);
    packet
}

/// Connect to `port` under [`LIMIT`] and require the bounded, named, retryable failure.
async fn expect_timeout(port: u16, tls: TlsMode) {
    let config = ConnectionConfig::new(DriverKind::Mysql, "127.0.0.1", port, "qh")
        .password("not-a-secret")
        .tls(tls);
    let started = Instant::now();
    let outcome = MysqlDriver::new().connect_within(&config, LIMIT).await;
    let elapsed = started.elapsed();
    let Err(error) = outcome else {
        panic!("a peer that never answers produced a session");
    };
    assert!(
        elapsed >= LIMIT && elapsed < LIMIT * 10,
        "bounded by the limit, took {elapsed:?}"
    );
    assert_eq!(error.failure_kind(), FailureKind::Transient, "{error}");
    assert!(matches!(error, EngineError::Connect { .. }), "{error}");
    let message = error.message();
    assert!(message.contains("no answer within 0.3 s"), "{message}");
    assert!(message.contains("TLS handshake"), "{message}");
    assert!(!message.contains("not-a-secret"), "{message}");
}

#[tokio::test]
async fn a_server_that_accepts_the_socket_and_says_nothing_ends_at_the_bound() {
    expect_timeout(peer(hold).await, TlsMode::Disable).await;
}

#[tokio::test]
async fn a_tls_exchange_that_never_finishes_ends_at_the_bound() {
    // Offers TLS in the greeting, takes the client's SSL request, and never answers the ClientHello.
    let port = peer(|mut socket| async move {
        let _ = socket.write_all(&greeting(true)).await;
        hold(socket).await;
    })
    .await;
    expect_timeout(port, TlsMode::Require).await;
}

#[tokio::test]
async fn a_login_that_is_never_answered_ends_at_the_bound() {
    // A greeting and then silence: the client sends its credentials and waits for the verdict.
    let port = peer(|mut socket| async move {
        let _ = socket.write_all(&greeting(false)).await;
        hold(socket).await;
    })
    .await;
    expect_timeout(port, TlsMode::Disable).await;
}
