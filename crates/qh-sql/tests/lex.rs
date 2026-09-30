//! G5: `lex` agrees with `walk` and clips to its range.
//!
//! The opaque tokens must be exactly the regions `walk` reports (so the editor colours a quote
//! the way Safe Mode reads it), the words must be the words `scan` collects, and lexing a
//! sub-range must be lexing that text on its own.

use qh_sql::{
    lex, scan_dialect, walk, Dialect, EndState, Lexer, OpaqueKind, Token, TokenKind, Visitor,
};

use TokenKind::{Number, Opaque, Param, Punct, Word};

fn all_lexers() -> Vec<Lexer> {
    let mut lexers = vec![Lexer::GENERIC, Lexer::TRINO];
    lexers.extend_from_slice(Dialect::Mysql.readings());
    lexers.extend_from_slice(Dialect::Postgres.readings());
    lexers
}

fn tokens(sql: &str, lexer: Lexer) -> Vec<Token> {
    let mut found = Vec::new();
    lex(sql.as_bytes(), 0..sql.len(), lexer, |token| {
        found.push(token)
    });
    found
}

/// `(kind, text)` pairs, for readable expectations.
fn spelled(sql: &str, lexer: Lexer) -> Vec<(TokenKind, &str)> {
    tokens(sql, lexer)
        .into_iter()
        .map(|token| (token.kind, &sql[token.start..token.end]))
        .collect()
}

#[derive(Default)]
struct Regions(Vec<(OpaqueKind, usize, usize)>);

impl Visitor for Regions {
    fn opaque(&mut self, kind: OpaqueKind, start: usize, end: usize) {
        self.0.push((kind, start, end));
    }
}

#[test]
fn code_is_cut_into_words_numbers_params_and_punctuation() {
    assert_eq!(
        spelled("select a, 12.5 from t where x >= :limit;", Lexer::GENERIC),
        vec![
            (Word, "select"),
            (Word, "a"),
            (Punct, ","),
            (Number, "12.5"),
            (Word, "from"),
            (Word, "t"),
            (Word, "where"),
            (Word, "x"),
            (Punct, ">="),
            (Param, ":limit"),
            (Punct, ";"),
        ]
    );
}

#[test]
fn a_cast_and_a_colon_after_a_word_are_not_parameters() {
    assert_eq!(
        spelled("x::int, a:b, (:c), 'x':d", Lexer::GENERIC),
        vec![
            (Word, "x"),
            (Punct, "::"),
            (Word, "int"),
            (Punct, ","),
            (Word, "a"),
            (Punct, ":"),
            (Word, "b"),
            (Punct, ","),
            (Punct, "("),
            (Param, ":c"),
            (Punct, "),"),
            (Opaque(OpaqueKind::SingleQuote), "'x'"),
            (Param, ":d"),
        ]
    );
}

#[test]
fn digits_glued_to_letters_are_a_word_and_not_a_number() {
    assert_eq!(
        spelled("1abc 2 3.x 1e5", Lexer::GENERIC),
        vec![
            (Word, "abc"),
            (Number, "2"),
            (Number, "3"),
            (Punct, "."),
            (Word, "x"),
            (Word, "e5"),
        ]
    );
}

#[test]
fn strings_comments_and_quoted_names_are_opaque() {
    assert_eq!(
        spelled("a 'b;' \"c\" `d` -- e\n/* f */ $t$ g $t$", Lexer::POSTGRES)
            .into_iter()
            .filter(|(kind, _)| matches!(kind, Opaque(_)))
            .collect::<Vec<_>>(),
        vec![
            (Opaque(OpaqueKind::SingleQuote), "'b;'"),
            (Opaque(OpaqueKind::DoubleQuote), "\"c\""),
            (Opaque(OpaqueKind::LineComment), "-- e"),
            (Opaque(OpaqueKind::BlockComment), "/* f */"),
            (Opaque(OpaqueKind::DollarQuote), "$t$ g $t$"),
        ]
    );
    // The backtick is an operator character in PostgreSQL, a quote elsewhere.
    assert_eq!(
        spelled("`d`", Lexer::MYSQL),
        vec![(Opaque(OpaqueKind::Backtick), "`d`")]
    );
    assert_eq!(
        spelled("`d`", Lexer::POSTGRES),
        vec![(Punct, "`"), (Word, "d"), (Punct, "`")]
    );
}

#[test]
fn a_prefix_belongs_to_its_string_and_the_lexer_decides_where_a_string_ends() {
    assert_eq!(
        spelled("E'a\\'b' u&'x'", Lexer::POSTGRES),
        vec![
            (Opaque(OpaqueKind::SingleQuote), "E'a\\'b'"),
            (Opaque(OpaqueKind::SingleQuote), "u&'x'"),
        ]
    );
    // The W3-T0 case: `'\''` is one string in MySQL, so the `DELETE` after it is code there;
    // elsewhere it is a string and a quote that open a second string, which hides the `DELETE`.
    let sql = "SELECT '\\''; DELETE FROM t; -- '";
    let is_delete = |(kind, text): &(TokenKind, &str)| *kind == Word && *text == "DELETE";
    let mysql = spelled(sql, Lexer::MYSQL);
    assert!(mysql.contains(&(Opaque(OpaqueKind::SingleQuote), "'\\''")));
    assert!(mysql.iter().any(is_delete));
    assert!(!spelled(sql, Lexer::GENERIC).iter().any(is_delete));
}

