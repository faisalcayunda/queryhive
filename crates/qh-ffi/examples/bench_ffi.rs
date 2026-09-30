//! In-process FFI bench: calls `EngineHost::run`, the entry the app uses, with a sink that
//! counts and timestamps events (performance-plan.md section 4, item 0.2).
//!
//! ```bash
//! cargo run --release -p qh-ffi --example bench_ffi -- <scenario> [--repeat N]
//! ```
//!
//! Scenarios: `local-loop`, `emit-only`, `preview-wide`, or `all` (the default). `--repeat`
//! defaults to 5. `preview-wide` needs the dev PostgreSQL (`deploy/dev/up.sh`) on
//! 127.0.0.1:55432 with `wide_500k` seeded (`BENCH_FFI_PG_PORT` overrides the port).
//!
//! # Output
//!
//! JSON lines on stdout, one sample per scenario per repetition, in the input contract of
//! `deploy/dev/bench_app.py`: `scenario`, `axis`, `repeat` and metrics printed flat, each
//! named with a unit suffix. Anything else goes to stderr. Pipe it in with
//! `... -- all --repeat 10 | python3 deploy/dev/bench_app.py --source ffi --repeat 10`
//! (`--axis` is not needed: each sample carries its own).
//!
//! - `local-loop` (scenario `ffi-local-loop`): `call_p50_ms`, `call_p95_ms` over 100
//!   `connections` calls through one host, so the local database is opened once and the
//!   runtime is the process's own.
//! - `emit-only` (scenario `ffi-emit-only`): `page_ms`, the mean time to build, JSON-encode and
//!   hand one page (200 rows x 30 text columns) to the sink.
//! - `preview-wide` (scenario `ffi-preview-wide`, no axis: an in-process engine number, not the grid number axis 2 grades): `ttfr_ms` (run start to first `rows`
//!   event), `total_ms` (run start to return, connect included),
//!   `rows_per_s` (rows / first `rows` event to `done`, so it excludes connect),
//!   `sink_ms` (time inside `on_event`), `sink_share_ratio` (`sink_ms / total_ms`). The sink
//!   only counts, so the share is a floor for a real sink.
//!
//! A run that reports an `error` event, or a `preview-wide` run that returns no rows, prints
//! the reason on stderr and exits 1 without printing any sample.

use qh_ffi::host::EngineHost;
use qh_ffi::uniffi_api::{EngineCommand, EventSink, RunCancel, Setting};
use serde_json::{json, Value as Json};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// One host for the whole process, as the app has: the pool and the local database handle
/// persist between runs, so what is measured is the path a Run takes after the first.
fn host() -> &'static Arc<EngineHost> {
    static HOST: OnceLock<Arc<EngineHost>> = OnceLock::new();
    HOST.get_or_init(EngineHost::new)
}

/// Counts events, timestamps the first `rows` and the `done` event, keeps the first `error`,
/// and times its own body.
struct Probe {
    start: Instant,
    sink_ns: AtomicU64,
    first_rows: OnceLock<Instant>,
    done: OnceLock<(Instant, u64)>,
    error: OnceLock<String>,
}

impl Probe {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            start: Instant::now(),
            sink_ns: AtomicU64::new(0),
            first_rows: OnceLock::new(),
            done: OnceLock::new(),
            error: OnceLock::new(),
        })
    }

    /// Exit 1, with no sample printed, when the run failed.
    fn require_ok(&self, what: &str) {
        if let Some(message) = self.error.get() {
            eprintln!("{what}: the run reported an error: {message}");
            std::process::exit(1);
        }
    }
}

impl EventSink for Probe {
    fn on_event(&self, line: String) {
        let entered = Instant::now();
        // Compact JSON, so the pair is byte-exact whatever the key order, and a cell holding
        // this text would carry escaped quotes. A rows event is ~100 KB: not worth a parse.
        if line.contains(r#""event":"rows""#) {
            let _ = self.first_rows.set(entered);
        } else if let Ok(event) = serde_json::from_str::<Json>(&line) {
            match event["event"].as_str() {
                Some("done") => {
                    let _ = self
                        .done
                        .set((entered, event["rows"].as_u64().unwrap_or(0)));
                }
                Some("error") => {
                    let _ = self.error.set(event["message"].to_string());
                }
                _ => {}
            }
        }
        self.sink_ns
            .fetch_add(entered.elapsed().as_nanos() as u64, Relaxed);
    }
}

