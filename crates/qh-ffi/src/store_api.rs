//! The data plane's FFI surface: `ResultHandle` and the records around it (blueprint
//! `fase-6-data-plane.md` section 12; ADR-0004 left the data plane to its own module).
//!
//! # What crosses the boundary
//!
//! A result lives in Rust, in a [`qh_result_store`] store. Swift holds a [`ResultHandle`] and asks
//! it for a window of rows; the answer is one packed `QHW1` buffer (section 11.1 of the blueprint,
//! built by `qh_result_store::render_window`), never a string per cell. Nothing the handle returns
//! is a pointer into Rust memory: `window` returns a copy, so a handle released on another thread
//! cannot leave Swift with a dangling reference.
//!
//! # Handle lifetime
//!
//! The handle owns a [`qh_result_store::StoreHandle`], whose `Drop` releases the store. A store is
//! also released by an explicit [`ResultHandle::release`], idempotently. Afterwards every method
//! answers [`StoreFfiError::StaleHandle`]: a [`qh_result_store::StoreId`] is never reused, and the
//! handle holds its store through an `Arc`, so a stale handle can never be pointed at another
//! result and there is no use-after-free to reach (section 9.2).
//!
//! # No panic crosses the boundary
//!
//! Every method body runs inside [`guarded`], a `catch_unwind` that turns a panic into
//! [`StoreFfiError::Internal`]. All methods return `Result`, so the Swift side sees a thrown error
//! rather than a trap. Inputs are validated first (column indices, row counts, window size), so a
//! caught panic is a bug in this crate, logged as one, and not a way for Swift to reach a panic.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use qh_core::ColumnMeta;
use qh_result_store as store;
use qh_result_store::{ChunkRef, CowCell, StoreChunk, StoreError, StoreRegistry, StoreWriter};

/// The most rows one window may ask for.
pub const MAX_WINDOW_ROWS: u32 = 4_096;
/// The most columns one window may ask for.
pub const MAX_WINDOW_COLUMNS: usize = 1_024;
/// The most cells (rows times columns) one window may ask for.
pub const MAX_WINDOW_CELLS: u64 = 262_144;
/// The biggest `rows_text` buffer; past it the caller splits its request.
pub const MAX_ROWS_TEXT_BYTES: usize = 64 * 1024 * 1024;

/// One column of a result, as the grid draws its header.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ColumnWire {
    pub name: String,
    pub type_name: String,
}

/// Where a result stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum StorePhase {
    Empty,
    Streaming,
    Complete,
    Cancelled,
    Failed,
}

/// What `row_count` answers: atomics only, no side effects, cheap enough for a display link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct RowCount {
    /// Rows the store holds.
    pub fetched: u32,
    /// Rows the current view shows.
    pub visible: u32,
    /// The current view; 0 is the identity view.
    pub view_id: u64,
    pub phase: StorePhase,
}

/// Sort by one column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct SortSpec {
    pub column: u32,
    pub descending: bool,
}

/// One filter of a view; the filters of a view are ANDed.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum FilterSpec {
    /// The cell is one of `values`; `None` stands for NULL.
    Values {
        column: u32,
        values: Vec<Option<String>>,
    },
    /// The cell matches a comparison needle (`>= 5`, `= abc`, or a substring).
    Text { column: u32, needle: String },
}

/// A view: filters, then search, then sort (the order `QueryTab.displayedRows` applies them).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ViewSpec {
    pub sort: Option<SortSpec>,
    pub filters: Vec<FilterSpec>,
    pub search: Option<String>,
}

/// A view that has been installed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct ViewInfo {
    pub view_id: u64,
    pub visible: u32,
    pub fetched: u32,
}

/// How a column's stored text is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum CellFormat {
    Raw,
    Text,
    Uuid,
    UnixTimestamp,
    Json,
}

/// The distinct values of one column.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DistinctValues {
    /// `None` is NULL.
    pub values: Vec<Option<String>>,
    /// True when the column held more than the limit; `values` is then empty.
    pub more: bool,
}

/// The registry's diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct StoreStats {
    pub stores: u32,
    pub resident_bytes: u64,
    /// Bytes written to the live stores' spill files.
    pub spilled_bytes: u64,
    pub budget_bytes: u64,
    pub spill_enabled: bool,
}

/// What the startup sweep of the spill directory did.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct StoreSweep {
    pub removed: u32,
    pub spill_enabled: bool,
    /// Why spill is off, when it is.
    pub reason: Option<String>,
}

