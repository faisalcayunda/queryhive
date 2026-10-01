//! Colour classes for the tokens of one statement.
//!
//! The classes come from three places that never overlap. The scanner's own reading of the
//! statement (`qh_sql::walk`) decides what is a string, a quoted identifier or a comment, so the
//! editor colours what Safe Mode and the server see even where the grammar disagrees (MySQL's
//! `'it\'s'`, a `#` comment). The tree gives every other leaf a class. Every stretch of text
//! neither covers (the gaps inside and around ERROR nodes, hidden tokens such as the word
//! `date` in a typed literal, or the whole statement when there is no tree) is lexed by
//! `qh_sql::lex`. Last, `:name` parameters are laid over the result by the same rule
//! `SQLScanner` uses to bind them, so the colour says what Run will do.
//!
//! Offsets are bytes from the start of the statement.

use std::ops::Range;

use qh_sql::{walk, Dialect, Lexer, OpaqueKind, TokenKind, Visitor};
use tree_sitter::{Node, Tree};

use crate::keywords::is_keyword;

/// What a token is, for colouring. The numbers are the FFI's; they never change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Class {
    Comment = 1,
    String = 2,
    QuotedIdentifier = 3,
    Number = 4,
    Keyword = 5,
    Literal = 6,
    Function = 7,
    Punctuation = 8,
    Parameter = 9,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::Comment => "comment",
            Class::String => "string",
            Class::QuotedIdentifier => "quotedIdentifier",
            Class::Number => "number",
            Class::Keyword => "keyword",
            Class::Literal => "literal",
            Class::Function => "function",
            Class::Punctuation => "punctuation",
            Class::Parameter => "parameter",
        }
    }
}

/// A classified byte range `[start, end)` of a statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tok {
    pub start: u32,
    pub end: u32,
    pub class: Class,
}

/// The tokens of `text`, one statement, in order and disjoint, with touching tokens of one
/// class merged. Words with no class (names, mostly) produce no token.
pub fn classify(text: &str, tree: Option<&Tree>, dialect: Dialect) -> Vec<Tok> {
    let bytes = text.as_bytes();
    let Some(tree) = tree else {
        return lex_statement(bytes, dialect);
    };
    let lexer = Lexer::from(dialect);
    let regions = opaque_regions(bytes, lexer);
    let mut out = Vec::new();
    from_tree(tree, bytes, dialect, lexer, &regions, &mut out);
    overlay_parameters(bytes, &regions, &mut out);
    coalesce(&mut out);
    out
}

/// Colour a statement with the lexer alone, as one too big to parse is coloured: one walk gives
/// the tokens and the regions the parameters are found by. `bytes` is the statement, or its
/// start up to some point (a token running past the end is cut there).
pub fn lex_statement(bytes: &[u8], dialect: Dialect) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut regions = Vec::new();
    lex_regions(
        bytes,
        0..bytes.len(),
        dialect,
        Lexer::from(dialect),
        &mut out,
        &mut |region| regions.push(region),
    );
    overlay_parameters(bytes, &regions, &mut out);
    coalesce(&mut out);
    out
}

/// The strings, quoted identifiers, comments and dollar quotes of a statement, as `walk` reads
/// them under `lexer`: `(kind, start, end)`, in order, not nested.
fn opaque_regions(bytes: &[u8], lexer: Lexer) -> Vec<(OpaqueKind, usize, usize)> {
    struct Regions(Vec<(OpaqueKind, usize, usize)>);
    impl Visitor for Regions {
        fn opaque(&mut self, kind: OpaqueKind, start: usize, end: usize) {
            self.0.push((kind, start, end));
        }
    }
    let mut regions = Regions(Vec::new());
    walk(bytes, lexer, &mut regions);
    regions.0
}

fn region_class(kind: OpaqueKind, dialect: Dialect) -> Class {
    match kind {
        OpaqueKind::SingleQuote | OpaqueKind::DollarQuote => Class::String,
        OpaqueKind::DoubleQuote if dialect == Dialect::Mysql => Class::String,
        OpaqueKind::DoubleQuote | OpaqueKind::Backtick => Class::QuotedIdentifier,
        OpaqueKind::LineComment | OpaqueKind::BlockComment => Class::Comment,
    }
}

fn push(out: &mut Vec<Tok>, start: usize, end: usize, class: Class) {
    if end > start {
        out.push(Tok {
            start: start as u32,
            end: end as u32,
            class,
        });
    }
}

/// The class of a bare word: a keyword, a value keyword, or nothing.
fn word_class(word: &[u8]) -> Option<Class> {
    if word.eq_ignore_ascii_case(b"null")
        || word.eq_ignore_ascii_case(b"true")
        || word.eq_ignore_ascii_case(b"false")
    {
        Some(Class::Literal)
    } else if is_keyword(word) {
        Some(Class::Keyword)
    } else {
        None
    }
}

