//! Toggle comment: the B1 line-comment cases, refusal inside regions, alignment, and a
//! property test over every dialect (a comment always comes off again, nothing but the marker
//! moves).

use qh_sql::{toggle_line_comment, CommentError, Dialect, Lexer, LineEdit};

const DIALECTS: [Dialect; 4] = [
    Dialect::Generic,
    Dialect::Postgres,
    Dialect::Mysql,
    Dialect::Trino,
];

/// Generic is the union of the four families (blueprint D-1).
fn readings(d: Dialect) -> Vec<Lexer> {
    if d == Dialect::Generic {
        DIALECTS
            .iter()
            .flat_map(|d| d.readings().to_vec())
            .collect()
    } else {
        d.readings().to_vec()
    }
}

fn apply(sql: &str, e: &LineEdit) -> String {
    format!("{}{}{}", &sql[..e.start], e.replacement, &sql[e.end..])
}

fn toggle_in(sql: &str, d: Dialect, first: usize, last: usize) -> Result<String, CommentError> {
    toggle_line_comment(sql, &readings(d), first, last).map(|e| apply(sql, &e))
}

/// Toggle every line of `sql` under Postgres readings.
fn pg(sql: &str) -> Result<String, CommentError> {
    toggle_in(sql, Dialect::Postgres, 1, line_count(sql))
}

fn line_count(sql: &str) -> usize {
    let b = sql.as_bytes();
    let mut n = 1;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\r' if b.get(i + 1) == Some(&b'\n') => {
                n += 1;
                i += 1;
            }
            b'\n' | b'\r' => n += 1,
            _ => {}
        }
        i += 1;
    }
    n
}

fn all_dialects(sql: &str, first: usize, last: usize, want: Result<&str, CommentError>) {
    for d in DIALECTS {
        assert_eq!(
            toggle_in(sql, d, first, last),
            want.map(String::from),
            "{d:?}: {sql:?}"
        );
    }
}

#[test]
fn toggle_uncomments_a_line_comment_line() {
    all_dialects("-- x", 1, 1, Ok("x"));
    all_dialects(
        "SELECT 1;\n-- x\nSELECT 2",
        2,
        2,
        Ok("SELECT 1;\nx\nSELECT 2"),
    );
    all_dialects("--\nSELECT 1", 1, 1, Ok("\nSELECT 1"));
}

#[test]
fn toggle_comments_a_line_ending_in_a_line_comment() {
    all_dialects("SELECT 1 -- x", 1, 1, Ok("-- SELECT 1 -- x"));
    all_dialects(
        "SELECT 1 -- x\nSELECT 2",
        1,
        1,
        Ok("-- SELECT 1 -- x\nSELECT 2"),
    );
}

#[test]
fn toggle_refuses_a_line_that_starts_inside_a_block_comment() {
    let sql = "SELECT 1; /* a\nb\nc */ SELECT 2";
    all_dialects(sql, 2, 2, Err(CommentError::InsideRegion { line: 2 }));
    // The line that opens it ends inside, the line that closes it starts inside.
    all_dialects(sql, 1, 1, Err(CommentError::InsideRegion { line: 1 }));
    all_dialects(sql, 3, 3, Err(CommentError::InsideRegion { line: 3 }));
    // The first offending line is the one reported, even in a larger range.
    all_dialects(
        "SELECT 0;\nSELECT 1; /* a\nb */",
        1,
        3,
        Err(CommentError::InsideRegion { line: 2 }),
    );
}

#[test]
fn toggle_treats_an_open_line_comment_at_eof_as_closed() {
    // No newline after the comment: `walk` ends `Open(LineComment)`, which is valid SQL.
    all_dialects("SELECT 1;\n-- note", 2, 2, Ok("SELECT 1;\nnote"));
    all_dialects("SELECT 1 -- note", 1, 1, Ok("-- SELECT 1 -- note"));
    all_dialects("-- a\n-- b", 1, 2, Ok("a\nb"));
}

