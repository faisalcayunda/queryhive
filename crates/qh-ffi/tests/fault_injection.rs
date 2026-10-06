//! What the engine does when the network misbehaves, with faults the test makes itself.
//!
//! Three things are pinned here (W11-T7, ENG-HYGIENE; DBX-46, DBX-59, PF-23):
//!
//! 1. A peer that accepts the socket and says nothing ends a **run** at the connect bound
//!    instead of hanging. No database is needed, so this one always runs. (The driver crates
//!    prove the same for a stuck TLS exchange and login, in milliseconds.)
//! 2. A `COMMIT` whose acknowledgement is lost is reported as `written`, with the advice not
//!    to run the plan again, and the rows really are in the table (`apply.rs`: the plan's
//!    last statement ran, and only the answer was lost).
//! 3. A slow consumer of a streamed MySQL result against a statement timeout (PF-23, see the
//!    test for what was measured).
//!
//! The last two need the dev MySQL (`deploy/dev/up.sh mysql`) and `QH_TEST_MYSQL=1`.
//!
//! # Why the faults are in-process proxies and not toxiproxy
//!
//! The shared toxiproxy container publishes only its API port and 55435, and 55435 is the one
//! PostgreSQL proxy that carries permanent latency toxics for the benchmarks. A proxy a test
//! created through `:8474` would listen on a port the host cannot reach. A `TcpListener` the
//! test owns can be silent, can read the wire, and can drop one packet's answer, which is all
//! these cases need.

use std::sync::Arc;
use std::time::{Duration, Instant};

use qh_ffi::events::{Capture, Emitter};
use qh_ffi::{run, CancelFlag, Command, RealEngine, Settings};
use serde_json::Value as Json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

/// The live tests share one MySQL and one table prefix.
static TURN: Mutex<()> = Mutex::const_new(());

const MYSQL: &[(&str, &str)] = &[
    ("DB_KIND", "mysql"),
    ("DB_HOST", "127.0.0.1"),
    ("DB_USER", "qh"),
    ("DB_PASSWORD", "qh-dev-only"),
    ("DB_DATABASE", "qh"),
    ("RETRIES", "0"),
];
const MYSQL_PORT: u16 = 53306;

fn mysql_live() -> bool {
    if std::env::var("QH_TEST_MYSQL").as_deref() == Ok("1") {
        return true;
    }
    eprintln!("skipped: set QH_TEST_MYSQL=1 with deploy/dev/up.sh running");
    false
}

fn settings(extra: &[(&str, &str)], port: u16, tls: &str) -> Settings {
    let mut pairs: Vec<(String, String)> = MYSQL
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    pairs.push(("DB_PORT".to_owned(), port.to_string()));
    pairs.push(("DB_SSLMODE".to_owned(), tls.to_owned()));
    pairs.extend(
        extra
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
    );
    Settings::from_pairs(pairs)
}

async fn mysql_run(sql: &str) -> Result<Vec<Json>, qh_ffi::CliError> {
    let mut out = Capture::new();
    run(
        Command::Preview,
        &settings(&[("SQL", sql)], MYSQL_PORT, "disable"),
        &mut out,
        &RealEngine::new(),
        &CancelFlag::new(),
    )
    .await?;
    Ok(out.lines)
}

// --------------------------------------------------------------------------- //
// a silent peer
// --------------------------------------------------------------------------- //

