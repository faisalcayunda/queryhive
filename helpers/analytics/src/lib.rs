//! `queryhive-analytics`: SQL over query results and local files, in its own process.
//!
//! The app does not link DataFusion. It starts this helper (inside a kernel sandbox, from a
//! component it has downloaded and verified), tells it which results and files exist, and
//! sends SQL; the helper sends the answer back as chunks in the store's own encodings
//! (blueprint `docs/architecture/blueprints/fase-6-data-plane.md`, section 14).
//!
//! # Who holds memory
//!
//! The app does, and nobody else. Its memory budget is one number (O-12), and for each query
//! it *leases* the helper a share of it. [`pool::LeasePool`] makes DataFusion stay inside the
//! sum of the leases that are running; what does not fit is spilled or the query fails with
//! "the result budget" message. Results registered for SQL stay in the app and are pulled one
//! chunk at a time ([`remote_table`]), so the helper never holds a whole result. Memory the
//! lease does not cover is the batches in transit (bounded by request credits), DataFusion's
//! small structures, and the process's own base; the benchmark sums the app's and the
//! helper's `phys_footprint` for exactly that reason.
//!
//! # Who writes to disk
//!
//! Only the helper's spill, and that is encrypted ([`spill`]): AES-256-GCM, a key made when
//! the process starts and never sent anywhere, files unlinked before their first byte. CSV
//! and Parquet inputs are read, never written. The kernel enforces the same (section 14.9),
//! so the rule does not depend on the SQL lock holding.
//!
//! # The SQL surface
//!
//! `SELECT` and `EXPLAIN`, nothing that changes anything: no DDL, no DML (so no `COPY`), no
//! `SET`, and no reading a path that was not registered ([`session`]). The one function
//! added is `qh_natural(text)`, the grid's natural sort key ([`udf`]).
//!
//! # Layout
//!
//! | Module | Owns |
//! |---|---|
//! | [`server`] | the protocol loop, threads, cancel, chunk requests |
//! | [`session`] | the engine, sessions, the locked SQL options, error classification |
//! | [`pool`] | the lease memory pool |
//! | [`spill`] | encrypted operator spill |
//! | [`remote_table`] | a result in the app as a table |
//! | [`files`] | CSV and Parquet as tables |
//! | [`output`] | batches to store-encoded chunks |
//! | [`udf`] | `qh_natural` |
#![forbid(unsafe_code)]

pub mod confinement;
pub mod files;
pub mod output;
pub mod pool;
pub mod remote_table;
pub mod server;
pub mod session;
pub mod spill;
pub mod udf;
