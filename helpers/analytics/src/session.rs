//! The engine, the sessions, and how DataFusion's errors become the user's errors.
//!
//! One [`Engine`] per helper process owns what is shared: the [`LeasePool`], the encrypted
//! spill, and so the `RuntimeEnv` every query runs in. One [`Session`] per tab owns what is
//! not: its tables, by name. Two tabs can both have a table called `r` without meeting.
//!
//! # The SQL surface is locked, twice
//!
//! Every statement goes through `sql_with_options` with DDL, DML and statements switched
//! off. `CREATE EXTERNAL TABLE`, `COPY ... TO`, `INSERT` and `SET` are refused after the
//! logical plan is built and before anything executes; `COPY` counts as DML. URL tables
//! (`SELECT * FROM '/etc/hosts'`) are never enabled, so SQL cannot name a path the app did
//! not register. The second lock is the kernel's (section 14.9): were the first ever to
//! leak, the process still could not write a file or open a socket.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use datafusion::common::TableReference;
use datafusion::datasource::TableProvider;
use datafusion::error::DataFusionError;
use datafusion::execution::context::SQLOptions;
use datafusion::execution::disk_manager::{DiskManagerBuilder, DiskManagerMode};
use datafusion::execution::runtime_env::{RuntimeEnv, RuntimeEnvBuilder};
use datafusion::execution::SendableRecordBatchStream;
use datafusion::prelude::{SessionConfig, SessionContext};
use qh_analytics_proto::{ErrorKind, FileFormat, Position, TableInfo, WireColumn, WireError};

use crate::pool::{Lease, LeasePool, OUT_OF_BUDGET};
use crate::spill::{EncryptedTempFiles, SpillError, SpillSetup, SpillStats};
use crate::udf::natural_udf;

/// Default cap on ciphertext in the helper's spill at any moment.
pub const DEFAULT_SPILL_QUOTA: u64 = 32 << 30;
const MAX_NAME_LEN: usize = 128;
const DEFAULT_BATCH_SIZE: usize = 8192;
/// Lease per partition. A sort holds about 7 MiB per partition at its merge, in memory it
/// cannot spill, so partitions are as many as the lease pays for: on a 32 MiB lease four
/// partitions ran out of memory in four runs out of ten, and two never did (0 of 25, also
/// with the machine loaded). The count is read from the pool's limit when a statement is
/// planned, so it follows the lease the app has just granted.
const LEASE_PER_PARTITION: usize = 16 << 20;
const DEFAULT_SPILLER_RESERVE_DIVISOR: usize = 4;
/// Bytes a spillable sort reserves for merging its runs: see [`Engine::session`].
const SORT_MERGE_RESERVATION: usize = 2 << 20;
/// Spill files one merge pass reads at once. DataFusion's default is unlimited, and every
/// open run holds a batch in memory the pool cannot make a sort give back: a big sort on a
/// small lease ended with hundreds of runs open and `SortPreservingMergeExec` refused 288 KB
/// from a 32 MiB lease. A fan-in of 8 makes the merge go in levels (more spill I/O) and
/// bounds what it holds at eight batches per partition.
const MERGE_FAN_IN: usize = 8;

/// A failure the app can be told about: a kind and a message fit for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure(pub WireError);

impl Failure {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self(WireError {
            kind: ErrorKind::InvalidArgument,
            message: message.into(),
            position: None,
        })
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self(WireError {
            kind: ErrorKind::Internal,
            message: message.into(),
            position: None,
        })
    }
}

impl From<DataFusionError> for Failure {
    fn from(error: DataFusionError) -> Self {
        Self(classify(&error))
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0.message)
    }
}

/// Sort a DataFusion error into the families of section 14.13.
///
/// * a reservation the lease cannot hold is `TooLarge`, with the budget message and not
///   DataFusion's accounting detail;
/// * a spill failure, however deeply wrapped, is `Spill`;
/// * a bug (`Internal`) and an I/O error that is not ours are `Internal`;
/// * everything else is the user's SQL, table, file or data: `InvalidArgument`.
pub fn classify(error: &DataFusionError) -> WireError {
    if let DataFusionError::Shared(inner) = error {
        return classify(inner);
    }
    let root = error.find_root();
    let (kind, message) = match root {
        DataFusionError::ResourcesExhausted(_) => (ErrorKind::TooLarge, OUT_OF_BUDGET.to_owned()),
        DataFusionError::External(inner) => match inner.downcast_ref::<SpillError>() {
            Some(spill) => (ErrorKind::Spill, spill.to_string()),
            None => (ErrorKind::Internal, root.message().into_owned()),
        },
        DataFusionError::IoError(io) => {
            match io.get_ref().and_then(|e| e.downcast_ref::<SpillError>()) {
                Some(spill) => (ErrorKind::Spill, spill.to_string()),
                None => (ErrorKind::Internal, io.to_string()),
            }
        }
        DataFusionError::Internal(_) => (ErrorKind::Internal, root.message().into_owned()),
        DataFusionError::SQL(inner, _) => (ErrorKind::InvalidArgument, inner.to_string()),
        _ => (ErrorKind::InvalidArgument, root.message().into_owned()),
    };
    let position = position_in(&message);
    WireError {
        kind,
        message,
        position,
    }
}

