//! The view: filter, then search, then sort, off the main thread (§13).
//!
//! The pipeline is a port of Swift's `displayedRows`, in that order, and the
//! comparator is a port of the grid's sort rather than SQL's: NULLs sort
//! last ascending, numbers compare by exact decimal value, and text orders by
//! the natural key that `collate::natural_key` also hands to the SQL
//! `qh_natural` function. Grid and SQL therefore cannot disagree.
//!
//! Everything parallel runs inside `qh_rt::view_pool()`, so a sort never
//! contends with ingest and neither touches the main thread.

use std::cmp::Ordering;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering as AtomicOrdering};
use std::sync::{Arc, RwLock};

use rayon::prelude::*;
use unicode_normalization::UnicodeNormalization;

use qh_columnar::Encoding;
use qh_core::Value;

use crate::collate::{
    ci_contains, ci_equal, natural_key, swift_double, swift_plain_number, NumKey,
};
use crate::store::{ChunkRef, StoreError, StoreShared};

/// Ascending is `Num < Temporal < Text < Null`. Descending reverses the whole
/// order, so NULLs come first, exactly like Swift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortKey {
    Num(NumKey),
    /// `(rank, value)`: dates and timestamps rank 0, times rank 1, so a
    /// midnight date sorts before any time on the same column.
    Temporal(u8, i64),
    Text(Vec<u8>),
    Null,
}

impl SortKey {
    fn of(value: &Value) -> SortKey {
        match value {
            Value::Null => SortKey::Null,
            Value::Date { days } => SortKey::Temporal(0, *days as i64),
            Value::Timestamp { micros, .. } => SortKey::Temporal(0, *micros),
            Value::Time { micros } => SortKey::Temporal(1, *micros),
            other => {
                // Everything else goes through its stored text, which is what
                // the user sees and what Swift compares. A float therefore
                // enters through its shortest representation, so float order
                // agrees with decimal order.
                let text = qh_core::render::to_text(other).unwrap_or_default();
                match swift_plain_number(&text) {
                    Some(number) => SortKey::Num(number),
                    None => {
                        let mut key = Vec::new();
                        natural_key(&text, &mut key);
                        SortKey::Text(key)
                    }
                }
            }
        }
    }
}

/// Compare two keys, ascending. Total: every pair is decided, which is what
/// lets `sort_unstable` skip its allocation and never panic.
fn cmp_keys(left: &SortKey, right: &SortKey) -> Ordering {
    fn rank(key: &SortKey) -> u8 {
        match key {
            SortKey::Num(_) => 0,
            SortKey::Temporal(..) => 1,
            SortKey::Text(_) => 2,
            SortKey::Null => 3,
        }
    }
    let (left_rank, right_rank) = (rank(left), rank(right));
    if left_rank != right_rank {
        return left_rank.cmp(&right_rank);
    }
    match (left, right) {
        (SortKey::Num(a), SortKey::Num(b)) => a.cmp(b),
        (SortKey::Temporal(ra, a), SortKey::Temporal(rb, b)) => ra.cmp(rb).then(a.cmp(b)),
        (SortKey::Text(a), SortKey::Text(b)) => a.cmp(b),
        _ => Ordering::Equal,
    }
}

/// A per-column value filter, port of `ColumnFilter.matches`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterSpec {
    /// A cell matches when its text is one of these, or when it is NULL and
    /// `None` is in the list. Compared after NFC, because Swift `String`
    /// equality honours canonical equivalence.
    Values {
        column: usize,
        values: Vec<Option<String>>,
    },
    /// A comparison needle, ported from `matchesText` line by line.
    Text { column: usize, needle: String },
}

impl FilterSpec {
    fn column(&self) -> usize {
        match self {
            FilterSpec::Values { column, .. } | FilterSpec::Text { column, .. } => *column,
        }
    }

