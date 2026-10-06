//! Formatter: layout goldens, the byte/token invariants as a property test over every
//! dialect, idempotence, and the B1 line-comment cases.

use qh_sql::{
    classify_readings, format_sql, lex, Dialect, FormatError, FormatOptions, Lexer, OpaqueKind,
    TokenKind,
};

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

fn fmt(sql: &str) -> String {
    format_sql(sql, Dialect::Postgres.readings(), &FormatOptions::default())
        .unwrap()
        .text
}

/// Tokens of `sql` with `,` `;` `(` `)` split out of operator runs, as the formatter's
/// atoms do (a separator may land after a comma that was glued to an operator).
fn tokens(sql: &str, lexer: Lexer) -> Vec<(String, String)> {
    let mut raw = Vec::new();
    lex(sql.as_bytes(), 0..sql.len(), lexer, |t| raw.push(t));
    let mut out = Vec::new();
    for t in raw {
        let text = &sql[t.start..t.end];
        if t.kind == TokenKind::Punct {
            let mut run = String::new();
            for c in text.chars() {
                if matches!(c, ',' | ';' | '(' | ')') {
                    if !run.is_empty() {
                        out.push(("P".into(), std::mem::take(&mut run)));
                    }
                    out.push(("P".into(), c.to_string()));
                } else {
                    run.push(c);
                }
            }
            if !run.is_empty() {
                out.push(("P".into(), run));
            }
        } else {
            out.push((format!("{:?}", t.kind), text.to_string()));
        }
    }
    out
}

#[test]
fn lays_out_clauses_lists_and_conditions() {
    assert_eq!(
        fmt("select a,b from t left join u on t.id=u.id where x=1 and y between 1 and 2 or z order by a,b;select 1"),
        "select\n  a,\n  b\nfrom t\nleft join u\n  on t.id=u.id\nwhere x=1\n  and y between 1 and 2\n  or z\norder by\n  a,\n  b;\nselect 1"
    );
}

#[test]
fn keeps_expression_keywords_on_one_line() {
    let sql = "select extract(year from d), a is distinct from b, count(*) over (partition by a order by b) from t";
    assert_eq!(
        fmt(sql),
        "select\n  extract(year from d),\n  a is distinct from b,\n  count(*) over (partition by a order by b)\nfrom t"
    );
}

#[test]
fn indents_a_subquery_and_keeps_a_blank_line_between_statements() {
    assert_eq!(
        fmt("select * from ( select 1 ) x;\n\n\nselect 2\n\n"),
        "select *\nfrom (\n  select 1\n) x;\n\nselect 2\n"
    );
}

#[test]
fn format_accepts_line_comment_at_eof_without_newline() {
    for d in DIALECTS {
        let r = readings(d);
        let out = format_sql("select  1 -- note", &r, &FormatOptions::default()).unwrap();
        assert_eq!(out.text, "select 1 -- note");
        let again = format_sql(&out.text, &r, &FormatOptions::default()).unwrap();
        assert_eq!(again, out);
    }
    assert_eq!(fmt("select 1;\n-- note"), "select 1;\n-- note");
}

#[test]
fn format_rejects_unterminated_block_comment_at_eof() {
    let err = format_sql(
        "select 1 /* note",
        Dialect::Generic.readings(),
        &FormatOptions::default(),
    );
    assert!(matches!(
        err,
        Err(FormatError::Unterminated {
            kind: OpaqueKind::BlockComment,
            ..
        })
    ));
    assert!(matches!(
        format_sql(
            "select 'a",
            Dialect::Generic.readings(),
            &FormatOptions::default()
        ),
        Err(FormatError::Unterminated { .. })
    ));
}

#[test]
fn refuses_text_the_readings_disagree_about() {
    // A backslash before the closing quote: MySQL (without NO_BACKSLASH_ESCAPES) keeps the
    // string open, the other reading closes it.
    let err = format_sql(
        r"select '\' , 'x'",
        Dialect::Mysql.readings(),
        &FormatOptions::default(),
    );
    assert!(
        matches!(err, Err(FormatError::Ambiguous { line: 1 })),
        "{err:?}"
    );
}

#[test]
fn keeps_string_continuation_gaps_and_comment_lines() {
    assert_eq!(fmt("select 'a'\n   'b'"), "select 'a'\n   'b'");
    assert_eq!(
        fmt("select a, -- first\n b /* c */ from t"),
        "select\n  a, -- first\n  b /* c */\nfrom t"
    );
}

