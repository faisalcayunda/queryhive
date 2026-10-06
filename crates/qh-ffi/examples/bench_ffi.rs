//! In-process FFI bench: calls `EngineHost::run`, the entry the app uses, with a sink that
//! counts and timestamps events (performance-plan.md section 4, item 0.2).
//!
//! ```bash
//! cargo run --release -p qh-ffi --example bench_ffi -- <scenario> [--repeat N] [--rows N] [--cols M]
//! ```
//!
//! Scenarios: `local-loop`, `emit-only`, `preview-wide`, `window`, `window-json`, `view-sort`,
//! `view-sort-text`, `view-filter`, `view-search`, or `all` (the default). `--repeat`
//! defaults to 5. `--rows` and `--cols` size the `window` scenario's window (default 128 x 30;
//! `--rows 64 --cols 32` is the page the grid asks for), and a store narrower than `--cols` is
//! built that many columns wide. `preview-wide` needs the dev PostgreSQL (`deploy/dev/up.sh`) on
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
//! - `window` (scenario `ffi-window`, or `ffi-window-<rows>x<cols>` for another size):
//!   `window_p50_ms`, `window_p95_ms`, `window_p99_ms`, `window_max_ms` over 5,000 random windows
//!   (128 x 30 unless `--rows` and `--cols` say otherwise) of a 1M-row synthetic store (30 bigint,
//!   double and text columns, all resident), through `ResultHandle::window`. This is the Rust
//!   side including the buffer copy; the UniFFI crossing is `StoreWindowBench` on the Swift side.
//!   The target is p99 <= 0.5 ms (NFR-P8); a miss goes to W8-T2, it does not fail the run.
//! - `window-json` (scenario `ffi-window-json`): the same numbers for 128 x 8 windows whose
//!   columns are JSON text shown with `CellFormat::Json`. Recorded, never gated: the printer
//!   re-parses every cell (blueprint section 11.5).
//! - `view-sort`, `view-sort-text`, `view-filter`, `view-search` (scenarios `ffi-view-*`):
//!   `view_ms`, the time `set_view` blocks on the 1M x 30 store, with the view pool's thread
//!   count as `view_threads`. Sort by a bigint, sort by a text column (natural key), a `Text`
//!   filter, and a search over every column.
//!
//! A run that reports an `error` event, or a `preview-wide` run that returns no rows, prints
//! the reason on stderr and exits 1 without printing any sample.

use qh_ffi::host::EngineHost;
use qh_ffi::store_api::{
    CellFormat, ColumnWire, FilterSpec, ResultHandle, SortSpec, StoreFfiError, ViewSpec,
    MAX_WINDOW_CELLS, MAX_WINDOW_COLUMNS, MAX_WINDOW_ROWS,
};
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

// ---- the data plane -------------------------------------------------------------------------

const BENCH_ROWS: u32 = 1_000_000;
const BENCH_COLUMNS: u32 = 30;
/// The `window` scenario's size when `--rows` and `--cols` are absent: rows, then columns.
const DEFAULT_WINDOW: (u32, u32) = (128, BENCH_COLUMNS);

/// The window `--rows` and `--cols` asked for, set once by `main`.
static WINDOW: OnceLock<(u32, u32)> = OnceLock::new();

fn window_size() -> (u32, u32) {
    *WINDOW.get().unwrap_or(&DEFAULT_WINDOW)
}

/// The registry, configured once: no spill and a budget the whole bench store fits in, so what
/// is measured is the resident path, which is the one the 0.5 ms target is about.
fn stores() -> &'static Arc<EngineHost> {
    static CONFIGURED: OnceLock<()> = OnceLock::new();
    CONFIGURED.get_or_init(|| {
        host()
            .configure_result_stores(None, 3 * 1024 * 1024 * 1024)
            .unwrap_or_else(|error| die(&format!("configure_result_stores: {error}")));
    });
    host()
}

fn die(message: &str) -> ! {
    eprintln!("bench_ffi: {message}");
    std::process::exit(1);
}

