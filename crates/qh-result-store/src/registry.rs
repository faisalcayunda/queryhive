//! `StoreRegistry`: store identity, the global budget, and eviction order
//! (blueprint §9).
//!
//! One registry per process, created once by the host. It owns the spill
//! cipher, hands out `StoreId`s that are never reused, keeps the accounted
//! byte total across every live store, and evicts when the budget is
//! exceeded. A store that spilled nothing still costs nothing here: the
//! accounting is charged at seal and returned at release.
//!
//! `StoreHandle` is the owner token: dropping it releases the store. A
//! `StoreWriter` is a clone handed to the fetch loop; it can outlive the
//! handle and then answers `Released` on the next push.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use qh_core::{ColumnBatch, ColumnMeta, Value};

use crate::spill::{SpillCipher, SweepReport};
use crate::store::{Outcome, StoreShared};
use crate::StoreError;

/// The budget and the shape every store inherits (blueprint §9.1).
#[derive(Debug, Clone)]
pub struct StoreConfig {
    /// Total accounted bytes across every live store, resident chunks,
    /// decoded cache, view reservations, and helper leases.
    pub budget_bytes: usize,
    /// The level eviction runs down to. The gap from `budget_bytes` is the
    /// hysteresis that keeps a full scan from happening every push.
    pub low_water_bytes: usize,
    /// Where spill files go. `None` disables spill (tests, headless use).
    pub spill_dir: Option<PathBuf>,
    /// Rows per chunk before a seal.
    pub chunk_max_rows: u32,
    /// Estimated bytes per chunk before a seal.
    pub chunk_target_bytes: usize,
    /// How many decrypted chunks stay in memory after a spill read.
    pub decoded_cache_chunks: usize,
    /// The ceiling on one helper lease.
    pub query_share_bytes: usize,
}

impl Default for StoreConfig {
    fn default() -> Self {
        // 256 MiB is the figure the blueprint names for the store's share of
        // the process budget (O-12): large enough that an ordinary query never
        // reaches the disk, small enough to leave room for the UI and the grid.
        const BUDGET: usize = 256 * 1024 * 1024;
        Self {
            budget_bytes: BUDGET,
            low_water_bytes: BUDGET - 32 * 1024 * 1024,
            spill_dir: None,
            chunk_max_rows: 65_536,
            chunk_target_bytes: 2 * 1024 * 1024,
            decoded_cache_chunks: 8,
            query_share_bytes: BUDGET / 2,
        }
    }
}

/// A store's identity, unique for the life of the process and never reused.
///
/// It rides in the spill AAD (§10.3), so one store's record can never be
/// authenticated for another, and it plays the "generation" role the
/// performance plan asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StoreId(pub u64);

/// A snapshot of the registry for the host's diagnostics.
#[derive(Debug, Clone, Copy)]
pub struct RegistryStats {
    /// Live stores, excluding released ones.
    pub stores: usize,
    /// Accounted bytes right now.
    pub resident_bytes: usize,
    /// The budget those bytes are charged against.
    pub budget_bytes: usize,
    /// Chunks currently held decrypted after a spill read.
    pub decoded_chunks: usize,
    /// Bytes written to the spill files of the live stores.
    pub spilled_bytes: u64,
}

/// The registry: the cipher, the live-store table, the budget, the clock.
pub struct StoreRegistry {
    pub config: StoreConfig,
    cipher: Option<SpillCipher>,
    live: Mutex<HashMap<StoreId, Weak<StoreShared>>>,
    next_id: AtomicU64,
    resident: AtomicUsize,
    query_reserved: AtomicUsize,
    /// Logical clock for LRU. Not wall time: the order only has to be
    /// consistent, and a monotonic counter cannot go backwards.
    clock: AtomicU64,
    /// Decrypted chunks kept after a spill read, keyed by (store, index).
    decoded: Mutex<Vec<DecodedEntry>>,
}

struct DecodedEntry {
    id: StoreId,
    index: u32,
    chunk: Arc<crate::chunk::StoreChunk>,
    last_access: u64,
}

