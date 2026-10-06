//! `LeasePool`: DataFusion's memory pool, bounded by the leases the app has granted.
//!
//! The memory budget is one number and the app owns it (O-12). The helper never asks the
//! registry for anything: for each query the app says "you may use `lease_bytes`", and
//! this pool allows DataFusion to reserve at most the sum of the leases of the queries
//! that are running. A reservation past that fails with `ResourcesExhausted`; operators
//! that can spill (sort, aggregation, sort-merge join) answer by writing to the helper's
//! encrypted spill, and the ones that cannot fail the query with
//! [`OUT_OF_BUDGET`].
//!
//! The sharing rule is the one of DataFusion's `FairSpillPool`, with a limit that moves, a
//! hard cap on the total, and a reserve: spillable consumers split what the unspillable
//! ones leave (less the last quarter of the lease) in equal shares, and unspillable
//! consumers are served first come, first served. Fairness *between* queries is not
//! promised, and that is accepted: parallel SQL across tabs is rare.

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use datafusion::common::{resources_datafusion_err, Result};
use datafusion::execution::memory_pool::{
    human_readable_size, MemoryConsumer, MemoryLimit, MemoryPool, MemoryReservation,
};

/// Spillers never take more than `1 - 1/N` of the lease; the rest is for unspillable
/// consumers. See `try_grow`. A quarter: with an eighth, a 2 million row sort on a 32 MiB
/// lease still ran the merge out of memory.
const SPILLER_HEADROOM_DIVISOR: usize = 4;

/// What the user is told when a query does not fit its lease.
pub const OUT_OF_BUDGET: &str = "This query needs more memory than the result budget allows.";

#[derive(Debug, Default)]
struct State {
    /// Consumers that can spill.
    spillers: usize,
    /// Bytes reserved by consumers that can spill.
    spillable: usize,
    /// Bytes reserved by consumers that cannot.
    unspillable: usize,
}

/// The pool. One per helper; leases come and go as queries do.
#[derive(Debug)]
pub struct LeasePool {
    /// The sum of the leases of the queries running now.
    limit: AtomicUsize,
    /// Spillers keep out of the last `1/divisor` of the limit.
    reserve_divisor: usize,
    state: Mutex<State>,
}

/// A lease: `bytes` added to the pool's limit until it is dropped.
#[derive(Debug)]
pub struct Lease {
    pool: Arc<LeasePool>,
    bytes: usize,
}

impl LeasePool {
    pub fn new() -> Arc<Self> {
        Self::with_reserve_divisor(SPILLER_HEADROOM_DIVISOR)
    }

    /// A pool whose spillers keep out of the last `1/divisor` of the lease.
    pub fn with_reserve_divisor(divisor: usize) -> Arc<Self> {
        Arc::new(Self {
            limit: AtomicUsize::new(0),
            reserve_divisor: divisor.max(1),
            state: Mutex::new(State::default()),
        })
    }

    /// Raise the limit by `bytes` until the returned [`Lease`] is dropped.
    pub fn grant(self: &Arc<Self>, bytes: usize) -> Lease {
        self.limit.fetch_add(bytes, Ordering::SeqCst);
        Lease {
            pool: Arc::clone(self),
            bytes,
        }
    }

    /// The total of the running queries' leases.
    pub fn limit(&self) -> usize {
        self.limit.load(Ordering::SeqCst)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // A panic while the lock was held leaves plain counters, which are still
        // meaningful; refusing every later query would be worse.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn refuse(
        &self,
        reservation: &MemoryReservation,
        additional: usize,
        available: usize,
    ) -> datafusion::error::DataFusionError {
        resources_datafusion_err!(
            "{OUT_OF_BUDGET} ({} wanted {} more with {} held; {} left of {})",
            reservation.consumer().name(),
            human_readable_size(additional),
            human_readable_size(reservation.size()),
            human_readable_size(available),
            human_readable_size(self.limit())
        )
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.pool.limit.fetch_sub(self.bytes, Ordering::SeqCst);
    }
}

impl MemoryPool for LeasePool {
    fn name(&self) -> &str {
        "lease"
    }

    fn register(&self, consumer: &MemoryConsumer) {
        if consumer.can_spill() {
            self.state().spillers += 1;
        }
    }

    fn unregister(&self, consumer: &MemoryConsumer) {
        if consumer.can_spill() {
            let mut state = self.state();
            state.spillers = state.spillers.saturating_sub(1);
        }
    }

    fn grow(&self, reservation: &MemoryReservation, additional: usize) {
        let mut state = self.state();
        if reservation.consumer().can_spill() {
            state.spillable += additional;
        } else {
            state.unspillable += additional;
        }
    }

    fn shrink(&self, reservation: &MemoryReservation, shrink: usize) {
        let mut state = self.state();
        if reservation.consumer().can_spill() {
            state.spillable = state.spillable.saturating_sub(shrink);
        } else {
            state.unspillable = state.unspillable.saturating_sub(shrink);
        }
    }

    fn try_grow(&self, reservation: &MemoryReservation, additional: usize) -> Result<()> {
        let limit = self.limit();
        let mut state = self.state();
        // The lease is a hard cap on everything held, whoever holds it. `FairSpillPool`
        // lets a spiller that arrived early keep what it took while it was alone, so a
        // second spiller could push the total past the limit; here that second spiller is
        // refused instead, and spills its own data or fails. The lease is the app's memory.
        let held = state.spillable + state.unspillable;
        if held + additional > limit {
            return Err(self.refuse(reservation, additional, limit.saturating_sub(held)));
        }
        if reservation.consumer().can_spill() {
            // And no spiller takes more than its share of what the unspillable ones leave,
            // so one sort cannot starve a second. The last quarter of the lease is not for
            // spillers at all: a sort that fills its share still has to merge, and the
            // merge buffers are unspillable consumers that arrive after the spillers have
            // taken everything they were allowed. `FairSpillPool` has no such reserve, and
            // with it a lease of 32 MiB failed a plain four-way sort with 95 KB left.
            let spill_cap = limit - limit / self.reserve_divisor;
            let spill_available = spill_cap.saturating_sub(state.unspillable);
            let available = spill_available
                .checked_div(state.spillers)
                .unwrap_or(spill_available);
            if reservation.size() + additional > available {
                return Err(self.refuse(reservation, additional, available));
            }
            state.spillable += additional;
        } else {
            state.unspillable += additional;
        }
        Ok(())
    }

    fn reserved(&self) -> usize {
        let state = self.state();
        state.spillable + state.unspillable
    }

    fn memory_limit(&self) -> MemoryLimit {
        MemoryLimit::Finite(self.limit())
    }
}

impl fmt::Display for LeasePool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "lease(limit: {}, reserved: {})",
            human_readable_size(self.limit()),
            human_readable_size(self.reserved())
        )
    }
}