/// What a store call can throw.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum StoreFfiError {
    /// The handle was released (the tab closed).
    #[error("result handle is stale")]
    StaleHandle,
    /// The caller's view id is not the current one; reload from `current`.
    #[error("the view changed; the current view is {current}")]
    StaleView { current: u64 },
    /// A newer `set_view` replaced this one before it finished.
    #[error("a newer view replaced this one")]
    Superseded,
    /// A sort needs every row to have arrived.
    #[error("sorting waits until every row has arrived")]
    Streaming,
    #[error("{message}")]
    TooLarge {
        needed_bytes: u64,
        budget_bytes: u64,
        message: String,
    },
    /// DiskFull, SpillAuth, SpillUnavailable, Io.
    #[error("{message}")]
    Spill { message: String },
    #[error("{message}")]
    InvalidArgument { message: String },
    #[error("{message}")]
    Corrupt { message: String },
    /// A caught panic, or a poisoned lock.
    #[error("internal error: {message}")]
    Internal { message: String },
}

impl From<StoreError> for StoreFfiError {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Released => StoreFfiError::StaleHandle,
            StoreError::StaleView { current } => StoreFfiError::StaleView { current },
            StoreError::Superseded => StoreFfiError::Superseded,
            StoreError::Streaming => StoreFfiError::Streaming,
            StoreError::TooLarge {
                needed_bytes,
                budget_bytes,
                message,
            } => StoreFfiError::TooLarge {
                needed_bytes: needed_bytes as u64,
                budget_bytes: budget_bytes as u64,
                message,
            },
            StoreError::BudgetExceeded { needed, budget } => StoreFfiError::TooLarge {
                needed_bytes: needed as u64,
                budget_bytes: budget as u64,
                message: format!("needs {needed} bytes but the budget is {budget}"),
            },
            error @ (StoreError::DiskFull
            | StoreError::SpillUnavailable { .. }
            | StoreError::SpillAuth { .. }
            | StoreError::Io(_)) => StoreFfiError::Spill {
                message: error.to_string(),
            },
            StoreError::InvalidArgument { message } => StoreFfiError::InvalidArgument { message },
            error @ (StoreError::ColumnCount { .. } | StoreError::Shape(_)) => {
                StoreFfiError::InvalidArgument {
                    message: error.to_string(),
                }
            }
            error @ (StoreError::Corrupt { .. } | StoreError::Arrow(_)) => StoreFfiError::Corrupt {
                message: error.to_string(),
            },
            StoreError::Internal { detail } => StoreFfiError::Internal { message: detail },
        }
    }
}

/// Run `body`, turning a panic into [`StoreFfiError::Internal`].
///
/// A panic that reached the FFI boundary would end the app's process (ADR-0009), so every method
/// that crosses it is wrapped. A caught panic is a bug in this crate, and the message says so.
pub(crate) fn guarded<T>(
    body: impl FnOnce() -> Result<T, StoreFfiError>,
) -> Result<T, StoreFfiError> {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(result) => result,
        Err(payload) => Err(StoreFfiError::Internal {
            message: format!(
                "a panic was caught at the FFI boundary: {}",
                panic_text(&payload)
            ),
        }),
    }
}

/// The text of a panic payload.
pub(crate) fn panic_text(payload: &Box<dyn Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panicked".to_owned())
}

fn too_large(needed: usize, limit: usize) -> StoreFfiError {
    StoreFfiError::TooLarge {
        needed_bytes: needed as u64,
        budget_bytes: limit as u64,
        message: "the text of these rows is too large for one request; ask for fewer rows"
            .to_owned(),
    }
}

fn invalid(message: impl Into<String>) -> StoreFfiError {
    StoreFfiError::InvalidArgument {
        message: message.into(),
    }
}

impl From<CellFormat> for store::ColumnFormat {
    fn from(format: CellFormat) -> Self {
        match format {
            CellFormat::Raw => store::ColumnFormat::Raw,
            CellFormat::Text => store::ColumnFormat::Text,
            CellFormat::Uuid => store::ColumnFormat::Uuid,
            CellFormat::UnixTimestamp => store::ColumnFormat::UnixTimestamp,
            CellFormat::Json => store::ColumnFormat::Json,
        }
    }
}