/// `sqlparser` writes where it stopped as `at Line: 1, Column: 15`.
fn position_in(message: &str) -> Option<Position> {
    let after_line = message.split("Line: ").nth(1)?;
    let (line, rest) = after_line.split_once(',')?;
    let column = rest.trim_start().strip_prefix("Column: ")?;
    let digits: String = column.chars().take_while(char::is_ascii_digit).collect();
    Some(Position {
        line: line.trim().parse().ok()?,
        column: digits.parse().ok()?,
    })
}

/// What an engine is built from.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// The helper's spill directory. `None` means no spill at all.
    pub spill_dir: Option<PathBuf>,
    pub spill_quota: u64,
    pub target_partitions: usize,
    /// Rows per batch DataFusion works in. Every buffer a sort cannot spill (a merge's
    /// open runs, a repartition's queues) is a number of batches, so this is also the size
    /// of the unspillable part of a query.
    pub batch_size: usize,
    /// Spillers keep out of the last `1/N` of a lease ([`LeasePool::with_reserve_divisor`]).
    pub spiller_reserve_divisor: usize,
    /// Spill files one merge pass reads at once ([`MERGE_FAN_IN`]).
    pub merge_fan_in: usize,
}

impl EngineConfig {
    pub fn new(spill_dir: Option<PathBuf>) -> Self {
        Self {
            spill_dir,
            spill_quota: DEFAULT_SPILL_QUOTA,
            // Performance cores: 4 on an M4, where 4 partitions beat 10 (section 2.7).
            target_partitions: qh_rt::cores().performance.max(1),
            batch_size: DEFAULT_BATCH_SIZE,
            spiller_reserve_divisor: DEFAULT_SPILLER_RESERVE_DIVISOR,
            merge_fan_in: MERGE_FAN_IN,
        }
    }
}

/// Shared by every query in the process.
pub struct Engine {
    runtime: Arc<RuntimeEnv>,
    pool: Arc<LeasePool>,
    spill: Option<Arc<EncryptedTempFiles>>,
    spill_reason: Option<String>,
    target_partitions: usize,
    batch_size: usize,
}

impl Engine {
    pub fn new(config: EngineConfig) -> Result<Self, Failure> {
        let pool = LeasePool::with_reserve_divisor(config.spiller_reserve_divisor);
        let setup = match &config.spill_dir {
            Some(dir) => EncryptedTempFiles::open(dir, config.spill_quota),
            None => SpillSetup::Disabled {
                reason: "no spill directory was given".to_owned(),
            },
        };
        let (mode, spill, spill_reason) = match setup {
            SpillSetup::Enabled(files) => {
                (DiskManagerMode::Custom(files.clone()), Some(files), None)
            }
            // `Disabled`, never `OsTmpDirectory`: an operator that needs to spill fails
            // with the budget message instead of writing plaintext to `$TMPDIR`.
            SpillSetup::Disabled { reason } => (DiskManagerMode::Disabled, None, Some(reason)),
        };
        let runtime = RuntimeEnvBuilder::new()
            .with_memory_pool(pool.clone())
            .with_disk_manager_builder(
                DiskManagerBuilder::default()
                    .with_mode(mode)
                    .with_max_spill_merge_fan_in(config.merge_fan_in),
            )
            .build_arc()?;
        Ok(Self {
            runtime,
            pool,
            spill,
            spill_reason,
            target_partitions: config.target_partitions.max(1),
            batch_size: config.batch_size.max(1),
        })
    }

    /// Lend a query `bytes` of memory until the lease is dropped.
    pub fn grant(&self, bytes: usize) -> Lease {
        self.pool.grant(bytes)
    }

    /// Bytes DataFusion holds reserved right now. Zero when no query is running.
    pub fn reserved(&self) -> usize {
        use datafusion::execution::memory_pool::MemoryPool;
        self.pool.reserved()
    }

    pub fn pool(&self) -> &Arc<LeasePool> {
        &self.pool
    }

    /// Why spill is off, if it is.
    pub fn spill_disabled_reason(&self) -> Option<&str> {
        self.spill_reason.as_deref()
    }

