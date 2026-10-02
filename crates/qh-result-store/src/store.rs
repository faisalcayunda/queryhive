//! The store's shared state: chunks, phase, budget accounting, and spill.
//!
//! One `StoreShared` per result. Writers seal chunks and publish them under a
//! short write lock; readers take a read lock, clone the `Arc` they need, and
//! let go. Chunk residency (memory or spill record) is behind its own mutex so
//! a reader never waits on a spill write, and the row count is an atomic so
//! the grid can ask how many rows exist without taking a lock at all.
//!
//! `StoreError` is the one error enum for the whole crate; every module maps
//! its own failures into it rather than inventing a second error type.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

use qh_columnar::{ChunkBuilder, Encoding, SealedChunk};
use qh_core::{ColumnBatch, ColumnMeta, Value};
use rayon::prelude::*;
use thiserror::Error;

use crate::chunk::{seal_store_chunk, ColumnStats, StoreChunk};
use crate::registry::{Origin, StoreId, StoreRegistry};
use crate::render::HeadWidths;
use crate::spill::{decode_record, encode_record, SpillFile};
use crate::view::{build_view, View, ViewSpec};

/// Everything that can go wrong reading or writing the store.
#[derive(Debug, Error)]
pub enum StoreError {
    /// A record is not one this build can read: a truncated or altered spill
    /// record, or a file from a build whose layout has since moved. Carries
    /// the detail so a diagnostics bundle says more than "corrupt".
    #[error("the encoded batch is damaged: {detail}")]
    Corrupt { detail: String },

    /// The spill file could not be written or read.
    #[error("spill file: {0}")]
    Io(#[from] std::io::Error),

    /// A batch whose shape does not match the store's columns.
    #[error(transparent)]
    Shape(#[from] qh_core::BatchError),

    /// A pushed batch had a different width from the store's column list.
    #[error("batch has {found} columns but the result declares {expected}")]
    ColumnCount { expected: usize, found: usize },

    /// Spill is unavailable (RNG failed, nonce space exhausted, spill off).
    #[error("spill unavailable: {reason}")]
    SpillUnavailable { reason: String },

    /// Spill record authentication failed.
    #[error("spill authentication failed for chunk {chunk}")]
    SpillAuth { chunk: u32 },

    /// Internal error (IPC write failure, a builder that refused its own
    /// values, ...).
    #[error("internal error: {detail}")]
    Internal { detail: String },

    /// Disk full during spill write.
    #[error("disk full")]
    DiskFull,

    /// Arrow refused a schema, batch, or array operation.
    #[error("arrow: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),

    /// The memory budget cannot accommodate the requested allocation.
    #[error("needs {needed} bytes but the budget is {budget}")]
    BudgetExceeded { needed: usize, budget: usize },

    /// The store handle has been released (tab closed).
    #[error("store handle is stale")]
    Released,

    /// A window asked for a view id that is no longer current.
    #[error("the view moved on (current {current})")]
    StaleView { current: u64 },

    /// A newer view replaced this one before it finished.
    #[error("a newer view replaced this one")]
    Superseded,

    /// A sort was asked for while rows are still arriving.
    #[error("the result is still streaming; sorting needs it finished")]
    Streaming,

    /// A view needed more scratch than the budget could give it.
    #[error("{message} (needs {needed_bytes} bytes, budget is {budget_bytes})")]
    TooLarge {
        needed_bytes: usize,
        budget_bytes: usize,
        message: String,
    },

    /// A request that cannot be satisfied as asked.
    #[error("invalid argument: {message}")]
    InvalidArgument { message: String },
}

/// The lifecycle of a result. Only `Empty` and `Streaming` accept writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Empty,
    Streaming,
    Complete,
    Cancelled,
    Failed,
    Released,
}

impl Phase {
    fn as_u8(self) -> u8 {
        match self {
            Phase::Empty => 0,
            Phase::Streaming => 1,
            Phase::Complete => 2,
            Phase::Cancelled => 3,
            Phase::Failed => 4,
            Phase::Released => 5,
        }
    }

    fn from_u8(value: u8) -> Self {
        match value {
            1 => Phase::Streaming,
            2 => Phase::Complete,
            3 => Phase::Cancelled,
            4 => Phase::Failed,
            5 => Phase::Released,
            _ => Phase::Empty,
        }
    }

