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
//! `stream_rows` pulls the page that carries `columns` before any event is emitted, and
//! neither `preview` nor `explain` calls `session.cancel()`. A Stop therefore has a
//! granularity of one page: it stops the *next* fetch, and it cannot cut short a fetch
//! already in flight. The measurement that establishes this: with
//! `pg_sleep(0.4) IS NULL` per row over 400 rows, a stop raised 400 ms into the run did
//! not return until 160.6 s, which is the whole statement -- the priming page and the
//! five after it, at 400 rows of 0.4 s. The query below keeps the row count over the
//! page size and no sleep at all, because the assertion is about *which* pages are
//! fetched, not about how long they took.

use std::io;

use qh_ffi::events::{Capture, Emitter};
use qh_ffi::{run, CancelFlag, Command, RealEngine, Settings};
use serde_json::Value as Json;

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
