//! G6: typing inside a `$body$ … $body$` must not leak the scanner's dollar-quote tag.
//!
//! Upstream's 0.3.11 `deserialize` overwrote `start_tag` without freeing it, about 74 bytes
//! per re-parse; `vendor/scanner.patch` (PR #361) fixes it. The scanner allocates with libc
//! `malloc`, not `ts_malloc`, so `tree_sitter::set_allocator` cannot see it; the check reads
//! the malloc zone's live byte count instead, which is why this file is macOS only and
//! holds the only `unsafe` in the crate's tests.
//!
//! One test only: the zone counter is process-wide, so a second test running on another
//! thread would move it.

#![cfg(target_os = "macos")]
#![allow(unsafe_code)]

use std::ffi::c_void;

use tree_sitter::{InputEdit, Parser, Point};

#[repr(C)]
#[derive(Default)]
struct MallocStatistics {
    blocks_in_use: u32,
    size_in_use: usize,
    max_size_in_use: usize,
    size_allocated: usize,
}

extern "C" {
    fn malloc_zone_statistics(zone: *mut c_void, stats: *mut MallocStatistics);
}

/// Live bytes across all malloc zones.
fn heap() -> usize {
    let mut stats = MallocStatistics::default();
    // SAFETY: a null zone asks for the totals over every zone, and `stats` is a live,
    // correctly laid out `malloc_statistics_t` (four fields, `u32` then three `usize`).
    unsafe { malloc_zone_statistics(std::ptr::null_mut(), &mut stats) };
    stats.size_in_use
}

/// Edit the text, tell the tree, re-parse incrementally: one keystroke.
fn edit(
    parser: &mut Parser,
    text: &mut String,
    tree: &mut tree_sitter::Tree,
    at: usize,
    del: usize,
    ins: &str,
) {
    let row = text[..at].matches('\n').count();
    let column = at - text[..at].rfind('\n').map_or(0, |newline| newline + 1);
    text.replace_range(at..at + del, ins);
    tree.edit(&InputEdit {
        start_byte: at,
        old_end_byte: at + del,
        new_end_byte: at + ins.len(),
        start_position: Point::new(row, column),
        old_end_position: Point::new(row, column + del),
        new_end_position: Point::new(row, column + ins.len()),
    });
    *tree = parser
        .parse(text.as_str(), Some(&*tree))
        .expect("no timeout is set");
}

#[test]
fn ten_thousand_edits_inside_a_dollar_quoted_body_do_not_leak() {
    let mut text = String::from(
        "CREATE OR REPLACE FUNCTION f() RETURNS text AS $body$\nBEGIN RETURN 'x' || 'y'; END; $body$ LANGUAGE plpgsql;\n",
    );
    let mut parser = Parser::new();
    parser
        .set_language(&qh_sql_grammar::LANGUAGE.into())
        .unwrap();
    let mut tree = parser.parse(&text, None).unwrap();
    let at = text.find("RETURN").unwrap();

    // Warm up: the allocator's own caches and the parser's stacks settle in the first
    // few hundred edits, and that growth is not a leak.
    for i in 0..2_000 {
        let (del, ins) = if i % 2 == 0 { (0, "x") } else { (1, "") };
        edit(&mut parser, &mut text, &mut tree, at, del, ins);
    }
    let before = heap();
    for i in 0..10_000 {
        let (del, ins) = if i % 2 == 0 { (0, "x") } else { (1, "") };
        edit(&mut parser, &mut text, &mut tree, at, del, ins);
    }
    // Upstream's second leak: a dollar-quote whose tag equals the open one returned without
    // freeing its 1024-byte buffer (vendor/scanner.c, local fix). Unfixed, 2 x 1,000 parses
    // grow the heap by about 2 MB,
    // inside the same window as the edits.
    for _ in 0..1_000 {
        for sql in [
            "CREATE FUNCTION f() RETURNS text AS $body$ SELECT 1 WHERE a = $body$ LANGUAGE sql;",
            "DO $x$ SELECT $x$ $x$",
        ] {
            parser.parse(sql, None).unwrap();
        }
    }
    drop(tree);
    drop(parser);
    let grown = heap().saturating_sub(before);

    // Unpatched, this is about 740 KB (10,000 edits x 74 bytes); patched it stays flat.
    assert!(
        grown < 128 * 1024,
        "the heap grew by {grown} bytes over 10,000 edits and 2,000 parses: the scanner leaks"
    );
}