    /// A finished result: a sort may run over it.
    pub fn is_finished(self) -> bool {
        matches!(self, Phase::Complete | Phase::Cancelled | Phase::Failed)
    }
}

/// How a result ended.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// The query ran to completion. `truncated` means a row limit cut it off.
    Complete { truncated: bool },
    /// The user cancelled.
    Cancelled,
    /// The query failed; the message is for the log, not the store.
    Failed { detail: String },
}

/// One chunk in the index: where it starts, how many rows, and where the
/// bytes live.
pub struct ChunkEntry {
    pub first_row: u32,
    pub rows: u32,
    pub cell: Arc<ChunkCell>,
}

/// A chunk's residency and its last-access stamp. The stamp is the logical
/// clock's value, used only to order eviction.
pub struct ChunkCell {
    pub residency: Mutex<Residency>,
    pub last_access: AtomicU64,
}

/// Where a chunk's bytes are.
pub enum Residency {
    /// In memory, ready to read.
    Resident(Arc<StoreChunk>),
    /// An encrypted record in the store's spill file. `bytes` is the resident
    /// size that was charged, so releasing it gives back the same number.
    Spilled {
        offset: u64,
        len: u32,
        nonce: u64,
        bytes: usize,
    },
}

/// The shared state of one result.
pub struct StoreShared {
    id: StoreId,
    registry: Weak<StoreRegistry>,
    columns: RwLock<Vec<ColumnMeta>>,
    chunks: RwLock<Vec<ChunkEntry>>,
    stats: Mutex<Vec<ColumnStats>>,
    rows: AtomicU32,
    phase: AtomicU8,
    head_widths: Mutex<HeadWidths>,
    head_rows: AtomicU32,
    builder: Mutex<Option<ChunkBuilder>>,
    spill: Mutex<Option<SpillFile>>,
    /// Test seam: a medium to hand to the next spill file instead of a real
    /// one, so the disk-full path can be exercised.
    #[doc(hidden)]
    pub faulty_medium: Mutex<Option<Box<dyn crate::spill::SpillMedium>>>,
    view: RwLock<Option<Arc<View>>>,
    view_seq: AtomicU64,
    truncated: AtomicBool,
}

impl StoreShared {
    /// A store with an identity and a weak link back to its registry. The
    /// registry registers it separately, because the `Arc` does not exist
    /// until this returns.
    pub(crate) fn new(id: StoreId, registry: Weak<StoreRegistry>) -> Self {
        Self {
            id,
            registry,
            columns: RwLock::new(Vec::new()),
            chunks: RwLock::new(Vec::new()),
            stats: Mutex::new(Vec::new()),
            rows: AtomicU32::new(0),
            phase: AtomicU8::new(Phase::Empty.as_u8()),
            head_widths: Mutex::new(HeadWidths::default()),
            head_rows: AtomicU32::new(0),
            builder: Mutex::new(None),
            spill: Mutex::new(None),
            faulty_medium: Mutex::new(None),
            view: RwLock::new(None),
            view_seq: AtomicU64::new(0),
            truncated: AtomicBool::new(false),
        }
    }

    pub fn id(&self) -> StoreId {
        self.id
    }

    pub fn rows(&self) -> u32 {
        self.rows.load(Ordering::Acquire)
    }

    pub fn phase(&self) -> Phase {
        Phase::from_u8(self.phase.load(Ordering::Acquire))
    }

    /// Whether a row limit cut the result short.
    pub fn is_truncated(&self) -> bool {
        self.truncated.load(Ordering::Acquire)
    }

    pub fn columns(&self) -> Vec<ColumnMeta> {
        self.columns
            .read()
            .map(|columns| columns.clone())
            .unwrap_or_default()
    }

    pub fn column_count(&self) -> usize {
        self.columns
            .read()
            .map(|columns| columns.len())
            .unwrap_or(0)
    }

    /// The running per-column statistics, for the logical schema.
    pub fn stats(&self) -> Vec<ColumnStats> {
        self.stats
            .lock()
            .map(|stats| stats.clone())
            .unwrap_or_default()
    }

