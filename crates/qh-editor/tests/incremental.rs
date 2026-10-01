//! G2 and G3: random edits, checked against a parse from nothing.
//!
//! A model of the screen stands in for the text view: it moves colours with the text the way
//! temporary attributes move (new text has none), and applies each paint the way the app does
//! (clear the ranges, set the runs, set the fonts). Nothing is ever re-synchronised with a fresh
//! parse, so a wrong colour left by one edit stays and shows up later.
//!
//! Every step checks, against the plain text: the statement list equals `scan_dialect`, the line
//! and UTF-16 indexes equal a naive count, and the screen equals a fresh document's paint once the
//! analyzer has settled and converged. Colours before convergence are compared too, and
//! counted; only the converged ones must be equal (an incremental tree that holds an error may
//! read the text differently from a fresh one until it is parsed again).
//!
//! `QH_EDITOR_SOAK=1` runs ten times as many steps.

mod common;

use common::{validate, Rng, DIALECTS};
use qh_editor::{Class, Dialect, Document, EditError, Paint};

/// The colour and the italic of every UTF-16 unit as the UI holds them: 0 is "none".
#[derive(Clone, PartialEq, Eq, Debug, Default)]
struct Screen {
    class: Vec<u8>,
    italic: Vec<u8>,
}

impl Screen {
    fn edit(&mut self, start: usize, len: usize, new_len: usize) {
        self.class
            .splice(start..start + len, std::iter::repeat_n(0, new_len));
        self.italic
            .splice(start..start + len, std::iter::repeat_n(0, new_len));
    }

    fn apply(&mut self, paint: &Paint) {
        assert_eq!(
            self.class.len(),
            paint.doc_len_utf16 as usize,
            "the paint is for another length"
        );
        for range in paint.ranges.chunks(2) {
            let (a, b) = (range[0] as usize, (range[0] + range[1]) as usize);
            self.class[a..b].fill(0);
        }
        for run in paint.runs.chunks(3) {
            self.class[run[0] as usize..(run[0] + run[1]) as usize].fill(run[2] as u8);
        }
        for font in paint.fonts.chunks(3) {
            self.italic[font[0] as usize..(font[0] + font[1]) as usize].fill(font[2] as u8);
        }
    }
}

/// What a document of `text` looks like painted from nothing.
fn fresh(text: &str, dialect: Dialect) -> Screen {
    let mut doc = Document::new(text, dialect).unwrap();
    let paint = doc.paint(1, 0, u32::MAX, u32::MAX).unwrap();
    validate(&paint);
    let mut screen = Screen {
        class: vec![0; paint.doc_len_utf16 as usize],
        italic: vec![0; paint.doc_len_utf16 as usize],
    };
    screen.apply(&paint);
    // A fresh paint says exactly which font each unit has: the italic ones are the comments.
    for i in 0..screen.class.len() {
        assert_eq!(
            screen.italic[i] == 1,
            screen.class[i] == Class::Comment as u8
        );
    }
    screen
}

/// Paint until nothing is dirty, in windows and budgets of random sizes.
fn settle(doc: &mut Document, screen: &mut Screen, rng: &mut Rng) {
    for _ in 0..400 {
        let len = doc.len_utf16();
        let window = 200 + rng.below(6000) as u32;
        let budget = 50 + rng.below(3000) as u32;
        let mut quiet = true;
        let mut at = 0;
        let mut calls = 0;
        loop {
            calls += 1;
            let paint = doc.paint(doc.revision(), at, window, budget).unwrap();
            assert!(
                calls < 3000,
                "stuck at window {at}+{window} budget {budget}: {paint:?}\ntext {:?}",
                doc.text()
            );
            validate(&paint);
            screen.apply(&paint);
            quiet &= paint.ranges.is_empty();
            doc.mark_applied(paint.revision, paint.ranges.clone());
            if paint.more_in_window {
                continue;
            }
            at += window;
            if at >= len {
                break;
            }
        }
        if quiet {
            return;
        }
    }
    panic!("a document never settled");
}

const PIECES: &[&str] = &[
    "'", "\"", "$", "$body$", "$$", "/*", "*/", "--", "\n", ";", ";", "a", "é", "😀", "::",
    ":name", "[", "]", "'it\\'s'", " select ", "(", ")", ",", " ", "`", "#", "\\", " from ", "1",
    "\r\n", " end ", "$1", "?",
];

