//! `preview` against a real PostgreSQL server, with a Stop landing between its pages.
//!
//! ```bash
//! deploy/dev/up.sh postgres
//! QH_TEST_POSTGRES=1 cargo test -p qh-ffi --test real_server
//! ```
//!
//! Without `QH_TEST_POSTGRES=1` both tests print why they are skipping and return, for
//! the reason `crates/qh-driver-postgres/tests/integration.rs` gives: a green run that
//! tested nothing is worse than a visible skip.
//!
//! # What this adds over the two layers that already cover cancelling
//!
//! The driver's own integration tests prove that `Session::cancel` reaches a real
//! server. `golden.rs` proves, through a fake cursor, that `emit_batches` asks the flag
//! before each fetch and hands on the page it already holds. Neither drives the flag
//! from outside a **real** run, with a real socket and a real cursor, so neither can
//! show that the check and the driver agree on what state a half-read statement leaves
//! behind: that the command still finishes, closes its session, and reports the page it
//! paid for. That is the pair below.
//!
//! # Why the stop is raised from inside the run rather than from a timer
//!
//! `emit_batches` hands page one over and *then* asks the flag before fetching page
//! two, so a stop raised while page one goes out is seen at exactly that check and page
//! two is never asked for. Raising it here, from the event sink, turns the assertion
//! into an ordering rather than a race: a `tokio::time::sleep` of some milliseconds
//! would hold only on a machine slow enough to still be inside the fetch, which is why
//! a first attempt at this file asserted 400 ms against a real server and measured a
//! 160-second run instead (see the module note at the bottom).
//!
//! # What a Stop cannot do, and why the query below is shaped for that
//!
//! A Stop raised *between* pages is seen at the check before the next fetch, and the
//! test below keeps the row count over the page size and no sleep at all, because that
//! assertion is about *which* pages are fetched, not about how long they took. A Stop
//! raised while a fetch or the `execute` is *in flight* is a different case: the wait
//! itself is raced against the flag and `session.cancel()` reaches the server, which the
//! two `pg_sleep(30)` tests at the bottom time from outside, through `pg_stat_activity`.
//! (An earlier version of this engine could not do that: a stop 400 ms into a
//! `pg_sleep(0.4)`-per-row statement did not return for 160.6 s.)

use std::io;

use qh_ffi::events::{Capture, Emitter};
use qh_ffi::{run, CancelFlag, Command, RealEngine, Settings};
use serde_json::Value as Json;

/// The timing tests measure a server's answer to a Stop in milliseconds, and the mid-stream
/// MySQL tests keep a core busy for seconds: taking turns keeps the first from measuring the
/// second.
static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const SKIP_HINT: &str = "skipped: set QH_TEST_POSTGRES=1 with deploy/dev/up.sh postgres running";

/// Two hundred rows for the first page, ten past the limit behind it: the cap of 210
/// is not a multiple of the page size, so a run nobody stopped has to fetch page two to
/// learn that more exists. That is what makes `truncated` the discriminating field
/// below rather than a value both runs happen to share.
const SQL: &str = "SELECT g, pg_sleep(0) IS NULL AS zzz FROM generate_series(1, 260) AS g";
const LIMIT: &str = "210";

/// The dev container's settings, matching `deploy/dev/up.sh` and the live golden cases.
fn settings() -> Option<Vec<(&'static str, &'static str)>> {
    if std::env::var("QH_TEST_POSTGRES").as_deref() != Ok("1") {
        return None;
    }
    Some(vec![
        ("DB_KIND", "postgres"),
        ("DB_HOST", "127.0.0.1"),
        ("DB_PORT", "55432"),
        ("DB_USER", "qh"),
        ("DB_PASSWORD", "qh-dev-only"),
        ("DB_DATABASE", "qh"),
        ("DB_SCHEMA", "public"),
        ("DB_SSLMODE", "disable"),
        // The container is either there or the test should say so now rather than after
        // five connect attempts.
        ("RETRIES", "0"),
    ])
}

fn pairs(env: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = env
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    pairs.push(("SQL".to_owned(), SQL.to_owned()));
    pairs.push(("LIMIT".to_owned(), LIMIT.to_owned()));
    pairs
}