    fn matches(&self, value: &Value) -> bool {
        match self {
            FilterSpec::Values { values, .. } => match value {
                Value::Null => values.iter().any(|entry| entry.is_none()),
                other => {
                    let text = qh_core::render::to_text(other).unwrap_or_default();
                    let text = text.nfc().collect::<String>();
                    values
                        .iter()
                        .flatten()
                        .any(|entry| entry.nfc().collect::<String>() == text)
                }
            },
            FilterSpec::Text { needle, .. } => match value {
                // A NULL cell has no text, so no needle matches it.
                Value::Null => false,
                other => {
                    let text = qh_core::render::to_text(other).unwrap_or_default();
                    matches_text(&text, needle)
                }
            },
        }
    }
}

/// Port of `ColumnFilter.matchesText`.
///
/// `needle` is trimmed; an empty trimmed needle matches everything. Then the
/// five comparison operators are tried in the order `>=`, `<=`, `>`, `<`, `=`.
/// When one is a prefix, the rest is trimmed into an operand; an empty
/// operand breaks out and the whole trimmed needle is used as a substring
/// test. `=` is case-insensitive; the others compare as doubles when both
/// sides parse, and as NFC-normalized strings otherwise.
pub fn matches_text(value: &str, needle: &str) -> bool {
    let trimmed = trim_white_space(needle);
    if trimmed.is_empty() {
        return true;
    }
    for op in [">=", "<=", ">", "<", "="] {
        let Some(rest) = trimmed.strip_prefix(op) else {
            continue;
        };
        let operand = trim_white_space(rest);
        if operand.is_empty() {
            break;
        }
        return match op {
            "=" => ci_equal(value, operand),
            _ => match (swift_double(value), swift_double(operand)) {
                (Some(left), Some(right)) => compare_f64(op, left, right),
                _ => compare_string(op, value, operand),
            },
        };
    }
    ci_contains(value, trimmed)
}

fn compare_f64(op: &str, left: f64, right: f64) -> bool {
    match op {
        ">=" => left >= right,
        "<=" => left <= right,
        ">" => left > right,
        "<" => left < right,
        _ => false,
    }
}

fn compare_string(op: &str, left: &str, right: &str) -> bool {
    let left = left.nfc().collect::<String>();
    let right = right.nfc().collect::<String>();
    // Rust's `str` ordering is by code point, which is the scalar order
    // Swift's `<` compares.
    match op {
        ">=" => left >= right,
        "<=" => left <= right,
        ">" => left > right,
        "<" => left < right,
        _ => false,
    }
}

/// Trim Unicode whitespace, the same set Swift's `White_Space` names.
///
/// Byte offsets from `char_indices`, not char counts: a leading multi-byte
/// whitespace (NBSP, ideographic space) would otherwise be sliced mid-char.
fn trim_white_space(s: &str) -> &str {
    let is_white = |c: char| c.is_whitespace() || matches!(c, '\u{85}' | '\u{2028}' | '\u{2029}');
    let start = s
        .char_indices()
        .find(|(_, c)| !is_white(*c))
        .map(|(index, _)| index)
        .unwrap_or(s.len());
    let end = s
        .char_indices()
        .rev()
        .find(|(_, c)| !is_white(*c))
        .map(|(index, c)| index + c.len_utf8())
        .unwrap_or(0);
    if start >= end {
        ""
    } else {
        &s[start..end]
    }
}

/// One view request.
#[derive(Debug, Clone, Default)]
pub struct ViewSpec {
    /// Sort by this column; `None` leaves the source order.
    pub sort: Option<(usize, bool)>,
    /// AND across columns.
    pub filters: Vec<FilterSpec>,
    /// Case-insensitive substring over every column, NULLs excluded.
    pub search: String,
}

impl ViewSpec {
    /// An empty spec is the identity view: no permutation, no scan.
    pub fn is_identity(&self) -> bool {
        self.sort.is_none() && self.filters.is_empty() && trim_white_space(&self.search).is_empty()
    }

    fn needs_rows(&self) -> bool {
        !self.filters.is_empty() || !trim_white_space(&self.search).is_empty()
    }
}

/// The result of installing a view.
#[derive(Debug, Clone)]
pub struct ViewInfo {
    pub id: u64,
    /// Source rows in view order. An identity view's rows are `0..fetched`,
    /// which the caller can build itself; this is the filtered or sorted list.
    pub rows: Vec<u32>,
    /// Rows fetched when the view was computed.
    pub fetched: u32,
    /// Total after filtering and searching, before the sort (its length).
    pub visible: u32,
    /// True when this view is not the identity, so the window header can set
    /// the `VIEWED` flag.
    pub is_identity: bool,
    /// True when the result was still streaming, so the grid knows the row
    /// count will grow.
    pub streaming: bool,
}