    pub fn spill_stats(&self) -> SpillStats {
        self.spill.as_ref().map(|s| s.stats()).unwrap_or_default()
    }

    pub fn spill(&self) -> Option<&Arc<EncryptedTempFiles>> {
        self.spill.as_ref()
    }

    pub fn session(&self) -> Session {
        let mut config = SessionConfig::new()
            .with_target_partitions(self.target_partitions)
            // `information_schema` would let SQL list every table of every catalog.
            .with_batch_size(self.batch_size)
            .with_information_schema(false);
        // Each spillable sort keeps this much back for its in-memory merge. The default,
        // 10 MB per partition, would swallow a 32 MiB lease on four partitions before a
        // row was read; the lease is small by design (section 14.5).
        config.options_mut().execution.sort_spill_reservation_bytes = SORT_MERGE_RESERVATION;
        let ctx = SessionContext::new_with_config_rt(config, Arc::clone(&self.runtime));
        ctx.register_udf(natural_udf());
        Session {
            ctx,
            tables: Mutex::new(BTreeMap::new()),
            pool: Arc::clone(&self.pool),
            max_partitions: self.target_partitions,
        }
    }
}

/// One tab's tables and queries.
pub struct Session {
    ctx: SessionContext,
    tables: Mutex<BTreeMap<String, TableInfo>>,
    pool: Arc<LeasePool>,
    max_partitions: usize,
}

fn locked() -> SQLOptions {
    SQLOptions::new()
        .with_allow_ddl(false)
        .with_allow_dml(false)
        .with_allow_statements(false)
}

fn check_name(name: &str) -> Result<(), Failure> {
    if name.is_empty() || name.chars().count() > MAX_NAME_LEN || name.chars().any(char::is_control)
    {
        return Err(Failure::invalid(
            "A table name must be 1 to 128 characters, with no control characters.",
        ));
    }
    Ok(())
}

pub(crate) fn info_of(name: &str, provider: &dyn TableProvider, rows: Option<u64>) -> TableInfo {
    TableInfo {
        name: name.to_owned(),
        columns: provider
            .schema()
            .fields()
            .iter()
            .map(|field| WireColumn {
                name: field.name().clone(),
                type_name: field.data_type().to_string(),
            })
            .collect(),
        rows,
    }
}

impl Session {
    /// Make `provider` queryable as `name`, replacing a table of that name.
    pub fn register(
        &self,
        name: &str,
        provider: Arc<dyn TableProvider>,
        rows: Option<u64>,
    ) -> Result<TableInfo, Failure> {
        check_name(name)?;
        let info = info_of(name, provider.as_ref(), rows);
        // `bare`: the name is the name, not a dotted path to be split and case-folded.
        // DataFusion refuses a second table of the same name, so a re-registration
        // (a result re-run, a file re-read) replaces the first.
        let table = TableReference::bare(name.to_owned());
        self.ctx.deregister_table(table.clone())?;
        self.ctx.register_table(table, provider)?;
        self.tables
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(name.to_owned(), info.clone());
        Ok(info)
    }

    /// Read a local file or directory as a table.
    pub async fn register_file(
        &self,
        name: &str,
        path: &str,
        format: &FileFormat,
    ) -> Result<TableInfo, Failure> {
        check_name(name)?;
        let table = crate::files::open(&self.ctx, path, format).await?;
        self.register(name, table, None)
    }

    pub fn deregister(&self, name: &str) -> Result<(), Failure> {
        let removed = self
            .ctx
            .deregister_table(TableReference::bare(name.to_owned()))?;
        self.tables
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(name);
        match removed {
            Some(_) => Ok(()),
            None => Err(Failure::invalid(format!(
                "There is no table called {name}."
            ))),
        }
    }

    pub fn tables(&self) -> Vec<TableInfo> {
        self.tables
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect()
    }

    /// How many partitions a statement planned now gets: what the leases granted so far pay
    /// for (see [`LEASE_PER_PARTITION`]), between one and the engine's own count.
    pub fn partitions_now(&self) -> usize {
        (self.pool.limit() / LEASE_PER_PARTITION).clamp(1, self.max_partitions)
    }

    /// Plan `sql` under the locked options and start executing it.
    pub async fn query(&self, sql: &str) -> Result<SendableRecordBatchStream, Failure> {
        // Same session, same tables and functions, planned for fewer partitions: the state
        // is a snapshot that shares the catalog.
        let mut state = self.ctx.state();
        state.config_mut().options_mut().execution.target_partitions = self.partitions_now();
        let ctx = SessionContext::new_with_state(state);
        let frame = ctx.sql_with_options(sql, locked()).await?;
        Ok(frame.execute_stream().await?)
    }
}
