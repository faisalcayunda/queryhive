//! The text of one document, and the log of what was done to it.
//!
//! Text is UTF-8; every offset that crosses the API is UTF-16, because the app measures text in
//! `NSString` units. Two indexes keep the conversion cheap after an edit in a 2 MB text: a
//! checkpoint (byte, UTF-16) at most every `CHUNK` bytes, and the byte offset of every line
//! (LF only; a CR is an ordinary character, as it is everywhere else in the editor).
//!
//! Both are shifted in place by an edit, so one keystroke costs O(edit + lines) and never a
//! rescan of the text.

use thiserror::Error;

/// Bytes between two checkpoints of the UTF-16 index.
const CHUNK: usize = 1024;

/// The largest text this holds. Offsets are `u32`, so the limit leaves room for the sum of two.
pub const MAX_BYTES: usize = (u32::MAX / 2) as usize;

/// Why an edit or a request was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum EditError {
    /// The caller asked about a revision older than the text's.
    #[error("the revision is out of date")]
    Stale,
    /// The range reaches past the text.
    #[error("the range is outside the text")]
    OutOfBounds,
    /// An end of the range falls between the two halves of a surrogate pair.
    #[error("the range splits a character")]
    SplitsCharacter,
    /// The text would be larger than the editor handles.
    #[error("the text is too large")]
    TooLarge,
    /// The analysis state is unusable (a poisoned lock, in the FFI).
    #[error("the analysis state is malformed")]
    Malformed,
}

/// One thing that happened to the text, in order. The analyzer replays these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogEntry {
    /// The text `[start, start + len)` (UTF-16) became `text`, giving `revision`.
    Edit {
        revision: u64,
        start: u32,
        len: u32,
        text: String,
    },
    /// The range must be painted again (a font or a colour scheme changed).
    Touch { start: u32, len: u32 },
    /// The UI applied a paint of `revision`: pairs `(start, len)` that are now up to date.
    Applied { revision: u64, ranges: Vec<u32> },
}

/// A text with UTF-16 and line indexes, a revision and a log.
#[derive(Debug, Clone)]
pub struct TextBuffer {
    text: String,
    /// `(byte, utf16)` at character boundaries, ascending; the first is `(0, 0)` and no two
    /// neighbours are more than about a chunk apart.
    marks: Vec<(u32, u32)>,
    /// Byte offset of the first byte of each line; the first is 0.
    lines: Vec<u32>,
    len16: u32,
    revision: u64,
    log: Vec<LogEntry>,
}