fn phase_of(phase: store::Phase) -> Option<StorePhase> {
    match phase {
        store::Phase::Empty => Some(StorePhase::Empty),
        store::Phase::Streaming => Some(StorePhase::Streaming),
        store::Phase::Complete => Some(StorePhase::Complete),
        store::Phase::Cancelled => Some(StorePhase::Cancelled),
        store::Phase::Failed => Some(StorePhase::Failed),
        store::Phase::Released => None,
    }
}

/// One result, held by the app for as long as its tab shows it.
#[derive(uniffi::Object)]
pub struct ResultHandle {
    handle: store::StoreHandle,
    registry: Arc<StoreRegistry>,
    /// Set by the first `run_with_store`; one store holds one run.
    run_claimed: AtomicBool,
}

impl ResultHandle {
    pub(crate) fn new(handle: store::StoreHandle, registry: Arc<StoreRegistry>) -> Arc<Self> {
        Arc::new(Self {
            handle,
            registry,
            run_claimed: AtomicBool::new(false),
        })
    }

    /// Claim the store for one run; false when a run already has it. Atomic, so two racing
    /// `run_with_store` calls cannot both pass.
    pub(crate) fn claim_run(&self) -> bool {
        !self.run_claimed.swap(true, Ordering::AcqRel)
    }

    /// The write side, for `run_with_store` and for tests driving a command with a
    /// [`StoreEmitter`](crate::StoreEmitter).
    #[doc(hidden)]
    pub fn writer(&self) -> StoreWriter {
        self.handle.writer()
    }

    /// The shared store state, for tests that inject faults (a medium that fails with ENOSPC).
    #[doc(hidden)]
    pub fn shared_for_test(&self) -> &Arc<store::StoreShared> {
        self.handle.shared()
    }

    /// The store's phase, or `None` once released.
    pub(crate) fn phase(&self) -> Option<StorePhase> {
        phase_of(self.handle.shared().phase())
    }

    /// The shared state, unless the handle was released.
    fn live(&self) -> Result<&Arc<store::StoreShared>, StoreFfiError> {
        let shared = self.handle.shared();
        if shared.phase() == store::Phase::Released {
            return Err(StoreFfiError::StaleHandle);
        }
        Ok(shared)
    }

    /// Check the shape of a window request before anything is indexed.
    fn check_request(
        shared: &store::StoreShared,
        row_count: u32,
        columns: &[u32],
    ) -> Result<Vec<usize>, StoreFfiError> {
        if row_count > MAX_WINDOW_ROWS {
            return Err(invalid(format!(
                "a window holds at most {MAX_WINDOW_ROWS} rows, not {row_count}"
            )));
        }
        if columns.len() > MAX_WINDOW_COLUMNS {
            return Err(invalid(format!(
                "a window holds at most {MAX_WINDOW_COLUMNS} columns, not {}",
                columns.len()
            )));
        }
        if u64::from(row_count) * columns.len() as u64 > MAX_WINDOW_CELLS {
            return Err(invalid(format!(
                "a window holds at most {MAX_WINDOW_CELLS} cells, not {row_count} x {}",
                columns.len()
            )));
        }
        let width = shared.column_count();
        columns
            .iter()
            .map(|&column| {
                if (column as usize) < width {
                    Ok(column as usize)
                } else {
                    Err(invalid(format!(
                        "column {column} is out of range (the result has {width})"
                    )))
                }
            })
            .collect()
    }

