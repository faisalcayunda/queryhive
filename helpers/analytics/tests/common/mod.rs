//! Helpers shared by the integration tests. Not every test uses every helper.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use datafusion::arrow::array::{ArrayRef, Int64Array, StringArray};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::arrow::util::display::array_value_to_string;
use datafusion::datasource::MemTable;
use futures::TryStreamExt;
use qh_analytics::remote_table::ChunkSource;
use qh_analytics::session::{Engine, EngineConfig, Failure, Session};
use qh_result_store::{logical_batch, LogicalSchema, StoreChunk, StoreHandle};

/// An engine with spill in a fresh directory. Returns the directory so it lives as long
/// as the test does.
pub fn engine(partitions: usize) -> (Engine, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut config = EngineConfig::new(Some(dir.path().join("spill")));
    config.target_partitions = partitions;
    (Engine::new(config).unwrap(), dir)
}

/// Run `sql` to the end and return the batches.
pub async fn collect(session: &Session, sql: &str) -> Result<Vec<RecordBatch>, Failure> {
    let stream = session.query(sql).await?;
    stream.try_collect().await.map_err(Failure::from)
}

/// Every cell of every batch as Arrow's display text; `None` is NULL.
pub fn texts(batches: &[RecordBatch]) -> Vec<Vec<Option<String>>> {
    let mut rows = Vec::new();
    for batch in batches {
        for row in 0..batch.num_rows() {
            rows.push(
                batch
                    .columns()
                    .iter()
                    .map(|column| {
                        if datafusion::arrow::array::Array::is_null(column, row) {
                            None
                        } else {
                            Some(array_value_to_string(column, row).unwrap())
                        }
                    })
                    .collect(),
            );
        }
    }
    rows
}

/// A table of `rows` rows: `k` (Int64, descending from `rows`) and `s` (a long text that
/// starts with `canary`), in batches of 8,192.
pub fn big_table(rows: usize, canary: &str) -> Arc<MemTable> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("k", DataType::Int64, false),
        Field::new("s", DataType::Utf8, false),
    ]));
    let mut batches = Vec::new();
    let mut start = 0;
    while start < rows {
        let end = (start + 8_192).min(rows);
        let keys: Vec<i64> = (start..end).map(|i| (rows - i) as i64).collect();
        let text: Vec<String> = (start..end).map(|i| format!("{canary}-{i:09}")).collect();
        let columns: Vec<ArrayRef> = vec![
            Arc::new(Int64Array::from(keys)),
            Arc::new(StringArray::from(text)),
        ];
        batches.push(RecordBatch::try_new(schema.clone(), columns).unwrap());
        start = end;
    }
    Arc::new(MemTable::try_new(schema, vec![batches]).unwrap())
}

/// Wait up to `seconds` for `condition`, polling.
pub async fn eventually(seconds: u64, mut condition: impl FnMut() -> bool) -> bool {
    for _ in 0..seconds * 50 {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    condition()
}

/// What the app would do for a registered result: serve its chunks, in the logical
/// types, projected to the columns asked for. Counts concurrent requests.
pub struct StoreSource {
    pub chunks: Vec<Arc<StoreChunk>>,
    pub schema: Arc<LogicalSchema>,
    pub delay: Duration,
    pub in_flight: AtomicUsize,
    pub peak: AtomicUsize,
    pub requests: AtomicUsize,
    /// When set, every request fails with this message.
    pub closed: std::sync::Mutex<Option<String>>,
    /// The `columns` of every request, in order.
    pub asked: std::sync::Mutex<Vec<Vec<usize>>>,
}

impl std::fmt::Debug for StoreSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoreSource")
    }
}

impl StoreSource {
    pub fn new(store: &StoreHandle, schema: LogicalSchema) -> Arc<Self> {
        Self::with_delay(store, schema, Duration::ZERO)
    }

    pub fn with_delay(store: &StoreHandle, schema: LogicalSchema, delay: Duration) -> Arc<Self> {
        let chunks = store
            .shared()
            .chunk_refs()
            .into_iter()
            .map(|reference| store.shared().load_chunk(reference.index).unwrap())
            .collect();
        Arc::new(Self {
            chunks,
            schema: Arc::new(schema),
            delay,
            in_flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            requests: AtomicUsize::new(0),
            closed: std::sync::Mutex::new(None),
            asked: std::sync::Mutex::new(Vec::new()),
        })
    }
}

