//! Statement boundaries: the editor's list is the engine's, in every dialect, before and after
//! any sequence of edits, and its UTF-16 offsets are right for characters outside the BMP.

mod common;

use common::{dialect_of, Rng, DIALECTS};
use qh_editor::{statement_ranges, Analyzer, Dialect, Document, TextBuffer};
use qh_sql::statements_with_lines_dialect;

fn corpus() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sql"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read_to_string(path).unwrap(),
            )
        })
        .collect()
}

/// The UI's ranges, cut out of the text and trimmed, as strings.
fn pieces(text: &str, dialect: Dialect) -> Vec<String> {
    let buffer = TextBuffer::new(text).unwrap();
    statement_ranges(text, dialect)
        .unwrap()
        .chunks(2)
        .map(|pair| {
            text[buffer.byte_of(pair[0]).unwrap()..buffer.byte_of(pair[1]).unwrap()]
                .trim()
                .to_owned()
        })
        .collect()
}

#[test]
fn the_ranges_are_the_engines_statements_for_every_fixture_and_dialect() {
    for (name, text) in corpus() {
        for dialect in DIALECTS {
            let want: Vec<&str> = statements_with_lines_dialect(&text, dialect)
                .into_iter()
                .map(|s| s.text)
                .collect();
            assert_eq!(pieces(&text, dialect), want, "{name} as {dialect:?}");
        }
    }
}

#[test]
fn a_range_holds_no_semicolon_and_a_comment_only_piece_is_not_a_statement() {
    let text = "select 1; -- nothing here\n; /* nor here */ ;select 2 -- last";
    assert_eq!(
        pieces(text, Dialect::Generic),
        ["select 1", "select 2 -- last"]
    );
    assert_eq!(
        statement_ranges(text, Dialect::Generic).unwrap(),
        [0, 8, 44, 60]
    );
    assert!(statement_ranges("", Dialect::Generic).unwrap().is_empty());
    assert!(statement_ranges(";;; ", Dialect::Generic)
        .unwrap()
        .is_empty());
}

#[test]
fn a_semicolon_in_a_string_a_quoted_name_or_a_dollar_quote_does_not_split() {
    assert_eq!(
        pieces("select 'a;b'; select \"a;b\"", Dialect::Generic).len(),
        2
    );
    assert_eq!(
        pieces("select $t$a;b$t$; select 2", Dialect::Postgres).len(),
        2
    );
    assert_eq!(pieces("select `a;b`", Dialect::Mysql).len(), 1);
}

/// The two shapes W3-T0 found: MySQL reads a backslash escape and a `#` comment where the
/// generic scanner does not, so the same text is one statement or two.
#[test]
fn mysql_statements_follow_mysql_escapes_and_comments() {
    // A backslash escapes the quote in MySQL, so the string ends at the third quote and the
    // server runs the DELETE; without it the whole text is two strings and one statement.
    let escape = "SELECT '\\''; DELETE FROM t; -- '";
    assert_eq!(
        pieces(escape, Dialect::Mysql),
        ["SELECT '\\''", "DELETE FROM t"]
    );
    assert_eq!(pieces(escape, Dialect::Trino).len(), 1);
    let hash = "SELECT 1 # '\n; DELETE FROM t; -- '";
    assert_eq!(
        pieces(hash, Dialect::Mysql),
        ["SELECT 1 # '", "DELETE FROM t"]
    );
    assert_eq!(
        pieces(hash, Dialect::Generic),
        ["SELECT 1 # '\n; DELETE FROM t; -- '"]
    );
}

#[test]
fn offsets_are_utf16_units_for_characters_outside_the_bmp() {
    // 😀 is two units: "select '😀'" is 7 + 4 = 11 units, the `;` is unit 11.
    let text = "select '😀'; select 😀 -- 😀\n; x";
    let ranges = statement_ranges(text, Dialect::Generic).unwrap();
    let units: Vec<u16> = text.encode_utf16().collect();
    assert_eq!(&ranges[..2], [0, 11]);
    assert_eq!(units[11], u16::from(b';'));
    for pair in ranges.chunks(2) {
        assert!(pair[0] < pair[1] && pair[1] as usize <= units.len());
    }
    assert_eq!(
        String::from_utf16(&units[ranges[2] as usize..ranges[3] as usize]).unwrap(),
        " select 😀 -- 😀\n"
    );
    assert_eq!(
        String::from_utf16(&units[ranges[4] as usize..ranges[5] as usize]).unwrap(),
        " x"
    );
}