/// A random edit `(start, len, text)` in UTF-16 units, on character boundaries.
fn random_edit(text: &str, rng: &mut Rng) -> (usize, usize, &'static str) {
    let boundaries: Vec<usize> = text
        .char_indices()
        .map(|(at, _)| at)
        .chain([text.len()])
        .collect();
    let mut at = boundaries[rng.below(boundaries.len())];
    if rng.below(4) == 0 {
        // Next to a separator: typing or deleting a `;`, a quote right before or after one.
        let separators: Vec<usize> = text.match_indices(';').map(|(at, _)| at).collect();
        if !separators.is_empty() {
            let sep = separators[rng.below(separators.len())];
            at = (sep + rng.below(3)).saturating_sub(1).min(text.len());
            while !text.is_char_boundary(at) {
                at += 1;
            }
        }
    }
    let mut end = at;
    if rng.below(3) > 0 {
        for _ in 0..rng.below(8) {
            match text[end..].chars().next() {
                Some(ch) => end += ch.len_utf8(),
                None => break,
            }
        }
    }
    let units = |bytes: usize| text[..bytes].encode_utf16().count();
    let insert = if rng.below(6) == 0 {
        ""
    } else {
        *rng.pick(PIECES)
    };
    (units(at), units(end) - units(at), insert)
}

/// The byte range of a UTF-16 range, counted the plain way.
fn byte_range(text: &str, start: usize, len: usize) -> (usize, usize) {
    let (mut units, mut from, mut to) = (0, None, None);
    for (at, ch) in text.char_indices().chain([(text.len(), ' ')]) {
        if units == start {
            from.get_or_insert(at);
        }
        if units == start + len {
            to.get_or_insert(at);
        }
        units += ch.len_utf16();
    }
    (from.unwrap(), to.unwrap())
}

fn base_text() -> String {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/corpus");
    let mut text = String::new();
    for name in [
        "pg-mixed.sql",
        "edge-dollar.sql",
        "pg-probes.sql",
        "mysql-probes.sql",
        "edge-params.sql",
        "trino-probes.sql",
        "edge-surrogate.sql",
    ] {
        text.push_str(&std::fs::read_to_string(dir.join(name)).unwrap());
    }
    text.truncate(text.floor_char_boundary_compat(12_000));
    text
}

trait FloorCompat {
    fn floor_char_boundary_compat(&self, at: usize) -> usize;
}

impl FloorCompat for String {
    fn floor_char_boundary_compat(&self, mut at: usize) -> usize {
        at = at.min(self.len());
        while !self.is_char_boundary(at) {
            at -= 1;
        }
        at
    }
}

fn steps() -> usize {
    if let Some(n) = std::env::var("QH_EDITOR_STEPS")
        .ok()
        .and_then(|n| n.parse().ok())
    {
        n
    } else if std::env::var_os("QH_EDITOR_SOAK").is_some() {
        4000
    } else {
        400
    }
}

fn check_indexes(doc: &Document, text: &str, rng: &mut Rng) {
    let buffer = doc.buffer();
    assert_eq!(buffer.as_str(), text);
    assert_eq!(buffer.line_count() as usize, text.matches('\n').count() + 1);
    assert_eq!(buffer.len_utf16() as usize, text.encode_utf16().count());
    let boundaries: Vec<usize> = text
        .char_indices()
        .map(|(at, _)| at)
        .chain([text.len()])
        .collect();
    for _ in 0..20 {
        let byte = boundaries[rng.below(boundaries.len())];
        let units = text[..byte].encode_utf16().count() as u32;
        assert_eq!(buffer.utf16_of(byte), units);
        assert_eq!(buffer.byte_of(units), Ok(byte));
        assert_eq!(
            buffer.line_of_byte(byte) as usize,
            text[..byte].matches('\n').count()
        );
    }
}

