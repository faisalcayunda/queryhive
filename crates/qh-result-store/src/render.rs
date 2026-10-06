//! The window renderer: one `QHW1` buffer, a 256-UTF-16 truncation rule, the
//! port of `ColumnFormat`, the JSON validator, and the width stats (§8, §11, §7).
//!
//! The QHW1 layout is a single little-endian buffer that Swift reads with
//! `loadUnaligned`. Every offset is an absolute byte offset into the same
//! buffer, so no pointer ever crosses the FFI boundary: the buffer is the
//! data and there is no second copy.

use unicode_segmentation::UnicodeSegmentation;

use qh_columnar::Encoding;
use qh_core::{ColumnMeta, Value};

use crate::chunk::StoreChunk;
use crate::collate;
use crate::StoreError;

/// Global flags in the QHW1 header, bit 0..2.
pub const FLAG_CUT: u16 = 1 << 0;
pub const FLAG_COMPLETE: u16 = 1 << 1;
pub const FLAG_VIEWED: u16 = 1 << 2;

/// Cell flags, bit 0..4.
pub const CELL_NULL: u8 = 1 << 0;
pub const CELL_EMPTY: u8 = 1 << 1;
pub const CELL_OPENABLE: u8 = 1 << 2;
pub const CELL_NUMERIC: u8 = 1 << 3;
pub const CELL_TRUNCATED: u8 = 1 << 4;

/// Header size: magic 4 + version 2 + global flags 2 + first_row 4 + row_count 4
/// + column_count 4 + visible_total 4 + heap_len 4 + reserved 4.
pub const HEADER_LEN: usize = 32;

/// The truncation cap in UTF-16 units (§11.3).
pub const TRUNCATE_UTF16: usize = 256;

/// A per-column display format, the port of `ColumnFormat` (§11.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnFormat {
    /// The stored text, unchanged.
    Raw,
    /// For binary types: decoded UTF-8; otherwise: the stored text.
    Text,
    /// A 32-hex-digit UUID, written 8-4-4-4-12; otherwise the stored text.
    Uuid,
    /// A Unix timestamp as `yyyy-MM-dd HH:mm:ss` UTC; otherwise the stored text.
    UnixTimestamp,
    /// Parsed and re-printed JSON; otherwise the stored text.
    Json,
}

impl ColumnFormat {
    /// Parse the metadata string `qh.fmt` back.
    pub fn parse(name: &str) -> Self {
        match name {
            "text" => ColumnFormat::Text,
            "uuid" => ColumnFormat::Uuid,
            "unix_timestamp" => ColumnFormat::UnixTimestamp,
            "json" => ColumnFormat::Json,
            _ => ColumnFormat::Raw,
        }
    }
}

/// The spec for one window request (§11.1 + §11.2 + §11.4).
#[derive(Debug, Clone)]
pub struct WindowSpec {
    /// The rows to read, in view order (already permuted by the view).
    pub rows: Vec<u32>,
    /// Which source columns to include, in requested order.
    pub columns: Vec<usize>,
    /// The per-column format applied to the stored text. One per `columns`.
    pub formats: Vec<ColumnFormat>,
    /// Global flag bits (CUT, COMPLETE, VIEWED).
    pub global_flags: u16,
}

/// The result of a window read: the single buffer, plus its shape for the reader.
#[derive(Debug, Clone)]
pub struct Window {
    pub data: Vec<u8>,
    pub first_row: u32,
    pub row_count: u32,
    pub column_count: u32,
}

impl Window {
    /// Total buffer length.
    pub fn total_len(&self) -> usize {
        self.data.len()
    }

    /// The stored `source_rows` table (`R` entries) at offset 32.
    pub fn source_rows(&self) -> Vec<u32> {
        let r = self.row_count as usize;
        let base = HEADER_LEN;
        (0..r)
            .map(|i| {
                let at = base + 4 * i;
                u32::from_le_bytes([
                    self.data[at],
                    self.data[at + 1],
                    self.data[at + 2],
                    self.data[at + 3],
                ])
            })
            .collect()
    }