impl TextBuffer {
    /// A buffer holding `text`, at revision 1.
    pub fn new(text: &str) -> Result<TextBuffer, EditError> {
        if text.len() > MAX_BYTES {
            return Err(EditError::TooLarge);
        }
        let mut buffer = TextBuffer {
            text: text.to_owned(),
            marks: vec![(0, 0)],
            lines: vec![0],
            len16: 0,
            revision: 1,
            log: Vec::new(),
        };
        let mut last = 0;
        let mut units = 0u32;
        for (at, ch) in text.char_indices() {
            if at - last >= CHUNK {
                buffer.marks.push((at as u32, units));
                last = at;
            }
            if ch == '\n' {
                buffer.lines.push(at as u32 + 1);
            }
            units += ch.len_utf16() as u32;
        }
        buffer.len16 = units;
        Ok(buffer)
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn len_bytes(&self) -> usize {
        self.text.len()
    }

    pub fn len_utf16(&self) -> u32 {
        self.len16
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The number of lines: one more than the number of LFs.
    pub fn line_count(&self) -> u32 {
        self.lines.len() as u32
    }

    /// The 0-based line the byte at `byte` is on. A LF belongs to the line it ends.
    pub fn line_of_byte(&self, byte: usize) -> u32 {
        (self.lines.partition_point(|&start| start as usize <= byte) - 1) as u32
    }

    /// The byte offset where `line` starts, or the text's length for a line past the last.
    pub fn line_start(&self, line: u32) -> usize {
        self.lines
            .get(line as usize)
            .map_or(self.text.len(), |&start| start as usize)
    }

    /// The byte offset of the UTF-16 offset `at`.
    pub fn byte_of(&self, at: u32) -> Result<usize, EditError> {
        if at > self.len16 {
            return Err(EditError::OutOfBounds);
        }
        let mark = self.marks.partition_point(|m| m.1 <= at) - 1;
        let (byte, units) = self.marks[mark];
        let (mut byte, mut units) = (byte as usize, units);
        for ch in self.text[byte..].chars() {
            if units == at {
                break;
            }
            units += ch.len_utf16() as u32;
            if units > at {
                return Err(EditError::SplitsCharacter);
            }
            byte += ch.len_utf8();
        }
        Ok(byte)
    }

    /// The byte offset of `at`, or of the character boundary just before it if `at` is inside a
    /// surrogate pair (a window or a budget cut by the caller can land there). `at` past the end
    /// is the end.
    pub fn byte_floor(&self, at: u32) -> usize {
        let at = at.min(self.len16);
        self.byte_of(at)
            .or_else(|_| self.byte_of(at - 1))
            .unwrap_or(self.text.len())
    }

    /// Like [`TextBuffer::byte_floor`], rounding up to the boundary after the pair.
    pub fn byte_ceil(&self, at: u32) -> usize {
        let at = at.min(self.len16);
        self.byte_of(at)
            .or_else(|_| self.byte_of(at + 1))
            .unwrap_or(self.text.len())
    }

    /// The UTF-16 offset of the byte offset `byte`, which should be on a character boundary.
    pub fn utf16_of(&self, mut byte: usize) -> u32 {
        // A byte inside a character rounds down; callers pass boundaries, and this keeps a
        // bad offset from a tree from being a panic.
        byte = byte.min(self.text.len());
        while !self.text.is_char_boundary(byte) {
            byte -= 1;
        }
        let mark = self.marks.partition_point(|m| m.0 as usize <= byte) - 1;
        let (from, units) = self.marks[mark];
        units + self.text[from as usize..byte].encode_utf16().count() as u32
    }

    /// A cursor for converting many ascending byte offsets, each from the last.
    pub fn cursor(&self) -> Utf16Cursor<'_> {
        Utf16Cursor {
            buffer: self,
            byte: 0,
            units: 0,
        }
    }

    /// Replace the UTF-16 range `[start, start + len)` with `new`, without logging it. This is
    /// what the analyzer's mirror does; the main thread's [`TextBuffer::replace`] logs.
    pub fn edit(&mut self, start: u32, len: u32, new: &str) -> Result<(), EditError> {
        let end = start.checked_add(len).ok_or(EditError::OutOfBounds)?;
        let from = self.byte_of(start)?;
        let to = self.byte_of(end)?;
        if self.text.len() - (to - from) + new.len() > MAX_BYTES {
            return Err(EditError::TooLarge);
        }
        let new16 = new.encode_utf16().count() as u32;
        let delta_bytes = new.len() as i64 - (to - from) as i64;
        let delta_units = i64::from(new16) - i64::from(len);

        // Checkpoints: keep up to the last one at or before the edit, drop those strictly
        // inside it, shift the rest, then re-cut the stretch around the edit.
        let keep = self.marks.partition_point(|m| m.0 as usize <= from);
        let shift_from = keep + self.marks[keep..].partition_point(|m| (m.0 as usize) < to);
        for mark in &mut self.marks[shift_from..] {
            mark.0 = (i64::from(mark.0) + delta_bytes) as u32;
            mark.1 = (i64::from(mark.1) + delta_units) as u32;
        }
        self.marks.drain(keep..shift_from);
        // Lines: a line start `p` exists because the byte before it is a LF, so the starts in
        // `(from, to]` are the ones whose LF was deleted.
        let first_gone = self.lines.partition_point(|&s| s as usize <= from);
        let after = self.lines.partition_point(|&s| s as usize <= to);
        for start in &mut self.lines[after..] {
            *start = (i64::from(*start) + delta_bytes) as u32;
        }
        let born = new
            .bytes()
            .enumerate()
            .filter(|&(_, byte)| byte == b'\n')
            .map(|(at, _)| (from + at + 1) as u32);
        self.lines.splice(first_gone..after, born);

        self.text.replace_range(from..to, new);
        self.len16 = (i64::from(self.len16) + delta_units) as u32;

        // Re-cut checkpoints between the kept one and the first shifted one.
        let (region_from, mut units) = self.marks[keep - 1];
        let region_to = self
            .marks
            .get(keep)
            .map_or(self.text.len(), |m| m.0 as usize);
        let mut fresh = Vec::new();
        let mut last = region_from as usize;
        for (at, ch) in self.text[region_from as usize..region_to].char_indices() {
            let at = at + region_from as usize;
            if at - last >= CHUNK {
                fresh.push((at as u32, units));
                last = at;
            }
            units += ch.len_utf16() as u32;
        }
        self.marks.splice(keep..keep, fresh);

        Ok(())
    }

    /// Replace `[start, start + len)` (UTF-16) with `new`, log it, and return the new revision.
    pub fn replace(&mut self, start: u32, len: u32, new: &str) -> Result<u64, EditError> {
        self.edit(start, len, new)?;
        self.revision += 1;
        self.log.push(LogEntry::Edit {
            revision: self.revision,
            start,
            len,
            text: new.to_owned(),
        });
        Ok(self.revision)
    }

    /// Record that the UI applied a paint of `revision` over `ranges` (pairs `(start, len)`).
    pub fn mark_applied(&mut self, revision: u64, ranges: Vec<u32>) {
        self.log.push(LogEntry::Applied { revision, ranges });
    }

    /// Record that `[start, start + len)` must be painted again.
    pub fn mark_dirty(&mut self, start: u32, len: u32) -> Result<(), EditError> {
        let end = start.checked_add(len).ok_or(EditError::OutOfBounds)?;
        self.byte_of(start)?;
        self.byte_of(end)?;
        self.log.push(LogEntry::Touch { start, len });
        Ok(())
    }

    /// Take the log: everything since the last call.
    pub fn drain_log(&mut self) -> Vec<LogEntry> {
        std::mem::take(&mut self.log)
    }
}

/// Byte offset to UTF-16 offset, for a caller that walks forward through the text.
pub struct Utf16Cursor<'a> {
    buffer: &'a TextBuffer,
    byte: usize,
    units: u32,
}