fn run(dialect: Dialect, seed: u64) -> (usize, usize) {
    let mut rng = Rng(seed);
    let mut text = base_text();
    let mut doc = Document::new(&text, dialect).unwrap();
    let mut screen = Screen {
        class: vec![0; doc.len_utf16() as usize],
        italic: vec![0; doc.len_utf16() as usize],
    };
    settle(&mut doc, &mut screen, &mut rng);
    let (mut unconverged, mut checked) = (0, 0);

    for step in 0..steps() {
        let (start, len, insert) = random_edit(&text, &mut rng);
        let before = text.clone();
        if std::env::var_os("QH_EDITOR_TRACE").is_some() {
            eprintln!("{dialect:?} step {step}: edit {start}+{len} {insert:?}");
        }
        doc.replace(start as u32, len as u32, insert).unwrap();
        screen.edit(start, len, insert.encode_utf16().count());
        let (from, to) = byte_range(&text, start, len);
        text.replace_range(from..to, insert);

        settle(&mut doc, &mut screen, &mut rng);
        let what = format!("{dialect:?} seed {seed} step {step}: edit {start}+{len} {insert:?}");

        // The statements are what the engine's scanner says.
        let scan = qh_sql::scan_dialect(&text, dialect);
        let mut starts = vec![0];
        starts.extend(scan.separators.iter().map(|at| at + 1));
        assert_eq!(doc.analyzer().statement_starts(), starts, "{what}");
        check_indexes(&doc, &text, &mut rng);

        let truth = fresh(&text, dialect);
        checked += 1;
        if screen != truth {
            unconverged += 1;
            if std::env::var_os("QH_EDITOR_TRACE").is_some() {
                let at = (0..screen.class.len())
                    .find(|&i| {
                        screen.class[i] != truth.class[i] || screen.italic[i] != truth.italic[i]
                    })
                    .unwrap();
                let units: Vec<u16> = text.encode_utf16().collect();
                eprintln!(
                    "UNCONVERGED {what}: unit {at} screen {} fresh {}: {:?}",
                    screen.class[at],
                    truth.class[at],
                    String::from_utf16_lossy(
                        &units[at.saturating_sub(60)..(at + 40).min(units.len())]
                    )
                );
            }
        }

        // Once the analyzer has converged, the screen is the fresh one, exactly.
        doc.converge().unwrap();
        settle(&mut doc, &mut screen, &mut rng);
        assert_eq!(screen.class.len(), truth.class.len(), "{what}");
        if let Some(at) = (0..screen.class.len())
            .find(|&i| screen.class[i] != truth.class[i] || screen.italic[i] != truth.italic[i])
        {
            let units: Vec<u16> = text.encode_utf16().collect();
            let lo = at.saturating_sub(30);
            let b: Vec<u16> = before.encode_utf16().collect();
            eprintln!(
                "BEFORE {:?}",
                String::from_utf16_lossy(
                    &b[start.saturating_sub(80)..(start + len + 80).min(b.len())]
                )
            );
            panic!(
                "{what}: unit {at} is class {} italic {}, a fresh paint says {} {}\ntext around it: {:?}",
                screen.class[at],
                screen.italic[at],
                truth.class[at],
                truth.italic[at],
                String::from_utf16_lossy(&units[lo..(at + 30).min(units.len())])
            );
        }

        // G3: what converged also outlines the same as a new document.
        let rev = doc.revision();
        let got = doc.outline(rev).unwrap();
        let want = Document::new(&text, dialect).unwrap().outline(1).unwrap();
        assert_eq!(got.statements, want.statements, "{what}");
        assert_eq!(got.folds, want.folds, "{what}");
        assert_eq!(got.issues, want.issues, "{what}");
    }
    (unconverged, checked)
}

#[test]
fn random_edits_converge_to_a_fresh_parse_in_every_dialect() {
    let (mut unconverged, mut checked) = (0, 0);
    for (i, dialect) in DIALECTS.into_iter().enumerate() {
        let (u, c) = run(dialect, 0x51DE_0000 + i as u64);
        unconverged += u;
        checked += c;
    }
    eprintln!(
        "colours differing from a fresh parse before convergence: {unconverged} of {checked} edits"
    );
}

#[test]
fn a_paint_of_an_old_revision_is_stale() {
    let mut doc = Document::new("select 1", Dialect::Generic).unwrap();
    doc.replace(8, 0, "0").unwrap();
    assert_eq!(doc.paint(1, 0, 10, 10), Err(EditError::Stale));
    assert_eq!(doc.outline(1).map(|_| ()), Err(EditError::Stale));
    assert!(doc.paint(2, 0, 10, 10).is_ok());
}