/// An event sink that presses Stop itself, at the moment page one goes out.
///
/// `emit_batches` emits the page and only then asks the flag, so a stop raised here is
/// always seen at that check. Nothing is timed and nothing races.
struct StopAfterPageOne {
    inner: Capture,
    cancel: CancelFlag,
    stopped: bool,
}

impl Emitter for StopAfterPageOne {
    fn emit(&mut self, event: Json) -> io::Result<()> {
        if !self.stopped && event["event"] == "rows" {
            self.stopped = true;
            self.cancel.request();
        }
        self.inner.emit(event)
    }
}

/// Every row the `rows` events carried, in order.
fn data(events: &[Json]) -> Vec<Json> {
    events
        .iter()
        .filter(|line| line["event"] == "rows")
        .flat_map(|line| match &line["data"] {
            Json::Array(rows) => rows.clone(),
            other => panic!("`data` is an array of rows, got {other}"),
        })
        .collect()
}

fn lines(events: &[Json]) -> String {
    events
        .iter()
        .map(|event| serde_json::to_string(event).expect("serialisable"))
        .collect::<Vec<_>>()
        .join("\n  ")
}

#[tokio::test]
async fn a_stop_on_a_live_preview_keeps_page_one_and_never_asks_for_page_two() {
    let Some(env) = settings() else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let cancel = CancelFlag::new();
    let mut out = StopAfterPageOne {
        inner: Capture::new(),
        cancel: cancel.clone(),
        stopped: false,
    };
    let settings = Settings::from_pairs(pairs(&env));
    let engine = RealEngine::new();
    run(Command::Preview, &settings, &mut out, &engine, &cancel)
        .await
        .expect("a stopped preview is not a failed preview");

    assert!(
        cancel.is_cancelled(),
        "the stop really reached the flag this run polls"
    );
    let events = out.inner.lines;
    assert!(out.stopped, "page one went out: {}", lines(&events));

    // `columns` arrives before any row, which is what lets the grid paint its header
    // while the first page is still being pulled.
    let columns = events
        .iter()
        .find(|line| line["event"] == "columns")
        .unwrap_or_else(|| panic!("no `columns` event: {}", lines(&events)));
    assert_eq!(columns["columns"][0]["name"], "g", "{}", lines(&events));
    assert_eq!(columns["columns"][1]["name"], "zzz", "{}", lines(&events));

    let done = events.last().expect("the run always reports its end");
    assert_eq!(done["event"], "done", "{}", lines(&events));
    // The app shows this, and the export path has carried the same key since the frozen
    // corpus was recorded.
    assert_eq!(done["cancelled"], true, "{}", lines(&events));

    // The page already paid for is on the wire rather than swallowed: a Stop that
    // blanked the grid would be worse than no Stop button. Exactly one page, because a
    // stop can never hand on a page it never asked for.
    assert_eq!(
        done["rows"],
        200,
        "expected page one and nothing else: {}",
        lines(&events)
    );
    assert_eq!(
        data(&events).len(),
        200,
        "every row counted was sent: {}",
        lines(&events)
    );
    // The control below is what makes this one say something: the same SQL and the same
    // cap without a stop reaches 210 rows and sets `truncated`, so a stopped run
    // reporting 200 and no truncation is the stop's doing and not the cursor's.
    assert_eq!(
        done["truncated"],
        false,
        "the cap of 210 did not cut this result short, the stop did: {}",
        lines(&events)
    );
    // The key is present and null rather than absent: PostgreSQL has no per-statement id
    // to hand over (its identity is the backend pid, which is what a CancelRequest
    // needs), and an absent key and a null are not the same to the app's decoder.
    assert_eq!(done["query_id"], Json::Null, "{}", lines(&events));
}

