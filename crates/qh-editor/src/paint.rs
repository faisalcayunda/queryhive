//! The analysis of one document: which statements it has, what colour each token is, and what
//! the UI still has to be told.
//!
//! **What the UI is told.** The UI keeps colours as temporary attributes. It repaints only the
//! ranges in `dirty`: everything an edit or a change of colour may have made wrong. An edit
//! adds its own range (the new text has no colour), and when a statement's tokens are worked
//! out again the difference from the old tokens is added. [`Analyzer::paint`] hands back the
//! part of `dirty` inside a window, cut to a budget and snapped to token boundaries, together
//! with the runs to draw in it; the UI reports what it applied ([`LogEntry::Applied`]) and that
//! part leaves `dirty`. `fonts` is the same for the italic of comments, which the UI keeps in
//! the text storage because a temporary attribute cannot carry a font.
//!
//! **Two threads, one log.** The main thread owns a [`TextBuffer`] and logs each edit with the
//! text it inserted. The analysis thread owns an [`Analyzer`] whose mirror of the text replays
//! that log, so analysis always sees the text as it was after each edit, and the main thread
//! never copies the text for it.

use qh_sql::{Dialect, Lexer};

use crate::classify::{self, Class, Tok};
use crate::folds::{self, Fold, RelFold};
use crate::issues::{self, Issue, RelIssue};
use crate::statements::Statements;
use crate::syntax::{self, Syntax};
use crate::text::{EditError, LogEntry, TextBuffer};
use crate::{CEILING_UTF16, TREE_CACHE_BYTES};

/// Sorted, disjoint, non-touching ranges `[start, end)` of UTF-16 offsets.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RangeSet(Vec<(u32, u32)>);

impl RangeSet {
    fn whole(len: u32) -> RangeSet {
        RangeSet(if len > 0 { vec![(0, len)] } else { Vec::new() })
    }

    fn add(&mut self, start: u32, end: u32) {
        if start >= end {
            return;
        }
        let first = self.0.partition_point(|r| r.1 < start);
        let (mut new_start, mut new_end) = (start, end);
        let mut last = first;
        while last < self.0.len() && self.0[last].0 <= new_end {
            new_start = new_start.min(self.0[last].0);
            new_end = new_end.max(self.0[last].1);
            last += 1;
        }
        self.0.splice(first..last, [(new_start, new_end)]);
    }

    fn subtract(&mut self, start: u32, end: u32) {
        let mut out = Vec::with_capacity(self.0.len() + 1);
        for &(s, e) in &self.0 {
            if e <= start || s >= end {
                out.push((s, e));
                continue;
            }
            if s < start {
                out.push((s, start));
            }
            if e > end {
                out.push((end, e));
            }
        }
        self.0 = out;
    }

    /// Move the ranges through an edit that replaced `old_len` units at `start` with `new_len`:
    /// what was inside the edit is gone, and the new text is not added (that is the caller's).
    fn edit(&mut self, start: u32, old_len: u32, new_len: u32) {
        let old_end = start + old_len;
        let delta = i64::from(new_len) - i64::from(old_len);
        let mut out: Vec<(u32, u32)> = Vec::with_capacity(self.0.len() + 1);
        let mut push = |s: u32, e: u32| match out.last_mut() {
            Some(last) if last.1 >= s => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        };
        for &(s, e) in &self.0 {
            if s < start {
                push(s, e.min(start));
            }
            if e > old_end {
                push(
                    (i64::from(s.max(old_end)) + delta) as u32,
                    (i64::from(e) + delta) as u32,
                );
            }
        }
        self.0 = out;
    }

    fn intersect(&self, start: u32, end: u32) -> Vec<(u32, u32)> {
        let first = self.0.partition_point(|r| r.1 <= start);
        self.0[first..]
            .iter()
            .take_while(|r| r.0 < end)
            .map(|&(s, e)| (s.max(start), e.min(end)))
            .collect()
    }