    /// The §8 width snapshot: the widest first 200 rows, capped at 256
    /// graphemes.
    pub fn head_widths(&self) -> Vec<u32> {
        let columns = self.column_count();
        self.head_widths
            .lock()
            .map(|widths| widths.snapshot(columns))
            .unwrap_or_else(|_| vec![0; columns])
    }

    /// The registry this store belongs to, or `Released` once it is gone.
    fn registry(&self) -> Result<Arc<StoreRegistry>, StoreError> {
        self.registry.upgrade().ok_or(StoreError::Released)
    }

    fn tick(&self) -> u64 {
        self.registry.upgrade().map(|r| r.tick()).unwrap_or(0)
    }

    /// The current view, or `None` for the implicit identity view.
    pub fn current_view(&self) -> Option<Arc<View>> {
        self.view.read().ok().and_then(|view| view.clone())
    }

    /// Rows visible under the current view.
    pub fn visible_rows(&self) -> u32 {
        match self.current_view() {
            Some(view) => view.row_count(),
            None => self.rows(),
        }
    }

    /// The view's id, or 0 for the identity view.
    pub fn view_id(&self) -> u64 {
        self.current_view().map(|view| view.id()).unwrap_or(0)
    }

    // --- write side -----------------------------------------------------

    /// Declare the column list. Once per store.
    pub fn begin(
        &self,
        registry: &Arc<StoreRegistry>,
        columns: Vec<ColumnMeta>,
    ) -> Result<(), StoreError> {
        self.check_writable()?;
        let mut current = self.columns.write().map_err(|_| StoreError::Internal {
            detail: "columns lock poisoned".to_owned(),
        })?;
        if !current.is_empty() {
            if current.len() == columns.len() {
                return Ok(());
            }
            return Err(StoreError::InvalidArgument {
                message: "begin was already called with a different width".to_owned(),
            });
        }
        let width = columns.len();
        if width == 0 {
            return Err(StoreError::InvalidArgument {
                message: "a result needs at least one column".to_owned(),
            });
        }
        *current = columns;
        drop(current);
        let mut builder = self.builder.lock().map_err(|_| StoreError::Internal {
            detail: "builder lock poisoned".to_owned(),
        })?;
        *builder = Some(ChunkBuilder::new(width));
        self.stats
            .lock()
            .map(|mut stats| {
                if stats.is_empty() {
                    *stats = vec![ColumnStats::default(); width];
                }
            })
            .ok();
        let _ = registry;
        Ok(())
    }

    /// Append a batch: stage rows, seal at the limits, publish each chunk.
    pub fn push(
        self: &Arc<Self>,
        registry: &Arc<StoreRegistry>,
        batch: &ColumnBatch,
    ) -> Result<(), StoreError> {
        self.check_writable()?;
        let width = self.column_count();
        if width == 0 {
            return Err(StoreError::InvalidArgument {
                message: "begin was not called before push".to_owned(),
            });
        }
        if batch.width() != width {
            return Err(StoreError::ColumnCount {
                expected: width,
                found: batch.width(),
            });
        }
        self.set_phase(Phase::Streaming);

        let mut sealed: Vec<SealedChunk> = Vec::new();
        {
            let mut guard = self.builder.lock().map_err(|_| StoreError::Internal {
                detail: "builder lock poisoned".to_owned(),
            })?;
            let builder = guard.as_mut().ok_or_else(|| StoreError::Internal {
                detail: "builder missing after begin".to_owned(),
            })?;
            for row in 0..batch.rows() {
                if builder.is_full() {
                    let full = std::mem::replace(builder, ChunkBuilder::new(width));
                    sealed.push(full.seal().map_err(builder_error)?);
                }
                for column in 0..width {
                    let cell = batch.value(row, column).cloned().unwrap_or(Value::Null);
                    builder.push_value(column, cell);
                }
            }
            // One push, one seal (§6.2): the rows this push staged are
            // published before it returns, so a stream that trickles is
            // visible the moment its batch arrives and `finish` has nothing
            // left to seal. A batch past the limits seals more than once
            // above; this catches the trailing part.
            if !builder.is_empty() {
                let staged = std::mem::replace(builder, ChunkBuilder::new(width));
                sealed.push(staged.seal().map_err(builder_error)?);
            }
        }
        for chunk in sealed {
            self.publish(registry, chunk)?;
        }
        self.feed_head_widths(batch);
        self.maybe_expand_view();
        Ok(())
    }