/// The example the blueprint's API section fixes.
#[test]
fn the_blueprints_example_paints_and_repaints_as_written() {
    let mut doc = Document::new("select :a -- c", Dialect::Generic).unwrap();
    let paint = doc.paint(1, 0, 14, u32::MAX).unwrap();
    assert_eq!(paint.revision, 1);
    assert_eq!(paint.doc_len_utf16, 14);
    assert!(!paint.inactive && !paint.more_in_window && !paint.dirty_elsewhere);
    assert_eq!(paint.ranges, [0, 14]);
    assert_eq!(paint.runs, [0, 6, 5, 7, 2, 9, 10, 4, 1]);
    assert_eq!(paint.fonts, [0, 10, 0, 10, 4, 1]);

    doc.mark_applied(1, vec![0, 14]);
    assert_eq!(doc.replace(14, 0, "x"), Ok(2));
    let paint = doc.paint(2, 0, 15, u32::MAX).unwrap();
    assert_eq!(paint.ranges, [10, 5]);
    assert_eq!(paint.runs, [10, 5, 1]);
    assert_eq!(paint.fonts, [14, 1, 1]);
}

#[test]
fn a_paint_that_was_not_applied_is_offered_again() {
    let mut doc = Document::new("select 1", Dialect::Generic).unwrap();
    let first = doc.paint(1, 0, 8, u32::MAX).unwrap();
    let again = doc.paint(1, 0, 8, u32::MAX).unwrap();
    assert_eq!(first, again);
    // Applied for a revision that is no longer the latest: ignored, still dirty.
    doc.replace(0, 0, " ").unwrap();
    doc.mark_applied(1, first.ranges.clone());
    let later = doc.paint(2, 0, 9, u32::MAX).unwrap();
    assert_eq!(later.ranges, [0, 9]);
}

#[test]
fn the_budget_cuts_a_paint_and_the_flags_say_what_is_left() {
    let text = "select 1;\n".repeat(50);
    let mut doc = Document::new(&text, Dialect::Generic).unwrap();
    let paint = doc.paint(1, 0, 100, 40).unwrap();
    validate(&paint);
    assert!(paint.more_in_window && paint.dirty_elsewhere);
    let painted: u32 = paint.ranges.chunks(2).map(|r| r[1]).sum();
    assert!((40..=50).contains(&painted), "{painted}");
    doc.mark_applied(1, paint.ranges.clone());
    let rest = doc.paint(1, 0, 100, u32::MAX).unwrap();
    assert!(!rest.more_in_window && rest.dirty_elsewhere);
    assert_eq!(rest.ranges[0], painted);
}

#[test]
fn a_change_of_font_repaints_the_range_it_names() {
    let mut doc = Document::new("select 1 -- c", Dialect::Generic).unwrap();
    let paint = doc.paint(1, 0, 13, u32::MAX).unwrap();
    doc.mark_applied(1, paint.ranges);
    assert!(doc.paint(1, 0, 13, u32::MAX).unwrap().ranges.is_empty());
    doc.mark_dirty(9, 4).unwrap();
    assert_eq!(doc.mark_dirty(9, 5), Err(EditError::OutOfBounds));
    let paint = doc.paint(1, 0, 13, u32::MAX).unwrap();
    assert_eq!(paint.ranges, [9, 4]);
    assert_eq!(paint.fonts, [9, 4, 1]);
}

#[test]
fn over_the_ceiling_nothing_is_coloured_and_it_comes_back_under_it() {
    let mut text = "select 1;\n".repeat(200_000);
    text.push_str("select 2");
    assert!(text.len() as u32 > qh_editor::CEILING_UTF16);
    let mut doc = Document::new(&text, Dialect::Generic).unwrap();
    let paint = doc.paint(1, 0, 1000, 1000).unwrap();
    assert!(paint.inactive && paint.runs.is_empty());
    assert_eq!(paint.ranges, [0, 1000]);
    assert_eq!(paint.fonts.len(), 3);
    // The statements are still counted.
    let outline = doc.outline(1).unwrap();
    assert_eq!(outline.statements.len(), 2 * 200_001);
    assert!(outline.folds.is_empty() && outline.issues.is_empty());

    let len = doc.len_utf16();
    let rev = doc
        .replace(
            qh_editor::CEILING_UTF16 / 2,
            len - qh_editor::CEILING_UTF16 / 2,
            "",
        )
        .unwrap();
    let paint = doc.paint(rev, 0, 100, u32::MAX).unwrap();
    assert!(!paint.inactive);
    assert!(!paint.runs.is_empty());
    assert!(
        paint.dirty_elsewhere,
        "the rest of the text has to be painted after the change"
    );
}