impl std::fmt::Debug for StoreRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreRegistry")
            .field("config", &self.config)
            .field("cipher", &self.cipher)
            .field(
                "stores",
                &self.live.lock().map(|live| live.len()).unwrap_or(0),
            )
            .finish()
    }
}

impl StoreRegistry {
    /// Build the registry and sweep the spill directory. The sweep report is
    /// the host's to log; spill is off when the sweep turned it off.
    pub fn new(config: StoreConfig) -> (Arc<Self>, SweepReport) {
        let report = match &config.spill_dir {
            Some(dir) => crate::spill::sweep_spill_dir(dir),
            // No directory means no spill: the tests and the headless paths
            // run with it off rather than writing into a temp folder nobody
            // sweeps.
            None => SweepReport {
                removed: 0,
                spill_enabled: false,
                reason: Some("no spill directory configured".to_owned()),
            },
        };
        let cipher = if report.spill_enabled {
            match SpillCipher::new() {
                Ok(cipher) => Some(cipher),
                Err(error) => {
                    // A key that cannot be made means spill off, not a weaker
                    // key. There is no fixed-key fallback (§10.1).
                    return (
                        Arc::new(Self::bare(config)),
                        SweepReport {
                            removed: report.removed,
                            spill_enabled: false,
                            reason: Some(error.to_string()),
                        },
                    );
                }
            }
        } else {
            None
        };
        let registry = Self {
            config,
            cipher,
            live: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            resident: AtomicUsize::new(0),
            query_reserved: AtomicUsize::new(0),
            clock: AtomicU64::new(1),
            decoded: Mutex::new(Vec::new()),
        };
        (Arc::new(registry), report)
    }

    /// A registry with no cipher, for the failure path above.
    fn bare(config: StoreConfig) -> Self {
        Self {
            config,
            cipher: None,
            live: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            resident: AtomicUsize::new(0),
            query_reserved: AtomicUsize::new(0),
            clock: AtomicU64::new(1),
            decoded: Mutex::new(Vec::new()),
        }
    }

    /// The spill cipher, or `None` when spill is off.
    pub fn cipher(&self) -> Option<&SpillCipher> {
        self.cipher.as_ref()
    }