    /// The text of cell `(row, column)` as UTF-8, decoded from the heap.
    pub fn cell_text(&self, row: usize, column: usize) -> String {
        let r = self.row_count as usize;
        let c = self.column_count as usize;
        let index = row * c + column;
        let s0 = HEADER_LEN + 4 * r;
        let off_at = |k: usize| {
            let at = s0 + 4 * k;
            u32::from_le_bytes([
                self.data[at],
                self.data[at + 1],
                self.data[at + 2],
                self.data[at + 3],
            ]) as usize
        };
        let s1 = s0 + 4 * (r * c + 1);
        let heap_len =
            u32::from_le_bytes([self.data[24], self.data[25], self.data[26], self.data[27]])
                as usize;
        let s2 = s1 + r * c;
        let start = off_at(index).min(heap_len);
        let end = off_at(index + 1).min(heap_len);
        String::from_utf8_lossy(&self.data[s2 + start..s2 + end]).into_owned()
    }

    /// The cell flags byte for `(row, column)`.
    pub fn cell_flags(&self, row: usize, column: usize) -> u8 {
        let r = self.row_count as usize;
        let c = self.column_count as usize;
        let s1 = HEADER_LEN + 4 * r + 4 * (r * c + 1);
        self.data[s1 + row * c + column]
    }

    /// The global flags word.
    pub fn global_flags(&self) -> u16 {
        u16::from_le_bytes([self.data[6], self.data[7]])
    }

    /// `heap_len` from the header.
    pub fn heap_len(&self) -> u32 {
        u32::from_le_bytes([self.data[24], self.data[25], self.data[26], self.data[27]])
    }
}