/// The distinct values of one column, port of
/// `ColumnFilter.distinctValues`.
#[derive(Debug, Clone)]
pub struct DistinctValues {
    pub values: Vec<Option<String>>,
    /// True when the column had more distinct values than the limit; the list
    /// is then empty, and the picker keeps its own rule.
    pub more: bool,
}

/// Scan one chunk for rows passing the filters and the search, appending
/// source-row indices. Every filter must pass, and at least one non-NULL cell
/// in any column must contain the search term.
pub fn scan_chunk(
    reference: &ChunkRef,
    chunk: &crate::chunk::StoreChunk,
    spec: &ViewSpec,
    out: &mut Vec<u32>,
) {
    let width = chunk.batch.num_columns();
    let search = trim_white_space(&spec.search);
    for row in 0..reference.rows as usize {
        let source = reference.first_row + row as u32;
        if !spec
            .filters
            .iter()
            .all(|filter| match value_of(chunk, filter.column(), row) {
                Some(value) => filter.matches(&value),
                // A column past the chunk's width has no value; a filter on it
                // matches nothing, which is Swift's answer for a missing cell.
                None => false,
            })
        {
            continue;
        }
        if !search.is_empty() {
            let mut hit = false;
            for column in 0..width {
                if let Some(value) = value_of(chunk, column, row) {
                    if value.is_null() {
                        continue;
                    }
                    let text = qh_core::render::to_text(&value).unwrap_or_default();
                    if ci_contains(&text, search) {
                        hit = true;
                        break;
                    }
                }
            }
            if !hit {
                continue;
            }
        }
        out.push(source);
    }
}

/// One cell as a `Value`, or `None` when the column is out of range.
///
/// The column is looked up with `get`, never `column`: a filter or a window
/// that names a column past the result width must be an error, not a panic.
fn value_of(chunk: &crate::chunk::StoreChunk, column: usize, row: usize) -> Option<Value> {
    let array = chunk.batch.columns().get(column)?;
    let enc = chunk
        .encodings
        .get(column)
        .copied()
        .unwrap_or(Encoding::Null);
    qh_columnar::value_at(array.as_ref(), enc, row).ok()
}

/// Refuse a spec that names a column past the result width, before any
/// `batch.column` call. §11.5: out of range is `InvalidArgument`.
fn check_columns(spec: &ViewSpec, width: usize) -> Result<(), StoreError> {
    if let Some((column, _)) = spec.sort {
        if column >= width {
            return Err(StoreError::InvalidArgument {
                message: format!("sort column {column} is out of range (width {width})"),
            });
        }
    }
    for filter in &spec.filters {
        if filter.column() >= width {
            return Err(StoreError::InvalidArgument {
                message: format!(
                    "filter column {} is out of range (width {width})",
                    filter.column()
                ),
            });
        }
    }
    Ok(())
}