    /// A fresh, never-reused id.
    fn next_store_id(&self) -> StoreId {
        StoreId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// The next value of the logical clock.
    pub fn tick(&self) -> u64 {
        self.clock.fetch_add(1, Ordering::Relaxed)
    }

    /// Open a store with no columns yet. The column list arrives through
    /// `StoreWriter::begin`, because it comes from the server and can land
    /// after the first rows do.
    pub fn create(self: &Arc<Self>) -> StoreHandle {
        let id = self.next_store_id();
        let shared = Arc::new(StoreShared::new(id, Arc::downgrade(self)));
        self.live
            .lock()
            .expect("the live table should not be poisoned")
            .insert(id, Arc::downgrade(&shared));
        StoreHandle {
            id,
            shared,
            registry: Arc::clone(self),
        }
    }

    /// Build a store from plain text rows. Used by fixtures, by the sample
    /// data path, and by anything that already has strings rather than
    /// `Value`s.
    pub fn from_text_rows(
        self: &Arc<Self>,
        columns: Vec<ColumnMeta>,
        rows: Vec<Vec<Option<String>>>,
    ) -> Result<StoreHandle, StoreError> {
        let handle = self.create();
        let writer = handle.writer();
        writer.begin(columns)?;
        let mut columns_of_values: Vec<Vec<Value>> = Vec::new();
        for row in &rows {
            for (index, cell) in row.iter().enumerate() {
                if columns_of_values.len() <= index {
                    columns_of_values.push(Vec::with_capacity(rows.len()));
                }
                columns_of_values[index].push(match cell {
                    Some(text) => Value::Text(text.clone().into_boxed_str()),
                    None => Value::Null,
                });
            }
        }
        if !columns_of_values.is_empty() {
            let batch = ColumnBatch::new(columns_of_values)?;
            writer.push(&batch)?;
        }
        writer.finish(Outcome::Complete { truncated: false })?;
        Ok(handle)
    }

    /// A store of synthetic rows, for the benchmarks and the Swift UI tests.
    pub fn synthetic(
        self: &Arc<Self>,
        rows: u32,
        columns: u32,
        seed: u64,
    ) -> Result<StoreHandle, StoreError> {
        let handle = self.create();
        let writer = handle.writer();
        let metas: Vec<ColumnMeta> = (0..columns)
            .map(|index| ColumnMeta::new(format!("c{index}"), "text"))
            .collect();
        writer.begin(metas)?;
        let mut state = seed;
        for row in 0..rows {
            let mut cells = Vec::with_capacity(columns as usize);
            for _ in 0..columns {
                // xorshift64: deterministic from the seed, so a bench run is
                // reproducible and a Swift test can assert on the content.
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                cells.push(Value::Text(
                    format!("row-{row}-{state:016x}").into_boxed_str(),
                ));
            }
            writer.push(&ColumnBatch::new(vec![cells])?)?;
        }
        writer.finish(Outcome::Complete { truncated: false })?;
        Ok(handle)
    }

    /// Charge the budget, evicting if the charge pushes past it.
    ///
    /// `origin` is who asked: a writer or a view reserves synchronously and
    /// takes the eviction cost itself, while a reader of the decoded cache
    /// only drops its own entry (§9.4).
    pub fn charge(&self, bytes: usize, origin: Origin) -> Result<(), StoreError> {
        let before = self.resident.fetch_add(bytes, Ordering::AcqRel) + bytes;
        if before <= self.config.budget_bytes {
            return Ok(());
        }
        match origin {
            Origin::Writer | Origin::View => {
                if let Err(error) = self.evict_until(self.config.low_water_bytes) {
                    // A failed eviction (a full disk, say) must not leave the
                    // failed charge counted: the caller's chunk is not
                    // published and the budget would leak otherwise.
                    self.uncharge(bytes);
                    return Err(error);
                }
                // Eviction is best-effort across stores; if the store that
                // charged cannot free enough, the budget is genuinely full.
                if self.resident.load(Ordering::Acquire) > self.config.budget_bytes {
                    self.uncharge(bytes);
                    return Err(StoreError::BudgetExceeded {
                        needed: bytes,
                        budget: self.config.budget_bytes,
                    });
                }
                Ok(())
            }
            // The decoded cache normally drops only its own entries (no I/O).
            // If that is not enough — a budget smaller than the cache's own
            // allotment, say — fall back to the same eviction a writer does,
            // so the accounted total never sits above the budget (§9.4).
            Origin::DecodedCache => {
                self.trim_decoded_cache(0);
                if self.resident.load(Ordering::Acquire) <= self.config.budget_bytes {
                    return Ok(());
                }
                if let Err(error) = self.evict_until(self.config.low_water_bytes) {
                    self.uncharge(bytes);
                    return Err(error);
                }
                if self.resident.load(Ordering::Acquire) > self.config.budget_bytes {
                    self.uncharge(bytes);
                    return Err(StoreError::BudgetExceeded {
                        needed: bytes,
                        budget: self.config.budget_bytes,
                    });
                }
                Ok(())
            }
        }
    }

    /// Give back budget. Saturating, because a double release or a release
    /// after a failed eviction must never wrap the counter negative.
    pub fn uncharge(&self, bytes: usize) {
        let _ = self
            .resident
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                Some(current.saturating_sub(bytes))
            });
    }

    /// Reserve scratch for a view. Evicts to make room, or fails with
    /// `BudgetExceeded`; the `Drop` on the reservation gives it back.
    pub fn reserve(&self, bytes: usize) -> Result<Reservation<'_>, StoreError> {
        if self.resident.load(Ordering::Acquire) + bytes > self.config.budget_bytes {
            self.evict_until(self.config.low_water_bytes)?;
        }
        if self.resident.load(Ordering::Acquire) + bytes > self.config.budget_bytes {
            return Err(StoreError::BudgetExceeded {
                needed: bytes,
                budget: self.config.budget_bytes,
            });
        }
        self.resident.fetch_add(bytes, Ordering::AcqRel);
        Ok(Reservation {
            registry: self,
            bytes,
        })
    }

    /// Reserve a helper lease, at most `query_share_bytes` (§9.4). Evicts
    /// idle stores first, and refuses under 32 MiB: a helper that gets less
    /// than that cannot do useful work.
    pub fn reserve_query(&self, max: usize) -> Result<QueryLease<'_>, StoreError> {
        const MIN_QUERY_BYTES: usize = 32 * 1024 * 1024;
        let want = max.min(self.config.query_share_bytes);
        if self.resident.load(Ordering::Acquire) + want > self.config.budget_bytes {
            self.evict_until(self.config.low_water_bytes)?;
        }
        let available = self
            .config
            .budget_bytes
            .saturating_sub(self.resident.load(Ordering::Acquire));
        if available < MIN_QUERY_BYTES {
            return Err(StoreError::BudgetExceeded {
                needed: MIN_QUERY_BYTES,
                budget: available,
            });
        }
        let grant = want.min(available);
        self.resident.fetch_add(grant, Ordering::AcqRel);
        self.query_reserved.fetch_add(grant, Ordering::AcqRel);
        Ok(QueryLease {
            registry: self,
            bytes: grant,
        })
    }

    /// Bytes held by helper leases.
    pub fn query_reserved_bytes(&self) -> usize {
        self.query_reserved.load(Ordering::Acquire)
    }

    /// Evict until the accounted total is at or below `target`.
    ///
    /// Decoded-cache entries go first (no I/O), then resident chunks from
    /// every live store, oldest access first, skipping any chunk a reader
    /// currently holds. This runs on the calling thread, which is the writer's
    /// FFI thread or a helper client, never main.
    fn evict_until(&self, target: usize) -> Result<(), StoreError> {
        self.trim_decoded_cache(self.config.decoded_cache_chunks.min(4));
        let live: Vec<Arc<StoreShared>> = {
            let table = self
                .live
                .lock()
                .expect("the live table should not be poisoned");
            table.values().filter_map(Weak::upgrade).collect()
        };
        // (store, chunk index, last_access), oldest first.
        let mut candidates: Vec<(Arc<StoreShared>, usize, u64)> = Vec::new();
        for store in &live {
            candidates.extend(
                store
                    .evictable_chunks()
                    .into_iter()
                    .map(|(index, access)| (Arc::clone(store), index, access)),
            );
        }
        candidates.sort_by_key(|(_, _, access)| *access);
        // A spill failure (a full disk, most often) must not stop eviction
        // from trying the other stores: the chunk stays resident and the
        // error is held to the end, so the triggering run gets one event.
        let mut first_error: Option<StoreError> = None;
        for (store, index, _) in candidates {
            if self.resident.load(Ordering::Acquire) <= target {
                return Ok(());
            }
            // `spill_one` re-checks the access count under the residency lock,
            // so a chunk a reader picked up between the scan and here is left
            // alone rather than pulled out from under it.
            match store.spill_one(index, self) {
                Ok(Some(_)) | Ok(None) => {}
                Err(error) => {
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Remember a decrypted chunk, evicting the oldest entries past the cap.
    ///
    /// The cache is charged inside the budget (§9.4): the chunk's bytes are
    /// added here and given back when the entry is evicted, so a spilled read
    /// does not let the cache grow the accounted total without bound. When the
    /// budget is too small to hold even one entry, the chunk is returned
    /// uncached rather than leaving the total above the budget.
    pub fn cache_decoded(
        &self,
        id: StoreId,
        index: u32,
        chunk: Arc<crate::chunk::StoreChunk>,
    ) -> Arc<crate::chunk::StoreChunk> {
        let now = self.tick();
        let bytes = chunk.bytes;
        // Charge first, then evict to make room. The decoded lock is not held
        // across this, and `Origin::DecodedCache` only drops its own entries
        // unless the cache alone cannot fit.
        if self.charge(bytes, Origin::DecodedCache).is_err() {
            return chunk;
        }
        if self.resident.load(Ordering::Acquire) > self.config.budget_bytes
            && self.evict_until(self.config.low_water_bytes).is_err()
        {
            self.uncharge(bytes);
            return chunk;
        }
        let mut decoded = self
            .decoded
            .lock()
            .expect("the decoded cache should not be poisoned");
        // A second entry for the same chunk would be charged twice.
        if let Some(position) = decoded
            .iter()
            .position(|entry| entry.id == id && entry.index == index)
        {
            let old = decoded.remove(position);
            self.uncharge(old.chunk.bytes);
        }
        decoded.push(DecodedEntry {
            id,
            index,
            chunk: Arc::clone(&chunk),
            last_access: now,
        });
        decoded.sort_by_key(|entry| entry.last_access);
        // Bound by both the count cap and the budget (§9.4). The loop keeps at
        // least the entry just added, unless the budget cannot hold it.
        while decoded.len() > self.config.decoded_cache_chunks
            || (self.resident.load(Ordering::Acquire) > self.config.budget_bytes
                && !decoded.is_empty())
        {
            let entry = decoded.remove(0);
            self.uncharge(entry.chunk.bytes);
            if decoded.is_empty() {
                break;
            }
        }
        chunk
    }

    /// A decrypted chunk if it is still cached.
    pub fn decoded_chunk(&self, id: StoreId, index: u32) -> Option<Arc<crate::chunk::StoreChunk>> {
        let mut decoded = self
            .decoded
            .lock()
            .expect("the decoded cache should not be poisoned");
        let entry = decoded
            .iter_mut()
            .find(|entry| entry.id == id && entry.index == index)?;
        entry.last_access = self.tick();
        Some(Arc::clone(&entry.chunk))
    }

    /// Drop decoded-cache entries, oldest first, keeping `keep` of them.
    fn trim_decoded_cache(&self, keep: usize) {
        let mut decoded = self
            .decoded
            .lock()
            .expect("the decoded cache should not be poisoned");
        decoded.sort_by_key(|entry| entry.last_access);
        while decoded.len() > keep {
            let entry = decoded.remove(0);
            self.uncharge(entry.chunk.bytes);
        }
    }

    /// Drop every decoded-cache entry for a released store, so its memory
    /// does not outlive the tab.
    fn purge_decoded(&self, id: StoreId) {
        let mut decoded = self
            .decoded
            .lock()
            .expect("the decoded cache should not be poisoned");
        let mut index = 0;
        while index < decoded.len() {
            if decoded[index].id == id {
                let entry = decoded.remove(index);
                self.uncharge(entry.chunk.bytes);
            } else {
                index += 1;
            }
        }
    }

    /// Diagnostics.
    pub fn stats(&self) -> RegistryStats {
        RegistryStats {
            stores: self
                .live
                .lock()
                .map(|live| live.values().filter(|weak| weak.strong_count() > 0).count())
                .unwrap_or(0),
            resident_bytes: self.resident.load(Ordering::Acquire),
            spilled_bytes: self
                .live
                .lock()
                .map(|live| {
                    live.values()
                        .filter_map(Weak::upgrade)
                        .map(|s| s.spilled_bytes())
                        .sum()
                })
                .unwrap_or(0),
            budget_bytes: self.config.budget_bytes,
            decoded_chunks: self
                .decoded
                .lock()
                .map(|decoded| decoded.len())
                .unwrap_or(0),
        }
    }

    /// Release a store: mark it released, free its budget, close its spill
    /// fd, and drop it from the live table. Idempotent.
    pub fn release(&self, id: StoreId) {
        let shared = {
            // This runs from `Drop` and from the UniFFI free path, so a poisoned table is
            // recovered, never a panic.
            let mut table = self
                .live
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            table.remove(&id).and_then(|weak| weak.upgrade())
        };
        if let Some(shared) = shared {
            shared.release(self);
        }
        self.purge_decoded(id);
    }

    /// Panics while holding the live-table lock, so it is poisoned. For the poison test only.
    #[doc(hidden)]
    pub fn poison_live_for_test(&self) {
        let _guard = self.live.lock();
        panic!("poisoning the live table on purpose");
    }

    /// Test and bench seam: a registry with spill off and a known budget.
    #[doc(hidden)]
    pub fn for_test(config: StoreConfig) -> Arc<Self> {
        let (registry, _) = Self::new(config);
        registry
    }
}

/// Who is charging the budget. Decides whether a full budget evicts or
/// just gives up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Writer,
    View,
    DecodedCache,
}

/// Scratch held for a view. The `Drop` returns it, so a view that fails
/// halfway cannot leak budget.
pub struct Reservation<'a> {
    registry: &'a StoreRegistry,
    bytes: usize,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        self.registry.uncharge(self.bytes);
    }
}