/// Build a QHW1 buffer from a chunk, a set of rows and a set of columns.
///
/// `rows` is the source-row indices (in view order), `columns` is the source
/// column indices (in display order), `formats` is the display format per
/// column, and `full` means "no truncation" (the `rows_text` / `cell_text`
/// path).
///
/// The `offsets_from` scratch array is one `CowCell` per source column and
/// is reused across the whole window call.
pub fn render_window(
    chunk: &StoreChunk,
    spec: &WindowSpec,
    offsets_from: &mut [CowCell],
    full: bool,
) -> Result<Window, StoreError> {
    let rows = spec.rows.len();
    let cols = spec.columns.len();
    let cell_count = rows * cols;

    // Validate the request before indexing anything: a short scratch array,
    // a short formats list, or a column past the chunk width must be an
    // error, not a panic (§11.5).
    if spec.formats.len() != cols {
        return Err(StoreError::InvalidArgument {
            message: format!(
                "window asked for {cols} columns but gave {} formats",
                spec.formats.len()
            ),
        });
    }
    if offsets_from.len() < cols {
        return Err(StoreError::InvalidArgument {
            message: format!(
                "window scratch holds {} cells but {cols} columns were asked for",
                offsets_from.len()
            ),
        });
    }
    let width = chunk.batch.num_columns();
    for &column in &spec.columns {
        if column >= width {
            return Err(StoreError::InvalidArgument {
                message: format!("window column {column} is out of range (width {width})"),
            });
        }
    }

    let mut text_offsets: Vec<u32> = Vec::with_capacity(cell_count + 1);
    text_offsets.push(0);
    let mut cell_flags: Vec<u8> = vec![0u8; cell_count];
    let mut heap: Vec<u8> = Vec::new();

    for (r, &row) in spec.rows.iter().enumerate() {
        if row as usize >= chunk.batch.num_rows() {
            return Err(StoreError::InvalidArgument {
                message: format!(
                    "window row {row} is out of range (chunk holds {} rows)",
                    chunk.batch.num_rows()
                ),
            });
        }
        for (c, &col) in spec.columns.iter().enumerate() {
            let array = chunk.batch.column(col);
            let enc = chunk.encodings.get(col).copied().unwrap_or(Encoding::Null);
            let scratch = &mut offsets_from[c];
            render_cell_into(
                scratch,
                array,
                enc,
                row as usize,
                col,
                spec.formats[c],
                full,
                &mut heap,
                &chunk.flags,
            )?;
            let flags = scratch.flags;
            text_offsets.push(heap.len() as u32);
            cell_flags[r * cols + c] = flags;
        }
    }

    // Assemble the buffer.
    let rc1 = cell_count + 1;
    let s0 = HEADER_LEN + 4 * rows;
    let s1 = s0 + 4 * rc1;
    let s2 = s1 + cell_count;
    let total = s2 + heap.len();

    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"QHW1");
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&spec.global_flags.to_le_bytes());
    out.extend_from_slice(&(spec.rows.first().copied().unwrap_or(0)).to_le_bytes());
    out.extend_from_slice(&(rows as u32).to_le_bytes());
    out.extend_from_slice(&(cols as u32).to_le_bytes());
    out.extend_from_slice(&(rows as u32).to_le_bytes()); // visible_total = rows for now
    out.extend_from_slice(&(heap.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // reserved
    for &row in &spec.rows {
        out.extend_from_slice(&row.to_le_bytes());
    }
    for &off in &text_offsets {
        out.extend_from_slice(&off.to_le_bytes());
    }
    for &flag in &cell_flags {
        out.push(flag);
    }
    out.extend_from_slice(&heap);
    out.shrink_to_fit();

    Ok(Window {
        data: out,
        first_row: spec.rows.first().copied().unwrap_or(0),
        row_count: rows as u32,
        column_count: cols as u32,
    })
}

/// One cell's scratch: the rendered text plus its flag bits. Reused across
/// a whole window call so no allocation is made per cell after the first.
#[derive(Default, Debug, Clone)]
pub struct CowCell {
    text: Vec<u8>,
    flags: u8,
}

/// Render one cell into `cell.text` (replacing previous contents) and set
/// `cell.flags`.
#[allow(clippy::too_many_arguments)]
fn render_cell_into(
    cell: &mut CowCell,
    array: &dyn arrow_array::Array,
    enc: Encoding,
    row: usize,
    column: usize,
    format: ColumnFormat,
    full: bool,
    heap: &mut Vec<u8>,
    chunk_flags: &crate::chunk::ChunkFlags,
) -> Result<(), StoreError> {
    cell.text.clear();
    cell.flags = 0;

    // A NullArray has no validity buffer, so `is_null` is false for it: the encoding decides.
    let is_null = enc == Encoding::Null || array.is_null(row);
    if is_null {
        cell.flags |= CELL_NULL;
        return Ok(());
    }

    // The text per encoding (§11.2).
    let text: String = match enc {
        Encoding::Text | Encoding::Json => qh_columnar::text_at(array, enc, row)
            .map_err(|e| StoreError::Corrupt {
                detail: e.to_string(),
            })?
            .map(|cow| cow.into_owned())
            .unwrap_or_default(),
        Encoding::Bytes => {
            let bytes =
                qh_columnar::value_at(array, enc, row).map_err(|e| StoreError::Corrupt {
                    detail: e.to_string(),
                })?;
            if let Value::Bytes(ref bytes) = bytes {
                let limit = if full {
                    bytes.len()
                } else {
                    TRUNCATE_UTF16 / 2
                };
                if limit < bytes.len() {
                    cell.flags |= CELL_TRUNCATED;
                }
                qh_core::render::to_text(&Value::Bytes(bytes[..limit.min(bytes.len())].to_vec()))
                    .unwrap_or_default()
            } else {
                String::new()
            }
        }
        _ => {
            let value =
                qh_columnar::value_at(array, enc, row).map_err(|e| StoreError::Corrupt {
                    detail: e.to_string(),
                })?;
            qh_core::render::to_text(&value).unwrap_or_default()
        }
    };

    if text.is_empty() {
        cell.flags |= CELL_EMPTY;
    }

    // Flags from the chunk's bitmaps (§7). The bitmap is per source column,
    // so the cell's own column decides whether it is openable.
    if let Some(bitmap) = chunk_flags.openable.get(column).and_then(|b| b.as_ref()) {
        if bitmap.value(row) {
            cell.flags |= CELL_OPENABLE;
        }
    }

    // Numeric flag: computed from the *text* (§7.2).
    if matches!(
        enc,
        Encoding::Text | Encoding::Json | Encoding::Tagged | Encoding::Bytes | Encoding::F64
    ) && collate::is_swift_plain_number(&text)
    {
        cell.flags |= CELL_NUMERIC;
    }

    // Format (§11.4).
    let formatted: String = match format {
        ColumnFormat::Raw => text.to_owned(),
        ColumnFormat::Text => format_text(&text, enc),
        ColumnFormat::Uuid => canonical_uuid(&text).unwrap_or_else(|| text.to_owned()),
        ColumnFormat::UnixTimestamp => unix_timestamp(&text).unwrap_or_else(|| text.to_owned()),
        ColumnFormat::Json => {
            if let Some(pretty) = pretty_json(&text) {
                pretty.replace('\n', " ")
            } else {
                text.to_owned()
            }
        }
    };

    // Truncate to 256 UTF-16 units (§11.3) unless in full mode.
    let final_text = if full {
        formatted.clone()
    } else {
        truncate_utf16(&formatted, TRUNCATE_UTF16)
    };
    if final_text != formatted {
        cell.flags |= CELL_TRUNCATED;
    }

    cell.text.extend_from_slice(final_text.as_bytes());
    heap.extend_from_slice(&cell.text);
    Ok(())
}

/// The `Text` format: hex-decode for a `Bytes` cell, identity otherwise.
fn format_text(text: &str, enc: Encoding) -> String {
    if !matches!(enc, Encoding::Bytes) {
        return text.to_owned();
    }
    let stripped = text
        .strip_prefix("\\x")
        .or_else(|| text.strip_prefix("\\X"))
        .or_else(|| text.strip_prefix("0x"))
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    if stripped.len() % 2 != 0 {
        return text.to_owned();
    }
    let bytes: Vec<u8> = (0..stripped.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&stripped[i..i + 2], 16).ok())
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Port of `GridSort.canonicalUUID`: 32 hex digits (case-insensitive, with
/// optional dashes) → 8-4-4-4-12.
fn canonical_uuid(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return None;
    }
    if !chars.iter().all(|c| c.is_ascii_hexdigit() || *c == '-') {
        return None;
    }
    let digits: Vec<char> = chars
        .iter()
        .filter(|c| c.is_ascii_hexdigit())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if digits.len() != 32 {
        return None;
    }
    let s = digits.iter().collect::<String>();
    Some(format!(
        "{}-{}-{}-{}-{}",
        &s[0..8],
        &s[8..12],
        &s[12..16],
        &s[16..20],
        &s[20..32]
    ))
}

/// Port of `GridSort.timestamp`: `n` (or `n/1000` when `|n| >= 1e11`) seconds
/// → `yyyy-MM-dd HH:mm:ss` UTC.
fn unix_timestamp(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let value = collate::swift_double(trimmed)?;
    if !value.is_finite() {
        return None;
    }
    let seconds = if value.abs() >= 1e11 {
        value / 1000.0
    } else {
        value
    };
    let whole = seconds as i64;
    let days = whole.div_euclid(86_400);
    let within_day = whole.rem_euclid(86_400);
    let (year, month, day) = qh_core::render::civil_from_days(days);
    let hours = within_day / 3600;
    let minutes = (within_day % 3600) / 60;
    let secs = within_day % 60;
    Some(format!(
        "{year:04}-{month:02}-{day:02} {hours:02}:{minutes:02}:{secs:02}"
    ))
}

/// Parse and pretty-print JSON, the port of `JSONSerialization` with
/// `[.prettyPrinted, .fragmentsAllowed, .sortedKeys]`.
fn pretty_json(text: &str) -> Option<String> {
    let parsed = serde_json::from_str::<serde_json::Value>(text).ok()?;
    Some(pretty_print_json(&parsed))
}

fn pretty_print_json(value: &serde_json::Value) -> String {
    let mut out = String::new();
    write_pretty_json(value, &mut out, 0);
    out
}

fn write_pretty_json(value: &serde_json::Value, out: &mut String, depth: usize) {
    use serde_json::Value as JV;
    let indent = "  ".repeat(depth);
    match value {
        JV::Null => out.push_str("null"),
        JV::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        JV::Number(n) => out.push_str(&n.to_string()),
        JV::String(s) => {
            out.push('"');
            for c in s.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '/' => out.push_str("\\/"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                    c => out.push(c),
                }
            }
            out.push('"');
        }
        JV::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&"  ".repeat(depth + 1));
                write_pretty_json(item, out, depth + 1);
            }
            out.push_str(&format!("\n{indent}]"));
        }
        JV::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            let mut entries: Vec<(&String, &JV)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push_str("{\n");
            for (i, (key, value)) in entries.iter().enumerate() {
                if i > 0 {
                    out.push_str(",\n");
                }
                out.push_str(&"  ".repeat(depth + 1));
                out.push('"');
                for c in key.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '/' => out.push_str("\\/"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        '\t' => out.push_str("\\t"),
                        c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                        c => out.push(c),
                    }
                }
                out.push('"');
                out.push_str(" : ");
                write_pretty_json(value, out, depth + 1);
            }
            out.push_str(&format!("\n{indent}}}"));
        }
    }
}