    /// Whether any part lies outside `[start, end)`.
    fn outside(&self, start: u32, end: u32) -> bool {
        self.0.iter().any(|&(s, e)| s < start || e > end)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The parts of `a` that `b` does not cover; both sorted and disjoint.
fn minus(a: &[(u32, u32)], b: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for &(s, e) in a {
        let mut at = s;
        for &(bs, be) in b.iter().skip_while(|r| r.1 <= s) {
            if bs >= e {
                break;
            }
            if bs > at {
                out.push((at, bs));
            }
            at = at.max(be);
        }
        if at < e {
            out.push((at, e));
        }
    }
    out
}

/// Where two token lists differ: the byte span (from the start of the statement) and the byte
/// ranges of it that are a comment in one list only.
struct Difference {
    span: (u32, u32),
    comments: Vec<(u32, u32)>,
}

/// Where two token lists differ, after trimming what they share at both ends: the covered span,
/// and the parts of it that are a comment in one list only (their italic changes).
fn diff(old: &[Tok], new: &[Tok]) -> Option<Difference> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let room = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    let (old, new) = (
        &old[prefix..old.len() - suffix],
        &new[prefix..new.len() - suffix],
    );
    let start = old
        .first()
        .map(|t| t.start)
        .into_iter()
        .chain(new.first().map(|t| t.start))
        .min()?;
    let end = old
        .last()
        .map(|t| t.end)
        .into_iter()
        .chain(new.last().map(|t| t.end))
        .max()?;
    let comments = |tokens: &[Tok]| -> Vec<(u32, u32)> {
        tokens
            .iter()
            .filter(|t| t.class == Class::Comment)
            .map(|t| (t.start, t.end))
            .collect()
    };
    let (old, new) = (comments(old), comments(new));
    let mut changed = minus(&old, &new);
    changed.extend(minus(&new, &old));
    changed.sort_unstable();
    Some(Difference {
        span: (start, end),
        comments: changed,
    })
}

/// Move tokens through an edit of `[start, old_end)` to `new_len` bytes, the way the UI moves
/// its colours: what the edit replaced is gone, the rest of a token it cut keeps its class, and
/// what follows shifts. The result is what is on screen, which is what a new list is compared
/// with. (Merging the two halves of a token cut in the middle is part of that: the new list
/// has one token there.)
fn map_tokens(tokens: &mut Vec<Tok>, start: usize, old_end: usize, new_len: usize) {
    let delta = new_len as i64 - (old_end - start) as i64;
    let mut out: Vec<Tok> = Vec::with_capacity(tokens.len() + 1);
    let mut push = |start: i64, end: i64, class: Class| {
        if end > start {
            match out.last_mut() {
                Some(last) if i64::from(last.end) == start && last.class == class => {
                    last.end = end as u32
                }
                _ => out.push(Tok {
                    start: start as u32,
                    end: end as u32,
                    class,
                }),
            }
        }
    };
    for token in tokens.iter() {
        let (s, e) = (i64::from(token.start), i64::from(token.end));
        push(s, e.min(start as i64), token.class);
        push(s.max(old_end as i64) + delta, e + delta, token.class);
    }
    *tokens = out;
}

/// What to draw for a window.
///
/// The three lists are flat: `ranges` is `(start, len)` pairs, `runs` is `(start, len, class)`
/// triples with `class` 1 to 9, and `fonts` is `(start, len, italic)` triples. All offsets are
/// UTF-16 units, ascending and disjoint, and every run and font lies inside one range.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Paint {
    pub revision: u64,
    pub doc_len_utf16: u32,
    pub window_start: u32,
    pub window_len: u32,
    /// The document is over the ceiling: no runs, and the ranges go back to the base colour.
    pub inactive: bool,
    /// The budget ran out before the window was done.
    pub more_in_window: bool,
    /// Something outside the window needs repainting.
    pub dirty_elsewhere: bool,
    pub ranges: Vec<u32>,
    pub runs: Vec<u32>,
    pub fonts: Vec<u32>,
}