/// One helper's share of the budget. `Drop` gives it back.
pub struct QueryLease<'a> {
    registry: &'a StoreRegistry,
    bytes: usize,
}

impl QueryLease<'_> {
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for QueryLease<'_> {
    fn drop(&mut self) {
        self.registry.uncharge(self.bytes);
        self.registry
            .query_reserved
            .fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// The owner token for a store. Dropping it releases the store, which is how
/// a closed tab frees its rows without the caller remembering to.
pub struct StoreHandle {
    id: StoreId,
    shared: Arc<StoreShared>,
    registry: Arc<StoreRegistry>,
}

impl StoreHandle {
    pub fn id(&self) -> StoreId {
        self.id
    }

    /// The shared state, for the reader side (`window`, `distinct_values`,
    /// the logical schema).
    pub fn shared(&self) -> &Arc<StoreShared> {
        &self.shared
    }

    /// A writer handle. It can outlive this one; the next push then answers
    /// `Released`, which the fetch loop treats as a cancel.
    pub fn writer(&self) -> StoreWriter {
        StoreWriter {
            shared: Arc::clone(&self.shared),
            registry: Arc::clone(&self.registry),
        }
    }
}

impl Drop for StoreHandle {
    fn drop(&mut self) {
        self.registry.release(self.id);
    }
}

impl std::fmt::Debug for StoreHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreHandle").field("id", &self.id).finish()
    }
}