/// Truncate `s` to at most `limit` UTF-16 units, ending at an extended
/// grapheme cluster boundary.
pub fn truncate_utf16(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        return s.to_owned();
    }
    let mut units: usize = 0;
    let mut end: usize = 0;
    for (i, g) in s.grapheme_indices(true) {
        let utf16 = g.encode_utf16().count();
        if units + utf16 > limit {
            break;
        }
        units += utf16;
        end = i + g.len();
    }
    s[..end.min(s.len())].to_owned()
}

/// The iterative JSON validator (§7.1): no tree, no per-node allocation.
///
/// The stack holds one frame per open container, each carrying the separator
/// state it is in, so the parser never has to re-scan a position it already
/// consumed. `expect_value` means "the next token must be a value";
/// `after_value` means "a value just completed, resolve its parent".
pub fn json_validate(text: &str) -> bool {
    let bytes = text.as_bytes();
    let n = bytes.len();

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum ObjState {
        /// Right after `{`: a key string or `}`.
        First,
        /// After a `,`: a key string, never `}`.
        Key,
        /// After a key: `:`.
        Colon,
        /// After `:`: a value.
        Value,
        /// After a value: `,` or `}`.
        Comma,
    }
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum ArrState {
        /// Right after `[`: a value or `]`.
        First,
        /// After a `,`: a value, never `]`.
        Value,
        /// After a value: `,` or `]`.
        Comma,
    }
    enum Frame {
        Obj(ObjState),
        Arr(ArrState),
    }

    fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
        while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        i
    }

    let mut stack: Vec<Frame> = Vec::new();
    let mut i = 0usize;
    let mut expect_value = true;
    let mut after_value = false;

    loop {
        if after_value {
            after_value = false;
            i = skip_ws(bytes, i);
            match stack.last_mut() {
                None => return i >= n,
                Some(Frame::Obj(state)) => *state = ObjState::Comma,
                Some(Frame::Arr(state)) => *state = ArrState::Comma,
            }
            continue;
        }

        if expect_value {
            expect_value = false;
            i = skip_ws(bytes, i);
            if i >= n {
                return false;
            }
            match bytes[i] {
                b'{' => {
                    i += 1;
                    stack.push(Frame::Obj(ObjState::First));
                }
                b'[' => {
                    i += 1;
                    stack.push(Frame::Arr(ArrState::First));
                }
                b'"' => match skip_string(bytes, i) {
                    Some(end) => {
                        i = end;
                        after_value = true;
                    }
                    None => return false,
                },
                b't' => match expect_literal(bytes, i, b"true") {
                    Some(end) => {
                        i = end;
                        after_value = true;
                    }
                    None => return false,
                },
                b'f' => match expect_literal(bytes, i, b"false") {
                    Some(end) => {
                        i = end;
                        after_value = true;
                    }
                    None => return false,
                },
                b'n' => match expect_literal(bytes, i, b"null") {
                    Some(end) => {
                        i = end;
                        after_value = true;
                    }
                    None => return false,
                },
                b'-' | b'0'..=b'9' => match skip_number(bytes, i) {
                    Some(end) => {
                        i = end;
                        after_value = true;
                    }
                    None => return false,
                },
                _ => return false,
            }
            continue;
        }

        i = skip_ws(bytes, i);
        if i >= n {
            return false;
        }
        match stack.last_mut() {
            None => expect_value = true,
            Some(Frame::Obj(state)) => match *state {
                ObjState::First => {
                    if bytes[i] == b'}' {
                        i += 1;
                        stack.pop();
                        after_value = true;
                    } else {
                        match skip_string(bytes, i) {
                            Some(end) => {
                                i = end;
                                *state = ObjState::Colon;
                            }
                            None => return false,
                        }
                    }
                }
                ObjState::Key => match skip_string(bytes, i) {
                    Some(end) => {
                        i = end;
                        *state = ObjState::Colon;
                    }
                    None => return false,
                },
                ObjState::Colon => {
                    if bytes[i] != b':' {
                        return false;
                    }
                    i += 1;
                    *state = ObjState::Value;
                }
                ObjState::Value => expect_value = true,
                ObjState::Comma => match bytes[i] {
                    b',' => {
                        i += 1;
                        *state = ObjState::Key;
                    }
                    b'}' => {
                        i += 1;
                        stack.pop();
                        after_value = true;
                    }
                    _ => return false,
                },
            },
            Some(Frame::Arr(state)) => match *state {
                ArrState::First => {
                    if bytes[i] == b']' {
                        i += 1;
                        stack.pop();
                        after_value = true;
                    } else {
                        expect_value = true;
                    }
                }
                ArrState::Value => expect_value = true,
                ArrState::Comma => match bytes[i] {
                    b',' => {
                        i += 1;
                        *state = ArrState::Value;
                    }
                    b']' => {
                        i += 1;
                        stack.pop();
                        after_value = true;
                    }
                    _ => return false,
                },
            },
        }
    }
}