    /// Append an already-sealed chunk (DataFusion output, a driver batch).
    pub fn push_chunk(
        self: &Arc<Self>,
        registry: &Arc<StoreRegistry>,
        chunk: SealedChunk,
    ) -> Result<(), StoreError> {
        self.check_writable()?;
        let width = self.column_count();
        if width == 0 {
            return Err(StoreError::InvalidArgument {
                message: "begin was not called before push_chunk".to_owned(),
            });
        }
        // The chunk is untrusted output (DataFusion, a driver, the helper), so
        // its width must match the store's column list. A mismatch would make
        // `seal_store_chunk`'s zip silently drop columns and a later read
        // panic on the index.
        if chunk.batch.num_columns() != width {
            return Err(StoreError::ColumnCount {
                expected: width,
                found: chunk.batch.num_columns(),
            });
        }
        self.set_phase(Phase::Streaming);
        self.publish(registry, chunk)?;
        self.maybe_expand_view();
        Ok(())
    }

    /// Finish the result: seal whatever is staged, then set the phase.
    pub fn finish(&self, outcome: Outcome) -> Result<(), StoreError> {
        if self.phase() == Phase::Released {
            return Err(StoreError::Released);
        }
        let registry = self.registry()?;
        let pending = {
            let mut guard = self.builder.lock().map_err(|_| StoreError::Internal {
                detail: "builder lock poisoned".to_owned(),
            })?;
            guard.take().filter(|builder| !builder.is_empty())
        };
        if let Some(builder) = pending {
            let sealed = builder.seal().map_err(builder_error)?;
            self.publish(&registry, sealed)?;
        }
        match outcome {
            Outcome::Complete { truncated } => {
                self.truncated.store(truncated, Ordering::Release);
                self.set_phase(Phase::Complete);
            }
            Outcome::Cancelled => self.set_phase(Phase::Cancelled),
            Outcome::Failed { .. } => self.set_phase(Phase::Failed),
        }
        // A streaming filter view may still owe rows; do one final scan now
        // that no more are coming.
        self.expand_view_now();
        Ok(())
    }

    fn check_writable(&self) -> Result<(), StoreError> {
        match self.phase() {
            Phase::Released => Err(StoreError::Released),
            Phase::Empty | Phase::Streaming => Ok(()),
            _ => Err(StoreError::InvalidArgument {
                message: "the result is already finished".to_owned(),
            }),
        }
    }

    fn set_phase(&self, phase: Phase) {
        // Never move backwards out of a terminal phase, and never past
        // Released.
        let current = self.phase();
        if current == Phase::Released {
            return;
        }
        if current.is_finished() && !phase.is_finished() {
            return;
        }
        self.phase.store(phase.as_u8(), Ordering::Release);
    }

    /// Seal one chunk into the index and charge the budget.
    fn publish(
        &self,
        registry: &Arc<StoreRegistry>,
        sealed: SealedChunk,
    ) -> Result<(), StoreError> {
        let columns = self.columns();
        let (chunk, deltas) = seal_store_chunk(sealed, &columns)?;
        let rows = chunk.batch.num_rows() as u32;
        let bytes = chunk.bytes;
        // Charge first: a budget that cannot take this chunk fails the push
        // before anything is published, so the row count stays honest.
        registry.charge(bytes, Origin::Writer)?;
        let index = self
            .chunks
            .read()
            .map(|chunks| chunks.len() as u32)
            .unwrap_or(0);
        let first_row = self.rows.load(Ordering::Acquire);
        let entry = ChunkEntry {
            first_row,
            rows,
            cell: Arc::new(ChunkCell {
                residency: Mutex::new(Residency::Resident(Arc::new(chunk))),
                last_access: AtomicU64::new(self.tick()),
            }),
        };
        {
            let mut chunks = self.chunks.write().map_err(|_| StoreError::Internal {
                detail: "chunks lock poisoned".to_owned(),
            })?;
            chunks.push(entry);
        }
        if let Ok(mut stats) = self.stats.lock() {
            merge_stats(&mut stats, &deltas);
        }
        // Publish rows last, with Release: a reader that sees the row count
        // sees the chunk behind it.
        self.rows.store(first_row + rows, Ordering::Release);
        let _ = index;
        Ok(())
    }

