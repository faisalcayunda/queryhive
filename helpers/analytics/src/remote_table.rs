//! `RemoteStoreTable`: a result that lives in the app, queryable here (blueprint
//! section 14.4).
//!
//! The helper holds no copy of the data. A scan of the table is an `ExecutionPlan` whose
//! partitions pull chunks one request at a time from a [`ChunkSource`], which in the real
//! helper is the pipe to the app and in tests is a mock. A chunk arrives already in the
//! *logical* schema of section 5.4 (the app converts), so what DataFusion sees is plain
//! Arrow with the types and names the UI showed.
//!
//! Memory in transit is bounded: chunks are dealt to partitions round-robin, and each
//! partition keeps at most [`CREDITS`] requests open, so a scan holds at most
//! `partitions x CREDITS` chunks of at most 2 MiB however large the result is.
//!
//! Filters are not pushed down (`Unsupported`, DataFusion applies them above the scan),
//! and a `LIMIT` is honoured per partition, so a `LIMIT 10` over a million rows asks the
//! app for a chunk or two, not for all of them.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::arrow::record_batch::{RecordBatch, RecordBatchOptions};
use datafusion::catalog::Session;
use datafusion::common::tree_node::TreeNodeRecursion;
use datafusion::common::{DataFusionError, Result, Statistics};
use datafusion::datasource::{TableProvider, TableType};
use datafusion::execution::{SendableRecordBatchStream, TaskContext};
use datafusion::logical_expr::Expr;
use datafusion::physical_expr::{EquivalenceProperties, PhysicalExpr};
use datafusion::physical_plan::execution_plan::{Boundedness, EmissionType};
use datafusion::physical_plan::statistics::StatisticsArgs;
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::{
    ChildrenPropertiesMode, DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning,
    PlanProperties, ReplaceChildrenOptions,
};
use futures::{future, StreamExt};

/// Requests a partition may have open at once (blueprint section 14.4).
pub const CREDITS: usize = 2;

/// Where chunks come from. `columns` are indices into the registered (logical) schema.
#[async_trait]
pub trait ChunkSource: Send + Sync + fmt::Debug {
    /// Chunk `chunk`, restricted to `columns`, in the logical types. An error is shown to
    /// the user as the query's error, so its message should say what happened ("the
    /// result was closed"), not how.
    async fn fetch(
        &self,
        chunk: u32,
        columns: &[usize],
    ) -> std::result::Result<RecordBatch, String>;
}

/// A finished result in the app, as a table.
#[derive(Debug)]
pub struct RemoteStoreTable {
    schema: SchemaRef,
    rows: u64,
    chunks: u32,
    source: Arc<dyn ChunkSource>,
}

impl RemoteStoreTable {
    pub fn new(schema: SchemaRef, rows: u64, chunks: u32, source: Arc<dyn ChunkSource>) -> Self {
        Self {
            schema,
            rows,
            chunks,
            source,
        }
    }

    pub fn rows(&self) -> u64 {
        self.rows
    }
}

#[async_trait]
impl TableProvider for RemoteStoreTable {
    fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        _filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let columns: Vec<usize> = match projection {
            Some(columns) => columns.clone(),
            None => (0..self.schema.fields().len()).collect(),
        };
        let schema = Arc::new(self.schema.project(&columns)?);
        let target = state.config_options().execution.target_partitions.max(1);
        let partitions = target.min(self.chunks.max(1) as usize);
        Ok(Arc::new(RemoteScanExec::new(
            schema,
            columns,
            self.rows,
            self.chunks,
            partitions,
            limit,
            Arc::clone(&self.source),
        )))
    }
}

/// The scan: partition `p` reads chunks `p, p + P, p + 2P, ...`.
#[derive(Debug)]
pub struct RemoteScanExec {
    schema: SchemaRef,
    columns: Vec<usize>,
    rows: u64,
    chunks: u32,
    partitions: usize,
    limit: Option<usize>,
    source: Arc<dyn ChunkSource>,
    properties: Arc<PlanProperties>,
}

impl RemoteScanExec {
    fn new(
        schema: SchemaRef,
        columns: Vec<usize>,
        rows: u64,
        chunks: u32,
        partitions: usize,
        limit: Option<usize>,
        source: Arc<dyn ChunkSource>,
    ) -> Self {
        let properties = PlanProperties::new(
            EquivalenceProperties::new(Arc::clone(&schema)),
            Partitioning::UnknownPartitioning(partitions),
            EmissionType::Incremental,
            Boundedness::Bounded,
        );
        Self {
            schema,
            columns,
            rows,
            chunks,
            partitions,
            limit,
            source,
            properties: Arc::new(properties),
        }
    }
}