#[tokio::test]
async fn the_same_preview_with_nobody_stopping_reaches_the_cap_on_a_real_server() {
    let Some(env) = settings() else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let cancel = CancelFlag::new();
    let mut out = Capture::new();
    let settings = Settings::from_pairs(pairs(&env));
    let engine = RealEngine::new();
    run(Command::Preview, &settings, &mut out, &engine, &cancel)
        .await
        .expect("a preview of a counted series");

    let events = out.lines;
    let done = events.last().expect("the run always reports its end");
    assert_eq!(done["event"], "done", "{}", lines(&events));
    // Ten rows past the 200-row page boundary, pulled for the verdict and then dropped:
    // the server really was asked for page two, and the response really carried the row
    // that answers "is there more".
    assert_eq!(done["rows"], 210, "{}", lines(&events));
    assert_eq!(done["truncated"], true, "{}", lines(&events));
    assert_eq!(data(&events).len(), 210, "{}", lines(&events));

    // Absent, not false: `cancelled` is added only when a stop actually arrived, so a
    // run nobody stopped keeps the shape `tests/golden/` recorded.
    assert!(
        done.get("cancelled").is_none(),
        "an unstopped run says nothing about cancelling: {}",
        lines(&events)
    );
}

#[tokio::test]
async fn a_live_preview_of_one_row_keeps_the_event_shape_the_corpus_froze() {
    let Some(env) = settings() else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let mut pairs = pairs(&env);
    pairs.retain(|(key, _)| key != "SQL" && key != "LIMIT");
    pairs.push(("SQL".to_owned(), "SELECT 1 AS one".to_owned()));

    let cancel = CancelFlag::new();
    let mut out = Capture::new();
    run(
        Command::Preview,
        &Settings::from_pairs(pairs),
        &mut out,
        &RealEngine::new(),
        &cancel,
    )
    .await
    .expect("a preview of one row");

    let done = out.lines.last().expect("the run always reports its end");
    assert_eq!(done["event"], "done", "{}", lines(&out.lines));
    assert!(
        done.get("cancelled").is_none(),
        "an unstopped run says nothing about cancelling: {}",
        lines(&out.lines)
    );
    assert_eq!(done["rows"], 1, "{}", lines(&out.lines));
}

#[tokio::test]
async fn a_count_that_times_out_names_the_bound_and_invents_no_number() {
    // `count` is the command most tempted to answer anyway: a grid footer wants a
    // number, and a slow count is exactly when one is wanted most. No driver here can
    // estimate the count of an arbitrary *statement* cheaply, so the honest answer is
    // the typed timeout and no `count` event — never a number this process did not get.
    let Some(env) = settings() else {
        eprintln!("{SKIP_HINT}");
        return;
    };

    let mut pairs: Vec<(String, String)> = env
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    pairs.retain(|(key, _)| key != "SQL" && key != "LIMIT");
    pairs.push(("SQL".to_owned(), "SELECT pg_sleep(5)".to_owned()));
    pairs.push(("STATEMENT_TIMEOUT_MS".to_owned(), "500".to_owned()));

    let cancel = CancelFlag::new();
    let mut out = Capture::new();
    let error = run(
        Command::Count,
        &Settings::from_pairs(pairs),
        &mut out,
        &RealEngine::new(),
        &cancel,
    )
    .await
    .expect_err("a count that timed out is not a count");
    assert!(
        error.message().contains("500 ms"),
        "the bound is named: {}",
        error.message()
    );
    assert!(
        out.lines.iter().all(|event| event["event"] != "count"),
        "a timeout must not emit a count: {}",
        lines(&out.lines)
    );
}

/// How many backends are still running a statement that carries `marker`.
///
/// Asked through the engine's own `preview` on a second connection, so this file needs no
/// driver of its own. The checker excludes itself by pid, since its text carries the marker.
async fn active_with(marker: &str) -> i64 {
    let mut pairs = pairs(&settings().expect("checked by the caller"));
    pairs.retain(|(key, _)| key != "SQL" && key != "LIMIT");
    pairs.push((
        "SQL".to_owned(),
        format!(
            "SELECT count(*) FROM pg_stat_activity \
             WHERE state = 'active' AND pid <> pg_backend_pid() AND query LIKE '%{marker}%'"
        ),
    ));
    let mut out = Capture::new();
    run(
        Command::Preview,
        &Settings::from_pairs(pairs),
        &mut out,
        &RealEngine::new(),
        &CancelFlag::new(),
    )
    .await
    .expect("the checker runs");
    data(&out.lines)[0][0]
        .as_str()
        .expect("a count is text")
        .parse()
        .expect("an integer")
}

