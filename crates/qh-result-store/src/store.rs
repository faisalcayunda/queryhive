//! The store itself: batches of encoded rows, a row index, and the spill file.

use std::borrow::Cow;
use std::fs::File;
use std::ops::Range;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use qh_core::{ColumnBatch, ColumnMeta, Value};
use thiserror::Error;

use crate::codec::{encode_batch, BatchView};

/// Everything that can go wrong reading or writing the store.
#[derive(Debug, Error)]
pub enum StoreError {
    /// The batch bytes are not a batch this build can read — a truncated or
    /// altered spill file, or a file from a build whose layout has since moved.
    /// Carries the detail so a diagnostics bundle says more than "corrupt".
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
}

/// How much memory the store may use before it starts spilling.
#[derive(Debug, Clone)]
pub struct StoreConfig {
    /// In-memory bytes above which the oldest batches move to disk.
    ///
    /// This is the knob behind the memory target in blueprint section 6: the
    /// store keeps the most recent batches in memory, because those are the ones
    /// a streaming grid is looking at, and pushes the older ones out.
    pub spill_threshold_bytes: usize,

    /// Where spill files go. `None` means a directory under the system temp
    /// directory; the app passes `~/Library/Caches/<app>/spill` instead.
    pub spill_dir: Option<PathBuf>,
}

impl Default for StoreConfig {
    fn default() -> Self {
        // 256 MB, the figure named in the blueprint's store design. Large enough
        // that an ordinary query never touches the disk, small enough to leave
        // room inside the 800 MB process budget for the UI and the grid.
        Self {
            spill_threshold_bytes: 256 * 1024 * 1024,
            spill_dir: None,
        }
    }
}

/// Where a batch currently lives.
enum Slot {
    /// Encoded bytes, ready to read.
    Memory(Vec<u8>),
    /// Written to the spill file at this offset, this many bytes long.
    Spilled { offset: u64, len: u64 },
}

/// A window of rows, with the index of its first row.
///
/// The start row is carried because a grid asks for a page it has already
/// decided on: without it, a window that was clamped at the end of the result
/// would be drawn at the wrong offset.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    /// Index of `values[0]` in the whole result.
    pub start_row: usize,
    /// Row-major, in the order the grid draws them.
    pub values: Vec<Vec<Value>>,
}