#[test]
fn an_opener_without_a_closer_runs_to_the_end_of_the_range() {
    assert_eq!(
        spelled("select 'abc", Lexer::GENERIC),
        vec![(Word, "select"), (Opaque(OpaqueKind::SingleQuote), "'abc")]
    );
    assert_eq!(
        spelled("x /* open", Lexer::GENERIC),
        vec![(Word, "x"), (Opaque(OpaqueKind::BlockComment), "/* open")]
    );
    // A range cuts a region short: what lies past the range is not read.
    let sql = "select 'abc' from t";
    let mut found = Vec::new();
    lex(sql.as_bytes(), 0..11, Lexer::GENERIC, |t| {
        found.push((t.kind, &sql[t.start..t.end]))
    });
    assert_eq!(
        found,
        vec![(Word, "select"), (Opaque(OpaqueKind::SingleQuote), "'abc")]
    );
}

#[test]
fn a_range_is_a_text_of_its_own() {
    let sql = "a 'x y' b";
    // Starting inside the string: its opener is not seen, so `y'` reads as a word and a quote.
    let mut found = Vec::new();
    lex(sql.as_bytes(), 5..sql.len(), Lexer::GENERIC, |t| {
        found.push((t.kind, &sql[t.start..t.end]))
    });
    assert_eq!(
        found,
        vec![(Word, "y"), (Opaque(OpaqueKind::SingleQuote), "' b")]
    );
    // Out-of-range and reversed ranges are clamped, not a panic.
    lex(sql.as_bytes(), 3..1_000, Lexer::GENERIC, |_| {});
    lex(sql.as_bytes(), 1_000..2_000, Lexer::GENERIC, |_| {});
    #[allow(clippy::reversed_empty_ranges)]
    lex(sql.as_bytes(), 5..2, Lexer::GENERIC, |_| {});
}

/// SplitMix64, as in `scan_refactor.rs`.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const ALPHABET: &[&str] = &[
    "'", "''", "\"", "\"\"", "`", "--", "-- ", "/*", "*/", "/*!", "/*!50000", "$", "$$", "$tag$",
    "$1", ";", "\n", "\r", "\\", "#", "_", ":", "::", ":n", "E'", "b", "select", "delete", "t",
    "1", "42", ".", "5", "é", "😀", " ", "\t", "&", "-", "*", "/", "(", ")", ",", "=",
];

fn random_sql(rng: &mut Rng) -> String {
    let pieces = rng.below(41);
    (0..pieces)
        .map(|_| ALPHABET[rng.below(ALPHABET.len())])
        .collect()
}

#[test]
fn random_inputs_hold_the_lexer_invariants() {
    let lexers = all_lexers();
    let mut rng = Rng(0x01E7_C0DE);
    for _ in 0..10_000 {
        let sql = random_sql(&mut rng);
        for &lexer in &lexers {
            let found = tokens(&sql, lexer);

            // In order, inside the text, no overlap, none empty.
            let mut previous_end = 0;
            for token in &found {
                assert!(
                    token.start >= previous_end
                        && token.end > token.start
                        && token.end <= sql.len(),
                    "bad token {token:?} in {sql:?} under {lexer:?}"
                );
                previous_end = token.end;
            }

            // The opaque tokens are exactly `walk`'s regions.
            let mut regions = Regions::default();
            let end_state = walk(sql.as_bytes(), lexer, &mut regions);
            let opaque: Vec<_> = found
                .iter()
                .filter_map(|t| match t.kind {
                    Opaque(kind) => Some((kind, t.start, t.end)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                opaque, regions.0,
                "opaque regions differ for {sql:?} under {lexer:?}"
            );
            // A region left open is the last one and ends at the end of the text.
            if let EndState::Open(kind) = end_state {
                assert_eq!(
                    regions.0.last().map(|r| (r.0, r.2)),
                    Some((kind, sql.len()))
                );
            }

            // The words are the words `scan` collects (a `:name` counts by its name).
            let words: Vec<String> = found
                .iter()
                .filter_map(|t| match t.kind {
                    Word => Some(sql[t.start..t.end].to_ascii_uppercase()),
                    Param => Some(sql[t.start + 1..t.end].to_ascii_uppercase()),
                    _ => None,
                })
                .collect();
            assert_eq!(
                words,
                scan_dialect(&sql, lexer).keywords,
                "words differ for {sql:?} under {lexer:?}"
            );

            // A sub-range lexes as the text on its own, shifted.
            let cut = sql.len() / 2;
            if sql.is_char_boundary(cut) {
                let tail = &sql[cut..];
                let mut shifted = Vec::new();
                lex(sql.as_bytes(), cut..sql.len(), lexer, |t| shifted.push(t));
                let alone: Vec<Token> = tokens(tail, lexer)
                    .into_iter()
                    .map(|t| Token {
                        start: t.start + cut,
                        end: t.end + cut,
                        kind: t.kind,
                    })
                    .collect();
                assert_eq!(
                    shifted, alone,
                    "range differs from text alone: {sql:?} from {cut}"
                );
            }
        }
    }
}