#[tokio::test]
async fn a_run_against_a_silent_peer_ends_at_the_connect_bound() {
    // Accepts every connection and keeps it open, saying nothing.
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("address").port();
    let held = tokio::spawn(async move {
        let mut sockets = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            sockets.push(socket);
        }
    });

    for (kind, tls) in [("postgres", "disable"), ("mysql", "disable")] {
        let mut out = Capture::new();
        let started = Instant::now();
        let result = run(
            Command::Preview,
            &Settings::from_pairs([
                ("DB_KIND", kind),
                ("DB_HOST", "127.0.0.1"),
                ("DB_PORT", &port.to_string()),
                ("DB_USER", "qh"),
                ("DB_SSLMODE", tls),
                ("RETRIES", "0"),
                ("SQL", "SELECT 1"),
            ]),
            &mut out,
            &RealEngine::new(),
            &CancelFlag::new(),
        )
        .await;
        let elapsed = started.elapsed();
        let message = result
            .expect_err("a silent peer is not a session")
            .to_string();
        assert!(
            elapsed >= qh_core::CONNECT_TIMEOUT && elapsed < qh_core::CONNECT_TIMEOUT * 2,
            "{kind}: bounded by CONNECT_TIMEOUT, took {elapsed:?}"
        );
        assert!(
            message.contains("no answer within 15 s"),
            "{kind}: {message}"
        );
        assert!(message.contains("TLS handshake"), "{kind}: {message}");
    }
    held.abort();
}

// --------------------------------------------------------------------------- //
// a COMMIT whose answer is lost
// --------------------------------------------------------------------------- //

/// A MySQL proxy that lets everything through until the client sends `COMMIT`, forwards that,
/// and then closes both sides when the server answers: the server has committed, the client
/// never hears it. Plain text only (the test connects with `DB_SSLMODE=disable`), because a
/// TLS stream hides the statement.
async fn commit_eating_proxy() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("address").port();
    tokio::spawn(async move {
        while let Ok((client, _)) = listener.accept().await {
            tokio::spawn(async move {
                let Ok(server) = TcpStream::connect(("127.0.0.1", MYSQL_PORT)).await else {
                    return;
                };
                let (mut from_client, mut to_client) = client.into_split();
                let (mut from_server, mut to_server) = server.into_split();
                let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
                let flag = Arc::clone(&armed);
                let upstream = async move {
                    // MySQL packets: a 3-byte length, a sequence id, then the payload.
                    let mut buffer: Vec<u8> = Vec::new();
                    let mut chunk = [0u8; 8192];
                    loop {
                        let Ok(read) = from_client.read(&mut chunk).await else {
                            return;
                        };
                        if read == 0 {
                            return;
                        }
                        buffer.extend_from_slice(&chunk[..read]);
                        while buffer.len() >= 4 {
                            let length = usize::from(buffer[0])
                                | usize::from(buffer[1]) << 8
                                | usize::from(buffer[2]) << 16;
                            if buffer.len() < 4 + length {
                                break;
                            }
                            let packet: Vec<u8> = buffer.drain(..4 + length).collect();
                            // COM_QUERY (0x03) carrying exactly `COMMIT`.
                            if packet.get(4) == Some(&0x03)
                                && packet[5..].eq_ignore_ascii_case(b"COMMIT")
                            {
                                flag.store(true, std::sync::atomic::Ordering::SeqCst);
                            }
                            if to_server.write_all(&packet).await.is_err() {
                                return;
                            }
                        }
                    }
                };
                let downstream = async move {
                    let mut chunk = [0u8; 8192];
                    loop {
                        let Ok(read) = from_server.read(&mut chunk).await else {
                            return;
                        };
                        if read == 0 {
                            return;
                        }
                        if armed.load(std::sync::atomic::Ordering::SeqCst) {
                            // The answer to COMMIT: dropped, and the connection with it.
                            return;
                        }
                        if to_client.write_all(&chunk[..read]).await.is_err() {
                            return;
                        }
                    }
                };
                tokio::select! { () = upstream => {}, () = downstream => {} }
            });
        }
    });
    port
}

/// Tables this file creates start with this, so a run killed half way is swept by the next.
const PREFIX: &str = "qh_fault_";

