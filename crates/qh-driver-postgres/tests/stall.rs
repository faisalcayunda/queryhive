//! A peer that stops answering ends the connect at the bound, and the error says where.
//!
//! No database and no container: every test opens a socket of its own and plays the server up
//! to the stage it wants to get stuck in. `tokio-postgres` bounds only the TCP connect, so
//! before `CONNECT_TIMEOUT` a server that accepted the socket and went quiet, or one that
//! agreed to TLS and never handshook, held the caller until the user pressed Stop.

use std::time::{Duration, Instant};

use qh_core::{EngineError, FailureKind};
use qh_driver::{ConnectionConfig, DriverKind, TlsMode};
use qh_driver_postgres::PostgresDriver;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const LIMIT: Duration = Duration::from_millis(300);

/// Listen on a loopback port and run `play` on every connection, forever. The sockets stay open
/// while `play` is awaiting, which is the point: the peer is alive and silent.
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

/// Connect to `port` under [`LIMIT`] and require the bounded, named, retryable failure.
async fn expect_timeout(port: u16, tls: TlsMode) {
    let config = ConnectionConfig::new(DriverKind::Postgres, "127.0.0.1", port, "qh")
        .password("not-a-secret")
        .tls(tls);
    let started = Instant::now();
    let outcome = PostgresDriver::new().connect_within(&config, LIMIT).await;
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
    // Reads the SSLRequest, agrees to TLS, and never answers the ClientHello.
    let port = peer(|mut socket| async move {
        let mut request = [0u8; 8];
        let _ = socket.read_exact(&mut request).await;
        let _ = socket.write_all(b"S").await;
        hold(socket).await;
    })
    .await;
    expect_timeout(port, TlsMode::Require).await;
}

#[tokio::test]
async fn a_login_that_is_never_answered_ends_at_the_bound() {
    // Reads the startup message and asks for a cleartext password, then never says `AuthenticationOk`.
    let port = peer(|mut socket| async move {
        let mut length = [0u8; 4];
        if socket.read_exact(&mut length).await.is_ok() {
            let rest = u32::from_be_bytes(length) as usize - 4;
            let mut startup = vec![0u8; rest];
            let _ = socket.read_exact(&mut startup).await;
            let _ = socket.write_all(&[b'R', 0, 0, 0, 8, 0, 0, 0, 3]).await;
        }
        hold(socket).await;
    })
    .await;
    expect_timeout(port, TlsMode::Disable).await;
}