    /// Feed the width stats from a batch, for the first 200 rows only.
    fn feed_head_widths(&self, batch: &ColumnBatch) {
        let width = batch.width();
        let mut guard = match self.head_widths.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        let mut seen = self.head_rows.load(Ordering::Acquire);
        let mut texts: Vec<String> = Vec::with_capacity(width);
        for row in 0..batch.rows() {
            if seen >= 200 {
                break;
            }
            texts.clear();
            for column in 0..width {
                let value = batch.value(row, column).cloned().unwrap_or(Value::Null);
                texts.push(qh_core::render::to_text(&value).unwrap_or_default());
            }
            guard.observe(&texts, width);
            seen += 1;
        }
        self.head_rows.store(seen, Ordering::Release);
    }

    // --- read side ------------------------------------------------------

    /// A snapshot of the chunk index: enough to find a row without holding a
    /// lock across the read.
    pub fn chunk_refs(&self) -> Vec<ChunkRef> {
        match self.chunks.read() {
            Ok(chunks) => chunks
                .iter()
                .enumerate()
                .map(|(index, entry)| ChunkRef {
                    index: index as u32,
                    first_row: entry.first_row,
                    rows: entry.rows,
                    cell: Arc::clone(&entry.cell),
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// The chunk holding `row`, by binary search over `first_row`.
    pub fn chunk_for_row(&self, row: u32) -> Option<ChunkRef> {
        let chunks = self.chunks.read().ok()?;
        let position = chunks.partition_point(|entry| entry.first_row <= row);
        let entry = chunks.get(position.checked_sub(1)?)?;
        Some(ChunkRef {
            index: (position - 1) as u32,
            first_row: entry.first_row,
            rows: entry.rows,
            cell: Arc::clone(&entry.cell),
        })
    }

    /// Load one chunk by index: resident, decoded-cache hit, or decrypted
    /// from spill.
    pub fn load_chunk(&self, index: u32) -> Result<Arc<StoreChunk>, StoreError> {
        let cell = {
            let chunks = self.chunks.read().map_err(|_| StoreError::Internal {
                detail: "chunks lock poisoned".to_owned(),
            })?;
            let entry = chunks
                .get(index as usize)
                .ok_or_else(|| StoreError::Corrupt {
                    detail: format!("no chunk {index}"),
                })?;
            Arc::clone(&entry.cell)
        };
        cell.last_access.store(self.tick(), Ordering::Release);
        {
            let residency = cell.residency.lock().map_err(|_| StoreError::Internal {
                detail: "residency lock poisoned".to_owned(),
            })?;
            if let Residency::Resident(chunk) = &*residency {
                return Ok(Arc::clone(chunk));
            }
        }
        let registry = self.registry()?;
        if let Some(chunk) = registry.decoded_chunk(self.id, index) {
            return Ok(chunk);
        }
        let (offset, len, nonce) = {
            let residency = cell.residency.lock().map_err(|_| StoreError::Internal {
                detail: "residency lock poisoned".to_owned(),
            })?;
            match &*residency {
                Residency::Spilled {
                    offset, len, nonce, ..
                } => (*offset, *len, *nonce),
                Residency::Resident(chunk) => return Ok(Arc::clone(chunk)),
            }
        };
        let mut record = {
            let spill = self.spill.lock().map_err(|_| StoreError::Internal {
                detail: "spill lock poisoned".to_owned(),
            })?;
            let spill = spill.as_ref().ok_or_else(|| StoreError::Corrupt {
                detail: format!("chunk {index} is spilled but there is no spill file"),
            })?;
            spill.read(offset, len)?
        };
        let cipher = registry
            .cipher()
            .ok_or_else(|| StoreError::SpillUnavailable {
                reason: "spill is off".to_owned(),
            })?;
        let plain_len = cipher.open(self.id.0, index, nonce, &mut record)?;
        record.truncate(plain_len);
        let decoded = decode_record(&record)?;
        let bytes = decoded.batch.get_array_memory_size() + decoded.flags.bytes();
        let chunk = Arc::new(StoreChunk {
            batch: decoded.batch,
            encodings: decoded.encodings.into(),
            flags: decoded.flags,
            bytes,
        });
        Ok(registry.cache_decoded(self.id, index, chunk))
    }

    /// Read a run of source rows as `Value`s, for tests and for the SQL path.
    pub fn values(&self, range: std::ops::Range<u32>) -> Result<Vec<Vec<Value>>, StoreError> {
        let total = self.rows();
        let start = range.start.min(total);
        let end = range.end.min(total);
        if end <= start {
            return Ok(Vec::new());
        }
        let width = self.column_count();
        let mut out = Vec::with_capacity((end - start) as usize);
        for row in start..end {
            let reference = self.chunk_for_row(row).ok_or_else(|| StoreError::Corrupt {
                detail: format!("no chunk for row {row}"),
            })?;
            let chunk = self.load_chunk(reference.index)?;
            let in_chunk = (row - reference.first_row) as usize;
            let mut cells = Vec::with_capacity(width);
            for column in 0..width {
                let array =
                    chunk
                        .batch
                        .columns()
                        .get(column)
                        .ok_or_else(|| StoreError::Corrupt {
                            detail: format!("chunk is narrower than the declared {width} columns"),
                        })?;
                let enc = chunk
                    .encodings
                    .get(column)
                    .copied()
                    .unwrap_or(Encoding::Null);
                cells.push(
                    qh_columnar::value_at(array.as_ref(), enc, in_chunk).map_err(|error| {
                        StoreError::Corrupt {
                            detail: error.to_string(),
                        }
                    })?,
                );
            }
            out.push(cells);
        }
        Ok(out)
    }

    // --- views ----------------------------------------------------------

    /// Install a view spec. Returns the new view id; the previous view is
    /// cancelled and answers `Superseded`.
    pub fn set_view(self: &Arc<Self>, spec: ViewSpec) -> Result<u64, StoreError> {
        if self.phase() == Phase::Released {
            return Err(StoreError::Released);
        }
        let id = self.view_seq.fetch_add(1, Ordering::AcqRel) + 1;
        let new_view = build_view(self, spec, id)?;
        let old = {
            let mut guard = self.view.write().map_err(|_| StoreError::Internal {
                detail: "view lock poisoned".to_owned(),
            })?;
            guard.replace(Arc::clone(&new_view))
        };
        if let Some(old) = old {
            old.cancel();
        }
        // Rows may have arrived while the view was building; the writer would
        // have missed them because the new view was not installed yet.
        self.maybe_expand_view();
        Ok(id)
    }

    /// The writer's hook: if a streaming filter view owes rows, schedule at
    /// most one expansion task on the view pool.
    pub fn maybe_expand_view(self: &Arc<Self>) {
        let view = match self.current_view() {
            Some(view) => view,
            None => return,
        };
        if !view.is_expandable() || view.is_cancelled() {
            return;
        }
        if view.scanned_chunks() >= self.chunk_refs().len() as u32 {
            return;
        }
        if view.begin_expanding() {
            let this = Arc::clone(self);
            qh_rt::view_pool().spawn(move || {
                this.expand_view(&view);
                view.end_expanding();
                if view.scanned_chunks() < this.chunk_refs().len() as u32 {
                    this.maybe_expand_view();
                }
            });
        }
    }

    /// A synchronous final expansion, used by `finish`.
    fn expand_view_now(&self) {
        let view = match self.current_view() {
            Some(view) => view,
            None => return,
        };
        if view.is_expandable() && !view.is_cancelled() {
            self.expand_view(&view);
        }
    }

    /// Scan chunks past the view's watermark and append the rows that match.
    ///
    /// The scan is parallel across chunks, but the rows are appended in chunk
    /// order so a streaming filter stays in source order as it grows.
    fn expand_view(&self, view: &View) {
        let chunks = self.chunk_refs();
        let start = view.scanned_chunks() as usize;
        if start >= chunks.len() {
            return;
        }
        let pending = &chunks[start..];
        let loaded: Vec<Arc<StoreChunk>> = pending
            .iter()
            .map(|reference| self.load_chunk(reference.index))
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_default();
        if loaded.len() != pending.len() {
            view.cancel();
            return;
        }
        if view.is_cancelled() {
            return;
        }
        let per_chunk: Vec<Vec<u32>> = qh_rt::view_pool().install(|| {
            pending
                .par_iter()
                .zip(loaded.par_iter())
                .map(|(reference, chunk)| {
                    let mut matched = Vec::new();
                    crate::view::scan_chunk(reference, chunk, view.spec(), &mut matched);
                    matched
                })
                .collect()
        });
        let mut matched: Vec<u32> = Vec::new();
        for mut rows in per_chunk {
            matched.append(&mut rows);
        }
        view.append_rows(&matched);
        view.set_scanned_chunks(chunks.len() as u32);
    }

    // --- release and eviction -------------------------------------------

    /// Free everything: mark released, drop chunks, return the budget, close
    /// the spill fd. Idempotent.
    pub fn release(&self, registry: &StoreRegistry) {
        if self.phase() == Phase::Released {
            return;
        }
        self.phase.store(Phase::Released.as_u8(), Ordering::Release);
        if let Some(view) = self.current_view() {
            view.cancel();
        }
        let entries = {
            let mut chunks = match self.chunks.write() {
                Ok(chunks) => chunks,
                Err(_) => return,
            };
            std::mem::take(&mut *chunks)
        };
        for entry in &entries {
            if let Ok(residency) = entry.cell.residency.lock() {
                if let Residency::Resident(chunk) = &*residency {
                    registry.uncharge(chunk.bytes);
                }
            }
        }
        // Dropping the file closes the fd; the blocks were already freed at
        // unlink.
        if let Ok(mut spill) = self.spill.lock() {
            *spill = None;
        }
    }

    /// Chunks a reader is not holding, oldest access first.
    pub(crate) fn evictable_chunks(&self) -> Vec<(usize, u64)> {
        let chunks = match self.chunks.read() {
            Ok(chunks) => chunks,
            Err(_) => return Vec::new(),
        };
        chunks
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry
                    .cell
                    .residency
                    .lock()
                    .map(|residency| matches!(*residency, Residency::Resident(_)))
                    .unwrap_or(false)
            })
            .map(|(index, entry)| (index, entry.cell.last_access.load(Ordering::Acquire)))
            .collect()
    }

    /// Move one chunk to the spill file. Returns the freed bytes, `None` if
    /// the chunk was already spilled, is borrowed, or spill is off, or the
    /// error when the write failed (a full disk, most often).
    ///
    /// The residency lock is not held across serialize, encrypt, and write:
    /// the chunk `Arc` is cloned under the lock, the lock is released, and it
    /// is taken again only to swap `Resident` for `Spilled`. Without that, a
    /// window touching this chunk would wait on ~2 MiB of I/O.
    pub(crate) fn spill_one(
        &self,
        index: usize,
        registry: &StoreRegistry,
    ) -> Result<Option<usize>, StoreError> {
        let cell = match self.chunks.read() {
            Ok(chunks) => match chunks.get(index) {
                Some(entry) => Arc::clone(&entry.cell),
                None => return Ok(None),
            },
            Err(_) => return Ok(None),
        };
        let chunk = {
            let residency = match cell.residency.lock() {
                Ok(residency) => residency,
                Err(_) => return Ok(None),
            };
            match &*residency {
                Residency::Spilled { .. } => return Ok(None),
                Residency::Resident(chunk) => {
                    // A reader, a sort pin, or a helper hand-off holds a
                    // second reference; leave it alone.
                    if Arc::strong_count(chunk) > 1 {
                        return Ok(None);
                    }
                    Arc::clone(chunk)
                }
            }
        };
        let cipher = match registry.cipher() {
            Some(cipher) => cipher,
            None => return Ok(None),
        };
        let mut plain = match encode_record(&chunk.batch, &chunk.flags) {
            Ok(plain) => plain,
            Err(_) => return Ok(None),
        };
        let (nonce, record) = match cipher.seal(self.id.0, index as u32, &mut plain) {
            Ok(sealed) => sealed,
            Err(_) => return Ok(None),
        };
        let (offset, len) = {
            let mut spill = match self.spill.lock() {
                Ok(spill) => spill,
                Err(_) => return Ok(None),
            };
            if spill.is_none() {
                let dir = match registry.config.spill_dir.as_ref() {
                    Some(dir) => dir,
                    None => return Ok(None),
                };
                *spill = Some(self.new_spill_file(registry, dir)?);
            }
            match spill.as_mut() {
                Some(file) => file.append(&record)?,
                None => return Ok(None),
            }
        };
        let bytes = chunk.bytes;
        {
            let mut residency = match cell.residency.lock() {
                Ok(residency) => residency,
                Err(_) => return Ok(None),
            };
            if let Residency::Resident(current) = &*residency {
                if Arc::ptr_eq(current, &chunk) {
                    *residency = Residency::Spilled {
                        offset,
                        len,
                        nonce,
                        bytes,
                    };
                }
            }
        }
        registry.uncharge(bytes);
        Ok(Some(bytes))
    }

    /// Open a spill file for this store. The test seam swaps in a faulty
    /// medium so the disk-full path can be exercised without a real disk.
    fn new_spill_file(
        &self,
        _registry: &StoreRegistry,
        dir: &std::path::Path,
    ) -> Result<SpillFile, StoreError> {
        if let Some(medium) = self.faulty_medium.lock().ok().and_then(|mut m| m.take()) {
            return Ok(SpillFile::with_medium(medium));
        }
        SpillFile::create(dir, std::process::id(), self.id.0)
    }

    /// The spill file's path, for the diagnostics bundle. `None` until the
    /// first spill, and always `None` once it is unlinked.
    pub fn spill_path(&self) -> Option<std::path::PathBuf> {
        self.spill
            .lock()
            .ok()
            .and_then(|spill| spill.as_ref().map(|_| None))
            .flatten()
    }

    /// Whether anything has been spilled.
    pub fn has_spilled(&self) -> bool {
        self.chunks
            .read()
            .map(|chunks| {
                chunks.iter().any(|entry| {
                    entry
                        .cell
                        .residency
                        .lock()
                        .map(|residency| matches!(*residency, Residency::Spilled { .. }))
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false)
    }

    /// Every byte in the spill file, for the "no plaintext on disk" test.
    #[doc(hidden)]
    pub fn spill_raw_for_test(&self) -> Vec<u8> {
        self.spill
            .lock()
            .ok()
            .and_then(|spill| {
                spill
                    .as_ref()
                    .and_then(|file| file.read_all_raw_for_test().ok())
            })
            .unwrap_or_default()
    }

    /// Force every resident chunk to spill, for tests that need a spilled
    /// store without a tiny budget. Returns the number moved.
    #[doc(hidden)]
    pub fn force_spill_all_for_test(&self, registry: &StoreRegistry) -> usize {
        let count = self.chunks.read().map(|chunks| chunks.len()).unwrap_or(0);
        let mut moved = 0;
        for index in 0..count {
            if matches!(self.spill_one(index, registry), Ok(Some(_))) {
                moved += 1;
            }
        }
        moved
    }
}

/// A handle to one chunk, cloned out of the index.
pub struct ChunkRef {
    pub index: u32,
    pub first_row: u32,
    pub rows: u32,
    pub cell: Arc<ChunkCell>,
}

fn builder_error(error: qh_columnar::ColumnarError) -> StoreError {
    StoreError::Internal {
        detail: error.to_string(),
    }
}

/// Fold per-chunk statistics into the running totals. A column that was Null
/// so far takes the new variants; otherwise the variants union and the
/// uniformity flags degrade.
fn merge_stats(accumulated: &mut Vec<ColumnStats>, deltas: &[ColumnStats]) {
    if accumulated.len() < deltas.len() {
        accumulated.resize(deltas.len(), ColumnStats::default());
    }
    for (index, delta) in deltas.iter().enumerate() {
        let into = &mut accumulated[index];
        if into.rows == 0 {
            *into = delta.clone();
            continue;
        }
        into.variants |= delta.variants;
        if delta.max_scale != into.max_scale {
            into.scale_uniform = false;
        }
        into.max_scale = into.max_scale.max(delta.max_scale);
        into.overflow |= delta.overflow;
        into.has_naive_ts |= delta.has_naive_ts;
        into.offset = match (into.offset, delta.offset) {
            (None, other) => other,
            (Some(first), Some(next)) if first != next => Some(i32::MIN),
            (Some(first), _) => Some(first),
        };
        into.rows += delta.rows;
    }
}