impl DisplayAs for RemoteScanExec {
    fn fmt_as(&self, kind: DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        match kind {
            DisplayFormatType::Default | DisplayFormatType::Verbose => write!(
                f,
                "RemoteScanExec: chunks={}, partitions={}, columns={}",
                self.chunks,
                self.partitions,
                self.columns.len()
            ),
            DisplayFormatType::TreeRender => Ok(()),
        }
    }
}

impl ExecutionPlan for RemoteScanExec {
    fn name(&self) -> &str {
        "RemoteScanExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![]
    }

    fn apply_expressions(
        &self,
        _f: &mut dyn FnMut(&Arc<dyn PhysicalExpr>) -> Result<TreeNodeRecursion>,
    ) -> Result<TreeNodeRecursion> {
        Ok(TreeNodeRecursion::Continue)
    }

    fn replace_children(
        self: Arc<Self>,
        _children: Vec<Arc<dyn ExecutionPlan>>,
        _options: ReplaceChildrenOptions,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        Ok(self)
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        self.replace_children(
            children,
            ReplaceChildrenOptions::new(ChildrenPropertiesMode::Recompute),
        )
    }

    fn execute(
        &self,
        partition: usize,
        _context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        if partition >= self.partitions {
            return Err(DataFusionError::Internal(format!(
                "RemoteScanExec has {} partitions, asked for {partition}",
                self.partitions
            )));
        }
        let mine: Vec<u32> = (partition as u32..self.chunks)
            .step_by(self.partitions)
            .collect();
        let schema = Arc::clone(&self.schema);
        let source = Arc::clone(&self.source);
        // A scan that needs no columns (`count(*)`) still has to learn how many rows each
        // chunk holds, so it asks for the first column and drops it.
        let wanted = if self.columns.is_empty() {
            vec![0]
        } else {
            self.columns.clone()
        };
        let conform_to = Arc::clone(&schema);
        let fetches = futures::stream::iter(mine)
            .map(move |chunk| {
                let source = Arc::clone(&source);
                let wanted = wanted.clone();
                async move {
                    source
                        .fetch(chunk, &wanted)
                        .await
                        .map_err(DataFusionError::Execution)
                }
            })
            // `buffered` keeps at most CREDITS fetches in flight and yields them in order.
            .buffered(CREDITS)
            .map(move |fetched| fetched.and_then(|batch| conform(batch, &conform_to)));
        let stream = fetches.scan(self.limit, |remaining, item| {
            let out = match (*remaining, item) {
                (Some(0), _) => return future::ready(None),
                (Some(left), Ok(batch)) => {
                    let take = batch.num_rows().min(left);
                    *remaining = Some(left - take);
                    Ok(batch.slice(0, take))
                }
                (None, item) | (Some(_), item @ Err(_)) => item,
            };
            future::ready(Some(out))
        });
        Ok(Box::pin(RecordBatchStreamAdapter::new(schema, stream)))
    }

    fn statistics_from_inputs(
        &self,
        _input_stats: &[Arc<Statistics>],
        args: &StatisticsArgs,
    ) -> Result<Arc<Statistics>> {
        use datafusion::common::stats::Precision;
        let mut stats = Statistics::new_unknown(&self.schema);
        let rows = usize::try_from(self.rows).unwrap_or(usize::MAX);
        stats.num_rows = match (args.partition(), self.limit) {
            // The whole table, nothing cut off: the count is exact.
            (None, None) => Precision::Exact(rows),
            (None, Some(limit)) => Precision::Inexact(rows.min(limit)),
            (Some(_), _) => Precision::Inexact(rows / self.partitions.max(1)),
        };
        Ok(Arc::new(stats))
    }
}

/// Check a chunk against what the scan promised, and give it the scan's own schema.
///
/// A chunk that disagrees (a different number of columns, a different type) is an error,
/// not something to coerce: the operators above this scan downcast columns by the type the
/// plan says, and a wrong one would be a panic inside DataFusion.
fn conform(batch: RecordBatch, schema: &SchemaRef) -> Result<RecordBatch> {
    let wide = schema.fields().is_empty();
    if !wide && batch.num_columns() != schema.fields().len() {
        return Err(DataFusionError::Execution(format!(
            "a chunk has {} columns, the scan expected {}",
            batch.num_columns(),
            schema.fields().len()
        )));
    }
    if wide {
        let options = RecordBatchOptions::new().with_row_count(Some(batch.num_rows()));
        return Ok(RecordBatch::try_new_with_options(
            Arc::clone(schema),
            vec![],
            &options,
        )?);
    }
    for (field, column) in schema.fields().iter().zip(batch.columns()) {
        if field.data_type() != column.data_type() {
            return Err(DataFusionError::Execution(format!(
                "a chunk column {} is {} but the result was registered as {}",
                field.name(),
                column.data_type(),
                field.data_type()
            )));
        }
    }
    Ok(RecordBatch::try_new(
        Arc::clone(schema),
        batch.columns().to_vec(),
    )?)
}