/// Start `command` on `sql`, wait until the server is running it, press Stop, and return how
/// long the server took to stop running it.
async fn stop_and_time(command: Command, sql: &str) -> (std::time::Duration, Vec<Json>) {
    use std::time::{Duration, Instant};
    let marker = format!(
        "qh-cancel-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    );
    let mut pairs = pairs(&settings().expect("checked by the caller"));
    pairs.retain(|(key, _)| key != "SQL" && key != "LIMIT");
    pairs.push(("SQL".to_owned(), format!("{sql} /* {marker} */")));

    let cancel = CancelFlag::new();
    let stop = cancel.clone();
    // Joined rather than spawned: a run is not `Send`, and the two halves only need to
    // interleave.
    let running = async {
        let mut out = Capture::new();
        let result = run(
            command,
            &Settings::from_pairs(pairs),
            &mut out,
            &RealEngine::new(),
            &stop,
        )
        .await;
        (
            result.map_err(|error| error.message().to_owned()),
            out.lines,
        )
    };
    let watching = async {
        let waiting = Instant::now();
        while active_with(&marker).await == 0 {
            assert!(
                waiting.elapsed() < Duration::from_secs(10),
                "the server never started the statement"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let pressed = Instant::now();
        cancel.request();
        while active_with(&marker).await != 0 {
            assert!(
                pressed.elapsed() < Duration::from_secs(5),
                "the server is still running the statement"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        pressed.elapsed()
    };
    let ((result, events), stopped) = tokio::join!(running, watching);
    result.unwrap_or_else(|error| panic!("a stopped run is not a failed run: {error}"));
    (stopped, events)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_on_a_sleeping_preview_is_confirmed_by_the_server_within_250_ms() {
    let _turn = TURN.lock().await;
    if settings().is_none() {
        eprintln!("{SKIP_HINT}");
        return;
    }
    let (stopped, events) = stop_and_time(Command::Preview, "SELECT pg_sleep(30)").await;
    let done = events.last().expect("the run always reports its end");
    assert_eq!(done["event"], "done", "{}", lines(&events));
    assert_eq!(done["cancelled"], true, "{}", lines(&events));
    assert!(
        stopped < std::time::Duration::from_millis(250),
        "the server took {stopped:?} to stop"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_on_a_sleeping_count_is_confirmed_by_the_server_within_250_ms() {
    let _turn = TURN.lock().await;
    // `explain` has no such test: EXPLAIN without ANALYZE plans the statement and never runs
    // it, so there is nothing for a Stop to interrupt on PostgreSQL.
    if settings().is_none() {
        eprintln!("{SKIP_HINT}");
        return;
    }
    let (stopped, events) = stop_and_time(Command::Count, "SELECT pg_sleep(30)").await;
    let done = events.last().expect("the run always reports its end");
    assert_eq!(done["event"], "done", "{}", lines(&events));
    assert_eq!(done["cancelled"], true, "{}", lines(&events));
    assert!(
        stopped < std::time::Duration::from_millis(250),
        "the server took {stopped:?} to stop"
    );
}

// --------------------------------------------------------------------------- //
// a capped preview leaves nothing running on any of the three servers
// --------------------------------------------------------------------------- //

/// One server's dev container, and how to ask it what is still running.
struct Server {
    flag: &'static str,
    env: &'static [(&'static str, &'static str)],
    /// The statement to cap; `{m}` is replaced with a unique marker comment.
    slow_sql: &'static str,
    /// Counts the statements still running that carry the marker; `{m}` as above.
    running_sql: &'static str,
}

fn env_pairs(env: &[(&str, &str)], sql: &str, limit: Option<&str>) -> Settings {
    let mut pairs: Vec<(String, String)> = env
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    pairs.push(("RETRIES".to_owned(), "0".to_owned()));
    pairs.push(("SQL".to_owned(), sql.to_owned()));
    if let Some(limit) = limit {
        pairs.push(("LIMIT".to_owned(), limit.to_owned()));
    }
    Settings::from_pairs(pairs)
}

async fn running_on(server: &Server, marker: &str) -> i64 {
    let mut out = Capture::new();
    run(
        Command::Preview,
        &env_pairs(server.env, &server.running_sql.replace("{m}", marker), None),
        &mut out,
        &RealEngine::new(),
        &CancelFlag::new(),
    )
    .await
    .expect("the checker runs");
    data(&out.lines)[0][0]
        .as_str()
        .expect("a count is text")
        .parse()
        .expect("an integer")
}

/// Cap a long statement at ten rows and require the server to stop running it soon after the
/// run reports `done`: an unfinished cursor left to its connection task would keep the server
/// busy (and a core at 100 %) for the rest of the statement.
async fn a_capped_preview_leaves_nothing_running(server: Server) {
    use std::time::{Duration, Instant};
    if std::env::var(server.flag).as_deref() != Ok("1") {
        eprintln!(
            "skipped: set {}=1 with deploy/dev/up.sh running",
            server.flag
        );
        return;
    }
    let marker = format!(
        "qh-capped-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    );
    let sql = format!("{} /* {marker} */", server.slow_sql);
    let mut out = Capture::new();
    run(
        Command::Preview,
        &env_pairs(server.env, &sql, Some("10")),
        &mut out,
        &RealEngine::new(),
        &CancelFlag::new(),
    )
    .await
    .expect("a capped preview succeeds");
    let done = out.lines.last().expect("the run always reports its end");
    assert_eq!(done["truncated"], true, "{}", lines(&out.lines));
    assert_eq!(done["rows"], 10, "{}", lines(&out.lines));

    let waiting = Instant::now();
    while running_on(&server, &marker).await != 0 {
        assert!(
            waiting.elapsed() < Duration::from_secs(1),
            "the server is still running the capped statement"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capped_postgres_preview_leaves_nothing_running() {
    a_capped_preview_leaves_nothing_running(Server {
        flag: "QH_TEST_POSTGRES",
        env: &[
            ("DB_KIND", "postgres"),
            ("DB_HOST", "127.0.0.1"),
            ("DB_PORT", "55432"),
            ("DB_USER", "qh"),
            ("DB_PASSWORD", "qh-dev-only"),
            ("DB_DATABASE", "qh"),
            ("DB_SCHEMA", "public"),
            ("DB_SSLMODE", "disable"),
        ],
        slow_sql: "SELECT g, pg_sleep(0.002) FROM generate_series(1, 4000) AS g",
        running_sql: "SELECT count(*) FROM pg_stat_activity WHERE state = 'active' \
                      AND pid <> pg_backend_pid() AND query LIKE '%{m}%'",
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capped_mysql_preview_leaves_nothing_running() {
    a_capped_preview_leaves_nothing_running(Server {
        flag: "QH_TEST_MYSQL",
        env: &[
            ("DB_KIND", "mysql"),
            ("DB_HOST", "127.0.0.1"),
            ("DB_PORT", "53306"),
            ("DB_USER", "qh"),
            ("DB_PASSWORD", "qh-dev-only"),
            ("DB_DATABASE", "qh"),
        ],
        // A billion rows from a three-way cross join: more than a second of sending.
        slow_sql:
            "WITH RECURSIVE n AS (SELECT 1 AS x UNION ALL SELECT x + 1 FROM n WHERE x < 1000) \
                   SELECT a.x AS ax, b.x AS bx, c.x AS cx FROM n a, n b, n c",
        running_sql: "SELECT COUNT(*) FROM information_schema.processlist WHERE command = 'Query' \
                      AND id <> CONNECTION_ID() AND info LIKE '%{m}%'",
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_capped_trino_preview_deletes_its_query() {
    a_capped_preview_leaves_nothing_running(Server {
        flag: "QH_TEST_TRINO",
        env: &[
            ("DB_KIND", "trino"),
            ("DB_HOST", "127.0.0.1"),
            ("DB_PORT", "58080"),
            ("DB_USER", "queryhive"),
            ("DB_DATABASE", "tpch"),
            ("DB_SCHEMA", "sf1"),
        ],
        slow_sql: "SELECT * FROM lineitem",
        running_sql: "SELECT count(*) FROM system.runtime.queries WHERE state = 'RUNNING' \
                      AND query LIKE '%{m}%' AND query NOT LIKE '%system.runtime.queries%'",
    })
    .await;
}

// --------------------------------------------------------------------------- //
// a Stop in the middle of a MySQL result reaches the server
// --------------------------------------------------------------------------- //

const MYSQL_ENV: &[(&str, &str)] = &[
    ("DB_KIND", "mysql"),
    ("DB_HOST", "127.0.0.1"),
    ("DB_PORT", "53306"),
    ("DB_USER", "qh"),
    ("DB_PASSWORD", "qh-dev-only"),
    ("DB_DATABASE", "qh"),
];

/// A billion rows, streaming from the first second on.
const MYSQL_BIG: &str =
    "WITH RECURSIVE n AS (SELECT 1 AS x UNION ALL SELECT x + 1 FROM n WHERE x < 1000) \
                         SELECT a.x AS ax, b.x AS bx, c.x AS cx FROM n a, n b, n c";

async fn mysql_scalar(sql: &str) -> i64 {
    let mut out = Capture::new();
    run(
        Command::Preview,
        &env_pairs(MYSQL_ENV, sql, None),
        &mut out,
        &RealEngine::new(),
        &CancelFlag::new(),
    )
    .await
    .expect("the checker runs");
    data(&out.lines)[0]
        .as_array()
        .and_then(|row| row.last())
        .and_then(Json::as_str)
        .expect("text")
        .parse()
        .expect("an integer")
}

const COM_KILL: &str = "SHOW GLOBAL STATUS LIKE 'Com_kill'";

/// Run `command` over the big result, press Stop 1.5 s in, and require that the server saw a
/// `KILL` and stopped running the statement within a second of `done`.
async fn a_mid_stream_stop_kills_the_mysql_statement(command: Command, extra: &[(&str, String)]) {
    let _turn = TURN.lock().await;
    use std::time::{Duration, Instant};
    if std::env::var("QH_TEST_MYSQL").as_deref() != Ok("1") {
        eprintln!("skipped: set QH_TEST_MYSQL=1 with deploy/dev/up.sh running");
        return;
    }
    let marker = format!(
        "qh-midstream-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    );
    let sql = format!("{MYSQL_BIG} /* {marker} */");
    let mut pairs: Vec<(String, String)> = MYSQL_ENV
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    pairs.push(("RETRIES".to_owned(), "0".to_owned()));
    pairs.push(("SQL".to_owned(), sql));
    pairs.push(("LIMIT".to_owned(), "1000000000".to_owned()));
    pairs.extend(
        extra
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone())),
    );
    let settings = Settings::from_pairs(pairs);
    let kills_before = mysql_scalar(COM_KILL).await;

    let cancel = CancelFlag::new();
    let stop = cancel.clone();
    let mut out = Capture::new();
    let engine = RealEngine::new();
    let (result, ()) = tokio::join!(
        run(command, &settings, &mut out, &engine, &cancel),
        async move {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            stop.request();
        }
    );
    result.expect("a stopped run is not a failed run");
    let done = out.lines.last().expect("the run always reports its end");
    assert_eq!(done["event"], "done", "{}", lines(&out.lines));
    assert_eq!(done["cancelled"], true, "{}", lines(&out.lines));
    if command != Command::Count {
        assert!(done["query_id"].is_string(), "{}", lines(&out.lines));
    }
    assert!(
        done.get("warnings")
            .is_none_or(|w| w.as_array().is_some_and(|w| w.is_empty())),
        "the stop was confirmed: {}",
        lines(&out.lines)
    );
    assert!(
        mysql_scalar(COM_KILL).await > kills_before,
        "no KILL reached the server"
    );

    let running = "SELECT COUNT(*) FROM information_schema.processlist WHERE command = 'Query' \
                   AND id <> CONNECTION_ID() AND info LIKE '%{m}%'"
        .replace("{m}", &marker);
    let waiting = Instant::now();
    while mysql_scalar(&running).await != 0 {
        assert!(
            waiting.elapsed() < Duration::from_secs(1),
            "the server is still running the statement"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_in_the_middle_of_a_mysql_preview_kills_the_statement() {
    a_mid_stream_stop_kills_the_mysql_statement(Command::Preview, &[]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_in_the_middle_of_a_mysql_count_kills_the_statement() {
    a_mid_stream_stop_kills_the_mysql_statement(Command::Count, &[]).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_in_the_middle_of_a_mysql_export_kills_the_statement() {
    let dir = tempfile::tempdir().expect("temp dir");
    a_mid_stream_stop_kills_the_mysql_statement(
        Command::Export,
        &[
            ("OUT_DIR", dir.path().to_string_lossy().into_owned()),
            ("FORMAT", "csv".to_owned()),
            ("NAME", "big".to_owned()),
        ],
    )
    .await;
}

// --------------------------------------------------------------------------- //
// the MySQL Safe Mode bypass W3-T0 found: a read-only connection must not run a
// write hidden in a string escape or a comment
// --------------------------------------------------------------------------- //

/// Run one statement against the dev MySQL under `SAFE_MODE=full`, for the scratch-table
/// setup and teardown. Returns the run's result so a caller can assert it ran.
async fn mysql_run(sql: &str) -> Result<(), qh_ffi::CliError> {
    let mut out = Capture::new();
    run(
        Command::Preview,
        &env_pairs(MYSQL_ENV, sql, Some("1")),
        &mut out,
        &RealEngine::new(),
        &CancelFlag::new(),
    )
    .await
}

/// The count of rows the scratch table holds now.
async fn mysql_count(table: &str) -> i64 {
    mysql_scalar(&format!("SELECT COUNT(*) FROM {table}")).await
}

/// A read-only MySQL connection asked to run a string-escape or comment injection must not
/// delete the scratch row: the write is refused (in the engine's guard or, failing that, in
/// the driver before it reaches the always-multi-statement text protocol) and the row
/// survives.
///
/// On the code before W3-T0 this fails: the payload classifies as one read-only SELECT, the
/// server runs the DELETE, and the row is gone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_mysql_connection_does_not_run_an_injected_delete() {
    let _turn = TURN.lock().await;
    if std::env::var("QH_TEST_MYSQL").as_deref() != Ok("1") {
        eprintln!("skipped: set QH_TEST_MYSQL=1 with deploy/dev/up.sh running");
        return;
    }

    // A table nobody else touches, so a stray DELETE here is unambiguous.
    let table = format!(
        "qh_w3t0_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    );
    mysql_run(&format!("DROP TABLE IF EXISTS {table}"))
        .await
        .expect("drop any leftover");
    mysql_run(&format!("CREATE TABLE {table} (id INT PRIMARY KEY)"))
        .await
        .expect("create scratch table");
    mysql_run(&format!("INSERT INTO {table} VALUES (1)"))
        .await
        .expect("seed one row");
    assert_eq!(
        mysql_count(&table).await,
        1,
        "the seed row is there to start"
    );

    // Both the `-- ` and the `#` comment variants, each hiding a DELETE of the scratch
    // table behind a backslash-escaped quote, plus the no-doubling spellings where the
    // server's `sql_mode` closes the string earlier than the MySQL reading thinks.
    let payloads = [
        format!("SELECT '\\''; DELETE FROM {table}; -- '"),
        format!("SELECT '\\''; DELETE FROM {table}; # '"),
        format!("SELECT '\\'; DELETE FROM {table}; -- '"),
        format!("SELECT 1 \"\\\" ; DELETE FROM {table} ; -- \""),
        // MySQL has no dollar quoting: the `;` between two markers separates statements.
        // The form the server really runs (verified on 8.4): the first statement is valid
        // because `x$a$` is one identifier, and the DELETE follows.
        format!("SELECT 1 AS x$a$ ; DELETE FROM {table} ; $a$"),
        format!("SELECT 1 $a$ ; DELETE FROM {table} ; $a$"),
        format!("SELECT $$; DELETE FROM {table}; $$"),
    ];
    for payload in payloads {
        let mut pairs: Vec<(String, String)> = MYSQL_ENV
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        pairs.push(("RETRIES".to_owned(), "0".to_owned()));
        pairs.push(("SAFE_MODE".to_owned(), "read_only".to_owned()));
        pairs.push(("SQL".to_owned(), payload.clone()));
        let mut out = Capture::new();
        let result = run(
            Command::Preview,
            &Settings::from_pairs(pairs),
            &mut out,
            &RealEngine::new(),
            &CancelFlag::new(),
        )
        .await;
        assert!(
            result.is_err(),
            "a read-only connection must refuse {payload:?}, got {:?}",
            out.lines
        );
        assert_eq!(
            mysql_count(&table).await,
            1,
            "the injected DELETE must not have run for {payload:?}"
        );
    }

    mysql_run(&format!("DROP TABLE {table}"))
        .await
        .expect("clean up the scratch table");
}

/// With Safe Mode `full` the engine's guard refuses nothing, so the driver's own backstop
/// is the last line: a text that any lexer the server might use reads as two statements
/// never reaches the always-multi-statement text protocol. The dollar-tag text is the case
/// the old readings called one statement while the server ran two.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mysql_driver_refuses_a_dollar_tag_second_statement_even_in_full_mode() {
    let _turn = TURN.lock().await;
    if std::env::var("QH_TEST_MYSQL").as_deref() != Ok("1") {
        eprintln!("skipped: set QH_TEST_MYSQL=1 with deploy/dev/up.sh running");
        return;
    }

    let table = format!(
        "qh_w3t0_full_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    );
    mysql_run(&format!("DROP TABLE IF EXISTS {table}"))
        .await
        .expect("drop any leftover");
    mysql_run(&format!("CREATE TABLE {table} (id INT PRIMARY KEY)"))
        .await
        .expect("create scratch table");
    mysql_run(&format!("INSERT INTO {table} VALUES (1)"))
        .await
        .expect("seed one row");

    for payload in [
        format!("SELECT 1 AS x$a$ ; DELETE FROM {table} ; $a$"),
        format!("SELECT 1 $a$ ; DELETE FROM {table} ; $a$"),
        format!("SELECT $$; DELETE FROM {table}; $$"),
        format!("SELECT 1 $tag$; DELETE FROM {table}; $tag$"),
    ] {
        let result = mysql_run(&payload).await;
        assert!(
            result.is_err(),
            "the driver must refuse the two-statement text {payload:?}"
        );
        assert_eq!(
            mysql_count(&table).await,
            1,
            "the second statement must not have run for {payload:?}"
        );
    }

    mysql_run(&format!("DROP TABLE {table}"))
        .await
        .expect("clean up the scratch table");
}

/// A read-only MySQL connection still runs an ordinary read whose comment mentions a write
/// keyword: `#` is a comment under every sql_mode, so there is nothing to refuse.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_only_mysql_connection_runs_a_read_with_a_hash_comment() {
    let _turn = TURN.lock().await;
    if std::env::var("QH_TEST_MYSQL").as_deref() != Ok("1") {
        eprintln!("skipped: set QH_TEST_MYSQL=1 with deploy/dev/up.sh running");
        return;
    }
    for sql in [
        "SELECT 41 + 1 AS answer # remember to update this later",
        "SELECT 42 AS answer -- delete me\n# drop table t",
        "SELECT 42 AS answer /*M! DELETE FROM nothing */",
    ] {
        let mut pairs: Vec<(String, String)> = MYSQL_ENV
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        pairs.push(("RETRIES".to_owned(), "0".to_owned()));
        pairs.push(("SAFE_MODE".to_owned(), "read_only".to_owned()));
        pairs.push(("SQL".to_owned(), sql.to_owned()));
        let mut out = Capture::new();
        run(
            Command::Preview,
            &Settings::from_pairs(pairs),
            &mut out,
            &RealEngine::new(),
            &CancelFlag::new(),
        )
        .await
        .unwrap_or_else(|error| panic!("a read-only connection must run {sql:?}: {error:?}"));
        let row = &data(&out.lines)[0];
        assert_eq!(
            row.as_array()
                .and_then(|cells| cells.last())
                .and_then(Json::as_str),
            Some("42"),
            "{sql:?}"
        );
    }
}