/// The parts of a document that change more slowly than a keystroke.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outline {
    pub revision: u64,
    /// `(start, end)` pairs of the statements the UI shows and Run splits: the same pieces as
    /// `qh_sql::statements_with_lines`, without their `;`.
    pub statements: Vec<u32>,
    pub folds: Vec<Fold>,
    pub issues: Vec<Issue>,
}

/// Sizes, for tests and the bench.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub statements: usize,
    pub giant: usize,
    pub trees: usize,
    /// Source bytes the held trees cover.
    pub tree_source_bytes: usize,
    pub tokens: usize,
}

/// The analysis half of a document: it holds its own copy of the text and follows the log.
pub struct Analyzer {
    dialect: Dialect,
    lexer: Lexer,
    mirror: TextBuffer,
    stmts: Statements,
    syntax: Syntax,
    dirty: RangeSet,
    fonts: RangeSet,
    revision: u64,
    tree_bytes: usize,
    tick: u64,
    inactive: bool,
}

impl Analyzer {
    /// An analyzer for `text`, all of which needs painting.
    pub fn new(text: &str, dialect: Dialect) -> Result<Analyzer, EditError> {
        let mirror = TextBuffer::new(text)?;
        let lexer = Lexer::from(dialect);
        let len = mirror.len_utf16();
        Ok(Analyzer {
            dialect,
            lexer,
            stmts: Statements::new(mirror.as_str(), lexer),
            mirror,
            syntax: Syntax::new(),
            dirty: RangeSet::whole(len),
            fonts: RangeSet::whole(len),
            revision: 1,
            tree_bytes: 0,
            tick: 0,
            inactive: len > CEILING_UTF16,
        })
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn stats(&self) -> Stats {
        let len = self.mirror.len_bytes();
        Stats {
            statements: self.stmts.len(),
            giant: (0..self.stmts.len())
                .filter(|&i| self.stmts.is_giant(i, len))
                .count(),
            trees: self.stmts.list.iter().filter(|s| s.tree.is_some()).count(),
            tree_source_bytes: self.tree_bytes,
            tokens: self
                .stmts
                .list
                .iter()
                .filter_map(|s| s.tokens.as_ref())
                .map(Vec::len)
                .sum(),
        }
    }

    /// The statement start offsets (bytes), for tests.
    pub fn statement_starts(&self) -> Vec<usize> {
        self.stmts.list.iter().map(|s| s.start).collect()
    }

    /// Follow the log. On an error the analyzer is out of step with the text and must be
    /// rebuilt from it.
    pub fn sync(&mut self, log: Vec<LogEntry>) -> Result<(), EditError> {
        for entry in log {
            match entry {
                LogEntry::Edit {
                    revision,
                    start,
                    len,
                    text,
                } => {
                    self.apply(start, len, &text)?;
                    self.revision = revision;
                }
                LogEntry::Touch { start, len } => {
                    let end = (start + len).min(self.mirror.len_utf16());
                    self.dirty.add(start, end);
                    self.fonts.add(start, end);
                }
                LogEntry::Applied { revision, ranges } if revision == self.revision => {
                    for pair in ranges.chunks_exact(2) {
                        self.dirty.subtract(pair[0], pair[0] + pair[1]);
                        self.fonts.subtract(pair[0], pair[0] + pair[1]);
                    }
                }
                LogEntry::Applied { .. } => {}
            }
        }
        Ok(())
    }

    fn apply(&mut self, start: u32, len: u32, new: &str) -> Result<(), EditError> {
        let from = self.mirror.byte_of(start)?;
        let to = self
            .mirror
            .byte_of(start.checked_add(len).ok_or(EditError::OutOfBounds)?)?;
        let new16 = new.encode_utf16().count() as u32;

        // The tree edit is worked out against the text as it was.
        let text_len = self.mirror.len_bytes();
        let held = self.stmts.index_of(from);
        let (held_start, held_end) = (
            self.stmts.list[held].start,
            self.stmts.end_of(held, text_len),
        );
        let tree_edit = (self.stmts.list[held].tree.is_some() && to <= held_end).then(|| {
            syntax::input_edit(
                &self.mirror.as_str()[held_start..held_end],
                from - held_start,
                to - held_start,
                new,
            )
        });

        self.mirror.edit(start, len, new)?;
        self.dirty.edit(start, len, new16);
        self.fonts.edit(start, len, new16);
        self.dirty.add(start, start + new16);
        self.fonts.add(start, start + new16);
        if new16 == 0 {
            // A deletion has no new text to repaint, but the statement it cut must still be
            // looked at (what is left of a word may not be a keyword any more), and only a
            // dirty range brings a paint to a statement. Mark a neighbouring character.
            let text = self.mirror.as_str();
            if let Some(ch) = text[from..].chars().next() {
                self.dirty.add(start, start + ch.len_utf16() as u32);
            } else if let Some(ch) = text[..from].chars().next_back() {
                self.dirty.add(start - ch.len_utf16() as u32, start);
            }
        }

        let change = self
            .stmts
            .apply_edit(self.mirror.as_str(), from, to - from, new.len());
        self.tree_bytes -= change.dropped_tree_cost;
        let text_len = self.mirror.len_bytes();
        if change.reused {
            let i = change.first;
            let giant = self.stmts.is_giant(i, text_len);
            let stmt_end16 = self.mirror.utf16_of(self.stmts.end_of(i, text_len));
            let stmt = &mut self.stmts.list[i];
            match (giant, stmt.tree.as_mut(), tree_edit) {
                (false, Some(tree), Some(edit)) => {
                    tree.edit(&edit);
                    stmt.tree_stale = true;
                }
                (_, Some(_), _) => {
                    self.tree_bytes -= stmt.tree_cost;
                    stmt.tree_cost = 0;
                    stmt.tree = None;
                    stmt.tree_stale = false;
                }
                _ => {}
            }
            stmt.outline = None;
            stmt.tokens_valid = false;
            if giant {
                // No cached tokens to compare with: everything from the edit to the end of
                // the statement may have changed colour (a quote that flipped, say).
                // ponytail: repaints the rest of a giant statement inside the window on every
                // keystroke; add a lex state checkpoint per 64 KiB if the bench says so.
                stmt.tokens = None;
                self.dirty.add(start, stmt_end16);
                self.fonts.add(start, stmt_end16);
            } else if let Some(tokens) = stmt.tokens.as_mut() {
                map_tokens(tokens, from - stmt.start, to - stmt.start, new.len());
            }
        } else if change.count > 0 {
            let last = change.first + change.count - 1;
            let a = self.mirror.utf16_of(self.stmts.list[change.first].start);
            let b = self.mirror.utf16_of(self.stmts.end_of(last, text_len));
            self.dirty.add(a, b);
            self.fonts.add(a, b);
        }
        Ok(())
    }

    /// Work out the tokens of statement `i` if they are not current, and add what changed to
    /// `dirty`. Returns whether it did.
    fn retokenize(&mut self, i: usize) -> bool {
        let text = self.mirror.as_str();
        let (start, end) = (self.stmts.list[i].start, self.stmts.end_of(i, text.len()));
        let giant = self.stmts.is_giant(i, text.len());
        let stmt = &mut self.stmts.list[i];
        if giant {
            self.tree_bytes -= stmt.tree_cost;
            stmt.tree_cost = 0;
            stmt.tree = None;
            stmt.tokens = None;
            stmt.tokens_valid = false;
            return false;
        }
        if stmt.tokens_valid && stmt.tokens.is_some() {
            return false;
        }
        let statement = &text[start..end];
        self.tick += 1;
        stmt.last_use = self.tick;
        self.tree_bytes -= stmt.tree_cost;
        self.syntax.ensure(stmt, statement);
        self.tree_bytes += stmt.tree_cost;
        let new = classify::classify(statement, stmt.tree.as_ref(), self.dialect);
        let old = stmt.tokens.replace(new);
        stmt.tokens_valid = true;
        if let Some(old) = old {
            let found = diff(&old, stmt.tokens.as_deref().unwrap_or(&[]));
            note_difference(&self.mirror, &mut self.dirty, &mut self.fonts, start, found);
        }
        true
    }

    /// Parse again from scratch every statement whose tree came from edits and holds an error,
    /// so that its colours, folds and issues no longer depend on how it was typed. Call it when
    /// typing pauses; the colours it changes are added to what needs painting.
    pub fn converge(&mut self) {
        let text = self.mirror.as_str();
        for i in 0..self.stmts.len() {
            let (start, end) = (self.stmts.list[i].start, self.stmts.end_of(i, text.len()));
            let stmt = &mut self.stmts.list[i];
            let Some(tree) = stmt.tree.as_ref() else {
                continue;
            };
            if !stmt.incremental || stmt.tree_stale || !tree.root_node().has_error() {
                continue;
            }
            let statement = &text[start..end];
            self.tree_bytes -= stmt.tree_cost;
            self.syntax.parse_fresh(stmt, statement);
            self.tree_bytes += stmt.tree_cost;
            stmt.outline = None;
            if stmt.tokens_valid {
                let new = classify::classify(statement, stmt.tree.as_ref(), self.dialect);
                let old = stmt.tokens.replace(new);
                let found = old.and_then(|old| diff(&old, stmt.tokens.as_deref().unwrap_or(&[])));
                note_difference(&self.mirror, &mut self.dirty, &mut self.fonts, start, found);
            }
        }
    }

    /// The tokens of statement `i` as offsets from its start. A statement too big to parse has
    /// none cached: they are lexed once per paint (kept in `memo`) from its start up to the byte
    /// `limit`, and a token running past it is cut there.
    fn tokens_of<'a>(
        &'a self,
        memo: &'a mut Vec<(usize, Vec<Tok>)>,
        i: usize,
        limit: usize,
    ) -> &'a [Tok] {
        let text = self.mirror.as_str();
        if !self.stmts.is_giant(i, text.len()) {
            return self.stmts.list[i].tokens.as_deref().unwrap_or(&[]);
        }
        let at = match memo.iter().position(|m| m.0 == i) {
            Some(at) => at,
            None => {
                let (start, end) = (self.stmts.list[i].start, self.stmts.end_of(i, text.len()));
                let bytes = &text.as_bytes()[start..limit.clamp(start, end)];
                memo.push((i, classify::lex_statement(bytes, self.dialect)));
                memo.len() - 1
            }
        };
        &memo[at].1
    }

    /// What to draw for `[window_start, window_start + window_len)`, at most `budget` units of
    /// it (the last range may be cut short). Call after [`Analyzer::sync`].
    pub fn paint(&mut self, window_start: u32, window_len: u32, budget: u32) -> Paint {
        let len = self.mirror.len_utf16();
        let start = window_start.min(len);
        let end = start.saturating_add(window_len).min(len);
        let mut out = Paint {
            revision: self.revision,
            doc_len_utf16: len,
            window_start: start,
            window_len: end - start,
            ..Paint::default()
        };

        self.follow_ceiling();
        if self.inactive {
            out.inactive = true;
            let mut taken: Vec<(u32, u32)> = Vec::new();
            for (a, b) in take(self.dirty.intersect(start, end), budget) {
                // Whole characters, so a cut range cannot split a surrogate pair.
                let a = self.mirror.utf16_of(self.mirror.byte_floor(a));
                let b = self.mirror.utf16_of(self.mirror.byte_ceil(b));
                if a < b {
                    taken.push((a, b));
                    out.ranges.extend([a, b - a]);
                    out.fonts.extend([a, b - a, 0]);
                }
            }
            self.finish(&mut out, &taken, start, end);
            return out;
        }

        // Work out the tokens of every statement the budgeted part of the window touches; that
        // can add to what is dirty, so take the budget again until nothing new turns up.
        let mut taken = take(self.dirty.intersect(start, end), budget);
        for _ in 0..4 {
            let mut worked = false;
            for &(a, b) in &taken {
                let (from, to) = (self.mirror.byte_floor(a), self.mirror.byte_ceil(b));
                let last = if to > from { to - 1 } else { from };
                for i in self.stmts.index_of(from)..=self.stmts.index_of(last) {
                    worked |= self.retokenize(i);
                }
            }
            if !worked {
                break;
            }
            taken = take(self.dirty.intersect(start, end), budget);
        }

        // Snap each range out to whole tokens, and merge what then touches.
        let mut memo = Vec::new();
        let limit = self.mirror.byte_ceil(end);
        let mut spans: Vec<(usize, usize)> = Vec::new();
        for &(a, b) in &taken {
            let (mut from, mut to) = (self.mirror.byte_floor(a), self.mirror.byte_ceil(b));
            if to <= from {
                continue;
            }
            let first = self.stmts.index_of(from);
            let tokens = self.tokens_of(&mut memo, first, limit);
            let base = self.stmts.list[first].start;
            let k = tokens.partition_point(|t| base + t.end as usize <= from);
            if let Some(token) = tokens.get(k).filter(|t| base + (t.start as usize) < from) {
                from = base + token.start as usize;
            }
            let last = self.stmts.index_of(to - 1);
            let tokens = self.tokens_of(&mut memo, last, limit);
            let base = self.stmts.list[last].start;
            let k = tokens.partition_point(|t| base + t.end as usize <= to);
            if let Some(token) = tokens.get(k).filter(|t| base + (t.start as usize) < to) {
                to = base + token.end as usize;
            }
            match spans.last_mut() {
                Some(prev) if prev.1 >= from => prev.1 = prev.1.max(to),
                _ => spans.push((from, to)),
            }
        }

        // The runs: every token inside a range, with the comments noted for the fonts.
        let mut comments: Vec<(u32, u32)> = Vec::new();
        let mut merged: Vec<(u32, u32)> = Vec::new();
        let mut cursor = self.mirror.cursor();
        for &(from, to) in &spans {
            let range_start = cursor.at(from);
            for i in self.stmts.index_of(from)..=self.stmts.index_of(to - 1) {
                let base = self.stmts.list[i].start;
                let tokens = self.tokens_of(&mut memo, i, limit);
                let first = tokens.partition_point(|t| base + (t.start as usize) < from);
                for token in tokens[first..]
                    .iter()
                    .take_while(|t| base + (t.end as usize) <= to)
                {
                    let (a, b) = (
                        cursor.at(base + token.start as usize),
                        cursor.at(base + token.end as usize),
                    );
                    out.runs.extend([a, b - a, token.class as u32]);
                    if token.class == Class::Comment {
                        comments.push((a, b));
                    }
                }
            }
            let range_end = cursor.at(to);
            out.ranges.extend([range_start, range_end - range_start]);
            merged.push((range_start, range_end));
        }

        // The fonts: the dirty font ranges inside the ranges, split at the comments.
        let mut next_comment = 0;
        for &(a, b) in &merged {
            for (fa, fb) in self.fonts.intersect(a, b) {
                let mut at = fa;
                while next_comment > 0 && comments[next_comment - 1].1 > fa {
                    next_comment -= 1;
                }
                for &(cs, ce) in comments[next_comment..].iter().take_while(|c| c.0 < fb) {
                    if ce <= at {
                        continue;
                    }
                    let (s, e) = (cs.max(at), ce.min(fb));
                    if s > at {
                        out.fonts.extend([at, s - at, 0]);
                    }
                    out.fonts.extend([s, e - s, 1]);
                    at = e;
                }
                if at < fb {
                    out.fonts.extend([at, fb - at, 0]);
                }
            }
        }

        self.finish(&mut out, &merged, start, end);
        syntax::evict(&mut self.stmts.list, &mut self.tree_bytes, TREE_CACHE_BYTES);
        out
    }

    /// Set the two flags: what is still dirty in the window after `painted`, and outside it.
    fn finish(&self, out: &mut Paint, painted: &[(u32, u32)], start: u32, end: u32) {
        let mut rest = RangeSet(self.dirty.intersect(start, end));
        for &(a, b) in painted {
            rest.subtract(a, b);
        }
        out.more_in_window = !rest.is_empty();
        out.dirty_elsewhere = self.dirty.outside(start, end);
    }

    /// Bring `inactive` in line with the text's length. Going over the ceiling drops every tree
    /// and token and has the whole text base-coloured; coming back under it, the whole text is
    /// dirty again.
    fn follow_ceiling(&mut self) {
        let over = self.mirror.len_utf16() > CEILING_UTF16;
        if over == self.inactive {
            return;
        }
        self.inactive = over;
        let len = self.mirror.len_utf16();
        self.dirty = RangeSet::whole(len);
        self.fonts = RangeSet::whole(len);
        if !over {
            return;
        }
        self.tree_bytes = 0;
        for stmt in &mut self.stmts.list {
            stmt.tree = None;
            stmt.tree_cost = 0;
            stmt.tree_stale = false;
            stmt.tokens = None;
            stmt.tokens_valid = false;
            stmt.outline = None;
        }
    }

    /// The statements, folds and issues as they are now.
    pub fn outline(&mut self) -> Outline {
        self.follow_ceiling();
        let ranges = self.stmts.ranges_for_ui(self.mirror.as_str());
        let mut out = Outline {
            revision: self.revision,
            ..Outline::default()
        };
        let mut cursor = self.mirror.cursor();
        for &(a, b) in &ranges {
            out.statements.extend([cursor.at(a), cursor.at(b)]);
        }
        if self.inactive {
            return out;
        }

        let mut found: Vec<Fold> = ranges
            .iter()
            .filter_map(|&(a, b)| folds::statement_fold(&self.mirror, self.lexer, a, b))
            .collect();
        let mut issues: Vec<Issue> = Vec::new();
        for i in 0..self.stmts.len() {
            self.ensure_outline(i);
            let stmt = &self.stmts.list[i];
            let Some((tree_folds, lexical)) = stmt.outline.as_ref() else {
                continue;
            };
            found.extend(
                tree_folds
                    .iter()
                    .filter_map(|rel| folds::placed(&self.mirror, stmt.start, rel)),
            );
            let mut cursor = self.mirror.cursor();
            for issue in lexical {
                let at = stmt.start + issue.start as usize;
                let (a, b) = (cursor.at(at), cursor.at(at + issue.len as usize));
                issues.push(Issue {
                    kind: issue.kind,
                    start: a,
                    len: b - a,
                });
            }
        }
        issues.sort_by_key(|issue| issue.start);
        out.folds = folds::dedupe(found);
        out.issues = issues;
        out
    }

    /// Fill in the cached folds and issues of statement `i`, parsing it if it has no tree. A
    /// tree parsed only for this, when the cache is full, is dropped again at once.
    fn ensure_outline(&mut self, i: usize) {
        let text = self.mirror.as_str();
        let (start, end) = (self.stmts.list[i].start, self.stmts.end_of(i, text.len()));
        let giant = self.stmts.is_giant(i, text.len());
        let stmt = &mut self.stmts.list[i];
        if stmt.outline.is_some() {
            return;
        }
        let statement = &text[start..end];
        let mut issues: Vec<RelIssue> = issues::lexical(statement, self.lexer);
        let mut found: Vec<RelFold> = Vec::new();
        if !giant {
            let had_tree = stmt.tree.is_some();
            self.tree_bytes -= stmt.tree_cost;
            self.syntax.ensure(stmt, statement);
            self.tree_bytes += stmt.tree_cost;
            if let Some(tree) = stmt.tree.as_ref() {
                found = folds::from_tree(tree);
                issues.extend(issues::syntax(tree));
            }
            if !had_tree && self.tree_bytes > TREE_CACHE_BYTES {
                self.tree_bytes -= stmt.tree_cost;
                stmt.tree_cost = 0;
                stmt.tree = None;
            }
        }
        issues.sort_by_key(|issue| issue.start);
        stmt.outline = Some((found, issues));
    }
}