/// Compute a view from a snapshot. Called on the view pool, or from a test.
pub fn compute_view(store: &StoreShared, spec: &ViewSpec) -> Result<ViewInfo, StoreError> {
    let fetched = store.rows();
    let streaming = !store.phase().is_finished();
    let width = store.column_count();

    if spec.is_identity() {
        return Ok(ViewInfo {
            id: 0,
            rows: Vec::new(),
            fetched,
            visible: fetched,
            is_identity: true,
            streaming,
        });
    }

    // Refuse an out-of-range column before touching a chunk: the detached
    // expansion task has no FFI guard, so a panic here would abort the app.
    check_columns(spec, width)?;

    // A sort needs every row. Refusing mid-stream is honest: sorting a
    // partial result would show an order that changes as more arrives.
    if spec.sort.is_some() && streaming {
        return Err(StoreError::Streaming);
    }

    let references = store.chunk_refs();
    let chunks: Vec<Arc<crate::chunk::StoreChunk>> = references
        .iter()
        .map(|reference| store.load_chunk(reference.index))
        .collect::<Result<Vec<_>, _>>()?;

    let mut rows: Vec<u32> = Vec::new();
    if spec.needs_rows() {
        // Each chunk yields its own rows and they are concatenated in chunk
        // order, so source order is preserved before the sort.
        let per_chunk: Vec<Vec<u32>> = qh_rt::view_pool().install(|| {
            references
                .par_iter()
                .zip(chunks.par_iter())
                .map(|(reference, chunk)| {
                    let mut matched = Vec::new();
                    scan_chunk(reference, chunk, spec, &mut matched);
                    matched
                })
                .collect()
        });
        for mut chunk_rows in per_chunk {
            rows.append(&mut chunk_rows);
        }
    } else {
        rows.extend(0..fetched);
    }

    let visible = rows.len() as u32;

    if let Some((column, descending)) = spec.sort {
        // Keys are built per chunk in parallel, then sorted once. The key
        // travels beside its row so the comparator is a slice lookup rather
        // than a re-read of the chunk. Membership is a bitmap rather than a
        // linear scan: a filtered sort over five million rows must not be
        // quadratic.
        let mut included = vec![false; fetched as usize];
        for &row in &rows {
            if let Some(slot) = included.get_mut(row as usize) {
                *slot = true;
            }
        }
        let included = &included;
        let keys: Vec<(u32, SortKey)> = qh_rt::view_pool().install(|| {
            references
                .par_iter()
                .zip(chunks.par_iter())
                .flat_map_iter(|(reference, chunk)| {
                    (0..reference.rows as usize).map(move |row| {
                        let source = reference.first_row + row as u32;
                        let value = value_of(chunk, column, row).unwrap_or(Value::Null);
                        (source, SortKey::of(&value))
                    })
                })
                .filter(|(source, _)| included.get(*source as usize).copied().unwrap_or(false))
                .collect()
        });
        rows = qh_rt::view_pool().install(|| {
            let mut paired = keys;
            paired.sort_unstable_by(|(row_a, key_a), (row_b, key_b)| {
                let mut order = cmp_keys(key_a, key_b);
                if descending {
                    order = order.reverse();
                }
                // The tie-break never reverses: Swift's `left.offset <
                // right.offset` holds in both directions.
                order.then(row_a.cmp(row_b))
            });
            paired.into_iter().map(|(row, _)| row).collect()
        });
    }

    Ok(ViewInfo {
        id: 0,
        rows,
        fetched,
        visible,
        is_identity: false,
        streaming,
    })
}

/// Scan the fetched rows for one column's distinct values.
///
/// Scans store order, not view order, exactly like `preview.rows`: the picker
/// offers what the column holds, not what the current filter shows.
pub fn distinct_values(
    store: &StoreShared,
    column: usize,
    limit: usize,
) -> Result<DistinctValues, StoreError> {
    if column >= store.column_count() {
        return Err(StoreError::InvalidArgument {
            message: format!(
                "distinct column {column} is out of range (width {})",
                store.column_count()
            ),
        });
    }
    let references = store.chunk_refs();
    let chunks: Vec<Arc<crate::chunk::StoreChunk>> = references
        .iter()
        .map(|reference| store.load_chunk(reference.index))
        .collect::<Result<Vec<_>, _>>()?;

    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut has_null = false;
    let mut more = false;

    'outer: for (index, reference) in references.iter().enumerate() {
        let chunk = &chunks[index];
        for row in 0..reference.rows as usize {
            match value_of(chunk, column, row) {
                Some(value) if !value.is_null() => {
                    let text = qh_core::render::to_text(&value).unwrap_or_default();
                    seen.insert(text.nfc().collect::<String>());
                }
                Some(_) => has_null = true,
                None => {}
            }
            // Stop as soon as the picker could not use the answer anyway, so
            // a million distinct values does not build a million-entry set.
            if seen.len() + usize::from(has_null) > limit {
                more = true;
                break 'outer;
            }
        }
    }

    if more {
        return Ok(DistinctValues {
            values: Vec::new(),
            more: true,
        });
    }
    let mut values: Vec<Option<String>> = Vec::with_capacity(seen.len() + 1);
    if has_null {
        values.push(None);
    }
    values.extend(seen.into_iter().map(Some));
    Ok(DistinctValues {
        values,
        more: false,
    })
}

