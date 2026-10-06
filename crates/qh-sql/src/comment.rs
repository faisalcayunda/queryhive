//! Toggle `-- ` line comments on whole lines, changing nothing but the marker.
//!
//! The contract (blueprint W12 section 3.3, D-5): the only bytes that are added or removed are
//! one `-- ` (or a bare `--` at a line's end) per non-blank line. A line is refused, and the
//! whole operation with it, when it starts or ends strictly inside a string, quoted
//! identifier, block comment or dollar quote under any reading the caller passes, because a
//! marker in the middle of such a region would change data and one at its edge would move
//! where the region ends. A line comment never refuses: opening and closing one is the point.
//! A region that is never closed swallows the rest of the text, so every line that holds part
//! of it is refused, but a line comment with no newline after it counts as closed.
//!
//! Commenting is checked against the whole result: the strings, quoted names, block comments
//! and dollar quotes outside the chosen lines must still be the same regions, and each marker
//! must open a comment that ends exactly at its line's end, under every reading. PostgreSQL
//! joins `'a'`, a newline (with comments) and `'b'` into one string, so commenting out the
//! line between two such strings would silently merge them; and under the generic and
//! MySQL readings a `--` comment runs past a lone `\r`. Both are refused as [`CommentError::Spills`]. Removing a marker is not
//! checked: changing what the lines mean is the point.
//!
//! Lines end at `\n`, `\r\n` or a lone `\r` (PostgreSQL and Trino end a `--` comment at `\r`,
//! so a lone `\r` inside a "line" would leave the rest of it live code). Line numbers are
//! 1-based and inclusive, like the `line` in every error of this crate. Offsets are bytes; the
//! caller converts to UTF-16.

use thiserror::Error;

use crate::scan::{walk, EndState, Lexer, OpaqueKind, Visitor};

/// One replacement: `text[start..end]` becomes `replacement`. The range is the smallest one
/// that holds every added or removed marker; it is empty when there is nothing to toggle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineEdit {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CommentError {
    /// `line` starts or ends inside a string, quoted name, block comment or dollar quote.
    #[error("line {line}: it starts or ends inside a string, quoted name, block comment or dollar quote; comment whole statements instead")]
    InsideRegion { line: usize },
    /// The line range is empty, starts at 0 or ends past the last line.
    #[error("the line range is outside the text")]
    OutOfRange,
    /// Commenting these lines would also change the text next to them: a string would join
    /// the next one, or a comment would run past the end of its line.
    #[error("commenting these lines would change the text around them (a string would join the next one, or a comment would run on); comment whole statements instead")]
    Spills,
}

type Regions = Vec<(OpaqueKind, usize, usize)>;

struct Collect(Regions);

impl Visitor for Collect {
    fn opaque(&mut self, kind: OpaqueKind, start: usize, end: usize) {
        self.0.push((kind, start, end));
    }
}

/// The opaque regions of `bytes` under `lexer`, in order. A region left open (other than a
/// line comment) has no end, so it gets `usize::MAX`.
fn regions(bytes: &[u8], lexer: Lexer) -> Regions {
    let mut c = Collect(Vec::new());
    if let EndState::Open(kind) = walk(bytes, lexer, &mut c) {
        if kind != OpaqueKind::LineComment {
            if let Some(last) = c.0.last_mut() {
                last.2 = usize::MAX;
            }
        }
    }
    c.0
}

/// Whether `pos` lies strictly inside a region that is not a line comment.
fn inside(regions: &Regions, pos: usize) -> bool {
    let i = regions.partition_point(|r| r.1 < pos);
    i > 0 && regions[i - 1].0 != OpaqueKind::LineComment && pos < regions[i - 1].2
}

/// Whether `from..to` lies within one line comment.
fn commented_out(regions: &Regions, from: usize, to: usize) -> bool {
    let i = regions.partition_point(|r| r.1 <= from);
    i > 0 && regions[i - 1].0 == OpaqueKind::LineComment && regions[i - 1].2 >= to
}

/// A line's content `start..end` (terminator excluded) and where its indentation ends.
struct Line {
    start: usize,
    end: usize,
    indent: usize,
}

/// Lines `first..=last` (1-based), or `None` when the text has fewer lines.
fn lines(bytes: &[u8], first: usize, last: usize) -> Option<Vec<Line>> {
    if first == 0 || first > last {
        return None;
    }
    let mut out = Vec::new();
    let (mut start, mut n) = (0, 1);
    loop {
        let end = bytes[start..]
            .iter()
            .position(|b| matches!(b, b'\n' | b'\r'))
            .map_or(bytes.len(), |p| start + p);
        if n >= first {
            let indent = bytes[start..end]
                .iter()
                .position(|b| !matches!(b, b' ' | b'\t'))
                .map_or(end, |p| start + p);
            out.push(Line { start, end, indent });
        }
        if n == last {
            return Some(out);
        }
        if end == bytes.len() {
            return None;
        }
        start = if bytes[end] == b'\r' && bytes.get(end + 1) == Some(&b'\n') {
            end + 2
        } else {
            end + 1
        };
        n += 1;
    }
}