fn lex_into(bytes: &[u8], range: Range<usize>, dialect: Dialect, lexer: Lexer, out: &mut Vec<Tok>) {
    lex_regions(bytes, range, dialect, lexer, out, &mut |_| {});
}

/// [`lex_into`] that also reports each opaque region `(kind, start, end)` it meets.
fn lex_regions(
    bytes: &[u8],
    range: Range<usize>,
    dialect: Dialect,
    lexer: Lexer,
    out: &mut Vec<Tok>,
    on_region: &mut impl FnMut((OpaqueKind, usize, usize)),
) {
    qh_sql::lex(bytes, range, lexer, |token| {
        let (start, end) = (token.start, token.end);
        match token.kind {
            TokenKind::Opaque(kind) => {
                on_region((kind, start, end));
                push(out, start, end, region_class(kind, dialect));
            }
            TokenKind::Word => {
                if let Some(class) = word_class(&bytes[start..end]) {
                    push(out, start, end, class);
                }
            }
            TokenKind::Number => push(out, start, end, Class::Number),
            // Whether a `:name` is a parameter is decided later, for the whole statement.
            TokenKind::Param => {
                push(out, start, start + 1, Class::Punctuation);
                if let Some(class) = word_class(&bytes[start + 1..end]) {
                    push(out, start + 1, end, class);
                }
            }
            TokenKind::Punct => push(out, start, end, Class::Punctuation),
        }
    });
}

/// What to do with one tree node.
enum Leaf {
    /// Colour it and do not look inside.
    Emit(Option<Class>),
    /// Look at its children.
    Descend,
    /// Leave it to the lexer.
    Gap,
}

/// A stretch of the statement that is already classified: a tree leaf or a region.
struct Item {
    start: usize,
    end: usize,
    class: Option<Class>,
}

fn from_tree(
    tree: &Tree,
    bytes: &[u8],
    dialect: Dialect,
    lexer: Lexer,
    regions: &[(OpaqueKind, usize, usize)],
    out: &mut Vec<Tok>,
) {
    // Dollar quotes are the tree's to split into tag and body; every other region is the
    // scanner's, and a leaf that touches one is dropped.
    let regions: Vec<Item> = regions
        .iter()
        .filter(|r| r.0 != OpaqueKind::DollarQuote)
        .map(|&(kind, start, end)| Item {
            start,
            end,
            class: Some(region_class(kind, dialect)),
        })
        .collect();

    let mut leaves: Vec<Item> = Vec::new();
    let mut cursor = tree.walk();
    // Kinds of the ancestors, nearest last, for the two rules that need a parent.
    let mut parents: Vec<&'static str> = Vec::new();
    'tree: loop {
        let node = cursor.node();
        let mut descend = false;
        match visit(node, bytes, dialect, &parents) {
            Leaf::Descend => descend = node.child_count() > 0,
            Leaf::Emit(class) => leaves.push(Item {
                start: node.start_byte(),
                end: node.end_byte(),
                class,
            }),
            Leaf::Gap => {}
        }
        if descend && cursor.goto_first_child() {
            parents.push(node.kind());
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                break 'tree;
            }
            parents.pop();
        }
    }

    // Both lists are ascending; merge them, the regions winning any overlap.
    let mut at = 0usize;
    let mut region = 0usize;
    let emit = |item: &Item, at: &mut usize, out: &mut Vec<Tok>| {
        if item.start < *at {
            return;
        }
        if item.start > *at {
            lex_into(bytes, *at..item.start, dialect, lexer, out);
        }
        if let Some(class) = item.class {
            push(out, item.start, item.end, class);
        }
        *at = item.end;
    };
    for leaf in &leaves {
        while region < regions.len() && regions[region].end <= leaf.start {
            emit(&regions[region], &mut at, out);
            region += 1;
        }
        if regions.get(region).is_some_and(|r| r.start < leaf.end) {
            continue;
        }
        emit(leaf, &mut at, out);
    }
    for item in &regions[region..] {
        emit(item, &mut at, out);
    }
    if at < bytes.len() {
        lex_into(bytes, at..bytes.len(), dialect, lexer, out);
    }
}

