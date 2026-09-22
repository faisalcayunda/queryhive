//! The throttled progress reporter.
//!
//! From `queryhive_engine.py`'s `make_progress`. Two rules, and both matter:
//!
//! - **At most one event per `PROGRESS_MS`.** A query that returns rows faster than
//!   the floor would otherwise put one JSON line on stdout per row, and the app
//!   would decode thousands of events to draw the same number.
//! - **Never fewer than one per whole 1000 rows.** A long query that each event is
//!   waited out on would show nothing at all until it finished, which reads as a
//!   hang.
//!
//! The caller emits the total once more after the export returns, so `done` is
//! always preceded by a progress event carrying the final count.

use std::io;
use std::time::{Duration, Instant};

use qh_export::plan::PROGRESS_EVERY;

use crate::events::{event, Emitter};

/// How long a progress event may be held back when the caller says nothing.
pub const PROGRESS_MS_DEFAULT: i64 = 250;

/// The reporter and the state it tracks.
///
/// `last` is `None` until the first event goes out — the Python engine's `-1`
/// sentinel, which exists so "nothing reported yet" is distinguishable from "zero
/// rows" when the true total is compared at the end.
#[derive(Debug)]
pub struct Progress {
    floor: Duration,
    last: Option<u64>,
    at: Instant,
}

impl Progress {
    pub fn new(floor_ms: i64) -> Self {
        Self {
            // A negative floor is a caller's mistake, not a reason to panic: it means
            // "no throttle at all", which is what clamping to zero gives.
            floor: Duration::from_millis(u64::try_from(floor_ms).unwrap_or(0)),
            last: None,
            at: Instant::now(),
        }
    }

    /// The last count reported, or `None` if nothing has been.
    pub fn last(&self) -> Option<u64> {
        self.last
    }

    /// Emit a `progress` event, if the floor or the 1000-row rule says it is due.
    pub fn emit(&mut self, rows: u64, out: &mut dyn Emitter) -> io::Result<()> {
        let due = self.last.is_none() || self.at.elapsed() >= self.floor;
        if due
            || self
                .last
                .is_some_and(|last| rows.saturating_sub(last) >= PROGRESS_EVERY)
        {
            self.last = Some(rows);
            self.at = Instant::now();
            out.emit(event("progress").field("rows", rows).build())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Capture;

    #[test]
    fn the_first_count_always_goes_out() {
        let mut capture = Capture::new();
        // A floor no clock can cross: without the "nothing reported yet" rule this
        // event would be held back, and the first thing the user sees of a slow
        // export would be nothing.
        let mut progress = Progress::new(60_000);
        progress.emit(1, &mut capture).unwrap();
        assert_eq!(capture.lines(), vec!["{\"event\":\"progress\",\"rows\":1}"]);
    }

    #[test]
    fn a_whole_thousand_rows_gets_through_a_floor_that_never_expires() {
        let mut capture = Capture::new();
        let mut progress = Progress::new(60_000);
        progress.emit(1, &mut capture).unwrap();
        progress.emit(999, &mut capture).unwrap();
        progress.emit(1_001, &mut capture).unwrap();
        assert_eq!(capture.lines().len(), 2, "{:?}", capture.lines());
        assert_eq!(progress.last(), Some(1_001));
    }
}