#[test]
fn toggle_refuses_lines_inside_strings_quoted_names_and_dollar_quotes() {
    let cases: [(&str, Dialect); 5] = [
        ("SELECT 'a\nb\nc'", Dialect::Postgres),
        ("SELECT \"a\nb\nc\"", Dialect::Postgres),
        ("SELECT `a\nb\nc`", Dialect::Mysql),
        ("SELECT $$a\nb\nc$$", Dialect::Postgres),
        ("SELECT $t$a\nb\nc$t$", Dialect::Postgres),
    ];
    for (sql, d) in cases {
        for line in 1..=3 {
            assert_eq!(
                toggle_in(sql, d, line, line),
                Err(CommentError::InsideRegion { line }),
                "{sql:?} line {line}"
            );
        }
        assert_eq!(
            toggle_in(sql, d, 1, 3),
            Err(CommentError::InsideRegion { line: 1 }),
            "{sql:?}"
        );
    }
}

#[test]
fn toggle_refuses_a_line_that_opens_a_string_or_comment_never_closed() {
    let d = Dialect::Postgres;
    assert_eq!(
        toggle_in("SELECT 'a", d, 1, 1),
        Err(CommentError::InsideRegion { line: 1 })
    );
    assert_eq!(
        toggle_in("SELECT 1 /* a", d, 1, 1),
        Err(CommentError::InsideRegion { line: 1 })
    );
    // Lines before the opener are fine, the opener and everything after is refused.
    let sql = "SELECT 1\n'abc\nSELECT 2";
    assert_eq!(
        toggle_in(sql, d, 1, 1).as_deref(),
        Ok("-- SELECT 1\n'abc\nSELECT 2")
    );
    assert_eq!(
        toggle_in(sql, d, 2, 2),
        Err(CommentError::InsideRegion { line: 2 })
    );
    assert_eq!(
        toggle_in(sql, d, 3, 3),
        Err(CommentError::InsideRegion { line: 3 })
    );
}

#[test]
fn toggle_accepts_a_region_that_opens_and_closes_on_its_line() {
    // Start and end sit on the region's edge or outside it: not inside.
    assert_eq!(pg("'a;b'").as_deref(), Ok("-- 'a;b'"));
    assert_eq!(
        pg("SELECT 'x', \"y z\", $$ a $$, /* c */ 1").as_deref(),
        Ok("-- SELECT 'x', \"y z\", $$ a $$, /* c */ 1")
    );
    assert_eq!(pg("/* c */").as_deref(), Ok("-- /* c */"));
}

#[test]
fn toggle_refuses_what_any_reading_says_is_inside() {
    // Backslash escapes: MySQL keeps the string open past the first line, the other reading
    // closes it there. One reading is enough to refuse.
    let sql = "SELECT '\\'\nb'";
    assert_eq!(
        toggle_line_comment(sql, &[Lexer::GENERIC], 1, 1).map(|e| apply(sql, &e)),
        Ok("-- SELECT '\\'\nb'".to_string())
    );
    assert_eq!(
        toggle_in(sql, Dialect::Mysql, 1, 1),
        Err(CommentError::InsideRegion { line: 1 })
    );
    // An executable comment is code in one MySQL reading and a block comment in another.
    assert_eq!(
        toggle_in("SELECT 1 /*! + 2\n*/", Dialect::Mysql, 2, 2),
        Err(CommentError::InsideRegion { line: 2 })
    );
}

#[test]
fn toggle_refuses_a_comment_that_would_join_two_strings() {
    // PostgreSQL reads `'a'`, a newline and `'b'` as one string `'ab'`, so commenting out the
    // line between them (here `=`) would silently merge them. Under no reading may it change.
    let sql = "SELECT x
WHERE y
= 'a'
AND
'b' = z";
    assert_eq!(
        toggle_in(sql, Dialect::Postgres, 4, 4),
        Err(CommentError::Spills)
    );
    // Other lines are fine, and so is the same text where the strings are not adjacent.
    assert!(toggle_in(sql, Dialect::Postgres, 5, 5).is_ok());
    assert!(toggle_in(
        "SELECT 'a'
AND
1 = 'b'",
        Dialect::Postgres,
        2,
        2
    )
    .is_ok());
}