/// An edit that lands between the two halves of a pair is refused, wherever it is.
#[test]
fn an_edit_inside_a_surrogate_pair_is_refused_and_leaves_the_document_alone() {
    let mut doc = Document::new("a😀b", Dialect::Generic).unwrap();
    for (start, len) in [(2, 0), (2, 1), (1, 1), (0, 2), (3, 2)] {
        assert!(doc.replace(start, len, "x").is_err(), "{start}+{len}");
    }
    assert_eq!(doc.text(), "a😀b");
    assert_eq!(doc.revision(), 1);
    assert!(doc.replace(1, 2, "x").is_ok());
}

const PIECES: &[&str] = &[
    "'", "\"", "`", "$", "$t$", "$$", "/*", "/*!", "*/", "--", "-- ", "#", "\n", "\r", ";", ";",
    ";", "a", " ", "\\", "''", "E'", "é", "😀", "select 1", "(", ")",
];

fn edits(text: &str, rng: &mut Rng) -> (u32, u32, &'static str) {
    let boundaries: Vec<usize> = text
        .char_indices()
        .map(|(at, _)| at)
        .chain([text.len()])
        .collect();
    let at = boundaries[rng.below(boundaries.len())];
    let mut end = at;
    for _ in 0..rng.below(6) {
        if let Some(ch) = text[end..].chars().next() {
            end += ch.len_utf8();
        }
    }
    let units = |bytes: usize| text[..bytes].encode_utf16().count() as u32;
    (
        units(at),
        units(end) - units(at),
        if rng.below(5) == 0 {
            ""
        } else {
            *rng.pick(PIECES)
        },
    )
}

/// The analyzer replays edits onto its own copy of the text and re-walks only from the
/// statement an edit is in. It must land where a scan of the whole text does, in every
/// dialect, including when several edits arrive before it looks.
#[test]
fn the_list_follows_the_engines_scanner_through_random_edits() {
    for (i, dialect) in DIALECTS.into_iter().enumerate() {
        let mut rng = Rng(0x57A7_0000 + i as u64);
        let mut text: String = corpus()
            .into_iter()
            .filter(|(name, _)| {
                name.starts_with("edge")
                    || dialect_of(name) == dialect
                    || name.starts_with("params")
            })
            .map(|(_, text)| text)
            .collect();
        text.truncate(
            text.char_indices()
                .map(|(at, _)| at)
                .take_while(|&at| at <= 6000)
                .last()
                .unwrap(),
        );
        let mut buffer = TextBuffer::new(&text).unwrap();
        let mut analyzer = Analyzer::new(&text, dialect).unwrap();
        for step in 0..1500 {
            let mut what = String::new();
            // Now and then several edits are logged before the analyzer syncs.
            for _ in 0..(if rng.below(5) == 0 {
                1 + rng.below(4)
            } else {
                1
            }) {
                let current = buffer.as_str().to_owned();
                let (start, len, piece) = edits(&current, &mut rng);
                buffer.replace(start, len, piece).unwrap();
                what.push_str(&format!(" {start}+{len}{piece:?}"));
            }
            analyzer.sync(buffer.drain_log()).unwrap();
            let scan = qh_sql::scan_dialect(buffer.as_str(), dialect);
            let mut starts = vec![0];
            starts.extend(scan.separators.iter().map(|at| at + 1));
            assert_eq!(
                analyzer.statement_starts(),
                starts,
                "{dialect:?} step {step}:{what}"
            );
        }
    }
}

#[test]
fn a_tree_cache_that_is_over_budget_drops_trees_and_keeps_the_colours() {
    let text = "select a, b from some_table where c = 1;\n".repeat(30_000);
    let mut doc = Document::new(&text, Dialect::Generic).unwrap();
    let mut painted = 0;
    let mut at = 0;
    while at < doc.len_utf16() {
        let paint = doc.paint(1, at, 60_000, u32::MAX).unwrap();
        painted += paint.runs.len() / 3;
        at += 60_000;
    }
    let stats = doc.stats();
    assert!(
        stats.tree_source_bytes <= qh_editor::TREE_CACHE_BYTES,
        "{stats:?}"
    );
    assert!(stats.trees < stats.statements, "{stats:?}");
    assert!(
        stats.tokens > 100_000,
        "the tokens outlive their trees: {stats:?}"
    );
    assert!(painted > 100_000);
}
