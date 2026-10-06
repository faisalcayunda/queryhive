//! A query's output: DataFusion batches, normalised to store encodings, cut into chunks
//! and handed to a [`Sink`] (blueprint section 14.12).
//!
//! * Each batch goes through `qh_columnar::from_arrow` (section 5.6), so what leaves the
//!   helper is always in the encodings of section 5.1 and the app only has to validate it.
//! * Batches are cut at 65,536 rows or about 2 MiB, the same limits ingest seals at, so a
//!   chunk of SQL output is the size of a chunk of a fetched result.
//! * Progress goes out at most once per 16 ms.
//! * The row cap is the app's limit and the app still enforces it. The helper stops at
//!   `row_cap`, and `truncated` is set only if a row beyond it actually exists, so a result
//!   of exactly `row_cap` rows is not reported as cut.
//! * Cancelling stops at the next batch. DataFusion is cooperative: a single long poll
//!   (one big sort step) finishes first, which is why the app, not this loop, owns the hard
//!   deadline (it kills the helper after two seconds).

use std::time::{Duration, Instant};

use async_trait::async_trait;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::execution::SendableRecordBatchStream;
use futures::StreamExt;
use qh_analytics_proto::{encode_batch, ErrorKind, WireColumn, WireError, MAX_FRAME_LEN};
use tokio::sync::watch;

use crate::session::classify;

pub const OUTPUT_CHUNK_ROWS: usize = 65_536;
pub const OUTPUT_CHUNK_BYTES: usize = 2 * 1024 * 1024;
pub const PROGRESS_EVERY: Duration = Duration::from_millis(16);

/// The other end is gone; stop quietly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Closed;

/// Where a query's output goes. A payload is one complete IPC stream (a `Batch` frame body).
#[async_trait]
pub trait Sink: Send {
    async fn columns(&mut self, columns: Vec<WireColumn>) -> Result<(), Closed>;
    async fn chunk(&mut self, payload: Vec<u8>) -> Result<(), Closed>;
    async fn progress(&mut self, rows: u64) -> Result<(), Closed>;

    /// The largest payload the other end accepts. A longer one cannot be sent as a frame.
    fn max_payload(&self) -> usize {
        MAX_FRAME_LEN as usize
    }
}

/// How a query ended: the content of the `Done` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub rows: u64,
    pub truncated: bool,
    pub cancelled: bool,
    pub error: Option<WireError>,
}

impl Outcome {
    fn failed(rows: u64, error: WireError) -> Self {
        Self {
            rows,
            truncated: false,
            cancelled: false,
            error: Some(error),
        }
    }
}

/// Cut a batch into pieces of at most [`OUTPUT_CHUNK_ROWS`] rows and about
/// [`OUTPUT_CHUNK_BYTES`] bytes.
pub fn split(batch: &RecordBatch) -> Vec<RecordBatch> {
    let rows = batch.num_rows();
    if rows == 0 {
        return Vec::new();
    }
    let per_row = (batch.get_array_memory_size() / rows).max(1);
    let step = (OUTPUT_CHUNK_BYTES / per_row).clamp(1, OUTPUT_CHUNK_ROWS);
    (0..rows)
        .step_by(step)
        .map(|offset| batch.slice(offset, step.min(rows - offset)))
        .collect()
}

fn encode(piece: RecordBatch) -> Result<Vec<u8>, WireError> {
    let sealed = qh_columnar::from_arrow(&piece).map_err(|error| WireError {
        kind: ErrorKind::InvalidArgument,
        message: error.to_string(),
        position: None,
    })?;
    encode_batch(&sealed.batch).map_err(|error| WireError {
        kind: ErrorKind::Internal,
        message: error.to_string(),
        position: None,
    })
}

/// Run `stream` to its end, a cancel, an error or the row cap, feeding `sink`.
pub async fn pump(
    mut stream: SendableRecordBatchStream,
    row_cap: u64,
    mut cancel: watch::Receiver<bool>,
    sink: &mut dyn Sink,
) -> Outcome {
    let columns = stream
        .schema()
        .fields()
        .iter()
        .map(|field| WireColumn {
            name: field.name().clone(),
            type_name: field.data_type().to_string(),
        })
        .collect();
    let mut rows = 0u64;
    let mut last_progress = Instant::now();
    if sink.columns(columns).await.is_err() {
        return Outcome {
            rows,
            truncated: false,
            cancelled: true,
            error: None,
        };
    }
    loop {
        let next = tokio::select! {
            biased;
            // `Ok(_)`: a dropped sender is not a cancel, and the arm is then disabled.
            Ok(_) = cancel.wait_for(|cancelled| *cancelled) => {
                return Outcome { rows, truncated: false, cancelled: true, error: None };
            }
            next = stream.next() => next,
        };
        let batch = match next {
            None => {
                return Outcome {
                    rows,
                    truncated: false,
                    cancelled: false,
                    error: None,
                }
            }
            Some(Err(error)) => return Outcome::failed(rows, classify(&error)),
            Some(Ok(batch)) => batch,
        };
        if batch.num_rows() == 0 {
            continue;
        }
        // At the cap already: a row exists beyond it, so the result was cut.
        let room = row_cap.saturating_sub(rows);
        if room == 0 {
            return Outcome {
                rows,
                truncated: true,
                cancelled: false,
                error: None,
            };
        }
        let cut = batch.num_rows() as u64 > room;
        let batch = if cut {
            batch.slice(0, room as usize)
        } else {
            batch
        };
        for piece in split(&batch) {
            let payload = match tokio::task::spawn_blocking(move || encode(piece)).await {
                Ok(Ok(payload)) => payload,
                Ok(Err(error)) => return Outcome::failed(rows, error),
                Err(join) => {
                    return Outcome::failed(
                        rows,
                        WireError {
                            kind: ErrorKind::Internal,
                            message: format!("the output conversion stopped: {join}"),
                            position: None,
                        },
                    )
                }
            };
            // A single row cannot be cut, so one huge cell can still make a piece the
            // protocol will not carry. Only this query fails; the helper keeps serving.
            if payload.len() > sink.max_payload() {
                return Outcome::failed(
                    rows,
                    WireError {
                        kind: ErrorKind::TooLarge,
                        message:
                            "A row of this result is larger than the analytics component can send."
                                .to_owned(),
                        position: None,
                    },
                );
            }
            if sink.chunk(payload).await.is_err() {
                return Outcome {
                    rows,
                    truncated: false,
                    cancelled: true,
                    error: None,
                };
            }
        }
        rows += batch.num_rows() as u64;
        if cut {
            return Outcome {
                rows,
                truncated: true,
                cancelled: false,
                error: None,
            };
        }
        if last_progress.elapsed() >= PROGRESS_EVERY {
            last_progress = Instant::now();
            if sink.progress(rows).await.is_err() {
                return Outcome {
                    rows,
                    truncated: false,
                    cancelled: true,
                    error: None,
                };
            }
        }
    }
}