#[test]
fn toggle_aligns_a_block_at_the_shared_indentation() {
    all_dialects("  a\n    b\n  c", 1, 3, Ok("  -- a\n  --   b\n  -- c"));
    // Mixed tabs and spaces: only the shared prefix counts, so every marker lines up.
    all_dialects("\ta\n  b", 1, 2, Ok("-- \ta\n--   b"));
    all_dialects("\t\ta\n\tb", 1, 2, Ok("\t-- \ta\n\t-- b"));
    all_dialects("a\n  b", 1, 2, Ok("-- a\n--   b"));
}

#[test]
fn toggle_removes_the_marker_wherever_it_stands() {
    all_dialects("  -- a\n    -- b", 1, 2, Ok("  a\n    b"));
    all_dialects("-- a\n--\n--  b", 1, 3, Ok("a\n\n b"));
    // Not every line is commented, so the lines are commented again, not stripped.
    all_dialects("-- a\nb", 1, 2, Ok("-- -- a\n-- b"));
}

#[test]
fn toggle_only_counts_dash_dash_followed_by_space_or_line_end() {
    all_dialects("--x", 1, 1, Ok("-- --x"));
    // MySQL's `#` is a comment, but not one this toggle opens.
    all_dialects("# x", 1, 1, Ok("-- # x"));
    all_dialects("a --b", 1, 1, Ok("-- a --b"));
}

#[test]
fn toggle_leaves_blank_lines_alone() {
    all_dialects("a\n\n  \nb", 1, 4, Ok("-- a\n\n  \n-- b"));
    all_dialects("-- a\n\n-- b", 1, 3, Ok("a\n\nb"));
    // Nothing but blank lines: an empty edit.
    for sql in ["", "\n", "  \n\t\n"] {
        let e = toggle_line_comment(sql, &readings(Dialect::Postgres), 1, line_count(sql)).unwrap();
        assert_eq!(e.start, e.end);
        assert!(e.replacement.is_empty());
        assert_eq!(apply(sql, &e), sql);
    }
}

#[test]
fn toggle_works_on_the_lines_given_and_leaves_the_rest() {
    let sql = "a\nb\nc\nd";
    all_dialects(sql, 2, 3, Ok("a\n-- b\n-- c\nd"));
    all_dialects(sql, 4, 4, Ok("a\nb\nc\n-- d"));
    // The edit covers the markers and no more.
    let e = toggle_line_comment(sql, &readings(Dialect::Postgres), 2, 3).unwrap();
    assert_eq!(
        (e.start, e.end, e.replacement.as_str()),
        (2, 4, "-- b\n-- ")
    );
}

#[test]
fn toggle_keeps_line_endings_and_non_ascii_text() {
    all_dialects("é\r\nb\r\n", 1, 3, Ok("-- é\r\n-- b\r\n"));
    all_dialects("-- é\r\n-- b\r\n", 1, 3, Ok("é\r\nb\r\n"));
    // PostgreSQL and Trino end a `--` comment at a lone CR, so each piece gets its own marker.
    // The others read the comment on past it, which would swallow the next line: refused.
    for d in [Dialect::Postgres, Dialect::Trino] {
        assert_eq!(
            toggle_in("a\rb", d, 1, 2).as_deref(),
            Ok("-- a\r-- b"),
            "{d:?}"
        );
    }
    for d in [Dialect::Generic, Dialect::Mysql] {
        assert_eq!(
            toggle_in("a\rb", d, 1, 2),
            Err(CommentError::Spills),
            "{d:?}"
        );
    }
    all_dialects("日本 'x'\n  語", 1, 2, Ok("-- 日本 'x'\n--   語"));
}

#[test]
fn toggle_rejects_a_range_outside_the_text() {
    for (first, last) in [(0, 1), (2, 1), (1, 3), (3, 3)] {
        assert_eq!(
            toggle_in("a\nb", Dialect::Generic, first, last),
            Err(CommentError::OutOfRange),
            "{first}..={last}"
        );
    }
    // The empty line after a final newline is a line.
    assert!(toggle_in("a\n", Dialect::Generic, 1, 2).is_ok());
    assert!(toggle_in("", Dialect::Generic, 1, 1).is_ok());
}

// A tiny deterministic generator: no dependency, reproducible.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.next() as usize % xs.len()]
    }
}