fn ok<T>(what: &str, result: Result<T, StoreFfiError>) -> T {
    result.unwrap_or_else(|error| die(&format!("{what}: {error}")))
}

/// 1M synthetic rows, 30 columns wide or as wide as the `window` scenario needs, built once and
/// shared by every data-plane scenario.
fn big_store() -> &'static Arc<ResultHandle> {
    static STORE: OnceLock<Arc<ResultHandle>> = OnceLock::new();
    STORE.get_or_init(|| {
        let started = Instant::now();
        let columns = BENCH_COLUMNS.max(window_size().1);
        let store = ok(
            "store_synthetic",
            stores().store_synthetic(BENCH_ROWS, columns, 42),
        );
        eprintln!(
            "built a {BENCH_ROWS} x {columns} store in {:.1} s",
            started.elapsed().as_secs_f64()
        );
        store
    })
}

/// 20,000 rows of 8 JSON-text columns, for `window-json`.
fn json_store() -> &'static Arc<ResultHandle> {
    static STORE: OnceLock<Arc<ResultHandle>> = OnceLock::new();
    STORE.get_or_init(|| {
        let columns = (0..8)
            .map(|k| ColumnWire {
                name: format!("j{k}"),
                type_name: "json".to_owned(),
            })
            .collect();
        let rows = (0..20_000u32)
            .map(|r| {
                (0..8u32)
                    .map(|k| {
                        Some(format!(
                            r#"{{"id":{r},"col":{k},"name":"item {r}-{k}","tags":["a","b","c"],"nested":{{"x":{},"y":[1,2,3,4,5],"z":"lorem ipsum dolor sit amet"}}}}"#,
                            r * 7 + k
                        ))
                    })
                    .collect()
            })
            .collect();
        ok("store_from_rows", stores().store_from_rows(columns, rows))
    })
}

/// A small deterministic generator, so a run is repeatable.
fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn percentile(sorted_ms: &[f64], p: f64) -> f64 {
    sorted_ms[(((sorted_ms.len() - 1) as f64) * p).round() as usize]
}

/// Time `calls` random windows and report the percentiles.
fn window_case(
    scenario: &str,
    store: &ResultHandle,
    rows: u32,
    window_rows: u32,
    columns: u32,
    format: CellFormat,
    repeat: u32,
) {
    const CALLS: usize = 5_000;
    const WARMUP: usize = 200;
    let cols: Vec<u32> = (0..columns).collect();
    let formats = vec![format; columns as usize];
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut bytes = 0usize;
    let mut times: Vec<f64> = Vec::with_capacity(CALLS);
    for call in 0..WARMUP + CALLS {
        let first = (xorshift(&mut state) % u64::from(rows - window_rows)) as u32;
        let started = Instant::now();
        let buffer = ok(
            "window",
            store.window(0, first, window_rows, cols.clone(), formats.clone()),
        );
        let took = ms(started.elapsed());
        bytes = buffer.len();
        if call >= WARMUP {
            times.push(took);
        }
    }
    times.sort_by(f64::total_cmp);
    emit(
        scenario,
        json!("window"),
        repeat,
        json!({
            "window_p50_ms": percentile(&times, 0.50),
            "window_p95_ms": percentile(&times, 0.95),
            "window_p99_ms": percentile(&times, 0.99),
            "window_max_ms": times[times.len() - 1],
            "window_rows": window_rows, "window_columns": columns, "window_bytes": bytes,
        }),
    );
}

fn window(repeat: u32) {
    let (rows, columns) = window_size();
    // Another size is another series: it must not land in the 128 x 30 baseline's name.
    let scenario = if (rows, columns) == DEFAULT_WINDOW {
        "ffi-window".to_owned()
    } else {
        format!("ffi-window-{rows}x{columns}")
    };
    window_case(
        &scenario,
        big_store(),
        BENCH_ROWS,
        rows,
        columns,
        CellFormat::Raw,
        repeat,
    );
}

fn window_json(repeat: u32) {
    window_case(
        "ffi-window-json",
        json_store(),
        20_000,
        128,
        8,
        CellFormat::Json,
        repeat,
    );
}