async fn sweep(table: &str) {
    mysql_run(&format!("DROP TABLE IF EXISTS {table}"))
        .await
        .expect("drop a leftover");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_commit_whose_answer_is_lost_is_reported_as_written_and_the_rows_are_there() {
    let _turn = TURN.lock().await;
    if !mysql_live() {
        return;
    }
    let table = format!("{PREFIX}commit_ack");
    sweep(&table).await;
    mysql_run(&format!(
        "CREATE TABLE {table} (id INT PRIMARY KEY) ENGINE=InnoDB"
    ))
    .await
    .expect("create the scratch table");

    let proxy = commit_eating_proxy().await;
    let plan = serde_json::json!([
        {"sql": format!("INSERT INTO {table} VALUES (1)"), "expected": 1},
        {"sql": format!("INSERT INTO {table} VALUES (2)"), "expected": 1},
    ]);
    let mut out = Capture::new();
    let outcome = run(
        Command::ApplyChanges,
        &settings(&[("CHANGES", &plan.to_string())], proxy, "disable"),
        &mut out,
        &RealEngine::new(),
        &CancelFlag::new(),
    )
    .await;

    // Read through the real port, then clean up, and only then assert: a failed assertion must
    // not leave the table behind.
    let count = mysql_run(&format!("SELECT count(*) FROM {table}")).await;
    sweep(&table).await;

    let error = outcome
        .expect_err("the answer to COMMIT never came")
        .to_string();
    assert!(error.contains("COMMIT failed"), "{error}");
    assert!(error.contains("disposition: written"), "{error}");
    assert!(error.contains("look at the table"), "{error}");
    assert!(
        !out.lines.iter().any(|event| event["event"] == "done"),
        "no `done` for a plan whose outcome is unknown"
    );
    let count = count.expect("the checker runs");
    assert_eq!(
        count
            .iter()
            .find(|event| event["event"] == "rows")
            .and_then(|event| event["data"][0][0].as_str()),
        Some("2"),
        "the server did commit: only the answer was lost"
    );
}

// --------------------------------------------------------------------------- //
// a slow consumer and the statement timeout (PF-23)
// --------------------------------------------------------------------------- //

/// An event sink that takes its time over the first pages of rows, then keeps up.
///
/// Slow for long enough to overrun the statement timeout and no longer: the rows the server had
/// already sent are then read at full speed, so the test lasts seconds and not the minutes a
/// consumer that stayed slow would need to get through them.
struct SlowSink {
    inner: Capture,
    pause: Duration,
    slow_pages: usize,
}

impl Emitter for SlowSink {
    fn emit(&mut self, event: Json) -> std::io::Result<()> {
        if event["event"] == "rows" && self.slow_pages > 0 {
            self.slow_pages -= 1;
            // Blocking on purpose: this is a consumer that is slow, on a runtime with a second
            // worker for everything else.
            std::thread::sleep(self.pause);
        }
        self.inner.emit(event)
    }
}

/// What a slow consumer of one big streamed result saw.
struct Slow {
    elapsed: Duration,
    rows: usize,
    result: Result<(), qh_ffi::CliError>,
}

/// Stream a result through `preview` with a 2 s statement timeout and a consumer that takes
/// 300 ms for each of its first ten pages, so the read overruns the timeout by half again.
async fn slow_consumer(pairs: Vec<(String, String)>) -> Slow {
    let mut out = SlowSink {
        inner: Capture::new(),
        pause: Duration::from_millis(300),
        slow_pages: 10,
    };
    let started = Instant::now();
    let result = run(
        Command::Preview,
        &Settings::from_pairs(pairs),
        &mut out,
        &RealEngine::new(),
        &CancelFlag::new(),
    )
    .await;
    let rows = out
        .inner
        .lines
        .iter()
        .filter(|event| event["event"] == "rows")
        .map(|event| event["data"].as_array().map_or(0, Vec::len))
        .sum();
    Slow {
        elapsed: started.elapsed(),
        rows,
        result,
    }
}

fn big_read(base: &[(&str, &str)], port: u16, sql: &str) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = base
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    pairs.push(("DB_PORT".to_owned(), port.to_string()));
    pairs.push(("DB_SSLMODE".to_owned(), "disable".to_owned()));
    pairs.push(("SQL".to_owned(), sql.to_owned()));
    pairs.push(("LIMIT".to_owned(), CAP.to_string()));
    pairs.push(("STATEMENT_TIMEOUT_MS".to_owned(), "2000".to_owned()));
    pairs
}