const FRAGMENTS: &[&str] = &[
    "select",
    "from",
    "where",
    "t",
    "a",
    "1",
    "$1",
    ":p",
    "::",
    "=",
    "-",
    "--",
    "-- c",
    "--x",
    "#",
    ",",
    ";",
    "(",
    ")",
    "'x y'",
    "'a''b'",
    "'multi\nline'",
    "'open",
    "\"q i\"",
    "\"two\nlines\"",
    "`b t`",
    "e'a\\'b'",
    "'\\'",
    "$$ a ; b $$",
    "$$ x\ny $$",
    "$t$ x $t$",
    "/* c */",
    "/* a\nb */",
    "/* open",
    "/*! x */",
    "/*! x\ny */",
    "# h",
    "-- c\n",
    "# h\n",
    "é",
    "\u{a0}",
    "\x01",
    "\x0c",
];
const GAPS: &[&str] = &[
    "", " ", " ", "  ", "\n", "\n", "\n", "\n  ", "\n\t", "\n\n", "\r\n", "\r", "\t", "\n    ",
];

fn random_sql(rng: &mut Rng) -> String {
    let n = 1 + rng.next() % 20;
    let mut s = String::new();
    for _ in 0..n {
        s.push_str(rng.pick(FRAGMENTS));
        s.push_str(rng.pick(GAPS));
    }
    s
}

#[test]
fn property_a_comment_comes_off_again_and_only_the_marker_moves() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    let mut commented = [0usize; 4];
    let mut uncommented = 0usize;
    let mut refused = 0usize;
    let mut spilled = 0usize;
    for round in 0..6000 {
        let sql = random_sql(&mut rng);
        let lines = line_count(&sql);
        let first = 1 + rng.next() as usize % lines;
        let last = first + rng.next() as usize % (lines - first + 1);
        for (di, d) in DIALECTS.into_iter().enumerate() {
            let r = readings(d);
            let edit = match toggle_line_comment(&sql, &r, first, last) {
                Ok(e) => e,
                Err(CommentError::Spills) => {
                    spilled += 1;
                    continue;
                }
                Err(CommentError::InsideRegion { line }) => {
                    assert!((first..=last).contains(&line), "{d:?} #{round}: {sql:?}");
                    refused += 1;
                    continue;
                }
                Err(e) => panic!("{d:?} #{round}: {e:?}: {sql:?} {first}..={last}"),
            };
            assert!(edit.start <= edit.end && edit.end <= sql.len());
            let out = apply(&sql, &edit);
            // Only dashes and white space may differ.
            let kept = |s: &str| {
                s.bytes()
                    .filter(|b| !b" \t\n\r\x0b\x0c-".contains(b))
                    .collect::<Vec<_>>()
            };
            assert_eq!(kept(&sql), kept(&out), "{d:?} #{round}: {sql:?} -> {out:?}");
            let removed = edit.end - edit.start;
            if edit.replacement.len() > removed {
                // A comment: markers only were added, and toggling again restores the text.
                commented[di] += 1;
                assert_eq!(line_count(&out), lines, "{d:?} #{round}: {sql:?}");
                let back = toggle_line_comment(&out, &r, first, last).map(|e| apply(&out, &e));
                assert_eq!(
                    back.as_deref(),
                    Ok(sql.as_str()),
                    "{d:?} #{round}: {sql:?} {first}..={last} -> {out:?}"
                );
            } else if edit.replacement.len() < removed {
                // An uncomment: only `--` and an optional space left each line.
                uncommented += 1;
                let gone = removed - edit.replacement.len();
                assert!((2..=3 * lines).contains(&gone), "{d:?} #{round}: {sql:?}");
            } else {
                assert_eq!(out, sql, "{d:?} #{round}: {sql:?}");
            }
        }
    }
    // The generator must reach every outcome, or the property proves nothing.
    for (d, n) in DIALECTS.iter().zip(commented) {
        assert!(n > 500, "{d:?}: only {n} comments");
    }
    assert!(uncommented > 100, "only {uncommented} uncomments");
    assert!(refused > 500, "only {refused} refusals");
    assert!(spilled > 0, "no input spilled");
}