impl StoreSource {
    /// What the app sends for one request: chunk `chunk`, projected to `columns`, in the
    /// logical types.
    pub fn chunk_batch(&self, chunk: u32, columns: &[usize]) -> Result<RecordBatch, String> {
        if let Some(message) = self.closed.lock().unwrap().clone() {
            return Err(message);
        }
        let chunk = self
            .chunks
            .get(chunk as usize)
            .ok_or_else(|| "no such chunk".to_owned())?;
        let fields: Vec<_> = columns
            .iter()
            .map(|&column| self.schema.schema.field(column).clone())
            .collect();
        let projected = LogicalSchema {
            schema: Arc::new(Schema::new(fields)),
            types: columns
                .iter()
                .map(|&c| self.schema.types[c].clone())
                .collect(),
            names: columns
                .iter()
                .map(|&c| self.schema.names[c].clone())
                .collect(),
        };
        logical_batch(chunk, columns, &projected).map_err(|error| error.to_string())
    }
}

#[async_trait]
impl ChunkSource for StoreSource {
    async fn fetch(&self, chunk: u32, columns: &[usize]) -> Result<RecordBatch, String> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        self.asked.lock().unwrap().push(columns.to_vec());
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.chunk_batch(chunk, columns)
    }
}

// ---- the output side: what a query sends, decoded ---------------------------------

use qh_analytics::output::{pump, Closed, Outcome, Sink};
use qh_analytics_proto::{decode_batch, WireColumn};
use qh_core::Value;
use tokio::sync::watch;

/// A sink that keeps everything it is given.
#[derive(Default)]
pub struct Collect {
    pub columns: Vec<WireColumn>,
    pub payloads: Vec<Vec<u8>>,
    pub progress: Vec<u64>,
}

#[async_trait]
impl Sink for Collect {
    async fn columns(&mut self, columns: Vec<WireColumn>) -> Result<(), Closed> {
        self.columns = columns;
        Ok(())
    }
    async fn chunk(&mut self, payload: Vec<u8>) -> Result<(), Closed> {
        self.payloads.push(payload);
        Ok(())
    }
    async fn progress(&mut self, rows: u64) -> Result<(), Closed> {
        self.progress.push(rows);
        Ok(())
    }
}

impl Collect {
    /// The chunks as the app would receive them.
    pub fn chunks(&self) -> Vec<RecordBatch> {
        self.payloads
            .iter()
            .map(|payload| decode_batch(payload.clone()).expect("a chunk the app can decode"))
            .collect()
    }

    /// Every cell as the app's own reader sees it: by `qh.enc`, with `value_at`.
    pub fn cells(&self) -> Vec<Vec<Value>> {
        let mut rows = Vec::new();
        for batch in self.chunks() {
            let encodings: Vec<_> = batch
                .schema()
                .fields()
                .iter()
                .map(|field| qh_columnar::encoding::encoding_of(field).expect("a store encoding"))
                .collect();
            for row in 0..batch.num_rows() {
                rows.push(
                    (0..batch.num_columns())
                        .map(|column| {
                            qh_columnar::value_at(
                                batch.column(column).as_ref(),
                                encodings[column],
                                row,
                            )
                            .expect("a readable cell")
                        })
                        .collect(),
                );
            }
        }
        rows
    }

    /// The `qh.enc` of each column of the first chunk.
    pub fn encodings(&self) -> Vec<String> {
        let chunks = self.chunks();
        chunks[0]
            .schema()
            .fields()
            .iter()
            .map(|field| field.metadata()["qh.enc"].clone())
            .collect()
    }
}

/// Plan and pump `sql` with a row cap, as a query does.
pub async fn run(session: &Session, sql: &str, row_cap: u64) -> (Collect, Outcome) {
    let (_keep, cancelled) = watch::channel(false);
    let mut sink = Collect::default();
    let outcome = match session.query(sql).await {
        Ok(stream) => pump(stream, row_cap, cancelled, &mut sink).await,
        Err(failure) => Outcome {
            rows: 0,
            truncated: false,
            cancelled: false,
            error: Some(failure.0),
        },
    };
    (sink, outcome)
}