fn skip_string(bytes: &[u8], start: usize) -> Option<usize> {
    let n = bytes.len();
    let mut i = start + 1;
    while i < n {
        match bytes[i] {
            b'"' => return Some(i + 1),
            b'\\' => {
                i += 1;
                if i >= n {
                    return None;
                }
                match bytes[i] {
                    b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => i += 1,
                    b'u' => {
                        if i + 4 >= n {
                            return None;
                        }
                        for offset in 1..=4 {
                            if !bytes[i + offset].is_ascii_hexdigit() {
                                return None;
                            }
                        }
                        i += 5;
                    }
                    _ => return None,
                }
            }
            c if c < 0x20 => return None,
            _ => i += 1,
        }
    }
    None
}

fn expect_literal(bytes: &[u8], start: usize, literal: &[u8]) -> Option<usize> {
    let end = start + literal.len();
    if end > bytes.len() {
        return None;
    }
    if &bytes[start..end] == literal {
        Some(end)
    } else {
        None
    }
}

fn skip_number(bytes: &[u8], start: usize) -> Option<usize> {
    let n = bytes.len();
    let mut i = start;
    if i < n && bytes[i] == b'-' {
        i += 1;
    }
    if i >= n || !bytes[i].is_ascii_digit() {
        return None;
    }
    if bytes[i] == b'0' {
        i += 1;
    } else {
        while i < n && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < n && bytes[i] == b'.' {
        i += 1;
        if i >= n || !bytes[i].is_ascii_digit() {
            return None;
        }
        while i < n && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i < n && matches!(bytes[i], b'e' | b'E') {
        i += 1;
        if i < n && matches!(bytes[i], b'+' | b'-') {
            i += 1;
        }
        if i >= n || !bytes[i].is_ascii_digit() {
            return None;
        }
        while i < n && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    Some(i)
}

/// The §8 width stats: `min(graphemes(to_text(v)), 256)` per cell for the
/// first 200 rows.
#[derive(Debug, Clone, Default)]
pub struct HeadWidths {
    /// One entry per source column, indexed by source column position.
    widths: Vec<u32>,
    /// Rows counted so far (capped at 200 by the caller).
    rows: u32,
    /// Whether all 200 rows have been fed.
    done: bool,
}

impl HeadWidths {
    pub fn new(_columns: &[ColumnMeta]) -> Self {
        Self::default()
    }

    /// Feed one row of stored texts (NULL is empty).
    pub fn observe(&mut self, texts: &[String], columns: usize) {
        if self.rows >= 200 {
            return;
        }
        if self.widths.len() < columns {
            self.widths.resize(columns, 0);
        }
        for c in 0..columns {
            let text = texts.get(c).map(|t| t.as_str()).unwrap_or("");
            let graphemes = text.graphemes(true).take(256).count() as u32;
            self.widths[c] = self.widths[c].max(graphemes);
        }
        self.rows += 1;
        if self.rows == 200 {
            self.done = true;
        }
    }

    /// A snapshot copy for the Swift side.
    pub fn snapshot(&self, columns: usize) -> Vec<u32> {
        let mut out = self.widths.clone();
        if out.len() < columns {
            out.resize(columns, 0);
        }
        out
    }

    pub fn done(&self) -> bool {
        self.done
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_validate_accepts_valid_json() {
        assert!(json_validate(r#"{"a": 1, "b": [true, null, 2.5, "x"]}"#));
        assert!(json_validate("[1, 2, 3]"));
        assert!(json_validate("42"));
        assert!(json_validate("-1.5e3"));
        assert!(json_validate(r#""hello \u00e9 world""#));
    }

    #[test]
    fn json_validate_rejects_invalid_json() {
        assert!(!json_validate(""));
        assert!(!json_validate("{\"a\": }"));
        assert!(!json_validate("[1,]"));
        assert!(!json_validate("{\"a\": 1}garbage"));
        assert!(!json_validate("\"unterminated"));
    }

    #[test]
    fn truncate_utf16_short_strings_are_not_truncated() {
        let s = "hello";
        assert_eq!(truncate_utf16(s, 100), "hello");
    }

    #[test]
    fn truncate_utf16_limits_multibyte() {
        let s: String = "a".repeat(500);
        let t = truncate_utf16(&s, 100);
        assert_eq!(t.chars().count(), 100);
    }

    #[test]
    fn canonical_uuid_formats_a_uuid() {
        let hex = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            canonical_uuid(hex).as_deref(),
            Some("01234567-89ab-cdef-0123-456789abcdef")
        );
    }

    #[test]
    fn canonical_uuid_with_dashes() {
        let dashed = "01234567-89ab-cdef-0123-456789abcdef";
        assert_eq!(
            canonical_uuid(dashed).as_deref(),
            Some("01234567-89ab-cdef-0123-456789abcdef")
        );
    }

    #[test]
    fn unix_timestamp_formats_seconds() {
        assert_eq!(unix_timestamp("0").as_deref(), Some("1970-01-01 00:00:00"));
        assert_eq!(unix_timestamp("1").as_deref(), Some("1970-01-01 00:00:01"));
    }

    #[test]
    fn pretty_json_prints_sorted_keys_with_two_space_indent() {
        let text = r#"{"b":1,"a":2}"#;
        let pretty = pretty_json(text).unwrap();
        assert!(pretty.contains("\"a\" : 2"), "{pretty}");
        assert!(pretty.contains("\"b\" : 1"), "{pretty}");
        assert!(pretty.contains("\n  "), "{pretty}");
    }
}