/// Add the span a token diff found, and the comments among it, to `dirty` and `fonts`.
fn note_difference(
    mirror: &TextBuffer,
    dirty: &mut RangeSet,
    fonts: &mut RangeSet,
    stmt_start: usize,
    found: Option<Difference>,
) {
    let Some(Difference {
        span: (start, end),
        comments,
    }) = found
    else {
        return;
    };
    let mut cursor = mirror.cursor();
    let (a, b) = (
        cursor.at(stmt_start + start as usize),
        cursor.at(stmt_start + end as usize),
    );
    dirty.add(a, b);
    let mut cursor = mirror.cursor();
    for (s, e) in comments {
        let (s, e) = (
            cursor.at(stmt_start + s as usize),
            cursor.at(stmt_start + e as usize),
        );
        fonts.add(s, e);
        dirty.add(s, e);
    }
}

/// The first `budget` units of `ranges`; the range the budget ends in is cut.
fn take(ranges: Vec<(u32, u32)>, budget: u32) -> Vec<(u32, u32)> {
    let mut left = u64::from(budget);
    let mut out = Vec::new();
    for (a, b) in ranges {
        if left == 0 {
            break;
        }
        let len = u64::from(b - a);
        if len <= left {
            out.push((a, b));
            left -= len;
        } else {
            out.push((a, a + left as u32));
            left = 0;
        }
    }
    out
}