fn setting(key: &str, value: &str) -> Setting {
    Setting {
        key: key.to_owned(),
        value: value.to_owned(),
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn emit(scenario: &str, axis: Json, repeat: u32, metrics: Json) {
    let mut obj =
        json!({"source": "bench_ffi", "scenario": scenario, "axis": axis, "repeat": repeat});
    if let (Some(o), Json::Object(m)) = (obj.as_object_mut(), metrics) {
        o.extend(m);
    }
    println!("{obj}");
}

fn local_loop(repeat: u32) {
    // An isolated DB_PATH, so the app's own database is never opened.
    let dir = tempfile::tempdir().expect("temp dir");
    let db = dir.path().join("bench.db");
    let legacy = dir.path().join("legacy");
    let call = || {
        let probe = Probe::new();
        let settings = vec![
            setting("DB_PATH", &db.to_string_lossy()),
            setting("LEGACY_PATH", &legacy.to_string_lossy()),
        ];
        let t = Instant::now();
        host().run(
            EngineCommand::Connections,
            settings,
            probe.clone(),
            RunCancel::new(),
        );
        let elapsed = ms(t.elapsed());
        probe.require_ok("local-loop");
        elapsed
    };
    call(); // migrates once
    let mut times: Vec<f64> = (0..100).map(|_| call()).collect();
    times.sort_by(f64::total_cmp);
    emit(
        "ffi-local-loop",
        Json::Null,
        repeat,
        json!({"call_p50_ms": times[49], "call_p95_ms": times[94]}),
    );
}

fn emit_only(repeat: u32) {
    const PAGES: u64 = 500;
    let sink = Probe::new();
    let t = Instant::now();
    for p in 0..PAGES {
        // The same work `SinkEmitter::emit` does: build the event value, serialise it compactly.
        let rows: Vec<Json> = (0..200)
            .map(|r| {
                Json::Array(
                    (0..30)
                        .map(|c| Json::from(format!("p{p}-r{r}-c{c}-lorem ipsum")))
                        .collect(),
                )
            })
            .collect();
        sink.on_event(serde_json::to_string(&json!({"event": "rows", "data": rows})).unwrap());
    }
    emit(
        "ffi-emit-only",
        Json::Null,
        repeat,
        json!({"page_ms": ms(t.elapsed()) / PAGES as f64}),
    );
}

fn preview_wide(repeat: u32) {
    let probe = Probe::new();
    let port = std::env::var("BENCH_FFI_PG_PORT").unwrap_or_else(|_| "55432".into());
    let settings = vec![
        setting("DB_KIND", "postgres"),
        setting("DB_HOST", "127.0.0.1"),
        setting("DB_PORT", &port),
        setting("DB_USER", "qh"),
        setting("DB_PASSWORD", "qh-dev-only"),
        setting("DB_DATABASE", "qh"),
        setting("DB_SCHEMA", "public"),
        setting("DB_SSLMODE", "disable"),
        setting("RETRIES", "0"),
        setting("SQL", "SELECT * FROM wide_500k"),
        setting("LIMIT", "10000000"),
    ];
    host().run(
        EngineCommand::Preview,
        settings,
        probe.clone(),
        RunCancel::new(),
    );
    let total = probe.start.elapsed();
    probe.require_ok("preview-wide");
    let (Some(first), Some((done_at, rows))) = (probe.first_rows.get(), probe.done.get()) else {
        eprintln!("preview-wide: no rows event or no done event, nothing to report");
        std::process::exit(1);
    };
    if *rows == 0 {
        eprintln!("preview-wide: done reported 0 rows, nothing to report");
        std::process::exit(1);
    }
    let sink_ms = probe.sink_ns.load(Relaxed) as f64 / 1e6;
    emit(
        "ffi-preview-wide",
        json!(null),
        repeat,
        json!({
            "ttfr_ms": ms(*first - probe.start), "total_ms": ms(total),
            "rows_per_s": *rows as f64 / (*done_at - *first).as_secs_f64(),
            "sink_ms": sink_ms, "sink_share_ratio": sink_ms / ms(total),
        }),
    );
}

type Scenario = (&'static str, fn(u32));

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let repeat: u32 = args
        .iter()
        .position(|a| a == "--repeat")
        .and_then(|i| args.get(i + 1))
        .map_or(5, |n| n.parse().expect("--repeat takes a number"));
    let scenario = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .map_or("all", |s| s.as_str());
    let scenarios: &[Scenario] = &[
        ("local-loop", local_loop),
        ("emit-only", emit_only),
        ("preview-wide", preview_wide),
    ];
    assert!(
        scenario == "all" || scenarios.iter().any(|(n, _)| *n == scenario),
        "unknown scenario {scenario:?}; one of local-loop, emit-only, preview-wide, all"
    );
    for (name, f) in scenarios {
        if scenario == "all" || scenario == *name {
            (0..repeat).for_each(f);
        }
    }
}