/// The row cap of the run. Both statements are far bigger, so a run that is not cut ends here,
/// `truncated`, instead of reading a billion rows.
const CAP: usize = 2_000_000;

const MYSQL_BIG: &str =
    "WITH RECURSIVE n AS (SELECT 1 AS x UNION ALL SELECT x + 1 FROM n WHERE x < 1000) \
     SELECT a.x AS ax, b.x AS bx, c.x AS cx FROM n a, n b, n c";
const POSTGRES_BIG: &str = "SELECT g, repeat('x', 200) FROM generate_series(1, 5000000) AS g";

const POSTGRES: &[(&str, &str)] = &[
    ("DB_KIND", "postgres"),
    ("DB_HOST", "127.0.0.1"),
    ("DB_USER", "qh"),
    ("DB_PASSWORD", "qh-dev-only"),
    ("DB_DATABASE", "qh"),
    ("RETRIES", "0"),
];

/// What PF-23 looks like today: the run ends with the server's statement timeout, short of the
/// cap, although the server was only waiting for the consumer. When this assertion starts to
/// fail because the run reaches the cap, PF-23 is fixed: flip it to expect the cap.
fn assert_cut(slow: &Slow) {
    let message = match &slow.result {
        Err(error) => error.to_string(),
        Ok(()) => panic!(
            "not cut: {} rows in {:?}; PF-23 is fixed, flip this",
            slow.rows, slow.elapsed
        ),
    };
    assert!(message.contains("2000 ms statement timeout"), "{message}");
    assert!(slow.rows < CAP, "{} rows", slow.rows);
    assert!(slow.elapsed < Duration::from_secs(60), "{:?}", slow.elapsed);
}

/// PF-23, measured. A result the consumer reads slowly, under a statement timeout the whole
/// read overruns: is it cut?
///
/// Both drivers hand pages to the consumer through bounded channels (MySQL's producer, and
/// `tokio-postgres`'s connection task), so a slow consumer backs the server up, and the
/// server's bound (`max_execution_time`, `statement_timeout`) runs on its wall clock whether it
/// is executing or waiting to send. ADR-0016 wants the bound to be on **execution**, not on how
/// slowly a person reads. This test records what the engine does today, and fails when that
/// changes, so the finding cannot go stale unnoticed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_consumer_against_a_statement_timeout_on_mysql() {
    let _turn = TURN.lock().await;
    if !mysql_live() {
        return;
    }
    let slow = slow_consumer(big_read(MYSQL, MYSQL_PORT, MYSQL_BIG)).await;
    eprintln!(
        "PF-23 mysql: {:?}, {} rows, {:?}",
        slow.elapsed, slow.rows, slow.result
    );
    assert_cut(&slow);
    // Whatever happened, nothing may be left running on the server.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let running = mysql_run(
        "SELECT count(*) FROM information_schema.processlist \
         WHERE info LIKE 'WITH RECURSIVE n AS%'",
    )
    .await
    .expect("the checker runs");
    assert_eq!(
        running
            .iter()
            .find(|event| event["event"] == "rows")
            .and_then(|event| event["data"][0][0].as_str()),
        Some("0"),
        "the statement is still running on the server"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_consumer_against_a_statement_timeout_on_postgres() {
    let _turn = TURN.lock().await;
    if std::env::var("QH_TEST_POSTGRES").as_deref() != Ok("1") {
        eprintln!("skipped: set QH_TEST_POSTGRES=1 with deploy/dev/up.sh running");
        return;
    }
    let slow = slow_consumer(big_read(POSTGRES, 55432, POSTGRES_BIG)).await;
    eprintln!(
        "PF-23 postgres: {:?}, {} rows, {:?}",
        slow.elapsed, slow.rows, slow.result
    );
    assert_cut(&slow);
}