    /// The one window builder: `window`, `rows_text` and `cell_text` differ only in `full`
    /// (no 256-unit cut, `CUT` clear) and in the formats they pass.
    #[allow(clippy::too_many_arguments)]
    fn read_window(
        &self,
        view_id: u64,
        first_row: u32,
        row_count: u32,
        columns: &[u32],
        formats: Vec<store::ColumnFormat>,
        full: bool,
        limit: Option<usize>,
    ) -> Result<Vec<u8>, StoreFfiError> {
        let shared = self.live()?;
        let columns = Self::check_request(shared, row_count, columns)?;
        if formats.len() != columns.len() {
            return Err(invalid(format!(
                "{} columns but {} formats",
                columns.len(),
                formats.len()
            )));
        }

        // One snapshot of the view, used for the id check, the row count and the row map, so a
        // `set_view` landing in between cannot make them disagree.
        let view = shared.current_view();
        let current = view.as_ref().map_or(0, |view| view.id());
        if view_id != current {
            return Err(StoreFfiError::StaleView { current });
        }
        let visible = view
            .as_ref()
            .map_or_else(|| shared.rows(), |view| view.row_count());
        let first = first_row.min(visible);
        let count = row_count.min(visible - first);
        let sources: Vec<u32> = match &view {
            Some(view) => view.source_rows(first..first + count)?,
            None => (first..first + count).collect(),
        };

        let mut flags = 0u16;
        if !full {
            flags |= store::FLAG_CUT;
        }
        if shared.phase().is_finished() {
            flags |= store::FLAG_COMPLETE;
        }
        if view.as_ref().is_some_and(|view| !view.is_identity()) {
            flags |= store::FLAG_VIEWED;
        }

        let mut scratch = vec![CowCell::default(); columns.len()];
        let mut parts: Vec<Part> = Vec::new();
        let mut loaded: Option<(ChunkRef, Arc<StoreChunk>)> = None;
        let mut at = 0;
        let mut estimated = 0usize;
        while at < sources.len() {
            let row = sources[at];
            let reuse = loaded.as_ref().is_some_and(|(chunk, _)| {
                row >= chunk.first_row && row - chunk.first_row < chunk.rows
            });
            if !reuse {
                let Some(chunk) = shared.chunk_for_row(row) else {
                    // A release racing this read empties the chunk table; that is a stale
                    // handle, not a bad row.
                    self.live()?;
                    return Err(invalid(format!("row {row} is not in the result")));
                };
                let data = shared.load_chunk(chunk.index)?;
                loaded = Some((chunk, data));
            }
            let (chunk, data) = loaded.as_ref().expect("a chunk was just loaded");
            let end = at
                + sources[at..]
                    .iter()
                    .take_while(|&&r| r >= chunk.first_row && r - chunk.first_row < chunk.rows)
                    .count();
            if let Some(limit) = limit {
                // Before anything is materialised: the columns' share of this chunk, plus a
                // text-width allowance per cell. Rendered text can be bigger than the stored
                // form, so the actual size is checked again once the part is built.
                let share = (end - at) as f64 / f64::from(chunk.rows.max(1));
                estimated += columns
                    .iter()
                    .map(|&c| {
                        (data.batch.column(c).get_array_memory_size() as f64 * share) as usize
                    })
                    .sum::<usize>()
                    + (end - at) * columns.len() * 8;
                if estimated > limit {
                    return Err(too_large(estimated, limit));
                }
            }
            let spec = store::WindowSpec {
                rows: sources[at..end]
                    .iter()
                    .map(|r| r - chunk.first_row)
                    .collect(),
                columns: columns.clone(),
                formats: formats.clone(),
                global_flags: flags,
            };
            let window = store::render_window(data, &spec, &mut scratch, full)?;
            if let Some(limit) = limit {
                let built: usize = parts
                    .iter()
                    .map(|part| part.window.data.len())
                    .sum::<usize>()
                    + window.data.len();
                if built > limit {
                    return Err(too_large(built, limit));
                }
            }
            parts.push(Part {
                window,
                source_rows: sources[at..end].to_vec(),
            });
            at = end;
        }
        Ok(assemble(parts, columns.len() as u32, flags, first, visible))
    }
}