/// The write side, cloneable and independent of the owner token.
#[derive(Clone)]
pub struct StoreWriter {
    shared: Arc<StoreShared>,
    registry: Arc<StoreRegistry>,
}

impl StoreWriter {
    pub fn shared(&self) -> &Arc<StoreShared> {
        &self.shared
    }

    /// Declare the column list. Once per store; the server can deliver it
    /// after the query starts, and the grid draws headers before rows.
    pub fn begin(&self, columns: Vec<ColumnMeta>) -> Result<(), StoreError> {
        self.shared.begin(self.registry(), columns)
    }

    /// Append a batch. Seals one or more chunks, publishes them, and may
    /// evict to stay inside the budget.
    pub fn push(&self, batch: &ColumnBatch) -> Result<(), StoreError> {
        self.shared.push(self.registry(), batch)
    }

    /// Append an already-sealed chunk: DataFusion output, or a batch a driver
    /// built itself.
    pub fn push_chunk(&self, chunk: qh_columnar::SealedChunk) -> Result<(), StoreError> {
        self.shared.push_chunk(self.registry(), chunk)
    }

    /// Finish the result. `rows` and `phase` reflect it afterwards.
    pub fn finish(&self, outcome: Outcome) -> Result<(), StoreError> {
        self.shared.finish(outcome)
    }

    pub fn rows(&self) -> u32 {
        self.shared.rows()
    }

    fn registry(&self) -> &Arc<StoreRegistry> {
        &self.registry
    }
}