/// A text and its analysis, on one thread. This is the whole API in one place, and what the
/// tests and the bench drive; `qh-ffi` will hold the [`TextBuffer`] and the [`Analyzer`] under
/// two locks instead, so the main thread never waits for analysis.
pub struct Document {
    text: TextBuffer,
    analyzer: Analyzer,
}

impl Document {
    pub fn new(text: &str, dialect: Dialect) -> Result<Document, EditError> {
        Ok(Document {
            text: TextBuffer::new(text)?,
            analyzer: Analyzer::new(text, dialect)?,
        })
    }

    pub fn text(&self) -> &str {
        self.text.as_str()
    }

    pub fn revision(&self) -> u64 {
        self.text.revision()
    }

    pub fn len_utf16(&self) -> u32 {
        self.text.len_utf16()
    }

    pub fn line_count(&self) -> u32 {
        self.text.line_count()
    }

    pub fn buffer(&self) -> &TextBuffer {
        &self.text
    }

    pub fn analyzer(&self) -> &Analyzer {
        &self.analyzer
    }

    /// Replace `[start, start + len)` (UTF-16) with `text`; returns the new revision. Does no
    /// analysis.
    pub fn replace(&mut self, start: u32, len: u32, text: &str) -> Result<u64, EditError> {
        self.text.replace(start, len, text)
    }