fn visit(node: Node, bytes: &[u8], dialect: Dialect, parents: &[&'static str]) -> Leaf {
    if node.is_missing() || node.start_byte() == node.end_byte() {
        return Leaf::Gap;
    }
    let kind = node.kind();
    match kind {
        "comment" | "marginalia" => Leaf::Emit(Some(Class::Comment)),
        "dollar_quote" => Leaf::Emit(Some(Class::String)),
        "parameter" => Leaf::Emit(Some(Class::Parameter)),
        "op_other" | "op_unary_other" => Leaf::Emit(Some(Class::Punctuation)),
        "identifier" => Leaf::Emit(identifier_class(node, bytes, dialect, parents)),
        "literal" => literal(node, bytes, dialect),
        _ if kind.starts_with("keyword_") => Leaf::Emit(Some(match kind {
            "keyword_null" | "keyword_true" | "keyword_false" => Class::Literal,
            _ => Class::Keyword,
        })),
        _ if node.child_count() == 0 => {
            if node.is_named() {
                Leaf::Gap
            } else {
                let text = &bytes[node.start_byte()..node.end_byte()];
                if text[0].is_ascii_alphanumeric() || text[0] == b'_' {
                    Leaf::Emit(word_class(text))
                } else {
                    Leaf::Emit(Some(Class::Punctuation))
                }
            }
        }
        _ => Leaf::Descend,
    }
}

fn identifier_class(
    node: Node,
    bytes: &[u8],
    dialect: Dialect,
    parents: &[&'static str],
) -> Option<Class> {
    match bytes[node.start_byte()] {
        b'"' if dialect == Dialect::Mysql => return Some(Class::String),
        b'"' | b'`' => return Some(Class::QuotedIdentifier),
        _ => {}
    }
    match parents {
        // `date '2020-01-01'`: the type word of a typed literal.
        [.., "literal"] => Some(Class::Keyword),
        // `schema.func(`: the last name of a reference the call is made on.
        [.., "invocation", "object_reference"] if node.next_named_sibling().is_none() => {
            Some(Class::Function)
        }
        _ => None,
    }
}

fn literal(node: Node, bytes: &[u8], dialect: Dialect) -> Leaf {
    let mut walk = node.walk();
    let has_parts = node.children(&mut walk).any(|child| {
        child.is_named() && (child.kind() == "identifier" || child.kind().starts_with("keyword_"))
    });
    if has_parts {
        // `null`, `true`, `date '…'`: the parts are classified one by one.
        return Leaf::Descend;
    }
    Leaf::Emit(Some(match bytes[node.start_byte()] {
        b'"' if dialect == Dialect::Mysql => Class::String,
        b'"' => Class::QuotedIdentifier,
        b'0'..=b'9' | b'.' | b'+' | b'-' => Class::Number,
        // Quoted strings, prefixed strings (`E'…'`, `U&'…'`, `X'…'`) and `$$…$$`.
        _ => Class::String,
    }))
}

/// The byte ranges of the `:name` parameters of a statement, by `SQLScanner`'s rule: a colon in
/// code (not in a string, quoted identifier, comment or dollar quote), outside `[…]`, not
/// after another colon, followed by a letter or `_`.
fn parameter_spans(bytes: &[u8], regions: &[(OpaqueKind, usize, usize)]) -> Vec<(usize, usize)> {
    if !bytes.contains(&b':') {
        return Vec::new();
    }
    let name_start = |byte: u8| byte.is_ascii_alphabetic() || byte == b'_';
    let mut spans = Vec::new();
    let (mut at, mut region, mut depth) = (0, 0, 0usize);
    while at < bytes.len() {
        while region < regions.len() && regions[region].2 <= at {
            region += 1;
        }
        if region < regions.len() && regions[region].1 <= at {
            at = regions[region].2;
            continue;
        }
        match bytes[at] {
            b'[' => depth += 1,
            b']' => depth = depth.saturating_sub(1),
            b':' if depth == 0
                && bytes.get(at + 1).copied().is_some_and(name_start)
                && !(at > 0 && bytes[at - 1] == b':') =>
            {
                let mut end = at + 2;
                while end < bytes.len() && (name_start(bytes[end]) || bytes[end].is_ascii_digit()) {
                    end += 1;
                }
                spans.push((at, end));
                at = end;
                continue;
            }
            _ => {}
        }
        at += 1;
    }
    spans
}

/// Replace whatever `tokens` say about the parameter spans with one `Parameter` token each.
fn overlay_parameters(bytes: &[u8], regions: &[(OpaqueKind, usize, usize)], tokens: &mut Vec<Tok>) {
    let spans = parameter_spans(bytes, regions);
    if spans.is_empty() {
        return;
    }
    let mut out = Vec::with_capacity(tokens.len() + spans.len());
    let mut next = 0;
    // Where the last parameter pushed ends; a token may begin inside it.
    let mut covered = 0;
    for token in tokens.drain(..) {
        let mut at = (token.start as usize).max(covered);
        let end = token.end as usize;
        while let Some(&(start, stop)) = spans.get(next).filter(|span| span.0 < end) {
            if start > at {
                push(&mut out, at, start, token.class);
            }
            push(&mut out, start, stop, Class::Parameter);
            next += 1;
            at = at.max(stop);
            covered = stop;
        }
        push(&mut out, at, end, token.class);
    }
    for &(start, stop) in &spans[next..] {
        push(&mut out, start, stop, Class::Parameter);
    }
    *tokens = out;
}

/// Merge tokens that touch and share a class.
fn coalesce(tokens: &mut Vec<Tok>) {
    tokens.dedup_by(|next, prev| {
        if prev.end == next.start && prev.class == next.class {
            prev.end = next.end;
            true
        } else {
            false
        }
    });
}