/// A published view. The reader swaps its pointer atomically; the writer
/// expands a streaming filter view and cancels it when superseded.
pub struct View {
    pub id: u64,
    rows: RwLock<Vec<u32>>,
    len: AtomicU32,
    cancel: AtomicBool,
    scanned: AtomicU32,
    expanding: AtomicBool,
    spec: ViewSpec,
    identity: bool,
    expandable: bool,
}

impl View {
    fn new(id: u64, spec: ViewSpec, info: ViewInfo, streaming: bool) -> Self {
        let identity = spec.is_identity();
        let expandable = streaming && !identity && spec.sort.is_none() && spec.needs_rows();
        let rows = info.rows;
        let len = if identity {
            info.fetched
        } else {
            rows.len() as u32
        };
        Self {
            id,
            rows: RwLock::new(rows),
            len: AtomicU32::new(len),
            cancel: AtomicBool::new(false),
            scanned: AtomicU32::new(0),
            expanding: AtomicBool::new(false),
            spec,
            identity,
            expandable,
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn is_identity(&self) -> bool {
        self.identity
    }

    pub fn row_count(&self) -> u32 {
        self.len.load(AtomicOrdering::Acquire)
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(AtomicOrdering::Acquire)
    }

    /// Set by `release` or by the next `set_view`.
    pub fn cancel(&self) {
        self.cancel.store(true, AtomicOrdering::Release);
    }

    pub fn spec(&self) -> &ViewSpec {
        &self.spec
    }

    /// The rows in view order, or `Superseded` if a newer view took over.
    pub fn rows(&self) -> Result<Vec<u32>, StoreError> {
        if self.is_cancelled() {
            return Err(StoreError::Superseded);
        }
        self.rows
            .read()
            .map(|rows| rows.clone())
            .map_err(|_| StoreError::Internal {
                detail: "view rows poisoned".to_owned(),
            })
    }

    /// Map a run of view positions to source rows.
    pub fn source_rows(&self, positions: std::ops::Range<u32>) -> Result<Vec<u32>, StoreError> {
        if self.is_cancelled() {
            return Err(StoreError::Superseded);
        }
        if self.identity {
            return Ok(positions.collect());
        }
        let rows = self.rows.read().map_err(|_| StoreError::Internal {
            detail: "view rows poisoned".to_owned(),
        })?;
        let start = (positions.start as usize).min(rows.len());
        let end = (positions.end as usize).min(rows.len());
        Ok(rows[start..end].to_vec())
    }

    pub(crate) fn scanned_chunks(&self) -> u32 {
        self.scanned.load(AtomicOrdering::Acquire)
    }

    pub(crate) fn set_scanned_chunks(&self, count: u32) {
        self.scanned.store(count, AtomicOrdering::Release);
    }

    /// True when a streaming filter view still owes rows.
    pub(crate) fn is_expandable(&self) -> bool {
        self.expandable
    }

    /// Claim the right to run one expansion. False when another task already
    /// holds it, so the writer never stacks tasks.
    pub(crate) fn begin_expanding(&self) -> bool {
        !self.expanding.swap(true, AtomicOrdering::AcqRel)
    }

    pub(crate) fn end_expanding(&self) {
        self.expanding.store(false, AtomicOrdering::Release);
    }

    pub(crate) fn append_rows(&self, rows: &[u32]) {
        if rows.is_empty() {
            return;
        }
        if let Ok(mut current) = self.rows.write() {
            current.extend_from_slice(rows);
            self.len
                .store(current.len() as u32, AtomicOrdering::Release);
        }
    }
}

/// Build and publish a view. The writer swaps its pointer atomically; the
/// old view answers `Superseded`.
pub fn build_view(
    store: &Arc<StoreShared>,
    spec: ViewSpec,
    id: u64,
) -> Result<Arc<View>, StoreError> {
    let streaming = !store.phase().is_finished();
    let info = qh_rt::view_pool().install(|| compute_view(store, &spec))?;
    Ok(Arc::new(View::new(id, spec, info, streaming)))
}