fn is_blank(bytes: &[u8], l: &Line) -> bool {
    bytes[l.start..l.end]
        .iter()
        .all(|b| matches!(b, b' ' | b'\t' | 0x0b | 0x0c))
}

const MARKER: &str = "-- ";

/// Toggle a `-- ` comment on lines `first_line..=last_line` (1-based, inclusive).
///
/// If every non-blank line already starts (after its indentation) with `--` followed by a
/// space or the end of the line, the markers are removed. Otherwise `-- ` is inserted on every
/// non-blank line at the longest indentation they all share, so a block stays aligned. Blank
/// lines are never touched, and a MySQL `#` is not a marker.
///
/// `readings` are the lexers the text must be safe under (`Dialect::readings()`; for a
/// generic dialect the caller may pass the union of several). An empty slice means the
/// generic lexer. A line is refused when any reading puts its start or end inside a region.
pub fn toggle_line_comment(
    text: &str,
    readings: &[Lexer],
    first_line: usize,
    last_line: usize,
) -> Result<LineEdit, CommentError> {
    let bytes = text.as_bytes();
    let target = lines(bytes, first_line, last_line).ok_or(CommentError::OutOfRange)?;
    let readings = if readings.is_empty() {
        &[Lexer::GENERIC][..]
    } else {
        readings
    };
    let observed: Vec<Regions> = readings.iter().map(|l| regions(bytes, *l)).collect();
    for (k, l) in target.iter().enumerate() {
        if observed
            .iter()
            .any(|r| inside(r, l.start) || inside(r, l.end))
        {
            return Err(CommentError::InsideRegion {
                line: first_line + k,
            });
        }
    }

    let body: Vec<&Line> = target.iter().filter(|l| !is_blank(bytes, l)).collect();
    let Some(first) = body.first() else {
        let at = target[0].start;
        return Ok(LineEdit {
            start: at,
            end: at,
            replacement: String::new(),
        });
    };
    let marked = |l: &Line| {
        bytes[l.indent..].starts_with(b"--")
            && (l.indent + 2 == l.end || bytes[l.indent + 2] == b' ')
            && observed.iter().all(|r| commented_out(r, l.indent, l.end))
    };
    let uncomment = body.iter().all(|l| marked(l));

    // One (position, bytes removed) per body line; the inserted text is `MARKER` or nothing.
    let edits: Vec<(usize, usize)> = if uncomment {
        body.iter()
            .map(|l| {
                (
                    l.indent,
                    2 + usize::from(bytes.get(l.indent + 2) == Some(&b' ')),
                )
            })
            .collect()
    } else {
        let lead = &bytes[first.start..first.indent];
        let shared = body.iter().fold(lead.len(), |n, l| {
            let own = &bytes[l.start..l.indent];
            n.min(lead.iter().zip(own).take_while(|(a, b)| a == b).count())
        });
        body.iter().map(|l| (l.start + shared, 0)).collect()
    };
    let insert = if uncomment { "" } else { MARKER };

    let start = edits[0].0;
    let end = edits[edits.len() - 1].0 + edits[edits.len() - 1].1;
    let mut replacement = String::new();
    let mut at = start;
    for &(pos, cut) in &edits {
        replacement.push_str(&text[at..pos]);
        replacement.push_str(insert);
        at = pos + cut;
    }

    if !uncomment {
        let joined = format!("{}{}{}", &text[..start], replacement, &text[end..]);
        let (from, to) = (target[0].start, target[target.len() - 1].end);
        let shift = body.len() * MARKER.len();
        let spilled = readings.iter().zip(&observed).any(|(lexer, old)| {
            let new = regions(joined.as_bytes(), *lexer);
            // Outside the chosen lines the non-comment regions must be the old ones, moved.
            let kept = |r: &&(OpaqueKind, usize, usize)| r.0 != OpaqueKind::LineComment;
            let expect: Vec<_> = old
                .iter()
                .filter(kept)
                .filter(|r| r.2 <= from || r.1 >= to)
                .map(|&(k, s, e)| {
                    let by = if s >= to { shift } else { 0 };
                    (k, s + by, e.saturating_add(by))
                })
                .collect();
            if new.iter().filter(kept).copied().collect::<Vec<_>>() != expect {
                return true;
            }
            // Each marker opens a line comment that ends where its line does (before the `\n`
            // of a CRLF at the latest: only PostgreSQL and Trino stop at the `\r`).
            body.iter().enumerate().any(|(k, l)| {
                let marker = edits[k].0 + k * MARKER.len();
                let at = new.partition_point(|r| r.1 <= marker);
                let line_end = l.end + (k + 1) * MARKER.len();
                let crlf = joined.as_bytes()[line_end..].starts_with(b"\r\n");
                at == 0
                    || new[at - 1].0 != OpaqueKind::LineComment
                    || new[at - 1].1 != marker
                    || ![line_end, line_end + usize::from(crlf)].contains(&new[at - 1].2)
            })
        });
        if spilled {
            return Err(CommentError::Spills);
        }
    }
    Ok(LineEdit {
        start,
        end,
        replacement,
    })
}