/// One chunk's share of a window: the rendered buffer, and the store rows it covers (the
/// buffer itself only knows chunk-local rows).
struct Part {
    window: store::Window,
    source_rows: Vec<u32>,
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Join the per-chunk buffers into one `QHW1` buffer with store-global `source_rows`.
///
/// A window inside one chunk, the common case, patches its own buffer in place.
fn assemble(mut parts: Vec<Part>, columns: u32, flags: u16, first: u32, visible: u32) -> Vec<u8> {
    if parts.len() == 1 {
        let part = parts.pop().expect("one part");
        let mut data = part.window.data;
        data[6..8].copy_from_slice(&flags.to_le_bytes());
        data[8..12].copy_from_slice(&first.to_le_bytes());
        data[20..24].copy_from_slice(&visible.to_le_bytes());
        for (index, row) in part.source_rows.iter().enumerate() {
            let at = store::HEADER_LEN + 4 * index;
            data[at..at + 4].copy_from_slice(&row.to_le_bytes());
        }
        return data;
    }

    let c = columns as usize;
    let rows: usize = parts.iter().map(|part| part.source_rows.len()).sum();
    let cells = rows * c;
    let heap: usize = parts
        .iter()
        .map(|part| part.window.heap_len() as usize)
        .sum();
    let mut out = Vec::with_capacity(store::HEADER_LEN + 4 * rows + 4 * (cells + 1) + cells + heap);
    out.extend_from_slice(b"QHW1");
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&first.to_le_bytes());
    out.extend_from_slice(&(rows as u32).to_le_bytes());
    out.extend_from_slice(&columns.to_le_bytes());
    out.extend_from_slice(&visible.to_le_bytes());
    out.extend_from_slice(&(heap as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for part in &parts {
        for row in &part.source_rows {
            out.extend_from_slice(&row.to_le_bytes());
        }
    }
    // The offsets are relative to the heap start, so each part's are shifted by the heap the
    // earlier parts already wrote. The leading 0 is written once.
    out.extend_from_slice(&0u32.to_le_bytes());
    let mut shift = 0u32;
    for part in &parts {
        let r = part.source_rows.len();
        let s0 = store::HEADER_LEN + 4 * r;
        for k in 1..=r * c {
            out.extend_from_slice(&(shift + le32(&part.window.data, s0 + 4 * k)).to_le_bytes());
        }
        shift += part.window.heap_len();
    }
    for part in &parts {
        let r = part.source_rows.len();
        let s1 = store::HEADER_LEN + 4 * r + 4 * (r * c + 1);
        out.extend_from_slice(&part.window.data[s1..s1 + r * c]);
    }
    for part in &parts {
        let r = part.source_rows.len();
        let s2 = store::HEADER_LEN + 4 * r + 4 * (r * c + 1) + r * c;
        out.extend_from_slice(&part.window.data[s2..s2 + part.window.heap_len() as usize]);
    }
    out
}

#[uniffi::export]
impl ResultHandle {
    /// Atomic loads only, no side effects: safe to call on every display-link tick.
    pub fn row_count(&self) -> Result<RowCount, StoreFfiError> {
        guarded(|| {
            let shared = self.live()?;
            let phase = phase_of(shared.phase()).ok_or(StoreFfiError::StaleHandle)?;
            // One snapshot, so `visible` and `view_id` always belong to the same view.
            let view = shared.current_view();
            Ok(RowCount {
                fetched: shared.rows(),
                visible: view
                    .as_ref()
                    .map_or_else(|| shared.rows(), |view| view.row_count()),
                view_id: view.as_ref().map_or(0, |view| view.id()),
                phase,
            })
        })
    }

    /// The columns, empty until the run has reported them.
    pub fn columns(&self) -> Result<Vec<ColumnWire>, StoreFfiError> {
        guarded(|| {
            Ok(self
                .live()?
                .columns()
                .into_iter()
                .map(|column| ColumnWire {
                    name: column.name.into_string(),
                    type_name: column.type_name.into_string(),
                })
                .collect())
        })
    }

    /// A window of the view as one `QHW1` buffer, cells cut at 256 UTF-16 units.
    pub fn window(
        &self,
        view_id: u64,
        first_row: u32,
        row_count: u32,
        columns: Vec<u32>,
        formats: Vec<CellFormat>,
    ) -> Result<Vec<u8>, StoreFfiError> {
        guarded(|| {
            self.read_window(
                view_id,
                first_row,
                row_count,
                &columns,
                formats.into_iter().map(Into::into).collect(),
                false,
                None,
            )
        })
    }

    /// The same layout with `CUT` clear and the full text of every cell, for copy and export.
    pub fn rows_text(
        &self,
        view_id: u64,
        first_row: u32,
        row_count: u32,
        columns: Vec<u32>,
    ) -> Result<Vec<u8>, StoreFfiError> {
        guarded(|| {
            let formats = vec![store::ColumnFormat::Raw; columns.len()];
            let data = self.read_window(
                view_id,
                first_row,
                row_count,
                &columns,
                formats,
                true,
                Some(MAX_ROWS_TEXT_BYTES),
            )?;
            // The parts are checked as they are built; the joined buffer adds its offsets.
            if data.len() > MAX_ROWS_TEXT_BYTES {
                return Err(too_large(data.len(), MAX_ROWS_TEXT_BYTES));
            }
            Ok(data)
        })
    }

    /// One cell's full text, `None` for NULL.
    pub fn cell_text(
        &self,
        view_id: u64,
        row: u32,
        column: u32,
        format: CellFormat,
    ) -> Result<Option<String>, StoreFfiError> {
        guarded(|| {
            let data =
                self.read_window(view_id, row, 1, &[column], vec![format.into()], true, None)?;
            // One row, one column: header, one source row, two offsets, one flag, the heap.
            if le32(&data, 12) != 1 {
                return Err(invalid(format!("row {row} is out of range")));
            }
            let flag = data[store::HEADER_LEN + 4 + 8];
            if flag & store::CELL_NULL != 0 {
                return Ok(None);
            }
            let end = le32(&data, store::HEADER_LEN + 4 + 4) as usize;
            let heap = store::HEADER_LEN + 4 + 8 + 1;
            Ok(Some(
                String::from_utf8_lossy(&data[heap..heap + end]).into_owned(),
            ))
        })
    }

    /// The width statistics for the columns, in UTF-16 units of the head of the result.
    pub fn column_widths(&self) -> Result<Vec<u32>, StoreFfiError> {
        guarded(|| Ok(self.live()?.head_widths()))
    }

    /// Install a view. Blocks while it is computed: call off the main thread.
    pub fn set_view(&self, spec: ViewSpec) -> Result<ViewInfo, StoreFfiError> {
        guarded(|| {
            let shared = self.live()?;
            let width = shared.column_count();
            let check = |column: u32| {
                if (column as usize) < width {
                    Ok(column as usize)
                } else {
                    Err(invalid(format!(
                        "column {column} is out of range (the result has {width})"
                    )))
                }
            };
            let sort = match spec.sort {
                Some(sort) => Some((check(sort.column)?, sort.descending)),
                None => None,
            };
            let filters = spec
                .filters
                .into_iter()
                .map(|filter| {
                    Ok(match filter {
                        FilterSpec::Values { column, values } => store::FilterSpec::Values {
                            column: check(column)?,
                            values,
                        },
                        FilterSpec::Text { column, needle } => store::FilterSpec::Text {
                            column: check(column)?,
                            needle,
                        },
                    })
                })
                .collect::<Result<Vec<_>, StoreFfiError>>()?;
            let view_id = shared.set_view(store::ViewSpec {
                sort,
                filters,
                search: spec.search.unwrap_or_default(),
            })?;
            Ok(ViewInfo {
                view_id,
                visible: shared.visible_rows(),
                fetched: shared.rows(),
            })
        })
    }

    /// The distinct values of one column, at most `limit` of them.
    pub fn distinct_values(
        &self,
        column: u32,
        limit: u32,
    ) -> Result<DistinctValues, StoreFfiError> {
        guarded(|| {
            let shared = self.live()?;
            let found = store::distinct_values(shared, column as usize, limit as usize)?;
            Ok(DistinctValues {
                values: found.values,
                more: found.more,
            })
        })
    }

    /// Let go of the rows now. Idempotent; every other method then answers `StaleHandle`.
    pub fn release(&self) -> Result<(), StoreFfiError> {
        guarded(|| {
            self.registry.release(self.handle.id());
            Ok(())
        })
    }
}

/// The registry's config for a budget, with the hysteresis the blueprint names (32 MiB, or an
/// eighth of a small budget so tests with a tiny budget still have room to evict into).
pub(crate) fn config_for(
    budget_bytes: usize,
    spill_dir: Option<std::path::PathBuf>,
) -> store::StoreConfig {
    let gap = (32 * 1024 * 1024).min(budget_bytes / 8);
    store::StoreConfig {
        budget_bytes,
        low_water_bytes: budget_bytes - gap,
        spill_dir,
        query_share_bytes: budget_bytes / 2,
        ..store::StoreConfig::default()
    }
}

/// `ColumnWire`s as the store's column list.
pub(crate) fn metas_of(columns: Vec<ColumnWire>) -> Vec<ColumnMeta> {
    columns
        .into_iter()
        .map(|column| ColumnMeta::new(column.name, column.type_name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_becomes_an_internal_error_and_not_an_unwind() {
        let caught: Result<(), _> = guarded(|| panic!("boom"));
        match caught {
            Err(StoreFfiError::Internal { message }) => assert!(message.contains("boom")),
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn every_store_error_has_a_typed_answer() {
        assert!(matches!(
            StoreFfiError::from(StoreError::Released),
            StoreFfiError::StaleHandle
        ));
        assert!(matches!(
            StoreFfiError::from(StoreError::DiskFull),
            StoreFfiError::Spill { .. }
        ));
        assert!(matches!(
            StoreFfiError::from(StoreError::StaleView { current: 7 }),
            StoreFfiError::StaleView { current: 7 }
        ));
    }
}