#[test]
fn tabs_indent() {
    let o = FormatOptions {
        indent: qh_sql::Indent::Tab,
    };
    let out = format_sql("select a,b", Dialect::Postgres.readings(), &o).unwrap();
    assert_eq!(out.text, "select\n\ta,\n\tb");
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
    "SELECT",
    "from",
    "where",
    "and",
    "or",
    "between",
    "group",
    "by",
    "order",
    "having",
    "limit",
    "union",
    "all",
    "join",
    "left",
    "on",
    "using",
    "insert",
    "into",
    "values",
    "update",
    "set",
    "delete",
    "with",
    "recursive",
    "as",
    "case",
    "when",
    "then",
    "end",
    "distinct",
    "is",
    "not",
    "null",
    "t",
    "u",
    "a",
    "b",
    "id",
    "1",
    "2.5",
    "1abc",
    "$1",
    ":p",
    "::",
    "->>",
    "=",
    "<>",
    "+",
    "*",
    "-",
    ",",
    ";",
    "(",
    ")",
    "'x y'",
    "'a''b'",
    "\"q i\"",
    "`b t`",
    "e'a\\'b'",
    "$$ a ; b $$",
    "$t$ x $t$",
    "/* c */",
    "/* ; */",
    "/*! x */",
    "# h\n",
    "-- c\n",
    "é",
    "\u{a0}",
    "x\u{a0}y",
    "\x01",
];
const GAPS: &[&str] = &["", " ", " ", " ", "  ", "\n", "\t", "\r\n", "\n\n", "\n  "];

fn random_sql(rng: &mut Rng) -> String {
    let n = 1 + rng.next() % 24;
    let mut s = String::new();
    for _ in 0..n {
        let frag = rng.pick(FRAGMENTS);
        s.push_str(frag);
        // A line comment needs its newline; the fragment carries it (or ends the text).
        if !frag.ends_with('\n') {
            s.push_str(rng.pick(GAPS));
        }
    }
    s
}

#[test]
fn property_tokens_survive_and_format_is_idempotent_per_dialect() {
    let mut rng = Rng(0x9e3779b97f4a7c15);
    let opts = FormatOptions::default();
    let mut accepted = [0usize; 4];
    for round in 0..4000 {
        let sql = random_sql(&mut rng);
        for (di, d) in DIALECTS.into_iter().enumerate() {
            let r = readings(d);
            let Ok(out) = format_sql(&sql, &r, &opts) else {
                continue;
            };
            accepted[di] += 1;
            let text = &out.text;
            // I-F1: same non-space bytes; tokens match under every reading.
            let kept = |s: &str| {
                s.bytes()
                    .filter(|b| !b" \t\n\r\x0b\x0c".contains(b))
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                kept(&sql),
                kept(text),
                "{d:?} #{round}: {sql:?} -> {text:?}"
            );
            for l in &r {
                assert_eq!(
                    tokens(&sql, *l),
                    tokens(text, *l),
                    "{d:?} #{round}: {sql:?} -> {text:?}"
                );
            }
            // I-F4: idempotent.
            let again = format_sql(text, &r, &opts).unwrap_or_else(|e| {
                panic!("{d:?} #{round}: second pass {e:?}: {sql:?} -> {text:?}")
            });
            assert_eq!(&again.text, text, "{d:?} #{round}: {sql:?}");
            // I-F5: the safety classification does not move.
            assert_eq!(
                classify_readings(&sql, &r),
                classify_readings(text, &r),
                "{d:?} #{round}: {sql:?} -> {text:?}"
            );
        }
    }
    // The generator must mostly get through, or the property proves nothing.
    for (d, n) in DIALECTS.iter().zip(accepted) {
        assert!(n > 800, "{d:?}: only {n} of 4000 inputs were accepted");
    }
}

#[test]
fn lists_starting_with_a_non_word_stay_compact() {
    for (sql, want) in [
        ("select 1 as x, 2 as y", "select\n  1 as x,\n  2 as y"),
        ("select 'a' || b, c", "select\n  'a' || b,\n  c"),
        ("select (a+b) as c, d", "select\n  (a+b) as c,\n  d"),
        (
            "select a from t order by 1 desc, 2",
            "select a\nfrom t\norder by\n  1 desc,\n  2",
        ),
    ] {
        assert_eq!(fmt(sql), want, "{sql}");
        assert_eq!(fmt(want), want, "idempotent: {sql}");
    }
}

#[test]
fn deep_nesting_is_bounded_and_never_aborts() {
    let opts = FormatOptions::default();
    for n in [4000usize, 20_000] {
        let sql = "( select ".repeat(n);
        // Either refused or formatted within the output bound; never a runaway allocation.
        if let Ok(o) = format_sql(&sql, &[Lexer::GENERIC], &opts) {
            assert!(o.text.len() <= (2 * sql.len()).max(4 << 20) + 1024);
        }
    }
}
