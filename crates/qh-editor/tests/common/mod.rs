//! Shared by the integration tests: a seeded generator, the invariants every paint obeys, and
//! the dialect a fixture file is read under.
#![allow(dead_code)]

use qh_editor::{Dialect, Paint};

/// SplitMix64: enough randomness for edit sequences, and no dependency.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n` (n > 0).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

pub const DIALECTS: [Dialect; 4] = [
    Dialect::Generic,
    Dialect::Postgres,
    Dialect::Mysql,
    Dialect::Trino,
];

/// The dialect a fixture is read under, from the start of its name.
pub fn dialect_of(name: &str) -> Dialect {
    match name.split(['-', '.']).next() {
        Some("pg") => Dialect::Postgres,
        Some("mysql") => Dialect::Mysql,
        Some("trino") => Dialect::Trino,
        _ => Dialect::Generic,
    }
}

/// What must hold of every paint, whatever the text: the lists have the right shape, ascend,
/// stay inside the text, and every run and font lies inside one range.
pub fn validate(paint: &Paint) {
    let len = paint.doc_len_utf16;
    assert_eq!(paint.ranges.len() % 2, 0);
    assert_eq!(paint.runs.len() % 3, 0);
    assert_eq!(paint.fonts.len() % 3, 0);
    let ranges: Vec<(u32, u32)> = paint
        .ranges
        .chunks(2)
        .map(|r| (r[0], r[0] + r[1]))
        .collect();
    let mut end = 0;
    for &(a, b) in &ranges {
        assert!(a >= end && a < b && b <= len, "ranges {ranges:?} in {len}");
        end = b;
    }
    let inside = |a: u32, b: u32| ranges.iter().any(|&(ra, rb)| ra <= a && b <= rb);
    let mut end = 0;
    for run in paint.runs.chunks(3) {
        let (a, b) = (run[0], run[0] + run[1]);
        assert!(
            a >= end && a < b,
            "runs are ascending and not empty: {:?}",
            paint.runs
        );
        assert!((1..=9).contains(&run[2]), "class {}", run[2]);
        assert!(
            inside(a, b),
            "run {a}..{b} is inside no range of {ranges:?}"
        );
        end = b;
    }
    let mut end = 0;
    for font in paint.fonts.chunks(3) {
        let (a, b) = (font[0], font[0] + font[1]);
        assert!(a >= end && a < b && font[2] <= 1);
        assert!(
            inside(a, b),
            "font {a}..{b} is inside no range of {ranges:?}"
        );
        end = b;
    }
    if paint.inactive {
        assert!(paint.runs.is_empty());
    }
}
