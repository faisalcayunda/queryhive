//! A lexer for the code between the parts a parser understands: it colours and splits SQL
//! that has no tree, such as the gaps a tree-sitter parse marks as errors and a statement too
//! big to parse.
//!
//! It is [`walk`] plus what `walk` skips. The opaque regions (strings, quoted identifiers,
//! comments, dollar quotes) and the bare words come straight from `walk`, so this lexer reads
//! a quote or a comment exactly as the engine's Safe Mode does. The code between them is
//! cut into numbers, `:name` parameters and runs of punctuation.
//!
//! Words carry no class. Which words are keywords is the editor's business, not this crate's
//! (`qh-sql` does not know the editor's keyword list).

use std::ops::Range;

use crate::scan::{walk, Lexer, OpaqueKind, Visitor};

/// What a [`Token`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// A string, quoted identifier, comment or dollar quote, as `walk` reports it. One that
    /// is never closed runs to the end of the range.
    Opaque(OpaqueKind),
    /// A bare word: a keyword, a name, anything that starts with a letter or `_`.
    Word,
    /// A number: digits with an optional fraction. Digits glued to a letter (`1abc`, `1e5`) are
    /// not one: the letters are a [`TokenKind::Word`] and the digits get no token.
    Number,
    /// A `:name` parameter, colon included. A `::` cast is not one.
    Param,
    /// A run of ASCII punctuation and operator characters.
    Punct,
}

/// One token: the byte range `[start, end)` in the text `lex` was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

/// Lex `bytes[range]` as if it were a text of its own, under one lexer's rules, calling
/// `emit` for each token in order. Token offsets are into `bytes`.
///
/// "As a text of its own" is the point: a quote opened inside the range and not closed
/// inside it is a token that ends at the range's end, and a quote opened before the range
/// is not seen. That is what a caller lexing the gap between two parse nodes wants.
/// Whitespace, control bytes and bytes at or above 0x80 that are not part of a word produce
/// no token. A range outside `bytes` is clamped, so this never panics.
pub fn lex(
    bytes: &[u8],
    range: Range<usize>,
    dialect: impl Into<Lexer>,
    mut emit: impl FnMut(Token),
) {
    let end = range.end.min(bytes.len());
    let start = range.start.min(end);
    let text = &bytes[start..end];
    let mut lexing = Lexing {
        text,
        base: start,
        cursor: 0,
        emit: &mut emit,
    };
    walk(text, dialect, &mut lexing);
    lexing.code(text.len());
}

/// The state of one [`lex`]: the events `walk` reports cut the text into opaque regions and
/// words, and what lies between them (`cursor` up to the next event) is code to cut here.
struct Lexing<'a, F: FnMut(Token)> {
    text: &'a [u8],
    /// Offset of `text` in the caller's bytes.
    base: usize,
    /// Everything before this offset (in `text`) has been handled.
    cursor: usize,
    emit: &'a mut F,
}

impl<F: FnMut(Token)> Lexing<'_, F> {
    fn token(&mut self, start: usize, end: usize, kind: TokenKind) {
        (self.emit)(Token {
            start: self.base + start,
            end: self.base + end,
            kind,
        });
    }

    /// Cut the code in `[cursor, to)` into numbers and punctuation, and move the cursor.
    fn code(&mut self, to: usize) {
        let mut at = self.cursor;
        while at < to {
            let byte = self.text[at];
            if byte.is_ascii_digit() {
                let end = number_end(&self.text[..to], at);
                // A digit run glued to a letter (`1abc`, `1e5`) is not a number: the letters
                // are the word `walk` reports next.
                let glued = self
                    .text
                    .get(end)
                    .is_some_and(|next| next.is_ascii_alphabetic() || *next == b'_');
                if !glued {
                    self.token(at, end, TokenKind::Number);
                }
                at = end;
            } else if byte.is_ascii_punctuation() {
                let mut end = at + 1;
                while end < to && self.text[end].is_ascii_punctuation() {
                    end += 1;
                }
                self.token(at, end, TokenKind::Punct);
                at = end;
            } else {
                at += 1;
            }
        }
        self.cursor = to.max(self.cursor);
    }
}

impl<F: FnMut(Token)> Visitor for Lexing<'_, F> {
    fn opaque(&mut self, kind: OpaqueKind, start: usize, end: usize) {
        self.code(start);
        self.token(start, end, TokenKind::Opaque(kind));
        self.cursor = end;
    }

    fn word(&mut self, start: usize, end: usize) {
        // `:name`, but not `::name` and not `a:name`: the colon must not follow a word
        // character or another colon.
        let param = start > self.cursor
            && self.text[start - 1] == b':'
            && (start < 2
                || !(self.text[start - 2].is_ascii_alphanumeric()
                    || self.text[start - 2] == b'_'
                    || self.text[start - 2] == b':'));
        if param {
            self.code(start - 1);
            self.token(start - 1, end, TokenKind::Param);
        } else {
            self.code(start);
            self.token(start, end, TokenKind::Word);
        }
        self.cursor = end;
    }
}

/// The end of the number that starts at `at`: digits, and `.digits` after them. `text` ends
/// where the code between two events does.
fn number_end(text: &[u8], at: usize) -> usize {
    let digits = |mut from: usize| {
        while text.get(from).is_some_and(u8::is_ascii_digit) {
            from += 1;
        }
        from
    };
    let end = digits(at);
    if text.get(end) == Some(&b'.') && text.get(end + 1).is_some_and(u8::is_ascii_digit) {
        return digits(end + 1);
    }
    end
}