fn view_case(scenario: &str, spec: impl Fn() -> ViewSpec, repeat: u32) {
    let store = big_store();
    let started = Instant::now();
    let info = ok("set_view", store.set_view(spec()));
    let took = ms(started.elapsed());
    emit(
        scenario,
        json!(null),
        repeat,
        json!({
            "view_ms": took, "view_visible": info.visible, "view_rows": info.fetched,
            "view_threads": qh_rt::view_pool().current_num_threads(),
        }),
    );
    // Back to the identity view, so the next case starts from the same place and the window
    // scenarios, which ask for view 0, still can.
    ok(
        "set_view",
        store.set_view(ViewSpec {
            sort: None,
            filters: vec![],
            search: None,
        }),
    );
}

fn view_sort(repeat: u32) {
    view_case(
        "ffi-view-sort",
        || ViewSpec {
            sort: Some(SortSpec {
                column: 0,
                descending: false,
            }),
            filters: vec![],
            search: None,
        },
        repeat,
    );
}

fn view_sort_text(repeat: u32) {
    view_case(
        "ffi-view-sort-text",
        || ViewSpec {
            sort: Some(SortSpec {
                column: 2,
                descending: true,
            }),
            filters: vec![],
            search: None,
        },
        repeat,
    );
}

fn view_filter(repeat: u32) {
    view_case(
        "ffi-view-filter",
        || ViewSpec {
            sort: None,
            filters: vec![FilterSpec::Text {
                column: 2,
                needle: "r99".to_owned(),
            }],
            search: None,
        },
        repeat,
    );
}

fn view_search(repeat: u32) {
    view_case(
        "ffi-view-search",
        || ViewSpec {
            sort: None,
            filters: vec![],
            search: Some("r123456-".to_owned()),
        },
        repeat,
    );
}

type Scenario = (&'static str, fn(u32));

/// The number after `name` on the command line, if the flag is there.
fn flag(args: &[String], name: &str) -> Option<u32> {
    let value = args.get(args.iter().position(|a| a == name)? + 1)?;
    Some(
        value
            .parse()
            .unwrap_or_else(|_| die(&format!("{name} takes a number, not {value:?}"))),
    )
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let repeat = flag(&args, "--repeat").unwrap_or(5);
    let (rows, columns) = (
        flag(&args, "--rows").unwrap_or(DEFAULT_WINDOW.0),
        flag(&args, "--cols").unwrap_or(DEFAULT_WINDOW.1),
    );
    // The limits `ResultHandle::window` enforces, checked here so a bad size fails before the
    // store is built rather than after it.
    if !(1..=MAX_WINDOW_ROWS).contains(&rows)
        || !(1..=MAX_WINDOW_COLUMNS as u32).contains(&columns)
        || u64::from(rows) * u64::from(columns) > MAX_WINDOW_CELLS
    {
        die(&format!(
            "a {rows} x {columns} window is outside 1..={MAX_WINDOW_ROWS} rows, \
             1..={MAX_WINDOW_COLUMNS} columns and {MAX_WINDOW_CELLS} cells"
        ));
    }
    WINDOW
        .set((rows, columns))
        .expect("main sets the window once");
    let scenario = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .map_or("all", |s| s.as_str());
    let scenarios: &[Scenario] = &[
        ("local-loop", local_loop),
        ("emit-only", emit_only),
        ("preview-wide", preview_wide),
        ("window", window),
        ("window-json", window_json),
        ("view-sort", view_sort),
        ("view-sort-text", view_sort_text),
        ("view-filter", view_filter),
        ("view-search", view_search),
    ];
    assert!(
        scenario == "all" || scenarios.iter().any(|(n, _)| *n == scenario),
        "unknown scenario {scenario:?}; one of local-loop, emit-only, preview-wide, window, window-json, view-sort, view-sort-text, view-filter, view-search, all"
    );
    for (name, f) in scenarios {
        if scenario == "all" || scenario == *name {
            (0..repeat).for_each(f);
        }
    }
}
