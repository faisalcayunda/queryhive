//! The numbers of blueprint fase-4b §1.3, §1.4 and §11, measured on `qh-editor` itself.
//!
//!     cargo run --release -p qh-editor --example editor_bench
//!
//! Reads the corpora `deploy/dev/make_sql_corpus.py` and the bench document write (default
//! `target/run/ts-bench/corpus`, or `QH_CORPUS_DIR`; missing files are skipped). Prints a
//! Markdown table. Memory is the process's resident set from `ps`, so it includes what the
//! allocator holds back; `stats` gives the sizes the analyzer can count itself.

use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use qh_editor::{Dialect, Document};

fn corpus_dir() -> PathBuf {
    std::env::var_os("QH_CORPUS_DIR").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/run/ts-bench/corpus"),
        PathBuf::from,
    )
}

fn rss_mb() -> f64 {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse::<f64>()
        .unwrap_or(0.0)
        / 1024.0
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// p50 and p99 of `samples`, in milliseconds.
fn percentiles(mut samples: Vec<Duration>) -> (f64, f64) {
    samples.sort();
    let at = |p: f64| ms(samples[((samples.len() as f64 * p) as usize).min(samples.len() - 1)]);
    (at(0.5), at(0.99))
}

/// The UTF-16 offset of the start of the line in the middle of the text, or the middle itself
/// when the next line break is far off (the one-line dump).
fn middle_line(text: &str) -> u32 {
    let mut mid = text.len() / 2;
    while !text.is_char_boundary(mid) {
        mid += 1;
    }
    let start = match text[mid..].find('\n') {
        Some(at) if at < 10_000 => mid + at + 1,
        _ => mid,
    };
    text[..start].encode_utf16().count() as u32
}

fn main() {
    let dir = corpus_dir();
    let names = ["lines-10k", "bench-10k", "chars-2m", "bench-2m", "dump-2m"];
    println!(
        "| corpus | bytes | statements | open | first window | paint all | typing replace p50/p99 | typing paint p50/p99 | outline | RSS after paint all | trees / tree src | tokens |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|");
    for name in names {
        let Ok(text) = std::fs::read_to_string(dir.join(format!("{name}.sql"))) else {
            eprintln!("{name}.sql is not in {}", dir.display());
            continue;
        };
        // The bench documents are held to the ceiling, as the app's `type-2m` is.
        let end = (0..=text.len().min(2_000_000))
            .rev()
            .find(|&at| text.is_char_boundary(at))
            .unwrap();
        let text = &text[..end];
        let dialect = Dialect::Generic;

        let before = rss_mb();
        let started = Instant::now();
        let mut doc = Document::new(text, dialect).unwrap();
        let open = started.elapsed();
        let statements = doc.stats().statements;

        // The first frame: the window around the caret, nothing else.
        let caret = middle_line(text);
        let window_start = caret.saturating_sub(24_000);
        let started = Instant::now();
        let paint = doc.paint(1, window_start, 48_000, 131_072).unwrap();
        let first = started.elapsed();
        assert!(!paint.runs.is_empty());
        doc.mark_applied(1, paint.ranges);

        // Everything, on a fresh document, to compare with the blueprint's "parse all".
        let mut whole = Document::new(text, dialect).unwrap();
        let started = Instant::now();
        let paint = whole.paint(1, 0, u32::MAX, u32::MAX).unwrap();
        let all = started.elapsed();
        let runs = paint.runs.len() / 3;
        whole.mark_applied(1, paint.ranges);
        let after = rss_mb();
        let stats = whole.stats();
        let started = Instant::now();
        let outline = whole.outline(1).unwrap();
        let outline_time = started.elapsed();
        drop(whole);

        // 200 keystrokes forward from the start of the middle line: the edit on the main side,
        // then the analysis side's paint of the window around the caret.
        let (mut replace, mut paints) = (Vec::new(), Vec::new());
        let mut revision = 1;
        let mut at = caret;
        for i in 0..200 {
            let ch = ["x", "y", " ", "1", "e"][i % 5];
            let started = Instant::now();
            revision = doc.replace(at, 0, ch).unwrap();
            replace.push(started.elapsed());
            at += 1;
            let started = Instant::now();
            let paint = doc
                .paint(revision, at.saturating_sub(24_000), 48_000, 131_072)
                .unwrap();
            paints.push(started.elapsed());
            doc.mark_applied(revision, paint.ranges);
        }
        let (r50, r99) = percentiles(replace);
        let (p50, p99) = percentiles(paints);
        println!(
            "| {name} | {} | {statements} | {:.1} ms | {:.1} ms | {:.0} ms ({runs} runs) | {r50:.4} / {r99:.4} ms | {p50:.3} / {p99:.3} ms | {:.0} ms ({} folds) | {:.0} MB (+{:.0}) | {} / {} KiB | {} |",
            text.len(),
            ms(open),
            ms(first),
            ms(all),
            ms(outline_time),
            outline.folds.len(),
            after,
            after - before,
            stats.trees,
            stats.tree_source_bytes / 1024,
            stats.tokens,
        );
        let _ = revision;
    }
}