impl Utf16Cursor<'_> {
    /// The UTF-16 offset of `byte` (a character boundary). Cheap when `byte` is near the last
    /// one asked; any order is correct.
    pub fn at(&mut self, mut byte: usize) -> u32 {
        let text = self.buffer.as_str();
        byte = byte.min(text.len());
        while !text.is_char_boundary(byte) {
            byte -= 1;
        }
        if byte >= self.byte && byte - self.byte <= 2 * CHUNK {
            self.units += text[self.byte..byte].encode_utf16().count() as u32;
        } else {
            self.units = self.buffer.utf16_of(byte);
        }
        self.byte = byte;
        self.units
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The indexes rebuilt from scratch, for comparison after every edit.
    fn assert_indexed(buffer: &TextBuffer) {
        let fresh = TextBuffer::new(buffer.as_str()).unwrap();
        assert_eq!(buffer.lines, fresh.lines);
        assert_eq!(buffer.len16, fresh.len16);
        // Marks may differ in where they sit, but must be honest and dense enough.
        let text = buffer.as_str();
        assert_eq!(buffer.marks[0], (0, 0));
        for pair in buffer.marks.windows(2) {
            assert!(pair[0].0 < pair[1].0 && pair[0].1 < pair[1].1);
            assert!(
                (pair[1].0 - pair[0].0) as usize <= 2 * CHUNK + 4,
                "{:?}",
                pair
            );
        }
        let mut units = 0u32;
        let mut marks = buffer.marks.iter().peekable();
        for (at, ch) in text.char_indices() {
            if let Some(&&(byte, expected)) = marks.peek() {
                if byte as usize == at {
                    assert_eq!(units, expected);
                    marks.next();
                }
            }
            units += ch.len_utf16() as u32;
        }
        assert!(
            marks.next().is_none(),
            "a mark past the end or inside a character"
        );
        let last = *buffer.marks.last().unwrap();
        assert!(text.len() - last.0 as usize <= 2 * CHUNK + 4);
    }

    #[test]
    fn offsets_convert_across_astral_characters() {
        // 😀 is 4 bytes and 2 UTF-16 units; é is 2 bytes and 1 unit.
        let buffer = TextBuffer::new("a😀é😀b").unwrap();
        assert_eq!(buffer.len_utf16(), 1 + 2 + 1 + 2 + 1);
        assert_eq!(buffer.byte_of(0), Ok(0));
        assert_eq!(buffer.byte_of(1), Ok(1));
        assert_eq!(buffer.byte_of(2), Err(EditError::SplitsCharacter));
        assert_eq!(buffer.byte_of(3), Ok(5));
        assert_eq!(buffer.byte_of(4), Ok(7));
        assert_eq!(buffer.byte_of(5), Err(EditError::SplitsCharacter));
        assert_eq!(buffer.byte_of(6), Ok(11));
        assert_eq!(buffer.byte_of(7), Ok(12));
        assert_eq!(buffer.byte_of(8), Err(EditError::OutOfBounds));
        for (byte, units) in [(0, 0), (1, 1), (5, 3), (7, 4), (11, 6), (12, 7)] {
            assert_eq!(buffer.utf16_of(byte), units);
        }
    }

    #[test]
    fn a_range_that_splits_a_surrogate_pair_is_refused() {
        let mut buffer = TextBuffer::new("x😀y").unwrap();
        assert_eq!(buffer.replace(2, 1, "z"), Err(EditError::SplitsCharacter));
        assert_eq!(buffer.replace(1, 1, "z"), Err(EditError::SplitsCharacter));
        assert_eq!(buffer.replace(1, 2, "z"), Ok(2));
        assert_eq!(buffer.as_str(), "xzy");
        assert_eq!(buffer.replace(0, 4, ""), Err(EditError::OutOfBounds));
        assert_eq!(buffer.replace(u32::MAX, 2, ""), Err(EditError::OutOfBounds));
    }

    #[test]
    fn lines_count_lf_only_and_a_lf_belongs_to_the_line_it_ends() {
        let buffer = TextBuffer::new("ab\r\ncd\n\nef").unwrap();
        assert_eq!(buffer.line_count(), 4);
        assert_eq!(buffer.line_of_byte(0), 0);
        assert_eq!(buffer.line_of_byte(3), 0); // the LF
        assert_eq!(buffer.line_of_byte(4), 1);
        assert_eq!(buffer.line_of_byte(6), 1); // the LF ending "cd"
        assert_eq!(buffer.line_of_byte(7), 2); // the LF of the empty line
        assert_eq!(buffer.line_of_byte(8), 3);
        assert_eq!(buffer.line_of_byte(9), 3);
        assert_eq!(buffer.line_start(3), 8);
        assert_eq!(buffer.line_start(9), buffer.len_bytes());
    }

    #[test]
    fn edits_keep_the_indexes_equal_to_a_rebuild() {
        let mut seed = 0x9E37_79B9_7F4A_7C15_u64;
        let mut next = move || {
            seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = seed;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^ (z >> 31)
        };
        let pieces = [
            "a",
            "é",
            "😀",
            "\n",
            "\n\n",
            "select 1;",
            " ",
            "",
            "xyzzy ".repeat(300).leak(),
        ];
        let mut buffer = TextBuffer::new(&"line of text\n".repeat(150)).unwrap();
        for _ in 0..400 {
            let len = buffer.len_utf16();
            let start = (next() % (u64::from(len) + 1)) as u32;
            let mut end = start + (next() % 40) as u32;
            end = end.min(len);
            let piece = pieces[(next() % pieces.len() as u64) as usize];
            // Only edits on character boundaries are valid; skip the others.
            if buffer.byte_of(start).is_err() || buffer.byte_of(end).is_err() {
                continue;
            }
            buffer.replace(start, end - start, piece).unwrap();
            assert_indexed(&buffer);
        }
    }

    #[test]
    fn the_log_records_edits_touches_and_applied_ranges() {
        let mut buffer = TextBuffer::new("abc").unwrap();
        assert_eq!(buffer.replace(1, 1, "XY"), Ok(2));
        buffer.mark_dirty(0, 2).unwrap();
        assert_eq!(buffer.mark_dirty(0, 9), Err(EditError::OutOfBounds));
        buffer.mark_applied(2, vec![0, 4]);
        assert_eq!(
            buffer.drain_log(),
            vec![
                LogEntry::Edit {
                    revision: 2,
                    start: 1,
                    len: 1,
                    text: "XY".into()
                },
                LogEntry::Touch { start: 0, len: 2 },
                LogEntry::Applied {
                    revision: 2,
                    ranges: vec![0, 4]
                },
            ]
        );
        assert!(buffer.drain_log().is_empty());
    }

    #[test]
    fn a_cursor_agrees_with_utf16_of_in_any_order() {
        let text = "é😀 select ".repeat(2000);
        let buffer = TextBuffer::new(&text).unwrap();
        let mut cursor = buffer.cursor();
        let boundaries: Vec<usize> = text.char_indices().map(|(at, _)| at).step_by(37).collect();
        for &byte in boundaries.iter().chain(boundaries.iter().rev()) {
            assert_eq!(cursor.at(byte), buffer.utf16_of(byte));
        }
    }
}