impl Window {
    pub fn rows(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn value(&self, row: usize, column: usize) -> Option<&Value> {
        self.values.get(row)?.get(column)
    }
}

/// The spill file, and how much of it is used.
struct SpillFile {
    path: PathBuf,
    file: File,
    next_offset: u64,
}

impl Drop for SpillFile {
    fn drop(&mut self) {
        // Best effort: a leftover spill file is cleaned up by the startup sweep
        // in `qh-storage`, so failing to remove it here is not worth an error
        // path the caller would have nowhere to report.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Every row a running query has produced, in a shape the grid can index.
pub struct ResultStore {
    columns: Vec<ColumnMeta>,
    slots: Vec<Slot>,
    /// `cumulative[i]` is the number of rows in every batch before `i`;
    /// `cumulative[slots.len()]` is the total. Monotonic, so finding the batch
    /// that holds row `n` is a binary search rather than a walk.
    cumulative: Vec<usize>,
    config: StoreConfig,
    memory_bytes: usize,
    spilled_bytes: usize,
    spill: Option<SpillFile>,
    spill_counter: u32,
}

impl ResultStore {
    /// A store for a result with these columns.
    ///
    /// The column list comes from the server and is known before any row is, so
    /// the store exists from the moment a query starts: a grid can draw headers
    /// while the first rows are still arriving.
    pub fn new(columns: Vec<ColumnMeta>, config: StoreConfig) -> Self {
        Self {
            columns,
            slots: Vec::new(),
            cumulative: vec![0],
            config,
            memory_bytes: 0,
            spilled_bytes: 0,
            spill: None,
            spill_counter: 0,
        }
    }

    /// Add a batch of rows.
    ///
    /// Batches arrive in order and are appended, never merged or reordered, so
    /// the row index stays a running total. Rows already delivered do not move:
    /// that is what lets the grid keep a scroll position while rows stream in.
    pub fn push(&mut self, batch: &ColumnBatch) -> Result<(), StoreError> {
        if batch.width() != self.columns.len() {
            return Err(StoreError::ColumnCount {
                expected: self.columns.len(),
                found: batch.width(),
            });
        }
        let encoded = encode_batch(batch.columns());
        let rows = batch.rows();
        self.memory_bytes += encoded.len();
        self.slots.push(Slot::Memory(encoded));
        self.cumulative
            .push(self.cumulative.last().copied().unwrap_or(0) + rows);
        self.spill_if_over_threshold()?;
        Ok(())
    }

    /// How many rows the result holds so far.
    pub fn rows(&self) -> usize {
        self.cumulative.last().copied().unwrap_or(0)
    }

    pub fn columns(&self) -> &[ColumnMeta] {
        &self.columns
    }

    /// Bytes currently held in memory.
    pub fn in_memory_bytes(&self) -> usize {
        self.memory_bytes
    }

    /// Bytes written to the spill file.
    pub fn spilled_bytes(&self) -> usize {
        self.spilled_bytes
    }

    /// The spill file's path, for the diagnostics bundle. `None` until the first
    /// spill.
    pub fn spill_path(&self) -> Option<&Path> {
        self.spill.as_ref().map(|spill| spill.path.as_path())
    }

    /// Read a window of rows.
    ///
    /// A range that starts past the end returns an empty window, and one that
    /// runs past the end is clamped. Neither is an error: the grid asks for a
    /// page while rows are still arriving, so it will ask for rows that do not
    /// exist yet, and that has to be an ordinary answer rather than a failure.
    pub fn window(&self, range: Range<usize>) -> Result<Window, StoreError> {
        let total = self.rows();
        let start = range.start.min(total);
        let end = range.end.min(total);
        if end <= start {
            return Ok(Window {
                start_row: start,
                values: Vec::new(),
            });
        }

        let mut values = Vec::with_capacity(end - start);
        let mut index = self.batch_index_for(start);
        let mut row = start;

        while row < end {
            let bytes = self.load_batch(index)?;
            let view = BatchView::parse(&bytes)?;
            // A spilled batch could have been written by a build whose column
            // list differed. Checking the width here turns that into an error
            // that names the batch rather than a wrong cell value.
            if view.width() != self.columns.len() {
                return Err(StoreError::Corrupt {
                    detail: format!(
                        "batch {index} has {} columns but the result declares {}",
                        view.width(),
                        self.columns.len()
                    ),
                });
            }
            let base = self.cumulative[index];
            let batch_rows = view.rows();
            let last = (base + batch_rows).min(end);

            for in_batch in (row - base)..(last - base) {
                let mut cells = Vec::with_capacity(self.columns.len());
                for column in 0..self.columns.len() {
                    cells.push(view.value(in_batch, column)?);
                }
                values.push(cells);
            }

            row = base + batch_rows;
            index += 1;
        }

        Ok(Window {
            start_row: start,
            values,
        })
    }

    /// The batch that holds `row`.
    ///
    /// Finds the last batch whose start is at or before `row`. The row index is
    /// monotonic, so this is a partition point rather than a scan.
    fn batch_index_for(&self, row: usize) -> usize {
        debug_assert!(!self.cumulative.is_empty());
        let position = self.cumulative.partition_point(|&start| start <= row);
        position.saturating_sub(1)
    }

    /// The bytes of one batch, from memory or from the spill file.
    fn load_batch(&self, index: usize) -> Result<Cow<'_, [u8]>, StoreError> {
        match self.slots.get(index) {
            Some(Slot::Memory(bytes)) => Ok(Cow::Borrowed(bytes)),
            Some(Slot::Spilled { offset, len }) => {
                let spill = self.spill.as_ref().ok_or_else(|| StoreError::Corrupt {
                    detail: format!("batch {index} is marked spilled but there is no spill file"),
                })?;
                let mut buffer = vec![0u8; *len as usize];
                spill.file.read_exact_at(&mut buffer, *offset)?;
                Ok(Cow::Owned(buffer))
            }
            None => Err(StoreError::Corrupt {
                detail: format!("no batch {index}"),
            }),
        }
    }

    /// Move the oldest in-memory batches to disk until the store is under its
    /// threshold.
    ///
    /// Oldest first, deliberately: a streaming grid reads the rows that just
    /// arrived, so the newest batches are the ones worth keeping in memory.
    fn spill_if_over_threshold(&mut self) -> Result<(), StoreError> {
        if self.memory_bytes <= self.config.spill_threshold_bytes {
            return Ok(());
        }
        // Taken out and put back so the write loop can borrow `self.slots`
        // immutably while writing.
        let mut spill = match self.spill.take() {
            Some(existing) => existing,
            None => self.create_spill_file()?,
        };

        for index in 0..self.slots.len() {
            if self.memory_bytes <= self.config.spill_threshold_bytes {
                break;
            }
            let (offset, len) = {
                let bytes = match &self.slots[index] {
                    Slot::Memory(bytes) => bytes,
                    Slot::Spilled { .. } => continue,
                };
                let offset = spill.next_offset;
                spill.file.write_all_at(bytes, offset)?;
                (offset, bytes.len() as u64)
            };
            spill.next_offset += len;
            self.memory_bytes -= len as usize;
            self.spilled_bytes += len as usize;
            self.slots[index] = Slot::Spilled { offset, len };
        }

        self.spill = Some(spill);
        Ok(())
    }

    fn create_spill_file(&mut self) -> Result<SpillFile, StoreError> {
        let directory = match &self.config.spill_dir {
            Some(given) => given.clone(),
            None => std::env::temp_dir().join("queryhive-spill"),
        };
        std::fs::create_dir_all(&directory)?;
        // The process id plus a counter keeps two stores in one process, or two
        // runs, from writing to the same file.
        self.spill_counter += 1;
        let path = directory.join(format!(
            "spill-{}-{}.bin",
            std::process::id(),
            self.spill_counter
        ));
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        Ok(SpillFile {
            path,
            file,
            next_offset: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A private directory per test, so parallel tests cannot collide and the
    /// spill file's removal is observable.
    fn config(threshold: usize) -> (StoreConfig, PathBuf) {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let directory = std::env::temp_dir().join(format!(
            "qh-store-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        (
            StoreConfig {
                spill_threshold_bytes: threshold,
                spill_dir: Some(directory.clone()),
            },
            directory,
        )
    }

    fn columns() -> Vec<ColumnMeta> {
        vec![
            ColumnMeta::new("id", "integer"),
            ColumnMeta::new("name", "varchar"),
        ]
    }

    /// `rows` rows of `(id, text)` in batches of `per_batch`.
    fn batch(start: i64, per_batch: i64) -> ColumnBatch {
        let ids: Vec<Value> = (start..start + per_batch).map(Value::Int).collect();
        let names: Vec<Value> = (start..start + per_batch)
            .map(|n| Value::Text(format!("name-{n}").into()))
            .collect();
        ColumnBatch::new(vec![ids, names]).unwrap()
    }

    fn fill(store: &mut ResultStore, batches: i64, per_batch: i64) {
        for index in 0..batches {
            store.push(&batch(index * per_batch, per_batch)).unwrap();
        }
    }

    #[test]
    fn rows_accumulate_across_batches() {
        let (config, _dir) = config(usize::MAX);
        let mut store = ResultStore::new(columns(), config);
        assert_eq!(store.rows(), 0);

        fill(&mut store, 3, 4);
        assert_eq!(store.rows(), 12);
        assert_eq!(store.columns().len(), 2);
    }

    #[test]
    fn a_window_reads_the_rows_it_asked_for() {
        let (config, _dir) = config(usize::MAX);
        let mut store = ResultStore::new(columns(), config);
        fill(&mut store, 3, 4);

        let window = store.window(5..8).unwrap();
        assert_eq!(window.start_row, 5);
        assert_eq!(window.rows(), 3);
        assert_eq!(window.value(0, 0), Some(&Value::Int(5)));
        assert_eq!(window.value(0, 1), Some(&Value::Text("name-5".into())));
        assert_eq!(window.value(2, 0), Some(&Value::Int(7)));
    }

    #[test]
    fn a_window_spans_a_batch_boundary() {
        // The case a naive per-batch implementation gets wrong: rows 3..5 of a
        // store whose batches hold four rows each.
        let (config, _dir) = config(usize::MAX);
        let mut store = ResultStore::new(columns(), config);
        fill(&mut store, 3, 4);

        let window = store.window(2..6).unwrap();
        assert_eq!(window.start_row, 2);
        let ids: Vec<&Value> = (0..window.rows())
            .map(|row| window.value(row, 0).unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![
                &Value::Int(2),
                &Value::Int(3),
                &Value::Int(4),
                &Value::Int(5)
            ]
        );
    }

    #[test]
    fn a_window_past_the_end_is_clamped_not_an_error() {
        // A streaming grid asks for a page while rows are still arriving, so
        // asking for rows that do not exist yet is an ordinary answer.
        let (config, _dir) = config(usize::MAX);
        let mut store = ResultStore::new(columns(), config);
        fill(&mut store, 1, 4);

        let clamped = store.window(2..100).unwrap();
        assert_eq!(clamped.start_row, 2);
        assert_eq!(clamped.rows(), 2);

        let beyond = store.window(50..60).unwrap();
        assert_eq!(beyond.start_row, 4);
        assert!(beyond.is_empty());
    }

    #[test]
    fn a_window_inside_one_batch_reads_only_that_batch() {
        let (config, _dir) = config(usize::MAX);
        let mut store = ResultStore::new(columns(), config);
        fill(&mut store, 2, 8);

        let window = store.window(9..11).unwrap();
        assert_eq!(window.value(0, 0), Some(&Value::Int(9)));
        assert_eq!(window.value(1, 0), Some(&Value::Int(10)));
    }

    #[test]
    fn a_window_on_an_empty_store_is_empty() {
        let (config, _dir) = config(usize::MAX);
        let store = ResultStore::new(columns(), config);
        assert_eq!(store.rows(), 0);
        assert!(store.window(0..10).unwrap().is_empty());
    }

    #[test]
    fn a_push_with_the_wrong_width_is_refused() {
        let (config, _dir) = config(usize::MAX);
        let mut store = ResultStore::new(columns(), config);
        let wrong = ColumnBatch::new(vec![vec![Value::Int(1)]]).unwrap();

        let error = store.push(&wrong).unwrap_err();
        assert!(
            matches!(
                error,
                StoreError::ColumnCount {
                    expected: 2,
                    found: 1
                }
            ),
            "{error}"
        );
        assert_eq!(
            store.rows(),
            0,
            "a refused batch must not move the row count"
        );
    }

    #[test]
    fn spill_happens_past_the_threshold() {
        let (config, _dir) = config(1024);
        let mut store = ResultStore::new(columns(), config);
        fill(&mut store, 20, 32);

        assert!(store.spilled_bytes() > 0, "nothing was spilled");
        assert!(
            store.in_memory_bytes() <= 1024,
            "still {} bytes in memory",
            store.in_memory_bytes()
        );
        assert_eq!(store.rows(), 640);
    }

    #[test]
    fn spilled_rows_read_back_identically() {
        // The point of one encoder shared by both forms: spilling must not be
        // able to change a value.
        let (small, _dir_a) = config(1024);
        let (huge, _dir_b) = config(usize::MAX);

        let mut spilled = ResultStore::new(columns(), small);
        let mut resident = ResultStore::new(columns(), huge);
        fill(&mut spilled, 20, 32);
        fill(&mut resident, 20, 32);

        assert!(spilled.spilled_bytes() > 0);

        for range in [0..5, 100..140, 300..333, 635..640, 500..640] {
            let from_disk = spilled.window(range.clone()).unwrap();
            let from_memory = resident.window(range.clone()).unwrap();
            assert_eq!(from_disk, from_memory, "range {range:?}");
        }
    }

    #[test]
    fn a_window_spanning_memory_and_disk_is_one_contiguous_answer() {
        let (config, _dir) = config(2048);
        let mut store = ResultStore::new(columns(), config);
        fill(&mut store, 20, 32);

        // The last batches stay in memory and the first went to disk, so this
        // range necessarily crosses the boundary between the two.
        let window = store.window(0..640).unwrap();
        assert_eq!(window.rows(), 640);
        for row in 0..640 {
            assert_eq!(
                window.value(row, 0),
                Some(&Value::Int(row as i64)),
                "row {row} came back wrong"
            );
        }
    }

    #[test]
    fn the_spill_file_is_removed_when_the_store_goes_away() {
        // A crash leaves spill files behind; the startup sweep exists for that.
        // A clean close must not depend on the sweep.
        let (config, _dir) = config(1024);
        let path = {
            let mut store = ResultStore::new(columns(), config);
            fill(&mut store, 20, 32);
            let path = store
                .spill_path()
                .expect("nothing was spilled")
                .to_path_buf();
            assert!(path.exists(), "the spill file was never written");
            path
        };
        assert!(!path.exists(), "the spill file outlived the store");
    }

    #[test]
    fn no_spill_file_is_created_when_nothing_spills() {
        let (config, _dir) = config(usize::MAX);
        let mut store = ResultStore::new(columns(), config);
        fill(&mut store, 5, 8);
        assert_eq!(store.spilled_bytes(), 0);
        assert!(
            store.spill_path().is_none(),
            "a spill file appeared for no reason"
        );
    }
}