    fn sync(&mut self) -> Result<(), EditError> {
        let log = self.text.drain_log();
        self.analyzer.sync(log)
    }

    /// See [`Analyzer::paint`]. `revision` is the revision the caller last saw: an older one
    /// than the text's is `Stale`.
    pub fn paint(
        &mut self,
        revision: u64,
        window_start: u32,
        window_len: u32,
        budget: u32,
    ) -> Result<Paint, EditError> {
        if revision < self.text.revision() {
            return Err(EditError::Stale);
        }
        self.sync()?;
        Ok(self.analyzer.paint(window_start, window_len, budget))
    }

    /// The UI applied a paint of `revision` over `ranges` (pairs `(start, len)`).
    pub fn mark_applied(&mut self, revision: u64, ranges: Vec<u32>) {
        self.text.mark_applied(revision, ranges);
    }

    /// `[start, start + len)` must be painted again (the font changed).
    pub fn mark_dirty(&mut self, start: u32, len: u32) -> Result<(), EditError> {
        self.text.mark_dirty(start, len)
    }

    pub fn outline(&mut self, revision: u64) -> Result<Outline, EditError> {
        if revision < self.text.revision() {
            return Err(EditError::Stale);
        }
        self.sync()?;
        Ok(self.analyzer.outline())
    }

    /// See [`Analyzer::converge`].
    pub fn converge(&mut self) -> Result<(), EditError> {
        self.sync()?;
        self.analyzer.converge();
        Ok(())
    }

    pub fn stats(&self) -> Stats {
        self.analyzer.stats()
    }
}
